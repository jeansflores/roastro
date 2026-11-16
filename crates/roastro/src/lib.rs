//! Roastro's core: reads the PLC, records every roast, and serves whoever
//! asks, the native UI in the same process or the HTTP API (ADR 0003).
//!
//! [`start`] wires it up: the database and the recorder on the engine's own
//! thread, the PLC poller on the Tokio runtime, and a [`Running`] handle with
//! the engine's request queue and live broadcast. It never writes to the PLC.

pub mod alarm;
pub mod api;
pub mod engine;
pub mod export;
pub mod machine;
mod record;
pub mod report;
// The wire types mirror the JSON contract field for field; it is documented
// in "Contract between the backend and the web UI" (#8), not per field here.
#[allow(missing_docs)]
pub mod wire;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use plc::{Client, Poller};
use recorder::{Millis, Rules};
use regmap::Map;
pub use store::RoastId;
use store::Store;
use tokio::sync::{broadcast, mpsc};

use crate::api::ApiError;
use crate::engine::{Engine, Failure, LinkCommand, Reply, Request, Target};
use crate::record::Recording;
use crate::wire::{Live, RorSettings, TestOut};

/// Starts the built-in simulator if it is not running and says where it
/// answers (`tcp://127.0.0.1:port`): what the Demo machine reads.
pub type DemoHook =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send>> + Send + Sync>;

/// What [`start`] needs.
#[derive(Clone)]
pub struct Config {
    /// A PLC address that overrides the machine set up in the app, e.g.
    /// `tcp://192.168.5.11:502` (development and diagnostics).
    pub url: Option<String>,
    /// The register map for `url`.
    pub map: Map,
    /// Database file.
    pub db: PathBuf,
    /// Between polls.
    pub interval: Duration,
    /// The simulator's speed (1 on the machine).
    pub time_scale: f64,
    /// How to run the Demo machine, where there is a simulator to run.
    pub demo: Option<DemoHook>,
}

/// A started core: ask the engine through `requests`, follow it on `live`.
#[derive(Debug, Clone)]
pub struct Running {
    /// The engine's request queue.
    pub requests: mpsc::Sender<Request>,
    /// Every live message the engine publishes.
    pub live: broadcast::Sender<Arc<Live>>,
}

/// `%LOCALAPPDATA%\Roastro\roastro.db` on Windows, `~/.local/share/Roastro/roastro.db` on Linux.
///
/// # Errors
///
/// Fails on a system with no local data folder.
pub fn default_db() -> anyhow::Result<PathBuf> {
    let base = dirs::data_local_dir().context("no local data folder on this system")?;
    Ok(base.join("Roastro").join("roastro.db"))
}

/// Wall-clock epoch milliseconds.
#[must_use]
pub fn now_ms() -> Millis {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| {
        Millis::try_from(d.as_millis()).unwrap_or(Millis::MAX)
    })
}

/// Opens the database, resumes any roast left recording, starts polling the
/// PLC and the engine. Must be called inside a Tokio runtime.
///
/// # Errors
///
/// Fails if the database cannot be opened or the PLC URL is invalid.
pub fn start(config: Config) -> anyhow::Result<Running> {
    anyhow::ensure!(
        config.time_scale > 0.0 && config.time_scale.is_finite(),
        "time scale must be positive"
    );
    if let Some(dir) = config.db.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let store =
        Store::open(&config.db).with_context(|| format!("opening {}", config.db.display()))?;
    tracing::info!(db = %config.db.display(), schema = store.schema_version()?, "database ready");
    let set_up = store.machine()?;
    let target = match config.url {
        Some(url) => {
            let map = Arc::new(config.map);
            // Fail now on an address that can never work.
            connect(&url, &map)?;
            tracing::info!(%url, "reading the PLC given on the command line");
            Some(Target::Machine { url, map })
        }
        None if set_up.demo => Some(Target::Demo),
        None if !set_up.address.is_empty() => Some(Target::Machine {
            url: set_up.address.clone(),
            map: Arc::new(machine::model_map(&set_up.model)?),
        }),
        None => {
            tracing::info!("no machine set up yet");
            None
        }
    };
    let waiting = target.is_none();
    let rules = Rules {
        time_scale: config.time_scale,
        ..Rules::default()
    };
    let recording = Recording::open(store, rules)?;

    // The engine owns the database on its own thread (SQLite is synchronous).
    let (requests, inbox) = mpsc::channel(256);
    let (live, _) = broadcast::channel(256);
    let ror = RorSettings {
        window: Duration::from_secs(30),
        gap: rules.gap,
        time_scale: config.time_scale,
    };
    let (link_control, commands) = mpsc::channel(4);
    let mut engine =
        Engine::new(recording, live.clone(), ror, now_ms).with_link_control(link_control);
    if waiting {
        engine = engine.waiting_for_setup();
    }
    std::thread::Builder::new()
        .name("roastro-engine".into())
        .spawn(move || engine.run(inbox))?;

    tokio::spawn(link(
        target,
        config.demo,
        config.interval,
        requests.clone(),
        commands,
    ));
    Ok(Running { requests, live })
}

