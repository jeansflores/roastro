//! Dashboard state. `App::update` is the only place that changes it, and it
//! never touches the terminal or the network: it returns an [`Action`] for
//! the caller to carry out. That keeps every rule testable on its own.

use std::collections::HashMap;
use std::time::Duration;

use plc::Event;
use roast::RoR;
use tokio::time::Instant;

/// What the dashboard needs to know about its environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// Shown in the header.
    pub target: String,
    /// False disables every command key.
    pub can_write: bool,
    /// The simulator's `--speed`, so `RoR` is in simulated °C/min.
    pub time_scale: f64,
}

/// A key the dashboard understands, independent of the terminal library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character.
    Char(char),
    /// Ctrl+C.
    Interrupt,
}

/// Everything that can happen to the dashboard.
#[derive(Debug)]
pub enum Msg {
    /// One poll attempt.
    Poll(Event),
    /// The poller stopped.
    PollerClosed,
    /// The clock moved (drives "x s ago").
    Tick(Instant),
    /// A key was pressed.
    Key(Key),
    /// A write sent earlier finished.
    WriteDone {
        /// Point written.
        name: String,
        /// Value asked for.
        value: f64,
        /// The PLC's answer.
        result: Result<(), String>,
    },
}

/// What the caller must do after an update.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Nothing.
    None,
    /// Leave the dashboard.
    Quit,
    /// Send this write to the PLC, then report back with [`Msg::WriteDone`].
    Write {
        /// Point to write.
        name: String,
        /// Value to write.
        value: f64,
    },
}

/// What a key does to the machine.
#[derive(Debug, Clone, Copy)]
enum Command {
    /// Flip a coil.
    Toggle(&'static str),
    /// Nudge a register by `step`, kept within 0..=max.
    Nudge {
        point: &'static str,
        step: f64,
        max: f64,
    },
}

fn command(key: char) -> Option<Command> {
    use Command::{Nudge, Toggle};
    Some(match key {
        'b' => Toggle("burner"),
        'd' => Toggle("drum_on"),
        'f' => Toggle("fan_on"),
        's' => Toggle("stirrer"),
        'c' => Toggle("cooler"),
        '+' | '=' => Nudge {
            point: "gas_cmd",
            step: 5.0,
            max: 100.0,
        }, // '=' is '+' without shift
        '-' => Nudge {
            point: "gas_cmd",
            step: -5.0,
            max: 100.0,
        },
        ']' => Nudge {
            point: "fan",
            step: 5.0,
            max: 120.0,
        },
        '[' => Nudge {
            point: "fan",
            step: -5.0,
            max: 120.0,
        },
        '}' => Nudge {
            point: "drum",
            step: 5.0,
            max: 120.0,
        },
        '{' => Nudge {
            point: "drum",
            step: -5.0,
            max: 120.0,
        },
        _ => return None,
    })
}

/// The dashboard state.
#[derive(Debug)]
pub struct App {
    pub(crate) cfg: Config,

    pub(crate) now: Instant,
    start: Option<Instant>, // first good reading; zero of the roast clock
    pub(crate) ror: RoR,
    /// Latest good reading; `None` until the first one.
    pub(crate) values: Option<HashMap<String, f64>>,
    /// Targets of writes still in flight, so a second key press nudges from
    /// the first one's target instead of a stale reading.
    pending: HashMap<String, f64>,

    pub(crate) connected: bool,
    pub(crate) attempts: u32,
    pub(crate) reads: u32,
    pub(crate) errors: u32,
    pub(crate) latency: Duration,
    pub(crate) last_ok: Option<Instant>,
    pub(crate) last_err: Option<String>,
    pub(crate) next_retry: Option<Instant>,

