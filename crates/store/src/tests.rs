use super::*;

fn sample(at: Millis, bt: f64) -> Sample {
    Sample {
        at,
        values: [("bt".to_owned(), bt), ("et".to_owned(), bt + 10.0)].into(),
    }
}

/// A temporary database file, removed on drop.
struct TempDb(std::path::PathBuf);

impl TempDb {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("roastro-test-{}.db", Uuid::now_v7())))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

#[test]
fn migrates_once_and_keeps_the_machine() {
    let db = TempDb::new();
    let first = Store::open(&db.0).unwrap();
    assert_eq!(first.schema_version().unwrap(), MIGRATIONS.len());
    let machine = first.machine_id;
    drop(first);

    let again = Store::open(&db.0).unwrap();
    assert_eq!(again.schema_version().unwrap(), MIGRATIONS.len());
    assert_eq!(again.machine_id, machine, "reopening keeps the machine row");
}

#[test]
fn records_a_roast_from_charge_to_drop() {
    let mut st = Store::open_in_memory().unwrap();
    let coffee = st.add_coffee("Bourbon Amarelo").unwrap();
    st.set_next_roast(&NextRoast {
        coffee_id: Some(coffee),
        green_weight_g: Some(4600),
        roasted_by: "Lucas".into(),
        reference_id: None,
    })
    .unwrap();

    let lead_in: Vec<Sample> = (0..60).map(|s| sample(s * 1000, 200.0)).collect();
    let id = st.start_roast(60_000, Source::Auto, &lead_in).unwrap();
    for s in 61..=700 {
        st.add_sample(id, &sample(s * 1000, 150.0)).unwrap();
    }
    st.set_event(id, EventKind::TurningPoint, 140_000, Source::Computed)
        .unwrap();
    assert_eq!(st.roast(id).unwrap().state, RoastState::Recording);
    st.set_event(id, EventKind::Drop, 670_000, Source::Auto)
        .unwrap();
    st.close_roast(id, Closure::AfterDrop).unwrap();

    let r = st.roast(id).unwrap();
    assert_eq!((r.number, r.state), (1, RoastState::Done));
    assert_eq!(
        (r.coffee_id, r.green_weight_g, r.roasted_by.as_str()),
        (Some(coffee), Some(4600), "Lucas"),
        "Next roast copied"
    );
    let kinds: Vec<_> = r.events.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [EventKind::Charge, EventKind::TurningPoint, EventKind::Drop]
    );
    let samples = st.samples(id).unwrap();
    assert_eq!(samples.len(), 60 + 640, "lead-in plus every reading");
    assert_eq!(samples[0], lead_in[0], "values survive the round trip");
    assert_eq!(
        st.next_roast().unwrap().coffee_id,
        Some(coffee),
        "Next roast stays filled after Charge"
    );

    let second = st.start_roast(900_000, Source::Manual, &[]).unwrap();
    assert_eq!(st.roast(second).unwrap().number, 2);
}

#[test]
fn a_correction_keeps_the_automatic_time() {
    let mut st = Store::open_in_memory().unwrap();
    let id = st.start_roast(0, Source::Auto, &[]).unwrap();
    st.set_event(id, EventKind::TurningPoint, 80_000, Source::Computed)
        .unwrap();
    st.set_event(id, EventKind::TurningPoint, 85_000, Source::Manual)
        .unwrap();
    st.set_event(id, EventKind::TurningPoint, 83_000, Source::Manual)
        .unwrap();
    st.set_event(id, EventKind::Charge, 2_000, Source::Manual)
        .unwrap();

    let events = st.roast(id).unwrap().events;
    let tp = events
        .iter()
        .find(|e| e.kind == EventKind::TurningPoint)
        .unwrap();
    assert_eq!(
        (tp.at, tp.source, tp.original_at),
        (83_000, Source::Manual, Some(80_000)),
        "first time kept across corrections"
    );
    let charge = events.iter().find(|e| e.kind == EventKind::Charge).unwrap();
    assert_eq!(charge.original_at, Some(0));
}

#[test]
fn closing_without_drop_is_incomplete_and_a_later_drop_completes_it() {
    let mut st = Store::open_in_memory().unwrap();
    let id = st.start_roast(0, Source::Manual, &[]).unwrap();
    st.close_roast(id, Closure::Gap).unwrap();
    assert_eq!(st.roast(id).unwrap().state, RoastState::Incomplete);
    assert_eq!(st.recording_roast().unwrap(), None);

    st.set_event(id, EventKind::Drop, 600_000, Source::Manual)
        .unwrap();
    assert_eq!(st.roast(id).unwrap().state, RoastState::Done);
}

