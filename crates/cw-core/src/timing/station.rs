//! The operator at the other end.

use crate::numeric::finite_or;
use crate::rng::Rng;
use crate::settings::{TrainingSettings, WPM_MIN};

/// The operator at the other end: one pitch, one fist, one signal strength.
///
/// Drawn per station rather than per send, because a station that repeats its
/// call does not change frequency, speed or strength between repeats. Leaving
/// this to whenever the planner happened to reach for the RNG meant three
/// sends of one group arrived as three different operators.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StationVoice {
    pub tone_hz: f64,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    pub volume: f64,
    /// How heavy this operator's fist is: how much of each element is key-down
    /// against the gap after it. One is a keyer. Above one the elements run
    /// long and crowd the spaces, below it they are clipped and airy.
    pub weight: f64,
    /// How long their dahs run against their dits. Three is the book; real
    /// fists sit anywhere either side of it, and it is the first thing you
    /// notice about somebody's sending.
    pub dash_ratio: f64,
}

/// How far a fist can stray from a keyer's at the top of the setting.
pub(super) const WEIGHT_SPREAD: f64 = 0.18;
pub(super) const DASH_RATIO_SPREAD: f64 = 0.45;

impl StationVoice {
    /// This voice, its characters and its spacing alike `drop` slower, as a
    /// contest's caller slows down for you, never slower than the Speed
    /// card's lowest speed, as MorseRunner keeps a floor under its callers'
    /// (`DxStn.pas:166-168`).
    #[must_use]
    pub(crate) fn slowed(mut self, drop: f64) -> Self {
        let slow = |wpm: f64| (wpm * (1.0 - drop)).max(WPM_MIN).min(wpm);
        self.char_wpm = slow(self.char_wpm);
        self.effective_wpm = slow(self.effective_wpm).min(self.char_wpm);
        self
    }
}

/// Tune in a station, within whatever range the settings allow.
pub fn resolve_station(settings: &TrainingSettings, rng: &mut impl Rng) -> StationVoice {
    let (char_wpm, effective_wpm) = speeds_from(settings, |min, max| rng.pick_in_range(min, max));
    // Every station is somebody, and on the air most of them are not keyers.
    // Drawn once per station and carried, so a repeat is the same operator
    // sending again rather than a different one with the same call.
    let spread = fist_spread(settings);
    StationVoice {
        tone_hz: resolve_tone_hz(settings, rng),
        char_wpm,
        effective_wpm,
        volume: resolve_volume(settings, rng),
        weight: 1.0 + spread * rng.pick_in_range(-WEIGHT_SPREAD, WEIGHT_SPREAD),
        dash_ratio: 3.0 + spread * rng.pick_in_range(-DASH_RATIO_SPREAD, DASH_RATIO_SPREAD),
    }
}

/// How far the settings let a fist stray from a keyer's, from none to the
/// most there is: a variation that is no number is none, a keyer.
pub(super) fn fist_spread(settings: &TrainingSettings) -> f64 {
    finite_or(settings.playback.fist_variation, 0.0).clamp(0.0, 1.0)
}

/// The character speed and spacing pairs at the ends of what
/// [`resolve_station`] draws, as it draws them: each end of the character
/// speed range, and with a spacing of its own, each end of the spacing's
/// range brought into the character speed's, each with each end of the
/// spacing's range, never faster than the characters. Every gap a station
/// keys is longest or shortest at one of them: a gap grows with the
/// character speed at a slower spacing, shrinks with it at its own, and
/// shrinks with the spacing.
pub(super) fn station_speed_ends(settings: &TrainingSettings) -> Vec<(f64, f64)> {
    let p = &settings.playback;
    let ends = |a: f64, b: f64| {
        let (a, b) = (a.max(1.0), b.max(1.0));
        (a.min(b), a.max(b))
    };
    let (slowest, fastest) = ends(p.char_wpm_min, p.char_wpm_max);
    if p.link_char_to_effective {
        return vec![(slowest, slowest), (fastest, fastest)];
    }
    let (spaced_slowest, spaced_fastest) = ends(p.effective_wpm_min, p.effective_wpm_max);
    let chars = [
        slowest,
        fastest,
        spaced_slowest.clamp(slowest, fastest),
        spaced_fastest.clamp(slowest, fastest),
    ];
    chars
        .into_iter()
        .flat_map(|char_wpm| {
            [spaced_slowest, spaced_fastest].map(|spacing| (char_wpm, spacing.min(char_wpm)))
        })
        .collect()
}

/// A station's character speed and the pace of its spacing, `pick` choosing
/// a value between two bounds: the character speed first, then the spacing,
/// never faster than the characters.
fn speeds_from(settings: &TrainingSettings, mut pick: impl FnMut(f64, f64) -> f64) -> (f64, f64) {
    let p = &settings.playback;
    let char_wpm = pick(p.char_wpm_min.max(1.0), p.char_wpm_max.max(1.0)).max(1.0);
    // Tied means no Farnsworth spacing: the gaps go at the character speed.
    // Drawn again from the copied range they would not, about half the time.
    let effective_wpm = if p.link_char_to_effective {
        char_wpm
    } else {
        pick(p.effective_wpm_min.max(1.0), p.effective_wpm_max.max(1.0))
            .max(1.0)
            .min(char_wpm)
    };
    (char_wpm, effective_wpm)
}

/// How many steps the character speed range, and the spacing's, are cut
/// into to see how stations' speeds spread without drawing them: fine enough
/// that a whole WPM of either range is as likely as the next to a tenth of a
/// percent.
const CHAR_GRID_STEPS: usize = 1_000;
const SPACING_GRID_STEPS: usize = 200;

