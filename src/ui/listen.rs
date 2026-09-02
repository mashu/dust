use cw_core::{compute_char_pool, morse_for, MixedAutoLevelAxis, TrainingSettings};
use dioxus::prelude::*;

fn pretty_morse(pattern: &str) -> String {
    pattern
        .chars()
        .map(|c| match c {
            '.' => '·',
            '-' => '−',
            _ => c,
        })
        .collect()
}

fn newest_index(settings: &TrainingSettings, pool: &[char]) -> usize {
    if pool.is_empty() {
        return 0;
    }
    let added_from = |previous: Vec<char>| {
        pool.iter()
            .rposition(|c| !previous.contains(c))
    };
    match settings.char_set_mode {
        cw_core::CharSetMode::Digits => {
            let mut prev = settings.clone();
            prev.digits_level = prev.digits_level.saturating_sub(1).max(1);
            added_from(compute_char_pool(&prev)).unwrap_or(pool.len() - 1)
        }
        cw_core::CharSetMode::Mixed => {
            let pct = settings.mixed_letters_percent.min(100);
            let try_letters = || {
                let mut prev = settings.clone();
                prev.level = prev.level.saturating_sub(1).max(1);
                added_from(compute_char_pool(&prev))
            };
            let try_digits = || {
                let mut prev = settings.clone();
                prev.digits_level = prev.digits_level.saturating_sub(1).max(1);
                added_from(compute_char_pool(&prev))
            };
            let letter_hit = (pct > 0).then(try_letters).flatten();
            let digit_hit = (pct < 100).then(try_digits).flatten();
            match settings.mixed_auto_level_next_axis.flip() {
                MixedAutoLevelAxis::Letters => letter_hit.or(digit_hit),
                MixedAutoLevelAxis::Digits => digit_hit.or(letter_hit),
            }
            .unwrap_or(pool.len() - 1)
        }
        _ => {
            let mut prev = settings.clone();
            prev.level = prev.level.saturating_sub(1).max(1);
            added_from(compute_char_pool(&prev)).unwrap_or(pool.len() - 1)
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
    let all_chars: String = pool.iter().collect();
    rsx! {
        div { class: "stack listen-page",
            div { class: "row", style: "justify-content: space-between;",
                h2 { class: "page-title", style: "margin: 0;", "Listen to letters" }
                button { class: "btn btn-ghost", onclick: move |_| on_back.call(()), "Back" }
            }
            p { class: "muted", "Newest unlocked character is selected. Play one, or the whole alphabet." }
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
                    div { class: "listen-morse mono", "{pretty_morse(pattern)}" }
                    p { class: "muted", if default_idx == idx { "Newly unlocked" } else { "From your current pool" } }
                } else {
                    p { class: "muted", "No characters in the pool." }
                }
            }
            div { class: "home-actions",
                if playing {
                    button { class: "btn btn-secondary", onclick: move |_| on_stop.call(()), "Stop" }
                } else {
                    if let Some(ch) = current {
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| on_play.call(ch.to_string()),
                            "Play {ch}"
                        }
                    }
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| on_play.call(all_chars.clone()),
                        "Play all"
                    }
                }
            }
        }
    }
}
