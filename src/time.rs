use std::borrow::Cow;

use cw_core::{SessionResult, settle_clock_ahead};

/// Fine-grained poll for playback/confirm waits. Keeps Morse→input latency low
/// without spinning.
pub const POLL_MS: u32 = 16;

pub async fn sleep_ms(ms: u32) {
    let ms = ms.max(1);
    #[cfg(target_arch = "wasm32")]
    {
        gloo_timers::future::TimeoutFuture::new(ms).await;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::sleep(std::time::Duration::from_millis(u64::from(ms))).await;
    }
}

pub fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Monotonic milliseconds, for anything timed against itself: the paddle
/// keyer, and the session's response times, which a wall clock stepped back
/// would turn negative.
///
/// Natively this is Tokio's clock, which is the system's monotonic clock in
/// the app and the paused, hand-advanced one in tests, so a test's session
/// is timed by the same clock its timers run on. It counts from one zero
/// for every thread ([`clock_origin`]): readings taken on two threads are
/// compared, as a session's heartbeat compares the one a tap left. A
/// browser has its own monotonic clock in `performance.now()`.
pub fn mono_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::Instant::now()
            .saturating_duration_since(clock_origin())
            .as_millis() as u64
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Every page has a window; the wall clock is only for a context
        // without one, which this app never runs in.
        web_sys::window()
            .and_then(|window| window.performance())
            .map_or_else(now_ms, |performance| performance.now().max(0.0) as u64)
    }
}

/// How long ago, in seconds, [`mono_ms`] stood at `at`: to well under a
/// millisecond, where `mono_ms` drops the part of one it has run into, so
/// an edge the paddle keyer timed at `at` is placed where it was meant.
/// On any thread, whichever read `at`; never less than nothing.
pub fn mono_secs_since(at: u64) -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let then = clock_origin() + std::time::Duration::from_millis(at);
        tokio::time::Instant::now()
            .saturating_duration_since(then)
            .as_secs_f64()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let now = web_sys::window()
            .and_then(|window| window.performance())
            .map_or(at as f64, |performance| performance.now());
        ((now - at as f64) / 1_000.0).max(0.0)
    }
}

/// Where [`mono_ms`] counts from: the first reading taken on any thread.
#[cfg(not(target_arch = "wasm32"))]
static CLOCK_ORIGIN: std::sync::OnceLock<tokio::time::Instant> = std::sync::OnceLock::new();

/// One zero for every thread. On Android the webview hands each tap to the
/// app on a thread of its own, while the app's tasks — a session's
/// heartbeat, its timers — run on the event loop's; the clock used to start
/// afresh on each thread, so the heartbeat measured a tap's reading against
/// a zero seconds apart, took the difference for time the app had not run,
/// and paused the session as if the page had gone out of sight.
///
/// A test runs on a clock of its own, paused and moved by hand, and starts
/// it from nothing ([`reset_mono_clock`]); a test that has not reads the
/// clock every thread shares.
#[cfg(not(target_arch = "wasm32"))]
fn clock_origin() -> tokio::time::Instant {
    #[cfg(test)]
    if let Some(origin) = test_clock_origin() {
        return origin;
    }
    *CLOCK_ORIGIN.get_or_init(tokio::time::Instant::now)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
std::thread_local! {
    /// This test's zero, once [`reset_mono_clock`] has asked for one: set
    /// by its first reading, on its own clock.
    static TEST_CLOCK_ORIGIN: std::cell::Cell<Option<Option<tokio::time::Instant>>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(all(test, not(target_arch = "wasm32")))]
fn test_clock_origin() -> Option<tokio::time::Instant> {
    TEST_CLOCK_ORIGIN.with(|origin| {
        let asked = origin.get()?;
        Some(asked.unwrap_or_else(|| {
            let now = tokio::time::Instant::now();
            origin.set(Some(Some(now)));
            now
        }))
    })
}

/// Start the monotonic clock over, for this test alone. Each test runtime
/// must call this, so its clock counts from its own start, not another
/// test's.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn reset_mono_clock() {
    TEST_CLOCK_ORIGIN.with(|origin| origin.set(Some(None)));
}

pub fn local_date_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// `history` as the screens show it: a session stamped ahead of the clock,
/// finished while it was set ahead, read as finished just after the latest
/// that was not, on its day ([`settle_clock_ahead`]). Only for showing: the
/// store, and whatever picks a session out of the history to change it, go
/// by the stamps as stored, so a clock that is wrong now moves nothing for
/// good. Borrowed as it is when nothing is stamped ahead.
pub fn as_shown(history: &[SessionResult]) -> Cow<'_, [SessionResult]> {
    let now = now_ms();
    if history.iter().all(|session| session.timestamp <= now) {
        return Cow::Borrowed(history);
    }
    let mut shown = history.to_vec();
    settle_clock_ahead(&mut shown, now);
    Cow::Owned(shown)
}

