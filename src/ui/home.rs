use cw_core::{
    AutoLevelProgress, CharSetMode, LevelAxis, SessionResult, Side, TrainingSettings,
    compute_char_pool, compute_streak_status, newest_unlocked, parse_callsign,
};
use dioxus::prelude::*;

use crate::audio::AUDIO_IS_SILENT;
use crate::ui::auto_level::AutoLevelCard;
use crate::ui::heatmap::{ActivityHeatmap, StreakCard};
use crate::ui::settings_station::{
    CallField, CallReading, NameField, focus_call_field, focus_name_field,
};
use crate::ui::tips::{CONTEST_TIPS, TIPS, TipsCarousel};
use crate::ui::widgets::{CardHead, Icon, IconName, control_id, pretty_number};

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
    /// Your call is missing, or not one a contest can read: the hero asks
    /// for it.
    needs_call: bool,
    /// The call reads, but the contest sends your name and you have given
    /// none: the hero asks for it.
    needs_name: bool,
    /// A call is typed that a contest cannot read, so the line under the
    /// field already says what is wrong with it.
    call_flagged: bool,
    /// What a run still needs before it can start, said as the settings say
    /// it.
    missing: Option<String>,
}

impl ContestHero {
    fn of(settings: &TrainingSettings) -> Self {
        let contest = &settings.contest;
        let rules = contest.contest_kind.rules();
        let side = Side::of_call(&contest.my_call);
        let fields = std::iter::once("CALL")
            .chain(
                rules
                    .into_iter()
                    .flat_map(|rules| rules.copied(side).iter().map(|field| field.label())),
            )
            .collect();
        let sends = rules.map_or_else(String::new, |rules| {
            format!("You send {}, as {}", rules.sends(), contest.my_call)
        });
        let limit = contest.limit_minutes();
        let length = match contest.contest_minutes {
            0 => ("Open".to_string(), format!("Up to {limit} min")),
            _ => (limit.to_string(), "Minutes".to_string()),
        };
        let needs_call = parse_callsign(&contest.my_call).is_none();
        let sends_name = rules.is_some_and(|rules| rules.sends_name());
        Self {
            fields,
            sends,
            length,
            needs_call,
            needs_name: !needs_call && sends_name && contest.my_name.is_empty(),
            call_flagged: needs_call && !contest.my_call.is_empty(),
            missing: contest.missing(),
        }
    }
}

/// A range as it reads: one number when its ends meet.
pub(crate) fn range_label(min: f64, max: f64) -> String {
    if (min - max).abs() < f64::EPSILON {
        pretty_number(min)
    } else {
        format!("{}–{}", pretty_number(min), pretty_number(max))
    }
}

