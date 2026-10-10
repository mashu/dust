//! The receiver's CW filter: the one thing every sound you hear has passed
//! through.
//!
//! A real CW filter is many poles deep — a crystal ladder in an IF strip, or
//! its DSP equivalent — and its shape is what an operator hears in the noise.
//! Band-limited hiss takes on the filter's pitch and width, and every dit and
//! every crash of static leaves the filter ringing for a moment afterwards.
//! There is no separate "ringing" to add: it is this filter's impulse
//! response, and it gets longer as the filter gets narrower and its skirts
//! steeper.
//!
//! The filter is designed the textbook way: an all-pole low-pass prototype
//! (Butterworth for [`FilterShape::Sharp`], Bessel for [`FilterShape::Soft`])
//! moved to the pitch by the low-pass to band-pass transform, then factored
//! into second-order band-pass sections. Each section is exactly the
//! "band-pass, 0 dB peak" biquad that Web Audio's `BiquadFilterNode` runs,
//! so the browser builds the same filter out of nodes that the native player
//! runs here in Rust.

use std::f64::consts::{PI, TAU};

use crate::settings::{FILTER_BANDWIDTH_MAX, FILTER_BANDWIDTH_MIN, FilterShape, TrainingSettings};

/// How many second-order sections make up the receiver's filter — one per
/// resonator of the low-pass prototype.
///
/// Eight is what "an 8-pole crystal filter" means: eight crystals, a shape
/// factor near two between its 6 dB and 60 dB widths. That is steep enough to
/// push a station two bandwidths off down by tens of decibels, and steep
/// enough to ring the way everyone knows a narrow filter by. Fewer sections
/// at the same width sound woolly: the skirts let the noise either side in.
pub const RECEIVER_SECTIONS: usize = 8;

/// The lowest centre a section may sit at. Below this a "CW filter" is a
/// rumble filter, and the bilinear transform has nothing useful to warp.
const MIN_SECTION_HZ: f64 = 20.0;
/// The broadest a section may be. Only reached by a filter far wider than its
/// own centre frequency, where it keeps the arithmetic away from zero.
const MIN_SECTION_Q: f64 = 0.05;

/// The low-pass prototype's poles — all of them, conjugates included —
/// normalised so the prototype is 3 dB down at 1 rad/s.
fn prototype_poles(shape: FilterShape) -> [Complex; RECEIVER_SECTIONS] {
    match shape {
        // Butterworth: a maximally flat passband and the steepest skirts an
        // all-pole filter of this order has without ripple, at the price of a
        // step response that overshoots and rings. That is the sound of a
        // modern narrow CW filter, and what Icom calls the SHARP shape.
        FilterShape::Sharp => {
            let order = RECEIVER_SECTIONS as f64;
            std::array::from_fn(|m| {
                let theta = PI * (2.0 * m as f64 + order + 1.0) / (2.0 * order);
                Complex::new(theta.cos(), theta.sin())
            })
        }
        // Bessel: maximally flat group delay, so a keyed tone comes through
        // with almost no overshoot and dies away without ringing on. Rounder
        // shoulders and a slower roll-off — the SOFT shape, closer to an older
        // analogue filter.
        FilterShape::Soft => {
            let [a, b, c, d] = BESSEL_POLES;
            [a, a.conj(), b, b.conj(), c, c.conj(), d, d.conj()]
        }
    }
}

/// The eighth-order Bessel poles in the upper half-plane — roots of the
/// reverse Bessel polynomial s⁸ + 36s⁷ + 630s⁶ + 6930s⁵ + 51975s⁴ +
/// 270270s³ + 945945s² + 2027025s + 2027025 — divided by its 3 dB frequency,
/// 3.179 617 238 rad/s. `the_soft_prototype_is_the_bessel_polynomial` checks
/// them against the polynomial.
const BESSEL_POLES: [Complex; 4] = [
    Complex::new(-1.757_408_400_402, 0.272_867_575_102),
    Complex::new(-1.636_939_418_127, 0.822_795_625_140),
    Complex::new(-1.373_841_217_637, 1.388_356_575_878),
    Complex::new(-0.892_869_718_847, 1.998_325_843_641),
];

