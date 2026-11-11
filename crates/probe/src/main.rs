//! A minimal Modbus client. It works against both the simulator and the real
//! machine.
//!
//! ```text
//! probe                                   # read and print every 1s
//! probe --csv roast.csv                   # also write a CSV
//! probe set burner=1 gas_cmd=60           # write (localhost only)
//! probe --url tcp://192.168.5.11:502      # real machine, read-only
//! ```

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use jiff::Zoned;
use plc::{Client, Config, Poller};
use regmap::Map;
use roast::RoR;

// Doc comments here are the --help text, so they stay plain prose.
#[allow(clippy::doc_markdown)]
#[derive(Debug, Parser)]
#[command(about = "Line-based Modbus client for the roaster PLC")]
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
    /// Also write readings to this CSV file.
    #[arg(long)]
    csv: Option<PathBuf>,
    /// The simulator's --speed, so RoR comes out in simulated °C/min.
    #[arg(long, default_value_t = 1.0)]
    time_scale: f64,
    /// Allow writes to a non-localhost target. Real machine: only with someone next to it.
    #[arg(long)]
    allow_write: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write points once and exit, e.g. `set burner=1 gas_cmd=60`.
    Set {
        /// `name=value` pairs, in physical units (1/0 for coils).
        #[arg(required = true, value_parser = parse_pair)]
        pairs: Vec<(String, f64)>,
    },
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
    Duration::try_from_secs_f64(secs)
        .map_err(|e| format!("{s:?}: {e}"))
        .and_then(|d| {
            if d.is_zero() {
                Err(format!("{s:?}: must be positive"))
            } else {
                Ok(d)
            }
        })
}

/// Parses `name=value`.
fn parse_pair(s: &str) -> Result<(String, f64), String> {
    let (name, value) = s
        .split_once('=')
        .ok_or_else(|| format!("{s:?}: use name=value"))?;
    let value = value
        .parse()
        .map_err(|_| format!("{s:?}: {value:?} is not a number"))?;
    Ok((name.to_owned(), value))
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

    let map =
        Arc::new(Map::load(&args.map).with_context(|| format!("loading {}", args.map.display()))?);
    let cfg = Config {
        timeout: Duration::from_secs(2),
        allow_write: args.allow_write,
        ..Config::new(&args.url, map.clone())
    };
    let client = Arc::new(Client::new(cfg)?);

    match args.command {
        Some(Command::Set { pairs }) => {
            for (name, value) in pairs {
                client
                    .write(&name, value)
                    .await
                    .with_context(|| format!("{name}={value}"))?;
                println!("ok {name}={value}");
            }
            Ok(())
        }
        None => poll(client, &map, args.interval, args.csv, args.time_scale).await,
    }
}

async fn poll(
    client: Arc<Client>,
    map: &Map,
    interval: Duration,
    csv: Option<PathBuf>,
    time_scale: f64,
) -> anyhow::Result<()> {
    if time_scale <= 0.0 || !time_scale.is_finite() {
        bail!("--time-scale must be positive");
    }
    let mut names: Vec<&str> = map.points().iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();

    let mut csv = match csv {
        Some(path) => {
            let mut w = BufWriter::new(
                File::create(&path).with_context(|| format!("csv {}", path.display()))?,
            );
            writeln!(w, "time,ror_bt,{}", names.join(","))?;
            Some(w)
        }
        None => None,
    };

    let mut ror = RoR::new(Duration::from_secs(30));
    let mut first = None;
    let mut events = Poller {
        interval,
        ..Poller::default()
    }
    .run(client);
    while let Some(ev) = events.recv().await {
        let now = Zoned::now();
        let reading = match ev.result {
            Ok(r) => r,
            Err(err) => {
                eprintln!("{}  read failed: {err}", now.strftime("%H:%M:%S"));
                continue;
            }
        };
        let v = &reading.values;
        let elapsed = ev.at - *first.get_or_insert(ev.at);
        ror.add(elapsed.mul_f64(time_scale), value(v, "bt"));
        let rate = ror.value().unwrap_or(0.0);

        println!("{}  {}", now.strftime("%H:%M:%S"), line(v, rate));
        if let Some(w) = csv.as_mut() {
            writeln!(w, "{}", csv_row(&now, rate, &names, v))?;
            w.flush()?;
        }
    }
    Ok(())
}

fn value(v: &HashMap<String, f64>, name: &str) -> f64 {
    v.get(name).copied().unwrap_or(0.0)
}

const FLAGS: [&str; 7] = [
    "burner",
    "stirrer",
    "cooler",
    "bell",
    "flame_fault",
    "emergency",
    "safety_trip",
];

/// One human-readable line for a reading.
fn line(v: &HashMap<String, f64>, rate: f64) -> String {
    let g = |name| value(v, name);
    let mut out = format!(
        "t={:4.0}s  BT={:6.1}  ET={:6.1}  RoR={rate:6.1}  gas={:3.0}/{:3.0}%  air={:3.0}%  drum={:3.0}%",
        g("timer_s"),
        g("bt"),
        g("et"),
        g("gas_cmd"),
        g("gas_fb"),
        g("fan"),
        g("drum"),
    );
    for flag in FLAGS.iter().filter(|f| g(f) != 0.0) {
        out.push(' ');
        out.push_str(&flag.to_uppercase());
    }
    out
}

/// One CSV row: RFC 3339 time, `RoR`, then every point in `names` order.
fn csv_row(now: &Zoned, rate: f64, names: &[&str], v: &HashMap<String, f64>) -> String {
    let mut row = format!(
        "{},{rate:.2}",
        now.timestamp().display_with_offset(now.offset())
    );
    for name in names {
        row.push(',');
        row.push_str(&value(v, name).to_string());
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("1s"), Ok(Duration::from_secs(1)));
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        for bad in ["", "1", "1h", "x s", "0s", "-1s"] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parses_pairs() {
        assert_eq!(parse_pair("gas_cmd=60"), Ok(("gas_cmd".into(), 60.0)));
        assert_eq!(parse_pair("burner=1"), Ok(("burner".into(), 1.0)));
        assert!(parse_pair("gas_cmd").is_err());
        assert!(parse_pair("gas_cmd=high").is_err());
    }

    fn reading() -> HashMap<String, f64> {
        [
            ("timer_s", 125.0),
            ("bt", 150.25),
            ("et", 190.0),
            ("gas_cmd", 70.0),
            ("gas_fb", 69.0),
            ("fan", 55.0),
            ("drum", 80.0),
            ("burner", 1.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
    }

    #[test]
    fn formats_a_line() {
        assert_eq!(
            line(&reading(), 12.34),
            "t= 125s  BT= 150.2  ET= 190.0  RoR=  12.3  gas= 70/ 69%  air= 55%  drum= 80% BURNER"
        );
    }

    #[test]
    fn formats_a_csv_row() {
        let now: Zoned = "2026-11-12T09:30:00-03:00[America/Sao_Paulo]"
            .parse()
            .unwrap();
        assert_eq!(
            csv_row(&now, 12.345, &["bt", "missing"], &reading()),
            "2026-11-12T09:30:00-03:00,12.35,150.25,0"
        );
    }
}
