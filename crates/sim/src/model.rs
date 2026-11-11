use std::fmt;
use std::str::FromStr;

use serde::Serialize;

const P_MAX_W: f64 = 8886.0; // 7,641 kcal/h (LPG), from the manual
const BURNER_EFF: f64 = 0.80;
const DRUM_CAP: f64 = 25_000.0; // J/K, drum + housing mass
const LOSS_W: f64 = 5.0; // W/K loss through the housing
const EXHAUST_FRAC: f64 = 0.12; // fraction of hot air that actually carries heat away
const SAFETY_LIMIT: f64 = 280.0; // "Limite segurança" on the HMI options screen
const LATENT_J_KG: f64 = 2.26e6;
const MAX_EVENTS: usize = 200;

/// Something notable that happened inside the simulator, at simulated time `t`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    /// Simulated seconds since the start.
    pub t: f64,
    /// Short name, e.g. `charge` or `safety_trip`.
    pub name: String,
    /// Details, if any.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub info: String,
}

/// Why the simulated PLC refused an action.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The register map has no such point in the simulator.
    #[error("unknown point {0:?}")]
    UnknownPoint(String),
    /// The point exists but cannot be written.
    #[error("point {0:?} is not writable")]
    NotWritable(String),
    /// The emergency stop is pressed.
    #[error("emergency stop engaged")]
    Emergency,
    /// Charge with coffee already in the drum.
    #[error("drum already loaded")]
    AlreadyLoaded,
    /// Drop with an empty drum.
    #[error("drum is empty")]
    Empty,
    /// Charge outside the machine's capacity.
    #[error("batch size out of range (0-6 kg)")]
    BatchOutOfRange,
    /// No fault with that name.
    #[error("unknown fault {0:?} (use emergency|flame)")]
    UnknownFault(String),
}

/// A fault that can be injected to test the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The emergency stop button.
    Emergency,
    /// Flame failure: the gas valve closes.
    Flame,
}

impl FromStr for Fault {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "emergency" => Ok(Fault::Emergency),
            "flame" => Ok(Fault::Flame),
            _ => Err(Error::UnknownFault(s.to_owned())),
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Fault::Emergency => "emergency",
            Fault::Flame => "flame",
        })
    }
}

/// The simulated roaster and its PLC: a lumped-parameter thermal model tuned
/// to produce curves that look like a real roast (turning point, declining
/// rate of rise, ET dip at charge, first crack around 196 °C). It is not exact
/// physics.
///
/// It has no lock of its own: share it behind a mutex.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // they mirror the PLC's coils and inputs
pub struct Sim {
    // Commands (what the PLC receives).
    gas_cmd: f64,
    drum: f64,
    fan: f64,
    roast_point: f64,
    burner: bool,
    drum_on: bool,
    fan_on: bool,
    stirrer: bool,
    cooler: bool,

    // Injectable faults.
    emergency: bool,
    flame_fault: bool,

    // Physical state.
    t_amb: f64,
    t_drum: f64,
    t_bean: f64,
    bt_probe: f64,
    et_probe: f64,
    gas_fb: f64,
    loaded: bool,
    bean_kg: f64,
    water_kg: f64,
    dry_kg: f64,
    charge_kg: f64,
    cooler_t: f64,

    // PLC state.
    t: f64,
    charge_at: f64,
    last_timer: f64,
    running: bool,
    bell: bool,
    trip: bool,
    hopper_until: f64,

    events: Vec<Event>,
    rng: crate::rng::Rng,
}

