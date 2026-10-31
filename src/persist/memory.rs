//! A store that keeps everything in memory, for tests: each test makes its
//! own, so nothing one saves reaches another, or the user's real data.
//!
//! Its history is kept by record, as a store's on disk is ([`super::record`]),
//! with what a test needs to make it misbehave as a real one can: full,
//! failing, unlisted, slow to answer, short of room as a browser's page
//! storage is, or losing a record from under the app. Beside it is the
//! history as an older build keeps it whole, which it moves and takes up as
//! the files do ([`super::legacy`]). Or else it keeps the history whole in
//! a browser's page storage, as a browser does where its database will not
//! open ([`MemoryStore::in_page`]).

use std::cell::{Cell, RefCell};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::task::{Poll, Waker};

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};

use super::backup::{BackupFile, NotSaved};
use super::legacy::{
    self, Aside, BlobHistory, BlobKeys, Fingerprint, Layout, parse_sessions, read_history,
};
use super::record::{
    HistoryChange, HistoryLoad, HistoryNotice, Pending, Record, RecordId, done, read_record,
    records_for_blob,
};
use super::{
    BackupSaving, CutOff, CutOffTaking, Deleted, HistorySaved, OpenSession, SessionKey, SetAside,
    Store, saved_as, saved_now,
};

#[derive(Default)]
pub struct MemoryStore {
    theme: RefCell<String>,
    tutorial_done: Cell<bool>,
    pub told_of_paddle_keys: Cell<bool>,
    pub asked_about_lock: Cell<bool>,
    settings: RefCell<Option<TrainingSettings>>,
    /// The history, record by record.
    records: RefCell<BTreeMap<RecordId, Held>>,
    /// The records deleted by hand: their tombstones.
    tombstones: RefCell<BTreeMap<RecordId, Deleted>>,
    /// A history has been stored, if only an empty one.
    stored: Cell<bool>,
    /// How the history is kept, once it has been moved.
    layout: RefCell<Option<Layout>>,
    /// The history as an older build keeps it whole, if it does
    /// ([`Self::keep_whole`]).
    legacy: RefCell<Option<String>>,
    /// How many times an older build has written it.
    legacy_writes: Cell<u64>,
    /// The deletions an older build remembers.
    pub legacy_deleted: RefCell<Vec<Deleted>>,
    /// What the move set aside of the history kept whole.
    pub legacy_aside: RefCell<Vec<String>>,
    /// The records this build could not read and has told of.
    told: RefCell<BTreeSet<RecordId>>,
    counters: RefCell<BTreeMap<CountersKey, AutoLevelCounters>>,
    /// How many times the settings were written, for a test that cares how
    /// often that happens.
    pub settings_saves: Cell<u32>,
    /// The settings cannot be written, for the reason given, the way a
    /// full disk refuses them.
    pub settings_refused: RefCell<Option<String>>,
    /// Room for only this many sessions, the way a browser's page storage
    /// has where it keeps the history whole: a change keeps the newest that
    /// fit, and names the oldest it let go ([`HistorySaved::Squeezed`]).
    pub page_room: Cell<Option<usize>>,
    /// The store has no room for any change to the history, the way a
    /// full disk or browser has none.
    pub full: Cell<bool>,
    /// Every change to the history fails, for the reason given.
    pub failing: RefCell<Option<String>>,
    /// The history cannot be listed or read.
    pub unlistable: Cell<bool>,
    /// Changes to the history are made only when the test says
    /// ([`Self::settle`]), the way a browser's database makes them a
    /// moment after they are asked for.
    pub deferred: Cell<bool>,
    /// The changes asked for and not yet made, in order.
    waiting: RefCell<VecDeque<(HistoryChange, Rc<RefCell<Answer>>)>>,
    /// How many records and tombstones have been written.
    pub record_writes: Cell<usize>,
    pub tombstone_writes: Cell<usize>,
    /// How many times the history has changed, and how many changes this
    /// copy had seen when it last looked.
    changes: Cell<u64>,
    seen_changes: Cell<u64>,
    /// What reads set aside, as they could not read it, not yet told of.
    pub set_aside: RefCell<Vec<SetAside>>,
    last_backup: Cell<Option<u64>>,
    /// Every backup file handed over: its name and what it held.
    pub backup_files: RefCell<Vec<(String, String)>>,
    /// A build that cannot hand over a backup file, the way a phone's cannot.
    pub no_backups: Cell<bool>,
    /// Backup files are handed over with no word of whether they were
    /// kept, the way a browser's downloads are.
    pub unconfirmed_backups: Cell<bool>,
    /// Backup files are kept inside the app, the way an iPhone keeps them,
    /// and moved out as this says.
    pub backups_inside: Cell<Option<&'static str>>,
    /// The history has grown past what Android's own backup of the app
    /// takes, the way a long one does there.
    pub device_backup_outgrown: Cell<bool>,
    /// How many times the store was asked to keep its data.
    pub keep_asks: Cell<u32>,
    /// The session or run marked open, as a copy of the app that stopped
    /// leaves it for the next.
    marked: RefCell<Option<OpenSession>>,
    /// Another copy of the app is running the session marked open.
    pub open_elsewhere: Cell<bool>,
    /// The history kept whole in a browser's page storage, through this
    /// copy's own adapter to it, as each tab has its own: none, where the
    /// history is kept by record here ([`Self::in_page`]).
    page: Option<BlobHistory<PageShelf>>,
    /// Another copy's word of each change it makes is heard, the way a
    /// browser's tabs tell each other ([`Self::tell_news`]): by a copy of
    /// the app made while this is set.
    pub hears_news: Cell<bool>,
    news: Rc<RefCell<News>>,
}

