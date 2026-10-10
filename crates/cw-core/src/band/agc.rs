//! The receiver's automatic gain control.
//!
//! One static law, shared by both backends: below a fixed knee the gain is
//! left alone, and above it the gain comes down half a decibel for every
//! decibel of excess, never more than [`AGC_MAX_DUCK_DB`]. The native player
//! runs that law sample by sample in [`Agc`], with a hang; the browser can only
//! run it through a `DynamicsCompressorNode`, which [`compressor_curve_db`]
//! describes so the two can be held to the same curve.
//!
//! What it is tuned to is what operators hear and what MorseRunner — the
//! benchmark contesters train on — does with its AGC on:
//!
//! - Under a keyed station the band's hiss sits a steady 3 to 5 dB down, from
//!   the first element of the over to the last. No swell at the start, no
//!   pumping between elements: the hang carries the gain across the gaps.
//! - In a long pause the hiss is audibly back.
//! - A crash pulls the gain for a moment and lets go; it does not dig a hole
//!   in the band for the next second.
//!
//! An earlier version learned the band's level over a second and a half, and
//! learned the station with it: every over started 8 dB ducked and swelled up
//! as the receiver got used to it, and every crash left the floor sagging for
//! most of a second. Real AGC-off recordings show neither, and no AGC with a
//! fixed reference can produce the first.

/// How far above the floor's own peaks the knee sits: a level has to stand
/// 2.2 times (about 7 dB) above the highest the hiss ordinarily reaches before
/// the gain moves. The floor alone never gets there, so on an empty band the
/// receiver leaves the noise exactly as the filter shaped it.
pub const AGC_TRIGGER: f64 = 2.2;
/// Decibels of gain reduction per decibel above the knee: a 2:1 compressor.
///
/// Under the default station — 21 dB over the floor in 500 Hz, about 10 dB
/// over the knee at its peaks — this ducks the band 5 dB, and 3.5 dB at the
/// bottom of the volume range: MorseRunner's 3.4 to 4.3 dB and the "noise
/// drops under a strong station" operators describe. A real receiver run at
/// full RF gain can duck deeper; this is the knob that would do it.
pub const AGC_SLOPE: f64 = 0.5;
/// The most the gain ever comes down, in decibels. Past this a crash is
/// silencing the band rather than riding over it.
pub const AGC_MAX_DUCK_DB: f64 = 12.0;
/// Seconds for the gain to close on a level it has to duck to. Fast, because
/// a receiver has to catch a crash before it is deafening.
pub const AGC_ATTACK_SEC: f64 = 0.002;
/// How long the gain holds after the last sustained signal: long enough to
/// carry it across the gaps between elements and characters (165 ms at
/// 22 WPM), short enough that a word gap or a pause lets the band back.
/// Operators prefer hang AGC on CW for exactly this: a per-element pumping
/// AGC is the one everyone dislikes.
pub const AGC_HANG_SEC: f64 = 0.22;
/// Once the hang has run out, the time constant the gain recovers with:
/// between the FAST and MID settings of a typical transceiver (0.1 and 0.5 s;
/// SLOW is 1.2 s), so a pause of a second has the band fully back.
pub const AGC_RELEASE_SEC: f64 = 0.3;
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
/// The most the receiver lets through to its audio stage, as an amplitude:
/// about where the soft limiter after it is 0.997 of full scale. A loud enough
/// crash on a wide-open filter would otherwise drive the limiter flat.
pub const AGC_CEILING: f64 = 1.5;
/// How far ahead of what it plays the receiver listens, so the ceiling is in
/// place by the time a stroke's first crest arrives rather than a couple of
/// milliseconds after it. A DSP receiver's AGC delays its audio the same way;
/// MorseRunner's looks 28 ms ahead.
pub const AGC_LOOKAHEAD_SEC: f64 = 0.002;
/// How fast the ceiling closes: a fifth of the look-ahead, so it has finished
/// before the crest it saw coming is played.
const AGC_CEILING_ATTACK_SEC: f64 = AGC_LOOKAHEAD_SEC / 5.0;

