//! SPIKE, throwaway: the live roast screen in native Rust with GPUI and
//! gpui-component, fed by the real recorder over the simulator.
//!
//!   cargo run --release                  # embedded simulator at 10x
//!   cargo run --release -- --exit-after 20   # smoke run: quit after 20 s

mod backend;

use std::time::Duration;

use backend::{Cmd, Feed, Reading};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use roast::EventKind;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

const SPEED: f64 = 10.0;
const X_MIN: f64 = -60.0; // roast seconds
const X_MAX: f64 = 780.0;
const T_MIN: f64 = 50.0; // °C
const T_MAX: f64 = 250.0;
const ROR_MAX: f64 = 30.0; // °C/min

fn bt_color() -> Hsla {
    rgb(0xf5a524).into()
}
fn et_color() -> Hsla {
    rgb(0x7aa2f7).into()
}
fn ror_color() -> Hsla {
    rgb(0x2dd4bf).into()
}
fn faded(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

struct LiveRoast {
    points: Vec<Reading>,
    charge_at: Option<i64>,
    events: Vec<(EventKind, i64)>,
    notes: Vec<(i64, String)>,
    reference: Vec<(f64, f64, f64)>,
    link_up: bool,
    note: Entity<InputState>,
    cmd: UnboundedSender<Cmd>,
}

impl LiveRoast {
    fn apply(&mut self, feed: Feed) {
        match feed {
            Feed::Sample(p) => {
                self.points.push(p);
                if self.charge_at.is_none() && self.points.len() > 200 {
                    self.points.drain(..self.points.len() - 200);
                }
            }
            Feed::Started(at) => {
                self.charge_at = Some(at);
                self.events = vec![(EventKind::Charge, at)];
                self.notes.clear();
                let lead_in = at - (60_000.0 / SPEED) as i64;
                self.points.retain(|p| p.at >= lead_in);
            }
            Feed::Event(kind, at) => self.events.push((kind, at)),
            Feed::Closed => {}
            Feed::Link(up) => self.link_up = up,
        }
    }

    fn now(&self) -> i64 {
        self.points.last().map_or(0, |p| p.at)
    }

    /// Roast seconds of an instant (relative to now while waiting).
    fn roast_s(&self, at: i64) -> f64 {
        let zero = self.charge_at.unwrap_or_else(|| self.now());
        (at - zero) as f64 * SPEED / 1000.0
    }

    fn mark(&mut self, kind: EventKind) {
        if self.charge_at.is_some() && !self.events.iter().any(|(k, _)| *k == kind) {
            let now = self.now();
            self.events.push((kind, now));
        }
    }
}

fn readout(label: &'static str, value: String, color: Hsla) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .child(div().text_xs().text_color(faded(white(), 0.45)).child(label))
        .child(div().text_3xl().font_weight(FontWeight::BOLD).text_color(color).child(value))
}

impl Render for LiveRoast {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let last = self.points.last().copied();
        let fmt = |v: Option<f64>| v.map_or("—".to_owned(), |v| format!("{v:.1}"));
        let clock = match self.charge_at {
            Some(c) => {
                let s = self.roast_s(self.now()).max(0.0) as i64;
                let _ = c;
                format!("{}:{:02}", s / 60, s % 60)
            }
            None => "–:––".to_owned(),
        };
        let status = if !self.link_up {
            "PLC disconnected"
        } else if self.charge_at.is_some() {
            "Recording"
        } else {
            "Waiting for charge"
        };

        let chart = chart_canvas(self);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x0b0c0e))
            .text_color(white())
            .p_4()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_8()
                    .child(readout("ROAST TIME", clock, white()))
                    .child(readout("BT °C", fmt(last.and_then(|p| p.bt)), bt_color()))
                    .child(readout("RoR °C/min", fmt(last.and_then(|p| p.ror)), ror_color()))
                    .child(readout("ET °C", fmt(last.and_then(|p| p.et)), et_color()))
                    .child(div().flex_1())
                    .child(div().text_sm().text_color(faded(white(), 0.6)).child(status))
                    .child(Button::new("charge").label("Charge").on_click(cx.listener(|this, _, _, _| {
                        let _ = this.cmd.send(Cmd::Charge);
                    })))
                    .child(Button::new("dry-end").label("Dry end").on_click(cx.listener(|this, _, _, cx| {
                        this.mark(EventKind::DryEnd);
                        cx.notify();
                    })))
                    .child(Button::new("first-crack").label("First crack").on_click(cx.listener(|this, _, _, cx| {
                        this.mark(EventKind::FirstCrack);
                        cx.notify();
                    })))
                    .child(Button::new("drop").label("Drop").on_click(cx.listener(|this, _, _, _| {
                        let _ = this.cmd.send(Cmd::Drop);
                    }))),
            )
            .child(div().flex_1().w_full().child(chart))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Input::new(&self.note).id("note").w(px(420.)))
                    .child(Button::new("add-note").label("Add note").on_click(cx.listener(|this, _, window, cx| {
                        let text = this.note.read(cx).value().to_string();
                        if !text.trim().is_empty() && this.charge_at.is_some() {
                            let now = this.now();
                            this.notes.push((now, text));
                            this.note = cx.new(|cx| InputState::new(window, cx));
                            cx.notify();
                        }
                    }))),
            )
    }
}

