//! Compare: two to five roasts side by side ("Roast history and curve
//! comparison", #10). Their curves overlaid, one color each, aligned on
//! Charge or on First crack, and their figures in columns with the
//! differences against the first. The only page that shows several roasts.
// Pixel math converts between f64 roast values and f32 pixels all the time.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, EventEmitter, FontWeight, Hsla, IntoElement, MouseMoveEvent,
    Pixels, Point, SharedString, Window, anchored, canvas, deferred, div, point, px, rgb,
};
use roast::EventKind;
use roastro::RoastId;
use roastro::engine::{Edit, Request};
use roastro::wire::{Columns, NextRoastIn, RoastDoc};
use uuid::Uuid;

use crate::chart::{self, line, text};
use crate::roasts::{OpenRoast, day};
use crate::state::Millis;
use crate::view::rail::label;
use crate::view::{Engine, clock, fmt1, white};

/// The shell compares these roasts.
pub struct CompareRoasts(pub Vec<Uuid>);

/// At most this many roasts on one chart: more stops being readable.
pub const MAX: usize = 5;

const T_MIN: f64 = 50.0;
const T_MAX: f64 = 250.0;
const ROR_MAX: f64 = 30.0;

/// One color per roast, in the order they were picked.
fn color(i: usize) -> Hsla {
    rgb([
        0x00f5_a524,
        0x007a_a2f7,
        0x00f4_72b6,
        0x002d_d4bf,
        0x00a3_e635,
    ][i % MAX])
    .into()
}

/// Where the curves line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Charge,
    FirstCrack,
}

/// Roast seconds, BT, ET, `RoR`.
type Point4 = (f64, Option<f64>, Option<f64>, Option<f64>);

/// One roast, ready to draw: roast seconds since Charge, BT, ET, `RoR`.
struct Loaded {
    doc: RoastDoc,
    points: Vec<Point4>,
    first_crack: Option<f64>,
    drop: Option<f64>,
}

impl Loaded {
    fn new(doc: RoastDoc, columns: &Columns, time_scale: f64) -> Self {
        let charge = doc.charge_at.unwrap_or(0);
        let s = |at: Millis| (at - charge) as f64 * time_scale / 1000.0;
        let bt = columns.values.get("bt");
        let et = columns.values.get("et");
        let tp = doc
            .events
            .iter()
            .find(|e| e.kind == "turning_point")
            .map(|e| e.at);
        let points = columns
            .at
            .iter()
            .enumerate()
            .map(|(i, &at)| {
                let ror = columns.ror[i].filter(|_| tp.is_some_and(|tp| at >= tp));
                (s(at), bt.and_then(|c| c[i]), et.and_then(|c| c[i]), ror)
            })
            .collect();
        let event = |kind: EventKind| {
            doc.events
                .iter()
                .find(|e| e.kind == kind.as_str())
                .map(|e| s(e.at))
        };
        Self {
            first_crack: event(EventKind::FirstCrack),
            drop: event(EventKind::Drop),
            points,
            doc,
        }
    }

    /// How far its curve moves when aligned this way.
    fn shift(&self, align: Align) -> f64 {
        match align {
            Align::Charge => 0.0,
            Align::FirstCrack => self.first_crack.unwrap_or(0.0),
        }
    }

    fn at(&self, s: f64) -> Option<&Point4> {
        self.points
            .iter()
            .min_by(|a, b| (a.0 - s).abs().total_cmp(&(b.0 - s).abs()))
            .filter(|p| (p.0 - s).abs() <= 5.0)
    }
}

/// Where the plot was drawn, to read the pointer.
#[derive(Debug, Clone, Copy)]
struct Plot {
    bounds: Bounds<Pixels>,
    x_min: f64,
    x_max: f64,
}

/// The Compare page.
pub struct ComparePage {
    engine: Engine,
    time_scale: f64,
    /// The roasts asked for, in order; loaded ones fill `loaded`.
    ids: Vec<Uuid>,
    loaded: Vec<Loaded>,
    align: Align,
    show_ror: bool,
    show_et: bool,
    message: Option<String>,
    plot: Rc<Cell<Option<Plot>>>,
    hover: Option<(Point<Pixels>, f64)>,
}

