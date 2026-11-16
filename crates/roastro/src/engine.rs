//! The engine owns the database and the recorder on one thread. Readings from
//! the PLC and requests from the API arrive on a single queue, so everything
//! happens in one order and the live stream never contradicts the database.

mod edits;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use plc::Reading;
use recorder::{Input, Millis, Sample};
use regmap::Map;
use roast::RoR;
use store::RoastId;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::api::ApiError;
use crate::record::Recording;
use crate::wire::{
    CoffeeIn, CoffeeOut, CoffeePatch, Columns, EventIn, Link, LinkDropOut, LinkState, Live,
    MachineIn, MachineOut, NextRoastIn, NextRoastOut, NoteIn, NoteOut, NotePatch, RoastDoc,
    RoastPage, RoastPatchIn, RoastQueryIn, RorSettings, SampleOut, Snapshot, SummaryOut, TestOut,
    samples_with_ror,
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
        result: Result<Reading, Failure>,
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
    /// Dial the machine now instead of waiting for the next attempt.
    RetryNow(Reply<()>),
    /// The latest link drops, newest first.
    LinkDrops(Reply<Vec<LinkDropOut>>),
    /// The machine in use.
    Machine(Reply<MachineOut>),
    /// Set up the machine (or use the Demo one) and connect to it. Refused
    /// while a roast is recording.
    SetMachine(MachineIn, Reply<MachineOut>),
    /// One read of settings not saved yet, to see that they work.
    TestMachine(MachineIn, Reply<TestOut>),
}

/// Why a poll brought no reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The error, as the PLC client reports it.
    pub error: String,
    /// What to check, in plain words.
    pub advice: String,
}

impl From<&plc::Error> for Failure {
    fn from(e: &plc::Error) -> Self {
        Self {
            error: e.to_string(),
            advice: e.trouble().advice().to_owned(),
        }
    }
}

/// Which machine to read.
#[derive(Debug, Clone)]
pub enum Target {
    /// A PLC at an address, read with a register map.
    Machine {
        /// `tcp://host:port`.
        url: String,
        /// Its register map.
        map: Arc<Map>,
    },
    /// The built-in simulator.
    Demo,
}

