//! What the app is: its state, reactive and not, and the moves every screen
//! shares.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use cw_core::{ContestView, GroupSession, SessionResult, SettingsSection, TrainingSettings};
use dioxus::prelude::*;

use crate::contest_runtime::ContestEntry;
use crate::persist::backup::{Backup, Filled, Merged, backup_name, merge_sessions, write_backup};
use crate::persist::{
    CutOff, HistorySaved, MAX_SESSIONS, OpenSession, SessionKey, Store, merge_history,
    merge_settings, pair_sessions,
};
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

/// The guided setup (src/ui/tour.rs): which guide, the step on screen, and
/// what the learner has done since it opened, which the steps on the
/// practice side wait for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tour {
    /// The guide to a contest run, for Contest mode, rather than the one to
    /// a first session and the settings after it.
    pub contest: bool,
    pub step: usize,
    /// A session, or a run, was started.
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

    /// Gate a live sidetone for paddle practice, at the keyer's edge.
    pub fn set_live_tone(&self, edge: cw_core::ToneEdge, settings: &TrainingSettings) {
        self.player.set_live_tone(edge, settings);
    }

    /// Open the sidetone stream ahead of the first squeeze.
    pub fn ensure_live_sidetone(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        self.player.ensure_live_sidetone(settings)
    }

    /// Keep the output awake for the first squeeze while `ready`, or let it
    /// sleep once quiet.
    pub fn keep_output_ready(&self, ready: bool) {
        self.player.keep_ready(ready);
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
    /// What Hear a QSO has played, as far as it has been heard: kept once it
    /// ends, until something else plays or the screen is left.
    pub dialogue: Signal<Option<crate::playback::Dialogue>>,
    /// The open answer box's text, on its way to the session.
    pub answer: AnswerOutbox,
    /// The run's Call and Exchange fields, as typed.
    pub entry: ContestEntry,
    /// The guided setup, while it is open.
    pub tour: Signal<Option<Tour>>,
    /// When the last backup was saved, if ever.
    pub last_backup: Signal<Option<u64>>,
    /// What became of the sessions and runs before, where it was not by
    /// your hand: up until it is put away, or the next one starts.
    pub notices: Signal<Vec<Notice>>,
    /// The sessions deleted last, each with where it sat, for Undo to put
    /// back.
    trash: Signal<Vec<(usize, SessionResult)>>,
    /// What the last restore changed, for Undo to put back.
    before_restore: Signal<Option<Restored>>,
    /// The settings card reset last, and the settings before it, for Undo
    /// to put back.
    before_reset: Signal<Option<(SettingsSection, TrainingSettings)>>,
    /// The settings as they were stored last, so that settings stored
    /// already are not stored again.
    stored_settings: CopyValue<TrainingSettings>,
    /// The history as it was stored last, by this copy or read from the
    /// store: what tells the changes made here from another copy's.
    stored_history: CopyValue<Vec<SessionResult>>,
    /// The settings could not be stored, and the user has been told: not
    /// again until they have been stored since.
    settings_unsaved: CopyValue<bool>,
    app: CopyValue<AppState>,
}

/// Make what `make` makes in the root scope, above the app's own, so that
/// it is owned there. What lasts as long as the app belongs there: the tasks
/// that outlive a screen run there too (`spawn_forever`), and a debug build
/// warns of every read or write of a value above the scope that owns it.
pub fn in_root<T>(make: impl FnOnce() -> T) -> T {
    dioxus::core::Runtime::current().in_scope(ScopeId::ROOT, make)
}

