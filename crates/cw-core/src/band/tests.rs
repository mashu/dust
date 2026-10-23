//! What the band has to sound like, measured.
//!
//! Most of these listen the way an engineer would: render a few seconds,
//! then look at the level, the spectrum and the statistics. They pin the
//! properties that make a receiver sound like a receiver — the ones whose
//! absence makes a band sound like the wind.

use std::f64::consts::TAU;

use super::agc::agc_law_db;
use super::calibration::{REFERENCE_BANDWIDTH_HZ, SIGNAL_RMS, noise_floor_rms, noise_floor_snr_db};
use super::noise::{Atmospherics, FLOOR_BELOW_DB, NoiseFloor};
use super::test_support::{
    ALL_SHAPES, Fft, IF_SHAPES, knee_for, kurtosis, peak, receiver_for, rms, source_for, welch,
};
use super::*;
use crate::settings::{FILTER_WIDTH_MAX, FILTER_WIDTH_MIN, FilterShape};

const SR: u32 = 48_000;

fn background(settings: &TrainingSettings, seconds: usize) -> Vec<f32> {
    let mut mixer = BandMixer::new(SR, settings, 9);
    let mut out = vec![0.0f32; SR as usize * seconds];
    mixer.fill_background(&mut out);
    out
}

/// Peak over RMS. Gaussian noise sits near 4; static pushes it far higher,
/// which is the whole difference between a hiss and a band with lightning on
/// it.
fn crest(v: &[f32]) -> f64 {
    let r = rms(v);
    if r > 0.0 { peak(v) / r } else { 0.0 }
}

fn quiet() -> TrainingSettings {
    let mut s = TrainingSettings::default();
    s.band.qrn_enabled = false;
    s.band.noise_enabled = false;
    s
}

fn only_qrn(level: f64) -> TrainingSettings {
    let mut s = quiet();
    s.band.qrn_enabled = true;
    s.band.qrn_level = level;
    s.clamp()
}

fn only_noise(level: f64) -> TrainingSettings {
    let mut s = quiet();
    s.band.noise_enabled = true;
    s.band.noise_level = level;
    s.clamp()
}

fn at(
    mut s: TrainingSettings,
    center: f64,
    bandwidth: f64,
    shape: FilterShape,
) -> TrainingSettings {
    s.band.side_tone_min = center;
    s.band.side_tone_max = center;
    s.band.filter_width_hz = bandwidth;
    s.band.filter_shape = shape;
    s.clamp()
}

/// The receiver's noise, straight from the filter, with no AGC or limiter in
/// the way — for measuring the noise itself.
fn filtered_floor(
    sample_rate: u32,
    level: f64,
    settings: &TrainingSettings,
    seconds: f64,
) -> Vec<f32> {
    let mut floor = NoiseFloor::new(sample_rate, noise_floor_rms(level), 77);
    let mut filter = receiver_for(sample_rate, settings);
    let n = (f64::from(sample_rate) * seconds) as usize;
    // Let the filter fill before measuring.
    for _ in 0..sample_rate / 10 {
        filter.process(floor.next_sample());
    }
    (0..n)
        .map(|_| filter.process(floor.next_sample()) as f32)
        .collect()
}

/// Power at `hz`, by a Hann-windowed DFT averaged over half-overlapping
/// segments (Welch). Slow and simple, which is all a test needs.
fn psd(x: &[f32], sample_rate: u32, freqs: &[f64], segment: usize) -> Vec<f64> {
    let window: Vec<f64> = (0..segment)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / segment as f64).cos())
        .collect();
    let mut acc = vec![0.0; freqs.len()];
    let mut segments = 0;
    let mut start = 0;
    while start + segment <= x.len() {
        for (slot, f) in acc.iter_mut().zip(freqs) {
            let w = TAU * f / f64::from(sample_rate);
            let (mut re, mut im) = (0.0, 0.0);
            for (i, win) in window.iter().enumerate() {
                let v = f64::from(x[start + i]) * win;
                let (s, c) = (w * i as f64).sin_cos();
                re += v * c;
                im -= v * s;
            }
            *slot += re * re + im * im;
        }
        segments += 1;
        start += segment / 2;
    }
    acc.into_iter()
        .map(|a| a / f64::from(segments.max(1)))
        .collect()
}

// ------------------------------------------------------------ the noise floor

/// The floor is Gaussian noise through the filter. Its sample values are
/// normally distributed — kurtosis three — and so its envelope is Rayleigh:
/// the restless, grainy hiss of a real receiver. Impulsive excitation fails
/// both.
#[test]
fn the_noise_floor_is_gaussian() {
    for shape in IF_SHAPES {
        let settings = at(TrainingSettings::default(), 600.0, 500.0, shape);
        let x = filtered_floor(SR, 0.5, &settings, 8.0);
        let k = kurtosis(&x);
        assert!((2.85..3.15).contains(&k), "{shape:?}: kurtosis {k:.3}");
        // RMS over mean magnitude is √(π/2) for a Gaussian.
        let mean_abs = x.iter().map(|s| f64::from(s.abs())).sum::<f64>() / x.len() as f64;
        let ratio = rms(&x) / mean_abs;
        assert!(
            (ratio - 1.2533).abs() < 0.01,
            "{shape:?}: rms/mean {ratio:.4}"
        );
    }
}