/// How many changes other copies have told of, and what waits on the next.
#[derive(Default)]
struct News {
    told: u64,
    waiting: Vec<Waker>,
}

/// A browser's page storage, as far as the history kept whole goes: what
/// every tab of a site shares ([`MemoryStore::in_page`]).
#[derive(Clone, Default)]
pub struct PageShelf {
    kept: Rc<RefCell<BTreeMap<String, String>>>,
    /// The browser will not let the site at its storage, as where the user
    /// blocks a site's data: every read and write of it fails.
    blocked: Rc<Cell<bool>>,
}

/// The keys a browser keeps the history whole under, as a browser's store
/// names them.
const PAGE_KEYS: BlobKeys = BlobKeys {
    history: "dust_sessions",
    deleted: "dust_deleted",
    aside: "dust_sessions_unreadable",
};

impl PageShelf {
    /// Keep `sessions` as the history, whole, over whatever it held.
    pub fn keep(&self, sessions: &[SessionResult]) {
        let raw = serde_json::to_string(sessions).expect("sessions write");
        self.put(PAGE_KEYS.history, raw);
    }

    /// The sessions the history kept whole holds, in its order.
    pub fn sessions(&self) -> Vec<SessionResult> {
        self.held(PAGE_KEYS.history)
            .and_then(|raw| parse_sessions(&raw))
            .unwrap_or_default()
    }

    /// The sessions remembered as deleted by hand, by their keys.
    pub fn deleted(&self) -> Vec<SessionKey> {
        self.held(PAGE_KEYS.deleted)
            .and_then(|raw| serde_json::from_str::<Vec<Deleted>>(&raw).ok())
            .unwrap_or_default()
            .iter()
            .map(Deleted::key)
            .collect()
    }

    /// Lose the sessions finished at `timestamp` from the history kept
    /// whole, the way a tab writing it whole from an older read loses what
    /// another stored since: nothing remembers them as deleted.
    pub fn lose(&self, timestamp: u64) {
        let raw = self.held(PAGE_KEYS.history).expect("a history");
        let mut sessions: Vec<serde_json::Value> = serde_json::from_str(&raw).expect("a list");
        sessions.retain(|session| session["timestamp"] != timestamp);
        self.put(
            PAGE_KEYS.history,
            serde_json::to_string(&sessions).expect("sessions write"),
        );
    }

    /// Let the site at the storage, or not, as the browser does
    /// ([`Self::blocked`]).
    pub fn block(&self, blocked: bool) {
        self.blocked.set(blocked);
    }

    /// What is kept under `key`, whether the site may reach it or not.
    fn held(&self, key: &str) -> Option<String> {
        self.kept.borrow().get(key).cloned()
    }

    /// Keep `raw` under `key`, whether the site may reach it or not.
    fn put(&self, key: &str, raw: String) {
        self.kept.borrow_mut().insert(key.into(), raw);
    }
}

impl Aside for PageShelf {
    fn keys(&self) -> Result<Vec<String>, ()> {
        if self.blocked.get() {
            return Err(());
        }
        Ok(self.kept.borrow().keys().cloned().collect())
    }

    fn get(&self, key: &str) -> Result<Option<String>, ()> {
        if self.blocked.get() {
            return Err(());
        }
        Ok(self.held(key))
    }

    fn set(&self, key: &str, raw: &str) -> bool {
        if self.blocked.get() {
            return false;
        }
        self.put(key, raw.into());
        true
    }

    fn remove(&self, key: &str) {
        if !self.blocked.get() {
            self.kept.borrow_mut().remove(key);
        }
    }
}

/// A record as this store holds it: what was written for it, and the
/// session as it went in, which comes back as it was: one a test stored
/// whole, or one the app stored as it wrote it.
struct Held {
    json: String,
    whole: Option<SessionResult>,
}

impl Held {
    /// The session the record holds, if it reads.
    fn session(&self) -> Option<SessionResult> {
        self.whole.clone().or_else(|| read_record(&self.json))
    }
}

