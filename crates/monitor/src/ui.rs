//! Renders the dashboard. Colors are the terminal's own ANSI palette, so it
//! reads well on dark and light themes.

use std::collections::HashMap;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use crate::app::App;

const PANEL_WIDTH: u16 = 46;
const PANEL_HEIGHT: u16 = 14;
const LABEL_WIDTH: usize = 9;

fn on() -> Style {
    Style::new().fg(Color::Green).add_modifier(Modifier::BOLD)
}
fn off() -> Style {
    Style::new().fg(Color::DarkGray)
}
fn alarm() -> Style {
    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
}
fn notice() -> Style {
    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
}
fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// Draws the whole dashboard into `frame`.
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let side_by_side = area.width > 2 * PANEL_WIDTH;
    let body_height = if side_by_side {
        PANEL_HEIGHT
    } else {
        2 * PANEL_HEIGHT
    };
    let [header, body, status, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(body_height),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(header_line(app)), header);

    let (roast_area, link_area) = if side_by_side {
        let [a, _, b] = Layout::horizontal([
            Constraint::Length(PANEL_WIDTH),
            Constraint::Length(1),
            Constraint::Length(PANEL_WIDTH),
        ])
        .areas(body);
        (a, b)
    } else {
        let [a, b] = Layout::vertical([Constraint::Length(PANEL_HEIGHT); 2]).areas(body);
        (narrow(a), narrow(b))
    };
    frame.render_widget(panel("Roast", roast_lines(app)), roast_area);
    frame.render_widget(panel("Link", link_lines(app)), link_area);

    frame.render_widget(Paragraph::new(app.status.as_str()), status);
    frame.render_widget(
        Paragraph::new(help_line(app)).wrap(Wrap { trim: true }),
        help,
    );
}

fn narrow(r: Rect) -> Rect {
    Rect {
        width: r.width.min(PANEL_WIDTH),
        ..r
    }
}

fn panel<'a>(title: &'a str, lines: Vec<Line<'a>>) -> Paragraph<'a> {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(off())
        .title(Span::styled(format!(" {title} "), bold()));
    Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
}

fn header_line(app: &App) -> Line<'_> {
    let conn = if app.connected {
        Span::styled("● CONNECTED", on())
    } else if app.errors > 0 {
        Span::styled("● DISCONNECTED", alarm())
    } else {
        Span::styled("◌ CONNECTING", notice())
    };
    let writes = if app.cfg.can_write {
        Span::styled("writes enabled", notice())
    } else {
        Span::styled("read-only", off())
    };
    Line::from(vec![
        "roastro monitor".bold(),
        "  ".into(),
        app.cfg.target.as_str().into(),
        "  ".into(),
        conn,
        "  ".into(),
        writes,
    ])
}

fn row<'a>(label: &'a str, value: Vec<Span<'a>>) -> Line<'a> {
    let mut spans = vec![Span::styled(format!("{label:<LABEL_WIDTH$}"), off())];
    spans.extend(value);
    Line::from(spans)
}

fn roast_lines(app: &App) -> Vec<Line<'_>> {
    let Some(v) = app.values.as_ref() else {
        return vec![
            Line::default(),
            Line::styled("waiting for the first reading…", off()),
        ];
    };
    let g = |name: &str| v.get(name).copied().unwrap_or(0.0);
    let rate = app
        .ror
        .value()
        .map_or_else(|| "  —".to_owned(), |r| format!("{r:+5.1}"));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // the PLC timer is whole seconds
    let timer = Duration::from_secs(g("timer_s").max(0.0) as u64);

    let mut lines = vec![
        Line::default(),
        row(
            "BT",
            vec![
                Span::styled(format!("{:6.1} °C", g("bt")), bold()),
                "   RoR ".into(),
                Span::styled(rate, bold()),
                " °C/min".into(),
            ],
        ),
        row("ET", vec![format!("{:6.1} °C", g("et")).into()]),
        row("Timer", vec![clock(timer).into()]),
        Line::default(),
        row(
            "Gas",
            vec![format!("{:3.0} %  (valve {:3.0} %)", g("gas_cmd"), g("gas_fb")).into()],
        ),
        row(
            "Drum",
            vec![
                format!(
                    "{:3.0} %  {:3.0} rpm",
                    g("drum"),
                    machine::drum_rpm(g("drum"))
                )
                .into(),
            ],
        ),
        row(
            "Air",
            vec![
                format!(
                    "{:3.0} %  {:3.0} m³/h",
                    g("fan"),
                    machine::airflow_m3h(g("fan"))
                )
                .into(),
            ],
        ),
        Line::default(),
        Line::from(
            [
                ("burner", "BURNER"),
                ("drum_on", "DRUM"),
                ("fan_on", "FAN"),
                ("stirrer", "STIRRER"),
                ("cooler", "COOLER"),
            ]
            .into_iter()
            .flat_map(|(point, text)| [flag(v, point, text), " ".into()])
            .collect::<Vec<_>>(),
        ),
    ];

    let mut alarms = Vec::new();
    for (point, text) in [
        ("emergency", "EMERGENCY STOP"),
        ("flame_fault", "FLAME FAILURE"),
        ("safety_trip", "SAFETY LIMIT"),
    ] {
        if g(point) != 0.0 {
            alarms.extend([Span::styled(text, alarm()), " ".into()]);
        }
    }
    for (point, text) in [("bell", "ROAST POINT"), ("hopper_sensor", "HOPPER")] {
        if g(point) != 0.0 {
            alarms.extend([Span::styled(text, notice()), " ".into()]);
        }
    }
    if !alarms.is_empty() {
        lines.push(Line::from(alarms));
    }
    lines
}

