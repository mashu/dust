use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cw_core::{
    resolve_station, CharSamplingState, FastrandRng, GroupSession, SessionMachine, SessionResult,
    StationVoice, TrainingSettings,
};
use dioxus::prelude::*;

use crate::audio::MorseBackend;
use crate::time::seed_rng;

/// A factory for the audio backend. Injectable so the session runtime can be
/// driven without a sound card.
pub type BackendFactory = Rc<dyn Fn() -> Result<Box<dyn MorseBackend>, String>>;

/// Everything a session writes back into the app: the screen it is on, the
/// session in flight, the history it joins, and the messages it raises.
/// Signals are cheap to copy, so this travels by value.
#[derive(Clone, Copy)]
pub struct SessionSignals {
    pub screen: Signal<Screen>,
    pub runtime: Signal<Option<GroupSession>>,
    pub result: Signal<Option<SessionResult>>,
    pub auto_message: Signal<Option<String>>,
    pub sessions: Signal<Vec<SessionResult>>,
    pub settings: Signal<TrainingSettings>,
    pub toast: Signal<Option<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Settings,
    Training,
    Results,
    Stats,
    Listen,
}

#[derive(Clone)]
pub struct AppState {
    pub session_gen: Rc<Cell<u64>>,
    pub player: Rc<RefCell<Option<Box<dyn MorseBackend>>>>,
    pub rng: Rc<RefCell<FastrandRng>>,
    pub sampling: Rc<RefCell<CharSamplingState>>,
    pub machine: Rc<RefCell<Option<SessionMachine>>>,
    make_player: BackendFactory,
}

impl AppState {
    pub fn new() -> Self {
        Self::with_backend(Rc::new(crate::audio::default_backend))
    }

    pub fn with_backend(make_player: BackendFactory) -> Self {
        Self {
            session_gen: Rc::new(Cell::new(0)),
            player: Rc::new(RefCell::new(None)),
            rng: Rc::new(RefCell::new(FastrandRng(seed_rng()))),
            sampling: Rc::new(RefCell::new(CharSamplingState::default())),
            machine: Rc::new(RefCell::new(None)),
            make_player,
        }
    }

    pub fn bump_session(&self) -> u64 {
        let next = self.session_gen.get() + 1;
        self.session_gen.set(next);
        *self.machine.borrow_mut() = None;
        next
    }

    fn ensure_player(&self, settings: &TrainingSettings) -> Result<(), String> {
        let mut slot = self
            .player
            .try_borrow_mut()
            .map_err(|_| "Audio is busy.".to_string())?;
        if slot.is_none() {
            *slot = Some((self.make_player)()?);
        }
        if let Some(player) = slot.as_mut() {
            player.resume_from_gesture();
            player.apply_band(settings)?;
        }
        Ok(())
    }

    pub(crate) fn rebuild_player(&self, settings: &TrainingSettings) -> Result<(), String> {
        if let Ok(mut slot) = self.player.try_borrow_mut() {
            if let Some(player) = slot.as_mut() {
                player.shutdown();
            }
            *slot = None;
        } else {
            return Err("Audio is busy.".into());
        }
        self.ensure_player(settings)
    }

    /// Stop current audio, invalidate waiters, then arm the player for a new gen.
    pub fn takeover_audio(&self, settings: &TrainingSettings) -> Result<u64, String> {
        self.stop_sending();
        let gen = self.bump_session();
        self.ensure_player(settings)?;
        Ok(gen)
    }

    /// Tune in a station, drawn from the app's own generator.
    pub fn station(&self, settings: &TrainingSettings) -> StationVoice {
        match self.rng.try_borrow_mut() {
            Ok(mut rng) => resolve_station(settings, &mut *rng),
            // Only reachable if a send were started from inside another; the
            // point is a usable voice, not which one.
            Err(_) => resolve_station(settings, &mut FastrandRng(crate::time::seed_rng())),
        }
    }

    pub fn apply_band_live(&self, settings: &TrainingSettings) {
        if let Ok(mut slot) = self.player.try_borrow_mut() {
            if let Some(player) = slot.as_mut() {
                let _ = player.apply_band(settings);
            }
        }
    }

    /// Stop whatever is being sent, and leave the receiver running. Between
    /// groups the background is meant to keep hissing — a real receiver does
    /// not go silent because the other station stopped keying.
    pub fn stop_sending(&self) {
        if let Ok(mut slot) = self.player.try_borrow_mut() {
            if let Some(player) = slot.as_mut() {
                player.stop();
            }
        }
    }

    /// Everything off, receiver included. What "stop" means when the user
    /// pressed it, or walked away from the screen that was making the sound.
    pub fn silence_audio(&self) {
        if let Ok(mut slot) = self.player.try_borrow_mut() {
            if let Some(player) = slot.as_mut() {
                player.shutdown();
            }
        }
    }

    /// Paddle sidetone: on for the squeeze, off on release. No-op if nothing
    /// has opened the player yet.
    pub fn set_live_tone(&self, on: bool, frequency_hz: f64, gain: f64) {
        if let Ok(mut slot) = self.player.try_borrow_mut() {
            if let Some(player) = slot.as_mut() {
                player.set_live_tone(on, frequency_hz, gain);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::audio::fake::Call;
    use crate::testing::{run, test_settings, Harness};

    #[test]
    fn taking_the_audio_over_stops_what_was_playing_and_moves_the_generation_on() {
        run(|| async {
            let h = Harness::new();
            let settings = test_settings();
            let first = h.app.takeover_audio(&settings).expect("player");
            assert_eq!(first, 1);
            assert!(h.calls().contains(&Call::New));

            h.recorder.clear();
            let second = h.app.takeover_audio(&settings).expect("player");
            assert_eq!(second, 2);
            assert!(h.calls().contains(&Call::Stop));
            // The same player is reused: taking over is not a rebuild.
            assert_eq!(h.recorder.players_built.get(), 1);
        });
    }

    #[test]
    fn a_player_that_will_not_open_is_reported_rather_than_ignored() {
        run(|| async {
            let h = Harness::new();
            h.recorder.build_error.set(true);
            let err = h.app.takeover_audio(&test_settings()).unwrap_err();
            assert_eq!(err, "No audio output device found");
            // The generation still moved, so nothing stale survives the attempt.
            assert_eq!(h.app.session_gen.get(), 1);
        });
    }

    #[test]
    fn stopping_and_shutting_down_without_a_player_are_harmless() {
        run(|| async {
            let h = Harness::new();
            h.app.stop_sending();
            h.app.silence_audio();
            h.app.apply_band_live(&test_settings());
            assert!(h.calls().is_empty());
        });
    }

    #[test]
    fn the_band_follows_the_settings_while_a_preview_runs() {
        run(|| async {
            let h = Harness::new();
            let mut settings = test_settings();
            h.app.takeover_audio(&settings).expect("player");
            h.recorder.clear();
            settings.band.qrn_enabled = true;
            settings.band.qrn_level = 0.4;
            h.app.apply_band_live(&settings);
            assert_eq!(h.calls(), vec![Call::Band(settings.band_signature())]);
        });
    }

    #[test]
    fn taking_over_a_borrowed_player_is_refused() {
        run(|| async {
            let h = Harness::new();
            let player = h.app.player.clone();
            let held = player.borrow_mut();
            assert_eq!(
                h.app.takeover_audio(&test_settings()).unwrap_err(),
                "Audio is busy."
            );
            drop(held);
        });
    }
}