/// What became of a change made later ([`MemoryStore::deferred`]), and what
/// waits on it.
#[derive(Default)]
struct Answer {
    saved: Option<HistorySaved>,
    waker: Option<Waker>,
}

impl Store for MemoryStore {
    fn load_theme(&self) -> String {
        self.theme.borrow().clone()
    }

    fn save_theme(&self, theme: &str) {
        *self.theme.borrow_mut() = theme.to_string();
    }

    fn tutorial_done(&self) -> bool {
        self.tutorial_done.get()
    }

    fn mark_tutorial_done(&self) {
        self.tutorial_done.set(true);
    }

    fn told_of_paddle_keys(&self) -> bool {
        self.told_of_paddle_keys.get()
    }

    fn mark_told_of_paddle_keys(&self) {
        self.told_of_paddle_keys.set(true);
    }

    fn asked_about_lock(&self) -> bool {
        self.asked_about_lock.get()
    }

    fn mark_asked_about_lock(&self) {
        self.asked_about_lock.set(true);
    }

    fn read_settings(&self) -> Option<TrainingSettings> {
        self.settings.borrow().clone().map(TrainingSettings::clamp)
    }

    fn write_settings(&self, settings: &TrainingSettings) -> Result<(), String> {
        if let Some(why) = self.settings_refused.borrow().clone() {
            return Err(why);
        }
        self.settings_saves.set(self.settings_saves.get() + 1);
        *self.settings.borrow_mut() = Some(settings.clone());
        Ok(())
    }

    fn take_set_aside(&self) -> Vec<SetAside> {
        let mut set_aside = std::mem::take(&mut *self.set_aside.borrow_mut());
        if let Some(page) = &self.page {
            set_aside.extend(page.take_set_aside());
        }
        set_aside
    }

    fn load_auto_counters(&self, key: &CountersKey) -> AutoLevelCounters {
        self.counters.borrow().get(key).copied().unwrap_or_default()
    }

    fn save_auto_counters(&self, key: &CountersKey, counters: AutoLevelCounters) {
        self.counters.borrow_mut().insert(key.clone(), counters);
    }

    fn clear_auto_counters(&self, keys: &[CountersKey]) {
        let mut counters = self.counters.borrow_mut();
        for key in keys {
            counters.remove(key);
        }
    }

    fn last_backup(&self) -> Option<u64> {
        self.last_backup.get()
    }

    fn mark_backup(&self, at: u64) {
        self.last_backup.set(Some(at));
    }

    fn saves_backups(&self) -> bool {
        !self.no_backups.get()
    }

    /// Written whole, from every record held, as a browser writes one; or,
    /// where they cannot be listed, from what the app holds, once it has
    /// given all of it.
    fn save_backup(&self, name: &str, backup: BackupFile) -> BackupSaving {
        if self.no_backups.get() {
            return saved_now(Err(NotSaved::Failed(
                "this build has nowhere to put it".into(),
            )));
        }
        let tombstones = self.tombstones.borrow();
        let stored = match &self.page {
            Some(page) => page.records(),
            None if self.unlistable.get() => None,
            None => Some(
                self.records
                    .borrow()
                    .iter()
                    .filter(|(id, _)| !tombstones.contains_key(id))
                    .map(|(id, held)| Record {
                        id: *id,
                        json: held.json.clone(),
                    })
                    .collect(),
            ),
        };
        let (contents, backed) = match backup.whole(stored, |id| tombstones.contains_key(&id)) {
            Ok(whole) => whole,
            Err(not) => return saved_now(Err(not)),
        };
        self.backup_files
            .borrow_mut()
            .push((name.to_string(), contents));
        saved_now(Ok(saved_as(&format!("{name}, in your downloads"), backed)))
    }

    fn confirms_backups(&self) -> bool {
        !self.unconfirmed_backups.get()
    }

