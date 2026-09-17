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
use super::scope::Heard;
use super::settings::SettingsView;
use super::stats::{StatsView, StatsViewProps};
use super::training::{TrainingScope, TrainingView};

/// Same shape the router draws: the scope is a sibling of the answer list,
/// not a child of it.
#[component]
fn TrainingScreen(
    current: usize,
    total: usize,
    groups: Vec<String>,
    inputs: Vec<String>,
    confirmed: Vec<bool>,
    focused: usize,
    playing: bool,
    locked: bool,
    repeat_total: u32,
    repeat_done: u32,
    settings: TrainingSettings,
    heard: Vec<Heard>,
    send_id: u64,
    on_change: EventHandler<(usize, String)>,
    on_confirm: EventHandler<usize>,
    on_focus: EventHandler<usize>,
    on_submit: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "stack",
            TrainingScope {
                focused,
                total,
                playing,
                repeat_total,
                repeat_done,
                settings,
                heard,
                send_id,
            }
            TrainingView {
                current,
                groups,
                inputs,
                confirmed,
                focused,
                playing,
                locked,
                repeat_total,
                repeat_done,
                on_change,
                on_confirm,
                on_focus,
                on_submit,
                on_stop,
            }
        }
    }
}

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
fn the_scope_shows_the_band_without_showing_the_answer() {
    use cw_core::timing::StationVoice;

    fn voice(tone_hz: f64, volume: f64) -> StationVoice {
        StationVoice {
            tone_hz,
            char_wpm: 20.0,
            effective_wpm: 20.0,
            volume,
            weight: 1.0,
            dash_ratio: 3.0,
        }
    }
    let html = render(|| {
        rsx! {
            TrainingScreen {
                current: 0,
                total: 1,
                groups: vec!["QRXZJ".to_string()],
                inputs: vec![String::new()],
                confirmed: vec![false],
                focused: 0,
                playing: true,
                locked: true,
                repeat_total: 1,
                repeat_done: 0,
                settings: TrainingSettings::default(),
                heard: vec![
                    Heard { voice: voice(520.0, 1.0), wanted: true, key: vec![(0.0, 1.0)], rise_sec: 0.005 },
                    Heard { voice: voice(700.0, 0.4), wanted: false, key: vec![(0.0, 1.0)], rise_sec: 0.005 },
                ],
                send_id: 0,
                on_change: move |_: (usize, String)| {},
                on_confirm: move |_: usize| {},
                on_focus: move |_: usize| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });

    // The band is on screen: the filter, and a mark for each station in it.
    assert!(html.contains("scope-curve"), "the filter should be drawn");
    assert!(html.contains("scope-face"));
    assert_eq!(
        html.matches("scope-blip").count(),
        2,
        "both stations should be marked"
    );
    assert!(html.contains("2 stations in the passband"));

    // And the group being sent is not, anywhere. A scope that showed the
    // keying would let you read the answer off the screen instead of hearing
    // it, which would quietly turn the trainer into a typing test.
    assert!(
        !html.contains("QRXZJ"),
        "the group being sent leaked onto the screen"
    );
    assert!(html.contains("•••"), "an unanswered group stays hidden");
}

#[test]
fn training_shows_the_send_counter_while_repeating() {
    let html = render(|| {
        rsx! {
            TrainingScreen {
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
                settings: cw_core::TrainingSettings::default(),
                heard: Vec::new(),
                send_id: 0,
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
            TrainingScreen {
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
                settings: cw_core::TrainingSettings::default(),
                heard: Vec::new(),
                send_id: 0,
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

/// Components that own state, driven through their own controls.
mod interactions {
    use super::*;
    use crate::testing::{stored_session, test_settings, Ui};
    use crate::ui::band::BandConditionsCard;
    use crate::ui::heatmap::ActivityHeatmap;
    use crate::ui::settings::SettingsView;

    #[component]
    fn HeatmapHarness() -> Element {
        rsx! {
            ActivityHeatmap {
                sessions: vec![
                    stored_session("2026-09-07", &[("KM", "KM")]),
                    stored_session("2026-09-08", &[("KM", "KX")]),
                ],
                today: "2026-09-09".to_string(),
            }
        }
    }

    #[test]
    fn a_day_on_the_calendar_can_be_opened() {
        let mut ui = Ui::new(HeatmapHarness, ());
        assert!(ui.has("Tap a day for details."));
        assert!(ui.has("Practice calendar"));
        // One button per day, sixteen weeks of them.
        assert_eq!(ui.controls("day-").len(), 16 * 7);

        // Each day answers to its own date.
        ui.click("day-2026-09-08");
        assert!(ui.has("2026-09-08 · 1 session · 2 chars"));
        assert!(ui.has("accuracy"));
    }

    #[test]
    fn a_quiet_day_says_so() {
        let mut ui = Ui::new(HeatmapHarness, ());
        ui.click("day-2026-09-09");
        assert!(ui.has("2026-09-09 · no practice"));
    }

    #[test]
    fn the_calendar_can_be_coloured_by_accuracy() {
        let mut ui = Ui::new(HeatmapHarness, ());
        assert!(ui.has("More chars"));
        ui.click("seg-accuracy");
        assert!(ui.has("Better copy"));
        assert!(ui.has("hsl("));
        ui.click("seg-volume");
        assert!(ui.has("More chars"));
    }

    #[test]
    fn a_history_that_predates_the_calendar_draws_an_empty_one() {
        let html = render(|| {
            rsx! {
                ActivityHeatmap {
                    sessions: vec![stored_session("2019-01-01", &[("KM", "KM")])],
                    today: "2026-09-09".to_string(),
                }
            }
        });
        assert!(html.contains("Practice calendar"));
        assert!(!html.contains("2019-01-01"));
    }

    #[test]
    fn a_calendar_with_no_readable_date_draws_nothing() {
        let html = render(|| {
            rsx! {
                ActivityHeatmap { sessions: Vec::new(), today: "nonsense".to_string() }
            }
        });
        assert!(html.is_empty());
    }

    #[component]
    fn BandHarness(previewing: bool) -> Element {
        let settings = use_signal(test_settings);
        rsx! {
            BandConditionsCard {
                settings,
                previewing,
                on_preview: move |_| {},
                on_stop: move |_| {},
            }
        }
    }

    #[test]
    fn the_band_card_opens_its_help_and_its_advanced_controls() {
        let mut ui = Ui::new(BandHarness, BandHarnessProps { previewing: false });
        assert!(ui.has("Live preview"));
        assert!(!ui.has("Model gain"));
        assert!(!ui.has("QSB slowly fades"));

        ui.open_disclosures();
        assert!(ui.has("QSB slowly fades"), "the help should have opened");
        assert!(ui.has("Model gain"), "the tuning should have opened");

        // Every advanced slider is moved, and the readout beside it has to
        // follow: a tuning control that never reaches the settings is a dead
        // one, and it would look exactly like this if it were.
        for (slider, moved_to, reads) in [
            ("slider-model-gain", "3.5", "3.5×"),
            ("slider-excitation", "40", "40/s"),
            ("slider-resonance-q", "120", "120"),
            ("slider-decay", "0.75", "0.750"),
            ("slider-filter-offset", "-250", "-250 Hz"),
            ("slider-wobble-depth", "400", "400 Hz"),
            ("slider-wobble-rate", "2.5", "2.50 Hz"),
        ] {
            ui.type_into(slider, moved_to);
            assert!(
                ui.has(&format!("class=\"slider-value\">{reads}<")),
                "{slider} should read {reads}"
            );
        }

        // And nothing on the card breaks it, whichever panel is open.
        for control in ui.controls("") {
            ui.open_disclosures();
            if ui.shows(&control) {
                ui.click(&control);
            }
            if ui.shows(&control) {
                ui.type_into(&control, "0.5");
            }
            if ui.shows(&control) {
                ui.commit(&control, "true");
            }
        }
        assert!(ui.has("Band conditions"));
    }

    #[test]
    fn a_running_preview_offers_a_stop_button() {
        let mut ui = Ui::new(BandHarness, BandHarnessProps { previewing: true });
        assert!(ui.has("Looping “CQ”"));
        ui.click("btn-band-stop");
        assert!(ui.has("Band conditions"));
    }

    #[component]
    fn SettingsHarness() -> Element {
        let settings = use_signal(test_settings);
        rsx! {
            SettingsView {
                settings,
                previewing: false,
                sample_playing: Some("E".to_string()),
                on_preview_band: move |_| {},
                on_stop_band: move |_| {},
                on_play_sample: move |_: String| {},
            }
        }
    }

    #[test]
    fn a_sample_that_is_playing_offers_a_stop_button() {
        let mut ui = Ui::new(SettingsHarness, ());
        assert!(ui.has("test-chip playing"));
        ui.click("btn-sample-stop");
        assert!(ui.has("Keying envelope"));
    }

    #[component]
    fn TrainingHarness() -> Element {
        let mut focused = use_signal(|| 0usize);
        let mut typed = use_signal(String::new);
        rsx! {
            TrainingScreen {
                current: 0,
                total: 2,
                groups: vec!["KM".to_string(), "UR".to_string()],
                inputs: vec![typed(), String::new()],
                confirmed: vec![false, false],
                focused: focused(),
                playing: true,
                locked: true,
                repeat_total: 2,
                repeat_done: 0,
                settings: cw_core::TrainingSettings::default(),
                heard: Vec::new(),
                send_id: 0,
                on_change: move |(_, value): (usize, String)| typed.set(value),
                on_confirm: move |_: usize| typed.set("confirmed".into()),
                on_focus: move |index: usize| focused.set(index),
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    }

    // Under a runtime because the training screen now carries a live scope,
    // and a scope redraws itself on a timer.
    #[test]
    fn typing_is_refused_while_the_group_is_still_being_sent() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(TrainingHarness, ());
            assert!(ui.has("answer locked"));
            assert!(ui.has("Listening…"));
            // The keypress is swallowed rather than confirming the group.
            ui.press_enter("group-input-0");
            assert!(!ui.has("confirmed"));
            // Focusing a group reports it upwards.
            ui.focus("group-input-0");
            assert!(ui.has("group focused"));
        });
    }

    /// The scope on the training screen really is running: left alone, it
    /// redraws itself.
    #[test]
    fn the_scope_keeps_redrawing_while_the_screen_sits_there() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(TrainingHarness, ());
            let first = ui.html();
            ui.advance(200).await;
            assert_ne!(first, ui.html(), "the scope stopped redrawing");
        });
    }

    fn scope_trace_path(html: &str) -> String {
        let from = html
            .find("class=\"scope-trace\"")
            .expect("the trace should be on screen");
        let d_at = html[from..].find("d=\"").expect("the trace needs a path") + from + 3;
        let end = html[d_at..].find('"').unwrap() + d_at;
        html[d_at..end].to_string()
    }

    #[component]
    fn AnsweringHarness() -> Element {
        let mut typed = use_signal(String::new);
        rsx! {
            TrainingScreen {
                current: 0,
                total: 2,
                groups: vec!["KM".to_string(), "UR".to_string()],
                inputs: vec![typed(), String::new()],
                confirmed: vec![false, false],
                focused: 0,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                settings: cw_core::TrainingSettings::default(),
                heard: Vec::new(),
                send_id: 0,
                on_change: move |(_, value): (usize, String)| typed.set(value),
                on_confirm: move |_: usize| {},
                on_focus: move |_: usize| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
            if !typed().is_empty() {
                p { id: "committed-answer", "committed:{typed()}" }
            }
        }
    }

    /// After playout the loop is off, so waiting does not paint a new trace.
    #[test]
    fn an_idle_training_scope_stays_still() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(AnsweringHarness, ());
            let first = scope_trace_path(&ui.html());
            ui.advance(200).await;
            assert_eq!(
                first,
                scope_trace_path(&ui.html()),
                "the idle training scope kept redrawing"
            );
        });
    }

    /// Typing is the group list's business. The scope's path must not change
    /// when a key lands in the answer box.
    #[test]
    fn typing_does_not_redraw_the_scope() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(AnsweringHarness, ());
            let first = scope_trace_path(&ui.html());
            ui.type_into("group-input-0", "K");
            assert!(ui.has("K"));
            assert_eq!(
                first,
                scope_trace_path(&ui.html()),
                "typing rebuilt the scope"
            );
        });
    }

    #[component]
    fn ManyGroupsHarness(count: usize) -> Element {
        let mut typed = use_signal(String::new);
        let groups: Vec<String> = (0..count).map(|_| "KM".to_string()).collect();
        let mut inputs = vec![String::new(); count];
        inputs[0] = typed();
        rsx! {
            TrainingScreen {
                current: 0,
                total: count,
                groups,
                inputs,
                confirmed: vec![false; count],
                focused: 0,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                settings: cw_core::TrainingSettings::default(),
                heard: Vec::new(),
                send_id: 0,
                on_change: move |(_, value): (usize, String)| typed.set(value),
                on_confirm: move |_: usize| {},
                on_focus: move |_: usize| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    }

    /// A key lands in one box, so it must cost one box.
    ///
    /// The group card used to be written inline in the list's loop, which put
    /// the draft signal inside every iteration: one keystroke rebuilt and
    /// re-diffed every card on screen, and on the desktop build every one of
    /// those mutations crosses to the webview between the key going down and
    /// the letter appearing. With twenty groups that is most of a session's
    /// worth of DOM per character, and it is what made typing feel like wading.
    ///
    /// So the measure is scaling, not a number: typing into a session of
    /// thirty groups must cost about what it costs in a session of three.
    #[test]
    fn typing_costs_the_same_whatever_the_session_length() {
        crate::testing::run(|| async {
            let cost_at = |count: usize| {
                let mut ui = Ui::new(ManyGroupsHarness, ManyGroupsHarnessProps { count });
                let _ = ui.take_work();
                ui.type_into("group-input-0", "K");
                ui.take_work()
            };
            let small = cost_at(3);
            let large = cost_at(30);
            assert!(small > 0, "the keystroke did nothing at all");
            assert!(
                large <= small * 2,
                "a key cost {small} mutations in a three-group session and {large} in a \
                 thirty-group one — the whole list is being rebuilt per character"
            );
        });
    }

    /// One character short of the send stays in the box. The session does not
    /// hear it until the debounce has sat still.
    #[test]
    fn an_incomplete_answer_stays_local_until_debounce() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(AnsweringHarness, ());
            ui.type_into("group-input-0", "K");
            assert!(ui.has("K"));
            assert!(!ui.has("committed:"));
            ui.advance(48).await;
            assert!(ui.has("committed:K"));
        });
    }

    /// A full-length answer is the one case that cannot wait: auto-confirm
    /// starts from this commit.
    #[test]
    fn a_full_length_answer_commits_on_the_key() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(AnsweringHarness, ());
            ui.type_into("group-input-0", "KM");
            assert!(ui.has("committed:KM"));
        });
    }
}
