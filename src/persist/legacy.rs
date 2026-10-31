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
use std::collections::{HashMap, HashSet};

use cw_core::SessionResult;
use serde::Deserialize;

use super::record::{self, HistoryChange, RecordId};
#[cfg(any(feature = "web", test))]
use super::record::{HistoryLoad, Record};
use super::{Deleted, SessionKey};
#[cfg(any(feature = "web", test))]
use super::{HistorySaved, SetAside};

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
/// is full, and nothing is written ([`HistorySaved::Full`]): the app keeps
/// what it would have stored until there is room. Halving used to go on
/// below the newest session's size, down to an empty list, which a store
/// always takes; so a browser short of room wrote an empty history over the
/// whole of the one it held, and said it had kept the newest none.
///
/// What it lets go is said by how many it kept: the oldest of `sessions`
/// ([`BlobHistory::apply`] names them).
#[cfg(any(feature = "web", test))]
pub(super) fn store_newest_that_fit(
    sessions: &[SessionResult],
    budget: usize,
    mut set: impl FnMut(&str) -> bool,
) -> HistorySaved {
    let full = || HistorySaved::Full;
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

/// The layout of the history this build writes: records and tombstones
/// beside them ([`super::record`]).
#[cfg_attr(feature = "web", allow(dead_code))]
pub(super) const LAYOUT: u64 = 1;

/// The oldest layout a build has to know to write to a history: a later
/// build that keeps it so that this one would harm it says so by raising
/// it, and this one then only reads ([`Gate::Newer`]).
#[cfg_attr(feature = "web", allow(dead_code))]
pub(super) const COMPATIBLE: u64 = 1;

/// How long before the move a session may have finished and still be one
/// an older build stored after it: one finished a day before the move,
/// stored by a copy left open since, is still taken up.
#[cfg_attr(feature = "web", allow(dead_code))]
const MOVE_SLACK_MS: u64 = 86_400_000;

/// How the history is kept, and what the move from the history kept whole
/// took of it: `layout.json` beside the records, written last, so that a
/// move cut short is made again.
///
/// Whatever a later build adds to it is carried along as it was read.
#[cfg_attr(feature = "web", allow(dead_code))]
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
    /// When the latest deletion taken up from an older build was made: one
    /// made after it is made here too.
    #[serde(default)]
    pub(super) deleted_seen: u64,
    /// What a later build added.
    #[serde(flatten)]
    pub(super) unread: serde_json::Map<String, serde_json::Value>,
}

/// What tells one state of the history kept whole from another without
/// reading it: its length, and when it was last written (on a computer or
/// a phone, in nanoseconds since the epoch).
#[cfg_attr(feature = "web", allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Fingerprint {
    pub(super) len: u64,
    pub(super) mtime: u64,
}

/// Whether, and how, this build may write to a history.
#[cfg_attr(feature = "web", allow(dead_code))]
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

#[cfg_attr(feature = "web", allow(dead_code))]
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

#[cfg_attr(feature = "web", allow(dead_code))]
impl Layout {
    /// The layout as it is written: pretty, as a file somebody may open.
    pub(super) fn written(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self)
    }
}

/// The latest deletion `deleted` holds, as a watermark: none, as nought.
#[cfg_attr(feature = "web", allow(dead_code))]
fn latest(deleted: &[Deleted]) -> u64 {
    deleted.iter().map(|gone| gone.at).max().unwrap_or(0)
}

/// The move from a history kept whole: each of `sessions`, oldest first,
/// as a record ([`record::records_for_blob`]), and the layout to write once
/// every record is, `now`. `fingerprint` is the history kept whole as it
/// was read, and `deleted` the deletions it remembers, already done to it.
#[cfg_attr(feature = "web", allow(dead_code))]
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
        unread: serde_json::Map::new(),
    };
    (record::records_for_blob(sessions), layout)
}

