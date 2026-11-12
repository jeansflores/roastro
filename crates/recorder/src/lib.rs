//! Turns the stream of PLC readings into roasts.
//!
//! The [`Recorder`] is a pure state machine: it is fed [`Input`]s (readings,
//! missed polls, the operator's Charge and Drop buttons) and answers with
//! [`Output`]s (a roast started, a sample belongs to it, an event happened, it
//! closed). It does no I/O and never reads a clock, so every lifecycle rule
//! can be tested with recorded or simulated readings.
//!
//! The rules, decided in "Roast lifecycle" and "Recording through a PLC
//! dropout or an app restart":
//!
//! - **Charge** is the first of: the Charge button, the PLC timer starting to
//!   count from (nearly) zero, or BT dropping by [`Rules::charge_drop`] within
//!   [`Rules::charge_window`] from above [`Rules::charge_min_bt`], from a BT
//!   that was steady or rising before (so an empty drum cooling after Drop is
//!   not a Charge). Later signals only confirm it.
//! - **Drop** is the first of: the Drop button, the PLC timer stopping after
//!   it ran in this roast, or BT
//!   dropping by [`Rules::drop_drop`] within [`Rules::drop_window`], only after
//!   [`Rules::drop_min_roast`] of roast.
//! - **Turning point** is the lowest BT after Charge, emitted once BT has
//!   risen [`Rules::turning_point_rise`] above it.
//! - A roast keeps [`Rules::lead_in`] of readings before Charge and
//!   [`Rules::tail`] after Drop.
//! - A **Gap** is two readings more than [`Rules::gap`] apart. Detection never
//!   looks across one. A Gap longer than [`Rules::max_gap`] closes the roast as
//!   Incomplete. A Charge while a roast is recording closes it as Incomplete.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use roast::{EventKind, Source};

/// Epoch milliseconds, as stored and sent on the wire (ADR 0001).
pub type Millis = i64;

/// One reading of every register-map point, by name.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// When it was read.
    pub at: Millis,
    /// Physical value per point name.
    pub values: HashMap<String, f64>,
}

impl Sample {
    fn bt(&self) -> Option<f64> {
        self.values.get("bt").copied()
    }

    fn timer(&self) -> Option<f64> {
        self.values.get("timer_s").copied()
    }
}

/// Thresholds for the lifecycle rules. Defaults follow Cropster's published
/// examples and are to be tuned with real roasts.
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    /// Readings kept before Charge.
    pub lead_in: Duration,
    /// Readings kept after Drop.
    pub tail: Duration,
    /// Readings further apart than this have a Gap between them.
    pub gap: Duration,
    /// A Gap longer than this closes the roast as Incomplete.
    pub max_gap: Duration,
    /// BT fall that means Charge…
    pub charge_drop: f64,
    /// …within this window…
    pub charge_window: Duration,
    /// …starting from a BT at least this hot (°C).
    pub charge_min_bt: f64,
    /// BT fall that means Drop…
    pub drop_drop: f64,
    /// …within this window…
    pub drop_window: Duration,
    /// …but only after this much roast.
    pub drop_min_roast: Duration,
    /// BT rise above the minimum that confirms the turning point (°C).
    pub turning_point_rise: f64,
    /// The PLC timer counts as started only from at most this much…
    pub timer_fresh: Duration,
    /// …and as stopped only after standing still this long.
    pub timer_stall: Duration,
    /// Simulator speed: roast-time rules are scaled by it. 1 for the machine.
    pub time_scale: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            lead_in: Duration::from_secs(60),
            tail: Duration::from_secs(30),
            gap: Duration::from_millis(2500),
            max_gap: Duration::from_secs(300),
            charge_drop: 7.0,
            charge_window: Duration::from_secs(30),
            charge_min_bt: 150.0,
            drop_drop: 3.0,
            drop_window: Duration::from_secs(5),
            drop_min_roast: Duration::from_secs(180),
            turning_point_rise: 2.0,
            timer_fresh: Duration::from_secs(5),
            timer_stall: Duration::from_secs(2),
            time_scale: 1.0,
        }
    }
}

