//! The simulator's scenarios run offline through the engine, a simulated
//! second at a time ("Simulator scenarios to exercise detection", #56). Each
//! asserts that the roasts recorded match what the simulator did, by the
//! rules of "Roast lifecycle" (#6) and "Recording through a PLC dropout or an
//! app restart" (#11).

use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use plc::Reading;
use recorder::Rules;
use sim::{Autopilot, Scenario, Script, Sim};
use store::Store;
use tokio::sync::{broadcast, oneshot};

use super::{Edit, Engine, Failure, Millis, Request};
use crate::record::Recording;
use crate::wire::{RoastDoc, RoastQueryIn, RorSettings};

thread_local! {
    static NOW: Cell<Millis> = const { Cell::new(0) };
}

fn now() -> Millis {
    NOW.get()
}

/// What happened in the simulator: each roast's Charge and Drop, in sim seconds.
#[derive(Debug, Default)]
struct Truth {
    charges: Vec<f64>,
    drops: Vec<f64>,
}

/// Runs the autopilot (and the scenario) for `seconds` and returns the roasts
/// recorded, oldest first, and what really happened.
fn run(scenario: Option<Scenario>, seconds: u32) -> (Vec<RoastDoc>, Truth) {
    let (live, _rx) = broadcast::channel(16);
    let rules = Rules::default();
    let recording = Recording::open(Store::open_in_memory().unwrap(), rules).unwrap();
    let ror = RorSettings {
        window: Duration::from_secs(30),
        gap: rules.gap,
        time_scale: 1.0,
    };
    let mut engine = Engine::new(recording, Arc::new(live).as_ref().clone(), ror, now);
    let map = crate::machine::model_map(crate::machine::SIMULATOR).unwrap();
    let mut sim = Sim::new(7);
    sim.preheat(195.0);
    let mut pilot = Autopilot::default();
    let mut script = scenario.map(Script::new);
    for second in 0..seconds {
        sim.step(1.0);
        pilot.tick(&mut sim);
        if let Some(script) = script.as_mut() {
            script.tick(&mut sim);
        }
        let at = Millis::from(second) * 1000;
        NOW.set(at);
        let (answers, latency) = sim.link();
        let result = if answers {
            let values = map
                .points()
                .iter()
                .filter_map(|p| sim.get(&p.name).ok().map(|v| (p.name.clone(), v)))
                .collect();
            Ok(Reading { values, latency })
        } else {
            Err(Failure {
                error: "plc: read: timed out".into(),
                advice: String::new(),
            })
        };
        engine.handle(Request::Poll {
            at,
            result,
            retry_in: Duration::from_secs(1),
        });
    }
    let (tx, mut rx) = oneshot::channel();
    let query = RoastQueryIn {
        discarded: true,
        ..RoastQueryIn::default()
    };
    engine.handle(Request::Edit(Edit::Roasts(query, tx)));
    let mut roasts = rx.try_recv().unwrap().unwrap().roasts;
    roasts.reverse();
    let mut truth = Truth::default();
    for e in sim.snapshot().events {
        match e.name.as_str() {
            "charge" => truth.charges.push(e.t),
            "drop" => truth.drops.push(e.t),
            _ => {}
        }
    }
    (roasts, truth)
}

/// Roast seconds of an event, if the roast has it.
fn event_s(roast: &RoastDoc, kind: &str) -> Option<f64> {
    let charge = roast.charge_at?;
    #[allow(clippy::cast_precision_loss)] // roast milliseconds
    roast
        .events
        .iter()
        .find(|e| e.kind == kind)
        .map(|e| (e.at - charge) as f64 / 1000.0)
}

/// Every roast the simulator finished was recorded once, Done, with its
/// Drop within a few seconds of the real one.
fn assert_clean(roasts: &[RoastDoc], truth: &Truth, what: &str) {
    assert_eq!(
        roasts.len(),
        truth.charges.len(),
        "{what}: one roast per Charge, no false ones: {:?}",
        roasts.iter().map(|r| r.state).collect::<Vec<_>>()
    );
    for (i, drop) in truth.drops.iter().enumerate() {
        let roast = &roasts[i];
        assert_eq!(roast.state, "done", "{what}: roast {} is done", i + 1);
        let recorded = event_s(roast, "drop").expect("a Drop");
        let real = drop - truth.charges[i];
        assert!(
            (recorded - real).abs() <= 6.0,
            "{what}: roast {} dropped at {real:.0} s, recorded {recorded:.0} s",
            i + 1
        );
        assert!(
            event_s(roast, "turning_point").is_some(),
            "{what}: a turning point"
        );
    }
}

#[test]
fn the_autopilot_alone_records_clean_roasts() {
    let (roasts, truth) = run(None, 2600);
    assert!(truth.drops.len() >= 2, "two roasts ran: {truth:?}");
    assert_clean(&roasts, &truth, "no scenario");
}

#[test]
fn a_noisy_probe_makes_no_false_events() {
    let (roasts, truth) = run(Some(Scenario::NoisyCharge), 2600);
    assert_clean(&roasts, &truth, "noisy-charge");
}

#[test]
fn a_stuck_probe_does_not_end_the_roast() {
    let (roasts, truth) = run(Some(Scenario::StuckProbe), 1500);
    assert_clean(&roasts, &truth, "stuck-probe");
}

#[test]
fn absurd_readings_are_not_events() {
    let (roasts, truth) = run(Some(Scenario::Spike), 1500);
    assert_clean(&roasts, &truth, "spike");
}

#[test]
fn a_short_dropout_is_a_gap_and_a_long_one_closes_the_roast() {
    let (roasts, truth) = run(Some(Scenario::Dropout), 2600);
    assert!(truth.charges.len() >= 2, "{truth:?}");
    assert_eq!(
        roasts[0].state, "done",
        "20 s without readings: the roast goes on"
    );
    assert_eq!(
        roasts[1].state, "incomplete",
        "6 min without readings: it closes"
    );
}

#[test]
fn a_drop_with_no_trace_is_closed_by_the_next_charge() {
    let (roasts, truth) = run(Some(Scenario::MissedDrop), 2600);
    assert!(truth.charges.len() >= 2, "{truth:?}");
    assert_eq!(roasts.len(), truth.charges.len(), "no false roasts");
    assert_eq!(
        roasts[0].state, "incomplete",
        "never dropped, closed by the next Charge"
    );
    assert!(event_s(&roasts[0], "drop").is_none());
}
