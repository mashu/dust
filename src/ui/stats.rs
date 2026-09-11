use cw_core::{
    accuracy_chart, character_diagnostics, AccuracyPoint, MasteryStatus, SessionResult,
    TrainingSettings,
};
use dioxus::prelude::*;

use crate::ui::stats_detail::{HistoryTab, LettersTab, MistakesTab, SamplingTab};
use crate::ui::widgets::{Icon, Seg};

const CHART_W: f64 = 300.0;
const CHART_H: f64 = 96.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatsTab {
    Overview,
    Letters,
    Mistakes,
    Sampling,
    History,
}

fn chart_geometry(points: &[AccuracyPoint]) -> (String, String) {
    if points.is_empty() {
        return (String::new(), String::new());
    }
    let last = (points.len() - 1).max(1) as f64;
    let coords: Vec<(f64, f64)> = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let x = i as f64 / last * CHART_W;
            let y = CHART_H - (p.accuracy_pct.clamp(0.0, 100.0) / 100.0) * CHART_H;
            (x, y)
        })
        .collect();
    let line = coords
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut area = format!("M{:.1},{CHART_H:.1}", coords[0].0);
    for (x, y) in &coords {
        area.push_str(&format!(" L{x:.1},{y:.1}"));
    }
    area.push_str(&format!(
        " L{:.1},{CHART_H:.1} Z",
        coords.last().map(|(x, _)| *x).unwrap_or(0.0)
    ));
    (line, area)
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
            header { class: "page-head",
                h2 { class: "page-title", "Stats" }
                p { class: "page-sub", "Everything scored on this device for the current alphabet." }
            }
            div { class: "segmented",
                Seg { label: "Overview".to_string(), active: tab() == StatsTab::Overview, onclick: move |_| tab.set(StatsTab::Overview) }
                Seg { label: "Letters".to_string(), active: tab() == StatsTab::Letters, onclick: move |_| tab.set(StatsTab::Letters) }
                Seg { label: "Mistakes".to_string(), active: tab() == StatsTab::Mistakes, onclick: move |_| tab.set(StatsTab::Mistakes) }
                Seg { label: "Sampling".to_string(), active: tab() == StatsTab::Sampling, onclick: move |_| tab.set(StatsTab::Sampling) }
                Seg { label: "History".to_string(), active: tab() == StatsTab::History, onclick: move |_| tab.set(StatsTab::History) }
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
    let (line, area) = chart_geometry(&chart);
    let empty = sessions.is_empty();
    let session_count = sessions.len();
    let latest = chart.last().map(|p| p.accuracy_pct).unwrap_or(0.0);
    rsx! {
        div { class: "stack",
            if empty {
                div { class: "card",
                    div { class: "card-head",
                        div { class: "card-head-main",
                            span { class: "card-icon", Icon { name: "chart" } }
                            div { h3 { class: "card-title", "Nothing scored yet" } }
                        }
                    }
                    p { class: "muted", style: "margin: 0;",
                        "Finish a session to unlock accuracy over time, letter mastery and sampling weights."
                    }
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
                div { class: "card chart-card",
                    div { class: "card-head",
                        div { class: "card-head-main",
                            span { class: "card-icon", Icon { name: "chart" } }
                            div {
                                h3 { class: "card-title", "Accuracy over time" }
                                p { class: "card-note", "{session_count} sessions · latest {latest.round()}%" }
                            }
                        }
                    }
                    svg {
                        class: "sparkline",
                        view_box: "0 0 300 96",
                        preserve_aspect_ratio: "none",
                        defs {
                            linearGradient { id: "dust-acc-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                                stop { offset: "0%", style: "stop-color: var(--copper); stop-opacity: 0.42;" }
                                stop { offset: "100%", style: "stop-color: var(--copper); stop-opacity: 0.02;" }
                            }
                        }
                        line { x1: "0", y1: "9.6", x2: "300", y2: "9.6", stroke: "var(--line-soft)", stroke_width: "1", stroke_dasharray: "3 5" }
                        line { x1: "0", y1: "48", x2: "300", y2: "48", stroke: "var(--line-soft)", stroke_width: "1", stroke_dasharray: "3 5" }
                        path { d: "{area}", fill: "url(#dust-acc-fill)", stroke: "none" }
                        polyline {
                            points: "{line}",
                            fill: "none",
                            stroke: "var(--copper)",
                            stroke_width: "2.2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                        }
                    }
                    div { class: "row-between",
                        span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;", "90% line is the level-up threshold" }
                        span { class: "chip neutral", "100% top" }
                    }
                }
            }
        }
    }
}
