use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cw_core::band::{
    shaped_level, AtmosphericNoise, QRN_OUTPUT_GAIN, QSB_MIN_GAIN, RECEIVER_OUTPUT_GAIN,
    RECEIVER_STAGES, RINGING_OUTPUT_GAIN,
};
use cw_core::{
    plan_transmission, PlannedTransmission, ReceiverProfile, TrainingSettings, Transmission,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{
    AudioBufferSourceNode, AudioContext, AudioContextState, AudioNode, AudioScheduledSourceNode,
    BiquadFilterType, GainNode, OscillatorType,
};

use super::{MorseBackend, PlaybackSignal, PlaybackWait, WaitFlags};

const NOISE_BUFFER_SECONDS: f32 = 2.0;

/// Static is sparse, so its loop has to be long enough that the same crashes
/// do not come round in a recognisable pattern.
const ATMOSPHERIC_BUFFER_SECONDS: f32 = 12.0;

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
    /// The receiver's own filter, as a cascade of band-pass nodes.
    receiver: Vec<web_sys::BiquadFilterNode>,
    /// Group gains that are fading out, with the context time their ramp ends.
    released: Vec<(GainNode, f64)>,
    pending_resume: RefCell<Option<js_sys::Promise>>,
}

struct BandGraph {
    sources: Vec<AudioScheduledSourceNode>,
    nodes: Vec<AudioNode>,
    signature: String,
}

impl BandGraph {
    fn new() -> Self {
        Self {
            sources: Vec::new(),
            nodes: Vec::new(),
            signature: String::new(),
        }
    }

    fn stop_layers(&mut self, ctx: &AudioContext, cw_gain: &GainNode) {
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
        let mut receiver = Vec::with_capacity(RECEIVER_STAGES);
        let mut tail: AudioNode = mix_gain.clone().unchecked_into();
        for _ in 0..RECEIVER_STAGES {
            let stage = ctx
                .create_biquad_filter()
                .map_err(|e| format!("receiver filter: {e:?}"))?;
            stage.set_type(BiquadFilterType::Bandpass);
            tail.connect_with_audio_node(&stage)
                .map_err(|e| format!("receiver connect: {e:?}"))?;
            tail = stage.clone().unchecked_into();
            receiver.push(stage);
        }
        tail.connect_with_audio_node(&ctx.destination())
            .map_err(|e| format!("mix connect: {e:?}"))?;
        install_resume_on_foreground(&ctx);
        Ok(Self {
            ctx,
            stop_flag: Rc::new(Cell::new(false)),
            epoch: Rc::new(Cell::new(0)),
            mix_gain,
            cw_gain,
            group_gain: None,
            band: BandGraph::new(),
            receiver,
            released: Vec::new(),
            pending_resume: RefCell::new(None),
        })
    }

    pub fn resume_from_gesture(&self) {
        if self.ctx.state() == AudioContextState::Suspended {
            if let Ok(promise) = self.ctx.resume() {
                *self.pending_resume.borrow_mut() = Some(promise);
            }
        }
    }

    /// Retune the receiver to the current pitch and width. Cheap, and the
    /// nodes stay put, so this can run on every send.
    fn tune_receiver(&self, settings: &TrainingSettings) {
        let now = self.ctx.current_time();
        let center = settings.side_tone_center();
        let q = cw_core::band::receiver_stage_q(center, settings.band.filter_bandwidth_hz);
        for stage in &self.receiver {
            let _ = stage.frequency().set_value_at_time(center as f32, now);
            let _ = stage.q().set_value_at_time(q as f32, now);
        }
    }

    fn apply_band_now(&mut self, settings: &TrainingSettings) -> Result<(), String> {
        self.tune_receiver(settings);
        let signature = settings.band_signature();
        if signature == self.band.signature {
            return Ok(());
        }
        self.band.stop_layers(&self.ctx, &self.cw_gain);
        if self.ctx.state() == AudioContextState::Closed {
            return Ok(());
        }
        add_qsb(&self.ctx, &self.cw_gain, settings, &mut self.band)?;
        add_qrn(&self.ctx, &self.mix_gain, settings, &mut self.band)?;
        add_receiver(&self.ctx, &self.mix_gain, settings, &mut self.band)?;
        self.band.signature = signature;
        Ok(())
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
        self.band.stop_layers(&self.ctx, &self.cw_gain);
        for (gain, _) in self.released.drain(..) {
            let _ = gain.disconnect();
        }
    }

