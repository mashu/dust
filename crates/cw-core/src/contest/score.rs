//! A log's contest score and rates: how well you operated, beside the copy
//! points ([`crate::score`]) that measure how well you copied.
//!
//! The score comes twice, as MorseRunner's does. Raw is what the log claims:
//! every logged QSO that is not a dupe, NILs and busted calls included,
//! times the multipliers as typed. Verified is what survives the check:
//! clean QSOs only, a dupe scoring nothing. Verified is never above raw.

use std::collections::BTreeSet;

use super::check::QsoCheck;
use super::rules::{Multiplier, Scoring};
use super::{MS_PER_HOUR, MS_PER_MINUTE};
use crate::callsign::wpx_prefix;

/// One logged QSO, as the score reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoredQso<'a> {
    /// When it was logged, in run time.
    pub at_ms: u64,
    /// The call as you logged it.
    pub call: &'a str,
    pub check: &'a QsoCheck,
}

/// One column of the score: QSO points, multipliers, and their product.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub qso_points: u32,
    pub mults: u32,
    pub score: u32,
}

/// QSOs an hour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rates {
    /// Over the last five minutes of the run, as MorseRunner shows it.
    pub last_5_min: Option<f64>,
    /// Over the whole run.
    pub whole_run: Option<f64>,
    /// Over the best five minutes.
    pub best_5_min: Option<f64>,
}

/// A log's score, raw and verified, and its rates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContestScore {
    pub raw: Tally,
    pub verified: Tally,
    pub rates: Rates,
}

/// The window a rate is counted over.
const RATE_WINDOW_MS: u64 = 5 * MS_PER_MINUTE;

