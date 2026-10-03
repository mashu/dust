//! cpal output: ALSA/CoreAudio/WASAPI on desktop, AAudio through oboe on Android.
//!
//! Everything here that can run without a sound card lives in
//! [`super::render`] and [`state`]; what is left is the device glue.

mod state;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, Sample, SampleFormat, SizedSample};
use cw_core::band::{BandMixer, ReceiverFilter};
use cw_core::{plan_transmission, TrainingSettings, Transmission};

use super::render::{mix_plan_into, render_plan, BandPlayback, LiveAgc, LiveQsb, TonePlayback};
use super::{MorseBackend, PlaybackWait};
use state::{PlayerState, ToneSignal};

/// Band and Morse share the paddle's output stream. Three device streams on
/// Pulse/PipeWire underrun: the sidetone stutters, turns to noise, then drops.
struct MixerSlots {
    band: Mutex<Option<BandPlayback>>,
    tone: Mutex<Option<Arc<TonePlayback>>>,
}

pub struct MorsePlayer {
    state: PlayerState,
    band_stop: Arc<AtomicBool>,
    slots: Arc<MixerSlots>,
    /// The one output stream. Morse, receiver hiss and paddle sidetone are
    /// mixed here so the device is not asked for three concurrent writers.
    stream: Option<cpal::Stream>,
    sample_rate: u32,
    qsb: Arc<LiveQsb>,
    /// What the background's AGC is holding everything down to.
    agc: Arc<LiveAgc>,
    /// When the player opened. Fading is read off this clock so it keeps
    /// running between groups instead of restarting with every send.
    opened_at: Instant,
    live: Arc<LiveSidetone>,
}

/// iOS starts an app with no audio session, which leaves output silent, tied to
/// the ringer switch and interrupted by anything else on the device. Claiming
/// the playback category before the first stream opens is what makes the Morse
/// audible — cpal does not do it, and there is no equivalent on other platforms.
#[cfg(target_os = "ios")]
fn claim_audio_session() {
    use objc2_avf_audio::{AVAudioSession, AVAudioSessionCategoryPlayback};

    // Safety: both calls are plain messages to the process-wide shared session,
    // and every failure is reported through the returned NSError rather than a
    // trap. Nothing here can leave the session half-configured.
    unsafe {
        let session = AVAudioSession::sharedInstance();
        if let Some(playback) = AVAudioSessionCategoryPlayback {
            let _ = session.setCategory_error(playback);
        }
        let _ = session.setActive_error(true);
    }
}

impl MorsePlayer {
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "ios")]
        claim_audio_session();
        let (_, supported) = default_output()?;
        let sample_rate = supported.sample_rate().0;
        let live = LiveSidetone::new();
        let slots = Arc::new(MixerSlots {
            band: Mutex::new(None),
            tone: Mutex::new(None),
        });
        let stream = start_mixer(Arc::clone(&live), Arc::clone(&slots)).ok();
        Ok(Self {
            state: PlayerState::new(),
            band_stop: Arc::new(AtomicBool::new(false)),
            slots,
            stream,
            sample_rate,
            qsb: LiveQsb::new(),
            agc: LiveAgc::new(),
            opened_at: Instant::now(),
            live,
        })
    }

    /// Tell the background to fade. The mixer keeps playing it until a new
    /// band replaces it, so the release is heard rather than a click.
    fn stop_band(&mut self) {
        self.band_stop.store(true, Ordering::SeqCst);
        self.band_stop = Arc::new(AtomicBool::new(false));
        self.state.forget_band();
    }

    fn set_band(&self, band: Option<BandPlayback>) {
        if let Ok(mut slot) = self.slots.band.lock() {
            *slot = band;
        }
    }

    fn set_tone(&self, tone: Option<Arc<TonePlayback>>) {
        if let Ok(mut slot) = self.slots.tone.lock() {
            *slot = tone;
        }
    }

    fn ensure_mixer(&mut self) {
        if self.stream.is_some() {
            return;
        }
        match start_mixer(Arc::clone(&self.live), Arc::clone(&self.slots)) {
            Ok(stream) => self.stream = Some(stream),
            Err(err) => eprintln!("audio mixer: {err}"),
        }
    }
}

