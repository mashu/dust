//! The settings screen: one card per part of the trainer.
//!
//! Each card reads the slice of the settings it shows, through a memo, so a
//! slider moving on one card redraws that card and leaves the rest of the
//! page alone.

use cw_core::{
    ADJUST_COUNT_MAX, ADJUST_THRESHOLD_MAX_PCT, ADJUST_THRESHOLD_MIN_PCT, Aim, AutoLevelSettings,
    CONTEST_MINUTES_MAX, COUNTDOWN_MAX_SEC, COVERAGE_STRENGTH_MAX, CharSetMode, ContestSettings,
    ERROR_WEIGHT_MAX, EXTRA_WORD_SPACE_MAX, EXTRA_WORD_SPACE_MIN, Experience, GROUP_PAUSE_MAX_SEC,
    GROUP_REPEAT_MAX, GROUP_REPEAT_MIN, GROUP_SIZE_MAX, GROUP_SIZE_MIN, GROUP_TIMEOUT_MAX_SEC,
    KeyerMode, MASTER_VOLUME_MAX, MASTER_VOLUME_MIN, MY_SENDING_MAX_DB, MY_SENDING_OFF_DB,
    NUM_GROUPS_MAX, NUM_GROUPS_MIN, RangeSetting, SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ,
    SettingsSection, SpeedPath, TrainingSettings, VOLUME_MAX, VOLUME_MIN, WPM_MAX, WPM_MIN,
    adaptive_speed_on, adapts_group_size, adapts_speed, aim_section,
};
use dioxus::prelude::*;

use crate::playback::Dialogue;
use crate::ui::band::BandConditionsCard;
use crate::ui::envelope::EnvelopeCard;
use crate::ui::help;
use crate::ui::home::range_label;
use crate::ui::paddle::PaddlePad;
use crate::ui::settings_charset::CharsetCard;
use crate::ui::settings_station::StationCard;
use crate::ui::widgets::{
    CardHead, Icon, IconName, NumberField, SectionReset, Seg, SettingsRange, SliderField, Switch,
    control_id, html_bool,
};

/// What an aim is called on screen.
pub(crate) fn aim_label(aim: Aim) -> &'static str {
    match aim {
        Aim::Comfortable => "Comfortable",
        Aim::Balanced => "Balanced",
        Aim::Stretch => "Stretch",
    }
}

/// The keyer modes, in the order the control offers them.
const KEYER_MODES: [(KeyerMode, &str); 4] = [
    (KeyerMode::Straight, "Straight"),
    (KeyerMode::IambicA, "Iambic A"),
    (KeyerMode::IambicB, "Iambic B"),
    (KeyerMode::Ultimatic, "Ultimatic"),
];

/// What a keyer mode does with the paddles.
fn keyer_description(mode: KeyerMode) -> &'static str {
    match mode {
        KeyerMode::Straight => {
            "One contact. Hold [ or ] — a short press is a dit, a long one a dah. You time the elements."
        }
        KeyerMode::IambicA => {
            "Hold a paddle and it repeats at the key speed you set. Squeeze both to alternate. Let go and the element you were on finishes, then it stops."
        }
        KeyerMode::IambicB => {
            "Like Iambic A, but releasing a squeeze sends one extra opposite element — the extra dit after a dah is how C is often sent."
        }
        KeyerMode::Ultimatic => {
            "Last paddle you close wins and repeats. It does not alternate. Squeeze the dit paddle in a dah for the two dits in X, then let the dit go for the last dah."
        }
    }
}

/// A settings card's head: its icon, title and note, the button that puts
/// the section back to its defaults, and its ⓘ with what the options do.
#[component]
fn SectionHead(
    icon: IconName,
    title: &'static str,
    note: &'static str,
    settings: Signal<TrainingSettings>,
    section: SettingsSection,
    help: Element,
) -> Element {
    rsx! {
        CardHead { icon, title, note, help,
            SectionReset {
                label: title.to_lowercase(),
                on_reset: move |()| settings.write().reset_section(section),
            }
        }
    }
}

