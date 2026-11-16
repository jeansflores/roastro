//! The Next roast: what the operator prepares before Charge (coffee, green
//! weight, who roasts, the reference). Charge copies it into the new roast.
//! Every change is stored at once, so nothing is lost if the roast starts
//! by itself.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, IntoElement, SharedString, Subscription, Window, div, px};
use roastro::engine::{Edit, Request};
use roastro::wire::{CoffeeIn, CoffeeOut, Live, NextRoastIn, NextRoastOut, RoastDoc, RoastQueryIn};
use uuid::Uuid;

use super::rail::label;
use super::{RoastScreen, grams, kilograms, white};

/// One option of a select: an id, or none.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub value: Option<Uuid>,
    pub title: SharedString,
}

impl SelectItem for Choice {
    type Value = Option<Uuid>;

    fn value(&self) -> &Option<Uuid> {
        &self.value
    }

    fn title(&self) -> SharedString {
        self.title.clone()
    }
}

type Picker = Entity<SelectState<Vec<Choice>>>;

/// The form's widgets and what it shows.
pub struct NextRoastForm {
    coffee: Picker,
    reference: Picker,
    green: Entity<InputState>,
    roasted_by: Entity<InputState>,
    new_coffee: Entity<InputState>,
    coffees: Vec<CoffeeOut>,
    /// Done roasts, newest first: the references to choose from.
    roasts: Vec<RoastDoc>,
    /// The lists changed: rebuild the selects' options.
    stale: bool,
    /// The Next roast the widgets show.
    shown: Option<NextRoastOut>,
    /// A coffee was just chosen: take the reference the engine suggests.
    follow_suggestion: bool,
    _subscriptions: Vec<Subscription>,
}

impl NextRoastForm {
    pub fn new(window: &mut Window, cx: &mut Context<RoastScreen>) -> Self {
        let picker = |window: &mut Window, cx: &mut Context<RoastScreen>| {
            cx.new(|cx| SelectState::new(Vec::<Choice>::new(), None, window, cx))
        };
        let coffee = picker(window, cx);
        let reference = picker(window, cx);
        let green = cx.new(|cx| InputState::new(window, cx).placeholder("0.000"));
        let roasted_by = cx.new(|cx| InputState::new(window, cx));
        let new_coffee = cx.new(|cx| InputState::new(window, cx).placeholder("New coffee"));
        let subscriptions = vec![
            cx.subscribe_in(
                &coffee,
                window,
                |this, _, event: &SelectEvent<Vec<Choice>>, _, cx| {
                    let SelectEvent::Confirm(Some(coffee)) = event else {
                        return;
                    };
                    this.choose_coffee(*coffee, cx);
                },
            ),
            cx.subscribe_in(
                &reference,
                window,
                |this, _, event: &SelectEvent<Vec<Choice>>, _, cx| {
                    let SelectEvent::Confirm(Some(reference)) = event else {
                        return;
                    };
                    let reference = *reference;
                    this.put_next(|n| n.reference_id = reference, cx);
                },
            ),
            cx.subscribe_in(&green, window, |this, _, event: &InputEvent, _, cx| {
                if done_typing(event) {
                    this.save_green(cx);
                }
            }),
            cx.subscribe_in(&roasted_by, window, |this, _, event: &InputEvent, _, cx| {
                if done_typing(event) {
                    let name = this.next_form.roasted_by.read(cx).value().trim().to_owned();
                    if this.state.next.as_ref().map(|n| n.roasted_by.as_str()) != Some(&name) {
                        this.put_next(|n| n.roasted_by = name, cx);
                    }
                }
            }),
            cx.subscribe_in(
                &new_coffee,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add_coffee(window, cx);
                    }
                },
            ),
        ];
        Self {
            coffee,
            reference,
            green,
            roasted_by,
            new_coffee,
            coffees: Vec::new(),
            roasts: Vec::new(),
            stale: true,
            shown: None,
            follow_suggestion: false,
            _subscriptions: subscriptions,
        }
    }
}