/// A client for the machine. v1 never writes to the PLC: no opt-in,
/// whatever the target.
fn connect(url: &str, map: &Arc<Map>) -> Result<Client, plc::Error> {
    Client::new(plc::Config {
        allow_write: false,
        ..plc::Config::new(url, Arc::clone(map))
    })
}

/// A client for the target: for the Demo machine, the simulator is started
/// first.
async fn open(target: &Target, demo: Option<&DemoHook>) -> Result<Client, Failure> {
    match target {
        Target::Machine { url, map } => connect(url, map).map_err(|e| Failure::from(&e)),
        Target::Demo => {
            let failure = |error: String| Failure {
                error,
                advice: "The built-in simulator could not start.".into(),
            };
            let hook = demo.ok_or_else(|| failure("no simulator in this build".into()))?;
            let url = hook().await.map_err(|e| failure(e.to_string()))?;
            let map = machine::model_map(machine::SIMULATOR).map_err(|e| failure(e.to_string()))?;
            connect(&url, &Arc::new(map)).map_err(|e| Failure::from(&e))
        }
    }
}

/// One read of a machine not in use, answered on `reply`.
async fn test(target: Target, demo: Option<DemoHook>, reply: Reply<TestOut>) {
    let refused = |failure: Failure| {
        ApiError::unprocessable(
            "machine_unreachable",
            format!("{} ({})", failure.advice, failure.error),
        )
    };
    let result = match open(&target, demo.as_ref()).await {
        Ok(client) => client.read().await.map_err(|e| refused(Failure::from(&e))),
        Err(failure) => Err(refused(failure)),
    };
    let _ = reply.send(result.map(|reading| TestOut {
        values: reading.values.into_iter().collect(),
        latency_ms: u64::try_from(reading.latency.as_millis()).unwrap_or(u64::MAX),
    }));
}

/// Handles a command while there is no connection to drop; says whether to
/// dial (again).
fn idle(command: LinkCommand, target: &mut Option<Target>, demo: Option<&DemoHook>) -> bool {
    match command {
        LinkCommand::RetryNow => target.is_some(),
        LinkCommand::Connect(t) => {
            *target = Some(t);
            true
        }
        LinkCommand::Test(t, reply) => {
            tokio::spawn(test(t, demo.cloned(), reply));
            false
        }
        LinkCommand::Disconnect => {
            *target = None;
            false
        }
    }
}

/// Polls the machine and hands every result to the engine. Waits while no
/// machine is set up; dials again at once on Retry now or another machine;
/// tests other settings on the side.
async fn link(
    mut target: Option<Target>,
    demo: Option<DemoHook>,
    interval: Duration,
    to_engine: mpsc::Sender<Request>,
    mut commands: mpsc::Receiver<LinkCommand>,
) {
    loop {
        let opened = match &target {
            Some(t) => Some(open(t, demo.as_ref()).await),
            None => None,
        };
        let client = match opened {
            Some(Ok(client)) => client,
            failed => {
                if let Some(Err(failure)) = failed {
                    let poll = Request::Poll {
                        at: now_ms(),
                        result: Err(failure),
                        retry_in: Duration::from_secs(10),
                    };
                    if to_engine.send(poll).await.is_err() {
                        return;
                    }
                }
                // Nothing to poll: wait to be told what to read.
                loop {
                    let Some(command) = commands.recv().await else {
                        return;
                    };
                    if idle(command, &mut target, demo.as_ref()) {
                        break;
                    }
                }
                continue;
            }
        };
        // Dropping the receiver stops the poller.
        let mut polls = Poller {
            interval,
            ..Poller::default()
        }
        .run(client);
        loop {
            tokio::select! {
                event = polls.recv() => {
                    let Some(event) = event else { break };
                    let poll = Request::Poll {
                        at: now_ms(),
                        result: event.result.map_err(|e| Failure::from(&e)),
                        retry_in: event.retry_in,
                    };
                    if to_engine.send(poll).await.is_err() {
                        return;
                    }
                }
                command = commands.recv() => match command {
                    None => return,
                    Some(LinkCommand::Test(t, reply)) => {
                        tokio::spawn(test(t, demo.clone(), reply));
                    }
                    Some(LinkCommand::RetryNow) => break,
                    Some(LinkCommand::Connect(t)) => {
                        target = Some(t);
                        break;
                    }
                    Some(LinkCommand::Disconnect) => {
                        target = None;
                        break;
                    }
                },
            }
        }
    }
}
