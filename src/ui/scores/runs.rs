//! The Runs card: every run of one contest kept on this device, newest
//! first, with its length, its QSOs and rate, both its scores and the share
//! of its QSOs with an error. The best verified run of each length is
//! marked; a run with Show copy on is marked assisted, and is never the
//! best; a run whose log this build cannot read is listed as History lists
//! it. A row whose log reads opens the run's results, as a History row
//! does. A page of runs at a time, as History lists its sessions, the best
//! of each length wherever it falls.

use cw_core::{ContestKind, RunEntry, SessionResult, StoredLog, contest_runs};
use dioxus::prelude::*;

use crate::state::SessionPick;
use crate::ui::format::{counted, day_and_clock, minutes_seconds};
use crate::ui::help;
use crate::ui::stats_detail::{ASSISTED, HISTORY_PAGE, log_parts};
use crate::ui::widgets::{CardHead, Icon, IconName, control_id};

/// Every run of `kind` in `sessions`, newest first. A row whose log reads
/// opens its results through `on_open`.
#[component]
pub(super) fn RunsCard(
    sessions: Vec<SessionResult>,
    kind: ContestKind,
    on_open: EventHandler<SessionPick>,
) -> Element {
    // Each run's log is read and scored to make its row: once for the
    // history and contest shown, not again for every redraw.
    let runs = use_memo(use_reactive!(|sessions, kind| contest_runs(
        &sessions, kind
    )));
    let runs = runs.read();
    let mut shown = use_signal(|| HISTORY_PAGE);
    // The newest a page at a time, and the best of each length wherever it
    // falls, so that every mark the card makes is in sight.
    let page = shown();
    let mut plain = 0;
    let listed: Vec<&RunEntry> = runs
        .iter()
        .filter(|run| {
            plain += usize::from(!run.best);
            run.best || plain <= page
        })
        .collect();
    let more = (runs.len() - listed.len()).min(HISTORY_PAGE);
    let note = format!(
        "{} of {} on this device, newest first",
        counted(runs.len(), "run"),
        kind.label()
    );
    rsx! {
        div { class: "card stack-sm runs-card",
            CardHead { icon: IconName::Flag, title: "Runs", note, help: help::runs() }
            p { class: "muted runs-key",
                if kind.scored() {
                    "Verified is the contest score of what was logged clean, raw of everything logged. The best verified run of each length is marked; an assisted run, with Show copy on, never is."
                } else {
                    "An everyday QSO has no contest score. An assisted run, with Show copy on, is marked."
                }
            }
            div { class: "history-list runs-list",
                for run in listed {
                    RunRow { key: "{run.index}", run: run.clone(), kind, on_open }
                }
            }
            if more > 0 {
                button {
                    id: control_id("runs", "more"),
                    class: "btn btn-sm btn-secondary history-more",
                    onclick: move |_| shown += HISTORY_PAGE,
                    "Show {more} more"
                }
            }
        }
    }
}

/// One run: when, how long, its QSOs, rate, points and error share, and
/// its contest score; marked best or assisted.
#[component]
fn RunRow(run: RunEntry, kind: ContestKind, on_open: EventHandler<SessionPick>) -> Element {
    let pick = SessionPick {
        index: run.index,
        timestamp: run.timestamp,
    };
    let opens = matches!(run.log, StoredLog::Read(_));
    let points = run.points.round();
    // Spans, as a button that holds them may hold nothing else.
    let shown = rsx! {
        span { class: "history-when",
            span { class: "runs-head",
                span { class: "runs-date", "{day_and_clock(&run.date, run.timestamp)}" }
                if run.best {
                    span { class: "chip good", "Best" }
                }
                if run.assisted {
                    span { class: "chip quiet", "{ASSISTED}" }
                }
            }
            span { class: "history-sub", "{run_sub(&run, kind)}" }
        }
        span { class: "history-acc",
            if let Some(verified) = run.verified {
                span { class: "value", "{verified}" }
                span { class: "muted", "{run.raw.unwrap_or_default()}\u{a0}raw" }
            } else if kind.scored() {
                // No contest score to show where its log does not read: the
                // column of scores keeps to scores, and its Points go under
                // it, as History puts them.
                span { class: "value", "—" }
                span { class: "muted", "{points}\u{a0}pts" }
            } else {
                span { class: "value", "{points}" }
                span { class: "muted", "pts" }
            }
        }
    };
    let class = if run.assisted {
        "history-item runs-row assisted"
    } else {
        "history-item runs-row"
    };
    rsx! {
        div { class,
            if opens {
                button {
                    id: control_id("runs-open", &run.index.to_string()),
                    class: "history-open",
                    onclick: move |_| on_open.call(pick),
                    {shown}
                    span { class: "history-open-mark", Icon { name: IconName::Chevron } }
                }
            } else {
                // Nothing to open: its figures keep to the column of those
                // that have the chevron.
                div { class: "history-open",
                    {shown}
                    span { class: "history-open-mark runs-no-open" }
                }
            }
        }
    }
}

