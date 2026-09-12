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
    // Accuracy, letters and mistakes are counted over the whole history: a
    // letter is the same letter whichever character set it was sent under.
    // Only the sampling snapshot narrows to the current set, and it does that
    // itself so it keeps matching what the trainer will actually draw.
    let letters = character_diagnostics(&sessions);
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
                        sessions: sessions.clone(),
                        threshold: settings.auto_level.auto_adjust_threshold,
                    }
                },
                StatsTab::Letters => rsx! { LettersTab { letters } },
                StatsTab::Mistakes => rsx! { MistakesTab { sessions: sessions.clone() } },
                StatsTab::Sampling => rsx! { SamplingTab { settings, sessions: sessions.clone() } },
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

#[cfg(test)]
mod tests {
    use super::{chart_floor, chart_geometry, CHART_H, CHART_W};
    use cw_core::AccuracyPoint;

    fn points(pcts: &[f64]) -> Vec<AccuracyPoint> {
        pcts.iter()
            .enumerate()
            .map(|(i, pct)| AccuracyPoint {
                date: format!("2026-09-{:02}", i + 1),
                accuracy_pct: *pct,
                timestamp: 1_757_000_000_000 + i as u64,
            })
            .collect()
    }

    #[test]
    fn no_sessions_means_no_chart() {
        assert!(chart_geometry(&[], 90.0).is_none());
    }

    #[test]
    fn one_session_draws_a_flat_line_with_a_point() {
        let geometry = chart_geometry(&points(&[89.0]), 90.0).unwrap();
        // Two coordinates, so the polyline actually has a segment to draw.
        assert_eq!(geometry.line.split(' ').count(), 2);
        assert!(geometry.line.starts_with("0.0,"));
        assert!(geometry.area.ends_with(" Z"));
        assert_eq!(geometry.dots.len(), 1);
        assert!((geometry.dots[0].0 - CHART_W / 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_axis_starts_below_the_worst_session() {
        assert_eq!(chart_floor(&points(&[88.0, 94.0])), 80.0);
        assert_eq!(chart_floor(&points(&[71.0, 94.0])), 60.0);
        // Never below zero, and never so high that the span disappears.
        assert_eq!(chart_floor(&points(&[3.0])), 0.0);
        assert_eq!(chart_floor(&points(&[100.0])), 80.0);
    }

    #[test]
    fn the_series_spans_the_full_width_and_height() {
        let geometry = chart_geometry(&points(&[60.0, 100.0, 80.0]), 90.0).unwrap();
        let coords: Vec<(f64, f64)> = geometry
            .line
            .split(' ')
            .map(|pair| {
                let (x, y) = pair.split_once(',').unwrap();
                (x.parse().unwrap(), y.parse().unwrap())
            })
            .collect();
        assert_eq!(coords.len(), 3);
        assert_eq!(coords[0].0, 0.0);
        assert_eq!(coords[2].0, CHART_W);
        // 100% sits on the top edge; the worst session sits above the bottom,
        // because the axis floor is the round ten below it.
        assert_eq!(coords[1].1, 0.0);
        assert_eq!(geometry.floor_pct, 50.0);
        assert!(coords[0].1 > coords[2].1, "60% must sit below 80%");
        assert!(coords.iter().all(|(_, y)| (0.0..=CHART_H).contains(y)));
        assert_eq!(geometry.dots.len(), 3);
    }

    #[test]
    fn only_the_last_point_is_marked_on_a_long_history() {
        let long: Vec<f64> = (0..40).map(|i| 60.0 + f64::from(i % 20)).collect();
        let geometry = chart_geometry(&points(&long), 90.0).unwrap();
        assert_eq!(geometry.dots.len(), 1);
    }

    #[test]
    fn the_threshold_line_hides_when_it_is_off_the_scale() {
        // Sessions in the 90s: the 90% guide sits inside the visible range.
        let inside = chart_geometry(&points(&[92.0, 97.0]), 90.0).unwrap();
        assert!(inside.threshold_y.is_some());
        // A threshold under the floor has nowhere to sit.
        let below = chart_geometry(&points(&[96.0, 99.0]), 50.0).unwrap();
        assert_eq!(below.floor_pct, 80.0);
        assert!(below.threshold_y.is_none());
        // Neither does one at the very top.
        let top = chart_geometry(&points(&[80.0]), 100.0).unwrap();
        assert!(top.threshold_y.is_none());
    }
}
