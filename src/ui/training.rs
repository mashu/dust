use cw_core::{ChosenSpeed, MonoMs, answer_length_matches};
use dioxus::core::Runtime;
use dioxus::prelude::*;

use crate::session_runtime::{AnswerOutbox, TypedAnswer};
use crate::time::{mono_ms, sleep_ms};
use crate::ui::paddle::PaddleKeys;
use crate::ui::widgets::{
    CharacterComparison, Icon, IconName, ProgressHeader, StatusDot, control_id, html_bool,
};

/// Incomplete answers wait this long before the session sees them. Short
/// enough that a pause still lands well before auto-confirm would have cared,
/// long enough that a burst of keys is one Input, not one per character.
pub(crate) const INPUT_COMMIT_DEBOUNCE_MS: u32 = 48;

const _: () = assert!(INPUT_COMMIT_DEBOUNCE_MS < cw_core::AUTO_CONFIRM_DELAY_MS);

/// Whether the box in focus is the one being answered: the current group's,
/// not answered yet. Between two groups it is not, for the focus has moved on
/// to the next group, which is still to be sent, while the current group is
/// the one just answered.
pub(crate) fn focused_box_open(current: usize, focused: usize, confirmed: &[bool]) -> bool {
    current == focused && !confirmed.get(focused).copied().unwrap_or(false)
}

/// The progress header, kept out of the view that updates on every key.
#[component]
pub fn TrainingProgress(
    focused: usize,
    total: usize,
    playing: bool,
    /// The box in focus is the one being answered ([`focused_box_open`]).
    open: bool,
    repeat_total: u32,
    repeat_done: u32,
    /// Seconds left before the first group, while the session counts down.
    countdown: Option<u32>,
) -> Element {
    let send_index = (repeat_done + 1).min(repeat_total.max(1));
    let status = if let Some(left) = countdown {
        format!("Starting in {left}")
    } else if playing && repeat_total > 1 {
        format!("Sending {send_index} of {repeat_total}")
    } else if playing {
        "Sending".to_string()
    } else if open {
        "Your turn".to_string()
    } else {
        // Between two groups, for a word space and the pause after a group:
        // the box below waits for the next one and takes no typing, so it is
        // nobody's turn yet.
        "Next group…".to_string()
    };
    let dot = if playing {
        StatusDot::Live
    } else if open {
        StatusDot::Ready
    } else {
        StatusDot::Waiting
    };
    let pct = if total == 0 {
        0.0
    } else {
        ((focused + 1) as f64 / total as f64) * 100.0
    };
    rsx! {
        div { style: "display: contents;",
            ProgressHeader {
                status,
                dot,
                figure: format!("{} / {total}", focused + 1),
                pct,
            }
        }
    }
}

/// The answer being typed into the open box.
///
/// Typing stays in the box, and waits in the [`AnswerOutbox`]. The session is
/// handed it when its length matches the group, or once it has sat still for
/// the debounce — or, by the runtime, the moment anything else happens to the
/// session first: Enter, End session, Exit, the answer window closing.
///
/// A key redraws nothing. The box is a controlled input, and drawing it writes
/// its `value` again whether or not it changed; on the desktop and mobile
/// builds that write reaches the page a frame later, over whatever was typed
/// in the meantime, and the key typed in between is lost for good. What is
/// typed is in the box already, so while it is open the box is drawn only for
/// what it has not seen: a letter the keypad or the paddles keyed. Its card is
/// drawn without it ([`AnswerBox`]).
#[derive(Clone, Copy, PartialEq)]
struct AnswerDraft {
    /// The text in the box. Not reactive: a key is in the box before it is
    /// here, and drawing the box for it would only write it back.
    text: CopyValue<String>,
    /// Moved on when `text` changes in a way the box has not seen, which
    /// draws the box again. Only the box reads it, so it redraws that box
    /// alone.
    repaint: Signal<u64>,
    /// The group `text` is the answer to.
    at: CopyValue<Option<usize>>,
    outbox: AnswerOutbox,
    /// Moved on by every key and every new group: a debounce that finds it
    /// moved stands down.
    epoch: CopyValue<u64>,
    /// The view the debounce belongs to, so one still waiting ends with it,
    /// whoever started it — a key in the box, the keypad, or the paddles.
    owner: ScopeId,
    on_change: EventHandler<TypedAnswer>,
}