    fn backups_inside_app(&self) -> Option<&'static str> {
        self.backups_inside.get()
    }

    fn outgrows_device_backup(&self) -> bool {
        self.device_backup_outgrown.get()
    }

    fn ask_to_keep(&self) {
        self.keep_asks.set(self.keep_asks.get() + 1);
    }

    fn mark_open(&self, open: Option<OpenSession>) {
        *self.marked.borrow_mut() = open;
    }

    fn take_cut_off(&self) -> CutOffTaking {
        let cut = if self.open_elsewhere.get() {
            None
        } else {
            self.marked.take().map(CutOff::Marked)
        };
        Box::pin(std::future::ready(cut.into_iter().collect()))
    }

    /// The history by record, the move made first, as the files make it.
    fn load_history(&self) -> HistoryLoad {
        if let Some(page) = &self.page {
            return page.load();
        }
        let mut load = HistoryLoad::default();
        if self.unlistable.get() {
            return load;
        }
        let moved = self.layout.borrow().is_some();
        if !moved && let Err(why) = self.migrate(&mut load) {
            load.notices.push(HistoryNotice::MovePending(why));
        }
        self.import();
        let tombstones = self.tombstones.borrow();
        for (id, held) in self.records.borrow().iter() {
            if tombstones.contains_key(id) {
                continue;
            }
            match held.session() {
                Some(session) => load.sessions.push((*id, session)),
                None => load.unread.push(*id),
            }
        }
        let mut told = self.told.borrow_mut();
        let new = load.unread.iter().filter(|id| !told.contains(id)).count();
        if new > 0 {
            load.notices.push(HistoryNotice::Unread { count: new });
            told.extend(load.unread.iter().copied());
        }
        // Complete once the move is made: until then, the sessions kept
        // whole are stored, though not here.
        load.complete = self.layout.borrow().is_some();
        self.seen_changes.set(self.changes.get());
        load
    }

    /// Made at once, or, while changes are deferred, when the test settles
    /// them, in the order they were asked for.
    fn change_history(&self, change: HistoryChange) -> Pending<HistorySaved> {
        if let Some(page) = &self.page {
            return done(page.apply(&change));
        }
        if !self.deferred.get() {
            return done(self.make(&change));
        }
        let answer = Rc::new(RefCell::new(Answer::default()));
        self.waiting
            .borrow_mut()
            .push_back((change, answer.clone()));
        Box::pin(std::future::poll_fn(move |asked| {
            let mut answer = answer.borrow_mut();
            match answer.saved.take() {
                Some(saved) => Poll::Ready(saved),
                None => {
                    answer.waker = Some(asked.waker().clone());
                    Poll::Pending
                }
            }
        }))
    }

    fn history_listing(&self) -> Pending<Option<Vec<RecordId>>> {
        if let Some(page) = &self.page {
            return done(page.listing());
        }
        if self.unlistable.get() {
            return done(None);
        }
        self.import();
        self.seen_changes.set(self.changes.get());
        let tombstones = self.tombstones.borrow();
        done(Some(
            self.records
                .borrow()
                .keys()
                .filter(|id| !tombstones.contains_key(id))
                .copied()
                .collect(),
        ))
    }

    fn read_records(&self, ids: Vec<RecordId>) -> Pending<Vec<Result<Record, RecordId>>> {
        if let Some(page) = &self.page {
            return done(page.read(&ids));
        }
        let records = self.records.borrow();
        done(
            ids.into_iter()
                .map(|id| match records.get(&id) {
                    Some(held) if !self.unlistable.get() => Ok(Record {
                        id,
                        json: held.json.clone(),
                    }),
                    _ => Err(id),
                })
                .collect(),
        )
    }

    fn forgotten(&self, ids: Vec<RecordId>) -> Pending<Vec<RecordId>> {
        if let Some(page) = &self.page {
            return done(page.forgotten(&ids));
        }
        let tombstones = self.tombstones.borrow();
        done(
            ids.into_iter()
                .filter(|id| tombstones.contains_key(id))
                .collect(),
        )
    }

    /// Whether the history changed since this store last read or listed
    /// it. Tabs sharing one store share what it last saw, so a test of
    /// tabs syncs each by hand.
    fn history_changed(&self) -> bool {
        if let Some(page) = &self.page {
            return page.changed();
        }
        self.seen_changes.replace(self.changes.get()) != self.changes.get()
    }

    fn history_news(&self) -> Option<Pending<()>> {
        if !self.hears_news.get() {
            return None;
        }
        let news = self.news.clone();
        let told = news.borrow().told;
        Some(Box::pin(std::future::poll_fn(move |asked| {
            let mut news = news.borrow_mut();
            if news.told > told {
                return Poll::Ready(());
            }
            news.waiting.push(asked.waker().clone());
            Poll::Pending
        })))
    }
}

impl MemoryStore {
    /// Another copy of the app tells of a change it made to the history:
    /// every copy that hears of them takes it up ([`Self::hears_news`]).
    pub fn tell_news(&self) {
        let mut news = self.news.borrow_mut();
        news.told += 1;
        for waker in news.waiting.drain(..) {
            waker.wake();
        }
    }

    /// A copy of the app that keeps the history whole in `shelf`, a
    /// browser's page storage, which other copies may share, each through
    /// an adapter of its own, as tabs share a browser's
    /// ([`legacy::BlobHistory`]); with room for all of it.
    pub fn in_page(shelf: &PageShelf) -> Self {
        Self {
            page: Some(BlobHistory::new(shelf.clone(), PAGE_KEYS, usize::MAX)),
            ..Self::default()
        }
    }

    /// The session or run marked open, if any.
    pub fn open(&self) -> Option<OpenSession> {
        self.marked.borrow().clone()
    }

    /// Every session stored that reads, in the order of the history,
    /// deleted ones left out: what the next launch would show. A session
    /// comes back as it went in.
    pub fn load_sessions(&self) -> Vec<SessionResult> {
        let tombstones = self.tombstones.borrow();
        self.records
            .borrow()
            .iter()
            .filter(|(id, _)| !tombstones.contains_key(id))
            .filter_map(|(_, held)| held.session())
            .collect()
    }