#[component]
pub fn SettingsView(
    settings: Signal<TrainingSettings>,
    previewing: bool,
    sample_playing: Option<String>,
    on_preview_band: EventHandler<()>,
    on_stop_band: EventHandler<()>,
    on_play_sample: EventHandler<String>,
    /// Hear a QSO is playing, from the Character set card in Contest mode.
    #[props(default)]
    hearing: bool,
    /// What Hear a QSO has played, kept once it ends.
    #[props(default)]
    dialogue: Option<Dialogue>,
    #[props(default)] on_hear_qsos: EventHandler<()>,
) -> Element {
    // A contest has a station of its own, and no level to move or groups to
    // set a preset for.
    let contest =
        use_memo(move || settings.read().curriculum.char_set_mode == CharSetMode::Contest)();
    rsx! {
        div { class: "stack",
            header { class: "page-head",
                h2 { class: "page-title", "Settings" }
                p { class: "page-sub", "Everything the trainer sends, and how hard it makes you work." }
            }
            if !contest {
                ExperienceCard { settings }
            }
            CharsetCard {
                settings,
                hearing,
                dialogue,
                on_hear: on_hear_qsos,
                on_stop: on_stop_band,
            }
            if contest {
                StationCard { settings }
            }
            SessionShapeCard { settings }
            SpeedCard { settings }
            ToneCard { settings }
            EnvelopeCard {
                settings,
                sample_playing,
                on_play: on_play_sample,
                on_stop: on_stop_band,
            }
            if !contest {
                AutoLevelSettingsCard { settings }
            }
            SamplingCard { settings }
            BandConditionsCard {
                settings,
                previewing,
                on_preview: on_preview_band,
                on_stop: on_stop_band,
            }
            crate::ui::privacy::PrivacyCard {}
            crate::ui::credits::CreditsCard {}
        }
    }
}

/// What a preset sets, in a line: "20 groups of 2–3 · 15–25 WPM".
fn experience_summary(experience: Experience) -> String {
    let (fewest, most) = experience.group_size();
    let (slowest, fastest) = experience.char_wpm();
    format!(
        "{} groups of {} · {} WPM",
        experience.groups(),
        range_label(f64::from(fewest), f64::from(most)),
        range_label(slowest, fastest),
    )
}

/// A starting point for the kind of learner you are. The preset the
/// settings are on is the one shown, so a change made anywhere on the page
/// to what it set shows them as your own.
#[component]
fn ExperienceCard(settings: Signal<TrainingSettings>) -> Element {
    let on = use_memo(move || Experience::of(&settings.read()))();
    let note = match on {
        Some(experience) => experience_summary(experience),
        None => "Sets session length, group size and speed to start from.".to_string(),
    };
    rsx! {
        div { class: "card experience",
            CardHead {
                icon: IconName::Sparkle,
                title: "Experience",
                note,
                help: help::experience(),
                ExperiencePicker {
                    on,
                    on_pick: move |experience: Experience| experience.apply(&mut settings.write()),
                }
            }
        }
    }
}

/// The presets as a dropdown: the platform's own picker, dressed as a pill.
///
/// The option marked is the one shown, even after another was picked by
/// hand: the renderer sets `selected` on the option itself, not just in its
/// markup, which a select stops reading once it has been picked from.
#[component]
fn ExperiencePicker(on: Option<Experience>, on_pick: EventHandler<Experience>) -> Element {
    rsx! {
        span { class: "select-wrap",
            select {
                id: control_id("select", "experience"),
                class: if on.is_some() { "select chosen" } else { "select" },
                aria_label: "Experience",
                onchange: move |e| {
                    if let Some(experience) = Experience::from_key(&e.value()) {
                        on_pick.call(experience);
                    }
                },
                if on.is_none() {
                    option { value: "", disabled: html_bool(true), selected: html_bool(true), "Custom" }
                }
                for experience in Experience::ALL {
                    option {
                        value: experience.key(),
                        selected: html_bool(on == Some(experience)),
                        "{experience.name()}"
                    }
                }
            }
            Icon { name: IconName::Chevron }
        }
    }
}

