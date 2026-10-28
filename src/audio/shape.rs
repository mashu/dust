//! What both players share about how a change sounds: how long a stop, a
//! band change and the receiver's gate take, and the shapes they take. Kept
//! in one place so the desktop and the browser cannot drift apart — values
//! that had to match used to be copied by hand.

use cw_core::TrainingSettings;

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

/// The gains a player plays at: the receiver's RF gain, on everything
/// reaching its AGC; the AF gain full volume lifts what the receiver plays
/// by after its AGC, sized to that receiver; and the volume, which turns
/// that down, and your own messages and the paddle's sidetone with it, as
/// they are set at the level they are heard at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gains {
    pub rf: f64,
    pub af: f64,
    pub volume: f64,
}

impl Gains {
    /// All at one: the band as it is calibrated, and your messages at their
    /// level.
    pub const UNITY: Self = Self {
        rf: 1.0,
        af: 1.0,
        volume: 1.0,
    };

    pub fn of(settings: &TrainingSettings) -> Self {
        Self {
            rf: settings.rf_gain(),
            af: cw_core::band::full_volume_gain(settings),
            volume: settings.master_volume(),
        }
    }
}

/// One step of a gate as the browser plays it: moved to a level in a
/// straight line ending at a moment, from the step before.
#[cfg(any(feature = "web", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GateStep {
    Ramp(f32, f64),
}

#[cfg(test)]
impl GateStep {
    /// When it is reached, on the gate's clock.
    pub fn at(&self) -> f64 {
        match *self {
            Self::Ramp(_, at) => at,
        }
    }
}

/// How open the gate is at `t` as `steps` play it, the way a browser plays
/// a gain's automation: moved to each step's level along a straight line
/// from the one before, and held at the last. Open before the first.
#[cfg(test)]
pub fn gate_at(steps: &[GateStep], t: f64) -> f64 {
    let mut since: Option<(f64, f64)> = None;
    for step in steps {
        let GateStep::Ramp(level, at) = *step;
        match since {
            _ if at <= t => since = Some((at, f64::from(level))),
            Some((from_at, from)) => {
                let through = (t - from_at) / (at - from_at);
                return from + (f64::from(level) - from) * through;
            }
            None => break,
        }
    }
    since.map_or(1.0, |(_, level)| level)
}

/// The release shape — a raised cosine, the same family the keying envelope
/// uses, so a cut send tails off like the end of a dit rather than a
/// fade-out. Full at `progress` 0, silent at 1.
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

/// How many straight lines a browser draws a whole edge of the sidetone's
/// gate with: a fifth of a millisecond each at the default rise, close
/// enough to the curve that what is left over is 60 dB down.
#[cfg(any(feature = "web", test))]
const SIDETONE_EDGE_LINES: f64 = 32.0;

/// How long the key has to be quiet before the browser's sidetone takes a
/// fresh reading of where the keyer's clock stands on the context's.
#[cfg(any(feature = "web", test))]
const SIDETONE_QUIET_SEC: f64 = 0.5;

/// The most a burst of keying is ever played later than it was keyed for:
/// for the context's own unevenness, or for the first edge coming late.
/// Past this, an edge sounds as soon as it can.
#[cfg(any(feature = "web", test))]
const SIDETONE_LATE_MAX_SEC: f64 = 0.1;

/// One edge of the browser's sidetone gate, as scheduled.
#[cfg(any(feature = "web", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct GateEdge {
    /// When it was keyed, on the keyer's clock, and when it plays, on the
    /// context's.
    keyed: f64,
    at: f64,
    /// How far up the gate stood as it began, which way it went, and how
    /// long a whole edge takes.
    from: f64,
    on: bool,
    rise: f64,
}

#[cfg(any(feature = "web", test))]
impl GateEdge {
    /// How far up the gate stands at `t`.
    fn progress_at(&self, t: f64) -> f64 {
        let moved = ((t - self.at) / self.rise).max(0.0);
        if self.on {
            (self.from + moved).min(1.0)
        } else {
            (self.from - moved).max(0.0)
        }
    }
}

/// The paddle's sidetone gate as a browser is told to play it.
///
/// A browser renders a device buffer at a time, so a gain set "now" changes
/// at the start of whichever buffer comes next: every element and space a
/// whole number of buffers long. What it plays exactly is what it is told
/// ahead of time. So each edge goes on the context's clock a constant lead
/// after the instant the keyer meant it for: the keyer's clock is read
/// against the context's at the first edge after a quiet spell, and every
/// edge of that burst is that far on. Should the context have run further
/// ahead than the lead allows for, the rest of the burst moves along, and
/// the bursts after allow for as much; a burst whose first edge comes late
/// keeps its time from there; any other edge late sounds as soon as it
/// can, the rest on time.
///
/// Each edge is drawn as straight lines along the keying envelope's edge,
/// from wherever the gate stands as it begins, so one that turns the gate
/// back mid-edge never steps.
#[cfg(any(feature = "web", test))]
#[derive(Debug, Default)]
pub struct SidetoneGate {
    /// Context seconds less keyer seconds, the lead included, for this burst.
    offset: Option<f64>,
    /// How much further ahead than the lead the context has been seen to
    /// run, which each burst allows for.
    spread: f64,
    last: Option<GateEdge>,
}

