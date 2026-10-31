//! Files under one directory: the desktop and mobile store.

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use cw_core::{AutoLevelCounters, CountersKey, TrainingSettings};

use super::backup::{BackedUp, BackupFile, NotSaved};
use super::files::HistoryFiles;
use super::record::{HistoryChange, HistoryLoad, Pending, Record, RecordId};
use super::worker::{FLUSH_WITHIN, HistoryJobs, Loading};
use super::{
    BackupSaving, CutOff, CutOffTaking, HistorySaved, OpenSession, SETTINGS_HELD, SetAside, Store,
    over_stored, read_settings_from,
};

const THEME: &str = "theme.txt";
const TUTORIAL: &str = "tutorial_done.txt";
const TOLD_OF_PADDLE_KEYS: &str = "told_of_paddle_keys.txt";
const ASKED_ABOUT_LOCK: &str = "asked_about_lock.txt";
const SETTINGS: &str = "settings.json";
/// The history as builds before records keep it whole, which the move to
/// records reads once and leaves as it was ([`HistoryFiles`]).
pub(super) const SESSIONS: &str = "sessions.json";
/// The sessions deleted by hand ([`super::Deleted`]), as builds before
/// records remember them.
pub(super) const DELETED: &str = "deleted.json";
const COUNTERS: &str = "auto_adjust.json";
const LAST_BACKUP: &str = "last_backup.txt";
/// The sessions and runs marked open ([`Store::mark_open`]): each copy of
/// the app's as `<id>.json`, beside `<id>.lock`, which that copy holds
/// locked for as long as its session runs. A copy that stops, however it
/// stops, lets go of its lock, so a mark nobody holds was cut off, and one
/// held belongs to a window still running it.
const OPEN: &str = "open";

/// How large the history may grow before the backup card says Android's
/// own backup of the app is about to leave it out: 20 MB, short of the
/// 25 MB past which Android backs up nothing of an app at all.
const DEVICE_BACKUP_WARN_BYTES: u64 = 20_000_000;

/// Whether a history of `bytes`, on Android or not, is past what Android's
/// own backup of the app takes, or nearly: only Android's has such a
/// limit, and an iPhone's iCloud backup none an app need fear.
fn outgrows_device_backup(bytes: u64, android: bool) -> bool {
    android && bytes > DEVICE_BACKUP_WARN_BYTES
}

/// Files under one directory. The directory is a field rather than a global
/// so a test can point one at a scratch folder.
pub struct DesktopStore {
    root: PathBuf,
    /// Where backup files go, when there is anywhere the user can reach.
    backup_folder: Option<PathBuf>,
    /// How the user moves a backup file out of the app, where it is saved
    /// inside it ([`Store::backups_inside_app`]).
    backups_inside: Option<&'static str>,
    /// The stored settings could not be read, nor moved out of the way, the
    /// last time they were read, so they are not to be written over until
    /// a read says otherwise.
    settings_held: Cell<bool>,
    /// What reads set aside, not yet told of ([`Store::take_set_aside`]).
    set_aside: RefCell<Vec<SetAside>>,
    /// This copy's mark, while its session is marked open.
    open_held: RefCell<Option<HeldMark>>,
    /// The history by record, under `history/` ([`HistoryFiles`]), read and
    /// written on a thread of its own ([`HistoryJobs`]).
    history: HistoryJobs,
    /// The history as it is being read for the app to open on, started
    /// as the app started ([`preload`]): taken by the first load.
    loading: RefCell<Option<Loading>>,
}

/// The history read as the app starts ([`preload`]), for the store the app
/// opens on to take.
struct Preloaded {
    root: PathBuf,
    history: HistoryJobs,
    loading: Loading,
}

static PRELOADED: Mutex<Option<Preloaded>> = Mutex::new(None);

/// Start reading the history, on its own thread, as the app starts: before
/// its window is up, so the webview starts up meanwhile, and the app opens
/// on what is left of it to read, if anything.
pub(super) fn preload() {
    let root = data_dir();
    let history = HistoryJobs::on_worker(HistoryFiles::at(&root));
    let loading = history.start_load();
    *PRELOADED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Preloaded {
        root,
        history,
        loading,
    });
}

/// A copy's mark ([`OPEN`]): its id, and the lock it holds on it.
struct HeldMark {
    id: String,
    lock: fs::File,
}

impl DesktopStore {
    /// The app's store, its history read and written on a thread of its
    /// own: the one started as the app started, where it is the same
    /// folder's ([`preload`]).
    pub fn new() -> Self {
        let root = data_dir();
        let preloaded = PRELOADED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .filter(|preloaded| preloaded.root == root);
        let (history, loading) = match preloaded {
            Some(Preloaded {
                history, loading, ..
            }) => (history, Some(loading)),
            None => (HistoryJobs::on_worker(HistoryFiles::at(&root)), None),
        };
        Self {
            backup_folder: backup_folder(),
            backups_inside: backups_inside_app(),
            loading: RefCell::new(loading),
            ..Self::with_history(root, history)
        }
    }

    /// Files under `root`, and no backups, the history read and written
    /// then and there: for a test, which looks at once.
    #[cfg(test)]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let history = HistoryJobs::inline(HistoryFiles::at(&root));
        Self::with_history(root, history)
    }

    /// Files under `root`, and no backups, the history read and written on
    /// a thread of its own, as the app's are.
    #[cfg(test)]
    pub fn on_worker_at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let history = HistoryJobs::on_worker(HistoryFiles::at(&root));
        Self::with_history(root, history)
    }

    fn with_history(root: PathBuf, history: HistoryJobs) -> Self {
        Self {
            history,
            loading: RefCell::new(None),
            root,
            backup_folder: None,
            backups_inside: None,
            settings_held: Cell::new(false),
            set_aside: RefCell::new(Vec::new()),
            open_held: RefCell::new(None),
        }
    }

    /// Backup files go into `folder`.
    #[cfg(test)]
    pub(super) fn with_backup_folder(self, folder: impl Into<PathBuf>) -> Self {
        Self {
            backup_folder: Some(folder.into()),
            ..self
        }
    }

    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        write_whole(&self.root, name, bytes)
    }

    /// Every counter stored that can be read, by what it counts.
    fn load_all_counters(&self) -> Counters {
        fs::read(self.root.join(COUNTERS))
            .map(|raw| read_counters(&raw).0)
            .unwrap_or_default()
    }

    /// Change the stored counters, read afresh just before they are
    /// written, so that what another copy of the app stored meanwhile is
    /// kept. Counters that cannot be read are set aside beside the file
    /// first rather than written over; a file there that cannot be read at
    /// all, or set aside, is left as it is.
    fn update_counters(&self, change: impl FnOnce(&mut Counters)) {
        let (mut all, mut stored) = match fs::read(self.root.join(COUNTERS)) {
            Ok(raw) => {
                let (all, whole) = read_counters(&raw);
                if !whole && let Err(err) = self.write(&aside_name(&self.root, "auto_adjust"), &raw)
                {
                    eprintln!(
                        "could not set aside auto-level counters that could not be read: {err}"
                    );
                    return;
                }
                let stored: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_slice(&raw).unwrap_or_default();
                (all, stored)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                (Counters::new(), serde_json::Map::new())
            }
            Err(err) => {
                eprintln!("could not read the auto-level counters: {err}");
                return;
            }
        };
        change(&mut all);
        // Each over what it held, so what a later build added to it stays.
        let written: serde_json::Map<String, serde_json::Value> = all
            .into_iter()
            .filter_map(|(key, counters)| {
                let value = over_stored(serde_json::to_value(counters).ok()?, stored.remove(&key));
                Some((key, value))
            })
            .collect();
        if let Ok(raw) = serde_json::to_vec_pretty(&written) {
            let _ = self.write(COUNTERS, &raw);
        }
    }

    /// Move a file that cannot be read, `stem`.json at `path`, out of the
    /// way of the next save ([`move_aside`]).
    fn set_aside(&self, path: &Path, stem: &str) -> Option<String> {
        move_aside(&self.root, path, stem)
    }

    /// Where the user can find `name`, a file of the store's ([`shown`]).
    fn shown(&self, name: &str) -> Option<String> {
        shown(&self.root, name)
    }
}

/// Move a file in `root` that cannot be read, `stem`.json at `path`, out of
/// the way of the next save, keeping it beside the new one to be recovered,
/// and say what it is called now: nothing, when it could not be moved.
pub(super) fn move_aside(root: &Path, path: &Path, stem: &str) -> Option<String> {
    let name = aside_name(root, stem);
    match fs::rename(path, root.join(&name)) {
        Ok(()) => Some(name),
        Err(err) => {
            eprintln!("could not set aside {stem}.json, which could not be read: {err}");
            None
        }
    }
}