/// What Show copy does, and what it costs a run.
const SHOW_COPY_NOTE: &str = "Shows each caller's text on the run screen a moment after it is sent. A run with it on is marked assisted: it counts in every stat, but is greyed on the board and never placed there.";

/// What the session-shape card shows.
#[derive(Clone, PartialEq)]
struct SessionShape {
    callsigns: bool,
    words: bool,
    /// A contest is timed rather than counted, and has no groups.
    contest: bool,
    contest_minutes: u32,
    /// Each caller's text shows on the run screen, and the run is assisted.
    show_copy: bool,
    /// The group size is one length, so there is nothing to adapt.
    one_size: bool,
    adaptive: bool,
    /// Adaptive group size is on and has lengths to choose between: what the
    /// sampler itself goes by.
    adapts: bool,
    /// Adaptive speed is steering too, so the aim covers characters.
    speed_adapts: bool,
    aim: Aim,
    num_groups: u32,
    group_timeout: f64,
    group_pause_sec: f64,
    countdown_sec: u32,
    lock_input: bool,
    keyer_mode: KeyerMode,
    keyer_wpm: f64,
    paddle_swap: bool,
}

impl SessionShape {
    fn of(settings: &TrainingSettings) -> Self {
        let p = &settings.playback;
        Self {
            callsigns: settings.curriculum.char_set_mode.sends_callsigns(),
            words: settings.curriculum.char_set_mode == CharSetMode::Words,
            contest: settings.curriculum.char_set_mode == CharSetMode::Contest,
            contest_minutes: settings.contest.contest_minutes,
            show_copy: settings.contest.contest_show_copy,
            one_size: settings.curriculum.min_group_size >= settings.curriculum.max_group_size,
            adaptive: settings.curriculum.adaptive_group_size,
            adapts: adapts_group_size(settings),
            speed_adapts: adapts_speed(settings),
            aim: settings.curriculum.aim,
            num_groups: settings.curriculum.num_groups,
            group_timeout: p.group_timeout,
            group_pause_sec: p.group_pause_sec,
            countdown_sec: p.countdown_sec,
            lock_input: p.lock_input_during_group_playback,
            keyer_mode: p.keyer_mode,
            keyer_wpm: p.keyer_wpm,
            paddle_swap: p.paddle_swap,
        }
    }
}

/// What an aim asks of you, in a sentence: of the groups copied whole, if
/// group size adapts; of the characters right, if speed does.
fn aim_note(aim: Aim, groups: bool, chars: bool) -> String {
    let label = aim_label(aim);
    let whole = aim.groups_whole() * 100.0;
    let right = aim.chars_right() * 100.0;
    match (groups, chars) {
        (true, true) => format!(
            "{label} aims for {whole:.0}% of groups copied whole and {right:.0}% of characters right."
        ),
        (false, true) => format!("{label} aims for {right:.0}% of characters right."),
        _ => format!("{label} aims for {whole:.0}% of groups copied whole."),
    }
}

/// How hard the adaptive features push: one setting for group size and
/// speed alike.
#[component]
fn AimFor(settings: Signal<TrainingSettings>, aim: Aim, note: String) -> Element {
    rsx! {
        div { class: "field",
            span { class: "field-label", "Aim for" }
            div { class: "segmented",
                for each in Aim::ALL {
                    Seg {
                        label: aim_label(each),
                        active: aim == each,
                        onclick: move |_| settings.write().curriculum.aim = each,
                    }
                }
            }
            p { class: "muted", style: "margin: 0; font-size: 0.8rem;", "{note}" }
        }
    }
}

