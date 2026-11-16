//! The engine owns the database and the recorder on one thread. Readings from
//! the PLC and requests from the API arrive on a single queue, so everything
//! happens in one order and the live stream never contradicts the database.

mod edits;

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
    CoffeeIn, CoffeeOut, CoffeePatch, Columns, EventIn, Link, LinkState, Live, NextRoastIn,
    NextRoastOut, NoteIn, NoteOut, NotePatch, RoastDoc, RoastPage, RoastPatchIn, RoastQueryIn,
    RorSettings, SampleOut, Snapshot, SummaryOut, samples_with_ror,
};
use uuid::Uuid;

/// Readings kept for the live screen while waiting for Charge.
const PRE_CHARGE: Duration = Duration::from_secs(60);

/// The span over which a falling `RoR` counts as a crash or a swing.
const RAPID_FALL: Duration = Duration::from_secs(30);

/// The live stream as received by one subscriber.
pub type LiveReceiver = broadcast::Receiver<Arc<Live>>;

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
    Subscribe(Reply<(Arc<Live>, LiveReceiver)>),
    /// The Charge button.
    Charge(Reply<RoastDoc>),
    /// The Drop button.
    Drop(Reply<RoastDoc>),
    /// One roast.
    Roast(RoastId, Reply<RoastDoc>),
    /// One roast's readings in columns.
    Samples(RoastId, Reply<Columns>),
    /// Something a person changes: see [`Edit`].
    Edit(Edit),
}

/// Changes people make through the API, each with its reply.
pub enum Edit {
    /// Mark or move an event.
    SetEvent(RoastId, String, EventIn, Reply<RoastDoc>),
    /// Remove a manual event.
    DeleteEvent(RoastId, String, Reply<RoastDoc>),
    /// Add a note.
    AddNote(RoastId, NoteIn, Reply<NoteOut>),
    /// Change a note.
    PatchNote(RoastId, Uuid, NotePatch, Reply<NoteOut>),
    /// Remove a note.
    DeleteNote(RoastId, Uuid, Reply<()>),
    /// Change a roast's fields.
    PatchRoast(RoastId, RoastPatchIn, Reply<RoastDoc>),
    /// Hide a roast.
    Discard(RoastId, Reply<RoastDoc>),
    /// Bring a discarded roast back.
    Restore(RoastId, Reply<RoastDoc>),
    /// Close the recording roast without a Drop.
    Close(RoastId, Reply<RoastDoc>),
    /// Read the Next roast.
    GetNextRoast(Reply<NextRoastOut>),
    /// Replace the Next roast.
    PutNextRoast(NextRoastIn, Reply<NextRoastOut>),
    /// List coffees.
    Coffees(bool, Reply<Vec<CoffeeOut>>),
    /// Add a coffee.
    AddCoffee(CoffeeIn, Reply<CoffeeOut>),
    /// Rename or archive a coffee.
    PatchCoffee(Uuid, CoffeePatch, Reply<CoffeeOut>),
    /// A page of roasts.
    Roasts(RoastQueryIn, Reply<RoastPage>),
}

