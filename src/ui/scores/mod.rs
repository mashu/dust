//! The score board: the best sessions kept on this device, the personal best
//! with its medal, and the score over time with the record it set.
//!
//! There is one player, so a row is a place, a score and when it was set —
//! no name, no callsign. The board is read off the stored history, so a
//! deleted session leaves it.
//!
//! The tab holds what its cards share — the mode shown, the session marked,
//! and under the Contest pill the contest and what its board ranks by — and
//! each card draws its own part: `best` the personal best, `timeline` the
//! score over time, `board` the rows, and under the Contest pill `runs`
//! every run of the contest chosen.

mod best;
mod board;
#[cfg(test)]
mod board_tests;
mod medal;
mod runs;
mod timeline;

use cw_core::{
    BOARD_MIN_GROUPS, BOARD_RUN_MIN_S, BoardEntry, BoardOf, CharSetMode, ContestKind,
    FORM_SESSIONS, Ranking, SessionResult, TimelinePoint, assisted_runs, board_kinds, board_modes,
    latest_run_length, run_kinds, run_lengths, score_board, score_timeline,
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

/// What a board ranks by, as the board shows it: "72.4" points a group, a
/// verified score of "1312", "86" clean QSOs an hour.
pub fn rating_label(ranking: Ranking, rating: f64) -> String {
    match ranking {
        Ranking::PerGroup => per_group_label(rating),
        Ranking::Verified(_) | Ranking::CleanRate => format!("{rating:.0}"),
    }
}

/// What follows a rating's figure: "points a group".
pub fn rating_unit(ranking: Ranking) -> &'static str {
    match ranking {
        Ranking::PerGroup => "points a group",
        Ranking::Verified(_) => "verified points",
        Ranking::CleanRate => "clean QSOs an hour",
    }
}

/// A rating said in a line: "72.4 a group", "1312 verified points".
fn rating_said(ranking: Ranking, rating: f64) -> String {
    match ranking {
        Ranking::PerGroup => format!("{} a group", per_group_label(rating)),
        _ => format!("{} {}", rating_label(ranking, rating), rating_unit(ranking)),
    }
}

/// When the session marked on the timeline was set: the row pointed at or
/// tapped, else the best's, set at `best_at`.
fn focus(marked: Memo<Option<u64>>, best_at: u64) -> u64 {
    marked().unwrap_or(best_at)
}

