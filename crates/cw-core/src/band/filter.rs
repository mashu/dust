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
//! skirts match to 1.00 at −40 dB. An earlier version moved the prototype to
//! the pitch with the audio band-pass transform instead, which is
//! geometrically symmetric: the hiss sat 56 Hz above the note and the upper
//! skirt was twice as wide as the lower one at −40 dB.
//!
//! So that is how it is built: an eight-pole low-pass prototype, Butterworth
//! for [`FilterShape::Sharp`] and Bessel for [`FilterShape::Soft`], cut off at
//! half the bandwidth, its impulse response multiplied by a cosine at the
//! pitch. In partial fractions that is eight resonators side by side, each one
//! pole pair of the translated prototype with a two-tap numerator. The native
//! player runs them here; the browser builds the same eight out of
//! `IIRFilterNode`s from the same coefficients ([`FilterDesign::sections`]).

use std::f64::consts::{PI, TAU};

use crate::settings::{FILTER_BANDWIDTH_MAX, FILTER_BANDWIDTH_MIN, FilterShape, TrainingSettings};

/// How many poles the receiver's low-pass prototype has, and so how many
/// resonators the translated filter is built from.
///
/// Eight is what "an 8-pole crystal filter" means: eight crystals, a shape
/// factor near two between its 6 dB and 60 dB widths. That is steep enough to
/// push a station two bandwidths off down by tens of decibels, and steep
/// enough to ring the way everyone knows a narrow filter by. Fewer poles at
/// the same width sound woolly: the skirts let the noise either side in.
pub const RECEIVER_SECTIONS: usize = 8;

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
/// The quietest the pitch itself may come out before the make-up gain, so
/// that gain stays bounded. A phase that would leave it quieter is never
/// taken; some phase always leaves it at unity or above.
const MIN_PITCH_RESPONSE: f64 = 0.25;

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
/// 3.179 617 238 rad/s. `the_prototypes_are_butterworth_and_bessel` checks
/// them against the polynomial.
const BESSEL_POLES: [Complex; 4] = [
    Complex::new(-1.757_408_400_402, 0.272_867_575_102),
    Complex::new(-1.636_939_418_127, 0.822_795_625_140),
    Complex::new(-1.373_841_217_637, 1.388_356_575_878),
    Complex::new(-0.892_869_718_847, 1.998_325_843_641),
];

