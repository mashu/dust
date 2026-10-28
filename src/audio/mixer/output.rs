//! The native stream's side of the mixer: what each audio callback plays,
//! the paddle's sidetone and the limiter over the lot. The browser has the
//! limiter and the sidetone in its graph instead.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};

use cw_core::band::soft_limit;

use super::Mixer;
use super::sidetone::{self, Sidetone, SidetoneControl};

/// The stream's side of the mixer: what each callback plays.
pub struct Output {
    mixer: Arc<Mutex<Mixer>>,
    control: Arc<SidetoneControl>,
    sidetone: Sidetone,
    /// The stream's word to the player that something broke.
    failed: Arc<AtomicBool>,
    /// The mixer panicked once; it is not run again on this stream, where it
    /// would only panic on every buffer that followed.
    abandoned: bool,
}

impl Output {
    pub fn new(
        mixer: Arc<Mutex<Mixer>>,
        control: Arc<SidetoneControl>,
        failed: Arc<AtomicBool>,
        sample_rate: u32,
    ) -> Self {
        Self {
            mixer,
            control,
            sidetone: Sidetone::new(sample_rate),
            failed,
            abandoned: false,
        }
    }

    /// One callback's worth of mono frames: the mixer, at its volume, the
    /// paddle's sidetone, set at the level it is heard at, and the limiter
    /// over the lot, `latency` frames ahead of the speaker.
    ///
    /// Never waits. A callback that finds the UI holding the mixer — it only
    /// ever does to swap a band or a send in — goes without the mixer for
    /// that one buffer; the sidetone needs no lock and plays regardless.
    pub fn fill(&mut self, out: &mut [f32], latency: usize) {
        self.fill_at(out, latency, sidetone::clock_sec());
    }

    /// [`Self::fill`], for a callback that came at `now` on the sidetone's
    /// clock.
    pub fn fill_at(&mut self, out: &mut [f32], latency: usize, now: f64) {
        out.fill(0.0);
        if !self.abandoned {
            let guard = match self.mixer.try_lock() {
                Ok(guard) => Some(guard),
                // A panic on the UI side cannot leave the mixer half-written:
                // every change to it is a move into a field.
                Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) => None,
            };
            if let Some(mut mixer) = guard
                && catch_unwind(AssertUnwindSafe(|| mixer.render(out, latency))).is_err()
            {
                // A send waiting on this stream fails, and the session
                // rebuilds the player rather than sitting in silence.
                self.abandoned = true;
                self.failed.store(true, Ordering::Release);
                out.fill(0.0);
            }
        }
        self.sidetone.add_to(out, &self.control, now);
        for sample in out {
            *sample = finish(*sample);
        }
    }
}

/// The output stage: the receiver's soft limiter, once, on everything played
/// together — band, Morse and sidetone — so a crash under a loud pile-up is
/// caught rather than squared off. Anything that is not a number is silence.
pub fn finish(sample: f32) -> f32 {
    if sample.is_finite() {
        (soft_limit(f64::from(sample)) as f32).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}
