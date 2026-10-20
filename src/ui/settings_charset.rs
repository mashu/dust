use cw_core::{
    CALLSIGN_TIER_MAX, CharSetMode, GROUP_SIZE_MAX, LevelAxis, MAX_DIGITS_LEVEL, PracticeWindow,
    SEQUENCE_PRESETS, STARTER_WORDS, SequenceChoice, SettingsSection, TrainingSettings,
    WORD_LIST_MAX, WordList, compute_char_pool, current_practice_window, tier_examples,
    unlocked_practice_count,
};
use dioxus::prelude::*;

use crate::ui::file_pick::{ChosenFile, FileButton};
use crate::ui::format::counted;
use crate::ui::widgets::{
    CardHead, DISCLOSURE, Icon, IconName, NumberField, SectionReset, Seg, control_id,
};

/// The largest file read as a word list. The longest list kept is a few
/// tens of kilobytes; anything near this is not a word list.
const WORD_FILE_MAX_BYTES: u64 = 1_000_000;

/// A list of characters, typed out.
///
/// What is typed stays as typed until it is committed — on Enter, or on
/// leaving the field — and only then is it cleaned down to the characters
/// that can be sent. Cleaned on every key, the box would move under the
/// cursor whenever a key was dropped, and could never be emptied to start
/// again: an empty unlock order is LCWO's, so clearing it would fill it
/// straight back up.
#[component]
fn AlphabetField(label: &'static str, value: String, on_commit: EventHandler<String>) -> Element {
    let mut typed = use_signal(|| None::<String>);
    let shown = typed().unwrap_or(value);
    let id = control_id("field", label);
    rsx! {
        div { class: "field",
            label { r#for: "{id}", "{label}" }
            input {
                id,
                class: "group-text",
                value: "{shown}",
                autocapitalize: "characters",
                spellcheck: false,
                oninput: move |e| typed.set(Some(e.value())),
                onchange: move |e| {
                    typed.set(None);
                    on_commit.call(e.value());
                },
            }
        }
    }
}

/// The character sets, in the order the control offers them.
pub(crate) const CHAR_SET_MODES: [(CharSetMode, &str); 5] = [
    (CharSetMode::Koch, "Koch"),
    (CharSetMode::Digits, "Digits"),
    (CharSetMode::Mixed, "Mixed"),
    (CharSetMode::Callsign, "Callsigns"),
    (CharSetMode::Words, "Words"),
];

/// What a character set is called on screen: "Callsigns". Contest has its
/// name before the control offers it, for the runs a later version kept.
pub(crate) fn mode_label(mode: CharSetMode) -> &'static str {
    match mode {
        CharSetMode::Contest => "Contest",
        offered => CHAR_SET_MODES
            .iter()
            .find(|(each, _)| *each == offered)
            .map(|(_, label)| *label)
            .unwrap_or_default(),
    }
}

/// The practice windows, and how many characters each needs unlocked before
/// it is offered.
const PRACTICE_WINDOWS: [(PracticeWindow, &str, usize); 3] = [
    (PracticeWindow::All, "Everything", 0),
    (PracticeWindow::Last3, "Newest 3", 3),
    (PracticeWindow::Last5, "Newest 5", 5),
];

/// What the character-set card shows, worked out from the settings. A memo
/// of this is what the card reads, so it redraws when one of these moves and
/// not for a slider on another card.
#[derive(Clone, PartialEq)]
struct Charset {
    mode: CharSetMode,
    callsigns: bool,
    sequence: String,
    choice: SequenceChoice,
    level: u32,
    level_max: u32,
    digits_level: u32,
    letters_percent: u32,
    unlocked: usize,
    window: PracticeWindow,
    pool: Vec<char>,
    tier_examples: Vec<String>,
    /// The words Words sends, and whether they are the learner's own.
    words: Vec<String>,
    own_words: bool,
}

