use cw_core::{BOARD_MIN_GROUPS, BoardEntry, CharSetMode, Ranking};
use dioxus::prelude::*;

use super::{Medal, mode_label, rating_label, run_rule, speed};
use crate::ui::format::day_and_clock;
use crate::ui::help;
use crate::ui::stats_detail::ASSISTED;
use crate::ui::widgets::{CardHead, Icon, IconName, control_id};

/// The rows the board shows before it is asked for all of them.
const BOARD_TOP: usize = 10;

/// What a board says when it holds contest runs among sessions of other
/// modes: runs earn less a group, so they sit lower than their copy
/// deserves there.
const CONTEST_RUNS_NOTE: &str = "A WPX run earns about 30% less a group than a \
     Callsigns session, since half its groups are short serials. Under Contest, each run is \
     ranked within its own contest.";

/// What a board says when it lists assisted runs.
const ASSISTED_NOTE: &str = "Runs with Show copy on are assisted: listed greyed after the places, never placed, and kept out of the record and the form.";

/// What a row of a board of every mode was: a contest run, flagged with
/// its contest, or a session of its mode.
fn badge(entry: &BoardEntry) -> Element {
    match (entry.mode, entry.contest) {
        (CharSetMode::Contest, contest) => rsx! {
            span { class: "board-badge contest",
                Icon { name: IconName::Flag }
                "{contest.map_or(\"Contest\", |kind| kind.label())}"
            }
        },
        (mode, _) => rsx! {
            span { class: "board-badge", "{mode_label(mode)}" }
        },
    }
}

/// What the board's note says it ranks by, and which sessions it takes.
fn ranked_by(ranking: Ranking, runs_only: bool) -> String {
    match ranking {
        Ranking::Verified(length) => format!(
            "Ranked by verified contest score, of what was logged clean, among runs set to {} minutes. The board takes {}; deleting a run takes it off.",
            length / 60,
            run_rule()
        ),
        Ranking::CleanRate => format!(
            "Ranked by QSOs logged clean an hour, fair between runs of any length. The board takes {}; deleting a run takes it off.",
            run_rule()
        ),
        Ranking::PerGroup if runs_only => format!(
            "Ranked by points a group, over {}. Deleting a run takes it off.",
            run_rule()
        ),
        Ranking::PerGroup => format!(
            "Ranked by points a group, over sessions of {BOARD_MIN_GROUPS} groups or more. Deleting a session takes it off."
        ),
    }
}

