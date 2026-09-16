//! Sample-rate QSB, QRN and receiver-character mixing, shared by every backend.
//!
//! The names here mean what an operator means by them. QRN is atmospheric
//! static. The receiver model is the set's own hiss and ringing — it used to
//! be called QRM in this file, which is wrong: QRM is another station on top of
//! yours, and that lives with the stations rather than in the noise.

use crate::rng::{FastrandRng, Rng};
use crate::settings::{
    ReceiverProfile, TrainingSettings, FILTER_BANDWIDTH_MAX, FILTER_BANDWIDTH_MIN,
};

pub const QSB_MIN_GAIN: f64 = 0.25;

/// Output gains, set so the sliders span a useful range of signal-to-noise
/// rather than a decorative one: at their defaults the band sits about 20 dB
/// under the Morse — present, easy to copy through — and at their maximum it
/// reaches the signal, which is where copying gets genuinely hard. A CW
/// contact at the edge of readability is around 0 dB, so that is the far end
/// worth having.
pub const QRN_OUTPUT_GAIN: f64 = 1.30;
pub const RECEIVER_OUTPUT_GAIN: f64 = 0.75;
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
        // A wide-open receiver really is this broad: a 2 kHz passband around a
        // 500 Hz note needs a Q well under one, and the structure is happy there.
        let q = q.max(0.1);
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

/// How many band-pass sections make up the receiver's filter.
///
/// One is not enough. A real CW filter is many poles deep, which is what gives
/// it both steep skirts and the ringing everyone knows — a lone biquad at the
/// same bandwidth rings about a third as long and lets far more static past
/// the edges.
pub const RECEIVER_STAGES: usize = 3;

/// The receiver's own filter: the one thing every sound you hear has passed
/// through.
///
/// Narrowing it does all three things at once, because they are the same
/// thing: less static gets through (the noise it passes goes as its
/// bandwidth), the signal loses a little as its keying sidebands run into the
/// skirts, and the filter rings for longer. That is why this is one control
/// and not three.
pub struct ReceiverFilter {
    stages: Vec<Svf>,
}

/// The Q each of the [`RECEIVER_STAGES`] sections needs to land the cascade on
/// this bandwidth. Shared so the browser, which builds the same filter out of
/// Web Audio nodes, gets the same passband as the native player.
pub fn receiver_stage_q(center_hz: f64, bandwidth_hz: f64) -> f64 {
    let center = center_hz.max(20.0);
    let bandwidth = bandwidth_hz.clamp(FILTER_BANDWIDTH_MIN, FILTER_BANDWIDTH_MAX);
    (center / (bandwidth * STAGE_WIDENING)).max(0.1)
}

impl ReceiverFilter {
    pub fn new(sample_rate: u32, center_hz: f64, bandwidth_hz: f64) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let center = center_hz.max(20.0);
        // Each section is wider than the filter as a whole: put three in a row
        // and the passband they share is narrower than any one of them.
        let q = receiver_stage_q(center, bandwidth_hz);
        Self {
            stages: (0..RECEIVER_STAGES)
                .map(|_| Svf::bandpass(sr, center, q))
                .collect(),
        }
    }

    pub fn from_settings(sample_rate: u32, settings: &TrainingSettings) -> Self {
        Self::new(
            sample_rate,
            settings.side_tone_center(),
            settings.band.filter_bandwidth_hz,
        )
    }

    pub fn process(&mut self, input: f64) -> f64 {
        self.stages
            .iter_mut()
            .fold(input, |signal, stage| stage.process(signal))
    }

    /// Run a rendered send through the receiver, in place.
    pub fn apply(&mut self, samples: &mut [f32]) {
        for sample in samples {
            *sample = self.process(f64::from(*sample)) as f32;
        }
    }
}

