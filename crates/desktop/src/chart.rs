//! The roast chart, drawn on GPUI's primitives: no chart library draws
//! several series, a second axis, dashed references, phase bands and event
//! lines together (ADR 0003).
// Pixel math converts between f64 roast values and f32 pixels all the time.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use gpui_kit::{
    App, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Point, SharedString, Styled, TextAlign,
    TextRun, Window, canvas, fill, point, px, rgb, size,
};
use roast::EventKind;

use std::cell::Cell;
use std::rc::Rc;

use crate::state::LiveState;

/// Roast-clock range shown (seconds); grows if a roast runs longer.
const X_MIN: f64 = -60.0;
const X_MAX: f64 = 780.0;
/// Temperature axis (°C) and rate-of-rise axis (°C/min).
const T_MIN: f64 = 50.0;
const T_MAX: f64 = 250.0;
const ROR_MAX: f64 = 30.0;
/// Readings further apart than this, in roast seconds per unit of scale, are a Gap.
const GAP_WALL_S: f64 = 2.5;

pub fn bt() -> Hsla {
    rgb(0x00f5_a524).into()
}
pub fn et() -> Hsla {
    rgb(0x007a_a2f7).into()
}
pub fn ror() -> Hsla {
    rgb(0x002d_d4bf).into()
}
pub fn gas() -> Hsla {
    rgb(0x00c0_84fc).into()
}
pub fn air() -> Hsla {
    rgb(0x00a3_e635).into()
}
pub fn faded(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}
fn white(a: f32) -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a,
    }
}

/// One reading on the roast clock: roast seconds, BT, ET, `RoR`, gas, air.
type Row = (
    f64,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

/// Everything the chart needs, copied so the paint closure owns it.
struct Frame {
    live: Vec<Row>,
    /// roast s, bt, et
    reference: Vec<(f64, Option<f64>, Option<f64>)>,
    events: Vec<(EventKind, f64)>,
    /// roast s, BT there
    notes: Vec<(f64, f64)>,
    now_s: f64,
    waiting: bool,
    gap_s: f64,
    /// The roast second under the pointer, and the values drawn as dots there:
    /// live BT, ET, `RoR`, then the reference's BT and ET.
    hover: Option<(f64, [Option<f64>; 5])>,
}

impl Frame {
    fn from(state: &LiveState) -> Self {
        let value = |s: &roastro::wire::SampleOut, k: &str| s.values.get(k).copied();
        let bt_at = |at: i64| {
            state
                .samples
                .iter()
                .min_by_key(|s| (s.at - at).abs())
                .and_then(|s| value(s, "bt"))
        };
        // The rate of rise is drawn only after the turning point ("Live roast screen layout").
        let ror_from = state.event_at(EventKind::TurningPoint);
        Self {
            live: state
                .samples
                .iter()
                .map(|s| {
                    let ror = s.ror.filter(|_| ror_from.is_some_and(|tp| s.at >= tp));
                    (
                        state.roast_s(s.at),
                        value(s, "bt"),
                        value(s, "et"),
                        ror,
                        value(s, "gas_fb"),
                        value(s, "fan"),
                    )
                })
                .collect(),
            reference: state
                .reference
                .as_ref()
                .map(|r| r.points.clone())
                .unwrap_or_default(),
            events: EventKind::ALL
                .into_iter()
                .filter_map(|k| state.event_at(k).map(|at| (k, state.roast_s(at))))
                .collect(),
            notes: state
                .roast
                .iter()
                .flat_map(|r| r.notes.iter())
                .map(|n| (state.roast_s(n.at), bt_at(n.at).unwrap_or(150.0)))
                .collect(),
            now_s: state.now().map_or(0.0, |at| state.roast_s(at)),
            waiting: state.roast.is_none(),
            gap_s: GAP_WALL_S * state.time_scale,
            hover: None,
        }
    }
}

/// Where the main plot was last drawn and what it spans, so the screen can
/// turn a pointer position into a roast second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    plot: Bounds<Pixels>,
    x_min: f64,
    x_max: f64,
}

impl Geometry {
    /// The roast second under a window position, if it is over the plot.
    #[must_use]
    pub fn roast_s(&self, position: Point<Pixels>) -> Option<f64> {
        if !self.plot.contains(&position) {
            return None;
        }
        let fraction = f64::from((position.x - self.plot.left()) / self.plot.size.width);
        Some(self.x_min + fraction * (self.x_max - self.x_min))
    }
}