    /// Store `sessions` in place of every record, each a record, in their
    /// order, as the move from a history kept whole names them: how a test
    /// sets up what a store holds.
    pub fn save_sessions(&self, sessions: &[SessionResult]) {
        *self.records.borrow_mut() = records_for_blob(sessions)
            .into_iter()
            .zip(sessions)
            .map(|(record, session)| {
                let held = Held {
                    json: record.json,
                    whole: Some(session.clone()),
                };
                (record.id, held)
            })
            .collect();
        self.stored.set(true);
        self.changed_here();
    }

    /// Make every change waiting ([`Self::deferred`]), in order, and wake
    /// what waits on each.
    pub fn settle(&self) {
        loop {
            let Some((change, answer)) = self.waiting.borrow_mut().pop_front() else {
                return;
            };
            let saved = self.make(&change);
            let mut answer = answer.borrow_mut();
            answer.saved = Some(saved);
            if let Some(waker) = answer.waker.take() {
                waker.wake();
            }
        }
    }

    /// How many changes are waiting to be made.
    pub fn waiting(&self) -> usize {
        self.waiting.borrow().len()
    }

    /// Lose the record `id` from under the app, the way another copy
    /// writing over the store from an older read could: no tombstone says
    /// it was deleted.
    pub fn lose(&self, id: RecordId) {
        self.records.borrow_mut().remove(&id);
        self.changed_here();
    }

    /// Store `raw` as a record of its own after every other, the way a
    /// newer build or a damaged disk leaves one this build cannot read.
    pub fn plant_unreadable(&self, raw: &str) -> RecordId {
        let mut records = self.records.borrow_mut();
        let newest = records.keys().next_back().copied();
        let id = RecordId::next(0, newest, raw);
        records.insert(
            id,
            Held {
                json: raw.to_string(),
                whole: None,
            },
        );
        self.stored.set(true);
        self.changed_here();
        id
    }

    /// The tombstone of the record `id`, if it was deleted.
    pub fn tombstone(&self, id: RecordId) -> Option<Deleted> {
        self.tombstones.borrow().get(&id).cloned()
    }

    /// What is stored for the record `id`, if it is.
    pub fn record(&self, id: RecordId) -> Option<String> {
        self.records.borrow().get(&id).map(|held| held.json.clone())
    }

    /// Keep `sessions` whole, as an older build writes its history, over
    /// whatever it held.
    pub fn keep_whole(&self, sessions: &[SessionResult]) {
        self.keep_whole_raw(&serde_json::to_string(sessions).expect("sessions write"));
    }

    /// Keep `raw` as the history an older build keeps whole, as it was
    /// written, whatever it holds.
    pub fn keep_whole_raw(&self, raw: &str) {
        *self.legacy.borrow_mut() = Some(raw.to_string());
        self.legacy_writes.set(self.legacy_writes.get() + 1);
    }

    /// The history an older build keeps whole, as it is.
    pub fn whole(&self) -> Option<String> {
        self.legacy.borrow().clone()
    }

    /// What tells the history kept whole as it stands from another state
    /// of it: how long it is, and how often it was written.
    fn fingerprint(&self) -> Option<Fingerprint> {
        self.legacy.borrow().as_ref().map(|raw| Fingerprint {
            len: raw.len() as u64,
            mtime: self.legacy_writes.get(),
        })
    }

    fn changed_here(&self) {
        self.changes.set(self.changes.get() + 1);
    }

    /// Make `change`, as the files make it.
    fn make(&self, change: &HistoryChange) -> HistorySaved {
        if let Some(refused) = self.refused() {
            return refused;
        }
        let mut tombstones = self.tombstones.borrow_mut();
        let mut records = self.records.borrow_mut();
        for id in &change.unforget {
            tombstones.remove(id);
        }
        let undone: HashSet<RecordId> = change.unforget.iter().copied().collect();
        for record in &change.put {
            if tombstones.contains_key(&record.id) && !undone.contains(&record.id) {
                continue;
            }
            let mut id = record.id;
            while records
                .get(&id)
                .is_some_and(|held| held.json != record.json)
            {
                id = id.bumped();
            }
            if let Entry::Vacant(place) = records.entry(id) {
                place.insert(Held {
                    json: record.json.clone(),
                    // As it was written, not scored again, as the files
                    // would read it: a test compares what went in.
                    whole: serde_json::from_str(&record.json).ok(),
                });
                self.record_writes.set(self.record_writes.get() + 1);
            }
        }
        for (id, deleted) in &change.forget {
            if !tombstones.contains_key(id) {
                tombstones.insert(*id, deleted.clone());
                self.tombstone_writes.set(self.tombstone_writes.get() + 1);
            }
            records.remove(id);
        }
        self.stored.set(true);
        self.changed_here();
        // Short of room, as a browser's page storage is: the oldest go, and
        // are named.
        let live: Vec<RecordId> = records
            .keys()
            .filter(|id| !tombstones.contains_key(id))
            .copied()
            .collect();
        if let Some(room) = self.page_room.get()
            && live.len() > room
        {
            let trimmed = live[..live.len() - room].to_vec();
            for id in &trimmed {
                records.remove(id);
            }
            let runs = records
                .values()
                .filter_map(Held::session)
                .any(|session| session.contest.is_some());
            return HistorySaved::Squeezed {
                kept: room,
                runs,
                trimmed,
            };
        }
        HistorySaved::Stored {
            kept: change.put.len(),
        }
    }

