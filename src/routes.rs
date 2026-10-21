//! One component per screen, each reading the state it shows and nothing
//! more. The shell above them redraws for its own state only, and a screen
//! redraws for its own: a toast or a theme change does not rebuild the
//! practice screen's history, and a key in a training answer does not
//! rebuild the shell.

use std::cell::RefCell;
use std::rc::Rc;

use cw_core::band::{FilterDesign, agc_knee};
use cw_core::{SessionEvent, TrainingSettings, compute_char_pool};
use dioxus::prelude::*;

use crate::persist::auto_progress;
use crate::playback;
use crate::session_runtime::{send_event, start_session};
use crate::state::{AppCtx, Finished, Screen, SessionPick};
use crate::time::{local_date_string, sleep_ms};
use crate::ui::backup::Backups;
use crate::ui::contest::ContestRunView;
use crate::ui::contest_results::NothingKept;
use crate::ui::home::Home;
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;
use crate::ui::training::{TrainingProgress, TrainingView};

/// How often the band follows the settings while it is being previewed:
/// often enough to sound live, seldom enough that a slider dragged across
/// the noise does not rebuild the band, and measure its AGC, every frame.
pub(crate) const BAND_FOLLOW_MS: u32 = 80;

/// How many times a follow measures the AGC's knee before it takes the
/// settings waiting: once for them, and again for a move that changed the
/// band while it measured, but no more than this, so a slider dragged
/// without a pause still has the band follow it.
///
/// Once in the browser, which measures on the page's own thread wherever it
/// does: measuring again for a newer move takes no work off the page, as the
/// band built for that move would measure the same knee on the same thread,
/// and under a drag that moves through every measurement it only adds
/// measurements of moves the drag has already left behind.
pub(crate) const BAND_WARM_ROUNDS: usize = if cfg!(target_arch = "wasm32") { 1 } else { 3 };

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
    // The same setting the Your station card writes, kept the same way.
    let on_call = use_callback(move |typed: String| {
        let mut settings = ctx.settings;
        settings.write().contest.set_my_call(&typed);
    });
    let on_name = use_callback(move |typed: String| {
        let mut settings = ctx.settings;
        settings.write().contest.set_my_name(&typed);
    });
    let on_state = use_callback(move |typed: String| {
        let mut settings = ctx.settings;
        settings.write().contest.set_my_state(&typed);
    });
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
            on_call,
            on_name,
            on_state,
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
///
/// Before a follow takes them, it measures the AGC's knee for the settings
/// waiting ([`warm_agc_knee`]), so building the band finds it remembered
/// rather than measuring it under the slider being dragged. The settings
/// stay waiting meanwhile: a move made while the knee is measured joins
/// them instead of starting a second follow, so there is only ever one on
/// its way and the newest settings are still the last to land. A move that
/// changed the band is measured in its turn before the follow takes it, up
/// to [`BAND_WARM_ROUNDS`] measurements in all, so a slider being dragged
/// finds its knee remembered too.
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
            for _ in 0..BAND_WARM_ROUNDS {
                // Copied out in a statement of its own: a borrow still held
                // over the await below is the one the next slider move would
                // panic on.
                let waiting = latest.borrow().clone();
                let Some(waiting) = waiting else {
                    break;
                };
                warm_agc_knee(&waiting).await;
                // The knee is asked of the band settings alone, the receiver
                // being worked out from them too, so only a move that changed
                // them asks it anew. A stopped preview has no band left to
                // build, so nothing to measure for.
                let moved =
                    latest.borrow().as_ref().map(|newest| &newest.band) != Some(&waiting.band);
                if !moved || !ctx.preview.peek().is_band() {
                    break;
                }
            }
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

