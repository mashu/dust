//! One component per screen, each reading the state it shows and nothing
//! more. The shell above them redraws for its own state only, and a screen
//! redraws for its own: a toast or a theme change does not rebuild the
//! practice screen's history, and a key in a training answer does not
//! rebuild the shell.

use std::cell::RefCell;
use std::rc::Rc;

use cw_core::band::{agc_knee, loudest_station, remembered_agc_knee};
use cw_core::{CharSetMode, SessionEvent, TrainingSettings, compute_char_pool};
use dioxus::prelude::*;

use crate::page_hidden::{LISTEN_HIDDEN, SETTINGS_HIDDEN, use_page_hidden};
use crate::persist::auto_progress;
use crate::playback;
use crate::session_runtime::{TypedAnswer, send_event, start_session};
use crate::state::{AppCtx, Finished, Screen, SessionPick};
use crate::time::{local_date_string, sleep_ms};
use crate::ui::backup::Backups;
use crate::ui::contest::ContestRunView;
use crate::ui::contest_results::NothingKept;
use crate::ui::home::Home;
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::scope::{BandKnee, knee_asked};
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;
use crate::ui::training::{TrainingProgress, TrainingView, focused_box_open};

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
    let on_zone = use_callback(move |typed: String| {
        let mut settings = ctx.settings;
        settings.write().contest.set_my_zone(&typed);
    });
    let on_section = use_callback(move |typed: String| {
        let mut settings = ctx.settings;
        settings.write().contest.set_my_section(&typed);
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
            on_zone,
            on_section,
        }
    }
}

#[component]
fn SettingsRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    let on_preview_band = use_callback(move |()| playback::start_band_preview(ctx));
    let on_stop_band = use_callback(move |()| ctx.stop_audio());
    let on_play_sample = use_callback(move |text| playback::start_sample(ctx, text));
    let on_hear_qsos = use_callback(move |()| playback::start_qsos(ctx));
    use_band_follow(ctx);
    use_qsos_follow(ctx);
    use_qsos_stop_when_hidden(ctx, SETTINGS_HIDDEN);
    let preview = ctx.preview.read();
    rsx! {
        SettingsView {
            settings: ctx.settings,
            previewing: preview.is_band(),
            sample_playing: preview.sample(),
            on_preview_band,
            on_stop_band,
            on_play_sample,
            hearing: preview.is_qsos(),
            dialogue: ctx.dialogue.read().clone(),
            on_hear_qsos,
        }
    }
}

/// Hear a QSO is of the contest it was started from: once the settings
/// move off it, to another contest or out of Contest mode, it stops and its
/// lines go, so what plays is always the contest the card shows, and Stop
/// is never left off the screen while it plays.
fn use_qsos_follow(ctx: AppCtx) {
    use_effect(move || {
        let chosen = {
            let settings = ctx.settings.read();
            (settings.curriculum.char_set_mode == CharSetMode::Contest)
                .then_some(settings.contest.contest_kind)
        };
        let mut dialogue = ctx.dialogue;
        let of = dialogue.peek().as_ref().map(|dialogue| dialogue.kind);
        if of.is_some_and(|kind| Some(kind) != chosen) {
            if ctx.preview.peek().is_qsos() {
                ctx.stop_audio();
            }
            dialogue.set(None);
        }
    });
}