#[component]
fn SessionShapeCard(settings: Signal<TrainingSettings>) -> Element {
    let shape = use_memo(move || SessionShape::of(&settings.read()));
    let s = shape.read();
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Timer,
                title: "Session shape",
                note: "How much gets sent in one run",
                settings,
                section: SettingsSection::SessionShape,
                help: help::session_shape(),
            }
            div { class: "field-grid",
                if s.contest {
                    NumberField {
                        label: "Minutes per run",
                        value: f64::from(s.contest_minutes),
                        min: 0.0,
                        max: f64::from(CONTEST_MINUTES_MAX),
                        step: 1.0,
                        unit: "min",
                        onchange: move |v| settings.write().contest.contest_minutes = v as u32,
                    }
                } else {
                    NumberField {
                        label: if s.callsigns {
                            "Callsigns per session"
                        } else if s.words {
                            "Words per session"
                        } else {
                            "Groups per session"
                        },
                        value: f64::from(s.num_groups),
                        min: f64::from(NUM_GROUPS_MIN),
                        max: f64::from(NUM_GROUPS_MAX),
                        step: 1.0,
                        onchange: move |v| settings.write().curriculum.num_groups = v as u32,
                    }
                    NumberField {
                        label: "Answer timeout",
                        value: s.group_timeout,
                        min: 0.0,
                        max: GROUP_TIMEOUT_MAX_SEC,
                        step: 1.0,
                        unit: "sec",
                        onchange: move |v| settings.write().playback.group_timeout = v,
                    }
                    NumberField {
                        label: "Pause after each group",
                        value: s.group_pause_sec,
                        min: 0.0,
                        max: GROUP_PAUSE_MAX_SEC,
                        step: 0.5,
                        unit: "sec",
                        onchange: move |v| settings.write().playback.group_pause_sec = v,
                    }
                }
                NumberField {
                    label: if s.contest {
                        "Countdown before the first CQ"
                    } else {
                        "Countdown before the first group"
                    },
                    value: f64::from(s.countdown_sec),
                    min: 0.0,
                    max: f64::from(COUNTDOWN_MAX_SEC),
                    step: 1.0,
                    unit: "sec",
                    onchange: move |v| settings.write().playback.countdown_sec = v as u32,
                }
            }
            if s.contest {
                p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                    "A run of zero minutes lasts until you stop it, and ends at {CONTEST_MINUTES_MAX} all the same."
                }
                Switch {
                    title: "Show copy",
                    description: "{SHOW_COPY_NOTE}",
                    checked: s.show_copy,
                    onchange: move |on| settings.write().contest.contest_show_copy = on,
                }
            } else {
                p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                    "Time to read the last group before the next send. Zero keeps contest pace. Repeats of the same group stay a word space apart."
                }
                p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                    "The answer timeout moves on from a group you have not started. Once you type, the group waits for you: it is checked when every character is in, or when you press Next or Enter."
                }
            }
            // A callsign or a word is as long as it is, so there is nothing to
            // set.
            if !s.callsigns && !s.words {
                SettingsRange {
                    settings,
                    which: RangeSetting::GroupSize,
                    label: "Group size",
                    unit: "chars",
                    min_bound: f64::from(GROUP_SIZE_MIN),
                    max_bound: f64::from(GROUP_SIZE_MAX),
                    step: 1.0,
                }
                Switch {
                    title: "Adapt group size to me",
                    description: if s.one_size {
                        "Set Group size to Random to use this: one size leaves nothing to adapt."
                    } else {
                        "Within this range: longer groups while you copy them whole, shorter ones when you don't. Your copy span is under Stats."
                    },
                    checked: s.adaptive,
                    onchange: move |on| settings.write().curriculum.adaptive_group_size = on,
                }
                if s.adapts {
                    AimFor {
                        settings,
                        aim: s.aim,
                        note: aim_note(s.aim, true, s.speed_adapts),
                    }
                }
            }
            // A caller sends again when you ask, and you type as it sends.
            if !s.contest {
                SettingsRange {
                    settings,
                    which: RangeSetting::GroupRepeat,
                    label: "Sends per group",
                    unit: "×",
                    min_bound: f64::from(GROUP_REPEAT_MIN),
                    max_bound: f64::from(GROUP_REPEAT_MAX),
                    step: 1.0,
                    hint: "Each group is repeated this many times before the answer box opens — one word space apart, like a station repeating its call. Switch to Random to vary it per group.",
                }
                Switch {
                    title: "Lock typing while sending",
                    description: "Keeps you copying by ear instead of typing along.",
                    checked: s.lock_input,
                    onchange: move |on| settings.write().playback.lock_input_during_group_playback = on,
                }
            }
            div { class: "field",
                span { class: "field-label", "Key" }
                div { class: "segmented",
                    for (mode, label) in KEYER_MODES {
                        Seg {
                            label,
                            active: s.keyer_mode == mode,
                            onclick: move |_| settings.write().playback.keyer_mode = mode,
                        }
                    }
                }
                p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                    "{keyer_description(s.keyer_mode)}"
                }
            }
            NumberField {
                label: "Key speed",
                value: s.keyer_wpm,
                min: WPM_MIN,
                max: WPM_MAX,
                step: 1.0,
                unit: "WPM",
                onchange: move |v| settings.write().playback.keyer_wpm = v,
            }
            PaddlePad { mode: s.keyer_mode }
            if !s.keyer_mode.is_straight() {
                Switch {
                    title: "Swap [ and ] paddles",
                    description: "[ is dit and ] is dah. Turn this on if your paddles are the other way around.",
                    checked: s.paddle_swap,
                    onchange: move |on| settings.write().playback.paddle_swap = on,
                }
            }
        }
    }
}

