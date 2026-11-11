//! A tiny deterministic random source for probe noise: `SplitMix64` with a
//! Box–Muller transform. Good enough for jitter on a thermocouple, nothing more.

use std::f64::consts::TAU;

#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1].
    #[allow(clippy::cast_precision_loss)] // 53 bits fit an f64 mantissa exactly
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) + 1) as f64 / (1_u64 << 53) as f64
    }

    /// Standard normal (mean 0, deviation 1).
    pub(crate) fn normal(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic_and_roughly_normal() {
        let draw = |seed| {
            let mut r = Rng::new(seed);
            (0..20_000).map(|_| r.normal()).collect::<Vec<_>>()
        };
        assert_eq!(draw(7), draw(7));
        let xs = draw(1);
        let n = 20_000.0;
        let mean = xs.iter().sum::<f64>() / n;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        assert!(
            mean.abs() < 0.05 && (var - 1.0).abs() < 0.05,
            "mean {mean}, var {var}"
        );
    }
}
