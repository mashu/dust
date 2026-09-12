//! Audio-less player for builds with no output backend (Android, for now).
//!
//! It runs the same [`PlaybackPlan`] the real backends do and reports the same
//! duration and resolved speeds, so a session keeps its Farnsworth pacing,
//! timing and scoring — it just never makes a sound.

use std::cell::Cell;
use std::rc::Rc;

use cw_core::{plan_morse_playback, Rng, TrainingSettings};

use super::PlaybackWait;

pub struct MorsePlayer {
    stop_flag: Rc<Cell<bool>>,
    epoch: Rc<Cell<u64>>,
}

impl MorsePlayer {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            stop_flag: Rc::new(Cell::new(false)),
            epoch: Rc::new(Cell::new(0)),
        })
    }

    pub fn resume_from_gesture(&mut self) {}

    pub fn apply_band(&mut self, _settings: &TrainingSettings) -> Result<(), String> {
        Ok(())
    }

    pub fn start_text(
        &mut self,
        text: &str,
        settings: &TrainingSettings,
        rng: &mut impl Rng,
    ) -> Result<PlaybackWait, String> {
        let plan = plan_morse_playback(text, settings, rng);
        self.stop_flag.set(false);
        let epoch = self.epoch.get().wrapping_add(1);
        self.epoch.set(epoch);
        Ok(PlaybackWait::polled(
            plan.duration_sec,
            plan.resolved_char_wpm,
            plan.resolved_effective_wpm,
            self.stop_flag.clone(),
            epoch,
            self.epoch.clone(),
        ))
    }

    pub fn stop(&mut self) {
        self.stop_flag.set(true);
        self.epoch.set(self.epoch.get().wrapping_add(1));
    }

    pub fn shutdown(&mut self) {
        self.stop();
    }
}