impl EventEmitter<OpenRoast> for ComparePage {}

impl ComparePage {
    pub fn new(engine: Engine, ids: Vec<Uuid>, time_scale: f64, cx: &mut Context<Self>) -> Self {
        let mut ids = ids;
        ids.dedup();
        ids.truncate(MAX);
        let page = Self {
            engine,
            time_scale,
            ids,
            loaded: Vec::new(),
            align: Align::Charge,
            show_ror: true,
            show_et: false,
            message: None,
            plot: Rc::new(Cell::new(None)),
            hover: None,
        };
        page.load(cx);
        page
    }

    fn load(&self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let ids = self.ids.clone();
        let time_scale = self.time_scale;
        cx.spawn(async move |this, cx| {
            let mut loaded = Vec::new();
            for id in ids {
                let rid = RoastId(id);
                let doc = engine.ask(|r| Request::Roast(rid, r))?.await.ok()?.ok();
                let columns = engine.ask(|r| Request::Samples(rid, r))?.await.ok()?.ok();
                if let Some((doc, columns)) = doc.zip(columns) {
                    loaded.push(Loaded::new(doc, &columns, time_scale));
                }
            }
            this.update(cx, |this, cx| {
                this.loaded = loaded;
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn remove(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.ids.retain(|x| *x != id);
        self.loaded.retain(|l| l.doc.id != id);
        cx.notify();
    }

    fn use_as_reference(&mut self, id: Uuid, number: i64, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let next = engine
                .ask(|r| Request::Edit(Edit::GetNextRoast(r)))?
                .await
                .ok()?
                .ok()?;
            let next = NextRoastIn {
                coffee_id: next.coffee_id,
                green_weight_g: next.green_weight_g,
                roasted_by: next.roasted_by,
                reference_id: Some(id),
            };
            let result = engine
                .ask(|r| Request::Edit(Edit::PutNextRoast(next, r)))?
                .await
                .ok()?;
            this.update(cx, |this, cx| {
                this.message = Some(match result {
                    Ok(_) => format!("The next roast will follow Roast #{number}."),
                    Err(err) => err.to_string(),
                });
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn pointer_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let s = self.plot.get().and_then(|p| {
            p.bounds.contains(&position).then(|| {
                let fraction = f64::from((position.x - p.bounds.left()) / p.bounds.size.width);
                p.x_min + fraction * (p.x_max - p.x_min)
            })
        });
        let hover = s.map(|s| (position, s));
        if hover.map(|h| h.1) != self.hover.map(|h| h.1) {
            self.hover = hover;
            cx.notify();
        }
    }

    /// The x range the curves need, aligned.
    fn x_range(&self) -> (f64, f64) {
        let shifted = self.loaded.iter().map(|l| {
            let end = l.drop.or(l.points.last().map(|p| p.0)).unwrap_or(600.0);
            (-l.shift(self.align), end - l.shift(self.align))
        });
        let (lo, hi) = shifted.fold((0.0_f64, 60.0_f64), |(lo, hi), (a, b)| {
            (lo.min(a), hi.max(b))
        });
        (lo.min(0.0) - 15.0, hi + 30.0)
    }

    fn chart(&self) -> impl IntoElement {
        let (x_min, x_max) = self.x_range();
        let align = self.align;
        let (show_ror, show_et) = (self.show_ror, self.show_et);
        let series: Vec<_> = self
            .loaded
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let shift = l.shift(align);
                let points: Vec<_> = l
                    .points
                    .iter()
                    .map(|p| (p.0 - shift, p.1, p.2, p.3))
                    .collect();
                let marks: Vec<(f64, Option<f64>)> = [l.first_crack, l.drop]
                    .into_iter()
                    .flatten()
                    .map(|s| (s - shift, l.at(s).and_then(|p| p.1)))
                    .collect();
                (color(i), points, marks)
            })
            .collect();
        let hover_s = self.hover.map(|h| h.1);
        let plot = self.plot.clone();
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                paint(
                    bounds,
                    &series,
                    (x_min, x_max),
                    align,
                    (show_ror, show_et),
                    hover_s,
                    &plot,
                    window,
                    cx,
                );
            },
        )
        .size_full()
    }

    fn hover_card(&self) -> Option<AnyElement> {
        let (position, s) = self.hover?;
        let rows: Vec<_> = self
            .loaded
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let p = l.at(s + l.shift(self.align))?;
                Some(
                    div()
                        .flex()
                        .gap_3()
                        .text_sm()
                        .child(div().size(px(9.)).mt(px(5.)).rounded_sm().bg(color(i)))
                        .child(div().w(px(56.)).child(format!("#{}", l.doc.number)))
                        .child(
                            div()
                                .w(px(64.))
                                .text_color(chart::bt())
                                .child(format!("{}°", fmt1(p.1))),
                        )
                        .child(div().w(px(48.)).text_color(chart::ror()).child(fmt1(p.3))),
                )
            })
            .collect();
        if rows.is_empty() {
            return None;
        }
        let title = match self.align {
            Align::Charge => clock(s),
            Align::FirstCrack => {
                format!("FC {}{}", if s < 0.0 { "−" } else { "+" }, clock(s.abs()))
            }
        };
        Some(
            deferred(
                anchored()
                    .position(position + point(px(16.), px(16.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .rounded_lg()
                            .bg(rgb(0x0018_1a1f))
                            .border_1()
                            .border_color(white(0.12))
                            .shadow_lg()
                            .child(div().font_weight(FontWeight::BOLD).child(title))
                            .children(rows),
                    ),
            )
            .into_any_element(),
        )
    }

    /// The figures, one column per roast, differences against the first.
    fn figures(&self) -> impl IntoElement {
        let figures: Vec<[Option<f64>; 7]> = self.loaded.iter().map(figures).collect();
        let names = [
            "Total",
            "First crack",
            "Development",
            "DTR",
            "Drop BT",
            "Weight loss",
            "RoR fall",
        ];
        let show = |row: usize, v: Option<f64>| match (row, v) {
            (_, None) => "—".to_owned(),
            (0..=2, Some(v)) => clock(v),
            (3 | 5, Some(v)) => format!("{v:.1}%"),
            (4, Some(v)) => format!("{v:.1}°"),
            (_, Some(v)) => format!("{v:.1}"),
        };
        let diff = |row: usize, d: f64| {
            let sign = if d < 0.0 { "−" } else { "+" };
            match row {
                0..=2 => format!("{sign}{}", clock(d.abs())),
                _ => format!("{sign}{:.1}", d.abs()),
            }
        };
        let base = figures.first().copied();
        div()
            .id("compare-figures")
            .test_support()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().flex().gap_4().child(div().w(px(120.))).children(
                self.loaded.iter().enumerate().map(|(i, l)| {
                    div()
                        .w(px(150.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().size(px(10.)).rounded_sm().bg(color(i)))
                        .child(label(format!("#{}", l.doc.number)))
                }),
            ))
            .children(names.iter().enumerate().map(|(row, name)| {
                div()
                    .flex()
                    .gap_4()
                    .text_sm()
                    .child(div().w(px(120.)).text_color(white(0.55)).child(*name))
                    .children(figures.iter().enumerate().map(|(col, f)| {
                        let v = f[row];
                        let d = (col > 0)
                            .then(|| base.and_then(|b| v.zip(b[row])).map(|(v, b)| v - b))
                            .flatten();
                        div()
                            .w(px(150.))
                            .flex()
                            .gap_2()
                            .child(show(row, v))
                            .children(d.map(|d| div().text_color(white(0.45)).child(diff(row, d))))
                    }))
            }))
    }
}

/// Total, First crack, development, DTR %, Drop BT, loss %, `RoR` fall.
fn figures(l: &Loaded) -> [Option<f64>; 7] {
    let m = &l.doc.metrics;
    let summary = l.doc.summary.as_ref();
    [
        l.drop,
        l.first_crack,
        l.drop.zip(l.first_crack).map(|(d, f)| d - f),
        m.development_time_ratio.map(|d| d * 100.0),
        summary.and_then(|s| s.drop_bt),
        m.weight_loss_ratio.map(|w| w * 100.0),
        summary.and_then(|s| s.ror_fall),
    ]
}

type Series = (Hsla, Vec<Point4>, Vec<(f64, Option<f64>)>);

#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one drawing pass
fn paint(
    bounds: Bounds<Pixels>,
    series: &[Series],
    (x_min, x_max): (f64, f64),
    align: Align,
    (show_ror, show_et): (bool, bool),
    hover_s: Option<f64>,
    plot: &Cell<Option<Plot>>,
    window: &mut Window,
    cx: &mut App,
) {
    let main = Bounds::from_corners(
        point(bounds.left() + px(48.), bounds.top() + px(16.)),
        point(bounds.right() - px(40.), bounds.bottom() - px(22.)),
    );
    plot.set(Some(Plot {
        bounds: main,
        x_min,
        x_max,
    }));
    let x = |s: f64| main.left() + main.size.width * (((s - x_min) / (x_max - x_min)) as f32);
    let y_t = |t: f64| {
        main.bottom()
            - main.size.height * (((t.clamp(T_MIN - 10.0, T_MAX) - T_MIN) / (T_MAX - T_MIN)) as f32)
    };
    let y_r =
        |r: f64| main.bottom() - main.size.height * ((r.clamp(-2.0, ROR_MAX) / ROR_MAX) as f32);
    let grid = white(0.06);
    let label_color = white(0.45);

    let mut s = (x_min / 60.0).ceil() * 60.0;
    while s <= x_max {
        line(
            window,
            &[(x(s), main.top()), (x(s), main.bottom())],
            px(1.),
            grid,
            None,
        );
        let prefix = if s < 0.0 { "−" } else { "" };
        text(
            window,
            cx,
            &format!("{prefix}{}", clock(s.abs())),
            point(x(s) - px(12.), main.bottom() + px(4.)),
            label_color,
        );
        s += 60.0;
    }
    if align == Align::FirstCrack {
        line(
            window,
            &[(x(0.0), main.top()), (x(0.0), main.bottom())],
            px(1.),
            white(0.3),
            Some([px(3.), px(4.)]),
        );
        text(
            window,
            cx,
            "First crack",
            point(x(0.0) + px(4.), main.top()),
            label_color,
        );
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
    if show_ror {
        for r in [0.0, 10.0, 20.0, 30.0] {
            text(
                window,
                cx,
                &format!("{r:.0}"),
                point(main.right() + px(8.), y_r(r) - px(7.)),
                chart::faded(chart::ror(), 0.7),
            );
        }
    }

    // A curve, broken where a value is missing.
    let curve = |window: &mut Window,
                 points: &[Point4],
                 pick: fn(&Point4) -> Option<f64>,
                 y: &dyn Fn(f64) -> Pixels,
                 width: Pixels,
                 color: Hsla,
                 dash: Option<[Pixels; 2]>| {
        let mut run: Vec<(Pixels, Pixels)> = Vec::new();
        for p in points {
            if !(x_min..=x_max).contains(&p.0) {
                continue;
            }
            if let Some(v) = pick(p) {
                run.push((x(p.0), y(v)));
            } else {
                line(window, &run, width, color, dash);
                run.clear();
            }
        }
        line(window, &run, width, color, dash);
    };
    for (color, points, marks) in series {
        if show_et {
            curve(
                window,
                points,
                |p| p.2,
                &y_t,
                px(1.2),
                chart::faded(*color, 0.5),
                Some([px(5.), px(4.)]),
            );
        }
        if show_ror {
            curve(
                window,
                points,
                |p| p.3,
                &y_r,
                px(1.2),
                chart::faded(*color, 0.8),
                Some([px(2.), px(3.)]),
            );
        }
        curve(window, points, |p| p.1, &y_t, px(2.), *color, None);
        for &(s, bt) in marks {
            if let Some(bt) = bt {
                let c = point(x(s), y_t(bt));
                window.paint_quad(gpui_kit::fill(
                    Bounds::from_corners(c - point(px(3.), px(3.)), c + point(px(3.), px(3.))),
                    *color,
                ));
            }
        }
    }
    if let Some(s) = hover_s {
        line(
            window,
            &[(x(s), main.top()), (x(s), main.bottom())],
            px(1.),
            white(0.3),
            None,
        );
    }
}

impl Render for ComparePage {
    #[allow(clippy::too_many_lines)] // the page's layout, read top to bottom
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toggle = |id: &'static str, text: &'static str, on: bool| {
            div()
                .id(id)
                .test_support()
                .px_3()
                .py_1()
                .rounded_full()
                .border_1()
                .cursor_pointer()
                .text_sm()
                .border_color(if on {
                    chart::faded(chart::bt(), 0.5)
                } else {
                    white(0.1)
                })
                .text_color(if on { chart::bt() } else { white(0.6) })
                .child(text)
        };
        let chips = self.loaded.iter().enumerate().map(|(i, l)| {
            let id = l.doc.id;
            let number = l.doc.number;
            let coffee = l
                .doc
                .coffee
                .as_ref()
                .map_or_else(|| "No coffee set".to_owned(), |c| c.name.clone());
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .rounded_lg()
                .border_1()
                .border_color(chart::faded(color(i), 0.4))
                .child(div().size(px(10.)).rounded_sm().bg(color(i)))
                .child(
                    div()
                        .id(SharedString::from(format!("compare-open-{number}")))
                        .test_support()
                        .cursor_pointer()
                        .font_weight(FontWeight::SEMIBOLD)
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenRoast(id))))
                        .child(format!("#{number}")),
                )
                .child(div().text_sm().text_color(white(0.6)).child(coffee))
                .child(
                    div()
                        .text_xs()
                        .text_color(white(0.4))
                        .child(l.doc.charge_at.map(day).unwrap_or_default()),
                )
                .child(
                    Button::new(SharedString::from(format!("compare-reference-{number}")))
                        .ghost()
                        .xsmall()
                        .label("Use as reference")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.use_as_reference(id, number, cx);
                        })),
                )
                .children((self.loaded.len() > 2).then(|| {
                    Button::new(SharedString::from(format!("compare-remove-{number}")))
                        .ghost()
                        .xsmall()
                        .label("✕")
                        .on_click(cx.listener(move |this, _, _, cx| this.remove(id, cx)))
                }))
        });
        let (align, ror, et) = (self.align, self.show_ror, self.show_et);
        div()
            .id("page-compare")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .children(self.hover_card())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(white(0.08))
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .child("Compare"),
                    )
                    .children(chips)
                    .child(div().flex_1())
                    .child(
                        toggle("align-charge", "Align on Charge", align == Align::Charge).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.align = Align::Charge;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        toggle(
                            "align-fc",
                            "Align on First crack",
                            align == Align::FirstCrack,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.align = Align::FirstCrack;
                            cx.notify();
                        })),
                    )
                    .child(toggle("show-ror", "RoR", ror).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.show_ror = !this.show_ror;
                            cx.notify();
                        },
                    )))
                    .child(
                        toggle("show-et", "ET", et).on_click(cx.listener(|this, _, _, cx| {
                            this.show_et = !this.show_et;
                            cx.notify();
                        })),
                    ),
            )
            .children(self.message.clone().map(|m| {
                div()
                    .px_6()
                    .pt_2()
                    .text_sm()
                    .text_color(white(0.7))
                    .child(m)
            }))
            .child(
                div()
                    .id("compare-chart")
                    .test_support()
                    .flex_1()
                    .min_h(px(280.))
                    .p_4()
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        this.pointer_moved(ev.position, cx);
                    }))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !hovered {
                            this.hover = None;
                            cx.notify();
                        }
                    }))
                    .child(self.chart()),
            )
            .child(
                div()
                    .px_6()
                    .py_4()
                    .border_t_1()
                    .border_color(white(0.08))
                    .child(self.figures())
                    .child(div().pt_2().text_xs().text_color(white(0.35)).child(
                        "Solid BT, dotted RoR, dashed ET. Differences are against the first roast.",
                    )),
            )
    }
}
