use super::{
    FILTER_BANDWIDTH_MAX, FILTER_BANDWIDTH_MIN, GROUP_REPEAT_MAX, GROUP_REPEAT_MIN,
    PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN, STATIONS_MAX,
    STATIONS_MIN, TrainingSettings,
};
use crate::level::{LEVEL_MIN, max_level_for_len};

impl TrainingSettings {
    /// Every number put back inside the range the trainer works in, every
    /// pair the right way round, every link honoured. Settles in one pass, and
    /// never panics: this is the sanitiser everything else trusts.
    #[must_use = "clamp returns the clamped settings; it does not change them in place"]
    pub fn clamp(mut self) -> Self {
        self.replace_non_finite();
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
        self.playback.keyer_wpm = self.playback.keyer_wpm.clamp(5.0, 80.0);
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
        self.band.noise_level = self.band.noise_level.clamp(0.0, 1.0);
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
        self.playback.group_pause_sec = self.playback.group_pause_sec.clamp(0.0, 15.0);
        self.auto_level.auto_adjust_threshold =
            self.auto_level.auto_adjust_threshold.clamp(0.0, 100.0);
        self.auto_level.error_weight_strength = self.auto_level.error_weight_strength.max(0.0);
        self.auto_level.char_sampling_coverage_strength =
            self.auto_level.char_sampling_coverage_strength.max(0.0);
        self
    }
}

impl TrainingSettings {
    /// NaN and infinity have no place in a setting, and NaN in particular
    /// breaks every range below it: `f64::clamp` panics when its lower bound
    /// is NaN, which is exactly what a NaN minimum hands the maximum after it.
    /// Neither can come out of a JSON save, but this is the function that
    /// makes settings safe, so it does not get to assume they already are.
    fn replace_non_finite(&mut self) {
        let d = Self::default();
        let fix = |value: &mut f64, fallback: f64| {
            if !value.is_finite() {
                *value = fallback;
            }
        };
        let (p, dp) = (&mut self.playback, &d.playback);
        fix(&mut p.char_wpm_min, dp.char_wpm_min);
        fix(&mut p.char_wpm_max, dp.char_wpm_max);
        fix(&mut p.effective_wpm_min, dp.effective_wpm_min);
        fix(&mut p.effective_wpm_max, dp.effective_wpm_max);
        fix(
            &mut p.extra_word_space_multiplier,
            dp.extra_word_space_multiplier,
        );
        fix(&mut p.group_timeout, dp.group_timeout);
        fix(&mut p.group_pause_sec, dp.group_pause_sec);
        fix(&mut p.keyer_wpm, dp.keyer_wpm);
        fix(&mut p.fist_variation, dp.fist_variation);
        let (b, db) = (&mut self.band, &d.band);
        fix(&mut b.side_tone_min, db.side_tone_min);
        fix(&mut b.side_tone_max, db.side_tone_max);
        fix(&mut b.volume_min, db.volume_min);
        fix(&mut b.volume_max, db.volume_max);
        fix(&mut b.steepness, db.steepness);
        fix(&mut b.envelope_smoothing, db.envelope_smoothing);
        fix(&mut b.qsb_depth, db.qsb_depth);
        fix(&mut b.qsb_rate_hz, db.qsb_rate_hz);
        fix(&mut b.qrn_level, db.qrn_level);
        fix(&mut b.noise_level, db.noise_level);
        fix(&mut b.filter_bandwidth_hz, db.filter_bandwidth_hz);
        fix(&mut b.pileup_spread_hz, db.pileup_spread_hz);
        fix(&mut b.pileup_level_db, db.pileup_level_db);
        let (a, da) = (&mut self.auto_level, &d.auto_level);
        fix(&mut a.auto_adjust_threshold, da.auto_adjust_threshold);
        fix(&mut a.error_weight_strength, da.error_weight_strength);
        fix(
            &mut a.char_sampling_coverage_strength,
            da.char_sampling_coverage_strength,
        );
    }
}
