use std::rc::Rc;

use cw_core::{fit_settings_to_alphabet, GroupSession, SessionEvent};
use dioxus::prelude::*;

use crate::engine::{
    loop_preview_text, play_chars, play_sample_text, AppState, Screen, SessionSignals,
};
use crate::persist::{load_sessions, load_settings, load_theme, save_settings, save_theme};
use crate::routes::{app_routes, AppCallbacks, ViewState};
use crate::session_runtime::{boot_machine_session, send_command, spawn_effects};
use crate::theme::Theme;
use crate::time::sleep_ms;
use crate::ui::widgets::{control_id, Icon};

/// Head tags are injected once, on mount. They live in their own component so
/// that App re-renders do not re-run them — `dioxus-document` warns on every
/// prop update of a head element ("Changing the props of `Style {}` is not
/// supported"), and a component with no props is memoized.
#[component]
fn AppHead() -> Element {
    rsx! {
        document::Title { "Dust" }
        document::Style { { include_str!("../assets/styles.css") } }
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1, viewport-fit=cover",
        }
        document::Meta { name: "theme-color", content: "#1b2436" }
        document::Meta { name: "mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-status-bar-style", content: "black-translucent" }
        document::Link {
            rel: "stylesheet",
            href: "https://fonts.googleapis.com/css2?family=Fraunces:opsz,wght@9..144,600;700&family=Figtree:wght@400;500;600;700&family=IBM+Plex+Mono:wght@500;700&display=optional",
        }
    }
}

