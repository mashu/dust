//! Per-character timing: how soon a character's key went in after its sound
//! went to the audio output, in every mode, and how fast that counts as;
//! and, for a group copied once it had ended, how long you paused before
//! each of its keys (its copy time). The design of record is
//! docs/recognition.md.
//!
//! What is kept of a group is raw: when each of its characters ended and
//! when each position of the answer first held its character
//! ([`CharTimes`]). The rules ([`recognise`], [`copy_times`]) are applied
//! when the history is read, so a better rule reads the whole of it again.
//! A group is timed one way or the other, never both.

use serde::{Deserialize, Deserializer, Serialize};

use crate::session::MonoMs;

/// The longest a recognition or copy time says: a character typed later
/// than this after it sounded, or after the key before it, is timed as
/// this, as slow as the bands go.
pub const RECOGNITION_MAX_MS: u32 = 3_000;

/// The quickest a key can answer a sound: sooner, and it was a guess, or a
/// letter read ahead from the word, not recognised.
pub const REACTION_MIN_MS: u32 = 100;

/// How long after a character reached you the key for the one before it may
/// still come and you be keeping up: later, and this one's time is the
/// backlog. A group's first key, for its last character, the same.
pub const KEEPING_UP_MS: u32 = 100;

/// However short the rest of a group is, its first key may come this long
/// after its first character ended, and the keeping-up slack after that,
/// and the group still count as typed along: a believable reaction to its
/// first character. Behind `ET` at 25 WPM the rest is gone in 288 ms,
/// sooner than most people key what they heard. A first key has no backlog
/// behind it, so its time is its character's own; one this lets in is
/// timed at this and the slack at most, with the delay to the ear, as every
/// time is. Only the first key: a later one's slack stays
/// [`KEEPING_UP_MS`], or the backlog would be timed.
pub const ALONG_FLOOR_MS: u32 = 400;

/// The fewest timed tries of a character for its recognition time to be put
/// in a band, or its copy time to go by: fewer, and one slow key moves the
/// median too far.
pub const RECOGNITION_MIN_TRIES: u32 = 8;

/// How many of a character's latest timed tries its recognition time, or
/// its copy time, is the median of: recent enough to show it getting faster.
pub const RECOGNITION_WINDOW: usize = 40;

/// The bands' edges are kept between these.
pub const RECOGNITION_BAND_MIN_MS: u32 = REACTION_MIN_MS;
pub const RECOGNITION_BAND_MAX_MS: u32 = RECOGNITION_MAX_MS;

/// The characters of an answer as it is kept and timed: whitespace left out,
/// upper case. Every place that counts an answer's characters counts them
/// with this, so a key's position is the answer's.
pub fn answer_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
}

/// When each position of an answer was first keyed, on one clock, and which
/// positions hold a correction: a character typed where another was, typed
/// again where one was taken out, or put in inside the text. A correction is
/// no recognition of its own, but when its position was first keyed still
/// says whether you were keeping up, so putting a slip right costs the
/// characters around it nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys<T> {
    /// Each position's first key; none where it cannot be timed, as typed
    /// for you.
    pub at: Vec<Option<T>>,
    /// The positions holding a correction, in order.
    pub corrected: Vec<usize>,
}

impl<T> Default for Keys<T> {
    fn default() -> Self {
        Self {
            at: Vec::new(),
            corrected: Vec::new(),
        }
    }
}

impl<T: Copy> Keys<T> {
    /// The same keys, each moment as `f` makes it: on another clock.
    pub fn map<U>(&self, mut f: impl FnMut(T) -> U) -> Keys<U> {
        Keys {
            at: self.at.iter().map(|at| at.map(&mut f)).collect(),
            corrected: self.corrected.clone(),
        }
    }

    /// The keys of positions `range`, as an answer's of their own: a word's
    /// within a field.
    pub fn within(&self, range: std::ops::Range<usize>) -> Self {
        Self {
            corrected: self
                .corrected
                .iter()
                .filter(|at| range.contains(at))
                .map(|at| at - range.start)
                .collect(),
            at: self.at.get(range).map(<[_]>::to_vec).unwrap_or_default(),
        }
    }

    /// As many positions as an answer of `length` characters has: what it
    /// holds no keys for is untimed.
    pub fn fitted(&self, length: usize) -> Self {
        let mut at: Vec<Option<T>> = self.at.iter().copied().take(length).collect();
        at.resize(length, None);
        Self {
            at,
            corrected: self
                .corrected
                .iter()
                .copied()
                .filter(|at| *at < length)
                .collect(),
        }
    }
}

/// The keys of a box, kept as it is typed into ([`Keys`]).
///
/// A character typed at the end takes the moment given with it. One typed
/// where another was, or again where one was taken out at the end, is a
/// correction, and keeps the moment its position was first keyed; one put
/// in inside the text is a correction keyed now. An edit inside the text
/// moves what follows it, moments and all, and the end with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyLog {
    held: Vec<Held>,
    /// The positions past the end, from the end on, that held a character
    /// taken out at the end, each with when it was first keyed: an edit
    /// inside the text moves them with the end.
    gone: Vec<Option<MonoMs>>,
}

/// One position of a box: its character, when the position was first keyed,
/// and whether it holds a correction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    ch: char,
    at: Option<MonoMs>,
    corrected: bool,
}

