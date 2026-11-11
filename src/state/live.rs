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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::Recorder;
    use crate::player::Player;
    use crate::testing::{run, test_settings};
    use cw_core::Rng;

    /// Something live of no kind, last seen at `seen`.
    fn seen_at(seen: Option<MonoMs>) -> LiveRun<(), ()> {
        let lease = Player::new(Recorder::new().factory())
            .claim(&test_settings())
            .expect("a player");
        LiveRun {
            machine: (),
            lease,
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

    /// The session's rule, as it was: when it was last seen goes to now
    /// first, and the gap is found from when it was before.
    fn session_rule(last_seen: &mut MonoMs, now: MonoMs) -> Option<MonoMs> {
        let seen = std::mem::replace(last_seen, now);
        (now.since(seen) > AWAY_GAP_MS).then_some(seen)
    }

    /// When the run screen last ticked, once it has, if it has not since
    /// for longer than a tick can take: the run's `away_since`, as it was.
    fn away_since(last_tick: Option<MonoMs>, now: MonoMs) -> Option<MonoMs> {
        last_tick.filter(|tick| now.since(*tick) > AWAY_GAP_MS)
    }

    /// The run's tick, as it was: a gap found pauses the run, which sees it
    /// then, and the tick sees it now in any case.
    fn tick_rule(last_tick: &mut Option<MonoMs>, now: MonoMs) -> Option<MonoMs> {
        let away = away_since(*last_tick, now);
        if away.is_some() {
            *last_tick = Some(now);
        }
        *last_tick = Some(now);
        away
    }

    /// The run hearing anything else, as it was: a gap found pauses it, and
    /// only then is it seen.
    fn event_rule(last_tick: &mut Option<MonoMs>, now: MonoMs) -> Option<MonoMs> {
        let away = away_since(*last_tick, now);
        if away.is_some() {
            *last_tick = Some(now);
        }
        away
    }

    /// The one rule finds the gap each of the three it replaces found, and
    /// leaves when it was last seen where each left it: the session's, on
    /// every beat and everything it hears, told it is seen; the run's on a
    /// tick, told it is seen; and the run's on anything else, not told.
    #[test]
    fn the_one_away_rule_does_what_the_three_it_replaces_did() {
        type Rule = fn(&mut Option<MonoMs>, MonoMs) -> Option<MonoMs>;
        run(|| async {
            let mut live = seen_at(None);
            let mut rng = cw_core::SplitMix64::new(0x00a1_7a9e);
            for _ in 0..1_000_000 {
                let last = MonoMs(rng.usize_in(0, 1_000_000) as u64);
                let gap = match rng.usize_in(0, 5) {
                    0 => 1_999,
                    1 => 2_000,
                    2 => 2_001,
                    3 => rng.usize_in(0, 4_000) as u64,
                    _ => rng.usize_in(0, 120_000) as u64,
                };
                // Now and then a clock behind when it was last seen.
                let now = if rng.f64() < 0.05 {
                    MonoMs(last.0.saturating_sub(gap))
                } else {
                    last.plus(gap)
                };
                let seen = (rng.f64() < 0.9).then_some(last);
                if let Some(last) = seen {
                    let mut was = last;
                    let found = session_rule(&mut was, now);
                    live.seen = seen;
                    assert_eq!(
                        live.away(now, true),
                        found,
                        "a session, {last:?} to {now:?}"
                    );
                    assert_eq!(live.seen, Some(was), "a session, {last:?} to {now:?}");
                }
                let rules: [(bool, Rule); 2] = [(true, tick_rule), (false, event_rule)];
                for (told, rule) in rules {
                    let mut was = seen;
                    let found = rule(&mut was, now);
                    live.seen = seen;
                    assert_eq!(live.away(now, told), found, "a run, {seen:?} to {now:?}");
                    assert_eq!(live.seen, was, "a run, {seen:?} to {now:?}, told {told}");
                }
            }
        });
    }
}
