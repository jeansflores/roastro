//! What the live roast screen knows, built only from the engine's messages.
//! No GPUI here, so every rule is a plain unit test.

use roast::EventKind;
use roastro::wire::{Columns, Link, LinkState, Live, NextRoastOut, RoastDoc, SampleOut};
use uuid::Uuid;

/// Epoch milliseconds.
pub type Millis = i64;

/// Readings kept on screen while waiting for Charge (roast seconds).
const PREHEAT_TRACE_S: f64 = 60.0;

/// The reference roast's curve, on its own roast clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    /// Which roast.
    pub id: Uuid,
    /// Roast seconds, BT, ET.
    pub points: Vec<(f64, Option<f64>, Option<f64>)>,
    /// Its events after Charge, in roast seconds.
    pub events: Vec<(EventKind, f64)>,
}

/// The live screen's state.
#[derive(Debug, Clone)]
pub struct LiveState {
    /// The PLC link, once known.
    pub link: Option<Link>,
    /// What the operator prepared.
    pub next: Option<NextRoastOut>,
    /// The roast on screen: Recording, or the last one until the next Charge.
    pub roast: Option<RoastDoc>,
    /// Its readings, or the preheat trace while waiting.
    pub samples: Vec<SampleOut>,
    /// The curve drawn behind, once fetched.
    pub reference: Option<Reference>,
    /// The simulator's speed (1 on the machine): wall time × scale = roast time.
    pub time_scale: f64,
}

impl LiveState {
    /// An empty screen.
    #[must_use]
    pub fn new(time_scale: f64) -> Self {
        Self {
            link: None,
            next: None,
            roast: None,
            samples: Vec::new(),
            reference: None,
            time_scale,
        }
    }

    /// Applies one message from the engine.
    pub fn apply(&mut self, msg: &Live) {
        match msg {
            Live::Snapshot(s) => {
                self.link = Some(s.link.clone());
                self.next = Some(s.next_roast.clone());
                self.roast = s.roast.clone().filter(|r| r.state != "discarded");
                self.samples.clone_from(&s.samples);
            }
            Live::Sample(s) => {
                self.samples.push(s.clone());
                if self.roast.is_none() {
                    self.trim_preheat();
                }
            }
            Live::Roast(doc) => {
                let new_roast = self.roast.as_ref().is_none_or(|r| r.id != doc.id);
                if doc.state == "discarded" {
                    self.roast = None;
                    self.trim_preheat();
                    return;
                }
                if new_roast {
                    // A new Charge: keep only its lead-in from the preheat trace.
                    if let Some(charge) = doc.charge_at {
                        let lead_in = self.wall_ms(PREHEAT_TRACE_S);
                        self.samples.retain(|s| s.at >= charge - lead_in);
                    }
                }
                self.roast = Some(doc.clone());
            }
            Live::NextRoast(n) => self.next = Some(n.clone()),
            Live::Link(l) => self.link = Some(l.clone()),
        }
    }

    fn wall_ms(&self, roast_s: f64) -> Millis {
        #[allow(clippy::cast_possible_truncation)] // seconds to ms, far from i64 limits
        let ms = (roast_s * 1000.0 / self.time_scale) as Millis;
        ms
    }

    fn trim_preheat(&mut self) {
        let keep = self.wall_ms(PREHEAT_TRACE_S);
        if let Some(newest) = self.samples.last().map(|s| s.at) {
            self.samples.retain(|s| newest - s.at <= keep);
        }
    }

    /// The latest reading.
    #[must_use]
    pub fn last(&self) -> Option<&SampleOut> {
        self.samples.last()
    }

    /// The instant of the latest reading: "now" for marks and notes.
    #[must_use]
    pub fn now(&self) -> Option<Millis> {
        self.last().map(|s| s.at)
    }

    /// Roast seconds of an instant: since Charge, or relative to now while waiting.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // millisecond spans of minutes
    pub fn roast_s(&self, at: Millis) -> f64 {
        let zero = self
            .roast
            .as_ref()
            .and_then(|r| r.charge_at)
            .or_else(|| self.now())
            .unwrap_or(at);
        (at - zero) as f64 * self.time_scale / 1000.0
    }

    /// The wall-clock instant of a roast second, never after the latest
    /// reading.
    #[must_use]
    pub fn wall_at(&self, roast_s: f64) -> Option<Millis> {
        let now = self.now()?;
        let zero = self.roast.as_ref().and_then(|r| r.charge_at).unwrap_or(now);
        #[allow(clippy::cast_possible_truncation)] // roast seconds to ms
        let at = zero + (roast_s * 1000.0 / self.time_scale).round() as Millis;
        Some(at.min(now))
    }

    /// The reading closest to a roast second, if one is near enough (not
    /// across a Gap).
    #[must_use]
    pub fn sample_near(&self, roast_s: f64) -> Option<&SampleOut> {
        let reach = 2.5 * self.time_scale;
        self.samples
            .iter()
            .min_by(|a, b| {
                (self.roast_s(a.at) - roast_s)
                    .abs()
                    .total_cmp(&(self.roast_s(b.at) - roast_s).abs())
            })
            .filter(|s| (self.roast_s(s.at) - roast_s).abs() <= reach)
    }

