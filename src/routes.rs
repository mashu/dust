use std::rc::Rc;

use cw_core::{Paddle, SessionEvent, compute_char_pool, dit_ms_for_wpm};
use dioxus::prelude::*;

use crate::persist::current_auto_progress;
use crate::session_runtime::send_command;
use crate::state::{AppState, Screen, SessionSignals};
use crate::time::local_date_string;
use crate::ui::home::Home;
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;
use crate::ui::training::{TrainingScope, TrainingView};

/// What the screens show beyond the session itself: whichever preview is
/// running, and the sample chip that is lit.
#[derive(Clone, PartialEq)]
pub struct ViewState {
    pub previewing: bool,
    pub listen_playing: bool,
    pub listen_streaming: bool,
    pub stream_heard: Vec<String>,
    pub sample_playing: Option<String>,
    pub paddle_heard: String,
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
    pub start_stream: EventHandler<()>,
    pub play_sample: EventHandler<String>,
    pub on_paddle_down: EventHandler<Paddle>,
    pub on_paddle_up: EventHandler<Paddle>,
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
        listen_streaming,
        stream_heard,
        sample_playing,
        paddle_heard,
    } = view;
    let AppCallbacks {
        start_training,
        go_home,
        go_listen,
        start_band_preview,
        stop_preview,
        start_listen,
        start_stream,
        play_sample,
        on_paddle_down,
        on_paddle_up,
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
                paddle_heard,
                on_paddle_down,
                on_paddle_up,
            }
        },
        Screen::Stats => rsx! {
            StatsView { settings: settings(), sessions: sessions() }
        },
        Screen::Listen => rsx! {
            ListenView {
                settings: settings(),
                playing: listen_playing,
                streaming: listen_streaming,
                heard: stream_heard,
                on_play: start_listen,
                on_stream: start_stream,
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
                let app_tone = app;
                let settings = session.settings();
                let dit_ms = dit_ms_for_wpm(settings.playback.keyer_wpm);
                let paddle_swap = settings.playback.paddle_swap;
                let keyer_mode = settings.playback.keyer_mode;
                let tone_settings = settings.clone();
                rsx! {
                    div { class: "stack",
                        TrainingScope {
                            focused: view.focused,
                            total: view.sent.len(),
                            playing,
                            repeat_total: view.repeat_total,
                            repeat_done: view.repeat_done,
                        }
                        TrainingView {
                            current: view.current,
                            groups: view.sent,
                            inputs: view.inputs,
                            confirmed: view.confirmed,
                            focused: view.focused,
                            playing,
                            locked: view.locked,
                            repeat_total: view.repeat_total,
                            repeat_done: view.repeat_done,
                            paddle_swap,
                            keyer_mode,
                            dit_ms,
                            on_tone: move |on| {
                                app_tone.set_live_tone(on, &tone_settings);
                            },
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
    use crate::testing::{Ui, run, test_settings};

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
        let noop_paddle = EventHandler::new(move |_: Paddle| {});
        app_routes(
            signals,
            ViewState {
                previewing: false,
                listen_playing: false,
                listen_streaming: false,
                stream_heard: Vec::new(),
                sample_playing: None,
                paddle_heard: String::new(),
            },
            app,
            AppCallbacks {
                start_training: noop,
                go_home: noop,
                go_listen: noop,
                start_band_preview: noop,
                stop_preview: noop,
                start_listen: noop_text,
                start_stream: noop,
                play_sample: noop_text,
                on_paddle_down: noop_paddle,
                on_paddle_up: noop_paddle,
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
