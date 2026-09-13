use std::rc::Rc;

use cw_core::{fit_settings_to_alphabet, GroupSession, SessionEvent};
use dioxus::prelude::*;

use crate::engine::{loop_preview_text, play_chars, play_sample_text, AppState, Screen};
use crate::persist::{load_sessions, load_settings, load_theme, save_settings, save_theme};
use crate::routes::app_routes;
use crate::session_runtime::{boot_machine_session, send_command, spawn_effects};
use crate::theme::Theme;
use crate::time::sleep_ms;
use crate::ui::widgets::Icon;

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
            app.shutdown_audio();
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
                boot_machine_session(settings_now.clone(), &history, &app, gen, runtime, screen)
            else {
                return;
            };
            spawn_effects(
                effects,
                settings_now,
                (*app).clone(),
                gen,
                runtime,
                screen,
                result,
                auto_message,
                sessions,
                settings,
                toast,
            );
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
                    app_loop.stop_audio();
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
                    app_loop.stop_audio();
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
            app.stop_audio();
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
            app.stop_audio();
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
            app.stop_audio();
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
            app.stop_audio();
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
            send_command(
                (*app).clone(),
                runtime,
                screen,
                result,
                auto_message,
                sessions,
                settings,
                toast,
                event,
            );
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
                                class: "btn btn-secondary btn-sm",
                                onclick: move |_| exit_training.call(()),
                                "Exit"
                            }
                        }
                    }
                }
                div { class: "screen", id: "screen-{screen_key}", key: "{screen_key}",
                    { app_routes(
                        screen,
                        settings,
                        sessions,
                        runtime,
                        result,
                        auto_message,
                        toast,
                        previewing(),
                        listen_playing(),
                        sample_playing(),
                        app.clone(),
                        start_training,
                        go_home,
                        go_listen,
                        start_band_preview,
                        stop_preview,
                        start_listen,
                        play_sample,
                    ) }
                }
            }
            if show_nav {
                nav { class: "bottom-nav",
                    button {
                        class: if matches!(screen(), Screen::Home | Screen::Listen | Screen::Results) { "nav-item active" } else { "nav-item" },
                        onclick: move |_| go_home.call(()),
                        Icon { name: "signal" }
                        span { "Practice" }
                    }
                    button {
                        class: if screen() == Screen::Stats { "nav-item active" } else { "nav-item" },
                        onclick: move |_| go_stats.call(()),
                        Icon { name: "chart" }
                        span { "Stats" }
                    }
                    button {
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
    use crate::testing::{run, test_settings, Ui};
    use cw_core::{CharSetMode, TrainingSettings};
    use dioxus::prelude::Key;

    /// Listener order follows the order Dioxus creates elements in: the app
    /// shell first (theme button, then the three nav buttons), then whatever
    /// the current screen mounted. Every test asserts what its click did, so a
    /// layout change fails loudly rather than passing quietly.
    const THEME: usize = 0;
    const NAV_PRACTICE: usize = 1;
    const NAV_STATS: usize = 2;
    const NAV_SETTINGS: usize = 3;
    const START: usize = 4;
    const LISTEN: usize = 5;
    /// First control belonging to the screen itself.
    const SCREEN: usize = 6;

    fn short_session() -> TrainingSettings {
        let mut settings = test_settings();
        settings.curriculum.num_groups = 1;
        settings
    }

    fn screen_of(ui: &Ui) -> String {
        ui.html()
            .split("id=\"screen-")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or("none")
            .to_string()
    }

    #[test]
    fn the_app_opens_on_the_practice_screen() {
        run(|| async {
            let (ui, _recorder) = Ui::app();
            assert_eq!(screen_of(&ui), "home");
            assert!(ui.has("Start training"));
            assert!(ui.has("bottom-nav"));
            assert_eq!(ui.count("click"), 6);
        });
    }

    #[test]
    fn the_bottom_nav_reaches_every_screen() {
        run(|| async {
            for (button, expected) in [
                (NAV_STATS, "stats"),
                (NAV_SETTINGS, "settings"),
                (NAV_PRACTICE, "home"),
                (LISTEN, "listen"),
            ] {
                let (mut ui, _recorder) = Ui::app();
                ui.click(button);
                assert_eq!(screen_of(&ui), expected, "button {button}");
            }
        });
    }

    #[test]
    fn the_theme_button_cycles_and_is_remembered() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            assert!(ui.has("Theme: follows your system"));
            ui.click(THEME);
            assert!(ui.has("Theme: light"));
            assert_eq!(crate::persist::load_theme(), "light");
            ui.click(THEME);
            assert!(ui.has("Theme: dark"));
            ui.click(THEME);
            assert!(ui.has("Theme: follows your system"));
            assert_eq!(crate::persist::load_theme(), "auto");
        });
    }

    #[test]
    fn a_session_can_be_played_from_the_practice_screen() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(short_session());
            ui.click(START);
            assert_eq!(screen_of(&ui), "training");
            assert!(ui.has("Listen — the answer box unlocks"));
            // The nav is out of the way while a session runs.
            assert!(!ui.has("bottom-nav"));
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);

            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.input(0, &sent);
            assert!(ui.run_until(5_000, |ui| screen_of(ui) == "results").await);
            assert!(ui.has("Session complete"));
            assert!(ui.has("Clean copy"));
            assert_eq!(crate::persist::load_sessions().len(), 1);
        });
    }

    #[test]
    fn a_typed_answer_can_be_confirmed_with_the_enter_key() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(short_session());
            ui.click(START);
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            // One character short, so nothing confirms on its own.
            ui.input(0, &sent[..1]);
            ui.advance(500).await;
            assert_eq!(screen_of(&ui), "training");
            // Keydown listener 0 is the app shell's; the group input is next.
            ui.keydown(1, Key::Enter);
            assert!(ui.run_until(2_000, |ui| screen_of(ui) == "results").await);
            assert!(ui.has("badge bad"));
        });
    }

    #[test]
    fn every_way_out_of_a_session_is_honoured() {
        run(|| async {
            // Exit in the header, End session, and Discard.
            for button in [SCREEN, SCREEN + 1, SCREEN + 2] {
                let (mut ui, _recorder) = Ui::app_with_settings(short_session());
                ui.click(START);
                assert!(ui.has("Exit"));
                ui.click(button);
                assert!(
                    ui.run_until(2_000, |ui| screen_of(ui) == "home").await,
                    "button {button} should have left the session"
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
            let (mut ui, recorder) = Ui::app_with_settings(settings);
            ui.click(START);
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.input(0, &sent);
            assert!(ui.run_until(5_000, |ui| ui.has("group done")).await);
            ui.click(SCREEN + 1); // End session
            assert!(ui.run_until(2_000, |ui| screen_of(ui) == "results").await);
            assert_eq!(crate::persist::load_sessions().len(), 1);
            assert!(ui.has("Group by group"));
        });
    }

    #[test]
    fn the_results_screen_can_start_another_session_or_go_back() {
        run(|| async {
            for (offset, expected) in [(0usize, "training"), (1, "home")] {
                let (mut ui, recorder) = Ui::app_with_settings(short_session());
                ui.click(START);
                assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
                let sent = recorder.texts().first().cloned().expect("a group");
                ui.input(0, &sent);
                assert!(ui.run_until(5_000, |ui| screen_of(ui) == "results").await);
                // The screen's own buttons, then the nav that came back with it:
                // Train again, Back, Practice, Stats, Settings.
                let last = ui.count("click");
                ui.click(last - 5 + offset);
                assert!(
                    ui.run_until(2_000, |ui| screen_of(ui) == expected).await,
                    "offset {offset} should have reached {expected}"
                );
            }
        });
    }

    #[test]
    fn an_audio_failure_is_shown_as_a_toast_that_clears_itself() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(short_session());
            recorder.build_error.set(true);
            ui.click(START);
            assert!(ui.has("class=\"toast\""), "the failure should be on screen");
            assert!(ui.has("No audio output device found"));
            // Still on the practice screen: no session was started.
            assert_eq!(screen_of(&ui), "home");
            assert!(ui.run_until(6_000, |ui| !ui.has("class=\"toast\"")).await);
        });
    }

    #[test]
    fn the_listen_screen_plays_a_character_and_stops() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click(LISTEN);
            assert!(ui.has("Play one character"));
            // Back, then Play <ch>, then Play all, then the character chips.
            ui.click(SCREEN + 1);
            assert!(ui.has("Stop"));
            assert!(ui.run_until(5_000, |_| !recorder.texts().is_empty()).await);
            assert_eq!(recorder.texts().first().map(String::as_str), Some("M"));

            // The Stop button is the newest control on the page.
            let last = ui.count("click");
            ui.click(last - 1);
            assert!(ui.has("Play all"));
        });
    }

    #[test]
    fn the_listen_button_goes_back_to_play_when_the_character_is_over() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click(LISTEN);
            ui.click(SCREEN + 1); // Play <ch>
            assert!(ui.has("Stop"));
            assert!(ui.run_until(20_000, |ui| !ui.has("Stop")).await);
            assert_eq!(recorder.texts().len(), 1);
            assert!(ui.has("Play all"));
        });
    }

    #[test]
    fn the_focus_of_a_group_is_reported_to_the_session() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click(START);
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
            // Focusing the group that is already current is accepted; the other
            // one is refused, and neither may disturb the session.
            ui.focus(0);
            assert!(ui.has("group focused"));
            if ui.count("focus") > 1 {
                ui.focus(1);
            }
            assert_eq!(screen_of(&ui), "training");
        });
    }

    #[test]
    fn a_sample_that_finishes_clears_the_playing_chip() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click(NAV_SETTINGS);
            let mut chip = None;
            for index in SCREEN..ui.count("click") {
                let (mut probe, probe_recorder) = Ui::app_with_settings(test_settings());
                probe.click(NAV_SETTINGS);
                probe.click(index);
                probe.advance(100).await;
                if probe.has("test-chip playing") {
                    assert!(!probe_recorder.texts().is_empty());
                    chip = Some(index);
                    break;
                }
            }
            let chip = chip.expect("a test chip");
            ui.click(chip);
            assert!(ui.has("test-chip playing"));
            assert!(
                ui.run_until(20_000, |ui| !ui.has("test-chip playing"))
                    .await
            );
            assert_eq!(recorder.texts().len(), 1);
        });
    }

    #[test]
    fn a_preview_that_loses_the_audio_stops_and_says_so() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click(NAV_SETTINGS);
            let mut preview = None;
            for index in SCREEN..ui.count("click") {
                let (mut probe, probe_recorder) = Ui::app_with_settings(test_settings());
                probe.click(NAV_SETTINGS);
                probe.click(index);
                probe.advance(400).await;
                if probe_recorder.texts().iter().any(|text| text == "CQ") {
                    preview = Some(index);
                    break;
                }
            }
            let button = preview.expect("the band card should offer a preview");
            ui.click(button);
            assert!(ui.run_until(5_000, |_| !recorder.texts().is_empty()).await);
            assert!(ui.has("Stop"));
            // The device disappears mid-loop.
            recorder.set(crate::audio::fake::Behaviour::RefuseToStart);
            assert!(ui.run_until(30_000, |ui| ui.has("class=\"toast\"")).await);
            assert!(ui.run_until(5_000, |ui| ui.has("Live preview")).await);
        });
    }

    #[test]
    fn the_listen_screen_can_play_the_whole_pool() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click(LISTEN);
            ui.click(SCREEN + 2); // Play all
            assert!(ui.run_until(20_000, |_| recorder.texts().len() >= 2).await);
            assert_eq!(recorder.texts()[..2], ["K".to_string(), "M".to_string()]);
        });
    }

    #[test]
    fn a_character_chip_changes_what_is_shown() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click(LISTEN);
            assert!(ui.has("Newest unlocked character"));
            ui.click(SCREEN + 3); // the first chip, K
            assert!(ui.has("From your current pool"));
            assert!(ui.has("listen-glyph"));
            // Back to the practice screen.
            ui.click(SCREEN);
            assert_eq!(screen_of(&ui), "home");
        });
    }

    /// Every control on a screen, pressed in turn on a fresh app: none of them
    /// may panic, and none may leave the settings in a state the trainer
    /// refuses to start from.
    async fn sweep(nav: usize, settings: TrainingSettings) {
        let (probe, _recorder) = Ui::app_with_settings(settings.clone());
        let mut probe = probe;
        probe.click(nav);
        let (clicks, inputs, changes) = (
            probe.count("click"),
            probe.count("input"),
            probe.count("change"),
        );
        drop(probe);

        for index in SCREEN..clicks {
            let (mut ui, _recorder) = Ui::app_with_settings(settings.clone());
            ui.click(nav);
            ui.click(index);
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!(stored.clone().clamp(), stored, "click {index}");
            assert!(!ui.html().is_empty());
        }
        for index in 0..inputs {
            for value in ["4", "0", "-3", "999", "", "KMU", "not a number"] {
                let (mut ui, _recorder) = Ui::app_with_settings(settings.clone());
                ui.click(nav);
                ui.input(index, value);
                ui.advance(50).await;
                let stored = crate::persist::load_settings();
                assert_eq!(stored.clone().clamp(), stored, "input {index} = {value:?}");
            }
        }
        for index in 0..changes {
            for value in ["true", "false", "4", "-1", "nonsense"] {
                let (mut ui, _recorder) = Ui::app_with_settings(settings.clone());
                ui.click(nav);
                ui.change(index, value);
                ui.advance(50).await;
                let stored = crate::persist::load_settings();
                assert_eq!(stored.clone().clamp(), stored, "change {index} = {value}");
            }
        }
    }

    #[test]
    fn shortening_the_alphabet_pulls_the_level_back_with_it() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 9;
            let (probe, _r) = Ui::app_with_settings(settings.clone());
            let inputs = {
                let mut probe = probe;
                probe.click(NAV_SETTINGS);
                probe.count("input")
            };
            // The sequence field is the one that takes letters.
            let mut found = false;
            for index in 0..inputs {
                let (mut ui, _r) = Ui::app_with_settings(settings.clone());
                ui.click(NAV_SETTINGS);
                ui.input(index, "KM");
                ui.advance(50).await;
                let stored = crate::persist::load_settings();
                if stored.curriculum.custom_sequence == vec!['K', 'M'] {
                    found = true;
                    // Two characters can only carry level 1.
                    assert_eq!(stored.curriculum.level, 1);
                    assert_eq!(stored.clone().clamp(), stored);
                    assert!(ui.has("K"));
                }
            }
            assert!(found, "the sequence field should accept characters");
        });
    }

    #[test]
    fn the_envelope_chips_send_a_sample_and_report_a_failure() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click(NAV_SETTINGS);
            let mut played = None;
            for index in SCREEN..ui.count("click") {
                let (mut ui, recorder) = Ui::app_with_settings(test_settings());
                ui.click(NAV_SETTINGS);
                ui.click(index);
                ui.advance(200).await;
                if !recorder.texts().is_empty() {
                    played = Some((index, recorder.texts()[0].clone()));
                    break;
                }
            }
            let (chip, text) = played.expect("a test chip should send something");
            assert!(!text.is_empty());

            // With no audio device the same chip reports the failure.
            recorder.build_error.set(true);
            ui.click(chip);
            ui.advance(200).await;
            assert!(ui.has("class=\"toast\""));
        });
    }

    #[test]
    fn the_band_preview_starts_and_stops_from_the_settings_screen() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click(NAV_SETTINGS);
            let mut preview = None;
            for index in SCREEN..ui.count("click") {
                let (mut ui, recorder) = Ui::app_with_settings(test_settings());
                ui.click(NAV_SETTINGS);
                ui.click(index);
                ui.advance(400).await;
                if recorder.texts().iter().any(|text| text == "CQ") {
                    preview = Some(index);
                    break;
                }
            }
            let button = preview.expect("the band card should offer a preview");
            ui.click(button);
            assert!(ui.run_until(5_000, |_| recorder.texts().len() >= 2).await);
            assert!(recorder.texts().iter().all(|text| text == "CQ"));
            assert!(ui.has("Stop"));

            // Leaving the screen stops the loop rather than leaving it running.
            ui.click(NAV_PRACTICE);
            assert_eq!(screen_of(&ui), "home");
            let sent = recorder.texts().len();
            ui.advance(3_000).await;
            assert_eq!(recorder.texts().len(), sent, "the loop should have stopped");
        });
    }

    #[test]
    fn a_preview_with_no_audio_device_is_reported() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            recorder.build_error.set(true);
            ui.click(NAV_SETTINGS);
            let mut toasted = false;
            for index in SCREEN..ui.count("click") {
                let (mut ui, recorder) = Ui::app_with_settings(test_settings());
                recorder.build_error.set(true);
                ui.click(NAV_SETTINGS);
                ui.click(index);
                ui.advance(100).await;
                if ui.has("No audio output device found") {
                    toasted = true;
                    break;
                }
            }
            assert!(toasted, "a control that plays should report a dead device");
        });
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_it() {
        run(|| async {
            sweep(NAV_SETTINGS, TrainingSettings::default()).await;
        });
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_a_custom_alphabet() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Custom;
            settings.curriculum.custom_set = vec!['K', 'M', 'U', 'R', 'E'];
            settings.curriculum.level = 4;
            sweep(NAV_SETTINGS, settings).await;
        });
    }

    #[test]
    fn nothing_on_the_stats_screen_can_break_it() {
        run(|| async {
            sweep(NAV_STATS, TrainingSettings::default()).await;
        });
    }

    #[test]
    fn the_stats_screen_shows_every_tab_of_a_real_history() {
        run(|| async {
            let history = [
                crate::testing::stored_session("2026-09-01", &[("KM", "KX"), ("MU", "MU")]),
                crate::testing::stored_session("2026-09-02", &[("KM", "KM")]),
            ];
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &history);
            ui.click(NAV_STATS);
            assert!(ui.has("Average"));
            assert!(ui.has("Accuracy over time"));

            // The five tabs are the newest controls on the page, and a row of
            // children is mounted last-first: Overview ends up at the end.
            let last = ui.count("click") - 1;
            for (back, marker) in [
                (1, "Letter accuracy"),
                (2, "Common confusions"),
                (3, "Sampling snapshot"),
                (4, "Session history"),
                (0, "Accuracy over time"),
            ] {
                ui.click(last - back);
                assert!(ui.has(marker), "tab {back} back should show {marker}");
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

            let history = [crate::testing::stored_session(
                &crate::time::local_date_string(),
                &[("KM", "KM")],
            )];
            let (ui, _recorder) = Ui::app_with_history(short_session(), &history);
            assert!(ui.has("Practice calendar"));
            assert!(ui.has("Last accuracy"));
            assert!(ui.has("streak"));
            assert!(ui.has("Tap a day for details."));
        });
    }
}