impl Rules {
    /// A roast-time duration as wall-clock milliseconds.
    #[allow(clippy::cast_possible_truncation)] // durations here are minutes, far below i64::MAX ms
    fn roast_ms(&self, d: Duration) -> Millis {
        (d.as_secs_f64() * 1000.0 / self.time_scale.max(f64::MIN_POSITIVE)) as Millis
    }

    /// A wall-clock duration in milliseconds.
    #[allow(clippy::cast_possible_truncation)]
    fn wall_ms(d: Duration) -> Millis {
        d.as_millis() as Millis
    }
}

/// Something that happened to the recorder.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A reading.
    Sample(Sample),
    /// A poll failed at this time: no reading, but time went on.
    Missed(Millis),
    /// The operator pressed Charge at this time.
    ChargePressed(Millis),
    /// The operator pressed Drop at this time.
    DropPressed(Millis),
}

/// Why a roast stopped receiving samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closure {
    /// The tail after Drop is complete; the roast is Done.
    AfterDrop,
    /// No readings for longer than [`Rules::max_gap`]; the roast is Incomplete.
    Gap,
    /// A new Charge arrived before any Drop; the roast is Incomplete.
    NewCharge,
}

impl Closure {
    /// Whether the roast ends up Incomplete.
    #[must_use]
    pub fn is_incomplete(self) -> bool {
        !matches!(self, Closure::AfterDrop)
    }
}

/// What the caller must record.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// A roast began. Its Charge event is `charge_at`/`source`; `lead_in`
    /// holds the readings kept from before it.
    Started {
        /// When Charge happened.
        charge_at: Millis,
        /// How it was decided.
        source: Source,
        /// Readings before Charge, oldest first.
        lead_in: Vec<Sample>,
    },
    /// A reading that belongs to the current roast.
    Sample(Sample),
    /// An event of the current roast (never Charge: see [`Output::Started`]).
    Event {
        /// Which event.
        kind: EventKind,
        /// When it happened.
        at: Millis,
        /// How it was decided.
        source: Source,
    },
    /// The current roast stopped receiving samples.
    Closed(Closure),
}

#[derive(Debug)]
enum Phase {
    Waiting,
    /// Between Charge and Drop.
    Roasting(Roast),
    /// After Drop, until the tail is complete.
    Tail {
        drop_at: Millis,
    },
}

#[derive(Debug)]
struct Roast {
    charge_at: Millis,
    /// Whether the PLC timer advanced during this roast.
    timer_ran: bool,
    /// Lowest BT so far and when; `None` once the turning point was emitted.
    turning_point: Option<(f64, Millis)>,
}

/// The lifecycle state machine. See the crate docs for the rules.
#[derive(Debug)]
pub struct Recorder {
    rules: Rules,
    phase: Phase,
    /// Readings over the last [`Rules::lead_in`], whatever the phase.
    recent: VecDeque<Sample>,
    /// Readings since the last Gap, for the detection windows.
    contiguous: VecDeque<Sample>,
    last_at: Option<Millis>,
    /// PLC timer on the previous contiguous reading, if the map has one.
    prev_timer: Option<f64>,
    /// When the PLC timer stopped advancing, if it has.
    timer_stalled_at: Option<Millis>,
}

impl Recorder {
    /// A recorder waiting for the first Charge.
    #[must_use]
    pub fn new(rules: Rules) -> Self {
        Self {
            rules,
            phase: Phase::Waiting,
            recent: VecDeque::new(),
            contiguous: VecDeque::new(),
            last_at: None,
            prev_timer: None,
            timer_stalled_at: None,
        }
    }

    /// Whether a roast is receiving samples (between Charge and the end of
    /// its tail).
    #[must_use]
    pub fn is_recording(&self) -> bool {
        !matches!(self.phase, Phase::Waiting)
    }

    /// Whether Drop has not happened yet in the current roast.
    #[must_use]
    pub fn is_before_drop(&self) -> bool {
        matches!(self.phase, Phase::Roasting(_))
    }

