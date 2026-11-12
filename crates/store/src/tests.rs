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