/// What the speed card shows beyond its two ranges.
#[derive(Clone, Copy, PartialEq)]
struct Speed {
    tied: bool,
    fist_variation: f64,
    extra_word_space: f64,
    /// The switch that has the speed adapt: group training's, or in Contest
    /// mode the callers' own.
    adaptive: bool,
    /// More than one speed to choose from.
    room: bool,
    adapts: bool,
    /// The aim is set here, not under Session shape.
    aim_here: bool,
    aim: Aim,
    /// The ranges are a contest's callers'.
    contest: bool,
}

/// Why an adaptive speed switch does nothing with one speed set, naming
/// only the ranges the card shows: tied, Effective speed is hidden.
fn one_speed(tied: bool) -> &'static str {
    if tied {
        "Give Character speed a range to use this: one speed leaves nothing to adapt."
    } else {
        "Give Character speed or Effective speed a range to use this: one speed leaves nothing to adapt."
    }
}

#[component]
fn SpeedCard(settings: Signal<TrainingSettings>) -> Element {
    let speed = use_memo(move || {
        let all = settings.read();
        let p = &all.playback;
        let contest = all.curriculum.char_set_mode == CharSetMode::Contest;
        Speed {
            tied: p.link_char_to_effective,
            fist_variation: p.fist_variation,
            extra_word_space: p.extra_word_space_multiplier,
            adaptive: adaptive_speed_on(&all),
            room: SpeedPath::of(&all).has_room(),
            adapts: adapts_speed(&all),
            aim_here: aim_section(&all) == SettingsSection::Speed,
            aim: all.curriculum.aim,
            contest,
        }
    })();
    let note = if speed.contest {
        "The callers' speeds, Farnsworth spacing, and yours"
    } else {
        "Character speed and Farnsworth spacing"
    };
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Gauge,
                title: "Speed",
                note,
                settings,
                section: SettingsSection::Speed,
                help: help::speed_settings(),
            }
            SettingsRange {
                settings,
                which: RangeSetting::CharWpm,
                label: "Character speed",
                unit: "WPM",
                min_bound: WPM_MIN,
                max_bound: WPM_MAX,
                step: 1.0,
            }
            if !speed.tied {
                SettingsRange {
                    settings,
                    which: RangeSetting::EffectiveWpm,
                    label: "Effective speed",
                    unit: "WPM",
                    min_bound: WPM_MIN,
                    max_bound: WPM_MAX,
                    step: 1.0,
                    hint: "Farnsworth: characters stay fast, the gaps between them stretch.",
                }
            }
            if speed.contest {
                p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                    "Callers come at speeds drawn from these ranges."
                }
                Switch {
                    title: "Adapt callers' speed to me",
                    description: if speed.room {
                        "New callers come faster while you copy calls and numbers clean, slower when you miss them; those already calling keep their speed. Your copy speed is under Stats."
                    } else {
                        one_speed(speed.tied)
                    },
                    checked: speed.adaptive,
                    onchange: move |on| settings.write().contest.contest_adapt_speed = on,
                }
            } else {
                Switch {
                    title: "Adapt speed to me",
                    description: if speed.room {
                        "Within these speeds: faster while you copy the characters, slower when you miss them. Your copy speed is under Stats."
                    } else {
                        one_speed(speed.tied)
                    },
                    checked: speed.adaptive,
                    onchange: move |on| settings.write().playback.adaptive_speed = on,
                }
            }
            if speed.aim_here {
                AimFor {
                    settings,
                    aim: speed.aim,
                    note: aim_note(speed.aim, false, true),
                }
            } else if speed.adapts {
                p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                    "{aim_note(speed.aim, false, true)} Aim for is set under Session shape, for group size and speed together."
                }
            }
            if speed.contest {
                YourSpeed { settings }
            }
            SliderField {
                label: "Operator fists",
                id: control_id("slider", "operator fists"),
                value_label: if speed.fist_variation <= 0.0 {
                    "Keyers".to_string()
                } else {
                    format!("{:.0}%", speed.fist_variation * 100.0)
                },
                value: speed.fist_variation,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: false,
                onchange: move |v| settings.write().playback.fist_variation = v,
            }
            p { class: "muted", style: "margin: 0;",
                "How far the stations calling stray from a keyer — heavier or lighter elements, longer or shorter dahs, each operator sending their own way. At zero every station is a machine, which is what you want while a character is still new."
            }
            NumberField {
                label: "Extra word spacing",
                value: speed.extra_word_space,
                min: EXTRA_WORD_SPACE_MIN,
                max: EXTRA_WORD_SPACE_MAX,
                step: 0.1,
                unit: "×",
                onchange: move |v| settings.write().playback.extra_word_space_multiplier = v,
            }
            Switch {
                title: "Tie effective speed to character speed",
                description: "Off gives you Farnsworth spacing controls.",
                checked: speed.tied,
                onchange: move |on| {
                    let w = &mut *settings.write();
                    w.playback.link_char_to_effective = on;
                    w.sync_effective_to_char();
                },
            }
        }
    }
}

