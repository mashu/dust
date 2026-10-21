//! What the app is: its state, reactive and not, and the moves every screen
//! shares.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use cw_core::{ContestView, GroupSession, SessionResult, TrainingSettings};
use dioxus::prelude::*;

use crate::contest_runtime::ContestEntry;
use crate::persist::backup::{Backup, backup_name, merge_sessions, write_backup};
use crate::persist::{HistorySaved, Store};
use crate::playback::Preview;
use crate::player::{AudioError, BackendFactory, Player};
use crate::session_runtime::{AnswerOutbox, Live};
use crate::ui::format::{counted, day_of, short_date};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Settings,
    Training,
    Results,
    Stats,
    Listen,
}

impl Screen {
    /// The screen's name in the page, which also keys its wrapper so that
    /// moving between screens mounts each afresh.
    pub fn key(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Settings => "settings",
            Self::Training => "training",
            Self::Results => "results",
            Self::Stats => "stats",
            Self::Listen => "listen",
        }
    }
}

/// The guided setup (src/ui/tour.rs): the step on screen, and what the
/// learner has done since it opened, which the steps on the practice side
/// wait for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tour {
    pub step: usize,
    /// A session was started.
    pub started: bool,
    /// One of its groups was answered.
    pub answered: bool,
    /// And it was left again: discarded, or ended.
    pub left: bool,
}

/// What the app holds outside the reactive tree: the audio player, the
/// session or run going on it, and where what it learns is kept. Cheap to
/// clone; every clone is the same app.
#[derive(Clone)]
pub struct AppState {
    pub player: Player,
    pub store: Rc<dyn Store>,
    pub session: Rc<RefCell<Option<Live>>>,
}

impl AppState {
    /// The app as it ships: this build's audio, this build's store.
    pub fn new() -> Self {
        Self::with(
            Rc::new(crate::audio::default_backend),
            crate::persist::default_store(),
        )
    }

    pub fn with(make_player: BackendFactory, store: Rc<dyn Store>) -> Self {
        Self {
            player: Player::new(make_player),
            store,
            session: Rc::new(RefCell::new(None)),
        }
    }

    /// Gate a live sidetone for paddle practice.
    pub fn set_live_tone(&self, on: bool, settings: &TrainingSettings) {
        self.player.set_live_tone(on, settings);
    }

    /// Open the sidetone stream ahead of the first squeeze.
    pub fn ensure_live_sidetone(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        self.player.ensure_live_sidetone(settings)
    }
}

/// One session of the history, as a screen names it to delete it: where it
/// sits, and when it finished, so that a list drawn before the history
/// changed can never take the wrong one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionPick {
    pub index: usize,
    pub timestamp: u64,
}

/// Everything a screen can read, and the moves every screen shares. Copy, so
/// each screen takes it from context and each callback carries its own.
///
/// A screen subscribes to the signals it reads and to nothing else, so the
/// shell above it redraws for the shell's own state only.
#[derive(Clone, Copy)]
pub struct AppCtx {
    pub screen: Signal<Screen>,
    pub settings: Signal<TrainingSettings>,
    pub sessions: Signal<Vec<SessionResult>>,
    /// The session in flight, as its screen shows it.
    pub runtime: Signal<Option<GroupSession>>,
    /// The contest run in flight, as its screen shows it.
    pub contest: Signal<Option<ContestView>>,
    /// What the run last said for a screen reader, once the air fell
    /// silent: logged and how it checked, corrected, paused, time up.
    pub announced: Signal<Option<Said>>,
    /// How the session just finished ended, for the results screen.
    pub result: Signal<Option<Finished>>,
    /// What the session just finished did to the level, if anything.
    pub auto_message: Signal<Option<String>>,
    pub toasts: Toasts,
    /// Whatever is playing outside a session.
    pub preview: Signal<Preview>,
    /// The groups the stream has sent, newest last.
    pub stream_heard: Signal<Vec<crate::playback::HeardGroup>>,
    /// The open answer box's text, on its way to the session.
    pub answer: AnswerOutbox,
    /// The run's Call and Exchange fields, as typed.
    pub entry: ContestEntry,
    /// The guided setup, while it is open.
    pub tour: Signal<Option<Tour>>,
    /// When the last backup was saved, if ever.
    pub last_backup: Signal<Option<u64>>,
    /// The sessions deleted last, each with where it sat, for Undo to put
    /// back.
    trash: Signal<Vec<(usize, SessionResult)>>,
    /// The history and settings as they stood before the last restore, for
    /// Undo to put back.
    before_restore: Signal<Option<(Vec<SessionResult>, TrainingSettings)>>,
    app: CopyValue<AppState>,
}

