//! Monotonic time: [`MonoMs`], a moment on a clock that only runs forward,
//! and [`whole_ms`], seconds as whole milliseconds.

/// A moment on a clock that only runs forward, in milliseconds from wherever
/// that clock starts.
///
/// Response times are measured between two of these, so they must come from
/// a monotonic clock: the wall clock steps when it is corrected, and a step
/// back between a send ending and its answer used to score the answer as
/// instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonoMs(pub u64);

impl MonoMs {
    /// Milliseconds since `earlier`; none if it is not earlier.
    pub fn since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }

    #[must_use]
    pub fn plus(self, ms: u64) -> Self {
        Self(self.0.saturating_add(ms))
    }

    /// `seconds` later, to the nearest millisecond: how far off a player
    /// says a send is heard, where a segment of a turn starts. A negative
    /// span moves it nowhere.
    #[must_use]
    pub fn plus_sec(self, seconds: f64) -> Self {
        self.plus(whole_ms(seconds))
    }
}

/// Seconds as whole milliseconds; none for a negative span.
pub(crate) fn whole_ms(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1_000.0).round() as u64
}
