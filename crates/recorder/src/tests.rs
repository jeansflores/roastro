use super::*;

#[allow(clippy::cast_possible_truncation)]
fn ms(seconds: f64) -> Millis {
    (seconds * 1000.0).round() as Millis
}

fn reading(at_s: f64, values: &[(&str, f64)]) -> Input {
    Input::Sample(Sample {
        at: ms(at_s),
        values: values.iter().map(|&(k, v)| (k.to_owned(), v)).collect(),
    })
}

fn bt(at_s: f64, bt: f64) -> Input {
    reading(at_s, &[("bt", bt)])
}

fn run(r: &mut Recorder, inputs: impl IntoIterator<Item = Input>) -> Vec<Output> {
    inputs.into_iter().flat_map(|i| r.handle(i)).collect()
}

fn started(out: &[Output]) -> Vec<(Millis, Source, usize)> {
    out.iter()
        .filter_map(|o| match o {
            Output::Started {
                charge_at,
                source,
                lead_in,
            } => Some((*charge_at, *source, lead_in.len())),
            _ => None,
        })
        .collect()
}

fn event(out: &[Output], kind: EventKind) -> Option<(Millis, Source)> {
    out.iter().find_map(|o| match o {
        Output::Event {
            kind: k,
            at,
            source,
        } if *k == kind => Some((*at, *source)),
        _ => None,
    })
}

fn closures(out: &[Output]) -> Vec<Closure> {
    out.iter()
        .filter_map(|o| match o {
            Output::Closed(c) => Some(*c),
            _ => None,
        })
        .collect()
}

fn assert_near(got: Millis, want: Millis, tolerance_s: i64, what: &str) {
    assert!(
        (got - want).abs() <= tolerance_s * 1000,
        "{what}: got {got} ms, want {want} ms ± {tolerance_s} s"
    );
}

/// A whole roast from the simulator's autopilot, read once per second with
/// its PLC timer, as the app would see it.
#[test]
fn records_a_simulated_roast() {
    let mut s = sim::Sim::new(1);
    let mut pilot = sim::Autopilot::default();
    let mut r = Recorder::new(Rules::default());
    let (mut out, mut truth_charge, mut truth_drop, mut was_loaded) =
        (Vec::new(), None, None, false);
    for second in 0..3 * 3600 {
        pilot.tick(&mut s);
        s.step(1.0);
        let sn = s.snapshot();
        let at = f64::from(second + 1);
        if sn.loaded && !was_loaded {
            truth_charge = Some(ms(at));
        }
        if !sn.loaded && was_loaded {
            truth_drop = Some(ms(at));
        }
        was_loaded = sn.loaded;
        let values = ["bt", "et", "timer_s"].map(|n| (n, s.get(n).unwrap()));
        out.extend(r.handle(reading(at, &values)));
        if truth_drop.is_some_and(|d| ms(at) > d + 40_000) {
            break;
        }
    }
    let (truth_charge, truth_drop) = (
        truth_charge.expect("sim charged"),
        truth_drop.expect("sim dropped"),
    );

    let starts = started(&out);
    assert_eq!(starts.len(), 1, "one roast: {starts:?}");
    let (charge_at, source, lead_in) = starts[0];
    assert_near(charge_at, truth_charge, 3, "charge");
    assert_eq!(source, Source::Auto);
    assert!(
        (55..=61).contains(&lead_in),
        "lead-in of {lead_in} readings"
    );

    let (tp, _) = event(&out, EventKind::TurningPoint).expect("turning point");
    assert!(
        (50_000..=120_000).contains(&(tp - charge_at)),
        "turning point {} s after charge",
        (tp - charge_at) / 1000
    );

    let (drop_at, source) = event(&out, EventKind::Drop).expect("drop");
    assert_near(drop_at, truth_drop, 3, "drop");
    assert_eq!(source, Source::Auto);
    assert_eq!(closures(&out), [Closure::AfterDrop]);
    assert!(!r.is_recording());
}

/// BT alone, as on the real machine, which exposes no timer.
fn roast_curve(t: f64) -> f64 {
    match t {
        t if t <= 70.0 => 200.0,                               // preheated, empty
        t if t <= 150.0 => 200.0 - (t - 70.0) * 1.375,         // charge: probe falls to 90
        t if t <= 670.0 => 90.0 + (t - 150.0) * 120.0 / 520.0, // roast up to 210
        t => 210.0 - (t - 670.0),                              // drop: 1 °C/s down
    }
}