/// How much of a steady tone at `hz` the receiver passes, as a gain between 0
/// and 1.
///
/// This is the same filter [`ReceiverFilter`] runs, read off rather than run:
/// the textbook magnitude of a second-order band-pass, raised to the number of
/// sections in the cascade. A display that draws this is drawing the filter
/// you are actually listening through, not a picture of one.
pub fn receiver_response_at(center_hz: f64, bandwidth_hz: f64, hz: f64) -> f64 {
    let center = center_hz.max(20.0);
    let freq = hz.max(1e-6);
    let q = receiver_stage_q(center, bandwidth_hz);
    // Detuning term: zero at the centre, growing either side of it.
    let detune = freq / center - center / freq;
    let stage = 1.0 / (1.0 + q * q * detune * detune).sqrt();
    stage.powi(RECEIVER_STAGES as i32)
}

/// Cascading sections narrows the result, so each one is widened to land the
/// cascade on the bandwidth that was asked for. Three sections reach their
/// combined -3 dB point where each is down to 2^(-1/6), which is this much
/// wider than the whole.
const STAGE_WIDENING: f64 = 1.96;

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
/// Crashes a second at each end of the fader.
///
/// The ceiling rose when the crashes got shorter, because that is how a band
/// actually gets noisy: a storm nearer or larger sends *more* sferics, it does
/// not stretch each one out. Sixty a second at about ten milliseconds apiece
/// is a busy crackle with gaps still in it. Past roughly a hundred they merge,
/// the limiter works constantly, and it is a roar again.
const CRASH_RATE_MIN: f64 = 0.4;
const CRASH_RATE_MAX: f64 = 60.0;
/// How much of the output is the steady receiver floor rather than crashes.
const HISS_SHARE: f64 = 0.16;
/// Crash sizes are heavy-tailed — most are small, a few are very loud — which
/// is what gives real static its restless, uneven character.
const CRASH_TAIL: f64 = 1.7;
/// Shortest and longest crash, in seconds — of the *excitation*, before the
/// receiver has had it.
///
/// A sferic reaches the antenna as very nearly an impulse. The crack you hear
/// is not the lightning, it is your own filter being hit and ringing, so this
/// has to stay well short of the ringing or the crash sets its own length.
///
/// It used to be 2 ms to 20 ms, which decay over 14 ms and 138 ms against a
/// filter that rings for 5 to 14 — so every crash came out about 80 ms wide
/// whatever the receiver was set to. That is a thump, not a crack, and closing
/// the filter down did nothing to it.
const CRASH_TAU_MIN: f64 = 0.000_10;
const CRASH_TAU_MAX: f64 = 0.001_20;

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

/// Real-time static and receiver-character generator. QSB is applied
/// separately, to the Morse samples themselves.
pub struct BandMixer {
    sample_rate: f64,
    rng: FastrandRng,
    settings: TrainingSettings,
    t: f64,
    ringing_energy: f64,
    atmospheric: AtmosphericNoise,
    receiver: ReceiverFilter,
    receiver_primary: Svf,
    receiver_secondary: Svf,
    receiver_ring: Svf,
}

