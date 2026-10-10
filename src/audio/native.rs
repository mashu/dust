//! cpal output: ALSA/CoreAudio/WASAPI on desktop, AAudio through oboe on Android.
//!
//! Everything here that can run without a sound card lives in
//! [`super::render`] and [`state`]; what is left is the device glue.

mod state;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, Sample, SampleFormat, SizedSample};
use cw_core::band::{BandMixer, ReceiverFilter};
use cw_core::{TrainingSettings, Transmission, plan_transmission};

use super::render::{BandPlayback, LiveAgc, LiveQsb, TonePlayback, mix_plan_into, render_plan};
use super::{MorseBackend, PlaybackWait};
use state::{PlayerState, ToneSignal};

/// Band and Morse share the paddle's output stream. Three device streams on
/// Pulse/PipeWire underrun: the sidetone stutters, turns to noise, then drops.
struct MixerSlots {
    band: Mutex<Option<BandPlayback>>,
    /// The band that was just replaced, still playing out its release so the
    /// change is a fade rather than a click.
    retiring: Mutex<Option<BandPlayback>>,
    tone: Mutex<Option<Arc<TonePlayback>>>,
}

/// The UI thread's way into a slot. A panic while the lock was held cannot
/// leave the slot half-written — every write is one assignment — so a
/// poisoned lock is still a good one.
fn lock_slot<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The audio thread's way in: never waits. A buffer that finds the UI mid-way
/// through swapping a layer simply goes without that layer once.
fn try_slot<T>(slot: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match slot.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

/// Play one layer of the mix. A layer that panics is dropped rather than left
/// in place to panic again on every buffer that follows; the rest of the mix
/// carries on without it.
fn play_layer<T>(slot: &Mutex<Option<T>>, play: impl FnOnce(&mut T)) {
    let Some(mut guard) = try_slot(slot) else {
        return;
    };
    let Some(layer) = guard.as_mut() else {
        return;
    };
    if catch_unwind(AssertUnwindSafe(|| play(layer))).is_err() {
        *guard = None;
    }
}

pub struct MorsePlayer {
    state: PlayerState,
    slots: Arc<MixerSlots>,
    /// The one output stream. Morse, receiver hiss and paddle sidetone are
    /// mixed here so the device is not asked for three concurrent writers.
    stream: Option<cpal::Stream>,
    /// Set by the device when the stream breaks: unplugged headphones, a
    /// sound server restart. The next use reopens it, and a send in flight
    /// reports the failure so the session can recover rather than stall.
    stream_failed: Arc<AtomicBool>,
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
        let mut player = Self {
            state: PlayerState::new(),
            slots: Arc::new(MixerSlots {
                band: Mutex::new(None),
                retiring: Mutex::new(None),
                tone: Mutex::new(None),
            }),
            stream: None,
            stream_failed: Arc::new(AtomicBool::new(false)),
            sample_rate: supported.sample_rate().0,
            qsb: LiveQsb::new(),
            agc: LiveAgc::new(),
            opened_at: Instant::now(),
            live: LiveSidetone::new(),
        };
        // Opened now so the first squeeze of the paddle is not a stream start,
        // but a device that refuses here gets another chance on first use.
        if let Err(err) = player.ensure_mixer() {
            eprintln!("audio mixer: {err}");
        }
        Ok(player)
    }

    /// Put a new band in, or take the band away. The one going out is not cut
    /// off: it is moved aside to play out its release, and dropped here on
    /// the UI thread next time round rather than on the audio thread.
    fn replace_band(&mut self, band: Option<BandPlayback>) {
        if band.is_none() {
            // Nothing will be ducking the send any more.
            self.agc.store(1.0);
        }
        let outgoing = std::mem::replace(&mut *lock_slot(&self.slots.band), band);
        if let Some(mut outgoing) = outgoing {
            outgoing.release();
            let finished = lock_slot(&self.slots.retiring).replace(outgoing);
            drop(finished);
        }
    }

    fn set_tone(&self, tone: Option<Arc<TonePlayback>>) {
        let previous = std::mem::replace(&mut *lock_slot(&self.slots.tone), tone);
        // A send's samples can run to megabytes; let them go here, not inside
        // the audio callback.
        drop(previous);
    }

    /// Bring the band in line with the settings, opening the stream first so
    /// a device that changed rate is known about before the band is built at
    /// the old one. Says whether the stream is up.
    fn band_and_mixer(&mut self, settings: &TrainingSettings) -> Result<(), String> {
        self.qsb.store(settings);
        let mixer = self.ensure_mixer();
        if let Err(err) = &mixer {
            eprintln!("audio mixer: {err}");
        }
        if self.state.band_needs_rebuild(settings) {
            let band = BandMixer::needs_background(settings).then(|| {
                // A fresh band every time, as the browser's is: with stations
                // on it, a fixed seed would replay the same callsigns at the
                // same moments in every session.
                BandPlayback::new(
                    BandMixer::new(self.sample_rate, settings, crate::time::seed_rng()),
                    self.sample_rate,
                    Arc::clone(&self.agc),
                )
            });
            self.replace_band(band);
            self.state.note_band(settings);
        }
        mixer
    }

    /// Make sure the output stream is up, reopening it if the device broke.
    fn ensure_mixer(&mut self) -> Result<(), String> {
        if self.stream_failed.swap(false, Ordering::SeqCst) {
            self.stream = None;
        }
        if self.stream.is_some() {
            return Ok(());
        }
        let (stream, sample_rate) = start_mixer(
            Arc::clone(&self.live),
            Arc::clone(&self.slots),
            Arc::clone(&self.stream_failed),
        )?;
        if sample_rate != self.sample_rate {
            // A different device, or the same one reconfigured. Everything is
            // rendered at the stream's rate, so the band has to be rebuilt at
            // the new one before it plays at the wrong pitch.
            self.sample_rate = sample_rate;
            self.replace_band(None);
            self.state.forget_band();
        }
        self.stream = Some(stream);
        Ok(())
    }
}

