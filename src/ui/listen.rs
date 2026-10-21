use cw_core::{CharSetMode, TrainingSettings, compute_char_pool, morse_for, newest_unlocked};
use dioxus::prelude::*;

use crate::playback::{Dialogue, HeardGroup};
use crate::ui::hear_qso::{HeardDialogue, QsoHead, heard_label};
use crate::ui::widgets::{Icon, IconName, control_id, html_bool};

/// A character chip's id. Most characters slug to themselves; `?`, `/`, `.`
/// and the rest would slug to nothing and share one, so they go by their
/// code point.
fn chip_id(ch: char) -> String {
    if ch.is_ascii_alphanumeric() {
        control_id("chip", &ch.to_string())
    } else {
        format!("chip-u{:x}", u32::from(ch))
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
    #[props(default)] streaming: bool,
    #[props(default)] heard: Vec<HeardGroup>,
    #[props(default)] on_stream: EventHandler<()>,
    /// Hear a QSO is playing, offered in Contest mode.
    #[props(default)]
    hearing: bool,
    /// What Hear a QSO has played, kept once it ends.
    #[props(default)]
    dialogue: Option<Dialogue>,
    #[props(default)] on_hear: EventHandler<()>,
    /// Put away what Hear a QSO played, for a character picked since.
    #[props(default)]
    on_put_away: EventHandler<()>,
    /// The groups in a window of their own, over other programs: offered
    /// where there is one to open, the desktop.
    #[props(default)]
    can_float: bool,
    #[props(default)] floating: bool,
    #[props(default)] on_float: EventHandler<()>,
    /// Whether clicks go through the floating window to what is under it.
    #[props(default)]
    float_locked: bool,
    #[props(default)] on_float_lock: EventHandler<()>,
) -> Element {
    let pool = compute_char_pool(&settings);
    let newest = newest_unlocked(&settings);
    let default_idx = newest
        .and_then(|newest| pool.iter().position(|ch| *ch == newest))
        .unwrap_or(0);
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
    let pool_empty = all_chars.is_empty();
    let busy = playing || streaming || hearing;
    // In Contest mode, a few QSOs of the contest to hear, once it has shipped.
    let contest = (settings.curriculum.char_set_mode == CharSetMode::Contest)
        .then_some(settings.contest.contest_kind)
        .filter(|kind| kind.rules().is_some());
    // What Hear a QSO played, while it is of the contest chosen now: kept
    // once it ends, but not when it stopped before a line was heard.
    let dialogue = dialogue
        .filter(|dialogue| Some(dialogue.kind) == contest)
        .filter(|dialogue| hearing || !dialogue.lines.is_empty())
        .filter(|_| !streaming);
    // On the stage while it plays, Stop stays put above lines that come.
    let stop_on_stage = hearing && dialogue.is_some();
    rsx! {
        div { class: "stack",
            header { class: "row-between",
                div {
                    h2 { class: "page-title", "Listen" }
                    p { class: "page-sub",
                        if streaming {
                            "Groups from your current settings. Nothing is scored."
                        } else if let Some(kind) = contest {
                            "Hear a few QSOs of {kind.label()}, or play one character, the pool, or a stream of groups."
                        } else {
                            "Play one character, the pool, or a stream of groups."
                        }
                    }
                }
                button {
                    id: control_id("btn", "listen back"),
                    class: "btn btn-secondary btn-sm",
                    onclick: move |_| on_back.call(()),
                    Icon { name: IconName::Back }
                    "Back"
                }
            }
            div { class: "chip-strip",
                for (i, ch) in pool.iter().copied().enumerate() {
                    {
                        // Nothing is picked while the stage shows something
                        // else.
                        let active = i == idx && !streaming && dialogue.is_none();
                        // A character picked once QSOs heard are up puts
                        // them away, to show it instead.
                        let put_away = dialogue.is_some() && !hearing;
                        let class = if active {
                            "letter-chip active"
                        } else if Some(ch) == newest {
                            "letter-chip newest"
                        } else {
                            "letter-chip"
                        };
                        rsx! {
                            button {
                                id: chip_id(ch),
                                class: class,
                                onclick: move |_| {
                                    selected.set(Some(i));
                                    if put_away {
                                        on_put_away.call(());
                                    }
                                },
                                "{ch}"
                            }
                        }
                    }
                }
            }
            if let Some(dialogue) = dialogue {
                div { class: "card listen-stage listen-qsos",
                    QsoHead { label: format!("{} · {}", heard_label(&dialogue, hearing), dialogue.kind.label()),
                        if hearing {
                            button {
                                id: control_id("btn", "stop"),
                                class: "btn btn-secondary btn-sm",
                                onclick: move |_| on_stop.call(()),
                                Icon { name: IconName::Stop }
                                "Stop"
                            }
                        }
                    }
                    HeardDialogue { dialogue, hearing }
                }
            } else {
                div { class: "card listen-stage",
                    if streaming {
                        p { class: "listen-glyph",
                            if let Some(last) = heard.last() {
                                "{last.text}"
                            } else {
                                "·−"
                            }
                        }
                        if let Some(last) = heard.last() {
                            p { id: "listen-speed", class: "mono listen-speed", "{last.speed_label()}" }
                        }
                        p { class: "muted", style: "margin: 0.5rem 0 0;",
                            if heard.is_empty() {
                                "Listening — groups appear here after they are sent."
                            } else {
                                "Last group. Earlier ones sit underneath."
                            }
                        }
                        if !heard.is_empty() {
                            div { class: "listen-heard",
                                for (i, group) in heard.iter().enumerate() {
                                    span {
                                        class: if i + 1 == heard.len() { "listen-heard-item latest" } else { "listen-heard-item" },
                                        title: group.speed_label(),
                                        "{group.text}"
                                    }
                                }
                            }
                        }
                    } else if let Some(ch) = current {
                        div { class: "listen-glyph", "{ch}" }
                        MorseBars { pattern: pattern.to_string() }
                        p { class: "mono", style: "margin: 0; letter-spacing: 0.16em; color: var(--copper-deep); font-size: 0.9rem;",
                            "{spoken}"
                        }
                        p { class: "muted", style: "margin: 0.5rem 0 0;",
                            if current == newest { "Newest unlocked character" } else { "From your current pool" }
                        }
                    } else {
                        p { class: "muted", "No characters in the pool." }
                    }
                }
            }
            div { class: "hero-actions",
                if busy && !stop_on_stage {
                    button {
                        id: control_id("btn", "stop"),
                        class: "btn btn-secondary",
                        onclick: move |_| on_stop.call(()),
                        Icon { name: IconName::Stop }
                        "Stop"
                    }
                } else if !busy {
                    if contest.is_some() {
                        button {
                            id: control_id("btn", "hear a qso"),
                            class: "btn btn-primary",
                            onclick: move |_| on_hear.call(()),
                            Icon { name: IconName::Signal }
                            "Hear a QSO"
                        }
                    }
                    if let Some(ch) = current {
                        button {
                            id: control_id("btn", "play"),
                            class: if contest.is_some() { "btn btn-secondary" } else { "btn btn-primary" },
                            onclick: move |_| on_play.call(ch.to_string()),
                            Icon { name: IconName::Play }
                            "Play {ch}"
                        }
                    }
                    button {
                        id: control_id("btn", "play all"),
                        class: "btn btn-secondary",
                        disabled: html_bool(pool_empty),
                        onclick: move |_| on_play.call(all_chars.clone()),
                        Icon { name: IconName::Headphones }
                        "Play all"
                    }
                    button {
                        id: control_id("btn", "stream groups"),
                        class: "btn btn-secondary",
                        disabled: html_bool(pool_empty),
                        onclick: move |_| on_stream.call(()),
                        Icon { name: IconName::Repeat }
                        "Stream groups"
                    }
                }
                if can_float {
                    button {
                        id: control_id("btn", "floating window"),
                        class: "btn btn-secondary",
                        title: "Show the groups in a small window kept above your other programs",
                        "aria-pressed": if floating { "true" } else { "false" },
                        onclick: move |_| on_float.call(()),
                        Icon { name: IconName::Sparkle }
                        if floating { "Close floating window" } else { "Floating window" }
                    }
                    if floating {
                        button {
                            id: control_id("btn", "lock floating window"),
                            class: "btn btn-secondary",
                            title: "Locked, clicks go through the floating window to the program underneath",
                            "aria-pressed": if float_locked { "true" } else { "false" },
                            onclick: move |_| on_float_lock.call(()),
                            if float_locked { "Unlock floating window" } else { "Lock floating window (click-through)" }
                        }
                        if !float_locked {
                            p { id: "floating-hint", class: "muted", style: "margin: 0; font-size: 0.85rem;",
                                "Drag it where you want it. To keep it above other windows, right-click it and choose Always on Top, then lock it."
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::chip_id;

    #[test]
    fn every_character_chip_has_an_id_of_its_own() {
        assert_eq!(chip_id('K'), "chip-k");
        assert_eq!(chip_id('7'), "chip-7");
        let punctuation: Vec<String> = ['?', '/', '.', ',', '=', '+'].map(chip_id).into();
        let mut unique = punctuation.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), punctuation.len(), "{punctuation:?}");
        assert!(punctuation.iter().all(|id| id.starts_with("chip-")));
    }
}
