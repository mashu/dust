//! Owns the session machine, audio, and effect execution. UI sends events only.

use cw_core::{
    generate_training_group, resolve_group_repeats, resolve_pileup, resolve_station,
    CharSamplingState, FastrandRng, SessionEffect, SessionEvent, SessionMachine, SessionPhase,
    StationVoice, TrainingSettings, Transmission,
};
use dioxus::prelude::*;

use crate::audio::focus_group_input;
use crate::engine::{
    finish_session, play_text_now, sleep_cancelable, AppState, PlayError, Screen, SessionSignals,
};
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

pub fn send_command(app: AppState, signals: SessionSignals, event: SessionEvent) {
    let gen = app.session_gen.get();
    let settings = signals
        .runtime
        .read()
        .as_ref()
        .map(|s| s.settings().clone())
        .unwrap_or_else(|| (signals.settings)());
    let effects = dispatch_event(&app, signals.runtime, event, gen);
    spawn_effects(effects, settings, app, gen, signals);
}

pub fn spawn_effects(
    effects: Vec<SessionEffect>,
    settings: TrainingSettings,
    app: AppState,
    gen: u64,
    signals: SessionSignals,
) {
    if effects.is_empty() {
        return;
    }
    spawn(async move {
        drive_effects(effects, settings, app, gen, signals).await;
    });
}

/// Build the session machine on the UI thread so Training has groups before the
/// first paint, and so a cancelled start cannot resurrect a dead session.
pub fn boot_machine_session(
    settings: TrainingSettings,
    history: &[cw_core::SessionResult],
    app: &AppState,
    gen: u64,
    signals: SessionSignals,
) -> Option<Vec<SessionEffect>> {
    let SessionSignals {
        mut runtime,
        mut screen,
        ..
    } = signals;
    if app.session_gen.get() != gen {
        return None;
    }
    let history_refs: Vec<_> = history
        .iter()
        .filter(|session| session.usable_for_sampling(&settings))
        .map(|s| &s.letter_accuracy)
        .collect();
    *app.sampling.borrow_mut() = cw_core::create_initial_sampling_state(&history_refs);

    let (first, first_repeats) = {
        let mut sampling = app.sampling.borrow_mut();
        let mut rng = app.rng.borrow_mut();
        let (group, next_state) = generate_training_group(&settings, &sampling, &mut *rng);
        *sampling = next_state;
        let repeats = resolve_group_repeats(&settings, &mut *rng);
        (group, repeats)
    };

    if app.session_gen.get() != gen {
        return None;
    }

    let (machine, effects) = SessionMachine::start(
        cw_core::SessionId::new(gen),
        now_ms(),
        settings,
        first,
        first_repeats,
    );
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
    signals: SessionSignals,
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
            pending.extend(handle_effect(effect, &settings, &app, gen, signals).await);
        }
    }
}

/// The station behind one group.
///
/// Derived from the session and the group's place in it, so every repeat of
/// that group comes from the same operator — a station repeating its call does
/// not change frequency, speed or strength between sends — while the next
/// group is somebody else. Deriving it beats remembering it: nothing has to be
/// carried across the sends, and a retry after a stalled send tunes back in to
/// the same station rather than a new one.
fn station_for(settings: &TrainingSettings, gen: u64, index: usize) -> StationVoice {
    resolve_station(settings, &mut group_rng(gen, index, 0))
}

/// A generator that depends only on the session and the group, so every repeat
/// of a group — and every retry after a stalled send — tunes back in to the
/// same operators rather than a new set.
fn group_rng(gen: u64, index: usize, salt: u64) -> FastrandRng {
    FastrandRng(
        gen.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((index as u64).wrapping_mul(0x2545_F491))
            .wrapping_add(salt)
            | 1,
    )
}