/// The regression test for the whistle. Band noise has no tones in it: its
/// spectrum is the filter's shape and nothing else. Divide the measured
/// spectrum by the filter's response and what is left is flat across the
/// passband. A resonator a few hertz wide inside the passband — the
/// whistle — stands out of this by a factor of several.
#[test]
fn the_noise_floor_has_no_tones_in_it() {
    for shape in IF_SHAPES {
        let settings = at(TrainingSettings::default(), 600.0, 500.0, shape);
        let design = FilterDesign::from_settings(&settings);
        let x = filtered_floor(SR, 0.5, &settings, 12.0);
        // 10 Hz resolution across the passband.
        let freqs: Vec<f64> = (0..=40).map(|k| 420.0 + f64::from(k) * 10.0).collect();
        let power = psd(&x, SR, &freqs, 4_800);
        let flattened: Vec<f64> = power
            .iter()
            .zip(&freqs)
            .map(|(p, f)| p / design.response_at(*f).powi(2))
            .collect();
        let mean = flattened.iter().sum::<f64>() / flattened.len() as f64;
        let worst = flattened
            .iter()
            .map(|p| (p / mean).max(mean / p))
            .fold(0.0, f64::max);
        assert!(
            worst < 1.6,
            "{shape:?}: the spectrum strays {worst:.2}x from flat — something in it is tonal"
        );
    }
}

/// The hiss is pitched where the filter is, because that is the only thing
/// giving it a pitch: its spectrum is the filter's, centred where the filter
/// is centred, with most of its power inside the passband rather than off to
/// one side.
#[test]
fn the_noise_floor_is_centred_on_the_pitch() {
    for (center, shape) in [(500.0, FilterShape::Sharp), (700.0, FilterShape::Soft)] {
        let settings = at(TrainingSettings::default(), center, 300.0, shape);
        let design = FilterDesign::from_settings(&settings);
        let x = filtered_floor(SR, 0.5, &settings, 6.0);
        let freqs: Vec<f64> = (0..=120).map(|k| 100.0 + f64::from(k) * 10.0).collect();
        let power = psd(&x, SR, &freqs, 4_800);
        let total: f64 = power.iter().sum();
        let centroid = power.iter().zip(&freqs).map(|(p, f)| p * f).sum::<f64>() / total;
        // An IF filter is symmetric in hertz, so the hiss is centred on the
        // note itself, as it is off the air — not tens of hertz above it,
        // where an audio band-pass would put it. The noise has to agree with
        // the filter, and the filter with the pitch.
        let weights: Vec<f64> = freqs
            .iter()
            .map(|f| design.response_at(*f).powi(2))
            .collect();
        let expected = weights.iter().zip(&freqs).map(|(w, f)| w * f).sum::<f64>()
            / weights.iter().sum::<f64>();
        assert!(
            (centroid - expected).abs() < 8.0,
            "{shape:?}: the hiss centred on {centroid:.0} Hz, the filter on {expected:.0} Hz"
        );
        assert!(
            (expected - center).abs() < 1.0,
            "{shape:?}: the filter itself centred on {expected:.0} Hz for a {center} Hz pitch"
        );
        assert!(
            (centroid - center).abs() < 8.0,
            "{shape:?}: the hiss centred on {centroid:.0} Hz for a {center} Hz note"
        );
        let inside: f64 = power
            .iter()
            .zip(&freqs)
            .filter(|(_, f)| (**f - center).abs() <= 150.0)
            .map(|(p, _)| p)
            .sum();
        assert!(
            inside / total > 0.75,
            "{shape:?}: only {:.0}% in the passband",
            100.0 * inside / total
        );
    }
}

/// The level control means a signal-to-noise ratio, and it means it at any
/// sample rate: through the reference filter — a sharp one 500 Hz wide at
/// 3 dB — the floor sits exactly as far under a full-level station as the
/// control says, and through any other it is what the readout on the control
/// says it is ([`heard_snr_db`] of the design's noise bandwidth).
#[test]
fn the_noise_floor_sits_at_its_signal_to_noise_ratio() {
    for sample_rate in [22_050, 44_100, 48_000, 96_000] {
        for level in [0.1, 0.5, 1.0] {
            let settings = at(
                TrainingSettings::default(),
                600.0,
                filter::reference_width_hz(),
                FilterShape::Sharp,
            );
            let x = filtered_floor(sample_rate, level, &settings, 6.0);
            let snr = 20.0 * (SIGNAL_RMS / rms(&x)).log10();
            // An eight-pole Butterworth passes a touch more noise than an
            // ideal filter of the same 3 dB width: (π/16) / sin(π/16), 0.6%.
            let enbw = (std::f64::consts::PI / 16.0) / (std::f64::consts::PI / 16.0).sin();
            let expected = noise_floor_snr_db(level) - 10.0 * enbw.log10();
            assert!(
                (snr - expected).abs() < 0.4,
                "{sample_rate} Hz, level {level}: {snr:.2} dB, wanted {expected:.2} dB"
            );
        }
    }
    for shape in ALL_SHAPES {
        for width in [FILTER_WIDTH_MIN, 300.0, 1_000.0] {
            let settings = at(TrainingSettings::default(), 600.0, width, shape);
            let design = FilterDesign::from_settings(&settings);
            let x = filtered_floor(48_000, 0.5, &settings, 6.0);
            let snr = 20.0 * (SIGNAL_RMS / rms(&x)).log10();
            let expected = heard_snr_db(0.5, design.noise_bandwidth_hz());
            assert!(
                (snr - expected).abs() < 0.4,
                "{shape:?} {width}: {snr:.2} dB, the readout says {expected:.2} dB"
            );
        }
    }
    assert_eq!(noise_floor_rms(0.0), 0.0);
    assert_eq!(noise_floor_rms(f64::NAN), 0.0);
    assert!(noise_floor_snr_db(0.5) > 15.0 && noise_floor_snr_db(0.5) < 25.0);
}

