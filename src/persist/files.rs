//! The history as files, one a record ([`super::record`]), under `history/`
//! in the store's folder:
//!
//! - `sessions/<id>.json`: a session, as the build that stored it wrote it,
//!   never written over.
//! - `deleted/<id>.json`: a tombstone. The record of the same name was
//!   deleted by hand, and is gone, or goes as the history is next read.
//!   What it holds ([`Deleted`]) tells the session apart by when and on
//!   what day it finished; one that is empty, or does not read, deletes it
//!   all the same.
//! - `layout.json`: how the history is kept, and what the move from the
//!   history kept whole took of it ([`Layout`]). The move writes it last.
//! - `told.json`: the records this build could not read, and has said so.
//! - `restoring-<n>.json`: the records a restore of many sessions at once
//!   is storing, held locked while it does: one left unlocked was cut
//!   short, as Dust stopped.
//! - `packs/*.jsonl.gz`: older records a later build packs together, to
//!   keep the history to fewer and smaller files. This build only reads
//!   them: gzip, one record a line, `{"id": <its name>, "s": <what was
//!   written for it, as a JSON string>}`. A record both packed and loose
//!   is one record; a tombstone deletes one packed as it deletes one
//!   loose.
//!
//! Every file is written whole or not at all ([`place_whole`]), so two
//! copies of the app on one folder, which take no lock between them, never
//! write into each other's: each takes up the records and tombstones the
//! other leaves ([`HistoryFiles::listing`]).
//!
//! The app reads and writes them on a thread of their own
//! ([`super::worker`]), and asks nothing of them on its own but whether
//! they changed elsewhere ([`Watch`]).
//!
//! An older build never looks in `history/`. It keeps its history whole, in
//! `sessions.json`, which the move reads once and leaves as it was, and
//! whatever an older build stores there after the move is taken up here
//! ([`legacy::imported`]).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime};

use cw_core::SessionResult;
use flate2::read::GzDecoder;

use super::backup::{BackedUp, BackupFile, NotSaved};
use super::disk::{aside_name, flush, move_aside, place_whole, shown, sync_dir, write_whole};
use super::legacy::{self, Fingerprint, Gate, Layout, read_history};
use super::progress::LoadProgress;
use super::record::{
    Deletions, HistoryChange, HistoryLoad, HistoryNotice, Record, RecordId, Unopened, Unreadable,
    held_key, key_of, keyed, names_noted, not_told, read_record, taken_up, tombstone_at,
    tombstone_key, unreadable,
};
use super::saved::HistorySaved;
use super::session::{Deleted, SessionKey};
use super::set_aside::SetAside;

/// The folder of the history, in the store's.
const HISTORY: &str = "history";
/// The history as builds before records keep it whole, which the move to
/// records reads once and leaves as it was ([`HistoryFiles`]).
pub(super) const SESSIONS: &str = "sessions.json";
/// The sessions deleted by hand ([`super::Deleted`]), as builds before
/// records remember them.
pub(super) const DELETED: &str = "deleted.json";
/// The records, in the history's folder.
const RECORDS: &str = "sessions";
/// The tombstones, in the history's folder.
const TOMBSTONES: &str = "deleted";
/// The records packed together, in the history's folder.
const PACKS: &str = "packs";
const PACK_EXTENSION: &str = ".jsonl.gz";
const LAYOUT_FILE: &str = "layout.json";
const TOLD: &str = "told.json";
/// The records that could not be read at all as the history was last read
/// whole, by name ([`HistoryLoad::failed`]), or at the app's last try at
/// one ([`HistoryFiles::still_unread`]), and have not read since, nor gone
/// ([`HistoryFiles::note_no_more`]): one that still cannot be read as it is
/// read again, at a later launch, is told of then ([`HistoryFiles::tell`]).
/// It holds this build's failures alone, whether told of or not; [`TOLD`]
/// names those this build told of, beside every record a build before
/// formats could not open or make out.
const FAILING: &str = "failing.json";
/// The records this build told of as ones that could not be read at all,
/// a moment's failure that lasted ([`HistoryNotice::Unread`]), by name,
/// until each reads, or is gone. No build before this one writes it, so a
/// record such a build told of in [`TOLD`], which it does of every record
/// it could not open, a moment's failure among them, holds back none of
/// this build's notices of a failure of its own. Each is noted in [`TOLD`]
/// as well, where such a build looks, so that it does not tell of it a
/// second time: that note only gains names, so such a build, used alone,
/// says nothing of one that reads since and fails again, which this build
/// tells of.
const UNREAD_TOLD: &str = "unread-told.json";
/// The records this build told of as damaged, or as a newer Dust's
/// ([`HistoryNotice::Damaged`], [`HistoryNotice::Later`]), by name, until
/// each reads, or is gone, so that one damaged again after is told of
/// again. No build before this one writes it, so a record such a build
/// noted in [`TOLD`], which it does of every record it could not open or
/// make out, one this build reads among them, holds back none of this
/// build's notices of a damage of its own. Until this build first writes
/// it, [`TOLD`] stands for it, so that a record such a build told of, and
/// this one cannot read either, is not told of a second time; written, it
/// stays, though it names none.
const DAMAGED_TOLD: &str = "damaged-told.json";
/// What a restore of many sessions at once is called while it runs:
/// `restoring-<n>.json`.
const JOURNAL_PREFIX: &str = "restoring-";

/// How old a temporary file a write left behind has to be to be cleared:
/// far older than any write takes, so none still being written goes.
const OLD_TEMPORARY: Duration = Duration::from_secs(60 * 60);

/// How many names past its own a record is tried under, where other bytes
/// have its name already ([`RecordId::bumped`]).
const BUMPS: u64 = 64;

/// How many records it takes for the history to be read on more than one
/// thread as it loads ([`read_each`]): fewer are read as fast on one.
const READ_APART_FROM: usize = 512;

/// How many threads at most read the history as it loads.
const READERS: usize = 4;

/// How many threads at most store the records the move makes
/// ([`place_each`]): a filesystem makes the flushes of many files at once
/// durable together, where flushed one after another each waits for the
/// disk on its own.
const WRITERS: usize = 8;

/// How many records a backup reads at a time, each part read whole before
/// any of it is written ([`HistoryFiles::stream_backup`]): some two
/// megabytes, at some seven kilobytes a session, however long the history.
const BACKUP_PART: usize = 256;

/// The thread the history's files are read and written on.
pub(super) const THREAD: &str = "dust-history";

/// The history as files under one folder.
pub(super) struct HistoryFiles {
    /// The store's folder, where an older build keeps the history whole.
    root: PathBuf,
    /// `history/` in it.
    dir: PathBuf,
    /// What the folders looked like when this copy last looked
    /// ([`Watch::changed`]).
    watch: Watch,
    /// The key of each record read so far, by its id: what an older
    /// build's deletions are told apart by, without reading every record
    /// again each time one is taken up.
    keys: RefCell<HashMap<RecordId, SessionKey>>,
    /// The files are the worker's ([`super::worker`]): every look at them,
    /// and every write, is made on its thread and on no other.
    on_worker: bool,
    /// How many bytes the sessions stored hold, as of the last read and
    /// every change since: what the app asks, on its own thread, of how
    /// large the history has grown ([`Self::size`]).
    size: Arc<AtomicU64>,
    /// How far the read of the whole history, or the move before it, has
    /// come, for the app's own thread to ask as it opens ([`Self::load`]).
    progress: Arc<LoadProgress>,
    /// The records each pack held when it last opened, by the pack's name:
    /// what one that cannot be opened for now holds, for all this copy
    /// knows ([`Self::packed`]).
    pack_records: RefCell<HashMap<OsString, Vec<RecordId>>>,
    /// What another copy of the app does while this one lists or reads the
    /// history, for a test to set: done each time a folder of it has been
    /// read ([`Self::meanwhile()`]).
    #[cfg(test)]
    elsewhere: RefCell<Option<Box<dyn FnMut() + Send>>>,
}

/// Whether the history changed since this copy last looked, here or in the
/// history kept whole: what the folders looked like then. Shared by the
/// thread that reads and writes the files, which notes each change it made
/// itself as none, and the app's own, which asks
/// ([`super::Store::history_changed`]): that takes a look at when each
/// folder last changed, and none at what is in it.
#[derive(Clone)]
pub(super) struct Watch {
    root: PathBuf,
    dir: PathBuf,
    seen: Arc<Mutex<Option<Stamps>>>,
}

/// What a look at the folders saw: when each was last changed, and the
/// history kept whole as it stands.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stamps {
    records: Option<SystemTime>,
    packs: Option<SystemTime>,
    tombstones: Option<SystemTime>,
    legacy: Option<Fingerprint>,
}

/// What the history's folders hold.
struct Listing {
    /// Every record, loose or packed, in order: one in a pack that could
    /// not be opened among them where the pack held it when it last opened.
    records: BTreeSet<RecordId>,
    /// Every tombstone.
    gone: BTreeSet<RecordId>,
    /// What each packed record holds, as it was written.
    packed: BTreeMap<RecordId, String>,
    /// Whether a pack could not be opened: what it holds is in none of
    /// the above, but for the records it held when it last opened.
    skipped: bool,
}

/// What the packs a later build wrote hold, as they were read
/// ([`HistoryFiles::packed`]).
#[derive(Default)]
struct Packs {
    /// What each packed record holds, as it was written.
    records: BTreeMap<RecordId, String>,
    /// Whether a pack could not be opened, and gave up none of its records.
    skipped: bool,
    /// The records the packs that could not be opened held when they last
    /// opened: there still, for all this copy knows, though not to be read.
    unopened: BTreeSet<RecordId>,
}

/// A record as a pack holds it: one line.
#[derive(serde::Deserialize)]
struct PackLine {
    id: String,
    s: String,
}

/// A restore of many sessions at once, as it runs: its journal, held
/// locked until the change is over.
struct Journal {
    path: PathBuf,
    /// Held for as long as the change runs.
    _lock: fs::File,
}

/// What a journal says: the records its change set out to store, and of
/// them those deleted before, whose deletion it undoes.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Journaled {
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    undone: Vec<String>,
}

