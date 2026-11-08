//! Group alignment via edit distance for letter-level scoring.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlignmentPair {
    pub sent_char: Option<char>,
    pub received_char: Option<char>,
    pub matched: bool,
}

/// What an answer holds where a character was skipped: Space on the
/// keypad or a keyboard, to move on past a character not caught. It keeps
/// the character's place, so the rest of the answer lines up as copied,
/// and it is read as that character missed, never as another character it
/// was taken for. No group ever holds it.
pub const SKIPPED: char = '_';

/// Pair what was sent with what was received, character by character, at
/// the fewest edits: a match, a wrong character, one dropped, or one added.
/// A character skipped ([`SKIPPED`]) is one dropped where it stands, and a
/// skip past the group's end is nothing.
pub fn align_group(sent: &str, received: &str) -> Vec<AlignmentPair> {
    let (s, r) = upper(sent, received);
    // A clean copy is the common case, and needs no table.
    if s == r {
        return s.iter().map(|&c| pair(Some(c), Some(c))).collect();
    }
    let table = Edits::new(&s, &r);
    unskipped(table.walk_back(&s, &r, s.len()))
}

/// `alignment` with each skip read as the character it stands for missed,
/// and a skip that stands for none left out.
fn unskipped(alignment: Vec<AlignmentPair>) -> Vec<AlignmentPair> {
    alignment
        .into_iter()
        .filter_map(|aligned| match aligned {
            AlignmentPair {
                sent_char: None,
                received_char: Some(SKIPPED),
                ..
            } => None,
            AlignmentPair {
                sent_char,
                received_char: Some(SKIPPED),
                ..
            } => Some(pair(sent_char, None)),
            aligned => Some(aligned),
        })
        .collect()
}

