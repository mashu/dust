//! The arithmetic every part of the band shares: one second-order section,
//! decibels, and holding nonsense out of a design.

use std::f64::consts::PI;

/// An amplitude ratio from decibels.
pub fn db_to_gain(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Decibels from an amplitude ratio.
pub fn gain_to_db(gain: f64) -> f64 {
    20.0 * gain.log10()
}

/// `value`, or `fallback` where it is not a number at all.
pub fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// One second-order section, (b₀ + b₁z⁻¹ + b₂z⁻²) / (1 + a₁z⁻¹ + a₂z⁻²), run
/// in transposed direct form II.
///
/// The coefficients hold still for the life of the section — a retune builds
/// a new one — and in double precision a pole pair this close to the unit
/// circle is no trouble for it, so the simplest structure, the one the
/// browser's `IIRFilterNode` also runs, is the right one here.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    state: [f64; 2],
}

/// Where a section's state counts as nothing: 600 dB under full scale.
const SETTLED: f64 = 1e-30;

impl Biquad {
    /// A section with feedforward taps `b` and feedback taps `a`, the leading
    /// one of the feedback left out.
    pub fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self {
            b,
            a,
            state: [0.0; 2],
        }
    }

    /// Turn its output over: every feedforward tap negated, its state with
    /// it, so it carries on from where it stands the other way up.
    pub fn negate(&mut self) {
        self.b = self.b.map(|tap| -tap);
        self.state = self.state.map(|held| -held);
    }

    /// A low-pass pole pair at `q`, unity at DC and falling off above
    /// `corner_hz`: designed in the analogue domain and brought over by the
    /// bilinear transform, prewarped so the corner lands where it was asked
    /// for at any sample rate.
    pub fn low_pass(sample_rate: f64, corner_hz: f64, q: f64) -> Self {
        let k = prewarp(sample_rate, corner_hz);
        let norm = 1.0 / (1.0 + k / q + k * k);
        let b0 = k * k * norm;
        Self::new([b0, 2.0 * b0, b0], bilinear_feedback(k, q, norm))
    }

    /// The same, high-pass: unity at Nyquist, falling off below `corner_hz`.
    pub fn high_pass(sample_rate: f64, corner_hz: f64, q: f64) -> Self {
        let k = prewarp(sample_rate, corner_hz);
        let norm = 1.0 / (1.0 + k / q + k * k);
        Self::new([norm, -2.0 * norm, norm], bilinear_feedback(k, q, norm))
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.state[0];
        self.state[0] = self.b[1] * x - self.a[0] * y + self.state[1];
        self.state[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    /// Bring a section that has died away to nothing to rest at zero.
    ///
    /// In silence a section's state decays towards zero for ever without
    /// reaching it: a second or two after the last sound it is in the
    /// subnormal range, where every operation on it takes a microcode assist,
    /// and rounding there keeps it ringing at the smallest numbers a double
    /// holds indefinitely. Zero is where it was heading, and stays put.
    ///
    /// Both states at once or neither: they hold one ringing between them,
    /// often as two nearly opposite numbers, and zeroing one alone would hand
    /// the other a ring of its own — enough, repeated, to keep it going.
    pub fn settle(&mut self) {
        if self.state.iter().all(|s| s.abs() < SETTLED) {
            self.state = [0.0; 2];
        }
    }

    /// This section's gain for a steady tone at `hz`, from its coefficients.
    #[cfg(test)]
    pub fn gain_at(&self, sample_rate: f64, hz: f64) -> f64 {
        let w = std::f64::consts::TAU * hz / sample_rate;
        // |c0 + c1·e^{-jw} + c2·e^{-2jw}|
        let magnitude = |[c0, c1, c2]: [f64; 3]| {
            let re = c0 + c1 * w.cos() + c2 * (2.0 * w).cos();
            let im = -c1 * w.sin() - c2 * (2.0 * w).sin();
            re.hypot(im)
        };
        magnitude(self.b) / magnitude([1.0, self.a[0], self.a[1]])
    }

    #[cfg(test)]
    pub fn state(&self) -> [f64; 2] {
        self.state
    }
}

/// The prewarped corner, K = tan(π·f/fs). A corner at or past Nyquist has no
/// digital equivalent, so it is held below it; only a degenerate sample rate
/// ever gets there.
fn prewarp(sample_rate: f64, corner_hz: f64) -> f64 {
    let corner = corner_hz.min(0.45 * sample_rate);
    (PI * corner / sample_rate).tan()
}

/// Both analogue prototypes share the denominator s² + s/Q + 1, which the
/// bilinear transform turns into this; `norm` makes its leading coefficient
/// one.
fn bilinear_feedback(k: f64, q: f64, norm: f64) -> [f64; 2] {
    [2.0 * (k * k - 1.0) * norm, (1.0 - k / q + k * k) * norm]
}
