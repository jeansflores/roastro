use std::collections::VecDeque;
use std::time::Duration;

/// Rate of rise of a temperature in °C/min over a sliding time window.
///
/// It fits a least-squares line through the samples in the window, which is
/// far less sensitive to probe noise than comparing the two endpoints.
#[derive(Debug, Clone)]
pub struct RoR {
    window: Duration,
    samples: VecDeque<Sample>,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    t: Duration, // roast clock
    celsius: f64,
}

impl RoR {
    /// Creates a rate of rise that looks back over `window`.
    #[must_use]
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            samples: VecDeque::new(),
        }
    }

    /// Records temperature `celsius` at roast-clock time `t`.
    ///
    /// Samples must arrive in increasing `t`. A sample that does not move
    /// forward resets the history, which is what happens when the roast clock
    /// restarts.
    pub fn add(&mut self, t: Duration, celsius: f64) {
        if self.samples.back().is_some_and(|last| t <= last.t) {
            self.samples.clear();
        }
        self.samples.push_back(Sample { t, celsius });

        // Drop samples that fell out of the window, keeping at least two.
        while self.samples.len() > 2 && t.saturating_sub(self.samples[0].t) > self.window {
            self.samples.pop_front();
        }
    }

    /// Returns the current rate of rise in °C/min, or `None` until there are
    /// at least two samples to fit a line through.
    #[must_use]
    pub fn value(&self) -> Option<f64> {
        if self.samples.len() < 2 {
            return None;
        }
        // Least-squares slope: Σ(dx·dy) / Σ(dx²), with x in minutes.
        #[allow(clippy::cast_precision_loss)] // a window holds a handful of samples
        let n = self.samples.len() as f64;
        let minutes = |s: &Sample| s.t.as_secs_f64() / 60.0;
        let mean_x = self.samples.iter().map(minutes).sum::<f64>() / n;
        let mean_y = self.samples.iter().map(|s| s.celsius).sum::<f64>() / n;
        let (num, den) = self.samples.iter().fold((0.0, 0.0), |(num, den), s| {
            let dx = minutes(s) - mean_x;
            (num + dx * (s.celsius - mean_y), den + dx * dx)
        });
        (den != 0.0).then(|| num / den)
    }

    /// Forgets all samples.
    pub fn reset(&mut self) {
        self.samples.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_secs(30);

    /// A temperature that rises `per_min` °C/min from `base`.
    fn ramp(base: f64, per_min: f64) -> impl Fn(Duration) -> f64 {
        move |t| base + per_min * t.as_secs_f64() / 60.0
    }

    /// Feeds one sample per second, starting at t = 1 s.
    fn feed(samples: u64, temp: impl Fn(Duration) -> f64) -> RoR {
        let mut ror = RoR::new(WINDOW);
        for i in 1..=samples {
            let t = Duration::from_secs(i);
            ror.add(t, temp(t));
        }
        ror
    }

    fn assert_close(got: Option<f64>, want: f64) {
        let got = got.expect("want a value");
        assert!((got - want).abs() < 1e-9, "got {got}, want {want}");
    }

    #[test]
    fn needs_two_samples() {
        assert_eq!(feed(0, ramp(100.0, 10.0)).value(), None);
        assert_eq!(feed(1, ramp(100.0, 10.0)).value(), None);
        assert_close(feed(2, ramp(100.0, 10.0)).value(), 10.0);
    }

    #[test]
    fn follows_a_steady_ramp() {
        for (name, per_min) in [
            ("flat", 0.0),
            ("rising", 12.5),
            ("falling, before the turning point", -40.0),
        ] {
            let got = feed(60, ramp(150.0, per_min)).value();
            assert!(
                got.is_some_and(|v| (v - per_min).abs() < 1e-9),
                "{name}: got {got:?}, want {per_min}"
            );
        }
    }

    #[test]
    fn only_the_window_counts() {
        // 20 °C/min for the first minute, then 5 °C/min.
        let temp = |t: Duration| {
            let min = t.as_secs_f64() / 60.0;
            if min <= 1.0 {
                100.0 + 20.0 * min
            } else {
                120.0 + 5.0 * (min - 1.0)
            }
        };
        assert_close(feed(120, temp).value(), 5.0);
    }

    #[test]
    fn resets_when_the_clock_goes_back() {
        let mut ror = RoR::new(WINDOW);
        ror.add(Duration::from_secs(10), 100.0);
        ror.add(Duration::from_secs(20), 110.0);
        ror.add(Duration::from_secs(5), 50.0); // roast clock restarted
        assert_eq!(ror.value(), None);
    }

    #[test]
    fn smooths_probe_noise() {
        let mut ror = RoR::new(WINDOW);
        for i in 1..=30_u32 {
            let noise = if i % 2 == 0 { -0.25 } else { 0.25 };
            ror.add(
                Duration::from_secs(i.into()),
                150.0 + 10.0 * f64::from(i) / 60.0 + noise,
            );
        }
        let got = ror.value().expect("want a value");
        assert!(
            (got - 10.0).abs() < 1.0,
            "got {got:.2} with ±0.25 °C noise, want within 1 of 10"
        );
    }

    #[test]
    fn reset_forgets_everything() {
        let mut ror = feed(10, ramp(100.0, 10.0));
        ror.reset();
        assert_eq!(ror.value(), None);
    }
}