/// What to take up of a history kept whole that an older build changed
/// since the move: `sessions`, what it holds now, as `fingerprint`, and
/// `deleted`, the deletions it remembers. `live` is every record here by
/// its session's key, and `gone` the key of every one deleted here.
///
/// A session is taken up once, where it finished after the move, less a
/// day, and is neither here already nor deleted here: only one an older
/// build finished since, or a copy of it left open stored since. One from
/// before, which an older copy brings back as it stores the history it
/// held, is never taken up again, nor one deleted here. Each goes where it
/// finished ([`RecordId::placed`]).
///
/// A deletion the older build made since is made here too, of every
/// record with its key, as it would have been there.
///
/// The change and the layout to write once it is made, or none where
/// there is nothing new.
#[cfg_attr(feature = "web", allow(dead_code))]
pub(super) fn imported(
    layout: &Layout,
    fingerprint: Option<Fingerprint>,
    sessions: &[SessionResult],
    deleted: &[Deleted],
    live: &HashMap<SessionKey, Vec<RecordId>>,
    gone: &HashSet<SessionKey>,
) -> Option<(HistoryChange, Layout)> {
    if fingerprint == layout.legacy {
        return None;
    }
    let taken: HashSet<&SessionKey> = layout.imported_keys.iter().collect();
    let since = layout.migrated_at.saturating_sub(MOVE_SLACK_MS);
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
    for gone in deleted.iter().filter(|gone| gone.at > layout.deleted_seen) {
        for id in live.get(&gone.key()).into_iter().flatten() {
            change.forget.push((*id, gone.clone()));
        }
    }
    let mut next = layout.clone();
    next.legacy = fingerprint;
    next.imported_keys.extend(keys);
    next.deleted_seen = layout.deleted_seen.max(latest(deleted));
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
/// ([`super::Store::change_history`]): how a browser keeps it until its
/// database holds it, and for good where the database will not open.
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

    /// The whole history, by record.
    pub(super) fn load(&self) -> HistoryLoad {
        let sessions = self.current();
        self.seen.set(Some(self.look()));
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
    pub(super) fn apply(&self, change: &HistoryChange) -> HistorySaved {
        let mut held = self.current();
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
            let kept_already = held.iter().any(|(kept, _)| kept.json == json);
            if kept_already || (gone.contains(&key) && !undone.contains(&record.id)) {
                continue;
            }
            let at = held.partition_point(|(kept, _)| kept.id < record.id);
            held.insert(
                at,
                (
                    Record {
                        id: record.id,
                        json,
                    },
                    session,
                ),
            );
        }
        for (id, gone) in &change.forget {
            // By its name here, or else by what it holds, as another copy
            // named it.
            let at = held
                .iter()
                .position(|(kept, _)| kept.id == *id)
                .or_else(|| {
                    held.iter().position(|(kept, session)| {
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
            .map(|(_, session)| SessionKey::of(session))
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
        let ids: Vec<RecordId> = held.iter().map(|(record, _)| record.id).collect();
        let sessions: Vec<SessionResult> = held.into_iter().map(|(_, session)| session).collect();
        let saved = store_newest_that_fit(&sessions, self.budget, |raw| {
            self.shelf.set(self.keys.history, raw)
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
        self.seen.set(Some(self.look()));
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

    /// Every record there is: none where the history cannot be read.
    pub(super) fn listing(&self) -> Option<Vec<RecordId>> {
        let held = self.current();
        self.seen.set(Some(self.look()));
        (!self.held.get()).then(|| held.into_iter().map(|(record, _)| record.id).collect())
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
    /// remember. One never seen here is taken as not deleted.
    pub(super) fn forgotten(&self, ids: &[RecordId]) -> Vec<RecordId> {
        let gone: HashSet<SessionKey> = self.deleted().iter().map(Deleted::key).collect();
        let keys = self.seen_keys.borrow();
        ids.iter()
            .copied()
            .filter(|id| keys.get(id).is_some_and(|key| gone.contains(key)))
            .collect()
    }

    /// Whether the history or its deletions changed since last looked at.
    pub(super) fn changed(&self) -> bool {
        let now = self.look();
        self.seen.replace(Some(now)) != Some(now)
    }

    /// What reads set aside, not yet told of: taken.
    pub(super) fn take_set_aside(&self) -> Vec<SetAside> {
        std::mem::take(&mut self.set_aside.borrow_mut())
    }

    /// What the history and its deletions hold, as one hash.
    fn look(&self) -> u64 {
        let held = |key| self.shelf.get(key).ok().flatten().unwrap_or_default();
        let both = format!("{}\n{}", held(self.keys.history), held(self.keys.deleted));
        record::fnv64(both.as_bytes())
    }

    /// The deletions remembered: none, where they cannot be read, which
    /// costs a session deleted staying, never one kept going.
    fn deleted(&self) -> Vec<Deleted> {
        self.shelf
            .get(self.keys.deleted)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
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
        self.held.set(false);
        let raw = match self.shelf.get(self.keys.history) {
            Ok(Some(raw)) => raw,
            Ok(None) => return Vec::new(),
            Err(()) => {
                self.held.set(true);
                return Vec::new();
            }
        };
        let (history, aside) = (self.keys.history, self.keys.aside);
        let Some(read) = read_history(&raw) else {
            let kept = set_aside_from(&self.shelf, history, &raw, aside, &raw, None);
            if !self.noted(kept, SetAside::History { kept: None }) {
                self.held.set(true);
            }
            return Vec::new();
        };
        if let Some(unread) = read.unread_json() {
            let kept = match serde_json::to_string(&read.sessions) {
                Ok(rest) => set_aside_from(&self.shelf, history, &raw, aside, &unread, Some(&rest)),
                Err(_) => Kept::Not,
            };
            let count = read.unread.len();
            if !self.noted(kept, SetAside::Sessions { count, kept: None }) {
                self.held.set(true);
            }
        }
        self.named(read.sessions)
    }

    /// `sessions`, the history in its order, each by its record: where this
    /// copy placed it, or else at its stamp, or just after the one before.
    fn named(&self, sessions: Vec<SessionResult>) -> Vec<(Record, SessionResult)> {
        let placed = self.placed.borrow();
        let mut keys = self.seen_keys.borrow_mut();
        let mut last: Option<u64> = None;
        let mut named = Vec::with_capacity(sessions.len());
        for session in sessions {
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
            named.push((Record { id, json }, session));
        }
        named
    }
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
        let saved = store_newest_that_fit(&[], usize::MAX, |raw| raw == "[]");
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
}
