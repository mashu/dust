//! Charts of a long history. Up to [`PER_SESSION_UP_TO`] sessions, a chart
//! of the history draws each session as a point of its own. Past that, its
//! older sessions are drawn a week to a point, or a month where weeks would
//! still be too many, so that the whole of the learning curve stays on a
//! chart of about [`POINTS_UP_TO`] points, its newest [`RECENT_SESSIONS`]
//! still drawn one by one.

use std::ops::Range;

use crate::stats::AccuracyPoint;
use crate::streak::{civil_from_days, date_to_day_index};

/// Up to this many sessions, a chart draws every one of them.
pub const PER_SESSION_UP_TO: usize = 1_000;

/// About how many points a chart of a longer history draws, at most.
pub const POINTS_UP_TO: usize = 300;

/// How many of a longer history's newest sessions are still drawn one by
/// one.
pub const RECENT_SESSIONS: usize = 100;

const DAY_MS: u64 = 86_400_000;

/// The day a session was practised on, as days since 1970-01-01: its date,
/// as the calendar counts it, or where its date does not read, the day its
/// stamp falls on.
pub fn chart_day(date: &str, timestamp: u64) -> i64 {
    date_to_day_index(date).unwrap_or_else(|| i64::try_from(timestamp / DAY_MS).unwrap_or(i64::MAX))
}

/// What a point of the older history stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Period {
    /// A week, from Monday.
    Week,
    /// This many months, from the first of a month.
    Months(u32),
}

impl Period {
    /// Which period `day` falls in, counted from 1970.
    fn of(self, day: i64) -> i64 {
        match self {
            // 1970-01-01 was a Thursday, three days into its week.
            Self::Week => (day + 3).div_euclid(7),
            Self::Months(months) => {
                let (year, month, _) = civil_from_days(day);
                (i64::from(year) * 12 + i64::from(month) - 1).div_euclid(i64::from(months))
            }
        }
    }

    /// The next longer period: a month after a week, then twice as many
    /// months each time, for a history too long even for a month a point.
    fn longer(self) -> Self {
        match self {
            Self::Week => Self::Months(1),
            Self::Months(months) => Self::Months(months.saturating_mul(2)),
        }
    }
}

/// The sessions each point of a chart stands for, as ranges of where they
/// sit, given the day each was practised on ([`chart_day`]), in the order
/// the chart draws them, oldest first. Up to [`PER_SESSION_UP_TO`] sessions,
/// a point is a session. Past that, the newest [`RECENT_SESSIONS`] are still
/// a point each, and the older ones are grouped by the week they were
/// practised in, or the month where that makes too many points for
/// [`POINTS_UP_TO`] in all.
pub fn chart_buckets(days: &[i64]) -> Vec<Range<usize>> {
    let count = days.len();
    if count <= PER_SESSION_UP_TO {
        return (0..count).map(|at| at..at + 1).collect();
    }
    let recent_from = count - RECENT_SESSIONS;
    let room = POINTS_UP_TO - RECENT_SESSIONS;
    let mut period = Period::Week;
    let older = loop {
        let older = runs(&days[..recent_from], period);
        if older.len() <= room || period == Period::Months(u32::MAX) {
            break older;
        }
        period = period.longer();
    };
    older
        .into_iter()
        .chain((recent_from..count).map(|at| at..at + 1))
        .collect()
}

/// `days` cut where one period ends and the next begins, as ranges of where
/// they sit. Sessions of one period apart in the history, should any be,
/// make a point each.
fn runs(days: &[i64], period: Period) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut last = None;
    for (at, day) in days.iter().enumerate() {
        let now = period.of(*day);
        match ranges.last_mut() {
            Some(range) if last == Some(now) => range.end = at + 1,
            _ => ranges.push(at..at + 1),
        }
        last = Some(now);
    }
    ranges
}