impl KeyLog {
    /// The box now holds `text`, already counted as the box counts it; what
    /// is new in it was keyed at `at`, or typed for you, untimed, at `None`.
    pub fn typed(&mut self, text: impl IntoIterator<Item = char>, at: Option<MonoMs>) {
        let new: Vec<char> = text.into_iter().collect();
        let old = &self.held;
        let same = |(held, ch): (&Held, &char)| held.ch == *ch;
        let prefix = old.iter().zip(&new).take_while(|pair| same(*pair)).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|pair| same(*pair))
            .count();
        let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
        let replaced = &old[prefix..old_end];
        let inside = suffix > 0;
        let mut held = Vec::with_capacity(new.len());
        held.extend_from_slice(&old[..prefix]);
        for (k, &ch) in new[prefix..new_end].iter().enumerate() {
            let first = match replaced.get(k) {
                Some(was) => Some(was.at),
                None if inside => None,
                None => self.gone.get(k - replaced.len()).copied(),
            };
            held.push(match first {
                Some(first) => Held {
                    ch,
                    at: first,
                    corrected: true,
                },
                None => Held {
                    ch,
                    at,
                    corrected: inside,
                },
            });
        }
        held.extend_from_slice(&old[old_end..]);
        // An edit at the end types over what was taken out there, or takes
        // out more; one inside moves the end with what follows it.
        let typed = new_end - prefix;
        if !inside {
            if typed >= replaced.len() {
                let past = (typed - replaced.len()).min(self.gone.len());
                self.gone.drain(..past);
            } else {
                let taken = replaced[typed..].iter().map(|was| was.at);
                self.gone.splice(0..0, taken);
            }
        }
        self.held = held;
    }

    /// The box's keys as they stand.
    pub fn keys(&self) -> Keys<MonoMs> {
        Keys {
            at: self.held.iter().map(|held| held.at).collect(),
            corrected: self
                .held
                .iter()
                .enumerate()
                .filter(|(_, held)| held.corrected)
                .map(|(at, _)| at)
                .collect(),
        }
    }

    /// The box emptied for a new answer: nothing in it was a correction.
    pub fn clear(&mut self) {
        self.held.clear();
        self.gone.clear();
    }
}

/// What is kept of how a group, or a contest's call or exchange token, was
/// heard and typed: the raw facts its recognition times are read from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharTimes {
    /// Where each character sent ended, its last sample gone to the output,
    /// in milliseconds after the sending's first sample did.
    pub ends: Vec<u32>,
    /// When each position of the answer was first keyed, on the same clock;
    /// none where it cannot be timed: typed for you, before the sending
    /// began, or after it was sent again.
    pub keys: Vec<Option<u32>>,
    /// The positions holding a correction ([`Keys::corrected`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrected: Vec<usize>,
    /// The delay the device reports from its output to the ear. It decides
    /// only whether you were keeping up, and is never added to a time.
    pub latency_ms: u32,
    /// Whether the box took keys while the sending went on.
    pub box_open: bool,
}

impl CharTimes {
    /// A sending whose first sample went out at `origin`, its characters
    /// ending `ends` milliseconds after it, keyed as `keys` say: every moment
    /// in milliseconds on one clock, the app's or a contest run's. A key
    /// before the sending, or from `until` on (it was sent again, or a
    /// contest run paused), says nothing.
    pub fn of(
        origin: u64,
        ends: &[u32],
        keys: &Keys<u64>,
        until: Option<u64>,
        latency_ms: u32,
        box_open: bool,
    ) -> Self {
        let heard = |key: &u64| *key >= origin && until.is_none_or(|until| *key < until);
        let after = |key: u64| u32::try_from(key - origin).unwrap_or(u32::MAX);
        Self {
            ends: ends.to_vec(),
            keys: keys
                .at
                .iter()
                .map(|key| key.filter(heard).map(after))
                .collect(),
            corrected: keys.corrected.clone(),
            latency_ms,
            box_open,
        }
    }

    /// Whether the rules could read a time from these at all: a key to time.
    /// With the box shut, a group of more than one character has no
    /// recognition time but has a copy time ([`copy_times`]), read from the
    /// same keys. What has no key is not kept, as it would cost a browser's
    /// store room for nothing.
    pub fn can_be_timed(&self) -> bool {
        self.keys.iter().any(Option::is_some)
    }
}

/// A group's [`CharTimes`] as a save holds them, read so that a shape this
/// build does not know reads as untimed: never as a session lost.
pub(crate) fn lenient_char_times<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<CharTimes>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Read {
        Known(CharTimes),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Read::deserialize(deserializer)? {
        Read::Known(times) => Some(times),
        Read::Other(_) => None,
    })
}

/// How soon each character of `sent` was recognised, as `received` was
/// typed and `times` keep it: in milliseconds, none where the rule says
/// nothing (docs/recognition.md):
///
/// 1. typed right, in its place ([`lined_up`]), and not as a correction;
/// 2. typed no sooner than [`REACTION_MIN_MS`] after it sounded;
/// 3. keeping up: the key before it in by [`KEEPING_UP_MS`] after it
///    reached you, and a group of more than one character typed along
///    ([`typed_along`]). A position's first key says so, put right since
///    or not.
///
/// A gap past [`RECOGNITION_MAX_MS`] counts as that.
pub fn recognise(sent: &str, received: &str, times: &CharTimes) -> Vec<Option<u32>> {
    let sent: Vec<char> = sent.chars().collect();
    let typed: Vec<char> = received.chars().collect();
    let mut out = vec![None; sent.len()];
    if times.ends.len() != sent.len() || sent.is_empty() {
        return out;
    }
    let keys = &times.keys;
    let reached = |at: usize| {
        times.ends[at]
            .saturating_add(times.latency_ms)
            .saturating_add(KEEPING_UP_MS)
    };
    if sent.len() > 1 && !typed_along(times) {
        return out;
    }
    for (at, time) in out.iter_mut().enumerate().take(lined_up(&sent, &typed)) {
        let Some(key) = keys.get(at).copied().flatten() else {
            continue;
        };
        let end = times.ends[at];
        let keeping_up = match at.checked_sub(1) {
            None => true,
            Some(before) => keys
                .get(before)
                .copied()
                .flatten()
                .is_some_and(|before| before <= reached(at)),
        };
        let own = !times.corrected.contains(&at);
        if typed[at] == sent[at] && own && key >= end.saturating_add(REACTION_MIN_MS) && keeping_up
        {
            *time = Some((key - end).min(RECOGNITION_MAX_MS));
        }
    }
    out
}

