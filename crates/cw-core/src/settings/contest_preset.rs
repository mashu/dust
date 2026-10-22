//! Starting points for three kinds of operator in Contest mode: how fast
//! the callers come, what calls they have, how they operate, how many call
//! at once, and what the band does to them.
//!
//! As with the Experience presets ([`super::Experience`]), a preset is
//! applied once, through the same calls the settings screen makes, and no
//! level of it is stored: the settings are on a preset for exactly as long
//! as everything it set is still what it set. All it sets is shared with
//! the other modes but the run style and the callers' habits, which are the
//! contest's own.

use super::experience::{holds, put};
use super::{LevelAxis, RangeSetting, TrainingSettings};
use crate::contest::{CallerHabits, RunStyle};

/// A kind of operator, and the contest settings that suit them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContestPreset {
    /// A first contest QSO: one caller at a time, slow and with a plain
    /// call and plain numbers, on a quiet band.
    FirstQso,
    /// A club operator's few contests a year: callers at an everyday
    /// speed, sending numbers as contesters do, a small pile-up, calls from
    /// anywhere, and the band as it usually is.
    Club,
    /// A contester's: callers fast and with every shape of call, careless
    /// ones among them, a pile-up of up to five, stations on your frequency
    /// that are not calling you, and a band that fades deep and crackles.
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
    /// characters and their spacing alike; you send at the middle of it,
    /// your own speed set back to follow it.
    pub fn char_wpm(self) -> (f64, f64) {
        match self {
            Self::FirstQso => (15.0, 18.0),
            Self::Club => (20.0, 26.0),
            Self::Contester => (28.0, 36.0),
        }
    }

    /// The callsign tier the callers' calls come from: the plainest shape
    /// first, every shape, portables and stations abroad among them, last.
    pub fn tier(self) -> u32 {
        match self {
            Self::FirstQso => 1,
            Self::Club => 3,
            Self::Contester => 6,
        }
    }

    pub fn style(self) -> RunStyle {
        match self {
            Self::FirstQso => RunStyle::Single,
            Self::Club | Self::Contester => RunStyle::Pileup,
        }
    }

    /// How the callers operate: plain numbers for a first QSO, as
    /// contesters send them at a club's pace, and with careless operators
    /// and QRM on your frequency for a contester.
    pub fn habits(self) -> CallerHabits {
        match self {
            Self::FirstQso => CallerHabits::Easy,
            Self::Club => CallerHabits::Real,
            Self::Contester => CallerHabits::Rough,
        }
    }

    /// The Band card's stations range, fewest and most: the callers at once
    /// in a pile-up, the band's own traffic for a single caller, where one
    /// station leaves the band empty.
    pub fn stations(self) -> (u32, u32) {
        match self {
            Self::FirstQso => (1, 1),
            Self::Club => (1, 3),
            Self::Contester => (2, 5),
        }
    }

    /// How deep the band fades, if it fades at all.
    pub fn qsb_depth(self) -> Option<f64> {
        match self {
            Self::FirstQso => None,
            Self::Club => Some(0.35),
            Self::Contester => Some(0.5),
        }
    }

    /// How loud the band's static crashes are, if it has any.
    pub fn qrn_level(self) -> Option<f64> {
        match self {
            Self::FirstQso => None,
            Self::Club => Some(0.25),
            Self::Contester => Some(0.4),
        }
    }

    /// Put the settings on this preset: the callers' speed and yours,
    /// their calls and their habits, the run style and how many call, and
    /// the band's fading and static, the way the settings screen sets each.
    /// The spacing goes with the characters: tied, it follows them; with
    /// Farnsworth spacing set, the same range goes there, as callers in a
    /// contest space at the speed they key. Your speed goes back to the
    /// middle of the callers' range, no speed of your own: so the settings
    /// a preset left before your speed was a setting are on it still.
    /// Whether the callers' speed follows your copy within that range is
    /// yours, and left as it is: a preset says how fast the callers may
    /// come, not how they are drawn in it.
    pub fn apply(self, settings: &mut TrainingSettings) {
        let (slowest, fastest) = self.char_wpm();
        put(settings, RangeSetting::CharWpm, slowest, fastest);
        put(settings, RangeSetting::EffectiveWpm, slowest, fastest);
        settings.contest.my_wpm = 0;
        settings.set_level(LevelAxis::Callsign, self.tier());
        settings.set_contest_style(self.style());
        settings.contest.contest_habits = self.habits();
        // Both ends at once: the stations range has no link of its own, its
        // bounds alone saying whether it is fixed, so moving one end at a
        // time could carry the other with it.
        let (fewest, most) = self.stations();
        settings.band.stations_min = fewest;
        settings.band.stations_max = most;
        let band = &mut settings.band;
        band.qsb_enabled = self.qsb_depth().is_some();
        band.qsb_depth = self.qsb_depth().unwrap_or(band.qsb_depth);
        band.qrn_enabled = self.qrn_level().is_some();
        band.qrn_level = self.qrn_level().unwrap_or(band.qrn_level);
    }

    /// The preset the settings are on: the one whose every setting they
    /// still hold, if there is one. A fading or static switched off holds
    /// whatever its depth or level, as the band does not hear them then.
    pub fn of(settings: &TrainingSettings) -> Option<Self> {
        let band = &settings.band;
        let level = |on: bool, value: f64, wanted: Option<f64>| match wanted {
            Some(wanted) => on && (value - wanted).abs() < 1e-9,
            None => !on,
        };
        Self::ALL.into_iter().find(|preset| {
            let (slowest, fastest) = preset.char_wpm();
            holds(settings.range(RangeSetting::CharWpm), slowest, fastest)
                && holds(settings.range(RangeSetting::EffectiveWpm), slowest, fastest)
                && settings.contest.my_wpm == 0
                && settings.level(LevelAxis::Callsign) == preset.tier()
                && settings.contest.contest_style == preset.style()
                && settings.contest.contest_habits == preset.habits()
                && (band.stations_min, band.stations_max) == preset.stations()
                && level(band.qsb_enabled, band.qsb_depth, preset.qsb_depth())
                && level(band.qrn_enabled, band.qrn_level, preset.qrn_level())
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
            (
                ContestPreset::FirstQso,
                range(15.0, 18.0),
                1,
                RunStyle::Single,
                CallerHabits::Easy,
                (1, 1),
                None,
                None,
            ),
            (
                ContestPreset::Club,
                range(20.0, 26.0),
                3,
                RunStyle::Pileup,
                CallerHabits::Real,
                (1, 3),
                Some(0.35),
                Some(0.25),
            ),
            (
                ContestPreset::Contester,
                range(28.0, 36.0),
                6,
                RunStyle::Pileup,
                CallerHabits::Rough,
                (2, 5),
                Some(0.5),
                Some(0.4),
            ),
        ];
        for (preset, speed, tier, style, habits, stations, qsb, qrn) in cases {
            let mut settings = contest();
            settings.contest.my_wpm = 40;
            preset.apply(&mut settings);
            assert_eq!(settings.range(RangeSetting::CharWpm), speed, "{preset:?}");
            assert_eq!(settings.contest.my_wpm, 0, "{preset:?}: your speed follows");
            assert_eq!(settings.my_wpm(), (speed.min + speed.max) / 2.0);
            assert_eq!(settings.level(LevelAxis::Callsign), tier, "{preset:?}");
            assert_eq!(settings.active_level(), tier, "the tier is Contest's level");
            assert_eq!(settings.contest.contest_style, style, "{preset:?}");
            assert_eq!(settings.contest.contest_habits, habits, "{preset:?}");
            let band = &settings.band;
            assert_eq!((band.stations_min, band.stations_max), stations);
            assert_eq!(
                band.qsb_enabled.then_some(band.qsb_depth),
                qsb,
                "{preset:?}"
            );
            assert_eq!(
                band.qrn_enabled.then_some(band.qrn_level),
                qrn,
                "{preset:?}"
            );
            assert_eq!(ContestPreset::of(&settings), Some(preset));
            assert_eq!(
                settings.clone().clamp(),
                settings,
                "{preset:?} is out of bounds"
            );
        }
    }

    /// Whichever preset the settings were on, another one lands whole, a
    /// stations range moved past its old ends among it.
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

    /// The rest of the settings are left as they were: your station, the
    /// contest, the other levels, the groups and the rest of the band.
    #[test]
    fn a_preset_leaves_everything_else_alone() {
        let mut settings = contest();
        settings.contest.contest_kind = ContestKind::Sprint;
        settings.contest.my_call = "K1ABC".to_string();
        settings.contest.contest_minutes = 25;
        settings.curriculum.level = 9;
        settings.curriculum.digits_level = 4;
        settings.curriculum.num_groups = 33;
        settings.band.side_tone_min = 550.0;
        settings.band.side_tone_max = 650.0;
        settings.band.qsb_rate_hz = 0.4;
        settings.band.qsb_depth = 0.8;
        settings.band.noise_level = 0.9;
        settings.band.pileup_spread_hz = 200.0;
        settings.contest.contest_adapt_speed = false;
        for preset in ContestPreset::ALL {
            let mut applied = settings.clone();
            preset.apply(&mut applied);
            assert_eq!(applied.contest.contest_kind, ContestKind::Sprint);
            assert_eq!(applied.contest.my_call, "K1ABC");
            assert_eq!(applied.contest.contest_minutes, 25);
            assert_eq!(applied.curriculum.char_set_mode, CharSetMode::Contest);
            assert_eq!(
                [LevelAxis::Letters, LevelAxis::Digits].map(|axis| applied.level(axis)),
                [9, 4],
                "{preset:?} moved a level"
            );
            assert_eq!(applied.curriculum.num_groups, 33);
            assert_eq!(
                applied.playback.countdown_sec,
                settings.playback.countdown_sec
            );
            let band = &applied.band;
            assert_eq!((band.side_tone_min, band.side_tone_max), (550.0, 650.0));
            assert_eq!(band.qsb_rate_hz, 0.4);
            assert_eq!(band.noise_level, 0.9);
            assert_eq!(band.pileup_spread_hz, 200.0);
            assert!(!applied.contest.contest_adapt_speed, "{preset:?}");
        }
        // Switched off, the fading keeps its depth for when it is on again.
        let mut first = settings;
        ContestPreset::FirstQso.apply(&mut first);
        assert_eq!(first.band.qsb_depth, 0.8);
    }

    /// A change to anything a preset set takes the settings off it, however
    /// small; a change to what it leaves alone does not.
    #[test]
    fn a_change_to_what_a_preset_set_drops_it() {
        let mut settings = contest();
        ContestPreset::Club.apply(&mut settings);
        settings.contest.contest_kind = ContestKind::SimpleQso;
        settings.band.side_tone_max = 900.0;
        settings.band.qsb_rate_hz = 0.5;
        settings.contest.contest_adapt_speed = !settings.contest.contest_adapt_speed;
        assert_eq!(ContestPreset::of(&settings), Some(ContestPreset::Club));
        // Each a change made as a screen makes it.
        type Change = fn(&mut TrainingSettings);
        let changes: [(&str, Change); 12] = [
            ("speed", |s| s.set_range_min(RangeSetting::CharWpm, 21.0)),
            ("your speed", |s| s.contest.my_wpm = 23),
            ("fixed speed", |s| {
                s.set_range_linked(RangeSetting::CharWpm, true)
            }),
            ("tier", |s| s.set_active_level(4)),
            ("style", |s| s.set_contest_style(RunStyle::Single)),
            ("easier habits", |s| {
                s.contest.contest_habits = CallerHabits::Easy;
            }),
            ("rougher habits", |s| {
                s.contest.contest_habits = CallerHabits::Rough;
            }),
            ("stations", |s| s.band.stations_max = 4),
            ("fading off", |s| s.band.qsb_enabled = false),
            ("fading depth", |s| s.band.qsb_depth = 0.36),
            ("static off", |s| s.band.qrn_enabled = false),
            ("static level", |s| s.band.qrn_level = 0.3),
        ];
        for (what, change) in changes {
            let mut changed = settings.clone();
            change(&mut changed);
            assert_eq!(ContestPreset::of(&changed), None, "{what}");
        }
        // With the fading off, its depth is nobody's: First QSO holds.
        let mut first = contest();
        ContestPreset::FirstQso.apply(&mut first);
        first.band.qsb_depth = 0.9;
        first.band.qrn_level = 0.9;
        assert_eq!(ContestPreset::of(&first), Some(ContestPreset::FirstQso));
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
