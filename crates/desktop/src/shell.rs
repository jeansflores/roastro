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
    AnyElement, Context, Entity, FontWeight, Hsla, IntoElement, SharedString, Subscription, Window,
    div, px, rgb,
};

use crate::chart;
use crate::state::LiveState;
use crate::view::{Engine, RoastScreen, clock, fmt1, white};

/// Where the app is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Live,
    Roasts,
    Coffees,
    Machine,
}

impl Page {
    const NAV: [Self; 4] = [Self::Live, Self::Roasts, Self::Coffees, Self::Machine];

    fn title(self) -> &'static str {
        match self {
            Self::Live => "Live roast",
            Self::Roasts => "Roasts",
            Self::Coffees => "Coffees",
            Self::Machine => "Machine",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Roasts => "roasts",
            Self::Coffees => "coffees",
            Self::Machine => "machine",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Live => Icon::new(Lucide::Flame),
            Self::Roasts => Icon::new(Lucide::ChartSpline),
            Self::Coffees => Icon::new(Lucide::Coffee),
            Self::Machine => Icon::new(IconName::Cpu),
        }
    }
}

/// The window's root: the sidebar and the page on show.
pub struct Shell {
    page: Page,
    /// The live roast screen. It lives as long as the app, shown or not, so
    /// it keeps following the engine.
    live: Entity<RoastScreen>,
    _live_changed: Subscription,
}

impl Shell {
    pub fn new(
        engine: Engine,
        time_scale: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let live = cx.new(|cx| RoastScreen::new(engine, time_scale, window, cx));
        let live_changed = cx.observe(&live, |_, _, cx| cx.notify());
        Self {
            page: Page::Live,
            live,
            _live_changed: live_changed,
        }
    }

    fn go(&mut self, page: Page, cx: &mut Context<Self>) {
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
                    .child("ROASTRO"),
            )
            .child(Self::live_card(st, cx))
            .child(
                Button::new("nav-new")
                    .primary()
                    .icon(IconName::Plus)
                    .label("New roast")
                    .on_click(cx.listener(|this, _, _, cx| this.go(Page::Live, cx))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(Page::NAV.map(|page| {
                        let on = self.page == page;
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
                let on = self.page == page;
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
        let (title, time, line, accent): (String, String, String, Hsla) = if st.recording() {
            let roast = st.roast.as_ref();
            (
                format!("Recording · Roast #{}", roast.map_or(0, |r| r.number)),
                st.now()
                    .map_or_else(|| "–:––".into(), |at| clock(st.roast_s(at))),
                roast
                    .and_then(|r| r.coffee.as_ref().map(|c| c.name.clone()))
                    .unwrap_or_else(|| "No coffee set".into()),
                chart::bt(),
            )
        } else if st.connected() {
            (
                "Waiting for charge".into(),
                "–:––".into(),
                "Charge is detected automatically".into(),
                white(0.6),
            )
        } else {
            (
                "Machine offline".into(),
                "–:––".into(),
                link_error(st),
                rgb(0x00f8_7171).into(),
            )
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
        let text = if st.connected() {
            "Machine connected".to_owned()
        } else {
            "Machine offline".to_owned()
        };
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

    /// A page that is not built yet: says what it will hold.
    fn coming(page: Page) -> AnyElement {
        let what = match page {
            Page::Roasts => "Every roast, with filters by coffee and date, and its curve.",
            Page::Coffees => "Your green coffees, their lots and how they roast.",
            _ => "The machine, its connection and its readings.",
        };
        div()
            .id(SharedString::from(format!("page-{}", page.id())))
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_6()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(page.title()),
            )
            .child(div().text_color(white(0.55)).child(what))
            .into_any_element()
    }
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
        .unwrap_or_else(|| "Connecting…".into())
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let st = self.live.read(cx).state.clone();
        let page = match self.page {
            Page::Live => self.live.clone().into_any_element(),
            other => Self::coming(other),
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
            .child(div().flex_1().min_w_0().h_full().child(page))
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size,
    };
    use tokio::sync::mpsc;

    use super::{Page, Shell};
    use crate::view::Engine;

    #[gpui_kit::test]
    fn navigates_and_collapses_on_the_live_roast(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (tx, _rx) = mpsc::channel(16);
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
            window.click("live-card", cx);
        })
        .unwrap();
        assert_eq!(cx.update(|cx| shell.read(cx).page), Page::Live);
    }
}
