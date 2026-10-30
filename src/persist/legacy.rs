//! The history as builds before records kept it: whole, as one JSON list
//! of every session, in `sessions.json` or a browser's `dust_sessions`.
//!
//! How such a history is read, what of it that does not read is set aside,
//! and, in a browser, how much of it is kept for the room there is. Older
//! builds still read and write it, and the move to records reads it once
//! and leaves it as it was for them.

use cw_core::SessionResult;
use serde::Deserialize;

#[cfg(any(feature = "web", test))]
use super::HistorySaved;
use super::record;

/// How much of the site's storage the history may take, in characters.
/// Browsers give a site about five million; the rest is left for the
/// settings, the counters and the room a write needs. Other sites on the
/// origin may have taken some of it, and then a write that does not fit
/// keeps fewer of the newest sessions ([`store_newest_that_fit`]).
#[cfg(feature = "web")]
pub(super) const HISTORY_BUDGET: usize = 3_000_000;

/// Read a stored history, keeping every session that still parses, and
/// what is stored of every one that does not.
///
/// `None` means the history as a whole is unreadable — not a list at all.
/// That is different from an empty one: the caller has to set the stored copy
/// aside before anything writes over it, or the first session finished after
/// a bad write would replace years of history with itself. So too with the
/// sessions that do not parse, which the next save would leave out.
///
/// Each session is read as a record is ([`record::read_record`]): scored
/// again, from what is stored of it, so the history is on one scale however
/// the score has been worked out over the versions, and its stamps and days
/// read as they were stored, whatever the clock says now: one that is wrong
/// now would otherwise move them for good at the next save. The screens
/// settle a session stamped ahead of the clock for themselves
/// ([`crate::time::as_shown`]).
pub(super) fn read_history(raw: &str) -> Option<ReadHistory> {
    // Almost always the whole list reads; only when it does not is it worth
    // reading again a session at a time to keep the ones that do.
    let (sessions, unread) = match serde_json::from_str::<Vec<SessionResult>>(raw) {
        Ok(sessions) => (sessions, Vec::new()),
        Err(_) => {
            let (mut sessions, mut unread) = (Vec::new(), Vec::new());
            for value in serde_json::from_str::<Vec<serde_json::Value>>(raw).ok()? {
                match SessionResult::deserialize(&value) {
                    Ok(session) => sessions.push(session),
                    Err(_) => unread.push(value),
                }
            }
            (sessions, unread)
        }
    };
    let sessions = sessions.into_iter().map(record::scored).collect();
    Some(ReadHistory { sessions, unread })
}

/// The sessions of a stored history that still parse ([`read_history`]),
/// where nothing is ever written over it: a backup file.
pub(super) fn parse_sessions(raw: &str) -> Option<Vec<SessionResult>> {
    read_history(raw).map(|read| read.sessions)
}

/// A stored history as it was read ([`read_history`]).
#[derive(Debug, PartialEq)]
pub(super) struct ReadHistory {
    /// Every session that still parses, oldest first.
    pub(super) sessions: Vec<SessionResult>,
    /// Every one that does not, as it is stored.
    pub(super) unread: Vec<serde_json::Value>,
}

impl ReadHistory {
    /// The sessions that could not be read, as one JSON list to set aside;
    /// none when every one could.
    #[cfg(any(feature = "web", feature = "native-runtime", test))]
    pub(super) fn unread_json(&self) -> Option<String> {
        if self.unread.is_empty() {
            return None;
        }
        serde_json::to_string(&self.unread).ok()
    }
}

/// How many copies of what could not be read a browser keeps set aside, at
/// most: the newest. Its room runs out well before many more would fit.
#[cfg(any(feature = "web", test))]
const MAX_SET_ASIDE: usize = 5;

/// Where a browser sets aside what it could not read: its `localStorage`,
/// or a test's stand-in for it.
#[cfg(any(feature = "web", test))]
pub(super) trait Aside {
    /// Every key it holds; `Err` when they cannot be listed.
    fn keys(&self) -> Result<Vec<String>, ()>;
    /// What it holds under `key`; `Err` when that cannot be read.
    fn get(&self, key: &str) -> Result<Option<String>, ()>;
    /// Hold `raw` under `key`, and say whether it took.
    fn set(&self, key: &str, raw: &str) -> bool;
    fn remove(&self, key: &str);
}

