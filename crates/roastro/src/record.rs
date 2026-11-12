//! Connects the recorder to the database: every [`Output`] becomes a write.

use recorder::{Input, Output, Recorder, Resume, Rules};
use roast::EventKind;
use store::{RoastId, Store};

/// The recorder, the database and the roast currently receiving samples.
pub struct Recording {
    store: Store,
    recorder: Recorder,
    current: Option<RoastId>,
}

impl Recording {
    /// Starts recording on `store`, picking up a roast that was still
    /// Recording when Roastro stopped (a restart is a Gap like any other).
    pub fn open(store: Store, rules: Rules) -> anyhow::Result<Self> {
        let Some(id) = store.recording_roast()? else {
            return Ok(Self {
                store,
                recorder: Recorder::new(rules),
                current: None,
            });
        };
        let roast = store.roast(id)?;
        let at = |kind| roast.events.iter().find(|e| e.kind == kind).map(|e| e.at);
        let charge_at = at(EventKind::Charge).unwrap_or_default();
        let resume = Resume {
            charge_at,
            last_at: store.last_sample_at(id)?.unwrap_or(charge_at),
            drop_at: at(EventKind::Drop),
            turning_point_known: at(EventKind::TurningPoint).is_some(),
        };
        tracing::info!(
            roast = roast.number,
            "resuming the roast that was recording"
        );
        Ok(Self {
            store,
            recorder: Recorder::resume(rules, resume),
            current: Some(id),
        })
    }

    /// Feeds one input through the recorder and writes what it decides.
    pub fn handle(&mut self, input: Input) -> anyhow::Result<()> {
        for out in self.recorder.handle(input) {
            self.apply(out)?;
        }
        Ok(())
    }

    fn apply(&mut self, out: Output) -> anyhow::Result<()> {
        match out {
            Output::Started {
                charge_at,
                source,
                lead_in,
            } => {
                let id = self.store.start_roast(charge_at, source, &lead_in)?;
                let number = self.store.roast(id)?.number;
                tracing::info!(
                    roast = number,
                    source = source.as_str(),
                    lead_in = lead_in.len(),
                    "charge"
                );
                self.current = Some(id);
            }
            Output::Sample(s) => {
                if let Some(id) = self.current {
                    self.store.add_sample(id, &s)?;
                }
            }
            Output::Event { kind, at, source } => {
                if let Some(id) = self.current {
                    self.store.set_event(id, kind, at, source)?;
                    tracing::info!(
                        event = kind.as_str(),
                        source = source.as_str(),
                        "roast event"
                    );
                }
            }
            Output::Closed(closure) => {
                if let Some(id) = self.current.take() {
                    self.store.close_roast(id, closure)?;
                    tracing::info!(?closure, "roast closed");
                }
            }
        }
        Ok(())
    }

    /// The database, for reading.
    #[cfg(test)]
    pub fn store(&self) -> &Store {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use recorder::Sample;
    use store::RoastState;

    use super::*;

    fn reading(at_s: i64, bt: f64) -> Input {
        Input::Sample(Sample {
            at: at_s * 1000,
            values: [("bt".to_owned(), bt)].into(),
        })
    }

    #[test]
    fn records_a_roast_from_readings() {
        let mut rec = Recording::open(Store::open_in_memory().unwrap(), Rules::default()).unwrap();
        for t in 0..=60 {
            rec.handle(reading(t, 200.0)).unwrap();
        }
        rec.handle(Input::ChargePressed(60_000)).unwrap();
        let id = rec.current.expect("charge started a roast");
        for t in 61..=300 {
            rec.handle(reading(t, 150.0)).unwrap();
        }
        rec.handle(Input::DropPressed(300_000)).unwrap();
        for t in 301..=340 {
            rec.handle(reading(t, 150.0)).unwrap();
        }

        let st = rec.store();
        assert_eq!(st.recording_roast().unwrap(), None);
        assert_eq!(rec.current, None, "the tail is over");
        let roast = st.roast(id).unwrap();
        assert_eq!(roast.state, RoastState::Done);
        let samples = st.samples(id).unwrap();
        assert_eq!(samples.first().map(|s| s.at), Some(0), "60 s lead-in");
        assert_eq!(samples.last().map(|s| s.at), Some(330_000), "30 s tail");
    }

    #[test]
    fn picks_up_a_roast_recording_before_a_restart() {
        let mut st = Store::open_in_memory().unwrap();
        let id = st.start_roast(0, roast::Source::Manual, &[]).unwrap();
        let mut rec = Recording::open(st, Rules::default()).unwrap();
        assert_eq!(rec.current, Some(id));
        rec.handle(reading(30, 120.0)).unwrap();
        assert_eq!(
            rec.store().samples(id).unwrap().len(),
            1,
            "the reading joined the resumed roast"
        );
    }
}