    fn take_resume_promise(&self) -> Option<js_sys::Promise> {
        self.pending_resume.borrow_mut().take()
    }
}

fn install_resume_on_foreground(ctx: &AudioContext) {
    let ctx = ctx.clone();
    let closure = Closure::wrap(Box::new(move || {
        if page_is_hidden() || ctx.state() != AudioContextState::Suspended {
            return;
        }
        let _ = ctx.resume();
    }) as Box<dyn FnMut()>);
    if let Some(doc) = web_sys::window().and_then(|window| window.document()) {
        let _ = doc
            .add_event_listener_with_callback("visibilitychange", closure.as_ref().unchecked_ref());
    }
    closure.forget();
}

fn push_source(graph: &mut BandGraph, source: impl JsCast) {
    graph.sources.push(source.unchecked_into());
}

fn push_node(graph: &mut BandGraph, node: impl JsCast) {
    graph.nodes.push(node.unchecked_into());
}

fn add_frequency_modulation(
    ctx: &AudioContext,
    target: &web_sys::AudioParam,
    depth_hz: f64,
    rate_hz: f64,
    graph: &mut BandGraph,
) -> Result<(), String> {
    let depth = depth_hz.clamp(0.0, 1000.0);
    let rate = rate_hz.clamp(0.0, 20.0);
    if depth <= 0.0 || rate <= 0.0 {
        return Ok(());
    }
    let oscillator = ctx
        .create_oscillator()
        .map_err(|e| format!("fm osc: {e:?}"))?;
    let gain = ctx.create_gain().map_err(|e| format!("fm gain: {e:?}"))?;
    oscillator.set_type(OscillatorType::Sine);
    oscillator
        .frequency()
        .set_value_at_time(rate as f32, ctx.current_time())
        .map_err(|e| format!("fm freq: {e:?}"))?;
    gain.gain()
        .set_value_at_time(depth as f32, ctx.current_time())
        .map_err(|e| format!("fm depth: {e:?}"))?;
    oscillator
        .connect_with_audio_node(&gain)
        .map_err(|e| format!("fm connect: {e:?}"))?;
    gain.connect_with_audio_param(target)
        .map_err(|e| format!("fm param: {e:?}"))?;
    oscillator.start().map_err(|e| format!("fm start: {e:?}"))?;
    push_source(graph, oscillator);
    push_node(graph, gain);
    Ok(())
}

fn fill_noise_buffer_seconds(
    ctx: &AudioContext,
    seconds: f32,
    mut fill: impl FnMut(usize) -> f32,
) -> Result<web_sys::AudioBuffer, String> {
    let frame_count = (ctx.sample_rate() * seconds).floor().max(1.0) as u32;
    let buffer = ctx
        .create_buffer(1, frame_count, ctx.sample_rate())
        .map_err(|e| format!("buffer: {e:?}"))?;
    let mut samples = vec![0.0f32; frame_count as usize];
    for (i, slot) in samples.iter_mut().enumerate() {
        *slot = fill(i);
    }
    buffer
        .copy_to_channel(&mut samples, 0)
        .map_err(|e| format!("copy channel: {e:?}"))?;
    Ok(buffer)
}

fn looping_source(
    ctx: &AudioContext,
    buffer: &web_sys::AudioBuffer,
) -> Result<AudioBufferSourceNode, String> {
    let source = ctx
        .create_buffer_source()
        .map_err(|e| format!("buffer source: {e:?}"))?;
    source.set_buffer(Some(buffer));
    source.set_loop(true);
    Ok(source)
}