/// Where the user can find `name`, a file of the store's in `root`, in
/// words for them: on a computer. A phone keeps the store inside the app,
/// where no file manager reaches.
pub(super) fn shown(root: &Path, name: &str) -> Option<String> {
    let reachable = !cfg!(any(target_os = "android", target_os = "ios"));
    reachable.then(|| root.join(name).display().to_string())
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

/// A name in `dir` no file has yet for one set aside from `stem`.json:
/// `stem.unreadable-<seconds>.json`, or failing that with `-2`, `-3`… after
/// it, so that one set aside before is never written over.
fn aside_name(dir: &Path, stem: &str) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    free_name(dir, &format!("{stem}.unreadable-{stamp}.json"))
}

/// The auto-level counters, by what each counts ([`CountersKey`]).
type Counters = std::collections::BTreeMap<String, AutoLevelCounters>;

/// Stored counters, each that can still be read, and whether every one
/// could: one bad entry costs that one alone.
fn read_counters(raw: &[u8]) -> (Counters, bool) {
    let Ok(serde_json::Value::Object(stored)) = serde_json::from_slice(raw) else {
        return (Counters::new(), false);
    };
    let count = stored.len();
    let read: Counters = stored
        .into_iter()
        .filter_map(|(key, value)| Some((key, serde_json::from_value(value).ok()?)))
        .collect();
    let whole = read.len() == count;
    (read, whole)
}

/// Write `name` in `dir` whole or not at all.
///
/// The bytes go to a temporary file beside it, are flushed to the disk, and
/// only then take the name, so a crash, a power cut or a full disk leaves
/// either the old file or the new one — never a half-written one, and never
/// an empty one whose rename reached the disk before its data. The temporary
/// name is this process's own, so two copies of the app running at once
/// cannot write into each other's.
pub(super) fn write_whole(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    place_whole(dir, name, bytes)?;
    sync_dir(dir)
}

/// Write `name` in `dir` whole or not at all, as [`write_whole`] does, but
/// leave the folder's own record of the name to the caller to make durable
/// ([`sync_dir`]): once for many files written together.
pub(super) fn place_whole(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let tmp = temporary(dir, name);
    let written = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, &path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Where `name` in `dir` is written before it takes its name: a name of
/// this process's own, and of this write's, so two copies of the app
/// running at once never write into each other's.
fn temporary(dir: &Path, name: &str) -> PathBuf {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Save a file in `folder` as `name`, or as the first name free after it
/// ([`free_name`]), written a piece at a time by `write`, and say what it
/// is called, and what `write` said of it.
///
/// It is written as [`write_whole`] writes a file, into a file of its own
/// beside it, made durable, and only then named: a file cut short, by a
/// full disk, the app stopping or `write` giving up, leaves none that is
/// half of one, and nothing is ever written over. However large it is, no
/// more of it than a buffer's worth is held at a time.
pub(super) fn save_streamed<T, E: From<io::Error>>(
    folder: &Path,
    name: &str,
    write: impl FnOnce(&mut dyn Write) -> Result<T, E>,
) -> Result<(String, T), E> {
    fs::create_dir_all(folder)?;
    let name = free_name(folder, name);
    let tmp = temporary(folder, &name);
    let written = (|| -> Result<T, E> {
        let mut out = io::BufWriter::new(fs::File::create(&tmp)?);
        let said = write(&mut out)?;
        out.into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .sync_all()?;
        fs::rename(&tmp, folder.join(&name))?;
        Ok(said)
    })();
    match written {
        Ok(said) => {
            sync_dir(folder)?;
            Ok((name, said))
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// Where backup files go. On a computer: Downloads, or failing that
/// Documents, or home. On an iPhone: the app's Documents, which its
/// Info.plist shows in the Files app (Dioxus.toml). Android has a folder of
/// its own only through the media store ([`super::android`]).
fn backup_folder() -> Option<PathBuf> {
    if cfg!(target_os = "android") {
        return None;
    }
    if cfg!(target_os = "ios") {
        return dirs::document_dir();
    }
    dirs::download_dir()
        .or_else(dirs::document_dir)
        .or_else(dirs::home_dir)
}

/// How the user moves a backup file out of the app, where this build saves
/// it inside the app ([`Store::backups_inside_app`]): with the Files app on
/// an iPhone, a file manager on Android before 10.
fn backups_inside_app() -> Option<&'static str> {
    if cfg!(target_os = "ios") {
        return Some("with the Files app, to iCloud Drive or another app,");
    }
    #[cfg(all(target_os = "android", not(test)))]
    if super::android::backups_inside_app() {
        return Some("with a file manager, to Download say,");
    }
    None
}

/// `name` in `folder`, or, if a file is already called that, the first of
/// `name-2`, `name-3` and so on that is free: an earlier backup is never
/// written over.
pub(super) fn free_name(folder: &Path, name: &str) -> String {
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    std::iter::once(name.to_string())
        .chain((2..1_000).map(|n| {
            if extension.is_empty() {
                format!("{stem}-{n}")
            } else {
                format!("{stem}-{n}.{extension}")
            }
        }))
        .find(|candidate| !folder.join(candidate).exists())
        .unwrap_or_else(|| name.to_string())
}

/// Make a rename in `dir` as durable as the data it names.
#[cfg(unix)]
pub(super) fn sync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

/// Windows cannot open a directory to flush it; its renames are journalled.
#[cfg(not(unix))]
pub(super) fn sync_dir(_: &Path) -> io::Result<()> {
    Ok(())
}

impl Store for DesktopStore {
    fn load_theme(&self) -> String {
        fs::read_to_string(self.root.join(THEME))
            .map(|raw| raw.trim().to_string())
            .unwrap_or_default()
    }

    fn save_theme(&self, theme: &str) {
        let _ = self.write(THEME, theme.as_bytes());
    }

    fn tutorial_done(&self) -> bool {
        self.root.join(TUTORIAL).exists()
    }

    fn mark_tutorial_done(&self) {
        let _ = self.write(TUTORIAL, b"done");
    }

    fn told_of_paddle_keys(&self) -> bool {
        self.root.join(TOLD_OF_PADDLE_KEYS).exists()
    }

    fn mark_told_of_paddle_keys(&self) {
        let _ = self.write(TOLD_OF_PADDLE_KEYS, b"told");
    }

    fn asked_about_lock(&self) -> bool {
        self.root.join(ASKED_ABOUT_LOCK).exists()
    }

    fn mark_asked_about_lock(&self) {
        let _ = self.write(ASKED_ABOUT_LOCK, b"asked");
    }

    /// A settings file that does not read at all, edited by hand and left
    /// with a stray comma say, is set aside before the defaults the app goes
    /// on with are written over it; until it is, nothing is. It used to read
    /// as no settings at all, and the first change made went over it.
    fn read_settings(&self) -> Option<TrainingSettings> {
        self.settings_held.set(false);
        let path = self.root.join(SETTINGS);
        let raw = match fs::read(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
            Err(err) => {
                eprintln!("could not read the settings: {err}");
                self.settings_held.set(true);
                return None;
            }
        };
        let settings = std::str::from_utf8(&raw).ok().and_then(read_settings_from);
        if settings.is_none() {
            match self.set_aside(&path, "settings") {
                Some(name) => self.set_aside.borrow_mut().push(SetAside::Settings {
                    kept: self.shown(&name),
                }),
                None => self.settings_held.set(true),
            }
        }
        settings
    }

    fn write_settings(&self, settings: &TrainingSettings) -> Result<(), String> {
        if self.settings_held.get() {
            return Err(SETTINGS_HELD.into());
        }
        // Pretty, as the one file somebody might open and edit by hand.
        let raw = serde_json::to_vec_pretty(settings).map_err(|err| err.to_string())?;
        self.write(SETTINGS, &raw).map_err(|err| err.to_string())
    }

    fn take_set_aside(&self) -> Vec<SetAside> {
        std::mem::take(&mut self.set_aside.borrow_mut())
    }

    fn load_auto_counters(&self, key: &CountersKey) -> AutoLevelCounters {
        self.load_all_counters()
            .get(key.as_str())
            .copied()
            .unwrap_or_default()
    }

    fn save_auto_counters(&self, key: &CountersKey, counters: AutoLevelCounters) {
        self.update_counters(|all| {
            all.insert(key.as_str().to_string(), counters);
        });
    }

    fn clear_auto_counters(&self, keys: &[CountersKey]) {
        self.update_counters(|all| {
            for key in keys {
                all.remove(key.as_str());
            }
        });
    }

    fn last_backup(&self) -> Option<u64> {
        fs::read_to_string(self.root.join(LAST_BACKUP))
            .ok()
            .and_then(|raw| raw.trim().parse().ok())
    }

    fn mark_backup(&self, at: u64) {
        let _ = self.write(LAST_BACKUP, at.to_string().as_bytes());
    }

    fn saves_backups(&self) -> bool {
        cfg!(all(target_os = "android", not(test))) || self.backup_folder.is_some()
    }

    fn backups_inside_app(&self) -> Option<&'static str> {
        self.backups_inside
    }

    fn outgrows_device_backup(&self) -> bool {
        outgrows_device_backup(self.history.bytes(), cfg!(target_os = "android"))
    }

    /// Written on the history's own thread, a session at a time as each
    /// is read from its file: however long the history, it is never held
    /// whole, here or on the way to Android's Downloads. Where the files
    /// cannot all be read, nothing is saved, and the backup is wanted again
    /// holding what the app does ([`NotSaved::Unread`]).
    fn save_backup(&self, name: &str, backup: BackupFile) -> BackupSaving {
        let name = name.to_string();
        #[cfg(all(target_os = "android", not(test)))]
        let saving = self.history.ask(move |files| {
            super::android::save_backup(&name, |out| files.stream_backup(&backup, out))
        });
        #[cfg(not(all(target_os = "android", not(test))))]
        let saving = {
            let Some(folder) = self.backup_folder.clone() else {
                return Box::pin(std::future::ready(Err(NotSaved::Failed(
                    "there is no folder to put it in".into(),
                ))));
            };
            self.history.ask(move |files| {
                save_in_folder(&folder, &name, |out| files.stream_backup(&backup, out))
            })
        };
        Box::pin(async move {
            saving
                .await
                .unwrap_or_else(|| {
                    Err(NotSaved::Failed(
                        "the history's files could not be reached".into(),
                    ))
                })
                .map(|(place, backed)| super::saved_as(&place, backed))
        })
    }

    /// Written unsynced: it is only a mark, and no group waits on it. Each
    /// write is whole, so a copy killed as it writes what its session has
    /// done so far leaves the mark as it was before or after, never half of
    /// one; one a power cut leaves empty still tells of a session cut off
    /// ([`CutOff::Unread`]).
    fn mark_open(&self, open: Option<OpenSession>) {
        let mut held = self.open_held.borrow_mut();
        match open {
            Some(open) => {
                let (mark, new) = match held.take() {
                    Some(mark) => (mark, false),
                    None => match self.new_mark() {
                        Some(mark) => (mark, true),
                        None => return,
                    },
                };
                let written = serde_json::to_vec(&open)
                    .map_err(io::Error::other)
                    .and_then(|raw| self.write_mark(&mark.id, &raw));
                // A mark that will not take what its session has done since
                // keeps what it held.
                if written.is_ok() || !new {
                    *held = Some(mark);
                } else {
                    self.unmark(mark);
                }
            }
            None => {
                if let Some(mark) = held.take() {
                    self.unmark(mark);
                }
            }
        }
    }

    fn take_cut_off(&self) -> CutOffTaking {
        Box::pin(std::future::ready(self.cut_off()))
    }

    /// The history read as the app started, once it is; or read now.
    fn load_history(&self) -> HistoryLoad {
        let started = self.loading.borrow_mut().take();
        match started {
            Some(loading) => loading.wait(),
            None => self.history.load(),
        }
    }

    /// Made on the history's own thread, after whatever was asked before.
    fn change_history(&self, change: HistoryChange) -> Pending<HistorySaved> {
        self.history.apply(change)
    }

    fn history_listing(&self) -> Pending<Option<Vec<RecordId>>> {
        self.history.listing()
    }

    fn read_records(&self, ids: Vec<RecordId>) -> Pending<Vec<Result<Record, RecordId>>> {
        self.history.read(ids)
    }

    fn forgotten(&self, ids: Vec<RecordId>) -> Pending<Vec<RecordId>> {
        self.history.forgotten(ids)
    }

    fn history_changed(&self) -> bool {
        self.history.changed()
    }

    fn flush_history(&self) {
        if !self.history.flush(FLUSH_WITHIN) {
            eprintln!("the app closed before the history was all written");
        }
    }
}

impl DesktopStore {
    fn mark_path(&self, id: &str, extension: &str) -> PathBuf {
        self.root.join(OPEN).join(format!("{id}.{extension}"))
    }

    /// A mark of this copy's own, its lock held.
    ///
    /// The lock file is made, and locked, under a name no look reads, and
    /// only then put where looks find it. Made there and locked after, a
    /// copy looking for marks in between found it unheld, took it for one
    /// cut off, and deleted it from under this copy, whose session then had
    /// no mark at all.
    fn new_mark(&self) -> Option<HeldMark> {
        fs::create_dir_all(self.root.join(OPEN)).ok()?;
        let id = format!("{:016x}", crate::time::seed_rng());
        let making = self.mark_path(&id, "making");
        let lock = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&making)
            .ok()?;
        // A rename keeps the file, and the lock held on it.
        let placed =
            lock.try_lock().is_ok() && fs::rename(&making, self.mark_path(&id, "lock")).is_ok();
        if !placed {
            let _ = fs::remove_file(&making);
            return None;
        }
        Some(HeldMark { id, lock })
    }

    /// Write what mark `id` says, whole: under a name no look reads, then
    /// renamed into place over what it said before.
    fn write_mark(&self, id: &str, raw: &[u8]) -> io::Result<()> {
        let writing = self.mark_path(id, "writing");
        fs::write(&writing, raw)?;
        fs::rename(&writing, self.mark_path(id, "json"))
    }

    /// The mark gone, and its lock let go once it is.
    fn unmark(&self, mark: HeldMark) {
        let _ = fs::remove_file(self.mark_path(&mark.id, "writing"));
        let _ = fs::remove_file(self.mark_path(&mark.id, "json"));
        let _ = fs::remove_file(self.mark_path(&mark.id, "lock"));
        drop(mark.lock);
    }

    /// Every mark no copy holds, taken: what each tells of is read under its
    /// lock, so a copy that has just marked its session is never taken for
    /// one cut off.
    fn cut_off(&self) -> Vec<CutOff> {
        let mine = self.open_held.borrow().as_ref().map(|mark| mark.id.clone());
        let Ok(marks) = fs::read_dir(self.root.join(OPEN)) else {
            return Vec::new();
        };
        marks
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let id = path.file_stem()?.to_str()?.to_string();
                if path.extension()? != "lock" || mine.as_ref() == Some(&id) {
                    return None;
                }
                let lock = fs::OpenOptions::new().write(true).open(&path).ok()?;
                // Held: another copy is running it.
                lock.try_lock().ok()?;
                let mark = HeldMark { id, lock };
                let read = fs::read(self.mark_path(&mark.id, "json"));
                self.unmark(mark);
                read.ok().map(|raw| CutOff::read(&raw))
            })
            .collect()
    }
}

