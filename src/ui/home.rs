use cw_core::{
    AutoLevelProgress, CharSetMode, ContestSettings, LevelAxis, RunStyle, StreakStatus,
    TrainingSettings, compute_char_pool, compute_streak_status, newest_unlocked, parse_callsign,
};
use dioxus::prelude::*;

use crate::audio::AUDIO_IS_SILENT;
use crate::ui::auto_level::AutoLevelCard;
use crate::ui::figures::History;
use crate::ui::heatmap::{ActivityHeatmap, Calendar, StreakCard};
use crate::ui::settings_charset::{pileup_callers, pileup_count};
use crate::ui::settings_station::{
    CallField, CallReading, NameField, SectionEntry, StateEntry, ZoneField, focus_call_field,
    focus_name_field, focus_section_field, focus_state_field, focus_zone_field, section_rejected,
    state_rejected,
};
use crate::ui::tips::{CONTEST_TIPS, TIPS, TipsCarousel};
use crate::ui::widgets::{
    CardHead, Icon, IconName, TextDraft, control_id, focus_script, keep_focus, range_label,
};

/// Give Start the cursor, as Enter in the hero's last field does.
fn focus_start() {
    let id = control_id("btn", "start training");
    let script = focus_script(&id);
    // Tests have no page to run it on: the harness looks for the button.
    #[cfg(test)]
    crate::testing::ask_focus(&id, &script);
    #[cfg(not(test))]
    spawn(async move {
        let _ = document::eval(&script).await;
    });
}

/// Where the learner is, as Home says it: in the hero, and on the tile.
#[derive(Debug, PartialEq)]
struct LevelReading {
    /// One line, for the hero: "Level 7 · digits 3".
    summary: String,
    /// The tile's caption and figure: "Levels", "7 / 3".
    label: &'static str,
    value: String,
}

/// The levels the settings are trained at — as far as the alphabets in use
/// reach, not what is stored for a longer one.
fn level_reading(settings: &TrainingSettings) -> LevelReading {
    let letters = settings.level(LevelAxis::Letters);
    let digits = settings.level(LevelAxis::Digits);
    let tier = settings.level(LevelAxis::Callsign);
    let (label, value, summary) = match settings.curriculum.char_set_mode {
        CharSetMode::Callsign => (
            "Callsign tier",
            tier.to_string(),
            format!("Callsign tier {tier}"),
        ),
        // A contest is told by its name; its callers are shaped by the
        // callsign tier, which the tile shows.
        CharSetMode::Contest => (
            "Callsign tier",
            tier.to_string(),
            settings.contest.contest_kind.label().to_string(),
        ),
        CharSetMode::Digits => (
            "Digits level",
            digits.to_string(),
            format!("Digits level {digits}"),
        ),
        CharSetMode::Mixed => (
            "Levels",
            format!("{letters} / {digits}"),
            format!("Level {letters} · digits {digits}"),
        ),
        CharSetMode::Koch => ("Level", letters.to_string(), format!("Level {letters}")),
        // A word list has no level: what it has is words.
        CharSetMode::Words => {
            let count = settings.words().len();
            let whose = if settings.has_own_words() {
                "Your words"
            } else {
                "Starter words"
            };
            ("Words", count.to_string(), format!("{whose} · {count}"))
        }
    };
    LevelReading {
        summary,
        label,
        value,
    }
}

/// What the hero shows in Contest mode.
#[derive(Debug, PartialEq)]
struct ContestHero {
    /// What you copy of each station, as a log heads it: `CALL NR`.
    fields: Vec<&'static str>,
    /// What you send, and as whom: "You send 5NN + serial, as DL1ABC".
    sends: String,
    /// How long a run is, as the specs give it: its figure and caption.
    length: (String, String),
    /// Who calls, as the specs give it: "Single" "Caller", or "1–3"
    /// "Callers".
    callers: (String, String),
    /// Who calls, in words: "single caller", "1–3 callers".
    who_calls: String,
    /// The field of your station the hero asks for, the first a run is
    /// missing; none once it has them all.
    asks: Option<StationAsk>,
    /// A call is typed that a contest cannot read, so the line under the
    /// field already says what is wrong with it.
    call_flagged: bool,
    /// What a run still needs before it can start, said as the settings say
    /// it.
    missing: Option<String>,
}