/// Whether a group of more than one character was typed along, as
/// [`recognise`] needs it to be before it times any of its characters: the
/// box open as it was sent, and its first key in by the time its last
/// character reached you, and the keeping-up slack; or, however short the
/// rest of the group, by [`ALONG_FLOOR_MS`] after its first character
/// ended, and the slack. Copied whole and typed after, it says how fast
/// you recall, not recognise.
pub fn typed_along(times: &CharTimes) -> bool {
    let (Some(&first_end), Some(&last_end)) = (times.ends.first(), times.ends.last()) else {
        return false;
    };
    let by = last_end
        .max(first_end.saturating_add(ALONG_FLOOR_MS))
        .saturating_add(times.latency_ms)
        .saturating_add(KEEPING_UP_MS);
    times.box_open
        && times
            .keys
            .first()
            .copied()
            .flatten()
            .is_some_and(|first| first <= by)
}

/// Whether a group of more than one character was copied after it ended
/// rather than typed along: its first key has a moment, and is later than
/// [`typed_along`] allows, or the box was shut. Its characters get a copy
/// time ([`copy_times`]) and never a recognition time; a group of one
/// character never gets one.
pub fn copied_after(times: &CharTimes) -> bool {
    times.ends.len() > 1 && times.keys.first().copied().flatten().is_some() && !typed_along(times)
}

/// How long you paused before each character's key as you copied `sent`,
/// typed `received`, once its group had ended (docs/recognition.md, Copy
/// time): the first from the group's end as heard (its last character's
/// end and the reported delay), each later one from the key before it.
/// None where the rule says nothing, and everywhere unless
/// [`copied_after`]:
///
/// 1. typed right, in its place ([`lined_up`]), and not as a correction;
/// 2. after the first, the position before it keyed, and not a
///    correction: a slip put right there holds the putting right;
/// 3. its key after the moment it is timed from.
///
/// A pause past [`RECOGNITION_MAX_MS`] counts as that. It holds your
/// typing, and for the first character your start: it is no recognition,
/// and has no bands.
pub fn copy_times(sent: &str, received: &str, times: &CharTimes) -> Vec<Option<u32>> {
    let sent: Vec<char> = sent.chars().collect();
    let typed: Vec<char> = received.chars().collect();
    let mut out = vec![None; sent.len()];
    if sent.is_empty() || times.ends.len() != sent.len() || !copied_after(times) {
        return out;
    }
    let keyed = |at: usize| times.keys.get(at).copied().flatten();
    let corrected = |at: usize| times.corrected.contains(&at);
    let heard = times.ends[sent.len() - 1].saturating_add(times.latency_ms);
    for (at, time) in out.iter_mut().enumerate().take(lined_up(&sent, &typed)) {
        if typed[at] != sent[at] || corrected(at) {
            continue;
        }
        let Some(key) = keyed(at) else {
            continue;
        };
        let from = match at.checked_sub(1) {
            None => Some(heard),
            Some(before) if corrected(before) => None,
            Some(before) => keyed(before),
        };
        if let Some(from) = from.filter(|from| key > *from) {
            *time = Some((key - from).min(RECOGNITION_MAX_MS));
        }
    }
    out
}

/// How long after a group had ended, its last character gone to the
/// output and on to you, its first key came: what a group typed after it
/// was sent says of when you started. Negative for a first key while it
/// still sounded; none without one.
pub fn first_key_late_ms(times: &CharTimes) -> Option<i64> {
    let first = times.keys.first().copied().flatten()?;
    let heard = i64::from(*times.ends.last()?) + i64::from(times.latency_ms);
    Some(i64::from(first) - heard)
}

/// How many positions of `typed` line up with `sent`: all of them for an
/// answer as long as what was sent; otherwise those before the first
/// difference, less a letter repeated at its end, which could be either of
/// the two (`KMM` typed `KM`).
fn lined_up(sent: &[char], typed: &[char]) -> usize {
    if sent.len() == typed.len() {
        return sent.len();
    }
    let mut before = sent.iter().zip(typed).take_while(|(a, b)| a == b).count();
    while before > 0 {
        let last = Some(&sent[before - 1]);
        if sent.get(before) != last && typed.get(before) != last {
            break;
        }
        before -= 1;
    }
    before
}

/// Where recognising a character counts as fast, instant (ICR), slow and too
/// slow: three edges, in milliseconds from its sound to its key, ascending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecognitionBands {
    #[serde(rename = "recognitionFastMs")]
    pub fast_ms: u32,
    #[serde(rename = "recognitionIcrMs")]
    pub icr_ms: u32,
    #[serde(rename = "recognitionSlowMs")]
    pub slow_ms: u32,
}

impl Default for RecognitionBands {
    fn default() -> Self {
        Self {
            fast_ms: 400,
            icr_ms: 600,
            slow_ms: 800,
        }
    }
}

/// One edge of the recognition bands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecognitionEdge {
    /// Under it is fast.
    Fast,
    /// Under it is instant character recognition (ICR).
    Icr,
    /// Under it is slow; from it on, too slow.
    Slow,
}

