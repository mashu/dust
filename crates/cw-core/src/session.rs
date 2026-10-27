//! One training session: its groups, what was answered, and the result it
//! scores to. [`crate::machine::SessionMachine`] is the only thing that moves
//! it on; everyone else reads it.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::alignment::{CopyTally, LetterAccuracy};
use crate::contest::{CONTEST_MARK, ContestLog};
use crate::level::LEVEL_MIN;
use crate::recognition::{CharTimes, answer_chars, lenient_char_times};
use crate::score::{
    GroupCopy, MIN_RESPONSE_MS_PER_CHAR, calculate_effective_alphabet_size, calculate_total_chars,
    compute_average_response_ms, copy_speed, session_points,
};
use crate::settings::{CharSetMode, LevelAxis, TrainingSettings, mode_on_disk};

/// The longest an answer is kept, against the group it answers: twice its
/// length, or this many characters for a short one. Room for every honest
/// mistake, and none for a paste of a novel to be stored, scored and aligned
/// on every visit to the stats screen.
const LONGEST_ANSWER: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl SessionId {
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A moment on a clock that only runs forward, in milliseconds from wherever
/// that clock starts.
///
/// Response times are measured between two of these, so they must come from
/// a monotonic clock: the wall clock steps when it is corrected, and a step
/// back between a send ending and its answer used to score the answer as
/// instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonoMs(pub u64);

impl MonoMs {
    /// Milliseconds since `earlier`; none if it is not earlier.
    pub fn since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }

    #[must_use]
    pub fn plus(self, ms: u64) -> Self {
        Self(self.0.saturating_add(ms))
    }

    /// `seconds` later, to the nearest millisecond: how far off a player
    /// says a send is heard, where a segment of a turn starts. A negative
    /// span moves it nowhere.
    #[must_use]
    pub fn plus_sec(self, seconds: f64) -> Self {
        self.plus(whole_ms(seconds))
    }
}

/// Seconds as whole milliseconds; none for a negative span.
pub(crate) fn whole_ms(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1_000.0).round() as u64
}

/// A send as it starts: the speeds it goes at, and when its characters go to
/// the audio output, by the player's own reckoning.
#[derive(Clone, Debug, PartialEq)]
pub struct SendStart {
    pub char_wpm: f64,
    pub effective_wpm: f64,
    /// When its first sample goes to the output.
    pub output_at: MonoMs,
    /// Where each of its characters' last element ends, in milliseconds
    /// after `output_at`.
    pub char_ends_ms: Vec<u32>,
    /// The delay the device reports from its output to the ear.
    pub latency_ms: u32,
}

impl SendStart {
    /// When its sound reaches the ear, as far as the device says.
    pub fn heard_at(&self) -> MonoMs {
        self.output_at.plus(u64::from(self.latency_ms))
    }
}

/// How a group's first send went to the output: what its characters are
/// timed from.
#[derive(Clone, Debug, PartialEq)]
struct Sounding {
    origin: MonoMs,
    char_ends_ms: Vec<u32>,
    latency_ms: u32,
    /// It can be timed from: it played on time, and was not tried again.
    trusted: bool,
    /// When a later send of the group began to go out: a key from then on
    /// came after hearing it twice.
    resent: Option<MonoMs>,
}

/// Where a session is. Every phase that is about a group names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    /// Counting down to the first group, `remaining` seconds to go. Nothing
    /// is sent and nothing can be typed yet.
    Countdown {
        remaining: u32,
    },
    Playing {
        index: usize,
    },
    /// Word-space pause between two sends of the same group.
    RepeatGap {
        index: usize,
    },
    InterGroupGap {
        next: usize,
    },
    AwaitingAnswer {
        index: usize,
    },
    Finished,
    Aborted,
}

/// A group just answered, as its source learns from it: what was sent and
/// typed, what was copied of it, and the character and effective speed it
/// was played at, once measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Answered<'a> {
    pub sent: &'a str,
    pub received: &'a str,
    pub copied: &'a CopyTally,
    pub played: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    sent: String,
    input: String,
    /// What was copied, once the group is answered. Answered is exactly
    /// when there is one.
    tally: Option<CopyTally>,
    start_at: MonoMs,
    /// When the last send of the group ended; none while it is still going.
    end_at: Option<MonoMs>,
    /// When the answer was complete: the moment its length first matched,
    /// or the confirm.
    answer_at: Option<MonoMs>,
    /// The character and effective speed it was sent at, once its first
    /// send starts.
    played_wpm: Option<(f64, f64)>,
    /// How its first send went to the output, once it starts.
    sounding: Option<Sounding>,
    /// When each position of the answer first held its character, as the
    /// box keyed it; none where it cannot be timed. As long as the answer.
    keys: Vec<Option<MonoMs>>,
    /// The character and effective speed the source chose for it, when it
    /// chooses one: adaptive speed does.
    planned_wpm: Option<(f64, f64)>,
    /// How many times this group is sent before the answer window opens.
    repeats: u32,
    /// Sends already finished for this group.
    plays_done: u32,
}

impl Group {
    fn blank() -> Self {
        Self {
            sent: String::new(),
            input: String::new(),
            tally: None,
            start_at: MonoMs::default(),
            end_at: None,
            answer_at: None,
            played_wpm: None,
            sounding: None,
            keys: Vec::new(),
            planned_wpm: None,
            repeats: 1,
            plays_done: 0,
        }
    }

    pub fn sent(&self) -> &str {
        &self.sent
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn confirmed(&self) -> bool {
        self.tally.is_some()
    }

    /// The group as it was answered, once it is.
    pub fn answered(&self) -> Option<Answered<'_>> {
        Some(Answered {
            sent: &self.sent,
            received: &self.input,
            copied: self.tally.as_ref()?,
            played: self.played_wpm,
        })
    }