/// White noise through a narrower filter is quieter by exactly the ratio of
/// the widths. Halve the bandwidth and the hiss drops 3 dB, which is what an
/// operator hears when the narrow filter goes in.
#[test]
fn halving_the_filter_halves_the_noise_power() {
    for shape in IF_SHAPES {
        let level = |bandwidth: f64| {
            let settings = at(TrainingSettings::default(), 600.0, bandwidth, shape);
            rms(&filtered_floor(SR, 0.5, &settings, 6.0))
        };
        let drop = 20.0 * (level(500.0) / level(250.0)).log10();
        assert!(
            (drop - 3.01).abs() < 0.3,
            "{shape:?}: halving the width dropped {drop:.2} dB"
        );
    }
}

/// How often the hiss's envelope crosses its median, per second.
fn envelope_crossings(x: &[f32], center: f64) -> f64 {
    // Quadrature demodulation at the pitch, smoothed over one period of the
    // image at twice the pitch, which the boxcar nulls.
    let window = (f64::from(SR) / (2.0 * center)).round() as usize;
    let step = TAU * center / f64::from(SR);
    let (mut i_sum, mut q_sum) = (0.0, 0.0);
    let mut history = std::collections::VecDeque::with_capacity(window);
    let mut envelope = Vec::with_capacity(x.len());
    for (n, s) in x.iter().enumerate() {
        let (sin, cos) = (step * n as f64).sin_cos();
        let (i, q) = (f64::from(*s) * cos, f64::from(*s) * sin);
        i_sum += i;
        q_sum += q;
        history.push_back((i, q));
        if history.len() > window {
            let (oi, oq) = history.pop_front().unwrap_or_default();
            i_sum -= oi;
            q_sum -= oq;
        }
        envelope.push(i_sum.hypot(q_sum));
    }
    let mut sorted = envelope.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let ups = envelope
        .windows(2)
        .filter(|w| w[0] < median && w[1] >= median)
        .count();
    ups as f64 / (x.len() as f64 / f64::from(SR))
}

/// Narrowband noise wanders at a rate set by its bandwidth — roughly 0.4 B
/// times a second through its median. A 500 Hz filter's hiss churns a couple
/// of hundred times a second; close the filter and it slows, towards the
/// burble of a very narrow one. Resonators a few hertz wide wander a few
/// times a second, which the ear hears as a whistle.
#[test]
fn the_hiss_churns_at_a_rate_set_by_the_filter() {
    let rate = |bandwidth: f64| {
        let settings = at(
            TrainingSettings::default(),
            600.0,
            bandwidth,
            FilterShape::Sharp,
        );
        envelope_crossings(&filtered_floor(SR, 0.5, &settings, 6.0), 600.0)
    };
    let (wide, narrow) = (rate(500.0), rate(FILTER_WIDTH_MIN));
    assert!(
        wide > 100.0,
        "the 500 Hz hiss crossed its median only {wide:.0} times a second"
    );
    let ratio = wide / narrow;
    assert!(
        (2.0..5.0).contains(&ratio),
        "{wide:.0}/s at 500 Hz against {narrow:.0}/s at 150 Hz"
    );
}

// --------------------------------------------------------------------- static

/// QRN is lightning, not hiss: sparse and wildly uneven, which is what crest
/// factor and kurtosis both measure.
#[test]
fn static_arrives_in_crashes_rather_than_as_a_hiss() {
    for level in [0.2, 0.5, 1.0] {
        let x = background(&only_qrn(level), 8);
        assert!(
            crest(&x) > 12.0,
            "at {level} the crest was only {:.1}",
            crest(&x)
        );
        assert!(
            kurtosis(&x) > 12.0,
            "at {level} the kurtosis was only {:.1}",
            kurtosis(&x)
        );
    }
    // And uneven second to second, which a steady hiss never is.
    let x = background(&only_qrn(1.0), 8);
    let seconds: Vec<f64> = x.chunks(SR as usize).map(rms).collect();
    let loudest = seconds.iter().copied().fold(0.0, f64::max);
    let quietest = seconds.iter().copied().fold(f64::MAX, f64::min);
    assert!(
        loudest > quietest * 1.5,
        "every second sounded the same: {seconds:?}"
    );
}

