//! A run's clock: run time, with every pause left out.
//!
//! It reads a monotonic clock ([`MonoMs`]), never wall time, and has no tick
//! of its own: whoever asks says what time it is now.

use crate::session::MonoMs;

/// When a run started, how long it has spent paused, and whether it is
/// paused now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunClock {
    started: MonoMs,
    /// Pauses already over.
    paused_ms: u64,
    /// When the pause going on now began.
    paused_since: Option<MonoMs>,
    /// The latest time the clock has been read, paused or resumed at. A
    /// pause stamped before it (the heartbeat's last tick) begins there, so
    /// no stretch of time is counted as paused twice, and no run time already
    /// read, a QSO's stamp, is taken back.
    seen: MonoMs,
}

impl RunClock {
    /// A clock started at `at`.
    pub fn start(at: MonoMs) -> Self {
        Self {
            started: at,
            paused_ms: 0,
            paused_since: None,
            seen: at,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_since.is_some()
    }

    /// Hold the clock from `at`, or from the latest time it was read at if
    /// that is later. Pausing a paused clock changes nothing.
    pub fn pause(&mut self, at: MonoMs) {
        if self.paused_since.is_none() {
            self.seen = at.max(self.seen);
            self.paused_since = Some(self.seen);
        }
    }

    /// Run the clock again from `at`, the pause left out. Resuming a clock
    /// that is running changes nothing.
    pub fn resume(&mut self, at: MonoMs) {
        if let Some(since) = self.paused_since.take() {
            let at = at.max(since);
            self.paused_ms += at.since(since);
            self.seen = at;
        }
    }

    /// Run time at `now`, pauses left out. It never goes backwards, and
    /// stands still while paused. The clock remembers the latest `now` it
    /// was read at, so a stale `now` reads as that, and a pause stamped
    /// earlier begins there.
    pub fn elapsed_ms(&mut self, now: MonoMs) -> u64 {
        if self.paused_since.is_none() {
            self.seen = now.max(self.seen);
        }
        self.peek_ms(now)
    }

    /// Run time at `now`, read as [`Self::elapsed_ms`] reads it but not
    /// remembered: a reading that stamps nothing, so a pause stamped before
    /// `now` may still begin before it.
    pub fn peek_ms(&self, now: MonoMs) -> u64 {
        let end = match self.paused_since {
            Some(since) => since,
            None => now.max(self.seen),
        };
        end.since(self.started).saturating_sub(self.paused_ms)
    }

    /// Time spent paused by `now`, the pause going on included.
    pub fn paused_ms(&self, now: MonoMs) -> u64 {
        self.paused_ms + self.paused_since.map_or(0, |since| now.since(since))
    }

    /// Time spent in pauses already over, the one going on left out.
    pub fn paused_before_ms(&self) -> u64 {
        self.paused_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::{Rng, SplitMix64};

    #[test]
    fn a_pause_is_left_out_of_run_time() {
        let mut clock = RunClock::start(MonoMs(1_000));
        assert_eq!(clock.elapsed_ms(MonoMs(1_000)), 0);
        assert_eq!(clock.elapsed_ms(MonoMs(5_000)), 4_000);
        clock.pause(MonoMs(5_000));
        assert!(clock.is_paused());
        assert_eq!(clock.elapsed_ms(MonoMs(9_000)), 4_000, "it stands still");
        assert_eq!(clock.paused_ms(MonoMs(9_000)), 4_000);
        assert_eq!(
            clock.paused_before_ms(),
            0,
            "the pause going on is not over"
        );
        clock.resume(MonoMs(8_000));
        assert!(!clock.is_paused());
        assert_eq!(clock.paused_ms(MonoMs(10_000)), 3_000);
        assert_eq!(clock.paused_before_ms(), 3_000);
        assert_eq!(clock.elapsed_ms(MonoMs(10_000)), 6_000);
    }

    #[test]
    fn pausing_twice_or_resuming_a_running_clock_is_harmless() {
        let mut clock = RunClock::start(MonoMs(0));
        clock.resume(MonoMs(2_000));
        assert_eq!(clock.elapsed_ms(MonoMs(3_000)), 3_000);
        clock.pause(MonoMs(3_000));
        clock.pause(MonoMs(4_000));
        clock.resume(MonoMs(6_000));
        clock.resume(MonoMs(7_000));
        assert_eq!(clock.paused_ms(MonoMs(8_000)), 3_000);
        assert_eq!(clock.elapsed_ms(MonoMs(8_000)), 5_000);
    }

    /// A pause stamped in the past — the heartbeat's last tick — never
    /// reaches back past the last resume, and a stale reading never shows
    /// less than the clock already showed.
    #[test]
    fn a_pause_stamped_early_counts_nothing_twice() {
        let mut clock = RunClock::start(MonoMs(0));
        clock.pause(MonoMs(5_000));
        clock.resume(MonoMs(8_000));
        clock.pause(MonoMs(7_000));
        assert_eq!(clock.elapsed_ms(MonoMs(9_000)), 5_000);
        clock.resume(MonoMs(10_000));
        assert_eq!(clock.paused_ms(MonoMs(10_000)), 5_000);
        assert_eq!(clock.elapsed_ms(MonoMs(9_500)), 5_000, "a stale now");
        assert_eq!(clock.elapsed_ms(MonoMs(11_000)), 6_000);
    }

    /// A pause stamped before a time the run has already read, as the
    /// heartbeat's is when a key was handled after its last tick, begins at
    /// that reading: run time a QSO was stamped with is never taken back.
    #[test]
    fn a_pause_stamped_before_a_reading_takes_no_run_time_back() {
        let mut clock = RunClock::start(MonoMs(0));
        assert_eq!(clock.elapsed_ms(MonoMs(10_000)), 10_000);
        clock.pause(MonoMs(7_000));
        assert_eq!(clock.elapsed_ms(MonoMs(10_500)), 10_000);
        assert_eq!(clock.paused_ms(MonoMs(10_500)), 500);
        clock.resume(MonoMs(11_000));
        assert_eq!(clock.elapsed_ms(MonoMs(12_000)), 11_000);
        assert_eq!(clock.paused_ms(MonoMs(12_000)), 1_000);
    }

    /// A reading that is only a look leaves a later pause free to begin
    /// before it.
    #[test]
    fn a_peek_is_not_remembered() {
        let mut clock = RunClock::start(MonoMs(0));
        assert_eq!(clock.elapsed_ms(MonoMs(1_000)), 1_000);
        assert_eq!(clock.peek_ms(MonoMs(6_000)), 6_000);
        assert_eq!(clock.peek_ms(MonoMs(500)), 1_000, "never less than read");
        clock.pause(MonoMs(2_000));
        assert_eq!(clock.elapsed_ms(MonoMs(9_000)), 2_000);
        assert_eq!(clock.peek_ms(MonoMs(9_000)), 2_000);
    }

    /// Whatever happens, pauses stamped up to a heartbeat tick late
    /// included, run time never goes back, and run time plus time paused is
    /// the time since the start.
    #[test]
    fn run_time_never_goes_backwards() {
        let mut rng = SplitMix64::new(3);
        for _ in 0..200 {
            let start = MonoMs(rng.usize_in(0, 10_000) as u64);
            let mut clock = RunClock::start(start);
            let mut now = start;
            let mut shown = 0;
            for _ in 0..100 {
                now = now.plus(rng.usize_in(0, 3_000) as u64);
                match rng.usize_in(0, 2) {
                    0 => clock.pause(MonoMs(now.0.saturating_sub(rng.usize_in(0, 250) as u64))),
                    1 => clock.resume(now),
                    _ => {}
                }
                let elapsed = clock.elapsed_ms(now);
                assert!(elapsed >= shown, "{elapsed} after {shown}");
                shown = elapsed;
                assert_eq!(elapsed + clock.paused_ms(now), now.since(start));
            }
        }
    }
}
