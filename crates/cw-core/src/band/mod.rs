//! The band: what a receiver in CW mode hears around the station you are
//! copying, shared by every backend.
//!
//! The names mean what an operator means by them. The noise floor is the
//! steady hiss of the band itself. QRN is atmospheric static — lightning. QSB
//! is fading. QRM, another station on top of yours, lives with the stations
//! rather than in here.
//!
//! The model follows the signal chain of a real receiver, because that is
//! where the sound comes from:
//!
//! 1. The band puts white Gaussian noise and impulsive static into the
//!    antenna ([`noise`]). Neither has a pitch.
//! 2. Everything passes through one CW filter ([`filter`]), the Morse
//!    included. That filter is what gives the hiss its pitch and width and
//!    makes every crash and every dit ring; narrowing it quiets the band,
//!    pushes off-frequency stations down and lengthens the ringing, all at
//!    once, because those are one thing.
//! 3. The AGC rides the gain ([`agc`]), ducking the whole band behind a
//!    crash and letting it back up.
//!
//! What it deliberately does not do is invent a sound the receiver would not
//! make. An earlier model fed sparse clicks into resonators a few hertz wide
//! and swept their tuning, which is how a wind-whistle is synthesised, and it
//! sounded like one. A real CW filter is hundreds of hertz wide, sits still,
//! and is fed with continuous noise; this is that.

pub mod agc;
pub mod filter;
pub mod noise;
pub mod qsb;

pub use agc::{AGC_ATTACK_SEC, AGC_MAX_DUCK, AGC_RELEASE_SEC, AGC_TRIGGER, Agc};
pub use filter::{
    FilterDesign, RECEIVER_SECTIONS, ReceiverFilter, Section, mirror_hz, passband_edges,
};
pub use noise::{
    Atmospherics, BandSource, Gaussian, NoiseFloor, REFERENCE_BANDWIDTH_HZ, SIGNAL_RMS,
    heard_snr_db, noise_floor_rms, noise_floor_snr_db,
};
pub use qsb::{QSB_MIN_GAIN, QSB_PATHS, QSB_SPREAD, qsb_gain_at, qsb_path};

use crate::settings::TrainingSettings;

/// What a receiver does with a loud crash: catches it, rather than letting it
/// square off. Linear up to the knee, asymptotic to full scale above it — so
/// the biggest static is loud without turning into a buzz.
pub fn soft_limit(x: f64) -> f64 {
    const KNEE: f64 = 0.7;
    let magnitude = x.abs();
    if magnitude <= KNEE {
        return x;
    }
    let over = (magnitude - KNEE) / (1.0 - KNEE);
    (KNEE + (1.0 - KNEE) * over.tanh()).copysign(x)
}

/// How far past full scale [`soft_limit_curve`] reaches before it simply holds
/// at its last value. Far beyond anything the band and the Morse add up to.
pub const SOFT_LIMIT_HEADROOM: f64 = 4.0;

/// [`soft_limit`] as a lookup table, for a Web Audio `WaveShaperNode`.
///
/// A shaper maps its input over -1..=1 onto the curve, so the input is to be
/// scaled down by [`SOFT_LIMIT_HEADROOM`] first; the curve maps that back up
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

/// How loud this band sits when nothing unusual is happening.
///
/// The browser cannot run the receiver's own AGC — its noise and its Morse
/// only meet as sound, and there is no arithmetic it can do across both. What
/// it has is a compressor node, and a compressor needs a threshold in absolute
/// terms, which is the very thing that would undo the filter. So it is given
/// one measured from these settings: narrow the receiver, the floor drops, the
/// threshold drops with it, and a crash still has to stand the same distance
/// above the band to duck it.
pub fn band_standing_level(sample_rate: u32, settings: &TrainingSettings) -> f64 {
    let mut mixer = BandMixer::new(sample_rate, settings, 0x5EED_1234);
    let mut buf = vec![0.0f32; sample_rate.max(1) as usize];
    for _ in 0..3 {
        mixer.fill_background(&mut buf);
    }
    mixer.agc_standing()
}

/// The native receiver: the band, through the filter, under the AGC, one
/// sample at a time. QSB is applied separately, to the Morse samples
/// themselves — fading is a property of the path a station arrives on, not of
/// the noise around it.
pub struct BandMixer {
    source: BandSource,
    receiver: ReceiverFilter,
    agc: Agc,
    send_level: f64,
}

impl BandMixer {
    pub fn new(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let sample_rate = sample_rate.max(1);
        let settings = settings.clone().clamp();
        Self {
            source: BandSource::new(sample_rate, &settings, seed),
            receiver: ReceiverFilter::from_settings(sample_rate, &settings),
            agc: Agc::new(sample_rate),
            send_level: 0.0,
        }
    }

    /// Whether these settings put anything on the band at all. When they do
    /// not, there is no background stream to run.
    pub fn needs_background(settings: &TrainingSettings) -> bool {
        let band = &settings.band;
        (band.qrn_enabled && band.qrn_level > 0.0) || (band.noise_enabled && band.noise_level > 0.0)
    }

    pub fn next_background(&mut self) -> f32 {
        // Everything meets at the receiver's filter, which is why narrowing
        // it quiets the whole band at once rather than one layer of it.
        let heard = self.receiver.process(self.source.next_sample());
        // The gain the receiver is riding at, which a crash has just pulled
        // down and which everything else has to come down with — including
        // the send, which reads this through `agc_gain`.
        let gain = self.agc.next_gain(heard.abs().max(self.send_level));
        soft_limit(heard * gain) as f32
    }

    /// How loud the send is, so the receiver's gain answers to everything
    /// reaching it rather than to the noise alone — which is what a real one
    /// does, and what the browser's compressor does for free by sitting where
    /// the two have already met.
    pub fn note_send_level(&mut self, level: f64) {
        self.send_level = if level.is_finite() {
            level.max(0.0)
        } else {
            0.0
        };
    }

    /// What the AGC is holding the band down to right now.
    ///
    /// The send rides this too. The two are separate playback objects by the
    /// time they reach the sound card, so the only honest way for a crash to
    /// duck the Morse as well is for the send to read the gain the receiver
    /// arrived at.
    pub fn agc_gain(&self) -> f64 {
        self.agc.gain()
    }

    /// What the AGC reckons this band's standing level is.
    pub fn agc_standing(&self) -> f64 {
        self.agc.standing()
    }

    pub fn fill_background(&mut self, out: &mut [f32]) {
        for slot in out {
            *slot = self.next_background();
        }
    }
}

#[cfg(test)]
mod tests;