#[test]
fn turning_the_static_up_brings_more_crashes_and_louder_ones() {
    let quiet = background(&only_qrn(0.2), 8);
    let loud = background(&only_qrn(1.0), 8);
    assert!(
        rms(&loud) > rms(&quiet) * 4.0,
        "quiet {} vs loud {}",
        rms(&quiet),
        rms(&loud)
    );
    assert!(peak(&loud) > peak(&quiet));
    // More of them: count ticks standing well clear of the quiet band's RMS.
    let ticks = |x: &[f32], threshold: f64| {
        x.windows(2)
            .filter(|w| f64::from(w[0].abs()) < threshold && f64::from(w[1].abs()) >= threshold)
            .count()
    };
    let threshold = rms(&quiet) * 3.0;
    assert!(ticks(&loud, threshold) > ticks(&quiet, threshold) * 5);
}

/// The static is calibrated in absolute terms, so it is the same storm at any
/// sample rate.
///
/// Judged on the crackle: how loud the busiest tenth of 10 ms stretches peaks,
/// and the median one. A minute's RMS would be the few loudest strokes that
/// happened to land in it, and swings by ±5 dB from one storm to the next at
/// the same rate; the crackle is a hundred events a second, and holds to a
/// decibel. A calibration that missed the sample rate would be 6.8 dB out
/// between these two.
#[test]
fn static_is_as_loud_at_any_sample_rate() {
    let levels = |sample_rate: u32| {
        let settings = at(only_qrn(0.8), 600.0, 500.0, FilterShape::Sharp);
        let mut source = Atmospherics::for_band(sample_rate, &settings.band, 31);
        let mut filter = receiver_for(sample_rate, &settings);
        let block = sample_rate as usize / 100;
        let mut peaks: Vec<f64> = (0..6_000)
            .map(|_| {
                (0..block)
                    .map(|_| filter.process(source.next_sample()).abs())
                    .fold(0.0, f64::max)
            })
            .collect();
        peaks.sort_by(f64::total_cmp);
        let db = |share: f64| 20.0 * peaks[((peaks.len() - 1) as f64 * share) as usize].log10();
        [db(0.5), db(0.9)]
    };
    let (low, high) = (levels(44_100), levels(96_000));
    for (low, high) in low.into_iter().zip(high) {
        assert!(
            (low - high).abs() < 2.0,
            "44.1 kHz at {low:.1} dB and 96 kHz at {high:.1} dB"
        );
    }
}

// ----------------------------------------------------------------- the mixer

/// The intensity controls have to be worth having: subtle at their defaults,
/// a hiss you copy through without effort, loud enough at the top to be the
/// reason you miss a character. A CW contact at the edge of readability is
/// around 0 dB signal-to-noise, so that is what the far end aims at — heard
/// through the 500 Hz filter the control is calibrated in, as its S/N readout
/// is.
///
/// Through any other receiver both ends move together by what its noise
/// bandwidth lets in. The default, the peaked 300 Hz filter built 353 Hz wide
/// for the default side-tone range, passes 179 Hz of noise against 503,
/// 4.5 dB less, so there the default band is about 37 dB and the loudest
/// about 6 — which is the reason to reach for a narrow filter — and the span
/// between them is the same.
#[test]
fn the_band_spans_a_useful_range_of_signal_to_noise() {
    let snr = |settings: &TrainingSettings| {
        20.0 * (SIGNAL_RMS / rms(&background(settings, 6)).max(1e-9)).log10()
    };
    let through_500_hz = |mut s: TrainingSettings| {
        s.band.filter_width_hz = filter::reference_width_hz();
        s.band.filter_shape = FilterShape::Sharp;
        s.clamp()
    };
    let mut loud = TrainingSettings::default();
    loud.band.qrn_level = 1.0;
    loud.band.noise_level = 1.0;

    let ordinary = snr(&through_500_hz(TrainingSettings::default()));
    let hard = snr(&through_500_hz(loud.clone()));
    let ordinary_heard = snr(&TrainingSettings::default().clamp());
    let hard_heard = snr(&loud.clamp());
    println!(
        "S/N through 500 Hz: {ordinary:.1} dB default, {hard:.1} dB loudest; through the \
         default receiver {ordinary_heard:.1} and {hard_heard:.1}"
    );
    assert!(
        (28.0..40.0).contains(&ordinary),
        "the default band should be present but subtle, was {ordinary:.1} dB"
    );
    assert!(
        hard < 6.0,
        "the loudest band should reach the signal, was {hard:.1} dB"
    );
    assert!(
        hard < ordinary - 12.0,
        "the range is too narrow to be worth a slider"
    );

    let default = FilterDesign::from_settings(&TrainingSettings::default());
    let narrower = 10.0 * (REFERENCE_BANDWIDTH_HZ / default.noise_bandwidth_hz()).log10();
    for (heard, wide) in [(ordinary_heard, ordinary), (hard_heard, hard)] {
        assert!(
            (heard - wide - narrower).abs() < 1.0,
            "the default receiver heard {heard:.1} dB where 500 Hz heard {wide:.1}, \
             not {narrower:.1} dB better"
        );
    }
    assert!(hard_heard < ordinary_heard - 12.0);
}