/// The chart element for the current state. It records where it was drawn in
/// `geometry`, and draws the pointer's crosshair at `hover_s`.
pub fn chart(
    state: &LiveState,
    geometry: Rc<Cell<Option<Geometry>>>,
    hover_s: Option<f64>,
) -> impl IntoElement {
    let mut frame = Frame::from(state);
    frame.hover = hover_s.map(|s| {
        let live = state.sample_near(s);
        let value = |k: &str| live.and_then(|x| x.values.get(k).copied());
        let reference = state.reference_near(s).unwrap_or((None, None));
        let ror = live
            .and_then(|x| x.ror)
            .filter(|_| state.event_at(EventKind::TurningPoint).is_some());
        (s, [value("bt"), value("et"), ror, reference.0, reference.1])
    });
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| paint(bounds, &frame, &geometry, window, cx),
    )
    .size_full()
}

/// Human label of an event, for the chart and the timeline.
pub fn label(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Charge => "Charge",
        EventKind::TurningPoint => "Turning point",
        EventKind::DryEnd => "Dry end",
        EventKind::FirstCrack => "First crack",
        EventKind::SecondCrack => "Second crack",
        EventKind::Drop => "Drop",
    }
}

#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one drawing pass; x/y/s/t read like the math
fn paint(
    bounds: Bounds<Pixels>,
    f: &Frame,
    geometry: &Cell<Option<Geometry>>,
    window: &mut Window,
    cx: &mut App,
) {
    let (left, right, top, bottom) = (px(48.), px(40.), px(28.), px(22.));
    let inner_h = bounds.size.height - top - bottom;
    let strip_h = inner_h * 0.16;
    let main = Bounds::from_corners(
        point(bounds.left() + left, bounds.top() + top),
        point(
            bounds.right() - right,
            bounds.bottom() - bottom - strip_h - px(14.),
        ),
    );
    let strip = Bounds::from_corners(
        point(main.left(), bounds.bottom() - bottom - strip_h),
        point(main.right(), bounds.bottom() - bottom),
    );
    let x_max = X_MAX.max(f.now_s + 60.0);
    let x = |s: f64| main.left() + main.size.width * (((s - X_MIN) / (x_max - X_MIN)) as f32);
    let y_t = |t: f64| {
        main.bottom()
            - main.size.height * (((t.clamp(T_MIN - 10.0, T_MAX) - T_MIN) / (T_MAX - T_MIN)) as f32)
    };
    let y_r =
        |r: f64| main.bottom() - main.size.height * ((r.clamp(-2.0, ROR_MAX) / ROR_MAX) as f32);
    let y_p = |p: f64| strip.bottom() - strip.size.height * ((p.clamp(0.0, 120.0) / 120.0) as f32);
    let in_x = |s: f64| (X_MIN..=x_max).contains(&s);
    geometry.set(Some(Geometry {
        plot: main,
        x_min: X_MIN,
        x_max,
    }));

    // Phase bands.
    if let Some(charge) = at(f, EventKind::Charge) {
        let end = at(f, EventKind::Drop).unwrap_or(f.now_s);
        let de = at(f, EventKind::DryEnd);
        let fc = at(f, EventKind::FirstCrack);
        for (from, to, color) in [
            (Some(charge), de.or(Some(end)), et()),
            (de, fc.or(Some(end)), bt()),
            (fc, Some(end), ror()),
        ] {
            if let (Some(a), Some(b)) = (from, to) {
                let band = Bounds::from_corners(
                    point(x(a), main.top()),
                    point(x(b).max(x(a)), main.bottom()),
                );
                window.paint_quad(fill(band, faded(color, 0.07)));
            }
        }
    }

    // Grid, axes and their labels.
    let grid = white(0.06);
    let label_color = white(0.45);
    let mut s = -60.0;
    while s <= x_max {
        line(
            window,
            &[(x(s), main.top()), (x(s), main.bottom())],
            px(1.),
            grid,
            None,
        );
        line(
            window,
            &[(x(s), strip.top()), (x(s), strip.bottom())],
            px(1.),
            grid,
            None,
        );
        let clock = format!(
            "{}{}:{:02}",
            if s < 0.0 { "−" } else { "" },
            (s.abs() as i64) / 60,
            (s.abs() as i64) % 60
        );
        text(
            window,
            cx,
            &clock,
            point(x(s) - px(12.), strip.bottom() + px(4.)),
            label_color,
        );
        s += 60.0;
    }
    let mut t = T_MIN;
    while t <= T_MAX {
        line(
            window,
            &[(main.left(), y_t(t)), (main.right(), y_t(t))],
            px(1.),
            grid,
            None,
        );
        text(
            window,
            cx,
            &format!("{t:.0}°"),
            point(bounds.left() + px(8.), y_t(t) - px(7.)),
            label_color,
        );
        t += 25.0;
    }
    for r in [0.0, 10.0, 20.0, 30.0] {
        text(
            window,
            cx,
            &format!("{r:.0}"),
            point(main.right() + px(8.), y_r(r) - px(7.)),
            faded(ror(), 0.7),
        );
    }
    text(
        window,
        cx,
        "RoR",
        point(main.right() + px(6.), main.top() - px(18.)),
        faded(ror(), 0.7),
    );
    text(
        window,
        cx,
        "gas · air %",
        point(bounds.left() + px(8.), strip.top()),
        label_color,
    );
    window.paint_quad(fill(
        Bounds::from_corners(
            point(strip.left(), strip.top()),
            point(strip.right(), strip.top() + px(1.)),
        ),
        grid,
    ));

    // The reference roast, dashed, behind.
    let dash = Some([px(6.), px(5.)]);
    for (pick, color, width) in [
        (0usize, faded(bt(), 0.4), px(2.)),
        (1, faded(et(), 0.35), px(1.5)),
    ] {
        let pts: Vec<_> = f
            .reference
            .iter()
            .filter(|r| in_x(r.0))
            .filter_map(|r| if pick == 0 { r.1 } else { r.2 }.map(|v| (x(r.0), y_t(v))))
            .collect();
        line(window, &pts, width, color, dash);
    }

    // Live curves, broken at Gaps.
    for (pick, color, width) in [
        (0usize, bt(), px(2.5)),
        (1, et(), px(2.)),
        (2, ror(), px(1.5)),
        (3, gas(), px(1.5)),
        (4, air(), px(1.5)),
    ] {
        let mut run: Vec<(Pixels, Pixels)> = Vec::new();
        let mut prev: Option<f64> = None;
        for &(s, bt_v, et_v, ror_v, gas_v, air_v) in &f.live {
            let v = [bt_v, et_v, ror_v, gas_v, air_v][pick];
            if prev.is_some_and(|p| s - p > f.gap_s) {
                line(window, &run, width, color, None);
                run.clear();
            }
            prev = Some(s);
            if let Some(v) = v.filter(|_| in_x(s)) {
                let y = match pick {
                    0 | 1 => y_t(v),
                    2 => y_r(v),
                    _ => y_p(v),
                };
                run.push((x(s), y));
            } else {
                line(window, &run, width, color, None);
                run.clear();
            }
        }
        line(window, &run, width, color, None);
    }

    // Events and notes.
    let mut row = 0.0;
    for &(kind, s) in &f.events {
        line(
            window,
            &[(x(s), main.top()), (x(s), main.bottom())],
            px(1.),
            white(0.4),
            Some([px(4.), px(4.)]),
        );
        text(
            window,
            cx,
            label(kind),
            point(x(s) + px(4.), main.top() - px(18.) + px(row)),
            white(0.85),
        );
        row = if row == 0.0 { 12.0 } else { 0.0 };
    }
    for &(s, bt_c) in &f.notes {
        let c = point(x(s), y_t(bt_c) - px(14.));
        window.paint_quad(
            fill(Bounds::centered_at(c, size(px(10.), px(10.))), white(0.95)).corner_radii(px(5.)),
        );
    }
    if f.waiting {
        line(
            window,
            &[(x(0.0), main.top()), (x(0.0), main.bottom())],
            px(1.),
            faded(bt(), 0.6),
            None,
        );
        text(
            window,
            cx,
            "now",
            point(x(0.0) + px(4.), main.top() - px(18.)),
            bt(),
        );
    }

    // The pointer: a crosshair and a dot on every curve under it.
    if let Some((s, values)) = f.hover.filter(|(s, _)| in_x(*s)) {
        line(
            window,
            &[(x(s), main.top()), (x(s), strip.bottom())],
            px(1.),
            white(0.35),
            None,
        );
        let colors = [bt(), et(), ror(), faded(bt(), 0.5), faded(et(), 0.5)];
        for (i, (v, color)) in values.iter().zip(colors).enumerate() {
            let Some(v) = v else { continue };
            let y = if i == 2 { y_r(*v) } else { y_t(*v) };
            let c = point(x(s), y);
            window.paint_quad(
                fill(Bounds::centered_at(c, size(px(11.), px(11.))), white(0.9))
                    .corner_radii(px(5.5)),
            );
            window.paint_quad(
                fill(Bounds::centered_at(c, size(px(7.), px(7.))), color).corner_radii(px(3.5)),
            );
        }
    }
}

fn at(f: &Frame, kind: EventKind) -> Option<f64> {
    f.events.iter().find(|(k, _)| *k == kind).map(|&(_, s)| s)
}

pub(crate) fn line(
    window: &mut Window,
    pts: &[(Pixels, Pixels)],
    width: Pixels,
    color: Hsla,
    dash: Option<[Pixels; 2]>,
) {
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

pub(crate) fn text(window: &mut Window, cx: &mut App, s: &str, origin: Point<Pixels>, color: Hsla) {
    let run = TextRun {
        len: s.len(),
        font: window.text_style().font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped =
        window
            .text_system()
            .shape_line(SharedString::from(s.to_owned()), px(11.), &[run], None);
    let _ = shaped.paint(origin, px(14.), TextAlign::Left, None, window, cx);
}