impl AnswerDraft {
    /// A draft that starts out as `held`: the group and answer the box opens
    /// on, if it opens on anything.
    fn use_new(
        outbox: AnswerOutbox,
        on_change: EventHandler<TypedAnswer>,
        held: impl FnOnce() -> Option<(usize, String)>,
    ) -> Self {
        use_hook(|| {
            let (at, text) = held().map_or((None, String::new()), |(idx, text)| (Some(idx), text));
            Self {
                text: CopyValue::new(text),
                repaint: Signal::new(0),
                at: CopyValue::new(at),
                outbox,
                epoch: CopyValue::new(0),
                owner: dioxus::core::current_scope_id(),
                on_change,
            }
        })
    }

    /// What the box for `idx` shows while the draft is its own. Reading it
    /// subscribes the caller to the draft's rewrites — before the first one
    /// too, since a keyed letter is what can make the draft this box's.
    fn read_for(self, idx: usize) -> Option<String> {
        let _ = self.repaint.read();
        self.peek_for(idx)
    }

    fn peek_for(self, idx: usize) -> Option<String> {
        (*self.at.peek() == Some(idx)).then(|| self.text.peek().clone())
    }

    /// The box for `idx` now holds `text`, so nothing is drawn for it: what
    /// is new in it keyed at `keyed`, or typed for you, untimed, at `None`.
    /// The session is handed it at once when its length matches the group's
    /// — auto-confirm starts from there — and otherwise once typing has
    /// paused, or sooner if anything happens to the session first.
    fn edit(mut self, idx: usize, text: String, sent: &str, keyed: Option<MonoMs>) {
        let full = answer_length_matches(sent, &text);
        self.at.set(Some(idx));
        self.text.set(text.clone());
        self.outbox.hold(idx, text, keyed);
        let epoch = self.cancel_pending();
        if full {
            self.hand_over();
            return;
        }
        // Spawned from inside the view's own scope, which is what ties the
        // pause to it: a session ending on the heels of a key takes the view
        // away, and the pause goes with it rather than waking to a draft that
        // is gone. (`Runtime::spawn` on a scope id does not.)
        Runtime::current().in_scope(self.owner, || {
            spawn(async move {
                sleep_ms(INPUT_COMMIT_DEBOUNCE_MS).await;
                if *self.epoch.peek() == epoch {
                    self.hand_over();
                }
            })
        });
    }

    /// The keypad or a paddle keyed `letter` into the box for `idx` at
    /// `keyed`. The page has not seen it, so the box is drawn again to show
    /// it.
    fn push(self, idx: usize, letter: char, sent: &str, keyed: Option<MonoMs>) {
        let mut text = self.peek_for(idx).unwrap_or_default();
        text.push(letter);
        self.edit(idx, text, sent, keyed);
        self.redraw();
    }

    /// The keypad's ⌫ took the last character out of the box for `idx`.
    fn pop(self, idx: usize, sent: &str) {
        let mut text = self.peek_for(idx).unwrap_or_default();
        if text.pop().is_some() {
            self.edit(idx, text, sent, None);
            self.redraw();
        }
    }

    /// Hand the session whatever is waiting. A debounce still waiting has
    /// nothing left to do.
    fn hand_over(self) {
        self.cancel_pending();
        if let Some(answer) = self.outbox.take() {
            self.on_change.call(answer);
        }
    }

    /// A new group is up, or this one is over: the pause has nothing left to
    /// wait for, and a finished group's draft is gone. Whatever was typed is
    /// with the session already — only the session moves a group on, and it
    /// takes what is waiting first. A finished group moves up to the slot
    /// above, drawn from what the session holds, and the box is drawn empty
    /// for the next one.
    fn settle(mut self, finished: bool) {
        self.cancel_pending();
        if finished {
            self.at.set(None);
            self.text.set(String::new());
            self.redraw();
        }
    }

