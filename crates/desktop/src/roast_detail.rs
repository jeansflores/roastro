//! Roast: understand one roast ("Roast history and curve comparison", #10).
//! Its curve as on the live screen, with the reference behind; its figures,
//! phases, events and notes; the roasted weight. Clicking the chart moves an
//! event to that instant or notes it. Nothing here compares several roasts.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, SharedString, Subscription, Window, anchored,
    deferred, div, point, px, rgb,
};
use roast::EventKind;
use roastro::RoastId;
use roastro::engine::{Edit, Reply, Request};
use roastro::wire::{
    Columns, EventIn, NextRoastIn, NoteIn, RoastDoc, RoastPatchIn, RoastQueryIn, SampleOut,
};
use uuid::Uuid;

use crate::chart::{self, Geometry};
use crate::compare::CompareRoasts;
use crate::roasts::day;
use crate::state::{LiveState, Millis};
use crate::view::rail::{Size, label, phases, summary_figures};
use crate::view::{Engine, clock, clock_signed, fmt1, grams, hover_card, kilograms, white};

/// The shell goes back to the list.
pub struct Back;

/// Events the roaster may move on a finished roast by clicking the chart.
const MOVABLE: [EventKind; 4] = [
    EventKind::DryEnd,
    EventKind::FirstCrack,
    EventKind::SecondCrack,
    EventKind::Drop,
];

/// The Roast page.
pub struct RoastDetail {
    engine: Engine,
    id: Uuid,
    /// The roast as the chart and the figures read it.
    state: LiveState,
    message: Option<String>,
    geometry: Rc<Cell<Option<Geometry>>>,
    hover: Option<(Point<Pixels>, f64)>,
    /// The chart's click menu: where, and the instant clicked.
    menu: Option<(Point<Pixels>, Millis)>,
    menu_note: Entity<InputState>,
    weight: Entity<InputState>,
    /// The stored weight is in the field.
    weight_shown: bool,
    /// The readings as read, for exports.
    columns: Option<Columns>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Back> for RoastDetail {}
impl EventEmitter<CompareRoasts> for RoastDetail {}

/// Readings in rows, from the columns the engine sends.
pub(crate) fn rows(c: &Columns) -> Vec<SampleOut> {
    c.at.iter()
        .enumerate()
        .map(|(i, &at)| SampleOut {
            at,
            values: c
                .values
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.get(i).copied().flatten()?)))
                .collect(),
            ror: c.ror.get(i).copied().flatten(),
        })
        .collect()
}