impl Charset {
    fn of(settings: &TrainingSettings) -> Self {
        let c = &settings.curriculum;
        let callsigns = c.char_set_mode.sends_callsigns();
        Self {
            mode: c.char_set_mode,
            callsigns,
            sequence: settings.sequence().iter().collect(),
            choice: settings.sequence_choice(),
            level: settings.active_level(),
            level_max: settings.max_active_level(),
            digits_level: settings.level(LevelAxis::Digits),
            letters_percent: c.mixed_letters_percent,
            unlocked: unlocked_practice_count(settings),
            window: current_practice_window(settings),
            pool: compute_char_pool(settings),
            tier_examples: if callsigns {
                tier_examples(settings.active_level())
            } else {
                Vec::new()
            },
            words: settings.words().into_iter().map(str::to_string).collect(),
            own_words: settings.has_own_words(),
        }
    }

    fn note(&self) -> String {
        let what = match self.mode {
            CharSetMode::Koch => match self.choice {
                SequenceChoice::Preset(id) => format!("Unlocked in {} order", id.preset().name),
                SequenceChoice::Custom => "Unlocked in your own order".to_string(),
            },
            CharSetMode::Digits => "Digits only".to_string(),
            CharSetMode::Mixed => "Letters and digits together".to_string(),
            // Contest is not offered yet; until it has a card of its own, its
            // callers' calls are what it shares with Callsigns.
            CharSetMode::Callsign | CharSetMode::Contest => {
                "Real callsigns, off the air".to_string()
            }
            CharSetMode::Words if self.own_words => "Your own words, sent whole".to_string(),
            CharSetMode::Words => "Starter words, sent whole".to_string(),
        };
        if self.callsigns {
            format!("{what} · tier {} of {CALLSIGN_TIER_MAX}", self.level)
        } else if self.mode == CharSetMode::Words {
            format!("{what} · {}", count_words(self.words.len()))
        } else {
            format!("{what} · {} in play", self.unlocked)
        }
    }
}

#[component]
pub fn CharsetCard(settings: Signal<TrainingSettings>) -> Element {
    let charset = use_memo(move || Charset::of(&settings.read()));
    let c = charset.read();
    let callsigns = c.callsigns;
    let words = c.mode == CharSetMode::Words;
    let has_unlock_order = matches!(c.mode, CharSetMode::Koch | CharSetMode::Mixed);
    rsx! {
        div { class: "card stack-sm",
            CardHead { icon: IconName::Letters, title: "Character set", note: c.note(),
                SectionReset {
                    label: "character set",
                    on_reset: move |()| {
                        settings.write().reset_section(SettingsSection::CharacterSet);
                    },
                }
            }
            div { class: "segmented",
                for (mode, label) in CHAR_SET_MODES {
                    Seg {
                        label,
                        active: c.mode == mode,
                        onclick: move |_| settings.write().set_char_set_mode(mode),
                    }
                }
            }
            if has_unlock_order {
                div { class: "eyebrow", "Unlock order" }
                div { class: "mode-pills",
                    for choice in SEQUENCE_PRESETS
                        .iter()
                        .map(|preset| (SequenceChoice::Preset(preset.id), preset.name))
                        .chain([(SequenceChoice::Custom, "Your own")])
                    {
                        Seg {
                            label: choice.1,
                            pill: true,
                            active: c.choice == choice.0,
                            onclick: move |_| settings.write().choose_sequence(choice.0),
                        }
                    }
                }
                AlphabetField {
                    label: "Sequence order",
                    value: c.sequence.clone(),
                    on_commit: move |typed: String| settings.write().type_sequence(&typed),
                }
            }
            // A word list has no level: every word is in from the start.
            if !words {
            div { class: "field-grid",
                NumberField {
                    label: if callsigns {
                        format!("Callsign tier (1–{})", c.level_max)
                    } else {
                        format!("Level (1–{})", c.level_max)
                    },
                    id: "field-level",
                    value: f64::from(c.level),
                    min: 1.0,
                    max: f64::from(c.level_max),
                    step: 1.0,
                    onchange: move |v| settings.write().set_active_level(v as u32),
                }
                if c.mode == CharSetMode::Mixed {
                    NumberField {
                        label: format!("Digits level (1–{MAX_DIGITS_LEVEL})"),
                        id: "field-digits-level",
                        value: f64::from(c.digits_level),
                        min: 1.0,
                        max: f64::from(MAX_DIGITS_LEVEL),
                        step: 1.0,
                        onchange: move |v| settings.write().curriculum.digits_level = v as u32,
                    }
                }
            }
            }
            if c.mode == CharSetMode::Mixed {
                NumberField {
                    label: "Share of letters",
                    value: f64::from(c.letters_percent),
                    min: 0.0,
                    max: 100.0,
                    step: 5.0,
                    unit: "%",
                    onchange: move |v| settings.write().curriculum.mixed_letters_percent = v as u32,
                }
            }
            if words {
                WordListEditor {
                    settings,
                    words: c.words.clone(),
                    own: c.own_words,
                }
            } else if callsigns {
                div { class: "eyebrow", "What this tier sends" }
                div { class: "calls",
                    for call in c.tier_examples.iter() {
                        span { class: "call", "{call}" }
                    }
                    // A few of endless calls, not the lot.
                    span { class: "muted", aria_label: "and more", "…" }
                }
                p { class: "muted", style: "margin: 0;",
                    "Every tier keeps what the ones below it send, so moving up only adds. Prefixes are real allocations, weighted the way you hear them."
                }
            } else {
                div { class: "eyebrow", "Practice window" }
                div { class: "mode-pills",
                    for (window, label, needs) in PRACTICE_WINDOWS {
                        if c.unlocked >= needs {
                            Seg {
                                label,
                                pill: true,
                                active: c.window == window,
                                onclick: move |_| settings.write().curriculum.practice_window = window,
                            }
                        }
                    }
                }
            }
            // The characters groups are drawn from, one at a time. A word or
            // a callsign is sent whole, never a character of it on its own,
            // so for them the characters would say nothing.
            if !words && !callsigns {
                div { class: "chars", style: "gap: 4px;",
                    for ch in c.pool.iter() {
                        span { class: "ch sent-row", "{ch}" }
                    }
                }
            }
        }
    }
}

