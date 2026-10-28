//! The player's bookkeeping, with no device attached.
//!
//! The epoch keeps an old waiter from claiming a new send's ending; stopping
//! the sound itself is the mixer's business, send by send.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cw_core::TrainingSettings;
use cw_core::band::{BandChange, BandKey};

use crate::audio::epoch::{Epoch, Ticket};
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
pub const SILENT_AFTER: Duration = Duration::from_millis(1_500);

/// Whether a stream whose callback has counted `beat` calls, last `seen`,
/// has gone silent by `now`: not called since it was last seen to be, for
/// longer than [`SILENT_AFTER`]. A count that has moved is seen afresh.
pub fn went_silent(seen: &mut (u32, Instant), beat: u32, now: Instant) -> bool {
    if beat != seen.0 {
        *seen = (beat, now);
        return false;
    }
    now.saturating_duration_since(seen.1) > SILENT_AFTER
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
        timing: SendTiming,
    ) -> Self {
        Self {
            ticket,
            finished,
            late,
            stream_failed,
            timing,
        }
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
            failed: self.stream_failed.load(Ordering::Acquire),
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

    /// A stream is taken for broken once it has not been called for longer
    /// than a host's longest buffer and then some, counted from when it was
    /// last seen to be called; never for a long gap between looks at one
    /// that was called in it.
    #[test]
    fn a_stream_left_uncalled_has_gone_silent() {
        let opened = Instant::now();
        let at = |sec: f64| opened + Duration::from_secs_f64(sec);
        let mut seen = (0, opened);
        assert!(!went_silent(&mut seen, 0, at(1.0)));
        assert!(went_silent(&mut seen, 0, at(1.6)));
        let mut seen = (0, opened);
        assert!(!went_silent(&mut seen, 7, at(1.0)), "it was called");
        assert!(!went_silent(&mut seen, 7, at(2.4)));
        assert!(went_silent(&mut seen, 7, at(2.6)));
        let mut seen = (7, opened);
        assert!(
            !went_silent(&mut seen, 9_000, at(600.0)),
            "called all along"
        );
        // The count wraps; a wrapped count has moved all the same.
        let mut seen = (u32::MAX, opened);
        assert!(!went_silent(&mut seen, u32::MAX.wrapping_add(1), at(9.0)));
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