    /// How long the answer took, from the end of the send — never quicker
    /// than a person types — and what its characters' recognition is read
    /// from ([`CharTimes`]), where its first send can be timed. `box_open`
    /// is whether the box took keys while the group was sent.
    fn timing(&self, box_open: bool) -> SessionTiming {
        let chars = self.sent.chars().count().max(1) as f64;
        let measured = match (self.end_at, self.answer_at) {
            (Some(end), Some(answer)) => answer.since(end) as f64,
            // Answered before the send ended: as fast as it gets.
            _ => 0.0,
        };
        SessionTiming {
            time_to_complete_ms: measured.max(MIN_RESPONSE_MS_PER_CHAR * chars),
            char_wpm: self.played_wpm.map(|(char_wpm, _)| char_wpm),
            effective_wpm: self.played_wpm.map(|(_, effective_wpm)| effective_wpm),
            // A group answered before its last send was heard was heard
            // fewer times than planned, and is scored for what it was.
            sends: self.plays_done.clamp(1, self.repeats.max(1)),
            char_times: self
                .sounding
                .as_ref()
                .filter(|sounding| sounding.trusted)
                .map(|sounding| {
                    CharTimes::of(
                        sounding.origin.0,
                        &sounding.char_ends_ms,
                        self.keys.iter().map(|key| key.map(|key| key.0)),
                        sounding.resent.map(|resent| resent.0),
                        sounding.latency_ms,
                        box_open,
                    )
                }),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionView {
    pub current: usize,
    pub focused: usize,
    pub sent: Vec<String>,
    pub inputs: Vec<String>,
    pub confirmed: Vec<bool>,
    /// The current group is being sent, or is between two of its sends.
    pub playing: bool,
    pub locked: bool,
    /// Seconds left before the first group is sent, while the session
    /// counts down to it.
    pub countdown: Option<u32>,
    /// Planned sends for the group being played, and how many are done.
    pub repeat_total: u32,
    pub repeat_done: u32,
    /// The speed the session chose for the current group, when it chooses
    /// one: adaptive speed does.
    pub speed: Option<ChosenSpeed>,
}

/// The speed the session chose for a group, as the training screen shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChosenSpeed {
    pub char_wpm: f64,
    pub effective_wpm: f64,
    /// Faster than the group before it (1), slower (-1), or about the same
    /// (0): a change under two percent is the same.
    pub change: i8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GroupSession {
    session_id: SessionId,
    phase: SessionPhase,
    groups: Vec<Group>,
    current_group: usize,
    focused_group: usize,
    settings: TrainingSettings,
}

impl GroupSession {
    pub(crate) fn new(
        session_id: SessionId,
        num_groups: usize,
        settings: TrainingSettings,
    ) -> Self {
        Self {
            session_id,
            phase: SessionPhase::Playing { index: 0 },
            groups: vec![Group::blank(); num_groups.max(1)],
            current_group: 0,
            focused_group: 0,
            settings,
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    pub fn current_group(&self) -> usize {
        self.current_group
    }

    /// The settings the session was started with. Every group is built from
    /// these, not from whatever the settings screen holds now.
    pub fn settings(&self) -> &TrainingSettings {
        &self.settings
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn group(&self, index: usize) -> Option<&Group> {
        self.groups.get(index)
    }

    pub fn any_confirmed(&self) -> bool {
        self.groups.iter().any(Group::confirmed)
    }

    /// Whether typing into `index` is refused: while its group is being sent,
    /// if the settings lock the box for that, and before anything has been
    /// sent at all.
    pub fn input_locked(&self, index: usize) -> bool {
        let sending = self.settings.playback.lock_input_during_group_playback && self.playing();
        (sending || self.countdown().is_some()) && index == self.current_group
    }

    /// Seconds left before the first group, while the session counts down.
    pub fn countdown(&self) -> Option<u32> {
        match self.phase {
            SessionPhase::Countdown { remaining } => Some(remaining),
            _ => None,
        }
    }

    fn playing(&self) -> bool {
        matches!(
            self.phase,
            SessionPhase::Playing { .. } | SessionPhase::RepeatGap { .. }
        )
    }

    pub fn view(&self) -> SessionView {
        let current = self.groups.get(self.current_group);
        SessionView {
            current: self.current_group,
            focused: self.focused_group,
            sent: self.groups.iter().map(|g| g.sent.clone()).collect(),
            inputs: self.groups.iter().map(|g| g.input.clone()).collect(),
            confirmed: self.groups.iter().map(Group::confirmed).collect(),
            playing: self.playing(),
            locked: self.input_locked(self.current_group),
            countdown: self.countdown(),
            repeat_total: current.map_or(1, |g| g.repeats.max(1)),
            repeat_done: current.map_or(0, |g| g.plays_done),
            speed: self.chosen_speed(self.current_group),
        }
    }

    /// The speed chosen for group `index`, and how it moved from the one
    /// before it.
    fn chosen_speed(&self, index: usize) -> Option<ChosenSpeed> {
        let (char_wpm, effective_wpm) = self.planned_wpm(index)?;
        let copy = |(c, e): (f64, f64)| crate::score::copy_speed(c, e);
        let change = index
            .checked_sub(1)
            .and_then(|before| self.planned_wpm(before))
            .map_or(0, |before| {
                let ratio = copy((char_wpm, effective_wpm)) / copy(before);
                if ratio > 1.02 {
                    1
                } else if ratio < 0.98 {
                    -1
                } else {
                    0
                }
            });
        Some(ChosenSpeed {
            char_wpm,
            effective_wpm,
            change,
        })
    }

    pub(crate) fn set_phase(&mut self, phase: SessionPhase) {
        self.phase = phase;
    }

    pub(crate) fn group_repeats(&self, index: usize) -> u32 {
        self.groups.get(index).map_or(1, |g| g.repeats.max(1))
    }

    pub(crate) fn played_wpm(&self, index: usize) -> Option<(f64, f64)> {
        self.groups.get(index)?.played_wpm
    }

    pub(crate) fn group_start(&self, index: usize) -> Option<MonoMs> {
        self.groups.get(index).map(|g| g.start_at)
    }

    /// Fill in what `index` sends, and how many times.
    pub(crate) fn set_group(&mut self, index: usize, text: String, repeats: u32) {
        if let Some(group) = self.groups.get_mut(index)
            && !group.confirmed()
        {
            group.sent = text;
            group.input.clear();
            group.repeats = repeats.max(1);
        }
    }

    /// The speed the source chose for group `index`, if it chose one.
    pub(crate) fn set_group_speed(&mut self, index: usize, speed: Option<(f64, f64)>) {
        if let Some(group) = self.groups.get_mut(index)
            && !group.confirmed()
        {
            group.planned_wpm = speed;
        }
    }

    /// The character and effective speed to send group `index` at, when the
    /// session chose them; otherwise the station sending it decides.
    pub fn planned_wpm(&self, index: usize) -> Option<(f64, f64)> {
        self.groups.get(index)?.planned_wpm
    }

    pub(crate) fn begin_group(&mut self, index: usize, now: MonoMs) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        group.start_at = now;
        group.plays_done = 0;
        self.current_group = index;
        self.focused_group = index;
    }

    /// A send of `index` started, as `start` says: its speeds, and for its
    /// first send, what its characters are timed from. The first send tried
    /// again cannot be timed: it was heard in part already. A later send
    /// marks where keys stop being timed.
    pub(crate) fn start_playback(&mut self, index: usize, start: &SendStart) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        if start.char_wpm > 0.0 && start.effective_wpm > 0.0 {
            group.played_wpm = Some((start.char_wpm, start.effective_wpm));
        }
        match (&mut group.sounding, group.plays_done) {
            (None, 0) => {
                group.sounding = Some(Sounding {
                    origin: start.output_at,
                    char_ends_ms: start.char_ends_ms.clone(),
                    latency_ms: start.latency_ms,
                    trusted: true,
                    resent: None,
                });
            }
            (Some(sounding), 0) => sounding.trusted = false,
            (Some(sounding), _) => {
                sounding.resent.get_or_insert(start.output_at);
            }
            (None, _) => {}
        }
    }

    /// Count one finished send of `index`, and whether it played on time:
    /// a first send held up on its way cannot be timed.
    pub(crate) fn note_play_finished(&mut self, index: usize, on_time: bool) {
        if let Some(group) = self.groups.get_mut(index) {
            if !on_time
                && group.plays_done == 0
                && let Some(sounding) = &mut group.sounding
            {
                sounding.trusted = false;
            }
            group.plays_done = group.plays_done.saturating_add(1);
        }
    }

    /// The last send of `index` was seen to end at `now`.
    pub(crate) fn end_playback(&mut self, index: usize, now: MonoMs) {
        if let Some(group) = self.groups.get_mut(index) {
            group.end_at = Some(now);
        }
    }

    /// Take what is typed into the current group, tidied ([`normalize_answer`]),
    /// with when each of its characters went in.
    pub(crate) fn set_input(&mut self, index: usize, typed: &str, keys: &[Option<MonoMs>]) {
        if index != self.current_group {
            return;
        }
        if let Some(group) = self.groups.get_mut(index)
            && !group.confirmed()
        {
            group.input = normalize_answer(&group.sent, typed);
            let length = group.input.chars().count();
            group.keys.clear();
            group.keys.extend(keys.iter().take(length));
            group.keys.resize(length, None);
        }
    }

    /// Stamp the moment the answer reached the group's length, the first
    /// time it does; withdraw the stamp when it no longer does.
    pub(crate) fn mark_answer_complete(&mut self, index: usize, complete: bool, now: MonoMs) {
        if let Some(group) = self.groups.get_mut(index)
            && !group.confirmed()
        {
            group.answer_at = if complete {
                group.answer_at.or(Some(now))
            } else {
                None
            };
        }
    }

    /// Answer the current group with what is in its box. Returns the group
    /// as answered, the first time only.
    pub(crate) fn confirm(&mut self, index: usize, now: MonoMs) -> Option<Answered<'_>> {
        if index != self.current_group {
            return None;
        }
        let last = self.groups.len().saturating_sub(1);
        let group = self.groups.get_mut(index)?;
        if group.confirmed() {
            return None;
        }
        group.answer_at = group.answer_at.or(Some(now));
        self.focused_group = (index + 1).min(last);
        group.tally = Some(CopyTally::of(&group.sent, &group.input));
        group.answered()
    }

    /// The finished session, scored, as it is stored. `finished_at` is wall
    /// time, the moment it ended; `date` the local day it counts towards.
    pub fn result(&self, finished_at: u64, date: String) -> SessionResult {
        let scored: Vec<&Group> = self
            .groups
            .iter()
            .filter(|group| group.confirmed() && !group.sent.is_empty())
            .collect();
        let mut tally = CopyTally::default();
        for copied in scored.iter().filter_map(|group| group.tally.as_ref()) {
            tally.merge(copied);
        }
        let accuracy = tally.accuracy();
        let sent: Vec<&str> = scored.iter().map(|g| g.sent.as_str()).collect();
        let box_open = !self.settings.playback.lock_input_during_group_playback;
        let timings: Vec<SessionTiming> = scored.iter().map(|g| g.timing(box_open)).collect();
        let avg_response_ms = compute_average_response_ms(
            scored
                .iter()
                .zip(&timings)
                .map(|(group, timing)| timing.ms_per_char(&group.sent)),
        );
        let total_chars = calculate_total_chars(&sent);
        let played: Vec<(f64, f64)> = scored.iter().filter_map(|g| g.played_wpm).collect();
        let playback = &self.settings.playback;
        let (char_wpm, effective_wpm) = if played.is_empty() {
            (
                playback.char_wpm_min,
                playback.effective_wpm_min.min(playback.char_wpm_min),
            )
        } else {
            let n = played.len() as f64;
            (
                played.iter().map(|(c, _)| c).sum::<f64>() / n,
                played.iter().map(|(_, e)| e).sum::<f64>() / n,
            )
        };

        // A contest run is recorded by its own machine, with its log, never
        // as groups.
        let mode = self.settings.curriculum.char_set_mode;
        debug_assert_ne!(
            mode,
            CharSetMode::Contest,
            "a contest run recorded as a group session"
        );
        let mut result = SessionResult {
            date,
            timestamp: finished_at,
            groups: scored
                .iter()
                .map(|group| GroupResult {
                    sent: group.sent.clone(),
                    received: group.input.clone(),
                    correct: group.sent == group.input,
                })
                .collect(),
            group_timings: timings,
            accuracy,
            tally,
            avg_response_ms,
            total_chars,
            score: 0.0,
            // One level per mode, and `level` is the one this mode was run at —
            // callsigns need no second field the way Mixed needs `digits_level`.
            level: self.settings.active_level(),
            digits_level: self.settings.level(LevelAxis::Digits),
            char_set_mode: recorded_mode(mode),
            char_wpm,
            effective_wpm,
            alphabet_fingerprint: self.settings.alphabet_fingerprint(),
            contest: None,
        };
        result.score = result.points();
        result
    }
}

/// The mode a group session run in `mode` is recorded as: its own, except
/// that a contest run, which only its own machine records, is filed as the
/// callsign practice it was, not as a run with no log to show.
fn recorded_mode(mode: CharSetMode) -> CharSetMode {
    match mode {
        CharSetMode::Contest => CharSetMode::Callsign,
        mode => mode,
    }
}

/// An answer as it is kept: upper case, with no whitespace — a group never
/// has any, so a stray space is neither an error nor a reason the length
/// does not match — and no longer than [`LONGEST_ANSWER`] allows.
fn normalize_answer(sent: &str, typed: &str) -> String {
    let longest = (2 * sent.chars().count()).max(LONGEST_ANSWER);
    answer_chars(typed).take(longest).collect()
}

/// Whether `received` is as long as `sent`, not counting whitespace: when an
/// answer is complete enough to confirm by itself.
pub fn answer_length_matches(sent: &str, received: &str) -> bool {
    !sent.is_empty() && answer_chars(received).count() == sent.chars().count()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupResult {
    pub sent: String,
    pub received: String,
    pub correct: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTiming {
    /// From the end of the send to the answer, never quicker than
    /// [`MIN_RESPONSE_MS_PER_CHAR`] a character.
    pub time_to_complete_ms: f64,
    #[serde(default)]
    pub char_wpm: Option<f64>,
    /// The pace the spacing set. Older saves have none, and are taken to
    /// have been sent with the session's spacing.
    #[serde(default)]
    pub effective_wpm: Option<f64>,
    /// How many times the group was sent before it was answered. Older saves
    /// sent each group once.
    #[serde(default = "one_send")]
    pub sends: u32,
    /// What its characters' recognition times are read from
    /// ([`crate::recognise`]): when each ended and when each was keyed.
    /// None for older saves, and where its sending cannot be timed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_char_times"
    )]
    pub char_times: Option<CharTimes>,
}

impl Default for SessionTiming {
    /// No answer time, no speed recorded, sent once, untimed: a timing that
    /// sets only what it is about.
    fn default() -> Self {
        Self {
            time_to_complete_ms: 0.0,
            char_wpm: None,
            effective_wpm: None,
            sends: 1,
            char_times: None,
        }
    }
}

impl SessionTiming {
    /// The answer's time a character of `sent`: what the score charges and
    /// older builds show as the average time.
    pub fn ms_per_char(&self, sent: &str) -> f64 {
        self.time_to_complete_ms / sent.chars().count().max(1) as f64
    }
}

fn one_send() -> u32 {
    1
}

fn session_level_default() -> u32 {
    LEVEL_MIN
}

fn session_mode_legacy() -> CharSetMode {
    CharSetMode::Koch
}

/// Sessions saved before the speed was recorded were sent at the default
/// then, 18 WPM: fixed, so that a later default never changes your history.
fn session_wpm_default() -> f64 {
    18.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "StoredSession")]
pub struct SessionResult {
    pub date: String,
    /// Wall time the session finished, in milliseconds: what history is
    /// ordered by.
    pub timestamp: u64,
    pub groups: Vec<GroupResult>,
    pub group_timings: Vec<SessionTiming>,
    pub accuracy: f64,
    /// What was copied, character by character.
    #[serde(flatten)]
    pub tally: CopyTally,
    pub avg_response_ms: f64,
    pub total_chars: u32,
    pub score: f64,
    pub level: u32,
    pub digits_level: u32,
    /// Contest is written "callsign", so a build that knows no contests
    /// still reads the session.
    #[serde(serialize_with = "mode_on_disk")]
    pub char_set_mode: CharSetMode,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    /// Full progress alphabet at the time of the session, in its unlock
    /// order. Empty on legacy saves. A contest run's starts with
    /// [`CONTEST_MARK`] and names the contest: it outlives a lost log.
    pub alphabet_fingerprint: String,
    /// A contest run's log, as it was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contest: Option<ContestLog>,
}

/// A session as it is read: the keys this version knows, with what older
/// saves lack filled in. Keys it does not read are dropped, not carried
/// along: a catch-all would keep retired keys alive and could write a key
/// twice. The contest log is the one part kept that this version does not
/// read, and it is kept by name.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    date: String,
    timestamp: u64,
    groups: Vec<GroupResult>,
    group_timings: Vec<SessionTiming>,
    accuracy: f64,
    #[serde(flatten)]
    tally: CopyTally,
    avg_response_ms: f64,
    total_chars: u32,
    score: f64,
    /// Saves from the first versions called it the Koch level.
    #[serde(alias = "kochLevel", default = "session_level_default")]
    level: u32,
    #[serde(default = "session_level_default")]
    digits_level: u32,
    #[serde(default = "session_mode_legacy")]
    char_set_mode: CharSetMode,
    #[serde(default = "session_wpm_default")]
    char_wpm: f64,
    #[serde(default = "session_wpm_default")]
    effective_wpm: f64,
    #[serde(default)]
    alphabet_fingerprint: String,
    #[serde(default)]
    contest: Option<ContestLog>,
}

impl From<StoredSession> for SessionResult {
    /// A contest run is known by its log, or by its mark once a build that
    /// knew no contests has saved it without the log.
    fn from(stored: StoredSession) -> Self {
        let run = stored.contest.is_some() || stored.alphabet_fingerprint.starts_with(CONTEST_MARK);
        Self {
            date: stored.date,
            timestamp: stored.timestamp,
            groups: stored.groups,
            group_timings: stored.group_timings,
            accuracy: stored.accuracy,
            tally: stored.tally,
            avg_response_ms: stored.avg_response_ms,
            total_chars: stored.total_chars,
            score: stored.score,
            level: stored.level,
            digits_level: stored.digits_level,
            char_set_mode: if run {
                CharSetMode::Contest
            } else {
                stored.char_set_mode
            },
            char_wpm: stored.char_wpm,
            effective_wpm: stored.effective_wpm,
            alphabet_fingerprint: stored.alphabet_fingerprint,
            contest: stored.contest,
        }
    }
}

impl SessionResult {
    /// What the session earns ([`crate::score`]): every group's points,
    /// added up. Worked out from what is stored, so a session saved before
    /// speed counted can be scored the same way as one finished today.
    pub fn points(&self) -> f64 {
        let sent: Vec<&str> = self.groups.iter().map(|g| g.sent.as_str()).collect();
        let copies: Vec<GroupCopy> = self
            .groups
            .iter()
            .zip(&self.group_timings)
            .map(|(group, timing)| {
                let char_wpm = timing.char_wpm.unwrap_or(self.char_wpm);
                // Older saves have the session's spacing only: the group's
                // own character speed, spaced as the session was.
                let effective_wpm = timing.effective_wpm.unwrap_or_else(|| {
                    if self.char_wpm > 0.0 {
                        self.effective_wpm * char_wpm / self.char_wpm
                    } else {
                        self.effective_wpm
                    }
                });
                GroupCopy::of(
                    &group.sent,
                    &group.received,
                    copy_speed(char_wpm, effective_wpm),
                    timing.ms_per_char(&group.sent),
                    timing.sends,
                )
            })
            .collect();
        session_points(&copies, calculate_effective_alphabet_size(&sent))
    }

