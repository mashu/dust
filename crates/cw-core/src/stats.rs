//! Letter diagnostics, bigrams, confusion, and sampling snapshots for the stats UI.

use std::collections::BTreeMap;

use crate::alignment::align_group;
use crate::belief::{EstimatePoint, sessions_faded};
use crate::board::BOARD_MIN_GROUPS;
use crate::contest::{ContestKind, StoredLog};
use crate::morse::LCWO_SEQUENCE;
use crate::numeric::median;
use crate::pool::compute_char_pool;
use crate::recognition::{
    RECOGNITION_MIN_TRIES, RECOGNITION_WINDOW, Recognition, RecognitionBands, first_key_late_ms,
    recognise, typed_along,
};
use crate::sampling::{
    CHAR_SAMPLING_PRIOR_ALPHA, CHAR_SAMPLING_PRIOR_BETA, CharSamplingConfig, CharSamplingState,
    adaptive_bounds, adapts_speed, belief_for, beta_posterior_mean_error,
    compute_raw_sampling_weights, config_from_settings, draw_chances, generate_callsign_group,
    generate_contest_call, generate_word_group, group_size_bounds,
};
use crate::session::{SessionResult, Untimed};
use crate::settings::{CharSetMode, TrainingSettings};
use crate::span::{LengthForecast, SpanBelief};
use crate::speed::{SpeedBelief, SpeedForecast, SpeedPath, forecast_speeds, station_shares};

pub const MASTERED_MIN_ATTEMPTS: u32 = 5;
pub const MASTERED_MIN_ACCURACY: f64 = 0.9;
pub const BUILDING_MIN_ACCURACY: f64 = 0.7;

pub const GROUP_START_BIGRAM_TOKEN: char = '▸';

/// How often a pair has to have been heard, and missed, before it can be
/// called one of the hardest: fewer is chance as often as not.
pub const HARD_PAIR_MIN_TRIES: u32 = 5;
const HARD_PAIR_MIN_WRONG: u32 = 2;
/// How much more often than its character does overall a pair has to go
/// wrong to be called hard: ten points of error rate, and half as often again.
const HARD_PAIR_MIN_EXCESS: f64 = 0.10;
pub const HARD_PAIR_RATIO: f64 = 1.5;
/// How often a history whose pairs are no harder than their letters lists
/// one anyway: one in twenty. With hundreds of pairs heard, a few always go
/// wrong more often by luck alone, so this is shared among every pair heard
/// [`HARD_PAIR_MIN_TRIES`] times, each held to its part of it.
const HARD_PAIR_CHANCE: f64 = 0.05;
/// How many of the hardest pairs are listed.
const HARD_PAIRS_SHOWN: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasteryStatus {
    Mastered,
    Building,
    Weak,
    /// Heard fewer than [`MASTERED_MIN_ATTEMPTS`] times: too few to call it
    /// anything, right every time or not.
    FewTries,
}

/// How many of a character's latest tries count as recent: whole sessions,
/// newest first, until there are this many.
pub const RECENT_TRIES: u32 = 20;
/// The fewest tries before them for there to be an earlier to compare with.
const EARLIER_MIN_TRIES: u32 = 10;
/// How many of the tries before the recent ones count as earlier: whole
/// sessions, newest first, until there are this many. What was put right
/// long before is not what changed lately.
pub const EARLIER_TRIES: u32 = 100;
/// How many times its noise recent accuracy has to stand from earlier to be
/// a change: a two-proportion test's standard error, chance alone moving a
/// steady character that far about once in a hundred.
const LATELY_SPREADS: f64 = 2.58;
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterDiagnostic {
    pub letter: char,
    /// The share of its tries copied right, older sessions counting for
    /// less: a session ten sessions back counts half as much as the latest,
    /// as the sampler's belief and the span and speed fade. What its status
    /// is read from.
    pub accuracy_pct: f64,
    /// Every time it was sent, and copied right, counted in full.
    pub total: u32,
    pub correct: u32,
    /// What its tries are worth by now, faded as `accuracy_pct` is: what
    /// its likely range is read from.
    pub weight: f64,
    pub status: MasteryStatus,
    /// Accuracy over the latest sessions that hold [`RECENT_TRIES`] tries of
    /// it, and over the sessions before them that hold [`EARLIER_TRIES`], in
    /// percent: which way it is going. None until there are enough of both.
    pub recent_pct: Option<f64>,
    pub earlier_pct: Option<f64>,
    /// How many tries each was read from; none without them.
    pub recent_tries: u32,
    pub earlier_tries: u32,
    /// How soon you type it after its sound went out, in milliseconds: the
    /// median of its latest [`RECOGNITION_WINDOW`] timed tries, once any is
    /// ([`crate::recognise`] says which are).
    pub recognition_ms: Option<f64>,
    /// How many tries of it have been timed.
    pub recognition_tries: u32,
}

impl CharacterDiagnostic {
    /// How fast it is recognised under `bands`: in a band once
    /// [`RECOGNITION_MIN_TRIES`] tries are timed, as fewer let one slow key
    /// move the median too far; and instant (ICR) only once it is copied
    /// right often enough to be building or mastered, a weak one's time
    /// being only that of the tries it got right.
    pub fn recognition(&self, bands: RecognitionBands) -> Recognition {
        match self.recognition_ms {
            Some(ms) if self.recognition_tries >= RECOGNITION_MIN_TRIES => {
                let band = bands.band_of(ms);
                Recognition::Banded {
                    ms,
                    band,
                    instant: band.counts_as_icr()
                        && matches!(
                            self.status,
                            MasteryStatus::Mastered | MasteryStatus::Building
                        ),
                }
            }
            Some(ms) => Recognition::Provisional {
                ms,
                tries: self.recognition_tries,
            },
            None => Recognition::Untimed,
        }
    }

    /// How far recent accuracy moved from earlier, in points, when it moved
    /// further than chance would over the tries each was read from; none
    /// when it did not, or while there is no trend to read.
    pub fn moved_lately(&self) -> Option<f64> {
        let (recent, earlier) = (self.recent_pct? / 100.0, self.earlier_pct? / 100.0);
        let (new, old) = (f64::from(self.recent_tries), f64::from(self.earlier_tries));
        if new <= 0.0 || old <= 0.0 {
            return None;
        }
        let right = (recent * new + earlier * old) / (new + old);
        let noise = (right * (1.0 - right) * (1.0 / new + 1.0 / old)).sqrt();
        let moved = recent - earlier;
        (moved.abs() > LATELY_SPREADS * noise).then_some(100.0 * moved)
    }

    /// The range its accuracy is likely in ([`rate_interval`]), from its
    /// tries as faded: wider again once it has not been heard for a while.
    pub fn accuracy_interval(&self, mass: f64) -> (f64, f64) {
        let right = self.accuracy_pct / 100.0 * self.weight;
        let wrong = self.weight - right;
        rate_interval(right + 1.0, wrong.max(0.0) + 1.0, mass)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccuracyPoint {
    pub date: String,
    pub accuracy_pct: f64,
    pub timestamp: u64,
}

/// One pair: `col` heard straight after `row`, and how often `col` was
/// copied wrong there.
#[derive(Clone, Debug, PartialEq)]
pub struct BigramCell {
    pub row: char,
    pub col: char,
    pub rate: f64,
    pub total: u32,
    pub wrong: u32,
    /// What `col` was copied as when it went wrong here, most often first:
    /// the character typed, or none for one left out.
    pub taken_for: Vec<(Option<char>, u32)>,
    /// How dark its square is drawn: its rate, with how `col` does overall
    /// counted as [`HARD_PAIR_MIN_TRIES`] tries of it, so that a pair heard
    /// once or twice is drawn near its letter's usual rather than at none
    /// or all of the time.
    pub shade: f64,
    /// It goes wrong clearly more often than `col` does overall: one of the
    /// hardest pairs, before they are cut to a handful.
    pub stands_out: bool,
}

/// How often a character went wrong, wherever it came: what a pair is held
/// against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CharTally {
    pub wrong: u32,
    pub total: u32,
}

impl CharTally {
    pub fn rate(self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            f64::from(self.wrong) / f64::from(self.total)
        }
    }
}

