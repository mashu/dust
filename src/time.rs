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
/// is timed by the same clock its timers run on. A browser has its own
/// monotonic clock in `performance.now()`.
pub fn mono_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        CLOCK_ORIGIN.with(|origin| {
            let start = origin.get().unwrap_or_else(|| {
                let now = tokio::time::Instant::now();
                origin.set(Some(now));
                now
            });
            tokio::time::Instant::now()
                .saturating_duration_since(start)
                .as_millis() as u64
        })
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

#[cfg(not(target_arch = "wasm32"))]
std::thread_local! {
    static CLOCK_ORIGIN: std::cell::Cell<Option<tokio::time::Instant>> =
        const { std::cell::Cell::new(None) };
}

/// Start the monotonic clock over. Each test runtime must call this so a
/// previous test's origin is not reused.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn reset_mono_clock() {
    CLOCK_ORIGIN.with(|origin| origin.set(None));
}

/// Move the monotonic clock on by `ms` at once, as a call that keeps the
/// thread busy that long moves it while nothing else runs: a test's paused
/// clock only moves while the test waits. The clock runs on from there,
/// with the test's timers.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn skip_mono_clock(ms: u64) {
    let _ = mono_ms();
    CLOCK_ORIGIN.with(|origin| {
        let moved = origin
            .get()
            .and_then(|start| start.checked_sub(std::time::Duration::from_millis(ms)));
        if moved.is_some() {
            origin.set(moved);
        }
    });
}

pub fn local_date_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
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
    /// While a test has pinned its seeds, the state the next is drawn from.
    static PINNED_SEEDS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Pin every seed [`seed_rng`] hands out on this thread to the sequence
/// `seed` starts, or, with none, go back to fresh entropy. A test that pins
/// its seeds meets the same stations, the same calls and the same timing
/// on every run, so what it checks can never depend on a lucky draw. Only
/// tests can: the app always seeds from the platform.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn pin_seeds(seed: Option<u64>) {
    PINNED_SEEDS.with(|pinned| pinned.set(seed));
}

/// The next of the pinned seeds, as SplitMix64 steps from the one pinned;
/// none while nothing is pinned.
#[cfg(all(test, not(target_arch = "wasm32")))]
fn next_pinned_seed() -> Option<u64> {
    PINNED_SEEDS.with(|pinned| {
        let state = pinned.get()?.wrapping_add(0x9e37_79b9_7f4a_7c15);
        pinned.set(Some(state));
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

    /// A skip moves the clock on at once, and it runs on from there.
    #[test]
    fn a_skip_moves_the_monotonic_clock_on() {
        let before = super::mono_ms();
        super::skip_mono_clock(30);
        let after = super::mono_ms();
        assert!(after >= before + 30, "{before} then {after}");
        assert!(super::mono_ms() >= after);
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

    #[test]
    fn no_two_seeds_are_the_same() {
        let seeds: std::collections::BTreeSet<u64> = (0..64).map(|_| super::seed_rng()).collect();
        assert_eq!(seeds.len(), 64, "a seed repeated within one launch");
    }
}
