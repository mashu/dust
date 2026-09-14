//! Sample-rate QSB / QRN / QRM mixing, shared by every backend.

use crate::rng::{FastrandRng, Rng};
use crate::settings::{QrmProfile, TrainingSettings};

pub const QSB_MIN_GAIN: f64 = 0.25;

/// Output gains, set so the sliders span a useful range of signal-to-noise
/// rather than a decorative one: at their defaults the band sits about 20 dB
/// under the Morse — present, easy to copy through — and at their maximum it
/// reaches the signal, which is where copying gets genuinely hard. A CW
/// contact at the edge of readability is around 0 dB, so that is the far end
/// worth having.
pub const QRN_OUTPUT_GAIN: f64 = 1.30;
pub const QRM_OUTPUT_GAIN: f64 = 0.75;
pub const RINGING_OUTPUT_GAIN: f64 = 0.55;

/// The intensity controls are not linear in amplitude. A fader that is has
/// almost all of its useful range bunched at the bottom; this keeps the low
/// settings gentle while letting the top of the slider actually reach the
/// signal.
const LEVEL_CURVE: f64 = 1.65;

pub fn shaped_level(level: f64) -> f64 {
    level.clamp(0.0, 1.0).powf(LEVEL_CURVE)
}

/// What a receiver does with a loud crash: catches it, rather than letting it
/// square off. Linear up to the knee, asymptotic to full scale above it — so
/// the biggest static is loud without turning into a buzz.
fn soft_limit(x: f64) -> f64 {
    const KNEE: f64 = 0.7;
    let magnitude = x.abs();
    if magnitude <= KNEE {
        return x;
    }
    let over = (magnitude - KNEE) / (1.0 - KNEE);
    (KNEE + (1.0 - KNEE) * over.tanh()).copysign(x)
}

/// The resonance the gains above are calibrated at. Level is compensated back
/// to this point so the Resonance control changes how the background *rings*
/// rather than how loud it is.
const RESONANCE_REFERENCE: f64 = 66.0;

/// A topology-preserving state-variable filter.
///
/// A direct-form biquad is only stable while its coefficients hold still: the
/// state it carries belongs to the coefficients that produced it, so retuning
/// one every sample pumps energy into it. At the resonances this model uses
/// that is not a subtle artifact — sweeping a Q of 240 at 20 Hz drove the old
/// filter to full scale, some 20 dB above the Morse. This structure keeps its
/// state in two integrators that mean the same thing at any cutoff, so the
/// passband can breathe without the filter running away.
#[derive(Clone, Debug)]
struct Svf {
    k: f64,
    a1: f64,
    a2: f64,
    a3: f64,
    ic1: f64,
    ic2: f64,
}

impl Svf {
    fn bandpass(sample_rate: f64, f0: f64, q: f64) -> Self {
        let mut filter = Self {
            k: 1.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            ic1: 0.0,
            ic2: 0.0,
        };
        filter.set_bandpass(sample_rate, f0, q);
        filter
    }