/// The knee width of the browser's compressor, in decibels. Web Audio's knee
/// starts at the threshold and is fully on six decibels later, so the
/// threshold sits half of this below the native knee to centre the soft knee
/// on the hard one.
pub const AGC_COMPRESSOR_KNEE_DB: f64 = 6.0;
/// The compressor ratio that is [`AGC_SLOPE`]: one decibel out for every two
/// in above the knee.
pub const AGC_COMPRESSOR_RATIO: f64 = 1.0 / (1.0 - AGC_SLOPE);

/// The static law, in decibels: how far the gain comes down for a level
/// `over_db` above the knee. Zero at or below the knee.
pub fn agc_law_db(over_db: f64) -> f64 {
    if over_db > 0.0 {
        -(AGC_SLOPE * over_db).min(AGC_MAX_DUCK_DB)
    } else {
        0.0
    }
}

/// Where the browser's compressor threshold goes for a native `knee` (an
/// amplitude): half the compressor's soft knee below it, so the compressor's
/// curve straddles the native corner rather than starting at it.
pub fn agc_compressor_threshold_db(knee: f64) -> f64 {
    20.0 * knee.max(1e-12).log10() - AGC_COMPRESSOR_KNEE_DB / 2.0
}

/// How the browser's compressor is set for an AGC with its knee at `knee`
/// (an amplitude), or for none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompressorSetting {
    pub threshold_db: f64,
    pub knee_db: f64,
    pub ratio: f64,
    /// The make-up gain the node will add of its own accord, which whatever
    /// follows it has to take back off.
    pub makeup_db: f64,
}

/// The browser's compressor for the native AGC's knee. With the AGC off it
/// is parked at full scale with a ratio of one, where it is transparent and
/// adds no make-up gain.
pub fn agc_compressor(knee: Option<f64>) -> CompressorSetting {
    let (threshold_db, ratio) = match knee {
        Some(knee) => (agc_compressor_threshold_db(knee), AGC_COMPRESSOR_RATIO),
        None => (0.0, 1.0),
    };
    CompressorSetting {
        threshold_db,
        knee_db: AGC_COMPRESSOR_KNEE_DB,
        ratio,
        makeup_db: compressor_makeup_db(threshold_db, AGC_COMPRESSOR_KNEE_DB, ratio),
    }
}

/// The static curve of a Web Audio `DynamicsCompressorNode`, as the
/// specification's reference implementation (Chromium's, which Firefox and
/// WebKit share) computes it: the gain in decibels it applies to a steady
/// input at `input_db`, leaving out its automatic make-up gain (see
/// [`compressor_makeup_db`]).
///
/// Linear below the threshold; between the threshold and `threshold + knee` an
/// exponential knee in the amplitude domain whose decibel slope falls from one
/// to `1 / ratio`; a straight line of slope `1 / ratio` above. The browser
/// cannot run the native AGC, so this is what holds the two to one law.
pub fn compressor_curve_db(input_db: f64, threshold_db: f64, knee_db: f64, ratio: f64) -> f64 {
    CompressorCurve::new(threshold_db, knee_db, ratio).gain_db(input_db)
}

/// The make-up gain a `DynamicsCompressorNode` adds on its own, in decibels:
/// the reduction its curve applies at full scale, to the power 0.6. Every
/// browser applies it, so a compressor that should only ever turn things
/// down has to be followed by its inverse.
pub fn compressor_makeup_db(threshold_db: f64, knee_db: f64, ratio: f64) -> f64 {
    -0.6 * CompressorCurve::new(threshold_db, knee_db, ratio).gain_db(0.0)
}

/// The arithmetic behind [`compressor_curve_db`].
struct CompressorCurve {
    threshold: f64,
    knee_end_db: f64,
    knee_end_out_db: f64,
    slope: f64,
    k: f64,
}

