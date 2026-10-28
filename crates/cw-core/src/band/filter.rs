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
//! The filter sits where a real one does: before the detector. A superhet's
//! IF filter and an SDR's channel filter are both a low-pass shape moved up in
//! frequency, and the product detector then maps radio frequency onto audio
//! hertz for hertz. So what reaches the ear is a low-pass response translated
//! to the pitch — H(f) = H_LP(f − pitch) — with skirts that are mirror images
//! in hertz either side of the note. Measured off the air (KiwiSDR, Hermes-Lite
//! 2 and contest-station captures of 20 m, and an RTL-SDR with its own
//! filters) the hiss is centred on the note to within a few hertz and the
//! skirts match to 1.00 at −40 dB. The audio band-pass transform will not do
//! for moving the prototype there: it is geometrically symmetric, which puts
//! the hiss 56 Hz above the note and the upper skirt twice as wide as the
//! lower one at −40 dB.
//!
//! So that is how it is built: an eight-pole low-pass prototype, Butterworth
//! for [`FilterShape::Sharp`] and Bessel for [`FilterShape::Soft`], cut off so
//! it is 6 dB down half the width either side, its impulse response
//! multiplied by a cosine at the pitch. In partial fractions that is eight
//! resonators side by side, each one pole pair of the translated prototype
//! with a two-tap numerator. The native player runs them here; the browser
//! builds the same ones out of `IIRFilterNode`s from the same coefficients
//! ([`FilterDesign::sections`]).
//!
//! [`FilterShape::Peaked`] is the Sharp filter with a rig's audio peak filter
//! after it, folded into the same prototype as one more real pole and a zero:
//! a ninth resonator. It is what makes the hiss ring at the note the way it
//! does on a receiver with the APF switched in.
//!
//! The width is the full width between the 6 dB points, for every shape,
//! because that is how a rig's CW filter is quoted: a "300 Hz filter" is
//! 300 Hz wide at 6 dB, and only about 280 Hz wide at 3 dB.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use super::calibration::REFERENCE_BANDWIDTH_HZ;
use super::dsp::{Biquad, db_to_gain, finite_or, gain_to_db};
use super::memo::{Exact, LastAnswer};
use crate::settings::{
    BandSettings, DEFAULT_FILTER_WIDTH_HZ, FILTER_WIDTH_MAX, FILTER_WIDTH_MIN, FilterShape,
    TrainingSettings,
};

/// How many poles the receiver's IF filter has: the low-pass prototype every
/// shape starts from.
///
/// Eight is what "an 8-pole crystal filter" means: eight crystals, a shape
/// factor near two between its 6 dB and 60 dB widths. That is steep enough to
/// push a station two bandwidths off down by tens of decibels, and steep
/// enough to ring the way everyone knows a narrow filter by. Fewer poles at
/// the same width sound woolly: the skirts let the noise either side in.
const IF_POLES: usize = 8;

/// The most resonators the translated filter is ever built from: the IF
/// filter's eight, and one more for the audio peak filter when it is in. A
/// design says how many it actually uses ([`FilterDesign::sections`]); the
/// rest of a fixed-size bank stays empty, so the running filter never
/// allocates.
pub const MAX_RECEIVER_SECTIONS: usize = IF_POLES + 1;

/// The audio peak filter's resonance, as its −3 dB full width in hertz, and
/// how far it lifts the pitch above a tone well off it, in decibels.
///
/// A rig's APF is a single resonance about a hundred hertz wide with a few
/// decibels of lift, and neither changes with the IF width: it is a separate
/// stage after the detector. Here it is one real pole a = π·width, which on its
/// own is 3 dB down half the width either side, and one zero
/// a·10^(boost/20) further out, which flattens the response to the boost once
/// a tone is well away from the pitch. Normalised to unity at the pitch, a
/// station off the pitch comes out up to that many decibels weaker.
pub const APF_WIDTH_HZ: f64 = 100.0;
pub const APF_BOOST_DB: f64 = 6.0;

/// The widest the receiver is ever built, in hertz at 6 dB.
///
/// Wider than the width control goes, because the receiver always passes the
/// whole side-tone range ([`FilterDesign::from_settings`]): the widest range the
/// controls allow, 200-1200 Hz, through the gentle shoulders of the soft
/// shape, needs its prototype 2.4 kHz wide, and the folded sideband — that
/// filter reaches 500 Hz past zero beat — asks for more again.
pub const MAX_RECEIVER_WIDTH_HZ: f64 = 3_000.0;

/// The most the receiver may take off any pitch the side-tone range can send
/// at, in decibels: every send goes through the same filter, so a station at
/// the end of the range has to come through as well as one in the middle,
/// near enough.
pub const COVERAGE_DB: f64 = -1.0;

/// Where the Bessel prototype, normalised to be 3 dB down at 1 rad/s, is
/// 6 dB down — half the voltage — in the same units.
/// `the_six_db_points_are_where_the_width_says` checks it.
const BESSEL_HALF_VOLTAGE: f64 = 1.391_855_048;

/// The lowest audio frequency the passband is taken to reach, for anything
/// that places a station inside it.
///
/// Below this a CW note is a flutter rather than a tone, and the side-tone
/// controls never go there (their floor is 100 Hz too). It is also where a
/// filter wider than twice the pitch has its folded sideband at full
/// strength, which is no place to put a station.
pub const LOWEST_EDGE_HZ: f64 = 100.0;

/// The pitch range a design accepts. Outside it the numbers are nonsense
/// rather than a receiver, and are pulled back in rather than passed on.
const MIN_PITCH_HZ: f64 = 20.0;
const MAX_PITCH_HZ: f64 = 5_000.0;

