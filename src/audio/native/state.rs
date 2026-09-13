//! The player's bookkeeping, with no device attached.
//!
//! One stop flag and one epoch are shared by every send the player has ever
//! made. The epoch is what keeps an old waiter from claiming a new send's
//! ending: a send is only current while the epoch it was armed with is the
//! player's own.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use cw_core::TrainingSettings;

use crate::audio::{PlaybackSignal, WaitFlags};

pub struct PlayerState {
    stop: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    band_signature: Option<String>,
}

/// A send that has been given the current epoch.
#[derive(Clone)]
pub struct ArmedSend {
    stop: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    mine: u64,
}

impl ArmedSend {
    pub fn stop_flag(&self) -> &Arc<AtomicBool> {
        &self.stop
    }

    /// True once this send is no longer the player's current one.
    pub fn superseded(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.epoch.load(Ordering::SeqCst) != self.mine
    }
}

impl PlayerState {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            epoch: Arc::new(AtomicU64::new(0)),
            band_signature: None,
        }
    }

    /// Whether the background layers have to be rebuilt for these settings.
    pub fn band_needs_rebuild(&self, settings: &TrainingSettings) -> bool {
        self.band_signature.as_deref() != Some(settings.band_signature().as_str())
    }

    pub fn note_band(&mut self, settings: &TrainingSettings) {
        self.band_signature = Some(settings.band_signature());
    }

    pub fn forget_band(&mut self) {
        self.band_signature = None;
    }

    /// Claim the audio for a new send.
    pub fn arm(&mut self) -> ArmedSend {
        let mine = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.stop.store(false, Ordering::SeqCst);
        ArmedSend {
            stop: Arc::clone(&self.stop),
            epoch: Arc::clone(&self.epoch),
            mine,
        }
    }

    /// A send that never got off the ground: retire its epoch so nothing can
    /// mistake it for the current one.
    pub fn note_start_failed(&mut self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub fn stop(&mut self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// What the tone stream tells a waiter.
pub struct ToneSignal {
    armed: ArmedSend,
    finished: Arc<AtomicBool>,
}

impl ToneSignal {
    pub fn new(armed: ArmedSend, finished: Arc<AtomicBool>) -> Self {
        Self { armed, finished }
    }
}

impl PlaybackSignal for ToneSignal {
    fn poll(&self) -> WaitFlags {
        WaitFlags {
            cancelled: self.armed.superseded(),
            finished: self.finished.load(Ordering::SeqCst),
            failed: false,
            // The stream either calls back or it does not; there is nothing
            // here that parks it the way a browser parks a hidden tab.
            suspended: false,
            // cpal gives no clock of its own: the callback running out of
            // samples is the only word on when a send is over.
            played_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_send_is_current_until_something_else_claims_the_audio() {
        let mut state = PlayerState::new();
        let first = state.arm();
        assert!(!first.superseded());

        let second = state.arm();
        assert!(
            first.superseded(),
            "the first send should have been retired"
        );
        assert!(!second.superseded());

        state.stop();
        assert!(second.superseded());
    }

    #[test]
    fn arming_clears_the_stop_flag_the_last_stop_left_behind() {
        let mut state = PlayerState::new();
        let first = state.arm();
        state.stop();
        assert!(first.stop_flag().load(Ordering::SeqCst));

        let second = state.arm();
        assert!(!second.stop_flag().load(Ordering::SeqCst));
        // The stopped send stays retired even though the flag was cleared.
        assert!(first.superseded());
        assert!(!second.superseded());
    }

    #[test]
    fn a_send_that_never_starts_is_retired_too() {
        let mut state = PlayerState::new();
        let armed = state.arm();
        state.note_start_failed();
        assert!(armed.superseded());
    }

    #[test]
    fn the_background_is_rebuilt_only_when_its_settings_change() {
        let mut state = PlayerState::new();
        let settings = TrainingSettings::default();
        assert!(state.band_needs_rebuild(&settings));
        state.note_band(&settings);
        assert!(!state.band_needs_rebuild(&settings));

        let mut louder = settings.clone();
        louder.band.qrn_level = 0.9;
        assert!(state.band_needs_rebuild(&louder));

        // Speed is not a band setting.
        let mut faster = settings.clone();
        faster.playback.char_wpm_min = 40.0;
        assert!(!state.band_needs_rebuild(&faster));

        state.forget_band();
        assert!(state.band_needs_rebuild(&settings));
    }

    #[test]
    fn the_signal_reports_what_the_stream_is_doing() {
        let mut state = PlayerState::new();
        let armed = state.arm();
        let finished = Arc::new(AtomicBool::new(false));
        let signal = ToneSignal::new(armed, Arc::clone(&finished));
        assert_eq!(
            signal.poll(),
            WaitFlags {
                cancelled: false,
                finished: false,
                failed: false,
                suspended: false,
                played_ms: None,
            }
        );

        finished.store(true, Ordering::SeqCst);
        assert!(signal.poll().finished);

        state.stop();
        assert!(signal.poll().cancelled);
    }
}
