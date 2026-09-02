//! Training settings for the group trainer.

use serde::{Deserialize, Serialize};

use crate::level::{max_level_for_len, LEVEL_MIN};
use crate::morse::{
    MixedAutoLevelAxis, DEFAULT_SLIDING_WINDOW_END, DEFAULT_SLIDING_WINDOW_START,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QrmProfile {
    Whistle,
    Ringing,
    Mixed,
}

impl Default for QrmProfile {
    fn default() -> Self {
        Self::Mixed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CharSetMode {
    Koch,
    Digits,
    Custom,
    Mixed,
}

impl Default for CharSetMode {
    fn default() -> Self {
        Self::Mixed
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainingSettings {
    /// Progress through the current letter/custom alphabet (level 1 unlocks two characters).
    #[serde(alias = "kochLevel")]
    pub level: u32,
    pub char_set_mode: CharSetMode,
    pub digits_level: u32,
    /// When mixed: 0–100 percent of characters that are letters (rest digits).
    pub mixed_letters_percent: u32,
    pub mixed_auto_level_next_axis: MixedAutoLevelAxis,
    pub custom_set: Vec<char>,
    pub custom_sequence: Vec<char>,
    pub sliding_window_start: u32,
    pub sliding_window_end: u32,
    pub side_tone_min: f64,
    pub side_tone_max: f64,
    pub volume_min: f64,
    pub volume_max: f64,
    pub link_volume: bool,
    pub steepness: f64,
    pub num_groups: u32,
    pub char_wpm_min: f64,
    pub char_wpm_max: f64,
    pub link_char_wpm: bool,
    pub effective_wpm_min: f64,
    pub effective_wpm_max: f64,
    pub link_effective_wpm: bool,
    pub link_char_to_effective: bool,
    pub extra_word_space_multiplier: f64,
    pub group_timeout: f64,
    pub lock_input_during_group_playback: bool,
    pub min_group_size: u32,
    pub max_group_size: u32,
    pub link_group_size: bool,
    pub envelope_smoothing: f64,
    #[serde(default = "defaults::enabled")]
    pub qsb_enabled: bool,
    #[serde(default = "defaults::qsb_depth")]
    pub qsb_depth: f64,
    #[serde(default = "defaults::qsb_rate_hz")]
    pub qsb_rate_hz: f64,
    #[serde(default = "defaults::enabled")]
    pub qrn_enabled: bool,
    #[serde(default = "defaults::qrn_level")]
    pub qrn_level: f64,
    #[serde(default = "defaults::enabled")]
    pub qrm_enabled: bool,
    #[serde(default = "defaults::qrm_level")]
    pub qrm_level: f64,
    #[serde(default)]
    pub qrm_profile: QrmProfile,
    #[serde(default = "defaults::receiver_background_gain")]
    pub receiver_background_gain: f64,
    #[serde(default = "defaults::receiver_background_excitation_rate")]
    pub receiver_background_excitation_rate: f64,
    #[serde(default = "defaults::receiver_background_resonance")]
    pub receiver_background_resonance: f64,
    #[serde(default = "defaults::receiver_background_decay")]
    pub receiver_background_decay: f64,
    #[serde(default = "defaults::receiver_background_offset_hz")]
    pub receiver_background_offset_hz: f64,
    #[serde(default = "defaults::receiver_background_offset_mod_depth_hz")]
    pub receiver_background_offset_mod_depth_hz: f64,
    #[serde(default = "defaults::receiver_background_offset_mod_rate_hz")]
    pub receiver_background_offset_mod_rate_hz: f64,
    #[serde(alias = "autoAdjustKoch")]
    pub auto_adjust_level: bool,
    pub auto_adjust_threshold: f64,
    pub auto_adjust_below_threshold_count: u32,
    pub auto_adjust_above_threshold_count: u32,
    pub error_weight_strength: f64,
    pub char_sampling_coverage_strength: f64,
    #[serde(default)]
    pub char_sampling_thompson: bool,
}

impl Default for TrainingSettings {
    fn default() -> Self {
        Self {
            level: LEVEL_MIN,
            char_set_mode: CharSetMode::Mixed,
            digits_level: 1,
            mixed_letters_percent: 70,
            mixed_auto_level_next_axis: MixedAutoLevelAxis::Letters,
            custom_set: Vec::new(),
            custom_sequence: Vec::new(),
            sliding_window_start: DEFAULT_SLIDING_WINDOW_START,
            sliding_window_end: DEFAULT_SLIDING_WINDOW_END,
            side_tone_min: 400.0,
            side_tone_max: 600.0,
            volume_min: 0.7,
            volume_max: 1.0,
            link_volume: false,
            steepness: 10.0,
            num_groups: 20,
            char_wpm_min: 18.0,
            char_wpm_max: 25.0,
            link_char_wpm: false,
            effective_wpm_min: 18.0,
            effective_wpm_max: 25.0,
            link_effective_wpm: false,
            link_char_to_effective: true,
            extra_word_space_multiplier: 1.0,
            group_timeout: 10.0,
            lock_input_during_group_playback: true,
            min_group_size: 3,
            max_group_size: 5,
            link_group_size: false,
            envelope_smoothing: 0.75,
            qsb_enabled: true,
            qsb_depth: 0.35,
            qsb_rate_hz: 0.12,
            qrn_enabled: true,
            qrn_level: 0.25,
            qrm_enabled: true,
            qrm_level: 0.2,
            qrm_profile: QrmProfile::Mixed,
            receiver_background_gain: 20.0,
            receiver_background_excitation_rate: 62.0,
            receiver_background_resonance: 66.0,
            receiver_background_decay: 0.984,
            receiver_background_offset_hz: 140.0,
            receiver_background_offset_mod_depth_hz: 45.0,
            receiver_background_offset_mod_rate_hz: 0.32,
            auto_adjust_level: true,
            auto_adjust_threshold: 90.0,
            auto_adjust_below_threshold_count: 1,
            auto_adjust_above_threshold_count: 5,
            error_weight_strength: 3.0,
            char_sampling_coverage_strength: 1.0,
            char_sampling_thompson: false,
        }
    }
}

impl TrainingSettings {
    /// Unique uppercase characters, first-seen order, skipping whitespace.
    pub fn unique_alphabet(chars: &[char]) -> Vec<char> {
        let mut out = Vec::new();
        for ch in chars {
            let up = ch.to_ascii_uppercase();
            if up.is_whitespace() {
                continue;
            }
            if !out.contains(&up) {
                out.push(up);
            }
        }
        out
    }

    /// Ordered alphabet that `level` unlocks. Custom uses `custom_set` when set.
    pub fn progress_alphabet(&self) -> Vec<char> {
        if self.char_set_mode == CharSetMode::Custom {
            let custom = Self::unique_alphabet(&self.custom_set);
            if !custom.is_empty() {
                return custom;
            }
        }
        self.sequence().to_vec()
    }

    pub fn max_letter_level(&self) -> u32 {
        max_level_for_len(self.progress_alphabet().len())
    }

    pub fn active_alphabet(&self) -> Vec<char> {
        match self.char_set_mode {
            CharSetMode::Digits => crate::morse::DIGITS.to_vec(),
            _ => self.progress_alphabet(),
        }
    }

    pub fn active_level(&self) -> u32 {
        match self.char_set_mode {
            CharSetMode::Digits => self.digits_level,
            _ => self.level,
        }
    }

    pub fn set_active_level(&mut self, value: u32) {
        match self.char_set_mode {
            CharSetMode::Digits => self.digits_level = value,
            _ => self.level = value,
        }
    }

    pub fn max_active_level(&self) -> u32 {
        match self.char_set_mode {
            CharSetMode::Digits => crate::morse::MAX_DIGITS_LEVEL,
            _ => self.max_letter_level(),
        }
    }

    pub fn clamp(mut self) -> Self {
        let seq_max = self.max_letter_level();
        self.level = self.level.clamp(LEVEL_MIN, seq_max);
        self.digits_level = self.digits_level.clamp(1, crate::morse::MAX_DIGITS_LEVEL);
        self.mixed_letters_percent = self.mixed_letters_percent.min(100);
        self.num_groups = self.num_groups.clamp(1, 200);
        self.min_group_size = self.min_group_size.clamp(1, 20);
        self.max_group_size = self.max_group_size.clamp(self.min_group_size, 20);
        if self.link_group_size {
            self.max_group_size = self.min_group_size;
        }
        self.char_wpm_min = self.char_wpm_min.clamp(5.0, 80.0);
        self.char_wpm_max = self.char_wpm_max.clamp(self.char_wpm_min, 80.0);
        if self.link_char_wpm {
            self.char_wpm_max = self.char_wpm_min;
        }
        self.effective_wpm_min = self.effective_wpm_min.clamp(5.0, 80.0);
        self.effective_wpm_max = self.effective_wpm_max.clamp(self.effective_wpm_min, 80.0);
        if self.link_effective_wpm {
            self.effective_wpm_max = self.effective_wpm_min;
        }
        if self.link_char_to_effective {
            self.effective_wpm_min = self.char_wpm_min;
            self.effective_wpm_max = self.char_wpm_max;
        }
        self.side_tone_min = self.side_tone_min.clamp(200.0, 1200.0);
        self.side_tone_max = self.side_tone_max.clamp(self.side_tone_min, 1200.0);
        self.volume_min = self.volume_min.clamp(0.1, 1.0);
        self.volume_max = self.volume_max.clamp(self.volume_min, 1.0);
        if self.link_volume {
            self.volume_max = self.volume_min;
        }
        self.steepness = self.steepness.clamp(1.0, 50.0);
        self.envelope_smoothing = self.envelope_smoothing.clamp(0.0, 1.0);
        self.qsb_depth = self.qsb_depth.clamp(0.0, 1.0);
        self.qsb_rate_hz = self.qsb_rate_hz.clamp(0.03, 1.5);
        self.qrn_level = self.qrn_level.clamp(0.0, 1.0);
        self.qrm_level = self.qrm_level.clamp(0.0, 1.0);
        self.receiver_background_gain = self.receiver_background_gain.clamp(0.0, 20.0);
        self.receiver_background_excitation_rate =
            self.receiver_background_excitation_rate.clamp(0.1, 500.0);
        self.receiver_background_resonance = self.receiver_background_resonance.clamp(0.5, 240.0);
        self.receiver_background_decay = self.receiver_background_decay.clamp(0.5, 0.9999);
        self.receiver_background_offset_hz = self.receiver_background_offset_hz.clamp(-1000.0, 1000.0);
        self.receiver_background_offset_mod_depth_hz =
            self.receiver_background_offset_mod_depth_hz.clamp(0.0, 1000.0);
        self.receiver_background_offset_mod_rate_hz =
            self.receiver_background_offset_mod_rate_hz.clamp(0.0, 20.0);
        self.extra_word_space_multiplier = self.extra_word_space_multiplier.max(0.1);
        self.group_timeout = self.group_timeout.clamp(0.0, 120.0);
        self.auto_adjust_threshold = self.auto_adjust_threshold.clamp(0.0, 100.0);
        self.error_weight_strength = self.error_weight_strength.max(0.0);
        self.char_sampling_coverage_strength = self.char_sampling_coverage_strength.max(0.0);
        self
    }

    pub fn sequence(&self) -> &[char] {
        if self.custom_sequence.is_empty() {
            crate::morse::LCWO_SEQUENCE
        } else {
            &self.custom_sequence
        }
    }

    pub fn side_tone_center(&self) -> f64 {
        let min = self.side_tone_min.max(100.0);
        let max = self.side_tone_max.max(min);
        min + (max - min) / 2.0
    }

    pub fn band_signature(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{}|{}|{}|{}|{}|{}|{}",
            self.side_tone_min,
            self.side_tone_max,
            self.qsb_enabled,
            self.qsb_depth,
            self.qsb_rate_hz,
            self.qrn_enabled,
            self.qrn_level,
            self.qrm_enabled,
            self.qrm_level,
            self.qrm_profile,
            self.receiver_background_gain,
            self.receiver_background_excitation_rate,
            self.receiver_background_resonance,
            self.receiver_background_decay,
            self.receiver_background_offset_hz,
            self.receiver_background_offset_mod_depth_hz,
            self.receiver_background_offset_mod_rate_hz,
        )
    }
}

mod defaults {
    pub fn enabled() -> bool {
        true
    }
    pub fn qsb_depth() -> f64 {
        0.35
    }
    pub fn qsb_rate_hz() -> f64 {
        0.12
    }
    pub fn qrn_level() -> f64 {
        0.25
    }
    pub fn qrm_level() -> f64 {
        0.2
    }
    pub fn receiver_background_gain() -> f64 {
        20.0
    }
    pub fn receiver_background_excitation_rate() -> f64 {
        62.0
    }
    pub fn receiver_background_resonance() -> f64 {
        66.0
    }
    pub fn receiver_background_decay() -> f64 {
        0.984
    }
    pub fn receiver_background_offset_hz() -> f64 {
        140.0
    }
    pub fn receiver_background_offset_mod_depth_hz() -> f64 {
        45.0
    }
    pub fn receiver_background_offset_mod_rate_hz() -> f64 {
        0.32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_links_and_bounds() {
        let mut s = TrainingSettings::default();
        s.char_wpm_min = 3.0;
        s.char_wpm_max = 200.0;
        s.link_char_wpm = true;
        s.link_char_to_effective = true;
        s.min_group_size = 8;
        s.max_group_size = 2;
        s.link_group_size = true;
        s.level = 99;
        let s = s.clamp();
        assert_eq!(s.char_wpm_min, 5.0);
        assert_eq!(s.char_wpm_max, 5.0);
        assert_eq!(s.effective_wpm_min, 5.0);
        assert_eq!(s.effective_wpm_max, 5.0);
        assert_eq!(s.min_group_size, 8);
        assert_eq!(s.max_group_size, 8);
        assert_eq!(s.level, (s.sequence().len().saturating_sub(1) as u32).max(1));
    }

    #[test]
    fn custom_level_clamps_to_alphabet_length() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Custom;
        s.custom_set = vec!['A', 'B', 'C', 'D'];
        s.level = 99;
        let s = s.clamp();
        assert_eq!(s.level, 3);
        assert_eq!(s.progress_alphabet(), vec!['A', 'B', 'C', 'D']);
    }
}
