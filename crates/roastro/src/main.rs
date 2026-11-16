//! The Roastro app: reads the PLC once a second, records every roast, and
//! serves the API and the live stream to the web UI on this machine only.
//!
//! ```text
//! make run-sim SPEED=10                              # in one terminal
//! cargo run -p roastro -- --time-scale 10            # in another
//! ```
//!
//! The app never writes to the PLC: v1 only reads and records.

mod api;
mod engine;
mod record;
mod wire;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use clap::Parser;
use plc::{Client, Config, Poller};
use recorder::{Millis, Rules};
use regmap::Map;
use store::Store;
use tokio::sync::{broadcast, mpsc};

use crate::engine::{Engine, Request};
use crate::record::Recording;
use crate::wire::RorSettings;

// Doc comments here are the --help text, so they stay plain prose.
#[allow(clippy::doc_markdown)]
#[derive(Debug, Parser)]
#[command(about = "Roastro: reads the roaster PLC, records every roast and serves the UI")]
struct Args {
    /// PLC address (real machine: tcp://192.168.5.11:502).
    #[arg(long, default_value = "tcp://127.0.0.1:5502")]
    url: String,
    /// Register map.
    #[arg(long, default_value = "configs/regmap.json")]
    map: PathBuf,
    /// Database file (default: Roastro/roastro.db in the local app data folder).
    #[arg(long)]
    db: Option<PathBuf>,
    /// Address for the API and the UI. Loopback only: remote access is not part of v1.
    #[arg(long, default_value = "127.0.0.1:8000")]
    http: SocketAddr,
    /// Another page origin allowed to use the API, e.g. the UI's dev server
    /// (http://localhost:5173). Repeatable.
    #[arg(long)]
    allow_origin: Vec<String>,
    /// Polling interval in milliseconds.
    #[arg(long, default_value_t = 1000)]
    interval_ms: u64,
    /// The simulator's --speed, so roast-time rules (60 s lead-in, 3 min before
    /// Drop detection...) run at simulated speed. 1 for the real machine.
    #[arg(long, default_value_t = 1.0)]
    time_scale: f64,
}

/// `%LOCALAPPDATA%\Roastro\roastro.db` on Windows, `~/.local/share/Roastro/roastro.db` on Linux.
fn default_db() -> anyhow::Result<PathBuf> {
    let base = dirs::data_local_dir().context("no local data folder on this system; pass --db")?;
    Ok(base.join("Roastro").join("roastro.db"))
}

fn now_ms() -> Millis {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| {
        Millis::try_from(d.as_millis()).unwrap_or(Millis::MAX)
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    anyhow::ensure!(
        args.time_scale > 0.0 && args.time_scale.is_finite(),
        "--time-scale must be positive"
    );
    anyhow::ensure!(
        args.http.ip().to_canonical().is_loopback(),
        "--http must be a loopback address: remote access is not part of v1"
    );

    let db = match args.db {
        Some(path) => path,
        None => default_db()?,
    };
    if let Some(dir) = db.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let store = Store::open(&db).with_context(|| format!("opening {}", db.display()))?;
    tracing::info!(db = %db.display(), schema = store.schema_version()?, "database ready");
    let rules = Rules {
        time_scale: args.time_scale,
        ..Rules::default()
    };
    let recording = Recording::open(store, rules)?;

    let map =
        Arc::new(Map::load(&args.map).with_context(|| format!("loading {}", args.map.display()))?);
    // v1 never writes to the PLC: no opt-in, whatever the target.
    let client = Client::new(Config {
        allow_write: false,
        ..Config::new(&args.url, map)
    })?;
    let mut polls = Poller {
        interval: Duration::from_millis(args.interval_ms),
        ..Poller::default()
    }
    .run(client);
    tracing::info!(url = %args.url, "reading the PLC");

    // The engine owns the database on its own thread (SQLite is synchronous).
    let (requests, inbox) = mpsc::channel(256);
    let (live, _) = broadcast::channel(256);
    let ror = RorSettings {
        window: Duration::from_secs(30),
        gap: rules.gap,
        time_scale: args.time_scale,
    };
    let engine = Engine::new(recording, live, ror, now_ms);
    std::thread::spawn(move || engine.run(inbox));

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

    let port = args.http.port();
    let mut origins = vec![
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
    ];
    origins.extend(args.allow_origin);
    let app = api::router(api::AppState::new(requests, origins));
    let listener = tokio::net::TcpListener::bind(args.http)
        .await
        .with_context(|| format!("listening on {}", args.http))?;
    tracing::info!("API on http://{}", args.http);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("serving the API")?;
    tracing::info!("stopped; every reading so far is on disk");
    Ok(())
}
