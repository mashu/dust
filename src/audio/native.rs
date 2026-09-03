use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use cw_core::band::{qsb_gain_at, BandMixer};
use cw_core::{plan_morse_playback, PlaybackPlan, Rng, ToneEvent, TrainingSettings};

struct LiveQsb {
    enabled: AtomicBool,
    depth_bits: AtomicU64,
    rate_bits: AtomicU64,
}

impl LiveQsb {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            depth_bits: AtomicU64::new(0.0f64.to_bits()),
            rate_bits: AtomicU64::new(0.12f64.to_bits()),
        })
    }

    fn store(&self, settings: &TrainingSettings) {
        self.enabled
            .store(settings.band.qsb_enabled, Ordering::Relaxed);
        self.depth_bits
            .store(settings.band.qsb_depth.to_bits(), Ordering::Relaxed);
        self.rate_bits
            .store(settings.band.qsb_rate_hz.to_bits(), Ordering::Relaxed);
    }

    fn gain_at(&self, t_sec: f64) -> f32 {
        qsb_gain_at(
            t_sec,
            self.enabled.load(Ordering::Relaxed),
            f64::from_bits(self.depth_bits.load(Ordering::Relaxed)),
            f64::from_bits(self.rate_bits.load(Ordering::Relaxed)),
        )
    }
}

pub struct MorsePlayer {
    stop_flag: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    band_stop: Arc<AtomicBool>,
    band_stream: Option<cpal::Stream>,
    band_signature: String,
    tone_stream: Option<cpal::Stream>,
    tone_finished: Arc<AtomicBool>,
    qsb: Arc<LiveQsb>,
}

impl MorsePlayer {
    pub fn new() -> Result<Self, String> {
        let _ = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| "No audio output device found".to_string())?;
        Ok(Self {
            stop_flag: Arc::new(AtomicBool::new(false)),
            epoch: Arc::new(AtomicU64::new(0)),
            band_stop: Arc::new(AtomicBool::new(false)),
            band_stream: None,
            band_signature: String::new(),
            tone_stream: None,
            tone_finished: Arc::new(AtomicBool::new(true)),
            qsb: LiveQsb::new(),
        })
    }

    pub fn resume_from_gesture(&self) {}

    pub fn apply_band(&mut self, settings: &TrainingSettings) -> Result<(), String> {
        self.qsb.store(settings);
        let signature = settings.band_signature();
        if signature == self.band_signature {
            return Ok(());
        }
        self.stop_band();
        if !BandMixer::needs_background(settings) {
            self.band_signature = signature;
            return Ok(());
        }
        self.band_stream = Some(start_band_stream(settings, Arc::clone(&self.band_stop))?);
        self.band_signature = signature;
        Ok(())
    }

    fn stop_band(&mut self) {
        self.band_stop.store(true, Ordering::SeqCst);
        self.band_stream = None;
        self.band_stop = Arc::new(AtomicBool::new(false));
        self.band_signature.clear();
    }

    fn bump_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn stop(&mut self) {
        self.bump_epoch();
        self.stop_flag.store(true, Ordering::SeqCst);
        self.tone_stream = None;
        self.tone_finished.store(true, Ordering::SeqCst);
    }

    pub fn shutdown(&mut self) {
        self.stop();
        self.stop_band();
    }

    pub fn reset_stop_flag(&self) {
        self.stop_flag.store(false, Ordering::SeqCst);
    }

    pub fn start_text(
        &mut self,
        text: &str,
        settings: &TrainingSettings,
        rng: &mut impl Rng,
    ) -> Result<crate::audio::PlaybackWait, String> {
        self.apply_band(settings)?;
        let plan = plan_morse_playback(text, settings, rng);
        let epoch = self.bump_epoch();
        self.reset_stop_flag();
        self.tone_stream = None;
        let (stream, finished) =
            start_tone_stream(&plan, Arc::clone(&self.qsb), Arc::clone(&self.stop_flag))?;
        self.tone_finished = Arc::clone(&finished);
        self.tone_stream = Some(stream);
        Ok(crate::audio::PlaybackWait::desktop(
            plan.duration_sec,
            plan.resolved_char_wpm,
            plan.resolved_effective_wpm,
            Arc::clone(&self.stop_flag),
            epoch,
            Arc::clone(&self.epoch),
            finished,
        ))
    }
}

