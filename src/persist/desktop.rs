//! Files under one directory: the desktop and mobile store.

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};

use super::{
    BackupSaving, HistorySaved, OpenSession, Store, newest, parse_sessions, read_settings_from,
};

const THEME: &str = "theme.txt";
const TUTORIAL: &str = "tutorial_done.txt";
const SETTINGS: &str = "settings.json";
const SESSIONS: &str = "sessions.json";
const COUNTERS: &str = "auto_adjust.json";
const LAST_BACKUP: &str = "last_backup.txt";
/// The session or run marked open ([`Store::mark_open`]).
const OPEN: &str = "open_session.json";
/// Locked by the copy of the app whose session is marked open, for as long
/// as it runs: a copy that stops, however it stops, lets go of it, so the
/// next can tell a session cut off from one another window is running.
const OPEN_LOCK: &str = "open_session.lock";

/// Files under one directory. The directory is a field rather than a global
/// so a test can point one at a scratch folder.
pub struct DesktopStore {
    root: PathBuf,
    /// Where backup files go, when there is anywhere the user can reach.
    backup_folder: Option<PathBuf>,
    /// The stored history could not be read, nor moved out of the way, so it
    /// is not to be written over.
    history_held: Cell<bool>,
    /// [`OPEN_LOCK`], held while this copy's session is marked open.
    open_held: RefCell<Option<fs::File>>,
}

impl DesktopStore {
    pub fn new() -> Self {
        Self {
            backup_folder: backup_folder(),
            ..Self::at(data_dir())
        }
    }

    /// Files under `root`, and no backups.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            backup_folder: None,
            history_held: Cell::new(false),
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

    fn load_all_counters(&self) -> std::collections::BTreeMap<String, AutoLevelCounters> {
        fs::read_to_string(self.root.join(COUNTERS))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_all_counters(&self, counters: &std::collections::BTreeMap<String, AutoLevelCounters>) {
        if let Ok(raw) = serde_json::to_vec_pretty(counters) {
            let _ = self.write(COUNTERS, &raw);
        }
    }

    /// Move an unreadable history out of the way of the next save, keeping
    /// it beside the new one to be recovered. If it cannot be moved, nothing
    /// is to be written over it.
    fn set_aside(&self, path: &Path) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let aside = self.root.join(format!("sessions.unreadable-{stamp}.json"));
        if let Err(err) = fs::rename(path, &aside) {
            eprintln!("could not set aside an unreadable history: {err}");
            self.history_held.set(true);
        }
    }
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

    fn read_settings(&self) -> Option<TrainingSettings> {
        read_settings_from(&fs::read_to_string(self.root.join(SETTINGS)).ok()?)
    }

    fn save_settings(&self, settings: &TrainingSettings) {
        // Pretty, as the one file somebody might open and edit by hand.
        if let Ok(raw) = serde_json::to_vec_pretty(settings) {
            let _ = self.write(SETTINGS, &raw);
        }
    }