impl RoastDetail {
    pub fn new(
        engine: Engine,
        id: Uuid,
        time_scale: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let menu_note =
            cx.new(|cx| InputState::new(window, cx).placeholder("Note at this instant"));
        let weight = cx.new(|cx| InputState::new(window, cx).placeholder("0.000"));
        let subscriptions = vec![
            cx.subscribe_in(
                &menu_note,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.note_at_menu(window, cx);
                    }
                },
            ),
            cx.subscribe_in(&weight, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save_weight(cx);
                }
            }),
        ];
        let page = Self {
            engine,
            id,
            state: LiveState::new(time_scale),
            message: None,
            geometry: Rc::new(Cell::new(None)),
            hover: None,
            menu: None,
            menu_note,
            weight,
            weight_shown: false,
            columns: None,
            _subscriptions: subscriptions,
        };
        page.load(cx);
        page
    }

    /// Reads the roast, its readings and its reference's.
    fn load(&self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let id = RoastId(self.id);
        cx.spawn(async move |this, cx| {
            let doc = engine.ask(|r| Request::Roast(id, r))?.await.ok()?.ok()?;
            let columns = engine.ask(|r| Request::Samples(id, r))?.await.ok()?.ok()?;
            let reference = match doc.reference.as_ref() {
                Some(r) => {
                    let rid = RoastId(r.id);
                    let rdoc = engine.ask(|r| Request::Roast(rid, r))?.await.ok()?.ok();
                    let rcols = engine.ask(|r| Request::Samples(rid, r))?.await.ok()?.ok();
                    rdoc.zip(rcols)
                }
                None => None,
            };
            this.update(cx, |this, cx| {
                this.state.samples = rows(&columns);
                this.columns = Some(columns);
                this.state.roast = Some(doc);
                if let Some((rdoc, rcols)) = reference {
                    this.state.set_reference(&rdoc, &rcols);
                }
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    /// Sends an edit; reloads the roast when it is done, shows the refusal if not.
    fn edit<T: 'static>(&mut self, make: impl FnOnce(Reply<T>) -> Request, cx: &mut Context<Self>) {
        self.message = None;
        let Some(reply) = self.engine.ask(make) else {
            self.message = Some("Roastro is busy; try again".into());
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = reply.await.ok()?;
            this.update(cx, |this, cx| {
                match result {
                    Ok(_) => this.load(cx),
                    Err(err) => this.message = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn move_event(&mut self, kind: EventKind, at: Millis, cx: &mut Context<Self>) {
        let id = RoastId(self.id);
        self.menu = None;
        self.edit(
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

    fn unmark(&mut self, kind: EventKind, cx: &mut Context<Self>) {
        let id = RoastId(self.id);
        self.edit(
            |r| Request::Edit(Edit::DeleteEvent(id, kind.as_str().to_owned(), r)),
            cx,
        );
    }

    fn remove_note(&mut self, note: Uuid, cx: &mut Context<Self>) {
        let id = RoastId(self.id);
        self.edit(|r| Request::Edit(Edit::DeleteNote(id, note, r)), cx);
    }

    fn note_at_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, at)) = self.menu else {
            return;
        };
        let text = self.menu_note.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let id = RoastId(self.id);
        self.menu = None;
        self.menu_note
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.edit(
            |r| Request::Edit(Edit::AddNote(id, NoteIn { at, text }, r)),
            cx,
        );
    }

    fn save_weight(&mut self, cx: &mut Context<Self>) {
        let text = self.weight.read(cx).value().to_string();
        let Some(grams) = grams(&text) else {
            self.message = Some("roasted weight: kilograms, like 4.250".into());
            cx.notify();
            return;
        };
        let id = RoastId(self.id);
        let patch = RoastPatchIn {
            roasted_weight_g: Some(grams),
            ..RoastPatchIn::default()
        };
        self.edit(|r| Request::Edit(Edit::PatchRoast(id, patch, r)), cx);
    }

    /// Makes this roast the reference of the Next roast, keeping the rest.
    fn use_as_reference(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let id = self.id;
        self.message = None;
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
                    Ok(_) => "The next roast will follow this one.".into(),
                    Err(err) => err.to_string(),
                });
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    /// Saves the roast as CSV where the person chooses.
    fn export_csv(&mut self, cx: &mut Context<Self>) {
        let (Some(roast), Some(columns)) = (self.state.roast.as_ref(), self.columns.as_ref())
        else {
            return;
        };
        let csv = roastro::export::roast_csv(roast, columns, self.state.time_scale);
        let name = roastro::export::file_name(roast, "csv");
        save_as(csv.into_bytes(), &name, cx);
    }

    /// Saves the roast's report as PDF where the person chooses.
    fn export_pdf(&mut self, cx: &mut Context<Self>) {
        let (Some(roast), Some(columns)) = (self.state.roast.as_ref(), self.columns.as_ref())
        else {
            return;
        };
        let points: Vec<(f64, Option<f64>)> = self
            .state
            .reference
            .as_ref()
            .map(|r| r.points.iter().map(|p| (p.0, p.1)).collect())
            .unwrap_or_default();
        let reference = roast
            .reference
            .as_ref()
            .filter(|_| !points.is_empty())
            .map(|r| roastro::report::ReferenceCurve {
                number: r.number,
                points: &points,
            });
        let pdf =
            roastro::report::roast_pdf(roast, columns, reference.as_ref(), self.state.time_scale);
        let name = roastro::export::file_name(roast, "pdf");
        save_as(pdf, &name, cx);
    }

    /// Compares this roast with the previous roast of its coffee.
    fn compare_with_previous(&mut self, cx: &mut Context<Self>) {
        let Some(roast) = self.state.roast.as_ref() else {
            return;
        };
        let id = roast.id;
        let query = RoastQueryIn {
            before: Some(id),
            coffee: roast.coffee.as_ref().map(|c| c.id),
            ..RoastQueryIn::default()
        };
        let Some(reply) = self.engine.ask(|r| Request::Edit(Edit::Roasts(query, r))) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let page = reply.await.ok()?.ok()?;
            this.update(cx, |this, cx| {
                if let Some(previous) = page.roasts.first() {
                    cx.emit(CompareRoasts(vec![id, previous.id]));
                } else {
                    this.message = Some("No earlier roast of this coffee.".into());
                    cx.notify();
                }
            })
            .ok()
        })
        .detach();
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

    fn pointer_down(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(s) = self.geometry.get().and_then(|g| g.roast_s(position)) else {
            return;
        };
        let Some(at) = self.state.wall_at(s) else {
            return;
        };
        self.menu = Some((position, at));
        self.hover = None;
        cx.notify();
    }

    fn sync_weight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.weight_shown {
            return;
        }
        let Some(roast) = self.state.roast.as_ref() else {
            return;
        };
        self.weight_shown = true;
        let text = roast.roasted_weight_g.map(kilograms).unwrap_or_default();
        self.weight
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

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
        let (position, at) = self.menu?;
        let st = &self.state;
        let s = st.roast_s(at);
        let buttons = MOVABLE.map(|kind| {
            let was = st
                .event_at(kind)
                .map(|t| format!("   was {}", clock_signed(st.roast_s(t))))
                .unwrap_or_default();
            Button::new(SharedString::from(format!("move-{}", kind.as_str())))
                .label(format!("{} here{was}", chart::label(kind)))
                .w_full()
                .on_click(cx.listener(move |this, _, _, cx| this.move_event(kind, at, cx)))
        });
        let panel = div()
            .id("detail-menu")
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
                    .child(div().font_weight(FontWeight::BOLD).child(clock_signed(s)))
                    .child(
                        div()
                            .text_sm()
                            .text_color(chart::bt())
                            .child(format!("BT {}°", fmt1(st.bt_at(at)))),
                    ),
            )
            .children(buttons)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Input::new(&self.menu_note).id("detail-note").flex_1())
                    .child(Button::new("detail-add-note").label("Add note").on_click(
                        cx.listener(|this, _, window, cx| this.note_at_menu(window, cx)),
                    )),
            );
        Some(
            deferred(
                anchored()
                    .position(position + point(px(12.), px(12.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(panel),
            )
            .into_any_element(),
        )
    }

    fn header(roast: &RoastDoc, cx: &mut Context<Self>) -> impl IntoElement {
        let weights = match (roast.green_weight_g, roast.roasted_weight_g) {
            (Some(g), Some(r)) => format!("{} → {} kg", kilograms(g), kilograms(r)),
            (Some(g), None) => format!("{} kg green", kilograms(g)),
            _ => String::new(),
        };
        let line = [
            roast.charge_at.map(day).unwrap_or_default(),
            roast.roasted_by.clone(),
            weights,
            roast
                .reference
                .as_ref()
                .map(|r| format!("reference #{}", r.number))
                .unwrap_or_default(),
        ]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
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
                Button::new("back")
                    .ghost()
                    .label("← Roasts")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Back))),
            )
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Roast #{}", roast.number)),
            )
            .child(
                div().text_color(white(0.85)).child(
                    roast
                        .coffee
                        .as_ref()
                        .map_or_else(|| "No coffee set".to_owned(), |c| c.name.clone()),
                ),
            )
            .child(div().text_sm().text_color(white(0.5)).child(line))
            .child(div().flex_1())
            .children(roast.reference.as_ref().map(|r| {
                let pair = vec![roast.id, r.id];
                Button::new("compare-reference")
                    .label(format!("Compare with its reference #{}", r.number))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(CompareRoasts(pair.clone()))))
            }))
            .child(
                Button::new("compare-previous")
                    .label("Compare with the previous")
                    .on_click(cx.listener(|this, _, _, cx| this.compare_with_previous(cx))),
            )
            .child(
                Button::new("export-pdf")
                    .label("Export PDF")
                    .on_click(cx.listener(|this, _, _, cx| this.export_pdf(cx))),
            )
            .child(
                Button::new("export-csv")
                    .label("Export CSV")
                    .on_click(cx.listener(|this, _, _, cx| this.export_csv(cx))),
            )
            .child(
                Button::new("use-as-reference")
                    .primary()
                    .label("Use as reference for the next roast")
                    .on_click(cx.listener(|this, _, _, cx| this.use_as_reference(cx))),
            )
    }

    fn events(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let st = &self.state;
        let rows = st.roast.iter().flat_map(|r| r.events.iter()).map(|e| {
            let kind = e.kind.parse::<EventKind>().ok();
            div()
                .flex()
                .items_center()
                .gap_3()
                .text_sm()
                .child(
                    div()
                        .w(px(44.))
                        .text_color(white(0.5))
                        .child(clock(st.roast_s(e.at))),
                )
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(kind.map_or(e.kind, chart::label)),
                )
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
                    Button::new(SharedString::from(format!("unmark-{}", kind.as_str())))
                        .ghost()
                        .xsmall()
                        .label("✕")
                        .on_click(cx.listener(move |this, _, _, cx| this.unmark(kind, cx)))
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(label("Events"))
            .children(rows)
            .child(
                div()
                    .text_xs()
                    .text_color(white(0.35))
                    .child("Click the chart to move an event or add a note."),
            )
    }

    fn notes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let st = &self.state;
        let notes = st.roast.iter().flat_map(|r| r.notes.iter());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(label("Notes"))
            .children(notes.enumerate().map(|(i, n)| {
                let id = n.id;
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_sm()
                    .child(
                        div()
                            .w(px(44.))
                            .text_color(white(0.5))
                            .child(clock(st.roast_s(n.at))),
                    )
                    .child(div().flex_1().child(n.text.clone()))
                    .child(
                        Button::new(SharedString::from(format!("remove-note-{}", i + 1)))
                            .ghost()
                            .xsmall()
                            .label("✕")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_note(id, cx))),
                    )
            }))
            .child(label("Roasted weight (kg)"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(Input::new(&self.weight).id("detail-weight").w(px(120.)))
                    .child(
                        Button::new("detail-save-weight")
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.save_weight(cx))),
                    ),
            )
    }
}