/// What the engine asks of the task that polls the machine.
pub enum LinkCommand {
    /// Drop the connection and dial again now.
    RetryNow,
    /// Read this machine from now on.
    Connect(Target),
    /// Read this machine once and answer, without changing the connection.
    Test(Target, Reply<TestOut>),
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
    /// The drop going on, once the machine stopped answering.
    open_drop: Option<i64>,
    /// The task polling the machine, when there is one.
    link_control: Option<mpsc::Sender<LinkCommand>>,
    /// The machine in use.
    machine: MachineOut,
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
        let machine = recording.store().machine().map_or_else(
            |err| {
                tracing::error!(%err, "could not read the machine");
                MachineOut {
                    name: String::new(),
                    address: String::new(),
                    model: String::new(),
                    model_title: String::new(),
                    demo: false,
                }
            },
            MachineOut::from,
        );
        let link = Link {
            state: LinkState::Connecting,
            since: now(),
            error: None,
            retry_at: None,
            advice: None,
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
            open_drop: None,
            link_control: None,
            machine,
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

    /// Lets the engine ask the polling task to dial again.
    #[must_use]
    pub fn with_link_control(mut self, control: mpsc::Sender<LinkCommand>) -> Self {
        self.link_control = Some(control);
        self
    }

    /// Nothing to read until a machine is set up.
    #[must_use]
    pub fn waiting_for_setup(mut self) -> Self {
        self.link.state = LinkState::NotSetUp;
        self
    }

    /// Where `body` points, checked.
    fn target(body: &MachineIn) -> Result<Target, ApiError> {
        if body.demo {
            return Ok(Target::Demo);
        }
        let url = crate::machine::normalize_address(&body.address).ok_or_else(|| {
            ApiError::invalid("invalid_address", plc::Trouble::BadAddress.advice())
        })?;
        let map = crate::machine::model_map(&body.model)
            .map_err(|e| ApiError::invalid("unknown_model", e.to_string()))?;
        Ok(Target::Machine {
            url,
            map: Arc::new(map),
        })
    }

    fn set_machine(&mut self, body: &MachineIn) -> Result<MachineOut, ApiError> {
        if self.recording.current().is_some() {
            return Err(ApiError::conflict(
                "roast_recording",
                "Finish the roast before changing the machine.",
            ));
        }
        let target = Self::target(body)?;
        let store = self.recording.store_mut();
        let saved = match &target {
            Target::Demo => store.use_demo(crate::machine::SIMULATOR),
            Target::Machine { url, .. } => {
                let name = body.name.trim();
                let name = if name.is_empty() {
                    crate::machine::model_title(&body.model)
                } else {
                    name
                };
                store.save_machine(name, url, &body.model)
            }
        }
        .map_err(ApiError::internal)?;
        self.machine = MachineOut::from(saved);
        if let Some(control) = &self.link_control
            && control.try_send(LinkCommand::Connect(target)).is_err()
        {
            tracing::error!("the polling task is not listening");
        }
        // What was on screen belongs to the machine before.
        self.shown = None;
        self.live_ror = None;
        self.recent.clear();
        self.open_drop = None;
        self.link = Link {
            state: LinkState::Connecting,
            since: (self.now)(),
            error: None,
            retry_at: None,
            advice: None,
        };
        self.publish(Live::Machine(self.machine.clone()));
        let snapshot = self.snapshot()?;
        self.publish(Live::Snapshot(snapshot));
        Ok(self.machine.clone())
    }

    fn test_machine(&self, body: &MachineIn, reply: Reply<TestOut>) {
        let target = match Self::target(body) {
            Ok(target) => target,
            Err(err) => {
                let _ = reply.send(Err(err));
                return;
            }
        };
        let Some(control) = &self.link_control else {
            let _ = reply.send(Err(ApiError::conflict(
                "no_link",
                "there is no machine connection to test with",
            )));
            return;
        };
        if let Err(
            mpsc::error::TrySendError::Full(LinkCommand::Test(_, reply))
            | mpsc::error::TrySendError::Closed(LinkCommand::Test(_, reply)),
        ) = control.try_send(LinkCommand::Test(target, reply))
        {
            let _ = reply.send(Err(ApiError::conflict(
                "busy",
                "Roastro is busy; try again",
            )));
        }
    }

    /// Dials the machine now; the link shows Connecting until it answers.
    fn retry_now(&mut self) -> Result<(), ApiError> {
        let sent = self
            .link_control
            .as_ref()
            .is_some_and(|c| c.try_send(LinkCommand::RetryNow).is_ok());
        if !sent {
            return Err(ApiError::conflict(
                "no_link",
                "there is no machine connection to retry",
            ));
        }
        let mut link = self.link.clone();
        link.state = LinkState::Connecting;
        link.retry_at = None;
        self.set_link(link);
        Ok(())
    }

    fn link_drops(&self) -> Result<Vec<LinkDropOut>, ApiError> {
        let store = self.recording.store();
        store
            .link_drops(50)
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|d| {
                let roast_number = match d.roast {
                    Some(id) => Some(store.roast(id).map_err(ApiError::internal)?.number),
                    None => None,
                };
                Ok(LinkDropOut {
                    started_at: d.started_at,
                    ended_at: d.ended_at,
                    error: d.error,
                    roast_number,
                })
            })
            .collect()
    }

