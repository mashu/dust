use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use cw_core::band::{
    AGC_ATTACK_SEC, AGC_RELEASE_SEC, AGC_TRIGGER, BandMixer, BandSource, FilterDesign,
    QSB_MIN_GAIN, QSB_PATHS, QSB_SPREAD, RECEIVER_SECTIONS, SOFT_LIMIT_HEADROOM,
    band_standing_level, qsb_path, soft_limit_curve,
};
use cw_core::{PlannedTransmission, TrainingSettings, Transmission, plan_transmission};
use gloo_timers::callback::Interval;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{
    AudioBufferSourceNode, AudioContext, AudioContextState, AudioNode, AudioScheduledSourceNode,
    BiquadFilterType, GainNode, OscillatorType,
};

use super::{MorseBackend, PlaybackSignal, PlaybackWait, WaitFlags};

/// How much of the band each streamed buffer holds.
const STREAM_CHUNK_SEC: f64 = 0.1;
/// How far ahead of the audio clock the band is kept scheduled. Enough to
/// ride over a busy main thread; short enough that a buffer is not wasted
/// every time a slider moves.
const STREAM_LEAD_SEC: f64 = 0.5;
/// A hidden tab's timers fire about once a second at best, so the band is
/// scheduled further ahead while the page is out of sight.
const STREAM_LEAD_HIDDEN_SEC: f64 = 2.5;
/// How often the stream is topped up.
const STREAM_TICK_MS: u32 = 100;
/// Points in the soft limiter's lookup table.
const LIMITER_POINTS: usize = 4_097;
/// The rate the band's standing level is measured at for the compressor.
const STANDING_MEASURE_RATE: u32 = 16_000;
/// Fade between one band and the next when the settings change. Long enough
/// to hide the seam, short enough that the change is immediate.
const STREAM_FADE_SEC: f64 = 0.02;

/// How long a stopped send takes to reach silence. The native player's
/// [`crate::audio::render::RELEASE_MS`] in seconds — the same ramp either side.
const RELEASE_SEC: f64 = 0.008;

/// What a scheduled send looks like to a waiter.
///
/// Progress is read off the AudioContext's own clock, never the wall clock: a
/// suspended context (autoplay policy, a backgrounded tab, iOS taking the
/// audio away) freezes `current_time` while `setTimeout` keeps firing, and a
/// send that is counted down on the wall clock would then be declared finished
/// while the Morse has not been heard at all.
struct WebSignal {
    ctx: AudioContext,
    stop_flag: Rc<Cell<bool>>,
    epoch: Rc<Cell<u64>>,
    mine: u64,
    started_at: f64,
}

impl PlaybackSignal for WebSignal {
    fn poll(&self) -> WaitFlags {
        let played = ((self.ctx.current_time() - self.started_at) * 1000.0).max(0.0);
        WaitFlags {
            cancelled: self.stop_flag.get() || self.epoch.get() != self.mine,
            finished: false,
            failed: self.ctx.state() == AudioContextState::Closed,
            // A hidden page is the one case where a frozen clock is not a
            // fault. The tones stay scheduled at their own context times, so
            // coming back resumes the group where it stopped.
            suspended: page_is_hidden(),
            played_ms: Some(played.min(f64::from(u32::MAX / 4)) as u32),
        }
    }
}

/// Whether the page is out of sight. A context this app suspended itself, on a
/// page the listener is looking at, is still a fault worth recovering from —
/// only the tab being away excuses a clock that is not moving.
fn page_is_hidden() -> bool {
    web_sys::window()
        .and_then(|window| window.document())
        .is_some_and(|doc| doc.hidden())
}

pub struct MorsePlayer {
    ctx: AudioContext,
    stop_flag: Rc<Cell<bool>>,
    epoch: Rc<Cell<u64>>,
    mix_gain: GainNode,
    cw_gain: GainNode,
    group_gain: Option<GainNode>,
    band: BandGraph,
    /// The receiver's own filter, as a cascade of band-pass nodes — the same
    /// sections the native player runs — and the gain that brings its centre
    /// back to unity.
    receiver: Vec<web_sys::BiquadFilterNode>,
    receiver_gain: GainNode,
    agc: web_sys::DynamicsCompressorNode,
    /// Gains that are fading out, with the context time their ramp ends: a
    /// stopped send's, or the band stream that was just replaced.
    released: Vec<(GainNode, f64)>,
    pending_resume: RefCell<Option<js_sys::Promise>>,
    live_osc: Option<web_sys::OscillatorNode>,
    live_gain: Option<GainNode>,
    /// Kept so the listener can be taken off again when this player goes.
    on_visibility: Option<Closure<dyn FnMut()>>,
}

