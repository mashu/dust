#[cfg(feature = "desktop")]
mod native;
#[cfg(feature = "web")]
mod web;

#[cfg(feature = "desktop")]
pub use native::MorsePlayer;
#[cfg(feature = "web")]
pub use web::MorsePlayer;

pub use crate::time::{local_date_string, now_ms};

use crate::time::sleep_ms;

#[cfg(feature = "web")]
use std::cell::Cell;
#[cfg(feature = "web")]
use std::rc::Rc;
#[cfg(feature = "desktop")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "desktop")]
use std::sync::Arc;

/// Morse already scheduled; wait without holding the player `RefCell`.
pub struct PlaybackWait {
    pub duration_sec: f64,
    pub char_wpm: f64,
    #[cfg(feature = "web")]
    stop_flag: Rc<Cell<bool>>,
    #[cfg(feature = "desktop")]
    stop_flag: Arc<AtomicBool>,
    #[cfg(feature = "desktop")]
    finished: Arc<AtomicBool>,
}

impl PlaybackWait {
    #[cfg(feature = "web")]
    pub(crate) fn web(duration_sec: f64, char_wpm: f64, stop_flag: Rc<Cell<bool>>) -> Self {
        Self {
            duration_sec,
            char_wpm,
            stop_flag,
        }
    }

    #[cfg(feature = "desktop")]
    pub(crate) fn desktop(
        duration_sec: f64,
        char_wpm: f64,
        stop_flag: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
    ) -> Self {
        Self {
            duration_sec,
            char_wpm,
            stop_flag,
            finished,
        }
    }

    pub async fn wait(self) {
        #[cfg(feature = "web")]
        {
            let wait_ms = ((self.duration_sec * 1000.0).ceil() as u32).saturating_add(60);
            let steps = wait_ms.div_ceil(50).max(1);
            for _ in 0..steps {
                if self.stop_flag.get() {
                    break;
                }
                sleep_ms(50).await;
            }
        }
        #[cfg(feature = "desktop")]
        {
            while !self.finished.load(Ordering::SeqCst) && !self.stop_flag.load(Ordering::SeqCst) {
                sleep_ms(20).await;
            }
        }
    }
}

pub fn focus_group_input(index: usize) {
    let js = format!(
        r#"(() => {{
            const el = document.getElementById("group-input-{index}");
            if (el) {{
                el.focus();
                el.scrollIntoView({{ block: "center", behavior: "smooth" }});
            }}
        }})()"#
    );
    let _ = dioxus::document::eval(&js);
}
