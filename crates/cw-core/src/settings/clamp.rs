use super::{
    TrainingSettings, FILTER_BANDWIDTH_MAX, FILTER_BANDWIDTH_MIN, GROUP_REPEAT_MAX,
    GROUP_REPEAT_MIN, PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX,
    PILEUP_SPREAD_MIN, RECEIVER_MODEL_GAIN_MAX, STATIONS_MAX, STATIONS_MIN,
};
use crate::level::{max_level_for_len, LEVEL_MIN};

impl TrainingSettings {
    pub fn clamp(mut self) -> Self {
        let seq_max = self.max_letter_level();
        self.curriculum.level = self.curriculum.level.clamp(LEVEL_MIN, seq_max);
        self.curriculum.digits_level = self
            .curriculum
            .digits_level
            .clamp(LEVEL_MIN, max_level_for_len(crate::morse::DIGITS.len()));
        self.curriculum.callsign_level = self.curriculum.callsign_level.clamp(
            crate::callsign::CALLSIGN_TIER_MIN,
            crate::callsign::CALLSIGN_TIER_MAX,
        );
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
        self.playback.fist_variation = self.playback.fist_variation.clamp(0.0, 1.0);
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
        self.band.filter_bandwidth_hz = self
            .band
            .filter_bandwidth_hz
            .clamp(FILTER_BANDWIDTH_MIN, FILTER_BANDWIDTH_MAX);
        self.band.stations_min = self.band.stations_min.clamp(STATIONS_MIN, STATIONS_MAX);
        self.band.stations_max = self
            .band
            .stations_max
            .clamp(self.band.stations_min, STATIONS_MAX);
        self.band.pileup_spread_hz = self
            .band
            .pileup_spread_hz
            .clamp(PILEUP_SPREAD_MIN, PILEUP_SPREAD_MAX);
        self.band.pileup_level_db = self
            .band
            .pileup_level_db
            .clamp(PILEUP_LEVEL_MIN_DB, PILEUP_LEVEL_MAX_DB);
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
        self.band.receiver_level = self.band.receiver_level.clamp(0.0, 1.0);
        self.band.receiver_background_gain = self
            .band
            .receiver_background_gain
            .clamp(0.0, RECEIVER_MODEL_GAIN_MAX);
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
        self.playback.group_repeat_min = self
            .playback
            .group_repeat_min
            .clamp(GROUP_REPEAT_MIN, GROUP_REPEAT_MAX);
        self.playback.group_repeat_max = self
            .playback
            .group_repeat_max
            .clamp(self.playback.group_repeat_min, GROUP_REPEAT_MAX);
        if self.playback.link_group_repeat {
            self.playback.group_repeat_max = self.playback.group_repeat_min;
        }
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
}
