//! New roast: prepare the Next roast ("Roast history and curve comparison",
//! #10): the coffee, green weight, who roasts and the reference, with a
//! preview of the reference's curve. Every change is stored at once, so an
//! automatically detected Charge still takes it into the roast. The live
//! screen only shows a summary of it.

use i18n::{t, tf};
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, IntoElement, SharedString, Subscription,
    Window, div, px,
};
use roastro::RoastId;
use roastro::engine::{Edit, Request};
use roastro::wire::{CoffeeIn, CoffeeOut, NextRoastIn, NextRoastOut, RoastDoc, RoastQueryIn};
use uuid::Uuid;

use crate::chart;
use crate::roast_detail::rows;
use crate::state::LiveState;
use crate::view::rail::{Size, label, summary_figures};
use crate::view::{Engine, RoastScreen, grams, kilograms, white};

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

/// The shell goes to the live roast.
pub struct GoLive;

type Picker = Entity<SelectState<Vec<Choice>>>;

/// The New roast page.
pub struct NewRoastPage {
    engine: Engine,
    /// Where the Next roast arrives, on the live stream.
    live: Entity<RoastScreen>,
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
    /// The Next roast seen last on the live stream.
    seen: Option<NextRoastOut>,
    /// A coffee was just chosen: take the reference the engine suggests.
    follow_suggestion: bool,
    was_recording: bool,
    /// The reference's curve and figures, once read.
    preview: Option<LiveState>,
    preview_for: Option<Uuid>,
    time_scale: f64,
    message: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GoLive> for NewRoastPage {}

/// Enter, or leaving the field.
fn done_typing(event: &InputEvent) -> bool {
    matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
}

/// "12 Nov", in the computer's time zone.
fn date(at: i64) -> String {
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
        title: t("No coffee").into(),
    }];
    choices.extend(coffees.iter().map(|c| Choice {
        value: Some(c.id),
        title: if c.lot.is_empty() {
            c.name.clone().into()
        } else {
            tf("{name} · lot {lot}", &[("name", &c.name), ("lot", &c.lot)]).into()
        },
    }));
    choices
}

/// The reference options: done roasts, newest first, the suggested one
/// marked; then none.
fn reference_choices(roasts: &[RoastDoc], suggested: Option<Uuid>) -> Vec<Choice> {
    let mut choices: Vec<Choice> = roasts
        .iter()
        .map(|r| {
            let mut parts = vec![tf("Roast #{n}", &[("n", &r.number)])];
            parts.extend(r.coffee.as_ref().map(|c| c.name.clone()));
            parts.extend(r.charge_at.map(date));
            let mut title = parts.join(" · ");
            if Some(r.id) == suggested {
                title.push_str(t(" (latest of this coffee)"));
            }
            Choice {
                value: Some(r.id),
                title: title.into(),
            }
        })
        .collect();
    choices.push(Choice {
        value: None,
        title: t("No reference").into(),
    });
    choices
}

