//! The serials callers send: each station keeps a log through its contest,
//! and its serial is one more than the QSOs in it so far.
//!
//! A station's log grows as a Poisson process at a rate of its own: by the
//! contest's end it holds `final` QSOs on average, and `final` is
//! log-normal. For WPX its median is 190 and its spread σ 1.45, fitted to
//! F6FVY's histogram of last serials in MorseRunner's `CqWpx.pas`; for the
//! NA Sprint its median is 80 and σ 0.9, Dust's estimate. Those counts are
//! of logs, and a station you hear tends to keep a big one, so `final` is
//! drawn with a weight of `final^0.5`: Dust's estimate, between counting
//! logs and counting QSOs. `final` is truncated to between one QSO and the
//! most four digits hold, drawn again where it falls outside, so no log
//! piles up at either end.
//!
//! A run is the whole contest drawn in to the run's length `L`: every
//! station starts it with the contest, so the QSOs a station has logged by
//! run time `t` are `N(t) ~ Poisson(final · t / L)`, and its serial is
//! `1 + N(t)`. The first serials of a run are short, and they grow in
//! proportion to how far through the run it is, to each station's whole
//! log by its end, whatever the run's length. A station heard again
//! later adds the QSOs it logged meanwhile, `Poisson(final · Δt / L)`,
//! the same process.

use super::MS_PER_MINUTE;
use crate::rng::Rng;

/// The shape of a contest's logs: how many QSOs its stations have logged by
/// its end. A run stands for the whole contest, however long it lasts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SerialFit {
    /// The median log, in QSOs.
    pub median: f64,
    /// The spread of the logarithm of a log's size.
    pub sigma: f64,
}

/// CQ WPX, 48 hours: logs fitted to F6FVY's histogram.
pub const WPX_SERIALS: SerialFit = SerialFit {
    median: 190.0,
    sigma: 1.45,
};

/// The NA Sprint, 4 hours: logs of Dust's estimate, smaller and less spread
/// than WPX's, as the contest is short and its field North American.
pub const SPRINT_SERIALS: SerialFit = SerialFit {
    median: 80.0,
    sigma: 0.9,
};

/// How strongly a big log is more likely to be heard: logs are drawn with
/// a weight of their size to this power.
const HEARD_WEIGHT: f64 = 0.5;

/// The biggest log drawn, and the highest serial sent: four digits.
const LOG_SIZE_MAX: f64 = 9_999.0;

/// One station's log, the run's length standing for the contest's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SerialModel {
    /// The QSOs it logs by the contest's end, on average.
    log_size: f64,
    /// Where its count falls among the counts a station of its rate could
    /// have logged at any moment, from nothing up to one: the quantile its
    /// Poisson count is read at. Held for the station, so its serial never
    /// goes back as the run goes on.
    rank: f64,
    /// The run's length, the whole contest drawn in to it.
    length_ms: f64,
}

impl SerialModel {
    /// A station drawn for a run of at most `run_minutes`: the run's limit,
    /// so the longest for a run left open, not its setting of 0. The run
    /// stands for all of the contest, however long the contest is.
    pub fn draw(fit: SerialFit, run_minutes: u32, rng: &mut impl Rng) -> Self {
        // A log-normal weighted by a power of its value is log-normal again,
        // its log-mean moved up by that power times σ². Truncated by drawing
        // again: no log is moved to an end.
        let log_mean = fit.median.ln() + HEARD_WEIGHT * fit.sigma * fit.sigma;
        let log_size = loop {
            let size = (log_mean + fit.sigma * rng.normal()).exp();
            if (1.0..=LOG_SIZE_MAX).contains(&size) {
                break size;
            }
        };
        let rank = rng.f64();
        let length_ms = (f64::from(run_minutes) * MS_PER_MINUTE as f64).max(1.0);
        Self {
            log_size,
            rank,
            length_ms,
        }
    }

    /// The QSOs it has logged on average by `run_ms` into the run: as far
    /// through its log as the run is through its length, and all of it
    /// past the run it was drawn for.
    fn logged_by(&self, run_ms: u64) -> f64 {
        self.log_size * (run_ms as f64 / self.length_ms).min(1.0)
    }

    /// The serial it sends at `run_ms` into the run: one more than its
    /// Poisson count of QSOs so far, read at its own quantile. Within four
    /// digits.
    pub fn serial_at(&self, run_ms: u64) -> u32 {
        let count = poisson_quantile(self.logged_by(run_ms), self.rank);
        four_digits(1 + count)
    }

