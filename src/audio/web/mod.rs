//! The browser's player: Web Audio nodes, playing the same models the native
//! player runs.
//!
//! - [`chain`]: the output stage — where everything is mixed, the receiver's
//!   AGC as a compressor, and the soft limiter.
//! - [`receiver`]: the receiver's filter as IIR nodes, crossfaded on a retune.
//! - [`stream`]: the band, generated here and streamed in.
//! - [`send`]: one send's tones, scheduled, your own in a contest around the
//!   receiver.
//! - [`sidetone`]: the paddle's sidetone.
//! - [`param`]: the fades and the letting go every part of it needs.

mod chain;
mod param;
mod receiver;
mod send;
mod sidetone;
mod stream;

use std::cell::Cell;
use std::rc::Rc;

use cw_core::band::{BandKey, Qsb, SOFT_LIMIT_HEADROOM};
use cw_core::{PlannedTransmission, TrainingSettings, Transmission};
use gloo_timers::callback::Timeout;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{AudioContext, AudioContextState, GainNode};

use super::band_queue::Reservation;
use super::epoch::{Epoch, Ticket};
use super::shape::{Gains, GateSchedule, GateStep, SEND_RELEASE_SEC, ToneRequest};
use super::wait::{heard_ms, schedule_lead_sec, send_timing};
use super::{METER_FRAME_SEC, MeterFrame, MorseBackend, PlaybackSignal, SendTiming, WaitFlags};
use crate::time::mono_ms;
use chain::Chain;
use param::{Retiring, ramp_to};
use receiver::Receiver;
use send::ScheduledSend;
use sidetone::Sidetone;
use stream::BandStream;

/// How long after everything has gone quiet the context is suspended: long
/// enough for the last fades to play out.
const SLEEP_AFTER_MS: u32 = 250;

pub struct MorsePlayer {
    ctx: AudioContext,
    epoch: Epoch,
    chain: Chain,
    /// The receiver's own filter — the same resonators the native player
    /// runs — built for the current pitch, width and shape.
    receiver: Receiver,
    band: Option<BandStream>,
    /// What the band, and the AGC set for it, were built from.
    band_key: Option<BandKey>,
    send: Option<ScheduledSend>,
    /// When the send playing is heard from, on the app's clock, as this
    /// player said it would be, and where it starts on the context's: what
    /// a send asked to start at a given time takes over from it against.
    heard: Option<(f64, f64)>,
    /// Gains fading out: a stopped send's, or a band that was replaced.
    released: Retiring<GainNode>,
    /// The gains of a send a continuation takes over from, playing on to
    /// where it does: a stop cuts them short with the rest.
    taken_over: Retiring<GainNode>,
    /// The receiver's gate as its gains have been told to play it, for each
    /// send to go on from where it stands.
    gate: GateSchedule,
    sidetone: Sidetone,
    /// Kept so the listener can be taken off again when this player goes.
    on_visibility: Option<Closure<dyn FnMut()>>,
    /// Suspends the context once everything has gone quiet, unless something
    /// is played first. A running context keeps the audio thread awake and
    /// the device from sleeping, with nothing to play.
    sleep: Option<Timeout>,
    /// Set while the context is suspended because nothing is playing, so
    /// the page coming back into view does not wake it for nothing.
    asleep: Rc<Cell<bool>>,
    /// When the meter was last read, on the context's clock, and the
    /// analysers' samples read into.
    metered_at: Option<f64>,
    meter_input: Vec<f32>,
    meter_output: Vec<f32>,
}

/// What a scheduled send looks like to a waiter.
///
/// Progress is read off the AudioContext's own clock, never the wall clock: a
/// suspended context (autoplay policy, a backgrounded tab, iOS taking the
/// audio away) freezes `current_time` while `setTimeout` keeps firing, and a
/// send that is counted down on the wall clock would then be declared finished
/// while the Morse has not been heard at all.
struct WebSignal {
    ctx: AudioContext,
    ticket: Ticket,
    started_at: f64,
    /// Its lead on the clock, and the latency the browser reports, read as
    /// it started.
    timing: SendTiming,
}

impl PlaybackSignal for WebSignal {
    fn timing(&self) -> SendTiming {
        self.timing
    }

