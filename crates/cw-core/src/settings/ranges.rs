use super::{RangeSetting, RangeValues, STATIONS_MAX, STATIONS_MIN, TrainingSettings};

/// How far apart the side tone bounds are pushed when a fixed pitch is opened
/// back up into a range.
const SIDE_TONE_SPREAD_HZ: f64 = 200.0;
const SIDE_TONE_MIN_HZ: f64 = 200.0;
const SIDE_TONE_MAX_HZ: f64 = 1200.0;

impl TrainingSettings {
    pub fn range(&self, which: RangeSetting) -> RangeValues {
        let (min, max, linked) = match which {
            RangeSetting::CharWpm => (
                self.playback.char_wpm_min,
                self.playback.char_wpm_max,
                self.playback.link_char_wpm,
            ),
            RangeSetting::EffectiveWpm => (
                self.playback.effective_wpm_min,
                self.playback.effective_wpm_max,
                self.playback.link_effective_wpm,
            ),
            RangeSetting::GroupSize => (
                f64::from(self.curriculum.min_group_size),
                f64::from(self.curriculum.max_group_size),
                self.curriculum.link_group_size,
            ),
            RangeSetting::GroupRepeat => (
                f64::from(self.playback.group_repeat_min),
                f64::from(self.playback.group_repeat_max),
                self.playback.link_group_repeat,
            ),
            // The side tone has no stored link flag: a single pitch is simply
            // a range with both ends together.
            RangeSetting::SideTone => (
                self.band.side_tone_min,
                self.band.side_tone_max,
                (self.band.side_tone_min - self.band.side_tone_max).abs() < f64::EPSILON,
            ),
            RangeSetting::Volume => (
                self.band.volume_min,
                self.band.volume_max,
                self.band.link_volume,
            ),
            // Like the side tone, the bounds themselves say whether the count
            // is fixed — there is no separate flag to keep in step with them.
            RangeSetting::Stations => (
                f64::from(self.band.stations_min),
                f64::from(self.band.stations_max),
                self.band.stations_min == self.band.stations_max,
            ),
        };
        RangeValues { min, max, linked }
    }

    /// Move the lower bound. The upper bound follows when it would end up below
    /// it, or when the pair is linked.
    pub fn set_range_min(&mut self, which: RangeSetting, value: f64) {
        let current = self.range(which);
        let max = if current.linked || value > current.max {
            value
        } else {
            current.max
        };
        self.write_range(which, value, max);
    }

    /// Move the upper bound, dragging the lower one down if it would overtake it.
    pub fn set_range_max(&mut self, which: RangeSetting, value: f64) {
        let current = self.range(which);
        let min = if current.linked || value < current.min {
            value
        } else {
            current.min
        };
        self.write_range(which, min, value);
    }

    /// Collapse a range to its lower bound, or open a fixed value back up.
    pub fn set_range_linked(&mut self, which: RangeSetting, linked: bool) {
        let current = self.range(which);
        match which {
            RangeSetting::CharWpm => self.playback.link_char_wpm = linked,
            RangeSetting::EffectiveWpm => self.playback.link_effective_wpm = linked,
            RangeSetting::GroupSize => self.curriculum.link_group_size = linked,
            RangeSetting::GroupRepeat => self.playback.link_group_repeat = linked,
            // Nothing to store: the bounds themselves say whether it is fixed.
            RangeSetting::SideTone | RangeSetting::Stations => {}
            RangeSetting::Volume => self.band.link_volume = linked,
        }
        if linked {
            self.write_range(which, current.min, current.min);
        } else if which == RangeSetting::SideTone {
            let max = (current.min + SIDE_TONE_SPREAD_HZ).min(SIDE_TONE_MAX_HZ);
            let min = (max - SIDE_TONE_SPREAD_HZ).max(SIDE_TONE_MIN_HZ);
            self.write_range(which, min, max);
        } else if which == RangeSetting::Stations {
            // Neither of these stores a link flag — the bounds themselves say
            // whether the value is fixed — so opening one back up has to push
            // them apart, or the button would do nothing at all. One station
            // either side is the smallest range that is still a range.
            let max = (current.min + 1.0).min(f64::from(STATIONS_MAX));
            let min = (max - 1.0).max(f64::from(STATIONS_MIN));
            self.write_range(which, min, max);
        }
    }

    fn write_range(&mut self, which: RangeSetting, min: f64, max: f64) {
        let as_u32 = |value: f64| value.max(0.0).round() as u32;
        match which {
            RangeSetting::CharWpm => {
                self.playback.char_wpm_min = min;
                self.playback.char_wpm_max = max;
                self.sync_effective_to_char();
            }
            RangeSetting::EffectiveWpm => {
                self.playback.effective_wpm_min = min;
                self.playback.effective_wpm_max = max;
            }
            RangeSetting::GroupSize => {
                self.curriculum.min_group_size = as_u32(min);
                self.curriculum.max_group_size = as_u32(max);
            }
            RangeSetting::GroupRepeat => {
                self.playback.group_repeat_min = as_u32(min);
                self.playback.group_repeat_max = as_u32(max);
            }
            RangeSetting::SideTone => {
                self.band.side_tone_min = min;
                self.band.side_tone_max = max;
            }
            RangeSetting::Volume => {
                self.band.volume_min = min;
                self.band.volume_max = max;
            }
            RangeSetting::Stations => {
                self.band.stations_min = as_u32(min);
                self.band.stations_max = as_u32(max);
            }
        }
    }

    /// Farnsworth off: effective speed simply mirrors character speed.
    pub fn sync_effective_to_char(&mut self) {
        if self.playback.link_char_to_effective {
            self.playback.effective_wpm_min = self.playback.char_wpm_min;
            self.playback.effective_wpm_max = self.playback.char_wpm_max;
        }
    }

    pub fn side_tone_center(&self) -> f64 {
        let min = self.band.side_tone_min.max(100.0);
        let max = self.band.side_tone_max.max(min);
        min + (max - min) / 2.0
    }

    /// Everything the background layers are built from. Two settings with
    /// the same signature sound the same, so a player only rebuilds its band
    /// when this changes.
    pub fn band_signature(&self) -> String {
        let band = &self.band;
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}",
            band.side_tone_min,
            band.side_tone_max,
            band.qsb_enabled,
            band.qsb_depth,
            band.qsb_rate_hz,
            band.qrn_enabled,
            band.qrn_level,
            band.noise_enabled,
            band.noise_level,
            band.activity_enabled,
            band.activity_level,
            band.agc_enabled,
            band.filter_bandwidth_hz,
            band.filter_shape,
        )
    }
}