    /// Draw the box again, for a change it has not seen — or to put back
    /// what a box that is not taking typing was typed into.
    fn redraw(mut self) {
        *self.repaint.write() += 1;
    }

    fn cancel_pending(mut self) -> u64 {
        let next = self.epoch.peek().wrapping_add(1);
        self.epoch.set(next);
        next
    }
}

/// Whether the phone's own keyboard is kept away from the answer box.
///
/// The keypad under the groups types into it instead, so the keyboard would
/// only cover the screen — and come and go with every group, moving
/// everything above it each time. A keyboard plugged in or paired still
/// types into the box: `none` asks only that no keyboard be drawn on screen.
pub(crate) const ANSWER_INPUT_MODE: &str = "none";

/// The open group: its number, how many times it has been sent, and the box
/// it is answered in.
///
/// It is the one place an answer is typed, for every group of the session,
/// and the same box all the while: a box that came and went with each group
/// took the focus with it, and with the focus the keyboard.
#[component]
fn OpenCard(
    idx: usize,
    sent: String,
    /// The box takes typing: the group has been sent, or the lock is off.
    live: bool,
    send_badge: Option<String>,
    placeholder: &'static str,
    draft: AnswerDraft,
    on_confirm: EventHandler<()>,
) -> Element {
    // A box that opens takes the focus, so a keyboard types straight into
    // it. One that has it already keeps it, and nothing moves.
    use_effect(use_reactive!(|idx, live| {
        // Tests have no page to run it on.
        #[cfg(not(test))]
        if live {
            spawn(async move {
                let script = crate::ui::widgets::focus_script(&format!("group-input-{idx}"));
                let _ = document::eval(&script).await;
            });
        }
        #[cfg(test)]
        let _ = (idx, live);
    }));
    rsx! {
        div { id: "group-card-{idx}", class: "group focused",
            div { class: "row-between",
                div { class: "row", style: "gap: 0.35rem;",
                    span { class: "badge current", "{idx + 1}" }
                    if let Some(badge) = send_badge {
                        span { class: "badge current", "{badge}" }
                    }
                }
                span { class: "sent", "•••" }
            }
            AnswerBox {
                idx,
                sent,
                live,
                placeholder,
                draft,
                on_confirm,
            }
        }
    }
}

/// The group just answered, dimmed, above the open one: what was sent, what
/// you copied, and whether it was right.
#[component]
fn PreviousCard(idx: usize, sent: String, received: String) -> Element {
    let received = received.trim().to_ascii_uppercase();
    let correct = received.eq_ignore_ascii_case(&sent);
    rsx! {
        div { id: "group-card-{idx}", class: "group done",
            div { class: "row-between",
                span { class: "badge", "{idx + 1}" }
                div { class: "row", style: "gap: 0.4rem;",
                    span { class: "sent", "{sent}" }
                    span { class: if correct { "badge good" } else { "badge bad" },
                        Icon { name: if correct { IconName::Check } else { IconName::X } }
                    }
                }
            }
            div { style: "margin-top: 0.5rem;",
                CharacterComparison { sent, received }
            }
        }
    }
}

/// The box a group is answered in.
///
/// A component of its own so that its card can be drawn without it. Drawing
/// the box writes its `value` again, whether or not it changed, and on the
/// desktop and mobile builds that write reaches the page a frame later, over
/// whatever was typed in the meantime. The card is drawn again for things the
/// box has nothing to do with — the send counter, while a group repeats with
/// the box open — and given props to compare, the box is skipped then.
///
/// It is never disabled or read-only, not even while a group is sent with
/// the lock on: either takes the focus away, and on a phone the keyboard
/// with it. A box that is not taking typing turns away what reaches it
/// instead, and draws itself again over it.
#[component]
fn AnswerBox(
    idx: usize,
    sent: String,
    live: bool,
    placeholder: &'static str,
    draft: AnswerDraft,
    on_confirm: EventHandler<()>,
) -> Element {
    let shown = draft.read_for(idx).unwrap_or_default();
    rsx! {
        input {
            id: "group-input-{idx}",
            class: if live { "answer" } else { "answer locked" },
            aria_label: "Answer for group {idx + 1}",
            value: "{shown}",
            placeholder,
            autocomplete: "off",
            autocorrect: "off",
            autocapitalize: "characters",
            spellcheck: false,
            enterkeyhint: "done",
            inputmode: ANSWER_INPUT_MODE,
            lang: "zxx",
            oninput: move |e| {
                // The key's moment, before anything else is done with it.
                let keyed = MonoMs(mono_ms());
                if live {
                    draft.edit(idx, e.value(), &sent, Some(keyed));
                } else {
                    draft.redraw();
                }
            },
            // `[` and `]` are paddles: the app's root keys them and keeps
            // them out of the box.
            onkeydown: move |e| {
                if !live {
                    e.prevent_default();
                    return;
                }
                // What is typed reaches the session ahead of the confirm.
                if e.key() == Key::Enter {
                    on_confirm.call(());
                }
            },
        }
    }
}

