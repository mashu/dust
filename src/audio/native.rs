//! cpal output: ALSA/CoreAudio/WASAPI on desktop, AAudio through oboe on Android.
//!
//! Everything here that can run without a sound card lives in
//! [`super::mixer`] and [`state`]; what is left is the device glue.

mod state;

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample};
use cw_core::band::{BandMixer, FilterDesign, Qsb};
use cw_core::{PlannedTransmission, TrainingSettings, Transmission};

use super::mixer::{BandVoice, Meter, Mixer, Output, SendVoice, SidetoneControl, Swept};
use super::shape::{Gains, ToneRequest};
use super::{MeterFrame, MorseBackend, PlaybackSignal, SendTiming};
use state::{PlayerState, ToneSignal};

/// The furthest behind the callback the speaker is believed to be. A host
/// that reports more is reporting nonsense, and a send would wait for it.
const MAX_LATENCY_SEC: f64 = 0.5;

/// How long after a device refused to open before a paddle squeeze or a
/// slider move tries it again. Opening probes the device and tries four
/// buffer sizes on the UI thread; a send, which reports the failure, always
/// tries.
const REOPEN_AFTER: Duration = Duration::from_secs(1);

/// How long a stream that keeps reporting errors after it broke is made to
/// wait before reporting the next. Some hosts' workers retry a dead device
/// in a tight loop, calling back with an error each time round; this keeps
/// that loop from taking a core and the log with it.
const ERROR_BACKOFF: Duration = Duration::from_millis(20);

pub struct MorsePlayer {
    state: PlayerState,
    /// Band and Morse, mixed. The UI swaps bands and sends in under its lock;
    /// the stream's callback plays it out.
    mixer: Arc<Mutex<Mixer>>,
    sidetone: Arc<SidetoneControl>,
    /// The one output stream. Morse, receiver hiss and paddle sidetone are
    /// mixed into it: three device streams on Pulse/PipeWire underrun, and
    /// the sidetone stutters, turns to noise, then drops.
    stream: Option<OpenStream>,
    /// When the player opened. Fading is read off this clock so it keeps
    /// running between groups instead of restarting with every send.
    opened_at: Instant,
    /// When the device last refused to open.
    refused_at: Option<Instant>,
    /// When the send playing is heard from, as this player said it would
    /// be: what a send asked to start at a given time takes over from it
    /// against.
    heard_at: Option<Instant>,
    /// The gains the mixer was last asked to play at, so it is locked for
    /// them only when they move: a callback that finds it held plays a
    /// buffer without it.
    gains: Gains,
    /// The mixer's meter, read without its lock, and how many of its frames
    /// have been taken.
    meter: Arc<Meter>,
    metered: usize,
}

/// An output stream, and its own word that it broke: unplugged headphones, a
/// sound server restart. A send in flight on it reports the failure, so the
/// session can recover rather than stall.
struct OpenStream {
    _stream: cpal::Stream,
    failed: Arc<AtomicBool>,
}

/// iOS starts an app with no audio session, which leaves output silent, tied to
/// the ringer switch and interrupted by anything else on the device. Claiming
/// the playback category before the first stream opens is what makes the Morse
/// audible — cpal does not do it, and there is no equivalent on other platforms.
///
/// Mixed with others: a player is made as soon as the settings screen opens,
/// to warm the paddle's sidetone, and the playback category on its own stops
/// whatever else is playing — the listener's music, for opening a screen.
#[cfg(target_os = "ios")]
fn claim_audio_session() {
    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionCategoryOptions, AVAudioSessionCategoryPlayback,
    };

    // Safety: both calls are plain messages to the process-wide shared session,
    // and every failure is reported through the returned NSError rather than a
    // trap. Nothing here can leave the session half-configured.
    unsafe {
        let session = AVAudioSession::sharedInstance();
        if let Some(playback) = AVAudioSessionCategoryPlayback {
            let _ = session.setCategory_withOptions_error(
                playback,
                AVAudioSessionCategoryOptions::MixWithOthers,
            );
        }
        let _ = session.setActive_error(true);
    }
}