/// The accuracy chart as it is drawn ([`chart_buckets`]): a session a point
/// up to [`PER_SESSION_UP_TO`] sessions; past that, each older week or
/// month a point at the average accuracy of its sessions, each weighing as
/// much as it does drawn on its own, dated as the last of them.
pub fn accuracy_buckets(points: &[AccuracyPoint]) -> Vec<AccuracyPoint> {
    if points.len() <= PER_SESSION_UP_TO {
        return points.to_vec();
    }
    let days: Vec<i64> = points
        .iter()
        .map(|point| chart_day(&point.date, point.timestamp))
        .collect();
    chart_buckets(&days)
        .into_iter()
        .filter_map(|range| {
            let bucket = &points[range];
            let last = bucket.last()?;
            // A session whose accuracy does not read says nothing of the
            // week's.
            let readable: Vec<f64> = bucket
                .iter()
                .map(|point| point.accuracy_pct)
                .filter(|pct| pct.is_finite())
                .collect();
            let accuracy_pct = if readable.is_empty() {
                last.accuracy_pct
            } else {
                readable.iter().sum::<f64>() / readable.len() as f64
            };
            Some(AccuracyPoint {
                accuracy_pct,
                ..last.clone()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streak::days_from_civil;

    /// `count` days, `per_day` sessions a day, from 2020-01-06, a Monday.
    fn practised(count: usize, per_day: usize) -> Vec<i64> {
        let first = days_from_civil(2020, 1, 6);
        (0..count)
            .map(|at| first + i64::try_from(at / per_day).unwrap())
            .collect()
    }

    /// Whether `ranges` cover `0..count` in order, each once.
    fn cover(ranges: &[Range<usize>], count: usize) -> bool {
        let mut next = 0;
        for range in ranges {
            if range.start != next || range.end <= range.start {
                return false;
            }
            next = range.end;
        }
        next == count
    }

    /// Up to the threshold every session is a point of its own: a chart of
    /// a history that short draws as it always did.
    #[test]
    fn a_short_history_is_a_point_a_session() {
        assert!(chart_buckets(&[]).is_empty());
        let days = practised(PER_SESSION_UP_TO, 3);
        let buckets = chart_buckets(&days);
        assert_eq!(buckets.len(), PER_SESSION_UP_TO);
        assert!(
            buckets
                .iter()
                .enumerate()
                .all(|(at, range)| *range == (at..at + 1))
        );
    }

    /// Past it, the older sessions go a week to a point, from Monday, and
    /// the newest are still one each.
    #[test]
    fn a_long_history_groups_its_older_sessions_by_the_week() {
        // Two sessions a day for 550 days: 1,100 sessions.
        let days = practised(1_100, 2);
        let buckets = chart_buckets(&days);
        assert!(cover(&buckets, days.len()), "{buckets:?}");
        assert!(buckets.len() <= POINTS_UP_TO, "{} points", buckets.len());
        let (older, recent) = buckets.split_at(buckets.len() - RECENT_SESSIONS);
        assert!(recent.iter().all(|range| range.len() == 1));
        // A whole week is fourteen sessions, Monday to Sunday.
        assert_eq!(older[0], 0..14);
        assert_eq!(older[1], 14..28);
        for range in older {
            let monday = (days[range.start] + 3).div_euclid(7);
            assert!(
                days[range.clone()]
                    .iter()
                    .all(|day| (day + 3).div_euclid(7) == monday)
            );
        }
    }

    /// Where weeks would still be too many, months: a session a day for
    /// twelve years has some 620 weeks, and 144 months.
    #[test]
    fn a_longer_history_groups_them_by_the_month() {
        let days = practised(12 * 365 + 100, 1);
        let buckets = chart_buckets(&days);
        assert!(cover(&buckets, days.len()));
        assert!(buckets.len() <= POINTS_UP_TO, "{} points", buckets.len());
        let older = &buckets[..buckets.len() - RECENT_SESSIONS];
        assert!((144..=146).contains(&older.len()), "{} months", older.len());
        for range in older {
            let (year, month, _) = civil_from_days(days[range.start]);
            let (end_year, end_month, _) = civil_from_days(days[range.end - 1]);
            assert_eq!((year, month), (end_year, end_month));
        }
        // January 2020 began on a Wednesday: its sessions from the 6th on.
        assert_eq!(older[0], 0..26);
    }

    /// However long the history, the chart keeps to its points: past a
    /// month a point, several months a point.
    #[test]
    fn no_history_draws_more_points_than_the_chart_holds() {
        let days = practised(40 * 365, 1);
        let buckets = chart_buckets(&days);
        assert!(cover(&buckets, days.len()));
        assert!(buckets.len() <= POINTS_UP_TO, "{} points", buckets.len());
        // Every session on one day, past the threshold: one point for all
        // the older ones.
        let one_day = vec![days_from_civil(2026, 10, 30); 1_500];
        let buckets = chart_buckets(&one_day);
        assert_eq!(buckets.len(), RECENT_SESSIONS + 1);
        assert_eq!(buckets[0], 0..1_500 - RECENT_SESSIONS);
    }

    /// A day reads from the date, the calendar's, and from the stamp where
    /// the date does not read.
    #[test]
    fn a_session_is_on_the_day_its_date_says() {
        assert_eq!(chart_day("2026-10-30", 0), days_from_civil(2026, 10, 30));
        assert_eq!(chart_day("", 86_400_000 * 3 + 5), 3);
    }

    fn point(date: &str, timestamp: u64, accuracy_pct: f64) -> AccuracyPoint {
        AccuracyPoint {
            date: date.to_string(),
            accuracy_pct,
            timestamp,
        }
    }

    /// The chart of a short history is its sessions' points, as they were;
    /// a longer one's older weeks are each the average of their sessions.
    #[test]
    fn an_older_week_is_drawn_at_its_sessions_average() {
        let short: Vec<AccuracyPoint> = (0..PER_SESSION_UP_TO as u64)
            .map(|at| point("2026-01-01", at, at as f64 % 100.0))
            .collect();
        assert_eq!(accuracy_buckets(&short), short);

        // Two sessions a day, at 80% and 90%, one at an accuracy that does
        // not read, for 600 days.
        let first = days_from_civil(2020, 1, 6);
        let long: Vec<AccuracyPoint> = (0..1_200_u64)
            .map(|at| {
                let day = first + i64::try_from(at / 2).unwrap();
                let (year, month, date) = civil_from_days(day);
                let pct = match at {
                    3 => f64::NAN,
                    _ if at % 2 == 0 => 80.0,
                    _ => 90.0,
                };
                point(&format!("{year:04}-{month:02}-{date:02}"), at, pct)
            })
            .collect();
        let drawn = accuracy_buckets(&long);
        assert!(drawn.len() <= POINTS_UP_TO);
        // The first week: seven sessions at 80%, six at 90% that read.
        assert!((drawn[0].accuracy_pct - (7.0 * 80.0 + 6.0 * 90.0) / 13.0).abs() < 1e-9);
        assert_eq!(drawn[0].timestamp, 13);
        assert_eq!(drawn[0].date, "2020-01-12");
        assert!((drawn[1].accuracy_pct - 85.0).abs() < 1e-9);
        // The newest are drawn as they were.
        assert_eq!(
            drawn[drawn.len() - RECENT_SESSIONS..],
            long[long.len() - RECENT_SESSIONS..]
        );
    }
}
