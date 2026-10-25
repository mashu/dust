//! Starting points for three kinds of operator in Contest mode: how fast
//! the callers come. That and nothing more.
//!
//! As with the Experience presets ([`super::Experience`]), a preset is
//! applied once, through the same calls the settings screen makes, and no
//! level of it is stored: the settings are on a preset for exactly as long
//! as everything it set is still what it set. Everything else is the
//! learner's own and left as it is: the callsign tier, which is a level and
//! shared with Callsigns; the run style and how the callers operate; the
//! band, which group training hears too; and your own speed, which the run
//! screen keeps from run to run.

use super::experience::{holds, put};
use super::{RangeSetting, TrainingSettings};

/// A kind of operator, and the speed the callers come at for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContestPreset {
    /// A first contest QSO: callers slow enough to copy every call.
    FirstQso,
    /// A club operator's few contests a year: callers at an everyday
    /// speed.
    Club,
    /// A contester's: callers fast.
    Contester,
}

impl ContestPreset {
    pub const ALL: [Self; 3] = [Self::FirstQso, Self::Club, Self::Contester];

    pub fn name(self) -> &'static str {
        match self {
            Self::FirstQso => "First QSO",
            Self::Club => "Club",
            Self::Contester => "Contester",
        }
    }

    /// The callers' speed, slowest and fastest, in words a minute, their
    /// characters and their spacing alike.
    pub fn char_wpm(self) -> (f64, f64) {
        match self {
            Self::FirstQso => (15.0, 18.0),
            Self::Club => (20.0, 26.0),
            Self::Contester => (28.0, 36.0),
        }
    }

    /// Put the settings on this preset: the callers' speed, the way the
    /// settings screen sets it.
    /// The spacing goes with the characters: tied, it follows them; with
    /// Farnsworth spacing set, the same range goes there, as callers in a
    /// contest space at the speed they key. Your own speed is left as it
    /// is: until you set one it is the middle of the callers' range, and
    /// follows it.
    pub fn apply(self, settings: &mut TrainingSettings) {
        let (slowest, fastest) = self.char_wpm();
        put(settings, RangeSetting::CharWpm, slowest, fastest);
        put(settings, RangeSetting::EffectiveWpm, slowest, fastest);
    }

    /// The preset the settings are on: the one whose every setting they
    /// still hold, if there is one.
    pub fn of(settings: &TrainingSettings) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| {
            let (slowest, fastest) = preset.char_wpm();
            holds(settings.range(RangeSetting::CharWpm), slowest, fastest)
                && holds(settings.range(RangeSetting::EffectiveWpm), slowest, fastest)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ContestPreset;
    use crate::contest::{CallerHabits, ContestKind, RunStyle};
    use crate::settings::{CharSetMode, LevelAxis, RangeSetting, RangeValues, TrainingSettings};

    /// Settings in Contest mode, as a learner would find them.
    fn contest() -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        settings
    }

    #[test]
    fn each_preset_sets_what_it_says() {
        let range = |min, max| RangeValues {
            min,
            max,
            linked: false,
        };
        let cases = [
            (ContestPreset::FirstQso, range(15.0, 18.0)),
            (ContestPreset::Club, range(20.0, 26.0)),
            (ContestPreset::Contester, range(28.0, 36.0)),
        ];
        for (preset, speed) in cases {
            let mut settings = contest();
            preset.apply(&mut settings);
            assert_eq!(settings.range(RangeSetting::CharWpm), speed, "{preset:?}");
            assert_eq!(
                settings.range(RangeSetting::EffectiveWpm),
                speed,
                "{preset:?}"
            );
            assert_eq!(settings.my_wpm(), (speed.min + speed.max) / 2.0);
            assert_eq!(ContestPreset::of(&settings), Some(preset));
            assert_eq!(
                settings.clone().clamp(),
                settings,
                "{preset:?} is out of bounds"
            );
        }
    }

    /// Whichever preset the settings were on, another one lands whole.
    #[test]
    fn any_preset_lands_whole_from_any_other() {
        for from in ContestPreset::ALL {
            for to in ContestPreset::ALL {
                let mut settings = contest();
                from.apply(&mut settings);
                to.apply(&mut settings);
                assert_eq!(ContestPreset::of(&settings), Some(to), "{from:?} to {to:?}");
            }
        }
    }

    /// The callers' speed is all a preset sets: with every setting moved
    /// off its default, each preset leaves every other one where it was.
    /// The tier, the run style, the callers' habits, the band and your own
    /// speed are the learner's own, and a preset used to set them all.
    #[test]
    fn a_preset_sets_the_callers_speed_and_nothing_more() {
        let mut moved = crate::settings::tests::wildly_different();
        moved.curriculum.char_set_mode = CharSetMode::Contest;
        for preset in ContestPreset::ALL {
            let mut applied = moved.clone();
            preset.apply(&mut applied);
            let p = &mut applied.playback;
            p.char_wpm_min = moved.playback.char_wpm_min;
            p.char_wpm_max = moved.playback.char_wpm_max;
            p.link_char_wpm = moved.playback.link_char_wpm;
            p.effective_wpm_min = moved.playback.effective_wpm_min;
            p.effective_wpm_max = moved.playback.effective_wpm_max;
            p.link_effective_wpm = moved.playback.link_effective_wpm;
            assert_eq!(applied, moved, "{preset:?} set more than it says");
        }
    }

    /// The rest of the settings are left as they were: your station, the
    /// contest, its callers, every level, the groups and the band.
    #[test]
    fn a_preset_leaves_everything_else_alone() {
        let mut settings = contest();
        settings.contest.contest_kind = ContestKind::Sprint;
        settings.contest.contest_style = RunStyle::Pileup;
        settings.contest.contest_habits = CallerHabits::Rough;
        settings.contest.my_call = "K1ABC".to_string();
        settings.contest.my_wpm = 40;
        settings.curriculum.callsign_level = 5;
        settings.band.stations_max = 4;
        settings.band.qsb_enabled = false;
        for preset in ContestPreset::ALL {
            let mut applied = settings.clone();
            preset.apply(&mut applied);
            assert_eq!(applied.contest.contest_kind, ContestKind::Sprint);
            assert_eq!(applied.contest.contest_style, RunStyle::Pileup);
            assert_eq!(applied.contest.contest_habits, CallerHabits::Rough);
            assert_eq!(applied.contest.my_call, "K1ABC");
            assert_eq!(applied.contest.my_wpm, 40, "{preset:?}: your speed");
            assert_eq!(
                applied.level(LevelAxis::Callsign),
                5,
                "{preset:?} moved the tier"
            );
            assert_eq!(applied.band, settings.band, "{preset:?} changed the band");
        }
    }

    /// A change to the callers' speed takes the settings off a preset,
    /// however small; a change to anything else does not.
    #[test]
    fn a_change_to_what_a_preset_set_drops_it() {
        let mut settings = contest();
        ContestPreset::Club.apply(&mut settings);
        settings.contest.contest_kind = ContestKind::SimpleQso;
        settings.band.side_tone_max = 900.0;
        settings.band.qsb_enabled = !settings.band.qsb_enabled;
        settings.band.stations_max = 4;
        settings.contest.my_wpm = 23;
        settings.contest.contest_habits = CallerHabits::Rough;
        settings.set_contest_style(RunStyle::Pileup);
        settings.set_active_level(4);
        assert_eq!(ContestPreset::of(&settings), Some(ContestPreset::Club));
        // Each a change made as a screen makes it.
        type Change = fn(&mut TrainingSettings);
        let changes: [(&str, Change); 3] = [
            ("speed", |s| s.set_range_min(RangeSetting::CharWpm, 21.0)),
            ("fixed speed", |s| {
                s.set_range_linked(RangeSetting::CharWpm, true)
            }),
            ("spacing", |s| {
                s.playback.link_char_to_effective = false;
                s.set_range_min(RangeSetting::EffectiveWpm, 18.0);
            }),
        ];
        for (what, change) in changes {
            let mut changed = settings.clone();
            change(&mut changed);
            assert_eq!(ContestPreset::of(&changed), None, "{what}");
        }
    }

    #[test]
    fn the_settings_as_they_ship_are_on_no_preset() {
        assert_eq!(ContestPreset::of(&contest()), None);
    }

    /// With the speeds tied, the callers' spacing follows the character
    /// speed a preset sets, as it does when the screen sets it.
    #[test]
    fn a_tied_effective_speed_follows() {
        let mut settings = contest();
        assert!(settings.playback.link_char_to_effective);
        ContestPreset::Contester.apply(&mut settings);
        let effective = settings.range(RangeSetting::EffectiveWpm);
        assert_eq!((effective.min, effective.max), (28.0, 36.0));
    }

    /// With Farnsworth spacing set, a preset puts its speed in the spacing
    /// too, so callers keying 28–36 WPM space at it, and a spacing changed
    /// after takes the settings off the preset.
    #[test]
    fn farnsworth_spacing_takes_the_presets_speed_too() {
        let mut settings = contest();
        settings.playback.link_char_to_effective = false;
        settings.set_range_min(RangeSetting::EffectiveWpm, 10.0);
        settings.set_range_max(RangeSetting::EffectiveWpm, 12.0);
        for preset in ContestPreset::ALL {
            let mut applied = settings.clone();
            preset.apply(&mut applied);
            let (slowest, fastest) = preset.char_wpm();
            let effective = applied.range(RangeSetting::EffectiveWpm);
            assert_eq!((effective.min, effective.max), (slowest, fastest));
            assert!(!applied.playback.link_char_to_effective, "still your own");
            assert_eq!(ContestPreset::of(&applied), Some(preset));
            assert_eq!(applied.clone().clamp(), applied, "{preset:?}");
            applied.set_range_min(RangeSetting::EffectiveWpm, slowest - 2.0);
            assert_eq!(ContestPreset::of(&applied), None, "{preset:?}");
        }
    }
}