    fn poll(&mut self, at: Millis, result: Result<Reading, Failure>, retry_in: Duration) {
        match result {
            Ok(reading) => {
                if self.link.state != LinkState::Connected {
                    if let Some(drop) = self.open_drop.take()
                        && let Err(err) = self.recording.store_mut().end_link_drop(drop, at)
                    {
                        tracing::error!(%err, "could not log the end of a link drop");
                    }
                    self.set_link(Link {
                        state: LinkState::Connected,
                        since: at,
                        error: None,
                        retry_at: None,
                        advice: None,
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
            Err(failure) => {
                let since = if self.link.state == LinkState::Down {
                    self.link.since
                } else {
                    at
                };
                if self.link.state == LinkState::Connected {
                    let roast = self.recording.current();
                    match self
                        .recording
                        .store_mut()
                        .begin_link_drop(at, &failure.error, roast)
                    {
                        Ok(drop) => self.open_drop = Some(drop),
                        Err(err) => tracing::error!(%err, "could not log a link drop"),
                    }
                }
                #[allow(clippy::cast_possible_truncation)] // backoff is seconds
                let retry_at = Some(at + retry_in.as_millis() as Millis);
                self.set_link(Link {
                    state: LinkState::Down,
                    since,
                    error: Some(failure.error),
                    retry_at,
                    advice: Some(failure.advice),
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
            machine: self.machine.clone(),
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
    use crate::wire::{
        CoffeeIn, EventIn, LinkDropOut, MachineIn, NextRoastIn, NoteIn, RoastPatchIn, RoastQueryIn,
    };

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
            result: Err(Failure {
                error: "connection refused".into(),
                advice: "Check the cable.".into(),
            }),
            retry_in: Duration::from_secs(2),
        });
        let msgs: Vec<serde_json::Value> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| serde_json::to_value(&*m).unwrap())
            .collect();
        let down = msgs.iter().rev().find(|m| m["type"] == "link").unwrap();
        assert_eq!(down["data"]["state"], "down");
        assert_eq!(down["data"]["retry_at"], 3000);
        assert_eq!(down["data"]["error"], "connection refused");
        assert_eq!(down["data"]["advice"], "Check the cable.");
    }

    fn fail(e: &mut Engine, second: i64, error: &str) {
        e.handle(Request::Poll {
            at: second * 1000,
            result: Err(Failure {
                error: error.into(),
                advice: String::new(),
            }),
            retry_in: Duration::from_secs(1),
        });
    }

    #[test]
    fn logs_a_drop_only_after_the_machine_answered() {
        let (e, _rx) = engine();
        let (control, mut commands) = mpsc::channel(4);
        let mut e = e.with_link_control(control);
        fail(&mut e, 0, "refused at startup");
        assert_eq!(
            edit(&mut e, Edit::LinkDrops).unwrap(),
            [],
            "a machine that never answered did not drop"
        );
        for s in 1..5 {
            poll(&mut e, s, 120.0);
        }
        let roast = ask(&mut e, Request::Charge).unwrap();
        poll(&mut e, 5, 121.0);
        fail(&mut e, 6, "timed out");
        fail(&mut e, 7, "timed out again");
        poll(&mut e, 9, 125.0);
        assert_eq!(
            edit(&mut e, Edit::LinkDrops).unwrap(),
            [LinkDropOut {
                started_at: 6000,
                ended_at: Some(9000),
                error: "timed out".into(),
                roast_number: Some(roast.number),
            }],
            "one drop, its first error, the roast it hit"
        );

        fail(&mut e, 10, "refused");
        assert_eq!(edit(&mut e, Edit::RetryNow), Ok(()));
        assert!(matches!(commands.try_recv(), Ok(LinkCommand::RetryNow)));
        let (snapshot, _sub) = ask(&mut e, Request::Subscribe).unwrap();
        let snap = serde_json::to_value(&*snapshot).unwrap();
        assert_eq!(snap["data"]["link"]["state"], "connecting");
    }

    #[test]
    fn sets_up_the_machine_only_between_roasts() {
        let (e, mut rx) = engine();
        let (control, mut commands) = mpsc::channel(4);
        let mut e = e.with_link_control(control).waiting_for_setup();
        let (snapshot, _sub) = ask(&mut e, Request::Subscribe).unwrap();
        let snap = serde_json::to_value(&*snapshot).unwrap();
        assert_eq!(snap["data"]["link"]["state"], "not_set_up");
        assert_eq!(snap["data"]["machine"]["address"], "");

        let set = |e: &mut Engine, address: &str, model: &str| {
            let body = MachineIn {
                name: "Caloratto".into(),
                address: address.into(),
                model: model.into(),
                demo: false,
            };
            edit(e, |r| Edit::SetMachine(body, r))
        };
        assert_eq!(
            set(&mut e, "192.168 5.11", "caloratto-2.0").map(|_| ()),
            Err("invalid_address")
        );
        assert_eq!(
            set(&mut e, "192.168.5.11", "espresso").map(|_| ()),
            Err("unknown_model")
        );
        let machine = set(&mut e, "192.168.5.11", "caloratto-2.0").unwrap();
        assert_eq!(
            (machine.address.as_str(), machine.model_title.as_str()),
            ("tcp://192.168.5.11:502", "Carmomaq Caloratto 2.0")
        );
        assert!(matches!(
            commands.try_recv(),
            Ok(LinkCommand::Connect(Target::Machine { ref url, .. })) if url == "tcp://192.168.5.11:502"
        ));
        assert!(
            types(&mut rx).ends_with(&["machine".to_owned(), "snapshot".to_owned()]),
            "the screens learn of the new machine"
        );

        let probe = MachineIn {
            demo: true,
            ..MachineIn::default()
        };
        let (reply, _answer) = oneshot::channel();
        e.handle(Request::Edit(Edit::TestMachine(probe, reply)));
        assert!(
            matches!(commands.try_recv(), Ok(LinkCommand::Test(Target::Demo, _))),
            "a test goes to the polling task, the machine in use stays"
        );
        assert_eq!(edit(&mut e, Edit::Machine).unwrap(), machine);

        for s in 0..3 {
            poll(&mut e, s, 120.0);
        }
        ask(&mut e, Request::Charge).unwrap();
        assert_eq!(
            set(&mut e, "192.168.5.12", "caloratto-2.0").map(|_| ()),
            Err("roast_recording")
        );
    }

    #[test]
    fn retry_now_needs_a_machine_connection() {
        let (mut e, _rx) = engine();
        assert_eq!(edit(&mut e, Edit::RetryNow), Err("no_link"));
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
