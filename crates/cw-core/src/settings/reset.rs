use super::{CharSetMode, SettingsSection, TrainingSettings};
use crate::sequences::SequenceChoice;

impl TrainingSettings {
    /// Put one card back the way it shipped, leaving every other card, and all
    /// of your progress, alone.
    ///
    /// Values come from `Self::default()` rather than being written out again
    /// here, so a changed default reaches the reset button on its own. What
    /// comes back is clamped, so a section put back against the others —
    /// the character set under a level, say — is never left half valid.
    ///
    /// A card's reset puts back the preferences it shows, and not what the
    /// learner made: an unlock order of their own and their word list stay,
    /// as the level does, and the Character set's reset keeps Contest mode
    /// while in it, as the contest is where the card is ([`Self::reset_to`]).
    pub fn reset_section(&mut self, which: SettingsSection) {
        let to = self.reset_to();
        self.put_section(which, &to);
    }

    /// Put one card back as it was in `before`, where a reset of it left
    /// it: what Undo does after [`Self::reset_section`]. Every other card,
    /// changed since or not, is left as it is now.
    pub fn restore_section(&mut self, which: SettingsSection, before: &Self) {
        self.put_section(which, before);
    }

    /// What a reset puts the cards back to: the settings as they ship, but
    /// with what is the learner's own, not a preference, as it is. Contest
    /// mode stays Contest mode, an unlock order of their own stays theirs,
    /// and the word list, which no card's reset touches, stays too.
    fn reset_to(&self) -> Self {
        let mut to = Self::default();
        if self.curriculum.char_set_mode == CharSetMode::Contest {
            to.curriculum.char_set_mode = CharSetMode::Contest;
        }
        if self.sequence_choice() == SequenceChoice::Custom {
            to.curriculum
                .custom_sequence
                .clone_from(&self.curriculum.custom_sequence);
            to.curriculum.sequence_is_custom = self.curriculum.sequence_is_custom;
        }
        to
    }