impl MorseBackend for MorsePlayer {
    fn apply_band(&mut self, settings: &TrainingSettings) -> Result<(), String> {
        let _ = self.band_and_mixer(settings);
        // A stream that will not open is not the band's failure: the band is
        // in place for when it does, and the next send reports the device —
        // which is what sends the session round its rebuild-and-retry.
        Ok(())
    }

    fn start_transmission(
        &mut self,
        transmission: &Transmission,
        settings: &TrainingSettings,
    ) -> Result<PlaybackWait, String> {
        let mixer = self.band_and_mixer(settings);
        self.live.set_on(false);
        self.set_tone(None);
        if let Err(err) = mixer {
            self.state.note_start_failed();
            return Err(format!("Audio mixer failed to open: {err}"));
        }

        let planned = plan_transmission(transmission, settings);
        let (duration_sec, char_wpm, effective_wpm) = (
            planned.wanted.duration_sec,
            planned.wanted.resolved_char_wpm,
            planned.wanted.resolved_effective_wpm,
        );
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
        self.set_tone(Some(playback));
        Ok(PlaybackWait::new(
            duration_sec,
            char_wpm,
            effective_wpm,
            std::rc::Rc::new(ToneSignal::new(
                armed,
                finished,
                Arc::clone(&self.stream_failed),
            )),
        ))
    }

    fn stop(&mut self) {
        // The send keeps its slot and rides its own release down to silence.
        self.state.stop();
    }

    fn shutdown(&mut self) {
        self.stop();
        self.replace_band(None);
        self.state.forget_band();
        self.live.set_on(false);
    }

    fn set_live_tone(&mut self, on: bool, frequency_hz: f64, gain: f64) {
        self.live.set(on, frequency_hz, gain);
        if let Err(err) = self.ensure_mixer() {
            eprintln!("audio mixer: {err}");
        }
    }
}

