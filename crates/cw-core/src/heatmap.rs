//! GitHub-style practice heatmap from session history.

use std::collections::BTreeMap;

use crate::session::SessionResult;
use crate::streak::{date_to_day_index, day_index_to_date, weekday_monday0};

pub const HEATMAP_WEEKS: u32 = 16;
pub const EMPTY_COLOR: &str = "#e5e7eb";
pub const NO_ACCURACY_COLOR: &str = "#cbd5e1";

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
    pub accuracy_sum: f64,
}

impl HeatmapDay {
    pub fn avg_accuracy(&self) -> Option<f64> {
        if self.sessions == 0 {
            None
        } else {
            Some(self.accuracy_sum / f64::from(self.sessions))
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapCell {
    pub date: String,
    pub in_future: bool,
    pub chars: u32,
    pub sessions: u32,
    pub group_count: u32,
    pub avg_accuracy: Option<f64>,
    pub color: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapGrid {
    pub weeks: u32,
    pub max_chars: u32,
    pub cells: Vec<HeatmapCell>,
}

pub fn aggregate_heatmap(sessions: &[SessionResult]) -> BTreeMap<String, HeatmapDay> {
    let mut days = BTreeMap::new();
    for session in sessions {
        if date_to_day_index(&session.date).is_none() {
            continue;
        }
        let entry = days
            .entry(session.date.clone())
            .or_insert_with(|| HeatmapDay {
                date: session.date.clone(),
                chars: 0,
                sessions: 0,
                group_count: 0,
                accuracy_sum: 0.0,
            });
        entry.chars += session.total_chars;
        entry.sessions += 1;
        entry.group_count += session.groups.len() as u32;
        if session.accuracy.is_finite() && (0.0..=1.0).contains(&session.accuracy) {
            entry.accuracy_sum += session.accuracy;
        }
    }
    days
}

pub fn hsl_from_normalized(n: f64) -> String {
    let clamped = n.clamp(0.0, 1.0);
    let hue = 120.0 * clamped;
    format!("hsl({hue:.0}, 75%, 45%)")
}

pub fn color_for_count(count: u32, max_count: u32) -> String {
    if count == 0 {
        return EMPTY_COLOR.to_string();
    }
    let n = f64::from(count) / f64::from(max_count.max(1));
    hsl_from_normalized(n.min(1.0))
}

pub fn color_for_accuracy(sessions: u32, avg_accuracy: Option<f64>) -> String {
    if sessions == 0 {
        return EMPTY_COLOR.to_string();
    }
    match avg_accuracy {
        Some(acc) => hsl_from_normalized(acc),
        None => NO_ACCURACY_COLOR.to_string(),
    }
}

pub fn build_heatmap(
    sessions: &[SessionResult],
    today: &str,
    weeks: u32,
    mode: HeatmapColorMode,
) -> Option<HeatmapGrid> {
    let today_idx = date_to_day_index(today)?;
    let weeks = weeks.max(1);
    let today_wd = i64::from(weekday_monday0(today_idx));
    let this_monday = today_idx - today_wd;
    let start = this_monday - i64::from(weeks.saturating_sub(1)) * 7;
    let by_date = aggregate_heatmap(sessions);
    let mut max_chars = 1u32;
    let mut cells = Vec::with_capacity((weeks * 7) as usize);
    for offset in 0..(i64::from(weeks) * 7) {
        let idx = start + offset;
        let date = day_index_to_date(idx);
        if let Some(day) = by_date.get(&date) {
            max_chars = max_chars.max(day.chars);
        }
        cells.push((idx, date));
    }

    let cells = cells
        .into_iter()
        .map(|(idx, date)| {
            let in_future = idx > today_idx;
            let day = by_date.get(&date);
            let chars = day.map(|d| d.chars).unwrap_or(0);
            let sessions_n = day.map(|d| d.sessions).unwrap_or(0);
            let group_count = day.map(|d| d.group_count).unwrap_or(0);
            let avg_accuracy = day.and_then(HeatmapDay::avg_accuracy);
            let color = if in_future {
                EMPTY_COLOR.to_string()
            } else {
                match mode {
                    HeatmapColorMode::Volume => color_for_count(chars, max_chars),
                    HeatmapColorMode::Accuracy => color_for_accuracy(sessions_n, avg_accuracy),
                }
            };
            HeatmapCell {
                date,
                in_future,
                chars,
                sessions: sessions_n,
                group_count,
                avg_accuracy,
                color,
            }
        })
        .collect();

    Some(HeatmapGrid {
        weeks,
        max_chars,
        cells,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{GroupResult, SessionResult, SessionTiming};
    use crate::settings::CharSetMode;
    use std::collections::BTreeMap;

    fn session(date: &str, chars: u32, accuracy: f64) -> SessionResult {
        SessionResult {
            date: date.into(),
            timestamp: 0,
            started_at: 0,
            finished_at: 1,
            groups: vec![GroupResult {
                sent: "A".repeat(chars as usize),
                received: "A".repeat(chars as usize),
                correct: true,
            }],
            group_timings: vec![SessionTiming {
                time_to_complete_ms: 1.0,
                per_char_ms: 1.0,
                char_wpm: None,
            }],
            accuracy,
            letter_accuracy: BTreeMap::new(),
            alphabet_size: 1,
            avg_response_ms: 0.0,
            total_chars: chars,
            effective_alphabet_size: 1.0,
            score: 0.0,
            level: 1,
            digits_level: 1,
            char_set_mode: CharSetMode::Koch,
            char_wpm: 18.0,
            effective_wpm: 18.0,
            alphabet_fingerprint: String::new(),
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
        assert!((day.avg_accuracy().unwrap_or(0.0) - 0.9).abs() < 1e-9);
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
    }

    #[test]
    fn a_day_without_sessions_has_no_average() {
        let day = HeatmapDay {
            date: "2026-07-17".into(),
            chars: 0,
            sessions: 0,
            group_count: 0,
            accuracy_sum: 0.0,
        };
        assert_eq!(day.avg_accuracy(), None);
    }

    #[test]
    fn colours_run_from_empty_through_red_to_green() {
        assert_eq!(color_for_count(0, 10), EMPTY_COLOR);
        assert_eq!(color_for_count(10, 10), hsl_from_normalized(1.0));
        assert_eq!(color_for_count(5, 0), hsl_from_normalized(1.0));
        assert_eq!(hsl_from_normalized(0.0), "hsl(0, 75%, 45%)");
        assert_eq!(hsl_from_normalized(1.0), "hsl(120, 75%, 45%)");
        // Out-of-range input is clamped rather than producing a broken colour.
        assert_eq!(hsl_from_normalized(-1.0), hsl_from_normalized(0.0));
        assert_eq!(hsl_from_normalized(9.0), hsl_from_normalized(1.0));
    }

    #[test]
    fn accuracy_colour_falls_back_when_a_day_has_no_accuracy() {
        assert_eq!(color_for_accuracy(0, Some(1.0)), EMPTY_COLOR);
        assert_eq!(color_for_accuracy(2, None), NO_ACCURACY_COLOR);
        assert_eq!(color_for_accuracy(2, Some(1.0)), hsl_from_normalized(1.0));
    }

    #[test]
    fn an_unreadable_today_draws_no_grid() {
        assert!(build_heatmap(&[], "nonsense", 4, HeatmapColorMode::Volume).is_none());
    }

    #[test]
    fn the_grid_paints_practice_and_leaves_the_future_empty() {
        // 2026-07-17 is a Friday, so Saturday and Sunday are still ahead.
        let grid = build_heatmap(
            &[session("2026-07-17", 10, 0.5)],
            "2026-07-17",
            1,
            HeatmapColorMode::Accuracy,
        )
        .expect("grid");
        assert_eq!(grid.weeks, 1);
        assert_eq!(grid.max_chars, 10);
        let friday = grid.cells.iter().find(|c| c.date == "2026-07-17").unwrap();
        assert_eq!(friday.sessions, 1);
        assert_eq!(friday.group_count, 1);
        assert_eq!(friday.avg_accuracy, Some(0.5));
        assert_eq!(friday.color, hsl_from_normalized(0.5));
        assert!(!friday.in_future);
        let sunday = grid.cells.last().expect("sunday");
        assert!(sunday.in_future);
        assert_eq!(sunday.color, EMPTY_COLOR);
        // Zero weeks still draws one.
        assert_eq!(
            build_heatmap(&[], "2026-07-17", 0, HeatmapColorMode::Volume)
                .expect("grid")
                .cells
                .len(),
            7
        );
    }

    #[test]
    fn grid_starts_on_monday() {
        // 2026-07-17 is a Friday.
        let grid = build_heatmap(&[], "2026-07-17", 1, HeatmapColorMode::Volume).expect("grid");
        assert_eq!(grid.cells.len(), 7);
        assert_eq!(
            grid.cells.first().map(|c| c.date.as_str()),
            Some("2026-07-13")
        );
        assert_eq!(
            grid.cells.get(4).map(|c| c.date.as_str()),
            Some("2026-07-17")
        );
    }
}
