//! What arrives at the antenna before the receiver has shaped it: the noise
//! floor and atmospheric static.
//!
//! Both are *excitation*. Neither has a pitch of its own — the noise floor is
//! white and a sferic is an impulse — and both become what an operator hears
//! only once the receiver's filter has had them. The floor comes out as a
//! steady hiss pitched at the filter's centre; each sferic comes out as the
//! filter's own impulse response, a short tick that rings at the same pitch.
//! That shared filter is what makes a band sound like a band.
//!
//! Levels are absolute, in units of the station you are copying: a floor is
//! set by its signal-to-noise ratio in a [`REFERENCE_BANDWIDTH_HZ`] filter,
//! and a sferic by how far its peak through that filter stands above the
//! default floor. Everything is scaled by the sample rate so a band sounds
//! the same at 44.1 kHz as at 48 kHz.

use std::f64::consts::{SQRT_2, TAU};

use super::activity::BandActivity;
use crate::rng::{FastrandRng, Rng};
use crate::settings::TrainingSettings;
use crate::timing::DEFAULT_TARGET_GAIN;

/// Key-down RMS of a station at full level: what every signal-to-noise figure
/// here is measured against.
pub const SIGNAL_RMS: f64 = DEFAULT_TARGET_GAIN / SQRT_2;

/// The bandwidth noise levels are quoted in, as CW signal reports are.
pub const REFERENCE_BANDWIDTH_HZ: f64 = 500.0;

/// Signal-to-noise in the reference bandwidth at each end of the noise
/// control. Forty decibels is a hiss you hear only between elements; two is
/// a station barely out of the noise, where copying is genuinely hard but a
/// trained ear still gets most of it.
const NOISE_SNR_QUIET_DB: f64 = 40.0;
const NOISE_SNR_LOUD_DB: f64 = 2.0;

/// The noise level QRN is calibrated against — the default floor — so static
/// sits the same distance above an ordinary band whether or not the floor is
/// switched on.
const QRN_REFERENCE_NOISE_LEVEL: f64 = 0.5;

/// Signal-to-noise of the floor at `level`, in decibels in the reference
/// bandwidth.
///
/// Linear in decibels, because loudness is: every step of the slider is the
/// same audible step, rather than all the action bunched at one end.
pub fn noise_floor_snr_db(level: f64) -> f64 {
    let level = level.clamp(0.0, 1.0);
    NOISE_SNR_QUIET_DB + (NOISE_SNR_LOUD_DB - NOISE_SNR_QUIET_DB) * level
}

/// RMS of the floor at `level` through a filter [`REFERENCE_BANDWIDTH_HZ`]
/// wide. Zero means off.
pub fn noise_floor_rms(level: f64) -> f64 {
    if level.is_nan() || level <= 0.0 {
        return 0.0;
    }
    SIGNAL_RMS * 10f64.powf(-noise_floor_snr_db(level) / 20.0)
}

/// Signal-to-noise of the floor at `level` as heard through a filter
/// `bandwidth_hz` wide: the reference figure, improved by however much less
/// noise the narrower filter lets in. Halve the width, gain 3 dB — which is
/// the reason to reach for the filter at all.
pub fn heard_snr_db(level: f64, bandwidth_hz: f64) -> f64 {
    let bandwidth = if bandwidth_hz.is_finite() && bandwidth_hz > 0.0 {
        bandwidth_hz
    } else {
        REFERENCE_BANDWIDTH_HZ
    };
    noise_floor_snr_db(level) + 10.0 * (REFERENCE_BANDWIDTH_HZ / bandwidth).log10()
}

/// Standard normal deviates, by Box–Muller.
///
/// The floor has to be Gaussian, not merely random: band noise is the sum of
/// countless independent sources, and only a Gaussian through a narrow filter
/// has the Rayleigh envelope — the restless, grainy "shhh" with a pitch —
/// that a receiver's hiss has. Uniform noise through the same filter comes
/// out close, but this is exact and costs nothing.
#[derive(Clone, Debug)]
pub struct Gaussian {
    rng: FastrandRng,
    spare: Option<f64>,
}

