//! How dates, counts, shares and places read on screen. Dates are stored as
//! `YYYY-MM-DD`; one that cannot be read is shown as it was stored rather
//! than not at all.

pub mod auto_level;

use std::fmt::Display;

use chrono::{NaiveDate, TimeZone};

fn parse_date(date: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

fn date_as(date: &str, pattern: &str) -> String {
    parse_date(date).map_or_else(|| date.to_string(), |day| day.format(pattern).to_string())
}

/// "Fri 16 Oct 2026".
pub fn short_date(date: &str) -> String {
    date_as(date, "%a %-d %b %Y")
}

/// "Friday 16 October 2026".
pub fn long_date(date: &str) -> String {
    date_as(date, "%A %-d %B %Y")
}

/// "September 2026" for 2026 and 9. A month that does not exist names the
/// year alone.
pub fn month_name(year: i32, month: u32) -> String {
    NaiveDate::from_ymd_opt(year, month, 1).map_or_else(
        || year.to_string(),
        |first| first.format("%B %Y").to_string(),
    )
}

/// The local moment `timestamp` names, in milliseconds since the epoch.
fn local_at(timestamp: u64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::Local
        .timestamp_millis_opt(i64::try_from(timestamp).ok()?)
        .single()
}

/// The local time of day at `timestamp`: "14:32".
pub fn clock(timestamp: u64) -> Option<String> {
    local_at(timestamp).map(|at| at.format("%H:%M").to_string())
}

/// When a session kept on `date`, the day it started, finished at
/// `timestamp`, as a list of them says it: "Fri 16 Oct 2026 · 14:32", or
/// the day alone where the time cannot be read. One that finished on a
/// later day says that day too, "Fri 16 Oct 2026 · ended Sat 17 Oct 00:05",
/// as its time alone would read as the night before it ended.
pub fn day_and_clock(date: &str, timestamp: u64) -> String {
    let Some(time) = clock(timestamp) else {
        return short_date(date);
    };
    match day_of(timestamp).filter(|ended| ended != date) {
        Some(ended) => format!(
            "{} · ended {} {time}",
            short_date(date),
            date_as(&ended, "%a %-d %b")
        ),
        None => format!("{} · {time}", short_date(date)),
    }
}

/// The local day of `timestamp`, the way dates are stored: "2026-10-19".
pub fn day_of(timestamp: u64) -> Option<String> {
    local_at(timestamp).map(|at| at.format("%Y-%m-%d").to_string())
}

/// How long ago `date` was, seen from `today`: "today", "yesterday", "12 days
/// ago". Nothing for a date that cannot be read or is still to come.
pub fn ago(date: &str, today: &str) -> Option<String> {
    let days = (parse_date(today)? - parse_date(date)?).num_days();
    match days {
        0 => Some("today".to_string()),
        1 => Some("yesterday".to_string()),
        2.. => Some(format!("{days} days ago")),
        _ => None,
    }
}

/// "1 session", "2 sessions", "0 characters".
pub fn counted<N: Display + PartialEq + From<u8>>(n: N, noun: &str) -> String {
    if n == N::from(1) {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// As [`counted`], with a few in words, as a sentence says them: "one
/// call", "two NRs", "13 calls".
pub fn counted_in_words(n: usize, noun: &str) -> String {
    const WORDS: [&str; 10] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    let counted = counted(n, noun);
    match WORDS.get(n) {
        Some(word) => counted.replacen(&n.to_string(), word, 1),
        None => counted,
    }
}

/// A share, from 0 to 1, in whole percent: "90%" for 0.9. A share half way
/// between two goes to the even one, as `{:.0}` rounds it: "12%" for
/// 0.125, "38%" for 0.375.
///
/// Not the Auto level's [`auto_level::percent_to_tenth`], which is handed a
/// value in percent already and gives it to a tenth where it has one, nor
/// [`chance`], which gives a chance over nothing as "<1%" where this gives
/// 0%.
pub fn percent(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

/// The number [`percent`] shows for `share`: 12 for 0.125, a half going to
/// the even one. What is judged by the percent a share reads, as a row
/// against the aim it is drawn beside, goes by this, so the judgement
/// agrees with what is shown.
pub fn whole_percent(share: f64) -> f64 {
    (share * 100.0).round_ties_even()
}

/// A chance, from 0 to 1, in whole percent as [`percent`] gives it, but
/// "<1%" for one over nothing that whole percents would show as 0, half a
/// percent among them, which goes to the even 0: a chance there is never
/// reads as none. What comes up next gives each character's chances so,
/// of a wrong copy and of coming up next, and the Group length and Speed
/// cards each row's chance of coming up next.
pub fn chance(share: f64) -> String {
    if share > 0.0 && whole_percent(share) == 0.0 {
        "<1%".to_string()
    } else {
        percent(share)
    }
}

/// A stretch of run time as a clock reads it: "07:42", "60:00".
pub fn minutes_seconds(ms: u64) -> String {
    let seconds = ms / 1_000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// "1st", "2nd", "3rd", "4th", "11th", "21st".
pub fn ordinal(place: usize) -> String {
    let suffix = match (place % 10, place % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{place}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session that ran past midnight is listed on the day it started,
    /// with the day it ended beside its time.
    #[test]
    fn a_session_past_midnight_says_the_day_it_ended() {
        let at = |day: &str, hour: u32, minute: u32| {
            let day = parse_date(day)
                .unwrap()
                .and_hms_opt(hour, minute, 0)
                .unwrap();
            let at = chrono::Local.from_local_datetime(&day).single().unwrap();
            u64::try_from(at.timestamp_millis()).unwrap()
        };
        assert_eq!(
            day_and_clock("2026-10-16", at("2026-10-16", 23, 50)),
            "Fri 16 Oct 2026 · 23:50"
        );
        assert_eq!(
            day_and_clock("2026-10-16", at("2026-10-17", 0, 5)),
            "Fri 16 Oct 2026 · ended Sat 17 Oct 00:05"
        );
    }

    #[test]
    fn a_date_reads_in_words() {
        assert_eq!(short_date("2026-10-16"), "Fri 16 Oct 2026");
        assert_eq!(long_date("2026-10-16"), "Friday 16 October 2026");
        assert_eq!(long_date("2026-09-08"), "Tuesday 8 September 2026");
        assert_eq!(short_date("not a date"), "not a date");
        assert_eq!(long_date("not a date"), "not a date");
    }

    #[test]
    fn a_month_is_named_in_full_with_its_year() {
        assert_eq!(month_name(2026, 1), "January 2026");
        assert_eq!(month_name(2026, 12), "December 2026");
        // A month out of range names the year rather than panicking.
        assert_eq!(month_name(2026, 0), "2026");
        assert_eq!(month_name(2026, 13), "2026");
    }

    #[test]
    fn a_moment_has_a_day_and_a_time_of_day() {
        let day = day_of(1_790_000_000_000).expect("a day");
        assert!(parse_date(&day).is_some(), "{day}");
        assert!(clock(1_790_000_000_000).is_some_and(|time| time.len() == 5));
        assert_eq!(day_of(u64::MAX), None);
    }

    #[test]
    fn how_long_ago_is_said_in_days() {
        assert_eq!(ago("2026-10-16", "2026-10-16").as_deref(), Some("today"));
        assert_eq!(
            ago("2026-10-15", "2026-10-16").as_deref(),
            Some("yesterday")
        );
        assert_eq!(
            ago("2026-10-04", "2026-10-16").as_deref(),
            Some("12 days ago")
        );
        assert_eq!(ago("2026-10-17", "2026-10-16"), None, "still to come");
        assert_eq!(ago("garbled", "2026-10-16"), None);
    }

    #[test]
    fn one_is_counted_in_the_singular() {
        assert_eq!(counted(1_u32, "session"), "1 session");
        assert_eq!(counted(2_usize, "session"), "2 sessions");
        assert_eq!(counted(0_u32, "character"), "0 characters");
        assert_eq!(counted_in_words(1, "call"), "one call");
        assert_eq!(counted_in_words(2, "NR"), "two NRs");
        assert_eq!(counted_in_words(0, "QSO"), "no QSOs");
        assert_eq!(counted_in_words(13, "call"), "13 calls");
    }

    #[test]
    fn a_share_reads_in_whole_percent() {
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.9), "90%");
        assert_eq!(percent(1.0), "100%");
        // Half way between two, it goes to the even one.
        assert_eq!(percent(0.125), "12%");
        assert_eq!(percent(0.375), "38%");
        // A share, where the Auto level's is in percent already.
        assert_eq!(percent(0.875), "88%");
        assert_eq!(auto_level::percent_to_tenth(87.5), "87.5%");
    }

    /// The number a share is judged by is the one it reads, ties and all,
    /// across the whole range of shares.
    #[test]
    fn whole_percent_is_the_number_percent_shows() {
        let ties = [0.005, 0.015, 0.125, 0.375, 0.625, 0.875];
        let shares = (0..=100_000).map(|k| f64::from(k) / 100_000.0);
        for share in shares.chain(ties) {
            assert_eq!(
                percent(share),
                format!("{}%", whole_percent(share)),
                "{share}"
            );
        }
        assert_eq!(whole_percent(0.125), 12.0);
        assert_eq!(whole_percent(0.625), 62.0);
        assert_eq!(whole_percent(0.005), 0.0);
    }

    /// A chance more than nothing never reads 0%: under half a percent, and
    /// half a percent, which whole percents send to the even 0, read under
    /// one percent; a hair over half reads 1%, the rest as [`percent`] reads
    /// them, and nothing 0%.
    #[test]
    fn a_chance_over_nothing_never_reads_none() {
        assert_eq!(chance(0.004), "<1%");
        assert_eq!(chance(0.005), "<1%");
        assert_eq!(chance(0.005_000_01), "1%");
        assert_eq!(chance(0.015), "2%");
        assert_eq!(chance(0.125), "12%");
        assert_eq!(chance(0.0), "0%");
    }

    #[test]
    fn run_time_reads_as_a_clock() {
        assert_eq!(minutes_seconds(0), "00:00");
        assert_eq!(minutes_seconds(462_999), "07:42");
        assert_eq!(minutes_seconds(3_600_000), "60:00");
    }

    #[test]
    fn places_are_said_as_ordinals() {
        let said: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111]
            .into_iter()
            .map(ordinal)
            .collect();
        assert_eq!(
            said,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd",
                "101st", "111th"
            ]
        );
    }
}
