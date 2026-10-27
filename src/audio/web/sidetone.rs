//! The paddle's sidetone: one oscillator, gated along the keying envelope's
//! edge, each edge played a constant lead after the instant it was keyed
//! ([`SidetoneGate`]).

use web_sys::{AudioContext, AudioParam, GainNode, OscillatorNode, OscillatorType};

use super::param::ramp_to;
use super::reported_latency;
use crate::audio::ToneRequest;
use crate::audio::shape::{GATE_RAMP_SEC, GateStep, SidetoneClocks, SidetoneGate, ToneShape};
use crate::audio::tone::SIDETONE_LEAD_MARGIN_SEC;

/// A browser renders this many frames at a time, at the least: nothing
/// scheduled sooner than one of them ahead of its clock is sure to play as
/// it was scheduled.
const RENDER_QUANTUM: f64 = 128.0;

/// The sidetone's oscillator, its gate and its level, made the first time a
/// key asks for them and kept running, so a squeeze is a gain change, not
/// a node start.
#[derive(Default)]
pub struct Sidetone {
    running: Option<Nodes>,
    gate: SidetoneGate,
    /// The level last asked for, and the edge's shape.
    level: f32,
    shape: Option<ToneShape>,
}

struct Nodes {
    osc: OscillatorNode,
    gate: GainNode,
    level: GainNode,
}

impl Sidetone {
    /// Play `tone` into `into`: its level from now, and the gate opening or
    /// closing at the edge it was keyed for.
    pub fn set(
        &mut self,
        ctx: &AudioContext,
        into: &GainNode,
        tone: ToneRequest,
    ) -> Result<(), String> {
        if self.running.is_none() {
            self.running = Some(start(ctx, into)?);
        }
        let Some(nodes) = &self.running else {
            return Ok(());
        };
        let current = ctx.current_time();
        if tone.gain != self.level {
            // A volume moved, not a key: eased, as the receiver's gate is.
            ramp_to(
                &nodes.level.gain(),
                tone.gain,
                current,
                current + GATE_RAMP_SEC,
            );
            self.level = tone.gain;
        }
        let shape = ToneShape {
            rise_sec: f64::from(tone.rise_sec),
            smoothing: f64::from(tone.smoothing),
        };
        self.shape = Some(shape);
        let now = performance_now_sec();
        let clocks = clocks(ctx, now, current);
        if let Some((at, steps)) = self.gate.edge(tone.on, now - tone.ago_sec, clocks, shape) {
            if tone.on {
                let _ = nodes.osc.frequency().set_value_at_time(tone.hz, at);
            }
            play(&nodes.gate.gain(), at, &steps);
        }
        Ok(())
    }

    /// Close the gate as soon as it can be closed, along its edge.
    pub fn silence(&mut self, ctx: &AudioContext) {
        let (Some(nodes), Some(shape)) = (&self.running, self.shape) else {
            return;
        };
        let now = performance_now_sec();
        let clocks = clocks(ctx, now, ctx.current_time());
        if let Some((at, steps)) = self.gate.edge(false, now, clocks, shape) {
            play(&nodes.gate.gain(), at, &steps);
        }
    }
}

impl Drop for Sidetone {
    fn drop(&mut self) {
        if let Some(nodes) = self.running.take() {
            let _ = nodes.osc.stop();
        }
    }
}

/// The keyer's clock, which is the page's: `performance.now()`, in seconds.
fn performance_now_sec() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now() / 1_000.0)
}

/// Where the keyer's clock, at `now`, and the context's, at `current`,
/// stand. The context runs ahead of what it has played by its base latency,
/// one device buffer, in a jump at a time; an edge is meant to sound a
/// buffer and the margin after its instant, so it is in hand before the
/// buffer it falls in is rendered.
fn clocks(ctx: &AudioContext, now: f64, current: f64) -> SidetoneClocks {
    let quantum = RENDER_QUANTUM / f64::from(ctx.sample_rate()).max(1.0);
    let buffer = reported_latency(ctx, "baseLatency").max(quantum);
    SidetoneClocks {
        now,
        current,
        soonest: current + quantum,
        lead: buffer + quantum + SIDETONE_LEAD_MARGIN_SEC,
    }
}

/// Put `steps` on `param` from `at`, in place of whatever was to play from
/// then: their first finishes the line being drawn when they begin.
fn play(param: &AudioParam, at: f64, steps: &[GateStep]) {
    let _ = param.cancel_scheduled_values(at);
    for step in steps {
        let _ = match *step {
            GateStep::Set(level, when) => param.set_value_at_time(level, when),
            GateStep::Ramp(level, when) => param.linear_ramp_to_value_at_time(level, when),
        };
    }
}

fn start(ctx: &AudioContext, into: &GainNode) -> Result<Nodes, String> {
    let osc = ctx
        .create_oscillator()
        .map_err(|e| format!("live osc: {e:?}"))?;
    let gate = ctx.create_gain().map_err(|e| format!("live gain: {e:?}"))?;
    let level = ctx
        .create_gain()
        .map_err(|e| format!("live level: {e:?}"))?;
    osc.set_type(OscillatorType::Sine);
    let now = ctx.current_time();
    osc.frequency()
        .set_value_at_time(500.0, now)
        .map_err(|e| format!("live freq: {e:?}"))?;
    for gain in [&gate, &level] {
        gain.gain()
            .set_value_at_time(0.0, now)
            .map_err(|e| format!("live mute: {e:?}"))?;
    }
    osc.connect_with_audio_node(&gate)
        .map_err(|e| format!("live osc connect: {e:?}"))?;
    gate.connect_with_audio_node(&level)
        .map_err(|e| format!("live gate connect: {e:?}"))?;
    level
        .connect_with_audio_node(into)
        .map_err(|e| format!("live out: {e:?}"))?;
    osc.start().map_err(|e| format!("live start: {e:?}"))?;
    Ok(Nodes { osc, gate, level })
}