impl AppCtx {
    /// A fresh app around `app`, starting from what was stored. Made in the
    /// scope that owns the app, as everything in it lives as long as that.
    pub fn new(app: AppState, settings: TrainingSettings, sessions: Vec<SessionResult>) -> Self {
        let last_backup = app.store.last_backup();
        Self {
            screen: Signal::new(Screen::Home),
            settings: Signal::new(settings),
            sessions: Signal::new(sessions),
            runtime: Signal::new(None),
            contest: Signal::new(None),
            announced: Signal::new(None),
            result: Signal::new(None),
            auto_message: Signal::new(None),
            toasts: Toasts::new(),
            preview: Signal::new(Preview::Idle),
            stream_heard: Signal::new(Vec::new()),
            answer: AnswerOutbox::new(),
            entry: ContestEntry::new(),
            tour: Signal::new(None),
            last_backup: Signal::new(last_backup),
            trash: Signal::new(Vec::new()),
            before_restore: Signal::new(None),
            app: CopyValue::new(app),
        }
    }

    pub fn app(self) -> AppState {
        self.app.cloned()
    }

    /// Whether a session or a run has the app: on its screen, or about to
    /// be.
    pub fn session_running(self) -> bool {
        matches!(*self.screen.peek(), Screen::Training)
            || self.runtime.peek().is_some()
            || self.contest.peek().is_some()
    }

    /// Stop whatever is playing outside a session.
    pub fn stop_audio(mut self) {
        if self.session_running() {
            return;
        }
        self.app().player.release();
        self.preview.set(Preview::Idle);
    }

    /// Store the history as it is now. What the screens show is what the
    /// store has: when it has room for only the newest, the rest go from
    /// here too.
    pub fn store_history(self) -> HistorySaved {
        let mut sessions = self.sessions;
        let saved = self.app().store.save_sessions(&sessions.peek());
        if let Some(kept) = saved.kept() {
            let extra = sessions.peek().len().saturating_sub(kept);
            if extra > 0 {
                sessions.write().drain(..extra);
            }
        }
        saved
    }

    /// Take `picked` out of the history, and out of the store. Until another
    /// deletion, Undo puts them back. A pick that no longer names the
    /// session it was made for is passed over, and a store that cannot take
    /// the change leaves the history as it was and says why.
    pub fn delete_sessions(self, picked: &[SessionPick]) {
        let mut picked = picked.to_vec();
        picked.sort_unstable_by_key(|pick| pick.index);
        picked.dedup_by_key(|pick| pick.index);
        let mut taken = Vec::new();
        {
            let mut sessions = self.sessions;
            let mut history = sessions.write();
            // From the back, so the places still to come stay where they are.
            for pick in picked.iter().rev() {
                if history
                    .get(pick.index)
                    .is_some_and(|session| session.timestamp == pick.timestamp)
                {
                    taken.push((pick.index, history.remove(pick.index)));
                }
            }
        }
        if taken.is_empty() {
            return;
        }
        taken.reverse();
        let count = taken.len();
        let saved = self.store_history();
        if let Some(why) = why_not_stored(&saved) {
            self.put_back(taken);
            self.toasts.show(format!("Nothing was deleted: {why}."));
            return;
        }
        let mut trash = self.trash;
        trash.set(taken);
        match saved.problem() {
            Some(problem) => self.toasts.show(problem),
            None => {
                self.toasts.show_with(
                    format!("{} deleted", counted(count, "session")),
                    ToastAction::UndoDelete,
                );
            }
        }
    }

