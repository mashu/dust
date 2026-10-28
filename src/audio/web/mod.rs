//! The browser's player: the native player's own mixer, rendered here ahead
//! of the browser's clock and streamed into a graph of a few nodes.
//!
//! - [`stream`]: the receiver — the band, the sends, the filter, the AGC, the
//!   mute while you send — rendered and streamed in ([`crate::audio::stream`]).
//! - [`chain`]: the output stage — the volume and the soft limiter.
//! - [`sidetone`]: the paddle's sidetone, played live.
//! - [`param`]: moving a gain smoothly from wherever it is.

mod chain;
mod param;
mod sidetone;
mod stream;

use std::cell::Cell;
use std::rc::Rc;

use cw_core::band::{BandChange, BandKey, FilterDesign, Qsb, Receiver};
use cw_core::{PlannedTransmission, TrainingSettings, Transmission};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{AudioContext, AudioContextState};

use super::ToneRequest;
use super::epoch::{Epoch, Ticket};
use super::mixer::{BandVoice, Mixer, SendVoice, fade_origin_sec};
use super::shape::Gains;
use super::wait::{heard_ms, send_timing};
use super::{MeterFrame, MorseBackend, PlaybackSignal, SendTiming, WaitFlags};
use crate::time::mono_ms;
use chain::Chain;
use sidetone::Sidetone;
use stream::MixerStream;

pub struct MorsePlayer {
    ctx: AudioContext,
    epoch: Epoch,
    chain: Chain,
    stream: MixerStream,
    /// What the band, and the receiver's gains, were last built from.
    band_key: Option<BandKey>,
    gains: Option<Gains>,
    /// The send playing, for one asked to start at a given time to take
    /// over from.
    playing: Option<Playing>,
    sidetone: Sidetone,
    /// Where on the band's fading the context's clock started
    /// ([`fade_origin_sec`]).
    fade_origin_sec: f64,
    /// Whether the page has taken the device's playback session, as it does
    /// the first time anything is about to be heard.
    claimed_session: bool,
    /// Kept so the listener can be taken off again when this player goes.
    on_visibility: Option<Closure<dyn FnMut()>>,
    /// Set while the context is suspended because nothing is playing, so
    /// the page coming back into view does not wake it for nothing.
    asleep: Rc<Cell<bool>>,
}

/// A send playing: when it was to be heard, on the app's clock.
#[derive(Clone, Copy, Debug)]
struct Playing {
    heard: f64,
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
            // fault. The receiver stays rendered at its own context times,
            // so coming back resumes the group where it stopped.
            suspended: page_is_hidden(),
            // Against the latency read as it started, as its timing is:
            // the browser refining it as the send plays is no hold-up.
            played_ms: Some(heard_ms(
                self.ctx.current_time(),
                self.started_at,
                self.timing.latency_sec,
            )),
            // Its clock says that, against the app's.
            late: false,
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

/// Take the device's playback session, where the browser has one to take
/// (Safari, since iOS 16.4). Left to itself, a page that plays only Web
/// Audio is given an ambient one, which the Ring/Silent switch mutes: the
/// context runs, every send completes, and nothing is heard. Playback is
/// not muted by the switch — and stops other audio while it plays, which is
/// why it is taken only once something is about to be heard, not as soon as
/// a screen builds the player.
fn claim_playback_session() {
    let Some(navigator) = web_sys::window().map(|window| window.navigator()) else {
        return;
    };
    if let Ok(session) = js_sys::Reflect::get(&navigator, &"audioSession".into())
        && session.is_object()
    {
        let _ = js_sys::Reflect::set(&session, &"type".into(), &"playback".into());
    }
}

impl MorsePlayer {
    pub fn new() -> Result<Self, String> {
        let ctx = AudioContext::new().map_err(|e| format!("AudioContext: {e:?}"))?;
        let chain = Chain::build(&ctx)?;
        let mixer = Mixer::new(ctx.sample_rate() as u32);
        let stream = MixerStream::start(&ctx, &chain.stream_in, mixer);
        let asleep = Rc::new(Cell::new(false));
        let on_visibility = install_resume_on_foreground(&ctx, Rc::clone(&asleep));
        Ok(Self {
            ctx,
            epoch: Epoch::default(),
            chain,
            stream,
            band_key: None,
            gains: None,
            playing: None,
            sidetone: Sidetone::default(),
            fade_origin_sec: fade_origin_sec(),
            claimed_session: false,
            on_visibility,
            asleep,
        })
    }