/// Everything the chart needs, copied so the paint closure owns it.
struct ChartData {
    live: Vec<(f64, Option<f64>, Option<f64>, Option<f64>)>, // roast s, bt, et, ror
    reference: Vec<(f64, f64, f64)>,
    events: Vec<(EventKind, f64)>,
    notes: Vec<(f64, f64)>, // roast s, bt at that time
    now_s: f64,
    recording: bool,
}

fn chart_canvas(view: &LiveRoast) -> impl IntoElement {
    let bt_at = |at: i64| view.points.iter().min_by_key(|p| (p.at - at).abs()).and_then(|p| p.bt);
    let data = ChartData {
        live: view.points.iter().map(|p| (view.roast_s(p.at), p.bt, p.et, p.ror)).collect(),
        reference: view.reference.clone(),
        events: view.events.iter().map(|&(k, at)| (k, view.roast_s(at))).collect(),
        notes: view.notes.iter().map(|(at, _)| (view.roast_s(*at), bt_at(*at).unwrap_or(150.0))).collect(),
        now_s: view.roast_s(view.now()),
        recording: view.charge_at.is_some(),
    };
    canvas(|_, _, _| {}, move |bounds, (), window, cx| paint_chart(bounds, &data, window, cx)).size_full()
}

fn paint_chart(bounds: Bounds<Pixels>, d: &ChartData, window: &mut Window, cx: &mut App) {
    let (left, right, top, bottom) = (px(48.), px(44.), px(12.), px(24.));
    let plot = Bounds::from_corners(
        point(bounds.left() + left, bounds.top() + top),
        point(bounds.right() - right, bounds.bottom() - bottom),
    );
    let x = |s: f64| plot.left() + plot.size.width * (((s - X_MIN) / (X_MAX - X_MIN)) as f32);
    let y_t = |t: f64| plot.bottom() - plot.size.height * (((t - T_MIN) / (T_MAX - T_MIN)) as f32);
    let y_r = |r: f64| plot.bottom() - plot.size.height * ((r.clamp(-5.0, ROR_MAX) / ROR_MAX) as f32);

    window.paint_quad(fill(bounds, rgb(0x0b0c0e)));

    // Phase bands.
    let at = |k| d.events.iter().find(|(e, _)| *e == k).map(|&(_, s)| s);
    if let Some(charge) = at(EventKind::Charge) {
        let end = at(EventKind::Drop).unwrap_or(d.now_s);
        let de = at(EventKind::DryEnd);
        let fc = at(EventKind::FirstCrack);
        let bands = [
            (Some(charge), de.or(Some(end)), rgb(0x7aa2f7)),
            (de, fc.or(Some(end)), rgb(0xf5a524)),
            (fc, Some(end), rgb(0x2dd4bf)),
        ];
        for (from, to, color) in bands {
            if let (Some(a), Some(b)) = (from, to) {
                let band = Bounds::from_corners(point(x(a), plot.top()), point(x(b), plot.bottom()));
                window.paint_quad(fill(band, faded(color.into(), 0.07)));
            }
        }
    }

    // Grid and axis labels.
    let grid = faded(white(), 0.06);
    let label = faded(white(), 0.45);
    let mut s = -60.0;
    while s <= X_MAX {
        stroke(window, &[(x(s), plot.top()), (x(s), plot.bottom())], px(1.), grid, None);
        text(window, cx, &format!("{}:{:02}", (s as i64).div_euclid(60), (s as i64).rem_euclid(60)), point(x(s) - px(12.), plot.bottom() + px(4.)), label);
        s += 120.0;
    }
    let mut t = T_MIN;
    while t <= T_MAX {
        stroke(window, &[(plot.left(), y_t(t)), (plot.right(), y_t(t))], px(1.), grid, None);
        text(window, cx, &format!("{t:.0}°"), point(bounds.left() + px(6.), y_t(t) - px(7.)), label);
        t += 50.0;
    }
    for r in [0.0, 10.0, 20.0, 30.0] {
        text(window, cx, &format!("{r:.0}"), point(plot.right() + px(8.), y_r(r) - px(7.)), faded(ror_color(), 0.6));
    }

    // Reference roast, dashed, behind.
    let dash = Some([px(6.), px(5.)]);
    let refs_bt: Vec<_> = d.reference.iter().filter(|r| r.0 >= X_MIN).map(|r| (x(r.0), y_t(r.1))).collect();
    let refs_et: Vec<_> = d.reference.iter().filter(|r| r.0 >= X_MIN).map(|r| (x(r.0), y_t(r.2))).collect();
    stroke(window, &refs_bt, px(2.), faded(bt_color(), 0.35), dash);
    stroke(window, &refs_et, px(1.5), faded(et_color(), 0.3), dash);

    // Live curves, broken at Gaps.
    for (pick, color, width, on_ror) in [
        (1usize, bt_color(), px(2.5), false),
        (2, et_color(), px(2.), false),
        (3, ror_color(), px(1.5), true),
    ] {
        let mut run: Vec<(Pixels, Pixels)> = Vec::new();
        let mut prev_s: Option<f64> = None;
        for &(s, bt, et, ror) in &d.live {
            let v = match pick {
                1 => bt,
                2 => et,
                _ => ror,
            };
            let gap = prev_s.is_some_and(|p| (s - p) > 2.5 * SPEED);
            prev_s = Some(s);
            match v {
                Some(v) if s >= X_MIN && !gap => run.push((x(s), if on_ror { y_r(v) } else { y_t(v) })),
                Some(v) if s >= X_MIN => {
                    stroke(window, &run, width, color, None);
                    run = vec![(x(s), if on_ror { y_r(v) } else { y_t(v) })];
                }
                _ => {
                    stroke(window, &run, width, color, None);
                    run.clear();
                }
            }
        }
        stroke(window, &run, width, color, None);
    }

    // Events and notes.
    for &(kind, s) in &d.events {
        stroke(window, &[(x(s), plot.top()), (x(s), plot.bottom())], px(1.), faded(white(), 0.4), Some([px(4.), px(4.)]));
        text(window, cx, kind.as_str(), point(x(s) + px(4.), plot.top() + px(2.)), faded(white(), 0.85));
    }
    for &(s, bt) in &d.notes {
        let c = point(x(s), y_t(bt) - px(14.));
        window.paint_quad(fill(Bounds::centered_at(c, size(px(12.), px(12.))), white()).corner_radii(px(6.)));
    }
    if !d.recording {
        stroke(window, &[(x(0.0), plot.top()), (x(0.0), plot.bottom())], px(1.), faded(bt_color(), 0.6), None);
        text(window, cx, "now", point(x(0.0) + px(4.), plot.top() + px(2.)), bt_color());
    }
}

