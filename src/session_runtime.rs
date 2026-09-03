//! Owns the session machine, audio, and effect execution. UI sends events only.

use cw_core::{
    generate_training_group, SessionEffect, SessionEvent, SessionMachine, SessionPhase,
    TrainingSettings,
};
use dioxus::prelude::*;

use crate::audio::focus_group_input;
use crate::engine::{finish_session, play_text_now, sleep_cancelable, AppState, PlayError, Screen};
use crate::time::now_ms;

pub fn dispatch_event(
    app: &AppState,
    mut runtime: Signal<Option<cw_core::GroupSession>>,
    event: SessionEvent,
    expected_gen: u64,
) -> Vec<SessionEffect> {
    if app.session_gen.get() != expected_gen {
        return Vec::new();
    }
    let mut slot = app.machine.borrow_mut();
    let Some(machine) = slot.as_mut() else {
        return Vec::new();
    };
    if machine.session().session_id().raw() != expected_gen {
        return Vec::new();
    }
    let before = machine.session().confirmed_flags();
    let effects = machine.apply(event, now_ms());
    let after = machine.session().confirmed_flags();
    for (index, (was, now)) in before.iter().zip(after.iter()).enumerate() {
        if !was && *now {
            if let Some(group) = machine.session().group(index) {
                let next = cw_core::update_sampling_state_from_answer(
                    &app.sampling.borrow(),
                    group.sent(),
                    group.input(),
                );
                *app.sampling.borrow_mut() = next;
            }
        }
    }
    runtime.set(Some(machine.session().clone()));
    effects
}

pub fn send_command(
    app: AppState,
    runtime: Signal<Option<cw_core::GroupSession>>,
    screen: Signal<crate::engine::Screen>,
    result: Signal<Option<cw_core::SessionResult>>,
    auto_message: Signal<Option<String>>,
    sessions: Signal<Vec<cw_core::SessionResult>>,
    settings_sig: Signal<TrainingSettings>,
    toast: Signal<Option<String>>,
    event: SessionEvent,
) {
    let gen = app.session_gen.get();
    let settings = runtime
        .read()
        .as_ref()
        .map(|s| s.settings().clone())
        .unwrap_or_else(|| settings_sig());
    let effects = dispatch_event(&app, runtime, event, gen);
    spawn_effects(
        effects,
        settings,
        app,
        gen,
        runtime,
        screen,
        result,
        auto_message,
        sessions,
        settings_sig,
        toast,
    );
}