/// How finely the BFO phase is searched, over half a turn: half a turn more
/// only flips the sign of the output.
const PHASE_STEPS: usize = 64;
/// How many frequencies across the audible passband the phase is judged at.
const PHASE_POINTS: usize = 64;
/// The folded sideband's strength, at the bottom of the audible passband,
/// below which it cannot shape anything you hear and the phase is left at
/// zero. −80 dB.
const NEGLIGIBLE_IMAGE: f64 = 1e-4;
/// How far above the pitch any tone in the audible passband may come out
/// before a phase is ruled out, in decibels.
const MAX_BUMP_DB: f64 = 0.5;
/// How close to a power sum the fold has to add at the pitch for one phase to
/// count as good as another there, in decibels.
const PITCH_TOLERANCE_DB: f64 = 0.02;
/// How close to a real receiver's the hiss has to come, in decibels, for one
/// phase to count as good as another there: where the fold is too faint to
/// move the hiss, every phase does, and the plainest is kept.
const NOISE_TOLERANCE_DB: f64 = 0.05;
/// The quietest the pitch itself may come out before the make-up gain, so
/// that gain stays bounded. A phase that would leave it quieter is never
/// taken; some phase always leaves it at unity or above.
const MIN_PITCH_RESPONSE: f64 = 0.25;

/// The IF filter's low-pass prototype poles — all of them, conjugates
/// included — normalised so the prototype is 3 dB down at 1 rad/s.
fn prototype_poles(shape: FilterShape) -> [Complex; IF_POLES] {
    match shape {
        // Butterworth: a maximally flat passband and the steepest skirts an
        // all-pole filter of this order has without ripple, at the price of a
        // step response that overshoots and rings. That is the sound of a
        // modern narrow CW filter, and what Icom calls the SHARP shape. The
        // peak filter sits behind the same IF filter.
        FilterShape::Sharp | FilterShape::Peaked => {
            let order = IF_POLES as f64;
            std::array::from_fn(|m| {
                let theta = PI * (2.0 * m as f64 + order + 1.0) / (2.0 * order);
                Complex::new(theta.cos(), theta.sin())
            })
        }
        // Bessel: maximally flat group delay, so a keyed tone comes through
        // with almost no overshoot and dies away without ringing on. Rounder
        // shoulders and a slower roll-off — the SOFT shape of a DSP rig. An
        // older crystal ladder is Chebyshev-like, and rings more than Sharp.
        FilterShape::Soft => {
            let [a, b, c, d] = BESSEL_POLES;
            [a, a.conj(), b, b.conj(), c, c.conj(), d, d.conj()]
        }
    }
}

/// The eighth-order Bessel poles in the upper half-plane — roots of the
/// reverse Bessel polynomial s⁸ + 36s⁷ + 630s⁶ + 6930s⁵ + 51975s⁴ +
/// 270270s³ + 945945s² + 2027025s + 2027025 — divided by its 3 dB frequency,
/// 3.179 617 238 rad/s. `the_prototypes_are_butterworth_and_bessel` checks
/// them against the polynomial.
const BESSEL_POLES: [Complex; 4] = [
    Complex::new(-1.757_408_400_402, 0.272_867_575_102),
    Complex::new(-1.636_939_418_127, 0.822_795_625_140),
    Complex::new(-1.373_841_217_637, 1.388_356_575_878),
    Complex::new(-0.892_869_718_847, 1.998_325_843_641),
];

/// Where the IF prototype of `shape`, normalised to be 3 dB down at 1 rad/s,
/// is 6 dB down: half the voltage. Scaling its poles by 2π·(width/2) over
/// this puts the 6 dB points half the width either side of the pitch.
fn half_voltage(shape: FilterShape) -> f64 {
    match shape {
        // |H|² = 1 / (1 + ω¹⁶) is a quarter where ω¹⁶ = 3.
        FilterShape::Sharp | FilterShape::Peaked => 3f64.powf(1.0 / (2 * IF_POLES) as f64),
        FilterShape::Soft => BESSEL_HALF_VOLTAGE,
    }
}

/// Where the IF prototype of `shape`, normalised to be 3 dB down at 1 rad/s,
/// falls to `db` (below zero), found by bisection on its own response: it
/// falls steadily from zero offset, so there is one crossing.
fn prototype_point(shape: FilterShape, db: f64) -> f64 {
    let poles = prototype_poles(shape);
    let level = db_to_gain(db.min(0.0));
    let gain = |w: f64| response_of(&poles, None, Complex::new(0.0, w)).abs();
    let (mut inside, mut outside) = (0.0, 1.0);
    while gain(outside) >= level && outside < 1e6 {
        (inside, outside) = (outside, outside * 2.0);
    }
    for _ in 0..BISECTIONS {
        let mid = 0.5 * (inside + outside);
        if gain(mid) >= level {
            inside = mid;
        } else {
            outside = mid;
        }
    }
    0.5 * (inside + outside)
}

/// How many halvings an edge search takes: from any bracket the controls make
/// down to well under a millihertz.
const BISECTIONS: usize = 48;
/// How finely [`FilterDesign::edges_at`] walks out from the pitch before it
/// bisects, in steps per width — fine enough that it cannot step over a
/// crossing and back — and how many widths out it looks before giving up on
/// one: further than the deepest skirt anything asks about.
const EDGE_STEPS_PER_WIDTH: f64 = 32.0;
const EDGE_REACH_WIDTHS: f64 = 20.0;

/// A prototype with `poles` and at most one `zero`, unity at zero offset:
/// Π(−p) / Π(s − p), times (s − z) / (−z) when there is a zero.
fn response_of(poles: &[Complex], zero: Option<Complex>, s: Complex) -> Complex {
    let all_pole = poles.iter().fold(Complex::new(1.0, 0.0), |acc, p| {
        acc.mul(p.scale(-1.0).div(s.sub(*p)))
    });
    match zero {
        Some(z) => all_pole.mul(s.sub(z).div(z.scale(-1.0))),
        None => all_pole,
    }
}