    fn poll(&self) -> WaitFlags {
        WaitFlags {
            cancelled: self.ticket.is_cancelled(),
            finished: false,
            failed: self.ctx.state() == AudioContextState::Closed,
            // A hidden page is the one case where a frozen clock is not a
            // fault. The tones stay scheduled at their own context times, so
            // coming back resumes the group where it stopped.
            suspended: page_is_hidden(),
            played_ms: Some(heard_ms(
                self.ctx.current_time(),
                self.started_at,
                heard_latency(&self.ctx),
            )),
        }
    }
}

/// A latency the context reports, in seconds, if it reports it at all:
/// `baseLatency` and `outputLatency` are not in every browser, nor in
/// web-sys.
fn reported_latency(ctx: &AudioContext, name: &str) -> f64 {
    js_sys::Reflect::get(ctx, &name.into())
        .ok()
        .and_then(|value| value.as_f64())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .unwrap_or(0.0)
}

/// How long after the context renders a sample it reaches the ear.
fn heard_latency(ctx: &AudioContext) -> f64 {
    reported_latency(ctx, "baseLatency") + reported_latency(ctx, "outputLatency")
}

/// Whether the page is out of sight. A context this app suspended itself, on a
/// page the listener is looking at, is still a fault worth recovering from —
/// only the tab being away excuses a clock that is not moving.
fn page_is_hidden() -> bool {
    web_sys::window()
        .and_then(|window| window.document())
        .is_some_and(|doc| doc.hidden())
}

impl MorsePlayer {
    pub fn new() -> Result<Self, String> {
        let ctx = AudioContext::new().map_err(|e| format!("AudioContext: {e:?}"))?;
        let chain = Chain::build(&ctx)?;
        let asleep = Rc::new(Cell::new(false));
        let on_visibility = install_resume_on_foreground(&ctx, Rc::clone(&asleep));
        Ok(Self {
            ctx,
            epoch: Epoch::default(),
            chain,
            receiver: Receiver::new(),
            band: None,
            band_key: None,
            send: None,
            heard: None,
            released: Retiring::new(),
            taken_over: Retiring::new(),
            gate: GateSchedule::default(),
            sidetone: Sidetone::default(),
            on_visibility,
            sleep: None,
            asleep,
            metered_at: None,
            meter_input: vec![0.0; chain::METER_WINDOW as usize],
            meter_output: vec![0.0; chain::METER_WINDOW as usize],
        })
    }

    fn apply_band_now(&mut self, settings: &TrainingSettings) -> Result<(), String> {
        self.chain.set_gains(&self.ctx, Gains::of(settings));
        self.receiver.tune(&self.ctx, &self.chain, settings)?;
        let key = BandKey::of(&settings.band);
        if self.band_key == Some(key) {
            return Ok(());
        }
        // A send in flight keeps its pile-up when the band under it changes.
        let pileup = self.band.as_ref().and_then(BandStream::pileup);
        self.retire_band();
        if self.ctx.state() == AudioContextState::Closed {
            return Ok(());
        }
        if settings.band.has_background() {
            // Its floor is the receiver's, muted with it while you send.
            self.band = Some(BandStream::start(
                &self.ctx,
                &self.chain.mix,
                &self.chain.band_floor,
                settings,
                pileup,
            )?);
        }
        self.chain.tune_agc(&self.ctx, settings);
        self.band_key = Some(key);
        Ok(())
    }

    /// Take the band down. It fades rather than stopping dead, and its
    /// output gains are let go of once the fade has run.
    fn retire_band(&mut self) {
        self.released.collect(self.ctx.current_time());
        self.band_key = None;
        if let Some(stream) = self.band.take() {
            let (gains, done) = stream.fade_out();
            for gain in gains {
                self.released.retire(gain, done);
            }
        }
    }

    /// Ramp the send down instead of cutting it: now, or, for one a
    /// continuation takes over from, ahead at `at` on the context's clock,
    /// where it does.
    fn release_send(&mut self, at: Option<f64>) {
        let now = self.ctx.current_time();
        self.released.collect(now);
        self.taken_over.collect(now);
        self.heard = None;
        if let Some(send) = self.send.take() {
            let (gains, done) = send.release_at(&self.ctx, at.unwrap_or(now));
            let into = if at.is_some() {
                &mut self.taken_over
            } else {
                &mut self.released
            };
            for gain in gains {
                into.retire(gain, done);
            }
        }
    }

