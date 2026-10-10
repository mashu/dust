//! What the band has to sound like, measured.
//!
//! Most of these listen the way an engineer would: render a few seconds,
//! then look at the level, the spectrum and the statistics. They pin the
//! properties that make a receiver sound like a receiver — and the ones whose
//! absence made the old model sound like the wind.

use std::f64::consts::{SQRT_2, TAU};

use super::*;
use crate::settings::{FILTER_BANDWIDTH_MAX, FILTER_BANDWIDTH_MIN, FilterShape};

const SR: u32 = 48_000;
const SHAPES: [FilterShape; 2] = [FilterShape::Sharp, FilterShape::Soft];

fn background(settings: &TrainingSettings, seconds: usize) -> Vec<f32> {
    let mut mixer = BandMixer::new(SR, settings, 9);
    let mut out = vec![0.0f32; SR as usize * seconds];
    mixer.fill_background(&mut out);
    out
}

fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0f32, |a, s| a.max(s.abs()))
}

fn rms(v: &[f32]) -> f32 {
    let sum: f64 = v.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / v.len().max(1) as f64).sqrt() as f32
}

/// Peak over RMS. Gaussian noise sits near 4; static pushes it far higher,
/// which is the whole difference between a hiss and a band with lightning on
/// it.
fn crest(v: &[f32]) -> f32 {
    let r = rms(v);
    if r > 0.0 { peak(v) / r } else { 0.0 }
}

