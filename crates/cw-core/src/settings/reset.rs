use super::{SettingsSection, TrainingSettings};

impl TrainingSettings {
    /// Put one card back the way it shipped, leaving every other card, and all
    /// of your progress, alone.
    ///
    /// Values come from `Self::default()` rather than being written out again
    /// here, so a changed default reaches the reset button on its own.
    pub fn reset_section(&mut self, which: SettingsSection) {
        let d = Self::default();
        match which {
            SettingsSection::SessionShape => {
                self.curriculum.num_groups = d.curriculum.num_groups;
                self.curriculum.min_group_size = d.curriculum.min_group_size;
                self.curriculum.max_group_size = d.curriculum.max_group_size;
                self.curriculum.link_group_size = d.curriculum.link_group_size;
                self.playback.group_repeat_min = d.playback.group_repeat_min;
                self.playback.group_repeat_max = d.playback.group_repeat_max;
                self.playback.link_group_repeat = d.playback.link_group_repeat;
                self.playback.group_timeout = d.playback.group_timeout;
                self.playback.group_pause_sec = d.playback.group_pause_sec;
                self.playback.lock_input_during_group_playback =
                    d.playback.lock_input_during_group_playback;
                self.playback.paddle_swap = d.playback.paddle_swap;
                self.playback.keyer_mode = d.playback.keyer_mode;
                self.playback.keyer_wpm = d.playback.keyer_wpm;
            }
            SettingsSection::Speed => {
                self.playback.char_wpm_min = d.playback.char_wpm_min;
                self.playback.char_wpm_max = d.playback.char_wpm_max;
                self.playback.link_char_wpm = d.playback.link_char_wpm;
                self.playback.effective_wpm_min = d.playback.effective_wpm_min;
                self.playback.effective_wpm_max = d.playback.effective_wpm_max;
                self.playback.link_effective_wpm = d.playback.link_effective_wpm;
                self.playback.link_char_to_effective = d.playback.link_char_to_effective;
                self.playback.extra_word_space_multiplier = d.playback.extra_word_space_multiplier;
                self.playback.fist_variation = d.playback.fist_variation;
            }
            SettingsSection::KeyingEnvelope => {
                self.band.steepness = d.band.steepness;
                self.band.envelope_smoothing = d.band.envelope_smoothing;
            }
            SettingsSection::CharacterSet => {
                self.curriculum.char_set_mode = d.curriculum.char_set_mode;
                self.curriculum.mixed_letters_percent = d.curriculum.mixed_letters_percent;
                self.curriculum.custom_set = d.curriculum.custom_set.clone();
                self.curriculum.custom_sequence = d.curriculum.custom_sequence.clone();
                self.curriculum.sequence_is_custom = d.curriculum.sequence_is_custom;
                self.curriculum.practice_window = d.curriculum.practice_window;
                self.curriculum.sliding_window_start = d.curriculum.sliding_window_start;
                self.curriculum.sliding_window_end = d.curriculum.sliding_window_end;
            }
            SettingsSection::ToneAndVolume => {
                self.band.side_tone_min = d.band.side_tone_min;
                self.band.side_tone_max = d.band.side_tone_max;
                self.band.volume_min = d.band.volume_min;
                self.band.volume_max = d.band.volume_max;
                self.band.link_volume = d.band.link_volume;
            }
            SettingsSection::BandConditions => {
                self.band.qsb_enabled = d.band.qsb_enabled;
                self.band.qsb_depth = d.band.qsb_depth;
                self.band.qsb_rate_hz = d.band.qsb_rate_hz;
                self.band.qrn_enabled = d.band.qrn_enabled;
                self.band.qrn_level = d.band.qrn_level;
                self.band.noise_enabled = d.band.noise_enabled;
                self.band.noise_level = d.band.noise_level;
                self.band.activity_enabled = d.band.activity_enabled;
                self.band.activity_level = d.band.activity_level;
                self.band.agc_enabled = d.band.agc_enabled;
                self.band.filter_bandwidth_hz = d.band.filter_bandwidth_hz;
                self.band.filter_shape = d.band.filter_shape;
                self.band.stations_min = d.band.stations_min;
                self.band.stations_max = d.band.stations_max;
                self.band.pileup_spread_hz = d.band.pileup_spread_hz;
                self.band.pileup_level_db = d.band.pileup_level_db;
            }
            SettingsSection::AutoLevel => {
                self.auto_level.auto_adjust_level = d.auto_level.auto_adjust_level;
                self.auto_level.auto_adjust_threshold = d.auto_level.auto_adjust_threshold;
                self.auto_level.auto_adjust_below_threshold_count =
                    d.auto_level.auto_adjust_below_threshold_count;
                self.auto_level.auto_adjust_above_threshold_count =
                    d.auto_level.auto_adjust_above_threshold_count;
                self.auto_level.mixed_auto_level_next_axis =
                    d.auto_level.mixed_auto_level_next_axis;
            }
            SettingsSection::CharacterSampling => {
                self.auto_level.error_weight_strength = d.auto_level.error_weight_strength;
                self.auto_level.char_sampling_coverage_strength =
                    d.auto_level.char_sampling_coverage_strength;
                self.auto_level.char_sampling_thompson = d.auto_level.char_sampling_thompson;
            }
        }
    }
}