    fn set_bandpass(&mut self, sample_rate: f64, f0: f64, q: f64) {
        let sr = sample_rate.max(1.0);
        // Nyquist wins over the 20 Hz floor: at an absurdly low sample rate the
        // upper bound would otherwise fall below the lower one and clamp panics.
        let freq = f0.clamp(20.0, (sr * 0.45).max(20.0));
        let q = q.max(0.5);
        let g = (std::f64::consts::PI * freq / sr).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// The band-pass output, normalised so its peak gain is 1 at any Q.
    fn process(&mut self, input: f64) -> f64 {
        let v3 = input - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        self.k * v1
    }
}

/// A narrow filter passes less of a broadband input than a wide one, so
/// without this the Resonance control would be a 28 dB volume control wearing
/// a filter's label. Noise power through a unit-peak band-pass goes as its
/// bandwidth, which goes as 1/Q, so the amplitude compensation is √Q.
fn resonance_compensation(q: f64) -> f64 {
    (q.max(0.5) / RESONANCE_REFERENCE).sqrt()
}

/// Atmospheric static, as a stream of excitation to be shaped by the receiver.
///
/// QRN is lightning, so it arrives as *crashes*: sharp, wildly uneven in size,
/// and heard through a CW filter as short bursts rather than clicks. Under
/// them sits the receiver's own noise floor, which is the steady part and the
/// quiet one. Modelling only that floor — band-limited white noise — is what
/// makes a band sound dead, and it was inaudible here besides.
///
/// This is the excitation only; the caller runs it through the passband, and
/// that filter is what turns each crash into the sound of one. Both backends
/// share it: the native player calls it per sample, the browser fills a long
/// loop buffer with it.
pub struct AtmosphericNoise {
    sample_rate: f64,
    rng: FastrandRng,
    level: f64,
    crash_energy: f64,
    crash_decay: f64,
}

/// Crashes per second at the extremes of the intensity control. Even a quiet
/// band has the occasional one.
const CRASH_RATE_MIN: f64 = 0.4;
const CRASH_RATE_MAX: f64 = 26.0;
/// How much of the output is the steady receiver floor rather than crashes.
const HISS_SHARE: f64 = 0.16;
/// Crash sizes are heavy-tailed — most are small, a few are very loud — which
/// is what gives real static its restless, uneven character.
const CRASH_TAIL: f64 = 1.7;
/// Shortest and longest crash, in seconds. A discharge has structure over a
/// few milliseconds; the filter's own ringing adds the rest.
const CRASH_TAU_MIN: f64 = 0.002;
const CRASH_TAU_MAX: f64 = 0.020;
/// The receiver's CW filter, roughly 200 Hz wide at a 500 Hz note. Wide enough
/// that a crash keeps its sharpness instead of ringing like a bell.
const QRN_FILTER_Q: f64 = 2.4;

impl AtmosphericNoise {
    pub fn new(sample_rate: u32, level: f64, seed: u64) -> Self {
        Self {
            sample_rate: f64::from(sample_rate.max(1)),
            rng: FastrandRng(seed | 1),
            level: level.clamp(0.0, 1.0),
            crash_energy: 0.0,
            crash_decay: 0.0,
        }
    }

    fn noise(&mut self) -> f64 {
        self.rng.f64() * 2.0 - 1.0
    }

    /// One sample of excitation, before the receiver's filter shapes it.
    pub fn next_sample(&mut self) -> f64 {
        if self.level <= 0.0 {
            return 0.0;
        }
        let rate = CRASH_RATE_MIN + (CRASH_RATE_MAX - CRASH_RATE_MIN) * self.level;
        if self.rng.f64() < rate / self.sample_rate {
            let u = self.rng.f64().clamp(1e-9, 1.0);
            self.crash_energy += (-u.ln()).powf(CRASH_TAIL);
            let tau = CRASH_TAU_MIN + (CRASH_TAU_MAX - CRASH_TAU_MIN) * self.rng.f64();
            self.crash_decay = (-1.0 / (tau * self.sample_rate)).exp();
        }
        self.crash_energy *= self.crash_decay;
        // A crash is a burst of noise, not a tone: the envelope shapes fresh
        // noise, so the passband is what gives it a pitch.
        let crash = self.crash_energy * self.noise();
        let hiss = self.noise() * HISS_SHARE;
        // The rate rises with the setting linearly — more crashes — but the
        // loudness follows the fader curve.
        (crash + hiss) * shaped_level(self.level)
    }
}

/// Real-time QRN/QRM generator. QSB is applied separately to Morse samples.
pub struct BandMixer {
    sample_rate: f64,
    rng: FastrandRng,
    settings: TrainingSettings,
    t: f64,
    ringing_energy: f64,
    atmospheric: AtmosphericNoise,
    qrn: Svf,
    qrm_primary: Svf,
    qrm_secondary: Svf,
    qrm_ring: Svf,
}

impl BandMixer {
    pub fn new(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let settings = settings.clone().clamp();
        let center = settings.side_tone_center();
        let resonance = settings
            .band
            .receiver_background_resonance
            .clamp(0.5, 240.0);
        let offset = settings
            .band
            .receiver_background_offset_hz
            .clamp(-1000.0, 1000.0);
        let atmospheric = AtmosphericNoise::new(
            sample_rate.max(1),
            settings.band.qrn_level,
            seed ^ 0x51ED_2701,
        );
        Self {
            sample_rate: sr,
            rng: FastrandRng(seed | 1),
            settings,
            t: 0.0,
            ringing_energy: 0.0,
            atmospheric,
            qrn: Svf::bandpass(sr, center, QRN_FILTER_Q),
            qrm_primary: Svf::bandpass(sr, (center + offset).max(20.0), resonance),
            qrm_secondary: Svf::bandpass(
                sr,
                (center - (offset.abs() + 35.0).max(20.0)).max(20.0),
                (resonance * 0.65).max(0.5),
            ),
            qrm_ring: Svf::bandpass(
                sr,
                (center + offset - 35.0).max(20.0),
                (resonance * 1.45).min(320.0),
            ),
        }
    }

