//! On Android, a line in the system log when the app stands still while it
//! holds the audio: the band plays on, and nothing is sent or drawn. tao's
//! event loop used to lose the wake that runs the app's tasks
//! (`vendor/tao`), and a tap that does too much holds them back as surely;
//! either way the phone sounds and the screen answers nothing. The line
//! says it happened, and for how long, in a log an owner can send.
//!
//! Every task of the app's that waits does so through
//! [`crate::time::sleep_ms`] — a session's heartbeat, each poll of a send in
//! flight, the gaps in the stream of groups, a run's ticks — and says as it
//! starts when it is due again, and as it ends, or is given up, that it is
//! due no more. A claim of the audio counts too, so a tap that claims it
//! and is followed by nothing is caught. A thread of its own looks every
//! half second: the app more than `STALL_MS` past the earliest wait under
//! way, with the audio held, writes one line to stderr, which Android's log
//! shows under `RustStdoutStderr`. A long wait asked for, such as a
//! stream's pause between groups, is not due until it is over, and hides
//! no wait due before it.
//!
//! Everywhere else this is nothing: the desktop and the browser have no
//! such loop to lose a wake.

#[cfg(any(test, target_os = "android"))]
use std::collections::BTreeMap;
#[cfg(any(test, target_os = "android"))]
use std::collections::btree_map::Entry;

/// How far past due the app may run before it is said to have stood still:
/// a busy page holds a task back by far less, and a stall the event loop's
/// kick clears (`LOOP_KICK_MS`) is over well before.
#[cfg(any(test, target_os = "android"))]
const STALL_MS: u64 = 1_500;

#[cfg(all(target_os = "android", not(test)))]
mod android {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::time::Duration;

    use super::Waits;
    use crate::time::mono_ms;

    /// How often the thread looks.
    const LOOK_MS: u64 = 500;

    /// The waits under way, and the claim of the audio, on [`mono_ms`]'s
    /// clock. Shared by every thread: a wait is begun on the event loop's,
    /// and a tap's thread may give it up with its task.
    static WAITS: Mutex<Waits> = Mutex::new(Waits::new());
    /// A lease holds the audio.
    static HELD: AtomicBool = AtomicBool::new(false);

    /// The waits, as they stand. A panic while they were held left them as
    /// whole as before: each change to them is one step.
    fn waits() -> MutexGuard<'static, Waits> {
        WAITS.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A wait of `ms` begins: when it is due.
    pub fn due_in(ms: u32) -> u64 {
        let due = mono_ms().saturating_add(u64::from(ms));
        waits().wait(due);
        due
    }

    pub fn done(due: u64) {
        waits().done(due);
    }

    pub fn audio_held(held: bool) {
        if held {
            waits().claim(mono_ms());
        }
        HELD.store(held, Ordering::Relaxed);
    }

    pub fn start() {
        let _ = std::thread::Builder::new()
            .name("dust-stall-log".into())
            .spawn(|| {
                let mut told = None;
                loop {
                    std::thread::sleep(Duration::from_millis(LOOK_MS));
                    let Some(due) = waits().earliest() else {
                        continue;
                    };
                    let held = HELD.load(Ordering::Relaxed);
                    if let Some(late) = super::stalled(mono_ms(), due, held, told) {
                        told = Some(due);
                        eprintln!(
                            "dust: stalled: a task due {late} ms ago has not run, audio held"
                        );
                    }
                }
            });
    }
}

/// A task's wait, under way for as long as this is held ([`due_in`]): let
/// go of as the wait ends, or as it is given up with its task.
#[must_use = "a wait is under way only while this is held"]
pub struct Due {
    #[cfg(all(target_os = "android", not(test)))]
    at: u64,
}

#[cfg(all(target_os = "android", not(test)))]
impl Drop for Due {
    fn drop(&mut self) {
        android::done(self.at);
    }
}

/// A task of the app's is about to wait `ms`, and is due again then: for
/// as long as what this hands back is held.
#[inline]
pub fn due_in(ms: u32) -> Due {
    #[cfg(all(target_os = "android", not(test)))]
    {
        Due {
            at: android::due_in(ms),
        }
    }
    #[cfg(not(all(target_os = "android", not(test))))]
    {
        let _ = ms;
        Due {}
    }
}

/// The audio was claimed, `held`, or given back. A claim is made inside a
/// tap, and what it starts is due to run at once.
#[inline]
pub fn audio_held(held: bool) {
    #[cfg(all(target_os = "android", not(test)))]
    android::audio_held(held);
    #[cfg(not(all(target_os = "android", not(test))))]
    let _ = held;
}

/// Start looking, on Android.
#[cfg(all(target_os = "android", not(test)))]
pub use android::start;

/// The waits under way, each by when it is due, and the last claim of the
/// audio until a task next waits: the app is due to run a task at the
/// earliest of them. A wait asked for after another, however long, hides
/// none due before it.
#[cfg(any(test, target_os = "android"))]
struct Waits {
    /// How many of the waits under way are due at each instant.
    pending: BTreeMap<u64, usize>,
    /// When the audio was claimed, if no task has waited since.
    claimed: Option<u64>,
}

