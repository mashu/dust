//! One send's tones, scheduled on the context clock.

use std::sync::Arc;

use cw_core::band::Qsb;
use cw_core::{PlannedTransmission, ToneEvent};
use web_sys::{AudioContext, GainNode, OscillatorType};

use super::param::{Curve, ramp_to};
use crate::audio::shape::SEND_RELEASE_SEC;

/// A scheduled send: every station's tones through one gain, which the
/// fading rides and a stop ramps down; and, in a contest, your message
/// through a gain of its own, past the receiver.
pub struct ScheduledSend {
    gain: GainNode,
    /// Your message, when the plan says it is dry.
    dry: Option<GainNode>,
}

/// A gain for a send's tones, connected into `into`.
fn send_gain(ctx: &AudioContext, into: &GainNode) -> Result<GainNode, String> {
    let gain = ctx.create_gain().map_err(|e| format!("gain: {e:?}"))?;
    gain.connect_with_audio_node(into)
        .map_err(|e| format!("connect: {e:?}"))?;
    Ok(gain)
}

impl ScheduledSend {
    /// Schedule every station's tones from `start` on the context clock,
    /// into `into`, the receiver's input; a dry `wanted` goes into
    /// `dry_into`, past the receiver.
    pub fn schedule(
        ctx: &AudioContext,
        into: &GainNode,
        dry_into: &GainNode,
        planned: &PlannedTransmission,
        fading: Option<Qsb>,
        start: f64,
    ) -> Result<Self, String> {
        let gain = send_gain(ctx, into)?;
        let dry = planned
            .dry_wanted
            .then(|| send_gain(ctx, dry_into))
            .transpose()
            .inspect_err(|_| {
                let _ = gain.disconnect();
            })?;
        let send = Self { gain, dry };
        // A send the browser will not finish building is taken out whole,
        // rather than left playing whatever tones it did get.
        if let Err(err) = schedule_into(ctx, &send.gain, send.dry.as_ref(), planned, fading, start)
        {
            for gain in send.gains() {
                let _ = gain.disconnect();
            }
            return Err(err);
        }
        Ok(send)
    }

    /// Ramp the send down from `at` on the context clock, now or ahead,
    /// instead of cutting it, and hand back its gains with the context time
    /// they are silent by: ahead for a send that a continuation takes over
    /// from, which plays on to there.
    ///
    /// Disconnecting on the spot is what made the old stop click: the fade
    /// was scheduled and then the node that would have played it was taken
    /// out of the graph in the same breath. The gains stay connected until
    /// the ramp has run.
    pub fn release_at(self, ctx: &AudioContext, at: f64) -> (impl Iterator<Item = GainNode>, f64) {
        let at = at.max(ctx.current_time());
        let done = at + SEND_RELEASE_SEC;
        for gain in self.gains() {
            ramp_to(&gain.gain(), 0.0, at, done);
        }
        (std::iter::once(self.gain).chain(self.dry), done)
    }

    fn gains(&self) -> impl Iterator<Item = &GainNode> {
        std::iter::once(&self.gain).chain(&self.dry)
    }
}

/// The fading on `gain`, and every station's tones into it, from `start`:
/// all but a dry `wanted`, which goes into `dry` unfaded.
fn schedule_into(
    ctx: &AudioContext,
    gain: &GainNode,
    dry: Option<&GainNode>,
    planned: &PlannedTransmission,
    fading: Option<Qsb>,
    start: f64,
) -> Result<(), String> {
    // The fading, read off the context's clock as the native player reads it
    // off its own: a send drops into a fade already in progress rather than
    // starting one. The curve is the shared fade, sampled — nothing else the
    // graph can build stays inside its range.
    if let Some(fading) = fading {
        let length = planned.wanted.duration_sec.max(0.01);
        let curve = fading.curve(start, length);
        Curve::new(&curve)
            .play_on(&gain.gain(), start, length)
            .map_err(|e| format!("fading: {e:?}"))?;
    }
    // The wanted station and everyone calling over it go into the same gain:
    // interference is addition, and each station's events already carry its
    // own pitch and level. Your own message is no station the receiver
    // hears, and goes around it.
    let every_station = std::iter::once((&planned.wanted, dry.unwrap_or(gain)))
        .chain(planned.others.iter().map(|plan| (plan, gain)));
    let mut envelopes = Envelopes::default();
    for (plan, into) in every_station {
        for event in &plan.events {
            schedule_tone(ctx, into, event, &mut envelopes, start + event.start_sec)?;
        }
    }
    Ok(())
}

/// The keying envelopes of one send's tones, each made into a [`Curve`] the
/// first time a tone is keyed with it.
///
/// Every dit a station sends has the same envelope, and so does every dah,
/// so the tones of a send share a few curves between them, told apart by
/// where they live. Kept for one send only: the plan is borrowed for as long
/// as they are, so none of them can go and leave its address to another
/// envelope — which is just what one kept from an earlier send could do.
#[derive(Default)]
struct Envelopes<'p>(Vec<(&'p Arc<[f32]>, Curve<'p>)>);

impl<'p> Envelopes<'p> {
    /// The curve for `envelope`, made the first time it is asked for.
    fn curve(&mut self, envelope: &'p Arc<[f32]>) -> &Curve<'p> {
        let known = self
            .0
            .iter()
            .position(|(seen, _)| Arc::ptr_eq(seen, envelope));
        let at = known.unwrap_or_else(|| {
            self.0.push((envelope, Curve::new(envelope)));
            self.0.len() - 1
        });
        &self.0[at].1
    }
}

/// One element: an oscillator through its own keying envelope, from `t0`.
fn schedule_tone<'p>(
    ctx: &AudioContext,
    into: &GainNode,
    event: &'p ToneEvent,
    envelopes: &mut Envelopes<'p>,
    t0: f64,
) -> Result<(), String> {
    let osc = ctx.create_oscillator().map_err(|e| format!("osc: {e:?}"))?;
    let element = ctx.create_gain().map_err(|e| format!("sym gain: {e:?}"))?;
    osc.set_type(OscillatorType::Sine);
    osc.frequency().set_value(event.frequency_hz as f32);
    osc.connect_with_audio_node(&element)
        .map_err(|e| format!("osc connect: {e:?}"))?;
    element
        .connect_with_audio_node(into)
        .map_err(|e| format!("gain connect: {e:?}"))?;
    let param = element.gain();
    let _ = param.set_value_at_time(0.0, t0);
    if event.envelope.len() >= 2
        && envelopes
            .curve(&event.envelope)
            .play_on(&param, t0, event.duration_sec)
            .is_err()
    {
        let rise = event.duration_sec.min(0.02);
        let _ = param.linear_ramp_to_value_at_time(event.target_gain as f32, t0 + rise);
        let _ = param.set_value_at_time(event.target_gain as f32, t0 + event.duration_sec - rise);
        let _ = param.linear_ramp_to_value_at_time(0.0, t0 + event.duration_sec);
    }
    osc.start_with_when(t0)
        .map_err(|e| format!("start: {e:?}"))?;
    osc.stop_with_when(t0 + event.duration_sec)
        .map_err(|e| format!("stop: {e:?}"))?;
    Ok(())
}