/// Atmospheric static, from the same model the native player uses — crashes
/// rather than hiss — rendered into a long loop because Web Audio has no place
/// to run a per-sample generator.
fn create_atmospheric_noise(
    ctx: &AudioContext,
    level: f64,
) -> Result<AudioBufferSourceNode, String> {
    let sample_rate = ctx.sample_rate();
    let mut model = AtmosphericNoise::new(sample_rate as u32, level, fastrand::u64(..) | 1);
    let buffer = fill_noise_buffer_seconds(ctx, ATMOSPHERIC_BUFFER_SECONDS, move |_| {
        model.next_sample() as f32
    })?;
    looping_source(ctx, &buffer)
}

fn create_resonator_source(
    ctx: &AudioContext,
    excitation_rate: f64,
    decay: f64,
) -> Result<AudioBufferSourceNode, String> {
    let sample_rate = f64::from(ctx.sample_rate());
    let impulse_p = excitation_rate.clamp(0.1, 500.0) / sample_rate;
    let ring_decay = decay.clamp(0.5, 0.9999);
    let mut ringing_energy = 0.0f32;
    let mut peak = 0.0f32;
    let buffer = fill_noise_buffer_seconds(ctx, NOISE_BUFFER_SECONDS, |_| {
        if fastrand::f64() < impulse_p {
            ringing_energy += (fastrand::f32() * 2.0 - 1.0) * (0.6 + fastrand::f32() * 0.4);
        }
        ringing_energy *= ring_decay as f32;
        let grain = ringing_energy + (fastrand::f32() * 2.0 - 1.0) * 0.015;
        peak = peak.max(grain.abs());
        grain
    })?;
    if peak > 0.0 {
        let mut samples = vec![0.0f32; buffer.length() as usize];
        buffer
            .copy_from_channel(&mut samples, 0)
            .map_err(|e| format!("copy from: {e:?}"))?;
        for sample in &mut samples {
            *sample /= peak;
        }
        buffer
            .copy_to_channel(&mut samples, 0)
            .map_err(|e| format!("copy norm: {e:?}"))?;
    }
    looping_source(ctx, &buffer)
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
    let lfo = ctx
        .create_oscillator()
        .map_err(|e| format!("qsb osc: {e:?}"))?;
    let lfo_gain = ctx.create_gain().map_err(|e| format!("qsb gain: {e:?}"))?;
    cw_gain
        .gain()
        .set_value_at_time(base_gain as f32, ctx.current_time())
        .map_err(|e| format!("qsb base: {e:?}"))?;
    lfo.set_type(OscillatorType::Sine);
    lfo.frequency()
        .set_value_at_time(rate as f32, ctx.current_time())
        .map_err(|e| format!("qsb rate: {e:?}"))?;
    lfo_gain
        .gain()
        .set_value_at_time((gain_range / 2.0) as f32, ctx.current_time())
        .map_err(|e| format!("qsb depth: {e:?}"))?;
    lfo.connect_with_audio_node(&lfo_gain)
        .map_err(|e| format!("qsb connect: {e:?}"))?;
    lfo_gain
        .connect_with_audio_param(&cw_gain.gain())
        .map_err(|e| format!("qsb param: {e:?}"))?;
    lfo.start().map_err(|e| format!("qsb start: {e:?}"))?;
    push_source(graph, lfo);
    push_node(graph, lfo_gain);
    Ok(())
}

fn add_qrn(
    ctx: &AudioContext,
    mix_gain: &GainNode,
    settings: &TrainingSettings,
    graph: &mut BandGraph,
) -> Result<(), String> {
    if !settings.band.qrn_enabled || settings.band.qrn_level <= 0.0 {
        return Ok(());
    }
    let level = settings.band.qrn_level.clamp(0.0, 1.0);
    let source = create_atmospheric_noise(ctx, level)?;
    let gain = ctx.create_gain().map_err(|e| format!("qrn gain: {e:?}"))?;
    gain.gain()
        .set_value_at_time(QRN_OUTPUT_GAIN as f32, ctx.current_time())
        .map_err(|e| format!("qrn level: {e:?}"))?;
    source
        .connect_with_audio_node(&gain)
        .map_err(|e| format!("qrn bp: {e:?}"))?;
    gain.connect_with_audio_node(mix_gain)
        .map_err(|e| format!("qrn mix: {e:?}"))?;
    source.start().map_err(|e| format!("qrn start: {e:?}"))?;
    push_source(graph, source);
    push_node(graph, gain);
    Ok(())
}