/// However loud it gets, it is caught rather than squared off — which is what
/// a receiver does with a crash, and the difference between loud static and a
/// buzz.
#[test]
fn the_loudest_band_is_limited_and_never_clipped() {
    for shape in ALL_SHAPES {
        let mut settings = TrainingSettings::default();
        settings.band.qrn_level = 1.0;
        settings.band.noise_level = 1.0;
        settings.band.filter_width_hz = FILTER_WIDTH_MAX;
        settings.band.filter_shape = shape;
        let out = background(&settings.clamp(), 6);
        assert!(
            out.iter().all(|s| s.is_finite() && s.abs() < 1.0),
            "{shape:?} hit full scale"
        );
    }
}

/// The whole point of the bandwidth control, through the whole receiver:
/// close the filter and the band gets quieter, and the AGC does not hand the
/// noise straight back.
#[test]
fn a_narrower_receiver_is_quieter_and_the_agc_does_not_undo_it() {
    let band = |bandwidth: f64| {
        let mut s = TrainingSettings::default();
        s.band.qrn_level = 0.6;
        s.band.noise_level = 0.6;
        rms(&background(&at(s, 600.0, bandwidth, FilterShape::Sharp), 6))
    };
    let quieter = 20.0 * (band(1_000.0) / band(FILTER_WIDTH_MIN)).log10();
    assert!(quieter > 6.0, "narrowing only bought {quieter:.1} dB");
}

/// A steady tone of `amplitude` at 600 Hz, `i` samples in: a station keyed
/// down, as the AGC hears it.
fn tone(amplitude: f64, i: usize) -> f64 {
    amplitude * (TAU * 600.0 * i as f64 / f64::from(SR)).sin()
}

/// A receiver's gain answers to everything reaching it, the signal included,
/// and it holds for as long as the signal does: the band sits the same
/// distance down from the start of a send to its end, then comes back.
#[test]
fn a_loud_send_rides_the_gain_down_too() {
    let settings = only_noise(0.3);
    let mut mixer = BandMixer::new(SR, &settings, 7);
    // A quarter second under a send of `amplitude`; the gain it ends on.
    let quarter = |mixer: &mut BandMixer, amplitude: f64| {
        (0..SR as usize / 4).fold(1.0, |_, i| mixer.next(tone(amplitude, i)).gain)
    };
    assert_eq!(
        quarter(&mut mixer, 0.0),
        1.0,
        "a quiet band should be left alone"
    );
    let held: Vec<f64> = (0..16).map(|_| quarter(&mut mixer, 1.0)).collect();
    assert!(
        held.iter().all(|g| *g < 0.8),
        "a send at full scale should pull the gain down: {held:?}"
    );
    let (low, high) = held
        .iter()
        .fold((f64::MAX, 0.0f64), |(l, h), g| (l.min(*g), h.max(*g)));
    assert!(
        20.0 * (high / low).log10() < 0.2,
        "the duck should hold steady under the send: {held:?}"
    );
    let mut back = 0.0;
    for _ in 0..6 {
        back = quarter(&mut mixer, 0.0);
    }
    assert!(
        back > 0.95,
        "the band should be back after the send, at {back:.2}"
    );
    // A nonsense sample is ignored rather than wedging the AGC.
    let mut mixer = BandMixer::new(SR, &settings, 7);
    for _ in 0..4_800 {
        let received = mixer.next(f64::NAN);
        assert!(received.band.is_finite() && received.gain.is_finite());
    }
}

/// The send is ducked with the band sample by sample, from its first
/// element: the gain it rides never jumps.
///
/// The native player used to hand the gain from the band to the send once a
/// callback, and the send's level back a callback late: every send began a
/// buffer unducked and then stepped down by the whole duck in one sample,
/// in the middle of its first element.
#[test]
fn the_send_rides_the_gain_without_a_step() {
    let settings = only_noise(0.3);
    let mut mixer = BandMixer::new(SR, &settings, 7);
    for _ in 0..SR / 2 {
        mixer.next(0.0);
    }
    let start = SR as usize / 2;
    let gains: Vec<f64> = (0..SR as usize)
        .map(|i| mixer.next(if i < start { 0.0 } else { tone(1.0, i) }).gain)
        .collect();
    let ducked = 20.0 * gains[gains.len() - 1].log10();
    assert!(
        ducked < -3.0,
        "the send never ducked the band: {ducked:.2} dB"
    );
    // Within a few milliseconds of the first element it is down to the duck.
    let settled = 20.0 * gains[start + SR as usize / 100].log10();
    assert!(
        (settled - ducked).abs() < 1.0,
        "10 ms into the send the gain was {settled:.2} dB, then {ducked:.2} dB"
    );
    let biggest = gains
        .windows(2)
        .map(|w| (20.0 * (w[1] / w[0]).log10()).abs())
        .fold(0.0_f64, f64::max);
    assert!(
        biggest < 0.1,
        "the gain stepped {biggest:.2} dB between two samples"
    );
}