#[component]
pub fn Home(
    settings: TrainingSettings,
    last_accuracy: Option<f64>,
    session_count: usize,
    pool: String,
    sessions: Vec<SessionResult>,
    today: String,
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
) -> Element {
    let level = level_reading(&settings);
    let contest = (settings.curriculum.char_set_mode == CharSetMode::Contest)
        .then(|| ContestHero::of(&settings));
    // A start the run would refuse says why here, rather than in a toast,
    // unless the line under the call field says it already; and when the
    // call is what is missing, Start takes you to its field.
    let mut refused = use_signal(|| false);
    let missing = contest.as_ref().and_then(|hero| hero.missing.clone());
    let needs_call = contest.as_ref().is_some_and(|hero| hero.needs_call);
    let needs_name = contest.as_ref().is_some_and(|hero| hero.needs_name);
    let call_flagged = contest.as_ref().is_some_and(|hero| hero.call_flagged);
    let refusal = missing.clone().filter(|_| refused() && !call_flagged);
    let acc = last_accuracy
        .map(|a| format!("{}%", (a * 100.0).round()))
        .unwrap_or_else(|| "—".into());
    let dates: Vec<String> = sessions.iter().map(|s| s.date.clone()).collect();
    let streak = compute_streak_status(&dates, &today);

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
            section { class: "hero",
                div { class: "hero-top",
                    span { class: "eyebrow", if contest.is_some() { "Contest" } else { "Current alphabet" } }
                    span { class: "hero-level",
                        Icon { name: IconName::Flag }
                        "{level.summary}"
                    }
                }
                if let Some(hero) = &contest {
                    if hero.needs_call {
                        div { class: "hero-call",
                            CallField {
                                value: settings.contest.my_call.clone(),
                                on_commit: move |typed| {
                                    refused.set(false);
                                    on_call.call(typed);
                                },
                            }
                            CallReading { call: settings.contest.my_call.clone() }
                        }
                    } else if hero.needs_name {
                        div { class: "hero-call",
                            NameField {
                                value: settings.contest.my_name.clone(),
                                on_commit: move |typed| {
                                    refused.set(false);
                                    on_name.call(typed);
                                },
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
                        // One station answers each CQ until pile-ups come.
                        div {
                            span { class: "spec-value", "Single" }
                            span { class: "spec-label", "Caller" }
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
                        onclick: move |_| {
                            if missing.is_none() {
                                on_start.call(());
                                return;
                            }
                            refused.set(true);
                            if needs_call {
                                focus_call_field();
                            } else if needs_name {
                                focus_name_field();
                            }
                        },
                        Icon { name: IconName::Play }
                        if contest.is_some() { "Start contest" } else { "Start training" }
                    }
                    button {
                        id: control_id("btn", "listen to letters"),
                        class: "btn btn-hero",
                        onclick: move |_| on_listen.call(()),
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
                        note: format!("{} · single caller", level.summary),
                    }
                    p { class: "muted", style: "margin: 0;",
                        if hero.needs_call {
                            "Add your call, press Start contest; Enter sends what the QSO needs next."
                        } else if hero.needs_name {
                            "Add your name, press Start contest; Enter sends what the QSO needs next."
                        } else {
                            "Press Start contest; Enter sends what the QSO needs next."
                        }
                    }
                }
            } else {
                div { class: "card",
                    CardHead {
                        icon: IconName::Sparkle,
                        title: "First session",
                        note: format!("{pool_len} characters are in the pool right now."),
                    }
                    p { class: "muted", style: "margin: 0;",
                        if words {
                            "Type the word you hear — the trainer scores every character of it."
                        } else {
                            "Type what you hear — the trainer scores every character, then moves the level for you."
                        }
                    }
                }
            }
            StreakCard { status: streak }
            if !sessions.is_empty() {
                ActivityHeatmap { sessions, today }
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
    use super::{ContestHero, LevelReading, level_reading, range_label};
    use cw_core::{CharSetMode, TrainingSettings};

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

    /// The hero says what a run copies and sends, and asks for your call
    /// until it has one a contest can read.
    #[test]
    fn a_contest_hero_says_what_a_run_is() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        let hero = ContestHero::of(&settings);
        assert!(hero.needs_call);
        assert_eq!(hero.fields, ["CALL", "NR"]);
        assert_eq!(hero.length, ("10".to_string(), "Minutes".to_string()));
        assert_eq!(
            hero.missing.as_deref(),
            Some("Add your callsign to start a contest")
        );
        assert!(!hero.call_flagged, "nothing typed for the line to flag");
        settings.contest.my_call = "ABC".to_string();
        let hero = ContestHero::of(&settings);
        assert!(hero.needs_call && hero.call_flagged, "not a call yet");
        settings.contest.my_call = "DL1ABC".to_string();
        settings.contest.contest_minutes = 0;
        let hero = ContestHero::of(&settings);
        assert!(!hero.needs_call);
        assert_eq!(hero.missing, None);
        assert_eq!(hero.sends, "You send 5NN + serial, as DL1ABC");
        assert_eq!(
            hero.length,
            ("Open".to_string(), "Up to 60 min".to_string())
        );
        assert!(!hero.needs_name, "WPX sends no name");
    }

    /// A Simple QSO copies a report and a name, sends yours, and asks for
    /// your name once the call reads, saying why a start is refused.
    #[test]
    fn a_simple_qso_hero_asks_for_your_name() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        settings.contest.contest_kind = cw_core::ContestKind::SimpleQso;
        let hero = ContestHero::of(&settings);
        assert!(hero.needs_call && !hero.needs_name, "the call first");
        settings.contest.my_call = "DL1ABC".to_string();
        let hero = ContestHero::of(&settings);
        assert!(!hero.needs_call && hero.needs_name);
        assert_eq!(hero.fields, ["CALL", "RST", "NAME"]);
        assert_eq!(
            hero.missing.as_deref(),
            Some("Add your name to start a Simple QSO")
        );
        settings.contest.my_name = "MAT".to_string();
        let hero = ContestHero::of(&settings);
        assert!(!hero.needs_name);
        assert_eq!(hero.missing, None);
        assert_eq!(hero.sends, "You send 599 + name, as DL1ABC");
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
    }
}
