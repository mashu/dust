//! What both players share about how a change sounds: how long a stop, a
//! band change and the receiver's gate take, the shapes they take, and the
//! range the paddle's sidetone plays in. Kept in one place so the desktop
//! and the browser cannot drift apart — values that had to match used to be
//! copied by hand.

use cw_core::{SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ};

/// How long a stopped send takes to reach silence: kept with the plans, as
/// a contest's machine splices a continuation onto a send no sooner than
/// the send can be ridden down.
pub use cw_core::SEND_RELEASE_SEC;

/// How long one band takes to hand over to the next when the settings
/// change. Long enough to hide the seam, short enough that the change is
/// immediate.
pub const BAND_FADE_SEC: f64 = 0.02;

/// How long the receiver takes to fall quiet as you start sending without
/// full break-in, and to come back as you stop: as quick as a rig's mute,
/// and a straight line long enough that neither the hiss nor a caller's
/// tone it catches clicks.
pub const GATE_RAMP_SEC: f64 = 0.005;

/// One step of the receiver's gate as the browser plays it: held at a
/// level from a moment, or moved there in a straight line ending then,
/// from the step before.
#[cfg(any(feature = "web", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GateStep {
    Set(f32, f64),
    Ramp(f32, f64),
}

#[cfg(any(feature = "web", test))]
impl GateStep {
    /// When it is reached, on the gate's clock.
    pub fn at(&self) -> f64 {
        match *self {
            Self::Set(_, at) | Self::Ramp(_, at) => at,
        }
    }
}

/// The receiver's gate for a send starting at `start` on a clock, muted
/// until `muted_until` seconds into it, the gate standing at `from` then:
/// a line to `from` at `start` first, which is the very line the steps the
/// send replaces were drawing, so what played up to then plays on without
/// a step; then shut over [`GATE_RAMP_SEC`], shut to the end of your
/// message, and open again over the ramp after it. A send that takes over
/// from one muted starts shut, and stays so; one muted for nothing opens
/// from wherever the gate stood. The native mixer slews to the same shape a
/// sample at a time.
#[cfg(any(feature = "web", test))]
pub fn gate_steps(muted_until: Option<f64>, start: f64, from: f64) -> Vec<GateStep> {
    let from = from.clamp(0.0, 1.0);
    let mut steps = vec![GateStep::Ramp(from as f32, start)];
    match muted_until {
        Some(until) => {
            let shut = start + GATE_RAMP_SEC;
            let open = (start + until).max(shut);
            steps.push(GateStep::Ramp(0.0, shut));
            steps.push(GateStep::Set(0.0, open));
            steps.push(GateStep::Ramp(1.0, open + GATE_RAMP_SEC));
        }
        None if from < 1.0 => steps.push(GateStep::Ramp(1.0, start + GATE_RAMP_SEC)),
        None => {}
    }
    steps
}

/// How open the gate is at `t` as `steps` play it, the way a browser plays
/// a gain's automation: held at a step's level until the next step, or
/// moved to the next along a straight line from the one before. Open
/// before the first.
#[cfg(any(feature = "web", test))]
pub fn gate_at(steps: &[GateStep], t: f64) -> f64 {
    let mut since: Option<(f64, f64)> = None;
    for step in steps {
        match (*step, since) {
            (GateStep::Set(level, at) | GateStep::Ramp(level, at), _) if at <= t => {
                since = Some((at, f64::from(level)));
            }
            (GateStep::Ramp(level, at), Some((from_at, from))) => {
                let through = (t - from_at) / (at - from_at);
                return from + (f64::from(level) - from) * through;
            }
            _ => break,
        }
    }
    since.map_or(1.0, |(_, level)| level)
}

/// The receiver's gate as the browser has been told to play it: the steps
/// put on its gains and not yet played, and the last one played, so a send
/// started on top goes on from exactly where the gate will stand, however
/// many sends ago its steps were scheduled.
#[cfg(any(feature = "web", test))]
#[derive(Debug, Default)]
pub struct GateSchedule(Vec<GateStep>);

#[cfg(any(feature = "web", test))]
impl GateSchedule {
    /// The steps to put on the gate's gains, at `now` on their clock, for a
    /// send starting at `start` and muted until `muted_until` seconds in, or
    /// for the gate opening there with nothing muted: what they were to play
    /// from `start` on is cancelled, and these go on from where that leaves
    /// the gate ([`gate_steps`]).
    pub fn schedule(&mut self, start: f64, muted_until: Option<f64>, now: f64) -> Vec<GateStep> {
        let from = gate_at(&self.0, start);
        self.0.retain(|step| step.at() < start);
        // What has played is no more use, but for the step the gate stands on.
        if let Some(standing) = self.0.iter().rposition(|step| step.at() <= now) {
            self.0.drain(..standing);
        }
        let steps = gate_steps(muted_until, start, from);
        self.0.extend_from_slice(&steps);
        steps
    }

    /// How open the gate will be at `t`, as it stands scheduled.
    #[cfg(test)]
    fn at(&self, t: f64) -> f64 {
        gate_at(&self.0, t)
    }
}