    /// Mute the receiver from `start` on the context's clock until
    /// `muted_until` seconds in, while you send without full break-in, or
    /// open it there with nothing muted: from wherever its gate stands then
    /// as scheduled, so it never steps.
    fn schedule_gate(&mut self, start: f64, muted_until: Option<f64>) {
        let steps = self
            .gate
            .schedule(start, muted_until, self.ctx.current_time());
        for param in self.chain.gates() {
            let _ = param.cancel_scheduled_values(start);
            for step in &steps {
                let _ = match *step {
                    GateStep::Set(level, at) => param.set_value_at_time(level, at),
                    GateStep::Ramp(level, at) => param.linear_ramp_to_value_at_time(level, at),
                };
            }
        }
    }
}

impl MorseBackend for MorsePlayer {
    /// Nothing waits on the resume. A context that the browser will not let
    /// start keeps its clock still, and the send waiting on it fails after
    /// its grace — which is what rebuilds the player and, failing that, tells
    /// the listener — rather than leaving the session parked on a promise
    /// that may never settle.
    fn resume_from_gesture(&mut self) {
        // Something is about to play: whatever was going to put the context
        // to sleep is called off.
        self.sleep = None;
        self.asleep.set(false);
        if self.ctx.state() == AudioContextState::Suspended {
            let _ = self.ctx.resume();
        }
    }

    fn apply_band(&mut self, settings: &TrainingSettings) {
        if settings.band.has_background() {
            self.resume_from_gesture();
        }
        // A node the browser will not make leaves the band as it was; the
        // send that follows reports anything worse.
        let _ = self.apply_band_now(settings);
    }

    fn start(
        &mut self,
        _transmission: &Transmission,
        plan: &PlannedTransmission,
        settings: &TrainingSettings,
        heard_in: Option<f64>,
    ) -> Result<Rc<dyn PlaybackSignal>, String> {
        self.resume_from_gesture();
        let ticket = self.epoch.arm();
        // One reading of the clock, for the tones, the fading, the pile-up
        // and the waiter alike.
        let now = self.ctx.current_time();
        let lead = schedule_lead_sec(reported_latency(&self.ctx, "baseLatency"));
        let latency = heard_latency(&self.ctx);
        let soonest = now + lead;
        let now_heard = mono_ms() as f64 / 1_000.0;
        // A send asked to start at a given time takes over from the one
        // playing where that one is then, on the clock it plays on, which
        // runs on to there; with nothing playing, it waits out the time
        // from now. Never sooner than a send can start.
        let start = match (heard_in, self.heard) {
            (Some(heard_in), Some((playing_heard, playing_start))) if self.send.is_some() => {
                playing_start + (now_heard + heard_in - playing_heard)
            }
            (Some(heard_in), _) => now + heard_in - latency,
            (None, _) => soonest,
        }
        .max(soonest);
        let takes_over = heard_in.is_some() && self.send.is_some();
        // The send taken over from is ridden down by the moment the new one
        // starts, your message in it silent then, so the two never sound
        // together.
        self.release_send(takes_over.then_some(start - SEND_RELEASE_SEC));
        // Your own message in a contest goes around the receiver, whose
        // filter, AGC and AF gain are for what you copy, as the paddle's
        // sidetone does; at the volume alone, as the native mixer monitors
        // it, and through the limiter.
        let send = ScheduledSend::schedule(
            &self.ctx,
            &self.chain.mix,
            &self.chain.monitor_in,
            plan,
            Qsb::of(&settings.band),
            start,
        )?;
        // The receiver muted while you send, without full break-in, from
        // where the gate stands as the send starts: shut still, when it
        // takes over from your message going on.
        self.schedule_gate(start, plan.muted_until);
        self.send = Some(send);
        if let Some(stream) = &self.band {
            stream.reserve(Reservation::new(
                plan.others_at_once(),
                start + plan.wanted.duration_sec,
            ));
        }
        let timing = send_timing(start - now, latency);
        self.heard = Some((now_heard + timing.heard_after_sec(), start));
        Ok(Rc::new(WebSignal {
            ctx: self.ctx.clone(),
            ticket,
            started_at: start,
            timing,
        }))
    }

    fn stop(&mut self) {
        self.epoch.bump();
        self.release_send(None);
        // A send a continuation was to take over from later stops now too,
        // as natively, and the receiver comes back from where it stands.
        let now = self.ctx.current_time();
        self.taken_over
            .hasten(now, now + SEND_RELEASE_SEC, |gain, from, until| {
                ramp_to(&gain.gain(), 0.0, from, until);
            });
        self.schedule_gate(now, None);
        if let Some(stream) = &self.band {
            stream.reserve(None);
        }
    }