    /// How each character sent was copied. Sessions saved before that was
    /// recorded have it worked out again from their groups.
    pub fn letter_accuracy(&self) -> Cow<'_, BTreeMap<char, LetterAccuracy>> {
        if !self.tally.sent.is_empty() || self.groups.is_empty() {
            return Cow::Borrowed(&self.tally.sent);
        }
        let mut tally = CopyTally::default();
        for group in &self.groups {
            tally.add(&group.sent, &group.received);
        }
        Cow::Owned(tally.sent)
    }

    /// A Koch session with nothing in it, for tests to fill in: the app's
    /// as well as this crate's, so a field added here is added to them all
    /// in one place.
    pub fn empty_for_test() -> Self {
        Self {
            date: "2026-09-01".into(),
            timestamp: 0,
            groups: Vec::new(),
            group_timings: Vec::new(),
            accuracy: 0.0,
            tally: CopyTally::default(),
            avg_response_ms: 0.0,
            total_chars: 1,
            score: 0.0,
            level: 1,
            digits_level: 1,
            char_set_mode: CharSetMode::Koch,
            char_wpm: 18.0,
            effective_wpm: 18.0,
            alphabet_fingerprint: String::new(),
            contest: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session of `copies`: each group sent, its send ending half a second
    /// in at 18 WPM, then answered as typed `ms` later.
    fn answered(settings: TrainingSettings, copies: &[(&str, &str, u64)]) -> GroupSession {
        let mut session = GroupSession::new(SessionId::new(1), copies.len(), settings);
        let mut now = MonoMs(1_000);
        for (index, (sent, typed, ms)) in copies.iter().enumerate() {
            session.set_group(index, (*sent).into(), 1);
            session.begin_group(index, now);
            now = now.plus(500);
            session.start_playback(index, &speeds(18.0, 18.0));
            session.end_playback(index, now);
            session.set_input(index, typed, &[]);
            now = now.plus(*ms);
            session.confirm(index, now);
        }
        session
    }

    fn result(session: &GroupSession) -> SessionResult {
        session.result(3_000, "2026-09-01".into())
    }

    #[test]
    fn a_span_in_seconds_moves_a_moment_to_the_millisecond() {
        assert_eq!(MonoMs(1_000).plus_sec(0.3), MonoMs(1_300));
        assert_eq!(MonoMs(1_000).plus_sec(0.000_4), MonoMs(1_000));
        assert_eq!(MonoMs(1_000).plus_sec(-2.0), MonoMs(1_000));
        assert_eq!(MonoMs(1_000).plus_sec(f64::NAN), MonoMs(1_000));
    }

    #[test]
    fn perfect_copy_is_full_accuracy() {
        let session = answered(
            TrainingSettings::default(),
            &[("KM", "KM", 400), ("UK", "uk", 400)],
        );
        let result = result(&session);
        assert!((result.accuracy - 1.0).abs() < 1e-9);
        assert!(result.groups.iter().all(|g| g.correct));
        assert!((result.char_wpm - 18.0).abs() < 1e-9);
        assert_eq!(result.timestamp, 3_000);
    }

    /// Characters typed that were never sent cost accuracy. Without that a
    /// padded answer, or a doubled key, scored as a perfect copy — and the
    /// score and the level both follow this accuracy.
    #[test]
    fn padding_an_answer_is_not_a_perfect_copy() {
        let session = answered(
            TrainingSettings::default(),
            &[("KMURE", "KMUURE", 400), ("KM", "KMN", 400)],
        );
        let result = result(&session);
        assert!(
            (result.accuracy - 7.0 / 9.0).abs() < 1e-9,
            "{}",
            result.accuracy
        );
        assert_eq!(result.tally.inserted.values().sum::<u32>(), 2);
    }

    #[test]
    fn unconfirmed_and_empty_groups_are_not_scored() {
        let mut session = answered(TrainingSettings::default(), &[("KM", "KM", 400)]);
        session.groups.push(Group::blank());
        session.groups[1].sent = "RS".into();
        let scored = result(&session);
        assert_eq!(scored.groups.len(), 1);
        assert_eq!(scored.group_timings.len(), 1);

        // A confirmed group with nothing in it scores nothing at all.
        let silent = answered(TrainingSettings::default(), &[("", "", 400)]);
        assert!(result(&silent).groups.is_empty());
    }

    /// The same copy, sent at `wpm`, each group sent `sends` times.
    fn copied_at(wpm: f64, sends: u32) -> SessionResult {
        let copies = [("KMURE", "KMURE"), ("SNAPT", "SNAPT"), ("LWIJZ", "LWIXZ")];
        let mut session =
            GroupSession::new(SessionId::new(1), copies.len(), TrainingSettings::default());
        let mut now = MonoMs(1_000);
        for (index, (sent, typed)) in copies.iter().enumerate() {
            session.set_group(index, (*sent).into(), sends);
            session.begin_group(index, now);
            session.start_playback(index, &speeds(wpm, wpm));
            for _ in 0..sends {
                session.note_play_finished(index, true);
            }
            now = now.plus(2_000);
            session.end_playback(index, now);
            session.set_input(index, typed, &[]);
            now = now.plus(600);
            session.confirm(index, now);
        }
        result(&session)
    }

    /// Thirty-five words a minute earns more than twenty for the same copy —
    /// it used to earn exactly the same.
    #[test]
    fn a_faster_session_earns_more() {
        let (slow, fast) = (copied_at(20.0, 1), copied_at(35.0, 1));
        assert_eq!(slow.accuracy, fast.accuracy);
        assert!(
            fast.score > 2.5 * slow.score,
            "{} at 35 WPM against {} at 20",
            fast.score,
            slow.score
        );
    }

    /// A group sent twice earns half: hearing it again is RufzXP's replay.
    #[test]
    fn a_group_heard_twice_earns_half() {
        let (once, twice) = (copied_at(25.0, 1), copied_at(25.0, 2));
        assert_eq!(twice.group_timings[0].sends, 2);
        assert!(
            (twice.score - once.score / 2.0).abs() < 0.05,
            "{} against {}",
            twice.score,
            once.score
        );
    }

    /// A session is scored from what is stored of it, so one saved before
    /// speed counted — no spacing or sends per group — is scored the same way
    /// as one finished today.
    #[test]
    fn a_stored_session_is_scored_from_what_is_stored() {
        let today = copied_at(30.0, 1);
        assert_eq!(today.points(), today.score);
        let mut json = serde_json::to_value(&today).expect("serializes");
        for timing in json["groupTimings"].as_array_mut().expect("timings") {
            let timing = timing.as_object_mut().expect("an object");
            timing.remove("effectiveWpm");
            timing.remove("sends");
        }
        json["score"] = serde_json::json!(1.0);
        let older: SessionResult = serde_json::from_value(json).expect("an older save reads");
        assert_eq!(older.group_timings[0].effective_wpm, None);
        assert_eq!(older.group_timings[0].sends, 1);
        assert!(
            (older.points() - today.score).abs() < 0.01,
            "{} against {}",
            older.points(),
            today.score
        );
    }

    #[test]
    fn an_empty_answer_is_scored_wrong() {
        let result = result(&answered(TrainingSettings::default(), &[("KM", "", 400)]));
        assert_eq!(result.groups.len(), 1);
        assert!(!result.groups[0].correct);
        assert!(result.accuracy < 0.01);
    }

    /// Answers typed while the group is still being sent used to be timed at
    /// a millisecond, and the speed term made them worth ten to twenty times
    /// an answer a second after the send. They are as fast as a person types.
    #[test]
    fn an_answer_that_beats_the_send_scores_as_a_fast_typist_not_as_instant() {
        let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        session.set_group(0, "KMURE".into(), 1);
        session.begin_group(0, MonoMs(1_000));
        session.set_input(0, "KMURE", &[]);
        session.confirm(0, MonoMs(1_400));
        let early = result(&session);
        assert_eq!(
            early.group_timings[0].time_to_complete_ms,
            5.0 * MIN_RESPONSE_MS_PER_CHAR
        );
        assert_eq!(early.avg_response_ms, MIN_RESPONSE_MS_PER_CHAR);

        let slower = result(&answered(
            TrainingSettings::default(),
            &[("KMURE", "KMURE", 1_500)],
        ));
        assert!(
            early.score <= slower.score * 2.0,
            "{} against {}",
            early.score,
            slower.score
        );
    }

    #[test]
    fn the_answer_is_timed_from_the_first_moment_it_was_complete() {
        let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        session.set_group(0, "KMURE".into(), 1);
        session.begin_group(0, MonoMs(1_000));
        session.start_playback(0, &speeds(20.0, 18.0));
        session.end_playback(0, MonoMs(2_000));
        session.mark_answer_complete(0, true, MonoMs(3_000));
        session.mark_answer_complete(0, true, MonoMs(3_500));
        session.confirm(0, MonoMs(4_000));
        assert_eq!(
            result(&session).group_timings[0].time_to_complete_ms,
            1_000.0
        );

        // A stamp withdrawn by a backspace is gone; the confirm is the answer.
        let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        session.set_group(0, "KMURE".into(), 1);
        session.start_playback(0, &speeds(20.0, 18.0));
        session.end_playback(0, MonoMs(2_000));
        session.mark_answer_complete(0, true, MonoMs(2_500));
        session.mark_answer_complete(0, false, MonoMs(2_600));
        session.confirm(0, MonoMs(4_000));
        let timing = &result(&session).group_timings[0];
        assert_eq!(timing.time_to_complete_ms, 2_000.0);
        assert_eq!(timing.char_wpm, Some(20.0));
    }

    /// A send at these speeds, with no character ends to time from.
    fn speeds(char_wpm: f64, effective_wpm: f64) -> SendStart {
        SendStart {
            char_wpm,
            effective_wpm,
            output_at: MonoMs(0),
            char_ends_ms: Vec::new(),
            latency_ms: 0,
        }
    }

    /// A send of `index` starting at `output_at`, at `wpm`, its characters
    /// ending `ends` ms after it, through a device reporting 40 ms.
    fn send_start(output_at: u64, wpm: f64, ends: &[u32]) -> SendStart {
        SendStart {
            char_wpm: wpm,
            effective_wpm: wpm,
            output_at: MonoMs(output_at),
            char_ends_ms: ends.to_vec(),
            latency_ms: 40,
        }
    }

    /// "KMR" sent once from 1000 ms, its characters ending 300, 700 and 1000
    /// ms into it, typed at `keys` with the box `open` or not: what its
    /// timing keeps.
    fn kept(keys: &[Option<u64>], open: bool) -> Option<CharTimes> {
        let mut settings = TrainingSettings::default();
        settings.playback.lock_input_during_group_playback = !open;
        let mut session = GroupSession::new(SessionId::new(1), 1, settings);
        session.set_group(0, "KMR".into(), 1);
        session.begin_group(0, MonoMs(980));
        session.start_playback(0, &send_start(1_000, 20.0, &[300, 700, 1_000]));
        let keys: Vec<Option<MonoMs>> = keys.iter().map(|at| at.map(MonoMs)).collect();
        session.set_input(0, "KMR", &keys);
        session.note_play_finished(0, true);
        session.end_playback(0, MonoMs(2_050));
        session.confirm(0, MonoMs(3_000));
        result(&session).group_timings[0].char_times.clone()
    }

    /// A group keeps the raw facts its characters are timed from: when each
    /// ended after its first sample went out, when each was keyed on the
    /// same clock, the reported delay, and whether the box was open.
    #[test]
    fn a_group_keeps_when_its_characters_ended_and_were_keyed() {
        assert_eq!(
            kept(&[Some(1_550), Some(1_900), None], true),
            Some(CharTimes {
                ends: vec![300, 700, 1_000],
                keys: vec![Some(550), Some(900), None],
                latency_ms: 40,
                box_open: true,
            })
        );
        assert_eq!(
            kept(&[Some(2_300)], false).map(|times| times.box_open),
            Some(false)
        );
        // Keys for more than was typed are no character of it.
        assert_eq!(
            kept(&[Some(1_550), Some(1_900), Some(2_250), Some(2_400)], true)
                .map(|times| times.keys.len()),
            Some(3)
        );
    }

    /// A first send tried again was heard in part already, and one held up
    /// on its way would time every character from the wrong moment: neither
    /// is timed. A key after a later send began came after hearing it twice.
    #[test]
    fn only_a_first_send_on_time_and_once_is_timed() {
        let session = |retried: bool, on_time: bool| {
            let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
            session.set_group(0, "K".into(), 1);
            session.begin_group(0, MonoMs(980));
            session.start_playback(0, &send_start(1_000, 20.0, &[300]));
            if retried {
                session.start_playback(0, &send_start(1_500, 20.0, &[300]));
            }
            session.note_play_finished(0, on_time);
            session.end_playback(0, MonoMs(1_350));
            session.set_input(0, "K", &[Some(MonoMs(1_700))]);
            session.confirm(0, MonoMs(2_000));
            result(&session).group_timings[0].char_times.is_some()
        };
        assert!(session(false, true));
        assert!(!session(true, true));
        assert!(!session(false, false));

        let mut twice = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        twice.set_group(0, "KM".into(), 2);
        twice.begin_group(0, MonoMs(980));
        twice.start_playback(0, &send_start(1_000, 20.0, &[300, 700]));
        twice.note_play_finished(0, true);
        twice.start_playback(0, &send_start(2_500, 20.0, &[300, 700]));
        twice.set_input(0, "KM", &[Some(MonoMs(1_500)), Some(MonoMs(2_600))]);
        twice.note_play_finished(0, true);
        twice.end_playback(0, MonoMs(3_200));
        twice.confirm(0, MonoMs(3_500));
        let timing = &result(&twice).group_timings[0];
        assert_eq!(
            timing.char_times.as_ref().map(|times| times.keys.clone()),
            Some(vec![Some(500), None])
        );
        assert_eq!(timing.sends, 2);
    }

    /// A send's speeds are kept as it starts, so a group answered before its
    /// send was seen to end is scored at the speed it was sent at.
    #[test]
    fn a_group_answered_before_its_send_ends_keeps_its_speed() {
        let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        session.set_group(0, "KM".into(), 1);
        session.begin_group(0, MonoMs(1_000));
        session.start_playback(0, &send_start(1_020, 27.0, &[200, 500]));
        session.set_input(0, "KM", &[]);
        session.confirm(0, MonoMs(1_600));
        let timing = &result(&session).group_timings[0];
        assert_eq!(
            (timing.char_wpm, timing.effective_wpm),
            (Some(27.0), Some(27.0))
        );
    }

    /// Typing is kept the way a group is written: no spaces, upper case, and
    /// not without end.
    #[test]
    fn an_answer_is_tidied_and_capped_on_the_way_in() {
        let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        session.set_group(0, "KM".into(), 1);
        session.set_input(0, " k m\t", &[]);
        assert_eq!(session.group(0).map(Group::input), Some("KM"));
        session.set_input(0, &"E".repeat(100_000), &[]);
        assert_eq!(
            session.group(0).map(|g| g.input().len()),
            Some(LONGEST_ANSWER)
        );
        let mut long = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        long.set_group(0, "KMUREKMUREKMUREKMURE".into(), 1);
        long.set_input(0, &"E".repeat(100), &[]);
        assert_eq!(long.group(0).map(|g| g.input().len()), Some(40));
    }

    #[test]
    fn input_is_only_accepted_for_the_current_unconfirmed_group() {
        let mut session = GroupSession::new(SessionId::new(1), 2, TrainingSettings::default());
        session.set_group(0, "KM".into(), 1);
        session.set_group(1, "UR".into(), 1);
        session.set_input(1, "nope", &[]);
        assert_eq!(session.group(1).map(Group::input), Some(""));
        session.set_input(0, "km", &[]);
        assert_eq!(session.group(0).map(Group::input), Some("KM"));
        assert!(session.confirm(0, MonoMs(1_200)).is_some());
        assert!(session.confirm(0, MonoMs(1_300)).is_none(), "only once");
        session.set_input(0, "later", &[]);
        assert_eq!(session.group(0).map(Group::input), Some("KM"));
        // A confirmed group keeps its text too.
        session.set_group(0, "XX".into(), 1);
        assert_eq!(session.group(0).map(Group::sent), Some("KM"));
    }

    #[test]
    fn confirming_only_takes_the_current_group_and_moves_the_focus_on() {
        let mut session = GroupSession::new(SessionId::new(1), 2, TrainingSettings::default());
        session.set_group(0, "KM".into(), 1);
        assert!(session.confirm(1, MonoMs(1_100)).is_none());
        assert!(session.confirm(9, MonoMs(1_100)).is_none());
        assert!(session.confirm(0, MonoMs(1_100)).is_some());
        assert!(session.any_confirmed());
        assert_eq!(session.view().focused, 1);

        // The last group keeps the focus.
        session.begin_group(1, MonoMs(2_000));
        assert!(session.confirm(1, MonoMs(2_100)).is_some());
        assert_eq!(session.view().focused, 1);
    }

    #[test]
    fn the_view_mirrors_the_current_group() {
        let mut settings = TrainingSettings::default();
        settings.playback.lock_input_during_group_playback = true;
        let mut session = GroupSession::new(SessionId::new(3), 2, settings);
        session.set_group(0, "KM".into(), 3);
        session.set_group(1, "UR".into(), 0);
        session.begin_group(0, MonoMs(1_000));
        session.note_play_finished(0, true);
        let view = session.view();
        assert!(view.playing && view.locked);
        assert_eq!(
            (view.current, view.repeat_total, view.repeat_done),
            (0, 3, 1)
        );
        assert_eq!(view.sent, vec!["KM", "UR"]);
        assert_eq!(view.confirmed, vec![false, false]);
        // Repeats are never below one.
        assert_eq!(session.group_repeats(1), 1);
        assert_eq!(session.group_repeats(9), 1);

        session.set_phase(SessionPhase::AwaitingAnswer { index: 0 });
        assert!(!session.view().playing);
        assert!(!session.input_locked(0));
        // A session always has at least one group, even if asked for none.
        let empty = GroupSession::new(SessionId::new(1), 0, TrainingSettings::default());
        assert_eq!(empty.group_count(), 1);
    }

    #[test]
    fn the_focus_moves_with_the_group_being_sent() {
        let mut session = GroupSession::new(SessionId::new(1), 2, TrainingSettings::default());
        assert_eq!(session.view().focused, 0);
        session.begin_group(1, MonoMs(10));
        assert_eq!(session.view().focused, 1);
        // Beginning a group that is not there changes nothing.
        session.begin_group(9, MonoMs(20));
        assert_eq!(session.current_group(), 1);
    }

    #[test]
    fn an_answer_matches_only_at_the_same_length() {
        assert!(answer_length_matches("KM", "km"));
        assert!(answer_length_matches("KM", " k m "));
        assert!(!answer_length_matches("KM", "k"));
        assert!(!answer_length_matches("KM", "kmu"));
        assert!(!answer_length_matches("", ""));
    }

    #[test]
    fn the_result_records_the_speed_it_was_sent_at() {
        let mut settings = TrainingSettings::default();
        settings.playback.effective_wpm_min = 18.0;
        let mut session = GroupSession::new(SessionId::new(1), 1, settings.clone());
        session.set_group(0, "KM".into(), 1);
        session.start_playback(0, &speeds(25.0, 12.0));
        session.end_playback(0, MonoMs(500));
        session.confirm(0, MonoMs(1_000));
        let played = result(&session);
        assert!((played.char_wpm - 25.0).abs() < 1e-9);
        assert!((played.effective_wpm - 12.0).abs() < 1e-9);
        assert_eq!(played.alphabet_fingerprint, settings.alphabet_fingerprint());

        // A session whose sends never reported a speed records the settings'.
        settings.playback.link_char_to_effective = false;
        settings.playback.char_wpm_min = 22.0;
        settings.playback.effective_wpm_min = 14.0;
        let mut silent = GroupSession::new(SessionId::new(1), 1, settings);
        silent.set_group(0, "KM".into(), 1);
        silent.confirm(0, MonoMs(10));
        let unplayed = result(&silent);
        assert_eq!((unplayed.char_wpm, unplayed.effective_wpm), (22.0, 14.0));
    }

    #[test]
    fn a_digits_session_records_the_digits_level() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Digits;
        settings.curriculum.digits_level = 4;
        let result = result(&answered(settings, &[("01", "01", 10)]));
        assert_eq!(result.level, 4);
        assert_eq!(result.digits_level, 4);
        assert_eq!(result.alphabet_fingerprint, "0123456789");
    }

    /// The tally is saved as the flat keys a session always had, plus the
    /// characters typed but never sent when there were any.
    #[test]
    fn a_session_tally_round_trips_through_its_saved_keys() {
        let result = result(&answered(TrainingSettings::default(), &[("KM", "KMN", 10)]));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["letterAccuracy"]["K"]["correct"], 1);
        assert_eq!(json["inserted"]["N"], 1);
        assert!(json.get("tally").is_none());
        let back: SessionResult = serde_json::from_value(json).unwrap();
        assert_eq!(back, result);

        let mut clean = result;
        clean.tally.inserted.clear();
        let json = serde_json::to_value(&clean).unwrap();
        assert!(json.get("inserted").is_none());
    }

