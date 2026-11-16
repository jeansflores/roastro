//! The app around the pages ("Roast history and curve comparison", #10): a
//! sidebar with the navigation, the roast in progress and the machine's state,
//! collapsed to an icon rail on Live roast so the live roast has the screen.

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, FontWeight, Hsla, IntoElement, SharedString, Subscription, Window, div, px,
    rgb,
};
use i18n::{t, tf};

use roastro::alarm::Alarm;
use roastro::wire::LinkState;
use uuid::Uuid;

use crate::chart;
use crate::coffees::{CoffeePage, CoffeesPage, OpenCoffee, RoastItNext, ShowRoasts};
use crate::compare::{ComparePage, CompareRoasts};
use crate::machine_page::{MachinePage, Restored, link_words};
use crate::new_roast::{GoLive, NewRoastPage};
use crate::roast_detail::{Back, RoastDetail};
use crate::roasts::{OpenRoast, RoastsPage};
use crate::state::LiveState;
use crate::view::{Engine, OpenNewRoast, RoastScreen, clock, fmt1, white};

/// Where the app is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Live,
    Roasts,
    /// One roast, opened from Roasts or after its Drop.
    Roast,
    /// Several roasts side by side.
    Compare,
    Coffees,
    /// One coffee.
    Coffee,
    /// The Next roast, prepared.
    NewRoast,
    Machine,
}

impl Page {
    const NAV: [Self; 4] = [Self::Live, Self::Roasts, Self::Coffees, Self::Machine];

    fn title(self) -> &'static str {
        match self {
            Self::Live => t("Live roast"),
            Self::Roasts | Self::Roast | Self::Compare => t("Roasts"),
            Self::Coffees | Self::Coffee => t("Coffees"),
            Self::NewRoast => t("New roast"),
            Self::Machine => t("Machine"),
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Roasts | Self::Roast | Self::Compare => "roasts",
            Self::Coffees | Self::Coffee => "coffees",
            Self::NewRoast => "new-roast",
            Self::Machine => "machine",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Live => Icon::new(Lucide::Flame),
            Self::Roasts | Self::Roast | Self::Compare => Icon::new(Lucide::ChartSpline),
            Self::Coffees | Self::Coffee => Icon::new(Lucide::Coffee),
            Self::NewRoast => Icon::new(IconName::Plus),
            Self::Machine => Icon::new(IconName::Cpu),
        }
    }
}

/// The window's root: the sidebar and the page on show.
pub struct Shell {
    engine: Engine,
    time_scale: f64,
    page: Page,
    /// The live roast screen. It lives as long as the app, shown or not, so
    /// it keeps following the engine.
    live: Entity<RoastScreen>,
    /// The list keeps its filters while other pages show.
    roasts: Entity<RoastsPage>,
    /// The roast opened last.
    detail: Option<Entity<RoastDetail>>,
    /// The comparison opened last.
    compare: Option<Entity<ComparePage>>,
    coffees: Entity<CoffeesPage>,
    new_roast: Entity<NewRoastPage>,
    machine: Entity<MachinePage>,
    /// The first look at the link was taken: with no machine set up, the app
    /// opens on Machine.
    looked_at_setup: bool,
    /// Alarms on screen at the last frame: a new one rings the bell.
    alarms_shown: Vec<Alarm>,
    /// The coffee opened last, and what keeps its events heard.
    coffee: Option<(Entity<CoffeePage>, [Subscription; 3])>,
    /// Whether a roast was recording at the last look: when that changes,
    /// the list has a roast more.
    was_recording: bool,
    _subscriptions: Vec<Subscription>,
    detail_back: Option<Subscription>,
    detail_compare: Option<Subscription>,
    compare_open: Option<Subscription>,
}

