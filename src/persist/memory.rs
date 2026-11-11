//! A store that keeps everything in memory, for tests: each test makes its
//! own, so nothing one saves reaches another, or the user's real data.
//!
//! Its history is kept by record, as a store's on disk is ([`super::record`]),
//! by the rules every store keeps it by ([`super::record_store`]), with what
//! a test needs to make it misbehave as a real one can: full,
//! failing, unlisted, slow to answer, short of room as a browser's page
//! storage is, or losing a record from under the app. Beside it is the
//! history as an older build keeps it whole, which it moves and takes up as
//! the files do ([`super::legacy`]). Or else it keeps the history whole in
//! a browser's page storage, as a browser does where its database will not
//! open ([`MemoryStore::in_page`]).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::rc::Rc;
use std::task::{Poll, Waker};

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};

use super::backup::{BackupFile, NotSaved};
use super::legacy::{Aside, BlobHistory, BlobKeys, Fingerprint, Gate, Layout, parse_sessions};
use super::progress::{HistoryProgress, LoadProgress};
use super::record::{
    Deletions, HistoryChange, HistoryLoad, HistoryNotice, Pending, Record, RecordId, Unopened,
    done, failed_for_now, key_of, read_record, records_for_blob, unreadable,
};
use super::record_store::notes::Note;
use super::record_store::steps::{Steps, There, make_steps};
use super::record_store::{
    self, Backend, By, KeyCache, Laid, Legacy, LegacyLook, Listing, Made, Order, Raw, RecordRead,
    Refusal, Tombstone, TombstoneRead, TombstonesOf, Writes, at_once,
};
use super::{
    BackupSaving, CutOff, CutOffTaking, Deleted, HistorySaved, Lock, LockHeld, Locking, MarkId,
    OpenSession, SessionKey, SetAside, Store, TakenMark, saved_as, saved_now,
};

#[derive(Default)]
pub struct MemoryStore {
    theme: RefCell<String>,
    tutorial_done: Cell<bool>,
    pub told_of_paddle_keys: Cell<bool>,
    settings: RefCell<Option<TrainingSettings>>,
    /// The history, record by record.
    records: RefCell<BTreeMap<RecordId, Held>>,
    /// The records deleted by hand: their tombstones.
    tombstones: RefCell<BTreeMap<RecordId, Deleted>>,
    /// The key of each record read or stored so far, as the files and a
    /// browser's database keep it: the session a record held open for a
    /// moment holds, as it was last read, which a take-up goes by.
    keys: RefCell<HashMap<RecordId, SessionKey>>,
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
    /// The records this build told of as damaged, or as a newer Dust's,
    /// until each reads, or is gone, as the files note them in
    /// `damaged-told.json` ([`Note::DamagedTold`]): one damaged again after
    /// is told of again. As it is written, by name: none until it is.
    damaged_told: RefCell<Option<Vec<u8>>>,
    /// The records this build told of as ones that could not be read, a
    /// failure that outlasted the app's tries, until each reads, or is
    /// gone, as the files note them in `unread-told.json`
    /// ([`Note::UnreadTold`]): one that fails again after is told of again.
    unread_told: RefCell<Option<Vec<u8>>>,
    /// How many times the store was asked to tell of records still unread
    /// after every try ([`Store::still_unread`]).
    pub still_unread_asks: Cell<usize>,
    /// The records that could not be read at all as the history was last
    /// read, or at the app's last try, and have not read since, nor gone,
    /// as the files note them in `failing.json` ([`Note::Failing`]): one
    /// that still cannot be read at the next read is told of then. It keeps
    /// no note of what was told as builds before formats read it, as no
    /// such build reads this store.
    failing_noted: RefCell<Option<Vec<u8>>>,
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
    /// The records listed that cannot be read for now, the way a file
    /// another program holds open for a moment cannot: as the history is
    /// read, too, as the files read one that cannot be read at all
    /// ([`HistoryLoad::failed`]).
    pub held_open: RefCell<BTreeSet<RecordId>>,
    /// Changes to the history are made only when the test says
    /// ([`Self::settle`]), the way a browser's database makes them a
    /// moment after they are asked for.
    pub deferred: Cell<bool>,
    /// The changes asked for and not yet made, in order.
    waiting: RefCell<VecDeque<(HistoryChange, Rc<RefCell<Answer>>)>>,
    /// Which records were deleted by hand is answered only when the test
    /// says ([`Self::answer_forgotten`]), the way a store on a thread of
    /// its own answers a moment after it is asked.
    pub forgotten_later: Cell<bool>,
    /// What was asked of which records were deleted, and not yet answered.
    asked_forgotten: RefCell<Vec<Rc<RefCell<Forgotten>>>>,
    /// Which records were deleted by hand cannot be looked for, the way
    /// the tombstones of files cannot be where their folder will not open,
    /// or those of a browser's database where it will not answer.
    pub deletions_unknown: Cell<bool>,
    /// Records are read only when the test says ([`Self::answer_reads`]),
    /// the way a browser's database reads them a moment after it is asked.
    pub reads_later: Cell<bool>,
    /// The reads asked for and not yet answered.
    asked_reads: RefCell<Vec<Rc<RefCell<Reads>>>>,
    /// How many records and tombstones have been written.
    pub record_writes: Cell<usize>,
    pub tombstone_writes: Cell<usize>,
    /// How many times the history has changed; and how many changes this
    /// copy had seen when it last looked, beside the history kept whole as
    /// it stood then, as an older build's write to it is a change too.
    changes: Cell<u64>,
    seen: Cell<(u64, Option<Fingerprint>)>,
    /// What an older build changed could not be taken up at the last look,
    /// as a store that takes no change takes none: still to take up, as the
    /// files and a browser's database leave it, until a look takes it up.
    held_back: Cell<bool>,
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
    /// The mark a copy of the app took as cut off, held until that copy
    /// lets go of it ([`Store::release_cut_off`]).
    taken: RefCell<Option<OpenSession>>,
    /// A mark let go of as carried goes only when the test says
    /// ([`Self::answer_releases`]), the way a browser takes it away a
    /// moment after it is asked to, once its tab's own mark is in place.
    pub releases_later: Cell<bool>,
    /// The lettings go of a mark carried asked for and not yet made.
    asked_releases: RefCell<Vec<Rc<RefCell<Release>>>>,
    /// What became of the marks, in order ([`MarkEvent`]).
    pub mark_log: RefCell<Vec<MarkEvent>>,
    /// The history kept whole in a browser's page storage, through this
    /// copy's own adapter to it, as each tab has its own: none, where the
    /// history is kept by record here ([`Self::in_page`]).
    page: Option<BlobHistory<PageShelf>>,
    /// Another copy's word of each change it makes is heard, the way a
    /// browser's tabs tell each other ([`Self::tell_news`]): by a copy of
    /// the app made while this is set.
    pub hears_news: Cell<bool>,
    news: Rc<RefCell<News>>,
    /// Another copy's word of each change it makes to what sync keeps,
    /// heard where [`Self::hears_news`] is set ([`Self::tell_sync_news`]).
    sync_news: Rc<RefCell<News>>,
    /// What sync keeps here ([`Store::load_sync`]).
    pub sync: RefCell<Option<String>>,
    /// What sync keeps cannot be written, as a full disk refuses it.
    pub sync_refused: Cell<bool>,
    /// What sync keeps cannot be read just now, as a file another program
    /// holds open cannot, though it is there.
    pub sync_unreadable: Cell<bool>,
    /// What another copy of the app keeps for sync just as this one has
    /// looked whether the store holds nothing for it, as a copy turning
    /// sync on anew at that moment would: kept once, after that look.
    pub sync_kept_after_look: RefCell<Option<String>>,
    /// The locks held ([`Store::try_lock`]), by whichever copy of the app
    /// holds each: every copy shares the store, and so its locks.
    locks: Rc<RefCell<BTreeSet<Lock>>>,
    /// The store takes no lock, as a browser without Web Locks takes none.
    pub unlockable: Cell<bool>,
    /// How many times a lock was asked for ([`Store::try_lock`]), held or
    /// not, for a test that cares how often a copy asks.
    pub lock_tries: Cell<usize>,
    /// The store takes only the locks a browser takes, each a Web Lock of
    /// the site's ([`Lock::web_name`]), as a browser's tabs share them: none
    /// on what sync keeps.
    pub browser: Cell<bool>,
    /// The history is being read as the app opens, and is in only once the
    /// test says ([`Self::release_history`]): until then the store says how
    /// far the read has come ([`Self::progress`]), and nothing may wait on
    /// it.
    history_held: Cell<bool>,
    /// How far a read held has come, for the test to move on.
    pub progress: LoadProgress,
}