    /// Older saves carry fields this version no longer writes, and lack ones
    /// it added. Both still load.
    #[test]
    fn legacy_session_json_fills_new_fields() {
        let timing: SessionTiming =
            serde_json::from_str(r#"{"timeToCompleteMs":1,"perCharMs":1}"#).unwrap();
        assert_eq!(timing.char_wpm, None);
        let raw = r#"{
            "date":"2026-01-01","timestamp":1,"startedAt":0,"finishedAt":1,
            "groups":[],"groupTimings":[],"accuracy":1,
            "letterAccuracy":{"K":{"correct":1,"total":2}},
            "alphabetSize":0,"avgResponseMs":0,"totalChars":0,
            "effectiveAlphabetSize":0,"score":0,"kochLevel":3
        }"#;
        let result: SessionResult = serde_json::from_str(raw).unwrap();
        assert_eq!(
            result.tally.sent.get(&'K'),
            Some(&crate::alignment::LetterAccuracy {
                correct: 1,
                total: 2
            })
        );
        assert!(result.tally.inserted.is_empty());
        assert_eq!(result.level, 3);
        assert_eq!(result.digits_level, 1);
        assert_eq!(result.char_set_mode, CharSetMode::Koch);
        assert_eq!(result.char_wpm, 18.0);
        assert_eq!(result.effective_wpm, 18.0);
        assert!(result.alphabet_fingerprint.is_empty());

        let json = serde_json::to_value(&result).unwrap();
        for gone in [
            "startedAt",
            "finishedAt",
            "alphabetSize",
            "effectiveAlphabetSize",
            "perCharMs",
        ] {
            assert!(json.get(gone).is_none(), "{gone} is still written");
        }
    }

