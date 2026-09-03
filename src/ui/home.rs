use cw_core::{compute_streak_status, AutoLevelProgress, SessionResult, TrainingSettings};
use dioxus::prelude::*;

use crate::ui::auto_level::AutoLevelCard;
use crate::ui::heatmap::{ActivityHeatmap, StreakCard};
use crate::ui::tips::TipsCarousel;

#[component]
pub fn Home(
    settings: TrainingSettings,
    last_accuracy: Option<f64>,
    session_count: usize,
    pool: String,
    sessions: Vec<SessionResult>,
    today: String,
    auto_progress: Option<AutoLevelProgress>,
    on_start: EventHandler<()>,
    on_listen: EventHandler<()>,
) -> Element {
    let level_label = match settings.curriculum.char_set_mode {
        cw_core::CharSetMode::Digits => "Digits level",
        cw_core::CharSetMode::Mixed => "Levels",
        _ => "Level",
    };
    let level_value = match settings.curriculum.char_set_mode {
        cw_core::CharSetMode::Digits => format!("{}", settings.curriculum.digits_level),
        cw_core::CharSetMode::Mixed => {
            format!(
                "{} / {}",
                settings.curriculum.level, settings.curriculum.digits_level
            )
        }
        _ => format!("{}", settings.curriculum.level),
    };
    let acc = last_accuracy
        .map(|a| format!("{}%", (a * 100.0).round()))
        .unwrap_or_else(|| "—".into());
    let dates: Vec<String> = sessions.iter().map(|s| s.date.clone()).collect();
    let streak = compute_streak_status(&dates, &today);
    rsx! {
        div { class: "stack",
            header {
                h2 { class: "page-title", style: "margin: 0 0 0.25rem;", "Practice" }
                p { class: "muted", "Hear the group first, then answer from memory." }
            }
            div { class: "card home-start",
                div { class: "tiny", "Current alphabet" }
                p { class: "pool", "{pool}" }
                p { class: "muted", style: "margin: 0.4rem 0 0.85rem;",
                    "{settings.playback.char_wpm_min as u32}–{settings.playback.char_wpm_max as u32} WPM · {settings.curriculum.min_group_size}–{settings.curriculum.max_group_size} chars · {settings.curriculum.num_groups} groups"
                }
                div { class: "home-actions",
                    button { class: "btn btn-primary", onclick: move |_| on_start.call(()), "Start training" }
                    button { class: "btn btn-secondary", onclick: move |_| on_listen.call(()), "Listen to letters" }
                }
            }
            if session_count > 0 {
                ActivityHeatmap { sessions, today }
                StreakCard { status: streak }
                div { class: "grid-3",
                    div { class: "kpi emerald",
                        div { class: "tiny", "Last accuracy" }
                        div { class: "value", "{acc}" }
                    }
                    div { class: "kpi indigo",
                        div { class: "tiny", "Sessions" }
                        div { class: "value", "{session_count}" }
                    }
                    div { class: "kpi blue",
                        div { class: "tiny", "{level_label}" }
                        div { class: "value", "{level_value}" }
                    }
                }
            } else {
                StreakCard { status: streak }
            }
            if let Some(progress) = auto_progress {
                AutoLevelCard { progress }
            }
            TipsCarousel {}
        }
    }
}