/// A band rebuilt under a send carries the receiver's gain on rather than
/// opening it wide and ducking the station afresh.
#[test]
fn a_rebuilt_band_carries_the_gain_on() {
    let settings = only_noise(0.3);
    let mut old = BandMixer::new(SR, &settings, 7);
    let mut gain = 1.0;
    for i in 0..SR as usize / 2 {
        gain = old.next(tone(1.0, i)).gain;
    }
    assert!(
        gain < 0.8,
        "the send should have ducked the band: {gain:.2}"
    );
    let mut new = BandMixer::new(SR, &settings, 8);
    new.carry_gain_from(&old);
    let next = new.next(tone(1.0, SR as usize / 2)).gain;
    assert!(
        (20.0 * (next / gain).log10()).abs() < 0.1,
        "the new band opened the gain from {gain:.3} to {next:.3}"
    );
}

#[test]
fn a_silent_band_needs_no_background_stream_and_mixes_to_silence() {
    assert!(!quiet().band.has_background());
    assert!(only_qrn(0.3).band.has_background());
    assert!(only_noise(0.2).band.has_background());
    // Enabled at zero level is still silence.
    assert!(!only_qrn(0.0).band.has_background());
    assert!(!only_noise(0.0).band.has_background());

    let mut mixer = BandMixer::new(8_000, &quiet(), 1);
    let mut out = [1.0f32; 64];
    mixer.fill_background(&mut out);
    assert!(out.iter().all(|s| *s == 0.0));
    let mut out = [1.0f32; 64];
    source_for(8_000, &quiet(), 1).fill(&mut out);
    assert!(out.iter().all(|s| *s == 0.0));
}

#[test]
fn every_band_stays_in_range_and_makes_a_sound() {
    for shape in IF_SHAPES {
        for (noise, qrn) in [(1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.05, 0.05)] {
            let mut s = quiet();
            s.band.noise_enabled = true;
            s.band.noise_level = noise;
            s.band.qrn_enabled = true;
            s.band.qrn_level = qrn;
            s.band.filter_shape = shape;
            let mut mixer = BandMixer::new(8_000, &s, 7);
            let mut out = [0.0f32; 16_000];
            mixer.fill_background(&mut out);
            assert!(
                out.iter().all(|s| (-1.0..=1.0).contains(s)),
                "{shape:?} {noise}/{qrn} left the unit range"
            );
            assert!(
                out.iter().any(|s| *s != 0.0),
                "{shape:?} {noise}/{qrn} produced nothing"
            );
        }
    }
}

#[test]
fn a_zero_sample_rate_is_treated_as_one() {
    let mut settings = only_qrn(0.5);
    settings.band.noise_enabled = true;
    let mut mixer = BandMixer::new(0, &settings, 1);
    let mut out = [0.0f32; 8];
    mixer.fill_background(&mut out);
    assert!(out.iter().all(|s| s.is_finite()));
}

/// The browser streams the same source through its own filter nodes, so the
/// source on its own has to be the band before the receiver: pitchless, and
/// deterministic for a seed.
#[test]
fn the_source_is_the_same_band_for_the_same_seed() {
    // A busy band, so the stations are part of what has to repeat.
    let mut settings = TrainingSettings::default();
    settings.band.stations_max = crate::settings::STATIONS_MAX;
    let mut a = source_for(SR, &settings, 5);
    let mut b = source_for(SR, &settings, 5);
    let mut c = source_for(SR, &settings, 6);
    let (mut x, mut y, mut z) = (
        vec![0.0f32; 4_096],
        vec![0.0f32; 4_096],
        vec![0.0f32; 4_096],
    );
    a.fill(&mut x);
    b.fill(&mut y);
    c.fill(&mut z);
    assert_eq!(x, y);
    assert_ne!(x, z);
    // Before the filter the floor is white: neighbouring samples are
    // uncorrelated. Heard without the other stations, whose tones are
    // pitched by nature.
    let floor = TrainingSettings::default();
    let mut x = vec![0.0f32; 4_096];
    source_for(SR, &floor, 5).fill(&mut x);
    let lag1: f64 = x
        .windows(2)
        .map(|w| f64::from(w[0]) * f64::from(w[1]))
        .sum::<f64>()
        / x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
    assert!(
        lag1.abs() < 0.08,
        "the raw floor is coloured: lag-one correlation {lag1:.3}"
    );
}

// ----------------------------------------------------------- the audio stage

/// Seconds of band the audio floor is measured over, and the spectrum's
/// segment: 5.9 Hz bins at 48 kHz, averaged over some 350 segments.
const STAGE_SECONDS: usize = 30;
const STAGE_SEGMENT: usize = 8_192;
const STAGE_SEED: u64 = 0xF100;

/// The median of a [`welch`] spectrum over `low..=high` hertz, in decibels.
fn median_db(spectrum: &[f64], low: f64, high: f64) -> f64 {
    let bin = f64::from(SR) / STAGE_SEGMENT as f64;
    let mut inside: Vec<f64> = spectrum
        .iter()
        .enumerate()
        .filter(|(k, _)| (low..=high).contains(&(*k as f64 * bin)))
        .map(|(_, p)| *p)
        .collect();
    assert!(!inside.is_empty(), "no bins in {low}..{high} Hz");
    inside.sort_by(f64::total_cmp);
    10.0 * inside[inside.len() / 2].log10()
}

