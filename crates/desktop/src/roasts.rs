//! Roasts: find a roast ("Roast history and curve comparison", #10). Newest
//! first, grouped by day, narrowed by coffee, date and a search that also
//! reads the notes. Opening a roast is the shell's job: this page only says
//! which one.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, Hsla, IntoElement, SharedString, Subscription,
    Window, div, px, rgb,
};
use roast::EventKind;
use roastro::engine::{Edit, Request};
use roastro::wire::{CoffeeOut, RoastDoc, RoastQueryIn};
use uuid::Uuid;

use crate::chart;
use crate::state::Millis;
use crate::view::next_roast::Choice;
use crate::view::rail::label;
use crate::view::{Engine, clock, fmt1, white};

/// A `RoR` fall this large within 30 s is flagged in the list (°C/min).
const ROR_FALL_FLAG: f64 = 5.0;

const DAY_MS: Millis = 24 * 60 * 60 * 1000;

/// The shell opens this roast.
pub struct OpenRoast(pub Uuid);

/// How far back the list looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Range {
    Week,
    Month,
    All,
}

impl Range {
    fn since(self, now: Millis) -> Option<Millis> {
        match self {
            Self::Week => Some(now - 7 * DAY_MS),
            Self::Month => Some(now - 30 * DAY_MS),
            Self::All => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Week => "Last 7 days",
            Self::Month => "Last 30 days",
            Self::All => "All time",
        }
    }
}

/// The Roasts page.
pub struct RoastsPage {
    engine: Engine,
    time_scale: f64,
    coffee: Entity<SelectState<Vec<Choice>>>,
    search: Entity<InputState>,
    range: Range,
    coffee_id: Option<Uuid>,
    roasts: Vec<RoastDoc>,
    next_before: Option<Uuid>,
    /// Bumped on every new query, so a slow reply to an old one is dropped.
    generation: u64,
    coffees_stale: Option<Vec<CoffeeOut>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OpenRoast> for RoastsPage {}

impl RoastsPage {
    pub fn new(
        engine: Engine,
        time_scale: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let coffee = cx.new(|cx| SelectState::new(Vec::<Choice>::new(), None, window, cx));
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search a number, coffee or note"));
        let subscriptions = vec![
            cx.subscribe_in(
                &coffee,
                window,
                |this, _, event: &SelectEvent<Vec<Choice>>, _, cx| {
                    if let SelectEvent::Confirm(Some(coffee)) = event {
                        this.coffee_id = *coffee;
                        this.reload(cx);
                    }
                },
            ),
            cx.subscribe_in(&search, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.reload(cx);
                }
            }),
        ];
        let page = Self {
            engine,
            time_scale,
            coffee,
            search,
            range: Range::All,
            coffee_id: None,
            roasts: Vec::new(),
            next_before: None,
            generation: 0,
            coffees_stale: None,
            _subscriptions: subscriptions,
        };
        page.load_coffees(cx);
        page
    }

    fn query(&self, before: Option<Uuid>, cx: &Context<Self>) -> RoastQueryIn {
        let text = self.search.read(cx).value().trim().to_owned();
        RoastQueryIn {
            before,
            coffee: self.coffee_id,
            since: self.range.since(roastro::now_ms()),
            q: (!text.is_empty()).then_some(text),
            ..RoastQueryIn::default()
        }
    }

    /// Lists from the newest again, as the filters say. Called when a filter
    /// changes and when a roast finishes.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.fetch(None, cx);
    }

    fn more(&mut self, cx: &mut Context<Self>) {
        if let Some(before) = self.next_before {
            self.fetch(Some(before), cx);
        }
    }

    fn fetch(&self, before: Option<Uuid>, cx: &mut Context<Self>) {
        let query = self.query(before, cx);
        let generation = self.generation;
        let Some(reply) = self.engine.ask(|r| Request::Edit(Edit::Roasts(query, r))) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let page = reply.await.ok()?.ok()?;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if before.is_none() {
                    this.roasts.clear();
                }
                this.roasts.extend(page.roasts);
                this.next_before = page.next_before;
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn load_coffees(&self, cx: &mut Context<Self>) {
        let Some(reply) = self.engine.ask(|r| Request::Edit(Edit::Coffees(true, r))) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let coffees = reply.await.ok()?.ok()?;
            this.update(cx, |this, cx| {
                this.coffees_stale = Some(coffees);
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn set_range(&mut self, range: Range, cx: &mut Context<Self>) {
        if self.range != range {
            self.range = range;
            self.reload(cx);
        }
    }

    /// Roast seconds between two instants of a roast.
    fn span(&self, from: Millis, to: Millis) -> f64 {
        #[allow(clippy::cast_precision_loss)] // minutes in milliseconds
        let ms = (to - from) as f64;
        ms * self.time_scale / 1000.0
    }

    fn row(&self, r: &RoastDoc, cx: &mut Context<Self>) -> impl IntoElement {
        let at = |kind: EventKind| {
            r.events
                .iter()
                .find(|e| e.kind == kind.as_str())
                .map(|e| e.at)
        };
        let charge = r.charge_at;
        let since_charge = |kind| {
            charge
                .zip(at(kind))
                .map_or_else(|| "—".to_owned(), |(c, t)| clock(self.span(c, t)))
        };
        let total = charge
            .zip(at(EventKind::Drop))
            .map_or_else(|| "—".to_owned(), |(c, d)| clock(self.span(c, d)));
        let dtr = r
            .metrics
            .development_time_ratio
            .map_or_else(|| "—".to_owned(), |d| format!("{:.1}%", d * 100.0));
        let loss = r
            .metrics
            .weight_loss_ratio
            .map_or_else(|| "—".to_owned(), |l| format!("{:.1}%", l * 100.0));
        let summary = r.summary.as_ref();
        let fall = summary.and_then(|s| s.ror_fall);
        let id = r.id;
        let cell = |text: String, w: f32| div().w(px(w)).flex().justify_end().child(text);
        div()
            .id(SharedString::from(format!("roast-{}", r.number)))
            .test_support()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|d| d.bg(white(0.03)))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenRoast(id))))
            .child(
                div()
                    .w(px(48.))
                    .text_sm()
                    .text_color(white(0.5))
                    .child(charge.map(time_of_day).unwrap_or_default()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("Roast #{}", r.number)),
                            )
                            .children((r.state == "incomplete").then(|| {
                                div().text_xs().text_color(white(0.45)).child("incomplete")
                            })),
                    )
                    .child(
                        div().text_sm().text_color(white(0.55)).child(
                            r.coffee
                                .as_ref()
                                .map_or_else(|| "No coffee set".to_owned(), |c| c.name.clone()),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .text_sm()
                    .child(cell(total, 52.))
                    .child(cell(since_charge(EventKind::FirstCrack), 52.))
                    .child(cell(dtr, 60.))
                    .child(cell(
                        format!("{}°", fmt1(summary.and_then(|s| s.drop_bt))),
                        64.,
                    ))
                    .child(cell(loss, 56.))
                    .child(
                        div()
                            .w(px(64.))
                            .flex()
                            .justify_end()
                            .text_color(fall_color(fall))
                            .child(fall.map_or_else(|| "—".to_owned(), |f| format!("{f:.1}"))),
                    ),
            )
    }

    fn refresh_coffees(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(coffees) = self.coffees_stale.take() else {
            return;
        };
        let mut choices = vec![Choice {
            value: None,
            title: "All coffees".into(),
        }];
        choices.extend(coffees.into_iter().map(|c| Choice {
            value: Some(c.id),
            title: c.name.into(),
        }));
        let selected = self.coffee_id;
        self.coffee.update(cx, |select, cx| {
            select.set_items(choices, window, cx);
            select.set_selected_value(&selected, window, cx);
        });
    }
}

