//! Coffees and one Coffee ("Roast history and curve comparison", #10): the
//! green coffees, where they come from and how they have roasted. Editing a
//! coffee happens only on its own page.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, IntoElement, SharedString, Window, div,
    px,
};
use i18n::{t, tf};
use roastro::engine::{Edit, Request};
use roastro::wire::{CoffeeIn, CoffeeOut, CoffeePatch, RoastDoc, RoastQueryIn};
use uuid::Uuid;

use crate::roasts::{OpenRoast, day};
use crate::view::rail::label;
use crate::view::{Engine, clock, kilograms, white};

/// The shell opens this coffee's page.
pub struct OpenCoffee(pub Uuid);
/// The shell prepares the next roast with this coffee.
pub struct RoastItNext(pub Uuid);
/// The shell lists this coffee's roasts.
pub struct ShowRoasts(pub Uuid);

/// A coffee's text fields, as inputs.
struct Fields {
    name: Entity<InputState>,
    lot: Entity<InputState>,
    origin: Entity<InputState>,
    process: Entity<InputState>,
    supplier: Entity<InputState>,
    notes: Entity<InputState>,
}

impl Fields {
    fn new<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<V>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        Self {
            name: input(t("Ethiopia Guji Natural"), window, cx),
            lot: input("2401", window, cx),
            origin: input(t("Ethiopia"), window, cx),
            process: input(t("Washed, natural, honey…"), window, cx),
            supplier: input(t("Who sold it"), window, cx),
            notes: input(t("How it roasts, what to watch"), window, cx),
        }
    }

    fn fill<V: 'static>(&self, c: &CoffeeOut, window: &mut Window, cx: &mut Context<V>) {
        for (input, value) in [
            (&self.name, &c.name),
            (&self.lot, &c.lot),
            (&self.origin, &c.origin),
            (&self.process, &c.process),
            (&self.supplier, &c.supplier),
            (&self.notes, &c.notes),
        ] {
            let value = value.clone();
            input.update(cx, |i, cx| i.set_value(value, window, cx));
        }
    }

    fn text<V>(input: &Entity<InputState>, cx: &Context<V>) -> String {
        input.read(cx).value().trim().to_owned()
    }

    fn as_new<V>(&self, cx: &Context<V>) -> CoffeeIn {
        CoffeeIn {
            name: Self::text(&self.name, cx),
            notes: Self::text(&self.notes, cx),
            lot: Self::text(&self.lot, cx),
            origin: Self::text(&self.origin, cx),
            process: Self::text(&self.process, cx),
            supplier: Self::text(&self.supplier, cx),
        }
    }

    fn as_patch<V>(&self, cx: &Context<V>) -> CoffeePatch {
        CoffeePatch {
            name: Some(Self::text(&self.name, cx)),
            notes: Some(Self::text(&self.notes, cx)),
            lot: Some(Self::text(&self.lot, cx)),
            origin: Some(Self::text(&self.origin, cx)),
            process: Some(Self::text(&self.process, cx)),
            supplier: Some(Self::text(&self.supplier, cx)),
            archived: None,
        }
    }

    fn form(&self, prefix: &str) -> impl IntoElement {
        let field = |name: &'static str, input: &Entity<InputState>, id: String| {
            div()
                .flex_1()
                .min_w(px(200.))
                .flex()
                .flex_col()
                .gap_1()
                .child(label(name))
                .child(Input::new(input).id(SharedString::from(id)))
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(field(t("Name"), &self.name, format!("{prefix}-name")))
                    .child(field(t("Lot"), &self.lot, format!("{prefix}-lot"))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(field(t("Origin"), &self.origin, format!("{prefix}-origin")))
                    .child(field(
                        t("Process"),
                        &self.process,
                        format!("{prefix}-process"),
                    ))
                    .child(field(
                        t("Supplier"),
                        &self.supplier,
                        format!("{prefix}-supplier"),
                    )),
            )
            .child(field(t("Notes"), &self.notes, format!("{prefix}-notes")))
    }
}

fn percent(v: Option<f64>) -> String {
    v.map_or_else(|| "—".to_owned(), |v| format!("{:.1}%", v * 100.0))
}