/// One second-order band-pass section: a centre and a Q, with unit gain at
/// its own centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    pub center_hz: f64,
    pub q: f64,
}

impl Section {
    /// Magnitude of the analogue section at `hz`.
    fn response_at(self, hz: f64) -> f64 {
        let hz = hz.max(1e-6);
        let detune = hz / self.center_hz - self.center_hz / hz;
        1.0 / (1.0 + self.q * self.q * detune * detune).sqrt()
    }
}

/// The receiver's filter, as numbers: which sections, and the gain that puts
/// the pitch it is tuned to back at unity.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterDesign {
    pub(super) sections: [Section; RECEIVER_SECTIONS],
    pub(super) gain: f64,
}

impl FilterDesign {
    /// A filter `bandwidth_hz` wide between its 3 dB points, centred on
    /// `center_hz`.
    ///
    /// The band-pass transform makes the passband geometrically symmetric
    /// about the centre, which is what a band-pass at audio frequencies is:
    /// the two edges multiply to the centre squared and lie `bandwidth_hz`
    /// apart.
    pub fn new(center_hz: f64, bandwidth_hz: f64, shape: FilterShape) -> Self {
        let center = finite_or(center_hz, 500.0).max(MIN_SECTION_HZ);
        let bandwidth =
            finite_or(bandwidth_hz, 500.0).clamp(FILTER_BANDWIDTH_MIN, FILTER_BANDWIDTH_MAX);
        let sections = band_pass_sections(prototype_poles(shape), center, bandwidth);
        let at_center: f64 = sections.iter().map(|s| s.response_at(center)).product();
        Self {
            sections,
            gain: 1.0 / at_center.max(f64::MIN_POSITIVE),
        }
    }

    pub fn from_settings(settings: &TrainingSettings) -> Self {
        Self::new(
            settings.side_tone_center(),
            settings.band.filter_bandwidth_hz,
            settings.band.filter_shape,
        )
    }

    pub fn sections(&self) -> &[Section; RECEIVER_SECTIONS] {
        &self.sections
    }

    /// The gain applied after the sections, so the centre passes at unity.
    pub fn gain(&self) -> f64 {
        self.gain
    }

    /// How much of a steady tone at `hz` the receiver passes, as a gain
    /// between 0 and 1.
    ///
    /// Read off the design rather than measured, but it is the same filter
    /// [`ReceiverFilter`] runs: a display that draws this is drawing what you
    /// are listening through.
    pub fn response_at(&self, hz: f64) -> f64 {
        self.gain
            * self
                .sections
                .iter()
                .map(|s| s.response_at(hz))
                .product::<f64>()
    }
}

/// The receiver's filter, running: four sections and a gain, one sample at a
/// time.
///
/// Fixed-size and allocation-free, because it runs inside the audio callback.
#[derive(Clone, Debug)]
pub struct ReceiverFilter {
    stages: [Svf; RECEIVER_SECTIONS],
    gain: f64,
}

impl ReceiverFilter {
    pub fn new(sample_rate: u32, design: &FilterDesign) -> Self {
        let sr = f64::from(sample_rate.max(1));
        Self {
            stages: design
                .sections
                .map(|section| Svf::band_pass(sr, section.center_hz, section.q)),
            gain: design.gain,
        }
    }

    pub fn from_settings(sample_rate: u32, settings: &TrainingSettings) -> Self {
        Self::new(sample_rate, &FilterDesign::from_settings(settings))
    }