    /// Applies one input and returns what to record, in order.
    pub fn handle(&mut self, input: Input) -> Vec<Output> {
        let mut out = Vec::new();
        match input {
            Input::Sample(s) => self.sample(&s, &mut out),
            Input::Missed(at) => self.close_if_gap_too_long(at, &mut out),
            Input::ChargePressed(at) => self.charge(at, Source::Manual, &mut out),
            Input::DropPressed(at) => {
                if self.is_before_drop() {
                    self.drop(at, Source::Manual, &mut out);
                }
            }
        }
        out
    }

    fn sample(&mut self, s: &Sample, out: &mut Vec<Output>) {
        self.close_if_gap_too_long(s.at, out);
        if self
            .last_at
            .is_some_and(|last| s.at - last > Rules::wall_ms(self.rules.gap))
        {
            // A Gap: detection starts over.
            self.contiguous.clear();
            self.prev_timer = None;
            self.timer_stalled_at = None;
        }
        let timer = s.timer();
        let timer_advanced =
            matches!((self.prev_timer, timer), (Some(prev), Some(now)) if now > prev);
        if timer_advanced {
            self.timer_stalled_at = None;
        } else if timer.is_some() && self.prev_timer.is_some() && self.timer_stalled_at.is_none() {
            self.timer_stalled_at = Some(s.at);
        }
        self.prev_timer = timer;
        self.last_at = Some(s.at);
        self.push_windows(s);

        match &mut self.phase {
            Phase::Waiting => {
                if let Some(at) = timer
                    .filter(|_| timer_advanced)
                    .and_then(|t| self.charge_by_timer(s.at, t))
                {
                    self.charge(at, Source::Auto, out);
                } else if let Some(at) = self.charge_by_bt(s) {
                    self.charge(at, Source::Auto, out);
                }
            }
            Phase::Roasting(roast) => {
                out.push(Output::Sample(s.clone()));
                roast.timer_ran |= timer_advanced;
                if let Some(at) = turning_point(roast, s, self.rules.turning_point_rise) {
                    out.push(Output::Event {
                        kind: EventKind::TurningPoint,
                        at,
                        source: Source::Computed,
                    });
                }
                let (charge_at, timer_ran) = (roast.charge_at, roast.timer_ran);
                let stalled = self.timer_stalled_at.filter(|&t| {
                    timer_ran && s.at - t >= self.rules.roast_ms(self.rules.timer_stall)
                });
                if let Some(at) = stalled {
                    self.drop(at, Source::Auto, out);
                } else if let Some(at) = self.drop_by_bt(charge_at, s) {
                    self.drop(at, Source::Auto, out);
                }
            }
            Phase::Tail { drop_at } => {
                out.push(Output::Sample(s.clone()));
                if s.at - *drop_at >= self.rules.roast_ms(self.rules.tail) {
                    self.phase = Phase::Waiting;
                    out.push(Output::Closed(Closure::AfterDrop));
                }
            }
        }
    }

    /// Charge time from a PLC timer that just started counting; `None` if it
    /// has been running too long (a roast already under way is never inferred).
    #[allow(clippy::cast_possible_truncation)]
    fn charge_by_timer(&self, now: Millis, timer_s: f64) -> Option<Millis> {
        (timer_s <= self.rules.timer_fresh.as_secs_f64()).then(|| {
            now - (timer_s * 1000.0 / self.rules.time_scale.max(f64::MIN_POSITIVE)) as Millis
        })
    }

    fn push_windows(&mut self, s: &Sample) {
        let lead_in = self.rules.roast_ms(self.rules.lead_in);
        let window = self
            .rules
            .roast_ms(self.rules.charge_window.max(self.rules.drop_window));
        self.recent.push_back(s.clone());
        while self.recent.front().is_some_and(|f| s.at - f.at > lead_in) {
            self.recent.pop_front();
        }
        self.contiguous.push_back(s.clone());
        while self
            .contiguous
            .front()
            .is_some_and(|f| s.at - f.at > window)
        {
            self.contiguous.pop_front();
        }
    }