    /// The receiver brought in line with `settings`: a band built afresh for
    /// what changes one, or the gains alone moved; and the volume, which the
    /// graph sets, at once.
    fn apply_band_now(&mut self, settings: &TrainingSettings) {
        if self.ctx.state() == AudioContextState::Closed {
            return;
        }
        let gains = Gains::of(settings);
        self.chain.set_volume(&self.ctx, gains.volume);
        let receiver = Gains {
            volume: 1.0,
            ..gains
        };
        let key = BandKey::of(&settings.band);
        let rate = self.ctx.sample_rate() as u32;
        let mut change = key.change_from(self.band_key.as_ref());
        if change == BandChange::Retune {
            // The band on the air carries on through the new receiver.
            let tuned = Receiver::new(rate, settings);
            let mut retuned = false;
            self.stream.change(|mixer, swept| {
                retuned = mixer.retune_band(tuned, receiver, swept);
            });
            change = if retuned {
                BandChange::None
            } else {
                BandChange::Rebuild
            };
        } else if change == BandChange::None && self.gains != Some(receiver) {
            self.stream.change(|mixer, _| mixer.set_gains(receiver));
        }
        if change == BandChange::Rebuild {
            // A fresh band every time, as the native player's is: with
            // stations on it, a fixed seed would replay the same callsigns at
            // the same moments on every visit.
            let band = BandVoice::for_settings(settings, rate, crate::time::seed_rng());
            self.stream
                .change(|mixer, swept| mixer.set_band_at(band, receiver, swept));
        }
        self.band_key = Some(key);
        self.gains = Some(receiver);
    }
}

impl MorseBackend for MorsePlayer {
    /// Nothing waits on the resume. A context that the browser will not let
    /// start keeps its clock still, and the send waiting on it fails after
    /// its grace — which is what rebuilds the player and, failing that, tells
    /// the listener — rather than leaving the session parked on a promise
    /// that may never settle.
    fn resume_from_gesture(&mut self) {
        if !self.claimed_session {
            claim_playback_session();
            self.claimed_session = true;
        }
        self.asleep.set(false);
        if self.ctx.state() == AudioContextState::Suspended {
            let _ = self.ctx.resume();
        }
    }

    fn apply_band(&mut self, settings: &TrainingSettings) {
        if settings.band.has_background() {
            self.resume_from_gesture();
        }
        self.apply_band_now(settings);
    }

    fn start(
        &mut self,
        _transmission: &Transmission,
        plan: &PlannedTransmission,
        settings: &TrainingSettings,
        heard_in: Option<f64>,
    ) -> Result<Rc<dyn PlaybackSignal>, String> {
        self.resume_from_gesture();
        if self.ctx.state() == AudioContextState::Closed {
            return Err("Audio is unavailable.".into());
        }
        let ticket = self.epoch.arm();
        // One reading of each clock, for where the send goes and what its
        // waiter is told.
        let now = self.ctx.current_time();
        let now_heard = mono_ms() as f64 / 1_000.0;
        let latency = heard_latency(&self.ctx);
        let soonest = self.stream.soonest();
        let (rate, lookahead) = self
            .stream
            .read(|mixer| (mixer.sample_rate(), mixer.lookahead_sec()));
        let sr = f64::from(rate);
        let samples = |seconds: f64| (seconds.max(0.0) * sr).round() as usize;
        // A send asked to start at a given time takes over from the one
        // playing where that one is then, sample for sample; with nothing
        // playing, it waits out the time from now. It is played the
        // receiver's look-ahead after it goes in, as natively, and never
        // sooner than a change can be.
        let (into, wait) = match heard_in {
            None => (None, 0),
            Some(heard_in) => (
                self.playing
                    .map(|playing| samples(now_heard + heard_in - playing.heard)),
                samples(now + heard_in - latency - lookahead - soonest),
            ),
        };
        let voice = SendVoice::new(
            plan,
            &FilterDesign::from_settings(settings),
            Qsb::of(&settings.band),
            rate,
            self.fade_origin_sec + soonest + wait as f64 / sr,
        );
        let mut waits = 0;
        let made = self.stream.change_at(soonest, |mixer, swept| {
            waits = mixer.start_at(voice, into, wait, swept);
        });
        let starts = made + waits as f64 / sr + lookahead;
        let timing = send_timing(starts - now, latency);
        self.playing = Some(Playing {
            heard: now_heard + timing.heard_after_sec(),
        });
        Ok(Rc::new(WebSignal {
            ctx: self.ctx.clone(),
            ticket,
            started_at: starts,
            timing,
        }))
    }

    fn stop(&mut self) {
        self.epoch.bump();
        self.playing = None;
        // The send rides its own release down to silence; the receiver
        // comes back from where its mute stands.
        self.stream.change(|mixer, _| mixer.stop());
    }

    fn shutdown(&mut self) {
        MorseBackend::stop(self);
        // Faded rather than cut, as natively.
        self.stream
            .change(|mixer, swept| mixer.set_band(None, swept));
        self.band_key = None;
        self.sidetone.silence(&self.ctx);
    }

    /// Suspended, once there is nothing left to hear: a running context
    /// keeps the audio thread awake and the device from sleeping, with
    /// nothing to play. The next band, send or sidetone resumes it.
    fn sleep(&mut self) {
        if self.stream.is_idle() && self.ctx.state() == AudioContextState::Running {
            self.asleep.set(true);
            let _ = self.ctx.suspend();
        }
    }

    /// Around the receiver — its filter and AGC are for what you copy, not
    /// for the sidetone of the key in your hand — and around the volume, as
    /// it comes at the level it is heard at, but through the limiter: added
    /// after it, a squeeze on top of a crash squared off.
    fn set_live_tone(&mut self, tone: ToneRequest) {
        self.resume_from_gesture();
        let _ = self.sidetone.set(&self.ctx, &self.chain.sidetone_in, tone);
    }

    /// The mixer's own frames, each given up once the context has played
    /// it, what was played at the volume the graph plays it at.
    fn take_meter(&mut self, into: &mut Vec<MeterFrame>) {
        let from = into.len();
        self.stream.take_meter(into);
        let volume = self.chain.volume() as f32;
        for frame in &mut into[from..] {
            frame.output *= volume;
        }
    }
}

impl Drop for MorsePlayer {
    /// A player is rebuilt after a failed send. Without this every rebuild
    /// left its whole audio context running, and its visibility listener
    /// resuming it, for the life of the page.
    fn drop(&mut self) {
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
