//! The Roastro app. For now it reads the PLC once a second and records every
//! roast in the database; the API and the web UI come next.
//!
//! ```text
//! make run-sim SPEED=10                              # in one terminal
//! cargo run -p roastro -- --time-scale 10            # in another
//! ```
//!
//! The app never writes to the PLC: v1 only reads and records.

mod record;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use clap::Parser;
use plc::{Client, Config, Poller};
use recorder::{Input, Millis, Rules, Sample};
use regmap::Map;
use store::Store;

use crate::record::Recording;

// Doc comments here are the --help text, so they stay plain prose.
#[allow(clippy::doc_markdown)]
#[derive(Debug, Parser)]
#[command(about = "Roastro: reads the roaster PLC and records every roast")]
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
    let mut recording = Recording::open(store, rules)?;

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

    // SQLite is synchronous: the recording loop owns the database on its own thread.
    let recorder = std::thread::spawn(move || -> anyhow::Result<()> {
        while let Some(ev) = polls.blocking_recv() {
            let at = now_ms();
            let input = match ev.result {
                Ok(reading) => Input::Sample(Sample {
                    at,
                    values: reading.values,
                }),
                Err(_) => Input::Missed(at),
            };
            if let Err(err) = recording.handle(input) {
                tracing::error!(%err, "could not record");
            }
        }
        Ok(())
    });

    tokio::signal::ctrl_c()
        .await
        .context("waiting for Ctrl+C")?;
    tracing::info!("stopping; every reading so far is on disk");
    drop(recorder); // the process exit ends the thread; nothing is buffered
    Ok(())
}