#[cfg(test)]
thread_local! {
    /// How many groups [`align_holding`] has aligned on this thread, for a
    /// test to count what reading a history costs.
    pub(crate) static HELD_ALIGNMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The alignment of what was held of `sent`, and how many of its characters
/// were lost off the end, after the last one received.
///
/// Of the readings that cost the fewest edits, it takes the one that loses
/// the most off the end: "KE" for "KMRSU" is one wrong and three lost, not
/// three dropped inside and the last one wrong. At least one character is
/// always held.
pub(crate) fn align_holding(sent: &str, received: &str) -> (Vec<AlignmentPair>, usize) {
    #[cfg(test)]
    HELD_ALIGNMENTS.set(HELD_ALIGNMENTS.get() + 1);
    let (s, r) = upper(sent, received);
    let table = Edits::new(&s, &r);
    let fewest = table.at(s.len(), r.len());
    let lost = (0..s.len())
        .rev()
        .find(|lost| table.at(s.len() - lost, r.len()) + *lost as u32 == fewest)
        .unwrap_or(0);
    (unskipped(table.walk_back(&s, &r, s.len() - lost)), lost)
}

/// The fewest edits — a wrong character, one dropped, or one added — that
/// turn `sent` into `received`: how far a call you sent is from a station's.
pub(crate) fn edit_distance(sent: &str, received: &str) -> u32 {
    let (s, r) = upper(sent, received);
    Edits::new(&s, &r).at(s.len(), r.len())
}

/// The most of a group that is aligned: far longer than anything Dust
/// sends, so only a history made by hand ever reaches it.
const SENT_MAX: usize = 256;

/// The most cells an alignment's table may take, 16 MB: a group against a
/// long paste fits many times over, but a history made by hand, with a
/// group of a hundred thousand characters each way, would otherwise ask
/// for some 40 GB each time a stat read it.
const TABLE_MAX: usize = 4_000_000;

/// What was sent and what was received, upper case, each cut to what an
/// alignment's table can hold: the answer keeps as much as fits beside the
/// group.
fn upper(sent: &str, received: &str) -> (Vec<char>, Vec<char>) {
    let s: Vec<char> = sent
        .chars()
        .take(SENT_MAX)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let r = received
        .chars()
        .take(TABLE_MAX / (s.len() + 1) - 1)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (s, r)
}

fn pair(sent_char: Option<char>, received_char: Option<char>) -> AlignmentPair {
    AlignmentPair {
        sent_char,
        received_char,
        matched: sent_char.is_some() && sent_char == received_char,
    }
}

/// The edit-distance table: entry `(i, j)` is the fewest edits that turn the
/// first `i` characters sent into the first `j` received. Filled row by row
/// in one flat table; every cell reads the row above it and the cell to its
/// left, so the indices are the point rather than something an iterator
/// would tidy away.
struct Edits {
    width: usize,
    cells: Vec<u32>,
}

impl Edits {
    fn new(s: &[char], r: &[char]) -> Self {
        let (m, n) = (s.len(), r.len());
        let width = n + 1;
        let mut cells = vec![0u32; (m + 1) * width];
        for (j, cell) in cells.iter_mut().take(width).enumerate() {
            *cell = j as u32;
        }
        for i in 1..=m {
            cells[i * width] = i as u32;
            for j in 1..=n {
                let match_cost = u32::from(s[i - 1] != r[j - 1]);
                cells[i * width + j] = (cells[(i - 1) * width + j] + 1)
                    .min(cells[i * width + j - 1] + 1)
                    .min(cells[(i - 1) * width + j - 1] + match_cost);
            }
        }
        Self { width, cells }
    }

    fn at(&self, i: usize, j: usize) -> u32 {
        self.cells[i * self.width + j]
    }

    /// The pairs that turn the first `held` characters sent into everything
    /// received, walked back from the end.
    fn walk_back(&self, s: &[char], r: &[char], held: usize) -> Vec<AlignmentPair> {
        let mut alignment = Vec::with_capacity(held.max(r.len()));
        let (mut i, mut j) = (held, r.len());
        while i > 0 || j > 0 {
            let here = self.at(i, j);
            if i > 0 && j > 0 && here == self.at(i - 1, j - 1) + u32::from(s[i - 1] != r[j - 1]) {
                alignment.push(pair(Some(s[i - 1]), Some(r[j - 1])));
                i -= 1;
                j -= 1;
            } else if i > 0 && (j == 0 || here == self.at(i - 1, j) + 1) {
                alignment.push(pair(Some(s[i - 1]), None));
                i -= 1;
            } else {
                alignment.push(pair(None, Some(r[j - 1])));
                j -= 1;
            }
        }
        alignment.reverse();
        alignment
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LetterAccuracy {
    pub correct: u32,
    pub total: u32,
}

/// What a copy got right and wrong, character by character: one alignment
/// per group, and everything that is scored read off it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyTally {
    /// For each character sent: how often, and how often it was copied.
    #[serde(rename = "letterAccuracy", default)]
    pub sent: BTreeMap<char, LetterAccuracy>,
    /// Characters typed that were never sent. They cost accuracy like a
    /// miss does, or typing every candidate for an unsure character would be
    /// free.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inserted: BTreeMap<char, u32>,
}

impl CopyTally {
    /// The tally of one group.
    pub fn of(sent: &str, received: &str) -> Self {
        let mut tally = Self::default();
        tally.add(sent, received);
        tally
    }

    /// Count one more group.
    pub fn add(&mut self, sent: &str, received: &str) {
        for pair in align_group(sent, received) {
            match (pair.sent_char, pair.received_char) {
                (Some(ch), _) => {
                    let entry = self.sent.entry(ch).or_default();
                    entry.total += 1;
                    entry.correct += u32::from(pair.matched);
                }
                (None, Some(ch)) => *self.inserted.entry(ch).or_default() += 1,
                (None, None) => {}
            }
        }
    }

    /// Fold another tally into this one. A tally read back from a save is
    /// not trusted to be small: a count goes no higher than it can hold.
    pub fn merge(&mut self, other: &Self) {
        for (ch, counts) in &other.sent {
            let entry = self.sent.entry(*ch).or_default();
            entry.correct = entry.correct.saturating_add(counts.correct);
            entry.total = entry.total.saturating_add(counts.total);
        }
        for (ch, count) in &other.inserted {
            let entry = self.inserted.entry(*ch).or_default();
            *entry = entry.saturating_add(*count);
        }
    }

    /// Characters copied right, over everything that had to be got right:
    /// every character sent and every one typed that was not. Zero when
    /// there was nothing to copy.
    pub fn accuracy(&self) -> f64 {
        self.accuracy_where(|_| true).unwrap_or(0.0)
    }

    /// The same, counting only the characters `keep` picks — one axis of a
    /// mixed session. `None` when none of them came up at all. Added up
    /// wide, so the counts of a tally read back from a save, however
    /// large, never run past what the sum holds.
    pub fn accuracy_where(&self, keep: impl Fn(char) -> bool) -> Option<f64> {
        let (mut correct, mut attempts) = (0u64, 0u64);
        for (_, counts) in self.sent.iter().filter(|(ch, _)| keep(**ch)) {
            correct += u64::from(counts.correct);
            attempts += u64::from(counts.total);
        }
        for (_, count) in self.inserted.iter().filter(|(ch, _)| keep(**ch)) {
            attempts += u64::from(*count);
        }
        (attempts > 0).then(|| correct as f64 / attempts as f64)
    }
}

/// The characters of a group a faster or slower send could have changed,
/// each copied right or not, and those typed that were never sent: what a
/// group says about speed, and what auto level judges while group size
/// adapts.
///
/// Everything up to the last character typed counts, misheard, dropped or
/// added. A group can also end early, with characters lost off its end, and
/// that happens two ways: the group was longer than you could hold, or copy
/// broke off at a character you did not recognise and the rest went by while
/// you looked for it. One group cannot tell which. The rest of a lost end is
/// a consequence either way, and is the length's alone. The character copy
/// broke off at, the first one lost, counts as a miss unless adaptive group
/// size can still answer for it: `length_floor` is the shortest group it
/// draws while it adapts, and a group longer than that can be sent shorter.
/// A group already as short as the range allows, or one whose length does
/// not adapt, has nothing else to answer for its break. Left out, a learner
/// who stops typing at the first miss only ever showed the speed characters
/// copied right, and drove it to the top of the range while copying almost
/// nothing.
///
/// A group with nothing typed speaks of no character: neither what was
/// recognised nor where copy broke. While group size adapts it is the
/// length's alone, at the shortest length too: there a blank is as often a
/// slip as a send too fast, and a send too fast shows in what does get
/// typed, now that the character copy broke off at counts. Where the length
/// does not adapt, nothing else learns from it, and it counts as its first
/// character missed.
///
/// Nothing sent, or a blank the length answers for: nothing to count.
pub(crate) fn speed_tally(
    sent: &str,
    received: &str,
    length_floor: Option<usize>,
) -> Option<CopyTally> {
    let sent_chars: Vec<char> = sent.chars().map(|c| c.to_ascii_uppercase()).collect();
    if sent_chars.is_empty() {
        return None;
    }
    let length_answers = length_floor.is_some_and(|floor| sent_chars.len() > floor);
    let mut tally = CopyTally::default();
    let lost = if received.trim().is_empty() {
        if length_floor.is_some() {
            return None;
        }
        sent_chars.len()
    } else {
        let (held, lost) = align_holding(sent, received);
        for pair in held {
            match (pair.sent_char, pair.received_char) {
                (Some(ch), _) => {
                    let entry = tally.sent.entry(ch).or_default();
                    entry.total += 1;
                    entry.correct += u32::from(pair.matched);
                }
                (None, Some(ch)) => *tally.inserted.entry(ch).or_default() += 1,
                (None, None) => {}
            }
        }
        lost
    };
    if lost > 0 && !length_answers {
        let broke_at = sent_chars[sent_chars.len() - lost];
        tally.sent.entry(broke_at).or_default().total += 1;
    }
    (!tally.sent.is_empty() || !tally.inserted.is_empty()).then_some(tally)
}

/// One group's characters as they are judged: every one sent and every one
/// typed that was not, as the session scored them, unless adaptive group
/// size drew its length, `length_floor` the shortest it draws. Then its
/// lost end is left out where the length answers for it, and what is
/// judged is what the speed could have changed ([`speed_tally`]), the same
/// characters adaptive speed learns from; a group with nothing typed counts
/// as its first character missed.
///
/// What auto level judges a session by, and what each character's own
/// tally counts ([`crate::SessionResult::letter_accuracy`]): a character lost off
/// the end of a group sent long on purpose is the length's, not a miss of
/// that character.
pub(crate) fn judged_copy(sent: &str, received: &str, length_floor: Option<usize>) -> CopyTally {
    let Some(floor) = length_floor else {
        return CopyTally::of(sent, received);
    };
    if received.trim().is_empty() {
        // Nothing typed reached nothing, which is no session to level up
        // on, whatever answers for it: its first character missed.
        let mut tally = CopyTally::default();
        if let Some(first) = sent.chars().next() {
            let entry = tally.sent.entry(first.to_ascii_uppercase()).or_default();
            entry.total += 1;
        }
        return tally;
    }
    speed_tally(sent, received, Some(floor)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What was lost off the end is counted apart from what was held; a
    /// clean copy loses nothing.
    #[test]
    fn a_group_cut_short_is_held_up_to_where_it_was_lost() {
        let held = |sent: &str, received: &str| {
            let (pairs, lost) = align_holding(sent, received);
            let right = pairs.iter().filter(|pair| pair.matched).count();
            (right, pairs.len(), lost)
        };
        assert_eq!(held("KMRSU", "KMRSU"), (5, 5, 0));
        assert_eq!(held("KMRSU", "KMR"), (3, 3, 2));
        assert_eq!(held("KMRSU", "KE"), (1, 2, 3), "one wrong, three lost");
        assert_eq!(held("KMRSU", "KRSU"), (4, 5, 0), "dropped inside");
        assert_eq!(held("kmrsu", "KMR"), (3, 3, 2), "case is no error");
        // The pairs held are the alignment of the part held.
        let (pairs, lost) = align_holding("KMRSU", "KXR");
        assert_eq!(lost, 2);
        assert_eq!(pairs, align_group("KMR", "KXR"));
    }

    #[test]
    fn the_distance_counts_the_fewest_edits() {
        assert_eq!(edit_distance("K5XYZ", "K5XYZ"), 0);
        assert_eq!(edit_distance("K5XYW", "K5XYZ"), 1);
        assert_eq!(edit_distance("K5XY", "K5XYZ"), 1);
        assert_eq!(edit_distance("K5XYZZ", "K5XYZ"), 1);
        assert_eq!(edit_distance("K5ZYX", "K5XYZ"), 2);
        assert_eq!(edit_distance("", "K5XYZ"), 5);
        assert_eq!(edit_distance("k5xyz", "K5XYZ"), 0, "case is no edit");
    }

    /// A group and an answer far longer than anything sent, as only a
    /// history made by hand holds, are aligned only as far as a table of
    /// bounded size reaches, not into one the size of their product.
    #[test]
    fn a_group_made_by_hand_far_too_long_is_aligned_only_so_far() {
        let sent = "K".repeat(100_000);
        let received = "M".repeat(100_000);
        let kept = TABLE_MAX / (SENT_MAX + 1) - 1;
        let alignment = align_group(&sent, &received);
        let count = |sent_side: bool| {
            alignment
                .iter()
                .filter(|pair| {
                    if sent_side {
                        pair.sent_char.is_some()
                    } else {
                        pair.received_char.is_some()
                    }
                })
                .count()
        };
        assert_eq!(count(true), SENT_MAX);
        assert_eq!(count(false), kept);
        assert!(alignment.iter().all(|pair| !pair.matched));
        assert_eq!(edit_distance(&sent, &received), kept as u32);
        let (held, lost) = align_holding(&sent, &received);
        assert!(held.len() + lost <= SENT_MAX + kept);
    }

    #[test]
    fn align_substitution() {
        let a = align_group("ABC", "ABD");
        assert_eq!(a.len(), 3);
        assert!(a[0].matched && a[1].matched);
        assert!(!a[2].matched);
        assert_eq!(a[2].sent_char, Some('C'));
        assert_eq!(a[2].received_char, Some('D'));
    }

    #[test]
    fn empty_received_is_all_deletions() {
        let a = align_group("KM", "");
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|p| p.received_char.is_none() && !p.matched));
    }

    #[test]
    fn two_empty_strings_align_to_nothing() {
        assert!(align_group("", "").is_empty());
    }

    #[test]
    fn empty_sent_is_all_insertions() {
        let a = align_group("", "KM");
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|p| p.sent_char.is_none() && !p.matched));
        assert_eq!(a[0].received_char, Some('K'));
    }

    #[test]
    fn a_dropped_character_is_a_deletion() {
        // "KMU" copied as "KU": M was never typed.
        let a = align_group("KMU", "KU");
        assert_eq!(a.len(), 3);
        assert_eq!(a[1].sent_char, Some('M'));
        assert_eq!(a[1].received_char, None);
        assert!(a[0].matched && a[2].matched);
    }

    #[test]
    fn an_extra_character_is_an_insertion() {
        // "KU" copied as "KMU": M was typed but never sent.
        let a = align_group("KU", "KMU");
        assert_eq!(a.len(), 3);
        assert_eq!(a[1].sent_char, None);
        assert_eq!(a[1].received_char, Some('M'));
        assert!(a[0].matched && a[2].matched);
    }

    #[test]
    fn a_missed_first_character_lines_the_rest_up() {
        // The traceback runs out of received characters before sent ones.
        let a = align_group("KMU", "MU");
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].sent_char, Some('K'));
        assert_eq!(a[0].received_char, None);
        assert!(a[1].matched && a[2].matched);
    }

    #[test]
    fn a_character_typed_before_the_group_lines_the_rest_up() {
        // The traceback runs out of sent characters before received ones.
        let a = align_group("MU", "KMU");
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].sent_char, None);
        assert_eq!(a[0].received_char, Some('K'));
        assert!(a[1].matched && a[2].matched);
    }

    #[test]
    fn alignment_is_case_insensitive() {
        assert!(align_group("km", "KM").iter().all(|p| p.matched));
    }

    #[test]
    fn letter_accuracy_counts_only_sent_characters() {
        let tally = CopyTally::of("KM", "KXU");
        let counts = |ch| tally.sent.get(&ch).map(|a| (a.correct, a.total));
        assert_eq!(counts('K'), Some((1, 1)));
        assert_eq!(counts('M'), Some((0, 1)));
        // The typed-but-never-sent U is not a letter of its own.
        assert!(!tally.sent.contains_key(&'U'));
    }

    #[test]
    fn accuracy_of_nothing_is_zero() {
        assert_eq!(CopyTally::default().accuracy(), 0.0);
        assert!(CopyTally::of("", "").sent.is_empty());
    }

    #[test]
    fn overall_accuracy_counts_sent_letters() {
        let mut tally = CopyTally::of("ABC", "ABC");
        tally.add("DE", "DX");
        assert!((tally.accuracy() - 0.8).abs() < 1e-9);
    }

    /// Padding an answer, or typing every candidate for a character you were
    /// not sure of, used to cost nothing: only sent characters were counted.
    #[test]
    fn characters_typed_but_never_sent_cost_accuracy() {
        for (sent, typed, expected) in [
            ("KMURE", "KMURE", 1.0),
            ("KMURE", "KXMXUXRXEX", 0.5),
            ("KM", "KMN", 2.0 / 3.0),
            ("KMURE", "KMUURE", 5.0 / 6.0),
            ("KMURE", "KMUSE", 0.8),
        ] {
            let tally = CopyTally::of(sent, typed);
            assert!(
                (tally.accuracy() - expected).abs() < 1e-9,
                "{sent} copied as {typed}: {}",
                tally.accuracy()
            );
        }
        assert_eq!(CopyTally::of("KM", "KMN").inserted.get(&'N'), Some(&1));
    }

    #[test]
    fn a_tally_can_be_read_one_axis_at_a_time() {
        let mut tally = CopyTally::of("KM0", "KM5");
        tally.add("E1", "E12");
        let digits = tally.accuracy_where(|c| c.is_ascii_digit());
        let letters = tally.accuracy_where(|c| !c.is_ascii_digit());
        assert_eq!(letters, Some(1.0));
        assert!((digits.unwrap() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(
            CopyTally::of("KM", "KM").accuracy_where(|c| c.is_ascii_digit()),
            None
        );
    }

    #[test]
    fn merging_tallies_adds_them_up() {
        let mut whole = CopyTally::of("KM", "KX");
        whole.merge(&CopyTally::of("KU", "KUU"));
        let mut direct = CopyTally::of("KM", "KX");
        direct.add("KU", "KUU");
        assert_eq!(whole, direct);
        assert_eq!(
            whole.sent[&'K'],
            LetterAccuracy {
                correct: 2,
                total: 2
            }
        );
    }

    /// A tally read back from a save is not trusted to be small: merging
    /// one, or reading its accuracy, panicked once its counts added up past
    /// what a count holds.
    #[test]
    fn counts_past_what_a_count_holds_neither_panic_nor_wrap() {
        let mut big = CopyTally::default();
        let all = |correct| LetterAccuracy {
            correct,
            total: u32::MAX,
        };
        big.sent.insert('K', all(u32::MAX));
        big.sent.insert('M', all(0));
        big.inserted.insert('E', u32::MAX);
        assert!((big.accuracy() - 1.0 / 3.0).abs() < 1e-9);
        let mut merged = big.clone();
        merged.merge(&big);
        assert_eq!(merged.sent[&'K'], all(u32::MAX));
        assert_eq!(merged.inserted[&'E'], u32::MAX);
    }

    #[test]
    fn a_long_paste_aligns_without_trouble() {
        let paste = "E".repeat(20_000);
        let pairs = align_group("KMURE", &paste);
        assert_eq!(pairs.iter().filter(|p| p.sent_char.is_some()).count(), 5);
        assert_eq!(
            pairs.iter().filter(|p| p.received_char.is_some()).count(),
            20_000
        );
    }

    /// A character skipped keeps its place: it is that character missed,
    /// the rest lines up as copied, a skip past the end is nothing, and a
    /// skip is never a character taken for another.
    #[test]
    fn a_skipped_character_is_missed_where_it_stands() {
        let pairs = align_group("ABCD", "A_CD");
        assert_eq!(pairs.len(), 4);
        assert_eq!(
            (pairs[1].sent_char, pairs[1].received_char),
            (Some('B'), None)
        );
        assert!(!pairs[1].matched);
        assert!(
            pairs
                .iter()
                .enumerate()
                .all(|(at, pair)| at == 1 || pair.matched)
        );
        // Skipped at the start and the end alike.
        let pairs = align_group("ABCD", "_BC_");
        assert_eq!(
            pairs
                .iter()
                .map(|pair| pair.received_char)
                .collect::<Vec<_>>(),
            [None, Some('B'), Some('C'), None]
        );
        // A skip past the group's end is nothing at all.
        let pairs = align_group("AB", "AB_");
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|pair| pair.matched));
        // Tallied as misses, and never as a character added.
        let tally = CopyTally::of("ABCD", "A__D_");
        assert_eq!(
            tally.sent[&'B'],
            LetterAccuracy {
                correct: 0,
                total: 1
            }
        );
        assert_eq!(
            tally.sent[&'C'],
            LetterAccuracy {
                correct: 0,
                total: 1
            }
        );
        assert!(tally.inserted.is_empty(), "{:?}", tally.inserted);
        // Held: what was skipped is missed, what never came is lost.
        let (held, lost) = align_holding("KMRSU", "K_");
        assert_eq!(lost, 3);
        assert_eq!(
            (held[1].sent_char, held[1].received_char),
            (Some('M'), None)
        );
    }
}
