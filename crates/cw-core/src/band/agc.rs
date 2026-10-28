//! The receiver's automatic gain control.
//!
//! One static law: below a fixed knee the gain is left alone, and above it
//! the gain comes down nine decibels for every ten of excess, never more
//! than [`AGC_MAX_DUCK_DB`], run sample by sample in [`Agc`], with a hang.
//! Both players run it, the browser rendering the native mixer ahead of its
//! clock.
//!
//! What it is tuned to is what an operator hears from a rig's AGC:
//!
//! - Everything over the knee comes out within a few decibels of everything
//!   else: a station 20 dB down plays nearly as loud as the strongest, and
//!   only those under the knee keep their own level. The knee is the AGC's
//!   threshold, and raising it lets more of them keep it.
//! - Under a keyed station the band's hiss sits a steady way down, as far as
//!   the station stands over the knee, from the first element of the over to
//!   the last. No swell at the start, no pumping between elements: the hang
//!   carries the gain across the gaps. Backing the RF gain off is what makes
//!   it duck less.
//! - In a pause the hiss is back, as fast as the decay setting lets it.
//! - A crash pulls the gain for a moment and lets go; it does not dig a hole
//!   in the band for the next second.
//! - The first element of an over is caught before it plays: the receiver
//!   listens [`AGC_LOOKAHEAD_SEC`] ahead of what it plays, so nothing slips
//!   past the gain wide open and comes out louder than the AGC holds it.
//!
//! The knee is fixed, set from the band's floor, rather than learned from
//! what is heard: an AGC that learns the band's level learns the station with
//! it, starts every over ducked and swells up as it gets used to it, and
//! leaves the floor sagging for most of a second after every crash. Real
//! receivers do neither.

use super::dsp::db_to_gain;
use crate::settings::AgcDecay;

/// How far above the floor's own peaks the knee sits: a level has to stand
/// 2.2 times (about 7 dB) above the highest the hiss ordinarily reaches before
/// the gain moves. The floor alone never gets there, so on an empty band the
/// receiver leaves the noise exactly as the filter shaped it.
pub const AGC_TRIGGER: f64 = 2.2;
/// Decibels of gain reduction per decibel above the knee: a 10:1
/// compressor, one decibel out for every ten in, as a rig's AGC holds its
/// audio within a few decibels from S3 to S9 and over.
///
/// The strongest station the band sends stands about 28 dB over the lowest
/// knee and comes out under 3 dB over it; one 20 dB weaker comes out a
/// decibel over it. Under the strongest the band's hiss ducks 25 dB.
pub const AGC_SLOPE: f64 = 0.9;
/// The most the gain ever comes down, in decibels: more than the strongest
/// station ever needs over the lowest knee, with room left for a crash.
pub const AGC_MAX_DUCK_DB: f64 = 40.0;
/// Seconds for the gain to close on a level it has to duck to, in decibels:
/// a station 28 dB over the knee is within a decibel of its gain in under
/// seven milliseconds, well inside the look-ahead. Fast, because a receiver
/// has to catch a crash before it is deafening.
pub const AGC_ATTACK_SEC: f64 = 0.002;
/// How long the gain holds after the last sustained signal at Fast decay:
/// long enough to carry it across the gaps between elements and characters
/// (165 ms at 22 WPM), short enough that a word gap or a pause lets the band
/// back. Operators prefer hang AGC on CW for exactly this: a per-element
/// pumping AGC is the one everyone dislikes.
pub const AGC_HANG_SEC: f64 = 0.22;
/// Once the hang has run out, the time constant the gain recovers with at
/// Fast decay, in decibels: the way down left shrinks by e every this long,
/// as an analogue AGC's capacitor discharges into a gain control that works
/// in decibels. The bulk of a 25 dB duck is back a tenth of a second after
/// the hang and the last decibel by 0.7 s, so a pause of a second has the
/// band fully back; between the FAST and MID settings of a typical
/// transceiver (0.1 and 0.5 s).
pub const AGC_RELEASE_SEC: f64 = 0.15;
/// The hang at Slow decay: across a word gap at 15 WPM (0.56 s), so the band
/// stays down from one word of an over to the next.
pub const AGC_SLOW_HANG_SEC: f64 = 0.6;
/// The release at Slow decay, in decibels as at Fast: the band back over a
/// second or so after the hang, as a typical transceiver's SLOW.
pub const AGC_SLOW_RELEASE_SEC: f64 = 0.5;

/// The hang and the release `decay` runs the AGC with, in seconds.
pub fn agc_decay_sec(decay: AgcDecay) -> (f64, f64) {
    match decay {
        AgcDecay::Fast => (AGC_HANG_SEC, AGC_RELEASE_SEC),
        AgcDecay::Slow => (AGC_SLOW_HANG_SEC, AGC_SLOW_RELEASE_SEC),
    }
}

/// The detector's smoothing: a one-pole average of power with this time
/// constant, which weighs the last five milliseconds or so. A tone reads as
/// steady through it, while a two-millisecond click is spread thin.
pub const AGC_DETECTOR_SEC: f64 = 0.0025;
/// How long the detector has to stand above the knee before the hang trusts
/// it. A Morse element is tens of milliseconds of steady tone — 55 ms for a
/// dit at 22 WPM, 20 ms at 60 — while a sferic or a lightning stroke through a
/// CW filter is a few milliseconds of ringing. Without this every crash would
/// hold the band down for a quarter of a second after it had gone.
pub const AGC_QUALIFY_SEC: f64 = 0.015;
/// The stretch the qualifying peaks are taken over: longer than half a cycle
/// of the lowest pitch a station can have (200 Hz), so a steady tone always
/// has a crest in it, and short enough that a click is forgotten within
/// twice this of ending.
pub const AGC_PEAK_HOLD_SEC: f64 = 0.003;
/// How quickly the gain comes back after a short impulse that never armed the
/// hang. MorseRunner's AGC is back 20-30 ms after a click; so is this one.
pub const AGC_LIMIT_RELEASE_SEC: f64 = 0.02;
/// The most the receiver lets through to its audio stage, in decibels over
/// the knee: about three over where the law holds the strongest station. A
/// crash comes and goes faster than the law rides it, and full volume lifts
/// what the AGC plays as far as that station plays clean, so a crash let
/// through as it came would drive the limiter flat.
pub const AGC_CEILING_DB: f64 = 6.0;
/// How far ahead of what it plays the receiver listens: long enough that the
/// gain is down before the first element of an over is played, the
/// detector, the attack and the keying's own rise all included, and the
/// ceiling in place before a stroke's first crest. A DSP receiver's AGC
/// delays its audio the same way; MorseRunner's looks 28 ms ahead.
pub const AGC_LOOKAHEAD_SEC: f64 = 0.01;
/// How fast the ceiling closes: a fifth of the look-ahead, so it has finished
/// before the crest it saw coming is played.
const AGC_CEILING_ATTACK_SEC: f64 = AGC_LOOKAHEAD_SEC / 5.0;

