//! The Roastro app: reads the PLC once a second, records every roast, and
//! serves the API and the live stream to the web UI on this machine only.
//!
//! ```text
//! make run-sim SPEED=10                              # in one terminal
//! cargo run -p roastro -- --time-scale 10            # in another
//! ```
//!
//! The app never writes to the PLC: v1 only reads and records.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use clap::Parser;
use regmap::Map;
use roastro::api;

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
        args.http.ip().to_canonical().is_loopback(),
        "--http must be a loopback address: remote access is not part of v1"
    );

    let db = match args.db {
        Some(path) => path,
        None => roastro::default_db().context("pass --db")?,
    };
    let map = Map::load(&args.map).with_context(|| format!("loading {}", args.map.display()))?;
    let running = roastro::start(roastro::Config {
        url: args.url,
        map,
        db,
        interval: Duration::from_millis(args.interval_ms),
        time_scale: args.time_scale,
    })?;

    let port = args.http.port();
    let mut origins = vec![
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
    ];
    origins.extend(args.allow_origin);
    let app = api::router(api::AppState::new(running.requests, origins));
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
