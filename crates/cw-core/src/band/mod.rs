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
//! 3. The AGC rides the gain ([`agc`]): a steady few decibels down under a
//!    keyed station, back up in the pauses, and only a moment's duck for a
//!    crash.
//!
//! What it deliberately does not do is invent a sound the receiver would not
//! make. An earlier model fed sparse clicks into resonators a few hertz wide
//! and swept their tuning, which is how a wind-whistle is synthesised, and it
//! sounded like one. A real CW filter is hundreds of hertz wide, sits still,
//! and is fed with continuous noise; this is that.

pub mod activity;
pub mod agc;
pub mod filter;
pub mod noise;
pub mod qsb;

pub use activity::BandActivity;
pub use agc::{
    AGC_ATTACK_SEC, AGC_COMPRESSOR_KNEE_DB, AGC_COMPRESSOR_RATIO, AGC_HANG_SEC, AGC_MAX_DUCK_DB,
    AGC_RELEASE_SEC, AGC_SLOPE, AGC_TRIGGER, Agc, CompressorSetting, agc_compressor,
    agc_compressor_threshold_db, agc_law_db, compressor_curve_db, compressor_makeup_db,
};
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

/// How high this band's floor ordinarily peaks, through the receiver.
///
/// The level a 60 ms peak follower on the filtered band sits at, as a median
/// over three seconds so a crash that happens to land in the measurement does
/// not move it: about 2.2 times the floor's RMS for plain hiss, more on a band
/// busy with crackle. Other stations are left out — they are what the AGC
/// rides over, not the floor it is set against.
///
/// Measured rather than worked out, because the filter's shape and width and
/// the static all move it. The band is calibrated to sound the same at any
/// sample rate, so a low rate measures it as well as a high one.
pub fn band_standing_level(sample_rate: u32, settings: &TrainingSettings) -> f64 {
    const SEED: u64 = 0x5EED_1234;
    const SETTLE_SEC: f64 = 0.25;
    const MEASURE_SEC: f64 = 2.75;
    const FOLLOWER_SEC: f64 = 0.06;
    const BLOCK_SEC: f64 = 0.01;
    let sample_rate = sample_rate.max(1);
    let sr = f64::from(sample_rate);
    let mut floor = settings.clone().clamp();
    floor.band.activity_enabled = false;
    let mut source = BandSource::new(sample_rate, &floor, SEED);
    let mut receiver = ReceiverFilter::from_settings(sample_rate, &floor);
    let fall = (-1.0 / (FOLLOWER_SEC * sr)).exp();
    let mut follower = 0.0f64;
    let mut step = || {
        let heard = receiver.process(source.next_sample()).abs();
        follower = heard.max(follower * fall);
        follower
    };
    for _ in 0..(SETTLE_SEC * sr) as usize {
        step();
    }
    let block = ((BLOCK_SEC * sr) as usize).max(1);
    let mut levels: Vec<f64> = (0..((MEASURE_SEC / BLOCK_SEC) as usize))
        .map(|_| {
            let mut last = 0.0;
            for _ in 0..block {
                last = step();
            }
            last
        })
        .collect();
    levels.sort_by(f64::total_cmp);
    levels.get(levels.len() / 2).copied().unwrap_or(0.0)
}

/// The rate the floor is measured at for the AGC's knee. Both backends use
/// it, so both arrive at the same knee to the bit; it is low because the
/// browser measures on its main thread every time a band setting moves.
pub const FLOOR_MEASURE_RATE: u32 = 16_000;

/// Where the receiver's AGC starts to turn the gain down, as the amplitude of
/// a steady tone; `None` with the AGC switched off.
///
/// [`AGC_TRIGGER`] times the band's own floor ([`band_standing_level`]), and
/// never below that of the default floor heard through a
/// [`REFERENCE_BANDWIDTH_HZ`] filter. Above that it follows the floor, the way
/// an operator backs the RF gain off on a noisy band or a wide filter, so the
/// hiss on its own never pumps the gain. Below it, it stays put, as a real
/// receiver's AGC threshold does: closing the filter or finding a quiet band
/// takes noise away from under the knee, and leaves the station you are
/// copying exactly as loud as it was.
pub fn agc_knee(settings: &TrainingSettings) -> Option<f64> {
    if !settings.band.agc_enabled {
        return None;
    }
    let floor = band_standing_level(FLOOR_MEASURE_RATE, settings).max(reference_floor_level());
    Some(AGC_TRIGGER * floor)
}

/// [`band_standing_level`] of the default floor alone, through a filter
/// [`REFERENCE_BANDWIDTH_HZ`] wide. Measured once.
fn reference_floor_level() -> f64 {
    static LEVEL: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *LEVEL.get_or_init(|| {
        let mut settings = TrainingSettings::default();
        settings.band.noise_enabled = true;
        settings.band.noise_level = noise::QRN_REFERENCE_NOISE_LEVEL;
        settings.band.qrn_enabled = false;
        settings.band.activity_enabled = false;
        settings.band.filter_bandwidth_hz = REFERENCE_BANDWIDTH_HZ;
        band_standing_level(FLOOR_MEASURE_RATE, &settings)
    })
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
            agc: match agc_knee(&settings) {
                Some(knee) => Agc::new(sample_rate, knee),
                None => Agc::off(),
            },
            send_level: 0.0,
        }
    }

    /// Whether these settings put anything on the band at all. When they do
    /// not, there is no background stream to run.
    pub fn needs_background(settings: &TrainingSettings) -> bool {
        let band = &settings.band;
        (band.qrn_enabled && band.qrn_level > 0.0)
            || (band.noise_enabled && band.noise_level > 0.0)
            || (band.activity_enabled && band.activity_level > 0.0)
    }

    pub fn next_background(&mut self) -> f32 {
        // Everything meets at the receiver's filter, which is why narrowing
        // it quiets the whole band at once rather than one layer of it.
        let heard = self.receiver.process(self.source.next_sample());
        // At the gain the receiver is riding at, which a crash has just
        // pulled down and which everything else has to come down with —
        // including the send, which reads this through `agc_gain`.
        soft_limit(self.agc.process(heard, self.send_level)) as f32
    }

    /// How loud the send is — its peak, as the native player hands it over a
    /// buffer at a time — so the receiver's gain answers to everything
    /// reaching it rather than to the noise alone. That is what a real one
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

    /// Where this receiver's AGC starts to act; `None` when it is off.
    pub fn agc_knee(&self) -> Option<f64> {
        self.agc.knee()
    }

    pub fn fill_background(&mut self, out: &mut [f32]) {
        for slot in out {
            *slot = self.next_background();
        }
    }
}

#[cfg(test)]
mod tests;
