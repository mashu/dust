//! Practice calendar from session history: one tile per month, one square per
//! day.

use std::collections::BTreeMap;

use crate::session::SessionResult;
use crate::streak::{
    date_to_day_index, day_index_to_date, days_from_civil, days_in_month, parse_ymd,
    weekday_monday0,
};

/// Whole months on the calendar, the one today is in among them.
pub const HEATMAP_MONTHS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeatmapColorMode {
    Volume,
    Accuracy,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapDay {
    pub date: String,
    pub chars: u32,
    pub sessions: u32,
    pub group_count: u32,
    /// Accuracy times the characters sent at it, so the day's accuracy is
    /// that of its copy, character by character: a session of two groups
    /// moves it as much as two groups do.
    pub accuracy_sum: f64,
    /// The characters the accuracy is summed over: those of sessions whose
    /// accuracy could be read. A session with a broken accuracy still
    /// practised that day, but it says nothing about how well.
    pub accuracy_chars: u32,
    /// Character and spacing speed, each times the characters sent at it, so
    /// the day's speed is what most of its copy was sent at.
    pub char_wpm_sum: f64,
    pub effective_wpm_sum: f64,
    /// The characters the speeds are summed over: those of sessions whose
    /// speeds could be read.
    pub speed_chars: u32,
    /// The day's best session score, if any could be read.
    pub best_score: Option<f64>,
}

impl HeatmapDay {
    fn new(date: &str) -> Self {
        Self {
            date: date.to_string(),
            chars: 0,
            sessions: 0,
            group_count: 0,
            accuracy_sum: 0.0,
            accuracy_chars: 0,
            char_wpm_sum: 0.0,
            effective_wpm_sum: 0.0,
            speed_chars: 0,
            best_score: None,
        }
    }

    /// The day's accuracy over every character it sent.
    pub fn avg_accuracy(&self) -> Option<f64> {
        (self.accuracy_chars > 0).then(|| self.accuracy_sum / f64::from(self.accuracy_chars))
    }

    /// The character speed most of the day's copy was sent at.
    pub fn char_wpm(&self) -> Option<f64> {
        (self.speed_chars > 0).then(|| self.char_wpm_sum / f64::from(self.speed_chars))
    }

    /// The pace its spacing set, slower than [`Self::char_wpm`] under
    /// Farnsworth.
    pub fn effective_wpm(&self) -> Option<f64> {
        (self.speed_chars > 0).then(|| self.effective_wpm_sum / f64::from(self.speed_chars))
    }
}

/// A speed a session can be read as having been sent at.
fn readable_wpm(wpm: f64) -> bool {
    wpm.is_finite() && wpm > 0.0
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapCell {
    pub date: String,
    pub in_future: bool,
    pub chars: u32,
    pub sessions: u32,
    pub group_count: u32,
    pub avg_accuracy: Option<f64>,
    pub char_wpm: Option<f64>,
    pub effective_wpm: Option<f64>,
    pub best_score: Option<f64>,
    /// How strongly to shade the day, from 0 to 1, by what the grid is
    /// showing: the share of the busiest day's characters, or the day's
    /// accuracy. None where there is nothing to shade — a day with no
    /// practice, one still to come, or one whose accuracy could not be read.
    pub intensity: Option<f64>,
}

/// One month's tile: its days, and where the first of them falls in a week.
#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapMonth {
    pub year: i32,
    /// 1 for January.
    pub month: u32,
    /// The weekday of the 1st, Monday as 0: how many days of the month's
    /// first week belong to the month before.
    pub lead: u32,
    /// Every day of the month, the 1st first.
    pub cells: Vec<HeatmapCell>,
}

/// The calendar, and the most of each thing any one day on it had: what a
/// day's figures are measured against.
#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapGrid {
    pub max_chars: u32,
    pub max_sessions: u32,
    pub max_groups: u32,
    pub max_char_wpm: f64,
    pub max_score: f64,
    /// Oldest first, ending with the month today is in.
    pub months: Vec<HeatmapMonth>,
}

