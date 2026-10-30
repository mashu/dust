//! One component per screen, each reading the state it shows and nothing
//! more. The shell above them redraws for its own state only, and a screen
//! redraws for its own: a toast or a theme change does not rebuild the
//! practice screen's history, and a key in a training answer does not
//! rebuild the shell.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cw_core::{CharSetMode, SessionEvent, TrainingSettings, compute_char_pool};
use dioxus::prelude::*;

use crate::page_hidden::{LISTEN_HIDDEN, SESSION_HIDDEN, SETTINGS_HIDDEN, use_page_hidden};
use crate::persist::auto_progress;
use crate::playback::{self, Preview, STREAM_PLAYS_OUT_OF_SIGHT};
use crate::session_runtime::{TypedAnswer, resume_session, send_event, start_session};
use crate::state::{AppCtx, Finished, Screen, SessionPick};
use crate::time::{local_date_string, mono_ms, sleep_ms};
use crate::ui::backup::Backups;
use crate::ui::contest::ContestRunView;
use crate::ui::contest_results::NothingKept;
use crate::ui::figures::use_history;
use crate::ui::home::{Home, Practice, lock_keeps_groups_untimed};
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;
use crate::ui::training::{TrainingProgress, TrainingView, focused_box_open};

/// How long the settings have to stay still, while the band is being
/// previewed, before the band follows them: past the steps of a slider
/// being dragged, so the band is rebuilt, and its AGC measured, once the
/// drag pauses rather than on every step of it.
pub(crate) const BAND_SETTLE_MS: u64 = 150;