/// A seed nobody else has: fresh entropy from the platform.
///
/// Every launch of the app, and every session in one, draws its stations and
/// its groups from one of these. A clock reading would hand the same seed to
/// two launches in the same millisecond, and a counter the same seed to the
/// first session of every launch.
pub fn seed_rng() -> u64 {
    #[cfg(all(test, not(target_arch = "wasm32")))]
    if let Some(seed) = next_pinned_seed() {
        return seed;
    }
    #[cfg(target_arch = "wasm32")]
    {
        // The browser build's std has no entropy source; the page has one.
        let word = || (js_sys::Math::random() * 4_294_967_296.0) as u64;
        ((word() << 32) | word()) ^ now_ms()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::hash::{BuildHasher, Hasher};
        // Each `RandomState` is keyed afresh from the operating system's
        // random source, so hashing anything with it gives a new seed.
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(now_ms());
        hasher.finish()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
std::thread_local! {
    /// While a test has pinned its seeds, the seed pinned, and the state the
    /// next is drawn from.
    static PINNED_SEEDS: std::cell::Cell<Option<(u64, u64)>> = const { std::cell::Cell::new(None) };
}

/// Pin every seed [`seed_rng`] hands out on this thread to the sequence
/// `seed` starts, or, with none, go back to fresh entropy. A test that pins
/// its seeds meets the same stations, the same calls and the same timing
/// on every run, so what it checks can never depend on a lucky draw. Only
/// tests can: the app always seeds from the platform.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn pin_seeds(seed: Option<u64>) {
    PINNED_SEEDS.with(|pinned| pinned.set(seed.map(|seed| (seed, seed))));
}

/// The seed this thread's seeds are pinned to, if they are: pinned again,
/// it hands them all out again, in the same order.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn pinned_seed() -> Option<u64> {
    PINNED_SEEDS.with(|pinned| pinned.get().map(|(seed, _)| seed))
}

/// The next of the pinned seeds, as SplitMix64 steps from the one pinned;
/// none while nothing is pinned.
#[cfg(all(test, not(target_arch = "wasm32")))]
fn next_pinned_seed() -> Option<u64> {
    PINNED_SEEDS.with(|pinned| {
        let (seed, state) = pinned.get()?;
        let state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        pinned.set(Some((seed, state)));
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Some(z ^ (z >> 31))
    })
}

#[cfg(test)]
mod tests {
    /// Outside any runtime too: the clock runs forward, never back.
    #[test]
    fn the_monotonic_clock_never_runs_back() {
        let mut last = super::mono_ms();
        for _ in 0..1_000 {
            let now = super::mono_ms();
            assert!(now >= last);
            last = now;
        }
    }

    /// Every thread reads one clock. On Android a tap is handled on the
    /// webview's thread and a session's heartbeat on the event loop's: each
    /// thread used to start a clock of its own, so a reading taken by a tap
    /// was seconds behind the heartbeat's, and the heartbeat took the
    /// difference for a gap and paused the session.
    #[test]
    fn every_thread_reads_one_clock() {
        let here = super::mono_ms();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let there = std::thread::spawn(super::mono_ms).join().unwrap();
        let back = super::mono_ms();
        assert!(
            here + 50 <= there && there <= back,
            "{here}, {there}, {back}"
        );
    }

    /// A moment one thread read is placed right by another.
    #[test]
    fn a_moment_is_placed_right_on_any_thread() {
        let at = super::mono_ms();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let since = std::thread::spawn(move || super::mono_secs_since(at))
            .join()
            .unwrap();
        assert!((0.05..1.0).contains(&since), "{since}");
    }

    /// A test's own clock starts from nothing, and moves only as the test
    /// moves it.
    #[test]
    fn a_test_runtime_keeps_a_clock_of_its_own() {
        crate::testing::run(|| async {
            assert_eq!(super::mono_ms(), 0);
            tokio::time::advance(std::time::Duration::from_millis(300)).await;
            assert_eq!(super::mono_ms(), 300);
        });
    }

    /// An instant read off the clock is placed to well under a millisecond,
    /// where the clock itself counts whole ones.
    #[test]
    fn an_instant_is_placed_finer_than_the_clock_counts() {
        let at = super::mono_ms();
        let since = super::mono_secs_since(at);
        assert!((0.0..0.5).contains(&since), "{since}");
        assert_eq!(super::mono_secs_since(at + 60_000), 0.0, "never ahead");
        std::thread::sleep(std::time::Duration::from_millis(3));
        let later = super::mono_secs_since(at);
        assert!(later >= 0.003 && later > since, "{since} then {later}");
        // Between the whole milliseconds the clock counts: read a few times
        // a fraction of one apart, some reading falls between two.
        let between = (0..20).any(|_| {
            std::thread::sleep(std::time::Duration::from_micros(300));
            (super::mono_secs_since(at) * 1_000.0).fract() != 0.0
        });
        assert!(between, "placed to the whole millisecond only");
    }

    /// A pinned seed starts the same sequence every time, another seed
    /// another one, and unpinned the seeds are fresh again.
    #[test]
    fn pinned_seeds_repeat() {
        let draw = |seed| {
            super::pin_seeds(Some(seed));
            let seeds: Vec<u64> = (0..3).map(|_| super::seed_rng()).collect();
            super::pin_seeds(None);
            seeds
        };
        assert_eq!(draw(7), draw(7));
        assert_ne!(draw(7), draw(8));
        let seeds = draw(7);
        assert_ne!(seeds[0], seeds[1], "a sequence, not one seed again");
        assert!(!seeds.contains(&super::seed_rng()), "fresh once unpinned");
    }

    /// While the seeds are pinned, the seed pinned is known, however many
    /// have been drawn from it; unpinned, there is none.
    #[test]
    fn the_seed_pinned_is_known_while_pinned() {
        super::pin_seeds(Some(42));
        let _ = (super::seed_rng(), super::seed_rng());
        assert_eq!(super::pinned_seed(), Some(42));
        super::pin_seeds(None);
        assert_eq!(super::pinned_seed(), None);
    }

    #[test]
    fn no_two_seeds_are_the_same() {
        let seeds: std::collections::BTreeSet<u64> = (0..64).map(|_| super::seed_rng()).collect();
        assert_eq!(seeds.len(), 64, "a seed repeated within one launch");
    }
}
