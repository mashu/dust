//! The receiver's output stage: its audio stage's roll-off below the note,
//! the soft limiter everything it plays passes through last, and how the
//! browser builds the same limiter out of a node.

use std::f64::consts::FRAC_1_SQRT_2;

use super::dsp::Biquad;

/// Where the receiver's audio stage starts: a second-order roll-off below
/// this, as a rig's audio is coupled and filtered, so a filter wide enough to
/// fold the opposite sideband down to zero beat brings no rumble with it. A
/// side tone at 200 Hz, the lowest the dial goes, loses a quarter of a
/// decibel to it.
pub const AUDIO_HIGH_PASS_HZ: f64 = 100.0;

/// The receiver's audio stage, after its AGC: a Butterworth high-pass at
/// [`AUDIO_HIGH_PASS_HZ`]. After the AGC, as in a rig, whose AGC works from
/// the IF and hears what is near zero beat; the hiss and the stations that
/// reach the speaker have it taken out.
#[derive(Clone, Copy, Debug)]
pub struct AudioHighPass(Biquad);

impl AudioHighPass {
    pub fn new(sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate.max(1));
        Self(Biquad::high_pass(rate, AUDIO_HIGH_PASS_HZ, FRAC_1_SQRT_2))
    }

    pub fn process(&mut self, sample: f64) -> f64 {
        self.0.process(sample)
    }

    /// Brought to rest once what it holds has died away to nothing, rather
    /// than left to sink into the subnormal range.
    pub fn settle(&mut self) {
        self.0.settle();
    }

    /// Its gain for a steady tone at `hz`.
    #[cfg(test)]
    fn gain_at(&self, sample_rate: f64, hz: f64) -> f64 {
        self.0.gain_at(sample_rate, hz)
    }
}

/// Where [`soft_limit`] starts to bend, as a share of full scale: below it
/// whatever is played comes out exactly as it went in.
pub const SOFT_LIMIT_KNEE: f64 = 0.7;

/// What a receiver does with a loud crash: catches it, rather than letting it
/// square off. Linear up to the knee, asymptotic to full scale above it — so
/// the biggest static is loud without turning into a buzz.
///
/// The last thing before the speaker, once, on everything played together:
/// the band, the Morse over it and the paddle's sidetone. Limited apart and
/// then added, any two of them could still square off.
pub fn soft_limit(x: f64) -> f64 {
    let magnitude = x.abs();
    if magnitude <= SOFT_LIMIT_KNEE {
        return x;
    }
    let over = (magnitude - SOFT_LIMIT_KNEE) / (1.0 - SOFT_LIMIT_KNEE);
    (SOFT_LIMIT_KNEE + (1.0 - SOFT_LIMIT_KNEE) * over.tanh()).copysign(x)
}

/// How far past full scale [`soft_limit_curve`] reaches before it simply holds
/// at its last value. Far beyond anything the band and the Morse add up to.
/// Everything on its way into the browser's limiter is this many times
/// quieter than it is played.
pub const SOFT_LIMIT_HEADROOM: f64 = 4.0;

