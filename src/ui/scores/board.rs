use cw_core::{BOARD_MIN_GROUPS, BoardEntry, CharSetMode};
use dioxus::prelude::*;

use super::{Medal, mode_label, per_group_label, run_rule, speed};
use crate::ui::format::{clock, short_date};
use crate::ui::widgets::{CardHead, IconName, control_id};

/// The rows the board shows before it is asked for all of them.
const BOARD_TOP: usize = 10;

/// What a board says when it holds contest runs among sessions of other
/// modes: runs earn less a group, so they sit lower than their copy
/// deserves there.
const CONTEST_RUNS_NOTE: &str = "A WPX run earns about 30% less a group than a \
     Callsigns session, since half its groups are short serials. Under Contest, each run is \
     ranked within its own contest.";

/// The board itself, best first: the top ten until asked for every row.
/// Pointing at a row with a mouse marks it on the timeline for as long as
/// it stays; a tap keeps it marked until the same row is tapped again.
#[component]
pub(super) fn BoardList(
    board: Vec<BoardEntry>,
    /// When the session marked on the timeline was set.
    focus: u64,
    /// The row a mouse is pointing at.
    hovered: Signal<Option<u64>>,
    /// The row tapped to keep marked.
    pinned: Signal<Option<u64>>,
    /// The board is of every mode, so each row says which it was.
    show_mode: bool,
) -> Element {
    let (mut hovered, mut pinned) = (hovered, pinned);
    let mut show_all = use_signal(|| false);
    let total = board.len();
    // The rule the card names is the one its rows went on by: a board of
    // runs alone names the run rule, whichever pill is chosen, and one that
    // mixes runs with group sessions says where the runs sit among them.
    let is_run = |entry: &BoardEntry| entry.mode == CharSetMode::Contest;
    let runs_only = board.iter().all(is_run);
    let runs_among_others = !runs_only && board.iter().any(is_run);
    let note = if runs_only {
        format!(
            "Ranked by points a group, over {}. Deleting a run takes it off.",
            run_rule()
        )
    } else {
        format!(
            "Ranked by points a group, over sessions of {BOARD_MIN_GROUPS} groups or more. Deleting a session takes it off."
        )
    };
    let mut rows = board;
    if !show_all() {
        rows.truncate(BOARD_TOP);
    }
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Medal,
                title: "Best sessions",
                note,
            }
            if runs_among_others {
                p { class: "muted", style: "margin: 0; font-size: 0.85rem;", "{CONTEST_RUNS_NOTE}" }
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
                        let mut detail = format!(
                            "{} groups · {} · {}%",
                            entry.groups,
                            speed(&entry),
                            (entry.accuracy * 100.0).round()
                        );
                        if show_mode {
                            detail.push_str(&format!(" · {}", mode_label(entry.mode)));
                        }
                        let when = match clock(at) {
                            Some(time) => format!("{} · {time}", short_date(&entry.date)),
                            None => short_date(&entry.date),
                        };
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
                                    span { class: "board-score", "{per_group_label(entry.per_group)}" }
                                    span { class: "board-when", "{when}" }
                                }
                                span { class: "board-detail", "{detail}" }
                            }
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