/// A pair that goes wrong more often than its second character does
/// overall: the pair is the trouble, not the letter.
#[derive(Clone, Debug, PartialEq)]
pub struct HardPair {
    pub row: char,
    pub col: char,
    pub wrong: u32,
    pub total: u32,
    pub rate: f64,
    /// How often `col` goes wrong overall.
    pub overall: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BigramHeatmap {
    /// The character each pair starts from, a row each: the start of a
    /// group first, then every character sent, in the order they are taught.
    pub rows: Vec<char>,
    /// The character each pair ends on, a column each: every character sent,
    /// in the same order. Nothing ends on the start of a group.
    pub cols: Vec<char>,
    /// One per row and column, row by row.
    pub cells: Vec<BigramCell>,
    /// The darkest any heard square is shaded ([`BigramCell::shade`]).
    pub max_shade: f64,
    /// How each column's character fares overall, in the order of `cols`.
    pub overall: Vec<CharTally>,
    /// The pairs that go wrong most often for their second character, worst
    /// first, at most a handful.
    pub hardest: Vec<HardPair>,
}

impl BigramHeatmap {
    /// How `col` fares overall.
    pub fn overall_of(&self, col: char) -> CharTally {
        self.cols
            .iter()
            .position(|c| *c == col)
            .and_then(|at| self.overall.get(at).copied())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfusionEntry {
    pub sent: char,
    pub typed: Option<char>,
    pub count: u32,
    pub percentage: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SamplingRow {
    pub character: char,
    pub p_error: f64,
    /// The chance it is drawn next; nothing for a character the current
    /// set does not draw.
    pub sampling_prob: f64,
    /// The belief in how often it is copied right and wrong, older sessions
    /// counting for less ([`CharSamplingState::from_history`]).
    pub alpha: f64,
    pub beta: f64,
    pub is_letter: bool,
    /// Whether the current set draws it at all.
    pub drawn_now: bool,
    /// How many times it has been sent, every one counted: none for a
    /// character never heard, however long ago the rest were.
    pub tries: u32,
}

impl SamplingRow {
    /// How much the history has said of the character: the weight of what
    /// it was copied right and wrong, beyond the flat prior every character
    /// starts from. Nothing for one never heard, whose error is the prior's
    /// even chance rather than anything measured.
    pub fn evidence(&self) -> f64 {
        (self.alpha + self.beta - CHAR_SAMPLING_PRIOR_ALPHA - CHAR_SAMPLING_PRIOR_BETA).max(0.0)
    }

    /// The range the error rate is likely in: of a Beta of what was missed
    /// against what was copied ([`rate_interval`]).
    pub fn error_interval(&self, mass: f64) -> (f64, f64) {
        rate_interval(self.beta, self.alpha, mass)
    }
}

/// A belief in a rate, from how often things went `toward` it and `away`
/// from it, each counted with the flat prior's one: a Beta of the two, read
/// as a density at the middle of each of `steps` equal steps from none of
/// the time to all of it, and scaled to take in one over the whole range.
///
/// Worked out on the grid, its scale from the grid's own sum, so no gamma
/// function is needed; a few hundred steps put it within a fraction of a
/// percent of the exact density.
pub fn rate_density(toward: f64, away: f64, steps: usize) -> Vec<f64> {
    let steps = steps.max(1);
    let (toward, away) = (toward.max(1e-9), away.max(1e-9));
    let log: Vec<f64> = (0..steps)
        .map(|step| {
            let x = (step as f64 + 0.5) / steps as f64;
            (toward - 1.0) * x.ln() + (away - 1.0) * (1.0 - x).ln()
        })
        .collect();
    let top = log.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let raw: Vec<f64> = log.iter().map(|value| (value - top).exp()).collect();
    let area = raw.iter().sum::<f64>() / steps as f64;
    raw.into_iter().map(|value| value / area).collect()
}

/// The range a rate believed as [`rate_density`] says is likely: the
/// central `mass` of the belief, from the rate below which half of the rest
/// lies to the one above which the other half does.
pub fn rate_interval(toward: f64, away: f64, mass: f64) -> (f64, f64) {
    const STEPS: usize = 2_000;
    let tail = (1.0 - mass.clamp(0.0, 1.0)) / 2.0;
    let density = rate_density(toward, away, STEPS);
    let (mut low, mut high) = (0.0, 1.0);
    let mut below = 0.0;
    for (step, value) in density.iter().enumerate() {
        let next = below + value / STEPS as f64;
        if below < tail && next >= tail {
            low = step as f64 / STEPS as f64;
        }
        if below < 1.0 - tail && next >= 1.0 - tail {
            high = (step + 1) as f64 / STEPS as f64;
        }
        below = next;
    }
    (low, high)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionHistoryRow {
    pub date: String,
    pub accuracy_pct: f64,
    pub score: f64,
    pub groups: u32,
    pub correct_groups: u32,
    pub total_chars: u32,
    pub timestamp: u64,
    /// What the session practised: every row says, since the history holds
    /// every mode whatever is chosen now.
    pub mode: CharSetMode,
    /// The contest a run was of, when this build knows it.
    pub contest: Option<ContestKind>,
    /// What became of a run's log; none for a session of groups.
    pub log: Option<StoredLog>,
    /// A contest run had Show copy on.
    pub assisted: bool,
    /// Where the session sits in the history the rows were made from, so a
    /// row names one session even when two finished in the same millisecond.
    pub index: usize,
}

/// Sort rank for a character: its place in the LCWO sequence, or last for a
/// character that sequence never teaches (`+` is a Morse character but is not
/// part of LCWO, and any custom alphabet can hold others).
fn lcwo_index(ch: char) -> usize {
    LCWO_SEQUENCE
        .iter()
        .position(|c| *c == ch)
        .unwrap_or(usize::MAX)
}

pub fn classify_mastery(accuracy01: f64, attempts: u32) -> MasteryStatus {
    if attempts < MASTERED_MIN_ATTEMPTS {
        MasteryStatus::FewTries
    } else if accuracy01 >= MASTERED_MIN_ACCURACY {
        MasteryStatus::Mastered
    } else if accuracy01 >= BUILDING_MIN_ACCURACY {
        MasteryStatus::Building
    } else {
        MasteryStatus::Weak
    }
}

pub fn accuracy_chart(sessions: &[SessionResult]) -> Vec<AccuracyPoint> {
    let mut rows: Vec<&SessionResult> = sessions.iter().collect();
    rows.sort_by_key(|s| s.timestamp);
    rows.into_iter()
        .map(|s| AccuracyPoint {
            date: s.date.clone(),
            accuracy_pct: s.accuracy * 100.0,
            timestamp: s.timestamp,
        })
        .collect()
}

/// Whether a session's accuracy can be read: a session with a broken one
/// still practised, but says nothing about how well.
fn readable_accuracy(session: &SessionResult) -> bool {
    session.accuracy.is_finite() && (0.0..=1.0).contains(&session.accuracy)
}

/// The accuracy of every character `sessions` sent: each session weighs as
/// much as the characters it sent, so one of a group or two moves it no
/// more than those groups do. None where no accuracy can be read.
pub fn overall_accuracy(sessions: &[SessionResult]) -> Option<f64> {
    let (right, sent) = sessions
        .iter()
        .filter(|session| readable_accuracy(session))
        .fold((0.0, 0_u64), |(right, sent), session| {
            let chars = session.total_chars;
            (
                right + session.accuracy * f64::from(chars),
                sent + u64::from(chars),
            )
        });
    (sent > 0).then(|| right / sent as f64)
}

/// The best accuracy of a session long enough to say something: one of
/// [`BOARD_MIN_GROUPS`] groups or more, as the score board takes. A group
/// or two copied right, in a session ended early, is no best.
pub fn best_accuracy(sessions: &[SessionResult]) -> Option<f64> {
    sessions
        .iter()
        .filter(|session| session.groups.len() >= BOARD_MIN_GROUPS && readable_accuracy(session))
        .map(|session| session.accuracy)
        .reduce(f64::max)
}

/// Each character of `session` the rule times (docs/recognition.md), with
/// its time in milliseconds: its latest group first.
fn timed_chars(session: &SessionResult) -> impl Iterator<Item = (char, u32)> + '_ {
    session
        .groups
        .iter()
        .zip(&session.group_timings)
        .rev()
        .filter_map(|(group, timing)| Some((group, timing.char_times.as_ref()?)))
        .flat_map(|(group, times)| {
            let recognised = recognise(&group.sent, &group.received, times);
            group
                .sent
                .chars()
                .zip(recognised)
                .filter_map(|(ch, took)| Some((ch.to_ascii_uppercase(), took?)))
        })
}

/// Each character's timed tries in `sessions`, every mode's: how many there
/// are, and the latest [`RECOGNITION_WINDOW`] of them, in milliseconds,
/// read newest session first.
fn recognition_times(sessions: &[SessionResult]) -> BTreeMap<char, (u32, Vec<f64>)> {
    let mut newest_first: Vec<&SessionResult> = sessions.iter().collect();
    newest_first.sort_by_key(|session| std::cmp::Reverse(session.timestamp));
    let mut times: BTreeMap<char, (u32, Vec<f64>)> = BTreeMap::new();
    for (ch, took) in newest_first.into_iter().flat_map(timed_chars) {
        let (tries, latest) = times.entry(ch).or_default();
        *tries += 1;
        if latest.len() < RECOGNITION_WINDOW {
            latest.push(f64::from(took));
        }
    }
    times
}

/// How fast a session's characters were recognised: the median of those
/// timed, and how many were.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionRecognition {
    pub median_ms: f64,
    pub timed: usize,
}

/// `session`'s recognition, if any of its characters was timed.
pub fn session_recognition(session: &SessionResult) -> Option<SessionRecognition> {
    let times: Vec<f64> = timed_chars(session)
        .map(|(_, took)| f64::from(took))
        .collect();
    Some(SessionRecognition {
        median_ms: median(&times)?,
        timed: times.len(),
    })
}

/// What a session kept of why its characters went untimed, group by group:
/// what the results say, rather than guess, when little of it was timed
/// (docs/recognition.md).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionUntimed {
    /// Groups answered, and the characters sent in them.
    pub groups: usize,
    pub chars: usize,
    /// Characters the rule timed.
    pub timed: usize,
    /// Groups whose first send the audio held up on its way, or that went
    /// out again from the start, heard in part: when their characters
    /// reached you is not known.
    pub held_up: usize,
    /// Groups of more than one character, the box open, whose first key
    /// came too late for them to have been typed along ([`typed_along`]).
    pub typed_after: usize,
    /// How long after each of those had ended its first key came: the
    /// median, in milliseconds.
    pub first_key_late_ms: Option<f64>,
    /// Groups of more than one character heard again before they were
    /// answered, with no key to time before the next send began.
    pub keyed_in_repeat: usize,
}

/// `session`'s [`SessionUntimed`].
pub fn session_untimed(session: &SessionResult) -> SessionUntimed {
    let mut late = Vec::new();
    let mut untimed = SessionUntimed {
        groups: session.groups.len(),
        chars: session.groups.iter().map(|g| g.sent.chars().count()).sum(),
        timed: timed_chars(session).count(),
        ..SessionUntimed::default()
    };
    for (group, timing) in session.groups.iter().zip(&session.group_timings) {
        let longer = group.sent.chars().count() > 1;
        if matches!(timing.untimed, Some(Untimed::HeldUp | Untimed::SentAgain)) {
            untimed.held_up += 1;
        }
        match &timing.char_times {
            Some(times) if longer && times.box_open && !typed_along(times) => {
                if let Some(ms) = first_key_late_ms(times) {
                    untimed.typed_after += 1;
                    late.push(ms as f64);
                }
            }
            None if longer && timing.untimed.is_none() && timing.sends > 1 => {
                untimed.keyed_in_repeat += 1;
            }
            _ => {}
        }
    }
    untimed.first_key_late_ms = median(&late);
    untimed
}

/// How many characters of `sessions` are mastered: [`character_diagnostics`]
/// without the times, which only the Letters card shows.
pub fn mastered_count(sessions: &[SessionResult]) -> usize {
    letter_tallies(sessions)
        .into_iter()
        .filter(|tally| classify_mastery(tally.rate(), tally.total) == MasteryStatus::Mastered)
        .count()
}

/// A character's tries in one session: when, right, and in all.
type SessionTally = (u64, u32, u32);

/// One character's tries over the history.
#[derive(Default)]
struct LetterTally {
    letter: char,
    /// Right and in all, every try counted in full.
    correct: u32,
    total: u32,
    /// Right and in all, each session's tries faded by how many sessions
    /// have passed since ([`sessions_faded`]).
    faded_correct: f64,
    faded_total: f64,
    /// By session, for its trend.
    by_session: Vec<SessionTally>,
}

impl LetterTally {
    /// The share right, as faded: a weakness put right long ago no longer
    /// holds it down.
    fn rate(&self) -> f64 {
        if self.faded_total > 0.0 {
            self.faded_correct / self.faded_total
        } else {
            0.0
        }
    }
}

/// Each character's tallies in `sessions`, in the order of its character.
/// Every session is read, however far back: its tries are counted in full
/// too.
fn letter_tallies(sessions: &[SessionResult]) -> Vec<LetterTally> {
    let mut tallies: BTreeMap<char, LetterTally> = BTreeMap::new();
    for (session, letters, worth) in sessions_faded(sessions, usize::MAX) {
        for (letter, stats) in letters.iter() {
            let tally = tallies.entry(*letter).or_default();
            tally.letter = *letter;
            tally.correct += stats.correct;
            tally.total += stats.total;
            tally.faded_correct += worth * f64::from(stats.correct);
            tally.faded_total += worth * f64::from(stats.total);
            tally
                .by_session
                .push((session.timestamp, stats.correct, stats.total));
        }
    }
    tallies.into_values().collect()
}

pub fn character_diagnostics(sessions: &[SessionResult]) -> Vec<CharacterDiagnostic> {
    let mut recognised = recognition_times(sessions);
    let mut out: Vec<CharacterDiagnostic> = letter_tallies(sessions)
        .into_iter()
        .map(|tally| {
            let accuracy01 = tally.rate();
            let status = classify_mastery(accuracy01, tally.total);
            let letter = tally.letter;
            let (recent, earlier) = trend(tally.by_session).unzip();
            let (recognition_tries, latest) = recognised.remove(&letter).unwrap_or_default();
            let recognition_ms = median(&latest);
            CharacterDiagnostic {
                letter,
                accuracy_pct: accuracy01 * 100.0,
                total: tally.total,
                correct: tally.correct,
                weight: tally.faded_total,
                status,
                recent_pct: recent.map(|(pct, _)| pct),
                earlier_pct: earlier.map(|(pct, _)| pct),
                recent_tries: recent.map_or(0, |(_, tries)| tries),
                earlier_tries: earlier.map_or(0, |(_, tries)| tries),
                recognition_ms,
                recognition_tries,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        let rank = |s: MasteryStatus| match s {
            MasteryStatus::Weak => 0,
            MasteryStatus::Building => 1,
            MasteryStatus::FewTries => 2,
            MasteryStatus::Mastered => 3,
        };
        rank(a.status).cmp(&rank(b.status)).then(
            a.accuracy_pct
                .partial_cmp(&b.accuracy_pct)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    out
}

/// A character's accuracy over its latest sessions that hold
/// [`RECENT_TRIES`] tries, and over the sessions before them that hold
/// [`EARLIER_TRIES`], in percent, each with its tries, from its tallies by
/// session as (when, correct, tries). None until there are enough of both.
fn trend(mut sessions: Vec<(u64, u32, u32)>) -> Option<((f64, u32), (f64, u32))> {
    sessions.sort_by_key(|(when, _, _)| std::cmp::Reverse(*when));
    let (mut recent, mut earlier) = ((0u32, 0u32), (0u32, 0u32));
    for (_, correct, tries) in sessions {
        let into = if recent.1 < RECENT_TRIES {
            &mut recent
        } else if earlier.1 < EARLIER_TRIES {
            &mut earlier
        } else {
            break;
        };
        into.0 += correct;
        into.1 += tries;
    }
    let pct = |(correct, tries): (u32, u32)| (100.0 * f64::from(correct) / f64::from(tries), tries);
    (recent.1 >= RECENT_TRIES && earlier.1 >= EARLIER_MIN_TRIES)
        .then(|| (pct(recent), pct(earlier)))
}

/// What the mistakes screen shows: which characters are taken for which,
/// and which pairs of characters are hard together.
#[derive(Clone, Debug, PartialEq)]
pub struct MistakeStats {
    /// The most common confusions, most common first.
    pub confusions: Vec<ConfusionEntry>,
    pub bigrams: BigramHeatmap,
}

/// Both mistake views from one alignment of every group, rather than one
/// each: the history can be hundreds of sessions long.
pub fn mistake_stats(sessions: &[SessionResult], confusion_limit: usize) -> MistakeStats {
    let mut confused: BTreeMap<(char, Option<char>), u32> = BTreeMap::new();
    let mut sent_totals: BTreeMap<char, u32> = BTreeMap::new();
    let mut pairs: BTreeMap<(char, char), PairTally> = BTreeMap::new();
    for group in sessions.iter().flat_map(|session| &session.groups) {
        let mut previous = GROUP_START_BIGRAM_TOKEN;
        for pair in align_group(&group.sent, &group.received) {
            let Some(sent) = pair.sent_char else {
                continue;
            };
            *sent_totals.entry(sent).or_insert(0) += 1;
            let entry = pairs.entry((previous, sent)).or_default();
            entry.total += 1;
            if !pair.matched {
                *confused.entry((sent, pair.received_char)).or_insert(0) += 1;
                *entry.taken_for.entry(pair.received_char).or_insert(0) += 1;
            }
            previous = sent;
        }
    }
    let mut sent_wrong: BTreeMap<char, u32> = BTreeMap::new();
    for ((sent, _), count) in &confused {
        *sent_wrong.entry(*sent).or_insert(0) += count;
    }
    let overall = |ch: char| CharTally {
        wrong: sent_wrong.get(&ch).copied().unwrap_or(0),
        total: sent_totals.get(&ch).copied().unwrap_or(0),
    };
    MistakeStats {
        bigrams: bigram_grid(sent_totals.keys().copied().collect(), &pairs, overall),
        confusions: confusion_rows(confused, &sent_totals, confusion_limit),
    }
}

/// One pair's tally as the history is read: how often it was heard, and
/// what its second character was copied as each time it went wrong.
#[derive(Default)]
struct PairTally {
    total: u32,
    taken_for: BTreeMap<Option<char>, u32>,
}

fn confusion_rows(
    confused: BTreeMap<(char, Option<char>), u32>,
    sent_totals: &BTreeMap<char, u32>,
    limit: usize,
) -> Vec<ConfusionEntry> {
    let mut rows: Vec<ConfusionEntry> = confused
        .into_iter()
        .map(|((sent, typed), count)| {
            let total = sent_totals.get(&sent).copied().unwrap_or(1);
            ConfusionEntry {
                sent,
                typed,
                count,
                percentage: 100.0 * f64::from(count) / f64::from(total),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.count.cmp(&a.count).then(
            b.percentage
                .partial_cmp(&a.percentage)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    rows.truncate(limit);
    rows
}

/// The grid of every pair of `sent` characters, and of each one starting a
/// group, with how each column's character fares overall.
fn bigram_grid(
    mut cols: Vec<char>,
    pairs: &BTreeMap<(char, char), PairTally>,
    overall: impl Fn(char) -> CharTally,
) -> BigramHeatmap {
    cols.sort_by_key(|c| lcwo_index(*c));
    let rows: Vec<char> = if cols.is_empty() {
        Vec::new()
    } else {
        std::iter::once(GROUP_START_BIGRAM_TOKEN)
            .chain(cols.iter().copied())
            .collect()
    };
    let overall: Vec<CharTally> = cols.iter().map(|c| overall(*c)).collect();
    // Every pair heard often enough is tested, and shares the chance of a
    // list turning up by luck.
    let tested = pairs
        .values()
        .filter(|tally| tally.total >= HARD_PAIR_MIN_TRIES)
        .count();
    let mut max_shade: f64 = 0.0;
    let mut cells = Vec::with_capacity(rows.len() * cols.len());
    for &row in &rows {
        for (&col, &letter) in cols.iter().zip(&overall) {
            let (total, mut taken_for) = match pairs.get(&(row, col)) {
                Some(tally) => (
                    tally.total,
                    tally
                        .taken_for
                        .iter()
                        .map(|(typed, count)| (*typed, *count))
                        .collect::<Vec<_>>(),
                ),
                None => (0, Vec::new()),
            };
            // Most often first; a tie keeps the order the map gave, which
            // puts a character left out before one typed instead.
            taken_for.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            let wrong = taken_for.iter().map(|(_, count)| count).sum::<u32>();
            let rate = if total == 0 {
                0.0
            } else {
                f64::from(wrong) / f64::from(total)
            };
            let prior = f64::from(HARD_PAIR_MIN_TRIES);
            let shade = (f64::from(wrong) + prior * letter.rate()) / (f64::from(total) + prior);
            if total > 0 {
                max_shade = max_shade.max(shade);
            }
            let stands_out = stands_out(wrong, total, letter, tested);
            cells.push(BigramCell {
                row,
                col,
                rate,
                total,
                wrong,
                taken_for,
                shade,
                stands_out,
            });
        }
    }
    let hardest = hardest_pairs(&cells, &cols, &overall);
    BigramHeatmap {
        rows,
        cols,
        cells,
        max_shade,
        overall,
        hardest,
    }
}

/// Whether a pair of `total` tries, `wrong` of them missed, goes wrong
/// clearly more often than its second character, whose tries are
/// `letter`: heard [`HARD_PAIR_MIN_TRIES`] times and missed more than once,
/// at least ten points and half as often again more often than the letter
/// overall, and more than chance would make it, with `tested` pairs
/// sharing [`HARD_PAIR_CHANCE`].
fn stands_out(wrong: u32, total: u32, letter: CharTally, tested: usize) -> bool {
    if total < HARD_PAIR_MIN_TRIES || wrong < HARD_PAIR_MIN_WRONG {
        return false;
    }
    let rate = f64::from(wrong) / f64::from(total);
    let usual = letter.rate();
    rate - usual >= HARD_PAIR_MIN_EXCESS
        && rate >= usual * HARD_PAIR_RATIO
        && chance_of_as_many(wrong, total, letter) <= HARD_PAIR_CHANCE / tested.max(1) as f64
}

/// The chance that `wrong` or more of `tries` tries of a letter go wrong
/// when the letter's misses in `letter` fall at random over all its tries:
/// how often a pair no harder than its letter does as badly. Fisher's exact
/// test, one-sided. Counts that cannot come from one tally give a chance of
/// one: nothing to go on.
fn chance_of_as_many(wrong: u32, tries: u32, letter: CharTally) -> f64 {
    let (all, missed) = (letter.total, letter.wrong);
    let right = all.saturating_sub(missed);
    if tries > all || missed > all || wrong > missed.min(tries) || tries - wrong > right {
        return 1.0;
    }
    // ln C(n, k), a factor at a time.
    let ln_choose = |n: u32, k: u32| -> f64 {
        let k = k.min(n - k);
        (1..=k)
            .map(|i| (f64::from(n - k + i) / f64::from(i)).ln())
            .sum()
    };
    let mut p =
        (ln_choose(missed, wrong) + ln_choose(right, tries - wrong) - ln_choose(all, tries)).exp();
    let mut tail = 0.0;
    for k in wrong..=missed.min(tries) {
        tail += p;
        // From k misses among the tries to k + 1: the tries hold at least
        // `tries - wrong` of the letter's right ones, so none is short.
        let more = f64::from(missed - k) * f64::from(tries - k);
        let fewer = f64::from(k + 1) * f64::from(right + k + 1 - tries);
        p *= more / fewer;
    }
    tail.min(1.0)
}

/// The pairs that stand out ([`BigramCell::stands_out`]): worst first, by
/// how far over their letter they go wrong, then by how often missed.
fn hardest_pairs(cells: &[BigramCell], cols: &[char], overall: &[CharTally]) -> Vec<HardPair> {
    let mut hard: Vec<HardPair> = cells
        .iter()
        .filter(|cell| cell.stands_out)
        .filter_map(|cell| {
            let at = cols.iter().position(|c| *c == cell.col)?;
            Some(HardPair {
                row: cell.row,
                col: cell.col,
                wrong: cell.wrong,
                total: cell.total,
                rate: cell.rate,
                overall: overall.get(at)?.rate(),
            })
        })
        .collect();
    hard.sort_by(|a, b| {
        (b.rate - b.overall)
            .total_cmp(&(a.rate - a.overall))
            .then(b.wrong.cmp(&a.wrong))
    });
    hard.truncate(HARD_PAIRS_SHOWN);
    hard
}

/// How often each character actually turns up, measured by running the real
/// callsign generator rather than modelling it. Nothing else can answer this:
/// the share of `Q` depends on the prefix tables and the tier, not on a weight.
fn whole_group_character_shares(
    settings: &TrainingSettings,
    state: &CharSamplingState,
    config: &CharSamplingConfig,
    pool: &[char],
    rng: &mut impl crate::rng::Rng,
) -> BTreeMap<char, f64> {
    const DRAWS: usize = 1_000;
    let mut counts: BTreeMap<char, f64> = pool.iter().map(|c| (*c, 0.0)).collect();
    let mut total = 0.0;
    let mut sampling = state.clone();
    for _ in 0..DRAWS {
        let call = match settings.curriculum.char_set_mode {
            CharSetMode::Words => generate_word_group(settings, &mut sampling, config, rng),
            // A contest's callers' calls; their exchanges are not drawn.
            CharSetMode::Contest => generate_contest_call(settings, &mut sampling, config, rng),
            _ => generate_callsign_group(settings, &mut sampling, config, rng),
        };
        for ch in call.chars() {
            if let Some(count) = counts.get_mut(&ch) {
                *count += 1.0;
                total += 1.0;
            }
        }
    }
    if total <= 0.0 {
        return counts;
    }
    for count in counts.values_mut() {
        *count /= total;
    }
    counts
}

/// What the sampler makes of each character: the chance it is drawn next,
/// and what the history says of how often it is missed. Every character the
/// history knows is listed, whatever set it was practised in, those the
/// current set does not draw after the rest: no setting hides what was
/// learned.
pub fn sampling_rows(settings: &TrainingSettings, sessions: &[SessionResult]) -> Vec<SamplingRow> {
    let state = CharSamplingState::from_history(sessions);
    let pool = compute_char_pool(settings);
    let config = config_from_settings(settings);
    let mut rng = crate::rng::SplitMix64::default();
    let probs = if !settings.curriculum.char_set_mode.draws_characters() {
        // Callsigns and words are not drawn character by character, so a
        // per-character weight would not describe them. Ask the real
        // generator instead.
        whole_group_character_shares(settings, &state, &config, &pool, &mut rng)
    } else {
        let weights = compute_raw_sampling_weights(&pool, &state, &config);
        pool.iter()
            .copied()
            .zip(draw_chances(&pool, &weights, &config))
            .collect()
    };
    let tries: BTreeMap<char, u32> = letter_tallies(sessions)
        .into_iter()
        .map(|tally| (tally.letter, tally.total))
        .collect();
    // Every character ever heard, from the tallies, which read every
    // session: the beliefs read only the newest.
    let mut others: Vec<char> = tries
        .keys()
        .copied()
        .filter(|character| !pool.contains(character))
        .collect();
    // In the order Koch training unlocks them, then the rest.
    others.sort_by_key(|character| {
        let unlocked = LCWO_SEQUENCE.iter().position(|each| each == character);
        (unlocked.unwrap_or(usize::MAX), *character)
    });
    let row = |character: char, drawn_now: bool| {
        let belief = belief_for(&state, character);
        SamplingRow {
            tries: tries.get(&character).copied().unwrap_or(0),
            character,
            p_error: beta_posterior_mean_error(belief),
            sampling_prob: probs.get(&character).copied().unwrap_or(0.0),
            alpha: belief.alpha,
            beta: belief.beta,
            is_letter: !character.is_ascii_digit(),
            drawn_now,
        }
    };
    pool.iter()
        .map(|character| row(*character, true))
        .chain(others.into_iter().map(|character| row(character, false)))
        .collect()
}

pub fn session_history(sessions: &[SessionResult]) -> Vec<SessionHistoryRow> {
    history_order(sessions)
        .into_iter()
        .filter_map(|index| history_row(sessions, index))
        .collect()
}

/// Where each session of `sessions` sits, in the order [`session_history`]
/// lists them: newest first, and of two that finished at the same moment,
/// the one stored first. Only the stamps are read, so a list can make the
/// rows it shows and no others ([`history_row`]).
pub fn history_order(sessions: &[SessionResult]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..sessions.len()).collect();
    order.sort_by_key(|&index| std::cmp::Reverse(sessions[index].timestamp));
    order
}

/// The row of the session at `index` in `sessions`, as [`session_history`]
/// lists it: its run's log read and checked, where it has one.
pub fn history_row(sessions: &[SessionResult], index: usize) -> Option<SessionHistoryRow> {
    let s = sessions.get(index)?;
    Some(SessionHistoryRow {
        date: s.date.clone(),
        accuracy_pct: s.accuracy * 100.0,
        score: s.score,
        groups: s.groups.len() as u32,
        correct_groups: s.groups.iter().filter(|g| g.correct).count() as u32,
        total_chars: s.total_chars,
        timestamp: s.timestamp,
        mode: s.char_set_mode,
        contest: s.contest_kind(),
        log: s.stored_log(),
        assisted: s.assisted(),
        index,
    })
}

/// The character sets `sessions` hold any practice in, in the order the
/// settings list them: whatever is chosen now, each keeps its history.
pub fn practised_modes(sessions: &[SessionResult]) -> Vec<CharSetMode> {
    CharSetMode::ALL
        .into_iter()
        .filter(|mode| {
            sessions
                .iter()
                .any(|session| session.char_set_mode == *mode && !session.groups.is_empty())
        })
        .collect()
}

/// Your copy span after each session of the character set `settings`
/// trains, oldest first. Empty for callsigns, which are as long as they are.
pub fn span_history(settings: &TrainingSettings, sessions: &[SessionResult]) -> Vec<EstimatePoint> {
    SpanBelief::history(sessions, settings)
}

/// Your copy speed after each session of the character set `settings`
/// trains, oldest first: the speed at which you copy the share of characters
/// their "Aim for" asks, in WPM.
pub fn speed_history(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
) -> Vec<EstimatePoint> {
    SpeedBelief::history(sessions, settings, settings.curriculum.aim.chars_right())
}

/// Each group length the next session can send: how likely it is to come
/// next, and how often a group that long is copied whole. With adaptive group
/// size off, every length is as likely as the next. Empty for callsigns and
/// words, which are as long as they are.
pub fn group_length_forecast(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
) -> Vec<LengthForecast> {
    if !settings.curriculum.char_set_mode.draws_characters() {
        return Vec::new();
    }
    let (shortest, longest) = group_size_bounds(settings);
    let target = adaptive_bounds(settings).map(|_| settings.curriculum.aim.groups_whole());
    SpanBelief::from_history(sessions, settings).forecast(shortest, longest, target)
}

/// Round speeds across the speed settings, and for each how likely the next
/// group is to be sent at about that speed, and the share of characters you
/// copy right at it. With adaptive speed off, the speed is the station's to
/// draw, evenly across the range.
pub fn speed_forecast(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
) -> Vec<SpeedForecast> {
    let (low, high) = SpeedPath::of(settings).bounds();
    let speeds = forecast_speeds(low, high);
    let belief = SpeedBelief::from_history(sessions, settings);
    let drawn = if adapts_speed(settings) {
        belief.drawn_near(&speeds, low, high, settings.curriculum.aim.chars_right())
    } else {
        station_shares(settings, &speeds)
    };
    speeds
        .into_iter()
        .zip(drawn)
        .map(|(wpm, drawn)| {
            let right = belief.right_at(wpm);
            SpeedForecast {
                wpm,
                drawn,
                right: right.mean,
                right_sd: right.sd,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alignment::{CopyTally, LetterAccuracy};
    use crate::recognition::CharTimes;
    use crate::session::{GroupResult, SessionTiming};
    use crate::settings::CharSetMode;

    fn session() -> SessionResult {
        SessionResult {
            timestamp: 1,
            groups: vec![GroupResult {
                sent: "KM".into(),
                received: "K?".into(),
                correct: false,
            }],
            group_timings: vec![SessionTiming {
                time_to_complete_ms: 400.0,
                char_wpm: None,
                effective_wpm: None,
                sends: 1,
                ..SessionTiming::default()
            }],
            accuracy: 0.5,
            tally: CopyTally::of("KM", "K?"),
            avg_response_ms: 400.0,
            total_chars: 2,
            score: 1.0,
            ..SessionResult::empty_for_test()
        }
    }

    /// A group of one character, its sound ending 300 ms into its sending,
    /// keyed `took` ms after that: one timed try.
    fn keyed_after(took: u32) -> CharTimes {
        CharTimes {
            ends: vec![300],
            keys: vec![Some(300 + took)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: false,
        }
    }

    /// Every mode's timed characters count, a contest run's among them; a
    /// run's letters count for accuracy whether timed or not.
    #[test]
    fn every_mode_counts_towards_the_times() {
        let mut calls = plain("2026-09-01", 1, &[("K", "K"); 3]);
        calls.char_set_mode = CharSetMode::Callsign;
        for timing in &mut calls.group_timings {
            timing.char_times = Some(keyed_after(300));
        }
        let mut run = plain("2026-09-02", 2, &[("K", "K"), ("K", "K")]);
        run.char_set_mode = CharSetMode::Contest;
        run.group_timings[0].char_times = Some(keyed_after(500));
        let k = |history: &[SessionResult]| {
            character_diagnostics(history)
                .into_iter()
                .find(|diag| diag.letter == 'K')
                .expect("K")
        };
        let both = k(&[calls.clone(), run]);
        assert_eq!(both.recognition_tries, 4);
        assert_eq!(both.total, k(std::slice::from_ref(&calls)).total + 2);
    }

    /// A run with Show copy on is assisted, and counts in every stat all
    /// the same: the calendar, accuracy, Letters, Mistakes and copy speed.
    #[test]
    fn an_assisted_run_counts_in_every_stat() {
        use crate::contest::bot::script::{Habit, run_settings, work};
        use crate::contest::bot::{Operator, Runtime};
        let mut settings = run_settings(25.0);
        settings.contest.contest_minutes = 2;
        settings.contest.contest_show_copy = true;
        let sampler = crate::Sampler::new(&[], &settings, 4);
        let mut runtime = Runtime::start(4, settings.clone(), sampler);
        // A digit off in every number: mistakes to count.
        work(&mut runtime, &mut Operator::new(Habit::DigitOff), &[]);
        let run = runtime.machine.result(2, "2026-10-20".into());
        assert!(run.assisted(), "{}", run.alphabet_fingerprint);
        let history = [run.clone()];
        let days = crate::heatmap::aggregate_heatmap(&history);
        assert_eq!(days.get("2026-10-20").map(|day| day.sessions), Some(1));
        assert_eq!(accuracy_chart(&history).len(), 1);
        let letters = character_diagnostics(&history);
        let counted: u32 = letters.iter().map(|letter| letter.total).sum();
        let sent: u32 = run.tally.sent.values().map(|letter| letter.total).sum();
        assert_eq!(counted, sent, "every character it sent");
        assert!(
            mistake_stats(&history, 8)
                .confusions
                .iter()
                .any(|confusion| confusion.sent.is_ascii_digit()),
            "the digits typed wrong"
        );
        assert_eq!(speed_history(&settings, &history).len(), 1);
        let row = &session_history(&history)[0];
        assert_eq!(row.contest, Some(ContestKind::Wpx));
        assert!(row.assisted, "History says so");
    }

    #[test]
    fn diagnostics_and_confusion() {
        let sessions = [session()];
        let diag = character_diagnostics(&sessions);
        assert_eq!(diag.len(), 2);
        assert!(!mistake_stats(&sessions, 8).confusions.is_empty());
    }

    fn plain(date: &str, timestamp: u64, groups: &[(&str, &str)]) -> SessionResult {
        let mut out = session();
        out.date = date.to_string();
        out.timestamp = timestamp;
        out.tally = CopyTally::default();
        out.groups = groups
            .iter()
            .map(|(sent, received)| GroupResult {
                sent: (*sent).to_string(),
                received: (*received).to_string(),
                correct: sent == received,
            })
            .collect();
        out.group_timings = groups
            .iter()
            .map(|(sent, _)| SessionTiming {
                time_to_complete_ms: 1000.0 * sent.chars().count() as f64,
                char_wpm: Some(20.0),
                effective_wpm: None,
                sends: 1,
                ..SessionTiming::default()
            })
            .collect();
        out
    }

    #[test]
    fn letter_accuracy_falls_back_to_the_stored_groups() {
        // Sessions written before letter_accuracy existed still show per-letter rows.
        let legacy = plain("2026-09-01", 1, &[("KM", "KX")]);
        let diag = character_diagnostics(&[legacy]);
        let m = diag.iter().find(|d| d.letter == 'M').expect("M row");
        assert_eq!((m.correct, m.total), (0, 1));
        let k = diag.iter().find(|d| d.letter == 'K').expect("K row");
        assert_eq!((k.correct, k.total), (1, 1));
    }

    /// A session whose lengths adapted has its groups aligned again to be
    /// read ([`SessionResult::letter_accuracy`]): once each for the
    /// mastered count, the Letters card and the sampler's beliefs, not
    /// once to find whether it copied anything and again to count it.
    #[test]
    fn a_history_is_aligned_once_a_group_to_read_its_characters() {
        use crate::alignment::HELD_ALIGNMENTS;
        let copies = [("KMRS", "KMR"), ("KM", "KX")].repeat(4);
        let history: Vec<SessionResult> = (0..6)
            .map(|at| {
                let mut adaptive = plain("2026-09-01", at, &copies);
                adaptive.length_floor = Some(2);
                adaptive
            })
            .collect();
        let groups = history.len() * copies.len();
        let aligned = |read: &dyn Fn()| {
            HELD_ALIGNMENTS.set(0);
            read();
            HELD_ALIGNMENTS.get()
        };
        assert_eq!(
            aligned(&|| {
                mastered_count(&history);
            }),
            groups
        );
        assert_eq!(
            aligned(&|| {
                character_diagnostics(&history);
            }),
            groups
        );
        assert_eq!(
            aligned(&|| {
                CharSamplingState::from_history(&history);
            }),
            groups
        );
    }

    #[test]
    fn mastery_needs_both_attempts_and_accuracy() {
        assert_eq!(classify_mastery(1.0, 4), MasteryStatus::FewTries);
        assert_eq!(
            classify_mastery(MASTERED_MIN_ACCURACY, MASTERED_MIN_ATTEMPTS),
            MasteryStatus::Mastered
        );
        assert_eq!(
            classify_mastery(BUILDING_MIN_ACCURACY, MASTERED_MIN_ATTEMPTS),
            MasteryStatus::Building
        );
        assert_eq!(classify_mastery(0.1, 50), MasteryStatus::Weak);
    }

    #[test]
    fn diagnostics_rank_weak_letters_first() {
        // Ten sends of each letter: K always right, M always wrong.
        let groups: Vec<(&str, &str)> = vec![("KM", "KX"); 10];
        let session = plain("2026-09-01", 1, &groups);
        let diag = character_diagnostics(&[session]);
        assert_eq!(diag.first().map(|d| d.letter), Some('M'));
        let k = diag.iter().find(|d| d.letter == 'K').expect("K row");
        assert_eq!(k.status, MasteryStatus::Mastered);
        let m = diag.iter().find(|d| d.letter == 'M').expect("M row");
        assert_eq!(m.status, MasteryStatus::Weak);
    }

    /// A character's recognition time is the median of its latest timed
    /// tries, put in a band once there are enough of them, and instant only
    /// once it is copied right often enough. Untimed groups, older saves',
    /// count for nothing.
    #[test]
    fn recognition_is_the_median_of_the_latest_timed_tries() {
        let timed = |date: &str, timestamp: u64, took: &[Option<u32>]| {
            let groups: Vec<(&str, &str)> = vec![("K", "K"); took.len()];
            let mut session = plain(date, timestamp, &groups);
            for (timing, took) in session.group_timings.iter_mut().zip(took) {
                timing.char_times = took.map(keyed_after);
            }
            session
        };
        let few = timed("2026-09-01", 1, &[Some(300); 7]);
        let k = |history: &[SessionResult]| {
            character_diagnostics(history)
                .into_iter()
                .find(|d| d.letter == 'K')
                .expect("K")
        };
        let bands = RecognitionBands::default();
        let early = k(std::slice::from_ref(&few));
        assert_eq!(
            early.recognition(bands),
            Recognition::Provisional {
                ms: 300.0,
                tries: 7
            },
            "seven is too few to say for sure"
        );
        let slow = timed(
            "2026-09-02",
            2,
            &[Some(900), Some(800), None, Some(700), Some(1_000)],
        );
        let both = k(&[few, slow]);
        // 300 seven times and 700, 800, 900, 1000: the middle is 300.
        assert_eq!(
            both.recognition(bands),
            Recognition::Banded {
                ms: 300.0,
                band: crate::RecognitionBand::Fast,
                instant: true,
            }
        );
        // Copied wrong more often than right: fast, but not instant.
        let weak = CharacterDiagnostic {
            status: MasteryStatus::Weak,
            ..both
        };
        assert!(matches!(
            weak.recognition(bands),
            Recognition::Banded { instant: false, .. }
        ));
        let untimed = plain("2026-09-03", 3, &[("M", "M")]);
        let m = character_diagnostics(&[untimed])
            .into_iter()
            .find(|d| d.letter == 'M')
            .expect("M");
        assert_eq!(m.recognition(bands), Recognition::Untimed);
    }

    /// The latest tries are the latest groups of the latest session: a
    /// session is read from its end.
    #[test]
    fn recognition_reads_a_session_from_its_latest_group() {
        let groups: Vec<(&str, &str)> = vec![("K", "K"); 41];
        let mut session = plain("2026-09-01", 1, &groups);
        for (at, timing) in session.group_timings.iter_mut().enumerate() {
            timing.char_times = Some(keyed_after(if at < 20 { 900 } else { 300 }));
        }
        let k = character_diagnostics(&[session])
            .into_iter()
            .find(|d| d.letter == 'K')
            .expect("K");
        assert_eq!(k.recognition_tries, 41);
        // The latest forty: 300 twenty-one times, 900 nineteen.
        assert_eq!(k.recognition_ms, Some(300.0));
    }

    /// A session that timed little says what it kept of why: groups the
    /// audio held up or sent again, groups typed after they were sent and
    /// how late their first key came, and groups keyed only once heard
    /// again.
    #[test]
    fn a_session_says_why_its_groups_went_untimed() {
        let mut session = plain(
            "2026-09-01",
            1,
            &[
                ("KM", "KM"),
                ("KM", "KM"),
                ("KMR", "KMR"),
                ("RS", "RS"),
                ("RS", "RS"),
                ("K", "K"),
            ],
        );
        let after = |first: u32| CharTimes {
            ends: vec![300, 700],
            keys: vec![Some(first), Some(first + 200)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        let timings = &mut session.group_timings;
        // Typed 400 and 800 ms after the group ended, and along with it.
        timings[0].char_times = Some(after(1_100));
        timings[1].char_times = Some(after(1_500));
        timings[2].char_times = Some(CharTimes {
            ends: vec![300, 700, 1_000],
            keys: vec![Some(550), Some(900), Some(1_250)],
            ..after(0)
        });
        timings[3].untimed = Some(Untimed::HeldUp);
        timings[4].sends = 2;
        timings[5].untimed = Some(Untimed::SentAgain);
        let untimed = session_untimed(&session);
        assert_eq!(
            untimed,
            SessionUntimed {
                groups: 6,
                chars: 12,
                timed: 3,
                held_up: 2,
                typed_after: 2,
                first_key_late_ms: Some(600.0),
                keyed_in_repeat: 1,
            }
        );
        // A pause is no fault of the audio's.
        session.group_timings[3].untimed = Some(Untimed::Paused);
        assert_eq!(session_untimed(&session).held_up, 1);
        // With nothing kept, nothing to say.
        let plain = session_untimed(&plain("2026-09-02", 2, &[("KM", "KM")]));
        assert_eq!(
            (plain.timed, plain.typed_after, plain.first_key_late_ms),
            (0, 0, None)
        );
    }

    /// Overview's count of mastered characters is the Letters card's.
    #[test]
    fn the_mastered_count_is_the_letters_cards() {
        let history = [
            plain("2026-09-01", 1, &[("KM", "KM"); 6]),
            plain("2026-09-02", 2, &[("KM", "KX"); 6]),
        ];
        let mastered = character_diagnostics(&history)
            .into_iter()
            .filter(|d| d.status == MasteryStatus::Mastered)
            .count();
        assert_eq!(mastered_count(&history), mastered);
        assert_eq!(mastered, 1);
    }

    #[test]
    fn the_accuracy_chart_is_ordered_by_time() {
        let newest = plain("2026-09-03", 30, &[("KM", "KM")]);
        let oldest = plain("2026-09-01", 10, &[("KM", "KX")]);
        let chart = accuracy_chart(&[newest, oldest]);
        assert_eq!(
            chart.iter().map(|p| p.timestamp).collect::<Vec<_>>(),
            vec![10, 30]
        );
        assert_eq!(chart[0].date, "2026-09-01");
        assert_eq!(chart[0].accuracy_pct, 50.0);
        assert!(accuracy_chart(&[]).is_empty());
    }

    /// A session of `groups` groups, `chars` characters in all, copied at
    /// `accuracy`.
    fn sized(groups: usize, chars: u32, accuracy: f64) -> SessionResult {
        SessionResult {
            groups: vec![GroupResult::default(); groups],
            total_chars: chars,
            accuracy,
            ..SessionResult::empty_for_test()
        }
    }

    /// The Overview's accuracy is that of every character sent, so a
    /// session of one group missed is one group of it, not half; and its
    /// best is a session long enough to be on the board, so one group
    /// copied right in a session ended early is never the best.
    #[test]
    fn a_session_counts_for_its_size_in_the_overview() {
        let short = sized(1, 2, 0.0);
        let long = sized(50, 100, 0.96);
        let average = overall_accuracy(&[short, long.clone()]).expect("an average");
        assert!((average - 96.0 / 102.0).abs() < 1e-9, "{average}");
        let lucky = sized(1, 2, 1.0);
        assert_eq!(best_accuracy(&[lucky.clone(), long.clone()]), Some(0.96));
        assert_eq!(best_accuracy(std::slice::from_ref(&lucky)), None);
        assert_eq!(overall_accuracy(&[]), None);
        // A broken accuracy is left out of both.
        let broken = sized(20, 40, f64::NAN);
        assert_eq!(overall_accuracy(std::slice::from_ref(&broken)), None);
        assert_eq!(best_accuracy(&[broken, long]), Some(0.96));
    }

    #[test]
    fn bigrams_rank_the_group_start_first_and_unknown_characters_last() {
        // `+` is a Morse character the LCWO sequence never teaches; it must not
        // overflow the sort key.
        let session = plain("2026-09-01", 1, &[("K+", "KM")]);
        let heat = mistake_stats(&[session], 8).bigrams;
        assert_eq!(heat.rows, [GROUP_START_BIGRAM_TOKEN, 'K', '+']);
        // Nothing ends on the start of a group, so it has no column.
        assert_eq!(heat.cols, ['K', '+']);
        assert_eq!(heat.cells.len(), heat.rows.len() * heat.cols.len());
        // K→+ was copied as K→M: one attempt, one error.
        let cell = heat
            .cells
            .iter()
            .find(|c| c.row == 'K' && c.col == '+')
            .expect("K+ cell");
        assert_eq!((cell.total, cell.wrong), (1, 1));
        assert_eq!(cell.rate, 1.0);
        assert_eq!(cell.taken_for, [(Some('M'), 1)]);
        assert_eq!(heat.max_shade, 1.0, "+ is missed every time it is sent");
        // An empty history draws no grid at all.
        let empty = mistake_stats(&[], 8).bigrams;
        assert!(empty.rows.is_empty() && empty.cols.is_empty() && empty.cells.is_empty());
    }

    /// A pair says what its second character was taken for there, and each
    /// column how its character fares overall.
    #[test]
    fn a_pair_knows_what_it_was_taken_for_and_its_letter_overall() {
        let session = plain(
            "2026-09-01",
            1,
            &[
                ("KM", "KN"),
                ("KM", "KN"),
                ("KM", "K"),
                ("KM", "KM"),
                ("UM", "UM"),
            ],
        );
        let heat = mistake_stats(&[session], 8).bigrams;
        let km = heat
            .cells
            .iter()
            .find(|c| (c.row, c.col) == ('K', 'M'))
            .expect("KM");
        assert_eq!((km.wrong, km.total), (3, 4));
        assert_eq!(km.taken_for, [(Some('N'), 2), (None, 1)]);
        // M went wrong three times in five, wherever it came.
        assert_eq!(heat.overall_of('M'), CharTally { wrong: 3, total: 5 });
        assert_eq!(heat.overall_of('K'), CharTally { wrong: 0, total: 4 });
        assert_eq!(heat.overall_of('X'), CharTally::default());
        assert_eq!(CharTally::default().rate(), 0.0);
    }

    /// The hardest pairs are the ones that go wrong more often than their
    /// second character does overall, by more than chance would make it: a
    /// letter that is hard everywhere is the Letters tab's, not a pair's.
    #[test]
    fn the_hardest_pairs_are_harder_than_their_letter() {
        let mut groups = Vec::new();
        // M after K: wrong sixteen times in twenty. M after U: right thirty
        // times.
        groups.extend([("KM", "KN"); 16]);
        groups.extend([("KM", "KM"); 4]);
        groups.extend([("UM", "UM"); 30]);
        // R is hard everywhere, after K as after U: no pair of it stands out.
        // Nor does Z after W, missed every time, where Z goes wrong seven
        // times in ten anyway: that is not half as often again.
        groups.extend([("WZ", "WX"); 5]);
        groups.extend([("UZ", "UX"); 2]);
        groups.extend([("UZ", "UZ"); 3]);
        groups.extend([("KR", "KS"); 6]);
        groups.extend([("KR", "KR"); 4]);
        groups.extend([("UR", "US"); 6]);
        groups.extend([("UR", "UR"); 4]);
        // E after U: missed once in five, too seldom to tell.
        groups.push(("UE", "UT"));
        groups.extend([("UE", "UE"); 4]);
        // T after K: missed in two of five, and right ten times after U.
        // Luck alone does that one time in ten: too often to tell, over so
        // many pairs.
        groups.extend([("KT", "KX"); 2]);
        groups.extend([("KT", "KT"); 3]);
        groups.extend([("UT", "UT"); 10]);
        // S starting a group: missed in twelve of twenty, and fine after K.
        groups.extend([("SK", "TK"); 12]);
        groups.extend([("SK", "SK"); 8]);
        groups.extend([("KS", "KS"); 40]);
        let session = plain("2026-09-01", 1, &groups);
        let heat = mistake_stats(&[session], 8).bigrams;
        let pairs: Vec<(char, char)> = heat.hardest.iter().map(|p| (p.row, p.col)).collect();
        assert_eq!(pairs, [('K', 'M'), (GROUP_START_BIGRAM_TOKEN, 'S')]);
        let km = &heat.hardest[0];
        assert_eq!((km.wrong, km.total), (16, 20));
        assert!((km.rate - 0.8).abs() < 1e-9);
        assert!((km.overall - 16.0 / 50.0).abs() < 1e-9);
        let kt = heat
            .cells
            .iter()
            .find(|c| (c.row, c.col) == ('K', 'T'))
            .expect("KT");
        assert!(!kt.stands_out, "{kt:?}");
        // Nothing heard yet, nothing hard.
        assert!(mistake_stats(&[], 8).bigrams.hardest.is_empty());
    }

    /// The chance a pair does as badly as it did with no pair any harder
    /// than its letter: Fisher's exact test.
    #[test]
    fn the_chance_of_as_many_misses_is_fishers() {
        let letter = |wrong, total| CharTally { wrong, total };
        // Two misses of fifteen, both in one pair of five: C(13,3)/C(15,5).
        let chance = chance_of_as_many(2, 5, letter(2, 15));
        assert!((chance - 286.0 / 3003.0).abs() < 1e-12, "{chance}");
        // Every miss in the pair, or none needed: certain.
        assert!((chance_of_as_many(0, 5, letter(2, 15)) - 1.0).abs() < 1e-12);
        // One miss of two among the tries, of four: 1 - C(2,2)/C(4,2).
        assert!((chance_of_as_many(1, 2, letter(2, 4)) - 5.0 / 6.0).abs() < 1e-12);
        // Counts no one tally could give say nothing.
        assert_eq!(chance_of_as_many(3, 5, letter(2, 15)), 1.0);
        assert_eq!(chance_of_as_many(1, 20, letter(2, 15)), 1.0);
    }

    /// A learner whose misses do not depend on the character before is
    /// almost never shown a hardest pair, however many pairs they have
    /// heard; a pair that really is harder is found. Each square is shaded
    /// by its tries, so one heard once and missed is not drawn darkest.
    #[test]
    fn a_learner_with_no_hard_pair_is_almost_never_shown_one() {
        use crate::rng::{Rng, SplitMix64};
        const LETTERS: [char; 26] = [
            'K', 'M', 'U', 'R', 'E', 'S', 'N', 'A', 'P', 'T', 'L', 'W', 'I', 'J', 'Z', 'F', 'O',
            'Y', 'V', 'G', 'Q', 'B', 'C', 'D', 'H', 'X',
        ];
        // `sessions` of twenty groups of three of the first `letters`, each
        // character missed at the chance `missed` gives it after the one
        // before.
        let history = |rng: &mut SplitMix64,
                       sessions: u64,
                       letters: usize,
                       missed: &dyn Fn(char, char) -> f64| {
            (0..sessions)
                .map(|timestamp| {
                    let groups: Vec<(String, String)> = (0..20)
                        .map(|_| {
                            let sent: String = (0..3)
                                .map(|_| LETTERS[rng.usize_in(0, letters - 1)])
                                .collect();
                            let mut before = GROUP_START_BIGRAM_TOKEN;
                            let received = sent
                                .chars()
                                .map(|ch| {
                                    let wrong = rng.f64() < missed(before, ch);
                                    before = ch;
                                    if wrong { '5' } else { ch }
                                })
                                .collect();
                            (sent, received)
                        })
                        .collect();
                    let groups: Vec<(&str, &str)> = groups
                        .iter()
                        .map(|(sent, received)| (sent.as_str(), received.as_str()))
                        .collect();
                    plain("2026-09-01", timestamp, &groups)
                })
                .collect::<Vec<_>>()
        };
        let mut rng = SplitMix64::new(63);
        for (sessions, letters) in [(10, 10), (30, 15), (100, 26)] {
            let shown = (0..40)
                .filter(|_| {
                    let sessions = history(&mut rng, sessions, letters, &|_, _| 0.1);
                    !mistake_stats(&sessions, 8).bigrams.hardest.is_empty()
                })
                .count();
            assert!(
                shown <= 4,
                "{shown} of 40, {sessions} sessions of {letters}"
            );
        }
        let hard = |before: char, ch: char| if (before, ch) == ('K', 'M') { 0.7 } else { 0.1 };
        let found = (0..20)
            .filter(|_| {
                let heat = mistake_stats(&history(&mut rng, 100, 15, &hard), 8).bigrams;
                heat.hardest.first().map(|pair| (pair.row, pair.col)) == Some(('K', 'M'))
            })
            .count();
        assert!(found >= 18, "{found} of 20");
        // A pair heard once and missed is shaded near its letter's usual.
        let once = plain("2026-09-01", 1, &[("KM", "KX"), ("UM", "UM"), ("UM", "UM")]);
        let heat = mistake_stats(&[once], 8).bigrams;
        let km = heat
            .cells
            .iter()
            .find(|c| (c.row, c.col) == ('K', 'M'))
            .expect("KM");
        assert_eq!(km.rate, 1.0);
        assert!((km.shade - (1.0 + 5.0 / 3.0) / 6.0).abs() < 1e-12, "{km:?}");
    }

    #[test]
    fn confusion_is_ordered_and_capped() {
        let session = plain(
            "2026-09-01",
            1,
            &[("KMU", "XMU"), ("KMU", "XMU"), ("KMU", "KMR")],
        );
        let rows = mistake_stats(&[session], 8).confusions;
        assert_eq!(rows[0].sent, 'K');
        assert_eq!(rows[0].typed, Some('X'));
        assert_eq!(rows[0].count, 2);
        // Two of the three K sends were copied as X.
        assert!((rows[0].percentage - 200.0 / 3.0).abs() < 1e-9);
        assert!(mistake_stats(&[], 8).confusions.is_empty());
        // The limit is honoured.
        let long = plain("2026-09-02", 2, &[("KMURE", "XXXXX")]);
        assert_eq!(mistake_stats(&[long], 2).confusions.len(), 2);
    }

    #[test]
    fn a_dropped_character_is_reported_as_no_answer() {
        let session = plain("2026-09-01", 1, &[("KM", "K")]);
        let rows = mistake_stats(&[session], 8).confusions;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sent, 'M');
        assert_eq!(rows[0].typed, None);
    }

    #[test]
    fn characters_that_were_never_sent_are_left_out_of_every_table() {
        // An extra character was typed in the middle of the group.
        let session = plain("2026-09-01", 1, &[("KM", "KXM")]);
        let diagnostics = character_diagnostics(std::slice::from_ref(&session));
        assert!(diagnostics.iter().all(|d| d.letter != 'X'));
        let mistakes = mistake_stats(&[session], 8);
        assert!(!mistakes.bigrams.rows.contains(&'X'));
        assert!(!mistakes.bigrams.cols.contains(&'X'));
        assert!(mistakes.confusions.iter().all(|c| c.sent != 'X'));
    }

    /// A character never heard has no evidence; one heard has the weight of
    /// what was heard.
    #[test]
    fn a_sampling_row_knows_how_much_was_heard() {
        let mut row = SamplingRow {
            character: 'K',
            p_error: 0.5,
            sampling_prob: 0.1,
            alpha: CHAR_SAMPLING_PRIOR_ALPHA,
            beta: CHAR_SAMPLING_PRIOR_BETA,
            is_letter: true,
            drawn_now: true,
            tries: 0,
        };
        assert_eq!(row.evidence(), 0.0);
        // Never heard: even everywhere, its likely range most of the way.
        let flat = rate_density(row.beta, row.alpha, 100);
        assert!(flat.iter().all(|d| (d - 1.0).abs() < 1e-9));
        let (low, high) = row.error_interval(0.8);
        assert!((low - 0.1).abs() < 0.002 && (high - 0.9).abs() < 0.002);
        row.alpha += 3.0;
        row.beta += 1.0;
        assert_eq!(row.evidence(), 4.0);
    }

    /// A character copied 190 times in 200 is believed wrong about one time
    /// in twenty, within a few points: a peak there, high and narrow, the
    /// density taking in one over the range.
    #[test]
    fn a_well_known_character_has_a_narrow_likely_range() {
        let row = SamplingRow {
            character: 'K',
            p_error: 11.0 / 202.0,
            sampling_prob: 0.1,
            alpha: CHAR_SAMPLING_PRIOR_ALPHA + 190.0,
            beta: CHAR_SAMPLING_PRIOR_BETA + 10.0,
            is_letter: true,
            drawn_now: true,
            tries: 0,
        };
        let density = rate_density(row.beta, row.alpha, 1_000);
        let area = density.iter().sum::<f64>() / 1_000.0;
        assert!((area - 1.0).abs() < 1e-9);
        let peak = density
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(step, _)| (step as f64 + 0.5) / 1_000.0)
            .expect("a peak");
        assert!((peak - 0.05).abs() < 0.002, "peak at {peak}");
        let (low, high) = row.error_interval(0.8);
        assert!(low < row.p_error && row.p_error < high);
        assert!((0.03..0.045).contains(&low), "{low}");
        assert!((0.065..0.08).contains(&high), "{high}");
        // More of the belief is a wider range.
        let (wide_low, wide_high) = row.error_interval(0.95);
        assert!(wide_low < low && wide_high > high);
    }

    /// A character's recent tries are its latest sessions' until there are
    /// twenty, and its earlier ones those before them; both are needed for a
    /// trend.
    #[test]
    fn a_character_knows_which_way_it_is_going() {
        // Ten tries of K a session: an early run half right, then clean.
        let half: Vec<(&str, &str)> = [("K", "K"), ("K", "X")].repeat(5);
        let clean = vec![("K", "K"); 10];
        let sessions = [
            plain("2026-09-01", 10, &half),
            plain("2026-09-02", 20, &clean),
            plain("2026-09-03", 30, &clean),
        ];
        let k = |sessions: &[SessionResult]| {
            character_diagnostics(sessions)
                .into_iter()
                .find(|d| d.letter == 'K')
                .expect("K")
        };
        let now = k(&sessions);
        assert_eq!(now.recent_pct, Some(100.0));
        assert_eq!(now.earlier_pct, Some(50.0));
        assert_eq!((now.recent_tries, now.earlier_tries), (20, 10));
        // Twenty tries in all: recent, and nothing earlier to compare with.
        let young = k(&sessions[1..]);
        assert_eq!((young.recent_pct, young.earlier_pct), (None, None));
        // The likely range takes in what was measured.
        let (low, high) = now.accuracy_interval(0.8);
        assert!(low < now.accuracy_pct / 100.0 && now.accuracy_pct / 100.0 < high);
        // From half right to clean over ten and twenty tries is a move.
        assert_eq!(now.moved_lately(), Some(50.0));
    }

    /// A character copied right a steady nine times in ten is almost never
    /// said to have moved lately; one put right long ago is not said to be
    /// improving still; one that has slipped is said to be going down.
    #[test]
    fn a_steady_character_is_almost_never_said_to_move_lately() {
        use crate::rng::{Rng, SplitMix64};
        const LETTERS: [char; 15] = [
            'K', 'M', 'U', 'R', 'E', 'S', 'N', 'A', 'P', 'T', 'L', 'W', 'I', 'J', 'Z',
        ];
        let mut rng = SplitMix64::new(65);
        let (mut rows, mut moved) = (0, 0);
        for _ in 0..20 {
            let sessions: Vec<SessionResult> = (0..40)
                .map(|timestamp| {
                    let groups: Vec<(String, String)> = (0..20)
                        .map(|_| {
                            let sent: String =
                                (0..3).map(|_| LETTERS[rng.usize_in(0, 14)]).collect();
                            let received = sent
                                .chars()
                                .map(|ch| if rng.f64() < 0.9 { ch } else { '5' })
                                .collect();
                            (sent, received)
                        })
                        .collect();
                    let groups: Vec<(&str, &str)> = groups
                        .iter()
                        .map(|(sent, received)| (sent.as_str(), received.as_str()))
                        .collect();
                    plain("2026-09-01", timestamp, &groups)
                })
                .collect();
            for letter in character_diagnostics(&sessions) {
                rows += 1;
                moved += u32::from(letter.moved_lately().is_some());
            }
        }
        assert!(moved * 100 <= rows * 3, "{moved} of {rows} rows moved");
        // Ten sessions of ten tries of K: `wrong` of them missed in each.
        let k = |missed: &[usize]| {
            let sessions: Vec<SessionResult> = missed
                .iter()
                .enumerate()
                .map(|(at, wrong)| {
                    let groups: Vec<(&str, &str)> = (0..10)
                        .map(|n| if n < *wrong { ("K", "X") } else { ("K", "K") })
                        .collect();
                    plain("2026-09-01", at as u64, &groups)
                })
                .collect();
            character_diagnostics(&sessions)
                .into_iter()
                .find(|d| d.letter == 'K')
                .expect("K")
        };
        // Four at 60%, then thirty at 95%: put right long ago.
        let mut righted = vec![4; 10];
        righted.extend([0, 1].repeat(15));
        let righted = k(&righted);
        assert_eq!(righted.earlier_tries, EARLIER_TRIES);
        assert_eq!(righted.moved_lately(), None, "{righted:?}");
        // Twenty at 95%, then two at 60%: slipping now.
        let mut slipped = [0, 1].repeat(10);
        slipped.extend([4, 4]);
        let slipped = k(&slipped).moved_lately().expect("a move");
        assert!((slipped + 35.0).abs() < 1e-9, "{slipped}");
    }

    #[test]
    fn history_is_newest_first() {
        let a = plain("2026-09-01", 10, &[("KM", "KM"), ("KM", "KX")]);
        let b = plain("2026-09-02", 20, &[("KM", "KM")]);
        let rows = session_history(&[a, b]);
        assert_eq!(rows[0].date, "2026-09-02");
        assert_eq!(rows[1].groups, 2);
        assert_eq!(rows[1].correct_groups, 1);
        assert_eq!(rows[1].total_chars, 2);
        assert_eq!(rows[1].accuracy_pct, 50.0);
        assert!(session_history(&[]).is_empty());
        // Each row still names the session it came from, and its mode.
        assert_eq!((rows[0].index, rows[1].index), (1, 0));
        assert_eq!(rows[0].mode, CharSetMode::Koch);
    }

    /// The order alone reads only the stamps, and a row made from it is the
    /// row the whole history lists there: two sessions of one moment in the
    /// order they were stored.
    #[test]
    fn a_list_can_make_only_the_rows_it_shows() {
        let history = [
            plain("2026-09-01", 10, &[("KM", "KM")]),
            plain("2026-09-03", 30, &[("KM", "KX")]),
            plain("2026-09-02", 20, &[("KM", "KM")]),
            plain("2026-09-03", 30, &[("MK", "MK")]),
        ];
        let order = history_order(&history);
        assert_eq!(order, [1, 3, 2, 0]);
        let rows: Vec<SessionHistoryRow> = order
            .iter()
            .filter_map(|&index| history_row(&history, index))
            .collect();
        assert_eq!(rows, session_history(&history));
        assert_eq!(history_row(&history, 4), None);
    }

    #[test]
    fn sampling_rows_follow_the_pool_it_falls_back_to() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.mixed_letters_percent = 100;
        settings.curriculum.custom_sequence = vec!['0'];
        // A letters-only mixed pool over a digits-only sequence sends digits.
        let rows = sampling_rows(&settings, &[]);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| !row.is_letter));
    }

    #[test]
    fn sampling_rows_mark_digits_and_sum_to_one() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.level = 1;
        settings.curriculum.digits_level = 1;
        let rows = sampling_rows(&settings, &[]);
        assert!(rows.iter().any(|r| !r.is_letter));
        assert!(rows.iter().any(|r| r.is_letter));
        let total: f64 = rows.iter().map(|r| r.sampling_prob).sum();
        assert!((total - 1.0).abs() < 1e-9);
        // With no history every character starts on the same flat prior.
        assert!(rows.iter().all(|r| (r.p_error - 0.5).abs() < 1e-9));
    }

    #[test]
    fn callsign_sampling_shows_what_callsigns_actually_contain() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MAX;
        let rows = sampling_rows(&settings, &[]);

        let pool = compute_char_pool(&settings);
        assert_eq!(rows.len(), pool.len());
        let total: f64 = rows.iter().map(|row| row.sampling_prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-6,
            "the shares should account for every character sent, got {total}"
        );
        assert!(
            rows.iter().all(|row| row.sampling_prob > 0.0),
            "every character a callsign can contain should turn up"
        );
        // A share is measured, not modelled: the separator is a digit on every
        // single call, so digits cannot be the rare ones here.
        let digits: f64 = rows
            .iter()
            .filter(|row| row.character.is_ascii_digit())
            .map(|row| row.sampling_prob)
            .sum();
        assert!(digits > 0.1, "digits carry every separator, got {digits}");

        // A tier that sends no portables must never show a slash.
        let mut tier_one = settings;
        tier_one.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MIN;
        assert!(
            !sampling_rows(&tier_one, &[])
                .iter()
                .any(|row| row.character == '/'),
            "tier 1 has no portable calls"
        );
    }

    /// A contest's snapshot is of its callers' calls, drawn as the run draws
    /// them: few portables and stations abroad, where the top tier sends
    /// plenty for practice. Drawn plainly, with no lean on any character,
    /// the difference is the spec's alone.
    #[test]
    fn contest_sampling_shows_what_its_callers_send() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MAX;
        settings.auto_level.error_weight_strength = 0.0;
        settings.auto_level.char_sampling_coverage_strength = 0.0;
        let slash = |mode| {
            let mut settings = settings.clone();
            settings.curriculum.char_set_mode = mode;
            let rows = sampling_rows(&settings, &[]);
            let total: f64 = rows.iter().map(|row| row.sampling_prob).sum();
            assert!((total - 1.0).abs() < 1e-6, "{mode:?}: {total}");
            rows.iter()
                .find(|row| row.character == '/')
                .map_or(0.0, |row| row.sampling_prob)
        };
        let calls = slash(CharSetMode::Callsign);
        let callers = slash(CharSetMode::Contest);
        assert!(
            callers > 0.0,
            "a contest still hears a portable now and then"
        );
        assert!(callers < calls / 2.0, "{callers} against {calls}");
    }

    /// A character weak for its first weeks and copied clean ever since is
    /// mastered: its old misses fade as the sessions pass, halving every
    /// ten, as the span and the speed do. Counted as one sum, they held it
    /// at Building for good. Its tries are still every one it was sent, and
    /// a weakness now shows at once.
    #[test]
    fn a_weakness_put_right_long_ago_stops_flagging_a_character() {
        let practised = |timestamp: u64, sent: &str, received: &str| {
            let mut session = SessionResult {
                timestamp,
                ..SessionResult::empty_for_test()
            };
            session.tally.add(sent, received);
            session
        };
        let mut history: Vec<SessionResult> = (0..10)
            .map(|at| practised(at, "MMMMMMMMMM", "MMMMXXXXXX"))
            .collect();
        history.extend((10..50).map(|at| practised(at, "MMMMM", "MMMMM")));
        let m = |history: &[SessionResult]| {
            character_diagnostics(history)
                .into_iter()
                .find(|d| d.letter == 'M')
                .expect("M")
        };
        let now = m(&history);
        assert_eq!((now.correct, now.total), (240, 300));
        assert!(now.accuracy_pct > 95.0, "{}", now.accuracy_pct);
        assert_eq!(now.status, MasteryStatus::Mastered);
        assert_eq!(mastered_count(&history), 1);
        let (low, high) = now.accuracy_interval(0.8);
        assert!(low > 0.9 && high > low, "{low}–{high}");

        // The other way round, a recent slump shows though the history
        // before it is long and clean.
        history.extend((50..53).map(|at| practised(at, "MMMMM", "XXXXM")));
        let slump = m(&history);
        assert!(slump.accuracy_pct < 90.0, "{}", slump.accuracy_pct);
        assert_eq!(slump.status, MasteryStatus::Building);
        assert_eq!(mastered_count(&history), 0);
    }

    /// Mixed draws letters and digits by the share asked for before it
    /// weighs one character against another, and the chances shown are the
    /// ones it draws by: ten letters and two digits at 70% letters used to
    /// show the digits at two in twelve while the sampler drew them three
    /// times in ten. A digit missed often still leads its side.
    #[test]
    fn mixed_chances_shown_are_the_ones_drawn() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.level = 9;
        settings.curriculum.digits_level = 1;
        settings.curriculum.mixed_letters_percent = 70;
        settings.curriculum.min_group_size = 5;
        settings.curriculum.max_group_size = 5;
        // Coverage changes the weights as a session goes; the chances shown
        // are a fresh session's.
        settings.auto_level.char_sampling_coverage_strength = 0.0;
        let settings = settings.clamp();
        let pool = compute_char_pool(&settings);
        assert_eq!(
            (
                pool.iter().filter(|c| !c.is_ascii_digit()).count(),
                pool.iter().filter(|c| c.is_ascii_digit()).count()
            ),
            (10, 2)
        );
        let digit = *pool.iter().find(|c| c.is_ascii_digit()).expect("a digit");
        let mut missed = SessionResult::empty_for_test();
        missed.tally = CopyTally::of(&digit.to_string().repeat(8), "");
        let history = [missed];

        let rows = sampling_rows(&settings, &history);
        let shown = |keep: &dyn Fn(char) -> bool| -> f64 {
            rows.iter()
                .filter(|row| keep(row.character))
                .map(|row| row.sampling_prob)
                .sum()
        };
        assert!((shown(&|c| c.is_ascii_digit()) - 0.3).abs() < 1e-9);
        assert!((shown(&|_| true) - 1.0).abs() < 1e-9);

        let mut drawn: BTreeMap<char, f64> = BTreeMap::new();
        let mut sampler = crate::Sampler::new(&history, &settings, 7);
        let groups = 4_000;
        for _ in 0..groups {
            let (group, _) = crate::machine::GroupSource::next_group(&mut sampler, &settings);
            for c in group.chars() {
                *drawn.entry(c).or_default() += 1.0 / f64::from(groups * 5);
            }
        }
        for row in &rows {
            let seen = drawn.get(&row.character).copied().unwrap_or(0.0);
            assert!(
                (row.sampling_prob - seen).abs() < 0.01,
                "{}: shown {:.3}, drawn {seen:.3}",
                row.character,
                row.sampling_prob
            );
        }
        let chance = |c: char| {
            rows.iter()
                .find(|row| row.character == c)
                .map(|row| row.sampling_prob)
        };
        let other = *pool
            .iter()
            .rfind(|c| c.is_ascii_digit())
            .expect("two digits");
        assert!(chance(digit) > chance(other), "the digit missed comes more");
    }

    /// A character's history counts wherever it was practised: a K missed
    /// in a digits session is as missed in Koch.
    #[test]
    fn sampling_rows_count_every_char_set_mode() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        let mut koch = session();
        koch.char_set_mode = CharSetMode::Koch;
        koch.tally = CopyTally::default();
        koch.tally.sent.insert(
            'K',
            LetterAccuracy {
                correct: 10,
                total: 10,
            },
        );
        let mut digits = koch.clone();
        digits.char_set_mode = CharSetMode::Digits;
        digits.timestamp = koch.timestamp + 1;
        digits.tally.sent.insert(
            'K',
            LetterAccuracy {
                correct: 0,
                total: 10,
            },
        );
        let k_belief = |rows: &[SamplingRow]| {
            rows.iter()
                .find(|row| row.character == 'K')
                .map(|row| (row.alpha, row.beta, row.tries))
                .expect("K in pool")
        };
        let (alpha, beta, tries) = k_belief(&sampling_rows(&settings, &[koch.clone()]));
        let both = k_belief(&sampling_rows(&settings, &[koch, digits]));
        // The Koch session is one session back once the digits one follows.
        let faded = CHAR_SAMPLING_PRIOR_ALPHA
            + (alpha - CHAR_SAMPLING_PRIOR_ALPHA) * crate::belief::SESSION_FADE;
        assert!((both.0 - faded).abs() < 1e-12, "{both:?}");
        assert_eq!((both.1, both.2), (beta + 10.0, tries + 10));
    }

    /// A character the current set does not draw is still listed, after the
    /// rest, with what its history says and no chance of being drawn.
    #[test]
    fn characters_the_set_does_not_draw_are_still_listed() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        let mut practised = session();
        for (character, correct) in [('Q', 1), ('5', 8)] {
            practised
                .tally
                .sent
                .insert(character, LetterAccuracy { correct, total: 10 });
        }
        let rows = sampling_rows(&settings, &[practised]);
        let drawn: Vec<char> = rows
            .iter()
            .filter(|row| row.drawn_now)
            .map(|row| row.character)
            .collect();
        assert_eq!(drawn, compute_char_pool(&settings));
        let others: Vec<&SamplingRow> = rows.iter().filter(|row| !row.drawn_now).collect();
        assert_eq!(
            others.iter().map(|row| row.character).collect::<Vec<_>>(),
            ['5', 'Q'],
            "in the order Koch unlocks them"
        );
        assert!(others.iter().all(|row| row.sampling_prob == 0.0));
        assert!(others[1].p_error > 0.8, "Q is mostly missed");
        assert!(rows.iter().take_while(|row| row.drawn_now).count() == drawn.len());
    }

    /// A character heard only longer ago than the beliefs read back is
    /// listed all the same, with every try it had: its belief is back on
    /// the flat prior, as near as anything shows.
    #[test]
    fn a_character_heard_only_long_ago_is_still_listed() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        let mut long_ago = session();
        long_ago.tally.sent.insert(
            'Q',
            LetterAccuracy {
                correct: 2,
                total: 10,
            },
        );
        let since = (2..=crate::belief::FADE_HORIZON as u64 + 1).map(|timestamp| SessionResult {
            timestamp,
            ..session()
        });
        let history: Vec<SessionResult> = std::iter::once(long_ago).chain(since).collect();
        let rows = sampling_rows(&settings, &history);
        let q = rows
            .iter()
            .find(|row| row.character == 'Q')
            .expect("Q is listed");
        assert_eq!(q.tries, 10);
        assert!(!q.drawn_now);
        assert!((q.p_error - 0.5).abs() < 1e-9, "{q:?}");
    }

    /// Picking another unlock order — the guided setup does — keeps what the
    /// history says of each character: a K always missed under LCWO's order
    /// is still the hardest under MorseMania's.
    #[test]
    fn a_new_unlock_order_keeps_what_was_learned_of_each_character() {
        let mut lcwo = TrainingSettings::default();
        lcwo.curriculum.char_set_mode = CharSetMode::Koch;
        let mut missed = session();
        missed.alphabet_fingerprint = lcwo.alphabet_fingerprint();
        missed.tally.sent.insert(
            'K',
            LetterAccuracy {
                correct: 0,
                total: 20,
            },
        );
        let mut mania = lcwo.clone();
        mania.choose_sequence(crate::sequences::SequenceChoice::Preset(
            crate::sequences::PresetId::MorseMania,
        ));
        assert_ne!(lcwo.alphabet_fingerprint(), mania.alphabet_fingerprint());
        let rows = sampling_rows(&mania, &[missed]);
        let k = rows
            .iter()
            .find(|row| row.character == 'K')
            .expect("K in MorseMania's pool");
        assert!(k.p_error > 0.9, "{}", k.p_error);
        assert!(
            rows.iter()
                .all(|row| row.character == 'K' || row.sampling_prob < k.sampling_prob),
            "K is drawn most"
        );
    }

    fn length_settings(adaptive: bool) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 7;
        settings.curriculum.adaptive_group_size = adaptive;
        settings
    }

    #[test]
    fn with_adaptive_length_off_every_length_is_as_likely() {
        let rows = group_length_forecast(&length_settings(false), &[session()]);
        assert_eq!(
            rows.iter().map(|row| row.length).collect::<Vec<_>>(),
            [2, 3, 4, 5, 6, 7]
        );
        assert!(rows.iter().all(|row| (row.drawn - 1.0 / 6.0).abs() < 1e-12));
        // What is copied whole is still the model's to say.
        assert!(rows.windows(2).all(|pair| pair[0].whole > pair[1].whole));
    }

    #[test]
    fn with_adaptive_length_on_the_lengths_lean_and_still_add_up() {
        let rows = group_length_forecast(&length_settings(true), &[session()]);
        let total: f64 = rows.iter().map(|row| row.drawn).sum();
        assert!((total - 1.0).abs() < 1e-9);
        assert!(rows.iter().any(|row| (row.drawn - 1.0 / 6.0).abs() > 0.01));
    }

    fn speed_settings(adaptive: bool) -> TrainingSettings {
        let mut settings = length_settings(false);
        settings.playback.char_wpm_min = 10.0;
        settings.playback.char_wpm_max = 50.0;
        settings.playback.link_char_to_effective = true;
        settings.playback.adaptive_speed = adaptive;
        settings
    }

    #[test]
    fn with_adaptive_speed_off_the_station_draws_evenly() {
        let rows = speed_forecast(&speed_settings(false), &[session()]);
        assert_eq!(
            rows.iter().map(|row| row.wpm).collect::<Vec<_>>(),
            [10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0]
        );
        assert!((rows.iter().map(|row| row.drawn).sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(
            rows[1..8]
                .iter()
                .all(|row| (row.drawn - 0.125).abs() < 1e-9)
        );
        // What is copied right is still the model's to say.
        assert!(rows.windows(2).all(|pair| pair[0].right > pair[1].right));
    }

    #[test]
    fn with_adaptive_speed_on_the_speeds_lean_and_still_add_up() {
        let rows = speed_forecast(&speed_settings(true), &[session()]);
        assert!((rows.iter().map(|row| row.drawn).sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(rows.iter().any(|row| (row.drawn - 0.125).abs() > 0.05));
    }

    /// In Contest mode the forecast is the callers': drawn from your copy
    /// speed while their switch is on, whatever group training's says, and
    /// evenly across the range while it is off.
    #[test]
    fn a_contests_forecast_follows_the_callers_switch() {
        for group in [false, true] {
            let mut contest = speed_settings(group);
            contest.curriculum.char_set_mode = CharSetMode::Contest;
            // Drawn evenly, every speed between the ends is an eighth.
            let leans = |settings: &TrainingSettings| {
                let rows = speed_forecast(settings, &[]);
                assert!((rows.iter().map(|row| row.drawn).sum::<f64>() - 1.0).abs() < 1e-9);
                rows[1..rows.len() - 1]
                    .iter()
                    .any(|row| (row.drawn - 0.125).abs() > 0.05)
            };
            assert!(leans(&contest), "group training's {group}");
            contest.contest.contest_adapt_speed = false;
            assert!(!leans(&contest), "group training's {group}");
        }
    }

    #[test]
    fn one_speed_is_the_only_one_sent() {
        let mut settings = speed_settings(true);
        settings.playback.char_wpm_max = 10.0;
        let rows = speed_forecast(&settings, &[session()]);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].wpm, rows[0].drawn), (10.0, 1.0));
    }

    #[test]
    fn every_set_with_practice_in_it_is_listed_in_order() {
        let of = |mode: CharSetMode| SessionResult {
            char_set_mode: mode,
            ..session()
        };
        let empty = SessionResult {
            char_set_mode: CharSetMode::Digits,
            groups: Vec::new(),
            ..session()
        };
        assert_eq!(
            practised_modes(&[of(CharSetMode::Words), of(CharSetMode::Koch), empty]),
            [CharSetMode::Koch, CharSetMode::Words]
        );
        assert!(practised_modes(&[]).is_empty());
    }

    #[test]
    fn callsigns_have_no_group_length_to_show() {
        let mut settings = length_settings(true);
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        assert!(group_length_forecast(&settings, &[session()]).is_empty());
        assert!(span_history(&settings, &[session()]).is_empty());
    }

    #[test]
    fn the_span_history_has_a_point_for_each_session_of_the_set() {
        let mut later = session();
        later.timestamp = 2;
        let points = span_history(&length_settings(false), &[session(), later]);
        assert_eq!(points.len(), 2);
    }
}