/// "1 word", "412 words".
fn count_words(count: usize) -> String {
    counted(count, "word")
}

/// What reading a list came to, in a line: how many words it holds, and why
/// any were left out. `source` names the file it came from, if one did.
fn list_report(list: &WordList, source: Option<&str>) -> String {
    let mut parts = vec![count_words(list.words.len())];
    if list.unsendable > 0 {
        parts.push(format!(
            "{} left out: a character Morse has no code for, or longer than {GROUP_SIZE_MAX}",
            list.unsendable
        ));
    }
    if list.repeated > 0 {
        parts.push(format!("{} repeated", list.repeated));
    }
    if list.over > 0 {
        parts.push(format!(
            "{} past the {WORD_LIST_MAX} a list keeps",
            list.over
        ));
    }
    let report = parts.join(" · ");
    match source {
        Some(name) => format!("{name}: {report}"),
        None => report,
    }
}

/// Make `list` the words, and say what reading it came to. A list that kept
/// nothing leaves the words as they were, unless it was meant to be empty:
/// emptying the box goes back to the starter words.
fn take_list(
    mut settings: Signal<TrainingSettings>,
    mut report: Signal<Option<String>>,
    text: &str,
    source: Option<&str>,
) {
    let list = WordList::read(text);
    if list.words.is_empty() && list.left_out() == 0 {
        settings.write().curriculum.word_list.clear();
        report.set(Some("Back to the starter words.".to_string()));
        return;
    }
    if list.words.is_empty() {
        let what = source.unwrap_or("The list");
        report.set(Some(format!(
            "{what}: no word in it can be sent, so the words are as they were."
        )));
        return;
    }
    report.set(Some(list_report(&list, source)));
    settings.write().curriculum.word_list = list.words;
}

/// Take a chosen file as the word list.
fn take_word_file(
    settings: Signal<TrainingSettings>,
    mut report: Signal<Option<String>>,
    file: ChosenFile,
) {
    let ChosenFile { name, size, text } = file;
    if size > WORD_FILE_MAX_BYTES {
        report.set(Some(format!(
            "{name} is over a megabyte, far more than a word list: nothing was read."
        )));
        return;
    }
    match text {
        Some(text) => take_list(settings, report, &text, Some(&name)),
        None => report.set(Some(format!(
            "{name} could not be read as text: is it a plain .txt file?"
        ))),
    }
}

