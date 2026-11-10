//! The history as builds before records kept it: whole, as one JSON list
//! of every session, in `sessions.json` or a browser's `dust_sessions`.
//!
//! How such a history is read, what of it that does not read is set aside,
//! and, in a browser, how much of it is kept for the room there is. Older
//! builds still read and write it, and the move to records reads it once
//! and leaves it as it was for them ([`moved`]), then takes up whatever an
//! older build stores there after it ([`imported`]). A browser keeps it
//! whole, behind the record API, where its database will not open
//! ([`BlobHistory`]).

#[cfg(any(feature = "web", test))]
use std::cell::{Cell, RefCell};
#[cfg(any(feature = "web", test))]
use std::collections::BTreeMap;
use std::collections::{BTreeSet, HashMap, HashSet};

use cw_core::SessionResult;
use serde::Deserialize;
use serde_json::value::RawValue;

use super::record::{self, HistoryChange, RecordId};
#[cfg(any(feature = "web", test))]
use super::record::{Deletions, HistoryLoad, Record};
#[cfg(any(feature = "web", test))]
use super::saved::HistorySaved;
use super::session::{Deleted, SessionKey};
#[cfg(any(feature = "web", test))]
use super::set_aside::SetAside;

/// How much of the site's page storage the history kept whole may take, in
/// characters. Browsers give a site about five million; the rest is left
/// for the settings, the counters and the room a write needs. Other sites
/// on the origin may have taken some of it, and then a write that does not
/// fit keeps fewer of the newest sessions ([`store_newest_that_fit`]).
///
/// Only a history kept whole has such a limit: records have none.
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
///
/// Beside each session that reads is its text, as the history holds it: a
/// browser that keeps the history whole writes that back of a session a
/// change leaves as it is (`BlobHistory::apply`, built for the web alone).
pub(super) fn read_history(raw: &str) -> Option<ReadHistory<'_>> {
    let raw = super::settings::unmarked(raw);
    let held: Vec<&RawValue> = serde_json::from_str(raw).ok()?;
    // Almost always every session reads; only when one does not is it
    // worth reading each again, as JSON, to keep the ones that do.
    let read: Result<Vec<SessionResult>, _> = held
        .iter()
        .map(|session| serde_json::from_str(session.get()))
        .collect();
    let (sessions, stored, unread) = match read {
        Ok(sessions) => (sessions, held.iter().map(|s| s.get()).collect(), Vec::new()),
        Err(_) => {
            let (mut sessions, mut stored, mut unread) = (Vec::new(), Vec::new(), Vec::new());
            for session in held {
                let value: serde_json::Value = serde_json::from_str(session.get()).ok()?;
                match SessionResult::deserialize(&value) {
                    Ok(read) => {
                        sessions.push(read);
                        stored.push(session.get());
                    }
                    Err(_) => unread.push(value),
                }
            }
            (sessions, stored, unread)
        }
    };
    let sessions = sessions.into_iter().map(record::scored).collect();
    Some(ReadHistory {
        sessions,
        stored,
        unread,
    })
}

/// The sessions of a stored history that still parse ([`read_history`]),
/// where nothing is ever written over it: a backup file, as Dust 0.16 read
/// one ([`super::backup::read_backup_as_0_16_2`]).
#[cfg(test)]
pub(super) fn parse_sessions(raw: &str) -> Option<Vec<SessionResult>> {
    read_history(raw).map(|read| read.sessions)
}

/// A stored history as it was read ([`read_history`]).
#[derive(Debug, PartialEq)]
pub(super) struct ReadHistory<'a> {
    /// Every session that still parses, oldest first.
    pub(super) sessions: Vec<SessionResult>,
    /// The text of each of them, by its place there, as the history holds
    /// it.
    pub(super) stored: Vec<&'a str>,
    /// Every one that does not, as it is stored.
    pub(super) unread: Vec<serde_json::Value>,
}

impl ReadHistory<'_> {
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

/// One store seen through a reference: two copies of the app on one, as
/// two tabs share a browser's.
#[cfg(any(feature = "web", test))]
impl<T: Aside> Aside for &T {
    fn keys(&self) -> Result<Vec<String>, ()> {
        (**self).keys()
    }

    fn get(&self, key: &str) -> Result<Option<String>, ()> {
        (**self).get(key)
    }

    fn set(&self, key: &str, raw: &str) -> bool {
        (**self).set(key, raw)
    }