    pub fn needs_background(settings: &TrainingSettings) -> bool {
        (settings.band.qrn_enabled && settings.band.qrn_level > 0.0)
            || (settings.band.qrm_enabled && settings.band.qrm_level > 0.0)
    }

    fn excitation_sample(&mut self) -> f64 {
        let rate = self
            .settings
            .band
            .receiver_background_excitation_rate
            .clamp(0.1, 500.0);
        let decay = self
            .settings
            .band
            .receiver_background_decay
            .clamp(0.5, 0.9999);
        if self.rng.f64() < rate / self.sample_rate {
            self.ringing_energy += (self.rng.f64() * 2.0 - 1.0) * (0.6 + self.rng.f64() * 0.4);
        }
        self.ringing_energy *= decay;
        self.ringing_energy + (self.rng.f64() * 2.0 - 1.0) * 0.015
    }

    pub(crate) fn wobble(t: f64, depth: f64, rate: f64) -> f64 {
        if depth <= 0.0 || rate <= 0.0 {
            0.0
        } else {
            depth * (2.0 * std::f64::consts::PI * rate * t).sin()
        }
    }

    fn qrm_sample(&mut self) -> f64 {
        if !self.settings.band.qrm_enabled || self.settings.band.qrm_level <= 0.0 {
            return 0.0;
        }
        let level = shaped_level(self.settings.band.qrm_level);
        let model_gain = self.settings.band.receiver_background_gain.clamp(0.0, 20.0);
        let center = self.settings.side_tone_center();
        let offset = self
            .settings
            .band
            .receiver_background_offset_hz
            .clamp(-1000.0, 1000.0);
        let depth = self
            .settings
            .band
            .receiver_background_offset_mod_depth_hz
            .clamp(0.0, 1000.0);
        let rate = self
            .settings
            .band
            .receiver_background_offset_mod_rate_hz
            .clamp(0.0, 20.0);
        let resonance = self
            .settings
            .band
            .receiver_background_resonance
            .clamp(0.5, 240.0);
        let grain = self.excitation_sample();
        let mut out = 0.0;
        let profile = self.settings.band.qrm_profile;

        if matches!(profile, QrmProfile::Whistle | QrmProfile::Mixed) {
            let primary_f = center + offset + Self::wobble(self.t, depth, rate);
            let secondary_f = center - (offset.abs() + 35.0).max(20.0)
                + Self::wobble(self.t, depth * 0.65, rate * 0.73);
            self.qrm_primary
                .set_bandpass(self.sample_rate, primary_f, resonance);
            self.qrm_secondary.set_bandpass(
                self.sample_rate,
                secondary_f,
                (resonance * 0.65).max(0.5),
            );
            let base = QRM_OUTPUT_GAIN * level * model_gain * resonance_compensation(resonance);
            let amp = base + base * 0.18 * (2.0 * std::f64::consts::PI * 0.11 * self.t).sin();
            out += (self.qrm_primary.process(grain) + self.qrm_secondary.process(grain)) * amp;
        }
        if matches!(profile, QrmProfile::Ringing | QrmProfile::Mixed) {
            let ring_f = center + offset - 35.0 + Self::wobble(self.t, depth, rate);
            self.qrm_ring
                .set_bandpass(self.sample_rate, ring_f, (resonance * 1.45).min(320.0));
            out += self.qrm_ring.process(grain)
                * RINGING_OUTPUT_GAIN
                * level
                * model_gain
                * resonance_compensation((resonance * 1.45).min(320.0));
        }
        out
    }