#[cfg(any(test, target_os = "android"))]
impl Waits {
    const fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            claimed: None,
        }
    }

    /// A task waits until `due`: it ran, and so does what a claim started.
    fn wait(&mut self, due: u64) {
        self.claimed = None;
        *self.pending.entry(due).or_default() += 1;
    }

    /// A wait due at `due` is over, or given up with its task.
    fn done(&mut self, due: u64) {
        if let Entry::Occupied(mut held) = self.pending.entry(due) {
            *held.get_mut() -= 1;
            if *held.get() == 0 {
                held.remove();
            }
        }
    }

    /// The audio was claimed at `now`, inside a tap: what the claim starts
    /// is due at once.
    fn claim(&mut self, now: u64) {
        self.claimed = Some(now);
    }

    /// When the app is next due to run a task, if anything waits on it.
    fn earliest(&self) -> Option<u64> {
        let first = self.pending.keys().next().copied();
        first.into_iter().chain(self.claimed).min()
    }
}

/// How long past `due` the app stands at `now`, if that is a stall to tell
/// of: the audio `held`, more than [`STALL_MS`] past due, and not the stall
/// already told of, the one `told` was due at. One line a stall: the next
/// one is due somewhere else.
#[cfg(any(test, target_os = "android"))]
fn stalled(now: u64, due: u64, held: bool, told: Option<u64>) -> Option<u64> {
    let late = now.saturating_sub(due);
    (held && late > STALL_MS && told != Some(due)).then_some(late)
}

#[cfg(test)]
mod tests {
    use super::{STALL_MS, Waits, stalled};

    /// The app is due at the earliest of the waits under way, however many
    /// longer ones are asked for after it: a session's heartbeat held up
    /// is a stall, whatever the stream's pause between groups asked for
    /// since. Each wait over, or given up, is due no more; a claim of the
    /// audio is due at once, until a task next waits.
    #[test]
    fn the_app_is_due_at_the_earliest_wait_under_way() {
        let mut waits = Waits::new();
        assert_eq!(waits.earliest(), None, "nothing waits");
        waits.wait(10_050);
        waits.wait(25_000);
        assert_eq!(waits.earliest(), Some(10_050), "the heartbeat");
        assert_eq!(stalled(12_000, 10_050, true, None), Some(1_950));
        waits.done(10_050);
        assert_eq!(waits.earliest(), Some(25_000), "the pause, once it ran");
        waits.wait(10_100);
        waits.wait(10_100);
        waits.done(10_100);
        assert_eq!(waits.earliest(), Some(10_100), "one of two alike");
        waits.done(10_100);
        waits.done(25_000);
        assert_eq!(waits.earliest(), None);
        waits.claim(30_000);
        assert_eq!(waits.earliest(), Some(30_000), "a claim");
        waits.wait(30_016);
        assert_eq!(waits.earliest(), Some(30_016), "what it started runs");
    }

    /// A heartbeat due at 10 s that has not run by 12 s, the band playing.
    #[test]
    fn a_task_long_past_due_with_the_audio_held_is_a_stall() {
        assert_eq!(stalled(12_000, 10_000, true, None), Some(2_000));
    }

    /// A busy page holds a task back a little; that is no stall.
    #[test]
    fn a_task_a_little_late_is_not_a_stall() {
        assert_eq!(stalled(10_000 + STALL_MS, 10_000, true, None), None);
        assert_eq!(
            stalled(10_001 + STALL_MS, 10_000, true, None),
            Some(STALL_MS + 1)
        );
    }

    /// Nothing is owed with the audio given back: a page out of sight, or
    /// Home with nothing playing, runs nothing for minutes.
    #[test]
    fn nothing_held_is_no_stall() {
        assert_eq!(stalled(70_000, 10_000, false, None), None);
    }

    /// A stream's pause between groups, fifteen seconds at most, is waited
    /// out in one go: due when it is over, not before.
    #[test]
    fn a_long_wait_asked_for_is_not_a_stall() {
        let asked_at = 10_000;
        let due = asked_at + 15_000;
        assert_eq!(stalled(asked_at + 14_000, due, true, None), None);
        assert_eq!(stalled(due + 2_000, due, true, None), Some(2_000));
    }

    /// One line a stall, however long it lasts; the next stall, due
    /// somewhere else, gets a line of its own.
    #[test]
    fn a_stall_is_told_once() {
        assert_eq!(stalled(12_000, 10_000, true, None), Some(2_000));
        assert_eq!(stalled(30_000, 10_000, true, Some(10_000)), None);
        assert_eq!(stalled(40_000, 35_000, true, Some(10_000)), Some(5_000));
    }

    /// A wake the event loop lost holds the app back until the next kick
    /// at most, and that is never told of as a stall.
    #[test]
    fn a_stall_the_loop_kick_clears_is_never_told() {
        let due = 10_000;
        assert_eq!(stalled(due + crate::LOOP_KICK_MS, due, true, None), None);
    }
}