/// Your speed in a run, in whole words a minute: one of your own, kept from
/// run to run as − and + leave it, or until you set one the middle of the
/// callers' range, which follows the range, rounded as the run bar shows
/// it. Set, a press puts it back to the middle. It reads its own values, so
/// the card around it never redraws for them.
#[component]
fn YourSpeed(settings: Signal<TrainingSettings>) -> Element {
    let (my_wpm, shown) = use_memo(move || {
        let all = settings.read();
        (all.contest.my_wpm, all.my_wpm().round())
    })();
    rsx! {
        NumberField {
            label: "Your speed",
            value: shown,
            min: WPM_MIN,
            max: WPM_MAX,
            step: 1.0,
            unit: "WPM",
            onchange: move |v: f64| settings.write().contest.my_wpm = v.round() as u32,
        }
        if my_wpm == 0 {
            p { id: "your-speed-line", class: "muted", style: "margin: 0; font-size: 0.8rem;",
                "The middle of the callers' range until you set it. The run screen's − and + move it, and it is kept for the next run."
            }
        } else {
            p { id: "your-speed-line", class: "muted", style: "margin: 0; font-size: 0.8rem;",
                "Yours from run to run: the run screen's − and + move it, and it is kept for the next run."
            }
            div {
                button {
                    id: control_id("btn", "callers middle"),
                    class: "btn btn-secondary btn-sm",
                    r#type: "button",
                    onclick: move |_| settings.write().contest.my_wpm = 0,
                    "Send at the callers' middle"
                }
            }
        }
    }
}