    pub fn process(&mut self, input: f64) -> f64 {
        self.gain
            * self
                .stages
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

/// A topology-preserving state-variable filter, band-pass output.
///
/// A direct-form biquad is only well behaved while its coefficients hold
/// still and its numbers are large; this structure keeps its state in two
/// integrators that mean the same thing at any tuning, and is exact to the
/// bilinear transform with the section's own centre pre-warped. Its transfer
/// function is the "band-pass, constant 0 dB peak" biquad — the one a Web
/// Audio `BiquadFilterNode` of type `bandpass` implements — so a section here
/// and a node in the browser are the same filter.
#[derive(Clone, Copy, Debug)]
struct Svf {
    k: f64,
    a1: f64,
    a2: f64,
    a3: f64,
    ic1: f64,
    ic2: f64,
}

impl Svf {
    fn band_pass(sample_rate: f64, f0: f64, q: f64) -> Self {
        let sr = sample_rate.max(1.0);
        // Nyquist wins over the floor: at an absurdly low sample rate the
        // upper bound would otherwise fall below the lower one and clamp
        // panics.
        let freq = f0.clamp(MIN_SECTION_HZ, (sr * 0.45).max(MIN_SECTION_HZ));
        let k = 1.0 / q.max(MIN_SECTION_Q);
        let g = (PI * freq / sr).tan();
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            k,
            a1,
            a2,
            a3: g * a2,
            ic1: 0.0,
            ic2: 0.0,
        }
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

/// The receiver's 3 dB edges for a filter `bandwidth_hz` wide around
/// `center_hz`.
///
/// An audio band-pass is geometrically symmetric: the edges are `bandwidth_hz`
/// apart and multiply to the centre squared, so the upper one is further from
/// the centre in hertz than the lower. Anything that places a station "inside
/// the passband" has to use these, not the centre plus and minus half.
pub fn passband_edges(center_hz: f64, bandwidth_hz: f64) -> (f64, f64) {
    let center = finite_or(center_hz, 500.0).max(MIN_SECTION_HZ);
    let bandwidth =
        finite_or(bandwidth_hz, 500.0).clamp(FILTER_BANDWIDTH_MIN, FILTER_BANDWIDTH_MAX);
    let low = (-bandwidth + bandwidth.mul_add(bandwidth, 4.0 * center * center).sqrt()) / 2.0;
    (low, low + bandwidth)
}

/// The tone the filter passes exactly as well as `hz`, on the other side of
/// the centre: the geometric mirror, `center²/hz`.
pub fn mirror_hz(center_hz: f64, hz: f64) -> f64 {
    center_hz * center_hz / hz.max(1e-3)
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// The low-pass to band-pass transform, s → (s² + ω₀²) / (B·s), applied pole
/// by pole.
///
/// Every prototype pole p becomes the two roots of s² − pBs + ω₀² = 0. Their
/// product is ω₀², a positive real, so one root is always in the upper
/// half-plane and the other is its partner's conjugate in the lower one. One
/// upper root per prototype pole is one section per pole, and a pole at
/// σ + jω is a section centred on |s| with a Q of |s| / −2σ. The whole
/// band-pass has a constant times sᴺ on top, which is exactly what N unit-peak
/// band-pass sections multiply out to — so nothing is lost in the factoring
/// but an overall gain.
fn band_pass_sections(
    prototype: [Complex; RECEIVER_SECTIONS],
    center_hz: f64,
    bandwidth_hz: f64,
) -> [Section; RECEIVER_SECTIONS] {
    let w0 = TAU * center_hz;
    let b = TAU * bandwidth_hz;
    prototype.map(|p| {
        let pb = p.scale(b);
        let disc = pb.mul(pb).sub(Complex::new(4.0 * w0 * w0, 0.0)).sqrt();
        let (r1, r2) = (pb.add(disc).scale(0.5), pb.sub(disc).scale(0.5));
        let root = if r1.im >= r2.im { r1 } else { r2 };
        let omega = root.abs();
        Section {
            center_hz: (omega / TAU).max(MIN_SECTION_HZ),
            q: (omega / (-2.0 * root.re)).max(MIN_SECTION_Q),
        }
    })
}

/// Just enough complex arithmetic for the pole transform.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }

    fn add(self, o: Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }

    fn sub(self, o: Self) -> Self {
        Self::new(self.re - o.re, self.im - o.im)
    }

    fn mul(self, o: Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }

    fn scale(self, k: f64) -> Self {
        Self::new(self.re * k, self.im * k)
    }

    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// Principal square root.
    fn sqrt(self) -> Self {
        let r = self.abs();
        let re = ((r + self.re) / 2.0).max(0.0).sqrt();
        let im = ((r - self.re) / 2.0).max(0.0).sqrt().copysign(self.im);
        Self::new(re, im)
    }
}
