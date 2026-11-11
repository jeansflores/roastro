//! A live terminal dashboard for the roaster PLC. It polls every point in the
//! register map, shows the roast and the health of the Modbus link, and sends
//! commands from the keyboard.
//!
//! ```text
//! make run-sim                                  # in one terminal
//! monitor --time-scale 10                       # in another
//! monitor --url tcp://192.168.5.11:502          # real machine, read-only
//! ```

mod app;
mod ui;

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use app::{Action, App, Config as AppConfig, Key, Msg};
use clap::Parser;
use plc::{Client, Config, Poller};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use regmap::Map;
use tokio::sync::mpsc;
use tokio::time::Instant;

// Doc comments here are the --help text, so they stay plain prose.
#[allow(clippy::doc_markdown)]
#[derive(Debug, Parser)]
#[command(about = "Live terminal dashboard for the roaster PLC")]
struct Args {
    /// PLC address (real machine: tcp://192.168.5.11:502).
    #[arg(long, default_value = "tcp://127.0.0.1:5502")]
    url: String,
    /// Register map.
    #[arg(long, default_value = "configs/regmap.json")]
    map: PathBuf,
    /// Polling interval, e.g. 1s or 500ms.
    #[arg(long, default_value = "1s", value_parser = parse_duration)]
    interval: Duration,
    /// Modbus request timeout.
    #[arg(long, default_value = "1s", value_parser = parse_duration)]
    timeout: Duration,
    /// The simulator's --speed, so RoR comes out in simulated °C/min.
    #[arg(long, default_value_t = 1.0)]
    time_scale: f64,
    /// Allow writes to a non-localhost target. Real machine: only with someone next to it.
    #[arg(long)]
    allow_write: bool,
    /// Log file (the terminal belongs to the dashboard).
    #[arg(long, default_value = "monitor.log")]
    log: PathBuf,
    /// Log level: error, warn, info, debug or trace.
    #[arg(long, default_value = "info")]
    log_level: String,
}

/// Parses `500ms`, `1s`, `1.5s` or `2m`.
fn parse_duration(s: &str) -> Result<Duration, String> {
    let (num, unit) = s
        .find(|c: char| c.is_ascii_alphabetic())
        .map_or((s, ""), |i| s.split_at(i));
    let n: f64 = num
        .parse()
        .map_err(|_| format!("{s:?}: want a number with ms, s or m"))?;
    let secs = match unit {
        "ms" => n / 1000.0,
        "s" => n,
        "m" => n * 60.0,
        _ => return Err(format!("{s:?}: unit must be ms, s or m")),
    };
    match Duration::try_from_secs_f64(secs) {
        Ok(d) if !d.is_zero() => Ok(d),
        _ => Err(format!("{s:?}: must be positive")),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.log)
        .with_context(|| format!("log file {}", args.log.display()))?;
    let filter = tracing_subscriber::EnvFilter::try_new(&args.log_level).context("--log-level")?;
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(log))
        .with_ansi(false)
        .with_env_filter(filter)
        .init();

    let map =
        Arc::new(Map::load(&args.map).with_context(|| format!("loading {}", args.map.display()))?);
    let cfg = Config {
        timeout: args.timeout,
        allow_write: args.allow_write,
        ..Config::new(&args.url, map)
    };
    let client = Arc::new(Client::new(cfg)?);
    let can_write = client.can_write();
    tracing::info!(url = %args.url, can_write, interval = ?args.interval, "monitor started");

    let polls = Poller {
        interval: args.interval,
        ..Poller::default()
    }
    .run(client.clone());
    let app = App::new(
        AppConfig {
            target: args.url,
            can_write,
            time_scale: args.time_scale,
        },
        Instant::now(),
    );

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, app, polls, client).await;
    ratatui::restore();
    tracing::info!("monitor stopped");
    result
}

/// The event loop: draw, wait for the next message, apply it, repeat.
async fn run(
    terminal: &mut DefaultTerminal,
    mut app: App,
    mut polls: mpsc::Receiver<plc::Event>,
    client: Arc<Client>,
) -> anyhow::Result<()> {
    // Keys and finished writes arrive on the same inbox.
    let (inbox, mut messages) = mpsc::channel(16);
    read_keys(inbox.clone());
    let mut tick = tokio::time::interval(Duration::from_millis(250));

    loop {
        terminal.draw(|f| ui::draw(f, &app)).context("drawing")?;
        let msg = tokio::select! {
            ev = polls.recv() => ev.map_or(Msg::PollerClosed, Msg::Poll),
            Some(msg) = messages.recv() => msg,
            now = tick.tick() => Msg::Tick(now),
        };
        match app.update(msg) {
            Action::None => {}
            Action::Quit => return Ok(()),
            Action::Write { name, value } => {
                // Off the UI loop: a slow PLC must not freeze the screen.
                let (client, inbox) = (client.clone(), inbox.clone());
                tokio::spawn(async move {
                    let result = client.write(&name, value).await.map_err(|e| e.to_string());
                    let _ = inbox
                        .send(Msg::WriteDone {
                            name,
                            value,
                            result,
                        })
                        .await;
                });
            }
        }
    }
}

/// Reads the keyboard on its own thread (the terminal read blocks) and
/// forwards key presses. Resizes need no message: every loop turn redraws.
fn read_keys(inbox: mpsc::Sender<Msg>) {
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            let Event::Key(k) = ev else { continue };
            // Windows reports releases too.
            if k.kind != KeyEventKind::Press {
                continue;
            }
            let key = match k.code {
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => Key::Interrupt,
                KeyCode::Char(c) => Key::Char(c),
                KeyCode::Esc => Key::Char('q'),
                _ => continue,
            };
            if inbox.blocking_send(Msg::Key(key)).is_err() {
                return; // the dashboard is gone
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("1s"), Ok(Duration::from_secs(1)));
        assert_eq!(parse_duration("250ms"), Ok(Duration::from_millis(250)));
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("1h").is_err());
    }
}