impl Sim {
    /// Returns a cold machine. The seed drives the probe noise.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::cold(crate::rng::Rng::new(seed))
    }

    fn cold(rng: crate::rng::Rng) -> Self {
        Self {
            gas_cmd: 0.0,
            drum: 80.0,
            fan: 45.0,
            roast_point: 210.0,
            burner: false,
            drum_on: false,
            fan_on: false,
            stirrer: false,
            cooler: false,
            emergency: false,
            flame_fault: false,
            t_amb: 25.0,
            t_drum: 25.0,
            t_bean: 25.0,
            bt_probe: 25.0,
            et_probe: 25.0,
            gas_fb: 0.0,
            loaded: false,
            bean_kg: 0.0,
            water_kg: 0.0,
            dry_kg: 0.0,
            charge_kg: 0.0,
            cooler_t: 25.0,
            t: 0.0,
            charge_at: 0.0,
            last_timer: 0.0,
            running: false,
            bell: false,
            trip: false,
            hopper_until: 0.0,
            events: Vec::new(),
            rng,
        }
    }

    /// Returns the machine to a cold, empty state.
    pub fn reset(&mut self) {
        *self = Self::cold(self.rng.clone());
        self.event("reset", String::new());
    }

    fn event(&mut self, name: &str, info: String) {
        self.events.push(Event {
            t: self.t,
            name: name.to_owned(),
            info,
        });
        if self.events.len() > MAX_EVENTS {
            let excess = self.events.len() - MAX_EVENTS;
            self.events.drain(..excess);
        }
    }

    /// Advances the simulation by `dt` seconds of simulated time.
    pub fn step(&mut self, mut dt: f64) {
        const H: f64 = 0.1;
        while dt > 1e-9 {
            let d = H.min(dt);
            self.step_once(d);
            dt -= d;
        }
    }

    #[allow(clippy::too_many_lines)] // one pass of the physics reads best in one place
    fn step_once(&mut self, dt: f64) {
        self.t += dt;

        // Interlocks a roaster PLC typically has.
        if self.emergency {
            if self.burner || self.drum_on || self.fan_on || self.stirrer || self.cooler {
                self.event("emergency_stop", "everything off".into());
            }
            (
                self.burner,
                self.drum_on,
                self.fan_on,
                self.stirrer,
                self.cooler,
            ) = (false, false, false, false, false);
        }
        if !self.trip && (self.bt_probe > SAFETY_LIMIT || self.et_probe > SAFETY_LIMIT) {
            self.trip = true;
            let info = format!("BT={:.0} ET={:.0}", self.bt_probe, self.et_probe);
            self.event("safety_trip", info);
        } else if self.trip
            && self.bt_probe < SAFETY_LIMIT - 20.0
            && self.et_probe < SAFETY_LIMIT - 20.0
        {
            self.trip = false;
            self.event("safety_clear", String::new());
        }
        let firing = self.burner && self.fan_on && self.drum_on && !self.flame_fault && !self.trip;

        // Valve servo: follows the command with lag and a slew-rate limit.
        let target = if firing {
            self.gas_cmd.clamp(0.0, 100.0)
        } else {
            0.0
        };
        let tau = if firing { 1.5 } else { 0.4 }; // safety valves close almost instantly
        let step = (target - self.gas_fb) * dt / tau;
        self.gas_fb += step.clamp(-60.0 * dt, 20.0 * dt);
        let power = if firing {
            self.gas_fb / 100.0 * P_MAX_W
        } else {
            0.0
        };

        // Air.
        let q = if self.fan_on {
            machine::airflow_m3h(self.fan)
        } else {
            10.0
        }; // natural draft
        let mcp = 1.2 * q / 3600.0 * 1005.0; // W/K
        let drum_f = if self.drum_on {
            0.6 + 0.4 * (self.drum / 100.0).clamp(0.0, 1.2)
        } else {
            0.3
        };

        // Part of the burner heat reaches the beans directly through hot air
        // (convection); the rest heats the drum, which passes it on by contact.
        let p_in = BURNER_EFF * power;
        let (mut q_beans, mut q_conv) = (0.0, 0.0);
        if self.loaded {
            const T_GAS: f64 = 420.0; // nominal temperature of the hot air entering the drum
            let air_f = (q / 150.0).clamp(0.4, 1.3);
            q_conv = 0.6
                * air_f
                * p_in
                * (self.bean_kg / 5.0).clamp(0.2, 1.2)
                * ((T_GAS - self.t_bean) / (T_GAS - 25.0)).clamp(0.0, 1.0);
            let ub = (10.0 + 0.2 * mcp) * drum_f * (self.bean_kg / 5.0);
            q_beans = ub * (self.t_drum - self.t_bean) + q_conv;
            let mut q_evap = 0.0;
            if self.water_kg > 0.0 && self.t_bean > 100.0 {
                // Simplification: water loss follows the "physical" rate, but its
                // energy cost is capped so the curve does not stall (the model has
                // no temperature gradient inside the bean).
                let rate = 5e-5 * (self.t_bean - 100.0) * self.water_kg; // kg/s
                q_evap = (rate * LATENT_J_KG).min(0.45 * q_beans.max(0.0));
                self.water_kg = (self.water_kg - rate * dt).max(0.0);
            }
            let x = (self.t_bean - 200.0) / 9.0;
            let q_exo = 50.0 * self.dry_kg * (-x * x).exp();
            let heat_cap = self.dry_kg * 1300.0 + self.water_kg * 4186.0;
            self.t_bean += (q_beans - q_evap + q_exo) / heat_cap * dt;
            self.bean_kg = self.dry_kg + self.water_kg;
        }
        self.t_drum += (p_in
            - q_conv
            - (EXHAUST_FRAC * mcp + LOSS_W) * (self.t_drum - self.t_amb)
            - (q_beans - q_conv))
            / DRUM_CAP
            * dt;

        // Probes (thermocouples) with thermal lag.
        let (et_target, bt_target, bt_tau) = if self.loaded {
            (
                self.t_bean + 0.5 * (self.t_drum - self.t_bean),
                self.t_bean,
                38.0,
            )
        } else {
            (
                self.t_amb + (self.t_drum - self.t_amb) * 0.96,
                self.t_amb + (self.t_drum - self.t_amb) * 0.93,
                20.0,
            )
        };
        self.et_probe += (et_target - self.et_probe) * dt / 10.0;
        self.bt_probe += (bt_target - self.bt_probe) * dt / bt_tau;

        // Cooling tray.
        let ct = if self.cooler && self.stirrer {
            45.0
        } else {
            400.0
        };
        self.cooler_t += (self.t_amb - self.cooler_t) * dt / ct;

        if self.running && self.roast_point > 0.0 && self.bt_probe >= self.roast_point && !self.bell
        {
            self.bell = true;
            let info = format!("roast point {:.1} °C", self.roast_point);
            self.event("bell", info);
        }
    }

    /// Charges `kg` of green coffee at `green_t` °C into the drum.
    ///
    /// # Errors
    ///
    /// Fails if the drum is already loaded or the batch is outside 0–6 kg.
    pub fn charge(&mut self, kg: f64, green_t: f64) -> Result<(), Error> {
        if self.loaded {
            return Err(Error::AlreadyLoaded);
        }
        if kg <= 0.0 || kg > 6.0 {
            return Err(Error::BatchOutOfRange);
        }
        self.loaded = true;
        self.charge_kg = kg;
        self.bean_kg = kg;
        self.water_kg = kg * 0.11;
        self.dry_kg = kg - self.water_kg;
        self.t_bean = green_t;
        (self.running, self.bell, self.charge_at) = (true, false, self.t);
        self.hopper_until = self.t + 3.0;
        let info = format!("{kg:.2} kg @ BT {:.1}", self.bt_probe);
        self.event("charge", info);
        Ok(())
    }

    /// Empties the drum into the cooling tray.
    ///
    /// # Errors
    ///
    /// Fails if the drum is empty.
    pub fn drop_batch(&mut self) -> Result<(), Error> {
        if !self.loaded {
            return Err(Error::Empty);
        }
        let loss = (1.0 - self.bean_kg / self.charge_kg) * 100.0;
        let info = format!(
            "BT {:.1}, {:.0}s, weight loss {loss:.1}%",
            self.bt_probe,
            self.t - self.charge_at
        );
        self.event("drop", info);
        self.cooler_t = self.t_bean;
        (self.loaded, self.running) = (false, false);
        self.last_timer = self.t - self.charge_at;
        Ok(())
    }

    /// Skips the warm-up (test shortcut only).
    pub fn preheat(&mut self, t: f64) {
        (self.t_drum, self.bt_probe, self.et_probe) = (t + 15.0, t, t + 7.0);
        self.event("preheat_shortcut", format!("{t:.0} °C"));
    }

    /// Turns an injectable fault on or off.
    pub fn set_fault(&mut self, fault: Fault, on: bool) {
        match fault {
            Fault::Emergency => self.emergency = on,
            Fault::Flame => self.flame_fault = on,
        }
        self.event(&format!("fault_{fault}"), on.to_string());
    }

    fn noise(&mut self) -> f64 {
        self.rng.normal() * 0.25
    }

    /// Returns the value of a register-map point as the PLC would report it,
    /// probe noise included.
    ///
    /// # Errors
    ///
    /// Fails for a name the simulator does not know.
    pub fn get(&mut self, name: &str) -> Result<f64, Error> {
        let value = self
            .value(name)
            .ok_or_else(|| Error::UnknownPoint(name.to_owned()))?;
        let noisy = matches!(name, "bt" | "et");
        Ok(if noisy { value + self.noise() } else { value })
    }

    /// The value of a point without probe noise.
    fn value(&self, name: &str) -> Option<f64> {
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        Some(match name {
            "bt" => self.bt_probe,
            "et" => self.et_probe,
            "gas_cmd" => self.gas_cmd,
            "gas_fb" => self.gas_fb.round(),
            "drum" => self.drum,
            "fan" => self.fan,
            "roast_point" => self.roast_point,
            "timer_s" if self.running => (self.t - self.charge_at).floor(),
            "timer_s" => self.last_timer.floor(),
            "burner" => b(self.burner),
            "drum_on" => b(self.drum_on),
            "fan_on" => b(self.fan_on),
            "stirrer" => b(self.stirrer),
            "cooler" => b(self.cooler),
            "emergency" => b(self.emergency),
            "flame_fault" => b(self.flame_fault),
            "hopper_sensor" => b(self.t < self.hopper_until),
            "bell" => b(self.bell),
            "safety_trip" => b(self.trip),
            _ => return None,
        })
    }

    /// Applies a write, as it would arrive over Modbus.
    ///
    /// # Errors
    ///
    /// Fails for an unknown or read-only point, and refuses to light the
    /// burner during an emergency stop.
    pub fn set(&mut self, name: &str, v: f64) -> Result<(), Error> {
        let on = v != 0.0;
        let before = self.value(name);
        match name {
            "gas_cmd" => self.gas_cmd = v.clamp(0.0, 100.0),
            "drum" => self.drum = v.clamp(0.0, 120.0),
            "fan" => self.fan = v.clamp(0.0, 120.0),
            "roast_point" => self.roast_point = v.clamp(0.0, 260.0),
            "burner" => {
                if on && self.emergency {
                    return Err(Error::Emergency);
                }
                self.burner = on;
            }
            "drum_on" => self.drum_on = on && !self.emergency,
            "fan_on" => self.fan_on = on && !self.emergency,
            "stirrer" => self.stirrer = on && !self.emergency,
            "cooler" => self.cooler = on && !self.emergency,
            _ if before.is_some() => return Err(Error::NotWritable(name.to_owned())),
            _ => return Err(Error::UnknownPoint(name.to_owned())),
        }
        if self.value(name) != before {
            self.event("write", format!("{name}={v}"));
        }
        Ok(())
    }

    /// A consistent copy of the whole state, including the internal "truth"
    /// (actual bean temperature, moisture) to help validate the app's event
    /// detection. The real PLC does not expose it.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let moisture = if self.loaded && self.bean_kg > 0.0 {
            self.water_kg / self.bean_kg * 100.0
        } else {
            0.0
        };
        Snapshot {
            sim_time: self.t,
            bt: r1(self.bt_probe),
            et: r1(self.et_probe),
            gas_cmd: self.gas_cmd,
            gas_fb: r1(self.gas_fb),
            drum: self.drum,
            drum_rpm: r1(machine::drum_rpm(self.drum)),
            fan: self.fan,
            airflow_m3h: r1(machine::airflow_m3h(self.fan)),
            burner: self.burner,
            drum_on: self.drum_on,
            fan_on: self.fan_on,
            stirrer: self.stirrer,
            cooler: self.cooler,
            loaded: self.loaded,
            bell: self.bell,
            safety_trip: self.trip,
            emergency: self.emergency,
            flame_fault: self.flame_fault,
            true_bean_t: r1(self.t_bean),
            true_drum_t: r1(self.t_drum),
            moisture_pct: r1(moisture),
            cooler_bean_t: r1(self.cooler_t),
            roast_elapsed: if self.running {
                r1(self.t - self.charge_at)
            } else {
                0.0
            },
            events: self.events.clone(),
        }
    }
}