impl AppCtx {
    /// A fresh app around `app`, starting from what was stored. Everything
    /// in it lives as long as the app, so it is made in the root scope.
    pub fn new(app: AppState, settings: TrainingSettings, sessions: Vec<SessionResult>) -> Self {
        in_root(move || {
            let last_backup = app.store.last_backup();
            let stored_settings = CopyValue::new(settings.clone().clamp());
            let stored_history = CopyValue::new(sessions.clone());
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
                dialogue: Signal::new(None),
                answer: AnswerOutbox::new(),
                entry: ContestEntry::new(),
                tour: Signal::new(None),
                last_backup: Signal::new(last_backup),
                notices: Signal::new(Vec::new()),
                trash: Signal::new(Vec::new()),
                before_restore: Signal::new(None),
                before_reset: Signal::new(None),
                stored_settings,
                stored_history,
                settings_unsaved: CopyValue::new(false),
                app: CopyValue::new(app),
            }
        })
    }

    pub fn app(self) -> AppState {
        self.app.cloned()
    }

    /// A session (`run` false) or a contest run now has the app: marked open
    /// until it ends, so that one the app stops under is told of when it
    /// next opens. Whatever the last one left to say is put away.
    pub fn mark_running(mut self, run: bool) {
        self.notices.set(Vec::new());
        // Another window's session may have been cut off since this one
        // opened, while it was still running: told of once this one is over.
        self.tell_of_cut_off();
        self.app().store.mark_open(Some(OpenSession {
            started_at: crate::time::now_ms(),
            run,
        }));
    }

    /// The session or run that had the app is over, kept and stored, or let
    /// go.
    pub fn mark_ended(self) {
        self.app().store.mark_open(None);
    }

    /// Tell of any session or run a copy of the app stopped under, once the
    /// store has looked: at once on a computer or a phone, a moment later
    /// in a browser.
    pub fn tell_of_cut_off(self) {
        let looking = self.app().store.take_cut_off();
        let mut notices = self.notices;
        // In the root scope: the press that starts a session leaves the
        // screen it was made on.
        dioxus::core::spawn_forever(async move {
            if let Some(cut) = looking.await {
                notices.write().push(Notice::CutOff(cut));
            }
        });
    }

    /// Whether a session or a run has the app: on its screen, or about to
    /// be.
    pub fn session_running(self) -> bool {
        matches!(*self.screen.peek(), Screen::Training)
            || self.runtime.peek().is_some()
            || self.contest.peek().is_some()
    }

    /// Store the settings as they are now, clamped, unless they are stored
    /// so already, and give back the settings stored.
    ///
    /// Another tab, window or copy of the app may have stored settings since
    /// this one last read or wrote them. Those are kept: only what changed
    /// here is laid over them, and whatever they changed is taken up here
    /// too, so a copy left open on old settings never puts them back.
    ///
    /// Settings the store would not take are kept here as they are, and
    /// tried again the next time; the user is told once, not on every
    /// change, until they have been stored.
    pub fn store_settings(self) -> TrainingSettings {
        let mut settings = self.settings;
        let ours = settings.peek().clone().clamp();
        let mut stored = self.stored_settings;
        let base = stored.peek().clone();
        let store = self.app().store;
        let elsewhere = store.read_settings().filter(|theirs| *theirs != base);
        let merged = match &elsewhere {
            Some(theirs) => merge_settings(&base, &ours, theirs),
            None => ours.clone(),
        };
        // What the store holds: what it held, unless the merge is written
        // over it. Only that is the base of the next merge, or an edit the
        // store refused would read next time as another copy's change, and
        // be undone by it.
        let mut held = elsewhere.unwrap_or(base);
        let mut refused = None;
        if held != merged {
            match store.write_settings(&merged) {
                Ok(()) => held = merged.clone(),
                Err(why) => refused = Some(why),
            }
        }
        if *stored.peek() != held {
            stored.set(held);
        }
        let mut unsaved = self.settings_unsaved;
        match refused {
            Some(why) if !*unsaved.peek() => {
                unsaved.set(true);
                self.toasts.show(format!(
                    "Your settings could not be saved: {}.",
                    why.trim_end_matches('.')
                ));
            }
            Some(_) => {}
            None if *unsaved.peek() => unsaved.set(false),
            None => {}
        }
        // Only what came from elsewhere: the settings screen leaves its
        // own values as typed until they are left.
        if merged != ours {
            settings.set(merged.clone());
        }
        merged
    }

    /// Stop whatever is playing outside a session.
    pub fn stop_audio(mut self) {
        if self.session_running() {
            return;
        }
        self.app().player.release();
        self.preview.set(Preview::Idle);
    }

    /// Bring the history and the settings here in line with the store's,
    /// writing nothing: whatever another tab, window or copy of the app
    /// stored or deleted since this one last read or wrote them is taken
    /// up, and whatever changed here and is not stored yet is kept on top,
    /// to be stored as it would have been.
    ///
    /// A restore and a backup go by what the store holds, not by what this
    /// copy last saw of it: a restore merged with a history another copy
    /// has since deleted from takes a session it brings back for one
    /// already here, and a backup of an old copy's history leaves out what
    /// was stored since, though it counts as backed up.
    fn sync_with_store(self) {
        let store = self.app().store;
        let (mut sessions, mut stored) = (self.sessions, self.stored_history);
        if let Some(theirs) = store.read_sessions() {
            let merged = merge_history(&stored.peek(), &sessions.peek(), &theirs);
            if *sessions.peek() != merged {
                sessions.set(merged);
            }
            stored.set(theirs);
        }
        let (mut settings, mut stored) = (self.settings, self.stored_settings);
        if let Some(theirs) = store.read_settings()
            && theirs != *stored.peek()
        {
            let merged = merge_settings(&stored.peek(), &settings.peek().clone().clamp(), &theirs);
            if *settings.peek() != merged {
                settings.set(merged);
            }
            stored.set(theirs);
        }
        // A backup another copy saved since is the last one here too, so a
        // restore of an older one never takes the mark back past it.
        let mut last_backup = self.last_backup;
        let marked = store.last_backup();
        if marked > *last_backup.peek() {
            last_backup.set(marked);
        }
    }

    /// Store the history as it is now. What the screens show is what the
    /// store has: when it has room for only the newest, the rest go from
    /// here too.
    ///
    /// Another tab, window or copy of the app may have stored sessions since
    /// this one last read or wrote the history, or deleted some. Those
    /// stand: only what changed here is done to the history stored
    /// ([`merge_history`]), and what is stored is shown here too. A store
    /// that takes nothing leaves the history here as it was.
    pub fn store_history(self) -> HistorySaved {
        self.store_history_counted().saved
    }

    /// As [`Self::store_history`], and what the store held before, and
    /// which of the oldest sessions it let go, for want of room.
    fn store_history_counted(self) -> HistoryStored {
        let mut sessions = self.sessions;
        let mut stored = self.stored_history;
        let store = self.app().store;
        let held = store.read_sessions();
        let merged = match &held {
            Some(theirs) => merge_history(&stored.peek(), &sessions.peek(), theirs),
            // None stored yet, or none that can be read: nothing to keep.
            None => sessions.peek().clone(),
        };
        let held = held.unwrap_or_default();
        let saved = store.save_sessions(&merged);
        let Some(count) = saved.kept() else {
            return HistoryStored {
                saved,
                held,
                let_go: Vec::new(),
            };
        };
        let mut kept = merged;
        let let_go = kept.drain(..kept.len().saturating_sub(count)).collect();
        if *sessions.peek() != kept {
            sessions.set(kept.clone());
        }
        stored.set(kept);
        HistoryStored {
            saved,
            held,
            let_go,
        }
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
    /// remember when, once it is handed over. Saving goes on whichever
    /// screen is up by then.
    ///
    /// A browser says nothing of a download it blocks, or one the user
    /// turns down, so there the backup card tells of the last one as handed
    /// to the browser, not as saved ([`Store::confirms_backups`]). It quiets
    /// the reminder all the same: the hand-over is all a browser ever says
    /// of a download, and a reminder that never goes quiet is not read.
    pub fn save_backup(self) {
        self.sync_with_store();
        let at = crate::time::now_ms();
        let contents = write_backup(&self.settings.peek(), &self.sessions.peek(), at);
        let name = backup_name(&crate::time::local_date_string());
        let saving = self.app().store.save_backup_file(&name, &contents);
        dioxus::core::spawn_forever(async move {
            match saving.await {
                Ok(said) => {
                    self.app().store.mark_backup(at);
                    let mut last_backup = self.last_backup;
                    last_backup.set(Some(at));
                    self.toasts.show(said);
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
        self.sync_with_store();
        let mut sessions = self.sessions;
        let mut settings = self.settings;
        let before = (sessions.peek().clone(), settings.peek().clone());
        let backed_up = covers_up_to(&backup.sessions, &before.0, backup.saved_at);
        let merged = merge_sessions(&before.0, backup.sessions);
        sessions.set(merged.sessions.clone());
        let stored = self.store_history_counted();
        if let Some(why) = why_not_stored(&stored.saved) {
            sessions.set(before.0);
            self.toasts.show(format!("Nothing was restored: {why}."));
            return;
        }
        let let_go = stored.let_go.len();
        let restored = Restored::of(
            &merged,
            &stored.held,
            stored.let_go,
            &sessions.peek(),
            before.1,
        );
        // Stored by the app's settings effect.
        settings.set(backup.settings);
        let store = self.app().store;
        // What the backup holds is backed up, as of when it was saved; but
        // only if that is all there was here by then. A backup from another
        // device leaves this one's own sessions in no backup at all.
        let mut last_backup = self.last_backup;
        if backed_up && backup.saved_at > last_backup.peek().unwrap_or(0) {
            store.mark_backup(backup.saved_at);
            last_backup.set(Some(backup.saved_at));
        }
        store.ask_to_keep();
        let from = day_of(backup.saved_at)
            .filter(|_| backup.saved_at > 0)
            .map(|day| format!(" from {}", short_date(&day)))
            .unwrap_or_default();
        let (added, filled) = (restored.added.len(), restored.filled.len());
        let mut text = match (added, filled) {
            (0, 0) if !restored.brought => {
                format!("Restored your settings{from}. Every session in it was already here.")
            }
            (0, 0) => format!("Restored your settings{from}."),
            (0, _) => format!(
                "Filled in {} and restored your settings{from}.",
                counted(filled, "session")
            ),
            (_, 0) => format!(
                "Restored {} and your settings{from}.",
                counted(added, "session")
            ),
            _ => format!(
                "Restored {} and filled in {filled}, with your settings{from}.",
                counted(added, "session")
            ),
        };
        if let Some(problem) = stored.saved.problem() {
            text = format!("{text} {problem}");
        } else if let_go > 0 {
            let oldest = match let_go {
                1 => "the oldest was".to_string(),
                n => format!("the {n} oldest were"),
            };
            text = format!(
                "{text} The history keeps only the newest {MAX_SESSIONS} sessions, so {oldest} let go."
            );
        }
        let mut before_restore = self.before_restore;
        before_restore.set(Some(restored));
        self.toasts.show_with(text, ToastAction::UndoRestore);
    }

    /// Put the history and settings back as they were before the last
    /// restore: what it added taken out, what it filled in as it was, and
    /// what it let go for want of room back. Whatever else the history took
    /// up since, here or in another copy of the app, stays: the undoing is
    /// done to the history as it is now, and merged with the store's as any
    /// change is.
    pub fn undo_restore(self) {
        let mut before_restore = self.before_restore;
        let Some(restored) = before_restore.write().take() else {
            return;
        };
        let mut sessions = self.sessions;
        let now = sessions.peek().clone();
        sessions.set(restored.undone(&now));
        let saved = self.store_history();
        if let Some(why) = why_not_stored(&saved) {
            sessions.set(now);
            before_restore.set(Some(restored));
            self.toasts
                .show(format!("The restore could not be undone: {why}."));
            return;
        }
        if let Some(problem) = saved.problem() {
            self.toasts.show(problem);
        }
        let mut settings = self.settings;
        settings.set(restored.settings);
    }

    /// Put one settings card back to its defaults, called `card` in what
    /// is said of it. Until another reset, Undo puts the card back as it
    /// was: a card can hold what took typing, your call and exchange or a
    /// list, and one press should not cost it. A card already at its
    /// defaults has nothing to undo, and nothing is said.
    pub fn reset_section(self, section: SettingsSection, card: &str) {
        let mut settings = self.settings;
        let before = settings.peek().clone();
        settings.write().reset_section(section);
        if *settings.peek() == before {
            return;
        }
        let mut before_reset = self.before_reset;
        before_reset.set(Some((section, before)));
        self.toasts.show_with(
            format!("{card} reset to defaults"),
            ToastAction::PutCardBack,
        );
    }

    /// Put the card reset last back as it was before the reset. Every other
    /// card, changed since or not, stays as it is now.
    pub fn undo_reset(self) {
        let mut before_reset = self.before_reset;
        let Some((section, before)) = before_reset.write().take() else {
            return;
        };
        let mut settings = self.settings;
        settings.write().restore_section(section, &before);
    }

    /// Put each of `taken` back where it sat, in the order they sat in; or,
    /// where the history has changed around that place since, where it
    /// falls by when it finished.
    fn put_back(self, taken: Vec<(usize, SessionResult)>) {
        let mut sessions = self.sessions;
        let mut history = sessions.write();
        for (index, session) in taken {
            let at = index.min(history.len());
            let fits = history[..at]
                .last()
                .is_none_or(|before| before.timestamp <= session.timestamp)
                && history
                    .get(at)
                    .is_none_or(|after| session.timestamp <= after.timestamp);
            let at = if fits {
                at
            } else {
                history.partition_point(|kept| kept.timestamp <= session.timestamp)
            };
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
        self.dialogue.set(None);
        self.screen.set(to);
    }
}

/// What a store of the history did ([`AppCtx::store_history_counted`]).
struct HistoryStored {
    saved: HistorySaved,
    /// The history the store held just before, as far as it could be read.
    held: Vec<SessionResult>,
    /// The oldest sessions it let go, for want of room, oldest first.
    let_go: Vec<SessionResult>,
}

/// What a restore changed ([`AppCtx::restore_backup`]), for Undo: no more
/// than it changed, so that undoing it leaves whatever else the history
/// took up, before it or since.
struct Restored {
    /// The sessions it added that the history kept, as it added them.
    added: Vec<SessionResult>,
    /// The sessions it filled in that the history kept.
    filled: Vec<Filled>,
    /// The sessions the history let go to make room for what it added.
    let_go: Vec<SessionResult>,
    /// Whether the backup held anything the history did not, kept or not.
    brought: bool,
    /// The settings before it.
    settings: TrainingSettings,
}

impl Restored {
    /// What a restore that made `merged` of the history changed, now that
    /// the history is stored: it was `held` in the store just before, the
    /// store let go `let_go` of the oldest, and it is `now`.
    ///
    /// A session another copy of the app had stored already, the same one
    /// restored from the same backup, was not added by this restore, nor
    /// was one filled in already: undoing this restore leaves them.
    fn of(
        merged: &Merged,
        held: &[SessionResult],
        let_go: Vec<SessionResult>,
        now: &[SessionResult],
        settings: TrainingSettings,
    ) -> Self {
        let keys = |sessions: &[SessionResult]| -> HashSet<SessionKey> {
            sessions.iter().map(SessionKey::of).collect()
        };
        let (held_keys, kept) = (keys(held), keys(now));
        let added: Vec<&SessionResult> = merged
            .added
            .iter()
            .filter(|session| !held_keys.contains(&SessionKey::of(session)))
            .collect();
        let filled: Vec<&Filled> = merged
            .filled
            .iter()
            .filter(|filled| !held.contains(&filled.now))
            .collect();
        let brought = !added.is_empty() || !filled.is_empty();
        let from_backup: HashSet<SessionKey> = added
            .iter()
            .map(|session| SessionKey::of(session))
            .collect();
        Self {
            added: added
                .into_iter()
                .filter(|session| kept.contains(&SessionKey::of(session)))
                .cloned()
                .collect(),
            filled: filled
                .into_iter()
                .filter(|filled| kept.contains(&SessionKey::of(&filled.now)))
                .cloned()
                .collect(),
            // One the restore filled in comes back as it was before it.
            let_go: let_go
                .into_iter()
                .filter(|session| !from_backup.contains(&SessionKey::of(session)))
                .map(|session| {
                    merged
                        .filled
                        .iter()
                        .find(|filled| filled.now == session)
                        .map_or(session, |filled| filled.was.clone())
                })
                .collect(),
            brought,
            settings,
        }
    }

    /// `now`, the history as it is, with this restore undone: what it
    /// added taken out, what it filled in put back as it was unless it has
    /// changed again since, and what it let go back where it falls by when
    /// it finished.
    fn undone(&self, now: &[SessionResult]) -> Vec<SessionResult> {
        let gone: HashSet<usize> = pair_sessions(
            &now.iter().collect::<Vec<_>>(),
            &self.added.iter().collect::<Vec<_>>(),
        )
        .into_iter()
        .flatten()
        .collect();
        let mut undone: Vec<SessionResult> = now
            .iter()
            .enumerate()
            .filter(|(at, _)| !gone.contains(at))
            .map(|(_, session)| session.clone())
            .collect();
        for filled in &self.filled {
            if let Some(session) = undone.iter_mut().find(|session| **session == filled.now) {
                *session = filled.was.clone();
            }
        }
        for session in &self.let_go {
            let at = undone.partition_point(|kept| kept.timestamp <= session.timestamp);
            undone.insert(at, session.clone());
        }
        undone
    }
}

/// Whether `backup`, saved at `saved_at`, holds every one of `here` that
/// had finished by then: only then is all there was backed up as of it.
fn covers_up_to(backup: &[SessionResult], here: &[SessionResult], saved_at: u64) -> bool {
    let held: HashSet<SessionKey> = backup.iter().map(SessionKey::of).collect();
    here.iter()
        .filter(|session| session.timestamp <= saved_at)
        .all(|session| held.contains(&SessionKey::of(session)))
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

/// What became of a session or run, where it was not by your hand
/// ([`AppCtx::notices`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// The app stopped with it running: closed, crashed, killed or rebuilt.
    /// It was not kept.
    CutOff(CutOff),
    /// The history could not take it, or let older sessions go to make
    /// room: the store's own words.
    History(String),
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
    /// Put the settings card reset last back as it was.
    PutCardBack,
}

impl ToastAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::UndoDelete | Self::UndoRestore | Self::PutCardBack => "Undo",
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
    use super::*;
    use crate::persist::memory::MemoryStore;

    /// A copy of the app — a tab, or a window — on `store`, which other
    /// copies may share, opened on what it holds.
    struct Tab {
        dom: VirtualDom,
        ctx: AppCtx,
    }

    impl Tab {
        fn on(store: &Rc<MemoryStore>) -> Self {
            let dom = VirtualDom::new(|| rsx! {});
            let app = AppState::with(crate::audio::fake::Recorder::new().factory(), store.clone());
            let ctx = dom.in_scope(ScopeId::ROOT, || {
                AppCtx::new(app, store.load_settings(), store.load_sessions())
            });
            Self { dom, ctx }
        }

        /// Do `act` in this copy, the way one of its callbacks does.
        fn act<T>(&self, act: impl FnOnce(AppCtx) -> T) -> T {
            self.dom.in_scope(ScopeId::ROOT, || act(self.ctx))
        }

        fn toast(&self) -> Option<Toast> {
            self.act(|ctx| ctx.toasts.current())
        }

        /// Keep `session`, as a session finished here is kept.
        fn finish(&self, session: SessionResult) {
            self.act(|ctx| crate::session_runtime::keep_and_show(ctx, session));
        }

        /// Delete the session that finished on `day`.
        fn delete(&self, day: u64) {
            self.act(|ctx| {
                let index = ctx
                    .sessions
                    .peek()
                    .iter()
                    .position(|session| session.timestamp == stamp(day))
                    .expect("shown");
                ctx.delete_sessions(&[SessionPick {
                    index,
                    timestamp: stamp(day),
                }]);
            });
        }

        /// The days of the sessions this copy shows.
        fn shown(&self) -> Vec<u64> {
            self.act(|ctx| days(&ctx.sessions.peek()))
        }
    }

    fn stamp(day: u64) -> u64 {
        day * 86_400_000
    }

    /// A session that finished on `day`, as the store keeps it.
    fn day(day: u64) -> SessionResult {
        SessionResult {
            timestamp: stamp(day),
            date: format!("2026-10-{day:02}"),
            ..SessionResult::empty_for_test()
        }
    }

    fn days(sessions: &[SessionResult]) -> Vec<u64> {
        sessions
            .iter()
            .map(|session| session.timestamp / stamp(1))
            .collect()
    }

    /// A store already holding a session for each of `held`.
    fn holding(held: impl IntoIterator<Item = u64>) -> Rc<MemoryStore> {
        let store = Rc::new(MemoryStore::default());
        store.save_sessions(&held.into_iter().map(day).collect::<Vec<_>>());
        store
    }

    /// Two tabs on one store each keep a session, and neither loses the
    /// other's. Each wrote back the whole history it held, so the one that
    /// stored last left out what the other had kept since it opened.
    #[test]
    fn a_session_kept_in_another_tab_is_not_lost() {
        let store = holding([1]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(2));
        a.finish(day(3));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        assert_eq!(a.shown(), [1, 2, 3], "taken up as it stored");
        b.finish(day(4));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4]);
        assert_eq!(b.shown(), [1, 2, 3, 4]);
    }

    /// A session deleted in one tab stays deleted when another stores, and
    /// one deleted in each goes from both. One deleted used to come back the
    /// next time a tab that still held it stored.
    #[test]
    fn a_session_deleted_in_another_tab_stays_deleted() {
        let store = holding([1, 2, 3, 4]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.delete(2);
        a.delete(4);
        assert_eq!(days(&store.load_sessions()), [1, 3]);
        assert_eq!(a.shown(), [1, 3]);
        a.finish(day(5));
        b.finish(day(6));
        assert_eq!(days(&store.load_sessions()), [1, 3, 5, 6]);
        assert_eq!(b.shown(), [1, 3, 5, 6]);
    }

    /// Undo puts back only what was deleted here, where it falls among
    /// what another tab kept since.
    #[test]
    fn undo_puts_a_session_back_among_another_tabs() {
        let store = holding([5, 6, 7]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        a.delete(6);
        b.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: 0,
                settings: TrainingSettings::default(),
                sessions: vec![day(1), day(2)],
            })
        });
        b.finish(day(8));
        // Taken up here: two sessions now sit where the sixth did.
        a.finish(day(9));
        assert_eq!(a.shown(), [1, 2, 5, 7, 8, 9]);
        a.act(AppCtx::undo_delete);
        assert_eq!(days(&store.load_sessions()), [1, 2, 5, 6, 7, 8, 9]);
        assert_eq!(a.shown(), [1, 2, 5, 6, 7, 8, 9]);
    }

    /// A restore, and its Undo, leave alone what another tab kept between
    /// them.
    #[test]
    fn undoing_a_restore_keeps_what_another_tab_kept() {
        let store = holding([3]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        a.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: 0,
                settings: TrainingSettings::default(),
                sessions: vec![day(1), day(2)],
            })
        });
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        b.finish(day(4));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4]);
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [3, 4]);
        assert_eq!(a.shown(), [3, 4]);
    }

    /// A restore goes by what the store holds: a session another tab
    /// deleted since this one last saw the history is restored, not taken
    /// for one already here. It used to be found here, so not added, and
    /// then dropped again as deleted elsewhere, with "Every session in it
    /// was already here."
    #[test]
    fn a_restore_brings_back_what_another_tab_deleted() {
        let store = holding([1, 2, 3]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.delete(2);
        restore(&a, [1, 2, 3]);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        assert_eq!(a.shown(), [1, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 1 session and your settings."
        );
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [1, 3]);
    }

    /// A restore takes the backup's settings whole, over what another tab
    /// stored since: a value the backup shares with this tab's stale copy
    /// used to read as unchanged here, and the other tab's stood.
    #[test]
    fn a_restore_takes_its_settings_over_another_tabs() {
        let store = holding([1]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.act(|ctx| {
            let mut settings = ctx.settings;
            settings.write().curriculum.num_groups = 9;
            ctx.store_settings();
        });
        let backup = TrainingSettings::default();
        a.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: 0,
                settings: backup.clone(),
                sessions: Vec::new(),
            });
            ctx.store_settings();
        });
        let stored = store.load_settings();
        assert_eq!(
            stored.curriculum.num_groups,
            backup.clamp().curriculum.num_groups
        );
    }

    /// A backup from another device, newer than every session here but
    /// holding none of them, does not count them as backed up; one holding
    /// all of them does. The reminder used to go quiet over sessions in no
    /// backup at all.
    #[test]
    fn a_restore_marks_a_backup_only_over_what_it_holds() {
        let store = holding(1..=12);
        let a = Tab::on(&store);
        let from_elsewhere = |sessions: Vec<SessionResult>| Backup {
            saved_at: stamp(20),
            settings: TrainingSettings::default(),
            sessions,
        };
        a.act(|ctx| ctx.restore_backup(from_elsewhere(vec![day(15)])));
        assert_eq!(store.last_backup(), None);
        assert_eq!(a.act(|ctx| *ctx.last_backup.peek()), None);
        // The 15 the first restore brought is here now too.
        let all = (1..=12).chain([15]).map(day).collect();
        a.act(|ctx| ctx.restore_backup(from_elsewhere(all)));
        assert_eq!(store.last_backup(), Some(stamp(20)));
    }

    /// A backup holds what the store holds, sessions another tab finished
    /// since this one last read the history among them. It used to hold
    /// only what this tab had seen, while counting the rest as backed up.
    #[test]
    fn a_backup_holds_what_another_tab_stored() {
        let store = holding([1]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(2));
        b.finish(day(3));
        a.act(AppCtx::save_backup);
        let files = store.backup_files.borrow().clone();
        let (_, contents) = files.last().expect("a backup file");
        let backup = crate::persist::backup::read_backup(contents).expect("a backup");
        assert_eq!(days(&backup.sessions), [1, 2, 3]);
        assert_eq!(a.shown(), [1, 2, 3]);
    }

    /// A restore of a backup older than one another tab saved since leaves
    /// the newer mark. The restore compared the backup's date with this
    /// tab's stale mark, and took the store's back to the older one.
    #[test]
    fn a_restore_leaves_a_newer_backup_mark_from_another_tab() {
        let store = holding([1, 2]);
        let a = Tab::on(&store);
        store.mark_backup(stamp(30));
        a.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: stamp(20),
                settings: TrainingSettings::default(),
                sessions: vec![day(1), day(2)],
            })
        });
        assert_eq!(store.last_backup(), Some(stamp(30)));
        assert_eq!(a.act(|ctx| *ctx.last_backup.peek()), Some(stamp(30)));
    }

    /// Undoing a restore says so when the store had room for only the
    /// newest of what it puts back, as undoing a delete does.
    #[test]
    fn undoing_a_restore_says_what_did_not_fit() {
        let store = holding([3, 4]);
        let a = Tab::on(&store);
        restore(&a, [1, 2]);
        store.room.set(Some(1));
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [4]);
        let told = a.toast().expect("told").text;
        assert!(told.contains("newest"), "{told}");
    }

    /// Restore the backup holding a session for each of `days`, in `tab`.
    fn restore(tab: &Tab, days: impl IntoIterator<Item = u64>) {
        let sessions = days.into_iter().map(day).collect();
        tab.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: 0,
                settings: TrainingSettings::default(),
                sessions,
            })
        });
    }

    /// A session another tab kept before the restore, which this tab had
    /// not seen yet, stays when the restore is undone. Undo used to take
    /// out everything the history held after the restore and not before
    /// it, and the other tab's session was among that.
    #[test]
    fn undoing_a_restore_keeps_what_another_tab_kept_before_it() {
        let store = holding([3]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(4));
        restore(&a, [1, 2]);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4]);
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [3, 4]);
        assert_eq!(a.shown(), [3, 4]);
        b.finish(day(5));
        assert_eq!(days(&store.load_sessions()), [3, 4, 5]);
    }

    /// A session another tab deleted before the restore stays deleted when
    /// it is undone. Undo used to put back everything the history held
    /// before the restore, and the deleted session was among that.
    #[test]
    fn undoing_a_restore_does_not_bring_back_what_another_tab_deleted() {
        let store = holding([3, 4]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.delete(4);
        restore(&a, [1, 2]);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [3]);
        assert_eq!(a.shown(), [3]);
        b.finish(day(5));
        assert_eq!(days(&store.load_sessions()), [3, 5]);
    }

    /// Another tab restored the same backup first: the sessions were in
    /// the store already, so this tab's restore added none of them, and
    /// undoing it takes none out. Undoing the first takes them out.
    #[test]
    fn undoing_a_restore_leaves_what_another_tabs_restore_added() {
        let store = holding([3]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        restore(&b, [1, 2]);
        restore(&a, [1, 2]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored your settings. Every session in it was already here."
        );
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        b.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [3]);
    }

    /// A restore whose sessions the history let go again, for want of
    /// room, takes out on Undo only those it kept, and leaves what another
    /// tab kept since. One that let go the history's own oldest sessions
    /// to make room puts them back.
    #[test]
    fn undoing_a_restore_the_history_trimmed() {
        let max = MAX_SESSIONS as u64;
        // Five of the ten restored fit, and five go again.
        let store = holding(11..=max + 5);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        restore(&a, 1..=10);
        assert_eq!(
            days(&store.load_sessions()),
            (6..=max + 5).collect::<Vec<_>>()
        );
        b.finish(day(max + 6));
        assert_eq!(
            days(&store.load_sessions()),
            (7..=max + 6).collect::<Vec<_>>()
        );
        a.act(AppCtx::undo_restore);
        let stored = days(&store.load_sessions());
        assert_eq!(stored, (11..=max + 6).collect::<Vec<_>>());
        assert_eq!(a.shown(), stored);

        // A full history, and a backup of newer sessions: the oldest five
        // went to make room, and come back.
        let store = holding(1..=max);
        let a = Tab::on(&store);
        restore(&a, max + 1..=max + 5);
        assert_eq!(
            days(&store.load_sessions()),
            (6..=max + 5).collect::<Vec<_>>()
        );
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), (1..=max).collect::<Vec<_>>());
    }

    /// A session a restore filled in goes back as it was on Undo, among
    /// whatever another tab did meanwhile; one filled in already by another
    /// tab's restore stays filled in.
    #[test]
    fn undoing_a_restore_puts_back_what_it_filled_in() {
        let run = |log: bool| {
            let mut run =
                crate::testing::stored_run("2026-10-02", &[("K5XYZ", "214", "K5XYZ", "214")]);
            run.timestamp = stamp(2);
            if !log {
                run.contest = None;
            }
            run
        };
        let restore_run = |tab: &Tab| {
            tab.act(|ctx| {
                ctx.restore_backup(Backup {
                    saved_at: 0,
                    settings: TrainingSettings::default(),
                    sessions: vec![run(true)],
                })
            })
        };
        let store = Rc::new(MemoryStore::default());
        store.save_sessions(&[day(1), run(false)]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(3));
        restore_run(&a);
        assert!(store.load_sessions()[1].contest.is_some(), "filled in");
        a.act(AppCtx::undo_restore);
        assert_eq!(store.load_sessions(), [day(1), run(false), day(3)]);

        restore_run(&b);
        restore_run(&a);
        a.act(AppCtx::undo_restore);
        assert_eq!(store.load_sessions(), [day(1), run(true), day(3)]);
    }

    /// A store that holds its history back, as one it could not read is,
    /// takes nothing and changes nothing here; what was kept meanwhile is
    /// stored once it takes the history again.
    #[test]
    fn a_held_history_takes_nothing_until_it_is_let_go() {
        let store = holding([1, 2]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(3));
        store.held.set(true);
        a.finish(day(4));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        assert_eq!(a.shown(), [1, 2, 4], "nothing taken up either");
        store.held.set(false);
        a.finish(day(5));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4, 5]);
        assert_eq!(a.shown(), [1, 2, 3, 4, 5]);
    }

    /// A history that could not be read for a moment holds back only the
    /// sessions kept meanwhile, and only until it reads again: then they
    /// are stored, beside what another tab kept in the meantime.
    #[test]
    fn a_history_that_could_not_be_read_for_a_moment_holds_only_that_moment() {
        let store = holding([1]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        store.unreadable.set(true);
        a.finish(day(2));
        assert_eq!(
            a.act(|ctx| ctx.notices.peek().clone()),
            [Notice::History(HistorySaved::Held.problem().expect("told"))]
        );
        store.unreadable.set(false);
        b.finish(day(3));
        assert_eq!(days(&store.load_sessions()), [1, 3]);
        a.finish(day(4));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4]);
        assert_eq!(a.shown(), [1, 2, 3, 4]);
    }

    /// A restore that takes the history past the newest it keeps counts
    /// only the sessions that stayed, and says the oldest were let go. It
    /// used to count every session it added, though the oldest went again
    /// as they were stored.
    #[test]
    fn a_restore_counts_only_the_sessions_the_history_kept() {
        let max = MAX_SESSIONS as u64;
        let restore = |tab: &Tab| {
            tab.act(|ctx| {
                ctx.restore_backup(Backup {
                    saved_at: 0,
                    settings: TrainingSettings::default(),
                    sessions: (1..=10).map(day).collect(),
                })
            });
            tab.toast().expect("told").text
        };

        let store = holding(11..=max + 5);
        let tab = Tab::on(&store);
        assert_eq!(
            restore(&tab),
            format!(
                "Restored 5 sessions and your settings. The history keeps only the newest {max} sessions, so the 5 oldest were let go."
            )
        );
        let stored = days(&store.load_sessions());
        assert_eq!(stored, (6..=max + 5).collect::<Vec<_>>());
        assert_eq!(tab.shown(), stored);

        // Every one of them older than the newest kept: none restored.
        let store = holding(11..=max + 10);
        let tab = Tab::on(&store);
        assert_eq!(
            restore(&tab),
            format!(
                "Restored your settings. The history keeps only the newest {max} sessions, so the 10 oldest were let go."
            )
        );
        assert_eq!(store.load_sessions().len(), MAX_SESSIONS);
    }

    /// Between them, tabs keep the newest sessions the history has room for.
    #[test]
    fn tabs_keep_the_newest_sessions_between_them() {
        let max = MAX_SESSIONS as u64;
        let store = holding(1..=max);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(max + 1));
        a.finish(day(max + 2));
        let stored = days(&store.load_sessions());
        assert_eq!(stored, (3..=max + 2).collect::<Vec<_>>());
        assert_eq!(a.shown(), stored);
    }

    /// Settings the store would not take stay as they were set, and are
    /// stored once it takes them again. The edit used to be taken as
    /// stored all the same, so the old settings still in the store read as
    /// another copy's change, and the next store put them back.
    #[test]
    fn settings_the_store_refused_are_kept_and_told_of_once() {
        let store = Rc::new(MemoryStore::default());
        let old = TrainingSettings::default();
        store.save_settings(&old);
        let copy = Tab::on(&store);
        *store.settings_refused.borrow_mut() = Some("the disk is full".into());

        copy.act(|ctx| {
            let mut settings = ctx.settings;
            settings.write().curriculum.num_groups = 9;
            ctx.store_settings();
        });
        let told = copy.toast().expect("told");
        assert_eq!(
            told.text,
            "Your settings could not be saved: the disk is full."
        );
        assert_eq!(store.load_settings(), old, "nothing was written");

        // Every edit after it is refused too, and not told of again.
        copy.act(|ctx| {
            let mut settings = ctx.settings;
            settings.write().band.steepness = 12.0;
            ctx.store_settings();
        });
        assert_eq!(copy.toast(), Some(told.clone()), "told once");

        // The store takes them again: both edits are stored, neither put
        // back.
        store.settings_refused.borrow_mut().take();
        let stored = copy.act(AppCtx::store_settings);
        assert_eq!(stored.curriculum.num_groups, 9);
        assert_eq!(stored.band.steepness, 12.0);
        assert_eq!(store.load_settings(), stored);
        assert_eq!(copy.act(|ctx| ctx.settings.peek().clone()), stored);

        // Stored since, so a store that refuses again is told of again.
        *store.settings_refused.borrow_mut() = Some("the disk is full".into());
        copy.act(|ctx| {
            let mut settings = ctx.settings;
            settings.write().curriculum.num_groups = 4;
            ctx.store_settings();
        });
        assert_ne!(copy.toast(), Some(told), "told again");
        assert_eq!(store.load_settings().curriculum.num_groups, 9);
    }

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