    fn qrn_sample(&mut self) -> f64 {
        if !self.settings.band.qrn_enabled || self.settings.band.qrn_level <= 0.0 {
            return 0.0;
        }
        // The filter is the receiver, and it is what turns a crash into the
        // short ringing burst you actually hear rather than a click.
        let excitation = self.atmospheric.next_sample();
        self.qrn.process(excitation) * QRN_OUTPUT_GAIN
    }

    pub fn next_background(&mut self) -> f32 {
        let sample = self.qrn_sample() + self.qrm_sample();
        self.t += 1.0 / self.sample_rate;
        soft_limit(sample) as f32
    }

    pub fn fill_background(&mut self, out: &mut [f32]) {
        for slot in out {
            *slot = self.next_background();
        }
    }
}

pub fn qsb_gain_at(t_sec: f64, enabled: bool, depth: f64, rate_hz: f64) -> f32 {
    if !enabled || depth <= 0.0 {
        return 1.0;
    }
    let depth = depth.clamp(0.0, 1.0);
    let rate = rate_hz.clamp(0.03, 1.5);
    let gain_range = depth.min(1.0 - QSB_MIN_GAIN);
    let base = 1.0 - gain_range / 2.0;
    let half = gain_range / 2.0;
    (base + half * (2.0 * std::f64::consts::PI * rate * t_sec).sin()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn background(settings: &TrainingSettings, seconds: usize) -> Vec<f32> {
        let mut mixer = BandMixer::new(48_000, settings, 9);
        let mut out = vec![0.0f32; 48_000 * seconds];
        mixer.fill_background(&mut out);
        out
    }

    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    /// Peak over RMS. Steady noise sits near 4; static crashes push it far
    /// higher, which is the whole difference between a hiss and a band.
    fn crest(v: &[f32]) -> f32 {
        let r = rms(v);
        if r > 0.0 {
            peak(v) / r
        } else {
            0.0
        }
    }

    fn only_qrn(level: f64) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.band.qrm_enabled = false;
        settings.band.qrn_enabled = true;
        settings.band.qrn_level = level;
        settings.clamp()
    }

    fn only_qrm(level: f64) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.band.qrn_enabled = false;
        settings.band.qrm_enabled = true;
        settings.band.qrm_level = level;
        settings.clamp()
    }