/// Partial-fraction residues of a prototype unity at zero offset.
///
/// With gain K = Π(−pⱼ) / (−z) the prototype is K·(s − z) / Π(s − pⱼ), and
/// ρₖ is what is left of that at pₖ once its own factor is taken out:
/// K·(pₖ − z) / Π_{j≠k}(pₖ − pⱼ). Without a zero, K = Π(−pⱼ) and the
/// numerator is just K. Either way the numerator is of lower degree than the
/// denominator, so the prototype is exactly Σ ρₖ / (s − pₖ), with nothing
/// left over to add alongside.
fn residues_of(poles: &[Complex], zero: Option<Complex>) -> [Complex; MAX_RECEIVER_SECTIONS] {
    let dc = poles
        .iter()
        .fold(Complex::new(1.0, 0.0), |acc, p| acc.mul(p.scale(-1.0)));
    let gain = match zero {
        Some(z) => dc.div(z.scale(-1.0)),
        None => dc,
    };
    std::array::from_fn(|k| {
        let Some(pole) = poles.get(k) else {
            return Complex::ZERO;
        };
        let apart = poles
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != k)
            .fold(Complex::new(1.0, 0.0), |acc, (_, p)| acc.mul(pole.sub(*p)));
        let numerator = match zero {
            Some(z) => gain.mul(pole.sub(z)),
            None => gain,
        };
        numerator.div(apart)
    })
}

/// One resonator of the receiver, as a digital filter: a pole pair and a
/// two-tap numerator, (b₀ + b₁z⁻¹) / (1 + a₁z⁻¹ + a₂z⁻²).
///
/// The fields are laid out exactly as a Web Audio `IIRFilterNode` takes them —
/// its feedforward and feedback coefficients — so the browser builds each one
/// with a single call and runs the very difference equation the native player
/// does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    /// b₀, b₁.
    pub feedforward: [f64; 2],
    /// 1, a₁, a₂ — the leading one included, as `IIRFilterNode` wants it.
    pub feedback: [f64; 3],
}

impl Section {
    /// A section that passes nothing: what fills the bank's unused slots.
    const SILENT: Self = Self {
        feedforward: [0.0; 2],
        feedback: [1.0, 0.0, 0.0],
    };
}

#[cfg(test)]
impl FilterDesign {
    fn shape(&self) -> FilterShape {
        self.shape
    }

    /// The make-up gain that brings the pitch back to unity, already folded
    /// into every section's numerator: how far the folded sideband moved it.
    pub(super) fn gain(&self) -> f64 {
        self.rotation.abs()
    }
}

#[cfg(test)]
impl Section {
    /// This section's response to a tone at `hz`, as the difference equation
    /// gives it: the transfer function on the unit circle.
    fn response(self, sample_rate: f64, hz: f64) -> Complex {
        let w = TAU * hz / sample_rate;
        let z1 = Complex::new(w.cos(), -w.sin());
        let z2 = z1.mul(z1);
        let [b0, b1] = self.feedforward;
        let [a0, a1, a2] = self.feedback;
        let num = Complex::new(b0, 0.0).add(z1.scale(b1));
        let den = Complex::new(a0, 0.0).add(z1.scale(a1)).add(z2.scale(a2));
        num.div(den)
    }
}

/// The resonators a design is built from, at one sample rate: eight for an IF
/// filter alone, nine with the peak filter. Fixed-size, so building a
/// receiver from them never allocates; it reads as a slice of the live ones,
/// and iterates over them by value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sections {
    slots: [Section; MAX_RECEIVER_SECTIONS],
    len: usize,
}

impl std::ops::Deref for Sections {
    type Target = [Section];

    fn deref(&self) -> &[Section] {
        &self.slots[..self.len]
    }
}

impl IntoIterator for Sections {
    type Item = Section;
    type IntoIter = std::iter::Take<std::array::IntoIter<Section, MAX_RECEIVER_SECTIONS>>;

    fn into_iter(self) -> Self::IntoIter {
        self.slots.into_iter().take(self.len)
    }
}

/// The receiver's filter, as numbers: the low-pass prototype at its cutoff,
/// where it is translated to, and the phase and gain it is translated with.
///
/// Nothing in here depends on the sample rate. The digital filter is this
/// analogue one sampled ([`FilterDesign::sections`]), so the one description
/// serves the native player, the browser at whatever rate its context runs,
/// and anything that draws the response.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterDesign {
    pitch_hz: f64,
    width_hz: f64,
    shape: FilterShape,
    /// How many of the slots below are live: the IF filter's poles, and the
    /// peak filter's after them when it is in.
    len: usize,
    /// The prototype's poles at the cutoff, in radians per second.
    poles: [Complex; MAX_RECEIVER_SECTIONS],
    /// Their residues, so the prototype is Σ residue / (s − pole).
    residues: [Complex; MAX_RECEIVER_SECTIONS],
    /// The peak filter's zero, when it is in.
    zero: Option<Complex>,
    /// The make-up gain and the BFO phase together: g·e^{jφ}.
    rotation: Complex,
    /// ∫|H(f)|² df over positive frequencies, unity at the pitch.
    noise_bandwidth_hz: f64,
}