/// What became of the marks of a [`MemoryStore`], for a test to follow.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkEvent {
    /// A copy's mark written, carrying the sessions finished at these times
    /// ([`OpenSession::unstored`]).
    Marked(Vec<u64>),
    /// A copy's mark taken away.
    Unmarked,
    /// The marks a copy took let go of: so many as carried, so many as
    /// they were ([`Store::release_cut_off`]).
    Released { carried: usize, left: usize },
}

/// The name of the one mark a [`MemoryStore`] keeps.
const MARK: &str = "mark";

/// A lock a copy of the app holds on a [`MemoryStore`], let go of as this
/// is dropped.
struct Unlocks {
    locks: Rc<RefCell<BTreeSet<Lock>>>,
    lock: Lock,
}

impl Drop for Unlocks {
    fn drop(&mut self) {
        self.locks.borrow_mut().remove(&self.lock);
    }
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

/// Records asked for, read later ([`MemoryStore::reads_later`]), and what
/// waits on them.
struct Reads {
    ids: Vec<RecordId>,
    read: Option<Vec<Result<Record, Unopened>>>,
    waker: Option<Waker>,
}

/// What was asked of which records were deleted by hand, answered later
/// ([`MemoryStore::forgotten_later`]), and what waits on the answer.
struct Forgotten {
    ids: Vec<RecordId>,
    gone: Option<Option<Deletions>>,
    waker: Option<Waker>,
}

/// A letting go of a mark carried, made later
/// ([`MemoryStore::releases_later`]), and what waits on it.
#[derive(Default)]
struct Release {
    made: bool,
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

