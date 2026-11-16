//! SPIKE: everything that is not UI runs on a Tokio thread and talks to the
//! GPUI side through channels. Optionally it also runs the simulator, so one
//! executable shows a live roast on a machine without anything else installed.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plc::{Client, Config, Poller};
use recorder::{Input, Output, Recorder, Rules, Sample};
use regmap::Map;
use roast::{EventKind, RoR};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// One reading as the chart needs it.
#[derive(Debug, Clone, Copy)]
pub struct Reading {
    pub at: i64,
    pub bt: Option<f64>,
    pub et: Option<f64>,
    pub ror: Option<f64>,
}

/// From the backend to the UI.
#[derive(Debug, Clone)]
pub enum Feed {
    Sample(Reading),
    Started(i64),
    Event(EventKind, i64),
    Closed,
    Link(bool),
}

/// From the UI to the backend.
#[derive(Debug, Clone, Copy)]
pub enum Cmd {
    Charge,
    Drop,
}

pub struct Settings {
    pub url: String,
    pub map: Map,
    pub time_scale: f64,
    pub embedded_sim: bool,
    pub sim_port: u16,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Starts the backend on its own thread.
pub fn start(settings: Settings, feed: UnboundedSender<Feed>, cmds: UnboundedReceiver<Cmd>) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
        rt.block_on(run(settings, feed, cmds));
    });
}

async fn run(settings: Settings, feed: UnboundedSender<Feed>, mut cmds: UnboundedReceiver<Cmd>) {
    let map = Arc::new(settings.map);
    if settings.embedded_sim {
        start_sim(map.clone(), settings.time_scale, settings.sim_port).await;
    }
    let client = Client::new(Config::new(&settings.url, map)).expect("PLC url");
    let mut polls = Poller::default().run(client);
    let rules = Rules { time_scale: settings.time_scale, ..Rules::default() };
    let mut recorder = Recorder::new(rules);
    let mut live: Option<(i64, i64, RoR)> = None; // charge_at, last_at, rate of rise
    let mut connected = None;

    loop {
        let input = tokio::select! {
            ev = polls.recv() => {
                let Some(ev) = ev else { return };
                let at = now_ms();
                let up = ev.result.is_ok();
                if connected != Some(up) {
                    connected = Some(up);
                    let _ = feed.send(Feed::Link(up));
                }
                match ev.result {
                    Ok(reading) => {
                        let s = Sample { at, values: reading.values };
                        let (bt, et) = (s.values.get("bt").copied(), s.values.get("et").copied());
                        let ror = live.as_mut().and_then(|(charge, last, ror)| {
                            if at - *last > 2500 { ror.reset(); }
                            *last = at;
                            let roast_ms = ((at - *charge).max(0) as f64 * settings.time_scale) as u64;
                            ror.add(Duration::from_millis(roast_ms), bt?);
                            ror.value()
                        });
                        let _ = feed.send(Feed::Sample(Reading { at, bt, et, ror }));
                        Input::Sample(s)
                    }
                    Err(_) => Input::Missed(at),
                }
            }
            cmd = cmds.recv() => match cmd {
                Some(Cmd::Charge) => Input::ChargePressed(now_ms()),
                Some(Cmd::Drop) => Input::DropPressed(now_ms()),
                None => return,
            },
        };
        for out in recorder.handle(input) {
            match out {
                Output::Started { charge_at, .. } => {
                    live = Some((charge_at, charge_at, RoR::new(Duration::from_secs(30))));
                    let _ = feed.send(Feed::Started(charge_at));
                }
                Output::Event { kind, at, .. } => {
                    let _ = feed.send(Feed::Event(kind, at));
                }
                Output::Closed(_) => {
                    live = None;
                    let _ = feed.send(Feed::Closed);
                }
                Output::Sample(_) => {}
            }
        }
    }
}

/// The simulator, in process: Modbus on 127.0.0.1:5502, physics at 10 Hz,
/// autopilot on, warm-up skipped.
async fn start_sim(map: Arc<Map>, speed: f64, port: u16) {
    let sim = Arc::new(Mutex::new(sim::Sim::new(1)));
    sim.lock().unwrap_or_else(PoisonError::into_inner).preheat(195.0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("a free port for the embedded simulator");
    let service = sim::ModbusService::new(map, sim.clone());
    tokio::spawn(async move {
        let on_connected = |stream, peer| {
            let svc = service.for_peer(peer);
            async move { tokio_modbus::server::tcp::accept_tcp_connection(stream, peer, move |_| Ok(Some(svc.clone()))) }
        };
        let _ = tokio_modbus::server::tcp::Server::new(listener).serve(&on_connected, |_| {}).await;
    });
    tokio::spawn(async move {
        let mut pilot = sim::Autopilot::default();
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut acc = 0.0;
        loop {
            tick.tick().await;
            let mut s = sim.lock().unwrap_or_else(PoisonError::into_inner);
            s.step(0.1 * speed);
            acc += 0.1 * speed;
            if acc >= 1.0 {
                acc = 0.0;
                pilot.tick(&mut s);
            }
        }
    });
}

/// A reference roast: one whole roast of the simulator, run ahead of time.
/// (roast seconds, BT, ET)
pub fn reference_curve() -> Vec<(f64, f64, f64)> {
    let mut s = sim::Sim::new(42);
    let mut pilot = sim::Autopilot::default();
    let mut out = Vec::new();
    let mut charge = None;
    for second in 0..4 * 3600 {
        pilot.tick(&mut s);
        s.step(1.0);
        let sn = s.snapshot();
        if sn.loaded && charge.is_none() {
            charge = Some(f64::from(second));
        }
        if let Some(c) = charge {
            out.push((f64::from(second) - c, sn.bt, sn.et));
            if !sn.loaded {
                break;
            }
        }
    }
    out
}