/// Partial-fraction residues of a unit-gain all-pole prototype: the low-pass
/// is Σ ρₖ / (s − pₖ), and ρₖ is what is left of Π(−pⱼ) / Π(s − pⱼ) at pₖ
/// once its own factor is taken out.
fn prototype_residues(poles: &[Complex; RECEIVER_SECTIONS]) -> [Complex; RECEIVER_SECTIONS] {
    let dc = poles
        .iter()
        .fold(Complex::new(1.0, 0.0), |acc, p| acc.mul(p.scale(-1.0)));
    std::array::from_fn(|k| {
        let apart = poles
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != k)
            .fold(Complex::new(1.0, 0.0), |acc, (_, p)| {
                acc.mul(poles[k].sub(*p))
            });
        dc.div(apart)
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
    bandwidth_hz: f64,
    /// The prototype's poles at the cutoff, in radians per second.
    poles: [Complex; RECEIVER_SECTIONS],
    /// Their residues, so the prototype is Σ residue / (s − pole).
    residues: [Complex; RECEIVER_SECTIONS],
    /// The make-up gain and the BFO phase together: g·e^{jφ}.
    rotation: Complex,
}

impl FilterDesign {
    /// A filter `bandwidth_hz` wide between its 3 dB points, centred on
    /// `center_hz` — arithmetically: the 3 dB points are half the bandwidth
    /// either side, and a tone so many hertz above the pitch passes exactly as
    /// well as one the same number of hertz below it.
    ///
    /// Unity at the pitch, whatever the width and shape.
    pub fn new(center_hz: f64, bandwidth_hz: f64, shape: FilterShape) -> Self {
        let pitch = finite_or(center_hz, 500.0).clamp(MIN_PITCH_HZ, MAX_PITCH_HZ);
        let bandwidth =
            finite_or(bandwidth_hz, 500.0).clamp(FILTER_BANDWIDTH_MIN, FILTER_BANDWIDTH_MAX);
        // The prototype is 3 dB down at 1 rad/s. Cut off at half the
        // bandwidth, it is 3 dB down half the bandwidth either side of the
        // pitch once it is moved there.
        let cutoff = TAU * bandwidth / 2.0;
        let prototype = prototype_poles(shape);
        let mut design = Self {
            pitch_hz: pitch,
            bandwidth_hz: bandwidth,
            poles: prototype.map(|p| p.scale(cutoff)),
            residues: prototype_residues(&prototype).map(|r| r.scale(cutoff)),
            rotation: Complex::new(1.0, 0.0),
        };
        let phase = design.bfo_phase();
        let turn = Complex::from_angle(phase);
        let at_pitch = design.translated(turn, pitch).abs();
        design.rotation = turn.scale(1.0 / at_pitch.max(MIN_PITCH_RESPONSE));
        design
    }

    pub fn from_settings(settings: &TrainingSettings) -> Self {
        Self::new(
            settings.side_tone_center(),
            settings.band.filter_bandwidth_hz,
            settings.band.filter_shape,
        )
    }

    /// The pitch the filter is centred on, after clamping.
    pub fn pitch_hz(&self) -> f64 {
        self.pitch_hz
    }

    /// The width between the 3 dB points, after clamping.
    pub fn bandwidth_hz(&self) -> f64 {
        self.bandwidth_hz
    }

    /// The make-up gain that brings the pitch back to unity. Already folded
    /// into every section's numerator; reported for anything that wants to
    /// know how far the folded sideband moved it.
    pub fn gain(&self) -> f64 {
        self.rotation.abs()
    }

    /// The eight resonators the digital receiver is the sum of, at
    /// `sample_rate`.
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
    /// There is no direct path to add alongside them: the prototype has no
    /// zeros, so its impulse response starts from nothing and the sections'
    /// taps carry the whole filter.
    pub fn sections(&self, sample_rate: f64) -> [Section; RECEIVER_SECTIONS] {
        let period = 1.0 / finite_or(sample_rate, 1.0).max(1.0);
        let shift = Complex::new(0.0, TAU * self.pitch_hz);
        std::array::from_fn(|k| {
            let pole = self.poles[k].add(shift).scale(period).exp();
            let residue = self.residues[k].mul(self.rotation).scale(period);
            Section {
                feedforward: [2.0 * residue.re, -2.0 * residue.mul(pole.conj()).re],
                feedback: [1.0, -2.0 * pole.re, pole.norm_sqr()],
            }
        })
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

    /// The prototype on its own at `offset_hz` from its centre: Π(−p) / Π(s −
    /// p), unity at zero.
    fn lowpass(&self, offset_hz: f64) -> Complex {
        let s = Complex::new(0.0, TAU * offset_hz);
        self.poles.iter().fold(Complex::new(1.0, 0.0), |acc, p| {
            acc.mul(p.scale(-1.0).div(s.sub(*p)))
        })
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
    /// So it is chosen for the two things the fold could spoil. First, no
    /// tone in the audible passband (from [`LOWEST_EDGE_HZ`] up) may come out
    /// more than [`MAX_BUMP_DB`] louder than the pitch, or a station sitting
    /// off to one side would be rewarded for it, which no real receiver does.
    /// Then, among the phases that manage that, the fold should add at the
    /// pitch the way two independent sidebands do — in power — so that
    /// bringing the pitch to unity does not drag the rest of the response up
    /// or down with it; and of those, the one with the least bump. Over every
    /// pitch and width the controls allow some phase meets the first rule.
    /// Where the fold cannot reach anything you hear, it stays at zero.
    fn bfo_phase(&self) -> f64 {
        let low = LOWEST_EDGE_HZ.min(self.pitch_hz);
        if self.lowpass(low + self.pitch_hz).abs() < NEGLIGIBLE_IMAGE {
            return 0.0;
        }
        let high = self.pitch_hz + self.bandwidth_hz / 2.0;
        let step = (high - low) / (PHASE_POINTS - 1) as f64;
        let near: [Complex; PHASE_POINTS] =
            std::array::from_fn(|i| self.lowpass(low + step * i as f64 - self.pitch_hz));
        let folded: [Complex; PHASE_POINTS] =
            std::array::from_fn(|i| self.lowpass(low + step * i as f64 + self.pitch_hz));
        let image_at_pitch = self.lowpass(2.0 * self.pitch_hz);
        // What two independent sidebands would come to at the pitch.
        let power_sum = (1.0 + image_at_pitch.norm_sqr()).sqrt();
        let mut best: Option<(PhaseScore, f64)> = None;
        for n in 0..PHASE_STEPS {
            let phase = PI * n as f64 / PHASE_STEPS as f64;
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
            let score = PhaseScore::new(
                20.0 * (loudest / at_pitch).log10(),
                (20.0 * (at_pitch / power_sum).log10()).abs(),
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
/// pitch is, then the bump itself.
#[derive(Clone, Copy, Debug)]
struct PhaseScore {
    over_limit: bool,
    pitch_db: f64,
    bump_db: f64,
}

impl PhaseScore {
    fn new(bump_db: f64, pitch_db: f64) -> Self {
        let over_limit = bump_db > MAX_BUMP_DB;
        Self {
            over_limit,
            // Past the limit nothing matters but getting back under it.
            pitch_db: if over_limit {
                0.0
            } else {
                (pitch_db - PITCH_TOLERANCE_DB).max(0.0)
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
        self.bump_db < other.bump_db - EPS
    }
}

/// The receiver's filter, running: eight resonators side by side, summed, one
/// sample at a time.
///
/// Fixed-size and allocation-free, because it runs inside the audio callback.
#[derive(Clone, Debug)]
pub struct ReceiverFilter {
    resonators: [Resonator; RECEIVER_SECTIONS],
}

impl ReceiverFilter {
    pub fn new(sample_rate: u32, design: &FilterDesign) -> Self {
        let sr = f64::from(sample_rate.max(1));
        Self {
            resonators: design.sections(sr).map(Resonator::new),
        }
    }

    pub fn from_settings(sample_rate: u32, settings: &TrainingSettings) -> Self {
        Self::new(sample_rate, &FilterDesign::from_settings(settings))
    }

    pub fn process(&mut self, input: f64) -> f64 {
        self.resonators
            .iter_mut()
            .map(|resonator| resonator.process(input))
            .sum()
    }

    /// Run a rendered send through the receiver, in place.
    pub fn apply(&mut self, samples: &mut [f32]) {
        for sample in samples {
            *sample = self.process(f64::from(*sample)) as f32;
        }
    }
}

/// One [`Section`], running: transposed direct form II.
///
/// The coefficients hold still for the life of the filter — a retune builds a
/// new one — and in double precision a pole pair this close to the unit circle
/// is no trouble for it, so the simplest structure the browser's node also
/// runs is the right one here.
#[derive(Clone, Copy, Debug)]
struct Resonator {
    b0: f64,
    b1: f64,
    a1: f64,
    a2: f64,
    s1: f64,
    s2: f64,
}

impl Resonator {
    fn new(section: Section) -> Self {
        let [b0, b1] = section.feedforward;
        let [_, a1, a2] = section.feedback;
        Self {
            b0,
            b1,
            a1,
            a2,
            s1: 0.0,
            s2: 0.0,
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        let out = self.b0 * input + self.s1;
        self.s1 = self.b1 * input - self.a1 * out + self.s2;
        self.s2 = -self.a2 * out;
        out
    }
}

/// The receiver's 3 dB edges for a filter `bandwidth_hz` wide around
/// `center_hz`: half the bandwidth either side, because an IF filter is
/// symmetric in hertz.
///
/// The lower edge stops at [`LOWEST_EDGE_HZ`]: a filter wider than twice the
/// pitch reaches past zero beat, and what lies beyond is the opposite sideband
/// folding back, not more room for a station.
pub fn passband_edges(center_hz: f64, bandwidth_hz: f64) -> (f64, f64) {
    let center = finite_or(center_hz, 500.0).clamp(MIN_PITCH_HZ, MAX_PITCH_HZ);
    let half =
        finite_or(bandwidth_hz, 500.0).clamp(FILTER_BANDWIDTH_MIN, FILTER_BANDWIDTH_MAX) / 2.0;
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

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// Just enough complex arithmetic for the design.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
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

    fn exp(self) -> Self {
        Self::from_angle(self.im).scale(self.re.exp())
    }
}

#[cfg(test)]
mod tests;