#[test]
fn a_restart_finds_the_roast_still_recording() {
    let db = TempDb::new();
    let id = {
        let mut st = Store::open(&db.0).unwrap();
        let id = st.start_roast(0, Source::Auto, &[]).unwrap();
        st.add_sample(id, &sample(1000, 120.0)).unwrap();
        id
    }; // the process dies here

    let st = Store::open(&db.0).unwrap();
    assert_eq!(st.recording_roast().unwrap(), Some(id));
    assert_eq!(st.last_sample_at(id).unwrap(), Some(1000));
}

#[test]
fn rejects_what_the_schema_forbids() {
    let mut st = Store::open_in_memory().unwrap();
    let ghost = RoastId(Uuid::now_v7());
    assert!(
        st.add_sample(ghost, &sample(0, 100.0)).is_err(),
        "sample for an unknown roast"
    );
    assert!(matches!(st.roast(ghost), Err(Error::NoRoast(_))));
    let bad = NextRoast {
        green_weight_g: Some(0),
        ..NextRoast::default()
    };
    assert!(st.set_next_roast(&bad).is_err(), "a zero weight");
}

#[test]
fn reads_coffee_names_and_notes() {
    let mut st = Store::open_in_memory().unwrap();
    let coffee = st.add_coffee("Catuaí Vermelho").unwrap();
    assert_eq!(
        st.coffee_name(coffee).unwrap().as_deref(),
        Some("Catuaí Vermelho")
    );
    assert_eq!(st.coffee_name(Uuid::now_v7()).unwrap(), None);
    let id = st.start_roast(0, Source::Manual, &[]).unwrap();
    assert_eq!(st.notes(id).unwrap(), []);
}

#[test]
fn edits_fields_notes_discards_and_lists() {
    let mut st = Store::open_in_memory().unwrap();
    let coffee = st.add_coffee("Mundo Novo").unwrap();
    let first = st.start_roast(0, Source::Manual, &[]).unwrap();
    st.set_event(first, EventKind::Drop, 600_000, Source::Manual)
        .unwrap();
    st.update_roast(
        first,
        &RoastPatch {
            coffee_id: Some(Some(coffee)),
            roasted_weight_g: Some(Some(4300)),
            ..RoastPatch::default()
        },
    )
    .unwrap();
    let r = st.roast(first).unwrap();
    assert_eq!(
        (r.coffee_id, r.roasted_weight_g),
        (Some(coffee), Some(4300))
    );
    st.update_roast(
        first,
        &RoastPatch {
            roasted_weight_g: Some(None),
            ..RoastPatch::default()
        },
    )
    .unwrap();
    assert_eq!(
        st.roast(first).unwrap().roasted_weight_g,
        None,
        "Some(None) clears"
    );
    assert_eq!(st.latest_done_roast(coffee).unwrap(), Some(first));

    let note = st.add_note(first, 120_000, "smell of bread").unwrap();
    let moved = st
        .update_note(first, note.id, Some(130_000), None)
        .unwrap()
        .unwrap();
    assert_eq!((moved.at, moved.text.as_str()), (130_000, "smell of bread"));
    assert!(st.delete_note(first, note.id).unwrap());
    assert!(!st.delete_note(first, note.id).unwrap());

    assert!(st.delete_event(first, EventKind::Drop).unwrap());
    let second = st.start_roast(900_000, Source::Manual, &[]).unwrap();
    st.set_discarded(first, Some(1)).unwrap();
    assert_eq!(
        st.latest_done_roast(coffee).unwrap(),
        None,
        "discarded roasts are not suggested"
    );
    let discarded_too = RoastQuery {
        include_discarded: true,
        ..RoastQuery::default()
    };
    let listed: Vec<_> = st
        .roasts(&RoastQuery::default(), 10)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(listed, [second], "discarded hidden by default");
    let all: Vec<_> = st
        .roasts(&discarded_too, 10)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(all, [second, first], "newest first");
    let older: Vec<_> = st
        .roasts(
            &RoastQuery {
                before: Some(second),
                ..discarded_too.clone()
            },
            10,
        )
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(older, [first], "cursor");

    st.update_coffee(
        coffee,
        &CoffeeChange {
            name: Some("Mundo Novo 2026".into()),
            archived: Some(true),
            ..CoffeeChange::default()
        },
    )
    .unwrap();
    assert_eq!(st.coffees(false).unwrap(), []);
    assert_eq!(st.coffees(true).unwrap()[0].name, "Mundo Novo 2026");
    let (before, after) = st.sample_neighbors(first, 5).unwrap();
    assert_eq!((before, after), (None, None));
}