/// The speeds stations are sent at, as a grid of character speed and spacing
/// pairs, each as likely as the next: what [`resolve_station`] draws, spread
/// evenly the way its draw is, rather than drawn.
pub(crate) fn station_speed_grid(
    settings: &TrainingSettings,
) -> impl Iterator<Item = (f64, f64)> + '_ {
    let steps = |n: usize| (0..n).map(move |i| (i as f64 + 0.5) / n as f64);
    let spacings = if settings.playback.link_char_to_effective {
        1
    } else {
        SPACING_GRID_STEPS
    };
    steps(CHAR_GRID_STEPS)
        .flat_map(move |along_char| {
            steps(spacings).map(move |along_spacing| (along_char, along_spacing))
        })
        .map(|(along_char, along_spacing)| {
            let mut along = [along_char, along_spacing].into_iter();
            speeds_from(settings, |min, max| {
                min + along.next().unwrap_or(0.5) * (max - min)
            })
        })
}

/// Evenly in decibels across the range, as it is shown.
fn resolve_volume(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    let range = settings.volume_range_db();
    settings.volume_at_db(rng.pick_in_range(*range.start(), *range.end()))
}

fn resolve_tone_hz(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    let range = settings.side_tone_range();
    rng.pick_in_range_inclusive_int(*range.start(), *range.end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;

    fn fist(variation: f64) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.playback.fist_variation = variation;
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        s.clamp()
    }

    /// Tied is the default, and tied means no Farnsworth spacing at all: the
    /// station sends its gaps at the speed it sends its characters. Drawing
    /// the effective speed separately from the same range stretched the gaps
    /// on about half the groups.
    #[test]
    fn a_tied_effective_speed_is_the_character_speed() {
        let settings = TrainingSettings::default().clamp();
        assert!(settings.playback.link_char_to_effective);
        assert!(settings.playback.char_wpm_min < settings.playback.char_wpm_max);
        let mut rng = SplitMix64::new(11);
        for _ in 0..2_000 {
            let voice = resolve_station(&settings, &mut rng);
            assert_eq!(voice.effective_wpm, voice.char_wpm);
        }
    }

    /// Turned off, every station is a keyer — which is what the trainer was
    /// before there was a setting, and what somebody learning their first
    /// characters should be able to get back.
    #[test]
    fn at_zero_every_station_is_a_keyer() {
        let s = fist(0.0);
        for seed in 0..100u64 {
            let voice = resolve_station(&s, &mut SplitMix64::new(seed * 31 + 7));
            assert!((voice.weight - 1.0).abs() < 1e-9);
            assert!((voice.dash_ratio - 3.0).abs() < 1e-9);
        }
    }

    /// A variation that is no number, which `TrainingSettings::clamp` never
    /// leaves but this takes from whoever calls it, is none: every station
    /// a keyer, not one whose weight and dash ratio are no number.
    #[test]
    fn a_fist_variation_of_no_number_is_a_keyer() {
        let mut s = fist(0.5);
        s.playback.fist_variation = f64::NAN;
        let voice = resolve_station(&s, &mut SplitMix64::new(7));
        assert_eq!((voice.weight, voice.dash_ratio), (1.0, 3.0));
    }

    /// A strength range is drawn evenly in decibels, as it is shown: from
    /// the strongest down to 20 dB under it, half the stations come in
    /// under 10 down, where evenly in amplitude half came in within 5 of
    /// the top. Fixed, it is the one strength, drawn from nothing.
    #[test]
    fn a_strength_is_drawn_evenly_in_decibels() {
        use crate::band::gain_to_db;
        use crate::settings::{VOLUME_MAX, VOLUME_MIN};
        let mut s = TrainingSettings::default();
        s.band.link_volume = false;
        s.band.volume_min = VOLUME_MIN;
        s.band.volume_max = VOLUME_MAX;
        let mut rng = SplitMix64::new(3);
        let mut heard: Vec<f64> = (0..2_000)
            .map(|_| gain_to_db(resolve_volume(&s, &mut rng)))
            .collect();
        heard.sort_by(f64::total_cmp);
        let median = heard[heard.len() / 2];
        assert!((median + 10.0).abs() < 1.0, "the middle at {median:.2} dB");
        assert!(heard[0] >= -20.0 - 1e-9 && heard[heard.len() - 1] <= 1e-9);
        s.band.link_volume = true;
        s.band.volume_min = 0.7;
        let mut rng = SplitMix64::new(3);
        assert_eq!(resolve_volume(&s, &mut rng), 0.7);
        assert_eq!(rng.f64(), SplitMix64::new(3).f64(), "nothing drawn");
    }

    /// Turned up, they are people: no two send quite alike.
    #[test]
    fn every_operator_has_their_own_fist() {
        let s = fist(1.0);
        let fists: Vec<(f64, f64)> = (0..100u64)
            .map(|seed| {
                let v = resolve_station(&s, &mut SplitMix64::new(seed * 2_654_435_761 + 11));
                (v.weight, v.dash_ratio)
            })
            .collect();
        let heaviest = fists.iter().map(|f| f.0).fold(f64::MIN, f64::max);
        let lightest = fists.iter().map(|f| f.0).fold(f64::MAX, f64::min);
        assert!(
            heaviest - lightest > 0.1,
            "every fist came out the same weight: {lightest:.3}..{heaviest:.3}"
        );
        let longest = fists.iter().map(|f| f.1).fold(f64::MIN, f64::max);
        let shortest = fists.iter().map(|f| f.1).fold(f64::MAX, f64::min);
        assert!(
            longest - shortest > 0.3,
            "every dah was the same length: {shortest:.2}..{longest:.2}"
        );
    }
}
