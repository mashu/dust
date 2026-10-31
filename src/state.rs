//! What the app is: its state, reactive and not, and the moves every screen
//! shares.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use cw_core::{
    ContestView, GroupSession, Sampler, SamplerPrior, SessionResult, SettingsSection,
    TrainingSettings,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use crate::contest_runtime::ContestEntry;
use crate::history::{StoredHistory, settle_or_spawn};
use crate::persist::backup::{Backup, Filled, backup_name, merge_sessions, write_backup};
use crate::persist::record::{HistoryChange, HistoryLoad, Record, RecordId, read_record};
use crate::persist::{
    CutOff, Deleted, HistorySaved, OpenSession, SessionKey, SetAside, SoFar, Store, merge_settings,
};
use crate::playback::Preview;
use crate::player::{AudioError, BackendFactory, Player};
use crate::session_runtime::{AnswerOutbox, Live, keep_cut_off};
use crate::time::{POLL_MS, now_ms, sleep_ms};
use crate::ui::format::{counted, day_of, short_date};

/// How long after the store last refused what the app has yet to store it
/// is tried again, while anything waits: the page coming back into sight,
/// or the next change, try it sooner.
pub const RETRY_MS: u32 = 60_000;

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
    /// The prior the last sampler started from, and the key of the history
    /// it was built from ([`history_key`]): the next one starts from it too
    /// while both read as they did ([`AppCtx::sampler`]).
    prior: Rc<RefCell<Option<(u64, SamplerPrior)>>>,
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
            prior: Rc::new(RefCell::new(None)),
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

