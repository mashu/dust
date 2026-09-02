use cw_core::{HeatmapColorMode, HeatmapGrid, SessionResult, StreakState, StreakStatus};
use dioxus::prelude::*;

#[component]
pub fn StreakCard(status: StreakStatus) -> Element {
    if status.state == StreakState::None {
        return rsx! {};
    }
    let freeze = if status.freezes_available > 0 {
        Some(status.freezes_available)
    } else {
        None
    };
    let (class, emoji, body) = match status.state {
        StreakState::Safe => {
            let used = if status.freezes_used > 0 {
                let noun = if status.freezes_used == 1 {
                    "freeze"
                } else {
                    "freezes"
                };
                format!(" ({} {noun} used)", status.freezes_used)
            } else {
                String::new()
            };
            (
                "streak safe",
                "🔥",
                format!("{}-day streak — today is in the bag.{used}", status.days),
            )
        }
        StreakState::AtRisk => (
            "streak risk",
            "⚠️",
            format!(
                "{}-day streak at risk — practice today to keep it alive.",
                status.days
            ),
        ),
        StreakState::Lost => (
            "streak lost",
            "💔",
            format!(
                "Your {}-day streak ended — one session today starts the next one.",
                status.lost_streak_days.unwrap_or(0)
            ),
        ),
        StreakState::None => unreachable!(),
    };
    rsx! {
        div { class: class,
            span { class: "streak-emoji", "{emoji}" }
            p { "{body}" }
            if let Some(count) = freeze {
                span { class: "freeze-chip", title: "A freeze covers one missed day. Earn one every 7 practiced days.", "🧊 ×{count}" }
            }
        }
    }
}

#[component]
pub fn ActivityHeatmap(sessions: Vec<SessionResult>, today: String) -> Element {
    let mut mode = use_signal(|| HeatmapColorMode::Volume);
    let mut selected = use_signal(|| None::<String>);
    let Some(grid) = cw_core::build_heatmap(&sessions, &today, cw_core::HEATMAP_WEEKS, mode()) else {
        return rsx! {};
    };
    let legend = if mode() == HeatmapColorMode::Volume {
        "More chars"
    } else {
        "Better copy"
    };
    let selected_cell = selected().and_then(|date| {
        grid.cells.iter().find(|c| c.date == date).cloned()
    });
    let selected_summary = selected_cell.as_ref().map(|cell| {
        if cell.sessions == 0 {
            format!("{} · no practice", cell.date)
        } else {
            let noun = if cell.sessions == 1 { "session" } else { "sessions" };
            let mut text = format!(
                "{} · {} {noun} · {} chars",
                cell.date, cell.sessions, cell.chars
            );
            if let Some(acc) = cell.avg_accuracy {
                text.push_str(&format!(" · {:.0}% accuracy", acc * 100.0));
            }
            text
        }
    });
    rsx! {
        div { class: "card stack heatmap-card",
            div { class: "row", style: "justify-content: space-between;",
                div { class: "tiny", "Practice calendar" }
                div { class: "mode-pills",
                    button {
                        class: if mode() == HeatmapColorMode::Volume { "pill active" } else { "pill" },
                        onclick: move |_| mode.set(HeatmapColorMode::Volume),
                        "Volume"
                    }
                    button {
                        class: if mode() == HeatmapColorMode::Accuracy { "pill active" } else { "pill" },
                        onclick: move |_| mode.set(HeatmapColorMode::Accuracy),
                        "Accuracy"
                    }
                }
            }
            HeatmapGridView { grid: grid.clone(), selected: selected(), on_select: move |date| selected.set(Some(date)) }
            div { class: "heatmap-legend",
                span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "Less" }
                span { class: "heat-cell", style: "background: #e5e7eb;" }
                span { class: "heat-cell", style: "background: hsl(0, 75%, 45%);" }
                span { class: "heat-cell", style: "background: hsl(60, 75%, 45%);" }
                span { class: "heat-cell", style: "background: hsl(120, 75%, 45%);" }
                span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "{legend}" }
            }
            if let Some(summary) = selected_summary {
                p { class: "muted", "{summary}" }
            } else {
                p { class: "muted", "Tap a day for details." }
            }
        }
    }
}

#[component]
fn HeatmapGridView(
    grid: HeatmapGrid,
    selected: Option<String>,
    on_select: EventHandler<String>,
) -> Element {
    let labels = ["Mon", "", "Wed", "", "Fri", "", ""];
    rsx! {
        div { class: "heatmap-wrap",
            div { class: "heatmap-days",
                for label in labels {
                    span { class: "heatmap-day-label", "{label}" }
                }
            }
            div { class: "heatmap-grid",
                for cell in grid.cells.iter() {
                    {
                        let date = cell.date.clone();
                        let is_selected = selected.as_deref() == Some(date.as_str());
                        let cls = if is_selected { "heat-cell selected" } else { "heat-cell" };
                        let title = if cell.sessions == 0 {
                            format!("{} — no practice", cell.date)
                        } else {
                            format!("{} — {} chars, {} sessions", cell.date, cell.chars, cell.sessions)
                        };
                        rsx! {
                            button {
                                class: cls,
                                style: "background: {cell.color};",
                                title: "{title}",
                                disabled: cell.in_future,
                                onclick: move |_| on_select.call(date.clone()),
                            }
                        }
                    }
                }
            }
        }
    }
}
