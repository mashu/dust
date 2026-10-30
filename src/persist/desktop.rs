//! Files under one directory: the desktop and mobile store.

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};

use super::{
    BackupSaving, CutOff, CutOffTaking, Deleted, HistorySaved, OpenSession, SETTINGS_HELD,
    SetAside, Store, newest, over_stored, read_history, read_settings_from,
};

const THEME: &str = "theme.txt";
const TUTORIAL: &str = "tutorial_done.txt";
const SETTINGS: &str = "settings.json";
const SESSIONS: &str = "sessions.json";
/// The sessions deleted by hand ([`Deleted`]).
const DELETED: &str = "deleted.json";
const COUNTERS: &str = "auto_adjust.json";
const LAST_BACKUP: &str = "last_backup.txt";
/// The sessions and runs marked open ([`Store::mark_open`]): each copy of
/// the app's as `<id>.json`, beside `<id>.lock`, which that copy holds
/// locked for as long as its session runs. A copy that stops, however it
/// stops, lets go of its lock, so a mark nobody holds was cut off, and one
/// held belongs to a window still running it.
const OPEN: &str = "open";

/// Files under one directory. The directory is a field rather than a global
/// so a test can point one at a scratch folder.
pub struct DesktopStore {
    root: PathBuf,
    /// Where backup files go, when there is anywhere the user can reach.
    backup_folder: Option<PathBuf>,
    /// How the user moves a backup file out of the app, where it is saved
    /// inside it ([`Store::backups_inside_app`]).
    backups_inside: Option<&'static str>,
    /// The stored history could not be read, nor moved out of the way, the
    /// last time it was read, so it is not to be written over until a read
    /// says otherwise.
    history_held: Cell<bool>,
    /// The same, of the settings.
    settings_held: Cell<bool>,
    /// What reads set aside, not yet told of ([`Store::take_set_aside`]).
    set_aside: RefCell<Vec<SetAside>>,
    /// This copy's mark, while its session is marked open.
    open_held: RefCell<Option<HeldMark>>,
}

/// A copy's mark ([`OPEN`]): its id, and the lock it holds on it.
struct HeldMark {
    id: String,
    lock: fs::File,
}

impl DesktopStore {
    pub fn new() -> Self {
        Self {
            backup_folder: backup_folder(),
            backups_inside: backups_inside_app(),
            ..Self::at(data_dir())
        }
    }

    /// Files under `root`, and no backups.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            backup_folder: None,
            backups_inside: None,
            history_held: Cell::new(false),
            settings_held: Cell::new(false),
            set_aside: RefCell::new(Vec::new()),
            open_held: RefCell::new(None),
        }
    }

    /// Backup files go into `folder`.
    #[cfg(test)]
    fn with_backup_folder(self, folder: impl Into<PathBuf>) -> Self {
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
    /// way of the next save, keeping it beside the new one to be recovered,
    /// and say where it went: nowhere, when it could not be moved.
    fn set_aside(&self, path: &Path, stem: &str) -> Option<String> {
        let name = aside_name(&self.root, stem);
        match fs::rename(path, self.root.join(&name)) {
            Ok(()) => Some(name),
            Err(err) => {
                eprintln!("could not set aside {stem}.json, which could not be read: {err}");
                None
            }
        }
    }

    /// Where the user can find `name`, a file of the store's, in words for
    /// them: on a computer. A phone keeps the store inside the app, where no
    /// file manager reaches.
    fn shown(&self, name: &str) -> Option<String> {
        let reachable = !cfg!(any(target_os = "android", target_os = "ios"));
        reachable.then(|| self.root.join(name).display().to_string())
    }

    /// Keep `unread`, the `count` sessions of the history that could not be
    /// read, in a file of their own beside it before the next save leaves
    /// them out — unless a file set aside before holds them already. If they
    /// cannot be kept, nothing is to be written over the history.
    fn keep_aside(&self, unread: &[u8], count: usize) {
        let kept = fs::read_dir(&self.root)
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
            return;
        }
        let name = aside_name(&self.root, "sessions");
        match self.write(&name, unread) {
            Ok(()) => self.set_aside.borrow_mut().push(SetAside::Sessions {
                count,
                kept: self.shown(&name),
            }),
            Err(err) => {
                eprintln!("could not set aside the sessions that could not be read: {err}");
                self.history_held.set(true);
            }
        }
    }
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
    static WRITES: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let tmp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, &path)?;
        sync_dir(dir)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
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
fn sync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

