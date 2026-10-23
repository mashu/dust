//! The score board: the best sessions kept on this device, the personal best
//! with its medal, and the score over time with the record it set.
//!
//! There is one player, so a row is a place, a score and when it was set —
//! no name, no callsign. The board is read off the stored history, so a
//! deleted session leaves it.
//!
//! The tab holds what its cards share — the mode shown and the session
//! marked — and each card draws its own part: `best` the personal best,
//! `timeline` the score over time, `board` the rows, and under the Contest
//! pill `runs` every run of the contest chosen.

mod best;
mod board;
#[cfg(test)]
mod board_tests;
mod medal;
mod runs;
mod timeline;

use cw_core::{
    BOARD_MIN_GROUPS, BOARD_RUN_MIN_S, BoardEntry, BoardOf, CharSetMode, ContestKind,
    FORM_SESSIONS, SessionResult, TimelinePoint, assisted_runs, board_kinds, board_modes,
    run_kinds, score_board, score_timeline,
};
use dioxus::prelude::*;

use crate::ui::contest_results::{opened_results, use_opened_run};
use crate::ui::format::short_date;
use crate::ui::settings_charset::mode_label;
use crate::ui::widgets::{CardHead, IconName, Seg, control_id};
use best::BestCard;
use board::BoardList;
pub use medal::Medal;
use runs::RunsCard;
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

/// Which contest runs go on the board, in a phrase: "runs of 5 minutes or
/// more…". A run's groups are its calls and fields, one each.
fn run_rule() -> String {
    format!(
        "runs of {} minutes or more that hold {BOARD_MIN_GROUPS} groups or more (each call and field is one) and a QSO logged clean",
        BOARD_RUN_MIN_S / 60
    )
}

