use cw_core::answer_length_matches;
use dioxus::prelude::*;

use crate::audio::focus_group_input;
use crate::time::sleep_ms;
use crate::ui::widgets::{control_id, Icon, ProgressHeader};

/// Incomplete answers wait this long before the session sees them. Short
/// enough that a pause still lands well before auto-confirm would have cared,
/// long enough that a burst of keys is one Input, not one per character.
const INPUT_COMMIT_DEBOUNCE_MS: u32 = 48;

const _: () = assert!(INPUT_COMMIT_DEBOUNCE_MS < cw_core::AUTO_CONFIRM_DELAY_MS);

/// The progress header, kept out of the view that updates on every key.
#[component]
pub fn TrainingScope(
    focused: usize,
    total: usize,
    playing: bool,
    repeat_total: u32,
    repeat_done: u32,
) -> Element {
    let send_index = (repeat_done + 1).min(repeat_total.max(1));
    let status = if playing && repeat_total > 1 {
        format!("Sending {send_index} of {repeat_total}")
    } else if playing {
        "Sending".to_string()
    } else {
        "Your turn".to_string()
    };
    rsx! {
        div { style: "display: contents;",
            ProgressHeader { current: focused, total, status, live: playing }
        }
    }
}

fn bump_epoch(mut epoch: Signal<u64>) -> u64 {
    let next = epoch.peek().saturating_add(1);
    epoch.set(next);
    next
}