/// The release shape — a raised cosine, the same family the keying envelope
/// uses, so a cut send tails off like the end of a dit rather than a
/// fade-out. Full at `progress` 0, silent at 1.
#[cfg(any(feature = "native-audio", test))]
pub fn release_gain(progress: f64) -> f64 {
    let p = progress.clamp(0.0, 1.0);
    0.5 * (1.0 + (std::f64::consts::PI * p).cos())
}

/// The gain of a band fading in, `progress` of the way through: the rising
/// half of an equal-power crossfade, whose falling half is the same curve
/// read backwards.
///
/// Two bands are two unrelated noises, so their powers add, not their
/// amplitudes. Gains whose squares sum to one keep the hiss level through
/// the change; a straight-line crossfade sags 3 dB in the middle of it.
pub fn fade_in_gain(progress: f64) -> f64 {
    (std::f64::consts::FRAC_PI_2 * progress.clamp(0.0, 1.0)).sin()
}

/// [`fade_in_gain`] as `points` values from start to end, for a browser gain
/// to play as a curve: rising, or falling from `from` to silence.
#[cfg(any(feature = "web", test))]
pub fn fade_curve(points: usize, rising: bool, from: f64) -> Vec<f32> {
    let points = points.max(2);
    let last = (points - 1) as f64;
    (0..points)
        .map(|i| {
            let progress = i as f64 / last;
            let gain = if rising {
                fade_in_gain(progress)
            } else {
                from * fade_in_gain(1.0 - progress)
            };
            gain as f32
        })
        .collect()
}

/// What the paddle's sidetone is asked to play, held to what either player
/// will play: a pitch inside the side-tone range the settings allow, and a
/// level no louder than full scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneRequest {
    pub on: bool,
    pub hz: f32,
    pub gain: f32,
}

