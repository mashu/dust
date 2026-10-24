//! Waiting for a send to finish, without trusting the wall clock.
//!
//! Every backend reports the same [`WaitFlags`]; [`next_wait_step`] turns them
//! into a decision. Keeping that decision here — pure, and away from the audio
//! APIs — is what makes the awkward cases testable: a send that is cancelled
//! mid-flight, a device that stops calling back, and a browser tab whose audio
//! clock freezes while `setTimeout` keeps ticking.

use std::rc::Rc;

use cw_core::PlaybackPlan;

use crate::time::{POLL_MS, mono_ms, sleep_ms};

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
    /// The audio clock is parked on purpose rather than broken: a hidden tab,
    /// a context the browser suspended, a phone that locked. Nothing is being
    /// missed, so none of this time counts against the stall budget.
    pub suspended: bool,
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
    if let Some(played) = flags.played_ms
        && played >= target
    {
        return WaitStep::Done(PlaybackOutcome::Completed);
    }
    // A send whose clock is parked is not stalling, it is waiting for the
    // listener to come back, and the scheduled tone is still there to play
    // when they do. Failing here would end a session because someone looked
    // at another tab.
    if flags.suspended {
        return WaitStep::KeepWaiting;
    }
    // The send has outlived its own length by the grace period without the
    // audio reporting it got there. The tone is not playing: a device that
    // went away, a stream that stopped calling back. Waiting longer only makes
    // the silence longer.
    if waited_ms >= target.saturating_add(grace_ms) {
        return WaitStep::Done(PlaybackOutcome::Failed);
    }
    WaitStep::KeepWaiting
}

/// What `elapsed_ms` between two polls costs the stall budget.
///
/// Time spent suspended is free. Charging it would only defer the same bug:
/// the budget would run out while the page was hidden and the send would fail
/// the instant it came back, with the audio running again.
pub fn stall_charge(flags: WaitFlags, elapsed_ms: u32) -> u32 {
    if flags.suspended { 0 } else { elapsed_ms }
}

/// The most of a delay between the audio clock and the ear that is believed.
/// Bluetooth headphones run to a quarter of a second; a browser reporting
/// more is reporting nonsense, and a send would wait for it.
#[cfg(any(feature = "web", test))]
const MAX_HEARD_LATENCY_SEC: f64 = 0.5;

/// How far past the browser's clock a send is scheduled, given the delay the
/// browser reports between rendering and handing over (`base_latency_sec`).
///
/// The clock the page reads is a render callback behind the one being
/// rendered. A tone scheduled for a moment already under way starts late,
/// part way through its rise — a click, and the start of the first element
/// lost. The band's own buffers are scheduled ahead for the same reason.
#[cfg(any(feature = "web", test))]
pub fn schedule_lead_sec(base_latency_sec: f64) -> f64 {
    const LEAD_SEC: f64 = 0.02;
    let base = if base_latency_sec.is_finite() {
        base_latency_sec.clamp(0.0, 0.1)
    } else {
        0.0
    };
    base + LEAD_SEC
}

/// How much of a send that starts at `started_at_sec` on the audio clock has
/// been heard when the clock reads `now_sec`, `latency_sec` after it was
/// rendered. A send is over when its last element has reached the ear, not
/// the speaker's queue: on Bluetooth that is a quarter of a second later,
/// and the answer window would open on top of the tone.
#[cfg(any(feature = "web", test))]
pub fn heard_ms(now_sec: f64, started_at_sec: f64, latency_sec: f64) -> u32 {
    let latency = believed_latency(latency_sec);
    let ms = ((now_sec - started_at_sec - latency) * 1000.0).max(0.0);
    ms.min(f64::from(u32::MAX / 4)) as u32
}

/// How long after a send is started its first sound reaches the ear, in
/// seconds: scheduled `lead_sec` past the browser's clock, then `latency_sec`
/// from rendering to the ear, as much of it as is believed.
#[cfg(any(feature = "web", test))]
pub fn heard_after_sec(lead_sec: f64, latency_sec: f64) -> f64 {
    let lead = if lead_sec.is_finite() {
        lead_sec.max(0.0)
    } else {
        0.0
    };
    lead + believed_latency(latency_sec)
}

