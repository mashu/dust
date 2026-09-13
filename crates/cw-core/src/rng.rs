//! Minimal RNG trait so sampling and audio planning stay deterministic in tests.

pub trait Rng {
    fn f64(&mut self) -> f64;

    fn usize_in(&mut self, min_inclusive: usize, max_inclusive: usize) -> usize {
        if max_inclusive <= min_inclusive {
            return min_inclusive;
        }
        let span = max_inclusive - min_inclusive + 1;
        min_inclusive + (self.f64() * span as f64).floor() as usize
    }

    fn pick_in_range(&mut self, min: f64, max: f64) -> f64 {
        if (min - max).abs() < f64::EPSILON {
            return min;
        }
        let lo = min.min(max);
        let hi = min.max(max);
        lo + self.f64() * (hi - lo)
    }

    fn pick_in_range_inclusive_int(&mut self, min: f64, max: f64) -> f64 {
        if (min - max).abs() < f64::EPSILON {
            return min;
        }
        let lo = min.min(max);
        let hi = min.max(max);
        (lo + self.f64() * (hi - lo + 1.0)).floor()
    }
}

/// Adapter over `fastrand::Rng` used by the web crate. Defined here so tests can use a stub.
pub struct FastrandRng(pub u64);

impl Default for FastrandRng {
    fn default() -> Self {
        Self(0x4d595df4d0f33173)
    }
}

impl Rng for FastrandRng {
    fn f64(&mut self) -> f64 {
        // SplitMix64 → unit interval, no extra crate in cw-core.
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        (z >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

pub fn weighted_random_pick(pool: &[char], weights: &[f64], rng: &mut impl Rng) -> char {
    if pool.is_empty() {
        return '\0';
    }
    if pool.len() == 1 || weights.is_empty() {
        let idx = rng.usize_in(0, pool.len() - 1);
        return pool.get(idx).copied().unwrap_or('\0');
    }
    let mut total = 0.0;
    for i in 0..pool.len() {
        total += weights.get(i).copied().unwrap_or(1.0);
    }
    if total <= 0.0 {
        let idx = rng.usize_in(0, pool.len() - 1);
        return pool.get(idx).copied().unwrap_or('\0');
    }
    let mut r = rng.f64() * total;
    for i in 0..pool.len() {
        r -= weights.get(i).copied().unwrap_or(1.0);
        if r <= 0.0 {
            return pool.get(i).copied().unwrap_or('\0');
        }
    }
    pool.last().copied().unwrap_or('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pool_returns_nul() {
        let mut rng = FastrandRng::default();
        assert_eq!(weighted_random_pick(&[], &[], &mut rng), '\0');
    }

    #[test]
    fn single_element() {
        let mut rng = FastrandRng::default();
        assert_eq!(weighted_random_pick(&['A'], &[1.0], &mut rng), 'A');
    }

    #[test]
    fn heavy_bias() {
        let mut rng = FastrandRng::default();
        let pool = ['A', 'B'];
        let weights = [100.0, 0.01];
        let mut a = 0;
        for _ in 0..1000 {
            if weighted_random_pick(&pool, &weights, &mut rng) == 'A' {
                a += 1;
            }
        }
        assert!(a > 900);
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;

    struct Fixed(f64);

    impl Rng for Fixed {
        fn f64(&mut self) -> f64 {
            self.0
        }
    }

    #[test]
    fn a_collapsed_range_returns_its_only_value() {
        let mut rng = Fixed(0.99);
        assert_eq!(rng.pick_in_range(18.0, 18.0), 18.0);
        assert_eq!(rng.pick_in_range_inclusive_int(600.0, 600.0), 600.0);
        assert_eq!(rng.usize_in(4, 4), 4);
        assert_eq!(rng.usize_in(4, 2), 4);
    }

    #[test]
    fn a_reversed_range_is_read_in_the_right_order() {
        let mut rng = Fixed(0.0);
        assert_eq!(rng.pick_in_range(30.0, 10.0), 10.0);
        assert_eq!(rng.pick_in_range_inclusive_int(600.0, 400.0), 400.0);
    }

    #[test]
    fn integer_picks_cover_both_ends_and_never_overshoot() {
        for (draw, expected) in [(0.0, 400.0), (0.5, 500.0), (0.999_999, 600.0)] {
            let mut rng = Fixed(draw);
            assert_eq!(rng.pick_in_range_inclusive_int(400.0, 600.0), expected);
        }
        for (draw, expected) in [(0.0, 1), (0.999_999, 5)] {
            let mut rng = Fixed(draw);
            assert_eq!(rng.usize_in(1, 5), expected);
        }
    }

    #[test]
    fn weights_that_add_up_to_nothing_fall_back_to_an_even_pick() {
        let mut rng = Fixed(0.75);
        assert_eq!(
            weighted_random_pick(&['A', 'B', 'C', 'D'], &[0.0; 4], &mut rng),
            'D'
        );
        // Fewer weights than characters: the missing ones count as one each,
        // so the total is positive and the walk starts at the first character.
        let mut rng = Fixed(0.0);
        assert_eq!(weighted_random_pick(&['A', 'B'], &[0.0], &mut rng), 'A');
        let mut rng = Fixed(0.99);
        assert_eq!(weighted_random_pick(&['A', 'B'], &[0.0], &mut rng), 'B');
    }

    #[test]
    fn a_draw_at_the_very_top_still_lands_on_a_character() {
        let mut rng = Fixed(1.0);
        assert_eq!(
            weighted_random_pick(&['A', 'B'], &[1.0, 1.0], &mut rng),
            'B'
        );
    }

    #[test]
    fn the_default_generator_is_deterministic_and_in_range() {
        let mut a = FastrandRng::default();
        let mut b = FastrandRng::default();
        for _ in 0..100 {
            let x = a.f64();
            assert_eq!(x, b.f64());
            assert!((0.0..1.0).contains(&x));
        }
        // A different seed gives a different stream.
        let mut other = FastrandRng(1);
        assert_ne!(FastrandRng::default().f64(), other.f64());
    }
}