/// Enter, or leaving the field.
fn done_typing(event: &InputEvent) -> bool {
    matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
}

/// "12 Nov", in the computer's time zone.
pub(crate) fn date(at: i64) -> String {
    jiff::Timestamp::from_millisecond(at)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%-d %b")
                .to_string()
        })
        .unwrap_or_default()
}

/// The coffee options: none, then the coffees by name.
fn coffee_choices(coffees: &[CoffeeOut]) -> Vec<Choice> {
    let mut choices = vec![Choice {
        value: None,
        title: "No coffee".into(),
    }];
    choices.extend(coffees.iter().map(|c| Choice {
        value: Some(c.id),
        title: c.name.clone().into(),
    }));
    choices
}

/// The reference options: done roasts, newest first, the suggested one
/// marked; then none.
fn reference_choices(roasts: &[RoastDoc], suggested: Option<Uuid>) -> Vec<Choice> {
    let mut choices: Vec<Choice> = roasts
        .iter()
        .map(|r| {
            let mut parts = vec![format!("Roast #{}", r.number)];
            parts.extend(r.coffee.as_ref().map(|c| c.name.clone()));
            parts.extend(r.charge_at.map(date));
            let mut title = parts.join(" · ");
            if Some(r.id) == suggested {
                title.push_str(" (latest of this coffee)");
            }
            Choice {
                value: Some(r.id),
                title: title.into(),
            }
        })
        .collect();
    choices.push(Choice {
        value: None,
        title: "No reference".into(),
    });
    choices
}