impl FilterDesign {
    /// A filter `width_hz` wide between its 6 dB points, centred on
    /// `center_hz` — arithmetically: the 6 dB points of the IF filter are half
    /// the width either side, and a tone so many hertz above the pitch passes
    /// exactly as well as one the same number of hertz below it.
    ///
    /// Unity at the pitch, whatever the width and shape.
    pub fn new(center_hz: f64, width_hz: f64, shape: FilterShape) -> Self {
        let pitch = finite_or(center_hz, 500.0).clamp(MIN_PITCH_HZ, MAX_PITCH_HZ);
        let width = finite_or(width_hz, DEFAULT_FILTER_WIDTH_HZ)
            .clamp(FILTER_WIDTH_MIN, MAX_RECEIVER_WIDTH_HZ);
        // The prototype is 3 dB down at 1 rad/s and 6 dB down at its half
        // voltage point; scaled so that point is half the width out, it is
        // 6 dB down half the width either side of the pitch once it is moved
        // there.
        let cutoff = TAU * width / 2.0 / half_voltage(shape);
        let mut poles = [Complex::ZERO; MAX_RECEIVER_SECTIONS];
        for (slot, pole) in poles.iter_mut().zip(prototype_poles(shape)) {
            *slot = pole.scale(cutoff);
        }
        let (len, zero) = match shape {
            FilterShape::Peaked => {
                // The peak filter does not scale with the IF width: it is its
                // own stage, the same resonance whatever filter is in front.
                let a = PI * APF_WIDTH_HZ;
                poles[IF_POLES] = Complex::new(-a, 0.0);
                let z = Complex::new(-a * db_to_gain(APF_BOOST_DB), 0.0);
                (IF_POLES + 1, Some(z))
            }
            FilterShape::Sharp | FilterShape::Soft => (IF_POLES, None),
        };
        let mut design = Self {
            pitch_hz: pitch,
            width_hz: width,
            shape,
            len,
            poles,
            residues: residues_of(&poles[..len], zero),
            zero,
            rotation: Complex::new(1.0, 0.0),
            noise_bandwidth_hz: 0.0,
        };
        let phase = design.bfo_phase();
        let turn = Complex::from_angle(phase);
        let at_pitch = design.translated(turn, pitch).abs();
        design.rotation = turn.scale(1.0 / at_pitch.max(MIN_PITCH_RESPONSE));
        design.noise_bandwidth_hz = design.energy_noise_bandwidth();
        design
    }

    /// The receiver the settings ask for: centred on the middle of the
    /// side-tone range, in the shape asked for, at the width asked for — or
    /// wider, where that would not pass the whole range
    /// (`ReceiverAsk::width_hz`).
    ///
    /// Remembered: everything that asks — the settings screen as it draws,
    /// every send, the pile-up planner, the band — asks about the settings in
    /// force, and working out how wide to build a receiver over a wide range
    /// takes milliseconds.
    pub fn from_settings(settings: &TrainingSettings) -> Self {
        static LAST: LastAnswer<ReceiverAsk, FilterDesign> = LastAnswer::new();
        let ask = ReceiverAsk::of(&settings.band);
        LAST.get_or(ask, || ask.design())
    }

    /// The pitch the filter is centred on, after clamping.
    pub fn pitch_hz(&self) -> f64 {
        self.pitch_hz
    }

    /// The IF filter's width between its 6 dB points, after clamping.
    pub fn width_hz(&self) -> f64 {
        self.width_hz
    }

    /// How much noise the receiver lets through, as the width of a perfect
    /// rectangular filter that would pass the same: ∫|H(f)|² df over positive
    /// frequencies, with the pitch at unity.
    ///
    /// This, not the width on the control, is what sets how loud the hiss is
    /// through the receiver: white noise comes out of it with exactly this
    /// many hertz' worth of power. A Sharp filter's is a little under its
    /// 6 dB width, a Soft one's well under, and the peak filter's resonance
    /// takes it down further still.
    pub fn noise_bandwidth_hz(&self) -> f64 {
        self.noise_bandwidth_hz
    }

    /// [`FilterDesign::noise_bandwidth_hz`], worked out in closed form from
    /// the poles and residues rather than by integrating the response.
    ///
    /// The impulse response is h(t) = u(t) + ū(t) for t ≥ 0, with
    /// u(t) = Σ rₖ·e^{qₖt}, each pole moved up to the pitch, qₖ = pₖ + j2π·pitch,
    /// and each residue turned by the rotation, rₖ = c·ρₖ. Squared and
    /// integrated, every term is an exponential that dies away, so
    ///
    ///   ∫h² dt = 2·Re Σⱼₖ rⱼrₖ / −(qⱼ + qₖ) + 2·Σⱼₖ rⱼr̄ₖ / −(qⱼ + q̄ₖ).
    ///
    /// The first sum is the part at twice the pitch, which only a filter
    /// wide enough to fold across zero beat makes anything of; the second is
    /// the noise in the passband. By Parseval that is ∫|H(f)|² df over every
    /// frequency, positive and negative alike, so the positive half is half
    /// of it; divided by the response at the pitch squared, which is unity
    /// except where the make-up gain hit its bound.
    fn energy_noise_bandwidth(&self) -> f64 {
        self.noise_bandwidth_with(self.rotation)
    }

    /// [`FilterDesign::energy_noise_bandwidth`] for the filter translated
    /// with `rotation`, whatever gain that leaves at the pitch: the noise
    /// bandwidth is a ratio to the pitch, so the gain falls out of it.
    fn noise_bandwidth_with(&self, rotation: Complex) -> f64 {
        let shift = Complex::new(0.0, TAU * self.pitch_hz);
        let live = |k: usize| (self.poles[k].add(shift), self.residues[k].mul(rotation));
        let mut doubled = Complex::ZERO;
        let mut passband = Complex::ZERO;
        for j in 0..self.len {
            let (qj, rj) = live(j);
            for k in 0..self.len {
                let (qk, rk) = live(k);
                doubled = doubled.add(rj.mul(rk).div(qj.add(qk).scale(-1.0)));
                passband = passband.add(rj.mul(rk.conj()).div(qj.add(qk.conj()).scale(-1.0)));
            }
        }
        let energy = 2.0 * doubled.re + 2.0 * passband.re;
        let at_pitch = self.translated(rotation, self.pitch_hz).abs();
        0.5 * energy / (at_pitch * at_pitch).max(f64::MIN_POSITIVE)
    }