/// How loud everything plays, the band, the stations, your sending and the
/// paddle's sidetone together, as a share of full: the one control for
/// that, the station strength below being how strong a station is against
/// the band. It reads its own value, so the card around it never redraws
/// for it.
#[component]
fn MasterVolume(settings: Signal<TrainingSettings>) -> Element {
    let volume = use_memo(move || settings.read().band.master_volume)();
    rsx! {
        SliderField {
            label: "Volume",
            value_label: format!("{:.0}%", volume * 100.0),
            value: volume,
            min: MASTER_VOLUME_MIN,
            max: MASTER_VOLUME_MAX,
            step: 0.05,
            disabled: false,
            onchange: move |v: f64| settings.write().band.master_volume = (v * 100.0).round() / 100.0,
        }
    }
}

/// What your sending reads as: "−9 dB", or "Off" at the bottom.
fn sending_label(db: i32) -> String {
    if db <= MY_SENDING_OFF_DB {
        "Off".to_string()
    } else if db < 0 {
        format!("\u{2212}{} dB", -db)
    } else {
        format!("{db} dB")
    }
}

/// How loud you hear your own messages in a run, in Contest mode only: from
/// off to the loudest, in decibels as MorseRunner's self-monitor reads,
/// whatever the station strength range above says, at the volume. It reads
/// its own value, so the card around it never redraws for it.
#[component]
fn SendingLevel(settings: Signal<TrainingSettings>) -> Element {
    let sending = use_memo(move || {
        let all = settings.read();
        (all.curriculum.char_set_mode == CharSetMode::Contest).then_some(all.contest.my_sending_db)
    })();
    let Some(db) = sending else {
        return rsx! {};
    };
    // Where it starts, which cw-core keeps.
    let start = sending_label(ContestSettings::default().my_sending_db);
    rsx! {
        SliderField {
            label: "Your sending",
            value_label: sending_label(db),
            value: f64::from(db),
            min: f64::from(MY_SENDING_OFF_DB),
            max: f64::from(MY_SENDING_MAX_DB),
            step: 3.0,
            disabled: false,
            onchange: move |v: f64| settings.write().contest.my_sending_db = v.round() as i32,
        }
        p { id: "sending-line", class: "muted", style: "margin: 0; font-size: 0.8rem;",
            "Your own messages in a run, apart from the station strength above: 0 dB is the loudest, {start} about level with an average caller, and Off silent, the run keeping its time."
        }
    }
}

/// Whether the band is heard through your own messages, in Contest mode
/// only: with full break-in, as a rig that has it hears between the
/// elements it sends; without it, as MorseRunner ships it, the receiver
/// muted while you send. It reads its own value, so the card around it
/// never redraws for it.
#[component]
fn FullBreakIn(settings: Signal<TrainingSettings>) -> Element {
    let qsk = use_memo(move || {
        let all = settings.read();
        (all.curriculum.char_set_mode == CharSetMode::Contest).then_some(all.contest.contest_qsk)
    })();
    let Some(on) = qsk else {
        return rsx! {};
    };
    rsx! {
        Switch {
            title: "Full break-in (QSK)",
            description: "Hear the band through your own messages, and a caller sending across one. Off, the receiver is muted while you send, as on a rig without it, and nothing sent meanwhile reaches you.",
            checked: on,
            onchange: move |on| settings.write().contest.contest_qsk = on,
        }
    }
}