/// The background layers: the band itself, streamed, and the fading
/// oscillators on the Morse.
struct BandGraph {
    stream: Option<BandStream>,
    sources: Vec<AudioScheduledSourceNode>,
    nodes: Vec<AudioNode>,
    signature: String,
}

impl BandGraph {
    fn new() -> Self {
        Self {
            stream: None,
            sources: Vec::new(),
            nodes: Vec::new(),
            signature: String::new(),
        }
    }

    /// Take the band down. The stream fades rather than stopping dead; its
    /// output gain is handed back so the caller can let it go once the fade
    /// has run.
    fn stop_layers(&mut self, ctx: &AudioContext, cw_gain: &GainNode) -> Option<(GainNode, f64)> {
        let fading = self.stream.take().map(BandStream::fade_out);
        for source in self.sources.drain(..) {
            let _ = source.stop();
            let _ = source.unchecked_ref::<AudioNode>().disconnect();
        }
        for node in self.nodes.drain(..) {
            let _ = node.disconnect();
        }
        let now = ctx.current_time();
        let _ = cw_gain.gain().cancel_scheduled_values(now);
        let _ = cw_gain.gain().set_value_at_time(1.0, now);
        self.signature.clear();
        fading
    }
}

/// The band, streamed into the graph.
///
/// Web Audio has nowhere to run a generator sample by sample short of an
/// AudioWorklet, and a looped buffer is a pattern: the same crash coming
/// round every few seconds is the one thing real static never does. So the
/// band is generated here, by the same [`BandSource`] the native player runs,
/// a tenth of a second at a time, and each buffer is scheduled on the audio
/// clock to start exactly where the last one ends. Nothing repeats, and the
/// browser plays the very model the desktop does.
struct BandStream {
    state: Rc<RefCell<StreamState>>,
    _ticker: Interval,
}

struct StreamState {
    ctx: AudioContext,
    output: GainNode,
    source: BandSource,
    chunk: Vec<f32>,
    /// Context time the next buffer starts at.
    next_start: f64,
    /// Buffers scheduled and not yet over, with the time each one ends.
    queued: VecDeque<(AudioBufferSourceNode, f64)>,
}

impl BandStream {
    fn start(
        ctx: &AudioContext,
        into: &GainNode,
        settings: &TrainingSettings,
    ) -> Result<Self, String> {
        let output = ctx.create_gain().map_err(|e| format!("band gain: {e:?}"))?;
        let now = ctx.current_time();
        let gain = output.gain();
        gain.set_value_at_time(0.0, now)
            .map_err(|e| format!("band fade: {e:?}"))?;
        gain.linear_ramp_to_value_at_time(1.0, now + STREAM_FADE_SEC)
            .map_err(|e| format!("band fade: {e:?}"))?;
        output
            .connect_with_audio_node(into)
            .map_err(|e| format!("band connect: {e:?}"))?;
        let sample_rate = ctx.sample_rate();
        let chunk = ((f64::from(sample_rate) * STREAM_CHUNK_SEC).round() as usize).max(128);
        let state = Rc::new(RefCell::new(StreamState {
            ctx: ctx.clone(),
            output,
            source: BandSource::new(sample_rate as u32, settings, fastrand::u64(..)),
            chunk: vec![0.0; chunk],
            next_start: now,
            queued: VecDeque::new(),
        }));
        state.borrow_mut().top_up()?;
        let ticking = Rc::clone(&state);
        let ticker = Interval::new(STREAM_TICK_MS, move || {
            // A tick that finds the state busy has nothing to add: whatever
            // holds it is topping up already.
            if let Ok(mut stream) = ticking.try_borrow_mut() {
                let _ = stream.top_up();
            }
        });
        Ok(Self {
            state,
            _ticker: ticker,
        })
    }