/// One of your station's fields, as the hero asks for it: in this order,
/// each once the one before it is there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StationAsk {
    /// Your call is missing, or not one a contest can read.
    Call,
    /// The call reads, but the contest sends your name and you have given
    /// none.
    Name,
    /// The call and the name are there, but the contest sends your state
    /// and a call in the US or Canada has none.
    State,
    /// The rest is there, but the contest sends your CQ zone and your call
    /// does not say it.
    Zone,
    /// The rest is there, but the contest sends your ARRL or RAC section
    /// and your call does not tell it.
    Section,
}

impl StationAsk {
    /// The first field of `contest` a run is missing, if the hero asks for
    /// one.
    fn of(contest: &ContestSettings) -> Option<Self> {
        let rules = contest.contest_kind.rules();
        if parse_callsign(&contest.my_call).is_none() {
            Some(Self::Call)
        } else if rules.sends_name() && contest.my_name.is_empty() {
            Some(Self::Name)
        } else if rules.sends_state(contest.side()) && contest.state_sent().is_empty() {
            Some(Self::State)
        } else if rules.sends_zone(contest.side()) && contest.zone().is_none() {
            Some(Self::Zone)
        } else if rules.sends_section(contest.side()) && contest.section_sent().is_empty() {
            Some(Self::Section)
        } else {
            None
        }
    }

    /// `typed` kept in `contest`, as its field keeps it.
    fn keep(self, contest: &mut ContestSettings, typed: &str) {
        match self {
            Self::Call => contest.set_my_call(typed),
            Self::Name => contest.set_my_name(typed),
            Self::State => contest.set_my_state(typed),
            Self::Zone => contest.set_my_zone(typed),
            Self::Section => contest.set_my_section(typed),
        }
    }

    /// What gives its field the focus.
    fn focus(self) -> fn() {
        match self {
            Self::Call => focus_call_field,
            Self::Name => focus_name_field,
            Self::State => focus_state_field,
            Self::Zone => focus_zone_field,
            Self::Section => focus_section_field,
        }
    }

    /// What the first contest's card says to do.
    fn first_step(self) -> &'static str {
        match self {
            Self::Call => {
                "Add your call, press Start contest; Enter sends what the QSO needs next."
            }
            Self::Name => {
                "Add your name, press Start contest; Enter sends what the QSO needs next."
            }
            Self::State => {
                "Add your state, press Start contest; Enter sends what the QSO needs next."
            }
            Self::Zone => {
                "Add your CQ zone, press Start contest; Enter sends what the QSO needs next."
            }
            Self::Section => {
                "Add your section, press Start contest; Enter sends what the QSO needs next."
            }
        }
    }
}

/// Whether `settings` lock typing while a group is sent, with groups the
/// lock keeps from being timed: longer than one character. A contest run's
/// fields are never locked.
pub(crate) fn lock_keeps_groups_untimed(settings: &TrainingSettings) -> bool {
    let mode = settings.curriculum.char_set_mode;
    let longer = match mode {
        CharSetMode::Contest => return false,
        CharSetMode::Callsign | CharSetMode::Words => true,
        _ => settings.curriculum.max_group_size > 1,
    };
    settings.playback.lock_input_during_group_playback && longer
}

/// What the hero shows in Contest mode, and nothing in any other.
fn contest_hero(settings: &TrainingSettings) -> Option<ContestHero> {
    (settings.curriculum.char_set_mode == CharSetMode::Contest).then(|| ContestHero::of(settings))
}

