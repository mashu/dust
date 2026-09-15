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

pub fn local_date_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn seed_rng() -> u64 {
    now_ms() | 1
}

/// A point on a clock that moves at the speed the program is actually running.
///
/// The scope needs this rather than a count of frames. A frame counter says
/// "40 ms have passed" once per redraw, but a redraw takes 40 ms of waiting
/// *plus* however long the render and the event loop take — so the count falls
/// steadily behind the sound, and the further into a send you are the further
/// out the trace is. Asking the clock instead means a slow frame drops a frame
/// rather than shifting everything after it.
///
/// On native this is tokio's clock, which the test runtime pauses, so
/// `Ui::advance` moves it exactly as it moves `sleep_ms`.
#[cfg(not(target_arch = "wasm32"))]
pub type Mark = tokio::time::Instant;
#[cfg(target_arch = "wasm32")]
pub type Mark = f64;

pub fn mark() -> Mark {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::Instant::now()
    }
}

/// Milliseconds since `mark`, never running backwards.
pub fn since_ms(mark: Mark) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() - mark).max(0.0) as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        mark.elapsed().as_millis() as u64
    }
}