    fn every_auto_counter(&self) -> Vec<(CountersKey, AutoLevelCounters)> {
        self.counters
            .borrow()
            .iter()
            .map(|(key, counters)| (key.clone(), *counters))
            .collect()
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
    /// given all of it. One that cannot be read now, as one held open
    /// cannot be, is the app's to give, as the files and a browser's
    /// database ask it of the app.
    fn save_backup(&self, name: &str, backup: BackupFile) -> BackupSaving {
        if self.no_backups.get() {
            return saved_now(Err(NotSaved::Failed(
                "this build has nowhere to put it".into(),
            )));
        }
        let tombstones = self.tombstones.borrow();
        let deleted = |id: RecordId| tombstones.contains_key(&id);
        let written = match &self.page {
            Some(page) => backup.whole(page.records(), deleted),
            None if self.unlistable.get() => backup.whole(None, deleted),
            None => {
                let held_open = self.held_open.borrow();
                let stored: Vec<Result<Record, RecordId>> = self
                    .records
                    .borrow()
                    .iter()
                    .filter(|(id, _)| !tombstones.contains_key(id))
                    .map(|(id, held)| {
                        if held_open.contains(id) {
                            return Err(*id);
                        }
                        Ok(Record {
                            id: *id,
                            json: held.json.clone(),
                        })
                    })
                    .collect();
                let mut out = Vec::new();
                backup
                    .write(&mut out, stored, deleted)
                    .map(|backed| (String::from_utf8(out).unwrap_or_default(), backed))
            }
        };
        let (contents, backed) = match written {
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
        self.mark_log.borrow_mut().push(match &open {
            Some(open) => MarkEvent::Marked(
                open.unstored
                    .iter()
                    .map(|session| session.timestamp)
                    .collect(),
            ),
            None => MarkEvent::Unmarked,
        });
        *self.marked.borrow_mut() = open;
    }

    /// The one mark, as a copy that stopped left it, held by the copy that
    /// takes it: none while another copy runs it, or another took it.
    fn take_cut_off(&self) -> CutOffTaking {
        let free = !self.open_elsewhere.get() && self.taken.borrow().is_none();
        let taken = if free { self.marked.take() } else { None };
        if let Some(open) = &taken {
            self.taken.replace(Some(open.clone()));
        }
        let cut = taken.map(|open| TakenMark {
            id: MarkId::new(MARK),
            cut: CutOff::Marked(open),
        });
        Box::pin(std::future::ready(cut.into_iter().collect()))
    }

    /// One let go of as it was is the mark again, unless a copy has marked
    /// a session since; one carried goes, at once, or once the test says
    /// ([`Self::releases_later`]). A letting go that names no mark, a
    /// copy's that took none, lets go of none: the mark a copy took is held
    /// by it still.
    fn release_cut_off(&self, carried: Vec<MarkId>, left: Vec<MarkId>) -> Pending<()> {
        self.mark_log.borrow_mut().push(MarkEvent::Released {
            carried: carried.len(),
            left: left.len(),
        });
        let named = |ids: &[MarkId]| ids.iter().any(|id| id.name() == MARK);
        if named(&left) {
            if let Some(taken) = self.taken.take() {
                let mut marked = self.marked.borrow_mut();
                if marked.is_none() {
                    *marked = Some(taken);
                }
            }
        } else if named(&carried) {
            if self.releases_later.get() {
                let release = Rc::new(RefCell::new(Release::default()));
                self.asked_releases.borrow_mut().push(release.clone());
                return Box::pin(std::future::poll_fn(move |asked| {
                    let mut release = release.borrow_mut();
                    if release.made {
                        return Poll::Ready(());
                    }
                    release.waker = Some(asked.waker().clone());
                    Poll::Pending
                }));
            }
            self.taken.take();
        }
        done(())
    }

    fn history_progress(&self) -> Option<HistoryProgress> {
        self.history_held.get().then(|| self.progress.now())
    }

    /// The history by record, the move made first, as the files make it.
    fn load_history(&self) -> HistoryLoad {
        assert!(
            !self.history_held.get(),
            "the history was waited on while it was still being read"
        );
        if let Some(page) = &self.page {
            return page.load();
        }
        if self.unlistable.get() {
            return HistoryLoad::default();
        }
        // What was held back is tried again, and held back afresh where it
        // still cannot be taken up.
        self.held_back.set(false);
        at_once(record_store::load(&InMem(self)))
            .and_then(Result::ok)
            .unwrap_or_default()
    }

    /// Made at once, or, while changes are deferred, when the test settles
    /// them, in the order they were asked for.
    fn change_history(&self, change: HistoryChange) -> Pending<HistorySaved> {
        if let Some(page) = &self.page {
            return done(page.apply(&change));
        }
        if !self.deferred.get() {
            return done(self.apply(&change));
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
        self.held_back.set(false);
        done(at_once(record_store::listing(&InMem(self))).flatten())
    }

    /// Each that cannot be read could not be read just now: the memory
    /// store holds text alone, as a browser's does.
    fn read_records(&self, ids: Vec<RecordId>) -> Pending<Vec<Result<Record, Unopened>>> {
        if let Some(page) = &self.page {
            return done(failed_for_now(page.read(&ids)));
        }
        if !self.reads_later.get() {
            return done(self.read_now(ids));
        }
        let asked = Rc::new(RefCell::new(Reads {
            ids,
            read: None,
            waker: None,
        }));
        self.asked_reads.borrow_mut().push(asked.clone());
        Box::pin(std::future::poll_fn(move |waiting| {
            let mut asked = asked.borrow_mut();
            match asked.read.take() {
                Some(read) => Poll::Ready(read),
                None => {
                    asked.waker = Some(waiting.waker().clone());
                    Poll::Pending
                }
            }
        }))
    }

    fn deleted_sessions(&self, ids: Vec<RecordId>) -> Pending<Option<Vec<(RecordId, SessionKey)>>> {
        if let Some(page) = &self.page {
            return done(page.deleted_sessions(&ids));
        }
        if self.deletions_unknown.get() {
            return done(None);
        }
        done(at_once(record_store::deleted_sessions(&InMem(self), &ids)).flatten())
    }

    fn deleted_keys(&self, keys: Vec<SessionKey>) -> Pending<Option<Vec<SessionKey>>> {
        if let Some(page) = &self.page {
            return done(page.deleted_keys(&keys));
        }
        if self.deletions_unknown.get() {
            return done(None);
        }
        done(at_once(record_store::deleted_keys(&InMem(self), keys)).flatten())
    }

    fn deletions(&self, ids: Vec<RecordId>) -> Pending<Option<Deletions>> {
        if let Some(page) = &self.page {
            return done(page.deletions(&ids));
        }
        if !self.forgotten_later.get() {
            return done(self.tombstoned(ids));
        }
        let answer = Rc::new(RefCell::new(Forgotten {
            ids,
            gone: None,
            waker: None,
        }));
        self.asked_forgotten.borrow_mut().push(answer.clone());
        Box::pin(std::future::poll_fn(move |asked| {
            let mut answer = answer.borrow_mut();
            match answer.gone.take() {
                Some(gone) => Poll::Ready(gone),
                None => {
                    answer.waker = Some(asked.waker().clone());
                    Poll::Pending
                }
            }
        }))
    }

    /// As the files tell of them: once each, until each reads or is gone;
    /// noted as failing too, as this build's failures.
    fn still_unread(&self, ids: Vec<RecordId>) -> Pending<Vec<HistoryNotice>> {
        self.still_unread_asks.set(self.still_unread_asks.get() + 1);
        done(at_once(record_store::still_unread(&InMem(self), &ids)).unwrap_or_default())
    }

    fn load_sync(&self) -> Option<String> {
        if self.sync_unreadable.get() {
            return None;
        }
        self.sync.borrow().clone()
    }

    fn sync_cleared(&self) -> bool {
        let cleared = self.sync.borrow().is_none();
        if let Some(kept) = self.sync_kept_after_look.borrow_mut().take() {
            *self.sync.borrow_mut() = Some(kept);
        }
        cleared
    }

    fn save_sync(&self, state: Option<&str>) -> Result<(), String> {
        if self.sync_refused.get() {
            return Err("the disk is full".into());
        }
        *self.sync.borrow_mut() = state.map(str::to_string);
        Ok(())
    }

    /// Kept whole in a browser's page storage, as [`Self::in_page`] keeps
    /// it, the history does not sync.
    fn syncs_history(&self) -> bool {
        self.page.is_none()
    }

    /// Answered at once, as by a computer's or a phone's store.
    fn try_lock(&self, lock: Lock) -> Pending<Locking> {
        self.lock_tries.set(self.lock_tries.get() + 1);
        if self.unlockable.get() || self.browser.get() && lock.web_name().is_none() {
            return done(Locking::Unlockable);
        }
        done(self.locked(lock).map_or(Locking::Busy, Locking::Held))
    }

    /// Whether the history changed since this store last read or listed
    /// it, or the history an older build keeps whole beside it did. Tabs
    /// sharing one store share what it last saw, so a test of tabs syncs
    /// each by hand.
    ///
    /// Nothing is noted: a change is told of each time it is asked, until
    /// a read or a listing of the history takes it up, as the files and a
    /// browser's database tell of one, so one whose look could not list
    /// it is told of again, as is one an older build made that a look could
    /// not take up ([`Self::held_back`]).
    fn history_changed(&self) -> bool {
        if let Some(page) = &self.page {
            return page.changed();
        }
        self.held_back.get() || self.seen.get() != self.looking()
    }

    fn history_news(&self) -> Option<Pending<()>> {
        self.hears_news.get().then(|| next_news(&self.news))
    }

    fn sync_news(&self) -> Option<Pending<()>> {
        self.hears_news.get().then(|| next_news(&self.sync_news))
    }
}

/// The test store, as a change is made in it step by step, as the files
/// make one ([`make_steps`]): every step is durable as it is made.
struct InMemory<'a>(&'a MemoryStore);

impl Steps for InMemory<'_> {
    type Error = HistorySaved;

    fn tombstoned(&self, id: RecordId) -> bool {
        self.0.tombstones.borrow().contains_key(&id)
    }

    fn holds(&self, id: RecordId) -> bool {
        self.0.records.borrow().contains_key(&id)
    }

    fn there(&self, id: RecordId, json: &str) -> Result<There, HistorySaved> {
        Ok(match self.0.records.borrow().get(&id) {
            None => There::Free,
            Some(held) if held.json == json => There::Same,
            Some(_) => There::Other,
        })
    }

    fn add(&self, id: RecordId, json: &str) -> Result<(), HistorySaved> {
        let held = Held {
            json: json.to_string(),
            // As it was written, not scored again, as the files would read
            // it: a test compares what went in.
            whole: serde_json::from_str(json).ok(),
        };
        self.0.records.borrow_mut().insert(id, held);
        self.0.record_writes.set(self.0.record_writes.get() + 1);
        Ok(())
    }

    fn no_name_free(&self, id: RecordId) -> HistorySaved {
        HistorySaved::Failed(format!("no name was free for the session stored as {id}"))
    }

    fn unforget(&self, ids: &[RecordId]) -> Result<(), HistorySaved> {
        let mut tombstones = self.0.tombstones.borrow_mut();
        for id in ids {
            tombstones.remove(id);
        }
        Ok(())
    }

    fn records_durable(&self) -> Result<(), HistorySaved> {
        Ok(())
    }

    fn tombstone(&self, new: &[(RecordId, &str)]) -> Result<(), HistorySaved> {
        let mut tombstones = self.0.tombstones.borrow_mut();
        for (id, held) in new {
            tombstones.insert(*id, serde_json::from_str(held).expect("a tombstone reads"));
            self.0
                .tombstone_writes
                .set(self.0.tombstone_writes.get() + 1);
        }
        Ok(())
    }

    fn unlink(&self, ids: &[RecordId]) {
        let mut records = self.0.records.borrow_mut();
        for id in ids {
            records.remove(id);
        }
    }
}

/// The test store as the record store reads and writes it
/// ([`record_store`]), as the files are, every read and change made then and
/// there: its knobs, a page's history, deferred answers and counters, are
/// answered by [`MemoryStore`]'s own methods before the record store is
/// asked. Its records are text, its notes of what was told the files' but
/// the note builds before formats read, and the history an older build keeps
/// whole is beside them, read whole.
struct InMem<'a>(&'a MemoryStore);

/// Why the test store made no change: what became of it.
struct Refused(HistorySaved);

impl Refusal for Refused {
    fn saved(&self) -> HistorySaved {
        self.0.clone()
    }

    fn words(&self) -> String {
        match &self.0 {
            HistorySaved::Failed(why) => why.clone(),
            _ => "no room left".into(),
        }
    }
}

impl Backend for InMem<'_> {
    type Refused = Refused;
    type Look = (u64, Option<Fingerprint>);
    type Listed = ();
    const ORDER: Order = Order::MoveThenRead;
    const FALLS_BACK: bool = false;
    const NOTES_STILL_UNREAD: bool = true;

    fn keys(&self) -> &RefCell<KeyCache> {
        &self.0.keys
    }

    async fn gate(&self) -> Result<Gate, Refused> {
        Ok(match self.0.layout.borrow().as_ref() {
            Some(layout) => Gate::Open(Box::new(layout.clone())),
            None => Gate::Unmoved,
        })
    }

    async fn list(&self) -> Result<Listing<()>, Refused> {
        Ok(Listing {
            records: self.0.records.borrow().keys().copied().collect(),
            gone: self.0.tombstones.borrow().keys().copied().collect(),
            skipped: false,
            listed: (),
        })
    }

