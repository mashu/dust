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
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime};

use cw_core::SessionResult;
use flate2::read::GzDecoder;

use super::desktop::{
    DELETED, SESSIONS, copy_aside, move_aside, place_whole, shown, sync_dir, write_whole,
};
use super::legacy::{self, Fingerprint, Gate, Layout, read_history};
use super::record::{HistoryChange, HistoryLoad, HistoryNotice, Record, RecordId, read_record};
use super::{Deleted, HistorySaved, SessionKey, SetAside};

/// The folder of the history, in the store's.
const HISTORY: &str = "history";
/// The records, in the history's folder.
const RECORDS: &str = "sessions";
/// The tombstones, in the history's folder.
const TOMBSTONES: &str = "deleted";
/// The records packed together, in the history's folder.
const PACKS: &str = "packs";
const PACK_EXTENSION: &str = ".jsonl.gz";
const LAYOUT_FILE: &str = "layout.json";
const TOLD: &str = "told.json";
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
    /// Every record, loose or packed, in order.
    records: BTreeSet<RecordId>,
    /// Every tombstone.
    gone: BTreeSet<RecordId>,
    /// What each packed record holds, as it was written.
    packed: BTreeMap<RecordId, String>,
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

/// The key a record's session finished at, read without the rest of it.
#[derive(serde::Deserialize)]
struct KeyOnly {
    timestamp: u64,
    date: String,
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
        }
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
    /// its thread alone ([`super::worker::THREAD`]).
    pub(super) fn keep_to_worker(&mut self) {
        self.on_worker = true;
    }

    /// Every look at the files, and every write, is made on the worker's
    /// thread, once they are the worker's: on a phone, any other thread
    /// that reaches them is one the app's taps wait on.
    fn on_its_thread(&self) {
        debug_assert!(
            !self.on_worker || thread::current().name() == Some(super::worker::THREAD),
            "the history's files were reached from {:?}, not from {}",
            thread::current().name(),
            super::worker::THREAD
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
    /// there since, where one was.
    ///
    /// A record and its tombstone read as deleted, and the deletion cut
    /// short is finished. A record this build cannot read is left where it
    /// is, and told of once ([`HistoryNotice::Unread`]). A history a newer
    /// build keeps in a way this one would harm is read, and nothing is
    /// written to it, not even to tidy up.
    pub(super) fn load(&self) -> HistoryLoad {
        self.on_its_thread();
        let mut load = HistoryLoad::default();
        // How the history is kept, once moved, and whether this build may
        // write to it at all.
        let (layout, writable) = match self.gate() {
            Ok(Gate::Open(layout)) => (Some(layout), true),
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
        let listing = match self.listing_now() {
            Ok(listing) => listing,
            Err(err) => {
                eprintln!("could not list the history: {err}");
                self.restamp();
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
        let read = read_each(&self.records(), &live, &listing.packed);
        let mut keys = self.keys.borrow_mut();
        load.sessions.reserve(live.len());
        for (id, session) in live.into_iter().zip(read) {
            match session {
                Some(session) => {
                    keys.insert(id, SessionKey::of(&session));
                    load.sessions.push((id, session));
                }
                None => load.unread.push(id),
            }
        }
        drop(keys);
        if unlinked {
            let _ = sync_dir(&self.records());
        }
        if let Some(count) = self.tell(&load.unread, writable) {
            load.notices.push(HistoryNotice::Unread { count });
        }
        if let Some(layout) = &layout {
            let live = keyed(load.sessions.iter().map(|(id, session)| (*id, session)));
            if let Some(change) = self.import(layout, &live) {
                taken_up(&mut load.sessions, &change);
            }
        }
        load.complete = layout.is_some();
        self.restamp();
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
        let made = self.make(change);
        // Made or not, the change is over, and what became of it is said
        // now: a journal left behind says only that Dust stopped first.
        if let Some(journal) = journal {
            end_journal(journal, &self.dir);
        }
        self.restamp();
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
            if let Ok(key) = serde_json::from_str::<KeyOnly>(&record.json) {
                self.keys
                    .borrow_mut()
                    .insert(id, SessionKey::new(key.timestamp, key.date));
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
            if let Err(err) = remove_if_there(&records.join(file_name(*id))) {
                eprintln!("a deleted session's record stays until the next read: {err}");
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
        let mut id = record.id;
        for _ in 0..BUMPS {
            let name = file_name(id);
            match fs::read(self.records().join(&name)) {
                Ok(held) if held == record.json.as_bytes() => return Ok(id),
                Ok(_) => id = id.bumped(),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    place_whole(&self.records(), &name, record.json.as_bytes())?;
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

    /// Every record there is, deleted ones left out, once whatever an older
    /// build stored since has been taken up. None where the history cannot
    /// be listed.
    pub(super) fn listing(&self) -> Option<Vec<RecordId>> {
        self.on_its_thread();
        if let Ok(Gate::Open(layout)) = self.gate()
            && self.legacy_changed(&layout)
            && let Some(live) = self.live_keys()
        {
            self.import(&layout, &live);
        }
        let listing = self.listing_now().ok()?;
        self.restamp();
        Some(listing.records.difference(&listing.gone).copied().collect())
    }

    /// The records `ids`, as they are stored, each that can be read: one
    /// that cannot comes back as its id.
    pub(super) fn read(&self, ids: &[RecordId]) -> Vec<Result<Record, RecordId>> {
        self.on_its_thread();
        let packed = self.packed().unwrap_or_default();
        ids.iter()
            .map(|&id| {
                self.raw_record(id, &packed)
                    .map(|json| Record { id, json })
                    .ok_or(id)
            })
            .collect()
    }

    /// Which of `ids` were deleted by hand. One whose tombstone cannot be
    /// looked for is taken as not deleted: missing is never deleted.
    pub(super) fn forgotten(&self, ids: &[RecordId]) -> Vec<RecordId> {
        self.on_its_thread();
        ids.iter()
            .copied()
            .filter(|id| fs::symlink_metadata(self.tombstones().join(file_name(*id))).is_ok())
            .collect()
    }

    /// Note the folders as they are now: what this copy did is no change
    /// for it to take up.
    fn restamp(&self) {
        self.watch.restamp();
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

    /// Every record and tombstone there is.
    fn listing_now(&self) -> io::Result<Listing> {
        let packed = self.packed()?;
        let mut records = ids_in(&self.records())?;
        records.extend(packed.keys().copied());
        Ok(Listing {
            records,
            gone: ids_in(&self.tombstones())?,
            packed,
        })
    }

    /// What is stored for `id`, loose, or else in a pack: none where it
    /// cannot be read.
    fn raw_record(&self, id: RecordId, packed: &BTreeMap<RecordId, String>) -> Option<String> {
        raw_at(&self.records(), id, packed)
    }

    /// Every record a later build packed together, and what each holds,
    /// as it was written. A line that does not read, or a pack cut short,
    /// gives up what it can: the pack is left as it is.
    fn packed(&self) -> io::Result<BTreeMap<RecordId, String>> {
        let entries = match fs::read_dir(self.dir.join(PACKS)) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(err) => return Err(err),
        };
        let mut packed = BTreeMap::new();
        for entry in entries {
            let entry = entry?;
            if !entry
                .file_name()
                .to_string_lossy()
                .ends_with(PACK_EXTENSION)
            {
                continue;
            }
            let lines = io::BufReader::new(GzDecoder::new(fs::File::open(entry.path())?)).lines();
            for line in lines.map_while(Result::ok) {
                let Ok(line) = serde_json::from_str::<PackLine>(&line) else {
                    continue;
                };
                if let Some(id) = RecordId::parse(&line.id) {
                    packed.entry(id).or_insert(line.s);
                }
            }
        }
        Ok(packed)
    }

    /// Every record there is by its session's key: those read before as
    /// they were read, the rest read now for their keys alone.
    fn live_keys(&self) -> Option<HashMap<SessionKey, Vec<RecordId>>> {
        let listing = self.listing_now().ok()?;
        let mut keys = self.keys.borrow_mut();
        let mut live: HashMap<SessionKey, Vec<RecordId>> = HashMap::new();
        for id in listing.records.difference(&listing.gone) {
            let key = match keys.get(id) {
                Some(key) => key.clone(),
                None => {
                    let Some(key) = self
                        .raw_record(*id, &listing.packed)
                        .and_then(|raw| serde_json::from_str::<KeyOnly>(&raw).ok())
                        .map(|key| SessionKey::new(key.timestamp, key.date))
                    else {
                        continue;
                    };
                    keys.insert(*id, key.clone());
                    key
                }
            };
            live.entry(key).or_default().push(*id);
        }
        Some(live)
    }

    /// Whether the history kept whole changed since it was last taken up.
    fn legacy_changed(&self, layout: &Layout) -> bool {
        fingerprint_of(&self.root.join(SESSIONS)).is_ok_and(|now| now != layout.legacy)
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
        for record in self.not_moved_yet(records).map_err(|err| err.to_string())? {
            self.place_record(&record).map_err(|err| err.to_string())?;
        }
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
        let mut elsewhere: HashMap<String, usize> = HashMap::new();
        for id in here.difference(&names) {
            if let Ok(json) = fs::read_to_string(self.records().join(file_name(*id))) {
                *elsewhere.entry(json).or_default() += 1;
            }
        }
        Ok(moved
            .into_iter()
            .filter(|record| {
                if here.contains(&record.id) {
                    return true;
                }
                if gone.contains(&record.id) {
                    return false;
                }
                if let Some(count) = elsewhere.get_mut(&record.json).filter(|count| **count > 0) {
                    *count -= 1;
                    return false;
                }
                !serde_json::from_str::<KeyOnly>(&record.json)
                    .is_ok_and(|key| gone_keys.contains(&SessionKey::new(key.timestamp, key.date)))
            })
            .collect())
    }

    /// The key of every session deleted here that its tombstone names.
    fn tombstoned_keys(&self) -> HashSet<SessionKey> {
        let Ok(gone) = ids_in(&self.tombstones()) else {
            return HashSet::new();
        };
        gone.into_iter()
            .filter_map(|id| {
                let raw = fs::read(self.tombstones().join(file_name(id))).ok()?;
                serde_json::from_slice::<Deleted>(&raw)
                    .ok()
                    .map(|gone| gone.key())
            })
            .collect()
    }

    /// Say how many of `unread`, the records this build cannot read, are
    /// new to it: none, once each has been told of. `writable` is whether
    /// what was told may be noted, so it is told of once.
    fn tell(&self, unread: &[RecordId], writable: bool) -> Option<usize> {
        let path = self.dir.join(TOLD);
        let told: HashSet<String> = fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        let new = unread
            .iter()
            .filter(|id| !told.contains(&id.name()))
            .count();
        if new == 0 {
            return None;
        }
        if writable {
            let names: Vec<String> = unread.iter().map(|id| id.name()).collect();
            if let Ok(raw) = serde_json::to_vec(&names) {
                let _ = write_whole(&self.dir, TOLD, &raw);
            }
        }
        Some(new)
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
            lock.sync_all()?;
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
    /// the history kept whole: noted as seen, so it is told once.
    pub(super) fn changed(&self) -> bool {
        let now = self.stamps();
        self.seen().replace(now) != Some(now)
    }

    /// Note the folders as they are now: what this copy did is no change
    /// for it to take up.
    fn restamp(&self) {
        let now = self.stamps();
        *self.seen() = Some(now);
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

/// What is stored for `id` in `records`, loose, or else in a pack: none
/// where it cannot be read.
fn raw_at(records: &Path, id: RecordId, packed: &BTreeMap<RecordId, String>) -> Option<String> {
    match fs::read_to_string(records.join(file_name(id))) {
        Ok(raw) => Some(raw),
        Err(_) => packed.get(&id).cloned(),
    }
}

/// Each of `ids`, read from `records` or else from `packed`, and scored as
/// it reads ([`read_record`]), in their order: none for one that cannot be
/// read. A long history is read on as many as [`READERS`] threads at once,
/// a part each, as it is read whole at every launch.
fn read_each(
    records: &Path,
    ids: &[RecordId],
    packed: &BTreeMap<RecordId, String>,
) -> Vec<Option<SessionResult>> {
    let one = |id: &RecordId| {
        raw_at(records, *id, packed)
            .as_deref()
            .and_then(read_record)
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
    fs::read(root.join(DELETED))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

/// Every record of `sessions` by its session's key.
fn keyed<'a>(
    sessions: impl Iterator<Item = (RecordId, &'a SessionResult)>,
) -> HashMap<SessionKey, Vec<RecordId>> {
    let mut live: HashMap<SessionKey, Vec<RecordId>> = HashMap::new();
    for (id, session) in sessions {
        live.entry(SessionKey::of(session)).or_default().push(id);
    }
    live
}

/// `sessions`, in the order of their records, with `change`, made, made to
/// them too.
fn taken_up(sessions: &mut Vec<(RecordId, SessionResult)>, change: &HistoryChange) {
    let forgotten: HashSet<RecordId> = change.forget.iter().map(|(id, _)| *id).collect();
    sessions.retain(|(id, _)| !forgotten.contains(id));
    for record in &change.put {
        let Some(session) = read_record(&record.json) else {
            continue;
        };
        if let Err(at) = sessions.binary_search_by_key(&record.id, |(id, _)| *id) {
            sessions.insert(at, (record.id, session));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::desktop::tests::TempDir;
    use crate::persist::record::{records_for_blob, scored};
    use crate::persist::tests::session;
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

        fn read(&self, ids: &[RecordId]) -> Vec<Result<Record, RecordId>> {
            wait(self.0.read(ids.to_vec()))
        }

        fn forgotten(&self, ids: &[RecordId]) -> Vec<RecordId> {
            wait(self.0.forgotten(ids.to_vec()))
        }

        fn changed(&self) -> bool {
            self.0.changed()
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

    /// A record this build cannot read — a newer build's, say — is left
    /// where it is as it is, and the user is told of it once.
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
        let raw = r#"{"date":5,"laterKey":true}"#;
        fs::write(records.join(file_name(unread)), raw).unwrap();
        let read = files.load();
        assert_eq!(read.sessions.len(), 1);
        assert_eq!(read.unread, [unread]);
        assert_eq!(read.notices, [HistoryNotice::Unread { count: 1 }]);
        let again = Files::at(&dir.0).load();
        assert_eq!(again.unread, [unread], "still known");
        assert!(again.notices.is_empty(), "told of once");
        assert_eq!(
            fs::read_to_string(records.join(file_name(unread))).unwrap(),
            raw
        );
        // One more is told of on its own.
        let another = RecordId {
            order: 3_000_000,
            nonce: 1,
        };
        fs::write(records.join(file_name(another)), "[").unwrap();
        let read = files.load();
        assert_eq!(read.unread, [unread, another]);
        assert_eq!(read.notices, [HistoryNotice::Unread { count: 1 }]);
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
        // has open: read once it can be.
        let busy = record(&finished(3_000));
        fs::create_dir(records.join(file_name(busy.id))).unwrap();
        assert!(files.load().unread.contains(&busy.id));
        assert_eq!(files.read(&[busy.id]), [Err(busy.id)]);
        fs::remove_dir(records.join(file_name(busy.id))).unwrap();
        fs::write(records.join(file_name(busy.id)), &busy.json).unwrap();
        let read = files.load();
        assert_eq!(read.unread, [later]);
        assert_eq!(stamps(&read), [3_000]);
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

    /// Write a pack of `records`, as a later build packs them, with
    /// `extra` lines after them.
    fn pack(dir: &TempDir, name: &str, records: &[&Record], extra: &[&str]) -> PathBuf {
        use flate2::write::GzEncoder;
        let packs = dir.0.join(HISTORY).join(PACKS);
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
            &dir,
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
        a_deletion_is_a_tombstone_beside_the_history,
        a_record_and_its_tombstone_read_as_deleted,
        an_empty_tombstone_still_deletes,
        a_deletion_is_made_once_its_tombstones_are,
        #[cfg(unix)]
        a_deletion_that_cannot_be_made_durable_deletes_nothing,
        a_session_put_back_whose_record_stayed_is_one_record,
        two_copies_on_one_folder_take_up_each_others_changes,
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
        a_deletion_an_older_build_made_after_the_move_is_honoured,
        a_layout_that_does_not_read_is_set_aside_and_the_move_made_again,
        a_pack_and_a_loose_copy_of_one_record_read_as_one,
    }
}
