//! Audio-less player for builds with no output backend.
//!
//! A send is waited on for the length of its plan, as on the real backends,
//! so a session keeps its Farnsworth pacing, timing and scoring — it just
//! never makes a sound. A contest's turn is waited on to its last reply, as
//! its plan stretches it.

use std::rc::Rc;

use cw_core::{PlannedTransmission, TrainingSettings, Transmission};

use super::epoch::{Epoch, Ticket};
use super::{MorseBackend, PlaybackSignal, SendTiming, WaitFlags};
use crate::time::mono_ms;

pub struct MorsePlayer {
    epoch: Epoch,
}

struct SilentSignal {
    ticket: Ticket,
    /// When the send starts, on the app's monotonic clock: later than it
    /// was started for one asked to start at a given time.
    started: u64,
    /// How long after it was started it starts.
    wait_ms: u64,
}

impl PlaybackSignal for SilentSignal {
    fn timing(&self) -> SendTiming {
        // Nothing sounds, so nothing is late to the ear.
        SendTiming {
            lead_sec: self.wait_ms as f64 / 1_000.0,
            latency_sec: 0.0,
        }
    }

    fn poll(&self) -> WaitFlags {
        let elapsed = mono_ms().saturating_sub(self.started);
        WaitFlags {
            cancelled: self.ticket.is_cancelled(),
            finished: false,
            failed: false,
            // There is no audio to park, so a send always runs to its length.
            suspended: false,
            // Nothing is playing, so the app's own clock is the only clock
            // there is.
            played_ms: Some(u32::try_from(elapsed).unwrap_or(u32::MAX)),
            late: false,
        }
    }
}

impl MorsePlayer {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            epoch: Epoch::default(),
        })
    }
}

impl MorseBackend for MorsePlayer {
    fn apply_band(&mut self, _settings: &TrainingSettings) {}

    fn start(
        &mut self,
        _transmission: &Transmission,
        _plan: &PlannedTransmission,
        _settings: &TrainingSettings,
        heard_in: Option<f64>,
    ) -> Result<Rc<dyn PlaybackSignal>, String> {
        // Nothing is heard: the plan's length, which the wait already has,
        // is all there is to a send, from when it was to start.
        let wait_ms = heard_in.map_or(0, |heard_in| (heard_in.max(0.0) * 1_000.0).round() as u64);
        Ok(Rc::new(SilentSignal {
            ticket: self.epoch.arm(),
            started: mono_ms() + wait_ms,
            wait_ms,
        }))
    }

    fn stop(&mut self) {
        self.epoch.bump();
    }

    fn shutdown(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::wait::Waited;
    use crate::audio::{PlaybackOutcome, start_planned};
    use crate::testing::{run, test_settings};
    use cw_core::{Segment, Sender, Turn, plan_morse_playback_for, plan_turn, resolve_station};

    /// Your message, then a reply that runs on well past it.
    fn stretched_turn(settings: &TrainingSettings) -> Turn {
        let mut rng = cw_core::SplitMix64::new(5);
        let you = resolve_station(settings, &mut rng);
        let mut slow = resolve_station(settings, &mut rng);
        slow.char_wpm = 15.0;
        slow.effective_wpm = 15.0;
        let segment = |who, text: &str, voice, start_sec: f64| Segment {
            who,
            text: text.to_string(),
            voice,
            start_sec,
            end_sec: start_sec + plan_morse_playback_for(text, settings, &voice).duration_sec,
            fade: None,
            skip_sec: 0.0,
        };
        let mine = segment(Sender::You, "CQ TEST", you, 0.0);
        let reply = segment(Sender::Caller(0), "K5XYZ", slow, mine.end_sec + 0.3);
        Turn {
            id: 1,
            at_run_ms: 0,
            starts_at: None,
            segments: vec![mine, reply],
        }
    }

    /// A turn is waited on until its last reply has been sent, not just your
    /// message, and a stop cancels it wherever it is.
    #[test]
    fn a_stretched_turn_plays_to_its_planned_length_and_a_stop_cancels_it() {
        run(|| async {
            let settings = test_settings();
            let turn = stretched_turn(&settings);
            let plan = plan_turn(&turn, &settings);
            let transcript = turn.transcript().expect("a turn with something in it");
            let mine_ms = (turn.segments[0].end_sec * 1_000.0) as u64;
            let planned_ms = (plan.wanted.duration_sec * 1_000.0).ceil() as u64;
            assert!(
                planned_ms > mine_ms + 300,
                "the reply runs past your message"
            );

            let mut player = MorsePlayer::new().expect("a player");
            let started = mono_ms();
            let wait =
                start_planned(&mut player, &plan, &transcript, &settings, None).expect("sent");
            assert_eq!(wait.start().latency_ms, 0);
            assert_eq!(
                wait.wait().await,
                Waited {
                    outcome: PlaybackOutcome::Completed,
                    held_up: false,
                }
            );
            let took = mono_ms() - started;
            assert!(
                (planned_ms..planned_ms + 100).contains(&took),
                "a {planned_ms} ms turn took {took} ms"
            );

            // Part way through, it is stopped.
            let wait =
                start_planned(&mut player, &plan, &transcript, &settings, None).expect("sent");
            let mut waiting = std::pin::pin!(wait.wait());
            let first =
                std::future::poll_fn(|cx| std::task::Poll::Ready(waiting.as_mut().poll(cx))).await;
            assert!(first.is_pending());
            crate::time::sleep_ms(200).await;
            player.stop();
            assert_eq!(waiting.await.outcome, PlaybackOutcome::Cancelled);
        });
    }
}
