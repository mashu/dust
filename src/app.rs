//! The shell every screen sits in: the header, the navigation, the toasts,
//! and the state they all share.

use std::cell::Cell;
use std::rc::Rc;

use dioxus::prelude::*;

use crate::contest_runtime::focus_field;
use crate::keep_awake::use_keep_awake;
use crate::routes::Route;
use crate::session_runtime::finish_run;
use crate::state::{AppCtx, AppState, Screen, ToastAction};
use crate::theme::Theme;
use crate::time::sleep_ms;
use crate::ui::contest::RunAnnouncer;
use crate::ui::paddle::use_paddle_keys;
use crate::ui::tour::{TourButton, TourDock};
use crate::ui::widgets::{Icon, IconName, control_id, keep_focus};

// Desktop inlines this in the window head. Web loads it through `asset!` so
// `dx bundle` copies the file; compiling the sheet into wasm would ship it
// twice. Mobile has neither, so it injects this string from AppHead.
#[cfg(not(feature = "web"))]
pub(crate) const STYLESHEET: &str = include_str!("../assets/styles.css");

/// How long the settings have to sit still on their screen before they are
/// stored: a slider being dragged changes them every frame.
pub(crate) const SETTINGS_SAVE_DELAY_MS: u32 = 250;

/// The type faces, from Google Fonts. Every build links the same sheet.
pub(crate) const FONT_CSS_URL: &str = "https://fonts.googleapis.com/css2?family=Fraunces:opsz,wght@9..144,600;700&family=Figtree:wght@400;500;600;700&family=IBM+Plex+Mono:wght@500;700&display=optional";

// The web stylesheet, written into the page's `<head>` by `dx bundle`.
//
// `dx bundle` does not copy Dioxus.toml [web.resource] style files into the
// site — Pages then 404s `assets/styles.css` and the UI renders raw — so the
// sheet has to go through `asset!`. Left at that, though, the `<link>` only
// went in when the app mounted: the app drew its first frame unstyled and then
// snapped into place once the sheet arrived. A static-head asset is linked from
// the head of `index.html` itself, which the browser loads before it paints
// anything, so the boot screen holds until the app can appear already dressed.
// Nothing has to insert it at run time.
#[cfg(feature = "web")]
const _: Asset = asset!(
    "/assets/styles.css",
    AssetOptions::css().with_static_head(true)
);

fn injected_stylesheet() -> Element {
    // Desktop has the sheet inlined in its window head, and the web in the
    // page head; only mobile needs it put in from here.
    #[cfg(any(feature = "desktop", feature = "web"))]
    {
        rsx! {}
    }
    #[cfg(not(any(feature = "desktop", feature = "web")))]
    {
        rsx! {
            document::Style { { STYLESHEET } }
        }
    }
}

fn font_link() -> Element {
    #[cfg(feature = "desktop")]
    {
        rsx! {}
    }
    #[cfg(not(feature = "desktop"))]
    {
        rsx! {
            document::Link { rel: "stylesheet", href: FONT_CSS_URL }
        }
    }
}

/// Head tags are injected once, on mount. They live in their own component so
/// that App re-renders do not re-run them — `dioxus-document` warns on every
/// prop update of a head element ("Changing the props of `Style {}` is not
/// supported"), and a component with no props is memoized.
#[component]
fn AppHead() -> Element {
    rsx! {
        document::Title { "Dust" }
        {injected_stylesheet()}
        {font_link()}
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1, viewport-fit=cover",
        }
        document::Meta { name: "theme-color", content: "#1b2436" }
        document::Meta { name: "mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-status-bar-style", content: "black-translucent" }
    }
}