/// Measure the AGC knee a band built for `settings` would set, so that
/// building it finds the answer remembered ([`agc_knee`]) instead of
/// spending three seconds of band on it there and then.
///
/// It is the question both players ask of the settings a follow hands them:
/// the band they describe, through the receiver they describe. The native
/// player clamps them once more first, which moves nothing the knee is
/// remembered by. It is asked only of a band there will be. The receiver is
/// worked out here, on the thread that draws the band card: the card has
/// just asked for the same one, and asking for it anywhere else would have
/// the two keep forgetting each other's. Should the settings move after the
/// follow's last measurement, the band measures its own knee, as it always
/// did.
async fn warm_agc_knee(settings: &TrainingSettings) {
    if !settings.band.has_background() {
        return;
    }
    let receiver = FilterDesign::from_settings(settings);
    // A worker measures it, and the slider is not held up at all. A worker
    // that panicked has measured nothing, which leaves it to the band.
    #[cfg(all(not(target_arch = "wasm32"), not(test)))]
    {
        let band = settings.band.clone();
        let _ = tokio::task::spawn_blocking(move || agc_knee(&band, &receiver)).await;
    }
    // The browser has the one thread. It measures here, then lets the page
    // draw and take input before the band is built: two short tasks rather
    // than one long one. Tests measure the same way: their clock is stepped
    // by hand, and keeps stepping while a worker runs, so a worker's knee
    // would land a different number of steps later every run.
    #[cfg(any(target_arch = "wasm32", test))]
    {
        agc_knee(&settings.band, &receiver);
        sleep_ms(1).await;
    }
}

#[component]
fn StatsRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    rsx! {
        StatsView {
            settings: ctx.settings.read().clone(),
            sessions: ctx.sessions.read().clone(),
            on_delete: move |picked: Vec<SessionPick>| ctx.delete_sessions(&picked),
            backups: Backups {
                last: *ctx.last_backup.read(),
                can_save: ctx.app().store.saves_backups(),
                on_save: EventHandler::new(move |()| ctx.save_backup()),
                on_restore: EventHandler::new(move |backup| ctx.restore_backup(backup)),
            },
        }
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
    let (can_float, is_floating, on_float, float_locked, on_float_lock) = (
        true,
        floating.is_open(),
        EventHandler::new(move |()| floating.toggle()),
        floating.is_locked(),
        EventHandler::new(move |()| floating.toggle_lock()),
    );
    #[cfg(not(feature = "desktop"))]
    let (can_float, is_floating, on_float, float_locked, on_float_lock) = (
        false,
        false,
        EventHandler::new(|()| {}),
        false,
        EventHandler::new(|()| {}),
    );
    rsx! {
        ListenView {
            can_float,
            floating: is_floating,
            on_float,
            float_locked,
            on_float_lock,
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
    // A contest run has a screen of its own, which reads the run itself.
    let contest = use_memo(move || ctx.contest.read().is_some());
    let on_change = use_callback(move |(index, text): (usize, String)| {
        send_event(ctx, SessionEvent::Input { index, text });
    });
    let on_confirm = use_callback(move |()| send_event(ctx, SessionEvent::Confirm));
    let on_submit = use_callback(move |()| send_event(ctx, SessionEvent::FinishNow));
    let on_stop = use_callback(move |()| send_event(ctx, SessionEvent::Abort));
    if contest() {
        return rsx! { ContestRunView {} };
    }
    let runtime = ctx.runtime.read();
    let Some(session) = runtime.as_ref() else {
        return rsx! { p { class: "muted", "Starting…" } };
    };
    let view = session.view();
    rsx! {
        div { class: "stack train",
            TrainingProgress {
                focused: view.focused,
                total: view.sent.len(),
                playing: view.playing,
                repeat_total: view.repeat_total,
                repeat_done: view.repeat_done,
                countdown: view.countdown,
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
                countdown: view.countdown,
                speed: view.speed,
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
    let result = match ctx.result.read().clone() {
        Some(Finished::Kept(result)) => *result,
        Some(Finished::NothingKept) => return rsx! { NothingKept { on_again, on_home } },
        None => return rsx! { p { class: "muted", "No result." } },
    };
    let place = cw_core::board_place(&ctx.sessions.read(), result.timestamp);
    rsx! {
        ResultsView {
            result,
            place,
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
