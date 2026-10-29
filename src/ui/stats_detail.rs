use std::collections::BTreeSet;

use cw_core::{
    ContestKind, SessionHistoryRow, SessionResult, StoredLog, TrainingSettings, mistake_stats,
    session_history,
};
use dioxus::prelude::*;

use crate::persist::backup::sessions_since;
use crate::state::SessionPick;
use crate::ui::backup::{BackupCard, Backups};
use crate::ui::bigram::{BigramCard, hardest_pairs_card};
use crate::ui::confusion::confusions_card;
use crate::ui::contest_results::{opened_results, use_opened_run};
use crate::ui::forecast::{group_length_card, speed_card};
use crate::ui::format::counted;
use crate::ui::sampling::NextUpCard;
use crate::ui::settings_charset::mode_label;
use crate::ui::widgets::{CardHead, Icon, IconName, control_id, html_bool};

#[component]
pub fn MistakesTab(sessions: Vec<SessionResult>) -> Element {
    let mistakes = mistake_stats(&sessions, 12);
    let (confusion, heat) = (mistakes.confusions, mistakes.bigrams);
    rsx! {
        div { class: "stack",
            {confusions_card(&confusion)}
            if !heat.cols.is_empty() {
                {hardest_pairs_card(&heat)}
                BigramCard { heat }
            }
        }
    }
}

#[component]
pub fn SamplingTab(settings: TrainingSettings, sessions: Vec<SessionResult>) -> Element {
    rsx! {
        div { class: "stack",
            NextUpCard { settings: settings.clone(), sessions: sessions.clone() }
            {group_length_card(&settings, &sessions)}
            {speed_card(&settings, &sessions)}
        }
    }
}

/// How many sessions the history lists before it offers more, as the Runs
/// card does its runs.
pub(crate) const HISTORY_PAGE: usize = 40;

/// What a run with Show copy on is called wherever it is listed.
pub const ASSISTED: &str = "assisted";

/// What a row says under its date: "Koch · 18/20 groups · 96 chars", or for
/// a contest run "Contest · CQ WPX · 11/12 QSOs · 72/h", and for one without
/// a score "Contest · Simple QSO · 8/9 QSOs · 1.2 min/QSO", whose parts each
/// keep to one piece, so a narrow line breaks only between them. A run with
/// Show copy on says it was assisted, after its contest.
fn history_sub(row: &SessionHistoryRow) -> String {
    let Some(log) = &row.log else {
        return format!(
            "{} · {}/{} groups · {} chars",
            mode_label(row.mode),
            row.correct_groups,
            row.groups,
            row.total_chars
        );
    };
    let mut parts = vec![mode_label(row.mode).to_string()];
    if let Some(kind) = row.contest {
        parts.push(kind.label().replace(' ', "\u{a0}"));
    }
    if row.assisted {
        parts.push(ASSISTED.to_string());
    }
    parts.extend(log_parts(log, row.contest));
    parts.join(" · ")
}

/// What a contest run's row says of its log, in History and on the Runs
/// card alike: "11/12 QSOs · 72/h", or for a contest without a score, told
/// by the minutes a QSO took, "8/9 QSOs · 1.2 min/QSO"; and for a log that
/// does not read, why.
pub(crate) fn log_parts(log: &StoredLog, kind: Option<ContestKind>) -> Vec<String> {
    match log {
        StoredLog::Read(run) => {
            let mut parts = vec![format!("{}/{}\u{a0}QSOs", run.clean, run.qsos)];
            if kind.is_none_or(ContestKind::scored) {
                parts.extend(run.rate.map(|rate| format!("{rate:.0}/h")));
            } else {
                let minutes = run.minutes_per_qso();
                parts.extend(minutes.map(|minutes| format!("{minutes:.1}\u{a0}min/QSO")));
            }
            parts
        }
        StoredLog::Dropped => vec!["log not kept by an older version".to_string()],
        StoredLog::Unreadable => vec!["this version cannot read its log".to_string()],
    }
}

/// Whether a row opens its run's results: a contest run whose log reads.
fn opens(row: &SessionHistoryRow) -> bool {
    matches!(row.log, Some(StoredLog::Read(_))) && row.contest.is_some()
}

