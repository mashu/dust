//! Runs a training session: the machine decides, and this carries out what
//! it decides — sends, timers, the results screen — under the session's
//! lease on the audio. The UI only hands it events.
//!
//! Its tasks belong to the app rather than to whichever screen started the
//! session, which is gone the moment the session's own screen comes up: they
//! are spawned on the root scope, and the lease, not a component, decides
//! how long they live.
//!
//! A contest run has a runtime of its own ([`crate::contest_runtime`]):
//! settings in Contest start one, and [`finish_run`] and [`abort_run`] reach
//! whichever is live.

use cw_core::{
    AnswerClose, CharSamplingState, CharSetMode, ContestEvent, CountersKey, CountersUpdate,
    KeyTimes, MonoMs, Sampler, SessionEffect, SessionEvent, SessionId, SessionMachine,
    SessionResult, SplitMix64, Transmission, complete_session, generate_training_group,
    plan_morse_playback_for, resolve_station,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use crate::contest_runtime::{LiveContest, send_run_event, start_run};
use crate::playback::Preview;
use crate::player::{AudioError, Lease};
use crate::state::{AppCtx, Finished, Screen};
use crate::time::{local_date_string, mono_ms, now_ms, seed_rng};

/// What has the training screen: a session of groups, or a contest run.
/// Each is a machine of some size, and only one is ever live, so each is
/// boxed.
pub enum Live {
    Groups(Box<LiveSession>),
    Contest(Box<LiveContest>),
}

impl Live {
    /// The session of groups, if that is what is live.
    fn groups(&self) -> Option<&LiveSession> {
        match self {
            Self::Groups(live) => Some(live.as_ref()),
            Self::Contest(_) => None,
        }
    }

    fn groups_mut(&mut self) -> Option<&mut LiveSession> {
        match self {
            Self::Groups(live) => Some(live.as_mut()),
            Self::Contest(_) => None,
        }
    }
}

/// A session in progress: the machine that decides it, the stations it is
/// sent by, and its hold on the audio.
pub struct LiveSession {
    machine: SessionMachine<Sampler>,
    /// Where every group's stations are drawn from. Drawn afresh for each
    /// session, so no two sessions — in one launch or across launches — hear
    /// the same operators.
    stations: SplitMix64,
    lease: Lease,
}

impl LiveSession {
    /// What the receiver hears for group `index`: the station being copied,
    /// and whoever else is calling across it.
    ///
    /// Both are drawn from generators that depend only on the session and
    /// the group, so every repeat of a group — and every retry after a
    /// stalled send — comes from the same operators, with the same pile-up
    /// around them: a station repeating its call does not change frequency,
    /// speed or strength between sends. Nothing has to be carried across the
    /// sends for that, while the next group is somebody else.
    ///
    /// The others send groups of their own, drawn from the same curriculum
    /// so they sound like stations rather than noise — but off a blank
    /// slate, so a pile-up never teaches the sampler anything. Only the
    /// station you answer counts.
    fn transmission(&self, index: usize, text: String) -> Transmission {
        let settings = self.machine.session().settings();
        let group = self.stations.fork(index as u64);
        let mut voice = resolve_station(settings, &mut group.fork(GroupStream::Voice as u64));
        // Adaptive speed chose this group's speed; the station keeps its own
        // tone, strength and fist.
        if let Some((char_wpm, effective_wpm)) = self.machine.session().planned_wpm(index) {
            voice.char_wpm = char_wpm;
            voice.effective_wpm = effective_wpm;
        }
        let mut elsewhere = CharSamplingState::default();
        Transmission::calling(
            settings,
            text,
            voice,
            &mut group.fork(GroupStream::Callers as u64),
            |rng| generate_training_group(settings, &mut elsewhere, rng),
        )
    }
}

/// The open answer box's text, on its way to the session.
///
/// The box tells the session what is typed once typing pauses, so a burst of
/// keys is one event rather than one per character. Until then the text waits
/// here, and the runtime hands it to the machine ahead of whatever comes next
/// — the answer window closing, a send ending, Enter, End session, Exit. So
/// nothing that moves the session on can overtake a keystroke, however close
/// the two land.
///
/// The box also has a say in what is still being entered and not in it yet:
/// a letter the paddles are keying. At a deadline it finishes what is keyed
/// in full; auto-confirm waits until nothing is under way.
#[derive(Clone, Copy, PartialEq)]
pub struct AnswerOutbox(CopyValue<Outbox>);

#[derive(Default)]
struct Outbox {
    /// Typed into the box for this group, and not handed over yet.
    waiting: Option<(usize, String)>,
    /// When each character typed into the box went in, key by key: the
    /// session hears the text only once typing pauses, and recognition is
    /// timed to the keys.
    keys: KeyTimes,
    entry: Option<Entry>,
}

/// What the box says about an entry still under way.
#[derive(Clone, Copy, PartialEq)]
struct Entry {
    /// Puts into the box whatever is entered in full but not there yet.
    finish: Callback,
    /// Whether anything is still being entered.
    open: Callback<(), bool>,
}

impl AnswerOutbox {
    pub fn new() -> Self {
        Self(CopyValue::new(Outbox::default()))
    }

    /// Group `index`'s box now holds `text`, not handed over yet.
    pub fn hold(mut self, index: usize, text: String) {
        let mut outbox = self.0.write();
        outbox.keys.typed(index, &text, MonoMs(mono_ms()));
        outbox.waiting = Some((index, text));
    }

    /// When each character typed into the box went in, and the group it is.
    fn keys(self) -> (usize, Vec<MonoMs>) {
        let outbox = self.0.peek();
        let (index, at) = outbox.keys.of();
        (index, at.to_vec())
    }

    /// Whatever is waiting, now handed over.
    pub fn take(mut self) -> Option<(usize, String)> {
        self.0.write().waiting.take()
    }

    /// While the calling component is mounted, `finish` puts into the box
    /// whatever is entered in full but not there yet, and `open` says
    /// whether anything is still being entered.
    pub fn use_entry(
        self,
        mut finish: impl FnMut() + 'static,
        mut open: impl FnMut() -> bool + 'static,
    ) {
        let entry = Entry {
            finish: use_callback(move |()| finish()),
            open: use_callback(move |()| open()),
        };
        use_hook(move || self.set_entry(Some(entry)));
        use_drop(move || {
            // Only its own: a box that has already taken over stays.
            if self.0.peek().entry == Some(entry) {
                self.set_entry(None);
            }
        });
    }

    fn set_entry(mut self, entry: Option<Entry>) {
        self.0.write().entry = entry;
    }

    /// Have the box finish what is entered in full. It may hand the session
    /// an answer of its own on the way.
    fn finish(self) {
        let entry = self.0.peek().entry;
        if let Some(entry) = entry {
            entry.finish.call(());
        }
    }

    /// Whether anything is still being entered into the box.
    fn entry_open(self) -> bool {
        let entry = self.0.peek().entry;
        entry.is_some_and(|entry| entry.open.call(()))
    }

    /// A new session: nothing typed for the last one carries over.
    fn clear(mut self) {
        let mut outbox = self.0.write();
        outbox.waiting = None;
        outbox.keys = KeyTimes::default();
    }
}

/// How soon auto-confirm asks again while a letter is still being keyed.
const ENTRY_RECHECK_MS: u32 = 50;

/// The generators one group draws from, one per thing it decides.
#[derive(Clone, Copy)]
enum GroupStream {
    /// The station you are copying.
    Voice,
    /// Whoever else is calling.
    Callers,
}

/// Start a session from the settings as they are.
///
/// Starting first settles the alphabet in the settings themselves, so the
/// session and the screens agree on what is being practised before anything
/// sounds. The machine is built here, on the UI thread, so Training has its
/// groups before the first paint.
pub fn start_session(ctx: AppCtx) {
    // Held before anything is written, not just before the audio is taken:
    // a session already running must leave the stored settings exactly as
    // they are.
    if ctx.session_running() {
        return;
    }
    let AppCtx {
        mut screen,
        mut runtime,
        mut settings,
        sessions,
        toasts,
        mut preview,
        ..
    } = ctx;
    let fitted = settings.peek().clone().clamp();
    if *settings.peek() != fitted {
        settings.set(fitted.clone());
    }
    // Stored now, with whatever the press that starts it kept, a call typed
    // into Home's hero among it: nothing is stored while the session runs,
    // so a reload mid-run would lose them otherwise.
    ctx.store_settings();
    // A contest is a run, with a machine of its own.
    if fitted.curriculum.char_set_mode == CharSetMode::Contest {
        start_run(ctx, fitted);
        return;
    }
    // The session has the audio now, whether or not it can open it: any
    // preview that had it has stopped.
    preview.set(Preview::Idle);
    let app = ctx.app();
    let lease = match app.player.claim(&fitted) {
        Ok(lease) => lease,
        Err(err) => {
            toasts.show(err.to_string());
            return;
        }
    };
    // Each session draws from a sampler of its own, seeded afresh.
    ctx.answer.clear();
    let sampler = Sampler::new(&sessions.peek(), &fitted, seed_rng());
    let (machine, effects) = SessionMachine::start(
        SessionId::new(lease.turn()),
        MonoMs(mono_ms()),
        fitted,
        sampler,
    );
    runtime.set(Some(machine.session().clone()));
    screen.set(Screen::Training);
    *app.session.borrow_mut() = Some(Live::Groups(Box::new(LiveSession {
        machine,
        stations: SplitMix64::new(seed_rng()),
        lease: lease.clone(),
    })));
    run_effects(ctx, &lease, effects);
}

/// Hand the running session something the user did. Nothing happens when
/// no session is running.
pub fn send_event(ctx: AppCtx, event: SessionEvent) {
    let lease = ctx
        .app()
        .session
        .borrow()
        .as_ref()
        .and_then(Live::groups)
        .map(|live| live.lease.clone());
    if let Some(lease) = lease {
        dispatch(ctx, &lease, event);
    }
}

/// Whether what is live is a contest run.
fn contest_live(ctx: AppCtx) -> bool {
    matches!(&*ctx.app().session.borrow(), Some(Live::Contest(_)))
}

/// End whatever has the training screen, keeping what it has to keep: Exit,
/// End session, End run. The machine that is live decides what that is.
pub fn finish_run(ctx: AppCtx) {
    if contest_live(ctx) {
        send_run_event(ctx, ContestEvent::FinishNow);
    } else {
        send_event(ctx, SessionEvent::FinishNow);
    }
}

/// Leave whatever has the training screen without keeping it: Discard, the
/// guided setup moving on.
pub fn abort_run(ctx: AppCtx) {
    if contest_live(ctx) {
        send_run_event(ctx, ContestEvent::Abort);
    } else {
        send_event(ctx, SessionEvent::Abort);
    }
}

/// Let the machine decide what `event` means, then do what it says.
///
/// `lease` is the session the event belongs to: an event from a session
/// that has ended, or from one before the session now running, is dropped
/// rather than handed to a machine it means nothing to.
fn dispatch(ctx: AppCtx, lease: &Lease, event: SessionEvent) {
    let mut runtime = ctx.runtime;
    let app = ctx.app();
    let closes = app
        .session
        .borrow()
        .as_ref()
        .and_then(Live::groups)
        .and_then(|live| {
            (live.lease.is(lease) && lease.is_current())
                .then(|| live.machine.closes_answer(&event))
                .flatten()
        });
    match closes {
        // What is entered in full by the deadline is part of the answer,
        // whatever is still settling. Finishing it can hand the session an
        // answer of its own, so it comes before the session is borrowed.
        Some(AnswerClose::Deadline) => ctx.answer.finish(),
        // Auto-confirm waits for the answer to sit still, and one with a
        // letter still being keyed into it has not: ask again shortly. The
        // letter, once in, lengthens the answer and calls the confirm off.
        // The answer window, likewise, closes only an answer nobody has
        // begun, and a letter being keyed is one begun: once it is in, the
        // window leaves the answer to its owner.
        Some(AnswerClose::Settled | AnswerClose::Unbegun) if ctx.answer.entry_open() => {
            let lease = lease.clone();
            spawn_forever(async move {
                if lease.sleep(ENTRY_RECHECK_MS).await.is_ok() {
                    dispatch(ctx, &lease, event);
                }
            });
            return;
        }
        // A letter keyed in full and not in the box yet begins the answer
        // too, once it is in.
        Some(AnswerClose::Unbegun) => ctx.answer.finish(),
        Some(AnswerClose::Settled) | None => {}
    }
    let effects = {
        let mut slot = app.session.borrow_mut();
        let Some(live) = slot
            .as_mut()
            .and_then(Live::groups_mut)
            .filter(|live| live.lease.is(lease) && lease.is_current())
        else {
            return;
        };
        let now = MonoMs(mono_ms());
        // When each key went in, for the group being typed into, before the
        // typing itself: recognition is timed to the keys.
        let (typing, keyed_at) = ctx.answer.keys();
        live.machine.note_keys(typing, &keyed_at);
        // Typing still waiting out the box's pause came before this event,
        // so the machine hears it first.
        let mut effects = ctx.answer.take().map_or_else(Vec::new, |(index, text)| {
            live.machine.apply(SessionEvent::Input { index, text }, now)
        });
        effects.extend(live.machine.apply(event, now));
        // Much of what arrives changes nothing — a key while the box is
        // locked, a timer the machine has forgotten — and publishing it
        // anyway would copy the whole session and redraw the screen for it.
        let session = live.machine.session();
        if runtime.peek().as_ref() != Some(session) {
            runtime.set(Some(session.clone()));
        }
        effects
    };
    run_effects(ctx, lease, effects);
}

fn run_effects(ctx: AppCtx, lease: &Lease, effects: Vec<SessionEffect>) {
    for effect in effects {
        match effect {
            SessionEffect::StopAudio => lease.stop_sending(),
            SessionEffect::Play { index, text } => play(ctx, lease, index, text),
            SessionEffect::StartTimer { id, ms } => {
                let lease = lease.clone();
                // One wait for the whole of it. A timer the machine has since
                // forgotten wakes once, is told so, and is gone.
                spawn_forever(async move {
                    if lease.sleep(ms).await.is_ok() {
                        dispatch(ctx, &lease, SessionEvent::TimerFired { id });
                    }
                });
            }
            SessionEffect::PersistAndShowResults => finish_session(ctx),
            SessionEffect::AbortToHome => abandon_session(ctx),
        }
    }
}

/// Send group `index`, and tell the machine how it went.
fn play(ctx: AppCtx, lease: &Lease, index: usize, text: String) {
    // Every group is built from the settings the session started with, not
    // from whatever the settings screen holds now: a change made mid-session
    // belongs to the next one.
    let Some((settings, transmission)) = ctx
        .app()
        .session
        .borrow()
        .as_ref()
        .and_then(Live::groups)
        .map(|live| {
            let settings = live.machine.session().settings().clone();
            (settings, live.transmission(index, text))
        })
    else {
        return;
    };
    // Where each character finishes in the send, as the player plans it:
    // what recognition is timed from.
    let char_ends_sec = plan_morse_playback_for(&transmission.text, &settings, &transmission.voice)
        .char_ends(&transmission.text);
    let lease = lease.clone();
    spawn_forever(async move {
        let event = match lease.send(&transmission, &settings).await {
            Ok(sent) => SessionEvent::PlaybackEnded {
                index,
                duration_sec: sent.duration_sec,
                char_wpm: sent.char_wpm,
                effective_wpm: sent.effective_wpm,
                char_ends_sec,
            },
            Err(AudioError::Cancelled) => SessionEvent::PlaybackStopped { index },
            Err(failure) => {
                ctx.toasts.show(failure.to_string());
                SessionEvent::PlaybackStopped { index }
            }
        };
        dispatch(ctx, &lease, event);
    });
}

/// The session, taken out of the app, with the audio given back.
fn end_session(ctx: AppCtx) -> Option<LiveSession> {
    let taken = ctx
        .app()
        .session
        .borrow_mut()
        .take_if(|live| matches!(live, Live::Groups(_)));
    let Some(Live::Groups(live)) = taken else {
        return None;
    };
    live.lease.release();
    Some(*live)
}

/// End the session without keeping it.
fn abandon_session(ctx: AppCtx) {
    let AppCtx {
        mut screen,
        mut runtime,
        ..
    } = ctx;
    end_session(ctx);
    runtime.set(None);
    screen.set(Screen::Home);
}

/// Score the session, move the level if it earned it, store it, and show
/// the results.
fn finish_session(ctx: AppCtx) {
    let AppCtx {
        mut runtime,
        mut auto_message,
        mut settings,
        toasts,
        ..
    } = ctx;
    let Some(live) = end_session(ctx) else {
        return;
    };
    let store = ctx.app().store;
    let session = live.machine.session();
    let run = store.load_auto_counters(&CountersKey::of(session.settings()));
    // The machine only finishes a session it has scored something in.
    let done = complete_session(session, run, now_ms(), local_date_string());
    match &done.counters {
        CountersUpdate::Keep(key, counters) => store.save_auto_counters(key, *counters),
        CountersUpdate::Clear(keys) => store.clear_auto_counters(keys),
    }
    let message = done
        .change
        .as_ref()
        .map(crate::ui::auto_level::change_message);
    if done.change.is_some() {
        // Stored by the app's settings effect, once the results are up.
        settings.set(done.settings);
    }
    if let Some(message) = &message {
        toasts.show(message.clone());
    }
    auto_message.set(message);
    runtime.set(None);
    keep_and_show(ctx, done.result);
}

/// Add `finished` to the history, store it, and show it on the results
/// screen: a session of groups and a contest run alike.
pub fn keep_and_show(ctx: AppCtx, finished: SessionResult) {
    let AppCtx {
        mut screen,
        mut result,
        mut sessions,
        toasts,
        ..
    } = ctx;
    // The history grows by one where it is, and is stored from there.
    sessions.write().push(finished.clone());
    let saved = ctx.store_history();
    // A browser may clear its store to make room; there is now something in
    // it worth asking it to keep.
    ctx.app().store.ask_to_keep();
    if let Some(problem) = saved.problem() {
        toasts.show(problem);
    }
    result.set(Some(Finished::Kept(Box::new(finished))));
    screen.set(Screen::Results);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::{Behaviour, Call};
    use crate::persist::Store;
    use crate::testing::{Harness, run, test_settings};
    use cw_core::AutoLevelCounters;
    use cw_core::SessionEffect;
    use cw_core::SessionPhase;
    use std::cell::Cell;
    use std::rc::Rc;

    impl Live {
        /// Its hold on the audio, for a test to end or to stop.
        pub(crate) fn lease(&self) -> &Lease {
            match self {
                Self::Groups(live) => &live.lease,
                Self::Contest(live) => live.lease(),
            }
        }
    }

    /// A whole session, answered correctly, ends on the results screen with the
    /// session stored, and the store asked to keep what it holds.
    #[test]
    fn answering_every_group_finishes_the_session_and_stores_it() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(result.groups.len(), 2);
            assert!(result.groups.iter().all(|g| g.correct));
            assert_eq!(result.accuracy, 1.0);
            assert_eq!(h.ctx.sessions.peek().len(), 1);
            assert_eq!(h.store.load_sessions().len(), 1);
            assert_eq!(h.store.keep_asks.get(), 1);
            // The machine is torn down, and the audio with it.
            assert!(h.app.session.borrow().is_none());
            assert!(h.calls().contains(&Call::Shutdown));
            // Two groups, one send each.
            assert_eq!(h.texts().len(), 2);
        });
    }

    /// Settings in Contest start a run, not a session of groups: the run's
    /// machine has the training screen, and its first CQ is on the air.
    #[test]
    fn settings_in_contest_start_a_run() {
        run(|| async {
            let mut h = Harness::with_settings(crate::testing::contest_settings());
            h.start_training();
            assert_eq!(h.screen(), Screen::Training);
            assert!(
                h.ctx.runtime.peek().is_none(),
                "a session of groups started"
            );
            assert!(matches!(&*h.app.session.borrow(), Some(Live::Contest(_))));
            assert!(h.ctx.session_running());
            assert_eq!(
                h.ctx.contest.peek().as_ref().map(|view| view.phase),
                Some(cw_core::ContestPhase::Running)
            );
            assert!(h.texts()[0].starts_with("CQ DL1ABC"), "{:?}", h.texts());
            assert_eq!(
                h.ctx.settings.peek().curriculum.char_set_mode,
                CharSetMode::Contest
            );
        });
    }

    #[test]
    fn a_group_nobody_answers_times_out_and_the_session_moves_on() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_timeout = 1.0;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            // Nothing typed: the answer window closes on its own.
            assert!(
                h.run_until(4_000, |h| h.texts().len() == 2).await,
                "the second group should have been sent"
            );
            let confirmed = h.ctx.runtime.peek().as_ref().map(|s| s.view().confirmed);
            assert_eq!(confirmed, Some(vec![true, false]));
            assert_eq!(
                h.ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .and_then(|s| s.group(0))
                    .map(|g| g.input().to_string()),
                Some(String::new())
            );
        });
    }

    /// What group `index` holds in the running session.
    fn input_of(h: &Harness, index: usize) -> Option<String> {
        h.ctx
            .runtime
            .peek()
            .as_ref()
            .and_then(|s| s.group(index))
            .map(|g| g.input().to_string())
    }

    /// A key still in the box's pause when the answer window runs out
    /// counts: the session hears it first, and it is an answer begun, which
    /// the window leaves open.
    #[test]
    fn typing_still_on_its_way_is_heard_before_the_answer_window_closes() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_timeout = 1.0;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            // Typed, and not yet handed over — nothing will hand it over but
            // the runtime.
            h.ctx.answer.hold(0, "K".into());
            h.advance(3_000).await;
            assert_eq!(input_of(&h, 0).as_deref(), Some("K"));
            assert_eq!(
                h.awaiting_answer(),
                Some(0),
                "the window cut the answer off"
            );
            assert_eq!(h.texts().len(), 1);
            // Enter takes it as it is.
            h.send(SessionEvent::Confirm);
            let confirmed = h.ctx.runtime.peek().as_ref().map(|s| s.view().confirmed);
            assert_eq!(confirmed, Some(vec![true, false]));
        });
    }

    /// A character typed as soon as it is heard is timed from where it
    /// finished sounding, as the audio plan has it, to its key in the box:
    /// one-character groups, which a box shut while sending can time.
    #[test]
    fn a_key_is_timed_from_the_end_of_its_character() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.min_group_size = 1;
            settings.curriculum.max_group_size = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            h.advance(300).await;
            let sent = h.sent(0);
            h.ctx.answer.hold(0, sent.clone());
            h.send(SessionEvent::Confirm);
            let session = h.ctx.runtime.peek().clone().expect("the session");
            let timing = &session.result(0, "2026-10-24".into()).group_timings[0];
            let took = timing
                .recognition_ms
                .first()
                .copied()
                .flatten()
                .expect("timed");
            assert!(
                (300..=300 + crate::time::POLL_MS).contains(&took),
                "{took} ms for {sent}"
            );
        });
    }

    /// Nothing typed: the answer window moves the session on by itself.
    #[test]
    fn the_answer_window_moves_on_from_a_group_not_begun() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_timeout = 1.0;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            assert!(h.run_until(4_000, |h| h.texts().len() == 2).await);
            assert_eq!(input_of(&h, 0).as_deref(), Some(""));
        });
    }

    /// A session counts down before its first group, a second a step, and
    /// sends nothing until it has.
    #[test]
    fn the_first_group_waits_for_the_count_down() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.countdown_sec = 3;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert_eq!(h.phase(), Some(SessionPhase::Countdown { remaining: 3 }));
            h.advance(1_000).await;
            assert_eq!(h.phase(), Some(SessionPhase::Countdown { remaining: 2 }));
            h.advance(1_900).await;
            assert_eq!(h.phase(), Some(SessionPhase::Countdown { remaining: 1 }));
            assert!(h.texts().is_empty(), "sent during the count down");
            h.advance(200).await;
            assert_eq!(h.texts().len(), 1);
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
        });
    }

    /// The same for every other way a group can close: what was typed gets
    /// there first.
    #[test]
    fn typing_still_on_its_way_is_heard_before_confirm_and_finish() {
        run(|| async {
            for event in [SessionEvent::Confirm, SessionEvent::FinishNow] {
                let mut h = Harness::new();
                h.start_training();
                assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
                h.ctx.answer.hold(0, "K".into());
                h.send(event.clone());
                if event == SessionEvent::FinishNow {
                    let result = h
                        .ctx
                        .result
                        .peek()
                        .clone()
                        .and_then(Finished::kept)
                        .expect("{event:?} kept it");
                    assert_eq!(result.groups[0].received, "K", "{event:?}");
                } else {
                    assert_eq!(input_of(&h, 0).as_deref(), Some("K"), "{event:?}");
                }
            }
        });
    }

    /// Auto-confirm waits while anything is still being entered, and comes
    /// back for the answer once it has settled. It never finishes an entry
    /// early, which would cut a paddle letter being keyed in two.
    #[test]
    fn auto_confirm_waits_for_an_entry_still_under_way() {
        run(|| async {
            let mut h = Harness::new();
            // Still entering for the first three asks.
            let open_for = Rc::new(Cell::new(3_u32));
            let finished = Rc::new(Cell::new(0));
            let entry = h.in_app(|| {
                let (open_for, finished) = (Rc::clone(&open_for), Rc::clone(&finished));
                Entry {
                    finish: Callback::new(move |()| finished.set(finished.get() + 1)),
                    open: Callback::new(move |()| {
                        let left = open_for.get();
                        open_for.set(left.saturating_sub(1));
                        left > 0
                    }),
                }
            });
            h.ctx.answer.set_entry(Some(entry));
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            let confirmed = |h: &Harness| {
                h.ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.view().confirmed[0])
            };
            h.advance(u64::from(cw_core::AUTO_CONFIRM_DELAY_MS) + 20)
                .await;
            assert!(!confirmed(&h), "auto-confirm went ahead mid-entry");
            assert!(h.run_until(1_000, confirmed).await);
            assert_eq!(open_for.get(), 0);
            assert_eq!(finished.get(), 0, "auto-confirm finished an entry");
            assert_eq!(input_of(&h, 0), Some(sent));
        });
    }

    /// A key that lands as a session closes does not carry over into the
    /// next one.
    #[test]
    fn a_new_session_starts_with_nothing_on_its_way() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.lock_input_during_group_playback = false;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            h.send(SessionEvent::Abort);
            assert_eq!(h.screen(), Screen::Home);
            // The closing box takes one more key.
            h.ctx.answer.hold(0, "K".into());
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            assert_eq!(input_of(&h, 0).as_deref(), Some(""));
        });
    }

    /// Before the answer closes — and only then — the box is asked to finish
    /// what is entered in full, and what it finishes is in the answer. A
    /// send ending, or the gap between two sends, leaves the answer open:
    /// finishing a paddle letter there would cut one being keyed in two. The
    /// answer window asks too, and what it finishes begins the answer, which
    /// it then leaves open for Enter.
    #[test]
    fn the_box_finishes_its_entry_only_as_the_answer_closes() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_timeout = 1.0;
            settings.playback.group_repeat_min = 2;
            settings.playback.group_repeat_max = 2;
            let mut h = Harness::with_settings(settings);
            let asked = Rc::new(Cell::new(0));
            let answer = h.ctx.answer;
            let entry = h.in_app(|| {
                let asked = Rc::clone(&asked);
                Entry {
                    finish: Callback::new(move |()| {
                        asked.set(asked.get() + 1);
                        answer.hold(0, "T".into());
                    }),
                    open: Callback::new(|()| false),
                }
            });
            answer.set_entry(Some(entry));
            h.start_training();
            assert!(h.run_until(8_000, |h| h.awaiting_answer() == Some(0)).await);
            assert_eq!(asked.get(), 0, "two sends and a gap closed nothing");
            assert!(h.run_until(4_000, |_| asked.get() == 1).await);
            assert_eq!(input_of(&h, 0).as_deref(), Some("T"));
            h.advance(2_000).await;
            assert_eq!(
                h.awaiting_answer(),
                Some(0),
                "an answer begun is waited for"
            );
            h.send(SessionEvent::Confirm);
            assert_eq!(asked.get(), 2);
            assert!(h.run_until(4_000, |h| h.texts().len() == 3).await);
            assert_eq!(input_of(&h, 0).as_deref(), Some("T"));
        });
    }

    #[test]
    fn a_group_sent_more_than_once_opens_the_answer_box_only_at_the_end() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            settings.playback.link_group_repeat = true;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            let sent = h.sent(0);

            assert!(h.run_until(20_000, |h| h.texts().len() == 3).await);
            // Every send is the same group.
            assert_eq!(h.texts(), vec![sent.clone(); 3]);
            assert!(h.run_until(5_000, |h| h.awaiting_answer() == Some(0)).await);
            // And no fourth send after the answer window opens.
            h.advance(2_000).await;
            assert_eq!(h.texts().len(), 3);
        });
    }

    #[test]
    fn a_matching_answer_confirms_itself_and_the_next_group_follows() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let first = h.sent(0);
            h.type_answer(0, &first.to_lowercase());
            // Nothing is confirmed until the auto-confirm delay has passed.
            assert!(
                h.ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| !s.view().confirmed[0])
            );
            assert!(
                h.run_until(1_000, |h| h
                    .ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.view().confirmed[0]))
                    .await
            );
            // Typed in lower case, stored the way it was sent.
            assert_eq!(
                h.ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .and_then(|s| s.group(0))
                    .map(|g| g.input().to_string()),
                Some(first)
            );
            // The next group is generated before it is played, never sent empty.
            assert!(h.run_until(8_000, |h| h.texts().len() == 2).await);
            assert!(h.texts().iter().all(|text| !text.is_empty()));
            assert_ne!(h.sent(1), "");
        });
    }

    #[test]
    fn changing_an_answer_withdraws_the_confirm_that_was_pending() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            // Backspace inside the 300 ms window.
            h.advance(100).await;
            h.type_answer(0, &sent[..1]);
            h.advance(1_000).await;
            assert!(
                h.ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| !s.view().confirmed[0])
            );
            assert_eq!(h.awaiting_answer(), Some(0));
        });
    }

    #[test]
    fn typing_is_refused_while_the_group_is_being_sent() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            h.advance(1_000).await;
            assert!(
                h.ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| !s.view().confirmed[0])
            );
        });
    }

    /// A session that ends mid-send leaves nothing behind to disturb the
    /// next one: the send it cancelled must not end the session after it.
    #[test]
    fn a_new_session_after_one_left_mid_send_keeps_nothing_of_the_old_one() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let first = h.lease();
            h.advance(100).await;
            h.send(SessionEvent::Abort);

            h.recorder.clear();
            h.start_training();
            let second = h.lease();
            assert!(second.turn() > first.turn());
            assert_eq!(h.texts().len(), 1);

            // Let the old send's cancellation land: it must not end the new session.
            h.advance(2_000).await;
            assert_eq!(h.screen(), Screen::Training);
            assert!(!matches!(h.phase(), Some(SessionPhase::Aborted)));
            assert_eq!(
                h.app
                    .session
                    .borrow()
                    .as_ref()
                    .and_then(Live::groups)
                    .map(|live| live.machine.session().session_id().raw()),
                Some(second.turn())
            );
            assert!(h.run_until(6_000, |h| h.awaiting_answer() == Some(0)).await);
        });
    }

    /// Leaving the session while a group is in flight: the cancelled send must
    /// not drag the app back onto a results screen.
    #[test]
    fn walking_away_mid_send_goes_home_and_stays_there() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(100).await;
            h.send(SessionEvent::Abort);
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.ctx.runtime.peek().is_none());
            assert!(h.app.session.borrow().is_none());

            h.advance(5_000).await;
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.ctx.result.peek().is_none());
            assert!(h.ctx.sessions.peek().is_empty());
        });
    }

    #[test]
    fn ending_a_session_early_keeps_what_was_already_scored() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            assert!(
                h.run_until(1_000, |h| h
                    .ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.view().confirmed[0]))
                    .await
            );
            h.send(SessionEvent::FinishNow);
            h.advance(200).await;
            assert_eq!(h.screen(), Screen::Results);
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(result.groups.len(), 1);
            assert_eq!(h.ctx.sessions.peek().len(), 1);
        });
    }

    /// The regression test for a send that is counted down on the wall clock:
    /// with the audio clock frozen, the session must not walk on as though the
    /// group had been heard.
    #[test]
    fn a_stalled_send_never_passes_for_a_finished_one() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::Stall);
            h.start_training();

            // Well past the length of the group, and still nothing is confirmed
            // and no answer window has opened.
            h.advance(1_500).await;
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
            assert!(h.awaiting_answer().is_none());

            // The stall is eventually reported, the player rebuilt, and after
            // three attempts the session gives up rather than pretending.
            assert!(
                h.run_until(60_000, |h| h.screen() == Screen::Home).await,
                "a stalled player should end the session, not hang"
            );
            assert!(h.recorder.players_built.get() > 1, "the player is rebuilt");
            assert_eq!(h.toast().as_deref(), Some("Audio playback stalled."));
            assert!(h.ctx.sessions.peek().is_empty());
        });
    }

    /// Leaving the tab must not cost the listener their session. The audio
    /// clock parks while the page is away, and a send that is counted down
    /// against it would run out the stall budget in silence and end the
    /// session — on the browser build, on the first group, for a tab switch.
    #[test]
    fn a_session_survives_the_page_going_to_the_background() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(200).await;
            h.set_page_hidden(true);

            // Far longer than the stall grace, in a phase that would have
            // failed on the wall clock.
            h.advance(60_000).await;
            assert_eq!(
                h.phase(),
                Some(SessionPhase::Playing { index: 0 }),
                "the group is still waiting to be heard"
            );
            assert_eq!(h.toast(), None, "nothing has gone wrong");
            assert_eq!(h.screen(), Screen::Training);
            assert_eq!(h.texts().len(), 1, "and it was not sent again");

            // Back to the tab: the same send finishes and the session goes on.
            h.set_page_hidden(false);
            assert!(
                h.run_until(30_000, |h| h.awaiting_answer().is_some()).await,
                "the answer window should open once the page is back"
            );
            assert_eq!(h.screen(), Screen::Training);
            h.play_through(120_000).await;
            assert_eq!(h.screen(), Screen::Results);
        });
    }

    /// Callsign mode changes how a group is built and nothing else. The proof
    /// is that a whole session runs through the same machine, the same audio
    /// and the same scoring, and lands on the results screen with real
    /// callsigns in it.
    #[test]
    fn a_callsign_session_runs_through_the_same_machine_as_any_other() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            settings.curriculum.callsign_level = cw_core::CALLSIGN_TIER_MAX;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(120_000).await;

            assert_eq!(h.screen(), Screen::Results);
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(result.groups.len(), 2);
            assert_eq!(result.accuracy, 1.0, "every call was answered correctly");
            // The calls themselves are cw-core's to get right; here, every
            // group has the digit that separates a call's prefix from its
            // suffix.
            for group in &result.groups {
                assert!(
                    group.sent.chars().any(|c| c.is_ascii_digit()),
                    "{:?} is not a callsign",
                    group.sent
                );
            }
            // The tier is what this session was run at, not the Koch level.
            assert_eq!(result.level, cw_core::CALLSIGN_TIER_MAX);
            assert_eq!(result.char_set_mode, CharSetMode::Callsign);
            assert_eq!(h.ctx.sessions.peek().len(), 1);
            // And what was sent is what was played.
            assert_eq!(h.texts().len(), 2);
            for text in h.texts() {
                assert!(text.chars().any(|c| c.is_ascii_digit()), "{text:?}");
            }
        });
    }

    /// A callsign session must not teach the group sampler, or a tier-6 call
    /// full of `Q` and `Z` would drag the Koch curriculum around behind it.
    /// What a callsign session says of each character counts for every set:
    /// switching to Koch afterwards keeps it.
    #[test]
    fn a_callsign_session_teaches_the_sampler_of_every_set() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(120_000).await;
            let stored = h.ctx.sessions.peek().clone();
            assert_eq!(stored.len(), 1);
            let sent = stored[0]
                .letter_accuracy()
                .keys()
                .copied()
                .collect::<Vec<_>>();
            assert!(!sent.is_empty());

            let mut koch = test_settings();
            koch.curriculum.char_set_mode = CharSetMode::Koch;
            let rows = cw_core::sampling_rows(&koch, &stored);
            for character in sent {
                let row = rows
                    .iter()
                    .find(|row| row.character == character)
                    .unwrap_or_else(|| panic!("{character} listed under Koch"));
                assert!(
                    row.alpha + row.beta > 2.0,
                    "{character} keeps what the callsign session taught"
                );
            }
        });
    }

    /// A station repeating its call is the same station. Redrawing the tone,
    /// speed and strength on every send made three repeats of one group arrive
    /// as three different operators — which is what the screen promises they
    /// are not, and which makes a repeat useless for confirming what you heard.
    #[test]
    fn every_repeat_of_a_group_comes_from_the_same_station() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            // Ranges wide enough that a redraw could not pass for a repeat.
            settings.playback.char_wpm_min = 15.0;
            settings.playback.char_wpm_max = 40.0;
            settings.playback.link_char_to_effective = true;
            settings.band.side_tone_min = 400.0;
            settings.band.side_tone_max = 900.0;
            settings.band.volume_min = 0.2;
            settings.band.volume_max = 1.0;
            settings.band.link_volume = false;

            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(200_000).await;
            assert_eq!(h.screen(), Screen::Results);

            let voices = h.recorder.voices();
            let texts = h.texts();
            assert_eq!(voices.len(), texts.len());
            assert_eq!(texts.len(), 6, "two groups, three sends each");

            // Every send of one group is the same operator...
            for group in texts.chunks(3).zip(voices.chunks(3)) {
                let (group_texts, group_voices) = group;
                assert!(
                    group_texts.windows(2).all(|w| w[0] == w[1]),
                    "a group's repeats should be the same text: {group_texts:?}"
                );
                assert!(
                    group_voices.windows(2).all(|w| w[0] == w[1]),
                    "a group's repeats changed station: {group_voices:?}"
                );
            }
            // ...and the next group is somebody else.
            assert_ne!(
                voices[0], voices[3],
                "every group came from the same station, which is its own problem"
            );
        });
    }

    /// With adaptive speed on, each group is sent at the speed the session
    /// chose for it, inside the speed settings, and every repeat at the same.
    #[test]
    fn adaptive_speed_sends_each_group_at_its_chosen_speed() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 3;
            settings.playback.group_repeat_min = 2;
            settings.playback.group_repeat_max = 2;
            settings.playback.char_wpm_min = 15.0;
            settings.playback.char_wpm_max = 35.0;
            settings.playback.link_char_to_effective = true;
            settings.playback.adaptive_speed = true;

            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(300_000).await;
            assert_eq!(h.screen(), Screen::Results);

            let voices = h.recorder.voices();
            assert_eq!(voices.len(), 6, "three groups, two sends each");
            for voice in &voices {
                assert!(
                    (15.0..=35.0).contains(&voice.char_wpm),
                    "sent at {} WPM",
                    voice.char_wpm
                );
                assert_eq!(voice.effective_wpm, voice.char_wpm, "tied spacing");
            }
            for pair in voices.chunks(2) {
                assert_eq!(
                    pair[0].char_wpm, pair[1].char_wpm,
                    "a repeat is the same speed"
                );
            }
        });
    }

    /// Every launch tunes in its own stations. The first session of every
    /// launch used to bring the same pitch, speed, strength and fist for each
    /// of its groups, and the same callers across them, because the stations
    /// were drawn from a counter that starts at zero each time the app does.
    #[test]
    fn two_launches_do_not_hear_the_same_stations() {
        run(|| async {
            let first_session = || {
                let mut settings = test_settings();
                settings.playback.char_wpm_min = 15.0;
                settings.playback.char_wpm_max = 40.0;
                settings.band.side_tone_min = 400.0;
                settings.band.side_tone_max = 900.0;
                settings.band.link_volume = false;
                settings.band.volume_min = 0.2;
                settings.band.volume_max = 1.0;
                let mut h = Harness::with_settings(settings);
                h.start_training();
                h.recorder.voices()
            };
            let (one, other) = (first_session(), first_session());
            assert_eq!(one.len(), 1);
            assert_ne!(one, other, "two launches opened on the same station");
        });
    }

    /// A pile-up changes what you hear, not what you answer. The session still
    /// scores the station you were copying, and the others are only ever in the
    /// way — which is the whole point of them.
    #[test]
    fn a_pile_up_is_heard_but_never_answered() {
        run(|| async {
            let mut settings = test_settings();
            // Every group is a full pile-up: each session draws its stations
            // afresh, and a range could leave a short one with nobody else.
            settings.band.stations_min = cw_core::STATIONS_MAX;
            settings.band.stations_max = cw_core::STATIONS_MAX;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(200_000).await;
            assert_eq!(h.screen(), Screen::Results);

            let sent = h.recorder.sent();
            assert!(!sent.is_empty());
            assert!(
                sent.iter().any(|t| !t.others.is_empty()),
                "with five calling, somebody else should have got through"
            );
            for transmission in &sent {
                for other in &transmission.others {
                    assert_ne!(
                        other.text, transmission.text,
                        "an interfering station sent the answer"
                    );
                    assert!(
                        other.voice.volume < transmission.voice.volume,
                        "an interfering station was the strongest"
                    );
                }
            }

            // Every group was answered correctly, because the pile-up is not
            // part of the answer.
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(result.accuracy, 1.0);
            assert_eq!(h.ctx.sessions.peek().len(), 1);
        });
    }

    /// A repeat is the same moment on the band, pile-up and all. Redrawing the
    /// others would make a repeat a different puzzle rather than another look
    /// at the same one.
    #[test]
    fn a_repeat_brings_back_the_same_pile_up() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            settings.band.stations_max = cw_core::STATIONS_MAX;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(200_000).await;

            let sent = h.recorder.sent();
            assert_eq!(sent.len(), 6, "two groups, three sends each");
            for group in sent.chunks(3) {
                assert!(
                    group.windows(2).all(|w| w[0] == w[1]),
                    "a repeat brought a different pile-up"
                );
            }
            assert_ne!(sent[0], sent[3], "both groups drew the same band");
        });
    }

    #[test]
    fn a_broken_stream_is_retried_before_the_session_is_given_up() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::FailMidSend);
            h.start_training();
            assert!(h.run_until(30_000, |h| h.screen() == Screen::Home).await);
            // Three attempts, each on a freshly built player.
            assert_eq!(h.texts().len(), 3);
            assert!(h.recorder.players_built.get() >= 3);
            assert!(h.toast().is_some());
        });
    }

    #[test]
    fn a_player_that_refuses_to_start_is_reported_once_the_retries_run_out() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::RefuseToStart);
            h.start_training();
            assert!(h.run_until(30_000, |h| h.screen() == Screen::Home).await);
            assert_eq!(h.toast().as_deref(), Some("Audio stream: device is gone"));
        });
    }

    /// A group answered while its send is being retried is not sent again.
    ///
    /// With the box open during the send, the answer can land while a
    /// failed send waits for its rebuilt player. Confirming it stopped the
    /// audio, but the retry only checked that the session was the same one,
    /// so it went on to send the confirmed group across the gap before the
    /// next.
    #[test]
    fn an_answer_given_during_a_retry_stops_the_retry() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.lock_input_during_group_playback = false;
            // A pause after the group, so no next one is sent in the half
            // second watched for a second send of this one.
            settings.playback.group_pause_sec = 2.0;
            let mut h = Harness::with_settings(settings);
            h.recorder.fail_next(1);
            h.start_training();
            // The first send breaks, and the retry rebuilds the player.
            let mut waited = 0;
            while h.recorder.players_built.get() < 2 {
                assert!(waited < 5_000, "the send was never retried");
                h.advance(4).await;
                waited += 4;
            }
            let sends = h.texts().len();
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            h.send(SessionEvent::Confirm);
            assert_eq!(h.phase(), Some(SessionPhase::InterGroupGap { next: 1 }));
            h.advance(500).await;
            assert_eq!(
                h.texts().len(),
                sends,
                "the answered group was sent again: {:?}",
                h.texts()
            );
        });
    }

    /// What changes nothing is not published: no copy of the session, and
    /// no redraw of the screen showing it.
    #[test]
    fn an_event_that_changes_nothing_is_not_published() {
        run(|| async {
            use std::sync::atomic::Ordering;

            let mut h = Harness::new();
            h.start_training();
            let writes = h.count_writes(h.ctx.runtime);
            // The group is still being sent, and the box is locked.
            h.type_answer(0, "K");
            assert_eq!(
                writes.load(Ordering::Relaxed),
                0,
                "a refused key was published"
            );

            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let before = writes.load(Ordering::Relaxed);
            h.type_answer(0, "K");
            assert!(
                writes.load(Ordering::Relaxed) > before,
                "an answer went unpublished"
            );
        });
    }

    /// A device that comes back mid-session must not cost the user the session.
    #[test]
    fn a_send_that_fails_once_and_then_works_carries_on() {
        run(|| async {
            let mut h = Harness::new();
            // The first send breaks; the device is back for the second.
            h.recorder.fail_next(1);
            h.start_training();
            assert!(
                h.run_until(20_000, |h| h.awaiting_answer() == Some(0))
                    .await
            );
            assert_eq!(h.texts().len(), 2, "the group is sent again, not skipped");
            assert!(h.recorder.players_built.get() >= 2, "the player is rebuilt");
            assert_eq!(h.screen(), Screen::Training);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            assert!(
                h.run_until(2_000, |h| h
                    .ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.view().confirmed[0]))
                    .await
            );
        });
    }

    /// A hard audio failure after something has been scored keeps the score.
    #[test]
    fn audio_that_dies_after_a_scored_group_still_shows_the_results() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            assert!(
                h.run_until(1_000, |h| h
                    .ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.view().confirmed[0]))
                    .await
            );
            h.set_behaviour(Behaviour::RefuseToStart);
            assert!(h.run_until(30_000, |h| h.screen() == Screen::Results).await);
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(result.groups.len(), 1);
        });
    }

    #[test]
    fn an_event_for_a_session_that_is_gone_is_ignored() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let stale = h.lease();
            let ctx = h.ctx;
            // The user leaves; an old send then reports that it ended.
            h.send(SessionEvent::Abort);
            h.in_app(|| {
                dispatch(
                    ctx,
                    &stale,
                    SessionEvent::PlaybackEnded {
                        index: 0,
                        duration_sec: 1.0,
                        char_wpm: 20.0,
                        effective_wpm: 20.0,
                        char_ends_sec: Vec::new(),
                    },
                )
            });
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.ctx.runtime.peek().is_none());
        });
    }

    #[test]
    fn a_session_at_the_end_of_its_groups_does_not_ask_for_another() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.texts().len(), 1);
        });
    }

    #[test]
    fn an_event_with_no_session_behind_it_does_nothing() {
        run(|| async {
            let mut h = Harness::new();
            h.send(SessionEvent::Confirm);
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.ctx.runtime.peek().is_none());
        });
    }

    #[test]
    fn an_event_from_an_older_session_never_reaches_the_new_machine() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let older = h.lease();
            h.send(SessionEvent::Abort);
            h.start_training();
            let ctx = h.ctx;
            h.in_app(|| dispatch(ctx, &older, SessionEvent::Abort));
            h.pump();
            assert_eq!(h.screen(), Screen::Training);
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
        });
    }

    /// Stopping the audio without ending the session is what a stray stop looks
    /// like from inside: the send reports cancelled and the session is closed
    /// with whatever it had, rather than sitting in silence.
    #[test]
    fn a_send_stopped_from_outside_ends_the_session_cleanly() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(100).await;
            h.lease().stop_sending();
            assert!(h.run_until(5_000, |h| h.screen() == Screen::Home).await);
            assert!(h.ctx.sessions.peek().is_empty());
        });
    }

    #[test]
    fn a_timer_the_machine_never_started_does_not_move_the_session_on() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let (ctx, lease) = (h.ctx, h.lease());
            h.in_app(|| {
                run_effects(
                    ctx,
                    &lease,
                    vec![SessionEffect::StartTimer { id: 999, ms: 20 }],
                )
            });
            h.pump();
            h.advance(200).await;
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
        });
    }

    #[test]
    fn effects_from_a_session_that_is_over_are_dropped() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let (ctx, stale) = (h.ctx, h.lease());
            h.send(SessionEvent::Abort);
            h.recorder.clear();
            h.in_app(|| {
                run_effects(
                    ctx,
                    &stale,
                    vec![
                        SessionEffect::Play {
                            index: 1,
                            text: "KM".into(),
                        },
                        SessionEffect::StartTimer { id: 1, ms: 10 },
                    ],
                )
            });
            h.pump();
            h.advance(500).await;
            assert!(h.texts().is_empty());
            assert_eq!(h.screen(), Screen::Home);
        });
    }

    #[test]
    fn a_timer_a_finished_session_no_longer_needs_does_nothing() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            let (ctx, stale) = (h.ctx, h.lease());
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            h.in_app(|| {
                run_effects(
                    ctx,
                    &stale,
                    vec![SessionEffect::StartTimer { id: 1, ms: 10 }],
                )
            });
            h.pump();
            h.advance(100).await;
            assert_eq!(h.screen(), Screen::Results);
        });
    }

    #[test]
    fn a_session_plays_its_first_group_and_opens_the_answer_box() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert_eq!(h.screen(), Screen::Training);
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
            let sent = h.sent(0);
            assert_eq!(sent.chars().count(), 2);
            assert_eq!(h.texts(), vec![sent.clone()]);
            // The answer box stays shut until the send is over.
            assert!(
                h.ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.input_locked(0))
            );

            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            assert!(
                !h.ctx
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.input_locked(0))
            );
        });
    }

    /// A session already running is not started over: not its audio, not
    /// its machine, and not the settings it was started with.
    #[test]
    fn starting_again_while_a_session_runs_changes_nothing() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let lease = h.lease();
            h.ctx.settings.set({
                let mut changed = test_settings();
                changed.curriculum.num_groups = 7;
                changed
            });
            h.start_training();
            assert!(h.lease().is(&lease) && lease.is_current());
            assert_eq!(h.ctx.settings.peek().curriculum.num_groups, 7);
            assert_eq!(h.texts().len(), 1);
        });
    }

    #[test]
    fn a_good_session_can_move_the_level_and_says_so() {
        run(|| async {
            let mut settings = test_settings();
            settings.auto_level.auto_adjust_level = true;
            settings.auto_level.auto_adjust_above_threshold_count = 1;
            settings.auto_level.auto_adjust_threshold = 50.0;
            settings.curriculum.level = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.ctx.settings.peek().curriculum.level, 2);
            let message = h.ctx.auto_message.peek().clone().expect("a message");
            assert!(message.contains("Level increased to 2"), "{message}");
            assert_eq!(h.toast().as_deref(), Some(message.as_str()));
            // The level that was just left behind keeps no counters.
            assert_eq!(
                h.store
                    .load_auto_counters(&CountersKey::of(&h.ctx.settings.peek())),
                AutoLevelCounters::default()
            );
        });
    }

    #[test]
    fn a_session_that_does_not_move_the_level_keeps_counting() {
        run(|| async {
            let mut settings = test_settings();
            settings.auto_level.auto_adjust_level = true;
            settings.auto_level.auto_adjust_above_threshold_count = 5;
            settings.auto_level.auto_adjust_threshold = 50.0;
            let mut h = Harness::with_settings(settings.clone());
            h.start_training();
            h.play_through(60_000).await;
            assert!(h.ctx.auto_message.peek().is_none());
            assert_eq!(
                h.store.load_auto_counters(&CountersKey::of(&settings)),
                AutoLevelCounters { above: 1, below: 0 }
            );
            let progress = crate::persist::auto_progress(&*h.store, &settings).expect("progress");
            assert_eq!(progress.above_count, 1);
            assert_eq!(progress.above_target, 5);
        });
    }

    /// A store with room for fewer sessions than the history keeps the
    /// newest, says so, and the screens show what it kept. A browser out of
    /// room used to drop the oldest in silence, and the app went on showing
    /// them until the next launch.
    #[test]
    fn a_history_too_big_to_store_keeps_its_newest_and_says_so() {
        run(|| async {
            let mut h = Harness::new();
            h.ctx.sessions.set(vec![
                crate::testing::stored_session("2026-09-01", &[("KM", "KM")]),
                crate::testing::stored_session("2026-09-02", &[("KM", "KM")]),
            ]);
            h.store.room.set(Some(2));
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let shown: Vec<String> = h
                .ctx
                .sessions
                .peek()
                .iter()
                .map(|s| s.date.clone())
                .collect();
            assert_eq!(shown.len(), 2);
            assert_eq!(shown[0], "2026-09-02", "the oldest is the one let go");
            assert_eq!(h.store.load_sessions().len(), 2);
            assert!(
                h.toast()
                    .is_some_and(|text| text.contains("newest 2") && !text.contains("contest")),
                "{:?}",
                h.toast()
            );
        });
    }

    /// A history that held a contest run says why it ran short of room: a
    /// run's log takes more of it than a session of groups.
    #[test]
    fn a_squeezed_history_that_held_a_run_says_runs_take_more_room() {
        run(|| async {
            let mut h = Harness::new();
            let qsos = [("K5XYZ", "214", "K5XYZ", "214")];
            h.ctx.sessions.set(vec![
                crate::testing::stored_run("2026-09-01", &qsos),
                crate::testing::stored_session("2026-09-02", &[("KM", "KM")]),
            ]);
            h.store.room.set(Some(2));
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.store.load_sessions().len(), 2);
            assert!(
                h.toast().is_some_and(|text| text.contains("newest 2")
                    && text.ends_with("Long contest runs take more room.")),
                "{:?}",
                h.toast()
            );
        });
    }

    /// A history the store could not read is not written over, and the user
    /// is told the session was not kept.
    #[test]
    fn a_session_is_not_stored_over_a_history_that_could_not_be_read() {
        run(|| async {
            let mut h = Harness::new();
            h.store.held.set(true);
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results, "the results are still shown");
            assert!(h.store.load_sessions().is_empty());
            assert!(
                h.toast()
                    .is_some_and(|text| text.contains("could not be read")),
                "{:?}",
                h.toast()
            );
        });
    }
}