/// Whether touch is how this build types: the phone and tablet builds,
/// where the keypad always shows. Elsewhere it shows only on a touch screen.
pub(crate) const TOUCH_BUILD: bool = cfg!(any(feature = "mobile", feature = "mobile-silent"));

/// A key on the keypad.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PadKey {
    Char(char),
    Back,
    /// The answer is done.
    Next,
    /// A run's next field: from Call on to Exchange, a space in Exchange.
    Field,
    /// A run's Enter, named for what it sends.
    Send(String),
}

/// One row of keys, and how the row lays them out: `keypad-row`, with a
/// class of its own for a row that is not ten even keys.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PadRow {
    pub keys: Vec<PadKey>,
    pub class: &'static str,
}

impl PadRow {
    /// A row of ten even keys, one for each of `chars`.
    pub fn chars(chars: &str) -> Self {
        Self {
            keys: chars.chars().map(PadKey::Char).collect(),
            class: "keypad-row",
        }
    }
}

/// The keypad's character rows, top to bottom: digits, then the letters as a
/// keyboard has them, with the punctuation a group can hold at the ends.
const PAD_ROWS: [&str; 4] = ["1234567890", "QWERTYUIOP", "ASDFGHJKL/", "ZXCVBNM,.?"];
/// What the bottom row holds before ⌫ and Next.
const PAD_LAST: [char; 2] = ['=', '+'];

/// The keys a group is answered with: every character a group can hold,
/// then ⌫ and Next.
fn group_rows() -> Vec<PadRow> {
    let mut rows: Vec<PadRow> = PAD_ROWS.into_iter().map(PadRow::chars).collect();
    let mut last: Vec<PadKey> = PAD_LAST.into_iter().map(PadKey::Char).collect();
    last.extend([PadKey::Back, PadKey::Next]);
    rows.push(PadRow {
        keys: last,
        class: "keypad-row keypad-last",
    });
    rows
}

/// What a character key is called: its id, and what a screen reader says.
fn pad_name(ch: char) -> (String, String) {
    let word = match ch {
        '/' => "slash",
        ',' => "comma",
        '.' => "period",
        '?' => "question mark",
        '=' => "equals",
        '+' => "plus",
        _ => return (control_id("key", &ch.to_string()), ch.to_string()),
    };
    (control_id("key", word), word.to_string())
}

/// What a key is called and shows: its id, what a screen reader says, what
/// it reads, and its class.
fn pad_face(key: &PadKey) -> (String, String, String, &'static str) {
    match key {
        PadKey::Char(ch) => {
            let (id, label) = pad_name(*ch);
            (id, label, ch.to_string(), "key group-text")
        }
        PadKey::Back => (
            control_id("key", "delete"),
            "Delete".into(),
            "⌫".into(),
            "key key-back",
        ),
        PadKey::Next => (
            control_id("key", "next"),
            "Next".into(),
            "Next".into(),
            "key key-next",
        ),
        PadKey::Field => (
            control_id("key", "field"),
            "Next field".into(),
            "␣ Field".into(),
            "key key-field",
        ),
        // Named for what it sends, aloud as on screen; the ↵ after it is
        // drawn apart, so a long label is cut short before it.
        PadKey::Send(label) => (
            control_id("key", "enter"),
            label.clone(),
            label.clone(),
            "key key-next key-send",
        ),
    }
}

