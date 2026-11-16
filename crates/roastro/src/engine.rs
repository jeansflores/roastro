//! The engine owns the database and the recorder on one thread. Readings from
//! the PLC and requests from the API arrive on a single queue, so everything
//! happens in one order and the live stream never contradicts the database.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use plc::Reading;
use recorder::{Input, Millis, Sample};
use roast::RoR;
use store::RoastId;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::api::ApiError;
use crate::record::Recording;
use crate::wire::{
    Columns, Link, LinkState, Live, NextRoastOut, RoastDoc, RorSettings, SampleOut, Snapshot,
    samples_with_ror,
};

/// Readings kept for the live screen while waiting for Charge.
const PRE_CHARGE: Duration = Duration::from_secs(60);

/// A reply channel for one request.
pub type Reply<T> = oneshot::Sender<Result<T, ApiError>>;

/// Everything the engine is asked to do.
pub enum Request {
    /// One poll attempt.
    Poll {
        /// Wall-clock time of the attempt.
        at: Millis,
        /// The reading, or why there is none.
        result: Result<Reading, String>,
        /// Wait before the next attempt.
        retry_in: Duration,
    },
    /// Join the live stream: a snapshot plus every message after it.
    Subscribe(Reply<(String, broadcast::Receiver<Arc<str>>)>),
    /// The Charge button.
    Charge(Reply<RoastDoc>),
    /// The Drop button.
    Drop(Reply<RoastDoc>),
    /// One roast.
    Roast(RoastId, Reply<RoastDoc>),
    /// One roast's readings in columns.
    Samples(RoastId, Reply<Columns>),
}

/// The engine's state. Lives on its own thread; see [`Engine::run`].
pub struct Engine {
    recording: Recording,
    /// The roast on the live screen: Recording, or the last one until the next Charge.
    shown: Option<RoastId>,
    /// The last minute of readings, for the preheat trace.
    recent: VecDeque<SampleOut>,
    link: Link,
    live: broadcast::Sender<Arc<str>>,
    ror: RorSettings,
    /// Live rate of rise of the roast receiving samples.
    live_ror: Option<LiveRoR>,
    now: fn() -> Millis,
}

struct LiveRoR {
    charge_at: Millis,
    last_at: Millis,
    ror: RoR,
}

impl Engine {
    /// An engine around `recording`, publishing on `live`.
    pub fn new(
        recording: Recording,
        live: broadcast::Sender<Arc<str>>,
        ror: RorSettings,
        now: fn() -> Millis,
    ) -> Self {
        let shown = recording.current();
        let link = Link {
            state: LinkState::Down,
            since: now(),
            error: Some("not connected yet".into()),
            retry_at: None,
        };
        let mut engine = Self {
            recording,
            shown,
            recent: VecDeque::new(),
            link,
            live,
            ror,
            live_ror: None,
            now,
        };
        if let Some(id) = shown {
            engine.start_live_ror(id);
        }
        engine
    }

