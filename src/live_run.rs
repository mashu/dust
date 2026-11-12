//! The life of what is live, a session of groups or a contest run, written
//! once for both: what reaches it, each step of it, how it gives the audio
//! back as it pauses, and how it ends, kept or let go. Its machine decides
//! what it does; the runtime holds where it is in its life, its [`Stage`],
//! and makes each move in one place here. Each kind's machine is a
//! [`RunMachine`], whose hooks its own runtime gives.

use std::borrow::Cow;

use cw_core::{MonoMs, PlannedTransmission, SessionResult, TrainingSettings, Transmission};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use crate::persist::SoFar;
use crate::player::{Asked, AudioError, Lease, SendNews, Sent};
use crate::state::live::{LiveRun, Stage};
use crate::state::{AppCtx, Finished, Live, Preview, Screen, keep_and_show};
use crate::time::{local_date_string, mono_ms, now_ms};

/// A machine that can be what is live: what the lifecycle cannot do the
/// same way for a session of groups and for a contest run, in the order of
/// its life.
///
/// Some are called with what is live borrowed from the app — `first`,
/// `hear`, `over`, `as_shown`, `progress`, `progressed` and `so_far` by
/// [`step`], as a step's own `ahead` is, `so_far` by [`give_back`] too,
/// and `sound` by [`resume`] — and must not reach what is live through the
/// app (`app.session`), do what a machine said or dispatch anything, show
/// a toast, or have the answer box finish what is entered
/// ([`AnswerOutbox::finish`], which can hand the session an answer from
/// inside). Taking the typing that waits in the box
/// ([`AnswerOutbox::take`]), which calls nothing back, they may, as a
/// session's `first` and its away step do. `so_far` changes only its copy
/// of the machine. `of`, `of_mut` and `unboxed` are handed the app's slot,
/// or what is taken out of it, while it is borrowed; the rest are called
/// with nothing borrowed.
///
/// [`AnswerOutbox::finish`]: crate::state::AnswerOutbox::finish
/// [`AnswerOutbox::take`]: crate::state::AnswerOutbox::take
pub(crate) trait RunMachine: Sized + 'static {
    /// What it hears: what the user did, or news from its own tasks.
    type Event: PartialEq;
    /// What it says to do.
    type Effect;
    /// What the training screen shows of it.
    type View: Clone + PartialEq + 'static;
    /// How far it has got, for what to mark as it goes.
    type Progress;
    /// Which of its sends news is of: a group's index, a turn's id.
    type Tag: Copy + 'static;
    /// What only its kind holds while it is live.
    type Own;
    /// What keeping it does to its level, settled once it is marked kept.
    type Level;
    /// A contest run, as its mark says ([`AppCtx::mark_running`]).
    const IS_RUN: bool;
    /// What tells it to carry on after a pause, once the audio is claimed
    /// again ([`resume`]).
    const RESUME: Self::Event;

    /// What is live, if it is of this kind.
    fn of(live: &Live) -> Option<&LiveRun<Self, Self::Own>>;

    /// What is live, if it is of this kind, to be changed.
    fn of_mut(live: &mut Live) -> Option<&mut LiveRun<Self, Self::Own>>;

    /// What goes live, of this kind.
    fn boxed(run: LiveRun<Self, Self::Own>) -> Live;

    /// What was live, taken out, if it is of this kind.
    fn unboxed(live: Live) -> Option<LiveRun<Self, Self::Own>>;

    /// Where what it shows is published.
    fn shown(ctx: AppCtx) -> Signal<Option<Self::View>>;

    /// What a claim of the audio for it plays from.
    fn sound(live: &LiveRun<Self, Self::Own>) -> &TrainingSettings;

    /// What its kind does with each claim of the audio it runs under, as
    /// it starts and as it resumes.
    fn watch(_ctx: AppCtx, _lease: &Lease) {}

    /// What its kind does with `event`, come under `lease`, before the
    /// machine hears it: `event` to hear now, or none to hear nothing now.
    fn before(_ctx: AppCtx, _lease: &Lease, event: Self::Event) -> Option<Self::Event> {
        Some(event)
    }

    /// What its machine hears at `now` before any event, as that event
    /// reaches it.
    fn first(live: &mut LiveRun<Self, Self::Own>, ctx: AppCtx, now: MonoMs) -> Vec<Self::Effect>;

    /// What it does on hearing `event` at `now`.
    #[must_use = "the effects are the machine's instructions to the runtime"]
    fn hear(&mut self, event: Self::Event, now: MonoMs) -> Vec<Self::Effect>;

    /// What tells it timer `id` has run out.
    fn timer_fired(id: u64) -> Self::Event;

    /// Whether it is over, its end said.
    fn over(&self) -> bool;

    /// What it shows now.
    fn as_shown(&self) -> Cow<'_, Self::View>;

    /// How far it has got now.
    fn progress(&self) -> Self::Progress;

    /// Whether it has got further since `before` in a way the mark keeps:
    /// a group answered, a QSO logged or checked. `published` is what was
    /// shown of it before, and `shown` what is shown now.
    fn progressed(
        &self,
        before: Self::Progress,
        published: Option<&Self::View>,
        shown: &Self::View,
    ) -> bool;

    /// What End would keep were it pressed at `now`, worked out on a copy
    /// of the machine, which alone it changes: what the mark holds, should
    /// the app stop under it. None while it would keep nothing.
    /// `finished_at` is the wall time it says it ended at.
    fn so_far(live: &LiveRun<Self, Self::Own>, now: MonoMs, finished_at: u64) -> Option<SoFar>;

    /// Do what its machine said, `effect`, under `lease`.
    fn carry_out(ctx: AppCtx, lease: &Lease, effect: Self::Effect);

    /// What its kind does once all its machine said is done.
    fn effects_done(_ctx: AppCtx) {}

    /// What tells it the news of its send `tag`, as the send goes.
    fn news(tag: Self::Tag, news: SendNews<'_>) -> Self::Event;

    /// What tells it how its send `tag` ended: played to its end, stopped,
    /// or failed after every retry, which was said already.
    fn sent(tag: Self::Tag, sent: Result<Sent, AudioError>) -> Self::Event;

    /// What its kind does once it has resumed.
    fn resumed(_ctx: AppCtx) {}

    /// What its kind does as it is taken out, the audio given back.
    fn ending(_ctx: AppCtx, _live: &mut LiveRun<Self, Self::Own>) {}

    /// What it keeps, ended, to be stored and shown, and what that does to
    /// its level.
    fn kept(ctx: AppCtx, live: &LiveRun<Self, Self::Own>) -> (SessionResult, Self::Level);

    /// Its level settled as `level` says: what that said, if anything.
    fn settle(_ctx: AppCtx, _level: Self::Level) -> Option<String> {
        None
    }
}

