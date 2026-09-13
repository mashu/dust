//! Waiting for a send to finish, without trusting the wall clock.
//!
//! Every backend reports the same [`WaitFlags`]; [`next_wait_step`] turns them
//! into a decision. Keeping that decision here — pure, and away from the audio
//! APIs — is what makes the awkward cases testable: a send that is cancelled
//! mid-flight, a device that stops calling back, and a browser tab whose audio
//! clock freezes while `setTimeout` keeps ticking.

use std::rc::Rc;

use crate::time::{sleep_ms, POLL_MS};

/// How long a send may overrun its own duration before it is called stalled.
/// Long enough to cover a slow device start and a throttled timer, short
/// enough that a session does not sit in silence.
pub const STALL_GRACE_MS: u32 = 2_000;

/// Scheduling a send takes a moment, and the plan's duration ends on the last
/// symbol. This tail keeps the answer window from opening on top of the tone.
pub const PLAYBACK_TAIL_MS: u32 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackOutcome {
    Completed,
    /// Something else took the audio over: a new session, a stop, a new send.
    Cancelled,
    /// The send never reached its end — a dead stream, or an audio clock that
    /// stopped moving. The caller rebuilds the player and tries again.
    Failed,
}

/// What a backend knows about the send in flight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WaitFlags {
    /// Stopped, or superseded by a newer send.
    pub cancelled: bool,
    /// The backend says the audio ran out.
    pub finished: bool,
    /// The backend says the stream is broken.
    pub failed: bool,
    /// Audio-clock progress, for backends that have one. `None` means the
    /// backend can only report `finished`.
    pub played_ms: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitStep {
    KeepWaiting,
    Done(PlaybackOutcome),
}

/// One decision about a send in flight.
///
/// Cancellation wins over everything: it is the app taking the audio back, and
/// the caller has already moved on. A failure outranks a completion because a
/// broken stream that also reports "finished" has not really played anything.
pub fn next_wait_step(
    flags: WaitFlags,
    waited_ms: u32,
    duration_ms: u32,
    grace_ms: u32,
) -> WaitStep {
    if flags.cancelled {
        return WaitStep::Done(PlaybackOutcome::Cancelled);
    }
    if flags.failed {
        return WaitStep::Done(PlaybackOutcome::Failed);
    }
    if flags.finished {
        return WaitStep::Done(PlaybackOutcome::Completed);
    }
    let target = duration_ms.saturating_add(PLAYBACK_TAIL_MS);
    if let Some(played) = flags.played_ms {
        if played >= target {
            return WaitStep::Done(PlaybackOutcome::Completed);
        }
    }
    // The send has outlived its own length by the grace period without the
    // audio reporting it got there. The tone is not playing: a suspended
    // AudioContext, a device that went away, a stream that stopped calling
    // back. Waiting longer only makes the silence longer.
    if waited_ms >= target.saturating_add(grace_ms) {
        return WaitStep::Done(PlaybackOutcome::Failed);
    }
    WaitStep::KeepWaiting
}

/// A send in flight, as the backend sees it.
pub trait PlaybackSignal {
    fn poll(&self) -> WaitFlags;
}

/// Morse already scheduled; wait without holding the player borrow.
pub struct PlaybackWait {
    pub duration_sec: f64,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    signal: Rc<dyn PlaybackSignal>,
}

impl PlaybackWait {
    pub fn new(
        duration_sec: f64,
        char_wpm: f64,
        effective_wpm: f64,
        signal: Rc<dyn PlaybackSignal>,
    ) -> Self {
        Self {
            duration_sec,
            char_wpm,
            effective_wpm,
            signal,
        }
    }

    fn duration_ms(&self) -> u32 {
        let ms = (self.duration_sec * 1000.0).ceil();
        if ms.is_finite() && ms > 0.0 {
            ms.min(f64::from(u32::MAX / 4)) as u32
        } else {
            0
        }
    }