    fn remove(&self, key: &str) {
        (**self).remove(key);
    }
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
pub(super) fn keep_aside(store: &impl Aside, prefix: &str, raw: &str) -> Kept {
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
/// is full, and nothing is written ([`HistorySaved::Full`]): the app keeps
/// what it would have stored until there is room. Halving used to go on
/// below the newest session's size, down to an empty list, which a store
/// always takes; so a browser short of room wrote an empty history over the
/// whole of the one it held, and said it had kept the newest none.
///
/// What it lets go is said by how many it kept: the oldest of `sessions`
/// ([`BlobHistory::apply`] names them).
///
/// Each session goes in as the text beside it: what the history held of
/// it, for one a change leaves as it is, so that a tab of an older build
/// that shares the history reads it as it did.
#[cfg(any(feature = "web", test))]
pub(super) fn store_newest_that_fit(
    sessions: &[(impl AsRef<str>, &SessionResult)],
    budget: usize,
    mut set: impl FnMut(&str) -> bool,
) -> HistorySaved {
    let full = || HistorySaved::Full;
    // The room the newest session takes on its own, as a list of one.
    let newest_alone = sessions
        .last()
        .map_or(0, |(json, _)| json.as_ref().len() + "[]".len());
    let mut budget = budget;
    loop {
        let (raw, kept) = newest_that_fit(sessions, budget);
        if kept == 0 && !sessions.is_empty() {
            return full();
        }
        if set(&raw) {
            return if kept < sessions.len() {
                let runs = sessions
                    .iter()
                    .any(|(_, session)| session.contest.is_some());
                HistorySaved::Squeezed {
                    kept,
                    runs,
                    trimmed: Vec::new(),
                }
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

/// The newest of `sessions` whose JSON, the text beside each, fits in
/// `budget` bytes, as one JSON list, and how many it holds.
///
/// A browser gives a site a few megabytes, and a session's size follows its
/// number of groups, so the cap on a browser's history is room, not count.
#[cfg(any(feature = "web", test))]
fn newest_that_fit(
    sessions: &[(impl AsRef<str>, &SessionResult)],
    budget: usize,
) -> (String, usize) {
    let mut parts: Vec<&str> = Vec::new();
    let mut used = "[]".len();
    for (json, _) in sessions.iter().rev() {
        let json = json.as_ref();
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

/// The layout of the history this build writes: records and tombstones
/// beside them ([`super::record`]).
pub(super) const LAYOUT: u64 = 1;

/// The oldest layout a build has to know to write to a history: a later
/// build that keeps it so that this one would harm it says so by raising
/// it, and this one then only reads ([`Gate::Newer`]).
pub(super) const COMPATIBLE: u64 = 1;

/// How long before the newest session the move took
/// ([`Layout::taken_through`]) a session may have finished and still be one
/// an older build stored after the move: one finished a day before it,
/// stored by a copy left open since, is still taken up.
const MOVE_SLACK_MS: u64 = 86_400_000;

/// How the history is kept, and what the move from the history kept whole
/// took of it: `layout.json` beside the records, written last, so that a
/// move cut short is made again.
///
/// Whatever a later build adds to it is carried along as it was read.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Layout {
    /// The layout written ([`LAYOUT`]).
    pub(super) layout: u64,
    /// The oldest layout a build has to know to write to it
    /// ([`COMPATIBLE`]).
    pub(super) compatible: u64,
    /// When the history kept whole was moved, in milliseconds since the
    /// epoch.
    pub(super) migrated_at: u64,
    /// The history kept whole, as it was when last taken up: none when
    /// there was none.
    #[serde(default)]
    pub(super) legacy: Option<Fingerprint>,
    /// Every session taken up from the history kept whole, by key: none of
    /// them is ever taken up again, though an older build keeps it there.
    #[serde(default)]
    pub(super) imported_keys: Vec<SessionKey>,
    /// When the latest deletion taken up from an older build was made.
    /// Builds before [`Self::deletions_taken`] take up each made after it,
    /// and move it on: this build moves it on as they do, for them.
    #[serde(default)]
    pub(super) deleted_seen: u64,
    /// The newest session the move took that finished no later than the
    /// clock said as it was made, `takenThrough`: a session finished after
    /// it, less a day, is one an older build may have stored since
    /// ([`imported`]). The clock as the move was made could be ahead by
    /// years, and a session stamped ahead of it by more. Where the move
    /// took none, what stands for it ([`taken_through`]). None, where an
    /// earlier build moved the history, and nothing has been taken up
    /// since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) taken_through: Option<u64>,
    /// Every deletion an older build made that the history here has taken,
    /// `deletionsTaken` ([`DeletionsTaken`]): one not among them is made
    /// here, whatever its stamp. None, where an earlier build wrote the
    /// layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) deletions_taken: Option<DeletionsTaken>,
    /// The history kept whole as this build's take-up last took it,
    /// `takenLegacy`: a build before its rules, which takes up a change
    /// there by its own, moves [`Self::legacy`] on and carries this as it
    /// is, so this build takes that change up again by its own
    /// ([`Self::taken_up`]). None, where an earlier build wrote the layout;
    /// noted as none where there was no history kept whole.
    #[serde(
        default,
        deserialize_with = "noted",
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) taken_legacy: Option<Option<Fingerprint>>,
    /// What a later build added.
    #[serde(flatten)]
    pub(super) unread: serde_json::Map<String, serde_json::Value>,
}

impl Layout {
    /// The history kept whole as this build last took it up: its own note
    /// ([`Self::taken_legacy`]), or, where an earlier build wrote the
    /// layout, the one every build moves on ([`Self::legacy`]). A change
    /// there since is taken up by [`imported`], though a build before this
    /// one's rules took it up first.
    pub(super) fn taken_up(&self) -> Option<Fingerprint> {
        self.taken_legacy.unwrap_or(self.legacy)
    }
}

/// A key a layout holds, as it holds it, a null among it: none only where
/// it holds no such key ([`Layout::taken_legacy`]).
fn noted<'de, D, T>(read: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(read).map(Some)
}

/// The deletions an older build made that the history here has taken, by
/// what each is, and when they were noted ([`Layout::deletions_taken`]):
/// `{"seen": …, "taken": [[timestamp, "date", at], …]}`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct DeletionsTaken {
    /// [`Layout::deleted_seen`] as this build left it beside them. Where it
    /// says more now, a build that does not note them took deletions up
    /// since, as it does, by when they were made: each made no later than
    /// that is taken.
    pub(super) seen: u64,
    /// Each deletion taken, of those an older build still remembers.
    pub(super) taken: BTreeSet<DeletionSeen>,
    /// What a later build added.
    #[serde(flatten)]
    pub(super) unread: serde_json::Map<String, serde_json::Value>,
}

/// A deletion an older build made, as the take-up tells it from another
/// ([`DeletionsTaken`]): when its session finished, on what day, and when
/// it was deleted, `[timestamp, "date", at]`. The same deletion remembered
/// again is the same.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub(super) struct DeletionSeen(u64, String, u64);

impl DeletionSeen {
    pub(super) fn of(gone: &Deleted) -> Self {
        Self(gone.timestamp, gone.date.clone(), gone.at)
    }
}

/// What tells one state of the history kept whole from another without
/// reading it: its length, and when it was last written (on a computer or
/// a phone, in nanoseconds since the epoch). A browser keeps no such time:
/// there it is the hash of what the history holds, and the length is in the
/// browser's characters ([`super::idb`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Fingerprint {
    pub(super) len: u64,
    pub(super) mtime: u64,
}

/// Whether, and how, this build may write to a history.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Gate {
    /// Moved, in a layout this build writes.
    Open(Layout),
    /// Not moved yet: there is no layout. Records are still read and
    /// written; the move is made when it can be.
    Unmoved,
    /// Kept by a newer build, in a way this one would harm: read, and
    /// written by nothing.
    Newer,
    /// There is a layout, and it does not read: a file damaged where it
    /// lies. It is set aside, and the move made again, which writes nothing
    /// twice.
    Unread,
}

impl Gate {
    /// The gate a layout stored as `raw` sets, or none being stored.
    ///
    /// A later layout is judged by `compatible` alone, read before the
    /// rest: whatever else a later build wrote, this one need not read to
    /// know it may not write.
    pub(super) fn of(raw: Option<&[u8]>) -> Self {
        let Some(raw) = raw else {
            return Self::Unmoved;
        };
        let Ok(serde_json::Value::Object(held)) = serde_json::from_slice(raw) else {
            return Self::Unread;
        };
        match held.get("compatible").and_then(serde_json::Value::as_u64) {
            Some(compatible) if compatible > COMPATIBLE => Self::Newer,
            Some(_) => serde_json::from_value(serde_json::Value::Object(held))
                .map_or(Self::Unread, Self::Open),
            None => Self::Unread,
        }
    }
}

impl Layout {
    /// The layout as it is written: pretty, as a file somebody may open.
    pub(super) fn written(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self)
    }
}

/// The latest deletion `deleted` holds, as a watermark: none, as nought.
fn latest(deleted: &[Deleted]) -> u64 {
    deleted.iter().map(|gone| gone.at).max().unwrap_or(0)
}

/// The deletions an older build remembers, `raw` being what it keeps of
/// them as it wrote it: none, where it keeps none or they cannot be read,
/// which costs a session it deleted staying, never one lost.
pub(super) fn deleted_of(raw: Option<&[u8]>) -> Vec<Deleted> {
    raw.and_then(|raw| serde_json::from_slice(raw).ok())
        .unwrap_or_default()
}

/// The move from a history kept whole: each of `sessions`, oldest first,
/// as a record ([`record::records_for_blob`]), and the layout to write once
/// every record is, `now`. `fingerprint` is the history kept whole as it
/// was read, and `deleted` the deletions it remembers, already done to it,
/// and so taken. What a later take-up looks at is cut off at the newest
/// session moved that finished no later than `now`, or what stands for it
/// where it moves none ([`taken_through`]): a clock ahead of the time, or a
/// session stamped ahead of the clock, holds back no session an older build
/// stores after the move.
pub(super) fn moved(
    sessions: &[SessionResult],
    fingerprint: Option<Fingerprint>,
    deleted: &[Deleted],
    now: u64,
) -> (Vec<record::Record>, Layout) {
    let mut keys: Vec<SessionKey> = Vec::with_capacity(sessions.len());
    let mut seen = HashSet::new();
    for key in sessions.iter().map(SessionKey::of) {
        if seen.insert(key.clone()) {
            keys.push(key);
        }
    }
    let layout = Layout {
        layout: LAYOUT,
        compatible: COMPATIBLE,
        migrated_at: now,
        legacy: fingerprint,
        imported_keys: keys,
        deleted_seen: latest(deleted),
        taken_through: Some(taken_through(sessions, fingerprint.is_some(), deleted, now)),
        deletions_taken: Some(DeletionsTaken {
            seen: latest(deleted),
            taken: deleted.iter().map(DeletionSeen::of).collect(),
            unread: serde_json::Map::new(),
        }),
        taken_legacy: Some(fingerprint),
        unread: serde_json::Map::new(),
    };
    (record::records_for_blob(sessions), layout)
}

/// The newest of `sessions`, those a move takes, that finished no later
/// than `now`, the clock as it is made ([`Layout::taken_through`]).
///
/// Where it takes none, the newest session the history kept whole says it
/// deleted, `deleted`, at the clock or before it, stands for it: what an
/// older copy left open brings back from before that stays out. Where it
/// says it deleted none either, and the move found no history kept whole
/// (`kept_whole` false: none was there, or what was there did not read and
/// was set aside), no session it took can come back, and an older build's
/// found there after it is one the history here never held: nothing is cut
/// off. Else the clock: an empty history says nothing of what came before
/// it.
fn taken_through(
    sessions: &[SessionResult],
    kept_whole: bool,
    deleted: &[Deleted],
    now: u64,
) -> u64 {
    let newest = |finished: &mut dyn Iterator<Item = u64>| finished.filter(|&at| at <= now).max();
    newest(&mut sessions.iter().map(|session| session.timestamp))
        .or_else(|| newest(&mut deleted.iter().map(|gone| gone.timestamp)))
        .unwrap_or(if kept_whole { now } else { 0 })
}

/// Of `moved`, the records a move makes, those to write into a store that
/// holds records already, each store saying for itself what it holds: one
/// there under its own name, as `here` says, is written; one deleted by
/// name, in `gone`, is not; one whose session is there under a name the
/// move does not give, as `elsewhere`, what each record there under such a
/// name holds, holds it ([`as_moved`]), is not, as many times as they hold
/// it; nor is one whose session's key a tombstone names, in `gone_keys`.
/// The rest are written.
pub(super) fn not_moved_yet(
    moved: Vec<record::Record>,
    here: impl Fn(RecordId) -> bool,
    gone: &BTreeSet<RecordId>,
    elsewhere: impl IntoIterator<Item = impl AsRef<str>>,
    gone_keys: &HashSet<SessionKey>,
) -> Vec<record::Record> {
    let mut held: HashMap<String, usize> = HashMap::new();
    for json in elsewhere {
        *held.entry(as_moved(json.as_ref())).or_default() += 1;
    }
    moved
        .into_iter()
        .filter(|record| {
            if here(record.id) {
                return true;
            }
            if gone.contains(&record.id) {
                return false;
            }
            if !held.is_empty()
                && let Some(count) = held
                    .get_mut(&as_moved(&record.json))
                    .filter(|count| **count > 0)
            {
                *count -= 1;
                return false;
            }
            !record::key_of(&record.json).is_some_and(|key| gone_keys.contains(&key))
        })
        .collect()
}

/// What a record holds, `json`, as a move tells it from what another holds
/// ([`not_moved_yet`]): the session without the format it says it is stored
/// in ([`cw_core::SESSION_FORMAT`]). Builds before the format wrote none,
/// and one of them writes this build's back with it at the end, as a key it
/// does not know: as they are written, the move's records, this build's,
/// would never be the bytes an older build stored of the same session.
/// Text that holds no JSON object is itself.
fn as_moved(json: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(serde_json::Value::Object(session)) => {
            let session: serde_json::Map<String, serde_json::Value> = session
                .into_iter()
                .filter(|(key, _)| key != "format")
                .collect();
            serde_json::to_string(&session).unwrap_or_else(|_| json.to_string())
        }
        _ => json.to_string(),
    }
}

