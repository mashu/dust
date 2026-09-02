//! Morse code tables, LCWO Koch sequence, and encode/decode helpers.

use std::collections::HashMap;
use std::sync::OnceLock;

/// LCWO (Learn CW Online) Koch sequence. Level 1 unlocks the first two characters.
pub const LCWO_SEQUENCE: &[char] = &[
    'K', 'M', 'U', 'R', 'E', 'S', 'N', 'A', 'P', 'T', 'L', 'W', 'I', '.', 'J', 'Z', '=', 'F', 'O',
    'Y', ',', 'V', 'G', '5', '/', 'Q', '9', '2', 'H', '3', '8', 'B', '?', '4', '7', 'C', '1', 'D',
    '6', '0', 'X',
];

pub use crate::level::{
    unlocked_count_for_level as unlocked_char_count_for_level, LEVEL_MIN as KOCH_LEVEL_MIN,
};
/// Final level of the built-in LCWO curriculum (`LCWO_SEQUENCE.len() - 1`).
pub const KOCH_LEVEL_MAX: u32 = (LCWO_SEQUENCE.len() as u32).saturating_sub(1);
pub const DEFAULT_SLIDING_WINDOW_START: u32 = 1;
pub const DEFAULT_SLIDING_WINDOW_END: u32 = LCWO_SEQUENCE.len() as u32;
pub const SLIDING_WINDOW_INDEX_MAX: u32 = LCWO_SEQUENCE.len() as u32;

pub const DIGITS: &[char] = &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
/// Highest digits level: level 1 unlocks two digits, so 10 digits → level 9.
pub const MAX_DIGITS_LEVEL: u32 = crate::level::max_level_for_len(DIGITS.len());
pub const MIN_DIGITS_LEVEL: u32 = crate::level::LEVEL_MIN;

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

pub fn is_digit(ch: char) -> bool {
    ch.is_ascii_digit()
}

pub fn is_scored_char(ch: char) -> bool {
    morse_for(ch).is_some()
}

/// Digits unlocked at digits level L (level 1 → digits 0–1).
pub fn digits_unlocked_count(level: u32) -> usize {
    crate::level::unlocked_prefix(DIGITS, level).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn koch_level_one_unlocks_k_and_m() {
        assert_eq!(unlocked_char_count_for_level(1), 2);
        assert_eq!(&LCWO_SEQUENCE[..2], &['K', 'M']);
    }

    #[test]
    fn morse_round_trip_letters() {
        for ch in 'A'..='Z' {
            let code = morse_for(ch).expect("letter in table");
            assert_eq!(decode_morse_pattern(code), Some(ch));
        }
    }

    #[test]
    fn prefix_check() {
        assert!(is_morse_code_prefix(""));
        assert!(is_morse_code_prefix(".-"));
        assert!(!is_morse_code_prefix("......-"));
    }
}