impl CompressorCurve {
    fn new(threshold_db: f64, knee_db: f64, ratio: f64) -> Self {
        let threshold = db_to_amplitude(threshold_db);
        let slope = 1.0 / ratio.max(1.0);
        let knee_end_db = threshold_db + knee_db.max(0.0);
        let knee_end = db_to_amplitude(knee_end_db);
        // The knee's sharpness, found the way the reference implementation
        // finds it: fifteen geometric bisections for the k that makes the
        // knee's decibel slope at its far end equal the ratio's.
        let (mut low, mut high, mut k) = (0.1, 10_000.0, 5.0_f64);
        for _ in 0..15 {
            if knee_slope(threshold, knee_end, k) < slope {
                high = k;
            } else {
                low = k;
            }
            k = (low * high).sqrt();
        }
        Self {
            threshold,
            knee_end_db,
            knee_end_out_db: amplitude_to_db(knee_curve(threshold, knee_end, k)),
            slope,
            k,
        }
    }

    fn gain_db(&self, input_db: f64) -> f64 {
        let output_db = if input_db < self.knee_end_db {
            amplitude_to_db(knee_curve(
                self.threshold,
                db_to_amplitude(input_db),
                self.k,
            ))
        } else {
            self.knee_end_out_db + self.slope * (input_db - self.knee_end_db)
        };
        output_db - input_db
    }
}

fn knee_curve(threshold: f64, x: f64, k: f64) -> f64 {
    if x < threshold {
        x
    } else {
        threshold + (1.0 - (-k * (x - threshold)).exp()) / k
    }
}

fn knee_slope(threshold: f64, x: f64, k: f64) -> f64 {
    if x < threshold {
        return 1.0;
    }
    let x2 = x * 1.001;
    let rise = amplitude_to_db(knee_curve(threshold, x2, k))
        - amplitude_to_db(knee_curve(threshold, x, k));
    rise / (amplitude_to_db(x2) - amplitude_to_db(x))
}

fn db_to_amplitude(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

fn amplitude_to_db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-30).log10()
}

