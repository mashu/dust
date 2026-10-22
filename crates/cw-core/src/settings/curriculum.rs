//! What the curriculum trains: the alphabet behind each level, how far into it
//! you are, and which of them the character-set mode practises.

use super::{CharSetMode, MixedAutoLevelAxis, PracticeWindow, TrainingSettings};
use crate::callsign::{CALLSIGN_TIER_MAX, CALLSIGN_TIER_MIN, callsign_pool};
use crate::level::{LEVEL_MIN, max_level_for_len};
use crate::morse::{DIGITS, LCWO_SEQUENCE, MAX_DIGITS_LEVEL, morse_for};

/// One of the levels the curriculum keeps.
///
/// Each is stored on its own, so switching character sets never
/// reinterprets a number that meant something else: Koch, Custom and Mixed's
/// letters share the letter level, digits have theirs, callsigns their tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelAxis {
    /// The letter alphabet: the sequence, a custom set, or Mixed's letters.
    Letters,
    Digits,
    /// The callsign tier. Its characters do not unlock one by one; the shape
    /// of the call is what climbs.
    Callsign,
    /// A word list. It has one level: every word is in from the start, and
    /// nothing climbs.
    Words,
}

impl LevelAxis {
    /// The bottom of the axis: where every alphabet and every tier starts.
    pub fn min_level(self) -> u32 {
        match self {
            Self::Letters | Self::Digits | Self::Words => LEVEL_MIN,
            Self::Callsign => CALLSIGN_TIER_MIN,
        }
    }
}

impl From<MixedAutoLevelAxis> for LevelAxis {
    fn from(axis: MixedAutoLevelAxis) -> Self {
        match axis {
            MixedAutoLevelAxis::Letters => Self::Letters,
            MixedAutoLevelAxis::Digits => Self::Digits,
        }
    }
}

impl CharSetMode {
    /// The level this mode is trained on. Mixed trains its digits beside its
    /// letters, but its level is the letter one.
    pub fn axis(self) -> LevelAxis {
        match self {
            Self::Digits => LevelAxis::Digits,
            // Contest callers' calls are shaped by the callsign tier.
            Self::Callsign | Self::Contest => LevelAxis::Callsign,
            Self::Words => LevelAxis::Words,
            Self::Koch | Self::Mixed => LevelAxis::Letters,
        }
    }
}

/// The highest letter level any alphabet can carry: every character with a
/// Morse code, one level short of all of them. A stored level is kept below
/// this rather than below the current alphabet's top, so a shorter alphabet
/// tried for a moment does not cost the level a longer one reached.
pub(crate) const LETTER_LEVEL_CEILING: u32 = max_level_for_len(crate::morse::MORSE_CHARACTERS);

impl TrainingSettings {
    /// Unique Morse characters, first-seen order, skipping whitespace and unknowns.
    pub fn unique_alphabet(chars: &[char]) -> Vec<char> {
        let mut out = Vec::new();
        for ch in chars {
            let up = ch.to_ascii_uppercase();
            if morse_for(up).is_some() && !out.contains(&up) {
                out.push(up);
            }
        }
        out
    }

    /// The unlock order: the chosen sequence, or LCWO's.
    pub fn sequence(&self) -> &[char] {
        if self.curriculum.custom_sequence.is_empty() {
            LCWO_SEQUENCE
        } else {
            &self.curriculum.custom_sequence
        }
    }

    /// The alphabet the letter level unlocks, in order. Mixed leaves the
    /// digits to their own level. A sequence with nothing sendable in it is
    /// LCWO's, so this is never empty except in Mixed, whose digits then
    /// carry the pool.
    pub fn progress_alphabet(&self) -> Vec<char> {
        let mut base = Self::unique_alphabet(self.sequence());
        if base.is_empty() {
            base = LCWO_SEQUENCE.to_vec();
        }
        if self.curriculum.char_set_mode == CharSetMode::Mixed {
            base.retain(|c| !c.is_ascii_digit());
        }
        base
    }