/// What a row of the board was, in figures: "500 points over 20 groups", or
/// for a run whose log reads, "42 of 45 QSOs clean · 10-minute run".
fn what_it_was(entry: &BoardEntry) -> String {
    match (entry.clean, entry.qsos, entry.limit_s) {
        (Some(clean), Some(qsos), Some(limit_s)) => {
            format!("{clean} of {qsos} QSOs clean · {}-minute run", limit_s / 60)
        }
        _ => format!(
            "{} points over {} groups",
            entry.score.round(),
            entry.groups
        ),
    }
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

/// What a contest's board ranks by: the ranking chosen, where this contest
/// has it; else its verified score among runs of the length of its latest
/// run, or its clean QSOs an hour where it has no score.
fn contest_ranking(
    sessions: &[SessionResult],
    kind: ContestKind,
    chosen: Option<Ranking>,
) -> Ranking {
    match chosen {
        Some(Ranking::CleanRate) => Ranking::CleanRate,
        Some(Ranking::Verified(length))
            if kind.scored() && run_lengths(sessions, kind).contains(&length) =>
        {
            Ranking::Verified(length)
        }
        _ => match latest_run_length(sessions, kind) {
            Some(length) if kind.scored() => Ranking::Verified(length),
            _ => Ranking::CleanRate,
        },
    }
}

/// What a contest's board can rank by, as its pills offer it: its verified
/// score at each length its runs were set to, and its clean QSOs an hour.
/// A contest without a score has the one, and no pills.
fn contest_rankings(sessions: &[SessionResult], kind: ContestKind) -> Vec<(Ranking, String)> {
    if !kind.scored() {
        return Vec::new();
    }
    let lengths = run_lengths(sessions, kind);
    let several = lengths.len() > 1;
    lengths
        .into_iter()
        .map(|length| {
            let label = if several {
                format!("Score · {} min", length / 60)
            } else {
                "Verified score".to_string()
            };
            (Ranking::Verified(length), label)
        })
        .chain([(Ranking::CleanRate, "Clean QSOs an hour".to_string())])
        .collect()
}

/// Every session of `mode`, best first, with the personal best, the score
/// over time and the board itself. Pointing at a row (or tapping it) marks
/// on the timeline when it was set. Contest runs are ranked within their
/// contest, chosen by a chip of its own under the Contest pill, by the
/// contest's own measure: the verified score among runs of one length, or
/// the QSOs logged clean an hour. The Runs card there lists every run of
/// it, placed or not; a row there opens the run's results in the tab's
/// place, as History opens them.
#[component]
pub fn ScoresTab(sessions: Vec<SessionResult>) -> Element {
    #[cfg(test)]
    crate::testing::rendered("ScoresTab");
    let mode = use_signal(|| None::<CharSetMode>);
    let kind = use_signal(|| None::<ContestKind>);
    let rank = use_signal(|| None::<Ranking>);
    let hovered = use_signal(|| None::<u64>);
    let pinned = use_signal(|| None::<u64>);
    // The row marked, read by what draws the mark alone: the tab reads
    // every stored run's log to build the board, and must not again for
    // each row a mouse crosses.
    let marked = use_memo(move || hovered().or(pinned()));
    // The run whose results are open, from the Runs card.
    let mut opened = use_opened_run();
    // The Contest pill is always there; a chip for each contest the
    // history holds runs of, placed or not: the Runs card is there.
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
    let (ranking, rankings) = match of {
        BoardOf::Contest(kind) => (
            contest_ranking(&sessions, kind, rank()),
            contest_rankings(&sessions, kind),
        ),
        _ => (Ranking::PerGroup, Vec::new()),
    };
    if let Some(results) = opened_results(&sessions, opened, "Scores") {
        return results;
    }
    let board = score_board(&sessions, of, ranking);
    let assisted = assisted_runs(&sessions, of, ranking);
    let timeline = score_timeline(&sessions, of, ranking);
    let today = crate::time::local_date_string();
    let runs_of = match of {
        BoardOf::Contest(kind) => Some(kind),
        _ => None,
    };
    let pills = rsx! {
        BoardPills {
            modes,
            kinds,
            current,
            of,
            mode,
            kind,
            rankings,
            ranking,
            rank,
        }
    };

    let Some(best) = board.first().cloned() else {
        let (title, body) = empty_board(&sessions, of, !assisted.is_empty());
        return rsx! {
            div { class: "stack",
                {pills}
                div { class: "card",
                    CardHead { icon: IconName::Medal, title }
                    p { class: "muted hint", "{body}" }
                }
                if !assisted.is_empty() {
                    BoardList {
                        board: Vec::new(),
                        assisted,
                        best_at: 0,
                        marked,
                        hovered,
                        pinned,
                        show_mode: current.is_none(),
                        ranking,
                    }
                }
                if let Some(kind) = runs_of {
                    RunsCard { sessions: sessions.clone(), kind, on_open: move |pick| opened.set(Some(pick)) }
                }
            }
        };
    };
    let best_at = best.timestamp;
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
            BestCard {
                best,
                form,
                form_over,
                today,
                show_mode,
                ranking,
            }
            ScoreTimeline {
                timeline,
                board: board.clone(),
                best_at,
                marked,
                total,
                first_date,
                last_date,
                ranking,
            }
            BoardList {
                board,
                assisted,
                best_at,
                marked,
                hovered,
                pinned,
                show_mode,
                ranking,
            }
            if let Some(kind) = runs_of {
                RunsCard { sessions: sessions.clone(), kind, on_open: move |pick| opened.set(Some(pick)) }
            }
        }
    }
}