fn add_passband_receiver(
    ctx: &AudioContext,
    mix_gain: &GainNode,
    settings: &TrainingSettings,
    graph: &mut BandGraph,
) -> Result<(), String> {
    let level = settings.band.receiver_level.clamp(0.0, 1.0);
    let model_gain = settings.band.receiver_background_gain.clamp(0.0, 20.0);
    let resonance = settings
        .band
        .receiver_background_resonance
        .clamp(0.5, 240.0);
    let offset_hz = settings
        .band
        .receiver_background_offset_hz
        .clamp(-1000.0, 1000.0);
    let center = settings.side_tone_center();
    let source = create_resonator_source(
        ctx,
        settings.band.receiver_background_excitation_rate,
        settings.band.receiver_background_decay,
    )?;
    let primary = ctx
        .create_biquad_filter()
        .map_err(|e| format!("receiver p: {e:?}"))?;
    let secondary = ctx
        .create_biquad_filter()
        .map_err(|e| format!("receiver s: {e:?}"))?;
    let amplitude_lfo = ctx
        .create_oscillator()
        .map_err(|e| format!("receiver lfo: {e:?}"))?;
    let amplitude_gain = ctx
        .create_gain()
        .map_err(|e| format!("receiver ag: {e:?}"))?;
    let gain = ctx
        .create_gain()
        .map_err(|e| format!("receiver g: {e:?}"))?;
    let base_gain = RECEIVER_OUTPUT_GAIN * shaped_level(level) * model_gain;

    primary.set_type(BiquadFilterType::Bandpass);
    primary
        .frequency()
        .set_value_at_time((center + offset_hz) as f32, ctx.current_time())
        .map_err(|e| format!("receiver pf: {e:?}"))?;
    primary
        .q()
        .set_value_at_time(resonance as f32, ctx.current_time())
        .map_err(|e| format!("receiver pq: {e:?}"))?;
    secondary.set_type(BiquadFilterType::Bandpass);
    secondary
        .frequency()
        .set_value_at_time(
            (center - (offset_hz.abs() + 35.0).max(20.0)) as f32,
            ctx.current_time(),
        )
        .map_err(|e| format!("receiver sf: {e:?}"))?;
    secondary
        .q()
        .set_value_at_time((resonance * 0.65).max(0.5) as f32, ctx.current_time())
        .map_err(|e| format!("receiver sq: {e:?}"))?;
    add_frequency_modulation(
        ctx,
        &primary.frequency(),
        settings.band.receiver_background_offset_mod_depth_hz,
        settings.band.receiver_background_offset_mod_rate_hz,
        graph,
    )?;
    add_frequency_modulation(
        ctx,
        &secondary.frequency(),
        settings.band.receiver_background_offset_mod_depth_hz * 0.65,
        settings.band.receiver_background_offset_mod_rate_hz * 0.73,
        graph,
    )?;
    amplitude_lfo.set_type(OscillatorType::Sine);
    amplitude_lfo
        .frequency()
        .set_value_at_time(0.11, ctx.current_time())
        .map_err(|e| format!("receiver lf: {e:?}"))?;
    amplitude_gain
        .gain()
        .set_value_at_time((base_gain * 0.18) as f32, ctx.current_time())
        .map_err(|e| format!("receiver ad: {e:?}"))?;
    gain.gain()
        .set_value_at_time(base_gain as f32, ctx.current_time())
        .map_err(|e| format!("receiver bg: {e:?}"))?;
    source
        .connect_with_audio_node(&primary)
        .map_err(|e| format!("receiver srcp: {e:?}"))?;
    source
        .connect_with_audio_node(&secondary)
        .map_err(|e| format!("receiver srcs: {e:?}"))?;
    amplitude_lfo
        .connect_with_audio_node(&amplitude_gain)
        .map_err(|e| format!("receiver lfo c: {e:?}"))?;
    amplitude_gain
        .connect_with_audio_param(&gain.gain())
        .map_err(|e| format!("receiver lfo p: {e:?}"))?;
    primary
        .connect_with_audio_node(&gain)
        .map_err(|e| format!("receiver pc: {e:?}"))?;
    secondary
        .connect_with_audio_node(&gain)
        .map_err(|e| format!("receiver sc: {e:?}"))?;
    gain.connect_with_audio_node(mix_gain)
        .map_err(|e| format!("receiver mix: {e:?}"))?;
    source
        .start()
        .map_err(|e| format!("receiver start: {e:?}"))?;
    amplitude_lfo
        .start()
        .map_err(|e| format!("receiver lfo start: {e:?}"))?;
    push_source(graph, source);
    push_source(graph, amplitude_lfo);
    push_node(graph, primary);
    push_node(graph, secondary);
    push_node(graph, amplitude_gain);
    push_node(graph, gain);
    Ok(())
}