/// The default band with its static off: the hiss and nothing else, at the
/// filter width given.
fn hiss_only(bandwidth_hz: Option<f64>) -> TrainingSettings {
    let mut s = TrainingSettings::default();
    s.band.qrn_enabled = false;
    if let Some(bandwidth) = bandwidth_hz {
        s.band.filter_width_hz = bandwidth;
    }
    s.clamp()
}

/// `seconds` of the background through `mixer`, under a send keyed down at
/// `send`, with the first half second — the filter filling and the AGC
/// settling on the send — left out. The send rides the gain but is not part
/// of the background, so it is not in what comes back.
fn render(mixer: &mut BandMixer, send: f64, seconds: usize) -> Vec<f32> {
    let settle = SR as usize / 2;
    (0..settle + SR as usize * seconds)
        .map(|i| soft_limit(mixer.next(tone(send, i)).band) as f32)
        .skip(settle)
        .collect()
}

/// The background as heard, and what the audio floor added to it: the same
/// band rendered again, to the sample, with the floor taken out. The floor
/// joins after the AGC and the AGC never hears it, so the difference is the
/// floor exactly as it was played.
fn with_and_without_floor(settings: &TrainingSettings, send: f64, seconds: usize) -> Floored {
    let heard = render(&mut BandMixer::new(SR, settings, STAGE_SEED), send, seconds);
    let mut bare = BandMixer::new(SR, settings, STAGE_SEED);
    bare.audio_floor = AudioFloor::new(SR, 0.0, 0);
    let band = render(&mut bare, send, seconds);
    let added = heard.iter().zip(&band).map(|(a, b)| a - b).collect();
    Floored { heard, band, added }
}

struct Floored {
    heard: Vec<f32>,
    band: Vec<f32>,
    added: Vec<f32>,
}

/// A real receiver is never quite silent outside its filter: its own audio
/// stage hisses far under the band ([`FLOOR_BELOW_DB`]), from a couple of
/// hundred hertz to the top of its audio.
///
/// Measured on what the floor itself adds, against the hiss at the note,
/// where the receiver is unity, so the floor sits in the same place whatever
/// width the filter is set to: [`FLOOR_BELOW_DB`] down from 1.2 to 2.4 kHz,
/// and gone by 5 kHz. Below 200 Hz it falls away as well, so it brings no
/// rumble. Down there, and up at 1.2 kHz for a wide filter, the filter's own
/// skirt is louder than the floor, which is why the floor is measured on its
/// own. And the band stays a hiss: Gaussian.
#[test]
fn the_receiver_hisses_faintly_outside_its_filter() {
    let mut outside_at = Vec::new();
    for bandwidth in [None, Some(150.0), Some(1_000.0)] {
        let settings = hiss_only(bandwidth);
        let pitch = settings.side_tone_center();
        let width = settings.band.filter_width_hz;
        let floored = with_and_without_floor(&settings, 0.0, STAGE_SECONDS);
        let spectrum = welch(&floored.heard, STAGE_SEGMENT);
        let added = welch(&floored.added, STAGE_SEGMENT);
        let at_pitch = median_db(&spectrum, pitch - 20.0, pitch + 20.0);
        let outside = median_db(&added, 1_200.0, 2_400.0) - at_pitch;
        let above = median_db(&spectrum, 5_000.0, 10_000.0) - at_pitch;
        let below = median_db(&added, 100.0, 150.0) - at_pitch;
        let k = kurtosis(&floored.heard);
        let report = format!(
            "{width} Hz wide: {outside:.2} dB at 1.2-2.4 kHz, {above:.2} dB at 5-10 kHz, \
             {below:.2} dB added at 100-150 Hz, kurtosis {k:.3}"
        );
        assert!((outside + FLOOR_BELOW_DB).abs() < 1.5, "{report}");
        assert!(above < -50.0, "{report}");
        assert!(below < -FLOOR_BELOW_DB - 6.0, "{report}");
        assert!((k - 3.0).abs() < 0.1, "{report}");
        outside_at.push(outside);
    }
    // Opening or closing the filter leaves the floor where it was.
    for outside in &outside_at[1..] {
        assert!(
            (outside - outside_at[0]).abs() < 1.0,
            "{outside_at:.2?} dB across 500, 150 and 1000 Hz"
        );
    }
}

/// A narrow filter leaves no wideband hiss to hear. The floor spreads over
/// the whole of the audio stage where the hiss comes through the filter's
/// width alone, and with a station keyed the AGC turns the hiss down and
/// never the floor; even so, at the narrowest filter, in every shape, its
/// power stays more than 35 dB under the hiss's, and 45 between stations.
#[test]
fn a_narrow_filter_leaves_no_wideband_hiss_to_hear() {
    for shape in ALL_SHAPES {
        let mut settings = hiss_only(Some(FILTER_WIDTH_MIN));
        settings.band.filter_shape = shape;
        let settings = settings.clamp();
        let knee = knee_for(&settings).expect("the AGC is on by default");
        for (when, send, under_db) in [("idle", 0.0, 45.0), ("keyed", 10.0 * knee, 35.0)] {
            let floored = with_and_without_floor(&settings, send, 5);
            let power = |v: &[f32]| v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            let floor_db = 10.0 * (power(&floored.added) / power(&floored.band)).log10();
            assert!(
                floor_db < -under_db,
                "{shape:?} {when}: the floor is {floor_db:.1} dB against the hiss"
            );
        }
    }
}

