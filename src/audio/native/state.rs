//! The player's bookkeeping, with no device attached.
//!
//! The epoch keeps an old waiter from claiming a new send's ending; stopping
//! the sound itself is the mixer's business, send by send.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cw_core::TrainingSettings;
use cw_core::band::{BandChange, BandKey};

use crate::audio::epoch::{Epoch, Ticket};
use crate::audio::mixer::clock_sec;
use crate::audio::{PlaybackSignal, SendTiming, WaitFlags};

pub struct PlayerState {
    epoch: Epoch,
    /// What the band playing now was built from.
    band: Option<BandKey>,
}

impl PlayerState {
    pub fn new() -> Self {
        Self {
            epoch: Epoch::default(),
            band: None,
        }
    }

    /// Whether the band has to be retuned or rebuilt for these settings.
    /// The fading is not part of it: it is applied to the Morse, a send at a
    /// time.
    pub fn band_change(&self, settings: &TrainingSettings) -> BandChange {
        BandKey::of(&settings.band).change_from(self.band.as_ref())
    }

    pub fn note_band(&mut self, settings: &TrainingSettings) {
        self.band = Some(BandKey::of(&settings.band));
    }

    pub fn forget_band(&mut self) {
        self.band = None;
    }

    /// Claim the audio for a new send.
    pub fn arm(&mut self) -> Ticket {
        self.epoch.arm()
    }

    /// Retire the current send, stopped or never started, so nothing can
    /// mistake it for the current one.
    pub fn retire(&mut self) {
        self.epoch.bump();
    }
}

/// How long an open stream may go uncalled before it is taken for broken:
/// longer than the longest buffer a host's default gives, a third of a
/// second or so, with room for a busy machine.
pub const SILENT_AFTER_SEC: f64 = 1.5;

/// How long a send waiting on a stream sees it uncalled before it says the
/// stream broke: a process the system froze and has just let go is called
/// again within a buffer or two, and a stream that stopped stays stopped.
const SILENT_CONFIRM_SEC: f64 = 0.25;

/// When a stream's callback last ran, in seconds on [`clock_sec`]'s clock:
/// what tells a stream stopped without a word, as an iOS interruption stops
/// one, from one that plays. Opened, it counts as called, so one never
/// called at all is given as long as one that stopped.
pub struct Heartbeat(AtomicU64);

impl Heartbeat {
    pub fn new(now_sec: f64) -> Self {
        Self(AtomicU64::new(now_sec.to_bits()))
    }

    /// The callback ran at `now_sec`.
    pub fn beat(&self, now_sec: f64) {
        self.0.store(now_sec.to_bits(), Ordering::Relaxed);
    }

    /// When the callback last ran.
    #[cfg(test)]
    pub fn last(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    /// Whether, at `now_sec`, the callback has gone uncalled for longer than
    /// [`SILENT_AFTER_SEC`], however long since anything last looked.
    pub fn went_silent(&self, now_sec: f64) -> bool {
        now_sec - f64::from_bits(self.0.load(Ordering::Relaxed)) > SILENT_AFTER_SEC
    }
}

/// Whether a stream opened on the device named `opened` should move to the
/// default now named `default`: only when both names are known and differ,
/// so a device that cannot say its name never has the stream reopened over
/// and over.
pub fn moved_off_default(opened: Option<&str>, default: Option<&str>) -> bool {
    matches!((opened, default), (Some(opened), Some(default)) if opened != default)
}

/// What the stream tells a waiter.
pub struct ToneSignal {
    ticket: Ticket,
    finished: Arc<AtomicBool>,
    /// The send's word that it played late ([`crate::audio::WaitFlags::late`]).
    late: Arc<AtomicBool>,
    /// The word of the stream the send was started on that it broke. A send
    /// is never moved to another stream: one that replaces a broken stream
    /// may run at another rate.
    stream_failed: Arc<AtomicBool>,
    /// That stream's heartbeat, and since when this waiter has seen it gone
    /// silent: a stream stopped without a word is told of a moment after it
    /// stopped, not at the end of the stall the send is allowed.
    heartbeat: Arc<Heartbeat>,
    silent_since: Cell<Option<f64>>,
    /// How long it waits to be rendered, its first sample going to the
    /// output, and how far behind the stream's callback the speaker was as
    /// it started.
    timing: SendTiming,
}

impl ToneSignal {
    pub fn new(
        ticket: Ticket,
        finished: Arc<AtomicBool>,
        late: Arc<AtomicBool>,
        stream_failed: Arc<AtomicBool>,
        heartbeat: Arc<Heartbeat>,
        timing: SendTiming,
    ) -> Self {
        Self {
            ticket,
            finished,
            late,
            stream_failed,
            heartbeat,
            silent_since: Cell::new(None),
            timing,
        }
    }

    /// Whether the stream has broken by `now_sec`: said so, or seen silent
    /// for [`SILENT_CONFIRM_SEC`] on end, which it then says for the player
    /// to open another.
    fn stream_broke_at(&self, now_sec: f64) -> bool {
        if self.stream_failed.load(Ordering::Acquire) {
            return true;
        }
        if !self.heartbeat.went_silent(now_sec) {
            self.silent_since.set(None);
            return false;
        }
        let since = self.silent_since.get().unwrap_or(now_sec);
        self.silent_since.set(Some(since));
        let broke = now_sec - since >= SILENT_CONFIRM_SEC;
        if broke {
            self.stream_failed.store(true, Ordering::Release);
        }
        broke
    }
}

impl PlaybackSignal for ToneSignal {
    fn timing(&self) -> SendTiming {
        self.timing
    }