/// Fourth moment over the squared second: 3 for a Gaussian, far more for
/// anything impulsive.
fn kurtosis(v: &[f32]) -> f64 {
    let n = v.len() as f64;
    let mean = v.iter().map(|s| f64::from(*s)).sum::<f64>() / n;
    let m2 = v
        .iter()
        .map(|s| (f64::from(*s) - mean).powi(2))
        .sum::<f64>()
        / n;
    let m4 = v
        .iter()
        .map(|s| (f64::from(*s) - mean).powi(4))
        .sum::<f64>()
        / n;
    m4 / (m2 * m2)
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
    s.band.filter_bandwidth_hz = bandwidth;
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
    let mut filter = ReceiverFilter::from_settings(sample_rate, settings);
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

// ---------------------------------------------------------------- the filter

/// Steady-state gain of the running filter for a tone at `hz`.
fn measured(sample_rate: u32, design: &FilterDesign, hz: f64) -> f64 {
    let mut filter = ReceiverFilter::new(sample_rate, design);
    let step = TAU * hz / f64::from(sample_rate);
    let settle = sample_rate as usize / 2;
    let measure = sample_rate as usize / 4;
    let mut sum_sq = 0.0;
    for n in 0..settle + measure {
        let out = filter.process((step * n as f64).sin());
        if n >= settle {
            sum_sq += out * out;
        }
    }
    (sum_sq / measure as f64).sqrt() * SQRT_2
}

/// The control has to mean what it says: asked for so many hertz between the
/// 3 dB points, that is what you get, sharp or soft.
#[test]
fn the_bandwidth_control_is_in_hertz_and_lands_where_it_says() {
    for shape in SHAPES {
        for asked in [
            FILTER_BANDWIDTH_MIN,
            250.0,
            500.0,
            1_000.0,
            FILTER_BANDWIDTH_MAX,
        ] {
            let design = FilterDesign::new(600.0, asked, shape);
            let edge = |direction: f64| {
                let mut hz = 600.0;
                while design.response_at(hz) > 1.0 / SQRT_2 && hz > 1.0 {
                    hz += direction * 0.25;
                }
                hz
            };
            let (low, high) = (edge(-1.0), edge(1.0));
            let error = ((high - low) - asked).abs() / asked;
            assert!(
                error < 0.01,
                "{shape:?}: asked for {asked} Hz and got {:.1} Hz",
                high - low
            );
            // Geometric symmetry: the edges multiply to the centre squared.
            assert!(((low * high).sqrt() - 600.0).abs() < 1.0);
        }
    }
}

/// The response the design reports has to be the response the audio has, or
/// a display that draws it is decoration. Measure the real filter and compare.
#[test]
fn the_design_response_is_the_filter_you_hear() {
    for shape in SHAPES {
        for (center, bandwidth) in [
            (500.0, 500.0),
            (600.0, 250.0),
            (700.0, 1_200.0),
            (500.0, 150.0),
        ] {
            let design = FilterDesign::new(center, bandwidth, shape);
            for offset in [-400.0, -200.0, -80.0, -30.0, 0.0, 30.0, 80.0, 200.0, 400.0] {
                let hz = center + offset;
                let drawn = design.response_at(hz);
                let real = measured(SR, &design, hz);
                // Each section is pre-warped at its own centre. Across a
                // passband narrower than its own centre frequency — every CW
                // setting — that is exact to a percent; wider than that, the
                // sections are far enough apart for a few percent to show.
                let tolerance = if bandwidth <= center { 0.01 } else { 0.04 };
                assert!(
                    (drawn - real).abs() < tolerance,
                    "{shape:?} {center}/{bandwidth} at {hz} Hz: design {drawn:.4}, measured {real:.4}"
                );
            }
        }
    }
}

/// A band-pass is a band-pass: unity where it is tuned, falling away either
/// side however wide it is set — never a bump off to one side, which is what
/// a station sitting off-centre would otherwise be rewarded by.
#[test]
fn the_response_peaks_where_the_filter_is_tuned() {
    for shape in SHAPES {
        for bandwidth in [FILTER_BANDWIDTH_MIN, 500.0, FILTER_BANDWIDTH_MAX] {
            let design = FilterDesign::new(600.0, bandwidth, shape);
            assert!((design.response_at(600.0) - 1.0).abs() < 1e-9);
            // Monotonic in the band-pass frequency variable, either side.
            let mut previous = (1.0, 1.0);
            for step in 1..200 {
                let ratio = 1.0 + f64::from(step) * 0.02;
                let above = design.response_at(600.0 * ratio);
                let below = design.response_at(600.0 / ratio);
                assert!(above <= previous.0 + 1e-12 && below <= previous.1 + 1e-12);
                assert!(
                    (above - below).abs() < 1e-9,
                    "{shape:?} {bandwidth}: not geometrically symmetric at ×{ratio}"
                );
                previous = (above, below);
            }
        }
    }
}

/// The two prototypes are what they claim to be. Butterworth is maximally
/// flat — |H|² = 1/(1+Ω¹⁶) for eight poles — and the soft one is the Bessel
/// polynomial, 3 dB down at the band edge.
#[test]
fn the_prototypes_are_butterworth_and_bessel() {
    let (center, bandwidth) = (600.0, 300.0);
    let sharp = FilterDesign::new(center, bandwidth, FilterShape::Sharp);
    for hz in [300.0, 450.0, 500.0, 600.0, 700.0, 760.0, 900.0, 1_400.0] {
        let omega: f64 = (hz / center - center / hz) * center / bandwidth;
        let ideal = 1.0 / (1.0 + omega.powi(2 * RECEIVER_SECTIONS as i32)).sqrt();
        assert!(
            (sharp.response_at(hz) - ideal).abs() < 1e-9,
            "Butterworth at {hz} Hz: {} vs {ideal}",
            sharp.response_at(hz)
        );
    }

    // Bessel: |H(jΩ)|² = θ(0)² / |θ(jΩ·ωc)|², with θ the reverse Bessel
    // polynomial and ωc its 3 dB frequency.
    const THETA: [f64; 9] = [
        2_027_025.0,
        2_027_025.0,
        945_945.0,
        270_270.0,
        51_975.0,
        6_930.0,
        630.0,
        36.0,
        1.0,
    ];
    const OMEGA_C: f64 = 3.179_617_237_511;
    let soft = FilterDesign::new(center, bandwidth, FilterShape::Soft);
    for hz in [300.0, 450.0, 500.0, 600.0, 700.0, 760.0, 900.0, 1_400.0] {
        let omega = (hz / center - center / hz) * center / bandwidth * OMEGA_C;
        // θ(jΩ): even powers are real with alternating signs, odd imaginary.
        let (mut re, mut im) = (0.0, 0.0);
        for (k, c) in THETA.iter().enumerate() {
            let term = c * omega.powi(k as i32);
            match k % 4 {
                0 => re += term,
                1 => im += term,
                2 => re -= term,
                _ => im -= term,
            }
        }
        let ideal = THETA[0] / re.hypot(im);
        assert!(
            (soft.response_at(hz) - ideal).abs() < 1e-6,
            "Bessel at {hz} Hz: {} vs {ideal}",
            soft.response_at(hz)
        );
    }
}

/// Each section is the "band-pass, 0 dB peak" biquad of the Audio EQ
/// Cookbook — the very filter a Web Audio `BiquadFilterNode` of type
/// `bandpass` runs — so the browser can build this receiver out of nodes and
/// get the same sound. Checked against the cookbook's own coefficients.
#[test]
fn a_section_is_the_browsers_band_pass_biquad() {
    for (f0, q) in [(450.0, 0.8), (600.0, 4.0), (712.0, 17.0)] {
        let design = FilterDesign {
            sections: [Section { center_hz: f0, q }; RECEIVER_SECTIONS],
            gain: 1.0,
        };
        let w0 = TAU * f0 / f64::from(SR);
        let alpha = w0.sin() / (2.0 * q);
        let (b0, b2) = (alpha, -alpha);
        let (a0, a1, a2) = (1.0 + alpha, -2.0 * w0.cos(), 1.0 - alpha);
        for hz in [200.0, 440.0, f0, 800.0, 1_500.0] {
            let w = TAU * hz / f64::from(SR);
            // H(e^jw), one section.
            let num = (b0 + b2 * (2.0 * w).cos(), -b2 * (2.0 * w).sin());
            let den = (
                a0 + a1 * w.cos() + a2 * (2.0 * w).cos(),
                -a1 * w.sin() - a2 * (2.0 * w).sin(),
            );
            let section = num.0.hypot(num.1) / den.0.hypot(den.1);
            let cascade = section.powi(RECEIVER_SECTIONS as i32);
            let real = measured(SR, &design, hz);
            assert!(
                (cascade - real).abs() < 1e-3 * cascade.max(1e-3),
                "{f0} Hz Q {q} at {hz} Hz: cookbook {cascade:.6}, ours {real:.6}"
            );
        }
    }
}

/// Sharp is steep: between its 6 dB and 60 dB widths, about the factor of two
/// a good eight-pole crystal filter has. Soft rolls off more gently, which is
/// the point of it — but neither is the wide-skirted mush of a single tuned
/// circuit.
#[test]
fn sharp_has_crystal_filter_skirts_and_soft_is_gentler() {
    let shape_factor = |shape| {
        let design = FilterDesign::new(600.0, 400.0, shape);
        let width_at = |db: f64| {
            let target = 10f64.powf(-db / 20.0);
            let edge = |direction: f64| {
                let mut hz = 600.0;
                while design.response_at(hz) > target && hz > 1.0 {
                    hz += direction * 0.5;
                }
                hz
            };
            edge(1.0) - edge(-1.0)
        };
        width_at(60.0) / width_at(6.0)
    };
    let sharp = shape_factor(FilterShape::Sharp);
    let soft = shape_factor(FilterShape::Soft);
    assert!(
        (1.8..2.8).contains(&sharp),
        "a sharp CW filter should have a shape factor near 2, got {sharp:.2}"
    );
    assert!(soft > sharp * 1.2, "soft {soft:.2} vs sharp {sharp:.2}");
    assert!(soft < 6.0, "even soft is eight poles: {soft:.2}");
}

/// How long the filter keeps sounding after the key comes up: milliseconds
/// for the envelope to fall 40 dB below where it was keyed down.
fn ring_ms(bandwidth: f64, shape: FilterShape) -> f64 {
    let design = FilterDesign::new(600.0, bandwidth, shape);
    let mut filter = ReceiverFilter::new(SR, &design);
    let step = TAU * 600.0 / f64::from(SR);
    for n in 0..SR as usize / 2 {
        filter.process((step * n as f64).sin());
    }
    // Envelope over one cycle of the tone at a time.
    let cycle = (f64::from(SR) / 600.0).round() as usize;
    let mut blocks = 0;
    loop {
        let block: Vec<f32> = (0..cycle).map(|_| filter.process(0.0) as f32).collect();
        blocks += 1;
        if f64::from(peak(&block)) < 0.01 || blocks > 2_000 {
            return blocks as f64 * cycle as f64 / f64::from(SR) * 1000.0;
        }
    }
}

/// Ringing is the filter, and it follows the filter: narrower rings longer,
/// and the sharp shape rings longer than the soft one at the same width —
/// which is the whole reason a rig offers both.
#[test]
fn narrow_and_sharp_filters_ring_longer() {
    for shape in SHAPES {
        let wide = ring_ms(500.0, shape);
        let narrow = ring_ms(FILTER_BANDWIDTH_MIN, shape);
        // Not the full ratio of the widths: 500 Hz around a 600 Hz pitch is
        // too wide a fraction of it to be a scaled copy of the narrow one.
        assert!(
            narrow > wide * 1.4,
            "{shape:?}: {narrow:.1} ms at 150 Hz against {wide:.1} ms at 500 Hz"
        );
    }
    for bandwidth in [FILTER_BANDWIDTH_MIN, 250.0, 500.0] {
        let sharp = ring_ms(bandwidth, FilterShape::Sharp);
        let soft = ring_ms(bandwidth, FilterShape::Soft);
        assert!(
            sharp > soft * 1.3,
            "at {bandwidth} Hz sharp rang {sharp:.1} ms and soft {soft:.1} ms"
        );
    }
    // And in absolute terms it is the ringing of a CW filter — tens of
    // milliseconds at its narrowest — not the hundreds a resonator a few
    // hertz wide rings for, which is a whistle.
    let longest = ring_ms(FILTER_BANDWIDTH_MIN, FilterShape::Sharp);
    assert!(
        (10.0..80.0).contains(&longest),
        "the narrowest sharp filter rang {longest:.1} ms"
    );
}

/// Overshoot is what ringing looks like on a keyed tone: a sharp filter's
/// envelope swings past the steady level when a dit starts, a soft one's
/// barely does.
#[test]
fn a_sharp_filter_overshoots_a_keyed_tone_and_a_soft_one_does_not() {
    let overshoot = |shape| {
        let design = FilterDesign::new(600.0, 250.0, shape);
        let mut filter = ReceiverFilter::new(SR, &design);
        let step = TAU * 600.0 / f64::from(SR);
        let out: Vec<f32> = (0..SR as usize / 4)
            .map(|n| filter.process((step * n as f64).sin()) as f32)
            .collect();
        let steady = peak(&out[out.len() / 2..]);
        peak(&out) / steady
    };
    let sharp = overshoot(FilterShape::Sharp);
    let soft = overshoot(FilterShape::Soft);
    assert!(sharp > 1.08, "sharp overshot by only {sharp:.3}");
    assert!(soft < 1.03, "soft overshot by {soft:.3}");
}

/// Narrower means narrower: at a fixed distance off the centre, squeezing the
/// filter always passes less of what is beside you.
#[test]
fn narrowing_the_filter_passes_less_of_what_is_beside_you() {
    for shape in SHAPES {
        for offset in [100.0, 200.0, 400.0] {
            let wide = FilterDesign::new(600.0, 1_000.0, shape).response_at(600.0 + offset);
            let narrow = FilterDesign::new(600.0, 200.0, shape).response_at(600.0 + offset);
            assert!(narrow < wide, "{shape:?} {offset} Hz off");
        }
    }
}

/// The filter is the answer to a pile-up as much as to the noise: a station
/// off your pitch falls away as you narrow it, while the one you are on is
/// left alone.
#[test]
fn narrowing_the_receiver_pushes_an_interfering_station_down() {
    let offset = crate::settings::BandSettings::default().pileup_spread_hz;
    for shape in SHAPES {
        let rejection = |bandwidth: f64| {
            let design = FilterDesign::new(500.0, bandwidth, shape);
            20.0 * (design.response_at(500.0 + offset) / design.response_at(500.0)).log10()
        };
        assert!(rejection(FILTER_BANDWIDTH_MIN) < rejection(1_000.0) - 20.0);
        assert!(
            20.0 * measured(
                SR,
                &FilterDesign::new(500.0, FILTER_BANDWIDTH_MIN, shape),
                500.0
            )
            .log10()
                > -0.5
        );
    }
}

/// Nonsense in, something finite out. No sample rate, no width, no centre:
/// the receiver still produces numbers rather than poisoning the stream.
#[test]
fn a_degenerate_filter_still_produces_numbers() {
    for shape in SHAPES {
        for (center, bandwidth) in [(f64::NAN, 500.0), (500.0, f64::NAN), (0.0, 0.0), (1e9, 1e9)] {
            let design = FilterDesign::new(center, bandwidth, shape);
            assert!(design.gain().is_finite() && design.gain() > 0.0);
            for sample_rate in [0, 1, 8, 8_000, 192_000] {
                let mut filter = ReceiverFilter::new(sample_rate, &design);
                let mut out = [1.0f32; 64];
                filter.apply(&mut out);
                assert!(
                    out.iter().all(|s| s.is_finite()),
                    "{center}/{bandwidth} at {sample_rate}"
                );
            }
        }
    }
}

// ------------------------------------------------------------ the noise floor

/// The floor is Gaussian noise through the filter. Its sample values are
/// normally distributed — kurtosis three — and so its envelope is Rayleigh:
/// the restless, grainy hiss of a real receiver. Impulsive excitation, which
/// is what the old model drove its resonators with, fails both.
#[test]
fn the_noise_floor_is_gaussian() {
    for shape in SHAPES {
        let settings = at(TrainingSettings::default(), 600.0, 500.0, shape);
        let x = filtered_floor(SR, 0.5, &settings, 8.0);
        let k = kurtosis(&x);
        assert!((2.85..3.15).contains(&k), "{shape:?}: kurtosis {k:.3}");
        // RMS over mean magnitude is √(π/2) for a Gaussian.
        let mean_abs = x.iter().map(|s| f64::from(s.abs())).sum::<f64>() / x.len() as f64;
        let ratio = f64::from(rms(&x)) / mean_abs;
        assert!(
            (ratio - 1.2533).abs() < 0.01,
            "{shape:?}: rms/mean {ratio:.4}"
        );
    }
}

/// The regression test for the whistle. Band noise has no tones in it: its
/// spectrum is the filter's shape and nothing else. Divide the measured
/// spectrum by the filter's response and what is left is flat across the
/// passband. A resonator a few hertz wide inside the passband — the old
/// receiver model had three, sweeping about — stands out of this by a factor
/// of several.
#[test]
fn the_noise_floor_has_no_tones_in_it() {
    for shape in SHAPES {
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
        // A band-pass at audio frequencies is geometrically symmetric, so the
        // power sits a little above the pitch in hertz. Where exactly is the
        // filter's business, and the noise has to agree with it.
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
            (expected - center).abs() < 0.1 * center,
            "{shape:?}: the filter itself centred on {expected:.0} Hz for a {center} Hz pitch"
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
/// sample rate: through a 500 Hz filter the floor sits exactly as far under a
/// full-level station as the control says.
#[test]
fn the_noise_floor_sits_at_its_signal_to_noise_ratio() {
    for sample_rate in [22_050, 44_100, 48_000, 96_000] {
        for level in [0.1, 0.5, 1.0] {
            let settings = at(
                TrainingSettings::default(),
                600.0,
                500.0,
                FilterShape::Sharp,
            );
            let x = filtered_floor(sample_rate, level, &settings, 6.0);
            let snr = 20.0 * (SIGNAL_RMS / f64::from(rms(&x))).log10();
            // An eight-pole Butterworth passes a touch more noise than an
            // ideal filter of the same 3 dB width.
            let expected = noise_floor_snr_db(level) - 10.0 * (1.026f64).log10();
            assert!(
                (snr - expected).abs() < 0.4,
                "{sample_rate} Hz, level {level}: {snr:.2} dB, wanted {expected:.2} dB"
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
    for shape in SHAPES {
        let level = |bandwidth: f64| {
            let settings = at(TrainingSettings::default(), 600.0, bandwidth, shape);
            f64::from(rms(&filtered_floor(SR, 0.5, &settings, 6.0)))
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
/// burble of a very narrow one. The old resonators were a few hertz wide and
/// wandered a few times a second, which the ear hears as a whistle.
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
    let (wide, narrow) = (rate(500.0), rate(FILTER_BANDWIDTH_MIN));
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
    let seconds: Vec<f32> = x.chunks(SR as usize).map(rms).collect();
    let loudest = seconds.iter().copied().fold(0.0f32, f32::max);
    let quietest = seconds.iter().copied().fold(f32::MAX, f32::min);
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
    let ticks = |x: &[f32], threshold: f32| {
        x.windows(2)
            .filter(|w| w[0].abs() < threshold && w[1].abs() >= threshold)
            .count()
    };
    let threshold = rms(&quiet) * 3.0;
    assert!(ticks(&loud, threshold) > ticks(&quiet, threshold) * 5);
}

/// A sferic is an impulse by the time it arrives; what you hear is your own
/// filter ringing, so its length is the filter's. Close the filter and the
/// ticks of static stretch into pings — the familiar sound of a narrow filter
/// on a noisy band.
#[test]
fn the_filter_decides_how_long_a_crash_rings() {
    let tail_ms = |bandwidth: f64| {
        let design = FilterDesign::new(600.0, bandwidth, FilterShape::Sharp);
        let mut filter = ReceiverFilter::new(SR, &design);
        let response: Vec<f64> = (0..SR as usize / 2)
            .map(|n| filter.process(if n == 0 { 1.0 } else { 0.0 }).abs())
            .collect();
        let top = response.iter().copied().fold(0.0, f64::max);
        let last = response.iter().rposition(|v| *v > top / 100.0).unwrap_or(0);
        last as f64 * 1000.0 / f64::from(SR)
    };
    assert!(tail_ms(FILTER_BANDWIDTH_MIN) > tail_ms(500.0) * 2.5);
}

/// The static is calibrated in absolute terms, so it is the same storm at any
/// sample rate.
#[test]
fn static_is_as_loud_at_any_sample_rate() {
    let level = |sample_rate: u32| {
        let settings = at(only_qrn(0.8), 600.0, 500.0, FilterShape::Sharp);
        let mut source = Atmospherics::from_settings(sample_rate, &settings, 31);
        let mut filter = ReceiverFilter::from_settings(sample_rate, &settings);
        let x: Vec<f32> = (0..sample_rate as usize * 60)
            .map(|_| filter.process(source.next_sample()) as f32)
            .collect();
        f64::from(rms(&x))
    };
    let (low, high) = (level(44_100), level(96_000));
    let spread = 20.0 * (low / high).log10();
    assert!(
        spread.abs() < 2.0,
        "44.1 kHz and 96 kHz differ by {spread:.1} dB"
    );
}

// ----------------------------------------------------------------- the mixer

/// The intensity controls have to be worth having: quiet enough at their
/// defaults to copy through comfortably, loud enough at the top to be the
/// reason you miss a character. A CW contact at the edge of readability is
/// around 0 dB signal-to-noise, so that is what the far end aims at.
#[test]
fn the_band_spans_a_useful_range_of_signal_to_noise() {
    let signal = SIGNAL_RMS as f32;
    let snr = |settings: &TrainingSettings| {
        20.0 * (signal / rms(&background(settings, 6)).max(1e-9)).log10()
    };

    let ordinary = snr(&TrainingSettings::default().clamp());
    assert!(
        (14.0..26.0).contains(&ordinary),
        "the default band should be present but easy, was {ordinary:.1} dB"
    );

    let mut loud = TrainingSettings::default();
    loud.band.qrn_level = 1.0;
    loud.band.noise_level = 1.0;
    let hard = snr(&loud.clamp());
    assert!(
        hard < 6.0,
        "the loudest band should reach the signal, was {hard:.1} dB"
    );
    assert!(
        hard < ordinary - 12.0,
        "the range is too narrow to be worth a slider"
    );
}

/// However loud it gets, it is caught rather than squared off — which is what
/// a receiver does with a crash, and the difference between loud static and a
/// buzz.
#[test]
fn the_loudest_band_is_limited_and_never_clipped() {
    for shape in SHAPES {
        let mut settings = TrainingSettings::default();
        settings.band.qrn_level = 1.0;
        settings.band.noise_level = 1.0;
        settings.band.filter_bandwidth_hz = FILTER_BANDWIDTH_MAX;
        settings.band.filter_shape = shape;
        let out = background(&settings.clamp(), 6);
        assert!(
            out.iter().all(|s| s.is_finite() && s.abs() < 1.0),
            "{shape:?} hit full scale"
        );
    }
    assert_eq!(soft_limit(0.0), 0.0);
    assert_eq!(soft_limit(0.5), 0.5, "below the knee it does nothing");
    assert_eq!(soft_limit(-0.5), -0.5);
    assert!(soft_limit(4.0) < 1.0 && soft_limit(4.0) > 0.9);
    assert!(soft_limit(-4.0) > -1.0 && soft_limit(-4.0) < -0.9);
    assert!(
        soft_limit(2.0) < soft_limit(3.0),
        "limiting, not clipping flat"
    );
}

/// The browser limits with a lookup table of the same curve, so the table
/// has to be the curve: identical below the knee, never past full scale,
/// always rising.
#[test]
fn the_soft_limit_table_is_the_soft_limit() {
    let curve = soft_limit_curve(4_097);
    assert_eq!(curve.len(), 4_097);
    assert!(
        curve.windows(2).all(|w| w[1] >= w[0]),
        "it must rise everywhere"
    );
    // At most full scale: the far ends round to exactly 1.0 in f32.
    assert!(curve.iter().all(|v| v.abs() <= 1.0));
    // The midpoint is silence, and the ends are as loud as it gets.
    assert_eq!(curve[2_048], 0.0);
    assert!(curve[4_096] > 0.99 && curve[0] < -0.99);
    // Below the knee it is the identity, once the headroom scaling is undone.
    for (i, value) in curve.iter().enumerate() {
        let x = (2.0 * i as f64 / 4_096.0 - 1.0) * SOFT_LIMIT_HEADROOM;
        if x.abs() <= 0.7 {
            assert!((f64::from(*value) - x).abs() < 1e-6, "at {x}");
        }
    }
    assert_eq!(soft_limit_curve(0).len(), 2);
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
    let quieter = 20.0 * f64::from(band(1_000.0) / band(FILTER_BANDWIDTH_MIN)).log10();
    assert!(quieter > 6.0, "narrowing only bought {quieter:.1} dB");
}

/// The browser's compressor is pointed at this number, so it has to fall with
/// the filter. If it did not, closing the receiver down would lower the noise
/// and the compressor would push it straight back up.
#[test]
fn the_measured_floor_falls_with_the_filter() {
    let mut s = TrainingSettings::default();
    s.band.qrn_level = 0.6;
    s.band.noise_level = 0.6;
    let floor_at = |bandwidth: f64| {
        let mut s = s.clone();
        s.band.filter_bandwidth_hz = bandwidth;
        band_standing_level(SR, &s.clamp())
    };
    let (wide, narrow) = (floor_at(1_000.0), floor_at(FILTER_BANDWIDTH_MIN));
    assert!(wide > 0.0 && narrow > 0.0, "the band measured as silent");
    assert!(
        wide > narrow * 1.4,
        "the floor barely moved: {wide:.5} against {narrow:.5}"
    );
}

/// The browser measures the floor at a low sample rate to keep the main
/// thread free, which is only sound because the floor does not depend on it.
#[test]
fn the_measured_floor_does_not_depend_on_the_sample_rate() {
    for shape in SHAPES {
        let mut s = TrainingSettings::default();
        s.band.filter_shape = shape;
        s.band.noise_level = 0.6;
        let s = s.clamp();
        let low = band_standing_level(16_000, &s);
        let high = band_standing_level(48_000, &s);
        let spread = 20.0 * (low / high).log10();
        assert!(
            spread.abs() < 1.5,
            "{shape:?}: 16 kHz and 48 kHz floors differ by {spread:.2} dB"
        );
    }
}

/// Extreme corners of the controls still make a filter a browser can build:
/// every section inside the audio band with a sane Q, and a make-up gain that
/// is large but finite.
#[test]
fn every_corner_of_the_controls_makes_a_buildable_filter() {
    for shape in SHAPES {
        for center in [200.0, 500.0, 1_200.0] {
            for bandwidth in [FILTER_BANDWIDTH_MIN, 500.0, FILTER_BANDWIDTH_MAX] {
                let design = FilterDesign::new(center, bandwidth, shape);
                assert!(
                    design.gain().is_finite() && design.gain() < 1e30,
                    "{shape:?} {center}/{bandwidth}: gain {}",
                    design.gain()
                );
                for section in design.sections() {
                    // Below the Nyquist frequency of any rate a sound card
                    // or browser runs at, 22.05 kHz and up.
                    assert!(
                        (20.0..10_000.0).contains(&section.center_hz),
                        "{shape:?} {center}/{bandwidth}: {section:?}"
                    );
                    assert!(
                        (0.05..60.0).contains(&section.q),
                        "{shape:?} {center}/{bandwidth}: {section:?}"
                    );
                }
            }
        }
    }
}

/// A crash has to duck the band and then let it back up — that memory is the
/// difference between an AGC and a limiter.
#[test]
fn a_crash_ducks_the_band_and_it_comes_back() {
    let mut agc = Agc::new(SR);
    let tone =
        |amplitude: f64, i: u32| amplitude * (TAU * 600.0 * f64::from(i) / f64::from(SR)).sin();
    let mut steady = 1.0;
    for i in 0..SR {
        steady = agc.next_gain(tone(0.05, i));
    }
    assert!(
        steady > 0.98,
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

    let mut recovered = 0.0;
    for i in 0..SR {
        recovered = agc.next_gain(tone(0.05, i));
    }
    assert!(
        recovered > 0.9,
        "the band never came back up, gain {recovered:.2}"
    );
}

/// A receiver's gain answers to everything reaching it, the signal included.
#[test]
fn a_loud_send_rides_the_gain_down_too() {
    let settings = only_noise(0.3);
    let settle = |send: f64| {
        let mut mixer = BandMixer::new(SR, &settings, 7);
        let mut buf = vec![0.0f32; SR as usize];
        mixer.note_send_level(0.0);
        mixer.fill_background(&mut buf);
        mixer.note_send_level(send);
        mixer.fill_background(&mut buf);
        mixer.agc_gain()
    };
    assert!(settle(0.0) > 0.95, "a quiet band should be left alone");
    assert!(
        settle(1.0) < 0.8,
        "a send at full scale should pull the gain down"
    );
    // A nonsense level is ignored rather than wedging the AGC.
    let mut mixer = BandMixer::new(SR, &settings, 7);
    mixer.note_send_level(f64::NAN);
    let mut buf = vec![0.0f32; 4_800];
    mixer.fill_background(&mut buf);
    assert!(buf.iter().all(|s| s.is_finite()) && mixer.agc_gain().is_finite());
}

#[test]
fn a_silent_band_needs_no_background_stream_and_mixes_to_silence() {
    assert!(!BandMixer::needs_background(&quiet()));
    assert!(BandMixer::needs_background(&only_qrn(0.3)));
    assert!(BandMixer::needs_background(&only_noise(0.2)));
    // Enabled at zero level is still silence.
    assert!(!BandMixer::needs_background(&only_qrn(0.0)));
    assert!(!BandMixer::needs_background(&only_noise(0.0)));

    let mut mixer = BandMixer::new(8_000, &quiet(), 1);
    let mut out = [1.0f32; 64];
    mixer.fill_background(&mut out);
    assert!(out.iter().all(|s| *s == 0.0));
    assert!(BandSource::new(8_000, &quiet(), 1).is_silent());
}

#[test]
fn every_band_stays_in_range_and_makes_a_sound() {
    for shape in SHAPES {
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
    let settings = TrainingSettings::default();
    let mut a = BandSource::new(SR, &settings, 5);
    let mut b = BandSource::new(SR, &settings, 5);
    let mut c = BandSource::new(SR, &settings, 6);
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
    // uncorrelated.
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

// ------------------------------------------------------------------------ QSB

#[test]
fn qsb_modulates_amplitude_without_reaching_silence() {
    let gains: Vec<f32> = (0..48_000)
        .map(|i| qsb_gain_at(f64::from(i) / 48_000.0, true, 1.0, 1.0))
        .collect();
    let min = gains.iter().copied().fold(f32::MAX, f32::min);
    let max = gains.iter().copied().fold(f32::MIN, f32::max);
    assert!(min < 0.8 && max > 0.9);
    assert!(
        min >= QSB_MIN_GAIN as f32,
        "a fade should not reach silence"
    );
}

#[test]
fn qsb_is_unity_when_off() {
    assert_eq!(qsb_gain_at(0.25, false, 1.0, 1.0), 1.0);
    assert_eq!(qsb_gain_at(0.25, true, 0.0, 1.0), 1.0);
    let trough = qsb_gain_at(0.75, true, 1.0, 1.0);
    assert!(trough > 0.2 && trough < 0.4);
}

/// Real QSB is not a tremolo: several paths beating against each other
/// wander and never come back round. Each cycle of the base rate has to
/// differ from the first; a sine would differ by zero.
#[test]
fn fading_never_comes_back_round() {
    let (depth, rate, dt) = (0.8, 0.2, 0.01);
    let env: Vec<f64> = (0..20_000)
        .map(|i| f64::from(qsb_gain_at(f64::from(i) * dt, true, depth, rate)))
        .collect();
    let period = (1.0 / rate / dt) as usize;
    for cycle in 1..8 {
        let differs = (0..period)
            .map(|i| (env[i] - env[cycle * period + i]).abs())
            .fold(0.0_f64, f64::max);
        assert!(
            differs > 0.1,
            "cycle {cycle} repeated the first to within {differs:.3}"
        );
    }
}

/// And it still spans the depth it is asked for: down to the floor that keeps
/// a group answerable, back up to full signal, and never outside that.
#[test]
fn fading_spans_its_depth_and_stays_inside_it() {
    let env: Vec<f64> = (0..20_000)
        .map(|i| f64::from(qsb_gain_at(f64::from(i) * 0.01, true, 0.8, 0.2)))
        .collect();
    let min = env.iter().copied().fold(f64::MAX, f64::min);
    let max = env.iter().copied().fold(f64::MIN, f64::max);
    assert!(
        min <= QSB_MIN_GAIN + 0.02,
        "deep fades never arrived: {min:.3}"
    );
    assert!(max >= 0.97, "the signal never came back up: {max:.3}");
    assert!(min >= QSB_MIN_GAIN - 1e-6, "faded past the floor: {min:.3}");

    let depth: f64 = 0.5;
    let range = depth.min(1.0 - QSB_MIN_GAIN);
    for step in 0..200 {
        let gain = f64::from(qsb_gain_at(f64::from(step) * 0.05, true, depth, 0.5));
        assert!(gain >= 1.0 - range - 1e-6 && gain <= 1.0 + 1e-6, "{gain}");
    }
    for t in [0.0, -1.0, 1e9] {
        assert!(qsb_gain_at(t, true, 0.6, 1.0).is_finite());
    }
}

/// The edges the pile-up is placed between are the filter's own 3 dB points,
/// and a tone and its mirror are passed exactly alike.
#[test]
fn the_passband_edges_are_the_filters_and_mirrors_pass_alike() {
    for shape in SHAPES {
        for (center, bandwidth) in [
            (500.0, 150.0),
            (600.0, 500.0),
            (400.0, FILTER_BANDWIDTH_MAX),
        ] {
            let design = FilterDesign::new(center, bandwidth, shape);
            let (low, high) = passband_edges(center, bandwidth);
            assert!(((high - low) - bandwidth).abs() < 1e-9);
            for edge in [low, high] {
                let db = 20.0 * design.response_at(edge).log10();
                assert!(
                    (db + 3.01).abs() < 0.05,
                    "{shape:?} {center}/{bandwidth}: {db:.2} dB at {edge:.1} Hz"
                );
            }
            for hz in [center * 0.7, center * 0.9, center * 1.3] {
                let a = design.response_at(hz);
                let b = design.response_at(mirror_hz(center, hz));
                assert!((a - b).abs() < 1e-9);
            }
        }
    }
    assert!(passband_edges(f64::NAN, f64::NAN).0.is_finite());
    assert!(mirror_hz(500.0, 0.0).is_finite());
}
