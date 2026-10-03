use cw_core::{answer_length_matches, paddle_from_bracket, KeyerMode, Paddle, PaddleKeyer};
use dioxus::prelude::*;

use crate::time::sleep_ms;
use crate::ui::focus::focus_group_input;
use crate::ui::paddle::{
    bracket_from_key, bump_epoch, capture_dom_paddles, paddle_down, paddle_up, TrainingPaddleSink,
};
use crate::ui::widgets::{control_id, html_bool, Icon, ProgressHeader};

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

#[allow(clippy::too_many_arguments)]
fn commit_draft(
    idx: usize,
    value: String,
    sent: &str,
    mut draft: Signal<String>,
    mut draft_at: Signal<usize>,
    mut committed: Signal<String>,
    mut committed_at: Signal<usize>,
    debounce_gen: Signal<u64>,
    on_change: EventHandler<(usize, String)>,
) {
    draft_at.set(idx);
    draft.set(value.clone());
    let matches_now = answer_length_matches(sent, &value);
    let matches_committed =
        *committed_at.peek() == idx && answer_length_matches(sent, committed.peek().as_str());
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
}

#[allow(clippy::too_many_arguments)]
fn push_keyed_char(
    ch: char,
    current: usize,
    sent_now: &str,
    draft: Signal<String>,
    draft_at: Signal<usize>,
    committed: Signal<String>,
    committed_at: Signal<usize>,
    debounce_gen: Signal<u64>,
    on_change: EventHandler<(usize, String)>,
) {
    let mut text = draft.peek().clone();
    text.push(ch);
    commit_draft(
        current,
        text,
        sent_now,
        draft,
        draft_at,
        committed,
        committed_at,
        debounce_gen,
        on_change,
    );
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
/// The active card may read the draft signal. The others must not: a read
/// would subscribe every card to every keystroke and quietly undo the split.
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
    #[props(default)] paddle_swap: bool,
    #[props(default)] on_paddle_down: EventHandler<Paddle>,
    #[props(default)] on_paddle_up: EventHandler<Paddle>,
) -> Element {
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
    let closed = disabled || input_locked;
    let display = if is_active && draft_at() == idx {
        draft()
    } else {
        value.clone()
    };
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
                key: if is_active && !input_locked { "live" } else { "idle" },
                id: "group-input-{idx}",
                class: input_cls,
                value: "{display}",
                disabled: html_bool(disabled),
                readonly: html_bool(input_locked || is_confirmed),
                autofocus: html_bool(is_active && !input_locked),
                placeholder: "{placeholder}",
                autocomplete: "off",
                autocorrect: "off",
                autocapitalize: "characters",
                spellcheck: false,
                enterkeyhint: "done",
                inputmode: "text",
                lang: "zxx",
                onmounted: move |evt| {
                    if is_active && !input_locked {
                        focus_group_input(idx);
                        // Tests rebuild a VirtualDom with no webview; set_focus
                        // panics if it tries to query one.
                        #[cfg(not(test))]
                        spawn(async move {
                            let _ = evt.data().set_focus(true).await;
                        });
                        #[cfg(test)]
                        let _ = evt;
                    }
                },
                onfocus: move |_| on_focus.call(idx),
                oninput: move |e| {
                    if closed {
                        return;
                    }
                    commit_draft(
                        idx,
                        e.value(),
                        &sent_for_input,
                        draft,
                        draft_at,
                        committed,
                        committed_at,
                        debounce_gen,
                        on_change,
                    );
                },
                onkeydown: move |e| {
                    if let Some(ch) = bracket_from_key(&e) {
                        e.prevent_default();
                        if e.is_auto_repeating() {
                            return;
                        }
                        if capture_dom_paddles() {
                            if let Some(paddle) = paddle_from_bracket(ch, paddle_swap) {
                                on_paddle_down.call(paddle);
                            }
                        }
                        return;
                    }
                    if closed {
                        e.prevent_default();
                        return;
                    }
                    if e.key() == Key::Enter {
                        let value = draft.peek().clone();
                        let _ = bump_epoch(debounce_gen);
                        if *committed_at.peek() != idx || committed.peek().as_str() != value {
                            committed.set(value.clone());
                            committed_at.set(idx);
                            on_change.call((idx, value));
                        }
                        on_confirm.call(idx);
                    }
                },
                onkeyup: move |e| {
                    if let Some(ch) = bracket_from_key(&e) {
                        e.prevent_default();
                        if capture_dom_paddles() {
                            if let Some(paddle) = paddle_from_bracket(ch, paddle_swap) {
                                on_paddle_up.call(paddle);
                            }
                        }
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
    #[props(default)] paddle_swap: bool,
    #[props(default)] keyer_mode: KeyerMode,
    #[props(default)] dit_ms: u64,
    #[props(default)] on_tone: EventHandler<bool>,
) -> Element {
    let dit_ms = if dit_ms == 0 { 60 } else { dit_ms };
    let send_index = (repeat_done + 1).min(repeat_total.max(1));
    let mut draft = use_signal(String::new);
    let mut draft_at = use_signal(|| usize::MAX);
    let mut committed = use_signal(String::new);
    let mut committed_at = use_signal(|| usize::MAX);
    let debounce_gen = use_signal(|| 0u64);
    let mut keyer = use_signal(|| PaddleKeyer::with_mode(keyer_mode));
    let mut keying = use_signal(|| false);
    let paddle_gen = use_signal(|| 0u64);
    let training_sink = use_hook(try_consume_context::<TrainingPaddleSink>);
    let current_done = confirmed.get(current).copied().unwrap_or(false);
    let can_paddle = !playing && !locked && !current_done;
    let sent_now = groups.get(current).cloned().unwrap_or_default();
    // The live box remounts when the send ends. `focused`/`current` do not
    // change then, so this effect has to watch the lock as well — otherwise
    // the webview never hears that it should grab the new node.
    let box_open = !current_done && !(playing && locked);
    use_effect(use_reactive!(|focused, box_open| {
        if box_open {
            focus_group_input(focused);
        }
    }));
    use_effect(use_reactive!(|keyer_mode| {
        keyer.write().set_mode(keyer_mode);
    }));
    use_effect(use_reactive!(|current, current_done| {
        let _ = bump_epoch(debounce_gen);
        let _ = bump_epoch(paddle_gen);
        keyer.write().reset();
        keying.set(false);
        on_tone.call(false);
        if current_done {
            draft.set(String::new());
            draft_at.set(usize::MAX);
        }
        let _ = current;
    }));
    use_drop(move || {
        let _ = bump_epoch(debounce_gen);
        let _ = bump_epoch(paddle_gen);
        on_tone.call(false);
    });
    let on_paddle_down = EventHandler::new({
        let sent_now = sent_now.clone();
        move |paddle: Paddle| {
            let on_letter = EventHandler::new({
                let sent_now = sent_now.clone();
                move |ch: char| {
                    push_keyed_char(
                        ch,
                        current,
                        &sent_now,
                        draft,
                        draft_at,
                        committed,
                        committed_at,
                        debounce_gen,
                        on_change,
                    );
                }
            });
            paddle_down(
                paddle, dit_ms, can_paddle, keyer, keying, paddle_gen, on_tone, on_letter,
            );
        }
    });
    let on_paddle_up = EventHandler::new({
        move |paddle: Paddle| {
            let on_letter = EventHandler::new({
                let sent_now = sent_now.clone();
                move |ch: char| {
                    push_keyed_char(
                        ch,
                        current,
                        &sent_now,
                        draft,
                        draft_at,
                        committed,
                        committed_at,
                        debounce_gen,
                        on_change,
                    );
                }
            });
            paddle_up(paddle, dit_ms, keyer, paddle_gen, on_tone, on_letter);
        }
    });
    if let Some(sink) = &training_sink {
        sink.bind(on_paddle_down, on_paddle_up);
    }
    use_drop(move || {
        if let Some(sink) = training_sink {
            sink.clear();
        }
    });
    let hint = if playing && locked {
        "Listen — the answer box unlocks when the group finishes."
    } else if playing {
        "Listen, or type along — the box stays open while this group is sent."
    } else {
        match keyer_mode {
            KeyerMode::Straight => {
                "Type the letters, or hold [ or ] as a straight key — a short press is a dit, a long one a dah."
            }
            KeyerMode::Ultimatic => {
                "Type the letters, or hold [ for dits and ] for dahs. The last paddle you close repeats — squeeze for the middle of X or P."
            }
            KeyerMode::IambicA | KeyerMode::IambicB => {
                "Type the letters, or hold [ for dits and ] for dahs. Squeeze both to alternate. A hold repeats at the key speed."
            }
        }
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
                            let is_confirmed = confirmed.get(idx).copied().unwrap_or(false);
                            let is_active = current == idx && !is_confirmed;
                            let awaiting_play = is_focused && !is_active && !is_confirmed;
                            let disabled = is_confirmed || (!is_active && !awaiting_play);
                            let input_locked =
                                (locked && is_active && !is_confirmed) || awaiting_play;
                            let value = inputs.get(idx).cloned().unwrap_or_default();
                            let correct = value.trim().eq_ignore_ascii_case(sent);
                            let card_key = format!(
                                "{idx}-{}",
                                if is_active && !input_locked { "live" } else { "wait" }
                            );
                            rsx! {
                                GroupCard {
                                    key: "{card_key}",
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
                                    paddle_swap,
                                    on_paddle_down,
                                    on_paddle_up,
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
