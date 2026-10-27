//! Smoke tests that render every screen to HTML, and drive the ones that
//! own state.
//!
//! They run the real component bodies — the rsx, the formatting, the SVG
//! geometry — so a panic or a broken template shows up here instead of on a
//! phone. The plain renders rebuild the virtual DOM in place and throw it
//! away, so their effects never run; the `interactions` below go through the
//! [`Ui`](crate::testing::Ui) harness, which settles effects after every
//! event the way a renderer would. Audio is a recorder throughout.

use std::rc::Rc;

use cw_core::{
    AutoLevelProgress, CharSetMode, GroupResult, KeyerMode, MixedAutoLevelAxis, SessionResult,
    SessionTiming, StreakState, StreakStatus, TrainingSettings,
};
use dioxus::prelude::*;

use super::auto_level::AutoLevelCard;
use super::envelope::EnvelopeCard;
use super::heatmap::{StreakCard, StreakCardProps};
use super::home::Home;
use super::listen::ListenView;
use super::paddle::{PaddlePad, use_paddle_keys};
use super::results::ResultsView;
use super::settings::SettingsView;
use super::stats::{StatsView, StatsViewProps};
use super::training::{INPUT_COMMIT_DEBOUNCE_MS, TrainingProgress, TrainingView, focused_box_open};
use crate::audio::fake::Recorder;
use crate::session_runtime::{AnswerOutbox, TypedAnswer};
use crate::state::{AppState, Screen};
use crate::testing::reads_as;