impl Render for RoastDetail {
    #[allow(clippy::too_many_lines)] // the page's layout, read top to bottom
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_weight(window, cx);
        let Some(roast) = self.state.roast.clone() else {
            return div()
                .id("page-roast")
                .test_support()
                .p_6()
                .text_color(white(0.5))
                .child("Loading the roast…")
                .into_any_element();
        };
        let hover = if self.menu.is_none() {
            self.hover_overlay()
        } else {
            None
        };
        let menu = self.menu_overlay(cx);
        let [total, dtr, development, drop_bt, loss] = summary_figures(&self.state);
        div()
            .id("page-roast")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .children(hover)
            .children(menu)
            .child(Self::header(&roast, cx))
            .children(self.message.clone().map(|m| {
                div()
                    .id("detail-message")
                    .px_6()
                    .pt_2()
                    .text_sm()
                    .text_color(white(0.7))
                    .child(m)
            }))
            .child(
                div()
                    .id("detail-chart")
                    .test_support()
                    .h(gpui_kit::relative(0.5))
                    .min_h(px(280.))
                    .w_full()
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
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            this.pointer_down(ev.position, cx);
                        }),
                    )
                    .child(chart::chart(
                        &self.state,
                        self.geometry.clone(),
                        self.hover.map(|h| h.1),
                    )),
            )
            .child(
                div()
                    .id("detail-below")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_wrap()
                    .gap_8()
                    .px_6()
                    .py_4()
                    .child(
                        div()
                            .w(px(420.))
                            .flex()
                            .flex_col()
                            .gap_5()
                            .child(total.render(Size::Large))
                            .child(
                                div()
                                    .flex()
                                    .gap_6()
                                    .child(dtr.render(Size::Medium))
                                    .child(development.render(Size::Medium)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_6()
                                    .child(drop_bt.render(Size::Medium))
                                    .child(loss.render(Size::Medium)),
                            )
                            .child(phases(&self.state)),
                    )
                    .child(div().w(px(340.)).child(self.events(cx)))
                    .child(div().w(px(340.)).child(self.notes(cx))),
            )
            .into_any_element()
    }
}

/// Asks where to save `bytes` (starting in Documents) and writes them there,
/// saying on the page how it went.
fn save_as(bytes: Vec<u8>, name: &str, cx: &mut Context<RoastDetail>) {
    let dir = dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default();
    let chosen = cx.prompt_for_new_path(&dir, Some(name));
    cx.spawn(async move |this, cx| {
        let path = chosen.await.ok()?.ok()??;
        let written = cx
            .background_executor()
            .spawn({
                let path = path.clone();
                async move { std::fs::write(&path, bytes) }
            })
            .await;
        this.update(cx, |this, cx| {
            this.message = Some(match written {
                Ok(()) => format!("Saved {}", path.display()),
                Err(err) => format!("Could not save {}: {err}", path.display()),
            });
            cx.notify();
        })
        .ok()
    })
    .detach();
}