#[component]
pub fn App() -> Element {
    let mut settings = use_signal(load_settings);
    let sessions = use_signal(load_sessions);
    let mut screen = use_signal(|| Screen::Home);
    let mut runtime = use_signal(|| None::<GroupSession>);
    let result = use_signal(|| None::<cw_core::SessionResult>);
    let auto_message = use_signal(|| None::<String>);
    let mut toast = use_signal(|| None::<String>);
    let mut previewing = use_signal(|| false);
    let mut listen_playing = use_signal(|| false);
    let mut sample_playing = use_signal(|| None::<String>);
    let mut theme = use_signal(|| Theme::from_key(&load_theme()));
    // The audio backend is injectable through context so a test can drive the
    // whole app with a player that records instead of one that needs a device.
    let app = use_hook(|| try_consume_context::<AppState>().unwrap_or_else(AppState::new));
    let app = Rc::new(app);
    let signals = SessionSignals {
        screen,
        runtime,
        result,
        auto_message,
        sessions,
        settings,
        toast,
    };

    use_effect(move || {
        let key = theme().key();
        save_theme(key);
        let _ = dioxus::document::eval(&format!(
            r#"(() => {{
                const root = document.documentElement;
                if ("{key}" === "auto") {{
                    root.removeAttribute("data-theme");
                }} else {{
                    root.setAttribute("data-theme", "{key}");
                }}
            }})()"#
        ));
    });

    use_effect(move || {
        if matches!(screen(), Screen::Training) {
            return;
        }
        let mut snapshot = settings();
        fit_settings_to_alphabet(&mut snapshot);
        let persist = snapshot.clone().clamp();
        if matches!(screen(), Screen::Settings) {
            if snapshot != settings() {
                settings.set(snapshot);
            }
        } else if persist != settings() {
            settings.set(persist.clone());
        }
        save_settings(&persist);
    });

    use_effect(move || {
        if let Some(message) = toast() {
            let mut toast = toast;
            spawn(async move {
                sleep_ms(4000).await;
                if toast.peek().as_deref() == Some(message.as_str()) {
                    toast.set(None);
                }
            });
        }
    });

    let go_home = use_callback({
        let app = app.clone();
        move |(): ()| {
            app.bump_session();
            app.silence_audio();
            previewing.set(false);
            listen_playing.set(false);
            sample_playing.set(None);
            runtime.set(None);
            screen.set(Screen::Home);
        }
    });

    let start_training = use_callback({
        let app = app.clone();
        move |(): ()| {
            let mut settings_now = settings().clamp();
            fit_settings_to_alphabet(&mut settings_now);
            if session_running(screen, runtime) {
                return;
            }
            if settings.peek().clone() != settings_now {
                settings.set(settings_now.clone());
            }
            previewing.set(false);
            listen_playing.set(false);
            sample_playing.set(None);
            let gen = match app.takeover_audio(&settings_now) {
                Ok(gen) => gen,
                Err(err) => {
                    toast.set(Some(err));
                    return;
                }
            };
            let history = sessions();
            let Some(effects) =
                boot_machine_session(settings_now.clone(), &history, &app, gen, signals)
            else {
                return;
            };
            spawn_effects(effects, settings_now, (*app).clone(), gen, signals);
        }
    });

    let start_listen = use_callback({
        let app = app.clone();
        move |chars: String| {
            if session_running(screen, runtime) {
                return;
            }
            let settings_now = settings().clamp();
            previewing.set(false);
            sample_playing.set(None);
            let gen = match app.takeover_audio(&settings_now) {
                Ok(gen) => gen,
                Err(err) => {
                    toast.set(Some(err));
                    return;
                }
            };
            listen_playing.set(true);
            let app_loop = (*app).clone();
            spawn(async move {
                play_chars(app_loop.clone(), gen, settings_now, chars, 420, toast).await;
                if app_loop.session_gen.get() == gen {
                    listen_playing.set(false);
                    app_loop.silence_audio();
                }
            });
        }
    });

    // Envelope test chips: send one short sample with the current keying settings.
    let play_sample = use_callback({
        let app = app.clone();
        move |text: String| {
            if session_running(screen, runtime) {
                return;
            }
            let settings_now = settings().clamp();
            let gen = match app.takeover_audio(&settings_now) {
                Ok(gen) => gen,
                Err(err) => {
                    toast.set(Some(err));
                    return;
                }
            };
            previewing.set(false);
            listen_playing.set(false);
            sample_playing.set(Some(text.clone()));
            let app_loop = (*app).clone();
            spawn(async move {
                play_sample_text(app_loop.clone(), gen, settings_now, text, toast).await;
                if app_loop.session_gen.get() == gen {
                    sample_playing.set(None);
                    app_loop.silence_audio();
                }
            });
        }
    });

    let start_band_preview = use_callback({
        let app = app.clone();
        move |(): ()| {
            if session_running(screen, runtime) {
                return;
            }
            let settings_now = settings().clamp();
            let gen = match app.takeover_audio(&settings_now) {
                Ok(gen) => gen,
                Err(err) => {
                    toast.set(Some(err));
                    return;
                }
            };
            previewing.set(true);
            listen_playing.set(false);
            sample_playing.set(None);
            let app_loop = (*app).clone();
            spawn(async move {
                loop_preview_text(app_loop.clone(), gen, settings, "CQ", 280, toast).await;
                if app_loop.session_gen.get() == gen {
                    previewing.set(false);
                }
            });
        }
    });

    let stop_preview = use_callback({
        let app = app.clone();
        move |(): ()| {
            if session_running(screen, runtime) {
                return;
            }
            app.bump_session();
            app.silence_audio();
            previewing.set(false);
            listen_playing.set(false);
            sample_playing.set(None);
        }
    });

    use_effect({
        let app = (*app).clone();
        move || {
            if !previewing() {
                return;
            }
            let settings_now = settings().clamp();
            app.apply_band_live(&settings_now);
        }
    });

    let go_listen = use_callback({
        let app = app.clone();
        move |(): ()| {
            if session_running(screen, runtime) {
                return;
            }
            app.bump_session();
            app.silence_audio();
            previewing.set(false);
            listen_playing.set(false);
            sample_playing.set(None);
            screen.set(Screen::Listen);
        }
    });
    let go_stats = use_callback({
        let app = app.clone();
        move |(): ()| {
            if session_running(screen, runtime) {
                return;
            }
            app.bump_session();
            app.silence_audio();
            previewing.set(false);
            listen_playing.set(false);
            sample_playing.set(None);
            screen.set(Screen::Stats);
        }
    });
    let go_settings = use_callback({
        let app = app.clone();
        move |(): ()| {
            if session_running(screen, runtime) {
                return;
            }
            app.bump_session();
            app.silence_audio();
            previewing.set(false);
            listen_playing.set(false);
            sample_playing.set(None);
            screen.set(Screen::Settings);
        }
    });
    let exit_training = use_callback({
        let app = app.clone();
        move |(): ()| {
            let event = match runtime.peek().as_ref() {
                Some(session) if session.any_confirmed() => SessionEvent::FinishNow,
                Some(_) => SessionEvent::Abort,
                None => {
                    go_home.call(());
                    return;
                }
            };
            send_command((*app).clone(), signals, event);
        }
    });
    let show_nav = !matches!(screen(), Screen::Training);
    let shell_class = if show_nav { "shell has-nav" } else { "shell" };
    let screen_key = screen_key(screen());

    rsx! {
        AppHead {}
        div {
            class: "app-root",
            onkeydown: move |e| {
                if e.key() == Key::F11 {
                    e.prevent_default();
                    toggle_fullscreen();
                }
            },
            div { class: shell_class,
                header { class: "header-bar",
                    div { class: "brand",
                        span { class: "brand-mark", aria_hidden: "true", "·−" }
                        div {
                            p { class: "brand-name", "Dust" }
                            p { class: "brand-sub", "CW group trainer" }
                        }
                    }
                    div { class: "header-actions",
                        button {
                            id: control_id("btn", "theme"),
                            class: "icon-btn",
                            title: "{theme().label()}",
                            aria_label: "{theme().label()}",
                            onclick: move |_| {
                                let next = theme().next();
                                theme.set(next);
                            },
                            Icon { name: theme().icon() }
                        }
                        if screen() == Screen::Training {
                            button {
                                id: control_id("btn", "exit"),
                                class: "btn btn-secondary btn-sm",
                                onclick: move |_| exit_training.call(()),
                                "Exit"
                            }
                        }
                    }
                }
                div { class: "screen", id: "screen-{screen_key}", key: "{screen_key}",
                    { app_routes(
                        signals,
                        ViewState {
                            previewing: previewing(),
                            listen_playing: listen_playing(),
                            sample_playing: sample_playing(),
                        },
                        app.clone(),
                        AppCallbacks {
                            start_training,
                            go_home,
                            go_listen,
                            start_band_preview,
                            stop_preview,
                            start_listen,
                            play_sample,
                        },
                    ) }
                }
            }
            if show_nav {
                nav { class: "bottom-nav",
                    button {
                        id: control_id("nav", "practice"),
                        class: if matches!(screen(), Screen::Home | Screen::Listen | Screen::Results) { "nav-item active" } else { "nav-item" },
                        onclick: move |_| go_home.call(()),
                        Icon { name: "signal" }
                        span { "Practice" }
                    }
                    button {
                        id: control_id("nav", "stats"),
                        class: if screen() == Screen::Stats { "nav-item active" } else { "nav-item" },
                        onclick: move |_| go_stats.call(()),
                        Icon { name: "chart" }
                        span { "Stats" }
                    }
                    button {
                        id: control_id("nav", "settings"),
                        class: if screen() == Screen::Settings { "nav-item active" } else { "nav-item" },
                        onclick: move |_| go_settings.call(()),
                        Icon { name: "sliders" }
                        span { "Settings" }
                    }
                }
            }
            if let Some(message) = toast() {
                div { class: "toast", "{message}" }
            }
        }
    }
}