/// As much of a latency the browser reports as is worth believing.
#[cfg(any(feature = "web", test))]
fn believed_latency(latency_sec: f64) -> f64 {
    if latency_sec.is_finite() {
        latency_sec.clamp(0.0, MAX_HEARD_LATENCY_SEC)
    } else {
        0.0
    }
}

/// `seconds` as whole milliseconds, rounded up, for a countdown: none for
/// what is no length, and no more than a countdown can hold.
fn whole_ms(seconds: f64) -> u32 {
    let ms = (seconds * 1000.0).ceil();
    if ms.is_finite() && ms > 0.0 {
        ms.min(f64::from(u32::MAX / 4)) as u32
    } else {
        0
    }
}

/// A send in flight, as the backend sees it.
pub trait PlaybackSignal {
    fn poll(&self) -> WaitFlags;

    /// How long after it was started the send's first sound reaches the ear,
    /// in seconds, as the player reckons it: how far ahead it was scheduled,
    /// and the delay from the player's output to the ear. Nothing, for a
    /// player with no sound to be late.
    fn heard_after_sec(&self) -> f64 {
        0.0
    }
}

/// Morse already scheduled; wait without holding the player borrow.
pub struct PlaybackWait {
    pub duration_sec: f64,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    /// How long after it was started the send reaches the ear
    /// ([`PlaybackSignal::heard_after_sec`]).
    pub heard_after_sec: f64,
    /// Where each character of a group's send finishes, in seconds from its
    /// start, as its plan has it: empty but for a group's send.
    pub char_ends_sec: Vec<f64>,
    signal: Rc<dyn PlaybackSignal>,
}

impl PlaybackWait {
    /// Wait on `signal` for a send that plays as `plan` says: its length,
    /// and the speeds it resolved to, are the plan's, whoever plays it.
    pub fn new(plan: &PlaybackPlan, signal: Rc<dyn PlaybackSignal>) -> Self {
        Self {
            duration_sec: plan.duration_sec,
            char_wpm: plan.resolved_char_wpm,
            effective_wpm: plan.resolved_effective_wpm,
            heard_after_sec: signal.heard_after_sec(),
            char_ends_sec: Vec::new(),
            signal,
        }
    }

    fn duration_ms(&self) -> u32 {
        whole_ms(self.duration_sec)
    }

