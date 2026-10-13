//! One component per screen, each reading the state it shows and nothing
//! more. The shell above them redraws for its own state only, and a screen
//! redraws for its own: a toast or a theme change does not rebuild the
//! practice screen's history, and a key in a training answer does not
//! rebuild the shell.

use std::cell::RefCell;
use std::rc::Rc;

use cw_core::{SessionEvent, TrainingSettings, compute_char_pool};
use dioxus::prelude::*;

use crate::persist::auto_progress;
use crate::playback;
use crate::session_runtime::{send_event, start_session};
use crate::state::{AppCtx, Screen};
use crate::time::{local_date_string, sleep_ms};
use crate::ui::home::Home;
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;
use crate::ui::training::{TrainingProgress, TrainingView};

/// How often the band follows the settings while it is being previewed:
/// often enough to sound live, seldom enough that a slider dragged across
/// the noise does not rebuild the band, and measure its AGC, every frame.
const BAND_FOLLOW_MS: u32 = 80;

/// The screen `screen` names.
#[component]
pub fn Route(screen: Screen) -> Element {
    match screen {
        Screen::Home => rsx! { HomeRoute {} },
        Screen::Settings => rsx! { SettingsRoute {} },
        Screen::Stats => rsx! { StatsRoute {} },
        Screen::Listen => rsx! { ListenRoute {} },
        Screen::Training => rsx! { TrainingRoute {} },
        Screen::Results => rsx! { ResultsRoute {} },
    }
}

#[component]
fn HomeRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    let on_start = use_callback(move |()| start_session(ctx));
    let on_listen = use_callback(move |()| ctx.go_to(Screen::Listen));
    let settings = ctx.settings.read();
    // Every session counts, whatever character set it was recorded with.
    let sessions = ctx.sessions.read();
    rsx! {
        Home {
            settings: settings.clone(),
            last_accuracy: sessions.last().map(|s| s.accuracy),
            session_count: sessions.len(),
            pool: compute_char_pool(&settings).into_iter().collect::<String>(),
            sessions: sessions.clone(),
            today: local_date_string(),
            auto_progress: auto_progress(&*ctx.app().store, &settings),
            on_start,
            on_listen,
        }
    }
}

#[component]
fn SettingsRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    let on_preview_band = use_callback(move |()| playback::start_band_preview(ctx));
    let on_stop_band = use_callback(move |()| ctx.stop_audio());
    let on_play_sample = use_callback(move |text| playback::start_sample(ctx, text));
    use_band_follow(ctx);
    let preview = ctx.preview.read();
    rsx! {
        SettingsView {
            settings: ctx.settings,
            previewing: preview.is_band(),
            sample_playing: preview.sample(),
            on_preview_band,
            on_stop_band,
            on_play_sample,
        }
    }
}

/// While the band preview runs, the band follows the settings: the latest
/// of them, at most every [`BAND_FOLLOW_MS`].
fn use_band_follow(ctx: AppCtx) {
    let latest = use_hook(|| Rc::new(RefCell::new(None::<TrainingSettings>)));
    use_effect(move || {
        if !ctx.preview.read().is_band() {
            return;
        }
        let settings = ctx.settings.read().clone().clamp();
        if latest.borrow_mut().replace(settings).is_some() {
            // A follow is already on its way, and will take these.
            return;
        }
        let latest = latest.clone();
        spawn(async move {
            sleep_ms(BAND_FOLLOW_MS).await;
            let settings = latest.borrow_mut().take();
            // A preview stopped meanwhile stays silent.
            if let Some(settings) = settings
                && ctx.preview.peek().is_band()
            {
                ctx.app().player.follow_band(&settings);
            }
        });
    });
}

#[component]
fn StatsRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    rsx! {
        StatsView { settings: ctx.settings.read().clone(), sessions: ctx.sessions.read().clone() }
    }
}

#[component]
fn ListenRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    let on_play = use_callback(move |chars| playback::start_letters(ctx, chars));
    let on_stream = use_callback(move |()| playback::start_stream(ctx));
    let on_stop = use_callback(move |()| ctx.stop_audio());
    let on_back = use_callback(move |()| ctx.go_to(Screen::Home));
    let preview = ctx.preview.read();
    #[cfg(feature = "desktop")]
    let floating = use_context::<crate::ui::floating::FloatingWindow>();
    #[cfg(feature = "desktop")]
    let (can_float, is_floating, on_float) = (
        true,
        floating.is_open(),
        EventHandler::new(move |()| floating.toggle()),
    );
    #[cfg(not(feature = "desktop"))]
    let (can_float, is_floating, on_float) = (false, false, EventHandler::new(|()| {}));
    rsx! {
        ListenView {
            can_float,
            floating: is_floating,
            on_float,
            settings: ctx.settings.read().clone(),
            playing: preview.is_letters(),
            streaming: preview.is_stream(),
            heard: ctx.stream_heard.read().clone(),
            on_play,
            on_stream,
            on_stop,
            on_back,
        }
    }
}

#[component]
fn TrainingRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    let on_change = use_callback(move |(index, text): (usize, String)| {
        send_event(ctx, SessionEvent::Input { index, text });
    });
    let on_confirm = use_callback(move |()| send_event(ctx, SessionEvent::Confirm));
    let on_submit = use_callback(move |()| send_event(ctx, SessionEvent::FinishNow));
    let on_stop = use_callback(move |()| send_event(ctx, SessionEvent::Abort));
    let runtime = ctx.runtime.read();
    let Some(session) = runtime.as_ref() else {
        return rsx! { p { class: "muted", "Starting…" } };
    };
    let view = session.view();
    let keyer_mode = session.settings().playback.keyer_mode;
    rsx! {
        div { class: "stack",
            TrainingProgress {
                focused: view.focused,
                total: view.sent.len(),
                playing: view.playing,
                repeat_total: view.repeat_total,
                repeat_done: view.repeat_done,
            }
            TrainingView {
                current: view.current,
                groups: view.sent,
                inputs: view.inputs,
                confirmed: view.confirmed,
                focused: view.focused,
                playing: view.playing,
                locked: view.locked,
                repeat_total: view.repeat_total,
                repeat_done: view.repeat_done,
                keyer_mode,
                answer: ctx.answer,
                on_change,
                on_confirm,
                on_submit,
                on_stop,
            }
        }
    }
}

#[component]
fn ResultsRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    let on_again = use_callback(move |()| start_session(ctx));
    let on_home = use_callback(move |()| ctx.go_to(Screen::Home));
    let Some(result) = ctx.result.read().clone() else {
        return rsx! { p { class: "muted", "No result." } };
    };
    rsx! {
        ResultsView {
            result,
            auto_message: ctx.auto_message.read().clone(),
            on_again,
            on_home,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use crate::testing::{Ui, run, test_settings};

    /// A screen with nothing behind it yet: the moment between starting a
    /// session and its first group, or a results screen with nothing to show.
    #[component]
    fn EmptyRoute(screen: Screen) -> Element {
        use_context_provider(|| AppCtx::new(AppState::new(), test_settings(), Vec::new()));
        rsx! { Route { screen } }
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
            let ui = Ui::new(
                EmptyRoute,
                EmptyRouteProps {
                    screen: Screen::Home,
                },
            );
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
