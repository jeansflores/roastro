//! Pretends to be the Caloratto PLC speaking Modbus TCP, with an HTTP panel
//! for the "physical world" (charging, dropping, faults).
//!
//! ```text
//! cargo run -p sim -- --auto --speed 10
//! ```

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use regmap::Map;
use sim::{Autopilot, Fault, ModbusService, Sim};
use tokio::net::TcpListener;
use tokio_modbus::server::tcp::{Server, accept_tcp_connection};

#[derive(Debug, Parser)]
#[command(about = "Pretends to be the Caloratto PLC speaking Modbus TCP")]
struct Args {
    /// Modbus TCP listen address (the real machine uses port 502).
    #[arg(long, default_value = "tcp://127.0.0.1:5502", value_parser = parse_addr)]
    modbus: SocketAddr,
    /// HTTP control panel listen address.
    #[arg(long, default_value = "127.0.0.1:8080", value_parser = parse_addr)]
    http: SocketAddr,
    /// Register map.
    #[arg(long, default_value = "configs/regmap.json")]
    map: PathBuf,
    /// Time acceleration (10 = a 10 min roast in 1 min).
    #[arg(long, default_value_t = 1.0)]
    speed: f64,
    /// Autopilot: roast on its own in a loop.
    #[arg(long)]
    auto: bool,
    /// Seed for the probe noise (default: the clock).
    #[arg(long)]
    seed: Option<u64>,
}

/// Accepts `host:port`, optionally prefixed with `tcp://`.
fn parse_addr(s: &str) -> Result<SocketAddr, String> {
    s.strip_prefix("tcp://")
        .unwrap_or(s)
        .parse()
        .map_err(|e| format!("{s:?}: {e} (want host:port)"))
}

/// What the HTTP panel can change besides the model itself.
struct Control {
    speed: f64,
    autopilot: Option<Autopilot>,
}

#[derive(Clone)]
struct App {
    sim: Arc<Mutex<Sim>>,
    control: Arc<Mutex<Control>>,
}

/// Locks a mutex, carrying on if another task panicked while holding it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();

    let map =
        Arc::new(Map::load(&args.map).with_context(|| format!("loading {}", args.map.display()))?);
    let seed = args.seed.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    });
    let app = App {
        sim: Arc::new(Mutex::new(Sim::new(seed))),
        control: Arc::new(Mutex::new(Control {
            speed: args.speed,
            autopilot: args.auto.then(Autopilot::default),
        })),
    };

    let modbus = TcpListener::bind(args.modbus)
        .await
        .with_context(|| format!("modbus listen {}", args.modbus))?;
    tracing::info!(addr = %args.modbus, unit = map.unit_id(), speed = args.speed, autopilot = args.auto, "Modbus TCP up");
    let service = ModbusService::new(map, app.sim.clone());
    let modbus = tokio::spawn(async move {
        let on_connected = |stream, peer| {
            let svc = service.for_peer(peer);
            async move { accept_tcp_connection(stream, peer, move |_| Ok(Some(svc.clone()))) }
        };
        Server::new(modbus)
            .serve(
                &on_connected,
                |err| tracing::warn!(%err, "modbus connection"),
            )
            .await
    });

    tokio::spawn(physics(app.clone()));

    let http = TcpListener::bind(args.http)
        .await
        .with_context(|| format!("http listen {}", args.http))?;
    tracing::info!("HTTP panel on http://{}", args.http);
    let panel = axum::serve(http, router(app)).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    });

    tokio::select! {
        res = panel => res.context("http panel")?,
        res = modbus => res.context("modbus task")?.context("modbus server")?,
    }
    Ok(())
}

/// Advances the model at 10 Hz of wall-clock time; the autopilot decides once
/// per simulated second.
async fn physics(app: App) {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut acc = 0.0;
    loop {
        tick.tick().await;
        let mut control = lock(&app.control);
        let mut sim = lock(&app.sim);
        let dt = 0.1 * control.speed;
        sim.step(dt);
        acc += dt;
        if acc >= 1.0 {
            acc = 0.0;
            if let Some(ap) = control.autopilot.as_mut() {
                ap.tick(&mut sim);
            }
        }
    }
}

const HELP: &str = "roastro sim: GET /state | /charge?kg=5&green=25 | /drop | /preheat?temp=200 | /reset \
                    | /fault?name=flame|emergency&on=1|0 | /speed?x=10\n";

fn router(app: App) -> Router {
    Router::new()
        .route("/", get(|| async { HELP }))
        .route("/state", get(state))
        .route("/charge", get(charge))
        .route("/drop", get(drop_batch))
        .route("/preheat", get(preheat))
        .route("/reset", get(reset))
        .route("/fault", get(fault))
        .route("/speed", get(speed))
        .with_state(app)
}

type Params = Query<HashMap<String, String>>;

/// A query number, or `default` when absent or malformed.
fn num(q: &HashMap<String, String>, key: &str, default: f64) -> f64 {
    q.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// The state after an action, or 400 with the reason it was refused.
fn reply(app: &App, result: Result<(), sim::Error>) -> Response {
    match result {
        Ok(()) => Json(lock(&app.sim).snapshot()).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn state(State(app): State<App>) -> Response {
    reply(&app, Ok(()))
}

async fn charge(State(app): State<App>, Query(q): Params) -> Response {
    let result = lock(&app.sim).charge(num(&q, "kg", 5.0), num(&q, "green", 25.0));
    reply(&app, result)
}

async fn drop_batch(State(app): State<App>) -> Response {
    let result = lock(&app.sim).drop_batch();
    reply(&app, result)
}

async fn preheat(State(app): State<App>, Query(q): Params) -> Response {
    lock(&app.sim).preheat(num(&q, "temp", 200.0));
    reply(&app, Ok(()))
}

async fn reset(State(app): State<App>) -> Response {
    {
        let mut control = lock(&app.control);
        lock(&app.sim).reset();
        if control.autopilot.is_some() {
            control.autopilot = Some(Autopilot::default());
        }
    }
    reply(&app, Ok(()))
}

async fn fault(State(app): State<App>, Query(q): Params) -> Response {
    let name = q.get("name").map_or("", String::as_str);
    let on = q.get("on").is_none_or(|v| v != "0");
    let result = name
        .parse::<Fault>()
        .map(|f| lock(&app.sim).set_fault(f, on));
    reply(&app, result)
}

async fn speed(State(app): State<App>, Query(q): Params) -> Response {
    {
        let mut control = lock(&app.control);
        control.speed = num(&q, "x", control.speed);
    }
    reply(&app, Ok(()))
}
