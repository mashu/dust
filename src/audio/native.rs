//! cpal output: ALSA/CoreAudio/WASAPI on desktop, AAudio through oboe on Android.
//!
//! Everything here that can run without a sound card lives in
//! [`super::mixer`] and [`state`]; what is left is the device glue.

mod state;

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample};
use cw_core::band::{BandChange, FilterDesign, Qsb, Receiver};
use cw_core::{PlannedTransmission, TrainingSettings, Transmission};

use super::ToneRequest;
use super::mixer::{
    BandVoice, Meter, Mixer, Output, SendVoice, SidetoneControl, Swept, fade_origin_sec,
};
use super::shape::Gains;
use super::{MeterFrame, MorseBackend, PlaybackSignal, SendTiming};
use state::{PlayerState, ToneSignal, moved_off_default, went_silent};

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
    /// When the player opened, and where on the band's fading its clock
    /// started ([`fade_origin_sec`]). Fading is read off this clock so it
    /// keeps running between groups instead of restarting with every send.
    opened_at: Instant,
    fade_origin_sec: f64,
    /// When the device last refused to open.
    refused_at: Option<Instant>,
    /// When the system's default output was last asked after.
    default_asked: Option<Instant>,
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
///
/// Some breaks say nothing: an iOS interruption, a call or Siri, stops the
/// stream without a word. So the callback counts its calls, and a stream
/// found not to have been called for a while is taken for broken too.
struct OpenStream {
    _stream: cpal::Stream,
    /// The device's name, where it has one to read.
    device: Option<String>,
    failed: Arc<AtomicBool>,
    beat: Arc<AtomicU32>,
    /// The count last seen, and when it was seen to move.
    seen: (u32, Instant),
}

/// The audio buffer iOS is asked for: as short as the desktop's.
#[cfg(target_os = "ios")]
const IO_BUFFER_SEC: f64 = 0.005;

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

    // Safety: every call is a plain message to the process-wide shared
    // session, and every failure is reported through the returned NSError
    // rather than a trap. Nothing here can leave the session half-configured.
    unsafe {
        let session = AVAudioSession::sharedInstance();
        if let Some(playback) = AVAudioSessionCategoryPlayback {
            let _ = session.setCategory_withOptions_error(
                playback,
                AVAudioSessionCategoryOptions::MixWithOthers,
            );
        }
        // cpal's iOS host takes no buffer size of its own, so a stream runs
        // on the session's, about 23 ms unless asked: the paddle's sidetone
        // that much and a buffer late. Asked for about 256 frames; iOS
        // rounds it to what the hardware allows, and may give more while
        // another app's audio is mixed in.
        let _ = session.setPreferredIOBufferDuration_error(IO_BUFFER_SEC);
        let _ = session.setActive_error(true);
    }
}

/// Before Android 9, the volume keys turn the ringer down while no stream
/// plays, and this player's stream is closed once quiet
/// ([`MorseBackend::sleep`]): they are given to the media volume, which Dust
/// plays at, for as long as its activity is in front. On the activity's own
/// thread, as its window is.
#[cfg(all(target_os = "android", not(test)))]
fn claim_volume_keys() {
    use dioxus::mobile::wry::prelude::{dispatch, jni};

    /// `AudioManager.STREAM_MUSIC`.
    const STREAM_MUSIC: i32 = 3;
    dispatch(|env, activity, _webview| {
        let set = env.call_method(
            activity,
            "setVolumeControlStream",
            "(I)V",
            &[jni::objects::JValue::Int(STREAM_MUSIC)],
        );
        // A Java exception left pending would fail every JNI call after
        // it, the webview's own among them.
        if set.is_err() && env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    });
}