impl Shell {
    pub fn new(
        engine: Engine,
        time_scale: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let live = cx.new(|cx| RoastScreen::new(engine.clone(), time_scale, window, cx));
        let roasts = cx.new(|cx| RoastsPage::new(engine.clone(), time_scale, window, cx));
        let coffees = cx.new(|cx| CoffeesPage::new(engine.clone(), cx));
        let new_roast =
            cx.new(|cx| NewRoastPage::new(engine.clone(), live.clone(), time_scale, window, cx));
        let machine = cx.new(|cx| MachinePage::new(engine.clone(), live.clone(), window, cx));
        roasts.update(cx, RoastsPage::reload);
        let subscriptions = vec![
            cx.observe(&live, |this, live, cx| {
                let st = &live.read(cx).state;
                if !this.looked_at_setup
                    && let Some(link) = &st.link
                {
                    this.looked_at_setup = true;
                    if link.state == LinkState::NotSetUp {
                        this.go(Page::Machine, cx);
                    }
                }
                let recording = live.read(cx).state.recording();
                if recording != this.was_recording {
                    this.was_recording = recording;
                    this.roasts.update(cx, RoastsPage::reload);
                }
                cx.notify();
            }),
            cx.subscribe_in(&roasts, window, |this, _, open: &OpenRoast, window, cx| {
                this.open(open.0, window, cx);
            }),
            cx.subscribe_in(&live, window, |this, _, open: &OpenRoast, window, cx| {
                this.open(open.0, window, cx);
            }),
            cx.subscribe(&live, |this, _, _: &OpenNewRoast, cx| {
                this.go(Page::NewRoast, cx);
            }),
            cx.subscribe(&machine, |this, _, _: &Restored, cx| {
                // Roasts, coffees and ids before the restore are gone.
                this.detail = None;
                this.compare = None;
                this.coffee = None;
                this.roasts.update(cx, RoastsPage::reload);
                this.coffees.update(cx, |c, cx| c.reload(cx));
                this.new_roast.update(cx, |n, cx| n.load_choices(cx));
            }),
            cx.subscribe(&machine, |this, _, _: &GoLive, cx| {
                this.go(Page::Live, cx);
            }),
            cx.subscribe(&new_roast, |this, _, _: &GoLive, cx| {
                this.go(Page::Live, cx);
            }),
            cx.subscribe_in(
                &coffees,
                window,
                |this, _, open: &OpenCoffee, window, cx| {
                    this.open_coffee(open.0, window, cx);
                },
            ),
            cx.subscribe_in(
                &roasts,
                window,
                |this, _, ids: &CompareRoasts, window, cx| {
                    this.compare(ids.0.clone(), window, cx);
                },
            ),
        ];
        Self {
            engine,
            time_scale,
            page: Page::Live,
            live,
            roasts,
            detail: None,
            compare: None,
            coffees,
            coffee: None,
            new_roast,
            machine,
            looked_at_setup: false,
            alarms_shown: Vec::new(),
            was_recording: false,
            _subscriptions: subscriptions,
            detail_back: None,
            detail_compare: None,
            compare_open: None,
        }
    }

    /// Opens one roast's page.
    fn open(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let time_scale = self.time_scale;
        let detail = cx.new(|cx| RoastDetail::new(engine, id, time_scale, window, cx));
        self.detail_back = Some(cx.subscribe(&detail, |this, _, _: &Back, cx| {
            this.roasts.update(cx, RoastsPage::reload);
            this.go(Page::Roasts, cx);
        }));
        self.detail_compare = Some(cx.subscribe_in(
            &detail,
            window,
            |this, _, ids: &CompareRoasts, window, cx| this.compare(ids.0.clone(), window, cx),
        ));
        self.detail = Some(detail);
        self.go(Page::Roast, cx);
    }

    /// Opens one coffee's page.
    fn open_coffee(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let time_scale = self.time_scale;
        let page = cx.new(|cx| CoffeePage::new(engine, id, time_scale, window, cx));
        let subscriptions = [
            cx.subscribe_in(&page, window, |this, _, open: &OpenRoast, window, cx| {
                this.open(open.0, window, cx);
            }),
            cx.subscribe(&page, |this, _, next: &RoastItNext, cx| {
                this.new_roast
                    .update(cx, |page, cx| page.choose_coffee(Some(next.0), cx));
                this.go(Page::NewRoast, cx);
            }),
            cx.subscribe(&page, |this, _, show: &ShowRoasts, cx| {
                this.roasts
                    .update(cx, |roasts, cx| roasts.show_coffee(show.0, cx));
                this.go(Page::Roasts, cx);
            }),
        ];
        self.coffee = Some((page, subscriptions));
        self.go(Page::Coffee, cx);
    }

    /// Opens the Compare page on these roasts.
    fn compare(&mut self, ids: Vec<Uuid>, window: &mut Window, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let time_scale = self.time_scale;
        let page = cx.new(|cx| ComparePage::new(engine, ids, time_scale, cx));
        self.compare_open = Some(cx.subscribe_in(
            &page,
            window,
            |this, _, open: &OpenRoast, window, cx| this.open(open.0, window, cx),
        ));
        self.compare = Some(page);
        self.go(Page::Compare, cx);
    }

    fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == Page::Machine {
            self.machine
                .update(cx, |machine, cx| machine.load_drops(cx));
        }
        if page == Page::Coffees {
            self.coffees.update(cx, |coffees, cx| coffees.reload(cx));
        }
        if self.page != page {
            self.page = page;
            cx.notify();
        }
    }

    /// The full sidebar, on every page but Live roast.
    fn sidebar(&self, st: &LiveState, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar")
            .test_support()
            .w(px(232.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .bg(rgb(0x0012_1317))
            .border_r_1()
            .border_color(white(0.08))
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(chart::bt())
                    .child(t("ROASTRO")),
            )
            .child(Self::live_card(st, cx))
            .child(
                Button::new("nav-new")
                    .primary()
                    .icon(IconName::Plus)
                    .label(t("New roast"))
                    .on_click(cx.listener(|this, _, _, cx| this.go(Page::NewRoast, cx))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(Page::NAV.map(|page| {
                        let on = self.page.id() == page.id();
                        div()
                            .id(SharedString::from(format!("nav-{}", page.id())))
                            .test_support()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(white(if on { 0.95 } else { 0.6 }))
                            .when(on, |d| d.bg(rgb(0x0018_1a1f)))
                            .hover(|d| d.text_color(white(0.95)))
                            .on_click(cx.listener(move |this, _, _, cx| this.go(page, cx)))
                            .child(page.icon().small())
                            .child(page.title())
                            .child(div().flex_1())
                            .children(
                                (page == Page::Live && st.recording())
                                    .then(|| div().size(px(7.)).rounded_full().bg(chart::bt())),
                            )
                    })),
            )
            .child(div().flex_1())
            .child(Self::machine_status(st, cx))
            .child(Self::languages(cx))
    }

    /// The icon rail that replaces the sidebar on Live roast.
    fn rail(&self, st: &LiveState, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-rail")
            .test_support()
            .w(px(56.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_3()
            .bg(rgb(0x0012_1317))
            .border_r_1()
            .border_color(white(0.08))
            .children(Page::NAV.map(|page| {
                let on = self.page.id() == page.id();
                div()
                    .id(SharedString::from(format!("rail-{}", page.id())))
                    .test_support()
                    .size(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(if on { chart::bt() } else { white(0.55) })
                    .when(on, |d| d.bg(rgb(0x0018_1a1f)))
                    .hover(|d| d.text_color(white(0.95)))
                    .on_click(cx.listener(move |this, _, _, cx| this.go(page, cx)))
                    .child(page.icon())
            }))
            .child(div().flex_1())
            .child(div().size(px(10.)).rounded_full().bg(link_color(st)))
    }

    /// The roast in progress (or what the machine is doing), one click from
    /// the live screen.
    fn live_card(st: &LiveState, cx: &mut Context<Self>) -> impl IntoElement {
        let bt = fmt1(st.last().and_then(|s| s.values.get("bt").copied()));
        let alarms = active_alarms(st);
        let (title, time, line, accent): (String, String, String, Hsla) =
            if let Some(alarm) = alarms.first() {
                (
                    tf("Alarm · {name}", &[("name", &alarm.name())]),
                    st.now()
                        .filter(|_| st.recording())
                        .map_or_else(|| "–:––".into(), |at| clock(st.roast_s(at))),
                    alarm.advice().to_owned(),
                    rgb(0x00f8_7171).into(),
                )
            } else if st.recording() {
                let roast = st.roast.as_ref();
                (
                    tf(
                        "Recording · Roast #{n}",
                        &[("n", &roast.map_or(0, |r| r.number))],
                    ),
                    st.now()
                        .map_or_else(|| "–:––".into(), |at| clock(st.roast_s(at))),
                    roast
                        .and_then(|r| r.coffee.as_ref().map(|c| c.name.clone()))
                        .unwrap_or_else(|| t("No coffee set").into()),
                    chart::bt(),
                )
            } else if st.connected() {
                (
                    t("Waiting for charge").into(),
                    "–:––".into(),
                    t("Charge is detected automatically").into(),
                    white(0.6),
                )
            } else {
                let (state, color) = link_words(st);
                let title = if state == "Not set up" {
                    t("Set up the machine").to_owned()
                } else {
                    tf("Machine · {state}", &[("state", &state)])
                };
                let line = st
                    .link
                    .as_ref()
                    .and_then(|l| l.advice.clone())
                    .unwrap_or_else(|| link_error(st));
                (title, "–:––".into(), line, color)
            };
        div()
            .id("live-card")
            .test_support()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(chart::faded(accent, 0.45))
            .bg(chart::faded(accent, 0.08))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.go(Page::Live, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(accent)
                    .children(
                        st.recording()
                            .then(|| div().size(px(7.)).rounded_full().bg(accent)),
                    )
                    .child(title.to_uppercase()),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .child(time)
                    .child(div().text_color(chart::bt()).child(format!("{bt}°"))),
            )
            .child(div().text_xs().text_color(white(0.5)).child(line))
    }

    fn machine_status(st: &LiveState, cx: &mut Context<Self>) -> impl IntoElement {
        let name = st
            .machine
            .as_ref()
            .map(|m| m.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| t("Machine").to_owned());
        let text = format!("{name} · {}", link_words(st).0);
        div()
            .id("machine-status")
            .test_support()
            .flex()
            .items_center()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(white(0.08))
            .text_xs()
            .text_color(white(0.6))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.go(Page::Machine, cx)))
            .child(div().size(px(8.)).rounded_full().bg(link_color(st)))
            .child(text)
    }
}

impl Shell {
    /// `PT · EN` at the foot of the sidebar.
    fn languages(cx: &mut Context<Self>) -> impl IntoElement {
        let now = i18n::current();
        div().flex().justify_center().gap_1().text_xs().children(
            i18n::Lang::ALL
                .into_iter()
                .enumerate()
                .flat_map(|(i, lang)| {
                    let on = lang == now;
                    let choice = div()
                        .id(SharedString::from(format!("lang-{}", lang.tag())))
                        .test_support()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .font_weight(if on {
                            FontWeight::BOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if on { chart::bt() } else { white(0.5) })
                        .hover(|d| d.text_color(white(0.9)))
                        .on_click(cx.listener(move |this, _, _, cx| this.speak(lang, cx)))
                        .child(lang.short())
                        .into_any_element();
                    let dot =
                        (i > 0).then(|| div().text_color(white(0.3)).child("·").into_any_element());
                    dot.into_iter().chain(std::iter::once(choice))
                }),
        )
    }

    /// Switches the language, remembers it, and redraws every page.
    fn speak(&mut self, lang: i18n::Lang, cx: &mut Context<Self>) {
        if i18n::current() == lang {
            return;
        }
        i18n::set(lang);
        let tag = lang.tag().to_owned();
        if let Some(reply) = self
            .engine
            .ask(|r| roastro::engine::Request::Edit(roastro::engine::Edit::SetLanguage(tag, r)))
        {
            cx.background_executor()
                .spawn(async move {
                    let _ = reply.await;
                })
                .detach();
        }
        // Lists whose items hold text are rebuilt; every page draws again.
        self.roasts.update(cx, RoastsPage::reload_coffees);
        self.new_roast.update(cx, |n, cx| n.load_choices(cx));
        self.live.update(cx, |_, cx| cx.notify());
        self.roasts.update(cx, |_, cx| cx.notify());
        self.coffees.update(cx, |_, cx| cx.notify());
        self.new_roast.update(cx, |_, cx| cx.notify());
        self.machine.update(cx, |_, cx| cx.notify());
        if let Some(d) = &self.detail {
            d.update(cx, |_, cx| cx.notify());
        }
        if let Some(c) = &self.compare {
            c.update(cx, |_, cx| cx.notify());
        }
        if let Some((c, _)) = &self.coffee {
            c.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// The roast being recorded, pinned above the list: one click to Live.
    fn recording_row(st: &LiveState, cx: &mut Context<Self>) -> impl IntoElement {
        let roast = st.roast.as_ref();
        div()
            .id("recording-row")
            .test_support()
            .flex()
            .items_center()
            .gap_3()
            .mx_6()
            .mt_4()
            .px_4()
            .py_2()
            .rounded_lg()
            .border_1()
            .border_color(chart::faded(chart::bt(), 0.45))
            .bg(chart::faded(chart::bt(), 0.08))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.go(Page::Live, cx)))
            .child(div().size(px(8.)).rounded_full().bg(chart::bt()))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tf(
                "Roast #{n} is recording",
                &[("n", &roast.map_or(0, |r| r.number))],
            )))
            .child(
                div().text_color(white(0.6)).child(
                    st.now()
                        .map_or_else(String::new, |at| clock(st.roast_s(at))),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(chart::bt())
                    .child(t("Open the live roast")),
            )
    }
}

/// The machine's alarms in the latest reading, while it answers.
fn active_alarms(st: &LiveState) -> Vec<Alarm> {
    if !st.connected() {
        return Vec::new();
    }
    st.last()
        .map(|s| Alarm::active(|p| s.values.get(p).copied()).collect())
        .unwrap_or_default()
}

fn link_color(st: &LiveState) -> Hsla {
    if st.connected() {
        chart::ror()
    } else {
        rgb(0x00f8_7171).into()
    }
}

fn link_error(st: &LiveState) -> String {
    st.link
        .as_ref()
        .and_then(|l| l.error.clone())
        .unwrap_or_else(|| t("Connecting…").into())
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let st = self.live.read(cx).state.clone();
        let alarms = active_alarms(&st);
        // One bell when an alarm starts, whatever page is open.
        if alarms.iter().any(|a| !self.alarms_shown.contains(a)) {
            window.play_system_bell();
        }
        self.alarms_shown.clone_from(&alarms);
        let page = match self.page {
            Page::Live => self.live.clone().into_any_element(),
            Page::Roasts => div()
                .size_full()
                .flex()
                .flex_col()
                .children(st.recording().then(|| Self::recording_row(&st, cx)))
                .child(div().flex_1().min_h_0().child(self.roasts.clone()))
                .into_any_element(),
            Page::Roast => match &self.detail {
                Some(detail) => detail.clone().into_any_element(),
                None => self.roasts.clone().into_any_element(),
            },
            Page::Compare => match &self.compare {
                Some(compare) => compare.clone().into_any_element(),
                None => self.roasts.clone().into_any_element(),
            },
            Page::Coffees => self.coffees.clone().into_any_element(),
            Page::Coffee => match &self.coffee {
                Some((coffee, _)) => coffee.clone().into_any_element(),
                None => self.coffees.clone().into_any_element(),
            },
            Page::NewRoast => self.new_roast.clone().into_any_element(),
            Page::Machine => self.machine.clone().into_any_element(),
        };
        let nav = if self.page == Page::Live {
            self.rail(&st, cx).into_any_element()
        } else {
            self.sidebar(&st, cx).into_any_element()
        };
        div()
            .id("shell")
            .size_full()
            .flex()
            .bg(rgb(0x000b_0c0e))
            .text_color(white(0.92))
            .child(nav)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .children(alarms.iter().map(|alarm| {
                        div()
                            .id(SharedString::from(format!("alarm-{}", alarm.point())))
                            .test_support()
                            .flex_none()
                            .flex()
                            .gap_2()
                            .px_5()
                            .py_2()
                            .bg(rgb(0x00b9_1c1c))
                            .text_color(white(0.98))
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child(format!("{}:", alarm.name())),
                            )
                            .child(alarm.advice())
                    }))
                    .child(div().flex_1().min_h_0().child(page)),
            )
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use std::collections::BTreeMap;

    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AnyWindowHandle, AppContext, Bounds, Entity, Point, TestAppContext, WindowBounds,
        WindowOptions, px, size,
    };
    use roastro::engine::{Edit, Request};
    use roastro::wire::{
        Columns, EventOut, MetricsOut, NextRoastOut, RoastDoc, RoastPage, SummaryOut,
    };
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use super::{Page, Shell};
    use crate::view::Engine;

    fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Shell>, mpsc::Receiver<Request>) {
        cx.update(gpui_kit::init);
        let (tx, rx) = mpsc::channel(64);
        let (handle, shell) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1440.), px(900.)),
                })),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Shell::new(Engine(tx), 1.0, window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        (handle, shell, rx)
    }

    fn done_roast(id: Uuid, number: i64) -> RoastDoc {
        let event = |kind, at| EventOut {
            kind,
            at,
            source: "auto",
            original_at: None,
        };
        RoastDoc {
            id,
            number,
            state: "done",
            charge_at: Some(1_000_000),
            roasted_by: String::new(),
            green_weight_g: Some(5000),
            roasted_weight_g: None,
            coffee: None,
            reference: None,
            events: vec![event("charge", 1_000_000), event("drop", 1_600_000)],
            notes: Vec::new(),
            metrics: MetricsOut {
                duration_ms: Some(600_000),
                phases: Vec::new(),
                development_time_ratio: Some(0.2),
                weight_loss_ratio: None,
            },
            summary: Some(SummaryOut {
                drop_bt: Some(212.4),
                ror_fall: Some(6.5),
            }),
        }
    }

    /// Answers what the pages ask, as the engine would, and says what they asked.
    fn serve(rx: &mut mpsc::Receiver<Request>, roast: &RoastDoc) -> Vec<String> {
        serve_all(rx, std::slice::from_ref(roast))
    }

    /// As `serve`, listing every roast given and finding each by id.
    fn serve_all(rx: &mut mpsc::Receiver<Request>, all: &[RoastDoc]) -> Vec<String> {
        let roast = &all[0];
        let mut seen = Vec::new();
        while let Ok(req) = rx.try_recv() {
            match req {
                Request::Edit(Edit::Roasts(q, reply)) => {
                    // Unfiltered listings are pages loading; say only what a filter asked.
                    if q.q.is_some() || q.since.is_some() {
                        seen.push(format!("roasts q={:?} since={}", q.q, q.since.is_some()));
                    }
                    let _ = reply.send(Ok(RoastPage {
                        roasts: all.to_vec(),
                        next_before: None,
                    }));
                }
                Request::Edit(Edit::Coffees(_, reply)) => {
                    let _ = reply.send(Ok(Vec::new()));
                }
                Request::Roast(id, reply) => {
                    let found = all.iter().find(|r| r.id == id.0).unwrap_or(roast);
                    seen.push(if all.len() == 1 {
                        "roast".into()
                    } else {
                        format!("roast #{}", found.number)
                    });
                    let _ = reply.send(Ok(found.clone()));
                }
                Request::Samples(_, reply) => {
                    let _ = reply.send(Ok(Columns {
                        at: Vec::new(),
                        ror: Vec::new(),
                        values: BTreeMap::new(),
                    }));
                }
                Request::Edit(Edit::GetNextRoast(reply)) => {
                    let _ = reply.send(Ok(NextRoastOut {
                        coffee_id: None,
                        green_weight_g: Some(4500),
                        roasted_by: "Ana".into(),
                        reference_id: None,
                        suggested_reference_id: None,
                        coffee_name: None,
                        reference_number: None,
                    }));
                }
                Request::Edit(Edit::PutNextRoast(next, _)) => {
                    seen.push(format!(
                        "next ref={} green={:?}",
                        next.reference_id == Some(roast.id),
                        next.green_weight_g
                    ));
                }
                _ => {}
            }
        }
        seen
    }

    #[gpui_kit::test]
    fn navigates_and_collapses_on_the_live_roast(cx: &mut TestAppContext) {
        let (handle, shell, _rx) = open(cx);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("sidebar-rail").is_some(),
                "Live opens collapsed"
            );
            assert!(window.try_find("sidebar").is_none());
            window.click("rail-roasts", cx);
            window.render_frame(cx);
            assert!(window.try_find("page-roasts").is_some());
            assert!(
                window.try_find("sidebar").is_some(),
                "other pages show the sidebar"
            );
            window.click("nav-coffees", cx);
            window.render_frame(cx);
            assert!(window.try_find("page-coffees").is_some());
            window.click("machine-status", cx);
            window.render_frame(cx);
            assert!(window.try_find("page-machine").is_some());
            window.click("nav-new", cx);
            window.render_frame(cx);
            assert!(window.try_find("page-new-roast").is_some());
            window.click("live-card", cx);
        })
        .unwrap();
        assert_eq!(cx.update(|cx| shell.read(cx).page), Page::Live);
    }

    #[gpui_kit::test]
    fn finds_a_roast_opens_it_and_comes_back(cx: &mut TestAppContext) {
        let (handle, shell, mut rx) = open(cx);
        let roast = done_roast(Uuid::now_v7(), 7);
        serve(&mut rx, &roast);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("rail-roasts", cx);
            window.render_frame(cx);
            window.click("range-week", cx);
            window.click("roast-search", cx);
            window.input("7", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let asked = serve(&mut rx, &roast);
        assert_eq!(
            asked.first().map(String::as_str),
            Some("roasts q=None since=true")
        );
        assert_eq!(
            asked.last().map(String::as_str),
            Some("roasts q=Some(\"7\") since=true")
        );
        cx.run_until_parked();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("roast-7", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(serve(&mut rx, &roast), ["roast"]);
        cx.run_until_parked();
        serve(&mut rx, &roast); // its readings
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| shell.read(cx).page), Page::Roast);

        let csv = std::env::temp_dir().join(format!("roastro-ui-{}.csv", Uuid::now_v7()));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("export-csv", cx);
        })
        .unwrap();
        cx.simulate_new_path_selection(|_| Some(csv.clone()));
        cx.run_until_parked();
        let written = std::fs::read_to_string(&csv).expect("the CSV is saved where chosen");
        // Another test may switch the language meanwhile: check the shape.
        assert!(
            written.starts_with('\u{feff}') && written.contains(';'),
            "{written}"
        );
        let _ = std::fs::remove_file(&csv);

        let pdf = csv.with_extension("pdf");
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("export-pdf", cx);
        })
        .unwrap();
        cx.simulate_new_path_selection(|_| Some(pdf.clone()));
        cx.run_until_parked();
        let written = std::fs::read(&pdf).expect("the PDF is saved where chosen");
        assert!(written.starts_with(b"%PDF-"));
        let _ = std::fs::remove_file(&pdf);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("use-as-reference", cx);
        })
        .unwrap();
        cx.run_until_parked();
        serve(&mut rx, &roast);
        cx.run_until_parked();
        assert_eq!(
            serve(&mut rx, &roast),
            ["next ref=true green=Some(4500)"],
            "the reference changes, the rest is kept"
        );

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("back", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| shell.read(cx).page), Page::Roasts);
        assert_eq!(
            serve(&mut rx, &roast).first().map(String::as_str),
            Some("roasts q=Some(\"7\") since=true"),
            "back to the list as it was, refreshed"
        );
    }

    #[gpui_kit::test]
    fn ticks_roasts_and_compares_them(cx: &mut TestAppContext) {
        let (handle, shell, mut rx) = open(cx);
        let all = [done_roast(Uuid::now_v7(), 8), done_roast(Uuid::now_v7(), 7)];
        serve_all(&mut rx, &all);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("rail-roasts", cx);
            window.render_frame(cx);
            window.click("pick-8", cx);
            window.click("pick-7", cx);
            window.render_frame(cx);
            window.click("compare-picked", cx);
        })
        .unwrap();
        assert_eq!(
            cx.update(|cx| shell.read(cx).page),
            Page::Compare,
            "ticking does not open a roast; Compare does"
        );
        let mut asked = Vec::new();
        for _ in 0..4 {
            cx.run_until_parked();
            asked.extend(serve_all(&mut rx, &all));
        }
        assert_eq!(asked, ["roast #8", "roast #7"], "in the order ticked");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("compare-figures").is_some());
            window.click("align-fc", cx);
            window.render_frame(cx);
            window.click("compare-open-7", cx);
        })
        .unwrap();
        assert_eq!(cx.update(|cx| shell.read(cx).page), Page::Roast);
    }

    #[gpui_kit::test]
    fn adds_a_coffee_and_roasts_it_next(cx: &mut TestAppContext) {
        use roastro::wire::CoffeeOut;

        let (handle, shell, mut rx) = open(cx);
        let guji = CoffeeOut {
            id: Uuid::now_v7(),
            name: "Ethiopia Guji".into(),
            notes: String::new(),
            lot: "2401".into(),
            origin: "Ethiopia".into(),
            process: "Natural".into(),
            supplier: String::new(),
            archived: false,
            stats: None,
        };
        // Answers as the engine would; says what was stored.
        let serve = |rx: &mut mpsc::Receiver<Request>| {
            let mut seen = Vec::new();
            while let Ok(req) = rx.try_recv() {
                match req {
                    Request::Edit(Edit::Coffees(_, reply)) => {
                        let _ = reply.send(Ok(vec![guji.clone()]));
                    }
                    Request::Edit(Edit::Roasts(_, reply)) => {
                        let _ = reply.send(Ok(RoastPage {
                            roasts: Vec::new(),
                            next_before: None,
                        }));
                    }
                    Request::Edit(Edit::AddCoffee(body, _)) => {
                        seen.push(format!("add {} lot {}", body.name, body.lot));
                    }
                    Request::Edit(Edit::PutNextRoast(next, _)) => {
                        seen.push(format!("next {}", next.coffee_id == Some(guji.id)));
                    }
                    _ => {}
                }
            }
            seen
        };
        serve(&mut rx);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("rail-coffees", cx);
        })
        .unwrap();
        cx.run_until_parked();
        serve(&mut rx);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("add-coffee-open", cx);
            window.render_frame(cx);
            window.click("new-coffee-name", cx);
            window.input("Kenya Nyeri", cx);
            window.click("new-coffee-lot", cx);
            window.input("2412", cx);
            window.click("add-coffee-save", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(serve(&mut rx), ["add Kenya Nyeri lot 2412"]);

        let row: &'static str = Box::leak(format!("coffee-{}", guji.id).into_boxed_str());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(row, cx);
        })
        .unwrap();
        assert_eq!(cx.update(|cx| shell.read(cx).page), Page::Coffee);
        for _ in 0..3 {
            cx.run_until_parked();
            serve(&mut rx);
        }
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("roast-it-next", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(serve(&mut rx), ["next true"]);
        assert_eq!(
            cx.update(|cx| shell.read(cx).page),
            Page::NewRoast,
            "the next roast is prepared on its page"
        );
    }

    fn snapshot(state: roastro::wire::LinkState, roast: Option<RoastDoc>) -> roastro::wire::Live {
        use roastro::wire::{Link, MachineOut, Snapshot};
        roastro::wire::Live::Snapshot(Snapshot {
            machine: MachineOut {
                name: "Caloratto".into(),
                // Nothing set up has no address.
                address: if state == roastro::wire::LinkState::NotSetUp {
                    String::new()
                } else {
                    "tcp://192.168.5.11:502".into()
                },
                model: "caloratto-2.0".into(),
                model_title: "Carmomaq Caloratto 2.0".into(),
                demo: false,
            },
            link: Link {
                state,
                since: 1_000_000,
                error: (state == roastro::wire::LinkState::Down).then(|| "timed out".into()),
                retry_at: None,
                advice: (state == roastro::wire::LinkState::Down)
                    .then(|| "Check the network.".into()),
            },
            next_roast: NextRoastOut {
                coffee_id: None,
                green_weight_g: None,
                roasted_by: String::new(),
                reference_id: None,
                suggested_reference_id: None,
                coffee_name: None,
                reference_number: None,
            },
            roast,
            samples: Vec::new(),
        })
    }

    #[gpui_kit::test]
    fn sets_up_the_machine_on_first_start(cx: &mut TestAppContext) {
        use roastro::wire::{LinkState, TestOut};

        let (handle, shell, mut rx) = open(cx);
        let live = cx.update(|cx| shell.read(cx).live.clone());
        cx.update(|cx| {
            live.update(cx, |s, cx| {
                s.apply(&snapshot(LinkState::NotSetUp, None), cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(
            cx.update(|cx| shell.read(cx).page),
            Page::Machine,
            "nothing set up: the app opens on Machine"
        );
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("machine-setup").is_some());
            window.click("machine-address", cx);
            window.input("192.168.5.11", cx);
            window.click("test-machine", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let mut tested = None;
        while let Ok(req) = rx.try_recv() {
            if let Request::Edit(Edit::TestMachine(body, reply)) = req {
                tested = Some((body.address, body.model));
                let _ = reply.send(Ok(TestOut {
                    values: [("bt".to_owned(), 182.5)].into(),
                    latency_ms: 12,
                }));
            }
        }
        assert_eq!(
            tested,
            Some(("192.168.5.11".to_owned(), "caloratto-2.0".to_owned())),
            "the real machine's model is the default"
        );
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("test-passed").is_some(),
                "the readings show"
            );
            window.click("save-machine", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let saved = std::iter::from_fn(|| rx.try_recv().ok())
            .any(|req| matches!(req, Request::Edit(Edit::SetMachine(ref b, _)) if b.address == "192.168.5.11"));
        assert!(saved, "Save sets the machine up");
    }

    #[gpui_kit::test]
    fn a_dropped_link_offers_retry_and_alerts_during_a_roast(cx: &mut TestAppContext) {
        use roastro::wire::LinkState;

        let (handle, shell, mut rx) = open(cx);
        let live = cx.update(|cx| shell.read(cx).live.clone());
        cx.update(|cx| live.update(cx, |s, cx| s.apply(&snapshot(LinkState::Down, None), cx)));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("rail-machine", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("link-advice").is_some(),
                "says what to check"
            );
            window.click("retry-now", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok())
                .any(|req| matches!(req, Request::Edit(Edit::RetryNow(_)))),
            "Retry now reaches the engine"
        );

        let mut roast = done_roast(Uuid::now_v7(), 9);
        roast.state = "recording";
        cx.update(|cx| {
            live.update(cx, |s, cx| {
                s.apply(&snapshot(LinkState::Down, Some(roast)), cx);
            });
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("nav-live", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("readings-stopped").is_some(),
                "a red band while the roast has no readings"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn backs_up_and_restores_from_the_machine_page(cx: &mut TestAppContext) {
        use roastro::wire::{BackupOut, LinkState};

        let (handle, shell, mut rx) = open(cx);
        let live = cx.update(|cx| shell.read(cx).live.clone());
        cx.update(|cx| {
            live.update(cx, |s, cx| {
                s.apply(&snapshot(LinkState::Connected, None), cx);
            });
        });
        let info = BackupOut {
            roasts: 12,
            last_charge_at: Some(1_000_000),
            coffees: 3,
        };
        let serve = |rx: &mut mpsc::Receiver<Request>| {
            let mut seen = Vec::new();
            while let Ok(req) = rx.try_recv() {
                match req {
                    Request::Edit(Edit::BackUp(path, reply)) => {
                        seen.push(format!("back up {}", path.display()));
                        let _ = reply.send(Ok(info.clone()));
                    }
                    Request::Edit(Edit::InspectBackup(path, reply)) => {
                        seen.push(format!("inspect {}", path.display()));
                        let _ = reply.send(Ok(info.clone()));
                    }
                    Request::Edit(Edit::RestoreBackup(path, reply)) => {
                        seen.push(format!("restore {}", path.display()));
                        let _ = reply.send(Ok(info.clone()));
                    }
                    _ => {}
                }
            }
            seen
        };
        serve(&mut rx);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("rail-machine", cx);
            window.render_frame(cx);
            window.click("back-up", cx);
        })
        .unwrap();
        cx.simulate_new_path_selection(|_| Some("/media/pen/roastro.db".into()));
        cx.run_until_parked();
        assert_eq!(serve(&mut rx), ["back up /media/pen/roastro.db"]);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("restore", cx);
        })
        .unwrap();
        cx.simulate_path_prompt_response(|_| Some(vec!["/media/pen/roastro.db".into()]));
        cx.run_until_parked();
        assert_eq!(serve(&mut rx), ["inspect /media/pen/roastro.db"]);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("restore-confirm").is_some(),
                "asks before replacing"
            );
            window.click("confirm-restore", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(serve(&mut rx), ["restore /media/pen/roastro.db"]);
    }

    #[gpui_kit::test]
    fn an_alarm_shows_on_every_page_until_it_clears(cx: &mut TestAppContext) {
        use std::collections::HashMap;

        use roastro::wire::{LinkState, Live, SampleOut};

        let (handle, shell, _rx) = open(cx);
        let live = cx.update(|cx| shell.read(cx).live.clone());
        let reading = |flame: f64| {
            Live::Sample(SampleOut {
                at: 2_000_000,
                values: HashMap::from([
                    ("bt".to_owned(), 180.0),
                    ("flame_fault".to_owned(), flame),
                ]),
                ror: None,
            })
        };
        cx.update(|cx| {
            live.update(cx, |s, cx| {
                s.apply(&snapshot(LinkState::Connected, None), cx);
                s.apply(&reading(1.0), cx);
            });
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("alarm-flame_fault").is_some(),
                "on the live roast"
            );
            window.click("rail-coffees", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("alarm-flame_fault").is_some(),
                "and on any other page"
            );
        })
        .unwrap();
        cx.update(|cx| live.update(cx, |s, cx| s.apply(&reading(0.0), cx)));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("alarm-flame_fault").is_none(),
                "gone when it clears"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn switches_the_language_and_remembers_it(cx: &mut TestAppContext) {
        let (handle, _shell, mut rx) = open(cx);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("rail-coffees", cx);
            window.render_frame(cx);
            window.click("lang-pt-BR", cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(i18n::current(), i18n::Lang::PtBr);
        let saved = std::iter::from_fn(|| rx.try_recv().ok()).any(
            |req| matches!(req, Request::Edit(Edit::SetLanguage(ref tag, _)) if tag == "pt-BR"),
        );
        assert!(saved, "the choice is kept");
        assert_eq!(i18n::t("Roasts"), "Torras");
        // Every page draws in Portuguese.
        cx.update_window(handle, |_, window, cx| {
            for page in ["nav-roasts", "nav-coffees", "nav-machine", "nav-new"] {
                window.click(page, cx);
                window.render_frame(cx);
            }
            window.click("nav-live", cx);
            window.render_frame(cx);
            window.click("rail-coffees", cx);
            window.render_frame(cx);
            window.click("lang-en", cx);
        })
        .unwrap();
        assert_eq!(i18n::current(), i18n::Lang::En);
    }
}
