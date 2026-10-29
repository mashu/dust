//! Minimal RNG trait so sampling and audio planning stay deterministic in tests.

pub trait Rng {
    /// A uniform draw from `[0, 1)`.
    ///
    /// The helpers below stay in range even for an implementation that hands
    /// back exactly 1.0, as test stubs do: a contract is only as good as what
    /// happens when it is broken, and here that would be an index past the
    /// end of a table.
    fn f64(&mut self) -> f64;

    fn usize_in(&mut self, min_inclusive: usize, max_inclusive: usize) -> usize {
        if max_inclusive <= min_inclusive {
            return min_inclusive;
        }
        let range = max_inclusive - min_inclusive;
        // In floating point, so a range of the whole of usize cannot overflow
        // the +1.
        let offset = (self.f64() * (range as f64 + 1.0)).floor() as usize;
        min_inclusive + offset.min(range)
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
        (lo + self.f64() * (hi - lo + 1.0)).floor().min(hi.floor())
    }

    /// A standard normal deviate, by Box–Muller. Straight-line: no draw is
    /// retried, so no generator — however badly behaved — can keep it
    /// spinning.
    fn normal(&mut self) -> f64
    where
        Self: Sized,
    {
        let (radius, angle) = box_muller(self);
        radius * angle.cos()
    }

    /// A normal deviate about `mean` that strays at most `limit` either
    /// side: half the limit is one standard deviation, and the tails are
    /// cut at the whole of it, so most draws sit near the mean.
    fn normal_within(&mut self, mean: f64, limit: f64) -> f64
    where
        Self: Sized,
    {
        (mean + self.normal() * limit / 2.0).clamp(mean - limit, mean + limit)
    }

    /// A Poisson count with mean `mean`, by multiplying uniforms (Knuth),
    /// and never more than `most`, which bounds the draws it takes too:
    /// one more than the count.
    fn poisson(&mut self, mean: f64, most: usize) -> usize {
        if mean.is_nan() || mean <= 0.0 {
            return 0;
        }
        let limit = (-mean).exp();
        let mut product = self.f64();
        let mut count = 0;
        while product > limit && count < most {
            product *= self.f64();
            count += 1;
        }
        count
    }
}

/// Box–Muller's radius and angle for one pair of standard normal deviates:
/// `radius·cos(angle)` and `radius·sin(angle)` are independent.
pub(crate) fn box_muller(rng: &mut impl Rng) -> (f64, f64) {
    // 1 − u maps [0, 1) onto (0, 1], where the logarithm is finite; the clamp
    // keeps it there for a generator that hands back 1.0.
    let u = (1.0 - rng.f64()).clamp(f64::EPSILON, 1.0);
    ((-2.0 * u.ln()).sqrt(), std::f64::consts::TAU * rng.f64())
}

/// A small, fast, seedable generator (SplitMix64). Deterministic for a given
/// seed, which is what lets every test replay the same session and the same
/// band.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitMix64 {
    pub(crate) state: u64,
}

impl Default for SplitMix64 {
    fn default() -> Self {
        Self::new(0x4d59_5df4_d0f3_3173)
    }
}