    /// The time BT started falling, if it fell enough, fast enough, from hot
    /// enough, and was not already falling before that.
    fn charge_by_bt(&self, now: &Sample) -> Option<Millis> {
        const STEADY_TOLERANCE: f64 = 1.0; // °C of probe noise allowed above the peak
        let bt = now.bt()?;
        let window = self.rules.roast_ms(self.rules.charge_window);
        let (peak, at) = peak_since(&self.contiguous, now.at - window)?;
        if peak < self.rules.charge_min_bt || peak - bt < self.rules.charge_drop {
            return None;
        }
        // Look at the window before the peak: it must exist (a third of it at
        // least) and never be hotter than the peak.
        let before: Vec<f64> = self
            .contiguous
            .iter()
            .filter(|s| s.at < at && s.at >= at - window)
            .filter_map(Sample::bt)
            .collect();
        let span = self
            .contiguous
            .iter()
            .find(|s| s.at >= at - window)
            .map_or(0, |first| at - first.at);
        let steady = span >= window / 3 && before.iter().all(|&b| b <= peak + STEADY_TOLERANCE);
        steady.then_some(at)
    }

    fn drop_by_bt(&self, charge_at: Millis, now: &Sample) -> Option<Millis> {
        if now.at - charge_at < self.rules.roast_ms(self.rules.drop_min_roast) {
            return None;
        }
        let bt = now.bt()?;
        let window = self.rules.roast_ms(self.rules.drop_window);
        let (peak, at) = peak_since(&self.contiguous, now.at - window)?;
        (peak - bt >= self.rules.drop_drop).then_some(at)
    }

    fn charge(&mut self, at: Millis, source: Source, out: &mut Vec<Output>) {
        match self.phase {
            Phase::Roasting(_) => out.push(Output::Closed(Closure::NewCharge)),
            Phase::Tail { .. } => out.push(Output::Closed(Closure::AfterDrop)),
            Phase::Waiting => {}
        }
        let lead_in = self.rules.roast_ms(self.rules.lead_in);
        let (before, after): (Vec<_>, Vec<_>) = self
            .recent
            .iter()
            .filter(|s| s.at >= at - lead_in)
            .cloned()
            .partition(|s| s.at < at);
        out.push(Output::Started {
            charge_at: at,
            source,
            lead_in: before,
        });
        // Readings from the charge instant on (a detection looks back) belong to the roast.
        out.extend(after.into_iter().map(Output::Sample));
        self.phase = Phase::Roasting(Roast {
            charge_at: at,
            timer_ran: false,
            turning_point: Some((f64::INFINITY, at)),
        });
    }

    fn drop(&mut self, at: Millis, source: Source, out: &mut Vec<Output>) {
        out.push(Output::Event {
            kind: EventKind::Drop,
            at,
            source,
        });
        self.phase = Phase::Tail { drop_at: at };
    }

    fn close_if_gap_too_long(&mut self, at: Millis, out: &mut Vec<Output>) {
        let too_long = self
            .last_at
            .is_some_and(|last| at - last > Rules::wall_ms(self.rules.max_gap));
        if too_long && self.is_recording() {
            let closure = if self.is_before_drop() {
                Closure::Gap
            } else {
                Closure::AfterDrop
            };
            out.push(Output::Closed(closure));
            self.phase = Phase::Waiting;
        }
        if too_long {
            self.recent.clear();
            self.contiguous.clear();
        }
    }
}

/// The highest BT in `window` from `since` on, and when.
fn peak_since(window: &VecDeque<Sample>, since: Millis) -> Option<(f64, Millis)> {
    window
        .iter()
        .filter(|s| s.at >= since)
        .filter_map(|s| s.bt().map(|bt| (bt, s.at)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
}

/// Tracks the lowest BT and returns its time once BT has risen `rise` above it.
fn turning_point(roast: &mut Roast, s: &Sample, rise: f64) -> Option<Millis> {
    let (min, at) = roast.turning_point?;
    let bt = s.bt()?;
    if bt < min {
        roast.turning_point = Some((bt, s.at));
        return None;
    }
    if bt - min >= rise {
        roast.turning_point = None;
        return Some(at);
    }
    None
}

#[cfg(test)]
mod tests;
