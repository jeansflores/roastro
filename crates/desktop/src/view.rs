//! The live roast screen (variant D of "Live roast screen layout", #9): the
//! chart first, a rail with the big numbers, the phases, the next event and a
//! timeline of events and notes.

use gpui_kit::component::button::Button;
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::theme::Theme;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AsyncApp, Context, Entity, FontWeight, Hsla, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, SharedString, Subscription, WeakEntity, Window,
    anchored, deferred, div, point, px, rgb,
};
use roast::EventKind;
use roastro::engine::{Edit, Reply, Request};
use roastro::wire::{EventIn, Live, NoteIn, RoastPatchIn};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::chart::{self, Geometry};
use crate::state::{LiveState, Millis};

mod next_roast;
mod rail;

/// Paints primary buttons in the screen's amber, with dark text.
pub fn use_accent(cx: &mut gpui_kit::App) {
    let theme = Theme::global_mut(cx);
    let accent = rail::accent();
    theme.primary = accent;
    theme.primary_hover = Hsla {
        l: accent.l + 0.06,
        ..accent
    };
    theme.primary_active = Hsla {
        l: accent.l - 0.06,
        ..accent
    };
    theme.primary_foreground = rgb(0x000b_0c0e).into();
}

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
    menu_subscription: Option<Subscription>,
    /// "Big numbers": the readouts in a strip across the top.
    big: bool,
    /// The roasted weight in kilograms, after Drop.
    weight: Entity<InputState>,
    /// The roast whose stored weight the field shows.
    weight_for: Option<Uuid>,
    /// Keeps the roasted weight's Enter handler alive.
    _weight_subscription: Subscription,
    /// The Next roast form.
    next_form: next_roast::NextRoastForm,
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
        let weight = cx.new(|cx| InputState::new(window, cx).placeholder("0.000"));
        let weight_subscription =
            cx.subscribe_in(&weight, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save_weight(cx);
                }
            });
        let screen = Self {
            state: LiveState::new(time_scale),
            engine,
            note: cx.new(|cx| InputState::new(window, cx)),
            message: None,
            geometry: Rc::new(Cell::new(None)),
            hover: None,
            menu: None,
            menu_note,
            menu_subscription: None,
            big: false,
            weight,
            weight_for: None,
            _weight_subscription: weight_subscription,
            next_form: next_roast::NextRoastForm::new(window, cx),
        };
        cx.spawn(async move |this, cx| follow(this, cx).await)
            .detach();
        screen
    }

    /// Applies a live message and fetches a new reference curve when needed.
    fn apply(&mut self, msg: &Live, cx: &mut Context<Self>) {
        self.state.apply(msg);
        self.next_roast_message(msg, cx);
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
            this.update(cx, |screen, cx| {
                screen.state.set_reference(&roast, &columns);
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
        self.menu_subscription = Some(enter);
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

    /// Removes a mark the operator made (Dry end, First crack, Second crack).
    fn unmark(&mut self, kind: EventKind, cx: &mut Context<Self>) {
        let Some(roast) = self.state.roast.as_ref() else {
            return;
        };
        let id = roastro::RoastId(roast.id);
        self.send(
            |r| Request::Edit(Edit::DeleteEvent(id, kind.as_str().to_owned(), r)),
            cx,
        );
    }

    fn remove_note(&mut self, note: Uuid, cx: &mut Context<Self>) {
        let Some(roast) = self.state.roast.as_ref() else {
            return;
        };
        let id = roastro::RoastId(roast.id);
        self.send(|r| Request::Edit(Edit::DeleteNote(id, note, r)), cx);
    }

    /// Stores the roasted weight typed in kilograms; empty clears it.
    fn save_weight(&mut self, cx: &mut Context<Self>) {
        let Some(roast) = self.state.roast.as_ref() else {
            return;
        };
        let id = roastro::RoastId(roast.id);
        let text = self.weight.read(cx).value().to_string();
        let Some(grams) = grams(&text) else {
            self.message = Some("roasted weight: kilograms, like 4.250".into());
            cx.notify();
            return;
        };
        let patch = RoastPatchIn {
            roasted_weight_g: Some(grams),
            ..RoastPatchIn::default()
        };
        self.send(|r| Request::Edit(Edit::PatchRoast(id, patch, r)), cx);
    }

    /// Shows the stored roasted weight once a roast is done, without
    /// overwriting what the operator is typing for the same roast.
    fn sync_weight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(roast) = self.state.roast.as_ref().filter(|r| r.state == "done") else {
            return;
        };
        if self.weight_for == Some(roast.id) {
            return;
        }
        self.weight_for = Some(roast.id);
        let text = roast.roasted_weight_g.map(kilograms).unwrap_or_default();
        self.weight
            .update(cx, |input, cx| input.set_value(text, window, cx));
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

pub(crate) fn clock(s: f64) -> String {
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

/// Grams from kilograms as typed ("4.25", "4,250"); `Some(None)` when empty.
#[allow(clippy::option_option)] // invalid vs. empty (clear the weight)
fn grams(text: &str) -> Option<Option<i64>> {
    let text = text.trim().replace(',', ".");
    if text.is_empty() {
        return Some(None);
    }
    let kg: f64 = text
        .parse()
        .ok()
        .filter(|kg: &f64| kg.is_finite() && *kg > 0.0)?;
    #[allow(clippy::cast_possible_truncation)] // a roaster's batch, far inside i64
    Some(Some((kg * 1000.0).round() as i64))
}

fn kilograms(grams: i64) -> String {
    #[allow(clippy::cast_precision_loss)] // a roaster's batch, far inside f64
    let kg = grams as f64 / 1000.0;
    format!("{kg:.3}")
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

pub(crate) fn fmt1(v: Option<f64>) -> String {
    v.map_or_else(|| "—".to_owned(), |v| format!("{v:.1}"))
}

fn fmt0(v: Option<f64>) -> String {
    v.map_or_else(|| "—".to_owned(), |v| format!("{v:.0}"))
}

pub(crate) fn white(a: f32) -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a,
    }
}

impl RoastScreen {
    /// The bar across the top: status, coffee, reference, "Big numbers".
    #[allow(clippy::too_many_lines)] // one bar, left to right
    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let st = &self.state;
        let (status, color): (SharedString, Hsla) = if !st.connected() {
            let text = st
                .link
                .as_ref()
                .and_then(|l| l.error.clone())
                .map_or("Connecting to the PLC…".into(), |e| {
                    format!("PLC disconnected: {e}").into()
                });
            (text, rgb(0x00f8_7171).into())
        } else if st.recording() {
            ("Recording".into(), rail::accent())
        } else if st.roast.is_some() {
            ("Done".into(), chart::ror())
        } else {
            ("Waiting for charge".into(), white(0.7))
        };
        let roast = st.roast.as_ref();
        let coffee = roast.and_then(|r| r.coffee.as_ref().map(|c| c.name.clone()));
        let green = roast
            .and_then(|r| r.green_weight_g)
            .or_else(|| st.next.as_ref().and_then(|n| n.green_weight_g));
        let reference = roast.map_or_else(
            || "No reference".to_owned(),
            |r| {
                r.reference.as_ref().map_or("No reference".to_owned(), |r| {
                    format!("Reference Roast #{}", r.number)
                })
            },
        );
        let big = self.big;
        div()
            .flex()
            .flex_none()
            .h(px(52.))
            .items_center()
            .gap_4()
            .px_5()
            .border_b_1()
            .border_color(white(0.08))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rail::accent())
                    .child("ROASTRO"),
            )
            .child(
                div()
                    .id("status")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(chart::faded(color, 0.15))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .children(
                        st.recording()
                            .then(|| div().size(px(8.)).rounded_full().bg(color)),
                    )
                    .child(status.to_uppercase()),
            )
            .children(coffee.map(|c| div().text_sm().text_color(white(0.85)).child(c)))
            .children(green.map(|g| {
                div()
                    .text_sm()
                    .text_color(white(0.5))
                    .child(format!("{} kg", kilograms(g)))
            }))
            .child(div().flex_1())
            .child(div().text_sm().text_color(white(0.5)).child(reference))
            .child(
                div()
                    .id("big-numbers")
                    .test_support()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .border_1()
                    .cursor_pointer()
                    .text_xs()
                    .when(big, |d| {
                        d.border_color(chart::faded(rail::accent(), 0.5))
                            .bg(chart::faded(rail::accent(), 0.15))
                            .text_color(rail::accent())
                    })
                    .when(!big, |d| d.border_color(white(0.1)).text_color(white(0.6)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.big = !this.big;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(24.))
                            .h(px(14.))
                            .rounded_full()
                            .p(px(2.))
                            .flex()
                            .when(big, |d| d.justify_end().bg(rail::accent()))
                            .when(!big, |d| d.bg(white(0.2)))
                            .child(div().size(px(10.)).rounded_full().bg(rgb(0x0000_0000))),
                    )
                    .child("Big numbers"),
            )
    }
}

