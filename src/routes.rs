use std::rc::Rc;

use cw_core::{compute_char_pool, GroupSession, SessionEvent, SessionResult, TrainingSettings};
use dioxus::prelude::*;

use crate::engine::{current_auto_progress, AppState, Screen};
use crate::session_runtime::send_command;
use crate::time::local_date_string;
use crate::ui::home::Home;
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;
use crate::ui::training::TrainingView;

pub fn app_routes(
    screen: Signal<Screen>,
    settings: Signal<TrainingSettings>,
    sessions: Signal<Vec<SessionResult>>,
    runtime: Signal<Option<GroupSession>>,
    result: Signal<Option<SessionResult>>,
    auto_message: Signal<Option<String>>,
    toast: Signal<Option<String>>,
    previewing: bool,
    listen_playing: bool,
    app: Rc<AppState>,
    start_training: EventHandler<()>,
    go_home: EventHandler<()>,
    go_listen: EventHandler<()>,
    start_band_preview: EventHandler<()>,
    stop_preview: EventHandler<()>,
    start_listen: EventHandler<String>,
) -> Element {
    match screen() {
        Screen::Home => {
            let pool: String = compute_char_pool(&settings()).into_iter().collect();
            let matching: Vec<_> = sessions()
                .iter()
                .filter(|s| s.usable_for_sampling(&settings()))
                .cloned()
                .collect();
            let last = matching.last().map(|s| s.accuracy);
            rsx! {
                Home {
                    settings: settings(),
                    last_accuracy: last,
                    session_count: matching.len(),
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
                previewing,
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
                playing: listen_playing,
                on_play: start_listen,
                on_stop: stop_preview,
                on_back: move |_| go_home.call(()),
            }
        },
        Screen::Training => {
            let session = runtime();
            if let Some(session) = session {
                let view = session.view();
                let playing = view.status == cw_core::RuntimeStatus::PlayingGroup;
                let app_change = app.clone();
                let app_confirm = app.clone();
                let app_focus = app.clone();
                let app_submit = app.clone();
                let app_stop = app.clone();
                rsx! {
                    TrainingView {
                        current: view.current,
                        total: view.sent.len(),
                        groups: view.sent,
                        inputs: view.inputs,
                        confirmed: view.confirmed,
                        focused: view.focused,
                        playing,
                        locked: view.locked,
                        on_change: move |(idx, value): (usize, String)| {
                            send_command(
                                (*app_change).clone(),
                                runtime,
                                screen,
                                result,
                                auto_message,
                                sessions,
                                settings,
                                toast,
                                SessionEvent::Input { index: idx, text: value },
                            );
                        },
                        on_confirm: move |_idx| {
                            send_command(
                                (*app_confirm).clone(),
                                runtime,
                                screen,
                                result,
                                auto_message,
                                sessions,
                                settings,
                                toast,
                                SessionEvent::Confirm,
                            );
                        },
                        on_focus: move |idx| {
                            send_command(
                                (*app_focus).clone(),
                                runtime,
                                screen,
                                result,
                                auto_message,
                                sessions,
                                settings,
                                toast,
                                SessionEvent::Focus { index: idx },
                            );
                        },
                        on_submit: move |_| {
                            send_command(
                                (*app_submit).clone(),
                                runtime,
                                screen,
                                result,
                                auto_message,
                                sessions,
                                settings,
                                toast,
                                SessionEvent::FinishNow,
                            );
                        },
                        on_stop: move |_| {
                            send_command(
                                (*app_stop).clone(),
                                runtime,
                                screen,
                                result,
                                auto_message,
                                sessions,
                                settings,
                                toast,
                                SessionEvent::Abort,
                            );
                        },
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
