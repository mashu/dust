use dioxus::prelude::*;

use crate::audio::focus_group_input;
use crate::ui::widgets::ProgressHeader;

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
    rsx! {
        div { class: "stack",
            ProgressHeader { current: focused, total }
            div { class: "card",
                p { class: "muted", if playing { "Listening…" } else { "Enter answers per group (auto-advances when complete)." } }
                div { class: "stack group-list",
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
                            let shown = if is_confirmed { sent.clone() } else { "••••".into() };
                            let cls = if is_focused { "group focused" } else { "group" };
                            let input_cls = if disabled {
                                "answer"
                            } else if input_locked {
                                "answer locked"
                            } else {
                                "answer"
                            };
                            let placeholder = if awaiting_play {
                                "Waiting..."
                            } else if input_locked {
                                "Listening..."
                            } else if disabled {
                                "Waiting..."
                            } else {
                                "Type group answer..."
                            };
                            rsx! {
                                div { id: "group-card-{idx}", class: cls,
                                    div { class: "row", style: "justify-content: space-between;",
                                        div { class: "row",
                                            span { class: if is_focused { "badge current" } else { "badge" }, "Group {idx + 1}" }
                                            if is_focused {
                                                span { class: "badge current", "Current" }
                                            }
                                        }
                                        div { class: "row",
                                            span { class: "sent", "{shown}" }
                                            if is_confirmed {
                                                if value.trim().eq_ignore_ascii_case(sent) {
                                                    span { style: "color: var(--emerald-600);", "✓" }
                                                } else {
                                                    span { style: "color: var(--rose-600);", "✗" }
                                                }
                                            }
                                        }
                                    }
                                    input {
                                        id: "group-input-{idx}",
                                        class: input_cls,
                                        value: "{value}",
                                        disabled: disabled,
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
                p { class: "tiny", style: "margin-top: 0.6rem; text-transform: none; letter-spacing: 0;",
                    "Auto-advances when the group is complete · Enter to confirm"
                }
            }
            div { class: "train-actions",
                button { class: "btn btn-primary", onclick: move |_| on_submit.call(()), "End session" }
                button { class: "btn btn-danger", onclick: move |_| on_stop.call(()), "Stop" }
            }
        }
    }
}