impl Gaussian {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: FastrandRng(seed),
            spare: None,
        }
    }

    pub fn sample(&mut self) -> f64 {
        if let Some(value) = self.spare.take() {
            return value;
        }
        // 1 − u keeps the logarithm's argument in (0, 1].
        let radius = (-2.0 * (1.0 - self.rng.f64()).ln()).sqrt();
        let angle = TAU * self.rng.f64();
        self.spare = Some(radius * angle.sin());
        radius * angle.cos()
    }

    /// A uniform draw from the same stream, for the decisions that are not
    /// about level.
    fn uniform(&mut self) -> f64 {
        self.rng.f64()
    }

    /// Log-normal around a geometric mean: what the durations and spacings in
    /// a lightning flash follow.
    fn log_normal(&mut self, geometric_mean: f64, sigma_ln: f64) -> f64 {
        geometric_mean * (sigma_ln * self.sample()).exp()
    }
}

/// The receiver's noise floor: white Gaussian noise, at a level that comes
/// out of a [`REFERENCE_BANDWIDTH_HZ`] filter at the RMS it was given.
///
/// Through the receiver it is the hiss every operator knows — centred on the
/// pitch, as wide as the filter, and quieter as the filter closes because a
/// narrower filter lets less of it through.
#[derive(Clone, Debug)]
pub struct NoiseFloor {
    gaussian: Gaussian,
    sigma: f64,
}

impl NoiseFloor {
    pub fn new(sample_rate: u32, rms_in_reference: f64, seed: u64) -> Self {
        // White noise of variance σ² spreads its power evenly up to Nyquist,
        // so a filter B wide passes σ²·2B/fs of it.
        let fs = f64::from(sample_rate.max(1));
        Self {
            gaussian: Gaussian::new(seed),
            sigma: rms_in_reference.max(0.0) * (fs / (2.0 * REFERENCE_BANDWIDTH_HZ)).sqrt(),
        }
    }

    pub fn from_settings(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let level = if settings.band.noise_enabled {
            settings.band.noise_level
        } else {
            0.0
        };
        Self::new(sample_rate, noise_floor_rms(level), seed)
    }

    pub fn is_silent(&self) -> bool {
        self.sigma <= 0.0
    }

    pub fn next_sample(&mut self) -> f64 {
        if self.is_silent() {
            return 0.0;
        }
        self.gaussian.sample() * self.sigma
    }
}

/// Atmospheric static: every thunderstorm within a few thousand kilometres,
/// heard through a CW filter.
///
/// Two processes, because that is what is out there:
///
/// - **Crackle.** Distant storms send sferics continuously, each one an
///   impulse by the time it arrives. Through a 500 Hz filter they are ticks a
///   few milliseconds long, of wildly varying size — the "frying" under a
///   summer band. Their rate is what the static control mostly turns up: a
///   couple a second on a quiet band, over a hundred when it is bad.
/// - **Crashes.** A storm near enough to matter is heard flash by flash. A
///   cloud-to-ground flash is a handful of return strokes, about sixty
///   milliseconds apart, each a cluster of impulses, laid over a train of
///   smaller in-cloud discharges (K-changes) every dozen milliseconds or so
///   for a few hundred milliseconds. Half of all flashes stay inside the
///   cloud and are only the train. Through the filter that is a crash with
///   texture: a few loud cracks inside a crackling burst, and then the AGC
///   letting the band back up.
///
/// The figures are the measured ones: 3.8 strokes a flash on average with one
/// in five single-stroke, 60 ms between strokes, K-changes at about 12 ms,
/// flashes of about 0.3 s.
///
/// Allocation-free and constant-time per sample, because it runs inside the
/// audio callback.
#[derive(Clone, Debug)]
pub struct Atmospherics {
    random: Gaussian,
    sample_rate: f64,
    /// Sample amplitude of an impulse whose peak through the reference filter
    /// equals the reference floor's RMS.
    unit: f64,
    crackle: Arrivals,
    crackle_median_db: f64,
    flashes: Arrivals,
    stroke_median_db: f64,
    bursts: [Burst; MAX_BURSTS],
    flash_slots: [Flash; MAX_FLASHES],
}

/// Impulse clusters that can be sounding at once. More would only be needed
/// at rates where they merge into a roar anyway; a cluster that finds no room
/// is dropped.
const MAX_BURSTS: usize = 24;
/// Flashes that can overlap. Two at once is already a violent storm.
const MAX_FLASHES: usize = 4;