/// How many samples the receiver plays its audio late at `sample_rate`:
/// [`AGC_LOOKAHEAD_SEC`], and never none. What the AGC delays the band by,
/// and what the player has to delay the send by for the AGC to have heard it
/// coming.
pub fn agc_lookahead_samples(sample_rate: u32) -> usize {
    (AGC_LOOKAHEAD_SEC * f64::from(sample_rate.max(1)))
        .round()
        .max(1.0) as usize
}
/// Where the detector's power counts as nothing: a tone 300 dB under full
/// scale, far below any knee.
const SETTLED_POWER: f64 = 1e-30;

/// The receiver's automatic gain control.
///
/// More than a limiter, and not a level follower either. It compares everything
/// reaching the receiver — the band and the station being copied — against a
/// fixed knee set from the band's own floor, and turns the gain down by the
/// static law when it is exceeded. Two paths decide how long that lasts:
///
/// - **Hang.** A level that stays above the knee for [`AGC_QUALIFY_SEC`] is a
///   signal, not a click. It sets the gain, and the gain then holds for the
///   decay's hang after the signal last stood there before recovering at its
///   release ([`agc_decay_sec`]). Under a keyed station that hold is unbroken
///   from the first element to the last, so the band sits steadily down for
///   the whole over and comes back in the pauses.
/// - **Limiter.** Anything above the knee pulls the gain down within
///   [`AGC_ATTACK_SEC`], but if it never qualified for the hang it lets go
///   again within [`AGC_LIMIT_RELEASE_SEC`]. A lightning stroke is squashed
///   for the few milliseconds it lasts, and the crackle of the crash around it
///   carries on at full level.
///
/// Under both, a ceiling: whatever the law says, nothing louder than
/// [`AGC_CEILING_DB`] over the knee reaches the audio stage.
///
/// The receiver plays everything [`AGC_LOOKAHEAD_SEC`] late, at the gain
/// worked out from what it has heard up to now ([`Agc::process`]): the gain
/// leads what it is played on by the look-ahead, so it is already down when
/// the first element of an over, or the first crest of a stroke, is played.
///
/// The detector is the power of everything reaching it — the band and the
/// send playing over it, sample by sample, as a real receiver's AGC hears
/// them — smoothed over a few milliseconds and read as the amplitude of a
/// steady tone of the same power. A station keyed at amplitude `a` reads `a`,
/// whether it is the one being copied or one of the band's own. Whether it
/// has *stayed* above the knee is judged on the peaks of the last few
/// milliseconds as well, which fall away as soon as a sound has gone however
/// loud it was: the smoothed level takes longer to forget a loud stroke than
/// a stroke lasts, and would otherwise let the loudest ones pass for signals.
#[derive(Clone)]
pub struct Agc {
    /// The knee, as the amplitude of a steady tone. Infinite when switched
    /// off.
    knee: f64,
    /// The most it lets through, [`AGC_CEILING_DB`] over the knee.
    ceiling_level: f64,
    /// The detector: smoothed power, in squared tone amplitude.
    power: f64,
    smoothing: f64,
    /// The loudest peak of this stretch of [`AGC_PEAK_HOLD_SEC`] and of the
    /// one before it, and how long this one has left.
    peak: f64,
    last_peak: f64,
    peak_left: u32,
    peak_hold: u32,
    /// The loudest crest of this stretch as long as the look-ahead and of the
    /// one before it, and how long this one has left: everything still
    /// waiting to be played is in one of the two.
    crest: f64,
    last_crest: f64,
    crest_left: u32,
    crest_hold: u32,
    /// Samples the detector has stood above the knee without a break.
    above: u32,
    qualify: u32,
    hang: u32,
    hang_left: u32,
    /// The gain the hang is holding.
    held: f64,
    /// The gain the limiter wants.
    limited: f64,
    /// The gain that keeps the loudest crest coming under the ceiling.
    ceiling: f64,
    gain: f64,
    deepest: f64,
    attack: f64,
    release: f64,
    limit_release: f64,
    ceiling_attack: f64,
    /// The band, waiting out the look-ahead.
    delay: Vec<f64>,
    delay_at: usize,
}

