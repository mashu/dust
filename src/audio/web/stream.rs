//! The receiver, rendered here ahead of the browser's clock and streamed
//! into the graph a tenth of a second at a time ([`ReceiverStream`]).

use std::cell::RefCell;
use std::ops::Range;
use std::rc::{Rc, Weak};

use gloo_timers::callback::Interval;
use wasm_bindgen::JsCast;
use web_sys::{
    AudioBufferSourceNode, AudioContext, AudioContextState, AudioScheduledSourceNode, GainNode,
};

use super::{page_is_hidden, reported_latency};
use crate::audio::mixer::{Mixer, Swept};
use crate::audio::stream::{Missed, ReceiverStream, Taken};

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
/// has not begun to be played. A stream fallen behind picks up there too.
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
            stream: ReceiverStream::new(mixer, ctx.current_time(), CHANGE_MARGIN_SEC),
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
        let made = state.change_at(at, change);
        state.top_up(CHANGE_LEAD_SEC);
        made
    }

    /// Start a send with `start` as soon as it can be heard, or at `at`
    /// should that be later: `start` puts it in the mixer and says when its
    /// sound is, counted from the moment it is made. When its sound is on
    /// the context's clock, and the stream's watch on it, kept from the
    /// moment it is made: the top-up that follows can find the stream
    /// fallen behind already.
    pub fn send_at(
        &self,
        at: f64,
        start: impl FnOnce(&mut Mixer, &mut Swept) -> Range<f64>,
    ) -> (Range<f64>, SendWatch) {
        let mut state = self.state.borrow_mut();
        let mut sound = 0.0..0.0;
        let made = state.change_at(at, |mixer, swept| sound = start(mixer, swept));
        let sound = made + sound.start..made + sound.end;
        let missed = state.stream.watch(sound.start, sound.end);
        state.top_up(CHANGE_LEAD_SEC);
        let watch = SendWatch {
            state: Rc::downgrade(&self.state),
            missed,
        };
        (sound, watch)
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

/// The stream's watch on a send's sound, for its falling behind the clock
/// across some of it, which then went unheard ([`ReceiverStream::watch`]).
pub struct SendWatch {
    state: Weak<RefCell<StreamState>>,
    missed: Missed,
}

impl SendWatch {
    /// What of the send's sound went unheard, if any: from when to when on
    /// the context's clock ([`Missed`]). The stream is caught up first, as a
    /// tick would: after a stall the send's clock has run on past where the
    /// stream got to, and the tick that finds it may come only after the
    /// send's waiter has read that clock and taken the send for heard.
    pub fn missed(&self) -> Option<(f64, f64)> {
        if let Some(state) = self.state.upgrade()
            && let Ok(mut state) = state.try_borrow_mut()
        {
            state.catch_up();
        }
        self.missed.span()
    }
}

impl StreamState {
    /// What the browser has taken of the stream, on the context's clock:
    /// whatever it is already rendering, its base latency past the clock.
    fn taken(&self) -> f64 {
        self.ctx.current_time() + reported_latency(&self.ctx, "baseLatency")
    }

    fn soonest(&self) -> f64 {
        self.taken() + CHANGE_MARGIN_SEC
    }

    /// Run the mixer on, should the clock have taken all that was rendered
    /// ([`ReceiverStream::catch_up_to`]). Only then: a change just made is
    /// short of the soonest read now once the clock has stepped on since,
    /// yet still heard on time.
    fn catch_up(&mut self) {
        let taken = self.taken();
        self.stream.catch_up_to(taken);
    }

    /// Make `change` as soon as it can be heard, or at `at` should that be
    /// later, taking back what was handed over from there on. When it was
    /// made.
    fn change_at(&mut self, at: f64, change: impl FnOnce(&mut Mixer, &mut Swept)) -> f64 {
        // Caught up first: a change put at the clock read before catching
        // up would land in what the catching up then runs on over unheard.
        self.catch_up();
        let at = at.max(self.soonest());
        let (made, taken) = self.stream.change_at(at, change);
        for taken in taken {
            match taken {
                Taken::StopAt(node, at) => {
                    let _ = node
                        .unchecked_ref::<AudioScheduledSourceNode>()
                        .stop_with_when(at);
                    self.stopped.push((node, at));
                }
                Taken::Never(node) => {
                    let _ = node.unchecked_ref::<AudioScheduledSourceNode>().stop();
                    let _ = node.disconnect();
                }
            }
        }
        made
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
        let taken = self.taken();
        let (ctx, into) = (&self.ctx, &self.into);
        // Picked up only should the clock have taken all that was rendered,
        // and then where a stretch can still start on time, as a change is
        // made: one started at the clock just read would start in the past,
        // play at once from its first sample, late, over the next.
        let rendered = self
            .stream
            .render_until(taken, now + lead, |starts, samples| {
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