    /// Fade out and stop. Dropping `self` here stops the ticker; the output
    /// gain comes back with the time its fade ends.
    fn fade_out(self) -> (GainNode, f64) {
        let stream = self.state.borrow();
        let now = stream.ctx.current_time();
        let done = now + STREAM_FADE_SEC;
        let gain = stream.output.gain();
        let _ = gain.cancel_scheduled_values(now);
        let _ = gain.set_value_at_time(gain.value(), now);
        let _ = gain.linear_ramp_to_value_at_time(0.0, done);
        for (node, _) in &stream.queued {
            let _ = node
                .unchecked_ref::<AudioScheduledSourceNode>()
                .stop_with_when(done);
        }
        (stream.output.clone(), done)
    }
}

impl StreamState {
    /// Schedule buffers until the band runs far enough ahead of the clock,
    /// and let go of the ones that have finished.
    fn top_up(&mut self) -> Result<(), String> {
        if self.ctx.state() == AudioContextState::Closed {
            return Ok(());
        }
        let now = self.ctx.current_time();
        // Fallen behind — a long task, a throttled tab. Pick up just ahead of
        // the clock rather than scheduling into the past, which would play
        // everything late at once.
        if self.next_start < now + 0.005 {
            self.next_start = now + 0.01;
        }
        let lead = if page_is_hidden() {
            STREAM_LEAD_HIDDEN_SEC
        } else {
            STREAM_LEAD_SEC
        };
        let sample_rate = self.ctx.sample_rate();
        while self.next_start < now + lead {
            self.source.fill(&mut self.chunk);
            let buffer = self
                .ctx
                .create_buffer(1, self.chunk.len() as u32, sample_rate)
                .map_err(|e| format!("band buffer: {e:?}"))?;
            buffer
                .copy_to_channel(&self.chunk, 0)
                .map_err(|e| format!("band copy: {e:?}"))?;
            let node = self
                .ctx
                .create_buffer_source()
                .map_err(|e| format!("band source: {e:?}"))?;
            node.set_buffer(Some(&buffer));
            node.connect_with_audio_node(&self.output)
                .map_err(|e| format!("band source connect: {e:?}"))?;
            node.start_with_when(self.next_start)
                .map_err(|e| format!("band start: {e:?}"))?;
            let ends = self.next_start + self.chunk.len() as f64 / f64::from(sample_rate);
            self.queued.push_back((node, ends));
            self.next_start = ends;
        }
        while let Some((node, _)) = self.queued.front().filter(|(_, ends)| *ends < now) {
            let _ = node.disconnect();
            self.queued.pop_front();
        }
        Ok(())
    }
}