/// The audio, taken for what goes live: whatever preview had it has
/// stopped, whether or not it can be opened; then claimed to play `sound`
/// ([`claim`]).
pub(crate) fn take_audio(ctx: AppCtx, sound: &TrainingSettings) -> Option<Lease> {
    let mut preview = ctx.preview;
    preview.set(Preview::Idle);
    claim(ctx, sound)
}

/// What was built, `machine`, goes live under `lease`, Running: it has the
/// training screen, is marked running, and is the app's, with what only
/// its kind holds, `own`, made once it is marked, then watched, as its
/// kind watches a claim; then what its machine said at its start is done.
/// It was last seen at `seen`, if it has been.
pub(crate) fn go_live<M: RunMachine>(
    ctx: AppCtx,
    lease: Lease,
    machine: M,
    effects: Vec<M::Effect>,
    seen: Option<MonoMs>,
    own: impl FnOnce() -> M::Own,
) {
    let mut shown = M::shown(ctx);
    shown.set(Some(machine.as_shown().into_owned()));
    let mut screen = ctx.screen;
    screen.set(Screen::Training);
    let mark = ctx.mark_running(M::IS_RUN);
    let live = M::boxed(LiveRun {
        machine,
        lease: lease.clone(),
        stage: Stage::Running,
        seen,
        started_on: local_date_string(),
        mark,
        own: own(),
    });
    *ctx.app().session.borrow_mut() = Some(live);
    M::watch(ctx, &lease);
    run_effects::<M>(ctx, &lease, effects);
}

