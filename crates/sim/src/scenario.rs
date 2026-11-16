//! Scenarios that make the simulated PLC misbehave on cue ("Simulator
//! scenarios to exercise detection", #56), to check that Roastro records
//! roasts as decided when the machine is not ideal. Times are roast seconds
//! (since the simulator's Charge) of a given roast in a run.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use crate::{Fault, Sim};

/// A way the PLC misbehaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Probe noise three times the usual (σ 0.75 °C, a bad thermocouple):
    /// no false Charge or Drop.
    NoisyCharge,
    /// The beans come out with no trace: the PLC timer runs on and BT does
    /// not fall, so the roast is closed by the next Charge.
    MissedDrop,
    /// The PLC stops answering: 20 s in the first roast (a Gap the roast
    /// survives), 6 min in the second (it closes as Incomplete).
    Dropout,
    /// BT freezes for 40 s in the first roast.
    StuckProbe,
    /// Two absurd BT readings in the first roast (999.9 and -3276.8, what a
    /// broken probe sends).
    Spike,
    /// Every answer arrives 400 ms late. A reading of the simulator's map
    /// takes three answers, so readings come about 1.2 s apart.
    Slow,
    /// Flame failure for 20 s, then the emergency stop for 10 s, in the
    /// first roast.
    Alarms,
}

impl Scenario {
    /// Every scenario.
    pub const ALL: [Self; 7] = [
        Self::NoisyCharge,
        Self::MissedDrop,
        Self::Dropout,
        Self::StuckProbe,
        Self::Spike,
        Self::Slow,
        Self::Alarms,
    ];

    /// The name it is asked for by.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::NoisyCharge => "noisy-charge",
            Self::MissedDrop => "missed-drop",
            Self::Dropout => "dropout",
            Self::StuckProbe => "stuck-probe",
            Self::Spike => "spike",
            Self::Slow => "slow",
            Self::Alarms => "alarms",
        }
    }
}

impl fmt::Display for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Scenario {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|x| x.name() == s)
            .ok_or_else(|| {
                let names: Vec<_> = Self::ALL.iter().map(|x| x.name()).collect();
                format!("unknown scenario {s:?}; one of {}", names.join(", "))
            })
    }
}

/// Plays a scenario on a [`Sim`]: call [`Script::tick`] once per simulated
/// second, after stepping the model.
#[derive(Debug, Clone)]
pub struct Script {
    scenario: Scenario,
    /// Roasts charged so far (the current one included).
    roast: u32,
    loaded: bool,
    /// Sim time until which the current effect lasts.
    until: Option<f64>,
    /// Effects already started in this run, by step.
    done: u8,
}

impl Script {
    /// Starts a scenario.
    #[must_use]
    pub fn new(scenario: Scenario) -> Self {
        Self {
            scenario,
            roast: 0,
            loaded: false,
            until: None,
            done: 0,
        }
    }

    /// The scenario played.
    #[must_use]
    pub fn scenario(&self) -> Scenario {
        self.scenario
    }

    /// Starts step `n` (once per run) when `when` holds.
    fn once(&mut self, n: u8, when: bool) -> bool {
        let bit = 1 << n;
        if when && self.done & bit == 0 {
            self.done |= bit;
            true
        } else {
            false
        }
    }

    /// Applies what the scenario does at this instant.
    pub fn tick(&mut self, sim: &mut Sim) {
        let snap = sim.snapshot();
        if snap.loaded && !self.loaded {
            self.roast += 1;
        }
        self.loaded = snap.loaded;
        let now = snap.sim_time;
        let current = self.roast;
        let at = |roast: u32, t: f64| snap.loaded && current == roast && snap.roast_elapsed >= t;
        let lasting = self.until.is_some_and(|u| now < u);
        match self.scenario {
            Scenario::NoisyCharge => sim.set_noise_scale(3.0),
            Scenario::MissedDrop => sim.set_quiet_drop(true),
            Scenario::Slow => sim.set_link_delay(Duration::from_millis(400)),
            Scenario::Dropout => {
                if self.once(0, at(1, 240.0)) {
                    self.until = Some(now + 20.0);
                } else if self.once(1, at(2, 240.0)) {
                    self.until = Some(now + 360.0);
                }
                sim.set_link_silent(self.until.is_some_and(|u| now < u));
            }
            Scenario::StuckProbe => {
                if self.once(0, at(1, 200.0)) {
                    self.until = Some(now + 40.0);
                    sim.set_bt_hold(Some(snap.bt));
                } else if !lasting {
                    sim.set_bt_hold(None);
                }
            }
            Scenario::Spike => {
                if self.once(0, at(1, 300.0)) {
                    sim.spike_bt(999.9);
                } else if self.once(1, at(1, 360.0)) {
                    sim.spike_bt(-3276.8);
                }
            }
            Scenario::Alarms => {
                if self.once(0, at(1, 300.0)) {
                    sim.set_fault(Fault::Flame, true);
                    self.until = Some(now + 20.0);
                } else if self.once(1, self.done & 1 != 0 && !lasting) {
                    sim.set_fault(Fault::Flame, false);
                } else if self.once(2, at(1, 420.0)) {
                    sim.set_fault(Fault::Emergency, true);
                    self.until = Some(now + 10.0);
                } else if self.once(3, self.done & 4 != 0 && !lasting) {
                    sim.set_fault(Fault::Emergency, false);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for s in Scenario::ALL {
            assert_eq!(s.name().parse::<Scenario>(), Ok(s));
        }
        assert!("nope".parse::<Scenario>().unwrap_err().contains("dropout"));
    }
}