impl MorsePlayer {
    /// A player on the default output, with its stream opened now so the
    /// first squeeze of the paddle is not a stream start. A device that
    /// refuses to open gets another chance on first use; no device at all is
    /// an error.
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "ios")]
        claim_audio_session();
        #[cfg(all(target_os = "android", not(test)))]
        claim_volume_keys();
        let (device, supported) = default_output()?;
        let mixer = Mixer::new(supported.sample_rate().0);
        let meter = mixer.meter();
        let mut player = Self {
            state: PlayerState::new(),
            mixer: Arc::new(Mutex::new(mixer)),
            sidetone: Arc::new(SidetoneControl::new()),
            stream: None,
            opened_at: Instant::now(),
            fade_origin_sec: fade_origin_sec(),
            refused_at: None,
            default_asked: None,
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

    /// Make sure the output stream is up: opened again after it slept, or
    /// after the device broke.
    fn ensure_output(&mut self) -> Result<(), String> {
        if self.stream.as_mut().is_some_and(|open| {
            let beat = open.beat.load(Ordering::Relaxed);
            if went_silent(&mut open.seen, beat, Instant::now()) {
                // Broken without a word: a send waiting on it is told so.
                open.failed.store(true, Ordering::Release);
            }
            open.failed.load(Ordering::Acquire)
        }) {
            // The old stream goes before a new one opens on the same device.
            // Whatever was playing on it went with it: a send waiting on it
            // has been told, and the band is rebuilt on the next apply. On
            // iOS the session an interruption took is claimed again first.
            self.stream = None;
            #[cfg(target_os = "ios")]
            claim_audio_session();
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

    /// Let the stream go if the system's default output has changed since it
    /// opened, for the next use to open on the new one: headphones plugged
    /// in or a headset connected mid-session, which a stream on Windows or
    /// macOS does not follow of itself. Asked at most once a
    /// [`REOPEN_AFTER`], and never under a send, so none is cut off; the band
    /// and whatever else the mixer holds carry on in the new stream.
    fn follow_default(&mut self) {
        let due = self
            .default_asked
            .is_none_or(|at| at.elapsed() >= REOPEN_AFTER);
        if !due || self.stream.is_none() || self.mixer().is_sending() {
            return;
        }
        self.default_asked = Some(Instant::now());
        let default = cpal::default_host()
            .default_output_device()
            .and_then(|device| device.name().ok());
        let opened = self.stream.as_ref().and_then(|open| open.device.as_deref());
        if moved_off_default(opened, default.as_deref()) {
            self.stream = None;
        }
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
        self.follow_default();
        // A stream that will not open is reported by the next send.
        self.ensure_output_unless_refused();
        let gains = Gains::of(settings);
        let rate = self.mixer().sample_rate();
        match self.state.band_change(settings) {
            BandChange::None => {
                if gains != self.gains {
                    self.mixer().set_gains(gains);
                    self.gains = gains;
                }
                return;
            }
            BandChange::Retune => {
                // Built here, where it may allocate; the band on the air
                // carries on through it.
                let receiver = Receiver::new(rate, settings);
                let mut retuned = false;
                self.with_mixer(|mixer, swept| {
                    retuned = mixer.retune_band(receiver, gains, swept);
                });
                if retuned {
                    self.gains = gains;
                    self.state.note_band(settings);
                    return;
                }
            }
            BandChange::Rebuild => {}
        }
        // A fresh band every time, as the browser's is: with stations on it,
        // a fixed seed would replay the same callsigns at the same moments
        // in every session.
        let band = BandVoice::for_settings(settings, rate, crate::time::seed_rng());
        self.with_mixer(|mixer, swept| mixer.set_band_at(band, gains, swept));
        self.gains = gains;
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
            self.fade_origin_sec + self.opened_at.elapsed().as_secs_f64() + wait_sec,
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

    /// Closed, not paused: a host that cannot pause would play silence on,
    /// and cpal does not say which cannot. Closed, it lets the device and
    /// the sound server rest. The next band, send or sidetone opens it again
    /// ([`MorsePlayer::ensure_output`]), as a new player does.
    fn sleep(&mut self) {
        if self.stream.take().is_none() {
            return;
        }
        // What has faded out is let go of now: with the stream closed, no
        // send or band comes in to make way for it.
        self.with_mixer(|mixer, swept| mixer.sweep(swept));
    }

    fn set_live_tone(&mut self, tone: ToneRequest) {
        self.sidetone.set(tone);
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
    beat: Arc<AtomicU32>,
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
                beat.fetch_add(1, Ordering::Relaxed);
                let frames = data.len().div_ceil(channels);
                if scratch.len() < frames {
                    // Only a host handing over more than it ever has before.
                    scratch.resize(frames, 0.0);
                }
                let mono = &mut scratch[..frames];
                output.fill(mono, latency_frames(info, sample_rate));
                interleave(data, channels, mono);
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

/// The mono receiver into a device's frames of `channels`: on the front two,
/// left and right, as a rig's audio and the browser's are played, or the one
/// a mono device has; the rest, a surround system's centre, rears and LFE or
/// an interface's other outputs, silent.
fn interleave<T: SizedSample + FromSample<f32>>(data: &mut [T], channels: usize, mono: &[f32]) {
    for (frame, sample) in data.chunks_mut(channels.max(1)).zip(mono) {
        let (front, rest) = frame.split_at_mut(frame.len().min(2));
        front.fill(T::from_sample(*sample));
        rest.fill(T::EQUILIBRIUM);
    }
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
        let beat = Arc::new(AtomicU32::new(0));
        let (failing, beating) = (Arc::clone(&failed), Arc::clone(&beat));
        let built = match supported.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(device, &config, output, failing, beating),
            SampleFormat::F64 => build_stream::<f64>(device, &config, output, failing, beating),
            SampleFormat::I16 => build_stream::<i16>(device, &config, output, failing, beating),
            SampleFormat::I32 => build_stream::<i32>(device, &config, output, failing, beating),
            SampleFormat::U16 => build_stream::<u16>(device, &config, output, failing, beating),
            SampleFormat::U32 => build_stream::<u32>(device, &config, output, failing, beating),
            SampleFormat::I8 => build_stream::<i8>(device, &config, output, failing, beating),
            SampleFormat::U8 => build_stream::<u8>(device, &config, output, failing, beating),
            other => Err(format!("Unsupported sample format: {other}")),
        };
        match built.and_then(|stream| {
            stream.play().map_err(|e| format!("Audio play: {e}"))?;
            Ok(stream)
        }) {
            Ok(stream) => {
                return Ok(OpenStream {
                    _stream: stream,
                    device: device.name().ok(),
                    failed,
                    beat,
                    seen: (0, Instant::now()),
                });
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| "Audio mixer: no stream config worked".to_string()))
}

#[cfg(test)]
mod interleave_tests {
    use super::interleave;

    /// The receiver goes to the front pair, or a mono device's one channel,
    /// and nowhere else: every other channel is silence, at its format's
    /// silence.
    #[test]
    fn the_receiver_plays_on_the_front_pair_alone() {
        let mono = [0.5f32, -0.25];
        let mut stereo = [9.0f32; 4];
        interleave(&mut stereo, 2, &mono);
        assert_eq!(stereo, [0.5, 0.5, -0.25, -0.25]);
        let mut single = [9.0f32; 2];
        interleave(&mut single, 1, &mono);
        assert_eq!(single, [0.5, -0.25]);
        let mut surround = [9.0f32; 12];
        interleave(&mut surround, 6, &mono);
        assert_eq!(
            surround,
            [
                0.5, 0.5, 0.0, 0.0, 0.0, 0.0, -0.25, -0.25, 0.0, 0.0, 0.0, 0.0
            ]
        );
        let mut unsigned = [1u16; 12];
        interleave(&mut unsigned, 6, &mono);
        assert!(unsigned[2..6].iter().all(|s| *s == 32_768), "{unsigned:?}");
        assert!(unsigned[0] > 32_768 && unsigned[0] == unsigned[1]);
        assert!(unsigned[6] < 32_768 && unsigned[6] == unsigned[7]);
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;

    /// Opens the machine's default output. On a runner with no sound card, an
    /// ALSA null device is enough, and CI gives it one, so there a player
    /// that will not open fails the test rather than passing it untried;
    /// on a machine with nothing at all the test says so and stops.
    fn player() -> Option<MorsePlayer> {
        match MorsePlayer::new() {
            Ok(player) => Some(player),
            Err(error) if std::env::var_os("CI").is_some() => {
                panic!("CI gives ALSA a null output, yet the player would not open: {error}")
            }
            Err(error) => {
                eprintln!("no audio output on this machine ({error}); skipping");
                None
            }
        }
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
        assert!(player.stream.is_some(), "the band opened the output");
        assert_eq!(player.state.band_change(&settings), BandChange::None);
        player.apply_band(&settings);
        assert_eq!(player.state.band_change(&settings), BandChange::None);
        settings.band.qrn_level = 0.9;
        assert_eq!(player.state.band_change(&settings), BandChange::Rebuild);
        player.apply_band(&settings);
        assert_eq!(player.state.band_change(&settings), BandChange::None);
        // A silent band takes the band out without complaining.
        settings.band.qrn_enabled = false;
        settings.band.noise_enabled = false;
        player.apply_band(&settings);
        assert_eq!(player.state.band_change(&settings), BandChange::None);
        player.shutdown();
    }

    /// Asleep, the stream is closed and calls back no more, so nothing is
    /// metered, and the next sidetone opens it to play on.
    #[test]
    fn a_sleeping_stream_calls_back_no_more_until_it_is_woken() {
        let Some(mut player) = player() else {
            return;
        };
        let metered = |player: &mut MorsePlayer, ms: u64| {
            let mut frames = Vec::new();
            player.take_meter(&mut frames);
            std::thread::sleep(Duration::from_millis(ms));
            frames.clear();
            player.take_meter(&mut frames);
            frames.len()
        };
        assert!(metered(&mut player, 200) > 0, "the stream plays");
        player.shutdown();
        player.sleep();
        assert!(player.stream.is_none());
        assert_eq!(metered(&mut player, 300), 0, "metered asleep");
        player.set_live_tone(ToneRequest::new(false, 600.0, 0.2));
        assert!(player.stream.is_some(), "the sidetone opened it");
        assert!(metered(&mut player, 300) > 0, "woken, it plays");
        player.shutdown();
    }

    #[test]
    fn the_paddle_sidetone_opens_a_stream_before_the_first_squeeze() {
        let Some(mut player) = player() else {
            return;
        };
        assert!(player.stream.is_some(), "opened with the player");
        player.set_live_tone(ToneRequest::new(false, 600.0, 0.2));
        player.set_live_tone(ToneRequest::new(true, 600.0, 0.2));
        player.set_live_tone(ToneRequest::new(false, 600.0, 0.2));
        assert!(player.stream.is_some(), "and kept through the squeeze");
        player.shutdown();
    }
}
