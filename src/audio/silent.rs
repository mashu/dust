//! Audio-less player for builds with no output backend.
//!
//! It runs the same [`cw_core::PlaybackPlan`] the real backends do and reports
//! the same duration and resolved speeds, so a session keeps its Farnsworth
//! pacing, timing and scoring — it just never makes a sound.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use cw_core::{plan_morse_playback, FastrandRng, TrainingSettings};

use super::{MorseBackend, PlaybackSignal, PlaybackWait, WaitFlags};

pub struct MorsePlayer {
    epoch: Rc<Cell<u64>>,
}

struct SilentSignal {
    epoch: Rc<Cell<u64>>,
    mine: u64,
    started: Instant,
}

impl PlaybackSignal for SilentSignal {
    fn poll(&self) -> WaitFlags {
        let elapsed = self.started.elapsed().as_millis();
        WaitFlags {
            cancelled: self.epoch.get() != self.mine,
            finished: false,
            failed: false,
            // There is no audio to park, so a send always runs to its length.
            suspended: false,
            // Nothing is playing, so the wall clock is the only clock there is.
            played_ms: Some(elapsed.min(u128::from(u32::MAX)) as u32),
        }
    }
}

impl MorsePlayer {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            epoch: Rc::new(Cell::new(0)),
        })
    }
}

impl MorseBackend for MorsePlayer {
    fn apply_band(&mut self, _settings: &TrainingSettings) -> Result<(), String> {
        Ok(())
    }

    fn start_text(
        &mut self,
        text: &str,
        settings: &TrainingSettings,
        rng: &mut FastrandRng,
    ) -> Result<PlaybackWait, String> {
        let plan = plan_morse_playback(text, settings, rng);
        let mine = self.epoch.get().wrapping_add(1);
        self.epoch.set(mine);
        Ok(PlaybackWait::new(
            plan.duration_sec,
            plan.resolved_char_wpm,
            plan.resolved_effective_wpm,
            Rc::new(SilentSignal {
                epoch: Rc::clone(&self.epoch),
                mine,
                started: Instant::now(),
            }),
        ))
    }

    fn stop(&mut self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
    }

    fn shutdown(&mut self) {
        self.stop();
    }
}