fn flag<'a>(v: &HashMap<String, f64>, point: &str, text: &'a str) -> Span<'a> {
    if v.get(point).is_some_and(|&x| x != 0.0) {
        Span::styled(text, on())
    } else {
        Span::styled(text.to_lowercase(), off())
    }
}

fn link_lines(app: &App) -> Vec<Line<'_>> {
    let status = if app.connected {
        vec![Span::styled("connected", on())]
    } else if app.errors > 0 {
        let wait = app
            .next_retry
            .map_or(Duration::ZERO, |t| t.saturating_duration_since(app.now));
        vec![
            Span::styled("down", alarm()),
            format!(", retry in {}s", wait.as_secs_f64().round()).into(),
        ]
    } else {
        vec![Span::styled("connecting", notice())]
    };
    let last_ok = app.last_ok.map_or_else(
        || "never".to_owned(),
        |t| {
            format!(
                "{:.1} s ago",
                app.now.saturating_duration_since(t).as_secs_f64()
            )
        },
    );
    vec![
        Line::default(),
        row("Status", status),
        row("Latency", vec![fmt_latency(app.latency).into()]),
        row("Last OK", vec![last_ok.into()]),
        row(
            "Polls",
            vec![format!("{}  ok {}  failed {}", app.attempts, app.reads, app.errors).into()],
        ),
        row("Last err", vec![]),
        Line::styled(app.last_err.as_deref().unwrap_or("none"), off()),
    ]
}

fn help_line(app: &App) -> Line<'static> {
    let text = if app.cfg.can_write {
        "b burner · d drum · f fan · s stirrer · c cooler · +/- gas · ]/[ air · }/{ drum · q quit"
    } else {
        "q quit · read-only target"
    };
    Line::styled(text, off())
}

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn fmt_latency(d: Duration) -> String {
    if d < Duration::from_millis(1) {
        format!("{}µs", d.as_micros())
    } else if d < Duration::from_secs(1) {
        format!("{:.2}ms", d.as_secs_f64() * 1000.0)
    } else {
        format!("{:.2}s", d.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use plc::{Event, Reading};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tokio::time::Instant;

    use super::*;
    use crate::app::{Config, Msg};

    fn render(app: &App, width: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(width, 34)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app() -> App {
        App::new(
            Config {
                target: "tcp://127.0.0.1:5502".into(),
                can_write: true,
                time_scale: 1.0,
            },
            Instant::now(),
        )
    }

    #[test]
    fn renders_before_the_first_reading() {
        let screen = render(&app(), 100);
        assert!(
            screen.contains("CONNECTING") && screen.contains("waiting for the first reading"),
            "{screen}"
        );
    }

    #[test]
    fn renders_readings_and_alarms() {
        let mut a = app();
        let values = [("bt", 200.0), ("emergency", 1.0), ("burner", 1.0)]
            .map(|(k, v)| (k.to_owned(), v))
            .into();
        a.update(Msg::Poll(Event {
            at: Instant::now(),
            result: Ok(Reading {
                values,
                latency: Duration::from_micros(540),
            }),
            failures: 0,
            retry_in: Duration::from_secs(1),
        }));
        for width in [100, 50] {
            let screen = render(&a, width);
            for want in [
                "CONNECTED",
                "200.0 °C",
                "BURNER",
                "EMERGENCY STOP",
                "540µs",
                "q quit",
            ] {
                assert!(
                    screen.contains(want),
                    "width {width}: no {want:?} in\n{screen}"
                );
            }
        }
    }

    #[test]
    fn formats_durations() {
        assert_eq!(clock(Duration::from_secs(233)), "03:53");
        assert_eq!(fmt_latency(Duration::from_micros(540)), "540µs");
        assert_eq!(fmt_latency(Duration::from_micros(1500)), "1.50ms");
    }
}
