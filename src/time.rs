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

/// Monotonic milliseconds for the paddle keyer.
///
/// Tests run on a paused Tokio clock, so wall time would make every straight-key
/// hold look like a dit. When a Tokio runtime is up, this follows that clock.
pub fn mono_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            return CLOCK_ORIGIN.with(|origin| {
                let start = origin.get().unwrap_or_else(|| {
                    let now = tokio::time::Instant::now();
                    origin.set(Some(now));
                    now
                });
                tokio::time::Instant::now()
                    .saturating_duration_since(start)
                    .as_millis() as u64
            });
        }
    }
    now_ms()
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

pub fn local_date_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn seed_rng() -> u64 {
    now_ms() | 1
}
