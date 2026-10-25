//! A store that keeps everything in memory, for tests: each test makes its
//! own, so nothing one saves reaches another, or the user's real data.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};

use super::{BackupSaving, HistorySaved, Store, newest, saved_now};

#[derive(Default)]
pub struct MemoryStore {
    theme: RefCell<String>,
    tutorial_done: Cell<bool>,
    settings: RefCell<Option<TrainingSettings>>,
    sessions: RefCell<Vec<SessionResult>>,
    counters: RefCell<BTreeMap<CountersKey, AutoLevelCounters>>,
    /// How many times the settings were written, for a test that cares how
    /// often that happens.
    pub settings_saves: Cell<u32>,
    /// Room for only this many sessions, the way a full browser leaves it.
    pub room: Cell<Option<usize>>,
    /// A history that could not be read is in the way of any save.
    pub held: Cell<bool>,
    last_backup: Cell<Option<u64>>,
    /// Every backup file handed over: its name and what it held.
    pub backup_files: RefCell<Vec<(String, String)>>,
    /// A build that cannot hand over a backup file, the way a phone's cannot.
    pub no_backups: Cell<bool>,
    /// How many times the store was asked to keep its data.
    pub keep_asks: Cell<u32>,
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

    fn read_settings(&self) -> Option<TrainingSettings> {
        self.settings.borrow().clone().map(TrainingSettings::clamp)
    }

    fn save_settings(&self, settings: &TrainingSettings) {
        self.settings_saves.set(self.settings_saves.get() + 1);
        *self.settings.borrow_mut() = Some(settings.clone());
    }

    fn load_sessions(&self) -> Vec<SessionResult> {
        self.sessions.borrow().clone()
    }

    fn save_sessions(&self, sessions: &[SessionResult]) -> HistorySaved {
        if self.held.get() {
            return HistorySaved::Held;
        }
        let newest = newest(sessions);
        let kept =
            &newest[newest.len() - self.room.get().unwrap_or(usize::MAX).min(newest.len())..];
        *self.sessions.borrow_mut() = kept.to_vec();
        if kept.len() < newest.len() {
            HistorySaved::Squeezed {
                kept: kept.len(),
                runs: newest.iter().any(|session| session.contest.is_some()),
            }
        } else {
            HistorySaved::Stored { kept: kept.len() }
        }
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

    fn save_backup_file(&self, name: &str, contents: &str) -> BackupSaving {
        if self.no_backups.get() {
            return saved_now(Err("this build has nowhere to put it".into()));
        }
        self.backup_files
            .borrow_mut()
            .push((name.to_string(), contents.to_string()));
        saved_now(Ok(format!("{name}, in your downloads")))
    }

    fn ask_to_keep(&self) {
        self.keep_asks.set(self.keep_asks.get() + 1);
    }
}