/// Every session of `mode`, best first, with the personal best, the score
/// over time and the board itself. Pointing at a row (or tapping it) marks
/// on the timeline when it was set. Contest runs are ranked within their
/// contest, chosen by a chip of its own under the Contest pill, which also
/// lists every run of it on its Runs card, placed or not; a row there opens
/// the run's results in the tab's place, as History opens them.
#[component]
pub fn ScoresTab(sessions: Vec<SessionResult>) -> Element {
    let mode = use_signal(|| None::<CharSetMode>);
    let kind = use_signal(|| None::<ContestKind>);
    let hovered = use_signal(|| None::<u64>);
    let pinned = use_signal(|| None::<u64>);
    // The run whose results are open, from the Runs card.
    let mut opened = use_opened_run();
    // The Contest pill shows once the history holds a run, placed or not,
    // and a chip for each contest it holds runs of: the Runs card is there.
    let kinds = run_kinds(&sessions);
    let modes = board_modes(&sessions);
    // A mode whose sessions have all been deleted shows every mode again,
    // and a contest with none left the first that has runs on the board, or
    // else the first that has runs.
    let current = mode().filter(|chosen| modes.contains(chosen));
    let contest = kind()
        .filter(|chosen| kinds.contains(chosen))
        .or_else(|| board_kinds(&sessions).first().copied())
        .or(kinds.first().copied());
    let of = match (current, contest) {
        (None, _) => BoardOf::All,
        (Some(CharSetMode::Contest), Some(contest)) => BoardOf::Contest(contest),
        (Some(mode), _) => BoardOf::Mode(mode),
    };
    if let Some(results) = opened_results(&sessions, opened, "Scores") {
        return results;
    }
    let board = score_board(&sessions, of);
    let assisted = assisted_runs(&sessions, of);
    let timeline = score_timeline(&sessions, of);
    let today = crate::time::local_date_string();
    let runs_of = match of {
        BoardOf::Contest(kind) => Some(kind),
        _ => None,
    };
    let pills = rsx! {
        BoardPills { modes, kinds, current, of, mode, kind }
    };

    let Some(best) = board.first().cloned() else {
        let (title, body) = empty_board(&sessions, of, !assisted.is_empty());
        return rsx! {
            div { class: "stack",
                {pills}
                div { class: "card",
                    CardHead { icon: IconName::Medal, title }
                    p { class: "muted", style: "margin: 0;", "{body}" }
                }
                if !assisted.is_empty() {
                    BoardList { board: Vec::new(), assisted, focus: 0, hovered, pinned, show_mode: current.is_none() }
                }
                if let Some(kind) = runs_of {
                    RunsCard { sessions: sessions.clone(), kind, on_open: move |pick| opened.set(Some(pick)) }
                }
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
            {pills}
            BestCard { best, form, form_over, today, show_mode }
            ScoreTimeline { timeline, focus, focused, total, first_date, last_date }
            BoardList { board, assisted, focus, hovered, pinned, show_mode }
            if let Some(kind) = runs_of {
                RunsCard { sessions: sessions.clone(), kind, on_open: move |pick| opened.set(Some(pick)) }
            }
        }
    }
}

/// The mode pills, when there is more than one mode to choose, or a
/// contest run whose Runs card the Contest pill opens; and under the
/// Contest pill, a chip for each contest the history holds runs of.
#[component]
fn BoardPills(
    modes: Vec<CharSetMode>,
    kinds: Vec<ContestKind>,
    current: Option<CharSetMode>,
    of: BoardOf,
    mode: Signal<Option<CharSetMode>>,
    kind: Signal<Option<ContestKind>>,
) -> Element {
    let (mut mode, mut kind) = (mode, kind);
    rsx! {
        if modes.len() > 1 || !kinds.is_empty() {
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
        if let BoardOf::Contest(shown) = of {
            div { class: "mode-pills kind-pills",
                for each in kinds.iter().copied() {
                    Seg {
                        label: each.label(),
                        id: control_id("kind", each.label()),
                        pill: true,
                        active: shown == each,
                        onclick: move |_| kind.set(Some(each)),
                    }
                }
            }
        }
    }
}

/// What a board with nothing placed on it says, `of` the sessions it
/// holds: none scored yet; runs, which go on by their own rule; or sessions
/// too short. A board that lists `assisted` runs says why they are not
/// placed.
fn empty_board(sessions: &[SessionResult], of: BoardOf, assisted: bool) -> (&'static str, String) {
    let scored: Vec<&SessionResult> = sessions
        .iter()
        .filter(|session| of.holds(session))
        .filter(|session| session.score.is_finite() && session.score > 0.0)
        .collect();
    // Runs go on by their own rule, and a run has a length to set, not a
    // count of groups.
    let runs_only = scored
        .iter()
        .all(|session| session.char_set_mode == CharSetMode::Contest);
    match (scored.is_empty(), runs_only, assisted) {
        (true, _, _) => (
            "No scores yet",
            format!(
                "Finish a session of {BOARD_MIN_GROUPS} groups or more and it goes on the board. The best one gets the gold."
            ),
        ),
        (false, true, true) => (
            "No placed run yet",
            format!(
                "Runs with Show copy on are assisted: listed below, never placed. The board takes {}, ranked by what they earn a group.",
                run_rule()
            ),
        ),
        (false, true, false) => (
            "No run long enough yet",
            format!(
                "The board takes {}, ranked by what they earn a group. Set Minutes per run to {} or more under Settings → Session shape.",
                run_rule(),
                BOARD_RUN_MIN_S / 60
            ),
        ),
        (false, false, _) => (
            "No session long enough yet",
            format!(
                "Sessions of {BOARD_MIN_GROUPS} groups or more go on the board, ranked by what they earn a group. Set a longer session under Settings → Session."
            ),
        ),
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
            contest: None,
            qsos: None,
        };
        assert_eq!(speed(&entry), "25 WPM");
        entry.effective_wpm = 15.0;
        assert_eq!(speed(&entry), "25/15 WPM");
        assert_eq!(per_group_label(72.44), "72.4");
    }
}
