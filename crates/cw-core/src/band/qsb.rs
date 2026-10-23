//! Fading: how much of the signal the path is letting through.

use crate::settings::{BandSettings, QSB_RATE_MAX_HZ, QSB_RATE_MIN_HZ};

/// The weakest a fade leaves the signal. Below this a group would simply be
/// lost rather than hard to copy.
const QSB_MIN_GAIN: f64 = 0.25;

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

/// Points per second in a [`Qsb::curve`]. The fastest path at the fastest
/// rate the Band card's fading goes to is under 3.5 Hz, so straight lines
/// between points this close follow the fade to a fraction of a decibel,
/// even at the bottom of the deepest fade, where it turns sharply as it
/// reaches the end of its range. A flutter's paths run some twenty times
/// faster, which this would not follow: a flutter is laid into its tones'
/// envelopes instead, and never made a curve.
const CURVE_POINTS_PER_SEC: f64 = 200.0;

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
    /// The fading `band` asks for, or None when it asks for none.
    pub fn of(band: &BandSettings) -> Option<Self> {
        let depth = held_depth(band.qsb_depth);
        if !band.qsb_enabled || depth <= 0.0 {
            return None;
        }
        let rate_hz = if band.qsb_rate_hz.is_finite() {
            band.qsb_rate_hz.clamp(QSB_RATE_MIN_HZ, QSB_RATE_MAX_HZ)
        } else {
            QSB_RATE_MIN_HZ
        };
        Some(Self {
            floor: 1.0 - depth,
            rate_hz,
        })
    }

    /// A flutter: the same wandering sum of paths, `depth` deep, at a rate
    /// far past the Band card's fading, as a signal over a polar path
    /// flutters, several times a second. Held to the depth a fade keeps
    /// ([`QSB_MIN_GAIN`]) and to a positive, finite rate. Laid into tones'
    /// envelopes only: too fast for a [`Self::curve`].
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

    /// The fade over `duration_sec` from `start_sec` on the band's clock, as
    /// evenly spaced values of [`Qsb::gain_at`] from the first instant to the
    /// last.
    ///
    /// This is how the browser fades a send: it schedules the curve on the
    /// send's gain, which plays it as straight lines between the points. A
    /// graph of oscillators cannot hold their sum inside the range the way
    /// `gain_at` does, so it is the one function, sampled, rather than a copy
    /// of it built from nodes. For the band's fading only: its points are
    /// too far apart for a flutter ([`CURVE_POINTS_PER_SEC`]).
    pub fn curve(&self, start_sec: f64, duration_sec: f64) -> Vec<f32> {
        let duration = if duration_sec.is_finite() {
            duration_sec.max(0.0)
        } else {
            0.0
        };
        let intervals = (duration * CURVE_POINTS_PER_SEC).ceil().max(1.0) as usize;
        (0..=intervals)
            .map(|k| self.gain_at(start_sec + duration * k as f64 / intervals as f64))
            .collect()
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
        let trough = qsb(1.0, 1.0).gain_at(0.75);
        assert!(trough > 0.2 && trough < 0.4);
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

    /// And it still spans the depth it is asked for: down to the floor that keeps
    /// a group answerable, back up to full signal, and never outside that.
    #[test]
    fn fading_spans_its_depth_and_stays_inside_it() {
        let deep = qsb(0.8, 0.2);
        let env: Vec<f64> = (0..20_000)
            .map(|i| f64::from(deep.gain_at(f64::from(i) * 0.01)))
            .collect();
        let min = env.iter().copied().fold(f64::MAX, f64::min);
        let max = env.iter().copied().fold(f64::MIN, f64::max);
        assert!(
            min <= QSB_MIN_GAIN + 0.02,
            "deep fades never arrived: {min:.3}"
        );
        assert!(max >= 0.97, "the signal never came back up: {max:.3}");
        assert!(min >= QSB_MIN_GAIN - 1e-6, "faded past the floor: {min:.3}");

        let depth: f64 = 0.5;
        let half = qsb(depth, 0.5);
        for step in 0..200 {
            let gain = f64::from(half.gain_at(f64::from(step) * 0.05));
            assert!(gain >= 1.0 - depth - 1e-6 && gain <= 1.0 + 1e-6, "{gain}");
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

    /// The browser fades a send by playing a curve of the shared fade, as
    /// straight lines between its points. Between them it has to be the fade
    /// the desktop plays, and it must never leave the range the desktop's
    /// stays in: an oscillator graph summed straight onto a gain did, going
    /// above full strength and, at full depth, through zero.
    #[test]
    fn the_browsers_curve_is_the_shared_fade() {
        for (depth, rate) in [(0.35, 0.12), (0.75, 1.5), (1.0, 1.5), (1.0, 0.03)] {
            let fading = qsb(depth, rate);
            let floor = 1.0 - depth.min(1.0 - QSB_MIN_GAIN);
            let (start, duration) = (123.4, 9.85);
            let curve = fading.curve(start, duration);
            assert!(curve.len() >= (duration * CURVE_POINTS_PER_SEC) as usize);
            let step = duration / (curve.len() - 1) as f64;
            for (k, point) in curve.iter().enumerate() {
                let t = start + step * k as f64;
                assert_eq!(*point, fading.gain_at(t), "point {k}");
                assert!(
                    f64::from(*point) >= floor - 1e-6 && *point <= 1.0,
                    "{depth}/{rate}: {point} outside {floor}..=1"
                );
            }
            // What the browser plays between the points.
            let worst = (0..curve.len() - 1)
                .flat_map(|k| (1..10).map(move |j| (k, f64::from(j) / 10.0)))
                .map(|(k, frac)| {
                    let line = f64::from(curve[k]) * (1.0 - frac) + f64::from(curve[k + 1]) * frac;
                    let t = start + step * (k as f64 + frac);
                    (line - f64::from(fading.gain_at(t))).abs()
                })
                .fold(0.0_f64, f64::max);
            let worst_db = 20.0 * (1.0 + worst / floor).log10();
            assert!(
                worst_db < 0.3,
                "{depth}/{rate}: off the fade by {worst_db:.3} dB"
            );
        }
        // A send with no length still gets a curve the browser accepts:
        // two points at least.
        assert_eq!(qsb(0.5, 0.5).curve(0.0, 0.0).len(), 2);
        assert_eq!(qsb(0.5, 0.5).curve(0.0, f64::NAN).len(), 2);
    }
}