impl MorsePlayer {
    pub fn new() -> Result<Self, String> {
        let ctx = AudioContext::new().map_err(|e| format!("AudioContext: {e:?}"))?;
        let mix_gain = ctx.create_gain().map_err(|e| format!("mix gain: {e:?}"))?;
        let cw_gain = ctx.create_gain().map_err(|e| format!("cw gain: {e:?}"))?;
        mix_gain
            .gain()
            .set_value_at_time(1.0, ctx.current_time())
            .map_err(|e| format!("mix set: {e:?}"))?;
        cw_gain
            .gain()
            .set_value_at_time(1.0, ctx.current_time())
            .map_err(|e| format!("cw set: {e:?}"))?;
        cw_gain
            .connect_with_audio_node(&mix_gain)
            .map_err(|e| format!("cw connect: {e:?}"))?;
        // The receiver's filter sits where a real one does: at the end, with
        // everything already mixed into it. The native player filters the send
        // and the background separately, which comes to the same thing — a
        // band-pass is linear — but here they are already together.
        let mut receiver = Vec::with_capacity(RECEIVER_SECTIONS);
        let mut tail: AudioNode = mix_gain.clone().unchecked_into();
        for _ in 0..RECEIVER_SECTIONS {
            let stage = ctx
                .create_biquad_filter()
                .map_err(|e| format!("receiver filter: {e:?}"))?;
            stage.set_type(BiquadFilterType::Bandpass);
            tail.connect_with_audio_node(&stage)
                .map_err(|e| format!("receiver connect: {e:?}"))?;
            tail = stage.clone().unchecked_into();
            receiver.push(stage);
        }
        let receiver_gain = ctx
            .create_gain()
            .map_err(|e| format!("receiver gain: {e:?}"))?;
        tail.connect_with_audio_node(&receiver_gain)
            .map_err(|e| format!("receiver gain connect: {e:?}"))?;
        let tail: AudioNode = receiver_gain.clone().unchecked_into();
        // The receiver's AGC, in the one place the browser can put it: after
        // the filter, with the send and the background already together. The
        // native player works the same gain out in Rust and hands it to both
        // streams; here a compressor does it, because the two halves only meet
        // as sound. Its threshold is set per-settings from the measured floor,
        // which is what stops it undoing the filter.
        let agc = ctx
            .create_dynamics_compressor()
            .map_err(|e| format!("agc: {e:?}"))?;
        agc.knee()
            .set_value_at_time(6.0, ctx.current_time())
            .map_err(|e| format!("agc knee: {e:?}"))?;
        agc.ratio()
            .set_value_at_time(12.0, ctx.current_time())
            .map_err(|e| format!("agc ratio: {e:?}"))?;
        agc.attack()
            .set_value_at_time(AGC_ATTACK_SEC as f32, ctx.current_time())
            .map_err(|e| format!("agc attack: {e:?}"))?;
        agc.release()
            .set_value_at_time(AGC_RELEASE_SEC as f32, ctx.current_time())
            .map_err(|e| format!("agc release: {e:?}"))?;
        tail.connect_with_audio_node(&agc)
            .map_err(|e| format!("agc connect: {e:?}"))?;
        // And the native player's soft limiter, as a shaper. A compressor's
        // attack lets the first milliseconds of a crash through, and a crash
        // through a wide-open filter on a noisy band is well past full scale:
        // without this the browser squared it off where the desktop catches it.
        let headroom = ctx
            .create_gain()
            .map_err(|e| format!("limiter gain: {e:?}"))?;
        headroom
            .gain()
            .set_value_at_time((1.0 / SOFT_LIMIT_HEADROOM) as f32, ctx.current_time())
            .map_err(|e| format!("limiter set: {e:?}"))?;
        let limiter = ctx
            .create_wave_shaper()
            .map_err(|e| format!("limiter: {e:?}"))?;
        limiter.set_curve_opt_f32_slice(Some(&mut soft_limit_curve(LIMITER_POINTS)));
        limiter.set_oversample(web_sys::OverSampleType::N2x);
        agc.connect_with_audio_node(&headroom)
            .map_err(|e| format!("limiter connect: {e:?}"))?;
        headroom
            .connect_with_audio_node(&limiter)
            .map_err(|e| format!("limiter connect: {e:?}"))?;
        limiter
            .connect_with_audio_node(&ctx.destination())
            .map_err(|e| format!("mix connect: {e:?}"))?;
        let on_visibility = install_resume_on_foreground(&ctx);
        Ok(Self {
            ctx,
            stop_flag: Rc::new(Cell::new(false)),
            epoch: Rc::new(Cell::new(0)),
            mix_gain,
            cw_gain,
            group_gain: None,
            band: BandGraph::new(),
            receiver,
            receiver_gain,
            agc,
            released: Vec::new(),
            pending_resume: RefCell::new(None),
            live_osc: None,
            live_gain: None,
            on_visibility,
        })
    }

    pub fn resume_from_gesture(&self) {
        if self.ctx.state() == AudioContextState::Suspended
            && let Ok(promise) = self.ctx.resume()
        {
            *self.pending_resume.borrow_mut() = Some(promise);
        }
    }

    fn ensure_live_tone(&mut self) -> Result<(), String> {
        if self.live_gain.is_some() {
            return Ok(());
        }
        let osc = self
            .ctx
            .create_oscillator()
            .map_err(|e| format!("live osc: {e:?}"))?;
        let gain = self
            .ctx
            .create_gain()
            .map_err(|e| format!("live gain: {e:?}"))?;
        osc.set_type(OscillatorType::Sine);
        let now = self.ctx.current_time();
        osc.frequency()
            .set_value_at_time(500.0, now)
            .map_err(|e| format!("live freq: {e:?}"))?;
        gain.gain()
            .set_value_at_time(0.0, now)
            .map_err(|e| format!("live mute: {e:?}"))?;
        osc.connect_with_audio_node(&gain)
            .map_err(|e| format!("live osc connect: {e:?}"))?;
        // Straight to the speakers: the receiver filter is for what you copy,
        // not for the sidetone of the key in your hand.
        gain.connect_with_audio_node(&self.ctx.destination())
            .map_err(|e| format!("live out: {e:?}"))?;
        osc.start().map_err(|e| format!("live start: {e:?}"))?;
        self.live_osc = Some(osc);
        self.live_gain = Some(gain);
        Ok(())
    }