/// Nothing but the volume and ranges, which read their own values, and in
/// Contest mode your sending and full break-in, which read their own: this
/// card itself never redraws for a setting.
#[component]
fn ToneCard(settings: Signal<TrainingSettings>) -> Element {
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Volume,
                title: "Tone & volume",
                note: "Volume, pitch and station strength",
                settings,
                section: SettingsSection::ToneAndVolume,
                help: help::tone(),
            }
            MasterVolume { settings }
            SettingsRange {
                settings,
                which: RangeSetting::SideTone,
                label: "Side tone",
                unit: "Hz",
                min_bound: SIDE_TONE_MIN_HZ,
                max_bound: SIDE_TONE_MAX_HZ,
                step: 10.0,
                hint: "A range picks a fresh pitch per group, the way different stations sound.",
            }
            SettingsRange {
                settings,
                which: RangeSetting::Volume,
                label: "Station strength",
                unit: "%",
                min_bound: VOLUME_MIN,
                max_bound: VOLUME_MAX,
                step: 0.05,
                percent: true,
                hint: "How far the station stands over the band's noise, not how loud it plays. A range draws a fresh strength per group.",
            }
            SendingLevel { settings }
            FullBreakIn { settings }
        }
    }
}

/// The auto-level settings, which both of the cards below read.
fn use_auto_level(settings: Signal<TrainingSettings>) -> Memo<AutoLevelSettings> {
    use_memo(move || settings.read().auto_level.clone())
}

#[component]
fn AutoLevelSettingsCard(settings: Signal<TrainingSettings>) -> Element {
    let auto = use_auto_level(settings);
    let a = auto.read();
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Target,
                title: "Auto level",
                note: "Move the level from session accuracy",
                settings,
                section: SettingsSection::AutoLevel,
                help: help::auto_level(),
            }
            Switch {
                title: "Adjust the level for me",
                description: "Unlocks a new character after a run of good sessions.",
                checked: a.auto_adjust_level,
                onchange: move |on| settings.write().auto_level.auto_adjust_level = on,
            }
            NumberField {
                label: "Accuracy threshold",
                value: a.auto_adjust_threshold,
                min: ADJUST_THRESHOLD_MIN_PCT,
                max: ADJUST_THRESHOLD_MAX_PCT,
                step: 1.0,
                unit: "%",
                onchange: move |v| settings.write().auto_level.auto_adjust_threshold = v,
            }
            div { class: "field-grid",
                NumberField {
                    label: "Sessions above to level up",
                    value: f64::from(a.auto_adjust_above_threshold_count),
                    min: 0.0,
                    max: f64::from(ADJUST_COUNT_MAX),
                    step: 1.0,
                    onchange: move |v| settings.write().auto_level.auto_adjust_above_threshold_count = v as u32,
                }
                NumberField {
                    label: "Sessions below to level down",
                    value: f64::from(a.auto_adjust_below_threshold_count),
                    min: 0.0,
                    max: f64::from(ADJUST_COUNT_MAX),
                    step: 1.0,
                    onchange: move |v| settings.write().auto_level.auto_adjust_below_threshold_count = v as u32,
                }
            }
        }
    }
}

#[component]
fn SamplingCard(settings: Signal<TrainingSettings>) -> Element {
    let auto = use_auto_level(settings);
    let a = auto.read();
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Shuffle,
                title: "Character sampling",
                note: "Which characters come round more often",
                settings,
                section: SettingsSection::CharacterSampling,
                help: help::sampling(),
            }
            div { class: "field-grid",
                NumberField {
                    label: "Error-weight strength",
                    value: a.error_weight_strength,
                    min: 0.0,
                    max: ERROR_WEIGHT_MAX,
                    step: 0.5,
                    onchange: move |v| settings.write().auto_level.error_weight_strength = v,
                }
                NumberField {
                    label: "Coverage strength",
                    value: a.char_sampling_coverage_strength,
                    min: 0.0,
                    max: COVERAGE_STRENGTH_MAX,
                    step: 0.5,
                    onchange: move |v| settings.write().auto_level.char_sampling_coverage_strength = v,
                }
            }
            Switch {
                title: "Thompson sampling",
                description: "Draws from each letter's uncertainty instead of its average error rate.",
                checked: a.char_sampling_thompson,
                onchange: move |on| settings.write().auto_level.char_sampling_thompson = on,
            }
        }
    }
}
