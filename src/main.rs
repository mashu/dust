mod audio;
mod engine;
mod persist;
mod time;
mod ui;

use std::rc::Rc;

use cw_core::{compute_char_pool, GroupSession, TrainingSettings};
use dioxus::prelude::*;

use crate::audio::focus_group_input;
use crate::engine::{
    confirm_group, current_auto_progress, finish_session, loop_preview_text, play_chars,
    run_group_session, AppState, Screen, AUTO_CONFIRM_DELAY_MS,
};
use crate::persist::{load_sessions, load_settings, save_settings};
use crate::time::sleep_ms;
use crate::ui::home::{Home, TrainingView};
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;

fn main() {
    dioxus::launch(App);
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
        save_settings(&settings().clamp());
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
            let settings_now = settings().clamp();
            settings.set(settings_now.clone());
            if let Err(err) = app.ensure_player(&settings_now) {
                toast.set(Some(err));
                return;
            }
            app.stop_audio();
            previewing.set(false);
            listen_playing.set(false);
            let gen = app.bump_session();
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
            let settings_now = settings().clamp();
            if let Err(err) = app.ensure_player(&settings_now) {
                toast.set(Some(err));
                return;
            }
            let gen = app.bump_session();
            listen_playing.set(true);
            previewing.set(false);
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
            let settings_now = settings().clamp();
            app.stop_audio();
            let gen = app.bump_session();
            if let Err(err) = app.ensure_player(&settings_now) {
                toast.set(Some(err));
                return;
            }
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
                            today: crate::audio::local_date_string(),
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
                        let locked = session.input_locked(session.current_group, &settings());
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
                                    let sent_len = runtime.read().as_ref().and_then(|s| s.groups.get(idx).map(|g| g.len())).unwrap_or(0);
                                    if let Some(s) = runtime.write().as_mut() {
                                        if s.input_locked(idx, &settings()) {
                                            return;
                                        }
                                        s.set_input(idx, value.clone());
                                        if !value.is_empty() && value.len() == sent_len {
                                            s.record_answer_time_if_empty(idx, crate::audio::now_ms());
                                        }
                                    }
                                    if !value.is_empty() && value.len() == sent_len {
                                        let app = app_change.clone();
                                        spawn(async move {
                                            sleep_ms(AUTO_CONFIRM_DELAY_MS).await;
                                            let still = runtime.read().as_ref().and_then(|s| s.user_input.get(idx).cloned());
                                            if still.as_deref() == Some(value.as_str()) {
                                                confirm_group(runtime, idx, Some(value), &app);
                                                focus_group_input(idx + 1);
                                                maybe_finish_if_complete(runtime, settings(), app, screen, result, auto_message, sessions, settings, toast);
                                            }
                                        });
                                    }
                                },
                                on_confirm: move |idx| {
                                    confirm_group(runtime, idx, None, &app_confirm);
                                    focus_group_input(idx + 1);
                                    maybe_finish_if_complete(runtime, settings(), app_confirm.clone(), screen, result, auto_message, sessions, settings, toast);
                                },
                                on_focus: move |idx| {
                                    if let Some(s) = runtime.write().as_mut() {
                                        s.focused_group = idx;
                                    }
                                },
                                on_submit: {
                                    let app = app.clone();
                                    move |_| {
                                        app.bump_session();
                                        finish_session(settings(), (*app).clone(), runtime, screen, result, auto_message, sessions, settings, toast);
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

fn maybe_finish_if_complete(
    runtime: Signal<Option<GroupSession>>,
    settings_now: TrainingSettings,
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
        .map(GroupSession::all_groups_answered)
        .unwrap_or(false);
    if done {
        finish_session(
            settings_now,
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