    /// Retune the receiver to the current pitch, width and shape. Cheap, and
    /// the nodes stay put, so this can run on every send.
    fn tune_receiver(&self, settings: &TrainingSettings) {
        let now = self.ctx.current_time();
        let design = FilterDesign::from_settings(settings);
        for (stage, section) in self.receiver.iter().zip(design.sections()) {
            let _ = stage
                .frequency()
                .set_value_at_time(section.center_hz as f32, now);
            let _ = stage.q().set_value_at_time(section.q as f32, now);
        }
        let _ = self
            .receiver_gain
            .gain()
            .set_value_at_time(design.gain() as f32, now);
    }

    fn apply_band_now(&mut self, settings: &TrainingSettings) -> Result<(), String> {
        self.tune_receiver(settings);
        let signature = settings.band_signature();
        if signature == self.band.signature {
            return Ok(());
        }
        self.retire_band();
        if self.ctx.state() == AudioContextState::Closed {
            return Ok(());
        }
        add_qsb(&self.ctx, &self.cw_gain, settings, &mut self.band)?;
        if BandMixer::needs_background(settings) {
            self.band.stream = Some(BandStream::start(&self.ctx, &self.mix_gain, settings)?);
        }
        self.tune_agc(settings);
        self.band.signature = signature;
        Ok(())
    }

    /// Fade the band out, and let go of whatever earlier fades have finished.
    fn retire_band(&mut self) {
        self.drop_released();
        if let Some(fading) = self.band.stop_layers(&self.ctx, &self.cw_gain) {
            self.released.push(fading);
        }
    }

    /// Point the compressor at this band's own floor.
    ///
    /// This is the whole reason it can be a compressor at all. Left at a fixed
    /// threshold it would ride the level rather than the crashes, and closing
    /// the filter — which is exactly what the app is trying to teach — would
    /// pull the noise down and the compressor would push it straight back up.
    /// Measured from the settings instead, a narrower receiver has a lower
    /// floor and a lower threshold to match, so a crash still has to stand the
    /// same distance above the band before anything ducks.
    fn tune_agc(&self, settings: &TrainingSettings) {
        // Measured at a third of the device rate: the band is calibrated to
        // sound the same at any rate, and this runs on the main thread every
        // time a band setting moves.
        let standing = band_standing_level(STANDING_MEASURE_RATE, settings);
        // A band with nothing on it has no floor to measure. Park the threshold
        // at the top, where the compressor has nothing to do.
        let threshold_db = if standing > 1e-6 {
            (20.0 * (standing * AGC_TRIGGER).log10()).clamp(-100.0, 0.0)
        } else {
            0.0
        };
        let _ = self
            .agc
            .threshold()
            .set_value_at_time(threshold_db as f32, self.ctx.current_time());
    }

    fn bump_epoch(&self) -> u64 {
        let next = self.epoch.get() + 1;
        self.epoch.set(next);
        next
    }

    /// Ramp a stopped send down instead of cutting it.
    ///
    /// Disconnecting on the spot is what made the old stop click: the fade was
    /// scheduled and then the node that would have played it was taken out of
    /// the graph in the same breath. The gain stays connected until the ramp
    /// has run, and is let go on the next send — by which time it is silent.
    fn release_group_gain(&mut self) {
        self.drop_released();
        if let Some(gain) = self.group_gain.take() {
            let now = self.ctx.current_time();
            let done = now + RELEASE_SEC;
            let _ = gain.gain().cancel_scheduled_values(now);
            let _ = gain.gain().set_value_at_time(gain.gain().value(), now);
            let _ = gain.gain().linear_ramp_to_value_at_time(0.0, done);
            self.released.push((gain, done));
        }
    }

    /// Disconnect the gains whose ramp has finished.
    fn drop_released(&mut self) {
        let now = self.ctx.current_time();
        self.released.retain(|(gain, done_at)| {
            if *done_at <= now {
                let _ = gain.disconnect();
                false
            } else {
                true
            }
        });
    }

    pub fn reset_stop_flag(&self) {
        self.stop_flag.set(false);
    }