    /// What a real receiver's noise bandwidth would be with this filter:
    /// one that hears the noise of both sidebands, which arrives on two
    /// independent paths and adds in power whatever the phase of the beat,
    /// against the pitch, at which the near sideband and the fold add in
    /// power too. The prototype's whole energy, ∫|H_LP(f)|² df over every
    /// frequency, is the two sidebands' together over positive frequencies;
    /// in closed form, Σⱼₖ ρⱼρₖ / −(pⱼ + pₖ).
    fn two_sideband_noise_bandwidth(&self, image_at_pitch: Complex) -> f64 {
        let mut energy = Complex::ZERO;
        for j in 0..self.len {
            for k in 0..self.len {
                let (pj, pk) = (self.poles[j], self.poles[k]);
                let (rj, rk) = (self.residues[j], self.residues[k]);
                energy = energy.add(rj.mul(rk).div(pj.add(pk).scale(-1.0)));
            }
        }
        energy.re / (1.0 + image_at_pitch.norm_sqr())
    }

    /// The resonators the digital receiver is the sum of, at `sample_rate`:
    /// eight, or nine with the peak filter.
    ///
    /// The translated filter's impulse response, 2·g·h_LP(t)·cos(2π·pitch·t +
    /// φ), sampled (impulse invariance): each prototype pole p, moved up to
    /// p + j2π·pitch, becomes a section with the pole e^{(p + j2π·pitch)T} and
    /// its conjugate, and its residue, turned by the phase and scaled by the
    /// gain, becomes the two numerator taps. Sampling an impulse response
    /// leaves its frequency response alone apart from aliases, and an
    /// eight-pole low-pass is far down its skirt by the next one: from 22 kHz
    /// up the sampled response is the analogue one to 10⁻⁵, and even at 8 kHz
    /// the widest soft filter strays by less than 10⁻³. Which is why
    /// [`FilterDesign::response_at`] needs no sample rate.
    ///
    /// There is no direct path to add alongside them: the prototype's
    /// numerator is of lower degree than its denominator — no zeros at all,
    /// or the peak filter's one against nine poles — so its impulse response
    /// starts from nothing and the sections' taps carry the whole filter.
    pub fn sections(&self, sample_rate: f64) -> Sections {
        let period = 1.0 / finite_or(sample_rate, 1.0).max(1.0);
        let shift = Complex::new(0.0, TAU * self.pitch_hz);
        let slots = std::array::from_fn(|k| {
            if k >= self.len {
                return Section::SILENT;
            }
            let pole = self.poles[k].add(shift).scale(period).exp();
            let residue = self.residues[k].mul(self.rotation).scale(period);
            Section {
                feedforward: [2.0 * residue.re, -2.0 * residue.mul(pole.conj()).re],
                feedback: [1.0, -2.0 * pole.re, pole.norm_sqr()],
            }
        });
        Sections {
            slots,
            len: self.len,
        }
    }

    /// How much of a steady tone at `hz` the receiver passes, as a gain —
    /// unity at the pitch, falling away either side.
    ///
    /// Read off the design rather than measured, but it is the same filter
    /// [`ReceiverFilter`] runs: a display that draws this is drawing what you
    /// are listening through.
    pub fn response_at(&self, hz: f64) -> f64 {
        self.translated(self.rotation, finite_or(hz, 0.0)).abs()
    }

    /// The tones either side of the pitch where the response first falls to
    /// `db` decibels (below zero), walking out from the pitch: how far the
    /// filter reaches at that level, read off its real response rather than
    /// worked out from the width, so the shape and the fold are in it.
    ///
    /// The lower one stops at `LOWEST_EDGE_HZ`, like `passband_edges`:
    /// below that is the folded sideband, not more of the passband.
    pub fn edges_at(&self, db: f64) -> (f64, f64) {
        let level = db_to_gain(finite_or(db, 0.0).min(0.0));
        let floor = LOWEST_EDGE_HZ.min(self.pitch_hz);
        let step = self.width_hz / EDGE_STEPS_PER_WIDTH;
        let ceiling = self.pitch_hz + EDGE_REACH_WIDTHS * self.width_hz;
        let crossing = |direction: f64, limit: f64| {
            let mut inside = self.pitch_hz;
            loop {
                let next = inside + direction * step;
                if (next - limit) * direction >= 0.0 {
                    if self.response_at(limit) >= level {
                        return limit;
                    }
                    return self.bisect(inside, limit, level);
                }
                if self.response_at(next) < level {
                    return self.bisect(inside, next, level);
                }
                inside = next;
            }
        };
        (crossing(-1.0, floor), crossing(1.0, ceiling))
    }

    /// Where between `inside` (at or above `level`) and `outside` (below it)
    /// the response crosses `level`.
    fn bisect(&self, mut inside: f64, mut outside: f64, level: f64) -> f64 {
        for _ in 0..BISECTIONS {
            let mid = 0.5 * (inside + outside);
            if self.response_at(mid) >= level {
                inside = mid;
            } else {
                outside = mid;
            }
        }
        0.5 * (inside + outside)
    }

    /// The prototype on its own at `offset_hz` from its centre, unity at
    /// zero: the IF filter, and the peak filter when it is in.
    fn lowpass(&self, offset_hz: f64) -> Complex {
        response_of(
            &self.poles[..self.len],
            self.zero,
            Complex::new(0.0, TAU * offset_hz),
        )
    }