/// The board itself, best first: the top ten until asked for every row,
/// then the assisted runs, greyed and with no place. Pointing at a row with
/// a mouse marks it on the timeline for as long as it stays; a tap keeps it
/// marked until the same row is tapped again. An assisted run is on no
/// timeline, so its row marks nothing.
#[component]
pub(super) fn BoardList(
    board: Vec<BoardEntry>,
    /// The whole runs it holds that had Show copy on, newest first.
    assisted: Vec<BoardEntry>,
    /// When the session marked on the timeline was set.
    focus: u64,
    /// The row a mouse is pointing at.
    hovered: Signal<Option<u64>>,
    /// The row tapped to keep marked.
    pinned: Signal<Option<u64>>,
    /// The board is of every mode, so each row says which it was.
    show_mode: bool,
    /// What it ranks by.
    ranking: Ranking,
) -> Element {
    let (mut hovered, mut pinned) = (hovered, pinned);
    let mut show_all = use_signal(|| false);
    let total = board.len() + assisted.len();
    // The rule the card names is the one its rows went on by: a board of
    // runs alone names the run rule, whichever pill is chosen, and one that
    // mixes runs with group sessions says where the runs sit among them.
    let is_run = |entry: &BoardEntry| entry.mode == CharSetMode::Contest;
    let runs_only = board.iter().chain(&assisted).all(is_run);
    let runs_among_others = !runs_only && board.iter().any(is_run);
    let note = ranked_by(ranking, runs_only);
    let (mut rows, mut greyed) = (board, assisted);
    if !show_all() {
        rows.truncate(BOARD_TOP);
        greyed.truncate(BOARD_TOP - rows.len());
    }
    // A run counts its QSOs and says its length, where its log reads; a
    // session counts its groups.
    let detail = |entry: &BoardEntry| {
        let count = match (entry.qsos, entry.limit_s) {
            (Some(qsos), Some(limit_s)) => format!(
                "{qsos} QSO{} · {} min",
                if qsos == 1 { "" } else { "s" },
                limit_s / 60
            ),
            (Some(qsos), None) => format!("{qsos} QSO{}", if qsos == 1 { "" } else { "s" }),
            (None, _) => format!("{} groups", entry.groups),
        };
        format!(
            "{count} · {} · {}%",
            speed(entry),
            (entry.accuracy * 100.0).round()
        )
    };
    let when = |entry: &BoardEntry| day_and_clock(&entry.date, entry.timestamp);
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Medal,
                title: if ranking == Ranking::PerGroup { "Best sessions" } else { "Best runs" },
                help: help::board(ranking),
                note,
            }
            if runs_among_others {
                p { class: "muted", style: "margin: 0; font-size: 0.85rem;", "{CONTEST_RUNS_NOTE}" }
            }
            if !greyed.is_empty() {
                p { class: "muted", style: "margin: 0; font-size: 0.85rem;", "{ASSISTED_NOTE}" }
            }
            div { class: "board-list",
                for entry in rows {
                    {
                        let at = entry.timestamp;
                        let is_focus = at == focus;
                        let is_pinned = pinned() == Some(at);
                        let class = match (is_focus, is_pinned) {
                            (_, true) => "board-row focused pinned",
                            (true, false) => "board-row focused",
                            (false, false) => "board-row",
                        };
                        let (detail, when) = (detail(&entry), when(&entry));
                        rsx! {
                            button {
                                id: control_id("board", &entry.place.to_string()),
                                class: class,
                                aria_pressed: if is_pinned { "true" } else { "false" },
                                onpointerenter: move |evt: PointerEvent| {
                                    if evt.pointer_type() == "mouse" {
                                        hovered.set(Some(at));
                                    }
                                },
                                onpointerleave: move |evt: PointerEvent| {
                                    if evt.pointer_type() == "mouse" && *hovered.peek() == Some(at) {
                                        hovered.set(None);
                                    }
                                },
                                onclick: move |_| {
                                    let again = *pinned.peek() == Some(at);
                                    pinned.set(if again { None } else { Some(at) });
                                },
                                span { class: "board-place",
                                    if entry.place <= 3 {
                                        Medal { place: entry.place }
                                    } else {
                                        span { class: "board-number", "{entry.place}" }
                                    }
                                }
                                span { class: "board-main",
                                    span { class: "board-score", "{rating_label(ranking, entry.rating)}" }
                                    span { class: "board-when", "{when}" }
                                }
                                span { class: "board-detail",
                                    if show_mode {
                                        {badge(&entry)}
                                    }
                                    "{detail}"
                                }
                            }
                        }
                    }
                }
                for entry in greyed {
                    div {
                        key: "{entry.timestamp}",
                        class: "board-row assisted",
                        span { class: "board-place",
                            span { class: "board-number unplaced", aria_label: "Not placed", "—" }
                        }
                        span { class: "board-main",
                            span { class: "board-score", "{rating_label(ranking, entry.rating)}" }
                            span { class: "board-when", "{when(&entry)}" }
                        }
                        span { class: "board-detail",
                            if show_mode {
                                {badge(&entry)}
                            }
                            "{detail(&entry)} · {ASSISTED}"
                        }
                    }
                }
            }
            if total > BOARD_TOP {
                button {
                    id: control_id("board", "all"),
                    class: if show_all() { "link-toggle board-more open" } else { "link-toggle board-more" },
                    onclick: move |_| show_all.toggle(),
                    if show_all() {
                        "Show the top {BOARD_TOP}"
                    } else {
                        "Show all {total}"
                    }
                }
            }
        }
    }
}
