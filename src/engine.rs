use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cw_core::{
    apply_auto_level, auto_level_progress, build_session_result, evaluate_auto_level,
    fit_settings_to_alphabet, resolve_station, AutoLevelProgress, CharSamplingState, FastrandRng,
    GroupSession, SessionMachine, SessionResult, StationVoice, TrainingSettings, Transmission,
};
use dioxus::prelude::*;

use crate::audio::{MorseBackend, PlaybackOutcome};
use crate::persist::{
    clear_auto_counters, load_auto_counters, save_auto_counters, save_sessions, save_settings,
};
use crate::time::{local_date_string, now_ms, seed_rng, sleep_ms, POLL_MS};

/// How many times one group is re-armed before the session is told the audio
/// is gone. Each retry rebuilds the player, which is what picks up a device
/// that was swapped, unplugged or suspended mid-session.
const PLAY_ATTEMPTS: u32 = 3;

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

    fn rebuild_player(&self, settings: &TrainingSettings) -> Result<(), String> {
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
}

#[derive(Debug)]
pub(crate) enum PlayError {
    Cancelled,
    Failed(String),
}

pub(crate) async fn play_text_now(
    app: &AppState,
    gen: u64,
    transmission: &Transmission,
    settings: &TrainingSettings,
) -> Result<(f64, f64, f64), PlayError> {
    let mut last_err = None;
    for attempt in 0..PLAY_ATTEMPTS {
        if app.session_gen.get() != gen {
            return Err(PlayError::Cancelled);
        }
        if attempt > 0 {
            let _ = app.rebuild_player(settings);
            sleep_ms(POLL_MS).await;
            if app.session_gen.get() != gen {
                return Err(PlayError::Cancelled);
            }
        }
        match schedule_text(app, gen, transmission, settings).await {
            Ok(wait) => {
                let duration = wait.duration_sec;
                let char_wpm = wait.char_wpm;
                let effective_wpm = wait.effective_wpm;
                match wait.wait().await {
                    PlaybackOutcome::Completed => {
                        if app.session_gen.get() != gen {
                            return Err(PlayError::Cancelled);
                        }
                        return Ok((duration, char_wpm, effective_wpm));
                    }
                    PlaybackOutcome::Cancelled => return Err(PlayError::Cancelled),
                    // The send never reached its end: a device that went away,
                    // or a browser audio clock that stopped moving. Rebuilding
                    // the player on the next attempt is what recovers it.
                    PlaybackOutcome::Failed => {
                        last_err = Some("Audio playback stalled.".to_string());
                    }
                }
            }
            Err(err) => {
                if app.session_gen.get() != gen {
                    return Err(PlayError::Cancelled);
                }
                last_err = Some(err);
            }
        }
    }
    Err(PlayError::Failed(
        last_err.unwrap_or_else(|| "Audio playback failed.".into()),
    ))
}

async fn schedule_text(
    app: &AppState,
    gen: u64,
    transmission: &Transmission,
    settings: &TrainingSettings,
) -> Result<crate::audio::PlaybackWait, String> {
    if app.session_gen.get() != gen {
        return Err("Cancelled.".into());
    }
    #[cfg(feature = "web")]
    {
        let promise = app.player.try_borrow().ok().and_then(|slot| {
            slot.as_ref()
                .and_then(|player| player.take_resume_promise())
        });
        if let Some(promise) = promise {
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        }
        if app.session_gen.get() != gen {
            return Err("Cancelled.".into());
        }
    }
    for _ in 0..8 {
        if app.session_gen.get() != gen {
            return Err("Cancelled.".into());
        }
        match app.player.try_borrow_mut() {
            Ok(mut slot) => {
                let Some(player) = slot.as_mut() else {
                    return Err("Audio is unavailable.".into());
                };
                return player.start_transmission(transmission, settings);
            }
            Err(_) => sleep_ms(POLL_MS).await,
        }
    }
    Err("Audio is busy.".into())
}

pub async fn sleep_cancelable(ms: u32, gen: u64, session_gen: Rc<Cell<u64>>) -> bool {
    let mut left = ms.max(1);
    while left > 0 {
        if session_gen.get() != gen {
            return false;
        }
        let chunk = left.min(POLL_MS);
        sleep_ms(chunk).await;
        left = left.saturating_sub(chunk);
    }
    session_gen.get() == gen
}