/// What keys each row of the history by: when its session finished, which
/// stays with it however the rows around it come and go, and how many
/// before it finished at the same moment, should any.
fn row_keys(rows: &[SessionHistoryRow]) -> Vec<String> {
    let mut seen = std::collections::HashMap::<u64, usize>::new();
    rows.iter()
        .map(|row| {
            let alike = seen.entry(row.timestamp).or_default();
            *alike += 1;
            format!("{}-{}", row.timestamp, *alike - 1)
        })
        .collect()
}

/// One session of the history: picked while choosing, otherwise opened to
/// its results where it has them, and deleted from its own row.
#[component]
fn HistoryRow(
    row: SessionHistoryRow,
    selecting: bool,
    chosen: Signal<BTreeSet<SessionPick>>,
    confirming: Signal<bool>,
    opened: Signal<Option<SessionPick>>,
    on_delete: Option<EventHandler<Vec<SessionPick>>>,
) -> Element {
    let pick = SessionPick {
        index: row.index,
        timestamp: row.timestamp,
    };
    let is_chosen = chosen.read().contains(&pick);
    let key = row.index.to_string();
    // Spans, as a button that holds them may hold nothing else.
    let when = rsx! {
        span { class: "history-when",
            span { class: "mono", "{row.date}" }
            span { class: "history-sub", "{history_sub(&row)}" }
        }
        span { class: "history-acc",
            span { class: "value", "{row.accuracy_pct.round()}%" }
            span { class: "muted", "{row.score.round()} pts" }
        }
    };
    if selecting {
        return rsx! {
            button {
                id: control_id("history-pick", &key),
                class: if is_chosen { "history-item selectable chosen" } else { "history-item selectable" },
                role: "checkbox",
                aria_checked: "{is_chosen}",
                onclick: move |_| {
                    let mut set = chosen.write();
                    if !set.remove(&pick) {
                        set.insert(pick);
                    }
                    confirming.set(false);
                },
                span { class: "history-check", Icon { name: IconName::Check } }
                {when}
            }
        };
    }
    rsx! {
        div { class: "history-item",
            // Named by what it shows, as every other row is read.
            if opens(&row) {
                button {
                    id: control_id("history-open", &key),
                    class: "history-open",
                    onclick: move |_| opened.set(Some(pick)),
                    {when}
                    span { class: "history-open-mark", Icon { name: IconName::Chevron } }
                }
            } else {
                {when}
            }
            if let Some(on_delete) = on_delete {
                button {
                    id: control_id("history-delete", &key),
                    class: "history-delete",
                    title: "Delete this session",
                    aria_label: "Delete the session of {row.date}",
                    onclick: move |_| on_delete.call(vec![pick]),
                    Icon { name: IconName::Trash }
                }
            }
        }
    }
}