/// `soft_limit` as a lookup table, for a Web Audio `WaveShaperNode`.
///
/// A shaper maps its input over -1..=1 onto the curve, so the input is to be
/// scaled down by `SOFT_LIMIT_HEADROOM` first; the curve maps that back up
/// before limiting. Between the points the shaper interpolates linearly, which
/// below the knee is exact — the limiter is a straight line there.
pub fn soft_limit_curve(points: usize) -> Vec<f32> {
    let points = points.max(2);
    let last = (points - 1) as f64;
    (0..points)
        .map(|i| {
            let x = (2.0 * i as f64 / last - 1.0) * SOFT_LIMIT_HEADROOM;
            soft_limit(x) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::band::random::Gaussian;
    use crate::band::test_support::ALL_SHAPES;
    use crate::band::{FilterDesign, ReceiverFilter};
    use crate::settings::{FILTER_WIDTH_MAX, FilterShape};

    /// However loud it gets, it is caught rather than squared off — which is
    /// what a receiver does with a crash, and the difference between loud
    /// static and a buzz.
    #[test]
    fn the_soft_limit_catches_rather_than_clips() {
        assert_eq!(soft_limit(0.0), 0.0);
        assert_eq!(soft_limit(0.5), 0.5, "below the knee it does nothing");
        assert_eq!(soft_limit(-0.5), -0.5);
        assert!(soft_limit(4.0) < 1.0 && soft_limit(4.0) > 0.9);
        assert!(soft_limit(-4.0) > -1.0 && soft_limit(-4.0) < -0.9);
        assert!(
            soft_limit(2.0) < soft_limit(3.0),
            "limiting, not clipping flat"
        );
    }

    /// The browser limits with a lookup table of the same curve, so the table
    /// has to be the curve: identical below the knee, never past full scale,
    /// always rising.
    #[test]
    fn the_soft_limit_table_is_the_soft_limit() {
        let curve = soft_limit_curve(4_097);
        assert_eq!(curve.len(), 4_097);
        assert!(
            curve.windows(2).all(|w| w[1] >= w[0]),
            "it must rise everywhere"
        );
        // At most full scale: the far ends round to exactly 1.0 in f32.
        assert!(curve.iter().all(|v| v.abs() <= 1.0));
        // The midpoint is silence, and the ends are as loud as it gets.
        assert_eq!(curve[2_048], 0.0);
        assert!(curve[4_096] > 0.99 && curve[0] < -0.99);
        // Below the knee it is the identity, once the headroom scaling is
        // undone.
        for (i, value) in curve.iter().enumerate() {
            let x = (2.0 * i as f64 / 4_096.0 - 1.0) * SOFT_LIMIT_HEADROOM;
            if x.abs() <= 0.7 {
                assert!((f64::from(*value) - x).abs() < 1e-6, "at {x}");
            }
        }
        assert_eq!(soft_limit_curve(0).len(), 2);
    }

    /// The audio stage takes out what a wide filter folds down to zero beat
    /// and leaves the note alone: through any receiver, 20 Hz comes out at
    /// least 20 dB under the pitch and 50 Hz 6 dB under; and the lowest side
    /// tone the dial has loses no more than 0.3 dB to it.
    #[test]
    fn the_audio_stage_rolls_off_below_the_note() {
        let rate = 48_000.0;
        let stage = AudioHighPass::new(48_000);
        let note = 20.0 * stage.gain_at(rate, 200.0).log10();
        assert!(note > -0.3, "200 Hz at {note:.2} dB");
        for shape in ALL_SHAPES {
            for centre in [200.0, 300.0, 450.0, 700.0, 1_200.0] {
                for width in [150.0, 600.0, 1_000.0, 2_000.0, FILTER_WIDTH_MAX] {
                    let design = FilterDesign::new(centre, width, shape);
                    let heard = |hz: f64| {
                        design.response_at(hz) * stage.gain_at(rate, hz)
                            / stage.gain_at(rate, centre)
                    };
                    let at = |hz: f64| 20.0 * heard(hz).log10();
                    let here = format!("{shape:?} {centre}/{width}");
                    assert!(at(20.0) <= -20.0, "{here}: 20 Hz at {:.1} dB", at(20.0));
                    assert!(at(50.0) <= -6.0, "{here}: 50 Hz at {:.1} dB", at(50.0));
                }
            }
        }
    }

    /// White noise through the widest receivers at the lowest pitches, the
    /// ones that fold most of the opposite sideband down to zero beat, comes
    /// out of the audio stage with under 2% of its power below 60 Hz, where
    /// without it up to a third was.
    #[test]
    fn the_audio_stage_leaves_no_rumble() {
        const SR: u32 = 16_000;
        let rate = f64::from(SR);
        for (centre, width) in [(200.0, 590.0), (300.0, 800.0), (450.0, 1_220.0)] {
            let design = FilterDesign::new(centre, width, FilterShape::Sharp);
            let mut receiver = ReceiverFilter::new(SR, &design);
            let mut stage = AudioHighPass::new(SR);
            // Below 60 Hz, as an eighth-order low-pass there measures it.
            let mut below = [Biquad::low_pass(rate, 60.0, FRAC_1_SQRT_2); 4];
            let mut noise = Gaussian::new(7);
            let (mut all, mut low) = (0.0, 0.0);
            for i in 0..(SR as usize * 20) {
                let heard = stage.process(receiver.process(noise.sample()));
                let under = below
                    .iter_mut()
                    .fold(heard, |x, section| section.process(x));
                if i > SR as usize {
                    all += heard * heard;
                    low += under * under;
                }
            }
            let share = low / all;
            assert!(
                share < 0.02,
                "{centre}/{width}: {:.1}% below 60 Hz",
                share * 100.0
            );
        }
    }
}
