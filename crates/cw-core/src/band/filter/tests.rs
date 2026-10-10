//! The receiver filter against what a real IF filter does, measured the way
//! the off-air recordings were: white noise in, a Welch spectrum out, edges
//! read off it.

use std::f64::consts::{SQRT_2, TAU};

use super::*;
use crate::band::Gaussian;

const SHAPES: [FilterShape; 2] = [FilterShape::Sharp, FilterShape::Soft];

/// The rate the long noise renders run at. Every edge measured here is below
/// 2 kHz, the design does not depend on the rate, and a quarter of 48 kHz keeps
/// a minute of noise quick to render and analyse.
const MEASURE_RATE: u32 = 12_000;
/// Welch segment: 4096 points at 12 kHz is 2.9 Hz per bin, inside the 3 Hz
/// the acceptance asks for.
const SEGMENT: usize = 4_096;
/// How many bins the spectrum is smoothed over before edges are read: about
/// 15 Hz, narrow beside the narrowest skirt and enough to steady a minute of
/// noise to a few hundredths of a decibel.
const SMOOTH_BINS: usize = 5;
/// A minute of noise, as the acceptance asks.
const RENDER_SEC: usize = 60;

/// In-place radix-2 FFT. Slow and simple, which is all a test needs.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let twiddles: Vec<(f64, f64)> = (0..n / 2)
        .map(|k| {
            let (s, c) = (-TAU * k as f64 / n as f64).sin_cos();
            (c, s)
        })
        .collect();
    let mut len = 2;
    while len <= n {
        let stride = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = twiddles[k * stride];
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// The receiver's power response to `seconds` of white noise: a Hann-window
/// Welch spectrum of the output divided by the input's variance, so a bin
/// reads |H(f)|² directly. Returns the bin spacing and the smoothed spectrum.
fn noise_response(design: &FilterDesign, seconds: usize) -> (f64, Vec<f64>) {
    let mut filter = ReceiverFilter::new(MEASURE_RATE, design);
    let mut noise = Gaussian::new(0x5EC7_0A11);
    // Let the narrowest, slowest filter fill before anything is kept.
    for _ in 0..MEASURE_RATE / 2 {
        filter.process(noise.sample());
    }
    let n = MEASURE_RATE as usize * seconds;
    let mut input_power = 0.0;
    let output: Vec<f64> = (0..n)
        .map(|_| {
            let x = noise.sample();
            input_power += x * x;
            filter.process(x)
        })
        .collect();
    let variance = input_power / n as f64;

    let window: Vec<f64> = (0..SEGMENT)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / SEGMENT as f64).cos())
        .collect();
    let window_power: f64 = window.iter().map(|w| w * w).sum();
    let mut power = vec![0.0; SEGMENT / 2];
    let mut segments = 0;
    let (mut re, mut im) = (vec![0.0; SEGMENT], vec![0.0; SEGMENT]);
    let mut start = 0;
    while start + SEGMENT <= output.len() {
        for (i, w) in window.iter().enumerate() {
            re[i] = output[start + i] * w;
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for (k, slot) in power.iter_mut().enumerate() {
            *slot += re[k] * re[k] + im[k] * im[k];
        }
        segments += 1;
        start += SEGMENT / 2;
    }
    let scale = 1.0 / (f64::from(segments) * window_power * variance);
    let raw: Vec<f64> = power.iter().map(|p| p * scale).collect();
    let half = SMOOTH_BINS / 2;
    let smoothed = (0..raw.len())
        .map(|k| {
            let lo = k.saturating_sub(half);
            let hi = (k + half + 1).min(raw.len());
            raw[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
        })
        .collect();
    (f64::from(MEASURE_RATE) / SEGMENT as f64, smoothed)
}

/// Where the spectrum first falls `db` below unity, walking out from the
/// pitch in `direction`, interpolated between bins in decibels. `None` if it
/// never does before zero hertz.
fn edge(bin_hz: f64, power: &[f64], pitch: f64, db: f64, direction: isize) -> Option<f64> {
    let level = |k: usize| 10.0 * power[k].max(1e-300).log10();
    let mut k = (pitch / bin_hz).round() as usize;
    loop {
        let next = k.checked_add_signed(direction)?;
        if next == 0 || next >= power.len() {
            return None;
        }
        if level(next) <= -db {
            let (a, b) = (level(k), level(next));
            let t = (a + db) / (a - b);
            let hz = (k as f64 + t * direction as f64) * bin_hz;
            return Some(hz);
        }
        k = next;
    }
}

/// The folded sideband's own contribution at `hz`, as an amplitude.
fn folded_at(design: &FilterDesign, hz: f64) -> f64 {
    design.gain() * design.lowpass(hz + design.pitch_hz()).abs()
}

/// Steady-state gain of the running filter for a tone at `hz`.
fn tone_gain(sample_rate: u32, design: &FilterDesign, hz: f64) -> f64 {
    let mut filter = ReceiverFilter::new(sample_rate, design);
    let step = TAU * hz / f64::from(sample_rate);
    let settle = sample_rate as usize / 2;
    // A whole number of cycles, so the RMS is exact.
    let cycles = (hz / 4.0).round().max(1.0);
    let measure = (cycles * f64::from(sample_rate) / hz).round() as usize;
    let mut sum_sq = 0.0;
    for n in 0..settle + measure {
        let out = filter.process((step * n as f64).sin());
        if n >= settle {
            sum_sq += out * out;
        }
    }
    (sum_sq / measure as f64).sqrt() * SQRT_2
}

/// The acceptance the off-air measurements set, for one shape at one pitch
/// across the three widths: a minute of white noise through the receiver, and
/// the hiss has to come out centred on the note with mirror-image skirts.
///
/// - −6 dB midpoint within ±5 Hz of the pitch.
/// - −3 dB edges within ±10 Hz of pitch ± bandwidth/2.
/// - −20 dB distances above and below the pitch within 5% of each other
///   wherever pitch − bandwidth/2 ≥ 150 Hz.
/// - Unity at the pitch to 0.1 dB.
///
/// An edge is only read where the folded sideband is at least 20 dB under the
/// level being measured there — and, since every level is read against the
/// pitch, at the pitch too. Closer than that the filter is reaching towards
/// zero beat, and the opposite sideband folding back moves the edge on a real
/// receiver as much as here: two independent sidebands add in power, which
/// alone shifts a soft 1 kHz filter's lower edge at an 800 Hz pitch by 25 Hz.
/// The acceptance assumes no fold. For Sharp that excuses nothing it names
/// wherever pitch − bandwidth/2 ≥ 150 Hz: every one of those edges is read.
fn accept(shape: FilterShape, pitch: f64) {
    for bandwidth in [250.0, 500.0, 1_000.0] {
        let design = FilterDesign::new(pitch, bandwidth, shape);
        let (bin_hz, power) = noise_response(&design, RENDER_SEC);
        let name = format!("{shape:?} {pitch}/{bandwidth}");
        let reaches_down = pitch - bandwidth / 2.0 >= 150.0;
        let clear = |hz: f64, level_db: f64| {
            hz > 0.0 && folded_at(&design, hz) <= 0.1 * 10f64.powf(-level_db / 20.0)
        };
        let pitch_is_clear = clear(pitch, 0.0);
        // Where an excused edge is allowed to be excused.
        let excusable = |what: &str| {
            assert!(
                shape == FilterShape::Soft || !reaches_down,
                "{name}: the fold reached {what}, which the acceptance checks"
            );
        };

        let gain_db = 20.0 * tone_gain(MEASURE_RATE, &design, pitch).log10();
        assert!(gain_db.abs() < 0.1, "{name}: {gain_db:+.3} dB at the pitch");
        if !pitch_is_clear {
            excusable("the pitch");
            continue;
        }

        let upper3 = edge(bin_hz, &power, pitch, 3.0, 1).expect("an upper 3 dB edge");
        assert!(
            (upper3 - (pitch + bandwidth / 2.0)).abs() <= 10.0,
            "{name}: upper 3 dB edge at {upper3:.1} Hz"
        );
        if clear(2.0 * pitch - upper3, 3.0) {
            let lower3 = edge(bin_hz, &power, pitch, 3.0, -1).expect("a lower 3 dB edge");
            assert!(
                (lower3 - (pitch - bandwidth / 2.0)).abs() <= 10.0,
                "{name}: lower 3 dB edge at {lower3:.1} Hz"
            );
        } else {
            excusable("the lower 3 dB edge");
        }

        let upper6 = edge(bin_hz, &power, pitch, 6.0, 1).expect("an upper 6 dB edge");
        if clear(2.0 * pitch - upper6, 6.0) {
            let lower6 = edge(bin_hz, &power, pitch, 6.0, -1).expect("a lower 6 dB edge");
            let midpoint = (lower6 + upper6) / 2.0;
            assert!(
                (midpoint - pitch).abs() <= 5.0,
                "{name}: the hiss is centred on {midpoint:.1} Hz"
            );
        } else {
            excusable("the lower 6 dB edge");
        }

        let upper20 = edge(bin_hz, &power, pitch, 20.0, 1).expect("an upper 20 dB edge");
        let lower = |db: f64| edge(bin_hz, &power, pitch, db, -1).unwrap_or(f64::NAN);
        println!(
            "{name}: pitch {gain_db:+.3} dB; -3 dB {:.1}/{upper3:.1}; -6 dB midpoint {:+.1}; \
             -20 dB {:.1}/{upper20:.1}",
            lower(3.0),
            (lower(6.0) + upper6) / 2.0 - pitch,
            lower(20.0),
        );
        if !reaches_down {
            continue;
        }
        if clear(2.0 * pitch - upper20, 20.0) {
            let lower20 = edge(bin_hz, &power, pitch, 20.0, -1).expect("a lower 20 dB edge");
            let ratio = (upper20 - pitch) / (pitch - lower20);
            assert!(
                ratio <= 1.05 && 1.0 / ratio <= 1.05,
                "{name}: −20 dB at {lower20:.1}/{upper20:.1} Hz, skirts {ratio:.3} apart"
            );
        } else {
            excusable("the lower 20 dB edge");
        }
    }
}

#[test]
fn sharp_hiss_is_centred_on_a_400_hz_note() {
    accept(FilterShape::Sharp, 400.0);
}

#[test]
fn sharp_hiss_is_centred_on_a_600_hz_note() {
    accept(FilterShape::Sharp, 600.0);
}

#[test]
fn sharp_hiss_is_centred_on_an_800_hz_note() {
    accept(FilterShape::Sharp, 800.0);
}

#[test]
fn soft_hiss_is_centred_on_a_400_hz_note() {
    accept(FilterShape::Soft, 400.0);
}

#[test]
fn soft_hiss_is_centred_on_a_600_hz_note() {
    accept(FilterShape::Soft, 600.0);
}

#[test]
fn soft_hiss_is_centred_on_an_800_hz_note() {
    accept(FilterShape::Soft, 800.0);
}

/// The browser builds the receiver from `IIRFilterNode`s, each running its
/// feedforward and feedback coefficients as the plain difference equation the
/// Web Audio spec defines. Run exactly that, section by section, and the sum
/// has to be the native filter sample for sample.
#[test]
fn the_sections_are_what_the_browsers_iir_nodes_run() {
    for shape in SHAPES {
        for (pitch, bandwidth, sample_rate) in [
            (600.0, 500.0, 48_000),
            (400.0, 150.0, 44_100),
            (1_200.0, 2_000.0, 22_050),
            (250.0, 2_000.0, 96_000),
        ] {
            let design = FilterDesign::new(pitch, bandwidth, shape);
            let sections = design.sections(f64::from(sample_rate));
            let mut native = ReceiverFilter::new(sample_rate, &design);
            // Direct form I, one history per node, as the spec writes it.
            let mut history = [([0.0f64; 2], [0.0f64; 2]); RECEIVER_SECTIONS];
            let mut noise = Gaussian::new(3);
            for _ in 0..4_096 {
                let x = noise.sample();
                let mut browser = 0.0;
                for (section, (xs, ys)) in sections.iter().zip(history.iter_mut()) {
                    let [b0, b1] = section.feedforward;
                    let [a0, a1, a2] = section.feedback;
                    let y = (b0 * x + b1 * xs[0] - a1 * ys[0] - a2 * ys[1]) / a0;
                    *xs = [x, xs[0]];
                    *ys = [y, ys[0]];
                    browser += y;
                }
                let ours = native.process(x);
                assert!(
                    (ours - browser).abs() < 1e-9 * (1.0 + browser.abs()),
                    "{shape:?} {pitch}/{bandwidth} at {sample_rate}: {ours} against {browser}"
                );
            }
        }
    }
}

/// The digital filter is the analogue one sampled, so the response the
/// design reports holds at any rate a sound card or a browser runs at — the
/// sections' own transfer function, summed, against the analogue formula.
#[test]
fn the_design_response_holds_at_every_sample_rate() {
    for shape in SHAPES {
        for (pitch, bandwidth) in [(600.0, 500.0), (200.0, 2_000.0), (1_200.0, 2_000.0)] {
            let design = FilterDesign::new(pitch, bandwidth, shape);
            for sample_rate in [8_000.0, 22_050.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
                let sections = design.sections(sample_rate);
                for k in 1..60 {
                    let hz = f64::from(k) * 40.0;
                    let digital = sections
                        .iter()
                        .fold(Complex::new(0.0, 0.0), |acc, s| {
                            acc.add(s.response(sample_rate, hz))
                        })
                        .abs();
                    let analogue = design.response_at(hz);
                    // The first alias of the widest soft filter is what
                    // shows at the lowest rate; at the usual ones nothing
                    // does.
                    let tolerance = if sample_rate < 20_000.0 { 1e-3 } else { 1e-5 };
                    assert!(
                        (digital - analogue).abs() < tolerance,
                        "{shape:?} {pitch}/{bandwidth} at {sample_rate} Hz, {hz} Hz: \
                         {digital:.6} against {analogue:.6}"
                    );
                }
            }
        }
    }
}

/// Arithmetic, not geometric: a tone so many hertz above the pitch passes
/// exactly as well as one the same distance below — the prototype's own
/// response, moved — to a fiftieth of a decibel, everywhere the folded
/// sideband is 60 dB out of the way. At a 1200 Hz pitch that is all of a
/// narrow filter's skirts down past −80 dB.
#[test]
fn the_skirts_are_mirror_images_in_hertz() {
    for shape in SHAPES {
        for bandwidth in [FILTER_BANDWIDTH_MIN, 500.0, 900.0] {
            let design = FilterDesign::new(1_200.0, bandwidth, shape);
            let mut checked = 0;
            for step in 0..=120 {
                let offset = f64::from(step) * bandwidth / 80.0;
                let (up, down) = (1_200.0 + offset, 1_200.0 - offset);
                let main = design.gain() * design.lowpass(offset).abs();
                if down < LOWEST_EDGE_HZ
                    || folded_at(&design, up).max(folded_at(&design, down)) > 1e-3 * main
                {
                    continue;
                }
                let (above, below) = (design.response_at(up), design.response_at(down));
                assert!(
                    (above / main).log10().abs() < 0.001 && (below / main).log10().abs() < 0.001,
                    "{shape:?} {bandwidth}: {offset} Hz off, {above:.3e} above, \
                     {below:.3e} below, {main:.3e} translated"
                );
                checked += 1;
            }
            // The passband and well down both skirts, even for the widest
            // soft one, whose gentle skirts meet the fold soonest.
            assert!(
                checked > 25,
                "{shape:?} {bandwidth}: only {checked} points clear"
            );
        }
    }
    // Narrow and sharp at a high pitch, the skirts are mirrored to beyond
    // −80 dB.
    let design = FilterDesign::new(1_200.0, FILTER_BANDWIDTH_MIN, FilterShape::Sharp);
    let deep = design.response_at(1_200.0 - 2.0 * FILTER_BANDWIDTH_MIN);
    assert!(deep < 1e-4);
    assert!((deep / design.response_at(1_200.0 + 2.0 * FILTER_BANDWIDTH_MIN) - 1.0).abs() < 1e-3);
}

/// The reason the BFO phase is chosen at all. Left at zero, a filter wide
/// enough to fold the far sideband back can cancel its own pitch — a
/// 1640 Hz Butterworth at 240 Hz loses it by 64 dB — or lift some other tone
/// well above it, rewarding a station for sitting off to one side. With the
/// phase chosen, nothing anywhere in the audible passband comes out more than
/// a fraction of a decibel louder than the pitch you are tuned to, at any
/// pitch and width the controls allow: half a decibel where the phase is
/// judged, three quarters between.
#[test]
fn no_tone_in_the_passband_is_louder_than_the_pitch() {
    let mut worst: (f64, f64, f64, FilterShape) = (0.0, 0.0, 0.0, FilterShape::Sharp);
    for shape in SHAPES {
        for pitch in (200..=1_200).step_by(50) {
            for bandwidth in (150..=2_000).step_by(50) {
                let (pitch, bandwidth) = (f64::from(pitch), f64::from(bandwidth));
                let design = FilterDesign::new(pitch, bandwidth, shape);
                let top = pitch + bandwidth;
                let loudest = (0..=400)
                    .map(|k| {
                        let hz = LOWEST_EDGE_HZ + (top - LOWEST_EDGE_HZ) * f64::from(k) / 400.0;
                        design.response_at(hz)
                    })
                    .fold(0.0, f64::max);
                let bump = 20.0 * loudest.log10();
                if bump > worst.0 {
                    worst = (bump, pitch, bandwidth, shape);
                }
            }
        }
    }
    let (bump, pitch, bandwidth, shape) = worst;
    println!("loudest tone above the pitch: {bump:.3} dB, {shape:?} {pitch}/{bandwidth}");
    assert!(
        bump < 0.75,
        "{shape:?} {pitch}/{bandwidth}: a tone came out {bump:.2} dB above the pitch"
    );

    // And the case that motivated it: without the phase, the pitch is gone.
    let design = FilterDesign::new(240.0, 1_640.0, FilterShape::Sharp);
    let unturned = design.translated(Complex::new(1.0, 0.0), 240.0).abs();
    assert!(
        unturned < 0.01,
        "the fold no longer cancels the pitch unturned ({unturned:.4}), so this test \
         no longer shows why the phase is chosen"
    );
    assert!((design.response_at(240.0) - 1.0).abs() < 1e-9);
    assert!(design.gain() < 1.0 / MIN_PITCH_RESPONSE);
}

/// Every corner of the controls, at every rate a sound card or browser runs
/// at, makes a filter that can be built and that stays finite: stable pole
/// pairs, a numerator an `IIRFilterNode` will accept (not all zeros), and
/// numbers out for a burst of noise and an impulse.
#[test]
fn every_corner_of_the_controls_is_stable_and_finite() {
    for shape in SHAPES {
        for pitch in [200.0, 333.0, 600.0, 1_200.0] {
            for bandwidth in [FILTER_BANDWIDTH_MIN, 700.0, 1_640.0, FILTER_BANDWIDTH_MAX] {
                let design = FilterDesign::new(pitch, bandwidth, shape);
                assert!(design.gain().is_finite() && design.gain() > 0.0);
                for sample_rate in [8_000u32, 11_025, 22_050, 44_100, 48_000, 96_000, 192_000] {
                    let name = format!("{shape:?} {pitch}/{bandwidth} at {sample_rate}");
                    for section in design.sections(f64::from(sample_rate)) {
                        let [_, a1, a2] = section.feedback;
                        assert_eq!(section.feedback[0], 1.0);
                        assert!(
                            section
                                .feedforward
                                .iter()
                                .chain(&section.feedback)
                                .all(|c| c.is_finite()),
                            "{name}: {section:?}"
                        );
                        assert!(
                            section.feedforward.iter().any(|b| *b != 0.0),
                            "{name}: {section:?}"
                        );
                        // Both poles inside the unit circle: a₂ is their
                        // product's magnitude, and |a₁| < 1 + a₂.
                        assert!((0.0..1.0).contains(&a2) && a1.abs() < 1.0 + a2, "{name}");
                    }
                    let mut filter = ReceiverFilter::new(sample_rate, &design);
                    let mut noise = Gaussian::new(11);
                    let mut loudest = 0.0f64;
                    for n in 0..sample_rate as usize / 4 {
                        let x = if n == 0 { 1.0 } else { noise.sample() };
                        let y = filter.process(x);
                        assert!(y.is_finite(), "{name}");
                        loudest = loudest.max(y.abs());
                    }
                    assert!(loudest < 20.0, "{name}: peaked at {loudest}");
                }
            }
        }
    }
}

/// The passband the pile-up is placed in is half the bandwidth either side of
/// the pitch, stopping short of zero beat; and the mirror is in hertz.
#[test]
fn the_passband_is_half_the_width_either_side() {
    assert_eq!(passband_edges(600.0, 500.0), (350.0, 850.0));
    assert_eq!(passband_edges(400.0, 2_000.0), (LOWEST_EDGE_HZ, 1_400.0));
    // Out-of-range inputs are clamped like the design clamps them.
    assert_eq!(passband_edges(600.0, 10.0), (525.0, 675.0));
    let (low, high) = passband_edges(0.0, 0.0);
    assert!(low.is_finite() && high > low);
    assert_eq!(mirror_hz(600.0, 450.0), 750.0);
    assert_eq!(mirror_hz(600.0, 750.0), 450.0);
}
