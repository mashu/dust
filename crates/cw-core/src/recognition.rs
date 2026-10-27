//! Per-character timing: how soon a character's key went in after its sound
//! went to the audio output, in every mode, and how fast that counts as.
//! The design of record is docs/recognition.md.
//!
//! What is kept of a group is raw: when each of its characters ended and
//! when each position of the answer first held its character
//! ([`CharTimes`]). The rule ([`recognise`]) is applied when the history is
//! read, so a better rule reads the whole of it again.

use serde::{Deserialize, Deserializer, Serialize};

use crate::session::MonoMs;

/// The longest a recognition time says: a character typed later than this
/// after it sounded is timed as this, as slow as the bands go.
pub const RECOGNITION_MAX_MS: u32 = 3_000;

/// The quickest a key can answer a sound: sooner, and it was a guess, or a
/// letter read ahead from the word, not recognised.
pub const REACTION_MIN_MS: u32 = 100;

/// How long after a character reached you the key for the one before it may
/// still come and you be keeping up: later, and this one's time is the
/// backlog. A group's first key, for its last character, the same.
pub const KEEPING_UP_MS: u32 = 100;

/// The fewest timed tries of a character for its recognition time to be put
/// in a band: fewer, and one slow key moves the median too far.
pub const RECOGNITION_MIN_TRIES: u32 = 8;

/// How many of a character's latest timed tries its recognition time is the
/// median of: recent enough to show it getting faster.
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

    /// Whether the rule could read a time from these at all: a key to time,
    /// and, for more than one character, the box open as it was sent. What
    /// it could not is not kept, as it would cost a browser's store room for
    /// nothing.
    pub fn can_be_timed(&self) -> bool {
        (self.box_open || self.ends.len() == 1) && self.keys.iter().any(Option::is_some)
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
///    reached you, and a group of more than one character typed along, the
///    box open and its first key in by then for its last character. A
///    position's first key says so, put right since or not.
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
    if sent.len() > 1 {
        let along = times.box_open
            && keys
                .first()
                .copied()
                .flatten()
                .is_some_and(|first| first <= reached(sent.len() - 1));
        if !along {
            return out;
        }
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
