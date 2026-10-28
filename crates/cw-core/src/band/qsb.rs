//! Fading: how much of the signal the path is letting through.

use super::dsp::db_to_gain;
use crate::settings::{BandSettings, QSB_DEPTH_MAX_DB, QSB_RATE_MAX_HZ, QSB_RATE_MIN_HZ};

/// How far into the band's fading a station's path is, at most, in seconds
/// of it: every station arrives on a path of its own, so no two fade
/// together. The fade never repeats, so any two points this far apart
/// wander apart.
pub const FADE_OFFSET_MAX_SEC: f64 = 3_600.0;

/// How far apart a fade is read along a send, with straight lines between.
/// The fastest path of the Band card's fading moves at under 3.5 Hz, and of
/// the fastest flutter at under 80, so four sines a sample to follow it
/// would be waste: a millisecond apart, the line is the fade to a few
/// hundredths of a decibel.
pub const FADE_STEP_SEC: f64 = 0.001;

/// The weakest a fade leaves the signal, as a gain: as deep as the Band card
/// goes ([`QSB_DEPTH_MAX_DB`]). The sum of paths is held there when it would
/// go further, so the signal sits at the bottom a moment as a fade bottoms
/// out; held any deeper, it would be out of hearing for letters at a time,
/// where a real null of 30 dB is over in a moment.
const QSB_MIN_GAIN: f64 = 0.1;

/// Opens the multipath sum back out to the depth a single sine would have had.
/// Four sines seldom line up, so their sum sits nearer the middle; this scales
/// it back by the ratio of the two RMS values. Scaled that far, the sum runs
/// past the ends of the range about a sixth of the time, and is held there:
/// the signal sits at full strength, or at the bottom of a fade, for a moment
/// before it moves on — which is how a fade bottoming out sounds.
const QSB_SPREAD: f64 = 1.85;

/// How many propagation paths the fading is made of.
const QSB_PATHS: usize = 4;

/// The rates of the paths a fade is made of, as multiples of its own rate.
///
/// The ratios are deliberately not simple fractions. Sines at 1, 2 and 3
/// times a rate come back round together and you are back to a pattern; at
/// these they do not, so the fading wanders and never repeats. The band's own
/// stations fade on the first three.
pub(super) const MULTIPATH_RATES: [f64; QSB_PATHS] = [1.000, 0.633, 1.471, 2.309];

/// How much of the fading each path carries. They sum to one, so the swing
/// spans -1..=1 at its widest and the depth setting keeps meaning what it
/// meant.
const PATH_WEIGHTS: [f64; QSB_PATHS] = [0.40, 0.28, 0.20, 0.12];

/// The fading on the Morse, as the settings ask for it: how deep and how
/// fast. A value both players hold for the length of a send, so moving a
/// slider while a group plays changes the next group, never one mid-element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Qsb {
    /// The bottom of a fade, as a gain; full strength is one.
    floor: f64,
    rate_hz: f64,
}

