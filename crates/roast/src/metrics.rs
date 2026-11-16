//! Numbers computed from a roast's events and weights, never stored ("What a
//! Roast record holds"): duration, the three phases, Development time ratio
//! and weight loss. One implementation, so the live screen, the history and
//! any export show the same figures.

use std::time::Duration;

use crate::{EventKind, RoR};

/// Epoch milliseconds.
type Millis = i64;

/// The three phases of a roast (see `GLOSSARY.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseKind {
    /// Charge to Dry end.
    Drying,
    /// Dry end to First crack.
    Maillard,
    /// First crack to Drop.
    Development,
}

impl PhaseKind {
    /// The stable name used on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PhaseKind::Drying => "drying",
            PhaseKind::Maillard => "maillard",
            PhaseKind::Development => "development",
        }
    }
}

/// One phase, possibly still running.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phase {
    /// Which phase.
    pub kind: PhaseKind,
    /// When it began.
    pub from: Millis,
    /// When it ended, or "now" while it runs.
    pub to: Millis,
    /// Its share of the roast so far (0–1).
    pub ratio: f64,
}

/// The computed figures of a roast.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Metrics {
    /// Charge to Drop, or Charge to now while recording.
    pub duration_ms: Option<Millis>,
    /// The phases begun so far, in order.
    pub phases: Vec<Phase>,
    /// Development phase over the whole roast (0–1), once First crack is marked.
    pub development_time_ratio: Option<f64>,
    /// (green − roasted) / green, once both weights are known.
    pub weight_loss_ratio: Option<f64>,
}

/// Computes a roast's figures. `events` are (kind, instant); `now` closes the
/// running phase when there is no Drop yet.
#[must_use]
#[allow(clippy::cast_precision_loss)] // millisecond spans of minutes are exact in f64
pub fn metrics(
    events: &[(EventKind, Millis)],
    now: Millis,
    green_g: Option<i64>,
    roasted_g: Option<i64>,
) -> Metrics {
    let at = |kind| events.iter().find(|(k, _)| *k == kind).map(|&(_, t)| t);
    let weight_loss_ratio = match (green_g, roasted_g) {
        (Some(g), Some(r)) if g > 0 => Some((g - r) as f64 / g as f64),
        _ => None,
    };
    let Some(charge) = at(EventKind::Charge) else {
        return Metrics {
            weight_loss_ratio,
            ..Metrics::default()
        };
    };
    let end = at(EventKind::Drop).unwrap_or(now).max(charge);
    let total = (end - charge) as f64;
    let ratio = |from: Millis, to: Millis| {
        if total > 0.0 {
            (to - from) as f64 / total
        } else {
            0.0
        }
    };

    let bounds = [
        (PhaseKind::Drying, Some(charge), at(EventKind::DryEnd)),
        (
            PhaseKind::Maillard,
            at(EventKind::DryEnd),
            at(EventKind::FirstCrack),
        ),
        (PhaseKind::Development, at(EventKind::FirstCrack), None),
    ];
    let phases = bounds
        .into_iter()
        .filter_map(|(kind, from, to)| {
            let from = from?;
            let to = to.unwrap_or(end).clamp(from, end.max(from));
            Some(Phase {
                kind,
                from,
                to,
                ratio: ratio(from, to),
            })
        })
        .collect();
    let development_time_ratio = at(EventKind::FirstCrack).map(|fc| ratio(fc.min(end), end));
    Metrics {
        duration_ms: Some(end - charge),
        phases,
        development_time_ratio,
        weight_loss_ratio,
    }
}

/// The rate of rise of BT at every reading, in °C/min of roast time, using the
/// same [`RoR`] as the live screen. It restarts after every Gap (readings more
/// than `gap` apart) and is `None` where it cannot be computed yet.
/// `time_scale` is the simulator's speed (1 on the machine).
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn ror_series(
    readings: &[(Millis, Option<f64>)],
    window: Duration,
    gap: Duration,
    time_scale: f64,
) -> Vec<Option<f64>> {
    let mut ror = RoR::new(window);
    let mut last: Option<Millis> = None;
    let gap_ms = gap.as_millis() as Millis;
    readings
        .iter()
        .map(|&(at, bt)| {
            if last.is_some_and(|l| at - l > gap_ms) {
                ror.reset();
            }
            last = Some(at);
            let bt = bt?;
            // Roast time since the first reading; only differences matter to the slope.
            let roast_ms = (at - readings[0].0).max(0) as f64 * time_scale;
            ror.add(Duration::from_millis(roast_ms as u64), bt);
            ror.value()
        })
        .collect()
}