/// Crackle rate at each end of the control, per second.
const CRACKLE_RATE_MIN: f64 = 2.0;
const CRACKLE_RATE_MAX: f64 = 120.0;
/// Median crackle peak above the reference floor, in decibels, at each end,
/// and how widely sferics vary around it.
const CRACKLE_MEDIAN_DB_MIN: f64 = -3.0;
const CRACKLE_MEDIAN_DB_MAX: f64 = 6.0;
const CRACKLE_SIGMA_DB: f64 = 7.0;
const CRACKLE_CAP_DB: f64 = 25.0;
/// How long one distant sferic's cluster of sub-impulses lasts.
const CRACKLE_SPAN_SEC: f64 = 0.002;

/// Flashes per second at each end of the control. Even a quiet band has the
/// occasional crash from somewhere.
const FLASH_RATE_MIN: f64 = 0.02;
const FLASH_RATE_MAX: f64 = 0.3;
/// Median first-stroke peak above the reference floor, and its spread.
const STROKE_MEDIAN_DB_MIN: f64 = 18.0;
const STROKE_MEDIAN_DB_MAX: f64 = 32.0;
const STROKE_SIGMA_DB: f64 = 6.0;
const STROKE_CAP_DB: f64 = 45.0;
/// How much weaker each later stroke is than the one before, in decibels.
const STROKE_FALL_DB: (f64, f64) = (3.0, 8.0);
/// One flash in five is a single stroke; the rest have two plus a geometric
/// number more. Continuing with probability c adds c/(1 − c) strokes on
/// average, so 5/7 makes 0.2 × 1 + 0.8 × (2 + 2.5) = 3.8 in all.
const SINGLE_STROKE_SHARE: f64 = 0.2;
const EXTRA_STROKE_CONTINUE: f64 = 5.0 / 7.0;
const MAX_STROKES: u8 = 12;
/// Inter-stroke interval: log-normal, geometric mean 60 ms.
const INTERSTROKE_SEC: (f64, f64) = (0.060, 0.6);
const INTERSTROKE_LIMITS_SEC: (f64, f64) = (0.010, 0.300);
/// A return stroke is itself a cluster: this many impulses, over this long.
const STROKE_IMPULSES: (usize, usize) = (5, 20);
const STROKE_SPAN_SEC: (f64, f64) = (0.001, 0.005);
/// Share of flashes that never reach the ground.
const INTRA_CLOUD_SHARE: f64 = 0.5;
/// Flash duration: log-normal, geometric mean 0.3 s.
const FLASH_SEC: (f64, f64) = (0.3, 0.5);
const FLASH_LIMITS_SEC: (f64, f64) = (0.15, 1.0);
/// K-change spacing: log-normal, geometric mean 12 ms.
const K_CHANGE_SEC: (f64, f64) = (0.012, 0.6);
const K_CHANGE_LIMITS_SEC: (f64, f64) = (0.002, 0.060);
/// How far below the flash's first stroke its K-changes sit.
const K_CHANGE_BELOW_DB: (f64, f64) = (12.0, 20.0);
const K_CHANGE_SIGMA_DB: f64 = 4.0;

impl Atmospherics {
    pub fn new(sample_rate: u32, level: f64, seed: u64) -> Self {
        let fs = f64::from(sample_rate.max(1));
        let level = if level.is_nan() {
            0.0
        } else {
            level.clamp(0.0, 1.0)
        };
        let between = |low: f64, high: f64| low + (high - low) * level;
        // Rates are spread on a log scale, like the noise control: every step
        // of the slider multiplies the activity rather than adding to it.
        let log_between = |low: f64, high: f64| low * (high / low).powf(level);
        let mut random = Gaussian::new(seed | 1);
        let crackle_rate = if level > 0.0 {
            log_between(CRACKLE_RATE_MIN, CRACKLE_RATE_MAX)
        } else {
            0.0
        };
        let flash_rate = if level > 0.0 {
            log_between(FLASH_RATE_MIN, FLASH_RATE_MAX)
        } else {
            0.0
        };
        let crackle = Arrivals::new(crackle_rate, fs, &mut random);
        let flashes = Arrivals::new(flash_rate, fs, &mut random);
        Self {
            random,
            sample_rate: fs,
            // A one-sample impulse of amplitude a carries a/fs of area; a
            // filter B wide turns that into a peak of about 2B·a/fs, while
            // passing noise at RMS σ·√(2B/fs). Equating the two fixes the
            // impulse that peaks at the reference floor.
            unit: noise_floor_rms(QRN_REFERENCE_NOISE_LEVEL) * fs / (2.0 * REFERENCE_BANDWIDTH_HZ),
            crackle,
            crackle_median_db: between(CRACKLE_MEDIAN_DB_MIN, CRACKLE_MEDIAN_DB_MAX),
            flashes,
            stroke_median_db: between(STROKE_MEDIAN_DB_MIN, STROKE_MEDIAN_DB_MAX),
            bursts: [Burst::default(); MAX_BURSTS],
            flash_slots: [Flash::default(); MAX_FLASHES],
        }
    }

