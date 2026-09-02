use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cw_core::band::{QRM_OUTPUT_GAIN, QRN_OUTPUT_GAIN, QSB_MIN_GAIN, RINGING_OUTPUT_GAIN};
use cw_core::{plan_morse_playback, PlaybackPlan, QrmProfile, Rng, TrainingSettings};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{
    AudioBufferSourceNode, AudioContext, AudioContextState, AudioNode, AudioScheduledSourceNode,
    BiquadFilterType, GainNode, OscillatorType,
};

const NOISE_BUFFER_SECONDS: f32 = 2.0;

pub struct MorsePlayer {
    ctx: AudioContext,
    stop_flag: Rc<Cell<bool>>,
    epoch: Rc<Cell<u64>>,
    mix_gain: GainNode,
    cw_gain: GainNode,
    group_gain: Option<GainNode>,
    band: BandGraph,
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
        mix_gain
            .connect_with_audio_node(&ctx.destination())
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

    pub fn take_resume_promise(&self) -> Option<js_sys::Promise> {
        self.pending_resume.borrow_mut().take()
    }

    pub fn apply_band(&mut self, settings: &TrainingSettings) -> Result<(), String> {
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
        add_qrm(&self.ctx, &self.mix_gain, settings, &mut self.band)?;
        self.band.signature = signature;
        Ok(())
    }

    fn bump_epoch(&self) -> u64 {
        let next = self.epoch.get() + 1;
        self.epoch.set(next);
        next
    }

    fn release_group_gain(&mut self) {
        if let Some(gain) = self.group_gain.take() {
            let now = self.ctx.current_time();
            let _ = gain.gain().cancel_scheduled_values(now);
            let _ = gain.gain().set_target_at_time(0.0, now, 0.01);
            let _ = gain.disconnect();
        }
    }

    pub fn stop(&mut self) {
        self.bump_epoch();
        self.stop_flag.set(true);
        self.release_group_gain();
    }

    pub fn shutdown(&mut self) {
        self.stop();
        self.band.stop_layers(&self.ctx, &self.cw_gain);
    }

    pub fn reset_stop_flag(&self) {
        self.stop_flag.set(false);
    }

    pub fn start_text(
        &mut self,
        text: &str,
        settings: &TrainingSettings,
        rng: &mut impl Rng,
    ) -> Result<crate::audio::PlaybackWait, String> {
        self.resume_from_gesture();
        self.release_group_gain();
        let epoch = self.bump_epoch();
        self.apply_band(settings)?;
        let plan = plan_morse_playback(text, settings, rng);
        self.schedule_plan(&plan)?;
        Ok(crate::audio::PlaybackWait::web(
            plan.duration_sec,
            plan.resolved_char_wpm,
            self.stop_flag.clone(),
            epoch,
            self.epoch.clone(),
        ))
    }