fn stroke(window: &mut Window, pts: &[(Pixels, Pixels)], width: Pixels, color: Hsla, dash: Option<[Pixels; 2]>) {
    if pts.len() < 2 {
        return;
    }
    let mut b = PathBuilder::stroke(width);
    if let Some(d) = dash {
        b = b.dash_array(&d);
    }
    b.move_to(point(pts[0].0, pts[0].1));
    for &(px_x, px_y) in &pts[1..] {
        b.line_to(point(px_x, px_y));
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

fn text(window: &mut Window, cx: &mut App, s: &str, origin: Point<Pixels>, color: Hsla) {
    let style = window.text_style();
    let run = TextRun {
        len: s.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(SharedString::from(s.to_owned()), px(11.), &[run], None);
    let _ = line.paint(origin, px(14.), TextAlign::Left, None, window, cx);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exit_after = args.iter().position(|a| a == "--exit-after").and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<u64>().ok());
    let url = args.iter().position(|a| a == "--url").and_then(|i| args.get(i + 1)).cloned();
    let sim_port = args.iter().position(|a| a == "--sim-port").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(5502u16);

    let map = regmap::Map::from_json(include_str!("../../../configs/regmap.json")).expect("register map");
    let (feed_tx, mut feed_rx) = unbounded_channel();
    let (cmd_tx, cmd_rx) = unbounded_channel();
    backend::start(
        backend::Settings {
            embedded_sim: url.is_none(),
            url: url.unwrap_or_else(|| format!("tcp://127.0.0.1:{sim_port}")),
            sim_port,
            map,
            time_scale: SPEED,
        },
        feed_tx,
        cmd_rx,
    );
    let reference = backend::reference_curve();

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(800.)), cx))),
            ..Default::default()
        };
        let (_, view) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| LiveRoast {
                points: Vec::new(),
                charge_at: None,
                events: Vec::new(),
                notes: Vec::new(),
                reference,
                link_up: false,
                note: cx.new(|cx| InputState::new(window, cx)),
                cmd: cmd_tx,
            })
        })
        .expect("open window");

        // Readings arrive from the Tokio thread; the GPUI executor awaits them.
        view.update(cx, |_, cx| {
            cx.spawn(async move |this, cx| {
                while let Some(feed) = feed_rx.recv().await {
                    if this.update(cx, |v, cx| {
                        v.apply(feed);
                        cx.notify();
                    }).is_err() {
                        break;
                    }
                }
            })
            .detach();
        });

        if let Some(secs) = exit_after {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(secs)).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
    });
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    // No `use super::*`: it would bring gpui_kit's `test` into scope and make
    // the attribute's own `#[test]` expand to itself.
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size};
    use tokio::sync::mpsc::unbounded_channel;

    use super::{Cmd, EventKind, Feed, InputState, LiveRoast, Reading};

    #[gpui_kit::test]
    fn marks_events_takes_notes_and_sends_charge(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (cmd_tx, mut cmd_rx) = unbounded_channel();
        let (handle, view) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds { origin: Point::default(), size: size(px(1280.), px(800.)) })),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| LiveRoast {
                    points: Vec::new(),
                    charge_at: None,
                    events: Vec::new(),
                    notes: Vec::new(),
                    reference: Vec::new(),
                    link_up: true,
                    note: cx.new(|cx| InputState::new(window, cx)),
                    cmd: cmd_tx,
                })
            })
            .expect("open window")
        });
        cx.update(|cx| {
            view.update(cx, |v, _| {
                v.apply(Feed::Started(0));
                for s in 0..=30 {
                    v.apply(Feed::Sample(Reading { at: s * 100, bt: Some(100.0 + s as f64), et: Some(150.0), ror: Some(12.0) }));
                }
            })
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("charge", cx);
            window.click("dry-end", cx);
            window.click("note", cx);
            window.input("smell of bread", cx);
            window.click("add-note", cx);
        })
        .unwrap();

        let (events, notes) = cx.update(|cx| {
            let v = view.read(cx);
            (v.events.clone(), v.notes.clone())
        });
        assert!(events.iter().any(|(k, _)| *k == EventKind::DryEnd), "{events:?}");
        assert_eq!(notes.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["smell of bread"]);
        assert!(matches!(cmd_rx.try_recv(), Ok(Cmd::Charge)), "the Charge button reached the backend");
    }
}