impl Qsb {
    /// The fading `band` asks for, or None when it asks for none: its depth
    /// a share of [`QSB_DEPTH_MAX_DB`], so the slider moves the fade by
    /// equal steps of loudness from one end to the other.
    pub fn of(band: &BandSettings) -> Option<Self> {
        let share = if band.qsb_depth.is_finite() {
            band.qsb_depth.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if !band.qsb_enabled || share <= 0.0 {
            return None;
        }
        let rate_hz = if band.qsb_rate_hz.is_finite() {
            band.qsb_rate_hz.clamp(QSB_RATE_MIN_HZ, QSB_RATE_MAX_HZ)
        } else {
            QSB_RATE_MIN_HZ
        };
        Some(Self {
            floor: db_to_gain(-share * QSB_DEPTH_MAX_DB).max(QSB_MIN_GAIN),
            rate_hz,
        })
    }

    /// A flutter: the same wandering sum of paths, `depth` deep, at a rate
    /// far past the Band card's fading, as a signal over a polar path
    /// flutters, several times a second. Held to the depth a fade keeps
    /// ([`QSB_MIN_GAIN`]) and to a positive, finite rate.
    pub fn flutter(depth: f64, rate_hz: f64) -> Self {
        let depth = held_depth(depth);
        let rate_hz = if rate_hz.is_finite() && rate_hz > 0.0 {
            rate_hz
        } else {
            QSB_RATE_MIN_HZ
        };
        Self {
            floor: 1.0 - depth,
            rate_hz,
        }
    }

    /// How deep it fades: from full strength down to `1 - depth`.
    pub fn depth(&self) -> f64 {
        1.0 - self.floor
    }

    /// How fast it wanders, in hertz: the rate of its first path.
    pub fn rate_hz(&self) -> f64 {
        self.rate_hz
    }

    /// How much of the signal is getting through at `t_sec` on the band's
    /// clock.
    ///
    /// Fading is not a tremolo. A single sine gives you a fade you can set
    /// your watch by, which is the one thing real QSB never is — what actually
    /// happens is several paths arriving with slightly different Doppler and
    /// beating against each other, so the signal wanders down and back up on
    /// no schedule. That is a sum of sinusoids at close but unrelated rates:
    /// the classical multipath model.
    ///
    /// `QSB_SPREAD` is there because a sum of four sines spends most of its
    /// time nearer the middle than one sine does; without it, turning
    /// multipath on would quietly halve the depth of every fade.
    pub fn gain_at(&self, t_sec: f64) -> f32 {
        let swing: f64 = MULTIPATH_RATES
            .iter()
            .zip(PATH_WEIGHTS)
            .map(|(multiple, weight)| {
                weight * (std::f64::consts::TAU * self.rate_hz * multiple * t_sec).sin()
            })
            .sum();
        let swing = (swing * QSB_SPREAD).clamp(-1.0, 1.0);
        let half = (1.0 - self.floor) / 2.0;
        (1.0 - half + half * swing) as f32
    }
}

/// `depth` as a fade keeps it: none for what is no number, and never so
/// deep the signal falls under the weakest a fade leaves it.
fn held_depth(depth: f64) -> f64 {
    if depth.is_finite() {
        depth.clamp(0.0, 1.0 - QSB_MIN_GAIN)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qsb(depth: f64, rate_hz: f64) -> Qsb {
        let band = BandSettings {
            qsb_enabled: true,
            qsb_depth: depth,
            qsb_rate_hz: rate_hz,
            ..BandSettings::default()
        };
        Qsb::of(&band).expect("fading is on")
    }

    #[test]
    fn qsb_modulates_amplitude_without_reaching_silence() {
        let fading = qsb(1.0, 1.0);
        let gains: Vec<f32> = (0..48_000)
            .map(|i| fading.gain_at(f64::from(i) / 48_000.0))
            .collect();
        let min = gains.iter().copied().fold(f32::MAX, f32::min);
        let max = gains.iter().copied().fold(f32::MIN, f32::max);
        assert!(min < 0.8 && max > 0.9);
        assert!(
            min >= QSB_MIN_GAIN as f32,
            "a fade should not reach silence"
        );
    }

    #[test]
    fn no_fading_is_asked_for_when_it_is_off_or_has_no_depth() {
        let band = |enabled: bool, depth: f64| BandSettings {
            qsb_enabled: enabled,
            qsb_depth: depth,
            ..BandSettings::default()
        };
        assert_eq!(Qsb::of(&band(false, 1.0)), None);
        assert_eq!(Qsb::of(&band(true, 0.0)), None);
        assert_eq!(Qsb::of(&band(true, f64::NAN)), None);
        assert!(Qsb::of(&band(true, 0.5)).is_some());
        // Well down a full-depth fade three quarters of a second in.
        let trough = qsb(1.0, 1.0).gain_at(0.75);
        assert!(trough >= QSB_MIN_GAIN as f32 && trough < 0.3, "{trough}");
        // Out-of-range rates are held to the settings' own range.
        assert_eq!(qsb(0.5, 99.0), qsb(0.5, QSB_RATE_MAX_HZ));
        assert_eq!(qsb(0.5, f64::NAN), qsb(0.5, QSB_RATE_MIN_HZ));
    }

    /// Real QSB is not a tremolo: several paths beating against each other
    /// wander and never come back round. Each cycle of the base rate has to
    /// differ from the first; a sine would differ by zero.
    #[test]
    fn fading_never_comes_back_round() {
        let (fading, rate, dt) = (qsb(0.8, 0.2), 0.2, 0.01);
        let env: Vec<f64> = (0..20_000)
            .map(|i| f64::from(fading.gain_at(f64::from(i) * dt)))
            .collect();
        let period = (1.0 / rate / dt) as usize;
        for cycle in 1..8 {
            let differs = (0..period)
                .map(|i| (env[i] - env[cycle * period + i]).abs())
                .fold(0.0_f64, f64::max);
            assert!(
                differs > 0.1,
                "cycle {cycle} repeated the first to within {differs:.3}"
            );
        }
    }

    /// And it still spans the depth it is asked for: down to its floor,
    /// back up to full signal, and never outside that.
    #[test]
    fn fading_spans_its_depth_and_stays_inside_it() {
        let deep = qsb(0.8, 0.2);
        let floor = db_to_gain(-0.8 * QSB_DEPTH_MAX_DB);
        let env: Vec<f64> = (0..20_000)
            .map(|i| f64::from(deep.gain_at(f64::from(i) * 0.01)))
            .collect();
        let min = env.iter().copied().fold(f64::MAX, f64::min);
        let max = env.iter().copied().fold(f64::MIN, f64::max);
        assert!(min <= floor + 0.02, "deep fades never arrived: {min:.3}");
        assert!(max >= 0.97, "the signal never came back up: {max:.3}");
        assert!(min >= floor - 1e-6, "faded past the floor: {min:.3}");

        let half = qsb(0.5, 0.5);
        let floor = db_to_gain(-0.5 * QSB_DEPTH_MAX_DB);
        for step in 0..200 {
            let gain = f64::from(half.gain_at(f64::from(step) * 0.05));
            assert!(gain >= floor - 1e-6 && gain <= 1.0 + 1e-6, "{gain}");
        }
        for t in [0.0, -1.0, 1e9] {
            assert!(qsb(0.6, 1.0).gain_at(t).is_finite());
        }
    }

    /// A flutter wanders as a fade does, at its own rate, past the Band
    /// card's range, and is held to the depth a fade keeps.
    #[test]
    fn a_flutter_wanders_at_its_own_rate_and_depth() {
        let flutter = Qsb::flutter(0.6, 20.0);
        assert_eq!((flutter.depth(), flutter.rate_hz()), (0.6, 20.0));
        let gains: Vec<f64> = (0..48_000)
            .map(|i| f64::from(flutter.gain_at(f64::from(i) / 48_000.0)))
            .collect();
        let min = gains.iter().copied().fold(f64::MAX, f64::min);
        let max = gains.iter().copied().fold(f64::MIN, f64::max);
        assert!((0.4 - 1e-6..0.45).contains(&min), "{min}");
        assert!(max > 0.97 && max <= 1.0 + 1e-6, "{max}");
        // Twenty times a second: within one second it falls through the
        // middle at least as often as its first path does, near forty times.
        let crossings = gains
            .windows(2)
            .filter(|pair| (pair[0] - 0.7) * (pair[1] - 0.7) < 0.0)
            .count();
        assert!((25..=120).contains(&crossings), "{crossings}");
        assert_eq!(Qsb::flutter(2.0, 5.0).depth(), 1.0 - QSB_MIN_GAIN);
        assert_eq!(Qsb::flutter(f64::NAN, -1.0).depth(), 0.0);
        assert_eq!(Qsb::flutter(0.5, f64::NAN).rate_hz(), QSB_RATE_MIN_HZ);
    }

    /// The depth slider is heard over the whole of its travel, in equal
    /// steps of loudness: each of its steps takes the bottom of the fade a
    /// decibel further down, to 20 dB at the end; out of the box, 12.
    #[test]
    fn every_step_of_the_depth_is_heard() {
        let floor_db = |share: f64| 20.0 * qsb(share, 0.2).floor.log10();
        for step in 1..=20 {
            let share = f64::from(step) * 0.05;
            assert!(
                (floor_db(share) + share * QSB_DEPTH_MAX_DB).abs() < 1e-9,
                "{share}: {:.2} dB",
                floor_db(share)
            );
        }
        assert!((floor_db(1.0) + 20.0).abs() < 1e-9);
        assert!(floor_db(0.95) > floor_db(1.0));
        let default = Qsb::of(&BandSettings::default()).expect("fading out of the box");
        let trough_db = 20.0 * default.floor.log10();
        assert!(trough_db <= -10.0, "out of the box only {trough_db:.1} dB");
    }
}
