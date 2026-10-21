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

use cw_core::band::{BandKey, Qsb};
use cw_core::{PlannedTransmission, TrainingSettings, Transmission};
use gloo_timers::callback::Timeout;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{AudioContext, AudioContextState, GainNode};

use super::band_queue::Reservation;
use super::epoch::{Epoch, Ticket};
use super::shape::ToneRequest;
use super::wait::{heard_after_sec, heard_ms, schedule_lead_sec};
use super::{MorseBackend, PlaybackSignal, WaitFlags};
use chain::Chain;
use param::Retiring;
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
    /// Gains fading out: a stopped send's, or a band that was replaced.
    released: Retiring<GainNode>,
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
    /// How long after it was started the send reaches the ear: its lead on
    /// the clock, and the latency the browser reports, read as it started.
    heard_after_sec: f64,
}

impl PlaybackSignal for WebSignal {
    fn heard_after_sec(&self) -> f64 {
        self.heard_after_sec
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
            released: Retiring::new(),
            sidetone: Sidetone::default(),
            on_visibility,
            sleep: None,
            asleep,
        })
    }

    fn apply_band_now(&mut self, settings: &TrainingSettings) -> Result<(), String> {
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
            self.band = Some(BandStream::start(
                &self.ctx,
                &self.chain.mix,
                &self.chain.floor_in,
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

    /// Ramp the send down instead of cutting it.
    fn release_send(&mut self) {
        self.released.collect(self.ctx.current_time());
        if let Some(send) = self.send.take() {
            let (gains, done) = send.release(&self.ctx);
            for gain in gains {
                self.released.retire(gain, done);
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
    ) -> Result<Rc<dyn PlaybackSignal>, String> {
        self.resume_from_gesture();
        self.release_send();
        let ticket = self.epoch.arm();
        // One reading of the clock, for the tones, the fading, the pile-up
        // and the waiter alike.
        let lead = schedule_lead_sec(reported_latency(&self.ctx, "baseLatency"));
        let start = self.ctx.current_time() + lead;
        // Your own message in a contest goes where the paddle's sidetone
        // goes: around the receiver, whose filter and AGC are for what you
        // copy, but through the limiter.
        let send = ScheduledSend::schedule(
            &self.ctx,
            &self.chain.mix,
            &self.chain.floor_in,
            plan,
            Qsb::of(&settings.band),
            start,
        )?;
        self.send = Some(send);
        if let Some(stream) = &self.band {
            stream.reserve(Reservation::new(
                plan.others_at_once(),
                start + plan.wanted.duration_sec,
            ));
        }
        Ok(Rc::new(WebSignal {
            ctx: self.ctx.clone(),
            ticket,
            started_at: start,
            heard_after_sec: heard_after_sec(lead, heard_latency(&self.ctx)),
        }))
    }

    fn stop(&mut self) {
        self.epoch.bump();
        self.release_send();
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
    /// for the sidetone of the key in your hand — but through the limiter,
    /// the way the floor goes: added after it, a squeeze on top of a crash
    /// squared off.
    fn set_live_tone(&mut self, on: bool, frequency_hz: f64, gain: f64) {
        self.resume_from_gesture();
        let tone = ToneRequest::new(on, frequency_hz, gain);
        let _ = self.sidetone.set(&self.ctx, &self.chain.floor_in, tone);
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
