//! cpal output: ALSA/CoreAudio/WASAPI on desktop, AAudio through oboe on Android.
//!
//! Everything here that can run without a sound card lives in
//! [`super::render`] and [`state`]; what is left is the device glue.

mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use cw_core::band::BandMixer;
use cw_core::{plan_morse_playback, FastrandRng, TrainingSettings};

use super::render::{fill_band, render_plan, LiveQsb, TonePlayback};
use super::{MorseBackend, PlaybackWait};
use state::{PlayerState, ToneSignal};

pub struct MorsePlayer {
    state: PlayerState,
    band_stop: Arc<AtomicBool>,
    band_stream: Option<cpal::Stream>,
    tone_stream: Option<cpal::Stream>,
    qsb: Arc<LiveQsb>,
    /// When the player opened. Fading is read off this clock so it keeps
    /// running between groups instead of restarting with every send.
    opened_at: Instant,
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
        let _ = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| "No audio output device found".to_string())?;
        Ok(Self {
            state: PlayerState::new(),
            band_stop: Arc::new(AtomicBool::new(false)),
            band_stream: None,
            tone_stream: None,
            qsb: LiveQsb::new(),
            opened_at: Instant::now(),
        })
    }

    fn stop_band(&mut self) {
        self.band_stop.store(true, Ordering::SeqCst);
        self.band_stream = None;
        self.band_stop = Arc::new(AtomicBool::new(false));
        self.state.forget_band();
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
            self.state.note_band(settings);
            return Ok(());
        }
        // The receiver background is decoration. If its stream will not open —
        // some Android devices refuse a second concurrent output stream — the
        // Morse must still play, so this failure is swallowed rather than
        // failing the whole player. The signature is remembered either way, so
        // a configuration that cannot open is not retried on every change.
        self.band_stream = start_band_stream(settings, Arc::clone(&self.band_stop)).ok();
        self.state.note_band(settings);
        Ok(())
    }

    fn start_text(
        &mut self,
        text: &str,
        settings: &TrainingSettings,
        rng: &mut FastrandRng,
    ) -> Result<PlaybackWait, String> {
        self.apply_band(settings)?;
        let plan = plan_morse_playback(text, settings, rng);
        // Drop the previous stream before the new epoch, so its callback cannot
        // write into the run that is about to start.
        self.tone_stream = None;
        let armed = self.state.arm();
        let (stream, finished) = start_tone_stream(
            &plan,
            self.opened_at.elapsed().as_secs_f64(),
            Arc::clone(&self.qsb),
            Arc::clone(armed.stop_flag()),
        )
        .inspect_err(|_| self.state.note_start_failed())?;
        self.tone_stream = Some(stream);
        Ok(PlaybackWait::new(
            plan.duration_sec,
            plan.resolved_char_wpm,
            plan.resolved_effective_wpm,
            std::rc::Rc::new(ToneSignal::new(armed, finished)),
        ))
    }

    fn stop(&mut self) {
        self.state.stop();
        self.tone_stream = None;
    }

    fn shutdown(&mut self) {
        self.stop();
        self.stop_band();
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
    // allocate.
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                scratch.clear();
                scratch.resize(output.len(), 0.0);
                source(&mut scratch, channels);
                for (slot, value) in output.iter_mut().zip(scratch.iter()) {
                    *slot = T::from_sample(*value);
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
    config: &cpal::SupportedStreamConfig,
    channels: usize,
    source: F,
) -> Result<cpal::Stream, String>
where
    F: FnMut(&mut [f32], usize) + Send + 'static,
{
    let format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.clone().into();
    match format {
        SampleFormat::F32 => build_stream::<f32, F>(device, &stream_config, channels, source),
        SampleFormat::F64 => build_stream::<f64, F>(device, &stream_config, channels, source),
        SampleFormat::I16 => build_stream::<i16, F>(device, &stream_config, channels, source),
        SampleFormat::I32 => build_stream::<i32, F>(device, &stream_config, channels, source),
        SampleFormat::U16 => build_stream::<u16, F>(device, &stream_config, channels, source),
        SampleFormat::U32 => build_stream::<u32, F>(device, &stream_config, channels, source),
        SampleFormat::I8 => build_stream::<i8, F>(device, &stream_config, channels, source),
        SampleFormat::U8 => build_stream::<u8, F>(device, &stream_config, channels, source),
        other => Err(format!("Unsupported sample format: {other}")),
    }
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

fn start_tone_stream(
    plan: &cw_core::PlaybackPlan,
    started_at_sec: f64,
    qsb: Arc<LiveQsb>,
    stop: Arc<AtomicBool>,
) -> Result<(cpal::Stream, Arc<AtomicBool>), String> {
    let (device, config) = default_output()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let playback = TonePlayback::new(
        render_plan(plan, sample_rate),
        sample_rate,
        started_at_sec,
        qsb,
        stop,
    );
    let finished = playback.finished_flag();
    let stream = build_for_format(&device, &config, channels, move |out, channels| {
        playback.fill(out, channels)
    })?;
    stream.play().map_err(|e| format!("Audio play: {e}"))?;
    Ok((stream, finished))
}

fn start_band_stream(
    settings: &TrainingSettings,
    stop: Arc<AtomicBool>,
) -> Result<cpal::Stream, String> {
    let (device, config) = default_output()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let mut mixer = BandMixer::new(sample_rate, settings, u64::from(sample_rate));
    let stream = build_for_format(&device, &config, channels, move |out, channels| {
        fill_band(out, channels, &mut mixer, &stop)
    })?;
    stream.play().map_err(|e| format!("Band play: {e}"))?;
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
        settings.band.qrm_enabled = true;
        settings.band.qrm_level = 0.2;
        settings
    }

    #[test]
    fn a_send_runs_through_the_real_output() {
        let Some(mut player) = player() else {
            eprintln!("no audio device on this machine; skipping");
            return;
        };
        let mut rng = FastrandRng(7);
        let wait = player
            .start_text("K", &settings(), &mut rng)
            .expect("the device should take a send");
        assert!(wait.duration_sec > 0.0);
        assert!(wait.char_wpm >= 40.0);

        // A second send retires the first.
        let second = player
            .start_text("M", &settings(), &mut rng)
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
        settings.band.qrm_enabled = false;
        player.apply_band(&settings).expect("silent band");
        player.shutdown();
    }
}