/// The audio, claimed to play `sound`; or, if it cannot be had, a toast
/// that says why, and none.
pub(crate) fn claim(ctx: AppCtx, sound: &TrainingSettings) -> Option<Lease> {
    let claimed = ctx.app().player.claim(sound);
    claimed
        .inspect_err(|err| ctx.toasts.show(err.to_string()))
        .ok()
}

/// Whether what comes under `lease` reaches `live`, as its stage says
/// ([`Stage::hears`]).
pub(crate) fn hears<M: RunMachine>(live: &LiveRun<M, M::Own>, lease: &Lease) -> bool {
    live.stage.hears(live.lease.is(lease), lease.is_current())
}

/// The hold on the audio of what is live, if it is of kind `M`: what the
/// user does to it comes under it.
pub(crate) fn lease<M: RunMachine>(ctx: AppCtx) -> Option<Lease> {
    let app = ctx.app();
    let slot = app.session.borrow();
    let live = slot.as_ref().and_then(M::of)?;
    // Running, it holds the audio: nothing else claims it or gives it back
    // while anything is live, so what the user does, under its lease,
    // reaches it.
    #[cfg(test)]
    assert!(
        live.stage != Stage::Running || live.lease.is_current(),
        "what is live runs without the audio"
    );
    Some(live.lease.clone())
}

/// Hand what is live of kind `M` something the user did, under its lease.
/// Nothing happens when nothing of that kind is live. Resume claims the
/// audio first ([`resume`]).
pub(crate) fn send<M: RunMachine>(ctx: AppCtx, event: M::Event) {
    if event == M::RESUME {
        resume::<M>(ctx);
    } else if let Some(lease) = lease::<M>(ctx) {
        dispatch::<M>(ctx, &lease, event);
    }
}

/// Carry on after a pause. The audio a pause gave back is claimed afresh
/// here, inside the tap on Resume, so the player opens — or, after the
/// sound failed, is built anew — where a browser lets it start; what is
/// live of kind `M` is Running again under the new claim, and its machine
/// hears Resume. A claim that fails says why, and it stays Paused. What is
/// not paused is left alone: a claim would cut its send from under it.
pub(crate) fn resume<M: RunMachine>(ctx: AppCtx) {
    let app = ctx.app();
    let sound = match app.session.borrow().as_ref().and_then(M::of) {
        Some(live) if live.stage == Stage::Paused => M::sound(live).clone(),
        _ => return,
    };
    let Some(lease) = claim(ctx, &sound) else {
        return;
    };
    if let Some(live) = app.session.borrow_mut().as_mut().and_then(M::of_mut)
        && let Some(running) = live.stage.claimed()
    {
        live.lease = lease.clone();
        live.stage = running;
    }
    dispatch::<M>(ctx, &lease, M::RESUME);
    M::watch(ctx, &lease);
    M::resumed(ctx);
}

/// Hand what is live of kind `M` `event`, come under `lease`: whatever its
/// kind does first ([`RunMachine::before`]), then a step that hears it.
pub(crate) fn dispatch<M: RunMachine>(ctx: AppCtx, lease: &Lease, event: M::Event) {
    let Some(event) = M::before(ctx, lease, event) else {
        return;
    };
    step::<M>(ctx, lease, Some(event), |live, now| {
        Some(M::first(live, ctx, now))
    });
}

