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
    chart_buckets, chart_day, latest_run_length, run_kinds, run_lengths, score_board,
    score_timeline,
};
use dioxus::prelude::*;

use crate::ui::contest_results::{opened_results, use_opened_run};
use crate::ui::figures::History;
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

/// The lengths runs of a contest were set to ([`run_lengths`]), and the
/// latest's ([`latest_run_length`]): what its board can rank by.
struct RunLengths {
    lengths: Vec<u32>,
    latest: Option<u32>,
}

impl RunLengths {
    fn of(sessions: &[SessionResult], kind: ContestKind) -> Self {
        Self {
            lengths: run_lengths(sessions, kind),
            latest: latest_run_length(sessions, kind),
        }
    }
}

/// What a contest's board ranks by: the ranking chosen, where this contest
/// has it; else its verified score among runs of the length of its latest
/// run, or its clean QSOs an hour where it has no score.
fn contest_ranking(runs: &RunLengths, kind: ContestKind, chosen: Option<Ranking>) -> Ranking {
    match chosen {
        Some(Ranking::CleanRate) => Ranking::CleanRate,
        Some(Ranking::Verified(length)) if kind.scored() && runs.lengths.contains(&length) => {
            Ranking::Verified(length)
        }
        _ => match runs.latest {
            Some(length) if kind.scored() => Ranking::Verified(length),
            _ => Ranking::CleanRate,
        },
    }
}