impl HeatmapGrid {
    /// Every day on the calendar, in order.
    pub fn cells(&self) -> impl Iterator<Item = &HeatmapCell> {
        self.months.iter().flat_map(|month| &month.cells)
    }

    pub fn cell(&self, date: &str) -> Option<&HeatmapCell> {
        self.cells().find(|cell| cell.date == date)
    }
}

pub fn aggregate_heatmap(sessions: &[SessionResult]) -> BTreeMap<String, HeatmapDay> {
    let mut days = BTreeMap::new();
    for session in sessions {
        if date_to_day_index(&session.date).is_none() {
            continue;
        }
        let entry = days
            .entry(session.date.clone())
            .or_insert_with(|| HeatmapDay::new(&session.date));
        entry.chars = entry.chars.saturating_add(session.total_chars);
        entry.sessions = entry.sessions.saturating_add(1);
        entry.group_count = entry
            .group_count
            .saturating_add(u32::try_from(session.groups.len()).unwrap_or(u32::MAX));
        if session.accuracy.is_finite() && (0.0..=1.0).contains(&session.accuracy) {
            entry.accuracy_sum += session.accuracy * f64::from(session.total_chars);
            entry.accuracy_chars = entry.accuracy_chars.saturating_add(session.total_chars);
        }
        if readable_wpm(session.char_wpm) && readable_wpm(session.effective_wpm) {
            let chars = f64::from(session.total_chars);
            entry.char_wpm_sum += session.char_wpm * chars;
            entry.effective_wpm_sum += session.effective_wpm * chars;
            entry.speed_chars = entry.speed_chars.saturating_add(session.total_chars);
        }
        if session.score.is_finite() && session.score >= 0.0 {
            entry.best_score = Some(
                entry
                    .best_score
                    .map_or(session.score, |best| best.max(session.score)),
            );
        }
    }
    days
}