    pub fn from_settings(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let level = if settings.band.qrn_enabled {
            settings.band.qrn_level
        } else {
            0.0
        };
        Self::new(sample_rate, level, seed)
    }

    pub fn is_silent(&self) -> bool {
        self.crackle.is_off() && self.flashes.is_off()
    }

    /// One sample of excitation, before the receiver's filter shapes it.
    pub fn next_sample(&mut self) -> f64 {
        if self.is_silent() {
            return 0.0;
        }
        if self.crackle.arrived(self.sample_rate, &mut self.random) {
            self.start_crackle();
        }
        if self.flashes.arrived(self.sample_rate, &mut self.random) {
            self.start_flash();
        }
        for index in 0..MAX_FLASHES {
            if self.flash_slots[index].is_active() {
                self.step_flash(index);
            }
        }
        let mut out = 0.0;
        for burst in &mut self.bursts {
            out += burst.step(&mut self.random);
        }
        out
    }

    fn amplitude(&self, db: f64) -> f64 {
        self.unit * 10f64.powf(db / 20.0)
    }

    fn seconds(&self, sec: f64) -> u32 {
        (sec * self.sample_rate)
            .round()
            .clamp(0.0, f64::from(u32::MAX)) as u32
    }

    fn log_normal_clamped(&mut self, (mean, sigma): (f64, f64), (low, high): (f64, f64)) -> f64 {
        self.random.log_normal(mean, sigma).clamp(low, high)
    }

    fn start_crackle(&mut self) {
        let db =
            (self.crackle_median_db + CRACKLE_SIGMA_DB * self.random.sample()).min(CRACKLE_CAP_DB);
        let impulses = 1 + (self.random.uniform() * 4.0) as usize;
        let span = self.seconds(CRACKLE_SPAN_SEC);
        self.start_burst(self.amplitude(db), impulses, span, 1.0);
    }

    fn start_flash(&mut self) {
        let Some(index) = self.flash_slots.iter().position(|f| !f.is_active()) else {
            return;
        };
        let first_db =
            (self.stroke_median_db + STROKE_SIGMA_DB * self.random.sample()).min(STROKE_CAP_DB);
        let strokes = if self.random.uniform() < INTRA_CLOUD_SHARE {
            0
        } else if self.random.uniform() < SINGLE_STROKE_SHARE {
            1
        } else {
            let mut count = 2;
            while count < MAX_STROKES && self.random.uniform() < EXTRA_STROKE_CONTINUE {
                count += 1;
            }
            count
        };
        let below = K_CHANGE_BELOW_DB.0
            + (K_CHANGE_BELOW_DB.1 - K_CHANGE_BELOW_DB.0) * self.random.uniform();
        let duration = self.log_normal_clamped(FLASH_SEC, FLASH_LIMITS_SEC);
        // The leader comes first: the first return stroke follows the start of
        // the discharge by a few tens of milliseconds.
        let first_stroke = 0.005 + 0.025 * self.random.uniform();
        self.flash_slots[index] = Flash {
            k_left: self.seconds(duration),
            k_wait: 0,
            k_amp: self.amplitude(first_db - below),
            strokes_left: strokes,
            stroke_wait: self.seconds(first_stroke),
            stroke_amp: self.amplitude(first_db),
        };
    }