/// The words Words sends: typed or pasted in, a word to a line, or loaded
/// from a text file.
///
/// What is typed stays as typed until it is committed, on leaving the box,
/// and is then read as a list: upper-cased, each word once, what cannot be
/// sent left out and said. A key does not redraw the box — on the desktop
/// and phone builds a redraw reaches it a frame late, over what was typed
/// since — so the count of what is being typed is drawn beside it instead.
#[component]
fn WordListEditor(settings: Signal<TrainingSettings>, words: Vec<String>, own: bool) -> Element {
    // The text being typed, until it is committed. Only the count reads it.
    let mut draft = use_signal(|| None::<String>);
    let report = use_signal(|| None::<String>);
    // Moved on by every commit, so the box shows the list again afterwards
    // even when the list did not change: text that kept nothing goes.
    let mut repaint = use_signal(|| 0u64);
    let _ = repaint.read();
    let mut settle = move || {
        draft.set(None);
        repaint += 1;
    };
    // The starter list, unfolded.
    let mut show_list = use_signal(|| false);
    let shown = draft
        .peek()
        .clone()
        .unwrap_or_else(|| if own { words.join("\n") } else { String::new() });
    rsx! {
        div { class: "eyebrow", "Your words" }
        textarea {
            id: control_id("field", "word list"),
            class: "group-text word-list",
            rows: "7",
            value: "{shown}",
            placeholder: "Paste or type your words here, one to a line",
            aria_label: "Your words, one to a line",
            autocapitalize: "characters",
            spellcheck: false,
            oninput: move |e| draft.set(Some(e.value())),
            onchange: move |e| {
                settle();
                take_list(settings, report, &e.value(), None);
            },
        }
        div { class: "word-tools",
            FileButton {
                id: control_id("file", "word list"),
                accept: ".txt,text/plain",
                max_bytes: WORD_FILE_MAX_BYTES,
                icon: IconName::Upload,
                label: "Load a text file",
                on_choose: move |file| {
                    settle();
                    take_word_file(settings, report, file);
                },
            }
            if own {
                button {
                    id: control_id("btn", "starter words"),
                    class: "btn btn-secondary btn-sm",
                    r#type: "button",
                    title: "Send the starter words instead of yours",
                    onclick: move |_| {
                        settle();
                        take_list(settings, report, "", None);
                    },
                    Icon { name: IconName::Reset }
                    "Starter words"
                }
            }
        }
        WordCount { draft, report, count: words.len(), own }
        if !own {
            p { class: "muted", style: "margin: 0;",
                "Until you add your own, Words sends a starter list of the abbreviations and Q-codes a contact is made of."
            }
            // How many words is the summary; the list itself is there to
            // look through when wanted, folded away until then.
            button {
                id: control_id(DISCLOSURE, "starter list"),
                class: if show_list() { "link-toggle open" } else { "link-toggle" },
                style: "align-self: flex-start;",
                aria_expanded: if show_list() { "true" } else { "false" },
                aria_controls: "starter-list",
                onclick: move |_| show_list.toggle(),
                "{count_words(STARTER_WORDS.len())} to draw from"
                Icon { name: IconName::Chevron }
            }
            if show_list() {
                div { id: "starter-list", class: "calls",
                    for word in STARTER_WORDS.iter() {
                        span { class: "call", "{word}" }
                    }
                }
            }
        }
    }
}

/// The line under the word box: what is being typed comes to, while it is,
/// and otherwise what the last list read came to, or how many words there
/// are.
#[component]
fn WordCount(
    draft: Signal<Option<String>>,
    report: Signal<Option<String>>,
    count: usize,
    own: bool,
) -> Element {
    let line = match draft() {
        Some(text) => format!(
            "{} — taken when you leave the box",
            list_report(&WordList::read(&text), None)
        ),
        None => report().unwrap_or_else(|| {
            if own {
                count_words(count)
            } else {
                String::new()
            }
        }),
    };
    rsx! {
        p { class: "word-report", aria_live: "polite", "{line}" }
    }
}