/// While a screen that plays Hear a QSO is up, the page going out of sight
/// stops it, as it pauses a run: QSOs played to nobody are only to be heard
/// again, and stopping is all a demonstration needs.
fn use_qsos_stop_when_hidden(ctx: AppCtx, slot: &'static str) {
    use_page_hidden(slot, move || {
        if ctx.preview.peek().is_qsos() {
            ctx.stop_audio();
        }
    });
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
///
/// The level scope draws its line by the knee of the band that plays
/// ([`BandKnee`]): each one measured, while the preview runs. Settings the
/// follow took unmeasured, moved in every measurement, have theirs from the
/// band built for them, which measured it with the AGC on; with it off they
/// wait to be measured in their turn, as a move does. A preview stopped
/// takes its knee with it.
fn use_band_follow(ctx: AppCtx) {
    let latest = use_hook(|| Rc::new(RefCell::new(None::<TrainingSettings>)));
    let BandKnee(mut band_knee) = use_context_provider(|| BandKnee(Signal::new(None)));
    let mut give_knee = move |knee: f64| {
        if *band_knee.peek() != Some(knee) {
            band_knee.set(Some(knee));
        }
    };
    use_effect(move || {
        if !ctx.preview.read().is_band() {
            // The next preview's scope waits for a knee of its own, not one
            // of settings that may have moved since.
            if band_knee.peek().is_some() {
                band_knee.set(None);
            }
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
            loop {
                // The band whose knee the scope was given last.
                let mut measured = None;
                for _ in 0..BAND_WARM_ROUNDS {
                    // Copied out in a statement of its own: a borrow still
                    // held over the await below is the one the next slider
                    // move would panic on.
                    let waiting = latest.borrow().clone();
                    let Some(waiting) = waiting else {
                        break;
                    };
                    let knee = warm_agc_knee(&waiting).await;
                    // A stopped preview has no band left to build, so
                    // nothing to measure for, and no scope to draw by it.
                    if !ctx.preview.peek().is_band() {
                        break;
                    }
                    if let Some(knee) = knee {
                        give_knee(knee);
                        measured = Some(waiting.band.clone());
                    }
                    // The knee is asked of the band settings alone, the
                    // receiver being worked out from them too, so only a
                    // move that changed them asks it anew.
                    let moved =
                        latest.borrow().as_ref().map(|newest| &newest.band) != Some(&waiting.band);
                    if !moved {
                        break;
                    }
                }
                let settings = latest.borrow_mut().take();
                // A preview stopped meanwhile stays silent.
                let Some(settings) = settings.filter(|_| ctx.preview.peek().is_band()) else {
                    return;
                };
                ctx.app().player.follow_band(&settings);
                if measured.as_ref() == Some(&settings.band) {
                    return;
                }
                let (on, receiver) = knee_asked(&settings);
                if let Some(knee) = remembered_agc_knee(&on, &receiver) {
                    give_knee(knee);
                    return;
                }
                if measured.is_none() {
                    // The worker measuring failed: the scope waits for the
                    // next move rather than drawing by another band's knee.
                    if band_knee.peek().is_some() {
                        band_knee.set(None);
                    }
                    return;
                }
                // Put back, with nothing awaited since they were taken, so
                // no second follow starts: measured next, and taken again,
                // which rebuilds nothing.
                *latest.borrow_mut() = Some(settings);
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
/// remembered by. The receiver is worked out here, on the thread that draws
/// the band card: the card has just asked for the same one, and asking for
/// it anywhere else would have the two keep forgetting each other's. Should
/// the settings move after the follow's last measurement, the band measures
/// its own knee, as it always did.
///
/// It is measured with the AGC on whether it is or not: one question either
/// way, the knee being remembered by the floor and the receiver alone, and
/// the one the level scope draws its line by, AGC on or off ([`BandKnee`]),
/// so that the scope never measures it on the thread that draws.
///
/// And with it, for a band with a background, how loud the strongest
/// station comes out of that receiver ([`loudest_station`]), which full
/// volume is sized by: a few milliseconds more, and asked of the knee just
/// measured. The knee, unless the worker measuring it failed.
async fn warm_agc_knee(settings: &TrainingSettings) -> Option<f64> {
    let (on, receiver) = knee_asked(settings);
    let band = settings.band.has_background().then(|| settings.clone());
    let measure = move || {
        let knee = agc_knee(&on, &receiver);
        if let Some(settings) = band {
            loudest_station(&settings);
        }
        knee
    };
    // A worker measures it, and the slider is not held up at all. A worker
    // that panicked has measured nothing, which leaves it to the band.
    #[cfg(all(not(target_arch = "wasm32"), not(test)))]
    {
        tokio::task::spawn_blocking(measure).await.ok().flatten()
    }
    // The browser has the one thread. It measures here, then lets the page
    // draw and take input before the band is built: two short tasks rather
    // than one long one. Tests measure the same way: their clock is stepped
    // by hand, and keeps stepping while a worker runs, so a worker's knee
    // would land a different number of steps later every run.
    #[cfg(any(target_arch = "wasm32", test))]
    {
        let knee = measure();
        sleep_ms(1).await;
        knee
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
    let on_hear = use_callback(move |()| playback::start_qsos(ctx));
    let mut dialogue = ctx.dialogue;
    let on_put_away = use_callback(move |()| dialogue.set(None));
    let on_stop = use_callback(move |()| ctx.stop_audio());
    let on_back = use_callback(move |()| ctx.go_to(Screen::Home));
    use_qsos_stop_when_hidden(ctx, LISTEN_HIDDEN);
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
            hearing: preview.is_qsos(),
            dialogue: ctx.dialogue.read().clone(),
            on_play,
            on_stream,
            on_hear,
            on_put_away,
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
    let on_change = use_callback(move |typed: TypedAnswer| send_event(ctx, typed.into()));
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
    let open = focused_box_open(view.current, view.focused, &view.confirmed);
    rsx! {
        div { class: "stack train",
            TrainingProgress {
                focused: view.focused,
                total: view.sent.len(),
                playing: view.playing,
                open,
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
            typing_locked: ctx.settings.read().playback.lock_input_during_group_playback,
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