#[cfg(any(feature = "web", test))]
impl SidetoneGate {
    /// The steps that play an edge keyed at `keyed` on the keyer's clock,
    /// the gate opening if `on` or closing, and when they begin: what was
    /// scheduled from then on is theirs to replace. None for an edge that
    /// changes nothing, the gate going the way it is going already.
    pub fn edge(
        &mut self,
        on: bool,
        keyed: f64,
        clocks: SidetoneClocks,
        shape: ToneShape,
    ) -> Option<(f64, Vec<GateStep>)> {
        let SidetoneClocks {
            now,
            current,
            soonest,
            lead,
        } = clocks;
        if self.last.map_or(!on, |last| last.on == on) {
            return None;
        }
        // A held key is not a quiet one: its edges keep the burst's reading.
        let quiet = self
            .last
            .is_none_or(|last| !last.on && keyed - last.keyed >= SIDETONE_QUIET_SEC);
        let mut offset = match self.offset {
            Some(offset) if !quiet => offset,
            _ => current - now + lead + self.spread,
        };
        // Where an edge keyed now would go, too soon: the context's clock
        // has run further ahead of the keyer's than the lead allows for.
        let short = soonest - (now + offset);
        if short > 0.0 {
            offset += short;
            self.spread = (self.spread + short).min(SIDETONE_LATE_MAX_SEC);
        }
        let mut at = keyed + offset;
        if at < soonest {
            if quiet && soonest - at <= SIDETONE_LATE_MAX_SEC {
                offset += soonest - at;
            }
            at = soonest;
        }
        self.offset = Some(offset);
        if let Some(last) = self.last {
            at = at.max(last.at);
        }
        let from = self.last.map_or(0.0, |last| last.progress_at(at));
        let rise = shape.rise_sec.max(1e-4);
        self.last = Some(GateEdge {
            keyed,
            at,
            from,
            on,
            rise,
        });
        Some((at, edge_steps(from, on, at, rise, shape.smoothing)))
    }
}

/// Where the keyer's clock and a browser's audio clock stand as an edge is
/// keyed: the keyer's now; the context's now, and the soonest it will
/// still play anything scheduled; and how far behind its instant an edge
/// is meant to sound.
#[cfg(any(feature = "web", test))]
#[derive(Clone, Copy, Debug)]
pub struct SidetoneClocks {
    pub now: f64,
    pub current: f64,
    pub soonest: f64,
    pub lead: f64,
}

/// How an edge of the sidetone's gate moves: how long a whole one takes,
/// and its shape ([`cw_core::edge_gain`]).
#[cfg(any(feature = "web", test))]
#[derive(Clone, Copy, Debug)]
pub struct ToneShape {
    pub rise_sec: f64,
    pub smoothing: f64,
}