impl ToneRequest {
    pub fn new(on: bool, frequency_hz: f64, gain: f64) -> Self {
        let hz = if frequency_hz.is_finite() {
            frequency_hz.clamp(SIDE_TONE_MIN_HZ, SIDE_TONE_MAX_HZ)
        } else {
            SIDE_TONE_MIN_HZ
        };
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            on,
            hz: hz as f32,
            gain: gain as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_shape_runs_from_full_to_silent() {
        assert_eq!(release_gain(0.0), 1.0);
        assert_eq!(release_gain(1.0), 0.0);
        assert!(release_gain(0.5) > 0.4 && release_gain(0.5) < 0.6);
        assert_eq!(release_gain(-1.0), 1.0, "out of range is still in range");
        assert_eq!(release_gain(2.0), 0.0);
        let mut previous = 2.0;
        for step in 0..=20 {
            let value = release_gain(f64::from(step) / 20.0);
            assert!(value < previous, "the release rose at {step}");
            previous = value;
        }
    }

    /// Two unrelated noises crossfaded keep their power all the way through:
    /// the outgoing gain is the incoming one read backwards, and their
    /// squares sum to one at every point.
    #[test]
    fn a_band_crossfade_keeps_the_power() {
        for step in 0..=100 {
            let p = f64::from(step) / 100.0;
            let (out, into) = (fade_in_gain(1.0 - p), fade_in_gain(p));
            assert!((out * out + into * into - 1.0).abs() < 1e-12, "at {p}");
        }
        assert_eq!(fade_in_gain(0.0), 0.0);
        assert_eq!(fade_in_gain(1.0), 1.0);
        assert_eq!(fade_in_gain(-1.0), 0.0);
        assert_eq!(fade_in_gain(2.0), 1.0);

        let rising = fade_curve(5, true, 1.0);
        let falling = fade_curve(5, false, 0.5);
        assert_eq!(rising.first(), Some(&0.0));
        assert_eq!(rising.last(), Some(&1.0));
        assert_eq!(falling.first(), Some(&0.5));
        assert_eq!(falling.last(), Some(&0.0));
        assert!(rising.windows(2).all(|w| w[1] > w[0]));
        assert!(falling.windows(2).all(|w| w[1] < w[0]));
        assert_eq!(fade_curve(0, true, 1.0).len(), 2);
    }

    /// The receiver's gate shuts over the ramp from where your message
    /// starts, stays shut to its end and opens over the ramp after it,
    /// never stepping; a send taking over from one still shut starts shut
    /// and stays so; with nothing to mute, it stays open, or opens.
    #[test]
    fn the_gate_shuts_while_you_send_and_never_steps() {
        let steps = gate_steps(Some(2.0), 10.0, 1.0);
        assert_eq!(gate_at(&steps, 9.9), 1.0);
        assert_eq!(gate_at(&steps, 10.0), 1.0);
        assert!((gate_at(&steps, 10.0 + GATE_RAMP_SEC / 2.0) - 0.5).abs() < 1e-9);
        assert_eq!(gate_at(&steps, 10.0 + GATE_RAMP_SEC), 0.0);
        assert_eq!(gate_at(&steps, 11.5), 0.0);
        assert_eq!(gate_at(&steps, 12.0), 0.0);
        assert!((gate_at(&steps, 12.0 + GATE_RAMP_SEC / 2.0) - 0.5).abs() < 1e-9);
        assert_eq!(gate_at(&steps, 12.1), 1.0);
        assert_eq!(worst_step(&steps, 9.0, 13.0), 0.2, "a fifth a millisecond");
        // Taking over from a send still shut: shut from the first moment.
        let over = gate_steps(Some(1.0), 20.0, 0.0);
        assert_eq!(gate_at(&over, 20.0), 0.0);
        assert_eq!(gate_at(&over, 20.0 + GATE_RAMP_SEC / 2.0), 0.0);
        assert_eq!(gate_at(&over, 21.5), 1.0);
        // Nothing to mute: open, or opening from where it stood.
        assert_eq!(gate_steps(None, 5.0, 1.0), vec![GateStep::Ramp(1.0, 5.0)]);
        let opening = gate_steps(None, 5.0, 0.0);
        assert!((gate_at(&opening, 5.0 + GATE_RAMP_SEC / 2.0) - 0.5).abs() < 1e-9);
        assert_eq!(gate_at(&opening, 5.1), 1.0);
    }

    /// The biggest move the gate makes in a millisecond between `from` and
    /// `to`, to a thousandth.
    fn worst_step(steps: &[GateStep], from: f64, to: f64) -> f64 {
        let moves = |t: f64| (gate_at(steps, t + 0.001) - gate_at(steps, t)).abs();
        let worst = (0..((to - from) * 10_000.0) as u32)
            .map(|k| moves(from + f64::from(k) / 10_000.0))
            .fold(0.0, f64::max);
        (worst * 1_000.0).round() / 1_000.0
    }

    /// A send scheduled on top of another goes on from where the gate
    /// stands as that one left it, never stepping: a continuation taking
    /// over from your message keeps it shut; a second taking over from the
    /// first at its own start, a moment either side of it, keeps it shut
    /// too; and a stop opens it from wherever it stands, a ramp it cut into
    /// included.
    #[test]
    fn a_send_on_top_goes_on_from_where_the_gate_stands() {
        let mut gate = GateSchedule::default();
        let first = gate.schedule(10.0, Some(2.0), 9.9);
        assert_eq!(first, gate_steps(Some(2.0), 10.0, 1.0));
        // A continuation half way through your message, shut from its start.
        let taken = gate.schedule(11.0, Some(1.5), 10.9);
        assert_eq!(taken[0], GateStep::Ramp(0.0, 11.0));
        for t in [10.5, 11.0, 11.2, 12.4] {
            assert_eq!(gate.at(t), 0.0, "at {t}");
        }
        assert_eq!(gate.at(12.6), 1.0);
        // Taken over again at its own start, or a millisecond either side.
        for again in [10.999, 11.0, 11.001] {
            let mut gate = GateSchedule::default();
            gate.schedule(10.0, Some(2.0), 9.9);
            gate.schedule(11.0, Some(1.5), 10.9);
            let steps = gate.schedule(again, Some(1.0), 10.95);
            assert_eq!(steps[0], GateStep::Ramp(0.0, again), "at {again}");
            assert_eq!(gate.at(again + 0.5), 0.0);
            assert_eq!(gate.at(again + 1.01), 1.0);
        }
        // A stop just as the gate is shutting opens it from part way down.
        let mut gate = GateSchedule::default();
        gate.schedule(10.0, Some(2.0), 9.9);
        let half = 10.0 + GATE_RAMP_SEC / 2.0;
        let open = gate.schedule(half, None, half);
        assert_eq!(open[0], GateStep::Ramp(0.5, half));
        assert!((gate.at(half) - 0.5).abs() < 1e-9);
        assert!((gate.at(half + GATE_RAMP_SEC / 2.0) - 0.75).abs() < 1e-9);
        assert_eq!(gate.at(half + GATE_RAMP_SEC), 1.0);
        assert!(worst_step(&gate.0, 9.0, 11.0) <= 0.2);
        // What has played is let go, but for the step the gate stands on.
        gate.schedule(20.0, Some(1.0), 19.9);
        assert!(gate.0.len() <= 5, "{:?}", gate.0);
        assert_eq!(gate.at(19.95), 1.0);
    }

    /// One rule for the sidetone's range on both players.
    #[test]
    fn the_sidetone_is_held_to_the_side_tone_range() {
        let tone = ToneRequest::new(true, 600.0, 0.4);
        assert_eq!((tone.hz, tone.gain), (600.0, 0.4));
        assert_eq!(
            ToneRequest::new(true, 20.0, 2.0).hz,
            SIDE_TONE_MIN_HZ as f32
        );
        assert_eq!(ToneRequest::new(true, 9e9, 2.0).gain, 1.0);
        assert_eq!(
            ToneRequest::new(true, 5_000.0, 0.1).hz,
            SIDE_TONE_MAX_HZ as f32
        );
        let nonsense = ToneRequest::new(true, f64::NAN, f64::NAN);
        assert!(nonsense.hz.is_finite() && nonsense.gain == 0.0);
    }
}