/// One session of the history, as a screen opens it: where it sits, and when
/// it finished, so that a list drawn before the history changed never opens
/// the wrong one. A session is deleted by its record ([`RecordId`]), which
/// stays with it however the history changes around it.
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
    /// The history, every session by its record ([`StoredHistory`]).
    pub sessions: Signal<StoredHistory>,
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
    /// The sessions deleted last, each by its record, for Undo to put back
    /// exactly where it was.
    trash: Signal<Vec<(RecordId, SessionResult)>>,
    /// What the last restore changed, for Undo to put back.
    before_restore: Signal<Option<Restored>>,
    /// The settings card reset last, and the settings before it, for Undo
    /// to put back.
    before_reset: Signal<Option<(SettingsSection, TrainingSettings)>>,
    /// The learner's own words the starter words took the place of last,
    /// for Undo to put back.
    before_starter_words: Signal<Option<Vec<String>>>,
    /// The settings as they were stored last, so that settings stored
    /// already are not stored again.
    stored_settings: CopyValue<TrainingSettings>,
    /// This copy's mark, as it was written last, while it holds one
    /// ([`Store::mark_open`]).
    mark: CopyValue<Option<OpenSession>>,
    /// A try at storing what the store has yet to take is waiting
    /// ([`RETRY_MS`]).
    retrying: CopyValue<bool>,
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
    /// A fresh app around `app`, starting from what was stored: the
    /// settings, and the history as the store read it. Everything in it
    /// lives as long as the app, so it is made in the root scope.
    pub fn new(app: AppState, settings: TrainingSettings, history: HistoryLoad) -> Self {
        in_root(move || {
            let last_backup = backup_mark(&*app.store);
            // What reading the store at launch set aside, and what it found
            // of the history, told of at once.
            let notices: Vec<Notice> = app
                .store
                .take_set_aside()
                .into_iter()
                .chain(history.set_aside.iter().cloned())
                .map(Notice::SetAside)
                .chain(
                    history
                        .notices
                        .iter()
                        .map(|notice| Notice::History(notice.text())),
                )
                .collect();
            let stored_settings = CopyValue::new(settings.clone().clamp());
            Self {
                screen: Signal::new(Screen::Home),
                settings: Signal::new(settings),
                sessions: Signal::new(StoredHistory::from_load(history)),
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
                notices: Signal::new(notices),
                trash: Signal::new(Vec::new()),
                before_restore: Signal::new(None),
                before_reset: Signal::new(None),
                before_starter_words: Signal::new(None),
                stored_settings,
                mark: CopyValue::new(None),
                retrying: CopyValue::new(false),
                settings_unsaved: CopyValue::new(false),
                app: CopyValue::new(app),
            }
        })
    }

    pub fn app(self) -> AppState {
        self.app.cloned()
    }

    /// A session (`run` false) or a contest run now has the app: marked open
    /// until it ends and is stored, so that one the app stops under is told
    /// of when it next opens. Whatever the last one left to say is put away.
    /// The mark is handed back, for what the session does to be added to it
    /// ([`Self::mark_so_far`]); it carries too whatever the store has yet to
    /// take of the sessions before ([`OpenSession::unstored`]).
    pub fn mark_running(mut self, run: bool) -> OpenSession {
        self.notices.set(Vec::new());
        // Another window's session may have been cut off since this one
        // opened, while it was still running: told of once this one is over.
        self.tell_of_cut_off();
        let open = OpenSession::starting(run, now_ms());
        self.write_mark(open.clone());
        open
    }

    /// The session or run marked `open` would keep `so_far` were it ended
    /// now: held with its mark, as the history is stored only once it ends,
    /// so that the app stopping under it keeps it all the same.
    pub fn mark_so_far(self, open: &OpenSession, so_far: SoFar) {
        let held = self
            .mark
            .peek()
            .clone()
            .filter(|held| held.started_at == open.started_at);
        let mut marked = held.unwrap_or_else(|| open.clone());
        marked.so_far = Some(Box::new(so_far));
        self.write_mark(marked);
    }

    /// The session or run that had the app is over, let go: nothing of it
    /// is kept. Its mark goes, but for what it carries of the sessions
    /// before that the store has yet to take.
    pub fn mark_ended(self) {
        let Some(mut open) = self.mark.peek().clone() else {
            return;
        };
        open.so_far = None;
        open.over = true;
        self.write_mark(open);
    }

    /// The session or run that had the app is over, and kept as its mark
    /// holds it: the mark stays as it is until the store has taken it
    /// ([`Self::settle_mark`]). Should the app stop first, or the store have
    /// no room for it, the next launch keeps it from the mark.
    pub fn mark_kept(self) {
        let mut mark = self.mark;
        if let Some(open) = mark.write().as_mut() {
            open.over = true;
        }
    }

    /// Write this copy's mark as `open` says, carrying every session the
    /// store has yet to take but the one it holds so far; or, once its
    /// session or run is over and nothing is left to carry, take it away.
    fn write_mark(self, mut open: OpenSession) {
        let own = open
            .so_far
            .as_ref()
            .map(|so_far| SessionKey::of(&so_far.result));
        open.unstored = self
            .sessions
            .peek()
            .unsaved_sessions()
            .filter(|session| own.as_ref() != Some(&SessionKey::of(session)))
            .cloned()
            .collect();
        let store = self.app().store;
        let mut mark = self.mark;
        if open.over && open.so_far.is_none() && open.unstored.is_empty() {
            store.mark_open(None);
            mark.set(None);
        } else {
            store.mark_open(Some(open.clone()));
            mark.set(Some(open));
        }
    }

    /// Bring this copy's mark in line with what the store has taken, after
    /// a change to the history: a session over and stored, or deleted, is
    /// held by its mark no more, a mark left with nothing to carry goes, and
    /// one short of a session the store has yet to take carries it, should
    /// the app stop before the store takes it. With no mark held, one is
    /// made for that: a session cut off and kept, say, whose own mark was
    /// taken as it was told of.
    fn settle_mark(self) {
        let waiting: HashSet<SessionKey> = self
            .sessions
            .peek()
            .unsaved_sessions()
            .map(SessionKey::of)
            .collect();
        let held = self.mark.peek().clone();
        let Some(mut open) = held else {
            if !waiting.is_empty() {
                self.write_mark(OpenSession {
                    over: true,
                    ..OpenSession::starting(false, now_ms())
                });
            }
            return;
        };
        let mut carried: HashSet<SessionKey> = open.unstored.iter().map(SessionKey::of).collect();
        if !open.over {
            // A session running writes its mark as it goes; only one it
            // does not carry yet is worth a write of its own.
            if !waiting.is_subset(&carried) {
                self.write_mark(open);
            }
            return;
        }
        let own = open
            .so_far
            .as_ref()
            .map(|so_far| SessionKey::of(&so_far.result));
        let mut dropped = false;
        if let Some(own) = own {
            if waiting.contains(&own) {
                carried.insert(own);
            } else {
                open.so_far = None;
                dropped = true;
            }
        }
        if dropped || carried != waiting {
            self.write_mark(open);
        }
    }

    /// Tell of any session or run a copy of the app stopped under, once the
    /// store has looked: at once on a computer or a phone, a moment later
    /// in a browser. What each had done by then is kept, as Exit would have
    /// kept it, and what each carried that the store had yet to take.
    pub fn tell_of_cut_off(self) {
        // This copy's own mark, which it holds still, is no other copy's
        // cut off, whatever a store says of it.
        let own = self.mark.peek().as_ref().map(|open| open.started_at);
        let looking = self.app().store.take_cut_off();
        let mut notices = self.notices;
        // In the root scope: the press that starts a session leaves the
        // screen it was made on.
        dioxus::core::spawn_forever(async move {
            let mut cut = looking.await;
            cut.retain(|cut| !matches!(cut, CutOff::Marked(open) if Some(open.started_at) == own));
            if let Some(newest) = CutOff::newest(cut.iter().cloned()) {
                notices.write().push(Notice::CutOff(newest));
            }
            keep_cut_off(self, cut).await;
        });
    }

    /// Tell of whatever reading the store has set aside since it was last
    /// asked, as it could not read it: once each, up until it is put away.
    fn tell_of_set_aside(self) {
        let set_aside = self.app().store.take_set_aside();
        if !set_aside.is_empty() {
            let mut notices = self.notices;
            notices
                .write()
                .extend(set_aside.into_iter().map(Notice::SetAside));
        }
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
        self.tell_of_set_aside();
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

    /// A sampler for a session, a run or a stream of `settings`, seeded
    /// with `seed`: what [`Sampler::new`] makes of the history, from the
    /// prior built last while the history and what the prior reads of the
    /// settings are as they were then. Otherwise the history is replayed
    /// here, which [`Self::warm_sampler`] does ahead of a tap.
    pub fn sampler(self, settings: &TrainingSettings, seed: u64) -> Sampler {
        Sampler::from_prior(self.prior(settings), settings, seed)
    }

    /// The prior a sampler for `settings` starts from: the one kept, while it
    /// is still the history's, or one built from the history now and kept.
    fn prior(self, settings: &TrainingSettings) -> SamplerPrior {
        let sessions = self.sessions.peek();
        let key = history_key(&sessions);
        let kept = self.app().prior;
        if let Some((built_from, prior)) = kept.borrow().as_ref()
            && *built_from == key
            && prior.suits(settings)
        {
            return prior.clone();
        }
        #[cfg(test)]
        crate::testing::rendered("SamplerPrior");
        let prior = SamplerPrior::from_history(&sessions, settings);
        *kept.borrow_mut() = Some((key, prior.clone()));
        prior
    }

    /// Build the prior the next sampler starts from, once the screen up now
    /// has been drawn, so that the tap that starts a session, a run or a
    /// stream need not replay the history: on a phone that was a pause
    /// between the tap and anything happening. A prior kept already that is
    /// still the history's costs a look at every session, and no more.
    pub fn warm_sampler(self) {
        // The app's: the screen it is warmed for may be left meanwhile.
        spawn_forever(async move {
            sleep_ms(POLL_MS).await;
            let settings = self.settings.peek().clone().clamp();
            self.prior(&settings);
        });
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
    /// then do `then`: at once wherever the store answers at once, as every
    /// store but a browser's database does.
    ///
    /// Whatever another tab, window or copy of the app stored or deleted
    /// since this one last looked is taken up: each record listed that is
    /// not here is read and put in its place, and each here that is no
    /// longer listed goes, if it was deleted by hand. One missing for any
    /// other reason is stored again: missing is never deleted. What is here
    /// and not stored yet is left as it is, to be stored. The settings are
    /// merged as [`Self::store_settings`] merges them, writing nothing.
    ///
    /// A restore and a backup go by what the store holds, not by what this
    /// copy last saw of it: a restore merged with a history another copy
    /// has since deleted from takes a session it brings back for one
    /// already here, and a backup of an old copy's history leaves out what
    /// was stored since, though it counts as backed up.
    fn sync_with_store(self, then: impl FnOnce() + 'static) {
        let store = self.app().store;
        let settings_read = store.read_settings();
        self.tell_of_set_aside();
        let (mut settings, mut stored) = (self.settings, self.stored_settings);
        if let Some(theirs) = settings_read
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
        let marked = backup_mark(&*store);
        if marked > *last_backup.peek() {
            last_backup.set(marked);
        }
        let (here, unread, unsaved) = {
            let history = self.sessions.peek();
            let here: BTreeSet<RecordId> = history.ids().iter().copied().collect();
            (here, history.unread().clone(), history.unsaved().clone())
        };
        let looking = Box::pin(async move {
            let there: BTreeSet<RecordId> = store.history_listing().await?.into_iter().collect();
            let new: Vec<RecordId> = there
                .difference(&here)
                .filter(|id| !unread.contains(id))
                .copied()
                .collect();
            let missing: Vec<RecordId> = here
                .difference(&there)
                .filter(|id| !unsaved.contains(id))
                .copied()
                .collect();
            let read = if new.is_empty() {
                Vec::new()
            } else {
                store.read_records(new).await
            };
            let forgotten = if missing.is_empty() {
                Vec::new()
            } else {
                store.forgotten(missing.clone()).await
            };
            Some(Looked {
                read,
                missing,
                forgotten,
            })
        });
        settle_or_spawn(looking, move |looked| {
            if let Some(looked) = looked {
                self.take_up(looked);
            }
            then();
        });
    }

    /// Take up what a look at the store found ([`Self::sync_with_store`]).
    fn take_up(self, looked: Looked) {
        let Looked {
            read,
            missing,
            forgotten,
        } = looked;
        let forgotten: HashSet<RecordId> = forgotten.into_iter().collect();
        let mut sessions = self.sessions;
        let changes = {
            let history = sessions.peek();
            read.iter().any(|record| match record {
                Ok(record) => history.index_of(record.id).is_none(),
                Err(_) => false,
            }) || missing
                .iter()
                .any(|id| history.index_of(*id).is_some() && !history.unsaved().contains(id))
        };
        if !changes {
            return;
        }
        let mut lost = false;
        {
            let mut history = sessions.write();
            let mut arrived = Vec::new();
            for record in read.into_iter().flatten() {
                match read_record(&record.json) {
                    Some(session) => arrived.push((record.id, session)),
                    None => history.mark_unread(record.id),
                }
            }
            for id in missing {
                if history.unsaved().contains(&id) {
                    continue;
                }
                if forgotten.contains(&id) {
                    history.remove(id);
                    continue;
                }
                let Some(here) = history.get(id) else {
                    continue;
                };
                // The same session listed under another name, as a browser's
                // page storage may name one anew: taken up by that name,
                // not stored a second time.
                if arrived.iter().any(|(_, session)| session == here) {
                    history.remove(id);
                } else {
                    // Lost in passing: stored again.
                    history.mark_unsaved(id);
                    lost = true;
                }
            }
            for (id, session) in arrived {
                history.insert(id, session);
            }
        }
        if lost {
            self.store_unsaved();
        }
    }

    /// Take up whatever another tab, window or copy of the app stored or
    /// deleted since this one last looked, where the store says anything
    /// may have changed: as a screen that shows the history opens, and as
    /// the page comes back into sight.
    pub fn take_up_changes(self) {
        if self.app().store.history_changed() {
            self.sync_with_store(|| {});
        }
    }

    /// Make `change` to the history in the store, with every session here
    /// it has yet to take riding along, and do `then` with what became of
    /// it, once the store says: at once wherever it can.
    ///
    /// What the change stored is stored here; what the store let go for
    /// want of room, as only a browser's page storage does, goes from here
    /// too; and the mark follows what the store has taken
    /// ([`Self::settle_mark`]). Whatever is still to be stored is tried
    /// again with the next change, as the page comes back into sight, and
    /// every [`RETRY_MS`] until it is.
    fn change_history(self, mut change: HistoryChange, then: impl FnOnce(HistorySaved) + 'static) {
        {
            let history = self.sessions.peek();
            for &id in history.unsaved() {
                if change.put.iter().all(|record| record.id != id)
                    && let Some(record) = history.record(id)
                {
                    change.put.push(record);
                }
            }
        }
        let put: Vec<RecordId> = change.put.iter().map(|record| record.id).collect();
        let pending = self.app().store.change_history(change);
        settle_or_spawn(pending, move |saved| {
            then(saved.clone());
            self.took(&put, &saved);
            let held = self.mark.peek().clone();
            match held {
                // Not stored: a mark held for a session over says so now,
                // should the app stop before the store takes it.
                Some(open) if open.over && saved.kept().is_none() => self.write_mark(open),
                _ => self.settle_mark(),
            }
            self.tell_of_set_aside();
            if !self.sessions.peek().unsaved().is_empty() {
                self.retry_later();
            }
        });
    }

    /// What the store took of a change that stored `put`: each is stored
    /// here too, and what the store let go for want of room goes from here
    /// as well, rather than be stored again with every change.
    fn took(self, put: &[RecordId], saved: &HistorySaved) {
        if saved.kept().is_none() {
            return;
        }
        let trimmed: &[RecordId] = match saved {
            HistorySaved::Squeezed { trimmed, .. } => trimmed,
            _ => &[],
        };
        let mut sessions = self.sessions;
        let changes = {
            let history = sessions.peek();
            put.iter().any(|id| history.unsaved().contains(id))
                || trimmed.iter().any(|id| history.index_of(*id).is_some())
        };
        if !changes {
            return;
        }
        let mut history = sessions.write();
        for id in put {
            history.mark_saved(*id);
        }
        for id in trimmed {
            history.remove(*id);
        }
    }

    /// Store whatever the store has yet to take, if anything.
    pub fn store_unsaved(self) {
        if !self.sessions.peek().unsaved().is_empty() {
            self.change_history(HistoryChange::default(), |_| {});
        }
    }

    /// Try again, in [`RETRY_MS`], to store what the store has yet to take:
    /// one try waiting at a time.
    fn retry_later(self) {
        let mut retrying = self.retrying;
        if *retrying.peek() {
            return;
        }
        retrying.set(true);
        spawn_forever(async move {
            sleep_ms(RETRY_MS).await;
            retrying.set(false);
            self.store_unsaved();
        });
    }

    /// Add `session`, just finished here, to the history, after every
    /// session in it, and store it, then do `then` with what became of it.
    /// It is kept here until the store has taken it, and carried by this
    /// copy's mark meanwhile.
    pub fn keep_finished(self, session: SessionResult, then: impl FnOnce(HistorySaved) + 'static) {
        let mut sessions = self.sessions;
        let newest = sessions.peek().newest_id();
        let Some(record) = Record::of(&session, |json| {
            RecordId::next(session.timestamp, newest, json)
        }) else {
            then(HistorySaved::Failed("it could not be written down.".into()));
            return;
        };
        {
            let mut history = sessions.write();
            history.insert(record.id, session);
            history.mark_unsaved(record.id);
        }
        self.change_history(
            HistoryChange {
                put: vec![record],
                ..HistoryChange::default()
            },
            then,
        );
    }

    /// Add `kept`, sessions from elsewhere, each as it is to be stored and
    /// where it finished, to the history, held with this copy's mark until
    /// the store has taken them, and hand back those it took, for
    /// [`Self::store_held`] to store. One the history holds by now, by when
    /// and on what day it finished, is not taken again.
    pub fn hold_from_elsewhere(self, kept: Vec<(Record, SessionResult)>) -> Vec<Record> {
        let mut sessions = self.sessions;
        let mut held = Vec::new();
        {
            let mut history = sessions.write();
            for (record, session) in kept {
                if history.has_key(&SessionKey::of(&session)) {
                    continue;
                }
                history.insert(record.id, session);
                history.mark_unsaved(record.id);
                held.push(record);
            }
        }
        if !held.is_empty() {
            // The marks they came from are gone: this copy's holds them.
            self.settle_mark();
        }
        held
    }

    /// Store `held`, sessions held here until the store takes them
    /// ([`Self::hold_from_elsewhere`]), then do `then` with what became of
    /// them.
    pub fn store_held(self, held: Vec<Record>, then: impl FnOnce(HistorySaved) + 'static) {
        self.change_history(
            HistoryChange {
                put: held,
                ..HistoryChange::default()
            },
            then,
        );
    }

    /// Take the sessions of `picked` out of the history, and out of the
    /// store. Until another deletion, Undo puts them back. A record no
    /// longer here is passed over, and a store that cannot take the change
    /// leaves the history as it was and says why.
    pub fn delete_sessions(self, picked: &[RecordId]) {
        let mut sessions = self.sessions;
        let present: Vec<RecordId> = {
            let history = sessions.peek();
            let mut present: Vec<RecordId> = picked
                .iter()
                .copied()
                .filter(|id| history.index_of(*id).is_some())
                .collect();
            present.sort_unstable();
            present.dedup();
            present
        };
        if present.is_empty() {
            return;
        }
        let at = now_ms();
        // Each, and whether the store had yet to take it.
        let taken: Vec<(RecordId, SessionResult, bool)> = {
            let mut history = sessions.write();
            present
                .into_iter()
                .filter_map(|id| {
                    let unsaved = history.unsaved().contains(&id);
                    Some((id, history.remove(id)?, unsaved))
                })
                .collect()
        };
        let change = HistoryChange {
            forget: taken
                .iter()
                .map(|(id, session, _)| (*id, Deleted::of(session, at)))
                .collect(),
            ..HistoryChange::default()
        };
        let (toasts, mut trash) = (self.toasts, self.trash);
        self.change_history(change, move |saved| {
            if let Some(why) = why_not_stored(&saved) {
                let mut history = sessions.write();
                for (id, session, unsaved) in taken {
                    history.insert(id, session);
                    if unsaved {
                        history.mark_unsaved(id);
                    }
                }
                toasts.show(format!("Nothing was deleted: {why}."));
                return;
            }
            let count = taken.len();
            trash.set(
                taken
                    .into_iter()
                    .map(|(id, session, _)| (id, session))
                    .collect(),
            );
            match saved.problem() {
                Some(problem) => toasts.show(problem),
                None => toasts.show_with(
                    format!("{} deleted", counted(count, "session")),
                    ToastAction::UndoDelete,
                ),
            }
        });
    }

    /// Put back the sessions deleted last, each exactly where it was, under
    /// the record it had, and store them again.
    pub fn undo_delete(self) {
        let mut trash = self.trash;
        let taken = std::mem::take(&mut *trash.write());
        if taken.is_empty() {
            return;
        }
        let mut sessions = self.sessions;
        let mut change = HistoryChange::default();
        {
            let mut history = sessions.write();
            for (id, session) in &taken {
                if let Ok(json) = serde_json::to_string(session) {
                    change.put.push(Record { id: *id, json });
                    change.unforget.push(*id);
                }
                history.insert(*id, session.clone());
            }
        }
        let toasts = self.toasts;
        self.change_history(change, move |saved| {
            if let Some(why) = why_not_stored(&saved) {
                {
                    let mut history = sessions.write();
                    for (id, _) in &taken {
                        history.remove(*id);
                    }
                }
                trash.set(taken);
                toasts.show(format!("The sessions could not be put back: {why}."));
            } else if let Some(problem) = saved.problem() {
                toasts.show(problem);
            }
        });
    }

    /// Hand the history and the settings to the user as a backup file, and
    /// remember when, once it is handed over. Saving goes on whichever
    /// screen is up by then.
    ///
    /// A browser says nothing of a download it blocks, or one the user
    /// turns down, so there the backup card tells of the last one as handed
    /// to the browser, not as saved ([`Store::confirms_backups`]). It quiets
    /// the reminder all the same: the hand-over is all a browser ever says
    /// of a download, and a reminder that never goes quiet is not read. So
    /// does a file saved inside the app, as on an iPhone, which the message
    /// and the card say goes with the app until it is moved out
    /// ([`Store::backups_inside_app`]).
    ///
    /// It holds every session here, those the store has yet to take too.
    pub fn save_backup(self) {
        self.sync_with_store(move || self.save_backup_now());
    }

    fn save_backup_now(self) {
        let at = now_ms();
        let contents = write_backup(&self.settings.peek(), &self.sessions.peek(), at);
        let name = backup_name(&crate::time::local_date_string());
        let saving = self.app().store.save_backup_file(&name, &contents);
        dioxus::core::spawn_forever(async move {
            match saving.await {
                Ok(said) => {
                    let store = self.app().store;
                    store.mark_backup(at);
                    let mut last_backup = self.last_backup;
                    last_backup.set(Some(at));
                    // Kept inside the app, it is no backup until moved out.
                    let said = match store.backups_inside_app() {
                        Some(how) => format!(
                            "{said} It goes if Dust is deleted: move it out {how} to keep it."
                        ),
                        None => said,
                    };
                    self.toasts.show(said);
                }
                Err(why) => self
                    .toasts
                    .show(format!("The backup could not be saved: {why}.")),
            }
        });
    }

    /// Put back what `backup` holds: every session of it not already here,
    /// and its settings, level and all, where it holds any, but for this
    /// device's own ([`TrainingSettings::keep_device_settings`]). Until
    /// another restore, Undo puts both back as they were. A store that
    /// cannot take the history leaves everything as it was and says why.
    ///
    /// Each session it adds is a record where it finished
    /// ([`RecordId::placed`]), so two copies restoring one backup store it
    /// once; one it fills in is a record in its place
    /// ([`RecordId::beside`]), the one it fills in deleted.
    pub fn restore_backup(self, backup: Backup) {
        self.sync_with_store(move || self.restore_now(backup));
    }

    fn restore_now(self, backup: Backup) {
        // As of when it was saved, or now, if a clock set ahead says later.
        let saved_at = backup.saved_at.min(now_ms());
        let at = now_ms();
        let (backed_up, merged) = {
            let history = self.sessions.peek();
            (
                covers_up_to(&backup.sessions, &history, saved_at),
                merge_sessions(&history, backup.sessions),
            )
        };
        // What it adds was deleted by hand, maybe; it is wanted back now.
        let mut change = HistoryChange {
            restore: true,
            ..HistoryChange::default()
        };
        let mut added = Vec::new();
        for session in merged.added {
            let Some(record) =
                Record::of(&session, |json| RecordId::placed(session.timestamp, json))
            else {
                continue;
            };
            change.unforget.push(record.id);
            added.push((record.id, session));
            change.put.push(record);
        }
        let mut filled = Vec::new();
        {
            let history = self.sessions.peek();
            for Filled { was, now } in merged.filled {
                let Some(was_id) = history
                    .ids_of(&SessionKey::of(&was))
                    .iter()
                    .copied()
                    .find(|id| history.get(*id) == Some(&was))
                else {
                    continue;
                };
                let Some(record) = Record::of(&now, |json| RecordId::beside(was_id, json)) else {
                    continue;
                };
                change.unforget.push(record.id);
                change.forget.push((was_id, Deleted::of(&was, at)));
                filled.push(FilledIn {
                    id: record.id,
                    now,
                    was_id,
                    was,
                });
                change.put.push(record);
            }
        }
        let brought = !added.is_empty() || !filled.is_empty();
        let restored_at = backup.saved_at;
        let restored_settings = backup.settings;
        self.change_history(change, move |saved| {
            self.restored(
                saved,
                Restoring {
                    added,
                    filled,
                    brought,
                    backed_up,
                    saved_at,
                    restored_at,
                    settings: restored_settings,
                },
            );
        });
    }

    /// What a restore comes to, once the store has said what became of it
    /// ([`Self::restore_backup`]).
    fn restored(self, saved: HistorySaved, restoring: Restoring) {
        let Restoring {
            added,
            filled,
            brought,
            backed_up,
            saved_at,
            restored_at,
            settings: backup_settings,
        } = restoring;
        if let Some(why) = why_not_stored(&saved) {
            self.toasts.show(format!("Nothing was restored: {why}."));
            return;
        }
        // What the store let go again at once, for want of room, is not
        // here, nor restored.
        let trimmed: HashSet<RecordId> = match &saved {
            HistorySaved::Squeezed { trimmed, .. } => trimmed.iter().copied().collect(),
            _ => HashSet::new(),
        };
        let added: Vec<(RecordId, SessionResult)> = added
            .into_iter()
            .filter(|(id, _)| !trimmed.contains(id))
            .collect();
        let filled: Vec<FilledIn> = filled
            .into_iter()
            .filter(|fill| !trimmed.contains(&fill.id))
            .collect();
        {
            let mut sessions = self.sessions;
            let mut history = sessions.write();
            for (id, session) in &added {
                history.insert(*id, session.clone());
            }
            for fill in &filled {
                history.remove(fill.was_id);
                history.insert(fill.id, fill.now.clone());
            }
        }
        // Stored by the app's settings effect; this device's own kept.
        let mut settings = self.settings;
        let before = backup_settings.as_ref().map(|_| settings.peek().clone());
        let with_settings = backup_settings.is_some();
        let mut kept_device = false;
        if let Some(mut restored) = backup_settings {
            kept_device = restored.keep_device_settings(&settings.peek());
            settings.set(restored);
        }
        let store = self.app().store;
        // What the backup holds is backed up, as of when it was saved; but
        // only if that is all there was here by then. A backup from another
        // device leaves this one's own sessions in no backup at all.
        let mut last_backup = self.last_backup;
        if backed_up && saved_at > last_backup.peek().unwrap_or(0) {
            store.mark_backup(saved_at);
            last_backup.set(Some(saved_at));
        }
        store.ask_to_keep();
        let from = day_of(restored_at)
            .filter(|_| restored_at > 0)
            .map(|day| format!(" from {}", short_date(&day)))
            .unwrap_or_default();
        let from = if kept_device {
            format!("{from}, keeping this device's volume and paddle keys")
        } else {
            from
        };
        let (added_count, filled_count) = (added.len(), filled.len());
        let mut text = match (added_count, filled_count, with_settings) {
            (0, 0, true) if !brought => {
                format!("Restored your settings{from}. Every session in it was already here.")
            }
            (0, 0, false) if !brought => "Every session in it was already here.".into(),
            (0, 0, true) => format!("Restored your settings{from}."),
            (0, 0, false) => format!("Restored no sessions{from}."),
            (0, _, true) => format!(
                "Filled in {} and restored your settings{from}.",
                counted(filled_count, "session")
            ),
            (0, _, false) => format!("Filled in {}{from}.", counted(filled_count, "session")),
            (_, 0, true) => format!(
                "Restored {} and your settings{from}.",
                counted(added_count, "session")
            ),
            (_, 0, false) => format!("Restored {}{from}.", counted(added_count, "session")),
            (_, _, true) => format!(
                "Restored {} and filled in {filled_count}, with your settings{from}.",
                counted(added_count, "session")
            ),
            (_, _, false) => format!(
                "Restored {} and filled in {filled_count}{from}.",
                counted(added_count, "session")
            ),
        };
        if let Some(problem) = saved.problem() {
            text = format!("{text} {problem}");
        }
        let mut before_restore = self.before_restore;
        before_restore.set(Some(Restored {
            added,
            filled,
            settings: before,
        }));
        self.toasts.show_with(text, ToastAction::UndoRestore);
    }

    /// Put the history and settings back as they were before the last
    /// restore: what it added deleted, and what it filled in as it was,
    /// under the record it had. Whatever else the history took up since,
    /// here or in another copy of the app, stays: only what the restore
    /// changed, and is here still, is undone.
    pub fn undo_restore(self) {
        let mut before_restore = self.before_restore;
        let Some(restored) = before_restore.write().take() else {
            return;
        };
        let at = now_ms();
        let mut change = HistoryChange::default();
        let (mut gone, mut back) = (Vec::new(), Vec::new());
        {
            let history = self.sessions.peek();
            for (id, session) in &restored.added {
                if history.index_of(*id).is_some() {
                    change.forget.push((*id, Deleted::of(session, at)));
                    gone.push(*id);
                }
            }
            for fill in &restored.filled {
                let Ok(json) = serde_json::to_string(&fill.was) else {
                    continue;
                };
                if history.index_of(fill.id).is_none() {
                    continue;
                }
                change.forget.push((fill.id, Deleted::of(&fill.now, at)));
                change.put.push(Record {
                    id: fill.was_id,
                    json,
                });
                change.unforget.push(fill.was_id);
                gone.push(fill.id);
                back.push((fill.was_id, fill.was.clone()));
            }
        }
        let (mut sessions, toasts) = (self.sessions, self.toasts);
        self.change_history(change, move |saved| {
            if let Some(why) = why_not_stored(&saved) {
                before_restore.set(Some(restored));
                toasts.show(format!("The restore could not be undone: {why}."));
                return;
            }
            {
                let mut history = sessions.write();
                for id in gone {
                    history.remove(id);
                }
                for (id, session) in back {
                    history.insert(id, session);
                }
            }
            if let Some(problem) = saved.problem() {
                toasts.show(problem);
            }
            if let Some(before) = restored.settings {
                let mut settings = self.settings;
                settings.set(before);
            }
        });
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

    /// Send the starter words in place of the learner's own, which Undo
    /// puts back: a list can be long, pasted in from elsewhere, and one
    /// press should not cost it. With none of their own there is nothing to
    /// undo, and nothing is said.
    pub fn starter_words(self) {
        let mut settings = self.settings;
        if !settings.peek().has_own_words() {
            return;
        }
        let words = std::mem::take(&mut settings.write().curriculum.word_list);
        let mut before = self.before_starter_words;
        before.set(Some(words));
        self.toasts
            .show_with("Back to the starter words", ToastAction::PutWordsBack);
    }

    /// Put back the words the starter words took the place of last.
    pub fn undo_starter_words(self) {
        let mut before = self.before_starter_words;
        let Some(words) = before.write().take() else {
            return;
        };
        let mut settings = self.settings;
        settings.write().curriculum.word_list = words;
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

/// A key to the history as a sampler reads it: every session in its place,
/// by its [`SessionKey`], how many groups it holds and its accuracy. Whatever
/// stores, deletes, puts back or restores sessions, here or in another copy
/// of the app, changes it, and it costs a look at each session rather than
/// a replay of them.
fn history_key(sessions: &[SessionResult]) -> u64 {
    let mut hasher = DefaultHasher::new();
    sessions.len().hash(&mut hasher);
    for session in sessions {
        session.timestamp.hash(&mut hasher);
        session.date.hash(&mut hasher);
        session.groups.len().hash(&mut hasher);
        session.accuracy.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

/// When the last backup was saved, as `store` remembers it, but never
/// later than now. A mark a clock set ahead left is later than every session
/// finished once the clock is put right, and the reminder said nothing new
/// since until the real time caught up with it, a year on maybe: such a
/// mark is taken as made now, and remembered so.
fn backup_mark(store: &dyn Store) -> Option<u64> {
    let marked = store.last_backup()?;
    let now = crate::time::now_ms();
    if marked > now {
        store.mark_backup(now);
        return Some(now);
    }
    Some(marked)
}

/// What a look at the store found ([`AppCtx::sync_with_store`]).
struct Looked {
    /// Each record listed that was not here, as it is stored, or its id
    /// where it could not be read.
    read: Vec<Result<Record, RecordId>>,
    /// Each record here, the store had taken, that was no longer listed.
    missing: Vec<RecordId>,
    /// Of those, the ones deleted by hand.
    forgotten: Vec<RecordId>,
}

/// A restore on its way to the store ([`AppCtx::restored`]).
struct Restoring {
    added: Vec<(RecordId, SessionResult)>,
    filled: Vec<FilledIn>,
    /// Whether the backup held anything the history did not.
    brought: bool,
    /// Whether it holds everything that had finished here by then.
    backed_up: bool,
    /// When it was saved, as the backup mark counts it.
    saved_at: u64,
    /// When it says it was saved, as it is told of.
    restored_at: u64,
    settings: Option<TrainingSettings>,
}

/// What a restore changed ([`AppCtx::restore_backup`]), for Undo: no more
/// than it changed, so that undoing it leaves whatever else the history
/// took up, before it or since.
struct Restored {
    /// The sessions it added, by record.
    added: Vec<(RecordId, SessionResult)>,
    /// The sessions it filled in.
    filled: Vec<FilledIn>,
    /// The settings before it, where it took the backup's.
    settings: Option<TrainingSettings>,
}

/// A session a restore filled in: the record it is now, and the record it
/// was, as it was.
struct FilledIn {
    id: RecordId,
    now: SessionResult,
    was_id: RecordId,
    was: SessionResult,
}

/// Whether `backup`, saved at `saved_at`, holds every one of `here` that
/// had finished by then: only then is all there was backed up as of it.
fn covers_up_to(backup: &[SessionResult], here: &[SessionResult], saved_at: u64) -> bool {
    let held: HashSet<SessionKey> = backup.iter().map(SessionKey::of).collect();
    here.iter()
        .filter(|session| session.timestamp <= saved_at)
        .all(|session| held.contains(&SessionKey::of(session)))
}

/// Why a change to the history was not stored at all, in words that fit
/// after "Nothing was deleted:". `None` when it was stored.
fn why_not_stored(saved: &HistorySaved) -> Option<String> {
    match saved {
        HistorySaved::Stored { .. } | HistorySaved::Squeezed { .. } => None,
        HistorySaved::Held => {
            Some("your saved history could not be read, so it was left as it is".into())
        }
        HistorySaved::Full => Some("there is no room left to save the change".into()),
        HistorySaved::Newer => Some(
            "a newer version of Dust changed how history is kept here; update Dust to change it"
                .into(),
        ),
        HistorySaved::Failed(reason) => Some(reason.trim_end_matches('.').to_string()),
    }
}

/// What became of a session or run, where it was not by your hand
/// ([`AppCtx::notices`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// The app stopped with it running: closed, crashed, killed or rebuilt.
    /// What it had done by then was kept, if its mark held any.
    CutOff(CutOff),
    /// The history could not take it yet, or a browser's page storage let
    /// older sessions go to make room for it, or the store found something
    /// to say of it as it was read: in words for the user.
    History(String),
    /// Reading the store set this aside, as it could not read it.
    SetAside(SetAside),
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
/// to read it and change your mind. A message to read through rather than
/// glance at stays as long.
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
    /// Put back the learner's own words, in place of the starter words.
    PutWordsBack,
}

impl ToastAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::UndoDelete | Self::UndoRestore | Self::PutCardBack | Self::PutWordsBack => "Undo",
        }
    }
}

/// A message at the foot of the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub action: Option<ToastAction>,
    /// Whether it is to be read through, a sentence or two, rather than
    /// glanced at.
    long: bool,
    /// Which raise this was. The same words raised twice are two toasts,
    /// each up for its own full time.
    serial: u64,
}

impl Toast {
    /// How long this one stays up.
    pub fn lasts_ms(&self) -> u32 {
        if self.action.is_some() || self.long {
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
        self.raise(text.into(), None, false);
    }

    /// A message of a sentence or two, up as long as one with something to
    /// do about it, to be read through.
    pub fn show_long(self, text: impl Into<String>) {
        self.raise(text.into(), None, true);
    }

    /// A message with something to do about it.
    pub fn show_with(self, text: impl Into<String>, action: ToastAction) {
        self.raise(text.into(), Some(action), false);
    }

    fn raise(self, text: String, action: Option<ToastAction>, long: bool) {
        static RAISED: AtomicU64 = AtomicU64::new(0);
        let mut shown = self.0;
        shown.set(Some(Toast {
            text,
            action,
            long,
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
                AppCtx::new(app, store.load_settings(), store.load_history())
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

        /// The record of the session that finished on `day`, here.
        fn id_of(&self, day: u64) -> RecordId {
            self.act(|ctx| {
                let history = ctx.sessions.peek();
                let index = history
                    .iter()
                    .position(|session| session.timestamp == stamp(day))
                    .expect("shown");
                history.id_at(index).expect("a record")
            })
        }

        /// Delete the session that finished on `day`.
        fn delete(&self, day: u64) {
            let id = self.id_of(day);
            self.act(|ctx| ctx.delete_sessions(&[id]));
        }

        /// Take up what the store holds, as a screen that shows the history
        /// does as it opens.
        fn sync(&self) {
            self.act(|ctx| ctx.sync_with_store(|| {}));
        }

        /// The days of the sessions this copy shows.
        fn shown(&self) -> Vec<u64> {
            self.act(|ctx| days(&ctx.sessions.peek()))
        }

        fn notices(&self) -> Vec<Notice> {
            self.act(|ctx| ctx.notices.peek().clone())
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
    /// other's: each stores its own as a record of its own, and takes up the
    /// other's as it looks. Each used to write back the whole history it
    /// held, so the one that stored last left out what the other had kept
    /// since it opened.
    #[test]
    fn a_session_kept_in_another_tab_is_not_lost() {
        let store = holding([1]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(2));
        a.finish(day(3));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        a.sync();
        assert_eq!(a.shown(), [1, 2, 3], "taken up as it looked");
        b.finish(day(4));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4]);
        b.sync();
        assert_eq!(b.shown(), [1, 2, 3, 4]);
    }

    /// A session deleted in one tab stays deleted when another stores, and
    /// goes from the other as it looks; one deleted in each goes from both.
    /// One deleted used to come back the next time a tab that still held it
    /// stored.
    #[test]
    fn a_session_deleted_in_another_tab_stays_deleted() {
        let store = holding([1, 2, 3, 4]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.delete(2);
        a.delete(4);
        assert_eq!(days(&store.load_sessions()), [1, 3]);
        a.sync();
        assert_eq!(a.shown(), [1, 3]);
        a.finish(day(5));
        b.finish(day(6));
        assert_eq!(days(&store.load_sessions()), [1, 3, 5, 6]);
        b.sync();
        assert_eq!(b.shown(), [1, 3, 5, 6]);
    }

    /// A record missing from the store, which no tombstone says was deleted,
    /// is stored again as a tab looks: missing is never deleted. It used to
    /// be dropped as deleted elsewhere, and lost from every tab for good.
    /// One deleted by hand goes from every tab, and Undo brings it back to
    /// every tab.
    #[test]
    fn a_record_lost_without_a_tombstone_is_put_back() {
        let store = holding([1]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        a.finish(day(2));
        store.lose(a.id_of(2));
        assert_eq!(days(&store.load_sessions()), [1], "lost for now");
        a.sync();
        assert_eq!(days(&store.load_sessions()), [1, 2], "stored again");
        assert_eq!(a.shown(), [1, 2]);

        b.delete(1);
        a.sync();
        assert_eq!(a.shown(), [2], "deleted by hand, it goes");
        b.act(AppCtx::undo_delete);
        a.sync();
        assert_eq!(a.shown(), [1, 2], "put back, it is wanted again");
        assert_eq!(days(&store.load_sessions()), [1, 2]);
    }

    /// A session the store lists under another name than the one it has
    /// here, as a browser's page storage may name one anew, is the same
    /// session: taken up once, by that name, and not stored again.
    #[test]
    fn a_session_the_store_names_anew_is_taken_up_once() {
        let store = holding([1, 2]);
        let a = Tab::on(&store);
        let was = a.id_of(2);
        let json = store.record(was).expect("stored");
        let renamed = RecordId {
            order: was.order + 1,
            ..was
        };
        store.lose(was);
        let put = HistoryChange {
            put: vec![Record { id: renamed, json }],
            ..HistoryChange::default()
        };
        let mut pending = Store::change_history(&*store, put);
        let mut asked = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(pending.as_mut().poll(&mut asked).is_ready());
        let writes = store.record_writes.get();
        a.sync();
        assert_eq!(a.shown(), [1, 2]);
        assert_eq!(a.id_of(2), renamed);
        assert_eq!(store.record_writes.get(), writes, "nothing stored again");
        assert_eq!(days(&store.load_sessions()), [1, 2]);
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
                settings: Some(TrainingSettings::default()),
                sessions: vec![day(1), day(2)],
            })
        });
        b.finish(day(8));
        a.sync();
        a.finish(day(9));
        assert_eq!(a.shown(), [1, 2, 5, 7, 8, 9]);
        a.act(AppCtx::undo_delete);
        assert_eq!(days(&store.load_sessions()), [1, 2, 5, 6, 7, 8, 9]);
        assert_eq!(a.shown(), [1, 2, 5, 6, 7, 8, 9]);
    }

    /// Undo puts a session back exactly where it was, by its record: before
    /// a session a clock set back stamped earlier, and apart from one alike
    /// in when it finished. Put back by its stamp, it used to go where the
    /// stamps put it.
    #[test]
    fn undo_puts_a_session_back_exactly_where_it_was() {
        let store = Rc::new(MemoryStore::default());
        let mut alike = day(4);
        alike.accuracy = 0.5;
        // As stored: 2, then 4, then 1 that a clock set back stamped, then
        // another 4.
        store.save_sessions(&[day(2), day(4), day(1), alike.clone()]);
        let a = Tab::on(&store);
        let before = a.act(|ctx| ctx.sessions.peek().ids().to_vec());
        let second = before[1];
        a.act(|ctx| ctx.delete_sessions(&[second]));
        assert_eq!(a.shown(), [2, 1, 4]);
        a.act(AppCtx::undo_delete);
        assert_eq!(a.act(|ctx| ctx.sessions.peek().ids().to_vec()), before);
        assert_eq!(a.shown(), [2, 4, 1, 4]);
        assert_eq!(store.load_sessions(), [day(2), day(4), day(1), alike]);
    }

    /// Deleting a session writes its tombstone and nothing else, however
    /// long the history: the whole of it used to be written again.
    #[test]
    fn deleting_one_session_writes_one_tombstone_and_nothing_else() {
        let store = holding(1..=60);
        let a = Tab::on(&store);
        a.delete(7);
        assert_eq!(store.tombstone_writes.get(), 1);
        assert_eq!(store.record_writes.get(), 0);
        assert_eq!(store.load_sessions().len(), 59);
        a.act(AppCtx::undo_delete);
        assert_eq!(store.record_writes.get(), 1, "the one put back");
        assert_eq!(store.load_sessions().len(), 60);
    }

    /// No session is ever let go: a history of two thousand takes one more,
    /// and a restore of ten older ones, and keeps every one, saying nothing
    /// of room. It used to keep the newest five hundred.
    #[test]
    fn no_session_is_ever_let_go() {
        let store = holding(11..=2_010);
        let a = Tab::on(&store);
        a.finish(day(2_011));
        assert_eq!(store.load_sessions().len(), 2_001);
        restore(&a, 1..=10);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 10 sessions and your settings."
        );
        let stored = days(&store.load_sessions());
        assert_eq!(stored, (1..=2_011).collect::<Vec<_>>());
        assert_eq!(a.shown(), stored);
        assert!(a.notices().is_empty(), "{:?}", a.notices());
    }

    /// A session another tab stored, stamped by a clock set back, falls
    /// after every session before it: it finished after them, whatever the
    /// clock said. What this tab adds after goes after it.
    #[test]
    fn a_session_a_clock_set_back_stamped_falls_where_it_finished() {
        let store = holding([10, 20]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(5));
        a.sync();
        a.finish(day(30));
        assert_eq!(a.shown(), [10, 20, 5, 30]);
        assert_eq!(days(&store.load_sessions()), [10, 20, 5, 30]);
    }

    /// Sessions alike in when and on what day they finished are told apart
    /// by their records: one deleted in another tab leaves the other.
    #[test]
    fn sessions_alike_are_told_apart_by_their_records() {
        let mut second = day(1);
        second.accuracy = 0.5;
        let store = Rc::new(MemoryStore::default());
        store.save_sessions(&[day(1), second.clone()]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        let first = b.act(|ctx| ctx.sessions.peek().id_at(0).expect("a record"));
        b.act(|ctx| ctx.delete_sessions(&[first]));
        a.sync();
        assert_eq!(a.act(|ctx| ctx.sessions.peek().to_vec()), [second.clone()]);
        assert_eq!(store.load_sessions(), [second]);
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
                settings: Some(TrainingSettings::default()),
                sessions: vec![day(1), day(2)],
            })
        });
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        b.finish(day(4));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4]);
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [3, 4]);
        a.sync();
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

    /// Two tabs restoring one backup, neither having seen the other's,
    /// store each session of it once: each is the same record wherever it
    /// is restored.
    #[test]
    fn two_tabs_restoring_one_backup_store_each_session_once() {
        let store = holding([3]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        // Neither can see what the other stored as it restores.
        store.unlistable.set(true);
        restore(&a, [1, 2]);
        restore(&b, [1, 2]);
        store.unlistable.set(false);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        a.sync();
        b.sync();
        assert_eq!(a.shown(), [1, 2, 3]);
        assert_eq!(b.shown(), [1, 2, 3]);
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
                settings: Some(backup.clone()),
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
            settings: Some(TrainingSettings::default()),
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

    /// A backup holds the sessions here the store has yet to take as well:
    /// a backup is how they are kept safe meanwhile.
    #[test]
    fn a_backup_holds_what_the_store_has_yet_to_take() {
        let store = holding([1]);
        let a = Tab::on(&store);
        store.full.set(true);
        a.finish(day(2));
        assert_eq!(days(&store.load_sessions()), [1]);
        a.act(AppCtx::save_backup);
        let files = store.backup_files.borrow().clone();
        let (_, contents) = files.last().expect("a backup file");
        let backup = crate::persist::backup::read_backup(contents).expect("a backup");
        assert_eq!(days(&backup.sessions), [1, 2]);
    }

    /// What reading the store set aside, as it could not read it, is told
    /// of: what the launch set aside at once, and what a later read set
    /// aside once it has. It used to go without a word, the settings back
    /// to their defaults or the history shorter.
    #[test]
    fn what_the_store_set_aside_is_told_of() {
        let store = holding([1]);
        let settings = SetAside::Settings { kept: None };
        store.set_aside.borrow_mut().push(settings.clone());
        let a = Tab::on(&store);
        assert_eq!(a.notices(), [Notice::SetAside(settings.clone())]);
        let sessions = SetAside::Sessions {
            count: 2,
            kept: None,
        };
        store.set_aside.borrow_mut().push(sessions.clone());
        a.finish(day(2));
        assert_eq!(
            a.notices(),
            [Notice::SetAside(settings), Notice::SetAside(sessions)]
        );
        a.finish(day(3));
        assert_eq!(a.notices().len(), 2, "each once");
    }

    /// What the store found of the history as it was read at launch is
    /// told of at once, beside what it set aside: here, records a newer
    /// Dust wrote.
    #[test]
    fn what_the_store_found_of_the_history_is_told_of_at_launch() {
        let store = holding([1]);
        store.plant_unreadable("{\"later\":1}");
        let a = Tab::on(&store);
        let told = crate::persist::record::HistoryNotice::Unread { count: 1 }.text();
        assert_eq!(a.notices(), [Notice::History(told)]);
        assert_eq!(a.shown(), [1]);
        a.sync();
        assert_eq!(a.shown(), [1], "one it cannot read is not read again");
    }

    /// A history on its own restores its sessions, and leaves the settings
    /// as they are, Undo too: it holds none.
    #[test]
    fn a_history_on_its_own_restores_its_sessions_alone() {
        let store = holding([3]);
        let a = Tab::on(&store);
        let groups = |n| {
            a.act(|ctx| {
                let mut settings = ctx.settings;
                settings.write().curriculum.num_groups = n;
            })
        };
        groups(9);
        a.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: 0,
                settings: None,
                sessions: vec![day(1)],
            })
        });
        assert_eq!(days(&store.load_sessions()), [1, 3]);
        assert_eq!(a.toast().expect("told").text, "Restored 1 session.");
        assert_eq!(a.act(|ctx| ctx.settings.peek().curriculum.num_groups), 9);
        groups(4);
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [3]);
        assert_eq!(a.act(|ctx| ctx.settings.peek().curriculum.num_groups), 4);
    }

    /// A backup marked by a clock set ahead counts as saved now, and so does
    /// a backup from such a clock that is restored: sessions finished from
    /// now on are counted since. Marked later than every session finished
    /// once the clock was put right, it used to say nothing new since until
    /// the real time caught up with it.
    #[test]
    fn a_backup_marked_by_a_clock_set_ahead_counts_from_now() {
        let year = 365 * 86_400_000;
        let store = holding([1]);
        store.mark_backup(crate::time::now_ms() + year);
        let a = Tab::on(&store);
        let marked = a.act(|ctx| *ctx.last_backup.peek()).expect("marked");
        assert!(marked <= crate::time::now_ms(), "{marked}");
        assert_eq!(store.last_backup(), Some(marked), "remembered so");

        let store = holding([1]);
        let a = Tab::on(&store);
        a.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: crate::time::now_ms() + year,
                settings: None,
                sessions: vec![day(1)],
            })
        });
        let marked = store.last_backup().expect("marked");
        assert!(marked <= crate::time::now_ms(), "{marked}");
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
                settings: Some(TrainingSettings::default()),
                sessions: vec![day(1), day(2)],
            })
        });
        assert_eq!(store.last_backup(), Some(stamp(30)));
        assert_eq!(a.act(|ctx| *ctx.last_backup.peek()), Some(stamp(30)));
    }

    /// Restore the backup holding a session for each of `days`, in `tab`.
    fn restore(tab: &Tab, days: impl IntoIterator<Item = u64>) {
        let sessions = days.into_iter().map(day).collect();
        tab.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: 0,
                settings: Some(TrainingSettings::default()),
                sessions,
            })
        });
    }

    /// A store that takes no change leaves a restore, and its Undo, undone,
    /// and says why: nothing here changes.
    #[test]
    fn a_restore_the_store_cannot_take_changes_nothing() {
        let store = holding([3]);
        let a = Tab::on(&store);
        store.full.set(true);
        restore(&a, [1, 2]);
        assert!(
            a.toast()
                .is_some_and(|toast| toast.text.starts_with("Nothing was restored:")),
            "{:?}",
            a.toast()
        );
        assert_eq!(a.shown(), [3]);
        store.full.set(false);
        restore(&a, [1, 2]);
        store.full.set(true);
        a.act(AppCtx::undo_restore);
        assert!(
            a.toast()
                .is_some_and(|toast| toast.text.starts_with("The restore could not be undone:"))
        );
        assert_eq!(a.shown(), [1, 2, 3]);
        store.full.set(false);
        a.act(AppCtx::undo_restore);
        assert_eq!(a.shown(), [3], "and can be undone once it can");
        assert_eq!(days(&store.load_sessions()), [3]);
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
                    settings: Some(TrainingSettings::default()),
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
        assert_eq!(store.load_sessions().len(), 3, "in its place");
        a.act(AppCtx::undo_restore);
        assert_eq!(store.load_sessions(), [day(1), run(false), day(3)]);

        restore_run(&b);
        restore_run(&a);
        a.act(AppCtx::undo_restore);
        assert_eq!(store.load_sessions(), [day(1), run(true), day(3)]);
    }

    /// A store that cannot be listed for now still takes the sessions
    /// finished here, and what another tab stored is taken up once it can
    /// be listed again. Nothing here goes for want of a listing: it is no
    /// word that anything is missing.
    #[test]
    fn a_history_that_cannot_be_listed_still_takes_new_sessions() {
        let store = holding([1, 2]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(3));
        store.unlistable.set(true);
        a.finish(day(4));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4]);
        a.sync();
        assert_eq!(a.shown(), [1, 2, 4], "nothing taken up, nothing let go");
        store.unlistable.set(false);
        a.sync();
        assert_eq!(a.shown(), [1, 2, 3, 4]);
    }

    /// A session a failing store could not take is kept here, told of, and
    /// stored with the next change the store takes, beside what another
    /// tab kept in the meantime.
    #[test]
    fn a_session_the_store_failed_to_take_is_stored_with_the_next_change() {
        let store = holding([1]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        *store.failing.borrow_mut() = Some("the disk went away.".into());
        a.finish(day(2));
        assert_eq!(
            a.notices(),
            [Notice::History(
                HistorySaved::Failed("the disk went away.".into())
                    .problem()
                    .expect("told")
            )]
        );
        assert_eq!(a.shown(), [1, 2], "kept here");
        store.failing.borrow_mut().take();
        b.finish(day(3));
        assert_eq!(days(&store.load_sessions()), [1, 3]);
        a.finish(day(4));
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4]);
        a.sync();
        assert_eq!(a.shown(), [1, 2, 3, 4]);
    }

    /// A browser's page storage, where it keeps the history whole for want
    /// of a database, keeps only the newest that fit: the oldest go from
    /// every tab as well as from the store, and the tab that stored says so
    /// by name. They are let go, not deleted: no tombstone says so, and a
    /// session deleted by hand stays deleted.
    #[test]
    fn a_browsers_page_storage_lets_the_oldest_go_and_says_so() {
        let store = holding([1, 2, 3]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        store.page_room.set(Some(3));
        b.finish(day(4));
        assert_eq!(days(&store.load_sessions()), [2, 3, 4]);
        assert_eq!(b.shown(), [2, 3, 4], "what the store holds");
        let notices = b.notices();
        let [Notice::History(said)] = notices.as_slice() else {
            panic!("{notices:?}");
        };
        assert!(
            said.contains("page storage") && said.contains("newest 3"),
            "{said}"
        );
        a.sync();
        assert_eq!(a.shown(), [2, 3, 4], "let go from here too");
        a.delete(3);
        b.finish(day(5));
        assert_eq!(days(&store.load_sessions()), [2, 4, 5], "3 stays deleted");
        b.sync();
        assert_eq!(b.shown(), [2, 4, 5]);
        assert_eq!(store.tombstone(a.id_of(4)), None);
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

    /// A sampler starts from the prior built last while the history and
    /// what the prior reads of the settings are as they were: the volume
    /// builds nothing again, the speeds do, and so does whatever changes
    /// the history, here or in another tab, once each.
    #[test]
    fn the_sampler_prior_is_built_again_only_when_what_it_reads_changes() {
        let store = holding([1, 2]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        let built = || {
            a.act(|ctx| {
                let settings = ctx.settings.peek().clone().clamp();
                ctx.sampler(&settings, 1);
            });
            crate::testing::take_renders("SamplerPrior")
        };
        crate::testing::take_renders("SamplerPrior");
        assert_eq!(built(), 1, "the first");
        assert_eq!(built(), 0, "kept");
        let set = |change: fn(&mut TrainingSettings)| {
            a.act(|ctx| {
                let mut settings = ctx.settings;
                change(&mut settings.write());
            });
        };
        set(|settings| settings.band.master_volume = 0.3);
        assert_eq!(built(), 0, "the volume is not read");
        set(|settings| settings.playback.char_wpm_max = 36.0);
        assert_eq!(built(), 1, "the speeds are");
        a.delete(1);
        assert_eq!(built(), 1, "deleted");
        a.act(AppCtx::undo_delete);
        assert_eq!(built(), 1, "put back");
        a.finish(day(3));
        assert_eq!(built(), 1, "kept here");
        b.finish(day(4));
        assert_eq!(built(), 0, "not here until it is read");
        a.sync();
        assert_eq!(built(), 1, "kept in another tab");
        restore(&a, [5]);
        assert_eq!(built(), 1, "restored");
        assert_eq!(built(), 0);
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