    /// Put back the sessions deleted last, and store them again.
    pub fn undo_delete(self) {
        let mut trash = self.trash;
        let taken = std::mem::take(&mut *trash.write());
        if taken.is_empty() {
            return;
        }
        let mut sessions = self.sessions;
        let before = sessions.peek().clone();
        self.put_back(taken.clone());
        let saved = self.store_history();
        if let Some(why) = why_not_stored(&saved) {
            sessions.set(before);
            trash.set(taken);
            self.toasts
                .show(format!("The sessions could not be put back: {why}."));
        } else if let Some(problem) = saved.problem() {
            self.toasts.show(problem);
        }
    }

    /// Hand the history and the settings to the user as a backup file, and
    /// remember when, once it is saved. Saving goes on whichever screen is
    /// up by then.
    pub fn save_backup(self) {
        let at = crate::time::now_ms();
        let contents = write_backup(&self.settings.peek(), &self.sessions.peek(), at);
        let name = backup_name(&crate::time::local_date_string());
        let saving = self.app().store.save_backup_file(&name, &contents);
        dioxus::core::spawn_forever(async move {
            match saving.await {
                Ok(place) => {
                    self.app().store.mark_backup(at);
                    let mut last_backup = self.last_backup;
                    last_backup.set(Some(at));
                    self.toasts.show(format!("Backup saved as {place}."));
                }
                Err(why) => self
                    .toasts
                    .show(format!("The backup could not be saved: {why}.")),
            }
        });
    }

    /// Put back what `backup` holds: every session of it not already here,
    /// and its settings, level and all. Until another restore, Undo puts
    /// both back as they were. A store that cannot take the history leaves
    /// everything as it was and says why.
    pub fn restore_backup(self, backup: Backup) {
        let mut sessions = self.sessions;
        let mut settings = self.settings;
        let before = (sessions.peek().clone(), settings.peek().clone());
        let (merged, added) = merge_sessions(&before.0, backup.sessions);
        sessions.set(merged);
        let saved = self.store_history();
        if let Some(why) = why_not_stored(&saved) {
            sessions.set(before.0);
            self.toasts.show(format!("Nothing was restored: {why}."));
            return;
        }
        // Stored by the app's settings effect.
        settings.set(backup.settings);
        let store = self.app().store;
        // What the backup holds is backed up, as of when it was saved.
        let mut last_backup = self.last_backup;
        if backup.saved_at > last_backup.peek().unwrap_or(0) {
            store.mark_backup(backup.saved_at);
            last_backup.set(Some(backup.saved_at));
        }
        store.ask_to_keep();
        let mut before_restore = self.before_restore;
        before_restore.set(Some(before));
        let from = day_of(backup.saved_at)
            .filter(|_| backup.saved_at > 0)
            .map(|day| format!(" from {}", short_date(&day)))
            .unwrap_or_default();
        let mut text = if added > 0 {
            format!(
                "Restored {} and your settings{from}.",
                counted(added, "session")
            )
        } else {
            format!("Restored your settings{from}. Every session in it was already here.")
        };
        if let Some(problem) = saved.problem() {
            text = format!("{text} {problem}");
        }
        self.toasts.show_with(text, ToastAction::UndoRestore);
    }

    /// Put the history and settings back as they were before the last
    /// restore.
    pub fn undo_restore(self) {
        let mut before_restore = self.before_restore;
        let Some((before, settings_before)) = before_restore.write().take() else {
            return;
        };
        let mut sessions = self.sessions;
        let restored = sessions.peek().clone();
        sessions.set(before.clone());
        let saved = self.store_history();
        if let Some(why) = why_not_stored(&saved) {
            sessions.set(restored);
            before_restore.set(Some((before, settings_before)));
            self.toasts
                .show(format!("The restore could not be undone: {why}."));
            return;
        }
        let mut settings = self.settings;
        settings.set(settings_before);
    }

    /// Put each of `taken` back where it sat, in the order they sat in.
    fn put_back(self, taken: Vec<(usize, SessionResult)>) {
        let mut sessions = self.sessions;
        let mut history = sessions.write();
        for (index, session) in taken {
            let at = index.min(history.len());
            history.insert(at, session);
        }
    }