fn fall_color(fall: Option<f64>) -> Hsla {
    if fall.is_some_and(|f| f >= ROR_FALL_FLAG) {
        rgb(0x00f8_7171).into()
    } else {
        white(0.6)
    }
}

/// "08:05" in the computer's time zone.
fn time_of_day(at: Millis) -> String {
    jiff::Timestamp::from_millisecond(at)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// "Mon 16 Nov 2026" in the computer's time zone.
pub fn day(at: Millis) -> String {
    jiff::Timestamp::from_millisecond(at)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%a %-d %b %Y")
                .to_string()
        })
        .unwrap_or_default()
}

impl Render for RoastsPage {
    #[allow(clippy::too_many_lines)] // the page's layout, read top to bottom
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_coffees(window, cx);
        let mut list: Vec<gpui_kit::AnyElement> = Vec::new();
        let mut last_day = String::new();
        for r in &self.roasts {
            let d = r.charge_at.map(day).unwrap_or_default();
            if d != last_day {
                list.push(
                    div()
                        .px_4()
                        .pt_4()
                        .pb_1()
                        .child(label(d.clone()))
                        .into_any_element(),
                );
                last_day = d;
            }
            list.push(self.row(r, cx).into_any_element());
        }
        let empty = self.roasts.is_empty();
        let header =
            |text: &'static str, w: f32| div().w(px(w)).flex().justify_end().child(label(text));
        div()
            .id("page-roasts")
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
                            .child("Roasts"),
                    )
                    .child(
                        div()
                            .w(px(260.))
                            .child(Select::new(&self.coffee).placeholder("All coffees")),
                    )
                    .children([Range::Week, Range::Month, Range::All].map(|range| {
                        let on = self.range == range;
                        div()
                            .id(SharedString::from(
                                format!("range-{range:?}").to_lowercase(),
                            ))
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
                            .on_click(cx.listener(move |this, _, _, cx| this.set_range(range, cx)))
                            .child(range.label())
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(240.))
                            .child(Input::new(&self.search).id("roast-search")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .px_4()
                    .pt_3()
                    .child(div().w(px(48.)))
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(header("Total", 52.))
                            .child(header("FC", 52.))
                            .child(header("DTR", 60.))
                            .child(header("Drop BT", 64.))
                            .child(header("Loss", 56.))
                            .child(header("RoR fall", 64.)),
                    ),
            )
            .child(
                div()
                    .id("roast-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_6()
                    .children(list)
                    .children(empty.then(|| {
                        div()
                            .p_6()
                            .text_color(white(0.45))
                            .child("No roasts match. Change the coffee, the dates or the search.")
                    }))
                    .children(self.next_before.map(|_| {
                        div().p_4().child(
                            Button::new("more-roasts")
                                .label("Older roasts")
                                .on_click(cx.listener(|this, _, _, cx| this.more(cx))),
                        )
                    })),
            )
    }
}
