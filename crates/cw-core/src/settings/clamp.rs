use super::{
    ADJUST_COUNT_MAX, ADJUST_THRESHOLD_MAX_PCT, ADJUST_THRESHOLD_MIN_PCT, AGC_THRESHOLD_MAX_DB,
    AGC_THRESHOLD_MIN_DB, CONTEST_MINUTES_MAX, COUNTDOWN_MAX_SEC, COVERAGE_STRENGTH_MAX,
    ERROR_WEIGHT_MAX, EXTRA_WORD_SPACE_MAX, EXTRA_WORD_SPACE_MIN, FILTER_WIDTH_MAX,
    FILTER_WIDTH_MIN, GROUP_PAUSE_MAX_SEC, GROUP_REPEAT_MAX, GROUP_REPEAT_MIN, GROUP_SIZE_MAX,
    GROUP_SIZE_MIN, GROUP_TIMEOUT_MAX_SEC, LETTER_LEVEL_CEILING, MASTER_VOLUME_MAX,
    MASTER_VOLUME_MIN, MY_SENDING_MAX_DB, MY_SENDING_OFF_DB, MY_WPM_MAX, MY_WPM_MIN,
    NUM_GROUPS_MAX, NUM_GROUPS_MIN, PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX,
    PILEUP_SPREAD_MIN, QSB_RATE_MAX_HZ, QSB_RATE_MIN_HZ, RF_GAIN_MAX_DB, RF_GAIN_MIN_DB,
    SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ, STATIONS_MAX, STATIONS_MIN, STEEPNESS_MAX_MS,
    STEEPNESS_MIN_MS, TrainingSettings, VOLUME_MAX, VOLUME_MIN, WPM_MAX, WPM_MIN,
};
use crate::callsign::{CALLSIGN_TIER_MAX, CALLSIGN_TIER_MIN};
use crate::level::LEVEL_MIN;
use crate::morse::MAX_DIGITS_LEVEL;