    /// The real filter's response at `hz` for a given turn of the BFO.
    ///
    /// A real filter answers a tone at +f and at −f alike, so moving the
    /// low-pass up to +pitch moves its mirror down to −pitch, and the mirror's
    /// tail reaches across zero into the audio: H(f) = c·H_LP(f − pitch) +
    /// c̄·H_LP(f + pitch). The second term is the opposite sideband folding
    /// back, and for any CW setting it is far below anything audible; only a
    /// filter wide enough to reach past zero beat brings it in.
    fn translated(&self, turn: Complex, hz: f64) -> Complex {
        turn.mul(self.lowpass(hz - self.pitch_hz))
            .add(turn.conj().mul(self.lowpass(hz + self.pitch_hz)))
    }

    /// Which phase to translate with: the BFO's, in effect.
    ///
    /// In a real receiver it is arbitrary — a station arrives in one sideband,
    /// and the phase of the beat note it makes is nothing the ear can hear.
    /// Here every signal is real audio, which carries both sidebands at once,
    /// so once a filter is wide enough to reach towards zero beat the folded
    /// sideband is a coherent copy of the near one and the phase decides
    /// whether the two add or cancel. Left at zero, an eight-pole Butterworth
    /// 1640 Hz wide at a 240 Hz pitch cancels the pitch itself by 64 dB.
    ///
    /// So it is chosen for the three things the fold could spoil. First, no
    /// tone in the audible passband (from [`LOWEST_EDGE_HZ`] up) may come out
    /// more than [`MAX_BUMP_DB`] louder than the pitch, or a station sitting
    /// off to one side would be rewarded for it, which no real receiver does.
    /// Then, among the phases that manage that, the fold should add at the
    /// pitch the way two independent sidebands do — in power — so that
    /// bringing the pitch to unity does not drag the rest of the response up
    /// or down with it. Two families of phases do, half a turn apart, one
    /// adding the fold's noise below the pitch and one cancelling it; both
    /// are tried exactly, so which of them the search steps land nearest
    /// never decides it. Of them, the one whose hiss is nearest a real
    /// receiver's, which hears both sidebands' noise in power whatever the
    /// phase ([`FilterDesign::two_sideband_noise_bandwidth`]), so the hiss
    /// rises smoothly as the filter widens rather than a decibel or two up
    /// and down with each step of it; and last the one with the least bump.
    /// Over every pitch and width the controls allow some phase meets the
    /// first rule. Where the fold cannot reach anything you hear, it stays at
    /// zero.
    fn bfo_phase(&self) -> f64 {
        let low = LOWEST_EDGE_HZ.min(self.pitch_hz);
        if self.lowpass(low + self.pitch_hz).abs() < NEGLIGIBLE_IMAGE {
            return 0.0;
        }
        let high = self.pitch_hz + self.width_hz / 2.0;
        let step = (high - low) / (PHASE_POINTS - 1) as f64;
        let near: [Complex; PHASE_POINTS] =
            std::array::from_fn(|i| self.lowpass(low + step * i as f64 - self.pitch_hz));
        let folded: [Complex; PHASE_POINTS] =
            std::array::from_fn(|i| self.lowpass(low + step * i as f64 + self.pitch_hz));
        let image_at_pitch = self.lowpass(2.0 * self.pitch_hz);
        // What two independent sidebands would come to at the pitch, and
        // the phases at which the fold comes to exactly that: where it adds
        // at right angles to the near sideband.
        let power_sum = (1.0 + image_at_pitch.norm_sqr()).sqrt();
        let exact = ((image_at_pitch.arg() + FRAC_PI_2) / 2.0).rem_euclid(PI);
        let two_sidebands = self.two_sideband_noise_bandwidth(image_at_pitch);
        let phases = (0..PHASE_STEPS)
            .map(|n| PI * n as f64 / PHASE_STEPS as f64)
            .chain([exact, (exact + FRAC_PI_2).rem_euclid(PI)]);
        let mut best: Option<(PhaseScore, f64)> = None;
        for phase in phases {
            let turn = Complex::from_angle(phase);
            let at_pitch = turn.add(turn.conj().mul(image_at_pitch)).abs();
            if at_pitch < MIN_PITCH_RESPONSE {
                continue;
            }
            let loudest = near
                .iter()
                .zip(&folded)
                .map(|(a, b)| turn.mul(*a).add(turn.conj().mul(*b)).abs())
                .fold(0.0, f64::max);
            let noise_db = (10.0 * (self.noise_bandwidth_with(turn) / two_sidebands).log10()).abs();
            let score = PhaseScore::new(
                gain_to_db(loudest / at_pitch),
                gain_to_db(at_pitch / power_sum).abs(),
                noise_db,
            );
            // Only a real improvement moves it, so ties keep the plainest
            // phase.
            if best.as_ref().is_none_or(|(b, _)| score.beats(b)) {
                best = Some((score, phase));
            }
        }
        best.map_or(0.0, |(_, phase)| phase)
    }
}

/// How well one BFO phase does, in the order [`FilterDesign::bfo_phase`]
/// weighs it: within the bump limit or not, then how far from a power sum the
/// pitch is, then how far its hiss is from a real receiver's, then the bump
/// itself.
#[derive(Clone, Copy, Debug)]
struct PhaseScore {
    over_limit: bool,
    pitch_db: f64,
    noise_db: f64,
    bump_db: f64,
}

impl PhaseScore {
    fn new(bump_db: f64, pitch_db: f64, noise_db: f64) -> Self {
        let over_limit = bump_db > MAX_BUMP_DB;
        // Past the limit nothing matters but getting back under it.
        let pitch_db = if over_limit {
            0.0
        } else {
            (pitch_db - PITCH_TOLERANCE_DB).max(0.0)
        };
        Self {
            over_limit,
            pitch_db,
            // The hiss decides only between phases as good at the pitch.
            noise_db: if over_limit || pitch_db > 0.0 {
                0.0
            } else {
                (noise_db - NOISE_TOLERANCE_DB).max(0.0)
            },
            bump_db,
        }
    }