    /// Why the store takes no change to the history now, the way a full or
    /// failing disk takes none: nothing, where it takes them.
    fn refused(&self) -> Option<HistorySaved> {
        if let Some(why) = self.failing.borrow().clone() {
            return Some(HistorySaved::Failed(why));
        }
        self.full.get().then_some(HistorySaved::Full)
    }

    /// Move the history kept whole into records, as the files move it:
    /// what does not read set aside, the rest in its order, and the layout
    /// noted last. A store that takes no change makes no move, and says
    /// why: the history kept whole is not taken for moved, and the next
    /// read moves it.
    fn migrate(&self, load: &mut HistoryLoad) -> Result<(), String> {
        let raw = self.legacy.borrow().clone();
        let sessions = match raw.as_deref().map(read_history) {
            None => Vec::new(),
            Some(None) => {
                self.legacy_aside
                    .borrow_mut()
                    .extend(self.legacy.borrow_mut().take());
                load.set_aside.push(SetAside::History { kept: None });
                Vec::new()
            }
            Some(Some(read)) => {
                if let Some(unread) = read.unread_json()
                    && !self.legacy_aside.borrow().contains(&unread)
                {
                    self.legacy_aside.borrow_mut().push(unread);
                    let count = read.unread.len();
                    load.set_aside
                        .push(SetAside::Sessions { count, kept: None });
                }
                read.sessions
            }
        };
        let (moved, layout) = legacy::moved(
            &sessions,
            self.fingerprint(),
            &self.legacy_deleted.borrow(),
            crate::time::now_ms(),
        );
        // The layout, written last, needs the room a record does.
        let saved = if moved.is_empty() {
            self.refused().unwrap_or(HistorySaved::Stored { kept: 0 })
        } else {
            self.make(&HistoryChange {
                put: moved,
                ..HistoryChange::default()
            })
        };
        if saved.kept().is_none() {
            return Err(match saved {
                HistorySaved::Failed(why) => why,
                _ => "no room left".into(),
            });
        }
        *self.layout.borrow_mut() = Some(layout);
        Ok(())
    }

