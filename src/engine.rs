use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cw_core::{
    apply_auto_level, auto_level_progress, build_session_result, evaluate_auto_level,
    fit_settings_to_alphabet, AutoLevelProgress, CharSamplingState, FastrandRng, GroupSession,
    SessionMachine, SessionResult, TrainingSettings,
};
use dioxus::prelude::*;

use crate::audio::{MorsePlayer, PlaybackOutcome};
use crate::persist::{
    clear_auto_counters, load_auto_counters, save_auto_counters, save_sessions, save_settings,
};
use crate::time::{local_date_string, now_ms, seed_rng, sleep_ms, POLL_MS};

const PLAY_ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
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
    pub player: Rc<RefCell<Option<MorsePlayer>>>,
    pub rng: Rc<RefCell<FastrandRng>>,
    pub sampling: Rc<RefCell<CharSamplingState>>,
    pub machine: Rc<RefCell<Option<SessionMachine>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session_gen: Rc::new(Cell::new(0)),
            player: Rc::new(RefCell::new(None)),
            rng: Rc::new(RefCell::new(FastrandRng(seed_rng()))),
            sampling: Rc::new(RefCell::new(CharSamplingState::default())),
            machine: Rc::new(RefCell::new(None)),
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
            *slot = Some(MorsePlayer::new()?);
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
        self.stop_audio();
        let gen = self.bump_session();
        self.ensure_player(settings)?;
        Ok(gen)
    }

    pub fn apply_band_live(&self, settings: &TrainingSettings) {
        if let Ok(mut slot) = self.player.try_borrow_mut() {
            if let Some(player) = slot.as_mut() {
                let _ = player.apply_band(settings);
            }
        }
    }

    pub fn stop_audio(&self) {
        if let Ok(mut slot) = self.player.try_borrow_mut() {
            if let Some(player) = slot.as_mut() {
                player.stop();
            }
        }
    }

    pub fn shutdown_audio(&self) {
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
    text: &str,
    settings: &TrainingSettings,
) -> Result<(f64, f64, f64), PlayError> {
    let mut last_err = None;
    for attempt in 0..PLAY_ATTEMPTS {
        if attempt > 0 {
            let _ = app.rebuild_player(settings);
            sleep_ms(POLL_MS).await;
        }
        match schedule_text(app, text, settings).await {
            Ok(wait) => {
                let duration = wait.duration_sec;
                let char_wpm = wait.char_wpm;
                let effective_wpm = wait.effective_wpm;
                match wait.wait().await {
                    PlaybackOutcome::Completed => {
                        return Ok((duration, char_wpm, effective_wpm));
                    }
                    PlaybackOutcome::Cancelled => return Err(PlayError::Cancelled),
                }
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(PlayError::Failed(
        last_err.unwrap_or_else(|| "Audio playback failed.".into()),
    ))
}

async fn schedule_text(
    app: &AppState,
    text: &str,
    settings: &TrainingSettings,
) -> Result<crate::audio::PlaybackWait, String> {
    #[cfg(feature = "web")]
    {
        let promise = app
            .player
            .try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(MorsePlayer::take_resume_promise));
        if let Some(promise) = promise {
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        }
    }
    for _ in 0..8 {
        match (app.rng.try_borrow_mut(), app.player.try_borrow_mut()) {
            (Ok(mut rng), Ok(mut slot)) => {
                let Some(player) = slot.as_mut() else {
                    return Err("Audio is unavailable.".into());
                };
                return player.start_text(text, settings, &mut *rng);
            }
            _ => sleep_ms(POLL_MS).await,
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

pub async fn run_group_session(
    settings: TrainingSettings,
    history: Vec<SessionResult>,
    app: AppState,
    gen: u64,
    runtime: Signal<Option<GroupSession>>,
    screen: Signal<Screen>,
    result: Signal<Option<SessionResult>>,
    auto_message: Signal<Option<String>>,
    sessions: Signal<Vec<SessionResult>>,
    settings_sig: Signal<TrainingSettings>,
    toast: Signal<Option<String>>,
) {
    crate::session_runtime::run_machine_session(
        settings,
        history,
        app,
        gen,
        runtime,
        screen,
        result,
        auto_message,
        sessions,
        settings_sig,
        toast,
    )
    .await;
}

pub fn finish_session(
    app: AppState,
    mut runtime: Signal<Option<GroupSession>>,
    mut screen: Signal<Screen>,
    mut result: Signal<Option<SessionResult>>,
    mut auto_message: Signal<Option<String>>,
    mut sessions: Signal<Vec<SessionResult>>,
    mut settings_sig: Signal<TrainingSettings>,
    mut toast: Signal<Option<String>>,
) {
    if matches!(screen(), Screen::Results) || runtime.read().is_none() {
        return;
    }
    app.bump_session();
    app.shutdown_audio();
    let Some(session) = runtime.read().clone() else {
        screen.set(Screen::Home);
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
    for (i, ch) in chars.iter().enumerate() {
        if app.session_gen.get() != gen {
            return;
        }
        let play = play_text_now(&app, &ch.to_string(), &settings).await;
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

pub async fn loop_preview_text(
    app: AppState,
    gen: u64,
    settings: Signal<TrainingSettings>,
    text: &'static str,
    gap_ms: u32,
    mut toast: Signal<Option<String>>,
) {
    loop {
        if app.session_gen.get() != gen {
            return;
        }
        let settings_now = settings().clamp();
        if let Err(err) = play_text_now(&app, text, &settings_now).await {
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