/// The Coffees page: every coffee and how it has roasted.
pub struct CoffeesPage {
    engine: Engine,
    coffees: Vec<CoffeeOut>,
    show_finished: bool,
    adding: Option<Fields>,
    message: Option<String>,
}

impl EventEmitter<OpenCoffee> for CoffeesPage {}

impl CoffeesPage {
    pub fn new(engine: Engine, cx: &mut Context<Self>) -> Self {
        let page = Self {
            engine,
            coffees: Vec::new(),
            show_finished: false,
            adding: None,
            message: None,
        };
        page.reload(cx);
        page
    }

    /// Reads the coffees again: when shown, and after a change.
    pub fn reload(&self, cx: &mut Context<Self>) {
        let finished = self.show_finished;
        let Some(reply) = self
            .engine
            .ask(|r| Request::Edit(Edit::Coffees(finished, r)))
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let coffees = reply.await.ok()?.ok()?;
            this.update(cx, |this, cx| {
                this.coffees = coffees;
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let Some(fields) = &self.adding else {
            return;
        };
        let body = fields.as_new(cx);
        if body.name.is_empty() {
            self.message = Some(t("A coffee needs a name.").into());
            cx.notify();
            return;
        }
        let Some(reply) = self.engine.ask(|r| Request::Edit(Edit::AddCoffee(body, r))) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = reply.await.ok()?;
            this.update(cx, |this, cx| {
                match result {
                    Ok(coffee) => {
                        this.adding = None;
                        this.message = None;
                        this.reload(cx);
                        cx.emit(OpenCoffee(coffee.id));
                    }
                    Err(err) => this.message = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn row(c: &CoffeeOut, cx: &mut Context<Self>) -> impl IntoElement {
        let id = c.id;
        let stats = c.stats.as_ref();
        let cell = |text: String, w: f32| div().w(px(w)).flex().justify_end().child(text);
        let origin = [c.origin.as_str(), c.process.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .id(SharedString::from(format!("coffee-{id}")))
            .test_support()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .opacity(if c.archived { 0.5 } else { 1.0 })
            .hover(|d| d.bg(white(0.03)))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenCoffee(id))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(c.name.clone()),
                    )
                    .child(div().text_sm().text_color(white(0.5)).child(if c.archived {
                        tf("{origin} · lot finished", &[("origin", &origin)])
                    } else {
                        origin
                    })),
            )
            .child(div().w(px(80.)).text_sm().child(c.lot.clone()))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .text_sm()
                    .child(cell(stats.map_or(0, |s| s.roasts).to_string(), 56.))
                    .child(cell(
                        stats.map_or_else(
                            || "—".to_owned(),
                            |s| {
                                let when = s.last_charge_at.map(day).unwrap_or_default();
                                format!("#{} · {when}", s.last_number)
                            },
                        ),
                        200.,
                    ))
                    .child(cell(
                        stats.map_or_else(
                            || "—".to_owned(),
                            |s| format!("{} kg", kilograms(s.green_weight_g)),
                        ),
                        96.,
                    ))
                    .child(cell(
                        percent(stats.and_then(|s| s.average_development_time_ratio)),
                        72.,
                    ))
                    .child(cell(
                        percent(stats.and_then(|s| s.average_weight_loss_ratio)),
                        72.,
                    )),
            )
    }
}

impl Render for CoffeesPage {
    #[allow(clippy::too_many_lines)] // the page's layout, read top to bottom
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header =
            |text: &'static str, w: f32| div().w(px(w)).flex().justify_end().child(label(text));
        let rows: Vec<AnyElement> = self
            .coffees
            .iter()
            .map(|c| Self::row(c, cx).into_any_element())
            .collect();
        let empty = rows.is_empty();
        let finished = self.show_finished;
        div()
            .id("page-coffees")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
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
                            .child(t("Coffees")),
                    )
                    .child(
                        div()
                            .text_color(white(0.5))
                            .child(tf("{n} green coffees", &[("n", &self.coffees.len())])),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("show-finished")
                            .ghost()
                            .label(if finished {
                                t("Hide finished lots")
                            } else {
                                t("Show finished lots")
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_finished = !this.show_finished;
                                this.reload(cx);
                            })),
                    )
                    .child(
                        Button::new("add-coffee-open")
                            .primary()
                            .label(t("Add coffee"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.adding = Some(Fields::new(window, cx));
                                cx.notify();
                            })),
                    ),
            )
            .children(self.adding.as_ref().map(|fields| {
                div()
                    .id("add-coffee-form")
                    .test_support()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .mx_6()
                    .mt_4()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(white(0.1))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t("New coffee")),
                    )
                    .child(fields.form("new-coffee"))
                    .children(
                        self.message
                            .clone()
                            .map(|m| div().text_sm().text_color(white(0.7)).child(m)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("add-coffee-save")
                                    .primary()
                                    .label(t("Add coffee"))
                                    .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
                            )
                            .child(
                                Button::new("add-coffee-cancel")
                                    .label(t("Cancel"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.adding = None;
                                        this.message = None;
                                        cx.notify();
                                    })),
                            ),
                    )
            }))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .px_4()
                    .mx_2()
                    .pt_3()
                    .child(div().flex_1().child(label(t("Coffee"))))
                    .child(div().w(px(80.)).child(label(t("Lot"))))
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(header(t("Roasts"), 56.))
                            .child(header(t("Last roast"), 200.))
                            .child(header(t("Green used"), 96.))
                            .child(header(t("Avg DTR"), 72.))
                            .child(header(t("Avg loss"), 72.)),
                    ),
            )
            .child(
                div()
                    .id("coffee-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_6()
                    .children(rows)
                    .children(empty.then(|| {
                        div().p_6().text_color(white(0.45)).child(t(
                            "No coffees yet. Add the first one to pick it for a roast.",
                        ))
                    })),
            )
    }
}