/// Large keys, laid out in the `rows` the screen gives it, to answer or log
/// by touch.
///
/// The phone's keyboard stays away ([`ANSWER_INPUT_MODE`]), so the screen
/// keeps one layout for the whole session. A key takes no focus: a press
/// leaves the box focused, for a keyboard or the paddles to type on into.
///
/// A key types the moment a finger lands on it, the way a phone's own
/// keyboard does. Waiting for the click lost keys: a finger that slid a few
/// pixels as it tapped, as fingers do typing quickly, was taken for the
/// start of a scroll, and the browser sent no click at all. The keypad does
/// not let a scroll start on it either (`touch-action: none`). A press with
/// no finger behind it — a screen reader's — still comes as a click alone,
/// and the click a browser sends after a finger's press is that press, not
/// another.
///
/// Shown wherever touch is how you type: always in the phone and tablet
/// builds, and on the web and desktop whenever the device has a touch
/// screen. A desktop with only a mouse and keyboard has no use for it.
#[component]
pub(crate) fn Keypad(open: bool, rows: Vec<PadRow>, on_key: Callback<PadKey>) -> Element {
    let class = if TOUCH_BUILD {
        "keypad"
    } else {
        "keypad touch-only"
    };
    // Set by a press on the way down, so the click after it is not a second.
    let mut pressed_down = use_hook(|| CopyValue::new(false));
    let key = move |pressed: PadKey| {
        let (id, label, text, class) = pad_face(&pressed);
        let sends = matches!(pressed, PadKey::Send(_));
        let clicked = pressed.clone();
        rsx! {
            button {
                id,
                class,
                r#type: "button",
                aria_label: label,
                disabled: html_bool(!open),
                onpointerdown: move |e| {
                    // Nor does the focus move, or any mouse event follow
                    // to move it: the box keeps it.
                    e.prevent_default();
                    pressed_down.set(true);
                    on_key.call(pressed.clone());
                },
                onclick: move |_| {
                    let by_finger = *pressed_down.peek();
                    pressed_down.set(false);
                    if !by_finger {
                        on_key.call(clicked.clone());
                    }
                },
                if sends {
                    span { class: "key-label", "{text}" }
                    span { class: "key-return", "↵" }
                } else {
                    "{text}"
                }
            }
        }
    };
    rsx! {
        div { id: "keypad", class, role: "group", aria_label: "Keypad",
            for row in rows {
                div { class: row.class,
                    for pressed in row.keys {
                        {key(pressed)}
                    }
                }
            }
        }
    }
}

/// "▲ 27 WPM", or "▼ 27/21 WPM" under Farnsworth: the speed adaptive speed
/// chose, and which way it moved.
fn speed_label(speed: ChosenSpeed) -> String {
    let arrow = match speed.change {
        1 => "▲ ",
        -1 => "▼ ",
        _ => "",
    };
    if speed.char_wpm - speed.effective_wpm >= 0.5 {
        format!(
            "{arrow}{:.0}/{:.0} WPM",
            speed.char_wpm, speed.effective_wpm
        )
    } else {
        format!("{arrow}{:.0} WPM", speed.char_wpm)
    }
}