/// One step of what is live of kind `M`, if what comes under `lease`
/// reaches it ([`hears`]): `ahead` hands its machine what comes before
/// `event` at this moment — typing still waiting, a gap the app was not
/// running in — or says it has nothing to hear; then the machine hears
/// `event`, if there is one. What it shows is published if that changed,
/// and what End would keep goes with its mark, should the app stop under
/// it, once it gets further, or once what it hears while paused changes
/// that; then what its machine said is done, under its lease. `ahead` runs
/// with what is live borrowed ([`RunMachine`]).
pub(crate) fn step<M: RunMachine>(
    ctx: AppCtx,
    lease: &Lease,
    event: Option<M::Event>,
    ahead: impl FnOnce(&mut LiveRun<M, M::Own>, MonoMs) -> Option<Vec<M::Effect>>,
) {
    let mut shown = M::shown(ctx);
    let app = ctx.app();
    let (lease, effects, so_far) = {
        let mut slot = app.session.borrow_mut();
        let Some(live) = slot
            .as_mut()
            .and_then(M::of_mut)
            .filter(|live| hears(live, lease))
        else {
            return;
        };
        let now = MonoMs(mono_ms());
        let finished_at = now_ms();
        let before = live.machine.progress();
        // Paused, having given the audio back, what it would keep before it
        // hears this: news of a send it hears now as held up takes the
        // times of what was heard in it.
        let paused = (event.is_some() && live.stage == Stage::Paused)
            .then(|| M::so_far(live, now, finished_at));
        let Some(mut effects) = ahead(live, now) else {
            return;
        };
        if let Some(event) = event {
            effects.extend(live.machine.hear(event, now));
        }
        let view = live.machine.as_shown();
        // Further, or what it would keep changed by what it heard while
        // paused, and going on: what it would keep now goes with its mark.
        let so_far = if live.machine.over() {
            None
        } else if live
            .machine
            .progressed(before, shown.peek().as_ref(), &view)
        {
            M::so_far(live, now, finished_at)
        } else {
            paused.and_then(|before| {
                M::so_far(live, now, finished_at).filter(|after| before.as_ref() != Some(after))
            })
        }
        .map(|so_far| (live.mark.clone(), so_far));
        // Much of what arrives changes nothing — a key while the box is
        // locked, a stale send's news, a timer the machine has forgotten —
        // and publishing it anyway would redraw the screen for it.
        if shown.peek().as_ref() != Some(&*view) {
            shown.set(Some(view.into_owned()));
        }
        (live.lease.clone(), effects, so_far)
    };
    if let Some((mark, so_far)) = so_far {
        ctx.mark_so_far(&mark, so_far);
    }
    run_effects::<M>(ctx, &lease, effects);
}

/// Do what the machine of kind `M` said, under `lease`.
pub(crate) fn run_effects<M: RunMachine>(ctx: AppCtx, lease: &Lease, effects: Vec<M::Effect>) {
    for effect in effects {
        M::carry_out(ctx, lease, effect);
    }
    M::effects_done(ctx);
}

/// Tell the machine of kind `M` its timer `id` ran out, `ms` from now:
/// one wait for the whole of it. A timer the machine has since forgotten
/// wakes once, is told so, and is gone; one a pause or an end let go of
/// with the audio never wakes ([`Lease::after`]).
pub(crate) fn start_timer<M: RunMachine>(ctx: AppCtx, lease: &Lease, id: u64, ms: u32) {
    lease.after(ms, move |lease| {
        dispatch::<M>(ctx, lease, M::timer_fired(id))
    });
}

/// What a send plays: a session's group, or a run's turn, planned already,
/// asked for as it went on the air ([`Lease::ask_to_send`]).
pub(crate) enum Outgoing {
    Group(Transmission),
    Turn {
        asked: Asked,
        plan: PlannedTransmission,
        transcript: Transmission,
        starts_at: Option<MonoMs>,
    },
}

/// Play `outgoing`, send `tag` of the machine of kind `M`, from `settings`,
/// on a task of the app's own, and tell the machine of each attempt at it
/// as it goes, and how it ended. A failure after every retry is said, once.
/// A group takes its place among the player's sends as its task first
/// runs; a turn was asked for as it went on the air.
pub(crate) fn play<M: RunMachine>(
    ctx: AppCtx,
    lease: &Lease,
    tag: M::Tag,
    settings: TrainingSettings,
    outgoing: Outgoing,
) {
    let lease = lease.clone();
    spawn_forever(async move {
        let news = |news: SendNews<'_>| dispatch::<M>(ctx, &lease, M::news(tag, news));
        let sent = match &outgoing {
            Outgoing::Group(transmission) => lease.send(transmission, &settings, news).await,
            Outgoing::Turn {
                asked,
                plan,
                transcript,
                starts_at,
            } => {
                lease
                    .send_planned(*asked, plan, transcript, &settings, *starts_at, news)
                    .await
            }
        };
        if let Err(failure) = &sent
            && *failure != AudioError::Cancelled
        {
            ctx.toasts.show(failure.to_string());
        }
        dispatch::<M>(ctx, &lease, M::sent(tag, sent));
    });
}