    /// `SessionResult` as builds before contest mode have it, key for key,
    /// with the character sets they know: what an older build makes of a
    /// session this one writes.
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BeforeContests {
        date: String,
        timestamp: u64,
        groups: Vec<GroupResult>,
        group_timings: Vec<SessionTiming>,
        accuracy: f64,
        #[serde(flatten)]
        tally: CopyTally,
        avg_response_ms: f64,
        total_chars: u32,
        score: f64,
        #[serde(alias = "kochLevel", default = "session_level_default")]
        level: u32,
        #[serde(default = "session_level_default")]
        digits_level: u32,
        #[serde(default)]
        char_set_mode: ModeBeforeContests,
        #[serde(default = "session_wpm_default")]
        char_wpm: f64,
        #[serde(default = "session_wpm_default")]
        effective_wpm: f64,
        #[serde(default)]
        alphabet_fingerprint: String,
    }

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum ModeBeforeContests {
        #[default]
        #[serde(alias = "custom")]
        Koch,
        Digits,
        Mixed,
        Callsign,
        Words,
    }

    /// A contest run as a later build records one: a call and its number as
    /// groups, the mark, and a log.
    fn contest_run() -> SessionResult {
        let timing = SessionTiming {
            time_to_complete_ms: 900.0,
            char_wpm: Some(26.0),
            effective_wpm: Some(26.0),
            sends: 1,
            ..SessionTiming::default()
        };
        SessionResult {
            groups: vec![
                GroupResult {
                    sent: "K5XYZ".into(),
                    received: "K5XYZ".into(),
                    correct: true,
                },
                GroupResult {
                    sent: "TT7".into(),
                    received: "TT1".into(),
                    correct: false,
                },
            ],
            group_timings: vec![timing.clone(), timing],
            char_set_mode: CharSetMode::Contest,
            level: 2,
            alphabet_fingerprint: "contest:wpx".into(),
            contest: Some(ContestLog(serde_json::json!({
                "v": 1,
                "kind": "wpx",
                "qsos": [{ "t": 61_000, "h": 58_400, "g": 2, "nr": 1 }],
            }))),
            ..SessionResult::empty_for_test()
        }
    }

