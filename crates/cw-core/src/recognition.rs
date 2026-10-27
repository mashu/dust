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

/// When each position of a box first held its character, kept as it is
/// typed into.
///
/// A position keeps its moment while it holds the same character. One typed
/// at the end takes the moment given with it; one typed where another
/// character was, or where one was taken out, is a correction, and has none:
/// a character got right on the second go was not recognised when it was
/// typed. An edit inside the text keeps the moments of what follows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyLog {
    chars: Vec<char>,
    at: Vec<Option<MonoMs>>,
    /// Positions that have held another character, or been taken out, as
    /// far as the box has been typed: one typed there again is no
    /// recognition. As long as `chars` at least.
    retyped: Vec<bool>,
}

impl KeyLog {
    /// The box now holds `text`, already counted as the box counts it; what
    /// is new in it was typed at `at`, or for you, untimed, at `None`.
    pub fn typed(&mut self, text: impl IntoIterator<Item = char>, at: Option<MonoMs>) {
        let new: Vec<char> = text.into_iter().collect();
        let old = &self.chars;
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
        let mut moments = Vec::with_capacity(new.len());
        let mut retyped = Vec::with_capacity(new.len().max(self.retyped.len()));
        moments.extend_from_slice(&self.at[..prefix]);
        retyped.extend_from_slice(&self.retyped[..prefix]);
        for position in prefix..new_end {
            let again = position < old_end || self.retyped.get(position).copied().unwrap_or(false);
            moments.push(if again { None } else { at });
            retyped.push(again);
        }
        moments.extend_from_slice(&self.at[old_end..]);
        retyped.extend_from_slice(&self.retyped[old_end..old.len()]);
        if suffix == 0 {
            // Taken out at the end, now or before: typed there again, it is
            // a correction. After an edit inside the text the positions
            // past its end have moved, and are forgotten.
            let held = old.len().max(self.retyped.len());
            for position in new.len()..held {
                let gone = position < old.len();
                retyped.push(gone || self.retyped.get(position).copied().unwrap_or(false));
            }
        }
        self.chars = new;
        self.at = moments;
        self.retyped = retyped;
    }

    /// When each position of the box first held its character, if it can be
    /// timed.
    pub fn moments(&self) -> &[Option<MonoMs>] {
        &self.at
    }

    /// The box emptied for a new answer: nothing in it was a correction.
    pub fn clear(&mut self) {
        self.chars.clear();
        self.at.clear();
        self.retyped.clear();
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
    /// When each position of the answer first held its character, on the
    /// same clock; none where it cannot be timed: a correction, typed for
    /// you, before the sending began, or after it was sent again.
    pub keys: Vec<Option<u32>>,
    /// The delay the device reports from its output to the ear. It decides
    /// only whether you were keeping up, and is never added to a time.
    pub latency_ms: u32,
    /// Whether the box took keys while the sending went on.
    pub box_open: bool,
}

impl CharTimes {
    /// A sending whose first sample went out at `origin`, its characters
    /// ending `ends` milliseconds after it, keyed at `keys`: every moment in
    /// milliseconds on one clock, the app's or a contest run's. A key before
    /// the sending, or from `resent` on, when it was sent again, says
    /// nothing.
    pub fn of(
        origin: u64,
        ends: &[u32],
        keys: impl IntoIterator<Item = Option<u64>>,
        resent: Option<u64>,
        latency_ms: u32,
        box_open: bool,
    ) -> Self {
        let heard = |key: &u64| *key >= origin && resent.is_none_or(|resent| *key < resent);
        let after = |key: u64| u32::try_from(key - origin).unwrap_or(u32::MAX);
        Self {
            ends: ends.to_vec(),
            keys: keys
                .into_iter()
                .map(|key| key.filter(heard).map(after))
                .collect(),
            latency_ms,
            box_open,
        }
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
/// 1. typed right, in its place ([`lined_up`]);
/// 2. typed no sooner than [`REACTION_MIN_MS`] after it sounded;
/// 3. keeping up: the key before it in by [`KEEPING_UP_MS`] after it
///    reached you, and a group of more than one character typed along, the
///    box open and its first key in by then for its last character.
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
        if typed[at] == sent[at] && key >= end.saturating_add(REACTION_MIN_MS) && keeping_up {
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

    /// A key log's moments, as milliseconds.
    fn moments(log: &KeyLog) -> Vec<Option<u64>> {
        log.moments().iter().map(|at| at.map(|at| at.0)).collect()
    }

    /// Typed at the end, each character takes its moment; taken out and
    /// typed again, it has none; an edit inside keeps what follows it; typed
    /// for you, none.
    #[test]
    fn a_key_log_keeps_the_first_moment_of_each_character() {
        let mut log = KeyLog::default();
        log.typed(answer_chars("k"), at(100));
        log.typed(answer_chars("k m"), at(200));
        assert_eq!(moments(&log), [Some(100), Some(200)]);
        // X typed, taken out, M typed in its place: a correction.
        log.typed(answer_chars("KMX"), at(300));
        log.typed(answer_chars("KM"), at(350));
        log.typed(answer_chars("KMR"), at(400));
        assert_eq!(moments(&log), [Some(100), Some(200), None]);
        // R typed over M: that position is a correction, what follows keeps.
        log.typed(answer_chars("KMRU"), at(500));
        log.typed(answer_chars("KXRU"), at(600));
        assert_eq!(moments(&log), [Some(100), None, None, Some(500)]);
        // Typed for you: none.
        log.typed(answer_chars("KXRUE"), None);
        assert_eq!(moments(&log).last(), Some(&None));
        log.clear();
        log.typed(answer_chars("K"), at(900));
        assert_eq!(moments(&log), [Some(900)]);
    }

    /// Characters ending 300, 700 and 1000 ms into a send heard through no
    /// delay, typed as `keys` say, with the box open as `open` says.
    fn times(keys: &[Option<u32>], open: bool) -> CharTimes {
        CharTimes {
            ends: vec![300, 700, 1_000],
            keys: keys.to_vec(),
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
        // A key untimed (a correction) says nothing of the one after it.
        let corrected = times(&[Some(550), None, Some(1_250)], true);
        assert_eq!(recognise("KMR", "KMR", &corrected), [Some(250), None, None]);
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
        let times = CharTimes::of(
            1_000,
            &[300, 700],
            [Some(990), Some(1_550), Some(2_100), None],
            Some(2_000),
            40,
            true,
        );
        assert_eq!(times.ends, [300, 700]);
        assert_eq!(times.keys, [None, Some(550), None, None]);
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
        let times = CharTimes {
            ends: vec![300, 700],
            keys: vec![Some(550), None],
            latency_ms: 40,
            box_open: true,
        };
        let json = serde_json::to_string(&times).expect("serialize");
        assert_eq!(
            json,
            r#"{"ends":[300,700],"keys":[550,null],"latencyMs":40,"boxOpen":true}"#
        );
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
