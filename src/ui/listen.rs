use cw_core::{compute_char_pool, morse_for, MixedAutoLevelAxis, TrainingSettings};
use dioxus::prelude::*;

use crate::ui::widgets::Icon;

pub fn newest_index(settings: &TrainingSettings, pool: &[char]) -> usize {
    if pool.is_empty() {
        return 0;
    }
    let added_from = |previous: Vec<char>| pool.iter().rposition(|c| !previous.contains(c));
    match settings.curriculum.char_set_mode {
        cw_core::CharSetMode::Digits => {
            let mut prev = settings.clone();
            prev.curriculum.digits_level = prev.curriculum.digits_level.saturating_sub(1).max(1);
            added_from(compute_char_pool(&prev)).unwrap_or(pool.len() - 1)
        }
        cw_core::CharSetMode::Mixed => {
            let pct = settings.curriculum.mixed_letters_percent.min(100);
            let try_letters = || {
                let mut prev = settings.clone();
                prev.curriculum.level = prev.curriculum.level.saturating_sub(1).max(1);
                added_from(compute_char_pool(&prev))
            };
            let try_digits = || {
                let mut prev = settings.clone();
                prev.curriculum.digits_level =
                    prev.curriculum.digits_level.saturating_sub(1).max(1);
                added_from(compute_char_pool(&prev))
            };
            let letter_hit = (pct > 0).then(try_letters).flatten();
            let digit_hit = (pct < 100).then(try_digits).flatten();
            match settings.auto_level.mixed_auto_level_next_axis.flip() {
                MixedAutoLevelAxis::Letters => letter_hit.or(digit_hit),
                MixedAutoLevelAxis::Digits => digit_hit.or(letter_hit),
            }
            .unwrap_or(pool.len() - 1)
        }
        _ => {
            let mut prev = settings.clone();
            prev.curriculum.level = prev.curriculum.level.saturating_sub(1).max(1);
            added_from(compute_char_pool(&prev)).unwrap_or(pool.len() - 1)
        }
    }
}

/// Dots and dashes drawn as keyed elements rather than punctuation.
#[component]
fn MorseBars(pattern: String) -> Element {
    rsx! {
        div { class: "listen-morse", "aria-hidden": "true",
            for symbol in pattern.chars() {
                i { class: if symbol == '.' { "dit" } else { "dah" } }
            }
        }
    }
}

#[component]
pub fn ListenView(
    settings: TrainingSettings,
    playing: bool,
    on_play: EventHandler<String>,
    on_stop: EventHandler<()>,
    on_back: EventHandler<()>,
) -> Element {
    let pool = compute_char_pool(&settings);
    let default_idx = newest_index(&settings, &pool);
    let mut selected = use_signal(|| None::<usize>);
    let idx = selected()
        .unwrap_or(default_idx)
        .min(pool.len().saturating_sub(1));
    let current = pool.get(idx).copied();
    let pattern = current.and_then(morse_for).unwrap_or("");
    let spoken: String = pattern
        .chars()
        .map(|c| if c == '.' { "di " } else { "dah " })
        .collect::<String>()
        .trim_end()
        .to_string();
    let all_chars: String = pool.iter().collect();
    rsx! {
        div { class: "stack listen-page",
            header { class: "row-between",
                div {
                    h2 { class: "page-title", "Listen" }
                    p { class: "page-sub", "Play one character, or the whole unlocked pool." }
                }
                button { class: "btn btn-secondary btn-sm", onclick: move |_| on_back.call(()),
                    Icon { name: "back" }
                    "Back"
                }
            }
            div { class: "chip-strip",
                for (i, ch) in pool.iter().copied().enumerate() {
                    {
                        let newest = i == default_idx;
                        let active = i == idx;
                        let class = if active {
                            "letter-chip active"
                        } else if newest {
                            "letter-chip newest"
                        } else {
                            "letter-chip"
                        };
                        rsx! {
                            button {
                                class: class,
                                onclick: move |_| selected.set(Some(i)),
                                "{ch}"
                            }
                        }
                    }
                }
            }
            div { class: "card listen-stage",
                if let Some(ch) = current {
                    div { class: "listen-glyph", "{ch}" }
                    MorseBars { pattern: pattern.to_string() }
                    p { class: "mono", style: "margin: 0; letter-spacing: 0.16em; color: var(--copper-deep); font-size: 0.9rem;",
                        "{spoken}"
                    }
                    p { class: "muted", style: "margin: 0.5rem 0 0;",
                        if default_idx == idx { "Newest unlocked character" } else { "From your current pool" }
                    }
                } else {
                    p { class: "muted", "No characters in the pool." }
                }
            }
            div { class: "hero-actions",
                if playing {
                    button { class: "btn btn-secondary", onclick: move |_| on_stop.call(()),
                        Icon { name: "stop" }
                        "Stop"
                    }
                } else {
                    if let Some(ch) = current {
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| on_play.call(ch.to_string()),
                            Icon { name: "play" }
                            "Play {ch}"
                        }
                    }
                    button {
                        class: "btn btn-secondary",
                        disabled: all_chars.is_empty(),
                        onclick: move |_| on_play.call(all_chars.clone()),
                        Icon { name: "headphones" }
                        "Play all"
                    }
                }
            }
        }
    }
}