pub fn spawn_effects(
    effects: Vec<SessionEffect>,
    settings: TrainingSettings,
    app: AppState,
    gen: u64,
    runtime: Signal<Option<cw_core::GroupSession>>,
    screen: Signal<Screen>,
    result: Signal<Option<cw_core::SessionResult>>,
    auto_message: Signal<Option<String>>,
    sessions: Signal<Vec<cw_core::SessionResult>>,
    settings_sig: Signal<TrainingSettings>,
    toast: Signal<Option<String>>,
) {
    if effects.is_empty() {
        return;
    }
    spawn(async move {
        drive_effects(
            effects,
            settings,
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
    });
}

/// Build the session machine on the UI thread so Training has groups before the
/// first paint, and so a cancelled start cannot resurrect a dead session.
pub fn boot_machine_session(
    settings: TrainingSettings,
    history: &[cw_core::SessionResult],
    app: &AppState,
    gen: u64,
    mut runtime: Signal<Option<cw_core::GroupSession>>,
    mut screen: Signal<Screen>,
) -> Option<Vec<SessionEffect>> {
    if app.session_gen.get() != gen {
        return None;
    }
    let history_refs: Vec<_> = history
        .iter()
        .filter(|session| session.usable_for_sampling(&settings))
        .map(|s| &s.letter_accuracy)
        .collect();
    *app.sampling.borrow_mut() = cw_core::create_initial_sampling_state(&history_refs);

    let first = {
        let mut sampling = app.sampling.borrow_mut();
        let mut rng = app.rng.borrow_mut();
        let (group, next_state) = generate_training_group(&settings, &sampling, &mut *rng);
        *sampling = next_state;
        group
    };

    if app.session_gen.get() != gen {
        return None;
    }

    let (machine, effects) =
        SessionMachine::start(cw_core::SessionId::new(gen), now_ms(), settings, first);
    if app.session_gen.get() != gen {
        return None;
    }
    runtime.set(Some(machine.session().clone()));
    screen.set(Screen::Training);
    *app.machine.borrow_mut() = Some(machine);
    Some(effects)
}

async fn drive_effects(
    mut pending: Vec<SessionEffect>,
    settings: TrainingSettings,
    app: AppState,
    gen: u64,
    runtime: Signal<Option<cw_core::GroupSession>>,
    screen: Signal<Screen>,
    result: Signal<Option<cw_core::SessionResult>>,
    auto_message: Signal<Option<String>>,
    sessions: Signal<Vec<cw_core::SessionResult>>,
    settings_sig: Signal<TrainingSettings>,
    toast: Signal<Option<String>>,
) {
    while !pending.is_empty() {
        if app.session_gen.get() != gen {
            return;
        }
        let batch = std::mem::take(&mut pending);
        for effect in batch {
            if app.session_gen.get() != gen {
                return;
            }
            pending.extend(
                handle_effect(
                    effect,
                    &settings,
                    &app,
                    gen,
                    runtime,
                    screen,
                    result,
                    auto_message,
                    sessions,
                    settings_sig,
                    toast,
                )
                .await,
            );
        }
    }
}

async fn handle_effect(
    effect: SessionEffect,
    settings: &TrainingSettings,
    app: &AppState,
    gen: u64,
    mut runtime: Signal<Option<cw_core::GroupSession>>,
    mut screen: Signal<Screen>,
    result: Signal<Option<cw_core::SessionResult>>,
    auto_message: Signal<Option<String>>,
    sessions: Signal<Vec<cw_core::SessionResult>>,
    settings_sig: Signal<TrainingSettings>,
    mut toast: Signal<Option<String>>,
) -> Vec<SessionEffect> {
    match effect {
        SessionEffect::Focus { index } => {
            focus_group_input(index);
            Vec::new()
        }
        SessionEffect::StopAudio => {
            app.stop_audio();
            Vec::new()
        }
        SessionEffect::NeedGroup { index } => {
            let terminal = app
                .machine
                .borrow()
                .as_ref()
                .is_some_and(|m| m.is_terminal());
            if terminal || app.session_gen.get() != gen {
                return Vec::new();
            }
            let snapshot = app
                .machine
                .borrow()
                .as_ref()
                .map(|m| m.session().settings().clone())
                .unwrap_or_else(|| settings.clone());
            let empty = app
                .machine
                .borrow()
                .as_ref()
                .and_then(|m| m.session().group(index).map(|g| g.sent().is_empty()))
                .unwrap_or(true);
            if empty {
                let mut sampling = app.sampling.borrow_mut();
                let mut rng = app.rng.borrow_mut();
                let (group, next_state) = generate_training_group(&snapshot, &sampling, &mut *rng);
                *sampling = next_state;
                drop(sampling);
                drop(rng);
                if let Some(machine) = app.machine.borrow_mut().as_mut() {
                    if !machine.is_terminal() && machine.session().session_id().raw() == gen {
                        machine.set_group_text(index, group);
                        runtime.set(Some(machine.session().clone()));
                    }
                }
            }
            Vec::new()
        }
        SessionEffect::Play { index, text } => {
            if app.session_gen.get() != gen {
                return Vec::new();
            }
            let snapshot = app
                .machine
                .borrow()
                .as_ref()
                .map(|m| m.session().settings().clone())
                .unwrap_or_else(|| settings.clone());
            let outcome = if text.is_empty() {
                Ok((0.0, 0.0, 0.0))
            } else {
                play_text_now(app, gen, &text, &snapshot).await
            };
            if app.session_gen.get() != gen {
                return Vec::new();
            }
            match outcome {
                Ok((duration, char_wpm, effective_wpm)) => dispatch_event(
                    app,
                    runtime,
                    SessionEvent::PlaybackEnded {
                        index,
                        duration_sec: duration,
                        char_wpm,
                        effective_wpm,
                    },
                    gen,
                ),
                Err(PlayError::Cancelled) => {
                    dispatch_event(app, runtime, SessionEvent::PlaybackCancelled { index }, gen)
                }
                Err(PlayError::Failed(message)) => {
                    toast.set(Some(message));
                    dispatch_event(app, runtime, SessionEvent::PlaybackFailed { index }, gen)
                }
            }
        }
        SessionEffect::Sleep { id, ms } => {
            if !sleep_cancelable(ms, gen, app.session_gen.clone()).await {
                return Vec::new();
            }
            let current = app
                .machine
                .borrow()
                .as_ref()
                .is_some_and(|m| m.sleep_is_current(id));
            if !current {
                return Vec::new();
            }
            let phase = app.machine.borrow().as_ref().map(|m| m.phase());
            match phase {
                Some(SessionPhase::InterGroupGap { .. }) => {
                    dispatch_event(app, runtime, SessionEvent::GapElapsed, gen)
                }
                Some(SessionPhase::AwaitingAnswer { .. }) => {
                    dispatch_event(app, runtime, SessionEvent::Timeout, gen)
                }
                _ => Vec::new(),
            }
        }
        SessionEffect::AutoConfirm {
            id,
            index,
            value,
            ms,
        } => {
            if !sleep_cancelable(ms, gen, app.session_gen.clone()).await {
                return Vec::new();
            }
            dispatch_event(
                app,
                runtime,
                SessionEvent::AutoConfirmDue { id, index, value },
                gen,
            )
        }
        SessionEffect::PersistAndShowResults => {
            *app.machine.borrow_mut() = None;
            finish_session(
                app.clone(),
                runtime,
                screen,
                result,
                auto_message,
                sessions,
                settings_sig,
                toast,
            );
            Vec::new()
        }
        SessionEffect::AbortToHome => {
            app.bump_session();
            app.shutdown_audio();
            *app.machine.borrow_mut() = None;
            runtime.set(None);
            screen.set(Screen::Home);
            Vec::new()
        }
    }
}