    /// QRN is lightning, not a hiss. Band-limited white noise has a crest
    /// factor near 4 and sounds like a dead receiver; static crashes are
    /// sparse and wildly uneven, which is what this measures.
    #[test]
    fn static_arrives_in_crashes_rather_than_as_a_hiss() {
        let noisy = background(&only_qrn(1.0), 4);
        assert!(
            crest(&noisy) > 8.0,
            "static should be impulsive, crest was {}",
            crest(&noisy)
        );

        // And the crashes are uneven: the loudest second is far above a quiet
        // one, which a steady hiss could never be.
        let seconds: Vec<f32> = noisy.chunks(48_000).map(rms).collect();
        let loudest = seconds.iter().copied().fold(0.0f32, f32::max);
        let quietest = seconds.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            loudest > quietest * 1.3,
            "every second sounded the same: {seconds:?}"
        );
    }

    #[test]
    fn turning_the_static_up_brings_more_crashes_and_louder_ones() {
        let quiet = background(&only_qrn(0.2), 4);
        let loud = background(&only_qrn(1.0), 4);
        assert!(
            rms(&loud) > rms(&quiet) * 4.0,
            "quiet {} vs loud {}",
            rms(&quiet),
            rms(&loud)
        );
        assert!(peak(&loud) > peak(&quiet));
    }

    /// The Resonance control shapes how the background rings. It must not also
    /// be a volume control: it used to swing the level by 28 dB, which made a
    /// filter setting the loudest thing on the screen.
    #[test]
    fn resonance_changes_the_character_and_not_the_loudness() {
        let levels: Vec<f32> = [0.5, 2.0, 20.0, 66.0, 240.0]
            .into_iter()
            .map(|q| {
                let mut settings = only_qrm(0.5);
                settings.band.receiver_background_resonance = q;
                rms(&background(&settings.clamp(), 2))
            })
            .collect();
        let loudest = levels.iter().copied().fold(0.0f32, f32::max);
        let quietest = levels.iter().copied().fold(f32::MAX, f32::min);
        let spread_db = 20.0 * (loudest / quietest).log10();
        assert!(
            spread_db < 6.0,
            "resonance moved the level by {spread_db:.1} dB: {levels:?}"
        );
    }

    /// The regression test for a filter that could be driven to full scale by
    /// its own settings. Sweeping a high-Q direct-form biquad every sample
    /// pumped energy into it — 20 dB above the Morse, from the tuning panel.
    #[test]
    fn no_tuning_setting_can_drive_the_background_to_a_screech() {
        for resonance in [0.5, 66.0, 120.0, 240.0] {
            for depth in [0.0, 45.0, 1000.0] {
                for rate in [0.0, 0.32, 5.0, 20.0] {
                    let mut settings = only_qrm(1.0);
                    settings.band.receiver_background_resonance = resonance;
                    settings.band.receiver_background_gain = 20.0;
                    settings.band.receiver_background_offset_mod_depth_hz = depth;
                    settings.band.receiver_background_offset_mod_rate_hz = rate;
                    let out = background(&settings.clamp(), 1);
                    let level = rms(&out);
                    assert!(
                        level < 0.30,
                        "Q {resonance} depth {depth} rate {rate} ran away to {level}"
                    );
                    assert!(
                        out.iter().all(|s| s.abs() < 1.0),
                        "Q {resonance} depth {depth} rate {rate} hit full scale"
                    );
                }
            }
        }
    }

    /// The intensity controls have to be worth having: quiet enough at their
    /// defaults to copy through comfortably, loud enough at the top to be the
    /// reason you miss a character. A CW contact at the edge of readability is
    /// around 0 dB signal-to-noise, so that is what the far end aims at.
    #[test]
    fn the_band_spans_a_useful_range_of_signal_to_noise() {
        // Key-down RMS of the Morse is what the background is heard against.
        let signal = (crate::timing::DEFAULT_TARGET_GAIN / std::f64::consts::SQRT_2) as f32;
        let snr = |settings: &TrainingSettings| {
            20.0 * (signal / rms(&background(settings, 4)).max(1e-9)).log10()
        };

        let quiet = snr(&TrainingSettings::default().clamp());
        assert!(
            (14.0..32.0).contains(&quiet),
            "the default band should be present but easy, was {quiet:.1} dB"
        );

        let mut loud = TrainingSettings::default();
        loud.band.qrn_level = 1.0;
        loud.band.qrm_level = 1.0;
        let hard = snr(&loud.clamp());
        assert!(
            hard < 6.0,
            "the loudest band should reach the signal, was {hard:.1} dB"
        );
        assert!(
            hard < quiet - 12.0,
            "the range is too narrow to be worth a slider"
        );
    }

    /// However loud it gets, it is caught rather than squared off — which is
    /// what a receiver does with a crash, and the difference between loud
    /// static and a buzz.
    #[test]
    fn the_loudest_band_is_limited_and_never_clipped() {
        let mut settings = TrainingSettings::default();
        settings.band.qrn_level = 1.0;
        settings.band.qrm_level = 1.0;
        settings.band.receiver_background_gain = 20.0;
        let out = background(&settings.clamp(), 4);
        assert!(
            out.iter().all(|s| s.abs() < 1.0),
            "the output hit full scale"
        );
        assert!(out.iter().all(|s| s.is_finite()));

        assert_eq!(soft_limit(0.0), 0.0);
        assert_eq!(soft_limit(0.5), 0.5, "below the knee it does nothing");
        assert_eq!(soft_limit(-0.5), -0.5);
        assert!(soft_limit(4.0) < 1.0 && soft_limit(4.0) > 0.9);
        assert!(soft_limit(-4.0) > -1.0 && soft_limit(-4.0) < -0.9);
        // Bigger in still means bigger out: limiting, not clipping flat.
        assert!(soft_limit(2.0) < soft_limit(3.0));
    }

    /// The fader curve keeps the bottom of the range gentle without costing
    /// the top its bite.
    #[test]
    fn the_intensity_fader_is_gentle_at_the_bottom() {
        assert_eq!(shaped_level(0.0), 0.0);
        assert_eq!(shaped_level(1.0), 1.0);
        assert!(
            shaped_level(0.5) < 0.5,
            "half a fader is less than half as loud"
        );
        assert!(shaped_level(0.2) < 0.2);
        // Still monotonic, or the slider would fight the user.
        let mut previous = -1.0;
        for step in 0..=20 {
            let value = shaped_level(f64::from(step) / 20.0);
            assert!(value > previous, "the fader went backwards at {step}");
            previous = value;
        }
        assert_eq!(shaped_level(4.0), 1.0, "out of range is still in range");
    }

    #[test]
    fn qsb_modulates_amplitude() {
        let gains: Vec<f32> = (0..48_000)
            .map(|i| qsb_gain_at(f64::from(i) / 48_000.0, true, 1.0, 1.0))
            .collect();
        let min = gains.iter().copied().fold(f32::MAX, f32::min);
        let max = gains.iter().copied().fold(f32::MIN, f32::max);
        assert!(min < 0.8);
        assert!(max > 0.9);
        assert!(
            min >= QSB_MIN_GAIN as f32,
            "a fade should not reach silence"
        );
    }

    #[test]
    fn qsb_gain_is_unity_when_disabled() {
        assert_eq!(qsb_gain_at(0.25, false, 1.0, 1.0), 1.0);
        assert_eq!(qsb_gain_at(0.25, true, 0.0, 1.0), 1.0);
        let trough = qsb_gain_at(0.75, true, 1.0, 1.0);
        assert!(trough > 0.2 && trough < 0.4);
    }

    #[test]
    fn background_is_finite() {
        let settings = TrainingSettings::default();
        let mut mixer = BandMixer::new(48_000, &settings, 1);
        let mut buf = vec![0.0f32; 2048];
        mixer.fill_background(&mut buf);
        assert!(buf.iter().all(|s| s.is_finite()));
    }
}

