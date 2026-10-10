use std::cell::Cell;
use std::rc::Rc;

use cw_core::{
    CharSamplingState, FastrandRng, TrainingSettings, Transmission, compute_after_group_gap_ms,
    generate_training_group,
};
use dioxus::prelude::*;

use crate::audio::PlaybackOutcome;
use crate::state::AppState;
use crate::time::{POLL_MS, seed_rng, sleep_ms};

/// How many sent groups the stream keeps on screen for a glance-back.
const STREAM_HEARD_KEEP: usize = 12;

/// How many times one group is re-armed before the session is told the audio
/// is gone. Each retry rebuilds the player, which is what picks up a device
/// that was swapped, unplugged or suspended mid-session.
const PLAY_ATTEMPTS: u32 = 3;

#[derive(Debug)]
pub(crate) enum PlayError {
    Cancelled,
    Failed(String),
}

pub(crate) async fn play_text_now(
    app: &AppState,
    generation: u64,
    transmission: &Transmission,
    settings: &TrainingSettings,
) -> Result<(f64, f64, f64), PlayError> {
    let mut last_err = None;
    for attempt in 0..PLAY_ATTEMPTS {
        if app.session_gen.get() != generation {
            return Err(PlayError::Cancelled);
        }
        if attempt > 0 {
            let rebuilt = app.rebuild_player(settings);
            sleep_ms(POLL_MS).await;
            if app.session_gen.get() != generation {
                return Err(PlayError::Cancelled);
            }
            if let Err(err) = rebuilt {
                // There is no player to send through, and the reason it could
                // not be opened is the one worth showing — not "unavailable".
                last_err = Some(err);
                continue;
            }
        }
        match schedule_text(app, generation, transmission, settings).await {
            Ok(wait) => {
                let duration = wait.duration_sec;
                let char_wpm = wait.char_wpm;
                let effective_wpm = wait.effective_wpm;
                match wait.wait().await {
                    PlaybackOutcome::Completed => {
                        if app.session_gen.get() != generation {
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
                if app.session_gen.get() != generation {
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
    generation: u64,
    transmission: &Transmission,
    settings: &TrainingSettings,
) -> Result<crate::audio::PlaybackWait, String> {
    if app.session_gen.get() != generation {
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
        if app.session_gen.get() != generation {
            return Err("Cancelled.".into());
        }
    }
    for _ in 0..8 {
        if app.session_gen.get() != generation {
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

pub async fn sleep_cancelable(ms: u32, generation: u64, session_gen: Rc<Cell<u64>>) -> bool {
    let mut left = ms.max(1);
    while left > 0 {
        if session_gen.get() != generation {
            return false;
        }
        let chunk = left.min(POLL_MS);
        sleep_ms(chunk).await;
        left = left.saturating_sub(chunk);
    }
    session_gen.get() == generation
}

pub async fn play_chars(
    app: AppState,
    generation: u64,
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
        if app.session_gen.get() != generation {
            return;
        }
        let alone = Transmission::alone(ch.to_string(), voice);
        let play = play_text_now(&app, generation, &alone, &settings).await;
        if app.session_gen.get() != generation {
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
        if i + 1 < chars.len()
            && !sleep_cancelable(gap_ms, generation, app.session_gen.clone()).await
        {
            return;
        }
    }
}

/// Send one short sample once — used by the keying-envelope test chips.
pub async fn play_sample_text(
    app: AppState,
    generation: u64,
    settings: TrainingSettings,
    text: String,
    mut toast: Signal<Option<String>>,
) {
    let alone = Transmission::alone(text, app.station(&settings));
    match play_text_now(&app, generation, &alone, &settings).await {
        Ok(_) | Err(PlayError::Cancelled) => {}
        Err(PlayError::Failed(message)) => toast.set(Some(message)),
    }
}

pub async fn loop_preview_text(
    app: AppState,
    generation: u64,
    settings: Signal<TrainingSettings>,
    text: &'static str,
    gap_ms: u32,
    mut toast: Signal<Option<String>>,
) {
    // One station calling, so moving a band slider changes the band and not
    // the signal you are judging it against.
    let voice = app.station(&settings().clamp());
    loop {
        if app.session_gen.get() != generation {
            return;
        }
        let settings_now = settings().clamp();
        let alone = Transmission::alone(text, voice);
        if let Err(err) = play_text_now(&app, generation, &alone, &settings_now).await {
            match err {
                PlayError::Cancelled => return,
                PlayError::Failed(message) => {
                    toast.set(Some(message));
                    return;
                }
            }
        }
        if !sleep_cancelable(gap_ms, generation, app.session_gen.clone()).await {
            return;
        }
    }
}

fn next_stream_group(
    app: &AppState,
    settings: &TrainingSettings,
    sampling: &mut CharSamplingState,
) -> String {
    let (group, next) = match app.rng.try_borrow_mut() {
        Ok(mut rng) => generate_training_group(settings, sampling, &mut *rng),
        Err(_) => generate_training_group(settings, sampling, &mut FastrandRng(seed_rng())),
    };
    *sampling = next;
    group
}

/// Send groups from the current alphabet until the generation moves on.
///
/// This is not a training session: sampling is copied so the stream cannot
/// change what the next scored session draws, and nothing is stored.
pub async fn loop_stream_groups(
    app: AppState,
    generation: u64,
    settings: Signal<TrainingSettings>,
    mut heard: Signal<Vec<String>>,
    mut toast: Signal<Option<String>>,
) {
    let mut sampling = app.sampling.borrow().clone();
    let voice = app.station(&settings().clamp());
    loop {
        if app.session_gen.get() != generation {
            return;
        }
        let settings_now = settings().clamp();
        let group = next_stream_group(&app, &settings_now, &mut sampling);
        if group.is_empty() {
            if !sleep_cancelable(POLL_MS, generation, app.session_gen.clone()).await {
                return;
            }
            continue;
        }
        let alone = Transmission::alone(group.clone(), voice);
        let played = play_text_now(&app, generation, &alone, &settings_now).await;
        if app.session_gen.get() != generation {
            return;
        }
        let (char_wpm, effective_wpm) = match played {
            Ok((_, char_wpm, effective_wpm)) => (char_wpm, effective_wpm),
            Err(PlayError::Cancelled) => return,
            Err(PlayError::Failed(message)) => {
                toast.set(Some(message));
                return;
            }
        };
        let mut list = heard();
        list.push(group);
        let extra = list.len().saturating_sub(STREAM_HEARD_KEEP);
        if extra > 0 {
            list.drain(..extra);
        }
        heard.set(list);
        let gap = compute_after_group_gap_ms(
            char_wpm,
            effective_wpm,
            settings_now.playback.extra_word_space_multiplier,
            settings_now.playback.group_pause_sec,
        );
        if !sleep_cancelable(gap, generation, app.session_gen.clone()).await {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::Behaviour;
    use crate::testing::{Harness, run, test_settings};

    #[test]
    fn a_cancelable_sleep_stops_the_moment_the_session_moves_on() {
        run(|| async {
            let h = Harness::new();
            let generation = h.app.session_gen.get();
            let slept = sleep_cancelable(0, generation, h.app.session_gen.clone()).await;
            assert!(slept, "a zero wait still completes");

            h.app.bump_session();
            let slept = sleep_cancelable(500, generation, h.app.session_gen.clone()).await;
            assert!(!slept, "a stale generation must not keep waiting");
        });
    }

    #[test]
    fn listening_plays_one_character_at_a_time() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let generation = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_chars(
                    app,
                    generation,
                    settings,
                    "KM".into(),
                    200,
                    toast,
                ));
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
            let generation = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_chars(
                    app,
                    generation,
                    settings,
                    "KMU".into(),
                    400,
                    toast,
                ));
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
            let generation = h.app.takeover_audio(&settings).expect("player");
            h.set_behaviour(Behaviour::RefuseToStart);
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_chars(
                    app,
                    generation,
                    settings,
                    "KM".into(),
                    200,
                    toast,
                ));
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
            let generation = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_sample_text(
                    app,
                    generation,
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
            let generation = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            h.in_app(|| {
                spawn(play_sample_text(
                    app,
                    generation,
                    settings,
                    "E".into(),
                    toast,
                ));
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
            let generation = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let (settings_sig, toast) = (h.settings, h.toast);
            h.in_app(|| {
                spawn(loop_preview_text(
                    app,
                    generation,
                    settings_sig,
                    "CQ",
                    100,
                    toast,
                ));
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
            let generation = h.app.takeover_audio(&settings).expect("player");
            h.set_behaviour(Behaviour::RefuseToStart);
            let app = h.app.clone();
            let (settings_sig, toast) = (h.settings, h.toast);
            h.in_app(|| {
                spawn(loop_preview_text(
                    app,
                    generation,
                    settings_sig,
                    "CQ",
                    100,
                    toast,
                ));
            });
            h.pump();
            assert!(h.run_until(20_000, |h| h.toast().is_some()).await);
            let played = h.texts().len();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), played, "the loop stopped");
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
            let generation = h.app.takeover_audio(&settings).expect("player");
            // Something else is holding the player: every attempt to schedule a
            // send has to give up instead of blocking the session.
            let player = h.app.player.clone();
            let held = player.borrow_mut();
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_sample_text(
                    app,
                    generation,
                    settings,
                    "E".into(),
                    toast,
                ));
            });
            h.pump();
            assert!(h.run_until(30_000, |h| h.toast().is_some()).await);
            assert_eq!(h.toast().as_deref(), Some("Audio is busy."));
            drop(held);
        });
    }

    #[test]
    fn a_send_with_no_player_left_rebuilds_one() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let generation = h.app.takeover_audio(&settings).expect("player");
            // The player is gone, the way a shutdown leaves it.
            *h.app.player.borrow_mut() = None;
            let app = h.app.clone();
            let toast = h.toast;
            h.in_app(|| {
                spawn(play_sample_text(
                    app,
                    generation,
                    settings,
                    "E".into(),
                    toast,
                ));
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
            let generation = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let outcome = std::rc::Rc::new(std::cell::RefCell::new(None));
            let sink = std::rc::Rc::clone(&outcome);
            h.in_app(|| {
                spawn(async move {
                    let alone = Transmission::alone("CQ", app.station(&settings));
                    let result = play_text_now(&app, generation, &alone, &settings).await;
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
            let generation = h.app.takeover_audio(&settings).expect("player");
            // The audio was claimed by something else before the send began.
            h.app.bump_session();
            h.recorder.clear();
            let app = h.app.clone();
            let outcome = std::rc::Rc::new(std::cell::RefCell::new(None));
            let sink = std::rc::Rc::clone(&outcome);
            h.in_app(|| {
                spawn(async move {
                    let alone = Transmission::alone("CQ", app.station(&settings));
                    *sink.borrow_mut() =
                        Some(play_text_now(&app, generation, &alone, &settings).await);
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
    fn a_group_stream_keeps_sending_until_it_is_stopped() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let sampling_before = h.app.sampling.borrow().clone();
            let generation = h.app.takeover_audio(&settings).expect("player");
            let app = h.app.clone();
            let (settings_sig, toast) = (h.settings, h.toast);
            let heard = h.in_app(|| Signal::new(Vec::<String>::new()));
            h.in_app(|| {
                spawn(loop_stream_groups(
                    app,
                    generation,
                    settings_sig,
                    heard,
                    toast,
                ));
            });
            h.pump();
            assert!(h.run_until(20_000, |h| h.texts().len() >= 3).await);
            assert!(
                h.texts().iter().all(|text| text.len() == 2),
                "each send is one training group"
            );
            assert!(
                heard.peek().len() >= 2,
                "finished groups show up after they play"
            );
            h.app.bump_session();
            let played = h.texts().len();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), played, "the stream stopped");
            assert_eq!(
                *h.app.sampling.borrow(),
                sampling_before,
                "a listen stream must not change what training will draw"
            );
        });
    }

    #[test]
    fn a_group_stream_gives_up_when_the_audio_fails() {
        run(|| async {
            let mut h = Harness::new();
            let settings = test_settings();
            let generation = h.app.takeover_audio(&settings).expect("player");
            h.set_behaviour(Behaviour::RefuseToStart);
            let app = h.app.clone();
            let (settings_sig, toast) = (h.settings, h.toast);
            let heard = h.in_app(|| Signal::new(Vec::<String>::new()));
            h.in_app(|| {
                spawn(loop_stream_groups(
                    app,
                    generation,
                    settings_sig,
                    heard,
                    toast,
                ));
            });
            h.pump();
            assert!(h.run_until(20_000, |h| h.toast().is_some()).await);
            let played = h.texts().len();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), played, "the stream stopped");
            assert!(heard.peek().is_empty());
        });
    }
}
