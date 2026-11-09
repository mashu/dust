//! cpal output: ALSA/CoreAudio/WASAPI on desktop, AAudio through oboe on Android.
//!
//! Everything here that can run without a sound card lives in
//! [`super::mixer`] and [`state`]; what is left is the device glue.

mod state;

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample};
use cw_core::band::{BandChange, FilterDesign, Qsb, Receiver};
use cw_core::{PlannedTransmission, TrainingSettings, Transmission};

use super::ToneRequest;
use super::mixer::{
    BandVoice, Mixer, Output, SendVoice, SidetoneControl, Swept, clock_sec, fade_origin_sec,
};
use super::shape::Gains;
use super::{MorseBackend, PlaybackSignal, SendTiming};
use state::{Heartbeat, PlayerState, ToneSignal, moved_off_default};

/// The furthest behind the callback the speaker is believed to be. A host
/// that reports more is reporting nonsense, and a send would wait for it.
const MAX_LATENCY_SEC: f64 = 0.5;

/// How long after a device refused to open before a paddle squeeze or a
/// slider move tries it again, and after a stream in a loop of errors was
/// opened again before another is, whatever asks. Opening probes the device
/// and tries four buffer sizes on the UI thread; a send, which reports the
/// failure, always tries a device that refused, but for one whose every
/// stream falls into a loop of errors ([`MorsePlayer::refuse_while_looping`]).
const REOPEN_AFTER: Duration = Duration::from_secs(1);

/// Why the output is not up while a stream opened in place of one in a loop
/// of errors has fallen into one of its own.
const LOOPING: &str = "Audio stream: the device reports one error after another";

/// How long a stream that reports one error after another, with no buffer
/// asked for between them, is made to wait before reporting the next. Some
/// hosts' workers retry a dead device in a tight loop, calling back with an
/// error each time round; this keeps that loop from taking a core and the
/// log with it.
const ERROR_BACKOFF: Duration = Duration::from_millis(20);

/// How long a retired stream's worker ([`OpenStream`]'s drop) is made to
/// wait after each error it reports before it reports the next. Nothing
/// will tell it to stop: cpal 0.15's ALSA worker, on a device that reports
/// POLLERR, reports an error each time round and never prepares the device
/// again, and it ends only as its stream closes, which a retired stream
/// never does. Made to wait [`ERROR_BACKOFF`], it woke some fifty times a
/// second for as long as the app ran, and every stream retired left one
/// more. A wait, and not a stop for good: a test marks streams retired
/// that are then closed where closing waits on the worker, as on macOS and
/// Windows.
const RETIRED_BACKOFF: Duration = Duration::from_secs(1);

/// Whether letting go of a stream waits for its worker thread, which may
/// never come back to be told. cpal 0.15's ALSA host, its one host on
/// Linux and the BSDs, does: its worker retries a failed write in a loop
/// that never looks for the word to stop (an unplugged USB device, a PCM
/// suspended by the system's sleep, a sound server gone), and one blocked
/// in a write to a device that died never returns at all. Waited for on
/// the UI thread, either holds the app for good ([`OpenStream`]'s drop).
const DROP_WAITS_ON_WORKER: bool = cfg!(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd"
));

/// Whether the stream opens again on the device and config it last opened
/// on, rather than probing for them afresh. On Android the device is the
/// system's default route, which oboe follows by itself, and finding its
/// config asks Java about every rate, channel count and format it might
/// take, a call each, on the thread that answers taps: most of what waking
/// the output cost a tap. On a computer the stream closed while asleep may
/// wake to another default, which only a probe finds.
const KEEP_PROBED: bool = cfg!(target_os = "android");

/// Where the default output and the config it plays best at are found:
/// [`default_output`], unless a test counts how often it is asked.
type Probe = fn() -> Result<(cpal::Device, cpal::SupportedStreamConfig), String>;

thread_local! {
    /// When a stream was last taken for broken for its worker being in a
    /// loop of errors ([`MorsePlayer::fail_a_looping_stream`]), by any
    /// player made on this thread, the UI thread. Kept for the thread, not
    /// by each player: a send that fails is tried again on a player built
    /// afresh ([`crate::player`]), and one that knew nothing of the last
    /// would take the stream it opened, in a loop of its own at once, for
    /// broken at once, opening another and leaving one more looping worker
    /// behind on every try.
    static LOOP_FAILED_AT: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Whether a stream opened since the one at [`LOOP_FAILED_AT`] was
    /// taken for broken has fallen into a loop of its own within the while,
    /// and been refused for it ([`LOOPING`]), as the one opened in its place
    /// does at once on a device whose every stream falls into one as it
    /// opens: none is opened on this thread until the while is out
    /// ([`MorsePlayer::refuse_while_looping`]), or that stream comes out of
    /// its loop ([`MorsePlayer::forget_a_loop_recovered_from`]).
    static LOOPED_AGAIN: Cell<bool> = const { Cell::new(false) };
}

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
    /// When the device last refused to open, or a stream opened in place of
    /// one in a loop of errors fell into one of its own, and why: the
    /// stream's refusal goes as the stream comes out of its loop
    /// ([`MorsePlayer::forget_a_loop_recovered_from`]).
    refused: Option<(Instant, String)>,
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
    /// The device the mixer last heard the speaker's latency from: one the
    /// stream reopens on with another name has it heard afresh.
    latency_device: Option<String>,
    /// How the default output is found.
    probe: Probe,
    /// Whether the device and config the stream last opened on are kept
    /// for it to open on again ([`KEEP_PROBED`]).
    keeps_probed: bool,
    /// The device and config the stream last opened on, while they are
    /// kept: let go of once a stream on them broke, or would not open.
    probed: Option<(cpal::Device, cpal::SupportedStreamConfig)>,
}

/// An output stream, and the word that it broke: its host's, that its
/// device is gone, as unplugged headphones are. A send in flight on it
/// reports the failure, so the session can recover rather than stall.
///
/// Some breaks say nothing: an iOS interruption, a call or Siri, stops the
/// stream without a word. And some say only what may pass: ALSA tells of a
/// dead device as it tells of a poll a signal cut short. So the callback
/// notes when it ran, and a stream found not to have been called for a
/// while is taken for broken.
struct OpenStream {
    /// Taken as the stream is let go of.
    stream: Option<cpal::Stream>,
    /// The device's name, where it has one to read.
    device: Option<String>,
    failed: Arc<AtomicBool>,
    heartbeat: Arc<Heartbeat>,
    /// It reported an error, and has not been asked for a buffer since
    /// ([`StreamWords`]).
    erring: Arc<AtomicBool>,
    /// Let go of without its worker being waited for: its callback plays
    /// silence from then on, and leaves the mixer to the stream after it,
    /// and its worker waits [`RETIRED_BACKOFF`] after every error.
    retired: Arc<AtomicBool>,
}

