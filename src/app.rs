use std::rc::Rc;

use cw_core::{fit_settings_to_alphabet, GroupSession};
use dioxus::prelude::*;

use crate::engine::{loop_preview_text, play_chars, run_group_session, AppState, Screen};
use crate::persist::{load_sessions, load_settings, save_settings};
use crate::routes::app_routes;
use crate::time::sleep_ms;

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
    let app = use_hook(AppState::new);
    let app = Rc::new(app);

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
            screen.set(Screen::Training);
            let gen = match app.takeover_audio(&settings_now) {
                Ok(gen) => gen,
                Err(err) => {
                    screen.set(Screen::Home);
                    toast.set(Some(err));
                    return;
                }
            };
            let history = sessions();
            let app_loop = (*app).clone();
            spawn(run_group_session(
                settings_now,
                history,
                app_loop,
                gen,
                runtime,
                screen,
                result,
                auto_message,
                sessions,
                settings,
                toast,
            ));
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
            screen.set(Screen::Settings);
        }
    });
    let show_nav = !matches!(screen(), Screen::Training);
    let shell_class = if show_nav { "shell has-nav" } else { "shell" };

    rsx! {
        document::Title { "Dust" }
        document::Link { rel: "stylesheet", href: asset!("/assets/styles.css") }
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1, viewport-fit=cover",
        }
        document::Meta { name: "theme-color", content: "#1c2740" }
        document::Meta { name: "mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-status-bar-style", content: "black-translucent" }
        document::Link {
            rel: "stylesheet",
            href: "https://fonts.googleapis.com/css2?family=Fraunces:opsz,wght@9..144,600;700&family=Figtree:wght@400;500;600;700&family=IBM+Plex+Mono:wght@500;700&display=swap",
        }
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
                p { class: "brand-name", "Dust" }
                if screen() == Screen::Training {
                    button { class: "btn btn-ghost header-ghost", onclick: move |_| go_home.call(()), "Exit" }
                }
            }
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
                app.clone(),
                start_training,
                go_home,
                go_listen,
                start_band_preview,
                stop_preview,
                start_listen,
            ) }
            }
            if show_nav {
            nav { class: "bottom-nav",
                button {
                    class: if matches!(screen(), Screen::Home | Screen::Listen | Screen::Results) { "nav-item active" } else { "nav-item" },
                    onclick: move |_| go_home.call(()),
                    span { class: "nav-icon", "⌁" }
                    span { "Practice" }
                }
                button {
                    class: if screen() == Screen::Stats { "nav-item active" } else { "nav-item" },
                    onclick: move |_| go_stats.call(()),
                    span { class: "nav-icon", "▣" }
                    span { "Stats" }
                }
                button {
                    class: if screen() == Screen::Settings { "nav-item active" } else { "nav-item" },
                    onclick: move |_| go_settings.call(()),
                    span { class: "nav-icon", "⚙" }
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
