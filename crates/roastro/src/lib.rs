//! Roastro's core: reads the PLC, records every roast, and serves whoever
//! asks, the native UI in the same process or the HTTP API (ADR 0003).
//!
//! [`start`] wires it up: the database and the recorder on the engine's own
//! thread, the PLC poller on the Tokio runtime, and a [`Running`] handle with
//! the engine's request queue and live broadcast. It never writes to the PLC.

pub mod api;
pub mod engine;
mod record;
// The wire types mirror the JSON contract field for field; it is documented
// in "Contract between the backend and the web UI" (#8), not per field here.
#[allow(missing_docs)]
pub mod wire;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use plc::{Client, Poller};
use recorder::{Millis, Rules};
use regmap::Map;
use store::Store;
use tokio::sync::{broadcast, mpsc};

use crate::engine::{Engine, Request};
use crate::record::Recording;
use crate::wire::{Live, RorSettings};

/// What [`start`] needs.
#[derive(Debug, Clone)]
pub struct Config {
    /// PLC address, e.g. `tcp://192.168.5.11:502`.
    pub url: String,
    /// Register map.
    pub map: Map,
    /// Database file.
    pub db: PathBuf,
    /// Between polls.
    pub interval: Duration,
    /// The simulator's speed (1 on the machine).
    pub time_scale: f64,
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
    let rules = Rules {
        time_scale: config.time_scale,
        ..Rules::default()
    };
    let recording = Recording::open(store, rules)?;

    // v1 never writes to the PLC: no opt-in, whatever the target.
    let client = Client::new(plc::Config {
        allow_write: false,
        ..plc::Config::new(&config.url, Arc::new(config.map))
    })?;
    let mut polls = Poller {
        interval: config.interval,
        ..Poller::default()
    }
    .run(client);
    tracing::info!(url = %config.url, "reading the PLC");

    // The engine owns the database on its own thread (SQLite is synchronous).
    let (requests, inbox) = mpsc::channel(256);
    let (live, _) = broadcast::channel(256);
    let ror = RorSettings {
        window: Duration::from_secs(30),
        gap: rules.gap,
        time_scale: config.time_scale,
    };
    let engine = Engine::new(recording, live.clone(), ror, now_ms);
    std::thread::Builder::new()
        .name("roastro-engine".into())
        .spawn(move || engine.run(inbox))?;

    let to_engine = requests.clone();
    tokio::spawn(async move {
        while let Some(ev) = polls.recv().await {
            let result = ev.result.map_err(|e| e.to_string());
            let poll = Request::Poll {
                at: now_ms(),
                result,
                retry_in: ev.retry_in,
            };
            if to_engine.send(poll).await.is_err() {
                return;
            }
        }
    });
    Ok(Running { requests, live })
}