pub fn finish_session(app: AppState, signals: SessionSignals) {
    let SessionSignals {
        mut screen,
        mut runtime,
        mut result,
        mut auto_message,
        mut sessions,
        settings: mut settings_sig,
        mut toast,
    } = signals;
    if matches!(screen(), Screen::Results) || runtime.read().is_none() {
        return;
    }
    app.bump_session();
    app.silence_audio();
    // `runtime` was checked just above, so there is a session here.
    let Some(session) = runtime.read().clone() else {
        return;
    };
    if !session.any_confirmed() {
        runtime.set(None);
        screen.set(Screen::Home);
        return;
    }
    let settings = session.settings().clone();
    let built = build_session_result(&session, &settings, now_ms(), local_date_string());
    if built.groups.is_empty() {
        runtime.set(None);
        screen.set(Screen::Home);
        return;
    }

    let mut counters = load_auto_counters(&settings);
    let mut next_settings = settings.clone();
    if let Some(adj) = evaluate_auto_level(built.accuracy, &settings, &mut counters) {
        clear_auto_counters(&adj.counters_cleared_keys);
        apply_auto_level(&mut next_settings, &adj);
        fit_settings_to_alphabet(&mut next_settings);
        next_settings = next_settings.clamp();
        settings_sig.set(next_settings.clone());
        save_settings(&next_settings);
        auto_message.set(Some(adj.message.clone()));
        toast.set(Some(adj.message));
    } else {
        save_auto_counters(&settings, counters);
        auto_message.set(None);
    }

    let mut history = sessions();
    history.push(built.clone());
    save_sessions(&history);
    sessions.set(history);
    result.set(Some(built));
    runtime.set(None);
    screen.set(Screen::Results);
}

pub fn current_auto_progress(settings: &TrainingSettings) -> Option<AutoLevelProgress> {
    auto_level_progress(settings, load_auto_counters(settings))
}

pub async fn play_chars(
    app: AppState,
    gen: u64,
    settings: TrainingSettings,
    chars: String,
    gap_ms: u32,
    mut toast: Signal<Option<String>>,
) {
    let chars: Vec<char> = chars.chars().collect();
    // One operator for the whole run: a different pitch per letter would make
    // the screen harder to learn from, not more realistic.
    let voice = app.station(&settings);
    for (i, ch) in chars.iter().enumerate() {
        if app.session_gen.get() != gen {
            return;
        }
        let alone = Transmission::alone(ch.to_string(), voice);
        let play = play_text_now(&app, gen, &alone, &settings).await;
        if app.session_gen.get() != gen {
            return;
        }
        match play {
            Ok(_) => {}
            Err(PlayError::Cancelled) => return,
            Err(PlayError::Failed(message)) => {
                toast.set(Some(message));
                return;
            }
        }
        if i + 1 < chars.len() && !sleep_cancelable(gap_ms, gen, app.session_gen.clone()).await {
            return;
        }
    }
}

/// Send one short sample once — used by the keying-envelope test chips.
pub async fn play_sample_text(
    app: AppState,
    gen: u64,
    settings: TrainingSettings,
    text: String,
    mut toast: Signal<Option<String>>,
) {
    let alone = Transmission::alone(text, app.station(&settings));
    match play_text_now(&app, gen, &alone, &settings).await {
        Ok(_) | Err(PlayError::Cancelled) => {}
        Err(PlayError::Failed(message)) => toast.set(Some(message)),
    }
}

