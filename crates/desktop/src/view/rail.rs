//! The rail beside the chart (variant D): the readouts, or the roast's summary
//! once it is done; the phases; the timeline, whose "now" row carries the
//! marks; the roasted weight after Drop. With "Big numbers" on, the readouts
//! move to a strip across the top instead.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{Context, FontWeight, Hsla, IntoElement, SharedString, div, px};
use roast::EventKind;
use roastro::wire::{EventOut, NoteOut};

use super::{RoastScreen, clock, fmt0, fmt1, white};
use crate::chart;
use crate::state::LiveState;

/// The amber of the brand, the primary buttons and the "now" row.
pub fn accent() -> Hsla {
    chart::bt()
}

/// How large a figure is drawn.
#[derive(Debug, Clone, Copy)]
pub enum Size {
    Huge,
    Large,
    Medium,
}

/// One number on screen: a label over a value and its unit, maybe a line
/// under it (the reference, a percentage).
pub struct Figure {
    label: &'static str,
    value: String,
    unit: &'static str,
    color: Hsla,
    sub: Option<String>,
}

impl Figure {
    fn new(label: &'static str, value: String, unit: &'static str, color: Hsla) -> Self {
        Self {
            label,
            value,
            unit,
            color,
            sub: None,
        }
    }

    fn sub(mut self, sub: Option<String>) -> Self {
        self.sub = sub;
        self
    }

    pub fn render(self, size: Size) -> impl IntoElement {
        let number = div()
            .font_weight(FontWeight::BOLD)
            .text_color(self.color)
            .line_height(gpui_kit::relative(1.0))
            .child(self.value);
        let number = match size {
            Size::Huge => number.text_size(px(56.)),
            Size::Large => number.text_size(px(42.)),
            Size::Medium => number.text_size(px(28.)),
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(label(self.label))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_1()
                    .child(number)
                    .children(
                        (!self.unit.is_empty())
                            .then(|| div().text_sm().text_color(white(0.4)).child(self.unit)),
                    ),
            )
            .children(
                self.sub
                    .map(|s| div().text_xs().text_color(white(0.45)).child(s)),
            )
    }
}

/// A small uppercase caption.
pub fn label(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(white(0.45))
        .child(text.into().to_uppercase())
}

/// `now vs. ref` as "ref 201.3 · +1.2", when both are known.
fn vs_reference(now: Option<f64>, then: Option<f64>) -> Option<String> {
    let (now, then) = now.zip(then)?;
    Some(format!("ref {then:.1} · {:+.1}", now - then))
}

/// The live readouts: roast time, `BT`, `RoR`, `ET`, air and gas.
pub fn live_figures(st: &LiveState) -> [Figure; 6] {
    let last = st.last();
    let value = |k: &str| last.and_then(|s| s.values.get(k).copied());
    let now_s = st.now().map(|at| st.roast_s(at));
    let reference = now_s
        .filter(|_| st.roast.is_some())
        .and_then(|s| st.reference_near(s))
        .unwrap_or((None, None));
    let time = match (st.roast.is_some(), now_s) {
        (true, Some(s)) => clock(s),
        _ => "–:––".to_owned(),
    };
    let ror = last
        .and_then(|s| s.ror)
        .filter(|_| st.event_at(EventKind::TurningPoint).is_some());
    [
        Figure::new(
            if st.roast.is_some() {
                "Roast time"
            } else {
                "Waiting for charge"
            },
            time,
            "",
            white(0.95),
        ),
        Figure::new("Bean temp · BT", fmt1(value("bt")), "°C", chart::bt())
            .sub(vs_reference(value("bt"), reference.0)),
        Figure::new("RoR", fmt1(ror), "°/min", chart::ror()),
        Figure::new("Air temp · ET", fmt1(value("et")), "°C", chart::et())
            .sub(vs_reference(value("et"), reference.1)),
        Figure::new("Air", fmt0(value("fan")), "%", chart::air()),
        Figure::new("Gas", fmt0(value("gas_fb")), "%", chart::gas()),
    ]
}