impl HistoryFiles {
    /// The history in the store's folder `root`.
    pub(super) fn at(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            dir: root.join(HISTORY),
            watch: Watch {
                root: root.to_path_buf(),
                dir: root.join(HISTORY),
                seen: Arc::new(Mutex::new(None)),
            },
            keys: RefCell::new(HashMap::new()),
            on_worker: false,
            size: Arc::new(AtomicU64::new(0)),
            progress: Arc::default(),
            pack_records: RefCell::new(HashMap::new()),
            #[cfg(test)]
            elsewhere: RefCell::new(None),
        }
    }

    /// How many bytes the sessions stored hold, kept up to date as the
    /// history is read and changed, for the app's own thread to ask.
    pub(super) fn size(&self) -> Arc<AtomicU64> {
        self.size.clone()
    }

    /// How far the read of the whole history has come, counted as it goes,
    /// for the app's own thread to ask.
    pub(super) fn progress(&self) -> Arc<LoadProgress> {
        self.progress.clone()
    }

    /// The store's folder.
    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    /// What tells whether the history changed elsewhere, for the app's own
    /// thread to ask while the files are the worker's.
    pub(super) fn watch(&self) -> Watch {
        self.watch.clone()
    }

    /// From here on the files are the worker's: looked at and written on
    /// its thread alone ([`THREAD`]).
    pub(super) fn keep_to_worker(&mut self) {
        self.on_worker = true;
    }

    /// Every look at the files, and every write, is made on the worker's
    /// thread, once they are the worker's: on a phone, any other thread
    /// that reaches them is one the app's taps wait on.
    fn on_its_thread(&self) {
        debug_assert!(
            !self.on_worker || thread::current().name() == Some(THREAD),
            "the history's files were reached from {:?}, not from {}",
            thread::current().name(),
            THREAD
        );
    }

    fn records(&self) -> PathBuf {
        self.dir.join(RECORDS)
    }

    fn tombstones(&self) -> PathBuf {
        self.dir.join(TOMBSTONES)
    }

    /// Read the whole history: making the move from a history kept whole
    /// first, where none was made, and taking up what an older build stored
    /// there since, where one was, and every record here could be read, and
    /// every pack of them opened.
    ///
    /// A record and its tombstone read as deleted, and the deletion cut
    /// short is finished. A record this build cannot read is left where it
    /// is, and told of once, as a newer Dust's or as damaged ([`not_told`]).
    /// One that could not be read at all just now failed
    /// ([`HistoryLoad::failed`]), for the app to read again: told of, once,
    /// only where it could not be read as the history was last read either
    /// ([`Self::tell`]). A history a newer build keeps in a way this one
    /// would harm is read, and nothing is written to it, not even to tidy
    /// up.
    pub(super) fn load(&self) -> HistoryLoad {
        self.on_its_thread();
        let mut load = HistoryLoad::default();
        // How the history is kept, once moved, and whether this build may
        // write to it at all.
        let (layout, writable) = match self.gate() {
            Ok(Gate::Open(layout)) => (Some(*layout), true),
            Ok(Gate::Unmoved) => match self.migrate(&mut load) {
                Ok(layout) => (Some(layout), true),
                Err(why) => {
                    load.notices.push(HistoryNotice::MovePending(why));
                    (None, true)
                }
            },
            Ok(Gate::Newer) => {
                load.notices.push(HistoryNotice::Newer);
                (None, false)
            }
            Ok(Gate::Unread) => (None, false),
            Err(err) => {
                eprintln!("could not read how the history is kept: {err}");
                (None, false)
            }
        };
        if writable {
            self.clear_temporaries();
            load.notices.extend(self.journals_cut_short());
        }
        // As the folders were before they were read: whatever another copy
        // changes from here on is taken up at the next look. Where they
        // cannot be read, nothing was seen, and the next look reads them.
        let before = self.watch.stamps();
        let listing = match self.listing_now() {
            Ok(listing) => listing,
            Err(err) => {
                eprintln!("could not list the history: {err}");
                return load;
            }
        };
        let mut unlinked = false;
        let mut live = Vec::with_capacity(listing.records.len());
        for &id in &listing.records {
            if listing.gone.contains(&id) {
                // Deleted, and the record left behind: a deletion cut short.
                if writable {
                    unlinked |= fs::remove_file(self.records().join(file_name(id))).is_ok();
                }
                continue;
            }
            live.push(id);
        }
        let records = self.records();
        self.progress.reading(Some(live.len() as u64));
        let mut read = read_each(&records, &live, &listing.packed, &self.progress);
        // One a later build packed once the packs were read, and took the
        // loose copy of before it was read, is in neither place: it is
        // looked for again, once, in the packs as they are by now.
        if read.iter().any(Option::is_none) {
            let packed = self.packed().unwrap_or_default().records;
            for (read, id) in read.iter_mut().zip(&live) {
                if read.is_none() {
                    *read = read_one(&records, *id, &packed);
                }
            }
        }
        let size = read.iter().flatten().map(|(_, bytes)| *bytes).sum();
        self.size.store(size, Ordering::Relaxed);
        let mut keys = self.keys.borrow_mut();
        load.sessions.reserve(live.len());
        // Each record that holds no session this build reads, with what it
        // says of itself.
        let mut unread = Vec::new();
        for (id, read) in live.into_iter().zip(read) {
            match read {
                Some((Ok(session), _)) => {
                    keys.insert(id, SessionKey::of(&session));
                    load.sessions.push((id, session));
                }
                Some((Err(said), _)) => {
                    // A later build's is there all the same: what an older
                    // build stores of the session it says it holds is no
                    // session to take up beside it. A damaged one holds
                    // none ([`held_key`]).
                    if let Some(key) = said.later_key() {
                        keys.insert(id, key.clone());
                        load.later.push((id, key.clone()));
                    }
                    load.unread.push(id);
                    unread.push((id, said));
                }
                None => load.failed.push(id),
            }
        }
        drop(keys);
        if unlinked {
            let _ = sync_dir(&self.records());
        }
        load.notices
            .extend(self.tell(&unread, &load.failed, writable));
        // Nothing is taken up from an older build while a record here
        // cannot be read for now, nor a pack of them opened: it may hold a
        // session the older build holds too. A later look takes it up, once
        // every record reads ([`Self::listing`]).
        if let Some(layout) = &layout
            && load.failed.is_empty()
            && !listing.skipped
        {
            let mut live = keyed(load.sessions.iter().map(|(id, session)| (*id, session)));
            for (id, key) in &load.later {
                live.entry(key.clone()).or_default().push(*id);
            }
            if let Some(change) = self.import(layout, &live) {
                taken_up(&mut load.sessions, &change);
            }
        }
        load.complete = layout.is_some();
        self.watch.looked(before);
        load
    }

    /// Make `change`, and say what became of it.
    ///
    /// Deletions undone come first, then the records, then the deletions,
    /// each step made durable before the next: a change cut short never
    /// loses a session, though it may leave one deleted that was being put
    /// back, or one beside what was filling it in. A restore of many at
    /// once is journaled while it runs, so the next to read the history can
    /// say it was cut short.
    ///
    /// What is said of it is what the history holds after it, as the app
    /// takes it: a change told of as failed deleted nothing, and one whose
    /// tombstones are made is told of as made, though a record they delete
    /// is left until the next read.
    pub(super) fn apply(&self, change: &HistoryChange) -> HistorySaved {
        self.on_its_thread();
        match self.gate() {
            Ok(Gate::Open(_) | Gate::Unmoved) => {}
            Ok(Gate::Newer) => return HistorySaved::Newer,
            Ok(Gate::Unread) => {
                return HistorySaved::Failed("how the history is kept could not be read.".into());
            }
            Err(err) => return saved_or_not(Err(err)),
        }
        let journal = if restores_many(change) {
            match self.begin_journal(change) {
                Ok(journal) => Some(journal),
                Err(err) => return saved_or_not(Err(err)),
            }
        } else {
            None
        };
        let before = self.watch.stamps();
        let made = self.make(change);
        // Made or not, the change is over, and what became of it is said
        // now: a journal left behind says only that Dust stopped first.
        if let Some(journal) = journal {
            end_journal(journal, &self.dir);
        }
        self.watch.made(before);
        saved_or_not(made.map(|()| change.put.len()))
    }

    fn make(&self, change: &HistoryChange) -> io::Result<()> {
        let (records, tombstones) = (self.records(), self.tombstones());
        for &id in &change.unforget {
            remove_if_there(&tombstones.join(file_name(id)))?;
        }
        if !change.unforget.is_empty() {
            sync_if_there(&tombstones)?;
        }
        let undone: HashSet<RecordId> = change.unforget.iter().copied().collect();
        for record in &change.put {
            let undoing = undone.contains(&record.id);
            // One deleted already stays deleted, unless this change undoes
            // that.
            if !undoing && tombstones.join(file_name(record.id)).exists() {
                continue;
            }
            // One put back whose record was never taken away, as a deletion
            // whose record would not go leaves it (below), is back as it
            // was stored, by its own name: not stored a second time beside
            // itself, where this build writes it otherwise.
            let id = if undoing && records.join(file_name(record.id)).exists() {
                record.id
            } else {
                self.place_record(record)?
            };
            if let Some(key) = key_of(&record.json) {
                self.keys.borrow_mut().insert(id, key);
            }
        }
        if !change.put.is_empty() {
            sync_if_there(&records)?;
        }
        if change.forget.is_empty() {
            return Ok(());
        }
        // The tombstones are the deletion: none of them stays unless all
        // of them do.
        let mut placed = Vec::new();
        let marked = (|| {
            for (id, deleted) in &change.forget {
                let name = file_name(*id);
                // One there already may hold what a later build added to
                // it.
                if !tombstones.join(&name).exists() {
                    place_whole(&tombstones, &name, &serde_json::to_vec(deleted)?)?;
                    placed.push(name);
                }
            }
            sync_dir(&tombstones)
        })();
        if let Err(err) = marked {
            for name in placed {
                let _ = fs::remove_file(tombstones.join(name));
            }
            let _ = sync_if_there(&tombstones);
            return Err(err);
        }
        // Made, from here: a record beside its tombstone reads as deleted,
        // and the next read takes it away ([`Self::load`]). So a record
        // that will not go now, as one held open elsewhere will not on
        // Windows, leaves the change made, and says so; were it told of as
        // failed, the app would show the session as kept, with nothing to
        // undo, until the next read took it away.
        for (id, _) in &change.forget {
            let path = records.join(file_name(*id));
            let held = fs::metadata(&path).map_or(0, |meta| meta.len());
            match remove_if_there(&path) {
                Ok(()) => self.shrunk(held),
                Err(err) => {
                    eprintln!("a deleted session's record stays until the next read: {err}");
                }
            }
            self.keys.borrow_mut().remove(id);
        }
        if let Err(err) = sync_if_there(&records) {
            eprintln!("could not make the deleted sessions' going durable: {err}");
        }
        Ok(())
    }

    /// Store `record` under its name, or, where other bytes have that name
    /// already, the next free one: no record is ever written over. The
    /// name it is stored under.
    fn place_record(&self, record: &Record) -> io::Result<RecordId> {
        place(&self.records(), record, &self.size)
    }

    /// `bytes` fewer stored, as a record goes: never fewer than none.
    fn shrunk(&self, bytes: u64) {
        self.size
            .update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                held.saturating_sub(bytes)
            });
    }

    /// Every record there is, deleted ones left out, once whatever an older
    /// build stored since has been taken up, where every record here can be
    /// read ([`Self::live_keys`]). None where the history cannot be listed.
    pub(super) fn listing(&self) -> Option<Vec<RecordId>> {
        self.on_its_thread();
        // As the folders were before they were looked at, the history kept
        // whole among them: whatever another copy changes from here on is
        // taken up at the next look.
        let before = self.watch.stamps();
        if let Ok(Gate::Open(layout)) = self.gate()
            && self.legacy_changed(&layout)
            && let Some(live) = self.live_keys()
        {
            self.import(&layout, &live);
        }
        let listing = self.listing_now().ok()?;
        self.watch.looked(before);
        let listed: Vec<RecordId> = listing.records.difference(&listing.gone).copied().collect();
        // Of what this build noted of records, those gone since: a listing
        // that skipped a pack it could not open does not say which are.
        if !listing.skipped {
            self.note_listed(&listed.iter().map(|id| id.name()).collect());
        }
        Some(listed)
    }

    /// Take out of this build's notes of records it could not read
    /// ([`FAILING`], [`UNREAD_TOLD`], [`DAMAGED_TOLD`]) each that `listed`,
    /// every record a look found, holds no more: it is gone, and should it
    /// come back and fail, that is a failure of its own, told of in turn.
    fn note_listed(&self, listed: &HashSet<String>) {
        for note in [FAILING, UNREAD_TOLD, DAMAGED_TOLD] {
            let Ok(raw) = fs::read(self.dir.join(note)) else {
                continue;
            };
            let gone: HashSet<String> = names_noted(Some(&raw))
                .into_iter()
                .filter(|name| !listed.contains(name))
                .collect();
            if !gone.is_empty() {
                self.note_no_more(note, &gone);
            }
        }
    }

    /// The records `ids`, as they are stored, each that can be read: one
    /// that cannot comes back as why ([`Unopened`]): it could not be read
    /// just now, or its bytes are no text, damaged, as the history read
    /// whole takes such a record ([`Raw::record`]). Each read fails no
    /// longer, and is told of no more ([`Self::note_no_more`]).
    ///
    /// One a later build packed once the packs were read, and took the
    /// loose copy of before it was read, is in neither place: every one of
    /// `ids` is read first, and the packs again only then, for all those in
    /// neither place ([`Self::raw_each`]).
    pub(super) fn read(&self, ids: &[RecordId]) -> Vec<Result<Record, Unopened>> {
        self.on_its_thread();
        let mut packed = self.packed().unwrap_or_default().records;
        let read: Vec<Result<Record, Unopened>> = self
            .raw_each(ids, &mut packed)
            .into_iter()
            .zip(ids)
            .map(|(raw, &id)| raw.record(id))
            .collect();
        let read_now: HashSet<String> = read
            .iter()
            .filter_map(|read| read.as_ref().ok().map(|record| record.id.name()))
            .collect();
        let damaged = read.iter().filter_map(|read| match read {
            Err(Unopened::Damaged(id)) => Some(id.name()),
            _ => None,
        });
        self.note_no_more(DAMAGED_TOLD, &read_now);
        let failing_no_longer: HashSet<String> = read_now.into_iter().chain(damaged).collect();
        self.note_no_more(FAILING, &failing_no_longer);
        self.note_no_more(UNREAD_TOLD, &failing_no_longer);
        read
    }

    /// Take `gone`, records by name, out of `note`, a note of records this
    /// build could not read: [`FAILING`], as those just read, or found
    /// damaged, fail no longer, so that one of them that fails for a moment
    /// again as the history is next read whole, at a later launch, is not
    /// told of then, as one that failed at the last read and has not read
    /// since is ([`Self::tell`]); [`UNREAD_TOLD`], as those it told of that
    /// read now are told of no more, so that one that fails again after, a
    /// failure of its own, is told of in turn ([`Self::still_unread`]); and
    /// [`DAMAGED_TOLD`], as those just read are damaged no more. Those a
    /// look no longer lists are gone, and go from each the same way
    /// ([`Self::note_listed`]). What a build before this one noted in
    /// [`TOLD`], the records it cannot read, is no failure this build noted,
    /// and stays as it is: dropped, that build would tell of it again as it
    /// next opens. Taken out only where this build may write to the
    /// history, as the note is made; a note that names none of them is left
    /// as it is, and one left naming none is removed, but for
    /// [`DAMAGED_TOLD`].
    fn note_no_more(&self, note: &str, gone: &HashSet<String>) {
        let path = self.dir.join(note);
        let Ok(raw) = fs::read(&path) else {
            return;
        };
        let noted = names_noted(Some(&raw));
        let left: BTreeSet<&String> = noted.iter().filter(|name| !gone.contains(*name)).collect();
        if left.len() == noted.len() || !matches!(self.gate(), Ok(Gate::Open(_) | Gate::Unmoved)) {
            return;
        }
        // The note of damage stays, though it names none: from its first
        // writing it alone holds back such a notice ([`DAMAGED_TOLD`]).
        let _ = if left.is_empty() && note != DAMAGED_TOLD {
            remove_if_there(&path)
        } else {
            serde_json::to_vec(&left)
                .map_err(io::Error::other)
                .and_then(|raw| write_whole(&self.dir, note, &raw))
        };
    }

    /// Write a backup to `out`: `backup`, with every record stored here,
    /// deleted ones left out, each as it is stored, in the order of the
    /// history ([`BackupFile::write`]). However long the history, no more
    /// of it is held than a part at a time ([`BACKUP_PART`]), beside what
    /// the packs hold: every record of a part is read, then the packs
    /// again for those in neither place, packed by a later build as the
    /// backup was written ([`Self::raw_each`]), and only then is the part
    /// written.
    ///
    /// A history that cannot be listed, or a record in it that cannot be
    /// read, is no reason to fail: the backup is wanted again holding what
    /// the app does ([`NotSaved::Unread`]), and then holds that. So is a
    /// pack that cannot be opened: what it holds is listed nowhere, and a
    /// backup of the rest would leave it out without a word.
    pub(super) fn stream_backup(
        &self,
        backup: &BackupFile,
        out: &mut dyn Write,
    ) -> Result<BackedUp, NotSaved> {
        self.on_its_thread();
        let listing = match self.listing_now() {
            Ok(listing) if !listing.skipped => listing,
            Ok(_) => {
                eprintln!("a pack of the history could not be read to back it up");
                return backup.write_unlisted(out);
            }
            Err(err) => {
                eprintln!("could not list the history to back it up: {err}");
                return backup.write_unlisted(out);
            }
        };
        let ids: Vec<RecordId> = listing.records.difference(&listing.gone).copied().collect();
        let mut packed = listing.packed;
        let stored = ids.chunks(BACKUP_PART).flat_map(|part| {
            let read = self.raw_each(part, &mut packed);
            part.iter()
                .zip(read)
                .map(|(&id, raw)| raw.text().map(|json| Record { id, json }).ok_or(id))
                .collect::<Vec<_>>()
        });
        backup.write(out, stored, |id| listing.gone.contains(&id))
    }

    /// What is stored for each of `ids`, in their order, loose or else in
    /// `packed`, the packs as last read ([`raw_at`]).
    ///
    /// A later build packs records by writing the pack before it takes
    /// their loose copies away, so one packed once `packed` was read, and
    /// whose loose copy went before it was read itself, is in neither
    /// place, but in the packs as they are after that. So every one of
    /// `ids` is read first, and only then the packs again, once, for all
    /// those in neither place, which are looked for again: one packed while
    /// they were read, in however many passes, is found. `packed` takes in
    /// what the packs hold by then.
    fn raw_each(&self, ids: &[RecordId], packed: &mut BTreeMap<RecordId, String>) -> Vec<Raw> {
        let records = self.records();
        let mut read: Vec<Raw> = ids.iter().map(|&id| raw_at(&records, id, packed)).collect();
        if read.iter().any(|raw| !raw.is_text()) {
            if let Ok(now) = self.packed() {
                packed.extend(now.records);
            }
            for (read, &id) in read.iter_mut().zip(ids) {
                if !read.is_text() {
                    *read = raw_at(&records, id, packed);
                }
            }
        }
        read
    }

    /// Which of `ids` were deleted by hand: those with a tombstone, each
    /// beside when it says the record was deleted, none for one that does
    /// not say, as an empty one, which deletes its record all the same.
    /// None where a tombstone could not be looked for, as where their
    /// folder will not open: that is no word that a record was not deleted.
    /// One with no tombstone was not: missing is never deleted.
    pub(super) fn deletions(&self, ids: &[RecordId]) -> Option<Deletions> {
        self.on_its_thread();
        let tombstones = self.tombstones();
        let mut found = Vec::new();
        for &id in ids {
            let tombstone = tombstones.join(file_name(id));
            match fs::symlink_metadata(&tombstone) {
                Ok(_) => {
                    let at = fs::read(&tombstone).ok().and_then(|raw| tombstone_at(&raw));
                    found.push((id, at));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return None,
            }
        }
        Some(found)
    }

    /// Which of `ids` were deleted by hand, each beside the session its
    /// tombstone remembers ([`Self::deletions`]): one whose tombstone does
    /// not say, or is there but does not read, is left out. None where a
    /// tombstone could not be looked for, as [`Self::deletions`] says. One
    /// that would not read gave no word at all, of any of `ids`.
    pub(super) fn deleted_sessions(&self, ids: &[RecordId]) -> Option<Vec<(RecordId, SessionKey)>> {
        self.on_its_thread();
        let tombstones = self.tombstones();
        let mut found = Vec::new();
        for &id in ids {
            let tombstone = tombstones.join(file_name(id));
            match fs::symlink_metadata(&tombstone) {
                Ok(_) => {
                    let key = fs::read(&tombstone)
                        .ok()
                        .and_then(|raw| tombstone_key(&raw));
                    found.extend(key.map(|key| (id, key)));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return None,
            }
        }
        Some(found)
    }

    /// Which of the sessions `keys` a tombstone remembers, whatever record
    /// it named ([`crate::persist::Store::deleted_keys`]): one that does not
    /// read, or names no session, is left out. None where the tombstones
    /// cannot be listed.
    pub(super) fn deleted_keys(&self, keys: &[SessionKey]) -> Option<Vec<SessionKey>> {
        self.on_its_thread();
        let tombstones = self.tombstones();
        let gone: HashSet<SessionKey> = ids_in(&tombstones)
            .ok()?
            .into_iter()
            .filter_map(|id| tombstone_key(&fs::read(tombstones.join(file_name(id))).ok()?))
            .collect();
        Some(
            keys.iter()
                .filter(|key| gone.contains(key))
                .cloned()
                .collect(),
        )
    }

    /// The gate `layout.json` sets. One that does not read is set aside
    /// and the move made again; one that cannot be set aside holds every
    /// write back.
    fn gate(&self) -> io::Result<Gate> {
        let path = self.dir.join(LAYOUT_FILE);
        match fs::read(&path) {
            Ok(raw) => match Gate::of(Some(&raw)) {
                Gate::Unread if move_aside(&self.dir, &path, "layout").is_some() => {
                    Ok(Gate::Unmoved)
                }
                gate => Ok(gate),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Gate::Unmoved),
            Err(err) => Err(err),
        }
    }

    /// Every record and tombstone there is. The loose records are listed
    /// before the packs: a later build packs records by writing the pack
    /// before it takes their loose copies away, so one packed meanwhile is
    /// found loose, or else in its pack, and never in neither.
    fn listing_now(&self) -> io::Result<Listing> {
        let mut records = ids_in(&self.records())?;
        self.meanwhile();
        let packs = self.packed()?;
        records.extend(packs.records.keys().copied());
        records.extend(packs.unopened.iter().copied());
        let gone = ids_in(&self.tombstones())?;
        self.meanwhile();
        Ok(Listing {
            records,
            gone,
            packed: packs.records,
            skipped: packs.skipped,
        })
    }

    /// Let another copy do what a test has it do while this one lists or
    /// reads the history, as a folder of it has been read: nothing, outside
    /// a test.
    fn meanwhile(&self) {
        #[cfg(test)]
        if let Some(elsewhere) = self.elsewhere.borrow_mut().as_mut() {
            elsewhere();
        }
    }

    /// Every record a later build packed together, and what each holds,
    /// as it was written. A line that does not read, or a pack cut short,
    /// gives up what it can, and a pack that cannot be opened, gone since
    /// its folder was read, say, gives up nothing, and is said to have been
    /// skipped: the rest are read all the same, and every pack is left as
    /// it is. A test's other copy acts once they are read, whatever they
    /// are read for ([`Self::meanwhile()`]).
    ///
    /// A pack skipped that opened before, held by another program for a
    /// moment, say, still holds the records it held then, for all this copy
    /// knows ([`Packs::unopened`]): a listing without them would have the
    /// app take each for lost in passing, and store it again beside it.
    fn packed(&self) -> io::Result<Packs> {
        let packs = self.read_packs();
        self.meanwhile();
        packs
    }

    /// What [`Self::packed`] hands back, read.
    fn read_packs(&self) -> io::Result<Packs> {
        let entries = match fs::read_dir(self.dir.join(PACKS)) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                self.pack_records.borrow_mut().clear();
                return Ok(Packs::default());
            }
            Err(err) => return Err(err),
        };
        let mut packs = Packs::default();
        let mut held = self.pack_records.borrow_mut();
        let mut now = HashMap::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            if !name.to_string_lossy().ends_with(PACK_EXTENSION) {
                continue;
            }
            let pack = match fs::File::open(entry.path()) {
                Ok(pack) => pack,
                Err(err) => {
                    eprintln!(
                        "could not read {} of the history: {err}",
                        name.to_string_lossy()
                    );
                    packs.skipped = true;
                    if let Some(ids) = held.remove(&name) {
                        packs.unopened.extend(ids.iter().copied());
                        now.insert(name, ids);
                    }
                    continue;
                }
            };
            let mut ids = Vec::new();
            let lines = io::BufReader::new(GzDecoder::new(pack)).lines();
            for line in lines.map_while(Result::ok) {
                let Ok(line) = serde_json::from_str::<PackLine>(&line) else {
                    continue;
                };
                if let Some(id) = RecordId::parse(&line.id) {
                    ids.push(id);
                    packs.records.entry(id).or_insert(line.s);
                }
            }
            now.insert(name, ids);
        }
        *held = now;
        Ok(packs)
    }

    /// Every record there is by its session's key: those read before as
    /// they were read, the rest read now for their keys alone. One that is
    /// damaged has none, though it may say one, as one whose bytes are no
    /// text has none: only one that reads, or that a later build stored,
    /// holds its session ([`held_key`]). None where the history cannot be
    /// listed, or a record in it cannot be read for now, or a pack of them
    /// opened: it may hold a session an older build holds too, which would
    /// be taken up a second time beside it.
    fn live_keys(&self) -> Option<HashMap<SessionKey, Vec<RecordId>>> {
        let listing = self.listing_now().ok()?;
        if listing.skipped {
            return None;
        }
        let records = self.records();
        let mut keys = self.keys.borrow_mut();
        let mut live: HashMap<SessionKey, Vec<RecordId>> = HashMap::new();
        for id in listing.records.difference(&listing.gone) {
            let key = match keys.get(id) {
                Some(key) => key.clone(),
                None => match raw_at(&records, *id, &listing.packed) {
                    Raw::Text(raw) => {
                        let Some(key) = held_key(&raw) else {
                            continue;
                        };
                        keys.insert(*id, key.clone());
                        key
                    }
                    Raw::NoText(_) => continue,
                    Raw::Failed => return None,
                },
            };
            live.entry(key).or_default().push(*id);
        }
        Some(live)
    }

    /// Whether the history kept whole changed since it was last taken up.
    fn legacy_changed(&self, layout: &Layout) -> bool {
        fingerprint_of(&self.root.join(SESSIONS)).is_ok_and(|now| Some(now) != layout.taken_up())
    }

    /// Make the move from the history kept whole, `sessions.json`, and say
    /// how the history is kept now, or why the move cannot be made yet.
    ///
    /// What does not read is set aside first, as a history kept whole
    /// always was: a file that is no list is moved out of the way, and the
    /// sessions that do not read are copied out beside it. Every session
    /// that reads becomes a record, and the layout is written last, so a
    /// move cut short is made again, to the same records. The file itself
    /// is never written: older builds keep their history in it.
    fn migrate(&self, load: &mut HistoryLoad) -> Result<Layout, String> {
        let path = self.root.join(SESSIONS);
        let (raw, mut fingerprint) = match read_with_fingerprint(&path) {
            Ok(Some((raw, fingerprint))) => (Some(raw), Some(fingerprint)),
            Ok(None) => (None, None),
            Err(err) => return Err(err.to_string()),
        };
        let read = raw
            .as_deref()
            .map(|raw| std::str::from_utf8(raw).ok().and_then(read_history));
        let sessions = match read {
            None => Vec::new(),
            Some(None) => {
                let moved = move_aside(&self.root, &path, "sessions");
                if moved.is_some() {
                    fingerprint = None;
                }
                let kept = moved.as_deref().unwrap_or(SESSIONS);
                load.set_aside.push(SetAside::History {
                    kept: shown(&self.root, kept),
                });
                Vec::new()
            }
            Some(Some(read)) => {
                if let Some(unread) = read.unread_json() {
                    let copied = copy_aside(&self.root, unread.as_bytes())
                        .map_err(|err| format!("the sessions that do not read: {err}"))?;
                    if let Some(name) = copied {
                        load.set_aside.push(SetAside::Sessions {
                            count: read.unread.len(),
                            kept: shown(&self.root, &name),
                        });
                    }
                }
                read.sessions
            }
        };
        let deleted = legacy_deleted(&self.root);
        let (records, layout) =
            legacy::moved(&sessions, fingerprint, &deleted, crate::time::now_ms());
        let moving = self.not_moved_yet(records).map_err(|err| err.to_string())?;
        self.progress.moving(Some(moving.len() as u64));
        place_each(&self.records(), &moving, &self.size, &self.progress)
            .map_err(|err| err.to_string())?;
        sync_if_there(&self.records()).map_err(|err| err.to_string())?;
        layout
            .written()
            .map_err(io::Error::other)
            .and_then(|raw| write_whole(&self.dir, LAYOUT_FILE, &raw))
            .map_err(|err| err.to_string())?;
        Ok(layout)
    }

    /// Take up what an older build stored in, or deleted from, the history
    /// kept whole since it was last taken up, `live` being every record
    /// here by key ([`legacy::imported`]), and hand back what was made of
    /// it. Nothing, where nothing changed there, or what changed cannot be
    /// read or made now: it is tried again the next time.
    ///
    /// It is no restore, and is not journaled: what was taken up is noted
    /// only once it is stored, so a take-up cut short is made again by the
    /// next read, and nobody need be told.
    fn import(
        &self,
        layout: &Layout,
        live: &HashMap<SessionKey, Vec<RecordId>>,
    ) -> Option<HistoryChange> {
        if !self.legacy_changed(layout) {
            return None;
        }
        let (sessions, fingerprint) = match read_with_fingerprint(&self.root.join(SESSIONS)) {
            Ok(Some((raw, fingerprint))) => {
                let read = std::str::from_utf8(&raw).ok().and_then(read_history);
                (
                    read.map(|read| read.sessions).unwrap_or_default(),
                    Some(fingerprint),
                )
            }
            Ok(None) => (Vec::new(), None),
            Err(err) => {
                eprintln!("could not read the history an older build keeps: {err}");
                return None;
            }
        };
        let deleted = legacy_deleted(&self.root);
        let gone = self.tombstoned_keys();
        let (change, next) =
            legacy::imported(layout, fingerprint, &sessions, &deleted, live, &gone)?;
        if !change.is_empty() && self.apply(&change).kept().is_none() {
            return None;
        }
        let noted = next
            .written()
            .map_err(io::Error::other)
            .and_then(|raw| write_whole(&self.dir, LAYOUT_FILE, &raw));
        if let Err(err) = noted {
            eprintln!("could not note what was taken up of an older build's history: {err}");
        }
        Some(change)
    }

    /// Of `moved`, the records a move makes, those to write: all of them
    /// on a first move. A move made again, as one whose layout no longer
    /// reads is, finds records already here: a session deleted since stays
    /// deleted, and one taken up from an older build since, under the name
    /// it was taken up by, is not written a second time under another.
    fn not_moved_yet(&self, moved: Vec<Record>) -> io::Result<Vec<Record>> {
        let (here, gone) = (ids_in(&self.records())?, ids_in(&self.tombstones())?);
        if here.is_empty() && gone.is_empty() {
            return Ok(moved);
        }
        let gone_keys = self.tombstoned_keys();
        let names: BTreeSet<RecordId> = moved.iter().map(|record| record.id).collect();
        // What each record here under a name the move does not give holds:
        // a session that needs no record of the move's.
        let elsewhere: Vec<String> = here
            .difference(&names)
            .filter_map(|id| fs::read_to_string(self.records().join(file_name(*id))).ok())
            .collect();
        Ok(legacy::not_moved_yet(
            moved,
            |id| here.contains(&id),
            &gone,
            elsewhere,
            &gone_keys,
        ))
    }

    /// The key of every session deleted here that its tombstone names.
    fn tombstoned_keys(&self) -> HashSet<SessionKey> {
        let Ok(gone) = ids_in(&self.tombstones()) else {
            return HashSet::new();
        };
        gone.into_iter()
            .filter_map(|id| {
                let raw = fs::read(self.tombstones().join(file_name(id))).ok()?;
                tombstone_key(&raw)
            })
            .collect()
    }

    /// What to tell of `unread`, the records this build cannot read, each
    /// with what it says of itself, and of `failed`, those that could not
    /// be read at all just now ([`not_told`]): of the second, only those
    /// that could not be read as the history was last read either, and have
    /// not read since ([`FAILING`]), as a moment's failure is over by the
    /// app's first try at reading it again; and nothing, once each has been
    /// told of.
    /// `writable` is whether what was told may be noted, so it is told of
    /// once, and what failed, for the next read to tell of.
    ///
    /// Each kind of notice goes by a note of its own. One of a record this
    /// build cannot read, by [`DAMAGED_TOLD`], which keeps each of those it
    /// cannot read now and each it told of that fails now, and drops the
    /// rest, as read or gone; until it is first written, [`TOLD`] stands
    /// for it. One of a failure, by [`UNREAD_TOLD`], which keeps each told
    /// of before, by this launch or by the app's tries
    /// ([`Self::still_unread`]), that still fails, and drops one that reads
    /// now or is gone: should it fail again, that is a failure of its own,
    /// told of in turn. [`TOLD`], where a build before formats looks, only
    /// gains names: every one told of, of either kind, beside what such a
    /// build noted there, the records it could not open or make out, some
    /// of which this one reads; dropped, such a build would tell of one
    /// again as it next opened. A look drops from this build's notes the
    /// records it reads, and those it no longer lists ([`Self::read`],
    /// [`Self::listing`]).
    fn tell(
        &self,
        unread: &[(RecordId, Unreadable)],
        failed: &[RecordId],
        writable: bool,
    ) -> Vec<HistoryNotice> {
        let failing = self.dir.join(FAILING);
        let failed_before = names_noted(fs::read(&failing).ok().as_deref());
        let unread_now: Vec<(RecordId, Option<Unreadable>)> = unread
            .iter()
            .map(|(id, said)| (*id, Some(said.clone())))
            .collect();
        let failed_now: Vec<(RecordId, Option<Unreadable>)> = failed
            .iter()
            .filter(|id| failed_before.contains(&id.name()))
            .map(|id| (*id, None))
            .collect();
        let path = self.dir.join(TOLD);
        let told = fs::read(&path).ok();
        let unread_told = fs::read(self.dir.join(UNREAD_TOLD)).ok();
        let damaged_told = fs::read(self.dir.join(DAMAGED_TOLD)).ok();
        // Of a record this build cannot read, and of a failure, as this
        // build told of it, whatever a build before this one noted of the
        // record ([`DAMAGED_TOLD`], [`UNREAD_TOLD`]); of the first, as such
        // a build noted it, until this build first notes one.
        let mut notices = not_told(&unread_now, damaged_told.as_deref().or(told.as_deref()));
        notices.extend(not_told(&failed_now, unread_told.as_deref()));
        if !writable {
            return notices;
        }
        // What it noted, with what it tells of now, whether or not there is
        // anything new to tell: what a build before this one noted there
        // stays. A failure told of, now or before, is noted there too,
        // where such a build looks, so that it does not tell of it again.
        let told_before = names_noted(told.as_deref());
        let names: BTreeSet<String> = told_before
            .iter()
            .cloned()
            .chain(unread_now.iter().map(|(id, _)| id.name()))
            .chain(failed_now.iter().map(|(id, _)| id.name()))
            .collect();
        let told_more = names.len() != told_before.len();
        if told_more {
            let _ = serde_json::to_vec(&names)
                .map_err(io::Error::other)
                .and_then(|raw| write_whole(&self.dir, TOLD, &raw));
        }
        // Of the failures told of, before or now, those that still fail.
        let unread_told_before = names_noted(unread_told.as_deref());
        let failing_now: HashSet<String> = failed.iter().map(|id| id.name()).collect();
        let still_told: BTreeSet<String> = unread_told_before
            .iter()
            .filter(|name| failing_now.contains(*name))
            .cloned()
            .chain(failed_now.iter().map(|(id, _)| id.name()))
            .collect();
        if still_told.len() != unread_told_before.len()
            || still_told
                .iter()
                .any(|name| !unread_told_before.contains(name))
        {
            let _ = if still_told.is_empty() {
                remove_if_there(&self.dir.join(UNREAD_TOLD))
            } else {
                serde_json::to_vec(&still_told)
                    .map_err(io::Error::other)
                    .and_then(|raw| write_whole(&self.dir, UNREAD_TOLD, &raw))
            };
        }
        // Of the records this build cannot read, every one it cannot read
        // now, told of now or before, and every one it told of that fails
        // now: one that reads, or is gone, is told of no more. Written the
        // first time where told.json is there, or this build writes it,
        // though it names none, so that told.json stands for it no more:
        // what this build notes there is no build before's.
        let damaged_before = names_noted(damaged_told.as_deref());
        let damaged_now: BTreeSet<String> = unread_now
            .iter()
            .map(|(id, _)| id.name())
            .chain(
                damaged_before
                    .iter()
                    .filter(|name| failing_now.contains(*name))
                    .cloned(),
            )
            .collect();
        let changed = damaged_now.len() != damaged_before.len()
            || damaged_now
                .iter()
                .any(|name| !damaged_before.contains(name));
        let first = damaged_told.is_none() && (told.is_some() || told_more);
        if first || changed {
            let _ = serde_json::to_vec(&damaged_now)
                .map_err(io::Error::other)
                .and_then(|raw| write_whole(&self.dir, DAMAGED_TOLD, &raw));
        }
        if failing_now != failed_before {
            let _ = if failing_now.is_empty() {
                remove_if_there(&failing)
            } else {
                let names: BTreeSet<&String> = failing_now.iter().collect();
                serde_json::to_vec(&names)
                    .map_err(io::Error::other)
                    .and_then(|raw| write_whole(&self.dir, FAILING, &raw))
            };
        }
        notices
    }

    /// What to tell of `ids`, records listed that could not be read at all
    /// at any of the app's tries since it found them so: of those this build
    /// has not told of before ([`UNREAD_TOLD`]), how many
    /// ([`HistoryNotice::Unread`]), each noted as told, where this build may
    /// write, so it is told of once ([`super::Store::still_unread`]), and in
    /// [`TOLD`] too, so that a build before formats does not tell of it
    /// again. What a build before this one noted of one in [`TOLD`] holds
    /// back nothing: such a build notes there every record it could not
    /// open, a moment's failure among them, and one that has read since
    /// fails now on its own.
    pub(super) fn still_unread(&self, ids: &[RecordId]) -> Vec<HistoryNotice> {
        self.on_its_thread();
        let told = fs::read(self.dir.join(UNREAD_TOLD)).ok();
        let still: Vec<(RecordId, Option<Unreadable>)> = ids.iter().map(|id| (*id, None)).collect();
        let notices = not_told(&still, told.as_deref());
        let writable = matches!(self.gate(), Ok(Gate::Open(_) | Gate::Unmoved));
        if !notices.is_empty() && writable {
            let mut names: BTreeSet<String> = names_noted(told.as_deref()).into_iter().collect();
            names.extend(ids.iter().map(|id| id.name()));
            if let Ok(raw) = serde_json::to_vec(&names) {
                let _ = write_whole(&self.dir, UNREAD_TOLD, &raw);
            }
            // Noted where a build before formats looks too ([`TOLD`]), so
            // such a build does not tell of it a second time.
            let mut older: BTreeSet<String> =
                names_noted(fs::read(self.dir.join(TOLD)).ok().as_deref())
                    .into_iter()
                    .collect();
            let held = older.len();
            older.extend(ids.iter().map(|id| id.name()));
            if older.len() != held
                && let Ok(raw) = serde_json::to_vec(&older)
            {
                let _ = write_whole(&self.dir, TOLD, &raw);
                // What this build notes there stands for no note of damage
                // of its own ([`DAMAGED_TOLD`]): from here on that note does.
                if !self.dir.join(DAMAGED_TOLD).exists() {
                    let _ = write_whole(&self.dir, DAMAGED_TOLD, b"[]");
                }
            }
            // Noted failing as well, as this build's failures: told of no
            // more once they read ([`Self::note_no_more`], [`Self::tell`]).
            let failing = self.dir.join(FAILING);
            let mut failing_now: BTreeSet<String> = names_noted(fs::read(&failing).ok().as_deref())
                .into_iter()
                .collect();
            let before = failing_now.len();
            failing_now.extend(ids.iter().map(|id| id.name()));
            if failing_now.len() != before
                && let Ok(raw) = serde_json::to_vec(&failing_now)
            {
                let _ = write_whole(&self.dir, FAILING, &raw);
            }
        }
        notices
    }

    /// Clear what writes cut short left behind: temporary files no write
    /// still running could own.
    fn clear_temporaries(&self) {
        let now = SystemTime::now();
        let packs = self.dir.join(PACKS);
        for dir in [self.dir.clone(), self.records(), self.tombstones(), packs] {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let old = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age > OLD_TEMPORARY));
                if name.starts_with('.') && name.ends_with(".tmp") && old {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    /// Start `change`, a restore of many sessions at once: its journal,
    /// locked.
    fn begin_journal(&self, change: &HistoryChange) -> io::Result<Journal> {
        fs::create_dir_all(&self.dir)?;
        let name = format!("{JOURNAL_PREFIX}{:016x}.json", crate::time::seed_rng());
        // A temporary name, as any write has, cleared like one if the
        // change stops before the journal is placed.
        let making = self.dir.join(format!(".{name}.tmp"));
        let mut lock = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&making)?;
        let journaled = Journaled {
            ids: change.put.iter().map(|record| record.id.name()).collect(),
            undone: change.unforget.iter().map(|id| id.name()).collect(),
        };
        let placed = (|| {
            lock.try_lock().map_err(io::Error::other)?;
            lock.write_all(&serde_json::to_vec(&journaled)?)?;
            flush(&lock)?;
            let path = self.dir.join(&name);
            fs::rename(&making, &path)?;
            sync_dir(&self.dir)?;
            Ok(path)
        })();
        match placed {
            Ok(path) => Ok(Journal { path, _lock: lock }),
            Err(err) => {
                let _ = fs::remove_file(&making);
                Err(err)
            }
        }
    }

    /// Every restore of many sessions at once that was cut short, told of
    /// once: its journal is there, and nothing holds it.
    fn journals_cut_short(&self) -> Vec<HistoryNotice> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut notices = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with(JOURNAL_PREFIX) && name.ends_with(".json")) {
                continue;
            }
            let Ok(mut file) = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(entry.path())
            else {
                continue;
            };
            // Held: another copy is making its change now.
            if file.try_lock().is_err() {
                continue;
            }
            let mut raw = Vec::new();
            let journaled: Journaled = file
                .read_to_end(&mut raw)
                .ok()
                .and_then(|_| serde_json::from_slice(&raw).ok())
                .unwrap_or_default();
            let meant: Vec<RecordId> = journaled
                .ids
                .iter()
                .filter_map(|name| RecordId::parse(name))
                .collect();
            let undone: HashSet<RecordId> = journaled
                .undone
                .iter()
                .filter_map(|name| RecordId::parse(name))
                .collect();
            // Stored, or deleted by hand and not to be put back, which the
            // change leaves deleted as it is.
            let stored = meant
                .iter()
                .filter(|id| {
                    self.records().join(file_name(**id)).exists()
                        || (!undone.contains(id)
                            && self.tombstones().join(file_name(**id)).exists())
                })
                .count();
            if stored < meant.len() {
                notices.push(HistoryNotice::CutShort {
                    meant: meant.len(),
                    stored,
                });
            }
            let _ = fs::remove_file(entry.path());
        }
        if !notices.is_empty() {
            let _ = sync_dir(&self.dir);
        }
        notices
    }
}

