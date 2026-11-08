use cw_core::{ContestMachine, MonoMs, Sampler, SessionMachine, SplitMix64, TrainingSettings};

use crate::persist::OpenSession;
use crate::player::Lease;

/// A gap this long since the app was last seen running what is live is time
/// it was not running: a phone locked, a process put on hold, a webview that
/// held every task back. A session of groups is seen by its heartbeat and a
/// run by its screen's ticks; each comes every quarter second, and a busy
/// page delays one by far less.
pub const AWAY_GAP_MS: u64 = 2_000;

/// What has the training screen: a session of groups, or a contest run.
/// Each is a machine of some size, and only one is ever live, so each is
/// boxed.
pub enum Live {
    Groups(Box<LiveSession>),
    Contest(Box<LiveContest>),
}

/// A session in progress: the machine that decides it, the stations it is
/// sent by, and its hold on the audio.
pub struct LiveSession {
    pub(crate) machine: SessionMachine<Sampler>,
    /// Where every group's stations are drawn from. Drawn afresh for each
    /// session, so no two sessions — in one launch or across launches — hear
    /// the same operators.
    pub(crate) stations: SplitMix64,
    pub(crate) lease: Lease,
    /// When the app was last seen running the session: its last heartbeat,
    /// or the last news it heard.
    pub(crate) last_seen: MonoMs,
    /// The local day it started: the day it counts towards, though it may
    /// end after midnight.
    pub(crate) started_on: String,
    /// How it is marked open, for what it has done to be added to.
    pub(crate) mark: OpenSession,
}

/// A run in progress: the machine that decides it, the settings its turns
/// are played with, and its hold on the audio.
pub struct LiveContest {
    pub(crate) machine: ContestMachine<Sampler>,
    /// The settings the run started with, fading off. Each caller brings a
    /// fade of its own, laid into its tones ([`plan_turn`](cw_core::plan_turn)), so no send has
    /// fading laid on as it plays, your message least of all; and the band
    /// has no fading in it, so it plays on, never rebuilt.
    pub(crate) sound: TrainingSettings,
    pub(crate) lease: Lease,
    /// When the run screen's clock last ticked, once it has.
    pub(crate) last_tick: Option<MonoMs>,
    /// What the run has to say to a screen reader, held while anything is
    /// on the air so that speech never covers the Morse.
    pub(crate) unsaid: Vec<String>,
    /// The local day it started: the day it counts towards, though it may
    /// end after midnight.
    pub(crate) started_on: String,
    /// How it is marked open, for what it has done to be added to.
    pub(crate) mark: OpenSession,
}