/// What the receiver hears for this group: the station being copied, and
/// whoever else is calling across it.
///
/// The others send groups of their own, drawn from the same curriculum so they
/// sound like stations rather than noise — but from a generator of their own,
/// off a blank slate, so a pile-up never teaches the sampler anything. Only
/// the station you answer counts.
fn transmission_for(
    settings: &TrainingSettings,
    gen: u64,
    index: usize,
    text: String,
) -> Transmission {
    let voice = station_for(settings, gen, index);
    let most = settings.band.stations_max.max(1) as usize;
    if most <= 1 {
        return Transmission::alone(text, voice);
    }
    let mut rng = group_rng(gen, index, 0x51ED_2701);
    let elsewhere = CharSamplingState::default();
    let texts: Vec<String> = (0..most.saturating_sub(1))
        .map(|_| generate_training_group(settings, &elsewhere, &mut rng).0)
        .collect();
    Transmission {
        others: resolve_pileup(settings, &voice, &text, &texts, &mut rng),
        text,
        voice,
    }
}

/// Who the receiver has in its passband for this group, as pitch and strength
/// alone.
///
/// Derived the same way the audio is, from the same generator, so the scope
/// draws the stations you are actually listening to rather than a plausible
/// set. Only the voices come back: what anybody is sending stays out of the
/// display, or the scope would hand you the answer you are meant to copy.
pub fn heard_for(
    settings: &TrainingSettings,
    gen: u64,
    index: usize,
    text: &str,
) -> Vec<crate::ui::scope::Heard> {
    let sending = transmission_for(settings, gen, index, text.to_string());
    std::iter::once(crate::ui::scope::Heard {
        voice: sending.voice,
        wanted: true,
    })
    .chain(sending.others.iter().map(|other| crate::ui::scope::Heard {
        voice: other.voice,
        wanted: false,
    }))
    .collect()
}