/// A finished roast at a glance: total time, Development time ratio,
/// development, BT at Drop and weight loss.
pub fn summary_figures(st: &LiveState) -> [Figure; 5] {
    let roast = st.roast.as_ref();
    let drop = st.event_at(EventKind::Drop);
    let total = drop.map(|at| st.roast_s(at));
    let development = drop
        .zip(st.event_at(EventKind::FirstCrack))
        .map(|(d, fc)| st.roast_s(d) - st.roast_s(fc));
    let dtr = roast.and_then(|r| r.metrics.development_time_ratio);
    let loss = roast.and_then(|r| r.metrics.weight_loss_ratio);
    let dash = || "—".to_owned();
    [
        Figure::new(
            "Total time",
            total.map_or_else(dash, clock),
            "",
            white(0.95),
        ),
        Figure::new(
            "Development time ratio",
            dtr.map_or_else(dash, |d| format!("{:.1}", d * 100.0)),
            "%",
            chart::ror(),
        ),
        Figure::new(
            "Development",
            development.map_or_else(dash, clock),
            "",
            white(0.95),
        ),
        Figure::new(
            "Drop BT",
            fmt1(drop.and_then(|at| st.bt_at(at))),
            "°C",
            chart::bt(),
        ),
        Figure::new(
            "Weight loss",
            loss.map_or_else(dash, |l| format!("{:.1}", l * 100.0)),
            "%",
            white(0.95),
        ),
    ]
}

/// The readouts laid out in the rail: time and BT across, the rest in pairs.
fn rail_readouts(figures: [Figure; 6]) -> impl IntoElement {
    let [time, bt, ror, et, air, gas] = figures;
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(time.render(Size::Huge))
        .child(bt.render(Size::Large))
        .child(pair(ror, et))
        .child(pair(air, gas))
}

fn rail_summary(figures: [Figure; 5]) -> impl IntoElement {
    let [total, dtr, development, drop_bt, loss] = figures;
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(total.render(Size::Huge))
        .child(
            div()
                .flex()
                .gap_4()
                .child(div().flex_1().child(dtr.render(Size::Large)))
                .child(div().flex_1().child(development.render(Size::Large))),
        )
        .child(pair(drop_bt, loss))
}

fn pair(a: Figure, b: Figure) -> impl IntoElement {
    div()
        .flex()
        .gap_4()
        .child(div().flex_1().child(a.render(Size::Medium)))
        .child(div().flex_1().child(b.render(Size::Medium)))
}

/// The phases bar with each phase's length and share, and the Development
/// time ratio.
pub(crate) fn phases(st: &LiveState) -> impl IntoElement {
    let Some(roast) = st.roast.as_ref() else {
        return div();
    };
    let phases = &roast.metrics.phases;
    let dtr = roast.metrics.development_time_ratio;
    let color = |kind: &str| match kind {
        "drying" => chart::et(),
        "maillard" => chart::bt(),
        _ => chart::ror(),
    };
    #[allow(clippy::cast_possible_truncation)] // a share in 0..=1
    let bar = div()
        .flex()
        .h(px(8.))
        .w_full()
        .rounded_full()
        .overflow_hidden()
        .bg(white(0.06))
        .children(phases.iter().map(|p| {
            div()
                .h_full()
                .w(gpui_kit::relative(p.ratio as f32))
                .bg(chart::faded(color(p.kind), 0.7))
        }));
    let columns = ["drying", "maillard", "development"].map(|kind| {
        let phase = phases.iter().find(|p| p.kind == kind);
        let (length, share) = phase.map_or((String::new(), String::new()), |p| {
            (
                clock(st.roast_s(p.to) - st.roast_s(p.from)),
                format!("{:.0}%", p.ratio * 100.0),
            )
        });
        let mut name = kind.to_owned();
        name[..1].make_ascii_uppercase();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .text_xs()
            .opacity(if phase.is_some() { 1.0 } else { 0.3 })
            .child(div().text_color(white(0.5)).child(name))
            .child(div().text_sm().child(if length.is_empty() {
                "—".to_owned()
            } else {
                length
            }))
            .child(div().text_color(white(0.4)).child(share))
    });
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .justify_between()
                .items_baseline()
                .child(label("Phases"))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_1()
                        .text_xs()
                        .text_color(white(0.6))
                        .child("DTR")
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::BOLD)
                                .text_color(white(0.95))
                                .child(
                                    dtr.map_or("—".to_owned(), |d| format!("{:.1}%", d * 100.0)),
                                ),
                        ),
                ),
        )
        .child(bar)
        .child(div().flex().gap_2().children(columns))
}

