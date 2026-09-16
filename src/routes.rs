use std::rc::Rc;

use cw_core::{compute_char_pool, SessionEvent};
use dioxus::prelude::*;

use crate::engine::{current_auto_progress, AppState, Screen, SessionSignals};
use crate::session_runtime::send_command;
use crate::time::local_date_string;
use crate::ui::home::Home;
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;
use crate::ui::training::TrainingView;

/// What the screens show beyond the session itself: whichever preview is
/// running, and the sample chip that is lit.
#[derive(Clone, PartialEq)]
pub struct ViewState {
    pub previewing: bool,
    pub listen_playing: bool,
    pub sample_playing: Option<String>,
}

/// What a screen can ask the app to do for it.
#[derive(Clone, Copy)]
pub struct AppCallbacks {
    pub start_training: EventHandler<()>,
    pub go_home: EventHandler<()>,
    pub go_listen: EventHandler<()>,
    pub start_band_preview: EventHandler<()>,
    pub stop_preview: EventHandler<()>,
    pub start_listen: EventHandler<String>,
    pub play_sample: EventHandler<String>,
}

pub fn app_routes(
    signals: SessionSignals,
    view: ViewState,
    app: Rc<AppState>,
    callbacks: AppCallbacks,
) -> Element {
    let SessionSignals {
        screen,
        runtime,
        result,
        auto_message,
        sessions,
        settings,
        toast: _,
    } = signals;
    let ViewState {
        previewing,
        listen_playing,
        sample_playing,
    } = view;
    let AppCallbacks {
        start_training,
        go_home,
        go_listen,
        start_band_preview,
        stop_preview,
        start_listen,
        play_sample,
    } = callbacks;
    match screen() {
        Screen::Home => {
            let pool: String = compute_char_pool(&settings()).into_iter().collect();
            // Every session counts, whatever character set it was recorded with.
            let history = sessions();
            let last = history.last().map(|s| s.accuracy);
            rsx! {
                Home {
                    settings: settings(),
                    last_accuracy: last,
                    session_count: history.len(),
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
                sample_playing,
                on_preview_band: start_band_preview,
                on_stop_band: stop_preview,
                on_play_sample: play_sample,
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
                // Between sends there is nobody in the passband, so the scope
                // shows the filter alone rather than a station that has stopped.
                let heard = if playing {
                    view.sent
                        .get(view.current)
                        .map(|text| {
                            crate::session_runtime::heard_for(
                                &settings(),
                                app.session_gen.get(),
                                view.current,
                                text,
                            )
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let app_change = app.clone();
                let app_confirm = app.clone();
                let app_focus = app.clone();
                let app_submit = app.clone();
                let app_stop = app;
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
                        repeat_total: view.repeat_total,
                        repeat_done: view.repeat_done,
                        settings: settings(),
                        heard,
                        // Changes on every send, repeats included, so the
                        // scope's timebase restarts with the keying.
                        send_id: (view.current as u64) << 16 | u64::from(view.repeat_done),
                        on_change: move |(idx, value): (usize, String)| {
                            send_command((*app_change).clone(), signals, SessionEvent::Input { index: idx, text: value });
                        },
                        on_confirm: move |_idx| {
                            send_command((*app_confirm).clone(), signals, SessionEvent::Confirm);
                        },
                        on_focus: move |idx| {
                            send_command((*app_focus).clone(), signals, SessionEvent::Focus { index: idx });
                        },
                        on_submit: move |_| {
                            send_command((*app_submit).clone(), signals, SessionEvent::FinishNow);
                        },
                        on_stop: move |_| {
                            send_command((*app_stop).clone(), signals, SessionEvent::Abort);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{run, test_settings, Ui};

    /// The screens the router draws when the state behind them is not there
    /// yet: the moment between starting a session and its first group, and a
    /// results screen with nothing to show.
    #[component]
    fn EmptyRoute(screen: Screen) -> Element {
        let signals = SessionSignals {
            screen: use_signal(|| screen),
            runtime: use_signal(|| None),
            result: use_signal(|| None),
            auto_message: use_signal(|| None),
            sessions: use_signal(Vec::new),
            settings: use_signal(test_settings),
            toast: use_signal(|| None),
        };
        let app = use_hook(|| Rc::new(AppState::new()));
        let noop = EventHandler::new(move |_| {});
        let noop_text = EventHandler::new(move |_: String| {});
        app_routes(
            signals,
            ViewState {
                previewing: false,
                listen_playing: false,
                sample_playing: None,
            },
            app,
            AppCallbacks {
                start_training: noop,
                go_home: noop,
                go_listen: noop,
                start_band_preview: noop,
                stop_preview: noop,
                start_listen: noop_text,
                play_sample: noop_text,
            },
        )
    }

    #[test]
    fn a_session_that_has_not_started_says_so() {
        run(|| async {
            let ui = Ui::new(
                EmptyRoute,
                EmptyRouteProps {
                    screen: Screen::Training,
                },
            );
            assert!(ui.has("Starting…"));
        });
    }

    #[test]
    fn the_practice_screen_is_reachable_without_any_state() {
        run(|| async {
            let mut ui = Ui::new(
                EmptyRoute,
                EmptyRouteProps {
                    screen: Screen::Home,
                },
            );
            assert!(ui.has("Start training"));
            // Both hero buttons report upwards without anything behind them.
            ui.click("btn-start-training");
            ui.click("btn-listen-to-letters");
            assert!(ui.has("Start training"));
        });
    }

    #[test]
    fn a_results_screen_with_no_result_says_so() {
        run(|| async {
            let ui = Ui::new(
                EmptyRoute,
                EmptyRouteProps {
                    screen: Screen::Results,
                },
            );
            assert!(ui.has("No result."));
        });
    }
}