async fn handle_effect(
    effect: SessionEffect,
    settings: &TrainingSettings,
    app: &AppState,
    gen: u64,
    signals: SessionSignals,
) -> Vec<SessionEffect> {
    let SessionSignals {
        mut runtime,
        mut screen,
        mut toast,
        ..
    } = signals;
    match effect {
        SessionEffect::Focus { index } => {
            focus_group_input(index);
            Vec::new()
        }
        SessionEffect::StopAudio => {
            app.stop_sending();
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
                let repeats = resolve_group_repeats(&snapshot, &mut *rng);
                drop(sampling);
                drop(rng);
                if let Some(machine) = app.machine.borrow_mut().as_mut() {
                    if !machine.is_terminal() && machine.session().session_id().raw() == gen {
                        machine.set_group_text(index, group, repeats);
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
                let transmission = transmission_for(&snapshot, gen, index, text.clone());
                play_text_now(app, gen, &transmission, &snapshot).await
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
                Some(SessionPhase::InterGroupGap { .. }) | Some(SessionPhase::RepeatGap { .. }) => {
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
            finish_session(app.clone(), signals);
            Vec::new()
        }
        SessionEffect::AbortToHome => {
            app.bump_session();
            app.silence_audio();
            *app.machine.borrow_mut() = None;
            runtime.set(None);
            screen.set(Screen::Home);
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::{Behaviour, Call};
    use crate::engine::Screen;
    use crate::testing::{run, test_settings, Harness};
    use cw_core::CharSetMode;
    use cw_core::SessionEffect;
    use cw_core::SessionPhase;

    /// A whole session, answered correctly, ends on the results screen with the
    /// session stored.
    #[test]
    fn answering_every_group_finishes_the_session_and_stores_it() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.groups.len(), 2);
            assert!(result.groups.iter().all(|g| g.correct));
            assert_eq!(result.accuracy, 1.0);
            assert_eq!(h.sessions.peek().len(), 1);
            assert_eq!(crate::persist::load_sessions().len(), 1);
            // The machine is torn down, and the audio with it.
            assert!(h.app.machine.borrow().is_none());
            assert!(h.calls().contains(&Call::Shutdown));
            // Two groups, one send each.
            assert_eq!(h.texts().len(), 2);
        });
    }

    #[test]
    fn a_group_nobody_answers_times_out_and_the_session_moves_on() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_timeout = 1.0;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            // Nothing typed: the answer window closes on its own.
            assert!(
                h.run_until(4_000, |h| h.texts().len() == 2).await,
                "the second group should have been sent"
            );
            let confirmed = h.runtime.peek().as_ref().map(|s| s.confirmed_flags());
            assert_eq!(confirmed, Some(vec![true, false]));
            assert_eq!(
                h.runtime
                    .peek()
                    .as_ref()
                    .and_then(|s| s.group(0))
                    .map(|g| g.input().to_string()),
                Some(String::new())
            );
        });
    }

    #[test]
    fn a_group_sent_more_than_once_opens_the_answer_box_only_at_the_end() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            settings.playback.link_group_repeat = true;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            let sent = h.sent(0);

            assert!(h.run_until(20_000, |h| h.texts().len() == 3).await);
            // Every send is the same group.
            assert_eq!(h.texts(), vec![sent.clone(); 3]);
            assert!(h.run_until(5_000, |h| h.awaiting_answer() == Some(0)).await);
            // And no fourth send after the answer window opens.
            h.advance(2_000).await;
            assert_eq!(h.texts().len(), 3);
        });
    }

    #[test]
    fn a_matching_answer_confirms_itself_and_the_next_group_follows() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let first = h.sent(0);
            h.type_answer(0, &first.to_lowercase());
            // Nothing is confirmed until the auto-confirm delay has passed.
            assert!(h
                .runtime
                .peek()
                .as_ref()
                .is_some_and(|s| !s.confirmed_flags()[0]));
            assert!(
                h.run_until(1_000, |h| h
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.confirmed_flags()[0]))
                    .await
            );
            // Typed in lower case, stored the way it was sent.
            assert_eq!(
                h.runtime
                    .peek()
                    .as_ref()
                    .and_then(|s| s.group(0))
                    .map(|g| g.input().to_string()),
                Some(first)
            );
            // The next group is generated before it is played, never sent empty.
            assert!(h.run_until(8_000, |h| h.texts().len() == 2).await);
            assert!(h.texts().iter().all(|text| !text.is_empty()));
            assert_ne!(h.sent(1), "");
        });
    }

    #[test]
    fn changing_an_answer_withdraws_the_confirm_that_was_pending() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            // Backspace inside the 300 ms window.
            h.advance(100).await;
            h.type_answer(0, &sent[..1]);
            h.advance(1_000).await;
            assert!(h
                .runtime
                .peek()
                .as_ref()
                .is_some_and(|s| !s.confirmed_flags()[0]));
            assert_eq!(h.awaiting_answer(), Some(0));
        });
    }

    #[test]
    fn typing_is_refused_while_the_group_is_being_sent() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            h.advance(1_000).await;
            assert!(h
                .runtime
                .peek()
                .as_ref()
                .is_some_and(|s| !s.confirmed_flags()[0]));
        });
    }

    /// Starting a second session while the first one is still sending must not
    /// leave the first session's effects running against the new one.
    #[test]
    fn starting_a_new_session_mid_send_leaves_nothing_of_the_old_one() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let first_gen = h.app.session_gen.get();
            let first_group = h.sent(0);
            h.advance(100).await;

            h.recorder.clear();
            h.start_training();
            let second_gen = h.app.session_gen.get();
            assert!(second_gen > first_gen);
            // The old send was stopped and a new one started.
            assert!(h.calls().contains(&Call::Stop));
            assert_eq!(h.texts().len(), 1);

            // Let the old chain's cancellation land: it must not end the new session.
            h.advance(2_000).await;
            assert_eq!(h.screen(), Screen::Training);
            assert!(!matches!(h.phase(), Some(SessionPhase::Aborted)));
            assert_eq!(
                h.app
                    .machine
                    .borrow()
                    .as_ref()
                    .map(|m| m.session().session_id().raw()),
                Some(second_gen)
            );
            // And the session in view is the new one.
            let _ = first_group;
            assert!(h.run_until(6_000, |h| h.awaiting_answer() == Some(0)).await);
        });
    }

    /// Leaving the session while a group is in flight: the cancelled send must
    /// not drag the app back onto a results screen.
    #[test]
    fn walking_away_mid_send_goes_home_and_stays_there() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(100).await;
            h.send(SessionEvent::Abort);
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.runtime.peek().is_none());
            assert!(h.app.machine.borrow().is_none());

            h.advance(5_000).await;
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.result.peek().is_none());
            assert!(h.sessions.peek().is_empty());
        });
    }

    #[test]
    fn ending_a_session_early_keeps_what_was_already_scored() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            assert!(
                h.run_until(1_000, |h| h
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.confirmed_flags()[0]))
                    .await
            );
            h.send(SessionEvent::FinishNow);
            h.advance(200).await;
            assert_eq!(h.screen(), Screen::Results);
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.groups.len(), 1);
            assert_eq!(h.sessions.peek().len(), 1);
        });
    }

    /// The regression test for a send that is counted down on the wall clock:
    /// with the audio clock frozen, the session must not walk on as though the
    /// group had been heard.
    #[test]
    fn a_stalled_send_never_passes_for_a_finished_one() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::Stall);
            h.start_training();

            // Well past the length of the group, and still nothing is confirmed
            // and no answer window has opened.
            h.advance(1_500).await;
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
            assert!(h.awaiting_answer().is_none());

            // The stall is eventually reported, the player rebuilt, and after
            // three attempts the session gives up rather than pretending.
            assert!(
                h.run_until(60_000, |h| h.screen() == Screen::Home).await,
                "a stalled player should end the session, not hang"
            );
            assert!(h.recorder.players_built.get() > 1, "the player is rebuilt");
            assert_eq!(h.toast().as_deref(), Some("Audio playback stalled."));
            assert!(h.sessions.peek().is_empty());
        });
    }

    /// Leaving the tab must not cost the listener their session. The audio
    /// clock parks while the page is away, and a send that is counted down
    /// against it would run out the stall budget in silence and end the
    /// session — on the browser build, on the first group, for a tab switch.
    #[test]
    fn a_session_survives_the_page_going_to_the_background() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(200).await;
            h.set_page_hidden(true);

            // Far longer than the stall grace, in a phase that would have
            // failed on the wall clock.
            h.advance(60_000).await;
            assert_eq!(
                h.phase(),
                Some(SessionPhase::Playing { index: 0 }),
                "the group is still waiting to be heard"
            );
            assert_eq!(h.toast(), None, "nothing has gone wrong");
            assert_eq!(h.screen(), Screen::Training);
            assert_eq!(h.texts().len(), 1, "and it was not sent again");

            // Back to the tab: the same send finishes and the session goes on.
            h.set_page_hidden(false);
            assert!(
                h.run_until(30_000, |h| h.awaiting_answer().is_some()).await,
                "the answer window should open once the page is back"
            );
            assert_eq!(h.screen(), Screen::Training);
            h.play_through(120_000).await;
            assert_eq!(h.screen(), Screen::Results);
        });
    }

    /// Callsign mode changes how a group is built and nothing else. The proof
    /// is that a whole session runs through the same machine, the same audio
    /// and the same scoring, and lands on the results screen with real
    /// callsigns in it.
    #[test]
    fn a_callsign_session_runs_through_the_same_machine_as_any_other() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            settings.curriculum.callsign_level = cw_core::CALLSIGN_TIER_MAX;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(120_000).await;

            assert_eq!(h.screen(), Screen::Results);
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.groups.len(), 2);
            assert_eq!(result.accuracy, 1.0, "every call was answered correctly");
            for group in &result.groups {
                assert!(
                    cw_core::parse_callsign(&group.sent).is_some(),
                    "{:?} is not a callsign",
                    group.sent
                );
            }
            // The tier is what this session was run at, not the Koch level.
            assert_eq!(result.level, cw_core::CALLSIGN_TIER_MAX);
            assert_eq!(result.char_set_mode, CharSetMode::Callsign);
            assert_eq!(h.sessions.peek().len(), 1);
            // And what was sent is what was played.
            assert_eq!(h.texts().len(), 2);
            for text in h.texts() {
                assert!(cw_core::parse_callsign(&text).is_some(), "{text:?}");
            }
        });
    }

    /// A callsign session must not teach the group sampler, or a tier-6 call
    /// full of `Q` and `Z` would drag the Koch curriculum around behind it.
    #[test]
    fn callsign_history_stays_out_of_the_group_sampler() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(120_000).await;
            let stored = h.sessions.peek().clone();
            assert_eq!(stored.len(), 1);

            let mut koch = test_settings();
            koch.curriculum.char_set_mode = CharSetMode::Koch;
            assert!(
                !stored[0].usable_for_sampling(&koch),
                "a callsign session must not seed the Koch sampler"
            );
            let mut callsigns = test_settings();
            callsigns.curriculum.char_set_mode = CharSetMode::Callsign;
            callsigns.curriculum.callsign_level = cw_core::CALLSIGN_TIER_MAX;
            assert!(
                stored[0].usable_for_sampling(&callsigns),
                "but every tier shares one callsign history"
            );
        });
    }

    /// A station repeating its call is the same station. Redrawing the tone,
    /// speed and strength on every send made three repeats of one group arrive
    /// as three different operators — which is what the screen promises they
    /// are not, and which makes a repeat useless for confirming what you heard.
    #[test]
    fn every_repeat_of_a_group_comes_from_the_same_station() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            // Ranges wide enough that a redraw could not pass for a repeat.
            settings.playback.char_wpm_min = 15.0;
            settings.playback.char_wpm_max = 40.0;
            settings.playback.link_char_to_effective = true;
            settings.band.side_tone_min = 400.0;
            settings.band.side_tone_max = 900.0;
            settings.band.volume_min = 0.2;
            settings.band.volume_max = 1.0;
            settings.band.link_volume = false;

            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(200_000).await;
            assert_eq!(h.screen(), Screen::Results);

            let voices = h.recorder.voices();
            let texts = h.texts();
            assert_eq!(voices.len(), texts.len());
            assert_eq!(texts.len(), 6, "two groups, three sends each");

            // Every send of one group is the same operator...
            for group in texts.chunks(3).zip(voices.chunks(3)) {
                let (group_texts, group_voices) = group;
                assert!(
                    group_texts.windows(2).all(|w| w[0] == w[1]),
                    "a group's repeats should be the same text: {group_texts:?}"
                );
                assert!(
                    group_voices.windows(2).all(|w| w[0] == w[1]),
                    "a group's repeats changed station: {group_voices:?}"
                );
            }
            // ...and the next group is somebody else.
            assert_ne!(
                voices[0], voices[3],
                "every group came from the same station, which is its own problem"
            );
        });
    }

    /// A pile-up changes what you hear, not what you answer. The session still
    /// scores the station you were copying, and the others are only ever in the
    /// way — which is the whole point of them.
    #[test]
    fn a_pile_up_is_heard_but_never_answered() {
        run(|| async {
            let mut settings = test_settings();
            settings.band.stations_max = cw_core::STATIONS_MAX;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(200_000).await;
            assert_eq!(h.screen(), Screen::Results);

            let sent = h.recorder.sent();
            assert!(!sent.is_empty());
            assert!(
                sent.iter().any(|t| !t.others.is_empty()),
                "with five allowed, somebody else should have called"
            );
            for transmission in &sent {
                for other in &transmission.others {
                    assert_ne!(
                        other.text, transmission.text,
                        "an interfering station sent the answer"
                    );
                    assert!(
                        other.voice.volume < transmission.voice.volume,
                        "an interfering station was the strongest"
                    );
                }
            }

            // Every group was answered correctly, because the pile-up is not
            // part of the answer.
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.accuracy, 1.0);
            assert_eq!(h.sessions.peek().len(), 1);
        });
    }

    /// A repeat is the same moment on the band, pile-up and all. Redrawing the
    /// others would make a repeat a different puzzle rather than another look
    /// at the same one.
    #[test]
    fn a_repeat_brings_back_the_same_pile_up() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            settings.band.stations_max = cw_core::STATIONS_MAX;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(200_000).await;

            let sent = h.recorder.sent();
            assert_eq!(sent.len(), 6, "two groups, three sends each");
            for group in sent.chunks(3) {
                assert!(
                    group.windows(2).all(|w| w[0] == w[1]),
                    "a repeat brought a different pile-up"
                );
            }
            assert_ne!(sent[0], sent[3], "both groups drew the same band");
        });
    }

    #[test]
    fn a_broken_stream_is_retried_before_the_session_is_given_up() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::FailMidSend);
            h.start_training();
            assert!(h.run_until(30_000, |h| h.screen() == Screen::Home).await);
            // Three attempts, each on a freshly built player.
            assert_eq!(h.texts().len(), 3);
            assert!(h.recorder.players_built.get() >= 3);
            assert!(h.toast().is_some());
        });
    }

    #[test]
    fn a_player_that_refuses_to_start_is_reported_once_the_retries_run_out() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::RefuseToStart);
            h.start_training();
            assert!(h.run_until(30_000, |h| h.screen() == Screen::Home).await);
            assert_eq!(h.toast().as_deref(), Some("Audio stream: device is gone"));
        });
    }

    /// A device that comes back mid-session must not cost the user the session.
    #[test]
    fn a_send_that_fails_once_and_then_works_carries_on() {
        run(|| async {
            let mut h = Harness::new();
            // The first send breaks; the device is back for the second.
            h.recorder.fail_next(1);
            h.start_training();
            assert!(
                h.run_until(20_000, |h| h.awaiting_answer() == Some(0))
                    .await
            );
            assert_eq!(h.texts().len(), 2, "the group is sent again, not skipped");
            assert!(h.recorder.players_built.get() >= 2, "the player is rebuilt");
            assert_eq!(h.screen(), Screen::Training);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            assert!(
                h.run_until(2_000, |h| h
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.confirmed_flags()[0]))
                    .await
            );
        });
    }

    /// A hard audio failure after something has been scored keeps the score.
    #[test]
    fn audio_that_dies_after_a_scored_group_still_shows_the_results() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            assert!(
                h.run_until(1_000, |h| h
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.confirmed_flags()[0]))
                    .await
            );
            h.set_behaviour(Behaviour::RefuseToStart);
            assert!(h.run_until(30_000, |h| h.screen() == Screen::Results).await);
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.groups.len(), 1);
        });
    }

    #[test]
    fn a_confirmed_group_teaches_the_sampler() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            let before = h.app.sampling.borrow().beliefs.clone();
            h.type_answer(0, &sent);
            assert!(
                h.run_until(1_000, |h| h
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.confirmed_flags()[0]))
                    .await
            );
            let after = h.app.sampling.borrow().beliefs.clone();
            assert_ne!(before, after, "a correct copy should raise the belief");
            for ch in sent.chars() {
                let belief = after.get(&ch).copied().expect("belief for a sent letter");
                assert!(belief.alpha > 1.0, "{ch} should have been credited");
            }
        });
    }

    #[test]
    fn an_event_for_a_session_that_is_gone_is_ignored() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let stale_gen = h.app.session_gen.get();
            let app = h.app.clone();
            let runtime = h.runtime;
            // The user leaves; an old effect chain then reports its send ended.
            h.send(SessionEvent::Abort);
            let effects = h.in_app(|| {
                dispatch_event(
                    &app,
                    runtime,
                    SessionEvent::PlaybackEnded {
                        index: 0,
                        duration_sec: 1.0,
                        char_wpm: 20.0,
                        effective_wpm: 20.0,
                    },
                    stale_gen,
                )
            });
            assert!(effects.is_empty());
            assert_eq!(h.screen(), Screen::Home);
        });
    }

    #[test]
    fn booting_a_session_that_was_already_cancelled_builds_nothing() {
        run(|| async {
            let h = Harness::new();
            let app = h.app.clone();
            let gen = app.session_gen.get();
            // Something else claimed the audio between the two calls.
            app.bump_session();
            let signals = h.signals();
            let effects =
                h.in_app(|| boot_machine_session(test_settings(), &[], &app, gen, signals));
            assert!(effects.is_none());
            assert!(app.machine.borrow().is_none());
            assert_eq!(h.screen(), Screen::Home);
        });
    }

    #[test]
    fn a_session_at_the_end_of_its_groups_does_not_ask_for_another() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.texts().len(), 1);
        });
    }

    #[test]
    fn an_event_with_no_machine_behind_it_does_nothing() {
        run(|| async {
            let h = Harness::new();
            let app = h.app.clone();
            let gen = app.session_gen.get();
            let effects = h.in_app(|| dispatch_event(&app, h.runtime, SessionEvent::Confirm, gen));
            assert!(effects.is_empty());
        });
    }

    #[test]
    fn an_event_from_an_older_session_never_reaches_the_new_machine() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let app = h.app.clone();
            // The machine belongs to the current generation; ask it to accept an
            // event stamped with a different one.
            let effects = h.in_app(|| {
                dispatch_event(
                    &app,
                    h.runtime,
                    SessionEvent::Abort,
                    app.session_gen.get() + 1,
                )
            });
            assert!(effects.is_empty());
            assert_eq!(h.screen(), Screen::Training);
        });
    }

    /// Stopping the audio without ending the session is what a stray stop looks
    /// like from inside: the send reports cancelled and the session is closed
    /// with whatever it had, rather than sitting in silence.
    #[test]
    fn a_send_stopped_from_outside_ends_the_session_cleanly() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(100).await;
            h.app.stop_sending();
            assert!(h.run_until(5_000, |h| h.screen() == Screen::Home).await);
            assert!(h.sessions.peek().is_empty());
        });
    }

    #[test]
    fn a_group_with_no_text_is_not_sent_at_all() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let app = h.app.clone();
            let gen = app.session_gen.get();
            h.recorder.clear();
            // An empty group can only come of a pool that generated nothing;
            // playing it must not stall the session waiting for silence.
            h.in_app(|| {
                if let Some(machine) = app.machine.borrow_mut().as_mut() {
                    machine.set_group_text(0, String::new(), 1);
                }
            });
            let signals = h.signals();
            let settings_now = h.settings.peek().clone();
            h.in_app(|| {
                spawn_effects(
                    vec![SessionEffect::Play {
                        index: 0,
                        text: String::new(),
                    }],
                    settings_now,
                    app.clone(),
                    gen,
                    signals,
                )
            });
            h.pump();
            h.advance(200).await;
            assert!(h.texts().is_empty(), "silence is not sent to the player");
        });
    }

    #[test]
    fn a_stale_sleep_does_not_move_the_session_on() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let app = h.app.clone();
            let gen = app.session_gen.get();
            let settings_now = h.settings.peek().clone();
            let signals = h.signals();
            // A sleep the machine has already forgotten about.
            h.in_app(|| {
                spawn_effects(
                    vec![SessionEffect::Sleep { id: 999, ms: 20 }],
                    settings_now,
                    app.clone(),
                    gen,
                    signals,
                )
            });
            h.pump();
            h.advance(200).await;
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
        });
    }

    #[test]
    fn effects_from_a_session_that_is_over_are_dropped() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let app = h.app.clone();
            let gen = app.session_gen.get();
            let settings_now = h.settings.peek().clone();
            let signals = h.signals();
            h.send(SessionEvent::Abort);
            h.recorder.clear();
            h.in_app(|| {
                spawn_effects(
                    vec![
                        SessionEffect::NeedGroup { index: 1 },
                        SessionEffect::Play {
                            index: 1,
                            text: "KM".into(),
                        },
                    ],
                    settings_now,
                    app.clone(),
                    gen,
                    signals,
                )
            });
            h.pump();
            h.advance(500).await;
            assert!(h.texts().is_empty());
            assert_eq!(h.screen(), Screen::Home);
        });
    }

    #[test]
    fn asking_for_a_group_a_finished_session_no_longer_needs_does_nothing() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            // The machine is gone; an effect that arrives late finds nothing.
            let app = h.app.clone();
            let gen = app.session_gen.get();
            let settings_now = h.settings.peek().clone();
            let signals = h.signals();
            h.in_app(|| {
                spawn_effects(
                    vec![SessionEffect::NeedGroup { index: 0 }],
                    settings_now,
                    app.clone(),
                    gen,
                    signals,
                )
            });
            h.pump();
            h.advance(100).await;
            assert_eq!(h.screen(), Screen::Results);
        });
    }

    #[test]
    fn a_session_plays_its_first_group_and_opens_the_answer_box() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert_eq!(h.screen(), Screen::Training);
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
            let sent = h.sent(0);
            assert_eq!(sent.chars().count(), 2);
            assert_eq!(h.texts(), vec![sent.clone()]);
            // The answer box stays shut until the send is over.
            assert!(h.runtime.peek().as_ref().is_some_and(|s| s.input_locked(0)));

            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            assert!(!h.runtime.peek().as_ref().is_some_and(|s| s.input_locked(0)));
        });
    }
}
