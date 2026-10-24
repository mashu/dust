//! Words of the learner's own, each sent whole as a group: a list pasted in,
//! or loaded from a text file with a word to a line.
//!
//! Until there is a list, Words sends a starter one: the Q-codes,
//! abbreviations, prosigns and reports a CW contact is made of. A list is read the same way however it
//! arrives, and what could not be kept is counted, so the screen can say so.

use crate::morse::morse_for;
use crate::settings::{GROUP_SIZE_MAX, TrainingSettings};

/// The most words a list keeps. Plenty for a vocabulary to drill, and a
/// list this long still costs a stored settings file only tens of
/// kilobytes.
pub const WORD_LIST_MAX: usize = 2_000;

/// What Words sends until a list of the learner's own replaces it: the
/// Q-codes, abbreviations, prosigns, numbers and reports a CW contact is
/// made of, the Q-codes first.
pub const STARTER_WORDS: &[&str] = &[
    "QRL", "QRM", "QRN", "QRO", "QRP", "QRS", "QRT", "QRZ", "QSB", "QSL", "QSO", "QSY", "QTH",
    "AGN", "ANT", "CFM", "CPI", "CPY", "CQ", "CS", "CU", "CUAGN", "CUL", "DE", "DX", "ES", "FB",
    "FER", "FREQ", "GA", "GD", "GE", "GL", "GM", "GN", "GUD", "HR", "HW?", "IS", "K", "MY", "NAME",
    "NR", "OK", "OM", "OP", "POTA", "PSE", "PWR", "REF", "RIG", "RST", "SIG", "SKED", "SN", "SOTA",
    "SRI", "TEMP", "TEST", "TNX", "TU", "UR", "VY", "WX", "YL", "AR", "BK", "KN", "SK", "44", "72",
    "73", "88", "539", "559", "579", "599", "53N", "55N", "57N", "5NN",
];

/// A list as it was read, and what was left out of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WordList {
    /// The words kept, upper-cased, each once, in the order they came.
    pub words: Vec<String>,
    /// Words with a character Morse has no code for, or longer than a group
    /// can be.
    pub unsendable: usize,
    /// Words that were in the list already.
    pub repeated: usize,
    /// Words past the most a list keeps.
    pub over: usize,
}

impl WordList {
    /// Read a list. Words are split at any white space, so one to a line
    /// reads as it should, and so do several to a line.
    pub fn read(text: &str) -> Self {
        Self::from_words(text.split_whitespace())
    }

    /// Keep what can be sent of `words`, each once, up to the most a list
    /// keeps, and count the rest.
    pub fn from_words<'a>(words: impl IntoIterator<Item = &'a str>) -> Self {
        let mut list = Self::default();
        for word in words {
            let Some(word) = clean_word(word) else {
                list.unsendable += 1;
                continue;
            };
            if list.words.contains(&word) {
                list.repeated += 1;
            } else if list.words.len() >= WORD_LIST_MAX {
                list.over += 1;
            } else {
                list.words.push(word);
            }
        }
        list
    }

    /// Words read and not kept, for whatever reason.
    pub fn left_out(&self) -> usize {
        self.unsendable + self.repeated + self.over
    }
}

/// `word` as a group sends it, upper-cased: `None` when it has a character
/// Morse has no code for, is empty, or is longer than a group can be.
pub fn clean_word(word: &str) -> Option<String> {
    let word = word.trim().to_uppercase();
    let length = word.chars().count();
    let sendable = (1..=GROUP_SIZE_MAX as usize).contains(&length)
        && word.chars().all(|ch| morse_for(ch).is_some());
    sendable.then_some(word)
}

impl TrainingSettings {
    /// The words Words sends: the learner's own, or the starter list while
    /// there are none.
    pub fn words(&self) -> Vec<&str> {
        if self.curriculum.word_list.is_empty() {
            STARTER_WORDS.to_vec()
        } else {
            self.curriculum
                .word_list
                .iter()
                .map(String::as_str)
                .collect()
        }
    }

    /// Whether the words are the learner's own rather than the starter list.
    pub fn has_own_words(&self) -> bool {
        !self.curriculum.word_list.is_empty()
    }

    /// The characters the words are made of, each once, in the order they
    /// first come.
    pub fn word_characters(&self) -> Vec<char> {
        let mut characters = Vec::new();
        for word in self.words() {
            for ch in word.chars() {
                if !characters.contains(&ch) {
                    characters.push(ch);
                }
            }
        }
        characters
    }
}

#[cfg(test)]
mod tests {
    use super::{STARTER_WORDS, WORD_LIST_MAX, WordList, clean_word};
    use crate::pool::{compute_char_pool, newest_unlocked};
    use crate::rng::SplitMix64;
    use crate::sampling::{CharSamplingState, generate_training_group};
    use crate::settings::{CharSetMode, GROUP_SIZE_MAX, TrainingSettings};