    /// The serial it sends when it calls again, a dupe, `elapsed_ms` after
    /// it sent `before`: the next in its log, its QSO with you being in it,
    /// and as many more as it logged meanwhile, a Poisson count of its rate,
    /// `final × Δt / L`, `L` the run's length. Within four digits.
    pub fn after(&self, before: u32, elapsed_ms: u64, rng: &mut impl Rng) -> u32 {
        let worked = self.log_size * elapsed_ms as f64 / self.length_ms;
        let more = poisson_quantile(worked, rng.f64());
        four_digits(u64::from(before) + 1 + more)
    }
}

/// `serial`, within the four digits a serial is sent in.
fn four_digits(serial: u64) -> u32 {
    u32::try_from(serial.min(LOG_SIZE_MAX as u64)).unwrap_or(LOG_SIZE_MAX as u32)
}

/// The count at quantile `rank` of a Poisson count of mean `mean`: the
/// least `k` whose cumulative probability reaches `rank`. Exact for any
/// mean: the probabilities are summed out from the most likely count in
/// both directions, in logarithms where they would underflow, so a mean
/// of thousands is read as surely as one of three, in a few hundred steps
/// at most.
pub(crate) fn poisson_quantile(mean: f64, rank: f64) -> u64 {
    if mean.is_nan() || mean <= 0.0 || mean.is_infinite() {
        return 0;
    }
    let rank = rank.clamp(0.0, 1.0);
    // The most likely count, and its probability, from ln k! by Stirling's
    // series past where it is exact enough to sum.
    let mode = mean.floor();
    let at_mode = (mode * mean.ln() - mean - ln_factorial(mode)).exp();
    // The probability below the mode, summed down from it until what is
    // left is nothing a double holds.
    let mut below = 0.0;
    let mut p = at_mode;
    let mut k = mode;
    while k > 0.0 {
        p *= k / mean;
        k -= 1.0;
        below += p;
        if p < below * 1e-17 {
            break;
        }
    }
    // Walk to where the cumulative probability reaches the rank.
    let mut cumulative = below + at_mode;
    if cumulative >= rank {
        // At the mode or below it: walk down while what is left still
        // reaches the rank.
        let mut k = mode;
        let mut p = at_mode;
        while k > 0.0 && cumulative - p >= rank {
            cumulative -= p;
            p *= k / mean;
            k -= 1.0;
        }
        return k as u64;
    }
    let mut k = mode;
    let mut p = at_mode;
    while cumulative < rank && p > 0.0 {
        k += 1.0;
        p *= mean / k;
        cumulative += p;
    }
    k as u64
}