    /// The characters `axis` unlocks, in order. A callsign tier has no order
    /// to unlock: every character of the tier is in from the start.
    pub fn alphabet(&self, axis: LevelAxis) -> Vec<char> {
        match axis {
            LevelAxis::Letters => self.progress_alphabet(),
            LevelAxis::Digits => DIGITS.to_vec(),
            LevelAxis::Callsign => callsign_pool(self.level(LevelAxis::Callsign)),
            LevelAxis::Words => self.word_characters(),
        }
    }

    /// The top of `axis` for the alphabet it has now.
    pub fn max_level(&self, axis: LevelAxis) -> u32 {
        match axis {
            LevelAxis::Letters => max_level_for_len(self.progress_alphabet().len()),
            LevelAxis::Digits => MAX_DIGITS_LEVEL,
            LevelAxis::Callsign => CALLSIGN_TIER_MAX,
            LevelAxis::Words => LEVEL_MIN,
        }
    }

    /// How far into `axis` you are, as far as its alphabet now reaches. The
    /// stored level may be higher, from a longer alphabet, and is kept for
    /// when that alphabet comes back.
    pub fn level(&self, axis: LevelAxis) -> u32 {
        let stored = match axis {
            LevelAxis::Letters => self.curriculum.level,
            LevelAxis::Digits => self.curriculum.digits_level,
            LevelAxis::Callsign => self.curriculum.callsign_level,
            LevelAxis::Words => LEVEL_MIN,
        };
        let min = axis.min_level();
        stored.clamp(min, self.max_level(axis).max(min))
    }

    pub fn set_level(&mut self, axis: LevelAxis, value: u32) {
        match axis {
            LevelAxis::Letters => self.curriculum.level = value,
            LevelAxis::Digits => self.curriculum.digits_level = value,
            LevelAxis::Callsign => self.curriculum.callsign_level = value,
            // A word list has no level to keep.
            LevelAxis::Words => {}
        }
    }

    /// The level the current mode is trained on.
    pub fn active_level(&self) -> u32 {
        self.level(self.curriculum.char_set_mode.axis())
    }

    pub fn set_active_level(&mut self, value: u32) {
        self.set_level(self.curriculum.char_set_mode.axis(), value);
    }

    pub fn max_active_level(&self) -> u32 {
        self.max_level(self.curriculum.char_set_mode.axis())
    }

    /// Whether Mixed is practising this axis at all: a share of 100% letters
    /// leaves the digits out, and 0% the letters.
    pub fn mixed_axis_practised(&self, axis: MixedAutoLevelAxis) -> bool {
        let letters_percent = self.curriculum.mixed_letters_percent.min(100);
        match axis {
            MixedAutoLevelAxis::Letters => letters_percent > 0,
            MixedAutoLevelAxis::Digits => letters_percent < 100,
        }
    }

    /// Identity of the alphabet this mode is training, in its unlock order:
    /// what keeps auto-level counters apart when the order changes, since a
    /// level means a different set of characters in another order. What the
    /// sampler knows of each character is not kept apart by it.
    pub fn alphabet_fingerprint(&self) -> String {
        match self.curriculum.char_set_mode.axis() {
            // One fingerprint for every tier: the characters do not change as
            // you climb, so what you learned at tier 1 still counts at tier 6.
            LevelAxis::Callsign => "callsign".to_string(),
            // One for every list, the same way: what was learned of a
            // character in one list still counts in the next.
            LevelAxis::Words => "words".to_string(),
            axis => self.alphabet(axis).into_iter().collect(),
        }
    }

    /// Switch character set, starting it on its whole alphabet.
    pub fn set_char_set_mode(&mut self, mode: CharSetMode) {
        self.curriculum.char_set_mode = mode;
        self.curriculum.practice_window = PracticeWindow::All;
    }
}