    fn words(list: &[&str]) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Words;
        settings.curriculum.word_list = list.iter().map(|word| (*word).to_string()).collect();
        settings
    }

    #[test]
    fn a_word_to_a_line_reads_as_a_list() {
        let list = WordList::read("cq\r\nDE\n\n  73 \nqth\n");
        assert_eq!(list.words, ["CQ", "DE", "73", "QTH"]);
        assert_eq!(list.left_out(), 0);
    }

    /// Several to a line, tabs, a file without a final newline: all words.
    #[test]
    fn any_white_space_separates_words() {
        let list = WordList::read("CQ CQ\tDE SO5KM\n K");
        assert_eq!(list.words, ["CQ", "DE", "SO5KM", "K"]);
        assert_eq!(list.repeated, 1);
    }

    /// What Morse has no code for, and what is longer than a group, is left
    /// out and counted; punctuation Morse does have is kept.
    #[test]
    fn what_cannot_be_sent_is_counted_and_left_out() {
        let long = "A".repeat(GROUP_SIZE_MAX as usize + 1);
        let list = WordList::read(&format!("ÄRGER {long} R/P 5NN? CAFÉ X"));
        assert_eq!(list.words, ["R/P", "5NN?", "X"]);
        assert_eq!(list.unsendable, 3);
    }

    #[test]
    fn a_list_keeps_at_most_its_limit() {
        let text: Vec<String> = (0..WORD_LIST_MAX + 7).map(|n| format!("W{n}")).collect();
        let list = WordList::read(&text.join("\n"));
        assert_eq!(list.words.len(), WORD_LIST_MAX);
        assert_eq!(list.over, 7);
        assert_eq!(list.words.first().map(String::as_str), Some("W0"));
    }

    #[test]
    fn a_word_is_cleaned_to_what_a_group_sends() {
        assert_eq!(clean_word(" qrz? "), Some("QRZ?".to_string()));
        assert_eq!(clean_word(""), None);
        assert_eq!(clean_word("Ω"), None);
    }

    /// Every group is a word of the list, sent whole, and every word comes
    /// round: the group size has no say.
    #[test]
    fn words_are_sent_whole_and_every_one_comes_round() {
        let mut settings = words(&["PARIS", "CODEX", "73", "QRL?"]);
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 2;
        let mut state = CharSamplingState::default();
        let mut rng = SplitMix64::new(7);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..200 {
            let group = generate_training_group(&settings, &mut state, &mut rng);
            assert!(settings.words().contains(&group.as_str()), "{group}");
            seen.insert(group);
        }
        assert_eq!(seen.len(), 4, "{seen:?}");
    }

    /// A character the history says is weak brings the words that hold it
    /// round more often, as it does a callsign.
    #[test]
    fn a_weak_character_brings_its_words_round_more_often() {
        let settings = words(&["MMM", "TTT", "EEE", "ZZZ"]);
        let count = |state: &CharSamplingState| {
            let mut state = state.clone();
            let mut rng = SplitMix64::new(11);
            (0..2_000)
                .filter(|_| generate_training_group(&settings, &mut state, &mut rng) == "ZZZ")
                .count()
        };
        let even = count(&CharSamplingState::default());
        let mut weak = CharSamplingState::default();
        let mut tally = crate::alignment::CopyTally::default();
        tally.sent.insert(
            'Z',
            crate::alignment::LetterAccuracy {
                correct: 0,
                total: 30,
            },
        );
        weak.learn(&tally);
        assert!(count(&weak) > even * 3 / 2, "{} vs {even}", count(&weak));
    }

    /// The characters of the words are the pool, all at once: there is
    /// nothing to unlock and no newest character.
    #[test]
    fn the_pool_is_the_characters_of_the_words() {
        let settings = words(&["CQ", "DE", "QRZ"]);
        assert_eq!(compute_char_pool(&settings), ['C', 'Q', 'D', 'E', 'R', 'Z']);
        assert_eq!(newest_unlocked(&settings), None);
        assert_eq!(settings.active_level(), 1);
        assert_eq!(settings.max_active_level(), 1);
    }

    /// A list stored by hand is cleaned when the settings are.
    #[test]
    fn a_stored_list_is_cleaned_with_the_settings() {
        let clamped = words(&["cq", "CQ", "", "ÄRGER", "73"]).clamp();
        assert_eq!(clamped.curriculum.word_list, ["CQ", "73"]);
    }

    /// Without a list of its own, Words sends the starter list, and every
    /// word of it can be sent.
    #[test]
    fn the_starter_list_stands_in_until_there_is_one() {
        let mut settings = TrainingSettings::default();
        assert!(!settings.has_own_words());
        assert_eq!(settings.words(), STARTER_WORDS);
        let starter = WordList::from_words(STARTER_WORDS.iter().copied());
        assert_eq!(starter.words.len(), STARTER_WORDS.len());
        assert_eq!(starter.left_out(), 0);

        settings.curriculum.word_list = vec!["PARIS".into(), "CODEX".into()];
        assert!(settings.has_own_words());
        assert_eq!(settings.words(), ["PARIS", "CODEX"]);
        assert_eq!(
            settings.word_characters(),
            ['P', 'A', 'R', 'I', 'S', 'C', 'O', 'D', 'E', 'X']
        );
    }
}
