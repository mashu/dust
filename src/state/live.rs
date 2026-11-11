use cw_core::{ContestMachine, MonoMs, Sampler, SessionMachine, SplitMix64, TrainingSettings};

use crate::persist::OpenSession;
use crate::player::Lease;

/// A gap this long since the app was last seen running what is live is time
/// it was not running: a phone locked, a process put on hold, a webview that
/// held every task back. A session of groups is seen by its heartbeat and a
/// run by its screen's ticks; each comes every [`SEEN_EVERY_MS`], and a busy
/// page delays one by far less.
pub const AWAY_GAP_MS: u64 = 2_000;

/// How often what is live is seen running: a session's heartbeat, and a run
/// screen's tick, which draws the run's clock and the air again too.
pub const SEEN_EVERY_MS: u32 = 250;

/// What has the training screen: a session of groups, or a contest run.
/// Each is a machine of some size, and only one is ever live, so each is
/// boxed.
pub enum Live {
    Groups(Box<LiveSession>),
    Contest(Box<LiveContest>),
}

/// A session in progress: the machine that decides it, its hold on the
/// audio, and the stations it is sent by.
pub type LiveSession = LiveRun<SessionMachine<Sampler>, GroupsOwn>;

/// A run in progress: the machine that decides it, its hold on the audio,
/// and the settings its turns are played with.
pub type LiveContest = LiveRun<ContestMachine<Sampler>, RunOwn>;

/// What is live, of either kind: the machine that decides it, its hold on
/// the audio, and what only its kind holds.
pub struct LiveRun<M, O> {
    pub(crate) machine: M,
    pub(crate) lease: Lease,
    /// Where it is in its life: what reaches it, and how it may move on.
    pub(crate) stage: Stage,
    /// When the app was last seen running it, once it has been: a session
    /// from its start, seen by every heartbeat and by everything it hears;
    /// a run from its screen's first tick, seen by its ticks alone.
    pub(crate) seen: Option<MonoMs>,
    /// The local day it started: the day it counts towards, though it may
    /// end after midnight.
    pub(crate) started_on: String,
    /// How it is marked open, for what it has done to be added to.
    pub(crate) mark: OpenSession,
    pub(crate) own: O,
}

/// What only a session of groups holds.
pub struct GroupsOwn {
    /// Where every group's stations are drawn from. Drawn afresh for each
    /// session, so no two sessions — in one launch or across launches — hear
    /// the same operators.
    pub(crate) stations: SplitMix64,
}

/// What only a run holds.
pub struct RunOwn {
    /// The settings the run started with, fading off. Each caller brings a
    /// fade of its own, laid into its tones ([`plan_turn`](cw_core::plan_turn)), so no send has
    /// fading laid on as it plays, your message least of all; and the band
    /// has no fading in it, so it plays on, never rebuilt.
    pub(crate) sound: TrainingSettings,
    /// What the run has to say to a screen reader, held while anything is
    /// on the air so that speech never covers the Morse.
    pub(crate) unsaid: Vec<String>,
}

/// Where what is live is in its life, as the runtime holds it beside what
/// its machine decides: whether it holds the audio, and so what reaches it
/// ([`Stage::hears`]). It goes live Running, once its claim on the audio
/// is in; its machine's pause gives the audio back, and it is Paused;
/// Resume claims the audio again, and it is Running; End or Discard takes
/// it out, Ending, to be kept or let go. Each move is made in one place,
/// and refused from any other stage. A start and an end are each one
/// call, on the one thread, so nothing reaches it while it starts, or once
/// it has ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// It holds the audio: its lease is current.
    Running,
    /// Its machine has paused, and the audio is given back.
    Paused,
    /// Taken out, to be kept or let go.
    Ending,
}

impl Stage {
    /// Its machine paused, the audio given back: from Running only.
    pub(crate) fn gave_back(self) -> Option<Self> {
        (self == Self::Running).then_some(Self::Paused)
    }

    /// Resume claimed the audio afresh: from Paused only, as a claim while
    /// it runs would cut its send from under it.
    pub(crate) fn claimed(self) -> Option<Self> {
        (self == Self::Paused).then_some(Self::Running)
    }

    /// Taken out to be kept or let go: once, from either stage it lives in.
    pub(crate) fn ending(self) -> Option<Self> {
        (self != Self::Ending).then_some(Self::Ending)
    }

    /// Whether what comes under a lease reaches it: under one of its own
    /// (`own`) that still holds the audio (`current`); or, while it is
    /// paused, under any of its own, as what is done to it then — End,
    /// Discard, typing — and the news of the send its pause cut short
    /// still are. A timer never comes then: the lease it was set under
    /// drops it ([`Lease::after`]). Nothing reaches it once it is taken
    /// out.
    ///
    /// Running, it holds the audio: nothing else claims or gives back the
    /// audio while anything is live, so its own lease is the current one,
    /// and what the user does reaches it.
    pub(crate) fn hears(self, own: bool, current: bool) -> bool {
        own && match self {
            Self::Running => current,
            Self::Paused => true,
            Self::Ending => false,
        }
    }
}