#[test]
fn filters_roasts_by_coffee_charge_time_and_text() {
    let mut st = Store::open_in_memory().unwrap();
    let guji = st.add_coffee("Ethiopia Guji 50%").unwrap();
    let cerrado = st.add_coffee("Brazil Cerrado").unwrap();
    let mut roast = |at: Millis, coffee: Uuid| {
        let id = st.start_roast(at, Source::Manual, &[]).unwrap();
        st.update_roast(
            id,
            &RoastPatch {
                coffee_id: Some(Some(coffee)),
                ..RoastPatch::default()
            },
        )
        .unwrap();
        id
    };
    let first = roast(1_000, guji);
    let second = roast(2_000, cerrado);
    let third = roast(3_000, guji);
    st.add_note(second, 2_500, "crash after first crack")
        .unwrap();
    let ids = |q: RoastQuery| -> Vec<RoastId> {
        st.roasts(&q, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect()
    };
    let by_text = |text: &str| RoastQuery {
        text: Some(text.into()),
        ..RoastQuery::default()
    };
    assert_eq!(
        ids(RoastQuery {
            coffee_id: Some(guji),
            ..RoastQuery::default()
        }),
        [third, first]
    );
    assert_eq!(
        ids(RoastQuery {
            since: Some(2_000),
            until: Some(3_000),
            ..RoastQuery::default()
        }),
        [second],
        "since inclusive, until exclusive, on Charge"
    );
    assert_eq!(ids(by_text("CRASH")), [second], "notes, any case");
    assert_eq!(ids(by_text("cerrado")), [second], "coffee name");
    assert_eq!(ids(by_text("#1")), [first], "number");
    assert_eq!(ids(by_text("50%")), [third, first], "% is literal");
    assert_eq!(ids(by_text("5_%")), [], "_ is literal");
    assert_eq!(
        ids(by_text("  ")),
        [third, second, first],
        "blank is no filter"
    );
}

#[test]
fn keeps_coffee_details_and_figures_over_finished_roasts() {
    let mut st = Store::open_in_memory().unwrap();
    let coffee = st.add_coffee("Guji").unwrap();
    let changed = st
        .update_coffee(
            coffee,
            &CoffeeChange {
                lot: Some("2401".into()),
                process: Some("Natural".into()),
                ..CoffeeChange::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            changed.name.as_str(),
            changed.lot.as_str(),
            changed.process.as_str()
        ),
        ("Guji", "2401", "Natural"),
        "only the fields given change"
    );
    let mut done = |charge: Millis, green: i64, roasted: i64| {
        let id = st.start_roast(charge, Source::Manual, &[]).unwrap();
        st.set_event(id, EventKind::FirstCrack, charge + 480_000, Source::Manual)
            .unwrap();
        st.set_event(id, EventKind::Drop, charge + 600_000, Source::Manual)
            .unwrap();
        st.close_roast(id, Closure::AfterDrop).unwrap();
        st.update_roast(
            id,
            &RoastPatch {
                coffee_id: Some(Some(coffee)),
                green_weight_g: Some(Some(green)),
                roasted_weight_g: Some(Some(roasted)),
                ..RoastPatch::default()
            },
        )
        .unwrap();
        id
    };
    done(0, 5000, 4250);
    let second = done(1_000_000, 5000, 4150);
    let recording = st.start_roast(2_000_000, Source::Manual, &[]).unwrap();
    st.update_roast(
        recording,
        &RoastPatch {
            coffee_id: Some(Some(coffee)),
            green_weight_g: Some(Some(5000)),
            ..RoastPatch::default()
        },
    )
    .unwrap();
    let stats = st.coffee_stats().unwrap();
    assert_eq!(stats.len(), 1);
    let s = &stats[0];
    assert_eq!(
        (
            s.coffee_id,
            s.roasts,
            s.last_number,
            s.last_charge_at,
            s.green_weight_g
        ),
        (
            coffee,
            2,
            st.roast(second).unwrap().number,
            Some(1_000_000),
            10_000
        ),
        "the roast still recording does not count"
    );
    assert!((s.average_development_time_ratio.unwrap() - 0.2).abs() < 1e-9);
    assert!((s.average_weight_loss_ratio.unwrap() - 0.16).abs() < 1e-9);
}

#[test]
fn logs_link_drops_newest_first() {
    let mut st = Store::open_in_memory().unwrap();
    let roast = st.start_roast(0, Source::Manual, &[]).unwrap();
    let first = st.begin_link_drop(1_000, "timed out", Some(roast)).unwrap();
    st.end_link_drop(first, 4_000).unwrap();
    st.end_link_drop(first, 9_000).unwrap();
    st.begin_link_drop(20_000, "refused", None).unwrap();
    let drops = st.link_drops(10).unwrap();
    assert_eq!(
        drops,
        [
            LinkDrop {
                started_at: 20_000,
                ended_at: None,
                error: "refused".into(),
                roast: None,
            },
            LinkDrop {
                started_at: 1_000,
                ended_at: Some(4_000),
                error: "timed out".into(),
                roast: Some(roast),
            },
        ],
        "an ended drop keeps its first end"
    );
}