/// The straight lines that draw the gate's edge from `from` of the way up,
/// opening if `on` or closing, starting at `at`: the first one to where it
/// stands then, which finishes whatever line was being drawn when the edge
/// came.
#[cfg(any(feature = "web", test))]
fn edge_steps(from: f64, on: bool, at: f64, rise: f64, smoothing: f64) -> Vec<GateStep> {
    let level = |p: f64| cw_core::edge_gain(p, smoothing) as f32;
    let target = if on { 1.0 } else { 0.0 };
    let distance = (target - from).abs();
    let lines = (distance * SIDETONE_EDGE_LINES).ceil() as usize;
    let mut steps = vec![GateStep::Ramp(level(from), at)];
    for k in 1..=lines {
        let through = k as f64 / lines as f64;
        let p = from + (target - from) * through;
        steps.push(GateStep::Ramp(level(p), at + distance * rise * through));
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A browser's automation, as it plays a gain: the steps scheduled on
    /// it, each edge's replacing whatever was to play from where it begins.
    #[derive(Default)]
    struct Automation(Vec<GateStep>);

    impl Automation {
        fn schedule(&mut self, (at, steps): (f64, Vec<GateStep>)) {
            self.0.retain(|step| step.at() < at);
            self.0.extend(steps);
        }

        /// The gain at `t`, shut before anything was scheduled.
        fn at(&self, t: f64) -> f64 {
            let mut steps = vec![GateStep::Ramp(0.0, f64::MIN)];
            steps.extend_from_slice(&self.0);
            gate_at(&steps, t)
        }
    }

    const QUANTUM: f64 = 128.0 / 48_000.0;
    const SHAPE: ToneShape = ToneShape {
        rise_sec: 0.006,
        smoothing: 0.75,
    };

    /// A browser rendering 10 ms at a time, its clock `ahead` of the
    /// keyer's: where both stand at `now`, with a lead that allows for one
    /// buffer.
    fn clocks(now: f64, ahead: f64) -> SidetoneClocks {
        let period = 0.01;
        let current = ((now + ahead) / period).floor() * period;
        SidetoneClocks {
            now,
            current,
            soonest: current + QUANTUM,
            lead: period + QUANTUM + 0.002,
        }
    }

    /// Edges keyed 37 ms apart are played 37 ms apart, to the sample, though
    /// the browser's clock moves 10 ms at a time and each edge reaches it a
    /// little late; and none is ever played sooner than it can be.
    #[test]
    fn the_browser_sidetone_keeps_the_keyers_time() {
        let mut gate = SidetoneGate::default();
        let mut placed = Vec::new();
        for k in 0..40 {
            let keyed = 1.0 + f64::from(k) * 0.037;
            let now = keyed + f64::from(k % 7) * 0.000_15;
            let clocks = clocks(now, 0.3);
            let (at, _) = gate
                .edge(k % 2 == 0, keyed, clocks, SHAPE)
                .expect("each edge turns the gate");
            assert!(at >= clocks.soonest, "edge {k} too soon");
            placed.push(at);
        }
        for pair in placed.windows(2) {
            assert!((pair[1] - pair[0] - 0.037).abs() < 1e-9, "{placed:?}");
        }
    }

    /// A burst whose first edge came late keeps its time from there; the
    /// next, keyed on time after a quiet spell, is as late as the lead and
    /// no later.
    #[test]
    fn a_late_burst_keeps_its_time_and_the_next_is_on_time() {
        let mut gate = SidetoneGate::default();
        let edge = |gate: &mut SidetoneGate, on: bool, keyed: f64, now: f64| {
            gate.edge(on, keyed, clocks(now, 0.3), SHAPE)
                .expect("an edge")
                .0
        };
        let on = edge(&mut gate, true, 1.0, 1.04);
        let off = edge(&mut gate, false, 1.06, 1.06);
        assert!((off - on - 0.06).abs() < 1e-9, "{on} then {off}");
        let on = edge(&mut gate, true, 3.0, 3.0);
        let latest = clocks(3.0, 0.3);
        assert!(
            on - latest.current <= latest.lead + 1e-9,
            "{on} against {latest:?}"
        );
    }

    /// An edge that turns the gate back mid-edge goes on from where it
    /// stands, never stepping, along the envelope's edge; a whole edge takes
    /// the rise, ends where it was going, and never turns back.
    #[test]
    fn the_browser_sidetone_never_steps() {
        let mut gate = SidetoneGate::default();
        let mut gain = Automation::default();
        let shape = ToneShape {
            rise_sec: 0.008,
            smoothing: 1.0,
        };
        let keyed = [(true, 1.0), (false, 1.002), (true, 1.003), (false, 1.2)];
        for (on, keyed) in keyed {
            gain.schedule(
                gate.edge(on, keyed, clocks(keyed, 0.3), shape)
                    .expect("an edge"),
            );
        }
        assert!(
            gate.edge(false, 1.3, clocks(1.3, 0.3), shape).is_none(),
            "shut already"
        );
        let start = gain.0[0].at();
        let mut last = 0.0;
        let mut biggest = 0.0f64;
        for i in 0..48_000 {
            let value = gain.at(start - 0.01 + f64::from(i) / 48_000.0);
            biggest = biggest.max((value - last).abs());
            last = value;
        }
        assert!(biggest < 0.01, "a step of {biggest}");

        let (at, steps) = SidetoneGate::default()
            .edge(true, 1.0, clocks(1.0, 0.3), SHAPE)
            .expect("an edge");
        let levels: Vec<f32> = steps
            .iter()
            .map(|step| match step {
                GateStep::Ramp(level, _) => *level,
            })
            .collect();
        assert!(levels.windows(2).all(|w| w[1] >= w[0]), "{levels:?}");
        assert_eq!((levels[0], levels[levels.len() - 1]), (0.0, 1.0));
        let took = steps.last().expect("steps").at() - at;
        assert!((took - SHAPE.rise_sec).abs() < 1e-9, "{took}");
    }

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
    }
}