fn add_ringing_receiver(
    ctx: &AudioContext,
    mix_gain: &GainNode,
    settings: &TrainingSettings,
    graph: &mut BandGraph,
) -> Result<(), String> {
    let level = settings.band.receiver_level.clamp(0.0, 1.0);
    let model_gain = settings.band.receiver_background_gain.clamp(0.0, 20.0);
    let resonance = settings
        .band
        .receiver_background_resonance
        .clamp(0.5, 240.0);
    let offset_hz = settings
        .band
        .receiver_background_offset_hz
        .clamp(-1000.0, 1000.0);
    let center = settings.side_tone_center();
    let source = create_resonator_source(
        ctx,
        settings.band.receiver_background_excitation_rate,
        settings.band.receiver_background_decay,
    )?;
    let filter = ctx
        .create_biquad_filter()
        .map_err(|e| format!("ring f: {e:?}"))?;
    let gain = ctx.create_gain().map_err(|e| format!("ring g: {e:?}"))?;
    filter.set_type(BiquadFilterType::Bandpass);
    filter
        .frequency()
        .set_value_at_time((center + offset_hz - 35.0) as f32, ctx.current_time())
        .map_err(|e| format!("ring freq: {e:?}"))?;
    filter
        .q()
        .set_value_at_time((resonance * 1.45).min(320.0) as f32, ctx.current_time())
        .map_err(|e| format!("ring q: {e:?}"))?;
    add_frequency_modulation(
        ctx,
        &filter.frequency(),
        settings.band.receiver_background_offset_mod_depth_hz,
        settings.band.receiver_background_offset_mod_rate_hz,
        graph,
    )?;
    gain.gain()
        .set_value_at_time(
            (RINGING_OUTPUT_GAIN * shaped_level(level) * model_gain) as f32,
            ctx.current_time(),
        )
        .map_err(|e| format!("ring level: {e:?}"))?;
    source
        .connect_with_audio_node(&filter)
        .map_err(|e| format!("ring src: {e:?}"))?;
    filter
        .connect_with_audio_node(&gain)
        .map_err(|e| format!("ring fc: {e:?}"))?;
    gain.connect_with_audio_node(mix_gain)
        .map_err(|e| format!("ring mix: {e:?}"))?;
    source.start().map_err(|e| format!("ring start: {e:?}"))?;
    push_source(graph, source);
    push_node(graph, filter);
    push_node(graph, gain);
    Ok(())
}

fn add_receiver(
    ctx: &AudioContext,
    mix_gain: &GainNode,
    settings: &TrainingSettings,
    graph: &mut BandGraph,
) -> Result<(), String> {
    if !settings.band.receiver_enabled || settings.band.receiver_level <= 0.0 {
        return Ok(());
    }
    if matches!(
        settings.band.receiver_profile,
        ReceiverProfile::Whistle | ReceiverProfile::Mixed
    ) {
        add_passband_receiver(ctx, mix_gain, settings, graph)?;
    }
    if matches!(
        settings.band.receiver_profile,
        ReceiverProfile::Ringing | ReceiverProfile::Mixed
    ) {
        add_ringing_receiver(ctx, mix_gain, settings, graph)?;
    }
    Ok(())
}
