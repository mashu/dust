//! Sample-rate QSB / QRN / QRM mixing for native playback.

use crate::rng::{FastrandRng, Rng};
use crate::settings::{QrmProfile, TrainingSettings};

pub const QSB_MIN_GAIN: f64 = 0.25;
pub const QRN_OUTPUT_GAIN: f64 = 0.022;
pub const QRM_OUTPUT_GAIN: f64 = 0.11;
pub const RINGING_OUTPUT_GAIN: f64 = 0.08;

#[derive(Clone, Debug)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
    f0: f64,
    q: f64,
}

impl Biquad {
    fn bandpass(sample_rate: f64, f0: f64, q: f64) -> Self {
        let mut filter = Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
            f0,
            q,
        };
        filter.set_bandpass(sample_rate, f0, q);
        filter
    }

    fn set_bandpass(&mut self, sample_rate: f64, f0: f64, q: f64) {
        let sr = sample_rate.max(1.0);
        let freq = f0.clamp(20.0, sr * 0.45);
        let q = q.max(0.5);
        let omega = 2.0 * std::f64::consts::PI * freq / sr;
        let sin = omega.sin();
        let cos = omega.cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.b1 = 0.0;
        self.b2 = -alpha / a0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
        self.f0 = freq;
        self.q = q;
    }

    fn process(&mut self, input: f64) -> f64 {
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        output
    }
}

/// Real-time QRN/QRM generator. QSB is applied separately to Morse samples.
pub struct BandMixer {
    sample_rate: f64,
    rng: FastrandRng,
    settings: TrainingSettings,
    t: f64,
    ringing_energy: f64,
    qrn: Biquad,
    qrm_primary: Biquad,
    qrm_secondary: Biquad,
    qrm_ring: Biquad,
}

impl BandMixer {
    pub fn new(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let settings = settings.clone().clamp();
        let center = settings.side_tone_center();
        let resonance = settings.receiver_background_resonance.clamp(0.5, 240.0);
        let offset = settings.receiver_background_offset_hz.clamp(-1000.0, 1000.0);
        Self {
            sample_rate: sr,
            rng: FastrandRng(seed | 1),
            settings,
            t: 0.0,
            ringing_energy: 0.0,
            qrn: Biquad::bandpass(sr, center, 2.4),
            qrm_primary: Biquad::bandpass(sr, (center + offset).max(20.0), resonance),
            qrm_secondary: Biquad::bandpass(
                sr,
                (center - (offset.abs() + 35.0).max(20.0)).max(20.0),
                (resonance * 0.65).max(0.5),
            ),
            qrm_ring: Biquad::bandpass(
                sr,
                (center + offset - 35.0).max(20.0),
                (resonance * 1.45).min(320.0),
            ),
        }
    }

    pub fn needs_background(settings: &TrainingSettings) -> bool {
        (settings.qrn_enabled && settings.qrn_level > 0.0)
            || (settings.qrm_enabled && settings.qrm_level > 0.0)
    }

    fn excitation_sample(&mut self) -> f64 {
        let rate = self.settings.receiver_background_excitation_rate.clamp(0.1, 500.0);
        let decay = self.settings.receiver_background_decay.clamp(0.5, 0.9999);
        if self.rng.f64() < rate / self.sample_rate {
            self.ringing_energy +=
                (self.rng.f64() * 2.0 - 1.0) * (0.6 + self.rng.f64() * 0.4);
        }
        self.ringing_energy *= decay;
        self.ringing_energy + (self.rng.f64() * 2.0 - 1.0) * 0.015
    }

    fn wobble(t: f64, depth: f64, rate: f64) -> f64 {
        if depth <= 0.0 || rate <= 0.0 {
            0.0
        } else {
            depth * (2.0 * std::f64::consts::PI * rate * t).sin()
        }
    }

    fn qrm_sample(&mut self) -> f64 {
        if !self.settings.qrm_enabled || self.settings.qrm_level <= 0.0 {
            return 0.0;
        }
        let level = self.settings.qrm_level.clamp(0.0, 1.0);
        let model_gain = self.settings.receiver_background_gain.clamp(0.0, 20.0);
        let center = self.settings.side_tone_center();
        let offset = self.settings.receiver_background_offset_hz.clamp(-1000.0, 1000.0);
        let depth = self.settings.receiver_background_offset_mod_depth_hz.clamp(0.0, 1000.0);
        let rate = self.settings.receiver_background_offset_mod_rate_hz.clamp(0.0, 20.0);
        let resonance = self.settings.receiver_background_resonance.clamp(0.5, 240.0);
        let grain = self.excitation_sample();
        let mut out = 0.0;
        let profile = self.settings.qrm_profile;

        if matches!(profile, QrmProfile::Whistle | QrmProfile::Mixed) {
            let primary_f = center + offset + Self::wobble(self.t, depth, rate);
            let secondary_f = center
                - (offset.abs() + 35.0).max(20.0)
                + Self::wobble(self.t, depth * 0.65, rate * 0.73);
            self.qrm_primary
                .set_bandpass(self.sample_rate, primary_f, resonance);
            self.qrm_secondary.set_bandpass(
                self.sample_rate,
                secondary_f,
                (resonance * 0.65).max(0.5),
            );
            let base = QRM_OUTPUT_GAIN * level * model_gain;
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
                * model_gain;
        }
        out
    }

    fn qrn_sample(&mut self) -> f64 {
        if !self.settings.qrn_enabled || self.settings.qrn_level <= 0.0 {
            return 0.0;
        }
        let noise = self.rng.f64() * 2.0 - 1.0;
        self.qrn.process(noise) * QRN_OUTPUT_GAIN * self.settings.qrn_level.clamp(0.0, 1.0)
    }

    pub fn next_background(&mut self) -> f32 {
        let sample = self.qrn_sample() + self.qrm_sample();
        self.t += 1.0 / self.sample_rate;
        sample.clamp(-1.0, 1.0) as f32
    }

    pub fn fill_background(&mut self, out: &mut [f32]) {
        for slot in out {
            *slot = self.next_background();
        }
    }
}

pub fn apply_qsb(samples: &mut [f32], sample_rate: u32, settings: &TrainingSettings) {
    if !settings.qsb_enabled || settings.qsb_depth <= 0.0 || samples.is_empty() {
        return;
    }
    let sr = f64::from(sample_rate.max(1));
    let depth = settings.qsb_depth.clamp(0.0, 1.0);
    let rate = settings.qsb_rate_hz.clamp(0.03, 1.5);
    let gain_range = depth.min(1.0 - QSB_MIN_GAIN);
    let base = 1.0 - gain_range / 2.0;
    let half = gain_range / 2.0;
    for (i, sample) in samples.iter_mut().enumerate() {
        let t = i as f64 / sr;
        let gain = base + half * (2.0 * std::f64::consts::PI * rate * t).sin();
        *sample *= gain as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qsb_modulates_amplitude() {
        let mut settings = TrainingSettings::default();
        settings.qsb_enabled = true;
        settings.qsb_depth = 1.0;
        settings.qsb_rate_hz = 1.0;
        let mut samples = vec![1.0f32; 48_000];
        apply_qsb(&mut samples, 48_000, &settings);
        let min = samples.iter().copied().fold(f32::MAX, f32::min);
        let max = samples.iter().copied().fold(f32::MIN, f32::max);
        assert!(min < 0.8);
        assert!(max > 0.9);
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