    /// A run goes to disk as a callsign session, with its mark and its log,
    /// and comes back as the run it was.
    #[test]
    fn a_contest_run_is_written_as_a_callsign_session_with_its_mark_and_log() {
        let run = contest_run();
        let json = serde_json::to_value(&run).unwrap();
        assert_eq!(json["charSetMode"], "callsign");
        assert_eq!(json["alphabetFingerprint"], "contest:wpx");
        assert_eq!(
            Some(&json["contest"]),
            run.contest.as_ref().map(|log| &log.0)
        );
        let back: SessionResult = serde_json::from_value(json).unwrap();
        assert_eq!(back, run);

        // Callsign practice is still callsign practice, and writes no log.
        let mut calls = SessionResult::empty_for_test();
        calls.char_set_mode = CharSetMode::Callsign;
        calls.alphabet_fingerprint = "callsign".into();
        let json = serde_json::to_value(&calls).unwrap();
        assert!(json.get("contest").is_none());
        let back: SessionResult = serde_json::from_value(json).unwrap();
        assert_eq!(back.char_set_mode, CharSetMode::Callsign);
    }

    /// A build that knows no contests keeps a run as callsign practice, and
    /// drops its log the next time it saves. The mark it keeps is what makes
    /// the session a run again after an upgrade.
    #[test]
    fn a_build_before_contests_keeps_a_run_and_its_mark_brings_it_back() {
        let written = serde_json::to_value(contest_run()).unwrap();
        let older: BeforeContests =
            serde_json::from_value(written.clone()).expect("an older build reads a run");
        assert_eq!(older.char_set_mode, ModeBeforeContests::Callsign);
        assert_eq!(older.groups, contest_run().groups);

        let stripped = serde_json::to_value(&older).unwrap();
        assert!(
            stripped.get("contest").is_none(),
            "the older build drops the log"
        );
        assert_eq!(stripped["alphabetFingerprint"], "contest:wpx");
        let back: SessionResult = serde_json::from_value(stripped).unwrap();
        assert_eq!(back.char_set_mode, CharSetMode::Contest);
        assert_eq!(back.contest, None);
        assert_eq!(back.groups, contest_run().groups);

        // Written under a name of its own, the run would have cost the
        // older build the whole session.
        let mut named = written;
        named["charSetMode"] = serde_json::json!("contest");
        assert!(serde_json::from_value::<BeforeContests>(named).is_err());
    }

