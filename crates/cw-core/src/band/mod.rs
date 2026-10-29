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
//! 3. The RF gain turns all of it down by the same decibels on its way to
//!    the AGC ([`crate::TrainingSettings::rf_gain`]), and the AGC rides the
//!    gain: a steady few decibels down under a keyed station, back up in the
//!    pauses, and only a moment's duck for a crash. Its knee is set from the
//!    band's own floor ([`agc_knee`]), clear of the hiss. It listens
//!    [`AGC_LOOKAHEAD_SEC`] ahead of what it plays, so its gain is down
//!    before the first element of an over is.
//! 4. The receiver's audio stage adds a very faint, wide hiss of its own
//!    ([`AudioFloor`]), after the filter and the AGC, so the band is never
//!    quite silent outside the passband — as no real receiver is.
//! 5. A soft limiter ([`soft_limit`]) catches whatever is still too loud,
//!    once, on everything the player is about to play, after the volume
//!    ([`crate::TrainingSettings::af_gain`]) has set how loud it is: as loud
//!    as the strongest station comes out of this receiver clean at full RF
//!    gain ([`full_volume_gain`]), at full.
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
mod loudness;
mod memo;
mod noise;
mod output;
mod qsb;
mod random;
mod source;

// What the players, the settings screen and the pile-up planner use. The rest
// is the band's own business.
pub use agc::{AGC_LOOKAHEAD_SEC, agc_decay_sec, agc_lookahead_samples};
pub use calibration::{NOISE_LEVEL_PER_DB, heard_snr_db, whole_db_noise_level};
pub use dsp::{db_to_gain, gain_to_db};
pub(crate) use filter::mirror_hz;
pub use filter::{
    FilterDesign, MAX_RECEIVER_SECTIONS, MAX_RECEIVER_WIDTH_HZ, ReceiverFilter, Section, Sections,
};
pub use key::{BandChange, BandKey};
pub use knee::agc_knee;
pub use loudness::{full_volume_gain, loudest_station};
pub use noise::AudioFloor;
pub use output::{
    AUDIO_HIGH_PASS_HZ, AudioHighPass, SOFT_LIMIT_HEADROOM, SOFT_LIMIT_KNEE, soft_limit,
    soft_limit_curve,
};
pub use qsb::{FADE_OFFSET_MAX_SEC, FADE_STEP_SEC, Qsb};
pub use source::BandSource;

use crate::settings::{BandSettings, STATIONS_MIN, TrainingSettings};
use activity::Tuning;
use agc::Agc;
use calibration::{noise_level, qrn_level};

/// The native receiver: the band, through the filter, under the AGC, one
/// sample at a time, with the send it is riding over. QSB is applied to the
/// Morse before it gets here — fading is a property of the path a station
/// arrives on, not of the noise around it.
///
/// A copy plays on exactly as the original would: what lets a player that
/// renders ahead of the clock go back to a moment and make the rest again.
#[derive(Clone)]
pub struct BandMixer {
    source: BandSource,
    receiver: ReceiverFilter,
    /// The RF gain, as a gain: what the band and the send are multiplied by
    /// on their way to the AGC.
    rf: f64,
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
    /// The gain the AGC is riding at, for what it heard the look-ahead ago
    /// ([`agc_lookahead_samples`]): the band comes out at it, and the send is
    /// to be played at it that late too, after the RF gain. One receiver, one
    /// gain for everything it hears, so a crash ducks the station you are
    /// copying with the noise.
    pub gain: f64,
    /// The part of that gain the AGC's hang holds: what a signal that stood
    /// over the threshold has set, which a crash it only squashes for an
    /// instant leaves alone. What an AGC lamp lights by.
    pub held: f64,
    /// What reached the AGC: the band and the send together, through the
    /// filter and the RF gain, before the AGC's, as it arrived rather than
    /// the look-ahead late. What a level meter on the receiver reads.
    pub input: f64,
}

/// A receiver for a band already on the air: its filter and AGC, and where
/// the band's stations are to keep clear of through it. Built where building
/// may allocate and take its time — the AGC's look-ahead, its knee's
/// measurement — and handed to the band whole ([`BandMixer::retuned`]).
#[derive(Clone)]
pub struct Receiver {
    filter: ReceiverFilter,
    agc: Agc,
    tuning: Tuning,
}

impl Receiver {
    /// The receiver `settings` describe, at `sample_rate`.
    pub fn new(sample_rate: u32, settings: &TrainingSettings) -> Self {
        let settings = settings.clone().clamp();
        let design = FilterDesign::from_settings(&settings);
        Self::through(sample_rate.max(1), &settings.band, &design)
    }