/// Which copy set aside under `prefix` `key` is, counting from one: the
/// first is `prefix` itself, the ones after it `prefix_2`, `prefix_3`…
#[cfg(any(feature = "web", test))]
fn aside_number(prefix: &str, key: &str) -> Option<u64> {
    match key.strip_prefix(prefix)? {
        "" => Some(1),
        rest => {
            let digits = rest.strip_prefix('_')?;
            let number = digits.parse().ok()?;
            (digits.bytes().all(|b| b.is_ascii_digit()) && number > 1).then_some(number)
        }
    }
}

/// What became of a copy of what could not be read, set aside.
#[cfg(any(feature = "web", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kept {
    /// Set aside now.
    Now,
    /// Set aside before: the same copy is kept already.
    Already,
    /// Not kept.
    Not,
}

/// Keep `raw`, what could not be read, set aside in `store` under
/// `prefix`, and say whether it is kept. A copy set aside before is never
/// written over, and one set aside already is not kept twice.
///
/// Each copy goes under the number after the newest, so the numbers say
/// which is newer, and only the newest [`MAX_SET_ASIDE`] are kept. One that
/// does not fit lets the oldest go, one at a time, until it does; if it
/// does not fit even then, those go back as they were. A copy set aside is
/// there for a person to recover from, while a history that cannot be set
/// aside holds back every session finished after it: so old copies give
/// way to a new one, the oldest first.
#[cfg(any(feature = "web", test))]
fn keep_aside(store: &impl Aside, prefix: &str, raw: &str) -> Kept {
    let Ok(keys) = store.keys() else {
        return Kept::Not;
    };
    let mut numbered: Vec<(u64, String)> = keys
        .into_iter()
        .filter_map(|key| Some((aside_number(prefix, &key)?, key)))
        .collect();
    numbered.sort_unstable();
    let Some(next) = numbered
        .last()
        .map_or(Some(1), |(number, _)| number.checked_add(1))
    else {
        return Kept::Not;
    };
    // Each copy already set aside, oldest first, and what it holds.
    let mut older = Vec::with_capacity(numbered.len());
    for (_, key) in numbered {
        match store.get(&key) {
            Ok(Some(kept)) if kept == raw => return Kept::Already,
            Ok(Some(kept)) => older.push((key, kept)),
            Ok(None) => {}
            Err(()) => return Kept::Not,
        }
    }
    let key = match next {
        1 => prefix.to_string(),
        n => format!("{prefix}_{n}"),
    };
    let mut gone = 0;
    while !store.set(&key, raw) {
        let Some((oldest, _)) = older.get(gone) else {
            for (key, kept) in &older {
                store.set(key, kept);
            }
            return Kept::Not;
        };
        store.remove(oldest);
        gone += 1;
    }
    let first_kept = older.len().saturating_sub(MAX_SET_ASIDE - 1).max(gone);
    for (key, _) in &older[gone..first_kept] {
        store.remove(key);
    }
    Kept::Now
}

/// Set `unread`, what could not be read of `raw`, stored under `key`, aside
/// under `prefix` before anything writes over it, and say whether it is
/// kept. `rest` is what `key` is to hold once it is: the sessions that
/// could be read, or nothing, when none of `raw` could be.
///
/// A copy goes beside what is stored, if there is room for it. If there is
/// not, it is moved: `key` gives up its room first, holding only `rest`,
/// and the copy goes into the room that frees; should even that fail, `key`
/// is put back as it was. A history too large to copy beside itself used
/// to be held instead, for as long as the browser kept it, and every
/// session finished after it went unsaved.
#[cfg(any(feature = "web", test))]
pub(super) fn set_aside_from(
    store: &impl Aside,
    key: &str,
    raw: &str,
    prefix: &str,
    unread: &str,
    rest: Option<&str>,
) -> Kept {
    let kept = keep_aside(store, prefix, unread);
    if kept != Kept::Not {
        if rest.is_none() {
            store.remove(key);
        }
        return kept;
    }
    let room = match rest {
        Some(rest) => store.set(key, rest),
        None => {
            store.remove(key);
            true
        }
    };
    let moved = if room {
        keep_aside(store, prefix, unread)
    } else {
        Kept::Not
    };
    if moved == Kept::Not {
        store.set(key, raw);
    }
    moved
}