impl ContestHero {
    fn of(settings: &TrainingSettings) -> Self {
        let contest = &settings.contest;
        let rules = contest.contest_kind.rules();
        let side = contest.side();
        let fields = std::iter::once("CALL")
            .chain(rules.copied(side).iter().map(|field| field.label()))
            .collect();
        let sends = format!("You send {}, as {}", rules.sends(side), contest.my_call);
        let limit = contest.limit_minutes();
        let length = match contest.contest_minutes {
            0 => ("Open".to_string(), format!("Up to {limit} min")),
            _ => (limit.to_string(), "Minutes".to_string()),
        };
        let (callers, who_calls) = match contest.contest_style {
            RunStyle::Single => (
                ("Single".to_string(), "Caller".to_string()),
                "single caller".to_string(),
            ),
            RunStyle::Pileup => {
                let (count, noun) = pileup_count(settings);
                // A label, as the others have it: "Callers".
                let mut label = noun.to_string();
                label[..1].make_ascii_uppercase();
                ((count, label), pileup_callers(settings))
            }
        };
        let asks = StationAsk::of(contest);
        Self {
            fields,
            sends,
            length,
            callers,
            who_calls,
            asks,
            call_flagged: asks == Some(StationAsk::Call) && !contest.my_call.is_empty(),
            missing: contest.missing(),
        }
    }
}

/// What Home shows of the history: the last session's accuracy, how many
/// sessions there are, the streak and the practice calendar. Worked out
/// once for each change to the history, and each day, not on every redraw
/// of the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Practice {
    pub last_accuracy: Option<f64>,
    pub session_count: usize,
    pub streak: StreakStatus,
    /// None without a session to show, or a day that reads.
    pub calendar: Option<Calendar>,
}

impl Practice {
    /// What Home shows of `history` on `today`. Every session counts,
    /// whatever character set it was recorded with; the streak and the
    /// calendar go by the days sessions count towards, as the clock reads
    /// them.
    pub fn of(history: &History, today: &str) -> Self {
        let (last_accuracy, session_count) = history.with(|sessions| {
            (
                sessions.last().map(|session| session.accuracy),
                sessions.len(),
            )
        });
        let days = history
            .shown()
            .figure("practice days", today.to_string(), |sessions| {
                let dates: Vec<&str> = sessions
                    .iter()
                    .map(|session| session.date.as_str())
                    .collect();
                let calendar = if sessions.is_empty() {
                    None
                } else {
                    Calendar::of(sessions, today)
                };
                (compute_streak_status(&dates, today), calendar)
            });
        Self {
            last_accuracy,
            session_count,
            streak: days.0,
            calendar: days.1.clone(),
        }
    }
}