    fn start_send(
        &mut self,
        transmission: &Transmission,
        settings: &TrainingSettings,
    ) -> Result<PlaybackWait, String> {
        self.resume_from_gesture();
        self.release_group_gain();
        let epoch = self.bump_epoch();
        self.reset_stop_flag();
        self.apply_band_now(settings)?;
        let planned = plan_transmission(transmission, settings);
        let plan = &planned.wanted;
        let started_at = self.ctx.current_time();
        self.schedule_plan(&planned)?;
        Ok(PlaybackWait::new(
            plan.duration_sec,
            plan.resolved_char_wpm,
            plan.resolved_effective_wpm,
            Rc::new(WebSignal {
                ctx: self.ctx.clone(),
                stop_flag: self.stop_flag.clone(),
                epoch: self.epoch.clone(),
                mine: epoch,
                started_at,
            }),
        ))
    }

    fn schedule_plan(&mut self, planned: &PlannedTransmission) -> Result<(), String> {
        self.reset_stop_flag();

        let group_gain = self.ctx.create_gain().map_err(|e| format!("gain: {e:?}"))?;
        group_gain
            .gain()
            .set_value_at_time(1.0, self.ctx.current_time())
            .map_err(|e| format!("gain set: {e:?}"))?;
        group_gain
            .connect_with_audio_node(&self.cw_gain)
            .map_err(|e| format!("connect: {e:?}"))?;
        self.group_gain = Some(group_gain.clone());

        let start = self.ctx.current_time();
        // The wanted station and everyone calling over it go into the same
        // gain: interference is addition, and each station's events already
        // carry its own pitch and level.
        let every_station = std::iter::once(&planned.wanted).chain(planned.others.iter());
        for event in every_station.flat_map(|plan| plan.events.iter()) {
            if self.stop_flag.get() {
                break;
            }
            let osc = self
                .ctx
                .create_oscillator()
                .map_err(|e| format!("osc: {e:?}"))?;
            let gain = self
                .ctx
                .create_gain()
                .map_err(|e| format!("sym gain: {e:?}"))?;
            osc.set_type(OscillatorType::Sine);
            osc.frequency().set_value(event.frequency_hz as f32);
            osc.connect_with_audio_node(&gain)
                .map_err(|e| format!("osc connect: {e:?}"))?;
            gain.connect_with_audio_node(&group_gain)
                .map_err(|e| format!("gain connect: {e:?}"))?;

            let t0 = start + event.start_sec;
            let param = gain.gain();
            let _ = param.set_value_at_time(0.0, t0);
            if event.envelope.len() >= 2 {
                let mut curve = event.envelope.clone();
                if param
                    .set_value_curve_at_time(&mut curve, t0, event.duration_sec)
                    .is_err()
                {
                    let rise = event.duration_sec.min(0.02);
                    let _ = param.linear_ramp_to_value_at_time(event.target_gain as f32, t0 + rise);
                    let _ = param.set_value_at_time(
                        event.target_gain as f32,
                        t0 + event.duration_sec - rise,
                    );
                    let _ = param.linear_ramp_to_value_at_time(0.0, t0 + event.duration_sec);
                }
            }
            osc.start_with_when(t0)
                .map_err(|e| format!("start: {e:?}"))?;
            osc.stop_with_when(t0 + event.duration_sec)
                .map_err(|e| format!("stop: {e:?}"))?;
        }
        Ok(())
    }
}

impl MorseBackend for MorsePlayer {
    fn resume_from_gesture(&mut self) {
        MorsePlayer::resume_from_gesture(self);
    }

    fn apply_band(&mut self, settings: &TrainingSettings) -> Result<(), String> {
        self.apply_band_now(settings)
    }

    fn start_transmission(
        &mut self,
        transmission: &Transmission,
        settings: &TrainingSettings,
    ) -> Result<PlaybackWait, String> {
        self.start_send(transmission, settings)
    }

    fn stop(&mut self) {
        self.bump_epoch();
        self.stop_flag.set(true);
        self.release_group_gain();
    }

    fn shutdown(&mut self) {
        MorseBackend::stop(self);
        // Faded rather than disconnected on the spot: the fades are let go of
        // on the next send, or when the player itself goes.
        self.retire_band();
        if let Some(gain) = &self.live_gain {
            let now = self.ctx.current_time();
            let _ = gain.gain().cancel_scheduled_values(now);
            let _ = gain.gain().set_value_at_time(0.0, now);
        }
    }