/// What is live under `lease` has paused, and gives the audio back: it is
/// Paused, and what End would keep now goes with its mark, the group or
/// the QSO the pause cut as it stood, should the app stop while it is
/// paused. What it hears while paused that changes that puts it there
/// anew.
pub(crate) fn give_back<M: RunMachine>(ctx: AppCtx, lease: &Lease) {
    lease.release();
    let so_far = {
        let app = ctx.app();
        let mut slot = app.session.borrow_mut();
        let Some(live) = slot
            .as_mut()
            .and_then(M::of_mut)
            .filter(|live| live.lease.is(lease))
        else {
            return;
        };
        let Some(paused) = live.stage.gave_back() else {
            return;
        };
        live.stage = paused;
        M::so_far(live, MonoMs(mono_ms()), now_ms()).map(|so_far| (live.mark.clone(), so_far))
    };
    if let Some((mark, so_far)) = so_far {
        ctx.mark_so_far(&mark, so_far);
    }
}

/// What is live, taken out of the app if it is of kind `M`, to be kept or
/// let go: Ending, with the audio given back. Its stage is asked before it
/// is taken out, so what cannot end is left as it is. It is marked over
/// once it is stored or let go.
pub(crate) fn end<M: RunMachine>(ctx: AppCtx) -> Option<LiveRun<M, M::Own>> {
    let mut live = {
        let app = ctx.app();
        let mut slot = app.session.borrow_mut();
        // What the app holds lives Running or Paused, so is taken out once.
        let ending = slot.as_ref().and_then(M::of)?.stage.ending()?;
        let mut live = slot.take().and_then(M::unboxed)?;
        live.stage = ending;
        live
    };
    live.lease.release();
    M::ending(ctx, &mut live);
    Some(live)
}

/// What is live of kind `M` ends, kept: what it keeps goes with its mark as
/// it is to be stored, then its level is settled and its screen cleared,
/// and it is stored and shown. Marked before its level moves, it is kept
/// once, its level counted once, should the app stop from here on. Whether
/// there was one.
pub(crate) fn keep<M: RunMachine>(ctx: AppCtx) -> bool {
    let Some(live) = end::<M>(ctx) else {
        return false;
    };
    let (result, level) = M::kept(ctx, &live);
    let so_far = SoFar {
        result: result.clone(),
        settings: None,
    };
    ctx.mark_so_far(&live.mark, so_far);
    let mut auto_message = ctx.auto_message;
    auto_message.set(M::settle(ctx, level));
    M::shown(ctx).set(None);
    keep_and_show(ctx, result);
    true
}