impl RecognitionBands {
    /// Each edge in bounds, and each at least the one before it: how a save
    /// is read.
    #[must_use]
    pub fn ordered(self) -> Self {
        let edge = |ms: u32| ms.clamp(RECOGNITION_BAND_MIN_MS, RECOGNITION_BAND_MAX_MS);
        let fast_ms = edge(self.fast_ms);
        let icr_ms = edge(self.icr_ms).max(fast_ms);
        Self {
            fast_ms,
            icr_ms,
            slow_ms: edge(self.slow_ms).max(icr_ms),
        }
    }

    /// `edge` set to `ms`, in bounds, the others moved only as far as it
    /// takes to keep them in order: how a slider moves one.
    #[must_use]
    pub fn with(self, edge: RecognitionEdge, ms: u32) -> Self {
        let ms = ms.clamp(RECOGNITION_BAND_MIN_MS, RECOGNITION_BAND_MAX_MS);
        match edge {
            RecognitionEdge::Fast => Self {
                fast_ms: ms,
                icr_ms: self.icr_ms.max(ms),
                slow_ms: self.slow_ms.max(ms),
            },
            RecognitionEdge::Icr => Self {
                fast_ms: self.fast_ms.min(ms),
                icr_ms: ms,
                slow_ms: self.slow_ms.max(ms),
            },
            RecognitionEdge::Slow => Self {
                fast_ms: self.fast_ms.min(ms),
                icr_ms: self.icr_ms.min(ms),
                slow_ms: ms,
            },
        }
    }

    /// The band a recognition time of `ms` falls in.
    pub fn band_of(self, ms: f64) -> RecognitionBand {
        if ms < f64::from(self.fast_ms) {
            RecognitionBand::Fast
        } else if ms < f64::from(self.icr_ms) {
            RecognitionBand::Icr
        } else if ms < f64::from(self.slow_ms) {
            RecognitionBand::Slow
        } else {
            RecognitionBand::TooSlow
        }
    }
}

/// How fast a character is recognised, by the bands its settings set
/// ([`RecognitionBands`]): from fast, through instant character recognition
/// (ICR) and slow, to too slow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecognitionBand {
    /// Under the first edge.
    Fast,
    /// Under the second: recognised at once rather than worked out.
    Icr,
    /// Under the third.
    Slow,
    /// From the third on.
    TooSlow,
}

impl RecognitionBand {
    /// Fast enough to count as instant character recognition.
    pub fn counts_as_icr(self) -> bool {
        matches!(self, Self::Fast | Self::Icr)
    }
}

/// How fast a character is recognised, as far as it has been timed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Recognition {
    /// Timed enough to say: its median, the band it falls in, and whether it
    /// counts as instant (ICR): fast enough, and copied right often enough
    /// that the time is its own, not only that of the tries it got right.
    Banded {
        ms: f64,
        band: RecognitionBand,
        instant: bool,
    },
    /// Timed this many times, too few to put in a band: the median so far.
    Provisional { ms: f64, tries: u32 },
    /// Never timed.
    Untimed,
}

