//! Machine: what Roastro reads and how the link is doing ("Machine setup and
//! connection lifecycle", #34). Its settings, typed by a person and tested
//! before saving; the readings now; Retry now; the drops. Roastro only reads
//! the machine: gas, air and drum are set on its own panel.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, Hsla, IntoElement, SharedString,
    Subscription, Window, div, px, rgb,
};
use roastro::engine::{Edit, Request};
use roastro::machine::{CALORATTO, MODELS};
use roastro::wire::{LinkDropOut, LinkState, MachineIn, TestOut};

use crate::chart;
use crate::new_roast::GoLive;
use crate::roasts::day;
use crate::state::LiveState;
use crate::view::rail::label;
use crate::view::{Engine, RoastScreen, clock, white};

/// A model to pick.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoice {
    id: &'static str,
    title: SharedString,
}

impl SelectItem for ModelChoice {
    type Value = &'static str;

    fn value(&self) -> &&'static str {
        &self.id
    }

    fn title(&self) -> SharedString {
        self.title.clone()
    }
}

/// How a reading is called on screen, and its unit.
fn reading_name(point: &str) -> (&str, &'static str) {
    match point {
        "bt" => ("Bean temp · BT", "°C"),
        "et" => ("Air temp · ET", "°C"),
        "gas_cmd" => ("Gas, set", "%"),
        "gas_fb" => ("Gas", "%"),
        "drum" => ("Drum", "%"),
        "fan" => ("Air (fan)", "%"),
        "roast_point" => ("Roast point", "°C"),
        "timer_s" => ("Timer", "s"),
        other => (other, ""),
    }
}

/// The link in words and a color.
pub fn link_words(st: &LiveState) -> (String, Hsla) {
    let red: Hsla = rgb(0x00f8_7171).into();
    let Some(link) = &st.link else {
        return ("Starting…".into(), white(0.6));
    };
    match link.state {
        LinkState::NotSetUp => ("Not set up".into(), white(0.6)),
        LinkState::Connecting => ("Connecting…".into(), white(0.6)),
        LinkState::Connected => ("Connected".into(), chart::ror()),
        LinkState::Down => {
            let retry = link
                .retry_at
                .map(|at| (at - roastro::now_ms()).max(0) / 1000)
                .map_or_else(String::new, |s| format!(" · retrying in {s} s"));
            (format!("Offline{retry}"), red)
        }
    }
}

/// The Machine page.
#[allow(clippy::struct_excessive_bools)] // independent switches of one form
pub struct MachinePage {
    engine: Engine,
    live: Entity<RoastScreen>,
    /// The settings form is open.
    editing: bool,
    name: Entity<InputState>,
    address: Entity<InputState>,
    model: Entity<SelectState<Vec<ModelChoice>>>,
    model_id: &'static str,
    /// Use the Demo machine instead of an address.
    demo: bool,
    /// The machine was put in the form.
    filled: bool,
    testing: bool,
    test: Option<Result<TestOut, String>>,
    drops: Vec<LinkDropOut>,
    message: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GoLive> for MachinePage {}

impl MachinePage {
    pub fn new(
        engine: Engine,
        live: Entity<RoastScreen>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let choices: Vec<ModelChoice> = MODELS
            .iter()
            .map(|(id, title)| ModelChoice {
                id,
                title: (*title).into(),
            })
            .collect();
        let model = cx.new(|cx| SelectState::new(choices, None, window, cx));
        let subscriptions = vec![
            cx.observe(&live, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &model,
                window,
                |this, _, event: &SelectEvent<Vec<ModelChoice>>, _, cx| {
                    if let SelectEvent::Confirm(Some(id)) = event {
                        this.model_id = id;
                        this.test = None;
                        cx.notify();
                    }
                },
            ),
        ];
        let page = Self {
            engine,
            live,
            editing: false,
            name: cx.new(|cx| InputState::new(window, cx).placeholder("Caloratto")),
            address: cx.new(|cx| InputState::new(window, cx).placeholder("192.168.5.11:502")),
            model,
            model_id: CALORATTO,
            demo: false,
            filled: false,
            testing: false,
            test: None,
            drops: Vec::new(),
            message: None,
            _subscriptions: subscriptions,
        };
        page.load_drops(cx);
        page
    }

