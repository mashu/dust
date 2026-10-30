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
    AnswerClose, CharSamplingState, CharSetMode, Completion, ContestEvent, CountersKey,
    CountersUpdate, Group, KeyLog, Keys, MonoMs, Sampler, SessionEffect, SessionEvent, SessionId,
    SessionMachine, SessionResult, SplitMix64, TrainingSettings, Transmission, answer_chars,
    apply_auto_level, complete_result, complete_session, generate_training_group, resolve_station,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use crate::contest_runtime::{LiveContest, send_run_event, start_run};
use crate::persist::{OpenSession, SessionKey, SoFar};
use crate::playback::Preview;
use crate::player::{AudioError, Lease, SendNews};
use crate::state::{AppCtx, Finished, Notice, Screen};
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

/// How often a running session's heartbeat says the app is still running
/// ([`beat`]).
const HEARTBEAT_MS: u32 = 250;

/// A gap this long since the app was last seen running a session is time it
/// was not running: a phone locked, a process put on hold, a webview that
/// held every task back. The heartbeat comes every quarter second, and a
/// busy page delays one by far less.
const HEARTBEAT_GAP_MS: u64 = 2_000;

/// A session in progress: the machine that decides it, the stations it is
/// sent by, and its hold on the audio.
pub struct LiveSession {
    machine: SessionMachine<Sampler>,
    /// Where every group's stations are drawn from. Drawn afresh for each
    /// session, so no two sessions — in one launch or across launches — hear
    /// the same operators.
    stations: SplitMix64,
    lease: Lease,
    /// When the app was last seen running the session: its last heartbeat,
    /// or the last news it heard.
    last_seen: MonoMs,
    /// The local day it started: the day it counts towards, though it may
    /// end after midnight.
    started_on: String,
    /// How it is marked open, for what it has done to be added to.
    mark: OpenSession,
}

impl LiveSession {
    /// How many of its groups are answered.
    fn answered(&self) -> usize {
        let session = self.machine.session();
        (0..session.group_count())
            .filter(|&index| session.group(index).is_some_and(Group::confirmed))
            .count()
    }

    /// What the session would keep were it ended now, as End session keeps
    /// it, and the settings auto level judges it by.
    fn so_far(&self) -> SoFar {
        let session = self.machine.session();
        SoFar {
            result: session.result(now_ms(), self.started_on.clone()),
            settings: Some(session.settings().clone()),
        }
    }

    /// Whether what comes under `lease` is this session's to hear: its own
    /// claim, while it holds the audio, or while it is paused, having given
    /// the audio back.
    fn hears(&self, lease: &Lease) -> bool {
        self.lease.is(lease) && (lease.is_current() || self.machine.session().paused())
    }

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
/// It keeps when each character went in, key by key, as the box is typed
/// into: the text waits out the pause, but recognition is timed to the keys,
/// and the moments travel with the text.
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
    /// The group the box was last typed into, and when each of its
    /// characters went in.
    typing: Option<usize>,
    keys: KeyLog,
    entry: Option<Entry>,
}

/// What the box held when it was handed over: group `index`'s answer so far,
/// and its keys.
#[derive(Clone, Debug, PartialEq)]
pub struct TypedAnswer {
    pub index: usize,
    pub text: String,
    pub keys: Keys<MonoMs>,
}

impl From<TypedAnswer> for SessionEvent {
    fn from(typed: TypedAnswer) -> Self {
        SessionEvent::Input {
            index: typed.index,
            text: typed.text,
            keys: typed.keys,
        }
    }
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

    /// Group `index`'s box now holds `text`, not handed over yet: what is
    /// new in it was keyed at `at`, or typed for you, untimed, at `None`.
    pub fn hold(mut self, index: usize, text: String, at: Option<MonoMs>) {
        let mut outbox = self.0.write();
        if outbox.typing != Some(index) {
            outbox.typing = Some(index);
            outbox.keys.clear();
        }
        outbox.keys.typed(answer_chars(&text), at);
        outbox.waiting = Some((index, text));
    }

    /// What group `index`'s box holds that is not handed over yet, if any.
    pub fn waiting_for(self, index: usize) -> Option<String> {
        let outbox = self.0.peek();
        let (at, text) = outbox.waiting.as_ref()?;
        (*at == index).then(|| text.clone())
    }