/// Save a backup file in `folder`, written by `write` ([`save_streamed`]),
/// and say where it went, in words for the user, and what it holds.
#[cfg_attr(all(target_os = "android", not(test)), allow(dead_code))]
fn save_in_folder(
    folder: &Path,
    name: &str,
    write: impl FnOnce(&mut dyn Write) -> Result<BackedUp, NotSaved>,
) -> Result<(String, BackedUp), NotSaved> {
    let (name, backed) = save_streamed(folder, name, write)?;
    let place = if cfg!(target_os = "ios") {
        format!("{name}, in the Files app, in Dust's folder")
    } else {
        folder.join(name).display().to_string()
    };
    Ok((place, backed))
}

/// The package an Android process runs: the first entry of
/// `/proc/self/cmdline`.
#[cfg(any(target_os = "android", test))]
fn package_from_cmdline(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let name = text.split('\0').next()?.trim();
    // Some processes are named `<package>:<process>`; the data dir is the package.
    let name = name.split(':').next()?;
    let valid = !name.is_empty()
        && name.contains('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
    valid.then(|| name.to_string())
}

/// The Android user a process runs as, from its `/proc/self/status`: its
/// uid over the 100 000 uids each user has.
#[cfg(any(target_os = "android", test))]
fn android_user_from_status(status: &str) -> Option<u32> {
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse::<u32>()
        .ok()?;
    Some(uid / 100_000)
}

/// The folder Android gives `package` for its own files, as `user`, where
/// it cannot be asked of Android itself.
#[cfg(any(target_os = "android", test))]
fn android_files_dir_of(user: u32, package: &str) -> PathBuf {
    PathBuf::from(format!("/data/user/{user}/{package}/files"))
}

/// Android hands an app no `HOME`, so `dirs` has nothing to work from. The
/// store goes in the app's own files folder, which Android names for the
/// user the app runs as: the phone's owner, a work profile, another user,
/// Private Space and a cloned app each have their own, and only the
/// owner's is under `/data/data`. It is asked of Android itself
/// ([`super::android::files_dir`]); failing that, it is worked out from the
/// process, the package from `/proc/self/cmdline` and the user from its uid.
#[cfg(target_os = "android")]
fn android_data_dir() -> Option<PathBuf> {
    #[cfg(not(test))]
    let asked = super::android::files_dir();
    #[cfg(test)]
    let asked = None;
    let files = asked.or_else(|| {
        let package = package_from_cmdline(&fs::read("/proc/self/cmdline").ok()?)?;
        let user = android_user_from_status(&fs::read_to_string("/proc/self/status").ok()?)?;
        Some(android_files_dir_of(user, &package))
    })?;
    let dir = files.join("dust");
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Where a desktop build keeps its files. `DUST_DATA_DIR` overrides it, which
/// is what a portable install or a test run uses.
fn data_dir() -> PathBuf {
    data_dir_from(std::env::var_os("DUST_DATA_DIR"))
}

/// The same, with the override passed in rather than read from the process
/// environment — which a test may not change while other threads run.
fn data_dir_from(overridden: Option<std::ffi::OsString>) -> PathBuf {
    if let Some(dir) = overridden.filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "android")]
    if let Some(dir) = android_data_dir() {
        return dir;
    }
    under_data_dir(dirs::data_dir())
}