impl MorseBackend for MorsePlayer {
    fn apply_band(&mut self, settings: &TrainingSettings) -> Result<(), String> {
        self.qsb.store(settings);
        if !self.state.band_needs_rebuild(settings) {
            return Ok(());
        }
        self.stop_band();
        if !BandMixer::needs_background(settings) {
            self.set_band(None);
            self.state.note_band(settings);
            self.ensure_mixer();
            return Ok(());
        }
        let mixer = BandMixer::new(self.sample_rate, settings, u64::from(self.sample_rate));
        self.set_band(Some(BandPlayback::new(
            mixer,
            Arc::clone(&self.band_stop),
            self.sample_rate,
            Arc::clone(&self.agc),
        )));
        self.state.note_band(settings);
        self.ensure_mixer();
        Ok(())
    }

    fn start_transmission(
        &mut self,
        transmission: &Transmission,
        settings: &TrainingSettings,
    ) -> Result<PlaybackWait, String> {
        self.apply_band(settings)?;
        self.live.set_on(false);
        let planned = plan_transmission(transmission, settings);
        let plan = planned.wanted.clone();
        self.set_tone(None);
        let armed = self.state.arm();
        let samples = render_send(&planned, settings, self.sample_rate);
        let playback = Arc::new(TonePlayback::new(
            samples,
            self.sample_rate,
            self.opened_at.elapsed().as_secs_f64(),
            Arc::clone(&self.qsb),
            Arc::clone(armed.stop_flag()),
            Arc::clone(&self.agc),
        ));
        let finished = playback.finished_flag();
        self.set_tone(Some(Arc::clone(&playback)));
        self.ensure_mixer();
        if self.stream.is_none() {
            self.state.note_start_failed();
            return Err("Audio mixer failed to open".to_string());
        }
        Ok(PlaybackWait::new(
            plan.duration_sec,
            plan.resolved_char_wpm,
            plan.resolved_effective_wpm,
            std::rc::Rc::new(ToneSignal::new(armed, finished)),
        ))
    }

    fn stop(&mut self) {
        self.state.stop();
    }

    fn shutdown(&mut self) {
        self.stop();
        self.stop_band();
        self.live.set_on(false);
        self.set_band(None);
        self.set_tone(None);
    }

    fn set_live_tone(&mut self, on: bool, frequency_hz: f64, gain: f64) {
        self.live.set(on, frequency_hz, gain);
        self.ensure_mixer();
    }
}

/// Build an output stream that fills `T` samples from an `f32` source.
fn build_stream<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    mut source: F,
) -> Result<cpal::Stream, String>
where
    T: Sample + SizedSample + FromSample<f32>,
    F: FnMut(&mut [f32], usize) + Send + 'static,
{
    let channels = channels.max(1);
    // One scratch buffer, grown once and reused: an audio callback must not
    // allocate. 16k frames covers a default host period with headroom.
    let mut scratch: Vec<f32> = vec![0.0; 16_384];
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                if scratch.len() < output.len() {
                    scratch.resize(output.len(), 0.0);
                }
                let n = output.len();
                scratch[..n].fill(0.0);
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    source(&mut scratch[..n], channels);
                }));
                for (slot, value) in output.iter_mut().zip(&scratch[..n]) {
                    let sample = if value.is_finite() {
                        value.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    };
                    *slot = T::from_sample(sample);
                }
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Audio stream: {e}"))
}