    /// Whatever is waiting, now handed over.
    pub fn take(mut self) -> Option<TypedAnswer> {
        let mut outbox = self.0.write();
        let (index, text) = outbox.waiting.take()?;
        let keys = outbox.keys.keys();
        Some(TypedAnswer { index, text, keys })
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
        outbox.typing = None;
        outbox.keys.clear();
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
    let clamped = settings.peek().clone().clamp();
    if *settings.peek() != clamped {
        settings.set(clamped);
    }
    // Stored now, with whatever the press that starts it kept, a call typed
    // into Home's hero among it: nothing is stored while the session runs,
    // so a reload mid-run would lose them otherwise. What another window
    // stored meanwhile comes in with them, and the session runs on that.
    let fitted = ctx.store_settings();
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
    let now = MonoMs(mono_ms());
    let (machine, effects) =
        SessionMachine::start(SessionId::new(lease.turn()), now, fitted, sampler);
    runtime.set(Some(machine.session().clone()));
    screen.set(Screen::Training);
    let mark = ctx.mark_running(false);
    *app.session.borrow_mut() = Some(Live::Groups(Box::new(LiveSession {
        machine,
        stations: SplitMix64::new(seed_rng()),
        lease: lease.clone(),
        last_seen: now,
        started_on: local_date_string(),
        mark,
    })));
    beat(ctx, lease.clone());
    run_effects(ctx, &lease, effects);
}

/// While `lease` holds the audio, say every [`HEARTBEAT_MS`] that the app is
/// still running the session. A pause gives the audio back, and the beat
/// stops with it; Resume starts it again under the new claim.
///
/// It is the session's own task, on the root scope, so it beats whatever
/// screen is up, and stops with the session, the way its timers do.
fn beat(ctx: AppCtx, lease: Lease) {
    spawn_forever(async move {
        while lease.sleep(HEARTBEAT_MS).await.is_ok() {
            pause_if_away(ctx, &lease);
        }
    });
}

/// Before the session hears anything, and on every heartbeat: a gap since
/// the app was last seen running it, longer than any heartbeat takes, is
/// time the app was not running, and the session is paused from when it
/// was last seen, as it would have been had it heard the page go.
///
/// A webview whose render waits on a frame a hidden page never gets holds
/// back every task the app has, and a frozen process holds back everything;
/// either way the page's word that it was hidden comes in after the timers
/// that came due in the gap, which would have closed answers nobody was
/// there to type, and sent groups to nobody. Here the first of them to run
/// pauses the session, and they find it paused. Typing still waiting out
/// the box's pause came before the gap, so the session hears it first.
fn pause_if_away(ctx: AppCtx, lease: &Lease) {
    let mut runtime = ctx.runtime;
    let app = ctx.app();
    let effects = {
        let mut slot = app.session.borrow_mut();
        let Some(live) = slot
            .as_mut()
            .and_then(Live::groups_mut)
            .filter(|live| live.hears(lease))
        else {
            return;
        };
        let now = MonoMs(mono_ms());
        let seen = std::mem::replace(&mut live.last_seen, now);
        if now.since(seen) <= HEARTBEAT_GAP_MS || live.machine.session().paused() {
            return;
        }
        let mut effects = ctx
            .answer
            .take()
            .map_or_else(Vec::new, |typed| live.machine.apply(typed.into(), seen));
        effects.extend(live.machine.apply(SessionEvent::Pause, seen));
        let session = live.machine.session();
        if runtime.peek().as_ref() != Some(session) {
            runtime.set(Some(session.clone()));
        }
        effects
    };
    run_effects(ctx, lease, effects);
}

/// Carry on after a pause. The audio a pause gave back is claimed afresh
/// here, inside the tap on Resume, so the player opens where a browser lets
/// it start; the session goes on under the new claim. A session that is not
/// paused is left alone: a claim would cut its send from under it.
pub fn resume_session(ctx: AppCtx) {
    let app = ctx.app();
    let settings = match app.session.borrow().as_ref().and_then(Live::groups) {
        Some(live) if live.machine.session().paused() => live.machine.session().settings().clone(),
        _ => return,
    };
    let lease = match app.player.claim(&settings) {
        Ok(lease) => lease,
        Err(err) => {
            ctx.toasts.show(err.to_string());
            return;
        }
    };
    if let Some(live) = app.session.borrow_mut().as_mut().and_then(Live::groups_mut) {
        live.lease = lease.clone();
    }
    dispatch(ctx, &lease, SessionEvent::Resume);
    beat(ctx, lease);
}

/// Hand the running session something the user did. Nothing happens when
/// no session is running. Resume claims the audio first
/// ([`resume_session`]).
pub fn send_event(ctx: AppCtx, event: SessionEvent) {
    if event == SessionEvent::Resume {
        resume_session(ctx);
        return;
    }
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

/// Hold whatever has the training screen, as the system's Back does first:
/// a run pauses as its Pause pauses it, a session of groups as it does out
/// of sight. Whether this held it: one already paused, or over, is the
/// caller's to end.
#[cfg(any(test, feature = "web", target_os = "android"))]
pub fn hold_run(ctx: AppCtx) -> bool {
    let held = |ctx: AppCtx| match ctx.app().session.borrow().as_ref() {
        Some(Live::Contest(_)) => ctx
            .contest
            .peek()
            .as_ref()
            .is_some_and(|run| matches!(run.phase, cw_core::ContestPhase::Paused { .. })),
        Some(Live::Groups(live)) => live.machine.session().paused(),
        None => false,
    };
    if held(ctx) {
        return false;
    }
    if contest_live(ctx) {
        crate::contest_runtime::pause_run(ctx, cw_core::PauseWhy::You);
    } else {
        send_event(ctx, SessionEvent::Hold);
    }
    held(ctx)
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
/// rather than handed to a machine it means nothing to. A paused session
/// has given its audio back, and still hears what is done to it.
///
/// News that waited out a gap the app was not running in finds the session
/// paused from the start of the gap ([`pause_if_away`]).
fn dispatch(ctx: AppCtx, lease: &Lease, event: SessionEvent) {
    pause_if_away(ctx, lease);
    let mut runtime = ctx.runtime;
    let app = ctx.app();
    let closes = app
        .session
        .borrow()
        .as_ref()
        .and_then(Live::groups)
        .and_then(|live| {
            live.hears(lease)
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
    let (effects, so_far) = {
        let mut slot = app.session.borrow_mut();
        let Some(live) = slot
            .as_mut()
            .and_then(Live::groups_mut)
            .filter(|live| live.hears(lease))
        else {
            return;
        };
        let now = MonoMs(mono_ms());
        let answered = live.answered();
        // Typing still waiting out the box's pause came before this event,
        // so the machine hears it first.
        let mut effects = ctx
            .answer
            .take()
            .map_or_else(Vec::new, |typed| live.machine.apply(typed.into(), now));
        effects.extend(live.machine.apply(event, now));
        // A group answered, and the session going on: what it would keep
        // now goes with its mark, should the app stop under it.
        let so_far = (live.answered() > answered && !live.machine.is_terminal())
            .then(|| (live.mark.clone(), live.so_far()));
        // Much of what arrives changes nothing — a key while the box is
        // locked, a timer the machine has forgotten — and publishing it
        // anyway would copy the whole session and redraw the screen for it.
        let session = live.machine.session();
        if runtime.peek().as_ref() != Some(session) {
            runtime.set(Some(session.clone()));
        }
        (effects, so_far)
    };
    if let Some((mark, so_far)) = so_far {
        ctx.mark_so_far(&mark, so_far);
    }
    run_effects(ctx, lease, effects);
}

/// Do what the machine said.
fn run_effects(ctx: AppCtx, lease: &Lease, effects: Vec<SessionEffect>) {
    for effect in effects {
        match effect {
            SessionEffect::StopAudio => lease.stop_sending(),
            SessionEffect::ReleaseAudio => {
                lease.release();
            }
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
            SessionEffect::PersistAndShowResults => {
                finish_session(ctx);
            }
            SessionEffect::AbortToHome => {
                abandon_session(ctx);
            }
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
    let lease = lease.clone();
    spawn_forever(async move {
        // The send's timeline, the moment its first sample goes out: what
        // its characters are timed from, and the speed it went at, however
        // soon the group is answered; whether any of it went unheard, as
        // soon as that is found; and whether it was held up on its way,
        // however it ended.
        let news = |news: SendNews<'_>| {
            let event = match news {
                SendNews::Started(start) => SessionEvent::PlaybackStarted {
                    index,
                    start: start.clone(),
                },
                SendNews::Missed {
                    from_sec,
                    until_sec,
                } => SessionEvent::PlaybackMissed {
                    index,
                    from_sec,
                    until_sec,
                },
                SendNews::HeldUp => SessionEvent::PlaybackHeldUp { index },
            };
            dispatch(ctx, &lease, event);
        };
        let event = match lease.send(&transmission, &settings, news).await {
            Ok(sent) => SessionEvent::PlaybackEnded {
                index,
                duration_sec: sent.duration_sec,
            },
            // Stopped from outside: the session pauses, if it had not moved
            // on, and there is nothing to say.
            Err(AudioError::Cancelled) => SessionEvent::PlaybackStopped { index },
            // The sound failed after every retry: the session pauses, as a
            // run does, and says why.
            Err(failure) => {
                ctx.toasts.show(failure.to_string());
                SessionEvent::PlaybackStopped { index }
            }
        };
        dispatch(ctx, &lease, event);
    });
}

/// The session, taken out of the app, with the audio given back. It is
/// marked over once it is stored or let go.
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

/// End the session without keeping it. Whether there was one.
fn abandon_session(ctx: AppCtx) -> bool {
    let AppCtx {
        mut screen,
        mut runtime,
        ..
    } = ctx;
    if end_session(ctx).is_none() {
        return false;
    }
    ctx.mark_ended();
    runtime.set(None);
    screen.set(Screen::Home);
    true
}

/// Score the session, move the level if it earned it, store it, and show
/// the results. Whether there was one.
fn finish_session(ctx: AppCtx) -> bool {
    let AppCtx {
        mut runtime,
        mut auto_message,
        ..
    } = ctx;
    let Some(live) = end_session(ctx) else {
        return false;
    };
    let store = ctx.app().store;
    let session = live.machine.session();
    let run = store.load_auto_counters(&CountersKey::of(session.settings()));
    // The machine only finishes a session it has scored something in.
    let done = complete_session(session, run, now_ms(), live.started_on.clone());
    // Held with its mark as it is to be stored, before its level's run is:
    // the app stopping from here on keeps it once, its level counted once.
    ctx.mark_so_far(
        &live.mark,
        SoFar {
            result: done.result.clone(),
            settings: None,
        },
    );
    auto_message.set(settle_level(ctx, &done));
    runtime.set(None);
    keep_and_show(ctx, done.result);
    true
}

/// Store what finishing a session did to its level's run, and move the
/// level if the run earned it, saying so. What it said, if anything.
fn settle_level(ctx: AppCtx, done: &Completion) -> Option<String> {
    let AppCtx {
        mut settings,
        toasts,
        ..
    } = ctx;
    let store = ctx.app().store;
    match &done.counters {
        CountersUpdate::Keep(key, counters) => store.save_auto_counters(key, *counters),
        CountersUpdate::Clear(keys) => store.clear_auto_counters(keys),
    }
    let message = done
        .change
        .as_ref()
        .map(crate::ui::auto_level::change_message);
    if let Some(change) = &done.change {
        // Onto the settings as they are now, not as the session started
        // with them, so the level is all it moves. Stored by the app's
        // settings effect.
        apply_auto_level(&mut settings.write(), change);
    }
    if let Some(message) = &message {
        toasts.show(message.clone());
    }
    message
}

/// Keep what the sessions and runs the app stopped under had done by then,
/// `kept`, as Exit would have kept each ([`AppCtx::tell_of_cut_off`]): in
/// the history, where each falls by when it was last added to, and a
/// session of groups counted towards its level's run, the level moved if
/// the run earned it. One the app stopped under as it was stored, its mark
/// holding it as stored, is in the history already, and kept once.
pub fn keep_cut_off(ctx: AppCtx, kept: Vec<SoFar>) {
    if kept.is_empty() {
        return;
    }
    let mut sessions = ctx.sessions;
    let store = ctx.app().store;
    for so_far in kept {
        let mut result = so_far.result;
        // A session ended and stored as the app stopped, its mark still
        // held with it as stored: kept already, and counted.
        let key = SessionKey::of(&result);
        if sessions
            .peek()
            .iter()
            .any(|session| SessionKey::of(session) == key)
        {
            continue;
        }
        // On today's scale, as every session of the history is read.
        result.score = result.points();
        if let Some(settings) = so_far.settings.map(TrainingSettings::clamp) {
            let run = store.load_auto_counters(&CountersKey::of(&settings));
            let done = complete_result(result, &settings, run);
            settle_level(ctx, &done);
            result = done.result;
        }
        let mut history = sessions.write();
        let at = history.partition_point(|session| session.timestamp <= result.timestamp);
        history.insert(at, result);
    }
    let saved = ctx.store_history();
    store.ask_to_keep();
    if let Some(problem) = saved.problem() {
        let mut notices = ctx.notices;
        notices.write().push(Notice::History(problem));
    }
}

/// Add `finished` to the history, store it, and show it on the results
/// screen: a session of groups and a contest run alike. It is marked over
/// once it is stored, so the app stopping before then still tells of it.
pub fn keep_and_show(ctx: AppCtx, finished: SessionResult) {
    let AppCtx {
        mut screen,
        mut result,
        mut sessions,
        mut notices,
        ..
    } = ctx;
    // The history grows by one where it is, and is stored from there.
    sessions.write().push(finished.clone());
    let saved = ctx.store_history();
    ctx.mark_ended();
    // A browser may clear its store to make room; there is now something in
    // it worth asking it to keep.
    ctx.app().store.ask_to_keep();
    // Up until it is read: a session not stored is a session lost.
    if let Some(problem) = saved.problem() {
        notices.write().push(Notice::History(problem));
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

    /// A session is dated the local day it started, so one that runs past
    /// midnight keeps that day's place in the streak, not the next day's.
    #[test]
    fn a_session_is_dated_the_day_it_started() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            if let Some(Live::Groups(live)) = h.app.session.borrow_mut().as_mut() {
                live.started_on = "2026-10-27".into();
            }
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let stored = h.store.load_sessions();
            assert_eq!(
                stored
                    .iter()
                    .map(|session| session.date.as_str())
                    .collect::<Vec<_>>(),
                ["2026-10-27"]
            );
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
            h.ctx.answer.hold(0, "K".into(), Some(MonoMs(mono_ms())));
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

    /// One-character group 0, a box shut while sending, its sound reaching
    /// the ear `latency_ms` after it goes out, keyed 300 ms after the box
    /// is seen open: `check` is handed its recognition time, and what was
    /// sent, while the test still runs, so that a failure says its seed.
    fn keyed_after_the_box_opens(latency_ms: u32, check: impl FnOnce(u32, &str)) {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.min_group_size = 1;
            settings.curriculum.max_group_size = 1;
            let mut h = Harness::with_settings(settings);
            h.recorder.latency_sec.set(f64::from(latency_ms) / 1_000.0);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            h.advance(300).await;
            let sent = h.sent(0);
            h.ctx.answer.hold(0, sent.clone(), Some(MonoMs(mono_ms())));
            h.send(SessionEvent::Confirm);
            let session = h.ctx.runtime.peek().clone().expect("the session");
            let result = session.result(0, "2026-10-24".into());
            let (group, timing) = (&result.groups[0], &result.group_timings[0]);
            let times = timing.char_times.as_ref().expect("its sound kept");
            let timed = cw_core::recognise(&group.sent, &group.received, times)[0].expect("timed");
            check(timed, &sent);
        });
    }

    /// Typed along with the box open, each character of a group is timed
    /// from where it finished sounding, gone to the output, to its key.
    #[test]
    fn typing_along_times_each_character_from_its_end() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.lock_input_during_group_playback = false;
            settings.curriculum.min_group_size = 3;
            settings.curriculum.max_group_size = 3;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(
                h.run_until(4_000, |h| !h.recorder.starts().is_empty())
                    .await
            );
            let started = h.recorder.starts()[0].clone();
            let ends: Vec<u64> = started
                .plan
                .wanted
                .chars
                .iter()
                .map(|span| started.at.0 + (span.end_sec * 1_000.0).round() as u64)
                .collect();
            let sent = h.sent(0);
            assert_eq!(ends.len(), sent.chars().count());
            for (typed, end) in ends.iter().enumerate() {
                let key = end + 250;
                h.advance(key.saturating_sub(mono_ms())).await;
                let text: String = sent.chars().take(typed + 1).collect();
                h.ctx.answer.hold(0, text, Some(MonoMs(mono_ms())));
            }
            h.send(SessionEvent::Confirm);
            let session = h.ctx.runtime.peek().clone().expect("the session");
            let result = session.result(0, "2026-10-27".into());
            let (group, timing) = (&result.groups[0], &result.group_timings[0]);
            let times = timing.char_times.as_ref().expect("its sound kept");
            assert!(times.box_open);
            // The harness's clock moves in steps, so a key may land a few
            // milliseconds after the moment it was due.
            let took = cw_core::recognise(&group.sent, &group.received, times);
            assert_eq!(took.len(), 3, "{times:?}");
            for took in took {
                assert!(
                    took.is_some_and(|took| (250..=255).contains(&took)),
                    "{times:?}"
                );
            }
        });
    }

    /// Typed whole only once the group had been sent, the box open, its
    /// first key 400 ms after the group ended: nothing is timed, but what
    /// was keyed is kept, and says how late the first key came.
    #[test]
    fn typing_after_the_group_keeps_how_late_its_first_key_came() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.lock_input_during_group_playback = false;
            settings.curriculum.min_group_size = 3;
            settings.curriculum.max_group_size = 3;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(
                h.run_until(4_000, |h| !h.recorder.starts().is_empty())
                    .await
            );
            let started = h.recorder.starts()[0].clone();
            let end = started
                .plan
                .wanted
                .chars
                .last()
                .map(|span| started.at.0 + (span.end_sec * 1_000.0).round() as u64)
                .expect("a character");
            let sent = h.sent(0);
            for typed in 1..=sent.chars().count() {
                let key = end + 400 + 150 * (typed as u64 - 1);
                h.advance(key.saturating_sub(mono_ms())).await;
                let text: String = sent.chars().take(typed).collect();
                h.ctx.answer.hold(0, text, Some(MonoMs(mono_ms())));
            }
            h.send(SessionEvent::Confirm);
            let session = h.ctx.runtime.peek().clone().expect("the session");
            let result = session.result(0, "2026-10-30".into());
            let (group, timing) = (&result.groups[0], &result.group_timings[0]);
            let times = timing.char_times.as_ref().expect("its keys kept");
            assert_eq!(
                cw_core::recognise(&group.sent, &group.received, times),
                [None, None, None]
            );
            // The harness's clock moves in steps, so a key may land a few
            // milliseconds after the moment it was due.
            let late = cw_core::first_key_late_ms(times).expect("a first key");
            assert!(
                (400..=400 + i64::from(crate::time::POLL_MS)).contains(&late),
                "{late} ms: {times:?}"
            );
            let untimed = cw_core::session_untimed(&result);
            assert_eq!(untimed.first_key_late_ms, Some(late as f64));
            assert_eq!(untimed.held_up, 0);
        });
    }

    /// Every send held up on its way: no group is timed, and each says
    /// why.
    #[test]
    fn a_send_held_up_on_its_way_leaves_its_group_untimed_and_says_why() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.lock_input_during_group_playback = false;
            let mut h = Harness::with_settings(settings);
            h.recorder.set(Behaviour::Late);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.ctx.answer.hold(0, sent, Some(MonoMs(mono_ms())));
            h.send(SessionEvent::Confirm);
            let session = h.ctx.runtime.peek().clone().expect("the session");
            let timing = &session.result(0, "2026-10-30".into()).group_timings[0];
            assert_eq!(
                (timing.char_times.clone(), timing.untimed),
                (None, Some(cw_core::Untimed::HeldUp))
            );
        });
    }

    /// A character typed as soon as it is heard is timed from where it
    /// finished sounding, as the audio plan has it, gone to the output, to
    /// its key in the box: one-character groups, which a box shut while
    /// sending can time. A device that says its sound reaches the ear later
    /// opens the box that much later, and the time is that much longer, as
    /// the delay is in every time alike.
    #[test]
    fn a_key_is_timed_from_the_end_of_its_character() {
        // Keyed 300 ms after the box was seen open, which is up to a poll
        // of the send's wait, and one of this harness's, after the sound
        // ended.
        let seen_late = 2 * crate::time::POLL_MS;
        for latency in [0, 200] {
            let from = 300 + latency;
            keyed_after_the_box_opens(latency, |took, sent| {
                assert!(
                    (from..=from + seen_late).contains(&took),
                    "{took} ms for {sent}, {latency} ms to the ear"
                );
            });
        }
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

    /// The page going out of sight pauses a session: its group stops, and
    /// nothing moves it on however long nobody is there, so no answer
    /// closes empty. Resumed, it counts down and sends the group again.
    #[test]
    fn a_session_paused_while_hidden_closes_no_answer_and_resumes() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_timeout = 1.0;
            settings.playback.countdown_sec = 2;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.texts().len() == 1).await);
            h.send(SessionEvent::Pause);
            assert_eq!(
                h.phase(),
                Some(SessionPhase::Paused {
                    index: 0,
                    why: cw_core::PauseWhy::Hidden
                })
            );
            h.advance(60_000).await;
            assert_eq!(
                h.phase(),
                Some(SessionPhase::Paused {
                    index: 0,
                    why: cw_core::PauseWhy::Hidden
                })
            );
            assert_eq!(h.texts().len(), 1, "sent while paused");
            assert_eq!(input_of(&h, 0).as_deref(), Some(""));
            h.send(SessionEvent::Resume);
            assert_eq!(h.phase(), Some(SessionPhase::Countdown { remaining: 2 }));
            assert!(h.run_until(3_000, |h| h.texts().len() == 2).await);
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
        });
    }

    /// A webview can hold back every task the app has — a render waiting
    /// on a frame a hidden page never gets, a phone's process frozen whole
    /// — while the clock runs on. Let go, the timers that came due in the
    /// gap fire before the page's word that it was hidden can land. The
    /// session is paused from when it was last seen running, so the answer
    /// window that ran out meanwhile closes nothing, and the gap is no time
    /// spent answering.
    #[test]
    fn a_gap_the_app_was_not_running_in_pauses_the_session_from_its_start() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_timeout = 1.0;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let seen = mono_ms();
            // Held back: the clock runs a minute, and nothing is polled.
            tokio::time::advance(std::time::Duration::from_secs(60)).await;
            tokio::task::yield_now().await;
            h.pump();
            // The page's word, at last.
            h.send(SessionEvent::Pause);
            assert_eq!(
                h.phase(),
                Some(SessionPhase::Paused {
                    index: 0,
                    why: cw_core::PauseWhy::Hidden
                })
            );
            assert_eq!(h.texts().len(), 1, "the next group was sent");
            let confirmed = h.ctx.runtime.peek().as_ref().map(|s| s.view().confirmed);
            assert_eq!(
                confirmed,
                Some(vec![false, false]),
                "an answer closed empty"
            );
            assert!(
                matches!(h.calls().last(), Some(Call::Shutdown)),
                "the audio was not given back: {:?}",
                h.calls()
            );
            // Resumed, the group is waited on again, with a fresh window,
            // and answered: the minute away is not its answering time.
            h.send(SessionEvent::Resume);
            assert_eq!(h.awaiting_answer(), Some(0));
            h.advance(400).await;
            let text = h.sent(0);
            h.type_answer(0, &text);
            h.send(SessionEvent::Confirm);
            assert!(mono_ms() - seen > 60_000);
            let session = h.ctx.runtime.peek().clone().expect("a session");
            assert!(session.group(0).is_some_and(|g| g.confirmed()));
            let timing = &session.result(0, String::new()).group_timings[0];
            assert!(
                timing.time_to_complete_ms < 2_000.0,
                "the gap counted as answering: {} ms",
                timing.time_to_complete_ms
            );
        });
    }

    /// A session that runs on, with nothing held back, never pauses
    /// itself, however long it waits on a send: its heartbeat goes on
    /// under it.
    #[test]
    fn a_session_running_on_never_pauses_itself() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_timeout = 0.0;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            h.advance(30_000).await;
            assert_eq!(h.awaiting_answer(), Some(0));
        });
    }

    /// End session while paused keeps what was typed and stores the session,
    /// though the audio was given back with the pause.
    #[test]
    fn ending_a_paused_session_keeps_it() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.lock_input_during_group_playback = false;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.texts().len() == 1).await);
            h.send(SessionEvent::Input {
                index: 0,
                text: "K".into(),
                keys: Default::default(),
            });
            h.send(SessionEvent::Pause);
            assert_eq!(
                h.phase(),
                Some(SessionPhase::Paused {
                    index: 0,
                    why: cw_core::PauseWhy::Hidden
                })
            );
            h.send(SessionEvent::FinishNow);
            assert_eq!(h.phase(), None, "the session is over");
            assert_eq!(h.ctx.sessions.peek().len(), 1, "and kept");
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
                h.ctx.answer.hold(0, "K".into(), Some(MonoMs(mono_ms())));
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
            h.ctx.answer.hold(0, "K".into(), Some(MonoMs(mono_ms())));
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
                        answer.hold(0, "T".into(), Some(MonoMs(mono_ms())));
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
            // three attempts the session pauses rather than pretending.
            assert!(
                h.run_until(60_000, |h| h.phase() == Some(sound_stopped(0)))
                    .await,
                "a stalled player should pause the session, not hang"
            );
            assert!(h.recorder.players_built.get() > 1, "the player is rebuilt");
            assert_eq!(h.toast().as_deref(), Some("Audio playback stalled."));
            assert_eq!(h.screen(), Screen::Training);
            assert!(h.ctx.sessions.peek().is_empty());
        });
    }

    /// The session paused on group `index` because its sound stopped.
    fn sound_stopped(index: usize) -> SessionPhase {
        SessionPhase::Paused {
            index,
            why: cw_core::PauseWhy::Sound,
        }
    }

    /// A session whose sound stops pauses, as a run does, rather than end:
    /// on a phone a call or Siri takes the sound for a while, and gives it
    /// back. Nothing answered is lost, and nothing is tried again behind
    /// the learner's back; Resume opens a fresh player from inside its own
    /// tap, where a browser lets one start, and the group goes out again.
    #[test]
    fn a_session_its_sound_stops_pauses_and_resume_brings_the_sound_back() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            h.set_behaviour(Behaviour::Stall);
            assert!(
                h.run_until(60_000, |h| h.phase() == Some(sound_stopped(1)))
                    .await
            );
            assert_eq!(h.toast().as_deref(), Some("Audio playback stalled."));
            assert!(!h.lease().is_current(), "the audio was given back");
            let sends = h.texts().len();
            let built = h.recorder.players_built.get();
            h.advance(30_000).await;
            assert_eq!(h.texts().len(), sends, "sent while paused");
            assert_eq!(h.recorder.players_built.get(), built, "opened untapped");
            assert_eq!(h.screen(), Screen::Training);

            // The sound is back; the tap on Resume opens a player for it.
            h.set_behaviour(Behaviour::Play);
            h.send(SessionEvent::Resume);
            assert_eq!(h.recorder.players_built.get(), built + 1);
            assert_eq!(h.texts().len(), sends + 1, "the group goes out again");
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(result.groups.len(), 2, "both groups answered");
        });
    }

    /// A session is marked open from its start until it ends, however it
    /// ends: one the app stops under is told of when it next opens.
    #[test]
    fn a_session_is_marked_open_while_it_runs() {
        run(|| async {
            let mut h = Harness::new();
            assert_eq!(h.store.open(), None);
            h.start_training();
            let open = h.store.open().expect("marked open");
            assert!(!open.run);
            h.send(SessionEvent::Abort);
            assert_eq!(h.store.open(), None, "discarded");

            h.start_training();
            assert!(h.store.open().is_some());
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.store.open(), None, "kept");
        });
    }

    /// Each group answered puts what the session would keep, were it ended
    /// then, with its mark: the history is never written mid-session, and
    /// the mark goes once the session ends.
    #[test]
    fn what_a_session_would_keep_goes_with_its_mark_as_each_group_is_answered() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 3;
            let mut h = Harness::with_settings(settings.clone());
            h.start_training();
            let kept_so_far = |h: &Harness| {
                h.store
                    .open()
                    .and_then(|open| open.so_far)
                    .map(|so_far| (so_far.result.groups.len(), so_far.settings))
            };
            assert_eq!(kept_so_far(&h), None, "nothing answered yet");
            for index in 0..2 {
                assert!(
                    h.run_until(4_000, |h| h.awaiting_answer() == Some(index))
                        .await
                );
                let sent = h.sent(index);
                h.type_answer(index, &sent);
                assert!(
                    h.run_until(1_000, |h| kept_so_far(h)
                        .is_some_and(|(groups, _)| groups == index + 1))
                        .await,
                    "{:?}",
                    kept_so_far(&h)
                );
            }
            let (_, judged_by) = kept_so_far(&h).expect("kept so far");
            assert_eq!(judged_by, Some(settings.clamp()));
            assert!(h.store.load_sessions().is_empty(), "stored mid-session");
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.store.open(), None);
            assert_eq!(h.store.load_sessions()[0].groups.len(), 3);
        });
    }

    /// Leaving a session that has already gone leaves the screen as it is.
    #[test]
    fn leaving_a_session_that_has_gone_changes_nothing() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let ctx = h.ctx;
            assert!(!h.in_app(|| abandon_session(ctx)));
            assert!(!h.in_app(|| finish_session(ctx)));
            assert_eq!(h.screen(), Screen::Results);
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

    /// A send its stream skipped some of, the page stalled for a while, is
    /// no group heard: the group goes out again from the start once the
    /// send ends, and only then is its answer waited for; nothing has gone
    /// wrong, so nothing is said. A send only parked on its way, heard
    /// whole, is answered as ever.
    #[test]
    fn a_group_heard_in_part_is_sent_again_before_it_is_answered() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let mut h = Harness::with_settings(settings);
            h.recorder.skip_next.set(Some((0.02, 0.06)));
            h.start_training();
            let sent = h.sent(0);
            let mut answer_opened_early = false;
            assert!(
                h.run_until(10_000, |h| {
                    answer_opened_early |= h.awaiting_answer().is_some();
                    h.texts().len() == 2
                })
                .await,
                "sent again"
            );
            assert!(
                !answer_opened_early,
                "the answer waited for the group heard whole"
            );
            assert_eq!(h.texts(), vec![sent; 2], "the same group, from the start");
            assert!(h.run_until(5_000, |h| h.awaiting_answer() == Some(0)).await);
            assert_eq!(h.toast(), None, "nothing has gone wrong");
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.texts().len(), 2, "heard whole the second time");
        });
    }

    /// A group its stream skipped part of goes out again with no count
    /// down, whatever the settings ask for, and with the lock off its box
    /// stays open throughout: what was typed along the lost send is kept,
    /// and the group is answered as soon as the rest is typed. Its first
    /// send held up, it is untimed.
    #[test]
    fn a_group_heard_in_part_never_counts_down_nor_locks_the_box() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            settings.playback.countdown_sec = 3;
            settings.playback.lock_input_during_group_playback = false;
            let mut h = Harness::with_settings(settings);
            // Over its first character, which starts as the send does.
            h.recorder.skip_next.set(Some((0.02, 0.06)));
            h.start_training();
            let playing = |h: &Harness| h.phase() == Some(SessionPhase::Playing { index: 0 });
            assert!(h.run_until(5_000, playing).await, "the first count down");
            h.advance(40).await;
            let sent = h.sent(0);
            let first: String = sent.chars().take(1).collect();
            h.type_answer(0, &first);
            let (mut counted_down, mut locked) = (false, false);
            assert!(
                h.run_until(10_000, |h| {
                    counted_down |= matches!(h.phase(), Some(SessionPhase::Countdown { .. }));
                    locked |= h
                        .ctx
                        .runtime
                        .peek()
                        .as_ref()
                        .is_some_and(|s| s.view().locked);
                    h.texts().len() == 2
                })
                .await,
                "sent again"
            );
            assert!(!counted_down, "counted down before it went out again");
            assert!(!locked, "the box locked while it was sent again");
            assert_eq!(h.texts(), vec![sent.clone(); 2], "the same group");
            let typed = h
                .ctx
                .runtime
                .peek()
                .as_ref()
                .and_then(|s| s.group(0).map(|g| g.input().to_string()));
            assert_eq!(typed, Some(first), "what was typed is kept");
            h.type_answer(0, &sent);
            assert!(h.run_until(5_000, |h| h.screen() == Screen::Results).await);
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(
                result.group_timings[0].untimed,
                Some(cw_core::Untimed::HeldUp)
            );
        });
    }

    /// A sliver of a send skipped, too short to lose any of its elements,
    /// sends nothing again: the answer window opens after the one send.
    /// Held up on its way all the same, the group is untimed.
    #[test]
    fn a_sliver_skipped_is_no_resend() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let mut h = Harness::with_settings(settings);
            h.recorder.skip_next.set(Some((0.0, 0.0005)));
            h.start_training();
            assert!(h.run_until(5_000, |h| h.awaiting_answer() == Some(0)).await);
            assert_eq!(h.texts().len(), 1, "sent once");
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.texts().len(), 1, "and only once");
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(
                result.group_timings[0].untimed,
                Some(cw_core::Untimed::HeldUp)
            );
        });
    }

    /// A stream that skips part of every send has the group sent again
    /// twice, then waits for its answer all the same: a page that keeps
    /// falling behind never keeps a group from being answered, nor its box
    /// locked for ever.
    #[test]
    fn a_group_skipped_every_time_is_answered_after_two_more_sends() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let mut h = Harness::with_settings(settings);
            h.recorder.skip_every.set(Some((0.02, 0.06)));
            h.start_training();
            let sent = h.sent(0);
            assert!(
                h.run_until(15_000, |h| h.awaiting_answer() == Some(0))
                    .await,
                "the answer window, after all"
            );
            assert_eq!(h.texts(), vec![sent; 3], "sent, then sent again twice");
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.texts().len(), 3, "and no more");
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
    fn a_broken_stream_is_retried_before_the_session_is_paused() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::FailMidSend);
            h.start_training();
            assert!(
                h.run_until(30_000, |h| h.phase() == Some(sound_stopped(0)))
                    .await
            );
            // Three attempts, each on a freshly built player.
            assert_eq!(h.texts().len(), 3);
            assert!(h.recorder.players_built.get() >= 3);
            assert!(h.toast().is_some(), "the failure is said");
        });
    }

    #[test]
    fn a_player_that_refuses_to_start_is_reported_once_the_retries_run_out() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::RefuseToStart);
            h.start_training();
            assert!(
                h.run_until(30_000, |h| h.phase() == Some(sound_stopped(0)))
                    .await
            );
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

    /// A hard audio failure after something has been scored keeps the
    /// score: the session pauses, and End session shows it.
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
            assert!(
                h.run_until(30_000, |h| h.phase() == Some(sound_stopped(1)))
                    .await
            );
            h.send(SessionEvent::FinishNow);
            assert_eq!(h.screen(), Screen::Results);
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
    /// like from inside: the send reports cancelled and the session pauses,
    /// with nothing to say, rather than sitting in silence.
    #[test]
    fn a_send_stopped_from_outside_pauses_the_session() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(100).await;
            h.lease().stop_sending();
            assert!(
                h.run_until(5_000, |h| h.phase() == Some(sound_stopped(0)))
                    .await
            );
            assert_eq!(h.toast(), None);
            assert_eq!(h.screen(), Screen::Training);
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

    /// The level is all a session that earns one moves: a setting changed
    /// while it ran is kept, where the level used to come back on the
    /// settings the session started with, and take the change away.
    #[test]
    fn a_level_earned_moves_the_level_and_nothing_else() {
        run(|| async {
            let mut settings = test_settings();
            settings.auto_level.auto_adjust_level = true;
            settings.auto_level.auto_adjust_above_threshold_count = 1;
            settings.auto_level.auto_adjust_threshold = 50.0;
            settings.curriculum.level = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.ctx.settings.write().band.master_volume = 0.5;
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let after = h.ctx.settings.peek().clone();
            assert_eq!(after.curriculum.level, 2);
            assert_eq!(after.band.master_volume, 0.5, "the change was taken away");
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
                history_said(&h)
                    .is_some_and(|text| text.contains("newest 2") && !text.contains("contest")),
                "{:?}",
                h.notices()
            );
        });
    }

    /// What the history said of the session just kept, up until it is read.
    fn history_said(h: &Harness) -> Option<String> {
        h.notices().into_iter().find_map(|notice| match notice {
            Notice::History(said) => Some(said),
            _ => None,
        })
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
                history_said(&h).is_some_and(|text| text.contains("newest 2")
                    && text.ends_with("Long contest runs take more room.")),
                "{:?}",
                h.notices()
            );
        });
    }

    /// A history the store could not read is not written over, and the user
    /// is told the session was not kept, until they have read it: a session
    /// not stored is a session lost.
    #[test]
    fn a_session_is_not_stored_over_a_history_that_could_not_be_read() {
        run(|| async {
            let mut h = Harness::new();
            h.store.held.set(true);
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results, "the results are still shown");
            assert!(h.store.load_sessions().is_empty());
            h.advance(u64::from(crate::state::ACTION_TOAST_MS) * 2)
                .await;
            assert!(
                history_said(&h).is_some_and(|text| text.contains("could not be read")),
                "{:?}",
                h.notices()
            );
            assert_eq!(h.store.open(), None, "told of, so no longer open");
        });
    }
}