    /// The reference roast's BT and ET closest to a roast second.
    #[must_use]
    pub fn reference_near(&self, roast_s: f64) -> Option<(Option<f64>, Option<f64>)> {
        let points = &self.reference.as_ref()?.points;
        points
            .iter()
            .min_by(|a, b| (a.0 - roast_s).abs().total_cmp(&(b.0 - roast_s).abs()))
            .filter(|p| (p.0 - roast_s).abs() <= 30.0)
            .map(|p| (p.1, p.2))
    }

    /// When a roast event happened, if it did.
    #[must_use]
    pub fn event_at(&self, kind: EventKind) -> Option<Millis> {
        self.roast
            .as_ref()?
            .events
            .iter()
            .find(|e| e.kind == kind.as_str())
            .map(|e| e.at)
    }

    /// Whether the roast on screen is between Charge and Drop.
    #[must_use]
    pub fn recording(&self) -> bool {
        self.roast.as_ref().is_some_and(|r| r.state == "recording")
    }

    /// The button the operator most likely needs next.
    #[must_use]
    pub fn next_event(&self) -> Option<EventKind> {
        if self.roast.is_none() || self.roast.as_ref().is_some_and(|r| r.state != "recording") {
            return Some(EventKind::Charge);
        }
        [EventKind::DryEnd, EventKind::FirstCrack]
            .into_iter()
            .find(|k| self.event_at(*k).is_none())
            .or(Some(EventKind::Drop))
    }

    /// The reference to draw: the roast's own, or the Next roast's while waiting.
    #[must_use]
    pub fn wanted_reference(&self) -> Option<Uuid> {
        match &self.roast {
            Some(r) => r.reference.as_ref().map(|r| r.id),
            None => self.next.as_ref().and_then(|n| n.reference_id),
        }
    }

    /// Stores a fetched reference roast, aligned on its own Charge. A roast
    /// without a Charge has no curve to align and is ignored.
    pub fn set_reference(&mut self, roast: &RoastDoc, columns: &Columns) {
        let Some(charge_at) = roast.charge_at else {
            return;
        };
        #[allow(clippy::cast_precision_loss)] // milliseconds well inside f64's range
        let s = |at: Millis| (at - charge_at) as f64 * self.time_scale / 1000.0;
        let bt = columns.values.get("bt");
        let et = columns.values.get("et");
        let points = columns
            .at
            .iter()
            .enumerate()
            .map(|(i, &at)| (s(at), bt.and_then(|c| c[i]), et.and_then(|c| c[i])))
            .collect();
        let events = roast
            .events
            .iter()
            .filter_map(|e| Some((e.kind.parse::<EventKind>().ok()?, s(e.at))))
            .filter(|(kind, _)| *kind != EventKind::Charge)
            .collect();
        self.reference = Some(Reference {
            id: roast.id,
            points,
            events,
        });
    }

    /// The reference's events still ahead of a roast being recorded and not
    /// marked yet, in roast seconds: what the timeline greys out below "now".
    #[must_use]
    pub fn upcoming(&self) -> Vec<(EventKind, f64)> {
        let (Some(reference), Some(now)) = (&self.reference, self.now()) else {
            return Vec::new();
        };
        if !self.recording() {
            return Vec::new();
        }
        let now = self.roast_s(now);
        reference
            .events
            .iter()
            .copied()
            .filter(|(kind, s)| *s > now && self.event_at(*kind).is_none())
            .collect()
    }

    /// Bean temperature at an instant: the nearest reading's.
    #[must_use]
    pub fn bt_at(&self, at: Millis) -> Option<f64> {
        self.samples
            .iter()
            .min_by_key(|s| (s.at - at).abs())
            .and_then(|s| s.values.get("bt").copied())
    }

