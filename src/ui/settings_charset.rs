use cw_core::{
    CALLSIGN_TIER_MAX, CharSetMode, ContestKind, ContestPreset, Fill, GROUP_SIZE_MAX, LevelAxis,
    MAX_DIGITS_LEVEL, PracticeWindow, RunStyle, SEQUENCE_PRESETS, STARTER_WORDS, Sender,
    SequenceChoice, SettingsSection, TrainingSettings, TuCall, WORD_LIST_MAX, WordList,
    compute_char_pool, current_practice_window, parse_callsign, tier_examples,
    unlocked_practice_count,
};
use dioxus::prelude::*;

use crate::playback::Dialogue;
use crate::ui::band::habits_label;
use crate::ui::file_pick::{ChosenFile, FileButton};
use crate::ui::format::counted;
use crate::ui::hear_qso::{HeardDialogue, QsoHead, QsoLines, heard_label};
use crate::ui::home::range_label;
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
pub(crate) const CHAR_SET_MODES: [(CharSetMode, &str); 6] = [
    (CharSetMode::Koch, "Koch"),
    (CharSetMode::Digits, "Digits"),
    (CharSetMode::Mixed, "Mixed"),
    (CharSetMode::Callsign, "Callsigns"),
    (CharSetMode::Words, "Words"),
    (CharSetMode::Contest, "Contest"),
];

/// What a character set is called on screen: "Callsigns".
pub(crate) fn mode_label(mode: CharSetMode) -> &'static str {
    CHAR_SET_MODES
        .iter()
        .find(|(each, _)| *each == mode)
        .map(|(_, label)| *label)
        .unwrap_or_default()
}

/// The run styles, in the order the card offers them.
const RUN_STYLES: [(RunStyle, &str); 2] = [
    (RunStyle::Single, "Single caller"),
    (RunStyle::Pileup, "Pile-up"),
];

/// How often your TU signs with your call, in the order the card offers
/// it.
const TU_CALLS: [(TuCall, &str); 3] = [
    (TuCall::Every, "Every QSO"),
    (TuCall::Third, "Every third"),
    (TuCall::Never, "Never"),
];

/// How many call at once in a pile-up of `settings`, from the Band card's
/// stations range, and the noun that counts them: "1–3" callers, "1" caller.
pub(crate) fn pileup_count(settings: &TrainingSettings) -> (String, &'static str) {
    let band = &settings.band;
    let count = range_label(f64::from(band.stations_min), f64::from(band.stations_max));
    let noun = if band.stations_max == 1 {
        "caller"
    } else {
        "callers"
    };
    (count, noun)
}

/// The same in words: "1–3 callers", "3 callers", "1 caller".
pub(crate) fn pileup_callers(settings: &TrainingSettings) -> String {
    let (count, noun) = pileup_count(settings);
    format!("{count} {noun}")
}

/// The practice windows, and how many characters each needs unlocked before
/// it is offered.
const PRACTICE_WINDOWS: [(PracticeWindow, &str, usize); 3] = [
    (PracticeWindow::All, "Everything", 0),
    (PracticeWindow::Last3, "Newest 3", 3),
    (PracticeWindow::Last5, "Newest 5", 5),
];

/// Your call where an example of a QSO has it, or a stand-in until you
/// give one a contest can read.
fn call_or_stand_in(my_call: &str) -> &str {
    if parse_callsign(my_call).is_some() {
        my_call
    } else {
        "<your call>"
    }
}

/// Your name, state or section where an example of a QSO sends it: what
/// you gave or your call tells (DX for a call outside the US and Canada),
/// or `stand_in` until then.
fn or_stand_in<'a>(sent: &'a str, stand_in: &'a str) -> &'a str {
    if sent.is_empty() { stand_in } else { sent }
}

/// Your CQ zone where an example of a QSO sends it: the one you gave or
/// your call reads as, or a stand-in until one is known.
fn zone_or_stand_in(zone: Option<u32>) -> String {
    zone.map_or_else(|| "<your zone>".to_string(), |zone| zone.to_string())
}