    fn step_flash(&mut self, index: usize) {
        let mut flash = self.flash_slots[index];
        if flash.k_left > 0 {
            flash.k_left -= 1;
            if flash.k_wait == 0 {
                let jitter = 10f64.powf(K_CHANGE_SIGMA_DB * self.random.sample() / 20.0);
                let impulses = 1 + (self.random.uniform() * 3.0) as usize;
                let span = self.seconds(0.001);
                self.start_burst(flash.k_amp * jitter, impulses, span, 1.0);
                let gap = self.log_normal_clamped(K_CHANGE_SEC, K_CHANGE_LIMITS_SEC);
                flash.k_wait = self.seconds(gap);
            } else {
                flash.k_wait -= 1;
            }
        }
        if flash.strokes_left > 0 {
            if flash.stroke_wait == 0 {
                let (fewest, most) = STROKE_IMPULSES;
                let impulses = self.random.rng.usize_in(fewest, most);
                let span_sec = STROKE_SPAN_SEC.0
                    + (STROKE_SPAN_SEC.1 - STROKE_SPAN_SEC.0) * self.random.uniform();
                let span = self.seconds(span_sec);
                // The current in a stroke falls away over its own few
                // milliseconds: the cluster starts loud and ends some 20 dB
                // down.
                let decay = 10f64.powf(-1.0 / (impulses.max(2) - 1) as f64);
                self.start_burst(flash.stroke_amp, impulses, span, decay);
                let fall = STROKE_FALL_DB.0
                    + (STROKE_FALL_DB.1 - STROKE_FALL_DB.0) * self.random.uniform();
                flash.stroke_amp *= 10f64.powf(-fall / 20.0);
                flash.strokes_left -= 1;
                let gap = self.log_normal_clamped(INTERSTROKE_SEC, INTERSTROKE_LIMITS_SEC);
                flash.stroke_wait = self.seconds(gap);
            } else {
                flash.stroke_wait -= 1;
            }
        }
        self.flash_slots[index] = flash;
    }

    fn start_burst(&mut self, amplitude: f64, impulses: usize, span: u32, decay: f64) {
        let Some(slot) = self.bursts.iter_mut().find(|b| b.left == 0) else {
            return;
        };
        let impulses = impulses.clamp(1, usize::from(u8::MAX));
        *slot = Burst {
            left: impulses as u8,
            wait: 0,
            spacing: span / impulses as u32,
            amplitude,
            decay,
        };
    }
}

/// A Poisson process, counted down in samples.
#[derive(Clone, Copy, Debug)]
struct Arrivals {
    rate: f64,
    wait: u64,
}

impl Arrivals {
    fn new(rate: f64, sample_rate: f64, random: &mut Gaussian) -> Self {
        let mut arrivals = Self { rate, wait: 0 };
        arrivals.wait = arrivals.draw(sample_rate, random);
        arrivals
    }

    fn is_off(&self) -> bool {
        self.rate <= 0.0
    }

    /// Exponential waiting time to the next arrival, in samples.
    fn draw(&self, sample_rate: f64, random: &mut Gaussian) -> u64 {
        if self.is_off() {
            return u64::MAX;
        }
        let wait = -(1.0 - random.uniform()).ln() * sample_rate / self.rate;
        wait.round().clamp(0.0, 1e15) as u64
    }

    fn arrived(&mut self, sample_rate: f64, random: &mut Gaussian) -> bool {
        if self.is_off() {
            return false;
        }
        if self.wait == 0 {
            self.wait = self.draw(sample_rate, random);
            true
        } else {
            self.wait -= 1;
            false
        }
    }
}

/// A cluster of impulses of random sign and spacing, each a little smaller
/// than the one before it.
#[derive(Clone, Copy, Debug, Default)]
struct Burst {
    left: u8,
    wait: u32,
    spacing: u32,
    amplitude: f64,
    decay: f64,
}

impl Burst {
    fn step(&mut self, random: &mut Gaussian) -> f64 {
        if self.left == 0 {
            return 0.0;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return 0.0;
        }
        let sign = if random.uniform() < 0.5 { -1.0 } else { 1.0 };
        let value = sign * self.amplitude * (0.6 + 0.4 * random.uniform());
        self.amplitude *= self.decay;
        self.left -= 1;
        self.wait = (random.uniform() * 2.0 * f64::from(self.spacing)) as u32;
        value
    }
}

/// One lightning flash in progress.
#[derive(Clone, Copy, Debug, Default)]
struct Flash {
    /// Samples of K-change train still to come.
    k_left: u32,
    k_wait: u32,
    k_amp: f64,
    strokes_left: u8,
    stroke_wait: u32,
    stroke_amp: f64,
}

impl Flash {
    fn is_active(&self) -> bool {
        self.k_left > 0 || self.strokes_left > 0
    }
}

