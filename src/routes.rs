//! One component per screen, each reading the state it shows and nothing
//! more. The shell above them redraws for its own state only, and a screen
//! redraws for its own: a toast or a theme change does not rebuild the
//! practice screen's history, and a key in a training answer does not
//! rebuild the shell.

use std::cell::Cell;
use std::rc::Rc;

use cw_core::{CharSetMode, SessionEvent, compute_char_pool};
use dioxus::prelude::*;

use crate::page_hidden::{LISTEN_HIDDEN, SESSION_HIDDEN, SETTINGS_HIDDEN, use_page_hidden};
use crate::persist::auto_progress;
use crate::playback;
use crate::session_runtime::{TypedAnswer, send_event, start_session};
use crate::state::{AppCtx, Finished, Screen, SessionPick};
use crate::time::{local_date_string, mono_ms, sleep_ms};
use crate::ui::backup::Backups;
use crate::ui::contest::ContestRunView;
use crate::ui::contest_results::NothingKept;
use crate::ui::home::Home;
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
    // Read from the store, a file on desktop and phones: once for each
    // move of the settings, not on every redraw of the screen.
    let progress = use_memo(move || auto_progress(&*ctx.app().store, &ctx.settings.read()));
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
            auto_progress: progress(),
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

/// While the band preview runs, the band follows the settings once they
/// settle ([`BAND_SETTLE_MS`]), and at least every [`BAND_FOLLOW_MAX_MS`]
/// under a slider that never pauses. Building the band for new receiver
/// settings measures its AGC, on the thread that draws in the browser, so it
/// is done once a drag pauses, not under every step of it.
fn use_band_follow(ctx: AppCtx) {
    let waiting = use_hook(|| Rc::new(Cell::new(Waiting::default())));
    use_effect(move || {
        let previewing = ctx.preview.read().is_band();
        // Read to follow every move; the follow takes them as they stand.
        let _ = ctx.settings.read();
        let turn = waiting.get().turn.wrapping_add(1);
        if !previewing {
            // A stopped preview has no band to follow the settings with.
            waiting.set(Waiting { since: None, turn });
            return;
        }
        let now = mono_ms();
        let since = waiting.get().since.unwrap_or(now);
        waiting.set(Waiting {
            since: Some(since),
            turn,
        });
        let due = (now + BAND_SETTLE_MS).min(since + BAND_FOLLOW_MAX_MS);
        let waiting = waiting.clone();
        spawn(async move {
            sleep_ms(due.saturating_sub(now).try_into().unwrap_or(u32::MAX)).await;
            // A newer move has a follow of its own coming, due no later.
            if waiting.get().turn != turn || !ctx.preview.peek().is_band() {
                return;
            }
            waiting.set(Waiting { since: None, turn });
            let settings = ctx.settings.peek().clone().clamp();
            ctx.app().player.follow_band(&settings);
        });
    });
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
    let on_resume = use_callback(move |()| send_event(ctx, SessionEvent::Resume));
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
                countdown: view.countdown,
                paused: view.paused,
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