/// Build a stream in whatever sample format the device wants.
fn build_for_format<F>(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    stream_config: &cpal::StreamConfig,
    channels: usize,
    source: F,
) -> Result<cpal::Stream, String>
where
    F: FnMut(&mut [f32], usize) + Send + 'static,
{
    let format = supported.sample_format();
    match format {
        SampleFormat::F32 => build_stream::<f32, F>(device, stream_config, channels, source),
        SampleFormat::F64 => build_stream::<f64, F>(device, stream_config, channels, source),
        SampleFormat::I16 => build_stream::<i16, F>(device, stream_config, channels, source),
        SampleFormat::I32 => build_stream::<i32, F>(device, stream_config, channels, source),
        SampleFormat::U16 => build_stream::<u16, F>(device, stream_config, channels, source),
        SampleFormat::U32 => build_stream::<u32, F>(device, stream_config, channels, source),
        SampleFormat::I8 => build_stream::<i8, F>(device, stream_config, channels, source),
        SampleFormat::U8 => build_stream::<u8, F>(device, stream_config, channels, source),
        other => Err(format!("Unsupported sample format: {other}")),
    }
}

/// ~5–20 ms first. Default Pulse periods are often a third of a second, which
/// is a dah of lag and a buffer that keeps sounding after the paddle is up.
fn mixer_configs(supported: &cpal::SupportedStreamConfig) -> Vec<cpal::StreamConfig> {
    let fallback: cpal::StreamConfig = supported.clone().into();
    let mut configs = Vec::new();
    for frames in [256u32, 512, 1024] {
        let mut config = fallback.clone();
        config.buffer_size = BufferSize::Fixed(frames);
        configs.push(config);
    }
    configs.push(fallback);
    configs
}

fn default_output() -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| "No audio output device found".to_string())?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("Audio config: {e}"))?;
    Ok((device, config))
}

fn render_send(
    planned: &cw_core::PlannedTransmission,
    settings: &TrainingSettings,
    sample_rate: u32,
) -> Vec<f32> {
    let mut samples = render_plan(&planned.wanted, sample_rate);
    for other in &planned.others {
        mix_plan_into(&mut samples, other, sample_rate);
    }
    ReceiverFilter::from_settings(sample_rate, settings).apply(&mut samples);
    samples
}

/// Gate and pitch for the paddle sidetone. The mixer stream stays up; only
/// these atomics change per squeeze, so the lag is one audio buffer.
struct LiveSidetone {
    on: AtomicBool,
    hz: AtomicU32,
    gain_bits: AtomicU32,
}

impl LiveSidetone {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            on: AtomicBool::new(false),
            hz: AtomicU32::new(500),
            gain_bits: AtomicU32::new(0.2f32.to_bits()),
        })
    }

    fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    fn set(&self, on: bool, frequency_hz: f64, gain: f64) {
        let hz = frequency_hz.round().clamp(100.0, 2_000.0) as u32;
        let gain = gain.clamp(0.0, 1.0) as f32;
        self.hz.store(hz, Ordering::Relaxed);
        self.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
        self.set_on(on);
    }
}

fn start_mixer(live: Arc<LiveSidetone>, slots: Arc<MixerSlots>) -> Result<cpal::Stream, String> {
    let (device, supported) = default_output()?;
    let channels = supported.channels() as usize;
    let mut last_err = None;
    for config in mixer_configs(&supported) {
        match start_mixer_on(
            &device,
            &supported,
            &config,
            channels,
            Arc::clone(&live),
            Arc::clone(&slots),
        ) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| "Audio mixer: no stream config worked".to_string()))
}