    fn load_sessions(&self) -> Vec<SessionResult> {
        let path = self.root.join(SESSIONS);
        let raw = match fs::read(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Vec::new(),
            Err(err) => {
                // There and unreadable: not a history to start afresh over.
                eprintln!("could not read the history: {err}");
                self.history_held.set(true);
                return Vec::new();
            }
        };
        match std::str::from_utf8(&raw).ok().and_then(parse_sessions) {
            Some(sessions) => sessions,
            None => {
                self.set_aside(&path);
                Vec::new()
            }
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
        let mut all = self.load_all_counters();
        all.insert(key.as_str().to_string(), counters);
        self.save_all_counters(&all);
    }

    fn clear_auto_counters(&self, keys: &[CountersKey]) {
        let mut all = self.load_all_counters();
        for key in keys {
            all.remove(key.as_str());
        }
        self.save_all_counters(&all);
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

    fn save_backup_file(&self, name: &str, contents: &str) -> BackupSaving {
        #[cfg(all(target_os = "android", not(test)))]
        return super::android::save_backup(name.to_string(), contents.to_string());
        #[cfg(not(all(target_os = "android", not(test))))]
        super::saved_now(self.save_in_folder(name, contents))
    }

    /// The marker is written and taken away only under the lock, and read
    /// only under it, so a copy starting up never takes a session another
    /// is running for one cut off. Written unsynced: it is only a marker,
    /// and the first group waits on nothing.
    fn mark_open(&self, open: Option<OpenSession>) {
        let path = self.root.join(OPEN);
        let mut held = self.open_held.borrow_mut();
        match open {
            Some(open) => {
                // Without the lock nothing is marked: better a session cut
                // off unsaid than one running told of as lost.
                let Some(lock) = held.take().or_else(|| self.lock_open()) else {
                    return;
                };
                if let Ok(raw) = serde_json::to_vec(&open)
                    && fs::write(&path, raw).is_ok()
                {
                    *held = Some(lock);
                }
            }
            None => {
                let _ = fs::remove_file(&path);
                // Let go once the marker is gone.
                held.take();
            }
        }
    }

    fn take_cut_off(&self) -> Option<OpenSession> {
        let path = self.root.join(OPEN);
        if !path.exists() {
            return None;
        }
        // Held: another copy of the app is running it.
        let _lock = self.lock_open()?;
        let raw = fs::read(&path).ok()?;
        let _ = fs::remove_file(&path);
        serde_json::from_slice(&raw).ok()
    }
}

impl DesktopStore {
    /// [`OPEN_LOCK`], locked for this copy, unless another holds it.
    fn lock_open(&self) -> Option<fs::File> {
        fs::create_dir_all(&self.root).ok()?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join(OPEN_LOCK))
            .ok()?;
        file.try_lock().ok()?;
        Some(file)
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

/// Android hands an app no `HOME`, so `dirs` has nothing to work from. The
/// first entry of `/proc/self/cmdline` is the package name, and
/// `/data/data/<package>/files` is the private directory the app owns.
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

#[cfg(target_os = "android")]
fn android_data_dir() -> Option<PathBuf> {
    let raw = fs::read("/proc/self/cmdline").ok()?;
    let package = package_from_cmdline(&raw)?;
    let dir = PathBuf::from(format!("/data/data/{package}/files/dust"));
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
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dust")
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

    /// A session marked open by a copy of the app that stopped under it is
    /// told of once; one a copy still running holds is left alone; one that
    /// ended is never told of.
    #[test]
    fn a_session_left_open_is_told_of_once_and_only_when_cut_off() {
        let (dir, running) = store("open");
        let open = OpenSession {
            started_at: 1_790_000_000_000,
            run: false,
        };
        running.mark_open(Some(open));
        // Another copy, started while it runs, leaves it to the one running.
        let other = DesktopStore::at(dir.0.clone());
        assert_eq!(other.take_cut_off(), None, "still running elsewhere");
        // Ended: nothing to tell of.
        running.mark_open(None);
        assert_eq!(other.take_cut_off(), None);

        // The app stops under it: the next copy is told, once.
        running.mark_open(Some(open));
        drop(running);
        let next = DesktopStore::at(dir.0.clone());
        assert_eq!(next.take_cut_off(), Some(open));
        assert_eq!(next.take_cut_off(), None, "told of once");
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
        assert_eq!(
            store.save_sessions(&[session("2026-09-02")]),
            HistorySaved::Stored { kept: 1 }
        );
        assert_eq!(store.load_sessions().len(), 1);

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

    #[test]
    fn a_history_that_cannot_be_written_leaves_the_old_one_alone() {
        let (dir, store) = store("unwritable");
        store.save_sessions(&[session("2026-09-01")]);
        assert_eq!(store.load_sessions().len(), 1);

        // JSON has no way to write a NaN, so such a session is written as null
        // and dropped on the way back in rather than breaking the whole file.
        let mut broken = session("2026-09-02");
        broken.accuracy = f64::NAN;
        store.save_sessions(&[broken, session("2026-09-03")]);
        let read = store.load_sessions();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].date, "2026-09-03");

        // A destination that cannot be replaced says so, and leaves no
        // scratch file behind.
        fs::remove_file(dir.0.join(SESSIONS)).unwrap();
        fs::create_dir(dir.0.join(SESSIONS)).unwrap();
        assert!(matches!(
            store.save_sessions(&[session("2026-09-04")]),
            HistorySaved::Failed(_)
        ));
        assert_eq!(files(&dir), [SESSIONS]);
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
}