    /// Put card `which` as it is in `from`, every field it shows, and clamp
    /// what comes of it. The aim is shared, and goes with whichever card
    /// shows it: before, or after, as a switch put back on can bring the
    /// aim onto the card.
    fn put_section(&mut self, which: SettingsSection, from: &Self) {
        let shown = crate::sampling::aim_section(self) == which;
        match which {
            SettingsSection::SessionShape => {
                let (c, p) = (&from.curriculum, &from.playback);
                self.curriculum.num_groups = c.num_groups;
                self.curriculum.min_group_size = c.min_group_size;
                self.curriculum.max_group_size = c.max_group_size;
                self.curriculum.link_group_size = c.link_group_size;
                self.curriculum.adaptive_group_size = c.adaptive_group_size;
                self.playback.group_repeat_min = p.group_repeat_min;
                self.playback.group_repeat_max = p.group_repeat_max;
                self.playback.link_group_repeat = p.link_group_repeat;
                self.playback.group_timeout = p.group_timeout;
                self.playback.group_pause_sec = p.group_pause_sec;
                self.playback.countdown_sec = p.countdown_sec;
                self.playback.lock_input_during_group_playback = p.lock_input_during_group_playback;
                // The key and the paddles are drawn on this card, under the
                // groups, and go back with it.
                self.playback.paddle_swap = p.paddle_swap;
                self.playback.paddle_keys_by_place = p.paddle_keys_by_place;
                self.playback.keyer_mode = p.keyer_mode;
                self.playback.keyer_wpm = p.keyer_wpm;
                self.contest.contest_minutes = from.contest.contest_minutes;
                self.contest.contest_show_copy = from.contest.contest_show_copy;
            }
            SettingsSection::Speed => {
                let p = &from.playback;
                self.playback.char_wpm_min = p.char_wpm_min;
                self.playback.char_wpm_max = p.char_wpm_max;
                self.playback.link_char_wpm = p.link_char_wpm;
                self.playback.effective_wpm_min = p.effective_wpm_min;
                self.playback.effective_wpm_max = p.effective_wpm_max;
                self.playback.link_effective_wpm = p.link_effective_wpm;
                self.playback.link_char_to_effective = p.link_char_to_effective;
                self.playback.adaptive_speed = p.adaptive_speed;
                self.contest.contest_adapt_speed = from.contest.contest_adapt_speed;
                self.playback.extra_word_space_multiplier = p.extra_word_space_multiplier;
                self.playback.fist_variation = p.fist_variation;
                self.contest.my_wpm = from.contest.my_wpm;
            }
            SettingsSection::KeyingEnvelope => {
                self.band.steepness = from.band.steepness;
                self.band.envelope_smoothing = from.band.envelope_smoothing;
            }
            SettingsSection::CharacterSet => {
                let c = &from.curriculum;
                self.curriculum.char_set_mode = c.char_set_mode;
                self.curriculum.mixed_letters_percent = c.mixed_letters_percent;
                self.curriculum
                    .custom_sequence
                    .clone_from(&c.custom_sequence);
                self.curriculum.sequence_is_custom = c.sequence_is_custom;
                self.curriculum.practice_window = c.practice_window;
                self.contest.contest_kind = from.contest.contest_kind;
                self.contest.contest_style = from.contest.contest_style;
                self.contest.contest_tu_call = from.contest.contest_tu_call;
                // A single caller again, or no contest: the stations a
                // pile-up opened go back; a pile-up again opens them.
                self.fit_stations_to_pileup();
            }
            SettingsSection::ToneAndVolume => {
                let b = &from.band;
                self.band.side_tone_min = b.side_tone_min;
                self.band.side_tone_max = b.side_tone_max;
                self.band.volume_min = b.volume_min;
                self.band.volume_max = b.volume_max;
                self.band.link_volume = b.link_volume;
                self.band.master_volume = b.master_volume;
                self.contest.my_sending_db = from.contest.my_sending_db;
                self.contest.contest_qsk = from.contest.contest_qsk;
            }
            SettingsSection::BandConditions => {
                let b = &from.band;
                self.band.qsb_enabled = b.qsb_enabled;
                self.band.qsb_depth = b.qsb_depth;
                self.band.qsb_rate_hz = b.qsb_rate_hz;
                self.band.qrn_enabled = b.qrn_enabled;
                self.band.qrn_level = b.qrn_level;
                self.band.noise_enabled = b.noise_enabled;
                self.band.noise_level = b.noise_level;
                self.band.rf_gain_db = b.rf_gain_db;
                self.band.agc_enabled = b.agc_enabled;
                self.band.agc_decay = b.agc_decay;
                self.band.filter_width_hz = b.filter_width_hz;
                self.band.filter_shape = b.filter_shape;
                self.band.stations_min = b.stations_min;
                self.band.stations_max = b.stations_max;
                self.contest.stations_opened_by_pileup = from.contest.stations_opened_by_pileup;
                self.band.pileup_spread_hz = b.pileup_spread_hz;
                self.band.pileup_level_db = b.pileup_level_db;
                self.contest.contest_habits = from.contest.contest_habits;
                self.contest.contest_flutter = from.contest.contest_flutter;
            }
            SettingsSection::AutoLevel => {
                let a = &from.auto_level;
                self.auto_level.auto_adjust_level = a.auto_adjust_level;
                self.auto_level.auto_adjust_threshold = a.auto_adjust_threshold;
                self.auto_level.auto_adjust_below_threshold_count =
                    a.auto_adjust_below_threshold_count;
                self.auto_level.auto_adjust_above_threshold_count =
                    a.auto_adjust_above_threshold_count;
                self.auto_level.mixed_auto_level_next_axis = a.mixed_auto_level_next_axis;
            }
            SettingsSection::CharacterSampling => {
                let a = &from.auto_level;
                self.auto_level.error_weight_strength = a.error_weight_strength;
                self.auto_level.char_sampling_coverage_strength = a.char_sampling_coverage_strength;
            }
            SettingsSection::Recognition => self.recognition = from.recognition,
            SettingsSection::Station => {
                let k = &from.contest;
                self.contest.my_call.clone_from(&k.my_call);
                self.contest.my_name.clone_from(&k.my_name);
                self.contest.my_state.clone_from(&k.my_state);
                self.contest.my_zone = k.my_zone;
                self.contest.my_power.clone_from(&k.my_power);
                self.contest.my_class.clone_from(&k.my_class);
                self.contest.my_section.clone_from(&k.my_section);
            }
        }
        *self = std::mem::take(self).clamp();
        if shown || crate::sampling::aim_section(self) == which {
            self.curriculum.aim = from.curriculum.aim;
        }
    }
}
