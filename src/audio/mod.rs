#[cfg(feature = "native-audio")]
mod native;
#[cfg(feature = "silent-audio")]
mod silent;
#[cfg(feature = "web")]
mod web;

#[cfg(feature = "native-audio")]
pub use native::MorsePlayer;
#[cfg(feature = "silent-audio")]
pub use silent::MorsePlayer;
#[cfg(feature = "web")]
pub use web::MorsePlayer;

/// True when the build has no audio output and only simulates the timing of a
/// session — the Android build, for now.
pub const AUDIO_IS_SILENT: bool = cfg!(feature = "silent-audio");

use crate::time::{sleep_ms, POLL_MS};

#[cfg(any(feature = "web", feature = "silent-audio"))]
const PLAYBACK_TAIL_MS: u32 = 24;

#[cfg(any(feature = "web", feature = "silent-audio"))]
use std::cell::Cell;
#[cfg(any(feature = "web", feature = "silent-audio"))]
use std::rc::Rc;
#[cfg(feature = "native-audio")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(feature = "native-audio")]
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackOutcome {
    Completed,
    Cancelled,
}

/// Morse already scheduled; wait without holding the player `RefCell`.
pub struct PlaybackWait {
    pub duration_sec: f64,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    #[cfg(any(feature = "web", feature = "silent-audio"))]
    stop_flag: Rc<Cell<bool>>,
    #[cfg(any(feature = "web", feature = "silent-audio"))]
    epoch: u64,
    #[cfg(any(feature = "web", feature = "silent-audio"))]
    current_epoch: Rc<Cell<u64>>,
    #[cfg(feature = "native-audio")]
    stop_flag: Arc<AtomicBool>,
    #[cfg(feature = "native-audio")]
    epoch: u64,
    #[cfg(feature = "native-audio")]
    current_epoch: Arc<AtomicU64>,
    #[cfg(feature = "native-audio")]
    finished: Arc<AtomicBool>,
}

impl PlaybackWait {
    #[cfg(any(feature = "web", feature = "silent-audio"))]
    pub(crate) fn polled(
        duration_sec: f64,
        char_wpm: f64,
        effective_wpm: f64,
        stop_flag: Rc<Cell<bool>>,
        epoch: u64,
        current_epoch: Rc<Cell<u64>>,
    ) -> Self {
        Self {
            duration_sec,
            char_wpm,
            effective_wpm,
            stop_flag,
            epoch,
            current_epoch,
        }
    }

    #[cfg(feature = "native-audio")]
    pub(crate) fn desktop(
        duration_sec: f64,
        char_wpm: f64,
        effective_wpm: f64,
        stop_flag: Arc<AtomicBool>,
        epoch: u64,
        current_epoch: Arc<AtomicU64>,
        finished: Arc<AtomicBool>,
    ) -> Self {
        Self {
            duration_sec,
            char_wpm,
            effective_wpm,
            stop_flag,
            epoch,
            current_epoch,
            finished,
        }
    }

    pub async fn wait(self) -> PlaybackOutcome {
        #[cfg(any(feature = "web", feature = "silent-audio"))]
        {
            let mut left =
                ((self.duration_sec * 1000.0).ceil() as u32).saturating_add(PLAYBACK_TAIL_MS);
            while left > 0 {
                if self.stop_flag.get() || self.current_epoch.get() != self.epoch {
                    return PlaybackOutcome::Cancelled;
                }
                let chunk = left.min(POLL_MS);
                sleep_ms(chunk).await;
                left = left.saturating_sub(chunk);
            }
            if self.stop_flag.get() || self.current_epoch.get() != self.epoch {
                PlaybackOutcome::Cancelled
            } else {
                PlaybackOutcome::Completed
            }
        }
        #[cfg(feature = "native-audio")]
        {
            let hang_ms = ((self.duration_sec * 1000.0).ceil() as u32).saturating_add(2_000);
            let mut waited = 0u32;
            while !self.finished.load(Ordering::SeqCst)
                && !self.stop_flag.load(Ordering::SeqCst)
                && self.current_epoch.load(Ordering::SeqCst) == self.epoch
            {
                if waited >= hang_ms {
                    break;
                }
                sleep_ms(POLL_MS).await;
                waited = waited.saturating_add(POLL_MS);
            }
            let stopped = self.stop_flag.load(Ordering::SeqCst)
                || self.current_epoch.load(Ordering::SeqCst) != self.epoch;
            if stopped {
                PlaybackOutcome::Cancelled
            } else {
                PlaybackOutcome::Completed
            }
        }
    }
}

pub fn focus_group_input(index: usize) {
    let js = format!(
        r#"(() => {{
            const cardId = "group-card-{index}";
            const inputId = "group-input-{index}";
            let scrolled = false;
            const apply = () => {{
                const card = document.getElementById(cardId);
                if (card && !scrolled) {{
                    card.scrollIntoView({{ behavior: "smooth", block: "center", inline: "nearest" }});
                    scrolled = true;
                }}
                const el = document.getElementById(inputId);
                if (!el || el.disabled) {{
                    return;
                }}
                if (document.activeElement !== el) {{
                    el.focus({{ preventScroll: true }});
                }}
            }};
            apply();
            requestAnimationFrame(() => {{
                apply();
                setTimeout(apply, 50);
                setTimeout(apply, 180);
            }});
        }})()"#
    );
    let _ = dioxus::document::eval(&js);
}