/// One group: what was sent, the box you answer it in, and the comparison
/// once it is confirmed.
///
/// Its own component for one reason, and it is the whole reason: a key lands
/// in one box, and the twenty other cards on the screen have not changed. As
/// an inline loop body every keystroke rebuilt and re-diffed all of them —
/// twenty cards of roughly twenty nodes each, over the wire to the webview,
/// between the key going down and the letter appearing. Given props to compare
/// instead, the nineteen that did not move are skipped.
///
/// So nothing in here may read the draft signal. The value arrives already
/// worked out; reading it here would subscribe every card to every keystroke
/// again and quietly undo the whole thing.
#[component]
#[allow(clippy::too_many_arguments)]
fn GroupCard(
    idx: usize,
    sent: String,
    shown: String,
    value: String,
    correct: bool,
    disabled: bool,
    input_locked: bool,
    is_focused: bool,
    is_active: bool,
    is_confirmed: bool,
    send_badge: Option<String>,
    placeholder: String,
    draft: Signal<String>,
    draft_at: Signal<usize>,
    committed: Signal<String>,
    committed_at: Signal<usize>,
    debounce_gen: Signal<u64>,
    on_change: EventHandler<(usize, String)>,
    on_confirm: EventHandler<usize>,
    on_focus: EventHandler<usize>,
) -> Element {
    let mut draft = draft;
    let mut draft_at = draft_at;
    let mut committed = committed;
    let mut committed_at = committed_at;
    let cls = if is_focused {
        "group focused"
    } else if is_confirmed {
        "group done"
    } else {
        "group"
    };
    let input_cls = if input_locked {
        "answer locked"
    } else {
        "answer"
    };
    let sent_for_input = sent.clone();
    rsx! {
        div { id: "group-card-{idx}", class: cls,
            div { class: "row-between",
                div { class: "row", style: "gap: 0.35rem;",
                    span { class: if is_focused { "badge current" } else { "badge" }, "{idx + 1}" }
                    if let Some(badge) = send_badge {
                        span { class: "badge current", "{badge}" }
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
                oninput: move |e| {
                    if input_locked {
                        return;
                    }
                    let value = e.value();
                    draft_at.set(idx);
                    draft.set(value.clone());
                    let matches_now = answer_length_matches(&sent_for_input, &value);
                    let matches_committed = *committed_at.peek() == idx
                        && answer_length_matches(&sent_for_input, committed.peek().as_str());
                    if matches_now || matches_committed {
                        let _ = bump_epoch(debounce_gen);
                        committed.set(value.clone());
                        committed_at.set(idx);
                        on_change.call((idx, value));
                        return;
                    }
                    let gen = bump_epoch(debounce_gen);
                    spawn(async move {
                        sleep_ms(INPUT_COMMIT_DEBOUNCE_MS).await;
                        if *debounce_gen.peek() != gen {
                            return;
                        }
                        committed.set(value.clone());
                        committed_at.set(idx);
                        on_change.call((idx, value));
                    });
                },
                onkeydown: move |e| {
                    if input_locked {
                        e.prevent_default();
                        return;
                    }
                    if e.key() == Key::Enter && !disabled {
                        let value = draft.peek().clone();
                        let _ = bump_epoch(debounce_gen);
                        if *committed_at.peek() != idx || committed.peek().as_str() != value {
                            committed.set(value.clone());
                            committed_at.set(idx);
                            on_change.call((idx, value));
                        }
                        on_confirm.call(idx);
                    }
                }
            }
            if is_confirmed {
                div { style: "margin-top: 0.5rem;",
                    crate::ui::widgets::CharacterComparison {
                        sent,
                        received: value.trim().to_ascii_uppercase(),
                    }
                }
            }
        }
    }
}

/// The group list and the way out of the session.
///
/// The answer box is local. The session machine hears a commit when the
/// length matches, when an incomplete draft has sat still, or when Enter /
/// End session flush what's there. Typing itself does not clone the session.
#[component]
pub fn TrainingView(
    current: usize,
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
    let send_index = (repeat_done + 1).min(repeat_total.max(1));
    let draft = use_signal(String::new);
    let draft_at = use_signal(|| usize::MAX);
    let mut committed = use_signal(String::new);
    let mut committed_at = use_signal(|| usize::MAX);
    let debounce_gen = use_signal(|| 0u64);
    use_effect(use_reactive!(|focused, current| {
        let _ = current;
        focus_group_input(focused);
    }));
    use_effect(use_reactive!(|current| {
        let _ = current;
        let _ = bump_epoch(debounce_gen);
    }));
    use_drop(move || {
        let _ = bump_epoch(debounce_gen);
    });
    let hint = if playing {
        "Listen — the answer box unlocks when the group finishes."
    } else {
        "Type what you heard. It advances on its own when the length matches."
    };
    rsx! {
        div { style: "display: contents;",
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
                            let value = if !is_confirmed && is_active && draft_at() == idx {
                                draft()
                            } else {
                                inputs.get(idx).cloned().unwrap_or_default()
                            };
                            let correct = value.trim().eq_ignore_ascii_case(sent);
                            rsx! {
                                GroupCard {
                                    key: "{idx}",
                                    idx,
                                    sent: sent.clone(),
                                    shown: if is_confirmed { sent.clone() } else { "•••".into() },
                                    value,
                                    correct,
                                    disabled,
                                    input_locked,
                                    is_focused,
                                    is_active,
                                    is_confirmed,
                                    send_badge: (is_focused && playing && repeat_total > 1)
                                        .then(|| format!("Send {send_index}/{repeat_total}")),
                                    placeholder: if awaiting_play {
                                        "Waiting…".to_string()
                                    } else if input_locked {
                                        "Listening…".to_string()
                                    } else if disabled {
                                        "Waiting…".to_string()
                                    } else {
                                        "Type the group".to_string()
                                    },
                                    draft,
                                    draft_at,
                                    committed,
                                    committed_at,
                                    debounce_gen,
                                    on_change,
                                    on_confirm,
                                    on_focus,
                                }
                            }
                        }
                    }
                }
            }
            div { class: "train-actions",
                button {
                    id: control_id("btn", "end session"),
                    class: "btn btn-primary",
                    onclick: move |_| {
                        let _ = bump_epoch(debounce_gen);
                        let value = draft.peek().clone();
                        if *draft_at.peek() == current
                            && (*committed_at.peek() != current
                                || committed.peek().as_str() != value)
                        {
                            committed.set(value.clone());
                            committed_at.set(current);
                            on_change.call((current, value));
                        }
                        on_submit.call(());
                    },
                    Icon { name: "flag" }
                    "End session"
                }
                button {
                    id: control_id("btn", "discard"),
                    class: "btn btn-danger",
                    onclick: move |_| on_stop.call(()),
                    Icon { name: "x" }
                    "Discard"
                }
            }
        }
    }
}