/// The calendar for the `months` whole months up to and including today's.
pub fn build_heatmap(
    sessions: &[SessionResult],
    today: &str,
    months: u32,
    mode: HeatmapColorMode,
) -> Option<HeatmapGrid> {
    let today_idx = date_to_day_index(today)?;
    let (mut year, mut month, _) = parse_ymd(today)?;
    let mut spans = Vec::new();
    for _ in 0..months.max(1) {
        spans.push((year, month));
        (year, month) = if month == 1 {
            (year - 1, 12)
        } else {
            (year, month - 1)
        };
    }
    spans.reverse();

    let by_date = aggregate_heatmap(sessions);
    let days = |(year, month): (i32, u32)| {
        let first = days_from_civil(year, month, 1);
        first..first + i64::from(days_in_month(year, month))
    };
    let on_calendar: Vec<&HeatmapDay> = spans
        .iter()
        .flat_map(|&span| days(span))
        .filter_map(|idx| by_date.get(&day_index_to_date(idx)))
        .collect();
    let max_chars = on_calendar.iter().map(|day| day.chars).fold(1, u32::max);
    let max_sessions = on_calendar.iter().map(|day| day.sessions).fold(1, u32::max);
    let max_groups = on_calendar
        .iter()
        .map(|day| day.group_count)
        .fold(1, u32::max);
    let max_char_wpm = on_calendar
        .iter()
        .filter_map(|day| day.char_wpm())
        .fold(0.0, f64::max);
    let max_score = on_calendar
        .iter()
        .filter_map(|day| day.best_score)
        .fold(0.0, f64::max);

    let cell = |idx: i64| {
        let date = day_index_to_date(idx);
        let in_future = idx > today_idx;
        let day = by_date.get(&date);
        let chars = day.map(|d| d.chars).unwrap_or(0);
        let sessions_n = day.map(|d| d.sessions).unwrap_or(0);
        let group_count = day.map(|d| d.group_count).unwrap_or(0);
        let avg_accuracy = day.and_then(HeatmapDay::avg_accuracy);
        let char_wpm = day.and_then(HeatmapDay::char_wpm);
        let effective_wpm = day.and_then(HeatmapDay::effective_wpm);
        let best_score = day.and_then(|d| d.best_score);
        let intensity = if in_future || sessions_n == 0 {
            None
        } else {
            match mode {
                HeatmapColorMode::Volume => {
                    (chars > 0).then(|| (f64::from(chars) / f64::from(max_chars.max(1))).min(1.0))
                }
                HeatmapColorMode::Accuracy => avg_accuracy.map(|a| a.clamp(0.0, 1.0)),
            }
        };
        HeatmapCell {
            date,
            in_future,
            chars,
            sessions: sessions_n,
            group_count,
            avg_accuracy,
            char_wpm,
            effective_wpm,
            best_score,
            intensity,
        }
    };

    let months = spans
        .into_iter()
        .map(|(year, month)| {
            let days = days((year, month));
            HeatmapMonth {
                year,
                month,
                lead: weekday_monday0(days.start),
                cells: days.map(cell).collect(),
            }
        })
        .collect();

    Some(HeatmapGrid {
        max_chars,
        max_sessions,
        max_groups,
        max_char_wpm,
        max_score,
        months,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{GroupResult, SessionResult};

    fn session(date: &str, chars: u32, accuracy: f64) -> SessionResult {
        SessionResult {
            date: date.into(),
            groups: vec![GroupResult {
                sent: "A".repeat(chars as usize),
                received: "A".repeat(chars as usize),
                correct: true,
            }],
            accuracy,
            total_chars: chars,
            ..SessionResult::empty_for_test()
        }
    }

    #[test]
    fn aggregates_same_day() {
        let sessions = [
            session("2026-07-17", 10, 0.8),
            session("2026-07-17", 6, 1.0),
        ];
        let days = aggregate_heatmap(&sessions);
        let day = days.get("2026-07-17").expect("day");
        assert_eq!(day.chars, 16);
        assert_eq!(day.sessions, 2);
        // Eight of ten and six of six: fourteen of sixteen characters.
        assert!((day.avg_accuracy().unwrap_or(0.0) - 0.875).abs() < 1e-9);
    }

    /// A day's accuracy is that of its copy, character by character: a
    /// session of one group missed is one group of the day's copy, not
    /// half of it.
    #[test]
    fn a_day_weighs_each_session_by_what_it_sent() {
        let sessions = [
            session("2026-07-17", 2, 0.0),
            session("2026-07-17", 100, 0.96),
        ];
        let day = aggregate_heatmap(&sessions)
            .remove("2026-07-17")
            .expect("day");
        let accuracy = day.avg_accuracy().expect("an accuracy");
        assert!((accuracy - 96.0 / 102.0).abs() < 1e-9, "{accuracy}");
        let grid =
            build_heatmap(&sessions, "2026-07-17", 1, HeatmapColorMode::Accuracy).expect("grid");
        let shade = grid.cell("2026-07-17").and_then(|cell| cell.intensity);
        assert!(shade.is_some_and(|shade| shade > 0.9), "{shade:?}");
    }

    /// A day's speed is what most of its copy was sent at, and its score is
    /// its best session's.
    #[test]
    fn a_day_keeps_its_speed_and_its_best_score() {
        let mut slow = session("2026-07-17", 30, 0.9);
        slow.char_wpm = 20.0;
        slow.effective_wpm = 10.0;
        slow.score = 400.0;
        let mut fast = session("2026-07-17", 10, 0.8);
        fast.char_wpm = 40.0;
        fast.effective_wpm = 30.0;
        fast.score = 250.0;
        let days = aggregate_heatmap(&[slow, fast]);
        let day = days.get("2026-07-17").expect("day");
        assert!((day.char_wpm().unwrap_or(0.0) - 25.0).abs() < 1e-9);
        assert!((day.effective_wpm().unwrap_or(0.0) - 15.0).abs() < 1e-9);
        assert_eq!(day.best_score, Some(400.0));
    }

    #[test]
    fn an_unreadable_speed_or_score_is_left_out() {
        let mut broken = session("2026-07-17", 10, 0.9);
        broken.char_wpm = f64::NAN;
        broken.score = f64::INFINITY;
        let days = aggregate_heatmap(&[broken]);
        let day = days.get("2026-07-17").expect("day");
        assert_eq!(day.char_wpm(), None);
        assert_eq!(day.best_score, None);
    }

    /// Each figure of a day is measured against the most any day on the
    /// calendar had, whichever day that was.
    #[test]
    fn the_grid_knows_the_most_of_everything() {
        let mut busy = session("2026-07-16", 30, 0.9);
        busy.score = 120.0;
        busy.char_wpm = 18.0;
        let mut quick = session("2026-07-17", 10, 0.9);
        quick.score = 300.0;
        quick.char_wpm = 32.0;
        let again = session("2026-07-17", 5, 0.9);
        let grid = build_heatmap(
            &[busy, quick, again],
            "2026-07-17",
            1,
            HeatmapColorMode::Volume,
        )
        .expect("grid");
        assert_eq!(grid.max_chars, 30);
        assert_eq!(grid.max_sessions, 2);
        assert_eq!(grid.max_groups, 2);
        assert_eq!(grid.max_score, 300.0);
        assert!(grid.max_char_wpm > 18.0);
        let friday = grid.cell("2026-07-17").expect("friday");
        assert_eq!(friday.best_score, Some(300.0));
        assert!(friday.char_wpm.is_some());
    }

    #[test]
    fn a_session_with_an_unreadable_date_is_skipped() {
        let days = aggregate_heatmap(&[session("not-a-date", 10, 1.0)]);
        assert!(days.is_empty());
    }

    #[test]
    fn an_out_of_range_accuracy_is_not_summed() {
        let days = aggregate_heatmap(&[session("2026-07-17", 4, f64::NAN)]);
        let day = days.get("2026-07-17").expect("day");
        assert_eq!(day.sessions, 1);
        assert_eq!(day.accuracy_sum, 0.0);
        assert_eq!(day.avg_accuracy(), None);
    }

    /// A broken session counts as practice that day, but not as a zero in
    /// the day's accuracy — which would drag a good day's colour towards red.
    #[test]
    fn an_unreadable_accuracy_does_not_drag_the_average_down() {
        let days = aggregate_heatmap(&[
            session("2026-07-17", 4, 0.9),
            session("2026-07-17", 4, f64::NAN),
        ]);
        let day = days.get("2026-07-17").expect("day");
        assert_eq!(day.sessions, 2);
        assert_eq!(day.avg_accuracy(), Some(0.9));
    }

    #[test]
    fn a_day_without_sessions_has_no_average() {
        let day = HeatmapDay::new("2026-07-17");
        assert_eq!(day.avg_accuracy(), None);
        assert_eq!(day.char_wpm(), None);
        assert_eq!(day.best_score, None);
    }

    #[test]
    fn volume_shades_each_day_against_the_busiest() {
        let grid = build_heatmap(
            &[
                session("2026-07-16", 5, 1.0),
                session("2026-07-17", 10, 1.0),
            ],
            "2026-07-17",
            1,
            HeatmapColorMode::Volume,
        )
        .expect("grid");
        let day = |date: &str| grid.cell(date).unwrap().intensity;
        assert_eq!(day("2026-07-17"), Some(1.0));
        assert_eq!(day("2026-07-16"), Some(0.5));
        assert_eq!(day("2026-07-15"), None, "no practice, nothing to shade");
    }

    /// The busiest day is the busiest on the calendar, whichever month it is
    /// in, so one shade means one amount of copy on every tile.
    #[test]
    fn every_month_is_shaded_against_the_same_busiest_day() {
        let grid = build_heatmap(
            &[
                session("2026-06-30", 20, 1.0),
                session("2026-07-17", 10, 1.0),
            ],
            "2026-07-17",
            2,
            HeatmapColorMode::Volume,
        )
        .expect("grid");
        assert_eq!(grid.max_chars, 20);
        assert_eq!(grid.cell("2026-07-17").unwrap().intensity, Some(0.5));
    }

    #[test]
    fn a_day_whose_accuracy_cannot_be_read_is_not_shaded() {
        let grid = build_heatmap(
            &[session("2026-07-17", 4, f64::NAN)],
            "2026-07-17",
            1,
            HeatmapColorMode::Accuracy,
        )
        .expect("grid");
        let friday = grid.cell("2026-07-17").unwrap();
        assert_eq!(friday.sessions, 1);
        assert_eq!(friday.intensity, None);
    }

    #[test]
    fn an_unreadable_today_draws_no_grid() {
        assert!(build_heatmap(&[], "nonsense", 4, HeatmapColorMode::Volume).is_none());
    }

    #[test]
    fn the_grid_paints_practice_and_leaves_the_future_empty() {
        // The rest of July 2026 after Friday the 17th is still ahead.
        let grid = build_heatmap(
            &[session("2026-07-17", 10, 0.5)],
            "2026-07-17",
            1,
            HeatmapColorMode::Accuracy,
        )
        .expect("grid");
        assert_eq!(grid.months.len(), 1);
        assert_eq!(grid.max_chars, 10);
        let friday = grid.cell("2026-07-17").unwrap();
        assert_eq!(friday.sessions, 1);
        assert_eq!(friday.group_count, 1);
        assert_eq!(friday.avg_accuracy, Some(0.5));
        assert_eq!(friday.intensity, Some(0.5));
        assert!(!friday.in_future);
        let last = grid.cells().last().expect("the 31st");
        assert_eq!(last.date, "2026-07-31");
        assert!(last.in_future);
        assert_eq!(last.intensity, None);
        // Zero months still draws one.
        assert_eq!(
            build_heatmap(&[], "2026-07-17", 0, HeatmapColorMode::Volume)
                .expect("grid")
                .cells()
                .count(),
            31
        );
    }

    #[test]
    fn a_month_runs_from_its_first_to_its_last_day() {
        // 2026-07-01 is a Wednesday, two days into its week.
        let grid = build_heatmap(&[], "2026-07-17", 1, HeatmapColorMode::Volume).expect("grid");
        let july = grid.months.first().expect("july");
        assert_eq!((july.year, july.month, july.lead), (2026, 7, 2));
        assert_eq!(july.cells.len(), 31);
        assert_eq!(
            july.cells.first().map(|c| c.date.as_str()),
            Some("2026-07-01")
        );
        assert_eq!(
            july.cells.last().map(|c| c.date.as_str()),
            Some("2026-07-31")
        );
    }

    #[test]
    fn months_run_oldest_first_back_across_new_year() {
        let grid = build_heatmap(&[], "2026-01-15", 3, HeatmapColorMode::Volume).expect("grid");
        let spans: Vec<(i32, u32, usize)> = grid
            .months
            .iter()
            .map(|m| (m.year, m.month, m.cells.len()))
            .collect();
        assert_eq!(spans, [(2025, 11, 30), (2025, 12, 31), (2026, 1, 31)]);
        // The months follow on from each other with no day left out.
        let dates: Vec<&str> = grid.cells().map(|c| c.date.as_str()).collect();
        assert_eq!(dates.first(), Some(&"2025-11-01"));
        assert_eq!(dates.last(), Some(&"2026-01-31"));
        assert_eq!(dates.len(), 30 + 31 + 31);
    }

    #[test]
    fn a_leap_february_has_its_29th() {
        let grid = build_heatmap(&[], "2028-02-10", 1, HeatmapColorMode::Volume).expect("grid");
        assert_eq!(grid.cells().count(), 29);
        assert!(grid.cell("2028-02-29").is_some());
    }
}