impl<M, O> LiveRun<M, O> {
    /// When it was last seen, if that was longer than [`AWAY_GAP_MS`] before
    /// `now`: the start of a gap the app was not running in. It is seen now
    /// if `seen`, and whenever a gap is found.
    pub(crate) fn away(&mut self, now: MonoMs, seen: bool) -> Option<MonoMs> {
        let away = self.seen.filter(|last| now.since(*last) > AWAY_GAP_MS);
        if seen || away.is_some() {
            self.seen = Some(now);
        }
        away
    }
}

#[cfg(test)]
impl Live {
    /// Its hold on the audio, for a test to end or to stop.
    pub(crate) fn lease(&self) -> &Lease {
        match self {
            Self::Groups(live) => &live.lease,
            Self::Contest(live) => &live.lease,
        }
    }

    /// Where it is in its life.
    pub(crate) fn stage(&self) -> Stage {
        match self {
            Self::Groups(live) => live.stage,
            Self::Contest(live) => live.stage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::Recorder;
    use crate::player::Player;
    use crate::testing::{run, test_settings};

    /// Something live of no kind, last seen at `seen`.
    fn seen_at(seen: Option<MonoMs>) -> LiveRun<(), ()> {
        let lease = Player::new(Recorder::new().factory())
            .claim(&test_settings())
            .expect("a player");
        LiveRun {
            machine: (),
            lease,
            stage: Stage::Running,
            seen,
            started_on: String::new(),
            mark: OpenSession {
                started_at: 0,
                run: false,
                so_far: None,
                unstored: Vec::new(),
                over: false,
            },
            own: (),
        }
    }

    /// Each stage moves only as a live run's life allows: its machine's
    /// pause gives the audio back from Running only, and Resume's claim
    /// takes it again from Paused only; either is taken out once. What
    /// comes under a lease reaches it as its stage says.
    #[test]
    fn a_live_runs_stage_moves_only_as_its_life_allows() {
        use Stage::{Ending, Paused, Running};
        for (stage, gave_back, claimed, ending) in [
            (Running, Some(Paused), None, Some(Ending)),
            (Paused, None, Some(Running), Some(Ending)),
            (Ending, None, None, None),
        ] {
            assert_eq!(
                stage.gave_back(),
                gave_back,
                "{stage:?}, the audio given back"
            );
            assert_eq!(stage.claimed(), claimed, "{stage:?}, the audio claimed");
            assert_eq!(stage.ending(), ending, "{stage:?}, taken out");
        }
        for (stage, own, current, hears) in [
            (Running, true, true, true),
            (Running, true, false, false),
            (Running, false, true, false),
            (Running, false, false, false),
            (Paused, true, true, true),
            (Paused, true, false, true),
            (Paused, false, true, false),
            (Paused, false, false, false),
            (Ending, true, true, false),
            (Ending, true, false, false),
            (Ending, false, true, false),
            (Ending, false, false, false),
        ] {
            assert_eq!(
                stage.hears(own, current),
                hears,
                "{stage:?}, under a lease of its own {own}, holding the audio {current}"
            );
        }
    }

    /// A gap is time since last seen past [`AWAY_GAP_MS`], found from when
    /// it was last seen: finding one, or being told it is seen, moves when
    /// it was last seen to now. Never seen, nothing is a gap.
    #[test]
    fn a_gap_is_found_by_one_rule() {
        run(|| async {
            let mut live = seen_at(None);
            let at = Some(10_000);
            for (seen, now, told, away, after) in [
                // Never seen: no gap, however long; seen only when told.
                (None, 99_000, false, None, None),
                (None, 99_000, true, None, Some(99_000)),
                // Up to the gap's length is no gap.
                (at, 11_999, false, None, at),
                (at, 12_000, false, None, at),
                (at, 12_000, true, None, Some(12_000)),
                // Past it is, from when it was last seen, and it is seen now.
                (at, 12_001, false, at, Some(12_001)),
                (at, 12_001, true, at, Some(12_001)),
                // A clock that has not moved on finds no gap.
                (at, 9_000, false, None, at),
                (at, 9_000, true, None, Some(9_000)),
            ] {
                live.seen = seen.map(MonoMs);
                let found = live.away(MonoMs(now), told);
                assert_eq!(
                    (found, live.seen),
                    (away.map(MonoMs), after.map(MonoMs)),
                    "seen {seen:?}, now {now}, told it is seen: {told}"
                );
            }
        });
    }
}