/// Store the newest of `sessions` that fit, with `set`, which writes a
/// history and says whether the store took it: as many as fit `budget`, and
/// if the store has less room than that, as many as fit in half as much,
/// and so on, down to the newest alone.
///
/// Never fewer than that: a store with no room even for the newest session
/// is full, and nothing is written. Halving used to go on below the newest
/// session's size, down to an empty list, which a store always takes; so a
/// browser short of room wrote an empty history over the whole of the one
/// it held, and said it had kept the newest none.
#[cfg(any(feature = "web", test))]
pub(super) fn store_newest_that_fit(
    sessions: &[SessionResult],
    budget: usize,
    mut set: impl FnMut(&str) -> bool,
) -> HistorySaved {
    let full = || HistorySaved::Failed("this browser's storage is full.".into());
    // The room the newest session takes on its own, as a list of one.
    let newest_alone = sessions
        .last()
        .and_then(|session| serde_json::to_string(session).ok())
        .map_or(0, |json| json.len() + "[]".len());
    let mut budget = budget;
    loop {
        let (raw, kept) = newest_that_fit(sessions, budget);
        if kept == 0 && !sessions.is_empty() {
            return full();
        }
        if set(&raw) {
            return if kept < sessions.len() {
                let runs = sessions.iter().any(|session| session.contest.is_some());
                HistorySaved::Squeezed { kept, runs }
            } else {
                HistorySaved::Stored { kept }
            };
        }
        if kept <= 1 {
            return full();
        }
        budget = (raw.len() / 2).max(newest_alone);
    }
}