    fn set_live_tone(&mut self, on: bool, frequency_hz: f64, gain: f64) {
        self.resume_from_gesture();
        if self.ensure_live_tone().is_err() {
            return;
        }
        let Some(osc) = self.live_osc.as_ref() else {
            return;
        };
        let Some(live_gain) = self.live_gain.as_ref() else {
            return;
        };
        let now = self.ctx.current_time();
        let hz = frequency_hz.clamp(100.0, 2_000.0) as f32;
        let level = if on { gain.clamp(0.0, 1.0) as f32 } else { 0.0 };
        let _ = osc.frequency().set_value_at_time(hz, now);
        let _ = live_gain.gain().cancel_scheduled_values(now);
        let current = live_gain.gain().value();
        let _ = live_gain.gain().set_value_at_time(current, now);
        let _ = live_gain
            .gain()
            .linear_ramp_to_value_at_time(level, now + 0.001);
    }

    fn take_resume_promise(&self) -> Option<js_sys::Promise> {
        self.pending_resume.borrow_mut().take()
    }
}

impl Drop for MorsePlayer {
    /// A player is rebuilt after a failed send. Without this every rebuild
    /// left its whole audio context running, and its visibility listener
    /// resuming it, for the life of the page.
    fn drop(&mut self) {
        if let Some(stream) = self.band.stream.take() {
            drop(stream.fade_out());
        }
        if let Some(osc) = self.live_osc.take() {
            let _ = osc.stop();
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
fn install_resume_on_foreground(ctx: &AudioContext) -> Option<Closure<dyn FnMut()>> {
    let ctx = ctx.clone();
    let closure = Closure::wrap(Box::new(move || {
        if page_is_hidden() || ctx.state() != AudioContextState::Suspended {
            return;
        }
        let _ = ctx.resume();
    }) as Box<dyn FnMut()>);
    let doc = web_sys::window().and_then(|window| window.document())?;
    doc.add_event_listener_with_callback("visibilitychange", closure.as_ref().unchecked_ref())
        .ok()?;
    Some(closure)
}

fn push_source(graph: &mut BandGraph, source: impl JsCast) {
    graph.sources.push(source.unchecked_into());
}

fn push_node(graph: &mut BandGraph, node: impl JsCast) {
    graph.nodes.push(node.unchecked_into());
}

fn add_qsb(
    ctx: &AudioContext,
    cw_gain: &GainNode,
    settings: &TrainingSettings,
    graph: &mut BandGraph,
) -> Result<(), String> {
    if !settings.band.qsb_enabled || settings.band.qsb_depth <= 0.0 {
        return Ok(());
    }
    let depth = settings.band.qsb_depth.clamp(0.0, 1.0);
    let rate = settings.band.qsb_rate_hz.clamp(0.03, 1.5);
    let gain_range = depth.min(1.0 - QSB_MIN_GAIN);
    let base_gain = 1.0 - gain_range / 2.0;
    cw_gain
        .gain()
        .set_value_at_time(base_gain as f32, ctx.current_time())
        .map_err(|e| format!("qsb base: {e:?}"))?;
    // One oscillator per propagation path, summed into the same gain — which
    // is the same arithmetic `qsb_gain_at` does for the native player, so the
    // browser wanders the same way rather than keeping the old tremolo. Every
    // oscillator starts at zero phase, as the shared model assumes.
    for path in 0..QSB_PATHS {
        let (multiple, weight) = qsb_path(path);
        let lfo = ctx
            .create_oscillator()
            .map_err(|e| format!("qsb osc: {e:?}"))?;
        let lfo_gain = ctx.create_gain().map_err(|e| format!("qsb gain: {e:?}"))?;
        lfo.set_type(OscillatorType::Sine);
        lfo.frequency()
            .set_value_at_time((rate * multiple) as f32, ctx.current_time())
            .map_err(|e| format!("qsb rate: {e:?}"))?;
        lfo_gain
            .gain()
            .set_value_at_time(
                (gain_range / 2.0 * weight * QSB_SPREAD) as f32,
                ctx.current_time(),
            )
            .map_err(|e| format!("qsb depth: {e:?}"))?;
        lfo.connect_with_audio_node(&lfo_gain)
            .map_err(|e| format!("qsb connect: {e:?}"))?;
        lfo_gain
            .connect_with_audio_param(&cw_gain.gain())
            .map_err(|e| format!("qsb param: {e:?}"))?;
        lfo.start().map_err(|e| format!("qsb start: {e:?}"))?;
        push_source(graph, lfo);
        push_node(graph, lfo_gain);
    }
    Ok(())
}