    /// Result of the last command.
    pub(crate) status: String,
}

impl App {
    /// A dashboard that has seen nothing yet.
    #[must_use]
    pub fn new(mut cfg: Config, now: Instant) -> Self {
        if cfg.time_scale <= 0.0 || !cfg.time_scale.is_finite() {
            cfg.time_scale = 1.0;
        }
        Self {
            cfg,
            now,
            start: None,
            ror: RoR::new(Duration::from_secs(30)),
            values: None,
            pending: HashMap::new(),
            connected: false,
            attempts: 0,
            reads: 0,
            errors: 0,
            latency: Duration::ZERO,
            last_ok: None,
            last_err: None,
            next_retry: None,
            status: String::new(),
        }
    }

    /// Applies one message and says what the caller must do next.
    pub fn update(&mut self, msg: Msg) -> Action {
        match msg {
            Msg::Poll(ev) => self.apply_poll(ev),
            Msg::PollerClosed | Msg::Key(Key::Interrupt | Key::Char('q')) => return Action::Quit,
            Msg::Tick(now) => self.now = now,
            Msg::Key(Key::Char(c)) => return self.handle_key(c),
            Msg::WriteDone {
                name,
                value,
                result,
            } => {
                self.pending.remove(&name);
                if let Err(err) = result {
                    tracing::warn!(point = %name, value, %err, "write failed");
                    self.status = format!("✗ {name}={value}: {err}");
                } else {
                    tracing::info!(point = %name, value, "write");
                    self.status = format!("✓ {name}={value}");
                    // The PLC accepted it; the next poll confirms.
                    if let Some(values) = self.values.as_mut() {
                        values.insert(name, value);
                    }
                }
            }
        }
        Action::None
    }

    fn apply_poll(&mut self, ev: Event) {
        self.attempts += 1;
        self.now = ev.at;
        match ev.result {
            Err(err) => {
                self.connected = false;
                self.errors += 1;
                self.last_err = Some(err.to_string());
                self.next_retry = Some(ev.at + ev.retry_in);
            }
            Ok(reading) => {
                self.connected = true;
                self.reads += 1;
                self.latency = reading.latency;
                self.last_ok = Some(ev.at);
                let start = *self.start.get_or_insert(ev.at);
                let roast_clock = (ev.at - start).mul_f64(self.cfg.time_scale);
                if let Some(&bt) = reading.values.get("bt") {
                    self.ror.add(roast_clock, bt);
                }
                self.values = Some(reading.values);
            }
        }
    }

    fn handle_key(&mut self, key: char) -> Action {
        let Some(cmd) = command(key) else {
            return Action::None;
        };
        if !self.cfg.can_write {
            self.status =
                "✗ read-only: writes to this target are blocked (see --allow-write)".into();
            return Action::None;
        }
        let point = match cmd {
            Command::Toggle(p) | Command::Nudge { point: p, .. } => p,
        };
        let current = self
            .pending
            .get(point)
            .or_else(|| self.values.as_ref().and_then(|v| v.get(point)))
            .copied();
        let Some(current) = current else {
            self.status = "✗ no reading yet; wait for the PLC before sending commands".into();
            return Action::None;
        };
        let value = match cmd {
            Command::Toggle(_) => {
                if current == 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Command::Nudge { step, max, .. } => (current + step).clamp(0.0, max),
        };
        self.pending.insert(point.to_owned(), value);
        self.status = format!("… {point}={value}");
        Action::Write {
            name: point.to_owned(),
            value,
        }
    }
}

#[cfg(test)]
mod tests {
    use plc::{Error, Reading};

    use super::*;

    fn app(can_write: bool) -> App {
        let cfg = Config {
            target: "tcp://127.0.0.1:5502".into(),
            can_write,
            time_scale: 1.0,
        };
        App::new(cfg, Instant::now())
    }

    fn reading(at: Instant, values: &[(&str, f64)]) -> Msg {
        let values = values.iter().map(|&(k, v)| (k.to_owned(), v)).collect();
        Msg::Poll(Event {
            at,
            result: Ok(Reading {
                values,
                latency: Duration::from_millis(1),
            }),
            failures: 0,
            retry_in: Duration::from_secs(1),
        })
    }

    fn value(app: &App, name: &str) -> Option<f64> {
        app.values.as_ref().and_then(|v| v.get(name)).copied()
    }

