use cw_core::{
    compute_char_pool, compute_streak_status, AutoLevelProgress, SessionResult, TrainingSettings,
};
use dioxus::prelude::*;

use crate::audio::AUDIO_IS_SILENT;
use crate::ui::auto_level::AutoLevelCard;
use crate::ui::heatmap::{ActivityHeatmap, StreakCard};
use crate::ui::listen::newest_index;
use crate::ui::tips::TipsCarousel;
use crate::ui::widgets::{control_id, pretty_number, Icon};

fn level_summary(settings: &TrainingSettings) -> String {
    match settings.curriculum.char_set_mode {
        cw_core::CharSetMode::Digits => {
            format!("Digits level {}", settings.curriculum.digits_level)
        }
        cw_core::CharSetMode::Mixed => format!(
            "Level {} · digits {}",
            settings.curriculum.level, settings.curriculum.digits_level
        ),
        _ => format!("Level {}", settings.curriculum.level),
    }
}

fn range_label(min: f64, max: f64) -> String {
    if (min - max).abs() < f64::EPSILON {
        pretty_number(min)
    } else {
        format!("{}–{}", pretty_number(min), pretty_number(max))
    }
}

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

    let chars = compute_char_pool(&settings);
    let newest = newest_index(&settings, &chars);
    let speed = range_label(
        settings.playback.char_wpm_min,
        settings.playback.char_wpm_max,
    );
    let size = range_label(
        settings.curriculum.min_group_size as f64,
        settings.curriculum.max_group_size as f64,
    );
    let repeats = range_label(
        settings.playback.group_repeat_min as f64,
        settings.playback.group_repeat_max as f64,
    );
    let pool_len = pool.chars().count();
    // Precomputed so the markup keeps only plain interpolation.
    let bars: Vec<(u32, String)> = (0..22)
        .map(|i| {
            let width = if i % 3 == 0 { 15 } else { 6 };
            (width, format!("{:.2}s", f64::from(i) * 0.09))
        })
        .collect();

    rsx! {
        div { class: "stack",
            header { class: "page-head",
                h2 { class: "page-title", "Practice" }
                p { class: "page-sub", "Hear the group first, then answer from memory." }
            }
            if AUDIO_IS_SILENT {
                div { class: "notice",
                    Icon { name: "volume" }
                    div {
                        div { style: "font-weight: 700;", "This build has no audio yet" }
                        p { class: "muted", style: "margin: 0.15rem 0 0;",
                            "Sessions still run and score at the right pace, they just play nothing. Android audio is the next piece of work."
                        }
                    }
                }
            }
            section { class: "hero",
                div { class: "hero-top",
                    span { class: "eyebrow", "Current alphabet" }
                    span { class: "hero-level",
                        Icon { name: "flag" }
                        "{level_summary(&settings)}"
                    }
                }
                if chars.is_empty() {
                    p { class: "muted", style: "color: rgb(248 241 227 / 70%);", "No characters unlocked yet." }
                } else {
                    div { class: "pool-chips",
                        for (i, ch) in chars.iter().copied().enumerate() {
                            span {
                                class: if i == newest { "pool-chip fresh" } else { "pool-chip" },
                                title: if i == newest { "Newest character" } else { "" },
                                "{ch}"
                            }
                        }
                    }
                }
                div { class: "hero-specs",
                    div {
                        span { class: "spec-value", "{speed}" }
                        span { class: "spec-label", "WPM" }
                    }
                    div {
                        span { class: "spec-value", "{settings.curriculum.num_groups} × {size}" }
                        span { class: "spec-label", "Groups" }
                    }
                    div {
                        span { class: "spec-value", "{repeats}×" }
                        span { class: "spec-label", "Sent" }
                    }
                }
                div { class: "hero-actions",
                    button {
                        id: control_id("btn", "start training"),
                        class: "btn btn-primary",
                        onclick: move |_| on_start.call(()),
                        Icon { name: "play" }
                        "Start training"
                    }
                    button {
                        id: control_id("btn", "listen to letters"),
                        class: "btn btn-hero",
                        onclick: move |_| on_listen.call(()),
                        Icon { name: "headphones" }
                        "Listen to letters"
                    }
                }
                div { class: "hero-wave", "aria-hidden": "true",
                    for (width, delay) in bars.iter() {
                        i { style: "width: {width}px; animation-delay: {delay};" }
                    }
                }
            }
            if session_count > 0 {
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
                div { class: "card",
                    div { class: "card-head",
                        div { class: "card-head-main",
                            span { class: "card-icon", Icon { name: "sparkle" } }
                            div {
                                h3 { class: "card-title", "First session" }
                                p { class: "card-note", "{pool_len} characters are in the pool right now." }
                            }
                        }
                    }
                    p { class: "muted", style: "margin: 0;",
                        "Type what you hear — the trainer scores every character, then moves the level for you."
                    }
                }
            }
            StreakCard { status: streak }
            if !sessions.is_empty() {
                ActivityHeatmap { sessions, today }
            }
            if let Some(progress) = auto_progress {
                AutoLevelCard { progress }
            }
            TipsCarousel {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{level_summary, range_label};
    use cw_core::{CharSetMode, TrainingSettings};

    #[test]
    fn the_level_summary_follows_the_mode() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 7;
        settings.curriculum.digits_level = 3;

        settings.curriculum.char_set_mode = CharSetMode::Koch;
        assert_eq!(level_summary(&settings), "Level 7");
        settings.curriculum.char_set_mode = CharSetMode::Digits;
        assert_eq!(level_summary(&settings), "Digits level 3");
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        assert_eq!(level_summary(&settings), "Level 7 · digits 3");
    }

    #[test]
    fn a_fixed_value_reads_as_one_number() {
        assert_eq!(range_label(18.0, 18.0), "18");
        assert_eq!(range_label(18.0, 25.0), "18–25");
        assert_eq!(range_label(0.7, 1.0), "0.7–1");
    }
}
