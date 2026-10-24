//! The receiver's band, as the audio callback plays it, fading in and out.

use cw_core::band::{BandMixer, Received};

use crate::audio::shape::{BAND_FADE_SEC, fade_in_gain};

/// How long a new band runs before it is heard, so that it comes in warm:
/// the narrowest filter has filled with noise to a tenth of a decibel by
/// then, and the AGC's look-ahead is full. A band switched in cold opens a
/// hole of ten decibels in the hiss for a few milliseconds.
const PRE_ROLL_SEC: f64 = 0.03;

/// Where a band is in its life on the air.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fade {
    /// Coming in: samples into its fade.
    In(usize),
    Steady,
    /// On its way out: samples into its fade.
    Out(usize),
    Gone,
}

/// One band: the band's own model under the receiver, and how loud it is
/// being played while one band hands over to the next.
pub struct BandVoice {
    mixer: BandMixer,
    fade: Fade,
    fade_len: usize,
}

/// One sample of a band.
pub struct Played {
    /// The band, at the level its fade has it at.
    pub band: f64,
    /// The receiver's gain, which the send rides.
    pub gain: f64,
    /// How much of the band is being played: one when steady, falling to
    /// nothing as it goes.
    pub weight: f64,
    /// What reached the receiver's AGC, for its meter.
    pub input: f64,
}

impl BandVoice {
    /// A new band, run in so it is heard warm, and starting to fade in.
    /// Built on the UI thread: the run-in is a few hundred microseconds of
    /// work the audio callback should not do.
    pub fn new(mut mixer: BandMixer, sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate.max(1));
        for _ in 0..(PRE_ROLL_SEC * sr) as usize {
            mixer.next(0.0);
        }
        Self {
            mixer,
            fade: Fade::In(0),
            fade_len: ((BAND_FADE_SEC * sr).round() as usize).max(1),
        }
    }

    /// Hand over from `previous`: the receiver's gain carries on rather than
    /// opening wide under a station being copied.
    pub fn take_over_from(&mut self, previous: &BandVoice) {
        self.mixer.carry_gain_from(&previous.mixer);
    }

    /// Start fading out, from wherever its fade has got to: a band replaced
    /// while it was still coming in goes from the level it had reached.
    pub fn fade_out(&mut self) {
        self.fade = match self.fade {
            Fade::In(done) => Fade::Out(self.fade_len.saturating_sub(done)),
            Fade::Steady => Fade::Out(0),
            other => other,
        };
    }

    pub fn is_gone(&self) -> bool {
        self.fade == Fade::Gone
    }

    pub fn reserve_stations(&mut self, callers: usize) {
        self.mixer.reserve_stations(callers);
    }

    /// The next sample, with `send` playing over it.
    pub fn next(&mut self, send: f64) -> Played {
        let progress = |done: usize| done as f64 / self.fade_len as f64;
        let (weight, fade) = match self.fade {
            Fade::Gone => {
                return Played {
                    band: 0.0,
                    gain: 1.0,
                    weight: 0.0,
                    input: send,
                };
            }
            Fade::Steady => (1.0, Fade::Steady),
            Fade::In(done) if done >= self.fade_len => (1.0, Fade::Steady),
            Fade::In(done) => (fade_in_gain(progress(done)), Fade::In(done + 1)),
            Fade::Out(done) if done >= self.fade_len => (0.0, Fade::Gone),
            Fade::Out(done) => (fade_in_gain(1.0 - progress(done)), Fade::Out(done + 1)),
        };
        self.fade = fade;
        let Received { band, gain, input } = self.mixer.next(send);
        Played {
            band: band * weight,
            gain,
            weight,
            input,
        }
    }
}