#[component]
pub fn App() -> Element {
    // The audio backend is injectable through context so a test can drive the
    // whole app with a player that records instead of one that needs a device.
    let ctx = use_context_provider(|| {
        let app = try_consume_context::<AppState>().unwrap_or_else(AppState::new);
        let (settings, sessions) = (app.store.load_settings(), app.store.load_sessions());
        AppCtx::new(app, settings, sessions)
    });
    let AppCtx {
        screen,
        settings,
        toasts,
        ..
    } = ctx;
    let mut theme = use_signal(|| Theme::from_key(&ctx.app().store.load_theme()));
    let paddle = use_paddle_keys(settings, screen, Rc::new(ctx.app()));
    use_stored_settings(ctx);
    use_keep_awake(screen, ctx.contest, ctx.preview);
    // Each screen opens at its top, wherever the one before it was scrolled
    // to: a run started from the foot of its results would otherwise begin
    // with its clock and its fields out of sight.
    use_effect(move || {
        let _ = screen();
        scroll_to_top();
    });
    #[cfg(feature = "desktop")]
    crate::ui::floating::use_floating_window(ctx);

    use_effect(move || {
        if let Some(shown) = toasts.current() {
            spawn(async move {
                sleep_ms(shown.lasts_ms()).await;
                toasts.dismiss(&shown);
            });
        }
    });

    let screen_now = screen();
    let show_nav = screen_now != Screen::Training;
    let shell_class = if show_nav { "shell has-nav" } else { "shell" };
    let screen_key = screen_now.key();
    let theme_attr = theme().key();

    rsx! {
        AppHead {}
        div {
            class: "app-root",
            "data-theme": theme_attr,
            onkeydown: move |e| {
                if e.key() == Key::F11 {
                    e.prevent_default();
                    toggle_fullscreen();
                    return;
                }
                paddle.keydown(e);
            },
            onkeyup: move |e| paddle.keyup(e),
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
                        if show_nav {
                            TourButton {}
                        }
                        button {
                            id: control_id("btn", "theme"),
                            class: "icon-btn",
                            title: "{theme().label()}",
                            aria_label: "{theme().label()}",
                            // On the training screen a click on it leaves
                            // the cursor in the box being typed in, so that
                            // Enter goes on sending or answering rather than
                            // pressing this again.
                            onmousedown: move |e| {
                                if screen_now == Screen::Training {
                                    keep_focus(e);
                                }
                            },
                            // Theme is a data-theme attribute on the app root,
                            // not a JS write to <html>. Both the desktop
                            // webview and the web build key CSS off that.
                            onclick: move |_| {
                                let next = theme().next();
                                theme.set(next);
                                ctx.app().store.save_theme(next.key());
                                // A run's fields take the cursor back, as
                                // after any of the run's own buttons.
                                if screen_now == Screen::Training && ctx.contest.peek().is_some() {
                                    focus_field(ctx, ctx.entry.focus());
                                }
                            },
                            Icon { name: theme().icon() }
                        }
                        if screen_now == Screen::Training {
                            button {
                                id: control_id("btn", "exit"),
                                class: "btn btn-secondary btn-sm",
                                // End session, or End run, from the header:
                                // the machine that is live decides what is
                                // worth keeping, the same way whichever
                                // button asked.
                                onclick: move |_| finish_run(ctx),
                                "Exit"
                            }
                        }
                    }
                }
                div { class: "screen", id: "screen-{screen_key}",
                    Route { screen: screen_now }
                }
            }
            if show_nav {
                div { class: "bottom-dock",
                    nav { class: "bottom-nav",
                        button {
                            id: control_id("nav", "practice"),
                            class: if matches!(screen_now, Screen::Home | Screen::Listen | Screen::Results) { "nav-item active" } else { "nav-item" },
                            onclick: move |_| ctx.go_to(Screen::Home),
                            Icon { name: IconName::Signal }
                            span { "Practice" }
                        }
                        button {
                            id: control_id("nav", "stats"),
                            class: if screen_now == Screen::Stats { "nav-item active" } else { "nav-item" },
                            onclick: move |_| ctx.go_to(Screen::Stats),
                            Icon { name: IconName::Chart }
                            span { "Stats" }
                        }
                        button {
                            id: control_id("nav", "settings"),
                            class: if screen_now == Screen::Settings { "nav-item active" } else { "nav-item" },
                            onclick: move |_| ctx.go_to(Screen::Settings),
                            Icon { name: IconName::Sliders }
                            span { "Settings" }
                        }
                    }
                }
            }
            TourDock {}
            RunAnnouncer {}
            if let Some(toast) = toasts.current() {
                div { class: if show_nav { "toast-dock has-nav" } else { "toast-dock" },
                    div { class: "toast", role: "status",
                        span { "{toast.text}" }
                        if let Some(action) = toast.action {
                            button {
                                id: control_id("toast", action.label()),
                                class: "toast-action",
                                onclick: move |_| {
                                    match action {
                                        ToastAction::UndoDelete => ctx.undo_delete(),
                                        ToastAction::UndoRestore => ctx.undo_restore(),
                                    }
                                    toasts.dismiss(&toast);
                                },
                                "{action.label()}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Keep the stored settings in step with the live ones.
///
/// What is stored is always clamped. On the settings screen the live values
/// are left as typed, so a field part-way through a number is not snapped
/// under the cursor, and they are stored once they have been still for a
/// moment. Anywhere else they are clamped in place and stored at once, which
/// is also what stores an edit the moment its screen is left. Settings that
/// are stored already are not stored again: moving between screens writes
/// nothing.
fn use_stored_settings(ctx: AppCtx) {
    let edits = use_hook(|| Rc::new(Cell::new(0u64)));
    use_effect(move || {
        let AppCtx {
            screen,
            mut settings,
            ..
        } = ctx;
        let screen = screen();
        if screen == Screen::Training {
            return;
        }
        let persist = settings.read().clone().clamp();
        if screen != Screen::Settings && persist != *settings.peek() {
            settings.set(persist);
        }
        // Whatever edit was still waiting, this one replaces it.
        let edit = edits.get() + 1;
        edits.set(edit);
        if screen == Screen::Settings {
            let edits = edits.clone();
            spawn(async move {
                sleep_ms(SETTINGS_SAVE_DELAY_MS).await;
                if edits.get() == edit {
                    ctx.store_settings();
                }
            });
        } else {
            ctx.store_settings();
        }
    });
}

/// Scroll the page back to its top.
fn scroll_to_top() {
    // Tests have no page to run it on.
    #[cfg(not(test))]
    spawn(async {
        let _ = document::eval("window.scrollTo(0, 0);").await;
    });
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
mod ui_tests {
    use crate::persist::Store;
    use crate::testing::{Ui, run, stored_session, test_settings};
    use cw_core::{CharSetMode, PracticeWindow, TrainingSettings};
    use dioxus::prelude::{Code, Key, ReadableExt};

    fn short_session() -> TrainingSettings {
        let mut settings = test_settings();
        settings.curriculum.num_groups = 1;
        settings
    }

    /// Whether the first group's box shows `text`.
    fn box_shows(ui: &Ui, text: &str) -> bool {
        ui.input_tag("group-input-0")
            .contains(&format!("value=\"{text}\""))
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
                assert_eq!(ui.store.load_theme(), key);
            }
        });
    }

    /// On the training screen the theme button never takes the cursor from
    /// the box being typed in, so Enter goes on reaching the session or the
    /// run, and a run's fields take it back after a press. Anywhere else it
    /// is an ordinary button.
    #[test]
    fn the_theme_button_leaves_the_cursor_where_it_is_in_training() {
        run(|| async {
            let (mut ui, _recorder) = awaiting_answer(short_session()).await;
            assert!(!ui.mouse_down("btn-theme"), "it took a session's cursor");
            ui.take_focused();
            ui.click("btn-theme");
            assert_eq!(ui.take_focused(), Vec::<String>::new());

            let (mut ui, recorder) = Ui::app_with_settings(crate::testing::contest_settings());
            assert!(
                ui.mouse_down("btn-theme"),
                "on Home it is an ordinary button"
            );
            ui.click("btn-start-training");
            ui.advance(8_000).await;
            let call = recorder.starts()[0].transcript.others[0].text.clone();
            ui.type_into("field-call", &call);
            assert!(!ui.mouse_down("btn-theme"), "it took the run's cursor");
            ui.take_focused();
            ui.click("btn-theme");
            assert!(ui.has("Theme: light"));
            assert_eq!(ui.take_focused(), ["field-call"], "the cursor stayed away");
            ui.press_enter("field-call");
            let sent = recorder
                .starts()
                .last()
                .map(|start| start.transcript.text.clone());
            assert!(
                sent.as_deref()
                    .is_some_and(|sent| sent.starts_with(&format!("{call} 5NN"))),
                "{sent:?}"
            );
        });
    }

    /// Settings are written when they change, and no more often than that:
    /// they used to be rewritten on every move between screens, and on
    /// every frame of a slider being dragged.
    #[test]
    fn settings_are_stored_once_per_change() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            let saves = |ui: &Ui| ui.store.settings_saves.get();
            for nav in ["nav-stats", "nav-settings", "nav-practice", "nav-settings"] {
                ui.click(nav);
            }
            ui.open_disclosures();
            ui.advance(1_000).await;
            assert_eq!(saves(&ui), 0, "nothing changed, and something was written");

            // A drag across a slider is one change, stored once it stops.
            for step in 1..=20 {
                ui.type_into("slider-rise-time", &format!("{}", 10 + step));
                ui.advance(16).await;
            }
            assert_eq!(saves(&ui), 0, "stored while still moving");
            assert_eq!(ui.stored_settings().await.band.steepness, 30.0);
            assert_eq!(saves(&ui), 1);

            // Leaving the screen stores an edit at once.
            ui.type_into("slider-rise-time", "12");
            ui.click("nav-practice");
            assert_eq!(saves(&ui), 2);
            assert_eq!(ui.store.load_settings().band.steepness, 12.0);
            ui.advance(1_000).await;
            assert_eq!(saves(&ui), 2, "the waiting edit was stored twice");
        });
    }

    #[test]
    fn a_session_can_be_played_from_the_practice_screen() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(short_session());
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert!(ui.has("Listen — the box opens when it ends."));
            // The nav is out of the way while a session runs.
            assert!(!ui.has("bottom-nav"));
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);

            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(ui.run_until(5_000, |ui| ui.screen() == "results").await);
            assert!(ui.has("Session complete"));
            assert!(ui.has("Clean copy"));
            assert_eq!(ui.store.load_sessions().len(), 1);
        });
    }

    #[test]
    fn a_group_locks_while_it_plays_then_unlocks() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            settings.playback.lock_input_during_group_playback = true;
            let (mut ui, recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert!(ui.has("Listen — the box opens when it ends."));
            // Locked, but neither read-only nor disabled: either would take
            // the focus away, and a phone's keyboard with it.
            let sending = ui.input_tag("group-input-0");
            assert!(sending.contains("answer locked"), "{sending}");
            assert!(!sending.contains("readonly"), "{sending}");
            assert!(!sending.contains("disabled"), "{sending}");
            ui.type_into("group-input-0", "XX");
            assert!(!ui.has("committed"), "{}", ui.html());

            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
            let open = ui.input_tag("group-input-0");
            assert!(!open.contains("locked"), "{open}");
            assert!(!open.contains("readonly"), "{open}");
            assert!(!open.contains("disabled"), "{open}");

            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(
                ui.run_until(5_000, |ui| ui.shows("group-input-1")
                    && ui.input_tag("group-input-1").contains("answer locked"))
                    .await,
                "the next group should lock while it plays: {}",
                ui.html()
            );
            assert!(
                ui.run_until(8_000, |ui| {
                    let tag = ui.input_tag("group-input-1");
                    !tag.contains("locked") && !tag.contains("disabled")
                })
                .await,
                "the next group's box should unlock: {}",
                ui.html()
            );
            // The finished group has no box any more: it sits above the open
            // one, with how it went.
            assert!(!ui.shows("group-input-0"), "{}", ui.html());
            assert!(ui.input_tag("group-card-0").contains("group done"));
        });
    }

    #[test]
    fn the_answer_box_stays_open_when_lock_while_sending_is_off() {
        run(|| async {
            let mut settings = short_session();
            settings.playback.lock_input_during_group_playback = false;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            assert!(ui.has("Listen, or type along"));
            let tag = ui.input_tag("group-input-0");
            assert!(!tag.contains("readonly"), "{tag}");
            assert!(!tag.contains("disabled"), "{tag}");
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
                assert!(ui.store.load_sessions().is_empty());
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
            assert_eq!(ui.store.load_sessions().len(), 1);
            assert!(ui.has("Group by group"));
        });
    }

    /// Exit and End session leave a session the same way. Exit used to
    /// decide for itself, from whether anything was confirmed yet, and threw
    /// away an answer End session would have kept.
    ///
    /// Clicked on the heels of the last key, too: the letter is still in the
    /// box's pause on its way to the session, and is kept all the same.
    #[test]
    fn exit_keeps_a_typed_answer_just_as_end_session_does() {
        run(|| async {
            for button in ["btn-exit", "btn-end-session"] {
                let mut settings = test_settings();
                settings.curriculum.num_groups = 3;
                let (mut ui, recorder) = awaiting_answer(settings).await;
                let sent = recorder.texts().first().cloned().expect("a group was sent");
                ui.type_into("group-input-0", &sent[..1]);
                ui.click(button);
                assert!(
                    ui.run_until(2_000, |ui| ui.screen() == "results").await,
                    "{button} threw the answer away"
                );
                let stored = ui.store.load_sessions();
                assert_eq!(stored.len(), 1, "{button}");
                assert_eq!(stored[0].groups[0].received, sent[..1], "{button}");
                // The box's pause was still running when the session ended,
                // and it ends with the box: running it out on the results
                // screen must find nothing to do.
                ui.advance(200).await;
                assert_eq!(ui.screen(), "results", "{button}");
            }
        });
    }

    /// A pause in an answer is time to think, not a character skipped: the
    /// answer window leaves an answer under way open — even one whose first
    /// key was still in the box's pause on its way to the session when the
    /// window ran out — and the rest of it, typed long after, is the answer.
    #[test]
    fn the_answer_window_waits_for_an_answer_under_way() {
        run(|| async {
            let mut settings = short_session();
            settings.playback.group_timeout = 1.0;
            let (mut ui, recorder) = awaiting_answer(settings).await;
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            // The window opened at most one poll ago. This key lands inside
            // its last 50 ms, nearer its end than the box's pause is long.
            ui.advance(970).await;
            assert_eq!(ui.screen(), "training");
            ui.type_into("group-input-0", &sent[..1]);
            // Ten windows' worth of thinking.
            ui.advance(10_000).await;
            assert_eq!(ui.screen(), "training", "the window cut the answer off");
            assert!(ui.has("Your turn"));
            ui.type_into("group-input-0", &sent);
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            let stored = ui.store.load_sessions();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].groups[0].received, sent);
        });
    }

    /// A letter keyed on the paddles as the window runs out begins the
    /// answer, even with its letter space still running: the window waits
    /// for the letter, and then for the rest.
    #[test]
    fn a_paddle_letter_keyed_before_the_window_closes_is_kept() {
        run(|| async {
            let mut settings = short_session();
            settings.playback.group_timeout = 1.0;
            settings.playback.keyer_mode = cw_core::KeyerMode::IambicA;
            settings.playback.keyer_wpm = 20.0;
            let (mut ui, _recorder) = awaiting_answer(settings).await;
            // The window opened at most one poll ago, and closes a second
            // after. A dah, 180 ms at 20 WPM, keyed to end 100 ms or so
            // before it does.
            ui.advance(700).await;
            let dah = Key::Character("]".into());
            ui.press_key("group-input-0", dah.clone(), Code::BracketRight);
            ui.advance(100).await;
            ui.release_key("group-input-0", dah, Code::BracketRight);
            ui.advance(150).await;
            assert_eq!(ui.screen(), "training");
            // The window runs out here. The keyer alone ends the letter a
            // dit of space and a letter space after the dah, 60 ms or more
            // from now, and the window waits for it.
            ui.advance(60).await;
            assert_eq!(ui.screen(), "training", "the window cut the dah off");
            ui.advance(2_000).await;
            assert_eq!(ui.screen(), "training", "the answer was not waited for");
            assert!(box_shows(&ui, "T"), "{}", ui.input_tag("group-input-0"));
            ui.press_enter("group-input-0");
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            let stored = ui.store.load_sessions();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].groups[0].received, "T");
        });
    }

    /// Auto-confirm waits for a letter still being keyed, as it would for a
    /// key still being typed: it does not cut the letter in two, and the
    /// letter, once in, calls the confirm off.
    #[test]
    fn auto_confirm_does_not_cut_a_paddle_letter_in_two() {
        run(|| async {
            let mut settings = short_session();
            settings.playback.keyer_mode = cw_core::KeyerMode::IambicA;
            settings.playback.keyer_wpm = 20.0;
            let (mut ui, _recorder) = awaiting_answer(settings).await;
            let (dit, dah) = (Key::Character("[".into()), Key::Character("]".into()));
            let key = |ui: &mut Ui, paddle: &Key, code: Code, down: bool| {
                if down {
                    ui.press_key("group-input-0", paddle.clone(), code);
                } else {
                    ui.release_key("group-input-0", paddle.clone(), code);
                }
            };
            // E, and E again: the group's full length at 500 ms, and
            // auto-confirm due 300 ms after.
            key(&mut ui, &dit, Code::BracketLeft, true);
            ui.advance(32).await;
            key(&mut ui, &dit, Code::BracketLeft, false);
            ui.advance(228).await;
            key(&mut ui, &dit, Code::BracketLeft, true);
            ui.advance(32).await;
            key(&mut ui, &dit, Code::BracketLeft, false);
            // An A: its dit at 720, its dah closed in the space after it,
            // at 820 — the auto-confirm falls between the two.
            ui.advance(428).await;
            key(&mut ui, &dit, Code::BracketLeft, true);
            ui.advance(32).await;
            key(&mut ui, &dit, Code::BracketLeft, false);
            ui.advance(68).await;
            key(&mut ui, &dah, Code::BracketRight, true);
            ui.advance(80).await;
            key(&mut ui, &dah, Code::BracketRight, false);
            ui.advance(600).await;
            assert_eq!(
                ui.screen(),
                "training",
                "auto-confirm went ahead mid-letter"
            );
            ui.press_enter("group-input-0");
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            assert_eq!(ui.store.load_sessions()[0].groups[0].received, "EEA");
        });
    }

    /// Enter on the heels of a key confirms what the box shows, not what the
    /// session had been told before the key.
    #[test]
    fn enter_right_after_a_key_confirms_the_whole_answer() {
        run(|| async {
            let (mut ui, recorder) = awaiting_answer(short_session()).await;
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent[..1]);
            ui.press_enter("group-input-0");
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            let stored = ui.store.load_sessions();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].groups[0].received, sent[..1]);
            ui.advance(200).await;
            assert_eq!(ui.screen(), "results");
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
    fn the_listen_screen_can_stream_groups_without_storing_anything() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            assert!(ui.has("Stream groups"));
            ui.click("btn-stream-groups");
            assert!(ui.has("Stop"));
            assert!(ui.has("Nothing is scored"));
            assert!(ui.run_until(20_000, |_| recorder.texts().len() >= 2).await);
            assert!(recorder.texts().iter().all(|text| text.len() == 2));
            assert!(ui.run_until(20_000, |ui| ui.has("listen-heard")).await);
            // Every group says how fast it was sent.
            assert!(ui.has("listen-speed") && ui.has("40 WPM"), "{}", ui.html());
            // Only the desktop can float the groups over other programs.
            assert_eq!(ui.has("btn-floating-window"), cfg!(feature = "desktop"));
            ui.click("btn-stop");
            assert!(ui.has("Play all"));
            assert!(ui.has("Stream groups"));
            assert_eq!(ui.screen(), "listen");
            assert!(ui.store.load_sessions().is_empty());
        });
    }

    /// Listen treats a contest as Callsigns: its pool is the calls'
    /// characters, and it streams calls, never storing a run.
    #[test]
    fn the_listen_screen_streams_calls_in_contest_mode() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(crate::testing::contest_settings());
            ui.click("btn-listen-to-letters");
            assert!(!ui.has("Newest unlocked character"));
            assert!(ui.shows("chip-k") && ui.shows("chip-1"), "{}", ui.html());
            ui.click("btn-stream-groups");
            assert!(ui.run_until(20_000, |_| recorder.texts().len() >= 2).await);
            for text in recorder.texts() {
                assert!(
                    cw_core::parse_callsign(&text).is_some(),
                    "{text} is no call"
                );
            }
            ui.click("btn-stop");
            assert!(ui.store.load_sessions().is_empty());
        });
    }

    /// The QSO the page shows, line by line: who sent each, and what, the
    /// sends of callers calling at once side by side.
    fn qso_lines(ui: &Ui) -> Vec<String> {
        let html = ui.html();
        html.split("<li class=\"qso-line ")
            .skip(1)
            .map(|line| {
                let line = line.split("</li>").next().unwrap_or_default();
                let who = if line.starts_with("you") {
                    "You"
                } else {
                    "They"
                };
                let sends: Vec<&str> = line
                    .split("<span class=\"call\">")
                    .skip(1)
                    .filter_map(|rest| rest.split("</span>").next())
                    .collect();
                format!("{who} {}", sends.join(" "))
            })
            .collect()
    }

    /// How far into its turn the last tone of a send's `plan` ends, in
    /// milliseconds.
    fn tones_end_ms(plan: &cw_core::PlaybackPlan) -> u64 {
        let last = plan.events.last().expect("a send has tones");
        ((last.start_sec + last.duration_sec) * 1_000.0) as u64
    }

    /// Hear a QSO, from Listen in Contest mode: a few QSOs played turn by
    /// turn as a run plays them, your message dry and the callers through
    /// the receiver, each line shown once its send has been heard whole and
    /// not before, to the TU of the third. The lines stay once it ends, and
    /// nothing is kept: no session, nothing stored.
    #[test]
    fn hear_a_qso_plays_a_few_qsos_and_shows_each_line_as_it_ends() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(crate::testing::contest_settings());
            ui.click("btn-listen-to-letters");
            assert!(ui.has("Hear a few QSOs of CQ WPX"), "{}", ui.html());
            ui.click("btn-hear-a-qso");
            assert!(ui.has(">QSO 1 of 3"));
            assert!(ui.has("Listening — each line shows once it has been sent."));
            assert_eq!(recorder.starts().len(), 1, "the first CQ at once");
            let first = recorder.starts()[0].clone();
            assert!(first.plan.dry_wanted, "your message dry, as in a run");
            assert_eq!(first.qsb, None, "the callers' fading is in their tones");
            assert_eq!(first.transcript.text, "CQ DL1ABC TEST");
            let call = first.transcript.others[0].text.clone();
            // The CQ shows once it has gone out, and not while it sounds;
            // the answer likewise. The fake clock moves in steps of 4 ms.
            let mut now = 0;
            for (ends, shown) in [
                (tones_end_ms(&first.plan.wanted), 1),
                (tones_end_ms(&first.plan.others[0]), 2),
            ] {
                let almost = (ends - 40).div_ceil(4) * 4;
                ui.advance(almost - now).await;
                now = almost;
                assert_eq!(qso_lines(&ui).len(), shown - 1, "{:?}", qso_lines(&ui));
                while qso_lines(&ui).len() < shown && now < ends + 1_000 {
                    ui.advance(4).await;
                    now += 4;
                }
                assert_eq!(qso_lines(&ui).len(), shown, "{:?}", qso_lines(&ui));
            }
            assert_eq!(
                qso_lines(&ui),
                ["You CQ DL1ABC TEST".to_string(), format!("They {call}")]
            );
            assert!(ui.run_until(240_000, |ui| ui.has("QSOs heard")).await);
            // Every send of every turn, in order, and the third TU last.
            let sent: Vec<String> = recorder
                .starts()
                .iter()
                .flat_map(|start| {
                    let yours = &start.transcript.text;
                    let yours = (!yours.is_empty()).then(|| format!("You {yours}"));
                    let theirs = start.transcript.others.iter();
                    yours
                        .into_iter()
                        .chain(theirs.map(|other| format!("They {}", other.text)))
                })
                .collect();
            let lines = qso_lines(&ui);
            assert_eq!(lines, sent);
            let tus = lines.iter().filter(|line| *line == "You TU").count();
            assert_eq!(tus, 3, "{lines:?}");
            assert_eq!(lines.last().map(String::as_str), Some("You TU"));
            // Each QSO after the first set apart where its caller comes in.
            let html = ui.html();
            assert_eq!(html.matches("qso-next").count(), 2, "{lines:?}");
            assert!(ui.has("btn-hear-a-qso"), "it can be heard again");
            assert!(ui.has("not scored, not saved to History"));
            assert_eq!(ui.screen(), "listen");
            assert!(ui.with_app(|ctx| ctx.sessions.peek().is_empty()));
            assert!(ui.store.load_sessions().is_empty());
            // A character picked puts the lines away.
            ui.click("chip-b");
            assert!(qso_lines(&ui).is_empty());
            assert!(ui.has("Play B"));
            ui.click("btn-hear-a-qso");
            assert!(ui.run_until(10_000, |ui| !qso_lines(ui).is_empty()).await);
            // Leaving the screen lets the lines go.
            ui.click("btn-listen-back");
            assert!(ui.with_app(|ctx| ctx.dialogue.peek().is_none()));
        });
    }

    /// Stop ends Hear a QSO where it is: nothing more plays, the lines
    /// heard so far stay, and another sound played puts them away. Stopped
    /// before a line was heard, it leaves the stage as it was.
    #[test]
    fn hear_a_qso_stops_on_stop() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(crate::testing::contest_settings());
            ui.click("btn-listen-to-letters");
            ui.click("btn-hear-a-qso");
            assert!(ui.run_until(10_000, |ui| qso_lines(ui).len() >= 2).await);
            ui.click("btn-stop");
            let played = recorder.starts().len();
            let shown = qso_lines(&ui);
            ui.advance(30_000).await;
            assert_eq!(recorder.starts().len(), played, "nothing more played");
            assert_eq!(qso_lines(&ui), shown);
            assert!(ui.has("QSOs heard"));
            assert!(ui.has("btn-hear-a-qso"));
            ui.click("btn-play");
            assert!(qso_lines(&ui).is_empty());
            assert!(ui.store.load_sessions().is_empty());

            ui.click("btn-stop");
            ui.click("btn-hear-a-qso");
            ui.advance(200).await;
            ui.click("btn-stop");
            assert!(!ui.has("Listening — each line"), "{}", ui.html());
            assert!(!ui.has("QSOs heard"));
            assert!(ui.has("listen-glyph"));
        });
    }

    /// Hear a QSO on the Character set card: the card's QSO gives way to
    /// the QSOs as they are heard, with Stop where Hear a QSO was, and
    /// Stop stops them. Stopped before a line, the card's QSO comes back.
    #[test]
    fn hear_a_qso_plays_from_the_character_set_card() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(crate::testing::contest_settings());
            ui.click("nav-settings");
            assert!(ui.has(">A QSO<"), "{}", ui.html());
            let example = qso_lines(&ui);
            assert_eq!(example[0], "You CQ DL1ABC TEST");
            ui.click("btn-hear-a-qso");
            assert!(ui.has(">QSO 1 of 3"));
            assert!(ui.run_until(10_000, |ui| qso_lines(ui).len() >= 2).await);
            assert!(recorder.starts().iter().all(|start| start.plan.dry_wanted));
            ui.click("btn-stop-hearing");
            assert!(ui.has("QSOs heard"));
            let played = recorder.starts().len();
            ui.advance(20_000).await;
            assert_eq!(recorder.starts().len(), played);
            assert!(ui.store.load_sessions().is_empty());

            ui.click("btn-hear-a-qso");
            ui.advance(200).await;
            ui.click("btn-stop-hearing");
            assert!(ui.has(">A QSO<"), "{}", ui.html());
            assert_eq!(qso_lines(&ui), example);
        });
    }

    /// Hear a QSO is of the contest it was started from: picking another
    /// contest, or another mode, while it plays stops it and puts its lines
    /// away, so the card never shows one contest while another sounds, nor
    /// leaves it playing with no Stop on the screen.
    #[test]
    fn hear_a_qso_stops_when_the_contest_or_the_mode_changes() {
        run(|| async {
            for pick in ["contest-simple-qso", "seg-koch"] {
                let (mut ui, recorder) = Ui::app_with_settings(crate::testing::contest_settings());
                ui.click("nav-settings");
                ui.click("btn-hear-a-qso");
                assert!(ui.run_until(10_000, |ui| qso_lines(ui).len() >= 2).await);
                ui.click(pick);
                assert!(
                    ui.with_app(|ctx| !ctx.preview.peek().is_qsos()),
                    "{pick}: stopped"
                );
                assert!(ui.with_app(|ctx| ctx.dialogue.peek().is_none()), "{pick}");
                let played = recorder.starts().len();
                ui.advance(20_000).await;
                assert_eq!(recorder.starts().len(), played, "{pick}");
                assert!(!ui.has("QSOs heard"), "{pick}");
            }
            // The first contest picked again shows its own QSO, not the
            // lines heard before.
            let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
            ui.click("nav-settings");
            ui.click("btn-hear-a-qso");
            assert!(ui.run_until(10_000, |ui| qso_lines(ui).len() >= 2).await);
            ui.click("contest-simple-qso");
            assert!(qso_lines(&ui)[0].starts_with("You CQ CQ DE DL1ABC"));
            ui.click("contest-cq-wpx");
            assert!(ui.has(">A QSO<"));
            assert_eq!(qso_lines(&ui)[0], "You CQ DL1ABC TEST");
        });
    }

    /// The press of Start that keeps what was typed into Home's hero, and
    /// starts the run with it, stores it then: the run stores no settings
    /// while it is on, so a reload mid-run used to lose it.
    #[test]
    fn what_start_keeps_from_the_hero_is_stored_as_the_run_starts() {
        run(|| async {
            let mut settings = crate::testing::contest_settings();
            settings.contest.my_call.clear();
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.type_into("field-your-callsign", "dl1abc");
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert_eq!(ui.store.load_settings().contest.my_call, "DL1ABC");

            // And a name, the last a Simple QSO asks for.
            let mut settings = crate::testing::contest_settings();
            settings.contest.contest_kind = cw_core::ContestKind::SimpleQso;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.type_into("field-your-name", "mat");
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert_eq!(ui.store.load_settings().contest.my_name, "MAT");
            let saves = ui.store.settings_saves.get();
            ui.advance(5_000).await;
            assert_eq!(ui.store.settings_saves.get(), saves, "nothing more mid-run");
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
                ("seg-koch", "Unlocked in LCWO order", CharSetMode::Koch),
                ("seg-digits", "Digits only", CharSetMode::Digits),
                (
                    "seg-mixed",
                    "Letters and digits together",
                    CharSetMode::Mixed,
                ),
            ] {
                ui.click(button);
                assert!(ui.has(note), "{button} should say {note}");
                assert_eq!(ui.stored_settings().await.curriculum.char_set_mode, mode);
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
            let stored = ui.stored_settings().await;
            assert_eq!(
                stored.sequence_choice(),
                cw_core::SequenceChoice::Preset(cw_core::PresetId::CwAcademy)
            );
            assert!(!stored.curriculum.sequence_is_custom);
            assert!(stored.progress_alphabet().starts_with(&['E', 'T', 'A']));
            ui.click("pill-lcwo");
            assert!(
                ui.stored_settings()
                    .await
                    .curriculum
                    .custom_sequence
                    .is_empty()
            );
        });
    }

    #[test]
    fn the_practice_window_can_be_narrowed_to_the_newest_characters() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 9;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            for (button, window, pool) in [
                ("pill-newest-3", PracticeWindow::Last3, 3),
                ("pill-newest-5", PracticeWindow::Last5, 5),
                ("pill-everything", PracticeWindow::All, 10),
            ] {
                ui.click(button);
                let stored = ui.stored_settings().await;
                assert_eq!(stored.curriculum.practice_window, window);
                assert_eq!(cw_core::compute_char_pool(&stored).len(), pool);
            }
        });
    }

    /// A sequence is shortened on its way to being retyped, a character at
    /// a time. The level it carries is read inside it, and kept for when it
    /// grows back.
    #[test]
    fn shortening_the_alphabet_keeps_the_level_for_when_it_grows_back() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 9;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            ui.commit("field-sequence-order", "KM");
            ui.advance(50).await;
            let stored = ui.stored_settings().await;
            assert_eq!(stored.curriculum.custom_sequence, vec!['K', 'M']);
            // Two characters can only carry level 1.
            assert_eq!(stored.active_level(), 1);
            assert_eq!(stored.clone().clamp(), stored);

            ui.commit("field-sequence-order", "KMURESNAPTL");
            ui.advance(50).await;
            assert_eq!(ui.stored_settings().await.active_level(), 9);
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

    /// The band preview, the letter player and an envelope sample all want the
    /// same audio, so starting one has to stop whatever else was sounding.
    ///
    /// That used to be three separate flags reset by hand at every call site,
    /// eight times over and already not the same twice. It is one value now,
    /// which cannot hold two of them at once — this is that promise, checked
    /// from the outside.
    #[test]
    fn starting_one_preview_stops_whatever_else_was_playing() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.open_disclosures();

            // The band preview is running and says so.
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            assert!(ui.has("Stop"), "the preview should be showing as live");

            // A sample chip takes the audio, and the preview gives it up.
            ui.click("chip-dit");
            ui.advance(50).await;
            assert!(
                ui.has("Live preview"),
                "the band preview should have stopped when the sample started"
            );

            // And the sample finishes on its own, leaving nothing playing.
            assert!(ui.run_until(20_000, |ui| ui.has("Live preview")).await);
            let settled = recorder.texts().len();
            ui.advance(3_000).await;
            assert_eq!(recorder.texts().len(), settled, "nothing should still loop");
        });
    }

    /// Dragging a band slider during the preview moves the band a few times
    /// a second, not on every frame: each move rebuilds the band and
    /// measures its AGC, on the thread the slider is drawn on.
    #[test]
    fn the_band_follows_a_dragged_slider_without_rebuilding_every_frame() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(with_receiver());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            let rebuilds = || {
                recorder
                    .calls()
                    .iter()
                    .filter(|call| matches!(call, crate::audio::fake::Call::Band(_)))
                    .count()
            };
            let before = rebuilds();
            for step in 0..20 {
                ui.type_into("slider-filter-width", &format!("{}", 300 + 10 * step));
                ui.advance(16).await;
            }
            ui.advance(200).await;
            let followed = rebuilds() - before;
            assert!(
                (1..=6).contains(&followed),
                "{followed} rebuilds for one drag"
            );
            // And it ends on where the slider was left.
            let last = recorder
                .calls()
                .into_iter()
                .rev()
                .find_map(|call| match call {
                    crate::audio::fake::Call::Band(key) => Some(key),
                    _ => None,
                });
            let mut expected = with_receiver();
            expected.band.filter_width_hz = 490.0;
            assert_eq!(
                last,
                Some(cw_core::band::BandKey::of(&expected.clamp().band))
            );
        });
    }

    /// Every band the player has been asked to bring up, oldest first.
    fn bands(recorder: &crate::audio::fake::Recorder) -> Vec<cw_core::band::BandKey> {
        recorder
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                crate::audio::fake::Call::Band(key) => Some(key),
                _ => None,
            })
            .collect()
    }

    /// Start the band preview on the settings screen, and wait for its first
    /// call. The call takes most of a second, so for a while after this only
    /// a follow brings the band up.
    async fn previewing_the_band() -> (Ui, std::rc::Rc<crate::audio::fake::Recorder>) {
        let (mut ui, recorder) = Ui::app_with_settings(with_receiver());
        ui.click("nav-settings");
        ui.click("btn-band-preview");
        assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
        (ui, recorder)
    }

    /// A follow measures the band's AGC before it takes the settings, so a
    /// stop can land in between. The band must not come back up behind it.
    #[test]
    fn a_preview_stopped_while_its_band_follows_stays_silent() {
        run(|| async {
            let (mut ui, recorder) = previewing_the_band().await;
            let before = bands(&recorder).len();
            ui.type_into("slider-filter-width", "400");
            // The step that wakes the follow is the last one: it measures
            // the knee then, and lets the page have its turn before it takes
            // the settings, which is where this leaves it.
            ui.advance(u64::from(crate::routes::BAND_FOLLOW_MS)).await;
            assert_eq!(
                bands(&recorder).len(),
                before,
                "the follow should still be measuring"
            );

            ui.click("btn-band-stop");
            ui.advance(200).await;
            assert_eq!(
                bands(&recorder).len(),
                before,
                "the band followed a stopped preview"
            );
            assert!(
                !recorder.band_running.get(),
                "the stopped preview's receiver came back"
            );
        });
    }

    /// A move made while a follow measures the band's AGC joins the settings
    /// that follow is about to take: it is neither left behind nor sent
    /// after them in a second rebuild, and the band ends where the slider
    /// was left.
    #[test]
    fn a_move_made_while_the_band_follows_still_lands() {
        run(|| async {
            let (mut ui, recorder) = previewing_the_band().await;
            let before = bands(&recorder).len();
            ui.type_into("slider-filter-width", "400");
            ui.advance(u64::from(crate::routes::BAND_FOLLOW_MS)).await;
            assert_eq!(
                bands(&recorder).len(),
                before,
                "the follow should still be measuring"
            );

            ui.type_into("slider-filter-width", "600");
            ui.advance(200).await;
            let mut expected = with_receiver();
            expected.band.filter_width_hz = 600.0;
            assert_eq!(
                bands(&recorder)[before..],
                [cw_core::band::BandKey::of(&expected.clamp().band)],
                "the band should have followed the newest move, once"
            );
            assert!(recorder.band_running.get());
        });
    }

    /// The band the preview's settings bring up with the receiver this wide.
    fn band_with_width(width_hz: f64) -> cw_core::band::BandKey {
        let mut settings = with_receiver();
        settings.band.filter_width_hz = width_hz;
        cw_core::band::BandKey::of(&settings.clamp().band)
    }

    /// A move made while a follow measures the band's AGC asks about a band
    /// of its own, so the follow measures that one too before it takes it:
    /// building the band then finds its knee remembered. The follow waits
    /// for the second measurement, and still brings the band up once, on the
    /// newest move.
    #[test]
    fn a_move_made_while_the_band_follows_is_measured_before_it_lands() {
        run(|| async {
            let (mut ui, recorder) = previewing_the_band().await;
            let before = bands(&recorder).len();
            ui.type_into("slider-filter-width", "400");
            ui.advance(u64::from(crate::routes::BAND_FOLLOW_MS)).await;
            ui.type_into("slider-filter-width", "600");
            // The step that ends the first measurement starts the second,
            // which this leaves unfinished; the next one ends it.
            ui.advance(1).await;
            assert_eq!(
                bands(&recorder).len(),
                before,
                "the follow took the newest move without measuring it"
            );
            ui.advance(1).await;
            assert_eq!(
                bands(&recorder)[before..],
                [band_with_width(600.0)],
                "the follow should take the newest move once it is measured"
            );

            ui.advance(200).await;
            assert_eq!(
                bands(&recorder)[before..],
                [band_with_width(600.0)],
                "the band should have followed the newest move, once"
            );
        });
    }

    /// A slider that moves through every measurement is measured again only
    /// so often: then the follow takes the newest move as it stands, and the
    /// band measures that knee itself, rather than the band never following.
    #[test]
    fn a_slider_moved_through_every_measurement_is_still_followed() {
        run(|| async {
            let (mut ui, recorder) = previewing_the_band().await;
            let before = bands(&recorder).len();
            let mut width_hz = 400.0;
            ui.type_into("slider-filter-width", &format!("{width_hz}"));
            ui.advance(u64::from(crate::routes::BAND_FOLLOW_MS)).await;
            for round in 1..=crate::routes::BAND_WARM_ROUNDS {
                assert_eq!(
                    bands(&recorder).len(),
                    before,
                    "the follow took a move before measurement {round} was done"
                );
                // Each step ends a measurement with a newer move waiting.
                width_hz += 10.0;
                ui.type_into("slider-filter-width", &format!("{width_hz}"));
                ui.advance(1).await;
            }
            assert_eq!(
                bands(&recorder)[before..],
                [band_with_width(width_hz)],
                "the follow should have taken the newest move after its last measurement"
            );
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

    /// The pile-up controls only exist once more than one station can call, so
    /// a sweep that never turns the count up never touches them — which is how
    /// a card of sliders goes untested.
    /// Each card can be put back on its own, without taking the rest of the
    /// screen with it.
    #[test]
    fn a_section_resets_itself_and_leaves_the_others_alone() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.open_disclosures();

            // Move something on two different cards.
            ui.type_into("slider-qrn-intensity", "0.9");
            ui.advance(50).await;
            ui.commit("fixed-stations-calling", "4");
            ui.advance(50).await;
            ui.click("seg-filter-soft");
            ui.advance(50).await;
            ui.type_into("slider-rise-time", "23");
            ui.advance(50).await;

            let changed = ui.stored_settings().await;
            assert_eq!(changed.band.stations_max, 4);
            assert_eq!(changed.band.filter_shape, cw_core::FilterShape::Soft);
            assert!((changed.band.steepness - 23.0).abs() < 0.5);

            // Reset the keying envelope only.
            ui.click("reset-keying-envelope");
            ui.advance(50).await;
            let after = ui.stored_settings().await;
            let fresh = cw_core::TrainingSettings::default();
            assert!(
                (after.band.steepness - fresh.band.steepness).abs() < 0.001,
                "the rise time should be back to its default"
            );
            assert_eq!(
                after.band.stations_max, 4,
                "resetting the keying envelope must not touch the band conditions"
            );
            assert_eq!(after.band.filter_shape, cw_core::FilterShape::Soft);

            // And now the band conditions, which owns the stations.
            ui.click("reset-band-conditions");
            ui.advance(50).await;
            let after = ui.stored_settings().await;
            assert_eq!(after.band.stations_max, fresh.band.stations_max);
            assert!((after.band.qrn_level - fresh.band.qrn_level).abs() < 0.001);
        });
    }

    #[test]
    fn the_pile_up_controls_appear_with_the_stations_and_work() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            assert!(!ui.has("Pile-up spread"), "one station needs no spread");

            // One station is the pair held together, so the count is fixed.
            assert!(ui.has("Fixed"));
            ui.commit("fixed-stations-calling", "4");
            ui.advance(50).await;
            let stored = ui.stored_settings().await;
            assert_eq!((stored.band.stations_min, stored.band.stations_max), (4, 4));
            assert!(
                ui.has("Pile-up spread"),
                "the controls should have appeared"
            );

            // Opened up, it becomes a range with both ends editable.
            ui.click("link-stations-calling");
            ui.advance(50).await;
            ui.commit("from-stations-calling", "2");
            ui.advance(50).await;
            ui.commit("to-stations-calling", "5");
            ui.advance(50).await;
            let stored = ui.stored_settings().await;
            assert_eq!((stored.band.stations_min, stored.band.stations_max), (2, 5));
            assert!(ui.has("2–5 at once"), "the range should read back");

            for (slider, value, reads) in [
                ("slider-pile-up-spread", "260", "±260 Hz"),
                ("slider-pile-up-is-weaker-by", "20", "20 dB"),
            ] {
                ui.type_into(slider, value);
                ui.advance(50).await;
                assert!(
                    ui.has(&format!("class=\"slider-value\">{reads}<")),
                    "{slider} should read {reads}"
                );
            }
            let stored = ui.stored_settings().await;
            assert_eq!(stored.band.pileup_spread_hz, 260.0);
            assert_eq!(stored.band.pileup_level_db, 20.0);
            assert_eq!(stored.clone().clamp(), stored);

            // Back to one station, alone, and they go away again.
            ui.click("link-stations-calling");
            ui.advance(50).await;
            ui.commit("fixed-stations-calling", "1");
            ui.advance(50).await;
            let stored = ui.stored_settings().await;
            assert_eq!((stored.band.stations_min, stored.band.stations_max), (1, 1));
            assert_eq!(stored.band.filter_shape, test_settings().band.filter_shape);
            assert!(!ui.has("Pile-up spread"));
        });
    }

    /// Test settings run a dead-quiet band; these tests need one that is on.
    fn with_receiver() -> TrainingSettings {
        let mut settings = test_settings();
        settings.band.noise_enabled = true;
        settings.band.noise_level = 0.4;
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

    /// A band preview that gives up on a failing device has to take the
    /// receiver down with it. Its retries rebuild the player, which brings
    /// the band back up on the new one; stopping only the loop left that
    /// hissing with the Stop button already gone.
    #[test]
    fn a_preview_that_gives_up_silences_the_receiver() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(with_receiver());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            recorder.set(crate::audio::fake::Behaviour::RefuseToStart);
            assert!(ui.run_until(30_000, |ui| ui.has("class=\"toast\"")).await);
            assert!(ui.run_until(5_000, |ui| ui.has("Live preview")).await);
            assert!(
                !recorder.band_running.get(),
                "the receiver was left running with nothing to stop it"
            );
        });
    }

    /// Taking the audio stops whatever had it, whether or not the new owner
    /// then gets a player. A claim that failed used to leave the screen
    /// showing a band preview whose loop it had already stopped.
    #[test]
    fn a_claim_that_fails_leaves_no_preview_showing() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);

            // The device goes away mid-send, and with it any new player.
            recorder.build_error.set(true);
            recorder.fail_next(1);
            let mut waited = 0;
            while !recorder
                .calls()
                .contains(&crate::audio::fake::Call::Shutdown)
            {
                assert!(waited < 5_000, "the send was never retried");
                ui.advance(4).await;
                waited += 4;
            }
            // The retry has no player left; a sample asks for the audio now.
            ui.click("chip-dit");
            ui.advance(50).await;
            assert!(ui.has("No audio output device found"));
            assert!(
                ui.has("Live preview"),
                "the band preview still shows as running: {}",
                ui.html()
            );
        });
    }

    /// The same message twice is shown for its full time the second time
    /// too. The first one's timer used to clear the second a second in.
    #[test]
    fn a_toast_raised_again_stays_for_its_full_time() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            recorder.set(crate::audio::fake::Behaviour::RefuseToStart);
            ui.click("nav-settings");
            ui.click("chip-dit");
            assert!(ui.run_until(5_000, |ui| ui.has("class=\"toast\"")).await);
            ui.advance(3_000).await;
            ui.click("chip-dit");
            assert!(ui.run_until(5_000, |ui| !ui.has("test-chip playing")).await);
            assert!(ui.has("class=\"toast\""));
            ui.advance(1_500).await;
            assert!(
                ui.has("class=\"toast\""),
                "the second toast was cleared by the first one's timer"
            );
            assert!(ui.run_until(4_000, |ui| !ui.has("class=\"toast\"")).await);
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

    /// Without a call, Home asks for one in the hero, and a start says what
    /// is missing there rather than in a toast; with one, it starts the run.
    #[test]
    fn home_asks_for_your_call_before_a_contest() {
        run(|| async {
            let mut settings = crate::testing::contest_settings();
            settings.contest.my_call.clear();
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            assert!(ui.has("Start contest"));
            assert!(ui.shows("field-your-callsign"));
            assert!(!ui.shows("home-missing"));

            ui.click("btn-start-training");
            ui.advance(50).await;
            assert_eq!(ui.screen(), "home");
            assert!(ui.shows("home-missing"));
            assert!(ui.has("Add your callsign to start a contest"));
            assert!(!ui.has("class=\"toast\""), "said on Home, not in a toast");

            // A call that does not read is flagged under the field, and a
            // start does not say it again above Start.
            ui.commit("field-your-callsign", "abc");
            ui.advance(50).await;
            assert!(ui.has("Not a callsign yet: needs a digit"));
            ui.click("btn-start-training");
            ui.advance(50).await;
            assert_eq!(ui.screen(), "home");
            assert!(!ui.shows("home-missing"), "{}", ui.html());
            assert!(!ui.has("ABC is not a callsign Dust can read"));

            ui.commit("field-your-callsign", "k1abc");
            ui.advance(50).await;
            assert_eq!(ui.stored_settings().await.contest.my_call, "K1ABC");
            assert!(!ui.shows("field-your-callsign"));
            assert!(!ui.shows("home-missing"));
            assert!(ui.has("You send 5NN + serial, as K1ABC"));

            ui.click("btn-start-training");
            assert!(ui.run_until(5_000, |ui| ui.screen() == "training").await);
        });
    }

    /// After a refused start, the call typed into the hero is kept by one
    /// press of Start, which keeps the cursor in the field: leaving the box
    /// would keep the call on the press and redraw the hero, Start moved,
    /// under the finger before its click came. So is a name, and what is
    /// still missing is said as before, the cursor going to its field once
    /// the hero has drawn it.
    #[test]
    fn one_press_of_start_keeps_what_was_typed_and_starts() {
        run(|| async {
            let mut settings = crate::testing::contest_settings();
            settings.contest.my_call.clear();
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            assert!(ui.shows("home-missing"));
            assert_eq!(ui.take_focused(), ["field-your-callsign"]);
            ui.type_into("field-your-callsign", "dl1abc");
            assert!(
                !ui.mouse_down("btn-start-training"),
                "Start took the cursor"
            );
            assert!(ui.shows("home-missing") && ui.shows("field-your-callsign"));
            ui.click("btn-start-training");
            assert!(ui.run_until(5_000, |ui| ui.screen() == "training").await);
            let kept = ui.with_app(|ctx| ctx.settings.peek().contest.my_call.clone());
            assert_eq!(kept, "DL1ABC");

            // A Simple QSO with neither: the call is kept, and the start is
            // refused for the name, which is asked for next.
            let mut settings = crate::testing::contest_settings();
            settings.contest.contest_kind = cw_core::ContestKind::SimpleQso;
            settings.contest.my_call.clear();
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.type_into("field-your-callsign", "dl1abc");
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "home");
            assert!(ui.has("Add your name to start a Simple\u{a0}QSO"));
            assert_eq!(
                ui.take_focused(),
                ["field-your-name"],
                "the name's field, drawn by the same press, has no cursor"
            );
            ui.type_into("field-your-name", "mat");
            assert!(!ui.mouse_down("btn-start-training"));
            ui.click("btn-start-training");
            assert!(ui.run_until(5_000, |ui| ui.screen() == "training").await);
            let kept = ui.with_app(|ctx| ctx.settings.peek().contest.clone());
            assert_eq!(
                (kept.my_call.as_str(), kept.my_name.as_str()),
                ("DL1ABC", "MAT")
            );

            // A call typed that does not read is kept, and said under the
            // field rather than above Start. With nothing to type, Start is
            // an ordinary button.
            let mut settings = crate::testing::contest_settings();
            settings.contest.my_call.clear();
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.type_into("field-your-callsign", "abc");
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "home");
            assert!(ui.has("Not a callsign yet: needs a digit"));
            assert!(!ui.shows("home-missing"));
            assert_eq!(ui.take_focused(), ["field-your-callsign"]);
            let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
            assert!(ui.mouse_down("btn-start-training"));
        });
    }

    /// Listen, beside Start, keeps the cursor and what was typed the same
    /// way, so one press of it after a refused start keeps the call and
    /// opens Listen.
    #[test]
    fn one_press_of_listen_keeps_what_was_typed() {
        run(|| async {
            let mut settings = crate::testing::contest_settings();
            settings.contest.my_call.clear();
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            ui.type_into("field-your-callsign", "dl1abc");
            assert!(
                !ui.mouse_down("btn-listen-to-letters"),
                "Listen took the cursor"
            );
            ui.click("btn-listen-to-letters");
            assert_eq!(ui.screen(), "listen");
            let kept = ui.with_app(|ctx| ctx.settings.peek().contest.my_call.clone());
            assert_eq!(kept, "DL1ABC");
            // With nothing to type, it is an ordinary button.
            let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
            assert!(ui.mouse_down("btn-listen-to-letters"));
        });
    }

    /// A Simple QSO sends your name: with the call given and no name, Home
    /// asks for it in the hero, a start says it is missing, and once it is
    /// given the run starts, sending it.
    #[test]
    fn home_asks_for_your_name_before_a_simple_qso() {
        run(|| async {
            let mut settings = crate::testing::contest_settings();
            settings.contest.contest_kind = cw_core::ContestKind::SimpleQso;
            let (mut ui, recorder) = Ui::app_with_settings(settings);
            assert!(ui.shows("field-your-name"));
            assert!(!ui.shows("field-your-callsign"));
            assert!(ui.has("Add your name, press Start contest"));
            ui.click("btn-start-training");
            ui.advance(50).await;
            assert_eq!(ui.screen(), "home");
            assert!(ui.has("Add your name to start a Simple\u{a0}QSO"));
            assert!(!ui.has("class=\"toast\""), "said on Home, not in a toast");

            ui.commit("field-your-name", "mat");
            ui.advance(50).await;
            assert_eq!(ui.stored_settings().await.contest.my_name, "MAT");
            assert!(!ui.shows("field-your-name"));
            assert!(!ui.shows("home-missing"));
            assert!(ui.has("You send 599 + name, as DL1ABC"));
            ui.click("btn-start-training");
            assert!(ui.run_until(5_000, |ui| ui.screen() == "training").await);
            ui.advance(10_000).await;
            let first = &recorder.starts()[0].transcript;
            assert!(
                first.text.starts_with("CQ CQ DE DL1ABC DL1ABC K"),
                "{}",
                first.text
            );
        });
    }

    /// The NA Sprint sends your name and your state: Home asks for the
    /// name, then the state, saying what is missing and taking the cursor
    /// to the field; a state that is none is said so under its field,
    /// however it was kept; once it is given the run starts, its CQ the
    /// Sprint's.
    #[test]
    fn home_asks_for_your_name_then_your_state_before_a_sprint() {
        run(|| async {
            let mut settings = crate::testing::contest_settings();
            settings.contest.contest_kind = cw_core::ContestKind::Sprint;
            settings.contest.my_call = "K1ABC".to_string();
            let (mut ui, recorder) = Ui::app_with_settings(settings);
            assert!(ui.shows("field-your-name"));
            ui.click("btn-start-training");
            ui.advance(50).await;
            assert_eq!(ui.screen(), "home");
            assert!(ui.has("Add your name to start an NA\u{a0}Sprint"));
            assert_eq!(ui.take_focused(), ["field-your-name"]);

            // The press that keeps the name brings in the state's field, and
            // the cursor goes there.
            ui.type_into("field-your-name", "mike");
            assert!(
                !ui.mouse_down("btn-start-training"),
                "Start took the cursor"
            );
            ui.click("btn-start-training");
            ui.advance(50).await;
            assert_eq!(ui.stored_settings().await.contest.my_name, "MIKE");
            assert!(ui.shows("field-your-state-or-province"));
            assert!(ui.has("Add your state, press Start contest"));
            assert!(ui.has("Add your state or province to start an NA\u{a0}Sprint"));
            assert_eq!(ui.take_focused(), ["field-your-state-or-province"]);
            ui.click("btn-start-training");
            ui.advance(50).await;
            assert_eq!(ui.screen(), "home");
            assert_eq!(ui.take_focused(), ["field-your-state-or-province"]);

            // A state written out is no code: kept as none, and said so,
            // whether the field was left or Start pressed from it.
            ui.commit("field-your-state-or-province", "Mass");
            ui.advance(50).await;
            assert_eq!(ui.stored_settings().await.contest.my_state, "");
            assert!(ui.has("MASS is not a state or province"), "{}", ui.html());
            ui.type_into("field-your-state-or-province", "Ontario");
            ui.click("btn-start-training");
            ui.advance(50).await;
            assert_eq!(ui.screen(), "home");
            assert!(
                ui.has("ONTARIO is not a state or province"),
                "{}",
                ui.html()
            );
            assert!(ui.has("Add your state or province to start an NA\u{a0}Sprint"));
            ui.commit("field-your-state-or-province", "ma");
            ui.advance(50).await;
            let kept = ui.stored_settings().await.contest;
            assert_eq!(
                (kept.my_name.as_str(), kept.my_state.as_str()),
                ("MIKE", "MA")
            );
            assert!(ui.has("You send serial + name + state, as K1ABC"));
            ui.click("btn-start-training");
            assert!(ui.run_until(5_000, |ui| ui.screen() == "training").await);
            ui.advance(10_000).await;
            let first = &recorder.starts()[0].transcript;
            assert!(first.text.starts_with("CQ NA K1ABC"), "{}", first.text);
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
                let stored = ui.stored_settings().await;
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
    fn nothing_on_the_settings_screen_can_break_an_order_of_ones_own() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Koch;
            settings.curriculum.custom_sequence = vec!['K', 'M', 'U', 'R', 'E'];
            settings.curriculum.sequence_is_custom = true;
            settings.curriculum.level = 4;
            sweep("nav-settings", settings).await;
        });
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_a_word_list() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Words;
            settings.curriculum.word_list = vec!["PARIS".into(), "73".into()];
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
            assert_eq!(ui.stored_settings().await.curriculum.callsign_level, 6);
        });
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_contest_mode() {
        run(|| async {
            sweep("nav-settings", crate::testing::contest_settings()).await;
        });
    }

    /// In Contest mode what a group has means nothing, and nor does a
    /// preset that sets it, a level that never moves, or a pile-up's spread
    /// and level around a wanted station that is not there: they go.
    #[test]
    fn the_settings_screen_only_offers_what_a_contest_has() {
        run(|| async {
            let mut settings = crate::testing::contest_settings();
            settings.band.stations_min = 3;
            settings.band.stations_max = 3;
            settings.playback.adaptive_speed = true;
            settings.curriculum.adaptive_group_size = true;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            ui.open_disclosures();
            for shown in [
                "Contest · CQ WPX",
                "Your station",
                "Minutes per run",
                "Callsign tier (1–6)",
                "Band activity",
                "Countdown before the first CQ",
                "Key speed",
                "Character sampling",
            ] {
                assert!(ui.has(shown), "{shown} should show");
            }
            for gone in [
                "select-experience",
                "number-groups-per-session",
                "number-callsigns-per-session",
                "number-answer-timeout",
                "number-pause-after-each-group",
                "switch-adapt-group-size-to-me",
                "switch-lock-typing-while-sending",
                "switch-adapt-speed-to-me",
                "switch-adjust-the-level-for-me",
                "slider-pile-up-spread",
                "slider-pile-up-is-weaker-by",
                "fixed-stations-calling",
                "field-sequence-order",
                "field-digits-level",
            ] {
                assert!(!ui.shows(gone), "{gone} has no meaning in a contest");
            }
            for gone in [
                "Experience",
                "Sends per group",
                "Group size",
                "Aim for",
                "Practice window",
                "What this tier sends",
                "Auto level",
                "Stations calling",
            ] {
                assert!(!ui.has(gone), "{gone} has no meaning in a contest");
            }

            // A run's length is a setting of its own.
            ui.commit("number-minutes-per-run", "25");
            ui.advance(50).await;
            assert_eq!(ui.stored_settings().await.contest.contest_minutes, 25);

            // Callsigns is a group mode again: its controls come back, and
            // the station goes.
            ui.click("seg-callsigns");
            ui.advance(50).await;
            assert!(ui.has("Callsigns per session"));
            assert!(ui.has("Stations calling"));
            assert!(!ui.shows("field-your-callsign"));
            assert!(!ui.has("Minutes per run"));
        });
    }

    /// The run style is chosen on the Character set card, which says how
    /// many a pile-up brings and where that is set; the Band card's range is
    /// then the callers at once, with their spread and how many the filter
    /// fits, and back to the band's activity for a single caller. Home says
    /// who calls.
    #[test]
    fn a_pile_up_is_chosen_with_the_contest_and_shaped_on_the_band_card() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
            ui.click("nav-settings");
            ui.open_disclosures();
            for shown in ["Run style", "One station calls at a time", "Band activity"] {
                assert!(ui.has(shown), "{shown} should show");
            }
            assert!(!ui.shows("slider-pile-up-spread"));
            assert!(!ui.shows("filter-fits"));

            // A pile-up of one is none: picked on the range as it ships,
            // the range opens to one to three.
            ui.click("style-pile-up");
            ui.advance(50).await;
            let stored = ui.stored_settings().await;
            assert_eq!(stored.contest.contest_style, cw_core::RunStyle::Pileup);
            assert_eq!((stored.band.stations_min, stored.band.stations_max), (1, 3));
            assert!(
                ui.has("1–3 callers · set on the Band card"),
                "{}",
                ui.html()
            );
            assert!(ui.has("Callers at once"));
            assert!(!ui.has("Band activity"));
            assert!(ui.shows("slider-pile-up-spread"));
            assert!(ui.shows("filter-fits"));
            assert!(ui.has("Your filter fits"));
            assert!(
                !ui.shows("slider-pile-up-is-weaker-by"),
                "no wanted station to sit under"
            );
            // Held at one, it counts one caller.
            ui.click("link-callers-at-once");
            ui.advance(50).await;
            assert!(ui.has("1 caller · set on the Band card"));
            assert!(ui.has("1 caller<"), "{}", ui.html());
            assert!(!ui.has("1 callers"));
            ui.click("link-callers-at-once");
            ui.commit("from-callers-at-once", "1");
            ui.commit("to-callers-at-once", "3");
            ui.advance(50).await;
            let stored = ui.stored_settings().await;
            assert_eq!((stored.band.stations_min, stored.band.stations_max), (1, 3));
            assert!(ui.has("1–3 callers · set on the Band card"));

            ui.click("nav-practice");
            assert!(ui.has("1–3</span>"), "{}", ui.html());
            assert!(ui.has("Callers</span>"));

            ui.click("nav-settings");
            ui.click("style-single-caller");
            ui.advance(50).await;
            assert!(ui.has("Band activity"));
            assert!(!ui.shows("slider-pile-up-spread"));
            ui.click("nav-practice");
            assert!(ui.has("Single</span>"));
        });
    }

    /// Your call is typed once, kept as a call is written, and read back:
    /// where a contest places it, or why it is no call yet.
    #[test]
    fn your_station_keeps_your_call_and_reads_it() {
        run(|| async {
            let mut settings = crate::testing::contest_settings();
            settings.contest.my_call.clear();
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            assert!(ui.has("No licence yet? Make up a call"));
            assert!(
                ui.has("CQ &#60;your call&#62; TEST"),
                "the QSO waits for your call"
            );
            assert!(
                ui.has("Add your callsign to start a contest"),
                "the card says what a run is missing, as Home does"
            );

            ui.commit("field-your-callsign", "abc");
            ui.advance(50).await;
            assert!(ui.has("Not a callsign yet: needs a digit"));
            assert!(!ui.has("Add your callsign to start a contest"));
            assert!(
                ui.has("CQ &#60;your call&#62; TEST"),
                "a call that does not read is not put in the QSO"
            );

            ui.commit("field-your-callsign", " dl1abc ");
            ui.advance(50).await;
            assert_eq!(ui.stored_settings().await.contest.my_call, "DL1ABC");
            assert!(ui.has("Reads as DL · WPX prefix DL1"));
            assert!(
                ui.input_tag("field-your-callsign")
                    .contains("value=\"DL1ABC\"")
            );
            for line in ["CQ DL1ABC TEST", "K5XYZ 5NN TT1", "TU 5NN 214"] {
                assert!(ui.has(line), "the QSO should read {line}");
            }

            // Its own reset clears it, and nothing else.
            ui.commit("number-minutes-per-run", "15");
            ui.advance(50).await;
            ui.click("reset-your-station");
            ui.advance(50).await;
            let stored = ui.stored_settings().await;
            assert!(stored.contest.my_call.is_empty());
            assert_eq!(stored.contest.contest_minutes, 15);
            assert_eq!(stored.curriculum.char_set_mode, CharSetMode::Contest);
        });
    }

    /// Your state, for the Sprint, reads as the place it names; what is no
    /// state is said so only while nothing else of your station has changed
    /// since: your call put outside the US and Canada sends DX, and the
    /// card's reset leaves nothing to say.
    #[test]
    fn your_station_says_what_your_state_reads_as_while_it_is_so() {
        run(|| async {
            let mut settings = crate::testing::contest_settings();
            settings.contest.contest_kind = cw_core::ContestKind::Sprint;
            settings.contest.my_call = "K1ABC".to_string();
            settings.contest.my_name = "MIKE".to_string();
            settings.contest.my_state = "MA".to_string();
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            assert!(ui.has("Reads as Massachusetts"));
            ui.commit("field-your-state-or-province", "Mass");
            ui.advance(50).await;
            assert_eq!(ui.stored_settings().await.contest.my_state, "");
            assert!(ui.has("MASS is not a state or province"));
            ui.commit("field-your-callsign", "DL1ABC");
            ui.advance(50).await;
            assert!(!ui.has("MASS is not a state or province"));
            assert!(ui.has("Sends DX: in Dust every station outside the US and Canada sends DX"));

            ui.commit("field-your-callsign", "K1ABC");
            ui.commit("field-your-state-or-province", "Mass");
            ui.advance(50).await;
            assert!(ui.has("MASS is not a state or province"));
            ui.click("reset-your-station");
            ui.advance(50).await;
            assert!(ui.stored_settings().await.contest.my_call.is_empty());
            assert!(!ui.has("MASS is not a state or province"), "{}", ui.html());
            // Nor does it come back with the call it was typed under.
            ui.commit("field-your-callsign", "K1ABC");
            ui.advance(50).await;
            assert!(!ui.has("MASS is not a state or province"));
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
            let stored = ui.stored_settings().await;
            assert_eq!(stored.curriculum.callsign_level, 5);
            assert_eq!(stored.curriculum.level, 9, "the Koch level is untouched");

            ui.click("seg-koch");
            ui.advance(50).await;
            let stored = ui.stored_settings().await;
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

    /// Nothing that happens over and over may cost the renderer more than a
    /// handful of mutations.
    ///
    /// On the desktop build every mutation crosses to the webview on the same
    /// loop that carries your keystrokes, so this is the number that decides
    /// whether typing feels immediate. Building a screen is allowed to be
    /// expensive — opening Settings is hundreds, and it happens once. What is
    /// not allowed is a repeating cost that scales with anything.
    #[test]
    fn nothing_that_repeats_costs_the_renderer_much() {
        run(|| async {
            let (mut ui, _rec) = Ui::app_with_settings(test_settings());
            ui.click("btn-start-training");
            assert!(ui.run_until(20_000, |ui| ui.screen() == "training").await);

            // Nothing redraws on a timer while a group is sending.
            let _ = ui.take_work();
            ui.advance(40).await;
            let sending = ui.take_work();
            assert!(sending <= 4, "40 ms of sending cost {sending} mutations");

            assert!(ui.run_until(30_000, |ui| ui.has("Your turn")).await);

            // Nothing may run on its own while you are reading and typing.
            let _ = ui.take_work();
            ui.advance(400).await;
            let idle = ui.take_work();
            assert_eq!(idle, 0, "the screen repainted {idle} times doing nothing");

            // And a key is one box, not the list.
            ui.type_into("group-input-0", "K");
            let key = ui.take_work();
            assert!(key <= 4, "a keystroke cost {key} mutations");
        });
    }
}