#[test]
fn detects_charge_turning_point_and_drop_from_bt() {
    let mut r = Recorder::new(Rules::default());
    let out = run(
        &mut r,
        (0..=720).map(|t| bt(f64::from(t), roast_curve(f64::from(t)))),
    );

    let starts = started(&out);
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_near(starts[0].0, ms(70.0), 1, "charge at the start of the fall");
    assert_near(
        event(&out, EventKind::TurningPoint).expect("tp").0,
        ms(150.0),
        1,
        "turning point",
    );
    assert_near(
        event(&out, EventKind::Drop).expect("drop").0,
        ms(670.0),
        1,
        "drop",
    );
    assert_eq!(
        closures(&out),
        [Closure::AfterDrop],
        "closed after the 30 s tail"
    );
    let samples = out
        .iter()
        .filter(|o| matches!(o, Output::Sample(_)))
        .count();
    assert!(
        (625..=632).contains(&samples),
        "{samples} samples from charge to the end of the tail"
    );
}

#[test]
fn buttons_follow_the_lifecycle() {
    let mut r = Recorder::new(Rules::default());
    assert!(
        run(&mut r, [Input::DropPressed(ms(1.0))]).is_empty(),
        "Drop with no roast does nothing"
    );

    let first = run(
        &mut r,
        (0..10)
            .map(|t| bt(f64::from(t), 120.0))
            .chain([Input::ChargePressed(ms(10.0))]),
    );
    assert_eq!(started(&first), [(ms(10.0), Source::Manual, 10)]);

    // A second Charge before any Drop closes the first roast as Incomplete.
    let second = run(
        &mut r,
        (11..20)
            .map(|t| bt(f64::from(t), 120.0))
            .chain([Input::ChargePressed(ms(20.0))]),
    );
    assert_eq!(closures(&second), [Closure::NewCharge]);
    assert!(Closure::NewCharge.is_incomplete());
    assert_eq!(started(&second).len(), 1);

    let dropped = run(&mut r, [Input::DropPressed(ms(25.0))]);
    assert_eq!(
        event(&dropped, EventKind::Drop),
        Some((ms(25.0), Source::Manual))
    );
    assert!(
        r.is_recording() && !r.is_before_drop(),
        "the tail is still recording"
    );
}

#[test]
fn a_short_gap_keeps_the_roast_and_a_long_one_closes_it() {
    let mut r = Recorder::new(Rules::default());
    run(
        &mut r,
        [
            bt(0.0, 120.0),
            Input::ChargePressed(ms(0.0)),
            bt(1.0, 121.0),
        ],
    );

    // Four minutes without readings, then the PLC comes back.
    let out = run(
        &mut r,
        (1..=240)
            .map(|t| Input::Missed(ms(1.0 + f64::from(t))))
            .chain([bt(242.0, 160.0)]),
    );
    assert!(
        closures(&out).is_empty() && r.is_before_drop(),
        "still recording after a 4 min Gap"
    );

    // More than five minutes: the roast closes as Incomplete.
    let out = run(
        &mut r,
        (1..=301).map(|t| Input::Missed(ms(242.0 + f64::from(t)))),
    );
    assert_eq!(closures(&out), [Closure::Gap]);
    assert!(!r.is_recording());
}

#[test]
fn detection_never_spans_a_gap() {
    // Waiting: hot and steady, a 10 s Gap, then much cooler. Not a Charge.
    let mut r = Recorder::new(Rules::default());
    let out = run(
        &mut r,
        (0..=20)
            .map(|t| bt(f64::from(t), 205.0))
            .chain([bt(30.0, 150.0), bt(31.0, 150.0)]),
    );
    assert!(started(&out).is_empty(), "no Charge across a Gap");

    // Roasting past the minimum time: the same jump across a Gap is not a Drop.
    let mut r = Recorder::new(Rules::default());
    let out = run(
        &mut r,
        [bt(0.0, 120.0), Input::ChargePressed(ms(0.0))]
            .into_iter()
            .chain((1..=200).map(|t| bt(f64::from(t), 205.0)))
            .chain([bt(210.0, 150.0), bt(211.0, 150.0)]),
    );
    assert_eq!(event(&out, EventKind::Drop), None, "no Drop across a Gap");
}

