//! Runs a contest: the machine decides, and this carries out what it decides
//! — turns, timers, the results — under the run's lease on the audio. The
//! run screen hands it keys, with the fields as typed, and says when the
//! page has gone out of sight.
//!
//! As for a session of groups ([`crate::session_runtime`]), its tasks are
//! spawned on the root scope, and the lease, not a component, decides how
//! long they live. A pause gives the audio back altogether, so the band
//! falls silent and the screen may sleep; Resume claims it again inside the
//! tap, where a browser lets a player start.

use std::borrow::Cow;

use cw_core::{
    ContestEffect, ContestEvent, ContestMachine, ContestPhase, ContestView, EntryField, MonoMs,
    OpKey, PauseWhy, Sampler, SessionResult, TrainingSettings, Turn, plan_turn,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use crate::live_run::{self, RunMachine};
use crate::page::{Caret, control_id, focus_by_id_anywhere};
use crate::persist::SoFar;
use crate::player::{AudioError, Lease, SendNews};
use crate::state::live::{LiveRun, RunOwn, Stage};
use crate::state::{AppCtx, Finished, Live, LiveContest, Preview, Said, Screen};
use crate::time::{local_date_string, local_hour, mono_ms, now_ms, seed_rng};

/// A contest run, as its life knows it ([`live_run`]).
pub(crate) type Run = ContestMachine<Sampler>;

impl RunMachine for Run {
    type Event = ContestEvent;
    type Effect = ContestEffect;
    type View = ContestView;
    type Progress = ();
    type Own = RunOwn;
    /// A run moves no level.
    type Level = ();

    fn of(live: &Live) -> Option<&LiveContest> {
        match live {
            Live::Contest(live) => Some(live),
            Live::Groups(_) => None,
        }
    }

    fn of_mut(live: &mut Live) -> Option<&mut LiveContest> {
        match live {
            Live::Contest(live) => Some(live),
            Live::Groups(_) => None,
        }
    }

    fn unboxed(live: Live) -> Option<LiveContest> {
        match live {
            Live::Contest(live) => Some(*live),
            Live::Groups(_) => None,
        }
    }

    fn shown(ctx: AppCtx) -> Signal<Option<ContestView>> {
        ctx.contest
    }

    fn hear(&mut self, event: ContestEvent, now: MonoMs) -> Vec<ContestEffect> {
        self.apply(event, now)
    }

    fn over(&self) -> bool {
        self.is_terminal()
    }

    fn as_shown(&self) -> Cow<'_, ContestView> {
        Cow::Owned(self.view())
    }

    fn progress(&self) {}

    /// A QSO logged, or its check shown.
    fn progressed(&self, (): (), published: Option<&ContestView>, shown: &ContestView) -> bool {
        published.is_some_and(|before| before.log != shown.log)
    }

    /// What End run would keep: none while it has worked nobody.
    fn so_far(live: &LiveContest, now: MonoMs, finished_at: u64) -> Option<SoFar> {
        let mut ended = live.machine.clone();
        let effects = ended.apply(ContestEvent::FinishNow, now);
        effects
            .contains(&ContestEffect::PersistAndShowResults)
            .then(|| SoFar {
                result: ended.result(finished_at, live.started_on.clone()),
                settings: None,
            })
    }

    /// Its sound has stopped, so what it held for a screen reader is said
    /// now.
    fn ending(ctx: AppCtx, live: &mut LiveContest) {
        say(ctx, std::mem::take(&mut live.own.unsaid));
    }

    /// A session of the history, its log beside its groups, shown as a
    /// session of groups is shown.
    fn kept(_ctx: AppCtx, live: &LiveContest) -> (SessionResult, ()) {
        (live.machine.result(now_ms(), live.started_on.clone()), ())
    }
}

/// Start a run of `settings`, clamped already: the training screen comes up
/// on its countdown, or on its first CQ. A run that cannot start says what
/// it still needs, and nothing changes.
pub fn start_run(ctx: AppCtx, settings: TrainingSettings) {
    if let Some(missing) = settings.contest.missing() {
        ctx.toasts.show(missing);
        return;
    }
    let rules = settings.contest.contest_kind.rules();
    let side = settings.contest.side();
    let AppCtx {
        mut screen,
        contest: mut view,
        mut announced,
        mut preview,
        toasts,
        entry,
        ..
    } = ctx;
    // The audio is claimed with what the run plays from, so every claim and
    // every turn of it has the one band.
    let mut sound = settings.clone();
    sound.band.qsb_enabled = false;
    // The run has the audio now, whether or not it can open it: any preview
    // that had it has stopped.
    preview.set(Preview::Idle);
    let app = ctx.app();
    let Ok(lease) = app
        .player
        .claim(&sound)
        .inspect_err(|err| toasts.show(err.to_string()))
    else {
        return;
    };
    // The callers' calls lean on what the history says is hard, and on what
    // the run copies as it goes.
    let sampler = ctx.sampler(&settings, seed_rng());
    let now = MonoMs(mono_ms());
    let (mut machine, effects) = ContestMachine::start(seed_rng(), now, settings, sampler);
    // Before anything of yours that greets: the first CQ greets nobody.
    machine.set_local_hour(local_hour());
    entry.start(rules, side);
    announced.set(None);
    view.set(Some(machine.view()));
    screen.set(Screen::Training);
    let mark = ctx.mark_running(true);
    *app.session.borrow_mut() = Some(Live::Contest(Box::new(LiveRun {
        machine,
        lease: lease.clone(),
        stage: Stage::Running,
        seen: None,
        started_on: local_date_string(),
        mark,
        own: RunOwn {
            sound,
            unsaid: Vec::new(),
        },
    })));
    run_effects(ctx, &lease, effects);
}

/// Hand the run something the user did: End run, Discard, the page going
/// out of sight. Nothing happens when no run is live. Resume claims the
/// audio first ([`resume_run`]), as a session's does.
pub fn send_run_event(ctx: AppCtx, event: ContestEvent) {
    if event == ContestEvent::Resume {
        resume_run(ctx);
        return;
    }
    if let Some(lease) = live_run::lease::<Run>(ctx) {
        dispatch(ctx, &lease, event);
    }
}

/// A key or a message button: whatever paddle letter is keyed in full goes
/// into its field first, the player is woken inside the press, and the key
/// carries the fields as they are then.
pub fn press_key(ctx: AppCtx, key: OpKey) {
    ctx.entry.finish_letter();
    let Some(lease) = live_run::lease::<Run>(ctx) else {
        return;
    };
    lease.wake();
    let fields = Box::new(ctx.entry.snapshot());
    dispatch(ctx, &lease, ContestEvent::Key { key, fields });
}

/// − / +: your speed, moved by `delta` WPM from your next message on, and
/// kept as your speed for the next run. The player is woken inside the
/// press, as for a key. The setting is stored once the run screen is left,
/// as nothing is stored while a run is on.
pub fn change_speed(ctx: AppCtx, delta: i32) {
    let Some(lease) = live_run::lease::<Run>(ctx) else {
        return;
    };
    lease.wake();
    dispatch(ctx, &lease, ContestEvent::Speed { delta });
    let moved = ctx.contest.peek().as_ref().map(|view| view.my_wpm);
    if let Some(wpm) = moved {
        // Whole already: − / + move it from the whole speed nearest it.
        let wpm = wpm.round() as u32;
        let mut settings = ctx.settings;
        if settings.peek().contest.my_wpm != wpm {
            settings.write().contest.my_wpm = wpm;
        }
    }
}

/// Pause the run, from now: you pressed Pause, or the page went out of
/// sight.
pub fn pause_run(ctx: AppCtx, why: PauseWhy) {
    let at = MonoMs(mono_ms());
    send_run_event(ctx, ContestEvent::Pause { why, at });
}

/// Carry on after a pause. The audio is claimed afresh here, inside the tap
/// on Resume, so the player opens — or, after the sound failed, is built
/// anew — where a browser lets it start. A run that is not paused is left
/// alone: a claim would cut the turn on the air from under it.
pub fn resume_run(ctx: AppCtx) {
    let app = ctx.app();
    let paused = match app.session.borrow().as_ref().and_then(Run::of) {
        Some(live) if matches!(live.machine.phase(), ContestPhase::Paused { .. }) => {
            Some(live.own.sound.clone())
        }
        _ => None,
    };
    let Some(sound) = paused else {
        return;
    };
    let Ok(lease) = app
        .player
        .claim(&sound)
        .inspect_err(|err| ctx.toasts.show(err.to_string()))
    else {
        return;
    };
    if let Some(live) = app.session.borrow_mut().as_mut().and_then(Run::of_mut)
        && let Some(running) = live.stage.claimed()
    {
        live.lease = lease.clone();
        live.stage = running;
    }
    dispatch(ctx, &lease, ContestEvent::Resume);
    // The paused page is shorter, so a phone scrolled down to Pause is left
    // part way down it, the run bar and the fields above the screen's top:
    // the run carries on from its top, as every screen opens. Pausing
    // scrolls nothing, so nothing moves under the finger that paused.
    crate::page::scroll_to_top();
}