    /// Whether readings are arriving.
    #[must_use]
    pub fn connected(&self) -> bool {
        self.link
            .as_ref()
            .is_some_and(|l| l.state == LinkState::Connected)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use roastro::wire::{EventOut, MetricsOut, Snapshot};

    use super::*;

    fn sample(at: Millis, bt: f64) -> Live {
        Live::Sample(SampleOut {
            at,
            values: HashMap::from([("bt".to_owned(), bt)]),
            ror: None,
        })
    }

    fn roast(id: Uuid, state: &'static str, events: &[(&'static str, Millis)]) -> RoastDoc {
        RoastDoc {
            id,
            number: 1,
            state,
            charge_at: events.iter().find(|e| e.0 == "charge").map(|e| e.1),
            roasted_by: String::new(),
            green_weight_g: None,
            roasted_weight_g: None,
            coffee: None,
            reference: None,
            events: events
                .iter()
                .map(|&(kind, at)| EventOut {
                    kind,
                    at,
                    source: "auto",
                    original_at: None,
                })
                .collect(),
            notes: Vec::new(),
            metrics: MetricsOut {
                duration_ms: None,
                phases: Vec::new(),
                development_time_ratio: None,
                weight_loss_ratio: None,
            },
        }
    }

    fn snapshot() -> Live {
        Live::Snapshot(Snapshot {
            link: Link {
                state: LinkState::Connected,
                since: 0,
                error: None,
                retry_at: None,
            },
            next_roast: NextRoastOut {
                coffee_id: None,
                green_weight_g: None,
                roasted_by: String::new(),
                reference_id: None,
                suggested_reference_id: None,
            },
            roast: None,
            samples: Vec::new(),
        })
    }

    #[test]
    fn keeps_a_minute_of_preheat_then_the_roast() {
        let mut s = LiveState::new(10.0); // 60 roast seconds = 6 wall seconds
        s.apply(&snapshot());
        for t in 0..=20 {
            s.apply(&sample(t * 1000, 200.0));
        }
        assert_eq!(
            s.samples.first().map(|x| x.at),
            Some(14_000),
            "the last six wall seconds"
        );
        assert_eq!(s.next_event(), Some(EventKind::Charge));

        let id = Uuid::now_v7();
        s.apply(&Live::Roast(roast(id, "recording", &[("charge", 20_000)])));
        for t in 21..=200 {
            s.apply(&sample(t * 1000, 150.0));
        }
        assert_eq!(
            s.samples.first().map(|x| x.at),
            Some(14_000),
            "the lead-in stays"
        );
        assert_eq!(s.samples.len(), 7 + 180, "nothing trimmed during a roast");
        assert!(
            (s.roast_s(26_000) - 60.0).abs() < 1e-9,
            "roast clock from Charge, scaled"
        );
        assert_eq!(s.next_event(), Some(EventKind::DryEnd));
    }

    #[test]
    fn walks_the_next_event_and_returns_to_waiting_on_discard() {
        let mut s = LiveState::new(1.0);
        s.apply(&snapshot());
        let id = Uuid::now_v7();
        s.apply(&Live::Roast(roast(
            id,
            "recording",
            &[("charge", 0), ("dry_end", 300_000)],
        )));
        assert_eq!(s.next_event(), Some(EventKind::FirstCrack));
        s.apply(&Live::Roast(roast(
            id,
            "recording",
            &[
                ("charge", 0),
                ("dry_end", 300_000),
                ("first_crack", 500_000),
            ],
        )));
        assert_eq!(s.next_event(), Some(EventKind::Drop));
        s.apply(&Live::Roast(roast(
            id,
            "done",
            &[("charge", 0), ("drop", 600_000)],
        )));
        assert!(!s.recording());
        assert_eq!(
            s.next_event(),
            Some(EventKind::Charge),
            "after Drop the next thing is a new Charge"
        );
        s.apply(&Live::Roast(roast(id, "discarded", &[("charge", 0)])));
        assert!(s.roast.is_none(), "a discarded roast leaves the screen");
    }

    #[test]
    fn aligns_the_reference_on_its_charge_and_greys_what_is_ahead() {
        let mut s = LiveState::new(1.0);
        let columns = Columns {
            at: vec![10_000, 70_000],
            ror: vec![None, None],
            values: [("bt".to_owned(), vec![Some(200.0), Some(90.0)])].into(),
        };
        let reference = roast(
            Uuid::now_v7(),
            "done",
            &[
                ("charge", 10_000),
                ("dry_end", 250_000),
                ("first_crack", 490_000),
                ("drop", 610_000),
            ],
        );
        s.set_reference(&reference, &columns);
        let r = s.reference.clone().unwrap();
        assert_eq!(
            r.points,
            [(0.0, Some(200.0), None), (60.0, Some(90.0), None)]
        );
        assert_eq!(r.events.len(), 3, "Charge is the origin, not an event");

        s.apply(&Live::Roast(roast(
            Uuid::now_v7(),
            "recording",
            &[("charge", 0), ("dry_end", 230_000)],
        )));
        s.apply(&sample(300_000, 170.0));
        assert_eq!(
            s.upcoming(),
            [(EventKind::FirstCrack, 480.0), (EventKind::Drop, 600.0)],
            "marked and past events drop out"
        );
        assert_eq!(s.bt_at(299_000), Some(170.0));
    }
}

#[cfg(test)]
mod pointer_tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    #[allow(clippy::cast_precision_loss)] // small test seconds
    fn maps_roast_seconds_to_readings_and_instants() {
        let mut s = LiveState::new(10.0);
        for t in 0..10 {
            s.samples.push(SampleOut {
                at: t * 1000,
                values: HashMap::from([("bt".to_owned(), 100.0 + t as f64)]),
                ror: None,
            });
        }
        // Waiting: zero is now (9 s wall); at 10x one wall second is ten roast seconds.
        assert_eq!(s.wall_at(-20.0), Some(7000));
        assert_eq!(
            s.wall_at(50.0),
            Some(9000),
            "never after the latest reading"
        );
        assert_eq!(s.sample_near(-31.0).map(|x| x.at), Some(6000));
        assert!(s.sample_near(-500.0).is_none(), "too far from any reading");
    }
}
