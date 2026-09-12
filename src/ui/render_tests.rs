//! Smoke tests that render every screen to HTML.
//!
//! They run the real component bodies — the rsx, the formatting, the SVG
//! geometry — so a panic or a broken template shows up here instead of on a
//! phone. Effects and audio never run: the virtual DOM is rebuilt in place and
//! thrown away.

use cw_core::{
    AutoLevelProgress, CharSetMode, GroupResult, MixedAutoLevelAxis, SessionResult, SessionTiming,
    StreakState, StreakStatus, TrainingSettings,
};
use dioxus::prelude::*;

use super::auto_level::AutoLevelCard;
use super::envelope::EnvelopeCard;
use super::heatmap::{StreakCard, StreakCardProps};
use super::home::Home;
use super::listen::ListenView;
use super::results::ResultsView;
use super::settings::SettingsView;
use super::stats::{StatsView, StatsViewProps};
use super::training::TrainingView;

fn render(app: fn() -> Element) -> String {
    let mut dom = VirtualDom::new(app);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// For cases that need to feed props in from the test body.
fn render_props<P: Clone + 'static, M: 'static>(
    component: impl dioxus::core::ComponentFunction<P, M>,
    props: P,
) -> String {
    let mut dom = VirtualDom::new_with_props(component, props);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

fn session(date: &str, accuracy: f64) -> SessionResult {
    SessionResult {
        date: date.to_string(),
        timestamp: 1_757_000_000_000,
        started_at: 1_757_000_000_000,
        finished_at: 1_757_000_060_000,
        groups: vec![
            GroupResult {
                sent: "KM".into(),
                received: "KM".into(),
                correct: true,
            },
            GroupResult {
                sent: "UR".into(),
                received: "UK".into(),
                correct: false,
            },
        ],
        group_timings: vec![SessionTiming {
            time_to_complete_ms: 900.0,
            per_char_ms: 450.0,
            char_wpm: Some(20.0),
        }],
        accuracy,
        letter_accuracy: Default::default(),
        alphabet_size: 4,
        avg_response_ms: 450.0,
        total_chars: 40,
        effective_alphabet_size: 3.4,
        score: 820.0,
        level: 3,
        digits_level: 1,
        char_set_mode: CharSetMode::Mixed,
        char_wpm: 20.0,
        effective_wpm: 18.0,
        alphabet_fingerprint: String::new(),
    }
}

fn history() -> Vec<SessionResult> {
    vec![
        session("2026-09-01", 0.72),
        session("2026-09-02", 0.86),
        session("2026-09-03", 0.94),
    ]
}

#[test]
fn home_renders_the_hero_and_stats() {
    let html = render(|| {
        rsx! {
            Home {
                settings: TrainingSettings::default(),
                last_accuracy: Some(0.94),
                session_count: 3,
                pool: "KM01".to_string(),
                sessions: history(),
                today: "2026-09-03".to_string(),
                auto_progress: None,
                on_start: move |_| {},
                on_listen: move |_| {},
            }
        }
    });
    assert!(html.contains("Current alphabet"));
    assert!(html.contains("Start training"));
    assert!(html.contains("class=\"pool-chip fresh\""));
    assert!(html.contains("94%"));
    // Desktop and web builds have audio, so the silent-build notice stays away.
    assert_eq!(
        html.contains("no audio yet"),
        super::super::audio::AUDIO_IS_SILENT
    );
}

#[test]
fn home_without_history_invites_a_first_session() {
    let html = render(|| {
        rsx! {
            Home {
                settings: TrainingSettings::default(),
                last_accuracy: None,
                session_count: 0,
                pool: "KM".to_string(),
                sessions: Vec::new(),
                today: "2026-09-03".to_string(),
                auto_progress: None,
                on_start: move |_| {},
                on_listen: move |_| {},
            }
        }
    });
    assert!(html.contains("First session"));
    assert!(!html.contains("Last accuracy"));
}

#[test]
fn stats_count_sessions_from_every_character_set() {
    // A history recorded in Digits mode, read while the app is set to Mixed.
    let mut digits = session("2026-09-01", 0.9);
    digits.char_set_mode = CharSetMode::Digits;
    digits.alphabet_fingerprint = "0123456789".to_string();
    let mut koch = session("2026-09-02", 0.7);
    koch.char_set_mode = CharSetMode::Koch;
    koch.alphabet_fingerprint = "KMURESNAPTLWI".to_string();

    let html = render_props(
        StatsView,
        StatsViewProps {
            settings: TrainingSettings::default(),
            sessions: vec![digits, koch],
        },
    );
    assert!(
        !html.contains("Nothing scored yet"),
        "history must not be hidden"
    );
    assert!(html.contains("2 sessions ·"));
    // Average of 90% and 70%.
    assert!(html.contains("80%"));
}

#[test]
fn auto_level_card_shows_both_meters() {
    let html = render(|| {
        rsx! {
            AutoLevelCard {
                progress: AutoLevelProgress {
                    threshold: 90.0,
                    above_count: 3,
                    above_target: 5,
                    below_count: 0,
                    below_target: 1,
                    above_disabled: false,
                    below_disabled: false,
                    alternating_mixed: true,
                    next_mixed_axis: Some(MixedAutoLevelAxis::Letters),
                },
            }
        }
    });
    assert!(html.contains("Auto level"));
    assert!(html.contains("3/5"));
    assert!(html.contains("90% to pass"));
}

#[test]
fn streak_card_renders_each_state() {
    for (state, needle) in [
        (StreakState::Safe, "in the bag"),
        (StreakState::AtRisk, "at risk"),
        (StreakState::Lost, "ended"),
    ] {
        let html = render_props(
            StreakCard,
            StreakCardProps {
                status: StreakStatus {
                    state,
                    days: 9,
                    freezes_available: 1,
                    freezes_used: 0,
                    lost_streak_days: Some(4),
                },
            },
        );
        assert!(html.contains(needle), "{state:?} should render {needle}");
    }
}

#[test]
fn the_app_shell_renders_the_practice_screen() {
    let html = render(crate::app::App);
    assert!(html.contains("class=\"app-root\""));
    assert!(html.contains("brand-mark"));
    assert!(html.contains("bottom-nav"));
    // Home is the landing screen.
    assert!(html.contains("Start training"));
    // Head tags live in their own component so they mount once.
    assert!(html.contains("class=\"screen\""));
    assert!(html.contains("id=\"screen-home\""));
}

#[test]
fn every_icon_draws_something() {
    for name in [
        "signal",
        "chart",
        "sliders",
        "play",
        "stop",
        "headphones",
        "check",
        "x",
        "chevron",
        "back",
        "sun",
        "moon",
        "auto",
        "envelope",
        "gauge",
        "waves",
        "target",
        "shuffle",
        "repeat",
        "letters",
        "timer",
        "volume",
        "trophy",
        "sparkle",
        "flag",
        "nonsense",
    ] {
        let html = render_props(
            super::widgets::Icon,
            super::widgets::IconProps {
                name: name.to_string(),
            },
        );
        assert!(html.contains("<svg"), "{name} rendered no svg");
        assert!(html.contains("<path"), "{name} rendered no path");
    }
}

#[test]
fn settings_renders_the_digit_and_custom_modes() {
    let digits = render(|| {
        let settings = use_signal(|| {
            let mut settings = TrainingSettings::default();
            settings.curriculum.char_set_mode = CharSetMode::Digits;
            settings
        });
        rsx! {
            SettingsView {
                settings,
                previewing: true,
                sample_playing: None,
                on_preview_band: move |_| {},
                on_stop_band: move |_| {},
                on_play_sample: move |_: String| {},
            }
        }
    });
    assert!(digits.contains("Digits only"));
    // No unlock order for digits, and the live preview swaps to a stop control.
    assert!(!digits.contains("Unlock order"));
    assert!(digits.contains("Looping"));

    let custom = render(|| {
        let settings = use_signal(|| {
            let mut settings = TrainingSettings::default();
            settings.curriculum.char_set_mode = CharSetMode::Custom;
            settings.curriculum.custom_set = vec!['K', 'M', 'R'];
            settings
        });
        rsx! {
            SettingsView {
                settings,
                previewing: false,
                sample_playing: None,
                on_preview_band: move |_| {},
                on_stop_band: move |_| {},
                on_play_sample: move |_: String| {},
            }
        }
    });
    assert!(custom.contains("Custom alphabet"));
    assert!(
        custom.contains("your own character list") || custom.contains("Your own character list")
    );
}

#[test]
fn settings_renders_every_card() {
    let html = render(|| {
        let settings = use_signal(TrainingSettings::default);
        rsx! {
            SettingsView {
                settings,
                previewing: false,
                sample_playing: None,
                on_preview_band: move |_| {},
                on_stop_band: move |_| {},
                on_play_sample: move |_: String| {},
            }
        }
    });
    for needle in [
        "Character set",
        "Session shape",
        "Sends per group",
        "Speed",
        // dioxus-ssr escapes the ampersand, so match the card note instead.
        "Side tone pitch and sending level",
        "Keying envelope",
        "Auto level",
        "Character sampling",
        "Band conditions",
    ] {
        assert!(html.contains(needle), "settings should render {needle}");
    }
}

#[test]
fn envelope_card_draws_a_closed_trace() {
    let html = render(|| {
        let settings = use_signal(TrainingSettings::default);
        rsx! {
            EnvelopeCard {
                settings,
                sample_playing: Some("T".to_string()),
                on_play: move |_: String| {},
                on_stop: move |_| {},
            }
        }
    });
    assert!(html.contains("Keying envelope"));
    // The trace is a filled shape: it must start with a move and close.
    assert!(html.contains("d=\"M"), "envelope path missing");
    assert!(html.contains(" Z\""), "envelope path is not closed");
    // The chip that is playing swaps to a stop control.
    assert!(html.contains("test-chip playing"));
}

#[test]
fn stats_renders_the_chart_with_one_session() {
    let html = render(|| {
        rsx! {
            StatsView { settings: TrainingSettings::default(), sessions: vec![session("2026-09-01", 0.89)] }
        }
    });
    assert!(html.contains("Accuracy over time"));
    assert!(html.contains("1 session ·"));
    assert!(
        html.contains("<polyline"),
        "a single session still draws a line"
    );
    assert!(
        html.contains("<circle"),
        "a single session still draws its point"
    );
}

#[test]
fn stats_is_empty_without_sessions() {
    let html = render(|| {
        rsx! {
            StatsView { settings: TrainingSettings::default(), sessions: Vec::new() }
        }
    });
    assert!(html.contains("Nothing scored yet"));
    assert!(!html.contains("<polyline"));
}

#[test]
fn results_renders_the_score_ring_and_rows() {
    let html = render(|| {
        rsx! {
            ResultsView {
                result: session("2026-09-03", 0.5),
                auto_message: Some("Unlocked a new character".to_string()),
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    assert!(html.contains("Session complete"));
    assert!(html.contains("score-ring"));
    assert!(html.contains("Unlocked a new character"));
    assert!(html.contains("50%"));
    assert!(html.contains("1/2"));
}

#[test]
fn listen_renders_the_selected_character() {
    let html = render(|| {
        rsx! {
            ListenView {
                settings: TrainingSettings::default(),
                playing: false,
                on_play: move |_: String| {},
                on_stop: move |_| {},
                on_back: move |_| {},
            }
        }
    });
    assert!(html.contains("listen-glyph"));
    assert!(html.contains("letter-chip"));
    assert!(html.contains("Play all"));
}

#[test]
fn training_shows_the_send_counter_while_repeating() {
    let html = render(|| {
        rsx! {
            TrainingView {
                current: 0,
                total: 3,
                groups: vec!["KM".to_string(), String::new(), String::new()],
                inputs: vec![String::new(), String::new(), String::new()],
                confirmed: vec![false, false, false],
                focused: 0,
                playing: true,
                locked: true,
                repeat_total: 2,
                repeat_done: 1,
                on_change: move |_: (usize, String)| {},
                on_confirm: move |_: usize| {},
                on_focus: move |_: usize| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });
    assert!(html.contains("Sending 2 of 2"));
    assert!(html.contains("Send 2/2"));
    assert!(html.contains("2× per group"));
    assert!(html.contains("answer locked"));
}

#[test]
fn training_without_repeats_omits_the_counter() {
    let html = render(|| {
        rsx! {
            TrainingView {
                current: 1,
                total: 2,
                groups: vec!["KM".to_string(), "UR".to_string()],
                inputs: vec!["KM".to_string(), String::new()],
                confirmed: vec![true, false],
                focused: 1,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                on_change: move |_: (usize, String)| {},
                on_confirm: move |_: usize| {},
                on_focus: move |_: usize| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });
    assert!(html.contains("Your turn"));
    assert!(!html.contains("per group"));
    // The confirmed group shows its comparison.
    assert!(html.contains("ch sent-row"));
}