std::thread_local! {
    /// How many times a [`Keyed`] with no sound device has looked for one.
    static DEVICE_LOOKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// What the session behind a `ManyGroupsHarness` has been handed, kept
    /// off the page so that its hearing an answer draws nothing of its own.
    static HEARD: std::cell::RefCell<Vec<(usize, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Every answer a `KeyMomentsHarness` handed over, with its keys.
    static HANDED: std::cell::RefCell<Vec<TypedAnswer>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The app's paddle keyer around a screen drawn on its own, keyed from a
/// root the way the shell keys it, with a player that records — or, with
/// `no_device`, no sound device to be found, however often it is looked for.
#[component]
fn Keyed(
    screen: Screen,
    #[props(default)] keyer_mode: KeyerMode,
    #[props(default)] no_device: bool,
    children: Element,
) -> Element {
    let settings = use_signal(|| {
        let mut settings = crate::testing::test_settings();
        settings.playback.keyer_mode = keyer_mode;
        settings
    });
    let screen = use_signal(|| screen);
    let app = use_hook(|| {
        let factory: crate::player::BackendFactory = if no_device {
            Rc::new(|| {
                DEVICE_LOOKS.with(|looks| looks.set(looks.get() + 1));
                Err("No audio output device found".to_string())
            })
        } else {
            Recorder::new().factory()
        };
        Rc::new(AppState::with(
            factory,
            Rc::new(crate::persist::memory::MemoryStore::default()),
        ))
    });
    let keys = use_paddle_keys(settings, screen, app);
    rsx! {
        div {
            id: crate::ui::widgets::control_id("keyed", "root"),
            onkeydown: move |e| keys.keydown(e),
            onkeyup: move |e| keys.keyup(e),
            {children}
        }
    }
}

/// Same shape the router draws: the progress header is a sibling of the
/// answer list, not a child of it.
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
    on_change: EventHandler<TypedAnswer>,
    on_confirm: EventHandler<()>,
    on_submit: EventHandler<()>,
    on_stop: EventHandler<()>,
    #[props(default)] keyer_mode: KeyerMode,
    countdown: Option<u32>,
) -> Element {
    let answer = use_hook(AnswerOutbox::new);
    let open = focused_box_open(current, focused, &confirmed);
    rsx! {
        Keyed { screen: Screen::Training, keyer_mode,
            div { class: "stack",
                TrainingProgress {
                    focused,
                    total,
                    playing,
                    open,
                    repeat_total,
                    repeat_done,
                    countdown,
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
                    countdown,
                    answer,
                    on_change,
                    on_confirm,
                    on_submit,
                    on_stop,
                }
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
        timestamp: 1_757_000_060_000,
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
            char_wpm: Some(20.0),
            effective_wpm: None,
            sends: 1,
            ..SessionTiming::default()
        }],
        accuracy,
        avg_response_ms: 450.0,
        total_chars: 40,
        score: 820.0,
        level: 3,
        char_set_mode: CharSetMode::Mixed,
        char_wpm: 20.0,
        ..SessionResult::empty_for_test()
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
        html.contains("This build has no audio"),
        super::super::audio::AUDIO_IS_SILENT
    );
}

/// A callsign tier brings in no new character, and a callsign has no group
/// size: Home claims neither.
#[test]
fn home_in_callsign_mode_points_at_no_character_and_counts_calls() {
    let html = render(|| {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.num_groups = 12;
        rsx! {
            Home {
                settings,
                last_accuracy: None,
                session_count: 0,
                pool: String::new(),
                sessions: Vec::new(),
                today: "2026-09-03".to_string(),
                auto_progress: None,
                on_start: move |_| {},
                on_listen: move |_| {},
            }
        }
    });
    assert!(html.contains("pool-chip"), "{html}");
    assert!(!html.contains("pool-chip fresh"), "{html}");
    assert!(!html.contains("Newest character"));
    assert!(html.contains(">12<") && html.contains("Calls"), "{html}");
    assert!(html.contains("Callsign tier 1"));
}

/// Home in Contest mode with `my_call` as your call, and no history.
fn contest_home(my_call: &'static str) -> String {
    #[component]
    fn ContestHome(my_call: &'static str) -> Element {
        let mut settings = crate::testing::contest_settings();
        settings.contest.my_call = my_call.to_string();
        rsx! {
            Home {
                settings,
                last_accuracy: None,
                session_count: 0,
                pool: String::new(),
                sessions: Vec::new(),
                today: "2026-10-21".to_string(),
                auto_progress: None,
                on_start: move |_| {},
                on_listen: move |_| {},
            }
        }
    }
    render_props(ContestHome, ContestHomeProps { my_call })
}

/// Without a call, the hero asks for one where the fields will show, and
/// the first-run card says what to do.
#[test]
fn home_in_contest_mode_asks_for_your_call() {
    let html = contest_home("");
    for shown in [
        ">Contest<",
        "CQ WPX",
        r#"id="field-your-callsign""#,
        "Your callsign",
        "Start contest",
        r#"id="btn-start-training""#,
        "First contest",
        "Add your call, press Start contest; Enter sends what the QSO needs next.",
        ">Minutes<",
        ">Single<",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    for gone in [
        "Current alphabet",
        "field-chip",
        "Start training",
        ">Sent<",
        "Calls",
        "First session",
    ] {
        assert!(!html.contains(gone), "{gone:?} in {html}");
    }
}

/// With a call, the hero shows what a run copies and what you send, as
/// whom, and how long and how busy a run is.
#[test]
fn home_in_contest_mode_says_what_a_run_is() {
    let html = contest_home("DL1ABC");
    for shown in [
        r#"class="pool-chip field-chip">CALL<"#,
        r#"class="pool-chip field-chip">NR<"#,
        "You send 5NN + serial, as DL1ABC",
        ">30<",
        ">WPM<",
        ">10<",
        ">Minutes<",
        ">Single<",
        ">Caller<",
        "Start contest",
        "Press Start contest; Enter sends what the QSO needs next.",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    assert!(!html.contains("field-your-callsign"), "{html}");
    assert!(!html.contains("home-missing"), "nothing refused yet");
}

/// Home in Contest mode for the NA Sprint as K1ABC, called `my_name`, in
/// `my_state`, and no history.
fn sprint_home(my_name: &'static str, my_state: &'static str) -> String {
    #[component]
    fn SprintHome(my_name: &'static str, my_state: &'static str) -> Element {
        let mut settings = crate::testing::contest_settings();
        settings.contest.contest_kind = cw_core::ContestKind::Sprint;
        settings.contest.my_call = "K1ABC".to_string();
        settings.contest.my_name = my_name.to_string();
        settings.contest.my_state = my_state.to_string();
        rsx! {
            Home {
                settings,
                last_accuracy: None,
                session_count: 0,
                pool: String::new(),
                sessions: Vec::new(),
                today: "2026-10-21".to_string(),
                auto_progress: None,
                on_start: move |_| {},
                on_listen: move |_| {},
            }
        }
    }
    render_props(SprintHome, SprintHomeProps { my_name, my_state })
}

/// With the call and the name, a Sprint's hero asks for your state where
/// the fields will show; with it, it shows what a run copies and sends.
#[test]
fn home_asks_for_your_state_before_a_sprint() {
    let html = sprint_home("MIKE", "");
    for shown in [
        "NA Sprint",
        r#"id="field-your-state-or-province""#,
        "Your state or province",
        "e.g. MA, ON or DX",
        "Add your state, press Start contest; Enter sends what the QSO needs next.",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    for gone in ["field-your-callsign", "field-your-name", "field-chip"] {
        assert!(!html.contains(gone), "{gone:?} in {html}");
    }
    // The name comes first.
    let html = sprint_home("", "");
    assert!(html.contains(r#"id="field-your-name""#), "{html}");
    assert!(!html.contains("field-your-state"), "{html}");
    let html = sprint_home("MIKE", "MA");
    for shown in [
        r#"class="pool-chip field-chip">NR<"#,
        r#"class="pool-chip field-chip">NAME<"#,
        r#"class="pool-chip field-chip">ST<"#,
        "You send serial + name + state, as K1ABC",
        "Press Start contest; Enter sends what the QSO needs next.",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    assert!(!html.contains("field-your-state"), "{html}");
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
            on_delete: None,
            backups: None,
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
    assert!(html.contains("data-theme=\"auto\""));
    assert!(html.contains("brand-mark"));
    assert!(html.contains("bottom-dock"));
    assert!(html.contains("bottom-nav"));
    // Home is the landing screen.
    assert!(html.contains("Start training"));
    // Head tags live in their own component so they mount once.
    assert!(html.contains("class=\"screen\""));
    assert!(html.contains("id=\"screen-home\""));
}

#[test]
fn every_icon_draws_something() {
    for name in super::widgets::IconName::ALL {
        let html = render_props(super::widgets::Icon, super::widgets::IconProps { name });
        assert!(html.contains("<svg"), "{name:?} rendered no svg");
        assert!(html.contains("<path"), "{name:?} rendered no path");
    }
}

#[test]
fn settings_renders_digits_and_an_order_of_ones_own() {
    let digits = render(|| {
        let settings = use_signal(|| {
            let mut settings = TrainingSettings::default();
            settings.curriculum.char_set_mode = CharSetMode::Digits;
            settings
        });
        rsx! {
            Keyed { screen: Screen::Settings,
                SettingsView {
                    settings,
                    previewing: true,
                    sample_playing: None,
                    on_preview_band: move |_| {},
                    on_stop_band: move |_| {},
                    on_play_sample: move |_: String| {},
                }
            }
        }
    });
    assert!(digits.contains("Digits only"));
    // No unlock order for digits, and the live preview swaps to a stop control.
    assert!(!digits.contains("Unlock order"));
    assert!(digits.contains("Looping"));

    let own = render(|| {
        let settings = use_signal(|| {
            let mut settings = TrainingSettings::default();
            settings.curriculum.char_set_mode = CharSetMode::Koch;
            settings.curriculum.custom_sequence = vec!['K', 'M', 'R'];
            settings.curriculum.sequence_is_custom = true;
            settings
        });
        rsx! {
            Keyed { screen: Screen::Settings,
                SettingsView {
                    settings,
                    previewing: false,
                    sample_playing: None,
                    on_preview_band: move |_| {},
                    on_stop_band: move |_| {},
                    on_play_sample: move |_: String| {},
                }
            }
        }
    });
    assert!(own.contains("Unlocked in your own order"));
    assert!(own.contains("value=\"KMR\""), "{own}");
    // One list on the card, the unlock order: no character set beside it
    // to hold a second.
    assert!(!own.contains("id=\"seg-custom\""), "{own}");
}

#[test]
fn settings_renders_every_card() {
    let html = render(|| {
        let settings = use_signal(TrainingSettings::default);
        rsx! {
            Keyed { screen: Screen::Settings,
                SettingsView {
                    settings,
                    previewing: false,
                    sample_playing: None,
                    on_preview_band: move |_| {},
                    on_stop_band: move |_| {},
                    on_play_sample: move |_: String| {},
                }
            }
        }
    });
    for needle in [
        "Character set",
        "Session shape",
        "Pause after each group",
        "Sends per group",
        "Iambic A",
        "Ultimatic",
        "Speed",
        // dioxus-ssr escapes the ampersand, so match the card note instead.
        "Volume, pitch and station strength",
        "Keying envelope",
        "Auto level",
        "Character sampling",
        "Band conditions",
        "Privacy policy",
        "https://mashu.github.io/dust/privacy/",
        "Credits",
        "The people who make Dust",
        "disclosure-credits",
    ] {
        assert!(html.contains(needle), "settings should render {needle}");
    }
    assert!(
        html.contains("slider-bar") && html.contains("slider-fill") && html.contains("slider-knob"),
        "sliders must paint a track of their own, not a naked range input"
    );
    assert!(
        html.contains("less-slider-") && html.contains("more-slider-"),
        "sliders need step buttons so values can change without dragging a range thumb"
    );
    assert!(
        html.contains("class=\"stepper\""),
        "number fields should keep their steppers"
    );
    // Every control says what it is to a screen reader: a number field's
    // label is tied to its input, and a step button names what it steps.
    assert!(html.contains("for=\"number-key-speed\""), "{html}");
    assert!(html.contains("aria-label=\"Decrease Key speed\""));
    assert!(html.contains("aria-label=\"Increase Rise time\""));
    assert!(html.contains("for=\"field-sequence-order\""));
    assert!(html.contains("aria-label=\"Rise time\""));
}

/// In Contest mode the Character set card shows the contest in place of an
/// alphabet: a pill for each contest, what you send and copy, a
/// QSO with your call in it, and the tier its callers' calls come from.
/// Your station follows, and no alphabet, group, preset or level control
/// is drawn.
#[test]
fn settings_in_contest_mode_show_the_contest_and_your_station() {
    let html = render(|| {
        let settings = use_signal(crate::testing::contest_settings);
        rsx! {
            Keyed { screen: Screen::Settings,
                SettingsView {
                    settings,
                    previewing: false,
                    sample_playing: None,
                    on_preview_band: move |_| {},
                    on_stop_band: move |_| {},
                    on_play_sample: move |_: String| {},
                }
            }
        }
    });
    for shown in [
        ">Character set<",
        "Contest · CQ WPX",
        r#"<div class="eyebrow">Contest</div><div class="mode-pills">"#,
        r#"id="contest-cq-wpx" class="pill active""#,
        "You send 5NN + serial\u{a0}· You\u{a0}copy\u{a0}NR",
        r#"<li class="qso-line you"><span class="qso-who tiny">You</span><span class="qso-sends"><span class="call">CQ DL1ABC TEST</span>"#,
        r#"<li class="qso-line they"><span class="qso-who tiny">They</span><span class="qso-sends"><span class="call">K5XYZ</span>"#,
        ">K5XYZ 5NN TT1<",
        ">TU 5NN 214<",
        ">TU<",
        "Callsign tier (1–6)",
        ">Your station<",
        "Signing DL1ABC",
        &reads_as(&[
            "Fed. Rep. of Germany",
            "Europe",
            "CQ zone 14",
            "WPX prefix DL1",
        ]),
        "Stored on this device only.",
        "Minutes per run",
        "Band activity",
        crate::ui::privacy::CONTEST_PRIVACY,
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    for gone in [
        "Unlock order",
        "Practice window",
        "What this tier sends",
        "Digits level",
        "select-experience",
        "Groups per session",
        "Sends per group",
        "Adapt speed to me",
        "Adjust the level for me",
    ] {
        assert!(!html.contains(gone), "{gone:?} in {html}");
    }
    // Contest's segment is the one chosen, and no alphabet's.
    for (mode, label) in crate::ui::settings_charset::CHAR_SET_MODES {
        let id = crate::ui::widgets::control_id("seg", label);
        let class = if mode == CharSetMode::Contest {
            "seg active"
        } else {
            "seg"
        };
        assert!(
            html.contains(&format!(r#"id="{id}" class="{class}">"#)),
            "{id} {html}"
        );
    }
    // Your station sits under the card whose contest it serves.
    let charset = html.find(">Character set<").expect("the card");
    let station = html.find(">Your station<").expect("the station");
    let shape = html.find(">Session shape<").expect("session shape");
    assert!(charset < station && station < shape);
}

/// Settings in Contest mode for a Simple QSO as DL1ABC, called `my_name`.
fn simple_qso_settings(my_name: &'static str) -> String {
    #[component]
    fn SimpleQsoSettings(my_name: &'static str) -> Element {
        let settings = use_signal(|| {
            let mut settings = crate::testing::contest_settings();
            settings.contest.contest_kind = cw_core::ContestKind::SimpleQso;
            settings.contest.my_name = my_name.to_string();
            settings
        });
        rsx! {
            Keyed { screen: Screen::Settings,
                SettingsView {
                    settings,
                    previewing: false,
                    sample_playing: None,
                    on_preview_band: move |_| {},
                    on_stop_band: move |_| {},
                    on_play_sample: move |_: String| {},
                }
            }
        }
    }
    render_props(SimpleQsoSettings, SimpleQsoSettingsProps { my_name })
}

/// Simple QSO has its pill, its own dialogue from its row, and on Your
/// station a name field, which a start needs and the card says so.
#[test]
fn a_simple_qso_shows_its_dialogue_and_asks_for_your_name() {
    let html = simple_qso_settings("");
    for shown in [
        "Contest · Simple QSO",
        r#"id="contest-simple-qso" class="pill active""#,
        r#"id="contest-cq-wpx" class="pill""#,
        "You send 599 + name\u{a0}· You\u{a0}copy\u{a0}RST\u{a0}NAME",
        ">CQ CQ DE DL1ABC DL1ABC K<",
        ">DL1ABC DE K5XYZ K5XYZ K<",
        ">K5XYZ DE DL1ABC UR 599 599 NAME &#60;your name&#62; &#60;your name&#62; BK<",
        ">R TNX UR 579 579 NAME BOB BOB BK<",
        ">TU BOB 73 EE<",
        ">73 EE<",
        r#"for="field-your-name""#,
        "Add your name to start a Simple\u{a0}QSO",
        "Signing DL1ABC<",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    let html = simple_qso_settings("MAT");
    assert!(html.contains("Signing DL1ABC · MAT"), "{html}");
    assert!(html.contains(">K5XYZ DE DL1ABC UR 599 599 NAME MAT MAT BK<"));
    assert!(!html.contains("Add your name"));
    // WPX sends no name, so Your station has no field for one.
    let wpx = render(|| {
        let settings = use_signal(crate::testing::contest_settings);
        rsx! {
            Keyed { screen: Screen::Settings,
                SettingsView {
                    settings,
                    previewing: false,
                    sample_playing: None,
                    on_preview_band: move |_| {},
                    on_stop_band: move |_| {},
                    on_play_sample: move |_: String| {},
                }
            }
        }
    });
    assert!(!wpx.contains("field-your-name"));
}

/// Settings in Contest mode for the NA Sprint as `my_call`, called
/// `my_name`, in `my_state`.
fn sprint_settings(my_call: &'static str, my_name: &'static str, my_state: &'static str) -> String {
    station_settings(
        cw_core::ContestKind::Sprint,
        Station {
            call: my_call,
            name: my_name,
            state: my_state,
            ..Station::default()
        },
    )
}

/// The NA Sprint has its pill, first as the card lists the contests, its
/// dialogue from its row, the simplifications it makes, and on Your station
/// a name and a state, each asked for in turn, the state read as the place
/// it names, or as DX for a call outside the US and Canada.
#[test]
fn a_sprint_shows_its_dialogue_and_asks_for_your_name_and_state() {
    let html = sprint_settings("K1ABC", "", "");
    for shown in [
        "Contest · NA Sprint",
        r#"id="contest-na-sprint" class="pill active""#,
        "You send serial + name + state\u{a0}· You\u{a0}copy\u{a0}NR\u{a0}NAME\u{a0}ST",
        "Simplified: you keep your frequency (in the real contest you QSY after each QSO), so you sign last",
        "outside the US and Canada you send DX",
        ">CQ NA K1ABC<",
        ">W7XYZ<",
        ">W7XYZ K1ABC 1 &#60;your name&#62; &#60;your state&#62;<",
        ">K1ABC 214 BOB OR<",
        ">TU K1ABC<",
        r#"for="field-your-name""#,
        r#"for="field-your-state-or-province""#,
        "Add your name to start an NA\u{a0}Sprint",
        "Signing K1ABC<",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    // The pills go as the card lists the contests: the Sprint first.
    let sprint = html.find("contest-na-sprint").expect("the pill");
    let wpx = html.find("contest-cq-wpx").expect("the pill");
    assert!(sprint < wpx);
    let html = sprint_settings("K1ABC", "MIKE", "");
    assert!(
        html.contains("Add your state or province to start an NA\u{a0}Sprint"),
        "{html}"
    );
    let html = sprint_settings("K1ABC", "MIKE", "MA");
    for shown in [
        "Signing K1ABC · MIKE · MA",
        "Reads as Massachusetts",
        ">W7XYZ K1ABC 1 MIKE MA<",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    assert!(!html.contains("Add your"), "{html}");
    // Outside North America your state is DX, and nothing is missing.
    let html = sprint_settings("DL1ABC", "MAT", "");
    for shown in [
        "Sends DX: in Dust every station outside the US and Canada sends DX",
        "Signing DL1ABC · MAT · DX",
        ">W7XYZ DL1ABC 1 MAT DX<",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    // WPX sends no state, so Your station has no field for one.
    let wpx = simple_qso_settings("MAT");
    assert!(!wpx.contains("field-your-state"));
}

/// Your station, as a render test gives it.
#[derive(Clone, Copy, PartialEq)]
struct Station {
    call: &'static str,
    name: &'static str,
    state: &'static str,
    zone: u32,
    power: &'static str,
    class: &'static str,
    section: &'static str,
}

impl Default for Station {
    fn default() -> Self {
        Self {
            call: "",
            name: "",
            state: "",
            zone: 0,
            power: "100",
            class: "1D",
            section: "",
        }
    }
}

/// Your station as `call`, everything else as it is until given.
fn called(call: &'static str) -> Station {
    Station {
        call,
        ..Station::default()
    }
}

/// Settings in Contest mode for `kind` as `station`.
fn station_settings(kind: cw_core::ContestKind, station: Station) -> String {
    #[component]
    fn StationSettings(kind: cw_core::ContestKind, station: Station) -> Element {
        let settings = use_signal(|| {
            let mut settings = crate::testing::contest_settings();
            settings.contest.contest_kind = kind;
            settings.contest.my_call = station.call.to_string();
            settings.contest.my_name = station.name.to_string();
            settings.contest.my_state = station.state.to_string();
            settings.contest.my_zone = station.zone;
            settings.contest.my_power = station.power.to_string();
            settings.contest.my_class = station.class.to_string();
            settings.contest.my_section = station.section.to_string();
            settings
        });
        rsx! {
            Keyed { screen: Screen::Settings,
                SettingsView {
                    settings,
                    previewing: false,
                    sample_playing: None,
                    on_preview_band: move |_| {},
                    on_stop_band: move |_| {},
                    on_play_sample: move |_: String| {},
                }
            }
        }
    }
    render_props(StationSettings, StationSettingsProps { kind, station })
}

/// CQ WW has its pill, its dialogue from its row with your zone cut, what
/// it simplifies, and on Your station your zone, read from your call while
/// you give none, with no name, state or power.
#[test]
fn cqww_shows_its_dialogue_and_your_zone() {
    let html = station_settings(cw_core::ContestKind::Cqww, called("DL1ABC"));
    let reading = reads_as(&["Fed. Rep. of Germany", "Europe", "CQ zone 14"]);
    for shown in [
        "Contest · CQ WW",
        r#"id="contest-cq-ww" class="pill active""#,
        "You send 5NN + zone\u{a0}· You\u{a0}copy\u{a0}ZN",
        "one band, so a station, a zone and a country each count once",
        "no WAE countries",
        ">CQ DL1ABC TEST<",
        ">JA1XYZ<",
        ">JA1XYZ 5NN A4<",
        ">TU 5NN 25<",
        r#"for="field-your-cq-zone-0-40""#,
        "0 takes it from your call: zone 14, sent A4",
        &format!("{reading}<"),
        "Signing DL1ABC · zone 14",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    for gone in [
        "field-your-name",
        "field-your-state",
        "field-your-power",
        "side-line",
    ] {
        assert!(!html.contains(gone), "{gone:?} in {html}");
    }
    // A zone given goes out in place of the call's, which the line names.
    let html = station_settings(
        cw_core::ContestKind::Cqww,
        Station {
            zone: 15,
            ..called("DL1ABC")
        },
    );
    for shown in [
        "Sends A5: zone 15, where your call reads as zone 14",
        ">JA1XYZ 5NN A5<",
        "Signing DL1ABC · zone 15",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    // A state left from another contest gives a US call its zone, and the
    // lines say so, though CQ WW has no field for it.
    let html = station_settings(
        cw_core::ContestKind::Cqww,
        Station {
            state: "CA",
            ..called("K1ABC")
        },
    );
    for shown in [
        "0 takes it from your call and state: zone 3 in California, sent 3",
        &reads_as(&["United States", "North America", "CQ zone 3 in California"]),
        ">JA1XYZ 5NN 3<",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    // A call Dust cannot place needs its zone given before a run, and
    // reads as no country, its WPX prefix left to WPX.
    let html = station_settings(cw_core::ContestKind::Cqww, called("C31AB"));
    for shown in [
        "Add your CQ zone to start a CQ\u{a0}WW",
        "JA1XYZ 5NN &#60;your zone&#62;",
        &format!("{}<", reads_as(&["unknown prefix, so no country"])),
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
}

/// ARRL DX has its pill and says which side you are on: as DX, you send
/// your power, which Your station asks for, and copy a state; as W/VE, you
/// send your state, Labrador among them, and copy power.
#[test]
fn arrl_dx_says_your_side_and_asks_for_what_it_sends() {
    let html = station_settings(cw_core::ContestKind::ArrlDx, called("DL1ABC"));
    let reading = reads_as(&["Fed. Rep. of Germany", "Europe", "CQ zone 14", "you are DX"]);
    for shown in [
        "Contest · ARRL DX",
        r#"id="contest-arrl-dx" class="pill active""#,
        "You send 5NN + power\u{a0}· You\u{a0}copy\u{a0}ST",
        r#"id="side-line" class="muted" style="margin: 0;">You are DX: you work US and Canada only<"#,
        "Simplified: one band, so a station and a multiplier each count once.",
        ">W7XYZ 5NN 1TT<",
        ">TU 5NN OR<",
        r#"for="field-your-power""#,
        "Sends 1TT: 100 W",
        &reading,
        "Signing DL1ABC · 100 W",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    for gone in ["field-your-name", "field-your-state", "field-your-cq-zone"] {
        assert!(!html.contains(gone), "{gone:?} in {html}");
    }
    // The pills go as the card lists the contests.
    let order: Vec<usize> = [
        "contest-na-sprint",
        "contest-arrl-dx",
        "contest-cq-wpx",
        "contest-cq-ww",
        "contest-field-day",
        "contest-simple-qso",
    ]
    .iter()
    .map(|id| html.find(id).expect("every pill"))
    .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    let html = station_settings(cw_core::ContestKind::ArrlDx, called("K1ABC"));
    for shown in [
        "You send 5NN + state\u{a0}· You\u{a0}copy\u{a0}PWR",
        "You are W/VE: you work DX only (Alaska and Hawaii count as DX)",
        ">G4XYZ 5NN &#60;your state&#62;<",
        ">TU 5NN KW<",
        r#"for="field-your-state-or-province""#,
        "Add your state or province to start an ARRL\u{a0}DX",
        "you\u{a0}are\u{a0}W/VE",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    assert!(!html.contains("field-your-power"), "{html}");
    // With no call yet, you are DX until you add one.
    let html = station_settings(cw_core::ContestKind::ArrlDx, called(""));
    assert!(
        html.contains("Until you add your call you are DX: you work US and Canada only"),
        "{html}"
    );
    // Labrador is a place of its own in ARRL DX.
    let html = station_settings(
        cw_core::ContestKind::ArrlDx,
        Station {
            state: "LB",
            ..called("VO2AB")
        },
    );
    for shown in ["Reads as Labrador", "Signing VO2AB · LB", ">G4XYZ 5NN LB<"] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
}

/// Field Day has its pill, the sixth, and its dialogue from its row: you
/// send their call, your class and your section, they send theirs led by
/// R. Your station asks for your class and section, each with a line
/// saying how it goes out; a call that tells its section sends it.
#[test]
fn field_day_shows_its_dialogue_your_class_and_section() {
    let html = station_settings(
        cw_core::ContestKind::FieldDay,
        Station {
            section: "EMA",
            ..called("K1ABC")
        },
    );
    for shown in [
        "Contest · Field Day",
        r#"id="contest-field-day" class="pill active""#,
        "You send class + section\u{a0}· You\u{a0}copy\u{a0}CL\u{a0}SEC",
        "Simplified: one band, so a station counts once; and no power multiplier or bonus points, which score the entry, not the copying.",
        ">CQ FD K1ABC<",
        ">W7XYZ<",
        ">W7XYZ 1D EMA<",
        ">R 3A OR<",
        r#"for="field-your-class""#,
        "Sends 1D: 1 transmitter, a home station on commercial power",
        r#"for="field-your-section""#,
        "Reads as Eastern Massachusetts",
        "Signing K1ABC · 1D · EMA",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    for gone in [
        "field-your-name",
        "field-your-state",
        "field-your-power",
        "field-your-cq-zone",
        "side-line",
    ] {
        assert!(!html.contains(gone), "{gone:?} in {html}");
    }
    // Six contests, a pill each.
    assert_eq!(html.matches(r#"id="contest-"#).count(), 6, "{html}");
    // A W/VE call with no section given is asked for one.
    let html = station_settings(
        cw_core::ContestKind::FieldDay,
        Station {
            class: "3A",
            ..called("K1ABC")
        },
    );
    for shown in [
        "Add your section to start a Field\u{a0}Day",
        ">W7XYZ 3A &#60;your section&#62;<",
        "Sends 3A: 3 transmitters, a club or group in the field",
        "Signing K1ABC · 3A",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    // Hawaii's call tells its section: PAC.
    let html = station_settings(cw_core::ContestKind::FieldDay, called("KH6XYZ"));
    for shown in [
        "Sends PAC from your call: Pacific",
        ">W7XYZ 1D PAC<",
        "Signing KH6XYZ · 1D · PAC",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    // Yukon's section goes out as the Territories'.
    let html = station_settings(
        cw_core::ContestKind::FieldDay,
        Station {
            section: "YT",
            ..called("VY1AB")
        },
    );
    for shown in [
        "Sends TER: ARRL contests count Yukon in the Territories",
        ">W7XYZ 1D TER<",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
}

/// Home in Contest mode for Field Day as `my_call`, in the section
/// `my_section`, and no history.
fn field_day_home(my_call: &'static str, my_section: &'static str) -> String {
    #[component]
    fn FieldDayHome(my_call: &'static str, my_section: &'static str) -> Element {
        let mut settings = crate::testing::contest_settings();
        settings.contest.contest_kind = cw_core::ContestKind::FieldDay;
        settings.contest.my_call = my_call.to_string();
        settings.contest.my_section = my_section.to_string();
        rsx! {
            Home {
                settings,
                last_accuracy: None,
                session_count: 0,
                pool: String::new(),
                sessions: Vec::new(),
                today: "2026-10-22".to_string(),
                auto_progress: None,
                on_start: move |_| {},
                on_listen: move |_| {},
            }
        }
    }
    render_props(
        FieldDayHome,
        FieldDayHomeProps {
            my_call,
            my_section,
        },
    )
}

/// With a W/VE call, Field Day's hero asks for your section where the
/// fields will show, as it asks for a state, and says what a section is;
/// with it, or with a call that tells it, it shows what a run copies and
/// sends.
#[test]
fn home_asks_for_your_section_before_field_day() {
    let html = field_day_home("K1ABC", "");
    for shown in [
        "Field Day",
        r#"id="field-your-section""#,
        "Your section",
        "e.g. EMA, ONS or DX",
        "Your ARRL or RAC section, as EMA for Eastern Massachusetts; DX outside them",
        "Add your section, press Start contest; Enter sends what the QSO needs next.",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    for gone in ["field-your-callsign", "field-your-class", "field-chip"] {
        assert!(!html.contains(gone), "{gone:?} in {html}");
    }
    for (call, section) in [("K1ABC", "EMA"), ("KL7AB", ""), ("DL1ABC", "")] {
        let html = field_day_home(call, section);
        for shown in [
            r#"class="pool-chip field-chip">CL<"#,
            r#"class="pool-chip field-chip">SEC<"#,
            &format!("You send class + section, as {call}"),
            "Press Start contest; Enter sends what the QSO needs next.",
        ] {
            assert!(html.contains(shown), "{call}: no {shown:?} in {html}");
        }
        assert!(!html.contains("field-your-section"), "{call}: {html}");
    }
}

/// A session with nothing timed says why, and what would have it timed:
/// the lock, when it was on and the groups were longer than a character.
#[test]
fn results_with_nothing_timed_say_why() {
    let locked = render(|| {
        rsx! {
            ResultsView {
                result: crate::testing::stored_session("2026-10-22", &[("KM", "KM"), ("RS", "RS")]),
                auto_message: None,
                typing_locked: true,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    assert!(locked.contains("none timed"), "{locked}");
    assert!(
        locked.contains("Lock typing while sending is on"),
        "{locked}"
    );
    let unlocked = render(|| {
        rsx! {
            ResultsView {
                result: crate::testing::stored_session("2026-10-22", &[("KM", "KM")]),
                auto_message: None,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    assert!(
        unlocked.contains("key each character as you hear it"),
        "{unlocked}"
    );
    assert!(!unlocked.contains("Lock typing"), "{unlocked}");
}

/// A Field Day run's results: two points a QSO and nothing to multiply
/// them, so the multipliers read a dash and the score is the points; raw
/// as logged, verified as sent, with what the checks found.
#[test]
fn a_field_day_run_has_results_with_its_points() {
    let html = render(|| {
        rsx! {
            ResultsView {
                result: crate::testing::stored_field_day_run(
                    "2026-10-22",
                    &[
                        [("W7XYZ", "W7XYZ"), ("3A", "3A"), ("OR", "OR")],
                        [("VE3ABC", "VE3ABC"), ("1D", "1D"), ("ONS", "ONS")],
                        [("KH6AB", "KH6AB"), ("2A", "2A"), ("PAC", "PAC")],
                        [("W1ABC", "W1ABC"), ("1E", "1E"), ("EMA", "WMA")],
                    ],
                ),
                auto_message: None,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    for shown in [
        "Run complete",
        "Field Day · 10:00",
        "4 QSOs, 24 an hour. One SEC was wrong.",
        ">3/4<",
        "Contest score",
        ">Mults<",
        r#"<th scope="row">Raw</th><td>8</td><td>—</td><td>8</td>"#,
        r#"<th scope="row">Verified</th><td>6</td><td>—</td><td>6</td>"#,
        "0\u{a0}busted\u{a0}calls\u{a0}· 0\u{a0}wrong\u{a0}CL\u{a0}· 1\u{a0}wrong\u{a0}SEC\u{a0}· 0\u{a0}NIL\u{a0}· 0\u{a0}dupes",
        "WMA→EMA",
        "wrong SEC",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
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
    assert!(html.contains(">Points<"), "Dust's points read Points");
    assert!(!html.contains(">Score<"));
    assert!(!html.contains("place-banner"), "no place, no banner");
}

/// A contest run's results: the share of QSOs logged clean, the run in a
/// sentence, the four tiles, both contest scores with what the checks
/// found, and the log beside what was really sent.
#[test]
fn a_contest_run_has_results_of_its_own() {
    let html = render(|| {
        rsx! {
            ResultsView {
                result: crate::testing::stored_run(
                    "2026-10-20",
                    &[("K5XYZ", "214", "K5XYZ", "214"), ("DL2XYZ", "13", "DL2XYZ", "31")],
                ),
                auto_message: None,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    for shown in [
        "Run complete",
        ">CLEAN<",
        r#"aria-label="50% of QSOs logged clean""#,
        "50%",
        "CQ WPX · 10:00",
        "2 QSOs, 12 an hour. One NR was wrong.",
        ">QSOs<",
        ">1/2<",
        ">Rate<",
        ">12/h<",
        ">Accuracy<",
        ">Points<",
        "Contest score",
        ">QSO pts<",
        ">Mults<",
        ">Score<",
        ">Raw<",
        ">Verified<",
        "0\u{a0}busted\u{a0}calls\u{a0}· 1\u{a0}wrong\u{a0}NR\u{a0}· 0\u{a0}NIL\u{a0}· 0\u{a0}dupes",
        "Log, with what was really sent",
        "31→13",
        "wrong NR",
        "Run again",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    assert!(!html.contains("Session complete"), "a run is not a session");
    assert!(!html.contains("Group by group"));
    assert!(
        !html.contains("btn-history"),
        "fresh from the run: no History"
    );
}

/// An NA Sprint run's results: its contest score, a point a QSO times the
/// places worked, raw as logged and verified as sent, with what the checks
/// found field by field, and its rate.
#[test]
fn a_sprint_run_has_results_with_its_score() {
    let html = render(|| {
        rsx! {
            ResultsView {
                result: crate::testing::stored_sprint_run(
                    "2026-10-21",
                    &[
                        [("W7XYZ", "W7XYZ"), ("145", "145"), ("BOB", "BOB"), ("OR", "OR")],
                        [("VE3ABC", "VE3ABC"), ("23", "23"), ("DAVE", "DAVE"), ("ON", "ON")],
                        [("K3DC", "K3DC"), ("7", "7"), ("JIM", "JIM"), ("DC", "DC")],
                        [("KL7AB", "KL7AB"), ("88", "88"), ("SUE", "SUE"), ("AK", "AK")],
                        [("W5ABC", "W5ABC"), ("61", "61"), ("TOM", "TIM"), ("TX", "NM")],
                    ],
                ),
                auto_message: None,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    for shown in [
        "Run complete",
        "NA Sprint · 10:00",
        "5 QSOs, 30 an hour. One NAME was wrong; one ST was wrong.",
        ">QSOs<",
        ">4/5<",
        ">Rate<",
        ">30/h<",
        "Contest score",
        // Raw: five QSOs, OR ON DC AK NM as logged; verified: four, and
        // their four places.
        r#"<th scope="row">Raw</th><td>5</td><td>5</td><td>25</td>"#,
        r#"<th scope="row">Verified</th><td>4</td><td>4</td><td>16</td>"#,
        "0\u{a0}busted\u{a0}calls\u{a0}· 0\u{a0}wrong\u{a0}NR\u{a0}· 1\u{a0}wrong\u{a0}NAME\u{a0}· 1\u{a0}wrong\u{a0}ST\u{a0}· 0\u{a0}NIL\u{a0}· 0\u{a0}dupes",
        "TIM→TOM",
        "NM→TX",
        "wrong NAME, wrong ST",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
}

/// A Simple QSO run has no contest score: its pace is minutes a QSO, and in
/// place of the score, how well each part of a QSO was copied.
#[test]
fn a_simple_qso_run_has_results_without_a_score() {
    let html = render(|| {
        rsx! {
            ResultsView {
                result: crate::testing::stored_simple_qso_run(
                    "2026-10-20",
                    &[
                        [("K5XYZ", "K5XYZ"), ("579", "579"), ("BOB", "BOB")],
                        [("DL2XYZ", "DL2XYZ"), ("599", "599"), ("HANS", "HANZ")],
                    ],
                ),
                auto_message: None,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    for shown in [
        "Run complete",
        "Simple QSO · 10:00",
        "2 QSOs, 5.0 minutes each. One NAME was wrong.",
        ">Per QSO<",
        ">5.0 min<",
        ">Accuracy<",
        ">Points<",
        "Accuracy per field",
        r#"<th scope="row">CALL</th><td>100%</td>"#,
        r#"<th scope="row">RST</th><td>100%</td>"#,
        r#"<th scope="row">NAME</th><td>86%</td>"#,
        "0\u{a0}busted\u{a0}calls\u{a0}· 0\u{a0}wrong\u{a0}RST\u{a0}· 1\u{a0}wrong\u{a0}NAME\u{a0}· 0\u{a0}NIL\u{a0}· 0\u{a0}dupes",
        "HANZ→HANS",
    ] {
        assert!(html.contains(shown), "no {shown:?} in {html}");
    }
    for gone in ["Contest score", ">Rate<", ">QSO pts<", ">Mults<"] {
        assert!(!html.contains(gone), "{gone:?} in {html}");
    }
}

/// A run long enough for the board says where it stands among runs of its
/// own contest and length, by its verified score.
#[test]
fn a_contest_run_is_placed_among_runs_of_its_contest() {
    let html = render(|| {
        let qsos: Vec<(String, String)> = (1..=5)
            .map(|n| (format!("K{n}XYZ"), format!("{n}{n}")))
            .collect();
        let rows: Vec<(&str, &str, &str, &str)> = qsos
            .iter()
            .map(|(call, nr)| (call.as_str(), nr.as_str(), call.as_str(), nr.as_str()))
            .collect();
        rsx! {
            ResultsView {
                result: crate::testing::stored_run("2026-10-20", &rows),
                place: Some((3, 14)),
                auto_message: None,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    assert!(
        html.contains("3rd best of your 14 CQ WPX runs of 10 minutes — "),
        "{html}"
    );
    assert!(html.contains(" verified points<"), "{html}");
    assert!(html.contains("100%"), "every QSO clean");
    assert!(html.contains("Every one logged clean."));
}

/// A session that tops the board says so with the gold medal; one further
/// down says where it came; one too short to place says what would.
#[test]
fn results_say_where_the_session_stands_on_the_board() {
    fn long() -> SessionResult {
        let mut result = session("2026-09-03", 0.9);
        result.groups = (0..5).flat_map(|_| result.groups.clone()).collect();
        result.score = 600.0;
        result
    }
    let best = render(|| {
        rsx! {
            ResultsView {
                result: long(),
                place: Some((1, 5)),
                auto_message: None,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    assert!(
        best.contains("New personal best — 60.0 points a group, the top of your 5 mixed sessions"),
        "{best}"
    );
    assert!(best.contains("place-banner best"));
    assert!(best.contains("aria-label=\"1st place\""));
    let fourth = render(|| {
        rsx! {
            ResultsView {
                result: long(),
                place: Some((4, 5)),
                auto_message: None,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    assert!(fourth.contains("4th best of your 5 mixed sessions — 60.0 points a group"));
    assert!(!fourth.contains("class=\"medal"), "no medal below third");
    let short = render(|| {
        rsx! {
            ResultsView {
                result: session("2026-09-03", 0.9),
                place: None,
                auto_message: None,
                on_again: move |_| {},
                on_home: move |_| {},
            }
        }
    });
    assert!(short.contains("Sessions of 10 groups or more go on the score board."));
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
    assert!(html.contains("Stream groups"));
}

/// The floating window's buttons are offered only where it can open, and
/// the lock only once it is open.
#[test]
fn listen_offers_the_floating_window_and_its_lock_where_there_is_one() {
    let shown = |can_float: bool, floating: bool, float_locked: bool| {
        render_props(
            |props: (bool, bool, bool)| {
                let (can_float, floating, float_locked) = props;
                rsx! {
                    ListenView {
                        settings: TrainingSettings::default(),
                        playing: false,
                        on_play: move |_: String| {},
                        on_stop: move |_| {},
                        on_back: move |_| {},
                        can_float,
                        floating,
                        float_locked,
                    }
                }
            },
            (can_float, floating, float_locked),
        )
    };
    let none = shown(false, false, false);
    assert!(!none.contains("btn-floating-window"));
    let closed = shown(true, false, false);
    assert!(closed.contains(">Floating window<"), "{closed}");
    assert!(!closed.contains("btn-lock-floating-window"));
    let open = shown(true, true, false);
    assert!(open.contains("Close floating window"));
    assert!(
        open.contains("Lock floating window (click-through)"),
        "{open}"
    );
    // Keeping it on top is done from its window menu, before it is locked.
    assert!(
        open.contains("right-click it and choose Always on Top, then lock it"),
        "{open}"
    );
    let locked = shown(true, true, true);
    assert!(locked.contains("Unlock floating window"), "{locked}");
    assert!(!locked.contains("floating-hint"));
    assert!(locked.contains(r#"aria-pressed="true""#));
}

/// Punctuation used to slug to a bare `chip`, so five chips shared an id.
#[test]
fn every_listen_chip_has_an_id_of_its_own() {
    #[component]
    fn Everything() -> Element {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 40;
        rsx! {
            ListenView {
                settings,
                playing: false,
                on_play: move |_: String| {},
                on_stop: move |_| {},
                on_back: move |_| {},
            }
        }
    }
    let ui = crate::testing::Ui::new(Everything, ());
    let chips = ui
        .page_ids()
        .into_iter()
        .filter(|id| id.starts_with("chip-"))
        .count();
    assert!(chips > 36, "{chips} chips");
}

/// Listen in Contest mode offers Hear a QSO first, and shows what it has
/// played line by line, You and They, callers calling at once on one line
/// and each QSO set apart, with where your station stood in and which QSO
/// is playing, Stop above the lines while it plays; no character is
/// picked meanwhile. A dialogue of a contest other than the one chosen
/// stays out of sight, and so does one stopped before a line was heard.
#[test]
fn listen_in_contest_mode_offers_hear_a_qso_and_shows_its_lines() {
    use crate::playback::{Dialogue, DialogueLine};
    use cw_core::{ContestKind, Sender};
    let shown = |dialogue: Option<Dialogue>, hearing: bool| {
        render_props(
            |props: (Option<Dialogue>, bool)| {
                let (dialogue, hearing) = props;
                let mut settings = TrainingSettings::default();
                settings.curriculum.char_set_mode = CharSetMode::Contest;
                rsx! {
                    ListenView {
                        settings,
                        playing: false,
                        on_play: move |_: String| {},
                        on_stop: move |_| {},
                        on_back: move |_| {},
                        hearing,
                        dialogue,
                    }
                }
            },
            (dialogue, hearing),
        )
    };
    let idle = shown(None, false);
    assert!(idle.contains("Hear a few QSOs of CQ WPX"), "{idle}");
    assert!(
        idle.contains("or a stream of groups."),
        "as the button says"
    );
    let hear = idle.find("id=\"btn-hear-a-qso\"").expect("Hear a QSO");
    let play = idle.find("id=\"btn-play\"").expect("Play");
    assert!(hear < play, "Hear a QSO comes first");
    assert!(idle.contains("listen-glyph"));
    assert!(idle.contains("letter-chip active"));
    let line = |who, sends: &[&str], qso| DialogueLine {
        who,
        sends: sends.iter().map(|send| (*send).to_string()).collect(),
        qso,
    };
    let dialogue = Dialogue {
        kind: ContestKind::Wpx,
        stand_in: Some("DL1ABC".to_string()),
        qsos: 3,
        lines: vec![
            line(Sender::You, &["CQ DL1ABC TEST"], 1),
            line(Sender::Caller(0), &["K5XYZ"], 1),
            line(Sender::You, &["TU"], 1),
            line(Sender::Caller(1), &["G4ABC", "W7XC"], 2),
        ],
    };
    let hearing = shown(Some(dialogue.clone()), true);
    assert!(hearing.contains(">QSO 2 of 3 · CQ WPX<"), "{hearing}");
    assert!(
        hearing
            .contains(">You</span><span class=\"qso-sends\"><span class=\"call\">CQ DL1ABC TEST<")
    );
    assert!(hearing.contains(">They</span><span class=\"qso-sends\"><span class=\"call\">K5XYZ<"));
    assert!(
        hearing.contains("<span class=\"call\">G4ABC</span><span class=\"call\">W7XC</span>"),
        "callers at once side by side"
    );
    assert_eq!(
        hearing.matches("qso-next").count(),
        1,
        "the second QSO apart"
    );
    assert!(hearing.contains("Your station is sent as DL1ABC until you add yours."));
    assert!(hearing.contains("not scored, not saved to History"));
    let stop = hearing.find("id=\"btn-stop\"").expect("Stop");
    let lines = hearing.find("qso-line").expect("lines");
    assert!(stop < lines, "Stop above the lines");
    assert_eq!(hearing.matches("id=\"btn-stop\"").count(), 1);
    assert!(!hearing.contains("btn-hear-a-qso"), "Stop in its place");
    assert!(!hearing.contains("listen-glyph"));
    assert!(!hearing.contains("letter-chip active"), "nothing picked");
    let heard = shown(Some(dialogue.clone()), false);
    assert!(heard.contains("QSOs heard · CQ WPX"));
    assert!(heard.contains("btn-hear-a-qso"));
    assert!(!heard.contains("letter-chip active"));
    let other = shown(
        Some(Dialogue {
            kind: ContestKind::SimpleQso,
            ..dialogue.clone()
        }),
        false,
    );
    assert!(!other.contains("qso-line"), "{other}");
    // Stopped before a line was heard: the stage as it was.
    let none = Dialogue {
        lines: Vec::new(),
        ..dialogue
    };
    let stopped = shown(Some(none.clone()), false);
    assert!(stopped.contains("listen-glyph"), "{stopped}");
    assert!(!stopped.contains("Listening"));
    let waiting = shown(Some(none), true);
    assert!(waiting.contains("Listening — each line shows once it has been sent."));
    // Outside Contest mode there is no QSO to hear.
    let koch = render(|| {
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
    assert!(!koch.contains("btn-hear-a-qso"));
}

#[test]
fn listen_in_callsign_mode_calls_no_character_the_newest() {
    let html = render(|| {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        rsx! {
            ListenView {
                settings,
                playing: false,
                on_play: move |_: String| {},
                on_stop: move |_| {},
                on_back: move |_| {},
            }
        }
    });
    assert!(!html.contains("letter-chip newest"), "{html}");
    assert!(!html.contains("Newest unlocked character"));
    assert!(html.contains("From your current pool"));
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
                on_change: move |_: TypedAnswer| {},
                on_confirm: move |()| {},
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
                on_change: move |_: TypedAnswer| {},
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });
    assert!(html.contains("Your turn"));
    assert!(!html.contains("per group"));
    // One short line to answer by. How to key with the paddles is said on
    // the settings screen, beside them.
    assert!(html.contains(">Type what you heard — no rush.<"), "{html}");
    assert!(!html.contains("dits"), "{html}");
    // The confirmed group shows its comparison.
    assert!(html.contains("ch sent-row"));
}

/// Counting down: the number big where the last group will show, the box
/// and the keypad shut, and the header saying how long.
#[test]
fn a_count_down_shows_where_the_last_group_will() {
    let html = render(|| {
        rsx! {
            TrainingScreen {
                current: 0,
                total: 2,
                groups: vec!["KM".to_string(), "UR".to_string()],
                inputs: vec![String::new(), String::new()],
                confirmed: vec![false, false],
                focused: 0,
                playing: false,
                locked: true,
                repeat_total: 1,
                repeat_done: 0,
                countdown: Some(3),
                on_change: move |_: TypedAnswer| {},
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });
    assert!(html.contains("Starting in 3"), "{html}");
    assert!(
        html.contains(r#"id="countdown" class="countdown-num">3<"#),
        "{html}"
    );
    assert!(html.contains("The first group starts in 3 s"), "{html}");
    assert!(
        !html.contains("Each group you answer is checked here."),
        "{html}"
    );
    let tag = input_open_tag(&html, "group-input-0");
    assert!(tag.contains("answer locked"), "{tag}");
    assert!(tag.contains("Get ready…"), "{tag}");
    assert!(input_open_tag(&html, "key-next").contains("disabled"));
}

fn input_open_tag(html: &str, id: &str) -> String {
    let needle = format!("id=\"{id}\"");
    let start = html
        .find(&needle)
        .unwrap_or_else(|| panic!("no element with {needle}"));
    let open = html[..start]
        .rfind('<')
        .unwrap_or_else(|| panic!("{id} tag has no opening bracket"));
    let close = html[start..]
        .find('>')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("{id} tag is unclosed"));
    html[open..=close].to_string()
}

#[test]
fn an_unlocked_answer_box_does_not_look_disabled() {
    let html = render(|| {
        rsx! {
            TrainingScreen {
                current: 0,
                total: 1,
                groups: vec!["KM".to_string()],
                inputs: vec![String::new()],
                confirmed: vec![false],
                focused: 0,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                on_change: move |_: TypedAnswer| {},
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });
    let tag = input_open_tag(&html, "group-input-0");
    assert!(
        !tag.contains("disabled"),
        "HTML treats disabled=\"false\" as disabled, so the attribute must be absent: {tag}"
    );
    assert!(
        !tag.contains("readonly"),
        "HTML treats readonly=\"false\" as readonly, so the attribute must be absent: {tag}"
    );
    assert!(tag.contains("aria-label=\"Answer for group 1\""), "{tag}");
}

#[test]
fn a_locked_answer_box_is_neither_readonly_nor_disabled() {
    let html = render(|| {
        rsx! {
            TrainingScreen {
                current: 0,
                total: 1,
                groups: vec!["KM".to_string()],
                inputs: vec![String::new()],
                confirmed: vec![false],
                focused: 0,
                playing: true,
                locked: true,
                repeat_total: 1,
                repeat_done: 0,
                on_change: move |_: TypedAnswer| {},
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });
    let tag = input_open_tag(&html, "group-input-0");
    assert!(
        tag.contains("answer locked"),
        "the listening box should look locked: {tag}"
    );
    assert!(
        !tag.contains("readonly"),
        "a read-only box loses the focus, and a phone its keyboard with it: {tag}"
    );
    assert!(
        !tag.contains("disabled"),
        "disabled would also swallow the click that focuses it after the send: {tag}"
    );
}

/// A finished group has no box: it steps up into the slot above, with how
/// it went, and the box below waits for the next group.
#[test]
fn a_confirmed_group_moves_up_and_leaves_no_box() {
    let html = render(|| {
        rsx! {
            TrainingScreen {
                current: 0,
                total: 2,
                groups: vec!["KM".to_string(), "UR".to_string()],
                inputs: vec!["KM".to_string(), String::new()],
                confirmed: vec![true, false],
                focused: 1,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                on_change: move |_: TypedAnswer| {},
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });
    assert!(!html.contains(r#"id="group-input-0""#), "{html}");
    assert!(
        html.contains(r#"id="group-card-0" class="group done""#),
        "{html}"
    );
    let tag = input_open_tag(&html, "group-input-1");
    assert!(
        tag.contains("answer locked"),
        "the next group's box waits for its send: {tag}"
    );
    assert!(html.contains("Waiting…"), "{html}");
}

/// Between two groups, once Enter has answered one, the box below waits for
/// the next and takes no typing: the header says the next group is coming,
/// beside the plain dot, not "Your turn" beside the green one, which wait
/// for the box to open.
#[test]
fn between_groups_the_header_says_the_next_group_is_coming() {
    let html = render(|| {
        rsx! {
            TrainingScreen {
                current: 0,
                total: 2,
                groups: vec!["KM".to_string(), "UR".to_string()],
                inputs: vec!["KM".to_string(), String::new()],
                confirmed: vec![true, false],
                focused: 1,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                on_change: move |_: TypedAnswer| {},
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });
    assert!(
        html.contains(r#"class="progress-status">Next group…<"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="status-dot"></span>"#),
        "{html}"
    );
    // A session's bar eases from group to group; only a run's clock does not.
    assert!(html.contains(r#"class="progress-bar""#), "{html}");
    assert!(!html.contains("Your turn"), "{html}");
    assert!(!html.contains("status-dot ready"), "{html}");
    assert!(!html.contains("status-dot live"), "{html}");
    // In the words the box and the hint below it use.
    assert!(html.contains("Next group on its way."), "{html}");
    assert!(
        input_open_tag(&html, "group-input-1").contains("Waiting…"),
        "{html}"
    );
}

/// The box open on a group that has been sent: the header says it is your
/// turn, beside the green dot.
#[test]
fn an_open_box_is_your_turn() {
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
                on_change: move |_: TypedAnswer| {},
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    });
    assert!(
        html.contains(r#"class="progress-status">Your turn<"#),
        "{html}"
    );
    assert!(html.contains(r#"class="status-dot ready""#), "{html}");
    assert!(!html.contains("Next group"), "{html}");
    let tag = input_open_tag(&html, "group-input-1");
    assert!(!tag.contains("answer locked"), "{tag}");
    assert!(tag.contains("Type the group"), "{tag}");
}

/// Components that own state, driven through their own controls.
mod interactions {
    use super::*;
    use crate::testing::{Ui, stored_session, test_settings};
    use crate::ui::band::BandConditionsCard;
    use crate::ui::heatmap::ActivityHeatmap;
    use crate::ui::settings::SettingsView;
    use crate::ui::widgets::control_id;
    use dioxus::prelude::{Code, Key};

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

    #[component]
    fn StatsHarness(adaptive: bool, mode: CharSetMode) -> Element {
        let mut settings = TrainingSettings::default();
        settings.curriculum.adaptive_group_size = adaptive;
        settings.curriculum.char_set_mode = mode;
        rsx! {
            StatsView { settings, sessions: history() }
        }
    }

    fn stats(adaptive: bool, mode: CharSetMode) -> Ui {
        Ui::new(StatsHarness, StatsHarnessProps { adaptive, mode })
    }

    #[component]
    fn SpeedStatsHarness(adaptive_speed: bool) -> Element {
        let mut settings = TrainingSettings::default();
        settings.playback.adaptive_speed = adaptive_speed;
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        rsx! {
            StatsView { settings, sessions: history() }
        }
    }

    /// The overview shows the copy speed at the share of characters the aim
    /// asks, and whether adaptive speed steers by it.
    #[test]
    fn the_overview_shows_the_copy_speed_and_whether_it_steers() {
        let ui = Ui::new(
            SpeedStatsHarness,
            SpeedStatsHarnessProps {
                adaptive_speed: false,
            },
        );
        assert!(ui.has("Copy speed"));
        assert!(ui.has("WPM · after 1 session") || ui.has("over 3 sessions"));
        assert!(ui.has("94% of characters right"));
        assert!(ui.has("Adaptive speed off"));
        let on = Ui::new(
            SpeedStatsHarness,
            SpeedStatsHarnessProps {
                adaptive_speed: true,
            },
        );
        assert!(on.has("Adapting · Balanced"));
    }

    /// Ten contest runs of five QSOs, a day apart from 2 October 2026,
    /// each copied clean, after `koch` days of Koch.
    fn contest_history(koch: u32) -> Vec<SessionResult> {
        let calls = ["K5XYZ", "DL2ABC", "JA1XYZ", "G4ABC", "VE3XYZ"];
        let mut sessions: Vec<SessionResult> = (1..=koch)
            .map(|day| {
                let mut s = stored_session(&format!("2026-09-{day:02}"), &[("KM", "KM"); 10]);
                s.timestamp = 1_759_000_000_000 + u64::from(day) * 86_400_000;
                s.char_set_mode = CharSetMode::Koch;
                s
            })
            .collect();
        for day in 1..=10u32 {
            let rows: Vec<(&str, &str, &str, &str)> = calls
                .iter()
                .map(|call| (*call, "214", *call, "214"))
                .collect();
            let mut run = crate::testing::stored_run(&format!("2026-10-{day:02}"), &rows);
            run.timestamp = 1_760_000_000_000 + u64::from(day) * 86_400_000;
            sessions.push(run);
        }
        sessions
    }

    /// Stats under the default settings in `mode`, but for speeds from 18 to
    /// 25 WPM, the callers adapting as `adapt` says, with one speed set when
    /// `one_speed`, after a history of Koch sessions and contest runs.
    #[component]
    fn ContestStatsHarness(mode: CharSetMode, adapt: bool, one_speed: bool) -> Element {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = mode;
        settings.contest.contest_adapt_speed = adapt;
        settings.playback.char_wpm_min = 18.0;
        settings.playback.char_wpm_max = 25.0;
        if one_speed {
            settings.playback.char_wpm_min = 30.0;
            settings.playback.char_wpm_max = 30.0;
        }
        let settings = settings.clamp();
        rsx! {
            StatsView { settings, sessions: contest_history(3) }
        }
    }

    /// Stats in `mode` as `ContestStatsHarness` shows them.
    fn contest_stats(mode: CharSetMode, adapt: bool, one_speed: bool) -> Ui {
        Ui::new(
            ContestStatsHarness,
            ContestStatsHarnessProps {
                mode,
                adapt,
                one_speed,
            },
        )
    }

    /// What the copy speed card says of a contest's callers while they
    /// adapt, as they do as the settings ship; with one speed set, the
    /// switch on; and with it off.
    const CALLERS_ADAPT: &str = "Each new caller&#39;s speed is drawn from your copy speed, within your range: faster as you copy calls and numbers clean, slower as you miss them. The run screen&#39;s − and + move only your own speed.";
    const CALLERS_AT_ONE: &str = "One speed is set, so every caller comes at it. The run screen&#39;s − and + move only your own speed.";
    const CALLERS_EVEN: &str = "Callers&#39; speeds are drawn evenly from your range. The run screen&#39;s − and + move only your own speed.";

    /// Copy speed has a Contest pill once there are runs, and in Contest it
    /// says how the callers' speeds come: from this copy speed while their
    /// switch is on, as it ships, steered by the aim; every one at the one
    /// speed set while the switch is on with no room, the chip saying so
    /// rather than that the switch is off; and evenly from the range while
    /// it is off.
    #[test]
    fn copy_speed_has_a_contest_pill_that_says_how_callers_are_drawn() {
        let mut ui = contest_stats(CharSetMode::Koch, true, false);
        assert!(
            ui.shows("speed-koch") && ui.shows("speed-contest"),
            "{}",
            ui.html()
        );
        assert!(!ui.has(CALLERS_ADAPT));
        ui.click("speed-contest");
        assert!(ui.has(CALLERS_ADAPT), "{}", ui.html());
        assert!(ui.has("Not the set practised now"));

        let contest = contest_stats(CharSetMode::Contest, true, false);
        assert!(contest.has(CALLERS_ADAPT));
        assert!(contest.has("Adapting · Balanced"));
        assert!(!contest.has("Adaptive speed off"));

        let one = contest_stats(CharSetMode::Contest, true, true);
        assert!(one.has(CALLERS_AT_ONE), "{}", one.html());
        assert!(one.has(">One speed set<"));
        assert!(!one.has("Adaptive callers off") && !one.has("Adapting · Balanced"));

        let fixed = contest_stats(CharSetMode::Contest, false, false);
        assert!(fixed.has(CALLERS_EVEN));
        assert!(fixed.has(">Adaptive callers off<"));
        assert!(!fixed.has(CALLERS_ADAPT));
    }

    /// The Sampling tab's speed card says how a contest's callers' speeds
    /// come, and its chances follow: leaning on your copy speed while they
    /// adapt, the first callers' of your next run, and saying so when your
    /// copy speed is past the range; at the one speed set; and even across
    /// the range while they do not adapt.
    #[test]
    fn the_sampling_speed_card_says_how_callers_are_drawn() {
        let mut adapting = contest_stats(CharSetMode::Contest, true, false);
        adapting.click("seg-sampling");
        assert!(adapting.has(">Speed<"));
        // The runs were copied clean at 26 WPM, past the 18 to 25 set.
        assert!(adapting.has(
            "The chances are for the first callers of your next run. Within the run they come faster as you copy calls and numbers clean, slower as you miss them. The run screen&#39;s − and + move only your own speed. Your copy speed is above this range, so most come at its top: a wider range on the Speed card lets them go faster."
        ), "{}", adapting.html());
        let mut one = contest_stats(CharSetMode::Contest, true, true);
        one.click("seg-sampling");
        assert!(one.has(CALLERS_AT_ONE));
        let mut even = contest_stats(CharSetMode::Contest, false, false);
        even.click("seg-sampling");
        assert!(even.has(CALLERS_EVEN));
        // At 18 to 25 WPM, a seventh of the range is nearest each whole
        // speed between, and half that at either end.
        let seventh = " 14% chance of the first callers<";
        assert_eq!(even.html().matches(seventh).count(), 6, "{}", even.html());
        assert_ne!(adapting.html().matches(seventh).count(), 6);
    }

    /// On a board of every mode, a run is flagged with its contest and
    /// counts its QSOs; a session says its mode and counts its groups.
    #[test]
    fn a_board_of_every_mode_flags_its_contest_runs() {
        use crate::ui::scores::{ScoresTab, ScoresTabProps};
        let mut ui = Ui::new(
            ScoresTab,
            ScoresTabProps {
                sessions: contest_history(3),
            },
        );
        // The personal best names its contest.
        assert!(ui.has("<span class=\"chip\">CQ WPX</span>"));
        ui.click("board-all");
        let html = ui.html();
        assert!(
            html.contains("class=\"board-badge contest\"><svg"),
            "{html}"
        );
        assert!(html.contains("</svg>CQ WPX</span>5 QSOs · "), "{html}");
        assert!(
            html.contains("class=\"board-badge\">Koch</span>10 groups · "),
            "{html}"
        );
        assert!(!html.contains("· Contest<"), "no mode tacked on at the end");
    }

    /// Under All, the board says where contest runs sit; under Contest, a
    /// chip for each contest ranks runs within it, by the verified score of
    /// runs of one length or by QSOs logged clean an hour.
    #[test]
    fn the_board_ranks_contest_runs_within_their_contest() {
        use crate::ui::scores::{ScoresTab, ScoresTabProps};
        let mut ui = Ui::new(
            ScoresTab,
            ScoresTabProps {
                sessions: contest_history(3),
            },
        );
        assert!(ui.shows("pill-all-modes") && ui.shows("pill-contest"));
        assert!(ui.has("A WPX run earns about 30% less a group"));
        assert!(!ui.shows("kind-cq-wpx"), "no contest chosen yet");
        ui.click("pill-contest");
        assert!(
            ui.has(r#"id="kind-cq-wpx" class="pill active""#),
            "{}",
            ui.html()
        );
        assert!(!ui.has("A WPX run earns about 30% less a group"));
        assert!(ui.has("Ranked by verified contest score, of what was logged clean, among runs set to 10 minutes."));
        assert!(ui.has("The board takes runs of 5 minutes or more that hold 10 groups or more"));
        assert!(ui.has(" verified points</span>"), "{}", ui.html());
        assert!(ui.has(">Verified score over time<"));
        assert!(ui.has(">Best runs<"));
        assert!(ui.input_tag("rank-verified-score").contains("active"));
        assert!(ui.has("5 of 5 QSOs clean · 10-minute run"));
        assert_eq!(ui.controls("board-").len(), 10, "every run, ranked");
        ui.click("rank-clean-qsos-an-hour");
        assert!(ui.input_tag("rank-clean-qsos-an-hour").contains("active"));
        assert!(ui.has("Ranked by QSOs logged clean an hour, fair between runs of any length."));
        assert!(ui.has(" clean QSOs an hour</span>"));
        assert!(ui.has(">Clean QSOs an hour over time<"));
        assert_eq!(ui.controls("board-").len(), 10, "every run, ranked");
        ui.click("pill-koch");
        assert!(!ui.shows("kind-cq-wpx"));
    }

    #[test]
    fn the_overview_shows_the_copy_span_and_whether_it_steers() {
        let ui = stats(false, CharSetMode::Mixed);
        assert!(ui.has("Copy span"));
        assert!(ui.has("chars · up") || ui.has("chars · down") || ui.has("chars · steady"));
        assert!(ui.has("Adaptive length off"));
        assert!(stats(true, CharSetMode::Mixed).has("Adapting · Balanced"));
        // A callsign is as long as it is, but choosing callsigns keeps the
        // span of the set practised before.
        let callsigns = stats(true, CharSetMode::Callsign);
        assert!(callsigns.has("Copy span"));
        assert!(callsigns.has("Not the set practised now"));
    }

    #[test]
    fn the_sampling_tab_shows_each_group_length() {
        let mut ui = stats(false, CharSetMode::Mixed);
        ui.click("seg-sampling");
        assert!(ui.has("Group length"));
        assert!(ui.has("every length is as likely as the next"));
        // The default range is 2 to 3 characters: half each.
        let html = ui.html();
        let card = &html[html.find("Group length").expect("the card")..];
        let card = &card[..card.find(">Speed<").unwrap_or(card.len())];
        assert_eq!(card.matches(" 50% chance next<").count(), 2, "{card}");
        // Each length is a bar of how often it is copied whole, against
        // the aim, Balanced's 70%; the summary says how far it reaches.
        assert!(card.contains("class=\"bar-mark\" style=\"left: 70.0%;\""));
        assert!(card.contains(">Your aim · 70% whole<"), "{card}");
        assert!(card.contains(">Across your range<"));
        assert!(ui.shows("next-length-2") && ui.shows("next-length-3"));

        let mut ui = stats(true, CharSetMode::Mixed);
        ui.click("seg-sampling");
        assert!(ui.has("Adaptive group size leans each group&#39;s length towards where you copy 70% of groups whole."), "{}", ui.html());
        assert!(ui.has("% whole · "));

        let mut ui = stats(true, CharSetMode::Callsign);
        ui.click("seg-sampling");
        assert!(!ui.has("Group length"));
    }

    /// Speed has its card beside length: how likely each speed is next, and
    /// the share of characters copied right at it.
    #[test]
    fn the_sampling_tab_shows_each_speed() {
        let mut ui = stats(false, CharSetMode::Mixed);
        ui.click("seg-sampling");
        assert!(ui.has(">Speed<"));
        assert!(ui.has("Adaptive speed is off, so each group"));
        // The default speeds are 15 to 30 WPM: a row every two, a bar of
        // the characters copied right at it against Balanced's 94%.
        assert_eq!(ui.html().matches("% right · ").count(), 8);
        assert!(ui.shows("next-speed-16") && ui.shows("next-speed-30"));
        assert!(ui.has("class=\"bar-mark\" style=\"left: 94.0%;\""));
        assert!(ui.has(">Your aim · 94% right<"));

        let mut ui = Ui::new(
            SpeedStatsHarness,
            SpeedStatsHarnessProps {
                adaptive_speed: true,
            },
        );
        ui.click("seg-sampling");
        assert!(ui.has("Adaptive speed leans each group&#39;s speed towards where you copy 94% of characters right."), "{}", ui.html());
        assert!(!ui.has("Farnsworth"), "tied spacing needs no word on it");

        // Callsigns have no length to choose, but they still have a speed.
        let mut ui = stats(true, CharSetMode::Callsign);
        ui.click("seg-sampling");
        assert!(ui.has(">Speed<"));
    }

    /// Mixed history from `history`, and a Koch session that missed Q, seen
    /// with `mode` chosen and one letter unlocked.
    #[component]
    fn TwoSetsHarness(mode: CharSetMode) -> Element {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = mode;
        settings.curriculum.level = 1;
        let mut sessions = history();
        let mut koch = stored_session("2026-09-04", &[("QQ", "QX"), ("KM", "KM")]);
        koch.char_set_mode = CharSetMode::Koch;
        koch.timestamp = 1_757_000_900_000;
        sessions.push(koch);
        rsx! {
            StatsView { settings, sessions }
        }
    }

    fn two_sets(mode: CharSetMode) -> Ui {
        Ui::new(TwoSetsHarness, TwoSetsHarnessProps { mode })
    }

    /// A character the set chosen now does not draw is still listed, greyed,
    /// with what its history says of it.
    #[test]
    fn characters_outside_the_set_stay_listed() {
        let mut ui = two_sets(CharSetMode::Koch);
        ui.click("seg-sampling");
        assert!(ui.has("Not in the current set"));
        // Q was missed in the Koch session, and is not drawn now.
        let q = ui.input_tag("sample-q");
        assert!(q.contains("not-drawn"), "{q}");
        assert!(q.contains("aria-label=\"Q: wrong"), "{q}");
        for character in ["u", "r"] {
            assert!(
                ui.shows(&format!("sample-{character}")),
                "{character} is listed"
            );
        }
    }

    /// K half right in its first ten tries and clean in twenty since; M
    /// tried twice.
    #[component]
    fn LettersHarness() -> Element {
        let half = [("K", "K"), ("K", "X")].repeat(5);
        let clean = vec![("K", "K"); 10];
        let sessions = [
            (1, stored_session("2026-09-01", &half)),
            (2, stored_session("2026-09-02", &clean)),
            (3, stored_session("2026-09-03", &clean)),
            (4, stored_session("2026-09-04", &[("MM", "MM")])),
        ]
        .into_iter()
        .map(|(timestamp, mut session)| {
            session.timestamp = timestamp;
            session
        })
        .collect();
        rsx! {
            crate::ui::letters::LettersTab { settings: crate::testing::test_settings(), sessions }
        }
    }

    /// The Letters card says where each character stands, which way it is
    /// going, and that two tries are too few to call M anything.
    #[test]
    fn letter_accuracy_says_where_each_stands_and_which_way_it_goes() {
        let ui = Ui::new(LettersHarness, ());
        assert!(ui.has(">Letter accuracy<"));
        assert!(ui.has(">Building · 1<"), "{}", ui.html());
        assert!(ui.has(">Few tries · 1<"));
        assert!(ui.has(">Mastered · 0<"));
        assert!(!ui.has(">Weak · "), "nothing is weak");
        let k = ui.input_tag("letter-k");
        assert!(k.contains("letter-row stat-row building"), "{k}");
        assert!(k.contains("aria-label=\"K: 83% right of 30, likely"), "{k}");
        assert!(k.contains("up 50 points lately"), "{k}");
        assert!(ui.has(">↑ 50<"));
        assert!(ui.has("Building · 30 tries · likely "), "{}", ui.html());
        // The bar is as long as the share right, with a notch at the
        // Mastered line.
        assert!(ui.has("width: 83.3%;"));
        assert!(ui.has("class=\"bar-mark\" style=\"left: 90.0%;\""));
        let m = ui.input_tag("letter-m");
        assert!(m.contains("letter-row stat-row finding-out"), "{m}");
        assert!(m.contains("few tries"), "{m}");
        // The weakest first: K, building, before M, too few tries to tell.
        let html = ui.html();
        assert!(html.find("id=\"letter-k\"") < html.find("id=\"letter-m\""));
    }

    /// K timed eight times at 320 ms, M at 520, R at 700 and S at 900, U
    /// three times, and E never: a band each, a provisional time, and none.
    /// Its characters a second apart, each keyed while the next one sounds,
    /// as keeping up is. E is also sent alone, answered after its send.
    fn timed_history() -> Vec<SessionResult> {
        let mut session = stored_session("2026-09-01", &[("KMRSUE", "KMRSUE"); 8]);
        for (n, timing) in session.group_timings.iter_mut().enumerate() {
            let u = (n < 3).then_some(410);
            let ends: Vec<u32> = (1..=6).map(|at| at * 1_000).collect();
            let took = [Some(320), Some(520), Some(700), Some(900), u, None];
            let keys = ends
                .iter()
                .zip(took)
                .map(|(end, took)| took.map(|took| end + took))
                .collect();
            timing.char_times = Some(cw_core::CharTimes {
                ends,
                keys,
                corrected: Vec::new(),
                latency_ms: 0,
                box_open: true,
            });
        }
        vec![session, stored_session("2026-09-02", &[("E", "E"); 8])]
    }

    #[component]
    fn TimedLettersHarness() -> Element {
        rsx! {
            crate::ui::letters::LettersTab { settings: crate::testing::test_settings(), sessions: timed_history() }
        }
    }

    /// Each character's recognition time is in the colour of its band, the
    /// ICR ones named in the summary; one timed too few times says so, and
    /// one never timed says that.
    #[test]
    fn recognition_times_are_shown_in_their_bands() {
        let ui = Ui::new(TimedLettersHarness, ());
        let line = |id: &str| {
            let html = ui.html();
            let at = html.find(&format!("id=\"{id}\"")).expect("the row");
            let row = &html[at..];
            row[..row.find("</div><div").unwrap_or(row.len())].to_string()
        };
        assert!(
            line("letter-k").contains(r#"class="rec-chip rec-fast">ICR 320 ms<"#),
            "{}",
            line("letter-k")
        );
        assert!(line("letter-m").contains(r#"class="rec-chip rec-icr">ICR 520 ms<"#));
        assert!(line("letter-r").contains(r#"class="rec-chip rec-slow">700 ms<"#));
        assert!(line("letter-s").contains(r#"class="rec-chip rec-too-slow">900 ms<"#));
        // What the row shows, not its label: " · " starts only the
        // visible text, where the label has ", ".
        assert!(line("letter-u").contains(" · about 410 ms so far, 3 of 8 tries timed"));
        assert!(line("letter-e").contains(" · not timed yet"));
        let e = cw_core::character_diagnostics(&timed_history())
            .into_iter()
            .find(|letter| letter.letter == 'E')
            .expect("E");
        assert_eq!((e.recognition_tries, e.recognition_ms), (0, None));
        assert!(ui.has(">Instant (ICR) · 2<"), "{}", ui.html());
        assert!(ui.has(r#"class="err-chip rec-fast group-text">K<"#));
        assert!(ui.has(r#"class="err-chip rec-icr group-text">M<"#));
        assert!(ui.has("ICR, under 600"));
    }

    /// KM answered after each send, nothing keyed along, as it ships.
    #[component]
    fn UntimedLettersHarness() -> Element {
        let session = stored_session("2026-09-01", &[("KM", "KM"); 8]);
        rsx! {
            crate::ui::letters::LettersTab { settings: crate::testing::test_settings(), sessions: vec![session] }
        }
    }

    /// Nothing timed yet: the card says once how a character is timed, and
    /// no row says more. Nothing stands in for a time.
    #[test]
    fn untimed_letters_say_how_to_be_timed_and_show_no_time() {
        let ui = Ui::new(UntimedLettersHarness, ());
        assert!(ui.has("as you key along"), "{}", ui.html());
        assert!(!ui.has("not timed yet"));
        assert!(!ui.has(" ms"), "a time with nothing timed: {}", ui.html());
        assert!(
            !ui.has("Recognised in"),
            "no bands' legend with nothing in them"
        );
        let k = ui.input_tag("letter-k");
        assert!(!k.contains("recognised"), "{k}");
    }

    /// Koch at level 3, K and M heard, M often missed.
    #[component]
    fn SamplingHarness() -> Element {
        let mut settings = test_settings();
        settings.curriculum.level = 3;
        let sessions = vec![stored_session(
            "2026-09-08",
            &[("KM", "KX"), ("KM", "KX"), ("KM", "KM"), ("KK", "KK")],
        )];
        rsx! {
            crate::ui::sampling::NextUpCard { settings, sessions }
        }
    }

    /// Every character the next group can draw is a row: one never heard is
    /// new, one missed often is to work on, and it opens with the most work
    /// first; Most likely puts the likeliest first, Taught order as taught.
    #[test]
    fn what_comes_up_next_has_a_row_a_character() {
        let first_row = |ui: &Ui| {
            let html = ui.html();
            let at = html.find("id=\"sample-").expect("a row");
            html[at + 4..].split('"').next().unwrap_or("").to_string()
        };
        let mut ui = Ui::new(SamplingHarness, ());
        assert!(ui.has(">What comes up next<"));
        // M missed two times in four: over the 10% threshold, and so few
        // tries that the trainer is still finding out.
        let m = ui.input_tag("sample-m");
        assert!(m.contains("letter-row stat-row finding-out"), "{m}");
        assert!(m.contains("aria-label=\"M: wrong about"), "{m}");
        // U, the third Koch character, was never heard.
        // and has no row: the summary names it, with the rest the set draws
        // and nothing has been heard of.
        assert!(!ui.shows("sample-u"));
        assert!(ui.has(">Not heard yet · "), "{}", ui.html());
        assert!(ui.has("class=\"err-chip finding-out group-text\">U<"));
        assert!(ui.has("Still finding out · "));
        // Each row is a bar, the line under it saying where its rate
        // likely is and its chance next.
        assert!(ui.has("class=\"bar-track marked\""));
        assert!(ui.has("% wrong · "), "{}", ui.html());
        assert!(ui.has(" chance next<"));
        assert!(ui.input_tag("seg-sample-work").contains("active"));
        assert_eq!(first_row(&ui), "sample-m", "the most work first");
        // The notch is at the 10% threshold, on bars that run to 60%.
        assert!(ui.has("class=\"bar-mark\" style=\"left: 16.7%;\""));

        ui.click("seg-sample-taught");
        assert_eq!(first_row(&ui), "sample-k", "taught order");
        let likeliest = cw_core::sampling_rows(
            &{
                let mut settings = test_settings();
                settings.curriculum.level = 3;
                settings
            },
            &[stored_session(
                "2026-09-08",
                &[("KM", "KX"), ("KM", "KX"), ("KM", "KM"), ("KK", "KK")],
            )],
        )
        .into_iter()
        .filter(|row| row.drawn_now)
        .max_by(|a, b| a.sampling_prob.total_cmp(&b.sampling_prob))
        .expect("a row")
        .character;
        ui.click("seg-sample-likely");
        assert!(ui.input_tag("seg-sample-likely").contains("active"));
        assert_eq!(
            first_row(&ui),
            format!("sample-{}", likeliest.to_ascii_lowercase())
        );
    }

    /// What comes up next says, under its ⓘ, what its colours mean against
    /// the threshold the settings set: 10% wrong, from 90% to level up.
    #[test]
    fn what_comes_up_next_explains_its_colours() {
        let mut ui = Ui::new(SamplingHarness, ());
        assert!(!ui.has("help-note"), "out of the way until asked for");
        ui.click("help-what-comes-up-next");
        assert!(ui.has("class=\"help-note\""));
        assert!(ui.has(">Under your 10%<"), "{}", ui.html());
        assert!(ui.has("a session levels up at 90% right (Settings → Auto level)"));
        assert!(
            ui.input_tag("help-what-comes-up-next")
                .contains("aria-expanded=true")
        );
    }

    /// A card's help wears the ⓘ, and the ? is the guided setup's alone:
    /// the two never look alike.
    #[test]
    fn help_and_the_guided_setup_wear_icons_of_their_own() {
        use crate::ui::widgets::IconName;
        let info = IconName::Info.paths();
        let question = IconName::Help.paths();
        assert_ne!(info, question);
        let ui = Ui::new(SamplingHarness, ());
        let html = ui.html();
        let at = html
            .find("id=\"help-what-comes-up-next\"")
            .expect("the chip");
        let chip = &html[at..at + html[at..].find("</button>").expect("its end")];
        assert!(chip.contains(info[1]), "{chip}");
        assert!(!chip.contains(question[1]), "{chip}");
    }

    /// Choosing another set never hides the span and speed of the ones
    /// practised before: with Callsigns chosen, which has none, the cards show
    /// the sets that have, one at a time.
    #[test]
    fn no_set_chosen_hides_the_history_of_another() {
        let mut ui = two_sets(CharSetMode::Callsign);
        assert!(ui.has("Copy span"));
        assert!(ui.has("Copy speed"));
        for pill in ["span-koch", "span-mixed", "speed-koch", "speed-mixed"] {
            assert!(ui.shows(pill), "{pill}");
        }
        assert!(!ui.shows("span-callsigns"), "no callsign history to show");
        assert!(ui.input_tag("span-koch").contains("active"));
        assert!(ui.has("Not the set practised now"));
        ui.click("span-mixed");
        assert!(ui.input_tag("span-mixed").contains("active"));
        assert!(!ui.input_tag("span-koch").contains("active"));

        // The set practised now is the one shown first.
        let ui = two_sets(CharSetMode::Mixed);
        assert!(ui.input_tag("span-mixed").contains("active"));
        assert!(ui.has("Adaptive length off"));
    }

    #[test]
    fn a_day_on_the_calendar_can_be_opened() {
        let mut ui = Ui::new(HeatmapHarness, ());
        assert!(ui.has("Tap a day for details."));
        assert!(ui.has("Practice calendar"));
        // One button per day, June to September of them.
        assert_eq!(ui.controls("day-").len(), 30 + 31 + 31 + 30);

        // A tap pins the day's card; a second tap on it takes it down.
        assert!(!ui.has("day-tip"));
        ui.click("day-2026-09-08");
        assert!(ui.has("Tuesday 8 September 2026"));
        for row in [
            "Characters",
            "Sessions",
            "Groups",
            "Speed",
            "Accuracy",
            "Points",
        ] {
            assert!(ui.has(&format!(">{row}<")), "no {row} on the card");
        }
        assert!(!ui.has(">Score<"), "Dust's points read Points");
        assert!(ui.input_tag("day-2026-09-08").contains("selected"));
        ui.click("day-2026-09-08");
        assert!(!ui.has("day-tip"));
    }

    /// A pinned card also comes down on a click anywhere off the squares, or
    /// Escape; a tap on another square moves it there.
    #[test]
    fn a_pinned_card_comes_down_on_a_click_elsewhere() {
        let mut ui = Ui::new(HeatmapHarness, ());
        ui.click("day-2026-09-08");
        assert!(ui.has(">Tuesday 8 September 2026<"));
        ui.click("day-2026-09-07");
        assert!(ui.has(">Monday 7 September 2026<"));
        assert!(!ui.has(">Tuesday 8 September 2026<"));
        ui.click_elsewhere();
        assert!(!ui.has("day-tip"));
        assert!(!ui.input_tag("day-2026-09-07").contains("selected"));
        // With nothing pinned there is nothing to take down, and the next
        // tap pins as ever.
        ui.click_elsewhere();
        ui.click("day-2026-09-08");
        assert!(ui.has(">Tuesday 8 September 2026<"));
    }

    /// A mouse opens a day's card by pointing at the square and closes it by
    /// M after K copied wrong four times in five, taken for N; M after U
    /// right six times out of six.
    #[component]
    fn MistakesHarness() -> Element {
        let mut groups = vec![("KM", "KN"); 4];
        groups.push(("KM", "KM"));
        groups.extend([("UM", "UM"); 6]);
        rsx! {
            crate::ui::stats_detail::MistakesTab { sessions: vec![stored_session("2026-09-08", &groups)] }
        }
    }

    /// The pair that goes wrong more often than its letter overall is listed
    /// as one of the hardest. Every pair has a square, and nothing ends on
    /// the start of a group; one not heard yet is an empty one that does
    /// nothing.
    #[test]
    fn the_mistakes_tab_lists_the_hardest_pairs_and_grids_them_all() {
        let ui = Ui::new(MistakesHarness, ());
        assert!(ui.has("Hardest pairs"));
        assert!(ui.has("class=\"pair-name\">M after K<"), "{}", ui.html());
        assert!(ui.has(">4 of 5<"));
        assert!(ui.has(">80% here · M 36% overall<"));
        // The bar marks where M stands overall.
        assert!(ui.has("class=\"bar-mark\" style=\"left: 36.4%;\""));
        // ▸, K, M and U before; K, M and U after.
        assert_eq!(ui.controls("pair-").len(), 4 * 3);
        assert!(!ui.shows("pair-k-start"));
        let unheard = ui.input_tag("pair-k-k");
        assert!(
            unheard.contains("disabled") && unheard.contains("empty"),
            "{unheard}"
        );
        assert!(unheard.contains("aria-label=\"K after K: not heard yet\""));
        assert!(
            ui.input_tag("pair-k-m")
                .contains("aria-label=\"M after K: 4 of 5 wrong\"")
        );
        assert!(ui.has("Tap a pair for details."));
        assert!(!ui.has("pair-card"));
    }

    /// Each confusion shows the two characters in Morse, the elements that
    /// differ picked out, how often it happened and of every one sent.
    #[test]
    fn a_confusion_shows_both_characters_in_morse() {
        let ui = Ui::new(MistakesHarness, ());
        assert!(ui.has(">Common confusions<"));
        let html = ui.html();
        let at = html
            .find("aria-label=\"M taken for N: 4 times, 36% of every M sent\"")
            .expect("M taken for N");
        let row = &html[at..];
        let row = &row[..row.find("of every M<").expect("its share")];
        // M is dah dah, N dah dit: the second element is where they part.
        let marks: Vec<&str> = row
            .split("<i class=\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap_or(""))
            .collect();
        assert_eq!(marks, ["dah", "dah differs", "dah", "dit differs"]);
        assert!(row.contains(">4×<"));
        assert!(row.contains("width: 36.4%;"));
        assert!(!ui.has("Once each"), "one confusion, drawn in full");
    }

    /// Most errors first, as the grid opens, puts the row and the column with
    /// the most errors in the top left corner, and taught order puts them
    /// back.
    #[test]
    fn the_grid_can_put_the_most_errors_first() {
        let first_square = |ui: &Ui| {
            let html = ui.html();
            let at = html.find("id=\"pair-").expect("a square");
            html[at + 4..].split('"').next().unwrap_or("").to_string()
        };
        // The most errors first, as it opens: K before has every error, and
        // M after.
        let mut ui = Ui::new(MistakesHarness, ());
        assert!(ui.input_tag("seg-most-errors").contains("active"));
        assert_eq!(first_square(&ui), "pair-k-m");
        // A pinned pair stays pinned however the grid is ordered.
        ui.click("pair-k-m");
        ui.click("seg-taught-order");
        assert!(ui.input_tag("seg-taught-order").contains("active"));
        assert_eq!(first_square(&ui), "pair-start-k");
        assert!(ui.input_tag("pair-k-m").contains("selected"));
        ui.click("seg-most-errors");
        assert_eq!(first_square(&ui), "pair-k-m");
    }

    /// Three sessions of twenty groups, the first ten copied clean, then
    /// two groups in the third five each with a character wrong, and three
    /// in the last five.
    #[component]
    fn EnduranceHarness(long: bool) -> Element {
        let mut groups = vec![("KM", "KM"); 10];
        groups.extend([("KM", "KE"); 2]);
        groups.extend([("KM", "KM"); 3]);
        groups.extend([("KM", "KE"); 3]);
        groups.extend([("KM", "KM"); 2]);
        if !long {
            groups.truncate(8);
        }
        let sessions = (0..3)
            .map(|_| stored_session("2026-09-08", &groups))
            .collect();
        rsx! {
            crate::ui::endurance::EnduranceCard { settings: test_settings(), sessions }
        }
    }

    /// The Endurance card says how far into a session copy holds, and
    /// before there are sessions long enough, what it takes.
    #[test]
    fn the_endurance_card_says_how_far_copy_holds() {
        let ui = Ui::new(EnduranceHarness, EnduranceHarnessProps { long: true });
        assert!(ui.has(">Endurance<"));
        assert!(
            ui.has("Accuracy as a session goes on · Koch · last 3 sessions"),
            "{}",
            ui.html()
        );
        assert!(ui.has("class=\"endu-headline fades\">Fades after group 10<"));
        assert!(ui.has(
            "You start a session copying 100%; by group 20 you are down to 70%, 30 points lower."
        ));
        // A bar a stretch, with its accuracy over it and its groups under it;
        // the first how you start, the last two copper.
        for text in [">100%<", ">80%<", ">70%<", ">1–5<", ">16–20<"] {
            assert!(ui.has(text), "no {text}");
        }
        assert_eq!(ui.html().matches("class=\"endu-bar fades\"").count(), 2);
        assert_eq!(ui.html().matches("class=\"endu-bar first\"").count(), 1);

        let short = Ui::new(EnduranceHarness, EnduranceHarnessProps { long: false });
        assert!(short.has(">Endurance<"));
        assert!(
            short
                .has("After 3 sessions of 10 groups or more, or contest runs of 4 minutes or more")
        );
        assert!(!short.has("endu-chart"));
    }

    /// A tap pins a pair's card and lights up its row and column; a second
    /// tap, or a click anywhere else, takes it down, and a tap on another
    /// pair moves it there.
    #[test]
    fn a_tap_on_a_pair_pins_its_card_and_lights_its_row_and_column() {
        let mut ui = Ui::new(MistakesHarness, ());
        ui.click("pair-k-m");
        for text in [
            "class=\"tip-date\">M after K<",
            ">Wrong<",
            ">80%<",
            ">4 of 5<",
            ">M overall<",
            ">36%<",
            ">4 of 11<",
            ">Taken for<",
            ">N 4×<",
            ">Wrong 2.2× as often as M overall: the pair is the trouble.<",
        ] {
            assert!(ui.has(text), "no {text} on the card: {}", ui.html());
        }
        assert!(ui.has("class=\"pair-dock\""));
        let square = ui.input_tag("pair-k-m");
        assert!(
            square.contains("selected") && square.contains("shown"),
            "{square}"
        );
        // Its row, K before, and its column, M after, light up; nothing else.
        for (id, lit) in [
            ("pair-k-k", true),
            ("pair-k-u", true),
            ("pair-start-m", true),
            ("pair-u-m", true),
            ("pair-u-k", false),
            ("pair-start-u", false),
        ] {
            assert_eq!(ui.input_tag(id).contains("in-line"), lit, "{id}");
        }
        assert!(ui.has("class=\"bigram-head row-head group-text in-line\">K<"));
        assert!(ui.has("class=\"bigram-head group-text in-line\">M<"));
        assert_eq!(
            ui.html().matches("in-line\">").count(),
            2,
            "one row, one column"
        );

        ui.click("pair-k-m");
        assert!(!ui.has("pair-card"));
        assert!(!ui.input_tag("pair-k-m").contains("selected"));
        assert!(!ui.input_tag("pair-k-k").contains("in-line"));

        ui.click("pair-k-m");
        ui.click("pair-u-m");
        assert!(ui.has("class=\"tip-date\">M after U<"));
        assert!(!ui.has("class=\"tip-date\">M after K<"));
        assert!(ui.has(">Copied right every time here.<"));
        assert!(!ui.has(">Taken for<"), "nothing was taken for anything");
        ui.click_elsewhere();
        assert!(!ui.has("pair-card"));
        assert!(!ui.input_tag("pair-u-m").contains("selected"));
    }

    /// A mouse shows a pair's card for as long as it points at the square,
    /// over a pinned one, which comes back when it leaves. A finger does not
    /// point, and a pair not heard yet has no card.
    #[test]
    fn pointing_at_a_pair_shows_its_card() {
        let mut ui = Ui::new(MistakesHarness, ());
        ui.point_at("pair-start-k", "mouse", (40.0, 300.0), (5.0, 5.0));
        assert!(ui.has("class=\"tip-date\">K at the start of a group<"));
        assert!(ui.input_tag("pair-start-u").contains("in-line"));
        ui.point_away("pair-start-k");
        assert!(!ui.has("pair-card"));

        ui.click("pair-k-m");
        ui.point_at("pair-u-m", "mouse", (40.0, 300.0), (5.0, 5.0));
        assert!(ui.has("class=\"tip-date\">M after U<"));
        assert!(
            ui.input_tag("pair-k-m").contains("selected"),
            "still pinned"
        );
        ui.point_away("pair-u-m");
        assert!(ui.has("class=\"tip-date\">M after K<"));
        ui.click("pair-k-m");

        ui.point_at("pair-u-m", "touch", (40.0, 300.0), (5.0, 5.0));
        assert!(!ui.has("pair-card"));
        ui.point_at("pair-k-k", "mouse", (40.0, 300.0), (5.0, 5.0));
        assert!(!ui.has("pair-card"));
    }

    /// A mouse opens a day's card by pointing at the square and closes it by
    /// leaving. The browser's own plain tooltip is gone; the label says the
    /// same for a screen reader.
    #[test]
    fn pointing_at_a_day_opens_its_card() {
        let mut ui = Ui::new(HeatmapHarness, ());
        let square = ui.input_tag("day-2026-09-08");
        assert!(!square.contains("title="), "{square}");
        assert!(
            square.contains("aria-label=\"Tuesday 8 September 2026: 2 characters, 1 session"),
            "{square}"
        );

        ui.point_at("day-2026-09-08", "mouse", (410.0, 520.0), (10.0, 20.0));
        assert!(ui.has("class=\"day-tip\""), "the card opens above");
        assert!(ui.has("--cell-left: 400px;"));
        assert!(ui.has(">Characters<"));
        assert!(ui.input_tag("day-2026-09-08").contains("has-tip"));
        ui.point_away("day-2026-09-08");
        assert!(!ui.has("day-tip"));

        // Near the top of the window it opens below.
        ui.point_at("day-2026-09-08", "mouse", (410.0, 60.0), (10.0, 20.0));
        assert!(ui.has("class=\"day-tip below\""));
        ui.point_away("day-2026-09-08");

        // A finger does not point: only its tap opens a card.
        ui.point_at("day-2026-09-08", "touch", (410.0, 520.0), (10.0, 20.0));
        assert!(!ui.has("day-tip"));
        // Nor does a day still to come.
        ui.point_at("day-2026-09-10", "mouse", (410.0, 520.0), (10.0, 20.0));
        assert!(!ui.has("day-tip"));
    }

    #[test]
    fn each_month_has_its_own_tile() {
        let ui = Ui::new(HeatmapHarness, ());
        let html = ui.html();
        let tiles: Vec<usize> = ["June 2026", "July 2026", "August 2026", "September 2026"]
            .iter()
            .map(|name| {
                html.find(&format!(">{name}<"))
                    .unwrap_or_else(|| panic!("no tile for {name}"))
            })
            .collect();
        assert!(tiles.is_sorted(), "the months run oldest first");
        assert_eq!(html.matches("class=\"heatmap-month\"").count(), 4);
        // 2026-09-01 is a Tuesday, so it sits in the second column; the 2nd
        // just follows on.
        assert!(
            ui.input_tag("day-2026-09-01")
                .contains("grid-column-start: 2;")
        );
        assert!(!ui.input_tag("day-2026-09-02").contains("grid-column-start"));
        // A day of the month is not in the month before's tile.
        let september = tiles[3];
        assert!(
            html.find("id=\"day-2026-08-31\"")
                .is_some_and(|at| at < september)
        );
        assert!(
            html.find("id=\"day-2026-09-01\"")
                .is_some_and(|at| at > september)
        );
    }

    #[test]
    fn a_quiet_day_says_so() {
        let mut ui = Ui::new(HeatmapHarness, ());
        ui.click("day-2026-09-09");
        assert!(ui.has("Wednesday 9 September 2026"));
        assert!(ui.has("No practice"));
        assert!(!ui.has(">Characters<"));
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

    /// The band card on a 400-600 Hz side-tone range, which the 300 Hz
    /// filter passes as it is set.
    #[component]
    fn BandHarness(previewing: bool) -> Element {
        let settings = use_signal(|| {
            let mut settings = test_settings();
            settings.band.side_tone_min = 400.0;
            settings.band.side_tone_max = 600.0;
            settings
        });
        rsx! {
            BandConditionsCard {
                settings,
                previewing,
                on_preview: move |_| {},
                on_stop: move |_| {},
            }
        }
    }

    #[component]
    fn DefaultBandHarness() -> Element {
        let settings = use_signal(TrainingSettings::default);
        rsx! {
            BandConditionsCard {
                settings,
                previewing: false,
                on_preview: move |_| {},
                on_stop: move |_| {},
            }
        }
    }

    /// Out of the box the band is one station, fixed, under a subtle hiss,
    /// and the 300 Hz filter is built wider to pass the 300-600 Hz pitches.
    /// The noise moves a decibel of signal-to-noise a step, either way.
    #[test]
    fn the_band_card_out_of_the_box() {
        let mut ui = Ui::new(DefaultBandHarness, ());
        assert!(ui.shows("fixed-stations-calling"), "{}", ui.html());
        assert!(ui.has("class=\"slider-value\">S/N 41 dB<"), "{}", ui.html());
        assert!(ui.has("300 Hz — widened to 353 Hz to pass your pitch range"));
        // The 6 dB is where the width is read, not a loss, and the label
        // does not make it look like one.
        assert!(ui.has(r#"<span class="slider-name">Filter width</span>"#));
        assert!(!ui.has("-6 dB") && !ui.has("−6 dB"));
        assert!(ui.has("Its width is read between the points 6 dB down"));
        for (button, reading) in [
            ("more-slider-noise-level", 40),
            ("more-slider-noise-level", 39),
            ("more-slider-noise-level", 38),
            ("less-slider-noise-level", 39),
            ("less-slider-noise-level", 40),
            ("less-slider-noise-level", 41),
        ] {
            ui.click(button);
            let says = format!("class=\"slider-value\">S/N {reading} dB<");
            assert!(ui.has(&says), "{button}: {}", ui.html());
        }
        // A step under the quietest hiss there is, it is off.
        ui.click("less-slider-noise-level");
        assert!(ui.has("class=\"slider-value\">Off<"));

        // The RF gain starts at full and backs off a decibel a step.
        assert!(ui.has(r#"<span class="slider-name">RF gain</span>"#));
        assert!(ui.has(r#"<span class="slider-value">Full</span>"#));
        assert!(ui.input_tag("more-slider-rf-gain").contains("disabled"));
        ui.click("less-slider-rf-gain");
        assert!(ui.has("<span class=\"slider-value\">\u{2212}1 dB</span>"));

        // The AGC's threshold starts at its lowest and rises a decibel a
        // step; with the AGC off there is none to set, while the RF gain
        // still turns the band down.
        assert!(ui.has(r#"<span class="slider-name">AGC threshold</span>"#));
        assert!(ui.has(r#"<span class="slider-value">Lowest</span>"#));
        assert!(
            ui.input_tag("less-slider-agc-threshold")
                .contains("disabled")
        );
        ui.click("more-slider-agc-threshold");
        assert!(ui.has(r#"<span class="slider-value">+1 dB</span>"#));

        // The AGC's decay is Fast until you pick Slow, and is there only
        // while the AGC is on.
        assert!(ui.has(r#"<span class="field-label">AGC decay</span>"#));
        assert!(ui.input_tag("seg-agc-fast").contains("seg active"));
        ui.click("seg-agc-slow");
        assert!(ui.input_tag("seg-agc-slow").contains("seg active"));
        assert!(!ui.input_tag("seg-agc-fast").contains("active"));
        ui.commit("switch-agc", "false");
        assert!(!ui.has("AGC decay"));
        assert!(
            ui.input_tag("more-slider-agc-threshold")
                .contains("disabled")
        );
        assert!(ui.has("With the AGC off there is no threshold to set."));
        assert!(!ui.input_tag("less-slider-rf-gain").contains("disabled"));
        ui.click("less-slider-rf-gain");
        assert!(ui.has("<span class=\"slider-value\">\u{2212}2 dB</span>"));
    }

    #[test]
    fn the_band_card_opens_its_help_and_drives_the_receiver() {
        let mut ui = Ui::new(BandHarness, BandHarnessProps { previewing: false });
        assert!(ui.has("Live preview"));
        assert!(!ui.has("QSB slowly fades"));
        // The old resonator model's controls are gone for good.
        for gone in [
            "Model gain",
            "Wobble",
            "Resonance Q",
            "Whistle",
            "Advanced receiver",
        ] {
            assert!(!ui.has(gone), "{gone} should no longer be on the card");
        }

        assert!(!ui.has("Slow swells and dips of the signal."));
        ui.click("help-band-conditions");
        assert!(
            ui.has("Slow swells and dips of the signal."),
            "the help should have opened"
        );
        ui.click("help-band-conditions");
        assert!(
            !ui.has("Slow swells and dips of the signal."),
            "and closes again"
        );

        // The filter shape is a real choice, and it shows which one is in:
        // the sharp one out of the box.
        assert!(ui.has("id=\"seg-filter-sharp\" class=\"seg active\""));
        ui.click("seg-filter-soft");
        assert!(ui.has("id=\"seg-filter-soft\" class=\"seg active\""));
        ui.click("seg-filter-sharp");
        assert!(ui.has("id=\"seg-filter-sharp\" class=\"seg active\""));

        // The noise reads as a signal-to-noise ratio through the filter you
        // have set, so the readout follows both controls. The test settings
        // start with the noise switched off, which reads as off whatever the
        // level; switch it on first.
        ui.type_into("slider-noise-level", "0.5");
        assert!(ui.has("class=\"slider-value\">Off<"), "switched off");
        ui.commit("switch-band-noise", "true");
        assert!(
            ui.has("class=\"slider-value\">S/N 23 dB<"),
            "sharp, 300 Hz, at half level"
        );
        ui.type_into("slider-filter-width", "600");
        assert!(
            ui.has("class=\"slider-value\">S/N 20 dB<"),
            "doubling the filter should cost 3 dB"
        );
        ui.type_into("slider-filter-width", "300");
        ui.click("seg-filter-peaked");
        assert!(
            ui.has("class=\"slider-value\">S/N 26 dB<"),
            "the peak filter lets in less noise than its width says"
        );
        // Narrower than the side-tone range allows, it says what it is
        // really built at.
        assert!(!ui.has("filter-widened"));
        ui.type_into("slider-filter-width", "150");
        assert!(
            ui.has("150 Hz — widened to 233 Hz to pass your pitch range"),
            "the 400-600 Hz range takes 233 Hz"
        );
        ui.type_into("slider-noise-level", "0");
        assert!(ui.has("class=\"slider-value\">Off<"));

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
    fn a_slider_step_button_moves_the_value() {
        let mut ui = Ui::new(BandHarness, BandHarnessProps { previewing: false });
        assert!(ui.has("slider-bar"), "the track has to be in the markup");
        ui.click("switch-qrn-static");
        assert!(
            ui.has("class=\"slider-value\">25%<"),
            "QRN starts at a quarter"
        );
        ui.click("more-slider-qrn-intensity");
        assert!(
            ui.has("class=\"slider-value\">30%<"),
            "the + button is how the value moves when range thumbs do not paint"
        );
        ui.click("less-slider-qrn-intensity");
        assert!(ui.has("class=\"slider-value\">25%<"));
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
            Keyed { screen: Screen::Settings,
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
    }

    #[test]
    fn a_sample_that_is_playing_offers_a_stop_button() {
        let mut ui = Ui::new(SettingsHarness, ());
        assert!(ui.has("test-chip playing"));
        ui.click("btn-sample-stop");
        assert!(ui.has("Keying envelope"));
    }

    #[test]
    fn the_keyer_mode_can_be_switched() {
        let mut ui = Ui::new(SettingsHarness, ());
        ui.click("seg-ultimatic");
        assert!(ui.has("Last paddle you close wins"));
        ui.click("seg-straight");
        assert!(ui.has("One contact"));
        assert!(
            !ui.has("Swap [ and ] paddles"),
            "a straight key has no paddle direction: {}",
            ui.html()
        );
        ui.click("seg-iambic-b");
        assert!(ui.has("one extra opposite element"));
        assert!(ui.has("Swap [ and ] paddles"));
        assert!(
            ui.has("paddle-practice"),
            "the paddles have to be tryable in settings: {}",
            ui.html()
        );
        assert!(
            ui.has("Key speed"),
            "the paddle has its own speed, not the station's: {}",
            ui.html()
        );
    }

    #[component]
    fn PaddlePadHarness() -> Element {
        rsx! {
            Keyed { screen: Screen::Settings,
                PaddlePad { mode: KeyerMode::IambicA }
            }
        }
    }

    #[test]
    fn holding_the_dah_paddle_outside_training_sends_o() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(PaddlePadHarness, ());
            let key = Key::Character("]".into());
            let code = Code::BracketRight;
            // The pad has no listener of its own: a key pressed on it lands
            // on the nearest element that has one, the root.
            ui.press_key("keyed-root", key.clone(), code);
            ui.advance(500).await;
            ui.release_key("keyed-root", key, code);
            ui.advance(160).await;
            ui.advance(60).await;
            ui.advance(120).await;
            assert!(
                ui.has("paddle-heard") && ui.has(">O<"),
                "decoded letters have to land on the pad: {}",
                ui.html()
            );
        });
    }

    /// Tap the dit paddle `times` times, a letter space apart: that many Es.
    async fn tap_dits(ui: &mut Ui, on: &str, times: usize) {
        for _ in 0..times {
            ui.press_key(on, Key::Character("[".into()), Code::BracketLeft);
            ui.advance(30).await;
            ui.release_key(on, Key::Character("[".into()), Code::BracketLeft);
            ui.advance(250).await;
        }
    }

    #[test]
    fn the_pad_keeps_only_the_last_line_it_heard() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(PaddlePadHarness, ());
            tap_dits(&mut ui, "keyed-root", 40).await;
            let heard = ui
                .html()
                .split("id=\"paddle-heard\"")
                .nth(1)
                .and_then(|rest| rest.split('>').nth(1))
                .and_then(|text| text.split('<').next())
                .unwrap_or_default()
                .to_string();
            assert_eq!(heard, "E".repeat(32), "{}", ui.html());
        });
    }

    #[component]
    fn NoDeviceHarness() -> Element {
        rsx! {
            Keyed { screen: Screen::Settings, no_device: true,
                PaddlePad { mode: KeyerMode::IambicA }
            }
        }
    }

    /// With no sound device, the pad says so, and a held paddle looks for
    /// one when it closes — not twice an element while it is held, which
    /// is a device search on the UI thread for every dit.
    #[test]
    fn a_missing_sound_device_is_said_and_looked_for_once_a_squeeze() {
        crate::testing::run(|| async {
            DEVICE_LOOKS.with(|looks| looks.set(0));
            let mut ui = Ui::new(NoDeviceHarness, ());
            assert!(
                ui.has("No sidetone: No audio output device found"),
                "{}",
                ui.html()
            );
            let looks = || DEVICE_LOOKS.with(std::cell::Cell::get);
            let before = looks();
            let key = Key::Character("]".into());
            ui.press_key("keyed-root", key.clone(), Code::BracketRight);
            ui.advance(1_000).await;
            ui.release_key("keyed-root", key, Code::BracketRight);
            ui.advance(500).await;
            assert_eq!(looks() - before, 1, "one squeeze, one look");
            // The letter is keyed all the same; it just cannot be heard.
            assert!(ui.has("paddle-heard"));
        });
    }

    #[component]
    fn ClosingBoxHarness() -> Element {
        let mut shown = use_signal(|| true);
        let mut typed = use_signal(String::new);
        let answer = use_hook(AnswerOutbox::new);
        rsx! {
            Keyed { screen: Screen::Training,
                if shown() {
                    TrainingView {
                        current: 0,
                        groups: vec!["E".to_string()],
                        inputs: vec![typed()],
                        confirmed: vec![false],
                        focused: 0,
                        playing: false,
                        locked: false,
                        repeat_total: 1,
                        repeat_done: 1,
                        answer,
                        on_change: move |answer: TypedAnswer| typed.set(answer.text),
                        on_confirm: move |()| {},
                        on_submit: move |_| {},
                        on_stop: move |_| {},
                    }
                }
                button {
                    id: control_id("btn", "close the box"),
                    onclick: move |_| shown.set(false),
                    "close"
                }
                p { "typed:{typed}" }
            }
        }
    }

    /// Once the training view has gone, nothing it lent the keyer is called:
    /// a paddle on a training screen with no answer box open keys nothing.
    #[test]
    fn a_paddle_after_the_answer_box_has_gone_keys_nothing() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(ClosingBoxHarness, ());
            // Half a letter keyed into the box, then the box goes.
            ui.press_key(
                "group-input-0",
                Key::Character("[".into()),
                Code::BracketLeft,
            );
            ui.advance(20).await;
            ui.click("btn-close-the-box");
            ui.release_key("keyed-root", Key::Character("[".into()), Code::BracketLeft);
            tap_dits(&mut ui, "keyed-root", 2).await;
            ui.advance(500).await;
            assert!(ui.has("typed:<") || ui.has("typed:</"), "{}", ui.html());
        });
    }

    #[component]
    fn TrainingHarness() -> Element {
        let mut typed = use_signal(String::new);
        rsx! {
            TrainingScreen {
                current: 0,
                total: 2,
                groups: vec!["KM".to_string(), "UR".to_string()],
                inputs: vec![typed(), String::new()],
                confirmed: vec![false, false],
                focused: 0,
                playing: true,
                locked: true,
                repeat_total: 2,
                repeat_done: 0,
                on_change: move |answer: TypedAnswer| typed.set(answer.text),
                on_confirm: move |()| typed.set("confirmed".into()),
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    }

    // Under a runtime: typing and keying are timed, by the debounce and the
    // keyer.
    #[test]
    fn typing_is_refused_while_the_group_is_still_being_sent() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(TrainingHarness, ());
            assert!(ui.has("answer locked"));
            assert!(ui.has("Listening…"));
            // The keypress is swallowed rather than confirming the group.
            ui.press_enter("group-input-0");
            assert!(!ui.has("confirmed"));
        });
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
                on_change: move |answer: TypedAnswer| typed.set(answer.text),
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
            if !typed().is_empty() {
                p { id: "committed-answer", "committed:{typed()}" }
            }
        }
    }

    /// An open box whose every hand-over is kept, keys and all.
    #[component]
    fn KeyMomentsHarness() -> Element {
        rsx! {
            TrainingScreen {
                current: 0,
                total: 1,
                groups: vec!["KE".to_string()],
                inputs: vec![String::new()],
                confirmed: vec![false],
                focused: 0,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                on_change: move |answer: TypedAnswer| HANDED.with(|handed| handed.borrow_mut().push(answer)),
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    }

    /// Each character goes to the session with when it was keyed: a key
    /// typed at its keystroke, a paddle letter at the contact that began
    /// it, though it goes in only once its letter space has run out.
    #[test]
    fn each_character_goes_to_the_session_with_when_it_was_keyed() {
        crate::testing::run(|| async {
            HANDED.with(|handed| handed.borrow_mut().clear());
            let mut ui = Ui::new(KeyMomentsHarness, ());
            let typed_at = cw_core::MonoMs(crate::time::mono_ms());
            ui.type_into("group-input-0", "K");
            ui.advance(200).await;
            let pressed_at = cw_core::MonoMs(crate::time::mono_ms());
            tap_dits(&mut ui, "group-input-0", 1).await;
            let handed = |text: &str| {
                HANDED.with(|handed| {
                    handed
                        .borrow()
                        .iter()
                        .rev()
                        .find(|answer| answer.text == text)
                        .cloned()
                })
            };
            assert!(
                ui.run_until(2_000, |_| handed("KE").is_some()).await,
                "{}",
                ui.html()
            );
            let answer = handed("KE").expect("handed over");
            assert_eq!(answer.index, 0);
            assert_eq!(answer.keys.at, [Some(typed_at), Some(pressed_at)]);
            assert!(answer.keys.corrected.is_empty());
            assert!(
                crate::time::mono_ms() > pressed_at.0 + 200,
                "the letter went in after its letter space"
            );
        });
    }

    #[component]
    fn KeypadHarness() -> Element {
        let mut typed = use_signal(String::new);
        let mut confirms = use_signal(|| 0u32);
        rsx! {
            TrainingScreen {
                current: 0,
                total: 2,
                groups: vec!["KMR".to_string(), "UR".to_string()],
                inputs: vec![typed(), String::new()],
                confirmed: vec![false, false],
                focused: 0,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                on_change: move |answer: TypedAnswer| typed.set(answer.text),
                on_confirm: move |()| confirms += 1,
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
            if !typed().is_empty() {
                p { id: "committed-answer", "committed:{typed()}" }
            }
            p { "confirmed:{confirms}" }
        }
    }

    /// The keypad types into the box a key at a time and draws each key at
    /// once, takes the last one back with ⌫, and Next confirms what is
    /// there. A press takes no focus: the box keeps it.
    #[test]
    fn the_keypad_types_into_the_open_box() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(KeypadHarness, ());
            assert!(!ui.input_tag("key-k").contains("disabled"));
            ui.click("key-k");
            assert!(box_shows(&ui, "K"), "{}", ui.input_tag("group-input-0"));
            ui.click("key-x");
            assert!(box_shows(&ui, "KX"), "{}", ui.input_tag("group-input-0"));
            ui.click("key-delete");
            assert!(box_shows(&ui, "K"), "{}", ui.input_tag("group-input-0"));
            ui.click("key-m");
            assert!(!ui.has("committed:KM"), "handed over before the pause");
            ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS)).await;
            assert!(ui.has("committed:KM"), "{}", ui.html());
            // A pause to think costs nothing: the box is as it was.
            ui.advance(30_000).await;
            assert!(box_shows(&ui, "KM"), "{}", ui.input_tag("group-input-0"));
            assert!(ui.has("confirmed:0"));
            ui.click("key-next");
            assert!(ui.has("confirmed:1"), "{}", ui.html());
            // The full length goes to the session at once.
            ui.click("key-r");
            assert!(ui.has("committed:KMR"), "{}", ui.html());
            // ⌫ on an empty box, and every key on the way, is harmless.
            for _ in 0..5 {
                ui.click("key-delete");
            }
            assert!(box_shows(&ui, ""), "{}", ui.input_tag("group-input-0"));
            // Keys keep the focus where it is.
            assert!(ui.input_tag("key-k").contains("type=\"button\""));
        });
    }

    /// A key types as the finger lands: a touch the browser sends no click
    /// for — one that slid as it tapped — types all the same, and the click
    /// it does send after a clean tap is not a second key. A click with no
    /// finger behind it, a screen reader's, types too.
    #[test]
    fn a_key_types_as_the_finger_lands() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(KeypadHarness, ());
            ui.touch_down("key-k");
            assert!(box_shows(&ui, "K"), "{}", ui.input_tag("group-input-0"));
            ui.tap("key-m");
            assert!(box_shows(&ui, "KM"), "{}", ui.input_tag("group-input-0"));
            ui.tap("key-delete");
            assert!(box_shows(&ui, "K"), "{}", ui.input_tag("group-input-0"));
            ui.click("key-x");
            assert!(box_shows(&ui, "KX"), "{}", ui.input_tag("group-input-0"));
            ui.tap("key-delete");
            ui.touch_down("key-m");
            ui.touch_down("key-r");
            ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS)).await;
            assert!(ui.has("committed:KMR"), "{}", ui.html());
            ui.tap("key-next");
            assert!(ui.has("confirmed:1"), "{}", ui.html());
        });
    }

    /// A finger on a key of a box that is not taking typing types nothing.
    #[test]
    fn a_finger_on_a_shut_keypad_types_nothing() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(LockCycleHarness, ());
            ui.touch_down("key-k");
            ui.tap("key-m");
            assert!(box_shows(&ui, ""), "{}", ui.input_tag("group-input-0"));
            assert!(!ui.has("committed:"));
        });
    }

    #[component]
    fn FinishedGroupHarness() -> Element {
        let mut typed = use_signal(String::new);
        rsx! {
            TrainingScreen {
                current: 0,
                total: 2,
                groups: vec!["KM".to_string(), "UR".to_string()],
                inputs: vec!["KM".to_string(), String::new()],
                confirmed: vec![true, false],
                focused: 1,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                on_change: move |answer: TypedAnswer| typed.set(answer.text),
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
            if !typed().is_empty() {
                p { id: "committed-answer", "committed:{typed()}" }
            }
        }
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
                on_change: move |answer: TypedAnswer| {
                    HEARD.with(|heard| heard.borrow_mut().push((answer.index, answer.text.clone())));
                    typed.set(answer.text);
                },
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
        }
    }

    /// A key costs the renderer nothing, however long the session.
    ///
    /// The group card used to be written inline in the list's loop, which put
    /// the draft signal inside every iteration: one keystroke rebuilt and
    /// re-diffed every card on screen, and on the desktop build every one of
    /// those mutations crosses to the webview between the key going down and
    /// the letter appearing. With twenty groups that is most of a session's
    /// worth of DOM per character, and it is what made typing feel like wading.
    ///
    /// Even one box is one too many. The key is in the box before anything
    /// here hears of it, and drawing the box again writes its `value` back,
    /// which the desktop and mobile builds apply a frame later — over a key
    /// typed in that frame, which is then lost. The session hearing the
    /// answer once typing has paused changes nothing on screen either, so it
    /// must not redraw the box, or any other card, for it.
    #[test]
    fn typing_costs_the_same_whatever_the_session_length() {
        crate::testing::run(|| async {
            for count in [3, 30] {
                let cost = typing_cost(count).await;
                assert_eq!(
                    cost.key, 0,
                    "a key cost {} mutations in a {count}-group session — the box \
                     is being written back under the typing",
                    cost.key
                );
                assert_eq!(
                    cost.commit, 0,
                    "the session hearing an answer cost {} mutations in a \
                     {count}-group session — the box is being written back",
                    cost.commit
                );
            }
        });
    }

    struct TypingCost {
        key: usize,
        commit: usize,
    }

    /// What one key costs the renderer in a session of `count` groups, and
    /// what the session hearing it after the debounce costs.
    async fn typing_cost(count: usize) -> TypingCost {
        HEARD.with(|heard| heard.borrow_mut().clear());
        let heard = || HEARD.with(|heard| heard.borrow().clone());
        let mut ui = Ui::new(ManyGroupsHarness, ManyGroupsHarnessProps { count });
        let _ = ui.take_work();
        ui.type_into("group-input-0", "K");
        let key = ui.take_work();
        assert!(heard().is_empty(), "heard before typing had paused");
        ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS)).await;
        assert_eq!(
            heard(),
            [(0, "K".to_string())],
            "the session must hear the key once typing has paused"
        );
        TypingCost {
            key,
            commit: ui.take_work(),
        }
    }

    /// A paddle letter draws the one box it was keyed into, however long the
    /// session: only the open box reads the draft, and a letter is the one
    /// change to it the page has not seen.
    #[test]
    fn a_paddle_letter_costs_the_same_whatever_the_session_length() {
        crate::testing::run(|| async {
            let mut costs = Vec::new();
            for count in [3, 30] {
                HEARD.with(|heard| heard.borrow_mut().clear());
                let mut ui = Ui::new(ManyGroupsHarness, ManyGroupsHarnessProps { count });
                let _ = ui.take_work();
                tap_dits(&mut ui, "group-input-0", 1).await;
                assert!(box_shows(&ui, "E"), "{}", ui.input_tag("group-input-0"));
                costs.push(ui.take_work());
            }
            assert_eq!(
                costs[0], costs[1],
                "a letter cost {} mutations in a three-group session and {} in a \
                 thirty-group one — every box is being drawn for it",
                costs[0], costs[1]
            );
            assert!(costs[0] <= 2, "a letter cost {} mutations", costs[0]);
        });
    }

    /// One character short of the send stays in the box. The session does not
    /// hear it until the debounce has sat still — and the box is not drawn
    /// for it in the meantime, since the page shows the key already.
    #[test]
    fn an_incomplete_answer_stays_local_until_debounce() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(AnsweringHarness, ());
            let _ = ui.take_work();
            ui.type_into("group-input-0", "K");
            assert_eq!(ui.take_work(), 0, "the key was written back into the box");
            assert!(!ui.has("committed:"));
            // The harness clock moves in steps of 4 ms.
            ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS) - 4).await;
            assert!(!ui.has("committed:"), "heard before the pause was over");
            ui.advance(4).await;
            assert!(ui.has("committed:K"), "{}", ui.html());
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

    #[test]
    fn holding_the_dah_paddle_sends_o() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(AnsweringHarness, ());
            let key = Key::Character("]".into());
            let code = Code::BracketRight;
            ui.press_key("group-input-0", key.clone(), code);
            // Three dahs at 60 ms: each is 3 dits on + 1 dit space = 240 ms.
            // The third starts at 480 ms; release before the fourth at 720 ms.
            // The in-progress dah still finishes (a keyer does not clip it).
            ui.advance(500).await;
            ui.release_key("group-input-0", key, code);
            ui.advance(160).await; // rest of the third dah
            ui.advance(60).await; // trailing element space
            ui.advance(120).await; // letter gap
            ui.advance(48).await;
            assert!(ui.has("committed:O"), "{}", ui.html());
            // The page has not seen a keyed letter, so the box is drawn with it.
            let tag = ui.input_tag("group-input-0");
            assert!(tag.contains("value=\"O\""), "{tag}");
            assert!(
                !ui.html().contains("value=\"]\""),
                "the dah paddle must not land as a bracket: {}",
                ui.html()
            );
        });
    }

    /// Whether the box for group 0 is drawn showing `text`.
    fn box_shows(ui: &Ui, text: &str) -> bool {
        ui.input_tag("group-input-0")
            .contains(&format!("value=\"{text}\""))
    }

    /// A keyed letter is in the box the moment it is keyed, not once the
    /// session has heard it: nothing else would ever put it on the page.
    #[test]
    fn a_paddle_letter_is_drawn_into_the_box_at_once() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(AnsweringHarness, ());
            ui.press_key(
                "group-input-0",
                Key::Character("[".into()),
                Code::BracketLeft,
            );
            ui.advance(30).await;
            ui.release_key(
                "group-input-0",
                Key::Character("[".into()),
                Code::BracketLeft,
            );
            assert!(
                ui.run_until(400, |ui| box_shows(ui, "E")).await,
                "{}",
                ui.html()
            );
            // Seen at the first look after the letter landed, which is
            // sooner than the pause it starts is long.
            assert!(!ui.has("committed:"), "{}", ui.html());
            ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS)).await;
            assert!(ui.has("committed:E"), "{}", ui.html());
        });
    }

    /// Typing and keying share a box. A letter keyed after a key typed goes
    /// after it, and the box is drawn with both — the typed key was never
    /// drawn, only typed, so drawing the letter alone would wipe it.
    #[test]
    fn a_paddle_letter_keys_onto_what_was_typed() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(AnsweringHarness, ());
            ui.type_into("group-input-0", "K");
            tap_dits(&mut ui, "group-input-0", 1).await;
            assert!(box_shows(&ui, "KE"), "{}", ui.input_tag("group-input-0"));
            assert!(ui.has("committed:KE"), "{}", ui.html());
        });
    }

    /// The paddles key as the keyboard types: along with the send when
    /// typing is not locked while it plays.
    #[test]
    fn the_paddles_key_along_with_a_send_left_unlocked() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(RepeatingHarness, ());
            tap_dits(&mut ui, "group-input-0", 1).await;
            assert!(box_shows(&ui, "E"), "{}", ui.input_tag("group-input-0"));
            ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS)).await;
            assert!(ui.has("committed:E"), "{}", ui.html());
        });
    }

    /// And not into a box locked while its group is sent, as the keyboard
    /// does not.
    #[test]
    fn the_paddles_key_nothing_into_a_box_locked_while_it_is_sent() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(TrainingHarness, ());
            tap_dits(&mut ui, "group-input-0", 2).await;
            ui.advance(500).await;
            assert!(box_shows(&ui, ""), "{}", ui.input_tag("group-input-0"));
        });
    }

    #[component]
    fn RepeatingHarness() -> Element {
        let mut sends_done = use_signal(|| 0u32);
        let mut typed = use_signal(String::new);
        rsx! {
            TrainingScreen {
                current: 0,
                total: 1,
                groups: vec!["KMR".to_string()],
                inputs: vec![typed()],
                confirmed: vec![false],
                focused: 0,
                playing: true,
                locked: false,
                repeat_total: 2,
                repeat_done: sends_done(),
                on_change: move |answer: TypedAnswer| typed.set(answer.text),
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
            button {
                id: control_id("btn", "finish a send"),
                onclick: move |_| sends_done += 1,
                "next send"
            }
            if !typed().is_empty() {
                p { id: "committed-answer", "committed:{typed()}" }
            }
        }
    }

    /// A group sent again with its box left open moves the card's send
    /// counter on, and the box inside it is not drawn for that: drawing it
    /// would write its value back a frame late, over a key typed meanwhile.
    /// What is typed stays the page's own, and the session still hears it.
    #[test]
    fn the_send_counter_moves_without_drawing_the_box() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(RepeatingHarness, ());
            assert!(ui.has("Send 1/2"));
            ui.type_into("group-input-0", "K");
            ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS)).await;
            assert!(ui.has("committed:K"), "{}", ui.html());
            ui.type_into("group-input-0", "KM");
            let before = ui.input_tag("group-input-0");
            ui.click("btn-finish-a-send");
            assert!(ui.has("Send 2/2"), "{}", ui.html());
            assert_eq!(
                ui.input_tag("group-input-0"),
                before,
                "the box was drawn for the send counter"
            );
            ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS)).await;
            assert!(ui.has("committed:KM"), "{}", ui.html());
        });
    }

    #[component]
    fn RedrawnViewHarness() -> Element {
        let mut shown = use_signal(|| true);
        let mut typed = use_signal(String::new);
        let answer = use_hook(AnswerOutbox::new);
        rsx! {
            Keyed { screen: Screen::Training,
                if shown() {
                    TrainingView {
                        current: 0,
                        groups: vec!["KMR".to_string()],
                        inputs: vec![typed()],
                        confirmed: vec![false],
                        focused: 0,
                        playing: false,
                        locked: false,
                        repeat_total: 1,
                        repeat_done: 1,
                        answer,
                        on_change: move |answer: TypedAnswer| typed.set(answer.text),
                        on_confirm: move |()| {},
                        on_submit: move |_| {},
                        on_stop: move |_| {},
                    }
                }
                button {
                    id: control_id("btn", "redraw the view"),
                    onclick: move |_| shown.toggle(),
                    "redraw"
                }
                if !typed().is_empty() {
                    p { id: "committed-answer", "committed:{typed()}" }
                }
            }
        }
    }

    /// A training view drawn afresh on a session under way — as a hot reload
    /// does — opens the box on the answer the session holds, and a letter
    /// keyed into it goes after that answer.
    #[test]
    fn a_view_drawn_afresh_opens_the_box_on_the_answer_so_far() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(RedrawnViewHarness, ());
            ui.type_into("group-input-0", "K");
            ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS)).await;
            assert!(ui.has("committed:K"), "{}", ui.html());
            ui.click("btn-redraw-the-view");
            assert!(!ui.shows("group-input-0"));
            ui.click("btn-redraw-the-view");
            assert!(box_shows(&ui, "K"), "{}", ui.input_tag("group-input-0"));
            tap_dits(&mut ui, "group-input-0", 1).await;
            assert!(
                ui.run_until(400, |ui| box_shows(ui, "KE")).await,
                "{}",
                ui.input_tag("group-input-0")
            );
            ui.advance(u64::from(INPUT_COMMIT_DEBOUNCE_MS)).await;
            assert!(ui.has("committed:KE"), "{}", ui.html());
        });
    }

    #[component]
    fn StraightKeyHarness() -> Element {
        let mut typed = use_signal(String::new);
        rsx! {
            TrainingScreen {
                current: 0,
                total: 1,
                groups: vec!["E".to_string()],
                inputs: vec![typed()],
                confirmed: vec![false],
                focused: 0,
                playing: false,
                locked: false,
                repeat_total: 1,
                repeat_done: 1,
                keyer_mode: KeyerMode::Straight,
                on_change: move |answer: TypedAnswer| typed.set(answer.text),
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
            if !typed().is_empty() {
                p { id: "committed-answer", "committed:{typed()}" }
            }
        }
    }

    #[test]
    fn a_short_straight_key_press_sends_e() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(StraightKeyHarness, ());
            let key = Key::Character("[".into());
            let code = Code::BracketLeft;
            ui.press_key("group-input-0", key.clone(), code);
            ui.advance(50).await;
            ui.release_key("group-input-0", key, code);
            ui.advance(120).await;
            ui.advance(48).await;
            assert!(ui.has("committed:E"), "{}", ui.html());
        });
    }

    #[component]
    fn LockCycleHarness() -> Element {
        let mut locked = use_signal(|| true);
        let mut playing = use_signal(|| true);
        let mut typed = use_signal(String::new);
        rsx! {
            TrainingScreen {
                current: 0,
                total: 1,
                groups: vec!["KM".to_string()],
                inputs: vec![typed()],
                confirmed: vec![false],
                focused: 0,
                playing: playing(),
                locked: locked(),
                repeat_total: 1,
                repeat_done: if playing() { 0 } else { 1 },
                on_change: move |answer: TypedAnswer| typed.set(answer.text),
                on_confirm: move |()| {},
                on_submit: move |_| {},
                on_stop: move |_| {},
            }
            button {
                id: control_id("btn", "unlock group"),
                onclick: move |_| {
                    playing.set(false);
                    locked.set(false);
                },
                "unlock"
            }
            if !typed().is_empty() {
                p { id: "committed-answer", "committed:{typed()}" }
            }
        }
    }

    /// The box locked while the group is sent opens when it ends, and takes
    /// what is typed into it from then on.
    #[test]
    fn unlocking_the_answer_box_opens_it_for_typing() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(LockCycleHarness, ());
            let locked = ui.input_tag("group-input-0");
            assert!(locked.contains("answer locked"), "{locked}");
            ui.type_into("group-input-0", "KM");
            assert!(!ui.has("committed:"), "typing must not land while locked");
            // What was typed is turned away: the box is drawn empty again.
            assert!(box_shows(&ui, ""), "{}", ui.input_tag("group-input-0"));
            // The keypad is shut while the box is.
            assert!(ui.input_tag("key-k").contains("disabled"));
            ui.click("key-k");
            assert!(box_shows(&ui, ""), "{}", ui.input_tag("group-input-0"));

            ui.click("btn-unlock-group");
            let open = ui.input_tag("group-input-0");
            assert!(!open.contains("locked"), "{open}");
            assert!(!open.contains("readonly"), "{open}");
            assert!(!open.contains("disabled"), "{open}");
            assert!(!ui.input_tag("key-k").contains("disabled"));
            ui.type_into("group-input-0", "KM");
            assert!(ui.has("committed:KM"), "{}", ui.html());
        });
    }

    #[test]
    fn typing_into_a_finished_group_is_ignored() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(FinishedGroupHarness, ());
            // The finished group has no box left to type into, and the next
            // one's turns typing away until its group is sent.
            assert!(!ui.shows("group-input-0"));
            ui.type_into("group-input-1", "XX");
            assert!(!ui.has("committed:"));
            ui.click("key-x");
            assert!(!ui.has("committed:"));
        });
    }
}
