use cw_core::{
    accuracy_chart, character_diagnostics, AccuracyPoint, MasteryStatus, SessionResult,
    TrainingSettings,
};
use dioxus::prelude::*;

use crate::ui::stats_detail::{HistoryTab, LettersTab, MistakesTab, SamplingTab};
use crate::ui::widgets::{Icon, Seg};

// 420×96 keeps the drawing box at the same aspect the card gives it, so the
// SVG can scale uniformly: stretched coordinates would turn the dots into
// ellipses and thin the stroke unevenly.
const CHART_W: f64 = 420.0;
const CHART_H: f64 = 96.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatsTab {
    Overview,
    Letters,
    Mistakes,
    Sampling,
    History,
}

/// Chart shape for the accuracy series. `None` when there is nothing to draw.
struct ChartGeometry {
    line: String,
    area: String,
    dots: Vec<(f64, f64)>,
    floor_pct: f64,
    threshold_y: Option<f64>,
}

/// Accuracy clusters near the top, so the axis starts below the worst session
/// instead of at zero — otherwise every series is a flat line in the top tenth.
fn chart_floor(points: &[AccuracyPoint]) -> f64 {
    let min = points
        .iter()
        .map(|p| p.accuracy_pct.clamp(0.0, 100.0))
        .fold(100.0_f64, f64::min);
    (((min - 6.0) / 10.0).floor() * 10.0).clamp(0.0, 80.0)
}

fn chart_geometry(points: &[AccuracyPoint], threshold_pct: f64) -> Option<ChartGeometry> {
    if points.is_empty() {
        return None;
    }
    let floor_pct = chart_floor(points);
    let span = (100.0 - floor_pct).max(1.0);
    let y_at = |pct: f64| {
        let pct = pct.clamp(0.0, 100.0);
        (CHART_H - ((pct - floor_pct) / span) * CHART_H).clamp(0.0, CHART_H)
    };

    let coords: Vec<(f64, f64)> = if points.len() == 1 {
        // One session: a flat line across the card with a dot on it beats an
        // empty chart, which is what a single-point polyline renders as.
        let y = y_at(points[0].accuracy_pct);
        vec![(0.0, y), (CHART_W, y)]
    } else {
        let last = (points.len() - 1) as f64;
        points
            .iter()
            .enumerate()
            .map(|(i, p)| (i as f64 / last * CHART_W, y_at(p.accuracy_pct)))
            .collect()
    };

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

    let dots = if points.len() == 1 {
        vec![(CHART_W / 2.0, coords[0].1)]
    } else if points.len() <= 30 {
        coords.clone()
    } else {
        coords.last().copied().into_iter().collect()
    };

    Some(ChartGeometry {
        line,
        area,
        dots,
        floor_pct,
        threshold_y: (threshold_pct > floor_pct && threshold_pct < 100.0)
            .then(|| y_at(threshold_pct)),
    })
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
                    OverviewTab {
                        sessions: matching,
                        threshold: settings.auto_level.auto_adjust_threshold,
                    }
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
fn OverviewTab(sessions: Vec<SessionResult>, threshold: f64) -> Element {
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
    let geometry = chart_geometry(&chart, threshold);
    let session_count = sessions.len();
    let session_noun = if session_count == 1 {
        "session"
    } else {
        "sessions"
    };
    let latest = chart.last().map(|p| p.accuracy_pct).unwrap_or(0.0);
    rsx! {
        div { class: "stack",
            if sessions.is_empty() {
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
                if let Some(geometry) = geometry {
                    div { class: "card chart-card",
                        div { class: "card-head",
                            div { class: "card-head-main",
                                span { class: "card-icon", Icon { name: "chart" } }
                                div {
                                    h3 { class: "card-title", "Accuracy over time" }
                                    p { class: "card-note", "{session_count} {session_noun} · latest {latest.round()}%" }
                                }
                            }
                        }
                        svg {
                            class: "sparkline",
                            view_box: "0 0 420 96",
                            defs {
                                linearGradient { id: "dust-acc-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                                    stop { offset: "0%", style: "stop-color: var(--copper); stop-opacity: 0.42;" }
                                    stop { offset: "100%", style: "stop-color: var(--copper); stop-opacity: 0.02;" }
                                }
                            }
                            if let Some(y) = geometry.threshold_y {
                                line {
                                    x1: "0",
                                    y1: "{y:.1}",
                                    x2: "420",
                                    y2: "{y:.1}",
                                    stroke: "var(--good)",
                                    stroke_width: "1",
                                    stroke_dasharray: "4 4",
                                    opacity: "0.65",
                                }
                            }
                            path { d: "{geometry.area}", fill: "url(#dust-acc-fill)", stroke: "none" }
                            polyline {
                                points: "{geometry.line}",
                                fill: "none",
                                stroke: "var(--copper)",
                                stroke_width: "2.2",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                            }
                            for (x, y) in geometry.dots.iter() {
                                circle {
                                    cx: "{x:.1}",
                                    cy: "{y:.1}",
                                    r: "3",
                                    fill: "var(--copper)",
                                    stroke: "var(--surface)",
                                    stroke_width: "1.5",
                                }
                            }
                        }
                        div { class: "row-between",
                            span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                                "Scale {geometry.floor_pct}–100%"
                            }
                            span { class: "chip good", "Level up at {threshold.round()}%" }
                        }
                    }
                }
            }
        }
    }
}