    #[test]
    fn tracks_the_link() {
        let mut a = app(true);
        let t0 = Instant::now();
        a.update(reading(t0, &[("bt", 198.4)]));
        assert!(a.connected && a.reads == 1 && a.last_ok == Some(t0));
        assert_eq!(value(&a, "bt"), Some(198.4));

        let down = Error::Timeout { op: "read".into() };
        let t1 = t0 + Duration::from_secs(1);
        a.update(Msg::Poll(Event {
            at: t1,
            result: Err(down),
            failures: 1,
            retry_in: Duration::from_secs(2),
        }));
        assert!(!a.connected && a.errors == 1 && a.last_err.is_some());
        assert_eq!(
            value(&a, "bt"),
            Some(198.4),
            "a failed read kept the last good value"
        );
        assert_eq!(a.next_retry, Some(t1 + Duration::from_secs(2)));
        assert_eq!(a.attempts, 2);
    }

    #[test]
    fn keys_send_commands() {
        let state = [
            ("burner", 0.0),
            ("drum_on", 1.0),
            ("gas_cmd", 98.0),
            ("fan", 3.0),
            ("drum", 80.0),
        ];
        let cases = [
            ('b', "burner", 1.0),
            ('d', "drum_on", 0.0),
            ('+', "gas_cmd", 100.0), // clamped
            ('-', "gas_cmd", 93.0),
            ('[', "fan", 0.0), // clamped
            (']', "fan", 8.0),
            ('}', "drum", 85.0),
        ];
        for (key, name, value) in cases {
            let mut a = app(true);
            a.update(reading(Instant::now(), &state));
            let action = a.update(Msg::Key(Key::Char(key)));
            assert_eq!(
                action,
                Action::Write {
                    name: name.into(),
                    value
                },
                "key {key}"
            );
            a.update(Msg::WriteDone {
                name: name.into(),
                value,
                result: Ok(()),
            });
            assert!(
                a.status.starts_with('✓'),
                "key {key}: status {:?}",
                a.status
            );
        }
    }

    /// Pressing "+" twice before the first write returns asks for 10%, not
    /// 5% twice: the second press builds on the pending target.
    #[test]
    fn repeated_keys_build_on_pending_writes() {
        let mut a = app(true);
        a.update(reading(Instant::now(), &[("gas_cmd", 0.0)]));
        let first = a.update(Msg::Key(Key::Char('+')));
        let second = a.update(Msg::Key(Key::Char('+')));
        assert_eq!(
            first,
            Action::Write {
                name: "gas_cmd".into(),
                value: 5.0
            }
        );
        assert_eq!(
            second,
            Action::Write {
                name: "gas_cmd".into(),
                value: 10.0
            }
        );
    }

    #[test]
    fn keys_refuse_when_unsafe_or_blind() {
        for (name, can_write, read) in [
            ("read-only target", false, true),
            ("no reading yet", true, false),
        ] {
            let mut a = app(can_write);
            if read {
                a.update(reading(Instant::now(), &[("burner", 0.0)]));
            }
            assert_eq!(a.update(Msg::Key(Key::Char('b'))), Action::None, "{name}");
            assert!(
                a.status.starts_with('✗'),
                "{name}: no explanation on the status line"
            );
        }
    }

    #[test]
    fn shows_a_failed_write() {
        let mut a = app(true);
        a.update(reading(Instant::now(), &[("burner", 0.0)]));
        a.update(Msg::Key(Key::Char('b')));
        a.update(Msg::WriteDone {
            name: "burner".into(),
            value: 1.0,
            result: Err("PLC refused".into()),
        });
        assert_eq!(a.status, "✗ burner=1: PLC refused");
        assert_eq!(
            value(&a, "burner"),
            Some(0.0),
            "a refused write does not change the shown value"
        );
    }

    #[test]
    fn quits() {
        for msg in [
            Msg::Key(Key::Char('q')),
            Msg::Key(Key::Interrupt),
            Msg::PollerClosed,
        ] {
            assert_eq!(app(true).update(msg), Action::Quit);
        }
    }
}