/// What to take up of a history kept whole that an older build changed
/// since the move: `sessions`, what it holds now, as `fingerprint`, and
/// `deleted`, the deletions it remembers. `live` is every record here by
/// its session's key, and `gone` the key of every one deleted here.
///
/// A session is taken up once, where it finished after the newest the move
/// took ([`Layout::taken_through`]), less a day, and is neither here
/// already nor deleted here: only one an older build finished since, or a
/// copy of it left open stored since. One from before, which an older copy
/// brings back as it stores the history it held, is never taken up again,
/// nor one deleted here. Each goes where it finished ([`RecordId::placed`]).
/// A layout an earlier build wrote says nothing of what the move took: the
/// newest session taken, at the clock as the move was made or before it,
/// stands for it, and is noted.
///
/// A deletion the older build made is made here too, of every record with
/// its key, as it would have been there, once: each taken is noted
/// ([`Layout::deletions_taken`]), whatever its stamp says. Where a layout
/// notes none, as an earlier build wrote it, each made no later than the
/// latest taken is taken, as such builds take them; where a build that
/// notes none has taken deletions up since ([`DeletionsTaken::seen`]),
/// each it took, made after what this build had seen and no later than the
/// latest taken now. One stamped no later than what this build had seen,
/// and not noted, it did not take, and is made here.
///
/// A change a build before these rules took up first, by its own, is
/// taken up again by them: such a build moves [`Layout::legacy`] on, and
/// leaves this build's note of what it took as it was
/// ([`Layout::taken_up`]).
///
/// The change and the layout to write once it is made, or none where
/// there is nothing new.
pub(super) fn imported(
    layout: &Layout,
    fingerprint: Option<Fingerprint>,
    sessions: &[SessionResult],
    deleted: &[Deleted],
    live: &HashMap<SessionKey, Vec<RecordId>>,
    gone: &HashSet<SessionKey>,
) -> Option<(HistoryChange, Layout)> {
    if fingerprint == layout.taken_up() {
        return None;
    }
    let taken: HashSet<&SessionKey> = layout.imported_keys.iter().collect();
    let through = layout.taken_through.unwrap_or_else(|| {
        layout
            .imported_keys
            .iter()
            .map(SessionKey::timestamp)
            .filter(|&finished| finished <= layout.migrated_at)
            .max()
            .unwrap_or(layout.migrated_at)
    });
    let since = through.saturating_sub(MOVE_SLACK_MS);
    let mut change = HistoryChange::default();
    let mut put = HashSet::new();
    let mut keys = Vec::new();
    for session in sessions {
        let key = SessionKey::of(session);
        let new = session.timestamp > since
            && !taken.contains(&key)
            && !gone.contains(&key)
            && !live.contains_key(&key);
        let Some(record) = new
            .then(|| record::Record::of(session, |json| RecordId::placed(session.timestamp, json)))
            .flatten()
        else {
            continue;
        };
        // Sessions alike in everything are one session from elsewhere.
        if put.insert(record.id) {
            change.put.push(record);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    let mut made = layout
        .deletions_taken
        .as_ref()
        .map(|noted| noted.taken.clone())
        .unwrap_or_default();
    // Taken by a build that takes them by when they were made, each made
    // after the latest it had taken and moved on to the latest it took: of
    // a layout that notes none, each made no later than that; of one this
    // build noted them in, each made after what it had seen as it did, no
    // later than that. One made no later than what it had seen that it did
    // not note, made since with a clock behind, no such build took.
    let seen = layout.deletions_taken.as_ref().map(|noted| noted.seen);
    if seen != Some(layout.deleted_seen) {
        made.extend(
            deleted
                .iter()
                .filter(|gone| gone.at <= layout.deleted_seen)
                .filter(|gone| seen.is_none_or(|seen| gone.at > seen))
                .map(DeletionSeen::of),
        );
    }
    for gone in deleted
        .iter()
        .filter(|gone| !made.contains(&DeletionSeen::of(gone)))
    {
        for id in live.get(&gone.key()).into_iter().flatten() {
            change.forget.push((*id, gone.clone()));
        }
    }
    let mut next = layout.clone();
    next.legacy = fingerprint;
    next.taken_legacy = Some(fingerprint);
    next.imported_keys.extend(keys);
    next.deleted_seen = layout.deleted_seen.max(latest(deleted));
    next.taken_through = Some(through);
    // Every deletion the older build remembers is taken now; one it no
    // longer remembers need not be noted. None remembered may be a list
    // that could not be read: what was noted stays.
    let taken = if deleted.is_empty() {
        made
    } else {
        deleted.iter().map(DeletionSeen::of).collect()
    };
    next.deletions_taken = Some(DeletionsTaken {
        seen: next.deleted_seen,
        taken,
        unread: layout
            .deletions_taken
            .as_ref()
            .map(|noted| noted.unread.clone())
            .unwrap_or_default(),
    });
    Some((change, next))
}

/// Where a history kept whole is stored, in a browser's `localStorage`: the
/// history, the deletions it remembers, and the prefix of what of it is set
/// aside.
#[cfg(any(feature = "web", test))]
pub(super) struct BlobKeys {
    pub(super) history: &'static str,
    pub(super) deleted: &'static str,
    pub(super) aside: &'static str,
}

/// The history kept whole, taking changes by record
/// ([`super::Store::change_history`]): how a browser keeps it for a visit
/// its database will not open, or will not take the history moved into it
/// ([`super::idb`]).
///
/// Each change reads the whole history, does to it what the change says,
/// and writes it whole again, as stores did before records, keeping the
/// newest that fit its room, `budget` ([`store_newest_that_fit`]), and
/// naming the oldest it let go for want of it: the app lets them go too,
/// and says so, rather than store them again with every change. Its
/// sessions are known by where they fall in it ([`record::records_for_blob`]),
/// and one another copy wrote by what it holds: the same session is the
/// same JSON wherever it falls. Deletions are remembered by key beside it,
/// as older builds remember them, and read by them too.
#[cfg(any(feature = "web", test))]
pub(super) struct BlobHistory<S> {
    shelf: S,
    keys: BlobKeys,
    budget: usize,
    /// The history could not be read, nor set aside, when it was last
    /// read: nothing is written over it until a read says otherwise.
    held: Cell<bool>,
    /// What reads set aside, as they could not read it, not yet told of.
    set_aside: RefCell<Vec<SetAside>>,
    /// The name of each record this copy stored, by where it falls and by
    /// the hash of what the history holds of it: so one placed where it
    /// finished keeps its name when the history is read again, though
    /// counted from the history alone it falls at its stamp, and one whose
    /// score reads otherwise today keeps the name it was stored under.
    placed: RefCell<HashMap<u64, BTreeMap<u64, RecordId>>>,
    /// The key of every record seen, by id: what tells those deleted.
    seen_keys: RefCell<HashMap<RecordId, SessionKey>>,
    /// What the history and its deletions held when last looked at.
    seen: Cell<Option<u64>>,
}

#[cfg(any(feature = "web", test))]
impl<S: Aside> BlobHistory<S> {
    pub(super) fn new(shelf: S, keys: BlobKeys, budget: usize) -> Self {
        Self {
            shelf,
            keys,
            budget,
            held: Cell::new(false),
            set_aside: RefCell::new(Vec::new()),
            placed: RefCell::new(HashMap::new()),
            seen_keys: RefCell::new(HashMap::new()),
            seen: Cell::new(None),
        }
    }

    /// The whole history, by record. What it held is noted as seen only
    /// where it could be read: a history held is still to take up.
    pub(super) fn load(&self) -> HistoryLoad {
        let (sessions, looked) = self.current_looked();
        if !self.held.get() {
            self.seen.set(Some(looked));
        }
        HistoryLoad {
            sessions: sessions
                .into_iter()
                .map(|(record, session)| (record.id, session))
                .collect(),
            set_aside: self.take_set_aside(),
            complete: !self.held.get(),
            ..HistoryLoad::default()
        }
    }

    /// Make `change` to the history, and say what became of it.
    ///
    /// Deletions are remembered before the history is written without
    /// them, and forgotten again if it cannot be; a record the change takes
    /// out for another of its key is not remembered as deleted at all. A
    /// session deleted is not stored again unless the change undoes its
    /// deletion.
    ///
    /// What it leaves is noted as seen only where nothing changed since
    /// this copy last looked: the history it writes holds whatever another
    /// copy stored meanwhile, which is still to take up.
    ///
    /// The history is read out of the store once: what tells whether it
    /// changed is what the change is made to, and what it leaves is what
    /// was written, which reads back the same.
    ///
    /// Only what the change puts is written as this build writes a
    /// session. Every other session goes back as the history held it, byte
    /// for byte: a tab of an older build that shares the history, which
    /// knows a session by what it holds, finds each as it left it.
    pub(super) fn apply(&self, change: &HistoryChange) -> HistorySaved {
        let stored = self.shelf.get(self.keys.history);
        let looked = self.look_at(stored.as_ref().ok().and_then(Option::as_deref));
        // Each session beside its text as the history holds it, none for
        // one the change puts.
        let mut held: Vec<(Record, SessionResult, Option<&str>)> = self
            .current_from(&stored)
            .0
            .into_iter()
            .map(|(record, session, stored)| (record, session, Some(stored)))
            .collect();
        if self.held.get() {
            return HistorySaved::Held;
        }
        let mut deleted = self.deleted();
        let before = deleted.clone();
        let undone: HashSet<RecordId> = change.unforget.iter().copied().collect();
        for id in &change.unforget {
            let key = change
                .put
                .iter()
                .find(|record| record.id == *id)
                .and_then(|record| record::read_record(&record.json))
                .map(|session| SessionKey::of(&session))
                .or_else(|| self.seen_keys.borrow().get(id).cloned());
            if let Some(key) = key {
                deleted.retain(|gone| gone.key() != key);
            }
        }
        let gone: HashSet<SessionKey> = deleted.iter().map(Deleted::key).collect();
        for record in &change.put {
            // As it reads back: what the history will hold of it.
            let read = record::read_record(&record.json)
                .and_then(|session| Some((serde_json::to_string(&session).ok()?, session)));
            let Some((json, session)) = read else {
                return HistorySaved::Failed("a session to store could not be read.".into());
            };
            self.placed
                .borrow_mut()
                .entry(record::fnv64(json.as_bytes()))
                .or_default()
                .insert(record.id.order, record.id);
            let key = SessionKey::of(&session);
            self.seen_keys.borrow_mut().insert(record.id, key.clone());
            let kept_already = held.iter().any(|(kept, _, _)| kept.json == json);
            if kept_already || (gone.contains(&key) && !undone.contains(&record.id)) {
                continue;
            }
            let at = held.partition_point(|(kept, _, _)| kept.id < record.id);
            held.insert(
                at,
                (
                    Record {
                        id: record.id,
                        json,
                    },
                    session,
                    None,
                ),
            );
        }
        for (id, gone) in &change.forget {
            // By its name here, or else by what it holds, as another copy
            // named it.
            let at = held
                .iter()
                .position(|(kept, _, _)| kept.id == *id)
                .or_else(|| {
                    held.iter().position(|(kept, session, _)| {
                        SessionKey::of(session) == gone.key()
                            && (kept.id.nonce == id.nonce
                                || record::fnv64(kept.json.as_bytes()) == id.nonce)
                    })
                });
            if let Some(at) = at {
                held.remove(at);
            }
        }
        // A deletion is remembered by its session's key, so one taken out
        // for another of the same key, as a restore fills a session in and
        // its Undo puts it back as it was, is no deletion: remembered, it
        // would read as deleted with the session in its place, which every
        // tab would then let go for good should it go missing in passing.
        let live: HashSet<SessionKey> = held
            .iter()
            .map(|(_, session, _)| SessionKey::of(session))
            .collect();
        for (_, gone) in &change.forget {
            if !live.contains(&gone.key()) && !deleted.contains(gone) {
                deleted.push(gone.clone());
            }
        }
        // A deletion is remembered before the history goes without it, and
        // one undone is forgotten only once the session is back.
        let remembered = deleted.iter().any(|gone| !before.contains(gone));
        let forgotten = deleted != before && !remembered;
        if remembered {
            self.save_deleted(&deleted);
        }
        let ids: Vec<RecordId> = held.iter().map(|(record, _, _)| record.id).collect();
        let sessions: Vec<(&str, &SessionResult)> = held
            .iter()
            .map(|(record, session, stored)| (stored.unwrap_or(&record.json), session))
            .collect();
        let mut written = None;
        let saved = store_newest_that_fit(&sessions, self.budget, |raw| {
            let took = self.shelf.set(self.keys.history, raw);
            if took {
                written = Some(raw.to_owned());
            }
            took
        });
        if saved.kept().is_none() {
            if remembered {
                self.save_deleted(&before);
            }
            return saved;
        }
        if forgotten {
            self.save_deleted(&deleted);
        }
        if self.seen.get() == Some(looked) {
            self.seen.set(Some(self.look_at(written.as_deref())));
        }
        match saved {
            HistorySaved::Squeezed { kept, runs, .. } => HistorySaved::Squeezed {
                kept,
                runs,
                trimmed: ids[..ids.len().saturating_sub(kept)].to_vec(),
            },
            _ => HistorySaved::Stored {
                kept: change.put.len(),
            },
        }
    }

    /// Every record there is: none where the history cannot be read. What
    /// it held is noted as seen only once it is listed.
    pub(super) fn listing(&self) -> Option<Vec<RecordId>> {
        let (held, looked) = self.current_looked();
        if self.held.get() {
            return None;
        }
        self.seen.set(Some(looked));
        Some(held.into_iter().map(|(record, _)| record.id).collect())
    }

    /// Every record there is, as the history holds it, in order: what a
    /// backup holds. None where the history cannot be read.
    pub(super) fn records(&self) -> Option<Vec<Record>> {
        let held = self.current();
        (!self.held.get()).then(|| held.into_iter().map(|(record, _)| record).collect())
    }

    /// The records `ids`, each that is there.
    pub(super) fn read(&self, ids: &[RecordId]) -> Vec<Result<Record, RecordId>> {
        let held = self.current();
        ids.iter()
            .map(|id| {
                held.iter()
                    .find(|(record, _)| record.id == *id)
                    .map(|(record, _)| record.clone())
                    .ok_or(*id)
            })
            .collect()
    }

    /// Which of `ids` were deleted by hand: those whose key the deletions
    /// remember, each beside when it was deleted, the latest where the key
    /// is remembered more than once. One never seen here is taken as not
    /// deleted. None where the deletions cannot be read: that is no word
    /// that a record was not deleted.
    pub(super) fn deletions(&self, ids: &[RecordId]) -> Option<Deletions> {
        let raw = self.shelf.get(self.keys.deleted).ok()?;
        let mut gone: HashMap<SessionKey, u64> = HashMap::new();
        for deleted in deleted_of(raw.as_deref().map(str::as_bytes)) {
            let at = gone.entry(deleted.key()).or_insert(deleted.at);
            *at = (*at).max(deleted.at);
        }
        let keys = self.seen_keys.borrow();
        Some(
            ids.iter()
                .filter_map(|id| Some((*id, Some(*gone.get(keys.get(id)?)?))))
                .collect(),
        )
    }

    /// Which of `ids` were deleted by hand ([`Self::deletions`]): none,
    /// where that could not be looked for.
    #[cfg(test)]
    pub(super) fn forgotten(&self, ids: &[RecordId]) -> Vec<RecordId> {
        let found = self.deletions(ids).unwrap_or_default();
        found.into_iter().map(|(id, _)| id).collect()
    }

    /// Whether the history or its deletions changed since last looked at.
    /// Nothing is noted: a change is told of each time it is asked, until a
    /// look that lists the history takes it up ([`Self::listing`]), so one
    /// whose look could not list it is told of again.
    pub(super) fn changed(&self) -> bool {
        self.seen.get() != Some(self.look())
    }

    /// What reads set aside, not yet told of: taken.
    pub(super) fn take_set_aside(&self) -> Vec<SetAside> {
        std::mem::take(&mut self.set_aside.borrow_mut())
    }

    /// What the history and its deletions hold, as one hash.
    fn look(&self) -> u64 {
        let history = self.shelf.get(self.keys.history).ok().flatten();
        self.look_at(history.as_deref())
    }

    /// What the history and its deletions hold, as one hash, the history
    /// holding `history`, as read out of the store or written to it: none
    /// where there is none to read.
    fn look_at(&self, history: Option<&str>) -> u64 {
        let deleted = self.shelf.get(self.keys.deleted).ok().flatten();
        let (history, deleted) = (history.unwrap_or_default(), deleted.unwrap_or_default());
        record::fnv64_parts([history.as_bytes(), b"\n", deleted.as_bytes()])
    }

    /// The deletions remembered: none, where they cannot be read, which
    /// costs a session deleted staying, never one kept going.
    fn deleted(&self) -> Vec<Deleted> {
        let raw = self.shelf.get(self.keys.deleted).ok().flatten();
        deleted_of(raw.as_deref().map(str::as_bytes))
    }

    fn save_deleted(&self, deleted: &[Deleted]) {
        if let Ok(raw) = serde_json::to_string(deleted) {
            self.shelf.set(self.keys.deleted, &raw);
        }
    }

    /// Note `what` to tell of, if it was set aside now, and say whether it
    /// is kept aside, now or before.
    fn noted(&self, kept: Kept, what: SetAside) -> bool {
        if kept == Kept::Now {
            self.set_aside.borrow_mut().push(what);
        }
        kept != Kept::Not
    }

    /// The history as it is stored, each session by its record. What does
    /// not read is set aside first, as a browser's store always set it
    /// aside; where it cannot be, the history is held, and only what reads
    /// of it comes back.
    fn current(&self) -> Vec<(Record, SessionResult)> {
        let stored = self.shelf.get(self.keys.history);
        self.current_from(&stored)
            .0
            .into_iter()
            .map(|(record, session, _)| (record, session))
            .collect()
    }

    /// The history as [`Self::current`] reads it, beside what it and its
    /// deletions held as it was read ([`Self::look_at`]): read out of the
    /// store once, and looked at again only where reading it set anything
    /// aside, which may write it anew.
    fn current_looked(&self) -> (Vec<(Record, SessionResult)>, u64) {
        let stored = self.shelf.get(self.keys.history);
        let looked = self.look_at(stored.as_ref().ok().and_then(Option::as_deref));
        let (current, set_aside) = self.current_from(&stored);
        let current = current
            .into_iter()
            .map(|(record, session, _)| (record, session))
            .collect();
        (current, if set_aside { self.look() } else { looked })
    }

    /// The history as it is stored, `stored` being what the store gave of
    /// it, as [`Self::current`] reads it, each session beside its text
    /// there. What reads beside what is set aside stays as it was held.
    /// Beside it, whether anything was set aside: the history may have been
    /// written anew.
    fn current_from<'a>(
        &self,
        stored: &'a Result<Option<String>, ()>,
    ) -> (Vec<(Record, SessionResult, &'a str)>, bool) {
        self.held.set(false);
        let raw = match stored {
            Ok(Some(raw)) => raw,
            Ok(None) => return (Vec::new(), false),
            Err(()) => {
                self.held.set(true);
                return (Vec::new(), false);
            }
        };
        let (history, aside) = (self.keys.history, self.keys.aside);
        let Some(read) = read_history(raw) else {
            let kept = set_aside_from(&self.shelf, history, raw, aside, raw, None);
            if !self.noted(kept, SetAside::History { kept: None }) {
                self.held.set(true);
            }
            return (Vec::new(), true);
        };
        let unread = read.unread_json();
        if let Some(unread) = &unread {
            let rest = format!("[{}]", read.stored.join(","));
            let kept = set_aside_from(&self.shelf, history, raw, aside, unread, Some(&rest));
            let count = read.unread.len();
            if !self.noted(kept, SetAside::Sessions { count, kept: None }) {
                self.held.set(true);
            }
        }
        (self.named(read.sessions, read.stored), unread.is_some())
    }

    /// `sessions`, the history in its order, each by its record: where this
    /// copy placed it, or else at its stamp, or just after the one before;
    /// each beside its text there, `stored`, by its place.
    fn named<'a>(
        &self,
        sessions: Vec<SessionResult>,
        stored: Vec<&'a str>,
    ) -> Vec<(Record, SessionResult, &'a str)> {
        let placed = self.placed.borrow();
        let mut keys = self.seen_keys.borrow_mut();
        let mut last: Option<u64> = None;
        let mut named = Vec::with_capacity(sessions.len());
        for (session, stored) in sessions.into_iter().zip(stored) {
            let Ok(json) = serde_json::to_string(&session) else {
                continue;
            };
            let nonce = record::fnv64(json.as_bytes());
            let floor = last.map_or(0, |last| last.saturating_add(1));
            let id = placed
                .get(&nonce)
                .and_then(|names| names.range(floor..).next().map(|(_, id)| *id))
                .unwrap_or_else(|| RecordId {
                    order: session.timestamp.saturating_mul(1000).max(floor),
                    nonce,
                });
            last = Some(id.order);
            keys.insert(id, SessionKey::of(&session));
            named.push((Record { id, json }, session, stored));
        }
        named
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::tests::{as_an_older_build_wrote, at, session, stamps};
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
        /// How many times each key was read.
        reads: std::cell::RefCell<std::collections::BTreeMap<String, usize>>,
    }

    impl Shelf {
        fn with_room(room: usize) -> Self {
            let shelf = Self {
                held: Default::default(),
                room: std::cell::Cell::new(room),
                unreadable: Default::default(),
                reads: Default::default(),
            };
            shelf.set("history", "[]");
            shelf
        }

        /// How many times `key` was read since this was last asked.
        fn reads_of(&self, key: &str) -> usize {
            self.reads.borrow_mut().remove(key).unwrap_or(0)
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
            *self.reads.borrow_mut().entry(key.into()).or_default() += 1;
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
        let (saved, kept) = newest_that_fit(&as_written(&back), usize::MAX);
        assert_eq!(kept, 1);
        let saved: Vec<serde_json::Value> = serde_json::from_str(&saved).unwrap();
        assert_eq!(saved[0]["charSetMode"], "callsign");
        assert_eq!(saved[0]["alphabetFingerprint"], "contest:wpx");
        assert_eq!(saved[0]["contest"], written["contest"]);
    }

    /// Each of `sessions` beside its text as this build writes it.
    fn as_written(sessions: &[SessionResult]) -> Vec<(String, &SessionResult)> {
        sessions
            .iter()
            .map(|session| (serde_json::to_string(session).unwrap(), session))
            .collect()
    }

    #[test]
    fn only_the_newest_sessions_that_fit_are_kept() {
        let sessions: Vec<SessionResult> = (0..10)
            .map(|i| session(&format!("2026-01-{i:02}")))
            .collect();
        let all = as_written(&sessions);
        let one = all[0].0.len();
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
        let mut sessions: Vec<SessionResult> = (0..5)
            .map(|i| session(&format!("2026-01-{i:02}")))
            .collect();
        // The newest is far larger than the others, as a long run is.
        sessions[4].alphabet_fingerprint = "x".repeat(20_000);
        let all = as_written(&sessions);
        let newest = all[4].0.len() + 2;
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
                runs: false,
                trimmed: Vec::new(),
            }
        );
        let last = written.borrow().last().cloned().unwrap();
        assert_eq!(parse_sessions(&last).map(|s| s.len()), Some(1));
        assert!(written.borrow().iter().all(|raw| raw != "[]"));
        // No room even for that: nothing is written over what is there,
        // and the store is full.
        written.borrow_mut().clear();
        let saved = store_newest_that_fit(&all, usize::MAX, with_room(newest - 1));
        assert_eq!(saved, HistorySaved::Full);
        assert!(written.borrow().iter().all(|raw| raw != "[]"));
        // Room for everything: everything.
        let saved = store_newest_that_fit(&all, usize::MAX, |_| true);
        assert_eq!(saved, HistorySaved::Stored { kept: 5 });
        // A history every session was deleted from is stored empty.
        let none: [(&str, &SessionResult); 0] = [];
        let saved = store_newest_that_fit(&none, usize::MAX, |raw| raw == "[]");
        assert_eq!(saved, HistorySaved::Stored { kept: 0 });
    }

    /// A layout says whether this build may write to the history by
    /// `compatible` alone: a later layout this build may write to opens,
    /// as it was written, what it added carried along; one it would harm
    /// is only read, whatever else it holds; and one without `compatible`,
    /// or that is no layout at all, does not read. None stored is a history
    /// not moved yet.
    #[test]
    fn a_layout_says_whether_this_build_may_write() {
        let gate = |raw: &str| Gate::of(Some(raw.as_bytes()));
        assert_eq!(Gate::of(None), Gate::Unmoved);
        let Gate::Open(later) = gate(r#"{"layout":2,"compatible":1,"migratedAt":5,"shards":[1]}"#)
        else {
            panic!("a later layout this build may write to");
        };
        assert_eq!((later.layout, later.compatible), (2, 1));
        assert_eq!(later.migrated_at, 5);
        assert_eq!(later.legacy, None);
        assert!(later.imported_keys.is_empty());
        assert_eq!(later.unread.get("shards"), Some(&serde_json::json!([1])));
        let written: serde_json::Value = serde_json::from_slice(&later.written().unwrap()).unwrap();
        assert_eq!(written["layout"], 2);
        assert_eq!(written["shards"], serde_json::json!([1]));
        // Read by what it says of itself, whatever the rest.
        assert_eq!(gate(r#"{"layout":3,"compatible":2}"#), Gate::Newer);
        assert_eq!(
            gate(r#"{"compatible":7,"migratedAt":"later"}"#),
            Gate::Newer
        );
        for unread in [
            r#"{"layout":1,"migratedAt":5}"#,
            r#"{"layout":1,"compatible":"1","migratedAt":5}"#,
            r#"{"layout":1,"compatible":1}"#,
            "[1]",
            "null",
            "",
            "{ damaged",
        ] {
            assert_eq!(gate(unread), Gate::Unread, "{unread}");
        }
    }

    const DAY: u64 = 86_400_000;
    const YEAR: u64 = 365 * DAY;
    /// When the sessions of these tests finish, as a clock that is right
    /// says.
    const T: u64 = 1_790_000_000_000;

    /// Every record of `records` by its session's key, as the history here
    /// holds them.
    fn live_of(records: &[Record]) -> HashMap<SessionKey, Vec<RecordId>> {
        let mut live: HashMap<SessionKey, Vec<RecordId>> = HashMap::new();
        for held in records {
            let key = record::key_of(&held.json).expect("a session");
            live.entry(key).or_default().push(held.id);
        }
        live
    }

    /// When each session `change` stores finished.
    fn put_stamps(change: &HistoryChange) -> Vec<u64> {
        change
            .put
            .iter()
            .filter_map(|held| record::read_record(&held.json))
            .map(|session| session.timestamp)
            .collect()
    }

    /// The history kept whole as an older build wrote it the `n`th time.
    fn written(n: u64) -> Option<Fingerprint> {
        Some(Fingerprint { len: n, mtime: n })
    }

    /// The layout an earlier build wrote, as `raw` holds it.
    fn earlier(raw: serde_json::Value) -> Layout {
        let Gate::Open(layout) = Gate::of(Some(raw.to_string().as_bytes())) else {
            panic!("a layout this build writes to: {raw}");
        };
        layout
    }

    /// A move made with the clock ten years ahead leaves what an older build
    /// stores after it to be taken up: what it takes up is cut off at the
    /// newest session the move took, a day before it, and not at the clock.
    /// It was the clock, and nothing was taken up until it came round.
    #[test]
    fn a_session_an_older_build_stores_after_a_move_made_with_the_clock_ahead_is_taken_up() {
        let first = finished(T);
        let (records, layout) = moved(std::slice::from_ref(&first), written(1), &[], T + 10 * YEAR);
        let since = finished(T + DAY);
        let (change, next) = imported(
            &layout,
            written(2),
            &[first, since.clone()],
            &[],
            &live_of(&records),
            &HashSet::new(),
        )
        .expect("taken up");
        assert_eq!(put_stamps(&change), [since.timestamp]);
        assert_eq!(layout.taken_through, Some(T));
        assert_eq!(next.taken_through, Some(T), "as the move left it");
    }

    /// A layout an earlier build wrote, at a move made with the clock ahead,
    /// says nothing of what the move took through: the newest session it
    /// took up to then, at the clock or before it, stands for it, and is
    /// noted. What an older build stored since is taken up.
    #[test]
    fn a_layout_an_earlier_build_moved_with_the_clock_ahead_takes_up_what_an_older_build_stored_since()
     {
        let first = finished(T);
        let layout = earlier(serde_json::json!({
            "layout": 1,
            "compatible": 1,
            "migratedAt": T + 10 * YEAR,
            "importedKeys": [[T, first.date]],
            "deletedSeen": 0,
        }));
        assert_eq!(layout.taken_through, None);
        let since = finished(T + DAY);
        let records = record::records_for_blob(std::slice::from_ref(&first));
        let (change, next) = imported(
            &layout,
            written(2),
            &[first, since.clone()],
            &[],
            &live_of(&records),
            &HashSet::new(),
        )
        .expect("taken up");
        assert_eq!(put_stamps(&change), [since.timestamp]);
        assert_eq!(next.taken_through, Some(T));
    }

    /// A move that takes no session, made with the clock ahead, leaves what
    /// an older build stores after it to be taken up. Where it found no
    /// history kept whole, nothing it took can come back, and nothing is cut
    /// off. Where the history kept whole was empty, the newest session it
    /// says it deleted, at the clock or before it, stands for the newest the
    /// move took, so what an older copy left open since brings back from
    /// before it stays out. It was the clock, and nothing was taken up until
    /// it came round.
    #[test]
    fn a_session_an_older_build_stores_after_a_move_of_no_session_made_with_the_clock_ahead_is_taken_up()
     {
        let since = finished(T + DAY);
        let (_, layout) = moved(&[], None, &[], T + 10 * YEAR);
        let (change, next) = imported(
            &layout,
            written(2),
            std::slice::from_ref(&since),
            &[],
            &HashMap::new(),
            &HashSet::new(),
        )
        .expect("taken up");
        assert_eq!(put_stamps(&change), [since.timestamp], "none kept whole");
        assert_eq!(next.taken_through, Some(0));

        let gone = Deleted::of(&finished(T), T + 1);
        let (_, layout) = moved(&[], written(1), std::slice::from_ref(&gone), T + 10 * YEAR);
        assert_eq!(layout.taken_through, Some(T));
        let (change, _) = imported(
            &layout,
            written(2),
            &[finished(T - 2 * DAY), since.clone()],
            std::slice::from_ref(&gone),
            &HashMap::new(),
            &HashSet::new(),
        )
        .expect("taken up");
        assert_eq!(put_stamps(&change), [since.timestamp], "all deleted");
    }

    /// An empty history kept whole that remembers no deletion says nothing
    /// of what came before the move: the clock as it was made stands for
    /// the newest session it took, as it did.
    #[test]
    fn a_move_of_an_empty_history_kept_whole_takes_through_its_clock() {
        let (_, layout) = moved(&[], written(1), &[], T + 10 * YEAR);
        assert_eq!(layout.taken_through, Some(T + 10 * YEAR));
    }

    /// One session stamped far ahead of the clock as the move was made, by
    /// a clock set ahead once, holds back no session finished after the
    /// move: the move took through the newest it took at the clock.
    #[test]
    fn a_session_stamped_past_the_clock_at_the_move_holds_back_no_later_one() {
        let (first, far) = (finished(T), finished(T + 10 * YEAR));
        let (records, layout) = moved(
            &[first.clone(), far.clone()],
            written(1),
            &[],
            T + 3_600_000,
        );
        let since = finished(T + 2 * 3_600_000);
        let (change, _) = imported(
            &layout,
            written(2),
            &[first, far, since.clone()],
            &[],
            &live_of(&records),
            &HashSet::new(),
        )
        .expect("taken up");
        assert_eq!(put_stamps(&change), [since.timestamp]);
        assert_eq!(layout.taken_through, Some(T));
    }

    /// A deletion an older build makes after the move is made here, however
    /// its stamp falls: here, below one a clock set ahead stamped before the
    /// move. It was made only where stamped past every deletion taken
    /// before, so it never was. A deletion is made once: the older build
    /// writing its deletions again makes none of them again, of a session
    /// put back here since.
    #[test]
    fn a_deletion_an_older_build_makes_after_the_move_is_made_whatever_its_stamp() {
        let (kept, other) = (finished(T), finished(T + DAY));
        // Made before the move by a clock ten years ahead.
        let far = Deleted::of(&finished(T - DAY), T + 10 * YEAR);
        let (records, layout) = moved(
            &[kept.clone(), other.clone()],
            written(1),
            std::slice::from_ref(&far),
            T + 2 * DAY,
        );
        // The older build deletes the second, by a clock that is right.
        let gone = Deleted::of(&other, T + 3 * DAY);
        let deleted = [far, gone.clone()];
        let live = live_of(&records);
        let (change, next) = imported(
            &layout,
            written(2),
            std::slice::from_ref(&kept),
            &deleted,
            &live,
            &HashSet::new(),
        )
        .expect("taken up");
        assert_eq!(change.forget, [(records[1].id, gone)]);
        // Written again, with the same deletions: none made again.
        let (change, _) = imported(
            &next,
            written(3),
            std::slice::from_ref(&kept),
            &deleted,
            &live,
            &HashSet::new(),
        )
        .expect("taken up");
        assert!(change.forget.is_empty(), "made once: {:?}", change.forget);
    }

    /// A layout an earlier build wrote says nothing of which deletions were
    /// taken: those made no later than the latest it took are taken, as it
    /// took them, and one made after is made. Both are noted as taken.
    #[test]
    fn a_layout_an_earlier_build_wrote_takes_deletions_as_it_did_then() {
        let (first, second) = (finished(T), finished(T + DAY));
        let layout = earlier(serde_json::json!({
            "layout": 1,
            "compatible": 1,
            "migratedAt": T + 2 * DAY,
            "importedKeys": [[first.timestamp, first.date], [second.timestamp, second.date]],
            "deletedSeen": T + 5,
        }));
        let records = record::records_for_blob(&[first.clone(), second.clone()]);
        let (taken, made) = (Deleted::of(&first, T + 5), Deleted::of(&second, T + 6));
        let (change, next) = imported(
            &layout,
            written(2),
            &[],
            &[taken.clone(), made.clone()],
            &live_of(&records),
            &HashSet::new(),
        )
        .expect("taken up");
        assert_eq!(change.forget, [(records[1].id, made.clone())]);
        let noted = next.deletions_taken.expect("noted");
        assert_eq!(
            noted.taken,
            [DeletionSeen::of(&taken), DeletionSeen::of(&made)].into()
        );
        assert_eq!(noted.seen, next.deleted_seen);
        assert_eq!(next.deleted_seen, T + 6);
    }

    /// A build that takes deletions up by when they were made alone, as
    /// builds before this one do, may take some up between two of this
    /// build's take-ups, and leave the deletions this one noted as it found
    /// them: each it took, made after what this build had seen and no later
    /// than the latest it took, as such a build takes them, is taken, and
    /// none is made again, of a session put back here since.
    #[test]
    fn deletions_an_earlier_build_took_up_since_are_not_made_again() {
        let (first, second) = (finished(T), finished(T + DAY));
        let records = record::records_for_blob(&[first.clone(), second.clone()]);
        let noted = Deleted::of(&first, T + 5);
        let since = Deleted::of(&second, T + 7);
        let layout = earlier(serde_json::json!({
            "layout": 1,
            "compatible": 1,
            "migratedAt": T + 2 * DAY,
            "importedKeys": [[first.timestamp, first.date], [second.timestamp, second.date]],
            // Moved on by an earlier build, which took the second's deletion.
            "deletedSeen": T + 9,
            "deletionsTaken": {"seen": T + 5, "taken": [[first.timestamp, first.date, T + 5]]},
        }));
        let (change, next) = imported(
            &layout,
            written(2),
            &[],
            &[noted, since.clone()],
            &live_of(&records),
            &HashSet::new(),
        )
        .expect("taken up");
        assert!(change.forget.is_empty(), "{:?}", change.forget);
        let taken = next.deletions_taken.expect("noted").taken;
        assert!(taken.contains(&DeletionSeen::of(&since)));
    }

    /// The layout as a build before this one's take-up rules writes it back
    /// once it has taken up the history kept whole as `fingerprint` by its
    /// own (0.16.3 to 0.16.9): `legacy` moved on, and `deletedSeen` to the
    /// latest of `deleted`; what it does not know, this build's notes among
    /// it, carried as it was read.
    fn taken_up_by_an_older_build(
        layout: &Layout,
        fingerprint: Option<Fingerprint>,
        deleted: &[Deleted],
    ) -> Layout {
        let mut raw: serde_json::Value =
            serde_json::from_slice(&layout.written().expect("written")).expect("JSON");
        raw["legacy"] = serde_json::to_value(fingerprint).expect("JSON");
        let seen = layout.deleted_seen.max(latest(deleted));
        raw["deletedSeen"] = serde_json::json!(seen);
        earlier(raw)
    }

    /// A change to the history kept whole that a build before this one's
    /// take-up rules took up first, by its own, is taken up again here by
    /// this build's: a session an older build stored after a move made long
    /// after the sessions it took is taken up, though that build's cut-off,
    /// a day before the move, left it out; and a deletion an older build
    /// made with its clock behind, stamped below the latest it took, is
    /// made, though that build makes only those stamped after it. The
    /// change used to read as taken up already, its `legacy` that build's
    /// note of it, and neither was ever taken up here.
    #[test]
    fn a_change_a_build_before_this_ones_rules_took_up_first_is_taken_up_again_here() {
        let older = [finished(T), finished(T + DAY), finished(T + 2 * DAY)];
        let noted = Deleted::of(&finished(T - DAY), T);
        let (records, layout) = moved(
            &older,
            written(1),
            std::slice::from_ref(&noted),
            T + 10 * YEAR,
        );
        let since = finished(T + 3 * DAY);
        let mut whole = older.to_vec();
        whole.push(since.clone());
        let looked = taken_up_by_an_older_build(&layout, written(2), std::slice::from_ref(&noted));
        let (change, next) = imported(
            &looked,
            written(2),
            &whole,
            std::slice::from_ref(&noted),
            &live_of(&records),
            &HashSet::new(),
        )
        .expect("taken up here");
        assert_eq!(put_stamps(&change), [since.timestamp]);
        assert_eq!(next.legacy, written(2));
        assert!(
            imported(
                &next,
                written(2),
                &whole,
                std::slice::from_ref(&noted),
                &live_of(&records),
                &HashSet::new(),
            )
            .is_none(),
            "once"
        );

        // A deletion stamped behind what this build had seen, which the
        // older build, taking up a later one too, passes over.
        let behind = Deleted::of(&older[2], T - 5);
        let later = Deleted::of(&older[1], T + 9);
        let deleted = [noted, behind.clone(), later.clone()];
        let looked = taken_up_by_an_older_build(&layout, written(3), &deleted);
        let mut live = live_of(&records);
        // The older build made the later deletion here as it took it up.
        live.remove(&later.key());
        let (change, next) = imported(
            &looked,
            written(3),
            &older[..1],
            &deleted,
            &live,
            &HashSet::new(),
        )
        .expect("taken up here");
        assert_eq!(change.forget, [(records[2].id, behind)]);
        assert_eq!(next.deletions_taken.expect("noted").taken.len(), 3);

        // Noted as none where there was no history kept whole, and read so.
        let (_, none) = moved(&[], None, &[], T);
        assert_eq!(none.taken_legacy, Some(None));
        let raw = none.written().expect("written");
        assert_eq!(Gate::of(Some(&raw)), Gate::Open(none));
    }

    /// What the take-up took is kept through every write of the layout: by
    /// this build, which writes it as it read it, and by an older one,
    /// which carries what it does not know ([`Layout::unread`]). A layout
    /// without it reads, as one an earlier build wrote.
    #[test]
    fn what_the_take_up_took_is_kept_in_the_layout() {
        let first = finished(T);
        let gone = Deleted::of(&finished(T - DAY), T + 1);
        let (_, layout) = moved(
            std::slice::from_ref(&first),
            written(1),
            std::slice::from_ref(&gone),
            T + 2,
        );
        let raw = layout.written().expect("written");
        let held: serde_json::Value = serde_json::from_slice(&raw).expect("JSON");
        assert_eq!(held["takenThrough"], serde_json::json!(T));
        assert_eq!(
            held["deletionsTaken"],
            serde_json::json!({"seen": T + 1, "taken": [[gone.timestamp, gone.date, T + 1]]})
        );
        assert_eq!(Gate::of(Some(&raw)), Gate::Open(layout));
        let without = earlier(serde_json::json!({"layout": 1, "compatible": 1, "migratedAt": 5}));
        assert_eq!(without.taken_through, None);
        assert_eq!(without.deletions_taken, None);
        let raw = without.written().expect("written");
        let held: serde_json::Value = serde_json::from_slice(&raw).expect("JSON");
        assert!(held.get("takenThrough").is_none() && held.get("deletionsTaken").is_none());
    }

    /// A browser's history kept whole, by the record API, on `shelf`.
    fn whole_on(shelf: &Shelf) -> BlobHistory<&Shelf> {
        let keys = BlobKeys {
            history: "history",
            deleted: "deleted",
            aside: "aside",
        };
        BlobHistory::new(shelf, keys, usize::MAX)
    }

    /// A session finished at `at`, scored as this build scores it.
    fn finished(at: u64) -> SessionResult {
        record::scored(SessionResult {
            timestamp: at,
            ..session(&format!("day {at}"))
        })
    }

    fn next_after(session: &SessionResult, newest: Option<RecordId>) -> Record {
        Record::of(session, |json| {
            RecordId::next(session.timestamp, newest, json)
        })
        .unwrap()
    }

    /// The history kept whole, as the shelf holds it.
    fn kept_whole(shelf: &Shelf) -> Vec<u64> {
        stamps(&parse_sessions(&shelf.get("history").unwrap().unwrap()).unwrap())
    }

    /// A history kept whole takes changes by record: each session stored
    /// keeps the name it was stored under, a deletion is remembered beside
    /// the history as older builds remember it, and undone, the session is
    /// back where it was.
    #[test]
    fn a_history_kept_whole_takes_changes_by_record() {
        let shelf = Shelf::with_room(usize::MAX);
        let whole = whole_on(&shelf);
        let read = whole.load();
        assert!(read.complete && read.sessions.is_empty());
        let (first, second) = (finished(1_000), finished(2_000));
        let one = next_after(&first, None);
        let two = next_after(&second, Some(one.id));
        let change = HistoryChange {
            put: vec![one.clone(), two.clone()],
            ..HistoryChange::default()
        };
        assert_eq!(whole.apply(&change), HistorySaved::Stored { kept: 2 });
        assert_eq!(whole.listing(), Some(vec![one.id, two.id]));
        assert_eq!(whole.read(&[two.id]), [Ok(two.clone())]);
        assert_eq!(kept_whole(&shelf), [1_000, 2_000]);
        // Deleted: gone from the history, and remembered as older builds
        // remember it.
        let gone = Deleted::of(&first, 7);
        let forget = HistoryChange {
            forget: vec![(one.id, gone.clone())],
            ..HistoryChange::default()
        };
        assert_eq!(whole.apply(&forget), HistorySaved::Stored { kept: 0 });
        assert_eq!(kept_whole(&shelf), [2_000]);
        let remembered: Vec<Deleted> =
            serde_json::from_str(&shelf.get("deleted").unwrap().unwrap()).unwrap();
        assert_eq!(remembered, [gone]);
        assert_eq!(whole.forgotten(&[one.id, two.id]), [one.id]);
        assert_eq!(
            whole.deletions(&[one.id, two.id]),
            Some(vec![(one.id, Some(7))]),
            "when, as remembered"
        );
        // The deletions cannot be read: no word of them, rather than the
        // word that there are none.
        shelf.unreadable.set(true);
        assert_eq!(whole.deletions(&[one.id, two.id]), None);
        shelf.unreadable.set(false);
        // Stored again without undoing it: still deleted.
        whole.apply(&HistoryChange {
            put: vec![one.clone()],
            ..HistoryChange::default()
        });
        assert_eq!(kept_whole(&shelf), [2_000]);
        // Undone: back where it was, and no longer remembered.
        let undo = HistoryChange {
            put: vec![one.clone()],
            unforget: vec![one.id],
            ..HistoryChange::default()
        };
        assert_eq!(whole.apply(&undo), HistorySaved::Stored { kept: 1 });
        assert_eq!(whole.listing(), Some(vec![one.id, two.id]));
        assert_eq!(shelf.get("deleted").unwrap().as_deref(), Some("[]"));
        assert!(whole.forgotten(&[one.id]).is_empty());
    }

    /// A change another tab made to the history kept whole is still to
    /// take up after a look that could not list it, however often it is
    /// asked of, until a look that can takes it up. Asking noted it as
    /// seen, and so did a listing that found the history held, so the
    /// session the other tab stored never showed here until another change.
    #[test]
    fn a_change_is_still_to_take_up_after_a_listing_that_failed() {
        let shelf = Shelf::with_room(usize::MAX);
        let (here, there) = (whole_on(&shelf), whole_on(&shelf));
        here.load();
        there.load();
        let theirs = next_after(&finished(1_000), None);
        there.apply(&HistoryChange {
            put: vec![theirs.clone()],
            ..HistoryChange::default()
        });
        assert!(here.changed(), "theirs, to take up");
        assert!(here.changed(), "still, until it is");
        // Beside it, a session that does not read, and no room to set it
        // aside: the history is held, and listed by no look.
        let stored = shelf.get("history").unwrap().unwrap();
        let with_unread = stored.replacen('[', r#"[{"date":5},"#, 1);
        shelf.set("history", &with_unread);
        shelf.room.set(with_unread.len());
        assert_eq!(here.listing(), None);
        shelf.room.set(usize::MAX);
        assert!(here.changed(), "still to take up");
        assert_eq!(here.listing(), Some(vec![theirs.id]));
        assert!(!here.changed(), "taken up");
    }

    /// A change of this tab's own, made after another tab changed the
    /// history kept whole, leaves the other's change still to take up: the
    /// history it writes holds what the other stored, which this tab has
    /// yet to show. Noted as seen with its own, the other's session never
    /// showed here until another change.
    #[test]
    fn a_change_another_tab_made_first_is_taken_up_after_ones_own() {
        let shelf = Shelf::with_room(usize::MAX);
        let (here, there) = (whole_on(&shelf), whole_on(&shelf));
        here.load();
        there.load();
        let theirs = next_after(&finished(1_000), None);
        there.apply(&HistoryChange {
            put: vec![theirs.clone()],
            ..HistoryChange::default()
        });
        let ours = next_after(&finished(2_000), None);
        here.apply(&HistoryChange {
            put: vec![ours.clone()],
            ..HistoryChange::default()
        });
        assert!(here.changed(), "theirs, to take up");
        assert_eq!(here.listing(), Some(vec![theirs.id, ours.id]));
        assert!(!here.changed(), "taken up");
        let more = next_after(&finished(3_000), Some(ours.id));
        here.apply(&HistoryChange {
            put: vec![more],
            ..HistoryChange::default()
        });
        assert!(!here.changed(), "its own change");
        assert!(there.changed());
    }

    /// A change to the history kept whole reads it out of the store once,
    /// and notes what it writes as seen, deletions remembered beside it
    /// among them. It was read whole three times a change: to tell whether
    /// another tab had changed it, to make the change to, and to note what
    /// was written as seen.
    #[test]
    fn a_change_to_a_history_kept_whole_reads_it_once() {
        let shelf = Shelf::with_room(usize::MAX);
        let whole = whole_on(&shelf);
        whole.load();
        let first = finished(1_000);
        let one = next_after(&first, None);
        let two = next_after(&finished(2_000), Some(one.id));
        let changes = [
            HistoryChange {
                put: vec![one.clone()],
                ..HistoryChange::default()
            },
            HistoryChange {
                put: vec![two],
                ..HistoryChange::default()
            },
            HistoryChange {
                forget: vec![(one.id, Deleted::of(&first, 7))],
                ..HistoryChange::default()
            },
        ];
        for change in &changes {
            shelf.reads_of("history");
            assert!(whole.apply(change).kept().is_some(), "{change:?}");
            assert_eq!(shelf.reads_of("history"), 1, "{change:?}");
            assert!(!whole.changed(), "its own change, seen: {change:?}");
        }
        assert_eq!(kept_whole(&shelf), [2_000]);
    }

    /// The history kept whole is read out of the store once to load it,
    /// and once to list it, and what is noted as seen is hashed from what
    /// was read, with no copy of it made. It was read whole twice each
    /// time, as much as the browser's room for the site, and copied again
    /// to be hashed. A history whose reading set something aside, which may
    /// write it anew, is looked at once more, as it was left.
    #[test]
    fn a_history_kept_whole_is_read_once_to_load_or_to_list() {
        let shelf = Shelf::with_room(usize::MAX);
        let whole = whole_on(&shelf);
        let put = HistoryChange {
            put: vec![next_after(&finished(1_000), None)],
            ..HistoryChange::default()
        };
        assert!(whole.apply(&put).kept().is_some());
        shelf.reads_of("history");
        assert_eq!(whole.load().sessions.len(), 1);
        assert_eq!(shelf.reads_of("history"), 1, "read once to load");
        assert!(!whole.changed(), "seen as it was read");
        shelf.reads_of("history");
        assert_eq!(whole.listing().map(|ids| ids.len()), Some(1));
        assert_eq!(shelf.reads_of("history"), 1, "read once to list");
        assert!(!whole.changed(), "seen as it was read");

        // A history no build reads, set aside and taken out of its key.
        shelf.set("history", "{not a list");
        assert!(whole.changed(), "another tab's");
        shelf.reads_of("history");
        let read = whole.load();
        assert_eq!((read.sessions.len(), read.set_aside.len()), (0, 1));
        assert_eq!(shelf.reads_of("history"), 2, "looked at once more");
        assert!(!whole.changed(), "seen as it was left");
        assert_eq!(shelf.get("history"), Ok(None));
    }

    /// A session stored where it finished, as a restore stores it, keeps
    /// its name here; another tab, which knows it by what it holds, still
    /// deletes it, and this one is told it was deleted.
    #[test]
    fn a_session_restored_into_a_history_kept_whole_keeps_its_name() {
        let shelf = Shelf::with_room(usize::MAX);
        let (here, there) = (whole_on(&shelf), whole_on(&shelf));
        here.load();
        there.load();
        let newer = next_after(&finished(9_000), None);
        here.apply(&HistoryChange {
            put: vec![newer.clone()],
            ..HistoryChange::default()
        });
        let restored = finished(5_000);
        let placed = Record::of(&restored, |json| RecordId::placed(5_000, json)).unwrap();
        here.apply(&HistoryChange {
            put: vec![placed.clone()],
            ..HistoryChange::default()
        });
        assert_eq!(here.listing(), Some(vec![placed.id, newer.id]));
        assert!(there.changed());
        let theirs = there.listing().expect("a listing");
        assert_eq!(theirs.len(), 2);
        let gone = Deleted::of(&restored, 7);
        there.apply(&HistoryChange {
            forget: vec![(theirs[0], gone)],
            ..HistoryChange::default()
        });
        assert!(here.changed());
        assert_eq!(here.listing(), Some(vec![newer.id]));
        assert_eq!(here.forgotten(&[placed.id]), [placed.id]);

        // One whose score reads otherwise today keeps its name too, and is
        // stored once however often it is stored.
        let unscored = SessionResult {
            timestamp: 7_000,
            score: 1.0,
            ..session("day 7000")
        };
        let stale = Record::of(&unscored, |json| RecordId::placed(7_000, json)).unwrap();
        let again = HistoryChange {
            put: vec![stale.clone()],
            ..HistoryChange::default()
        };
        here.apply(&again);
        here.apply(&again);
        assert_eq!(here.listing(), Some(vec![stale.id, newer.id]));
    }

    /// The deletions the history kept whole remembers, as older builds
    /// read them.
    fn remembered(shelf: &Shelf) -> Vec<Deleted> {
        shelf
            .get("deleted")
            .unwrap()
            .map(|raw| serde_json::from_str(&raw).unwrap())
            .unwrap_or_default()
    }

    /// A session a restore fills in is taken out for the one filled in,
    /// and that one for the first again as Undo puts it back: neither is
    /// remembered as deleted, as the deletions are remembered by key, and
    /// a session of that key is kept all along. Another tab takes the one
    /// in its place for kept, and stores it again should it go missing in
    /// passing. The one taken out used to be remembered as deleted by hand,
    /// so the session kept in its place read as deleted wherever it went
    /// missing, and was let go for good.
    #[test]
    fn a_session_filled_in_is_not_remembered_as_deleted() {
        let shelf = Shelf::with_room(usize::MAX);
        let (here, there) = (whole_on(&shelf), whole_on(&shelf));
        let mut now = crate::testing::stored_run("2026-10-02", &[("K5XYZ", "214", "K5XYZ", "214")]);
        now.timestamp = 2_000;
        let was = SessionResult {
            contest: None,
            ..now.clone()
        };
        here.load();
        let one = next_after(&was, None);
        here.apply(&HistoryChange {
            put: vec![one.clone()],
            ..HistoryChange::default()
        });
        there.load();
        let filled = Record::of(&now, |json| RecordId::beside(one.id, json)).unwrap();
        let fill = HistoryChange {
            put: vec![filled.clone()],
            forget: vec![(one.id, Deleted::of(&was, 7))],
            unforget: vec![filled.id],
            restore: true,
        };
        assert_eq!(here.apply(&fill), HistorySaved::Stored { kept: 1 });
        assert_eq!(here.listing(), Some(vec![filled.id]));
        assert_eq!(remembered(&shelf), [], "nothing deleted by hand");
        let theirs = there.listing().expect("a listing");
        assert_eq!(theirs.len(), 1);
        assert_eq!(there.forgotten(&theirs), [], "kept, for another tab too");
        // Lost in passing, and stored again by the other tab: taken.
        let again = there
            .read(&theirs)
            .into_iter()
            .map(Result::unwrap)
            .collect();
        shelf.set("history", "[]");
        let stored = there.apply(&HistoryChange {
            put: again,
            ..HistoryChange::default()
        });
        assert_eq!(stored, HistorySaved::Stored { kept: 1 });
        assert_eq!(here.listing(), Some(vec![filled.id]));

        // Undone: the session as it was is back in its place, and nothing
        // is remembered as deleted still.
        let undo = HistoryChange {
            put: vec![one.clone()],
            forget: vec![(filled.id, Deleted::of(&now, 8))],
            unforget: vec![one.id],
            ..HistoryChange::default()
        };
        assert_eq!(here.apply(&undo), HistorySaved::Stored { kept: 1 });
        assert_eq!(here.listing(), Some(vec![one.id]));
        assert_eq!(remembered(&shelf), []);
        assert_eq!(here.forgotten(&[one.id]), []);
        // Deleted by hand, it is remembered, as ever.
        let gone = Deleted::of(&was, 9);
        here.apply(&HistoryChange {
            forget: vec![(one.id, gone.clone())],
            ..HistoryChange::default()
        });
        assert_eq!(remembered(&shelf), [gone]);
        assert_eq!(here.forgotten(&[one.id]), [one.id]);
    }

    /// A history kept whole is kept to its room, the newest that fit, as it
    /// always was, and names the records it let go; a change it cannot
    /// store at all is full, and leaves the history and the deletions
    /// remembered as they were.
    #[test]
    fn a_history_kept_whole_keeps_to_its_room() {
        let sessions: Vec<SessionResult> = (1..=4).map(|n| finished(n * 1_000)).collect();
        let one = serde_json::to_string(&sessions[0]).unwrap().len();
        let shelf = Shelf::with_room(usize::MAX);
        let keys = BlobKeys {
            history: "history",
            deleted: "deleted",
            aside: "aside",
        };
        let whole = BlobHistory::new(&shelf, keys, 2 + 2 * one + 1);
        let mut newest = None;
        let records: Vec<Record> = sessions
            .iter()
            .map(|session| {
                let record = next_after(session, newest);
                newest = Some(record.id);
                record
            })
            .collect();
        let saved = whole.apply(&HistoryChange {
            put: records.clone(),
            ..HistoryChange::default()
        });
        assert_eq!(
            saved,
            HistorySaved::Squeezed {
                kept: 2,
                runs: false,
                trimmed: vec![records[0].id, records[1].id],
            }
        );
        assert_eq!(kept_whole(&shelf), [3_000, 4_000]);
        // No room even for the newest: nothing written, nothing remembered.
        let before = shelf.get("history").unwrap();
        shelf.room.set(one);
        let change = HistoryChange {
            put: vec![next_after(&finished(5_000), newest)],
            forget: vec![(records[2].id, Deleted::of(&sessions[2], 7))],
            ..HistoryChange::default()
        };
        assert_eq!(whole.apply(&change), HistorySaved::Full);
        assert_eq!(shelf.get("history").unwrap(), before);
        assert!(whole.forgotten(&[records[2].id]).is_empty());
    }

    /// A history kept whole that does not read is set aside before a
    /// change goes over it, as it always was, and told of once; one that
    /// cannot be set aside holds every change back.
    #[test]
    fn a_history_kept_whole_that_does_not_read_is_set_aside_first() {
        let shelf = Shelf::with_room(usize::MAX);
        shelf.set("history", "{ not a list");
        let whole = whole_on(&shelf);
        let read = whole.load();
        assert_eq!(read.set_aside, [SetAside::History { kept: None }]);
        assert_eq!(shelf.aside(), [("aside".into(), "{ not a list".into())]);
        let first = next_after(&finished(1_000), None);
        let change = HistoryChange {
            put: vec![first.clone()],
            ..HistoryChange::default()
        };
        assert_eq!(whole.apply(&change), HistorySaved::Stored { kept: 1 });
        assert_eq!(whole.listing(), Some(vec![first.id]));
        assert!(whole.take_set_aside().is_empty(), "told of once");

        let shelf = Shelf::with_room(usize::MAX);
        shelf.set("history", "{ not a list");
        shelf.unreadable.set(true);
        let whole = whole_on(&shelf);
        assert!(!whole.load().complete);
        assert_eq!(whole.listing(), None);
        assert_eq!(whole.apply(&change), HistorySaved::Held);
        shelf.unreadable.set(false);
        assert_eq!(
            shelf.get("history").unwrap().as_deref(),
            Some("{ not a list")
        );
    }

    /// What a tab of an older build writes back of `session`, which it
    /// read as this build wrote it: the format last, as a key it does not
    /// know and carries.
    fn as_an_older_build_writes_back(session: &SessionResult) -> String {
        let older = as_an_older_build_wrote(session);
        let open = older.strip_suffix('}').expect("an object");
        format!("{open},\"format\":{}}}", cw_core::SESSION_FORMAT)
    }

    /// Each session of the history kept whole, as the shelf holds it.
    fn held_as_stored(shelf: &Shelf) -> Vec<String> {
        let raw = shelf.get("history").unwrap().expect("a history");
        let held: Vec<&RawValue> = serde_json::from_str(&raw).unwrap();
        held.iter()
            .map(|session| session.get().to_string())
            .collect()
    }

    /// Every session of the history kept whole that a change leaves as it
    /// is goes back into it as the history held it, byte for byte, and so
    /// does every one that reads beside one set aside: a tab of an older
    /// build sharing the history reads each as it did. Each was written
    /// again as this build writes a session, saying its format first. An
    /// older tab (0.16.3 to 0.16.9) then read each under a new name, and
    /// as another session than the one it held, which it took for lost in
    /// passing and stored again beside it: the history doubled with every
    /// write here, until its room let the oldest go.
    #[test]
    fn a_history_kept_whole_writes_back_as_it_held_them_the_sessions_a_change_leaves() {
        let older = vec![
            as_an_older_build_wrote(&finished(1_000)),
            as_an_older_build_writes_back(&finished(2_000)),
        ];
        let shelf = Shelf::with_room(usize::MAX);
        shelf.set("history", &format!("[{}]", older.join(", ")));
        let whole = whole_on(&shelf);
        let read = whole.load();
        assert_eq!(read.sessions.len(), 2);
        let newest = read.sessions.last().map(|(id, _)| *id);
        let ours = next_after(&finished(3_000), newest);
        let change = HistoryChange {
            put: vec![ours.clone()],
            ..HistoryChange::default()
        };
        assert_eq!(whole.apply(&change), HistorySaved::Stored { kept: 1 });
        assert_eq!(
            held_as_stored(&shelf),
            [older[0].clone(), older[1].clone(), ours.json],
            "an older build's as it wrote them, this build's as it writes it"
        );
        assert_eq!(
            whole.listing().map(|ids| ids.len()),
            Some(3),
            "each once, here too"
        );

        // A session that does not read, in a history with no room to copy
        // it beside: the history gives up its room, holding the rest as it
        // held them.
        let unread = format!(r#"{{"date":"damaged","by":"{}"}}"#, "x".repeat(100));
        let raw = format!("[{},{unread},{}]", older[0], older[1]);
        let copy = format!(
            "[{}]",
            serde_json::from_str::<serde_json::Value>(&unread).unwrap()
        );
        let shelf = Shelf::with_room(raw.len() + copy.len() - 1);
        shelf.set("history", &raw);
        let read = whole_on(&shelf).load();
        assert_eq!(
            read.set_aside,
            [SetAside::Sessions {
                count: 1,
                kept: None
            }]
        );
        assert_eq!(shelf.aside(), shelved(&[("aside", copy.as_str())]));
        assert_eq!(held_as_stored(&shelf), older, "the rest, as they were held");
    }
}
