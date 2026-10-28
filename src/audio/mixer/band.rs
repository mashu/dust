//! The receiver's band, as the audio callback plays it, fading in and out.

use cw_core::TrainingSettings;
use cw_core::band::{BandMixer, Received, Receiver};

use crate::audio::shape::{BAND_FADE_SEC, fade_in_gain, release_gain};

/// How long a new band runs before it is heard, so that it comes in warm:
/// the narrowest filter has filled with noise to a tenth of a decibel by
/// then, and the AGC's look-ahead is full. A band switched in cold opens a
/// hole of ten decibels in the hiss for a few milliseconds.
const PRE_ROLL_SEC: f64 = 0.03;

/// Where a band is in its life on the air.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fade {
    /// Running unheard, in step with the band it is to take over from,
    /// until it is warm: samples left.
    Warming(usize),
    /// Coming in: samples into its fade.
    In(usize),
    Steady,
    /// Heard as it is while the one taking over from it warms: samples left.
    Holding(usize),
    /// On its way out: samples into its fade.
    Out(usize),
    Gone,
}

/// How two bands cross: unrelated noises, whose powers add, or one band
/// through two receivers, the same noise and stations twice, whose
/// amplitudes do. Each way, gains that keep the hiss level through it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Crossfade {
    EqualPower,
    Complementary,
}

impl Crossfade {
    /// The gain `progress` of the way in.
    fn rise(self, progress: f64) -> f64 {
        match self {
            Self::EqualPower => fade_in_gain(progress),
            Self::Complementary => 1.0 - release_gain(progress),
        }
    }

    /// How far in a band is that stands at `gain`.
    fn progress_at(self, gain: f64) -> f64 {
        let gain = gain.clamp(0.0, 1.0);
        match self {
            Self::EqualPower => gain.asin() / std::f64::consts::FRAC_PI_2,
            Self::Complementary => (1.0 - 2.0 * gain).acos() / std::f64::consts::PI,
        }
    }
}

/// A gain moving to where it was last asked to be over a fixed time, a
/// decibel at a time: as quick to one 25 dB away as to one a decibel away,
/// and never a step.
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    now: f64,
    target: f64,
    step: f64,
    left: usize,
}

impl Glide {
    pub fn at(gain: f64) -> Self {
        Self {
            now: gain,
            target: gain,
            step: 1.0,
            left: 0,
        }
    }

    /// Move to `target` over `samples`, from wherever it stands. One that is
    /// not a gain over nothing, or is nothing itself, is gone to at once.
    pub fn to(&mut self, target: f64, samples: usize) {
        if target == self.target {
            return;
        }
        self.target = target;
        if self.now > 0.0 && target > 0.0 && samples > 0 {
            self.step = (target / self.now).powf(1.0 / samples as f64);
            self.left = samples;
        } else {
            self.now = target;
            self.left = 0;
        }
    }

    /// The gain at the next sample.
    pub fn next(&mut self) -> f64 {
        if self.left > 0 {
            self.left -= 1;
            self.now = if self.left == 0 {
                self.target
            } else {
                self.now * self.step
            };
        }
        self.now
    }

    /// The gain it is going to.
    pub fn target(&self) -> f64 {
        self.target
    }
}

/// One band: the band's own model under the receiver, how loud it is
/// being played while one band hands over to the next, and the AF gain its
/// receiver is played at: full volume's, for that receiver. A band fades
/// out at its own, and the next fades in at its, so a change of receiver
/// never plays one band at the other's.
#[derive(Clone)]
pub struct BandVoice {
    mixer: BandMixer,
    fade: Fade,
    crossfade: Crossfade,
    fade_len: usize,
    warm_len: usize,
    af: Glide,
}

