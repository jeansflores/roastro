//! The live roast screen (variant D of "Live roast screen layout", #9): the
//! chart first, a rail with the big numbers, the phases, the next event and a
//! timeline of events and notes.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AsyncApp, Context, Entity, FontWeight, Hsla, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, SharedString, Subscription, WeakEntity, Window,
    anchored, deferred, div, point, px, relative, rgb,
};
use roast::EventKind;
use roastro::engine::{Edit, Reply, Request};
use roastro::wire::{EventIn, Live, NoteIn};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::chart::{self, Geometry, faded};
use crate::state::{LiveState, Millis};

/// The engine's request queue, as the UI uses it.
#[derive(Clone)]
pub struct Engine(pub mpsc::Sender<Request>);

impl Engine {
    /// Queues a request; the reply arrives on the returned channel.
    fn ask<T>(
        &self,
        make: impl FnOnce(Reply<T>) -> Request,
    ) -> Option<oneshot::Receiver<Result<T, roastro::api::ApiError>>> {
        let (tx, rx) = oneshot::channel();
        self.0.try_send(make(tx)).ok().map(|()| rx)
    }
}

/// The screen's state and its links to the engine.
pub struct RoastScreen {
    pub state: LiveState,
    engine: Engine,
    note: Entity<InputState>,
    /// The last refusal or failure, shown under the buttons.
    message: Option<String>,
    /// Where the chart was last drawn, to read the pointer.
    geometry: Rc<Cell<Option<Geometry>>>,
    /// The pointer over the chart: window position and roast second.
    hover: Option<(Point<Pixels>, f64)>,
    /// The chart's click menu, when open.
    menu: Option<Menu>,
    menu_note: Entity<InputState>,
    /// Keeps the menu note's Enter handler alive.
    subscriptions: Vec<Subscription>,
}

/// The chart's click menu: an instant to mark an event or add a note on.
#[derive(Debug, Clone, Copy)]
struct Menu {
    position: Point<Pixels>,
    at: Millis,
    /// Clicked ahead of the latest reading: the menu acts on "now".
    ahead: bool,
}