/// How long you pause before keying a character as you copy a group once it
/// has ended, as far as it has been timed ([`copy_times`]). No bands: it
/// holds your typing as well.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CopyTime {
    /// Timed enough to go by: the median of its latest tries.
    Timed { ms: f64 },
    /// Timed this many times, too few to go by: the median so far.
    Provisional { ms: f64, tries: u32 },
    /// Never timed.
    Untimed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Option<MonoMs> {
        Some(MonoMs(ms))
    }

    /// A key log's keys, as milliseconds, and its corrections.
    fn keys(log: &KeyLog) -> (Vec<Option<u64>>, Vec<usize>) {
        let keys = log.keys().map(|at| at.0);
        (keys.at, keys.corrected)
    }

    /// Typed at the end, each character takes its moment. Taken out and
    /// typed again, or typed over another, it is a correction keeping the
    /// moment its position was first keyed; put in inside the text, a
    /// correction keyed then. An edit inside moves what follows it, moments
    /// and all. Typed for you, none.
    #[test]
    fn a_key_log_keeps_when_each_position_was_first_keyed() {
        let mut log = KeyLog::default();
        log.typed(answer_chars("k"), at(100));
        log.typed(answer_chars("k m"), at(200));
        assert_eq!(keys(&log), (vec![Some(100), Some(200)], vec![]));
        // X typed, taken out, R typed in its place: a correction, keyed
        // first as X was.
        log.typed(answer_chars("KMX"), at(300));
        log.typed(answer_chars("KM"), at(350));
        log.typed(answer_chars("KMR"), at(400));
        assert_eq!(keys(&log), (vec![Some(100), Some(200), Some(300)], vec![2]));
        // Typed on past where the box ever reached: its own again.
        log.typed(answer_chars("KMRU"), at(500));
        assert_eq!(keys(&log).1, [2]);
        // M put right inside: a correction, keyed first as M was; what
        // follows keeps its moments.
        log.typed(answer_chars("KXRU"), at(600));
        assert_eq!(
            keys(&log),
            (vec![Some(100), Some(200), Some(300), Some(500)], vec![1, 2])
        );
        // X taken out inside, and M put in there: a correction keyed then.
        log.typed(answer_chars("KRU"), at(700));
        log.typed(answer_chars("KMRU"), at(800));
        assert_eq!(
            keys(&log),
            (vec![Some(100), Some(800), Some(300), Some(500)], vec![1, 2])
        );
        // Typed for you: none.
        log.typed(answer_chars("KMRUE"), None);
        assert_eq!(keys(&log).0.last(), Some(&None));
        log.clear();
        log.typed(answer_chars("K"), at(900));
        assert_eq!(keys(&log), (vec![Some(900)], vec![]));
    }

    /// Two letters taken out at the end and typed again: both corrections,
    /// keyed first as they were; and once past them, the box's own again.
    #[test]
    fn letters_taken_out_at_the_end_are_corrections_when_typed_again() {
        let mut log = KeyLog::default();
        log.typed(answer_chars("K"), at(100));
        log.typed(answer_chars("KX"), at(200));
        log.typed(answer_chars("KXY"), at(300));
        log.typed(answer_chars("K"), at(400));
        log.typed(answer_chars("KM"), at(500));
        log.typed(answer_chars("KMR"), at(600));
        log.typed(answer_chars("KMRU"), at(700));
        assert_eq!(
            keys(&log),
            (vec![Some(100), Some(200), Some(300), Some(700)], vec![1, 2])
        );
        // A selection typed over at the end: the same.
        log.typed(answer_chars("KME"), at(800));
        assert_eq!(
            keys(&log),
            (vec![Some(100), Some(200), Some(300)], vec![1, 2])
        );
        log.typed(answer_chars("KMEE"), at(900));
        assert_eq!(keys(&log).1, [1, 2, 3]);
        // A letter taken out at the end, the text put right inside, and the
        // letter typed again: still a correction, keyed first as it was.
        let mut log = KeyLog::default();
        for (text, ms) in [("K", 100), ("KM", 200), ("KMR", 300), ("KMRU", 400)] {
            log.typed(answer_chars(text), at(ms));
        }
        log.typed(answer_chars("KMR"), at(500));
        log.typed(answer_chars("KXR"), at(600));
        log.typed(answer_chars("KXRU"), at(700));
        assert_eq!(
            keys(&log),
            (vec![Some(100), Some(200), Some(300), Some(400)], vec![1, 3])
        );
    }

    /// Keys on another clock, a word's of a field, and as many as an
    /// answer holds.
    #[test]
    fn keys_move_clocks_and_cut_as_an_answer_needs() {
        let keys = Keys {
            at: vec![Some(10), None, Some(30), Some(40)],
            corrected: vec![1, 3],
        };
        assert_eq!(
            keys.map(|at| at + 1).at,
            [Some(11), None, Some(31), Some(41)]
        );
        let word = keys.within(2..4);
        assert_eq!(
            (word.at, word.corrected),
            (vec![Some(30), Some(40)], vec![1])
        );
        assert_eq!(keys.within(3..9).at, Vec::<Option<u64>>::new());
        let fitted = keys.fitted(2);
        assert_eq!(
            (fitted.at, fitted.corrected),
            (vec![Some(10), None], vec![1])
        );
        assert_eq!(keys.fitted(5).at.len(), 5);
    }

    /// Characters ending 300, 700 and 1000 ms into a send heard through no
    /// delay, typed as `keys` say, with the box open as `open` says.
    fn times(keys: &[Option<u32>], open: bool) -> CharTimes {
        CharTimes {
            ends: vec![300, 700, 1_000],
            keys: keys.to_vec(),
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: open,
        }
    }

    #[test]
    fn typed_along_each_character_is_timed_from_its_end() {
        let typed_along = times(&[Some(550), Some(900), Some(1_250)], true);
        assert_eq!(
            recognise("KMR", "KMR", &typed_along),
            [Some(250), Some(200), Some(250)]
        );
        // A gap past the cap counts as the cap.
        let slow = times(&[Some(550), Some(900), Some(4_500)], true);
        assert_eq!(recognise("KMR", "KMR", &slow)[2], Some(RECOGNITION_MAX_MS));
    }

    /// Typed wrong, typed too soon to be heard, or typed behind: untimed.
    #[test]
    fn only_a_character_typed_right_while_keeping_up_is_timed() {
        let keys = times(&[Some(550), Some(900), Some(1_250)], true);
        assert_eq!(recognise("KMR", "KXR", &keys), [Some(250), None, Some(250)]);
        // M keyed 50 ms after its end: a guess.
        let early = times(&[Some(550), Some(750), Some(1_250)], true);
        assert_eq!(
            recognise("KMR", "KMR", &early),
            [Some(250), None, Some(250)]
        );
        // M keyed after R reached you, and the keeping-up slack: R's gap is
        // M's backlog. At the slack exactly, it is not.
        let behind = times(&[Some(550), Some(1_101), Some(1_300)], true);
        assert_eq!(
            recognise("KMR", "KMR", &behind),
            [Some(250), Some(401), None]
        );
        let just = times(&[Some(550), Some(1_100), Some(1_300)], true);
        assert_eq!(recognise("KMR", "KMR", &just)[2], Some(300));
        // A key with no moment says nothing of the one after it.
        let untimed = times(&[Some(550), None, Some(1_250)], true);
        assert_eq!(recognise("KMR", "KMR", &untimed), [Some(250), None, None]);
    }

    /// A slip put right is no recognition of its own, but costs the
    /// characters around it nothing: its position's first key still says
    /// you were keeping up, and a group whose first was put right is still
    /// typed along. Left wrong, it is the same.
    #[test]
    fn a_slip_put_right_is_untimed_and_costs_the_rest_nothing() {
        let mut fixed = times(&[Some(550), Some(900), Some(1_250)], true);
        fixed.corrected = vec![1];
        assert_eq!(
            recognise("KMR", "KMR", &fixed),
            [Some(250), None, Some(250)]
        );
        assert_eq!(
            recognise("KMR", "KXR", &fixed),
            [Some(250), None, Some(250)]
        );
        fixed.corrected = vec![0];
        assert_eq!(
            recognise("KMR", "KMR", &fixed),
            [None, Some(200), Some(250)]
        );
    }

    /// The device's delay counts towards keeping up, never towards the time.
    #[test]
    fn a_reported_delay_moves_when_you_are_keeping_up_not_the_time() {
        // Each key 550 ms after its character: behind by the time the next
        // went out, but not by the time it reached you through 300 ms.
        let mut late = times(&[Some(850), Some(1_250), Some(1_550)], true);
        assert_eq!(recognise("KMR", "KMR", &late), [Some(550), None, None]);
        late.latency_ms = 300;
        assert_eq!(
            recognise("KMR", "KMR", &late),
            [Some(550), Some(550), Some(550)]
        );
    }

    /// Copied whole and typed after, or with the box shut: a group of more
    /// than one character is not timed. A group of one is, either way.
    #[test]
    fn a_group_is_timed_only_when_typed_along() {
        let after = times(&[Some(1_101), Some(1_300), Some(1_500)], true);
        assert_eq!(recognise("KMR", "KMR", &after), [None, None, None]);
        let in_time = times(&[Some(1_100), Some(1_200), Some(1_300)], true);
        assert_eq!(recognise("KMR", "KMR", &in_time)[0], Some(800));
        let shut = times(&[Some(550), Some(900), Some(1_250)], false);
        assert_eq!(recognise("KMR", "KMR", &shut), [None, None, None]);
        let one = CharTimes {
            ends: vec![300],
            keys: vec![Some(620)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: false,
        };
        assert_eq!(recognise("K", "K", &one), [Some(320)]);
    }

    /// However short the rest of a group, its first key may come up to
    /// [`ALONG_FLOOR_MS`] after its first character ended, and the slack,
    /// and the group still be typed along: `ET` at 25 WPM, its T gone 288
    /// ms after E, sooner than a key for E could come. T is timed only if
    /// E's key was in by the time T reached you, as ever.
    #[test]
    fn a_first_key_soon_after_its_own_end_is_typed_along_however_short_the_rest() {
        let et = |keys: [u32; 2], latency_ms: u32| CharTimes {
            ends: vec![48, 336],
            keys: keys.map(Some).to_vec(),
            corrected: Vec::new(),
            latency_ms,
            box_open: true,
        };
        assert_eq!(recognise("ET", "ET", &et([448, 700], 0)), [Some(400), None]);
        // In by 48 + 400 + 100 ms, and no later.
        assert_eq!(recognise("ET", "ET", &et([548, 800], 0))[0], Some(500));
        assert_eq!(recognise("ET", "ET", &et([549, 800], 0)), [None, None]);
        assert_eq!(recognise("ET", "ET", &et([600, 800], 0)), [None, None]);
        // Typed along to the end: both timed.
        assert_eq!(
            recognise("ET", "ET", &et([400, 600], 0)),
            [Some(352), Some(264)]
        );
        // A reported delay moves both deadlines.
        assert_eq!(
            recognise("ET", "ET", &et([588, 800], 40)),
            [Some(540), None]
        );
        assert_eq!(recognise("ET", "ET", &et([589, 800], 40)), [None, None]);
        assert_eq!(recognise("ET", "ET", &et([476, 700], 40))[1], Some(364));
        // With the box shut, the floor lets nothing in.
        let shut = CharTimes {
            box_open: false,
            ..et([448, 700], 0)
        };
        assert_eq!(recognise("ET", "ET", &shut), [None, None]);
    }

    /// How late a group's first key came after the group reached you: what
    /// a group typed after it was sent says.
    #[test]
    fn a_first_key_says_how_long_after_its_group_it_came() {
        let mut after = times(&[Some(1_400), Some(1_600), Some(1_800)], true);
        assert_eq!(first_key_late_ms(&after), Some(400));
        after.latency_ms = 40;
        assert_eq!(first_key_late_ms(&after), Some(360));
        let along = times(&[Some(550), Some(900), Some(1_250)], true);
        assert_eq!(first_key_late_ms(&along), Some(-450));
        let none = times(&[None, Some(1_600), Some(1_800)], true);
        assert_eq!(first_key_late_ms(&none), None);
    }

    /// An answer shorter or longer than what was sent lines up only before
    /// its first difference, and not on a letter that could be either of a
    /// pair; ends that do not fit what was sent say nothing.
    #[test]
    fn only_keys_that_line_up_with_what_was_sent_are_timed() {
        let missed = CharTimes {
            ends: vec![300, 700, 1_000],
            keys: vec![Some(550), Some(1_250)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        // KMM typed KM: which M was typed cannot be told.
        assert_eq!(recognise("KMM", "KM", &missed), [Some(250), None, None]);
        // KMK typed KMMK: the K before the extra M is K's all the same.
        let inserted = CharTimes {
            keys: vec![Some(550), Some(900), Some(950), Some(1_250)],
            ..missed.clone()
        };
        assert_eq!(recognise("KMK", "KMMK", &inserted), [Some(250), None, None]);
        // KMR typed KR: R is in M's place.
        assert_eq!(recognise("KMR", "KR", &missed), [Some(250), None, None]);
        // Only the start typed so far: what there is is timed.
        assert_eq!(
            recognise("KMR", "KM", &missed),
            [Some(250), Some(550), None]
        );
        let extra = CharTimes {
            keys: vec![Some(550), Some(900), Some(950)],
            ..missed.clone()
        };
        assert_eq!(
            recognise(
                "KM",
                "KMM",
                &CharTimes {
                    ends: vec![300, 700],
                    ..extra
                }
            ),
            [Some(250), None]
        );
        assert_eq!(
            recognise(
                "KMR",
                "KMR",
                &CharTimes {
                    ends: vec![300],
                    ..missed
                }
            ),
            [None, None, None]
        );
    }

    #[test]
    fn char_times_are_kept_from_the_sending_on() {
        let keys = Keys {
            at: vec![Some(990), Some(1_550), Some(2_100), None],
            corrected: vec![1],
        };
        let times = CharTimes::of(1_000, &[300, 700], &keys, Some(2_000), 40, true);
        assert_eq!(times.ends, [300, 700]);
        assert_eq!(times.keys, [None, Some(550), None, None]);
        assert_eq!(times.corrected, [1]);
    }

    /// Kept under the keys a save names, and a shape this build does not
    /// know read as untimed.
    #[test]
    fn char_times_read_back_and_forgive_what_they_do_not_know() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default, deserialize_with = "lenient_char_times")]
            times: Option<CharTimes>,
        }
        let mut times = CharTimes {
            ends: vec![300, 700],
            keys: vec![Some(550), None],
            corrected: Vec::new(),
            latency_ms: 40,
            box_open: true,
        };
        let json = serde_json::to_string(&times).expect("serialize");
        assert_eq!(
            json,
            r#"{"ends":[300,700],"keys":[550,null],"latencyMs":40,"boxOpen":true}"#
        );
        times.corrected = vec![1];
        let fixed = serde_json::to_string(&times).expect("serialize");
        assert!(fixed.contains(r#""corrected":[1]"#), "{fixed}");
        let read: Holder =
            serde_json::from_str(&format!(r#"{{"times":{fixed}}}"#)).expect("a save loads");
        assert_eq!(read.times.as_ref(), Some(&times));
        times.corrected.clear();
        let read: Holder =
            serde_json::from_str(&format!(r#"{{"times":{json}}}"#)).expect("a save loads");
        assert_eq!(read.times, Some(times));
        for odd in [
            r#""fast""#,
            "[1,2]",
            r#"{"ends":[1.5]}"#,
            r#"{"ends":[-1]}"#,
            "7",
        ] {
            let read: Holder =
                serde_json::from_str(&format!(r#"{{"times":{odd}}}"#)).expect("still loads");
            assert_eq!(read.times, None, "{odd}");
        }
    }

    /// `KMR` copied after it ended: each later letter timed from the key
    /// before it, the first from the group's end as heard. None of it is
    /// recognised.
    #[test]
    fn copied_after_each_letter_is_timed_from_the_key_before() {
        let copied = times(&[Some(1_400), Some(1_600), Some(1_850)], true);
        assert!(copied_after(&copied));
        assert_eq!(
            copy_times("KMR", "KMR", &copied),
            [Some(400), Some(200), Some(250)]
        );
        assert_eq!(recognise("KMR", "KMR", &copied), [None, None, None]);
        // Heard 40 ms after it went out: the first pause is from then.
        let delayed = CharTimes {
            latency_ms: 40,
            ..copied
        };
        assert_eq!(
            copy_times("KMR", "KMR", &delayed),
            [Some(360), Some(200), Some(250)]
        );
    }

    /// A group is recognised or copied, never both: the first key in by
    /// the typed-along deadline (its last character's end and the keeping-up
    /// slack) times it as recognised, a millisecond later as copied.
    #[test]
    fn a_group_is_recognised_or_copied_never_both() {
        let along = times(&[Some(1_100), Some(1_300), Some(1_500)], true);
        assert_eq!(recognise("KMR", "KMR", &along)[0], Some(800));
        assert_eq!(copy_times("KMR", "KMR", &along), [None, None, None]);
        let after = times(&[Some(1_101), Some(1_300), Some(1_500)], true);
        assert_eq!(recognise("KMR", "KMR", &after), [None, None, None]);
        assert_eq!(
            copy_times("KMR", "KMR", &after),
            [Some(101), Some(199), Some(200)]
        );
        // Typed along, falling behind: recognised where it kept up (M slow,
        // but K was in before M reached you), and R, behind M, untimed:
        // never copied.
        let behind = times(&[Some(550), Some(1_500), Some(1_700)], true);
        assert_eq!(
            recognise("KMR", "KMR", &behind),
            [Some(250), Some(800), None]
        );
        assert_eq!(copy_times("KMR", "KMR", &behind), [None, None, None]);
        // `ET` at 25 WPM: the along floor decides.
        let et = |first: u32| CharTimes {
            ends: vec![48, 336],
            keys: vec![Some(first), Some(first + 251)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        assert_eq!(recognise("ET", "ET", &et(548)), [Some(500), None]);
        assert_eq!(copy_times("ET", "ET", &et(548)), [None, None]);
        assert_eq!(recognise("ET", "ET", &et(549)), [None, None]);
        assert_eq!(copy_times("ET", "ET", &et(549)), [Some(213), Some(251)]);
    }

    /// A group of one character has a recognition time, never a copy time,
    /// whether the box was open or shut.
    #[test]
    fn a_group_of_one_character_has_recognition_only() {
        for open in [true, false] {
            let one = CharTimes {
                ends: vec![300],
                keys: vec![Some(1_300)],
                corrected: Vec::new(),
                latency_ms: 0,
                box_open: open,
            };
            assert!(!copied_after(&one));
            assert_eq!(copy_times("K", "K", &one), [None]);
            assert_eq!(recognise("K", "K", &one), [Some(1_000)]);
        }
    }

    /// With the box shut, a group of more than one character is copied
    /// after it ended, whenever its first key came: a key before the group
    /// was heard to its end says nothing of the first, and the one after it
    /// is timed from it all the same.
    #[test]
    fn with_the_box_shut_a_group_is_copied() {
        let shut = times(&[Some(1_050), Some(1_300), Some(1_500)], false);
        assert!(copied_after(&shut));
        assert_eq!(
            copy_times("KMR", "KMR", &shut),
            [Some(50), Some(250), Some(200)]
        );
        assert_eq!(recognise("KMR", "KMR", &shut), [None, None, None]);
        let early = times(&[Some(990), Some(1_300), Some(1_500)], false);
        assert_eq!(
            copy_times("KMR", "KMR", &early),
            [None, Some(310), Some(200)]
        );
        let delayed = CharTimes {
            latency_ms: 200,
            ..times(&[Some(1_100), Some(1_300), Some(1_500)], false)
        };
        assert_eq!(
            copy_times("KMR", "KMR", &delayed),
            [None, Some(200), Some(200)]
        );
    }

    /// Only letters copied right, in their place, and not as a slip put
    /// right are timed; nor the letter after one put right, whose pause
    /// holds the putting right.
    #[test]
    fn only_letters_copied_right_in_their_place_are_timed() {
        let keys = [Some(1_400), Some(1_600), Some(1_850)];
        let copied = times(&keys, true);
        assert_eq!(
            copy_times("KMR", "KXR", &copied),
            [Some(400), None, Some(250)]
        );
        let fixed = |at: usize| CharTimes {
            corrected: vec![at],
            ..times(&keys, true)
        };
        assert_eq!(copy_times("KMR", "KMR", &fixed(1)), [Some(400), None, None]);
        assert_eq!(copy_times("KMR", "KMR", &fixed(0)), [None, None, Some(250)]);
        let short = times(&[Some(1_400), Some(1_600)], true);
        assert_eq!(copy_times("KMR", "KR", &short), [Some(400), None, None]);
        assert_eq!(copy_times("KMM", "KM", &short), [Some(400), None, None]);
        let long = times(&[Some(1_500), Some(1_700), Some(1_950), Some(2_100)], true);
        assert_eq!(
            copy_times("KMR", "KMRE", &long),
            [Some(500), Some(200), Some(250)]
        );
    }

    /// A pause is timed only between two keys with moments, in order.
    #[test]
    fn a_copy_gap_needs_two_keys_in_order() {
        let first_unknown = times(&[None, Some(1_600), Some(1_850)], true);
        assert!(!copied_after(&first_unknown), "neither kind");
        assert_eq!(copy_times("KMR", "KMR", &first_unknown), [None, None, None]);
        assert_eq!(recognise("KMR", "KMR", &first_unknown), [None, None, None]);
        let together = times(&[Some(1_400), Some(1_400), Some(1_850)], true);
        assert_eq!(
            copy_times("KMR", "KMR", &together),
            [Some(400), None, Some(450)]
        );
        let out_of_order = times(&[Some(1_500), Some(1_400), Some(1_950)], true);
        assert_eq!(
            copy_times("KMR", "KMR", &out_of_order),
            [Some(500), None, Some(550)]
        );
    }

    /// A pause past the cap counts as the cap.
    #[test]
    fn a_copy_gap_past_the_cap_counts_as_the_cap() {
        let slow = times(&[Some(1_500), Some(5_000), Some(5_100)], true);
        assert_eq!(
            copy_times("KMR", "KMR", &slow),
            [Some(500), Some(RECOGNITION_MAX_MS), Some(100)]
        );
    }

    /// Ends that do not fit what was sent say nothing.
    #[test]
    fn ends_that_do_not_fit_say_nothing() {
        let odd = CharTimes {
            ends: vec![300],
            ..times(&[Some(1_400), Some(1_600), Some(1_850)], true)
        };
        assert_eq!(copy_times("KMR", "KMR", &odd), [None, None, None]);
        assert_eq!(copy_times("", "", &odd), Vec::<Option<u32>>::new());
    }

    /// A group with a key is kept whether the box was open or shut, for its
    /// copy time; one with no key is not. Shut, it reads back as kept.
    #[test]
    fn a_group_with_a_key_is_kept_with_the_box_shut() {
        let shut = times(&[None, Some(1_300), None], false);
        assert!(shut.can_be_timed());
        assert!(!times(&[None, None, None], false).can_be_timed());
        assert!(!times(&[None, None, None], true).can_be_timed());
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default, deserialize_with = "lenient_char_times")]
            times: Option<CharTimes>,
        }
        let json = serde_json::to_string(&shut).expect("serialize");
        assert!(json.contains(r#""boxOpen":false"#), "{json}");
        let read: Holder =
            serde_json::from_str(&format!(r#"{{"times":{json}}}"#)).expect("a save loads");
        assert_eq!(read.times, Some(shut));
    }

    #[test]
    fn the_bands_stay_in_order() {
        let bands = RecognitionBands::default();
        assert_eq!(
            (bands.fast_ms, bands.icr_ms, bands.slow_ms),
            (400, 600, 800)
        );
        let moved = bands.with(RecognitionEdge::Fast, 700);
        assert_eq!(
            (moved.fast_ms, moved.icr_ms, moved.slow_ms),
            (700, 700, 800)
        );
        let moved = bands.with(RecognitionEdge::Slow, 300);
        assert_eq!(
            (moved.fast_ms, moved.icr_ms, moved.slow_ms),
            (300, 300, 300)
        );
        let moved = bands.with(RecognitionEdge::Icr, 9_999);
        assert_eq!(moved.slow_ms, RECOGNITION_BAND_MAX_MS);
        let odd = RecognitionBands {
            fast_ms: 900,
            icr_ms: 500,
            slow_ms: 10,
        };
        let read = odd.ordered();
        assert_eq!((read.fast_ms, read.icr_ms, read.slow_ms), (900, 900, 900));
        // The edges, each the first of the band above it.
        let band = |ms| bands.band_of(ms);
        assert_eq!(
            [
                band(399.0),
                band(400.0),
                band(599.0),
                band(600.0),
                band(799.0),
                band(800.0)
            ],
            [
                RecognitionBand::Fast,
                RecognitionBand::Icr,
                RecognitionBand::Icr,
                RecognitionBand::Slow,
                RecognitionBand::Slow,
                RecognitionBand::TooSlow,
            ]
        );
        assert!(band(100.0).counts_as_icr() && band(500.0).counts_as_icr());
        assert!(!band(700.0).counts_as_icr());
    }
}