/// The floor follows the noise control, so turning the noise off turns the
/// receiver off with it: not a trace of the floor under the static.
#[test]
fn the_audio_floor_goes_with_the_noise() {
    let mut off = only_qrn(0.6);
    off.band.noise_level = 0.5;
    for settings in [off, only_noise(0.0)] {
        let floored = with_and_without_floor(&settings, 0.0, 5);
        assert_eq!(floored.heard, floored.band);
    }
    let floored = with_and_without_floor(&only_qrn(0.6), 0.0, 5);
    assert!(
        floored.heard.iter().any(|s| *s != 0.0),
        "the static should still be there"
    );
}

/// The largest normalised cross-correlation of `x` and `y`, at any lag.
fn cross_correlation_peak(x: &[f32], y: &[f32]) -> f64 {
    let n = (x.len() + y.len()).next_power_of_two();
    let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
    for (slot, v) in re.iter_mut().zip(x) {
        *slot = f64::from(*v);
    }
    for (slot, v) in im.iter_mut().zip(y) {
        *slot = f64::from(*v);
    }
    // Both spectra from one transform: X is the even part of Z, jY the odd.
    let fft = Fft::new(n);
    fft.run(&mut re, &mut im);
    for k in 0..=n / 2 {
        let m = (n - k) % n;
        let (zr, zi, wr, wi) = (re[k], im[k], re[m], -im[m]);
        let (xr, xi) = ((zr + wr) / 2.0, (zi + wi) / 2.0);
        let (yr, yi) = ((zi - wi) / 2.0, -(zr - wr) / 2.0);
        // The cross-spectrum conj(X)·Y, which at n − k is its own conjugate.
        let (cr, ci) = (xr * yr + xi * yi, xr * yi - xi * yr);
        // Conjugated, so the forward transform does for the inverse.
        (re[k], im[k]) = (cr, -ci);
        (re[m], im[m]) = (cr, ci);
    }
    fft.run(&mut re, &mut im);
    let energy = |v: &[f32]| v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
    let scale = n as f64 * (energy(x) * energy(y)).sqrt();
    re.iter().map(|r| (r / scale).abs()).fold(0.0, f64::max)
}

/// The floor is noise of its own, not the band's noise over again: anything
/// shared between them would be a second copy of the hiss, filtered wide,
/// and would show as a peak at some lag.
#[test]
fn the_audio_floor_is_its_own_noise() {
    let floored = with_and_without_floor(&hiss_only(None), 0.0, STAGE_SECONDS);
    let peak = cross_correlation_peak(&floored.band, &floored.added);
    assert!(peak < 0.02, "the floor and the band correlate to {peak:.4}");
    // And the measure would see the band in it, even at 40 dB down.
    let shared: Vec<f32> = floored
        .band
        .iter()
        .zip(&floored.added)
        .map(|(b, a)| 0.01 * b + a)
        .collect();
    let seen = cross_correlation_peak(&floored.band, &shared);
    assert!(seen > 0.1, "a shared noise correlates to only {seen:.4}");
}

/// The AGC turns the receiver down ahead of the audio stage, so under a
/// strong station the hiss at the note drops by the AGC's duck while the
/// floor outside the filter stays exactly where it was — as on a real
/// receiver, where the audio stage's hiss is the one thing the AGC never
/// touches.
#[test]
fn the_agc_ducks_the_hiss_but_not_the_audio_floor() {
    let settings = hiss_only(None);
    let pitch = settings.side_tone_center();
    let knee = knee_for(&settings).expect("the AGC is on by default");
    let quiet = render(&mut BandMixer::new(SR, &settings, STAGE_SEED), 0.0, 10);
    let mut mixer = BandMixer::new(SR, &settings, STAGE_SEED);
    let under = render(&mut mixer, 10.0 * knee, 10);
    // The gain over the next 10 ms, through which a tone's own ripple in the
    // detector averages out.
    let samples = SR as usize / 100;
    let mean = (0..samples)
        .map(|i| mixer.next(tone(10.0 * knee, i)).gain)
        .sum::<f64>()
        / samples as f64;
    let duck = 20.0 * mean.log10();
    // Ten times the knee is 20 dB over it, which the 2:1 law takes 10 dB off.
    assert!((duck - agc_law_db(20.0)).abs() < 0.2, "ducked {duck:.2} dB");
    let (quiet, under) = (welch(&quiet, STAGE_SEGMENT), welch(&under, STAGE_SEGMENT));
    let change = |low: f64, high: f64| median_db(&under, low, high) - median_db(&quiet, low, high);
    let hiss = change(pitch - 20.0, pitch + 20.0);
    let floor = change(1_500.0, 2_400.0);
    let report = format!("AGC {duck:.2} dB: hiss {hiss:.2} dB, floor {floor:.2} dB");
    assert!((hiss - duck).abs() < 0.5, "{report}");
    assert!(floor.abs() < 1.0, "{report}");
}