/// The whole simulator state at one instant, as served on `/state`.
#[derive(Debug, Clone, Serialize)]
#[allow(clippy::struct_excessive_bools)] // they mirror the PLC's coils and inputs
#[allow(missing_docs)] // field names are the documentation; they match the JSON
pub struct Snapshot {
    #[serde(rename = "sim_time_s")]
    pub sim_time: f64,
    pub bt: f64,
    pub et: f64,
    pub gas_cmd: f64,
    pub gas_fb: f64,
    #[serde(rename = "drum_pct")]
    pub drum: f64,
    pub drum_rpm: f64,
    #[serde(rename = "fan_pct")]
    pub fan: f64,
    pub airflow_m3h: f64,
    pub burner: bool,
    pub drum_on: bool,
    pub fan_on: bool,
    pub stirrer: bool,
    pub cooler: bool,
    pub loaded: bool,
    pub bell: bool,
    pub safety_trip: bool,
    pub emergency: bool,
    pub flame_fault: bool,
    #[serde(rename = "truth_bean_temp")]
    pub true_bean_t: f64,
    #[serde(rename = "truth_drum_temp")]
    pub true_drum_t: f64,
    #[serde(rename = "truth_moisture_pct")]
    pub moisture_pct: f64,
    #[serde(rename = "truth_cooler_bean_temp")]
    pub cooler_bean_t: f64,
    #[serde(rename = "roast_elapsed_s")]
    pub roast_elapsed: f64,
    pub events: Vec<Event>,
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Autopilot;