fn envelope_gain(event: &ToneEvent, t: f64) -> f32 {
    let curve = &event.envelope;
    if curve.len() < 2 || event.duration_sec <= 0.0 {
        return event.target_gain as f32;
    }
    let rel = (t / event.duration_sec).clamp(0.0, 1.0);
    let pos = rel * (curve.len() - 1) as f64;
    let i = pos.floor() as usize;
    let frac = (pos - i as f64) as f32;
    let a = curve.get(i).copied().unwrap_or(0.0);
    let b = curve.get(i + 1).copied().unwrap_or(a);
    a * (1.0 - frac) + b * frac
}

fn render_plan(plan: &PlaybackPlan, sample_rate: u32) -> Vec<f32> {
    let extra = sample_rate / 20;
    let n = ((plan.duration_sec * f64::from(sample_rate)).ceil() as usize)
        .saturating_add(extra as usize);
    let mut buf = vec![0.0f32; n.max(1)];
    let sr = f64::from(sample_rate);
    for event in &plan.events {
        let start = (event.start_sec * sr).round() as usize;
        let len = ((event.duration_sec * sr).round() as usize).max(1);
        let two_pi_f = 2.0 * std::f64::consts::PI * event.frequency_hz;
        for i in 0..len {
            let t = i as f64 / sr;
            let sample = (two_pi_f * t).sin() as f32 * envelope_gain(event, t);
            if let Some(slot) = buf.get_mut(start + i) {
                *slot += sample;
            }
        }
    }
    buf
}

fn start_tone_stream(
    plan: &PlaybackPlan,
    qsb: Arc<LiveQsb>,
    stop: Arc<AtomicBool>,
) -> Result<(cpal::Stream, Arc<AtomicBool>), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "No audio output device found".to_string())?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("Audio config: {e}"))?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let samples = render_plan(plan, sample_rate);
    let pos = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicBool::new(false));

    let err_fn = |err| eprintln!("audio stream error: {err}");
    let stream = match config.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(
            &device,
            &config.into(),
            samples,
            sample_rate,
            channels,
            qsb,
            Arc::clone(&pos),
            Arc::clone(&stop),
            Arc::clone(&finished),
            err_fn,
        )?,
        SampleFormat::F64 => build_stream::<f64>(
            &device,
            &config.into(),
            samples,
            sample_rate,
            channels,
            qsb,
            Arc::clone(&pos),
            Arc::clone(&stop),
            Arc::clone(&finished),
            err_fn,
        )?,
        SampleFormat::I16 => build_stream::<i16>(
            &device,
            &config.into(),
            samples,
            sample_rate,
            channels,
            qsb,
            Arc::clone(&pos),
            Arc::clone(&stop),
            Arc::clone(&finished),
            err_fn,
        )?,
        SampleFormat::I32 => build_stream::<i32>(
            &device,
            &config.into(),
            samples,
            sample_rate,
            channels,
            qsb,
            Arc::clone(&pos),
            Arc::clone(&stop),
            Arc::clone(&finished),
            err_fn,
        )?,
        SampleFormat::U16 => build_stream::<u16>(
            &device,
            &config.into(),
            samples,
            sample_rate,
            channels,
            qsb,
            Arc::clone(&pos),
            Arc::clone(&stop),
            Arc::clone(&finished),
            err_fn,
        )?,
        SampleFormat::U32 => build_stream::<u32>(
            &device,
            &config.into(),
            samples,
            sample_rate,
            channels,
            qsb,
            Arc::clone(&pos),
            Arc::clone(&stop),
            Arc::clone(&finished),
            err_fn,
        )?,
        SampleFormat::I8 => build_stream::<i8>(
            &device,
            &config.into(),
            samples,
            sample_rate,
            channels,
            qsb,
            Arc::clone(&pos),
            Arc::clone(&stop),
            Arc::clone(&finished),
            err_fn,
        )?,
        SampleFormat::U8 => build_stream::<u8>(
            &device,
            &config.into(),
            samples,
            sample_rate,
            channels,
            qsb,
            Arc::clone(&pos),
            Arc::clone(&stop),
            Arc::clone(&finished),
            err_fn,
        )?,
        other => return Err(format!("Unsupported sample format: {other}")),
    };
    stream.play().map_err(|e| format!("Audio play: {e}"))?;
    Ok((stream, finished))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: Vec<f32>,
    sample_rate: u32,
    channels: usize,
    qsb: Arc<LiveQsb>,
    pos: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: Sample + SizedSample + FromSample<f32>,
{
    let channels = channels.max(1);
    let sr = f64::from(sample_rate.max(1));
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                if stop.load(Ordering::SeqCst) {
                    for sample in output.iter_mut() {
                        *sample = T::from_sample(0.0);
                    }
                    finished.store(true, Ordering::SeqCst);
                    return;
                }
                let mut i = pos.load(Ordering::SeqCst);
                let mut out_i = 0;
                while out_i < output.len() {
                    let dry = samples.get(i).copied().unwrap_or(0.0);
                    let value = dry * qsb.gain_at(i as f64 / sr);
                    for _ in 0..channels {
                        if out_i >= output.len() {
                            break;
                        }
                        output[out_i] = T::from_sample(value);
                        out_i += 1;
                    }
                    i += 1;
                    if i >= samples.len() {
                        finished.store(true, Ordering::SeqCst);
                        while out_i < output.len() {
                            output[out_i] = T::from_sample(0.0);
                            out_i += 1;
                        }
                        break;
                    }
                }
                pos.store(i, Ordering::SeqCst);
                if i >= samples.len() {
                    finished.store(true, Ordering::SeqCst);
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| format!("Audio stream: {e}"))
}