/// The mode pills, when there is more than one mode to choose; under the
/// Contest pill, a chip for each contest the history holds runs of; and
/// under a contest with a score, what its board ranks by.
#[component]
fn BoardPills(
    modes: Vec<CharSetMode>,
    kinds: Vec<ContestKind>,
    current: Option<CharSetMode>,
    of: BoardOf,
    mode: Signal<Option<CharSetMode>>,
    kind: Signal<Option<ContestKind>>,
    /// What the contest shown can rank by, each with its label.
    rankings: Vec<(Ranking, String)>,
    /// What it ranks by now.
    ranking: Ranking,
    rank: Signal<Option<Ranking>>,
) -> Element {
    let (mut mode, mut kind, mut rank) = (mode, kind, rank);
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
            if rankings.len() > 1 {
                div { class: "mode-pills kind-pills rank-pills",
                    span { class: "rank-label", "Ranked by" }
                    for (each , label) in rankings.iter().cloned() {
                        Seg {
                            label: label.clone(),
                            id: control_id("rank", &label),
                            pill: true,
                            active: ranking == each,
                            onclick: move |_| rank.set(Some(each)),
                        }
                    }
                }
            }
        }
    }
}

/// What a board with nothing placed on it says, `of` the sessions it
/// holds: no contest run yet, under the Contest pill; none scored yet;
/// runs, which go on by their own rule; or sessions too short. A board
/// that lists `assisted` runs says why they are not placed.
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
    // A contest's own board ranks by the contest's measure; a board that
    // mixes runs with sessions, by what each earns a group.
    let ranked = match of {
        BoardOf::Contest(_) => {
            "ranked by verified contest score among runs of the same length, or by QSOs logged clean an hour"
        }
        _ => "ranked by what they earn a group",
    };
    match (scored.is_empty(), runs_only, assisted) {
        (true, _, _) if of == BoardOf::Mode(CharSetMode::Contest) => (
            "No contest runs yet",
            "Choose Contest under Settings → Character set and work a run. Every run that works a station is kept: here each contest gets a board of its own, ranked by its verified score among runs of the same length, or by QSOs logged clean an hour.".to_string(),
        ),
        (true, _, _) => (
            "No scores yet",
            format!(
                "Finish a session of {BOARD_MIN_GROUPS} groups or more and it goes on the board. The best one gets the gold."
            ),
        ),
        (false, true, true) => (
            "No placed run yet",
            format!(
                "Runs with Show copy on are assisted: listed below, never placed. The board takes {}, {ranked}.",
                run_rule()
            ),
        ),
        (false, true, false) => (
            "No run long enough yet",
            format!(
                "The board takes {}, {ranked}. Set Minutes per run to {} or more under Settings → Session shape.",
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

    fn entry() -> BoardEntry {
        BoardEntry {
            place: 1,
            index: 0,
            rating: 25.0,
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
            clean: None,
            limit_s: None,
        }
    }

    #[test]
    fn a_farnsworth_speed_says_both() {
        let mut entry = entry();
        assert_eq!(speed(&entry), "25 WPM");
        entry.effective_wpm = 15.0;
        assert_eq!(speed(&entry), "25/15 WPM");
        assert_eq!(per_group_label(72.44), "72.4");
    }

    /// Each ranking is said in its own unit; a run is told by its QSOs,
    /// any other session by its points and groups.
    #[test]
    fn a_rating_is_said_in_its_own_unit() {
        assert_eq!(rating_said(Ranking::PerGroup, 72.44), "72.4 a group");
        assert_eq!(
            rating_said(Ranking::Verified(600), 1312.4),
            "1312 verified points"
        );
        assert_eq!(
            rating_said(Ranking::CleanRate, 86.6),
            "87 clean QSOs an hour"
        );
        let mut run = entry();
        assert_eq!(what_it_was(&run), "500 points over 20 groups");
        run.qsos = Some(45);
        run.clean = Some(42);
        run.limit_s = Some(600);
        assert_eq!(what_it_was(&run), "42 of 45 QSOs clean · 10-minute run");
    }
}