impl BandMixer {
    pub fn new(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let settings = settings.clone().clamp();
        let settings_for_filter = settings.clone();
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
            receiver: ReceiverFilter::from_settings(sample_rate.max(1), &settings_for_filter),
            receiver_primary: Svf::bandpass(sr, (center + offset).max(20.0), resonance),
            receiver_secondary: Svf::bandpass(
                sr,
                (center - (offset.abs() + 35.0).max(20.0)).max(20.0),
                (resonance * 0.65).max(0.5),
            ),
            receiver_ring: Svf::bandpass(
                sr,
                (center + offset - 35.0).max(20.0),
                (resonance * 1.45).min(320.0),
            ),
        }
    }

    pub fn needs_background(settings: &TrainingSettings) -> bool {
        (settings.band.qrn_enabled && settings.band.qrn_level > 0.0)
            || (settings.band.receiver_enabled && settings.band.receiver_level > 0.0)
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

    fn receiver_sample(&mut self) -> f64 {
        if !self.settings.band.receiver_enabled || self.settings.band.receiver_level <= 0.0 {
            return 0.0;
        }
        let level = shaped_level(self.settings.band.receiver_level);
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
        let profile = self.settings.band.receiver_profile;

        if matches!(profile, ReceiverProfile::Whistle | ReceiverProfile::Mixed) {
            let primary_f = center + offset + Self::wobble(self.t, depth, rate);
            let secondary_f = center - (offset.abs() + 35.0).max(20.0)
                + Self::wobble(self.t, depth * 0.65, rate * 0.73);
            self.receiver_primary
                .set_bandpass(self.sample_rate, primary_f, resonance);
            self.receiver_secondary.set_bandpass(
                self.sample_rate,
                secondary_f,
                (resonance * 0.65).max(0.5),
            );
            let base =
                RECEIVER_OUTPUT_GAIN * level * model_gain * resonance_compensation(resonance);
            let amp = base + base * 0.18 * (2.0 * std::f64::consts::PI * 0.11 * self.t).sin();
            out += (self.receiver_primary.process(grain) + self.receiver_secondary.process(grain))
                * amp;
        }
        if matches!(profile, ReceiverProfile::Ringing | ReceiverProfile::Mixed) {
            let ring_f = center + offset - 35.0 + Self::wobble(self.t, depth, rate);
            self.receiver_ring.set_bandpass(
                self.sample_rate,
                ring_f,
                (resonance * 1.45).min(320.0),
            );
            out += self.receiver_ring.process(grain)
                * RINGING_OUTPUT_GAIN
                * level
                * model_gain
                * resonance_compensation((resonance * 1.45).min(320.0));
        }
        out
    }

    /// Static, before the receiver shapes it. The crashes are broadband where
    /// they start; it is the filter that turns each one into the short ringing
    /// burst you actually hear, and that filter is shared with everything else.
    fn qrn_excitation(&mut self) -> f64 {
        if !self.settings.band.qrn_enabled || self.settings.band.qrn_level <= 0.0 {
            return 0.0;
        }
        self.atmospheric.next_sample() * QRN_OUTPUT_GAIN
    }

    pub fn next_background(&mut self) -> f32 {
        // Everything meets at the receiver's filter, which is why narrowing it
        // quiets the whole band at once rather than one layer of it.
        let raw = self.qrn_excitation() + self.receiver_sample();
        let heard = self.receiver.process(raw);
        self.t += 1.0 / self.sample_rate;
        soft_limit(heard) as f32
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
        settings.band.receiver_enabled = false;
        settings.band.qrn_enabled = true;
        settings.band.qrn_level = level;
        settings.clamp()
    }

    fn only_receiver(level: f64) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.band.qrn_enabled = false;
        settings.band.receiver_enabled = true;
        settings.band.receiver_level = level;
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
                let mut settings = only_receiver(0.5);
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
                    let mut settings = only_receiver(1.0);
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
        loud.band.receiver_level = 1.0;
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
        settings.band.receiver_level = 1.0;
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

    /// Steady-state amplitude response at one frequency.
    fn response_at(bandwidth: f64, freq: f64) -> f64 {
        const SR: u32 = 8_000;
        let mut filter = ReceiverFilter::new(SR, 500.0, bandwidth);
        let sr = f64::from(SR);
        let mut settled = Vec::new();
        for i in 0..(SR as usize * 2) {
            let x = (2.0 * std::f64::consts::PI * freq * (i as f64 / sr)).sin();
            let y = filter.process(x);
            if i > SR as usize {
                settled.push(y as f32);
            }
        }
        f64::from(rms(&settled)) * std::f64::consts::SQRT_2
    }

    /// How long the filter keeps sounding after the key comes up, measured on
    /// the envelope rather than on the waveform, which crosses zero.
    fn ring_ms(bandwidth: f64) -> f64 {
        const SR: u32 = 8_000;
        let mut filter = ReceiverFilter::new(SR, 500.0, bandwidth);
        let sr = f64::from(SR);
        for i in 0..(SR as usize) {
            filter.process((2.0 * std::f64::consts::PI * 500.0 * (i as f64 / sr)).sin());
        }
        let window = (SR / 500) as usize;
        let mut first = 0.0;
        for step in 0..500 {
            let block: Vec<f32> = (0..window).map(|_| filter.process(0.0) as f32).collect();
            let level = f64::from(rms(&block));
            if step == 0 {
                first = level;
            } else if level < first * 0.05 {
                return step as f64 * window as f64 / sr * 1000.0;
            }
        }
        f64::MAX
    }

    fn band_noise(bandwidth: f64) -> f32 {
        let mut settings = only_qrn(1.0);
        settings.band.filter_bandwidth_hz = bandwidth;
        settings.band.side_tone_min = 500.0;
        settings.band.side_tone_max = 500.0;
        rms(&background(&settings.clamp(), 4))
    }

    /// The whole point of the bandwidth control: it is one filter, so
    /// narrowing it does the three things narrowing a real one does. This is
    /// that claim, measured.
    #[test]
    fn a_narrower_receiver_is_quieter_choosier_and_rings_longer() {
        let (wide, narrow) = (1_000.0, FILTER_BANDWIDTH_MIN);

        // Quieter: the static it passes goes as its bandwidth.
        let quieter = 20.0 * f64::from(band_noise(wide) / band_noise(narrow)).log10();
        assert!(
            quieter > 3.0,
            "narrowing should cut the static, got {quieter:.1} dB"
        );

        // Choosier: a station off your pitch falls away.
        let off_wide = 20.0 * (response_at(wide, 600.0) / response_at(wide, 500.0)).log10();
        let off_narrow = 20.0 * (response_at(narrow, 600.0) / response_at(narrow, 500.0)).log10();
        assert!(
            off_narrow < off_wide - 2.0,
            "a narrow filter should push an off-pitch station down: \
             {off_wide:.1} dB wide vs {off_narrow:.1} dB narrow"
        );
        // But never to nothing, or half the stations would simply vanish.
        assert!(
            off_narrow > -12.0,
            "off-pitch stations disappeared: {off_narrow:.1} dB"
        );

        // Rings longer, which is the sound everybody knows a narrow filter by.
        assert!(
            ring_ms(narrow) > ring_ms(wide) * 1.4,
            "ringing barely changed: {:.1} ms wide vs {:.1} ms narrow",
            ring_ms(wide),
            ring_ms(narrow)
        );

        // And a station on your pitch is left alone: at CW speeds the signal
        // is far narrower than the filter, so "slightly weaker" really is.
        for bandwidth in [wide, 500.0, narrow] {
            let on_pitch = 20.0 * response_at(bandwidth, 500.0).log10();
            assert!(
                on_pitch > -1.0,
                "{bandwidth} Hz cost an on-pitch signal {on_pitch:.1} dB"
            );
        }
    }

    /// The filter is the answer to a pile-up as much as to static. Another
    /// station off your pitch is further from the filter's centre than the one
    /// you are on, so narrowing pushes it down and leaves yours alone — which
    /// is what makes reaching for the filter a tactic rather than a fidget.
    #[test]
    fn narrowing_the_receiver_pushes_an_interfering_station_down() {
        let offset = crate::settings::BandSettings::default().pileup_spread_hz;
        let rejection = |bandwidth: f64| {
            let wanted = response_at(bandwidth, 500.0);
            let other = response_at(bandwidth, 500.0 + offset);
            20.0 * (other / wanted.max(1e-12)).log10()
        };
        let wide = rejection(1_000.0);
        let narrow = rejection(FILTER_BANDWIDTH_MIN);
        assert!(
            narrow < wide - 3.0,
            "narrowing barely touched the pile-up: {wide:.1} dB wide vs {narrow:.1} dB narrow"
        );
        // The station you want is still there, whichever way the filter is set.
        assert!(20.0 * response_at(FILTER_BANDWIDTH_MIN, 500.0).log10() > -1.0);
    }

    /// The control has to mean what it says, or the numbers on the screen are
    /// decoration.
    #[test]
    fn the_bandwidth_control_is_in_hertz_and_lands_where_it_says() {
        for asked in [1_000.0, 500.0, 250.0, FILTER_BANDWIDTH_MIN] {
            let peak = response_at(asked, 500.0);
            let target = peak / std::f64::consts::SQRT_2;
            let edge = |direction: f64| {
                let mut freq = 500.0;
                while freq > 30.0 && freq < 3_500.0 {
                    freq += direction * 2.0;
                    if response_at(asked, freq) < target {
                        return freq;
                    }
                }
                freq
            };
            let measured = edge(1.0) - edge(-1.0);
            let error = (measured - asked).abs() / asked;
            assert!(
                error < 0.15,
                "asked for {asked} Hz and measured {measured:.0} Hz"
            );
        }
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

    /// Milliseconds for an envelope to fall 60 dB below its peak.
    fn decay_ms(env: &[f64]) -> f64 {
        let peak = env.iter().fold(0.0_f64, |a, v| a.max(*v));
        let floor = peak / 1000.0;
        env.iter().rposition(|v| *v > floor).unwrap_or(0) as f64 / 48.0
    }

    /// How long one crash lasts once the receiver has had it.
    fn crash_ms(bandwidth: f64) -> f64 {
        let mut noise = AtmosphericNoise::new(48_000, 1.0, 12_345);
        let mut filter = ReceiverFilter::new(48_000, 600.0, bandwidth);
        // One crash of the usual size, then no further arrivals.
        noise.crash_energy = 1.0;
        let tau = (CRASH_TAU_MIN + CRASH_TAU_MAX) / 2.0;
        noise.crash_decay = (-1.0 / (tau * 48_000.0)).exp();
        let env: Vec<f64> = (0..48_000)
            .map(|_| {
                let crash = noise.crash_energy * (noise.rng.f64() * 2.0 - 1.0);
                noise.crash_energy *= noise.crash_decay;
                filter.process(crash).abs()
            })
            .collect();
        decay_ms(&env)
    }

    /// A static crash is your filter being hit, not the lightning itself, so
    /// the excitation has to stay far shorter than the receiver rings.
    ///
    /// It did not: a 20 ms tail decays over 138 ms against a filter that rings
    /// for 5 to 14, so the crash carried its own length and the receiver had
    /// no say in it.
    #[test]
    fn a_crash_is_shorter_than_the_filter_it_rings() {
        // Both to the same point — down to a twentieth, which is what
        // `ring_ms` reports — so they are comparable.
        let excitation = CRASH_TAU_MAX * (1.0f64 / 0.05).ln() * 1000.0;
        // At the widest ordinary CW setting, the least ringing there is to
        // hide behind.
        let ringing = ring_ms(500.0);
        assert!(
            excitation < ringing,
            "a crash decays over {excitation:.1} ms against a filter ringing for \
             {ringing:.1} ms — the crash would be setting its own length"
        );
    }

    /// And so the receiver decides how long a crash lasts. Close the filter
    /// down and the crashes stretch and start to ring, which is the whole
    /// character of a narrow filter on a noisy band.
    #[test]
    fn a_narrower_filter_makes_the_crashes_ring_longer() {
        let wide = crash_ms(500.0);
        let narrow = crash_ms(150.0);
        assert!(
            narrow > wide * 1.5,
            "a 150 Hz filter rang a crash for {narrow:.1} ms against {wide:.1} ms \
             at 500 Hz — the filter is not shaping the crash"
        );
    }

    /// Static is impulsive, and the crest factor is the number that says so.
    /// At the top of the fader this used to sit near nine with the limiter
    /// working flat out, which is a roar rather than a crackle.
    #[test]
    fn the_static_stays_crackly_even_at_its_loudest() {
        for level in [0.2, 0.5, 1.0] {
            let noisy = background(&only_qrn(level), 4);
            let crest = crest(&noisy);
            assert!(
                crest > 15.0,
                "at {level:.1} the static came out at a crest of {crest:.1}, which \
                 is a wash rather than crashes"
            );
        }
    }
}

#[cfg(test)]
mod mixer_tests {
    use super::*;

    fn quiet() -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.band.qrn_enabled = false;
        s.band.receiver_enabled = false;
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
        interference.band.receiver_enabled = true;
        interference.band.receiver_level = 0.2;
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
    fn every_receiver_profile_produces_sound_and_stays_in_range() {
        for profile in [
            ReceiverProfile::Whistle,
            ReceiverProfile::Ringing,
            ReceiverProfile::Mixed,
        ] {
            let mut settings = quiet();
            settings.band.receiver_enabled = true;
            settings.band.receiver_level = 1.0;
            settings.band.receiver_profile = profile;
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

    /// The response the scope draws has to be the response the audio has, or
    /// the display is a decoration. Measure the real filter and compare.
    #[test]
    fn the_drawn_response_is_the_filter_you_hear() {
        const SAMPLE_RATE: u32 = 48_000;

        fn measured(center: f64, bandwidth: f64, hz: f64) -> f64 {
            let mut filter = ReceiverFilter::new(SAMPLE_RATE, center, bandwidth);
            let step = std::f64::consts::TAU * hz / f64::from(SAMPLE_RATE);
            let (settle, measure) = (24_000usize, 12_000usize);
            let mut sum_sq = 0.0;
            for n in 0..settle + measure {
                let out = filter.process((step * n as f64).sin());
                if n >= settle {
                    sum_sq += out * out;
                }
            }
            // Against a unit sine, whose RMS is 1/sqrt(2).
            (sum_sq / measure as f64).sqrt() * std::f64::consts::SQRT_2
        }

        for (center, bandwidth) in [(500.0, 500.0), (600.0, 250.0), (700.0, 1_200.0)] {
            for offset in [-400.0, -200.0, -80.0, 0.0, 80.0, 200.0, 400.0] {
                let hz = center + offset;
                if hz < 60.0 {
                    continue;
                }
                let drawn = receiver_response_at(center, bandwidth, hz);
                let real = measured(center, bandwidth, hz);
                assert!(
                    (drawn - real).abs() < 0.03,
                    "{center} Hz / {bandwidth} Hz at {hz} Hz: drew {drawn:.3}, measured {real:.3}"
                );
            }
        }
    }

    /// A band-pass is a band-pass: strongest where it is tuned, and falling
    /// away either side of that however wide it is set.
    #[test]
    fn the_response_peaks_where_the_filter_is_tuned() {
        for bandwidth in [FILTER_BANDWIDTH_MIN, 500.0, FILTER_BANDWIDTH_MAX] {
            let peak = receiver_response_at(600.0, bandwidth, 600.0);
            assert!((peak - 1.0).abs() < 1e-9, "the centre should pass in full");
            let mut previous = peak;
            for offset in [20.0, 60.0, 120.0, 260.0, 520.0, 1_040.0] {
                let above = receiver_response_at(600.0, bandwidth, 600.0 + offset);
                let below = receiver_response_at(600.0, bandwidth, 600.0 - offset);
                assert!(
                    above < previous && above > 0.0,
                    "{bandwidth}: {offset} Hz up"
                );
                assert!(below < previous, "{bandwidth}: {offset} Hz down");
                previous = above;
            }
        }
    }

    /// Narrower means narrower: at a fixed distance off the centre, squeezing
    /// the filter always passes less of what is out there.
    #[test]
    fn narrowing_the_filter_passes_less_of_what_is_beside_you() {
        for offset in [100.0, 200.0, 400.0] {
            let wide = receiver_response_at(600.0, 1_000.0, 600.0 + offset);
            let narrow = receiver_response_at(600.0, 200.0, 600.0 + offset);
            assert!(
                narrow < wide,
                "{offset} Hz off: narrow passed {narrow:.4}, wide {wide:.4}"
            );
        }
    }
}