impl Agc {
    /// An AGC with its knee at `knee`, the amplitude of a steady tone, that
    /// lets the gain back up as `decay` does.
    pub fn new(sample_rate: u32, knee: f64, decay: AgcDecay) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let coefficient = |seconds: f64| (-1.0 / (seconds * sr)).exp();
        let samples = |seconds: f64| (seconds * sr).round().max(1.0) as u32;
        let knee = if knee > 0.0 { knee } else { f64::INFINITY };
        let lookahead = agc_lookahead_samples(sample_rate);
        let (hang, release) = agc_decay_sec(decay);
        Self {
            knee,
            ceiling_level: knee * db_to_gain(AGC_CEILING_DB),
            power: 0.0,
            smoothing: coefficient(AGC_DETECTOR_SEC),
            peak: 0.0,
            last_peak: 0.0,
            peak_left: samples(AGC_PEAK_HOLD_SEC),
            peak_hold: samples(AGC_PEAK_HOLD_SEC),
            crest: 0.0,
            last_crest: 0.0,
            crest_left: lookahead as u32,
            crest_hold: lookahead as u32,
            above: 0,
            qualify: samples(AGC_QUALIFY_SEC),
            hang: samples(hang),
            hang_left: 0,
            held: 1.0,
            limited: 1.0,
            ceiling: 1.0,
            gain: 1.0,
            deepest: db_to_gain(-AGC_MAX_DUCK_DB),
            attack: coefficient(AGC_ATTACK_SEC),
            release: coefficient(release),
            limit_release: coefficient(AGC_LIMIT_RELEASE_SEC),
            ceiling_attack: coefficient(AGC_CEILING_ATTACK_SEC),
            // Allocated here, once: the audio callback only ever writes into it.
            delay: vec![0.0; lookahead],
            delay_at: 0,
        }
    }

    /// A receiver run on its RF gain: the gain is one, whatever arrives, and
    /// the band is played the look-ahead late all the same, so switching the
    /// AGC on or off never moves it against the send.
    pub fn off(sample_rate: u32) -> Self {
        Self::new(sample_rate, f64::INFINITY, AgcDecay::Fast)
    }

    pub fn is_off(&self) -> bool {
        !self.knee.is_finite()
    }

    /// Feed one sample of the band as the receiver hears it, and one of the
    /// send playing over it, through the same filter; get back the band to
    /// play, [`AGC_LOOKAHEAD_SEC`] late, at the gain worked out from
    /// everything heard up to now. The send is to be played the same
    /// look-ahead late, at [`Agc::gain`]. Switched off, the band goes
    /// through at one.
    pub fn process(&mut self, heard: f64, send: f64) -> f64 {
        let gain = self.next_gain(heard + send);
        let due = std::mem::replace(&mut self.delay[self.delay_at], heard);
        self.delay_at = (self.delay_at + 1) % self.delay.len();
        due * gain
    }

    /// Feed one sample of everything reaching the AGC — the band and the send
    /// over it, together; get back the gain what was heard the look-ahead ago
    /// should be played at.
    pub fn next_gain(&mut self, sample: f64) -> f64 {
        if self.is_off() {
            return 1.0;
        }
        // A sample of a tone of amplitude a carries a²/2 of power on average,
        // so samples count double: a steady tone reads as its amplitude.
        let input = 2.0 * sample * sample;
        let input = if input.is_finite() { input } else { 0.0 };
        self.power = input + (self.power - input) * self.smoothing;
        // In silence the average dies away towards zero for ever; brought to
        // rest once it is nothing, rather than left to sink into the
        // subnormal range, where it and the square root below would take a
        // microcode assist on every sample.
        if self.power < SETTLED_POWER {
            self.power = 0.0;
        }
        let level = self.power.sqrt();
        let wanted = self.law(level);

        // Down fast, and up again fast — on its own this is a limiter.
        self.limited = if wanted < self.limited {
            closer_in_db(self.limited, wanted, self.attack)
        } else {
            wanted + (self.limited - wanted) * self.limit_release
        };

        // The peaks of the last few milliseconds, for judging whether
        // anything is still there, and the crests of the last look-ahead,
        // for the ceiling.
        let magnitude = if sample.is_finite() {
            sample.abs()
        } else {
            0.0
        };
        self.peak = self.peak.max(magnitude);
        self.peak_left -= 1;
        let recent = self.peak.max(self.last_peak);
        if self.peak_left == 0 {
            self.last_peak = self.peak;
            self.peak = 0.0;
            self.peak_left = self.peak_hold;
        }
        self.crest = self.crest.max(magnitude);
        self.crest_left -= 1;
        let coming = self.crest.max(self.last_crest);
        if self.crest_left == 0 {
            self.last_crest = self.crest;
            self.crest = 0.0;
            self.crest_left = self.crest_hold;
        }

        // The hang: only a level that has stood above the knee long enough to
        // be a signal sets it or keeps it.
        self.above = if level > self.knee && recent > self.knee {
            self.above.saturating_add(1)
        } else {
            0
        };
        if self.above >= self.qualify {
            self.hang_left = self.hang;
            // Both ways in decibels: down at the attack, and up, to a weaker
            // signal still standing over the knee, at the release.
            let rate = if wanted < self.held {
                self.attack
            } else {
                self.release
            };
            self.held = closer_in_db(self.held, wanted, rate);
        } else if self.hang_left > 0 {
            self.hang_left -= 1;
        } else if self.held < 1.0 {
            // Back at one once within a millionth of it, a ten-thousandth of
            // a decibel: in decibels it would only ever get closer.
            self.held = closer_in_db(self.held, 1.0, self.release);
            if self.held > 1.0 - 1e-6 {
                self.held = 1.0;
            }
        }

        // The ceiling, on the crests still waiting to be played, at the gain
        // the law is on its way to: by the time a station's first crests are
        // played the law has them, and the ceiling is for what it never will,
        // a crash gone before it is caught. Judged on the gain as it stands,
        // the ceiling would take the first element of every over down a
        // second time while the law caught up.
        let law = self.limited.min(self.held);
        let loudest = coming * wanted.min(self.held);
        let room = if loudest > self.ceiling_level {
            self.ceiling_level / loudest
        } else {
            1.0
        };
        let coefficient = if room < self.ceiling {
            self.ceiling_attack
        } else {
            self.limit_release
        };
        self.ceiling = room + (self.ceiling - room) * coefficient;

        self.gain = law * self.ceiling;
        self.gain
    }

    /// The gain everything is riding at right now.
    pub fn gain(&self) -> f64 {
        self.gain
    }

    /// The gain the hang is holding: what a signal that stood over the knee
    /// has set, without the instants the limiter or the ceiling squash a
    /// crash for. One while nothing has, and with the AGC off.
    pub fn held(&self) -> f64 {
        self.held
    }

    /// Pick up where `previous` left off: the same detector reading, the
    /// same hang, the same gain. A band rebuilt under a station would
    /// otherwise open its gain wide and duck the station afresh — a swell on
    /// every rebuild, and a flutter while a slider moves. From here it rides
    /// to its own knee at its own attack and release. Whether it could: one
    /// switched off, on either side, has nothing to carry.
    pub fn carry_on_from(&mut self, previous: &Agc) -> bool {
        if self.is_off() || previous.is_off() {
            return false;
        }
        self.power = previous.power;
        self.peak = previous.peak;
        self.last_peak = previous.last_peak;
        self.peak_left = previous.peak_left.clamp(1, self.peak_hold);
        self.crest = previous.crest;
        self.last_crest = previous.last_crest;
        self.crest_left = previous.crest_left.clamp(1, self.crest_hold);
        self.above = previous.above;
        self.hang_left = previous.hang_left.min(self.hang);
        self.held = previous.held;
        self.limited = previous.limited;
        self.ceiling = previous.ceiling;
        self.gain = previous.gain;
        true
    }

    /// The static law as a gain, for a detector reading of `level`: nothing
    /// at or below the knee, [`AGC_SLOPE`] decibels down for every decibel
    /// above it, never more than [`AGC_MAX_DUCK_DB`].
    fn law(&self, level: f64) -> f64 {
        let over = level / self.knee;
        if over > 1.0 {
            over.powf(-AGC_SLOPE).max(self.deepest)
        } else {
            1.0
        }
    }
}

