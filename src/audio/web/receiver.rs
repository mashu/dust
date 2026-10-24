//! The receiver's filter in nodes, retuned by crossfading to a new bank that
//! is built a node at a time while the old one plays on.

use std::cell::RefCell;
use std::rc::Rc;

use cw_core::TrainingSettings;
use cw_core::band::{FilterDesign, MAX_RECEIVER_SECTIONS, Section};
use gloo_timers::callback::Timeout;
use web_sys::{AudioContext, GainNode, IirFilterNode};

use super::chain::Chain;
use super::param::{Retiring, Unplug, ramp_to};
use crate::audio::shape::BAND_FADE_SEC;

/// How long a new design has to stand before a bank is built for it. A
/// filter slider being dragged asks for a new design every few tens of
/// milliseconds; building for each would throw most of them away half made.
const SETTLE_MS: u32 = 150;

/// The pause between one node of a bank being built and the next: enough for
/// the page to draw and take input in between.
const BUILD_TICK_MS: u32 = 1;

/// The receiver's filter, as it stands: the bank playing, the banks a retune
/// replaced, fading out, and the bank a retune is building.
///
/// A browser takes its time over an `IIRFilterNode`: Chromium works out how
/// long each one rings for as it is made, 35 to 50 ms a node for these
/// resonators, and a bank is eight or nine of them. Made all at once on
/// every retune, that held the page for a third of a second at every step of
/// the filter slider, the sends due meanwhile included. So the first bank is
/// built on the spot — there is nothing to play through until it is — and
/// every bank after it a node at a time, once the design asked for has
/// settled, with the old one playing on until the new one is whole.
pub struct Receiver {
    state: Rc<RefCell<State>>,
}

struct State {
    bank: Option<ReceiverBank>,
    retired: Retiring<ReceiverBank>,
    /// The bank being built for the design asked for last, if it is not the
    /// one playing.
    pending: Option<Pending>,
    /// Moved on whenever what is pending changes, or the receiver goes: a
    /// build step scheduled for anything else finds it moved and does
    /// nothing.
    generation: u64,
}

/// A bank on its way: the design, the sections still to make, and the bank
/// as far as it has got.
struct Pending {
    design: FilterDesign,
    sections: Vec<Section>,
    built: Option<ReceiverBank>,
}

/// One `IIRFilterNode` per resonator of the design, side by side between the
/// mix and a gain that sums them — the native player's
/// [`cw_core::band::ReceiverFilter`], node for resonator, from the same
/// coefficients.
///
/// `IIRFilterNode`s rather than `BiquadFilterNode`s, because only they take
/// the numerator the design asks for: a band-pass biquad's is fixed.
///
/// Its coefficients are fixed once it is made, so a retune builds a new bank
/// and crossfades to it rather than moving the old one.
struct ReceiverBank {
    /// What it was built from.
    design: FilterDesign,
    /// The mix it is fed from.
    input: GainNode,
    sections: Vec<IirFilterNode>,
    /// Sums the sections. Ramped in and out across a retune.
    output: GainNode,
}

impl Receiver {
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(State {
                bank: None,
                retired: Retiring::new(),
                pending: None,
                generation: 0,
            })),
        }
    }

    /// Tune the receiver to the current pitch, width and shape.
    ///
    /// Runs on every band change, and costs a comparison when nothing has
    /// moved. The first time, the bank is built there and then. After that,
    /// a design that differs from the one playing is built in the background
    /// ([`SETTLE_MS`] after it was last asked for, a node a tick), and once
    /// it is whole the old bank fades out over [`BAND_FADE_SEC`] while the
    /// new one fades in, then is taken out of the graph. In straight lines:
    /// both banks are filtering the same sound, so their outputs add as
    /// amplitudes, not powers. Asking for the design playing again drops
    /// whatever was being built.
    pub fn tune(
        &mut self,
        ctx: &AudioContext,
        chain: &Chain,
        settings: &TrainingSettings,
    ) -> Result<(), String> {
        let design = FilterDesign::from_settings(settings);
        let mut state = self.state.borrow_mut();
        state.retired.collect(ctx.current_time());
        let Some(playing) = state.bank.as_ref().map(|bank| bank.design.clone()) else {
            let bank = ReceiverBank::whole(ctx, &chain.mix, &chain.receiver_out, &design)?;
            let _ = bank
                .output
                .gain()
                .set_value_at_time(1.0, ctx.current_time());
            state.bank = Some(bank);
            return Ok(());
        };
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.design == design)
        {
            return Ok(());
        }
        state.drop_pending();
        if playing == design {
            return Ok(());
        }
        state.pending = Some(Pending {
            sections: design
                .sections(f64::from(ctx.sample_rate()))
                .into_iter()
                .collect(),
            design,
            built: None,
        });
        let generation = state.generation;
        drop(state);
        schedule_step(
            Rc::clone(&self.state),
            generation,
            ctx.clone(),
            chain.mix.clone(),
            chain.receiver_out.clone(),
            SETTLE_MS,
        );
        Ok(())
    }
}

impl Drop for Receiver {
    /// Whatever was being built is let go of, and no step still to come
    /// builds any more of it.
    fn drop(&mut self) {
        self.state.borrow_mut().drop_pending();
    }
}

impl State {
    /// Unplug whatever was being built, and call off the steps to come.
    fn drop_pending(&mut self) {
        self.generation += 1;
        if let Some(bank) = self.pending.take().and_then(|pending| pending.built) {
            bank.unplug();
        }
    }
}

