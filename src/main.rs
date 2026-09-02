mod audio;
mod engine;
mod persist;
mod time;
mod ui;

use std::rc::Rc;

use cw_core::{
    answer_length_matches, compute_char_pool, fit_settings_to_alphabet, GroupSession,
    TrainingSettings,
};
use dioxus::prelude::*;

use crate::audio::focus_group_input;
use crate::engine::{
    confirm_group, current_auto_progress, finish_session, loop_preview_text, play_chars,
    run_group_session, AppState, Screen, AUTO_CONFIRM_DELAY_MS,
};
use crate::persist::{load_sessions, load_settings, save_settings};
use crate::time::{local_date_string, now_ms, sleep_ms};
use crate::ui::home::{Home, TrainingView};
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;

fn main() {
    #[cfg(feature = "desktop")]
    {
        use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
        dioxus::LaunchBuilder::desktop()
            .with_cfg(
                Config::new()
                    .with_menu(None)
                    .with_background_color((243, 234, 217, 255))
                    .with_window(
                        WindowBuilder::new()
                            .with_title("Dust")
                            .with_inner_size(LogicalSize::new(560.0, 860.0))
                            .with_min_inner_size(LogicalSize::new(420.0, 640.0)),
                    ),
            )
            .launch(App);
    }
    #[cfg(not(feature = "desktop"))]
    {
        dioxus::launch(App);
    }
}