    /// Each as it went in, one held open not at all.
    async fn read_listed(&self, listing: &Listing<()>) -> Vec<(RecordId, RecordRead)> {
        let (records, held_open) = (self.0.records.borrow(), self.0.held_open.borrow());
        listing
            .records
            .difference(&listing.gone)
            .filter_map(|id| {
                let held = records.get(id)?;
                let read = (!held_open.contains(id))
                    .then(|| held.session().ok_or_else(|| unreadable(&held.json)));
                Some((*id, read))
            })
            .collect()
    }

    async fn read(&self, ids: &[RecordId], _listed: Option<&()>) -> Vec<Raw> {
        let (records, held_open) = (self.0.records.borrow(), self.0.held_open.borrow());
        ids.iter()
            .map(|id| match records.get(id) {
                Some(held) if !self.0.unlistable.get() && !held_open.contains(id) => {
                    Raw::Text(held.json.clone())
                }
                _ => Raw::Failed,
            })
            .collect()
    }

    async fn tombstones(&self) -> Result<Vec<Tombstone>, Refused> {
        Ok(self.0.tombstones_read())
    }

    async fn tombstones_of(&self, ids: &[RecordId]) -> Option<TombstonesOf> {
        Some(self.0.tombstones_of(ids))
    }

    async fn note(&self, note: Note) -> Option<Vec<u8>> {
        self.0.note(note)
    }

    async fn write_note(&self, note: Note, names: Option<&BTreeSet<String>>) {
        self.0.write_note(note, names);
    }

    /// Read whole, as it is: whether it changed is the take-up's to say.
    async fn legacy_look(&self, _layout: &Layout) -> LegacyLook {
        LegacyLook::Changed(Some(self.0.kept_whole()))
    }

    async fn legacy(&self) -> Result<Legacy, String> {
        Ok(self.0.kept_whole())
    }

    async fn legacy_deleted(&self) -> Vec<Deleted> {
        self.0.legacy_deleted.borrow().clone()
    }

    /// Taken away, and kept aside: what an older build keeps whole is gone.
    async fn set_aside_history(&self, _raw: &[u8]) -> (Option<SetAside>, bool) {
        let whole = self.0.legacy.borrow_mut().take();
        self.0.legacy_aside.borrow_mut().extend(whole);
        (Some(SetAside::History { kept: None }), true)
    }

    async fn set_aside_sessions(
        &self,
        unread: &str,
        count: usize,
    ) -> Result<Option<SetAside>, String> {
        let mut aside = self.0.legacy_aside.borrow_mut();
        if aside.iter().any(|kept| kept == unread) {
            return Ok(None);
        }
        aside.push(unread.to_string());
        Ok(Some(SetAside::Sessions { count, kept: None }))
    }

    /// None, while one here cannot be read for now, as one held open
    /// cannot, as the files' move waits on each it reads: it may hold a
    /// session the move would write again.
    async fn here(&self) -> Result<(BTreeSet<RecordId>, BTreeSet<RecordId>), Refused> {
        let records = self.0.records.borrow();
        let held_open = self.0.held_open.borrow();
        if records.keys().any(|id| held_open.contains(id)) {
            let why = "a record here could not be read just now".to_string();
            return Err(Refused(HistorySaved::Failed(why)));
        }
        let gone = self.0.tombstones.borrow().keys().copied().collect();
        Ok((records.keys().copied().collect(), gone))
    }

    async fn texts(&self, ids: &[RecordId]) -> Result<Vec<(RecordId, String)>, Refused> {
        let records = self.0.records.borrow();
        Ok(ids
            .iter()
            .filter_map(|id| Some((*id, records.get(id)?.json.clone())))
            .collect())
    }

    /// Step by step, as the files make it; where it takes something up,
    /// how the history is kept noted once it is made.
    async fn commit(&self, writes: &Writes<'_>, laid: Laid<'_>, by: By) -> Made<Refused> {
        let (placed, saved) = if by == By::Store && writes.change.is_empty() {
            (Vec::new(), HistorySaved::Stored { kept: 0 })
        } else {
            self.0.make(writes)
        };
        if saved.kept().is_none() {
            return Made {
                placed,
                saved: Err(Refused(saved)),
            };
        }
        if let Laid::Over(next) = laid {
            *self.0.layout.borrow_mut() = Some(next.clone());
        }
        Made {
            placed,
            saved: Ok(saved),
        }
    }

    /// As a change is made, its records' keys noted; how the history is
    /// kept noted last. One with nothing to store stores nothing, where the
    /// store would take no change.
    async fn write_move(
        &self,
        records: &[Record],
        layout: &Layout,
    ) -> Result<Vec<Option<RecordId>>, Refused> {
        let (placed, saved) = if records.is_empty() {
            let saved = self.0.refused().unwrap_or(HistorySaved::Stored { kept: 0 });
            (Vec::new(), saved)
        } else {
            let change = HistoryChange {
                put: records.to_vec(),
                ..HistoryChange::default()
            };
            let writes = Writes::of(&change)
                .map_err(|err| Refused(HistorySaved::Failed(err.to_string())))?;
            let (placed, saved) = self.0.make(&writes);
            let mut keys = self.0.keys.borrow_mut();
            for (record, id) in records.iter().zip(&placed) {
                if let Some(id) = id
                    && let Some(key) = key_of(&record.json)
                {
                    keys.insert(*id, key);
                }
            }
            (placed, saved)
        };
        if saved.kept().is_none() {
            return Err(Refused(saved));
        }
        *self.0.layout.borrow_mut() = Some(layout.clone());
        Ok(placed)
    }

    fn look(&self) -> (u64, Option<Fingerprint>) {
        self.0.looking()
    }

    fn saw(&self, look: (u64, Option<Fingerprint>)) {
        self.0.seen.set(look);
    }

    fn held_back(&self) {
        self.0.held_back.set(true);
    }

    /// Each session as it went in, as the test store hands it back, and the
    /// store as it is now taken for seen: what this read stored itself is
    /// no change for it to take up.
    fn loaded(&self, load: &mut HistoryLoad) {
        let records = self.0.records.borrow();
        // Short of room, a take-up lets the oldest go: they are no longer
        // here to read.
        load.sessions.retain(|(id, _)| records.contains_key(id));
        load.unread.retain(|id| records.contains_key(id));
        load.later.retain(|(id, _)| records.contains_key(id));
        load.failed.retain(|id| records.contains_key(id));
        for (id, session) in &mut load.sessions {
            if let Some(whole) = records.get(id).and_then(Held::session) {
                *session = whole;
            }
        }
        self.0.seen.set(self.0.looking());
    }
}

/// What a tombstone holds, as it is written.
fn written(deleted: &Deleted) -> Vec<u8> {
    serde_json::to_vec(deleted).expect("a tombstone writes")
}

/// A wait that ends at the next word of `news` told after it began.
fn next_news(news: &Rc<RefCell<News>>) -> Pending<()> {
    let news = news.clone();
    let told = news.borrow().told;
    Box::pin(std::future::poll_fn(move |asked| {
        let mut news = news.borrow_mut();
        if news.told > told {
            return Poll::Ready(());
        }
        news.waiting.push(asked.waker().clone());
        Poll::Pending
    }))
}

/// Tell `news` of a change: every wait on it ends.
fn tell(news: &RefCell<News>) {
    let mut news = news.borrow_mut();
    news.told += 1;
    for waker in news.waiting.drain(..) {
        waker.wake();
    }
}

impl MemoryStore {
    /// Another copy of the app holds `lock`, until what this hands back is
    /// dropped: as a copy mid-sync holds what syncs take.
    pub fn hold_lock(&self, lock: Lock) -> LockHeld {
        self.locked(lock)
            .unwrap_or_else(|| panic!("{lock:?} is held already"))
    }

    /// `lock`, now held, where no copy held it.
    fn locked(&self, lock: Lock) -> Option<LockHeld> {
        self.locks.borrow_mut().insert(lock).then(|| {
            LockHeld::new(Unlocks {
                locks: Rc::clone(&self.locks),
                lock,
            })
        })
    }

