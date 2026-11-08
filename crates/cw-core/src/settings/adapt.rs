use super::{CharSetMode, SettingsSection, TrainingSettings};
use crate::score::copy_speed;

/// Whether `settings` have each group's speed, or each new caller's in a
/// contest, chosen by adaptive speed.
pub fn adapts_speed(settings: &TrainingSettings) -> bool {
    adaptive_speed_path(settings).is_some()
}

/// Whether the adaptive speed switch of the mode `settings` train is on,
/// room or not: in Contest mode the callers' own, on as it ships, so group
/// training's, off as it ships, is left as it was; in any other, group
/// training's.
pub fn adaptive_speed_on(settings: &TrainingSettings) -> bool {
    if settings.curriculum.char_set_mode == CharSetMode::Contest {
        settings.contest.contest_adapt_speed
    } else {
        settings.playback.adaptive_speed
    }
}

/// The line adaptive speed moves along, when it has one: the mode's switch
/// is on, and the speed settings leave more than one speed to choose from.
pub(crate) fn adaptive_speed_path(settings: &TrainingSettings) -> Option<SpeedPath> {
    let path = SpeedPath::of(settings);
    (adaptive_speed_on(settings) && path.has_room()).then_some(path)
}

/// The shortest and longest group `settings` asks for. At least one
/// character: a group of none is a silent send nobody can answer, and only
/// settings that were never clamped could ask for it.
pub(crate) fn group_size_bounds(settings: &TrainingSettings) -> (usize, usize) {
    let shortest = settings.curriculum.min_group_size.max(1) as usize;
    let longest = (settings.curriculum.max_group_size as usize).max(shortest);
    (shortest, longest)
}

/// Whether `settings` have group lengths drawn by adaptive group size: what
/// the screens say about it, read off the same rule the sampler follows.
pub fn adapts_group_size(settings: &TrainingSettings) -> bool {
    adaptive_bounds(settings).is_some()
}

/// The settings card that shows the aim, and so puts it back: Session shape
/// while group size adapts, for both; Speed while only the speed does. With
/// neither adapting it shows nowhere, and Session shape keeps it.
pub fn aim_section(settings: &TrainingSettings) -> SettingsSection {
    if adapts_speed(settings) && !adapts_group_size(settings) {
        SettingsSection::Speed
    } else {
        SettingsSection::SessionShape
    }
}

/// The range adaptive group size draws within, when it has one to draw in:
/// it is switched on, there is more than one length to choose from, and the
/// groups are drawn a character at a time: callsigns and words are as long as
/// they are.
pub(crate) fn adaptive_bounds(settings: &TrainingSettings) -> Option<(usize, usize)> {
    let (shortest, longest) = group_size_bounds(settings);
    (settings.curriculum.adaptive_group_size
        && settings.curriculum.char_set_mode.draws_characters()
        && shortest < longest)
        .then_some((shortest, longest))
}

/// The shortest group adaptive group size draws, while it draws the
/// lengths: a group longer than that answers for its own lost end.
pub(crate) fn length_floor(settings: &TrainingSettings) -> Option<usize> {
    adaptive_bounds(settings).map(|(shortest, _)| shortest)
}

/// The speed settings, seen as one line from the slowest corner to the
/// fastest: the character speed and the spacing's pace each move across
/// their range together, on a log scale, the spacing kept within its own
/// range wherever the characters are fast enough to allow it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedPath {
    char_wpm: (f64, f64),
    effective_wpm: (f64, f64),
    /// The spacing's range as set. The slowest corner may sit below it, as
    /// the spacing is never faster than the characters; the line comes up
    /// to it as soon as they allow, as a station drawn from the ranges
    /// does.
    spacing: (f64, f64),
}

/// How many times [`SpeedPath::voice_at`] halves the line to find a speed:
/// to well under a millionth of it.
const VOICE_HALVINGS: usize = 48;

impl SpeedPath {
    /// The line `settings` allow. The spacing is never faster than the
    /// characters, and tied to them it is the characters' own speed.
    pub fn of(settings: &TrainingSettings) -> Self {
        let p = &settings.playback;
        let char_wpm = (
            p.char_wpm_min.max(1.0),
            p.char_wpm_max.max(p.char_wpm_min).max(1.0),
        );
        let spacing = if p.link_char_to_effective {
            char_wpm
        } else {
            (
                p.effective_wpm_min.max(1.0),
                p.effective_wpm_max.max(p.effective_wpm_min).max(1.0),
            )
        };
        Self {
            char_wpm,
            effective_wpm: (spacing.0.min(char_wpm.0), spacing.1.min(char_wpm.1)),
            spacing,
        }
    }

    /// The slowest and fastest copy speed along it.
    pub fn bounds(&self) -> (f64, f64) {
        (
            copy_speed(self.char_wpm.0, self.effective_wpm.0),
            copy_speed(self.char_wpm.1, self.effective_wpm.1),
        )
    }

    /// The character speed and the spacing's pace at its slowest corner and
    /// at its fastest, in WPM: its ends as the speed settings put them.
    pub fn ends(&self) -> ((f64, f64), (f64, f64)) {
        (
            (self.char_wpm.0, self.effective_wpm.0),
            (self.char_wpm.1, self.effective_wpm.1),
        )
    }

    /// Whether there is more than one speed on it.
    pub fn has_room(&self) -> bool {
        let (low, high) = self.bounds();
        high > low * (1.0 + 1e-6)
    }

    /// The character speed and the spacing's pace that send at `copy`,
    /// kept on the line.
    pub fn voice_at(&self, copy: f64) -> (f64, f64) {
        if !self.has_room() || !copy.is_finite() {
            return self.voice_along(0.0);
        }
        // The copy speed only rises along the line, so halving it finds
        // where it sends at `copy`, or the end nearer it.
        let (mut slower, mut faster) = (0.0, 1.0);
        for _ in 0..VOICE_HALVINGS {
            let middle = 0.5 * (slower + faster);
            let (char_wpm, effective_wpm) = self.voice_along(middle);
            if copy_speed(char_wpm, effective_wpm) < copy {
                slower = middle;
            } else {
                faster = middle;
            }
        }
        self.voice_along(0.5 * (slower + faster))
    }

    /// The character speed and the spacing's pace `along` the line, from
    /// the slowest corner at 0 to the fastest at 1: the spacing within its
    /// range wherever the characters are as fast as its slowest, and never
    /// faster than they are.
    fn voice_along(&self, along: f64) -> (f64, f64) {
        let between = |(from, to): (f64, f64)| (from.ln() + along * (to.ln() - from.ln())).exp();
        let char_wpm = between(self.char_wpm);
        let (slowest, fastest) = self.spacing;
        let effective_wpm =
            between(self.effective_wpm).clamp(slowest.min(char_wpm), fastest.min(char_wpm));
        (char_wpm, effective_wpm)
    }
}
