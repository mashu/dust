use cw_core::{
    AutoLevelProgress, CharSetMode, LevelAxis, SessionResult, TrainingSettings, compute_char_pool,
    compute_streak_status, newest_unlocked,
};
use dioxus::prelude::*;

use crate::audio::AUDIO_IS_SILENT;
use crate::ui::auto_level::AutoLevelCard;
use crate::ui::heatmap::{ActivityHeatmap, StreakCard};
use crate::ui::tips::TipsCarousel;
use crate::ui::widgets::{CardHead, Icon, IconName, control_id, pretty_number};

/// Where the learner is, as Home says it: in the hero, and on the tile.
#[derive(Debug, PartialEq)]
struct LevelReading {
    /// One line, for the hero: "Level 7 · digits 3".
    summary: String,
    /// The tile's caption and figure: "Levels", "7 / 3".
    label: &'static str,
    value: String,
}

/// The levels the settings are trained at — as far as the alphabets in use
/// reach, not what is stored for a longer one.
fn level_reading(settings: &TrainingSettings) -> LevelReading {
    let letters = settings.level(LevelAxis::Letters);
    let digits = settings.level(LevelAxis::Digits);
    let (label, value, summary) = match settings.curriculum.char_set_mode {
        // A contest's callers are shaped by the callsign tier.
        CharSetMode::Callsign | CharSetMode::Contest => {
            let tier = settings.level(LevelAxis::Callsign);
            (
                "Callsign tier",
                tier.to_string(),
                format!("Callsign tier {tier}"),
            )
        }
        CharSetMode::Digits => (
            "Digits level",
            digits.to_string(),
            format!("Digits level {digits}"),
        ),
        CharSetMode::Mixed => (
            "Levels",
            format!("{letters} / {digits}"),
            format!("Level {letters} · digits {digits}"),
        ),
        CharSetMode::Koch => ("Level", letters.to_string(), format!("Level {letters}")),
        // A word list has no level: what it has is words.
        CharSetMode::Words => {
            let count = settings.words().len();
            let whose = if settings.has_own_words() {
                "Your words"
            } else {
                "Starter words"
            };
            ("Words", count.to_string(), format!("{whose} · {count}"))
        }
    };
    LevelReading {
        summary,
        label,
        value,
    }
}

/// A range as it reads: one number when its ends meet.
pub(crate) fn range_label(min: f64, max: f64) -> String {
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
    let level = level_reading(&settings);
    let acc = last_accuracy
        .map(|a| format!("{}%", (a * 100.0).round()))
        .unwrap_or_else(|| "—".into());
    let dates: Vec<String> = sessions.iter().map(|s| s.date.clone()).collect();
    let streak = compute_streak_status(&dates, &today);

    let chars = compute_char_pool(&settings);
    let newest = newest_unlocked(&settings);
    let callsigns = settings.curriculum.char_set_mode.sends_callsigns();
    let words = settings.curriculum.char_set_mode == CharSetMode::Words;
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
    rsx! {
        div { class: "stack",
            header { class: "page-head",
                h2 { class: "page-title", "Practice" }
                    p { class: "page-sub", "Hear a group and type it back — or just listen." }
            }
            if AUDIO_IS_SILENT {
                div { class: "notice",
                    Icon { name: IconName::Volume }
                    div {
                        div { style: "font-weight: 700;", "This build has no audio" }
                        p { class: "muted", style: "margin: 0.15rem 0 0;",
                            "It is the one for bringing up a new phone toolchain. Sessions still run and score at the right pace; they just play nothing."
                        }
                    }
                }
            }
            section { class: "hero",
                div { class: "hero-top",
                    span { class: "eyebrow", "Current alphabet" }
                    span { class: "hero-level",
                        Icon { name: IconName::Flag }
                        "{level.summary}"
                    }
                }
                if chars.is_empty() {
                    p { class: "muted", style: "color: var(--hero-ink-soft);", "No characters unlocked yet." }
                } else {
                    div { class: "pool-chips",
                        for ch in chars.iter().copied() {
                            if Some(ch) == newest {
                                span { class: "pool-chip fresh", title: "Newest character", "{ch}" }
                            } else {
                                span { class: "pool-chip", "{ch}" }
                            }
                        }
                    }
                }
                div { class: "hero-specs",
                    div {
                        span { class: "spec-value", "{speed}" }
                        span { class: "spec-label", "WPM" }
                    }
                    // A callsign is as long as it is: there is no size to give.
                    if callsigns || words {
                        div {
                            span { class: "spec-value", "{settings.curriculum.num_groups}" }
                            span { class: "spec-label", if callsigns { "Calls" } else { "Words" } }
                        }
                    } else {
                        div {
                            span { class: "spec-value", "{settings.curriculum.num_groups} × {size}" }
                            span { class: "spec-label", "Groups" }
                        }
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
                        Icon { name: IconName::Play }
                        "Start training"
                    }
                    button {
                        id: control_id("btn", "listen to letters"),
                        class: "btn btn-hero",
                        onclick: move |_| on_listen.call(()),
                        Icon { name: IconName::Headphones }
                        "Listen"
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
                        div { class: "tiny", "{level.label}" }
                        div { class: "value", "{level.value}" }
                    }
                }
            } else {
                div { class: "card",
                    CardHead {
                        icon: IconName::Sparkle,
                        title: "First session",
                        note: format!("{pool_len} characters are in the pool right now."),
                    }
                    p { class: "muted", style: "margin: 0;",
                        if words {
                            "Type the word you hear — the trainer scores every character of it."
                        } else {
                            "Type what you hear — the trainer scores every character, then moves the level for you."
                        }
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
    use super::{LevelReading, level_reading, range_label};
    use cw_core::{CharSetMode, TrainingSettings};

    #[test]
    fn the_level_reading_follows_the_mode() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 7;
        settings.curriculum.digits_level = 3;
        settings.curriculum.callsign_level = 2;
        let reading = |settings: &TrainingSettings, mode| {
            let mut settings = settings.clone();
            settings.curriculum.char_set_mode = mode;
            level_reading(&settings)
        };
        assert_eq!(reading(&settings, CharSetMode::Koch).summary, "Level 7");
        assert_eq!(
            reading(&settings, CharSetMode::Digits).summary,
            "Digits level 3"
        );
        assert_eq!(
            reading(&settings, CharSetMode::Mixed),
            LevelReading {
                summary: "Level 7 · digits 3".to_string(),
                label: "Levels",
                value: "7 / 3".to_string(),
            }
        );
        assert_eq!(
            reading(&settings, CharSetMode::Callsign).summary,
            "Callsign tier 2"
        );
        assert_eq!(
            reading(&settings, CharSetMode::Contest),
            reading(&settings, CharSetMode::Callsign)
        );
    }

    /// A level stored for a longer alphabet is kept for it, but not shown as
    /// the level of a shorter one: Mixed has only its letters to climb.
    #[test]
    fn the_level_shown_is_the_one_the_alphabet_in_use_reaches() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 38;
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        let mixed = level_reading(&settings);
        assert_ne!(mixed.summary, "Level 38 · digits 1", "{mixed:?}");
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        assert_eq!(level_reading(&settings).summary, "Level 38");
    }

    #[test]
    fn a_fixed_value_reads_as_one_number() {
        assert_eq!(range_label(18.0, 18.0), "18");
        assert_eq!(range_label(18.0, 25.0), "18–25");
        assert_eq!(range_label(0.7, 1.0), "0.7–1");
    }
}