/// One step of a one-pole from `from` towards `to`, taken in decibels, with
/// `coefficient` the share of the way left: a gain closing on one 30 dB down
/// gets there as fast in decibels as one closing on 3 dB, where a step in
/// amplitude would take the last twenty of the thirty at a crawl.
fn closer_in_db(from: f64, to: f64, coefficient: f64) -> f64 {
    if from == to {
        return to;
    }
    to * (from / to).powf(coefficient)
}

/// The static law in decibels, written out from its definition rather than
/// taken from [`Agc`]: how far the gain comes down for a level `over_db`
/// above the knee. What both backends are held to.
#[cfg(test)]
pub(super) fn agc_law_db(over_db: f64) -> f64 {
    if over_db > 0.0 {
        -(AGC_SLOPE * over_db).min(AGC_MAX_DUCK_DB)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    use crate::band::BandMixer;
    use crate::band::calibration::{
        NOISE_LEVEL_PER_DB, REFERENCE_NOISE_LEVEL, reference_floor_rms,
    };
    use crate::band::knee::{FLOOR_MEASURE_RATE, band_standing_level, quietest_floor_level};
    use crate::band::noise::NoiseFloor;
    use crate::band::test_support::{knee_for, receiver_for, rms};
    use crate::settings::{FILTER_WIDTH_MAX, FilterShape, TrainingSettings};
    use crate::timing::{StationVoice, plan_morse_playback_for};

    const SR: u32 = 48_000;
    const WPM: f64 = 22.0;

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    fn floor_only() -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.band.noise_level = REFERENCE_NOISE_LEVEL;
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        s.band.qrn_enabled = false;
        s.band.qsb_enabled = false;
        s.clamp()
    }

    /// The same, through the receiver the static and the knee are calibrated
    /// through: the reference filter, a sharp one 500 Hz wide at 3 dB.
    fn floor_through_500_hz() -> TrainingSettings {
        let mut s = floor_only();
        s.band.filter_width_hz = crate::band::filter::reference_width_hz();
        s.band.filter_shape = FilterShape::Sharp;
        s.clamp()
    }

    /// Where a send is keyed down, on the band's clock.
    struct Keyed {
        samples: Vec<f32>,
        downs: Vec<(f64, f64)>,
    }

    /// A station at full level, sending `texts` one after another with
    /// `pause` seconds between them, starting `start` seconds in: rendered as
    /// the native player renders a send, through the receiver's filter.
    fn station(settings: &TrainingSettings, texts: &[&str], start: f64, pause: f64) -> Keyed {
        let voice = StationVoice {
            tone_hz: 600.0,
            char_wpm: WPM,
            effective_wpm: WPM,
            volume: 1.0,
            weight: 1.0,
            dash_ratio: 3.0,
        };
        let sr = f64::from(SR);
        let mut downs = Vec::new();
        let mut plans = Vec::new();
        let mut t = start;
        for text in texts {
            let plan = plan_morse_playback_for(text, settings, &voice);
            for event in &plan.events {
                downs.push((t + event.start_sec, t + event.start_sec + event.keyed_sec));
            }
            plans.push((t, plan.events));
            t += plan.duration_sec + pause;
        }
        let mut samples = vec![0.0f32; (t * sr) as usize + 1];
        for (offset, events) in plans {
            for event in events {
                let first = ((offset + event.start_sec) * sr).round() as usize;
                let len = (event.duration_sec * sr).round() as usize;
                for i in 0..len.min(samples.len().saturating_sub(first)) {
                    let t = i as f64 / sr;
                    let phase = TAU * event.frequency_hz * t;
                    samples[first + i] += (event.envelope_at(t) * phase.sin()) as f32;
                }
            }
        }
        receiver_for(SR, settings).apply(&mut samples);
        Keyed { samples, downs }
    }

    /// Play the band under a send the way the native player does: the AGC
    /// hears the two together, sample by sample. Returns the background and
    /// the gain, sample by sample.
    fn ride(settings: &TrainingSettings, send: &[f32], seconds: f64) -> (Vec<f32>, Vec<f64>) {
        let n = (seconds * f64::from(SR)) as usize;
        let mut mixer = BandMixer::new(SR, settings, 0xA11CE);
        let (mut band, mut gain) = (vec![0.0f32; n], vec![0.0f64; n]);
        for i in 0..n {
            let received = mixer.next(send.get(i).copied().map_or(0.0, f64::from));
            band[i] = received.band as f32;
            gain[i] = received.gain;
        }
        (band, gain)
    }

    fn mask(n: usize, spans: &[(f64, f64)]) -> Vec<bool> {
        let mut out = vec![false; n];
        for (a, b) in spans {
            let (a, b) = ((a * f64::from(SR)) as usize, (b * f64::from(SR)) as usize);
            out[a.min(n)..b.min(n)].fill(true);
        }
        out
    }

    /// Mean gain, in decibels, over the samples `pick` keeps.
    fn mean_db(gain: &[f64], pick: impl Fn(usize) -> bool) -> f64 {
        let (sum, count) = (0..gain.len())
            .filter(|i| pick(*i))
            .fold((0.0, 0usize), |(s, c), i| (s + gain[i], c + 1));
        assert!(count > 0, "nothing to average");
        db(sum / count as f64)
    }

    fn seconds(t: f64) -> usize {
        (t * f64::from(SR)) as usize
    }

    /// The law the two backends share: nothing at the knee, nine decibels
    /// for every ten above it, never more than forty.
    #[test]
    fn the_static_law_is_ten_to_one_above_the_knee_and_stops_at_forty() {
        assert_eq!(agc_law_db(-5.0), 0.0);
        assert_eq!(agc_law_db(0.0), 0.0);
        assert_eq!(agc_law_db(f64::NAN), 0.0);
        assert!((agc_law_db(10.0) + 9.0).abs() < 1e-12);
        assert!((agc_law_db(40.0) + 36.0).abs() < 1e-12);
        assert!((agc_law_db(60.0) + 40.0).abs() < 1e-12);
        // The gain the native AGC settles at under a steady tone is that law,
        // whether the tone is in the band or is the send, and at either decay.
        let knee = 0.01;
        for (over_db, decay) in [
            (3.0, AgcDecay::Fast),
            (10.0, AgcDecay::Slow),
            (20.0, AgcDecay::Fast),
            (30.0, AgcDecay::Slow),
        ] {
            let amplitude = knee * 10f64.powf(over_db / 20.0);
            let (mut heard, mut sent) = (Agc::new(SR, knee, decay), Agc::new(SR, knee, decay));
            for i in 0..SR / 2 {
                let tone = amplitude * (TAU * 600.0 * f64::from(i) / f64::from(SR)).sin();
                heard.process(tone, 0.0);
                sent.process(0.0, tone);
            }
            for gain in [heard.gain(), sent.gain()] {
                assert!(
                    (db(gain) - agc_law_db(over_db)).abs() < 0.3,
                    "{over_db} dB over the knee: {:.2} dB, law {:.2}",
                    db(gain),
                    agc_law_db(over_db)
                );
            }
        }
    }

    /// (1)-(3): under a keyed station the band sits a constant few decibels
    /// down from the first element of the over to the last — no swell as the
    /// receiver gets used to it — the hang carries the gain across the gaps
    /// inside and between characters, and the band is back to idle within a
    /// second of the over.
    #[test]
    fn a_keyed_station_ducks_the_band_steadily_and_lets_it_back() {
        let settings = floor_only();
        let start = 2.0;
        let over = station(
            &settings,
            &["CQ CQ DE DUST DUST DUST K CQ CQ DE DUST K"],
            start,
            0.0,
        );
        // An eight-second over: cut the text there.
        let downs: Vec<(f64, f64)> = over
            .downs
            .iter()
            .copied()
            .filter(|(_, b)| *b <= start + 8.0)
            .collect();
        let end = downs.last().unwrap().1;
        let mut send = over.samples;
        send.truncate(seconds(end + 0.01));
        let length = end + 2.0;
        let (band, gain) = ride(&settings, &send, length);
        let keyed = mask(gain.len(), &downs);
        let keyed = &keyed;
        let at = |a: f64, b: f64| {
            let span = seconds(start + a)..seconds(start + b);
            move |i: usize| keyed[i] && span.contains(&i)
        };
        let early = mean_db(&gain, at(0.0, 0.5));
        let late = mean_db(&gain, at(4.0, 8.0));
        assert!(
            (-15.0..=-9.0).contains(&late),
            "the band should sit 9-15 dB down under the station, was {late:.2} dB"
        );
        assert!(
            (early - late).abs() <= 1.0,
            "the start of the over is ducked {early:.2} dB and the rest {late:.2} dB"
        );

        // The gaps inside words: between elements and between letters.
        let dit = 1.2 / WPM;
        let gaps: Vec<(f64, f64)> = downs
            .windows(2)
            .map(|w| (w[0].1, w[1].0))
            .filter(|(a, b)| b - a < 5.0 * dit)
            .collect();
        assert!(
            gaps.iter().any(|(a, b)| b - a > 2.5 * dit),
            "no letter gaps"
        );
        let in_gap = mask(gain.len(), &gaps);
        let between = mean_db(&gain, |i| in_gap[i]);
        assert!(
            (between - late).abs() <= 1.5,
            "between elements the gain was {between:.2} dB against {late:.2} dB keyed"
        );

        // The same band with the AGC off, to hold the hiss up against.
        let mut off = settings;
        off.band.agc_enabled = false;
        let (open, _) = ride(&off, &[], length);
        let window = |x: &[f32]| rms(&x[seconds(end + 0.8)..seconds(end + 1.2)]);
        let after = db(window(&band) / window(&open));
        assert!(
            after > -1.0,
            "a second after the over the band was still {after:.2} dB down"
        );
    }

    /// (7): sending groups with pauses between them, every group is ducked
    /// the same from its first letter to its last.
    /// After the hang the gain comes back in decibels, as a rig's does: the
    /// last of a deep duck no slower, against the first, than a curve
    /// exponential in decibels takes it, where one stepped in amplitude
    /// snapped the first fifteen decibels back and crept over the rest; and
    /// the band is back within a decibel a second after the station at Fast,
    /// within two at Slow two seconds after.
    #[test]
    fn the_gain_comes_back_in_decibels() {
        for (decay, back_by, within_db) in [(AgcDecay::Fast, 1.0, 1.0), (AgcDecay::Slow, 2.0, 2.0)]
        {
            let knee = 0.01;
            let mut agc = Agc::new(SR, knee, decay);
            let level = knee * db_to_gain(28.0);
            let tone = seconds(0.5);
            for i in 0..tone {
                let _ = agc.next_gain(level * (TAU * 600.0 * i as f64 / f64::from(SR)).sin());
            }
            let gains: Vec<f64> = (0..seconds(2.5)).map(|_| db(agc.next_gain(0.0))).collect();
            let when = |down: f64| {
                gains.iter().position(|g| *g >= down).expect("it came back") as f64 / f64::from(SR)
            };
            assert!(gains[0] < -24.0, "{decay:?}: ducked {:.1} dB", gains[0]);
            let first = when(-15.0) - when(-20.0);
            let last = when(-5.0) - when(-10.0);
            assert!(
                first / last >= 0.35,
                "{decay:?}: 5 dB in {first:.3} s near the bottom, {last:.3} s near the top"
            );
            let after = gains[seconds(back_by)];
            assert!(
                after > -within_db,
                "{decay:?}: {after:.2} dB {back_by} s on"
            );
        }
    }

    #[test]
    fn every_group_is_ducked_the_same_from_start_to_finish() {
        let settings = floor_only();
        let groups = ["KQXVR", "BZ7HJ", "MWPL2", "YOGFC", "TNA9U"];
        let keyed = station(&settings, &groups, 1.0, 2.0);
        let length = keyed.samples.len() as f64 / f64::from(SR);
        let (_, gain) = ride(&settings, &keyed.samples, length);
        let downs = mask(gain.len(), &keyed.downs);
        // A group starts where a key-down follows a long silence.
        let starts: Vec<f64> = keyed
            .downs
            .iter()
            .enumerate()
            .filter(|(k, d)| *k == 0 || d.0 - keyed.downs[k - 1].1 > 1.0)
            .map(|(_, d)| d.0)
            .collect();
        assert_eq!(starts.len(), groups.len());
        for start in starts {
            let downs = &downs;
            let window = |a: f64, b: f64| {
                let span = seconds(start + a)..seconds(start + b);
                move |i: usize| downs[i] && span.contains(&i)
            };
            let first = mean_db(&gain, window(0.0, 0.5));
            let later = mean_db(&gain, window(1.0, 2.0));
            // The detector fills over a group's first letters, a decibel
            // at most: the same to the ear.
            assert!(
                (first - later).abs() <= 1.2,
                "group at {start:.1} s: {first:.2} dB at first, {later:.2} dB later"
            );
            assert!(
                later < -3.0,
                "group at {start:.1} s barely ducked: {later:.2} dB"
            );
        }
    }

    /// (5): a click that never lasted long enough to be a signal is let go
    /// within a few tens of milliseconds, rather than holding the band down.
    ///
    /// The click is calibrated the way the static is, through the 500 Hz
    /// receiver, and that is where it is caught. Through the default peaked
    /// 300 Hz one the same cluster comes out 7.5 dB weaker — a narrow filter
    /// is what takes the edge off a click — and barely reaches the knee; it
    /// has to be let go there just as soon.
    #[test]
    fn a_short_impulse_does_not_hold_the_band_down() {
        for (settings, caught) in [(floor_through_500_hz(), true), (floor_only(), false)] {
            let name = format!(
                "{:?} {:.0} Hz",
                settings.band.filter_shape, settings.band.filter_width_hz
            );
            let knee = knee_for(&settings).unwrap();
            let mut agc = Agc::new(SR, knee, AgcDecay::Fast);
            let mut floor = NoiseFloor::new(SR, reference_floor_rms(), 3);
            let mut receiver = receiver_for(SR, &settings);
            // An impulse whose peak through a 500 Hz filter is the default
            // floor's RMS, the way the static is calibrated; this cluster is
            // five of them twenty decibels louder, across five milliseconds.
            // Spaced unevenly, as crackle is: five evenly a millisecond apart
            // are a comb with a null at every multiple of 200 Hz, the 600 Hz
            // pitch included, and the receiver would rightly hear next to
            // nothing of them.
            let unit = reference_floor_rms() * f64::from(SR) / 1_000.0;
            let ms = |t: f64| seconds(t / 1_000.0);
            let hits = [0.0, 0.8, 1.9, 3.1, 4.6].map(|t| ms(1_000.0 + t));
            let mut gains = Vec::new();
            for i in 0..ms(1_500.0) {
                let kick = if hits.contains(&i) { 10.0 * unit } else { 0.0 };
                let heard = receiver.process(floor.next_sample() + kick);
                gains.push(agc.next_gain(heard));
            }
            assert!(
                gains[..ms(1_000.0)].iter().all(|g| *g == 1.0),
                "{name}: the floor alone moved the gain"
            );
            let deepest = gains[ms(1_000.0)..].iter().copied().fold(1.0, f64::min);
            assert!(
                !caught || db(deepest) < -0.5,
                "{name}: the cluster should have been caught, deepest {:.2} dB",
                db(deepest)
            );
            let after = gains[ms(1_150.0)];
            assert!(
                db(after) >= -1.0,
                "{name}: 150 ms on the band was still {:.2} dB down",
                db(after)
            );
        }
    }

    /// The hang holds the band down for what stays over the knee, not for
    /// the static's crackle, which the limiter squashes and lets go of: an
    /// AGC lamp lit by the hang, as the level scope's is, lights for the odd
    /// long crash the AGC takes for a signal, as a rig's hang AGC does, where
    /// one lit by the gain would flicker with every crackle. Read as the
    /// scope reads it: a twentieth of a second of 10 ms frames, each at its
    /// lowest, held a decibel or more down throughout. A minute of the
    /// default band's light static, and of the hiss alone.
    #[test]
    fn the_static_alone_seldom_holds_the_band_down() {
        // How many times each would light the lamp, the hang and the gain.
        let flashes = |settings: &TrainingSettings| -> (usize, usize) {
            let frame = SR as usize / 100;
            let mut counts = (0, 0);
            for seed in [1, 2, 3] {
                let mut mixer = BandMixer::new(SR, settings, seed);
                let mut lit = (false, false);
                // Twenty seconds of polls.
                for _ in 0..20 * 20 {
                    // The most each let through, of the poll's frames' lowest.
                    let (mut held, mut gain) = (0.0f64, 0.0f64);
                    for _ in 0..5 {
                        let (mut frame_held, mut frame_gain) = (1.0f64, 1.0f64);
                        for _ in 0..frame {
                            let received = mixer.next(0.0);
                            frame_held = frame_held.min(received.held);
                            frame_gain = frame_gain.min(received.gain);
                        }
                        held = held.max(frame_held);
                        gain = gain.max(frame_gain);
                    }
                    let now = (db(held) <= -1.0, db(gain) <= -1.0);
                    counts.0 += usize::from(now.0 && !lit.0);
                    counts.1 += usize::from(now.1 && !lit.1);
                    lit = now;
                }
            }
            counts
        };
        let settings = TrainingSettings::default().clamp();
        assert!(settings.band.qrn_enabled && settings.band.stations_max == 1);
        let (held, gain) = flashes(&settings);
        assert!(
            gain >= 15,
            "the static should crackle: {gain} flickers a minute"
        );
        assert!(
            held <= 6 && held * 4 <= gain,
            "the hang held the band {held} times a minute, the gain dipped {gain}"
        );
        let mut hiss = settings;
        hiss.band.qrn_enabled = false;
        assert_eq!(flashes(&hiss), (0, 0), "the hiss alone moved the AGC");
    }

    /// However loud a crash, nothing past the ceiling reaches the audio
    /// stage: the receiver hears it coming and has the gain down before its
    /// first crest is played. Otherwise the loudest strokes on a wide filter
    /// drive the soft limiter flat.
    ///
    /// Through the default receiver and through the widest filter of every
    /// shape. The stroke is sixty decibels over the floor as the static is
    /// calibrated, which through any of them comes out tens of times the
    /// ceiling.
    #[test]
    fn the_loudest_crash_is_held_under_the_ceiling() {
        let widest = |shape: FilterShape| {
            let mut s = floor_only();
            s.band.filter_width_hz = FILTER_WIDTH_MAX;
            s.band.filter_shape = shape;
            s.clamp()
        };
        for settings in [
            floor_only(),
            widest(FilterShape::Sharp),
            widest(FilterShape::Soft),
            widest(FilterShape::Peaked),
        ] {
            let name = format!(
                "{:?} {:.0} Hz",
                settings.band.filter_shape, settings.band.filter_width_hz
            );
            let knee = knee_for(&settings).unwrap();
            let ceiling = knee * db_to_gain(AGC_CEILING_DB);
            let mut agc = Agc::new(SR, knee, AgcDecay::Fast);
            let mut receiver = receiver_for(SR, &settings);
            // A return stroke sixty decibels over the reference floor: ten
            // impulses across three milliseconds.
            let unit = reference_floor_rms() * f64::from(SR) / 1_000.0;
            let (mut loudest_in, mut loudest_out) = (0.0f64, 0.0f64);
            for i in 0..SR / 10 {
                let kick = if (1_000..1_150).contains(&i) && i % 15 == 0 {
                    1_000.0 * unit
                } else {
                    0.0
                };
                let heard = receiver.process(kick);
                loudest_in = loudest_in.max(heard.abs());
                loudest_out = loudest_out.max(agc.process(heard, 0.0).abs());
            }
            assert!(
                loudest_in > 4.0 * ceiling,
                "{name}: not a loud enough stroke, {loudest_in:.3} against {ceiling:.3}"
            );
            assert!(
                loudest_out <= ceiling * 1.02,
                "{name}: {loudest_out:.3} reached the audio stage, over {ceiling:.3}"
            );
        }

        // The band is played exactly the look-ahead late, and untouched when
        // nothing is happening; switched off, as late, at one.
        let lag = agc_lookahead_samples(SR);
        assert_eq!(lag, seconds(AGC_LOOKAHEAD_SEC));
        for mut agc in [Agc::new(SR, 1.0, AgcDecay::Fast), Agc::off(SR)] {
            let played: Vec<f64> = (0..lag + 2)
                .map(|i| agc.process(if i == 0 { 0.01 } else { 0.0 }, 0.0))
                .collect();
            assert_eq!(played[lag], 0.01);
            assert!(
                played
                    .iter()
                    .enumerate()
                    .all(|(i, s)| i == lag || *s == 0.0)
            );
        }
    }

    /// The gain leads what it is played on by the look-ahead, so the first
    /// element of an over, keyed as hard as the keying goes and well over
    /// the knee, never plays louder than the AGC holds it once it has it.
    #[test]
    fn the_first_element_is_caught_before_it_plays() {
        let knee = 0.01;
        let over_db = 28.0;
        let amplitude = knee * db_to_gain(over_db);
        let mut agc = Agc::new(SR, knee, AgcDecay::Fast);
        // A hundred milliseconds of silence, then a tone switched straight
        // on: harder than any keying.
        let tone = |i: usize| {
            if i < seconds(0.1) {
                0.0
            } else {
                amplitude * (TAU * 600.0 * i as f64 / f64::from(SR)).sin()
            }
        };
        let lag = agc_lookahead_samples(SR);
        let mut sent = vec![0.0; lag];
        let mut loudest = 0.0f64;
        for i in 0..seconds(0.5) {
            let now = tone(i);
            let gain = agc.next_gain(now);
            let due = std::mem::replace(&mut sent[i % lag], now);
            loudest = loudest.max((due * gain).abs());
        }
        let held = amplitude * db_to_gain(agc_law_db(over_db));
        assert!(
            db(loudest / held) < 1.0,
            "the onset came out {:.2} dB over what the AGC holds",
            db(loudest / held)
        );
    }

    /// (4): ten minutes of the default band, crashes and all, and the floor
    /// all but never sags behind them. Now and then a long rumble stands
    /// over the knee long enough to pass for a signal and hold the band down
    /// for a hang, as static pumps a rig's AGC; at ten to one that hang is a
    /// deep one, so it counts, but it is one quarter second in a hundred or
    /// two.
    ///
    /// Read off the gain the receiver rides at, the robust gain of each
    /// quarter second — the median of its 5 ms frames, which a crash's own
    /// duck cannot drag down but a sag behind it does — rather than off the
    /// band's power. The hiss through the default peaked 300 Hz receiver,
    /// 163 Hz of noise bandwidth, swings on its own from one quarter second to
    /// the next: with the AGC switched off altogether 2.2% of them read more
    /// than 1.5 dB under the median, so the power cannot tell a sag from the
    /// hiss.
    #[test]
    fn the_floor_does_not_sag_behind_the_static() {
        for (qrn, length, most) in [(0.25, 600, 0.02), (0.5, 300, 0.03)] {
            let mut settings = floor_only();
            settings.band.qrn_enabled = true;
            settings.band.qrn_level = qrn;
            // The band sounds the same at any rate, and this one keeps ten
            // minutes of it quick.
            let rate = FLOOR_MEASURE_RATE;
            let mut mixer = BandMixer::new(rate, &settings, 0xC0FFEE);
            let frame = rate as usize / 200;
            let gains: Vec<f64> = (0..length * 200)
                .map(|_| {
                    let mut sum = 0.0;
                    for _ in 0..frame {
                        sum += mixer.next(0.0).gain;
                    }
                    sum / frame as f64
                })
                .collect();
            let median = |mut v: Vec<f64>| {
                v.sort_by(f64::total_cmp);
                v[v.len() / 2]
            };
            let windows: Vec<f64> = gains
                .as_chunks::<50>()
                .0
                .iter()
                .map(|w| median(w.to_vec()))
                .collect();
            let sagging = windows.iter().filter(|w| db(**w) < -1.5).count();
            let share = sagging as f64 / windows.len() as f64;
            assert!(
                share < most,
                "QRN {qrn}: {:.1}% of quarter seconds sagged",
                share * 100.0
            );
        }
    }

    /// Off is off: a receiver on its RF gain never moves, whatever arrives.
    #[test]
    fn with_the_agc_off_the_gain_is_one() {
        let mut settings = floor_only();
        settings.band.agc_enabled = false;
        settings.band.qrn_enabled = true;
        settings.band.qrn_level = 1.0;
        settings.band.noise_level = 1.0;
        assert_eq!(knee_for(&settings), None);
        let keyed = station(&settings, &["TEST"], 0.2, 0.0);
        let (_, gain) = ride(&settings, &keyed.samples, 3.0);
        assert!(gain.iter().all(|g| *g == 1.0));
        let mut agc = Agc::off(SR);
        assert!(agc.is_off());
        assert_eq!(agc.next_gain(10.0), 1.0);
    }

    /// A crash has to duck the band — the AGC catching it within a couple of
    /// milliseconds — and then let it straight back up. A crash is a few
    /// milliseconds of ringing; a receiver that held the band down for most of a
    /// second after each one dug holes in the floor that real ones do not.
    #[test]
    fn a_crash_ducks_the_band_and_it_comes_back() {
        // The knee a band at 0.05 would have: its peaks, 2.2 times over.
        let mut agc = Agc::new(SR, 0.05 * AGC_TRIGGER, AgcDecay::Fast);
        let tone =
            |amplitude: f64, i: u32| amplitude * (TAU * 600.0 * f64::from(i) / f64::from(SR)).sin();
        let mut steady = 1.0;
        for i in 0..SR {
            steady = agc.next_gain(tone(0.05, i));
        }
        assert!(
            steady == 1.0,
            "a steady band should be left alone, gain {steady:.2}"
        );

        let mut ducked = 1.0_f64;
        for i in 0..SR / 100 {
            ducked = ducked.min(agc.next_gain(tone(1.0, i)));
        }
        assert!(
            ducked < 0.5,
            "the crash should have pulled the band down, gain {ducked:.2}"
        );

        let mut back = 0.0;
        for i in 0..SR / 10 {
            back = agc.next_gain(tone(0.05, i));
        }
        assert!(
            back > 0.9,
            "a tenth of a second on the band was still down at {back:.2}"
        );
    }

    /// The detector's average dies away after the last sound, and has to reach
    /// zero rather than hang on in the subnormal range, where every multiply
    /// and the square root after it take a microcode assist on every sample
    /// of a silent band.
    #[test]
    fn the_detector_comes_to_rest_in_silence() {
        let mut agc = Agc::new(SR, 0.1, AgcDecay::Fast);
        for i in 0..SR / 10 {
            let phase = TAU * 600.0 * f64::from(i) / f64::from(SR);
            agc.next_gain(0.5 * phase.sin());
        }
        for n in 0..SR * 10 {
            agc.next_gain(0.0);
            assert!(
                !agc.power.is_subnormal(),
                "the detector went subnormal {n} samples into the silence"
            );
        }
        assert_eq!(agc.power, 0.0);
    }

    /// The knee follows a louder floor up, so plain hiss never pumps the
    /// gain, but not a quieter one down past the quietest hiss the noise
    /// control sets: switching the noise off or closing the filter takes
    /// noise out from under the knee and leaves the station as loud as it
    /// was.
    ///
    /// The knee's floor is the quietest floor's, through the 500 Hz filter.
    /// The default receiver, a sharp one 300 Hz wide, lets in 2.5 to 3.5 dB
    /// less noise than that, so even the quietest hiss through it holds the
    /// knee there. The reference floor is 18 dB louder than the quietest and
    /// raises the knee by that less what the narrower receiver takes off;
    /// three tenths of the control more add 11.4 dB. A wide filter lets more
    /// noise in and raises it — a sharp one 2 kHz wide by 5 dB; the peak
    /// filter's resonance holds the noise bandwidth near the reference's
    /// however wide the IF filter behind it goes.
    #[test]
    fn the_knee_rises_with_a_loud_floor_and_holds_for_a_quiet_one() {
        let knee = |edit: fn(&mut TrainingSettings)| {
            let mut s = floor_only();
            edit(&mut s);
            knee_for(&s.clamp()).unwrap()
        };
        let held = AGC_TRIGGER * quietest_floor_level();
        let reference = knee(|_| {});
        let loud = knee(|s| s.band.noise_level = 0.8);
        let wide = knee(|s| {
            s.band.filter_width_hz = FILTER_WIDTH_MAX;
            s.band.filter_shape = FilterShape::Peaked;
        });
        let wide_sharp = knee(|s| {
            s.band.filter_width_hz = FILTER_WIDTH_MAX;
            s.band.filter_shape = FilterShape::Sharp;
        });
        let quietest = knee(|s| s.band.noise_level = NOISE_LEVEL_PER_DB);
        let narrow = knee(|s| {
            s.band.noise_level = NOISE_LEVEL_PER_DB;
            s.band.filter_width_hz = 150.0;
        });
        let silent = knee(|s| s.band.noise_enabled = false);
        // Through the default receiver the floor is under the same floor
        // through the 500 Hz one, by about what the noise bandwidths say.
        let floor = floor_only();
        let receiver = crate::band::FilterDesign::from_settings(&floor);
        let through_500 = quietest_floor_level() * db_to_gain(18.0);
        let under =
            db(through_500 / band_standing_level(FLOOR_MEASURE_RATE, &floor.band, &receiver));
        assert!(
            (2.5..3.5).contains(&under),
            "the default floor is {under:.2} dB under"
        );
        // Below the quietest floor the knee holds at the quietest floor's.
        for other in [quietest, narrow, silent] {
            assert_eq!(other, held);
        }
        // The reference floor raises it 18 dB, less what the receiver takes
        // off; three tenths of the control more, 11.4 dB more.
        let rise = db(reference / held) + under;
        assert!(
            (17.0..19.0).contains(&rise),
            "the reference floor raised the knee {:.1} dB",
            db(reference / held)
        );
        let louder = db(loud / reference);
        assert!(
            (10.0..13.0).contains(&louder),
            "a louder floor raised the knee {louder:.1} dB"
        );
        // Against the same floor through the 500 Hz filter.
        let reference_500 = AGC_TRIGGER * through_500;
        assert!(
            db(wide_sharp / reference_500) > 4.0,
            "a wide sharp filter left the knee put"
        );
        // The peak keeps most of the wide filter's hiss out: about 2 dB
        // more than the 500 Hz filter lets in, where the sharp one lets in 5.
        assert!(
            wide >= reference_500 && db(wide / reference_500) < 2.5,
            "wide and peaked: {:.2} dB",
            db(wide / reference_500)
        );
        // And that floor, through the AGC, is left alone.
        let mut s = floor_only();
        s.band.noise_level = 0.8;
        let (_, gain) = ride(&s.clamp(), &[], 10.0);
        assert!(gain.iter().all(|g| *g > 0.97), "the hiss moved the gain");
    }
}