/// The longest the band is left behind a slider that never pauses: it
/// follows the settings as they stand this long after the first move it has
/// not followed yet, so a long drag is still heard as it goes.
pub(crate) const BAND_FOLLOW_MAX_MS: u64 = 600;

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
    // Asked once on this install, whichever way it is answered; the lock
    // turned off is stored at once, as a setting changed off the settings
    // screen is.
    let mut asked_about_lock = use_signal(|| ctx.app().store.asked_about_lock());
    let on_lock_answer = use_callback(move |type_along: bool| {
        ctx.app().store.mark_asked_about_lock();
        asked_about_lock.set(true);
        if type_along {
            let mut settings = ctx.settings;
            settings.write().playback.lock_input_during_group_playback = false;
        }
    });
    // Read from the store, a file on desktop and phones: once for each
    // move of the settings, not on every redraw of the screen.
    let progress = use_memo(move || auto_progress(&*ctx.app().store, &ctx.settings.read()));
    use_warm_sampler(ctx);
    // What Home shows of the history: once for each change to it, and each
    // day, read from what the screens share of it; the screen opened again
    // on the same history works nothing out again.
    let history = use_history(ctx);
    let today = local_date_string();
    let practice = use_memo(use_reactive!(|today| Practice::of(&history(), &today)));
    let settings = ctx.settings.read();
    rsx! {
        Home {
            settings: settings.clone(),
            practice: practice(),
            pool: compute_char_pool(&settings).into_iter().collect::<String>(),
            auto_progress: progress(),
            on_start,
            on_listen,
            on_call,
            on_name,
            on_state,
            on_zone,
            on_section,
            ask_about_lock: !asked_about_lock() && lock_keeps_groups_untimed(&settings),
            on_lock_answer,
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
    // Hear a QSO starts from the prior, built once the screen is drawn.
    use_warm_sampler(ctx);
    use_band_follow(ctx);
    use_qsos_follow(ctx);
    use_preview_stop_when_hidden(ctx, SETTINGS_HIDDEN);
    use_lock_chosen(ctx);
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

/// Lock typing while sending switched on the settings screen is a choice,
/// and Home need not ask whether to keep it ([`crate::persist::Store::asked_about_lock`]).
fn use_lock_chosen(ctx: AppCtx) {
    let opened_with = use_hook(|| {
        ctx.settings
            .peek()
            .playback
            .lock_input_during_group_playback
    });
    let mut marked = use_hook(|| CopyValue::new(false));
    use_effect(move || {
        let locked = ctx
            .settings
            .read()
            .playback
            .lock_input_during_group_playback;
        if locked != opened_with && !*marked.peek() {
            marked.set(true);
            ctx.app().store.mark_asked_about_lock();
        }
    });
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

/// While a screen that plays anything is up, the page going out of sight
/// stops what it plays, as it pauses a run: the band, letters or QSOs
/// played to nobody are only to be heard again, and stopping is all a
/// preview needs. The stream of groups plays on where copying it out of
/// sight is what it is for ([`STREAM_PLAYS_OUT_OF_SIGHT`]).
fn use_preview_stop_when_hidden(ctx: AppCtx, slot: &'static str) {
    use_page_hidden(slot, move || {
        let preview = ctx.preview.peek().clone();
        let plays_on = preview.is_stream() && STREAM_PLAYS_OUT_OF_SIGHT;
        if preview != Preview::Idle && !plays_on {
            ctx.stop_audio();
        }
    });
}

/// While a screen with a session, a run or a stream a tap away is up, the
/// prior the sampler starts from is built once it is drawn, and again
/// whenever the history changes under it, rather than in the tap
/// ([`AppCtx::warm_sampler`]).
fn use_warm_sampler(ctx: AppCtx) {
    use_effect(move || {
        // Read to build it again for every change of the history.
        let _ = ctx.sessions.read();
        ctx.warm_sampler();
    });
}

/// While the band preview runs, the band follows the settings once they
/// settle ([`BAND_SETTLE_MS`]), and at least every [`BAND_FOLLOW_MAX_MS`]
/// under a slider that never pauses. Building the band for new receiver
/// settings measures its AGC, on the thread that draws in the browser, so it
/// is done once a drag pauses, not under every step of it. A move of the
/// volume or the RF gain alone builds nothing, and is followed at once.
fn use_band_follow(ctx: AppCtx) {
    let waiting = use_hook(|| Rc::new(Cell::new(Waiting::default())));
    // The settings the band was last brought in line with, clamped.
    let followed = use_hook(|| Rc::new(RefCell::new(None::<TrainingSettings>)));
    use_effect(move || {
        let previewing = ctx.preview.read().is_band();
        // Read to follow every move; the follow takes them as they stand.
        let settings = ctx.settings.read();
        let turn = waiting.get().turn.wrapping_add(1);
        if !previewing {
            // A stopped preview has no band to follow the settings with.
            waiting.set(Waiting { since: None, turn });
            followed.borrow_mut().take();
            return;
        }
        let clamped = settings.clone().clamp();
        drop(settings);
        let gains_only = followed
            .borrow()
            .as_ref()
            .is_some_and(|last| only_gains_moved(last, &clamped));
        if gains_only {
            ctx.app().player.follow_band(&clamped);
            *followed.borrow_mut() = Some(clamped);
            return;
        }
        let now = mono_ms();
        let since = waiting.get().since.unwrap_or(now);
        waiting.set(Waiting {
            since: Some(since),
            turn,
        });
        let due = (now + BAND_SETTLE_MS).min(since + BAND_FOLLOW_MAX_MS);
        let (waiting, followed) = (waiting.clone(), followed.clone());
        spawn(async move {
            sleep_ms(due.saturating_sub(now).try_into().unwrap_or(u32::MAX)).await;
            // A newer move has a follow of its own coming, due no later.
            if waiting.get().turn != turn || !ctx.preview.peek().is_band() {
                return;
            }
            waiting.set(Waiting { since: None, turn });
            let settings = ctx.settings.peek().clone().clamp();
            ctx.app().player.follow_band(&settings);
            *followed.borrow_mut() = Some(settings);
        });
    });
}

/// Whether `now` is `last` but for the volume and the RF gain, which the
/// player moves a sample at a time without building anything.
fn only_gains_moved(last: &TrainingSettings, now: &TrainingSettings) -> bool {
    let mut gains_put_back = now.clone();
    gains_put_back.band.master_volume = last.band.master_volume;
    gains_put_back.band.rf_gain_db = last.band.rf_gain_db;
    gains_put_back == *last && now != last
}

/// The settings moves the band has still to follow: since when, and which
/// was the newest, so only the follow for it brings the band in line.
#[derive(Clone, Copy, Default)]
struct Waiting {
    since: Option<u64>,
    turn: u64,
}

#[component]
fn StatsRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    // The history as the screens share it, never copied, with what was
    // worked out of it when Stats was last open.
    let history = use_history(ctx);
    rsx! {
        StatsView {
            settings: ctx.settings.read().clone(),
            sessions: history(),
            on_delete: move |picked: Vec<SessionPick>| ctx.delete_sessions(&picked),
            backups: Backups {
                last: *ctx.last_backup.read(),
                can_save: ctx.app().store.saves_backups(),
                confirmed: ctx.app().store.confirms_backups(),
                inside_app: ctx.app().store.backups_inside_app(),
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
    use_preview_stop_when_hidden(ctx, LISTEN_HIDDEN);
    use_warm_sampler(ctx);
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
    let on_resume = use_callback(move |()| resume_session(ctx));
    // A session of groups holds while nobody can hear it, as a run does,
    // rather than close answers nobody is there to type. A run has a
    // listener of its own; this one says nothing to it.
    use_page_hidden(SESSION_HIDDEN, move || send_event(ctx, SessionEvent::Pause));
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
                paused: view.paused,
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
                again: view.again,
                countdown: view.countdown,
                countdown_to: view.countdown_to,
                paused: view.paused,
                sound_stopped: view.sound_stopped,
                held: view.held,
                speed: view.speed,
                answer: ctx.answer,
                on_change,
                on_confirm,
                on_submit,
                on_stop,
                on_resume,
            }
        }
    }
}

#[component]
fn ResultsRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    let on_again = use_callback(move |()| start_session(ctx));
    let on_home = use_callback(move |()| ctx.go_to(Screen::Home));
    // Stored at once, as a setting changed off the settings screen is.
    let on_unlock = use_callback(move |()| {
        ctx.app().store.mark_asked_about_lock();
        let mut settings = ctx.settings;
        settings.write().playback.lock_input_during_group_playback = false;
    });
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
            on_unlock: ctx
                .settings
                .read()
                .playback
                .lock_input_during_group_playback
                .then_some(on_unlock),
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

    /// The page going out of sight stops the band preview, as it stops
    /// Hear a QSO: nothing plays on to nobody, the screen left to dim
    /// among it.
    #[test]
    fn hiding_the_page_stops_the_band_preview() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            ui.with_app(|_| crate::page_hidden::hide_page());
            assert!(ui.has("Live preview"), "still previewing: {}", ui.html());
            let sent = recorder.texts().len();
            ui.advance(3_000).await;
            assert_eq!(recorder.texts().len(), sent, "the band played on");
        });
    }

    /// The stream of groups plays on out of sight on the desktop and in a
    /// browser, where copying it while you do something else is what it is
    /// for; a phone's stops, as everything else that plays does.
    #[test]
    fn hiding_the_page_stops_the_stream_only_on_a_phone() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            ui.click("btn-stream-groups");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            ui.with_app(|_| crate::page_hidden::hide_page());
            assert_eq!(ui.has("Stream groups"), !STREAM_PLAYS_OUT_OF_SIGHT);
            let sent = recorder.texts().len();
            ui.advance(10_000).await;
            assert_eq!(
                recorder.texts().len() > sent,
                STREAM_PLAYS_OUT_OF_SIGHT,
                "{:?}",
                recorder.texts()
            );
        });
    }

    /// The page going out of sight pauses a session of groups on the
    /// training screen, and Resume carries it on.
    #[test]
    fn hiding_the_page_pauses_a_session() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-start-training");
            ui.advance(500).await;
            ui.with_app(|_| crate::page_hidden::hide_page());
            let paused = ui.with_app(|ctx| ctx.runtime.peek().as_ref().map(|s| s.paused()));
            assert_eq!(paused, Some(true));
            ui.click("btn-resume");
            let paused = ui.with_app(|ctx| ctx.runtime.peek().as_ref().map(|s| s.paused()));
            assert_eq!(paused, Some(false));
        });
    }
}