/// What the card shows in Contest mode: which contest, what its exchange
/// is, and a QSO of it as it goes, with your call in it.
#[derive(Clone, PartialEq)]
struct ContestCard {
    kind: ContestKind,
    /// One caller at a time, or a pile-up, and how many call in one.
    style: RunStyle,
    callers: String,
    /// How often your TU signs with your call, where the contest's TU
    /// leaves it to you; none where its rules decide.
    tu_call: Option<TuCall>,
    /// What you send: `5NN + serial`.
    sends: String,
    /// What you copy, as the log heads it: `NR`.
    copies: String,
    /// The side you are on and who you work from it, where sides change
    /// it.
    side_note: Option<String>,
    /// What Dust leaves out of the real contest, if anything.
    simplified: Option<&'static str>,
    /// Who sends each line of the QSO, and what.
    qso: Vec<(Sender, String)>,
    /// The preset the settings are on, if any.
    preset: Option<ContestPreset>,
}

impl ContestCard {
    /// The card for the contest `settings` choose.
    fn of(settings: &TrainingSettings) -> Self {
        let contest = &settings.contest;
        let kind = contest.contest_kind;
        let side = contest.side();
        let style = contest.contest_style;
        let callers = pileup_callers(settings);
        let preset = ContestPreset::of(settings);
        let rules = kind.rules();
        let copies: Vec<&str> = rules
            .copied(side)
            .iter()
            .map(|field| field.label())
            .collect();
        let zone = zone_or_stand_in(contest.zone());
        Self {
            kind,
            style,
            callers,
            tu_call: rules.tu_call_chosen().then_some(contest.contest_tu_call),
            sends: rules.sends(side),
            copies: copies.join("\u{a0}"),
            side_note: rules.side_note(side, parse_callsign(&contest.my_call).is_some()),
            simplified: rules.simplified,
            qso: rules.example_qso(
                side,
                &Fill {
                    my_call: call_or_stand_in(&contest.my_call),
                    my_name: or_stand_in(&contest.my_name, "<your name>"),
                    my_state: or_stand_in(contest.state_sent(), "<your state>"),
                    my_zone: &zone,
                    my_power: &contest.my_power,
                    my_class: &contest.my_class,
                    my_section: or_stand_in(contest.section_sent(), "<your section>"),
                    // The QSO is your first: its TU signs as a run's
                    // first would, with your call in TU every QSO.
                    tu_signed: rules.tu_adds_call(style, contest.contest_tu_call, 1),
                    ..Fill::default()
                },
            ),
            preset,
        }
    }
}

/// What the character-set card shows, worked out from the settings. A memo
/// of this is what the card reads, so it redraws when one of these moves and
/// not for a slider on another card.
#[derive(Clone, PartialEq)]
struct Charset {
    mode: CharSetMode,
    /// In Contest mode, the contest; none in any other.
    contest: Option<ContestCard>,
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
    /// A few calls of the tier, as Callsigns shows them; none in Contest,
    /// whose QSO shows a call.
    tier_examples: Vec<String>,
    /// The words Words sends, and whether they are the learner's own.
    words: Vec<String>,
    own_words: bool,
}