impl Drop for OpenStream {
    /// A stream that broke, or has gone uncalled for as long as one that
    /// stopped is given, though a send would wait longer for it to start
    /// ([`Heartbeat::may_be_stuck`]), or whose worker is erring still, is
    /// let go of without its worker being waited for where that wait may
    /// never end ([`DROP_WAITS_ON_WORKER`]): retired, so that its
    /// callback, should the worker live on after all, plays silence and
    /// never takes the mixer from the stream opened after it, and left to
    /// the process with the device it holds. One that is not known to have
    /// broken closes as cpal closes it, but whatever that close throws stays
    /// here: cpal's ALSA worker can die by a panic of its own without a word
    /// to the error callback (a timestamp it does not expect), and the close
    /// then panics too, telling a worker that is gone to stop, or rejoining
    /// its panic. On the UI thread that took the app with it.
    fn drop(&mut self) {
        let Some(stream) = self.stream.take() else {
            return;
        };
        let broke = self.failed.load(Ordering::Acquire)
            || self.erring.load(Ordering::Acquire)
            || self.heartbeat.may_be_stuck(clock_sec());
        if DROP_WAITS_ON_WORKER && broke {
            self.retired.store(true, Ordering::Release);
            std::mem::forget(stream);
            return;
        }
        if catch_unwind(AssertUnwindSafe(move || drop(stream))).is_err() {
            eprintln!("audio output: the stream's worker had died");
        }
    }
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
    use dioxus::mobile::wry::prelude::jni;

    use crate::jvm::on_activity_thread;

    /// `AudioManager.STREAM_MUSIC`.
    const STREAM_MUSIC: i32 = 3;
    on_activity_thread(|env, activity| {
        env.call_method(
            activity,
            "setVolumeControlStream",
            "(I)V",
            &[jni::objects::JValue::Int(STREAM_MUSIC)],
        )?;
        Ok(())
    });
}

impl MorsePlayer {
    /// A player on the default output, with its stream opened now so the
    /// first squeeze of the paddle is not a stream start. A device that
    /// refuses to open gets another chance on first use, and one whose every
    /// stream falls into a loop of errors once the while is out
    /// ([`MorsePlayer::refuse_while_looping`]); no device at all is an
    /// error.
    pub fn new() -> Result<Self, String> {
        Self::probing(default_output, KEEP_PROBED)
    }

    /// The same, finding the default output by `probe`, and keeping what
    /// it opened on for the next open if `keeps_probed`.
    fn probing(probe: Probe, keeps_probed: bool) -> Result<Self, String> {
        #[cfg(target_os = "ios")]
        claim_audio_session();
        #[cfg(all(target_os = "android", not(test)))]
        claim_volume_keys();
        let (device, supported) = probe()?;
        let mixer = Mixer::new(supported.sample_rate().0);
        let mut player = Self {
            state: PlayerState::new(),
            mixer: Arc::new(Mutex::new(mixer)),
            sidetone: Arc::new(SidetoneControl::new()),
            stream: None,
            opened_at: Instant::now(),
            fade_origin_sec: fade_origin_sec(),
            refused: None,
            default_asked: None,
            heard_at: None,
            gains: Gains::UNITY,
            latency_device: None,
            probe,
            keeps_probed,
            probed: None,
        };
        let opened = player
            .refuse_while_looping()
            .and_then(|()| player.open(&device, &supported));
        match opened {
            Ok(()) => player.keep_probed(device, supported),
            Err(err) => eprintln!("audio output: {err}"),
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
    /// after the device broke, or, at most once a [`REOPEN_AFTER`], after
    /// its worker fell into a loop of errors
    /// ([`MorsePlayer::fail_a_looping_stream`]); but not on a device whose
    /// every stream falls into one, until the while is out
    /// ([`MorsePlayer::refuse_while_looping`]).
    fn ensure_output(&mut self) -> Result<(), String> {
        self.fail_a_looping_stream()?;
        if self.stream.as_ref().is_some_and(|open| {
            if open.heartbeat.went_silent(clock_sec()) {
                // Broken without a word: a send waiting on it is told so.
                open.failed.store(true, Ordering::Release);
            }
            open.failed.load(Ordering::Acquire)
        }) {
            // The old stream goes before a new one opens on the same device.
            // Whatever was playing on it went with it: a send waiting on it
            // has been told, and the band is rebuilt on the next apply. On
            // iOS the session an interruption took is claimed again first.
            // The route it played on may have gone with it, so the default
            // output is probed for again.
            self.stream = None;
            self.probed = None;
            #[cfg(target_os = "ios")]
            claim_audio_session();
            let rate = self.mixer().sample_rate();
            self.with_mixer(|mixer, swept| mixer.clear(rate, swept));
            self.state.forget_band();
        }
        if self.stream.is_some() {
            return Ok(());
        }
        self.refuse_while_looping()?;
        let (device, supported) = match self.probed.take() {
            Some(probed) => probed,
            None => (self.probe)().inspect_err(|why| self.note_refusal(why))?,
        };
        self.open(&device, &supported)?;
        self.keep_probed(device, supported);
        Ok(())
    }

    /// Take the stream for broken if its worker is in a loop of errors, as
    /// cpal's ALSA worker is on a device that died, or after an underrun it
    /// never recovers from: it sounds nothing, and says nothing of the
    /// stream breaking. At most once a [`REOPEN_AFTER`], whatever asks, and
    /// whichever player ([`LOOP_FAILED_AT`]): each time, the device is
    /// probed and opened again on the UI thread, the band is cleared, the
    /// old stream's worker is left looping where it is not waited for
    /// ([`DROP_WAITS_ON_WORKER`]), and the stream opened in its place can
    /// fall into a loop of its own at once. One that has, sooner than that,
    /// is said to have refused, and why, and no other stream is opened
    /// until the while is out ([`LOOPED_AGAIN`]), unless it comes out of its
    /// loop first ([`MorsePlayer::forget_a_loop_recovered_from`]).
    fn fail_a_looping_stream(&mut self) -> Result<(), String> {
        self.forget_a_loop_recovered_from();
        let Some(open) = &self.stream else {
            return Ok(());
        };
        if !open.heartbeat.looping() || open.failed.load(Ordering::Acquire) {
            return Ok(());
        }
        if LOOP_FAILED_AT
            .get()
            .is_some_and(|at| at.elapsed() < REOPEN_AFTER)
        {
            LOOPED_AGAIN.set(true);
            self.note_refusal(LOOPING);
            return Err(LOOPING.to_string());
        }
        open.failed.store(true, Ordering::Release);
        LOOP_FAILED_AT.set(Some(Instant::now()));
        LOOPED_AGAIN.set(false);
        Ok(())
    }

    /// Let go of the refusal of a stream that fell into a loop of errors
    /// ([`MorsePlayer::fail_a_looping_stream`]) once its worker has come
    /// out of the loop, called again since ([`Heartbeat::beat`]) and not
    /// broken: it plays, and is the output again, and the device is not one
    /// whose every stream falls into a loop ([`LOOPED_AGAIN`]). The refusal
    /// used to stand for the while whatever the stream did meanwhile, the
    /// sidetone and a slider move refused on a stream that played. One that
    /// is no stream's, refused as a stream was to be opened
    /// ([`MorsePlayer::refuse_while_looping`]), stands: while there is a
    /// stream, a refusal for a loop is that stream's, any opened since
    /// having let go of it ([`MorsePlayer::open`]).
    fn forget_a_loop_recovered_from(&mut self) {
        let recovered = self
            .stream
            .as_ref()
            .is_some_and(|open| !open.heartbeat.looping() && !open.failed.load(Ordering::Acquire));
        if recovered && self.refused.as_ref().is_some_and(|(_, why)| why == LOOPING) {
            self.refused = None;
            LOOPED_AGAIN.set(false);
        }
    }

    /// Refuse to open a stream, saying why ([`LOOPING`]), on a device whose
    /// every stream falls into a loop of errors as it opens: once a stream
    /// opened since one in a loop was taken for broken has fallen into a
    /// loop of its own ([`LOOPED_AGAIN`]), until a [`REOPEN_AFTER`] is out
    /// since that one was, or that stream comes out of its loop, whichever
    /// player asks, a player built afresh as it is made included. Every
    /// stream opened on such a device leaves its worker looping for good
    /// once it is let go of ([`DROP_WAITS_ON_WORKER`]), and a send that
    /// fails is tried again on a player built afresh ([`crate::player`]):
    /// one that opened a stream as it was made left one more worker behind
    /// on every try.
    fn refuse_while_looping(&mut self) -> Result<(), String> {
        let looping = LOOPED_AGAIN.get()
            && LOOP_FAILED_AT
                .get()
                .is_some_and(|at| at.elapsed() < REOPEN_AFTER);
        if looping {
            self.note_refusal(LOOPING);
            return Err(LOOPING.to_string());
        }
        Ok(())
    }

    /// Keep what the stream opened on, `device` at `supported`, for it to
    /// open on again, where that is done ([`KEEP_PROBED`]).
    fn keep_probed(&mut self, device: cpal::Device, supported: cpal::SupportedStreamConfig) {
        if self.keeps_probed {
            self.probed = Some((device, supported));
        }
    }

    /// Let the stream go if the system's default output has changed since it
    /// opened, for the next use to open on the new one: headphones plugged
    /// in or a headset connected mid-session, which a stream on Windows or
    /// macOS does not follow of itself. Asked at most once a
    /// [`REOPEN_AFTER`], and never before every send has been heard to its
    /// end, its last samples out of the speaker too, so none is cut off
    /// and no waiter is left without word that its send was heard; the band
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
            self.probed = None;
        }
    }

