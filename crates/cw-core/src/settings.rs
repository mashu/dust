//! Training settings for the group trainer.

use serde::{Deserialize, Serialize};

use crate::level::{max_level_for_len, LEVEL_MIN};
use crate::morse::{is_digit, morse_for, DEFAULT_SLIDING_WINDOW_END, DEFAULT_SLIDING_WINDOW_START};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MixedAutoLevelAxis {
    Letters,
    Digits,
}

impl MixedAutoLevelAxis {
    pub fn flip(self) -> Self {
        match self {
            Self::Letters => Self::Digits,
            Self::Digits => Self::Letters,
        }
    }
}

impl Default for MixedAutoLevelAxis {
    fn default() -> Self {
        Self::Letters
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PracticeWindow {
    All,
    Last3,
    Last5,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CurriculumSettings {
    /// Progress through the current letter/custom alphabet (level 1 unlocks two characters).
    #[serde(alias = "kochLevel", default = "defaults::level")]
    pub level: u32,
    pub char_set_mode: CharSetMode,
    pub digits_level: u32,
    /// When mixed: 0–100 percent of characters that are letters (rest digits).
    pub mixed_letters_percent: u32,
    pub custom_set: Vec<char>,
    pub custom_sequence: Vec<char>,
    /// True when the user chose Sequence → Custom, even if the order still matches a preset.
    #[serde(default)]
    pub sequence_is_custom: bool,
    /// Named practice window. `None` means infer from the saved start/end (old saves).
    #[serde(default)]
    pub practice_window: Option<PracticeWindow>,
    pub sliding_window_start: u32,
    pub sliding_window_end: u32,
    pub num_groups: u32,
    pub min_group_size: u32,
    pub max_group_size: u32,
    pub link_group_size: bool,
}

impl Default for CurriculumSettings {
    fn default() -> Self {
        Self {
            level: LEVEL_MIN,
            char_set_mode: CharSetMode::Mixed,
            digits_level: 1,
            mixed_letters_percent: 70,
            custom_set: Vec::new(),
            custom_sequence: Vec::new(),
            sequence_is_custom: false,
            practice_window: Some(PracticeWindow::All),
            sliding_window_start: DEFAULT_SLIDING_WINDOW_START,
            sliding_window_end: DEFAULT_SLIDING_WINDOW_END,
            num_groups: 20,
            min_group_size: 3,
            max_group_size: 5,
            link_group_size: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaybackSettings {
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
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BandSettings {
    pub side_tone_min: f64,
    pub side_tone_max: f64,
    pub volume_min: f64,
    pub volume_max: f64,
    pub link_volume: bool,
    pub steepness: f64,
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
}

impl Default for BandSettings {
    fn default() -> Self {
        Self {
            side_tone_min: 400.0,
            side_tone_max: 600.0,
            volume_min: 0.7,
            volume_max: 1.0,
            link_volume: false,
            steepness: 10.0,
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoLevelSettings {
    pub mixed_auto_level_next_axis: MixedAutoLevelAxis,
    #[serde(alias = "autoAdjustKoch", default = "defaults::enabled")]
    pub auto_adjust_level: bool,
    pub auto_adjust_threshold: f64,
    pub auto_adjust_below_threshold_count: u32,
    pub auto_adjust_above_threshold_count: u32,
    pub error_weight_strength: f64,
    pub char_sampling_coverage_strength: f64,
    #[serde(default)]
    pub char_sampling_thompson: bool,
}

impl Default for AutoLevelSettings {
    fn default() -> Self {
        Self {
            mixed_auto_level_next_axis: MixedAutoLevelAxis::Letters,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default = "TrainingSettings::default")]
pub struct TrainingSettings {
    #[serde(flatten)]
    pub curriculum: CurriculumSettings,
    #[serde(flatten)]
    pub playback: PlaybackSettings,
    #[serde(flatten)]
    pub band: BandSettings,
    #[serde(flatten)]
    pub auto_level: AutoLevelSettings,
}

impl Default for TrainingSettings {
    fn default() -> Self {
        Self {
            curriculum: CurriculumSettings::default(),
            playback: PlaybackSettings::default(),
            band: BandSettings::default(),
            auto_level: AutoLevelSettings::default(),
        }
    }
}

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
            _ => self.progress_alphabet(),
        }
    }

    pub fn active_level(&self) -> u32 {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => self.curriculum.digits_level,
            _ => self.curriculum.level,
        }
    }

    pub fn set_active_level(&mut self, value: u32) {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => self.curriculum.digits_level = value,
            _ => self.curriculum.level = value,
        }
    }

    pub fn max_active_level(&self) -> u32 {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => max_level_for_len(crate::morse::DIGITS.len()),
            _ => self.max_letter_level(),
        }
    }

    pub fn clamp(mut self) -> Self {
        let seq_max = self.max_letter_level();
        self.curriculum.level = self.curriculum.level.clamp(LEVEL_MIN, seq_max);
        self.curriculum.digits_level = self
            .curriculum
            .digits_level
            .clamp(LEVEL_MIN, max_level_for_len(crate::morse::DIGITS.len()));
        self.curriculum.mixed_letters_percent = self.curriculum.mixed_letters_percent.min(100);
        self.curriculum.num_groups = self.curriculum.num_groups.clamp(1, 200);
        self.curriculum.min_group_size = self.curriculum.min_group_size.clamp(1, 20);
        self.curriculum.max_group_size = self
            .curriculum
            .max_group_size
            .clamp(self.curriculum.min_group_size, 20);
        if self.curriculum.link_group_size {
            self.curriculum.max_group_size = self.curriculum.min_group_size;
        }
        self.playback.char_wpm_min = self.playback.char_wpm_min.clamp(5.0, 80.0);
        self.playback.char_wpm_max = self
            .playback
            .char_wpm_max
            .clamp(self.playback.char_wpm_min, 80.0);
        if self.playback.link_char_wpm {
            self.playback.char_wpm_max = self.playback.char_wpm_min;
        }
        self.playback.effective_wpm_min = self.playback.effective_wpm_min.clamp(5.0, 80.0);
        self.playback.effective_wpm_max = self
            .playback
            .effective_wpm_max
            .clamp(self.playback.effective_wpm_min, 80.0);
        if self.playback.link_effective_wpm {
            self.playback.effective_wpm_max = self.playback.effective_wpm_min;
        }
        if self.playback.link_char_to_effective {
            self.playback.effective_wpm_min = self.playback.char_wpm_min;
            self.playback.effective_wpm_max = self.playback.char_wpm_max;
        }
        self.band.side_tone_min = self.band.side_tone_min.clamp(200.0, 1200.0);
        self.band.side_tone_max = self
            .band
            .side_tone_max
            .clamp(self.band.side_tone_min, 1200.0);
        self.band.volume_min = self.band.volume_min.clamp(0.1, 1.0);
        self.band.volume_max = self.band.volume_max.clamp(self.band.volume_min, 1.0);
        if self.band.link_volume {
            self.band.volume_max = self.band.volume_min;
        }
        self.band.steepness = self.band.steepness.clamp(1.0, 50.0);
        self.band.envelope_smoothing = self.band.envelope_smoothing.clamp(0.0, 1.0);
        self.band.qsb_depth = self.band.qsb_depth.clamp(0.0, 1.0);
        self.band.qsb_rate_hz = self.band.qsb_rate_hz.clamp(0.03, 1.5);
        self.band.qrn_level = self.band.qrn_level.clamp(0.0, 1.0);
        self.band.qrm_level = self.band.qrm_level.clamp(0.0, 1.0);
        self.band.receiver_background_gain = self.band.receiver_background_gain.clamp(0.0, 20.0);
        self.band.receiver_background_excitation_rate = self
            .band
            .receiver_background_excitation_rate
            .clamp(0.1, 500.0);
        self.band.receiver_background_resonance =
            self.band.receiver_background_resonance.clamp(0.5, 240.0);
        self.band.receiver_background_decay =
            self.band.receiver_background_decay.clamp(0.5, 0.9999);
        self.band.receiver_background_offset_hz = self
            .band
            .receiver_background_offset_hz
            .clamp(-1000.0, 1000.0);
        self.band.receiver_background_offset_mod_depth_hz = self
            .band
            .receiver_background_offset_mod_depth_hz
            .clamp(0.0, 1000.0);
        self.band.receiver_background_offset_mod_rate_hz = self
            .band
            .receiver_background_offset_mod_rate_hz
            .clamp(0.0, 20.0);
        self.playback.extra_word_space_multiplier =
            self.playback.extra_word_space_multiplier.max(0.1);
        self.playback.group_timeout = self.playback.group_timeout.clamp(0.0, 120.0);
        self.auto_level.auto_adjust_threshold =
            self.auto_level.auto_adjust_threshold.clamp(0.0, 100.0);
        self.auto_level.error_weight_strength = self.auto_level.error_weight_strength.max(0.0);
        self.auto_level.char_sampling_coverage_strength =
            self.auto_level.char_sampling_coverage_strength.max(0.0);
        self
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
            _ => self.progress_alphabet().into_iter().collect(),
        }
    }

    pub fn side_tone_center(&self) -> f64 {
        let min = self.band.side_tone_min.max(100.0);
        let max = self.band.side_tone_max.max(min);
        min + (max - min) / 2.0
    }

    pub fn band_signature(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{}|{}|{}|{}|{}|{}|{}",
            self.band.side_tone_min,
            self.band.side_tone_max,
            self.band.qsb_enabled,
            self.band.qsb_depth,
            self.band.qsb_rate_hz,
            self.band.qrn_enabled,
            self.band.qrn_level,
            self.band.qrm_enabled,
            self.band.qrm_level,
            self.band.qrm_profile,
            self.band.receiver_background_gain,
            self.band.receiver_background_excitation_rate,
            self.band.receiver_background_resonance,
            self.band.receiver_background_decay,
            self.band.receiver_background_offset_hz,
            self.band.receiver_background_offset_mod_depth_hz,
            self.band.receiver_background_offset_mod_rate_hz,
        )
    }
}

mod defaults {
    pub fn enabled() -> bool {
        true
    }
    pub fn level() -> u32 {
        crate::level::LEVEL_MIN
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
        s.playback.char_wpm_min = 3.0;
        s.playback.char_wpm_max = 200.0;
        s.playback.link_char_wpm = true;
        s.playback.link_char_to_effective = true;
        s.curriculum.min_group_size = 8;
        s.curriculum.max_group_size = 2;
        s.curriculum.link_group_size = true;
        s.curriculum.level = 99;
        let s = s.clamp();
        assert_eq!(s.playback.char_wpm_min, 5.0);
        assert_eq!(s.playback.char_wpm_max, 5.0);
        assert_eq!(s.playback.effective_wpm_min, 5.0);
        assert_eq!(s.playback.effective_wpm_max, 5.0);
        assert_eq!(s.curriculum.min_group_size, 8);
        assert_eq!(s.curriculum.max_group_size, 8);
        assert_eq!(s.curriculum.level, s.max_letter_level());
    }

    #[test]
    fn custom_level_clamps_to_alphabet_length() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Custom;
        s.curriculum.custom_set = vec!['A', 'B', 'C', 'D'];
        s.curriculum.level = 99;
        let s = s.clamp();
        assert_eq!(s.curriculum.level, 3);
        assert_eq!(s.progress_alphabet(), vec!['A', 'B', 'C', 'D']);
    }

    #[test]
    fn alphabet_fingerprint_tracks_sequence() {
        let mut lcwo = TrainingSettings::default();
        lcwo.curriculum.char_set_mode = CharSetMode::Koch;
        lcwo.curriculum.custom_sequence.clear();
        let mut mania = lcwo.clone();
        mania.curriculum.custom_sequence = crate::sequences::TRADITIONAL_KOCH_SEQUENCE.to_vec();
        assert_ne!(lcwo.alphabet_fingerprint(), mania.alphabet_fingerprint());
        let mut digits = TrainingSettings::default();
        digits.curriculum.char_set_mode = CharSetMode::Digits;
        assert_eq!(digits.alphabet_fingerprint(), "0123456789");
    }

    #[test]
    fn missing_json_fields_use_training_defaults() {
        let s: TrainingSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.curriculum.mixed_letters_percent, 70);
        assert!(s.playback.lock_input_during_group_playback);
        assert!(s.playback.link_char_to_effective);
        assert_eq!(s.playback.group_timeout, 10.0);
        assert_eq!(s.auto_level.auto_adjust_threshold, 90.0);
        assert_eq!(s.curriculum.num_groups, 20);
        assert_eq!(s.curriculum.digits_level, 1);
        assert_eq!(s.playback.char_wpm_min, 18.0);
        let koch: TrainingSettings = serde_json::from_str(r#"{"charSetMode":"koch"}"#).unwrap();
        assert_eq!(koch.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(koch.curriculum.mixed_letters_percent, 70);
        assert!(koch.playback.lock_input_during_group_playback);
    }

    #[test]
    fn nested_settings_roundtrip_keeps_flat_keys() {
        let json = serde_json::to_value(TrainingSettings::default()).unwrap();
        assert!(json.get("numGroups").is_some());
        assert!(json.get("curriculum").is_none());
        assert!(json.get("charWpmMin").is_some());
        assert!(json.get("autoAdjustLevel").is_some());
        let back: TrainingSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back, TrainingSettings::default());
    }
}