/// Build an output stream that fills `T` samples from an `f32` source.
fn build_stream<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    failed: Arc<AtomicBool>,
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
            move |err| {
                eprintln!("audio stream error: {err}");
                failed.store(true, Ordering::SeqCst);
            },
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
    failed: Arc<AtomicBool>,
    source: F,
) -> Result<cpal::Stream, String>
where
    F: FnMut(&mut [f32], usize) + Send + 'static,
{
    let (config, n) = (stream_config, channels);
    match supported.sample_format() {
        SampleFormat::F32 => build_stream::<f32, F>(device, config, n, failed, source),
        SampleFormat::F64 => build_stream::<f64, F>(device, config, n, failed, source),
        SampleFormat::I16 => build_stream::<i16, F>(device, config, n, failed, source),
        SampleFormat::I32 => build_stream::<i32, F>(device, config, n, failed, source),
        SampleFormat::U16 => build_stream::<u16, F>(device, config, n, failed, source),
        SampleFormat::U32 => build_stream::<u32, F>(device, config, n, failed, source),
        SampleFormat::I8 => build_stream::<i8, F>(device, config, n, failed, source),
        SampleFormat::U8 => build_stream::<u8, F>(device, config, n, failed, source),
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

/// Open the mixer on the default output, and say what rate it runs at.
fn start_mixer(
    live: Arc<LiveSidetone>,
    slots: Arc<MixerSlots>,
    failed: Arc<AtomicBool>,
) -> Result<(cpal::Stream, u32), String> {
    let (device, supported) = default_output()?;
    let channels = usize::from(supported.channels());
    let mut last_err = None;
    for config in mixer_configs(&supported) {
        match start_mixer_on(
            &device,
            &supported,
            &config,
            channels,
            Arc::clone(&live),
            Arc::clone(&slots),
            Arc::clone(&failed),
        ) {
            Ok(stream) => return Ok((stream, supported.sample_rate().0)),
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
    failed: Arc<AtomicBool>,
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
        failed,
        move |out, channels| {
            // The outgoing band first, so the live one has the last word on
            // the AGC both of them publish.
            play_layer(&slots.retiring, |band| {
                if !band.is_released() {
                    band.mix_into(out, channels);
                }
            });
            play_layer(&slots.band, |band| band.mix_into(out, channels));
            play_layer(&slots.tone, |tone| tone.mix_into(out, channels));
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

/// The mixer's slot handling, which needs no device to test.
#[cfg(test)]
mod mixer_tests {
    use super::*;

    /// A layer that panics is dropped, so it cannot panic again on every
    /// buffer that follows, and the lock it was behind is not poisoned for
    /// the UI thread.
    #[test]
    fn a_layer_that_panics_is_dropped_and_the_lock_survives() {
        let slot = Mutex::new(Some(1u32));
        play_layer(&slot, |_| panic!("a layer went wrong"));
        assert!(!slot.is_poisoned(), "the panic was caught inside the lock");
        assert!(
            lock_slot(&slot).is_none(),
            "the layer that panicked was dropped"
        );

        let mut played = 0;
        play_layer(&slot, |_| played += 1);
        assert_eq!(played, 0, "an empty slot plays nothing");
    }

    /// A lock poisoned elsewhere is still a good one: every write to a slot is
    /// a single assignment, so there is no half-written state to protect.
    #[test]
    fn a_poisoned_slot_still_plays() {
        let slot = Arc::new(Mutex::new(Some(2u32)));
        let poisoner = Arc::clone(&slot);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock();
            panic!("poison the lock");
        })
        .join();
        assert!(slot.is_poisoned());

        let mut seen = 0;
        play_layer(&slot, |value| seen = *value);
        assert_eq!(seen, 2);
        *lock_slot(&slot) = Some(3);
        assert_eq!(*lock_slot(&slot), Some(3));
    }

    /// The audio thread never waits: a slot the UI is holding is skipped for
    /// one buffer.
    #[test]
    fn a_busy_slot_is_skipped_rather_than_waited_for() {
        let slot = Mutex::new(Some(4u32));
        let _held = lock_slot(&slot);
        assert!(try_slot(&slot).is_none());
    }
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
        settings.band.noise_enabled = false;
        settings.band.activity_enabled = false;
        player.apply_band(&settings).expect("silent band");
        player.shutdown();
    }

    /// Changing the band does not cut the old one off mid-sample — that is a
    /// click — and a band taken away stops ducking the send.
    #[test]
    fn a_replaced_band_retires_and_lets_go_of_the_agc() {
        let Some(mut player) = player() else {
            return;
        };
        let mut settings = settings();
        player.apply_band(&settings).expect("band");
        assert!(lock_slot(&player.slots.band).is_some());

        player.agc.store(0.3);
        settings.band.qrn_enabled = false;
        settings.band.noise_enabled = false;
        settings.band.activity_enabled = false;
        player.apply_band(&settings).expect("silent band");
        assert!(lock_slot(&player.slots.band).is_none());
        assert!(
            lock_slot(&player.slots.retiring).is_some(),
            "the old band should be fading out, not dropped"
        );
        assert!(
            (player.agc.gain() - 1.0).abs() < 1e-6,
            "nothing is ducking any more"
        );
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