/// A dot on the timeline's rule.
fn dot(fill: Option<Hsla>, ring: Hsla) -> impl IntoElement {
    let d = div()
        .flex_none()
        .size(px(10.))
        .rounded_full()
        .border_1()
        .border_color(ring);
    match fill {
        Some(color) => d.bg(color),
        None => d,
    }
}

/// One entry of the timeline, events and notes merged in roast order.
enum Entry<'a> {
    Event(&'a EventOut),
    /// A note and its number (1-based, in the order they were written).
    Note(usize, &'a NoteOut),
}

impl Entry<'_> {
    fn at(&self) -> i64 {
        match self {
            Entry::Event(e) => e.at,
            Entry::Note(_, n) => n.at,
        }
    }
}

impl RoastScreen {
    /// The rail: depends on whether a roast is on screen, recording or done.
    pub(super) fn rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let st = &self.state;
        let top = div()
            .flex()
            .flex_col()
            .gap_5()
            .p_5()
            .border_b_1()
            .border_color(white(0.08));
        let top = if st.roast.is_none() {
            top.child(self.waiting(cx))
        } else {
            let readouts = (!self.big).then(|| {
                if st.recording() {
                    rail_readouts(live_figures(st)).into_any_element()
                } else {
                    rail_summary(summary_figures(st)).into_any_element()
                }
            });
            top.children(readouts).child(phases(st))
        };
        let done = st.roast.is_some() && !st.recording();
        div()
            .w(px(if self.big { 340. } else { 380. }))
            .h_full()
            .flex()
            .flex_col()
            .bg(gpui_kit::rgb(0x0012_1317))
            .border_l_1()
            .border_color(white(0.08))
            .child(top)
            .children(st.roast.is_some().then(|| self.timeline(cx)))
            .children(done.then(|| self.after_drop(cx)))
            .children(self.message.clone().map(|m| {
                div()
                    .id("message")
                    .px_5()
                    .pb_2()
                    .text_xs()
                    .text_color(gpui_kit::rgb(0x00f8_7171))
                    .child(m)
            }))
    }

    /// The Next roast in one line, and the way to change it: the New roast
    /// page is where it is prepared.
    fn next_summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let next = self.state.next.as_ref();
        let parts: Vec<String> = [
            next.and_then(|n| n.coffee_name.clone())
                .or_else(|| Some("No coffee set".to_owned())),
            next.and_then(|n| n.green_weight_g)
                .map(|g| format!("{} kg", super::kilograms(g))),
            next.map(|n| n.roasted_by.clone()).filter(|b| !b.is_empty()),
            Some(
                next.and_then(|n| n.reference_number)
                    .map_or_else(|| "no reference".to_owned(), |n| format!("reference #{n}")),
            ),
        ]
        .into_iter()
        .flatten()
        .collect();
        div()
            .id("next-summary")
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().child(parts.join(" · ")))
            .child(
                Button::new("prepare-next")
                    .label("Prepare the next roast")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(super::OpenNewRoast))),
            )
    }

    /// Before Charge: the current temperatures and the Charge button.
    fn waiting(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let [_, bt, _, et, _, _] = live_figures(&self.state);
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child("Next roast"),
            )
            .child(self.next_summary(cx))
            .children((!self.big).then(|| pair_large(bt, et)))
            .child(
                Button::new("next-event")
                    .primary()
                    .large()
                    .label("Charge")
                    .on_click(cx.listener(|this, _, _, cx| this.press(EventKind::Charge, cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(white(0.4))
                    .child("Charge is detected automatically; press it only if detection misses."),
            )
    }

    /// Events and notes in roast order, then "now" with the marks while
    /// recording, then the reference's events still ahead, greyed.
    #[allow(clippy::too_many_lines)] // one list, read top to bottom
    fn timeline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let st = &self.state;
        let mut entries: Vec<Entry> = Vec::new();
        if let Some(r) = &st.roast {
            entries.extend(r.events.iter().map(Entry::Event));
            entries.extend(
                r.notes
                    .iter()
                    .enumerate()
                    .map(|(i, n)| Entry::Note(i + 1, n)),
            );
        }
        entries.sort_by_key(Entry::at);
        let time = |at: i64| {
            div()
                .w(px(44.))
                .flex_none()
                .text_xs()
                .text_color(white(0.5))
                .child(clock(st.roast_s(at)))
        };
        let remove = |id: String| {
            Button::new(SharedString::from(id))
                .ghost()
                .xsmall()
                .label("✕")
        };
        let rows = entries.into_iter().map(|entry| {
            let row = div().flex().items_center().gap_3().py_1().text_sm();
            match entry {
                Entry::Event(e) => {
                    let kind = e.kind.parse::<EventKind>().ok();
                    let name = kind.map_or(e.kind, chart::label);
                    row.child(dot(Some(accent()), accent()))
                        .child(time(e.at))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                        .child(
                            div()
                                .text_xs()
                                .text_color(chart::bt())
                                .child(format!("{}°", fmt1(st.bt_at(e.at)))),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(white(0.3))
                                .child(e.source.to_uppercase()),
                        )
                        .child(div().flex_1())
                        .children(kind.filter(|k| k.is_manual_only()).map(|kind| {
                            remove(format!("unmark-{}", kind.as_str()))
                                .on_click(cx.listener(move |this, _, _, cx| this.unmark(kind, cx)))
                        }))
                }
                Entry::Note(n, note) => {
                    let id = note.id;
                    row.child(dot(Some(white(0.9)), white(0.9)))
                        .child(time(note.at))
                        .child(
                            div()
                                .flex_none()
                                .size(px(16.))
                                .rounded_full()
                                .bg(white(0.9))
                                .text_color(gpui_kit::rgb(0x0000_0000))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(n.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_color(white(0.85))
                                .child(note.text.clone()),
                        )
                        .child(
                            remove(format!("remove-note-{n}")).on_click(
                                cx.listener(move |this, _, _, cx| this.remove_note(id, cx)),
                            ),
                        )
                }
            }
        });
        let now = st.recording().then(|| {
            let next = st.next_event().filter(|k| *k != EventKind::Drop);
            let others = [
                EventKind::DryEnd,
                EventKind::FirstCrack,
                EventKind::SecondCrack,
            ]
            .into_iter()
            .filter(|k| Some(*k) != next && st.event_at(*k).is_none())
            .filter(|k| {
                *k != EventKind::SecondCrack || st.event_at(EventKind::FirstCrack).is_some()
            });
            let hint = |kind: EventKind| match kind {
                EventKind::DryEnd => " · color change",
                EventKind::FirstCrack => " · first pops",
                _ => "",
            };
            div()
                .flex()
                .flex_col()
                .gap_2()
                .py_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .text_sm()
                        .text_color(accent())
                        .child(dot(None, accent()))
                        .child(
                            div().w(px(44.)).text_xs().child(
                                st.now()
                                    .map_or_else(String::new, |at| clock(st.roast_s(at))),
                            ),
                        )
                        .child("now"),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .pl(px(66.))
                        .children(next.map(|kind| {
                            Button::new("next-event")
                                .primary()
                                .small()
                                .label(format!("{}{}", chart::label(kind), hint(kind)))
                                .on_click(cx.listener(move |this, _, _, cx| this.press(kind, cx)))
                        }))
                        .children(others.map(|kind| {
                            Button::new(SharedString::from(format!("mark-{}", kind.as_str())))
                                .small()
                                .label(chart::label(kind))
                                .on_click(cx.listener(move |this, _, _, cx| this.press(kind, cx)))
                        }))
                        .child(
                            Button::new("drop")
                                .outline()
                                .small()
                                .label("Drop")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.press(EventKind::Drop, cx);
                                })),
                        ),
                )
        });
        let upcoming = st.upcoming().into_iter().map(|(kind, s)| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .py_1()
                .text_sm()
                .text_color(white(0.3))
                .child(dot(None, white(0.25)))
                .child(div().w(px(44.)).text_xs().child(clock(s)))
                .child(chart::label(kind))
                .child(div().text_xs().child("IN REFERENCE"))
        });
        div()
            .id("timeline")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .px_5()
            .py_4()
            .gap_2()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_baseline()
                    .child(label("Timeline"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(white(0.3))
                            .child("click the chart to mark the past"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_l_1()
                    .border_color(white(0.1))
                    .ml(px(4.))
                    .pl(px(0.))
                    .children(rows)
                    .children(now)
                    .children(upcoming),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .pt_2()
                    .child(Input::new(&self.note).id("note").flex_1())
                    .child(
                        Button::new("add-note")
                            .label("Add note")
                            .on_click(cx.listener(|this, _, window, cx| this.add_note(window, cx))),
                    ),
            )
    }

    /// After Drop: the roasted weight (and so the weight loss), and Charge
    /// for when detection misses the next roast.
    fn after_drop(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let loss = self
            .state
            .roast
            .as_ref()
            .and_then(|r| r.metrics.weight_loss_ratio)
            .map_or_else(
                || "loss —".to_owned(),
                |l| format!("−{:.1} % loss", l * 100.0),
            );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_5()
            .border_t_1()
            .border_color(white(0.08))
            .children(self.state.roast.as_ref().map(|r| {
                let id = r.id;
                Button::new("open-roast")
                    .label(format!("Open Roast #{} in Roasts", r.number))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(crate::roasts::OpenRoast(id));
                    }))
            }))
            .child(label("Roasted weight (kg)"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(Input::new(&self.weight).id("roasted-weight").w(px(120.)))
                    .child(
                        Button::new("save-weight")
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.save_weight(cx))),
                    )
                    .child(div().text_sm().text_color(white(0.6)).child(loss)),
            )
            .child(
                div()
                    .pt_3()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child("Next roast"),
            )
            .child(self.next_summary(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .pt_3()
                    .child(
                        Button::new("next-event")
                            .outline()
                            .label("Charge")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.press(EventKind::Charge, cx);
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(white(0.4))
                            .child("Next Charge is detected automatically."),
                    ),
            )
    }

    /// The "Big numbers" strip across the top: live readouts while waiting
    /// or recording, the summary once done.
    pub(super) fn strip(&self) -> impl IntoElement {
        let st = &self.state;
        let cells: Vec<Figure> = if st.roast.is_some() && !st.recording() {
            summary_figures(st).into()
        } else {
            let [time, bt, ror, et, air, gas] = live_figures(st);
            let air = air.sub(Some(format!("gas {} %", gas.value)));
            vec![time, bt, ror, et, air]
        };
        div()
            .id("big-numbers-strip")
            .test_support()
            .flex()
            .flex_none()
            .bg(gpui_kit::rgb(0x0012_1317))
            .border_b_1()
            .border_color(white(0.08))
            .children(cells.into_iter().map(|f| {
                div()
                    .flex_1()
                    .px_5()
                    .py_3()
                    .border_r_1()
                    .border_color(white(0.06))
                    .child(f.render(Size::Huge))
            }))
    }
}

fn pair_large(a: Figure, b: Figure) -> impl IntoElement {
    div()
        .flex()
        .gap_4()
        .child(div().flex_1().child(a.render(Size::Large)))
        .child(div().flex_1().child(b.render(Size::Large)))
}