/// Every session kept on this device, newest first.
///
/// Given `on_delete`, sessions can be deleted: one from its own row, or
/// several chosen together in select mode, all of them included. Deleting
/// everything asks once more; anything else is put right with the Undo the
/// app offers after. Given `backups`, the history can be saved to a file and
/// restored from one, above the list.
#[component]
pub fn HistoryTab(
    sessions: Vec<SessionResult>,
    on_delete: Option<EventHandler<Vec<SessionPick>>>,
    backups: Option<Backups>,
) -> Element {
    // Each run's log is read and checked to make its row: once for the
    // history as it stands, not again for every choice made in the list.
    let rows = use_memo(use_reactive!(|sessions| session_history(&sessions)));
    let rows = rows.read();
    let mut selecting = use_signal(|| false);
    let mut chosen = use_signal(BTreeSet::<SessionPick>::new);
    let mut confirming = use_signal(|| false);
    let mut shown = use_signal(|| HISTORY_PAGE);
    // The run whose results are open, read again from its row.
    let opened = use_opened_run();
    if let Some(results) = opened_results(&sessions, opened, "History") {
        return results;
    }

    let every: Vec<SessionPick> = rows
        .iter()
        .map(|row| SessionPick {
            index: row.index,
            timestamp: row.timestamp,
        })
        .collect();
    let total = every.len();
    let selecting_now = selecting() && on_delete.is_some() && total > 0;
    // Only what is still in the history counts as chosen.
    let picked: Vec<SessionPick> = every
        .iter()
        .filter(|pick| chosen.read().contains(pick))
        .copied()
        .collect();
    let count = picked.len();
    let everything = count == total;
    let mut finish = move || {
        selecting.set(false);
        chosen.write().clear();
        confirming.set(false);
    };
    let more = total.saturating_sub(shown()).min(HISTORY_PAGE);

    let since = backups
        .as_ref()
        .map(|backups| sessions_since(&sessions, backups.last))
        .unwrap_or_default();

    rsx! {
        if let Some(backups) = backups {
            BackupCard { backups, since }
        }
        div { class: if selecting_now { "card stack-sm history-card selecting" } else { "card stack-sm history-card" },
            CardHead {
                icon: IconName::Repeat,
                title: "Session history",
                note: format!("{} on this device", counted(total, "session")),
                if on_delete.is_some() && total > 0 {
                    if selecting_now {
                        button {
                            id: control_id("history", "done"),
                            class: "btn btn-sm btn-secondary",
                            onclick: move |_| finish(),
                            "Done"
                        }
                    } else {
                        button {
                            id: control_id("history", "select"),
                            class: "btn btn-sm btn-secondary",
                            onclick: move |_| selecting.set(true),
                            "Select"
                        }
                    }
                }
            }
            if rows.is_empty() {
                div { class: "history-empty",
                    span { class: "card-icon", Icon { name: IconName::Repeat } }
                    p { "No sessions stored yet." }
                    p { class: "muted", "Finish a session and it is kept here." }
                }
            } else {
                div { class: "history-list",
                    for (row, key) in rows.iter().take(shown()).zip(row_keys(&rows)) {
                        HistoryRow {
                            key: "{key}",
                            row: row.clone(),
                            selecting: selecting_now,
                            chosen,
                            confirming,
                            opened,
                            on_delete,
                        }
                    }
                }
                if more > 0 {
                    button {
                        id: control_id("history", "more"),
                        class: "btn btn-sm btn-secondary history-more",
                        onclick: move |_| shown += HISTORY_PAGE,
                        "Show {more} more"
                    }
                }
            }
        }
        if selecting_now {
            div { class: "select-dock",
                div { class: if confirming() { "select-bar confirming" } else { "select-bar" },
                    role: "toolbar",
                    aria_label: "Chosen sessions",
                    if confirming() {
                        span { class: "select-count", "Delete all {total}?" }
                        button {
                            id: control_id("history", "keep"),
                            class: "select-btn",
                            onclick: move |_| confirming.set(false),
                            "Keep"
                        }
                        button {
                            id: control_id("history", "delete-all"),
                            class: "select-btn danger",
                            onclick: move |_| {
                                if let Some(on_delete) = on_delete {
                                    on_delete.call(every.clone());
                                }
                                finish();
                            },
                            "Delete all"
                        }
                    } else {
                        button {
                            id: control_id("history", "choose-all"),
                            class: "select-btn",
                            onclick: move |_| {
                                let mut set = chosen.write();
                                if everything {
                                    set.clear();
                                } else {
                                    set.extend(every.iter().copied());
                                }
                            },
                            if everything { "Select none" } else { "Select all" }
                        }
                        span { class: "select-count", "{count} selected" }
                        button {
                            id: control_id("history", "delete-chosen"),
                            class: "select-btn danger",
                            disabled: html_bool(count == 0),
                            onclick: move |_| {
                                if everything {
                                    confirming.set(true);
                                } else {
                                    if let Some(on_delete) = on_delete {
                                        on_delete.call(picked.clone());
                                    }
                                    finish();
                                }
                            },
                            Icon { name: IconName::Trash }
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod row_key_tests {
    use super::*;

    fn row(timestamp: u64, index: usize) -> SessionHistoryRow {
        SessionHistoryRow {
            date: "2026-10-29".into(),
            accuracy_pct: 90.0,
            score: 10.0,
            groups: 10,
            correct_groups: 9,
            total_chars: 30,
            timestamp,
            mode: cw_core::CharSetMode::Koch,
            contest: None,
            log: None,
            assisted: false,
            index,
        }
    }

    /// A row keeps its key however the rows around it come and go, and two
    /// sessions that finished in the same millisecond have keys of their own.
    #[test]
    fn a_row_keeps_its_key_and_no_two_rows_share_one() {
        let rows = [row(30, 2), row(20, 1), row(20, 0)];
        assert_eq!(row_keys(&rows), ["30-0", "20-0", "20-1"]);
        let after_a_delete = [row(20, 1), row(20, 0)];
        assert_eq!(row_keys(&after_a_delete), ["20-0", "20-1"]);
        let keys = row_keys(&[row(30, 1), row(10, 0)]);
        assert_eq!(
            keys[0], "30-0",
            "the row above a deleted one is keyed as before"
        );
    }
}