    /// Another copy of the app tells of a change it made to the history:
    /// every copy that hears of them takes it up ([`Self::hears_news`]).
    pub fn tell_news(&self) {
        tell(&self.news);
    }

    /// Another copy of the app tells of a change it made to what sync
    /// keeps, as a browser fires a `storage` event in every other tab of the
    /// site as one writes `localStorage`: every copy that hears of them
    /// takes it up ([`Self::hears_news`]).
    pub fn tell_sync_news(&self) {
        tell(&self.sync_news);
    }

    /// A copy of the app that keeps the history whole in `shelf`, a
    /// browser's page storage, which other copies may share, each through
    /// an adapter of its own, as tabs share a browser's
    /// ([`BlobHistory`]); with room for all of it.
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

    /// The mark a copy took as cut off, while it holds it.
    pub fn taken(&self) -> Option<OpenSession> {
        self.taken.borrow().clone()
    }

    /// Have the history read as the app opens take its time, as a long one
    /// does: in only once [`Self::release_history`] says so.
    pub fn hold_history(&self) {
        self.history_held.set(true);
    }

    /// The history held is in ([`Self::hold_history`]).
    pub fn release_history(&self) {
        self.history_held.set(false);
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
            let saved = self.apply(&change);
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

    /// Of `ids`, those deleted by hand, each beside when its tombstone says
    /// it was: no word at all, where they cannot be looked for
    /// ([`Self::deletions_unknown`]).
    fn tombstoned(&self, ids: Vec<RecordId>) -> Option<Deletions> {
        if self.deletions_unknown.get() {
            return None;
        }
        at_once(record_store::deletions(&InMem(self), &ids)).flatten()
    }

    /// Of `ids`, those deleted by hand, each beside what its tombstone
    /// holds, as a store reads it: what was written for it.
    fn tombstones_of(&self, ids: &[RecordId]) -> TombstonesOf {
        let tombstones = self.tombstones.borrow();
        ids.iter()
            .filter_map(|id| Some((*id, Some(written(tombstones.get(id)?)))))
            .collect()
    }

    /// Every tombstone, as a store reads them: each holds what was written
    /// for it.
    fn tombstones_read(&self) -> Vec<Tombstone> {
        self.tombstones
            .borrow()
            .iter()
            .map(|(id, deleted)| Tombstone {
                id: Some(*id),
                read: TombstoneRead::Bytes(written(deleted)),
            })
            .collect()
    }

    /// The records `ids`, as they are stored now: one that cannot be read
    /// comes back as its id. Each read fails no longer, and is told of no
    /// more, as the files note it: one that fails again as the history is
    /// next read is not told of then, and one that fails again after the
    /// app's tries, or is damaged, is told of again.
    fn read_now(&self, ids: Vec<RecordId>) -> Vec<Result<Record, Unopened>> {
        at_once(record_store::read(&InMem(self), &ids))
            .unwrap_or_else(|| ids.iter().map(|&id| Err(Unopened::Failed(id))).collect())
    }

    /// What `note` holds, as it is written: none where it is not there, as
    /// the note of what builds before formats were told is not here.
    fn note(&self, note: Note) -> Option<Vec<u8>> {
        match note {
            Note::Told => None,
            Note::Failing => self.failing_noted.borrow().clone(),
            Note::UnreadTold => self.unread_told.borrow().clone(),
            Note::DamagedTold => self.damaged_told.borrow().clone(),
        }
    }

    /// Have `note` hold `names`, or, given none, take it away.
    fn write_note(&self, note: Note, names: Option<&BTreeSet<String>>) {
        let held = match note {
            Note::Told => return,
            Note::Failing => &self.failing_noted,
            Note::UnreadTold => &self.unread_told,
            Note::DamagedTold => &self.damaged_told,
        };
        *held.borrow_mut() = names.map(|names| serde_json::to_vec(names).expect("a note writes"));
    }

    /// Answer every read asked for ([`Self::reads_later`]), as the store
    /// holds the records now, and wake what waits on each.
    pub fn answer_reads(&self) {
        let asked = std::mem::take(&mut *self.asked_reads.borrow_mut());
        for reads in asked {
            let ids = std::mem::take(&mut reads.borrow_mut().ids);
            let read = self.read_now(ids);
            let mut reads = reads.borrow_mut();
            reads.read = Some(read);
            if let Some(waker) = reads.waker.take() {
                waker.wake();
            }
        }
    }

    /// How many reads are waiting to be answered ([`Self::reads_later`]).
    pub fn reads_waiting(&self) -> usize {
        self.asked_reads.borrow().len()
    }

    /// Answer every ask of which records were deleted by hand
    /// ([`Self::forgotten_later`]), as the store holds them now, and wake
    /// what waits on each.
    pub fn answer_forgotten(&self) {
        let asked = std::mem::take(&mut *self.asked_forgotten.borrow_mut());
        for answer in asked {
            let mut answer = answer.borrow_mut();
            let ids = std::mem::take(&mut answer.ids);
            answer.gone = Some(self.tombstoned(ids));
            if let Some(waker) = answer.waker.take() {
                waker.wake();
            }
        }
    }

    /// Make every letting go of a mark carried asked for
    /// ([`Self::releases_later`]): the mark goes, and what waits on each is
    /// woken.
    pub fn answer_releases(&self) {
        let asked = std::mem::take(&mut *self.asked_releases.borrow_mut());
        for release in asked {
            self.taken.take();
            let mut release = release.borrow_mut();
            release.made = true;
            if let Some(waker) = release.waker.take() {
                waker.wake();
            }
        }
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
        let newest = self.records.borrow().keys().next_back().copied();
        self.plant_unreadable_as(RecordId::next(0, newest, raw), raw)
    }

    /// Store `raw` as a record of its own where a session that finished at
    /// `timestamp` goes, the way a newer build or a damaged disk leaves one
    /// this build cannot read.
    pub fn plant_unreadable_at(&self, timestamp: u64, raw: &str) -> RecordId {
        self.plant_unreadable_as(RecordId::next(timestamp, None, raw), raw)
    }

    fn plant_unreadable_as(&self, id: RecordId, raw: &str) -> RecordId {
        self.records.borrow_mut().insert(
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

    /// Have the record `id` hold `raw` from now on, under the same name:
    /// one that could not be read for a moment, and reads now.
    pub fn mend(&self, id: RecordId, raw: &str) {
        if let Some(held) = self.records.borrow_mut().get_mut(&id) {
            held.json = raw.to_string();
        }
        // What it holds is read afresh for its key.
        self.keys.borrow_mut().remove(&id);
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

    /// The history an older build keeps whole, with what tells it from
    /// another state of it ([`Self::fingerprint`]).
    fn kept_whole(&self) -> Legacy {
        let raw = self.legacy.borrow().clone()?;
        Some((raw.into_bytes(), self.fingerprint()?))
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

    /// What a look at the history sees: how many times it has changed, and
    /// the history kept whole as it stands ([`Self::seen`]).
    fn looking(&self) -> (u64, Option<Fingerprint>) {
        (self.changes.get(), self.fingerprint())
    }

    /// Make `change` through the record store, as the files make it.
    fn apply(&self, change: &HistoryChange) -> HistorySaved {
        at_once(record_store::apply(&InMem(self), change))
            .unwrap_or_else(|| HistorySaved::Failed("it waited".into()))
    }

    /// Make `writes`, as the files make them ([`make_steps`]), and say where
    /// each record went, as far as it got.
    fn make(&self, writes: &Writes<'_>) -> (Vec<Option<RecordId>>, HistorySaved) {
        if let Some(refused) = self.refused() {
            return (Vec::new(), refused);
        }
        let change = writes.change;
        let (placed, made) = make_steps(
            &InMemory(self),
            &change.put,
            &writes.tombstones(),
            &change.unforget,
        );
        if let Err(saved) = made {
            self.changed_here();
            return (placed, saved);
        }
        let (tombstones, mut records) = (self.tombstones.borrow(), self.records.borrow_mut());
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
            let squeezed = HistorySaved::Squeezed {
                kept: room,
                runs,
                trimmed,
            };
            return (placed, squeezed);
        }
        let stored = HistorySaved::Stored {
            kept: change.put.len(),
        };
        (placed, stored)
    }

    /// Why the store takes no change to the history now, the way a full or
    /// failing disk takes none: nothing, where it takes them.
    fn refused(&self) -> Option<HistorySaved> {
        if let Some(why) = self.failing.borrow().clone() {
            return Some(HistorySaved::Failed(why));
        }
        self.full.get().then_some(HistorySaved::Full)
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

    /// A session an older build stored, taken up where other bytes have the
    /// name the take-up gives it, goes under the next name, and is read
    /// there, as on the files and in a browser: the load holds it there,
    /// beside what holds its own name, and an older build's deletion of it
    /// deletes that record and not the other.
    #[test]
    fn a_session_taken_up_under_a_name_other_bytes_have_is_read_where_it_went() {
        let store = MemoryStore::default();
        let at = crate::time::now_ms();
        let before = [finished(at - 30 * DAY), finished(at - 20 * DAY)];
        store.keep_whole(&before);
        store.load_history();
        let since = finished(at - 10 * DAY);
        let asked = Record::of(&since, |json| RecordId::placed(since.timestamp, json))
            .expect("a record")
            .id;
        let other = serde_json::to_string(&finished(at - 5 * DAY)).unwrap();
        store.records.borrow_mut().insert(
            asked,
            Held {
                json: other.clone(),
                whole: None,
            },
        );
        store.keep_whole(&[before[0].clone(), before[1].clone(), since.clone()]);
        let load = store.load_history();
        let went = asked.bumped();
        assert!(
            load.sessions.contains(&(went, since.clone())),
            "where it went: {:?}",
            load.sessions
        );
        assert!(
            load.sessions.iter().any(|(id, _)| *id == asked),
            "beside the other bytes"
        );
        // The older build deletes it.
        store.keep_whole(&before);
        *store.legacy_deleted.borrow_mut() = vec![Deleted::of(&since, at)];
        let listed = now(store.history_listing()).expect("listed");
        assert!(
            listed.contains(&asked) && !listed.contains(&went),
            "{listed:?}"
        );
        assert_eq!(store.record(asked), Some(other));
        assert_eq!(store.tombstone(asked), None);
        assert!(store.tombstone(went).is_some());
    }

    /// A session put back whose record is there still, as one a sync took
    /// back from the drive in the meantime, is back as it is stored, under
    /// its own name, as the files and a browser's database keep it: one
    /// record, not two of one session. The test store used to store the
    /// one put back beside it, under the next name, where its bytes
    /// differed.
    #[test]
    fn a_session_put_back_whose_record_is_there_is_one_record() {
        let store = MemoryStore::default();
        let first = finished(DAY);
        let one = record(&first);
        now(store.change_history(put(std::slice::from_ref(&one))));
        let undo = HistoryChange {
            put: vec![Record {
                id: one.id,
                json: serde_json::to_string_pretty(&first).expect("JSON"),
            }],
            unforget: vec![one.id],
            ..HistoryChange::default()
        };
        assert_eq!(
            now(store.change_history(undo)),
            HistorySaved::Stored { kept: 1 }
        );
        let ids: Vec<RecordId> = store
            .load_history()
            .sessions
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(ids, [one.id], "one record");
        assert_eq!(store.record(one.id), Some(one.json), "as it was stored");
    }

    /// Which records were deleted by hand comes with when each was, as its
    /// tombstone says, here and in the page storage's history kept whole;
    /// and with no word at all where the store cannot look, rather than
    /// the word that none was.
    #[test]
    fn deletions_say_when_each_was_made_or_that_none_could_be_looked_for() {
        let store = MemoryStore::default();
        let (first, second) = (finished(DAY), finished(2 * DAY));
        let (one, two) = (record(&first), record(&second));
        now(store.change_history(put(&[one.clone(), two.clone()])));
        now(store.change_history(HistoryChange {
            forget: vec![(one.id, Deleted::of(&first, 7))],
            ..HistoryChange::default()
        }));
        assert_eq!(
            now(store.deletions(vec![one.id, two.id])),
            Some(vec![(one.id, Some(7))])
        );
        assert_eq!(
            now(store.deleted_sessions(vec![one.id, two.id])),
            Some(vec![(one.id, SessionKey::of(&first))])
        );
        let keys = vec![SessionKey::of(&first), SessionKey::of(&second)];
        assert_eq!(
            now(store.deleted_keys(keys.clone())),
            Some(vec![SessionKey::of(&first)])
        );
        store.deletions_unknown.set(true);
        assert_eq!(now(store.deletions(vec![one.id, two.id])), None);
        assert_eq!(now(store.deleted_sessions(vec![one.id])), None);
        assert_eq!(now(store.deleted_keys(keys)), None);
        assert!(
            now(store.forgotten(vec![one.id])).is_empty(),
            "missing is never deleted"
        );

        let shelf = PageShelf::default();
        shelf.keep(&[first.clone(), second]);
        let page = MemoryStore::in_page(&shelf);
        let ids: Vec<RecordId> = page
            .load_history()
            .sessions
            .iter()
            .map(|(id, _)| *id)
            .collect();
        now(page.change_history(HistoryChange {
            forget: vec![(ids[0], Deleted::of(&first, 9))],
            ..HistoryChange::default()
        }));
        assert_eq!(
            now(page.deletions(ids.clone())),
            Some(vec![(ids[0], Some(9))])
        );
        shelf.block(true);
        assert_eq!(now(page.deletions(ids)), None, "the page storage blocked");
    }

    /// A lock is held by one copy of the app at a time, whichever copy
    /// takes it, and is free again once let go of; each lock is apart from
    /// the other. A store that takes no locks says so.
    #[test]
    fn a_lock_is_held_by_one_copy_at_a_time() {
        let store = MemoryStore::default();
        let run = match now(store.try_lock(Lock::SyncRun)) {
            Locking::Held(held) => held,
            other => panic!("held by the copy that tries it first: {other:?}"),
        };
        let busy = now(store.try_lock(Lock::SyncRun));
        assert!(matches!(busy, Locking::Busy), "{busy:?}");
        let another = store.hold_lock(Lock::SyncState);
        let busy = now(store.try_lock(Lock::SyncState));
        assert!(matches!(busy, Locking::Busy), "held by another: {busy:?}");
        drop(run);
        let free = now(store.try_lock(Lock::SyncRun));
        assert!(
            matches!(free, Locking::Held(_)),
            "free once let go of: {free:?}"
        );
        drop(another);
        let free = now(store.try_lock(Lock::SyncState));
        assert!(matches!(free, Locking::Held(_)), "{free:?}");
        store.unlockable.set(true);
        let none = now(store.try_lock(Lock::SyncRun));
        assert!(matches!(none, Locking::Unlockable), "{none:?}");
    }

    /// As a browser's, the store takes only the locks a browser takes: the
    /// one on syncing, and none on what sync keeps.
    #[test]
    fn as_a_browser_the_store_takes_only_what_a_browser_locks() {
        let store = MemoryStore::default();
        store.browser.set(true);
        let none = now(store.try_lock(Lock::SyncState));
        assert!(matches!(none, Locking::Unlockable), "{none:?}");
        let held = now(store.try_lock(Lock::SyncRun));
        assert!(matches!(held, Locking::Held(_)), "{held:?}");
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
        assert_eq!(
            now(store.read_records(vec![one.id])),
            [Err(Unopened::Failed(one.id))]
        );
        store.unlistable.set(false);
        assert!(store.history_changed());
        assert_eq!(now(store.history_listing()), Some(vec![one.id]));
        assert!(!store.history_changed(), "seen as it was listed");
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
        assert_eq!(read.notices, [HistoryNotice::Damaged { count: 1 }]);
        assert!(store.load_history().notices.is_empty(), "told of once");
    }

    /// A change is told of each time it is asked of, until a look that
    /// lists the history takes it up, as the files and a browser's
    /// database tell of one: a look that could not list it leaves it to
    /// take up. Asking noted it as seen, so a test of a listing that
    /// failed had the change never taken up, as no real store has it.
    #[test]
    fn a_change_is_still_to_take_up_after_a_listing_that_failed() {
        let store = MemoryStore::default();
        store.load_history();
        assert!(!store.history_changed());
        let one = record(&finished(1_000));
        now(store.change_history(put(std::slice::from_ref(&one))));
        assert!(store.history_changed(), "to take up");
        assert!(store.history_changed(), "still, until it is");
        store.unlistable.set(true);
        assert_eq!(now(store.history_listing()), None);
        store.load_history();
        store.unlistable.set(false);
        assert!(store.history_changed(), "still to take up");
        assert_eq!(now(store.history_listing()), Some(vec![one.id]));
        assert!(!store.history_changed(), "taken up");
        store.lose(one.id);
        assert!(store.history_changed());
        store.load_history();
        assert!(!store.history_changed(), "taken up as it is read");
    }

    /// A record told of as one that could not be read, which then reads,
    /// at a later launch or at one of the app's tries, and fails again
    /// after, is told of again, once, as the files tell of it: that is a
    /// failure of its own. The test store kept one note of all it told of,
    /// for good, and it was never told of again.
    #[test]
    fn a_record_told_of_as_unread_that_reads_and_fails_again_is_told_of_again() {
        let store = MemoryStore::default();
        store.load_history();
        let busy = record(&finished(2_000));
        now(store.change_history(put(std::slice::from_ref(&busy))));
        let hold = |held: bool| {
            let mut open = store.held_open.borrow_mut();
            if held {
                open.insert(busy.id);
            } else {
                open.clear();
            }
        };
        let told = [HistoryNotice::Unread { count: 1 }];
        hold(true);
        assert_eq!(store.load_history().notices, []);
        assert_eq!(
            now(store.still_unread(vec![busy.id])),
            told,
            "after every try"
        );
        // It reads at the next launch, and fails at the one after.
        hold(false);
        assert_eq!(store.load_history().sessions.len(), 1);
        hold(true);
        assert_eq!(store.load_history().failed, [busy.id]);
        assert_eq!(
            now(store.still_unread(vec![busy.id])),
            told,
            "told of again"
        );
        assert_eq!(now(store.still_unread(vec![busy.id])), [], "once");
        // It reads at one of the app's tries, and fails at a later one.
        hold(false);
        assert_eq!(now(store.read_records(vec![busy.id])), [Ok(busy.clone())]);
        hold(true);
        assert_eq!(
            now(store.read_records(vec![busy.id])),
            [Err(Unopened::Failed(busy.id))]
        );
        assert_eq!(now(store.still_unread(vec![busy.id])), told, "and again");
        assert_eq!(now(store.still_unread(vec![busy.id])), [], "once");
    }

    /// A record told of as one that could not be read, after the app's
    /// tries, that is found damaged at a later launch is told of as damaged,
    /// as the files tell of it: one note of all it told of held it back.
    #[test]
    fn a_record_told_of_as_unread_that_is_damaged_since_is_told_of_as_damaged() {
        let store = MemoryStore::default();
        store.load_history();
        let json = serde_json::to_string(&finished(2_000)).expect("writes");
        let busy = store.plant_unreadable(&json);
        store.held_open.borrow_mut().insert(busy);
        assert_eq!(store.load_history().failed, [busy]);
        assert_eq!(
            now(store.still_unread(vec![busy])),
            [HistoryNotice::Unread { count: 1 }]
        );
        store.held_open.borrow_mut().clear();
        store.mend(busy, "{ cut short");
        assert_eq!(
            store.load_history().notices,
            [HistoryNotice::Damaged { count: 1 }],
            "told of as damaged"
        );
    }

    /// A record told of as one that could not be read, which is no longer
    /// listed at a look, is told of no more, as the files note it: it is
    /// gone, and should it come back and fail, that is a failure of its own,
    /// told of in turn. One note of all it told of held it back for good.
    #[test]
    fn a_record_told_of_that_is_gone_at_a_look_is_told_of_no_more() {
        let store = MemoryStore::default();
        store.load_history();
        let busy = record(&finished(2_000));
        now(store.change_history(put(std::slice::from_ref(&busy))));
        store.held_open.borrow_mut().insert(busy.id);
        assert_eq!(store.load_history().failed, [busy.id]);
        let told = [HistoryNotice::Unread { count: 1 }];
        assert_eq!(now(store.still_unread(vec![busy.id])), told);
        store.lose(busy.id);
        assert_eq!(
            now(store.history_listing()),
            Some(Vec::new()),
            "gone, at a look"
        );
        now(store.change_history(put(std::slice::from_ref(&busy))));
        assert_eq!(
            now(store.read_records(vec![busy.id])),
            [Err(Unopened::Failed(busy.id))]
        );
        assert_eq!(
            now(store.still_unread(vec![busy.id])),
            told,
            "back, and failing on its own"
        );
    }

    /// A record told of as damaged, which reads again, then is damaged once
    /// more, is told of again, as the files tell of it: that is a damage of
    /// its own. One note of all it told of held it back for good.
    #[test]
    fn a_record_told_of_as_damaged_that_reads_and_is_damaged_again_is_told_of_again() {
        let store = MemoryStore::default();
        store.load_history();
        let damaged = store.plant_unreadable("{ cut short");
        let told = [HistoryNotice::Damaged { count: 1 }];
        assert_eq!(store.load_history().notices, told);
        assert_eq!(store.load_history().notices, [], "once");
        let json = serde_json::to_string(&finished(2_000)).expect("writes");
        store.mend(damaged, &json);
        assert_eq!(store.load_history().sessions.len(), 1, "it reads");
        store.mend(damaged, "{ cut short again");
        assert_eq!(store.load_history().notices, told, "damaged again");
    }

    /// A backup asks the app for a record the store holds but cannot read
    /// now, as one held open cannot be, as the files and a browser's
    /// database ask for it: the app saves again with every session it
    /// holds. The test store backed up the record's text all the same.
    #[test]
    fn a_backup_asks_the_app_for_a_record_that_cannot_be_read_now() {
        let store = MemoryStore::default();
        store.load_history();
        let busy = record(&finished(2_000));
        now(store.change_history(put(std::slice::from_ref(&busy))));
        store.held_open.borrow_mut().insert(busy.id);
        let backup = BackupFile::new(&TrainingSettings::default(), 5, Vec::new());
        assert_eq!(
            now(store.save_backup("b.json", backup)),
            Err(NotSaved::Unread)
        );
        assert!(store.backup_files.borrow().is_empty(), "none saved");
        let every =
            BackupFile::new(&TrainingSettings::default(), 5, Vec::new()).holding_every_session();
        assert!(now(store.save_backup("b.json", every)).is_ok());
        let (_, saved) = store.backup_files.borrow()[0].clone();
        assert!(!saved.contains(&busy.json), "{saved}");
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

    /// An older build's write to the history it keeps whole is a change to
    /// take up, as the files and a browser's database see one: the history
    /// changed, until a listing takes it up. It was no change, so the app
    /// never listed to take up what the older build stored.
    #[test]
    fn an_older_builds_write_to_the_history_kept_whole_is_a_change_to_take_up() {
        let store = MemoryStore::default();
        let now_ms = crate::time::now_ms();
        let first = finished(now_ms - 9 * DAY);
        store.keep_whole(std::slice::from_ref(&first));
        store.load_history();
        assert!(!store.history_changed());
        store.keep_whole(&[first, finished(now_ms + 60_000)]);
        assert!(store.history_changed(), "to take up");
        assert_eq!(
            now(store.history_listing()).map(|listed| listed.len()),
            Some(2)
        );
        assert!(!store.history_changed(), "taken up");
    }

    /// What an older build stored that could not be taken up just then, as a
    /// full store takes no change, is still to take up: the history changed,
    /// as the app is told, until a look takes it up, as the files and a
    /// browser's database tell of it. It was noted as seen at the look that
    /// could not take it up, so the app did not look again.
    #[test]
    fn a_take_up_the_store_could_not_make_is_still_to_take_up() {
        let store = MemoryStore::default();
        let now_ms = crate::time::now_ms();
        let first = finished(now_ms - 9 * DAY);
        store.keep_whole(std::slice::from_ref(&first));
        store.load_history();
        store.keep_whole(&[first, finished(now_ms + 60_000)]);
        store.full.set(true);
        assert_eq!(
            now(store.history_listing()).map(|listed| listed.len()),
            Some(1),
            "not taken up"
        );
        assert!(store.history_changed(), "still to take up");
        assert_eq!(store.load_history().sessions.len(), 1, "nor as it is read");
        assert!(store.history_changed(), "still to take up");
        store.full.set(false);
        assert_eq!(
            now(store.history_listing()).map(|listed| listed.len()),
            Some(2),
            "taken up"
        );
        assert!(!store.history_changed(), "taken up");
    }

    /// A record read once holds back no take-up while it cannot be read for
    /// a moment, as one held open cannot be: its session is known, as the
    /// files and a browser's database keep the key of each record they
    /// read, and what an older build stored since is taken up at a
    /// listing. The test store gave up every take-up while any record was
    /// held open.
    #[test]
    fn a_record_read_once_holds_back_no_take_up_while_it_cannot_be_read_for_a_moment() {
        let store = MemoryStore::default();
        let now_ms = crate::time::now_ms();
        let first = finished(now_ms - 9 * DAY);
        store.keep_whole(std::slice::from_ref(&first));
        let read = store.load_history();
        let [(moved, _)] = read.sessions[..] else {
            panic!("moved: {:?}", read.sessions);
        };
        store.held_open.borrow_mut().insert(moved);
        store.keep_whole(&[first, finished(now_ms + 60_000)]);
        assert_eq!(
            now(store.history_listing()).map(|listed| listed.len()),
            Some(2),
            "taken up"
        );
    }

    /// A read of the history whole takes nothing up from an older build
    /// while a record here cannot be read, as the files' read takes none:
    /// it may hold a session the older build holds too. The next look at
    /// which every record reads takes it up.
    #[test]
    fn a_read_takes_nothing_up_while_a_record_cannot_be_read() {
        let store = MemoryStore::default();
        let now_ms = crate::time::now_ms();
        let first = finished(now_ms - 9 * DAY);
        store.keep_whole(std::slice::from_ref(&first));
        let read = store.load_history();
        let [(moved, _)] = read.sessions[..] else {
            panic!("moved: {:?}", read.sessions);
        };
        store.held_open.borrow_mut().insert(moved);
        store.keep_whole(&[first, finished(now_ms + 60_000)]);
        let writes = store.record_writes.get();
        let read = store.load_history();
        assert_eq!(read.failed, [moved]);
        assert!(read.sessions.is_empty(), "{:?}", read.sessions);
        assert_eq!(store.record_writes.get(), writes, "nothing taken up");
        store.held_open.borrow_mut().clear();
        assert_eq!(store.load_history().sessions.len(), 2, "taken up then");
    }

    /// A record here a later build stored, which says which session it
    /// holds, holds back the older build's copy of that session, as the
    /// files take them; a damaged one, which no build reads though it says
    /// the same, does not, and is left as it is.
    #[test]
    fn only_a_later_builds_record_holds_back_the_older_builds_copy_of_its_session() {
        let store = MemoryStore::default();
        let now_ms = crate::time::now_ms();
        let first = finished(now_ms - 9 * DAY);
        store.keep_whole(std::slice::from_ref(&first));
        store.load_history();
        let (later, damaged) = (finished(now_ms + 60_000), finished(now_ms + 120_000));
        let says = |session: &SessionResult, format: u32| {
            format!(
                r#"{{"format":{format},"date":{:?},"timestamp":{},"by":"elsewhere"}}"#,
                session.date, session.timestamp
            )
        };
        let theirs = says(&later, cw_core::SESSION_FORMAT + 1);
        store.plant_unreadable_at(later.timestamp, &theirs);
        let mine = says(&damaged, cw_core::SESSION_FORMAT);
        let left = store.plant_unreadable_at(damaged.timestamp, &mine);
        store.keep_whole(&[first.clone(), later, damaged.clone()]);
        let read = store.load_history();
        assert_eq!(
            read.sessions
                .iter()
                .map(|(_, session)| session.timestamp)
                .collect::<Vec<_>>(),
            [first.timestamp, damaged.timestamp]
        );
        assert_eq!(store.record(left), Some(mine), "left as it is");
    }

    /// A move made again over records stored since, as one the store would
    /// not take waits for the next read while a restore stores, writes no
    /// session held already, under whatever name, as the files and a
    /// browser's database write none: one record of it, not two.
    #[test]
    fn a_move_made_over_records_writes_no_session_held_already() {
        let store = MemoryStore::default();
        let now_ms = crate::time::now_ms();
        let (first, second) = (finished(now_ms - 9 * DAY), finished(now_ms - 8 * DAY));
        store.keep_whole(&[first.clone(), second.clone()]);
        store.full.set(true);
        assert!(!store.load_history().complete, "the move waits");
        store.full.set(false);
        let restored =
            Record::of(&first, |json| RecordId::placed(first.timestamp, json)).expect("a record");
        assert_eq!(
            now(store.change_history(put(std::slice::from_ref(&restored)))),
            HistorySaved::Stored { kept: 1 }
        );
        let read = store.load_history();
        assert!(read.complete);
        let stamps: Vec<u64> = read
            .sessions
            .iter()
            .map(|(_, session)| session.timestamp)
            .collect();
        assert_eq!(stamps, [first.timestamp, second.timestamp], "each once");
        assert!(store.record(restored.id).is_some(), "where it was restored");
    }

    /// The same move waits while a record here cannot be read, as one held
    /// open cannot be, as the files' waits: it may hold a session the move
    /// would write again. Once it reads, the move is made, each session once.
    /// The test store read what each record held, held open or not, so its
    /// move never waited, where the files' does.
    #[test]
    fn a_move_made_over_a_record_that_cannot_be_read_now_waits_for_it() {
        let store = MemoryStore::default();
        let now_ms = crate::time::now_ms();
        let (first, second) = (finished(now_ms - 9 * DAY), finished(now_ms - 8 * DAY));
        store.keep_whole(&[first.clone(), second.clone()]);
        store.full.set(true);
        assert!(!store.load_history().complete, "the move waits");
        store.full.set(false);
        let restored =
            Record::of(&first, |json| RecordId::placed(first.timestamp, json)).expect("a record");
        assert_eq!(
            now(store.change_history(put(std::slice::from_ref(&restored)))),
            HistorySaved::Stored { kept: 1 }
        );
        store.held_open.borrow_mut().insert(restored.id);
        let waited = store.load_history();
        assert!(
            matches!(&waited.notices[..], [HistoryNotice::MovePending(_)]),
            "{:?}",
            waited.notices
        );
        assert!(!waited.complete);
        assert_eq!(waited.failed, [restored.id]);
        store.held_open.borrow_mut().clear();
        let read = store.load_history();
        assert!(read.complete && read.notices.is_empty());
        let stamps: Vec<u64> = read
            .sessions
            .iter()
            .map(|(_, session)| session.timestamp)
            .collect();
        assert_eq!(stamps, [first.timestamp, second.timestamp], "each once");
    }

    /// A move with nothing to move waits for no record, as the files' waits
    /// for none: it writes nothing.
    #[test]
    fn a_move_with_nothing_to_move_waits_for_nothing() {
        let store = MemoryStore::default();
        let one = record(&finished(1_000));
        now(store.change_history(put(std::slice::from_ref(&one))));
        store.held_open.borrow_mut().insert(one.id);
        let read = store.load_history();
        assert!(
            read.complete && read.notices.is_empty(),
            "{:?}",
            read.notices
        );
        assert_eq!(read.failed, [one.id]);
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