    pub async fn wait(self) -> PlaybackOutcome {
        let duration_ms = self.duration_ms();
        let mut waited = 0u32;
        loop {
            match next_wait_step(self.signal.poll(), waited, duration_ms, STALL_GRACE_MS) {
                WaitStep::Done(outcome) => return outcome,
                WaitStep::KeepWaiting => {}
            }
            sleep_ms(POLL_MS).await;
            waited = waited.saturating_add(POLL_MS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn flags() -> WaitFlags {
        WaitFlags::default()
    }

    #[test]
    fn a_send_in_flight_keeps_waiting() {
        assert_eq!(
            next_wait_step(flags(), 0, 1_000, 2_000),
            WaitStep::KeepWaiting
        );
        let mut playing = flags();
        playing.played_ms = Some(500);
        assert_eq!(
            next_wait_step(playing, 500, 1_000, 2_000),
            WaitStep::KeepWaiting
        );
    }

    #[test]
    fn the_audio_clock_decides_when_a_send_is_over() {
        let mut done = flags();
        done.played_ms = Some(1_000 + PLAYBACK_TAIL_MS);
        assert_eq!(
            next_wait_step(done, 1, 1_000, 2_000),
            WaitStep::Done(PlaybackOutcome::Completed)
        );
        // The wall clock on its own never completes a send.
        assert_eq!(
            next_wait_step(flags(), 1_500, 1_000, 2_000),
            WaitStep::KeepWaiting
        );
    }

    #[test]
    fn a_backend_that_only_reports_the_end_is_believed() {
        let mut done = flags();
        done.finished = true;
        assert_eq!(
            next_wait_step(done, 10, 10_000, 2_000),
            WaitStep::Done(PlaybackOutcome::Completed)
        );
    }

    #[test]
    fn a_stalled_audio_clock_is_a_failure_not_a_finished_group() {
        // A suspended AudioContext: the wall clock runs, the audio clock does not.
        let mut stalled = flags();
        stalled.played_ms = Some(0);
        assert_eq!(
            next_wait_step(stalled, 3_000, 1_000, 2_000),
            WaitStep::KeepWaiting
        );
        assert_eq!(
            next_wait_step(stalled, 3_024, 1_000, 2_000),
            WaitStep::Done(PlaybackOutcome::Failed)
        );
        // Same for a device that stopped calling back.
        assert_eq!(
            next_wait_step(flags(), 3_030, 1_000, 2_000),
            WaitStep::Done(PlaybackOutcome::Failed)
        );
    }

    #[test]
    fn a_cancelled_send_outranks_everything_else() {
        let mut all = flags();
        all.cancelled = true;
        all.finished = true;
        all.failed = true;
        all.played_ms = Some(99_999);
        assert_eq!(
            next_wait_step(all, 99_999, 1_000, 2_000),
            WaitStep::Done(PlaybackOutcome::Cancelled)
        );
    }

    #[test]
    fn a_broken_stream_outranks_a_finished_one() {
        let mut broken = flags();
        broken.failed = true;
        broken.finished = true;
        broken.played_ms = Some(99_999);
        assert_eq!(
            next_wait_step(broken, 10, 1_000, 2_000),
            WaitStep::Done(PlaybackOutcome::Failed)
        );
    }

    struct Scripted {
        steps: RefCell<Vec<WaitFlags>>,
    }

    impl PlaybackSignal for Scripted {
        fn poll(&self) -> WaitFlags {
            let mut steps = self.steps.borrow_mut();
            if steps.len() > 1 {
                steps.remove(0)
            } else {
                steps.first().copied().unwrap_or_default()
            }
        }
    }

    fn run(future: impl std::future::Future<Output = PlaybackOutcome>) -> PlaybackOutcome {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .expect("runtime")
            .block_on(future)
    }

    fn scripted(duration_sec: f64, steps: Vec<WaitFlags>) -> PlaybackWait {
        PlaybackWait::new(
            duration_sec,
            20.0,
            18.0,
            Rc::new(Scripted {
                steps: RefCell::new(steps),
            }),
        )
    }

    #[test]
    fn waiting_polls_until_the_audio_says_it_is_done() {
        let mut playing = flags();
        playing.played_ms = Some(0);
        let mut done = flags();
        done.finished = true;
        let outcome = run(scripted(0.1, vec![playing, playing, done]).wait());
        assert_eq!(outcome, PlaybackOutcome::Completed);
    }

    #[test]
    fn waiting_gives_up_on_a_send_that_never_starts() {
        let stalled = WaitFlags {
            played_ms: Some(0),
            ..Default::default()
        };
        assert_eq!(
            run(scripted(0.05, vec![stalled]).wait()),
            PlaybackOutcome::Failed
        );
    }

    #[test]
    fn a_send_with_no_length_is_over_as_soon_as_the_audio_reports_it() {
        let done = WaitFlags {
            played_ms: Some(PLAYBACK_TAIL_MS),
            ..Default::default()
        };
        assert_eq!(
            run(scripted(0.0, vec![done]).wait()),
            PlaybackOutcome::Completed
        );
        // A silly duration cannot overflow the countdown.
        assert_eq!(
            run(scripted(
                f64::INFINITY,
                vec![WaitFlags {
                    cancelled: true,
                    ..Default::default()
                }]
            )
            .wait()),
            PlaybackOutcome::Cancelled
        );
        assert_eq!(
            run(scripted(
                f64::NAN,
                vec![WaitFlags {
                    finished: true,
                    ..Default::default()
                }]
            )
            .wait()),
            PlaybackOutcome::Completed
        );
    }
}
