//! The receiver, rendered here ahead of the browser's clock and streamed
//! into the graph a tenth of a second at a time ([`ReceiverStream`]).

use std::cell::RefCell;
use std::rc::Rc;

use gloo_timers::callback::Interval;
use wasm_bindgen::JsCast;
use web_sys::{
    AudioBufferSourceNode, AudioContext, AudioContextState, AudioScheduledSourceNode, GainNode,
};

use super::{page_is_hidden, reported_latency};
use crate::audio::mixer::{Mixer, Swept};
use crate::audio::stream::{ReceiverStream, Taken};

/// How far ahead of the audio clock the receiver is kept rendered. Enough to
/// ride over a busy main thread; a change made sooner than this takes back
/// what was rendered past it, so it costs nothing in how soon it is heard.
const STREAM_LEAD_SEC: f64 = 0.5;
/// A hidden tab's timers fire about once a second at best, so the receiver
/// is rendered further ahead while the page is out of sight.
const STREAM_LEAD_HIDDEN_SEC: f64 = 2.5;
/// How often the stream is topped up.
const STREAM_TICK_MS: u32 = 100;
/// How far past the browser's clock, beyond its base latency, a change is
/// made: past whatever it is already rendering, so the stretch taken back
/// has not begun to be played.
const CHANGE_MARGIN_SEC: f64 = 0.03;
/// How far ahead a change renders at once, the rest left to the next tick:
/// a click that starts a band and a send is two changes, and each takes
/// back whatever the one before it rendered.
const CHANGE_LEAD_SEC: f64 = 0.2;

/// The receiver, streamed into the graph.
pub struct MixerStream {
    state: Rc<RefCell<StreamState>>,
    _ticker: Interval,
}

struct StreamState {
    ctx: AudioContext,
    into: GainNode,
    stream: ReceiverStream<AudioBufferSourceNode>,
    /// Stretches stopped short by a change, connected until they are over.
    stopped: Vec<(AudioBufferSourceNode, f64)>,
}

impl MixerStream {
    /// Stream `mixer` into `into`, starting now.
    pub fn start(ctx: &AudioContext, into: &GainNode, mixer: Mixer) -> Self {
        let state = Rc::new(RefCell::new(StreamState {
            ctx: ctx.clone(),
            into: into.clone(),
            stream: ReceiverStream::new(mixer, ctx.current_time()),
            stopped: Vec::new(),
        }));
        let ticking = Rc::clone(&state);
        let ticker = Interval::new(STREAM_TICK_MS, move || {
            // A tick that finds the state busy has nothing to add: whatever
            // holds it is topping up already.
            if let Ok(mut stream) = ticking.try_borrow_mut() {
                let lead = if page_is_hidden() {
                    STREAM_LEAD_HIDDEN_SEC
                } else {
                    STREAM_LEAD_SEC
                };
                stream.top_up(lead);
            }
        });
        Self {
            state,
            _ticker: ticker,
        }
    }

    /// The soonest a change can be heard to start, on the context's clock.
    pub fn soonest(&self) -> f64 {
        self.state.borrow().soonest()
    }

    /// Make `change` as soon as it can be heard, or at `at` should that be
    /// later. When it was made.
    pub fn change_at(&self, at: f64, change: impl FnOnce(&mut Mixer, &mut Swept)) -> f64 {
        let mut state = self.state.borrow_mut();
        // Caught up first: a change put at the clock read before catching
        // up would land in what the catching up then runs on over unheard.
        state.catch_up();
        let at = at.max(state.soonest());
        let (made, taken) = state.stream.change_at(at, change);
        for taken in taken {
            match taken {
                Taken::StopAt(node, at) => {
                    let _ = node
                        .unchecked_ref::<AudioScheduledSourceNode>()
                        .stop_with_when(at);
                    state.stopped.push((node, at));
                }
                Taken::Never(node) => {
                    let _ = node.unchecked_ref::<AudioScheduledSourceNode>().stop();
                    let _ = node.disconnect();
                }
            }
        }
        state.top_up(CHANGE_LEAD_SEC);
        made
    }

    /// Make `change` as soon as it can be heard. When it was made.
    pub fn change(&self, change: impl FnOnce(&mut Mixer, &mut Swept)) -> f64 {
        self.change_at(f64::MIN, change)
    }

    /// Something of the mixer as it stands, at the end of what is rendered.
    pub fn read<R>(&self, read: impl FnOnce(&Mixer) -> R) -> R {
        read(self.state.borrow().stream.mixer())
    }

    /// Whether there is nothing left to hear, nor anything being rendered.
    pub fn is_idle(&self) -> bool {
        self.state.borrow().stream.is_idle()
    }
}

impl StreamState {
    fn soonest(&self) -> f64 {
        self.ctx.current_time() + reported_latency(&self.ctx, "baseLatency") + CHANGE_MARGIN_SEC
    }

    /// Run the mixer on to the clock, should it have fallen behind it.
    fn catch_up(&mut self) {
        let soonest = self.soonest();
        self.stream.catch_up_to(soonest);
    }

    /// Render until the receiver runs `lead` ahead of the clock, and let go
    /// of what has played.
    fn top_up(&mut self, lead: f64) {
        if self.ctx.state() == AudioContextState::Closed {
            return;
        }
        // Caught up before the clock is read: catching up takes time, and a
        // stretch put at the moment read before it would start in the past.
        self.catch_up();
        let now = self.ctx.current_time();
        let soonest = self.soonest();
        let (ctx, into) = (&self.ctx, &self.into);
        // Picked up only should the clock have run past what was rendered,
        // and then where a stretch can still start on time, as a change is
        // made: one started at the clock just read would start in the past,
        // play at once from its first sample, late, over the next.
        let rendered = self
            .stream
            .render_until(soonest, now + lead, |starts, samples| {
                schedule(ctx, into, starts, samples)
            });
        // A stretch the browser would not take is tried again on the next
        // tick; a context that cannot make a buffer at all is one the next
        // send finds closed.
        drop(rendered);
        for node in self.stream.played_by(now) {
            let _ = node.disconnect();
        }
        self.stopped.retain(|(node, at)| {
            let over = *at < now;
            if over {
                let _ = node.disconnect();
            }
            !over
        });
    }
}

/// One stretch of the receiver, played from `starts` on the context's clock.
fn schedule(
    ctx: &AudioContext,
    into: &GainNode,
    starts: f64,
    samples: &[f32],
) -> Result<AudioBufferSourceNode, String> {
    let buffer = ctx
        .create_buffer(1, samples.len() as u32, ctx.sample_rate())
        .map_err(|e| format!("buffer: {e:?}"))?;
    buffer
        .copy_to_channel(samples, 0)
        .map_err(|e| format!("buffer copy: {e:?}"))?;
    let node = ctx
        .create_buffer_source()
        .map_err(|e| format!("source: {e:?}"))?;
    node.set_buffer(Some(&buffer));
    node.connect_with_audio_node(into)
        .map_err(|e| format!("source connect: {e:?}"))?;
    node.start_with_when(starts)
        .map_err(|e| format!("source start: {e:?}"))?;
    Ok(node)
}