/// Windows cannot open a directory to flush it; its renames are journalled.
#[cfg(not(unix))]
fn sync_dir(_: &Path) -> io::Result<()> {
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

    /// Whether the history is held is this read's to say, whatever the last
    /// one said: a file another program had open a moment ago, an
    /// antivirus scanning it say, may read now, and one that could not be
    /// set aside then may be now.
    fn read_sessions(&self) -> Option<Vec<SessionResult>> {
        self.history_held.set(false);
        let path = self.root.join(SESSIONS);
        let raw = match fs::read(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
            Err(err) => {
                // There and unreadable: not a history to start afresh over.
                eprintln!("could not read the history: {err}");
                self.history_held.set(true);
                return None;
            }
        };
        let Some(read) = std::str::from_utf8(&raw).ok().and_then(read_history) else {
            match self.set_aside(&path, "sessions") {
                Some(name) => self.set_aside.borrow_mut().push(SetAside::History {
                    kept: self.shown(&name),
                }),
                None => self.history_held.set(true),
            }
            return None;
        };
        if let Some(unread) = read.unread_json() {
            self.keep_aside(unread.as_bytes(), read.unread.len());
        }
        Some(read.sessions)
    }

    fn take_set_aside(&self) -> Vec<SetAside> {
        std::mem::take(&mut self.set_aside.borrow_mut())
    }

    /// None, where the file cannot be read: a deletion forgotten costs a
    /// session deleted coming back, never one kept going.
    fn read_deleted(&self) -> Vec<Deleted> {
        fs::read(self.root.join(DELETED))
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default()
    }

    fn save_deleted(&self, deleted: &[Deleted]) {
        if let Ok(raw) = serde_json::to_vec(deleted) {
            let _ = self.write(DELETED, &raw);
        }
    }

    fn save_sessions(&self, sessions: &[SessionResult]) -> HistorySaved {
        if self.history_held.get() {
            return HistorySaved::Held;
        }
        let kept = newest(sessions);
        // Compact: the history is the one file that grows, and nobody edits it.
        let written = serde_json::to_vec(kept)
            .map_err(io::Error::other)
            .and_then(|raw| self.write(SESSIONS, &raw));
        match written {
            Ok(()) => HistorySaved::Stored { kept: kept.len() },
            Err(err) => HistorySaved::Failed(err.to_string()),
        }
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

    fn save_backup_file(&self, name: &str, contents: &str) -> BackupSaving {
        #[cfg(all(target_os = "android", not(test)))]
        let saving = super::android::save_backup(name.to_string(), contents.to_string());
        #[cfg(not(all(target_os = "android", not(test))))]
        let saving = super::saved_now(self.save_in_folder(name, contents));
        Box::pin(async move { saving.await.map(|place| super::saved_as(&place)) })
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

impl DesktopStore {
    /// Save a backup file in the backup folder, and say where it went.
    #[cfg_attr(all(target_os = "android", not(test)), allow(dead_code))]
    fn save_in_folder(&self, name: &str, contents: &str) -> Result<String, String> {
        let folder = self
            .backup_folder
            .as_deref()
            .ok_or("there is no folder to put it in")?;
        let name = free_name(folder, name);
        write_whole(folder, &name, contents.as_bytes()).map_err(|err| err.to_string())?;
        if cfg!(target_os = "ios") {
            Ok(format!("{name}, in the Files app, in Dust's folder"))
        } else {
            Ok(folder.join(name).display().to_string())
        }
    }
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
mod tests {
    use super::*;
    use crate::persist::MAX_SESSIONS;
    use crate::persist::tests::{every_setting_moved, session};

    /// A scratch directory that cleans up after itself.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("dust-test-{}-{name}-{unique}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn store(name: &str) -> (TempDir, DesktopStore) {
        let dir = TempDir::new(name);
        let store = DesktopStore::at(dir.0.clone());
        (dir, store)
    }

    /// The files in the store's directory, by name.
    fn files(dir: &TempDir) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn open_at(started_at: u64) -> OpenSession {
        OpenSession {
            started_at,
            run: false,
            so_far: None,
        }
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
        assert_eq!(store.load_settings(), TrainingSettings::default().clamp());
        assert!(store.load_sessions().is_empty());
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

        let mut settings = TrainingSettings::default();
        settings.curriculum.num_groups = 7;
        settings.playback.char_wpm_min = 24.0;
        settings.playback.char_wpm_max = 24.0;
        store.save_settings(&settings);
        assert_eq!(store.load_settings().curriculum.num_groups, 7);
        assert_eq!(store.load_settings().playback.char_wpm_min, 24.0);

        assert_eq!(
            store.save_sessions(&[session("2026-09-01"), session("2026-09-02")]),
            HistorySaved::Stored { kept: 2 }
        );
        let read = store.load_sessions();
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
                "auto_adjust.json",
                "sessions.json",
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

    /// The history is the file that grows: written compact, it is about half
    /// the size it was pretty-printed.
    #[test]
    fn the_history_is_written_compact() {
        let (dir, store) = store("compact");
        store.save_sessions(&[session("2026-09-01")]);
        let raw = fs::read_to_string(dir.0.join(SESSIONS)).unwrap();
        assert!(!raw.contains('\n'), "{raw}");
    }

    #[test]
    fn history_is_trimmed_on_the_way_to_disk() {
        let (_dir, store) = store("trim");
        let all: Vec<SessionResult> = (0..MAX_SESSIONS + 5)
            .map(|i| session(&format!("2026-01-{i:03}")))
            .collect();
        assert_eq!(
            store.save_sessions(&all),
            HistorySaved::Stored { kept: MAX_SESSIONS }
        );
        assert_eq!(store.load_sessions().len(), MAX_SESSIONS);
    }

    #[test]
    fn a_damaged_file_does_not_take_the_rest_with_it() {
        let (dir, store) = store("damaged");
        store.save_theme("light");
        fs::write(dir.0.join(SESSIONS), "{ not json").unwrap();
        fs::write(dir.0.join(SETTINGS), "{ not json").unwrap();
        fs::write(dir.0.join(COUNTERS), "{ not json").unwrap();
        assert!(store.load_sessions().is_empty());
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
        fs::write(dir.0.join(SESSIONS), format!("[{later}]")).unwrap();
        let mut history = store.load_sessions();
        history.push(session("2026-09-02"));
        store.save_sessions(&history);
        let held: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.0.join(SESSIONS)).unwrap()).unwrap();
        assert_eq!(held[0]["laterKey"], "kept");
        assert_eq!(store.load_sessions().len(), 2);
    }

    /// The sessions deleted by hand are remembered, what a later build
    /// added to each along with them; a file that cannot be read remembers
    /// none, which costs a deleted session coming back, never one kept.
    #[test]
    fn the_sessions_deleted_are_remembered() {
        let (dir, store) = store("deleted");
        assert!(store.read_deleted().is_empty());
        let mut gone = Deleted::of(&session("2026-09-01"), 7);
        gone.unread.insert("laterKey".into(), serde_json::json!(1));
        store.save_deleted(std::slice::from_ref(&gone));
        assert_eq!(DesktopStore::at(dir.0.clone()).read_deleted(), [gone]);
        fs::write(dir.0.join(DELETED), "[{").unwrap();
        assert!(store.read_deleted().is_empty());
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
    /// overwritten. It used to come back as an empty history, and the next
    /// finished session was saved over it — every session before it gone.
    #[test]
    fn an_unreadable_history_is_kept_rather_than_written_over() {
        let (dir, store) = store("unreadable");
        fs::create_dir_all(&dir.0).unwrap();
        let damaged = "{\"this\": \"is not a history\"";
        fs::write(dir.0.join(SESSIONS), damaged).unwrap();

        assert!(store.load_sessions().is_empty());
        let told = store.take_set_aside();
        let [SetAside::History { kept: Some(told) }] = told.as_slice() else {
            panic!("{told:?}");
        };
        assert_eq!(fs::read_to_string(told).unwrap(), damaged);
        assert_eq!(
            store.save_sessions(&[session("2026-09-02")]),
            HistorySaved::Stored { kept: 1 }
        );
        assert_eq!(store.load_sessions().len(), 1);
        assert!(store.take_set_aside().is_empty(), "told of once");

        let kept: Vec<String> = files(&dir)
            .into_iter()
            .filter(|name| name.starts_with("sessions.unreadable-"))
            .collect();
        assert_eq!(
            kept.len(),
            1,
            "the damaged history should have been set aside"
        );
        assert_eq!(fs::read_to_string(dir.0.join(&kept[0])).unwrap(), damaged);
    }

    /// When an unreadable history cannot even be moved out of the way, it is
    /// left alone: no save goes over it. This used to be logged and then
    /// written over by the next finished session.
    #[test]
    fn an_unreadable_history_that_cannot_be_moved_is_never_written_over() {
        let (dir, store) = store("stuck");
        fs::create_dir_all(dir.0.join(SESSIONS)).unwrap();
        // A directory where the history belongs: it reads as unreadable, and
        // a rename onto it cannot happen.
        assert!(store.load_sessions().is_empty());
        assert_eq!(
            store.save_sessions(&[session("2026-09-02")]),
            HistorySaved::Held
        );
        assert!(
            dir.0.join(SESSIONS).is_dir(),
            "the history was written over"
        );
    }

    /// A history that could not be read for a moment is held only until it
    /// reads again: the next save after that goes ahead, over the history
    /// as it was just read. The hold used to last until the app was closed,
    /// and every session in between went unsaved.
    #[test]
    fn a_history_held_for_a_moment_is_let_go_once_it_reads() {
        let (dir, store) = store("held-for-a-moment");
        store.save_sessions(&[session("2026-09-01")]);
        let path = dir.0.join(SESSIONS);
        let stored = fs::read(&path).unwrap();
        // Something else has it, the way an antivirus scanning it might:
        // it is there, and does not read.
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(store.read_sessions(), None);
        assert_eq!(
            store.save_sessions(&[session("2026-09-02")]),
            HistorySaved::Held
        );
        assert!(path.is_dir(), "nothing written over it");
        // Still held, however often it is saved, until a read says not.
        fs::remove_dir(&path).unwrap();
        fs::write(&path, &stored).unwrap();
        assert_eq!(
            store.save_sessions(&[session("2026-09-02")]),
            HistorySaved::Held
        );
        let read = store.read_sessions().expect("it reads again");
        assert_eq!(read.len(), 1);
        let mut more = read;
        more.push(session("2026-09-02"));
        assert_eq!(store.save_sessions(&more), HistorySaved::Stored { kept: 2 });
        assert_eq!(store.load_sessions().len(), 2);
        // And held again the next time it does not read.
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(store.load_sessions().is_empty());
        assert_eq!(store.save_sessions(&more), HistorySaved::Held);
    }

    #[test]
    fn a_history_that_cannot_be_written_leaves_the_old_one_alone() {
        let (dir, store) = store("unwritable");
        store.save_sessions(&[session("2026-09-01")]);
        assert_eq!(store.load_sessions().len(), 1);

        // JSON has no way to write a NaN, so such a session is written as null
        // and dropped on the way back in rather than breaking the whole file:
        // set aside, as any session that cannot be read is.
        let mut broken = session("2026-09-02");
        broken.accuracy = f64::NAN;
        store.save_sessions(&[broken, session("2026-09-03")]);
        let read = store.load_sessions();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].date, "2026-09-03");
        assert_eq!(set_aside(&dir).len(), 1);

        // A destination that cannot be replaced says so, and leaves no
        // scratch file behind.
        fs::remove_file(dir.0.join(SESSIONS)).unwrap();
        fs::create_dir(dir.0.join(SESSIONS)).unwrap();
        assert!(matches!(
            store.save_sessions(&[session("2026-09-04")]),
            HistorySaved::Failed(_)
        ));
        let mut left = files(&dir);
        left.retain(|name| !name.starts_with("sessions.unreadable-"));
        assert_eq!(left, [SESSIONS]);
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
    /// finished since reads as stamped ahead of it; read, and stored again
    /// merged as a store of the history merges it, each keeps the stamp and
    /// the day it was stored with. They used to be settled onto the day of
    /// the latest session the clock agreed with as they were read, and the
    /// next save wrote that over them for good.
    #[test]
    fn a_clock_set_behind_moves_no_stored_session() {
        let (dir, store) = store("behind");
        let now = crate::time::now_ms();
        let at = |days_on: i64, date: &str| SessionResult {
            timestamp: now.saturating_add_signed(days_on * 86_400_000),
            ..session(date)
        };
        // The clock was set a week back: the last week reads as ahead.
        let stored = vec![
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
            store.save_sessions(&stored),
            HistorySaved::Stored { kept: 5 }
        );
        let opened = store.load_sessions();
        assert_eq!(stamped(&opened), stamped(&stored));
        let held = store.read_sessions().expect("a history");
        let merged = crate::persist::merge_history(&opened, &opened, &held, &Default::default());
        assert_eq!(
            store.save_sessions(&merged),
            HistorySaved::Stored { kept: 5 }
        );
        let reopened = DesktopStore::at(dir.0.clone()).load_sessions();
        assert_eq!(stamped(&reopened), stamped(&stored));
    }

    /// Sessions that cannot be read are set aside before a save leaves them
    /// out, once however often the history is read before it, and the rest
    /// read as ever. They used to be dropped as they were read, and the
    /// next save wrote the history without them.
    #[test]
    fn sessions_that_cannot_be_read_are_set_aside_before_a_save() {
        let (dir, store) = store("unread");
        fs::create_dir_all(&dir.0).unwrap();
        let good = |date| serde_json::to_string(&session(date)).unwrap();
        let raw = format!(
            "[{},{{\"date\":\"nonsense\"}},{}]",
            good("2026-09-01"),
            good("2026-09-02")
        );
        fs::write(dir.0.join(SESSIONS), &raw).unwrap();

        let read = store.load_sessions();
        assert_eq!(read.len(), 2);
        assert_eq!(set_aside(&dir), [r#"[{"date":"nonsense"}]"#]);
        assert!(matches!(
            store.take_set_aside().as_slice(),
            [SetAside::Sessions {
                count: 1,
                kept: Some(_)
            }]
        ));
        // Read again, as another copy opening would, before any save.
        let another = DesktopStore::at(dir.0.clone());
        assert_eq!(another.load_sessions(), read);
        assert_eq!(set_aside(&dir).len(), 1, "set aside once");
        assert!(another.take_set_aside().is_empty(), "and told of once");

        let mut more = read;
        more.push(session("2026-09-03"));
        assert_eq!(store.save_sessions(&more), HistorySaved::Stored { kept: 3 });
        assert_eq!(store.load_sessions().len(), 3);
        assert_eq!(set_aside(&dir), [r#"[{"date":"nonsense"}]"#]);
    }

    /// One history set aside is never written over by the next, however
    /// soon after it that comes.
    #[test]
    fn a_history_set_aside_is_never_written_over_by_the_next() {
        let (dir, store) = store("aside-twice");
        fs::create_dir_all(&dir.0).unwrap();
        for damaged in ["{ first", "{ second"] {
            fs::write(dir.0.join(SESSIONS), damaged).unwrap();
            assert!(store.load_sessions().is_empty());
        }
        let mut kept = set_aside(&dir);
        kept.sort();
        assert_eq!(kept, ["{ first", "{ second"]);
    }

    /// A backup goes into the backup folder, never over an earlier one, and
    /// the store remembers when.
    #[test]
    fn a_backup_is_a_new_file_in_the_backup_folder() {
        let (dir, store) = store("backup");
        let folder = TempDir::new("backup-folder");
        assert!(!store.saves_backups(), "a store with no folder saves none");
        assert!(store.save_in_folder("b.json", "{}").is_err());
        let store = store.with_backup_folder(folder.0.clone());
        assert!(store.saves_backups());
        let first = store.save_in_folder("dust-backup-2026-10-19.json", "one");
        let second = store.save_in_folder("dust-backup-2026-10-19.json", "two");
        assert_eq!(
            first,
            Ok(folder
                .0
                .join("dust-backup-2026-10-19.json")
                .display()
                .to_string())
        );
        assert_eq!(
            second,
            Ok(folder
                .0
                .join("dust-backup-2026-10-19-2.json")
                .display()
                .to_string())
        );
        assert_eq!(
            fs::read_to_string(folder.0.join("dust-backup-2026-10-19.json")).unwrap(),
            "one"
        );
        assert_eq!(
            files(&folder),
            [
                "dust-backup-2026-10-19-2.json",
                "dust-backup-2026-10-19.json"
            ]
        );

        assert_eq!(store.last_backup(), None);
        store.mark_backup(1_790_000_000_000);
        assert_eq!(store.last_backup(), Some(1_790_000_000_000));
        assert_eq!(files(&dir), [LAST_BACKUP]);
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