    /// Guards that the curve still looks like a roast when someone tunes the parameters.
    #[test]
    fn roast_looks_real() {
        let mut s = Sim::new(1);
        let mut ap = Autopilot::default();
        let (mut charge_at, mut tp, mut tp_t, mut drop_at) = (None, f64::INFINITY, 0.0, None);
        for i in 0..2 * 3600 * 10 {
            s.step(0.1);
            if i % 10 == 0 {
                ap.tick(&mut s);
            }
            let sn = s.snapshot();
            if sn.loaded && charge_at.is_none() {
                charge_at = Some(sn.sim_time);
            }
            if let Some(c) = charge_at {
                if sn.loaded && sn.bt < tp {
                    (tp, tp_t) = (sn.bt, sn.sim_time - c);
                }
                if !sn.loaded {
                    drop_at = Some(sn.sim_time - c);
                    break;
                }
            }
        }
        let drop_at = drop_at.expect("autopilot did not complete a roast");
        assert!(
            (70.0..=105.0).contains(&tp) && (50.0..=120.0).contains(&tp_t),
            "turning point {tp:.1} °C at {tp_t:.0}s"
        );
        assert!(
            (480.0..=900.0).contains(&drop_at),
            "roast duration {drop_at:.0}s"
        );
    }

    /// A machine with the burner lit at 80% gas for 10 s.
    fn firing() -> Sim {
        let mut s = Sim::new(1);
        for name in ["drum_on", "fan_on", "burner"] {
            s.set(name, 1.0).unwrap();
        }
        s.set("gas_cmd", 80.0).unwrap();
        s.step(10.0);
        s
    }

    #[test]
    fn emergency_cuts_everything() {
        let mut s = firing();
        s.set_fault(Fault::Emergency, true);
        s.step(5.0);
        let sn = s.snapshot();
        assert!(
            !sn.burner && !sn.fan_on && !sn.drum_on && sn.gas_fb <= 1.0,
            "{sn:?}"
        );
        assert_eq!(s.set("burner", 1.0), Err(Error::Emergency));
    }

    #[test]
    fn flame_fault_closes_gas() {
        let mut s = firing();
        s.set_fault(Fault::Flame, true);
        s.step(10.0);
        assert!(s.snapshot().gas_fb <= 1.0);
    }

    #[test]
    fn refuses_bad_writes() {
        let mut s = Sim::new(1);
        assert_eq!(s.set("bt", 100.0), Err(Error::NotWritable("bt".into())));
        assert_eq!(s.set("nope", 1.0), Err(Error::UnknownPoint("nope".into())));
        assert_eq!(
            "smoke".parse::<Fault>(),
            Err(Error::UnknownFault("smoke".into()))
        );
    }
}
