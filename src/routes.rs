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
use crate::persist::record::RecordId;
use crate::playback::{self, Preview, STREAM_PLAYS_OUT_OF_SIGHT};
use crate::session_runtime::{TypedAnswer, resume_session, send_event, start_session};
use crate::state::SyncStatus;
use crate::state::{AppCtx, Finished, Screen};
use crate::time::{mono_ms, sleep_ms};
use crate::ui::backup::Backups;
use crate::ui::contest::ContestRunView;
use crate::ui::contest_results::NothingKept;
use crate::ui::figures::use_history;
use crate::ui::home::{Home, Practice};
use crate::ui::listen::ListenView;
use crate::ui::results::ResultsView;
use crate::ui::settings::SettingsView;
use crate::ui::stats::StatsView;
use crate::ui::sync::SyncView;
use crate::ui::today::use_today;
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

/// Where Home, and Stats, keep their listeners for the page coming back
/// into sight, to read the day again.
const HOME_SHOWN: &str = "__dustHomeShown";
const STATS_SHOWN: &str = "__dustStatsShown";

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
    use_warm_sampler(ctx);
    // What Home shows of the history: once for each change to it, and each
    // day, read from what the screens share of it; the screen opened again
    // on the same history works nothing out again. What another copy of
    // the app stored or deleted since is taken up as it opens.
    use_hook(move || ctx.take_up_changes());
    let history = use_history(ctx);
    let today = use_today(HOME_SHOWN);
    let practice = use_memo(move || Practice::of(&history(), &today.read()));
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
/// ([`AppCtx::warm_sampler`]). A change is one to its sessions
/// ([`crate::history::StoredHistory::generation`]): what else is written
/// of the history, as the store saying it took a session, changes none.
fn use_warm_sampler(ctx: AppCtx) {
    let generation = use_memo(move || ctx.sessions.read().generation());
    use_effect(move || {
        // Read to build it again for every change of the sessions.
        let _ = generation();
        #[cfg(test)]
        crate::testing::rendered("warm sampler");
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
        let held = waiting.get();
        if !previewing {
            // A stopped preview has no band to follow the settings with.
            waiting.set(Waiting {
                since: None,
                ..held
            });
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
        // Each move puts the follow off, to the longest after the first.
        let now = mono_ms();
        let since = held.since.unwrap_or(now);
        waiting.set(Waiting {
            since: Some(since),
            due: (now + BAND_SETTLE_MS).min(since + BAND_FOLLOW_MAX_MS),
            waits: true,
        });
        if held.waits {
            return;
        }
        let (waiting, followed) = (waiting.clone(), followed.clone());
        #[cfg(test)]
        crate::testing::rendered("band wait");
        spawn(async move {
            // Until the follow is due, however far the moves meanwhile put
            // it off.
            loop {
                let (due, now) = (waiting.get().due, mono_ms());
                if now >= due {
                    break;
                }
                sleep_ms((due - now).try_into().unwrap_or(u32::MAX)).await;
            }
            let held = waiting.replace(Waiting::default());
            // Stopped meanwhile, the preview has no band to follow.
            if held.since.is_none() || !ctx.preview.peek().is_band() {
                return;
            }
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

/// The settings moves the band has still to follow: since when, and when it
/// follows them, with whether a wait for that is under way: one wait for
/// every move until the band follows, each move putting it off.
#[derive(Clone, Copy, Default)]
struct Waiting {
    since: Option<u64>,
    due: u64,
    waits: bool,
}

#[component]
fn StatsRoute() -> Element {
    let ctx = use_context::<AppCtx>();
    // The history as the screens share it, never copied, with what was
    // worked out of it when Stats was last open, and what another copy of
    // the app stored or deleted since taken up as it opens.
    use_hook(move || ctx.take_up_changes());
    let history = use_history(ctx);
    // The Scores tab says how long ago the best was set, and the sync card
    // when this device last synced: by the day as it is, past midnight too.
    let today = use_today(STATS_SHOWN);
    // Made once, as the sync card's are: the History tab, handed the same
    // ones, is drawn again only for what it shows.
    let on_save = use_callback(move |()| ctx.save_backup());
    let sync = use_sync_view(ctx, today());
    rsx! {
        StatsView {
            settings: ctx.settings.read().clone(),
            sessions: history(),
            today: today(),
            on_delete: move |picked: Vec<RecordId>| ctx.delete_sessions(&picked),
            backups: Backups {
                last: *ctx.last_backup.read(),
                can_save: ctx.app().store.saves_backups(),
                confirmed: ctx.app().store.confirms_backups(),
                inside_app: ctx.app().store.backups_inside_app(),
                outgrows_device_backup: ctx.app().store.outgrows_device_backup(),
                on_save,
            },
            sync,
        }
    }
}

/// What the History tab's sync card shows and does, on `today`: no card
/// where this build cannot sync at all. What it does is made once, for as
/// long as the screen is up.
fn use_sync_view(ctx: AppCtx, today: String) -> Option<SyncView> {
    let on_sign_in = use_callback(move |()| ctx.sync_sign_in());
    let on_passphrase = use_callback(move |(passphrase, repeated)| {
        ctx.sync_passphrase(passphrase, repeated);
    });
    let on_cancel = use_callback(move |()| ctx.sync_cancel());
    let on_sync_now = use_callback(move |()| ctx.sync_now());
    let on_turn_off = use_callback(move |and_copy| ctx.sync_turn_off(and_copy));
    let status = ctx.sync.status.read().clone();
    if status == SyncStatus::Unavailable(None) {
        return None;
    }
    Some(SyncView {
        status,
        today,
        on_sign_in,
        on_passphrase,
        on_cancel,
        on_sync_now,
        on_turn_off,
    })
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
    // Where the session stands on the board reads the history, every run's
    // log for a run: worked out once for it, not on every redraw.
    let history = use_history(ctx);
    let on_again = use_callback(move |()| start_session(ctx));
    let on_home = use_callback(move |()| ctx.go_to(Screen::Home));
    // Stored at once, as a setting changed off the settings screen is.
    let on_unlock = use_callback(move |()| {
        let mut settings = ctx.settings;
        settings.write().playback.lock_input_during_group_playback = false;
    });
    let (result, kept) = match ctx.result.read().clone() {
        Some(Finished::Kept(result, kept)) => (*result, kept),
        Some(Finished::NothingKept) => return rsx! { NothingKept { on_again, on_home } },
        None => return rsx! { p { class: "muted", "No result." } },
    };
    // Found by its record, not by when it finished, which another session
    // can share: no place where it is in no history.
    let history = history();
    let at = kept.and_then(|id| history.index_of(id));
    let place = *history.figure("board place", at, |sessions| {
        at.and_then(|index| cw_core::board_place_at(sessions, index))
    });
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
    use crate::testing::{Ui, run, stored_session, test_settings};
    use crate::ui::today::{DAY_LOOK_MS, days_after, pretend_today};

    /// Home left up past midnight goes by the new day: its streak and its
    /// calendar take it once the clock passes midnight, and as the page
    /// comes back into sight. Home read the day only as it was drawn, and an
    /// app left open overnight said the next morning that the day before's
    /// practice was the day's, the streak safe.
    #[test]
    fn home_left_up_past_midnight_goes_by_the_new_day() {
        run(|| async {
            pretend_today("2026-09-02");
            let sessions: Vec<_> = (1..=2)
                .map(|day| {
                    let mut session = stored_session(&format!("2026-09-0{day}"), &[("KM", "KM")]);
                    session.timestamp = day * 86_400_000;
                    session
                })
                .collect();
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &sessions);
            assert!(
                ui.has("2-day streak — today is in the bag."),
                "{}",
                ui.html()
            );
            pretend_today("2026-09-03");
            tokio::time::advance(std::time::Duration::from_millis(DAY_LOOK_MS.into())).await;
            ui.advance(20).await;
            assert!(
                ui.has("2-day streak at risk — practice today to keep it alive."),
                "{}",
                ui.html()
            );
            pretend_today("2026-09-04");
            ui.with_app(|_| crate::page_hidden::show_page());
            assert!(
                ui.has("Your 2-day streak ended — one session today starts the next one."),
                "{}",
                ui.html()
            );
        });
    }

    /// The Scores tab left up past midnight says how long ago the best was
    /// set by the new day: once the clock passes midnight, and as the page
    /// comes back into sight. It read the day only as it was drawn, and
    /// went on saying a best set the day before was set today.
    #[test]
    fn the_scores_tab_left_up_past_midnight_goes_by_the_new_day() {
        run(|| async {
            let at = crate::time::now_ms();
            let day = crate::format::day_of(at).expect("a day");
            pretend_today(&day);
            let mut best = stored_session(&day, &[("KM", "KM"); 10]);
            best.timestamp = at;
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &[best]);
            ui.click("nav-stats");
            ui.click("seg-scores");
            assert!(ui.has(" · today<"), "{}", ui.html());
            pretend_today(&days_after(&day, 1));
            tokio::time::advance(std::time::Duration::from_millis(DAY_LOOK_MS.into())).await;
            ui.advance(20).await;
            assert!(ui.has(" · yesterday<"), "{}", ui.html());
            pretend_today(&days_after(&day, 2));
            ui.with_app(|_| crate::page_hidden::show_page());
            assert!(ui.has(" · 2 days ago<"), "{}", ui.html());
        });
    }

    /// The sync card left up past midnight says when this device last
    /// synced by the new day: once the clock passes midnight, and as the
    /// page comes back into sight. It read the clock only as it was drawn,
    /// and went on saying a sync made the day before was made today.
    #[test]
    fn the_sync_card_left_up_past_midnight_goes_by_the_new_day() {
        run(|| async {
            let at = crate::time::now_ms();
            let day = crate::format::day_of(at).expect("a day");
            pretend_today(&day);
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.with_app(|ctx| {
                let mut status = ctx.sync.status;
                status.set(SyncStatus::On {
                    last: Some(at),
                    syncing: false,
                    problem: None,
                    needs_sign_in: false,
                });
            });
            ui.click("nav-stats");
            ui.click("seg-history");
            let time = crate::format::clock(at).expect("a time");
            assert!(ui.has(&format!("Synced today at {time}")), "{}", ui.html());
            pretend_today(&days_after(&day, 1));
            tokio::time::advance(std::time::Duration::from_millis(DAY_LOOK_MS.into())).await;
            ui.advance(20).await;
            let yesterday = format!("Synced yesterday at {time}");
            assert!(ui.has(&yesterday), "{}", ui.html());
            pretend_today(&days_after(&day, 2));
            ui.with_app(|_| crate::page_hidden::show_page());
            let before = format!("Synced {} at {time}", crate::format::short_date(&day));
            assert!(ui.has(&before), "{}", ui.html());
        });
    }

    /// A screen with nothing behind it yet: the moment between starting a
    /// session and its first group, or a results screen with nothing to show.
    #[component]
    fn EmptyRoute(screen: Screen) -> Element {
        use_context_provider(|| AppCtx::new(AppState::new(), test_settings(), Default::default()));
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

    /// The results of a session just finished work out where it stands on
    /// the board once for the history: drawn again for a setting moved, or
    /// for the store saying it took the session, they find it worked out;
    /// another session kept is another history, and it is worked out again.
    #[test]
    fn a_sessions_results_work_out_their_place_once() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let (mut ui, recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(ui.run_until(5_000, |ui| ui.screen() == "results").await);
            ui.advance(200).await;
            assert!(ui.take_worked_out("board place") >= 1);
            ui.with_app(|ctx| {
                let mut settings = ctx.settings;
                settings.write().band.master_volume = 0.5;
            });
            ui.with_app(|ctx| {
                let mut sessions = ctx.sessions;
                let first = sessions.peek().id_at(0).expect("the session kept");
                sessions.write().mark_saved(first);
            });
            assert_eq!(ui.screen(), "results");
            assert_eq!(ui.take_worked_out("board place"), 0);
            ui.with_app(|ctx| {
                let mut sessions = ctx.sessions;
                let another = crate::testing::stored_session("2026-10-01", &[("KM", "KM")]);
                sessions.write().push(another);
            });
            assert_eq!(ui.take_worked_out("board place"), 1);
        });
    }

    /// The results of a session just finished place it as it stands on the
    /// board, though another in the history finished at the same moment, as
    /// two devices' sessions can once synced: it is found by its record,
    /// not by when it finished. It was handed the place of the first in the
    /// history to finish then.
    #[test]
    fn a_sessions_results_place_it_beside_another_finished_at_the_same_moment() {
        run(|| async {
            let at = crate::time::now_ms();
            let mut best = stored_session("2026-10-01", &[("KM", "KM"); 10]);
            best.timestamp = at;
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &[best]);
            let copied: Vec<_> = [("KM", "KM"); 5]
                .into_iter()
                .chain([("KM", "MK"); 5])
                .collect();
            let mut worse = stored_session("2026-10-01", &copied);
            worse.timestamp = at;
            ui.with_app(|ctx| crate::session_runtime::keep_and_show(ctx, worse));
            assert_eq!(ui.screen(), "results");
            assert!(ui.has("2nd best of your 2 Koch sessions"), "{}", ui.html());
        });
    }

    /// The store saying it took a session writes the history and changes
    /// none of its sessions: the sampler's prior is not looked at again for
    /// it, every session keyed, as it is for a session added. It was, on
    /// every screen a session or a run is a tap away from, for every word
    /// the store sent.
    #[test]
    fn a_history_written_with_no_session_changed_warms_no_sampler() {
        run(|| async {
            let first = stored_session("2026-10-01", &[("KM", "KM")]);
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &[first]);
            ui.advance(100).await;
            ui.take_renders("warm sampler");
            ui.with_app(|ctx| {
                let mut sessions = ctx.sessions;
                let first = sessions.peek().id_at(0).expect("the session kept");
                sessions.write().mark_saved(first);
            });
            ui.advance(100).await;
            assert_eq!(ui.take_renders("warm sampler"), 0);
            ui.with_app(|ctx| {
                let mut sessions = ctx.sessions;
                let another = stored_session("2026-10-02", &[("KM", "KM")]);
                sessions.write().push(another);
            });
            ui.advance(100).await;
            assert_eq!(ui.take_renders("warm sampler"), 1);
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
