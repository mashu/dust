use std::collections::BTreeSet;

use cw_core::{
    CONTEST_TIMES_NOTE, CharSetMode, ContestKind, GROUP_START_BIGRAM_TOKEN, MasteryStatus,
    SamplingRow, SessionHistoryRow, SessionResult, SpeedForecast, SpeedPath, StoredLog,
    TrainingSettings, adapts_speed, character_diagnostics, group_length_forecast, mistake_stats,
    sampling_rows, session_history, speed_forecast,
};
use dioxus::prelude::*;

use crate::persist::backup::sessions_since;
use crate::state::SessionPick;
use crate::ui::backup::{BackupCard, Backups};
use crate::ui::contest_results::{opened_results, use_opened_run};
use crate::ui::format::counted;
use crate::ui::settings_charset::mode_label;
use crate::ui::stats::{YOUR_SPEED_KEYS, contest_speeds};
use crate::ui::widgets::{CardHead, Icon, IconName, control_id, html_bool};

#[component]
pub fn LettersTab(sessions: Vec<SessionResult>) -> Element {
    let letters = character_diagnostics(&sessions);
    if letters.is_empty() {
        return rsx! {
            div { class: "card", p { class: "muted", "No letter data yet." } }
        };
    }
    let contest = sessions
        .iter()
        .any(|session| session.char_set_mode == CharSetMode::Contest);
    rsx! {
        div { class: "card stack",
            div { class: "tiny", "Letter accuracy" }
            if contest {
                p { class: "muted", "{CONTEST_TIMES_NOTE}" }
            }
            for row in letters.iter() {
                {
                    let status = match row.status {
                        MasteryStatus::Mastered => "Mastered",
                        MasteryStatus::Building => "Building",
                        MasteryStatus::Weak => "Weak",
                    };
                    let slow = if row.is_slow { " · slow" } else { "" };
                    rsx! {
                        div { class: "letter-row",
                            span { class: "letter-key group-text", "{row.letter}" }
                            div { class: "bar-track",
                                span { class: "bar-fill", style: "width: {row.accuracy_pct.clamp(0.0, 100.0)}%;" }
                            }
                            span { class: "mono muted", "{row.accuracy_pct.round() as i32}%" }
                            span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "{status}{slow} · {row.total}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn MistakesTab(sessions: Vec<SessionResult>) -> Element {
    let mistakes = mistake_stats(&sessions, 12);
    let (confusion, heat) = (mistakes.confusions, mistakes.bigrams);
    let n = heat.letters.len();
    rsx! {
        div { class: "stack",
            div { class: "card stack",
                div { class: "tiny", "Common confusions" }
                if confusion.is_empty() {
                    p { class: "muted", "No substitution errors yet." }
                } else {
                    for row in confusion.iter() {
                        {
                            let typed = row.typed.map(|c| c.to_string()).unwrap_or_else(|| "—".into());
                            rsx! {
                                div { class: "confuse-row",
                                    span { class: "group-text", "{row.sent} → {typed}" }
                                    span { class: "muted", "{row.count}× · {row.percentage.round()}%" }
                                }
                            }
                        }
                    }
                }
            }
            if n > 1 {
                div { class: "card stack",
                    div { class: "tiny", "Bigram error rate" }
                    p { class: "muted", "Rows are the previous character ({GROUP_START_BIGRAM_TOKEN} = start of group). Darker means more errors." }
                    div { class: "bigram-scroll",
                        div {
                            class: "bigram-grid",
                            style: "grid-template-columns: repeat({n + 1}, minmax(1.35rem, 1fr));",
                            div { class: "bigram-head" }
                            for col in heat.letters.iter() {
                                div { class: "bigram-head group-text", "{col}" }
                            }
                            for (idx, cell) in heat.cells.iter().enumerate() {
                                {
                                    let show_row = idx % n == 0;
                                    let alpha = if heat.max_rate <= 0.0 {
                                        0.0
                                    } else {
                                        cell.rate / heat.max_rate
                                    };
                                    let row_label = cell.row;
                                    rsx! {
                                        if show_row {
                                            div { class: "bigram-head group-text", "{row_label}" }
                                        }
                                        div {
                                            class: "bigram-cell",
                                            style: "background: rgba(180, 83, 9, {alpha * 0.85});",
                                            title: "{cell.row}{cell.col} {cell.wrong}/{cell.total}",
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn SamplingTab(settings: TrainingSettings, sessions: Vec<SessionResult>) -> Element {
    rsx! {
        div { class: "stack",
            {character_sampling_card(&settings, &sessions)}
            {group_length_card(&settings, &sessions)}
            {speed_card(&settings, &sessions)}
        }
    }
}

/// What the sampler makes of each character. A plain function rather than a
/// component: it holds no state, and is drawn from the tab's own settings and
/// history without copying them.
fn character_sampling_card(settings: &TrainingSettings, sessions: &[SessionResult]) -> Element {
    let rows = sampling_rows(settings, sessions);
    let max_p = rows
        .iter()
        .map(|r| r.sampling_prob)
        .fold(0.0_f64, f64::max)
        .max(0.0001);
    let (drawn, others): (Vec<&SamplingRow>, Vec<&SamplingRow>) =
        rows.iter().partition(|row| row.drawn_now);
    // A contest draws its callers' calls; their exchanges are not drawn.
    let next = if settings.curriculum.char_set_mode == CharSetMode::Contest {
        "of turning up in the next caller's call"
    } else {
        "of being drawn in the next group"
    };
    rsx! {
        div { class: "card stack",
            div { class: "tiny", "Sampling snapshot" }
            p { class: "muted", "Posterior error vs chance {next} (mean weights, not Thompson draws). What every character you have practised has taught counts, whatever set it was sent in." }
            if drawn.is_empty() {
                p { class: "muted", "Pool is empty." }
            }
            for row in drawn {
                {
                    let width = (row.sampling_prob / max_p * 100.0).clamp(4.0, 100.0);
                    rsx! {
                        div { class: "letter-row",
                            span { class: "letter-key group-text", "{row.character}" }
                            div { class: "bar-track",
                                span { class: "bar-fill sample", style: "width: {width}%;" }
                            }
                            span { class: "mono muted", "{(row.sampling_prob * 100.0).round()}%" }
                            span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "p(err) {(row.p_error * 100.0).round()}%" }
                        }
                    }
                }
            }
            if !others.is_empty() {
                div { class: "tiny", "Not in the current set" }
                p { class: "muted", "Practised before, and not drawn now. What you learned of them is kept, and counts again when they come back." }
                for row in others {
                    div { class: "letter-row not-drawn",
                        span { class: "letter-key group-text", "{row.character}" }
                        div { class: "bar-track" }
                        span { class: "mono muted", "—" }
                        span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "p(err) {(row.p_error * 100.0).round()}%" }
                    }
                }
            }
        }
    }
}

/// The group lengths the next session can send: how likely each is, and how
/// often a group that long is copied whole. Nothing for callsigns, which are
/// as long as they are. A plain function, like the character card.
fn group_length_card(settings: &TrainingSettings, sessions: &[SessionResult]) -> Element {
    let rows = group_length_forecast(settings, sessions);
    if rows.is_empty() {
        return rsx! {};
    }
    let max_drawn = rows
        .iter()
        .map(|row| row.drawn)
        .fold(0.0_f64, f64::max)
        .max(0.0001);
    let curriculum = &settings.curriculum;
    let note = if !curriculum.adaptive_group_size {
        "Adaptive group size is off, so every length is as likely as the next. Whole is how often you copy a group that long without a mistake."
    } else if rows.len() == 1 {
        "The group size is fixed, so there is no length to choose. Whole is how often you copy a group that long without a mistake."
    } else {
        "Chance of each length in the next group, and how often you copy a group that long without a mistake."
    };
    rsx! {
        div { class: "card stack",
            div { class: "tiny", "Group length" }
            p { class: "muted", "{note}" }
            for row in rows.iter() {
                {
                    let width = (row.drawn / max_drawn * 100.0).clamp(4.0, 100.0);
                    rsx! {
                        div { class: "letter-row",
                            span { class: "letter-key mono", "{row.length}" }
                            div { class: "bar-track",
                                span { class: "bar-fill sample", style: "width: {width}%;" }
                            }
                            span { class: "mono muted", "{(row.drawn * 100.0).round()}%" }
                            span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "whole {(row.whole * 100.0).round()}%" }
                        }
                    }
                }
            }
        }
    }
}

/// The speeds the next session can send at: how likely the next group is to
/// be sent at about each, and the share of characters copied right at it. A
/// plain function, like the length card.
fn speed_card(settings: &TrainingSettings, sessions: &[SessionResult]) -> Element {
    let rows = speed_forecast(settings, sessions);
    if rows.is_empty() {
        return rsx! {};
    }
    let max_drawn = rows
        .iter()
        .map(|row| row.drawn)
        .fold(0.0_f64, f64::max)
        .max(0.0001);
    let playback = &settings.playback;
    let contest = settings.curriculum.char_set_mode == CharSetMode::Contest;
    let mut note = if contest && adapts_speed(settings) {
        adapting_callers_note(&rows)
    } else if contest {
        format!(
            "{} Right is the share of characters you copy at that speed.",
            contest_speeds(settings)
        )
    } else if !playback.adaptive_speed {
        "Adaptive speed is off, so each group's speed is drawn evenly from your range. Right is the share of characters you copy at that speed.".to_string()
    } else if !SpeedPath::of(settings).has_room() {
        "One speed is set, so there is no speed to choose. Right is the share of characters you copy at it.".to_string()
    } else {
        "Chance of each speed in the next group, and the share of characters you copy right at it."
            .to_string()
    };
    if !playback.link_char_to_effective {
        note.push_str(
            " With Farnsworth spacing, a speed counts the spacing too, as the score does.",
        );
    }
    rsx! {
        div { class: "card stack",
            div { class: "tiny", "Speed, WPM" }
            p { class: "muted", "{note}" }
            for row in rows.iter() {
                {
                    let width = (row.drawn / max_drawn * 100.0).clamp(4.0, 100.0);
                    rsx! {
                        div { class: "letter-row",
                            span { class: "letter-key mono", "{row.wpm:.0}" }
                            div { class: "bar-track",
                                span { class: "bar-fill sample", style: "width: {width}%;" }
                            }
                            span { class: "mono muted", "{(row.drawn * 100.0).round()}%" }
                            span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "right {(row.right * 100.0).round()}%" }
                        }
                    }
                }
            }
        }
    }
}

/// What the speed card says of a contest's callers while their speed
/// adapts: its chances are the first callers' of the next run, which the
/// run moves on from, and when most of them sit at an end of the range, the
/// copy speed is past it.
fn adapting_callers_note(rows: &[SpeedForecast]) -> String {
    let mut note = format!(
        "Chance of each speed for the first callers of your next run, and the share of characters you copy right at it. Within the run they come faster as you copy calls and numbers clean, slower as you miss them. {YOUR_SPEED_KEYS}"
    );
    let most = |row: Option<&SpeedForecast>| row.is_some_and(|row| row.drawn > 0.5);
    if most(rows.last()) {
        note.push_str(" Your copy speed is above this range, so most come at its top: a wider range on the Speed card lets them go faster.");
    } else if most(rows.first()) {
        note.push_str(" Your copy speed is below this range, so most come at its bottom: a wider range on the Speed card lets them go slower.");
    }
    note
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
    let mut opened = use_opened_run();
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
                    for row in rows.iter().take(shown()) {
                        {
                            let pick = SessionPick { index: row.index, timestamp: row.timestamp };
                            let is_chosen = chosen.read().contains(&pick);
                            let key = row.index.to_string();
                            // Spans, as a button that holds them may hold
                            // nothing else.
                            let when = rsx! {
                                span { class: "history-when",
                                    span { class: "mono", "{row.date}" }
                                    span { class: "history-sub", "{history_sub(row)}" }
                                }
                                span { class: "history-acc",
                                    span { class: "value", "{row.accuracy_pct.round()}%" }
                                    span { class: "muted", "{row.score.round()} pts" }
                                }
                            };
                            if selecting_now {
                                rsx! {
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
                                }
                            } else {
                                rsx! {
                                    div { class: "history-item",
                                        // Named by what it shows, as every
                                        // other row is read.
                                        if opens(row) {
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
