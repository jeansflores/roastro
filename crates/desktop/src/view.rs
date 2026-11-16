//! The live roast screen (variant D of "Live roast screen layout", #9): the
//! chart first, a rail with the big numbers, the phases, the next event and a
//! timeline of events and notes.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AsyncApp, Context, Entity, FontWeight, Hsla, IntoElement, SharedString, WeakEntity, Window,
    div, px, relative, rgb,
};
use roast::EventKind;
use roastro::engine::{Edit, Reply, Request};
use roastro::wire::{EventIn, Live, NoteIn};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::chart::{self, faded};
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
}

impl RoastScreen {
    pub fn new(
        engine: Engine,
        time_scale: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let screen = Self {
            state: LiveState::new(time_scale),
            engine,
            note: cx.new(|cx| InputState::new(window, cx)),
            message: None,
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
                    .child(div().flex_1().w_full().child(chart::chart(st))),
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
                    seen.push(format!("event {kind} at {} on {}", body.at, roast.0 == id))
                }
                Request::Edit(Edit::AddNote(_, note, _)) => {
                    seen.push(format!("note {:?} at {}", note.text, note.at))
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
}