fn start_band_stream(
    settings: &TrainingSettings,
    stop: Arc<AtomicBool>,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "No audio output device found".to_string())?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("Audio config: {e}"))?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels().max(1) as usize;
    let mut mixer = BandMixer::new(sample_rate, settings, sample_rate as u64);
    let err_fn = |err| eprintln!("band stream error: {err}");
    let stream_config: cpal::StreamConfig = config.clone().into();
    let stream = match config.sample_format() {
        SampleFormat::F32 => device
            .build_output_stream(
                &stream_config,
                move |output: &mut [f32], _| fill_band(output, channels, &mut mixer, &stop),
                err_fn,
                None,
            )
            .map_err(|e| format!("Band stream: {e}"))?,
        SampleFormat::F64 => device
            .build_output_stream(
                &stream_config,
                move |output: &mut [f64], _| fill_band(output, channels, &mut mixer, &stop),
                err_fn,
                None,
            )
            .map_err(|e| format!("Band stream: {e}"))?,
        SampleFormat::I16 => device
            .build_output_stream(
                &stream_config,
                move |output: &mut [i16], _| fill_band(output, channels, &mut mixer, &stop),
                err_fn,
                None,
            )
            .map_err(|e| format!("Band stream: {e}"))?,
        SampleFormat::I32 => device
            .build_output_stream(
                &stream_config,
                move |output: &mut [i32], _| fill_band(output, channels, &mut mixer, &stop),
                err_fn,
                None,
            )
            .map_err(|e| format!("Band stream: {e}"))?,
        SampleFormat::U16 => device
            .build_output_stream(
                &stream_config,
                move |output: &mut [u16], _| fill_band(output, channels, &mut mixer, &stop),
                err_fn,
                None,
            )
            .map_err(|e| format!("Band stream: {e}"))?,
        SampleFormat::U32 => device
            .build_output_stream(
                &stream_config,
                move |output: &mut [u32], _| fill_band(output, channels, &mut mixer, &stop),
                err_fn,
                None,
            )
            .map_err(|e| format!("Band stream: {e}"))?,
        SampleFormat::I8 => device
            .build_output_stream(
                &stream_config,
                move |output: &mut [i8], _| fill_band(output, channels, &mut mixer, &stop),
                err_fn,
                None,
            )
            .map_err(|e| format!("Band stream: {e}"))?,
        SampleFormat::U8 => device
            .build_output_stream(
                &stream_config,
                move |output: &mut [u8], _| fill_band(output, channels, &mut mixer, &stop),
                err_fn,
                None,
            )
            .map_err(|e| format!("Band stream: {e}"))?,
        other => return Err(format!("Unsupported sample format: {other}")),
    };
    stream.play().map_err(|e| format!("Band play: {e}"))?;
    Ok(stream)
}

fn fill_band<T: Sample + FromSample<f32>>(
    output: &mut [T],
    channels: usize,
    mixer: &mut BandMixer,
    stop: &AtomicBool,
) {
    if stop.load(Ordering::SeqCst) {
        for sample in output.iter_mut() {
            *sample = T::from_sample(0.0);
        }
        return;
    }
    let channels = channels.max(1);
    let mut i = 0;
    while i < output.len() {
        let value = mixer.next_background();
        for _ in 0..channels {
            if i >= output.len() {
                break;
            }
            output[i] = T::from_sample(value);
            i += 1;
        }
    }
}