/// The largest fall of a series over `window`, among the readings from `from`
/// to `to`: the `RoR` crash or swing the roaster looks for after a
/// disappointing cupping (a `RoR` dropping by more than a few °C/min within
/// 30 s). `None` when no pair of readings `window` apart has both values.
#[must_use]
pub fn largest_fall(
    series: &[(Millis, Option<f64>)],
    window: Duration,
    from: Millis,
    to: Millis,
) -> Option<f64> {
    let window = Millis::try_from(window.as_millis()).unwrap_or(Millis::MAX);
    let inside: Vec<(Millis, f64)> = series
        .iter()
        .filter(|(at, _)| (from..=to).contains(at))
        .filter_map(|&(at, v)| Some((at, v?)))
        .collect();
    let mut later = 0;
    let mut largest: Option<f64> = None;
    for &(at, v) in &inside {
        while later < inside.len() && inside[later].0 < at + window {
            later += 1;
        }
        let Some(&(_, then)) = inside.get(later) else {
            break;
        };
        let fall = v - then;
        largest = Some(largest.map_or(fall, |l| l.max(fall)));
    }
    largest
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Millis = 60_000;

    #[test]
    fn computes_phases_and_ratios_of_a_finished_roast() {
        let events = [
            (EventKind::Charge, 0),
            (EventKind::DryEnd, 5 * MIN),
            (EventKind::FirstCrack, 8 * MIN),
            (EventKind::Drop, 10 * MIN),
        ];
        let m = metrics(&events, 99 * MIN, Some(5000), Some(4300));
        assert_eq!(m.duration_ms, Some(10 * MIN));
        let shares: Vec<_> = m.phases.iter().map(|p| (p.kind, p.ratio)).collect();
        assert_eq!(
            shares,
            [
                (PhaseKind::Drying, 0.5),
                (PhaseKind::Maillard, 0.3),
                (PhaseKind::Development, 0.2)
            ]
        );
        assert_eq!(m.development_time_ratio, Some(0.2));
        assert!((m.weight_loss_ratio.unwrap() - 0.14).abs() < 1e-12);
    }

    #[test]
    fn a_live_roast_runs_its_open_phase_to_now() {
        let m = metrics(
            &[(EventKind::Charge, 0), (EventKind::DryEnd, 4 * MIN)],
            6 * MIN,
            None,
            None,
        );
        assert_eq!(m.duration_ms, Some(6 * MIN));
        let last = m.phases.last().unwrap();
        assert_eq!((last.kind, last.to), (PhaseKind::Maillard, 6 * MIN));
        assert_eq!(m.development_time_ratio, None);
        assert_eq!(m.weight_loss_ratio, None);
    }

    #[test]
    fn nothing_before_charge() {
        assert_eq!(metrics(&[], MIN, Some(5000), None), Metrics::default());
    }

    #[test]
    #[allow(clippy::cast_precision_loss)] // small test seconds
    fn ror_follows_the_ramp_and_restarts_after_a_gap() {
        // 12 °C/min for a minute, a 10 s Gap, then readings again.
        let mut readings: Vec<_> = (0..=60)
            .map(|s| (s * 1000, Some(100.0 + 0.2 * s as f64)))
            .collect();
        readings.extend((70..=75).map(|s| (s * 1000, Some(100.0 + 0.2 * s as f64))));
        let ror = ror_series(
            &readings,
            Duration::from_secs(30),
            Duration::from_millis(2500),
            1.0,
        );
        assert_eq!(ror[0], None, "one reading is not a slope");
        assert!((ror[60].unwrap() - 12.0).abs() < 1e-9);
        assert_eq!(ror[61], None, "restarted after the Gap");
        assert!(ror[62].is_some());
    }

    #[test]
    fn finds_the_largest_fall_within_the_window() {
        let s = 1000;
        // RoR 20 → 18 slowly, then a crash 18 → 11 at 60–90 s, then a flick.
        let series: Vec<(Millis, Option<f64>)> = [20.0, 19.0, 18.0, 11.0, 14.0, 13.0]
            .iter()
            .enumerate()
            .map(|(i, &v)| (Millis::try_from(i).unwrap() * 30 * s, Some(v)))
            .collect();
        let fall = largest_fall(&series, Duration::from_secs(30), 0, 200 * s).unwrap();
        assert!((fall - 7.0).abs() < 1e-9, "{fall}");
        assert_eq!(
            largest_fall(&series, Duration::from_secs(30), 100 * s, 200 * s),
            Some(1.0),
            "only readings in range"
        );
        assert_eq!(
            largest_fall(&series[..1], Duration::from_secs(30), 0, 200 * s),
            None
        );
    }
}