    /// Leave for another screen. A session in progress holds you; otherwise
    /// whatever was sounding stops before the screen changes, wherever you
    /// are going.
    pub fn go_to(mut self, to: Screen) {
        if self.session_running() {
            return;
        }
        self.app().player.release();
        self.preview.set(Preview::Idle);
        self.stream_heard.set(Vec::new());
        self.screen.set(to);
    }
}

/// Why a history was not stored at all, in words that fit after "Nothing was
/// deleted:". `None` when it was stored.
fn why_not_stored(saved: &HistorySaved) -> Option<String> {
    match saved {
        HistorySaved::Stored { .. } | HistorySaved::Squeezed { .. } => None,
        HistorySaved::Held => {
            Some("your saved history could not be read, so it was left as it is".into())
        }
        HistorySaved::Failed(reason) => Some(reason.clone()),
    }
}

/// How the session just finished ended, as its results screen shows it.
/// One value, so the screen can never show both.
#[derive(Clone, Debug, PartialEq)]
pub enum Finished {
    Kept(Box<SessionResult>),
    /// A run that worked nobody, so nothing was kept.
    NothingKept,
}

#[cfg(test)]
impl Finished {
    /// The session kept, if one was.
    pub fn kept(self) -> Option<SessionResult> {
        match self {
            Self::Kept(result) => Some(*result),
            Self::NothingKept => None,
        }
    }
}

/// Something said to a screen reader.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub text: String,
    /// Which time this was said. The same words said twice are said twice:
    /// a screen reader speaks what is put in its region anew, not what is
    /// left there unchanged.
    pub serial: u64,
}

impl Said {
    pub fn new(text: String) -> Self {
        static SAID: AtomicU64 = AtomicU64::new(0);
        Self {
            text,
            serial: SAID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// How long a message stays up.
pub const TOAST_MS: u32 = 4_000;
/// How long a message that offers to undo something stays up: long enough
/// to read it and change your mind.
pub const ACTION_TOAST_MS: u32 = 8_000;

/// Something a message offers to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastAction {
    /// Put back the sessions just deleted.
    UndoDelete,
    /// Put the history and settings back as they were before a restore.
    UndoRestore,
}

impl ToastAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::UndoDelete | Self::UndoRestore => "Undo",
        }
    }
}

/// A message at the foot of the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub action: Option<ToastAction>,
    /// Which raise this was. The same words raised twice are two toasts,
    /// each up for its own full time.
    serial: u64,
}

impl Toast {
    /// How long this one stays up.
    pub fn lasts_ms(&self) -> u32 {
        if self.action.is_some() {
            ACTION_TOAST_MS
        } else {
            TOAST_MS
        }
    }
}

/// Where messages are raised and shown: one at a time, the newest in place
/// of whatever was up.
#[derive(Clone, Copy, PartialEq)]
pub struct Toasts(Signal<Option<Toast>>);

impl Toasts {
    /// Messages for the scope this is made in.
    pub fn new() -> Self {
        Self(Signal::new(None))
    }

    pub fn show(self, text: impl Into<String>) {
        self.raise(text.into(), None);
    }

    /// A message with something to do about it.
    pub fn show_with(self, text: impl Into<String>, action: ToastAction) {
        self.raise(text.into(), Some(action));
    }

    fn raise(self, text: String, action: Option<ToastAction>) {
        static RAISED: AtomicU64 = AtomicU64::new(0);
        let mut shown = self.0;
        shown.set(Some(Toast {
            text,
            action,
            serial: RAISED.fetch_add(1, Ordering::Relaxed),
        }));
    }

    /// The message up now. Read from a component, it subscribes it.
    pub fn current(self) -> Option<Toast> {
        self.0.cloned()
    }

    /// Take `toast` down, unless a newer one has taken its place.
    pub fn dismiss(self, toast: &Toast) {
        let mut shown = self.0;
        if shown.peek().as_ref() == Some(toast) {
            shown.set(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Screen;

    #[test]
    fn every_screen_has_its_own_key() {
        let keys: Vec<&str> = [
            Screen::Home,
            Screen::Settings,
            Screen::Training,
            Screen::Results,
            Screen::Stats,
            Screen::Listen,
        ]
        .into_iter()
        .map(Screen::key)
        .collect();
        // The key remounts the screen wrapper, so it has to differ per screen.
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len());
    }
}