impl Rng for SplitMix64 {
    fn f64(&mut self) -> f64 {
        self.state = self.state.wrapping_add(SPLITMIX_GAMMA);
        (mix(self.state) >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

impl SplitMix64 {
    /// The generator that `seed` starts.
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// A generator of its own for `stream`, derived from this one's state
    /// without drawing from it: the same state and stream always give the
    /// same generator, and any other pair an unrelated one.
    ///
    /// Adding to the seed is not the same thing. SplitMix64 steps its state
    /// by a fixed increment, so a seed that is another plus a multiple of
    /// that increment is the other's stream a few draws on — two streams
    /// seeded that way overlap, draw for draw. Hashing the stream into the
    /// state puts each one somewhere unrelated on the cycle.
    #[must_use]
    pub fn fork(&self, stream: u64) -> Self {
        Self::new(mix(self.state ^ mix(stream)))
    }
}

/// SplitMix64's state increment.
const SPLITMIX_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64's output function: a bijection on `u64` that scatters nearby
/// inputs across the whole range.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One of `pool`, each as likely as its weight; a missing weight counts as
/// one. One weighed at nothing is never picked while any other weighs
/// something, whatever the generator gives. `None` only for an empty pool.
pub fn weighted_random_pick(pool: &[char], weights: &[f64], rng: &mut impl Rng) -> Option<char> {
    let last = pool.len().checked_sub(1)?;
    let weight = |i: usize| weights.get(i).copied().unwrap_or(1.0);
    let total: f64 = (0..pool.len()).map(weight).sum();
    if pool.len() == 1 || weights.is_empty() || total <= 0.0 {
        return pool.get(rng.usize_in(0, last)).copied();
    }
    let mut r = rng.f64() * total;
    let mut picked = None;
    for (i, ch) in pool.iter().enumerate() {
        if weight(i) <= 0.0 {
            continue;
        }
        picked = Some(*ch);
        r -= weight(i);
        if r <= 0.0 {
            break;
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_pool_has_nothing_to_pick() {
        let mut rng = SplitMix64::default();
        assert_eq!(weighted_random_pick(&[], &[], &mut rng), None);
    }

    #[test]
    fn single_element() {
        let mut rng = SplitMix64::default();
        assert_eq!(weighted_random_pick(&['A'], &[1.0], &mut rng), Some('A'));
    }

    #[test]
    fn heavy_bias() {
        let mut rng = SplitMix64::default();
        let pool = ['A', 'B'];
        let weights = [100.0, 0.01];
        let mut a = 0;
        for _ in 0..1000 {
            if weighted_random_pick(&pool, &weights, &mut rng) == Some('A') {
                a += 1;
            }
        }
        assert!(a > 900);
    }

    /// A generator at its very bottom does not pick a character weighed at
    /// nothing, nor one at its top: Mixed at 100% letters weighs every digit
    /// at nothing.
    #[test]
    fn nothing_weighed_at_nothing_is_picked() {
        let pool = ['0', 'K', '1', 'M', '2'];
        let weights = [0.0, 1.0, 0.0, 2.0, 0.0];
        for at in [0.0, 0.5, 0.999_999_9] {
            let picked = weighted_random_pick(&pool, &weights, &mut Fixed(at));
            assert!(matches!(picked, Some('K' | 'M')), "{at}: {picked:?}");
        }
    }

    struct Fixed(f64);

    impl Rng for Fixed {
        fn f64(&mut self) -> f64 {
            self.0
        }
    }

    /// A draw within a limit stays within it, about a third of draws
    /// beyond half of it, as one standard deviation is.
    #[test]
    fn a_normal_draw_within_a_limit_stays_within_it() {
        let mut rng = SplitMix64::new(3);
        let draws: Vec<f64> = (0..10_000).map(|_| rng.normal_within(4.0, 2.0)).collect();
        assert!(draws.iter().all(|draw| (2.0..=6.0).contains(draw)));
        let wide = draws
            .iter()
            .filter(|draw| (*draw - 4.0).abs() > 1.0)
            .count();
        assert!(
            (2_900..3_500).contains(&wide),
            "{wide} beyond half the limit"
        );
        assert_eq!(Fixed(0.5).normal_within(4.0, 0.0), 4.0);
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
            Some('D')
        );
        // Fewer weights than characters: the missing ones count as one each,
        // so the total is positive, and the one weighed at nothing is passed
        // over even by a draw at the very bottom.
        let mut rng = Fixed(0.0);
        assert_eq!(
            weighted_random_pick(&['A', 'B'], &[0.0], &mut rng),
            Some('B')
        );
        let mut rng = Fixed(0.99);
        assert_eq!(
            weighted_random_pick(&['A', 'B'], &[0.0], &mut rng),
            Some('B')
        );
    }

    /// A generator that hands back exactly 1.0 breaks the `[0, 1)` contract,
    /// and test stubs do. The helpers stay in range anyway, rather than
    /// indexing one past the end of a table.
    #[test]
    fn draws_at_the_very_top_stay_in_range() {
        let mut rng = Fixed(1.0);
        assert_eq!(rng.usize_in(0, 25), 25);
        assert_eq!(rng.usize_in(3, 3), 3);
        assert_eq!(rng.pick_in_range_inclusive_int(400.0, 600.0), 600.0);
        // The whole of usize, which used to overflow working out the span.
        assert_eq!(rng.usize_in(0, usize::MAX), usize::MAX);
        let mut rng = Fixed(0.0);
        assert_eq!(rng.usize_in(0, usize::MAX), 0);
        assert_eq!(rng.usize_in(7, 9), 7);
    }

    #[test]
    fn a_draw_at_the_very_top_still_lands_on_a_character() {
        let mut rng = Fixed(1.0);
        assert_eq!(
            weighted_random_pick(&['A', 'B'], &[1.0, 1.0], &mut rng),
            Some('B')
        );
    }

    #[test]
    fn the_default_generator_is_deterministic_and_in_range() {
        let mut a = SplitMix64::default();
        let mut b = SplitMix64::default();
        for _ in 0..100 {
            let x = a.f64();
            assert_eq!(x, b.f64());
            assert!((0.0..1.0).contains(&x));
        }
        // A different seed gives a different stream.
        let mut other = SplitMix64::new(1);
        assert_ne!(SplitMix64::default().f64(), other.f64());
    }

    /// Streams forked from neighbouring seeds, and neighbouring streams of
    /// one seed, share no draws. Seeding each with a multiple of SplitMix64's
    /// own increment added in made one session's stations the last-but-one
    /// session's, two draws on.
    #[test]
    fn forked_streams_do_not_overlap() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..64u64 {
            for stream in 0..16u64 {
                let mut rng = SplitMix64::new(seed).fork(stream);
                for _ in 0..8 {
                    assert!(
                        seen.insert(rng.f64().to_bits()),
                        "seed {seed}, stream {stream} repeats a draw of another stream"
                    );
                }
            }
        }
        // And the same seed and stream are the same generator, every time.
        assert_eq!(SplitMix64::new(5).fork(3), SplitMix64::new(5).fork(3));
        assert_ne!(SplitMix64::new(5).fork(3), SplitMix64::new(5).fork(4));
    }
}