impl Charset {
    fn of(settings: &TrainingSettings) -> Self {
        let c = &settings.curriculum;
        let callsigns = c.char_set_mode.sends_callsigns();
        let contest = (c.char_set_mode == CharSetMode::Contest).then(|| ContestCard::of(settings));
        let tier_examples = if callsigns && contest.is_none() {
            tier_examples(settings.active_level())
        } else {
            Vec::new()
        };
        Self {
            mode: c.char_set_mode,
            contest,
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
            tier_examples,
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
            CharSetMode::Callsign => "Real callsigns, off the air".to_string(),
            // A contest is told by its name; its tier is the stepper's.
            CharSetMode::Contest => {
                let kind = self.contest.as_ref().map(|card| card.kind);
                return format!("Contest · {}", kind.unwrap_or_default().label());
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

/// What a preset sets, in a line: "15–18 WPM · tier 1 calls · one caller
/// at a time · easy habits · no fading or static", the callers' habits and
/// the band in the Band card's own words.
fn preset_summary(preset: ContestPreset) -> String {
    let (slowest, fastest) = preset.char_wpm();
    let (fewest, most) = preset.stations();
    let callers = match preset.style() {
        RunStyle::Single => "one caller at a time".to_string(),
        RunStyle::Pileup => format!(
            "{} callers",
            range_label(f64::from(fewest), f64::from(most))
        ),
    };
    let band = match preset {
        ContestPreset::FirstQso => "no fading or static",
        ContestPreset::Club => "QSB fading and QRN static",
        ContestPreset::Contester => "deep fading, strong static",
    };
    let parts = [
        format!("{} WPM", range_label(slowest, fastest)),
        format!("tier {} calls", preset.tier()),
        callers,
        format!("{} habits", habits_label(preset.habits()).to_lowercase()),
        band.to_string(),
    ];
    // One piece each, and the dot at the end of a line, not the start of
    // the next, where a narrow line breaks.
    let parts: Vec<String> = parts
        .iter()
        .map(|part| part.replace(' ', "\u{a0}"))
        .collect();
    parts.join("\u{a0}· ")
}

/// The character set card. In Contest mode it also plays Hear a QSO:
/// `hearing` while it plays, `dialogue` what it has played, `on_hear` to
/// start it and `on_stop` to stop it.
#[component]
pub fn CharsetCard(
    settings: Signal<TrainingSettings>,
    #[props(default)] hearing: bool,
    #[props(default)] dialogue: Option<Dialogue>,
    #[props(default)] on_hear: EventHandler<()>,
    #[props(default)] on_stop: EventHandler<()>,
) -> Element {
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
            div { class: "segmented charset-modes",
                for (mode, label) in CHAR_SET_MODES {
                    Seg {
                        label,
                        active: c.mode == mode,
                        onclick: move |_| settings.write().set_char_set_mode(mode),
                    }
                }
            }
            if let Some(contest) = c.contest.clone() {
                ContestBody {
                    settings,
                    contest,
                    hearing,
                    dialogue,
                    on_hear,
                    on_stop,
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
            if let Some(contest) = c.contest.clone() {
                RunStylePicker { settings, style: contest.style, callers: contest.callers }
                // A single caller's TU never signs, as MorseRunner's single
                // calls never do.
                if let (RunStyle::Pileup, Some(tu_call)) = (contest.style, contest.tu_call) {
                    TuCallPicker { settings, tu_call }
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
            } else if !c.tier_examples.is_empty() {
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
            } else if !callsigns {
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

/// A contest in place of an alphabet: a pill for each contest, what you send
/// and copy, and a first QSO of it, line by line, with a few to hear (Hear a
/// QSO); then where to start from, a preset.
#[component]
fn ContestBody(
    settings: Signal<TrainingSettings>,
    contest: ContestCard,
    hearing: bool,
    dialogue: Option<Dialogue>,
    on_hear: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    // What Hear a QSO played, while it is of the contest chosen now: kept
    // once it ends, but not when it stopped before a line was heard.
    let heard = dialogue
        .filter(|dialogue| dialogue.kind == contest.kind)
        .filter(|dialogue| hearing || !dialogue.lines.is_empty());
    let eyebrow = heard
        .as_ref()
        .map_or_else(|| "A QSO".to_string(), |heard| heard_label(heard, hearing));
    rsx! {
        div { class: "eyebrow", "Contest" }
        div { class: "mode-pills",
            for kind in ContestKind::ALL {
                Seg {
                    label: kind.label(),
                    id: control_id("contest", kind.label()),
                    pill: true,
                    active: contest.kind == kind,
                    onclick: move |_| settings.write().contest.contest_kind = kind,
                }
            }
        }
        // What you copy keeps to one piece, after the dot where a narrow
        // line breaks.
        p { class: "muted", style: "margin: 0;",
            "You send {contest.sends}\u{a0}· You\u{a0}copy\u{a0}{contest.copies}"
        }
        if let Some(side_note) = &contest.side_note {
            p { id: "side-line", class: "muted", style: "margin: 0;", "{side_note}" }
        }
        if let Some(simplified) = contest.simplified {
            p { class: "muted", style: "margin: 0;", "{simplified}" }
        }
        QsoHead { label: eyebrow,
            if hearing {
                button {
                    id: control_id("btn", "stop hearing"),
                    class: "btn btn-secondary btn-sm",
                    r#type: "button",
                    onclick: move |_| on_stop.call(()),
                    Icon { name: IconName::Stop }
                    "Stop"
                }
            } else {
                button {
                    id: control_id("btn", "hear a qso"),
                    class: "btn btn-secondary btn-sm",
                    r#type: "button",
                    onclick: move |_| on_hear.call(()),
                    Icon { name: IconName::Signal }
                    "Hear a QSO"
                }
            }
        }
        if let Some(heard) = heard {
            HeardDialogue { dialogue: heard, hearing }
        } else {
            QsoLines { lines: contest.qso.clone() }
        }
        PresetPicker { settings, preset: contest.preset }
    }
}

/// Where a run starts from: a preset for a first QSO, a club operator or a
/// contester, each setting the callers' speed, calls and habits, the run
/// style and how many call, and the band. The settings read Custom once
/// any of that is changed, as the Experience presets do.
#[component]
fn PresetPicker(settings: Signal<TrainingSettings>, preset: Option<ContestPreset>) -> Element {
    let line = match preset {
        Some(preset) => preset_summary(preset),
        None => "Custom · a preset sets the callers' speed, calls and habits, how many call, and the band"
            .to_string(),
    };
    rsx! {
        div { class: "eyebrow", "Preset" }
        div { class: "mode-pills",
            for each in ContestPreset::ALL {
                Seg {
                    label: each.name(),
                    id: control_id("preset", each.name()),
                    pill: true,
                    active: preset == Some(each),
                    onclick: move |_| each.apply(&mut settings.write()),
                }
            }
        }
        p { id: "preset-line", class: "muted", style: "margin: 0;", "{line}" }
    }
}

/// How a run goes: one station at a time, or a pile-up of as many as the
/// Band card's stations range draws, which the line under it names.
#[component]
fn RunStylePicker(settings: Signal<TrainingSettings>, style: RunStyle, callers: String) -> Element {
    let line = match style {
        RunStyle::Single => "One station calls at a time".to_string(),
        RunStyle::Pileup => format!("{callers} · set on the Band card"),
    };
    rsx! {
        div { class: "eyebrow", "Run style" }
        div { class: "mode-pills",
            for (each, label) in RUN_STYLES {
                Seg {
                    label,
                    id: control_id("style", label),
                    pill: true,
                    active: style == each,
                    onclick: move |_| settings.write().set_contest_style(each),
                }
            }
        }
        p { id: "run-style-line", class: "muted", style: "margin: 0;", "{line}" }
    }
}

/// How often your TU signs with your call in a pile-up: every QSO, every
/// third, as MorseRunner's station ID rate has it, or never. The callers
/// call after your TU whichever it is.
#[component]
fn TuCallPicker(settings: Signal<TrainingSettings>, tu_call: TuCall) -> Element {
    let line = match tu_call {
        TuCall::Every => "Every TU signs with your call",
        TuCall::Third => {
            "Every third QSO's TU signs with your call, so a station tuning in hears who is running"
        }
        TuCall::Never => "No TU signs with your call; the callers still call after each",
    };
    rsx! {
        div { class: "eyebrow", "Your call in TU" }
        div { class: "mode-pills",
            for (each, label) in TU_CALLS {
                Seg {
                    label,
                    id: control_id("tu", label),
                    pill: true,
                    active: tu_call == each,
                    onclick: move |_| settings.write().contest.contest_tu_call = each,
                }
            }
        }
        p { id: "tu-call-line", class: "muted", style: "margin: 0;", "{line}" }
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