/// Everything the band puts into the receiver, before the receiver: the noise
/// floor and the static. Both backends play this — the native player runs it
/// through its filter sample by sample, the browser streams it in buffers into
/// a filter made of nodes.
#[derive(Clone, Debug)]
pub struct BandSource {
    floor: NoiseFloor,
    atmospherics: Atmospherics,
    activity: BandActivity,
}

impl BandSource {
    pub fn new(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        Self {
            floor: NoiseFloor::from_settings(sample_rate, settings, seed ^ 0x9E37_79B9_7F4A_7C15),
            atmospherics: Atmospherics::from_settings(sample_rate, settings, seed ^ 0x51ED_2701),
            activity: BandActivity::from_settings(sample_rate, settings, seed ^ 0xAC71_7174),
        }
    }

    pub fn is_silent(&self) -> bool {
        self.floor.is_silent() && self.atmospherics.is_silent() && self.activity.is_silent()
    }

    pub fn next_sample(&mut self) -> f64 {
        self.floor.next_sample() + self.atmospherics.next_sample() + self.activity.next_sample()
    }

    pub fn fill(&mut self, out: &mut [f32]) {
        for slot in out {
            *slot = self.next_sample() as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn geometric_mean(values: &[f64]) -> f64 {
        (values.iter().map(|v| v.ln()).sum::<f64>() / values.len() as f64).exp()
    }

    #[test]
    fn a_narrower_filter_hears_a_better_signal_to_noise_ratio() {
        let reference = heard_snr_db(0.5, REFERENCE_BANDWIDTH_HZ);
        assert!((reference - noise_floor_snr_db(0.5)).abs() < 1e-12);
        assert!((heard_snr_db(0.5, 250.0) - reference - 3.0103).abs() < 1e-3);
        assert!(heard_snr_db(0.5, 2_000.0) < reference);
        assert!((heard_snr_db(0.5, f64::NAN) - reference).abs() < 1e-12);
        assert!(heard_snr_db(1.0, 500.0) < heard_snr_db(0.0, 500.0));
    }

    #[test]
    fn the_gaussian_is_a_standard_normal() {
        let mut g = Gaussian::new(3);
        let n = 200_000;
        let draws: Vec<f64> = (0..n).map(|_| g.sample()).collect();
        let mean = draws.iter().sum::<f64>() / f64::from(n);
        let var = draws.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / f64::from(n);
        let within_one = draws.iter().filter(|x| x.abs() < 1.0).count() as f64 / f64::from(n);
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((var - 1.0).abs() < 0.015, "variance {var}");
        assert!(
            (within_one - 0.6827).abs() < 0.005,
            "{within_one} within one sigma"
        );
        assert!(draws.iter().all(|x| x.is_finite()));
    }

    /// Lightning, as counted by the people who count it: a cloud-to-ground
    /// flash averages 3.8 return strokes, one in five has just the one, and
    /// half of all flashes never reach the ground at all.
    #[test]
    fn flashes_have_the_stroke_counts_of_real_lightning() {
        let mut sky = Atmospherics::new(SR, 0.5, 11);
        let mut counts = Vec::new();
        for _ in 0..20_000 {
            sky.flash_slots = [Flash::default(); MAX_FLASHES];
            sky.start_flash();
            counts.push(f64::from(sky.flash_slots[0].strokes_left));
        }
        let ground: Vec<f64> = counts.iter().copied().filter(|c| *c > 0.0).collect();
        let intra_cloud = 1.0 - ground.len() as f64 / counts.len() as f64;
        let mean = ground.iter().sum::<f64>() / ground.len() as f64;
        let single = ground.iter().filter(|c| **c == 1.0).count() as f64 / ground.len() as f64;
        assert!(
            (intra_cloud - 0.5).abs() < 0.02,
            "{intra_cloud:.3} intra-cloud"
        );
        assert!((3.5..4.0).contains(&mean), "{mean:.2} strokes a flash");
        assert!((single - 0.2).abs() < 0.02, "{single:.3} single-stroke");
        assert!(ground.iter().all(|c| *c <= f64::from(MAX_STROKES)));
    }

    /// Strokes about sixty milliseconds apart, in-cloud pulses about twelve:
    /// that spacing is what turns a crash from one click into the ragged
    /// "krrsh" of a real one.
    #[test]
    fn strokes_and_k_changes_are_spaced_like_real_ones() {
        let mut sky = Atmospherics::new(SR, 0.5, 5);
        let (mut strokes, mut k_changes) = (Vec::new(), Vec::new());
        for _ in 0..400 {
            sky.flash_slots = [Flash::default(); MAX_FLASHES];
            sky.bursts = [Burst::default(); MAX_BURSTS];
            sky.start_flash();
            let (mut last_stroke, mut last_k) = (None, None);
            let mut t = 0u32;
            while sky.flash_slots[0].is_active() {
                let before = sky.flash_slots[0];
                sky.step_flash(0);
                let after = sky.flash_slots[0];
                if after.strokes_left < before.strokes_left {
                    if let Some(previous) = last_stroke {
                        strokes.push(f64::from(t - previous) / f64::from(SR));
                    }
                    last_stroke = Some(t);
                }
                if before.k_left > 0 && before.k_wait == 0 {
                    if let Some(previous) = last_k {
                        k_changes.push(f64::from(t - previous) / f64::from(SR));
                    }
                    last_k = Some(t);
                }
                // Drain the bursts so the pool never fills.
                sky.bursts = [Burst::default(); MAX_BURSTS];
                t += 1;
            }
        }
        let stroke_gm = geometric_mean(&strokes);
        let k_gm = geometric_mean(&k_changes);
        assert!(
            (0.050..0.070).contains(&stroke_gm),
            "strokes {stroke_gm:.4} s apart"
        );
        assert!(
            (0.010..0.015).contains(&k_gm),
            "K-changes {k_gm:.4} s apart"
        );
        assert!(strokes.iter().all(|s| (0.0099..=0.3001).contains(s)));
    }

    /// A Poisson process arrives at its rate, and an off one never arrives.
    #[test]
    fn arrivals_come_at_their_rate() {
        let mut random = Gaussian::new(9);
        let fs = f64::from(SR);
        let mut arrivals = Arrivals::new(50.0, fs, &mut random);
        let seconds = 60;
        let count = (0..SR as usize * seconds)
            .filter(|_| arrivals.arrived(fs, &mut random))
            .count() as f64;
        let expected = 50.0 * seconds as f64;
        assert!(
            (count - expected).abs() < 5.0 * expected.sqrt(),
            "{count} arrivals"
        );

        let mut off = Arrivals::new(0.0, fs, &mut random);
        assert!(off.is_off());
        assert!((0..10_000).all(|_| !off.arrived(fs, &mut random)));
    }

    #[test]
    fn static_at_zero_is_silence() {
        let mut sky = Atmospherics::new(SR, 0.0, 1);
        assert!(sky.is_silent());
        assert!((0..48_000).all(|_| sky.next_sample() == 0.0));
        assert!(Atmospherics::new(SR, f64::NAN, 1).is_silent());
        let mut floor = NoiseFloor::new(SR, 0.0, 1);
        assert!(floor.is_silent());
        assert_eq!(floor.next_sample(), 0.0);
    }

    /// The busier the band, the more of everything: crackle and crashes both
    /// arrive faster and land louder as the control goes up.
    #[test]
    fn the_static_control_turns_up_rate_and_size() {
        let quiet = Atmospherics::new(SR, 0.1, 1);
        let busy = Atmospherics::new(SR, 0.9, 1);
        assert!(busy.crackle.rate > quiet.crackle.rate * 10.0);
        assert!(busy.flashes.rate > quiet.flashes.rate * 5.0);
        assert!(busy.crackle_median_db > quiet.crackle_median_db);
        assert!(busy.stroke_median_db > quiet.stroke_median_db);
        assert!((CRACKLE_RATE_MIN..=CRACKLE_RATE_MAX).contains(&busy.crackle.rate));
        assert!((FLASH_RATE_MIN..=FLASH_RATE_MAX).contains(&quiet.flashes.rate));
    }

    /// A pool that is full drops what does not fit rather than allocating in
    /// the audio callback.
    #[test]
    fn a_full_pool_drops_rather_than_grows() {
        let mut sky = Atmospherics::new(SR, 1.0, 1);
        for _ in 0..MAX_BURSTS * 3 {
            sky.start_burst(1.0, 4, 100, 1.0);
        }
        assert!(sky.bursts.iter().all(|b| b.left > 0));
        for _ in 0..MAX_FLASHES * 3 {
            sky.start_flash();
        }
        assert!(sky.flash_slots.iter().filter(|f| f.is_active()).count() <= MAX_FLASHES);
        assert!((0..48_000).all(|_| sky.next_sample().is_finite()));
    }
}
