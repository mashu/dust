//! The score board: the best sessions kept on this device, the personal best
//! with its medal, and the score over time with the record it set.
//!
//! There is one player, so a row is a place, a score and when it was set —
//! no name, no callsign. The board is read off the stored history, so a
//! deleted session leaves it.
//!
//! The tab holds what its cards share — the mode shown and the session
//! marked — and each card draws its own part: `best` the personal best,
//! `timeline` the score over time, `board` the rows.

mod best;
mod board;
#[cfg(test)]
mod board_tests;
mod medal;
mod timeline;

use cw_core::{
    BOARD_MIN_GROUPS, BoardEntry, CharSetMode, FORM_SESSIONS, SessionResult, TimelinePoint,
    board_modes, score_board, score_timeline,
};
use dioxus::prelude::*;

use crate::ui::format::short_date;
use crate::ui::settings_charset::mode_label;
use crate::ui::widgets::{CardHead, IconName, Seg};
use best::BestCard;
use board::BoardList;
pub use medal::Medal;
use timeline::ScoreTimeline;

/// What a session of `mode` is called in a sentence: "callsign sessions".
pub fn mode_noun(mode: CharSetMode) -> &'static str {
    match mode {
        CharSetMode::Koch => "Koch",
        CharSetMode::Digits => "digit",
        CharSetMode::Mixed => "mixed",
        CharSetMode::Callsign => "callsign",
        CharSetMode::Words => "word",
        CharSetMode::Contest => "contest",
    }
}

/// What a session earned a group, as the board shows it: "72.4".
pub fn per_group_label(per_group: f64) -> String {
    format!("{per_group:.1}")
}

/// The speed a session was sent at: "25 WPM", or "25/15 WPM" under
/// Farnsworth.
fn speed(entry: &BoardEntry) -> String {
    if entry.char_wpm - entry.effective_wpm >= 0.5 {
        format!("{:.0}/{:.0} WPM", entry.char_wpm, entry.effective_wpm)
    } else {
        format!("{:.0} WPM", entry.char_wpm)
    }
}

/// Every session of `mode`, best first, with the personal best, the score
/// over time and the board itself. Pointing at a row (or tapping it) marks
/// on the timeline when it was set.
#[component]
pub fn ScoresTab(sessions: Vec<SessionResult>) -> Element {
    let mut mode = use_signal(|| None::<CharSetMode>);
    let hovered = use_signal(|| None::<u64>);
    let pinned = use_signal(|| None::<u64>);
    let modes = board_modes(&sessions);
    // A mode whose sessions have all been deleted shows every mode again.
    let current = mode().filter(|chosen| modes.contains(chosen));
    let board = score_board(&sessions, current);
    let timeline = score_timeline(&sessions, current);
    let today = crate::time::local_date_string();

    let Some(best) = board.first().cloned() else {
        let scored = sessions
            .iter()
            .any(|session| session.score.is_finite() && session.score > 0.0);
        let (title, body) = if scored {
            (
                "No session long enough yet",
                format!(
                    "Sessions of {BOARD_MIN_GROUPS} groups or more go on the board, ranked by what they earn a group. Set a longer session under Settings → Session."
                ),
            )
        } else {
            (
                "No scores yet",
                format!(
                    "Finish a session of {BOARD_MIN_GROUPS} groups or more and it goes on the board. The best one gets the gold."
                ),
            )
        };
        return rsx! {
            div { class: "card",
                CardHead { icon: IconName::Medal, title }
                p { class: "muted", style: "margin: 0;", "{body}" }
            }
        };
    };
    let focus = hovered().or(pinned()).unwrap_or(best.timestamp);
    let focused = board.iter().find(|entry| entry.timestamp == focus).cloned();
    let date_of = |point: Option<&TimelinePoint>| {
        point
            .and_then(|point| sessions.get(point.index))
            .map(|session| short_date(&session.date))
            .unwrap_or_default()
    };
    let first_date = date_of(timeline.first());
    let last_date = date_of(timeline.last());
    let form = timeline.last().map(|point| point.form);
    let form_over = timeline.len().min(FORM_SESSIONS);
    let total = board.len();
    let show_mode = current.is_none();

    rsx! {
        div { class: "stack",
            if modes.len() > 1 {
                div { class: "mode-pills",
                    Seg { label: "All modes", pill: true, active: current.is_none(), onclick: move |_| mode.set(None) }
                    for each in modes.iter().copied() {
                        Seg {
                            label: mode_label(each),
                            pill: true,
                            active: current == Some(each),
                            onclick: move |_| mode.set(Some(each)),
                        }
                    }
                }
            }
            BestCard { best, form, form_over, today, show_mode }
            ScoreTimeline { timeline, focus, focused, total, first_date, last_date }
            BoardList { board, focus, hovered, pinned, show_mode }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::settings_charset::CHAR_SET_MODES;

    #[test]
    fn every_mode_has_a_name() {
        for (mode, label) in CHAR_SET_MODES {
            assert_eq!(mode_label(mode), label);
        }
        // Contest too, though the control does not offer it yet: History
        // names the runs a later version kept.
        for mode in CharSetMode::ALL {
            assert!(!mode_label(mode).is_empty(), "{mode:?}");
            assert!(!mode_noun(mode).is_empty(), "{mode:?}");
        }
        assert_eq!(mode_label(CharSetMode::Contest), "Contest");
    }

    #[test]
    fn a_farnsworth_speed_says_both() {
        let mut entry = BoardEntry {
            place: 1,
            index: 0,
            per_group: 25.0,
            score: 500.0,
            timestamp: 0,
            date: "2026-10-16".to_string(),
            char_wpm: 25.0,
            effective_wpm: 25.0,
            accuracy: 0.95,
            groups: 20,
            mode: CharSetMode::Callsign,
        };
        assert_eq!(speed(&entry), "25 WPM");
        entry.effective_wpm = 15.0;
        assert_eq!(speed(&entry), "25/15 WPM");
        assert_eq!(per_group_label(72.44), "72.4");
    }
}