/// `dust` in the platform's data folder `data`. With none, it goes in the
/// temporary folder, a whole path of its own, rather than in the folder the
/// app happened to start in, which on a phone is the root, `/`, where
/// nothing can be written.
fn under_data_dir(data: Option<PathBuf>) -> PathBuf {
    data.unwrap_or_else(std::env::temp_dir).join("dust")
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::persist::Deleted;
    use crate::persist::backup::{read_backup, read_backup_as_0_16_2, write_backup};
    use crate::persist::record::{RecordId, records_for_blob};
    use crate::persist::tests::{every_setting_moved, session};
    use crate::persist::worker::wait;
    use cw_core::SessionResult;

    /// A scratch directory that cleans up after itself.
    pub(crate) struct TempDir(pub(crate) PathBuf);

    impl TempDir {
        pub(crate) fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("dust-test-{}-{name}-{unique}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }

        /// Whether a folder's permissions, taken away, hold whoever runs
        /// the tests to them: they hold no one running as root, who may
        /// write wherever they like, so a test of a folder that will not
        /// take a change has nothing to try there.
        #[cfg(unix)]
        pub(crate) fn permissions_hold() -> bool {
            use std::os::unix::fs::PermissionsExt;
            let probe = Self::new("permissions");
            fs::create_dir_all(&probe.0).unwrap();
            fs::set_permissions(&probe.0, fs::Permissions::from_mode(0o555)).unwrap();
            let held = fs::write(probe.0.join("probe"), "").is_err();
            fs::set_permissions(&probe.0, fs::Permissions::from_mode(0o755)).unwrap();
            held
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // A test that took a folder's permissions away, and failed
            // before it gave them back, still leaves nothing behind.
            #[cfg(unix)]
            given_back(&self.0);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Give back the permissions of `dir`, and of every folder in it.
    #[cfg(unix)]
    fn given_back(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if fs::symlink_metadata(dir).is_ok_and(|meta| meta.is_dir()) {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                given_back(&entry.path());
            }
        }
    }

    fn store(name: &str) -> (TempDir, DesktopStore) {
        let dir = TempDir::new(name);
        let store = DesktopStore::at(dir.0.clone());
        (dir, store)
    }

    /// The files in the store's directory, by name.
    fn files(dir: &TempDir) -> Vec<String> {
        names(&dir.0)
    }

    /// The files in `dir`, by name.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn open_at(started_at: u64) -> OpenSession {
        OpenSession::starting(false, started_at)
    }

    /// What a change came to: the files make every change then and there.
    fn now<T>(mut pending: Pending<T>) -> T {
        let mut asked = std::task::Context::from_waker(std::task::Waker::noop());
        match pending.as_mut().poll(&mut asked) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => panic!("not done"),
        }
    }

    /// Store `sessions`, oldest first, each a record, as the app stores
    /// them.
    fn store_sessions(store: &DesktopStore, sessions: &[SessionResult]) -> HistorySaved {
        now(store.change_history(HistoryChange {
            put: records_for_blob(sessions),
            ..HistoryChange::default()
        }))
    }

    /// Every session the store holds, in the order of the history.
    fn stored(store: &DesktopStore) -> Vec<SessionResult> {
        store
            .load_history()
            .sessions
            .into_iter()
            .map(|(_, session)| session)
            .collect()
    }

    /// Each copy of the app marks its own session. One a copy is still
    /// running is left to it, one whose copy stopped is told of once, and
    /// one that ended is never told of.
    #[test]
    fn a_session_is_told_of_once_and_only_when_its_copy_stopped_under_it() {
        let (dir, first) = store("open");
        let second = DesktopStore::at(dir.0.clone());
        first.mark_open(Some(open_at(1)));
        second.mark_open(Some(open_at(2)));
        assert_eq!(first.cut_off(), [], "the other is still running");
        assert_eq!(second.cut_off(), []);

        // The first ends its own, and leaves the second's be.
        first.mark_open(None);
        assert_eq!(first.cut_off(), []);

        // The second stops under its session: a copy open all along is told,
        // once, and so would the next to open have been.
        drop(second);
        let third = DesktopStore::at(dir.0.clone());
        assert_eq!(first.cut_off(), [CutOff::Marked(open_at(2))]);
        assert_eq!(third.cut_off(), [], "told of once");
        let left = fs::read_dir(dir.0.join(OPEN)).unwrap().count();
        assert_eq!(left, 0, "nothing left behind");
    }

    /// A mark's lock file is where looks find it only once its lock is
    /// held, and nothing it was made under is left behind.
    #[test]
    fn a_mark_is_found_only_with_its_lock_held() {
        let (dir, store) = store("open-held");
        store.mark_open(Some(open_at(1)));
        let id = store
            .open_held
            .borrow()
            .as_ref()
            .map(|mark| mark.id.clone());
        let id = id.expect("marked");
        let mut left: Vec<String> = fs::read_dir(dir.0.join(OPEN))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, [format!("{id}.json"), format!("{id}.lock")]);
        let lock = fs::OpenOptions::new()
            .write(true)
            .open(store.mark_path(&id, "lock"))
            .unwrap();
        assert!(
            lock.try_lock().is_err(),
            "the lock was let go of on the way"
        );
        assert_eq!(DesktopStore::at(dir.0.clone()).cut_off(), []);
    }

    /// What a session has done so far goes into its mark, again and again
    /// as it goes, each time whole and with nothing left beside it; a copy
    /// that stops under the session leaves it for the next, which reads it
    /// back as it was last written.
    #[test]
    fn a_mark_keeps_what_its_session_did_so_far() {
        let (dir, cut) = store("open-so-far");
        cut.mark_open(Some(open_at(1)));
        for groups in 1..=3 {
            let mut result = SessionResult::empty_for_test();
            result.groups = vec![
                cw_core::GroupResult {
                    sent: "KM".into(),
                    received: "KM".into(),
                    correct: true,
                };
                groups
            ];
            let so_far = crate::persist::SoFar {
                result,
                settings: Some(TrainingSettings::default()),
            };
            cut.mark_open(Some(OpenSession {
                so_far: Some(Box::new(so_far)),
                ..open_at(1)
            }));
        }
        let id = cut.open_held.borrow().as_ref().map(|mark| mark.id.clone());
        let id = id.expect("marked");
        let mut left: Vec<String> = fs::read_dir(dir.0.join(OPEN))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, [format!("{id}.json"), format!("{id}.lock")]);
        drop(cut);
        let cut = DesktopStore::at(dir.0.clone()).cut_off();
        let so_far = cut[..].first().and_then(CutOff::so_far).expect("kept");
        assert_eq!(so_far.result.groups.len(), 3);
        assert_eq!(so_far.settings, Some(TrainingSettings::default()));
    }

    /// A mark a power cut left empty still says a session was cut off.
    #[test]
    fn an_unreadable_mark_still_tells_of_a_session_cut_off() {
        let (dir, cut) = store("open-unread");
        cut.mark_open(Some(open_at(1)));
        let id = cut.open_held.borrow().as_ref().map(|mark| mark.id.clone());
        fs::write(cut.mark_path(&id.expect("marked"), "json"), b"").unwrap();
        drop(cut);
        let next = DesktopStore::at(dir.0.clone());
        assert_eq!(next.cut_off(), [CutOff::Unread]);
    }

    #[test]
    fn a_fresh_install_reads_back_defaults() {
        let (_dir, store) = store("fresh");
        assert_eq!(store.load_theme(), "");
        assert!(!store.tutorial_done());
        assert!(!store.asked_about_lock());
        assert_eq!(store.load_settings(), TrainingSettings::default().clamp());
        assert!(stored(&store).is_empty());
        assert_eq!(
            store.load_auto_counters(&CountersKey::of(&TrainingSettings::default())),
            AutoLevelCounters::default()
        );
    }

    #[test]
    fn everything_written_comes_back() {
        let (dir, store) = store("roundtrip");
        store.save_theme("dark");
        assert_eq!(store.load_theme(), "dark");
        store.mark_tutorial_done();
        assert!(store.tutorial_done());
        store.mark_asked_about_lock();
        assert!(store.asked_about_lock());
        assert!(
            DesktopStore::at(dir.0.clone()).asked_about_lock(),
            "once an install"
        );

        let mut settings = TrainingSettings::default();
        settings.curriculum.num_groups = 7;
        settings.playback.char_wpm_min = 24.0;
        settings.playback.char_wpm_max = 24.0;
        store.save_settings(&settings);
        assert_eq!(store.load_settings().curriculum.num_groups, 7);
        assert_eq!(store.load_settings().playback.char_wpm_min, 24.0);

        assert_eq!(
            store_sessions(&store, &[session("2026-09-01"), session("2026-09-02")]),
            HistorySaved::Stored { kept: 2 }
        );
        let read = stored(&DesktopStore::at(dir.0.clone()));
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].date, "2026-09-02");

        let key = CountersKey::of(&settings);
        let counters = AutoLevelCounters { above: 3, below: 1 };
        store.save_auto_counters(&key, counters);
        assert_eq!(store.load_auto_counters(&key), counters);

        // Counters are kept per level, not shared.
        let mut other = settings.clone();
        other.curriculum.level = settings.curriculum.level + 1;
        assert_eq!(
            store.load_auto_counters(&CountersKey::of(&other)),
            AutoLevelCounters::default()
        );

        store.clear_auto_counters(std::slice::from_ref(&key));
        assert_eq!(store.load_auto_counters(&key), AutoLevelCounters::default());
        // Clearing a key that was never written is harmless.
        let mut never = settings;
        never.curriculum.level += 7;
        store.clear_auto_counters(&[CountersKey::of(&never)]);

        // Every write took its name whole, and left nothing behind.
        assert_eq!(
            files(&dir),
            [
                "asked_about_lock.txt",
                "auto_adjust.json",
                "history",
                "settings.json",
                "theme.txt",
                "tutorial_done.txt"
            ]
        );
    }

    /// Every setting written to the file comes back from it, by a store
    /// opened afresh on the same folder, as the next launch opens it.
    #[test]
    fn every_setting_comes_back_on_the_next_launch() {
        let (dir, store) = store("every-setting");
        let moved = every_setting_moved();
        store.save_settings(&moved);
        assert_eq!(DesktopStore::at(dir.0.clone()).load_settings(), moved);
    }

    /// A settings file with the AGC threshold in it comes back with the RF
    /// gain backed off as far, and stays so once it is written again.
    #[test]
    fn a_settings_file_with_an_agc_threshold_is_upgraded_once() {
        let (dir, store) = store("agc-threshold");
        fs::create_dir_all(&dir.0).unwrap();
        let old = r#"{"agcThresholdDb":6,"rfGainDb":-4,"numGroups":7}"#;
        fs::write(dir.0.join(SETTINGS), old).unwrap();
        let read = store.read_settings().expect("settings");
        assert_eq!(read.band.rf_gain_db, -10.0);
        assert_eq!(read.curriculum.num_groups, 7);
        store.save_settings(&read);
        assert_eq!(store.read_settings(), Some(read));
    }

    /// A settings file that does not read at all, edited by hand and left
    /// with a stray comma, is set aside before anything is written over it,
    /// and the app is told where, once. It used to read as no settings at
    /// all, and the first change made went over it.
    #[test]
    fn a_settings_file_that_does_not_read_is_set_aside_before_a_write() {
        let (dir, store) = store("settings-unreadable");
        fs::create_dir_all(&dir.0).unwrap();
        let edited = "{\n  \"level\": 12,\n}";
        fs::write(dir.0.join(SETTINGS), edited).unwrap();
        assert_eq!(store.read_settings(), None);
        let told = store.take_set_aside();
        let [SetAside::Settings { kept: Some(kept) }] = told.as_slice() else {
            panic!("{told:?}");
        };
        assert!(kept.contains("settings.unreadable-"), "{kept}");
        assert_eq!(fs::read_to_string(kept).unwrap(), edited);
        assert_eq!(store.read_settings(), None, "a first launch now");
        assert!(store.take_set_aside().is_empty(), "told of once");
        store.save_settings(&every_setting_moved());
        assert_eq!(store.load_settings(), every_setting_moved());
        assert_eq!(fs::read_to_string(kept).unwrap(), edited, "kept as it was");
    }

    /// Settings that cannot be read at all are never written over.
    #[test]
    fn settings_that_cannot_be_read_are_never_written_over() {
        let (dir, store) = store("settings-held");
        fs::create_dir_all(dir.0.join(SETTINGS)).unwrap();
        assert_eq!(store.read_settings(), None);
        assert!(store.write_settings(&every_setting_moved()).is_err());
        assert!(dir.0.join(SETTINGS).is_dir());
    }

    /// Settings that cannot be written say why, and leave what is there.
    #[test]
    fn settings_that_cannot_be_written_say_so() {
        let (dir, store) = store("settings-unwritable");
        fs::create_dir_all(dir.0.join(SETTINGS)).unwrap();
        assert!(store.write_settings(&every_setting_moved()).is_err());
        assert!(dir.0.join(SETTINGS).is_dir());
        assert_eq!(files(&dir), [SETTINGS], "no scratch file left behind");
    }

    /// The history is what grows: each session is written compact, about
    /// half the size it would be pretty-printed.
    #[test]
    fn the_history_is_written_compact() {
        let (dir, store) = store("compact");
        store_sessions(&store, &[session("2026-09-01")]);
        let records = dir.0.join("history").join("sessions");
        let [name] = names(&records).try_into().expect("one record");
        let raw = fs::read_to_string(records.join(name)).unwrap();
        assert!(!raw.contains('\n'), "{raw}");
    }

    /// No session is ever let go: two thousand stored, two thousand read,
    /// by a store opened afresh as the next launch opens it, oldest first.
    /// The history used to keep only the newest five hundred.
    #[test]
    fn no_session_is_ever_let_go() {
        let (dir, store) = store("no-cap");
        let all: Vec<SessionResult> = (1..=2_000u64)
            .map(|n| SessionResult {
                timestamp: n * 1_000,
                ..session(&format!("day {n}"))
            })
            .collect();
        assert_eq!(
            store_sessions(&store, &all),
            HistorySaved::Stored { kept: 2_000 }
        );
        let read = stored(&DesktopStore::at(dir.0.clone()));
        assert_eq!(read.len(), 2_000);
        assert_eq!(read.first().map(|s| s.timestamp), Some(1_000));
        assert_eq!(read.last().map(|s| s.timestamp), Some(2_000_000));
    }

    #[test]
    fn a_damaged_file_does_not_take_the_rest_with_it() {
        let (dir, store) = store("damaged");
        store.save_theme("light");
        fs::write(dir.0.join(SESSIONS), "{ not json").unwrap();
        fs::write(dir.0.join(SETTINGS), "{ not json").unwrap();
        fs::write(dir.0.join(COUNTERS), "{ not json").unwrap();
        assert!(stored(&store).is_empty());
        assert_eq!(store.load_settings(), TrainingSettings::default().clamp());
        assert_eq!(
            store.load_auto_counters(&CountersKey::of(&TrainingSettings::default())),
            AutoLevelCounters::default()
        );
        assert_eq!(store.load_theme(), "light");
    }

    /// Counters for each of `levels`, the settings they count, and what
    /// they hold.
    fn counted_at(levels: impl IntoIterator<Item = u32>) -> Vec<(CountersKey, AutoLevelCounters)> {
        levels
            .into_iter()
            .map(|level| {
                let mut settings = TrainingSettings::default();
                settings.curriculum.level = level;
                let counters = AutoLevelCounters {
                    above: level,
                    below: 1,
                };
                (CountersKey::of(&settings), counters)
            })
            .collect()
    }

    /// The counters set aside, by what each file holds.
    fn counters_set_aside(dir: &TempDir) -> Vec<String> {
        files(dir)
            .into_iter()
            .filter(|name| name.starts_with("auto_adjust.unreadable-"))
            .map(|name| fs::read_to_string(dir.0.join(name)).unwrap())
            .collect()
    }

    /// A counters file that cannot be read is set aside before the next
    /// count is written, not written over by a file holding that count
    /// alone; one unreadable entry costs that one alone.
    #[test]
    fn unreadable_counters_are_set_aside_rather_than_written_over() {
        let (dir, store) = store("counters-unreadable");
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.0.join(COUNTERS), "{ not json").unwrap();
        let counted = counted_at([3, 4, 5]);
        store.save_auto_counters(&counted[0].0, counted[0].1);
        assert_eq!(store.load_auto_counters(&counted[0].0), counted[0].1);
        assert_eq!(counters_set_aside(&dir), ["{ not json"]);

        let damaged = format!(
            r#"{{"{}":{{"above":4,"below":1}},"{}":"nonsense"}}"#,
            counted[1].0.as_str(),
            counted[0].0.as_str()
        );
        fs::write(dir.0.join(COUNTERS), &damaged).unwrap();
        store.save_auto_counters(&counted[2].0, counted[2].1);
        assert_eq!(store.load_auto_counters(&counted[1].0), counted[1].1);
        assert_eq!(store.load_auto_counters(&counted[2].0), counted[2].1);
        assert_eq!(
            store.load_auto_counters(&counted[0].0),
            AutoLevelCounters::default()
        );
        let mut kept = counters_set_aside(&dir);
        kept.sort();
        assert_eq!(kept, ["{ not json".to_string(), damaged]);
    }

    /// Two copies of the app counting at once each keep the other's
    /// counts: every count is read afresh just before it is written.
    #[test]
    fn counters_written_by_another_copy_are_kept() {
        let (dir, first) = store("counters-shared");
        let second = DesktopStore::at(dir.0.clone());
        let counted = counted_at([3, 4]);
        first.save_auto_counters(&counted[0].0, counted[0].1);
        second.save_auto_counters(&counted[1].0, counted[1].1);
        assert_eq!(first.load_auto_counters(&counted[0].0), counted[0].1);
        assert_eq!(first.load_auto_counters(&counted[1].0), counted[1].1);
        second.clear_auto_counters(std::slice::from_ref(&counted[0].0));
        assert_eq!(
            first.load_auto_counters(&counted[0].0),
            AutoLevelCounters::default()
        );
        assert_eq!(first.load_auto_counters(&counted[1].0), counted[1].1);
    }

    /// What a later build added to a counter, or to a stored session,
    /// stays when this build writes the file again. Each was written back
    /// as this build reads it, without whatever else it held.
    #[test]
    fn what_a_later_build_added_stays_through_a_write() {
        let (dir, store) = store("later-keys");
        fs::create_dir_all(&dir.0).unwrap();
        let counted = counted_at([3, 4]);
        let held = format!(
            r#"{{"{}":{{"above":1,"below":0,"laterKey":[1]}}}}"#,
            counted[0].0.as_str()
        );
        fs::write(dir.0.join(COUNTERS), held).unwrap();
        store.save_auto_counters(&counted[0].0, counted[0].1);
        store.save_auto_counters(&counted[1].0, counted[1].1);
        let held: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.0.join(COUNTERS)).unwrap()).unwrap();
        assert_eq!(
            held[counted[0].0.as_str()]["laterKey"],
            serde_json::json!([1])
        );
        assert_eq!(store.load_auto_counters(&counted[0].0), counted[0].1);

        let mut later = serde_json::to_value(session("2026-09-01")).unwrap();
        later["laterKey"] = serde_json::json!("kept");
        let whole = format!("[{later}]");
        fs::write(dir.0.join(SESSIONS), &whole).unwrap();
        let (moved, read) = store.load_history().sessions.remove(0);
        let next = SessionResult {
            timestamp: read.timestamp + 1,
            ..session("2026-09-02")
        };
        let record = Record::of(&next, |json| {
            RecordId::next(next.timestamp, Some(moved), json)
        })
        .expect("a record");
        now(store.change_history(HistoryChange {
            put: vec![record],
            ..HistoryChange::default()
        }));
        let read: [Result<Record, RecordId>; 1] = now(store.read_records(vec![moved]))
            .try_into()
            .expect("one");
        let [Ok(held)] = read else {
            panic!("the record moved");
        };
        let held: serde_json::Value = serde_json::from_str(&held.json).unwrap();
        assert_eq!(held["laterKey"], "kept");
        assert_eq!(stored(&store).len(), 2);
        assert_eq!(fs::read_to_string(dir.0.join(SESSIONS)).unwrap(), whole);
    }

    #[test]
    fn a_theme_file_with_stray_whitespace_still_reads() {
        let (dir, store) = store("theme");
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.0.join(THEME), "  dark\n").unwrap();
        assert_eq!(store.load_theme(), "dark");
    }

    /// Without touching the process environment: tests run on many threads,
    /// and changing a variable while another thread — or a C library — reads
    /// the environment is undefined behaviour.
    #[test]
    fn the_data_directory_can_be_pointed_somewhere_else() {
        let dir = TempDir::new("datadir");
        assert_eq!(data_dir_from(Some(dir.0.clone().into_os_string())), dir.0);

        // An empty value is ignored rather than writing to the current folder.
        let fallback = data_dir_from(Some(std::ffi::OsString::new()));
        assert_ne!(fallback, PathBuf::new());
        assert!(fallback.ends_with("dust"));
        assert_eq!(fallback, data_dir_from(None));
        assert_eq!(DesktopStore::new().root, data_dir());
    }

    /// A history file that cannot be read at all is set aside, not
    /// overwritten, and told of once. It used to come back as an empty
    /// history, and the next finished session was saved over it — every
    /// session before it gone. Sessions are stored beside it all the same.
    #[test]
    fn an_unreadable_history_is_kept_rather_than_written_over() {
        let (dir, store) = store("unreadable");
        fs::create_dir_all(&dir.0).unwrap();
        let damaged = "{\"this\": \"is not a history\"";
        fs::write(dir.0.join(SESSIONS), damaged).unwrap();

        let read = store.load_history();
        assert!(read.sessions.is_empty());
        let [SetAside::History { kept: Some(told) }] = read.set_aside.as_slice() else {
            panic!("{:?}", read.set_aside);
        };
        assert_eq!(fs::read_to_string(told).unwrap(), damaged);
        assert_eq!(
            store_sessions(&store, &[session("2026-09-02")]),
            HistorySaved::Stored { kept: 1 }
        );
        let again = store.load_history();
        assert_eq!(again.sessions.len(), 1);
        assert!(again.set_aside.is_empty(), "told of once");
        assert_eq!(set_aside(&dir), [damaged]);
    }

    /// A history file that cannot be read for now, nor moved out of the
    /// way, is left alone, and says so: sessions are stored beside it, as
    /// records, and nothing goes over it.
    #[test]
    fn an_unreadable_history_that_cannot_be_moved_is_never_written_over() {
        let (dir, store) = store("stuck");
        fs::create_dir_all(dir.0.join(SESSIONS)).unwrap();
        // A directory where the history belongs: it does not read, and a
        // rename onto it cannot happen.
        let read = store.load_history();
        assert!(read.sessions.is_empty() && !read.complete);
        assert_eq!(
            store_sessions(&store, &[session("2026-09-02")]),
            HistorySaved::Stored { kept: 1 }
        );
        assert_eq!(stored(&store).len(), 1);
        assert!(
            dir.0.join(SESSIONS).is_dir(),
            "the history was written over"
        );
    }

    /// A session that cannot be written says why, leaves what is stored as
    /// it was, and no scratch file behind.
    #[test]
    fn a_session_that_cannot_be_written_leaves_the_rest_alone() {
        let (dir, store) = store("unwritable");
        store_sessions(&store, &[session("2026-09-01")]);
        let records = dir.0.join("history").join("sessions");
        let held = fs::read_dir(&records).unwrap().count();
        // Where the records go cannot take a file.
        let blocked = TempDir::new("unwritable-elsewhere");
        fs::create_dir_all(&blocked.0).unwrap();
        fs::rename(&records, blocked.0.join("sessions")).unwrap();
        fs::write(&records, "not a folder").unwrap();
        let later = SessionResult {
            timestamp: 9_000,
            ..session("2026-09-02")
        };
        assert!(matches!(
            store_sessions(&store, &[later]),
            HistorySaved::Failed(_)
        ));
        assert_eq!(fs::read_to_string(&records).unwrap(), "not a folder");
        fs::remove_file(&records).unwrap();
        fs::rename(blocked.0.join("sessions"), &records).unwrap();
        assert_eq!(fs::read_dir(&records).unwrap().count(), held);
        assert_eq!(stored(&store).len(), 1);
        let left = names(&dir.0.join("history"));
        assert!(left.iter().all(|name| !name.ends_with(".tmp")), "{left:?}");
    }

    /// What every file set aside from the history holds, oldest name first.
    fn set_aside(dir: &TempDir) -> Vec<String> {
        files(dir)
            .into_iter()
            .filter(|name| name.starts_with("sessions.unreadable-"))
            .map(|name| fs::read_to_string(dir.0.join(name)).unwrap())
            .collect()
    }

    /// A clock set behind the history moves nothing stored. Every session
    /// finished since reads as stamped ahead of it, and each keeps the
    /// stamp and the day it was stored with, however often it is read: a
    /// record is never written again. They used to be settled onto the day
    /// of the latest session the clock agreed with as they were read, and
    /// the next save wrote that over them for good.
    #[test]
    fn a_clock_set_behind_moves_no_stored_session() {
        let (dir, store) = store("behind");
        let now_ms = crate::time::now_ms();
        let at = |days_on: i64, date: &str| SessionResult {
            timestamp: now_ms.saturating_add_signed(days_on * 86_400_000),
            ..session(date)
        };
        // The clock was set a week back: the last week reads as ahead.
        let kept = vec![
            at(-10, "2026-10-20"),
            at(-9, "2026-10-21"),
            at(-8, "2026-10-22"),
            at(1, "2026-10-31"),
            at(3, "2026-11-02"),
        ];
        let stamped = |sessions: &[SessionResult]| -> Vec<(u64, String)> {
            sessions
                .iter()
                .map(|session| (session.timestamp, session.date.clone()))
                .collect()
        };
        assert_eq!(
            store_sessions(&store, &kept),
            HistorySaved::Stored { kept: 5 }
        );
        assert_eq!(stamped(&stored(&store)), stamped(&kept));
        // Another session stored, and the history read again as the next
        // launch reads it: the five are as they were.
        let next = SessionResult {
            timestamp: now_ms,
            ..session("2026-10-24")
        };
        let newest = store.load_history().sessions.last().map(|(id, _)| *id);
        let record = Record::of(&next, |json| RecordId::next(next.timestamp, newest, json))
            .expect("a record");
        now(store.change_history(HistoryChange {
            put: vec![record],
            ..HistoryChange::default()
        }));
        let reopened = stored(&DesktopStore::at(dir.0.clone()));
        assert_eq!(stamped(&reopened[..5]), stamped(&kept));
        assert_eq!(reopened.len(), 6, "the new one after them all");
    }

    /// A session deleted is a tombstone beside it, which a store opened
    /// afresh reads as deleted, and a store asked which were deleted names.
    #[test]
    fn the_sessions_deleted_are_remembered() {
        let (dir, store) = store("deleted");
        let kept = [session("2026-09-01")];
        store_sessions(&store, &kept);
        let id = records_for_blob(&kept)[0].id;
        let gone = HistoryChange {
            forget: vec![(id, Deleted::of(&kept[0], 7))],
            ..HistoryChange::default()
        };
        assert_eq!(
            now(store.change_history(gone)),
            HistorySaved::Stored { kept: 0 }
        );
        let another = DesktopStore::at(dir.0.clone());
        assert!(stored(&another).is_empty());
        assert_eq!(now(another.forgotten(vec![id])), [id]);
        let never = RecordId { order: 1, nonce: 1 };
        assert!(now(another.forgotten(vec![never])).is_empty());
    }

    /// A backup goes into the backup folder, never over an earlier one, and
    /// the store remembers when.
    #[test]
    fn a_backup_is_a_new_file_in_the_backup_folder() {
        let (dir, store) = store("backup");
        let folder = TempDir::new("backup-folder");
        let settings = every_setting_moved();
        let backup = || BackupFile::new(&settings, 7, Vec::new());
        assert!(!store.saves_backups(), "a store with no folder saves none");
        assert!(now(store.save_backup("b.json", backup())).is_err());
        let store = store.with_backup_folder(folder.0.clone());
        assert!(store.saves_backups());
        store_sessions(&store, &[session("2026-10-19")]);
        let first = now(store.save_backup("dust-backup-2026-10-19.json", backup()));
        let second = now(store.save_backup("dust-backup-2026-10-19.json", backup()));
        let saved = |name: &str| {
            Ok(format!(
                "Backup saved as {}.",
                folder.0.join(name).display()
            ))
        };
        assert_eq!(first, saved("dust-backup-2026-10-19.json"));
        assert_eq!(second, saved("dust-backup-2026-10-19-2.json"));
        assert_eq!(
            files(&folder),
            [
                "dust-backup-2026-10-19-2.json",
                "dust-backup-2026-10-19.json"
            ],
            "and nothing half written beside them"
        );
        let read =
            read_backup(&fs::read_to_string(folder.0.join("dust-backup-2026-10-19.json")).unwrap())
                .expect("a backup");
        assert_eq!(read.settings, Some(settings.clone()));
        assert_eq!(read.sessions.len(), 1);
        assert_eq!(read.saved_at, 7);

        assert_eq!(store.last_backup(), None);
        store.mark_backup(1_790_000_000_000);
        assert_eq!(store.last_backup(), Some(1_790_000_000_000));
        assert_eq!(files(&dir), ["history", LAST_BACKUP]);
    }

    /// A backup written a session at a time from the records, those the
    /// store has yet to take in their places among them, is byte for byte
    /// the backup written whole of the same history, which every version of
    /// Dust reads; Dust 0.16 reads it as this one does.
    #[test]
    fn a_streamed_backup_reads_back_as_the_whole_one() {
        let (_dir, store) = store("streamed");
        let folder = TempDir::new("streamed-folder");
        let store = store.with_backup_folder(folder.0.clone());
        let settings = every_setting_moved();
        let mut history: Vec<SessionResult> = (1..=5)
            .map(|day| SessionResult {
                timestamp: day * 86_400_000,
                ..session(&format!("2026-10-{day:02}"))
            })
            .collect();
        history[2].contest = Some(cw_core::ContestLog(
            serde_json::json!({ "later": [1, 2.5] }),
        ));
        let records = records_for_blob(&history);
        // The third, and the last, are here and not stored yet.
        now(store.change_history(HistoryChange {
            put: [0, 1, 3].map(|at| records[at].clone()).into(),
            ..HistoryChange::default()
        }));
        assert_eq!(stored(&store).len(), 3);
        let unsaved = vec![records[4].clone(), records[2].clone()];
        let at = 1_790_000_000_000;
        now(store.save_backup("b.json", BackupFile::new(&settings, at, unsaved))).expect("saved");
        let written = fs::read_to_string(folder.0.join("b.json")).unwrap();
        assert_eq!(written, write_backup(&settings, &history, at));
        let read = read_backup(&written).expect("a backup");
        assert_eq!(read_backup_as_0_16_2(&written), Ok(read.clone()));
        assert_eq!(read.sessions.len(), 5);
        assert_eq!(read.settings, Some(settings));
    }

    /// A session stored that this build cannot read, saved by a newer
    /// Dust say, goes into a backup as it is stored, so a build that reads
    /// it restores it whole; one damaged where it lies, that is not even
    /// JSON, cannot, and the user is told. Neither read as the app read
    /// the store, so the app holds neither. A session stored and still to
    /// be heard of as stored is in it once.
    #[test]
    fn a_backup_carries_a_record_this_build_cannot_read_as_it_is() {
        let (dir, store) = store("unread-backed-up");
        let folder = TempDir::new("unread-folder");
        let store = store.with_backup_folder(folder.0.clone());
        let first = SessionResult {
            timestamp: 86_400_000,
            ..session("2026-10-01")
        };
        let records = records_for_blob(std::slice::from_ref(&first));
        now(store.change_history(HistoryChange {
            put: records.clone(),
            ..HistoryChange::default()
        }));
        let sessions = dir.0.join("history").join("sessions");
        let later = r#"{"date":5,"laterShape":{"groups":[1,2]}}"#;
        let (newer, damaged) = (
            RecordId {
                order: 2_000_000_000,
                nonce: 1,
            },
            RecordId {
                order: 3_000_000_000,
                nonce: 1,
            },
        );
        fs::write(sessions.join(format!("{newer}.json")), later).unwrap();
        fs::write(sessions.join(format!("{damaged}.json")), "{ half").unwrap();
        assert_eq!(store.load_history().unread, [newer, damaged]);
        let backup = BackupFile::new(&every_setting_moved(), 9, records)
            .beside_unread([newer, damaged].into());
        let said = now(store.save_backup("b.json", backup)).expect("saved");
        assert!(
            said.ends_with("1 session stored here could not be read, and could not go in it."),
            "{said}"
        );
        let written = fs::read_to_string(folder.0.join("b.json")).unwrap();
        assert!(written.contains(later), "{written}");
        assert!(!written.contains("{ half"), "{written}");
        let read = read_backup(&written).expect("a backup");
        assert_eq!(read.sessions.len(), 1, "once, and what this build reads");
        assert_eq!(read.sessions[0].date, first.date);
        assert_eq!(read_backup_as_0_16_2(&written), Ok(read));
    }

    /// A history whose folder cannot be listed saves no backup of its own,
    /// nor leaves anything half written, and says it could not read what
    /// it holds; given every session the app holds, it saves those. The
    /// backup used to fail outright, though the app held the whole history
    /// and a backup was the user's only way to keep what it could not
    /// store.
    #[test]
    fn a_history_that_cannot_be_listed_is_backed_up_as_the_app_holds_it() {
        let dir = TempDir::new("unlisted-backed-up");
        let folder = TempDir::new("unlisted-folder");
        let store = DesktopStore::on_worker_at(dir.0.clone()).with_backup_folder(folder.0.clone());
        store.load_history();
        let history: Vec<SessionResult> = (1..=3)
            .map(|day| SessionResult {
                timestamp: day * 86_400_000,
                ..session(&format!("2026-10-{day:02}"))
            })
            .collect();
        let records = records_for_blob(&history);
        wait(store.change_history(HistoryChange {
            put: records[..2].to_vec(),
            ..HistoryChange::default()
        }));
        // The folder of sessions is a file now, which cannot be listed,
        // whoever runs the tests: root too, whom no permission holds.
        let sessions = dir.0.join("history").join("sessions");
        fs::rename(&sessions, dir.0.join("history").join("moved")).unwrap();
        fs::write(&sessions, "").unwrap();
        let settings = every_setting_moved();
        let unsaved = vec![records[2].clone()];
        let said = wait(store.save_backup("b.json", BackupFile::new(&settings, 5, unsaved)));
        assert_eq!(said, Err(NotSaved::Unread));
        assert!(files(&folder).is_empty(), "{:?}", files(&folder));

        let every = BackupFile::new(&settings, 5, records).holding_every_session();
        let said = wait(store.save_backup("b.json", every));
        assert_eq!(
            said,
            Ok(format!(
                "Backup saved as {}.",
                folder.0.join("b.json").display()
            ))
        );
        let written = fs::read_to_string(folder.0.join("b.json")).unwrap();
        assert_eq!(written, write_backup(&settings, &history, 5));
    }

    /// A session stored that cannot be read as the backup is written,
    /// though the app read it, goes in as the app holds it: no backup is
    /// saved until the app gives what it holds. It used to be left out,
    /// and called damaged.
    #[test]
    fn a_session_that_cannot_be_read_goes_in_as_the_app_holds_it() {
        let (dir, store) = store("unreadable-backed-up");
        let folder = TempDir::new("unreadable-folder");
        let store = store.with_backup_folder(folder.0.clone());
        let history: Vec<SessionResult> = (1..=3)
            .map(|day| SessionResult {
                timestamp: day * 86_400_000,
                ..session(&format!("2026-10-{day:02}"))
            })
            .collect();
        let records = records_for_blob(&history);
        store_sessions(&store, &history);
        // Its file is a folder now: listed, and not read, by root too.
        let second = dir
            .0
            .join("history")
            .join("sessions")
            .join(format!("{}.json", records[1].id));
        fs::remove_file(&second).unwrap();
        fs::create_dir(&second).unwrap();
        let settings = every_setting_moved();
        let said = now(store.save_backup("b.json", BackupFile::new(&settings, 5, Vec::new())));
        assert_eq!(said, Err(NotSaved::Unread));
        assert!(files(&folder).is_empty(), "{:?}", files(&folder));

        let every = BackupFile::new(&settings, 5, records).holding_every_session();
        let said = now(store.save_backup("b.json", every)).expect("saved");
        assert!(!said.contains("could not go in it"), "{said}");
        let written = fs::read_to_string(folder.0.join("b.json")).unwrap();
        assert_eq!(written, write_backup(&settings, &history, 5));
    }

    /// A session deleted never goes into a backup: not one whose deletion
    /// was made, nor one whose deletion was cut short and left its record
    /// beside its tombstone, nor one the app still holds as yet to be
    /// stored. Restoring the backup would bring each back. None of them
    /// is told of as left out.
    #[test]
    fn a_session_deleted_never_goes_into_a_backup() {
        let (dir, store) = store("deleted-backed-up");
        let folder = TempDir::new("deleted-folder");
        let store = store.with_backup_folder(folder.0.clone());
        let history: Vec<SessionResult> = (1..=5)
            .map(|day| SessionResult {
                timestamp: day * 86_400_000,
                ..session(&format!("2026-10-{day:02}"))
            })
            .collect();
        let records = records_for_blob(&history);
        store_sessions(&store, &history[..3]);
        let forgotten = HistoryChange {
            forget: vec![(records[1].id, Deleted::of(&history[1], 7))],
            ..HistoryChange::default()
        };
        assert_eq!(
            now(store.change_history(forgotten)),
            HistorySaved::Stored { kept: 0 }
        );
        // The fourth was deleted, and its record never went: a deletion
        // cut short.
        let history_dir = dir.0.join("history");
        let tombstone = |at: usize| {
            fs::write(
                history_dir
                    .join("deleted")
                    .join(format!("{}.json", records[at].id)),
                serde_json::to_vec(&Deleted::of(&history[at], 7)).unwrap(),
            )
            .unwrap();
        };
        fs::write(
            history_dir
                .join("sessions")
                .join(format!("{}.json", records[3].id)),
            &records[3].json,
        )
        .unwrap();
        tombstone(3);
        // The fifth the app holds as yet to be stored, and its deletion was
        // made.
        tombstone(4);
        let settings = every_setting_moved();
        let unsaved = vec![records[4].clone()];
        let said = now(store.save_backup("b.json", BackupFile::new(&settings, 5, unsaved)))
            .expect("saved");
        assert_eq!(
            said,
            format!("Backup saved as {}.", folder.0.join("b.json").display()),
            "nothing left out"
        );
        let written = fs::read_to_string(folder.0.join("b.json")).unwrap();
        let kept = [0, 2].map(|at| history[at].clone());
        assert_eq!(written, write_backup(&settings, &kept, 5));

        // Every session the app holds given, the deleted ones stay out all
        // the same.
        let every = BackupFile::new(&settings, 5, records).holding_every_session();
        now(store.save_backup("c.json", every)).expect("saved");
        let written = fs::read_to_string(folder.0.join("c.json")).unwrap();
        assert_eq!(written, write_backup(&settings, &kept, 5));
    }

    /// The app's store saves a backup on the history's own thread: the
    /// same file, said of in the same words.
    #[test]
    fn a_backup_is_written_on_the_historys_own_thread() {
        let dir = TempDir::new("backup-on-worker");
        let folder = TempDir::new("backup-on-worker-folder");
        let store = DesktopStore::on_worker_at(dir.0.clone()).with_backup_folder(folder.0.clone());
        store.load_history();
        let history = [session("2026-10-01")];
        wait(store.change_history(HistoryChange {
            put: records_for_blob(&history),
            ..HistoryChange::default()
        }));
        let settings = every_setting_moved();
        let said = wait(store.save_backup("b.json", BackupFile::new(&settings, 3, Vec::new())));
        assert_eq!(
            said,
            Ok(format!(
                "Backup saved as {}.",
                folder.0.join("b.json").display()
            ))
        );
        let written = fs::read_to_string(folder.0.join("b.json")).unwrap();
        assert_eq!(written, write_backup(&settings, &history, 3));
    }

    #[test]
    fn reads_the_package_name_from_cmdline() {
        assert_eq!(
            package_from_cmdline(b"dev.dust.morse\0").as_deref(),
            Some("dev.dust.morse")
        );
        assert_eq!(
            package_from_cmdline(b"dev.dust.morse\0/system/bin/app_process\0").as_deref(),
            Some("dev.dust.morse")
        );
        assert_eq!(
            package_from_cmdline(b"dev.dust.morse:remote\0").as_deref(),
            Some("dev.dust.morse")
        );
    }

    #[test]
    fn rejects_anything_that_is_not_a_package() {
        assert_eq!(package_from_cmdline(b""), None);
        assert_eq!(package_from_cmdline(b"\0"), None);
        // A desktop process name has no dots and must not become a /data/data path.
        assert_eq!(package_from_cmdline(b"dust\0"), None);
        assert_eq!(package_from_cmdline(b"/usr/bin/dust\0"), None);
    }

    /// The phone's owner keeps the app's files under `/data/user/0`, which
    /// is `/data/data`; a work profile, another user, Private Space or a
    /// cloned app under a user of its own, read from the process's uid.
    #[test]
    fn the_android_files_folder_is_the_users_own() {
        let status = |uid: &str| {
            format!(
                "Name:\tdev.dust.morse\nUmask:\t0077\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nGid:\t{uid}\t{uid}\t{uid}\t{uid}\n"
            )
        };
        assert_eq!(android_user_from_status(&status("10123")), Some(0));
        assert_eq!(android_user_from_status(&status("1010123")), Some(10));
        assert_eq!(android_user_from_status(&status("95010123")), Some(950));
        assert_eq!(android_user_from_status("Name:\tdust\n"), None);
        assert_eq!(android_user_from_status("Uid:\tnobody\n"), None);
        assert_eq!(
            android_files_dir_of(0, "dev.dust.morse"),
            PathBuf::from("/data/user/0/dev.dust.morse/files")
        );
        assert_eq!(
            android_files_dir_of(10, "dev.dust.morse"),
            PathBuf::from("/data/user/10/dev.dust.morse/files")
        );
    }

    /// On Android, a history past 20 MB is about to outgrow Android's own
    /// backup of the app, which takes none of an app past 25 MB; nowhere
    /// else is there such a limit. The store knows how large its history
    /// is from the history itself.
    #[test]
    fn a_history_past_twenty_megabytes_outgrows_androids_own_backup() {
        assert!(!outgrows_device_backup(DEVICE_BACKUP_WARN_BYTES, true));
        assert!(outgrows_device_backup(DEVICE_BACKUP_WARN_BYTES + 1, true));
        assert!(!outgrows_device_backup(u64::MAX, false));
        let (_dir, store) = store("outgrown");
        store.load_history();
        let history = [session("2026-10-01"), session("2026-10-02")];
        let records = records_for_blob(&history);
        store_sessions(&store, &history);
        let held: usize = records.iter().map(|record| record.json.len()).sum();
        assert_eq!(store.history.bytes(), held as u64);
        assert!(!store.outgrows_device_backup(), "not on this platform");
    }

    /// With no data folder to be had — no home to find one from — the
    /// store still goes somewhere whole, never under whatever folder the
    /// app was started in.
    #[test]
    fn the_data_directory_is_never_a_relative_path() {
        assert!(under_data_dir(None).is_absolute());
        assert!(under_data_dir(None).ends_with("dust"));
        let data = std::env::temp_dir().join("data");
        assert_eq!(under_data_dir(Some(data.clone())), data.join("dust"));
        assert!(data_dir_from(None).is_absolute());
    }
}