    /// [`MorsePlayer::ensure_output`], unless the device refused a moment
    /// ago: a squeeze of the paddle or a slider being dragged would otherwise
    /// probe it again on every event. A stream refused for a loop of errors
    /// is refused no more once it has come out of it
    /// ([`MorsePlayer::forget_a_loop_recovered_from`]). Why the output is
    /// not open, if it is not: what it said as it refused, a moment ago or
    /// now.
    fn ensure_output_unless_refused(&mut self) -> Result<(), String> {
        self.forget_a_loop_recovered_from();
        if let Some((at, why)) = &self.refused
            && at.elapsed() < REOPEN_AFTER
        {
            return Err(why.clone());
        }
        self.ensure_output()
            .inspect_err(|err| eprintln!("audio output: {err}"))
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
        // Woken on the same device, the speaker is as far behind as it was;
        // on another one, how far waits for its first callback. Forgotten
        // before the stream starts, so that callback is never the one
        // forgotten. A device that cannot say its name cannot be told from
        // the last.
        let name = device.name().ok();
        if name.is_some() && name != self.latency_device {
            self.mixer().forget_latency();
            self.latency_device = name;
        }
        let open = open_stream(device, supported, &self.mixer, &self.sidetone)
            .inspect_err(|why| self.note_refusal(why))?;
        self.stream = Some(open);
        self.refused = None;
        Ok(())
    }

    /// The device refused to open, or could not be found, saying `why`:
    /// the next try probes for the default output afresh.
    fn note_refusal(&mut self, why: &str) {
        self.refused = Some((Instant::now(), why.to_string()));
        self.probed = None;
    }
}

