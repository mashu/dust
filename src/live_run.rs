//! The life of what is live, a session of groups or a contest run, written
//! once for both: what reaches it, each step of it, how it gives the audio
//! back as it pauses, and how it is taken out at its end. Its machine
//! decides what it does; the runtime holds where it is in its life, its
//! [`Stage`], and makes each move in one place here. Each kind's machine
//! is a [`RunMachine`], whose hooks its own runtime gives.

use std::borrow::Cow;

use cw_core::MonoMs;
use dioxus::prelude::*;

use crate::persist::SoFar;
use crate::player::Lease;
use crate::state::live::{LiveRun, Stage};
use crate::state::{AppCtx, Live};
use crate::time::{mono_ms, now_ms};

/// A machine that can be what is live: what the lifecycle cannot do the
/// same way for a session of groups and for a contest run.
///
/// [`step`] calls some of these with what is live borrowed from the app
/// (`progress`, `progressed`, `so_far`, `as_shown`, `hear`, and the step's
/// own `first`): they must leave the app alone, borrowing nothing of it,
/// doing nothing the machine said, handing nothing to it, and showing no
/// toast. `so_far` changes only its copy of the machine.
pub(crate) trait RunMachine: Sized {
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

    /// What its kind does as it is taken out, the audio given back.
    fn ending(_ctx: AppCtx, _live: &mut LiveRun<Self, Self::Own>) {}
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

/// One step of what is live of kind `M`, if what comes under `lease`
/// reaches it ([`hears`]): `first` hands its machine what comes before
/// `event` at this moment — typing still waiting, a gap the app was not
/// running in — or says it has nothing to hear; then the machine hears
/// `event`, if there is one. What it shows is published if that changed,
/// and what End would keep goes with its mark, should the app stop under
/// it, once it gets further, or once what it hears while paused changes
/// that. Back come what its machine said to do and the lease to do it
/// under. `first` runs with what is live borrowed ([`RunMachine`]).
pub(crate) fn step<M: RunMachine>(
    ctx: AppCtx,
    lease: &Lease,
    event: Option<M::Event>,
    first: impl FnOnce(&mut LiveRun<M, M::Own>, MonoMs) -> Option<Vec<M::Effect>>,
) -> Option<(Lease, Vec<M::Effect>)> {
    let mut shown = M::shown(ctx);
    let app = ctx.app();
    let (lease, effects, so_far) = {
        let mut slot = app.session.borrow_mut();
        let live = slot
            .as_mut()
            .and_then(M::of_mut)
            .filter(|live| hears(live, lease))?;
        let now = MonoMs(mono_ms());
        let finished_at = now_ms();
        let before = live.machine.progress();
        // Paused, having given the audio back, what it would keep before it
        // hears this: news of a send it hears now as held up takes the
        // times of what was heard in it.
        let paused = (event.is_some() && live.stage == Stage::Paused)
            .then(|| M::so_far(live, now, finished_at));
        let mut effects = first(live, now)?;
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
    Some((lease, effects))
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