/// The newest of `sessions` whose JSON fits in `budget` bytes, as one JSON
/// list, and how many it holds.
///
/// A browser gives a site a few megabytes, and a session's size follows its
/// number of groups, so the cap on a browser's history is room, not count.
#[cfg(any(feature = "web", test))]
fn newest_that_fit(sessions: &[SessionResult], budget: usize) -> (String, usize) {
    let mut parts: Vec<String> = Vec::new();
    let mut used = "[]".len();
    for session in sessions.iter().rev() {
        let Ok(json) = serde_json::to_string(session) else {
            continue;
        };
        let cost = json.len() + usize::from(!parts.is_empty());
        if used + cost > budget {
            break;
        }
        used += cost;
        parts.push(json);
    }
    let kept = parts.len();
    parts.reverse();
    (format!("[{}]", parts.join(",")), kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::tests::{at, session, stamps};
    use cw_core::CharSetMode;

    #[test]
    fn a_corrupt_session_is_dropped_instead_of_losing_the_file() {
        let good = serde_json::to_string(&session("2026-09-01")).unwrap();
        let raw = format!("[{good}, {{\"date\": \"nonsense\"}}]");
        let recovered = parse_sessions(&raw).expect("a list");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].date, "2026-09-01");
        // What it dropped is kept as it was stored, to be set aside.
        let read = read_history(&raw).expect("a list");
        assert_eq!(read.unread, [serde_json::json!({ "date": "nonsense" })]);
        assert_eq!(
            read.unread_json().as_deref(),
            Some(r#"[{"date":"nonsense"}]"#)
        );
        assert_eq!(
            read_history(&format!("[{good}]")).unwrap().unread_json(),
            None
        );
        // A whole list reads in one go.
        let both = format!("[{good},{good}]");
        assert_eq!(parse_sessions(&both).map(|s| s.len()), Some(2));
        // A file that is not even a list is unreadable, which is not the same
        // as empty: the caller has to keep it safe.
        assert!(parse_sessions("{}").is_none());
        assert!(parse_sessions("").is_none());
        assert_eq!(parse_sessions("[]"), Some(Vec::new()));
    }

    /// A browser's store, as far as setting aside goes: `room` characters
    /// in all, as a browser's quota has, and keys of other kinds beside.
    struct Shelf {
        held: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
        room: std::cell::Cell<usize>,
        unreadable: std::cell::Cell<bool>,
    }

    impl Shelf {
        fn with_room(room: usize) -> Self {
            let shelf = Self {
                held: Default::default(),
                room: std::cell::Cell::new(room),
                unreadable: Default::default(),
            };
            shelf.set("history", "[]");
            shelf
        }

        /// What is set aside, by key.
        fn aside(&self) -> Vec<(String, String)> {
            let held = self.held.borrow();
            held.iter()
                .filter(|(key, _)| key.starts_with("aside"))
                .map(|(key, raw)| (key.clone(), raw.clone()))
                .collect()
        }
    }

    impl Aside for Shelf {
        fn keys(&self) -> Result<Vec<String>, ()> {
            if self.unreadable.get() {
                return Err(());
            }
            Ok(self.held.borrow().keys().cloned().collect())
        }

        fn get(&self, key: &str) -> Result<Option<String>, ()> {
            if self.unreadable.get() {
                return Err(());
            }
            Ok(self.held.borrow().get(key).cloned())
        }

        fn set(&self, key: &str, raw: &str) -> bool {
            let mut held = self.held.borrow_mut();
            let used: usize = held
                .iter()
                .filter(|(held, _)| *held != key)
                .map(|(_, raw)| raw.len())
                .sum();
            if used + raw.len() > self.room.get() {
                return false;
            }
            held.insert(key.into(), raw.into());
            true
        }

        fn remove(&self, key: &str) {
            self.held.borrow_mut().remove(key);
        }
    }

    fn shelved(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, raw)| (key.to_string(), raw.to_string()))
            .collect()
    }

    /// What could not be read is set aside under a key of its own: never
    /// over a copy set aside before, and never twice.
    #[test]
    fn what_could_not_be_read_is_set_aside_beside_what_was_before() {
        let shelf = Shelf::with_room(usize::MAX);
        assert_eq!(keep_aside(&shelf, "aside", "first"), Kept::Now);
        assert_eq!(keep_aside(&shelf, "aside", "second"), Kept::Now);
        assert_eq!(keep_aside(&shelf, "aside", "first"), Kept::Already);
        assert_eq!(
            shelf.aside(),
            shelved(&[("aside", "first"), ("aside_2", "second")])
        );
        // A store that cannot be read: not kept, and nothing changed.
        shelf.unreadable.set(true);
        assert_eq!(keep_aside(&shelf, "aside", "third"), Kept::Not);
        shelf.unreadable.set(false);
        assert_eq!(shelf.aside().len(), 2);
        assert_eq!(aside_number("aside", "asidea"), None);
        assert_eq!(aside_number("aside", "aside_1"), None);
        assert_eq!(aside_number("aside", "aside_+3"), None);
        assert_eq!(aside_number("aside", "aside_12"), Some(12));
    }

    /// Only the newest copies are kept. Every slot a browser had used to
    /// stay full for good, and the next history that could not be read
    /// could not be set aside, so nothing was saved over it again.
    #[test]
    fn only_the_newest_copies_set_aside_are_kept() {
        let shelf = Shelf::with_room(usize::MAX);
        // As twenty slots were filled before there was a limit.
        shelf.set("aside", "1");
        for n in 2..=20 {
            shelf.set(&format!("aside_{n}"), &n.to_string());
        }
        assert_eq!(keep_aside(&shelf, "aside", "new"), Kept::Now);
        assert_eq!(
            shelf.aside(),
            shelved(&[
                ("aside_17", "17"),
                ("aside_18", "18"),
                ("aside_19", "19"),
                ("aside_20", "20"),
                ("aside_21", "new"),
            ])
        );
        assert_eq!(keep_aside(&shelf, "aside", "newer"), Kept::Now);
        assert_eq!(shelf.aside().len(), MAX_SET_ASIDE);
        assert_eq!(
            shelf.aside().last(),
            Some(&("aside_22".into(), "newer".into()))
        );
    }

    /// A copy that does not fit lets the oldest go until it does; one that
    /// would not fit even then leaves every copy as it was.
    #[test]
    fn a_copy_set_aside_makes_room_by_letting_the_oldest_go() {
        // The history's own room, and a little short of room for three.
        let shelf = Shelf::with_room(2 + 4 + 4 + 5);
        assert_eq!(keep_aside(&shelf, "aside", "aaaa"), Kept::Now);
        assert_eq!(keep_aside(&shelf, "aside", "bbbb"), Kept::Now);
        assert_eq!(
            keep_aside(&shelf, "aside", "cccccc"),
            Kept::Now,
            "room made"
        );
        assert_eq!(
            shelf.aside(),
            shelved(&[("aside_2", "bbbb"), ("aside_3", "cccccc")])
        );
        assert_eq!(
            keep_aside(&shelf, "aside", "far too long for it"),
            Kept::Not
        );
        assert_eq!(
            shelf.aside(),
            shelved(&[("aside_2", "bbbb"), ("aside_3", "cccccc")]),
            "put back as they were"
        );
    }

    /// A history too large to copy beside itself is moved aside into its
    /// own room; one only some of whose sessions could not be read keeps
    /// the rest. It used to be held instead, for as long as the browser
    /// kept it, and every session finished after it went unsaved.
    #[test]
    fn a_history_too_large_to_copy_beside_itself_is_moved_aside() {
        let (raw, unread, rest) = ("r".repeat(100), "u".repeat(60), "g".repeat(40));
        // Room for the history and a little more, never for it twice.
        let shelf = Shelf::with_room(150);
        shelf.set("history", &raw);
        let kept = set_aside_from(&shelf, "history", &raw, "aside", &raw, None);
        assert_eq!(kept, Kept::Now);
        assert_eq!(shelf.aside(), shelved(&[("aside", raw.as_str())]));
        assert_eq!(shelf.get("history"), Ok(None), "out of the way of a save");

        let shelf = Shelf::with_room(150);
        shelf.set("history", &raw);
        let kept = set_aside_from(&shelf, "history", &raw, "aside", &unread, Some(&rest));
        assert_eq!(kept, Kept::Now);
        assert_eq!(shelf.aside(), shelved(&[("aside", unread.as_str())]));
        assert_eq!(shelf.get("history"), Ok(Some(rest.clone())));

        // With room to copy, the history stays as it is until a save.
        let shelf = Shelf::with_room(usize::MAX);
        shelf.set("history", &raw);
        let kept = set_aside_from(&shelf, "history", &raw, "aside", &unread, Some(&rest));
        assert_eq!(kept, Kept::Now);
        assert_eq!(shelf.get("history"), Ok(Some(raw.clone())));
        let again = set_aside_from(&shelf, "history", &raw, "aside", &unread, Some(&rest));
        assert_eq!(again, Kept::Already, "not twice");

        // Where nothing can be set aside, the history is put back as it was.
        let shelf = Shelf::with_room(150);
        shelf.set("history", &raw);
        shelf.unreadable.set(true);
        let kept = set_aside_from(&shelf, "history", &raw, "aside", &raw, None);
        assert_eq!(kept, Kept::Not);
        shelf.unreadable.set(false);
        assert_eq!(shelf.get("history"), Ok(Some(raw)));
    }

    /// A browser's history is capped by room. Large sessions — two hundred
    /// groups each — used to overflow the quota by count, and the oldest half
    /// went at a time until something fitted, with nobody told.
    /// A history saved under an older score reads on today's scale: every
    /// session is scored again from what is stored of it.
    #[test]
    fn a_history_reads_scored_the_way_sessions_score_today() {
        let mut stored = session("2026-01-01");
        stored.groups = vec![cw_core::GroupResult {
            sent: "KMURE".into(),
            received: "KMURE".into(),
            correct: true,
        }];
        stored.group_timings = vec![cw_core::SessionTiming {
            time_to_complete_ms: 500.0,
            char_wpm: Some(35.0),
            effective_wpm: None,
            sends: 1,
            ..cw_core::SessionTiming::default()
        }];
        stored.char_wpm = 35.0;
        stored.effective_wpm = 35.0;
        stored.score = 1.0;
        let raw = serde_json::to_string(&[stored.clone()]).unwrap();
        let back = parse_sessions(&raw).expect("a list");
        assert_eq!(back[0].score, stored.points());
        assert!(back[0].score > 1.0, "{}", back[0].score);
    }

    /// A session finished with the clock set ahead reads as it was stored,
    /// stamp and day, and so do those stored after a clock set behind: the
    /// clock now may be the one that is wrong, and what is read is what the
    /// next save writes.
    #[test]
    fn a_history_reads_as_stamped_whatever_the_clock_says() {
        let now = crate::time::now_ms();
        let ahead = SessionResult {
            timestamp: now + 365 * 86_400_000,
            ..session("2099-10-30")
        };
        let raw = serde_json::to_string(&[at(1), at(2), at(3), ahead.clone()]).unwrap();
        let back = parse_sessions(&raw).expect("a list");
        assert_eq!(stamps(&back), [1, 2, 3, ahead.timestamp]);
        assert_eq!(back[3].date, "2099-10-30");
    }

    /// A contest run a later version wrote reads as a run, and goes back to
    /// the store as it was written: a callsign session with its mark, and
    /// the log this version cannot read yet.
    #[test]
    fn a_contest_run_from_a_later_version_is_kept_whole() {
        let mut written = serde_json::to_value(session("2026-10-20")).unwrap();
        written["charSetMode"] = serde_json::json!("callsign");
        written["alphabetFingerprint"] = serde_json::json!("contest:wpx");
        written["contest"] = serde_json::json!({ "v": 9, "laterKey": [1, 2.5, "x"] });
        let raw = serde_json::to_string(&[written.clone()]).unwrap();
        let back = parse_sessions(&raw).expect("a list");
        assert_eq!(back[0].char_set_mode, CharSetMode::Contest);
        let (saved, kept) = newest_that_fit(&back, usize::MAX);
        assert_eq!(kept, 1);
        let saved: Vec<serde_json::Value> = serde_json::from_str(&saved).unwrap();
        assert_eq!(saved[0]["charSetMode"], "callsign");
        assert_eq!(saved[0]["alphabetFingerprint"], "contest:wpx");
        assert_eq!(saved[0]["contest"], written["contest"]);
    }

    #[test]
    fn only_the_newest_sessions_that_fit_are_kept() {
        let all: Vec<SessionResult> = (0..10)
            .map(|i| session(&format!("2026-01-{i:02}")))
            .collect();
        let one = serde_json::to_string(&all[0]).unwrap().len();
        let (raw, kept) = newest_that_fit(&all, 2 + 3 * one + 2);
        assert_eq!(kept, 3);
        let back = parse_sessions(&raw).expect("a list");
        assert_eq!(
            back.iter().map(|s| s.date.as_str()).collect::<Vec<_>>(),
            ["2026-01-07", "2026-01-08", "2026-01-09"],
            "the newest, oldest first"
        );
        assert!(raw.len() <= 2 + 3 * one + 2);

        let (raw, kept) = newest_that_fit(&all, usize::MAX);
        assert_eq!(
            (kept, parse_sessions(&raw).map(|s| s.len())),
            (10, Some(10))
        );
        assert_eq!(newest_that_fit(&all, 3), ("[]".to_string(), 0));
    }

    /// A store short of room keeps the newest sessions that fit, down to
    /// the newest alone, and with no room even for that writes nothing:
    /// halving its room used to end at an empty list, which went over the
    /// whole history as "the newest 0".
    #[test]
    fn a_store_short_of_room_never_writes_an_empty_history() {
        let mut all: Vec<SessionResult> = (0..5)
            .map(|i| session(&format!("2026-01-{i:02}")))
            .collect();
        // The newest is far larger than the others, as a long run is.
        all[4].alphabet_fingerprint = "x".repeat(20_000);
        let newest = serde_json::to_string(&all[4]).unwrap().len() + 2;
        let writes = std::cell::RefCell::new(Vec::<String>::new());
        let written = &writes;
        let with_room = |room: usize| {
            move |raw: &str| {
                written.borrow_mut().push(raw.to_string());
                raw.len() <= room
            }
        };
        // Room for the newest alone, but not twice it.
        let saved = store_newest_that_fit(&all, usize::MAX, with_room(newest + 10));
        assert_eq!(
            saved,
            HistorySaved::Squeezed {
                kept: 1,
                runs: false
            }
        );
        let last = written.borrow().last().cloned().unwrap();
        assert_eq!(parse_sessions(&last).map(|s| s.len()), Some(1));
        assert!(written.borrow().iter().all(|raw| raw != "[]"));
        // No room even for that: nothing is written over what is there.
        written.borrow_mut().clear();
        let saved = store_newest_that_fit(&all, usize::MAX, with_room(newest - 1));
        assert!(matches!(saved, HistorySaved::Failed(_)), "{saved:?}");
        assert!(written.borrow().iter().all(|raw| raw != "[]"));
        // Room for everything: everything.
        let saved = store_newest_that_fit(&all, usize::MAX, |_| true);
        assert_eq!(saved, HistorySaved::Stored { kept: 5 });
        // A history every session was deleted from is stored empty.
        let saved = store_newest_that_fit(&[], usize::MAX, |raw| raw == "[]");
        assert_eq!(saved, HistorySaved::Stored { kept: 0 });
    }
}
