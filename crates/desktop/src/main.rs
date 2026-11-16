//! Roastro's desktop app: the engine (PLC, recorder, database) and the live
//! roast screen in one process (ADR 0003).
//!
//! ```text
//! cargo run -p desktop --release -- --simulator     # a demo: simulator inside, 10x
//! cargo run -p desktop --release                    # against `make run-sim`
//! ```
//!
//! It never writes to the PLC.

// A windowed app on Windows: no console window behind it in release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
mod chart;
mod coffees;
mod compare;
mod machine_page;
mod new_roast;
mod roast_detail;
mod roasts;
mod shell;
mod simulator;
mod state;
mod view;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use clap::Parser;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::{AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use regmap::Map;

use crate::shell::Shell;
use crate::view::Engine;

// Doc comments here are the --help text, so they stay plain prose.
#[allow(clippy::doc_markdown)]
#[derive(Debug, Parser)]
#[command(about = "Roastro: the live roast screen")]
struct Args {
    /// PLC address to read instead of the machine set up in the app
    /// (development and diagnostics), e.g. tcp://192.168.5.11:502.
    #[arg(long)]
    url: Option<String>,
    /// Register map (default: the simulator's, built in).
    #[arg(long)]
    map: Option<PathBuf>,
    /// Database file (default: Roastro/roastro.db in the local app data folder).
    #[arg(long)]
    db: Option<PathBuf>,
    /// Read the built-in simulator instead of the machine set up in the app.
    #[arg(long)]
    simulator: bool,
    /// Port for the built-in simulator (also the Demo machine's).
    #[arg(long, default_value_t = 5502)]
    sim_port: u16,
    /// The simulator's speed, so roast-time rules and the clock run at simulated speed.
    /// 1 for the real machine; defaults to 10 with --simulator.
    #[arg(long)]
    time_scale: Option<f64>,
    /// Quit after this many seconds (smoke tests).
    #[arg(long)]
    exit_after: Option<u64>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let time_scale = args
        .time_scale
        .unwrap_or(if args.simulator { 10.0 } else { 1.0 });
    let map = match &args.map {
        Some(path) => Map::load(path).with_context(|| format!("loading {}", path.display()))?,
        None => Map::from_json(include_str!("../../../configs/regmap.json"))?,
    };
    let db = match args.db {
        Some(db) => db,
        None => roastro::default_db()?,
    };
    // The built-in simulator, started once, when the Demo machine (or
    // --simulator) needs it.
    let started: Arc<tokio::sync::OnceCell<String>> = Arc::new(tokio::sync::OnceCell::new());
    let port = args.sim_port;
    let demo: roastro::DemoHook = Arc::new(move || {
        let started = started.clone();
        Box::pin(async move {
            started
                .get_or_try_init(|| async {
                    let map = Arc::new(roastro::machine::model_map(roastro::machine::SIMULATOR)?);
                    simulator::start(map, time_scale, port)
                        .await
                        .context("starting the built-in simulator")?;
                    Ok::<_, anyhow::Error>(format!("tcp://127.0.0.1:{port}"))
                })
                .await
                .cloned()
        })
    });

    // Tokio runs the PLC poller (and the simulator) on its own threads; it
    // lives as long as the process.
    let runtime = tokio::runtime::Runtime::new()?;
    let running = runtime.block_on(async {
        let url = if args.simulator {
            Some(demo().await?)
        } else {
            args.url
        };
        roastro::start(roastro::Config {
            url,
            map,
            db,
            interval: Duration::from_secs(1),
            time_scale,
            demo: Some(demo),
        })
    })?;
    std::thread::Builder::new()
        .name("tokio".into())
        .spawn(move || runtime.block_on(std::future::pending::<()>()))?;

    let engine = Engine(running.requests);
    let exit_after = args.exit_after;
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            view::use_accent(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1440.), px(900.)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Roastro".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            if let Err(err) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Shell::new(engine, time_scale, window, cx))
            }) {
                tracing::error!(%err, "could not open the window");
                cx.quit();
            }
            if let Some(secs) = exit_after {
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_secs(secs))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
        });
    Ok(())
}