    fn beats(&self, other: &Self) -> bool {
        const EPS: f64 = 1e-9;
        if self.over_limit != other.over_limit {
            return !self.over_limit;
        }
        if (self.pitch_db - other.pitch_db).abs() > EPS {
            return self.pitch_db < other.pitch_db;
        }
        if (self.noise_db - other.noise_db).abs() > EPS {
            return self.noise_db < other.noise_db;
        }
        self.bump_db < other.bump_db - EPS
    }
}

/// The receiver's filter, running: its resonators side by side, summed, one
/// sample at a time.
///
/// Fixed-size and allocation-free, because it runs inside the audio callback:
/// a bank big enough for the peak filter, of which only the design's own
/// sections run.
#[derive(Clone, Debug)]
pub struct ReceiverFilter {
    resonators: [Biquad; MAX_RECEIVER_SECTIONS],
    len: usize,
    /// What it passes at the pitch, phase and all: its sign there, against
    /// another's, is what [`Self::align_to`] matches.
    at_pitch: Complex,
    /// Samples until the resonators are next brought to rest
    /// ([`Biquad::settle`]).
    until_settle: u32,
}

/// How often, in samples, the resonators are checked for a state that has
/// died away to nothing. Once a block rather than once a sample, so the
/// running filter pays for it in one predictable branch. Even the fastest
/// pole of the widest filter at 8 kHz decays only some 400 dB in a block, so
/// a state is brought to rest long before it could reach the subnormal range.
const SETTLE_EVERY: u32 = 32;

impl ReceiverFilter {
    pub fn new(sample_rate: u32, design: &FilterDesign) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let sections = design.sections(sr);
        Self {
            resonators: sections.slots.map(|section| {
                let [b0, b1] = section.feedforward;
                let [_, a1, a2] = section.feedback;
                Biquad::new([b0, b1, 0.0], [a1, a2])
            }),
            len: sections.len,
            at_pitch: design.translated(design.rotation, design.pitch_hz),
            until_settle: SETTLE_EVERY,
        }
    }

    /// Come out the same way up at the pitch as `playing`, the filter it
    /// takes over from. Two designs a slider step apart can have their BFO
    /// phases half a turn apart, and handed over as the same band twice,
    /// their gains summing to one, the two would cancel mid-fade: the hiss
    /// and every station on the band dropping 20 dB for a few milliseconds.
    /// Which way up a filter plays is nothing the ear hears on its own.
    pub fn align_to(&mut self, playing: &ReceiverFilter) {
        if self.at_pitch.mul(playing.at_pitch.conj()).re < 0.0 {
            for resonator in &mut self.resonators[..self.len] {
                resonator.negate();
            }
            self.at_pitch = self.at_pitch.scale(-1.0);
        }
    }

    pub fn process(&mut self, input: f64) -> f64 {
        let out = self.resonators[..self.len]
            .iter_mut()
            .map(|resonator| resonator.process(input))
            .sum();
        self.until_settle -= 1;
        if self.until_settle == 0 {
            self.until_settle = SETTLE_EVERY;
            for resonator in &mut self.resonators[..self.len] {
                resonator.settle();
            }
        }
        out
    }

    /// Run a rendered buffer through the receiver, in place. The players
    /// filter as they play; this is for measuring a filter whole.
    #[cfg(test)]
    pub(crate) fn apply(&mut self, samples: &mut [f32]) {
        for sample in samples {
            *sample = self.process(f64::from(*sample)) as f32;
        }
    }
}

/// The receiver's 6 dB edges for a filter `width_hz` wide around
/// `center_hz`: half the width either side, because an IF filter is
/// symmetric in hertz.
///
/// The lower edge stops at [`LOWEST_EDGE_HZ`]: a filter wider than twice the
/// pitch reaches past zero beat, and what lies beyond is the opposite sideband
/// folding back, not more room for a station.
pub fn passband_edges(center_hz: f64, width_hz: f64) -> (f64, f64) {
    let center = finite_or(center_hz, 500.0).clamp(MIN_PITCH_HZ, MAX_PITCH_HZ);
    let half = finite_or(width_hz, DEFAULT_FILTER_WIDTH_HZ)
        .clamp(FILTER_WIDTH_MIN, MAX_RECEIVER_WIDTH_HZ)
        / 2.0;
    (
        (center - half).max(LOWEST_EDGE_HZ.min(center)),
        center + half,
    )
}

/// The tone the filter passes exactly as well as `hz`, on the other side of
/// the centre: the arithmetic mirror, `2·center − hz`.
pub fn mirror_hz(center_hz: f64, hz: f64) -> f64 {
    2.0 * center_hz - hz
}

/// The 6 dB width of the sharp filter that is [`REFERENCE_BANDWIDTH_HZ`] wide
/// at 3 dB, about 535 Hz: the 500 Hz filter CW signal reports are quoted in,
/// and the one the band's levels — the AGC's knee among them — are
/// calibrated through. Its noise bandwidth is about 503 Hz.
pub fn reference_width_hz() -> f64 {
    REFERENCE_BANDWIDTH_HZ * half_voltage(FilterShape::Sharp)
}

/// How many points across the side-tone range the real response is checked
/// at, ends included, before a width is taken to pass it.
const COVERAGE_POINTS: usize = 9;
/// How much wider each step is when the fold means the first width found does
/// not quite pass, and the most steps taken: from any width the controls
/// make, enough to reach [`MAX_RECEIVER_WIDTH_HZ`].
const COVERAGE_STEP: f64 = 1.01;
const COVERAGE_STEPS: usize = 128;
/// How close to passing the side-tone range a widened receiver has to come,
/// when none passes it, to be worth building that wide.
const NEARLY_PASSES_DB: f64 = 0.5;