/// The engine's state. Lives on its own thread; see [`Engine::run`].
pub struct Engine {
    recording: Recording,
    /// The roast on the live screen: Recording, or the last one until the next Charge.
    shown: Option<RoastId>,
    /// The last minute of readings, for the preheat trace.
    recent: VecDeque<SampleOut>,
    link: Link,
    live: broadcast::Sender<Arc<Live>>,
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
        live: broadcast::Sender<Arc<Live>>,
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
                let _ = reply.send(self.snapshot().map(|s| (Arc::new(Live::Snapshot(s)), rx)));
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
                let _ = reply.send(self.summarized(id));
            }
            Request::Samples(id, reply) => {
                let _ = reply.send(self.columns(id));
            }
            Request::Edit(edit) => self.edit(edit),
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
                self.publish(Live::Sample(out));
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
                self.publish(Live::Roast(doc));
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
            self.publish(Live::Link(self.link.clone()));
        }
    }

    fn publish(&self, msg: Live) {
        // No subscribers is fine: nobody has the screen open.
        let _ = self.live.send(Arc::new(msg));
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

    /// The roast with the figures read from its samples, once it is over.
    fn summarized(&self, id: RoastId) -> Result<RoastDoc, ApiError> {
        let mut doc = self.doc(id)?;
        if doc.state == "recording" {
            return Ok(doc);
        }
        let samples = self
            .recording
            .store()
            .samples(id)
            .map_err(ApiError::internal)?;
        let bt: Vec<(Millis, Option<f64>)> = samples
            .iter()
            .map(|s| (s.at, s.values.get("bt").copied()))
            .collect();
        let at = |kind: &str| doc.events.iter().find(|e| e.kind == kind).map(|e| e.at);
        let drop = at("drop");
        let drop_bt = drop.and_then(|d| {
            bt.iter()
                .rev()
                .find(|(t, v)| *t <= d && v.is_some())
                .and_then(|(_, v)| *v)
        });
        let ror = roast::ror_series(&bt, self.ror.window, self.ror.gap, self.ror.time_scale);
        let ror: Vec<(Millis, Option<f64>)> = bt.iter().map(|(t, _)| *t).zip(ror).collect();
        let end = drop.or_else(|| bt.last().map(|(t, _)| *t));
        let ror_fall = at("turning_point")
            .zip(end)
            .and_then(|(from, to)| roast::largest_fall(&ror, RAPID_FALL, from, to));
        doc.summary = Some(SummaryOut { drop_bt, ror_fall });
        Ok(doc)
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
    use std::cell::Cell;

    use recorder::Rules;
    use store::Store;

    use super::*;
    use crate::wire::{CoffeeIn, EventIn, NextRoastIn, NoteIn, RoastPatchIn, RoastQueryIn};

    thread_local! {
        /// A clock the tests move by hand, one per test thread so tests
        /// running in parallel do not move each other's clock.
        static NOW: Cell<Millis> = const { Cell::new(0) };
    }
    fn now() -> Millis {
        NOW.get()
    }

    fn engine() -> (Engine, LiveReceiver) {
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
        NOW.set(second * 1000);
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

    fn types(rx: &mut LiveReceiver) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| {
                serde_json::to_value(&*m).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn lists_finished_roasts_with_what_their_samples_say() {
        let (mut e, _rx) = engine();
        for s in 0..5 {
            poll(&mut e, s, 200.0);
        }
        let id = RoastId(ask(&mut e, Request::Charge).unwrap().id);
        // Down to a turning point at 90 °C, up at 0.4 °C/s (24 °C/min), then
        // flat for 40 s: the RoR crashes by 24 °C/min within 30 s.
        let mut bt = 200.0;
        for s in 5..200 {
            bt = match s {
                5..60 => bt - 2.0,
                60..150 => bt + 0.4,
                _ => bt,
            };
            poll(&mut e, s, bt);
        }
        ask(&mut e, Request::Drop).unwrap();
        assert!(
            ask(&mut e, |r| Request::Roast(id, r))
                .unwrap()
                .events
                .iter()
                .any(|e| e.kind == "turning_point"),
            "the turning point is computed"
        );

        let doc = ask(&mut e, |r| Request::Roast(id, r)).unwrap();
        let summary = doc.summary.expect("a finished roast is summarized");
        assert!((summary.drop_bt.unwrap() - bt).abs() < 1e-6, "BT at Drop");
        let fall = summary.ror_fall.unwrap();
        assert!(fall > 20.0, "the crash shows: {fall}");

        let mut page = |q: &str| {
            edit(&mut e, |r| {
                Edit::Roasts(
                    RoastQueryIn {
                        q: Some(q.into()),
                        ..RoastQueryIn::default()
                    },
                    r,
                )
            })
            .unwrap()
            .roasts
            .len()
        };
        assert_eq!(page("#1"), 1);
        assert_eq!(page("#2"), 0);
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
        let snap = serde_json::to_value(&*snapshot).unwrap();
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
            serde_json::to_value(&*std::iter::from_fn(|| rx.try_recv().ok()).last().unwrap())
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
            .map(|m| serde_json::to_value(&*m).unwrap())
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

    fn edit<T>(e: &mut Engine, make: impl FnOnce(Reply<T>) -> Edit) -> Result<T, &'static str> {
        ask(e, |r| Request::Edit(make(r))).map_err(|err| err.code())
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one scenario, read top to bottom
    fn enforces_the_contract_rules_on_edits() {
        let (mut e, _rx) = engine();
        for s in 0..5 {
            poll(&mut e, s, 120.0);
        }
        let id = RoastId(ask(&mut e, Request::Charge).unwrap().id);
        for s in 5..60 {
            poll(&mut e, s, 120.0 + f64::from(u32::try_from(s).unwrap()));
        }
        // A 20 s Gap, then readings again.
        for s in 80..100 {
            poll(&mut e, s, 200.0);
        }
        let set = |e: &mut Engine, kind: &str, at_s: i64| {
            let kind = kind.to_owned();
            edit(e, |r| {
                Edit::SetEvent(id, kind, EventIn { at: at_s * 1000 }, r)
            })
            .map(|_| ())
        };
        assert_eq!(set(&mut e, "dry_end", 200), Err("event_in_future"));
        NOW.set(100_500);
        assert_eq!(
            set(&mut e, "second_crack", 100),
            Ok(()),
            "\"now\": just after the last reading"
        );
        assert_eq!(
            edit(&mut e, |r| Edit::DeleteEvent(id, "second_crack".into(), r)).map(|_| ()),
            Ok(())
        );
        assert_eq!(
            set(&mut e, "dry_end", 70),
            Err("event_outside_samples"),
            "inside the Gap"
        );
        assert_eq!(set(&mut e, "first_crack", 30), Ok(()));
        assert_eq!(
            set(&mut e, "dry_end", 40),
            Err("event_out_of_order"),
            "after First crack"
        );
        assert_eq!(set(&mut e, "dry_end", 20), Ok(()));
        assert_eq!(set(&mut e, "crack", 20), Err("unknown_event_kind"));
        assert_eq!(
            edit(&mut e, |r| Edit::DeleteEvent(id, "charge".into(), r)).map(|_| ()),
            Err("event_not_deletable")
        );

        let note = edit(&mut e, |r| {
            Edit::AddNote(
                id,
                NoteIn {
                    at: 25_000,
                    text: " gas 70 ".into(),
                },
                r,
            )
        })
        .unwrap();
        assert_eq!(note.text, "gas 70", "trimmed");
        assert_eq!(
            edit(&mut e, |r| Edit::AddNote(
                id,
                NoteIn {
                    at: 25_000,
                    text: "  ".into()
                },
                r
            ))
            .map(|_| ()),
            Err("empty_note")
        );

        let bad_weight = RoastPatchIn {
            green_weight_g: Some(Some(0)),
            ..RoastPatchIn::default()
        };
        assert_eq!(
            edit(&mut e, |r| Edit::PatchRoast(id, bad_weight, r)).map(|_| ()),
            Err("invalid_weight")
        );
        let self_ref = RoastPatchIn {
            reference_id: Some(Some(id.0)),
            ..RoastPatchIn::default()
        };
        assert_eq!(
            edit(&mut e, |r| Edit::PatchRoast(id, self_ref, r)).map(|_| ()),
            Err("invalid_reference")
        );
        assert_eq!(
            edit(&mut e, |r| Edit::Discard(id, r)).map(|_| ()),
            Err("roast_recording")
        );

        // Drop may sit inside the Gap; the roast is then Done.
        let doc = edit(&mut e, |r| {
            Edit::SetEvent(id, "drop".into(), EventIn { at: 70_000 }, r)
        })
        .unwrap();
        assert_eq!(doc.state, "done");
        assert_eq!(
            edit(&mut e, |r| Edit::Close(id, r)).map(|_| ()),
            Err("roast_not_recording")
        );

        let coffee = edit(&mut e, |r| {
            Edit::AddCoffee(
                CoffeeIn {
                    name: "Bourbon".into(),
                    lot: " 2401 ".into(),
                    ..CoffeeIn::default()
                },
                r,
            )
        })
        .unwrap();
        assert_eq!(coffee.lot, "2401", "details are kept, trimmed");
        let coffees = edit(&mut e, |r| Edit::Coffees(false, r)).unwrap();
        assert_eq!(coffees[0].stats, None, "no figures before a finished roast");
        let patch = RoastPatchIn {
            coffee_id: Some(Some(coffee.id)),
            ..RoastPatchIn::default()
        };
        edit(&mut e, |r| Edit::PatchRoast(id, patch, r)).unwrap();
        let next = NextRoastIn {
            coffee_id: Some(coffee.id),
            green_weight_g: Some(4600),
            roasted_by: "Lucas".into(),
            reference_id: None,
        };
        let next = edit(&mut e, |r| Edit::PutNextRoast(next, r)).unwrap();
        assert_eq!(
            next.suggested_reference_id,
            Some(id.0),
            "the coffee's latest Done roast"
        );

        let page = edit(&mut e, |r| Edit::Roasts(RoastQueryIn::default(), r)).unwrap();
        assert_eq!((page.roasts.len(), page.next_before), (1, None));
        edit(&mut e, |r| Edit::Discard(id, r)).unwrap();
        assert_eq!(
            edit(&mut e, |r| Edit::Roasts(RoastQueryIn::default(), r))
                .unwrap()
                .roasts
                .len(),
            0,
            "discarded is hidden"
        );
        let (snapshot, _sub) = ask(&mut e, Request::Subscribe).unwrap();
        assert!(
            serde_json::to_value(&*snapshot).unwrap()["data"]["roast"].is_null(),
            "back to waiting"
        );
    }

    #[test]
    fn closes_a_recording_roast_without_drop() {
        let (mut e, _rx) = engine();
        poll(&mut e, 0, 120.0);
        let id = RoastId(ask(&mut e, Request::Charge).unwrap().id);
        poll(&mut e, 1, 120.0);
        assert_eq!(
            edit(&mut e, |r| Edit::Close(id, r)).unwrap().state,
            "incomplete"
        );
    }
}
