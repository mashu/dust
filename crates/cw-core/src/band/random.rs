//! The band's random numbers: the crate's own generator, with the shapes the
//! band draws from it.

use crate::rng::{Rng, SplitMix64, box_muller};

/// The crate's generator as a plain value, small enough to copy into every
/// transmitter on the band, with the distributions the band draws.
#[derive(Clone, Copy, Debug, Default)]
pub struct Draw(u64);

impl Rng for Draw {
    fn f64(&mut self) -> f64 {
        let mut inner = SplitMix64::new(self.0);
        let value = inner.f64();
        self.0 = inner.state;
        value
    }
}

impl Draw {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// A seed for another stream of its own, drawn from this one.
    pub fn next_u64(&mut self) -> u64 {
        (self.f64() * (1u64 << 53) as f64) as u64 ^ self.0.rotate_left(17)
    }

    pub fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.f64()
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    /// An index into something `len` long; zero for nothing at all.
    pub fn index(&mut self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            self.usize_in(0, len - 1)
        }
    }

    /// Two independent standard normal deviates, by Box–Muller. Where they
    /// are wanted a sample at a time, [`Gaussian`] keeps the other one of
    /// each pair.
    pub fn normal_pair(&mut self) -> (f64, f64) {
        let (radius, angle) = box_muller(self);
        let (sin, cos) = angle.sin_cos();
        (radius * cos, radius * sin)
    }

    /// Log-normal around a median: what durations and spacings follow, from
    /// the length of a lightning flash to the speed of a fist.
    pub fn log_normal(&mut self, median: f64, sigma_ln: f64) -> f64 {
        median * (sigma_ln * self.normal()).exp()
    }

    /// Log-normal with a different spread either side of the median, for a
    /// distribution with a long tail on one side only.
    pub fn split_log_normal(&mut self, median: f64, (below, above): (f64, f64)) -> f64 {
        let z = self.normal();
        median * (z * if z < 0.0 { below } else { above }).exp()
    }

    /// Exponential waiting time, in samples, to the next arrival of a Poisson
    /// process at `rate` a second; never, for a process that is off.
    pub fn wait(&mut self, rate: f64, sample_rate: f64) -> u64 {
        if rate.is_nan() || rate <= 0.0 {
            return u64::MAX;
        }
        let wait = -(1.0 - self.f64()).ln() * sample_rate / rate;
        wait.round().clamp(0.0, 1e15) as u64
    }
}

/// Standard normal deviates one at a time, for the noise floors: Box–Muller,
/// keeping the second of each pair for the next sample.
///
/// The floor has to be Gaussian, not merely random: band noise is the sum of
/// countless independent sources, and only a Gaussian through a narrow filter
/// has the Rayleigh envelope — the restless, grainy "shhh" with a pitch —
/// that a receiver's hiss has. Uniform noise through the same filter comes
/// out close, but this is exact and costs nothing.
#[derive(Clone, Debug)]
pub struct Gaussian {
    draw: Draw,
    spare: Option<f64>,
}

impl Gaussian {
    pub fn new(seed: u64) -> Self {
        Self {
            draw: Draw::new(seed),
            spare: None,
        }
    }

    pub fn sample(&mut self) -> f64 {
        if let Some(value) = self.spare.take() {
            return value;
        }
        let (value, spare) = self.draw.normal_pair();
        self.spare = Some(spare);
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_standard_normal(draws: &[f64]) {
        let n = draws.len() as f64;
        let mean = draws.iter().sum::<f64>() / n;
        let var = draws.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        let within_one = draws.iter().filter(|x| x.abs() < 1.0).count() as f64 / n;
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((var - 1.0).abs() < 0.015, "variance {var}");
        assert!(
            (within_one - 0.6827).abs() < 0.005,
            "{within_one} within one sigma"
        );
        assert!(draws.iter().all(|x| x.is_finite()));
    }

    /// Both halves of each pair are normal: the one [`Draw::normal`] keeps,
    /// and the spare the noise floors use as well.
    #[test]
    fn the_normals_are_standard_normals() {
        let mut g = Gaussian::new(3);
        is_standard_normal(&(0..200_000).map(|_| g.sample()).collect::<Vec<f64>>());
        let mut draw = Draw::new(4);
        is_standard_normal(&(0..200_000).map(|_| draw.normal()).collect::<Vec<f64>>());
    }

    /// A zero seed is as good as any: the generator is SplitMix64, which
    /// needs nothing odd about it.
    #[test]
    fn a_zero_seed_draws_like_any_other() {
        let mut zero = Draw::new(0);
        let mean = (0..100_000).map(|_| zero.f64()).sum::<f64>() / 100_000.0;
        assert!((mean - 0.5).abs() < 0.005, "mean {mean}");
    }

    #[test]
    fn waits_and_counts_are_never_nonsense() {
        let mut draw = Draw::new(1);
        assert_eq!(draw.wait(0.0, 48_000.0), u64::MAX);
        assert_eq!(draw.wait(f64::NAN, 48_000.0), u64::MAX);
        assert_eq!(draw.poisson(f64::NAN, 10), 0);
        assert!((0..1_000).all(|_| draw.poisson(50.0, 10) <= 10));
        assert_eq!(draw.index(0), 0);
    }
}
