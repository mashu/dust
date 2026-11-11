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
use crate::state::{AppCtx, Finished, Live, Screen, keep_and_show};
use crate::time::{mono_ms, now_ms};

/// A machine that can be what is live: what the lifecycle cannot do the
/// same way for a session of groups and for a contest run.
///
/// [`step`] calls some of these with what is live borrowed from the app
/// (`first`, `progress`, `progressed`, `so_far`, `as_shown`, `hear`, and
/// a step's own `ahead`): they must leave the app alone, borrowing
/// nothing of it, doing nothing the machine said, handing nothing to it,
/// and showing no toast. `so_far` changes only its copy of the machine. The
/// others are called with nothing borrowed.
pub(crate) trait RunMachine: Sized + 'static {
    /// What it hears: what the user did, or news from its own tasks.
    type Event;
    /// What it says to do.
    type Effect;
    /// What the training screen shows of it.
    type View: Clone + PartialEq + 'static;
    /// How far it has got, for what to mark as it goes.
    type Progress;
    /// What only its kind holds while it is live.
    type Own;
    /// Which of its sends news is of: a group's index, a turn's id.
    type Tag: Copy + 'static;
    /// What keeping it does to its level, settled once it is marked kept.
    type Level;

    /// What is live, if it is of this kind.
    fn of(live: &Live) -> Option<&LiveRun<Self, Self::Own>>;

    /// What is live, if it is of this kind, to be changed.
    fn of_mut(live: &mut Live) -> Option<&mut LiveRun<Self, Self::Own>>;

    /// What was live, taken out, if it is of this kind.
    fn unboxed(live: Live) -> Option<LiveRun<Self, Self::Own>>;

    /// Where what it shows is published.
    fn shown(ctx: AppCtx) -> Signal<Option<Self::View>>;

    /// What it does on hearing `event` at `now`.
    #[must_use = "the effects are the machine's instructions to the runtime"]
    fn hear(&mut self, event: Self::Event, now: MonoMs) -> Vec<Self::Effect>;

    /// What tells it timer `id` has run out.
    fn timer_fired(id: u64) -> Self::Event;

    /// What its kind does with `event`, come under `lease`, before the
    /// machine hears it: `event` to hear now, or none to hear nothing now.
    fn before(_ctx: AppCtx, _lease: &Lease, event: Self::Event) -> Option<Self::Event> {
        Some(event)
    }

    /// What its machine hears at `now` before any event, as that event
    /// reaches it.
    fn first(live: &mut LiveRun<Self, Self::Own>, ctx: AppCtx, now: MonoMs) -> Vec<Self::Effect>;

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
/// let go: Ending, with the audio given back. It is marked over once it is
/// stored or let go.
pub(crate) fn end<M: RunMachine>(ctx: AppCtx) -> Option<LiveRun<M, M::Own>> {
    let taken = ctx
        .app()
        .session
        .borrow_mut()
        .take_if(|live| M::of(live).is_some())?;
    let mut live = M::unboxed(taken)?;
    // What the app holds lives Running or Paused, so is taken out once.
    live.stage = live.stage.ending()?;
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
