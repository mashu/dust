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
//! What is read and written, in what order, and what is made of it, is the
//! record store's, as for every store ([`record_store`]): this module makes
//! its reads and its durable steps on the files ([`Disk`]).
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
use super::legacy::{self, Fingerprint, Gate, Layout};
use super::progress::LoadProgress;
use super::record::{
    Deletions, HistoryChange, HistoryLoad, HistoryNotice, Record, RecordId, Unopened, Unreadable,
    read_record, unreadable,
};
use super::record_store::notes::Note;
use super::record_store::steps::{Steps, There, make_steps, place};
use super::record_store::{
    self, Backend, By, KeyCache, Laid, Legacy, LegacyLook, Listing, Made, Order, Raw, RecordRead,
    Refusal, Tombstone, TombstoneRead, TombstonesOf, Writes, at_once,
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
/// ([`notes::no_more`]): one that still cannot be read as it is read again,
/// at a later launch, is told of then ([`notes::at_open`]).
/// It holds this build's failures alone, whether told of or not; [`TOLD`]
/// names those this build told of, beside every record a build before
/// formats could not open or make out.
///
/// [`notes::no_more`]: record_store::notes::no_more
/// [`notes::at_open`]: record_store::notes::at_open
const FAILING: &str = "failing.json";
/// The records this build told of as ones that could not be read at all,
/// a moment's failure that lasted ([`HistoryNotice::Unread`]), by name,
/// until each reads, or is gone. No build before this one writes it, so a
/// record such a build told of in [`TOLD`], which it does of every record
/// it could not open, a moment's failure among them, holds back none of
/// this build's notices of a failure of its own. Each is noted in [`TOLD`]
/// as well, where such a build looks, so that it does not tell of it a
/// second time. This build only adds names to that note, so such a build,
/// used alone, says nothing of one that reads since and fails again, which
/// this build tells of, unless it has written the note over meanwhile: it
/// does so as it tells of a record the note does not name, with every
/// record it cannot read just then.
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
/// stays, though it names none. From then on, a record that such a build,
/// used on the history again, tells of first, and that this one cannot
/// read either, is told of here a second time: such a build writes
/// [`TOLD`] over as it tells of one, and which of the names there it wrote
/// since this build last left the note, this build cannot tell. A note of
/// what this build left there would tell them apart; for so narrow a case,
/// a build before formats used again after this one, it is not kept.
const DAMAGED_TOLD: &str = "damaged-told.json";
/// What a restore of many sessions at once is called while it runs:
/// `restoring-<n>.json`.
const JOURNAL_PREFIX: &str = "restoring-";

/// How old a temporary file a write left behind has to be to be cleared:
/// far older than any write takes, so none still being written goes.
const OLD_TEMPORARY: Duration = Duration::from_secs(60 * 60);

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

/// What each record a later build packed holds, by its id, as it was
/// written: what a listing of the history found in the packs
/// ([`Listing::listed`]).
type Packed = BTreeMap<RecordId, String>;

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
    /// is, and told of once, as a newer Dust's or as damaged. One that could
    /// not be read at all just now failed ([`HistoryLoad::failed`]), for the
    /// app to read again: told of, once, only where it could not be read as
    /// the history was last read either ([`notes::at_open`]). A history a
    /// newer build keeps in a way this one would harm is read, and nothing
    /// is written to it, not even to tidy up.
    ///
    /// [`notes::at_open`]: record_store::notes::at_open
    pub(super) fn load(&self) -> HistoryLoad {
        self.on_its_thread();
        // The files fall back to nothing: a refused read is no history read,
        // and the next look reads it.
        then_and_there(record_store::load(&Disk(self)))
            .and_then(Result::ok)
            .unwrap_or_default()
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
        then_and_there(record_store::apply(&Disk(self), change))
            .unwrap_or_else(|| HistorySaved::Failed(WAITED.into()))
    }

    /// The history's folders, as a change is made in them, step by step.
    fn folders(&self) -> Folders<'_> {
        Folders {
            records: self.records(),
            tombstones: self.tombstones(),
            size: &self.size,
        }
    }

    /// Every record there is, deleted ones left out, once whatever an older
    /// build stored since has been taken up, where every record here can be
    /// read ([`record_store::listing`]). None where the history cannot be
    /// listed.
    pub(super) fn listing(&self) -> Option<Vec<RecordId>> {
        self.on_its_thread();
        then_and_there(record_store::listing(&Disk(self))).flatten()
    }

    /// The records `ids`, as they are stored, each that can be read: one
    /// that cannot comes back as why ([`Unopened`]): it could not be read
    /// just now, or its bytes are no text, damaged, as the history read
    /// whole takes such a record ([`Raw::record`]). Each read fails no
    /// longer, and is told of no more ([`notes::no_more`]).
    ///
    /// One a later build packed once the packs were read, and took the
    /// loose copy of before it was read, is in neither place: every one of
    /// `ids` is read first, and the packs again only then, for all those in
    /// neither place ([`Self::raw_each`]).
    ///
    /// [`notes::no_more`]: record_store::notes::no_more
    pub(super) fn read(&self, ids: &[RecordId]) -> Vec<Result<Record, Unopened>> {
        self.on_its_thread();
        then_and_there(record_store::read(&Disk(self), ids))
            .unwrap_or_else(|| ids.iter().map(|&id| Err(Unopened::Failed(id))).collect())
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
        let mut packed = listing.listed;
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
        then_and_there(record_store::deletions(&Disk(self), ids)).flatten()
    }

    /// Which of `ids` were deleted by hand, each beside the session its
    /// tombstone remembers ([`Self::deletions`]): one whose tombstone does
    /// not say, or is there but does not read, is left out. None where a
    /// tombstone could not be looked for, as [`Self::deletions`] says. One
    /// that would not read gave no word at all, of any of `ids`.
    pub(super) fn deleted_sessions(&self, ids: &[RecordId]) -> Option<Vec<(RecordId, SessionKey)>> {
        self.on_its_thread();
        then_and_there(record_store::deleted_sessions(&Disk(self), ids)).flatten()
    }

    /// Which of the sessions `keys` a tombstone remembers, whatever record
    /// it named ([`crate::persist::Store::deleted_keys`]): one that does not
    /// read, or names no session, is left out. None where the tombstones
    /// cannot be listed.
    pub(super) fn deleted_keys(&self, keys: &[SessionKey]) -> Option<Vec<SessionKey>> {
        self.on_its_thread();
        then_and_there(record_store::deleted_keys(&Disk(self), keys.to_vec())).flatten()
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
    fn listing_now(&self) -> io::Result<Listing<Packed>> {
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
            skipped: packs.skipped,
            listed: packs.records,
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

    /// What to tell of `ids`, records listed that could not be read at all
    /// at any of the app's tries since it found them so: of those this build
    /// has not told of before ([`UNREAD_TOLD`]), how many
    /// ([`HistoryNotice::Unread`]), each noted as told, where this build may
    /// write, so it is told of once ([`super::Store::still_unread`]), and in
    /// [`TOLD`] too, so that a build before formats does not tell of it
    /// again ([`notes::still_unread`]). What a build before this one noted
    /// of one in [`TOLD`] holds back nothing: such a build notes there every
    /// record it could not open, a moment's failure among them, and one
    /// that has read since fails now on its own.
    ///
    /// [`notes::still_unread`]: record_store::notes::still_unread
    pub(super) fn still_unread(&self, ids: &[RecordId]) -> Vec<HistoryNotice> {
        self.on_its_thread();
        then_and_there(record_store::still_unread(&Disk(self), ids)).unwrap_or_default()
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
    /// again, as is one an older build made that a look could not take up
    /// ([`Self::unsee`]).
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

    /// Note nothing as seen, as a look that held back the take-up of what an
    /// older build changed leaves it ([`Backend::held_back`]): it is still
    /// to take up, and the next look tries again, though the folders, and
    /// the history kept whole, stay as they are, as a tombstone held open
    /// reads again without its folder changing.
    fn unsee(&self) {
        *self.seen() = None;
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
        Raw::Failed | Raw::Missing => None,
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

/// What the app is told of a change to the files that waited, as none does
/// ([`then_and_there`]).
const WAITED: &str = "the history's files did not answer at once.";

/// What `op`, made on the files through the record store, comes to: every
/// look and write it makes is made then and there, so it is done as it is
/// first asked ([`at_once`]). None where it waited, as none does: a debug
/// build stops there.
fn then_and_there<T>(op: impl Future<Output = T>) -> Option<T> {
    let done = at_once(op);
    debug_assert!(done.is_some(), "a look at the history's files waited");
    done
}

/// The history's files, as the record store reads and writes them
/// ([`record_store`]): every look and every write made then and there, on
/// the worker's thread, so each of its futures is ready as it is first
/// polled ([`at_once`]).
struct Disk<'a>(&'a HistoryFiles);

impl Refusal for io::Error {
    /// A full disk is a full store.
    fn saved(&self) -> HistorySaved {
        if matches!(
            self.kind(),
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
        ) {
            HistorySaved::Full
        } else {
            HistorySaved::Failed(self.to_string())
        }
    }

    fn words(&self) -> String {
        self.to_string()
    }
}

impl Backend for Disk<'_> {
    type Refused = io::Error;
    type Look = Stamps;
    type Listed = Packed;
    const ORDER: Order = Order::MoveThenRead;
    const FALLS_BACK: bool = false;
    const NOTES_STILL_UNREAD: bool = true;

    fn keys(&self) -> &RefCell<KeyCache> {
        &self.0.keys
    }

    async fn gate(&self) -> io::Result<Gate> {
        self.0.gate()
    }

    async fn list(&self) -> io::Result<Listing<Packed>> {
        self.0.listing_now()
    }

    /// Read on as many as [`READERS`] threads at once, a long history in
    /// parts, each counted as it is read; one a later build packed once the
    /// packs were read, and took the loose copy of before it was read, is
    /// in neither place, and is looked for again, once, in the packs as they
    /// are by now. What they hold, in bytes, is what the history holds.
    async fn read_listed(&self, listing: &Listing<Self::Listed>) -> Vec<(RecordId, RecordRead)> {
        let live: Vec<RecordId> = listing.records.difference(&listing.gone).copied().collect();
        let records = self.0.records();
        self.0.progress.reading(Some(live.len() as u64));
        let mut read = read_each(&records, &live, &listing.listed, &self.0.progress);
        if read.iter().any(Option::is_none) {
            let packed = self.0.packed().unwrap_or_default().records;
            for (read, id) in read.iter_mut().zip(&live) {
                if read.is_none() {
                    *read = read_one(&records, *id, &packed);
                }
            }
        }
        let size = read.iter().flatten().map(|(_, bytes)| *bytes).sum();
        self.0.size.store(size, Ordering::Relaxed);
        live.into_iter()
            .zip(read)
            .map(|(id, read)| (id, read.map(|(read, _)| read)))
            .collect()
    }

    /// By what a listing found in the packs, where it is given, as a
    /// take-up reads the records it listed for their keys; or else as the
    /// app asks, the packs read afresh, and again for those in neither
    /// place ([`HistoryFiles::raw_each`]).
    async fn read(&self, ids: &[RecordId], listed: Option<&Self::Listed>) -> Vec<Raw> {
        let records = self.0.records();
        match listed {
            Some(packed) => ids.iter().map(|&id| raw_at(&records, id, packed)).collect(),
            None => {
                let mut packed = self.0.packed().unwrap_or_default().records;
                self.0.raw_each(ids, &mut packed)
            }
        }
    }

    async fn tombstones(&self) -> io::Result<Vec<Tombstone>> {
        let tombstones = self.0.tombstones();
        let mut found = Vec::new();
        for id in ids_in(&tombstones)? {
            let read = match fs::read(tombstones.join(file_name(id))) {
                Ok(raw) => TombstoneRead::Bytes(raw),
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => TombstoneRead::Failed(err.to_string()),
            };
            found.push(Tombstone { id: Some(id), read });
        }
        Ok(found)
    }

    /// Each looked for on its own: one whose look fails other than as not
    /// there gives no word of any.
    async fn tombstones_of(&self, ids: &[RecordId]) -> Option<TombstonesOf> {
        let tombstones = self.0.tombstones();
        let mut found = Vec::new();
        for &id in ids {
            let tombstone = tombstones.join(file_name(id));
            match fs::symlink_metadata(&tombstone) {
                Ok(_) => found.push((id, fs::read(&tombstone).ok())),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return None,
            }
        }
        Some(found)
    }

    async fn note(&self, note: Note) -> Option<Vec<u8>> {
        fs::read(self.0.dir.join(note_file(note))).ok()
    }

    async fn write_note(&self, note: Note, names: Option<&BTreeSet<String>>) {
        let _ = match names {
            Some(names) => serde_json::to_vec(names)
                .map_err(io::Error::other)
                .and_then(|raw| write_whole(&self.0.dir, note_file(note), &raw)),
            None => remove_if_there(&self.0.dir.join(note_file(note))),
        };
    }

    /// By when `sessions.json` was last written, and how long it is: one
    /// that cannot be looked at now is taken as unchanged, and nothing is
    /// taken up until it can be ([`LegacyLook::Same`]).
    async fn legacy_look(&self, layout: &Layout) -> LegacyLook {
        let changed = fingerprint_of(&self.0.root.join(SESSIONS))
            .is_ok_and(|now| Some(now) != layout.taken_up());
        if changed {
            LegacyLook::Changed(None)
        } else {
            LegacyLook::Same
        }
    }

    async fn legacy(&self) -> Result<Legacy, String> {
        read_with_fingerprint(&self.0.root.join(SESSIONS)).map_err(|err| err.to_string())
    }

    async fn legacy_deleted(&self) -> Vec<Deleted> {
        legacy_deleted(&self.0.root)
    }

    /// Moved out of the way, where it can be, and told of either way.
    async fn set_aside_history(&self, _raw: &[u8]) -> (Option<SetAside>, bool) {
        let root = &self.0.root;
        let moved = move_aside(root, &root.join(SESSIONS), "sessions");
        let kept = moved.as_deref().unwrap_or(SESSIONS);
        let aside = SetAside::History {
            kept: shown(root, kept),
        };
        (Some(aside), moved.is_some())
    }

    async fn set_aside_sessions(
        &self,
        unread: &str,
        count: usize,
    ) -> Result<Option<SetAside>, String> {
        let root = &self.0.root;
        let copied = copy_aside(root, unread.as_bytes())
            .map_err(|err| format!("the sessions that do not read: {err}"))?;
        Ok(copied.map(|name| SetAside::Sessions {
            count,
            kept: shown(root, &name),
        }))
    }

    /// The loose records alone, by name, beside the tombstones.
    async fn here(&self) -> io::Result<(BTreeSet<RecordId>, BTreeSet<RecordId>)> {
        Ok((ids_in(&self.0.records())?, ids_in(&self.0.tombstones())?))
    }

    async fn texts(&self, ids: &[RecordId]) -> io::Result<Vec<(RecordId, String)>> {
        let mut texts = Vec::new();
        for &id in ids {
            match fs::read(self.0.records().join(file_name(id))) {
                Ok(bytes) => texts.extend(String::from_utf8(bytes).ok().map(|text| (id, text))),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        Ok(texts)
    }

    async fn cut_short(&self) -> Vec<HistoryNotice> {
        self.0.clear_temporaries();
        self.0.journals_cut_short()
    }

    /// Taken away as this copy's own change: none for it to take up.
    async fn unlink(&self, ids: &[RecordId]) {
        let unlinking = self.0.watch.stamps();
        let records = self.0.records();
        let mut unlinked = false;
        for &id in ids {
            unlinked |= fs::remove_file(records.join(file_name(id))).is_ok();
        }
        if unlinked {
            let _ = sync_dir(&records);
        }
        self.0.watch.made(unlinking);
    }

    /// Step by step, each made durable before the next ([`make_steps`]); a
    /// restore of many sessions at once journaled while it runs, so the
    /// next to read the history can say it was cut short; and how the
    /// history is kept written after, where the change is made. What the
    /// change made is this copy's own: no change for it to take up. An
    /// empty one this copy makes of its own, as a take-up that only notes
    /// what it took, makes nothing.
    async fn commit(&self, writes: &Writes<'_>, laid: Laid<'_>, by: By) -> Made<io::Error> {
        let change = writes.change;
        let files = self.0;
        let journal = if restores_many(change) {
            match files.begin_journal(change) {
                Ok(journal) => Some(journal),
                Err(err) => {
                    return Made {
                        placed: Vec::new(),
                        saved: Err(err),
                    };
                }
            }
        } else {
            None
        };
        let (placed, made) = if by == By::Store && change.is_empty() {
            (Vec::new(), Ok(()))
        } else {
            let before = files.watch.stamps();
            let tombstones = writes.tombstones();
            let made = make_steps(&files.folders(), &change.put, &tombstones, &change.unforget);
            // Made or not, the change is over, and what became of it is said
            // now: a journal left behind says only that Dust stopped first.
            if let Some(journal) = journal {
                end_journal(journal, &files.dir);
            }
            files.watch.made(before);
            made
        };
        if made.is_ok()
            && let Laid::Over { next, .. } = laid
        {
            let noted = next
                .written()
                .map_err(io::Error::other)
                .and_then(|raw| write_whole(&files.dir, LAYOUT_FILE, &raw));
            if let Err(err) = noted {
                eprintln!("could not note what was taken up of an older build's history: {err}");
            }
        }
        Made {
            placed,
            saved: made.map(|()| HistorySaved::Stored {
                kept: change.put.len(),
            }),
        }
    }

    /// Each record flushed before it takes its name, on as many threads as
    /// it takes ([`place_each`]), all of them made durable, and the layout
    /// written last.
    async fn write_move(
        &self,
        records: &[Record],
        layout: &Layout,
    ) -> io::Result<Vec<Option<RecordId>>> {
        let files = self.0;
        files.progress.moving(Some(records.len() as u64));
        let placed = place_each(&files.folders(), records, &files.progress)?;
        sync_if_there(&files.records())?;
        let raw = layout.written().map_err(io::Error::other)?;
        write_whole(&files.dir, LAYOUT_FILE, &raw)?;
        Ok(placed.into_iter().map(Some).collect())
    }

    fn look(&self) -> Stamps {
        self.0.watch.stamps()
    }

    fn saw(&self, look: Stamps) {
        self.0.watch.looked(look);
    }

    fn held_back(&self) {
        self.0.watch.unsee();
    }
}

/// The history's folders, as a change is made in them step by step
/// ([`make_steps`]), and as the move stores its records, on as many threads
/// at once as it takes ([`place_each`]), `size` counting what they hold.
struct Folders<'a> {
    records: PathBuf,
    tombstones: PathBuf,
    size: &'a AtomicU64,
}

impl Steps for Folders<'_> {
    type Error = io::Error;

    fn tombstoned(&self, id: RecordId) -> bool {
        self.tombstones.join(file_name(id)).exists()
    }

    fn holds(&self, id: RecordId) -> bool {
        self.records.join(file_name(id)).exists()
    }

    fn there(&self, id: RecordId, json: &str) -> io::Result<There> {
        match fs::read(self.records.join(file_name(id))) {
            Ok(held) if held == json.as_bytes() => Ok(There::Same),
            Ok(_) => Ok(There::Other),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(There::Free),
            Err(err) => Err(err),
        }
    }

    /// Stored whole, and counted in what the history holds.
    fn add(&self, id: RecordId, json: &str) -> io::Result<()> {
        place_whole(&self.records, &file_name(id), json.as_bytes())?;
        self.size.fetch_add(json.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    fn no_name_free(&self, id: RecordId) -> io::Error {
        io::Error::other(format!("no name was free for the session stored as {id}"))
    }

    fn unforget(&self, ids: &[RecordId]) -> io::Result<()> {
        for &id in ids {
            remove_if_there(&self.tombstones.join(file_name(id)))?;
        }
        if !ids.is_empty() {
            sync_if_there(&self.tombstones)?;
        }
        Ok(())
    }

    fn records_durable(&self) -> io::Result<()> {
        sync_if_there(&self.records)
    }

    fn tombstone(&self, new: &[(RecordId, &str)]) -> io::Result<()> {
        let mut placed = Vec::new();
        let marked = (|| {
            for (id, held) in new {
                let name = file_name(*id);
                place_whole(&self.tombstones, &name, held.as_bytes())?;
                placed.push(name);
            }
            sync_dir(&self.tombstones)
        })();
        if let Err(err) = marked {
            for name in placed {
                let _ = fs::remove_file(self.tombstones.join(name));
            }
            let _ = sync_if_there(&self.tombstones);
            return Err(err);
        }
        Ok(())
    }

    /// A record beside its tombstone reads as deleted, and the next read
    /// takes it away ([`HistoryFiles::load`]). So a record that will not go
    /// now, as one held open elsewhere will not on Windows, leaves the
    /// change made; were it told of as failed, the app would show the
    /// session as kept, with nothing to undo, until the next read took it
    /// away.
    fn unlink(&self, ids: &[RecordId]) {
        for &id in ids {
            let path = self.records.join(file_name(id));
            let held = fs::metadata(&path).map_or(0, |meta| meta.len());
            match remove_if_there(&path) {
                Ok(()) => {
                    // Never fewer than none.
                    self.size
                        .update(Ordering::Relaxed, Ordering::Relaxed, |was| {
                            was.saturating_sub(held)
                        });
                }
                Err(err) => {
                    eprintln!("a deleted session's record stays until the next read: {err}");
                }
            }
        }
        if let Err(err) = sync_if_there(&self.records) {
            eprintln!("could not make the deleted sessions' going durable: {err}");
        }
    }
}

/// Store each of `records` in `folders` ([`place`]), each counted in `progress`
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
/// parts ever reach for the same name. Where each went, in their order:
/// under its own name, or one past it that other bytes left free.
fn place_each(
    folders: &Folders,
    records: &[Record],
    progress: &LoadProgress,
) -> io::Result<Vec<RecordId>> {
    let one = |record: &Record| place(folders, record).inspect(|_| progress.step());
    let part_of = |records: &[Record]| records.iter().map(one).collect::<io::Result<Vec<_>>>();
    if records.len() < 2 {
        return part_of(records);
    }
    let part = records.len().div_ceil(WRITERS);
    thread::scope(|scope| {
        let placing: Vec<_> = records
            .chunks(part)
            .map(|records| {
                let started = thread::Builder::new()
                    .name("dust-history-write".into())
                    .spawn_scoped(scope, move || part_of(records));
                (records, started)
            })
            .collect();
        // A part whose thread would not start, or stopped short, is stored
        // here instead: what it stored already is found stored.
        let mut placed = Vec::with_capacity(records.len());
        for (records, started) in placing {
            let part = started
                .ok()
                .and_then(|writer| writer.join().ok())
                .unwrap_or_else(|| part_of(records));
            placed.extend(part?);
        }
        Ok(placed)
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

/// The file `note` is kept in, in the history's folder.
fn note_file(note: Note) -> &'static str {
    match note {
        Note::Told => TOLD,
        Note::Failing => FAILING,
        Note::UnreadTold => UNREAD_TOLD,
        Note::DamagedTold => DAMAGED_TOLD,
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
    use crate::persist::conformance::Harness;
    use crate::persist::desktop::tests::TempDir;
    use crate::persist::record::{key_of, keyed, names_noted, records_for_blob, scored};
    use crate::persist::record_store::steps::BUMPS;
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

    /// A record whose name other bytes hold is tried under its own name and
    /// under each of the [`BUMPS`] past it, as the page script tries it:
    /// with every one of those but the last held, it goes under the last;
    /// with that one held too, the change fails, and nothing is written
    /// over. One name fewer was tried, and it failed a name early.
    #[test]
    fn a_record_is_tried_under_its_own_name_and_as_many_past_it_as_bumps_says() {
        let (dir, files) = history("bumps");
        files.load();
        let records = dir.0.join(HISTORY).join(RECORDS);
        fs::create_dir_all(&records).unwrap();
        // Other bytes under `held` names of `record`'s, from its own on; the
        // first name past them.
        let crowd = |record: &Record, held: u64| {
            let mut id = record.id;
            for _ in 0..held {
                fs::write(records.join(file_name(id)), "other bytes").unwrap();
                id = id.bumped();
            }
            id
        };
        let first = record(&finished(1_000));
        let last = crowd(&first, BUMPS);
        assert_eq!(
            files.apply(&put(std::slice::from_ref(&first))),
            HistorySaved::Stored { kept: 1 }
        );
        assert_eq!(
            fs::read_to_string(records.join(file_name(last))).unwrap(),
            first.json,
            "under the last name tried"
        );
        let second = record(&finished(2_000));
        let past = crowd(&second, BUMPS + 1);
        assert!(
            matches!(
                files.apply(&put(std::slice::from_ref(&second))),
                HistorySaved::Failed(_)
            ),
            "no name free"
        );
        assert!(!records.join(file_name(past)).exists(), "none past them");
    }

    /// The page script a browser's history goes through writes only to the
    /// layouts this build writes to, and tries a record under as many names
    /// past its own as the files do, where other bytes have its name: each
    /// says so in a constant of its own, which nothing else ties to this
    /// build's (`legacy::COMPATIBLE`, `BUMPS`).
    #[test]
    fn the_page_script_writes_to_the_layouts_and_tries_the_names_this_build_does() {
        let said = |name: &str| -> u64 {
            let line = crate::persist::idb::IDB_JS
                .lines()
                .find_map(|line| line.strip_prefix(&format!("const {name} = ")))
                .unwrap_or_else(|| panic!("the page script sets no {name}"));
            line.strip_suffix(';')
                .and_then(|value| value.parse().ok())
                .unwrap_or_else(|| panic!("the page script sets {name} = {line}"))
        };
        assert_eq!(said("COMPATIBLE"), legacy::COMPATIBLE);
        assert_eq!(said("BUMPS"), BUMPS);
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

    /// What this build notes of the records it told of, and of those that
    /// failed, is kept under the names older builds and this one read:
    /// `told.json`, which 0.16.4 to 0.16.9 go by too, and this build's own
    /// `damaged-told.json`, `unread-told.json` and `failing.json`, each in
    /// `history/`. Under another name a note is lost to every build that
    /// reads it, and what it held back is told of again.
    #[test]
    fn the_notes_of_what_was_told_keep_the_names_builds_read_them_by() {
        let (dir, files) = history("notes-by-name");
        files.load();
        let records = dir.0.join("history/sessions");
        let damaged = RecordId {
            order: 2_000_000,
            nonce: 1,
        };
        fs::create_dir_all(&records).unwrap();
        fs::write(records.join(format!("{}.json", damaged.name())), "[").unwrap();
        let busy = record(&finished(3_000));
        fs::create_dir_all(records.join(format!("{}.json", busy.id.name()))).unwrap();
        let read = files.load();
        assert_eq!(read.notices, [HistoryNotice::Damaged { count: 1 }]);
        assert_eq!(read.failed, [busy.id]);
        assert_eq!(
            files.still_unread(&[busy.id]),
            [HistoryNotice::Unread { count: 1 }]
        );
        let noted = |name: &str| -> BTreeSet<String> {
            let raw = fs::read(dir.0.join(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
            names_noted(Some(&raw)).into_iter().collect()
        };
        let (damaged, busy) = (damaged.name(), busy.id.name());
        assert_eq!(
            noted("history/told.json"),
            [damaged.clone(), busy.clone()].into()
        );
        assert_eq!(noted("history/damaged-told.json"), [damaged].into());
        assert_eq!(noted("history/unread-told.json"), [busy.clone()].into());
        assert_eq!(noted("history/failing.json"), [busy].into());
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

    /// A record a build before formats tells of first, once this build has
    /// written its own note of damage, is told of again here, where this
    /// build cannot read it either: such a build notes in told.json what it
    /// tells of, writing the note over then with every record it cannot
    /// read, as 0.16.9 and 95aa091 do, and which names there it wrote since
    /// this build last left the note, this build cannot tell, so it goes by
    /// its own note alone. Told of once here, it is told of no more. A note
    /// of what told.json named as this build left it would keep it from
    /// being told of twice; for so narrow a case, a build before formats
    /// used on the history again after this one, it is left as it is.
    #[test]
    fn a_record_an_older_build_tells_of_after_this_build_noted_damage_is_told_of_again_here() {
        let (dir, files) = history("older-tells-after-note");
        files.load();
        files.apply(&put(&[record(&finished(1_000))]));
        let records = dir.0.join(HISTORY).join(RECORDS);
        let ours = RecordId {
            order: 2_000_000,
            nonce: 1,
        };
        fs::write(records.join(file_name(ours)), "[").unwrap();
        assert_eq!(
            Files::at(&dir.0).load().notices,
            [HistoryNotice::Damaged { count: 1 }],
            "told of here"
        );
        let theirs = RecordId {
            order: 3_000_000,
            nonce: 1,
        };
        fs::write(records.join(file_name(theirs)), "[").unwrap();
        let told = dir.0.join(HISTORY).join(TOLD);
        fs::write(
            &told,
            serde_json::to_vec(&[ours.name(), theirs.name()]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            Files::at(&dir.0).load().notices,
            [HistoryNotice::Damaged { count: 1 }],
            "told of by that build, and again here"
        );
        assert_eq!(
            Files::at(&dir.0).load().notices,
            [],
            "not at the launch after"
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

    /// Which sessions a tombstone names, whatever record it was deleted
    /// under, is said on every platform, as the app's cut-off sessions ask it
    /// on every platform: one whose tombstone does not read is left out.
    #[test]
    fn which_sessions_a_tombstone_names_is_said_whatever_its_record() {
        let (dir, files) = history("deleted-keys");
        files.load();
        let (first, second, third) = (finished(1_000), finished(2_000), finished(3_000));
        let (one, two) = (record(&first), record(&second));
        files.apply(&put(&[one.clone(), two.clone()]));
        files.apply(&forget(&[(&one, &first)]));
        let tombstones = dir.0.join(HISTORY).join(TOMBSTONES);
        fs::write(tombstones.join(file_name(two.id)), "{ not").unwrap();
        let keys = [&first, &second, &third].map(SessionKey::of);
        assert_eq!(files.deleted_keys(&keys), Some(vec![keys[0].clone()]));
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

    /// A session deleted here under a name an older build's copy of it is
    /// not taken up by, as a fill-in's, is not taken up from that build's
    /// history while its tombstone cannot be read, as a folder in its place
    /// cannot, or a file held open: it may say the session was deleted. Nor
    /// does a move made again, as one whose layout no longer reads is, write
    /// it: the move waits. Once the tombstone reads, both are made, and the
    /// session stays deleted. Taken for no deletion, it came back, at the
    /// take-up and at the move. Which sessions the tombstones name, asked of
    /// the store, leaves it out meanwhile, as it always has: that is no word
    /// that it was not deleted.
    #[cfg(unix)]
    #[test]
    fn a_session_deleted_here_comes_back_from_no_older_build_while_its_tombstone_cannot_be_read() {
        let dir = TempDir::new("unread-tombstone");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = Files::at(&dir.0);
        let moved = files.load();
        let [(moved, _)] = moved.sessions[..] else {
            panic!("moved: {:?}", moved.sessions);
        };
        // Stored in the place of the one moved, as a restore fills one in,
        // and deleted by hand; then the older build, still open, stores it,
        // as one it holds as new; and its tombstone cannot be read.
        let session = finished(now + 60_000);
        let json = serde_json::to_string(&session).unwrap();
        let filled = Record {
            id: RecordId::beside(moved, &json),
            json,
        };
        files.apply(&put(std::slice::from_ref(&filled)));
        files.apply(&forget(&[(&filled, &session)]));
        keep_whole(&dir, &[old, session.clone()]);
        let tombstone = dir
            .0
            .join(HISTORY)
            .join(TOMBSTONES)
            .join(file_name(filled.id));
        let held = fs::read(&tombstone).unwrap();
        fs::remove_file(&tombstone).unwrap();
        fs::create_dir(&tombstone).unwrap();
        let files = Files::at(&dir.0);
        assert_eq!(ids(&files.load()), [moved], "nothing taken up");
        assert_eq!(
            files.deleted_keys(&[SessionKey::of(&session)]),
            Some(Vec::new()),
            "left out, as one that does not read is"
        );
        assert_eq!(files.listing(), Some(vec![moved]), "nor at a listing");
        // The layout damaged, the move made again waits.
        fs::write(dir.0.join(HISTORY).join(LAYOUT_FILE), "{ damaged").unwrap();
        let again = Files::at(&dir.0).load();
        assert!(
            matches!(&again.notices[..], [HistoryNotice::MovePending(_)]),
            "{:?}",
            again.notices
        );
        assert!(!again.complete);
        assert_eq!(ids(&again), [moved], "nothing moved");
        fs::remove_dir(&tombstone).unwrap();
        fs::write(&tombstone, held).unwrap();
        let read = Files::at(&dir.0).load();
        assert!(
            read.complete && read.notices.is_empty(),
            "{:?}",
            read.notices
        );
        assert_eq!(ids(&read), [moved], "still deleted");
        assert_eq!(files.listing(), Some(vec![moved]));
    }

    /// A take-up held back while a tombstone here cannot be read, as one
    /// held open cannot be, leaves what the older build stored to take up,
    /// at a listing and as the history is read: the history has changed, as
    /// the app is told, though no folder of it changes as the tombstone
    /// reads again, and the next look takes it up. Noted as seen by the look
    /// that held it back, it stayed out until the next launch, or another
    /// change.
    #[cfg(unix)]
    #[test]
    fn a_take_up_held_back_by_a_tombstone_that_cannot_be_read_is_still_to_take_up() {
        let dir = TempDir::new("held-back-by-a-tombstone");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = Files::at(&dir.0);
        let read = files.load();
        let [(moved, _)] = read.sessions[..] else {
            panic!("moved: {:?}", read.sessions);
        };
        // One deleted here, whose tombstone then cannot be read: a link to
        // a folder elsewhere, a file again later, the tombstones' folder
        // left as it was.
        let gone = finished(now - 5 * DAY);
        let one = record(&gone);
        files.apply(&put(std::slice::from_ref(&one)));
        files.apply(&forget(&[(&one, &gone)]));
        let tombstone = dir.0.join(HISTORY).join(TOMBSTONES).join(file_name(one.id));
        let held = fs::read(&tombstone).unwrap();
        let target = dir.0.join("held-open");
        fs::create_dir(&target).unwrap();
        fs::remove_file(&tombstone).unwrap();
        std::os::unix::fs::symlink(&target, &tombstone).unwrap();
        // An older build stores a session of its own since.
        keep_whole(&dir, &[old, finished(now + 60_000)]);
        assert!(files.changed());
        assert_eq!(files.listing(), Some(vec![moved]), "held back");
        assert!(files.changed(), "still to take up");
        let other = Files::at(&dir.0);
        assert_eq!(ids(&other.load()), [moved], "held back as it is read");
        assert!(other.changed(), "still to take up");
        fs::remove_dir(&target).unwrap();
        fs::write(&target, held).unwrap();
        assert!(files.changed() && other.changed());
        assert_eq!(
            files.listing().map(|listed| listed.len()),
            Some(2),
            "taken up"
        );
        assert!(!files.changed(), "taken up");
        assert_eq!(other.listing().map(|listed| listed.len()), Some(2));
        assert!(!other.changed());
    }

    /// A take-up whose change cannot be made just then, as where something
    /// stands for a moment in the place its record goes, leaves what the
    /// older build stored to take up, as one held back does: the history
    /// has changed, as the app is told, and the next look takes it up.
    /// Noted as seen, it stayed out until another change, or the next
    /// launch.
    #[cfg(unix)]
    #[test]
    fn a_take_up_that_cannot_be_made_now_is_still_to_take_up() {
        let dir = TempDir::new("take-up-not-made");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = Files::at(&dir.0);
        files.load();
        let newer = finished(now + 60_000);
        keep_whole(&dir, &[old, newer.clone()]);
        // Its place is taken once the history is listed for the take-up.
        let place = Record::of(&newer, |json| RecordId::placed(newer.timestamp, json))
            .expect("a record")
            .id;
        let path = dir.0.join(HISTORY).join(RECORDS).join(file_name(place));
        let standing = path.clone();
        let mut looks = 0;
        files.meanwhile(move || {
            looks += 1;
            if looks == 3 {
                fs::create_dir(&standing).unwrap();
            }
        });
        files.listing();
        assert!(files.changed(), "still to take up");
        fs::remove_dir(&path).unwrap();
        assert_eq!(
            files.listing().map(|listed| listed.len()),
            Some(2),
            "taken up"
        );
        assert!(!files.changed(), "taken up");
    }

    /// A history kept whole that cannot be looked at now, as a link that
    /// leads back to itself cannot, is taken as unchanged since it was last
    /// taken up: nothing is taken up of it until it can be, and nothing is
    /// held back meanwhile ([`LegacyLook::Same`]). Taken as changed, as the
    /// files' look at it said it was, it could not be read either, and each
    /// look would hold the take-up back and tell of a change again.
    #[cfg(unix)]
    #[test]
    fn a_history_kept_whole_that_cannot_be_looked_at_is_taken_as_unchanged() {
        let dir = TempDir::new("cannot-be-looked-at");
        keep_whole(&dir, &[finished(1_000), finished(2_000)]);
        let files = Files::at(&dir.0);
        let moved = ids(&files.load());
        let sessions = dir.0.join(SESSIONS);
        fs::remove_file(&sessions).unwrap();
        std::os::unix::fs::symlink(&sessions, &sessions).unwrap();
        assert_eq!(files.listing(), Some(moved.clone()));
        assert!(!files.changed(), "nothing held back");
        assert_eq!(files.listing(), Some(moved));
    }

    /// A tombstone gone by the time it is read, as another copy's put-back
    /// takes one away between the listing of the tombstones and its read,
    /// names no session, as every other reader of them takes it: a move
    /// made over it, and a take-up, go on without it. Taken for one that
    /// cannot be read, it made the move pending, and held the take-up back
    /// for that look.
    #[cfg(unix)]
    #[test]
    fn a_tombstone_gone_as_it_is_read_names_no_session() {
        use crate::persist::record_store::{Reading, keys_of_tombstones};
        let (dir, files) = raw_history("tombstone-gone-as-read");
        let tombstones = dir.0.join(HISTORY).join(TOMBSTONES);
        fs::create_dir_all(&tombstones).unwrap();
        // Listed, and gone as it is read: a link to nothing.
        let id = record(&finished(1_000)).id;
        std::os::unix::fs::symlink(dir.0.join("nothing"), tombstones.join(file_name(id))).unwrap();
        let tombstones = at_once(Disk(&files).tombstones())
            .expect("at once")
            .expect("listed");
        assert_eq!(
            keys_of_tombstones(&tombstones, Reading::Strict),
            Ok(HashSet::new())
        );
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let moved = files.load();
        assert!(
            moved.complete && moved.notices.is_empty(),
            "{:?}",
            moved.notices
        );
        assert_eq!(stamps(&moved), [old.timestamp]);
        keep_whole(&dir, &[old, finished(now + 60_000)]);
        assert_eq!(
            files.listing().map(|listed| listed.len()),
            Some(2),
            "taken up"
        );
    }

    /// An Undo another copy makes of a deletion here as this one takes up
    /// what an older build stored, between this one's listing of the
    /// records and its read of the tombstones, leaves the session stored
    /// once, at a listing and as the history is read: the tombstones read
    /// before the records still name it. Read after them alone, they left
    /// the session neither here nor deleted, and it was taken up beside the
    /// record the Undo put back.
    #[test]
    fn an_undo_another_copy_makes_as_this_one_takes_up_leaves_one_record() {
        for reading_whole in [false, true] {
            let dir = TempDir::new("undo-as-taken-up");
            let now = crate::time::now_ms();
            let old = finished(now - 10 * DAY);
            keep_whole(&dir, std::slice::from_ref(&old));
            let files = Files::at(&dir.0);
            files.load();
            // Held here as a later build stored it, so not as the older
            // build's bytes, and deleted here.
            let session = finished(now - 5 * DAY);
            let mut held = serde_json::to_value(&session).unwrap();
            held["laterKey"] = serde_json::json!(1);
            let json = serde_json::to_string(&held).unwrap();
            let here = Record {
                id: RecordId::placed(session.timestamp, &json),
                json,
            };
            files.apply(&put(std::slice::from_ref(&here)));
            files.apply(&forget(&[(&here, &session)]));
            // An older build's copy, holding it as new, writes its history
            // back; and another copy undoes the deletion once this one has
            // listed the records.
            keep_whole(&dir, &[old, session.clone()]);
            let history = dir.0.join(HISTORY);
            let (tombstone, record) = (
                history.join(TOMBSTONES).join(file_name(here.id)),
                history.join(RECORDS).join(file_name(here.id)),
            );
            let looking = if reading_whole {
                Files::at(&dir.0)
            } else {
                files
            };
            let mut looks = 0;
            let undone = here.json.clone();
            looking.meanwhile(move || {
                looks += 1;
                if looks == 3 {
                    fs::remove_file(&tombstone).unwrap();
                    fs::write(&record, &undone).unwrap();
                }
            });
            if reading_whole {
                looking.load();
            } else {
                looking.listing();
            }
            let key = SessionKey::of(&session);
            let holding = names(&history.join(RECORDS))
                .iter()
                .filter(|name| {
                    fs::read_to_string(history.join(RECORDS).join(name))
                        .is_ok_and(|json| key_of(&json).as_ref() == Some(&key))
                })
                .count();
            assert_eq!(holding, 1, "reading whole {reading_whole}");
        }
    }

    /// Which records were deleted by hand comes with when each was, as its
    /// tombstone says, none for one that does not say; and which session
    /// each was, where its tombstone says. On every platform, as the
    /// app's cut-off sessions ask it on every platform.
    #[test]
    fn deletions_say_when_each_was_made_and_of_which_session() {
        let (dir, files) = history("deletions-said");
        files.load();
        let (first, second, third) = (finished(1_000), finished(2_000), finished(3_000));
        let (one, two, three) = (record(&first), record(&second), record(&third));
        files.apply(&put(&[one.clone(), two.clone(), three.clone()]));
        files.apply(&forget(&[(&one, &first)]));
        let tombstones = dir.0.join(HISTORY).join(TOMBSTONES);
        fs::write(tombstones.join(file_name(two.id)), "{ not").unwrap();
        let all = [one.id, two.id, three.id];
        assert_eq!(
            files.deletions(&all),
            Some(vec![(one.id, Some(7)), (two.id, None)])
        );
        assert_eq!(
            files.deleted_sessions(&all),
            Some(vec![(one.id, SessionKey::of(&first))]),
            "which session, where its tombstone says"
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

    /// What a read of the history whole takes up from an older build is no
    /// change for the copy that read it to take up: it stored it itself.
    /// The read noted the folders as they were before the take-up wrote,
    /// so its own records looked like another copy's, and it listed once
    /// more.
    #[test]
    fn what_a_read_takes_up_from_an_older_build_is_no_change_for_it_to_take_up() {
        let dir = TempDir::new("own-take-up-at-read");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        Files::at(&dir.0).load();
        keep_whole(&dir, &[old, finished(now + 60_000)]);
        let files = Files::at(&dir.0);
        assert_eq!(files.load().sessions.len(), 2, "taken up");
        assert!(!files.changed(), "its own");
    }

    /// What a listing takes up from an older build is no change for the
    /// copy that listed to take up, as the read's is not. It noted the
    /// folders as they were before the take-up wrote.
    #[test]
    fn what_a_listing_takes_up_from_an_older_build_is_no_change_for_it_to_take_up() {
        let dir = TempDir::new("own-take-up-at-listing");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = Files::at(&dir.0);
        files.load();
        keep_whole(&dir, &[old, finished(now + 60_000)]);
        assert!(files.changed(), "the history kept whole changed");
        assert_eq!(files.listing().map(|listed| listed.len()), Some(2));
        assert!(!files.changed(), "its own");
    }

    /// A deletion cut short that a read of the history whole finishes, its
    /// record taken away from beside its tombstone, is no change for the
    /// copy that read it to take up. The read noted the folders as they
    /// were before it took the record away.
    #[test]
    fn a_deletion_a_read_finishes_is_no_change_for_it_to_take_up() {
        let (dir, files) = history("own-deletion-finished");
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
        let files = Files::at(&dir.0);
        assert!(files.load().sessions.is_empty());
        assert!(
            names(&dir.0.join(HISTORY).join(RECORDS)).is_empty(),
            "finished"
        );
        assert!(!files.changed(), "its own");
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
        place(&files.folders(), &restored[0]).unwrap();
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
        let moved = files.load();
        let live = keyed(moved.sessions.iter().map(|(id, session)| (*id, session)));
        // An older build stores two sessions: taken up together, and no
        // restore.
        let (first, second) = (finished(now + 60_000), finished(now + 120_000));
        let whole = [old.clone(), first.clone(), second.clone()];
        keep_whole(&dir, &whole);
        let Ok(Gate::Open(layout)) = files.gate() else {
            panic!("not moved");
        };
        let fingerprint = fingerprint_of(&dir.0.join(SESSIONS)).unwrap();
        let (change, _) =
            legacy::imported(&layout, fingerprint, &whole, &[], &live, &HashSet::new())
                .expect("something new");
        assert_eq!(change.put.len(), 2);
        assert!(!restores_many(&change));
        // The second cannot be taken up for now, every name it could go
        // under held by other bytes: the take-up is tried again, the first
        // stored already, and leaves nothing to tell of.
        let records = dir.0.join(HISTORY).join(RECORDS);
        let mut in_the_way = Vec::new();
        let mut id = change.put[1].id;
        for _ in 0..=BUMPS {
            fs::write(records.join(file_name(id)), "other bytes").unwrap();
            in_the_way.push(id);
            id = id.bumped();
        }
        assert!(files.listing().is_some());
        assert!(files.watch.changed(), "held back, to be tried again");
        assert!(journals(&dir).is_empty(), "{:?}", journals(&dir));
        for id in in_the_way {
            fs::remove_file(records.join(file_name(id))).unwrap();
        }
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

    /// A record whose bytes are no text, which no build reads, holds back no
    /// take-up of what an older build stored in the history it keeps whole
    /// as a look lists the history: it holds no session, as a damaged one
    /// holds none, and the older build's session is taken up beside it,
    /// which is left as it is. Only one that cannot be read for now holds
    /// the take-up back, as it may hold that session.
    #[test]
    fn a_record_whose_bytes_are_no_text_holds_back_no_take_up_from_an_older_build() {
        let dir = TempDir::new("older-no-text");
        let now = crate::time::now_ms();
        let old = finished(now - 10 * DAY);
        keep_whole(&dir, std::slice::from_ref(&old));
        let files = Files::at(&dir.0);
        files.load();
        // Damaged once this copy read the history: no text at all.
        let records = dir.0.join(HISTORY).join(RECORDS);
        let damaged = RecordId {
            order: (now + 60_000) * 1000,
            nonce: 1,
        };
        let bytes = b"{\"date\":\"\xff\xfe\",\"timestamp\":1}";
        fs::write(records.join(file_name(damaged)), bytes).unwrap();
        let theirs = finished(now + 120_000);
        keep_whole(&dir, &[old.clone(), theirs.clone()]);
        assert_eq!(
            files.listing().map(|listed| listed.len()),
            Some(3),
            "taken up as it is listed"
        );
        assert_eq!(names(&records).len(), 3);
        let read = Files::at(&dir.0).load();
        assert_eq!(stamps(&read), [old.timestamp, theirs.timestamp]);
        assert_eq!(read.unread, [damaged]);
        assert_eq!(read.notices, [HistoryNotice::Damaged { count: 1 }]);
        assert_eq!(
            fs::read(records.join(file_name(damaged))).unwrap(),
            bytes,
            "left as it is"
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

    /// Two deletions of one session an older build remembers, as two of its
    /// copies each deleted it, are made here as one, its tombstone stamped
    /// with the later, as in a browser: the first was kept here, the last
    /// in a browser.
    #[test]
    fn two_deletions_of_one_session_an_older_build_made_are_one_tombstone_stamped_with_the_later() {
        let dir = TempDir::new("older-deleted-twice");
        let now = crate::time::now_ms();
        let (first, second) = (finished(now - 10 * DAY), finished(now - 9 * DAY));
        keep_whole(&dir, &[first.clone(), second.clone()]);
        let read = Files::at(&dir.0).load();
        let second_id = read.sessions[1].0;
        keep_whole(&dir, std::slice::from_ref(&first));
        let deleted = [Deleted::of(&second, now), Deleted::of(&second, now + 1)];
        fs::write(dir.0.join(DELETED), serde_json::to_vec(&deleted).unwrap()).unwrap();
        let files = Files::at(&dir.0);
        assert_eq!(stamps(&files.load()), [first.timestamp]);
        assert_eq!(
            files.deletions(&[second_id]),
            Some(vec![(second_id, Some(now + 1))])
        );
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

    /// A move made again leaves a session deleted here deleted, though its
    /// deletion was cut short, its record left beside its tombstone, and
    /// that record was damaged where it lies since: the move's record of it,
    /// under the same name, is written nowhere. The record there was taken
    /// for the session, there by its own name, before its tombstone was
    /// looked at, so the move stored the session under the next name, which
    /// no tombstone names, and it came back.
    #[test]
    fn a_move_made_again_leaves_deleted_a_session_whose_record_stayed_beside_its_tombstone_damaged()
    {
        let dir = TempDir::new("move-again-beside-tombstone");
        let now = crate::time::now_ms();
        let session = finished(now - 9 * DAY);
        keep_whole(&dir, std::slice::from_ref(&session));
        let moved = Files::at(&dir.0).load();
        let [(id, _)] = moved.sessions[..] else {
            panic!("moved: {:?}", moved.sessions);
        };
        // Deleted by hand, cut short with its record still there, which is
        // damaged since; then the layout is.
        let history = dir.0.join(HISTORY);
        fs::create_dir_all(history.join(TOMBSTONES)).unwrap();
        fs::write(
            history.join(TOMBSTONES).join(file_name(id)),
            serde_json::to_vec(&Deleted::of(&session, 7)).unwrap(),
        )
        .unwrap();
        fs::write(history.join(RECORDS).join(file_name(id)), "{ damaged").unwrap();
        fs::write(history.join(LAYOUT_FILE), "{ damaged").unwrap();
        let again = Files::at(&dir.0).load();
        assert!(again.complete);
        assert_eq!(stamps(&again), Vec::<u64>::new(), "still deleted");
        assert!(
            !history.join(RECORDS).join(file_name(id.bumped())).exists(),
            "not stored under the next name"
        );
        assert_eq!(Files::at(&dir.0).listing(), Some(Vec::new()));
    }

    /// A move made over records stored while it waited, as a restore stores
    /// one while the history kept whole cannot be read, waits again while a
    /// record here under a name the move does not give cannot be read, as a
    /// folder in its place cannot, or a file held open: it may hold a session
    /// the move would write a second time. Once it reads, the move is made,
    /// each session once. Passed over, it was taken to hold nothing, the move
    /// wrote its session again under the move's own name, and once it read,
    /// the session was there twice.
    #[cfg(unix)]
    #[test]
    fn a_move_made_over_a_record_that_cannot_be_read_now_waits_for_it() {
        let dir = TempDir::new("move-over-unread");
        let now = crate::time::now_ms();
        let (first, second) = (finished(now - 9 * DAY), finished(now - 8 * DAY));
        keep_whole(&dir, &[first.clone(), second.clone()]);
        let sessions = dir.0.join(SESSIONS);
        let whole = fs::read(&sessions).unwrap();
        fs::remove_file(&sessions).unwrap();
        fs::create_dir(&sessions).unwrap();
        let files = Files::at(&dir.0);
        assert!(!files.load().complete, "the move waits");
        // Restored meanwhile, under the name a restore gives it.
        let restored =
            Record::of(&first, |json| RecordId::placed(first.timestamp, json)).expect("a record");
        assert_eq!(
            files.apply(&put(std::slice::from_ref(&restored))),
            HistorySaved::Stored { kept: 1 }
        );
        fs::remove_dir(&sessions).unwrap();
        fs::write(&sessions, whole).unwrap();
        // It cannot be read just as the move is made.
        let path = dir
            .0
            .join(HISTORY)
            .join(RECORDS)
            .join(file_name(restored.id));
        let held = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let waited = Files::at(&dir.0).load();
        assert!(
            matches!(&waited.notices[..], [HistoryNotice::MovePending(_)]),
            "{:?}",
            waited.notices
        );
        assert!(!waited.complete);
        assert_eq!(waited.failed, [restored.id]);
        fs::remove_dir(&path).unwrap();
        fs::write(&path, held).unwrap();
        let read = Files::at(&dir.0).load();
        assert!(
            read.complete && read.notices.is_empty(),
            "{:?}",
            read.notices
        );
        assert_eq!(
            stamps(&read),
            [first.timestamp, second.timestamp],
            "each once"
        );
        assert_eq!(names(&dir.0.join(HISTORY).join(RECORDS)).len(), 2);
    }

    /// A move with nothing to move, as one made again over a layout that no
    /// longer reads, where no history is kept whole, writes nothing, and so
    /// waits for nothing: a record or a tombstone here that cannot be read
    /// now holds it back no more than it holds back a first move. It
    /// waited, and the app told of sessions from before this version that
    /// could not be read, where there were none.
    #[cfg(unix)]
    #[test]
    fn a_move_with_nothing_to_move_waits_for_nothing() {
        let (dir, files) = history("move-nothing");
        files.load();
        let (first, second) = (finished(1_000), finished(2_000));
        let (one, two) = (record(&first), record(&second));
        files.apply(&put(&[one.clone(), two.clone()]));
        files.apply(&forget(&[(&two, &second)]));
        // The one left cannot be read now, nor the other's tombstone; and
        // the layout is damaged.
        let history = dir.0.join(HISTORY);
        for path in [
            history.join(RECORDS).join(file_name(one.id)),
            history.join(TOMBSTONES).join(file_name(two.id)),
        ] {
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
        }
        fs::write(history.join(LAYOUT_FILE), "{ damaged").unwrap();
        let again = Files::at(&dir.0).load();
        assert!(
            again.complete && again.notices.is_empty(),
            "{:?}",
            again.notices
        );
        assert_eq!(again.failed, [one.id]);
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

    /// Every operation on the files, made through the record store, is
    /// done as it is first asked, as the worker has it done ([`at_once`]):
    /// the move, a change, a take-up of what an older build stored, the
    /// reads, and each ask of what was deleted or is still unread.
    #[test]
    fn every_operation_on_the_files_is_done_as_it_is_first_asked() {
        let (dir, files) = raw_history("first-poll");
        let (first, second, later) = (finished(DAY), finished(2 * DAY), finished(3 * DAY));
        keep_whole(&dir, &[first.clone(), second.clone()]);
        let disk = Disk(&files);
        let moved = at_once(record_store::load(&disk)).expect("the move, at once");
        assert_eq!(stamps(&moved.expect("read")), [DAY, 2 * DAY]);
        assert_eq!(
            at_once(record_store::apply(&disk, &put(&[record(&later)]))),
            Some(HistorySaved::Stored { kept: 1 })
        );
        let listed = at_once(record_store::listing(&disk)).expect("a listing, at once");
        let listed = listed.expect("listed");
        assert_eq!(listed.len(), 3);
        let read = at_once(record_store::read(&disk, &listed)).expect("a read, at once");
        assert!(read.iter().all(Result::is_ok), "{read:?}");
        let gone: &Record = read[0].as_ref().unwrap();
        let session = read_record(&gone.json).expect("a session");
        assert_eq!(
            at_once(record_store::apply(&disk, &forget(&[(gone, &session)]))),
            Some(HistorySaved::Stored { kept: 0 })
        );
        // An older build stores a session since, which a load takes up.
        keep_whole(&dir, &[first, second, finished(4 * DAY)]);
        let load = at_once(record_store::load(&disk)).expect("a take-up, at once");
        assert_eq!(load.expect("read").sessions.len(), 3);
        let deletions = at_once(record_store::deletions(&disk, &[gone.id])).expect("at once");
        assert_eq!(deletions.map(|found| found.len()), Some(1));
        let deleted = at_once(record_store::deleted_sessions(&disk, &[gone.id])).expect("at once");
        assert_eq!(deleted.map(|found| found.len()), Some(1));
        let key = SessionKey::of(&session);
        let keys = at_once(record_store::deleted_keys(&disk, vec![key.clone()])).expect("at once");
        assert_eq!(keys, Some(vec![key]));
        let unread = at_once(record_store::still_unread(&disk, &[gone.id])).expect("at once");
        assert_eq!(unread, [HistoryNotice::Unread { count: 1 }]);
    }

    /// The files, on a folder of their own, as every store's cases reach
    /// them ([`Harness`]).
    struct OnDisk {
        dir: TempDir,
        files: HistoryFiles,
    }

    impl Harness for OnDisk {
        fn holding(whole: &[SessionResult]) -> Self {
            let dir = TempDir::new("conformance");
            if !whole.is_empty() {
                keep_whole(&dir, whole);
            }
            let files = HistoryFiles::at(&dir.0);
            Self { dir, files }
        }

        fn load(&mut self) -> HistoryLoad {
            self.files.load()
        }

        fn apply(&self, change: HistoryChange) -> HistorySaved {
            self.files.apply(&change)
        }

        fn listing(&self) -> Option<Vec<RecordId>> {
            self.files.listing()
        }

        fn read(&self, ids: Vec<RecordId>) -> Vec<Option<String>> {
            let read = self.files.read(&ids).into_iter();
            read.map(|read| read.ok().map(|record| record.json))
                .collect()
        }

        fn deletions(&self, ids: Vec<RecordId>) -> Option<Deletions> {
            self.files.deletions(&ids)
        }

        fn deleted_sessions(&self, ids: Vec<RecordId>) -> Option<Vec<(RecordId, SessionKey)>> {
            self.files.deleted_sessions(&ids)
        }

        fn deleted_keys(&self, keys: Vec<SessionKey>) -> Option<Vec<SessionKey>> {
            self.files.deleted_keys(&keys)
        }

        fn still_unread(&self, ids: Vec<RecordId>) -> Vec<HistoryNotice> {
            self.files.still_unread(&ids)
        }

        fn changed(&self) -> bool {
            self.files.watch().changed()
        }

        fn keep_whole(&self, sessions: &[SessionResult]) {
            keep_whole(&self.dir, sessions);
        }

        fn keep_deleted(&self, deleted: &[Deleted]) {
            fs::write(
                self.dir.0.join(DELETED),
                serde_json::to_vec(deleted).unwrap(),
            )
            .unwrap();
        }

        fn plant(&self, id: RecordId, json: &str) {
            let records = self.dir.0.join(HISTORY).join(RECORDS);
            fs::create_dir_all(&records).unwrap();
            fs::write(records.join(file_name(id)), json).unwrap();
        }
    }

    /// What every store must do with the history ([`crate::persist::conformance`]).
    mod conformance {
        use super::OnDisk;

        crate::persist::conformance::conformance_tests!(OnDisk:
            a_history_kept_whole_moves_in_once,
            a_deletion_is_a_tombstone_that_names_its_session,
            a_session_put_back_is_there_again,
            what_an_older_build_stores_after_the_move_is_taken_up_once,
            a_deletion_an_older_build_made_after_the_move_is_honoured,
            a_record_this_build_cannot_read_is_told_of_once,
            records_still_unread_are_told_of_once,
            a_session_taken_up_under_a_name_other_bytes_have_is_read_where_it_went,
            a_session_stored_under_a_name_other_bytes_have_is_known_by_where_it_went,
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
        deletions_say_when_each_was_made_and_of_which_session,
        #[cfg(unix)]
        deletions_say_when_each_was_made_or_that_none_could_be_looked_for,
        which_sessions_a_tombstone_names_is_said_whatever_its_record,
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
        #[cfg(unix)]
        a_session_deleted_here_comes_back_from_no_older_build_while_its_tombstone_cannot_be_read,
        #[cfg(unix)]
        a_take_up_held_back_by_a_tombstone_that_cannot_be_read_is_still_to_take_up,
        #[cfg(unix)]
        a_take_up_that_cannot_be_made_now_is_still_to_take_up,
        #[cfg(unix)]
        a_history_kept_whole_that_cannot_be_looked_at_is_taken_as_unchanged,
        an_undo_another_copy_makes_as_this_one_takes_up_leaves_one_record,
        a_deletion_an_older_build_made_after_the_move_is_honoured,
        a_deletion_an_older_build_made_after_the_move_is_honoured_whatever_its_clock,
        a_layout_that_does_not_read_is_set_aside_and_the_move_made_again,
        #[cfg(unix)]
        a_move_made_over_a_record_that_cannot_be_read_now_waits_for_it,
        #[cfg(unix)]
        a_move_with_nothing_to_move_waits_for_nothing,
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
