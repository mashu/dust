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
//! 1. The band puts white Gaussian noise, impulsive static and other
//!    stations into the antenna ([`BandSource`]). Only the stations have a
//!    pitch.
//! 2. Everything passes through one CW filter ([`FilterDesign`]), the Morse
//!    included. That filter is what gives the hiss its pitch and width and
//!    makes every crash and every dit ring; narrowing it quiets the band,
//!    pushes off-frequency stations down and lengthens the ringing, all at
//!    once, because those are one thing.
//! 3. The AGC rides the gain: a steady few decibels down under a keyed
//!    station, back up in the pauses, and only a moment's duck for a crash.
//!    Its knee is set from the band's own floor ([`agc_knee`]).
//! 4. The receiver's audio stage adds a very faint, wide hiss of its own
//!    ([`AudioFloor`]), after the filter and the AGC, so the band is never
//!    quite silent outside the passband — as no real receiver is.
//! 5. A soft limiter ([`soft_limit`]) catches whatever is still too loud,
//!    once, on everything the player is about to play, after the volume
//!    ([`crate::TrainingSettings::output_gain`]) has set how loud it is.
//!
//! QSB is applied to the Morse alone ([`Qsb`]): fading is a property of the
//! path a station arrives on, not of the noise around it.
//!
//! What it deliberately does not do is invent a sound the receiver would not
//! make. Sparse clicks through resonators a few hertz wide, their tuning
//! swept about, is how a wind-whistle is synthesised. A real CW filter is
//! hundreds of hertz wide, sits still, and is fed with continuous noise; this
//! is that.

mod activity;
mod agc;
mod calibration;
mod dsp;
mod filter;
mod key;
mod knee;
mod memo;
mod noise;
mod output;
mod qsb;
mod random;
mod source;

// What the players, the settings screen and the pile-up planner use. The rest
// is the band's own business.
pub use agc::{
    AGC_ATTACK_SEC, AGC_COMPRESSOR_KNEE_DB, AGC_RELEASE_SEC, CompressorSetting, agc_compressor,
};
pub use calibration::heard_snr_db;
pub(crate) use filter::mirror_hz;
pub use filter::{
    FilterDesign, MAX_RECEIVER_SECTIONS, MAX_RECEIVER_WIDTH_HZ, ReceiverFilter, Section, Sections,
};
pub use key::BandKey;
pub use knee::agc_knee;
pub use noise::AudioFloor;
pub use output::{
    FULL_VOLUME_GAIN, LimiterFeed, SOFT_LIMIT_KNEE, limiter_feed, soft_limit, soft_limit_curve,
};
pub use qsb::Qsb;
pub use source::BandSource;

use crate::settings::{BandSettings, STATIONS_MIN, TrainingSettings};
use agc::Agc;
use calibration::{noise_level, qrn_level};

/// The native receiver: the band, through the filter, under the AGC, one
/// sample at a time, with the send it is riding over. QSB is applied to the
/// Morse before it gets here — fading is a property of the path a station
/// arrives on, not of the noise around it.
pub struct BandMixer {
    source: BandSource,
    receiver: ReceiverFilter,
    agc: Agc,
    /// The receiver's audio stage, joining after the AGC.
    audio_floor: AudioFloor,
}

/// One sample of what the receiver plays, before the output stage's limiter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Received {
    /// The band: its noise, static and stations through the filter and the
    /// AGC, and the audio stage's own floor.
    pub band: f64,
    /// The gain the AGC is riding at. The send is played at it: one receiver,
    /// one gain for everything it hears, so a crash ducks the station you are
    /// copying with the noise.
    pub gain: f64,
}

impl BandMixer {
    /// The band these settings describe, through the receiver they describe.
    /// The source, the filter and the AGC's knee all hear the one receiver.
    pub fn new(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let sample_rate = sample_rate.max(1);
        let settings = settings.clone().clamp();
        let band = &settings.band;
        let receiver = FilterDesign::from_settings(&settings);
        Self {
            source: BandSource::new(sample_rate, band, &receiver, seed),
            receiver: ReceiverFilter::new(sample_rate, &receiver),
            agc: match agc_knee(band, &receiver) {
                Some(knee) => Agc::new(sample_rate, knee),
                None => Agc::off(),
            },
            audio_floor: AudioFloor::for_band(sample_rate, band, seed),
        }
    }

    /// One sample of the band, with `send` — the Morse at this instant,
    /// through the receiver's filter already — playing over it.
    ///
    /// The AGC hears the two together, sample by sample, as a real one does
    /// and as the browser's compressor does by sitting where they have
    /// already met, so the station ducks the band from its first element and
    /// a crash ducks the station with it.
    pub fn next(&mut self, send: f64) -> Received {
        let send = if send.is_finite() { send } else { 0.0 };
        // Everything meets at the receiver's filter, which is why narrowing
        // it quiets the whole band at once rather than one layer of it.
        let heard = self.receiver.process(self.source.next_sample());
        let ridden = self.agc.process(heard, send);
        // The audio stage's own hiss comes in after all of that: the filter
        // never had it, so it runs on outside the passband, and the AGC never
        // saw it, so it neither moves the gain nor ducks under a station.
        Received {
            band: ridden + self.audio_floor.next_sample(),
            gain: self.agc.gain(),
        }
    }

    /// A pile-up is calling with this many stations besides yours: they come
    /// out of how many of the band's own stations may key at once, for as
    /// long as they call. Zero when the send is over.
    pub fn reserve_stations(&mut self, callers: usize) {
        self.source.reserve_stations(callers);
    }

    /// Take the receiver's gain over from the band this one replaces, so a
    /// station being copied across the change is not ducked afresh.
    pub fn carry_gain_from(&mut self, previous: &BandMixer) {
        self.agc.carry_on_from(&previous.agc);
    }

    /// The band alone, as the player would play it with nothing sent over
    /// it: through the output stage's limiter.
    #[cfg(test)]
    pub(crate) fn fill_background(&mut self, out: &mut [f32]) {
        for slot in out {
            *slot = soft_limit(self.next(0.0).band) as f32;
        }
    }
}

impl BandSettings {
    /// Whether these settings put anything on the band at all. When they do
    /// not, there is no background stream to run.
    pub fn has_background(&self) -> bool {
        qrn_level(self) > 0.0 || noise_level(self) > 0.0 || self.stations_max > STATIONS_MIN
    }
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests;