    /// Take up what an older build stored in, or deleted from, the history
    /// kept whole since it was last taken up.
    fn import(&self) {
        let Some(layout) = self.layout.borrow().clone() else {
            return;
        };
        let sessions = self
            .legacy
            .borrow()
            .as_deref()
            .and_then(read_history)
            .map(|read| read.sessions)
            .unwrap_or_default();
        let mut live: HashMap<SessionKey, Vec<RecordId>> = HashMap::new();
        {
            let tombstones = self.tombstones.borrow();
            for (id, held) in self.records.borrow().iter() {
                if let Some(session) = held.session().filter(|_| !tombstones.contains_key(id)) {
                    live.entry(SessionKey::of(&session)).or_default().push(*id);
                }
            }
        }
        let gone: HashSet<SessionKey> = self
            .tombstones
            .borrow()
            .values()
            .map(Deleted::key)
            .collect();
        let Some((change, next)) = legacy::imported(
            &layout,
            self.fingerprint(),
            &sessions,
            &self.legacy_deleted.borrow(),
            &live,
            &gone,
        ) else {
            return;
        };
        if change.is_empty() || self.make(&change).kept().is_some() {
            *self.layout.borrow_mut() = Some(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::tests::session;

    const DAY: u64 = 86_400_000;

    fn finished(at: u64) -> SessionResult {
        crate::persist::record::scored(SessionResult {
            timestamp: at,
            ..session(&format!("day {at}"))
        })
    }

    fn record(session: &SessionResult) -> Record {
        Record::of(session, |json| {
            RecordId::next(session.timestamp, None, json)
        })
        .expect("a record")
    }

    fn put(records: &[Record]) -> HistoryChange {
        HistoryChange {
            put: records.to_vec(),
            ..HistoryChange::default()
        }
    }

    /// What a change came to, when it came to anything yet.
    fn answer(pending: &mut Pending<HistorySaved>) -> Option<HistorySaved> {
        let mut asked = std::task::Context::from_waker(Waker::noop());
        match pending.as_mut().poll(&mut asked) {
            Poll::Ready(saved) => Some(saved),
            Poll::Pending => None,
        }
    }

    fn now<T>(mut pending: Pending<T>) -> T {
        let mut asked = std::task::Context::from_waker(Waker::noop());
        match pending.as_mut().poll(&mut asked) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("not done"),
        }
    }

    /// The store keeps the history by record, as the files do: one write a
    /// session, one a deletion, and a deletion undone puts the session
    /// back where it was.
    #[test]
    fn the_history_is_kept_record_by_record() {
        let store = MemoryStore::default();
        assert!(store.load_history().complete);
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        assert_eq!(
            now(store.change_history(put(&[two.clone(), one.clone()]))),
            HistorySaved::Stored { kept: 2 }
        );
        assert_eq!(store.record_writes.get(), 2);
        now(store.change_history(put(std::slice::from_ref(&one))));
        assert_eq!(store.record_writes.get(), 2, "the same record once");
        assert_eq!(now(store.history_listing()), Some(vec![one.id, two.id]));
        let forget = HistoryChange {
            forget: vec![(one.id, Deleted::of(&first, 7))],
            ..HistoryChange::default()
        };
        now(store.change_history(forget));
        assert_eq!(store.tombstone_writes.get(), 1);
        assert_eq!(store.record_writes.get(), 2, "and nothing else written");
        assert_eq!(now(store.history_listing()), Some(vec![two.id]));
        assert_eq!(now(store.forgotten(vec![one.id, two.id])), [one.id]);
        assert_eq!(store.tombstone(one.id), Some(Deleted::of(&first, 7)));
        let undo = HistoryChange {
            put: vec![one.clone()],
            unforget: vec![one.id],
            ..HistoryChange::default()
        };
        now(store.change_history(undo));
        assert_eq!(
            store.load_history().sessions,
            [(one.id, first), (two.id, second)]
        );
        assert_eq!(store.record(two.id), Some(two.json.clone()));
        assert_eq!(now(store.read_records(vec![two.id])), [Ok(two)]);
    }

    /// A store can be full, failing, unlisted, or lose a record from under
    /// the app, and write nothing it was asked to.
    #[test]
    fn a_store_misbehaves_as_a_test_asks() {
        let store = MemoryStore::default();
        store.load_history();
        let one = record(&finished(1_000));
        store.full.set(true);
        assert_eq!(
            now(store.change_history(put(std::slice::from_ref(&one)))),
            HistorySaved::Full
        );
        store.full.set(false);
        *store.failing.borrow_mut() = Some("disk gone".into());
        assert_eq!(
            now(store.change_history(put(std::slice::from_ref(&one)))),
            HistorySaved::Failed("disk gone".into())
        );
        *store.failing.borrow_mut() = None;
        assert_eq!(store.record_writes.get(), 0);
        now(store.change_history(put(std::slice::from_ref(&one))));
        store.unlistable.set(true);
        assert_eq!(now(store.history_listing()), None);
        assert_eq!(now(store.read_records(vec![one.id])), [Err(one.id)]);
        store.unlistable.set(false);
        assert!(store.history_changed());
        assert!(!store.history_changed(), "seen");
        store.lose(one.id);
        assert!(store.history_changed());
        assert_eq!(now(store.history_listing()), Some(Vec::new()));
        assert!(
            now(store.forgotten(vec![one.id])).is_empty(),
            "lost, not deleted"
        );
        let unread = store.plant_unreadable("{\"later\":1}");
        let read = store.load_history();
        assert_eq!(read.unread, [unread]);
        assert_eq!(read.notices, [HistoryNotice::Unread { count: 1 }]);
        assert!(store.load_history().notices.is_empty(), "told of once");
    }

    /// Changes made later are made in the order they were asked for, each
    /// only once the test settles them, and each answered then.
    #[test]
    fn deferred_changes_wait_to_be_settled() {
        let store = MemoryStore::default();
        store.load_history();
        store.deferred.set(true);
        let (one, two) = (record(&finished(1_000)), record(&finished(2_000)));
        let mut first = store.change_history(put(std::slice::from_ref(&one)));
        let mut second = store.change_history(put(std::slice::from_ref(&two)));
        assert_eq!(answer(&mut first), None);
        assert_eq!(store.waiting(), 2);
        assert_eq!(now(store.history_listing()), Some(Vec::new()), "not yet");
        store.full.set(true);
        store.settle();
        assert_eq!(answer(&mut first), Some(HistorySaved::Full));
        assert_eq!(answer(&mut second), Some(HistorySaved::Full));
        store.full.set(false);
        let mut third = store.change_history(put(&[one.clone(), two.clone()]));
        store.settle();
        assert_eq!(answer(&mut third), Some(HistorySaved::Stored { kept: 2 }));
        assert_eq!(store.waiting(), 0);
        assert_eq!(now(store.history_listing()), Some(vec![one.id, two.id]));
    }

    /// The history an older build kept whole is moved as the files move
    /// it, and what it stores and deletes after is taken up.
    #[test]
    fn a_history_kept_whole_is_moved_and_taken_up() {
        let store = MemoryStore::default();
        let now_ms = crate::time::now_ms();
        let (first, second) = (finished(now_ms - 9 * DAY), finished(now_ms - 8 * DAY));
        store.keep_whole(&[first.clone(), second.clone()]);
        let kept = store.whole();
        let read = store.load_history();
        assert_eq!(read.sessions.len(), 2);
        assert_eq!(store.whole(), kept, "left as it was");
        let third = finished(now_ms + 60_000);
        store.keep_whole(&[first.clone(), third.clone()]);
        *store.legacy_deleted.borrow_mut() = vec![Deleted::of(&second, now_ms)];
        let read = store.load_history();
        assert_eq!(
            read.sessions
                .iter()
                .map(|(_, session)| session.timestamp)
                .collect::<Vec<_>>(),
            [first.timestamp, third.timestamp]
        );
        // Unreadable, set aside, and not moved.
        let damaged = MemoryStore::default();
        damaged.keep_whole_raw("{ not a list");
        let read = damaged.load_history();
        assert!(read.sessions.is_empty());
        assert_eq!(read.set_aside, [SetAside::History { kept: None }]);
        assert_eq!(*damaged.legacy_aside.borrow(), ["{ not a list"]);
    }

    /// A store that takes no change, full or failing, makes no move, as the
    /// files make none: the read says the move waits, and is not complete,
    /// and the history kept whole is moved once the store takes it.
    #[test]
    fn a_move_the_store_cannot_make_waits_for_the_next_read() {
        let store = MemoryStore::default();
        let now_ms = crate::time::now_ms();
        let whole = [finished(now_ms - 9 * DAY), finished(now_ms - 8 * DAY)];
        store.keep_whole(&whole);
        store.full.set(true);
        let read = store.load_history();
        assert!(!read.complete);
        assert!(read.sessions.is_empty());
        assert_eq!(
            read.notices,
            [HistoryNotice::MovePending("no room left".into())]
        );
        assert!(store.layout.borrow().is_none());
        assert_eq!(store.record_writes.get(), 0);
        store.full.set(false);
        *store.failing.borrow_mut() = Some("disk gone".into());
        let read = store.load_history();
        assert!(!read.complete);
        assert_eq!(
            read.notices,
            [HistoryNotice::MovePending("disk gone".into())]
        );
        *store.failing.borrow_mut() = None;
        let read = store.load_history();
        assert!(read.complete && read.notices.is_empty());
        assert_eq!(
            read.sessions
                .into_iter()
                .map(|(_, session)| session)
                .collect::<Vec<_>>(),
            whole
        );
        assert!(store.layout.borrow().is_some());
        // With nothing kept whole, the layout alone waits for room.
        let fresh = MemoryStore::default();
        fresh.full.set(true);
        assert!(!fresh.load_history().complete);
        fresh.full.set(false);
        assert!(fresh.load_history().complete);
    }

    /// The history a test stores whole comes back as it went in, and so
    /// does a session the app stores, as it wrote it.
    #[test]
    fn a_history_stored_whole_comes_back_as_it_went_in() {
        let store = MemoryStore::default();
        assert!(store.load_sessions().is_empty());
        let mut stored = session("2026-10-01");
        stored.score = 123.0;
        store.save_sessions(std::slice::from_ref(&stored));
        assert_eq!(store.load_sessions(), [stored.clone()]);
        assert_eq!(store.load_history().sessions.len(), 1);
        let mut written = session("2026-10-02");
        written.score = 7.0;
        written.timestamp = 2_000;
        let newest = store.load_history().sessions.last().map(|(id, _)| *id);
        let record = Record::of(&written, |json| {
            RecordId::next(written.timestamp, newest, json)
        })
        .expect("a record");
        now(store.change_history(put(&[record])));
        assert_eq!(store.load_sessions(), [stored, written]);
        store.save_sessions(&[]);
        assert!(store.load_sessions().is_empty());
    }

    /// Short of room, as a browser's page storage is, the store keeps the
    /// newest that fit and names the oldest it let go.
    #[test]
    fn a_store_short_of_room_names_what_it_let_go() {
        let store = MemoryStore::default();
        store.load_history();
        store.page_room.set(Some(2));
        let records: Vec<Record> = (1..=3).map(|n| record(&finished(n * 1_000))).collect();
        assert_eq!(
            now(store.change_history(put(&records))),
            HistorySaved::Squeezed {
                kept: 2,
                runs: false,
                trimmed: vec![records[0].id],
            }
        );
        assert_eq!(
            now(store.history_listing()),
            Some(vec![records[1].id, records[2].id])
        );
        assert!(
            now(store.forgotten(vec![records[0].id])).is_empty(),
            "let go, not deleted"
        );
    }
}