impl Watch {
    /// Whether anything changed since this copy last looked, here or in
    /// the history kept whole. Nothing is noted: a change is told of each
    /// time it is asked, until a look that lists the history takes it up
    /// ([`Self::looked`]), so one whose look could not list it is told of
    /// again.
    pub(super) fn changed(&self) -> bool {
        let now = self.stamps();
        *self.seen() != Some(now)
    }

    /// Note the folders as `before` found them, just before this copy
    /// looked at what they hold: whatever changed after, as another copy
    /// stored while this one read, is still to be taken up.
    fn looked(&self, before: Stamps) {
        *self.seen() = Some(before);
    }

    /// Note the folders as this copy's own change left them, `before` being
    /// how they were just before it was made: what it did is no change for
    /// it to take up. Where they had changed since it last looked, as
    /// another copy changed them, nothing is noted, and what the other did
    /// is taken up at the next look, with this change beside it.
    ///
    /// The folders are looked at before what was seen is taken: the app's
    /// own thread takes it to ask whether anything changed, and waits on
    /// no disk for it. Only the thread that reads and writes the files
    /// notes anything, so what was seen is as it was when they were looked
    /// at.
    fn made(&self, before: Stamps) {
        let after = self.stamps();
        let mut seen = self.seen();
        if *seen == Some(before) {
            *seen = Some(after);
        }
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Option<Stamps>> {
        // What was seen is a value whole, whatever stopped while it was
        // held: at worst a change is taken up once more.
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stamps(&self) -> Stamps {
        let modified = |dir: &Path| fs::metadata(dir).and_then(|meta| meta.modified()).ok();
        Stamps {
            records: modified(&self.dir.join(RECORDS)),
            packs: modified(&self.dir.join(PACKS)),
            tombstones: modified(&self.dir.join(TOMBSTONES)),
            legacy: fingerprint_of(&self.root.join(SESSIONS)).ok().flatten(),
        }
    }
}

/// What is stored of a record ([`raw_at`]).
enum Raw {
    /// Its text.
    Text(String),
    /// So many bytes that are no text, as damage to a disk leaves them: no
    /// build reads them, now or later.
    NoText(u64),
    /// Nothing, for now: it could not be read just then.
    Failed,
}

impl Raw {
    /// What is stored, where it is text.
    fn text(self) -> Option<String> {
        match self {
            Self::Text(raw) => Some(raw),
            Self::NoText(_) | Self::Failed => None,
        }
    }

    fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// What a read of record `id` hands back of what is stored
    /// ([`HistoryFiles::read`]): bytes that are no text are a damaged
    /// record's, as the history read whole takes them ([`read_one`]).
    fn record(self, id: RecordId) -> Result<Record, Unopened> {
        match self {
            Self::Text(json) => Ok(Record { id, json }),
            Self::NoText(_) => Err(Unopened::Damaged(id)),
            Self::Failed => Err(Unopened::Failed(id)),
        }
    }
}

/// What is stored for `id` in `records`, loose, or else in a pack: a loose
/// copy that cannot be read, or whose bytes are no text, is looked for
/// there.
fn raw_at(records: &Path, id: RecordId, packed: &BTreeMap<RecordId, String>) -> Raw {
    let in_a_pack = || packed.get(&id).cloned().map(Raw::Text);
    match fs::read(records.join(file_name(id))) {
        Ok(bytes) => String::from_utf8(bytes).map_or_else(
            |bytes| in_a_pack().unwrap_or(Raw::NoText(bytes.as_bytes().len() as u64)),
            Raw::Text,
        ),
        Err(_) => in_a_pack().unwrap_or(Raw::Failed),
    }
}

/// A record as it is read ([`read_one`]): its session, or where it holds
/// none this build reads, what its text says of it ([`unreadable`]), beside
/// how many bytes are stored of it.
type ReadOne = (Result<SessionResult, Unreadable>, u64);

/// Record `id`, read from `records` or else from `packed`, and scored as it
/// reads ([`read_record`]), beside how many bytes are stored of it: none
/// where it could not be read just then, and what it says of itself where
/// it holds no session that reads. Bytes that are no text say nothing:
/// damaged.
fn read_one(records: &Path, id: RecordId, packed: &BTreeMap<RecordId, String>) -> Option<ReadOne> {
    match raw_at(records, id, packed) {
        Raw::Text(raw) => {
            let read = read_record(&raw).ok_or_else(|| unreadable(&raw));
            Some((read, raw.len() as u64))
        }
        Raw::NoText(bytes) => Some((Err(Unreadable::default()), bytes)),
        Raw::Failed => None,
    }
}

/// Each of `ids`, read as [`read_one`] reads it, in their order, each
/// counted in `progress` once read. A long history is read on as many as
/// [`READERS`] threads at once, a part each, as it is read whole at every
/// launch.
fn read_each(
    records: &Path,
    ids: &[RecordId],
    packed: &BTreeMap<RecordId, String>,
    progress: &LoadProgress,
) -> Vec<Option<ReadOne>> {
    let one = |id: &RecordId| {
        let read = read_one(records, *id, packed);
        progress.step();
        read
    };
    let threads = thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .min(READERS);
    if threads < 2 || ids.len() < READ_APART_FROM {
        return ids.iter().map(one).collect();
    }
    let part = ids.len().div_ceil(threads);
    thread::scope(|scope| {
        let reading: Vec<_> = ids
            .chunks(part)
            .map(|ids| {
                let started = thread::Builder::new()
                    .name("dust-history-read".into())
                    .spawn_scoped(scope, move || ids.iter().map(one).collect::<Vec<_>>());
                (ids, started)
            })
            .collect();
        // A part whose thread would not start, or stopped short, is read
        // here instead.
        reading
            .into_iter()
            .flat_map(|(ids, started)| {
                started
                    .ok()
                    .and_then(|reader| reader.join().ok())
                    .unwrap_or_else(|| ids.iter().map(one).collect())
            })
            .collect()
    })
}

/// Store `record` in `records` under its name, or, where other bytes have
/// that name already, the next free one: no record is ever written over.
/// The name it is stored under, and its bytes counted in `size` where they
/// were written.
fn place(records: &Path, record: &Record, size: &AtomicU64) -> io::Result<RecordId> {
    let mut id = record.id;
    for _ in 0..BUMPS {
        let name = file_name(id);
        match fs::read(records.join(&name)) {
            Ok(held) if held == record.json.as_bytes() => return Ok(id),
            Ok(_) => id = id.bumped(),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                place_whole(records, &name, record.json.as_bytes())?;
                size.fetch_add(record.json.len() as u64, Ordering::Relaxed);
                return Ok(id);
            }
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other(format!(
        "no name was free for the session stored as {}",
        record.id
    )))
}

/// Store each of `records` in `dir` ([`place`]), each counted in `progress`
/// once stored, on as many as [`WRITERS`] threads at once, a part each, as
/// the move to records stores a whole history while the app waits on it,
/// its boot screen up: one after another, each waiting for its own flush of
/// the disk, three thousand sessions kept it waiting for seconds, on a slow
/// disk for a minute.
///
/// Each record is flushed before it takes its name, as ever, so a move
/// cut short leaves every record whole or not there at all, in whatever
/// order they were stored, and is made again to the same records before
/// its layout is written. Each record has a place of its own, so no two
/// parts ever reach for the same name.
fn place_each(
    dir: &Path,
    records: &[Record],
    size: &AtomicU64,
    progress: &LoadProgress,
) -> io::Result<()> {
    let one = |record: &Record| place(dir, record, size).map(|_| progress.step());
    if records.len() < 2 {
        return records.iter().try_for_each(one);
    }
    let part = records.len().div_ceil(WRITERS);
    thread::scope(|scope| {
        let placing: Vec<_> = records
            .chunks(part)
            .map(|records| {
                let started = thread::Builder::new()
                    .name("dust-history-write".into())
                    .spawn_scoped(scope, move || records.iter().try_for_each(one));
                (records, started)
            })
            .collect();
        // A part whose thread would not start, or stopped short, is stored
        // here instead: what it stored already is found stored.
        placing.into_iter().try_for_each(|(records, started)| {
            started
                .ok()
                .and_then(|writer| writer.join().ok())
                .unwrap_or_else(|| records.iter().try_for_each(one))
        })
    })
}

/// Whether `change` restores many sessions at once, and so is journaled
/// while it runs ([`HistoryChange::restore`]).
fn restores_many(change: &HistoryChange) -> bool {
    change.restore && change.put.len() > 1
}

/// The change over: its journal gone, and let go of.
fn end_journal(journal: Journal, dir: &Path) {
    let _ = fs::remove_file(&journal.path);
    let _ = sync_dir(dir);
}

/// What became of a change, in the store's words: stored, `put` sessions
/// of it, or why not. A full disk is a full store.
fn saved_or_not(made: io::Result<usize>) -> HistorySaved {
    match made {
        Ok(put) => HistorySaved::Stored { kept: put },
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
            ) =>
        {
            HistorySaved::Full
        }
        Err(err) => HistorySaved::Failed(err.to_string()),
    }
}

/// The file record `id` is kept in.
fn file_name(id: RecordId) -> String {
    format!("{}.json", id.name())
}

/// Every record named in `dir`, in order: none where there is no `dir`
/// yet. Anything else in it is left alone.
fn ids_in(dir: &Path) -> io::Result<BTreeSet<RecordId>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(err) => return Err(err),
    };
    let mut ids = BTreeSet::new();
    for entry in entries {
        let name = entry?.file_name();
        if let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(RecordId::parse)
        {
            ids.insert(id);
        }
    }
    Ok(ids)
}

