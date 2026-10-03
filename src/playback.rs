use std::cell::Cell;
use std::rc::Rc;

use cw_core::{TrainingSettings, Transmission};
use dioxus::prelude::*;

use crate::audio::PlaybackOutcome;
use crate::state::AppState;
use crate::time::{sleep_ms, POLL_MS};

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
    use crate::audio::fake::Behaviour;
    use crate::testing::{run, test_settings, Harness};

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
}
