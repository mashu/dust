use super::{CharSetMode, PracticeWindow, TrainingSettings};
use crate::level::max_level_for_len;
use crate::morse::{is_digit, morse_for};

impl TrainingSettings {
    /// Unique Morse characters, first-seen order, skipping whitespace and unknowns.
    pub fn unique_alphabet(chars: &[char]) -> Vec<char> {
        let mut out = Vec::new();
        for ch in chars {
            let up = ch.to_ascii_uppercase();
            if up.is_whitespace() || morse_for(up).is_none() {
                continue;
            }
            if !out.contains(&up) {
                out.push(up);
            }
        }
        out
    }

    /// Ordered alphabet that `level` unlocks. Custom uses `custom_set` when set.
    /// Mixed drops digits so the letter axis and digits axis stay separate.
    pub fn progress_alphabet(&self) -> Vec<char> {
        let base = if self.curriculum.char_set_mode == CharSetMode::Custom {
            let custom = Self::unique_alphabet(&self.curriculum.custom_set);
            if custom.is_empty() {
                Self::unique_alphabet(self.sequence())
            } else {
                custom
            }
        } else {
            Self::unique_alphabet(self.sequence())
        };
        if self.curriculum.char_set_mode == CharSetMode::Mixed {
            base.into_iter().filter(|c| !is_digit(*c)).collect()
        } else {
            base
        }
    }

    pub fn max_letter_level(&self) -> u32 {
        max_level_for_len(self.progress_alphabet().len())
    }

    pub fn active_alphabet(&self) -> Vec<char> {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => crate::morse::DIGITS.to_vec(),
            CharSetMode::Callsign => crate::callsign::callsign_pool(self.curriculum.callsign_level),
            _ => self.progress_alphabet(),
        }
    }

    pub fn active_level(&self) -> u32 {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => self.curriculum.digits_level,
            CharSetMode::Callsign => self.curriculum.callsign_level,
            _ => self.curriculum.level,
        }
    }

    pub fn set_active_level(&mut self, value: u32) {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => self.curriculum.digits_level = value,
            CharSetMode::Callsign => self.curriculum.callsign_level = value,
            _ => self.curriculum.level = value,
        }
    }

    pub fn max_active_level(&self) -> u32 {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => max_level_for_len(crate::morse::DIGITS.len()),
            CharSetMode::Callsign => crate::callsign::CALLSIGN_TIER_MAX,
            _ => self.max_letter_level(),
        }
    }

    pub fn sequence(&self) -> &[char] {
        if self.curriculum.custom_sequence.is_empty() {
            crate::morse::LCWO_SEQUENCE
        } else {
            &self.curriculum.custom_sequence
        }
    }

    /// Identity of the alphabet this mode is training, used to isolate auto-level
    /// counters and sampling history when the user switches sequence or custom set.
    pub fn alphabet_fingerprint(&self) -> String {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => crate::morse::DIGITS.iter().copied().collect(),
            // One fingerprint for every tier: the characters do not change as
            // you climb, so what you learned at tier 1 still counts at tier 6.
            CharSetMode::Callsign => "callsign".to_string(),
            _ => self.progress_alphabet().into_iter().collect(),
        }
    }

    /// Switch character set, resetting the practice window with it.
    pub fn set_char_set_mode(&mut self, mode: CharSetMode) {
        self.curriculum.char_set_mode = mode;
        self.curriculum.practice_window = Some(PracticeWindow::All);
    }
}