    /// The receiver `design` is, set for `band`.
    fn through(sample_rate: u32, band: &BandSettings, design: &FilterDesign) -> Self {
        Self {
            filter: ReceiverFilter::new(sample_rate, design),
            agc: match agc_knee(band, design) {
                Some(knee) => Agc::new(sample_rate, knee, band.agc_decay),
                None => Agc::off(sample_rate),
            },
            tuning: Tuning::new(band, design),
        }
    }
}

impl BandMixer {
    /// The band these settings describe, through the receiver they describe.
    /// The source, the filter and the AGC's knee all hear the one receiver.
    pub fn new(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let sample_rate = sample_rate.max(1);
        let settings = settings.clone().clamp();
        let band = &settings.band;
        let design = FilterDesign::from_settings(&settings);
        let Receiver { filter, agc, .. } = Receiver::through(sample_rate, band, &design);
        Self {
            source: BandSource::new(sample_rate, band, &design, seed),
            receiver: filter,
            rf: settings.rf_gain(),
            agc,
            audio_floor: AudioFloor::for_band(sample_rate, band, seed),
        }
    }

    /// This band carried on through `receiver`: the same noise, static and
    /// stations, from where they are, heard through another filter and AGC,
    /// the AGC taking the gain over as it was. A rig's filter and AGC act on
    /// whatever is on the air; turning them never changes who is on it.
    ///
    /// Nothing is allocated, the source copied whole and its traffic
    /// shared, so the audio thread may make it.
    pub fn retuned(&self, receiver: Receiver) -> Self {
        let Receiver {
            mut filter,
            mut agc,
            tuning,
        } = receiver;
        filter.align_to(&self.receiver);
        let mut source = self.source.clone();
        source.retune(tuning);
        agc.carry_on_from(&self.agc);
        Self {
            source,
            receiver: filter,
            rf: self.rf,
            agc,
            audio_floor: self.audio_floor.clone(),
        }
    }

    /// One sample of the band, with `send` — the Morse at this instant,
    /// through the receiver's filter already — playing over it. The band
    /// comes out the look-ahead late, and the send is for the player to play
    /// as late, at the gain that comes with it.
    ///
    /// The AGC hears the two together, sample by sample, as a real one does
    /// and as the browser's compressor does by sitting where they have
    /// already met, so the station ducks the band from its first element and
    /// a crash ducks the station with it.
    pub fn next(&mut self, send: f64) -> Received {
        let send = if send.is_finite() { send } else { 0.0 };
        // Everything meets at the receiver's filter, which is why narrowing
        // it quiets the whole band at once rather than one layer of it; and
        // the RF gain turns all of it down alike, so backing it off lowers
        // the station and the noise together and leaves the AGC less to do.
        let heard = self.rf * self.receiver.process(self.source.next_sample());
        let send = self.rf * send;
        let ridden = self.agc.process(heard, send);
        let input = heard + send;
        // The audio stage's own hiss comes in after all of that: the filter
        // never had it, so it runs on outside the passband, and the AGC never
        // saw it, so it neither moves the gain nor ducks under a station.
        Received {
            band: ridden + self.audio_floor.next_sample(),
            gain: self.agc.gain(),
            held: self.agc.held(),
            input,
        }
    }

    /// Hear the band and the send at RF gain `rf` from the next sample on,
    /// as a gain: the player moves it a sample at a time as the control
    /// moves, so turning it neither rebuilds the band nor clicks. Anything
    /// that is not a number, or is less than nothing, is left alone.
    pub fn set_rf_gain(&mut self, rf: f64) {
        if rf.is_finite() && rf >= 0.0 {
            self.rf = rf;
        }
    }

    /// A pile-up is calling with this many stations besides yours: they come
    /// out of how many of the band's own stations may key at once, for as
    /// long as they call. Zero when the send is over.
    pub fn reserve_stations(&mut self, callers: usize) {
        self.source.reserve_stations(callers);
    }

    /// Take the receiver's gain over from the band this one replaces, so a
    /// station being copied across the change is not ducked afresh. Whether
    /// there was a gain to take: an AGC switched off has none.
    pub fn carry_gain_from(&mut self, previous: &BandMixer) -> bool {
        self.agc.carry_on_from(&previous.agc)
    }

    /// Let the AGC hear `send`, a sample of the send still waiting out the
    /// look-ahead, through the RF gain already, without the band moving on:
    /// a band put in under a station its AGC never heard, the AGC switched
    /// on or the receiver new, has the station's gain by the time the send
    /// it never heard is played at it.
    pub fn hear_ahead(&mut self, send: f64) {
        if send.is_finite() {
            let _ = self.agc.next_gain(send);
        }
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

    /// Whether the receiver has anything to do but filter the station: a
    /// band to play, or an AGC to ride the station with. A rig's AGC rides
    /// whatever reaches it, on a dead band as much as a busy one, and on a
    /// quiet one is where its evening out is plainest to hear.
    pub fn runs_receiver(&self) -> bool {
        self.has_background() || self.agc_enabled
    }
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests;
