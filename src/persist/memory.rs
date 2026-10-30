//! A store that keeps everything in memory, for tests: each test makes its
//! own, so nothing one saves reaches another, or the user's real data.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};

use super::{
    BackupSaving, CutOff, CutOffTaking, Deleted, HistorySaved, OpenSession, SetAside, Store,
    newest, saved_as, saved_now,
};

#[derive(Default)]
pub struct MemoryStore {
    theme: RefCell<String>,
    tutorial_done: Cell<bool>,
    pub told_of_paddle_keys: Cell<bool>,
    settings: RefCell<Option<TrainingSettings>>,
    /// The history, once one has been stored.
    sessions: RefCell<Option<Vec<SessionResult>>>,
    counters: RefCell<BTreeMap<CountersKey, AutoLevelCounters>>,
    /// How many times the settings were written, for a test that cares how
    /// often that happens.
    pub settings_saves: Cell<u32>,
    /// The settings cannot be written, for the reason given, the way a
    /// full disk refuses them.
    pub settings_refused: RefCell<Option<String>>,
    /// Room for only this many sessions, the way a full browser leaves it.
    pub room: Cell<Option<usize>>,
    /// A history that could not be read is in the way of any save.
    pub held: Cell<bool>,
    /// The history cannot be read for now, the way a file another program
    /// has open cannot: a read finds none, and holds it until a read does.
    pub unreadable: Cell<bool>,
    /// The last read could not read the history.
    held_by_read: Cell<bool>,
    /// What reads set aside, as they could not read it, not yet told of.
    pub set_aside: RefCell<Vec<SetAside>>,
    /// The sessions deleted by hand.
    pub deleted: RefCell<Vec<Deleted>>,
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
    /// How many times the store was asked to keep its data.
    pub keep_asks: Cell<u32>,
    /// The session or run marked open, as a copy of the app that stopped
    /// leaves it for the next.
    marked: RefCell<Option<OpenSession>>,
    /// Another copy of the app is running the session marked open.
    pub open_elsewhere: Cell<bool>,
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

    fn read_sessions(&self) -> Option<Vec<SessionResult>> {
        self.held_by_read.set(self.unreadable.get());
        if self.unreadable.get() {
            return None;
        }
        self.sessions.borrow().clone()
    }

    fn take_set_aside(&self) -> Vec<SetAside> {
        std::mem::take(&mut self.set_aside.borrow_mut())
    }

    fn read_deleted(&self) -> Vec<Deleted> {
        self.deleted.borrow().clone()
    }

    fn save_deleted(&self, deleted: &[Deleted]) {
        *self.deleted.borrow_mut() = deleted.to_vec();
    }

    fn save_sessions(&self, sessions: &[SessionResult]) -> HistorySaved {
        if self.held.get() || self.held_by_read.get() {
            return HistorySaved::Held;
        }
        let newest = newest(sessions);
        let kept =
            &newest[newest.len() - self.room.get().unwrap_or(usize::MAX).min(newest.len())..];
        *self.sessions.borrow_mut() = Some(kept.to_vec());
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
        saved_now(Ok(saved_as(&format!("{name}, in your downloads"))))
    }

    fn confirms_backups(&self) -> bool {
        !self.unconfirmed_backups.get()
    }

    fn backups_inside_app(&self) -> Option<&'static str> {
        self.backups_inside.get()
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
}

impl MemoryStore {
    /// The session or run marked open, if any.
    pub fn open(&self) -> Option<OpenSession> {
        self.marked.borrow().clone()
    }
}
