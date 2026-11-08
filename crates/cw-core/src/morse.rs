//! Morse code tables, LCWO Koch sequence, and encode/decode helpers.

use std::collections::HashMap;
use std::sync::OnceLock;

/// LCWO (Learn CW Online) Koch sequence. Level 1 unlocks the first two characters.
pub const LCWO_SEQUENCE: &[char] = &[
    'K', 'M', 'U', 'R', 'E', 'S', 'N', 'A', 'P', 'T', 'L', 'W', 'I', '.', 'J', 'Z', '=', 'F', 'O',
    'Y', ',', 'V', 'G', '5', '/', 'Q', '9', '2', 'H', '3', '8', 'B', '?', '4', '7', 'C', '1', 'D',
    '6', '0', 'X',
];

pub const DIGITS: &[char] = &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
/// Highest digits level: level 1 unlocks two digits, so 10 digits → level 9.
pub const MAX_DIGITS_LEVEL: u32 = crate::level::max_level_for_len(DIGITS.len());

const MORSE_PAIRS: &[(char, &str)] = &[
    ('A', ".-"),
    ('B', "-..."),
    ('C', "-.-."),
    ('D', "-.."),
    ('E', "."),
    ('F', "..-."),
    ('G', "--."),
    ('H', "...."),
    ('I', ".."),
    ('J', ".---"),
    ('K', "-.-"),
    ('L', ".-.."),
    ('M', "--"),
    ('N', "-."),
    ('O', "---"),
    ('P', ".--."),
    ('Q', "--.-"),
    ('R', ".-."),
    ('S', "..."),
    ('T', "-"),
    ('U', "..-"),
    ('V', "...-"),
    ('W', ".--"),
    ('X', "-..-"),
    ('Y', "-.--"),
    ('Z', "--.."),
    ('0', "-----"),
    ('1', ".----"),
    ('2', "..---"),
    ('3', "...--"),
    ('4', "....-"),
    ('5', "....."),
    ('6', "-...."),
    ('7', "--..."),
    ('8', "---.."),
    ('9', "----."),
    ('/', "-..-."),
    ('=', "-...-"),
    ('+', ".-.-."),
    ('?', "..--.."),
    (',', "--..--"),
    ('.', ".-.-.-"),
];

fn morse_map() -> &'static HashMap<char, &'static str> {
    static MAP: OnceLock<HashMap<char, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| MORSE_PAIRS.iter().copied().collect())
}

fn reverse_morse_map() -> &'static HashMap<&'static str, char> {
    static MAP: OnceLock<HashMap<&'static str, char>> = OnceLock::new();
    MAP.get_or_init(|| MORSE_PAIRS.iter().map(|(ch, code)| (*code, *ch)).collect())
}

pub fn morse_for(ch: char) -> Option<&'static str> {
    morse_map().get(&ch.to_ascii_uppercase()).copied()
}

pub fn decode_morse_pattern(pattern: &str) -> Option<char> {
    reverse_morse_map().get(pattern).copied()
}

pub fn is_morse_code_prefix(pattern: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    MORSE_PAIRS
        .iter()
        .any(|(_, code)| code.starts_with(pattern))
}

pub(crate) fn is_scored_char(ch: char) -> bool {
    morse_for(ch).is_some()
}

/// How many characters have a Morse code at all: the longest any alphabet
/// can be.
pub(crate) const MORSE_CHARACTERS: usize = MORSE_PAIRS.len();

/// PARIS-style dot duration in seconds from character WPM.
pub fn dot_seconds(wpm: f64) -> f64 {
    1.2 / wpm.max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morse_round_trip_letters() {
        for ch in 'A'..='Z' {
            let code = morse_for(ch).expect("letter in table");
            assert_eq!(decode_morse_pattern(code), Some(ch));
        }
    }

    #[test]
    fn a_prefix_of_a_real_code_is_a_prefix() {
        assert!(is_morse_code_prefix(""));
        assert!(is_morse_code_prefix("-"));
        assert!(is_morse_code_prefix("-.-"));
        assert!(is_morse_code_prefix(".-"));
        assert!(!is_morse_code_prefix("......."));
        assert!(!is_morse_code_prefix("......-"));
    }

    #[test]
    fn characters_round_trip_through_their_code() {
        for (ch, code) in MORSE_PAIRS {
            assert_eq!(morse_for(*ch), Some(*code));
            assert_eq!(decode_morse_pattern(code), Some(*ch));
            assert!(is_scored_char(*ch));
        }
        assert_eq!(morse_for('k'), morse_for('K'));
        assert_eq!(morse_for('#'), None);
        assert_eq!(decode_morse_pattern("........"), None);
        assert!(!is_scored_char(' '));
    }

    #[test]
    fn the_lcwo_curriculum_is_consistent() {
        assert_eq!(&LCWO_SEQUENCE[..2], &['K', 'M']);
        for ch in LCWO_SEQUENCE {
            assert!(morse_for(*ch).is_some(), "{ch:?} has no code");
        }
        assert!(LCWO_SEQUENCE.len() <= MORSE_CHARACTERS);
        assert_eq!(MAX_DIGITS_LEVEL, 9);
    }
}
