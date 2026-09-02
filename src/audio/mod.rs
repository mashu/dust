#[cfg(feature = "desktop")]
mod native;
#[cfg(feature = "web")]
mod web;

#[cfg(feature = "desktop")]
pub use native::MorsePlayer;
#[cfg(feature = "web")]
pub use web::MorsePlayer;

use crate::time::{sleep_ms, POLL_MS};

#[cfg(feature = "web")]
const PLAYBACK_TAIL_MS: u32 = 24;

#[cfg(feature = "web")]
use std::cell::Cell;
#[cfg(feature = "web")]
use std::rc::Rc;
#[cfg(feature = "desktop")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(feature = "desktop")]
use std::sync::Arc;

/// Morse already scheduled; wait without holding the player `RefCell`.
pub struct PlaybackWait {
    pub duration_sec: f64,
    pub char_wpm: f64,
    #[cfg(feature = "web")]
    stop_flag: Rc<Cell<bool>>,
    #[cfg(feature = "web")]
    epoch: u64,
    #[cfg(feature = "web")]
    current_epoch: Rc<Cell<u64>>,
    #[cfg(feature = "desktop")]
    stop_flag: Arc<AtomicBool>,
    #[cfg(feature = "desktop")]
    epoch: u64,
    #[cfg(feature = "desktop")]
    current_epoch: Arc<AtomicU64>,
    #[cfg(feature = "desktop")]
    finished: Arc<AtomicBool>,
}

impl PlaybackWait {
    #[cfg(feature = "web")]
    pub(crate) fn web(
        duration_sec: f64,
        char_wpm: f64,
        stop_flag: Rc<Cell<bool>>,
        epoch: u64,
        current_epoch: Rc<Cell<u64>>,
    ) -> Self {
        Self {
            duration_sec,
            char_wpm,
            stop_flag,
            epoch,
            current_epoch,
        }
    }

    #[cfg(feature = "desktop")]
    pub(crate) fn desktop(
        duration_sec: f64,
        char_wpm: f64,
        stop_flag: Arc<AtomicBool>,
        epoch: u64,
        current_epoch: Arc<AtomicU64>,
        finished: Arc<AtomicBool>,
    ) -> Self {
        Self {
            duration_sec,
            char_wpm,
            stop_flag,
            epoch,
            current_epoch,
            finished,
        }
    }

    pub async fn wait(self) {
        #[cfg(feature = "web")]
        {
            let mut left =
                ((self.duration_sec * 1000.0).ceil() as u32).saturating_add(PLAYBACK_TAIL_MS);
            while left > 0 {
                if self.stop_flag.get() || self.current_epoch.get() != self.epoch {
                    break;
                }
                let chunk = left.min(POLL_MS);
                sleep_ms(chunk).await;
                left = left.saturating_sub(chunk);
            }
        }
        #[cfg(feature = "desktop")]
        {
            while !self.finished.load(Ordering::SeqCst)
                && !self.stop_flag.load(Ordering::SeqCst)
                && self.current_epoch.load(Ordering::SeqCst) == self.epoch
            {
                sleep_ms(POLL_MS).await;
            }
        }
    }
}

pub fn focus_group_input(index: usize) {
    let js = format!(
        r#"(() => {{
            const el = document.getElementById("group-input-{index}");
            if (!el || el.disabled) {{
                return;
            }}
            if (document.activeElement !== el) {{
                el.focus({{ preventScroll: true }});
            }}
        }})()"#
    );
    let _ = dioxus::document::eval(&js);
}
