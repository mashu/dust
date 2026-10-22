//! The serials callers send: each caller is somewhere in a contest of its
//! own, with a log of its own size, and its serial is how far through that
//! log it has got.
//!
//! A caller's log, `final` QSOs by the contest's end, is log-normal. For WPX
//! its median is 190 and its spread σ 1.45, fitted to F6FVY's histogram of
//! last serials in MorseRunner's `CqWpx.pas`; for the NA Sprint, four hours
//! long, its median is 80 and σ 0.9, Dust's estimate. Those counts are of
//! logs, and a station you hear tends to keep a big one, so `final` is drawn
//! with a weight of `final^0.5`: Dust's estimate, between counting logs and
//! counting QSOs. The run starts at a time `T0` drawn evenly through the
//! contest's `L` hours, leaving room for the run, and a caller's serial at
//! run time `t` is `1 + floor(final · (T0 + t) / L)`, never past `final`.

use super::{MS_PER_HOUR, MS_PER_MINUTE};
use crate::rng::Rng;

/// The shape of a contest's logs, and how long the contest lasts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SerialFit {
    /// The median log, in QSOs.
    pub median: f64,
    /// The spread of the logarithm of a log's size.
    pub sigma: f64,
    /// The contest's length.
    pub hours: f64,
}

/// CQ WPX: 48 hours, logs fitted to F6FVY's histogram.
pub const WPX_SERIALS: SerialFit = SerialFit {
    median: 190.0,
    sigma: 1.45,
    hours: 48.0,
};

/// The NA Sprint: 4 hours, logs of Dust's estimate, smaller and less spread
/// than WPX's, as the contest is short and its field North American.
pub const SPRINT_SERIALS: SerialFit = SerialFit {
    median: 80.0,
    sigma: 0.9,
    hours: 4.0,
};

/// How strongly a big log is more likely to be heard: logs are drawn with
/// a weight of their size to this power.
const HEARD_WEIGHT: f64 = 0.5;

/// The biggest log drawn, so a serial never runs past four digits. About
/// one WPX caller in fifty would otherwise draw more.
const LOG_SIZE_MAX: f64 = 9_999.0;

/// One caller's place in its contest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SerialModel {
    /// QSOs in its log by the contest's end.
    log_size: f64,
    /// How far into the contest it was when the run started.
    offset_ms: f64,
    length_ms: f64,
}

impl SerialModel {
    /// A caller drawn for a run of at most `run_minutes`: the run's limit,
    /// so the longest for a run left open, not its setting of 0.
    pub fn draw(fit: SerialFit, run_minutes: u32, rng: &mut impl Rng) -> Self {
        // A log-normal weighted by a power of its value is log-normal again,
        // its log-mean moved up by that power times σ².
        let log_mean = fit.median.ln() + HEARD_WEIGHT * fit.sigma * fit.sigma;
        let log_size = (log_mean + fit.sigma * rng.normal())
            .exp()
            .clamp(1.0, LOG_SIZE_MAX);
        let length_ms = fit.hours * MS_PER_HOUR as f64;
        let run_ms = f64::from(run_minutes) * MS_PER_MINUTE as f64;
        let offset_ms = rng.f64() * (length_ms - run_ms).max(0.0);
        Self {
            log_size,
            offset_ms,
            length_ms,
        }
    }

    /// The serial it sends at `run_ms` into the run.
    pub fn serial_at(&self, run_ms: u64) -> u32 {
        let through = (self.offset_ms + run_ms as f64) / self.length_ms;
        let serial = 1 + (self.log_size * through).floor() as u32;
        // Its last QSO is the last serial it sends. A run longer than the one
        // it was drawn for would otherwise take it past its log, and a log of
        // the most drawn into five digits.
        serial.min(self.log_size as u32)
    }

    /// The serial it sends when it calls again, a dupe, `elapsed_ms` after
    /// it sent `before`: the next in its log, its QSO with you being in it,
    /// and as many more as it worked meanwhile, a Poisson count of its rate,
    /// `final × Δt / L`. Within four digits.
    pub fn after(&self, before: u32, elapsed_ms: u64, rng: &mut impl Rng) -> u32 {
        let worked = self.log_size * elapsed_ms as f64 / self.length_ms;
        // A mean of a couple of hundred at most: the biggest WPX log over
        // the longest run.
        let more = rng.poisson(worked, LOG_SIZE_MAX as usize);
        let serial = u64::from(before) + 1 + more as u64;
        u32::try_from(serial.min(LOG_SIZE_MAX as u64)).unwrap_or(u32::MAX)
    }
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

    /// The logs heard sit where the fit puts them: their median is the
    /// fitted median moved up by the weighting, and their spread is σ.
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
    }

    /// A serial starts where the caller is in its contest, never goes back,
    /// and never passes its log.
    #[test]
    fn a_serial_climbs_through_the_run_and_stays_in_the_log() {
        for model in models(2_000) {
            let start = model.serial_at(0);
            let end = model.serial_at(10 * MS_PER_MINUTE);
            assert!(start >= 1);
            assert!(end >= start);
            assert!(f64::from(end) <= model.log_size, "{end} past {model:?}");
            assert!(model.offset_ms <= model.length_ms - (10 * MS_PER_MINUTE) as f64);
        }
    }

    /// Read past the run it was drawn for, as an open run's 0 passed for its
    /// limit would have it, a caller still stops at its last QSO, within
    /// four digits.
    #[test]
    fn a_serial_stops_at_the_log_past_the_run_drawn_for() {
        let mut rng = SplitMix64::new(60);
        for _ in 0..20_000 {
            let model = SerialModel::draw(WPX_SERIALS, 0, &mut rng);
            let serial = model.serial_at(60 * MS_PER_MINUTE);
            assert!(
                f64::from(serial) <= model.log_size,
                "{serial} past {model:?}"
            );
            assert!(serial <= LOG_SIZE_MAX as u32, "{serial}");
        }
    }

    /// A dupe sends the serial after its last, and more the longer since:
    /// about as many as its log's rate gives over the time between, a
    /// Poisson count of it.
    #[test]
    fn a_dupe_sends_the_next_serial_and_what_it_worked_meanwhile() {
        let mut rng = SplitMix64::new(11);
        let model = SerialModel {
            log_size: 4_800.0,
            offset_ms: 0.0,
            length_ms: 48.0 * MS_PER_HOUR as f64,
        };
        // A hundred QSOs an hour: a minute later, about one and two thirds.
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
    }

    /// A Sprint caller's serial stays within its four-hour contest, the
    /// longest run fitting in it, and the logs heard sit where the fit puts
    /// them.
    #[test]
    fn a_sprint_serial_climbs_through_its_four_hours() {
        let mut rng = SplitMix64::new(80);
        let mut ends = Vec::new();
        for _ in 0..5_000 {
            let model = SerialModel::draw(SPRINT_SERIALS, 60, &mut rng);
            assert!(model.offset_ms <= model.length_ms - (60 * MS_PER_MINUTE) as f64);
            let start = model.serial_at(0);
            let end = model.serial_at(60 * MS_PER_MINUTE);
            assert!(start >= 1 && end >= start, "{model:?}");
            assert!(f64::from(end) <= model.log_size, "{end} past {model:?}");
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