/// One coffee's page: its fields, its latest roasts, Roast it next.
pub struct CoffeePage {
    engine: Engine,
    id: Uuid,
    time_scale: f64,
    coffee: Option<CoffeeOut>,
    fields: Fields,
    /// The coffee was put in the fields once loaded.
    filled: bool,
    roasts: Vec<RoastDoc>,
    message: Option<String>,
}

impl EventEmitter<OpenRoast> for CoffeePage {}
impl EventEmitter<RoastItNext> for CoffeePage {}
impl EventEmitter<ShowRoasts> for CoffeePage {}

impl CoffeePage {
    pub fn new(
        engine: Engine,
        id: Uuid,
        time_scale: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let page = Self {
            engine,
            id,
            time_scale,
            coffee: None,
            fields: Fields::new(window, cx),
            filled: false,
            roasts: Vec::new(),
            message: None,
        };
        page.load(cx);
        page
    }

    fn load(&self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let id = self.id;
        cx.spawn(async move |this, cx| {
            let coffees = engine
                .ask(|r| Request::Edit(Edit::Coffees(true, r)))?
                .await
                .ok()?
                .ok()?;
            let query = RoastQueryIn {
                coffee: Some(id),
                ..RoastQueryIn::default()
            };
            let page = engine
                .ask(|r| Request::Edit(Edit::Roasts(query, r)))?
                .await
                .ok()?
                .ok()?;
            this.update(cx, |this, cx| {
                this.coffee = coffees.into_iter().find(|c| c.id == id);
                this.roasts = page.roasts.into_iter().take(6).collect();
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn patch(&mut self, patch: CoffeePatch, done: &'static str, cx: &mut Context<Self>) {
        let id = self.id;
        let Some(reply) = self
            .engine
            .ask(|r| Request::Edit(Edit::PatchCoffee(id, patch, r)))
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = reply.await.ok()?;
            this.update(cx, |this, cx| {
                this.message = Some(match result {
                    Ok(_) => done.to_owned(),
                    Err(err) => err.to_string(),
                });
                this.load(cx);
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn roast_row(&self, r: &RoastDoc, cx: &mut Context<Self>) -> impl IntoElement {
        let id = r.id;
        #[allow(clippy::cast_precision_loss)] // minutes in milliseconds
        let total = r.metrics.duration_ms.map_or_else(
            || "—".to_owned(),
            |ms| clock(ms as f64 * self.time_scale / 1000.0),
        );
        div()
            .id(SharedString::from(format!("coffee-roast-{}", r.number)))
            .test_support()
            .flex()
            .gap_4()
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .hover(|d| d.bg(white(0.03)))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenRoast(id))))
            .child(
                div()
                    .w(px(80.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("#{}", r.number)),
            )
            .child(
                div()
                    .w(px(160.))
                    .text_color(white(0.55))
                    .child(r.charge_at.map(day).unwrap_or_default()),
            )
            .child(div().w(px(60.)).child(total))
            .child(
                div()
                    .w(px(80.))
                    .child(format!("DTR {}", percent(r.metrics.development_time_ratio))),
            )
            .child(div().child(tf(
                "loss {loss}",
                &[("loss", &percent(r.metrics.weight_loss_ratio))],
            )))
    }
}

impl Render for CoffeePage {
    #[allow(clippy::too_many_lines)] // the page's layout, read top to bottom
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(coffee) = self.coffee.clone() else {
            return div()
                .id("page-coffee")
                .test_support()
                .p_6()
                .text_color(white(0.5))
                .child(t("Loading the coffee…"))
                .into_any_element();
        };
        if !self.filled {
            self.filled = true;
            self.fields.fill(&coffee, window, cx);
        }
        let id = coffee.id;
        let archived = coffee.archived;
        let rows: Vec<AnyElement> = self
            .roasts
            .iter()
            .map(|r| self.roast_row(r, cx).into_any_element())
            .collect();
        let none = rows.is_empty();
        div()
            .id("page-coffee")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
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
                            .child(coffee.name.clone()),
                    )
                    .children(
                        archived
                            .then(|| div().text_sm().text_color(white(0.5)).child(t("lot finished"))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("coffee-roasts")
                            .label(t("See all its roasts"))
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ShowRoasts(id)))),
                    )
                    .child(
                        Button::new("coffee-archive")
                            .label(if archived {
                                t("Bring the lot back")
                            } else {
                                t("Lot finished")
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let patch = CoffeePatch {
                                    archived: Some(!archived),
                                    ..CoffeePatch::default()
                                };
                                this.patch(
                                    patch,
                                    if archived {
                                        t("Back in the pickers.")
                                    } else {
                                        t("Hidden from the pickers; its roasts stay.")
                                    },
                                    cx,
                                );
                            })),
                    )
                    .children((!archived).then(|| {
                        Button::new("roast-it-next")
                            .primary()
                            .label(t("Roast it next"))
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(RoastItNext(id))))
                    })),
            )
            .children(self.message.clone().map(|m| {
                div()
                    .id("coffee-message")
                    .px_6()
                    .pt_3()
                    .text_sm()
                    .text_color(white(0.7))
                    .child(m)
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_8()
                    .px_6()
                    .py_4()
                    .child(
                        div()
                            .w(px(560.))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(self.fields.form("coffee"))
                            .child(div().child(
                                Button::new("coffee-save").label(t("Save changes")).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        let patch = this.fields.as_patch(cx);
                                        if patch.name.as_deref() == Some("") {
                                            this.message = Some(t("A coffee needs a name.").into());
                                            cx.notify();
                                            return;
                                        }
                                        this.patch(patch, t("Saved."), cx);
                                    }),
                                ),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(label(t("Latest roasts")))
                            .children(rows)
                            .children(none.then(|| {
                                div()
                                    .text_sm()
                                    .text_color(white(0.45))
                                    .child(t("Not roasted yet."))
                            }))
                            .children(coffee.stats.as_ref().map(|s| {
                                div().pt_2().text_sm().text_color(white(0.55)).child(tf(
                                    "{roasts} roasts · {green} kg green · average DTR {dtr} · average loss {loss}",
                                    &[
                                        ("roasts", &s.roasts),
                                        ("green", &kilograms(s.green_weight_g)),
                                        ("dtr", &percent(s.average_development_time_ratio)),
                                        ("loss", &percent(s.average_weight_loss_ratio)),
                                    ],
                                ))
                            })),
                    ),
            )
            .into_any_element()
    }
}