impl Render for RoastScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_weight(window, cx);
        self.sync_next(window, cx);
        let hover_overlay = if self.menu.is_none() {
            self.hover_overlay()
        } else {
            None
        };
        let menu_overlay = self.menu_overlay(cx);
        let chart = div()
            .id("chart")
            .test_support()
            .flex_1()
            .min_w_0()
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.pointer_down(ev.position, window, cx);
                }),
            )
            .child(chart::chart(
                &self.state,
                self.geometry.clone(),
                self.hover.map(|h| h.1),
            ));
        div()
            .id("roast-screen")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x000b_0c0e))
            .text_color(white(0.92))
            .children(hover_overlay)
            .children(menu_overlay)
            .child(self.header(cx))
            .children(self.big.then(|| self.strip()))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(chart)
                    .child(self.rail(cx)),
            )
    }
}

#[cfg(test)]
mod weight_tests {
    use super::{grams, kilograms};

    #[test]
    fn reads_kilograms_as_typed() {
        assert_eq!(grams("4.25"), Some(Some(4250)));
        assert_eq!(grams(" 4,1 "), Some(Some(4100)));
        assert_eq!(grams(""), Some(None), "empty clears");
        assert_eq!(grams("abc"), None);
        assert_eq!(grams("-1"), None);
        assert_eq!(kilograms(4250), "4.250");
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
    use roastro::wire::{EventOut, Live, MetricsOut, NextRoastOut, NoteOut, RoastDoc, SampleOut};
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
            summary: None,
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
        while let Some(req) = next_request(&mut rx) {
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
        match next_request(&mut rx) {
            Some(Request::Edit(Edit::SetEvent(roast, kind, body, _))) => {
                assert_eq!(
                    (roast.0, kind.as_str(), body.at),
                    (id, "first_crack", 300_000),
                    "clicked ahead of the curve: now"
                );
            }
            Some(other) => panic!("expected First crack, got {}", name(&other)),
            None => panic!("the menu sent nothing"),
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
        match next_request(&mut rx) {
            Some(Request::Edit(Edit::AddNote(_, note, _))) => assert_eq!(note.text, "gas 70"),
            _ => panic!("Enter in the menu did not add the note"),
        }
        assert!(
            cx.update(|cx| screen.read(cx).menu.is_none()),
            "the menu closes after an action"
        );
    }

    fn open(
        cx: &mut TestAppContext,
    ) -> (
        gpui_kit::AnyWindowHandle,
        gpui_kit::Entity<RoastScreen>,
        mpsc::Receiver<Request>,
    ) {
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
        (handle, screen, rx)
    }

    #[gpui_kit::test]
    fn the_rail_undoes_marks_and_notes_and_records_the_weight(cx: &mut TestAppContext) {
        let (handle, screen, mut rx) = open(cx);
        let id = Uuid::now_v7();
        let note = Uuid::now_v7();
        let mut roast = recording_roast(id);
        roast.events.push(EventOut {
            kind: "dry_end",
            at: 2000,
            source: "manual",
            original_at: None,
        });
        roast.notes.push(NoteOut {
            id: note,
            at: 2000,
            text: "yellow".into(),
        });
        cx.update(|cx| {
            screen.update(cx, |s, cx| {
                s.apply(&Live::Roast(roast.clone()), cx);
                let values = HashMap::from([("bt".to_owned(), 150.0)]);
                s.apply(
                    &Live::Sample(SampleOut {
                        at: 3000,
                        values,
                        ror: None,
                    }),
                    cx,
                );
            });
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("unmark-charge").is_none(),
                "Charge can only be moved"
            );
            window.click("unmark-dry_end", cx);
            window.click("remove-note-1", cx);
            window.click("drop", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let mut seen = Vec::new();
        while let Some(req) = next_request(&mut rx) {
            seen.push(match req {
                Request::Edit(Edit::DeleteEvent(_, kind, _)) => format!("unmark {kind}"),
                Request::Edit(Edit::DeleteNote(_, n, _)) => format!("remove note {}", n == note),
                Request::Drop(_) => "drop".into(),
                other => name(&other).into(),
            });
        }
        assert_eq!(seen, ["unmark dry_end", "remove note true", "drop"]);

        roast.state = "done";
        roast.roasted_weight_g = Some(4250);
        cx.update(|cx| screen.update(cx, |s, cx| s.apply(&Live::Roast(roast), cx)));
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        assert_eq!(
            cx.update(|cx| screen.read(cx).weight.read(cx).value().to_string()),
            "4.250",
            "the stored weight shows once done"
        );

        let other = recording_roast(Uuid::now_v7());
        let other = RoastDoc {
            state: "done",
            ..other
        };
        cx.update(|cx| screen.update(cx, |s, cx| s.apply(&Live::Roast(other), cx)));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("roasted-weight", cx);
            window.input("4,1", cx);
            window.click("save-weight", cx);
        })
        .unwrap();
        cx.run_until_parked();
        match next_request(&mut rx) {
            Some(Request::Edit(Edit::PatchRoast(_, patch, _))) => {
                assert_eq!(patch.roasted_weight_g, Some(Some(4100)));
            }
            _ => panic!("Save did not patch the roasted weight"),
        }
    }

    #[gpui_kit::test]
    fn big_numbers_moves_the_readouts_to_a_strip(cx: &mut TestAppContext) {
        let (handle, screen, _rx) = open(cx);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("big-numbers-strip").is_none());
            window.click("big-numbers", cx);
            window.render_frame(cx);
            assert!(window.try_find("big-numbers-strip").is_some());
        })
        .unwrap();
        assert!(cx.update(|cx| screen.read(cx).big));
    }

    #[gpui_kit::test]
    fn the_next_roast_is_stored_as_it_is_filled(cx: &mut TestAppContext) {
        let (handle, screen, mut rx) = open(cx);
        let coffee = Uuid::now_v7();
        let suggested = Uuid::now_v7();
        cx.update(|cx| screen.update(cx, |s, cx| s.choose_coffee(Some(coffee), cx)));
        cx.run_until_parked();
        match next_request(&mut rx) {
            Some(Request::Edit(Edit::PutNextRoast(next, _))) => {
                assert_eq!((next.coffee_id, next.reference_id), (Some(coffee), None));
            }
            _ => panic!("choosing a coffee did not store it"),
        }
        let stored = NextRoastOut {
            coffee_id: Some(coffee),
            green_weight_g: None,
            roasted_by: String::new(),
            reference_id: None,
            suggested_reference_id: Some(suggested),
        };
        cx.update(|cx| screen.update(cx, |s, cx| s.apply(&Live::NextRoast(stored.clone()), cx)));
        cx.run_until_parked();
        match next_request(&mut rx) {
            Some(Request::Edit(Edit::PutNextRoast(next, _))) => {
                assert_eq!(
                    next.reference_id,
                    Some(suggested),
                    "the coffee's latest roast becomes the reference"
                );
            }
            _ => panic!("the suggested reference was not taken"),
        }
        let stored = NextRoastOut {
            reference_id: Some(suggested),
            ..stored
        };
        cx.update(|cx| screen.update(cx, |s, cx| s.apply(&Live::NextRoast(stored), cx)));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("green-weight", cx);
            window.input("5", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        match next_request(&mut rx) {
            Some(Request::Edit(Edit::PutNextRoast(next, _))) => {
                assert_eq!(
                    (next.coffee_id, next.green_weight_g, next.reference_id),
                    (Some(coffee), Some(5000), Some(suggested)),
                    "one change, the rest kept"
                );
            }
            _ => panic!("the green weight was not stored"),
        }
    }

    /// The next request, skipping the lists the Next roast form loads.
    fn next_request(rx: &mut mpsc::Receiver<Request>) -> Option<Request> {
        loop {
            match rx.try_recv().ok()? {
                Request::Edit(Edit::Coffees(..) | Edit::Roasts(..)) => {}
                req => return Some(req),
            }
        }
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