    /// Serves requests until every sender is gone.
    pub fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        while let Some(req) = requests.blocking_recv() {
            self.handle(req);
        }
    }

    /// Applies one request.
    pub fn handle(&mut self, req: Request) {
        match req {
            Request::Poll {
                at,
                result,
                retry_in,
            } => self.poll(at, result, retry_in),
            Request::Subscribe(reply) => {
                let rx = self.live.subscribe();
                let _ = reply.send(self.snapshot().map(|s| (Live::Snapshot(s).to_json(), rx)));
            }
            Request::Charge(reply) => {
                let at = (self.now)();
                let _ = reply.send(
                    self.apply(Input::ChargePressed(at))
                        .and_then(|()| self.current_doc()),
                );
            }
            Request::Drop(reply) => {
                let result = if self.recording.is_before_drop() {
                    let at = (self.now)();
                    self.apply(Input::DropPressed(at))
                        .and_then(|()| self.shown_doc())
                } else {
                    Err(ApiError::conflict(
                        "no_roast_before_drop",
                        "there is no roast between Charge and Drop",
                    ))
                };
                let _ = reply.send(result);
            }
            Request::Roast(id, reply) => {
                let _ = reply.send(self.doc(id));
            }
            Request::Samples(id, reply) => {
                let _ = reply.send(self.columns(id));
            }
        }
    }

    fn poll(&mut self, at: Millis, result: Result<Reading, String>, retry_in: Duration) {
        match result {
            Ok(reading) => {
                if self.link.state != LinkState::Connected {
                    self.set_link(Link {
                        state: LinkState::Connected,
                        since: at,
                        error: None,
                        retry_at: None,
                    });
                }
                let sample = Sample {
                    at,
                    values: reading.values,
                };
                if let Err(err) = self.apply(Input::Sample(sample.clone())) {
                    tracing::error!(%err, "could not record a reading");
                }
                let out = SampleOut {
                    at,
                    ror: self.live_rate(&sample),
                    values: sample.values,
                };
                self.remember(out.clone());
                self.publish(&Live::Sample(out));
            }
            Err(error) => {
                let since = if self.link.state == LinkState::Down {
                    self.link.since
                } else {
                    at
                };
                #[allow(clippy::cast_possible_truncation)] // backoff is seconds
                let retry_at = Some(at + retry_in.as_millis() as Millis);
                self.set_link(Link {
                    state: LinkState::Down,
                    since,
                    error: Some(error),
                    retry_at,
                });
                if let Err(err) = self.apply(Input::Missed(at)) {
                    tracing::error!(%err, "could not record a missed poll");
                }
            }
        }
    }

    /// Feeds the recorder and tells the live screens what changed.
    fn apply(&mut self, input: Input) -> Result<(), ApiError> {
        let changes = self.recording.handle(input).map_err(ApiError::internal)?;
        if let Some(id) = changes.started {
            self.shown = Some(id);
            self.start_live_ror(id);
        }
        if changes.closed.is_some() {
            self.live_ror = None;
        }
        if changes.roast_changed {
            if let Ok(doc) = self.shown_doc() {
                self.publish(&Live::Roast(doc));
            }
        }
        Ok(())
    }

    fn start_live_ror(&mut self, id: RoastId) {
        let charge_at = self.doc(id).ok().and_then(|d| d.charge_at);
        self.live_ror = charge_at.map(|charge_at| LiveRoR {
            charge_at,
            last_at: charge_at,
            ror: RoR::new(self.ror.window),
        });
    }

    /// The rate of rise at this reading, while a roast receives samples.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn live_rate(&mut self, s: &Sample) -> Option<f64> {
        let live = self.live_ror.as_mut()?;
        if s.at < live.charge_at {
            return None;
        }
        if s.at - live.last_at > self.ror.gap.as_millis() as Millis {
            live.ror.reset(); // a Gap
        }
        live.last_at = s.at;
        let roast_ms = (s.at - live.charge_at) as f64 * self.ror.time_scale;
        live.ror
            .add(Duration::from_millis(roast_ms as u64), *s.values.get("bt")?);
        live.ror.value()
    }

    fn remember(&mut self, s: SampleOut) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // one minute in ms
        let keep = (PRE_CHARGE.as_millis() as f64 / self.ror.time_scale) as Millis;
        let newest = s.at;
        self.recent.push_back(s);
        while self.recent.front().is_some_and(|f| newest - f.at > keep) {
            self.recent.pop_front();
        }
    }

    fn set_link(&mut self, link: Link) {
        if link != self.link {
            self.link = link;
            self.publish(&Live::Link(self.link.clone()));
        }
    }

    fn publish(&self, msg: &Live) {
        // No subscribers is fine: nobody has the screen open.
        let _ = self.live.send(msg.to_json().into());
    }

    fn snapshot(&self) -> Result<Snapshot, ApiError> {
        let store = self.recording.store();
        let roast = self.shown.map(|id| self.doc(id)).transpose()?;
        let samples = match &roast {
            Some(doc) => {
                let id = RoastId(doc.id);
                samples_with_ror(
                    store.samples(id).map_err(ApiError::internal)?,
                    doc.charge_at,
                    self.ror,
                )
            }
            None => self.recent.iter().cloned().collect(),
        };
        Ok(Snapshot {
            link: self.link.clone(),
            next_roast: NextRoastOut::load(store).map_err(ApiError::internal)?,
            roast,
            samples,
        })
    }

    fn doc(&self, id: RoastId) -> Result<RoastDoc, ApiError> {
        RoastDoc::load(self.recording.store(), id, (self.now)()).map_err(|e| match e {
            store::Error::NoRoast(_) => {
                ApiError::not_found("roast_not_found", format!("no roast {id}"))
            }
            other => ApiError::internal(other),
        })
    }

    fn shown_doc(&self) -> Result<RoastDoc, ApiError> {
        let id = self
            .shown
            .ok_or_else(|| ApiError::internal("no roast on screen"))?;
        self.doc(id)
    }

    fn current_doc(&self) -> Result<RoastDoc, ApiError> {
        let id = self
            .recording
            .current()
            .ok_or_else(|| ApiError::internal("Charge did not start a roast"))?;
        self.doc(id)
    }

    fn columns(&self, id: RoastId) -> Result<Columns, ApiError> {
        let doc = self.doc(id)?;
        let samples = self
            .recording
            .store()
            .samples(id)
            .map_err(ApiError::internal)?;
        Ok(Columns::from_rows(&samples_with_ror(
            samples,
            doc.charge_at,
            self.ror,
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use recorder::Rules;
    use store::Store;

    use super::*;

    /// A clock the tests move by hand.
    static NOW: AtomicI64 = AtomicI64::new(0);
    fn now() -> Millis {
        NOW.load(Ordering::SeqCst)
    }

    fn engine() -> (Engine, broadcast::Receiver<Arc<str>>) {
        let (live, rx) = broadcast::channel(4096);
        let recording =
            Recording::open(Store::open_in_memory().unwrap(), Rules::default()).unwrap();
        let ror = RorSettings {
            window: Duration::from_secs(30),
            gap: Duration::from_millis(2500),
            time_scale: 1.0,
        };
        (Engine::new(recording, live, ror, now), rx)
    }

    fn poll(e: &mut Engine, second: i64, bt: f64) {
        NOW.store(second * 1000, Ordering::SeqCst);
        let reading = Reading {
            values: [("bt".to_owned(), bt)].into(),
            latency: Duration::ZERO,
        };
        e.handle(Request::Poll {
            at: second * 1000,
            result: Ok(reading),
            retry_in: Duration::from_secs(1),
        });
    }

    fn ask<T>(e: &mut Engine, make: impl FnOnce(Reply<T>) -> Request) -> Result<T, ApiError> {
        let (tx, mut rx) = oneshot::channel();
        e.handle(make(tx));
        rx.try_recv().expect("the engine replies at once")
    }

    fn types(rx: &mut broadcast::Receiver<Arc<str>>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| {
                serde_json::from_str::<serde_json::Value>(&m).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn streams_readings_and_the_roast_through_charge_and_drop() {
        let (mut e, mut rx) = engine();
        for s in 0..10 {
            poll(&mut e, s, 120.0);
        }
        assert_eq!(
            types(&mut rx).iter().filter(|t| *t == "link").count(),
            1,
            "connected once"
        );

        let (snapshot, _sub) = ask(&mut e, Request::Subscribe).unwrap();
        let snap: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(snap["type"], "snapshot");
        assert_eq!(snap["data"]["link"]["state"], "connected");
        assert!(snap["data"]["roast"].is_null(), "no roast before Charge");
        assert_eq!(
            snap["data"]["samples"].as_array().unwrap().len(),
            10,
            "the preheat trace"
        );

        let refused = ask(&mut e, Request::Drop).map(|_| ()).map_err(|e| e.code());
        assert_eq!(refused, Err("no_roast_before_drop"));
        let doc = ask(&mut e, Request::Charge).unwrap();
        assert_eq!((doc.number, doc.state), (1, "recording"));
        assert!(
            types(&mut rx).contains(&"roast".to_owned()),
            "Charge is announced"
        );

        for s in 10..40 {
            poll(&mut e, s, 120.0 + f64::from(u32::try_from(s).unwrap()));
        }
        let last: serde_json::Value =
            serde_json::from_str(&std::iter::from_fn(|| rx.try_recv().ok()).last().unwrap())
                .unwrap();
        assert_eq!(last["type"], "sample");
        assert!(
            (last["data"]["ror"].as_f64().unwrap() - 60.0).abs() < 1e-6,
            "1 °C/s is 60 °C/min"
        );

        let doc = ask(&mut e, Request::Drop).unwrap();
        assert_eq!(doc.state, "done");
        assert!(doc.metrics.duration_ms.is_some());
        let columns = ask(&mut e, |r| Request::Samples(RoastId(doc.id), r)).unwrap();
        assert_eq!(columns.at.len(), columns.values["bt"].len());
        assert!(
            columns.ror.iter().flatten().count() > 0,
            "rate of rise stored with the columns"
        );
    }

    #[test]
    fn a_dropped_link_is_announced_with_its_retry() {
        let (mut e, mut rx) = engine();
        poll(&mut e, 0, 120.0);
        e.handle(Request::Poll {
            at: 1000,
            result: Err("connection refused".into()),
            retry_in: Duration::from_secs(2),
        });
        let msgs: Vec<serde_json::Value> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| serde_json::from_str(&m).unwrap())
            .collect();
        let down = msgs.iter().rev().find(|m| m["type"] == "link").unwrap();
        assert_eq!(down["data"]["state"], "down");
        assert_eq!(down["data"]["retry_at"], 3000);
        assert_eq!(down["data"]["error"], "connection refused");
    }

    #[test]
    fn an_unknown_roast_is_a_404() {
        let (mut e, _rx) = engine();
        let missing = ask(&mut e, |r| Request::Roast(RoastId(uuid::Uuid::now_v7()), r));
        assert_eq!(
            missing.map(|_| ()).map_err(|e| e.code()),
            Err("roast_not_found")
        );
    }
}