impl NewRoastPage {
    pub fn new(
        engine: Engine,
        live: Entity<RoastScreen>,
        time_scale: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = |window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| SelectState::new(Vec::<Choice>::new(), None, window, cx))
        };
        let coffee = picker(window, cx);
        let reference = picker(window, cx);
        let green = cx.new(|cx| InputState::new(window, cx).placeholder("0.000"));
        let roasted_by = cx.new(|cx| InputState::new(window, cx));
        let new_coffee = cx.new(|cx| InputState::new(window, cx).placeholder(t("New coffee")));
        let subscriptions = vec![
            cx.observe(&live, |this, live, cx| this.live_changed(&live, cx)),
            cx.subscribe_in(
                &coffee,
                window,
                |this, _, event: &SelectEvent<Vec<Choice>>, _, cx| {
                    if let SelectEvent::Confirm(Some(coffee)) = event {
                        this.choose_coffee(*coffee, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &reference,
                window,
                |this, _, event: &SelectEvent<Vec<Choice>>, _, cx| {
                    if let SelectEvent::Confirm(Some(reference)) = event {
                        let reference = *reference;
                        this.put_next(|n| n.reference_id = reference, cx);
                    }
                },
            ),
            cx.subscribe_in(&green, window, |this, _, event: &InputEvent, _, cx| {
                if done_typing(event) {
                    this.save_green(cx);
                }
            }),
            cx.subscribe_in(&roasted_by, window, |this, _, event: &InputEvent, _, cx| {
                if done_typing(event) {
                    let name = this.roasted_by.read(cx).value().trim().to_owned();
                    if this.next(cx).map(|n| n.roasted_by) != Some(name.clone()) {
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
        let page = Self {
            engine,
            live,
            coffee,
            reference,
            green,
            roasted_by,
            new_coffee,
            coffees: Vec::new(),
            roasts: Vec::new(),
            stale: true,
            shown: None,
            seen: None,
            follow_suggestion: false,
            was_recording: false,
            preview: None,
            preview_for: None,
            time_scale,
            message: None,
            _subscriptions: subscriptions,
        };
        page.load_choices(cx);
        page
    }

    fn next(&self, cx: &Context<Self>) -> Option<NextRoastOut> {
        self.live.read(cx).state.next.clone()
    }

    /// A new Next roast after choosing a coffee brings its suggested
    /// reference; a roast starting or finishing changes the lists.
    fn live_changed(&mut self, live: &Entity<RoastScreen>, cx: &mut Context<Self>) {
        let st = &live.read(cx).state;
        let next = st.next.clone();
        let recording = st.recording();
        if next != self.seen {
            if let Some(n) = &next
                && n.reference_id.is_none()
                && let Some(suggested) = n.suggested_reference_id
                && std::mem::take(&mut self.follow_suggestion)
            {
                self.put_next(|n| n.reference_id = Some(suggested), cx);
            }
            self.seen = next;
            cx.notify();
        }
        if recording != self.was_recording {
            self.was_recording = recording;
            self.load_choices(cx);
        }
    }

    /// Reads the coffees and the done roasts to choose from.
    pub fn load_choices(&self, cx: &mut Context<Self>) {
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
            this.update(cx, |this, cx| {
                this.coffees = coffees;
                this.roasts = page
                    .roasts
                    .into_iter()
                    .filter(|r| r.state == "done")
                    .collect();
                this.stale = true;
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    /// Stores the Next roast with one change.
    fn put_next(&mut self, change: impl FnOnce(&mut NextRoastIn), cx: &mut Context<Self>) {
        let current = self.next(cx);
        let mut next = NextRoastIn {
            coffee_id: current.as_ref().and_then(|n| n.coffee_id),
            green_weight_g: current.as_ref().and_then(|n| n.green_weight_g),
            roasted_by: current
                .as_ref()
                .map(|n| n.roasted_by.clone())
                .unwrap_or_default(),
            reference_id: current.as_ref().and_then(|n| n.reference_id),
        };
        change(&mut next);
        self.message = None;
        let Some(reply) = self
            .engine
            .ask(|r| Request::Edit(Edit::PutNextRoast(next, r)))
        else {
            self.message = Some(t("Roastro is busy; try again").into());
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            if let Ok(Err(err)) = reply.await {
                let _ = this.update(cx, |this, cx| {
                    this.message = Some(err.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// A new coffee clears the reference, then takes the suggested one.
    pub fn choose_coffee(&mut self, coffee: Option<Uuid>, cx: &mut Context<Self>) {
        if self.next(cx).and_then(|n| n.coffee_id) == coffee {
            return;
        }
        self.follow_suggestion = coffee.is_some();
        self.put_next(
            |n| {
                n.coffee_id = coffee;
                n.reference_id = None;
            },
            cx,
        );
    }

    fn save_green(&mut self, cx: &mut Context<Self>) {
        let text = self.green.read(cx).value().to_string();
        let Some(green) = grams(&text) else {
            self.message = Some(t("green weight: kilograms, like 5.000").into());
            cx.notify();
            return;
        };
        if self.next(cx).and_then(|n| n.green_weight_g) != green {
            self.put_next(|n| n.green_weight_g = green, cx);
        }
    }

    fn add_coffee(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_coffee.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        self.new_coffee
            .update(cx, |input, cx| input.set_value("", window, cx));
        let body = CoffeeIn {
            name,
            ..CoffeeIn::default()
        };
        let Some(reply) = self.engine.ask(|r| Request::Edit(Edit::AddCoffee(body, r))) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = reply.await.ok()?;
            this.update(cx, |this, cx| {
                match result {
                    Ok(coffee) => {
                        this.choose_coffee(Some(coffee.id), cx);
                        this.load_choices(cx);
                    }
                    Err(err) => this.message = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    /// Reads the reference's curve when it changes.
    fn sync_preview(&mut self, reference: Option<Uuid>, cx: &mut Context<Self>) {
        if self.preview_for == reference {
            return;
        }
        self.preview_for = reference;
        self.preview = None;
        let Some(id) = reference else {
            return;
        };
        let engine = self.engine.clone();
        let time_scale = self.time_scale;
        cx.spawn(async move |this, cx| {
            let rid = RoastId(id);
            let doc = engine.ask(|r| Request::Roast(rid, r))?.await.ok()?.ok()?;
            let columns = engine.ask(|r| Request::Samples(rid, r))?.await.ok()?.ok()?;
            this.update(cx, |this, cx| {
                if this.preview_for == Some(id) {
                    let mut st = LiveState::new(time_scale);
                    st.samples = rows(&columns);
                    st.roast = Some(doc);
                    this.preview = Some(st);
                    cx.notify();
                }
            })
            .ok()
        })
        .detach();
    }

    /// Brings the widgets in line with the lists and the stored Next roast,
    /// leaving alone what the operator is typing.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.next(cx);
        self.sync_preview(next.as_ref().and_then(|n| n.reference_id), cx);
        if !self.stale && self.shown == next {
            return;
        }
        let suggested = next.as_ref().and_then(|n| n.suggested_reference_id);
        let coffees = coffee_choices(&self.coffees);
        let references = reference_choices(&self.roasts, suggested);
        let coffee = next.as_ref().and_then(|n| n.coffee_id);
        let reference = next.as_ref().and_then(|n| n.reference_id);
        self.coffee.update(cx, |select, cx| {
            select.set_items(coffees, window, cx);
            select.set_selected_value(&coffee, window, cx);
        });
        self.reference.update(cx, |select, cx| {
            select.set_items(references, window, cx);
            select.set_selected_value(&reference, window, cx);
        });
        let green = next.as_ref().and_then(|n| n.green_weight_g);
        if grams(&self.green.read(cx).value()) != Some(green) {
            let text = green.map(kilograms).unwrap_or_default();
            self.green
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
        let by = next
            .as_ref()
            .map(|n| n.roasted_by.clone())
            .unwrap_or_default();
        if self.roasted_by.read(cx).value().trim() != by {
            self.roasted_by
                .update(cx, |input, cx| input.set_value(by, window, cx));
        }
        self.stale = false;
        self.shown = next;
    }

    fn preview_card(&self) -> AnyElement {
        let Some(st) = &self.preview else {
            let text = if self.preview_for.is_some() {
                t("Reading the reference…")
            } else {
                t("No reference: the live chart has nothing behind the curve.")
            };
            return div()
                .text_sm()
                .text_color(white(0.45))
                .child(text)
                .into_any_element();
        };
        let roast = st.roast.as_ref();
        let [total, dtr, _, drop_bt, loss] = summary_figures(st);
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(label(t("Reference preview")))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(roast.map_or_else(String::new, |r| {
                        let coffee = r
                            .coffee
                            .as_ref()
                            .map_or(t("No coffee set"), |c| c.name.as_str());
                        tf(
                            "Roast #{n} · {coffee}",
                            &[("n", &r.number), ("coffee", &coffee)],
                        )
                    })),
            )
            .child(div().h(px(240.)).w_full().child(chart::chart(
                st,
                Rc::new(Cell::new(None)),
                None,
            )))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .child(total.render(Size::Medium))
                    .child(dtr.render(Size::Medium))
                    .child(drop_bt.render(Size::Medium))
                    .child(loss.render(Size::Medium)),
            )
            .into_any_element()
    }
}

impl Render for NewRoastPage {
    #[allow(clippy::too_many_lines)] // the page's layout, read top to bottom
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        let recording = self
            .live
            .read(cx)
            .state
            .roast
            .as_ref()
            .filter(|r| r.state == "recording")
            .map(|r| r.number);
        let field = |name: &'static str, widget: AnyElement| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(label(name))
                .child(widget)
        };
        div()
            .id("page-new-roast")
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
                    .flex_wrap()
                    .items_baseline()
                    .gap_3()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .child(t("New roast")),
                    )
                    .child(div().text_color(white(0.55)).child(t(
                        "What the next Charge takes into the roast. Saved as you go.",
                    ))),
            )
            .children(recording.map(|n| {
                div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(chart::faded(chart::bt(), 0.45))
                    .child(tf(
                        "Roast #{n} is recording now. This prepares the roast after it.",
                        &[("n", &n)],
                    ))
            }))
            .children(self.message.clone().map(|m| {
                div()
                    .id("new-roast-message")
                    .text_sm()
                    .text_color(white(0.7))
                    .child(m)
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_8()
                    .child(
                        div()
                            .w(px(520.))
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(field(
                                t("Coffee"),
                                Select::new(&self.coffee)
                                    .placeholder(t("Choose a coffee"))
                                    .into_any_element(),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(Input::new(&self.new_coffee).id("new-coffee").flex_1())
                                    .child(
                                        Button::new("add-coffee").label(t("Add coffee")).on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.add_coffee(window, cx);
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .child(field(
                                        t("Green weight (kg)"),
                                        Input::new(&self.green)
                                            .id("green-weight")
                                            .into_any_element(),
                                    ))
                                    .child(field(
                                        t("Roasted by"),
                                        Input::new(&self.roasted_by)
                                            .id("roasted-by")
                                            .into_any_element(),
                                    )),
                            )
                            .child(field(
                                t("Reference roast"),
                                Select::new(&self.reference)
                                    .placeholder(t("No reference"))
                                    .into_any_element(),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        Button::new("go-live")
                                            .primary()
                                            .label(t("Go to the live roast"))
                                            .on_click(cx.listener(|_, _, _, cx| cx.emit(GoLive))),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(white(0.4))
                                            .child(t("Charge is detected automatically.")),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(360.))
                            .p_4()
                            .rounded_lg()
                            .border_1()
                            .border_color(white(0.08))
                            .child(self.preview_card()),
                    ),
            )
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size,
    };
    use roastro::engine::{Edit, Request};
    use roastro::wire::{Live, NextRoastOut};
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use super::NewRoastPage;
    use crate::view::{Engine, RoastScreen};

    /// The next Next roast stored, skipping the lists the page reads.
    fn stored(rx: &mut mpsc::Receiver<Request>) -> Option<roastro::wire::NextRoastIn> {
        while let Ok(req) = rx.try_recv() {
            if let Request::Edit(Edit::PutNextRoast(next, _)) = req {
                return Some(next);
            }
        }
        None
    }

    #[gpui_kit::test]
    fn the_next_roast_is_stored_as_it_is_filled(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        // No animations in tests: every frame is the final one.
        cx.update(|cx| cx.set_reduce_motion(true));
        let (tx, mut rx) = mpsc::channel(64);
        let (handle, (live, page)) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1440.), px(900.)),
                })),
                ..Default::default()
            };
            let mut pair = None;
            let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
                let live = cx.new(|cx| RoastScreen::new(Engine(tx.clone()), 1.0, window, cx));
                let page =
                    cx.new(|cx| NewRoastPage::new(Engine(tx), live.clone(), 1.0, window, cx));
                pair = Some((live, page.clone()));
                page
            })
            .unwrap();
            (handle, pair.unwrap())
        });
        cx.run_until_parked();
        let coffee = Uuid::now_v7();
        let suggested = Uuid::now_v7();
        cx.update(|cx| page.update(cx, |p, cx| p.choose_coffee(Some(coffee), cx)));
        cx.run_until_parked();
        let next = stored(&mut rx).expect("choosing a coffee stores it");
        assert_eq!((next.coffee_id, next.reference_id), (Some(coffee), None));

        let arrived = NextRoastOut {
            coffee_id: Some(coffee),
            green_weight_g: None,
            roasted_by: String::new(),
            reference_id: None,
            suggested_reference_id: Some(suggested),
            coffee_name: None,
            reference_number: None,
        };
        cx.update(|cx| {
            live.update(cx, |s, cx| s.apply(&Live::NextRoast(arrived.clone()), cx));
        });
        cx.run_until_parked();
        let next = stored(&mut rx).expect("the suggested reference is taken");
        assert_eq!(next.reference_id, Some(suggested));

        let arrived = NextRoastOut {
            reference_id: Some(suggested),
            ..arrived
        };
        cx.update(|cx| live.update(cx, |s, cx| s.apply(&Live::NextRoast(arrived), cx)));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("green-weight", cx);
            window.input("5", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let next = stored(&mut rx).expect("the green weight is stored");
        assert_eq!(
            (next.coffee_id, next.green_weight_g, next.reference_id),
            (Some(coffee), Some(5000), Some(suggested)),
            "one change, the rest kept"
        );
    }
}