/// What a run's row says under its date: "10 min · 11/12 QSOs · 72/h ·
/// 8% errors · 496 pts", "03:12 of 10 min" for a run ended early, and a
/// contest without a score its minutes a QSO in place of the rate, its
/// Points beside the row in place of the contest score. A run whose log
/// does not read says so, as History does. Each part keeps to one piece,
/// so a narrow line breaks only between them.
fn run_sub(run: &RunEntry, kind: ContestKind) -> String {
    let mut parts = Vec::new();
    if let (Some(limit), Some(ran)) = (run.limit_s, run.run_s) {
        let limit_min = limit / 60;
        // The run time is kept in whole seconds, cut down: a run that went
        // its length may read a second short of it.
        if ran + 1 < u64::from(limit) {
            parts.push(format!(
                "{} of {limit_min}\u{a0}min",
                minutes_seconds(ran * 1_000)
            ));
        } else {
            parts.push(format!("{limit_min}\u{a0}min"));
        }
    }
    parts.extend(log_parts(&run.log, Some(kind)));
    if let StoredLog::Read(summary) = &run.log
        && let Some(share) = summary.error_share()
    {
        parts.push(format!("{:.0}%\u{a0}errors", (share * 100.0).round()));
    }
    if run.verified.is_some() {
        parts.push(format!("{:.0}\u{a0}pts", run.points.round()));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::RunSummary;

    /// A ten-minute run whose log reads: twelve QSOs, one with an error.
    fn entry() -> RunEntry {
        RunEntry {
            index: 0,
            timestamp: 1,
            date: "2026-10-20".into(),
            log: StoredLog::Read(RunSummary {
                qsos: 12,
                clean: 11,
                rate: Some(72.0),
                repeated: 0,
                lost: 0,
            }),
            limit_s: Some(600),
            run_s: Some(600),
            points: 496.4,
            raw: Some(144),
            verified: Some(121),
            assisted: false,
            best: false,
        }
    }

    /// A run whose log does not read, as `log` says: nothing but its points.
    fn unread(log: StoredLog) -> RunEntry {
        RunEntry {
            log,
            limit_s: None,
            run_s: None,
            raw: None,
            verified: None,
            ..entry()
        }
    }

    /// A row says how long the run was, its QSOs, rate, errors and points,
    /// each part whole; one ended early how far it went.
    #[test]
    fn a_row_says_what_the_run_was() {
        let run = entry();
        assert_eq!(
            run_sub(&run, ContestKind::Wpx),
            "10\u{a0}min · 11/12\u{a0}QSOs · 72/h · 8%\u{a0}errors · 496\u{a0}pts"
        );
        let early = RunEntry {
            run_s: Some(192),
            ..run.clone()
        };
        assert!(run_sub(&early, ContestKind::Wpx).starts_with("03:12 of 10\u{a0}min · "));
        let a_second_short = RunEntry {
            run_s: Some(599),
            ..run.clone()
        };
        assert!(run_sub(&a_second_short, ContestKind::Wpx).starts_with("10\u{a0}min · "));
        let simple = RunEntry {
            raw: None,
            verified: None,
            ..run
        };
        assert_eq!(
            run_sub(&simple, ContestKind::SimpleQso),
            "10\u{a0}min · 11/12\u{a0}QSOs · 0.8\u{a0}min/QSO · 8%\u{a0}errors"
        );
        assert_eq!(
            run_sub(&unread(StoredLog::Dropped), ContestKind::Wpx),
            "log not kept by an older version"
        );
        assert_eq!(
            run_sub(&unread(StoredLog::Unreadable), ContestKind::Wpx),
            "this version cannot read its log"
        );
    }
}