/// Remove `path`, which may be gone already.
fn remove_if_there(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Make what was done in `dir` durable, where there is a `dir`.
fn sync_if_there(dir: &Path) -> io::Result<()> {
    match sync_dir(dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// What tells the file at `path` as it stands from another state of it:
/// none, where there is no file.
fn fingerprint_of(path: &Path) -> io::Result<Option<Fingerprint>> {
    match fs::metadata(path) {
        Ok(meta) => Ok(Some(fingerprint(&meta))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn fingerprint(meta: &fs::Metadata) -> Fingerprint {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|at| at.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    Fingerprint {
        len: meta.len(),
        mtime,
    }
}

/// The file at `path`, and what told it apart as it was read: none, where
/// there is no file.
fn read_with_fingerprint(path: &Path) -> io::Result<Option<(Vec<u8>, Fingerprint)>> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let meta = file.metadata()?;
    let mut raw = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.read_to_end(&mut raw)?;
    Ok(Some((raw, fingerprint(&meta))))
}

/// The deletions an older build remembers: none, where they cannot be
/// read, which costs a session it deleted staying here, never one lost.
fn legacy_deleted(root: &Path) -> Vec<Deleted> {
    legacy::deleted_of(fs::read(root.join(DELETED)).ok().as_deref())
}

/// Keep `unread`, sessions of the history in `root` that could not be
/// read, in a file of their own beside it, and say what it is called:
/// nothing, where a file set aside before holds them already.
pub(super) fn copy_aside(root: &Path, unread: &[u8]) -> io::Result<Option<String>> {
    let kept = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("sessions.unreadable-")
                && fs::read(entry.path()).is_ok_and(|kept| kept == unread)
        });
    if kept {
        return Ok(None);
    }
    let name = aside_name(root, "sessions");
    write_whole(root, &name, unread)?;
    Ok(Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::desktop::tests::TempDir;
    use crate::persist::record::{records_for_blob, scored};
    use crate::persist::tests::{as_an_older_build_wrote, session};
    use crate::persist::worker::{HistoryJobs, wait};
    use std::cell::Cell;

    const DAY: u64 = 86_400_000;

    thread_local! {
        /// This test reaches the files through a worker of their own, as
        /// the app does ([`on_the_worker`]), rather than then and there.
        static ON_WORKER: Cell<bool> = const { Cell::new(false) };
    }

    /// The history in a folder, as this test reaches it: then and there,
    /// or through a worker of its own ([`ON_WORKER`]), each answer waited
    /// for.
    struct Files(HistoryJobs);

    impl Files {
        fn at(root: &Path) -> Self {
            let files = HistoryFiles::at(root);
            Self(if ON_WORKER.get() {
                HistoryJobs::on_worker(files)
            } else {
                HistoryJobs::inline(files)
            })
        }

        fn load(&self) -> HistoryLoad {
            self.0.load()
        }

        fn apply(&self, change: &HistoryChange) -> HistorySaved {
            wait(self.0.apply(change.clone()))
        }

        fn listing(&self) -> Option<Vec<RecordId>> {
            wait(self.0.listing())
        }

        fn read(&self, ids: &[RecordId]) -> Vec<Result<Record, Unopened>> {
            wait(self.0.read(ids.to_vec()))
        }

        fn deletions(&self, ids: &[RecordId]) -> Option<Deletions> {
            wait(self.0.deletions(ids.to_vec()))
        }

        fn deleted_sessions(&self, ids: &[RecordId]) -> Option<Vec<(RecordId, SessionKey)>> {
            wait(self.0.deleted_sessions(ids.to_vec()))
        }

        fn deleted_keys(&self, keys: &[SessionKey]) -> Option<Vec<SessionKey>> {
            wait(self.0.deleted_keys(keys.to_vec()))
        }

        /// Which of `ids` were deleted by hand: none, where that could not
        /// be looked for, as a store answers sync ([`Store::forgotten`]).
        ///
        /// [`Store::forgotten`]: crate::persist::Store::forgotten
        fn forgotten(&self, ids: &[RecordId]) -> Vec<RecordId> {
            let found = self.deletions(ids).unwrap_or_default();
            found.into_iter().map(|(id, _)| id).collect()
        }

        fn still_unread(&self, ids: &[RecordId]) -> Vec<HistoryNotice> {
            wait(self.0.still_unread(ids.to_vec()))
        }

        fn changed(&self) -> bool {
            self.0.changed()
        }

        fn bytes(&self) -> u64 {
            self.0.bytes()
        }

        /// Have `elsewhere` done each time a folder of the history has been
        /// read, from the next look on, as another copy of the app would do
        /// it meanwhile ([`HistoryFiles::meanwhile()`]).
        fn meanwhile(&self, elsewhere: impl FnMut() + Send + 'static) {
            let elsewhere: Box<dyn FnMut() + Send> = Box::new(elsewhere);
            wait(
                self.0
                    .ask(move |files| *files.elsewhere.borrow_mut() = Some(elsewhere)),
            );
        }
    }

    /// A history in a scratch folder of its own, as this test reaches it.
    fn history(name: &str) -> (TempDir, Files) {
        let dir = TempDir::new(name);
        let files = Files::at(&dir.0);
        (dir, files)
    }

    /// A history in a scratch folder of its own, its insides reached then
    /// and there.
    fn raw_history(name: &str) -> (TempDir, HistoryFiles) {
        let dir = TempDir::new(name);
        let files = HistoryFiles::at(&dir.0);
        (dir, files)
    }

    /// A session finished at `at`, on a day of its own, scored as this
    /// build scores it, as every session it stores is.
    fn finished(at: u64) -> SessionResult {
        scored(SessionResult {
            timestamp: at,
            ..session(&format!("day {at}"))
        })
    }

    /// `session` as a session just finished here stores it.
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

    /// `records` restored from a backup.
    fn restore(records: &[Record]) -> HistoryChange {
        HistoryChange {
            restore: true,
            ..put(records)
        }
    }

    /// The journals in the history's folder in `dir`.
    fn journals(dir: &TempDir) -> Vec<String> {
        names(&dir.0.join(HISTORY))
            .into_iter()
            .filter(|name| name.contains(JOURNAL_PREFIX))
            .collect()
    }

    fn forget(records: &[(&Record, &SessionResult)]) -> HistoryChange {
        HistoryChange {
            forget: records
                .iter()
                .map(|(record, session)| (record.id, Deleted::of(session, 7)))
                .collect(),
            ..HistoryChange::default()
        }
    }

    /// The names in `dir`, in order: none where there is no `dir`.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn stamps(load: &HistoryLoad) -> Vec<u64> {
        load.sessions
            .iter()
            .map(|(_, session)| session.timestamp)
            .collect()
    }

    fn ids(load: &HistoryLoad) -> Vec<RecordId> {
        load.sessions.iter().map(|(id, _)| *id).collect()
    }

    /// Write the history an older build keeps whole in `dir`.
    fn keep_whole(dir: &TempDir, sessions: &[SessionResult]) {
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.0.join(SESSIONS), serde_json::to_vec(sessions).unwrap()).unwrap();
    }

    /// Every file under `dir` and what it holds, by path.
    fn everything(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut found = Vec::new();
        let mut look = vec![dir.to_path_buf()];
        while let Some(dir) = look.pop() {
            for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    look.push(path);
                } else {
                    let held = fs::read(&path).unwrap();
                    found.push((path, held));
                }
            }
        }
        found.sort();
        found
    }

    /// A session is one file, holding exactly what was written for it,
    /// and nothing is left beside it.
    #[test]
    fn a_session_is_one_file_written_whole() {
        let (dir, files) = history("one-file");
        let opened = files.load();
        assert!(opened.complete);
        assert!(opened.sessions.is_empty());
        let session = finished(1_790_000_000_000);
        let kept = record(&session);
        assert_eq!(
            files.apply(&put(std::slice::from_ref(&kept))),
            HistorySaved::Stored { kept: 1 }
        );
        let records = dir.0.join(HISTORY).join(RECORDS);
        assert_eq!(names(&records), [file_name(kept.id)]);
        assert_eq!(
            fs::read(records.join(file_name(kept.id))).unwrap(),
            kept.json.as_bytes()
        );
        assert_eq!(
            names(&dir.0.join(HISTORY)),
            [LAYOUT_FILE, RECORDS],
            "nothing left beside it"
        );
        let read = Files::at(&dir.0).load();
        assert_eq!(read.sessions, [(kept.id, session)]);
        assert!(read.notices.is_empty());
    }

    /// A record is never written over: the same bytes again are written
    /// not at all, and other bytes under its name go under the next one.
    #[test]
    fn a_record_is_never_written_over() {
        let (dir, files) = history("never-over");
        files.load();
        let first = finished(1_000);
        let kept = record(&first);
        files.apply(&put(std::slice::from_ref(&kept)));
        files.apply(&put(std::slice::from_ref(&kept)));
        let records = dir.0.join(HISTORY).join(RECORDS);
        assert_eq!(names(&records), [file_name(kept.id)], "once");
        // Other bytes with its name: damaged where they lie, or alike in
        // their place and their hash.
        let other = Record {
            id: kept.id,
            json: serde_json::to_string(&finished(1_000_000)).unwrap(),
        };
        assert_eq!(
            files.apply(&put(std::slice::from_ref(&other))),
            HistorySaved::Stored { kept: 1 }
        );
        assert_eq!(
            names(&records),
            [file_name(kept.id), file_name(kept.id.bumped())]
        );
        assert_eq!(
            fs::read_to_string(records.join(file_name(kept.id))).unwrap(),
            kept.json,
            "the first as it was"
        );
        assert_eq!(files.load().sessions.len(), 2, "and both kept");
    }

    /// The move from a history kept whole, and the read of the records it
    /// made, each say how far they have come as they go, for the boot screen
    /// to say: so many sessions moved of so many, then so many records read
    /// of so many.
    #[test]
    fn the_move_and_the_read_say_how_far_they_have_got() {
        use crate::persist::progress::HistoryProgress;
        let (dir, files) = raw_history("how-far");
        let sessions: Vec<SessionResult> = (1..=40).map(finished).collect();
        keep_whole(&dir, &sessions);
        let progress = files.progress();
        let seen = Arc::new(Mutex::new(Vec::new()));
        {
            let (progress, seen) = (Arc::clone(&progress), Arc::clone(&seen));
            *files.elsewhere.borrow_mut() = Some(Box::new(move || {
                seen.lock().unwrap().push(progress.now());
            }));
        }
        assert_eq!(files.load().sessions.len(), 40);
        let moved = HistoryProgress {
            moving: true,
            count: Some((40, 40)),
        };
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            seen.first(),
            Some(&moved),
            "as the folders are listed after the move"
        );
        let read = HistoryProgress {
            moving: false,
            count: Some((40, 40)),
        };
        assert_eq!(progress.now(), read);
        assert_eq!(read.said(), "Reading your history… 40 of 40");
    }

    /// The history reads in the order of its records, however they were
    /// stored: where each finished, and one a clock set back stamped after
    /// the one it finished after.
    #[test]
    fn history_reads_in_the_order_it_was_kept() {
        let (_dir, files) = history("in-order");
        files.load();
        let kept = records_for_blob(&[finished(5), finished(3), finished(9)]);
        let restored = finished(6);
        let placed = Record::of(&restored, |json| RecordId::placed(6, json)).unwrap();
        let change = put(&[kept[2].clone(), placed, kept[1].clone(), kept[0].clone()]);
        assert_eq!(files.apply(&change), HistorySaved::Stored { kept: 4 });
        assert_eq!(stamps(&files.load()), [5, 3, 6, 9]);
        let listed = files.listing().expect("a listing");
        assert_eq!(listed, ids(&files.load()));
    }

    /// What a write cut short left behind long ago is cleared as the
    /// history is read; one still being written, and whatever else is in
    /// the folders, is left be.
    #[test]
    fn old_temporary_files_are_cleared() {
        let (dir, files) = history("temporaries");
        files.load();
        let kept = record(&finished(1_000));
        files.apply(&put(std::slice::from_ref(&kept)));
        let records = dir.0.join(HISTORY).join(RECORDS);
        let old = records.join(".x.json.1.2.tmp");
        let fresh = records.join(".y.json.1.3.tmp");
        let other = records.join("notes.txt");
        for path in [&old, &fresh, &other] {
            fs::write(path, "half").unwrap();
        }
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - 2 * OLD_TEMPORARY)
            .unwrap();
        let read = files.load();
        assert_eq!(read.sessions.len(), 1);
        assert!(!old.exists(), "cleared");
        assert!(fresh.exists() && other.exists(), "left be");
    }

    /// A record this build cannot read, a newer build's as it says
    /// ([`cw_core::SESSION_FORMAT`]), is left where it is as it is, known
    /// by the session it says it holds, and the user is told of it once,
    /// as a newer Dust's; one that is damaged is told of as damaged.
    #[test]
    fn a_record_this_build_cannot_read_is_left_in_place_and_told_of_once() {
        let (dir, files) = history("unread");
        files.load();
        files.apply(&put(&[record(&finished(1_000))]));
        let records = dir.0.join(HISTORY).join(RECORDS);
        let unread = RecordId {
            order: 2_000_000,
            nonce: 1,
        };
        let raw = format!(
            r#"{{"format":{},"date":"2026-10-30","timestamp":2000,"laterKey":true}}"#,
            cw_core::SESSION_FORMAT + 1
        );
        fs::write(records.join(file_name(unread)), &raw).unwrap();
        let read = files.load();
        assert_eq!(read.sessions.len(), 1);
        assert_eq!(read.unread, [unread]);
        let said = SessionKey::new(2_000, "2026-10-30".into());
        assert_eq!(read.later, [(unread, said.clone())]);
        assert_eq!(read.notices, [HistoryNotice::Later { count: 1 }]);
        let again = Files::at(&dir.0).load();
        assert_eq!(again.unread, [unread], "still known");
        assert_eq!(again.later, [(unread, said)]);
        assert!(again.notices.is_empty(), "told of once");
        assert_eq!(
            fs::read_to_string(records.join(file_name(unread))).unwrap(),
            raw
        );
        // One more is told of on its own, as damaged.
        let another = RecordId {
            order: 3_000_000,
            nonce: 1,
        };
        fs::write(records.join(file_name(another)), "[").unwrap();
        let read = files.load();
        assert_eq!(read.unread, [unread, another]);
        assert_eq!(read.notices, [HistoryNotice::Damaged { count: 1 }]);
    }

    /// A record whose bytes are no text, as damage to a disk leaves them, is
    /// told of as damaged, as one whose text holds no session is: no build
    /// reads it, now or at the next launch. It was told of as one that could
    /// not be read just now, which Dust would read again, and noted as told,
    /// so that was never so. One packed by a later build beside such bytes
    /// reads from its pack, as before.
    #[test]
    fn a_record_whose_bytes_are_no_text_is_told_of_as_damaged() {
        let (dir, files) = history("no-text");
        files.load();
        files.apply(&put(&[record(&finished(1_000))]));
        let records = dir.0.join(HISTORY).join(RECORDS);
        let damaged = RecordId {
            order: 2_000_000,
            nonce: 1,
        };
        let bytes = b"{\"date\":\"2026-10-30\",\"timestamp\":2000,\"\xff\xfe\":1}";
        fs::write(records.join(file_name(damaged)), bytes).unwrap();
        let read = files.load();
        assert_eq!(read.sessions.len(), 1);
        assert_eq!(read.unread, [damaged]);
        assert!(read.later.is_empty());
        assert_eq!(read.notices, [HistoryNotice::Damaged { count: 1 }]);
        assert_eq!(
            fs::read(records.join(file_name(damaged))).unwrap(),
            bytes,
            "left as it is"
        );
        // So a look that reads it finds it.
        assert_eq!(files.read(&[damaged]), [Err(Unopened::Damaged(damaged))]);

        // Packed, it reads from the pack.
        let third = record(&finished(3_000));
        let name = format!("{}-{}{PACK_EXTENSION}", third.id, third.id);
        pack(&dir.0, &name, &[&third], &[]);
        fs::write(records.join(file_name(third.id)), b"\xff").unwrap();
        let read = files.load();
        assert_eq!(stamps(&read), [1_000, 3_000]);
        assert_eq!(read.unread, [damaged]);
    }

    /// What this build could not read stays as it was through every change
    /// made beside it, and is handed over as it was, so a build that reads
    /// it has it whole; one that could not be read for a moment reads once
    /// it can.
    #[test]
    fn a_later_build_reads_what_this_one_could_not() {
        let (dir, files) = history("later");
        files.load();
        let records = dir.0.join(HISTORY).join(RECORDS);
        fs::create_dir_all(&records).unwrap();
        let later = RecordId {
            order: 2_000_000,
            nonce: 9,
        };
        let raw = r#"{"v":2,"date":"2026-10-30","laterShape":{"groups":[1,2]}}"#;
        fs::write(records.join(file_name(later)), raw).unwrap();
        let first = finished(1_000);
        let kept = record(&first);
        files.apply(&put(std::slice::from_ref(&kept)));
        files.apply(&forget(&[(&kept, &first)]));
        assert_eq!(files.load().unread, [later]);
        assert_eq!(
            files.read(&[later]),
            [Ok(Record {
                id: later,
                json: raw.to_string()
            })]
        );
        // One that cannot be read for a moment, as a file another program
        // has open: no record this build cannot read, and read once it can
        // be.
        let busy = record(&finished(3_000));
        fs::create_dir(records.join(file_name(busy.id))).unwrap();
        let read = files.load();
        assert!(!read.unread.contains(&busy.id), "{:?}", read.unread);
        assert_eq!(read.failed, [busy.id]);
        assert_eq!(files.read(&[busy.id]), [Err(Unopened::Failed(busy.id))]);
        fs::remove_dir(records.join(file_name(busy.id))).unwrap();
        fs::write(records.join(file_name(busy.id)), &busy.json).unwrap();
        let read = files.load();
        assert_eq!(read.unread, [later]);
        assert!(read.failed.is_empty());
        assert_eq!(stamps(&read), [3_000]);
    }

    /// A record that could not be read at all just now, as a file another
    /// program holds open for a moment cannot be, failed: it is not one
    /// this build cannot read, and is not told of, as the app reads it
    /// again. One that still cannot be read at the next launch is told of
    /// then, once, as one that could not be read; read once it can be, it
    /// is noted as failing no more. It was held as one this build cannot
    /// read, and told of at once, though a moment later it read.
    #[test]
    fn a_record_that_could_not_be_read_for_a_moment_is_failed_not_unread() {
        let (dir, files) = history("failed");
        files.load();
        files.apply(&put(&[record(&finished(1_000))]));
        let records = dir.0.join(HISTORY).join(RECORDS);
        // A folder where its file goes, which cannot be read as one.
        let busy = record(&finished(2_000));
        fs::create_dir(records.join(file_name(busy.id))).unwrap();
        let read = files.load();
        assert_eq!(read.unread, [], "not one this build cannot read");
        assert_eq!(read.notices, [], "not told of");
        assert_eq!(read.failed, [busy.id]);
        assert_eq!(stamps(&read), [1_000]);

        // Still so at the next launch: told of then, once.
        let read = Files::at(&dir.0).load();
        assert_eq!(read.failed, [busy.id]);
        assert!(read.unread.is_empty());
        assert_eq!(read.notices, [HistoryNotice::Unread { count: 1 }]);
        let read = Files::at(&dir.0).load();
        assert_eq!(read.failed, [busy.id]);
        assert!(read.notices.is_empty(), "told of once");

        // Read once it can be.
        fs::remove_dir(records.join(file_name(busy.id))).unwrap();
        fs::write(records.join(file_name(busy.id)), &busy.json).unwrap();
        let read = files.load();
        assert_eq!(stamps(&read), [1_000, 2_000]);
        assert!(read.failed.is_empty());
        assert!(!dir.0.join(HISTORY).join(FAILING).exists(), "noted no more");
    }

    /// A record that still cannot be read after every try the app makes is
    /// told of then, as one that could not be read, and noted as told: once,
    /// however often the app asks after, and not again at the next launch.
    #[test]
    fn a_record_still_unread_after_every_try_is_told_of_once() {
        let (dir, files) = history("still-unread");
        files.load();
        let busy = record(&finished(2_000));
        let records = dir.0.join(HISTORY).join(RECORDS);
        fs::create_dir_all(records.join(file_name(busy.id))).unwrap();
        assert_eq!(files.load().notices, [], "not as it is first read");
        assert_eq!(
            files.still_unread(&[busy.id]),
            [HistoryNotice::Unread { count: 1 }]
        );
        assert_eq!(files.still_unread(&[busy.id]), [], "once");
        let read = Files::at(&dir.0).load();
        assert_eq!(read.failed, [busy.id]);
        assert_eq!(read.notices, [], "nor at the next launch");
    }

    /// A record told of as one that could not be read, which then reads,
    /// at a later launch or at one of the app's tries, and fails again
    /// after, is told of again, once: that is a failure of its own. The
    /// note of those told of kept it for good, as it was rewritten only to
    /// note one more, and it was never told of again.
    #[test]
    fn a_record_told_of_as_unread_that_reads_and_fails_again_is_told_of_again() {
        let (dir, files) = history("told-read-failed");
        files.load();
        let busy = record(&finished(2_000));
        let path = dir.0.join(HISTORY).join(RECORDS).join(file_name(busy.id));
        let hold = || {
            let _ = fs::remove_file(&path);
            fs::create_dir_all(&path).unwrap();
        };
        let free = || {
            fs::remove_dir(&path).unwrap();
            fs::write(&path, &busy.json).unwrap();
        };
        let told = [HistoryNotice::Unread { count: 1 }];
        hold();
        assert_eq!(files.load().notices, []);
        assert_eq!(files.still_unread(&[busy.id]), told, "after every try");
        // It reads at the next launch, and fails at the one after.
        free();
        assert_eq!(stamps(&Files::at(&dir.0).load()), [2_000]);
        hold();
        let files = Files::at(&dir.0);
        assert_eq!(files.load().failed, [busy.id]);
        assert_eq!(files.still_unread(&[busy.id]), told, "told of again");
        assert_eq!(files.still_unread(&[busy.id]), [], "once");
        // It reads at one of the app's tries, and fails at a later one.
        free();
        assert_eq!(files.read(&[busy.id]), [Ok(busy.clone())]);
        hold();
        assert_eq!(files.read(&[busy.id]), [Err(Unopened::Failed(busy.id))]);
        assert_eq!(files.still_unread(&[busy.id]), told, "and again");
        assert_eq!(files.still_unread(&[busy.id]), [], "once");
    }

    /// What a build before this one noted there as told of, a record it
    /// cannot read, which this build reads, is left as it noted it, at a
    /// launch and at a look: such a build notes there the records it cannot
    /// read, to tell of each once, and one dropped from the note it tells
    /// of again as it next opens. 319d303 dropped every record that read
    /// here, and such a build told of its own at every launch after this
    /// one's.
    #[test]
    fn what_an_older_build_noted_as_told_of_is_left_as_it_noted_it() {
        let (dir, files) = history("told-by-an-older-build");
        files.load();
        let first = record(&finished(1_000));
        files.apply(&put(std::slice::from_ref(&first)));
        let told = dir.0.join(HISTORY).join(TOLD);
        let noted = serde_json::to_vec(&[first.id.name()]).unwrap();
        fs::write(&told, &noted).unwrap();
        let files = Files::at(&dir.0);
        assert_eq!(stamps(&files.load()), [1_000]);
        assert_eq!(
            fs::read(&told).unwrap(),
            noted,
            "left as it was at a launch"
        );
        assert_eq!(files.read(&[first.id]), [Ok(first)]);
        assert_eq!(fs::read(&told).unwrap(), noted, "and at a look");
    }

    /// What a build before this one noted there as told of, a record it
    /// cannot read that this build reads, stays as it noted it when this
    /// build has something new of its own to tell: dropped, that build
    /// tells of it again as it next opens (0.16.9 with a session stored
    /// with a null score, which it cannot read).
    #[test]
    fn what_an_older_build_noted_as_told_of_stays_when_this_build_tells_of_another() {
        let (dir, files) = history("told-elsewhere-and-here");
        files.load();
        let first = record(&finished(1_000));
        files.apply(&put(std::slice::from_ref(&first)));
        let told = dir.0.join(HISTORY).join(TOLD);
        fs::write(&told, serde_json::to_vec(&[first.id.name()]).unwrap()).unwrap();
        let damaged = RecordId {
            order: 2_000_000,
            nonce: 1,
        };
        let records = dir.0.join(HISTORY).join(RECORDS);
        fs::write(records.join(file_name(damaged)), "[").unwrap();
        let load = Files::at(&dir.0).load();
        assert_eq!(load.notices, [HistoryNotice::Damaged { count: 1 }]);
        let noted = names_noted(fs::read(&told).ok().as_deref());
        assert!(noted.contains(&damaged.name()), "this build's noted");
        assert!(
            noted.contains(&first.id.name()),
            "the older build's note stays: {noted:?}"
        );
    }

    /// A record this build told of as one that could not be read, after
    /// the app's tries or at the launch after, is noted as told in told.json
    /// too, where a build before formats (0.16.9, 95aa091) looks: such a
    /// build tells of each record it cannot open that told.json does not
    /// name. Noted in unread-told.json alone, which no such build reads, it
    /// was told of a second time by such a build as it next opened while
    /// the record still could not be read.
    #[test]
    fn a_record_this_build_told_of_as_unread_is_not_told_of_again_by_an_older_build() {
        let fails = |name: &str| {
            let (dir, files) = history(name);
            files.load();
            let busy = record(&finished(2_000));
            files.apply(&put(std::slice::from_ref(&busy)));
            let path = dir.0.join(HISTORY).join(RECORDS).join(file_name(busy.id));
            fs::remove_file(&path).unwrap();
            fs::create_dir_all(&path).unwrap();
            (dir, busy)
        };
        // What 0.16.9 and 95aa091 go by as they open.
        let an_older_build_tells_of = |dir: &TempDir, busy: &Record| {
            !names_noted(fs::read(dir.0.join(HISTORY).join(TOLD)).ok().as_deref())
                .contains(&busy.id.name())
        };
        let told = [HistoryNotice::Unread { count: 1 }];
        // Told of after the app's tries.
        let (dir, busy) = fails("told-here-after-tries");
        let files = Files::at(&dir.0);
        assert_eq!(files.load().failed, [busy.id]);
        assert_eq!(files.still_unread(&[busy.id]), told);
        assert!(
            !an_older_build_tells_of(&dir, &busy),
            "not again by an older build, after the tries"
        );
        // Told of at the launch after, the app closed before its tries were
        // over.
        let (dir, busy) = fails("told-here-at-launch");
        assert_eq!(Files::at(&dir.0).load().notices, []);
        assert_eq!(Files::at(&dir.0).load().notices, told);
        assert!(
            !an_older_build_tells_of(&dir, &busy),
            "nor after the launch that told of it"
        );
    }

    /// A record this build told of as one that could not be read, its name
    /// noted in told.json for builds before formats, that is found damaged
    /// at a later launch is told of as damaged: told.json stands for this
    /// build's note of damage only for what such a build noted there, not
    /// for what this build noted itself.
    #[test]
    fn a_record_this_build_told_of_as_unread_that_is_damaged_since_is_told_of_as_damaged() {
        let (dir, files) = history("told-unread-then-damaged");
        files.load();
        let busy = record(&finished(2_000));
        files.apply(&put(std::slice::from_ref(&busy)));
        let path = dir.0.join(HISTORY).join(RECORDS).join(file_name(busy.id));
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        let files = Files::at(&dir.0);
        assert_eq!(files.load().failed, [busy.id]);
        assert_eq!(
            files.still_unread(&[busy.id]),
            [HistoryNotice::Unread { count: 1 }]
        );
        fs::remove_dir(&path).unwrap();
        fs::write(&path, "{ cut short").unwrap();
        assert_eq!(
            Files::at(&dir.0).load().notices,
            [HistoryNotice::Damaged { count: 1 }],
            "told of as damaged"
        );
    }

    /// A record this build told of as one that could not be read, which is no
    /// longer listed at a look, is noted as told of no more: it is gone, and
    /// should it come back and fail, that is a failure of its own.
    #[test]
    fn a_record_told_of_that_is_gone_at_a_look_is_told_of_no_more() {
        let (dir, files) = history("told-gone-at-a-look");
        files.load();
        let busy = record(&finished(2_000));
        files.apply(&put(std::slice::from_ref(&busy)));
        let path = dir.0.join(HISTORY).join(RECORDS).join(file_name(busy.id));
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        let files = Files::at(&dir.0);
        assert_eq!(files.load().failed, [busy.id]);
        assert_eq!(
            files.still_unread(&[busy.id]),
            [HistoryNotice::Unread { count: 1 }]
        );
        fs::remove_dir(&path).unwrap();
        assert_eq!(files.listing(), Some(Vec::new()), "gone, at a look");
        fs::create_dir_all(&path).unwrap();
        assert_eq!(files.read(&[busy.id]), [Err(Unopened::Failed(busy.id))]);
        assert_eq!(
            files.still_unread(&[busy.id]),
            [HistoryNotice::Unread { count: 1 }],
            "back, and failing on its own"
        );
    }

    /// A record a build before this one told of as one it could not read,
    /// as 0.16.9 and 95aa091 tell of one they cannot open at all (its name
    /// in told.json), which reads here at a later launch and then fails for
    /// good at the one after, is told of after every try: that is a failure
    /// of its own.
    #[test]
    fn a_record_an_older_build_told_of_that_reads_here_and_fails_again_is_told_of() {
        let (dir, files) = history("older-told-fails-here");
        files.load();
        let busy = record(&finished(2_000));
        files.apply(&put(std::slice::from_ref(&busy)));
        // As 0.16.9 notes a record it could not open as it opened.
        let told = dir.0.join(HISTORY).join(TOLD);
        fs::write(&told, serde_json::to_vec(&[busy.id.name()]).unwrap()).unwrap();
        // It reads at this build's next launch.
        assert_eq!(stamps(&Files::at(&dir.0).load()), [2_000]);
        // It cannot be read at the launch after, nor at any try.
        let path = dir.0.join(HISTORY).join(RECORDS).join(file_name(busy.id));
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        let files = Files::at(&dir.0);
        assert_eq!(files.load().failed, [busy.id]);
        assert_eq!(
            files.still_unread(&[busy.id]),
            [HistoryNotice::Unread { count: 1 }],
            "told of after every try"
        );
    }

    /// A record a build before this one told of as one it could not read,
    /// for a moment, that reads here, then fails here for good, is told of
    /// here once the app's tries are over. Such a build (0.16.9, 95aa091)
    /// tells of every record whose read failed, a moment's failure among
    /// them, and notes it in told.json; kept there as that build's note, it
    /// held back this build's notice of the record's own failure for good,
    /// at every launch after.
    #[test]
    fn a_record_an_older_build_told_of_that_reads_here_and_then_fails_is_told_of() {
        let (dir, files) = history("told-elsewhere-fails-here");
        files.load();
        let busy = record(&finished(2_000));
        files.apply(&put(std::slice::from_ref(&busy)));
        // As 0.16.9 notes it, having failed to read it as it opened.
        let told = dir.0.join(HISTORY).join(TOLD);
        fs::write(&told, serde_json::to_vec(&[busy.id.name()]).unwrap()).unwrap();
        // It reads here, at a launch and at a look.
        let files = Files::at(&dir.0);
        assert_eq!(stamps(&files.load()), [2_000]);
        assert_eq!(files.read(&[busy.id]), [Ok(busy.clone())]);
        // Then it fails here, for good.
        let path = dir.0.join(HISTORY).join(RECORDS).join(file_name(busy.id));
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        let files = Files::at(&dir.0);
        assert_eq!(files.load().failed, [busy.id]);
        assert_eq!(
            files.still_unread(&[busy.id]),
            [HistoryNotice::Unread { count: 1 }],
            "told of after every try"
        );
    }

    /// The same, where the app is closed before its tries are over at the
    /// first launch the record fails at: it is told of at the next, as one
    /// that failed at the last read and still does.
    #[test]
    fn a_record_an_older_build_told_of_that_reads_here_and_fails_at_two_launches_is_told_of() {
        let (dir, files) = history("told-elsewhere-fails-twice");
        files.load();
        let busy = record(&finished(2_000));
        files.apply(&put(std::slice::from_ref(&busy)));
        let told = dir.0.join(HISTORY).join(TOLD);
        fs::write(&told, serde_json::to_vec(&[busy.id.name()]).unwrap()).unwrap();
        assert_eq!(stamps(&Files::at(&dir.0).load()), [2_000]);
        let path = dir.0.join(HISTORY).join(RECORDS).join(file_name(busy.id));
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        assert_eq!(
            Files::at(&dir.0).load().notices,
            [],
            "a moment's, for all it knows"
        );
        assert_eq!(
            Files::at(&dir.0).load().notices,
            [HistoryNotice::Unread { count: 1 }],
            "told of at the launch after"
        );
    }

    /// A record a build before formats told of, as it does of every record it
    /// could not open, a moment's failure among them (its name in told.json),
    /// which reads here, and is damaged later, its bytes cut short where they
    /// lie, is told of as damaged at the launch that finds it so, as one with
    /// no such note is. That build's note held back this build's notice of a
    /// damage of its own, at every launch.
    #[test]
    fn a_record_an_older_build_told_of_that_reads_here_and_is_damaged_later_is_told_of() {
        for older_noted in [false, true] {
            let (dir, files) = history(if older_noted {
                "older-told-damaged"
            } else {
                "damaged-later"
            });
            files.load();
            let busy = record(&finished(2_000));
            files.apply(&put(std::slice::from_ref(&busy)));
            if older_noted {
                let told = dir.0.join(HISTORY).join(TOLD);
                fs::write(&told, serde_json::to_vec(&[busy.id.name()]).unwrap()).unwrap();
            }
            assert_eq!(stamps(&Files::at(&dir.0).load()), [2_000]);
            let path = dir.0.join(HISTORY).join(RECORDS).join(file_name(busy.id));
            fs::write(&path, "{ cut short").unwrap();
            assert_eq!(
                Files::at(&dir.0).load().notices,
                [HistoryNotice::Damaged { count: 1 }],
                "told of as damaged (an older build's note: {older_noted})"
            );
        }
    }

    /// A record a build before formats told of, its name in told.json, that
    /// this build cannot read either, as one damaged since that build found
    /// it so, is not told of a second time: told.json stands for this
    /// build's note of damage until this build first writes it, and that
    /// note holds it from then on.
    #[test]
    fn a_record_an_older_build_told_of_that_this_build_cannot_read_either_is_not_told_of_again() {
        let (dir, files) = history("older-told-still-damaged");
        files.load();
        files.apply(&put(&[record(&finished(1_000))]));
        let damaged = RecordId {
            order: 2_000_000,
            nonce: 1,
        };
        let records = dir.0.join(HISTORY).join(RECORDS);
        fs::write(records.join(file_name(damaged)), "[").unwrap();
        let told = dir.0.join(HISTORY).join(TOLD);
        fs::write(&told, serde_json::to_vec(&[damaged.name()]).unwrap()).unwrap();
        assert_eq!(
            Files::at(&dir.0).load().notices,
            [],
            "told of by that build"
        );
        assert_eq!(
            Files::at(&dir.0).load().notices,
            [],
            "nor at the launch after"
        );
    }

    /// A record that could not be read as the history was read, and reads
    /// at one of the app's tries after, fails no longer: should it fail for
    /// a moment again as the history is next read, at the next launch, it
    /// is not told of then, as one that failed at the last read and has not
    /// read since is. The note of what failed lasted until the history was
    /// read whole again, and such a record was told of at once.
    #[test]
    fn a_record_read_at_a_try_after_it_failed_fails_no_longer() {
        let (dir, files) = history("failed-then-read");
        files.load();
        files.apply(&put(&[record(&finished(1_000))]));
        let records = dir.0.join(HISTORY).join(RECORDS);
        let (busy, stuck) = (record(&finished(2_000)), record(&finished(3_000)));
        for held in [&busy, &stuck] {
            fs::create_dir(records.join(file_name(held.id))).unwrap();
        }
        let read = files.load();
        assert_eq!(read.failed, [busy.id, stuck.id]);
        assert_eq!(read.notices, []);
        // The first reads at the app's next try; the second does not.
        let path = records.join(file_name(busy.id));
        fs::remove_dir(&path).unwrap();
        fs::write(&path, &busy.json).unwrap();
        assert_eq!(
            files.read(&[busy.id, stuck.id]),
            [Ok(busy.clone()), Err(Unopened::Failed(stuck.id))]
        );
        // As the app next opens, the first fails for a moment again: only
        // the second, which has not read since it failed, is told of.
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let read = Files::at(&dir.0).load();
        assert_eq!(read.failed, [busy.id, stuck.id]);
        assert_eq!(read.notices, [HistoryNotice::Unread { count: 1 }]);
        // Both read: nothing is noted as failing.
        fs::remove_dir(&path).unwrap();
        fs::write(&path, &busy.json).unwrap();
        let path = records.join(file_name(stuck.id));
        fs::remove_dir(&path).unwrap();
        fs::write(&path, &stuck.json).unwrap();
        assert_eq!(files.read(&[busy.id, stuck.id]), [Ok(busy), Ok(stuck)]);
        assert!(!dir.0.join(HISTORY).join(FAILING).exists(), "noted no more");
    }

    /// A record another copy deletes as this one reads the history, once
    /// the tombstones were listed and before the record is read, is gone by
    /// the time it is read: it failed, as one that cannot be read at all
    /// just now, and is neither told of nor held as one this build cannot
    /// read. The next look finds it deleted. It was told of as one that
    /// could not be read, and held for one stored, for as long as the app
    /// ran.
    #[test]
    fn a_record_deleted_elsewhere_as_the_history_is_read_is_not_told_of_as_unreadable() {
        let (dir, files) = history("deleted-as-read");
        files.load();
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        files.apply(&put(&[one.clone(), two.clone()]));
        let root = dir.0.clone();
        let (gone, of) = (two.clone(), second);
        let mut looked = 0;
        files.meanwhile(move || {
            looked += 1;
            // Once the records, the packs and the tombstones are listed,
            // before any record is read: its tombstone made, and its file
            // taken away, as another copy deletes it.
            if looked == 3 {
                HistoryFiles::at(&root).apply(&forget(&[(&gone, &of)]));
            }
        });
        let read = files.load();
        assert_eq!(read.notices, [], "not told of");
        assert_eq!(read.unread, [], "nor held as one this build cannot read");
        assert_eq!(read.failed, [two.id]);
        assert_eq!(stamps(&read), [1_000]);
        assert_eq!(
            files.listing(),
            Some(vec![one.id]),
            "deleted, the next look finds"
        );
        let read = files.load();
        assert!(read.failed.is_empty());
        assert!(read.notices.is_empty(), "nor told of at the next launch");
    }

    /// A deletion is a tombstone beside the history, by the record's
    /// name, holding what tells the session apart, and one there already,
    /// with what a later build added to it, is never written over; undone,
    /// the session is back where it was.
    #[test]
    fn a_deletion_is_a_tombstone_beside_the_history() {
        let (dir, files) = history("tombstone");
        files.load();
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        files.apply(&put(&[one.clone(), two.clone()]));
        assert_eq!(
            files.apply(&forget(&[(&one, &first)])),
            HistorySaved::Stored { kept: 0 }
        );
        let (records, tombstones) = (
            dir.0.join(HISTORY).join(RECORDS),
            dir.0.join(HISTORY).join(TOMBSTONES),
        );
        assert_eq!(names(&records), [file_name(two.id)]);
        assert_eq!(names(&tombstones), [file_name(one.id)]);
        let held: Deleted =
            serde_json::from_slice(&fs::read(tombstones.join(file_name(one.id))).unwrap()).unwrap();
        assert_eq!(held, Deleted::of(&first, 7));
        assert_eq!(ids(&files.load()), [two.id]);
        assert_eq!(files.forgotten(&[one.id, two.id]), [one.id]);
        // A later build wrote more into it, and this one deletes the
        // session again, as another copy that still held it would.
        let mut later = serde_json::to_value(Deleted::of(&first, 5)).unwrap();
        later["reason"] = "later".into();
        let later = serde_json::to_vec(&later).unwrap();
        fs::write(tombstones.join(file_name(one.id)), &later).unwrap();
        assert_eq!(
            files.apply(&forget(&[(&one, &first)])),
            HistorySaved::Stored { kept: 0 }
        );
        assert_eq!(fs::read(tombstones.join(file_name(one.id))).unwrap(), later);
        // Stored again without undoing it: still deleted.
        files.apply(&put(std::slice::from_ref(&one)));
        assert_eq!(ids(&files.load()), [two.id]);
        // Undone: back where it was.
        let undo = HistoryChange {
            put: vec![one.clone()],
            unforget: vec![one.id],
            ..HistoryChange::default()
        };
        assert_eq!(files.apply(&undo), HistorySaved::Stored { kept: 1 });
        assert_eq!(ids(&files.load()), [one.id, two.id]);
        assert!(names(&tombstones).is_empty());
        assert!(files.forgotten(&[one.id]).is_empty());
    }

    /// A record beside its tombstone, as a deletion cut short leaves it,
    /// reads as deleted, and the deletion is finished.
    #[test]
    fn a_record_and_its_tombstone_read_as_deleted() {
        let (dir, files) = history("cut-deletion");
        files.load();
        let first = finished(1_000);
        let one = record(&first);
        files.apply(&put(std::slice::from_ref(&one)));
        let tombstones = dir.0.join(HISTORY).join(TOMBSTONES);
        fs::create_dir_all(&tombstones).unwrap();
        fs::write(
            tombstones.join(file_name(one.id)),
            serde_json::to_vec(&Deleted::of(&first, 7)).unwrap(),
        )
        .unwrap();
        assert!(files.load().sessions.is_empty());
        assert!(names(&dir.0.join(HISTORY).join(RECORDS)).is_empty());
        assert_eq!(files.listing(), Some(Vec::new()));
    }

    /// A tombstone that holds nothing, or nothing that reads, deletes its
    /// record all the same.
    #[test]
    fn an_empty_tombstone_still_deletes() {
        let (dir, files) = history("empty-tombstone");
        files.load();
        let one = record(&finished(1_000));
        let two = record(&finished(2_000));
        files.apply(&put(&[one.clone(), two.clone()]));
        let tombstones = dir.0.join(HISTORY).join(TOMBSTONES);
        fs::create_dir_all(&tombstones).unwrap();
        fs::write(tombstones.join(file_name(one.id)), "").unwrap();
        fs::write(tombstones.join(file_name(two.id)), "{ not").unwrap();
        assert_eq!(files.listing(), Some(Vec::new()));
        assert_eq!(files.forgotten(&[one.id, two.id]), [one.id, two.id]);
        assert!(files.load().sessions.is_empty());
    }

    /// Which records were deleted by hand, each beside the session its
    /// tombstone names, answers for a tombstone that is there but cannot be
    /// read, as a folder in its place cannot, as deletions() does: deleted,
    /// with no word of which session, so it is left out. No word at all came
    /// back, for every record asked of, the one whose tombstone names its
    /// session among them.
    #[cfg(unix)]
    #[test]
    fn which_sessions_were_deleted_is_said_of_a_tombstone_that_cannot_be_read_as_deletions_say() {
        let (dir, files) = history("deleted-sessions-unread-tombstone");
        files.load();
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        files.apply(&put(&[one.clone(), two.clone()]));
        files.apply(&forget(&[(&one, &first), (&two, &second)]));
        let path = dir.0.join(HISTORY).join(TOMBSTONES).join(file_name(two.id));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let all = [one.id, two.id];
        assert_eq!(
            files.deletions(&all),
            Some(vec![(one.id, Some(7)), (two.id, None)])
        );
        assert_eq!(
            files.deleted_sessions(&all),
            Some(vec![(one.id, SessionKey::of(&first))]),
            "as deletions() says"
        );
    }

    /// Which records were deleted by hand comes with when each was, as its
    /// tombstone says, none for one that does not say. Where the tombstones
    /// cannot be looked for, as where their folder will not open, there is
    /// no word at all, rather than the word that none was deleted; and once
    /// they can be again, the word comes back.
    #[cfg(unix)]
    #[test]
    fn deletions_say_when_each_was_made_or_that_none_could_be_looked_for() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, files) = history("deletions");
        files.load();
        let (first, second, third) = (finished(1_000), finished(2_000), finished(3_000));
        let (one, two, three) = (record(&first), record(&second), record(&third));
        files.apply(&put(&[one.clone(), two.clone(), three.clone()]));
        files.apply(&forget(&[(&one, &first)]));
        let tombstones = dir.0.join(HISTORY).join(TOMBSTONES);
        fs::write(tombstones.join(file_name(two.id)), "{ not").unwrap();
        let all = [one.id, two.id, three.id];
        let found = Some(vec![(one.id, Some(7)), (two.id, None)]);
        assert_eq!(files.deletions(&all), found);
        assert_eq!(
            files.deleted_sessions(&all),
            Some(vec![(one.id, SessionKey::of(&first))]),
            "which session, where its tombstone says"
        );
        let keys = [&first, &second, &third].map(SessionKey::of);
        assert_eq!(
            files.deleted_keys(&keys),
            Some(vec![keys[0].clone()]),
            "which sessions a tombstone names, whatever its record"
        );
        // In the folder's place, a link that leads back to itself: nothing
        // in it can be looked for, whoever runs this.
        let aside = dir.0.join("tombstones-aside");
        fs::rename(&tombstones, &aside).unwrap();
        std::os::unix::fs::symlink(&tombstones, &tombstones).unwrap();
        assert_eq!(files.deletions(&all), None, "looked for through a loop");
        assert_eq!(files.deleted_sessions(&all), None);
        assert_eq!(files.deleted_keys(&keys), None);
        assert!(files.forgotten(&all).is_empty(), "missing is never deleted");
        fs::remove_file(&tombstones).unwrap();
        fs::rename(&aside, &tombstones).unwrap();
        assert_eq!(files.deletions(&all), found, "looked for again");
        // A folder that cannot be searched, where its permissions hold.
        if TempDir::permissions_hold() {
            let mode = |mode| fs::set_permissions(&tombstones, fs::Permissions::from_mode(mode));
            mode(0o600).unwrap();
            let unsearchable = files.deletions(&all);
            mode(0o755).unwrap();
            assert_eq!(
                unsearchable, None,
                "looked for in a folder not to be searched"
            );
        }
    }

    /// A deletion is made once its tombstones are: a record that will not
    /// be taken away yet, as one held open elsewhere will not on Windows,
    /// leaves the change made, not failed, and the session reads as
    /// deleted, here, to another copy and at the next read. Told of as
    /// failed, the app showed the session as kept, with nothing to undo,
    /// until the next read let it go.
    #[test]
    fn a_deletion_is_made_once_its_tombstones_are() {
        let (dir, files) = history("record-stays");
        files.load();
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        files.apply(&put(&[one.clone(), two.clone()]));
        // Where the record is, something that cannot be taken away as one.
        let stuck = dir.0.join(HISTORY).join(RECORDS).join(file_name(one.id));
        fs::remove_file(&stuck).unwrap();
        fs::create_dir_all(stuck.join("held")).unwrap();
        assert_eq!(
            files.apply(&forget(&[(&one, &first)])),
            HistorySaved::Stored { kept: 0 }
        );
        assert!(stuck.exists(), "left for now");
        assert_eq!(files.listing(), Some(vec![two.id]));
        assert_eq!(files.forgotten(&[one.id, two.id]), [one.id]);
        let read = Files::at(&dir.0).load();
        assert_eq!(ids(&read), [two.id]);
        assert!(
            read.unread.is_empty() && read.notices.is_empty(),
            "{read:?}"
        );
    }

    /// A deletion whose tombstones cannot all be made durable deletes
    /// nothing: those it placed are taken away again, and it is told of as
    /// failed, so the app, which puts the sessions back, shows what the
    /// history holds. Those placed used to stay, and the sessions the app
    /// put back went from it at the next read.
    #[cfg(unix)]
    #[test]
    fn a_deletion_that_cannot_be_made_durable_deletes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        if !TempDir::permissions_hold() {
            eprintln!("skipped: a folder's permissions do not hold whoever runs this");
            return;
        }
        let (dir, files) = history("unmarked");
        files.load();
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        files.apply(&put(&[one.clone(), two.clone()]));
        let tombstones = dir.0.join(HISTORY).join(TOMBSTONES);
        fs::create_dir_all(&tombstones).unwrap();
        // Tombstones go in, but the folder cannot be opened to make them
        // durable.
        let mode = |mode| fs::set_permissions(&tombstones, fs::Permissions::from_mode(mode));
        mode(0o300).unwrap();
        let saved = files.apply(&forget(&[(&one, &first), (&two, &second)]));
        mode(0o755).unwrap();
        assert!(matches!(saved, HistorySaved::Failed(_)), "{saved:?}");
        assert!(names(&tombstones).is_empty(), "{:?}", names(&tombstones));
        assert_eq!(files.listing(), Some(vec![one.id, two.id]));
        assert_eq!(ids(&Files::at(&dir.0).load()), [one.id, two.id]);
    }

    /// A session put back whose record was never taken away, as a deletion
    /// whose record would not go leaves it, is back as it was stored, under
    /// its own name, though the app writes it otherwise now: one record,
    /// not two of one session.
    #[test]
    fn a_session_put_back_whose_record_stayed_is_one_record() {
        let (dir, files) = history("put-back-stayed");
        files.load();
        let first = finished(1_000);
        let one = record(&first);
        files.apply(&put(std::slice::from_ref(&one)));
        let tombstones = dir.0.join(HISTORY).join(TOMBSTONES);
        fs::create_dir_all(&tombstones).unwrap();
        fs::write(
            tombstones.join(file_name(one.id)),
            serde_json::to_vec(&Deleted::of(&first, 7)).unwrap(),
        )
        .unwrap();
        let undo = HistoryChange {
            put: vec![Record {
                id: one.id,
                json: serde_json::to_string_pretty(&first).unwrap(),
            }],
            unforget: vec![one.id],
            ..HistoryChange::default()
        };
        assert_eq!(files.apply(&undo), HistorySaved::Stored { kept: 1 });
        let records = dir.0.join(HISTORY).join(RECORDS);
        assert_eq!(names(&records), [file_name(one.id)]);
        assert_eq!(
            fs::read_to_string(records.join(file_name(one.id))).unwrap(),
            one.json
        );
        assert!(names(&tombstones).is_empty());
        assert_eq!(ids(&files.load()), [one.id]);
    }

    /// The history knows how large it has grown, as it is read and as each
    /// change is made, without a look at the disk for it: what tells the
    /// app that Android's own backup is about to leave it out.
    #[test]
    fn the_history_knows_how_large_it_has_grown() {
        let (dir, files) = history("size");
        files.load();
        assert_eq!(files.bytes(), 0);
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        files.apply(&put(&[one.clone(), two.clone()]));
        let both = (one.json.len() + two.json.len()) as u64;
        assert_eq!(files.bytes(), both);
        files.apply(&put(std::slice::from_ref(&one)));
        assert_eq!(files.bytes(), both, "nothing written twice");
        files.apply(&forget(&[(&one, &first)]));
        assert_eq!(files.bytes(), two.json.len() as u64);
        // Read again, by another copy too, it is what the records hold.
        assert_eq!(files.load().sessions.len(), 1);
        assert_eq!(files.bytes(), two.json.len() as u64);
        let other = Files::at(&dir.0);
        other.load();
        assert_eq!(other.bytes(), two.json.len() as u64);
    }

    /// Two copies on one folder each take up what the other stored and
    /// deleted, and are told the folder changed when it did, and not when
    /// they changed it themselves.
    #[test]
    fn two_copies_on_one_folder_take_up_each_others_changes() {
        let dir = TempDir::new("two-copies");
        let (here, there) = (Files::at(&dir.0), Files::at(&dir.0));
        here.load();
        there.load();
        let first = finished(1_000);
        let one = record(&first);
        here.apply(&put(std::slice::from_ref(&one)));
        assert!(!here.changed(), "its own change");
        assert!(there.changed());
        assert_eq!(there.listing(), Some(vec![one.id]));
        assert!(!there.changed(), "taken up");
        assert_eq!(there.read(&[one.id]), [Ok(one.clone())]);
        there.apply(&forget(&[(&one, &first)]));
        assert!(here.changed());
        assert_eq!(here.listing(), Some(Vec::new()));
        assert_eq!(here.forgotten(&[one.id]), [one.id]);
    }

    /// What another copy stored or deleted since this one last looked is
    /// taken up after this one makes a change of its own: its own change
    /// is none for it to take up, the other's is. Noting the folders as its
    /// own change left them noted the other's as seen too, and the session
    /// it stored, or deleted, was never taken up until the app was opened
    /// again.
    #[test]
    fn a_change_another_copy_made_first_is_taken_up_after_ones_own() {
        let dir = TempDir::new("theirs-first");
        let (here, there) = (Files::at(&dir.0), Files::at(&dir.0));
        here.load();
        there.load();
        let (first, second, third) = (finished(1_000), finished(2_000), finished(3_000));
        let (theirs, ours, more) = (record(&first), record(&second), record(&third));
        there.apply(&put(std::slice::from_ref(&theirs)));
        here.apply(&put(std::slice::from_ref(&ours)));
        assert!(here.changed(), "theirs, to take up");
        assert_eq!(here.listing(), Some(vec![theirs.id, ours.id]));
        assert!(!here.changed(), "taken up");
        there.apply(&forget(&[(&theirs, &first)]));
        here.apply(&put(std::slice::from_ref(&more)));
        assert!(here.changed(), "their deletion, to take up");
        assert_eq!(here.listing(), Some(vec![ours.id, more.id]));
        assert_eq!(here.forgotten(&[theirs.id]), [theirs.id]);
        assert!(!here.changed());
    }

    /// Have another copy of the app store each of `theirs`, one each time
    /// a folder of the history in `root` has been read, from the next
    /// listing of `files` on, as it would while they read.
    fn stored_meanwhile(files: &Files, root: &Path, theirs: Vec<Record>) {
        let root = root.to_path_buf();
        let mut theirs = theirs.into_iter();
        files.meanwhile(move || {
            if let Some(record) = theirs.next() {
                HistoryFiles::at(&root).apply(&put(std::slice::from_ref(&record)));
            }
        });
    }

    /// What another copy stores while this one reads the history whole, or
    /// lists it, is taken up at the next look, wherever in the reading it
    /// was stored. Each noted the folders as they were once it was done,
    /// what the other stored meanwhile among them, and what the reading had
    /// not seen was never taken up.
    #[test]
    fn what_another_copy_stores_while_the_history_is_read_is_taken_up() {
        let theirs: Vec<Record> = (1..=3).map(|n| record(&finished(n * 1_000))).collect();
        let named: Vec<RecordId> = theirs.iter().map(|record| record.id).collect();
        let (dir, files) = history("stored-as-read");
        stored_meanwhile(&files, &dir.0, theirs.clone());
        files.load();
        assert!(files.changed(), "what was stored as it read, to take up");
        assert_eq!(files.listing(), Some(named.clone()));
        assert!(!files.changed(), "taken up");

        let (dir, files) = history("stored-as-listed");
        files.load();
        stored_meanwhile(&files, &dir.0, theirs);
        files.listing();
        assert!(files.changed(), "what was stored as it listed, to take up");
        assert_eq!(files.listing(), Some(named));
        assert!(!files.changed(), "taken up");
    }

    /// A change another copy made is still to take up after a look that
    /// could not list the history, however often it is asked of, until a
    /// look that can takes it up. Asking noted it as seen, so a listing
    /// that failed left it for good: the session another copy stored never
    /// showed here for as long as the app ran.
    #[test]
    fn a_change_is_still_to_take_up_after_a_listing_that_failed() {
        let dir = TempDir::new("listing-failed");
        let (here, there) = (Files::at(&dir.0), Files::at(&dir.0));
        here.load();
        there.load();
        let one = record(&finished(1_000));
        there.apply(&put(std::slice::from_ref(&one)));
        assert!(here.changed(), "theirs, to take up");
        assert!(here.changed(), "still, until it is");
        // Once the records are listed, the packs cannot be: a file stands
        // where their folder goes, for this one look.
        let packs = dir.0.join(HISTORY).join(PACKS);
        let mut in_the_way = Some(packs.clone());
        here.meanwhile(move || {
            if let Some(packs) = in_the_way.take() {
                fs::write(packs, "in the way").unwrap();
            }
        });
        assert_eq!(here.listing(), None);
        fs::remove_file(&packs).unwrap();
        assert!(here.changed(), "still to take up");
        assert_eq!(here.listing(), Some(vec![one.id]));
        assert!(!here.changed(), "taken up");
    }

    /// A restore of many sessions at once that stops before it is done is
    /// told of by the next to read the history, once; one still running
    /// in another copy is left to it.
    #[test]
    fn a_restore_cut_short_is_told_of() {
        let (dir, files) = raw_history("cut-short");
        files.load();
        let restored: Vec<Record> = (1..=3).map(|n| record(&finished(n * 1_000))).collect();
        let journal = files.begin_journal(&restore(&restored)).expect("a journal");
        files.place_record(&restored[0]).unwrap();
        // Another copy reads the history while this one stores.
        let other = HistoryFiles::at(&dir.0).load();
        assert!(other.notices.is_empty(), "still running");
        // And this one stops.
        drop(journal);
        let next = HistoryFiles::at(&dir.0).load();
        assert_eq!(
            next.notices,
            [HistoryNotice::CutShort {
                meant: 3,
                stored: 1
            }]
        );
        assert!(HistoryFiles::at(&dir.0).load().notices.is_empty(), "once");
        // Made whole, a restore leaves no journal behind.
        assert_eq!(
            files.apply(&restore(&restored)),
            HistorySaved::Stored { kept: 3 }
        );
        assert!(journals(&dir).is_empty(), "{:?}", journals(&dir));
        assert_eq!(files.load().sessions.len(), 3);
    }

    /// A restore that fails, the store full or a name in the way, is said
    /// to have failed then and there, and leaves no journal: the next to
    /// read the history is not told that Dust stopped, as it never did.
    #[test]
    fn a_restore_that_fails_is_not_told_of_as_cut_short() {
        let (dir, files) = history("restore-failed");
        files.load();
        let restored: Vec<Record> = (1..=3).map(|n| record(&finished(n * 1_000))).collect();
        let records = dir.0.join(HISTORY).join(RECORDS);
        fs::create_dir_all(records.join(file_name(restored[1].id))).unwrap();
        assert!(
            matches!(files.apply(&restore(&restored)), HistorySaved::Failed(_)),
            "a name in the way"
        );
        assert!(journals(&dir).is_empty(), "{:?}", journals(&dir));
        fs::remove_dir(records.join(file_name(restored[1].id))).unwrap();
        let read = Files::at(&dir.0).load();
        assert!(read.notices.is_empty(), "{:?}", read.notices);
        assert_eq!(stamps(&read), [1_000]);
        // Restored again, the rest is stored, each session once.
        assert_eq!(
            files.apply(&restore(&restored)),
            HistorySaved::Stored { kept: 3 }
        );
        assert_eq!(stamps(&files.load()), [1_000, 2_000, 3_000]);
    }

    /// A restore cut short counts as stored what it left deleted by hand,
    /// as it was asked to, and not what it was putting back.
    #[test]
    fn a_restore_cut_short_counts_what_it_left_deleted_as_done() {
        let (dir, files) = raw_history("cut-short-deleted");
        files.load();
        let sessions: Vec<SessionResult> = (1..=4).map(|n| finished(n * 1_000)).collect();
        let restored: Vec<Record> = sessions.iter().map(record).collect();
        files.apply(&put(&restored[..2]));
        files.apply(&forget(&[
            (&restored[0], &sessions[0]),
            (&restored[1], &sessions[1]),
        ]));
        // The first stays deleted; the second is put back; neither is
        // stored yet, nor the last two, as Dust stops.
        let change = HistoryChange {
            unforget: vec![restored[1].id],
            ..restore(&restored)
        };
        drop(files.begin_journal(&change).expect("a journal"));
        let next = HistoryFiles::at(&dir.0).load();
        assert_eq!(
            next.notices,
            [HistoryNotice::CutShort {
                meant: 4,
                stored: 1
            }]
        );
    }

    /// Only a restore of many sessions at once is journaled: not one
    /// session, nor many the app stores of its own, nor what is taken up
    /// from an older build, which is taken up again if it is cut short.
    #[test]
    fn only_a_restore_of_many_sessions_is_journaled() {
        let records: Vec<Record> = (1..=2).map(|n| record(&finished(n * 1_000))).collect();
        assert!(restores_many(&restore(&records)));
        assert!(!restores_many(&restore(&records[..1])));
        assert!(!restores_many(&put(&records)));

        let dir = TempDir::new("import-unjournaled");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = HistoryFiles::at(&dir.0);
        files.load();
        // An older build stores two sessions: taken up together, and no
        // restore.
        let (first, second) = (finished(now + 60_000), finished(now + 120_000));
        let whole = [old.clone(), first.clone(), second.clone()];
        keep_whole(&dir, &whole);
        let Ok(Gate::Open(layout)) = files.gate() else {
            panic!("not moved");
        };
        let live = files.live_keys().expect("listed");
        let fingerprint = fingerprint_of(&dir.0.join(SESSIONS)).unwrap();
        let (change, _) =
            legacy::imported(&layout, fingerprint, &whole, &[], &live, &HashSet::new())
                .expect("something new");
        assert_eq!(change.put.len(), 2);
        assert!(!restores_many(&change));
        // The second cannot be taken up for now: the take-up is tried
        // again, and leaves nothing to tell of.
        let in_the_way = change.put[1].id;
        let records = dir.0.join(HISTORY).join(RECORDS);
        fs::create_dir_all(records.join(file_name(in_the_way))).unwrap();
        assert_eq!(files.import(&layout, &live), None, "tried again later");
        assert!(journals(&dir).is_empty(), "{:?}", journals(&dir));
        fs::remove_dir(records.join(file_name(in_the_way))).unwrap();
        let read = HistoryFiles::at(&dir.0).load();
        assert!(read.notices.is_empty(), "{:?}", read.notices);
        assert_eq!(
            stamps(&read),
            [old.timestamp, first.timestamp, second.timestamp]
        );
    }

    /// A history a newer build keeps in a way this one would harm is read,
    /// and nothing is written to it, not even to tidy it.
    #[test]
    fn a_store_from_a_newer_incompatible_layout_is_only_read() {
        let (dir, files) = history("newer");
        files.load();
        let first = finished(1_000);
        let (one, two) = (record(&first), record(&finished(2_000)));
        files.apply(&put(&[one.clone(), two.clone()]));
        let layout = dir.0.join(HISTORY).join(LAYOUT_FILE);
        let newer = r#"{"layout":3,"compatible":2,"migratedAt":1,"shards":[1]}"#;
        fs::write(&layout, newer).unwrap();
        // A deletion cut short, and a write left behind long ago.
        let tombstones = dir.0.join(HISTORY).join(TOMBSTONES);
        fs::create_dir_all(&tombstones).unwrap();
        fs::write(tombstones.join(file_name(one.id)), "").unwrap();
        let old = dir.0.join(HISTORY).join(".x.tmp");
        fs::write(&old, "half").unwrap();
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - 2 * OLD_TEMPORARY)
            .unwrap();
        let before = everything(&dir.0);
        let read = Files::at(&dir.0).load();
        assert_eq!(ids(&read), [two.id]);
        assert_eq!(read.notices, [HistoryNotice::Newer]);
        assert!(!read.complete);
        let third = record(&finished(3_000));
        assert_eq!(
            files.apply(&put(std::slice::from_ref(&third))),
            HistorySaved::Newer
        );
        assert_eq!(everything(&dir.0), before, "nothing written");
    }

    /// A layout a later build wrote, in a way this one may still write to,
    /// keeps all it holds as this one notes what it took up: its own
    /// number, and what this one does not know.
    #[test]
    fn a_later_compatible_layout_keeps_what_it_added() {
        let dir = TempDir::new("layout-later");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        Files::at(&dir.0).load();
        let path = dir.0.join(HISTORY).join(LAYOUT_FILE);
        let read_layout =
            || -> serde_json::Value { serde_json::from_slice(&fs::read(&path).unwrap()).unwrap() };
        let mut later = read_layout();
        later["layout"] = 2.into();
        later["shards"] = serde_json::json!([1]);
        fs::write(&path, serde_json::to_vec(&later).unwrap()).unwrap();
        // An older build stores a session, taken up at the next launch,
        // and another, taken up as the history is next listed.
        let (first, second) = (finished(now + 60_000), finished(now + 120_000));
        keep_whole(&dir, &[old.clone(), first.clone()]);
        let files = Files::at(&dir.0);
        let read = files.load();
        assert!(read.complete && read.notices.is_empty());
        assert_eq!(stamps(&read), [old.timestamp, first.timestamp]);
        keep_whole(&dir, &[old.clone(), first.clone(), second.clone()]);
        assert_eq!(files.listing().map(|ids| ids.len()), Some(3));
        let noted = read_layout();
        assert_eq!(noted["layout"], 2);
        assert_eq!(noted["compatible"], 1);
        assert_eq!(noted["shards"], serde_json::json!([1]));
        assert_eq!(noted["migratedAt"], later["migratedAt"]);
        let taken: Vec<SessionKey> = serde_json::from_value(noted["importedKeys"].clone()).unwrap();
        assert_eq!(
            taken,
            [&old, &first, &second].map(SessionKey::of),
            "what was taken up, noted"
        );
    }

    /// The history an older build kept whole becomes records, once, in its
    /// order; the move notes what it took, and is not made again.
    #[test]
    fn a_history_file_becomes_records_once() {
        let dir = TempDir::new("moved");
        let whole = [finished(5), finished(3), finished(9)];
        keep_whole(&dir, &whole);
        let gone = Deleted::of(&finished(1), 77);
        fs::write(dir.0.join(DELETED), serde_json::to_vec(&[gone]).unwrap()).unwrap();
        let files = Files::at(&dir.0);
        let read = files.load();
        assert!(read.complete);
        assert!(read.set_aside.is_empty() && read.notices.is_empty());
        let records = records_for_blob(&whole);
        assert_eq!(
            read.sessions,
            records
                .iter()
                .map(|record| record.id)
                .zip(whole.iter().cloned())
                .collect::<Vec<_>>()
        );
        let layout: Layout =
            serde_json::from_slice(&fs::read(dir.0.join(HISTORY).join(LAYOUT_FILE)).unwrap())
                .unwrap();
        assert_eq!(
            (layout.layout, layout.compatible),
            (legacy::LAYOUT, legacy::COMPATIBLE)
        );
        assert_eq!(layout.imported_keys.len(), 3);
        assert_eq!(layout.deleted_seen, 77);
        assert_eq!(
            layout.legacy,
            fingerprint_of(&dir.0.join(SESSIONS)).unwrap()
        );
        let before = everything(&dir.0);
        let again = Files::at(&dir.0).load();
        assert_eq!(again.sessions, read.sessions);
        assert_eq!(everything(&dir.0), before, "made once");
    }

    /// A move cut short, before its layout was written, is made again to
    /// the same records, byte for byte.
    #[test]
    fn a_migration_cut_short_runs_again_to_the_same_records() {
        let dir = TempDir::new("move-again");
        keep_whole(&dir, &[finished(5), finished(3), finished(9)]);
        Files::at(&dir.0).load();
        let moved = everything(&dir.0.join(HISTORY).join(RECORDS));
        // Cut short: one record and the layout never reached the disk.
        fs::remove_file(dir.0.join(HISTORY).join(LAYOUT_FILE)).unwrap();
        fs::remove_file(&moved[1].0).unwrap();
        let read = Files::at(&dir.0).load();
        assert_eq!(stamps(&read), [5, 3, 9]);
        assert_eq!(everything(&dir.0.join(HISTORY).join(RECORDS)), moved);
        assert!(dir.0.join(HISTORY).join(LAYOUT_FILE).exists());
    }

    /// A history of three thousand sessions kept whole is moved to records
    /// in a moment, the first time this layout opens: the app waits on it,
    /// its boot screen up.
    #[test]
    #[ignore = "a timing, on a disk of its own: run by hand"]
    fn three_thousand_sessions_kept_whole_move_in_a_moment() {
        let dir = TempDir::new("move-timing");
        let whole: Vec<SessionResult> = (1..=3_000).map(|n| finished(n * 60_000)).collect();
        keep_whole(&dir, &whole);
        let started = std::time::Instant::now();
        let read = Files::at(&dir.0).load();
        let took = started.elapsed();
        assert_eq!(read.sessions.len(), whole.len());
        assert!(read.complete);
        eprintln!("3,000 sessions moved in {took:?}");
        assert!(took < Duration::from_secs(1), "{took:?}");
    }

    /// A history file that is no list at all is set aside as it always
    /// was, and nothing is made of it; one only some of whose sessions do
    /// not read has those copied aside, and the rest moved.
    #[test]
    fn an_unreadable_history_file_is_set_aside_and_not_migrated() {
        let dir = TempDir::new("move-unreadable");
        fs::create_dir_all(&dir.0).unwrap();
        let damaged = "{\"this\": \"is not a history\"";
        fs::write(dir.0.join(SESSIONS), damaged).unwrap();
        let read = Files::at(&dir.0).load();
        assert!(read.sessions.is_empty() && read.complete);
        let [SetAside::History { kept: Some(kept) }] = read.set_aside.as_slice() else {
            panic!("{:?}", read.set_aside);
        };
        assert!(kept.contains("sessions.unreadable-"), "{kept}");
        assert_eq!(fs::read_to_string(kept).unwrap(), damaged);
        assert!(!dir.0.join(SESSIONS).exists());
        assert!(names(&dir.0.join(HISTORY).join(RECORDS)).is_empty());
        assert!(Files::at(&dir.0).load().set_aside.is_empty(), "once");

        let dir = TempDir::new("move-part-unreadable");
        fs::create_dir_all(&dir.0).unwrap();
        let good = |at| serde_json::to_string(&finished(at)).unwrap();
        let raw = format!("[{},{{\"date\":\"nonsense\"}},{}]", good(1), good(2));
        fs::write(dir.0.join(SESSIONS), &raw).unwrap();
        let read = Files::at(&dir.0).load();
        assert_eq!(stamps(&read), [1, 2]);
        let [
            SetAside::Sessions {
                count: 1,
                kept: Some(kept),
            },
        ] = read.set_aside.as_slice()
        else {
            panic!("{:?}", read.set_aside);
        };
        assert_eq!(
            fs::read_to_string(kept).unwrap(),
            r#"[{"date":"nonsense"}]"#
        );
        assert_eq!(fs::read_to_string(dir.0.join(SESSIONS)).unwrap(), raw);
    }

    /// A history file that cannot be read for now leaves the move for the
    /// next launch, and says so; sessions are stored all the same, and the
    /// move made later takes up both.
    #[test]
    fn a_history_that_cannot_be_read_now_is_migrated_later() {
        let dir = TempDir::new("move-later");
        fs::create_dir_all(dir.0.join(SESSIONS)).unwrap();
        let files = Files::at(&dir.0);
        let read = files.load();
        assert!(!read.complete);
        assert!(
            matches!(read.notices.as_slice(), [HistoryNotice::MovePending(_)]),
            "{:?}",
            read.notices
        );
        assert!(!dir.0.join(HISTORY).join(LAYOUT_FILE).exists());
        let new = record(&finished(9_000));
        assert_eq!(
            files.apply(&put(std::slice::from_ref(&new))),
            HistorySaved::Stored { kept: 1 }
        );
        fs::remove_dir(dir.0.join(SESSIONS)).unwrap();
        keep_whole(&dir, &[finished(1_000), finished(2_000)]);
        let read = Files::at(&dir.0).load();
        assert!(read.complete && read.notices.is_empty());
        assert_eq!(stamps(&read), [1_000, 2_000, 9_000]);
    }

    /// What an older build keeps — its history, its deletions, and what it
    /// set aside — is left as it was, byte for byte, by the move and every
    /// change after it.
    #[test]
    fn the_old_history_file_is_left_for_older_builds() {
        let dir = TempDir::new("left-for-older");
        let whole = [finished(1_000), finished(2_000)];
        keep_whole(&dir, &whole);
        let deleted = serde_json::to_vec(&[Deleted::of(&finished(1), 5)]).unwrap();
        fs::write(dir.0.join(DELETED), &deleted).unwrap();
        fs::write(dir.0.join("sessions.unreadable-1.json"), "[{}]").unwrap();
        let older = |dir: &TempDir| {
            [SESSIONS, DELETED, "sessions.unreadable-1.json"]
                .map(|name| fs::read(dir.0.join(name)).unwrap())
        };
        let before = older(&dir);
        let files = Files::at(&dir.0);
        let read = files.load();
        files.apply(&put(&[record(&finished(3_000))]));
        let (id, first) = &read.sessions[0];
        let first_record = Record {
            id: *id,
            json: serde_json::to_string(first).unwrap(),
        };
        files.apply(&forget(&[(&first_record, first)]));
        Files::at(&dir.0).load();
        assert_eq!(older(&dir), before);
    }

    /// Sessions an older build stores in the history it keeps whole, after
    /// the move, are taken up: at the next launch, and as the history is
    /// next listed.
    #[test]
    fn sessions_an_older_build_added_after_the_move_are_taken_up() {
        let dir = TempDir::new("older-added");
        let now = crate::time::now_ms();
        let old = [finished(now - 10 * DAY), finished(now - 9 * DAY)];
        keep_whole(&dir, &old);
        let files = Files::at(&dir.0);
        files.load();
        // An older build, still open, stores a session.
        let first = finished(now + 60_000);
        keep_whole(&dir, &[old[0].clone(), old[1].clone(), first.clone()]);
        let read = Files::at(&dir.0).load();
        assert_eq!(
            stamps(&read),
            [old[0].timestamp, old[1].timestamp, first.timestamp]
        );
        let placed = Record::of(&first, |json| RecordId::placed(first.timestamp, json)).unwrap();
        assert_eq!(read.sessions[2].0, placed.id);
        // And another, taken up as the history is next listed.
        let second = finished(now + 120_000);
        keep_whole(
            &dir,
            &[old[0].clone(), old[1].clone(), first, second.clone()],
        );
        assert!(files.changed());
        let listed = files.listing().expect("a listing");
        assert_eq!(listed.len(), 4);
        let taken = listed.last().copied().expect("the newest");
        assert_eq!(
            files
                .read(&[taken])
                .pop()
                .and_then(Result::ok)
                .and_then(|record| read_record(&record.json)),
            Some(second)
        );
    }

    /// A session an older build stored in the history kept whole after a
    /// move made long after the sessions it took, which a build before this
    /// one's take-up rules looked at first, is taken up as the history is
    /// next read here, and as it is next listed. Such a build (0.16.9)
    /// takes up by its own rules, which leave it out, and writes the layout
    /// back with `legacy` moved on to the history kept whole as it took it,
    /// `takenLegacy`, this build's note, left as it was. Gated by `legacy`,
    /// the change looked taken up already, and imported() was never asked.
    #[test]
    fn a_change_an_older_build_looked_at_first_is_taken_up_as_the_history_is_read() {
        let dir = TempDir::new("older-looked-first");
        let now = crate::time::now_ms();
        let old = [finished(now - 30 * DAY), finished(now - 20 * DAY)];
        keep_whole(&dir, &old);
        Files::at(&dir.0).load();
        let older_looks = || {
            let path = dir.0.join(HISTORY).join(LAYOUT_FILE);
            let mut layout: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let whole = fingerprint_of(&dir.0.join(SESSIONS)).unwrap();
            layout["legacy"] = serde_json::to_value(whole).unwrap();
            fs::write(&path, serde_json::to_vec_pretty(&layout).unwrap()).unwrap();
        };
        // A copy of a build before records, left open, stores a session it
        // finished ten days ago, and an older build looks first.
        let since = finished(now - 10 * DAY);
        keep_whole(&dir, &[old[0].clone(), old[1].clone(), since.clone()]);
        older_looks();
        assert_eq!(
            stamps(&Files::at(&dir.0).load()),
            [old[0].timestamp, old[1].timestamp, since.timestamp],
            "taken up as the history is read"
        );
        // Another, as the history is next listed.
        let files = Files::at(&dir.0);
        files.load();
        let later = finished(now - 9 * DAY);
        keep_whole(&dir, &[old[0].clone(), old[1].clone(), since, later]);
        older_looks();
        assert_eq!(
            files.listing().expect("a listing").len(),
            4,
            "and as it is listed"
        );
    }

    /// The same where a build before this one's take-up rules made the move
    /// as well, as every build from 0.16.3 to 0.16.9 did: its layout holds
    /// none of this build's notes (`takenLegacy` among them), and the change
    /// it looked at first is taken up as the history is next read here.
    /// Gated by `legacy` where `takenLegacy` is missing, the change looked
    /// taken up already, though imported() would take it up.
    #[test]
    fn a_change_an_older_build_looked_at_first_is_taken_up_where_it_made_the_move() {
        let dir = TempDir::new("older-moved-looked-first");
        let now = crate::time::now_ms();
        let old = [finished(now - 30 * DAY), finished(now - 20 * DAY)];
        keep_whole(&dir, &old);
        Files::at(&dir.0).load();
        // The layout as 0.16.9 writes it as it moves the history and as it
        // looks: `legacy` moved on to the history kept whole as it took it.
        let as_an_older_build_wrote_it = || {
            let path = dir.0.join(HISTORY).join(LAYOUT_FILE);
            let mut layout: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let held = layout.as_object_mut().expect("an object");
            for key in ["takenThrough", "deletionsTaken", "takenLegacy"] {
                held.remove(key);
            }
            let whole = fingerprint_of(&dir.0.join(SESSIONS)).unwrap();
            layout["legacy"] = serde_json::to_value(whole).unwrap();
            fs::write(&path, serde_json::to_vec_pretty(&layout).unwrap()).unwrap();
        };
        as_an_older_build_wrote_it();
        // A copy of a build before records, left open, stores a session it
        // finished ten days ago, and the older build looks first.
        let since = finished(now - 10 * DAY);
        keep_whole(&dir, &[old[0].clone(), old[1].clone(), since.clone()]);
        as_an_older_build_wrote_it();
        assert_eq!(
            stamps(&Files::at(&dir.0).load()),
            [old[0].timestamp, old[1].timestamp, since.timestamp],
            "taken up as the history is read"
        );
    }

    /// A session a record here holds is not taken up from the history an
    /// older build keeps whole while that record cannot be read for now, as
    /// a file another program holds open cannot be: the record may be that
    /// session. Once it reads, the session is there once. The take-up left
    /// out every record that did not read, and stored the session a second
    /// time beside it.
    #[test]
    fn a_session_a_record_here_holds_is_not_taken_up_from_an_older_build_while_that_record_cannot_be_read()
     {
        let dir = TempDir::new("older-busy");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = Files::at(&dir.0);
        files.load();
        // Finished here after the move, and held by an older build too.
        let mine = finished(now + 60_000);
        let kept = record(&mine);
        files.apply(&put(std::slice::from_ref(&kept)));
        keep_whole(&dir, &[old.clone(), mine.clone()]);
        // A folder where its file goes, which cannot be read as one.
        let records = dir.0.join(HISTORY).join(RECORDS);
        let path = records.join(file_name(kept.id));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        // A copy opened since, which has not read it.
        let since = Files::at(&dir.0);
        let read = since.load();
        assert_eq!(stamps(&read), [old.timestamp], "nothing taken up");
        assert_eq!(read.failed, [kept.id]);
        assert_eq!(names(&records).len(), 2, "nothing stored");
        assert_eq!(
            since.listing().map(|listed| listed.len()),
            Some(2),
            "nor as it is listed"
        );
        assert_eq!(names(&records).len(), 2);

        // It reads again.
        fs::remove_dir(&path).unwrap();
        fs::write(&path, &kept.json).unwrap();
        let read = Files::at(&dir.0).load();
        assert_eq!(stamps(&read), [old.timestamp, mine.timestamp], "once");
        assert_eq!(names(&records).len(), 2);
        assert_eq!(files.listing().map(|listed| listed.len()), Some(2));
    }

    /// A session a later build packed here is not taken up from the history
    /// an older build keeps whole while its pack cannot be opened, though
    /// this copy never opened that pack, and knows nothing of what it holds:
    /// it may hold that session. Once it opens, the session is there once.
    /// A pack this copy had never opened hid its records altogether, and the
    /// take-up stored the session a second time beside it.
    #[cfg(unix)]
    #[test]
    fn a_session_a_pack_here_holds_is_not_taken_up_from_an_older_build_while_that_pack_cannot_be_opened()
     {
        let dir = TempDir::new("older-pack-unopened");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = Files::at(&dir.0);
        files.load();
        // Finished here after the move, packed by a later build since, and
        // held by an older build too.
        let mine = finished(now + 60_000);
        let kept = record(&mine);
        files.apply(&put(std::slice::from_ref(&kept)));
        pack_away(&dir.0, "packed.jsonl.gz", &[&kept]);
        keep_whole(&dir, &[old.clone(), mine.clone()]);
        // The pack will not open, for a copy opened since, which never
        // opened it.
        let packed = dir.0.join(HISTORY).join(PACKS).join("packed.jsonl.gz");
        let held = fs::read(&packed).unwrap();
        fs::remove_file(&packed).unwrap();
        std::os::unix::fs::symlink(dir.0.join("nowhere"), &packed).unwrap();
        let records = dir.0.join(HISTORY).join(RECORDS);
        let since = Files::at(&dir.0);
        let read = since.load();
        assert_eq!(stamps(&read), [old.timestamp], "nothing taken up");
        assert_eq!(names(&records).len(), 1, "nothing stored");
        assert_eq!(
            since.listing().map(|listed| listed.len()),
            Some(1),
            "nor as it is listed"
        );
        assert_eq!(names(&records).len(), 1);

        // It opens again.
        fs::remove_file(&packed).unwrap();
        fs::write(&packed, held).unwrap();
        let read = Files::at(&dir.0).load();
        assert_eq!(stamps(&read), [old.timestamp, mine.timestamp], "once");
        assert_eq!(names(&records).len(), 1);
    }

    /// A session a later build stored here, in a record this build cannot
    /// read that says which session it holds, is not taken up a second time
    /// from the history an older build keeps whole, as the history is read
    /// whole nor as it is listed. Read whole, the take-up left that record
    /// out, and stored the session again beside it.
    #[test]
    fn a_session_a_later_build_stored_here_is_not_taken_up_from_an_older_build() {
        let dir = TempDir::new("older-later");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = Files::at(&dir.0);
        files.load();
        let theirs = finished(now + 60_000);
        let later = format!(
            r#"{{"format":{},"date":{:?},"timestamp":{},"by":"a later build"}}"#,
            cw_core::SESSION_FORMAT + 1,
            theirs.date,
            theirs.timestamp
        );
        let id = RecordId::next(theirs.timestamp, None, &later);
        let records = dir.0.join(HISTORY).join(RECORDS);
        fs::write(records.join(file_name(id)), &later).unwrap();
        keep_whole(&dir, &[old.clone(), theirs.clone()]);
        let read = Files::at(&dir.0).load();
        assert_eq!(stamps(&read), [old.timestamp], "not taken up");
        assert_eq!(read.later, [(id, SessionKey::of(&theirs))]);
        assert_eq!(names(&records).len(), 2, "nothing stored");
        keep_whole(&dir, &[old, theirs, finished(now + 120_000)]);
        assert_eq!(files.listing().map(|listed| listed.len()), Some(3));
        assert_eq!(
            names(&records).len(),
            3,
            "only the one an older build added"
        );
    }

    /// A session whose record here is damaged, JSON that still says when
    /// and on what day it finished but that no build reads, is taken up
    /// beside it from the history an older build keeps whole, as the history
    /// is read whole and as it is listed: the older build's copy may be the
    /// only good one, as a restore takes a backup's. The record is left as
    /// it is, and the older build's deletion of the session leaves it too.
    /// The take-up took the damaged record for the session, left the good
    /// copy out, and deleted the record with the session.
    #[test]
    fn a_session_whose_record_here_is_damaged_is_taken_up_from_an_older_build_beside_it() {
        let dir = TempDir::new("older-damaged");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = Files::at(&dir.0);
        files.load();
        let records = dir.0.join(HISTORY).join(RECORDS);
        let damage = |session: &SessionResult| {
            let raw = format!(
                r#"{{"date":{:?},"timestamp":{},"by":"damage"}}"#,
                session.date, session.timestamp
            );
            let id = RecordId::next(session.timestamp, None, &raw);
            fs::write(records.join(file_name(id)), &raw).unwrap();
            id
        };
        let mine = finished(now + 60_000);
        let damaged = damage(&mine);
        keep_whole(&dir, &[old.clone(), mine.clone()]);
        let read = Files::at(&dir.0).load();
        assert_eq!(
            stamps(&read),
            [old.timestamp, mine.timestamp],
            "taken up beside it"
        );
        assert_eq!(read.unread, [damaged], "left as it is");
        // Another, damaged once this copy read the history, which an older
        // build adds after.
        let since = finished(now + 120_000);
        let damaged_since = damage(&since);
        keep_whole(&dir, &[old.clone(), mine.clone(), since.clone()]);
        assert_eq!(
            files.listing().map(|listed| listed.len()),
            Some(5),
            "as it is listed"
        );
        // The older build deletes the first: its copy taken up goes, and
        // the damaged record stays, with no tombstone, as the next read
        // finds it.
        let gone = [Deleted::of(&mine, now + 180_000)];
        fs::write(dir.0.join(DELETED), serde_json::to_vec(&gone).unwrap()).unwrap();
        keep_whole(&dir, &[old.clone(), since.clone()]);
        let deleting = Files::at(&dir.0);
        let read = deleting.load();
        assert_eq!(stamps(&read), [old.timestamp, since.timestamp]);
        assert_eq!(deleting.forgotten(&[damaged]), [], "no tombstone");
        let read = Files::at(&dir.0).load();
        assert_eq!(read.unread, [damaged, damaged_since], "left as they are");
    }

    /// A session from before the move that an older copy writes back, as
    /// one left open stores the history it held, is never brought back:
    /// neither one it let go long ago, nor one deleted here since.
    #[test]
    fn sessions_from_before_the_move_are_never_brought_back() {
        let dir = TempDir::new("never-back");
        let now = crate::time::now_ms();
        let kept = [finished(now - 10 * DAY), finished(now - 9 * DAY)];
        keep_whole(&dir, &kept);
        let files = Files::at(&dir.0);
        let read = files.load();
        let (id, first) = &read.sessions[0];
        let first_record = Record {
            id: *id,
            json: serde_json::to_string(first).unwrap(),
        };
        files.apply(&forget(&[(&first_record, first)]));
        let let_go = finished(now - 30 * DAY);
        keep_whole(&dir, &[let_go, kept[0].clone(), kept[1].clone()]);
        let read = Files::at(&dir.0).load();
        assert_eq!(stamps(&read), [kept[1].timestamp]);
    }

    /// A session taken up from an older build is taken up once: not again
    /// however often the older build stores it, nor once deleted here.
    #[test]
    fn a_session_imported_once_is_never_imported_again() {
        let dir = TempDir::new("imported-once");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        Files::at(&dir.0).load();
        let (first, second) = (finished(now + 60_000), finished(now + 120_000));
        keep_whole(&dir, &[old.clone(), first.clone()]);
        let files = Files::at(&dir.0);
        let read = files.load();
        assert_eq!(stamps(&read), [old.timestamp, first.timestamp]);
        // Stored again by the older build, with another: the first once.
        keep_whole(&dir, &[old.clone(), first.clone(), second.clone()]);
        let read = Files::at(&dir.0).load();
        assert_eq!(
            stamps(&read),
            [old.timestamp, first.timestamp, second.timestamp]
        );
        // Deleted here, and stored again there: still deleted.
        let (id, _) = &read.sessions[1];
        let taken = Record {
            id: *id,
            json: serde_json::to_string(&first).unwrap(),
        };
        files.apply(&forget(&[(&taken, &first)]));
        keep_whole(&dir, &[old.clone(), first, second.clone()]);
        let read = Files::at(&dir.0).load();
        assert_eq!(stamps(&read), [old.timestamp, second.timestamp]);
    }

    /// A session an older build deletes after the move is deleted here too;
    /// a deletion it made before is not made again, of a session restored
    /// here since.
    #[test]
    fn a_deletion_an_older_build_made_after_the_move_is_honoured() {
        let dir = TempDir::new("older-deleted");
        let now = crate::time::now_ms();
        let (first, second) = (finished(now - 10 * DAY), finished(now - 9 * DAY));
        keep_whole(&dir, &[first.clone(), second.clone()]);
        // Deleted before the move, and since restored: kept.
        let before = Deleted::of(&first, now - 20 * DAY);
        fs::write(
            dir.0.join(DELETED),
            serde_json::to_vec(std::slice::from_ref(&before)).unwrap(),
        )
        .unwrap();
        let read = Files::at(&dir.0).load();
        let second_id = read.sessions[1].0;
        // The older build deletes the second.
        let after = Deleted::of(&second, now);
        keep_whole(&dir, std::slice::from_ref(&first));
        fs::write(
            dir.0.join(DELETED),
            serde_json::to_vec(&[before, after.clone()]).unwrap(),
        )
        .unwrap();
        let files = Files::at(&dir.0);
        let read = files.load();
        assert_eq!(stamps(&read), [first.timestamp]);
        assert_eq!(files.forgotten(&[second_id]), [second_id]);
        let held: Deleted = serde_json::from_slice(
            &fs::read(
                dir.0
                    .join(HISTORY)
                    .join(TOMBSTONES)
                    .join(file_name(second_id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(held, after);
        assert_eq!(stamps(&Files::at(&dir.0).load()), [first.timestamp]);
    }

    /// A deletion an older build makes after the move is made here,
    /// whatever its clock says: here, behind a deletion a clock set a year
    /// ahead stamped before the move. Only one stamped past every deletion
    /// taken before was made, so it never was.
    #[test]
    fn a_deletion_an_older_build_made_after_the_move_is_honoured_whatever_its_clock() {
        let dir = TempDir::new("older-deleted-clock");
        let now = crate::time::now_ms();
        let (first, second) = (finished(now - 10 * DAY), finished(now - 9 * DAY));
        keep_whole(&dir, &[first.clone(), second.clone()]);
        // Made before the move, of a session gone since, by a clock a year
        // ahead.
        let ahead = Deleted::of(&finished(now - 40 * DAY), now + 365 * DAY);
        fs::write(
            dir.0.join(DELETED),
            serde_json::to_vec(std::slice::from_ref(&ahead)).unwrap(),
        )
        .unwrap();
        let read = Files::at(&dir.0).load();
        let second_id = read.sessions[1].0;
        // The older build deletes the second, by a clock that is right.
        let after = Deleted::of(&second, now - 30 * DAY);
        keep_whole(&dir, std::slice::from_ref(&first));
        fs::write(
            dir.0.join(DELETED),
            serde_json::to_vec(&[ahead, after]).unwrap(),
        )
        .unwrap();
        let files = Files::at(&dir.0);
        assert_eq!(stamps(&files.load()), [first.timestamp]);
        assert_eq!(files.forgotten(&[second_id]), [second_id]);
    }

    /// A move an older build cut short, its records written as it writes
    /// them, with no format, and no layout yet, is made again over them
    /// writing no session twice; and so is a session an older build took
    /// up since, stored as it writes one, when the move is made again over
    /// a layout that no longer reads. This build writes the format in
    /// every session ([`cw_core::SESSION_FORMAT`]), so the move's own
    /// bytes are never an older build's, and a record of each was written
    /// beside the older build's, every session twice.
    #[test]
    fn a_move_made_again_over_what_an_older_build_stored_writes_no_session_twice() {
        let dir = TempDir::new("move-again-older");
        let now = crate::time::now_ms();
        let held = [finished(now - 9 * DAY), finished(now - 8 * DAY)];
        keep_whole(&dir, &held);
        let records = dir.0.join(HISTORY).join(RECORDS);
        fs::create_dir_all(&records).unwrap();
        let mut last = None;
        for session in &held {
            let older = as_an_older_build_wrote(session);
            let id = RecordId::next(session.timestamp, last, &older);
            fs::write(records.join(file_name(id)), &older).unwrap();
            last = Some(id);
        }
        let moved = Files::at(&dir.0).load();
        assert!(moved.complete);
        assert_eq!(moved.sessions.len(), 2, "each once");
        assert_eq!(names(&records).len(), 2, "no record written twice");
        // An older build took up a session stored whole since, as it
        // stores one, and the layout was damaged.
        let third = finished(now - 7 * DAY);
        let older = as_an_older_build_wrote(&third);
        let taken = RecordId::placed(third.timestamp, &older);
        fs::write(records.join(file_name(taken)), &older).unwrap();
        keep_whole(&dir, &[held[0].clone(), held[1].clone(), third]);
        fs::write(dir.0.join(HISTORY).join(LAYOUT_FILE), "{ damaged").unwrap();
        let again = Files::at(&dir.0).load();
        assert!(again.complete);
        assert_eq!(again.sessions.len(), 3, "each once");
        assert_eq!(names(&records).len(), 3, "no record written twice");
    }

    /// A layout that does not read, damaged where it lies, is set aside,
    /// and the move made again writes nothing twice.
    #[test]
    fn a_layout_that_does_not_read_is_set_aside_and_the_move_made_again() {
        let dir = TempDir::new("layout-damaged");
        let now = crate::time::now_ms();
        let (first, second) = (finished(now - 9 * DAY), finished(now - 8 * DAY));
        keep_whole(&dir, &[first.clone(), second.clone()]);
        let files = Files::at(&dir.0);
        let moved = files.load();
        // Since the move: an older build stored two sessions, taken up
        // here, and the first moved and the last taken up were deleted here.
        let (third, fourth) = (finished(now + 60_000), finished(now + 120_000));
        keep_whole(&dir, &[first.clone(), second, third, fourth.clone()]);
        let taken = files.load();
        assert_eq!(taken.sessions.len(), 4);
        let as_stored = |(id, session): &(RecordId, SessionResult)| Record {
            id: *id,
            json: serde_json::to_string(session).unwrap(),
        };
        let (moved_first, taken_last) =
            (as_stored(&moved.sessions[0]), as_stored(&taken.sessions[3]));
        files.apply(&forget(&[(&moved_first, &first), (&taken_last, &fourth)]));
        let read = files.load();
        assert_eq!(read.sessions.len(), 2);
        let records = everything(&dir.0.join(HISTORY).join(RECORDS));
        fs::write(dir.0.join(HISTORY).join(LAYOUT_FILE), "{ damaged").unwrap();
        let again = Files::at(&dir.0).load();
        assert!(again.complete);
        assert_eq!(again.sessions, read.sessions, "none twice, none back");
        assert_eq!(everything(&dir.0.join(HISTORY).join(RECORDS)), records);
        let aside: Vec<String> = names(&dir.0.join(HISTORY))
            .into_iter()
            .filter(|name| name.starts_with("layout.unreadable-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(
            fs::read_to_string(dir.0.join(HISTORY).join(&aside[0])).unwrap(),
            "{ damaged"
        );
    }

    /// Write a pack of `records` in the history in `root`, as a later
    /// build packs them, with `extra` lines after them.
    fn pack(root: &Path, name: &str, records: &[&Record], extra: &[&str]) -> PathBuf {
        use flate2::write::GzEncoder;
        let packs = root.join(HISTORY).join(PACKS);
        fs::create_dir_all(&packs).unwrap();
        let mut lines = String::new();
        for record in records {
            let line = serde_json::json!({ "id": record.id.name(), "s": record.json });
            lines.push_str(&line.to_string());
            lines.push('\n');
        }
        for line in extra {
            lines.push_str(line);
            lines.push('\n');
        }
        let path = packs.join(name);
        let mut packing = GzEncoder::new(Vec::new(), flate2::Compression::default());
        packing.write_all(lines.as_bytes()).unwrap();
        fs::write(&path, packing.finish().unwrap()).unwrap();
        path
    }

    /// Records a later build packed together are read as records: one both
    /// packed and loose is one record, a tombstone deletes one packed, and
    /// the pack is never written.
    #[test]
    fn a_pack_and_a_loose_copy_of_one_record_read_as_one() {
        let (dir, files) = history("packs");
        files.load();
        let (first, second, third) = (finished(1_000), finished(2_000), finished(3_000));
        let (one, two, three) = (record(&first), record(&second), record(&third));
        let packed = pack(
            &dir.0,
            &format!("{}-{}{PACK_EXTENSION}", one.id, two.id),
            &[&one, &two],
            &["not a line", r#"{"id":"not a name","s":"{}"}"#],
        );
        fs::write(dir.0.join(HISTORY).join(PACKS).join("notes.txt"), "x").unwrap();
        files.apply(&put(&[two.clone(), three.clone()]));
        let read = files.load();
        assert_eq!(
            read.sessions,
            [(one.id, first.clone()), (two.id, second), (three.id, third)]
        );
        assert!(read.unread.is_empty());
        assert_eq!(files.listing(), Some(vec![one.id, two.id, three.id]));
        assert_eq!(files.read(&[one.id]), [Ok(one.clone())]);
        let before = fs::read(&packed).unwrap();
        files.apply(&forget(&[(&one, &first)]));
        assert_eq!(ids(&files.load()), [two.id, three.id]);
        assert_eq!(files.listing(), Some(vec![two.id, three.id]));
        assert_eq!(files.forgotten(&[one.id]), [one.id]);
        assert_eq!(fs::read(&packed).unwrap(), before, "never written");
    }

    /// A record a later build packs while this one lists the history, its
    /// pack written and then its loose copy taken away, is listed all the
    /// same, and read, wherever the listing had got to. The packs were read
    /// before the loose records, so one packed in between was in neither,
    /// and the app stored it again as lost.
    #[test]
    fn a_record_packed_while_the_history_is_listed_is_listed_all_the_same() {
        let (dir, files) = history("packed-meanwhile");
        files.load();
        let (one, two) = (record(&finished(1_000)), record(&finished(2_000)));
        files.apply(&put(&[one.clone(), two.clone()]));
        let root = dir.0.clone();
        let mut packing = Some(one.clone());
        files.meanwhile(move || {
            // Halfway through the listing, once one folder of it is read.
            if let Some(one) = packing.take() {
                pack(&root, "packed.jsonl.gz", &[&one], &[]);
                let loose = root.join(HISTORY).join(RECORDS).join(file_name(one.id));
                fs::remove_file(loose).unwrap();
            }
        });
        assert_eq!(files.listing(), Some(vec![one.id, two.id]));
        assert_eq!(files.read(&[one.id]), [Ok(one)]);
    }

    /// Have another copy pack `record` into the history in `root` the
    /// `nth` time a folder of it has been read, from the next look of
    /// `files` on: its pack written, then its loose copy taken away.
    fn packed_meanwhile(files: &Files, root: &Path, record: &Record, nth: usize) {
        let (root, record) = (root.to_path_buf(), record.clone());
        let mut looked = 0;
        files.meanwhile(move || {
            looked += 1;
            if looked == nth {
                pack_away(&root, "packed.jsonl.gz", &[&record]);
            }
        });
    }

    /// Pack `records` into the history in `root` as a later build packs
    /// them, in a pack called `name`: the pack written, then their loose
    /// copies taken away.
    fn pack_away(root: &Path, name: &str, records: &[&Record]) {
        pack(root, name, records, &[]);
        for record in records {
            let loose = root.join(HISTORY).join(RECORDS).join(file_name(record.id));
            fs::remove_file(loose).unwrap();
        }
    }

    /// A record a later build packs once the packs were read, and before
    /// this build reads the record itself, is read all the same, from its
    /// pack: as the history is read whole, and as a record is read on its
    /// own. It was in neither place, and told of as one this build cannot
    /// read, which it then never took up for as long as it ran.
    #[test]
    fn a_record_packed_while_the_history_is_read_is_read_all_the_same() {
        let (dir, files) = history("packed-as-read");
        files.load();
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        files.apply(&put(&[one.clone(), two.clone()]));
        // Once the packs and the tombstones have been listed.
        packed_meanwhile(&files, &dir.0, &one, 3);
        let read = files.load();
        assert_eq!(read.sessions, [(one.id, first), (two.id, second)]);
        assert!(read.unread.is_empty(), "{:?}", read.unread);
        assert!(read.notices.is_empty(), "{:?}", read.notices);

        let (dir, files) = history("packed-as-read-alone");
        files.load();
        files.apply(&put(&[one.clone(), two.clone()]));
        // Once the packs have been read, before the record is.
        packed_meanwhile(&files, &dir.0, &one, 1);
        assert_eq!(files.read(&[one.id, two.id]), [Ok(one), Ok(two)]);
    }

    /// Records a later build packs in two passes while this one reads
    /// them, each pass once the packs have been read, are read all the
    /// same: every one is read where it lies first, and only then the
    /// packs again, for all those in neither place. The packs were read
    /// again at the first such record, and one packed after that came back
    /// as one that cannot be read, and was not taken up until the next
    /// look.
    #[test]
    fn records_packed_in_two_passes_as_they_are_read_are_read_all_the_same() {
        let (dir, files) = history("packed-twice-as-read");
        files.load();
        let (one, two) = (record(&finished(1_000)), record(&finished(2_000)));
        files.apply(&put(&[one.clone(), two.clone()]));
        let (root, passes) = (dir.0.clone(), [one.clone(), two.clone()]);
        let mut looked = 0;
        files.meanwhile(move || {
            if let Some(record) = passes.get(looked) {
                pack_away(&root, &format!("pass-{looked}{PACK_EXTENSION}"), &[record]);
            }
            looked += 1;
        });
        assert_eq!(files.read(&[one.id, two.id]), [Ok(one), Ok(two)]);
    }

    /// A record a later build packs while a backup is written, once the
    /// history has been listed and before the record is read, goes in the
    /// backup all the same, from its pack: one this build reads, and one
    /// it cannot, which only the store holds. Each was in neither place:
    /// the backup was saved again from what the app holds, or went without
    /// the one this build cannot read, said to be one that could not be
    /// read.
    #[test]
    fn a_record_packed_while_a_backup_is_written_goes_in_it() {
        let (dir, files) = raw_history("packed-as-backed-up");
        files.load();
        let history = [finished(1_000), finished(2_000)];
        let records: Vec<Record> = history.iter().map(record).collect();
        files.apply(&put(&records));
        // Stored by a later build, as this one cannot read it.
        let later = Record {
            id: RecordId {
                order: 3_000_000,
                nonce: 9,
            },
            json: r#"{"date":"later"}"#.into(),
        };
        place_whole(
            &files.records(),
            &file_name(later.id),
            later.json.as_bytes(),
        )
        .unwrap();
        let (root, packing) = (dir.0.clone(), [records[0].clone(), later.clone()]);
        let mut looked = 0;
        *files.elsewhere.borrow_mut() = Some(Box::new(move || {
            looked += 1;
            // Once its records, its packs and its tombstones are listed.
            if looked == 3 {
                pack_away(&root, "packed.jsonl.gz", &[&packing[0], &packing[1]]);
            }
        }));
        let settings = cw_core::TrainingSettings::default();
        let backup = BackupFile::new(&settings, 7, Vec::new())
            .beside_unread([later.id].into_iter().collect());
        let mut out = Vec::new();
        let backed = files.stream_backup(&backup, &mut out).expect("written");
        assert_eq!(
            backed,
            BackedUp {
                sessions: 3,
                left_out: 0
            }
        );
        let written = String::from_utf8(out).unwrap();
        assert!(written.contains(&later.json), "{written}");
        let read = crate::persist::backup::read_backup(&written).expect("a backup");
        assert_eq!(read.sessions, history);
        assert_eq!(read.left_out, 1, "the one this build cannot read");
    }

    /// A pack that cannot be opened, gone between the look at its folder
    /// and the read, say, costs only the records it holds: the rest of the
    /// history is read and listed. It used to leave the whole history
    /// unlisted, and the app opened on none of it, nor took up anything for
    /// as long as it ran.
    #[cfg(unix)]
    #[test]
    fn a_pack_that_cannot_be_opened_costs_only_its_own_records() {
        let (dir, files) = history("pack-unopened");
        files.load();
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        pack(&dir.0, "packed.jsonl.gz", &[&one], &[]);
        files.apply(&put(std::slice::from_ref(&two)));
        let gone = dir.0.join(HISTORY).join(PACKS).join("gone.jsonl.gz");
        std::os::unix::fs::symlink(dir.0.join("nowhere"), gone).unwrap();
        assert_eq!(
            Files::at(&dir.0).load().sessions,
            [(one.id, first), (two.id, second)]
        );
        assert_eq!(files.listing(), Some(vec![one.id, two.id]));
    }

    /// A pack that opened before and cannot be opened now, held by another
    /// program for a moment, say, or put back by a later build as it is
    /// listed, still lists the records it held, which the app holds: a
    /// listing without them had the app take them for lost in passing, and
    /// store each again beside its pack.
    #[cfg(unix)]
    #[test]
    fn a_pack_that_opened_before_still_lists_its_records() {
        let (dir, files) = history("pack-opened-before");
        files.load();
        let (one, two) = (record(&finished(1_000)), record(&finished(2_000)));
        let packed = pack(&dir.0, "packed.jsonl.gz", &[&one], &[]);
        files.apply(&put(std::slice::from_ref(&two)));
        assert_eq!(files.listing(), Some(vec![one.id, two.id]));
        fs::remove_file(&packed).unwrap();
        std::os::unix::fs::symlink(dir.0.join("nowhere"), &packed).unwrap();
        assert_eq!(files.listing(), Some(vec![one.id, two.id]));
        assert_eq!(
            files.read(&[one.id]),
            [Err(Unopened::Failed(one.id))],
            "read nowhere"
        );
    }

    /// A pack that cannot be opened leaves a backup to what the app holds:
    /// the rest of what is stored is not all there is. Written from the
    /// rest, the backup left out the sessions of that pack, and said
    /// nothing of them.
    #[cfg(unix)]
    #[test]
    fn a_pack_that_cannot_be_opened_leaves_a_backup_to_the_app() {
        use crate::persist::backup::write_backup;
        let (dir, files) = raw_history("pack-unopened-backup");
        files.load();
        let history = [finished(1_000), finished(2_000)];
        let records: Vec<Record> = history.iter().map(record).collect();
        // The first was packed, in a pack that will not open now.
        files.apply(&put(&records[1..]));
        fs::create_dir_all(dir.0.join(HISTORY).join(PACKS)).unwrap();
        let gone = dir.0.join(HISTORY).join(PACKS).join("gone.jsonl.gz");
        std::os::unix::fs::symlink(dir.0.join("nowhere"), gone).unwrap();
        let settings = cw_core::TrainingSettings::default();
        let mut out = Vec::new();
        assert_eq!(
            files.stream_backup(&BackupFile::new(&settings, 7, Vec::new()), &mut out),
            Err(NotSaved::Unread)
        );
        let every = BackupFile::new(&settings, 7, records).holding_every_session();
        let mut out = Vec::new();
        let backed = files.stream_backup(&every, &mut out).expect("written");
        assert_eq!(
            backed,
            BackedUp {
                sessions: 2,
                left_out: 0
            }
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            write_backup(&settings, &history, 7)
        );
    }

    /// Each test of the files that reaches them as the app does, made
    /// again with the files on a worker of their own, as the app has them:
    /// each answer the same, and every look and write on the worker's
    /// thread ([`HistoryFiles::on_its_thread`]).
    macro_rules! on_the_worker_too {
        ($($(#[$cfg:meta])* $test:ident,)*) => {
            mod on_the_worker {
                $(
                    $(#[$cfg])*
                    #[test]
                    fn $test() {
                        super::ON_WORKER.set(true);
                        super::$test();
                    }
                )*
            }
        };
    }

    on_the_worker_too! {
        a_session_is_one_file_written_whole,
        a_record_is_never_written_over,
        history_reads_in_the_order_it_was_kept,
        old_temporary_files_are_cleared,
        a_record_this_build_cannot_read_is_left_in_place_and_told_of_once,
        a_later_build_reads_what_this_one_could_not,
        a_record_that_could_not_be_read_for_a_moment_is_failed_not_unread,
        a_record_still_unread_after_every_try_is_told_of_once,
        a_record_read_at_a_try_after_it_failed_fails_no_longer,
        a_record_deleted_elsewhere_as_the_history_is_read_is_not_told_of_as_unreadable,
        a_deletion_is_a_tombstone_beside_the_history,
        a_record_and_its_tombstone_read_as_deleted,
        an_empty_tombstone_still_deletes,
        #[cfg(unix)]
        deletions_say_when_each_was_made_or_that_none_could_be_looked_for,
        a_deletion_is_made_once_its_tombstones_are,
        #[cfg(unix)]
        a_deletion_that_cannot_be_made_durable_deletes_nothing,
        a_session_put_back_whose_record_stayed_is_one_record,
        the_history_knows_how_large_it_has_grown,
        two_copies_on_one_folder_take_up_each_others_changes,
        a_change_another_copy_made_first_is_taken_up_after_ones_own,
        what_another_copy_stores_while_the_history_is_read_is_taken_up,
        a_change_is_still_to_take_up_after_a_listing_that_failed,
        a_restore_that_fails_is_not_told_of_as_cut_short,
        a_store_from_a_newer_incompatible_layout_is_only_read,
        a_later_compatible_layout_keeps_what_it_added,
        a_history_file_becomes_records_once,
        a_migration_cut_short_runs_again_to_the_same_records,
        an_unreadable_history_file_is_set_aside_and_not_migrated,
        a_history_that_cannot_be_read_now_is_migrated_later,
        the_old_history_file_is_left_for_older_builds,
        sessions_an_older_build_added_after_the_move_are_taken_up,
        sessions_from_before_the_move_are_never_brought_back,
        a_session_imported_once_is_never_imported_again,
        a_session_a_record_here_holds_is_not_taken_up_from_an_older_build_while_that_record_cannot_be_read,
        #[cfg(unix)]
        a_session_a_pack_here_holds_is_not_taken_up_from_an_older_build_while_that_pack_cannot_be_opened,
        a_session_a_later_build_stored_here_is_not_taken_up_from_an_older_build,
        a_session_whose_record_here_is_damaged_is_taken_up_from_an_older_build_beside_it,
        a_deletion_an_older_build_made_after_the_move_is_honoured,
        a_deletion_an_older_build_made_after_the_move_is_honoured_whatever_its_clock,
        a_layout_that_does_not_read_is_set_aside_and_the_move_made_again,
        a_pack_and_a_loose_copy_of_one_record_read_as_one,
        a_record_packed_while_the_history_is_listed_is_listed_all_the_same,
        a_record_packed_while_the_history_is_read_is_read_all_the_same,
        records_packed_in_two_passes_as_they_are_read_are_read_all_the_same,
        #[cfg(unix)]
        a_pack_that_cannot_be_opened_costs_only_its_own_records,
        #[cfg(unix)]
        a_pack_that_opened_before_still_lists_its_records,
    }
}
