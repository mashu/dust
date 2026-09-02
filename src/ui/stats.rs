use cw_core::{
    accuracy_chart, character_diagnostics, AccuracyPoint, MasteryStatus, SessionResult,
    TrainingSettings,
};
use dioxus::prelude::*;

use crate::ui::stats_detail::{HistoryTab, LettersTab, MistakesTab, SamplingTab};
use crate::ui::widgets::ModePill;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatsTab {
    Overview,
    Letters,
    Mistakes,
    Sampling,
    History,
}

fn sparkline_points(points: &[AccuracyPoint]) -> String {
    if points.is_empty() {
        return String::new();
    }
    let w = 280.0;
    let h = 72.0;
    let last = (points.len() - 1).max(1) as f64;
    points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let x = i as f64 / last * w;
            let y = h - (p.accuracy_pct.clamp(0.0, 100.0) / 100.0) * h;
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[component]
pub fn StatsView(settings: TrainingSettings, sessions: Vec<SessionResult>) -> Element {
    let mut tab = use_signal(|| StatsTab::Overview);
    let matching: Vec<SessionResult> = sessions
        .iter()
        .filter(|s| s.usable_for_sampling(&settings))
        .cloned()
        .collect();
    let letters = character_diagnostics(&matching);
    rsx! {
        div { class: "stack stats-page",
            h2 { class: "page-title", "Stats" }
            div { class: "mode-pills tab-bar",
                ModePill { label: "Overview".to_string(), active: tab() == StatsTab::Overview, onclick: move |_| tab.set(StatsTab::Overview) }
                ModePill { label: "Letters".to_string(), active: tab() == StatsTab::Letters, onclick: move |_| tab.set(StatsTab::Letters) }
                ModePill { label: "Mistakes".to_string(), active: tab() == StatsTab::Mistakes, onclick: move |_| tab.set(StatsTab::Mistakes) }
                ModePill { label: "Sampling".to_string(), active: tab() == StatsTab::Sampling, onclick: move |_| tab.set(StatsTab::Sampling) }
                ModePill { label: "History".to_string(), active: tab() == StatsTab::History, onclick: move |_| tab.set(StatsTab::History) }
            }
            match tab() {
                StatsTab::Overview => rsx! {
                    OverviewTab { sessions: matching }
                },
                StatsTab::Letters => rsx! { LettersTab { letters } },
                StatsTab::Mistakes => rsx! { MistakesTab { sessions: matching } },
                StatsTab::Sampling => rsx! { SamplingTab { settings, sessions: matching } },
                StatsTab::History => rsx! { HistoryTab { sessions } },
            }
        }
    }
}

#[component]
fn OverviewTab(sessions: Vec<SessionResult>) -> Element {
    let chart = accuracy_chart(&sessions);
    let letters = character_diagnostics(&sessions);
    let avg = if sessions.is_empty() {
        0.0
    } else {
        sessions.iter().map(|s| s.accuracy).sum::<f64>() / sessions.len() as f64 * 100.0
    };
    let best = sessions
        .iter()
        .map(|s| s.accuracy * 100.0)
        .fold(0.0_f64, f64::max);
    let mastered = letters
        .iter()
        .filter(|d| d.status == MasteryStatus::Mastered)
        .count();
    let points = sparkline_points(&chart);
    let empty = sessions.is_empty();
    let session_count = sessions.len();
    rsx! {
        div { class: "stack",
            if empty {
                div { class: "card",
                    p { class: "muted", "Complete a session to see accuracy over time, letter mastery, and sampling weights." }
                }
            } else {
                div { class: "grid-3",
                    div { class: "kpi emerald",
                        div { class: "tiny", "Average" }
                        div { class: "value", "{avg.round()}%" }
                    }
                    div { class: "kpi indigo",
                        div { class: "tiny", "Best" }
                        div { class: "value", "{best.round()}%" }
                    }
                    div { class: "kpi blue",
                        div { class: "tiny", "Mastered" }
                        div { class: "value", "{mastered}" }
                    }
                }
                div { class: "card stack",
                    div { class: "tiny", "Accuracy" }
                    svg {
                        class: "sparkline",
                        view_box: "0 0 280 72",
                        preserve_aspect_ratio: "none",
                        polyline {
                            points: "{points}",
                            fill: "none",
                            stroke: "var(--copper)",
                            stroke_width: "2.5",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                        }
                    }
                    p { class: "muted", "{session_count} sessions on this device" }
                }
            }
        }
    }
}
