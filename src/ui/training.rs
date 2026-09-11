use dioxus::prelude::*;

use crate::audio::focus_group_input;
use crate::ui::widgets::{Icon, ProgressHeader};

#[component]
pub fn TrainingView(
    current: usize,
    total: usize,
    groups: Vec<String>,
    inputs: Vec<String>,
    confirmed: Vec<bool>,
    focused: usize,
    playing: bool,
    locked: bool,
    repeat_total: u32,
    repeat_done: u32,
    on_change: EventHandler<(usize, String)>,
    on_confirm: EventHandler<usize>,
    on_focus: EventHandler<usize>,
    on_submit: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    use_effect(use_reactive!(|focused, current| {
        let _ = current;
        focus_group_input(focused);
    }));
    let send_index = (repeat_done + 1).min(repeat_total.max(1));
    let status = if playing && repeat_total > 1 {
        format!("Sending {send_index} of {repeat_total}")
    } else if playing {
        "Sending".to_string()
    } else {
        "Your turn".to_string()
    };
    let hint = if playing {
        "Listen — the answer box unlocks when the group finishes."
    } else {
        "Type what you heard. It advances on its own when the length matches."
    };
    rsx! {
        div { class: "stack",
            ProgressHeader { current: focused, total, status, live: playing }
            div { class: "card",
                div { class: "row-between",
                    p { class: "muted", style: "margin: 0;", "{hint}" }
                    if repeat_total > 1 {
                        span { class: "chip",
                            Icon { name: "repeat" }
                            "{repeat_total}× per group"
                        }
                    }
                }
                div { class: "group-list", style: "margin-top: 0.85rem;",
                    for (idx, sent) in groups.iter().enumerate() {
                        {
                            let is_focused = focused == idx;
                            let is_active = current == idx;
                            let is_confirmed = confirmed.get(idx).copied().unwrap_or(false);
                            let awaiting_play = is_focused && !is_active && !is_confirmed;
                            let disabled = is_confirmed || (!is_active && !awaiting_play);
                            let input_locked =
                                (locked && is_active && !is_confirmed) || awaiting_play;
                            let value = inputs.get(idx).cloned().unwrap_or_default();
                            let shown = if is_confirmed { sent.clone() } else { "•••".into() };
                            let cls = if is_focused {
                                "group focused"
                            } else if is_confirmed {
                                "group done"
                            } else {
                                "group"
                            };
                            let input_cls = if input_locked { "answer locked" } else { "answer" };
                            let correct = value.trim().eq_ignore_ascii_case(sent);
                            let placeholder = if awaiting_play {
                                "Waiting…"
                            } else if input_locked {
                                "Listening…"
                            } else if disabled {
                                "Waiting…"
                            } else {
                                "Type the group"
                            };
                            rsx! {
                                div { id: "group-card-{idx}", class: cls,
                                    div { class: "row-between",
                                        div { class: "row", style: "gap: 0.35rem;",
                                            span { class: if is_focused { "badge current" } else { "badge" }, "{idx + 1}" }
                                            if is_focused && playing && repeat_total > 1 {
                                                span { class: "badge current", "Send {send_index}/{repeat_total}" }
                                            }
                                        }
                                        div { class: "row", style: "gap: 0.4rem;",
                                            span { class: "sent", "{shown}" }
                                            if is_confirmed {
                                                span { class: if correct { "badge good" } else { "badge bad" },
                                                    Icon { name: if correct { "check" } else { "x" } }
                                                }
                                            }
                                        }
                                    }
                                    input {
                                        id: "group-input-{idx}",
                                        class: input_cls,
                                        value: "{value}",
                                        disabled,
                                        readonly: input_locked,
                                        autofocus: is_active && !is_confirmed,
                                        placeholder: "{placeholder}",
                                        autocomplete: "off",
                                        autocorrect: "off",
                                        autocapitalize: "characters",
                                        spellcheck: false,
                                        enterkeyhint: "done",
                                        inputmode: "text",
                                        lang: "zxx",
                                        onfocus: move |_| on_focus.call(idx),
                                        oninput: move |e| on_change.call((idx, e.value())),
                                        onkeydown: move |e| {
                                            if input_locked {
                                                e.prevent_default();
                                                return;
                                            }
                                            if e.key() == Key::Enter && !disabled {
                                                on_confirm.call(idx);
                                            }
                                        }
                                    }
                                    if is_confirmed {
                                        div { style: "margin-top: 0.5rem;",
                                            crate::ui::widgets::CharacterComparison {
                                                sent: sent.clone(),
                                                received: value.trim().to_ascii_uppercase(),
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "train-actions",
                button { class: "btn btn-primary", onclick: move |_| on_submit.call(()),
                    Icon { name: "flag" }
                    "End session"
                }
                button { class: "btn btn-danger", onclick: move |_| on_stop.call(()),
                    Icon { name: "x" }
                    "Discard"
                }
            }
        }
    }
}