impl RoastScreen {
    /// Fetches the coffees and the done roasts the form chooses from.
    pub(super) fn load_choices(&self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let coffees = engine
                .ask(|r| Request::Edit(Edit::Coffees(false, r)))?
                .await
                .ok()?
                .ok()?;
            let page = engine
                .ask(|r| Request::Edit(Edit::Roasts(RoastQueryIn::default(), r)))?
                .await
                .ok()?
                .ok()?;
            this.update(cx, |screen, cx| {
                let form = &mut screen.next_form;
                form.coffees = coffees;
                form.roasts = page
                    .roasts
                    .into_iter()
                    .filter(|r| r.state == "done")
                    .collect();
                form.stale = true;
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    /// What the Next roast form reacts to on the live stream: a fresh
    /// snapshot or a finished roast changes the lists; a new Next roast after
    /// choosing a coffee brings its suggested reference.
    pub(super) fn next_roast_message(&mut self, msg: &Live, cx: &mut Context<Self>) {
        match msg {
            Live::Snapshot(_) => self.load_choices(cx),
            Live::Roast(r) if r.state != "recording" => self.load_choices(cx),
            Live::NextRoast(next) => {
                if std::mem::take(&mut self.next_form.follow_suggestion)
                    && next.reference_id.is_none()
                    && let Some(suggested) = next.suggested_reference_id
                {
                    self.put_next(|n| n.reference_id = Some(suggested), cx);
                }
            }
            _ => {}
        }
    }

    /// Stores the Next roast with one change.
    pub(super) fn put_next(
        &mut self,
        change: impl FnOnce(&mut NextRoastIn),
        cx: &mut Context<Self>,
    ) {
        let current = self.state.next.clone();
        let mut next = NextRoastIn {
            coffee_id: current.as_ref().and_then(|n| n.coffee_id),
            green_weight_g: current.as_ref().and_then(|n| n.green_weight_g),
            roasted_by: current.map(|n| n.roasted_by).unwrap_or_default(),
            reference_id: self.state.next.as_ref().and_then(|n| n.reference_id),
        };
        change(&mut next);
        self.send(|r| Request::Edit(Edit::PutNextRoast(next, r)), cx);
    }

    /// A new coffee clears the reference, then takes the suggested one.
    pub(crate) fn choose_coffee(&mut self, coffee: Option<Uuid>, cx: &mut Context<Self>) {
        if self.state.next.as_ref().and_then(|n| n.coffee_id) == coffee {
            return;
        }
        self.next_form.follow_suggestion = coffee.is_some();
        self.put_next(
            |n| {
                n.coffee_id = coffee;
                n.reference_id = None;
            },
            cx,
        );
    }

    fn save_green(&mut self, cx: &mut Context<Self>) {
        let text = self.next_form.green.read(cx).value().to_string();
        let Some(green) = grams(&text) else {
            self.message = Some("green weight: kilograms, like 5.000".into());
            cx.notify();
            return;
        };
        if self.state.next.as_ref().and_then(|n| n.green_weight_g) != green {
            self.put_next(|n| n.green_weight_g = green, cx);
        }
    }

    fn add_coffee(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.next_form.new_coffee.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        self.next_form
            .new_coffee
            .update(cx, |input, cx| input.set_value("", window, cx));
        let Some(reply) = self.engine.ask(|r| {
            Request::Edit(Edit::AddCoffee(
                CoffeeIn {
                    name,
                    ..CoffeeIn::default()
                },
                r,
            ))
        }) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = reply.await.ok()?;
            this.update(cx, |screen, cx| {
                match result {
                    Ok(coffee) => {
                        screen.choose_coffee(Some(coffee.id), cx);
                        screen.load_choices(cx);
                    }
                    Err(err) => screen.message = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    /// Brings the widgets in line with the lists and the stored Next roast,
    /// leaving alone what the operator is typing.
    pub(super) fn sync_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.state.next.clone();
        let form = &mut self.next_form;
        if !form.stale && form.shown == next {
            return;
        }
        let suggested = next.as_ref().and_then(|n| n.suggested_reference_id);
        let coffees = coffee_choices(&form.coffees);
        let references = reference_choices(&form.roasts, suggested);
        let coffee = next.as_ref().and_then(|n| n.coffee_id);
        let reference = next.as_ref().and_then(|n| n.reference_id);
        form.coffee.update(cx, |select, cx| {
            select.set_items(coffees, window, cx);
            select.set_selected_value(&coffee, window, cx);
        });
        form.reference.update(cx, |select, cx| {
            select.set_items(references, window, cx);
            select.set_selected_value(&reference, window, cx);
        });
        let green = next.as_ref().and_then(|n| n.green_weight_g);
        if grams(&form.green.read(cx).value()) != Some(green) {
            let text = green.map(kilograms).unwrap_or_default();
            form.green
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
        let by = next
            .as_ref()
            .map(|n| n.roasted_by.clone())
            .unwrap_or_default();
        if form.roasted_by.read(cx).value().trim() != by {
            form.roasted_by
                .update(cx, |input, cx| input.set_value(by, window, cx));
        }
        form.stale = false;
        form.shown = next;
    }

    /// The form, as the rail shows it before Charge and after Drop.
    pub(super) fn next_roast_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let form = &self.next_form;
        let field = |name: &'static str, widget: gpui_kit::AnyElement| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(label(name))
                .child(widget)
        };
        div()
            .id("next-roast")
            .flex()
            .flex_col()
            .gap_3()
            .child(field(
                "Coffee",
                Select::new(&form.coffee)
                    .placeholder("Choose a coffee")
                    .into_any_element(),
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Input::new(&form.new_coffee).id("new-coffee").flex_1())
                    .child(
                        Button::new("add-coffee").label("Add coffee").on_click(
                            cx.listener(|this, _, window, cx| this.add_coffee(window, cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(field(
                        "Green weight (kg)",
                        Input::new(&form.green)
                            .id("green-weight")
                            .into_any_element(),
                    ))
                    .child(field(
                        "Roasted by",
                        Input::new(&form.roasted_by)
                            .id("roasted-by")
                            .into_any_element(),
                    )),
            )
            .child(field(
                "Reference roast",
                Select::new(&form.reference)
                    .placeholder("No reference")
                    .into_any_element(),
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(white(0.4))
                    .child("Saved as you go; Charge takes it into the roast."),
            )
            .min_w(px(0.))
    }
}