    fn schedule_plan(&mut self, plan: &PlaybackPlan) -> Result<(), String> {
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
        for event in &plan.events {
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

fn install_resume_on_foreground(ctx: &AudioContext) {
    let ctx = ctx.clone();
    let closure = Closure::wrap(Box::new(move || {
        let hidden = web_sys::window()
            .and_then(|window| window.document())
            .map(|doc| doc.hidden())
            .unwrap_or(true);
        if hidden || ctx.state() != AudioContextState::Suspended {
            return;
        }
        let _ = ctx.resume();
    }) as Box<dyn FnMut()>);
    if let Some(doc) = web_sys::window().and_then(|window| window.document()) {
        let _ = doc.add_event_listener_with_callback(
            "visibilitychange",
            closure.as_ref().unchecked_ref(),
        );
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

fn fill_noise_buffer(ctx: &AudioContext, fill: impl FnMut(usize) -> f32) -> Result<web_sys::AudioBuffer, String> {
    fill_noise_buffer_with(ctx, fill)
}

fn fill_noise_buffer_with(
    ctx: &AudioContext,
    mut fill: impl FnMut(usize) -> f32,
) -> Result<web_sys::AudioBuffer, String> {
    let frame_count = (ctx.sample_rate() * NOISE_BUFFER_SECONDS).floor().max(1.0) as u32;
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

fn looping_source(ctx: &AudioContext, buffer: &web_sys::AudioBuffer) -> Result<AudioBufferSourceNode, String> {
    let source = ctx
        .create_buffer_source()
        .map_err(|e| format!("buffer source: {e:?}"))?;
    source.set_buffer(Some(buffer));
    source.set_loop(true);
    Ok(source)
}

fn create_white_noise(ctx: &AudioContext) -> Result<AudioBufferSourceNode, String> {
    let buffer = fill_noise_buffer(ctx, |_| fastrand::f32() * 2.0 - 1.0)?;
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
    let buffer = fill_noise_buffer(ctx, |_| {
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
    if !settings.qsb_enabled || settings.qsb_depth <= 0.0 {
        return Ok(());
    }
    let depth = settings.qsb_depth.clamp(0.0, 1.0);
    let rate = settings.qsb_rate_hz.clamp(0.03, 1.5);
    let gain_range = depth.min(1.0 - QSB_MIN_GAIN);
    let base_gain = 1.0 - gain_range / 2.0;
    let lfo = ctx.create_oscillator().map_err(|e| format!("qsb osc: {e:?}"))?;
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
    if !settings.qrn_enabled || settings.qrn_level <= 0.0 {
        return Ok(());
    }
    let level = settings.qrn_level.clamp(0.0, 1.0);
    let source = create_white_noise(ctx)?;
    let bandpass = ctx
        .create_biquad_filter()
        .map_err(|e| format!("qrn filter: {e:?}"))?;
    let gain = ctx.create_gain().map_err(|e| format!("qrn gain: {e:?}"))?;
    bandpass.set_type(BiquadFilterType::Bandpass);
    bandpass
        .frequency()
        .set_value_at_time(settings.side_tone_center() as f32, ctx.current_time())
        .map_err(|e| format!("qrn freq: {e:?}"))?;
    bandpass
        .q()
        .set_value_at_time(2.4, ctx.current_time())
        .map_err(|e| format!("qrn q: {e:?}"))?;
    gain.gain()
        .set_value_at_time((QRN_OUTPUT_GAIN * level) as f32, ctx.current_time())
        .map_err(|e| format!("qrn level: {e:?}"))?;
    source
        .connect_with_audio_node(&bandpass)
        .map_err(|e| format!("qrn src: {e:?}"))?;
    bandpass
        .connect_with_audio_node(&gain)
        .map_err(|e| format!("qrn bp: {e:?}"))?;
    gain.connect_with_audio_node(mix_gain)
        .map_err(|e| format!("qrn mix: {e:?}"))?;
    source.start().map_err(|e| format!("qrn start: {e:?}"))?;
    push_source(graph, source);
    push_node(graph, bandpass);
    push_node(graph, gain);
    Ok(())
}

fn add_passband_qrm(
    ctx: &AudioContext,
    mix_gain: &GainNode,
    settings: &TrainingSettings,
    graph: &mut BandGraph,
) -> Result<(), String> {
    let level = settings.qrm_level.clamp(0.0, 1.0);
    let model_gain = settings.receiver_background_gain.clamp(0.0, 20.0);
    let resonance = settings.receiver_background_resonance.clamp(0.5, 240.0);
    let offset_hz = settings.receiver_background_offset_hz.clamp(-1000.0, 1000.0);
    let center = settings.side_tone_center();
    let source = create_resonator_source(
        ctx,
        settings.receiver_background_excitation_rate,
        settings.receiver_background_decay,
    )?;
    let primary = ctx
        .create_biquad_filter()
        .map_err(|e| format!("qrm p: {e:?}"))?;
    let secondary = ctx
        .create_biquad_filter()
        .map_err(|e| format!("qrm s: {e:?}"))?;
    let amplitude_lfo = ctx.create_oscillator().map_err(|e| format!("qrm lfo: {e:?}"))?;
    let amplitude_gain = ctx.create_gain().map_err(|e| format!("qrm ag: {e:?}"))?;
    let gain = ctx.create_gain().map_err(|e| format!("qrm g: {e:?}"))?;
    let base_gain = QRM_OUTPUT_GAIN * level * model_gain;

    primary.set_type(BiquadFilterType::Bandpass);
    primary
        .frequency()
        .set_value_at_time((center + offset_hz) as f32, ctx.current_time())
        .map_err(|e| format!("qrm pf: {e:?}"))?;
    primary
        .q()
        .set_value_at_time(resonance as f32, ctx.current_time())
        .map_err(|e| format!("qrm pq: {e:?}"))?;
    secondary.set_type(BiquadFilterType::Bandpass);
    secondary
        .frequency()
        .set_value_at_time(
            (center - (offset_hz.abs() + 35.0).max(20.0)) as f32,
            ctx.current_time(),
        )
        .map_err(|e| format!("qrm sf: {e:?}"))?;
    secondary
        .q()
        .set_value_at_time((resonance * 0.65).max(0.5) as f32, ctx.current_time())
        .map_err(|e| format!("qrm sq: {e:?}"))?;
    add_frequency_modulation(
        ctx,
        &primary.frequency(),
        settings.receiver_background_offset_mod_depth_hz,
        settings.receiver_background_offset_mod_rate_hz,
        graph,
    )?;
    add_frequency_modulation(
        ctx,
        &secondary.frequency(),
        settings.receiver_background_offset_mod_depth_hz * 0.65,
        settings.receiver_background_offset_mod_rate_hz * 0.73,
        graph,
    )?;
    amplitude_lfo.set_type(OscillatorType::Sine);
    amplitude_lfo
        .frequency()
        .set_value_at_time(0.11, ctx.current_time())
        .map_err(|e| format!("qrm lf: {e:?}"))?;
    amplitude_gain
        .gain()
        .set_value_at_time((base_gain * 0.18) as f32, ctx.current_time())
        .map_err(|e| format!("qrm ad: {e:?}"))?;
    gain.gain()
        .set_value_at_time(base_gain as f32, ctx.current_time())
        .map_err(|e| format!("qrm bg: {e:?}"))?;
    source
        .connect_with_audio_node(&primary)
        .map_err(|e| format!("qrm srcp: {e:?}"))?;
    source
        .connect_with_audio_node(&secondary)
        .map_err(|e| format!("qrm srcs: {e:?}"))?;
    amplitude_lfo
        .connect_with_audio_node(&amplitude_gain)
        .map_err(|e| format!("qrm lfo c: {e:?}"))?;
    amplitude_gain
        .connect_with_audio_param(&gain.gain())
        .map_err(|e| format!("qrm lfo p: {e:?}"))?;
    primary
        .connect_with_audio_node(&gain)
        .map_err(|e| format!("qrm pc: {e:?}"))?;
    secondary
        .connect_with_audio_node(&gain)
        .map_err(|e| format!("qrm sc: {e:?}"))?;
    gain.connect_with_audio_node(mix_gain)
        .map_err(|e| format!("qrm mix: {e:?}"))?;
    source.start().map_err(|e| format!("qrm start: {e:?}"))?;
    amplitude_lfo
        .start()
        .map_err(|e| format!("qrm lfo start: {e:?}"))?;
    push_source(graph, source);
    push_source(graph, amplitude_lfo);
    push_node(graph, primary);
    push_node(graph, secondary);
    push_node(graph, amplitude_gain);
    push_node(graph, gain);
    Ok(())
}

fn add_ringing_qrm(
    ctx: &AudioContext,
    mix_gain: &GainNode,
    settings: &TrainingSettings,
    graph: &mut BandGraph,
) -> Result<(), String> {
    let level = settings.qrm_level.clamp(0.0, 1.0);
    let model_gain = settings.receiver_background_gain.clamp(0.0, 20.0);
    let resonance = settings.receiver_background_resonance.clamp(0.5, 240.0);
    let offset_hz = settings.receiver_background_offset_hz.clamp(-1000.0, 1000.0);
    let center = settings.side_tone_center();
    let source = create_resonator_source(
        ctx,
        settings.receiver_background_excitation_rate,
        settings.receiver_background_decay,
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
        settings.receiver_background_offset_mod_depth_hz,
        settings.receiver_background_offset_mod_rate_hz,
        graph,
    )?;
    gain.gain()
        .set_value_at_time((RINGING_OUTPUT_GAIN * level * model_gain) as f32, ctx.current_time())
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

fn add_qrm(
    ctx: &AudioContext,
    mix_gain: &GainNode,
    settings: &TrainingSettings,
    graph: &mut BandGraph,
) -> Result<(), String> {
    if !settings.qrm_enabled || settings.qrm_level <= 0.0 {
        return Ok(());
    }
    if matches!(settings.qrm_profile, QrmProfile::Whistle | QrmProfile::Mixed) {
        add_passband_qrm(ctx, mix_gain, settings, graph)?;
    }
    if matches!(settings.qrm_profile, QrmProfile::Ringing | QrmProfile::Mixed) {
        add_ringing_qrm(ctx, mix_gain, settings, graph)?;
    }
    Ok(())
}