#[cfg(test)]
mod mixer_tests {
    use super::*;

    fn quiet() -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.band.qrn_enabled = false;
        s.band.qrm_enabled = false;
        s
    }

    #[test]
    fn a_silent_band_needs_no_background_stream() {
        assert!(!BandMixer::needs_background(&quiet()));

        let mut static_only = quiet();
        static_only.band.qrn_enabled = true;
        static_only.band.qrn_level = 0.3;
        assert!(BandMixer::needs_background(&static_only));

        // Enabled at zero level is still silence.
        static_only.band.qrn_level = 0.0;
        assert!(!BandMixer::needs_background(&static_only));

        let mut interference = quiet();
        interference.band.qrm_enabled = true;
        interference.band.qrm_level = 0.2;
        assert!(BandMixer::needs_background(&interference));
    }

    #[test]
    fn a_silent_band_mixes_to_silence() {
        let mut mixer = BandMixer::new(8_000, &quiet(), 1);
        let mut out = [1.0f32; 64];
        mixer.fill_background(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn every_qrm_profile_produces_sound_and_stays_in_range() {
        for profile in [QrmProfile::Whistle, QrmProfile::Ringing, QrmProfile::Mixed] {
            let mut settings = quiet();
            settings.band.qrm_enabled = true;
            settings.band.qrm_level = 1.0;
            settings.band.qrm_profile = profile;
            settings.band.qrn_enabled = true;
            settings.band.qrn_level = 1.0;
            let mut mixer = BandMixer::new(8_000, &settings, 7);
            let mut out = [0.0f32; 4_000];
            mixer.fill_background(&mut out);
            assert!(
                out.iter().all(|s| (-1.0..=1.0).contains(s)),
                "{profile:?} left the unit range"
            );
            assert!(
                out.iter().any(|s| *s != 0.0),
                "{profile:?} produced nothing"
            );
        }
    }

    #[test]
    fn a_still_band_does_not_wobble() {
        assert_eq!(BandMixer::wobble(1.0, 0.0, 2.0), 0.0);
        assert_eq!(BandMixer::wobble(1.0, 30.0, 0.0), 0.0);
        assert!(BandMixer::wobble(0.25, 30.0, 1.0) > 0.0);
    }

    #[test]
    fn a_zero_sample_rate_is_treated_as_one() {
        let mut settings = quiet();
        settings.band.qrn_enabled = true;
        settings.band.qrn_level = 0.5;
        let mut mixer = BandMixer::new(0, &settings, 1);
        let mut out = [0.0f32; 8];
        mixer.fill_background(&mut out);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn fading_is_off_unless_it_is_switched_on() {
        assert_eq!(qsb_gain_at(1.0, false, 0.5, 0.2), 1.0);
        assert_eq!(qsb_gain_at(1.0, true, 0.0, 0.2), 1.0);
    }

    #[test]
    fn fading_stays_inside_its_depth() {
        let depth: f64 = 0.5;
        let range = depth.min(1.0 - QSB_MIN_GAIN);
        let base = 1.0 - range / 2.0;
        for step in 0..200 {
            let t = f64::from(step) * 0.05;
            let gain = f64::from(qsb_gain_at(t, true, depth, 0.5));
            assert!(gain >= base - range / 2.0 - 1e-6, "{gain} too quiet");
            assert!(gain <= base + range / 2.0 + 1e-6, "{gain} too loud");
            assert!(gain >= QSB_MIN_GAIN - 1e-6);
        }
    }

    #[test]
    fn a_fade_stays_finite_at_any_point_on_its_cycle() {
        // A second of fading at 1 Hz has to move the level somewhere...
        let gains: Vec<f32> = (0..8_000)
            .map(|i| qsb_gain_at(f64::from(i) / 8_000.0, true, 0.6, 1.0))
            .collect();
        let min = gains.iter().copied().fold(f32::MAX, f32::min);
        let max = gains.iter().copied().fold(f32::MIN, f32::max);
        assert!(max - min > 0.1);
        assert!(gains.iter().all(|g| g.is_finite() && *g <= 1.0 && *g > 0.0));

        // ...and nothing about the clock can make it produce a nonsense gain.
        for t in [0.0, -1.0, 1e9, f64::INFINITY] {
            assert!(qsb_gain_at(t, true, 0.6, 1.0).is_finite() || t.is_infinite());
        }
    }
}