impl RoastScreen {
    pub fn new(
        engine: Engine,
        time_scale: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let menu_note = cx.new(|cx| InputState::new(window, cx));
        let enter = cx.subscribe_in(
            &menu_note,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.menu_add_note(window, cx);
                }
            },
        );
        let screen = Self {
            state: LiveState::new(time_scale),
            engine,
            note: cx.new(|cx| InputState::new(window, cx)),
            message: None,
            geometry: Rc::new(Cell::new(None)),
            hover: None,
            menu: None,
            menu_note,
            subscriptions: vec![enter],
        };
        cx.spawn(async move |this, cx| follow(this, cx).await)
            .detach();
        screen
    }

    /// Applies a live message and fetches a new reference curve when needed.
    fn apply(&mut self, msg: &Live, cx: &mut Context<Self>) {
        self.state.apply(msg);
        let wanted = self.state.wanted_reference();
        if wanted != self.state.reference.as_ref().map(|r| r.id) {
            self.state.reference = None;
            if let Some(id) = wanted {
                self.fetch_reference(id, cx);
            }
        }
        cx.notify();
    }

    fn fetch_reference(&self, id: Uuid, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let roast = engine
                .ask(|r| Request::Roast(roastro::RoastId(id), r))?
                .await
                .ok()?
                .ok()?;
            let columns = engine
                .ask(|r| Request::Samples(roastro::RoastId(id), r))?
                .await
                .ok()?
                .ok()?;
            let charge_at = roast.charge_at?;
            this.update(cx, |screen, cx| {
                screen.state.set_reference(id, charge_at, &columns);
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    /// Sends a request and shows its error, if any; the result itself
    /// arrives on the live stream.
    fn send<T: 'static>(&mut self, make: impl FnOnce(Reply<T>) -> Request, cx: &mut Context<Self>) {
        self.message = None;
        let Some(reply) = self.engine.ask(make) else {
            self.message = Some("Roastro is busy; try again".into());
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            if let Ok(Err(err)) = reply.await {
                let _ = this.update(cx, |screen, cx| {
                    screen.message = Some(err.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn press(&mut self, kind: EventKind, cx: &mut Context<Self>) {
        match kind {
            EventKind::Charge => self.send(Request::Charge, cx),
            EventKind::Drop if self.state.recording() => self.send(Request::Drop, cx),
            _ => {
                let (Some(roast), Some(at)) = (self.state.roast.as_ref(), self.state.now()) else {
                    return;
                };
                let id = roastro::RoastId(roast.id);
                self.send(
                    |r| {
                        Request::Edit(Edit::SetEvent(
                            id,
                            kind.as_str().to_owned(),
                            EventIn { at },
                            r,
                        ))
                    },
                    cx,
                );
            }
        }
    }

    fn pointer_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let hover = self
            .geometry
            .get()
            .and_then(|g| g.roast_s(position))
            .map(|s| (position, s));
        if hover.map(|h| h.1) != self.hover.map(|h| h.1) {
            self.hover = hover;
            cx.notify();
        }
    }

    /// Clicking the chart opens the menu on that instant (or on "now", when
    /// clicked ahead of the latest reading). Only during or after a roast.
    fn pointer_down(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(s) = self.geometry.get().and_then(|g| g.roast_s(position)) else {
            return;
        };
        if self.state.roast.is_none() {
            return;
        }
        let Some(at) = self.state.wall_at(s) else {
            return;
        };
        let ahead = self.state.now().is_some_and(|now| at >= now);
        self.menu = Some(Menu {
            position,
            at,
            ahead,
        });
        self.hover = None;
        self.menu_note = cx.new(|cx| InputState::new(window, cx));
        let enter = cx.subscribe_in(
            &self.menu_note,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.menu_add_note(window, cx);
                }
            },
        );
        self.subscriptions = vec![enter];
        cx.notify();
    }

    fn mark_at(&mut self, kind: EventKind, at: Millis, cx: &mut Context<Self>) {
        let Some(roast) = self.state.roast.as_ref() else {
            return;
        };
        let id = roastro::RoastId(roast.id);
        self.menu = None;
        self.send(
            |r| {
                Request::Edit(Edit::SetEvent(
                    id,
                    kind.as_str().to_owned(),
                    EventIn { at },
                    r,
                ))
            },
            cx,
        );
    }

    fn menu_add_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(menu), Some(roast)) = (self.menu, self.state.roast.as_ref()) else {
            return;
        };
        let text = self.menu_note.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let id = roastro::RoastId(roast.id);
        self.menu = None;
        self.send(
            |r| Request::Edit(Edit::AddNote(id, NoteIn { at: menu.at, text }, r)),
            cx,
        );
        self.menu_note = cx.new(|cx| InputState::new(window, cx));
    }

    /// The card that follows the pointer over the chart.
    fn hover_overlay(&self) -> Option<AnyElement> {
        let (position, s) = self.hover?;
        let card = hover_card(&self.state, s)?;
        Some(
            deferred(
                anchored()
                    .position(position + point(px(16.), px(16.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .into_any_element(),
        )
    }

    fn menu_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu?;
        let st = &self.state;
        let s = st.roast_s(menu.at);
        let bt_then = st.sample_near(s).and_then(|x| x.values.get("bt").copied());
        let title = if menu.ahead {
            format!("now · {}", clock_signed(s))
        } else {
            clock_signed(s)
        };
        let buttons = [
            EventKind::DryEnd,
            EventKind::FirstCrack,
            EventKind::SecondCrack,
        ]
        .into_iter()
        .map(|kind| {
            let hint = match kind {
                EventKind::DryEnd => "  color change",
                EventKind::FirstCrack => "  first pops",
                _ => "",
            };
            let was = st
                .event_at(kind)
                .map(|at| format!("   was {}", clock_signed(st.roast_s(at))))
                .unwrap_or_default();
            Button::new(SharedString::from(format!("menu-{}", kind.as_str())))
                .label(format!("{}{hint}{was}", chart::label(kind)))
                .w_full()
                .on_click(cx.listener(move |this, _, _, cx| this.mark_at(kind, menu.at, cx)))
        });
        let panel = div()
            .id("chart-menu")
            .test_support()
            .occlude()
            .w(px(320.))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_lg()
            .bg(rgb(0x0018_1a1f))
            .border_1()
            .border_color(white(0.12))
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().font_weight(FontWeight::BOLD).child(title))
                    .child(
                        div()
                            .text_sm()
                            .text_color(chart::bt())
                            .child(format!("BT {}°", fmt1(bt_then))),
                    ),
            )
            .children(buttons)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Input::new(&self.menu_note).id("menu-note").flex_1())
                    .child(Button::new("menu-add-note").label("Add note").on_click(
                        cx.listener(|this, _, window, cx| this.menu_add_note(window, cx)),
                    )),
            );
        Some(
            deferred(
                anchored()
                    .position(menu.position + point(px(12.), px(12.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(panel),
            )
            .into_any_element(),
        )
    }

    fn add_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.note.read(cx).value().to_string();
        let (Some(roast), Some(at)) = (self.state.roast.as_ref(), self.state.now()) else {
            self.message = Some("notes belong to a roast: Charge first".into());
            cx.notify();
            return;
        };
        if text.trim().is_empty() {
            return;
        }
        let id = roastro::RoastId(roast.id);
        self.send(
            |r| Request::Edit(Edit::AddNote(id, NoteIn { at, text }, r)),
            cx,
        );
        self.note = cx.new(|cx| InputState::new(window, cx));
    }
}

/// Follows the engine's live stream for as long as the screen exists: a
/// snapshot, then every message; a fresh snapshot after falling behind.
async fn follow(this: WeakEntity<RoastScreen>, cx: &mut AsyncApp) -> Option<()> {
    let engine = this.update(cx, |s, _| s.engine.clone()).ok()?;
    loop {
        let (snapshot, mut rx) = engine.ask(Request::Subscribe)?.await.ok()?.ok()?;
        this.update(cx, |s, cx| s.apply(&snapshot, cx)).ok()?;
        loop {
            match rx.recv().await {
                Ok(msg) => this.update(cx, |s, cx| s.apply(&msg, cx)).ok()?,
                Err(RecvError::Lagged(_)) => break,
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

fn clock(s: f64) -> String {
    #[allow(clippy::cast_possible_truncation)]
    let s = s.max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// `m:ss`, with a minus sign before Charge.
fn clock_signed(s: f64) -> String {
    if s < 0.0 {
        format!("−{}", clock(-s))
    } else {
        clock(s)
    }
}

/// What the hover card shows at one roast second: live values against the
/// reference, gas and air, and any event or note right there.
#[allow(clippy::too_many_lines)] // one card, built top to bottom
fn hover_card(st: &LiveState, s: f64) -> Option<impl IntoElement> {
    let live = st.sample_near(s);
    let reference = st.reference_near(s);
    if live.is_none() && reference.is_none() {
        return None;
    }
    let value = |k: &str| live.and_then(|x| x.values.get(k).copied());
    let reference = reference.unwrap_or((None, None)); // (BT, ET)
    let ror = live
        .and_then(|x| x.ror)
        .filter(|_| st.event_at(EventKind::TurningPoint).is_some());
    let title = if st.roast.is_none() {
        if s.abs() < 0.5 {
            "now".to_owned()
        } else {
            format!("{} ago", clock(-s))
        }
    } else {
        clock_signed(s)
    };
    let cell = |text: String, color: Hsla| {
        div()
            .w(px(56.))
            .flex()
            .justify_end()
            .text_color(color)
            .child(text)
    };
    let row = |name: &'static str, now: Option<f64>, then: Option<f64>, color: Hsla| {
        div()
            .flex()
            .text_sm()
            .child(div().w(px(36.)).text_color(white(0.5)).child(name))
            .child(cell(fmt1(now), color))
            .child(cell(
                then.map_or_else(String::new, |v| format!("{v:.1}")),
                white(0.45),
            ))
            .child(cell(
                now.zip(then)
                    .map_or_else(String::new, |(a, b)| format!("{:+.1}", a - b)),
                white(0.7),
            ))
    };
    let tolerance = |window_s: f64| move |at: Millis| (st.roast_s(at) - s).abs() <= window_s;
    let near_event = tolerance(2.0);
    let near_note = tolerance(3.0);
    let events: Vec<String> = st
        .roast
        .iter()
        .flat_map(|r| r.events.iter())
        .filter(|e| near_event(e.at))
        .map(|e| {
            format!(
                "{} · {}",
                e.kind.parse::<EventKind>().map_or(e.kind, chart::label),
                clock_signed(st.roast_s(e.at))
            )
        })
        .collect();
    let notes: Vec<String> = st
        .roast
        .iter()
        .flat_map(|r| r.notes.iter())
        .filter(|n| near_note(n.at))
        .map(|n| format!("“{}”", n.text))
        .collect();
    let hint =
        (live.is_some() && st.roast.is_some()).then_some("Click to mark an event or add a note");
    Some(
        div()
            .w(px(240.))
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .rounded_lg()
            .bg(rgb(0x0018_1a1f))
            .border_1()
            .border_color(white(0.12))
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().font_weight(FontWeight::BOLD).child(title))
                    .children(live.is_none().then(|| {
                        div()
                            .text_xs()
                            .text_color(white(0.45))
                            .child("reference only")
                    })),
            )
            .child(
                div()
                    .flex()
                    .text_xs()
                    .text_color(white(0.35))
                    .child(div().w(px(36.)))
                    .child(cell("now".into(), white(0.35)))
                    .child(cell("ref".into(), white(0.35)))
                    .child(cell("diff".into(), white(0.35))),
            )
            .child(row("BT", value("bt"), reference.0, chart::bt()))
            .child(row("ET", value("et"), reference.1, chart::et()))
            .child(row("RoR", ror, None, chart::ror()))
            .children(live.map(|_| {
                div()
                    .flex()
                    .gap_4()
                    .text_sm()
                    .child(
                        div()
                            .text_color(chart::gas())
                            .child(format!("gas {}%", fmt0(value("gas_fb")))),
                    )
                    .child(
                        div()
                            .text_color(chart::air())
                            .child(format!("air {}%", fmt0(value("fan")))),
                    )
            }))
            .children(
                events
                    .into_iter()
                    .map(|e| div().text_sm().text_color(chart::bt()).child(e)),
            )
            .children(notes.into_iter().map(|n| div().text_sm().child(n)))
            .children(hint.map(|h| div().text_xs().text_color(white(0.35)).child(h))),
    )
}

fn fmt1(v: Option<f64>) -> String {
    v.map_or_else(|| "—".to_owned(), |v| format!("{v:.1}"))
}

fn fmt0(v: Option<f64>) -> String {
    v.map_or_else(|| "—".to_owned(), |v| format!("{v:.0}"))
}

fn white(a: f32) -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a,
    }
}

fn readout(
    label: &'static str,
    value: String,
    color: Hsla,
    big: bool,
    sub: Option<String>,
) -> impl IntoElement {
    let number = div()
        .font_weight(FontWeight::BOLD)
        .text_color(color)
        .child(value);
    let number = if big {
        number.text_size(px(44.))
    } else {
        number.text_2xl()
    };
    div()
        .flex()
        .flex_col()
        .child(div().text_xs().text_color(white(0.45)).child(label))
        .child(number)
        .children(sub.map(|s| div().text_xs().text_color(white(0.5)).child(s)))
}

impl Render for RoastScreen {
    #[allow(clippy::too_many_lines)] // the screen's layout, read top to bottom
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hover_overlay = if self.menu.is_none() {
            self.hover_overlay()
        } else {
            None
        };
        let menu_overlay = self.menu_overlay(cx);
        let st = &self.state;
        let last = st.last();
        let value = |k: &str| last.and_then(|s| s.values.get(k).copied());
        let ref_bt = st.reference.as_ref().and_then(|r| {
            let now = st.roast_s(st.now()?);
            r.points
                .iter()
                .min_by(|a, b| (a.0 - now).abs().total_cmp(&(b.0 - now).abs()))
                .and_then(|p| p.1)
        });
        let bt_sub = match (value("bt"), ref_bt, st.roast.is_some()) {
            (Some(b), Some(r), true) => Some(format!("ref {r:.1}  {:+.1}", b - r)),
            _ => None,
        };
        let status: SharedString = if !st.connected() {
            st.link
                .as_ref()
                .and_then(|l| l.error.clone())
                .map_or("Connecting to the PLC…".into(), |e| {
                    format!("PLC disconnected: {e}").into()
                })
        } else if st.recording() {
            "Recording".into()
        } else if st.roast.is_some() {
            "Done".into()
        } else {
            "Waiting for charge".into()
        };
        let time = if st.roast.is_some() {
            let end = st.event_at(EventKind::Drop).or(st.now());
            end.map_or_else(|| "–:––".into(), |at| clock(st.roast_s(at)))
        } else {
            "–:––".into()
        };
        let coffee = st
            .roast
            .as_ref()
            .and_then(|r| r.coffee.as_ref().map(|c| c.name.clone()));
        let reference = st.roast.as_ref().and_then(|r| {
            r.reference
                .as_ref()
                .map(|r| format!("Reference: Roast #{}", r.number))
        });
        let next = st.next_event();
        let recording = st.recording();

        // Phases bar and Development time ratio.
        let phases = st
            .roast
            .as_ref()
            .map(|r| r.metrics.phases.clone())
            .unwrap_or_default();
        let dtr = st
            .roast
            .as_ref()
            .and_then(|r| r.metrics.development_time_ratio);
        let phase_bar = div()
            .flex()
            .h(px(8.))
            .w_full()
            .rounded_full()
            .overflow_hidden()
            .bg(white(0.06))
            .children(phases.iter().map(|p| {
                let color = match p.kind {
                    "drying" => chart::et(),
                    "maillard" => chart::bt(),
                    _ => chart::ror(),
                };
                #[allow(clippy::cast_possible_truncation)]
                div()
                    .h_full()
                    .w(relative(p.ratio as f32))
                    .bg(faded(color, 0.7))
            }));

        // Timeline: events and notes in roast order.
        let mut timeline: Vec<(Millis, String, Hsla)> = Vec::new();
        if let Some(r) = &st.roast {
            for e in &r.events {
                let kind = e.kind.parse::<EventKind>().ok();
                let name = kind.map_or(e.kind, chart::label);
                let bt_then = st
                    .samples
                    .iter()
                    .min_by_key(|s| (s.at - e.at).abs())
                    .and_then(|s| s.values.get("bt").copied());
                timeline.push((
                    e.at,
                    format!("{}  {name}  {}°", clock(st.roast_s(e.at)), fmt1(bt_then)),
                    chart::bt(),
                ));
            }
            for n in &r.notes {
                timeline.push((
                    n.at,
                    format!("{}  “{}”", clock(st.roast_s(n.at)), n.text),
                    white(0.85),
                ));
            }
        }
        timeline.sort_by_key(|t| t.0);

        let next_label = next.map(|k| match k {
            EventKind::Charge => "Charge".to_owned(),
            EventKind::Drop => "Drop".to_owned(),
            k => format!("Mark {}", chart::label(k)),
        });

        div()
            .id("roast-screen")
            .size_full()
            .flex()
            .bg(rgb(0x000b_0c0e))
            .text_color(white(0.92))
            .children(hover_overlay)
            .children(menu_overlay)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .p_4()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(chart::bt())
                                    .child("ROASTRO"),
                            )
                            .child(
                                div()
                                    .id("status")
                                    .text_sm()
                                    .text_color(white(0.7))
                                    .child(status),
                            )
                            .children(coffee.map(|c| div().text_sm().child(c)))
                            .child(div().flex_1())
                            .children(
                                reference.map(|r| div().text_sm().text_color(white(0.5)).child(r)),
                            ),
                    )
                    .child(
                        div()
                            .id("chart")
                            .test_support()
                            .flex_1()
                            .w_full()
                            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                                this.pointer_moved(ev.position, cx);
                            }))
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if !hovered {
                                    this.hover = None;
                                    cx.notify();
                                }
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                                    this.pointer_down(ev.position, window, cx);
                                }),
                            )
                            .child(chart::chart(
                                st,
                                self.geometry.clone(),
                                self.hover.map(|h| h.1),
                            )),
                    ),
            )
            .child(
                div()
                    .w(px(360.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_4()
                    .bg(rgb(0x0012_1317))
                    .border_l_1()
                    .border_color(white(0.08))
                    .child(readout("ROAST TIME", time, white(0.95), true, None))
                    .child(readout(
                        "BEAN TEMP · BT °C",
                        fmt1(value("bt")),
                        chart::bt(),
                        true,
                        bt_sub,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_6()
                            .child(readout(
                                "RoR °C/min",
                                fmt1(
                                    last.and_then(|s| s.ror)
                                        .filter(|_| st.event_at(EventKind::TurningPoint).is_some()),
                                ),
                                chart::ror(),
                                false,
                                None,
                            ))
                            .child(readout(
                                "AIR TEMP · ET °C",
                                fmt1(value("et")),
                                chart::et(),
                                false,
                                None,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_6()
                            .child(readout(
                                "AIR %",
                                fmt0(value("fan")),
                                chart::air(),
                                false,
                                None,
                            ))
                            .child(readout(
                                "GAS %",
                                fmt0(value("gas_fb")),
                                chart::gas(),
                                false,
                                None,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_xs()
                                    .text_color(white(0.45))
                                    .child("PHASES")
                                    .child(format!(
                                        "DTR {}",
                                        dtr.map_or("—".to_owned(), |d| format!(
                                            "{:.1}%",
                                            d * 100.0
                                        ))
                                    )),
                            )
                            .child(phase_bar),
                    )
                    .children(next.zip(next_label).map(|(kind, label)| {
                        Button::new("next-event")
                            .primary()
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| this.press(kind, cx)))
                    }))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .children(
                                (recording
                                    && st.event_at(EventKind::FirstCrack).is_some()
                                    && st.event_at(EventKind::SecondCrack).is_none())
                                .then(|| {
                                    Button::new("second-crack").label("Second crack").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.press(EventKind::SecondCrack, cx);
                                        }),
                                    )
                                }),
                            )
                            .children((recording && next != Some(EventKind::Drop)).then(|| {
                                Button::new("drop").label("Drop").on_click(
                                    cx.listener(|this, _, _, cx| this.press(EventKind::Drop, cx)),
                                )
                            })),
                    )
                    .children(self.message.clone().map(|m| {
                        div()
                            .id("message")
                            .text_xs()
                            .text_color(rgb(0x00f8_7171))
                            .child(m)
                    }))
                    .child(div().text_xs().text_color(white(0.45)).child("TIMELINE"))
                    .child(
                        div()
                            .id("timeline")
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .overflow_y_scroll()
                            .children(timeline.into_iter().map(|(_, text, color)| {
                                div().text_sm().text_color(color).child(text)
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(Input::new(&self.note).id("note").flex_1())
                            .child(Button::new("add-note").label("Add note").on_click(
                                cx.listener(|this, _, window, cx| this.add_note(window, cx)),
                            )),
                    ),
            )
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    // No `use super::*`: it would bring gpui_kit's `test` into scope and make
    // the attribute's own `#[test]` expand to itself.
    use std::collections::HashMap;

    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size,
    };
    use roastro::engine::{Edit, Request};
    use roastro::wire::{EventOut, Live, MetricsOut, RoastDoc, SampleOut};
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use super::{Engine, RoastScreen};

    fn recording_roast(id: Uuid) -> RoastDoc {
        RoastDoc {
            id,
            number: 1,
            state: "recording",
            charge_at: Some(0),
            roasted_by: String::new(),
            green_weight_g: None,
            roasted_weight_g: None,
            coffee: None,
            reference: None,
            events: vec![EventOut {
                kind: "charge",
                at: 0,
                source: "auto",
                original_at: None,
            }],
            notes: Vec::new(),
            metrics: MetricsOut {
                duration_ms: None,
                phases: Vec::new(),
                development_time_ratio: None,
                weight_loss_ratio: None,
            },
        }
    }

    #[gpui_kit::test]
    fn the_next_event_button_and_notes_reach_the_engine(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (tx, mut rx) = mpsc::channel(16);
        let (handle, screen) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1440.), px(900.)),
                })),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| RoastScreen::new(Engine(tx), 1.0, window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        assert!(
            matches!(rx.try_recv(), Ok(Request::Subscribe(_))),
            "the screen follows the live stream"
        );

        let id = Uuid::now_v7();
        cx.update(|cx| {
            screen.update(cx, |s, cx| {
                s.apply(&Live::Roast(recording_roast(id)), cx);
                for at in [1000, 2000] {
                    let values = HashMap::from([("bt".to_owned(), 150.0)]);
                    s.apply(
                        &Live::Sample(SampleOut {
                            at,
                            values,
                            ror: None,
                        }),
                        cx,
                    );
                }
            });
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("next-event", cx);
            window.click("note", cx);
            window.input("smell of bread", cx);
            window.click("add-note", cx);
        })
        .unwrap();
        cx.run_until_parked();

        let mut seen = Vec::new();
        while let Ok(req) = rx.try_recv() {
            match req {
                Request::Edit(Edit::SetEvent(roast, kind, body, _)) => {
                    seen.push(format!("event {kind} at {} on {}", body.at, roast.0 == id));
                }
                Request::Edit(Edit::AddNote(_, note, _)) => {
                    seen.push(format!("note {:?} at {}", note.text, note.at));
                }
                _ => seen.push("other".into()),
            }
        }
        assert_eq!(
            seen,
            [
                "event dry_end at 2000 on true",
                "note \"smell of bread\" at 2000"
            ]
        );
    }

    #[gpui_kit::test]
    fn hovering_and_clicking_the_chart(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (tx, mut rx) = mpsc::channel(16);
        let (handle, screen) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1440.), px(900.)),
                })),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| RoastScreen::new(Engine(tx), 1.0, window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let _subscribe = rx.try_recv();
        let id = Uuid::now_v7();
        cx.update(|cx| {
            screen.update(cx, |s, cx| {
                s.apply(&Live::Roast(recording_roast(id)), cx);
                for at in (0..=300).map(|t| t * 1000) {
                    let values =
                        HashMap::from([("bt".to_owned(), 100.0), ("et".to_owned(), 180.0)]);
                    s.apply(
                        &Live::Sample(SampleOut {
                            at,
                            values,
                            ror: None,
                        }),
                        cx,
                    );
                }
            });
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.hover("chart", cx);
        })
        .unwrap();
        assert!(
            cx.update(|cx| screen.read(cx).hover.is_some()),
            "the pointer is tracked over the chart"
        );

        cx.update_window(handle, |_, window, cx| {
            window.click("chart", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("chart-menu").is_some(),
                "a click opens the menu"
            );
            window.click("menu-first_crack", cx);
        })
        .unwrap();
        cx.run_until_parked();
        match rx.try_recv() {
            Ok(Request::Edit(Edit::SetEvent(roast, kind, body, _))) => {
                assert_eq!(
                    (roast.0, kind.as_str(), body.at),
                    (id, "first_crack", 300_000),
                    "clicked ahead of the curve: now"
                );
            }
            Ok(other) => panic!("expected First crack, got {}", name(&other)),
            Err(e) => panic!("the menu sent nothing: {e:?}"),
        }

        cx.update_window(handle, |_, window, cx| {
            window.click("chart", cx);
            window.render_frame(cx);
            window.click("menu-note", cx);
            window.input("gas 70", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        match rx.try_recv() {
            Ok(Request::Edit(Edit::AddNote(_, note, _))) => assert_eq!(note.text, "gas 70"),
            _ => panic!("Enter in the menu did not add the note"),
        }
        assert!(
            cx.update(|cx| screen.read(cx).menu.is_none()),
            "the menu closes after an action"
        );
    }

    fn name(req: &Request) -> &'static str {
        match req {
            Request::Poll { .. } => "poll",
            Request::Subscribe(_) => "subscribe",
            Request::Charge(_) => "charge",
            Request::Drop(_) => "drop",
            Request::Roast(..) => "roast",
            Request::Samples(..) => "samples",
            Request::Edit(Edit::SetEvent(..)) => "set event",
            Request::Edit(Edit::AddNote(..)) => "add note",
            Request::Edit(_) => "other edit",
        }
    }
}