#[component]
pub fn Home(
    settings: TrainingSettings,
    /// What Home shows of the history.
    practice: Practice,
    pool: String,
    auto_progress: Option<AutoLevelProgress>,
    on_start: EventHandler<()>,
    on_listen: EventHandler<()>,
    /// Your callsign, typed into the hero while a contest has none it can
    /// read.
    #[props(default)]
    on_call: EventHandler<String>,
    /// Your name, typed into the hero while a contest that sends it has
    /// none.
    #[props(default)]
    on_name: EventHandler<String>,
    /// Your state, typed into the hero while a contest that sends it has
    /// none.
    #[props(default)]
    on_state: EventHandler<String>,
    /// Your CQ zone, typed into the hero while a contest that sends it has
    /// none your call says.
    #[props(default)]
    on_zone: EventHandler<String>,
    /// Your section, typed into the hero while a contest that sends it has
    /// none your call tells.
    #[props(default)]
    on_section: EventHandler<String>,
    /// Lock typing while sending is on, keeping groups from being timed,
    /// and you have not been asked yet whether to keep it: it was the
    /// default before Dust 0.16, and a save from then still has it.
    #[props(default)]
    ask_about_lock: bool,
    /// Your answer: true to turn it off and type along, false to keep it.
    #[props(default)]
    on_lock_answer: EventHandler<bool>,
) -> Element {
    let level = level_reading(&settings);
    let contest = contest_hero(&settings);
    // A start the run would refuse says why here, rather than in a toast,
    // unless the line under the call field says it already; and when the
    // call, the name, the state, the zone or the section is what is missing,
    // Start takes you to its field.
    let mut refused = use_signal(|| false);
    // It does so once the hero has drawn the field, from an effect: the
    // press that kept a call may have swapped the call's field for the
    // name's, which the page has only after the press.
    let mut take_to = use_signal(|| None::<fn()>);
    use_effect(move || {
        if let Some(focus) = take_to() {
            focus();
        }
    });
    let missing = contest.as_ref().and_then(|hero| hero.missing.clone());
    let asks = contest.as_ref().and_then(|hero| hero.asks);
    // Enter keeps what was typed in the hero's field, as leaving it does,
    // and then the hero may ask for the next field, or for none: the field
    // goes, and the cursor with it, to nowhere. Once the hero has drawn
    // what comes next, the cursor goes on to its field, or to Start. A
    // field left by Tab or a click leaves the cursor where that took it.
    let mut entered = use_hook(|| CopyValue::new(false));
    use_effect(use_reactive!(|asks| {
        if std::mem::take(&mut *entered.write()) {
            asks.map_or(focus_start as fn(), StationAsk::focus)();
        }
    }));
    let call_flagged = contest.as_ref().is_some_and(|hero| hero.call_flagged);
    let refusal = missing.filter(|_| refused() && !call_flagged);
    // A state or a section typed that is no code is kept as none, and the
    // line under the field says so, however it was kept.
    let state_said = use_signal(|| None::<String>);
    let section_said = use_signal(|| None::<String>);
    // While the hero holds a field, Start and Listen keep the cursor in it,
    // and keep what was typed there before they act. Leaving the box would
    // keep it on the press instead, and redraw the hero under the finger,
    // the button moved, before the click came.
    let draft = TextDraft::use_draft();
    let hold = move |e: MouseEvent| {
        if asks.is_some() {
            keep_focus(e);
        }
    };
    // Each field kept as its own setting keeps it.
    let keep = move |ask: StationAsk, typed: String| match ask {
        StationAsk::Call => on_call.call(typed),
        StationAsk::Name => on_name.call(typed),
        StationAsk::State => {
            let mut said = state_said;
            said.set(state_rejected(&typed));
            on_state.call(typed);
        }
        StationAsk::Zone => on_zone.call(typed),
        StationAsk::Section => {
            let mut said = section_said;
            said.set(section_rejected(&typed));
            on_section.call(typed);
        }
    };
    // What was typed and not kept yet, kept as leaving the box keeps it;
    // and what it was, for Start to decide on.
    let keep_typed = move || {
        let typed = draft.take()?;
        if let Some(ask) = asks {
            keep(ask, typed.clone());
        }
        Some(typed)
    };
    let held = settings.clone();
    let acc = practice
        .last_accuracy
        .map(|a| format!("{}%", (a * 100.0).round()))
        .unwrap_or_else(|| "—".into());
    let session_count = practice.session_count;

    let chars = compute_char_pool(&settings);
    let newest = newest_unlocked(&settings);
    let callsigns = settings.curriculum.char_set_mode.sends_callsigns();
    let words = settings.curriculum.char_set_mode == CharSetMode::Words;
    let speed = range_label(
        settings.playback.char_wpm_min,
        settings.playback.char_wpm_max,
    );
    let size = range_label(
        settings.curriculum.min_group_size as f64,
        settings.curriculum.max_group_size as f64,
    );
    let repeats = range_label(
        settings.playback.group_repeat_min as f64,
        settings.playback.group_repeat_max as f64,
    );
    let pool_len = pool.chars().count();
    rsx! {
        div { class: "stack",
            header { class: "page-head",
                h2 { class: "page-title", "Practice" }
                if contest.is_some() {
                    p { class: "page-sub", "Call CQ and work the stations that answer — or just listen." }
                } else {
                    p { class: "page-sub", "Hear a group and type it back — or just listen." }
                }
            }
            if AUDIO_IS_SILENT {
                div { class: "notice",
                    Icon { name: IconName::Volume }
                    div {
                        div { style: "font-weight: 700;", "This build has no audio" }
                        p { class: "muted", style: "margin: 0.15rem 0 0;",
                            "It is the one for bringing up a new phone toolchain. Sessions still run and score at the right pace; they just play nothing."
                        }
                    }
                }
            }
            if ask_about_lock {
                div { id: "lock-ask", class: "notice",
                    Icon { name: IconName::Timer }
                    div {
                        div { style: "font-weight: 700;", "Lock typing while sending is on" }
                        p { class: "muted", style: "margin: 0.15rem 0 0;",
                            "It was the default before Dust 0.16. While it is on, a group longer than one character is typed only after it has been sent, so none of its characters is timed."
                        }
                        div { class: "row", style: "margin-top: 0.6rem;",
                            button {
                                id: control_id("btn", "type along"),
                                class: "btn btn-primary btn-sm",
                                onclick: move |_| on_lock_answer.call(true),
                                "Type along — turn it off"
                            }
                            button {
                                id: control_id("btn", "keep it on"),
                                class: "btn btn-secondary btn-sm",
                                onclick: move |_| on_lock_answer.call(false),
                                "Keep it on"
                            }
                        }
                    }
                }
            }
            section {
                class: "hero",
                onkeydown: move |e| entered.set(e.key() == Key::Enter),
                div { class: "hero-top",
                    span { class: "eyebrow", if contest.is_some() { "Contest" } else { "Current alphabet" } }
                    span { class: "hero-level",
                        Icon { name: IconName::Flag }
                        "{level.summary}"
                    }
                }
                if let Some(hero) = &contest {
                    if hero.asks == Some(StationAsk::Call) {
                        div { class: "hero-call",
                            CallField {
                                value: settings.contest.my_call.clone(),
                                on_commit: move |typed| {
                                    refused.set(false);
                                    keep(StationAsk::Call, typed);
                                },
                                draft,
                            }
                            CallReading {
                                call: settings.contest.my_call.clone(),
                                kind: settings.contest.contest_kind,
                                zone: settings.contest.zone_from_call(),
                                zone_by: settings.contest.zone_state(),
                            }
                        }
                    } else if hero.asks == Some(StationAsk::Name) {
                        div { class: "hero-call",
                            NameField {
                                value: settings.contest.my_name.clone(),
                                on_commit: move |typed| {
                                    refused.set(false);
                                    keep(StationAsk::Name, typed);
                                },
                                draft,
                            }
                        }
                    } else if hero.asks == Some(StationAsk::State) {
                        div { class: "hero-call",
                            StateEntry {
                                value: settings.contest.my_state.clone(),
                                rejected: state_said(),
                                on_commit: move |typed| {
                                    refused.set(false);
                                    keep(StationAsk::State, typed);
                                },
                                draft,
                            }
                        }
                    } else if hero.asks == Some(StationAsk::Zone) {
                        div { class: "hero-call",
                            ZoneField {
                                value: settings.contest.my_zone,
                                on_commit: move |typed| {
                                    refused.set(false);
                                    keep(StationAsk::Zone, typed);
                                },
                                draft,
                            }
                        }
                    } else if hero.asks == Some(StationAsk::Section) {
                        div { class: "hero-call",
                            SectionEntry {
                                value: settings.contest.my_section.clone(),
                                rejected: section_said(),
                                on_commit: move |typed| {
                                    refused.set(false);
                                    keep(StationAsk::Section, typed);
                                },
                                draft,
                            }
                        }
                    } else {
                        div { class: "pool-chips",
                            for field in hero.fields.iter() {
                                span { class: "pool-chip field-chip", "{field}" }
                            }
                        }
                        p { class: "hero-sends", "{hero.sends}" }
                    }
                } else if chars.is_empty() {
                    p { class: "muted", style: "color: var(--hero-ink-soft);", "No characters unlocked yet." }
                } else {
                    div { class: "pool-chips",
                        for ch in chars.iter().copied() {
                            if Some(ch) == newest {
                                span { class: "pool-chip fresh", title: "Newest character", "{ch}" }
                            } else {
                                span { class: "pool-chip", "{ch}" }
                            }
                        }
                    }
                }
                div { class: "hero-specs",
                    div {
                        span { class: "spec-value", "{speed}" }
                        span { class: "spec-label", "WPM" }
                    }
                    if let Some(hero) = &contest {
                        div {
                            span { class: "spec-value", "{hero.length.0}" }
                            span { class: "spec-label", "{hero.length.1}" }
                        }
                        div {
                            span { class: "spec-value", "{hero.callers.0}" }
                            span { class: "spec-label", "{hero.callers.1}" }
                        }
                    } else if callsigns || words {
                        div {
                            span { class: "spec-value", "{settings.curriculum.num_groups}" }
                            span { class: "spec-label", if callsigns { "Calls" } else { "Words" } }
                        }
                    } else {
                        div {
                            span { class: "spec-value", "{settings.curriculum.num_groups} × {size}" }
                            span { class: "spec-label", "Groups" }
                        }
                    }
                    if contest.is_none() {
                        div {
                            span { class: "spec-value", "{repeats}×" }
                            span { class: "spec-label", "Sent" }
                        }
                    }
                }
                if let Some(refusal) = refusal {
                    p { id: "home-missing", class: "hero-missing", role: "alert", "{refusal}" }
                }
                div { class: "hero-actions",
                    button {
                        id: control_id("btn", "start training"),
                        class: "btn btn-primary",
                        onmousedown: hold,
                        onclick: move |_| {
                            // Decided on the settings as keeping what was
                            // typed leaves them, which the hero shows from
                            // the next render on. The run starts here, in
                            // the press, as a browser lets sound start only
                            // in one.
                            let mut settings = held.clone();
                            if let Some(typed) = keep_typed()
                                && let Some(ask) = asks
                            {
                                ask.keep(&mut settings.contest, &typed);
                            }
                            let hero = contest_hero(&settings).filter(|hero| hero.missing.is_some());
                            let Some(hero) = hero else {
                                on_start.call(());
                                return;
                            };
                            refused.set(true);
                            if let Some(ask) = hero.asks {
                                take_to.set(Some(ask.focus()));
                            }
                        },
                        Icon { name: IconName::Play }
                        if contest.is_some() { "Start contest" } else { "Start training" }
                    }
                    button {
                        id: control_id("btn", "listen to letters"),
                        class: "btn btn-hero",
                        onmousedown: hold,
                        onclick: move |_| {
                            keep_typed();
                            on_listen.call(());
                        },
                        Icon { name: IconName::Headphones }
                        "Listen"
                    }
                }
            }
            if session_count > 0 {
                div { class: "grid-3",
                    div { class: "kpi emerald",
                        div { class: "tiny", "Last accuracy" }
                        div { class: "value", "{acc}" }
                    }
                    div { class: "kpi indigo",
                        div { class: "tiny", "Sessions" }
                        div { class: "value", "{session_count}" }
                    }
                    div { class: "kpi blue",
                        div { class: "tiny", "{level.label}" }
                        div { class: "value", "{level.value}" }
                    }
                }
            } else if let Some(hero) = &contest {
                div { class: "card",
                    CardHead {
                        icon: IconName::Sparkle,
                        title: "First contest",
                        note: format!("{} · {}", level.summary, hero.who_calls),
                    }
                    p { class: "muted hint",
                        {hero.asks.map_or("Press Start contest; Enter sends what the QSO needs next.", StationAsk::first_step)}
                    }
                }
            } else {
                div { class: "card",
                    CardHead {
                        icon: IconName::Sparkle,
                        title: "First session",
                        note: format!("{pool_len} characters are in the pool right now."),
                    }
                    p { class: "muted hint",
                        if words {
                            "Type the word you hear — the trainer scores every character of it."
                        } else {
                            "Type what you hear — the trainer scores every character, then moves the level for you."
                        }
                    }
                }
            }
            StreakCard { status: practice.streak }
            if practice.calendar.is_some() {
                ActivityHeatmap { calendar: practice.calendar }
            }
            if let Some(progress) = auto_progress {
                AutoLevelCard { progress }
            }
            TipsCarousel { tips: if contest.is_some() { CONTEST_TIPS } else { TIPS } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContestHero, LevelReading, StationAsk, level_reading, lock_keeps_groups_untimed};
    use crate::ui::widgets::range_label;
    use cw_core::{CharSetMode, RunStyle, TrainingSettings};

    #[test]
    fn the_level_reading_follows_the_mode() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 7;
        settings.curriculum.digits_level = 3;
        settings.curriculum.callsign_level = 2;
        let reading = |settings: &TrainingSettings, mode| {
            let mut settings = settings.clone();
            settings.curriculum.char_set_mode = mode;
            level_reading(&settings)
        };
        assert_eq!(reading(&settings, CharSetMode::Koch).summary, "Level 7");
        assert_eq!(
            reading(&settings, CharSetMode::Digits).summary,
            "Digits level 3"
        );
        assert_eq!(
            reading(&settings, CharSetMode::Mixed),
            LevelReading {
                summary: "Level 7 · digits 3".to_string(),
                label: "Levels",
                value: "7 / 3".to_string(),
            }
        );
        assert_eq!(
            reading(&settings, CharSetMode::Callsign).summary,
            "Callsign tier 2"
        );
        // A contest is told by its name, and its tile shows the tier that
        // shapes its callers.
        assert_eq!(
            reading(&settings, CharSetMode::Contest),
            LevelReading {
                summary: "CQ WPX".to_string(),
                label: "Callsign tier",
                value: "2".to_string(),
            }
        );
    }

    /// The lock keeps a group from being timed only where groups are
    /// longer than one character, and a contest's fields never lock.
    #[test]
    fn the_lock_keeps_only_longer_groups_untimed() {
        let mut settings = TrainingSettings::default();
        settings.playback.lock_input_during_group_playback = true;
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.min_group_size = 1;
        settings.curriculum.max_group_size = 1;
        assert!(!lock_keeps_groups_untimed(&settings), "one character");
        settings.curriculum.max_group_size = 2;
        assert!(lock_keeps_groups_untimed(&settings));
        settings.curriculum.max_group_size = 1;
        for (mode, untimed) in [
            (CharSetMode::Callsign, true),
            (CharSetMode::Words, true),
            (CharSetMode::Contest, false),
        ] {
            settings.curriculum.char_set_mode = mode;
            assert_eq!(lock_keeps_groups_untimed(&settings), untimed, "{mode:?}");
        }
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.playback.lock_input_during_group_playback = false;
        assert!(!lock_keeps_groups_untimed(&settings), "off");
    }

    /// The hero says what a run copies and sends, and asks for your call
    /// until it has one a contest can read.
    #[test]
    fn a_contest_hero_says_what_a_run_is() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, Some(StationAsk::Call));
        assert_eq!(hero.fields, ["CALL", "NR"]);
        assert_eq!(hero.length, ("10".to_string(), "Minutes".to_string()));
        assert_eq!(
            hero.missing.as_deref(),
            Some("Add your callsign to start a contest")
        );
        assert!(!hero.call_flagged, "nothing typed for the line to flag");
        settings.contest.my_call = "ABC".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, Some(StationAsk::Call));
        assert!(hero.call_flagged, "not a call yet");
        settings.contest.my_call = "DL1ABC".to_string();
        settings.contest.contest_minutes = 0;
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, None, "WPX sends no name");
        assert_eq!(hero.missing, None);
        assert_eq!(hero.sends, "You send 5NN + serial, as DL1ABC");
        assert_eq!(
            hero.length,
            ("Open".to_string(), "Up to 60 min".to_string())
        );
    }

    /// The specs say who calls: a single caller, or as many as a pile-up
    /// draws from the Band card's stations range.
    #[test]
    fn a_contest_hero_says_who_calls() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.callers, ("Single".to_string(), "Caller".to_string()));
        assert_eq!(hero.who_calls, "single caller");
        settings.contest.contest_style = RunStyle::Pileup;
        settings.band.stations_min = 1;
        settings.band.stations_max = 3;
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.callers, ("1–3".to_string(), "Callers".to_string()));
        assert_eq!(hero.who_calls, "1–3 callers");
        settings.band.stations_max = 1;
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.callers, ("1".to_string(), "Caller".to_string()));
        assert_eq!(hero.who_calls, "1 caller");
    }

    /// A Simple QSO copies a report and a name, sends yours, and asks for
    /// your name once the call reads, saying why a start is refused.
    #[test]
    fn a_simple_qso_hero_asks_for_your_name() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        settings.contest.contest_kind = cw_core::ContestKind::SimpleQso;
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, Some(StationAsk::Call), "the call first");
        settings.contest.my_call = "DL1ABC".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, Some(StationAsk::Name));
        assert_eq!(hero.fields, ["CALL", "RST", "NAME"]);
        assert_eq!(
            hero.missing.as_deref(),
            Some("Add your name to start a Simple\u{a0}QSO")
        );
        settings.contest.my_name = "MAT".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, None);
        assert_eq!(hero.missing, None);
        assert_eq!(hero.sends, "You send 599 + name, as DL1ABC");
    }

    /// The NA Sprint sends your name and your state: once the call reads,
    /// the hero asks for the name, then the state, and sends what you send
    /// in the order it goes; a call outside the US and Canada sends DX, so
    /// it asks for no state.
    #[test]
    fn a_sprint_hero_asks_for_your_name_then_your_state() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        settings.contest.contest_kind = cw_core::ContestKind::Sprint;
        settings.contest.my_call = "K1ABC".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, Some(StationAsk::Name));
        assert_eq!(hero.fields, ["CALL", "NR", "NAME", "ST"]);
        settings.contest.my_name = "MIKE".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, Some(StationAsk::State));
        assert_eq!(
            hero.missing.as_deref(),
            Some("Add your state or province to start an NA\u{a0}Sprint")
        );
        settings.contest.my_state = "MA".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, None);
        assert_eq!(hero.missing, None);
        assert_eq!(hero.sends, "You send serial + name + state, as K1ABC");
        settings.contest.my_state.clear();
        settings.contest.my_call = "DL1ABC".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, None, "DX, outside the US and Canada");
        assert_eq!(hero.missing, None);
    }

    /// ARRL DX asks by your side: as DX it asks for nothing more, as you
    /// send your power, always given, and copy a state; as W/VE it asks for
    /// your state and copies power.
    #[test]
    fn an_arrl_dx_hero_asks_by_your_side() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        settings.contest.contest_kind = cw_core::ContestKind::ArrlDx;
        settings.contest.my_call = "DL1ABC".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, None);
        assert_eq!(hero.missing, None);
        assert_eq!(hero.fields, ["CALL", "ST"]);
        assert_eq!(hero.sends, "You send 5NN + power, as DL1ABC");
        settings.contest.my_call = "K1ABC".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, Some(StationAsk::State));
        assert_eq!(hero.fields, ["CALL", "PWR"]);
        assert_eq!(
            hero.missing.as_deref(),
            Some("Add your state or province to start an ARRL\u{a0}DX")
        );
        settings.contest.my_state = "MA".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, None);
        assert_eq!(hero.sends, "You send 5NN + state, as K1ABC");
    }

    /// CQ WW takes your zone from your call, and asks for it only where
    /// your call does not say it, in a field of its own, as the first
    /// contest's card says.
    #[test]
    fn a_cqww_hero_asks_for_your_zone_where_your_call_does_not_say_it() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        settings.contest.contest_kind = cw_core::ContestKind::Cqww;
        settings.contest.my_call = "DL1ABC".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, None);
        assert_eq!(hero.fields, ["CALL", "ZN"]);
        assert_eq!(hero.sends, "You send 5NN + zone, as DL1ABC");
        settings.contest.my_call = "C31AB".to_string();
        let hero = ContestHero::of(&settings);
        assert_eq!(hero.asks, Some(StationAsk::Zone));
        assert_eq!(
            hero.missing.as_deref(),
            Some("Add your CQ zone to start a CQ\u{a0}WW")
        );
        assert!(
            StationAsk::Zone
                .first_step()
                .starts_with("Add your CQ zone,")
        );
        StationAsk::Zone.keep(&mut settings.contest, " 27 ");
        assert_eq!(settings.contest.my_zone, 27);
        let hero = ContestHero::of(&settings);
        assert_eq!((hero.asks, hero.missing), (None, None));
    }

    /// A level stored for a longer alphabet is kept for it, but not shown as
    /// the level of a shorter one: Mixed has only its letters to climb.
    #[test]
    fn the_level_shown_is_the_one_the_alphabet_in_use_reaches() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 38;
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        let mixed = level_reading(&settings);
        assert_ne!(mixed.summary, "Level 38 · digits 1", "{mixed:?}");
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        assert_eq!(level_reading(&settings).summary, "Level 38");
    }

    #[test]
    fn a_fixed_value_reads_as_one_number() {
        assert_eq!(range_label(18.0, 18.0), "18");
        assert_eq!(range_label(18.0, 25.0), "18–25");
        assert_eq!(range_label(0.7, 1.0), "0.7–1");
        assert_eq!(range_label(-20.0, 0.0), "\u{2212}20 to 0");
        assert_eq!(range_label(-20.0, -20.0004), "\u{2212}20");
    }
}