impl TrainingSettings {
    /// Every number put back inside the range the trainer works in, every
    /// pair the right way round, every link honoured, and the alphabets
    /// cleaned down to unique sendable characters. Settles in one pass, and
    /// never panics: this is the sanitiser everything else trusts.
    ///
    /// Levels are kept inside what any alphabet could carry rather than the
    /// current one, which is read at its own top by
    /// [`TrainingSettings::level`]: trying a shorter alphabet, or another
    /// character set, does not cost a level a longer one reached.
    #[must_use = "clamp returns the clamped settings; it does not change them in place"]
    pub fn clamp(mut self) -> Self {
        self.replace_non_finite();
        let c = &mut self.curriculum;
        c.custom_sequence = Self::unique_alphabet(&c.custom_sequence);
        c.word_list =
            crate::words::WordList::from_words(c.word_list.iter().map(String::as_str)).words;
        c.level = c.level.clamp(LEVEL_MIN, LETTER_LEVEL_CEILING);
        c.digits_level = c.digits_level.clamp(LEVEL_MIN, MAX_DIGITS_LEVEL);
        c.callsign_level = c.callsign_level.clamp(CALLSIGN_TIER_MIN, CALLSIGN_TIER_MAX);
        c.mixed_letters_percent = c.mixed_letters_percent.min(100);
        c.num_groups = c.num_groups.clamp(NUM_GROUPS_MIN, NUM_GROUPS_MAX);
        c.min_group_size = c.min_group_size.clamp(GROUP_SIZE_MIN, GROUP_SIZE_MAX);
        c.max_group_size = c.max_group_size.clamp(c.min_group_size, GROUP_SIZE_MAX);
        if c.link_group_size {
            c.max_group_size = c.min_group_size;
        }

        let p = &mut self.playback;
        p.fist_variation = p.fist_variation.clamp(0.0, 1.0);
        p.char_wpm_min = p.char_wpm_min.clamp(WPM_MIN, WPM_MAX);
        p.char_wpm_max = p.char_wpm_max.clamp(p.char_wpm_min, WPM_MAX);
        if p.link_char_wpm {
            p.char_wpm_max = p.char_wpm_min;
        }
        if p.link_char_to_effective {
            // Tied, the effective range is a copy of the character range,
            // link included, so the two can never disagree about it.
            p.effective_wpm_min = p.char_wpm_min;
            p.effective_wpm_max = p.char_wpm_max;
            p.link_effective_wpm = p.link_char_wpm;
        } else {
            p.effective_wpm_min = p.effective_wpm_min.clamp(WPM_MIN, WPM_MAX);
            p.effective_wpm_max = p.effective_wpm_max.clamp(p.effective_wpm_min, WPM_MAX);
            if p.link_effective_wpm {
                p.effective_wpm_max = p.effective_wpm_min;
            }
        }
        p.keyer_wpm = p.keyer_wpm.clamp(WPM_MIN, WPM_MAX);
        p.group_repeat_min = p.group_repeat_min.clamp(GROUP_REPEAT_MIN, GROUP_REPEAT_MAX);
        p.group_repeat_max = p
            .group_repeat_max
            .clamp(p.group_repeat_min, GROUP_REPEAT_MAX);
        if p.link_group_repeat {
            p.group_repeat_max = p.group_repeat_min;
        }
        p.extra_word_space_multiplier = p
            .extra_word_space_multiplier
            .clamp(EXTRA_WORD_SPACE_MIN, EXTRA_WORD_SPACE_MAX);
        p.group_timeout = p.group_timeout.clamp(0.0, GROUP_TIMEOUT_MAX_SEC);
        p.group_pause_sec = p.group_pause_sec.clamp(0.0, GROUP_PAUSE_MAX_SEC);
        p.countdown_sec = p.countdown_sec.min(COUNTDOWN_MAX_SEC);

        let b = &mut self.band;
        b.filter_width_hz = b.filter_width_hz.clamp(FILTER_WIDTH_MIN, FILTER_WIDTH_MAX);
        b.stations_min = b.stations_min.clamp(STATIONS_MIN, STATIONS_MAX);
        b.stations_max = b.stations_max.clamp(b.stations_min, STATIONS_MAX);
        b.pileup_spread_hz = b
            .pileup_spread_hz
            .clamp(PILEUP_SPREAD_MIN, PILEUP_SPREAD_MAX);
        b.pileup_level_db = b
            .pileup_level_db
            .clamp(PILEUP_LEVEL_MIN_DB, PILEUP_LEVEL_MAX_DB);
        b.side_tone_min = b.side_tone_min.clamp(SIDE_TONE_MIN_HZ, SIDE_TONE_MAX_HZ);
        b.side_tone_max = b.side_tone_max.clamp(b.side_tone_min, SIDE_TONE_MAX_HZ);
        b.volume_min = b.volume_min.clamp(VOLUME_MIN, VOLUME_MAX);
        b.volume_max = b.volume_max.clamp(b.volume_min, VOLUME_MAX);
        if b.link_volume {
            b.volume_max = b.volume_min;
        }
        b.master_volume = b.master_volume.clamp(MASTER_VOLUME_MIN, MASTER_VOLUME_MAX);
        b.steepness = b.steepness.clamp(STEEPNESS_MIN_MS, STEEPNESS_MAX_MS);
        b.envelope_smoothing = b.envelope_smoothing.clamp(0.0, 1.0);
        b.qsb_depth = b.qsb_depth.clamp(0.0, 1.0);
        b.qsb_rate_hz = b.qsb_rate_hz.clamp(QSB_RATE_MIN_HZ, QSB_RATE_MAX_HZ);
        b.qrn_level = b.qrn_level.clamp(0.0, 1.0);
        b.noise_level = crate::band::whole_db_noise_level(b.noise_level.clamp(0.0, 1.0));
        b.agc_threshold_db = b
            .agc_threshold_db
            .clamp(AGC_THRESHOLD_MIN_DB, AGC_THRESHOLD_MAX_DB);
        b.rf_gain_db = b.rf_gain_db.clamp(RF_GAIN_MIN_DB, RF_GAIN_MAX_DB);

        let a = &mut self.auto_level;
        a.auto_adjust_threshold = a
            .auto_adjust_threshold
            .clamp(ADJUST_THRESHOLD_MIN_PCT, ADJUST_THRESHOLD_MAX_PCT);
        a.auto_adjust_below_threshold_count =
            a.auto_adjust_below_threshold_count.min(ADJUST_COUNT_MAX);
        a.auto_adjust_above_threshold_count =
            a.auto_adjust_above_threshold_count.min(ADJUST_COUNT_MAX);
        a.error_weight_strength = a.error_weight_strength.clamp(0.0, ERROR_WEIGHT_MAX);
        a.char_sampling_coverage_strength = a
            .char_sampling_coverage_strength
            .clamp(0.0, COVERAGE_STRENGTH_MAX);

        // The contest, its run style, its callers' habits and how often
        // your TU signs need nothing: each can hold only what it names, and
        // a name read back that this build does not know is the default
        // already. Show copy, flutter, the callers' adaptive speed and full
        // break-in are switches, on or off, and need nothing either.
        let k = &mut self.contest;
        k.contest_minutes = k.contest_minutes.min(CONTEST_MINUTES_MAX);
        // Zero is no speed of your own: the middle of the callers' range.
        if k.my_wpm != 0 {
            k.my_wpm = k.my_wpm.clamp(MY_WPM_MIN, MY_WPM_MAX);
        }
        k.my_sending_db = k.my_sending_db.clamp(MY_SENDING_OFF_DB, MY_SENDING_MAX_DB);
        k.my_call = super::contest::clean_call(&k.my_call);
        k.my_name = super::contest::clean_name(&k.my_name);
        k.my_state = super::contest::clean_state(&k.my_state);
        k.my_zone = k.my_zone.min(super::contest::MY_ZONE_MAX);
        k.my_power = super::contest::clean_power(&k.my_power);
        k.my_class = super::contest::clean_class(&k.my_class);
        k.my_section = super::contest::clean_section(&k.my_section);
        self
    }

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
        fix(&mut b.master_volume, db.master_volume);
        fix(&mut b.steepness, db.steepness);
        fix(&mut b.envelope_smoothing, db.envelope_smoothing);
        fix(&mut b.qsb_depth, db.qsb_depth);
        fix(&mut b.qsb_rate_hz, db.qsb_rate_hz);
        fix(&mut b.qrn_level, db.qrn_level);
        fix(&mut b.noise_level, db.noise_level);
        fix(&mut b.filter_width_hz, db.filter_width_hz);
        fix(&mut b.pileup_spread_hz, db.pileup_spread_hz);
        fix(&mut b.pileup_level_db, db.pileup_level_db);
        fix(&mut b.agc_threshold_db, db.agc_threshold_db);
        fix(&mut b.rf_gain_db, db.rf_gain_db);
        let (a, da) = (&mut self.auto_level, &d.auto_level);
        fix(&mut a.auto_adjust_threshold, da.auto_adjust_threshold);
        fix(&mut a.error_weight_strength, da.error_weight_strength);
        fix(
            &mut a.char_sampling_coverage_strength,
            da.char_sampling_coverage_strength,
        );
    }
}