/// The open group, the one before it, the keypad, and the way out of the
/// session.
///
/// Only two groups are on screen, each in a slot of its own that does not
/// move: the one just answered above, dimmed, with how it went, and the open
/// one below it, with the box. The keypad takes the room under them that a
/// keyboard would have.
///
/// The answer box is local: [`AnswerDraft`] says when the session hears it.
/// Typing itself does not clone the session.
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
    /// Seconds left before the first group, while the session counts down.
    countdown: Option<u32>,
    /// The speed adaptive speed chose for the current group, if it chose.
    #[props(default)]
    speed: Option<ChosenSpeed>,
    /// Where the box's text waits on its way to the session.
    answer: AnswerOutbox,
    on_change: EventHandler<TypedAnswer>,
    on_confirm: EventHandler<()>,
    on_submit: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    let send_index = (repeat_done + 1).min(repeat_total.max(1));
    let current_done = confirmed.get(current).copied().unwrap_or(false);
    // The open box is never drawn from the session, so a view drawn afresh
    // on a session under way — after a hot reload, say — starts its draft
    // from whatever the session holds of the open answer.
    let draft = AnswerDraft::use_new(answer, on_change, || {
        let held = inputs.get(current).filter(|_| !current_done)?;
        (!held.is_empty()).then(|| (current, held.clone()))
    });
    let can_paddle = !playing && !locked && !current_done;
    let sent_now = groups.get(current).cloned().unwrap_or_default();
    // The paddles key into the box that is open, and only while it is: a
    // letter timed from its first contact, though it goes in only once its
    // letter space has run out.
    let keys = use_context::<PaddleKeys>();
    {
        let sent_now = sent_now.clone();
        keys.use_answer_box(can_paddle, move |letter, keyed| {
            draft.push(current, letter, &sent_now, keyed);
        });
    }
    // A letter keyed in full is in the answer when it closes, even with its
    // letter space still running out; and auto-confirm waits for a letter
    // still being keyed, as it would for a key still being typed.
    answer.use_entry(move || keys.finish_letter(), move || keys.letter_open());
    use_effect(use_reactive!(|current, current_done| {
        let _ = current;
        draft.settle(current_done);
        // A letter half keyed for one group is not the next one's.
        keys.stop();
    }));

    // The open slot shows the group in focus: the one being sent or
    // answered, or, between two groups, the one about to be sent.
    let open = focused;
    let open_done = confirmed.get(open).copied().unwrap_or(false);
    let is_active = focused_box_open(current, focused, &confirmed);
    let awaiting_play = !is_active && !open_done;
    let input_locked = (locked && is_active) || awaiting_play;
    let live = is_active && !input_locked;
    let placeholder = if countdown.is_some() {
        "Get ready…"
    } else if awaiting_play {
        "Waiting…"
    } else if input_locked {
        "Listening…"
    } else if live {
        "Type the group"
    } else {
        "Waiting…"
    };
    let previous = open
        .checked_sub(1)
        .filter(|&idx| confirmed.get(idx).copied().unwrap_or(false));
    let on_key = use_callback(move |pressed: PadKey| {
        let keyed = MonoMs(mono_ms());
        if !live {
            return;
        }
        match pressed {
            PadKey::Char(ch) => draft.push(current, ch, &sent_now, Some(keyed)),
            PadKey::Back => draft.pop(current, &sent_now),
            PadKey::Next => on_confirm.call(()),
            // A run's keys: the keys a group is answered with have neither.
            PadKey::Field | PadKey::Send(_) => {}
        }
    });
    // One short line. How to key with the paddles is the settings screen's to
    // say, beside the paddles, not something to read with a group to copy.
    // Each fits one line of a narrow phone, so the groups below never move
    // as it changes.
    let hint = if countdown.is_some() {
        "Get ready — the first group is coming."
    } else if playing && locked {
        "Listen — the box opens when it ends."
    } else if playing {
        "Listen, or type along."
    } else if live {
        "Type what you heard — no rush."
    } else {
        "Next group on its way."
    };
    rsx! {
        div { style: "display: contents;",
            div { class: "card",
                div { class: "train-head",
                    p { class: "muted train-hint", "{hint}" }
                    if let Some(speed) = speed {
                        span {
                            class: match speed.change {
                                1 => "chip speed-chip faster",
                                -1 => "chip speed-chip slower",
                                _ => "chip speed-chip",
                            },
                            title: "Adaptive speed",
                            {speed_label(speed)}
                        }
                    }
                    if repeat_total > 1 {
                        span { class: "chip",
                            Icon { name: IconName::Repeat }
                            "{repeat_total}× per group"
                        }
                    }
                }
                div { class: "train-groups",
                    // Held before the first answer too, so the open group
                    // below it never moves.
                    div { class: "group-slot",
                        if let Some(left) = countdown {
                            div { class: "group countdown", role: "status",
                                span { id: "countdown", class: "countdown-num", "{left}" }
                                span { class: "muted", "The first group starts in {left} s" }
                            }
                        } else if let Some(idx) = previous {
                            PreviousCard {
                                idx,
                                sent: groups.get(idx).cloned().unwrap_or_default(),
                                received: inputs.get(idx).cloned().unwrap_or_default(),
                            }
                        } else {
                            div { class: "group placeholder",
                                span { class: "muted", "Each group you answer is checked here." }
                            }
                        }
                    }
                    OpenCard {
                        idx: open,
                        sent: groups.get(open).cloned().unwrap_or_default(),
                        live,
                        send_badge: (playing && repeat_total > 1)
                            .then(|| format!("Send {send_index}/{repeat_total}")),
                        placeholder,
                        draft,
                        on_confirm,
                    }
                }
            }
            Keypad { open: live, rows: group_rows(), on_key }
            div { class: "train-actions",
                button {
                    id: control_id("btn", "end session"),
                    class: "btn btn-primary",
                    onclick: move |_| on_submit.call(()),
                    Icon { name: IconName::Flag }
                    "End session"
                }
                button {
                    id: control_id("btn", "discard"),
                    class: "btn btn-danger",
                    onclick: move |_| on_stop.call(()),
                    Icon { name: IconName::X }
                    "Discard"
                }
            }
        }
    }
}