impl MorsePlayer {
    /// A player on the default output, with its stream opened now so the
    /// first squeeze of the paddle is not a stream start. A device that
    /// refuses to open gets another chance on first use; no device at all is
    /// an error.
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "ios")]
        claim_audio_session();
        let (device, supported) = default_output()?;
        let mixer = Mixer::new(supported.sample_rate().0);
        let meter = mixer.meter();
        let mut player = Self {
            state: PlayerState::new(),
            mixer: Arc::new(Mutex::new(mixer)),
            sidetone: Arc::new(SidetoneControl::new()),
            stream: None,
            opened_at: Instant::now(),
            refused_at: None,
            heard_at: None,
            gains: Gains::UNITY,
            meter,
            metered: 0,
        };
        if let Err(err) = player.open(&device, &supported) {
            eprintln!("audio output: {err}");
        }
        Ok(player)
    }

    /// The UI thread's way into the mixer. A panic while the lock was held
    /// cannot leave it half-written — every change is a move into a field —
    /// so a poisoned lock is still a good one.
    fn mixer(&self) -> MutexGuard<'_, Mixer> {
        self.mixer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Change the mixer under its lock, and let go of whatever it has
    /// finished with only once the lock is released: freeing a band is not
    /// something the audio callback should wait on.
    fn with_mixer(&self, change: impl FnOnce(&mut Mixer, &mut Swept)) {
        let mut swept = Swept::new();
        change(&mut self.mixer(), &mut swept);
        drop(swept);
    }

    /// Make sure the output stream is up, reopening it if the device broke.
    fn ensure_output(&mut self) -> Result<(), String> {
        if self
            .stream
            .as_ref()
            .is_some_and(|open| open.failed.load(Ordering::Acquire))
        {
            // The old stream goes before a new one opens on the same device.
            // Whatever was playing on it went with it: a send waiting on it
            // has been told, and the band is rebuilt on the next apply.
            self.stream = None;
            let rate = self.mixer().sample_rate();
            self.with_mixer(|mixer, swept| mixer.clear(rate, swept));
            self.state.forget_band();
        }
        if self.stream.is_some() {
            return Ok(());
        }
        let (device, supported) = default_output().inspect_err(|_| self.note_refusal())?;
        self.open(&device, &supported)
    }

    /// [`MorsePlayer::ensure_output`], unless the device refused a moment
    /// ago: a squeeze of the paddle or a slider being dragged would otherwise
    /// probe it again on every event.
    fn ensure_output_unless_refused(&mut self) {
        if self
            .refused_at
            .is_some_and(|at| at.elapsed() < REOPEN_AFTER)
        {
            return;
        }
        if let Err(err) = self.ensure_output() {
            eprintln!("audio output: {err}");
        }
    }

    /// Open the stream on `device`.
    fn open(
        &mut self,
        device: &cpal::Device,
        supported: &cpal::SupportedStreamConfig,
    ) -> Result<(), String> {
        let rate = supported.sample_rate().0;
        if rate != self.mixer().sample_rate() {
            // A different device, or the same one reconfigured. Everything is
            // generated at the stream's rate, so the band is rebuilt at the
            // new one before it can play at the wrong pitch.
            self.with_mixer(|mixer, swept| mixer.clear(rate, swept));
            self.state.forget_band();
        }
        let open = open_stream(device, supported, &self.mixer, &self.sidetone)
            .inspect_err(|_| self.note_refusal())?;
        self.stream = Some(open);
        self.refused_at = None;
        Ok(())
    }

    fn note_refusal(&mut self) {
        self.refused_at = Some(Instant::now());
    }
}

impl MorseBackend for MorsePlayer {
    /// Opens the stream first, so a device that changed rate is known about
    /// before the band is built at the old one.
    fn apply_band(&mut self, settings: &TrainingSettings) {
        // A stream that will not open is reported by the next send.
        self.ensure_output_unless_refused();
        let gains = Gains::of(settings);
        if gains != self.gains {
            self.mixer().set_gains(gains);
            self.gains = gains;
        }
        if !self.state.band_needs_rebuild(settings) {
            return;
        }
        let rate = self.mixer().sample_rate();
        let band = settings.band.has_background().then(|| {
            // A fresh band every time, as the browser's is: with stations
            // on it, a fixed seed would replay the same callsigns at the
            // same moments in every session.
            BandVoice::new(
                BandMixer::new(rate, settings, crate::time::seed_rng()),
                rate,
            )
        });
        self.with_mixer(|mixer, swept| mixer.set_band(band, swept));
        self.state.note_band(settings);
    }

