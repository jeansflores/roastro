use std::fmt;

use crate::Sim;

/// The autopilot never drops before this much roast time.
const MIN_ROAST_S: f64 = 240.0;

/// The step of the roast cycle the autopilot is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing switched on yet.
    Start,
    /// Heating the empty drum up to the charge temperature.
    Preheat,
    /// Coffee in the drum.
    Roast,
    /// Cooling the batch before the next preheat.
    Between,
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Phase::Start => "start",
            Phase::Preheat => "preheat",
            Phase::Roast => "roast",
            Phase::Between => "between",
        })
    }
}

/// Plays the operator: it preheats, charges, adjusts gas and air by
/// temperature band, drops and starts over. It keeps data flowing into the
/// app with nobody at the controls.
#[derive(Debug, Clone)]
pub struct Autopilot {
    /// BT at which to charge.
    pub charge_temp: f64,
    /// BT at which to drop.
    pub drop_temp: f64,
    /// Green coffee per batch.
    pub batch_kg: f64,
    phase: Phase,
    wait_until: f64,
}

impl Default for Autopilot {
    /// Charges 5 kg at 200 °C and drops at 210 °C.
    fn default() -> Self {
        Self {
            charge_temp: 200.0,
            drop_temp: 210.0,
            batch_kg: 5.0,
            phase: Phase::Start,
            wait_until: 0.0,
        }
    }
}

impl Autopilot {
    /// The current step of the roast cycle.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Makes one operator decision. Call it once per simulated second.
    pub fn tick(&mut self, s: &mut Sim) {
        let snap = s.snapshot();
        if snap.emergency || snap.flame_fault {
            return; // injected fault: the "operator" waits for you to clear it
        }
        // The only refusal `Sim::set` can produce for these points is the
        // emergency stop, checked above.
        let mut set = |name: &str, v: f64| {
            let _ = s.set(name, v);
        };
        match self.phase {
            Phase::Start => {
                set("drum_on", 1.0);
                set("fan_on", 1.0);
                set("drum", 80.0);
                set("fan", 35.0);
                set("gas_cmd", 70.0);
                set("roast_point", self.drop_temp);
                set("burner", 1.0);
                self.phase = Phase::Preheat;
            }
            Phase::Preheat => {
                // Hold BT near the charge temperature.
                if snap.bt < self.charge_temp - 10.0 {
                    set("gas_cmd", 75.0);
                } else if snap.bt < self.charge_temp {
                    set("gas_cmd", 50.0);
                } else {
                    set("gas_cmd", 80.0);
                    set("fan", 45.0);
                    if s.charge(self.batch_kg, 25.0).is_ok() {
                        self.phase = Phase::Roast;
                    }
                }
            }
            Phase::Roast => {
                // The probe is misleading until the turning point.
                if snap.roast_elapsed < 90.0 || snap.bt < 150.0 {
                    set("gas_cmd", 80.0);
                } else if snap.bt < 185.0 {
                    set("gas_cmd", 70.0);
                    set("fan", 55.0);
                } else {
                    set("gas_cmd", 50.0);
                    set("fan", 65.0);
                }
                // A drum still hot from the last batch can read above the drop
                // temperature right after charge; no real roast is that short.
                let roasted_long_enough = snap.roast_elapsed >= MIN_ROAST_S;
                if roasted_long_enough && snap.bt >= self.drop_temp && s.drop_batch().is_ok() {
                    let mut set = |name: &str, v: f64| {
                        let _ = s.set(name, v);
                    };
                    set("stirrer", 1.0);
                    set("cooler", 1.0);
                    set("gas_cmd", 40.0);
                    set("fan", 35.0);
                    self.wait_until = snap.sim_time + 240.0;
                    self.phase = Phase::Between;
                }
            }
            Phase::Between => {
                if snap.sim_time >= self.wait_until {
                    set("stirrer", 0.0);
                    set("cooler", 0.0);
                    self.phase = Phase::Preheat;
                }
            }
        }
    }
}