impl MorseBackend for MorsePlayer {
    /// Opens the stream first, so a device that changed rate is known about
    /// before the band is built at the old one.
    fn apply_band(&mut self, settings: &TrainingSettings) {
        self.follow_default();
        // A stream that will not open is reported by the next send.
        let _ = self.ensure_output_unless_refused();
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
        let Some((failed, heartbeat)) = self
            .stream
            .as_ref()
            .map(|open| (Arc::clone(&open.failed), Arc::clone(&open.heartbeat)))
        else {
            self.state.retire();
            return Err("Audio mixer failed to open.".into());
        };
        // In a statement of its own, after the stream is up at whatever rate
        // it opened at. Read inside the call below, the guard would last to
        // the end of that statement, and the voice would be built — its
        // allocations and all — with the audio callback locked out of the
        // mixer.
        let (rate, lookahead_sec, output_latency_sec, latency_heard) = {
            let mixer = self.mixer();
            (
                mixer.sample_rate(),
                mixer.lookahead_sec(),
                mixer.output_latency_sec(),
                mixer.latency_heard(),
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
        // An output that has not called back yet has not said how far behind
        // it the speaker is, so when the send is heard is not known: it is
        // not timed, as one held up on its way is not.
        if !latency_heard {
            late.store(true, Ordering::Release);
        }
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
            heartbeat,
            clock_sec(),
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

    /// A stream whose worker is in a loop of errors sounds no tone, and is
    /// taken for broken here as anywhere ([`MorsePlayer::ensure_output`]):
    /// the sidetone is told what the device says, not that it is there for
    /// as long as the stream is given to be called again.
    fn set_live_tone(&mut self, tone: ToneRequest) -> Result<(), String> {
        self.sidetone.set(tone);
        self.ensure_output_unless_refused()
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

/// What a stream's callbacks and its player tell each other: that it broke,
/// that it is still being called, that it is erring, and that it was let
/// go of.
#[derive(Clone)]
struct StreamWords {
    failed: Arc<AtomicBool>,
    heartbeat: Arc<Heartbeat>,
    /// It reported an error, and has not been asked for a buffer since: a
    /// worker retrying in a loop of errors, as cpal's ALSA host retries a
    /// write to a dead device, which never looks for the word to stop.
    erring: Arc<AtomicBool>,
    retired: Arc<AtomicBool>,
}

impl StreamWords {
    /// The stream asks for a buffer at `now`: whether to play it, rather
    /// than silence, retired. A worker that asks is in no loop of errors.
    fn called(&self, now: f64) -> bool {
        if self.retired.load(Ordering::Acquire) {
            return false;
        }
        self.heartbeat.beat(now);
        self.erring.store(false, Ordering::Release);
        true
    }

    /// The stream reports `err`. A device gone is gone for good, and a send
    /// on it is told so at once; anything else may pass, as a poll a signal
    /// cut short does, and the stream is taken for broken only once it is
    /// no longer called ([`Heartbeat`]). Whether another came before it
    /// with no buffer asked for since: a worker in a loop of them, which,
    /// before the stream was ever called, has a device that will not start.
    fn erred(&self, err: &cpal::StreamError) -> bool {
        if matches!(err, cpal::StreamError::DeviceNotAvailable) {
            self.failed.store(true, Ordering::Release);
        }
        let again = self.erring.swap(true, Ordering::AcqRel);
        if again {
            self.heartbeat.erred_again();
        }
        again
    }

    /// The stream reports `err` ([`Self::erred`]): how long its worker is
    /// made to wait before it reports the next, if at all. A live stream's
    /// waits [`ERROR_BACKOFF`] in a loop of errors; a retired stream's,
    /// [`RETIRED_BACKOFF`] after every error.
    fn backoff_after(&self, err: &cpal::StreamError) -> Option<Duration> {
        let again = self.erred(err);
        if self.retired.load(Ordering::Acquire) {
            Some(RETIRED_BACKOFF)
        } else {
            again.then_some(ERROR_BACKOFF)
        }
    }
}

/// Build an output stream that plays `output` into `T` samples, until it is
/// retired ([`OpenStream`]'s drop).
fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut output: Output,
    words: StreamWords,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let playing = words.clone();
    let mut told = false;
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
                if !playing.called(clock_sec()) {
                    data.fill(T::EQUILIBRIUM);
                    return;
                }
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
                if let Some(backoff) = words.backoff_after(&err) {
                    std::thread::sleep(backoff);
                } else if !told {
                    // The first error is the one worth reading.
                    told = true;
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
        let heartbeat = Arc::new(Heartbeat::new(clock_sec()));
        let erring = Arc::new(AtomicBool::new(false));
        let retired = Arc::new(AtomicBool::new(false));
        let words = StreamWords {
            failed: Arc::clone(&failed),
            heartbeat: Arc::clone(&heartbeat),
            erring: Arc::clone(&erring),
            retired: Arc::clone(&retired),
        };
        let built = match supported.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(device, &config, output, words),
            SampleFormat::F64 => build_stream::<f64>(device, &config, output, words),
            SampleFormat::I16 => build_stream::<i16>(device, &config, output, words),
            SampleFormat::I32 => build_stream::<i32>(device, &config, output, words),
            SampleFormat::U16 => build_stream::<u16>(device, &config, output, words),
            SampleFormat::U32 => build_stream::<u32>(device, &config, output, words),
            SampleFormat::I8 => build_stream::<i8>(device, &config, output, words),
            SampleFormat::U8 => build_stream::<u8>(device, &config, output, words),
            other => Err(format!("Unsupported sample format: {other}")),
        };
        match built.and_then(|stream| {
            stream.play().map_err(|e| format!("Audio play: {e}"))?;
            Ok(stream)
        }) {
            Ok(stream) => {
                return Ok(OpenStream {
                    stream: Some(stream),
                    device: device.name().ok(),
                    failed,
                    heartbeat,
                    erring,
                    retired,
                });
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| "Audio mixer: no stream config worked".to_string()))
}

#[cfg(test)]
mod word_tests {
    use super::*;

    fn words() -> StreamWords {
        StreamWords {
            failed: Arc::new(AtomicBool::new(false)),
            heartbeat: Arc::new(Heartbeat::new(0.0)),
            erring: Arc::new(AtomicBool::new(false)),
            retired: Arc::new(AtomicBool::new(false)),
        }
    }

    /// An error that may pass, as an ALSA poll a signal cut short does,
    /// fails no stream: a send on it plays on, and the stream is taken for
    /// broken only once it is no longer called. It is erring until it next
    /// asks for a buffer, and an error that follows another with none
    /// asked for between them is a loop's. A device gone fails the stream
    /// at once. Any error used to fail it for good: the send on it failed,
    /// and was sent again over a band built anew.
    #[test]
    fn only_a_device_gone_fails_a_stream_at_once() {
        let words = words();
        let passing = cpal::StreamError::BackendSpecific {
            err: cpal::BackendSpecificError {
                description: "Interrupted system call".into(),
            },
        };
        let erring = || words.erring.load(Ordering::Acquire);
        assert!(!words.erred(&passing), "the first");
        assert!(!words.failed.load(Ordering::Acquire), "it may pass");
        assert!(erring(), "erring");
        assert!(words.erred(&passing), "another, no buffer between");
        assert!(words.called(1.0));
        assert!(!erring(), "asked for a buffer since");
        assert_eq!(words.heartbeat.last(), 1.0);
        assert!(!words.erred(&passing));
        assert!(!words.failed.load(Ordering::Acquire));
        assert!(words.called(2.0));
        words.erred(&cpal::StreamError::DeviceNotAvailable);
        assert!(words.failed.load(Ordering::Acquire), "a device gone");
        words.retired.store(true, Ordering::Release);
        assert!(!words.called(3.0), "retired, it plays silence");
        assert_eq!(words.heartbeat.last(), 2.0);
    }

    /// A stream whose worker errs over and over before it is ever called
    /// has a device that will not start, not one slow to: it is given no
    /// longer to be called the first time than a stream that stopped is to
    /// be called again, and a send on it fails as soon. One error alone may
    /// pass, as a poll a signal cut short does, and leaves it its time.
    #[test]
    fn a_stream_erring_over_and_over_before_it_is_called_is_given_no_time_to_start() {
        let passing = cpal::StreamError::BackendSpecific {
            err: cpal::BackendSpecificError {
                description: "Interrupted system call".into(),
            },
        };
        let looked = state::SILENT_AFTER_SEC + 0.1;
        let unstarted = words();
        unstarted.erred(&passing);
        assert!(
            !unstarted.heartbeat.went_silent(looked),
            "one error may pass"
        );
        unstarted.erred(&passing);
        assert!(unstarted.heartbeat.went_silent(looked), "a loop of them");

        let started = words();
        assert!(started.called(1.0));
        started.erred(&passing);
        started.erred(&passing);
        assert!(!started.heartbeat.went_silent(1.0 + state::SILENT_AFTER_SEC));
        assert!(
            started.heartbeat.went_silent(1.1 + state::SILENT_AFTER_SEC),
            "called, then erring: as one that stopped"
        );
    }

    /// A retired stream's worker, left looping on errors with nothing ever
    /// to tell it to stop, as cpal's ALSA worker is on a device that
    /// reports POLLERR, waits about a second after each error before it
    /// reports the next, from the first error after it was retired; a
    /// live stream's worker in a loop of errors waits [`ERROR_BACKOFF`],
    /// and after an error that may pass, not at all. About a second, and
    /// not for good: a stream a test marks retired may be closed after all
    /// where closing waits on its worker. A retired stream's worker used to
    /// wait [`ERROR_BACKOFF`] too, and woke some fifty times a second for
    /// as long as the app ran, a thread more for every stream retired.
    #[test]
    fn a_retired_stream_in_a_loop_of_errors_waits_a_second_between_them() {
        let pollerr = cpal::StreamError::BackendSpecific {
            err: cpal::BackendSpecificError {
                description: "`alsa::poll()` returned POLLERR".into(),
            },
        };
        let about_a_second = Duration::from_secs(1)..=Duration::from_secs(2);
        let looping = words();
        assert_eq!(looping.backoff_after(&pollerr), None, "the first");
        assert_eq!(
            looping.backoff_after(&pollerr),
            Some(ERROR_BACKOFF),
            "a loop of them"
        );
        looping.retired.store(true, Ordering::Release);
        for error in 0..3 {
            let waits = looping.backoff_after(&pollerr);
            assert!(
                waits.is_some_and(|waits| about_a_second.contains(&waits)),
                "retired, error {error}: {waits:?}"
            );
        }
        let retired = words();
        retired.retired.store(true, Ordering::Release);
        let waits = retired.backoff_after(&pollerr);
        assert!(
            waits.is_some_and(|waits| about_a_second.contains(&waits)),
            "retired before it ever erred: {waits:?}"
        );
    }
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
        opened(MorsePlayer::new())
    }

    /// The player `made`, as [`player`] takes it.
    fn opened(made: Result<MorsePlayer, String>) -> Option<MorsePlayer> {
        match made {
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

    /// Asleep, the stream is closed and calls back no more, and the next
    /// sidetone opens it to play on.
    #[test]
    fn a_sleeping_stream_calls_back_no_more_until_it_is_woken() {
        let Some(mut player) = player() else {
            return;
        };
        // Whether the stream's callback ran over the next `ms`.
        let called = |player: &MorsePlayer, ms: u64| {
            let beat =
                |player: &MorsePlayer| player.stream.as_ref().map(|open| open.heartbeat.last());
            let before = beat(player);
            std::thread::sleep(Duration::from_millis(ms));
            before.is_some() && beat(player) > before
        };
        assert!(called(&player, 200), "the stream plays");
        player.shutdown();
        player.sleep();
        assert!(player.stream.is_none(), "asleep, it is closed");
        player
            .set_live_tone(ToneRequest::new(false, 600.0, 0.2))
            .expect("the sidetone opened it");
        assert!(player.stream.is_some(), "the sidetone opened it");
        assert!(called(&player, 300), "woken, it plays");
        player.shutdown();
    }

    thread_local! {
        /// How many times [`counted_probe`] has been asked on this thread.
        static PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// [`default_output`], counted.
    fn counted_probe() -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
        PROBES.set(PROBES.get() + 1);
        default_output()
    }

    /// A stream that broke is let go of without waiting on its worker where
    /// that wait may never end, as cpal's ALSA worker stuck retrying a
    /// write to a dead device never ends it: retired, its callback plays
    /// silence and leaves the mixer to the stream opened after it, which
    /// plays on. A stream that did not break, closed as the output sleeps,
    /// is closed as cpal closes it, and retired never.
    #[test]
    fn a_broken_stream_is_let_go_of_without_waiting_on_its_worker() {
        let Some(mut player) = player() else {
            return;
        };
        player.ensure_output().expect("open");
        let broken = player.stream.as_ref().expect("open");
        let retired = Arc::clone(&broken.retired);
        broken.failed.store(true, Ordering::Release);
        player.ensure_output().expect("opened again");
        assert_eq!(
            retired.load(Ordering::Acquire),
            DROP_WAITS_ON_WORKER,
            "a broken stream is retired where its worker is not waited for"
        );
        let next = player.stream.as_ref().expect("a new stream");
        let (beat, healthy) = (Arc::clone(&next.heartbeat), Arc::clone(&next.retired));
        let since = beat.last();
        let called = Instant::now() + Duration::from_secs(5);
        while beat.last() <= since {
            assert!(Instant::now() < called, "the new stream plays");
            std::thread::sleep(Duration::from_millis(5));
        }
        player.sleep();
        assert!(player.stream.is_none(), "asleep, it is closed");
        assert!(!healthy.load(Ordering::Acquire), "closed, not retired");
    }

    /// A stream its device has yet to call is let go of as one that plays
    /// is, closed as cpal closes it, while it is just opened; and, as one
    /// that stopped is, without its worker being waited for where that wait
    /// may never end, once it has gone uncalled for as long as one that
    /// stopped is given, though a send would wait longer for it to start: a
    /// worker that has not called back by then may be stuck in a call to
    /// its device, on a sound server's lock or a prepare after an underrun,
    /// and waited for, it held the UI thread as long. It used to be closed
    /// as cpal closes it until it had gone uncalled for as long as a stream
    /// is given to be called the first time.
    #[test]
    fn a_stream_never_called_is_retired_as_soon_as_one_that_stopped_would_be() {
        let Some(mut player) = player() else {
            return;
        };
        player.sleep();
        let (device, supported) = default_output().expect("the output");
        // A stream opened at `opened_sec` and never called, let go of now:
        // whether it was retired.
        let mut let_go = |opened_sec: f64| {
            let stream = device
                .build_output_stream_raw(
                    &supported.config(),
                    supported.sample_format(),
                    |_: &mut cpal::Data, _: &cpal::OutputCallbackInfo| {},
                    |_| {},
                    None,
                )
                .expect("a stream");
            stream.play().expect("it plays");
            let retired = Arc::new(AtomicBool::new(false));
            player.stream = Some(OpenStream {
                stream: Some(stream),
                device: None,
                failed: Arc::new(AtomicBool::new(false)),
                heartbeat: Arc::new(Heartbeat::new(opened_sec)),
                erring: Arc::new(AtomicBool::new(false)),
                retired: Arc::clone(&retired),
            });
            player.sleep();
            assert!(player.stream.is_none(), "asleep, it is closed");
            retired.load(Ordering::Acquire)
        };
        assert!(!let_go(clock_sec()), "just opened, it is closed");
        assert!(
            !let_go(clock_sec() - state::SILENT_AFTER_SEC + 0.5),
            "uncalled for less than one that stopped is given, it is closed"
        );
        assert_eq!(
            let_go(clock_sec() - state::SILENT_AFTER_SEC - 0.5),
            DROP_WAITS_ON_WORKER,
            "uncalled for longer, though still given time to start, it is retired where its \
             worker is not waited for"
        );
        assert_eq!(
            let_go(clock_sec() - state::FIRST_CALL_SEC - 1.0),
            DROP_WAITS_ON_WORKER,
            "never called in its time, it is retired where its worker is not waited for"
        );
    }

    /// A stream whose worker died without a word is let go of without its
    /// close taking the app with it: cpal's ALSA close tells the worker to
    /// stop down a pipe the dead worker no longer reads, and panics; that
    /// stays in the close. It used to unwind through the UI thread as the
    /// output went to sleep. Linux alone: on a Mac a panic in the audio
    /// callback aborts.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_stream_whose_worker_died_is_let_go_of_without_a_panic() {
        let Some(mut player) = player() else {
            return;
        };
        player.sleep();
        let (device, supported) = default_output().expect("the output");
        let ran = Arc::new(AtomicBool::new(false));
        let running = Arc::clone(&ran);
        let stream = device
            .build_output_stream_raw(
                &supported.config(),
                supported.sample_format(),
                move |_: &mut cpal::Data, _: &cpal::OutputCallbackInfo| {
                    running.store(true, Ordering::Release);
                    panic!("the worker dies");
                },
                |_| {},
                None,
            )
            .expect("a stream");
        stream.play().expect("it plays");
        let called = Instant::now() + Duration::from_secs(5);
        while !ran.load(Ordering::Acquire) {
            assert!(Instant::now() < called, "the callback ran");
            std::thread::sleep(Duration::from_millis(5));
        }
        // The worker unwinds, and the end of the pipe it read goes with it.
        std::thread::sleep(Duration::from_millis(100));
        player.stream = Some(OpenStream {
            stream: Some(stream),
            device: None,
            failed: Arc::new(AtomicBool::new(false)),
            heartbeat: Arc::new(Heartbeat::new(clock_sec())),
            erring: Arc::new(AtomicBool::new(false)),
            retired: Arc::new(AtomicBool::new(false)),
        });
        let slept = catch_unwind(AssertUnwindSafe(|| player.sleep()));
        assert!(slept.is_ok(), "the close kept its panic to itself");
        assert!(player.stream.is_none(), "asleep, it is closed");
    }

    /// A stream erring still, its worker in a loop of errors with no buffer
    /// asked for since, is let go of without its worker being waited for,
    /// though nothing said it broke: cpal's ALSA worker retrying a write to
    /// a dead device never comes back to be told. The worker holds the
    /// stream's words for as long as it lives.
    #[test]
    fn an_erring_stream_is_let_go_of_without_waiting_on_its_worker() {
        let Some(mut player) = player() else {
            return;
        };
        player.ensure_output().expect("open");
        let open = player.stream.as_ref().expect("open");
        // It asks for no buffer from here, as a worker in a loop of errors
        // asks for none, and errs.
        open.retired.store(true, Ordering::Release);
        // A callback already past its look at `retired` would clear
        // `erring` as it ends: it is let end first.
        std::thread::sleep(Duration::from_millis(100));
        open.erring.store(true, Ordering::Release);
        let words = Arc::clone(&open.heartbeat);
        player.sleep();
        assert!(player.stream.is_none(), "let go of");
        assert_eq!(
            Arc::strong_count(&words) > 1,
            DROP_WAITS_ON_WORKER,
            "left to its worker where it is not waited for"
        );
    }

    /// Where what it opened on is kept, as on Android, the output wakes
    /// from sleep on it again and again without probing for it; a stream
    /// that broke, or a device that refused, has it probed for afresh, once.
    /// Where it is not, every wake probes, so a new default is found.
    #[test]
    fn a_kept_output_wakes_without_probing_until_it_breaks() {
        PROBES.set(0);
        let Some(mut player) = opened(MorsePlayer::probing(counted_probe, true)) else {
            return;
        };
        let wake = |player: &mut MorsePlayer| {
            player.sleep();
            assert!(player.stream.is_none(), "asleep, it is closed");
            player.ensure_output().expect("the output wakes");
            assert!(player.stream.is_some(), "woken, it is open");
        };
        assert_eq!(PROBES.get(), 1, "probed as it opened");
        for _ in 0..3 {
            wake(&mut player);
        }
        assert_eq!(PROBES.get(), 1, "woken on what it opened on");
        player
            .stream
            .as_ref()
            .expect("open")
            .failed
            .store(true, Ordering::Release);
        player.ensure_output().expect("opened again");
        assert_eq!(PROBES.get(), 2, "a broken stream has the output probed");
        wake(&mut player);
        assert_eq!(PROBES.get(), 2);
        player.note_refusal("refused");
        wake(&mut player);
        assert_eq!(PROBES.get(), 3, "a refusal has the output probed");
        wake(&mut player);
        assert_eq!(PROBES.get(), 3);
        player.shutdown();

        PROBES.set(0);
        let Some(mut probing) = opened(MorsePlayer::probing(counted_probe, false)) else {
            return;
        };
        for _ in 0..3 {
            wake(&mut probing);
        }
        assert_eq!(PROBES.get(), 4, "each wake probed");
        probing.shutdown();
    }

    /// The machine's default output, at a sample format no stream takes:
    /// an output that is there, and will not open.
    fn unplayable() -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
        let (device, supported) = default_output()?;
        let refused = cpal::SupportedStreamConfig::new(
            supported.channels(),
            supported.sample_rate(),
            *supported.buffer_size(),
            SampleFormat::U64,
        );
        Ok((device, refused))
    }

    /// An output that is there and will not open is said to the sidetone,
    /// as the pad asks before the first squeeze, for it to show and to key
    /// nothing into it; and said again while the output is not tried
    /// again. It used to be said only where there was no output at all.
    #[test]
    fn an_output_that_will_not_open_is_said_to_the_sidetone() {
        if opened(MorsePlayer::probing(unplayable, false)).is_none() {
            return;
        }
        let player = crate::player::Player::new(Rc::new(|| {
            let made: Box<dyn MorseBackend> = Box::new(MorsePlayer::probing(unplayable, false)?);
            Ok(made)
        }));
        for _ in 0..2 {
            let said = player.ensure_live_sidetone(&settings());
            assert!(
                matches!(&said, Err(crate::player::AudioError::Device(why)) if why.contains("u64")),
                "{said:?}"
            );
        }
    }

    thread_local! {
        /// Whether [`dying`] has found the output on this thread.
        static FOUND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// The machine's default output the first time it is asked for, and
    /// from then on there and will not open, as a device that died is.
    fn dying() -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
        if FOUND.replace(true) {
            unplayable()
        } else {
            default_output()
        }
    }

    /// `open`'s worker falls into a loop of errors, as cpal's ALSA worker
    /// does on a device that died: it reports one error after another, and
    /// asks for no buffer between them.
    fn fall_into_a_loop(open: &OpenStream) {
        let words = StreamWords {
            failed: Arc::clone(&open.failed),
            heartbeat: Arc::clone(&open.heartbeat),
            erring: Arc::clone(&open.erring),
            retired: Arc::clone(&open.retired),
        };
        // It asks for no buffer from here, as a worker in a loop of errors
        // asks for none, and a callback already past its look at `retired`
        // is let end first.
        words.retired.store(true, Ordering::Release);
        std::thread::sleep(Duration::from_millis(100));
        let gone = cpal::StreamError::BackendSpecific {
            err: cpal::BackendSpecificError {
                description: "No such device".into(),
            },
        };
        words.erred(&gone);
        words.erred(&gone);
    }

    /// An output whose worker has fallen into a loop of errors, as cpal's
    /// ALSA worker does on a device that died, is not said to the sidetone
    /// to be there: taken for broken, it is opened again, and the sidetone
    /// is told what the device says as it refuses. It used to be said to be
    /// there for the second and a half the stream is given to be called
    /// again, an ALSA device's death being no word that the stream broke.
    #[test]
    fn an_output_in_a_loop_of_errors_is_said_to_the_sidetone() {
        FOUND.set(false);
        let Some(mut player) = opened(MorsePlayer::probing(dying, false)) else {
            return;
        };
        fall_into_a_loop(player.stream.as_ref().expect("open"));
        let said = player.set_live_tone(ToneRequest::new(false, 600.0, 0.2));
        assert!(matches!(&said, Err(why) if why.contains("u64")), "{said:?}");
    }

    /// A stream in a loop of errors is opened again at most once a
    /// [`REOPEN_AFTER`], however many edges the paddle keys meanwhile, and
    /// however soon the stream opened in its place falls into a loop of its
    /// own, as each can after an underrun cpal's ALSA host never recovers
    /// from: every opening probes the device and tries four buffer sizes on
    /// the UI thread, clears the band, and on Linux leaves the worker of the
    /// stream it replaced looping for good. In between, the sidetone is told
    /// the output is not there. It used to be opened again at every edge, a
    /// thread left behind each time.
    #[test]
    fn a_stream_in_a_loop_of_errors_is_opened_again_at_most_once_a_while() {
        PROBES.set(0);
        let Some(mut player) = opened(MorsePlayer::probing(counted_probe, false)) else {
            return;
        };
        // An edge keyed, every stream the player has falling into a loop
        // of errors before it: whether it had the output opened again, and
        // what the sidetone was told.
        let key = |player: &mut MorsePlayer, on: bool| {
            if let Some(open) = &player.stream
                && !open.heartbeat.looping()
            {
                fall_into_a_loop(open);
            }
            let probes = PROBES.get();
            let said = player.set_live_tone(ToneRequest::new(on, 600.0, 0.2));
            (PROBES.get() > probes, said)
        };
        let began = Instant::now();
        let mut reopened = 0u32;
        for edge in 0..40 {
            let (opened_again, said) = key(&mut player, edge % 2 == 0);
            if opened_again {
                reopened += 1;
            } else {
                assert!(said.is_err(), "edge {edge}: {said:?}");
            }
        }
        let windows = began.elapsed().as_secs_f64() / REOPEN_AFTER.as_secs_f64();
        assert!(reopened >= 1, "the first edge opened it again");
        assert!(
            f64::from(reopened) <= windows.floor() + 1.0,
            "opened again {reopened} times in {windows:.2} windows"
        );
        std::thread::sleep(REOPEN_AFTER);
        let (opened_again, said) = key(&mut player, false);
        assert!(opened_again, "a while on, it is opened again: {said:?}");
        player.shutdown();
    }

    /// A send finds a stream in a loop of errors as the sidetone does: it
    /// is opened again before the send starts, and the send plays on the
    /// one opened in its place, not on one that plays nothing until it has
    /// gone uncalled long enough to be taken for broken. Should that one
    /// fall into a loop of its own a moment later, the next send fails at
    /// once rather than open a third.
    #[test]
    fn a_send_on_a_stream_in_a_loop_of_errors_plays_on_one_opened_in_its_place() {
        let Some(mut player) = player() else {
            return;
        };
        let voice = cw_core::resolve_station(&settings(), &mut cw_core::SplitMix64::new(7));
        let send = |player: &mut MorsePlayer| {
            crate::audio::start_send(player, &Transmission::alone("K", voice), &settings())
        };
        let looping = player.stream.as_ref().expect("open");
        fall_into_a_loop(looping);
        let looping = Arc::clone(&looping.heartbeat);
        let began = Instant::now();
        send(&mut player).expect("the send starts");
        let open = player.stream.as_ref().expect("open");
        assert!(
            !Arc::ptr_eq(&open.heartbeat, &looping),
            "the send plays on a stream opened in its place"
        );
        fall_into_a_loop(open);
        let said = send(&mut player).map(|_| ());
        if began.elapsed() < REOPEN_AFTER {
            assert!(said.is_err(), "{said:?}");
        }
        player.shutdown();
    }

    /// A stream in a loop of errors is opened again at most once a
    /// [`REOPEN_AFTER`] whichever player asks: a player built in place of
    /// one that took a stream in a loop for broken, as a send that failed
    /// is tried again on a player built afresh, finds the stream it opened
    /// fallen into a loop of its own within the while, and says the output
    /// refused rather than open it again. A player built afresh used to
    /// take it for broken at once, so every try of the send opened a stream
    /// more and left one more looping worker behind.
    #[test]
    fn a_stream_in_a_loop_of_errors_is_opened_again_at_most_once_a_while_whichever_player_asks() {
        PROBES.set(0);
        let Some(mut failed) = opened(MorsePlayer::probing(counted_probe, false)) else {
            return;
        };
        fall_into_a_loop(failed.stream.as_ref().expect("open"));
        let began = Instant::now();
        failed
            .set_live_tone(ToneRequest::new(false, 600.0, 0.2))
            .expect("opened again in its place");
        failed.shutdown();
        drop(failed);
        let Some(mut rebuilt) = opened(MorsePlayer::probing(counted_probe, false)) else {
            return;
        };
        fall_into_a_loop(rebuilt.stream.as_ref().expect("open"));
        let probes = PROBES.get();
        let said = rebuilt.set_live_tone(ToneRequest::new(false, 600.0, 0.2));
        if began.elapsed() < REOPEN_AFTER {
            assert_eq!(PROBES.get(), probes, "opened again: {said:?}");
            assert_eq!(said, Err(LOOPING.to_string()));
        }
        rebuilt.shutdown();
    }

    /// Once the stream opened in place of one in a loop of errors has
    /// fallen into a loop of its own, the device is one whose every stream
    /// does, and no stream is opened on the thread until a [`REOPEN_AFTER`]
    /// is out, whichever player asks: a player built afresh, as a send that
    /// failed is tried again on one, opens none as it is made, and a send
    /// and the sidetone on it are refused, saying why, the device not
    /// probed for them; a while on, it opens one. A player built afresh
    /// used to open a stream as it was made, which on such a device fell
    /// into a loop of its own at once: every try of a failed send left one
    /// more worker looping for good, some four a send.
    #[test]
    fn a_player_built_afresh_opens_no_stream_while_every_stream_opened_falls_into_a_loop() {
        let Some(mut failed) = opened(MorsePlayer::probing(counted_probe, false)) else {
            return;
        };
        fall_into_a_loop(failed.stream.as_ref().expect("open"));
        let began = Instant::now();
        failed
            .set_live_tone(ToneRequest::new(false, 600.0, 0.2))
            .expect("opened again in its place");
        fall_into_a_loop(failed.stream.as_ref().expect("opened again"));
        let refused = failed.set_live_tone(ToneRequest::new(false, 600.0, 0.2));
        failed.shutdown();
        drop(failed);
        let Some(mut rebuilt) = opened(MorsePlayer::probing(counted_probe, false)) else {
            return;
        };
        let opened_afresh = rebuilt.stream.is_some();
        let probes = PROBES.get();
        let voice = cw_core::resolve_station(&settings(), &mut cw_core::SplitMix64::new(7));
        let sent =
            crate::audio::start_send(&mut rebuilt, &Transmission::alone("K", voice), &settings())
                .map(|_| ());
        let toned = rebuilt.set_live_tone(ToneRequest::new(false, 600.0, 0.2));
        if began.elapsed() < REOPEN_AFTER {
            assert_eq!(refused, Err(LOOPING.to_string()));
            assert!(!opened_afresh, "a player built afresh opened a stream");
            assert!(
                matches!(&sent, Err(why) if why.contains(LOOPING)),
                "{sent:?}"
            );
            assert_eq!(toned, Err(LOOPING.to_string()));
            assert!(
                rebuilt.stream.is_none(),
                "the send or the sidetone opened one"
            );
            assert_eq!(PROBES.get(), probes, "probed for the send or the sidetone");
        }
        std::thread::sleep(REOPEN_AFTER);
        rebuilt
            .set_live_tone(ToneRequest::new(false, 600.0, 0.2))
            .expect("a while on, it opens one");
        assert!(rebuilt.stream.is_some(), "a while on, it opens one");
        rebuilt.shutdown();
    }

    /// `player`'s stream falls into a loop of errors and is opened again in
    /// its place, and the one opened falls into a loop of its own within the
    /// while: what the sidetone is then told.
    fn looped_again(player: &mut MorsePlayer) -> Result<(), String> {
        fall_into_a_loop(player.stream.as_ref().expect("open"));
        player
            .set_live_tone(ToneRequest::new(false, 600.0, 0.2))
            .expect("opened again in its place");
        fall_into_a_loop(player.stream.as_ref().expect("opened again"));
        player.set_live_tone(ToneRequest::new(false, 600.0, 0.2))
    }

    /// `open`'s worker comes out of its loop of errors ([`fall_into_a_loop`])
    /// and asks for buffers again, as one does once an underrun it was
    /// caught in passes.
    fn come_out_of_the_loop(open: &OpenStream) {
        open.retired.store(false, Ordering::Release);
        let called = Instant::now() + Duration::from_secs(5);
        while open.heartbeat.looping() {
            assert!(Instant::now() < called, "called again");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A stream refused for falling into a loop of errors as soon as it was
    /// opened in place of another is the output again as soon as its worker
    /// comes out of the loop and calls back: the sidetone is told it is
    /// there, on that stream, the device not probed for it, and a player
    /// built afresh within the while opens one, the device's streams not
    /// all falling into a loop. The refusal used to stand for the second
    /// the device is given after a loop, whatever the stream did meanwhile:
    /// the sidetone, and a slider move, were refused on a stream that
    /// played, and a player built afresh opened none.
    #[test]
    fn a_stream_refused_for_a_loop_of_errors_is_the_output_again_once_called_again() {
        PROBES.set(0);
        let Some(mut player) = opened(MorsePlayer::probing(counted_probe, false)) else {
            return;
        };
        let began = Instant::now();
        let refused = looped_again(&mut player);
        let open = player.stream.as_ref().expect("kept while refused");
        come_out_of_the_loop(open);
        let came_back = Arc::clone(&open.heartbeat);
        let probes = PROBES.get();
        let toned = player.set_live_tone(ToneRequest::new(false, 600.0, 0.2));
        let kept =
            (player.stream.as_ref()).is_some_and(|open| Arc::ptr_eq(&open.heartbeat, &came_back));
        let probed = PROBES.get() > probes;
        player.shutdown();
        drop(player);
        let afresh = opened(MorsePlayer::probing(counted_probe, false));
        if began.elapsed() < REOPEN_AFTER {
            assert_eq!(refused, Err(LOOPING.to_string()), "refused while it loops");
            assert_eq!(toned, Ok(()), "called again, it is the output");
            assert!(kept && !probed, "on the stream that came out of its loop");
            assert!(
                afresh
                    .as_ref()
                    .is_some_and(|afresh| afresh.stream.is_some()),
                "a player built afresh opened none"
            );
        }
        if let Some(mut afresh) = afresh {
            afresh.shutdown();
        }
    }

    /// A send finds a stream that came out of its loop of errors as the
    /// sidetone does: it plays on that stream, and the device is no longer
    /// taken for one whose every stream falls into a loop, so a player built
    /// afresh within the while, as the send is tried again on one should it
    /// fail after all, opens a stream. It used to open none until the while
    /// was out, the send on the stream that played notwithstanding.
    #[test]
    fn a_send_on_a_stream_out_of_its_loop_of_errors_lets_a_player_built_afresh_open_one() {
        let Some(mut player) = opened(MorsePlayer::probing(counted_probe, false)) else {
            return;
        };
        let began = Instant::now();
        let refused = looped_again(&mut player);
        let open = player.stream.as_ref().expect("kept while refused");
        come_out_of_the_loop(open);
        let came_back = Arc::clone(&open.heartbeat);
        let voice = cw_core::resolve_station(&settings(), &mut cw_core::SplitMix64::new(7));
        let sent =
            crate::audio::start_send(&mut player, &Transmission::alone("K", voice), &settings())
                .map(|_| ());
        let kept =
            (player.stream.as_ref()).is_some_and(|open| Arc::ptr_eq(&open.heartbeat, &came_back));
        player.shutdown();
        drop(player);
        let afresh = opened(MorsePlayer::probing(counted_probe, false));
        if began.elapsed() < REOPEN_AFTER {
            assert_eq!(refused, Err(LOOPING.to_string()), "refused while it loops");
            assert_eq!(sent, Ok(()), "the send plays");
            assert!(kept, "on the stream that came out of its loop");
            assert!(
                afresh
                    .as_ref()
                    .is_some_and(|afresh| afresh.stream.is_some()),
                "a player built afresh opened none"
            );
        }
        if let Some(mut afresh) = afresh {
            afresh.shutdown();
        }
    }

    #[test]
    fn the_paddle_sidetone_opens_a_stream_before_the_first_squeeze() {
        let Some(mut player) = player() else {
            return;
        };
        assert!(player.stream.is_some(), "opened with the player");
        for on in [false, true, false] {
            player
                .set_live_tone(ToneRequest::new(on, 600.0, 0.2))
                .expect("keyed");
        }
        assert!(player.stream.is_some(), "and kept through the squeeze");
        player.shutdown();
    }
}