/// One sample of a band.
pub struct Played {
    /// The band, at the level its fade has it at and its AF gain.
    pub band: f64,
    /// The receiver's gain, which the send rides.
    pub gain: f64,
    /// The part of it the AGC's hang holds, for its meter.
    pub held: f64,
    /// The AF gain it is played at, which the send rides too.
    pub af: f64,
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
        let warm_len = (PRE_ROLL_SEC * sr) as usize;
        for _ in 0..warm_len {
            mixer.next(0.0);
        }
        Self {
            mixer,
            fade: Fade::In(0),
            crossfade: Crossfade::EqualPower,
            fade_len: ((BAND_FADE_SEC * sr).round() as usize).max(1),
            warm_len,
            af: Glide::at(1.0),
        }
    }

    /// This band, carried on through `receiver` at AF gain `af`: warming
    /// unheard in step with this one, on the same noise and stations, then
    /// coming in as this one goes ([`Self::give_way`]). Allocates nothing,
    /// so the audio thread may make it.
    pub fn retuned(&self, receiver: Receiver, af: f64) -> Self {
        Self {
            mixer: self.mixer.retuned(receiver),
            fade: Fade::Warming(self.warm_len),
            crossfade: Crossfade::Complementary,
            fade_len: self.fade_len,
            warm_len: self.warm_len,
            af: Glide::at(af),
        }
    }

    /// Give way to a copy of itself through another receiver: held as it is
    /// while that one warms, then fading as it comes in, the two gains
    /// summing to one, as the same band twice does.
    pub fn give_way(&mut self) {
        self.crossfade = Crossfade::Complementary;
        self.fade = Fade::Holding(self.warm_len);
    }

    /// Whether it is on the air at full, coming and going neither.
    pub fn is_steady(&self) -> bool {
        self.fade == Fade::Steady
    }

    /// The receiver `settings` run, seeded with `seed`, if they run one: a
    /// band to play, or an AGC to ride the station on a band with nothing
    /// else on it ([`cw_core::band::BandSettings::runs_receiver`]).
    pub fn for_settings(settings: &TrainingSettings, sample_rate: u32, seed: u64) -> Option<Self> {
        settings
            .band
            .runs_receiver()
            .then(|| Self::new(BandMixer::new(sample_rate, settings, seed), sample_rate))
    }

    /// Play at AF gain `af` from the start: a band coming in.
    pub fn start_at(&mut self, af: f64) {
        self.af = Glide::at(af);
    }

    /// Move to AF gain `af` over the band's fade: what full volume is for
    /// its receiver moved with the keying or the speed, which leave the
    /// band as it is.
    pub fn glide_to(&mut self, af: f64) {
        self.af.to(af, self.fade_len);
    }

    /// Hand over from `previous`: the receiver's gain carries on rather than
    /// opening wide under a station being copied. Whether it could.
    pub fn take_over_from(&mut self, previous: &BandVoice) -> bool {
        self.mixer.carry_gain_from(&previous.mixer)
    }

    /// Let the AGC hear a sample of the send waiting out the look-ahead.
    pub fn hear_ahead(&mut self, send: f64) {
        self.mixer.hear_ahead(send);
    }

    /// Start fading out under a band of its own, from wherever its fade has
    /// got to: one replaced while it was still coming in goes from the level
    /// it had reached, and one still warming is never heard at all.
    pub fn fade_out(&mut self) {
        let len = self.fade_len as f64;
        let level = match self.fade {
            Fade::Warming(_) | Fade::Gone => {
                self.fade = Fade::Gone;
                return;
            }
            Fade::In(done) => self.crossfade.rise(done as f64 / len),
            Fade::Steady | Fade::Holding(_) => 1.0,
            Fade::Out(done) => self.crossfade.rise(1.0 - done as f64 / len),
        };
        // The band coming in is another noise: their powers add.
        self.crossfade = Crossfade::EqualPower;
        let through = self.crossfade.progress_at(level);
        self.fade = Fade::Out(((1.0 - through) * len).round() as usize);
    }

    pub fn is_gone(&self) -> bool {
        self.fade == Fade::Gone
    }

    pub fn reserve_stations(&mut self, callers: usize) {
        self.mixer.reserve_stations(callers);
    }

    /// The next sample, with `send` playing over it, both heard at RF gain
    /// `rf`.
    pub fn next(&mut self, send: f64, rf: f64) -> Played {
        let progress = |done: usize| done as f64 / self.fade_len as f64;
        let rise = |progress: f64| self.crossfade.rise(progress);
        let (weight, fade) = match self.fade {
            Fade::Gone => {
                return Played {
                    band: 0.0,
                    gain: 1.0,
                    held: 1.0,
                    af: self.af.target(),
                    weight: 0.0,
                    input: send * rf,
                };
            }
            Fade::Warming(0) => (0.0, Fade::In(0)),
            Fade::Warming(left) => (0.0, Fade::Warming(left - 1)),
            Fade::Steady => (1.0, Fade::Steady),
            Fade::In(done) if done >= self.fade_len => (1.0, Fade::Steady),
            Fade::In(done) => (rise(progress(done)), Fade::In(done + 1)),
            Fade::Holding(0) => (1.0, Fade::Out(0)),
            Fade::Holding(left) => (1.0, Fade::Holding(left - 1)),
            Fade::Out(done) if done >= self.fade_len => (0.0, Fade::Gone),
            Fade::Out(done) => (rise(1.0 - progress(done)), Fade::Out(done + 1)),
        };
        self.fade = fade;
        self.mixer.set_rf_gain(rf);
        let Received {
            band,
            gain,
            held,
            input,
        } = self.mixer.next(send);
        let af = self.af.next();
        Played {
            band: band * weight * af,
            gain,
            held,
            af,
            weight,
            input,
        }
    }
}