/// The run screen's clock ticked: the run is seen running now. A tick long
/// after the one before is time the app was not running, and the run is
/// paused from that one first.
pub fn tick(ctx: AppCtx) {
    let Some(lease) = live_run::lease::<Run>(ctx) else {
        return;
    };
    let away = live_run::step::<Run>(ctx, &lease, None, |live, now| {
        let at = live.away(now, true)?;
        Some(away_from(live, at, now))
    });
    if let Some((lease, effects)) = away {
        run_effects(ctx, &lease, effects);
    }
}

/// The run paused from `at`, when it was last seen before a gap the app was
/// not running in.
fn away_from(live: &mut LiveContest, at: MonoMs, now: MonoMs) -> Vec<ContestEffect> {
    let why = PauseWhy::Hidden;
    live.machine.apply(ContestEvent::Pause { why, at }, now)
}

/// Let the machine decide what `event` means, then do what it says: what
/// the user did, and news from the run's own tasks, a timer, a send. What
/// comes under the run's own lease is its to hear while it holds the audio,
/// and while it is paused, having given the audio back, as a paused session
/// hears it: what is done to it then, and the news of the send its pause
/// cut short ([`live_run::hears`]). News under another lease — a run that
/// has ended, one before it — is dropped. A timer never comes to a paused
/// run: the lease it was armed under drops it ([`Lease::after`]).
///
/// Before anything is heard, a gap since the run screen last ticked longer
/// than any tick takes is credited as paused, from that tick: a process
/// frozen whole — a locked phone — hears nothing until it thaws, and then
/// its first news is old.
fn dispatch(ctx: AppCtx, lease: &Lease, event: ContestEvent) {
    let heard = live_run::step::<Run>(ctx, lease, Some(event), |live, now| {
        let away = live.away(now, false);
        Some(away.map_or_else(Vec::new, |at| away_from(live, at, now)))
    });
    if let Some((lease, effects)) = heard {
        run_effects(ctx, &lease, effects);
    }
}

fn run_effects(ctx: AppCtx, lease: &Lease, effects: Vec<ContestEffect>) {
    for effect in effects {
        match effect {
            ContestEffect::PlayTurn(turn) => play_turn(ctx, lease, turn),
            ContestEffect::StopAudio => lease.stop_sending(),
            ContestEffect::ReleaseAudio => live_run::give_back::<Run>(ctx, lease),
            ContestEffect::StartTimer { id, ms } => {
                // One wait for the whole of it. A timer the machine has since
                // forgotten wakes once, is told so, and is gone; one a pause
                // let go of with the audio never wakes.
                lease.after(ms, move |lease| {
                    dispatch(ctx, lease, ContestEvent::TimerFired { id });
                });
            }
            ContestEffect::ClearFields => ctx.entry.clear(),
            ContestEffect::Focus(field) => focus_field(ctx, field),
            ContestEffect::Announce(text) => announce(ctx, text),
            ContestEffect::PersistAndShowResults => {
                live_run::keep::<Run>(ctx);
            }
            // A run with no station worked has nothing for any stat: it is
            // let go, and the results screen says so, with another run a
            // press away.
            ContestEffect::ShowNothingKept => {
                live_run::let_go::<Run>(ctx, Some(Finished::NothingKept));
            }
            ContestEffect::AbortToHome => {
                live_run::let_go::<Run>(ctx, None);
            }
        }
    }
    speak_if_silent(ctx);
}

/// Hold `text` for a screen reader until the air is silent.
fn announce(ctx: AppCtx, text: String) {
    let app = ctx.app();
    match app.session.borrow_mut().as_mut().and_then(Run::of_mut) {
        Some(live) => live.own.unsaid.push(text),
        None => say(ctx, vec![text]),
    }
}

/// Say what the run holds for a screen reader, once nothing is on the air.
fn speak_if_silent(ctx: AppCtx) {
    let silent = ctx
        .contest
        .peek()
        .as_ref()
        .is_some_and(|view| view.air.is_none());
    if !silent {
        return;
    }
    let unsaid = match ctx
        .app()
        .session
        .borrow_mut()
        .as_mut()
        .and_then(Run::of_mut)
    {
        Some(live) => std::mem::take(&mut live.own.unsaid),
        None => return,
    };
    say(ctx, unsaid);
}

/// Say `unsaid`, all of it at once, in the order it came.
fn say(ctx: AppCtx, unsaid: Vec<String>) {
    if unsaid.is_empty() {
        return;
    }
    let mut announced = ctx.announced;
    announced.set(Some(Said::new(unsaid.join(". "))));
}

/// Play `turn`, and tell the machine when each attempt at it started, and
/// how it went.
///
/// It plays from the run's own settings, fading off ([`RunOwn::sound`]),
/// not from whatever the settings screen holds now.
fn play_turn(ctx: AppCtx, lease: &Lease, turn: Turn) {
    let sound = ctx
        .app()
        .session
        .borrow()
        .as_ref()
        .and_then(Run::of)
        .filter(|live| live.lease.is(lease))
        .map(|live| live.own.sound.clone());
    let Some(settings) = sound else {
        return;
    };
    let id = turn.id;
    let lease = lease.clone();
    let Some(transcript) = turn.transcript() else {
        // A turn with nothing in it is over as soon as it begins; the
        // machine never plays one.
        dispatch(ctx, &lease, ContestEvent::TurnEnded { id });
        return;
    };
    let plan = plan_turn(&turn, &settings);
    // A continuation starts where the turn before gives way, which plays on
    // till then: no character of your message goes twice, and none waits.
    let starts_at = turn.starts_at;
    // Asked for now, as the turn goes on the air, not when its task first
    // runs: a stop handled before then ends it, and a turn before it still
    // waiting out a retry gives up rather than start over it.
    let asked = lease.ask_to_send();
    spawn_forever(async move {
        let heard = lease.clone();
        // Each attempt, retries too: when its sound goes to the output, and
        // when it reaches the ear, the player's lead, any wait for its
        // moment and latency after it starts; what of it went unheard, as
        // soon as that is found, before anything heard after it counts; and
        // whether it was held up on its way, however it ended.
        let news = move |news: SendNews<'_>| {
            let event = match news {
                SendNews::Started(start) => ContestEvent::TurnStarted {
                    id,
                    heard_from: start.heard_at(),
                    output_from: start.output_at,
                },
                SendNews::Missed {
                    from_sec,
                    until_sec,
                } => ContestEvent::TurnMissed {
                    id,
                    from_sec,
                    until_sec,
                },
                SendNews::HeldUp => ContestEvent::TurnHeldUp { id },
            };
            dispatch(ctx, &heard, event);
        };
        let event = match lease
            .send_planned(asked, &plan, &transcript, &settings, starts_at, news)
            .await
        {
            Ok(_) => ContestEvent::TurnEnded { id },
            // A stop, a newer turn, a pause: the machine has moved on.
            Err(AudioError::Cancelled) => ContestEvent::TurnCancelled { id },
            Err(failure) => {
                ctx.toasts.show(failure.to_string());
                ContestEvent::TurnFailed { id }
            }
        };
        dispatch(ctx, &lease, event);
    });
}

/// A field's id, and what it is called.
pub fn field_name(field: EntryField) -> (String, &'static str) {
    let label = match field {
        EntryField::Call => "Call",
        EntryField::Exchange => "Exchange",
    };
    (control_id("field", label), label)
}

/// Move the cursor to `field`, on the page and for the paddles: to the end
/// of what it holds, with nothing selected, so the next key goes after it
/// and ⌫ takes its last character.
pub fn focus_field(ctx: AppCtx, field: EntryField) {
    ctx.entry.set_focus(field);
    let (id, _) = field_name(field);
    focus_by_id_anywhere(&id, Caret::AtEnd);
}