/// What is live of kind `M` ends, let go: nothing of it is kept. The
/// results screen says so, as `finished` has it, or the app goes Home.
/// Whether there was one.
pub(crate) fn let_go<M: RunMachine>(ctx: AppCtx, finished: Option<Finished>) -> bool {
    if end::<M>(ctx).is_none() {
        return false;
    }
    ctx.mark_ended();
    M::shown(ctx).set(None);
    let AppCtx {
        mut screen,
        mut result,
        mut auto_message,
        ..
    } = ctx;
    match finished {
        Some(finished) => {
            result.set(Some(finished));
            auto_message.set(None);
            screen.set(Screen::Results);
        }
        None => screen.set(Screen::Home),
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering::Relaxed;

    use cw_core::{ContestEvent, Group, OpKey, PauseWhy, RowStatus, SessionEvent};

    use super::*;
    use crate::contest_runtime::Run;
    use crate::persist::Store;
    use crate::session_runtime::{Groups, finish_run};
    use crate::testing::{
        Harness, Ui, contest_settings, copy_what_was_heard, press, quiet, run, test_settings,
    };
    use crate::time::POLL_MS;

    /// What a test does to what is live of one kind, as its screen does.
    trait Driven: RunMachine {
        /// The kind, for what a test says when it fails.
        const NAME: &'static str;

        /// The settings it starts from.
        fn settings() -> TrainingSettings;

        /// What its Pause sends.
        fn pause() -> Self::Event;

        /// Get it as far as one thing it keeps: a group answered, a QSO
        /// logged.
        async fn earn(h: &mut Harness);
    }

    impl Driven for Groups {
        const NAME: &'static str = "a session";

        fn settings() -> TrainingSettings {
            test_settings()
        }

        fn pause() -> SessionEvent {
            SessionEvent::Pause
        }

        async fn earn(h: &mut Harness) {
            assert!(
                h.run_until(10_000, |h| h.awaiting_answer() == Some(0))
                    .await
            );
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            let answered = |h: &Harness| {
                let runtime = h.ctx.runtime.peek();
                let first = runtime.as_ref().and_then(|session| session.group(0));
                first.is_some_and(Group::confirmed)
            };
            assert!(h.run_until(10_000, answered).await, "the group is answered");
        }
    }

    impl Driven for Run {
        const NAME: &'static str = "a run";

        fn settings() -> TrainingSettings {
            contest_settings()
        }

        fn pause() -> ContestEvent {
            let at = MonoMs(mono_ms());
            ContestEvent::Pause {
                why: PauseWhy::You,
                at,
            }
        }

        /// Its check shown too, so that nothing of it is left for a pause
        /// to settle.
        async fn earn(h: &mut Harness) {
            let logged = |h: &Harness| {
                h.ctx.contest.peek().as_ref().is_some_and(|run| {
                    run.qsos > 0
                        && run
                            .log
                            .iter()
                            .all(|row| !matches!(row.status, RowStatus::Pending))
                })
            };
            let mut waited = 0;
            while !logged(h) && waited < 60_000 {
                if quiet(h) {
                    copy_what_was_heard(h);
                    press(h, OpKey::Esm);
                }
                h.advance(u64::from(POLL_MS)).await;
                waited += u64::from(POLL_MS);
            }
            assert!(logged(h), "a QSO is logged");
        }
    }

    /// Where what is live of kind `M` is in its life, if it is live.
    fn stage<M: RunMachine>(h: &Harness) -> Option<Stage> {
        let slot = h.app.session.borrow();
        slot.as_ref().and_then(M::of).map(|live| live.stage)
    }

    /// Whether what comes under `lease` reaches what is live of kind `M`.
    fn reaches<M: RunMachine>(h: &Harness, lease: &Lease) -> bool {
        let slot = h.app.session.borrow();
        slot.as_ref()
            .and_then(M::of)
            .is_some_and(|live| hears(live, lease))
    }

    /// `result` as it is stored, but for when it ended.
    fn but_when(result: &SessionResult) -> serde_json::Value {
        let mut stored = serde_json::to_value(result).expect("a session writes");
        if let Some(stored) = stored.as_object_mut() {
            stored.remove("date");
            stored.remove("timestamp");
        }
        stored
    }

    /// Whether the mark holds what End would keep of what is live of kind
    /// `M`, were it pressed now.
    fn marked_as_end_keeps<M: RunMachine>(h: &Harness) -> bool {
        let end_now = {
            let slot = h.app.session.borrow();
            let live = slot.as_ref().and_then(M::of).expect("live");
            M::so_far(live, MonoMs(mono_ms()), 0)
        };
        let marked = h.store.open().and_then(|open| open.so_far);
        marked.map(|so_far| but_when(&so_far.result))
            == end_now.map(|so_far| but_when(&so_far.result))
    }

    /// What is live of kind `M` lives one life. Refused the audio, it never
    /// goes live, and says why; it goes live Running, and hears what comes
    /// under its own lease. Its sound failing after every retry pauses it:
    /// Paused, the audio given back, its mark holding what End would keep,
    /// it still hears what comes under the lease it gave back. Resume
    /// refused the audio says why and leaves it Paused; Resume with the
    /// audio has it Running under a new lease, which alone it hears. Its
    /// Pause gives the audio back again, and End takes it out, once.
    async fn life<M: Driven>() {
        let name = M::NAME;
        let mut h = Harness::with_settings(M::settings());
        let ctx = h.ctx;
        h.recorder.build_error.set(true);
        h.start_training();
        assert_eq!(stage::<M>(&h), None, "{name}: gone live without the audio");
        assert_eq!(h.toast().as_deref(), Some("No audio output device found"));

        h.recorder.build_error.set(false);
        h.recorder.fail_next(3);
        h.start_training();
        assert_eq!(stage::<M>(&h), Some(Stage::Running), "{name}: started");
        let first = h.lease();
        assert!(reaches::<M>(&h, &first), "{name}: deaf to its own lease");
        let failed = |h: &Harness| stage::<M>(h) == Some(Stage::Paused);
        assert!(
            h.run_until(30_000, failed).await,
            "{name}: its sound failed"
        );
        assert!(!first.is_current(), "{name}: kept the audio, paused");
        assert!(
            reaches::<M>(&h, &first),
            "{name}: deaf, paused, to its own lease"
        );
        assert!(
            marked_as_end_keeps::<M>(&h),
            "{name}: marked otherwise than End keeps"
        );

        h.recorder.build_error.set(true);
        h.in_app(|| resume::<M>(ctx));
        assert_eq!(
            stage::<M>(&h),
            Some(Stage::Paused),
            "{name}: resumed without the audio"
        );
        assert_eq!(h.toast().as_deref(), Some("No audio output device found"));
        h.recorder.build_error.set(false);
        h.in_app(|| resume::<M>(ctx));
        assert_eq!(stage::<M>(&h), Some(Stage::Running), "{name}: not resumed");
        let second = h.lease();
        assert!(
            second.is_current() && !second.is(&first),
            "{name}: resumed on its old lease"
        );
        assert!(reaches::<M>(&h, &second), "{name}: deaf to its new lease");
        assert!(
            !reaches::<M>(&h, &first),
            "{name}: still hears the lease it gave back"
        );

        h.in_app(|| send::<M>(ctx, M::pause()));
        assert_eq!(stage::<M>(&h), Some(Stage::Paused), "{name}: not paused");
        assert!(!second.is_current(), "{name}: kept the audio, paused");
        assert!(
            marked_as_end_keeps::<M>(&h),
            "{name}: marked otherwise than End keeps"
        );

        h.in_app(|| finish_run(ctx));
        assert_eq!(stage::<M>(&h), None, "{name}: not ended");
        assert!(!h.in_app(|| keep::<M>(ctx)), "{name}: kept twice");
        assert!(
            !h.in_app(|| let_go::<M>(ctx, None)),
            "{name}: let go once ended"
        );
    }

    #[test]
    fn either_kind_lives_one_life() {
        run(|| async {
            life::<Groups>().await;
            life::<Run>().await;
        });
    }

    /// What is live of kind `M` publishes what it shows only when that
    /// changes, and marks what it would keep only once it gets further or
    /// pauses: a timer it has forgotten changes neither; what it earns goes
    /// with its mark; its Pause is shown once and marked once, as End
    /// would keep it then; and Resume marks nothing.
    async fn shows_and_marks<M: Driven>() {
        let name = M::NAME;
        let mut h = Harness::with_settings(M::settings());
        let ctx = h.ctx;
        h.start_training();
        let lease = h.lease();
        let shown = h.count_writes(M::shown(ctx));
        let marks = h.store.mark_log.borrow().len();
        let counts = |h: &Harness| {
            let marked = h.store.mark_log.borrow().len() - marks;
            (shown.load(Relaxed), marked)
        };

        h.in_app(|| dispatch::<M>(ctx, &lease, M::timer_fired(u64::MAX)));
        assert_eq!(
            counts(&h),
            (0, 0),
            "{name}: a forgotten timer shown or marked"
        );

        M::earn(&mut h).await;
        let (shown_at, marked_at) = counts(&h);
        assert!(marked_at > 0, "{name}: what it earned left unmarked");
        let open = h.store.open().expect("marked");
        assert!(
            open.so_far.is_some(),
            "{name}: the mark holds nothing earned"
        );

        h.in_app(|| send::<M>(ctx, M::pause()));
        let paused = (shown_at + 1, marked_at + 1);
        assert_eq!(
            counts(&h),
            paused,
            "{name}: its pause shown or marked otherwise"
        );
        assert!(
            marked_as_end_keeps::<M>(&h),
            "{name}: marked otherwise than End keeps"
        );
        h.in_app(|| resume::<M>(ctx));
        assert_eq!(counts(&h).1, marked_at + 1, "{name}: Resume marked");
    }

    #[test]
    fn either_kind_shows_and_marks_only_what_changed() {
        run(|| async {
            shows_and_marks::<Groups>().await;
            shows_and_marks::<Run>().await;
        });
    }

    /// What is live of kind `M`, ended, goes with its mark as it is to be
    /// stored before the store has it, and is kept once: by the store, and
    /// not again by the next launch, as the app stopping after that would
    /// leave it.
    async fn kept_once<M: Driven>() {
        let name = M::NAME;
        let mut h = Harness::with_settings(M::settings());
        h.store.mark_tutorial_done();
        h.store.deferred.set(true);
        let ctx = h.ctx;
        h.start_training();
        M::earn(&mut h).await;
        h.in_app(|| finish_run(ctx));
        assert_eq!(stage::<M>(&h), None, "{name}: not ended");
        let open = h.store.open().expect("marked");
        assert!(open.over, "{name}: marked kept");
        let kept = h.ctx.result.peek().clone().and_then(Finished::kept);
        let kept = kept.expect("shown kept");
        let marked = open.so_far.expect("held with its mark").result;
        assert_eq!(
            but_when(&marked),
            but_when(&kept),
            "{name}: marked otherwise than kept"
        );
        assert!(h.store.load_sessions().is_empty(), "{name}: stored at once");

        h.store.settle();
        h.advance(1).await;
        assert_eq!(h.store.load_sessions().len(), 1, "{name}: not stored once");
        let (mut ui, _recorder) = Ui::launch(h.store.clone());
        ui.advance(20).await;
        h.store.settle();
        assert_eq!(h.store.load_sessions().len(), 1, "{name}: kept again");
    }

    #[test]
    fn either_kind_is_marked_as_kept_and_kept_once() {
        run(|| async {
            kept_once::<Groups>().await;
            kept_once::<Run>().await;
        });
    }

    /// What is live of kind `M` whose stage refuses to end is left where it
    /// is by an end, which asks the stage before it takes anything out:
    /// still the app's, holding its audio, still shown, and neither kept
    /// nor let go. The app never holds such a stage; a test makes one.
    async fn left_as_it_is_when_it_cannot_end<M: Driven>() {
        let name = M::NAME;
        let mut h = Harness::with_settings(M::settings());
        let ctx = h.ctx;
        h.start_training();
        let lease = h.lease();
        if let Some(live) = h.app.session.borrow_mut().as_mut().and_then(M::of_mut) {
            live.stage = Stage::Ending;
        }
        assert!(h.in_app(|| end::<M>(ctx)).is_none(), "{name}: ended again");
        assert_eq!(stage::<M>(&h), Some(Stage::Ending), "{name}: taken out");
        assert!(lease.is_current(), "{name}: its audio given back");
        assert!(M::shown(ctx).peek().is_some(), "{name}: no longer shown");
        assert!(!h.in_app(|| keep::<M>(ctx)), "{name}: kept");
        assert!(!h.in_app(|| let_go::<M>(ctx, None)), "{name}: let go");
        assert_eq!(stage::<M>(&h), Some(Stage::Ending), "{name}: taken out");
    }

    #[test]
    fn either_kind_whose_stage_cannot_end_is_left_as_it_is() {
        run(|| async {
            left_as_it_is_when_it_cannot_end::<Groups>().await;
            left_as_it_is_when_it_cannot_end::<Run>().await;
        });
    }
}