#[test]
fn a_timer_already_running_is_not_a_charge() {
    // Roastro comes online mid-roast: the PLC timer is at 120 s and counting.
    let mut r = Recorder::new(Rules::default());
    let out = run(
        &mut r,
        (0..5).map(|t| {
            reading(
                f64::from(t),
                &[("bt", 150.0), ("timer_s", 120.0 + f64::from(t))],
            )
        }),
    );
    assert!(
        started(&out).is_empty(),
        "a roast under way is never inferred"
    );

    // A timer starting from zero is.
    let mut r = Recorder::new(Rules::default());
    let out = run(
        &mut r,
        [0.0, 0.0, 0.0, 1.0, 2.0]
            .iter()
            .enumerate()
            .map(|(i, &timer)| {
                #[allow(clippy::cast_precision_loss)]
                reading(i as f64, &[("bt", 200.0), ("timer_s", timer)])
            }),
    );
    assert_eq!(
        started(&out),
        [(ms(2.0), Source::Auto, 2)],
        "charge one timer-second before the reading"
    );
}

#[test]
fn a_timer_hiccup_is_not_a_drop() {
    let mut r = Recorder::new(Rules::default());
    let timer = |t: u32| f64::from(t - 1);
    let mut inputs: Vec<Input> = (1..=10)
        .map(|t| reading(f64::from(t), &[("bt", 150.0), ("timer_s", timer(t))]))
        .collect();
    // The same timer second read twice (poll jitter), then it moves on.
    inputs.push(reading(11.0, &[("bt", 150.0), ("timer_s", 9.0)]));
    inputs.extend(
        (12..=20).map(|t| reading(f64::from(t), &[("bt", 150.0), ("timer_s", timer(t) - 1.0)])),
    );
    let out = run(&mut r, inputs);
    assert_eq!(started(&out).len(), 1);
    assert_eq!(
        event(&out, EventKind::Drop),
        None,
        "one repeated second is not a stop"
    );

    // Standing still for two seconds is.
    let out = run(
        &mut r,
        (21..=24).map(|t| reading(f64::from(t), &[("bt", 150.0), ("timer_s", 18.0)])),
    );
    assert_eq!(event(&out, EventKind::Drop), Some((ms(21.0), Source::Auto)));
}

/// Back-to-back roasts from the autopilot: every Charge is found, no Charge
/// appears while the empty drum cools between batches, every roast closes.
#[test]
fn records_back_to_back_simulated_roasts() {
    let mut s = sim::Sim::new(7);
    let mut pilot = sim::Autopilot::default();
    let mut r = Recorder::new(Rules::default());
    let (mut out, mut charges, mut was_loaded) = (Vec::new(), 0, false);
    for second in 0..2 * 3600 {
        pilot.tick(&mut s);
        s.step(1.0);
        let loaded = s.snapshot().loaded;
        charges += usize::from(loaded && !was_loaded);
        was_loaded = loaded;
        let values = ["bt", "et", "timer_s"].map(|n| (n, s.get(n).unwrap()));
        out.extend(r.handle(reading(f64::from(second + 1), &values)));
    }
    assert!(charges >= 3, "the autopilot roasted {charges} times");
    let detail: Vec<_> = out
        .iter()
        .filter_map(|o| match o {
            Output::Started {
                charge_at, source, ..
            } => Some(format!("start {} {:?}", charge_at / 1000, source)),
            Output::Event {
                kind: EventKind::Drop,
                at,
                source,
            } => Some(format!("drop {} {:?}", at / 1000, source)),
            Output::Closed(c) => Some(format!("closed {c:?}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        started(&out).len(),
        charges,
        "one roast per real charge: {detail:#?}"
    );
    let closed = closures(&out);
    assert!(
        closed.iter().all(|c| *c == Closure::AfterDrop),
        "{closed:?}"
    );
    assert_eq!(
        closed.len() + usize::from(r.is_recording()),
        charges,
        "every roast closed"
    );
}