fn screen_key(screen: Screen) -> &'static str {
    match screen {
        Screen::Home => "home",
        Screen::Settings => "settings",
        Screen::Training => "training",
        Screen::Results => "results",
        Screen::Stats => "stats",
        Screen::Listen => "listen",
    }
}

fn session_running(screen: Signal<Screen>, runtime: Signal<Option<GroupSession>>) -> bool {
    matches!(screen(), Screen::Training) || runtime.peek().is_some()
}

fn toggle_fullscreen() {
    #[cfg(feature = "desktop")]
    {
        let desktop = dioxus::desktop::window();
        let fullscreen = desktop.window.fullscreen().is_some();
        desktop.set_fullscreen(!fullscreen);
    }
    #[cfg(feature = "web")]
    {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        if document.fullscreen_element().is_some() {
            document.exit_fullscreen();
            return;
        }
        if let Some(element) = document.document_element() {
            let _ = element.request_fullscreen();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::screen_key;
    use crate::engine::Screen;

    #[test]
    fn every_screen_has_its_own_key() {
        let keys: Vec<&str> = [
            Screen::Home,
            Screen::Settings,
            Screen::Training,
            Screen::Results,
            Screen::Stats,
            Screen::Listen,
        ]
        .into_iter()
        .map(screen_key)
        .collect();
        // The key remounts the screen wrapper, so it has to differ per screen.
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len());
    }
}

#[cfg(test)]
mod ui_tests {
    use crate::testing::{run, stored_session, test_settings, Ui};
    use cw_core::{CharSetMode, PracticeWindow, TrainingSettings};

    fn short_session() -> TrainingSettings {
        let mut settings = test_settings();
        settings.curriculum.num_groups = 1;
        settings
    }

    /// Start a session and wait for the first group's answer box to open.
    async fn awaiting_answer(
        settings: TrainingSettings,
    ) -> (Ui, std::rc::Rc<crate::audio::fake::Recorder>) {
        let (mut ui, recorder) = Ui::app_with_settings(settings);
        ui.click("btn-start-training");
        assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
        (ui, recorder)
    }

    #[test]
    fn the_app_opens_on_the_practice_screen() {
        run(|| async {
            let (ui, _recorder) = Ui::app();
            assert_eq!(ui.screen(), "home");
            assert!(ui.has("Start training"));
            assert!(ui.has("bottom-nav"));
        });
    }

    #[test]
    fn the_bottom_nav_reaches_every_screen() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            for (button, expected) in [
                ("nav-stats", "stats"),
                ("nav-settings", "settings"),
                ("nav-practice", "home"),
                ("btn-listen-to-letters", "listen"),
                ("btn-listen-back", "home"),
            ] {
                ui.click(button);
                assert_eq!(ui.screen(), expected, "after pressing {button}");
            }
        });
    }

    #[test]
    fn the_theme_button_cycles_and_is_remembered() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            for (shown, key) in [
                ("Theme: light", "light"),
                ("Theme: dark", "dark"),
                ("Theme: follows your system", "auto"),
            ] {
                ui.click("btn-theme");
                assert!(ui.has(shown));
                assert_eq!(crate::persist::load_theme(), key);
            }
        });
    }

    #[test]
    fn a_session_can_be_played_from_the_practice_screen() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(short_session());
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert!(ui.has("Listen — the answer box unlocks"));
            // The nav is out of the way while a session runs.
            assert!(!ui.has("bottom-nav"));
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);

            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(ui.run_until(5_000, |ui| ui.screen() == "results").await);
            assert!(ui.has("Session complete"));
            assert!(ui.has("Clean copy"));
            assert_eq!(crate::persist::load_sessions().len(), 1);
        });
    }

    #[test]
    fn a_typed_answer_can_be_confirmed_with_the_enter_key() {
        run(|| async {
            let (mut ui, recorder) = awaiting_answer(short_session()).await;
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            // One character short, so nothing confirms on its own.
            ui.type_into("group-input-0", &sent[..1]);
            ui.advance(500).await;
            assert_eq!(ui.screen(), "training");
            ui.press_enter("group-input-0");
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            assert!(ui.has("badge bad"));
        });
    }

    #[test]
    fn every_way_out_of_a_session_is_honoured() {
        run(|| async {
            for button in ["btn-exit", "btn-end-session", "btn-discard"] {
                let (mut ui, _recorder) = Ui::app_with_settings(short_session());
                ui.click("btn-start-training");
                ui.click(button);
                assert!(
                    ui.run_until(2_000, |ui| ui.screen() == "home").await,
                    "{button} should have left the session"
                );
                assert!(crate::persist::load_sessions().is_empty());
            }
        });
    }

    #[test]
    fn ending_a_session_early_keeps_the_answered_groups() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 3;
            let (mut ui, recorder) = awaiting_answer(settings).await;
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(ui.run_until(5_000, |ui| ui.has("group done")).await);
            ui.click("btn-end-session");
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            assert_eq!(crate::persist::load_sessions().len(), 1);
            assert!(ui.has("Group by group"));
        });
    }

    #[test]
    fn the_results_screen_can_start_another_session_or_go_back() {
        run(|| async {
            for (button, expected) in [("btn-train-again", "training"), ("btn-back", "home")] {
                let (mut ui, recorder) = awaiting_answer(short_session()).await;
                let sent = recorder.texts().first().cloned().expect("a group");
                ui.type_into("group-input-0", &sent);
                assert!(ui.run_until(5_000, |ui| ui.screen() == "results").await);
                ui.click(button);
                assert!(
                    ui.run_until(2_000, |ui| ui.screen() == expected).await,
                    "{button} should have reached {expected}"
                );
            }
        });
    }

    #[test]
    fn focusing_a_group_is_reported_to_the_session() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            let (mut ui, _recorder) = awaiting_answer(settings).await;
            ui.focus("group-input-0");
            assert!(ui.has("group focused"));
            assert_eq!(ui.screen(), "training");
        });
    }

    #[test]
    fn an_audio_failure_is_shown_as_a_toast_that_clears_itself() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(short_session());
            recorder.build_error.set(true);
            ui.click("btn-start-training");
            assert!(ui.has("class=\"toast\""), "the failure should be on screen");
            assert!(ui.has("No audio output device found"));
            // Still on the practice screen: no session was started.
            assert_eq!(ui.screen(), "home");
            assert!(ui.run_until(6_000, |ui| !ui.has("class=\"toast\"")).await);
        });
    }

    #[test]
    fn the_listen_screen_plays_one_character_and_stops() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            assert!(ui.has("Play one character"));
            ui.click("btn-play");
            assert!(ui.has("Stop"));
            assert!(ui.run_until(5_000, |_| !recorder.texts().is_empty()).await);
            assert_eq!(recorder.texts(), vec!["M".to_string()]);
            ui.click("btn-stop");
            assert!(ui.has("Play all"));
        });
    }

    #[test]
    fn the_listen_screen_lets_a_character_finish_on_its_own() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            ui.click("btn-play");
            assert!(ui.run_until(20_000, |ui| !ui.has("Stop")).await);
            assert_eq!(recorder.texts().len(), 1);
            assert!(ui.has("Play all"));
        });
    }

    #[test]
    fn the_listen_screen_can_play_the_whole_pool() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            ui.click("btn-play-all");
            assert!(ui.run_until(20_000, |_| recorder.texts().len() >= 2).await);
            assert_eq!(recorder.texts()[..2], ["K".to_string(), "M".to_string()]);
        });
    }

    #[test]
    fn a_character_chip_changes_what_is_shown() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            assert!(ui.has("Newest unlocked character"));
            // K is the older of the two, so it is not the newest.
            ui.click("chip-k");
            assert!(ui.has("From your current pool"));
            assert!(ui.has("listen-glyph"));
        });
    }

    #[test]
    fn the_character_set_can_be_switched_and_is_saved() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            ui.click("nav-settings");
            for (button, note, mode) in [
                (
                    "seg-koch",
                    "Letters unlocked in Koch order",
                    CharSetMode::Koch,
                ),
                ("seg-digits", "Digits only", CharSetMode::Digits),
                ("seg-custom", "Your own character list", CharSetMode::Custom),
                (
                    "seg-mixed",
                    "Letters and digits together",
                    CharSetMode::Mixed,
                ),
            ] {
                ui.click(button);
                assert!(ui.has(note), "{button} should say {note}");
                assert_eq!(
                    crate::persist::load_settings().curriculum.char_set_mode,
                    mode
                );
            }
            assert!(ui.has("Share of letters"));
        });
    }

    #[test]
    fn the_unlock_order_can_be_swapped_for_another_preset() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            ui.click("nav-settings");
            ui.click("pill-cw-academy");
            let stored = crate::persist::load_settings();
            assert_eq!(cw_core::sequence_preset_id(&stored), "cw-academy");
            assert!(!stored.curriculum.sequence_is_custom);
            assert!(stored.progress_alphabet().starts_with(&['E', 'T', 'A']));
            ui.click("pill-lcwo");
            assert!(crate::persist::load_settings()
                .curriculum
                .custom_sequence
                .is_empty());
        });
    }

    #[test]
    fn the_practice_window_can_be_narrowed_to_the_newest_characters() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 9;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            for (button, window, start) in [
                ("pill-newest-3", PracticeWindow::Last3, 8),
                ("pill-newest-5", PracticeWindow::Last5, 6),
                ("pill-everything", PracticeWindow::All, 1),
            ] {
                ui.click(button);
                let stored = crate::persist::load_settings();
                assert_eq!(stored.curriculum.practice_window, Some(window));
                assert_eq!(stored.curriculum.sliding_window_start, start);
            }
        });
    }

    #[test]
    fn shortening_the_alphabet_pulls_the_level_back_with_it() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 9;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            ui.type_into("field-sequence-order", "KM");
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!(stored.curriculum.custom_sequence, vec!['K', 'M']);
            // Two characters can only carry level 1.
            assert_eq!(stored.curriculum.level, 1);
            assert_eq!(stored.clone().clamp(), stored);
        });
    }

    #[test]
    fn an_envelope_chip_sends_a_sample_and_clears_when_it_is_over() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("chip-dit");
            assert!(ui.has("test-chip playing"));
            assert_eq!(recorder.texts(), vec!["E".to_string()]);
            assert!(
                ui.run_until(20_000, |ui| !ui.has("test-chip playing"))
                    .await
            );
        });
    }

    #[test]
    fn an_envelope_chip_with_no_audio_device_says_so() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            recorder.build_error.set(true);
            ui.click("nav-settings");
            ui.click("chip-dah");
            ui.advance(200).await;
            assert!(ui.has("No audio output device found"));
        });
    }

    #[test]
    fn the_band_preview_loops_until_the_screen_is_left() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| recorder.texts().len() >= 2).await);
            assert!(recorder.texts().iter().all(|text| text == "CQ"));

            ui.click("nav-practice");
            assert_eq!(ui.screen(), "home");
            let sent = recorder.texts().len();
            ui.advance(3_000).await;
            assert_eq!(recorder.texts().len(), sent, "the loop should have stopped");
        });
    }

    #[test]
    fn the_band_preview_can_be_stopped_where_it_was_started() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            ui.click("btn-band-stop");
            let sent = recorder.texts().len();
            ui.advance(3_000).await;
            assert_eq!(recorder.texts().len(), sent);
            assert!(ui.has("Live preview"));
        });
    }

    /// Test settings run a dead-quiet band; these tests need one that is on.
    fn with_receiver() -> TrainingSettings {
        let mut settings = test_settings();
        settings.band.qrm_enabled = true;
        settings.band.qrm_level = 0.4;
        settings
    }

    /// Pressing stop has to stop everything. The receiver background lives on
    /// its own stream, outliving any one send, so stopping only the send left
    /// it hissing away with nothing to hear it under — and the quieter the
    /// band used to be, the longer that went unnoticed.
    #[test]
    fn stopping_the_preview_silences_the_receiver_too() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(with_receiver());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            assert!(
                recorder.band_running.get(),
                "the preview should have the receiver running"
            );

            ui.click("btn-band-stop");
            ui.advance(200).await;
            assert!(
                !recorder.band_running.get(),
                "stop left the receiver background playing"
            );
        });
    }

    /// Same for walking away from the screen that was making the sound.
    #[test]
    fn leaving_the_screen_silences_the_receiver_too() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(with_receiver());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            assert!(recorder.band_running.get());

            ui.click("nav-practice");
            ui.advance(200).await;
            assert_eq!(ui.screen(), "home");
            assert!(
                !recorder.band_running.get(),
                "the receiver followed us off the screen"
            );
        });
    }

    #[test]
    fn a_preview_that_loses_the_audio_stops_and_says_so() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            // The device disappears mid-loop.
            recorder.set(crate::audio::fake::Behaviour::RefuseToStart);
            assert!(ui.run_until(30_000, |ui| ui.has("class=\"toast\"")).await);
            assert!(ui.run_until(5_000, |ui| ui.has("Live preview")).await);
        });
    }

    #[test]
    fn a_preview_with_no_audio_device_is_reported() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            recorder.build_error.set(true);
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            ui.advance(100).await;
            assert!(ui.has("No audio output device found"));
            assert!(ui.has("Live preview"), "nothing should be playing");
        });
    }

    #[test]
    fn the_stats_screen_shows_every_tab_of_a_real_history() {
        run(|| async {
            let history = [
                stored_session("2026-09-01", &[("KM", "KX"), ("MU", "MU")]),
                stored_session("2026-09-02", &[("KM", "KM")]),
            ];
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &history);
            ui.click("nav-stats");
            assert!(ui.has("Average"));
            for (tab, marker) in [
                ("seg-letters", "Letter accuracy"),
                ("seg-mistakes", "Common confusions"),
                ("seg-sampling", "Sampling snapshot"),
                ("seg-history", "Session history"),
                ("seg-overview", "Accuracy over time"),
            ] {
                ui.click(tab);
                assert!(ui.has(marker), "{tab} should show {marker}");
            }
        });
    }

    #[test]
    fn the_practice_screen_shows_a_history_once_there_is_one() {
        run(|| async {
            let (ui, _recorder) = Ui::app_with_settings(short_session());
            assert!(!ui.has("Practice calendar"));
            assert!(ui.has("First session"));
            drop(ui);

            let history = [stored_session(
                &crate::time::local_date_string(),
                &[("KM", "KM")],
            )];
            let (ui, _recorder) = Ui::app_with_history(short_session(), &history);
            assert!(ui.has("Practice calendar"));
            assert!(ui.has("Last accuracy"));
            assert!(ui.has("streak"));
        });
    }

    /// Every control on a screen, pressed and typed into in turn: none may
    /// panic, and none may leave settings the trainer would refuse to start
    /// from. Each one starts from a fresh app, so the sweep cannot walk itself
    /// off the screen it is testing, and every panel behind a toggle is opened
    /// first so the sweep reaches what the screen hides as well as what it
    /// shows.
    async fn sweep(nav: &str, settings: TrainingSettings) {
        let (mut probe, _recorder) = Ui::app_with_settings(settings.clone());
        probe.click(nav);
        probe.open_disclosures();
        let controls = probe.controls("");
        drop(probe);
        assert!(controls.len() > 5, "the screen should have controls");

        for control in controls {
            for value in ["4", "-3", "999", "KMU", "true"] {
                let (mut ui, _recorder) = Ui::app_with_settings(settings.clone());
                ui.click(nav);
                ui.open_disclosures();
                if !ui.shows(&control) {
                    continue;
                }
                ui.click(&control);
                if ui.shows(&control) {
                    ui.type_into(&control, value);
                }
                if ui.shows(&control) {
                    ui.commit(&control, value);
                }
                ui.advance(50).await;
                let stored = crate::persist::load_settings();
                assert_eq!(stored.clone().clamp(), stored, "{value:?} into {control}");
                assert!(!ui.html().is_empty());
                ui.character_boxes_hold_single_characters();
            }
        }
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_it() {
        run(|| async {
            sweep("nav-settings", TrainingSettings::default()).await;
        });
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_a_custom_alphabet() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Custom;
            settings.curriculum.custom_set = vec!['K', 'M', 'U', 'R', 'E'];
            settings.curriculum.level = 4;
            sweep("nav-settings", settings).await;
        });
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_callsign_mode() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            settings.curriculum.callsign_level = 4;
            sweep("nav-settings", settings).await;
        });
    }

    /// The controls that mean nothing for a callsign have to be gone, not just
    /// ignored: a group-size slider that does nothing is worse than no slider.
    #[test]
    fn the_settings_screen_only_offers_what_a_callsign_has() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            assert!(ui.has("Callsign tier"));
            assert!(ui.has("Callsigns per session"));
            assert!(ui.has("What this tier sends"));
            for gone in [
                "field-sequence-order",
                "field-custom-alphabet",
                "field-digits-level",
            ] {
                assert!(!ui.shows(gone), "{gone} has no meaning for a callsign");
            }
            assert!(!ui.has("Practice window"), "there is no unlock order");
            assert!(!ui.has("Group size"), "a callsign is as long as it is");

            // The tier changes what the preview shows.
            let before = ui.html();
            ui.commit("field-level", "6");
            ui.advance(50).await;
            assert_ne!(before, ui.html(), "the preview should follow the tier");
            assert_eq!(crate::persist::load_settings().curriculum.callsign_level, 6);
        });
    }

    /// Switching away and back must not leave one mode's level in the other's
    /// field — the whole reason callsigns carry their own.
    #[test]
    fn each_character_set_keeps_its_own_level() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 9;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            ui.click("seg-callsigns");
            ui.commit("field-level", "5");
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!(stored.curriculum.callsign_level, 5);
            assert_eq!(stored.curriculum.level, 9, "the Koch level is untouched");

            ui.click("seg-koch");
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!(stored.curriculum.level, 9);
            assert_eq!(stored.curriculum.callsign_level, 5);
        });
    }

    #[test]
    fn nothing_on_the_stats_screen_can_break_it() {
        run(|| async {
            sweep("nav-stats", TrainingSettings::default()).await;
        });
    }
}
