use std::ops::RangeInclusive;

use super::{
    MASTER_VOLUME_MAX, MASTER_VOLUME_MIN, RF_GAIN_MAX_DB, RF_GAIN_MIN_DB, RangeSetting,
    RangeValues, SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ, STATIONS_MAX, STATIONS_MIN, STEEPNESS_MAX_MS,
    STEEPNESS_MIN_MS, TrainingSettings, VOLUME_MAX, VOLUME_MIN, WPM_MAX, WPM_MIN,
};
use crate::band::{db_to_gain, full_volume_gain, gain_to_db};

/// How far apart the side tone bounds are pushed when a fixed pitch is opened
/// back up into a range.
const SIDE_TONE_SPREAD_HZ: f64 = 200.0;

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
            self.playback.link_effective_wpm = self.playback.link_char_wpm;
        }
    }

    /// The pitches a station can be on, inside the dial whatever is set: a
    /// preview can run on settings the screen is still being typed into.
    pub fn side_tone_range(&self) -> RangeInclusive<f64> {
        let min = finite_within(self.band.side_tone_min, SIDE_TONE_MIN_HZ, SIDE_TONE_MAX_HZ);
        let max = finite_within(self.band.side_tone_max, min, SIDE_TONE_MAX_HZ);
        min..=max
    }

    /// The levels a station can come in at, inside full scale whatever is
    /// set, and a single level when the range is linked.
    pub fn volume_range(&self) -> RangeInclusive<f64> {
        let min = finite_within(self.band.volume_min, VOLUME_MIN, VOLUME_MAX);
        let max = if self.band.link_volume {
            min
        } else {
            finite_within(self.band.volume_max, min, VOLUME_MAX)
        };
        min..=max
    }

    /// The station strength range in decibels down from full, as it is
    /// shown. A strength is drawn evenly in these ([`Self::volume_at_db`]),
    /// so a station is as likely to come in 3 dB under the top of the range
    /// as 3 dB over its bottom.
    pub fn volume_range_db(&self) -> RangeInclusive<f64> {
        let range = self.volume_range();
        gain_to_db(*range.start())..=gain_to_db(*range.end())
    }

    /// The strength `db` decibels down from full, inside the range.
    pub fn volume_at_db(&self, db: f64) -> f64 {
        let range = self.volume_range();
        db_to_gain(db).clamp(*range.start(), *range.end())
    }

    /// How loud everything plays, as a share of full volume, inside the
    /// control's range whatever is set.
    pub fn master_volume(&self) -> f64 {
        finite_within(
            self.band.master_volume,
            MASTER_VOLUME_MIN,
            MASTER_VOLUME_MAX,
        )
    }

    /// The receiver's AF gain: what the output stage multiplies the band and
    /// the stations by before the limiter, that share of the gain full
    /// volume is for this receiver ([`full_volume_gain`]), at full RF gain.
    /// Your own messages and the paddle's sidetone are monitored, not
    /// received, and are set at the level they are heard at, turned down by
    /// the volume alone.
    pub fn af_gain(&self) -> f64 {
        self.master_volume() * full_volume_gain(self)
    }

    /// The receiver's RF gain, as a gain: what everything reaching the AGC
    /// is multiplied by, one at full, inside the control's range whatever
    /// is set and full for anything that is not a number. Full volume does
    /// not follow it ([`Self::af_gain`]), so backing it off plays the band
    /// quieter, as a rig's RF gain does.
    pub fn rf_gain(&self) -> f64 {
        let db = if self.band.rf_gain_db.is_finite() {
            self.band.rf_gain_db.clamp(RF_GAIN_MIN_DB, RF_GAIN_MAX_DB)
        } else {
            RF_GAIN_MAX_DB
        };
        10f64.powf(db / 20.0)
    }

    /// The middle of the side-tone range: where the receiver is tuned.
    pub fn side_tone_center(&self) -> f64 {
        let range = self.side_tone_range();
        range.start() + (range.end() - range.start()) / 2.0
    }

    /// How long the paddle's sidetone takes to open and to close, in
    /// seconds: the keying envelope's rise time, as a rig shapes its
    /// sidetone with its transmitter's, but never more than a quarter of a
    /// dit at the paddles' speed, so a fast fist's elements stay apart.
    pub fn sidetone_rise_sec(&self) -> f64 {
        let rise = finite_within(self.band.steepness, STEEPNESS_MIN_MS, STEEPNESS_MAX_MS);
        let wpm = finite_within(self.playback.keyer_wpm, WPM_MIN, WPM_MAX);
        let quarter_dit = crate::timing::dot_seconds(wpm) / 4.0;
        (rise / 1_000.0)
            .min(quarter_dit)
            .max(STEEPNESS_MIN_MS / 1_000.0)
    }
}

/// `value` held to `low..=high`, or `low` when it is not a number at all.
fn finite_within(value: f64, low: f64, high: f64) -> f64 {
    if value.is_finite() {
        value.clamp(low, high.max(low))
    } else {
        low
    }
}