    fn shutdown(&mut self) {
        MorseBackend::stop(self);
        // Faded rather than disconnected on the spot: the fades are let go of
        // on the next send, or when the player itself goes.
        self.retire_band();
        self.sidetone.silence(&self.ctx);
        // Once the fades have played out there is nothing left to hear, and
        // a running context keeps the audio thread and the device awake for
        // the life of the page. Anything played before then calls it off.
        let (ctx, asleep) = (self.ctx.clone(), Rc::clone(&self.asleep));
        self.sleep = Some(Timeout::new(SLEEP_AFTER_MS, move || {
            if ctx.state() == AudioContextState::Running {
                asleep.set(true);
                let _ = ctx.suspend();
            }
        }));
    }

    /// Around the receiver — its filter and AGC are for what you copy, not
    /// for the sidetone of the key in your hand — and around the volume, as
    /// it comes at the level it is heard at, but through the limiter: added
    /// after it, a squeeze on top of a crash squared off.
    fn set_live_tone(&mut self, on: bool, frequency_hz: f64, gain: f64) {
        self.resume_from_gesture();
        let tone = ToneRequest::new(on, frequency_hz, gain);
        let _ = self.sidetone.set(&self.ctx, &self.chain.sidetone_in, tone);
    }

    /// The analysers hold the latest stretch of samples, not a queue of
    /// frames: as many frames as the context's clock has moved since the
    /// last look are cut from the end of what they hold, the AGC's gain
    /// read once for all of them. What is played is read on its way into
    /// the limiter, where everything is [`SOFT_LIMIT_HEADROOM`] times
    /// quieter than it is played, and put back up by as much: read as it
    /// stood, the scope had the browser twelve decibels under the limiter
    /// that it was not.
    fn take_meter(&mut self, into: &mut Vec<MeterFrame>) {
        let now = self.ctx.current_time();
        let Some(last) = self.metered_at.replace(now) else {
            return;
        };
        let frame_len = ((METER_FRAME_SEC * f64::from(self.ctx.sample_rate())) as usize).max(1);
        let held = self.meter_input.len() / frame_len;
        let frames = (((now - last) / METER_FRAME_SEC).round().max(0.0) as usize).min(held);
        if frames == 0 {
            return;
        }
        self.chain
            .meter_input
            .get_float_time_domain_data(&mut self.meter_input);
        self.chain
            .meter_output
            .get_float_time_domain_data(&mut self.meter_output);
        let gain = self.chain.agc_gain() as f32;
        let peak = |samples: &[f32]| samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        let end = self.meter_input.len();
        for k in (0..frames).rev() {
            let span = end - (k + 1) * frame_len..end - k * frame_len;
            into.push(MeterFrame {
                input: peak(&self.meter_input[span.clone()]),
                gain,
                output: peak(&self.meter_output[span]) * SOFT_LIMIT_HEADROOM as f32,
            });
        }
    }
}

impl Drop for MorsePlayer {
    /// A player is rebuilt after a failed send. Without this every rebuild
    /// left its whole audio context running, and its visibility listener
    /// resuming it, for the life of the page.
    fn drop(&mut self) {
        if let Some(stream) = self.band.take() {
            drop(stream.fade_out());
        }
        if let (Some(listener), Some(doc)) = (
            self.on_visibility.take(),
            web_sys::window().and_then(|window| window.document()),
        ) {
            let _ = doc.remove_event_listener_with_callback(
                "visibilitychange",
                listener.as_ref().unchecked_ref(),
            );
        }
        let _ = self.ctx.close();
    }
}

/// Resume the context when the page comes back into view, if it was the page
/// going away that suspended it. Returns the listener so it can be removed.
fn install_resume_on_foreground(
    ctx: &AudioContext,
    asleep: Rc<Cell<bool>>,
) -> Option<Closure<dyn FnMut()>> {
    let ctx = ctx.clone();
    let closure = Closure::wrap(Box::new(move || {
        if asleep.get() || page_is_hidden() || ctx.state() != AudioContextState::Suspended {
            return;
        }
        let _ = ctx.resume();
    }) as Box<dyn FnMut()>);
    let doc = web_sys::window().and_then(|window| window.document())?;
    doc.add_event_listener_with_callback("visibilitychange", closure.as_ref().unchecked_ref())
        .ok()?;
    Some(closure)
}