/// Build one more node of the pending bank `after_ms` from now, if it is
/// still `generation`'s; and once the bank is whole, put it in place of the
/// one playing.
fn schedule_step(
    state: Rc<RefCell<State>>,
    generation: u64,
    ctx: AudioContext,
    mix: GainNode,
    into: GainNode,
    after_ms: u32,
) {
    Timeout::new(after_ms, move || {
        let more = step(&state, generation, &ctx, &mix, &into);
        if more {
            schedule_step(state, generation, ctx, mix, into, BUILD_TICK_MS);
        }
    })
    .forget();
}

/// One step of building the pending bank: a node, or the swap once there
/// are no more to make. Says whether there is more to do. A node the
/// browser will not make leaves the bank playing as it was.
fn step(
    state: &Rc<RefCell<State>>,
    generation: u64,
    ctx: &AudioContext,
    mix: &GainNode,
    into: &GainNode,
) -> bool {
    let mut state = state.borrow_mut();
    if state.generation != generation {
        return false;
    }
    let Some(pending) = state.pending.as_mut() else {
        return false;
    };
    let made = match pending.built.as_mut() {
        None => ReceiverBank::empty(ctx, mix, &pending.design).map(|bank| {
            pending.built = Some(bank);
        }),
        Some(bank) => match pending.sections.get(bank.sections.len()).copied() {
            Some(section) => bank.add_section(ctx, section),
            None => {
                let finished = state.pending.take().and_then(|pending| pending.built);
                if let Some(bank) = finished {
                    state.swap_in(ctx, bank, into);
                }
                return false;
            }
        },
    };
    if made.is_err() {
        state.drop_pending();
        return false;
    }
    true
}

impl State {
    /// Fade `bank` in over the one playing, which fades out and is let go of
    /// once it has.
    fn swap_in(&mut self, ctx: &AudioContext, bank: ReceiverBank, into: &GainNode) {
        if bank.connect(into).is_err() {
            bank.unplug();
            return;
        }
        let now = ctx.current_time();
        let done = now + BAND_FADE_SEC;
        let gain = bank.output.gain();
        let _ = gain.set_value_at_time(0.0, now);
        let _ = gain.linear_ramp_to_value_at_time(1.0, done);
        if let Some(old) = self.bank.replace(bank) {
            ramp_to(&old.output.gain(), 0.0, now, done);
            self.retired.retire(old, done);
        }
    }
}

impl ReceiverBank {
    /// A bank for `design`, fed from `input`, with no sections yet and its
    /// output gain silent and connected to nothing.
    fn empty(ctx: &AudioContext, input: &GainNode, design: &FilterDesign) -> Result<Self, String> {
        let output = ctx
            .create_gain()
            .map_err(|e| format!("receiver output: {e:?}"))?;
        output
            .gain()
            .set_value_at_time(0.0, ctx.current_time())
            .map_err(|e| format!("receiver output set: {e:?}"))?;
        Ok(Self {
            design: design.clone(),
            input: input.clone(),
            // Only the design's own sections — eight, or nine with the peak
            // filter — each one node.
            sections: Vec::with_capacity(MAX_RECEIVER_SECTIONS),
            output,
        })
    }

    /// A bank for `design` with every section, fed from `input` and summed
    /// into `into`, its output gain silent; built on the spot. One the
    /// browser will not finish is taken out whole.
    fn whole(
        ctx: &AudioContext,
        input: &GainNode,
        into: &GainNode,
        design: &FilterDesign,
    ) -> Result<Self, String> {
        let mut bank = Self::empty(ctx, input, design)?;
        let built = design
            .sections(f64::from(ctx.sample_rate()))
            .into_iter()
            .try_for_each(|section| bank.add_section(ctx, section))
            .and_then(|()| bank.connect(into));
        match built {
            Ok(()) => Ok(bank),
            Err(e) => {
                bank.unplug();
                Err(e)
            }
        }
    }

    fn add_section(&mut self, ctx: &AudioContext, section: Section) -> Result<(), String> {
        let [b0, b1] = section.feedforward;
        let [a0, a1, a2] = section.feedback;
        let feedforward = js_sys::Array::of2(&b0.into(), &b1.into());
        let feedback = js_sys::Array::of3(&a0.into(), &a1.into(), &a2.into());
        let node = ctx
            .create_iir_filter(&feedforward, &feedback)
            .map_err(|e| format!("receiver section: {e:?}"))?;
        // Kept before connecting, so a failure part-way still unplugs it.
        self.sections.push(node.clone());
        self.input
            .connect_with_audio_node(&node)
            .map_err(|e| format!("receiver section in: {e:?}"))?;
        node.connect_with_audio_node(&self.output)
            .map_err(|e| format!("receiver section out: {e:?}"))?;
        Ok(())
    }

    /// Sum into `into`, the way to the AGC.
    fn connect(&self, into: &GainNode) -> Result<(), String> {
        self.output
            .connect_with_audio_node(into)
            .map(|_| ())
            .map_err(|e| format!("receiver output connect: {e:?}"))
    }
}

impl Unplug for ReceiverBank {
    /// Off the mix it was fed from, and away from the sum. A node left
    /// connected to the mix would go on filtering silence into nothing for
    /// the life of the page.
    fn unplug(&self) {
        for node in &self.sections {
            let _ = self.input.disconnect_with_audio_node(node);
            let _ = node.disconnect();
        }
        let _ = self.output.disconnect();
    }
}