    /// A log this version cannot read, an empty one or a later layout, still
    /// marks a run, and goes back to disk exactly as it was read.
    #[test]
    fn a_log_this_version_cannot_read_goes_back_as_it_was_read() {
        for log in [
            serde_json::json!({}),
            serde_json::json!({
                "v": 2,
                "contestKind": "wpx",
                "operators": 2,
                "log": [
                    { "at": 61.5, "copied": ["K5XYZ", "TT7"], "drift": -0.1, "note": null },
                    { "at": 1e300, "copied": [], "serial": 18_446_744_073_709_551_615u64 },
                ],
            }),
        ] {
            let mut written = serde_json::to_value(SessionResult::empty_for_test()).unwrap();
            written["charSetMode"] = serde_json::json!("callsign");
            written["contest"] = log.clone();
            let read: SessionResult = serde_json::from_value(written.clone()).unwrap();
            assert_eq!(read.char_set_mode, CharSetMode::Contest, "{log}");
            assert_eq!(serde_json::to_value(&read).unwrap(), written);
        }
    }

    /// Every mode a group session runs in is the mode it is recorded as.
    #[test]
    fn a_group_session_records_the_mode_it_ran_in() {
        for mode in CharSetMode::ALL
            .into_iter()
            .filter(|mode| *mode != CharSetMode::Contest)
        {
            let mut settings = TrainingSettings::default();
            settings.curriculum.char_set_mode = mode;
            let recorded = result(&answered(settings, &[("KM", "KM", 400)]));
            assert_eq!(recorded.char_set_mode, mode);
            assert_eq!(recorded.contest, None);
            let json = serde_json::to_value(&recorded).unwrap();
            let back: SessionResult = serde_json::from_value(json).unwrap();
            assert_eq!(back, recorded);
        }
    }

    /// Should a group session ever run in Contest mode, it is filed as the
    /// callsign practice it was; every other mode is filed as itself.
    #[test]
    fn a_group_session_is_never_recorded_as_a_contest_run() {
        for mode in CharSetMode::ALL {
            let recorded = recorded_mode(mode);
            assert_ne!(recorded, CharSetMode::Contest);
            if mode != CharSetMode::Contest {
                assert_eq!(recorded, mode);
            }
        }
        assert_eq!(recorded_mode(CharSetMode::Contest), CharSetMode::Callsign);
    }

    /// A contest run is only ever recorded by its own machine, with its log,
    /// so a group session run in Contest mode is a bug a debug build says
    /// out loud.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a contest run recorded as a group session")]
    fn a_debug_build_says_when_a_group_session_ran_as_a_contest() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        result(&answered(settings, &[("K5XYZ", "K5XYZ", 400)]));
    }

    #[test]
    fn a_monotonic_moment_never_runs_backwards() {
        assert_eq!(MonoMs(1_500).since(MonoMs(1_000)), 500);
        assert_eq!(MonoMs(1_000).since(MonoMs(1_500)), 0);
        assert_eq!(MonoMs(u64::MAX).plus(5), MonoMs(u64::MAX));
    }
}