#[cfg(test)]
mod speed_tests {
    use super::{ChosenSpeed, speed_label};

    #[test]
    fn the_chosen_speed_says_which_way_it_moved() {
        let speed = |char_wpm, effective_wpm, change| ChosenSpeed {
            char_wpm,
            effective_wpm,
            change,
        };
        assert_eq!(speed_label(speed(27.0, 27.0, 1)), "▲ 27 WPM");
        assert_eq!(speed_label(speed(24.4, 24.4, -1)), "▼ 24 WPM");
        assert_eq!(speed_label(speed(20.0, 20.0, 0)), "20 WPM");
        assert_eq!(speed_label(speed(25.0, 15.0, 0)), "25/15 WPM", "Farnsworth");
    }
}

#[cfg(test)]
mod tests {
    use super::{PAD_LAST, PAD_ROWS, pad_name};
    use crate::testing::{Ui, run, test_settings};
    use cw_core::CharSetMode;

    /// Every character a group can be sent in has a key, once, and every key
    /// a name of its own.
    #[test]
    fn the_keypad_has_a_key_for_every_character_and_one_only() {
        let keys: Vec<char> = PAD_ROWS
            .iter()
            .flat_map(|row| row.chars())
            .chain(PAD_LAST)
            .collect();
        for ch in ('A'..='Z').chain('0'..='9').chain("/=+?,.".chars()) {
            assert!(cw_core::morse_for(ch).is_some(), "{ch} is not sendable");
            assert_eq!(keys.iter().filter(|&&key| key == ch).count(), 1, "{ch}");
        }
        assert!(keys.iter().all(|&key| cw_core::morse_for(key).is_some()));
        let mut ids: Vec<String> = keys.iter().map(|&key| pad_name(key).0).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), keys.len(), "{ids:?}");
        assert!(ids.contains(&"key-slash".to_string()), "{ids:?}");
        assert!(ids.contains(&"key-a".to_string()), "{ids:?}");
    }

    /// Two groups on screen at most, in their own slots, then the keypad,
    /// then the session's buttons.
    #[test]
    fn only_the_open_group_and_the_one_before_it_are_shown() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 3;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            let html = ui.html();
            assert!(html.contains(r#"id="group-card-0""#), "{html}");
            assert!(!html.contains(r#"id="group-card-1""#), "{html}");
            assert!(!html.contains(r#"id="group-card-2""#), "{html}");
            assert!(ui.has("Each group you answer is checked here."));
            let at = |needle: &str| {
                html.find(needle)
                    .unwrap_or_else(|| panic!("no {needle} in {html}"))
            };
            assert!(at(r#"id="group-input-0""#) < at(r#"id="key-q""#), "{html}");
            assert!(
                at(r#"id="key-next""#) < at(r#"id="btn-end-session""#),
                "{html}"
            );
            assert_eq!(html.matches("inputmode=\"none\"").count(), 1, "{html}");
        });
    }

    /// A session is drawn as the stylesheet's rule for short phones names it
    /// (`styles_tests.rs`): its root is a `.train`, and the group answered a
    /// `.group.done` with what was sent in its top row beside the mark, then
    /// its comparison, the sent group over the copy, a box for every
    /// character typed past the end. A class renamed, or a wrapper put in,
    /// would leave the keypad's last row under the foot of a 360×640 phone.
    #[test]
    fn a_session_is_drawn_as_the_rule_for_short_phones_names_it() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-start-training");
            let your_turn = r#"class="progress-status">Your turn<"#;
            assert!(ui.run_until(5_000, |ui| ui.has(your_turn)).await);
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &format!("{sent}XY"));
            ui.press_enter("group-input-0");
            let html = ui.html();
            assert!(html.contains(r#"<div class="stack train">"#), "{html}");
            let top_row = format!(
                r#"<div id="group-card-0" class="group done"><div class="row-between"><span class="badge">1</span><div class="row" style="gap: 0.4rem;"><span class="sent">{sent}</span>"#
            );
            assert!(html.contains(&top_row), "{html}");
            let sent_row: String = sent
                .chars()
                .map(|ch| format!(r#"<span class="ch sent-row">{ch}</span>"#))
                .collect();
            let comparison = format!(
                r#"<div class="stack-sm comparison" style="gap: 3px;"><div class="chars">{sent_row}</div><div class="chars">"#
            );
            assert!(html.contains(&comparison), "{html}");
            let copy = html[html.find(&comparison).expect("the comparison") + comparison.len()..]
                .split("</div>")
                .next()
                .unwrap_or_default();
            assert_eq!(
                copy.matches("<span").count(),
                sent.chars().count() + 2,
                "{copy}"
            );
        });
    }

    /// In every group mode, with no pause after a group or one, Enter leaves
    /// the header saying the next group is coming, beside the plain dot, for
    /// as long as the box waits for it; "Your turn" comes back with the green
    /// dot once the next group's box opens.
    #[test]
    fn between_groups_the_header_waits_with_the_box() {
        const YOUR_TURN: &str = r#"class="progress-status">Your turn<"#;
        const NEXT_GROUP: &str = r#"class="progress-status">Next group…<"#;
        const READY: &str = r#"class="status-dot ready""#;
        const PLAIN: &str = r#"class="status-dot""#;
        for mode in [
            CharSetMode::Koch,
            CharSetMode::Digits,
            CharSetMode::Mixed,
            CharSetMode::Callsign,
            CharSetMode::Words,
        ] {
            for pause in [0.0, 2.0] {
                run(|| async move {
                    let case = format!("{mode:?}, a pause of {pause} s");
                    let mut settings = test_settings();
                    settings.curriculum.char_set_mode = mode;
                    settings.playback.group_pause_sec = pause;
                    let (mut ui, recorder) = Ui::app_with_settings(settings);
                    ui.click("btn-start-training");
                    assert!(ui.run_until(5_000, |ui| ui.has(YOUR_TURN)).await, "{case}");
                    assert!(ui.has(READY), "{case}");
                    let sent = recorder.texts().first().cloned().expect("a group was sent");
                    ui.type_into("group-input-0", &sent);
                    ui.press_enter("group-input-0");
                    let between = |ui: &Ui| {
                        ui.has(NEXT_GROUP)
                            && ui.has(PLAIN)
                            && !ui.has(YOUR_TURN)
                            && !ui.has(READY)
                            && ui.input_tag("group-input-1").contains("Waiting…")
                    };
                    assert!(between(&ui), "{case}: {}", ui.html());
                    if pause > 0.0 {
                        ui.advance(1_000).await;
                        assert!(between(&ui), "{case}: {}", ui.html());
                    }
                    assert!(ui.run_until(5_000, |ui| ui.has(YOUR_TURN)).await, "{case}");
                    assert!(ui.has(READY), "{case}");
                    assert!(!ui.has(NEXT_GROUP), "{case}");
                    assert!(
                        ui.input_tag("group-input-1").contains("Type the group"),
                        "{case}"
                    );
                });
            }
        }
    }
}