fn start_mixer_on(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    stream_config: &cpal::StreamConfig,
    channels: usize,
    live: Arc<LiveSidetone>,
    slots: Arc<MixerSlots>,
) -> Result<cpal::Stream, String> {
    let sample_rate = supported.sample_rate().0 as f32;
    let mut phase = 0.0f32;
    let mut env = 0.0f32;
    let coeff = 1.0 - (-1.0f32 / (sample_rate.max(1.0) * 0.0005)).exp();
    let two_pi = std::f32::consts::TAU;
    let stream = build_for_format(
        device,
        supported,
        stream_config,
        channels,
        move |out, channels| {
            if let Ok(mut band) = slots.band.try_lock() {
                if let Some(playback) = band.as_mut() {
                    playback.fill(out, channels);
                }
            }
            let tone = slots.tone.try_lock().ok().and_then(|slot| slot.clone());
            if let Some(playback) = tone {
                playback.mix_into(out, channels);
            }
            let hz = live.hz.load(Ordering::Relaxed) as f32;
            let gain = f32::from_bits(live.gain_bits.load(Ordering::Relaxed));
            let incr = two_pi * hz / sample_rate.max(1.0);
            for frame in out.chunks_mut(channels.max(1)) {
                // Read the gate every frame. A Default Pulse callback can be
                // hundreds of milliseconds: one sample of "on" would otherwise
                // fill the whole thing.
                let want = if live.on.load(Ordering::Relaxed) {
                    1.0
                } else {
                    0.0
                };
                env += (want - env) * coeff;
                if env < 1e-5 && want == 0.0 {
                    env = 0.0;
                }
                let sample = phase.sin() * env * gain;
                phase += incr;
                if phase >= two_pi {
                    phase -= two_pi;
                }
                if !sample.is_finite() || sample == 0.0 {
                    continue;
                }
                for slot in frame {
                    *slot = (*slot + sample).clamp(-1.0, 1.0);
                }
            }
        },
    )?;
    stream.play().map_err(|e| format!("Audio play: {e}"))?;
    Ok(stream)
}

#[cfg(test)]
mod device_tests {
    use super::*;

    /// Opens the machine's default output. On a runner with no sound card, an
    /// ALSA null device is enough; where there is nothing at all the test says
    /// so and stops rather than failing.
    fn player() -> Option<MorsePlayer> {
        MorsePlayer::new().ok()
    }

    fn settings() -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 40.0;
        settings.playback.char_wpm_max = 40.0;
        // Quietly: on a developer's machine this opens the real output.
        settings.band.link_volume = true;
        settings.band.volume_min = 0.1;
        settings.band.qrn_enabled = true;
        settings.band.qrn_level = 0.3;
        settings.band.receiver_enabled = true;
        settings.band.receiver_level = 0.2;
        settings
    }

    #[test]
    fn a_send_runs_through_the_real_output() {
        let Some(mut player) = player() else {
            eprintln!("no audio device on this machine; skipping");
            return;
        };
        let voice = cw_core::resolve_station(&settings(), &mut cw_core::FastrandRng(7));
        let wait = player
            .start_transmission(&Transmission::alone("K", voice), &settings())
            .expect("the device should take a send");
        assert!(wait.duration_sec > 0.0);
        assert!(wait.char_wpm >= 40.0);

        // A second send retires the first.
        let second = player
            .start_transmission(&Transmission::alone("M", voice), &settings())
            .expect("a second send");
        assert!(second.duration_sec > 0.0);

        player.stop();
        player.shutdown();
    }

    #[test]
    fn the_background_is_only_rebuilt_when_it_changes() {
        let Some(mut player) = player() else {
            return;
        };
        let mut settings = settings();
        player.apply_band(&settings).expect("band");
        player.apply_band(&settings).expect("band again");
        settings.band.qrn_level = 0.9;
        player.apply_band(&settings).expect("changed band");
        // A silent band tears the stream down without complaining.
        settings.band.qrn_enabled = false;
        settings.band.receiver_enabled = false;
        player.apply_band(&settings).expect("silent band");
        player.shutdown();
    }

    #[test]
    fn the_paddle_sidetone_opens_a_stream_before_the_first_squeeze() {
        let Some(mut player) = player() else {
            return;
        };
        player.set_live_tone(false, 600.0, 0.2);
        player.set_live_tone(true, 600.0, 0.2);
        player.set_live_tone(false, 600.0, 0.2);
        player.shutdown();
    }
}
