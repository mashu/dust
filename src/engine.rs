use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cw_core::{
    apply_auto_level, apply_practice_window, auto_level_progress, build_session_result,
    compute_group_gap_ms, create_initial_sampling_state, current_practice_window,
    evaluate_auto_level, generate_training_group, update_sampling_state_from_answer,
    AutoAdjustMode, AutoLevelProgress, CharSamplingState, FastrandRng, GroupSession, SessionResult,
    TrainingSettings,
};
use dioxus::prelude::*;

use crate::audio::{focus_group_input, MorsePlayer};
use crate::persist::{
    clear_auto_counters, load_auto_counters, save_auto_counters, save_sessions, save_settings,
};
use crate::time::{local_date_string, now_ms, seed_rng, sleep_ms, POLL_MS};

pub const AUTO_CONFIRM_DELAY_MS: u32 = 300;
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
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session_gen: Rc::new(Cell::new(0)),
            player: Rc::new(RefCell::new(None)),
            rng: Rc::new(RefCell::new(FastrandRng(seed_rng()))),
            sampling: Rc::new(RefCell::new(CharSamplingState::default())),
        }
    }

    pub fn bump_session(&self) -> u64 {
        let next = self.session_gen.get() + 1;
        self.session_gen.set(next);
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

async fn play_text_now(
    app: &AppState,
    text: &str,
    settings: &TrainingSettings,
) -> Result<(f64, f64), String> {
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
                wait.wait().await;
                return Ok((duration, char_wpm));
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| "Audio playback failed.".into()))
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
        match (
            app.rng.try_borrow_mut(),
            app.player.try_borrow_mut(),
        ) {
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

fn letter_history(
    sessions: &[SessionResult],
) -> Vec<&std::collections::BTreeMap<char, cw_core::LetterAccuracy>> {
    sessions.iter().map(|s| &s.letter_accuracy).collect()
}

pub async fn run_group_session(
    settings: TrainingSettings,
    history: Vec<SessionResult>,
    app: AppState,
    gen: u64,
    mut runtime: Signal<Option<GroupSession>>,
    mut screen: Signal<Screen>,
    result: Signal<Option<SessionResult>>,
    auto_message: Signal<Option<String>>,
    sessions: Signal<Vec<SessionResult>>,
    settings_sig: Signal<TrainingSettings>,
    mut toast: Signal<Option<String>>,
) {
    let n = settings.num_groups as usize;
    let started = now_ms();
    let mut session = GroupSession::new(gen, started, n);
    let history_refs = letter_history(&history);
    *app.sampling.borrow_mut() = create_initial_sampling_state(&history_refs);

    {
        let mut sampling = app.sampling.borrow_mut();
        let mut rng = app.rng.borrow_mut();
        let (group, next_state) = generate_training_group(&settings, &sampling, &mut *rng);
        *sampling = next_state;
        session.set_group(0, group);
    }
    runtime.set(Some(session.clone()));
    screen.set(Screen::Training);

    for i in 0..n {
        if app.session_gen.get() != gen {
            return;
        }
        {
            let mut current = runtime.write();
            let Some(s) = current.as_mut() else {
                return;
            };
            if s.groups.get(i).map(|g| g.is_empty()).unwrap_or(true) {
                let sampling = app.sampling.borrow();
                let mut rng = app.rng.borrow_mut();
                let (g, next_state) = generate_training_group(&settings, &sampling, &mut *rng);
                drop(sampling);
                *app.sampling.borrow_mut() = next_state;
                s.set_group(i, g);
            }
        }
        // Keep the previous group current through the Farnsworth gap so a lock-off
        // user cannot type/confirm the next group before its Morse starts.
        if i > 0 {
            let gap = compute_group_gap_ms(&settings);
            if gap > 0 && !sleep_cancelable(gap, gen, app.session_gen.clone()).await {
                return;
            }
            if app.session_gen.get() != gen {
                return;
            }
        }

        let group = {
            let mut current = runtime.write();
            let Some(s) = current.as_mut() else {
                return;
            };
            s.begin_group(i, now_ms());
            s.groups.get(i).cloned().unwrap_or_default()
        };
        focus_group_input(i);
        if group.is_empty() {
            if let Some(s) = runtime.write().as_mut() {
                s.end_playback(i, now_ms(), 0.0);
            }
        } else {
            let play = play_text_now(&app, &group, &settings).await;
            if app.session_gen.get() != gen {
                return;
            }
            match play {
                Ok((duration, char_wpm)) => {
                    let ended = now_ms().max(
                        runtime
                            .read()
                            .as_ref()
                            .and_then(|s| s.group_start_at.get(i).copied())
                            .unwrap_or(now_ms())
                            + (duration * 1000.0).round() as u64,
                    );
                    if let Some(s) = runtime.write().as_mut() {
                        s.end_playback(i, ended, char_wpm);
                    }
                }
                Err(message) => {
                    toast.set(Some(message));
                    if let Some(s) = runtime.write().as_mut() {
                        s.end_playback(i, now_ms(), 0.0);
                    }
                }
            }
        }
        focus_group_input(i);

        let timeout_ms = (settings.group_timeout * 1000.0).round() as u32;
        let confirmed =
            wait_for_confirm(runtime, i, timeout_ms, gen, app.session_gen.clone()).await;
        if app.session_gen.get() != gen {
            return;
        }
        if !confirmed {
            confirm_group(runtime, i, None, &app);
        }
    }

    if app.session_gen.get() != gen {
        return;
    }
    finish_session(
        settings,
        app,
        runtime,
        screen,
        result,
        auto_message,
        sessions,
        settings_sig,
        toast,
    );
}

async fn wait_for_confirm(
    runtime: Signal<Option<GroupSession>>,
    index: usize,
    timeout_ms: u32,
    gen: u64,
    session_gen: Rc<Cell<u64>>,
) -> bool {
    let start = now_ms();
    loop {
        if session_gen.get() != gen {
            return false;
        }
        if runtime
            .read()
            .as_ref()
            .and_then(|s| s.confirmed.get(index).copied())
            .unwrap_or(false)
        {
            return true;
        }
        if timeout_ms > 0 && now_ms().saturating_sub(start) >= u64::from(timeout_ms) {
            return false;
        }
        sleep_ms(POLL_MS).await;
    }
}

pub fn confirm_group(
    mut runtime: Signal<Option<GroupSession>>,
    index: usize,
    override_value: Option<String>,
    app: &AppState,
) {
    let mut current = runtime.write();
    let Some(session) = current.as_mut() else {
        return;
    };
    let value = override_value
        .unwrap_or_else(|| session.user_input.get(index).cloned().unwrap_or_default())
        .trim()
        .to_ascii_uppercase();
    let sent = session.groups.get(index).cloned().unwrap_or_default();
    if index != session.current_group {
        return;
    }
    if !session.confirm(index, value.clone(), now_ms()) {
        return;
    }
    let next = update_sampling_state_from_answer(&app.sampling.borrow(), &sent, &value);
    *app.sampling.borrow_mut() = next;
}

pub fn finish_session(
    settings: TrainingSettings,
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
    if !session.confirmed.iter().any(|confirmed| *confirmed) {
        runtime.set(None);
        screen.set(Screen::Home);
        return;
    }
    let built = build_session_result(&session, &settings, now_ms(), local_date_string());
    if built.groups.is_empty() {
        runtime.set(None);
        screen.set(Screen::Home);
        return;
    }

    let mode = AutoAdjustMode::from_char_set(settings.char_set_mode);
    let digits = if matches!(mode, AutoAdjustMode::Mixed) {
        Some(settings.digits_level)
    } else {
        None
    };
    let level = match mode {
        AutoAdjustMode::Digits => settings.digits_level,
        _ => settings.level,
    };
    let mut counters = load_auto_counters(mode, settings.char_set_mode, level, digits);
    let mut next_settings = settings.clone();
    let practice_window = current_practice_window(&settings);
    if let Some(adj) = evaluate_auto_level(built.accuracy, &settings, &mut counters) {
        save_auto_counters(mode, settings.char_set_mode, level, digits, counters);
        clear_auto_counters(&adj.counters_cleared_keys);
        apply_auto_level(&mut next_settings, &adj);
        if let Some(window) = practice_window {
            apply_practice_window(&mut next_settings, window);
        }
        next_settings = next_settings.clamp();
        settings_sig.set(next_settings.clone());
        save_settings(&next_settings);
        auto_message.set(Some(adj.message.clone()));
        toast.set(Some(adj.message));
    } else {
        save_auto_counters(mode, settings.char_set_mode, level, digits, counters);
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
    let mode = AutoAdjustMode::from_char_set(settings.char_set_mode);
    let digits = matches!(mode, AutoAdjustMode::Mixed).then_some(settings.digits_level);
    let level = match mode {
        AutoAdjustMode::Digits => settings.digits_level,
        _ => settings.level,
    };
    auto_level_progress(settings, load_auto_counters(mode, settings.char_set_mode, level, digits))
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
        match play {
            Ok(_) => {}
            Err(message) => {
                toast.set(Some(message));
                return;
            }
        }
        if i + 1 < chars.len()
            && !sleep_cancelable(gap_ms, gen, app.session_gen.clone()).await
        {
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
            toast.set(Some(err));
            return;
        }
        if !sleep_cancelable(gap_ms, gen, app.session_gen.clone()).await {
            return;
        }
    }
}