/// What a contest's board can rank by, as its pills offer it: its verified
/// score at each length its runs were set to, and its clean QSOs an hour.
/// A contest without a score has the one, and no pills.
fn contest_rankings(runs: &RunLengths, kind: ContestKind) -> Vec<(Ranking, String)> {
    if !kind.scored() {
        return Vec::new();
    }
    let lengths = &runs.lengths;
    let several = lengths.len() > 1;
    lengths
        .iter()
        .copied()
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

/// What the history holds boards and runs of: worked out once for it.
struct Choices {
    /// The modes with sessions on the board ([`board_modes`]).
    modes: Vec<CharSetMode>,
    /// The contests it holds runs of, placed or not ([`run_kinds`]).
    kinds: Vec<ContestKind>,
    /// Those with runs on the board ([`board_kinds`]).
    placed: Vec<ContestKind>,
}

/// One board of the history, as the tab draws it: worked out once for the
/// history, what the board is of and what it ranks by.
struct Board {
    /// Its rows, best first.
    entries: Vec<BoardEntry>,
    /// The runs with Show copy on, listed but never placed.
    assisted: Vec<BoardEntry>,
    /// Every session on it, oldest first.
    timeline: Vec<TimelinePoint>,
    /// Which of those the timeline draws a dot for ([`timeline_dots`]).
    dots: Vec<usize>,
    /// The dates of the first session on the timeline and the last.
    first_date: String,
    last_date: String,
    /// What it says while it places nothing ([`empty_board`]).
    empty: Option<(&'static str, String)>,
}

impl Board {
    fn of(sessions: &[SessionResult], of: BoardOf, ranking: Ranking) -> Self {
        let entries = score_board(sessions, of, ranking);
        let assisted = assisted_runs(sessions, of, ranking);
        let timeline = score_timeline(sessions, of, ranking);
        let date_of = |point: Option<&TimelinePoint>| {
            point
                .and_then(|point| sessions.get(point.index))
                .map(|session| short_date(&session.date))
                .unwrap_or_default()
        };
        Self {
            dots: timeline_dots(sessions, &timeline),
            first_date: date_of(timeline.first()),
            last_date: date_of(timeline.last()),
            empty: entries
                .is_empty()
                .then(|| empty_board(sessions, of, !assisted.is_empty())),
            entries,
            assisted,
            timeline,
        }
    }
}

/// Which sessions of `timeline` the timeline draws a dot for, and the form
/// through: every one up to [`cw_core::PER_SESSION_UP_TO`]; past that, the
/// newest each, and of each older week or month the best, so that the
/// record it reached still shows ([`chart_buckets`]).
fn timeline_dots(sessions: &[SessionResult], timeline: &[TimelinePoint]) -> Vec<usize> {
    let days: Vec<i64> = timeline
        .iter()
        .map(|point| {
            let date = sessions
                .get(point.index)
                .map_or("", |session| &session.date);
            chart_day(date, point.timestamp)
        })
        .collect();
    chart_buckets(&days)
        .into_iter()
        .filter_map(|range| {
            range.max_by(|a, b| timeline[*a].rating.total_cmp(&timeline[*b].rating))
        })
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
///
/// Each board reads every stored run's log: it is worked out once for the
/// history, and for what it is of and ranks by, and found again as the
/// pills go back to it ([`History::figure`]).
#[component]
pub fn ScoresTab(#[props(into)] sessions: History) -> Element {
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
    let choices = sessions.figure("board choices", (), |sessions| Choices {
        modes: board_modes(sessions),
        kinds: run_kinds(sessions),
        placed: board_kinds(sessions),
    });
    let (modes, kinds) = (choices.modes.clone(), choices.kinds.clone());
    // A mode whose sessions have all been deleted shows every mode again,
    // and a contest with none left the first that has runs on the board, or
    // else the first that has runs.
    let current = mode().filter(|chosen| modes.contains(chosen));
    let contest = kind()
        .filter(|chosen| kinds.contains(chosen))
        .or_else(|| choices.placed.first().copied())
        .or(kinds.first().copied());
    let of = match (current, contest) {
        (None, _) => BoardOf::All,
        (Some(CharSetMode::Contest), Some(contest)) => BoardOf::Contest(contest),
        (Some(mode), _) => BoardOf::Mode(mode),
    };
    let (ranking, rankings) = match of {
        BoardOf::Contest(kind) => {
            let runs = sessions.figure("run lengths", kind, |sessions| {
                RunLengths::of(sessions, kind)
            });
            (
                contest_ranking(&runs, kind, rank()),
                contest_rankings(&runs, kind),
            )
        }
        _ => (Ranking::PerGroup, Vec::new()),
    };
    if let Some(results) = sessions.with(|all| opened_results(all, opened, "Scores")) {
        return results;
    }
    let board = sessions.figure("score board", (of, ranking), |sessions| {
        Board::of(sessions, of, ranking)
    });
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

    let Some(best) = board.entries.first().cloned() else {
        let (title, body) = board.empty.clone().unwrap_or_default();
        return rsx! {
            div { class: "stack",
                {pills}
                div { class: "card",
                    CardHead { icon: IconName::Medal, title }
                    p { class: "muted hint", "{body}" }
                }
                if !board.assisted.is_empty() {
                    BoardList {
                        board: Vec::new(),
                        assisted: board.assisted.clone(),
                        best_at: 0,
                        marked,
                        hovered,
                        pinned,
                        show_mode: current.is_none(),
                        ranking,
                    }
                }
                if let Some(kind) = runs_of {
                    RunsCard { sessions, kind, on_open: move |pick| opened.set(Some(pick)) }
                }
            }
        };
    };
    let best_at = best.timestamp;
    let form = board.timeline.last().map(|point| point.form);
    let form_over = board.timeline.len().min(FORM_SESSIONS);
    let total = board.entries.len();
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
                timeline: board.timeline.clone(),
                dots: board.dots.clone(),
                board: board.entries.clone(),
                best_at,
                marked,
                total,
                first_date: board.first_date.clone(),
                last_date: board.last_date.clone(),
                ranking,
            }
            BoardList {
                board: board.entries.clone(),
                assisted: board.assisted.clone(),
                best_at,
                marked,
                hovered,
                pinned,
                show_mode,
                ranking,
            }
            if let Some(kind) = runs_of {
                RunsCard { sessions, kind, on_open: move |pick| opened.set(Some(pick)) }
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
    use cw_core::{POINTS_UP_TO, RECENT_SESSIONS};

    /// Past a thousand sessions on the board, the timeline draws the best
    /// of each older week, and every one of the newest: the record still
    /// shows, on a chart that keeps to its points. A shorter one draws
    /// every session.
    #[test]
    fn a_long_timeline_draws_the_best_of_each_older_week() {
        let first = chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sessions: Vec<SessionResult> = (0..1_100_u64)
            .map(|at| SessionResult {
                date: (first + chrono::Days::new(at / 2))
                    .format("%Y-%m-%d")
                    .to_string(),
                timestamp: at,
                ..SessionResult::empty_for_test()
            })
            .collect();
        let timeline: Vec<TimelinePoint> = (0..1_100_usize)
            .map(|at| TimelinePoint {
                index: at,
                timestamp: at as u64,
                rating: if at == 5 { 99.0 } else { (at % 7) as f64 },
                record: 99.0,
                set_record: at == 5,
                form: 3.0,
            })
            .collect();
        let dots = timeline_dots(&sessions, &timeline);
        assert!(dots.len() <= POINTS_UP_TO, "{} dots", dots.len());
        assert!(dots.contains(&5), "the record has its dot");
        let newest: Vec<usize> = (1_100 - RECENT_SESSIONS..1_100).collect();
        assert!(dots.ends_with(&newest));
        assert!(dots.is_sorted());
        assert_eq!(
            timeline_dots(&sessions[..10], &timeline[..10]),
            (0..10).collect::<Vec<_>>()
        );
    }

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