    fn poll(&self) -> WaitFlags {
        WaitFlags {
            cancelled: self.ticket.is_cancelled(),
            finished: self.finished.load(Ordering::Acquire),
            // A broken stream never calls back again, so the send would never
            // finish; saying so lets the session rebuild the player.
            failed: self.stream_broke_at(clock_sec()),
            // The stream either calls back or it does not; there is nothing
            // here that parks it the way a browser parks a hidden tab.
            suspended: false,
            // cpal gives no clock of its own: the send having been heard to
            // its end is the only word on when it is over, and the send says
            // itself whether it played late.
            played_ms: None,
            late: self.late.load(Ordering::Acquire),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_moves_only_when_the_default_does() {
        assert!(!moved_off_default(Some("Speakers"), Some("Speakers")));
        assert!(moved_off_default(Some("Speakers"), Some("Headphones")));
        assert!(!moved_off_default(None, Some("Headphones")));
        assert!(!moved_off_default(Some("Speakers"), None));
    }

    /// A stream is taken for broken once it has not been called for longer
    /// than a host's longest buffer and then some, counted from when it was
    /// last called, however long since anything looked: one that stopped
    /// minutes ago is known for stopped at the first look after.
    #[test]
    fn a_stream_left_uncalled_has_gone_silent() {
        let heartbeat = Heartbeat::new(10.0);
        assert!(!heartbeat.went_silent(11.0));
        assert!(heartbeat.went_silent(11.6), "never called since it opened");
        heartbeat.beat(11.7);
        assert!(!heartbeat.went_silent(13.0));
        assert!(heartbeat.went_silent(13.3));
        assert!(heartbeat.went_silent(300.0), "stopped long ago");
        heartbeat.beat(299.99);
        assert!(!heartbeat.went_silent(300.0), "called all along");
    }

    fn waiter(heartbeat: &Arc<Heartbeat>) -> ToneSignal {
        ToneSignal::new(
            Epoch::default().arm(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::clone(heartbeat),
            SendTiming::default(),
        )
    }

    /// A send waiting on a stream that stops calling is told it broke a
    /// moment after the stream is seen silent, and says so for the player;
    /// one called again in that moment, as a process let go after the
    /// system froze it is, never.
    #[test]
    fn a_waiter_is_told_of_a_stream_stopped_without_a_word() {
        let heartbeat = Arc::new(Heartbeat::new(0.0));
        let send = waiter(&heartbeat);
        assert!(!send.stream_broke_at(1.0));
        assert!(!send.stream_broke_at(1.6), "seen silent, not yet confirmed");
        assert!(send.stream_broke_at(1.6 + SILENT_CONFIRM_SEC));
        assert!(send.stream_failed.load(Ordering::Acquire), "and said so");
        let thawed = waiter(&Arc::new(Heartbeat::new(0.0)));
        assert!(!thawed.stream_broke_at(2.0));
        thawed.heartbeat.beat(2.1);
        assert!(!thawed.stream_broke_at(2.1 + SILENT_CONFIRM_SEC));
        assert!(!thawed.stream_broke_at(3.0));
    }

    #[test]
    fn the_background_is_rebuilt_only_when_its_settings_change() {
        let mut state = PlayerState::new();
        let settings = TrainingSettings::default();
        assert_eq!(state.band_change(&settings), BandChange::Rebuild);
        state.note_band(&settings);
        assert_eq!(state.band_change(&settings), BandChange::None);

        let mut louder = settings.clone();
        louder.band.qrn_level = 0.9;
        assert_eq!(state.band_change(&louder), BandChange::Rebuild);

        // The receiver's controls retune the band on the air.
        let mut narrower = settings.clone();
        narrower.band.filter_width_hz = 250.0;
        assert_eq!(state.band_change(&narrower), BandChange::Retune);

        // Speed is not a band setting.
        let mut faster = settings.clone();
        faster.playback.char_wpm_min = 40.0;
        assert_eq!(state.band_change(&faster), BandChange::None);

        // Nor is the fading: it plays on the Morse, a send at a time, and a
        // new band would only be a new draw of stations and static for
        // nothing.
        let mut fading = settings.clone();
        fading.band.qsb_depth = 0.9;
        fading.band.qsb_rate_hz = 0.5;
        assert_eq!(state.band_change(&fading), BandChange::None);
        fading.band.qsb_enabled = false;
        assert_eq!(state.band_change(&fading), BandChange::None);

        state.forget_band();
        assert_eq!(state.band_change(&settings), BandChange::Rebuild);
    }

    #[test]
    fn the_signal_reports_what_the_stream_is_doing() {
        let mut state = PlayerState::new();
        let armed = state.arm();
        let finished = Arc::new(AtomicBool::new(false));
        let broken = Arc::new(AtomicBool::new(false));
        let late = Arc::new(AtomicBool::new(false));
        let timing = SendTiming {
            lead_sec: 0.0,
            latency_sec: 0.04,
        };
        let signal = ToneSignal::new(
            armed,
            Arc::clone(&finished),
            Arc::clone(&late),
            Arc::clone(&broken),
            Arc::new(Heartbeat::new(clock_sec())),
            timing,
        );
        assert_eq!(signal.timing().heard_after_sec(), 0.04);
        assert_eq!(
            signal.poll(),
            WaitFlags {
                cancelled: false,
                finished: false,
                failed: false,
                suspended: false,
                played_ms: None,
                late: false,
            }
        );

        late.store(true, Ordering::Release);
        assert!(signal.poll().late, "the send's word that it played late");
        finished.store(true, Ordering::Release);
        assert!(signal.poll().finished);

        broken.store(true, Ordering::Release);
        assert!(
            signal.poll().failed,
            "a broken device has to reach the waiter"
        );

        state.retire();
        assert!(signal.poll().cancelled);
    }
}
