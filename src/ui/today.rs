//! The day it is where you are, for whatever shows something of it: read
//! again just past each midnight, and as the page comes back into sight, so
//! that an app left open overnight goes by the new day.

use dioxus::prelude::*;

use crate::page_hidden::use_page_shown;
use crate::time::{local_date_string, sleep_ms};

/// The longest the day goes without being read again: it is read just past
/// each midnight, but a clock put right, a time zone left behind or a
/// computer asleep moves midnight meanwhile.
pub const DAY_LOOK_MS: u32 = 15 * 60 * 1_000;

/// How long past midnight the new day is read: past any difference between
/// the clock it waits by and the one that says the day.
const MIDNIGHT_PASSED_MS: u32 = 1_000;

/// The day it is where you are, for as long as the calling component is
/// mounted: read again just past each midnight, and as the page comes back
/// into sight, heard through the listener kept at `slot`
/// ([`use_page_shown`]), so that an app left open overnight goes by the new
/// day.
pub fn use_today(slot: &'static str) -> Signal<String> {
    let mut today = use_signal(today_here);
    let mut look = move || {
        let now = today_here();
        if *today.peek() != now {
            today.set(now);
        }
    };
    use_page_shown(slot, look);
    use_hook(move || {
        spawn(async move {
            loop {
                sleep_ms(until_tomorrow_ms()).await;
                look();
            }
        })
    });
    today
}

/// How long until just past the next midnight where you are, as the clock
/// reads it, but [`DAY_LOOK_MS`] at the longest.
fn until_tomorrow_ms() -> u32 {
    let now = chrono::Local::now();
    now.date_naive()
        .succ_opt()
        .and_then(|tomorrow| tomorrow.and_hms_opt(0, 0, 0))
        .and_then(|midnight| midnight.and_local_timezone(chrono::Local).earliest())
        .and_then(|midnight| u32::try_from((midnight - now).num_milliseconds()).ok())
        .map_or(DAY_LOOK_MS, |until| {
            until.saturating_add(MIDNIGHT_PASSED_MS).min(DAY_LOOK_MS)
        })
}

/// The day it is where you are, as the clock reads it; in a test, the day
/// the test says it is ([`pretend_today`]).
fn today_here() -> String {
    #[cfg(test)]
    if let Some(day) = PRETENDED_TODAY.with_borrow(Clone::clone) {
        return day;
    }
    local_date_string()
}

#[cfg(test)]
thread_local! {
    /// The day a test says it is, in place of the clock's.
    static PRETENDED_TODAY: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// From now on in this test, it is `day` where you are.
#[cfg(test)]
pub fn pretend_today(day: &str) {
    PRETENDED_TODAY.set(Some(day.to_string()));
}

/// The day `days` after `day`, as the clock writes a day: the one a test
/// pretends it is next.
#[cfg(test)]
pub fn days_after(day: &str, days: u64) -> String {
    let day = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").expect("a day");
    (day + chrono::Days::new(days))
        .format("%Y-%m-%d")
        .to_string()
}