/// The score of `qsos`, in the order they were logged, over a run of
/// `run_ms`, as `scoring` counts it.
pub fn contest_score(scoring: &Scoring, qsos: &[ScoredQso<'_>], run_ms: u64) -> ContestScore {
    // Raw drops a call already logged, as a logger marks a dupe: by the
    // call as typed. Verified keeps clean QSOs that are not dupes, each
    // station once, so it can only count calls raw counts.
    let mut typed = BTreeSet::new();
    let raw: Vec<&ScoredQso<'_>> = qsos.iter().filter(|qso| typed.insert(qso.call)).collect();
    let mut clean = BTreeSet::new();
    let verified: Vec<&ScoredQso<'_>> = qsos
        .iter()
        .filter(|qso| qso.check.is_clean() && !qso.check.dupe && clean.insert(qso.call))
        .collect();
    let times: Vec<u64> = qsos.iter().map(|qso| qso.at_ms).collect();
    ContestScore {
        raw: tally(scoring, &raw),
        verified: tally(scoring, &verified),
        rates: Rates::of(&times, run_ms),
    }
}

/// The points and multipliers of the QSOs that count.
fn tally(scoring: &Scoring, counted: &[&ScoredQso<'_>]) -> Tally {
    let mults: BTreeSet<String> = counted
        .iter()
        .filter_map(|qso| match scoring.multiplier {
            Multiplier::WpxPrefix => wpx_prefix(qso.call),
        })
        .collect();
    let qsos = u32::try_from(counted.len()).unwrap_or(u32::MAX);
    let qso_points = qsos.saturating_mul(scoring.points);
    let mults = u32::try_from(mults.len()).unwrap_or(u32::MAX);
    Tally {
        qso_points,
        mults,
        score: qso_points.saturating_mul(mults),
    }
}

impl Rates {
    /// The rates of QSOs logged at `times`, over a run of `run_ms`.
    pub fn of(times: &[u64], run_ms: u64) -> Self {
        let window = run_ms.min(RATE_WINDOW_MS);
        let best = times
            .iter()
            .map(|&end| count_in(times, end, window))
            .max()
            .unwrap_or(0);
        Self {
            last_5_min: rate_at(times, run_ms),
            whole_run: (run_ms > 0).then(|| per_hour(times.len(), run_ms)),
            best_5_min: (window > 0).then(|| per_hour(best, window)),
        }
    }
}

/// QSOs an hour at `now_ms` of run time, counted over the last five
/// minutes, or over the run so far while it is shorter; none at its start.
pub fn rate_at(times: &[u64], now_ms: u64) -> Option<f64> {
    let window = now_ms.min(RATE_WINDOW_MS);
    (window > 0).then(|| per_hour(count_in(times, now_ms, window), window))
}

/// How many of `times` fall in the `window` that ends at `end`.
fn count_in(times: &[u64], end: u64, window: u64) -> usize {
    times
        .iter()
        .filter(|&&at| at <= end && end - at < window)
        .count()
}

fn per_hour(count: usize, over_ms: u64) -> f64 {
    count as f64 * MS_PER_HOUR as f64 / over_ms as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::{ContestKind, QsoError};

    fn wpx() -> Scoring {
        ContestKind::Wpx
            .rules()
            .and_then(|rules| rules.scoring)
            .expect("WPX scores")
    }

    fn clean() -> QsoCheck {
        QsoCheck::default()
    }

    fn logged<'a>(at_s: u64, call: &'a str, check: &'a QsoCheck) -> ScoredQso<'a> {
        ScoredQso {
            at_ms: at_s * 1_000,
            call,
            check,
        }
    }

    fn wrong() -> QsoCheck {
        QsoCheck {
            errors: vec![QsoError::Field(crate::contest::Field::Nr)],
            dupe: false,
        }
    }

    /// A ten-minute run worked by hand: seven QSOs, one a busted call, one a
    /// wrong number, one NIL and one a dupe.
    #[test]
    fn a_hand_worked_wpx_log_scores_raw_and_verified() {
        let busted = QsoCheck {
            errors: vec![QsoError::Call {
                correct: "K5XYZ".into(),
            }],
            dupe: false,
        };
        let nil = QsoCheck {
            errors: vec![QsoError::Nil],
            dupe: false,
        };
        let dupe = QsoCheck {
            errors: Vec::new(),
            dupe: true,
        };
        let (clean, wrong) = (clean(), wrong());
        let log = [
            // WPX prefix K5: clean.
            logged(30, "K5ABC", &clean),
            // DL1: clean.
            logged(90, "DL1ABC", &clean),
            // K5 again (K5XYW was really K5XYZ): busted, raw only.
            logged(150, "K5XYW", &busted),
            // 9A1: wrong NR, raw only.
            logged(240, "9A1AA", &wrong),
            // W4 (W1XYZ/4): NIL, raw only.
            logged(330, "W1XYZ/4", &nil),
            // PA0 (PA/N8XYZ): clean.
            logged(420, "PA/N8XYZ", &clean),
            // DL1ABC again: a dupe copied right, which scores nothing.
            logged(500, "DL1ABC", &dupe),
        ];
        let score = contest_score(&wpx(), &log, 600_000);
        // Raw: six calls (the dupe dropped), prefixes K5 DL1 9A1 W4 PA0.
        assert_eq!(
            score.raw,
            Tally {
                qso_points: 6,
                mults: 5,
                score: 30
            }
        );
        // Verified: K5ABC, DL1ABC and PA/N8XYZ; prefixes K5 DL1 PA0.
        assert_eq!(
            score.verified,
            Tally {
                qso_points: 3,
                mults: 3,
                score: 9
            }
        );
        assert!(score.verified.score <= score.raw.score);
        // Seven logged in ten minutes is 42 an hour; the last five minutes
        // (from 5:00) hold the last three.
        assert_eq!(score.rates.whole_run, Some(42.0));
        assert_eq!(score.rates.last_5_min, Some(36.0));
        // The best five minutes, to 4:00, hold four: 48 an hour.
        assert_eq!(score.rates.best_5_min, Some(48.0));
    }

    /// A dupe costs nothing but its own points: it is no error, and the
    /// rest of the log scores as it would without it.
    #[test]
    fn a_dupe_scores_nothing_and_is_no_error() {
        let dupe = QsoCheck {
            errors: Vec::new(),
            dupe: true,
        };
        let clean = clean();
        assert!(dupe.is_clean());
        let one = [logged(1, "K5XYZ", &clean)];
        let with_dupe = [one[0], logged(2, "K5XYZ", &dupe)];
        let without = contest_score(&wpx(), &one, 60_000);
        let with = contest_score(&wpx(), &with_dupe, 60_000);
        assert_eq!(with.raw, without.raw);
        assert_eq!(with.verified, without.verified);
    }

    /// Verified never passes raw, whatever the log holds.
    #[test]
    fn verified_is_never_above_raw() {
        use crate::rng::{Rng, SplitMix64};
        let calls = [
            "K5XYZ", "K5ABC", "DL1ABC", "DL2ABC", "9A1AA", "W1XYZ/4", "K5XYW",
        ];
        let checks = [
            clean(),
            wrong(),
            QsoCheck {
                errors: Vec::new(),
                dupe: true,
            },
        ];
        let mut rng = SplitMix64::new(4);
        for _ in 0..500 {
            let log: Vec<ScoredQso<'_>> = (0..rng.usize_in(0, 12))
                .map(|i| ScoredQso {
                    at_ms: i as u64 * 40_000,
                    call: calls[rng.usize_in(0, calls.len() - 1)],
                    check: &checks[rng.usize_in(0, checks.len() - 1)],
                })
                .collect();
            let score = contest_score(&wpx(), &log, 600_000);
            assert!(score.verified.qso_points <= score.raw.qso_points);
            assert!(score.verified.mults <= score.raw.mults);
            assert!(score.verified.score <= score.raw.score);
        }
    }

    #[test]
    fn a_rate_counts_the_last_five_minutes_or_the_run_so_far() {
        assert_eq!(rate_at(&[], 0), None, "no rate at the start");
        assert_eq!(rate_at(&[], 60_000), Some(0.0));
        // One QSO in the first minute is 60 an hour.
        assert_eq!(rate_at(&[30_000], 60_000), Some(60.0));
        // Ten minutes in, only the last five count.
        let times = [60_000, 120_000, 400_000, 500_000, 590_000];
        assert_eq!(rate_at(&times, 600_000), Some(36.0));
        // A QSO exactly five minutes back has left the window.
        assert_eq!(rate_at(&[300_000], 600_000), Some(0.0));
        let empty = Rates::of(&[], 0);
        assert_eq!(empty, Rates::default());
    }
}