pub async fn loop_preview_text(
    app: AppState,
    gen: u64,
    settings: Signal<TrainingSettings>,
    text: &'static str,
    gap_ms: u32,
    mut toast: Signal<Option<String>>,
) {
    // One station calling, so moving a band slider changes the band and not
    // the signal you are judging it against.
    let voice = app.station(&settings().clamp());
    loop {
        if app.session_gen.get() != gen {
            return;
        }
        let settings_now = settings().clamp();
        let alone = Transmission::alone(text, voice);
        if let Err(err) = play_text_now(&app, gen, &alone, &settings_now).await {
            match err {
                PlayError::Cancelled => return,
                PlayError::Failed(message) => {
                    toast.set(Some(message));
                    return;
                }
            }
        }
        if !sleep_cancelable(gap_ms, gen, app.session_gen.clone()).await {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::{Behaviour, Call};
    use crate::testing::{run, test_settings, Harness};
    use cw_core::{AutoLevelCounters, CharSetMode};

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
    fn a_cancelable_sleep_stops_the_moment_the_session_moves_on() {
        run(|| async {
            let h = Harness::new();
            let gen = h.app.session_gen.get();
            let slept = sleep_cancelable(0, gen, h.app.session_gen.clone()).await;
            assert!(slept, "a zero wait still completes");

            h.app.bump_session();
            let slept = sleep_cancelable(500, gen, h.app.session_gen.clone()).await;
            assert!(!slept, "a stale generation must not keep waiting");
        });
    }

    #[test]
    fn listening_plays_one_character_at_a_time() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_chars(app, gen, settings, "KM".into(), 200, toast));
            });
            h.pump();
            assert!(h.run_until(10_000, |h| h.texts().len() == 2).await);
            assert_eq!(h.texts(), vec!["K".to_string(), "M".to_string()]);
            assert!(h.toast().is_none());
        });
    }

    #[test]
    fn listening_stops_when_the_user_leaves() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_chars(app, gen, settings, "KMU".into(), 400, toast));
            });
            h.pump();
            assert!(h.run_until(5_000, |h| h.texts().len() == 1).await);
            h.app.bump_session();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), 1, "nothing is played after leaving");
        });
    }

    #[test]
    fn listening_reports_an_audio_failure_once() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            h.set_behaviour(Behaviour::RefuseToStart);
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_chars(app, gen, settings, "KM".into(), 200, toast));
            });
            h.pump();
            assert!(h.run_until(20_000, |h| h.toast().is_some()).await);
            assert_eq!(h.toast().as_deref(), Some("Audio stream: device is gone"));
            // It gave up on the first character rather than grinding through both.
            assert!(h.texts().iter().all(|text| text == "K"));
        });
    }

    #[test]
    fn a_test_sample_plays_once_and_reports_a_failure() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_sample_text(
                    app,
                    gen,
                    settings.clone(),
                    "CQ".into(),
                    toast,
                ));
            });
            h.pump();
            assert!(
                h.run_until(10_000, |h| h.texts() == vec!["CQ".to_string()])
                    .await
            );
            assert!(h.toast().is_none());

            h.set_behaviour(Behaviour::RefuseToStart);
            let gen = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            h.in_app(|| {
                spawn(play_sample_text(app, gen, settings, "E".into(), toast));
            });
            h.pump();
            assert!(h.run_until(20_000, |h| h.toast().is_some()).await);
        });
    }

    #[test]
    fn the_band_preview_loops_until_it_is_stopped() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let (settings_sig, toast) = (h.settings, h.toast);
            h.in_app(|| {
                spawn(loop_preview_text(app, gen, settings_sig, "CQ", 100, toast));
            });
            h.pump();
            assert!(h.run_until(20_000, |h| h.texts().len() >= 3).await);
            assert!(h.texts().iter().all(|text| text == "CQ"));
            h.app.bump_session();
            let played = h.texts().len();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), played);
        });
    }

    #[test]
    fn the_band_preview_gives_up_when_the_audio_fails() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            h.set_behaviour(Behaviour::RefuseToStart);
            let app = h.app.clone();
            let (settings_sig, toast) = (h.settings, h.toast);
            h.in_app(|| {
                spawn(loop_preview_text(app, gen, settings_sig, "CQ", 100, toast));
            });
            h.pump();
            assert!(h.run_until(20_000, |h| h.toast().is_some()).await);
            let played = h.texts().len();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), played, "the loop stopped");
        });
    }

    #[test]
    fn finishing_with_nothing_answered_goes_home_without_storing_anything() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let (app, signals) = (h.app.clone(), h.signals());
            h.in_app(|| finish_session(app, signals));
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.sessions.peek().is_empty());
            assert!(h.result.peek().is_none());
        });
    }

    #[test]
    fn finishing_without_a_session_at_all_is_a_no_op() {
        run(|| async {
            let mut h = Harness::new();
            let (app, signals) = (h.app.clone(), h.signals());
            h.in_app(|| finish_session(app, signals));
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
        });
    }

    #[test]
    fn a_session_of_nothing_but_silence_is_not_stored() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            // A confirmed group with nothing in it scores nothing at all.
            let app = h.app.clone();
            h.in_app(|| {
                if let Some(machine) = app.machine.borrow_mut().as_mut() {
                    machine.set_group_text(0, String::new(), 1);
                    machine.session_mut().confirm(0, String::new(), 10);
                }
            });
            let session = h.app.machine.borrow().as_ref().map(|m| m.session().clone());
            h.runtime.set(session);
            h.pump();
            let (app, signals) = (h.app.clone(), h.signals());
            h.in_app(|| finish_session(app, signals));
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.sessions.peek().is_empty());
            assert!(h.runtime.peek().is_none());
        });
    }

    #[test]
    fn a_good_session_can_move_the_level_and_says_so() {
        run(|| async {
            let mut settings = test_settings();
            settings.auto_level.auto_adjust_level = true;
            settings.auto_level.auto_adjust_above_threshold_count = 1;
            settings.auto_level.auto_adjust_threshold = 50.0;
            settings.curriculum.level = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.settings.peek().curriculum.level, 2);
            let message = h.auto_message.peek().clone().expect("a message");
            assert!(message.contains("Level increased to 2"), "{message}");
            assert_eq!(h.toast().as_deref(), Some(message.as_str()));
            // The level that was just left behind keeps no counters.
            assert_eq!(
                crate::persist::load_auto_counters(&h.settings.peek().clone()),
                AutoLevelCounters::default()
            );
        });
    }

    #[test]
    fn a_session_that_does_not_move_the_level_keeps_counting() {
        run(|| async {
            let mut settings = test_settings();
            settings.auto_level.auto_adjust_level = true;
            settings.auto_level.auto_adjust_above_threshold_count = 5;
            settings.auto_level.auto_adjust_threshold = 50.0;
            let mut h = Harness::with_settings(settings.clone());
            h.start_training();
            h.play_through(60_000).await;
            assert!(h.auto_message.peek().is_none());
            assert_eq!(
                crate::persist::load_auto_counters(&settings),
                AutoLevelCounters { above: 1, below: 0 }
            );
            let progress = current_auto_progress(&settings).expect("progress");
            assert_eq!(progress.above_count, 1);
            assert_eq!(progress.above_target, 5);
        });
    }

    /// Holding the player borrow across the await is the point of the test:
    /// it is how a second caller sees the player while it is in use.
    #[allow(clippy::await_holding_refcell_ref)]
    #[test]
    fn a_send_while_the_player_is_busy_is_reported_rather_than_wedged() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            // Something else is holding the player: every attempt to schedule a
            // send has to give up instead of blocking the session.
            let player = h.app.player.clone();
            let held = player.borrow_mut();
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_sample_text(app, gen, settings, "E".into(), toast));
            });
            h.pump();
            assert!(h.run_until(30_000, |h| h.toast().is_some()).await);
            assert_eq!(h.toast().as_deref(), Some("Audio is busy."));
            drop(held);
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

    #[test]
    fn a_send_with_no_player_left_rebuilds_one() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            // The player is gone, the way a shutdown leaves it.
            *h.app.player.borrow_mut() = None;
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_sample_text(app, gen, settings, "E".into(), toast));
            });
            h.pump();
            assert!(h.run_until(20_000, |h| !h.texts().is_empty()).await);
            assert!(h.toast().is_none());
            assert_eq!(h.recorder.players_built.get(), 2);
        });
    }

    #[test]
    fn a_send_that_is_taken_over_mid_flight_is_cancelled() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let outcome = std::rc::Rc::new(std::cell::RefCell::new(None));
            let sink = std::rc::Rc::clone(&outcome);
            h.in_app(|| {
                spawn(async move {
                    let alone = Transmission::alone("CQ", app.station(&settings));
                    let result = play_text_now(&app, gen, &alone, &settings).await;
                    *sink.borrow_mut() = Some(result);
                });
            });
            h.pump();
            h.advance(100).await;
            assert!(outcome.borrow().is_none(), "still sending");
            // Another screen claims the audio.
            h.app.bump_session();
            assert!(h.run_until(5_000, |_| outcome.borrow().is_some()).await);
            assert!(matches!(
                outcome.borrow().as_ref(),
                Some(Err(PlayError::Cancelled))
            ));
        });
    }

    #[test]
    fn a_send_asked_for_after_the_session_moved_on_never_starts() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let gen = h.app.takeover_audio(&settings).expect("player");
            // The audio was claimed by something else before the send began.
            h.app.bump_session();
            h.recorder.clear();
            let app = h.app.clone();
            let outcome = std::rc::Rc::new(std::cell::RefCell::new(None));
            let sink = std::rc::Rc::clone(&outcome);
            h.in_app(|| {
                spawn(async move {
                    let alone = Transmission::alone("CQ", app.station(&settings));
                    *sink.borrow_mut() = Some(play_text_now(&app, gen, &alone, &settings).await);
                });
            });
            h.pump();
            assert!(h.run_until(1_000, |_| outcome.borrow().is_some()).await);
            assert!(matches!(
                outcome.borrow().as_ref(),
                Some(Err(PlayError::Cancelled))
            ));
            assert!(h.texts().is_empty(), "nothing should have been sent");
        });
    }

    #[test]
    fn auto_level_progress_is_hidden_when_the_setting_is_off() {
        let mut settings = test_settings();
        settings.auto_level.auto_adjust_level = false;
        assert!(current_auto_progress(&settings).is_none());
        settings.auto_level.auto_adjust_level = true;
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        assert!(current_auto_progress(&settings).is_some());
    }
}