    pub async fn wait(self) -> PlaybackOutcome {
        let duration_ms = self.duration_ms();
        // A send waiting for the moment it was asked to start at, a
        // contest's continuation, is not stalling meanwhile.
        let grace_ms = STALL_GRACE_MS.saturating_add(whole_ms(self.heard_after_sec));
        let mut waited = 0u32;
        let mut polled_at = mono_ms();
        loop {
            let flags = self.signal.poll();
            match next_wait_step(flags, waited, duration_ms, grace_ms) {
                WaitStep::Done(outcome) => return outcome,
                WaitStep::KeepWaiting => {}
            }
            sleep_ms(POLL_MS).await;
            // The time that really passed, not the time asked for: a timer
            // runs late, and a budget of requested ticks would run long with
            // it.
            let now = mono_ms();
            let elapsed = u32::try_from(now.saturating_sub(polled_at)).unwrap_or(u32::MAX);
            polled_at = now;
            waited = waited.saturating_add(stall_charge(flags, elapsed));
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

    /// The browser schedules a send a little past its clock, and counts it
    /// heard only once the latency to the ear has passed.
    #[test]
    fn the_browsers_clock_is_read_for_what_is_heard() {
        assert_eq!(schedule_lead_sec(0.0), 0.02);
        assert!((schedule_lead_sec(0.01) - 0.03).abs() < 1e-12);
        assert_eq!(schedule_lead_sec(f64::NAN), 0.02);
        assert_eq!(schedule_lead_sec(-1.0), 0.02);
        assert!((schedule_lead_sec(9.0) - 0.12).abs() < 1e-12);

        // A second in, with a quarter second still on its way to the ear.
        assert_eq!(heard_ms(11.0, 10.0, 0.25), 750);
        assert_eq!(heard_ms(11.0, 10.0, 0.0), 1_000);
        // Not yet started, or not yet reached the ear: nothing heard.
        assert_eq!(heard_ms(9.0, 10.0, 0.0), 0);
        assert_eq!(heard_ms(10.1, 10.0, 0.25), 0);
        // A latency the browser does not know, or gets wrong, is no help.
        assert_eq!(heard_ms(11.0, 10.0, f64::NAN), 1_000);
        assert_eq!(heard_ms(11.0, 10.0, 30.0), 500);

        // A send reaches the ear its lead and the believed latency after it
        // was started.
        assert!((heard_after_sec(0.02, 0.25) - 0.27).abs() < 1e-12);
        assert_eq!(heard_after_sec(0.02, f64::NAN), 0.02);
        assert_eq!(heard_after_sec(0.02, 30.0), 0.52);
        assert_eq!(heard_after_sec(f64::NAN, -1.0), 0.0);
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
        let plan = PlaybackPlan {
            events: Vec::new(),
            duration_sec,
            resolved_char_wpm: 20.0,
            resolved_effective_wpm: 18.0,
        };
        PlaybackWait::new(
            &plan,
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
    fn a_parked_clock_is_not_a_stalled_one() {
        let mut hidden = flags();
        hidden.suspended = true;
        hidden.played_ms = Some(0);
        // However long the page stays away, the send keeps its place.
        assert_eq!(
            next_wait_step(hidden, 10 * 60 * 1_000, 1_000, 2_000),
            WaitStep::KeepWaiting
        );
        assert_eq!(stall_charge(hidden, POLL_MS), 0);
        // And a visible page is charged for every tick, as before.
        assert_eq!(stall_charge(flags(), POLL_MS), POLL_MS);
        // Being away excuses a frozen clock, not a dead stream or a stop.
        let mut broken = hidden;
        broken.failed = true;
        assert_eq!(
            next_wait_step(broken, 0, 1_000, 2_000),
            WaitStep::Done(PlaybackOutcome::Failed)
        );
        let mut stopped = hidden;
        stopped.cancelled = true;
        assert_eq!(
            next_wait_step(stopped, 0, 1_000, 2_000),
            WaitStep::Done(PlaybackOutcome::Cancelled)
        );
    }

    #[test]
    fn a_send_survives_the_page_going_away_and_coming_back() {
        let mut hidden = flags();
        hidden.suspended = true;
        hidden.played_ms = Some(20);
        let mut done = flags();
        done.played_ms = Some(1_000 + PLAYBACK_TAIL_MS);
        // Hidden for far longer than the grace period, then back and finishing.
        let mut steps = vec![hidden; 500];
        steps.push(done);
        assert_eq!(
            run(scripted(1.0, steps).wait()),
            PlaybackOutcome::Completed,
            "a backgrounded tab must not cost the listener the group"
        );
    }

    /// A native stream has no clock of its own to report, so a stall is
    /// judged by the time that really passed between polls: all of it,
    /// however it was spent. A process frozen whole for longer than the send
    /// and its grace — a phone that put the app on hold, a debugger that
    /// stopped it — wakes to a send that is still playing, because its audio
    /// froze with it, and calls that a stall: the group is rebuilt and sent
    /// again rather than heard out. Charging less would let a stream that
    /// died during the gap sit silent for up to the whole grace before
    /// anything noticed. A send the audio heard out while the waiter slept is
    /// still believed: its word outranks the budget.
    #[test]
    fn a_long_gap_between_polls_is_charged_to_the_stall_budget() {
        use std::task::Poll;
        use std::time::Duration;

        let playing = flags();
        let mut done = flags();
        done.finished = true;
        let after_a_gap = |steps: Vec<WaitFlags>| {
            run(async move {
                crate::time::reset_mono_clock();
                let mut wait = std::pin::pin!(scripted(1.0, steps).wait());
                // One poll, still playing, and the waiter asleep until the
                // next; then ten seconds go by at once.
                let first = std::future::poll_fn(|cx| Poll::Ready(wait.as_mut().poll(cx))).await;
                assert!(first.is_pending());
                tokio::time::advance(Duration::from_millis(10_000)).await;
                wait.await
            })
        };
        assert_eq!(
            after_a_gap(vec![playing, done]),
            PlaybackOutcome::Completed,
            "finished by the time the waiter woke"
        );
        assert_eq!(
            after_a_gap(vec![playing, playing, done]),
            PlaybackOutcome::Failed,
            "still playing when the waiter woke, and finished a poll later"
        );
        // Polled as often as asked, the same send is heard out.
        assert_eq!(
            run(scripted(1.0, vec![playing, playing, done]).wait()),
            PlaybackOutcome::Completed
        );
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