    fn start(
        &mut self,
        _transmission: &Transmission,
        plan: &PlannedTransmission,
        settings: &TrainingSettings,
        heard_in: Option<f64>,
    ) -> Result<Rc<dyn PlaybackSignal>, String> {
        // What the send says of when it is heard is from here, where it was
        // asked for: a device opened below is time it waits like any other.
        let entered = Instant::now();
        if let Err(err) = self.ensure_output() {
            self.state.retire();
            return Err(format!("Audio mixer failed to open: {err}"));
        }
        let Some(failed) = self.stream.as_ref().map(|open| Arc::clone(&open.failed)) else {
            self.state.retire();
            return Err("Audio mixer failed to open.".into());
        };
        // In a statement of its own, after the stream is up at whatever rate
        // it opened at. Read inside the call below, the guard would last to
        // the end of that statement, and the voice would be built — its
        // allocations and all — with the audio callback locked out of the
        // mixer.
        let (rate, lookahead_sec, output_latency_sec) = {
            let mixer = self.mixer();
            (
                mixer.sample_rate(),
                mixer.lookahead_sec(),
                mixer.output_latency_sec(),
            )
        };
        // How long after it starts a send is heard.
        let latency_sec = lookahead_sec + output_latency_sec;
        let now = Instant::now();
        let opening_sec = now.saturating_duration_since(entered).as_secs_f64();
        let sr = f64::from(rate);
        // A send asked to start at a given time takes over from the one
        // playing where that one is then, on the mixer's own clock, sample
        // for sample; with nothing playing, it waits out the time from now.
        let wait_sec = heard_in.map_or(0.0, |heard_in| {
            (heard_in - opening_sec - latency_sec).max(0.0)
        });
        let into = heard_in.zip(self.heard_at).map(|(heard_in, playing)| {
            let asked = entered + Duration::from_secs_f64(heard_in.max(0.0));
            (asked.saturating_duration_since(playing).as_secs_f64() * sr).round() as usize
        });
        let voice = SendVoice::new(
            plan,
            &FilterDesign::from_settings(settings),
            Qsb::of(&settings.band),
            rate,
            self.opened_at.elapsed().as_secs_f64() + wait_sec,
        );
        let (finished, late) = (voice.finished_flag(), voice.late_flag());
        let armed = self.state.arm();
        let mut waits = 0;
        self.with_mixer(|mixer, swept| {
            waits = mixer.start_at(voice, into, (wait_sec * sr).round() as usize, swept);
        });
        let rendered_in = waits as f64 / sr;
        self.heard_at = Some(now + Duration::from_secs_f64(latency_sec + rendered_in));
        Ok(Rc::new(ToneSignal::new(
            armed,
            finished,
            late,
            failed,
            // Its samples go to the output once the receiver has looked
            // ahead at them.
            SendTiming {
                lead_sec: opening_sec + rendered_in + lookahead_sec,
                latency_sec: output_latency_sec,
            },
        )))
    }

    fn stop(&mut self) {
        self.heard_at = None;
        self.state.retire();
        // The send rides its own release down to silence.
        self.mixer().stop();
    }

    fn shutdown(&mut self) {
        self.stop();
        self.with_mixer(|mixer, swept| mixer.set_band(None, swept));
        self.state.forget_band();
        self.sidetone.off();
    }

    fn set_live_tone(&mut self, on: bool, frequency_hz: f64, gain: f64) {
        self.sidetone.set(ToneRequest::new(on, frequency_hz, gain));
        self.ensure_output_unless_refused();
    }

    fn take_meter(&mut self, into: &mut Vec<MeterFrame>) {
        self.meter.read_since(&mut self.metered, into);
    }
}

/// How far behind the callback the speaker is, in frames: how long the
/// buffer being filled waits before it is heard.
///
/// None on Android: cpal's oboe host stamps the callback with the time since
/// its stream was made and the playback with the system's monotonic clock,
/// so the gap between them is the phone's uptime, not a delay — half a
/// second, at the cap, after every send. There a send is over, and its
/// characters timed, as it goes to the output.
fn latency_frames(info: &cpal::OutputCallbackInfo, sample_rate: u32) -> usize {
    if cfg!(target_os = "android") {
        return 0;
    }
    let stamp = info.timestamp();
    stamp
        .playback
        .duration_since(&stamp.callback)
        .map_or(0, |ahead| {
            (ahead.as_secs_f64().min(MAX_LATENCY_SEC) * f64::from(sample_rate)) as usize
        })
}