/// The receiver's automatic gain control.
///
/// More than a limiter, and not a level follower either. It compares everything
/// reaching the receiver — the band and the station being copied — against a
/// fixed knee set from the band's own floor, and turns the gain down by the
/// static law when it is exceeded. Two paths decide how long that lasts:
///
/// - **Hang.** A level that stays above the knee for [`AGC_QUALIFY_SEC`] is a
///   signal, not a click. It sets the gain, and the gain then holds for
///   [`AGC_HANG_SEC`] after the signal last stood there before recovering with
///   [`AGC_RELEASE_SEC`]. Under a keyed station that hold is unbroken from
///   the first element to the last, so the band sits a steady few decibels
///   down for the whole over and comes back in the pauses.
/// - **Limiter.** Anything above the knee pulls the gain down within
///   [`AGC_ATTACK_SEC`], but if it never qualified for the hang it lets go
///   again within [`AGC_LIMIT_RELEASE_SEC`]. A lightning stroke is squashed
///   for the few milliseconds it lasts, and the crackle of the crash around it
///   carries on at full level.
///
/// Under both, a ceiling: whatever the law says, nothing louder than
/// [`AGC_CEILING`] reaches the audio stage. The receiver plays the band
/// [`AGC_LOOKAHEAD_SEC`] late ([`Agc::process`]) so the ceiling is already
/// down when the crest it saw coming is played.
///
/// The detector is the power of whatever is loudest — a sample of the band,
/// or the send's level — smoothed over a few milliseconds and read as the
/// amplitude of a steady tone of the same power. A station keyed at amplitude
/// `a` reads `a` whichever way it arrives. Whether it has *stayed* above the
/// knee is judged on the peaks of the last few milliseconds as well, which
/// fall away as soon as a sound has gone however loud it was: the smoothed
/// level takes longer to forget a loud stroke than a stroke lasts, and would
/// otherwise let the loudest ones pass for signals.
pub struct Agc {
    /// The knee, as the amplitude of a steady tone. Infinite when switched
    /// off.
    knee: f64,
    /// The detector: smoothed power, in squared tone amplitude.
    power: f64,
    smoothing: f64,
    /// The loudest peak of this stretch of [`AGC_PEAK_HOLD_SEC`] and of the
    /// one before it, and how long this one has left.
    peak: f64,
    last_peak: f64,
    peak_left: u32,
    peak_hold: u32,
    /// Samples the detector has stood above the knee without a break.
    above: u32,
    qualify: u32,
    hang: u32,
    hang_left: u32,
    /// The gain the hang is holding.
    held: f64,
    /// The gain the limiter wants.
    limited: f64,
    /// The gain that keeps the loudest recent crest under the ceiling.
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
    /// An AGC with its knee at `knee`, the amplitude of a steady tone.
    pub fn new(sample_rate: u32, knee: f64) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let coefficient = |seconds: f64| (-1.0 / (seconds * sr)).exp();
        let samples = |seconds: f64| (seconds * sr).round().max(1.0) as u32;
        Self {
            knee: if knee > 0.0 { knee } else { f64::INFINITY },
            power: 0.0,
            smoothing: coefficient(AGC_DETECTOR_SEC),
            peak: 0.0,
            last_peak: 0.0,
            peak_left: samples(AGC_PEAK_HOLD_SEC),
            peak_hold: samples(AGC_PEAK_HOLD_SEC),
            above: 0,
            qualify: samples(AGC_QUALIFY_SEC),
            hang: samples(AGC_HANG_SEC),
            hang_left: 0,
            held: 1.0,
            limited: 1.0,
            ceiling: 1.0,
            gain: 1.0,
            deepest: db_to_amplitude(-AGC_MAX_DUCK_DB),
            attack: coefficient(AGC_ATTACK_SEC),
            release: coefficient(AGC_RELEASE_SEC),
            limit_release: coefficient(AGC_LIMIT_RELEASE_SEC),
            ceiling_attack: coefficient(AGC_CEILING_ATTACK_SEC),
            // Allocated here, once: the audio callback only ever writes into it.
            delay: vec![0.0; samples(AGC_LOOKAHEAD_SEC) as usize],
            delay_at: 0,
        }
    }

    /// A receiver run on its RF gain: the gain is one, whatever arrives.
    pub fn off() -> Self {
        Self::new(1, f64::INFINITY)
    }

    pub fn is_off(&self) -> bool {
        !self.knee.is_finite()
    }

    /// Feed one sample of the band as the receiver hears it, and the peak of
    /// the send playing over it; get back the band to play, at the gain the
    /// receiver is riding at and [`AGC_LOOKAHEAD_SEC`] late. Switched off, the
    /// band goes straight through.
    ///
    /// Only the ceiling gets the benefit of the look-ahead. The law's gain
    /// waits in the delay with the sample it was worked out on, so a crash's
    /// first crest still gets through before the AGC catches it — the crack a
    /// receiver's AGC is too slow for, and most of what makes static sound
    /// like static.
    pub fn process(&mut self, heard: f64, send_peak: f64) -> f64 {
        if self.is_off() {
            return heard;
        }
        self.next_gain(heard, send_peak);
        let ridden = heard * self.limited.min(self.held);
        let due = std::mem::replace(&mut self.delay[self.delay_at], ridden);
        self.delay_at = (self.delay_at + 1) % self.delay.len();
        due * self.ceiling
    }

    /// Feed one sample of the band as the receiver hears it, and the peak of
    /// the send playing over it; get back the gain both should ride at.
    pub fn next_gain(&mut self, heard: f64, send_peak: f64) -> f64 {
        if self.is_off() {
            return 1.0;
        }
        // A sample of a tone of amplitude a carries a²/2 of power on average,
        // so the band's samples count double; the send arrives as its peak.
        let input = (2.0 * heard * heard).max(send_peak * send_peak);
        let input = if input.is_finite() { input } else { 0.0 };
        self.power = input + (self.power - input) * self.smoothing;
        let level = self.power.sqrt();
        let wanted = self.law(level);

        // Down fast, and up again fast — on its own this is a limiter.
        let coefficient = if wanted < self.limited {
            self.attack
        } else {
            self.limit_release
        };
        self.limited = wanted + (self.limited - wanted) * coefficient;

        // The peaks of the last few milliseconds, for judging whether
        // anything is still there.
        self.peak = self.peak.max(heard.abs()).max(send_peak);
        self.peak_left -= 1;
        let recent = self.peak.max(self.last_peak);
        if self.peak_left == 0 {
            self.last_peak = self.peak;
            self.peak = 0.0;
            self.peak_left = self.peak_hold;
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
            let coefficient = if wanted < self.held {
                self.attack
            } else {
                self.release
            };
            self.held = wanted + (self.held - wanted) * coefficient;
        } else if self.hang_left > 0 {
            self.hang_left -= 1;
        } else {
            self.held = 1.0 + (self.held - 1.0) * self.release;
        }

        // The ceiling. The crests of the last few milliseconds include
        // everything still waiting in the look-ahead, so a gain that keeps
        // them under the ceiling keeps what is about to be played there too.
        let room = if recent > AGC_CEILING {
            AGC_CEILING / recent
        } else {
            1.0
        };
        let coefficient = if room < self.ceiling {
            self.ceiling_attack
        } else {
            self.limit_release
        };
        self.ceiling = room + (self.ceiling - room) * coefficient;

        self.gain = self.limited.min(self.held).min(self.ceiling);
        self.gain
    }

    /// The gain everything is riding at right now.
    pub fn gain(&self) -> f64 {
        self.gain
    }

    /// The knee, as the amplitude of a steady tone; `None` when off.
    pub fn knee(&self) -> Option<f64> {
        (!self.is_off()).then_some(self.knee)
    }

    /// [`agc_law_db`] as a gain, for a detector reading of `level`.
    fn law(&self, level: f64) -> f64 {
        let over = level / self.knee;
        if over > 1.0 {
            over.powf(-AGC_SLOPE).max(self.deepest)
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::band::{BandMixer, NoiseFloor, ReceiverFilter, agc_knee, noise_floor_rms};
    use crate::settings::TrainingSettings;
    use crate::timing::{StationVoice, plan_morse_playback_for};

    const SR: u32 = 48_000;
    /// The native player hands the send's level to the band a callback at a
    /// time; this is a typical callback.
    const BUFFER: usize = 512;
    const WPM: f64 = 22.0;

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    /// The default band with nothing on it but the floor, at a fixed pitch.
    fn floor_only() -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        s.band.qrn_enabled = false;
        s.band.qsb_enabled = false;
        s.band.activity_enabled = false;
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
                downs.push((
                    t + event.start_sec,
                    t + event.start_sec + event.duration_sec,
                ));
            }
            plans.push((t, plan.events));
            t += plan.duration_sec + pause;
        }
        let mut samples = vec![0.0f32; (t * sr) as usize + 1];
        for (offset, events) in plans {
            for event in events {
                let first = ((offset + event.start_sec) * sr).round() as usize;
                let len = (event.duration_sec * sr).round() as usize;
                let last = event.envelope.len() - 1;
                for i in 0..len.min(samples.len().saturating_sub(first)) {
                    let pos = i as f64 / len as f64 * last as f64;
                    let k = (pos as usize).min(last - 1);
                    let frac = pos - k as f64;
                    let env = f64::from(event.envelope[k]) * (1.0 - frac)
                        + f64::from(event.envelope[k + 1]) * frac;
                    let phase = std::f64::consts::TAU * event.frequency_hz * i as f64 / sr;
                    samples[first + i] += (env * phase.sin()) as f32;
                }
            }
        }
        ReceiverFilter::from_settings(SR, settings).apply(&mut samples);
        Keyed { samples, downs }
    }

    /// Play the band under a send the way the native player's callback does:
    /// the send's loudest sample of each buffer reaches the AGC for the next
    /// one. Returns the background and the gain, sample by sample.
    fn ride(settings: &TrainingSettings, send: &[f32], seconds: f64) -> (Vec<f32>, Vec<f64>) {
        let n = (seconds * f64::from(SR)) as usize;
        let mut mixer = BandMixer::new(SR, settings, 0xA11CE);
        let (mut band, mut gain) = (vec![0.0f32; n], vec![0.0f64; n]);
        let mut loudest = 0.0f32;
        for start in (0..n).step_by(BUFFER) {
            let end = (start + BUFFER).min(n);
            mixer.note_send_level(f64::from(loudest));
            for i in start..end {
                band[i] = mixer.next_background();
                gain[i] = mixer.agc_gain();
            }
            loudest = send
                .get(start.min(send.len())..end.min(send.len()))
                .unwrap_or(&[])
                .iter()
                .fold(0.0, |a, s| a.max(s.abs()));
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

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
    }

    fn seconds(t: f64) -> usize {
        (t * f64::from(SR)) as usize
    }

    /// The law the two backends share: nothing at the knee, half a decibel per
    /// decibel above it, never more than twelve.
    #[test]
    fn the_static_law_is_two_to_one_above_the_knee_and_stops_at_twelve() {
        assert_eq!(agc_law_db(-5.0), 0.0);
        assert_eq!(agc_law_db(0.0), 0.0);
        assert_eq!(agc_law_db(f64::NAN), 0.0);
        assert!((agc_law_db(10.0) + 5.0).abs() < 1e-12);
        assert!((agc_law_db(24.0) + 12.0).abs() < 1e-12);
        assert!((agc_law_db(60.0) + 12.0).abs() < 1e-12);
        // The gain the native AGC settles at under a steady tone is that law,
        // whether the tone is in the band or is the send.
        let knee = 0.1;
        for over_db in [3.0, 10.0, 20.0, 30.0] {
            let amplitude = knee * 10f64.powf(over_db / 20.0);
            let (mut heard, mut sent) = (Agc::new(SR, knee), Agc::new(SR, knee));
            let (mut in_band, mut as_send) = (1.0, 1.0);
            for i in 0..SR / 2 {
                let phase = std::f64::consts::TAU * 600.0 * f64::from(i) / f64::from(SR);
                in_band = heard.next_gain(amplitude * phase.sin(), 0.0);
                as_send = sent.next_gain(0.0, amplitude);
            }
            for gain in [in_band, as_send] {
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
            (-8.0..=-3.0).contains(&late),
            "the band should sit 3-8 dB down under the station, was {late:.2} dB"
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
    /// the same from its first letter to its last. The old receiver started
    /// each group a few decibels deeper and let go halfway through.
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
            assert!(
                (first - later).abs() <= 1.0,
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
    #[test]
    fn a_short_impulse_does_not_hold_the_band_down() {
        let settings = floor_only();
        let knee = agc_knee(&settings).unwrap();
        let mut agc = Agc::new(SR, knee);
        let mut floor = NoiseFloor::new(SR, noise_floor_rms(0.5), 3);
        let mut receiver = ReceiverFilter::from_settings(SR, &settings);
        // An impulse whose peak through a 500 Hz filter is the default floor's
        // RMS, the way the static is calibrated; this cluster is five of them
        // twenty decibels louder, across five milliseconds.
        let unit = noise_floor_rms(0.5) * f64::from(SR) / 1_000.0;
        let ms = |t: f64| seconds(t / 1_000.0);
        let mut gains = Vec::new();
        for i in 0..ms(1_500.0) {
            let t = i.wrapping_sub(ms(1_000.0));
            let kick = if t < ms(5.0) && t % ms(1.0) == 0 {
                10.0 * unit
            } else {
                0.0
            };
            let heard = receiver.process(floor.next_sample() + kick);
            gains.push(agc.next_gain(heard, 0.0));
        }
        assert!(
            gains[..ms(1_000.0)].iter().all(|g| *g == 1.0),
            "the floor alone moved the gain"
        );
        let deepest = gains[ms(1_000.0)..].iter().copied().fold(1.0, f64::min);
        assert!(
            db(deepest) < -0.5,
            "the cluster should have been caught, deepest {:.2} dB",
            db(deepest)
        );
        let after = gains[ms(1_150.0)];
        assert!(
            db(after) >= -1.0,
            "150 ms on the band was still {:.2} dB down",
            db(after)
        );
    }

    /// However loud a crash, nothing past the ceiling reaches the audio
    /// stage: the receiver hears it coming and has the gain down before its
    /// first crest is played. Otherwise the loudest strokes on a wide filter
    /// drive the soft limiter flat.
    #[test]
    fn the_loudest_crash_is_held_under_the_ceiling() {
        let settings = floor_only();
        let mut agc = Agc::new(SR, agc_knee(&settings).unwrap());
        let mut receiver = ReceiverFilter::from_settings(SR, &settings);
        // A return stroke sixty decibels over the default floor: ten
        // impulses across three milliseconds.
        let unit = noise_floor_rms(0.5) * f64::from(SR) / 1_000.0;
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
        assert!(loudest_in > 10.0 * AGC_CEILING, "not a loud enough stroke");
        assert!(
            loudest_out <= AGC_CEILING * 1.02,
            "{loudest_out:.2} reached the audio stage"
        );

        // The band is played exactly the look-ahead late, and untouched when
        // nothing is happening.
        let mut agc = Agc::new(SR, 1.0);
        let lag = seconds(AGC_LOOKAHEAD_SEC);
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
        // Switched off, straight through.
        assert_eq!(Agc::off().process(5.0, 0.0), 5.0);
    }

    /// (4): ten minutes of the default band, crashes and all, and the floor
    /// never sags behind them. The old receiver left 5% of the band's quarter
    /// seconds more than 1.5 dB under its own median, 12% at QRN 0.5.
    #[test]
    fn the_floor_does_not_sag_behind_the_static() {
        for (qrn, length, most) in [(0.25, 600, 0.01), (0.5, 300, 0.03)] {
            let mut settings = floor_only();
            settings.band.qrn_enabled = true;
            settings.band.qrn_level = qrn;
            // The band sounds the same at any rate, and this one keeps ten
            // minutes of it quick.
            let rate = crate::band::FLOOR_MEASURE_RATE;
            let mut mixer = BandMixer::new(rate, &settings, 0xC0FFEE);
            let mut frame = vec![0.0f32; rate as usize / 200];
            let powers: Vec<f64> = (0..length * 200)
                .map(|_| {
                    mixer.fill_background(&mut frame);
                    frame.iter().map(|s| f64::from(*s).powi(2)).sum()
                })
                .collect();
            let median = |mut v: Vec<f64>| {
                v.sort_by(f64::total_cmp);
                v[v.len() / 2]
            };
            // The robust floor of each quarter second: the median of its 5 ms
            // frames, which a crash cannot drag up but a duck pulls down.
            let windows: Vec<f64> = powers
                .as_chunks::<50>()
                .0
                .iter()
                .map(|w| median(w.to_vec()))
                .collect();
            let middle = median(windows.clone());
            let sagging = windows
                .iter()
                .filter(|w| 10.0 * (**w / middle).log10() < -1.5)
                .count();
            let share = sagging as f64 / windows.len() as f64;
            assert!(
                share < most,
                "QRN {qrn}: {:.1}% of quarter seconds sagged",
                share * 100.0
            );
        }
    }

    /// (6): the browser's compressor and the native AGC are one law. A
    /// compressor's knee is soft and it has no hang, but its static curve has
    /// to land on the native one.
    #[test]
    fn the_browsers_compressor_follows_the_native_law() {
        for knee in [0.01, 0.0914, 0.3] {
            let threshold = agc_compressor_threshold_db(knee);
            let knee_db = 20.0 * f64::log10(knee);
            for over in [6.0, 12.0, 20.0] {
                let browser = compressor_curve_db(
                    knee_db + over,
                    threshold,
                    AGC_COMPRESSOR_KNEE_DB,
                    AGC_COMPRESSOR_RATIO,
                );
                let native = agc_law_db(over);
                assert!(
                    (browser - native).abs() <= 1.5,
                    "knee {knee}, {over} dB over: browser {browser:.2} dB, native {native:.2} dB"
                );
            }
            // Well below the knee the compressor leaves the band alone.
            let quiet = compressor_curve_db(
                knee_db - 10.0,
                threshold,
                AGC_COMPRESSOR_KNEE_DB,
                AGC_COMPRESSOR_RATIO,
            );
            assert!(quiet.abs() < 1e-9);
        }
        // Its own make-up gain is what it takes off at full scale, to the 0.6.
        let makeup = compressor_makeup_db(-20.0, 6.0, 2.0);
        let full = compressor_curve_db(0.0, -20.0, 6.0, 2.0);
        assert!((makeup + 0.6 * full).abs() < 1e-9 && makeup > 0.0);
        // What the browser is given: the knee's curve, and its own make-up
        // gain to take back off.
        let on = agc_compressor(Some(0.0914));
        assert_eq!(on.threshold_db, agc_compressor_threshold_db(0.0914));
        assert_eq!(on.ratio, AGC_COMPRESSOR_RATIO);
        assert!(on.makeup_db > 0.0);
        // With the AGC off it is parked where it does nothing at all: every
        // level the receiver produces is below its threshold, and it adds no
        // make-up gain.
        let off = agc_compressor(None);
        for input in [-80.0, -20.0, -3.0, 0.0] {
            let gain = compressor_curve_db(input, off.threshold_db, off.knee_db, off.ratio);
            assert!(gain.abs() < 1e-9, "{input} dB came out {gain} dB off");
        }
        assert!(off.makeup_db.abs() < 1e-9);
    }

    /// Off is off: a receiver on its RF gain never moves, whatever arrives.
    #[test]
    fn with_the_agc_off_the_gain_is_one() {
        let mut settings = floor_only();
        settings.band.agc_enabled = false;
        settings.band.qrn_enabled = true;
        settings.band.qrn_level = 1.0;
        settings.band.noise_level = 1.0;
        assert_eq!(agc_knee(&settings), None);
        let keyed = station(&settings, &["TEST"], 0.2, 0.0);
        let (_, gain) = ride(&settings, &keyed.samples, 3.0);
        assert!(gain.iter().all(|g| *g == 1.0));
        let mut agc = Agc::off();
        assert!(agc.is_off() && agc.knee().is_none());
        assert_eq!(agc.next_gain(10.0, 10.0), 1.0);
    }

    /// The knee follows a louder floor up, so plain hiss never pumps the
    /// gain, but not a quieter one down: closing the filter or quieting the
    /// band takes noise out from under the knee and leaves the station as
    /// loud as it was.
    #[test]
    fn the_knee_rises_with_a_loud_floor_and_holds_for_a_quiet_one() {
        let knee = |edit: fn(&mut TrainingSettings)| {
            let mut s = floor_only();
            edit(&mut s);
            agc_knee(&s.clamp()).unwrap()
        };
        let reference = knee(|_| {});
        let loud = knee(|s| s.band.noise_level = 0.8);
        let wide = knee(|s| s.band.filter_bandwidth_hz = 2_000.0);
        let narrow = knee(|s| s.band.filter_bandwidth_hz = 150.0);
        let quiet = knee(|s| s.band.noise_level = 0.1);
        let silent = knee(|s| s.band.noise_enabled = false);
        // Three tenths of the control is 11.4 dB more floor.
        let rise = db(loud / reference);
        assert!(
            (10.0..13.0).contains(&rise),
            "a louder floor raised the knee {rise:.1} dB"
        );
        assert!(
            db(wide / reference) > 4.0,
            "a wide filter left the knee put"
        );
        for other in [narrow, quiet, silent] {
            assert_eq!(other, reference);
        }
        // And that floor, through the AGC, is left alone.
        let mut s = floor_only();
        s.band.noise_level = 0.8;
        let (_, gain) = ride(&s.clamp(), &[], 10.0);
        assert!(gain.iter().all(|g| *g > 0.97), "the hiss moved the gain");
    }
}