/// While a run has the training screen, the cursor back in its field, as it
/// was: after a press of a button that is not the run's own, which a
/// keyboard may have moved the cursor to.
pub fn refocus_run(ctx: AppCtx) {
    if *ctx.screen.peek() == Screen::Training && ctx.contest.peek().is_some() {
        focus_field(ctx, ctx.entry.focus());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::{Behaviour, Call};
    use crate::persist::Store;
    use crate::persist::backup::{read_backup, write_backup};
    use crate::testing::{
        Harness, contest_settings, copy_what_was_heard, press, quiet, run, type_into,
    };
    use crate::time::POLL_MS;
    use cw_core::band::BandKey;
    use cw_core::{CharSetMode, ContestView, MONITOR_LEVEL, plan_morse_playback_for};

    fn view(h: &Harness) -> Option<ContestView> {
        h.ctx.contest.peek().clone()
    }

    fn phase(h: &Harness) -> Option<ContestPhase> {
        view(h).map(|view| view.phase)
    }

    /// Work the run as a perfect operator until it is over or `budget_ms`
    /// has passed: each time the air falls quiet, copy and press Enter. With
    /// `qsos`, stop once that many are logged instead.
    async fn work(h: &mut Harness, budget_ms: u64, qsos: Option<u32>) {
        let mut waited = 0;
        while waited < budget_ms && h.screen() == Screen::Training {
            if qsos.is_some_and(|qsos| view(h).is_some_and(|view| view.qsos >= qsos)) {
                return;
            }
            if quiet(h) {
                copy_what_was_heard(h);
                press(h, OpKey::Esm);
            }
            h.advance(u64::from(POLL_MS)).await;
            waited += u64::from(POLL_MS);
        }
    }

    /// A one-minute run worked to its end is stored, log and all, and shown
    /// on the results screen; it reads back as a run, from the store's
    /// parsing and from a backup alike.
    #[test]
    fn a_full_run_stores_its_log_and_shows_the_results() {
        run(|| async {
            let mut settings = contest_settings();
            settings.contest.contest_minutes = 1;
            let mut h = Harness::with_settings(settings.clone());
            h.start_training();
            work(&mut h, 90_000, None).await;
            assert_eq!(h.screen(), Screen::Results);
            assert!(h.ctx.contest.peek().is_none());
            assert!(h.app.session.borrow().is_none());
            assert!(
                h.calls().contains(&Call::Shutdown),
                "the audio was given back"
            );

            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(result.char_set_mode, CharSetMode::Contest);
            assert_eq!(result.alphabet_fingerprint, "contest:wpx");
            let log = result.contest.as_ref().expect("its log");
            let stored = log.run(result.groups.len()).expect("the log reads back");
            let logged = stored.qsos.iter().filter(|qso| qso.k.is_empty()).count();
            assert!(logged >= 2, "{logged} QSOs logged");
            // The QSO open as time ran out, if one was, is a lost copy.
            assert!(stored.qsos.len() <= logged + 1, "{:?}", stored.qsos);
            assert_eq!(stored.run_ms, 60_000);
            // Every copy is right, but the number of that QSO, which was
            // never logged.
            let mut at = 0;
            for qso in &stored.qsos {
                let owned = &result.groups[at..at + qso.g as usize];
                at += qso.g as usize;
                let copied = if qso.k.is_empty() { owned } else { &owned[..1] };
                assert!(copied.iter().all(|group| group.correct), "{qso:?}");
            }
            assert_eq!(h.ctx.sessions.peek().len(), 1);
            assert_eq!(h.store.load_sessions(), vec![result.clone()]);
            assert_eq!(h.store.keep_asks.get(), 1);
            assert_eq!(*h.ctx.auto_message.peek(), None, "a run moves no level");

            let file = write_backup(&settings, &h.store.load_sessions(), 0);
            let back = read_backup(&file).expect("a backup").sessions;
            assert_eq!(back.len(), 1);
            assert_eq!(back[0].char_set_mode, CharSetMode::Contest);
            assert_eq!(back[0].contest, result.contest);
            assert_eq!(back[0].groups, result.groups);
        });
    }

    /// A run and Hear a QSO draw their callers from the prior the app keeps
    /// ([`AppCtx::sampler`]): built ahead of the tap that starts either,
    /// the tap replays no history, and built in it only once the history
    /// has changed since.
    #[test]
    fn a_run_and_hear_a_qso_start_from_the_prior_built_ahead() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            let ctx = h.ctx;
            let built = || crate::testing::take_renders("SamplerPrior");
            built();
            h.in_app(|| ctx.warm_sampler());
            h.advance(u64::from(POLL_MS) * 2).await;
            assert_eq!(built(), 1, "built ahead");
            h.in_app(|| crate::playback::start_qsos(ctx));
            h.pump();
            assert!(h.in_app(|| ctx.preview.peek().is_qsos()));
            assert_eq!(built(), 0, "Hear a QSO started from it");
            h.in_app(|| ctx.stop_audio());
            h.pump();
            h.start_training();
            assert!(view(&h).is_some(), "the run started");
            assert_eq!(built(), 0, "so did the run");
            h.in_app(|| crate::session_runtime::abort_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            let mut sessions = ctx.sessions;
            h.in_app(|| {
                sessions
                    .write()
                    .push(crate::testing::stored_run("2026-09-01", &[]))
            });
            h.start_training();
            assert!(view(&h).is_some(), "another run started");
            assert_eq!(built(), 1, "on a history changed since");
        });
    }

    /// Ending a run with one QSO in the log ends it there, keeping it. The
    /// run screen's End run and the header's Exit both end it so.
    #[test]
    fn ending_with_one_qso_keeps_it_and_shows_the_results() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            assert_eq!(view(&h).map(|view| view.qsos), Some(1));
            let ctx = h.ctx;
            h.in_app(|| crate::session_runtime::finish_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Results);
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            let stored = result
                .contest
                .as_ref()
                .and_then(|log| log.run(result.groups.len()))
                .expect("its log");
            assert_eq!(stored.qsos.len(), 1);
            assert_eq!(result.groups.len(), 2, "the call and its number");
        });
    }

    /// A run is dated the local day it started, so one that runs past
    /// midnight keeps that day's place in the streak.
    #[test]
    fn a_run_is_dated_the_day_it_started() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            if let Some(Live::Contest(live)) = h.app.session.borrow_mut().as_mut() {
                live.started_on = "2026-10-27".into();
            }
            work(&mut h, 60_000, Some(1)).await;
            let ctx = h.ctx;
            h.in_app(|| crate::session_runtime::finish_run(ctx));
            h.pump();
            let stored = h.store.load_sessions();
            assert_eq!(
                stored
                    .iter()
                    .map(|run| run.date.as_str())
                    .collect::<Vec<_>>(),
                ["2026-10-27"]
            );
        });
    }

    /// The player is handed your message and every reply whole, with no
    /// fading laid on, and the band it plays over is the run's.
    #[test]
    fn the_recorder_hears_no_fading_your_message_last_and_no_reply_cut() {
        run(|| async {
            let settings = contest_settings();
            let mut h = Harness::with_settings(settings.clone());
            h.start_training();
            assert!(h.run_until(10_000, quiet).await);
            let starts = h.recorder.starts();
            let cq = &starts[0];
            assert!(
                cq.transcript.text.starts_with("CQ DL1ABC"),
                "{:?}",
                cq.transcript
            );
            assert_eq!(cq.qsb, None, "the fading was left on");
            assert_eq!(cq.transcript.others.len(), 1, "one caller answers");
            let ends = cq.plan.others.iter().map(|other| other.duration_sec);
            assert!(
                ends.into_iter()
                    .all(|end| end <= cq.plan.wanted.duration_sec),
                "a reply outlasts your message"
            );
            for (reply, played) in cq.transcript.others.iter().zip(&cq.plan.others) {
                let whole = plan_morse_playback_for(&reply.text, &settings, &reply.voice);
                assert_eq!(
                    played.events.len(),
                    whole.events.len(),
                    "{} was cut",
                    reply.text
                );
            }
            let band = BandKey::of(&settings);
            assert!(h.calls().iter().all(|call| match call {
                Call::Band(key) => *key == band,
                _ => true,
            }));
        });
    }

    /// Your message in a run goes around the receiver: every turn the player
    /// is handed is planned dry, your tones at your sending's level, the
    /// monitor's turned down. A session of groups never is: the station you
    /// copy is one the receiver hears.
    #[test]
    fn a_runs_turns_are_planned_dry_and_a_sessions_groups_never() {
        run(|| async {
            let settings = contest_settings();
            let level = settings.contest.sending_level();
            assert!(level > 0.0 && level < MONITOR_LEVEL);
            let mut h = Harness::with_settings(settings);
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            let starts = h.recorder.starts();
            assert!(starts.len() >= 3, "{} turns", starts.len());
            assert!(starts.iter().all(|start| start.plan.dry_wanted));
            let cq = &starts[0].plan.wanted.events;
            assert!(!cq.is_empty(), "your CQ was not played");
            assert!(cq.iter().all(|tone| tone.target_gain == level));

            let mut h = Harness::new();
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let starts = h.recorder.starts();
            assert!(!starts.is_empty(), "no group was sent");
            assert!(starts.iter().all(|start| !start.plan.dry_wanted));
        });
    }

    /// With your sending off, your messages are silent and nothing else
    /// changes: each turn the player is handed has no tones of yours and
    /// lasts as long, the air line shows your message going out, the heard
    /// timer brings the caller's call to Enter as ever, and a QSO is worked
    /// and logged clean.
    #[test]
    fn your_sending_off_is_silent_and_the_run_keeps_its_time() {
        run(|| async {
            let mut settings = contest_settings();
            settings.contest.my_sending_db = cw_core::MY_SENDING_OFF_DB;
            let mut loud = settings.clone();
            loud.contest.my_sending_db = 0;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.advance(300).await;
            let air = view(&h).and_then(|view| view.air).expect("your CQ");
            assert!(air.still_sending(MonoMs(mono_ms())).starts_with("CQ"));
            assert!(h.run_until(10_000, quiet).await);
            work(&mut h, 60_000, Some(1)).await;
            let starts = h.recorder.starts();
            assert!(starts.len() >= 3, "{} turns", starts.len());
            for start in &starts {
                assert!(start.plan.dry_wanted && start.plan.wanted.events.is_empty());
                // As long as it would be heard at the loudest.
                let turn_sec =
                    plan_morse_playback_for(&start.transcript.text, &loud, &start.transcript.voice)
                        .duration_sec;
                assert!(start.plan.wanted.duration_sec + 1e-9 >= turn_sec);
            }
            let clean = |h: &Harness| {
                let Some(row) = view(h).and_then(|view| view.log.first().cloned()) else {
                    return false;
                };
                matches!(&row.status, cw_core::RowStatus::Checked(check) if check.is_clean())
            };
            assert!(h.run_until(15_000, clean).await, "logged and checked clean");
        });
    }

    /// A run is marked open from its start until it ends, kept or not: one
    /// the app stops under is told of when it next opens.
    #[test]
    fn a_run_is_marked_open_while_it_runs() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let open = h.store.open().expect("marked open");
            assert!(open.run);
            let ctx = h.ctx;
            h.in_app(|| crate::session_runtime::abort_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            assert_eq!(h.store.open(), None, "discarded");

            h.start_training();
            assert!(h.store.open().is_some());
            h.in_app(|| crate::session_runtime::finish_run(ctx));
            h.pump();
            assert_ne!(h.screen(), Screen::Training);
            assert_eq!(h.store.open(), None, "ended");
        });
    }

    /// A run the app stops under, with no End run — a crash, the phone
    /// taking the app back — keeps what End run would
    /// have kept as of its last QSO logged: each QSO goes with its mark as
    /// it is logged, never into the history mid-run, and the next launch
    /// keeps the run and says so.
    #[test]
    fn a_run_the_app_stops_under_keeps_its_qsos() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.store.mark_tutorial_done();
            h.start_training();
            work(&mut h, 120_000, Some(2)).await;
            assert_eq!(view(&h).map(|view| view.qsos), Some(2));
            let logged = |h: &Harness| {
                let so_far = h.store.open().and_then(|open| open.so_far)?;
                let log = so_far.result.contest?;
                log.0["qsos"].as_array().map(Vec::len)
            };
            assert!(
                h.run_until(15_000, |h| logged(h) == Some(2)).await,
                "{:?}",
                logged(&h)
            );
            assert!(h.store.load_sessions().is_empty(), "stored mid-run");

            let (mut ui, _recorder) = crate::testing::Ui::launch(h.store.clone());
            ui.advance(20).await;
            let kept = h.store.load_sessions();
            assert_eq!(kept.len(), 1, "the run was kept");
            let log = kept[0].contest.as_ref().expect("its log");
            assert_eq!(log.0["qsos"].as_array().map(Vec::len), Some(2));
            assert!(
                ui.has("Dust closed while it ran. The QSOs you had logged were kept."),
                "{}",
                ui.html()
            );
            assert_eq!(h.store.open(), None);
        });
    }

    /// − and + move your speed, and it is kept: written to the settings as
    /// it moves, though nothing is stored mid-run, and the next run sends
    /// at it, storing it as it starts. Callers keep the Speed card's speeds.
    #[test]
    fn your_speed_is_kept_for_the_next_run() {
        run(|| async {
            let settings = contest_settings();
            assert_eq!(settings.contest.my_wpm, 0, "the callers' middle, 30");
            let mut h = Harness::with_settings(settings);
            h.start_training();
            let ctx = h.ctx;
            h.in_app(|| change_speed(ctx, 2));
            h.in_app(|| change_speed(ctx, 2));
            assert_eq!(view(&h).map(|view| view.my_wpm), Some(34.0));
            assert_eq!(h.ctx.settings.peek().contest.my_wpm, 34);
            assert_eq!(
                h.store.load_settings().contest.my_wpm,
                0,
                "not stored mid-run"
            );
            h.in_app(|| crate::session_runtime::abort_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            h.start_training();
            assert_eq!(h.store.load_settings().contest.my_wpm, 34);
            assert_eq!(view(&h).map(|view| view.my_wpm), Some(34.0));
            let cq = h
                .recorder
                .starts()
                .last()
                .expect("the next run's CQ")
                .clone();
            assert_eq!(cq.transcript.voice.char_wpm, 34.0);
            assert!(
                cq.transcript
                    .others
                    .iter()
                    .all(|caller| caller.voice.char_wpm == 30.0),
                "the callers keep the Speed card's"
            );
        });
    }

    /// Each attempt at a turn tells the run when its sound reaches the ear:
    /// the start, and the player's latency after it.
    #[test]
    fn a_turn_starting_tells_the_run_when_it_is_heard() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.recorder.latency_sec.set(0.3);
            h.start_training();
            let air = view(&h)
                .and_then(|view| view.air)
                .expect("the CQ on the air");
            assert_eq!(air.heard_from, MonoMs(300));
        });
    }

    /// A key while your message is still going out is spliced on where the
    /// message reaches the start of its next character: the player is asked
    /// to start the continuation at that moment, the rest of your message
    /// going on from that character and the one sounding at the key never
    /// sent again, and the run hears it from then.
    #[test]
    fn a_key_mid_message_is_spliced_on_where_the_message_gives_way() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            h.advance(300).await;
            let pressed = mono_ms();
            press(&mut h, OpKey::Again);
            let starts = h.recorder.starts();
            let (cq, next) = (&starts[0], starts.last().expect("the continuation"));
            assert!(
                cq.transcript.text.starts_with("CQ "),
                "{}",
                cq.transcript.text
            );
            // The C sounding at the key goes out whole in the CQ's send; the
            // rest goes on from the Q, the C's four elements before it.
            assert!(
                next.transcript.text.starts_with("Q DL1ABC"),
                "{}",
                next.transcript.text
            );
            let q_at = cq.plan.wanted.events[4].start_sec;
            let heard_in = next.heard_in.expect("a moment to start at");
            let meant = q_at - (pressed as f64 / 1_000.0);
            assert!((heard_in - meant).abs() < 0.002, "{heard_in} for {meant}");
            let air = view(&h).and_then(|view| view.air).expect("on the air");
            assert_eq!(air.heard_from, MonoMs(pressed).plus_sec(heard_in));
        });
    }

    /// Without full break-in, as it ships, every turn with a message of
    /// yours mutes the receiver from its start to your message's end, and a
    /// turn of callers alone none; with full break-in, none does.
    #[test]
    fn the_receiver_is_muted_while_you_send_unless_full_break_in() {
        run(|| async {
            for qsk in [false, true] {
                let mut settings = contest_settings();
                settings.contest.contest_qsk = qsk;
                let mut h = Harness::with_settings(settings.clone());
                h.start_training();
                work(&mut h, 30_000, Some(1)).await;
                let starts = h.recorder.starts();
                assert!(starts.len() >= 3, "{} turns", starts.len());
                for start in &starts {
                    let yours = &start.transcript;
                    let muted = (!qsk && !yours.text.is_empty()).then(|| {
                        plan_morse_playback_for(&yours.text, &settings, &yours.voice).duration_sec
                    });
                    assert_eq!(start.plan.muted_until, muted, "{qsk}: {}", yours.text);
                }
            }
        });
    }

    /// A turn its stream skipped some of, the page stalled, never reached
    /// you there: the caller it skipped is not heard, so Show copy shows
    /// nothing of it, and the run goes on with nothing said.
    #[test]
    fn a_caller_skipped_in_part_is_never_shown() {
        run(|| async {
            for skipped in [false, true] {
                let mut settings = contest_settings();
                settings.contest.contest_show_copy = true;
                let mut h = Harness::with_settings(settings);
                if skipped {
                    h.recorder.skip_next.set(Some((1.0, 60.0)));
                }
                h.start_training();
                assert!(h.run_until(20_000, quiet).await, "{skipped}");
                let cq = &h.recorder.starts()[0].transcript;
                assert!(
                    !cq.others.is_empty(),
                    "{skipped}: a caller in the CQ's turn"
                );
                assert!(
                    cq.others[0].delay_sec > 1.0,
                    "{skipped}: after the skip begins"
                );
                let shown = view(&h).map(|view| view.shown).unwrap_or_default();
                assert_eq!(shown.len(), usize::from(!skipped), "{skipped}: {shown:?}");
                assert_eq!(phase(&h), Some(ContestPhase::Running), "{skipped}");
                assert_eq!(h.toast(), None, "{skipped}");
            }
        });
    }

    /// A send stopped or replaced is no failure: the run carries on. One that
    /// fails every attempt pauses it, and says so.
    #[test]
    fn a_cancel_never_pauses_and_a_failure_does() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            h.advance(500).await;
            // A key mid-turn replaces it; Stop cuts the one after.
            press(&mut h, OpKey::Cq);
            h.advance(300).await;
            press(&mut h, OpKey::StopTx);
            h.advance(2_000).await;
            assert_eq!(phase(&h), Some(ContestPhase::Running));
            assert!(h.toast().is_none());

            h.set_behaviour(Behaviour::FailMidSend);
            press(&mut h, OpKey::Cq);
            assert!(
                h.run_until(30_000, |h| phase(h)
                    == Some(ContestPhase::Paused {
                        why: PauseWhy::Sound
                    }))
                .await
            );
            assert!(h.toast().is_some(), "the failure is said");
            assert!(!h.lease().is_current(), "the audio was given back");
            assert_eq!(h.screen(), Screen::Training);
        });
    }

    /// A turn replaced while it waits out a retry gives up: it never starts
    /// over the turn that replaced it, which plays to its end, and the run
    /// falls quiet again.
    #[test]
    fn a_turn_waiting_out_a_retry_never_starts_over_a_newer_one() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            assert!(h.run_until(10_000, quiet).await);
            let before = h.texts().len();
            // The CQ's first attempt breaks, and Again is keyed before its
            // retry starts.
            h.recorder.fail_next(1);
            press(&mut h, OpKey::Cq);
            press(&mut h, OpKey::Again);
            assert!(
                h.run_until(30_000, quiet).await,
                "on the air: {:?}",
                view(&h).and_then(|view| view.air)
            );
            let sent = &h.texts()[before..];
            assert_eq!(sent.len(), 2, "{sent:?}");
            assert!(sent[1].ends_with('?'), "{sent:?}");
            assert!(h.toast().is_none(), "a replaced turn's failure was shown");
        });
    }

    /// Esc handled before a turn's send has started stops it all the same:
    /// what the run took back is never heard.
    #[test]
    fn esc_before_a_turn_starts_sounding_stops_it() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            assert!(h.run_until(10_000, quiet).await);
            let before = h.texts().len();
            let ctx = h.ctx;
            h.in_app(|| {
                press_key(ctx, OpKey::Cq);
                press_key(ctx, OpKey::StopTx);
            });
            h.pump();
            // Nothing of yours is on the air: at most a caller answering
            // what it heard of the CQ.
            let air = view(&h).and_then(|view| view.air);
            let yours = air.as_ref().map(|air| air.my_text.as_str());
            assert!(yours.is_none_or(str::is_empty), "{air:?}");
            h.advance(3_000).await;
            // Callers may call again; your CQ never goes out.
            let sent = &h.texts()[before..];
            assert!(sent.iter().all(String::is_empty), "{sent:?}");
        });
    }

    /// Resume after the sound failed opens a fresh player inside the tap,
    /// before the callers come back a second later.
    #[test]
    fn resume_after_a_sound_failure_opens_a_player_before_the_callers_return() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.set_behaviour(Behaviour::FailMidSend);
            h.start_training();
            assert!(
                h.run_until(30_000, |h| matches!(
                    phase(h),
                    Some(ContestPhase::Paused { .. })
                ))
                .await
            );
            h.set_behaviour(Behaviour::Play);
            let built = h.recorder.players_built.get();
            let sends = h.texts().len();
            let ctx = h.ctx;
            h.in_app(|| resume_run(ctx));
            h.pump();
            assert_eq!(h.recorder.players_built.get(), built + 1);
            assert!(h.lease().is_current());
            assert_eq!(phase(&h), Some(ContestPhase::Running));
            assert_eq!(
                h.texts().len(),
                sends,
                "nothing is sent before the callers return"
            );
            assert!(h.run_until(1_500, |h| h.texts().len() > sends).await);
            // Resume on a run that is not paused claims nothing.
            let lease = h.lease();
            h.in_app(|| resume_run(ctx));
            assert!(lease.is_current());
        });
    }

    /// A page hidden, and a gap in the run screen's ticks, are both left
    /// out of run time from when they began.
    #[test]
    fn a_hidden_page_and_a_gap_in_the_ticks_are_credited_as_paused() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let ctx = h.ctx;
            h.advance(2_000).await;
            h.in_app(|| pause_run(ctx, PauseWhy::Hidden));
            let paused_at = mono_ms();
            h.advance(5_000).await;
            let shown = view(&h).expect("a run");
            assert_eq!(
                shown.phase,
                ContestPhase::Paused {
                    why: PauseWhy::Hidden
                }
            );
            let clock = shown.clock.expect("running");
            assert_eq!(clock.paused_ms(MonoMs(mono_ms())), mono_ms() - paused_at);
            h.advance(5_000).await;
            assert!(
                matches!(phase(&h), Some(ContestPhase::Paused { .. })),
                "it resumed by itself"
            );

            let mut h = Harness::with_settings(contest_settings());
            let started = mono_ms();
            h.start_training();
            let ctx = h.ctx;
            h.advance(1_000).await;
            h.in_app(|| tick(ctx));
            let ticked = mono_ms();
            // Frozen: nothing ticks for five seconds.
            h.advance(5_000).await;
            h.in_app(|| tick(ctx));
            let shown = view(&h).expect("a run");
            assert_eq!(
                shown.phase,
                ContestPhase::Paused {
                    why: PauseWhy::Hidden
                }
            );
            let now = MonoMs(mono_ms());
            let clock = shown.clock.expect("running");
            assert_eq!(
                clock.peek_ms(now),
                ticked - started,
                "the gap counted as run time"
            );
            assert_eq!(clock.paused_ms(now), now.0 - ticked);
        });
    }

    /// Without a call Dust can read, the run does not start, and says what
    /// it needs.
    #[test]
    fn a_bad_own_call_refuses_to_start() {
        run(|| async {
            for (call, said) in [
                ("", "Add your callsign to start a contest"),
                ("ABC", "ABC is not a callsign Dust can read"),
            ] {
                let mut settings = contest_settings();
                settings.contest.my_call = call.to_string();
                let mut h = Harness::with_settings(settings);
                h.start_training();
                assert_eq!(h.screen(), Screen::Home);
                assert_eq!(h.toast().as_deref(), Some(said));
                assert!(h.app.session.borrow().is_none());
                assert!(h.ctx.contest.peek().is_none());
                assert!(h.calls().is_empty(), "the audio was claimed");
            }
        });
    }

    /// Discard leaves without keeping anything, and news from the run's old
    /// sends changes nothing after it.
    #[test]
    fn discard_keeps_nothing() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            let ctx = h.ctx;
            h.in_app(|| crate::session_runtime::abort_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.ctx.contest.peek().is_none());
            h.advance(5_000).await;
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.ctx.sessions.peek().is_empty());
        });
    }

    /// What changes nothing is not published: no copy of the view, and no
    /// redraw of the screen showing it.
    #[test]
    fn news_that_changes_nothing_is_not_published() {
        run(|| async {
            use std::sync::atomic::Ordering;

            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            assert!(h.run_until(10_000, quiet).await);
            let writes = h.count_writes(h.ctx.contest);
            let (ctx, lease) = (h.ctx, h.lease());
            h.in_app(|| {
                dispatch(ctx, &lease, ContestEvent::TimerFired { id: 9_999 });
                dispatch(ctx, &lease, ContestEvent::TurnCancelled { id: 1 });
            });
            h.pump();
            assert_eq!(
                writes.load(Ordering::Relaxed),
                0,
                "stale news was published"
            );
            press(&mut h, OpKey::Cq);
            assert!(writes.load(Ordering::Relaxed) > 0, "a CQ went unpublished");
        });
    }

    /// A run counts down before its first CQ, and a pause in the countdown
    /// gives the audio back and starts it again on Resume.
    #[test]
    fn a_run_counts_down_before_its_first_cq() {
        run(|| async {
            let mut settings = contest_settings();
            settings.playback.countdown_sec = 2;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert_eq!(phase(&h), Some(ContestPhase::Countdown { left: 2 }));
            assert!(h.texts().is_empty(), "sent during the countdown");
            let ctx = h.ctx;
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            assert!(!h.lease().is_current());
            h.advance(5_000).await;
            assert!(h.texts().is_empty());
            h.in_app(|| resume_run(ctx));
            assert_eq!(phase(&h), Some(ContestPhase::Countdown { left: 2 }));
            h.advance(2_100).await;
            assert_eq!(phase(&h), Some(ContestPhase::Running));
            assert!(h.texts()[0].starts_with("CQ DL1ABC"), "{:?}", h.texts());
        });
    }

    /// The fields keep what is typed, with when: the call's last edit, and
    /// the first moment the exchange read complete since it was empty. A
    /// paddle letter goes into the field with the cursor, and is drawn.
    #[test]
    fn the_fields_keep_what_is_typed_and_when() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let entry = h.ctx.entry;
            let repaints = |h: &Harness| *h.ctx.entry.repaint.peek();
            let drawn = repaints(&h);
            h.advance(100).await;
            type_into(&mut h, EntryField::Call, "K5XYZ");
            let typed_at = MonoMs(mono_ms());
            h.advance(100).await;
            type_into(&mut h, EntryField::Exchange, "21");
            let complete_at = MonoMs(mono_ms());
            h.advance(100).await;
            type_into(&mut h, EntryField::Exchange, "214");
            let fields = entry.snapshot();
            assert_eq!(
                (fields.call.as_str(), fields.exch.as_str()),
                ("K5XYZ", "214")
            );
            assert_eq!(fields.call_edited, typed_at);
            assert_eq!(
                fields.exch_complete,
                Some(complete_at),
                "stamped when first complete"
            );
            assert_eq!(repaints(&h), drawn, "typing drew the fields");

            h.in_app(|| entry.set_focus(EntryField::Exchange));
            h.in_app(|| entry.push('5', Some(MonoMs(mono_ms()))));
            assert_eq!(entry.snapshot().exch, "2145");
            assert_eq!(repaints(&h), drawn + 1, "a paddle letter is drawn");

            type_into(&mut h, EntryField::Exchange, "");
            assert_eq!(entry.snapshot().exch_complete, None);
            h.in_app(|| entry.clear());
            assert_eq!(entry.snapshot().call, "");
        });
    }

    /// A run that worked nobody keeps nothing, and its results screen says
    /// so in place of a result.
    #[test]
    fn a_run_with_nobody_worked_keeps_nothing() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let ctx = h.ctx;
            h.in_app(|| crate::session_runtime::finish_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(*h.ctx.result.peek(), Some(Finished::NothingKept));
            assert!(h.app.session.borrow().is_none());
            assert!(h.ctx.sessions.peek().is_empty());
            // The next run that keeps something shows its result instead.
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            h.in_app(|| crate::session_runtime::finish_run(ctx));
            h.pump();
            assert!(
                h.ctx
                    .result
                    .peek()
                    .clone()
                    .and_then(Finished::kept)
                    .is_some()
            );
        });
    }

    /// What the run says as it ends, time's up last, is still said once
    /// the results are up: the region that says it is the app's, not the
    /// run screen's.
    #[test]
    fn times_up_is_said_on_the_results() {
        run(|| async {
            let mut settings = contest_settings();
            settings.contest.contest_minutes = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            work(&mut h, 90_000, None).await;
            assert_eq!(h.screen(), Screen::Results);
            let said = h.ctx.announced.peek().clone().map(|said| said.text);
            assert!(
                said.as_deref()
                    .is_some_and(|said| said.ends_with("Time's up")),
                "{said:?}"
            );
        });
    }

    /// What the run says for a screen reader waits for the air to fall
    /// silent, and comes whole: a check shown as the TU goes out is said
    /// once it has; Pause, which silences the air, is said at once.
    #[test]
    fn what_the_run_says_waits_for_the_air_to_fall_silent() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let said = |h: &Harness| {
                h.ctx
                    .announced
                    .peek()
                    .as_ref()
                    .map(|said| said.text.clone())
            };
            assert!(h.run_until(30_000, quiet).await);
            copy_what_was_heard(&mut h);
            press(&mut h, OpKey::Esm);
            assert!(h.run_until(30_000, quiet).await);
            copy_what_was_heard(&mut h);
            press(&mut h, OpKey::Esm);
            // Logged with the TU on the air: nothing is said over it.
            assert!(view(&h).is_some_and(|view| view.air.is_some()));
            assert_eq!(said(&h), None);
            assert!(h.run_until(30_000, quiet).await);
            let logged = said(&h).expect("said once the TU was out");
            assert!(logged.ends_with("logged, clean"), "{logged}");
            let ctx = h.ctx;
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            assert_eq!(said(&h).as_deref(), Some("Paused"));
            let first = h.ctx.announced.peek().clone().map(|said| said.serial);
            h.in_app(|| resume_run(ctx));
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            let again = h.ctx.announced.peek().clone().map(|said| said.serial);
            assert_ne!(first, again, "the same words, said again");
        });
    }

    /// The run the store holds last, and its log.
    fn stored_last(h: &Harness) -> (cw_core::SessionResult, cw_core::ContestRun) {
        let kept = h.store.load_sessions().pop().expect("a run stored");
        let log = kept
            .contest
            .as_ref()
            .and_then(|log| log.run(kept.groups.len()))
            .expect("its log");
        (kept, log)
    }

    /// End the run as End run does.
    fn end(h: &mut Harness) {
        let ctx = h.ctx;
        h.in_app(|| crate::session_runtime::finish_run(ctx));
        h.pump();
    }

    /// Work the run on from one QSO logged until the next station has
    /// taken the call sent to it, its exchange come: a second QSO open.
    async fn open_a_second_qso(h: &mut Harness) {
        let first = view(h)
            .and_then(|view| view.log.first().map(|row| row.call.clone()))
            .expect("a QSO logged");
        let open = |h: &Harness| {
            view(h).is_some_and(|view| {
                view.qsos == 1 && view.qso.call_taken && view.qso.call_sent != first
            })
        };
        let mut waited = 0;
        while !open(h) {
            assert!(waited < 30_000, "no second station took its call");
            if quiet(h) {
                copy_what_was_heard(h);
                press(h, OpKey::Esm);
            }
            h.advance(u64::from(POLL_MS)).await;
            waited += u64::from(POLL_MS);
        }
    }

    /// A paused run's ticks change nothing: a gap between two of them is
    /// found, but the run is paused already, as you paused it, and nothing
    /// is said of it.
    #[test]
    fn a_paused_runs_ticks_change_nothing() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let ctx = h.ctx;
            h.advance(500).await;
            h.in_app(|| tick(ctx));
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            h.pump();
            let said = h.ctx.announced.peek().clone();
            assert_eq!(said.as_ref().map(|said| said.text.as_str()), Some("Paused"));
            h.advance(5_000).await;
            h.in_app(|| tick(ctx));
            h.pump();
            assert_eq!(phase(&h), Some(ContestPhase::Paused { why: PauseWhy::You }));
            assert_eq!(*h.ctx.announced.peek(), said, "something was said");
        });
    }

    /// A run held back before its screen first ticks, a phone frozen as it
    /// started, runs on, the gap counted as run time: its gap check waits
    /// for the first tick, where a session's is armed as it starts.
    #[test]
    fn a_run_held_back_before_its_first_tick_runs_on() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            let started = mono_ms();
            h.start_training();
            tokio::time::advance(std::time::Duration::from_secs(5)).await;
            tokio::task::yield_now().await;
            h.pump();
            let ctx = h.ctx;
            h.in_app(|| tick(ctx));
            let shown = view(&h).expect("a run");
            assert_eq!(shown.phase, ContestPhase::Running);
            let now = MonoMs(mono_ms());
            let clock = shown.clock.expect("running");
            assert_eq!(clock.peek_ms(now), now.0 - started, "the gap is run time");
            assert_eq!(clock.paused_ms(now), 0);
        });
    }

    /// Keys pressed while every task is held back do not keep a run seen,
    /// as a session's news keeps it: only its screen's ticks do, so a gap
    /// between two ticks pauses it from the first, keys or not.
    #[test]
    fn keys_do_not_keep_a_run_seen_between_ticks() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let ctx = h.ctx;
            h.advance(500).await;
            h.in_app(|| tick(ctx));
            let ticked = mono_ms();
            for _ in 0..2 {
                tokio::time::advance(std::time::Duration::from_millis(1_500)).await;
                tokio::task::yield_now().await;
                h.in_app(|| change_speed(ctx, 2));
            }
            let shown = view(&h).expect("a run");
            assert_eq!(
                shown.phase,
                ContestPhase::Paused {
                    why: PauseWhy::Hidden
                }
            );
            let now = MonoMs(mono_ms());
            let clock = shown.clock.expect("running");
            assert_eq!(clock.paused_ms(now), now.0 - ticked, "paused from the tick");
        });
    }

    /// End run while paused keeps the run, as it would running.
    #[test]
    fn ending_a_paused_run_keeps_it() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            let ctx = h.ctx;
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            h.pump();
            assert!(!h.lease().is_current());
            end(&mut h);
            assert_eq!(h.screen(), Screen::Results);
            assert!(
                h.ctx
                    .result
                    .peek()
                    .clone()
                    .and_then(Finished::kept)
                    .is_some()
            );
            let (_, log) = stored_last(&h);
            assert_eq!(log.qsos.len(), 1);
            assert_eq!(h.store.load_sessions().len(), 1);
        });
    }

    /// News under an earlier run's lease never reaches a paused run, nor
    /// the copies it keeps: told that every turn of the run before was held
    /// up, the run keeps its copies timed.
    #[test]
    fn news_of_an_earlier_run_never_reaches_a_paused_run() {
        run(|| async {
            crate::time::pin_seeds(Some(7));
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let older = h.lease();
            let ctx = h.ctx;
            h.in_app(|| crate::session_runtime::abort_run(ctx));
            h.pump();
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            h.pump();
            let writes = h.count_writes(h.ctx.contest);
            h.in_app(|| {
                for id in 1..=200 {
                    dispatch(ctx, &older, ContestEvent::TurnHeldUp { id });
                }
            });
            h.pump();
            assert_eq!(
                writes.load(std::sync::atomic::Ordering::Relaxed),
                0,
                "an earlier run's news was published"
            );
            h.in_app(|| resume_run(ctx));
            h.pump();
            end(&mut h);
            let (kept, _) = stored_last(&h);
            assert!(
                kept.group_timings
                    .iter()
                    .all(|timing| timing.char_times.is_some() && timing.untimed.is_none()),
                "{:?}",
                kept.group_timings
            );
        });
    }

    /// The owner's first rule: a QSO worked with no pause, every turn of it
    /// played out as planned, keeps its copies' times, and says of none
    /// why it has none.
    #[test]
    fn a_qso_logged_with_no_pause_is_stored_timed() {
        run(|| async {
            crate::time::pin_seeds(Some(7));
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            end(&mut h);
            let (kept, log) = stored_last(&h);
            assert_eq!(log.qsos.len(), 1);
            assert_eq!(kept.groups.len(), 2, "the call and its number");
            assert!(
                kept.group_timings
                    .iter()
                    .all(|timing| timing.char_times.is_some() && timing.untimed.is_none()),
                "{:?}",
                kept.group_timings
            );
        });
    }

    /// The owner's third rule, at End: a run ended while paused with a QSO
    /// open keeps that QSO, as lost, beside the one logged.
    #[test]
    fn ending_a_run_paused_mid_qso_keeps_that_qso_as_lost() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            open_a_second_qso(&mut h).await;
            let ctx = h.ctx;
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            h.pump();
            end(&mut h);
            let (_, log) = stored_last(&h);
            let kinds: Vec<&str> = log.qsos.iter().map(|qso| qso.k.as_str()).collect();
            assert_eq!(kinds, ["", "lost"]);
        });
    }

    /// The owner's fourth rule: a pause and Resume change no QSO logged
    /// before the pause, its record or its groups, field for field and
    /// time for time, against what the mark held of it before the pause:
    /// paused by Back, by the page going out of sight, or by a gap the app
    /// was not running in.
    #[test]
    fn resuming_a_run_changes_no_qso_logged_before_its_pause() {
        run(|| async {
            for way in ["Back", "the page hidden", "a gap away"] {
                let mut h = Harness::with_settings(contest_settings());
                h.start_training();
                work(&mut h, 60_000, Some(1)).await;
                let checked = |h: &Harness| {
                    view(h).is_some_and(|view| {
                        view.log
                            .first()
                            .is_some_and(|row| matches!(row.status, cw_core::RowStatus::Checked(_)))
                    })
                };
                assert!(h.run_until(15_000, checked).await, "{way}");
                let before = h
                    .store
                    .open()
                    .and_then(|open| open.so_far)
                    .expect("the QSO with the mark")
                    .result;
                let before_log = before
                    .contest
                    .as_ref()
                    .and_then(|log| log.run(before.groups.len()))
                    .expect("its log");
                let owned = before_log.qsos[0].g as usize;
                let ctx = h.ctx;
                match way {
                    "Back" => assert!(h.in_app(|| crate::session_runtime::hold_run(ctx)), "{way}"),
                    "the page hidden" => {
                        h.set_page_hidden(true);
                        h.advance(u64::from(POLL_MS)).await;
                        h.in_app(|| pause_run(ctx, PauseWhy::Hidden));
                    }
                    _ => {
                        h.in_app(|| tick(ctx));
                        tokio::time::advance(std::time::Duration::from_secs(5)).await;
                        tokio::task::yield_now().await;
                        h.pump();
                        h.in_app(|| tick(ctx));
                    }
                }
                h.pump();
                assert!(
                    matches!(phase(&h), Some(ContestPhase::Paused { .. })),
                    "{way}: {:?}",
                    phase(&h)
                );
                h.advance(5_000).await;
                h.set_page_hidden(false);
                h.in_app(|| resume_run(ctx));
                h.pump();
                work(&mut h, 60_000, Some(2)).await;
                end(&mut h);
                let (kept, log) = stored_last(&h);
                assert!(log.qsos.len() >= 2, "{way}: {:?}", log.qsos);
                assert_eq!(log.qsos[0], before_log.qsos[0], "{way}");
                assert_eq!(kept.groups[..owned], before.groups[..owned], "{way}");
                assert_eq!(
                    kept.group_timings[..owned],
                    before.group_timings[..owned],
                    "{way}"
                );
            }
        });
    }

    /// A run whose turns were held up on their way, as a native stream
    /// fallen behind the wall clock finds them, keeps its copies untimed,
    /// each saying so, as a session's group says it.
    #[test]
    fn a_run_whose_turn_was_held_up_stores_its_copies_saying_so() {
        run(|| async {
            crate::time::pin_seeds(Some(7));
            let mut h = Harness::with_settings(contest_settings());
            h.set_behaviour(Behaviour::Late);
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            end(&mut h);
            let (kept, log) = stored_last(&h);
            assert_eq!(log.qsos.len(), 1);
            assert_eq!(kept.groups.len(), 2, "the call and its number");
            for timing in &kept.group_timings {
                assert_eq!(
                    (timing.char_times.clone(), timing.untimed),
                    (None, Some(cw_core::Untimed::HeldUp))
                );
            }
        });
    }

    /// The owner's second rule, for a run: a call heard whole and alone in
    /// the turn on the air, and keyed as soon as it was heard, is stored
    /// with no times when the page going out of sight cut that turn short,
    /// in the moment before the turn's end was told, and says it was
    /// paused.
    #[test]
    fn a_run_paused_mid_turn_stores_the_copies_heard_in_it_as_paused() {
        run(|| async {
            crate::time::pin_seeds(Some(7));
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let cq = h.recorder.starts()[0].clone();
            let call = cq.transcript.others[0].text.clone();
            let heard_by = cq.plan.others[0]
                .events
                .iter()
                .map(|tone| tone.end_sec())
                .fold(0.0, f64::max);
            let call_ends = cq.at.0 + (heard_by * 1_000.0).ceil() as u64;
            // The turn's end is told a poll or so after its sound ends.
            h.advance(call_ends - mono_ms()).await;
            type_into(&mut h, EntryField::Call, &call);
            h.advance(4).await;
            assert!(
                view(&h).is_some_and(|view| view.air.is_some()),
                "the turn's end was told"
            );
            let ctx = h.ctx;
            h.set_page_hidden(true);
            h.in_app(|| pause_run(ctx, PauseWhy::Hidden));
            h.pump();
            h.advance(5_000).await;
            h.set_page_hidden(false);
            h.in_app(|| resume_run(ctx));
            h.pump();
            // Its call sent, its number copied, and the QSO logged.
            assert!(h.run_until(10_000, quiet).await);
            press(&mut h, OpKey::Esm);
            work(&mut h, 30_000, Some(1)).await;
            end(&mut h);
            let (kept, _) = stored_last(&h);
            assert_eq!(kept.groups[0].sent, call);
            let timing = &kept.group_timings[0];
            assert_eq!(
                (timing.char_times.clone(), timing.untimed),
                (None, Some(cw_core::Untimed::Paused))
            );
        });
    }

    /// The owner's second rule, for a run, as it comes most often: the page
    /// going out of sight part way through a caller's call, its first
    /// sending, leaves the copy of that call with no times, saying it was
    /// paused, where a run worked with no pause keeps it timed.
    #[test]
    fn a_run_paused_part_way_through_a_call_stores_its_copy_as_paused() {
        run(|| async {
            for cut in [false, true] {
                crate::time::pin_seeds(Some(7));
                let mut h = Harness::with_settings(contest_settings());
                h.start_training();
                let cq = h.recorder.starts()[0].clone();
                let call = cq.transcript.others[0].text.clone();
                let tones = &cq.plan.others[0].events;
                let begins = tones
                    .iter()
                    .map(|tone| tone.start_sec)
                    .fold(f64::INFINITY, f64::min);
                let heard_by = tones.iter().map(|tone| tone.end_sec()).fold(0.0, f64::max);
                let mid = cq.at.0 + ((begins + heard_by) / 2.0 * 1_000.0) as u64;
                h.advance(mid - mono_ms()).await;
                assert!(view(&h).is_some_and(|view| view.air.is_some()), "{cut}");
                if cut {
                    let ctx = h.ctx;
                    h.set_page_hidden(true);
                    h.in_app(|| pause_run(ctx, PauseWhy::Hidden));
                    h.pump();
                    h.advance(5_000).await;
                    h.set_page_hidden(false);
                    h.in_app(|| resume_run(ctx));
                    h.pump();
                }
                work(&mut h, 30_000, Some(1)).await;
                end(&mut h);
                let (kept, _) = stored_last(&h);
                assert_eq!(kept.groups[0].sent, call, "{cut}");
                let timing = &kept.group_timings[0];
                if cut {
                    assert_eq!(
                        (timing.char_times.clone(), timing.untimed),
                        (None, Some(cw_core::Untimed::Paused))
                    );
                } else {
                    assert!(
                        timing.char_times.is_some() && timing.untimed.is_none(),
                        "{timing:?}"
                    );
                }
            }
        });
    }

    /// The owner's third rule, cut off: a run the app stops under while it
    /// is paused is kept at the next launch as End run would have kept it
    /// at the pause, the QSO the pause cut kept as lost beside the one
    /// logged.
    #[test]
    fn a_run_the_app_stops_under_while_paused_keeps_the_qso_the_pause_cut() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.store.mark_tutorial_done();
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            open_a_second_qso(&mut h).await;
            let ctx = h.ctx;
            h.in_app(|| pause_run(ctx, PauseWhy::Hidden));
            h.pump();
            assert!(matches!(phase(&h), Some(ContestPhase::Paused { .. })));
            assert!(h.store.load_sessions().is_empty(), "stored while paused");

            let (mut ui, _recorder) = crate::testing::Ui::launch(h.store.clone());
            ui.advance(20).await;
            let (_, log) = stored_last(&h);
            let kinds: Vec<&str> = log.qsos.iter().map(|qso| qso.k.as_str()).collect();
            assert_eq!(kinds, ["", "lost"]);
        });
    }

    /// A paused run still hears what comes under the lease its pause gave
    /// back, as a paused session does: told that the turns of its QSO were
    /// held up, it keeps the QSO's copies untimed, each saying so, where a
    /// run told nothing keeps them timed.
    #[test]
    fn a_paused_run_hears_the_news_of_its_own_sends() {
        run(|| async {
            for told in [false, true] {
                crate::time::pin_seeds(Some(7));
                let mut h = Harness::with_settings(contest_settings());
                h.start_training();
                work(&mut h, 60_000, Some(1)).await;
                let cut = h.lease();
                let ctx = h.ctx;
                h.in_app(|| pause_run(ctx, PauseWhy::You));
                h.pump();
                assert!(!cut.is_current(), "{told}");
                if told {
                    h.in_app(|| {
                        for id in 1..=200 {
                            dispatch(ctx, &cut, ContestEvent::TurnHeldUp { id });
                        }
                    });
                    h.pump();
                }
                h.in_app(|| resume_run(ctx));
                h.pump();
                end(&mut h);
                let (kept, log) = stored_last(&h);
                assert!(!log.qsos.is_empty(), "{told}");
                for timing in &kept.group_timings {
                    if told {
                        assert_eq!(
                            (timing.char_times.clone(), timing.untimed),
                            (None, Some(cw_core::Untimed::HeldUp)),
                            "{told}"
                        );
                    } else {
                        assert!(
                            timing.char_times.is_some() && timing.untimed.is_none(),
                            "{told}: {timing:?}"
                        );
                    }
                }
            }
        });
    }

    /// Resume handed to a run as an event goes the way Resume's button
    /// does: the audio is claimed afresh inside the tap, and the run goes
    /// on under the new claim, a CQ keyed after it going out.
    #[test]
    fn resume_sent_as_an_event_claims_the_audio_first() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            assert!(h.run_until(10_000, quiet).await);
            let ctx = h.ctx;
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            h.pump();
            let paused = h.lease();
            assert!(!paused.is_current());
            h.in_app(|| send_run_event(ctx, ContestEvent::Resume));
            h.pump();
            let lease = h.lease();
            assert!(
                lease.is_current() && !lease.is(&paused),
                "resumed under the lease the pause gave back"
            );
            assert_eq!(phase(&h), Some(ContestPhase::Running));
            let sends = h.texts().len();
            press(&mut h, OpKey::Cq);
            assert!(
                h.run_until(1_500, |h| h.texts().len() > sends).await,
                "nothing went out after Resume"
            );
        });
    }

    /// What a paused run hears goes in its mark, as it would go in what
    /// End keeps: told while paused that the turns of its QSO were held
    /// up, the run the app stops under then is kept at the next launch as
    /// End would keep it, the QSO's copies untimed and saying so.
    #[test]
    fn news_a_paused_run_hears_goes_in_its_mark() {
        run(|| async {
            for stop in [false, true] {
                crate::time::pin_seeds(Some(7));
                let mut h = Harness::with_settings(contest_settings());
                h.store.mark_tutorial_done();
                h.start_training();
                work(&mut h, 60_000, Some(1)).await;
                let cut = h.lease();
                let ctx = h.ctx;
                h.in_app(|| pause_run(ctx, PauseWhy::You));
                h.pump();
                assert!(!cut.is_current(), "{stop}");
                h.in_app(|| {
                    for id in 1..=200 {
                        dispatch(ctx, &cut, ContestEvent::TurnHeldUp { id });
                    }
                });
                h.pump();
                if stop {
                    let (mut ui, _recorder) = crate::testing::Ui::launch(h.store.clone());
                    ui.advance(20).await;
                } else {
                    end(&mut h);
                }
                let (kept, log) = stored_last(&h);
                assert!(!log.qsos.is_empty(), "stopped {stop}");
                for timing in &kept.group_timings {
                    assert_eq!(
                        (timing.char_times.clone(), timing.untimed),
                        (None, Some(cw_core::Untimed::HeldUp)),
                        "stopped {stop}"
                    );
                }
            }
        });
    }
}
