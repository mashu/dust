use cw_core::{
    bigram_heatmap, confusion_entries, sampling_rows, session_history, CharacterDiagnostic,
    MasteryStatus, SessionResult, TrainingSettings, GROUP_START_BIGRAM_TOKEN,
};
use dioxus::prelude::*;

#[component]
pub fn LettersTab(letters: Vec<CharacterDiagnostic>) -> Element {
    if letters.is_empty() {
        return rsx! {
            div { class: "card", p { class: "muted", "No letter data yet." } }
        };
    }
    rsx! {
        div { class: "card stack",
            div { class: "tiny", "Letter accuracy" }
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
                            span { class: "letter-key mono", "{row.letter}" }
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
    let confusion = confusion_entries(&sessions, 12);
    let heat = bigram_heatmap(&sessions);
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
                                    span { class: "mono", "{row.sent} → {typed}" }
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
                                div { class: "bigram-head mono", "{col}" }
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
                                            div { class: "bigram-head mono", "{row_label}" }
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
    let rows = sampling_rows(&settings, &sessions);
    let max_p = rows
        .iter()
        .map(|r| r.sampling_prob)
        .fold(0.0_f64, f64::max)
        .max(0.0001);
    rsx! {
        div { class: "card stack",
            div { class: "tiny", "Sampling snapshot" }
            p { class: "muted", "Posterior error vs chance of being drawn in the next group (mean weights, not Thompson draws)." }
            if rows.is_empty() {
                p { class: "muted", "Pool is empty." }
            } else {
                for row in rows.iter() {
                    {
                        let width = (row.sampling_prob / max_p * 100.0).clamp(4.0, 100.0);
                        rsx! {
                            div { class: "letter-row",
                                span { class: "letter-key mono", "{row.character}" }
                                div { class: "bar-track",
                                    span { class: "bar-fill sample", style: "width: {width}%;" }
                                }
                                span { class: "mono muted", "{(row.sampling_prob * 100.0).round()}%" }
                                span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "p(err) {(row.p_error * 100.0).round()}%" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn HistoryTab(sessions: Vec<SessionResult>) -> Element {
    let rows = session_history(&sessions);
    rsx! {
        div { class: "card stack",
            div { class: "tiny", "Session history" }
            if rows.is_empty() {
                p { class: "muted", "No sessions stored yet." }
            } else {
                for row in rows.iter().take(40) {
                    div { class: "history-row",
                        div {
                            div { class: "mono", "{row.date}" }
                            div { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "{row.correct_groups}/{row.groups} groups · {row.total_chars} chars" }
                        }
                        div { class: "history-acc",
                            span { class: "value", "{row.accuracy_pct.round()}%" }
                            span { class: "muted", "{row.score.round()}" }
                        }
                    }
                }
            }
        }
    }
}