    /// Reads the drop log: when shown, and when the link changes.
    pub fn load_drops(&self, cx: &mut Context<Self>) {
        let Some(reply) = self.engine.ask(|r| Request::Edit(Edit::LinkDrops(r))) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let drops = reply.await.ok()?.ok()?;
            this.update(cx, |this, cx| {
                this.drops = drops;
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn settings(&self, cx: &Context<Self>) -> MachineIn {
        MachineIn {
            name: self.name.read(cx).value().trim().to_owned(),
            address: self.address.read(cx).value().trim().to_owned(),
            model: self.model_id.to_owned(),
            demo: self.demo,
        }
    }

    fn run_test(&mut self, cx: &mut Context<Self>) {
        let body = self.settings(cx);
        let Some(reply) = self
            .engine
            .ask(|r| Request::Edit(Edit::TestMachine(body, r)))
        else {
            return;
        };
        self.testing = true;
        self.test = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = reply.await.ok()?;
            this.update(cx, |this, cx| {
                this.testing = false;
                this.test = Some(result.map_err(|e| message_of(&e)));
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let body = self.settings(cx);
        let Some(reply) = self
            .engine
            .ask(|r| Request::Edit(Edit::SetMachine(body, r)))
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = reply.await.ok()?;
            this.update(cx, |this, cx| {
                match result {
                    Ok(machine) => {
                        this.editing = false;
                        this.test = None;
                        this.message = Some(format!("Reading {} now.", machine.name));
                    }
                    Err(err) => this.message = Some(message_of(&err)),
                }
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        let Some(reply) = self.engine.ask(|r| Request::Edit(Edit::RetryNow(r))) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            if let Ok(Err(err)) = reply.await {
                let _ = this.update(cx, |this, cx| {
                    this.message = Some(message_of(&err));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Opens the form with the machine as it is set up.
    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = true;
        self.filled = false;
        self.fill(window, cx);
        cx.notify();
    }

    fn fill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filled {
            return;
        }
        let Some(machine) = self.live.read(cx).state.machine.clone() else {
            return;
        };
        self.filled = true;
        self.demo = machine.demo;
        let address = machine
            .address
            .strip_prefix("tcp://")
            .unwrap_or(&machine.address)
            .to_owned();
        let name = if machine.demo {
            String::new()
        } else {
            machine.name
        };
        self.name.update(cx, |i, cx| i.set_value(name, window, cx));
        self.address
            .update(cx, |i, cx| i.set_value(address, window, cx));
        if let Some((id, _)) = MODELS.iter().find(|(id, _)| *id == machine.model) {
            self.model_id = id;
        }
        let id = self.model_id;
        self.model
            .update(cx, |s, cx| s.set_selected_value(&id, window, cx));
    }

    #[allow(clippy::too_many_lines)] // one form, read top to bottom
    fn setup_form(&self, recording: bool, set_up: bool, cx: &mut Context<Self>) -> AnyElement {
        let field = |name: &'static str, widget: AnyElement| {
            div()
                .flex_1()
                .min_w(px(220.))
                .flex()
                .flex_col()
                .gap_1()
                .child(label(name))
                .child(widget)
        };
        let demo = self.demo;
        let test = match &self.test {
            None if self.testing => Some(
                div()
                    .text_sm()
                    .text_color(white(0.6))
                    .child("Reading the machine…")
                    .into_any_element(),
            ),
            None => None,
            Some(Err(advice)) => Some(
                div()
                    .id("test-failed")
                    .test_support()
                    .text_sm()
                    .text_color(rgb(0x00f8_7171))
                    .child(advice.clone())
                    .into_any_element(),
            ),
            Some(Ok(test)) => Some(
                div()
                    .id("test-passed")
                    .test_support()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().text_color(chart::ror()).child(format!(
                        "It answers ({} ms). Readings now:",
                        test.latency_ms
                    )))
                    .child(readings(test.values.iter().map(|(k, v)| (k.as_str(), *v))))
                    .into_any_element(),
            ),
        };
        div()
            .id("machine-setup")
            .test_support()
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .rounded_lg()
            .border_1()
            .border_color(white(0.1))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(if set_up {
                "Change the machine"
            } else {
                "Set up the machine"
            }))
            .child(
                div()
                    .text_sm()
                    .text_color(white(0.55))
                    .child("Type the address shown on the machine's network settings. Roastro only reads it."),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("kind-machine")
                            .label("A machine on the network")
                            .when(!demo, ButtonVariants::primary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.demo = false;
                                this.test = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("kind-demo")
                            .label("Demo (built-in simulator)")
                            .when(demo, ButtonVariants::primary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.demo = true;
                                this.test = None;
                                cx.notify();
                            })),
                    ),
            )
            .children((!demo).then(|| {
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(field(
                        "Name",
                        Input::new(&self.name).id("machine-name").into_any_element(),
                    ))
                    .child(field(
                        "Address",
                        Input::new(&self.address)
                            .id("machine-address")
                            .into_any_element(),
                    ))
                    .child(field(
                        "Model",
                        Select::new(&self.model).placeholder("Choose the model").into_any_element(),
                    ))
            }))
            .children(demo.then(|| {
                div()
                    .text_sm()
                    .text_color(white(0.55))
                    .child("The Demo machine roasts on its own, for training and showing the app. Its roasts stay out of the real history.")
            }))
            .children(test)
            .children(recording.then(|| {
                div()
                    .text_sm()
                    .text_color(chart::bt())
                    .child("A roast is recording: finish it before changing the machine.")
            }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("test-machine")
                            .label("Test connection")
                            .disabled(self.testing)
                            .on_click(cx.listener(|this, _, _, cx| this.run_test(cx))),
                    )
                    .child(
                        Button::new("save-machine")
                            .primary()
                            .label("Save and connect")
                            .disabled(recording)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .children(set_up.then(|| {
                        Button::new("cancel-machine").label("Cancel").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.editing = false;
                                this.test = None;
                                cx.notify();
                            },
                        ))
                    })),
            )
            .into_any_element()
    }
}

fn message_of(err: &roastro::api::ApiError) -> String {
    let text = err.to_string();
    // "code: message": people read the message.
    text.split_once(": ")
        .map_or(text.clone(), |(_, m)| m.to_owned())
}

/// Readings as a two-column table, known points first.
fn readings<'a>(values: impl Iterator<Item = (&'a str, f64)>) -> impl IntoElement {
    let order = ["bt", "et", "gas_fb", "gas_cmd", "fan", "drum"];
    let mut rows: Vec<(&str, f64)> = values.collect();
    rows.sort_by_key(|(k, _)| order.iter().position(|o| o == k).unwrap_or(order.len()));
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(rows.into_iter().map(|(point, v)| {
            let (name, unit) = reading_name(point);
            div()
                .flex()
                .gap_4()
                .text_sm()
                .child(
                    div()
                        .w(px(160.))
                        .text_color(white(0.6))
                        .child(name.to_owned()),
                )
                .child(
                    div()
                        .w(px(90.))
                        .text_color(white(0.35))
                        .child(point.to_owned()),
                )
                .child(div().child(format!("{v:.1} {unit}")))
        }))
}

impl Render for MachinePage {
    #[allow(clippy::too_many_lines)] // the page's layout, read top to bottom
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let st = self.live.read(cx).state.clone();
        let not_set_up = st
            .link
            .as_ref()
            .is_some_and(|l| l.state == LinkState::NotSetUp);
        if not_set_up && !self.editing {
            self.editing = true;
        }
        if self.editing {
            self.fill(window, cx);
        }
        let recording = st.recording();
        let (state, color) = link_words(&st);
        let machine = st.machine.clone();
        let link = st.link.clone();
        let down = link
            .as_ref()
            .is_some_and(|l| matches!(l.state, LinkState::Down | LinkState::Connecting));
        let values: Vec<(String, f64)> = if st.connected() {
            st.last()
                .map(|s| s.values.iter().map(|(k, v)| (k.clone(), *v)).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let form = self
            .editing
            .then(|| self.setup_form(recording, !not_set_up, cx));
        let drops = self.drops.clone();
        div()
            .id("page-machine")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_4()
            .px_6()
            .py_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_2xl().font_weight(FontWeight::BOLD).child("Machine"))
                    .child(
                        div()
                            .id("link-state")
                            .test_support()
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(chart::faded(color, 0.15))
                            .text_sm()
                            .text_color(color)
                            .child(state),
                    ),
            )
            .children(self.message.clone().map(|m| {
                div()
                    .id("machine-message")
                    .text_sm()
                    .text_color(white(0.7))
                    .child(m)
            }))
            .children(form)
            .children((!not_set_up).then(|| {
                let machine = machine.clone();
                div()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .child(
                        div()
                            .w(px(420.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_5()
                            .rounded_lg()
                            .border_1()
                            .border_color(white(0.08))
                            .children(machine.map(|m| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(m.name))
                                    .child(div().text_sm().text_color(white(0.6)).child(if m.demo {
                                        "Built-in simulator".to_owned()
                                    } else {
                                        format!("{} · Modbus TCP", m.address)
                                    }))
                                    .child(div().text_sm().text_color(white(0.6)).child(m.model_title))
                            }))
                            .children(link.as_ref().and_then(|l| l.error.clone()).map(|e| {
                                div().pt_2().text_sm().text_color(rgb(0x00f8_7171)).child(e)
                            }))
                            .children(link.as_ref().and_then(|l| l.advice.clone()).map(|a| {
                                div()
                                    .id("link-advice")
                                    .test_support()
                                    .text_sm()
                                    .text_color(white(0.75))
                                    .child(a)
                            }))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .pt_2()
                                    .children(down.then(|| {
                                        Button::new("retry-now")
                                            .primary()
                                            .label("Retry now")
                                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx)))
                                    }))
                                    .child(
                                        Button::new("edit-machine")
                                            .label("Edit")
                                            .disabled(recording)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.edit(window, cx);
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .pt_2()
                                    .text_xs()
                                    .text_color(white(0.4))
                                    .child("Roastro only reads the machine. Gas, air and drum are set on its own panel."),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(360.))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .p_5()
                            .rounded_lg()
                            .border_1()
                            .border_color(white(0.08))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .child(label("Now"))
                                    .children(recording.then(|| {
                                        Button::new("machine-open-live")
                                            .label("Open the live roast")
                                            .on_click(cx.listener(|_, _, _, cx| cx.emit(GoLive)))
                                    })),
                            )
                            .child(if values.is_empty() {
                                div()
                                    .text_sm()
                                    .text_color(white(0.45))
                                    .child("No readings while the machine does not answer.")
                                    .into_any_element()
                            } else {
                                readings(values.iter().map(|(k, v)| (k.as_str(), *v))).into_any_element()
                            }),
                    )
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(label("Link drops"))
                    .children(drops.iter().map(|d| {
                        #[allow(clippy::cast_precision_loss)] // seconds of a day
                        let lasted = d
                            .ended_at
                            .map_or_else(|| "still down".to_owned(), |end| {
                                format!("lasted {}", clock((end - d.started_at) as f64 / 1000.0))
                            });
                        let roast = d
                            .roast_number
                            .map_or_else(String::new, |n| format!(" · during Roast #{n}"));
                        div()
                            .flex()
                            .gap_4()
                            .text_sm()
                            .child(div().w(px(220.)).child(format!("{} {}", day(d.started_at), crate::roasts::time_of_day(d.started_at))))
                            .child(div().w(px(160.)).text_color(white(0.7)).child(format!("{lasted}{roast}")))
                            .child(div().flex_1().text_color(white(0.45)).child(d.error.clone()))
                    }))
                    .children(drops.is_empty().then(|| {
                        div()
                            .text_sm()
                            .text_color(white(0.45))
                            .child("No drops: the machine answered every time.")
                    })),
            )
    }
}