#[component]
fn App() -> Element {
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
        let mut snapshot = settings();
        if matches!(screen(), Screen::Training) {
            return;
        }
        fit_settings_to_alphabet(&mut snapshot);
        save_settings(&snapshot.clamp());
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

    let start_listen = {
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
    };

    let start_band_preview = {
        let app = app.clone();
        move |_| {
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
    };

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
                div {
                    p { class: "brand-kicker", "CW copy trainer" }
                    h1 { "Dust" }
                }
                if screen() == Screen::Training {
                    button { class: "btn btn-ghost header-ghost", onclick: move |_| go_home.call(()), "Exit" }
                }
            }
            match screen() {
                Screen::Home => {
                    let pool: String = compute_char_pool(&settings()).into_iter().collect();
                    let last = sessions().last().map(|s| s.accuracy);
                    rsx! {
                        Home {
                            settings: settings(),
                            last_accuracy: last,
                            session_count: sessions().len(),
                            pool,
                            sessions: sessions(),
                            today: local_date_string(),
                            auto_progress: current_auto_progress(&settings()),
                            on_start: start_training,
                            on_listen: move |_| go_listen.call(()),
                        }
                    }
                }
                Screen::Settings => rsx! {
                    SettingsView {
                        settings,
                        previewing: previewing(),
                        on_preview_band: start_band_preview,
                        on_stop_band: stop_preview,
                    }
                },
                Screen::Stats => rsx! {
                    StatsView { settings: settings(), sessions: sessions() }
                },
                Screen::Listen => rsx! {
                    ListenView {
                        settings: settings(),
                        playing: listen_playing(),
                        on_play: start_listen,
                        on_stop: stop_preview,
                        on_back: move |_| go_home.call(()),
                    }
                },
                Screen::Training => {
                    let session = runtime();
                    if let Some(session) = session {
                        let playing = session.status == cw_core::RuntimeStatus::PlayingGroup;
                        let locked = session.input_locked(session.current_group);
                        let status = session.error_message.clone().unwrap_or_default();
                        let groups = session.groups.clone();
                        let inputs = session.user_input.clone();
                        let confirmed = session.confirmed.clone();
                        let app_confirm = app.clone();
                        let app_change = app.clone();
                        rsx! {
                            TrainingView {
                                current: session.current_group,
                                total: session.groups.len(),
                                groups,
                                inputs,
                                confirmed,
                                focused: session.focused_group,
                                playing,
                                locked,
                                status,
                                on_change: move |(idx, value): (usize, String)| {
                                    let (session_id, sent) = {
                                        let guard = runtime.read();
                                        let Some(session) = guard.as_ref() else {
                                            return;
                                        };
                                        (
                                            session.session_id,
                                            session.groups.get(idx).cloned().unwrap_or_default(),
                                        )
                                    };
                                    if let Some(s) = runtime.write().as_mut() {
                                        if s.session_id != session_id
                                            || idx != s.current_group
                                            || s.confirmed.get(idx).copied().unwrap_or(false)
                                            || s.input_locked(idx)
                                        {
                                            return;
                                        }
                                        s.set_input(idx, value.clone());
                                        if answer_length_matches(&sent, &value) {
                                            s.record_answer_time_if_empty(idx, now_ms());
                                        }
                                    }
                                    if answer_length_matches(&sent, &value) {
                                        let app = app_change.clone();
                                        spawn(async move {
                                            sleep_ms(AUTO_CONFIRM_DELAY_MS).await;
                                            let still_current = runtime.read().as_ref().is_some_and(|s| {
                                                s.session_id == session_id
                                                    && s.current_group == idx
                                                    && s.user_input.get(idx).map(String::as_str)
                                                        == Some(value.as_str())
                                                    && !s.confirmed.get(idx).copied().unwrap_or(false)
                                            });
                                            if still_current {
                                                confirm_group(runtime, idx, Some(value), &app);
                                                focus_group_input(idx + 1);
                                                maybe_finish_if_complete(runtime, app, screen, result, auto_message, sessions, settings, toast);
                                            }
                                        });
                                    }
                                },
                                on_confirm: move |idx| {
                                    let allowed = runtime.read().as_ref().is_some_and(|s| {
                                        idx == s.current_group
                                            && !s.confirmed.get(idx).copied().unwrap_or(false)
                                            && !s.input_locked(idx)
                                    });
                                    if !allowed {
                                        return;
                                    }
                                    confirm_group(runtime, idx, None, &app_confirm);
                                    focus_group_input(idx + 1);
                                    maybe_finish_if_complete(runtime, app_confirm.clone(), screen, result, auto_message, sessions, settings, toast);
                                },
                                on_focus: move |idx| {
                                    if let Some(s) = runtime.write().as_mut() {
                                        if idx == s.current_group {
                                            s.focused_group = idx;
                                        }
                                    }
                                },
                                on_submit: {
                                    let app = app.clone();
                                    move |_| {
                                        if let Some(session) = runtime.read().as_ref() {
                                            let idx = session.current_group;
                                            let locked = session.input_locked(idx);
                                            let typed = session
                                                .user_input
                                                .get(idx)
                                                .map(|value| !value.trim().is_empty())
                                                .unwrap_or(false);
                                            if typed && !locked {
                                                confirm_group(runtime, idx, None, &app);
                                            }
                                        }
                                        finish_session(
                                            (*app).clone(),
                                            runtime,
                                            screen,
                                            result,
                                            auto_message,
                                            sessions,
                                            settings,
                                            toast,
                                        );
                                    }
                                },
                                on_stop: go_home,
                            }
                        }
                    } else {
                        rsx! { p { class: "muted", "Starting…" } }
                    }
                }
                Screen::Results => {
                    if let Some(res) = result() {
                        rsx! {
                            ResultsView {
                                result: res,
                                auto_message: auto_message(),
                                on_again: start_training,
                                on_home: go_home,
                            }
                        }
                    } else {
                        rsx! { p { class: "muted", "No result." } }
                    }
                }
            }
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

fn maybe_finish_if_complete(
    runtime: Signal<Option<GroupSession>>,
    app: Rc<AppState>,
    screen: Signal<Screen>,
    result: Signal<Option<cw_core::SessionResult>>,
    auto_message: Signal<Option<String>>,
    sessions: Signal<Vec<cw_core::SessionResult>>,
    settings: Signal<TrainingSettings>,
    toast: Signal<Option<String>>,
) {
    let done = runtime
        .read()
        .as_ref()
        .map(GroupSession::all_groups_confirmed)
        .unwrap_or(false);
    if done {
        finish_session(
            (*app).clone(),
            runtime,
            screen,
            result,
            auto_message,
            sessions,
            settings,
            toast,
        );
    }
}