/// What a receiver is built from: the side-tone range it has to pass, and the
/// width and shape asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ReceiverAsk {
    tone_min: Exact,
    tone_max: Exact,
    width: Exact,
    shape: FilterShape,
}

impl ReceiverAsk {
    fn of(band: &BandSettings) -> Self {
        Self {
            tone_min: Exact(band.side_tone_min),
            tone_max: Exact(band.side_tone_max),
            width: Exact(band.filter_width_hz),
            shape: band.filter_shape,
        }
    }

    /// The side-tone range, held to the audible passband.
    fn range(self) -> (f64, f64) {
        let low = finite_or(self.tone_min.0, 500.0).max(LOWEST_EDGE_HZ);
        let high = finite_or(self.tone_max.0, low).max(low);
        (low, high)
    }

    fn design(self) -> FilterDesign {
        let (low, high) = self.range();
        FilterDesign::new(low + (high - low) / 2.0, self.width_hz(), self.shape)
    }

    /// The width the receiver is built at, in hertz at 6 dB: the one asked for,
    /// or wider, if that would not pass the whole side-tone range.
    ///
    /// Every send goes through the same filter, and the filter sits still at the
    /// middle of the side-tone range while the sends are scattered across it. So
    /// it has to pass both ends to within [`COVERAGE_DB`] of the middle, or a
    /// wide range behind a narrow filter would quietly bury every send near
    /// either end of it — which is not what anyone asked for by setting a range.
    /// The narrowest width that does that is what the width is widened to, and
    /// only ever widened: a filter already wide enough is left as it is.
    ///
    /// For [`FilterShape::Peaked`] that is the IF filter's width, worked out for
    /// the Sharp filter it is. The peak filter's resonance is there to favour the
    /// pitch, so a send near the end of a wide range comes out a few decibels
    /// weaker through it, as it would with the APF in on a rig; that is the
    /// point of it, and accepted.
    ///
    /// A range that runs close enough to zero beat can be beyond any width: the
    /// soft filter over the widest range the controls allow, 200-1200 Hz, folds
    /// so much of the other sideband onto its pitch that the top of the range
    /// stays a little over a decibel down however wide it is built. There the
    /// width that comes closest is used.
    ///
    /// Runs whenever the settings change, never per sample.
    fn width_hz(self) -> f64 {
        let asked = finite_or(self.width.0, DEFAULT_FILTER_WIDTH_HZ)
            .clamp(FILTER_WIDTH_MIN, FILTER_WIDTH_MAX);
        let (low, high) = self.range();
        let center = low + (high - low) / 2.0;
        let shape = match self.shape {
            FilterShape::Peaked => FilterShape::Sharp,
            shape => shape,
        };
        // On its own the prototype falls steadily away from the pitch, so the
        // narrowest width that passes both ends puts them exactly at its
        // coverage point.
        let cover = (high - low) * half_voltage(shape) / prototype_point(shape, COVERAGE_DB);
        let mut width = asked.max(cover).min(MAX_RECEIVER_WIDTH_HZ);
        // Wide enough to reach towards zero beat, though, the folded sideband can
        // take a little more off the range: off the bottom of it, where it
        // reaches, and off all of it once it adds to the pitch and the make-up
        // gain brings the pitch back to unity. So the real response is the judge,
        // and the filter is widened a step at a time until it passes — to within
        // a ten-millionth, because where the width just worked out puts the ends
        // exactly at the level, a sideband folding back 140 dB down is no reason
        // to widen it a step.
        let level = db_to_gain(COVERAGE_DB) * (1.0 - 1e-7);
        let quietest = |width: f64| {
            let design = FilterDesign::new(center, width, shape);
            (0..COVERAGE_POINTS)
                .map(|i| {
                    design.response_at(low + (high - low) * i as f64 / (COVERAGE_POINTS - 1) as f64)
                })
                .fold(f64::INFINITY, f64::min)
        };
        // Where the range runs so close to zero beat that no width passes it, a
        // wider build is only worth the noise it lets in if it nearly does —
        // within half a decibel. Otherwise the width it started from stands.
        let start = width;
        let nearly = level * db_to_gain(-NEARLY_PASSES_DB);
        let mut closest = (f64::NEG_INFINITY, width);
        for _ in 0..COVERAGE_STEPS {
            let level_here = quietest(width);
            if level_here >= level {
                return width;
            }
            if level_here > closest.0 {
                closest = (level_here, width);
            }
            if width >= MAX_RECEIVER_WIDTH_HZ {
                break;
            }
            width = (width * COVERAGE_STEP).min(MAX_RECEIVER_WIDTH_HZ);
        }
        if closest.0 >= nearly {
            closest.1
        } else {
            start
        }
    }
}

/// Just enough complex arithmetic for the design.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const ZERO: Self = Self::new(0.0, 0.0);

    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn from_angle(theta: f64) -> Self {
        let (sin, cos) = theta.sin_cos();
        Self::new(cos, sin)
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

    /// Division, or zero where the divisor is: nothing in a design divides by
    /// zero unless its input was already nonsense.
    fn div(self, o: Self) -> Self {
        let d = o.norm_sqr();
        if d > 0.0 {
            Self::new(
                (self.re * o.re + self.im * o.im) / d,
                (self.im * o.re - self.re * o.im) / d,
            )
        } else {
            Self::new(0.0, 0.0)
        }
    }

    fn scale(self, k: f64) -> Self {
        Self::new(self.re * k, self.im * k)
    }

    fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }

    fn exp(self) -> Self {
        Self::from_angle(self.im).scale(self.re.exp())
    }
}

#[cfg(test)]
mod tests;