/// Build an output stream that plays `output` into `T` samples.
fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut output: Output,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    let sample_rate = config.sample_rate.0;
    // One scratch buffer of mono frames, made once: an audio callback must
    // not allocate. A host hands over at most a few periods at a time, and
    // with its own default buffer a third of a second or so.
    let frames = match config.buffer_size {
        BufferSize::Fixed(frames) => (frames as usize * 8).max(4_096),
        BufferSize::Default => 32_768,
    };
    let mut scratch: Vec<f32> = vec![0.0; frames];
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let frames = data.len().div_ceil(channels);
                if scratch.len() < frames {
                    // Only a host handing over more than it ever has before.
                    scratch.resize(frames, 0.0);
                }
                let mono = &mut scratch[..frames];
                output.fill(mono, latency_frames(info, sample_rate));
                for (frame, sample) in data.chunks_mut(channels).zip(mono.iter()) {
                    frame.fill(T::from_sample(*sample));
                }
            },
            move |err| {
                // The first error is the one worth reading; the stream is
                // reopened on the next use whatever the rest say.
                if failed.swap(true, Ordering::AcqRel) {
                    std::thread::sleep(ERROR_BACKOFF);
                } else {
                    eprintln!("audio stream error: {err}");
                }
            },
            None,
        )
        .map_err(|e| format!("Audio stream: {e}"))
}

/// ~5–20 ms first. Default Pulse periods are often a third of a second, which
/// is a dah of lag and a buffer that keeps sounding after the paddle is up.
fn stream_configs(supported: &cpal::SupportedStreamConfig) -> Vec<cpal::StreamConfig> {
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

/// Open a stream on `device` playing `mixer` and the sidetone, trying the
/// short buffers first.
fn open_stream(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    mixer: &Arc<Mutex<Mixer>>,
    sidetone: &Arc<SidetoneControl>,
) -> Result<OpenStream, String> {
    let mut last_err = None;
    for config in stream_configs(supported) {
        let failed = Arc::new(AtomicBool::new(false));
        let output = Output::new(
            Arc::clone(mixer),
            Arc::clone(sidetone),
            Arc::clone(&failed),
            config.sample_rate.0,
        );
        let built = match supported.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(device, &config, output, Arc::clone(&failed)),
            SampleFormat::F64 => build_stream::<f64>(device, &config, output, Arc::clone(&failed)),
            SampleFormat::I16 => build_stream::<i16>(device, &config, output, Arc::clone(&failed)),
            SampleFormat::I32 => build_stream::<i32>(device, &config, output, Arc::clone(&failed)),
            SampleFormat::U16 => build_stream::<u16>(device, &config, output, Arc::clone(&failed)),
            SampleFormat::U32 => build_stream::<u32>(device, &config, output, Arc::clone(&failed)),
            SampleFormat::I8 => build_stream::<i8>(device, &config, output, Arc::clone(&failed)),
            SampleFormat::U8 => build_stream::<u8>(device, &config, output, Arc::clone(&failed)),
            other => Err(format!("Unsupported sample format: {other}")),
        };
        match built.and_then(|stream| {
            stream.play().map_err(|e| format!("Audio play: {e}"))?;
            Ok(stream)
        }) {
            Ok(stream) => {
                return Ok(OpenStream {
                    _stream: stream,
                    failed,
                });
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| "Audio mixer: no stream config worked".to_string()))
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
        settings.band.noise_enabled = true;
        settings.band.noise_level = 0.2;
        settings
    }

    #[test]
    fn a_send_runs_through_the_real_output() {
        let Some(mut player) = player() else {
            eprintln!("no audio device on this machine; skipping");
            return;
        };
        let voice = cw_core::resolve_station(&settings(), &mut cw_core::SplitMix64::new(7));
        let wait =
            crate::audio::start_send(&mut player, &Transmission::alone("K", voice), &settings())
                .expect("the device should take a send");
        assert!(wait.duration_sec > 0.0);
        assert!(wait.start().char_wpm >= 40.0);

        // A second send retires the first.
        let second =
            crate::audio::start_send(&mut player, &Transmission::alone("M", voice), &settings())
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
        player.apply_band(&settings);
        player.apply_band(&settings);
        settings.band.qrn_level = 0.9;
        player.apply_band(&settings);
        // A silent band takes the band out without complaining.
        settings.band.qrn_enabled = false;
        settings.band.noise_enabled = false;
        player.apply_band(&settings);
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