/// ln n!: summed where n is small, and Stirling's series past that, good to
/// far more places than a probability needs.
fn ln_factorial(n: f64) -> f64 {
    if n < 20.0 {
        return (2..=n as u64).map(|k| (k as f64).ln()).sum();
    }
    let inverse = 1.0 / n;
    let inverse_squared = inverse * inverse;
    n * n.ln() - n
        + 0.5 * (std::f64::consts::TAU * n).ln()
        + inverse * (1.0 / 12.0 - inverse_squared * (1.0 / 360.0 - inverse_squared / 1_260.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;

    fn models(count: usize) -> Vec<SerialModel> {
        let mut rng = SplitMix64::new(190);
        (0..count)
            .map(|_| SerialModel::draw(WPX_SERIALS, 10, &mut rng))
            .collect()
    }

    /// The Poisson count read at a quantile is a Poisson count: drawn at
    /// evenly spread ranks, its mean and its variance are its mean, for a
    /// mean under one and for one of thousands alike; it never falls as
    /// the rank or the mean rises, and a mean of nothing is no count.
    #[test]
    fn a_poisson_count_read_at_its_quantile_is_poisson() {
        for mean in [0.3, 4.0, 40.0, 400.0, 4_000.0, 9_999.0] {
            let draws: Vec<f64> = (0..20_000)
                .map(|at| poisson_quantile(mean, (f64::from(at) + 0.5) / 20_000.0) as f64)
                .collect();
            let average = draws.iter().sum::<f64>() / draws.len() as f64;
            let variance = draws
                .iter()
                .map(|count| (count - average).powi(2))
                .sum::<f64>()
                / draws.len() as f64;
            assert!(
                (average / mean - 1.0).abs() < 0.01,
                "{mean}: mean {average}"
            );
            assert!(
                (variance / mean - 1.0).abs() < 0.03,
                "{mean}: variance {variance}"
            );
            assert!(draws.windows(2).all(|pair| pair[0] <= pair[1]), "{mean}");
        }
        for rank in [0.01, 0.5, 0.99] {
            let counts: Vec<u64> = [1.0, 10.0, 100.0, 1_000.0]
                .into_iter()
                .map(|mean| poisson_quantile(mean, rank))
                .collect();
            assert!(
                counts.windows(2).all(|pair| pair[0] <= pair[1]),
                "{counts:?}"
            );
        }
        assert_eq!(poisson_quantile(0.0, 0.9), 0);
        assert_eq!(poisson_quantile(f64::NAN, 0.9), 0);
        // P(0) for a mean of 3 is e^-3, about 0.0498.
        assert_eq!(poisson_quantile(3.0, 0.049), 0);
        assert_eq!(poisson_quantile(3.0, 0.051), 1);
    }

    /// The logs heard sit where the fit puts them: their median is the
    /// fitted median moved up by the weighting, their spread is σ, and the
    /// truncation piles none at either end.
    #[test]
    fn the_logs_heard_have_the_median_and_spread_fitted() {
        let mut logs: Vec<f64> = models(20_000)
            .iter()
            .map(|model| model.log_size.ln())
            .collect();
        logs.sort_by(f64::total_cmp);
        let quantile = |q: f64| logs[(q * logs.len() as f64) as usize];
        let sigma = WPX_SERIALS.sigma;
        let heard_median = WPX_SERIALS.median * (HEARD_WEIGHT * sigma * sigma).exp();
        let median = quantile(0.5).exp();
        assert!(
            (median / heard_median - 1.0).abs() < 0.08,
            "median {median:.0}, fitted {heard_median:.0}"
        );
        // Between the quartiles of a normal lie 1.349 σ.
        let spread = (quantile(0.75) - quantile(0.25)) / 1.349;
        assert!((spread - sigma).abs() < 0.08, "spread {spread:.2}");
        assert!(
            logs.iter()
                .all(|log| (0.0..=LOG_SIZE_MAX.ln()).contains(log))
        );
        // No pile at the top: as few within 1% of the most as a smooth
        // tail has there.
        let top = logs
            .iter()
            .filter(|log| log.exp() > LOG_SIZE_MAX * 0.99)
            .count();
        assert!(top < 20, "{top} of 20000 at the top");
    }

    /// A station's serial starts at one with the contest, never goes back
    /// as the run goes on, and stays within four digits; across stations,
    /// it is on average one more than the QSOs their logs have reached.
    #[test]
    fn a_serial_climbs_through_the_run_and_stays_in_four_digits() {
        let stations = models(4_000);
        for model in &stations {
            assert_eq!(model.serial_at(0), 1, "{model:?}");
            let serials: Vec<u32> = (0..=10)
                .map(|minute| model.serial_at(minute * MS_PER_MINUTE))
                .collect();
            assert!(
                serials.windows(2).all(|pair| pair[0] <= pair[1]),
                "{serials:?}"
            );
            assert!(serials.iter().all(|serial| *serial <= LOG_SIZE_MAX as u32));
        }
        for minute in [2, 5, 10] {
            let expected = stations
                .iter()
                .map(|model| 1.0 + model.log_size * f64::from(minute as u32) / 10.0)
                .sum::<f64>();
            let sent = stations
                .iter()
                .map(|model| f64::from(model.serial_at(minute * MS_PER_MINUTE)))
                .sum::<f64>();
            assert!(
                (sent / expected - 1.0).abs() < 0.02,
                "{minute}: {sent} for {expected}"
            );
        }
    }

    /// The first serials of a run are short and they grow with it, in
    /// proportion to how far through the run it is, the same however long
    /// the run: four digits only as the logs reach them, none in its first
    /// twentieth and more and more after.
    #[test]
    fn the_first_serials_are_short_and_they_grow_with_the_run() {
        for minutes in [10, 60] {
            let mut rng = SplitMix64::new(u64::from(minutes));
            let callers: Vec<SerialModel> = (0..5_000)
                .map(|_| SerialModel::draw(WPX_SERIALS, minutes, &mut rng))
                .collect();
            let at = |share: f64| -> Vec<u32> {
                let run_ms = (share * f64::from(minutes) * MS_PER_MINUTE as f64) as u64;
                let mut serials: Vec<u32> = callers
                    .iter()
                    .map(|model| model.serial_at(run_ms))
                    .collect();
                serials.sort_unstable();
                serials
            };
            let median = |serials: &[u32]| serials[serials.len() / 2];
            let four_digits =
                |serials: &[u32]| serials.iter().filter(|serial| **serial >= 1_000).count();
            let (early, middle, late) = (at(0.05), at(0.5), at(1.0));
            assert!(median(&early) < median(&middle) && median(&middle) < median(&late));
            assert!(median(&early) < 40, "{minutes} min: {}", median(&early));
            // Four digits as often as the logs say: about the stations whose
            // log has reached 999 QSOs on average by then, give or take the
            // Poisson count's own spread, so none in the run's first
            // twentieth, when the biggest log is at 500, and more and more
            // after.
            let mut before = 0;
            for share in [0.05, 0.1, 0.2, 0.5, 1.0] {
                let sent = four_digits(&at(share));
                let reached = callers
                    .iter()
                    .filter(|model| model.log_size * share >= 999.0)
                    .count();
                let slack = 3.0 * (reached as f64).sqrt() + 0.01 * callers.len() as f64;
                assert!(
                    (sent as f64 - reached as f64).abs() <= slack,
                    "{minutes} min, {share}: {sent} sent, {reached} reached"
                );
                assert!(sent >= before, "{minutes} min, {share}");
                before = sent;
            }
            assert_eq!(four_digits(&at(0.05)), 0, "{minutes} min");
            // Halfway through, about half way through each log.
            let ratio = f64::from(median(&middle)) / f64::from(median(&late));
            assert!((0.45..0.56).contains(&ratio), "{minutes} min: {ratio:.2}");
        }
    }

    /// Read past the run it was drawn for, as an open run's 0 passed for its
    /// limit would have it, a station's serial is where its log ends, within
    /// four digits.
    #[test]
    fn a_serial_stops_at_the_log_past_the_run_drawn_for() {
        let mut rng = SplitMix64::new(60);
        for _ in 0..5_000 {
            let model = SerialModel::draw(WPX_SERIALS, 0, &mut rng);
            let serial = model.serial_at(60 * MS_PER_MINUTE);
            assert_eq!(serial, model.serial_at(MS_PER_MINUTE), "{model:?}");
            assert!(serial <= LOG_SIZE_MAX as u32, "{serial}");
        }
    }

    /// A dupe sends the serial after its last, and more the longer since:
    /// about as many as its log's rate gives over the time between, a
    /// Poisson count of it.
    #[test]
    fn a_dupe_sends_the_next_serial_and_what_it_worked_meanwhile() {
        let mut rng = SplitMix64::new(11);
        // A log of 4,800 over a run of 48 hours: a hundred QSOs an hour, so
        // a minute later about one and two thirds.
        let model = SerialModel {
            log_size: 4_800.0,
            rank: 0.5,
            length_ms: 48.0 * 60.0 * MS_PER_MINUTE as f64,
        };
        let draws: Vec<u32> = (0..20_000)
            .map(|_| model.after(214, MS_PER_MINUTE, &mut rng))
            .collect();
        assert!(draws.iter().all(|serial| *serial >= 215));
        let mean = draws
            .iter()
            .map(|serial| f64::from(serial - 215))
            .sum::<f64>()
            / 20_000.0;
        assert!((mean - 5.0 / 3.0).abs() < 0.05, "mean {mean:.3}");
        assert_eq!(
            model.after(214, 0, &mut rng),
            215,
            "straight after: the next"
        );
        assert_eq!(
            model.after(9_999, 0, &mut rng),
            9_999,
            "four digits at most"
        );
        // A Poisson count's variance is its mean.
        let variance = draws
            .iter()
            .map(|serial| (f64::from(serial - 215) - mean).powi(2))
            .sum::<f64>()
            / 20_000.0;
        assert!((variance - mean).abs() < 0.1, "variance {variance:.3}");
        // Over a whole run, its whole log, which the old sampler could not
        // draw past a mean of some 700.
        let whole = SerialModel {
            log_size: 4_800.0,
            rank: 0.5,
            length_ms: 10.0 * MS_PER_MINUTE as f64,
        };
        let later = whole.after(1, 10 * MS_PER_MINUTE, &mut rng);
        assert!((4_500..5_100).contains(&later), "{later}");
    }

    /// A Sprint caller's serial climbs through the run as WPX's does, and
    /// the logs heard sit where the Sprint's fit puts them.
    #[test]
    fn a_sprint_serial_climbs_through_the_run() {
        let mut rng = SplitMix64::new(80);
        let mut ends = Vec::new();
        for _ in 0..5_000 {
            let model = SerialModel::draw(SPRINT_SERIALS, 60, &mut rng);
            let start = model.serial_at(0);
            let end = model.serial_at(60 * MS_PER_MINUTE);
            assert!(start == 1 && end >= start, "{model:?}");
            ends.push(model.log_size);
        }
        ends.sort_by(f64::total_cmp);
        let median = ends[ends.len() / 2];
        let sigma = SPRINT_SERIALS.sigma;
        let heard = SPRINT_SERIALS.median * (HEARD_WEIGHT * sigma * sigma).exp();
        assert!((median / heard - 1.0).abs() < 0.08, "median {median:.0}");
    }

    #[test]
    fn the_same_generator_draws_the_same_caller() {
        let draw = || SerialModel::draw(WPX_SERIALS, 30, &mut SplitMix64::new(7));
        assert_eq!(draw(), draw());
        let model = draw();
        assert_eq!(model.serial_at(60_000), draw().serial_at(60_000));
    }
}
