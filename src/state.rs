//! What the app is: its state, reactive and not, and the moves every screen
//! shares.

pub mod answer;
pub mod entry;
mod keep;
pub mod live;
pub mod preview;
pub mod sync;

pub use answer::{AnswerOutbox, TypedAnswer};
pub use entry::{ContestEntry, typed_len_max};
pub use keep::{keep_and_show, keep_cut_off, settle_level};
pub use live::{Live, LiveContest, LiveSession};
pub use preview::{Dialogue, DialogueLine, HeardGroup, Preview};
pub use sync::SyncStatus;

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

use self::sync::SyncCtx;
use crate::format::{counted, day_of, short_date};
use crate::history::{StoredHistory, settle_or_spawn};
use crate::persist::backup::{
    Backup, BackupFile, Filled, NotSaved, backup_name, filled_in, merge_sessions,
};
use crate::persist::record::{
    HistoryChange, HistoryLoad, Record, RecordId, Unopened, Unreadable, read_record, unreadable,
};
use crate::persist::{
    CutOff, Deleted, HistorySaved, Lock, LockHeld, Locking, MarkId, OpenSession, SessionKey,
    SetAside, SoFar, Store, TakenMark, merge_settings,
};
use crate::player::{AudioError, BackendFactory, Player};
use crate::time::{POLL_MS, now_ms, sleep_ms};

/// How long after the store last refused what the app has yet to store it
/// is tried again, while anything waits: the page coming back into sight,
/// or the next change, try it sooner.
pub const RETRY_MS: u32 = 60_000;

/// How long after the store could not read a record it listed, as the app
/// opened or as a look found it, the history is looked at again on its own,
/// at each of these from the first, while any such record is left
/// ([`AppCtx::read_again_soon`]): a window kept in front, with no other copy
/// of the app beside it, may not look again for as long as it runs. A
/// moment's failure, as of a file another program holds open, is over by
/// the first; a record that still cannot be read at the last is told of
/// ([`crate::persist::record::HistoryNotice::Unread`]).
pub const READ_AGAIN_MS: [u32; 3] = [2_000, 10_000, 60_000];

/// How long a restore asked for while a session or a run has the app waits
/// before it looks again, to be made once that is over
/// ([`AppCtx::restore_backup`]).
const RESTORE_WAIT_MS: u32 = 1_000;

/// How long a restore asked for while another copy of the app has every
/// copy's restores waits before it tries again ([`Lock::Restore`]): as a
/// sync waits for another copy's.
const RESTORE_LOCK_POLL_MS: u32 = 250;

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
    pub stream_heard: Signal<Vec<HeardGroup>>,
    /// What Hear a QSO has played, as far as it has been heard: kept once it
    /// ends, until something else plays or the screen is left.
    pub dialogue: Signal<Option<Dialogue>>,
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
    /// What the last restore changed, for Undo to put back, until another
    /// restore is asked for ([`Self::undo_restore`]).
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
    /// What the marks of sessions cut off carried, which went from the
    /// store once this copy's mark held it, until the history holds it:
    /// this copy's mark holds it meanwhile, as nothing else does
    /// ([`Self::tell_of_cut_off`]).
    cut_off_carried: CopyValue<Vec<SessionResult>>,
    /// A try at storing what the store has yet to take is waiting
    /// ([`RETRY_MS`]).
    retrying: CopyValue<bool>,
    /// The settings could not be stored, and the user has been told: not
    /// again until they have been stored since.
    settings_unsaved: CopyValue<bool>,
    /// The records the store listed and could not read at all for now, as
    /// the app opened or as a look found them ([`HistoryLoad::failed`]):
    /// neither shown nor held as stored, but read again at every look, and
    /// on their own soon after ([`READ_AGAIN_MS`]), until each reads or is
    /// no longer listed. A restore is refused while any is, once each
    /// ([`Self::restore_backup`]), and a backup holds none of them but as
    /// the store holds it.
    unread_for_now: CopyValue<BTreeSet<RecordId>>,
    /// Of those, the ones told of as still unread at their last try, or
    /// whose store was asked to: not read again on their own as this copy
    /// runs, though every look still reads them.
    told_unread_for_now: CopyValue<BTreeSet<RecordId>>,
    /// Tries at reading those again on their own are under way
    /// ([`Self::read_again_soon`]).
    reading_again: CopyValue<bool>,
    /// Of those, the ones a restore was refused for already: one still
    /// unread at a later restore is taken as one that cannot be read at
    /// all ([`Self::restore_backup`]).
    restore_refused_for: CopyValue<BTreeSet<RecordId>>,
    /// A restore, or the Undo of one, is under way: from its being asked
    /// for until what became of it has been told ([`Self::restore_backup`]).
    restore_under_way: CopyValue<bool>,
    /// How many restores have been asked for: each is known by its number
    /// until it is told of ([`Self::restore_told`]).
    restores_asked: CopyValue<u64>,
    /// The restore under way, by its number, until what became of it has
    /// been told: none while an Undo is, or nothing is.
    current_restore: CopyValue<Option<u64>>,
    /// The restore asked for last while another, or an Undo, was under way,
    /// a session or run had the app, or another copy of the app had every
    /// copy's restores, by its number: made once that is over. One asked
    /// for after takes its place, and it is never made.
    restores_waiting: CopyValue<Option<(u64, Backup)>>,
    /// A task waits to make the restore waiting, once no session or run has
    /// the app and no other copy has every copy's restores
    /// ([`Self::restore_when_free`]).
    restore_waiter: CopyValue<bool>,
    /// The lock on every copy's restores, held by this copy from a
    /// restore's start until no restore, nor Undo, is under way, waiting or
    /// offered ([`Lock::Restore`], [`Self::let_go_of_restores`]).
    restore_lock: CopyValue<Option<LockHeld>>,
    /// Another copy of the app has every copy's restores, and the restore
    /// waiting waits for it ([`Self::restore_blocked`]).
    restore_blocked: CopyValue<bool>,
    /// The message offering the Undo of the last restore, which it is
    /// offered for no longer than that is up ([`Self::undo_offered_no_more`]).
    undo_toast: CopyValue<Option<u64>>,
    /// Sync with the user's own Google Drive, where it is on
    /// ([`self::sync`]).
    pub sync: SyncCtx,
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
            let sync = SyncCtx::new(&*app.store);
            let unread_for_now = history.failed.iter().copied().collect();
            let ctx = Self {
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
                cut_off_carried: CopyValue::new(Vec::new()),
                retrying: CopyValue::new(false),
                settings_unsaved: CopyValue::new(false),
                unread_for_now: CopyValue::new(unread_for_now),
                told_unread_for_now: CopyValue::new(BTreeSet::new()),
                reading_again: CopyValue::new(false),
                restore_refused_for: CopyValue::new(BTreeSet::new()),
                restore_under_way: CopyValue::new(false),
                restores_asked: CopyValue::new(0),
                current_restore: CopyValue::new(None),
                restores_waiting: CopyValue::new(None),
                restore_waiter: CopyValue::new(false),
                restore_lock: CopyValue::new(None),
                restore_blocked: CopyValue::new(false),
                undo_toast: CopyValue::new(None),
                sync,
                app: CopyValue::new(app),
            };
            ctx.toasts.when_offered_no_more(move |action, serial| {
                ctx.undo_offered_no_more(action, serial)
            });
            ctx.hear_history_news();
            ctx.hear_sync_news();
            ctx.read_again_soon();
            ctx.sync_at_launch();
            ctx
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
    /// holds it: the mark stays, over, until the store has taken it
    /// ([`Self::settle_mark`]). Should the app stop first, or the store have
    /// no room for it, the next launch keeps it from the mark, and knows it
    /// was ended and kept rather than cut off. Over only here, a page
    /// closed under a browser's database, which answers only later, left a
    /// mark that said its session still ran.
    pub fn mark_kept(self) {
        let Some(mut open) = self.mark.peek().clone() else {
            return;
        };
        open.over = true;
        self.write_mark(open);
    }

    /// Write this copy's mark as `open` says, carrying every session the
    /// store has yet to take but the one it holds so far, and what the
    /// marks of sessions cut off carried that the history has yet to hold
    /// ([`Self::cut_off_carried`]); or, once its session or run is over and
    /// nothing is left to carry, take it away.
    fn write_mark(self, mut open: OpenSession) {
        let own = open
            .so_far
            .as_ref()
            .map(|so_far| SessionKey::of(&so_far.result));
        let mut unstored: Vec<SessionResult> = self
            .sessions
            .peek()
            .unsaved_sessions()
            .filter(|session| own.as_ref() != Some(&SessionKey::of(session)))
            .cloned()
            .collect();
        let mut keys: HashSet<SessionKey> = unstored.iter().map(SessionKey::of).collect();
        keys.extend(own);
        unstored.extend(
            self.cut_off_carried
                .peek()
                .iter()
                .filter(|session| keys.insert(SessionKey::of(session)))
                .cloned(),
        );
        open.unstored = unstored;
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
    /// the app stop before the store takes it, and so does one short of
    /// what a mark of a session cut off carried. With no mark held, one is
    /// made for that: a session cut off and kept, say, whose own mark was
    /// taken as it was told of.
    fn settle_mark(self) {
        let waiting: HashSet<SessionKey> = self
            .sessions
            .peek()
            .unsaved_sessions()
            .chain(self.cut_off_carried.peek().iter())
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
    ///
    /// What the store holds is taken up first: a copy that stored what its
    /// mark carries, and stopped before its mark went, kept it already,
    /// though a copy open all along may not have taken it up yet. Told of
    /// as kept, it is not stored a second time.
    ///
    /// The marks this copy takes are held by it, and left in the store,
    /// until what they carry is held with this copy's mark, at once, before
    /// the store is taken up, which on a computer or a phone is a look on a
    /// thread of its own, and in a browser a moment later still; then they
    /// go ([`Store::release_cut_off`]), and nothing is kept, nor a level
    /// counted, until they have gone, or wait for a write of this copy's
    /// mark that goes in. Until the history holds it
    /// ([`keep_cut_off`]), this copy's mark does, and nothing else: a copy
    /// that stops before its mark holds it leaves the marks to the next
    /// copy to look.
    pub fn tell_of_cut_off(self) {
        // This copy's own mark, which it holds still, is no other copy's
        // cut off, whatever a store says of it.
        let own = self.mark.peek().as_ref().map(|open| open.started_at);
        let looking = self.app().store.take_cut_off();
        let mut notices = self.notices;
        // In the root scope: the press that starts a session leaves the
        // screen it was made on.
        dioxus::core::spawn_forever(async move {
            let (own, others): (Vec<TakenMark>, Vec<TakenMark>) =
                looking.await.into_iter().partition(|taken| {
                    matches!(&taken.cut, CutOff::Marked(open) if Some(open.started_at) == own)
                });
            let left: Vec<MarkId> = own.into_iter().map(|taken| taken.id).collect();
            let store = self.app().store;
            if others.is_empty() {
                store.release_cut_off(Vec::new(), left).await;
                return;
            }
            let (carried, cut): (Vec<MarkId>, Vec<CutOff>) = others
                .into_iter()
                .map(|taken| (taken.id, taken.cut))
                .unzip();
            self.carry_cut_off(&cut);
            // Held by this copy's mark now, or by the history: the marks
            // that carried it go, before its level is counted, so that it
            // is counted once. A browser takes them away a moment later,
            // once its tab's mark is in place, and that is waited for here.
            // While this copy's mark will not take a write, they wait for
            // one that does, and what they carry is kept meanwhile: a copy
            // that stops first leaves them to the next, which keeps each
            // session once, and may count its level again.
            store.release_cut_off(carried, left).await;
            // Kept whether or not the store could be listed: its marks are
            // let go of already, so what they carry is kept here, as this
            // copy's mark holds it, or nowhere. Stored twice at worst, never
            // lost.
            self.sync_with_store(move |_listed| {
                let told = {
                    let history = self.sessions.peek();
                    CutOff::newest(
                        cut.iter()
                            .filter(|cut| !kept_already(cut, &history))
                            .cloned(),
                    )
                };
                if let Some(newest) = told {
                    notices.write().push(Notice::CutOff(newest));
                }
                spawn_forever(keep_cut_off(self, cut));
            });
        });
    }

    /// Hold what `cut`, the marks of sessions cut off, carry, that the
    /// history does not hold, with this copy's mark, until the history
    /// holds it or lets it go ([`Self::let_go_of_carried`]).
    fn carry_cut_off(self, cut: &[CutOff]) {
        let mut carried = self.cut_off_carried;
        {
            let history = self.sessions.peek();
            let mut held = carried.write();
            let mut keys: HashSet<SessionKey> = held.iter().map(SessionKey::of).collect();
            let sessions = cut
                .iter()
                .filter_map(CutOff::so_far)
                .map(|so_far| &so_far.result)
                .chain(cut.iter().flat_map(CutOff::unstored));
            for session in sessions {
                let key = SessionKey::of(session);
                if !history.has_key(&key) && keys.insert(key) {
                    held.push(session.clone());
                }
            }
        }
        self.settle_mark();
    }

    /// What `cut`, the marks of sessions cut off, carried is held by the
    /// history now, or let go: this copy's mark holds it no longer for
    /// their sake ([`Self::carry_cut_off`]).
    pub fn let_go_of_carried(self, cut: &[CutOff]) {
        let keys: HashSet<SessionKey> = cut
            .iter()
            .filter_map(CutOff::so_far)
            .map(|so_far| &so_far.result)
            .chain(cut.iter().flat_map(CutOff::unstored))
            .map(SessionKey::of)
            .collect();
        let mut carried = self.cut_off_carried;
        let held = carried
            .peek()
            .iter()
            .any(|session| keys.contains(&SessionKey::of(session)));
        if held {
            carried
                .write()
                .retain(|session| !keys.contains(&SessionKey::of(session)));
            self.settle_mark();
        }
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
                Ok(()) => {
                    held = merged.clone();
                    self.sync_settings_stored(&merged);
                }
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
        // own values as typed until they are left. What is kept for a later
        // build is taken up all the same, as is what of it a change here
        // has let go of (`TrainingSettings::unread_values`), so a setting
        // chosen back after it is stored is written as chosen.
        let mut as_typed = merged.clone();
        as_typed.unread_values.clone_from(&ours.unread_values);
        if as_typed != ours {
            settings.set(merged.clone());
        } else if settings.peek().unread_values != merged.unread_values {
            settings.write().unread_values = merged.unread_values.clone();
        }
        merged
    }

    /// Store the settings at once, rather than once an edit on the settings
    /// screen has been still for a moment, or once the effect that stores them
    /// next runs: the page has gone out of sight, or the app is closing, and a
    /// tab closed or an app put away straight after an edit would lose it while
    /// it waited. Not while a session runs, which stores nothing until it ends.
    pub fn store_settings_now(self) {
        if *self.screen.peek() != Screen::Training {
            self.store_settings();
        }
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

    /// Take up the settings another tab, window or copy of the app stored
    /// since this one last read or wrote them, merged with those here as
    /// [`Self::store_settings`] merges them, writing nothing: as the app
    /// looks at the store ([`Self::sync_with_store`]), and as a sync begins,
    /// which takes what differs here from the settings as of the last sync,
    /// another copy's or this one's, for a change made here
    /// ([`self::sync`]).
    fn take_up_stored_settings(self) {
        let settings_read = self.app().store.read_settings();
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
    }

    /// Bring the history and the settings here in line with the store's,
    /// then do `then`, told whether the store could be listed: at once
    /// wherever the store answers at once, as every store but a browser's
    /// database does.
    ///
    /// Whatever another tab, window or copy of the app stored or deleted
    /// since this one last looked is taken up: each record listed that is
    /// not here is read and put in its place, and each here that is no
    /// longer listed goes, if it was deleted by hand. One missing for any
    /// other reason is stored again: missing is never deleted. What is here
    /// and not stored yet is left as it is, to be stored. A record this
    /// build could not read that is no longer listed is no longer held for
    /// one stored ([`StoredHistory::unread`]). One listed that could not be
    /// read at all for now is read again, at this look and the next, until
    /// it reads or is no longer listed ([`Self::unread_for_now`]). The
    /// settings are taken up as well ([`Self::take_up_stored_settings`]).
    ///
    /// A restore and a backup go by what the store holds, not by what this
    /// copy last saw of it: a restore merged with a history another copy
    /// has since deleted from takes a session it brings back for one
    /// already here, and a backup of an old copy's history leaves out what
    /// was stored since, though it counts as backed up. Where the store
    /// cannot be listed, nothing is taken up, and `then` is told so.
    fn sync_with_store(self, then: impl FnOnce(bool) + 'static) {
        let store = self.app().store;
        self.take_up_stored_settings();
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
        // Those that could not be read for now are read again, as they are
        // neither here nor held as unread.
        let unread_for_now = self.unread_for_now.peek().clone();
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
            let unread_gone: Vec<RecordId> = unread.difference(&there).copied().collect();
            let failed_gone: Vec<RecordId> = unread_for_now.difference(&there).copied().collect();
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
                unread_gone,
                failed_gone,
            })
        });
        settle_or_spawn(looking, move |looked| {
            let listed = looked.is_some();
            if let Some(looked) = looked {
                self.take_up(looked);
            }
            then(listed);
        });
    }

    /// Take up what a look at the store found ([`Self::sync_with_store`]).
    fn take_up(self, looked: Looked) {
        let Looked {
            read,
            missing,
            forgotten,
            unread_gone,
            failed_gone,
        } = looked;
        self.note_unread_for_now(&read, &failed_gone);
        let forgotten: HashSet<RecordId> = forgotten.into_iter().collect();
        let mut sessions = self.sessions;
        let gone = {
            let history = sessions.peek();
            unread_gone.iter().any(|id| history.unread().contains(id))
        };
        if gone {
            sessions.write().forget_unread(&unread_gone);
        }
        // One whose bytes are no text is damaged, as the history read whole
        // takes it: held as one no build reads, not read again, and told of
        // as damaged at the next launch.
        let damaged: Vec<RecordId> = read
            .iter()
            .filter_map(|record| match record {
                Err(Unopened::Damaged(id)) => Some(*id),
                _ => None,
            })
            .collect();
        if !damaged.is_empty() {
            let mut history = sessions.write();
            for id in damaged {
                history.mark_unread(id, &Unreadable::default());
            }
        }
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
        // Each record here let go for one that arrived as its session, and
        // that one.
        let mut taken_for = Vec::new();
        {
            let mut history = sessions.write();
            let mut arrived = Vec::new();
            for record in read.into_iter().flatten() {
                match read_record(&record.json) {
                    Some(session) => arrived.push((record.id, session)),
                    None => history.mark_unread(record.id, &unreadable(&record.json)),
                }
            }
            // Each arrived that stands for a session here already.
            let mut claimed = vec![false; arrived.len()];
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
                // page storage may name one anew, or filled in by a restore
                // elsewhere, or put back as it was by its Undo, where the
                // store remembers no deletion of what it took the place of:
                // taken up by that name, not stored a second time.
                let alike = |at: usize| !claimed[at] && arrived[at].1 == *here;
                let filled = |at: usize| !claimed[at] && filled_in(&arrived[at].1, here);
                let same = (0..arrived.len())
                    .find(|&at| alike(at))
                    .or_else(|| (0..arrived.len()).find(|&at| filled(at)));
                if let Some(at) = same {
                    claimed[at] = true;
                    history.remove(id);
                    taken_for.push((id, arrived[at].0));
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
        self.follow_the_session_kept(&taken_for);
        if lost {
            self.store_unsaved();
        }
    }

    /// Where the record the session just finished was kept under is no
    /// longer here, another takes its place in what the results screen
    /// finds it by ([`Finished::Kept`]): the one a take-up took up as that
    /// session, `taken_for` it, under another name, filled in, or written
    /// back by an older build without what it does not keep
    /// ([`Self::take_up`]); failing that, as for a sync's twin of it, whose
    /// name here the sync deletes, the one record holding that very
    /// session, whatever format either says it was stored in
    /// ([`StoredHistory::holding`]). The session shown stays as it ended.
    /// None takes its place where the session went, or two records hold it
    /// alike and neither was taken up as it. The screen used to keep the
    /// name gone, and place the session nowhere.
    fn follow_the_session_kept(self, taken_for: &[(RecordId, RecordId)]) {
        let mut result = self.result;
        let holding = {
            let Some(Finished::Kept(session, Some(kept))) = &*result.peek() else {
                return;
            };
            let history = self.sessions.peek();
            if history.index_of(*kept).is_some() {
                return;
            }
            taken_for
                .iter()
                .find(|(was, _)| was == kept)
                .map(|(_, now)| *now)
                .or_else(|| history.holding(session))
        };
        if let Some(holding) = holding
            && let Some(Finished::Kept(_, kept)) = &mut *result.write()
        {
            *kept = Some(holding);
        }
    }

    /// Note what a look found of the records the store could not read for
    /// now ([`Self::unread_for_now`]): each read now is one no more, as is
    /// each found damaged and each no longer listed, `gone`, and each
    /// listed that could not be read just now is one, and is read again
    /// soon ([`Self::read_again_soon`]). One that is no more refuses no
    /// restore, and is told of no more, for having been so: should it fail
    /// again, that is a failure of its own.
    fn note_unread_for_now(self, read: &[Result<Record, Unopened>], gone: &[RecordId]) {
        let mut unread_for_now = self.unread_for_now;
        let (over, joined) = {
            let held = unread_for_now.peek();
            let over: Vec<RecordId> = read
                .iter()
                .filter_map(|record| match record {
                    Ok(record) => Some(record.id),
                    Err(Unopened::Damaged(id)) => Some(*id),
                    Err(Unopened::Failed(_)) => None,
                })
                .chain(gone.iter().copied())
                .filter(|id| held.contains(id))
                .collect();
            let joined: Vec<RecordId> = read
                .iter()
                .filter_map(|record| match record {
                    Err(Unopened::Failed(id)) => Some(*id),
                    _ => None,
                })
                .filter(|id| !held.contains(id))
                .collect();
            (over, joined)
        };
        if over.is_empty() && joined.is_empty() {
            return;
        }
        {
            let mut held = unread_for_now.write();
            for id in &over {
                held.remove(id);
            }
            held.extend(joined.iter().copied());
        }
        let (mut refused, mut told) = (self.restore_refused_for, self.told_unread_for_now);
        if over.iter().any(|id| refused.peek().contains(id)) {
            refused.write().retain(|id| !over.contains(id));
        }
        if over.iter().any(|id| told.peek().contains(id)) {
            told.write().retain(|id| !over.contains(id));
        }
        if !joined.is_empty() {
            self.read_again_soon();
        }
    }

    /// Read again, on their own, the records the store could not read for
    /// now and has not told of ([`Self::unread_for_now`]), at each of
    /// [`READ_AGAIN_MS`] from now while any of them is left: one window
    /// kept in front, with no other copy of the app beside it, may never
    /// look again. Each look reads every one of them, and takes up what
    /// else changed. Those still unread at the last are told of, once, as
    /// records that could not be read ([`Self::tell_still_unread`]), and
    /// any that joined them meanwhile are read again in turn. One round at
    /// a time.
    fn read_again_soon(self) {
        let mut reading = self.reading_again;
        if *reading.peek() {
            return;
        }
        let round: BTreeSet<RecordId> = {
            let told = self.told_unread_for_now.peek();
            self.unread_for_now
                .peek()
                .difference(&told)
                .copied()
                .collect()
        };
        if round.is_empty() {
            return;
        }
        reading.set(true);
        self.read_again_at(Rc::new(round), 0);
    }

    /// The `step`th try of a round of [`Self::read_again_soon`] at reading
    /// `round` again, the tries before it made: or, past the last, what is
    /// still unread told of, and the round over.
    fn read_again_at(self, round: Rc<BTreeSet<RecordId>>, step: usize) {
        let left = move |round: &BTreeSet<RecordId>| {
            self.unread_for_now
                .peek()
                .iter()
                .any(|id| round.contains(id))
        };
        let Some(&due) = READ_AGAIN_MS.get(step) else {
            self.tell_still_unread(&round);
            self.round_over();
            return;
        };
        let since = step
            .checked_sub(1)
            .and_then(|before| READ_AGAIN_MS.get(before))
            .copied()
            .unwrap_or(0);
        // In the root scope: the screen it began on may be left.
        spawn_forever(async move {
            sleep_ms(due - since).await;
            if !left(&round) {
                self.round_over();
                return;
            }
            self.sync_with_store(move |_| self.read_again_at(round, step + 1));
        });
    }

    /// A round of tries at reading again what could not be read for now is
    /// over ([`Self::read_again_soon`]): the next starts, for what joined
    /// them meanwhile, where anything did.
    fn round_over(self) {
        let mut reading = self.reading_again;
        reading.set(false);
        self.read_again_soon();
    }

    /// Tell of each of `round` still unread for now after every try, as
    /// the store says of it, so that each is told of once, at this launch
    /// or a later one ([`Store::still_unread`]).
    fn tell_still_unread(self, round: &BTreeSet<RecordId>) {
        let still: Vec<RecordId> = self
            .unread_for_now
            .peek()
            .iter()
            .filter(|id| round.contains(id))
            .copied()
            .collect();
        if still.is_empty() {
            return;
        }
        let mut told = self.told_unread_for_now;
        told.write().extend(still.iter().copied());
        let telling = self.app().store.still_unread(still);
        let mut notices = self.notices;
        settle_or_spawn(telling, move |said| {
            if !said.is_empty() {
                notices
                    .write()
                    .extend(said.iter().map(|notice| Notice::History(notice.text())));
            }
        });
    }

    /// Take up what the store holds now, whatever it says of having
    /// changed: as sync has just changed it ([`self::sync`]).
    pub(crate) fn take_up_store(self) {
        self.sync_with_store(|_| {});
    }

    /// Take up whatever another tab, window or copy of the app stored or
    /// deleted since this one last looked, where the store says anything
    /// may have changed, or a record it listed could not be read for now
    /// ([`Self::unread_for_now`]): as a screen that shows the history
    /// opens, and as the page comes back into sight.
    pub fn take_up_changes(self) {
        if self.app().store.history_changed() || !self.unread_for_now.peek().is_empty() {
            self.sync_with_store(|_| {});
        }
    }

    /// Take up each change another copy of the app tells of as it tells of
    /// it, where the store hears of them ([`Store::history_news`]): a tab
    /// left showing the history beside another shows what the other stores
    /// and deletes, without waiting for a screen to open.
    fn hear_history_news(self) {
        let store = self.app().store;
        let Some(first) = store.history_news() else {
            return;
        };
        spawn_forever(async move {
            let mut news = first;
            loop {
                news.await;
                self.take_up_changes();
                match store.history_news() {
                    Some(next) => news = next,
                    None => return,
                }
            }
        });
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
            if saved.kept().is_some() {
                self.sync_soon();
            }
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

    /// As the app closes: wait, a moment at most, for the store to make
    /// every change asked of the history, and take up there and then what
    /// became of each, rather than leave it to tasks the app's going cuts
    /// short. A session kept as the app closes is stored, and its mark
    /// goes, before the app does; one the store is still making is kept
    /// by its mark, for the next launch.
    pub fn flush_history(self) {
        self.app().store.flush_history();
        crate::history::settle_waiting();
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
    /// session in it, and store it, then do `then` with what became of it;
    /// and hand back the record it is kept under, if it could be written
    /// down. It is kept here until the store has taken it, and carried by
    /// this copy's mark meanwhile.
    pub fn keep_finished(
        self,
        session: SessionResult,
        then: impl FnOnce(HistorySaved) + 'static,
    ) -> Option<RecordId> {
        let mut sessions = self.sessions;
        let newest = sessions.peek().newest_id();
        let Some(record) = Record::of(&session, |json| {
            RecordId::next(session.timestamp, newest, json)
        }) else {
            then(HistorySaved::Failed("it could not be written down.".into()));
            return None;
        };
        let id = record.id;
        {
            let mut history = sessions.write();
            history.insert(id, session);
            history.mark_unsaved(id);
        }
        self.change_history(
            HistoryChange {
                put: vec![record],
                ..HistoryChange::default()
            },
            then,
        );
        Some(id)
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

    /// Let go of `gone`, sessions held from elsewhere
    /// ([`Self::hold_from_elsewhere`]) that the store says were deleted by
    /// hand under the records they would be stored as: out of the history,
    /// and out of this copy's mark.
    pub fn let_go_of_held(self, gone: &HashSet<RecordId>) {
        if gone.is_empty() {
            return;
        }
        let mut sessions = self.sessions;
        {
            let mut history = sessions.write();
            for id in gone {
                history.remove(*id);
            }
        }
        self.settle_mark();
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
                None => {
                    toasts.show_with(
                        format!("{} deleted", counted(count, "session")),
                        ToastAction::UndoDelete,
                    );
                }
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
                return;
            }
            // Back by hand: a deletion the drive holds no longer stands.
            self.sync_revived(taken.iter().map(|(id, _)| *id));
            if let Some(problem) = saved.problem() {
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
    /// It holds every session the store holds, each as stored, and those
    /// here it has yet to take: the store writes it a session at a time
    /// ([`BackupFile`]), so a history of any length is never held whole to
    /// be backed up. A store that cannot read all it holds, its history
    /// unlisted or a session in it unread, saves nothing and says so; the
    /// backup is then saved holding every session here as well, and holds
    /// what the app shows, as it did before stores were read a session at
    /// a time. It is how the sessions only the app holds are kept safe.
    pub fn save_backup(self) {
        self.sync_with_store(move |_| self.save_backup_now());
    }

    fn save_backup_now(self) {
        let at = now_ms();
        let backup = {
            let history = self.sessions.peek();
            let unsaved = history
                .unsaved()
                .iter()
                .filter_map(|id| history.record(*id))
                .collect();
            BackupFile::new(&self.settings.peek(), at, unsaved)
                .beside_unread(self.unread_here(&history))
        };
        let name = backup_name(&crate::time::local_date_string());
        let saving = self.app().store.save_backup(&name, backup);
        settle_or_spawn(saving, move |saved| match saved {
            // Only now is the whole history copied for it.
            Err(NotSaved::Unread) => {
                let every = self.backup_of_every_session(at);
                let saving = self.app().store.save_backup(&name, every);
                settle_or_spawn(saving, move |saved| self.backup_saved(at, saved));
            }
            saved => self.backup_saved(at, saved),
        });
    }

    /// A backup saved at `at` that holds every session here, each as this
    /// build writes it: for a store that could not read all it holds.
    fn backup_of_every_session(self, at: u64) -> BackupFile {
        let history = self.sessions.peek();
        let every = history
            .ids()
            .iter()
            .filter_map(|id| history.record(*id))
            .collect();
        BackupFile::new(&self.settings.peek(), at, every)
            .beside_unread(self.unread_here(&history))
            .holding_every_session()
    }

    /// Every record the store holds that `history` has no session of, as
    /// this build cannot read it, or could not for now
    /// ([`Self::unread_for_now`]): a backup has them as the store holds
    /// them, or not at all.
    fn unread_here(self, history: &StoredHistory) -> BTreeSet<RecordId> {
        let mut unread = history.unread().clone();
        unread.extend(self.unread_for_now.peek().iter().copied());
        unread
    }

    /// Tell of the backup asked for at `at` as `saved` says, and remember
    /// it once it is saved.
    fn backup_saved(self, at: u64, saved: Result<String, NotSaved>) {
        match saved {
            Ok(said) => {
                let store = self.app().store;
                store.mark_backup(at);
                let mut last_backup = self.last_backup;
                last_backup.set(Some(at));
                // Kept inside the app, it is no backup until moved out.
                let said = match store.backups_inside_app() {
                    Some(how) => {
                        format!("{said} It goes if Dust is deleted: move it out {how} to keep it.")
                    }
                    None => said,
                };
                self.toasts.show(said);
            }
            Err(not) => self
                .toasts
                .show(format!("The backup could not be saved: {}.", not.why())),
        }
    }

    /// Put back what `backup` holds: every session of it not already here,
    /// and its settings, level and all, where it holds any, but for this
    /// device's own ([`TrainingSettings::keep_device_settings`]). Until
    /// another restore is asked for, Undo puts both back as they were
    /// ([`Self::undo_restore`]). A store that cannot take the history
    /// leaves everything as it was and says why, and so does one that
    /// cannot be listed for now: merged with what this copy last saw of the
    /// history, the backup would bring back, a second time, a session
    /// another copy stored since. So does one that lists a record it cannot
    /// read for now, the first time a restore finds it so: one still unread
    /// at a later restore is taken as one that cannot be read at all, and
    /// refuses no restore again. A session of the backup that a record
    /// stored here may hold, though this build cannot read it, is looked at
    /// first ([`Self::restore_beside`]).
    ///
    /// Each session it adds is a record where it finished
    /// ([`RecordId::placed`]), so two copies restoring one backup store it
    /// once; one it fills in is a record in its place
    /// ([`RecordId::beside`]), the one it fills in deleted.
    ///
    /// One at a time, and between sessions: a restore asked for while
    /// another is under way, as a store that takes a moment to list or
    /// change the history leaves one, or while the Undo of one is, is made
    /// once that is over, merged with the history it left. Merged with a
    /// history short of what the other brings back, it would take that for
    /// its own, and its Undo take it back as well; merged with one that
    /// still holds what the Undo takes back, it would take that for
    /// sessions already here, and the Undo delete them. One asked for while
    /// a session or a run has the app is made once that is over, as a sync
    /// is, never under it: a restore takes its settings, the level among
    /// them. Of those asked for meanwhile, the last is made, and none
    /// before it.
    ///
    /// Gives back the number it is known by until what became of it has
    /// been told ([`Self::restore_told`]).
    pub fn restore_backup(self, backup: Backup) -> u64 {
        let mut asked = self.restores_asked;
        let ticket = *asked.peek() + 1;
        asked.set(ticket);
        self.restore_in_turn(ticket, backup);
        ticket
    }

    /// Whether what became of the restore asked for under `ticket` has been
    /// told ([`Self::restore_backup`]): it is neither waiting its turn nor
    /// under way, whatever else is, as the Undo of it, which may follow at
    /// once. One that another asked for after it took the place of is told
    /// of as soon as it is, though it is never made.
    pub fn restore_told(self, ticket: u64) -> bool {
        ticket <= *self.restores_asked.peek()
            && *self.current_restore.peek() != Some(ticket)
            && !self.waits(ticket)
    }

    /// Whether the restore asked for under `ticket` waits its turn.
    fn waits(self, ticket: u64) -> bool {
        self.restores_waiting
            .peek()
            .as_ref()
            .is_some_and(|(waiting, _)| *waiting == ticket)
    }

    /// Whether the restore asked for under `ticket` waits for another copy
    /// of the app, a tab or a window, whose restore, or the Undo of it, has
    /// every copy's restores ([`Lock::Restore`]): as the card says.
    pub fn restore_blocked(self, ticket: u64) -> bool {
        *self.restore_blocked.peek() && self.waits(ticket)
    }

    /// The restore asked for under `ticket`, where it still waits its turn,
    /// is made no more: another backup was chosen in its place, to be asked
    /// for once it is read.
    pub fn withdraw_restore(self, ticket: u64) {
        if self.waits(ticket) {
            let mut waiting = self.restores_waiting;
            waiting.set(None);
            self.let_go_of_restores();
        }
    }

    /// Restore `backup`, asked for under `ticket` ([`Self::restore_backup`]),
    /// now, or once no other restore, nor Undo, is under way, no session or
    /// run has the app, and no other copy of the app has every copy's
    /// restores: the last asked for until then, in place of any asked for
    /// before.
    ///
    /// One copy at a time: a restore holds every copy's restores from its
    /// start until neither it nor its Undo is under way or offered
    /// ([`Lock::Restore`]). A restore made in another copy beside this one,
    /// while its Undo was offered, took what both bring back for sessions
    /// already here, and this one's Undo then deleted them.
    fn restore_in_turn(self, ticket: u64, backup: Backup) {
        let mut under_way = self.restore_under_way;
        let mut current = self.current_restore;
        if *under_way.peek() || self.session_running() {
            self.wait_turn(ticket, backup);
            self.offer_no_undo();
            if !*under_way.peek() {
                self.restore_when_free();
            }
            return;
        }
        // Under way from now, before the Undo of the one before is offered
        // no more: this copy keeps every copy's restores across.
        under_way.set(true);
        current.set(Some(ticket));
        self.offer_no_undo();
        self.with_restore_lock(move |taken| {
            // Another copy has them; or, as a browser grants a lock a
            // moment after it is asked for, a session began meanwhile.
            if !taken || self.session_running() {
                under_way.set(false);
                current.set(None);
                self.wait_turn(ticket, backup);
                if !taken {
                    let mut blocked = self.restore_blocked;
                    blocked.set(true);
                }
                self.restore_when_free();
                return;
            }
            self.restore_held(backup);
        });
    }

    /// The restore before is undone no more, as another is asked for, and
    /// its message, while it is up, offers no Undo that would do nothing.
    fn offer_no_undo(self) {
        let mut before_restore = self.before_restore;
        if before_restore.peek().is_some() {
            before_restore.set(None);
            self.toasts.withdraw(ToastAction::UndoRestore);
        }
    }

    /// The restore asked for under `ticket` waits its turn, unless one asked
    /// for after it waits already, as one taken from the queue as a session
    /// began may find: that one is made in its place.
    fn wait_turn(self, ticket: u64, backup: Backup) {
        let mut waiting = self.restores_waiting;
        let later = waiting
            .peek()
            .as_ref()
            .is_some_and(|(waiting, _)| *waiting > ticket);
        if !later {
            waiting.set(Some((ticket, backup)));
        }
        let mut current = self.current_restore;
        if *current.peek() == Some(ticket) {
            current.set(None);
        }
    }

    /// `then`, told whether this copy has every copy's restores now
    /// ([`Lock::Restore`]): it holds them already, or takes them, or the
    /// store takes no such lock, and goes on without, as copies did before
    /// they locked; or another copy has them.
    fn with_restore_lock(self, then: impl FnOnce(bool) + 'static) {
        if self.restore_lock.peek().is_some() {
            then(true);
            return;
        }
        let mut held = self.restore_lock;
        let taking = self.app().store.try_lock(Lock::Restore);
        settle_or_spawn(taking, move |locking| match locking {
            Locking::Held(lock) => {
                held.set(Some(lock));
                then(true);
            }
            Locking::Unlockable => then(true),
            Locking::Busy => then(false),
        });
    }

    /// Let go of every copy's restores ([`Lock::Restore`]), unless a
    /// restore or an Undo is under way here, a restore waits, or an Undo is
    /// offered.
    fn let_go_of_restores(self) {
        let busy = *self.restore_under_way.peek()
            || self.restores_waiting.peek().is_some()
            || self.before_restore.peek().is_some();
        if !busy && self.restore_lock.peek().is_some() {
            let mut held = self.restore_lock;
            held.set(None);
        }
    }

    /// The message offering the Undo of a restore, `serial`, offers it no
    /// more: taken down, as one is after a while, or put in the place of by
    /// another message. The restore is undone no more, as there is nothing
    /// left to undo it with, and every copy's restores are let go of.
    fn undo_offered_no_more(self, action: ToastAction, serial: u64) {
        let mut undo_toast = self.undo_toast;
        if action != ToastAction::UndoRestore || *undo_toast.peek() != Some(serial) {
            return;
        }
        undo_toast.set(None);
        let mut before_restore = self.before_restore;
        before_restore.set(None);
        self.let_go_of_restores();
    }

    /// Restore `backup`, now, every copy's restores held
    /// ([`Self::restore_in_turn`]).
    fn restore_held(self, backup: Backup) {
        self.sync_with_store(move |listed| {
            if listed {
                self.restore_beside(backup);
            } else {
                self.restore_refused(match why_not_stored(&HistorySaved::Held) {
                    Some(why) => format!("Nothing was restored: {why}."),
                    None => "Nothing was restored.".into(),
                });
            }
        });
    }

    /// Restore `backup` ([`Self::restore_backup`]), the store looked at:
    /// what it listed that could not be read for now was read again, and
    /// taken up where it read ([`Self::unread_for_now`]).
    ///
    /// A record here that a later build stored, as it says
    /// ([`cw_core::SESSION_FORMAT`]), leaves out each session of the backup
    /// that finished when, and on the day, it says its own did, whatever
    /// the record is named, and the user is told: merged with a history
    /// short of it, the backup would store that session a second time,
    /// beside the record a later build shows.
    ///
    /// Each other record here this build could not read, as the app opened
    /// or as a look found it, that is named for when a session of the
    /// backup not here finished is read again first, as that session may
    /// be its own:
    /// - one that reads now, as one another program was still writing in
    ///   place as it was read does, is taken up, the backup's session
    ///   merged with it, and the user told;
    /// - one that says now that a later build stored it is taken as such;
    /// - one that holds no session at all, damaged, though it may be JSON
    ///   still that says when its session finished, or that cannot be read
    ///   now, leaves the backup's session to be restored beside it: no
    ///   build may ever read it, and the backup may hold the only copy of
    ///   that session there is.
    ///
    /// What the app told of unread records as it opened is not taken back
    /// where one reads now.
    fn restore_beside(self, backup: Backup) {
        let (again, later) = {
            let history = self.sessions.peek();
            let finished: HashSet<u64> = backup
                .sessions
                .iter()
                .filter(|session| !history.has_key(&SessionKey::of(session)))
                .map(|session| session.timestamp)
                .collect();
            let again: Vec<RecordId> = history
                .unread()
                .iter()
                .filter(|id| finished.contains(&(id.order / 1000)))
                .copied()
                .collect();
            (again, history.later_keys().cloned().collect())
        };
        let beside = Beside { later, read_now: 0 };
        if again.is_empty() {
            self.restore_unless_unread(backup, beside);
            return;
        }
        let reading = self.app().store.read_records(again);
        settle_or_spawn(reading, move |read| {
            let mut beside = beside;
            let mut reads_now = Vec::new();
            let mut said_now = Vec::new();
            // One that still cannot be read is left as it is, as damaged.
            for record in read.into_iter().flatten() {
                match read_record(&record.json) {
                    Some(session) => reads_now.push((record.id, session)),
                    None => {
                        let said = unreadable(&record.json);
                        if said.later {
                            beside.later.extend(said.key.clone());
                        }
                        said_now.push((record.id, said));
                    }
                }
            }
            beside.read_now = reads_now.len();
            if !reads_now.is_empty() || !said_now.is_empty() {
                let mut sessions = self.sessions;
                let mut history = sessions.write();
                for (id, session) in reads_now {
                    history.read_after_all(id, session);
                }
                for (id, said) in &said_now {
                    history.mark_unread(*id, said);
                }
            }
            self.restore_unless_unread(backup, beside);
        });
    }

    /// Restore `backup` as `beside` says ([`Self::restore_beside`]), unless
    /// the store listed a record it could not read for now, at this look or
    /// before ([`Self::unread_for_now`]), that no restore was refused for
    /// yet: it may hold a session of the backup, which would be stored a
    /// second time beside it once it reads. That refuses this one, and the
    /// user is told why, and that asking again goes ahead.
    fn restore_unless_unread(self, backup: Backup, beside: Beside) {
        let mut refused_for = self.restore_refused_for;
        let new: Vec<RecordId> = {
            let refused = refused_for.peek();
            self.unread_for_now
                .peek()
                .iter()
                .filter(|id| !refused.contains(id))
                .copied()
                .collect()
        };
        if new.is_empty() {
            self.restore_now(backup, beside);
        } else {
            refused_for.write().extend(new);
            self.restore_refused(
                "Nothing was restored: a session stored here could not be read just now, and the backup could store it a second time. Restore it again to go ahead all the same.".into(),
            );
        }
    }

    /// Nothing is restored, and the user is told so, `said`
    /// ([`Self::restore_backup`]).
    fn restore_refused(self, said: String) {
        self.toasts.show(said);
        self.restore_over();
    }

    /// Whether a restore, or the Undo of one, is under way
    /// ([`Self::restore_backup`]).
    #[cfg(test)]
    pub fn restore_under_way(self) -> bool {
        *self.restore_under_way.peek()
    }

    /// Whether a restore asked for waits its turn in the app
    /// ([`Self::restore_backup`]).
    #[cfg(test)]
    pub fn restore_waits(self) -> bool {
        self.restores_waiting.peek().is_some()
    }

    /// The restore under way, or the Undo of one, is over, what became of
    /// it told: the restore asked for meanwhile is made next, in a task of
    /// its own, and is under way until then, so that one asked for in
    /// between waits behind it; or, where a session or a run has the app,
    /// once that is over. What became of a change is told of as soon as the
    /// store answers, before it is taken up here
    /// ([`Self::change_history`]): what rode along with it marked stored,
    /// and what the store let go for room gone. Made then and there, the
    /// next would be merged with a history that still holds what the store
    /// let go, and take it for a session lost in passing, to be stored
    /// again.
    ///
    /// The restore over is told of from now ([`Self::restore_told`]), and
    /// the next is under way, not told of until it is over in turn.
    fn restore_over(self) {
        let mut under_way = self.restore_under_way;
        let mut waiting = self.restores_waiting;
        let next = if self.session_running() {
            None
        } else {
            waiting.write().take()
        };
        let mut current = self.current_restore;
        current.set(next.as_ref().map(|(ticket, _)| *ticket));
        let Some((ticket, next)) = next else {
            under_way.set(false);
            if waiting.peek().is_some() {
                self.restore_when_free();
            }
            // Kept while its Undo is offered.
            self.let_go_of_restores();
            return;
        };
        spawn_forever(async move {
            under_way.set(false);
            self.restore_in_turn(ticket, next);
        });
    }

    /// Make the restore waiting once it may be: once the session or run
    /// that has the app is over, as one made under it would change its
    /// settings, the level among them; and once no other copy of the app
    /// has every copy's restores ([`Lock::Restore`]), as the card says
    /// meanwhile ([`Self::restore_blocked`]). In a task of its own, looking
    /// again every [`RESTORE_WAIT_MS`] while a session runs, and every
    /// [`RESTORE_LOCK_POLL_MS`] while another copy has the restores, and
    /// one task however many restores are asked for meanwhile. No task
    /// waits where neither holds a restore back: it is made then and
    /// there.
    fn restore_when_free(self) {
        let mut waits = self.restore_waiter;
        if *waits.peek() {
            return;
        }
        waits.set(true);
        spawn_forever(async move {
            let (mut blocked, mut held) = (self.restore_blocked, self.restore_lock);
            loop {
                // Withdrawn; or a restore or an Undo under way, which makes
                // it as it ends.
                if self.restores_waiting.peek().is_none() || *self.restore_under_way.peek() {
                    break;
                }
                if self.session_running() {
                    sleep_ms(RESTORE_WAIT_MS).await;
                    continue;
                }
                if held.peek().is_some() {
                    break;
                }
                let taking = self.app().store.try_lock(Lock::Restore);
                match taking.await {
                    Locking::Held(lock) => {
                        held.set(Some(lock));
                        break;
                    }
                    Locking::Unlockable => break,
                    Locking::Busy => {
                        blocked.set(true);
                        sleep_ms(RESTORE_LOCK_POLL_MS).await;
                    }
                }
            }
            blocked.set(false);
            waits.set(false);
            if !*self.restore_under_way.peek() {
                let mut waiting = self.restores_waiting;
                let next = waiting.write().take();
                if let Some((ticket, next)) = next {
                    self.restore_in_turn(ticket, next);
                }
            }
            self.let_go_of_restores();
        });
    }

    /// Restore `backup` ([`Self::restore_backup`]), as `beside` says: each
    /// session of it that a record a later build stored here says it holds
    /// is left out, where the history holds none like it
    /// ([`Self::restore_beside`]).
    fn restore_now(self, backup: Backup, beside: Beside) {
        // As of when it was saved, or now, if a clock set ahead says later.
        let saved_at = backup.saved_at.min(now_ms());
        let at = now_ms();
        // What it adds was deleted by hand, maybe; it is wanted back now.
        let mut change = HistoryChange {
            restore: true,
            ..HistoryChange::default()
        };
        let (backed_up, merged, filled_ids, beside_unread) = {
            let history = self.sessions.peek();
            let backed_up = covers_up_to(&backup.sessions, &history, saved_at);
            let (sessions, left_beside): (Vec<SessionResult>, Vec<SessionResult>) =
                backup.sessions.into_iter().partition(|session| {
                    let key = SessionKey::of(session);
                    !beside.later.contains(&key) || history.has_key(&key)
                });
            // Each session once, however often the backup holds it.
            let beside_unread = left_beside
                .iter()
                .map(SessionKey::of)
                .collect::<HashSet<_>>()
                .len();
            let merged = merge_sessions(&history, sessions);
            // Each it fills in by its record, where it is now.
            let filled_ids: Vec<Option<RecordId>> = merged
                .filled
                .iter()
                .map(|fill| history.id_at(fill.at))
                .collect();
            (backed_up, merged, filled_ids, beside_unread)
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
        for (Filled { was, now, .. }, was_id) in merged.filled.into_iter().zip(filled_ids) {
            let Some(was_id) = was_id else {
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
        let brought = !added.is_empty() || !filled.is_empty();
        if brought {
            // A restore of thousands of sessions takes a moment to store.
            self.toasts.show(format!(
                "Restoring {}…",
                counted(added.len() + filled.len(), "session")
            ));
        }
        let restored_at = backup.saved_at;
        let restored_settings = backup.settings;
        let left_out = backup.left_out;
        self.change_history(change, move |saved| {
            self.restored(
                saved,
                Restoring {
                    added,
                    filled,
                    brought,
                    left_out,
                    beside_unread,
                    read_now: beside.read_now,
                    backed_up,
                    saved_at,
                    restored_at,
                    settings: restored_settings,
                },
            );
            self.restore_over();
        });
    }

    /// What a restore comes to, once the store has said what became of it
    /// ([`Self::restore_backup`]): its Undo offered, unless another restore
    /// was asked for while it was under way.
    fn restored(self, saved: HistorySaved, restoring: Restoring) {
        let Restoring {
            added,
            filled,
            brought,
            left_out,
            beside_unread,
            read_now,
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
        // Wanted back, whatever a synced deletion said of them.
        self.sync_revived(
            added
                .iter()
                .map(|(id, _)| *id)
                .chain(filled.iter().map(|fill| fill.id)),
        );
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
        // Every session in it was here already: it brought none, and left
        // none out, which may not be here.
        let all_here = !brought && left_out == 0 && beside_unread == 0 && read_now == 0;
        let mut text = match (added_count, filled_count, with_settings) {
            (0, 0, true) if all_here => {
                format!("Restored your settings{from}. Every session in it was already here.")
            }
            (0, 0, false) if all_here => "Every session in it was already here.".into(),
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
        if let Some(told) = Backup::told_left_out(left_out) {
            text = format!("{text} {told}");
        }
        if let Some(told) = told_beside_unread(beside_unread) {
            text = format!("{text} {told}");
        }
        if let Some(told) = told_read_now(read_now) {
            text = format!("{text} {told}");
        }
        if let Some(problem) = saved.problem() {
            text = format!("{text} {problem}");
        }
        // A restore asked for while this one was under way, and waiting
        // for it, is the last one asked for: this one is undone no more.
        if self.restores_waiting.peek().is_none() {
            let mut before_restore = self.before_restore;
            before_restore.set(Some(Restored {
                added,
                filled,
                settings: before,
            }));
            let serial = self.toasts.show_with(text, ToastAction::UndoRestore);
            let mut undo_toast = self.undo_toast;
            undo_toast.set(Some(serial));
        } else {
            self.toasts.show(text);
        }
    }

    /// Put the history and settings back as they were before the last
    /// restore: what it added deleted, and what it filled in as it was,
    /// under the record it had. Whatever else the history took up since,
    /// here or in another copy of the app, stays: only what the restore
    /// changed, and is here still, is undone.
    ///
    /// Only until another restore is asked for ([`Self::restore_backup`]):
    /// from then on there is nothing to undo, and an Undo clicked while
    /// that one is under way, its message still up, does nothing. The
    /// restore asked for is merged with a history that holds what this
    /// would take back, and takes it for sessions already here: undone
    /// beneath it, or once it is made, they would be lost, though the
    /// backup it restores holds them. And an Undo is under way as a
    /// restore is, until the store has said what became of it: a restore
    /// asked for meanwhile waits for it.
    ///
    /// Asked for while a session or a run has the app, its message still
    /// up, it is made once that is over, as a restore is
    /// ([`Self::restore_backup`]), and the user is told so: it puts settings
    /// back, the level among them, which a session is not to have changed
    /// beneath it. It is under way meanwhile, so a restore asked for after
    /// it waits behind it. Asked for just before one began, and made by the
    /// store only once it has, as a browser's database takes a moment to,
    /// it takes out what the restore added then, and puts the settings back
    /// once the session or run is over ([`Self::undo_now`]). An Undo that
    /// waits for a session waits with this copy of the app: closed first,
    /// it makes none, and the restore stands, or its settings do.
    pub fn undo_restore(self) {
        let mut before_restore = self.before_restore;
        let Some(restored) = before_restore.write().take() else {
            return;
        };
        // Its message offers what is under way now: taken down, it says no
        // more of it.
        let mut undo_toast = self.undo_toast;
        undo_toast.set(None);
        let mut under_way = self.restore_under_way;
        under_way.set(true);
        if self.session_running() {
            let what = if self.contest.peek().is_some() {
                "run"
            } else {
                "session"
            };
            self.toasts.show(format!(
                "The restore will be undone once this {what} is over."
            ));
            spawn_forever(async move {
                while self.session_running() {
                    sleep_ms(RESTORE_WAIT_MS).await;
                }
                self.undo_now(restored);
            });
            return;
        }
        self.undo_now(restored);
    }

    /// Undo `restored` now ([`Self::undo_restore`]), as the history holds
    /// it now: the Undo is under way.
    fn undo_now(self, restored: Restored) {
        let mut before_restore = self.before_restore;
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
                // Still to undo, and offered again, unless another restore
                // was asked for meanwhile: offered, as an Undo held with
                // nothing to undo it with would hold every copy's restores
                // for good.
                let said = format!("The restore could not be undone: {why}.");
                if self.restores_waiting.peek().is_none() {
                    before_restore.set(Some(restored));
                    let serial = toasts.show_with(said, ToastAction::UndoRestore);
                    let mut undo_toast = self.undo_toast;
                    undo_toast.set(Some(serial));
                } else {
                    toasts.show(said);
                }
                self.restore_over();
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
            // Asked for before a session or run that has begun since, as
            // the store takes a moment to answer: the settings go back once
            // it is over, as for an Undo asked for under it, and the Undo
            // is under way until then, so a restore asked for meanwhile is
            // made after it.
            let settings = restored.settings;
            if self.session_running() {
                spawn_forever(async move {
                    while self.session_running() {
                        sleep_ms(RESTORE_WAIT_MS).await;
                    }
                    self.undone(settings);
                });
            } else {
                self.undone(settings);
            }
        });
    }

    /// The Undo of a restore is over, the history as it was before the
    /// restore ([`Self::undo_now`]): `settings`, those from before it, put
    /// back where it took any, and the restore asked for meanwhile made.
    fn undone(self, settings: Option<TrainingSettings>) {
        if let Some(before) = settings {
            let mut settings = self.settings;
            settings.set(before);
        }
        self.restore_over();
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
    /// Each record listed that was not here, as it is stored, or why it
    /// could not be read.
    read: Vec<Result<Record, Unopened>>,
    /// Each record here, the store had taken, that was no longer listed.
    missing: Vec<RecordId>,
    /// Of those, the ones deleted by hand.
    forgotten: Vec<RecordId>,
    /// Each record here this build could not read that was no longer
    /// listed.
    unread_gone: Vec<RecordId>,
    /// Each record that could not be read for now that was no longer
    /// listed ([`AppCtx::unread_for_now`]).
    failed_gone: Vec<RecordId>,
}

/// A restore on its way to the store ([`AppCtx::restored`]).
struct Restoring {
    added: Vec<(RecordId, SessionResult)>,
    filled: Vec<FilledIn>,
    /// Whether the backup held anything the history did not.
    brought: bool,
    /// How many of its sessions this version could not read, and left out.
    left_out: usize,
    /// How many of its sessions it left out, as records a later build
    /// stored here say they hold them ([`AppCtx::restore_beside`]).
    beside_unread: usize,
    /// How many records here that could not be read before read now, and
    /// were taken up ([`AppCtx::restore_beside`]).
    read_now: usize,
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

/// What a restore found of the records here that this build could not read
/// ([`AppCtx::restore_beside`]).
#[derive(Default)]
struct Beside {
    /// The sessions records a later build stored here say they hold: those
    /// of the backup are left out.
    later: HashSet<SessionKey>,
    /// How many records here that could not be read before read now.
    read_now: usize,
}

/// What to say of the sessions of a backup a restore left out, `count` of
/// them, as records a later build stored here say they hold them
/// ([`AppCtx::restore_beside`]): nothing, where there are none.
fn told_beside_unread(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some(
            "1 session in it was left out as stored here already, by a newer Dust, which shows it."
                .to_string(),
        ),
        count => Some(format!(
            "{count} sessions in it were left out as stored here already, by a newer Dust, which shows them."
        )),
    }
}

/// What to say of the records here that could not be read before, `count`
/// of them, that a restore found it could read now, and took up
/// ([`AppCtx::restore_beside`]): nothing, where there are none.
fn told_read_now(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some("1 session stored here that could not be read before reads now.".to_string()),
        count => Some(format!(
            "{count} sessions stored here that could not be read before read now."
        )),
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

/// Whether `cut` is a mark of a session or run ended and kept, every
/// session it carries held by `history` already: left behind only for want
/// of the store's word that it had taken them, as a page closed under a
/// browser's database leaves it. Nothing of it was cut off, nor lost.
fn kept_already(cut: &CutOff, history: &StoredHistory) -> bool {
    let CutOff::Marked(open) = cut else {
        return false;
    };
    open.over
        && open
            .so_far
            .iter()
            .map(|so_far| &so_far.result)
            .chain(&open.unstored)
            .all(|session| history.has_key(&SessionKey::of(session)))
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
    /// Kept, with the record it is kept under in the history, by which its
    /// place there and on the board is found, as two sessions can finish at
    /// one moment: none where it could not be written down, and so is in
    /// no history. Followed to the one record holding it where a take-up
    /// lets that one go ([`AppCtx::follow_the_session_kept`]).
    Kept(Box<SessionResult>, Option<RecordId>),
    /// A run that worked nobody, so nothing was kept.
    NothingKept,
}

#[cfg(test)]
impl Finished {
    /// The session kept, if one was.
    pub fn kept(self) -> Option<SessionResult> {
        match self {
            Self::Kept(result, _) => Some(*result),
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

/// What is told as a message stops offering what it offered
/// ([`Toasts::when_offered_no_more`]): the offer, and which message it was.
type OfferedNoMore = Rc<dyn Fn(ToastAction, u64)>;

/// Where messages are raised and shown: one at a time, the newest in place
/// of whatever was up.
#[derive(Clone, Copy, PartialEq)]
pub struct Toasts {
    shown: Signal<Option<Toast>>,
    /// Told as a message stops offering what it offered: taken down, its
    /// offer withdrawn, or another raised in its place.
    no_more: CopyValue<Option<OfferedNoMore>>,
}

impl Toasts {
    /// Messages for the scope this is made in.
    pub fn new() -> Self {
        Self {
            shown: Signal::new(None),
            no_more: CopyValue::new(None),
        }
    }

    /// Have `told` told as each message stops offering what it offered, by
    /// what that was and which message it was ([`Self::show_with`]): taken
    /// down, as each is after a while, its offer withdrawn, or another
    /// raised in its place.
    pub fn when_offered_no_more(self, told: impl Fn(ToastAction, u64) + 'static) {
        let mut no_more = self.no_more;
        no_more.set(Some(Rc::new(told)));
    }

    pub fn show(self, text: impl Into<String>) {
        self.raise(text.into(), None, false);
    }

    /// A message of a sentence or two, up as long as one with something to
    /// do about it, to be read through.
    pub fn show_long(self, text: impl Into<String>) {
        self.raise(text.into(), None, true);
    }

    /// A message with something to do about it: which message it is, as
    /// it is told of once it offers it no more
    /// ([`Self::when_offered_no_more`]).
    pub fn show_with(self, text: impl Into<String>, action: ToastAction) -> u64 {
        self.raise(text.into(), Some(action), false)
    }

    fn raise(self, text: String, action: Option<ToastAction>, long: bool) -> u64 {
        static RAISED: AtomicU64 = AtomicU64::new(0);
        let serial = RAISED.fetch_add(1, Ordering::Relaxed);
        let mut shown = self.shown;
        let was = shown.peek().clone();
        shown.set(Some(Toast {
            text,
            action,
            long,
            serial,
        }));
        self.offers_no_more(was.as_ref());
        serial
    }

    /// Tell that `toast`, gone, offers what it offered no more.
    fn offers_no_more(self, toast: Option<&Toast>) {
        let Some((action, serial)) = toast.and_then(|toast| Some((toast.action?, toast.serial)))
        else {
            return;
        };
        let told = self.no_more.peek().clone();
        if let Some(told) = told {
            told(action, serial);
        }
    }

    /// The message up now. Read from a component, it subscribes it.
    pub fn current(self) -> Option<Toast> {
        self.shown.cloned()
    }

    /// The message up, where it offers `action`, offers it no more: what it
    /// says stays up, with nothing to do about it.
    pub fn withdraw(self, action: ToastAction) {
        let mut shown = self.shown;
        let offers = shown
            .peek()
            .as_ref()
            .filter(|toast| toast.action == Some(action))
            .cloned();
        if let Some(toast) = offers {
            if let Some(up) = shown.write().as_mut() {
                up.action = None;
            }
            self.offers_no_more(Some(&toast));
        }
    }

    /// Take `toast` down, unless a newer one has taken its place.
    pub fn dismiss(self, toast: &Toast) {
        let mut shown = self.shown;
        if shown.peek().as_ref() == Some(toast) {
            shown.set(None);
            self.offers_no_more(Some(toast));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "native-runtime")]
    use crate::persist::TempDir;
    use crate::persist::memory::{MarkEvent, MemoryStore, PageShelf};
    use crate::persist::record::HistoryNotice;

    /// A copy of the app — a tab, or a window — on `store`, which other
    /// copies may share, opened on what it holds.
    struct Tab {
        dom: VirtualDom,
        ctx: AppCtx,
    }

    impl Tab {
        fn on(store: &Rc<MemoryStore>) -> Self {
            Self::on_store(store.clone())
        }

        /// A copy of the app on `store`, whatever store it is.
        fn on_store(store: Rc<dyn Store>) -> Self {
            let dom = VirtualDom::new(|| rsx! {});
            let (settings, history) = (store.load_settings(), store.load_history());
            let app = AppState::with(crate::audio::fake::Recorder::new().factory(), store);
            let ctx = dom.in_scope(ScopeId::ROOT, || AppCtx::new(app, settings, history));
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
            self.act(|ctx| crate::state::keep_and_show(ctx, session));
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
            self.act(|ctx| ctx.sync_with_store(|_| {}));
        }

        /// The days of the sessions this copy shows.
        fn shown(&self) -> Vec<u64> {
            self.act(|ctx| days(&ctx.sessions.peek()))
        }

        fn notices(&self) -> Vec<Notice> {
            self.act(|ctx| ctx.notices.peek().clone())
        }

        /// Run whatever this copy's tasks are ready to do.
        fn pump(&mut self) {
            self.dom.process_events();
        }

        /// Move the clock on by `ms`, a tenth of a second at a time, doing
        /// whatever this copy's tasks are ready to do as it goes: in a test
        /// run on a paused clock ([`crate::testing::run`]).
        async fn advance(&mut self, ms: u64) {
            for _ in 0..ms.div_ceil(100) {
                tokio::time::advance(std::time::Duration::from_millis(100)).await;
                tokio::task::yield_now().await;
                self.pump();
            }
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

    /// A tab that hears of the changes other tabs make, as a browser's tabs
    /// tell each other, takes each up as it is told of it: what another
    /// stores or deletes shows at once, with no screen opened for it.
    #[test]
    fn a_change_another_tab_tells_of_is_taken_up_at_once() {
        let store = holding([1]);
        let theirs = Tab::on(&store);
        store.hears_news.set(true);
        let mut ours = Tab::on(&store);
        ours.pump();
        theirs.finish(day(2));
        ours.pump();
        assert_eq!(ours.shown(), [1], "not told yet");
        store.tell_news();
        ours.pump();
        assert_eq!(ours.shown(), [1, 2]);
        theirs.delete(2);
        store.tell_news();
        ours.pump();
        assert_eq!(ours.shown(), [1]);
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

    /// The record the results of the session just finished find it by.
    fn kept(tab: &Tab) -> Option<RecordId> {
        tab.act(|ctx| match &*ctx.result.peek() {
            Some(Finished::Kept(_, kept)) => *kept,
            _ => None,
        })
    }

    /// Make `change` in `store`, as another copy of the app makes it.
    fn change(store: &MemoryStore, change: HistoryChange) {
        let mut pending = Store::change_history(store, change);
        let mut asked = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(pending.as_mut().poll(&mut asked).is_ready());
    }

    /// The results of a session just finished follow it to the record a
    /// take-up takes up as that session in place of the one it was kept
    /// under, though it holds less of it: as a copy of an older build
    /// writes a run back under another name without the log it does not
    /// keep. They kept the name gone, and placed the run nowhere.
    #[test]
    fn the_results_follow_their_session_to_the_record_taken_up_as_it() {
        let mut run = crate::testing::stored_run("2026-10-03", &[("K5XYZ", "214", "K5XYZ", "214")]);
        run.timestamp = stamp(3);
        let store = holding([1, 2]);
        let a = Tab::on(&store);
        a.finish(run.clone());
        let was = a.id_of(3);
        store.lose(was);
        run.contest = None;
        let json = serde_json::to_string(&run).expect("a session");
        let older = RecordId::beside(was, &json);
        change(
            &store,
            HistoryChange {
                put: vec![Record { id: older, json }],
                ..HistoryChange::default()
            },
        );
        a.sync();
        assert_eq!(a.shown(), [1, 2, 3]);
        assert_eq!(a.id_of(3), older, "taken up as the run");
        assert_eq!(kept(&a), Some(older), "followed");
    }

    /// The results of a session just finished follow it to the one record
    /// holding that very session where a take-up lets go of the record it
    /// was kept under for none it took up as it: as a sync keeps the
    /// drive's name for a session this device holds under another, deleting
    /// this one's. They kept the name gone, and placed the session nowhere.
    /// Where the session is gone, deleted by hand elsewhere, or two records
    /// hold it alike, no record takes the place of the one gone.
    #[test]
    fn the_results_follow_their_session_to_the_one_record_holding_it() {
        let store = holding([1, 2]);
        let a = Tab::on(&store);
        a.finish(day(3));
        let was = a.id_of(3);
        let json = store.record(was).expect("stored");
        let twin = RecordId {
            nonce: was.nonce.wrapping_sub(1),
            ..was
        };
        change(
            &store,
            HistoryChange {
                put: vec![Record { id: twin, json }],
                forget: vec![(was, Deleted::of(&day(3), stamp(4)))],
                ..HistoryChange::default()
            },
        );
        a.sync();
        assert_eq!(a.shown(), [1, 2, 3]);
        assert_eq!(kept(&a), Some(twin), "followed to the drive's name");

        let store = holding([1, 2]);
        let a = Tab::on(&store);
        a.finish(day(3));
        let was = a.id_of(3);
        Tab::on(&store).delete(3);
        a.sync();
        assert_eq!(a.shown(), [1, 2]);
        assert_eq!(kept(&a), Some(was), "deleted: nothing holds it");

        let store = holding([1, 2]);
        let a = Tab::on(&store);
        a.finish(day(3));
        let was = a.id_of(3);
        let json = store.record(was).expect("stored");
        let put = [1, 2].map(|later| Record {
            id: RecordId {
                order: was.order + later,
                ..was
            },
            json: json.clone(),
        });
        change(
            &store,
            HistoryChange {
                put: put.into(),
                forget: vec![(was, Deleted::of(&day(3), stamp(4)))],
                ..HistoryChange::default()
            },
        );
        a.sync();
        assert_eq!(a.shown(), [1, 2, 3, 3]);
        assert_eq!(kept(&a), Some(was), "two hold it alike: neither is it");
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
                left_out: 0,
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
                left_out: 0,
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
    /// is restored. So they do where the store takes no lock on restores,
    /// as a browser without Web Locks takes none, and both are made at once.
    #[test]
    fn two_tabs_restoring_one_backup_store_each_session_once() {
        let store = holding([3]);
        store.unlockable.set(true);
        let (mut a, mut b) = (Tab::on(&store), Tab::on(&store));
        // Neither can see what the other stored as it restores: both are
        // asked for before either is made, as a browser's database makes
        // each a moment after it is asked for.
        store.deferred.set(true);
        restore(&a, [1, 2]);
        restore(&b, [1, 2]);
        store.deferred.set(false);
        store.settle();
        a.pump();
        b.pump();
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        a.sync();
        b.sync();
        assert_eq!(a.shown(), [1, 2, 3]);
        assert_eq!(b.shown(), [1, 2, 3]);
    }

    /// A restore asked for while another is under way, as a browser's
    /// database makes the first a moment after it is asked for, waits for
    /// it, and is merged with the history it restored: it says what it
    /// brought itself, and its Undo takes back that alone. Merged with a
    /// history short of the first's sessions, it took them for its own,
    /// said it restored them, and its Undo deleted them as well.
    #[test]
    fn a_restore_asked_for_while_another_is_under_way_waits_for_it() {
        let store = holding([5]);
        let mut a = Tab::on(&store);
        store.deferred.set(true);
        restore(&a, [1, 2]);
        restore(&a, [1, 2, 3]);
        assert_eq!(store.waiting(), 1, "the second waits for the first");
        store.settle();
        a.pump();
        assert_eq!(store.waiting(), 1, "then it is made");
        store.settle();
        a.pump();
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 5]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 1 session and your settings."
        );
        store.deferred.set(false);
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [1, 2, 5]);
        assert_eq!(a.shown(), [1, 2, 5]);
    }

    /// A restore is told of, by the number it was asked for under, once
    /// what became of it has been said: not while it waits for another, nor
    /// while it is under way. The Undo of it, under way after, leaves it
    /// told of.
    #[test]
    fn a_restore_is_told_of_once_what_became_of_it_is_said() {
        let store = holding([5]);
        let mut a = Tab::on(&store);
        let told = |a: &Tab, ticket| a.act(|ctx| ctx.restore_told(ticket));
        store.deferred.set(true);
        let first = restore(&a, [1, 2]);
        let second = restore(&a, [1, 2, 3]);
        assert!(!told(&a, first), "under way");
        assert!(!told(&a, second), "waiting its turn");
        store.settle();
        a.pump();
        assert!(told(&a, first), "told of");
        assert!(!told(&a, second), "under way in its turn");
        store.settle();
        a.pump();
        assert!(told(&a, second), "told of in its turn");
        a.act(AppCtx::undo_restore);
        assert!(a.act(AppCtx::restore_under_way), "the Undo under way");
        assert!(told(&a, second), "told of all the same");
        store.settle();
        a.pump();
        assert_eq!(days(&store.load_sessions()), [1, 2, 5], "undone");
    }

    /// A restore's message offers its Undo no more once another restore is
    /// asked for, though it stays up while the other is under way: the
    /// Undo would undo nothing, and a click on it did nothing, without a
    /// word.
    #[test]
    fn a_restores_undo_is_taken_off_its_message_as_another_is_asked_for() {
        let store = holding([5]);
        let a = Tab::on(&store);
        restore(&a, [1]);
        assert_eq!(
            a.toast().and_then(|toast| toast.action),
            Some(ToastAction::UndoRestore)
        );
        store.deferred.set(true);
        restore(&a, [1]);
        let toast = a.toast().expect("still up");
        assert_eq!(toast.text, "Restored 1 session and your settings.");
        assert_eq!(toast.action, None, "nothing to undo");
    }

    /// A restore asked for while the Undo of the one before it is under
    /// way, as a browser's database makes the Undo a moment after it is
    /// asked for, waits for it, and is merged with the history the Undo
    /// left: what both hold is restored. Merged with a history that still
    /// held it, it took those sessions for ones already here, and the Undo
    /// deleted them: lost, though the backup restored holds them.
    #[test]
    fn a_restore_asked_for_while_an_undo_is_under_way_waits_for_it() {
        let store = holding([5]);
        let mut a = Tab::on(&store);
        restore(&a, [1, 2]);
        store.deferred.set(true);
        a.act(AppCtx::undo_restore);
        restore(&a, [1, 2, 3]);
        assert_eq!(store.waiting(), 1, "the restore waits for the Undo");
        store.settle();
        a.pump();
        assert_eq!(days(&store.load_sessions()), [5], "undone");
        assert_eq!(store.waiting(), 1, "then the restore is made");
        store.settle();
        a.pump();
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 5]);
        assert_eq!(a.shown(), [1, 2, 3, 5]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 3 sessions and your settings."
        );
    }

    /// The Undo of a restore, its message still up, undoes nothing once
    /// another restore is asked for: that one is merged with a history
    /// that holds what the Undo would take back, and takes those sessions
    /// for ones already here. Undone beneath it, they were deleted, lost
    /// though the backup under way holds them. Its own Undo takes back
    /// what it brought, and no more.
    #[test]
    fn an_undo_clicked_while_another_restore_is_under_way_does_nothing() {
        let store = holding([5]);
        let mut a = Tab::on(&store);
        restore(&a, [1, 2]);
        store.deferred.set(true);
        restore(&a, [1, 2, 3]);
        assert_eq!(store.waiting(), 1, "the second under way");
        a.act(AppCtx::undo_restore);
        assert_eq!(store.waiting(), 1, "nothing undone");
        store.settle();
        a.pump();
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 5]);
        assert_eq!(a.shown(), [1, 2, 3, 5]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 1 session and your settings."
        );
        store.deferred.set(false);
        a.act(AppCtx::undo_restore);
        assert_eq!(days(&store.load_sessions()), [1, 2, 5]);
        assert_eq!(a.shown(), [1, 2, 5]);
    }

    /// The Undo of a restore asked for while a session or a run has the app
    /// waits for it to be over, as a restore does: it puts back settings,
    /// the level among them, which a session is not to have changed beneath
    /// it. A restore asked for meanwhile waits behind it. The Undo was made
    /// at once, under the session.
    #[test]
    fn an_undo_of_a_restore_asked_during_a_session_waits_for_it() {
        crate::testing::run(|| async {
            let store = holding([5]);
            let mut a = Tab::on(&store);
            let level = |a: &Tab| a.act(|ctx| ctx.settings.peek().curriculum.level);
            let at_level = |level| {
                let mut settings = TrainingSettings::default();
                settings.curriculum.level = level;
                settings
            };
            a.act(|ctx| {
                let mut settings = ctx.settings;
                settings.set(at_level(3));
            });
            a.act(|ctx| {
                ctx.restore_backup(Backup {
                    saved_at: 0,
                    settings: Some(at_level(9)),
                    sessions: vec![day(1)],
                    left_out: 0,
                })
            });
            assert_eq!(level(&a), 9);
            let on = |a: &Tab, screen| {
                a.act(|ctx| {
                    let mut shown = ctx.screen;
                    shown.set(screen);
                })
            };
            on(&a, Screen::Training);
            a.act(AppCtx::undo_restore);
            a.advance(3_000).await;
            assert_eq!(level(&a), 9, "not under the session");
            assert_eq!(days(&store.load_sessions()), [1, 5], "nor what it restored");
            on(&a, Screen::Results);
            a.advance(2_000).await;
            assert_eq!(level(&a), 3, "undone once it is over");
            assert_eq!(days(&store.load_sessions()), [5]);
            assert_eq!(a.shown(), [5]);
        });
    }

    /// The Undo of a restore asked for before a session or run, that the
    /// store makes only once it has begun, as a browser's database takes a
    /// moment to, puts the settings back once it is over, the level among
    /// them, and is under way until then: a restore asked for meanwhile is
    /// made after it. The settings went back as soon as the store
    /// answered, under the session.
    #[test]
    fn an_undo_the_store_makes_once_a_session_has_begun_puts_the_settings_back_after_it() {
        crate::testing::run(|| async {
            let store = holding([5]);
            let mut a = Tab::on(&store);
            let level = |a: &Tab| a.act(|ctx| ctx.settings.peek().curriculum.level);
            let at_level = |level| {
                let mut settings = TrainingSettings::default();
                settings.curriculum.level = level;
                settings
            };
            let backup = |day_of: u64, level| Backup {
                saved_at: 0,
                settings: Some(at_level(level)),
                sessions: vec![day(day_of)],
                left_out: 0,
            };
            a.act(|ctx| {
                let mut settings = ctx.settings;
                settings.set(at_level(3));
            });
            a.act(|ctx| ctx.restore_backup(backup(1, 9)));
            assert_eq!(level(&a), 9);
            let on = |a: &Tab, screen| {
                a.act(|ctx| {
                    let mut shown = ctx.screen;
                    shown.set(screen);
                })
            };
            store.deferred.set(true);
            a.act(AppCtx::undo_restore);
            on(&a, Screen::Training);
            store.deferred.set(false);
            store.settle();
            a.pump();
            a.advance(3_000).await;
            assert_eq!(level(&a), 9, "not under the session");
            assert_eq!(days(&store.load_sessions()), [5], "the history undone");
            let waiting = a.act(|ctx| ctx.restore_backup(backup(2, 7)));
            a.advance(2_000).await;
            assert_eq!(days(&store.load_sessions()), [5], "nor restored under it");
            on(&a, Screen::Results);
            a.advance(3_000).await;
            assert!(a.act(|ctx| ctx.restore_told(waiting)));
            assert_eq!(days(&store.load_sessions()), [2, 5], "restored after it");
            assert_eq!(level(&a), 7, "the settings of the restore after it");
        });
    }

    /// A restore asked for in another copy of the app, a tab or a window,
    /// while this one's Undo is offered waits for it, and is made once the
    /// Undo is over, merged with the history it left: what both backups
    /// hold is here. Made beside it, it took the session both restore for
    /// one already here, and this copy's Undo then deleted it, lost though
    /// both backups held it.
    #[test]
    fn a_restore_in_another_copy_waits_while_this_ones_undo_is_offered() {
        crate::testing::run(|| async {
            let store = holding([5]);
            let (a, mut b) = (Tab::on(&store), Tab::on(&store));
            restore(&a, [1]);
            assert_eq!(
                a.toast().and_then(|toast| toast.action),
                Some(ToastAction::UndoRestore)
            );
            let waiting = restore(&b, [1, 2]);
            b.advance(1_000).await;
            assert_eq!(days(&store.load_sessions()), [1, 5], "waits for the Undo");
            assert!(b.act(|ctx| ctx.restore_blocked(waiting)), "and says why");
            a.act(AppCtx::undo_restore);
            assert_eq!(days(&store.load_sessions()), [5], "undone");
            b.advance(1_000).await;
            assert_eq!(
                days(&store.load_sessions()),
                [1, 2, 5],
                "what both backups hold"
            );
            assert_eq!(b.shown(), [1, 2, 5]);
            assert_eq!(
                b.toast().expect("told").text,
                "Restored 2 sessions and your settings."
            );
        });
    }

    /// A copy lets go of every copy's restores as its Undo is offered no
    /// more, its message gone, as another message takes its place: a
    /// restore in another copy waits no longer. Nor does one wait for a
    /// copy whose restore was refused, which offers no Undo.
    #[test]
    fn restores_are_let_go_of_as_the_undo_is_offered_no_more() {
        crate::testing::run(|| async {
            let store = holding([5]);
            let (a, mut b) = (Tab::on(&store), Tab::on(&store));
            restore(&a, [1]);
            let waiting = restore(&b, [1, 2]);
            b.advance(1_000).await;
            assert!(b.act(|ctx| ctx.restore_blocked(waiting)));
            a.act(|ctx| ctx.toasts.show("Something else."));
            b.advance(1_000).await;
            assert_eq!(days(&store.load_sessions()), [1, 2, 5]);
            assert!(!b.act(|ctx| ctx.restore_blocked(waiting)));
            assert_eq!(
                b.toast().expect("told").text,
                "Restored 1 session and your settings."
            );

            // Refused: nothing is held.
            let c = Tab::on(&store);
            b.act(|ctx| ctx.toasts.show("Something else."));
            store.full.set(true);
            restore(&c, [3]);
            store.full.set(false);
            restore(&b, [3]);
            assert_eq!(days(&store.load_sessions()), [1, 2, 3, 5]);
        });
    }

    /// A restore made while another, asked for after it, waits its turn
    /// offers no Undo: the one waiting is the last restore asked for, and
    /// is made at once, merged with what this one brought back. An Undo of
    /// this one beneath it deleted what both hold.
    #[test]
    fn a_restore_made_while_another_waits_offers_no_undo() {
        let store = holding([5]);
        let mut a = Tab::on(&store);
        store.deferred.set(true);
        restore(&a, [1, 2]);
        restore(&a, [1, 2]);
        store.settle();
        a.pump();
        assert_eq!(store.waiting(), 1, "the second under way");
        let told = a.toast().expect("told");
        assert_eq!(told.text, "Restored 2 sessions and your settings.");
        assert_eq!(told.action, None);
        a.act(AppCtx::undo_restore);
        assert_eq!(store.waiting(), 1, "nothing undone");
        store.settle();
        a.pump();
        assert_eq!(days(&store.load_sessions()), [1, 2, 5]);
        assert_eq!(a.shown(), [1, 2, 5]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored your settings. Every session in it was already here."
        );
    }

    /// A restore waiting for another is made once what became of the
    /// other's change has been taken up whole: a session the store let go
    /// for room, as a browser's page storage lets the oldest go, is gone
    /// from the history it is merged with, not taken for one lost in
    /// passing and stored again. It was made as soon as the other was told
    /// of, before the session let go had gone from here, and the store was
    /// asked to keep it twice more.
    #[test]
    fn a_restore_waiting_for_another_is_made_once_that_ones_change_is_taken_up() {
        let store = holding([3, 4, 5]);
        store.page_room.set(Some(3));
        let mut a = Tab::on(&store);
        store.deferred.set(true);
        restore(&a, [6]);
        restore(&a, [7]);
        store.settle();
        a.pump();
        assert_eq!(a.shown(), [4, 5, 6], "3 let go for room");
        assert_eq!(store.waiting(), 1, "the second restore, and nothing else");
        let writes = store.record_writes.get();
        store.settle();
        a.pump();
        assert_eq!(store.record_writes.get(), writes + 1, "7 alone");
        assert_eq!(days(&store.load_sessions()), [5, 6, 7]);
        assert_eq!(a.shown(), [5, 6, 7]);
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
                left_out: 0,
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
            left_out: 0,
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

    /// A store that cannot be listed still gets a backup saved: one of
    /// every session here, those it has yet to take among them, as a
    /// backup is how they are kept safe meanwhile. Saving one used to fail,
    /// saying the history stored here could not be read.
    #[test]
    fn a_backup_holds_every_session_here_where_the_store_cannot_be_listed() {
        let store = holding([1, 2]);
        let a = Tab::on(&store);
        store.full.set(true);
        a.finish(day(3));
        store.unlistable.set(true);
        a.act(AppCtx::save_backup);
        let files = store.backup_files.borrow().clone();
        assert_eq!(files.len(), 1, "one file, saved once");
        let (name, contents) = &files[0];
        let backup = crate::persist::backup::read_backup(contents).expect("a backup");
        assert_eq!(days(&backup.sessions), [1, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            format!("Backup saved as {name}, in your downloads.")
        );
        assert_eq!(store.last_backup(), Some(backup.saved_at));
    }

    /// So does a browser's page storage the browser will not let the site
    /// at, as where the user blocks a site's data: the sessions finished
    /// since are kept only here, and the backup holds them with the rest.
    #[test]
    fn a_backup_holds_every_session_here_where_page_storage_is_blocked() {
        let shelf = PageShelf::default();
        shelf.keep(&[day(1), day(2)]);
        let store = Rc::new(MemoryStore::in_page(&shelf));
        let a = Tab::on(&store);
        shelf.block(true);
        a.finish(day(3));
        assert_eq!(days(&shelf.sessions()), [1, 2], "not stored");
        a.act(AppCtx::save_backup);
        let files = store.backup_files.borrow().clone();
        let (_, contents) = files.last().expect("a backup file");
        let backup = crate::persist::backup::read_backup(contents).expect("a backup");
        assert_eq!(days(&backup.sessions), [1, 2, 3]);
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
        store.plant_unreadable(&format!(r#"{{"format":{}}}"#, cw_core::SESSION_FORMAT + 1));
        let a = Tab::on(&store);
        let told = crate::persist::record::HistoryNotice::Later { count: 1 }.text();
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
                left_out: 0,
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
                left_out: 0,
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
                left_out: 0,
            })
        });
        assert_eq!(store.last_backup(), Some(stamp(30)));
        assert_eq!(a.act(|ctx| *ctx.last_backup.peek()), Some(stamp(30)));
    }

    /// Restore the backup holding a session for each of `days`, in `tab`:
    /// the number the restore is asked for under.
    fn restore(tab: &Tab, days: impl IntoIterator<Item = u64>) -> u64 {
        let sessions = days.into_iter().map(day).collect();
        tab.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: 0,
                settings: Some(TrainingSettings::default()),
                sessions,
                left_out: 0,
            })
        })
    }

    /// A restore asked for while the store cannot be listed restores
    /// nothing, and says why: merged with what this tab last saw of the
    /// history, it took a session another tab stored since for one the
    /// backup brings back, and stored it a second time, under the name a
    /// restore gives it. Once the store can be listed, the restore brings
    /// back what is missing, each session once.
    #[test]
    fn a_restore_while_the_store_cannot_be_listed_stores_nothing_twice() {
        let store = holding([1]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(2));
        store.unlistable.set(true);
        restore(&a, [1, 2, 3]);
        store.unlistable.set(false);
        assert_eq!(days(&store.load_sessions()), [1, 2], "nothing restored");
        assert!(
            a.toast()
                .is_some_and(|toast| toast.text.starts_with("Nothing was restored:")),
            "{:?}",
            a.toast()
        );
        assert_eq!(a.shown(), [1]);
        restore(&a, [1, 2, 3]);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3], "each once");
        assert_eq!(a.shown(), [1, 2, 3]);
    }

    /// A restore asked for while a session another tab stored, listed by
    /// the store, cannot be read for a moment restores nothing either, and
    /// says why: merged with a history short of it, the backup brought it
    /// back a second time. Asked for again, once it reads, the restore
    /// brings back what is missing, each session once. One that still
    /// cannot be read is taken as one that cannot be read at all, and
    /// refuses no restore a second time.
    #[test]
    fn a_restore_while_a_listed_session_cannot_be_read_stores_nothing_twice() {
        let store = holding([1]);
        let (a, b) = (Tab::on(&store), Tab::on(&store));
        b.finish(day(2));
        store.held_open.borrow_mut().insert(b.id_of(2));
        restore(&a, [1, 2, 3]);
        assert!(
            a.toast()
                .is_some_and(|toast| toast.text.starts_with("Nothing was restored:")),
            "{:?}",
            a.toast()
        );
        assert_eq!(days(&store.load_sessions()), [1, 2], "nothing restored");
        store.held_open.borrow_mut().clear();
        restore(&a, [1, 2, 3]);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3], "each once");
        assert_eq!(a.shown(), [1, 2, 3]);

        b.finish(day(4));
        store.held_open.borrow_mut().insert(b.id_of(4));
        restore(&a, [5]);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4], "refused once");
        restore(&a, [5]);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3, 4, 5]);
    }

    /// What a later build stores of a session finished on day 2, in a
    /// format this build cannot read ([`cw_core::SESSION_FORMAT`]).
    fn day_2_as_a_later_build_stores_it() -> String {
        format!(
            r#"{{"format":{},"date":"2026-10-02","timestamp":{},"by":"a later build"}}"#,
            cw_core::SESSION_FORMAT + 1,
            stamp(2)
        )
    }

    /// What a restore says of `count` sessions it left out beside a later
    /// build's records.
    fn left_beside(count: usize) -> String {
        super::told_beside_unread(count).expect("some left out")
    }

    /// A restore leaves out the backup's session from the moment of a
    /// record stored here that this build cannot read, where the record
    /// says it holds a session that finished then, on the same day, as a
    /// later build stores one: a later build reads it, and would show that
    /// session twice. The user is told, and the record is left as it is,
    /// however often the restore is asked for. The backup stored each such
    /// session a second time, beside its record.
    #[test]
    fn a_restore_stores_nothing_twice_beside_a_session_a_later_build_stored() {
        let store = holding([1]);
        let later = day_2_as_a_later_build_stores_it();
        let unread = store.plant_unreadable_at(stamp(2), &later);
        let a = Tab::on(&store);
        restore(&a, [1, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            format!("Restored 1 session and your settings. {}", left_beside(1))
        );
        assert_eq!(days(&store.load_sessions()), [1, 3]);
        assert_eq!(store.record(unread), Some(later), "left as it is");
        restore(&a, [2]);
        assert_eq!(days(&store.load_sessions()), [1, 3], "however often");
        assert_eq!(a.shown(), [1, 3]);
    }

    /// So does one that a later build stored after this copy opened, which
    /// the restore's own look finds and cannot read.
    #[test]
    fn a_restore_stores_nothing_twice_beside_a_session_a_later_build_stored_since_it_opened() {
        let store = holding([1]);
        let a = Tab::on(&store);
        let later = day_2_as_a_later_build_stores_it();
        let unread = store.plant_unreadable_at(stamp(2), &later);
        restore(&a, [1, 2, 3]);
        assert_eq!(days(&store.load_sessions()), [1, 3]);
        assert_eq!(store.record(unread), Some(later));
        assert!(a.act(|ctx| ctx.sessions.peek().unread().contains(&unread)));
    }

    /// A backup that holds such a session twice, as one made of a history
    /// that had stored it twice does, is told of as leaving out one.
    #[test]
    fn a_session_left_out_beside_a_later_builds_record_is_told_of_once() {
        let store = holding([1]);
        store.plant_unreadable_at(stamp(2), &day_2_as_a_later_build_stores_it());
        let a = Tab::on(&store);
        restore(&a, [1, 2, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            format!("Restored 1 session and your settings. {}", left_beside(1))
        );
    }

    /// A later build's record is known by the session it says it holds,
    /// whatever it is named: one named after the newest session, as a
    /// clock set back names one ([`RecordId::next`]), leaves the backup's
    /// session out all the same. Only a record named for the moment of a
    /// session of the backup was read, and the backup stored that session
    /// a second time, beside the later build's.
    #[test]
    fn a_restore_leaves_out_a_session_a_later_build_stored_under_a_name_a_clock_set_back_gave() {
        let store = holding([1, 5]);
        let later = day_2_as_a_later_build_stores_it();
        let unread = store.plant_unreadable(&later);
        assert!(unread.order > stamp(5) * 1000, "named after the newest");
        let a = Tab::on(&store);
        restore(&a, [1, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            format!("Restored 1 session and your settings. {}", left_beside(1))
        );
        assert_eq!(days(&store.load_sessions()), [1, 3, 5]);
        assert_eq!(store.record(unread), Some(later), "left as it is");
    }

    /// A later build's record from the same moment that says its session
    /// finished on another day holds another session: the backup's is
    /// brought back.
    #[test]
    fn a_restore_brings_back_a_session_a_later_builds_record_of_another_day_is_not() {
        let store = holding([1]);
        let other_day = format!(
            r#"{{"format":{},"date":"2026-10-03","timestamp":{},"by":"a later build"}}"#,
            cw_core::SESSION_FORMAT + 1,
            stamp(2)
        );
        store.plant_unreadable_at(stamp(2), &other_day);
        let a = Tab::on(&store);
        restore(&a, [1, 2, 3]);
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
    }

    /// A restore brings back the backup's session from the moment of a
    /// record stored here that holds no session at all, damaged: no build
    /// reads that record, and the backup may hold the only copy of its
    /// session there is. The record is left as it is.
    #[test]
    fn a_restore_brings_back_a_session_whose_record_here_is_damaged() {
        let store = holding([1]);
        let damaged = store.plant_unreadable_at(stamp(2), "{ damaged");
        let a = Tab::on(&store);
        restore(&a, [1, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 2 sessions and your settings."
        );
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        assert_eq!(store.record(damaged).as_deref(), Some("{ damaged"));
    }

    /// So does one damaged so that it is JSON still, but says no moment its
    /// session finished at.
    #[test]
    fn a_restore_brings_back_a_session_whose_record_here_is_json_saying_no_moment() {
        let store = holding([1]);
        let raw = r#"{"date":"2026-10-02","timestamp":null}"#;
        let damaged = store.plant_unreadable_at(stamp(2), raw);
        let a = Tab::on(&store);
        restore(&a, [1, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 2 sessions and your settings."
        );
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        assert_eq!(store.record(damaged).as_deref(), Some(raw));
    }

    /// So does one damaged so that it is JSON still, and says when, and on
    /// what day, its session finished: it says no format later than this
    /// build's ([`cw_core::SESSION_FORMAT`]), so no later build stored it,
    /// and no build reads it. It was taken at its word, as a later build's
    /// record, and the backup's session, maybe the only copy of it there
    /// was, was left out.
    #[test]
    fn a_restore_brings_back_a_session_whose_damaged_record_here_still_says_when_it_finished() {
        let store = holding([1]);
        let raw = format!(
            r#"{{"date":"2026-10-02","timestamp":{},"by":"damage"}}"#,
            stamp(2)
        );
        let damaged = store.plant_unreadable_at(stamp(2), &raw);
        let a = Tab::on(&store);
        restore(&a, [1, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 2 sessions and your settings."
        );
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
        assert_eq!(store.record(damaged), Some(raw), "left as it is");
    }

    /// So does a record file whose bytes are no text at all, as damage on a
    /// disk leaves one: it cannot be read now as it could not as the app
    /// opened, and refuses no restore. It was read again, and refused the
    /// first restore at every launch.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_restore_brings_back_a_session_whose_file_here_is_no_text() {
        let dir = TempDir::new("restore-beside-no-text");
        let first = Tab::on_store(crate::persist::files_at(&dir.0));
        first.finish(day(1));
        let damaged = RecordId {
            order: stamp(2) * 1000,
            nonce: 7,
        };
        std::fs::write(
            dir.0
                .join("history")
                .join("sessions")
                .join(format!("{damaged}.json")),
            [0xff_u8, 0xfe, 0x00, 0x80],
        )
        .expect("written");
        let a = Tab::on_store(crate::persist::files_at(&dir.0));
        assert!(a.act(|ctx| ctx.sessions.peek().unread().contains(&damaged)));
        restore(&a, [1, 2]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 1 session and your settings."
        );
        assert_eq!(a.shown(), [1, 2]);
    }

    /// So does one whose bytes became no text after the app opened: a look
    /// that finds it takes it for one no build reads, as the app's launch
    /// does, and neither reads it again nor refuses a restore for it. A
    /// look took it for one that could not be read just now: it refused the
    /// first restore, was read again at each try, and was told of as one
    /// that could not be read, and noted so, which kept the next launch from
    /// telling of it as damaged.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_restore_brings_back_a_session_whose_file_here_became_no_text_after_the_app_opened() {
        let dir = TempDir::new("restore-beside-no-text-since");
        let a = Tab::on_store(crate::persist::files_at(&dir.0));
        a.finish(day(1));
        let damaged = RecordId {
            order: stamp(2) * 1000,
            nonce: 7,
        };
        std::fs::write(
            dir.0
                .join("history")
                .join("sessions")
                .join(format!("{damaged}.json")),
            [0xff_u8, 0xfe, 0x00, 0x80],
        )
        .expect("written");
        restore(&a, [1, 2]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 1 session and your settings."
        );
        assert_eq!(a.shown(), [1, 2]);
        assert!(a.act(|ctx| ctx.sessions.peek().unread().contains(&damaged)));
        assert!(a.act(|ctx| ctx.unread_for_now.peek().is_empty()));
    }

    /// Such a record, found by a look, is not told of as one that could not
    /// be read, at any try ([`READ_AGAIN_MS`]): the next launch tells of it
    /// as damaged, once, as it tells of one found so as the app opened. It
    /// was told of after the last try as one that could not be read, and
    /// noted as told, so no launch told of it as damaged.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_file_here_that_became_no_text_after_the_app_opened_is_told_of_as_damaged() {
        crate::testing::run(|| async {
            let dir = TempDir::new("no-text-since-told");
            let mut a = Tab::on_store(crate::persist::files_at(&dir.0));
            a.finish(day(1));
            let damaged = RecordId {
                order: stamp(2) * 1000,
                nonce: 7,
            };
            std::fs::write(
                dir.0
                    .join("history")
                    .join("sessions")
                    .join(format!("{damaged}.json")),
                [0xff_u8, 0xfe, 0x00, 0x80],
            )
            .expect("written");
            a.act(AppCtx::take_up_changes);
            let last = u64::from(READ_AGAIN_MS[READ_AGAIN_MS.len() - 1]);
            a.advance(last + 1_000).await;
            assert_eq!(a.notices(), [], "not as one that could not be read");
            assert_eq!(a.shown(), [1]);
            drop(a);
            let b = Tab::on_store(crate::persist::files_at(&dir.0));
            let told = HistoryNotice::Damaged { count: 1 }.text();
            assert_eq!(b.notices(), [Notice::History(told)]);
        });
    }

    /// A record that could not be read as the app opened, and still cannot
    /// be read as a restore looks at the history, refuses the first restore,
    /// as one the restore's own look finds so does: it may hold a session
    /// of the backup, which would be stored a second time. Asked for again,
    /// the restore goes ahead. It was held as one this build cannot read,
    /// and refused no restore, though it may read a moment later.
    #[test]
    fn a_record_that_cannot_be_read_as_the_app_opens_nor_as_it_restores_refuses_the_first_restore()
    {
        let store = holding([1]);
        let unread = store.plant_unreadable_at(stamp(2), &day_2_as_a_later_build_stores_it());
        store.held_open.borrow_mut().insert(unread);
        let a = Tab::on(&store);
        restore(&a, [1, 2, 3]);
        assert!(
            a.toast()
                .is_some_and(|toast| toast.text.starts_with("Nothing was restored:")),
            "{:?}",
            a.toast()
        );
        assert_eq!(days(&store.load_sessions()), [1], "nothing restored");
        restore(&a, [1, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 2 sessions and your settings."
        );
        assert_eq!(days(&store.load_sessions()), [1, 2, 3]);
    }

    /// A store holding a session for each of `held`, and one for `busy` it
    /// cannot read for now, as a file another program holds open for a
    /// moment cannot be ([`MemoryStore::held_open`]): that one's record.
    fn holding_busy(held: impl IntoIterator<Item = u64>, busy: u64) -> (Rc<MemoryStore>, RecordId) {
        let store = holding(held);
        let raw = serde_json::to_string(&day(busy)).expect("a session writes");
        // It reads, once it is let go.
        let id = store.plant_unreadable_at(stamp(busy), &raw);
        store.held_open.borrow_mut().insert(id);
        (store, id)
    }

    /// A record that could not be read as the app opened, as a file another
    /// program holds open for a moment cannot be, is read at the next look,
    /// once it can be, and shown, and nothing is told of it meanwhile. It
    /// was held as one this build cannot read, told of at once as one that
    /// could not be read just now, and not read again while the app ran.
    #[test]
    fn a_record_that_could_not_be_read_as_the_app_opened_is_read_at_the_next_look() {
        let (store, _) = holding_busy([1], 2);
        let a = Tab::on(&store);
        assert_eq!(a.notices(), [], "not told of yet");
        assert_eq!(a.shown(), [1]);
        store.held_open.borrow_mut().clear();
        a.act(AppCtx::take_up_changes);
        assert_eq!(a.shown(), [1, 2]);
        assert_eq!(a.notices(), []);
    }

    /// A window kept in front, with no other copy of the app beside it, may
    /// not look at the history again for as long as it runs: a record that
    /// could not be read as the app opened is read again on its own soon
    /// after ([`READ_AGAIN_MS`]), and shown once it reads.
    #[test]
    fn a_record_that_could_not_be_read_as_the_app_opened_is_read_again_soon_on_its_own() {
        crate::testing::run(|| async {
            let (store, _) = holding_busy([1], 2);
            let mut a = Tab::on(&store);
            a.advance(1_000).await;
            assert_eq!(a.shown(), [1]);
            store.held_open.borrow_mut().clear();
            a.advance(u64::from(READ_AGAIN_MS[0])).await;
            assert_eq!(a.shown(), [1, 2], "read again on its own");
            assert_eq!(a.notices(), []);
        });
    }

    /// A record that still cannot be read after every try is told of then,
    /// once, as one that could not be read ([`READ_AGAIN_MS`]): not as the
    /// app opens, as a moment's failure is over before the first try, nor
    /// again later, nor at the next launch. It was told of at once, as one
    /// that could not be read just now, though it may read a moment later.
    #[test]
    fn a_record_still_unread_after_every_try_is_told_of_once() {
        crate::testing::run(|| async {
            let (store, _) = holding_busy([1], 2);
            let mut a = Tab::on(&store);
            assert_eq!(a.notices(), [], "not as the app opens");
            let last = u64::from(READ_AGAIN_MS[READ_AGAIN_MS.len() - 1]);
            a.advance(last - 1_000).await;
            assert_eq!(a.notices(), [], "nor before every try is made");
            a.advance(2_000).await;
            let told = HistoryNotice::Unread { count: 1 }.text();
            assert_eq!(a.notices(), [Notice::History(told.clone())]);
            a.advance(3 * last).await;
            assert_eq!(a.notices(), [Notice::History(told)], "once");
            let b = Tab::on(&store);
            assert_eq!(b.notices(), [], "nor at the next launch");
            assert_eq!(b.shown(), [1]);
        });
    }

    /// A record that could not be read as the app opened, and is deleted
    /// in another copy of the app before it reads, is let go of as a look
    /// finds it deleted: neither shown nor told of, at any try
    /// ([`READ_AGAIN_MS`]).
    #[test]
    fn a_record_unread_for_now_and_deleted_elsewhere_is_let_go_of() {
        crate::testing::run(|| async {
            let (store, _) = holding_busy([1], 2);
            let mut a = Tab::on(&store);
            store.held_open.borrow_mut().clear();
            let b = Tab::on(&store);
            b.delete(2);
            let last = u64::from(READ_AGAIN_MS[READ_AGAIN_MS.len() - 1]);
            a.advance(2 * last).await;
            assert_eq!(a.shown(), [1]);
            assert_eq!(a.notices(), [], "told of at no try");
            assert!(a.act(|ctx| ctx.unread_for_now.peek().is_empty()));
        });
    }

    /// A record that could not be read as the app opened, and still cannot
    /// be read as it opens again, is told of then, as one that could not be
    /// read, and once: the app closed before its tries were made.
    #[test]
    fn a_record_still_unread_at_the_next_launch_is_told_of_then() {
        let (store, _) = holding_busy([1], 2);
        let a = Tab::on(&store);
        assert_eq!(a.notices(), [], "not told of yet");
        drop(a);
        let b = Tab::on(&store);
        let told = HistoryNotice::Unread { count: 1 }.text();
        assert_eq!(b.notices(), [Notice::History(told)]);
        drop(b);
        assert_eq!(Tab::on(&store).notices(), [], "told of once");
    }

    /// A record that could not be read as the app opened, and read at its
    /// first try, fails no longer: should it fail for a moment again as the
    /// app next opens, it is not told of then, any more than at its first
    /// launch. The store remembered it as failing at the last launch, and it
    /// was told of at once.
    #[test]
    fn a_record_read_at_a_try_after_failing_as_the_app_opened_is_not_told_of_as_it_next_opens() {
        crate::testing::run(|| async {
            let (store, id) = holding_busy([1], 2);
            let mut a = Tab::on(&store);
            store.held_open.borrow_mut().clear();
            a.advance(u64::from(READ_AGAIN_MS[0]) + 500).await;
            assert_eq!(a.shown(), [1, 2]);
            store.held_open.borrow_mut().insert(id);
            drop(a);
            let b = Tab::on(&store);
            assert_eq!(b.notices(), [], "not told of at once");
            assert_eq!(b.shown(), [1]);
        });
    }

    /// The record of day 2 in the files in a folder, a folder in its place
    /// for now, which cannot be read as one, as a file another program holds
    /// open for a moment cannot be; the records folder's time of change set
    /// back each time it is changed, so that the store sees nothing changed
    /// there ([`Store::history_changed`]).
    #[cfg(all(unix, feature = "native-runtime"))]
    struct BusyFile {
        path: std::path::PathBuf,
        held: Vec<u8>,
        records: std::path::PathBuf,
        changed: std::time::SystemTime,
    }

    #[cfg(all(unix, feature = "native-runtime"))]
    impl BusyFile {
        /// Days 1 and 2 stored in the files in `dir`, as a copy of the app
        /// stores them, and the record of day 2 held for now.
        fn of_day_2(dir: &TempDir) -> Self {
            let a = Tab::on_store(crate::persist::files_at(&dir.0));
            a.finish(day(1));
            a.finish(day(2));
            let id = a.id_of(2);
            drop(a);
            let records = dir.0.join("history").join("sessions");
            let changed = std::fs::metadata(&records)
                .and_then(|meta| meta.modified())
                .expect("when it changed");
            let path = records.join(format!("{id}.json"));
            let held = std::fs::read(&path).expect("stored");
            std::fs::remove_file(&path).expect("taken away");
            std::fs::create_dir(&path).expect("a folder in its place");
            let busy = Self {
                path,
                held,
                records,
                changed,
            };
            busy.as_it_was();
            busy
        }

        /// The record read again, as it was stored.
        fn let_go(&self) {
            std::fs::remove_dir(&self.path).expect("the folder taken away");
            std::fs::write(&self.path, &self.held).expect("the record back");
            self.as_it_was();
        }

        /// The records folder's time of change set back.
        fn as_it_was(&self) {
            std::fs::File::open(&self.records)
                .and_then(|records| records.set_modified(self.changed))
                .expect("its time set back");
        }
    }

    /// On the files themselves: a record that could not be read as the app
    /// opened is read at the next look, and shown, though the store says
    /// nothing changed: one that could not be read is reason enough to look.
    /// Nothing is told of it meanwhile. It was held as one this build cannot
    /// read, told of at once, and never read again while the app ran.
    #[cfg(all(unix, feature = "native-runtime"))]
    #[test]
    fn a_file_that_could_not_be_read_as_the_app_opened_is_read_at_the_next_look() {
        let dir = TempDir::new("busy-at-launch-looked");
        let busy = BusyFile::of_day_2(&dir);
        let store = crate::persist::files_at(&dir.0);
        let a = Tab::on_store(store.clone());
        assert_eq!(a.shown(), [1]);
        assert_eq!(a.notices(), [], "not told of yet");
        busy.let_go();
        assert!(
            !store.history_changed(),
            "nothing changed, as the store sees"
        );
        a.act(AppCtx::take_up_changes);
        assert_eq!(a.shown(), [1, 2]);
        assert_eq!(a.notices(), []);
    }

    /// On the files themselves, a window kept in front, with no other copy
    /// of the app beside it: a record that could not be read as the app
    /// opened is read again on its own soon after ([`READ_AGAIN_MS`]), and
    /// shown, though the store says nothing changed.
    #[cfg(all(unix, feature = "native-runtime"))]
    #[test]
    fn a_file_that_could_not_be_read_as_the_app_opened_is_read_again_soon_on_its_own() {
        crate::testing::run(|| async {
            let dir = TempDir::new("busy-at-launch-again");
            let busy = BusyFile::of_day_2(&dir);
            let store = crate::persist::files_at(&dir.0);
            let mut a = Tab::on_store(store.clone());
            assert_eq!(a.shown(), [1]);
            busy.let_go();
            assert!(
                !store.history_changed(),
                "nothing changed, as the store sees"
            );
            a.advance(u64::from(READ_AGAIN_MS[0]) + 500).await;
            assert_eq!(a.shown(), [1, 2], "read again on its own");
            assert_eq!(a.notices(), []);
        });
    }

    /// A record that could not be read as the app opened, and reads now, is
    /// taken up by a restore whose backup holds a session from its moment,
    /// and the backup's session is merged with it: stored once, under the
    /// record's own name, and the user told. It stayed unread until the app
    /// opened again, and the backup stored its session a second time,
    /// beside it.
    #[test]
    fn a_restore_takes_up_a_record_unread_as_the_app_opened_that_reads_now() {
        let store = holding([1]);
        let unread = store.plant_unreadable_at(stamp(2), "{ cut short");
        let a = Tab::on(&store);
        assert_eq!(a.shown(), [1]);
        store.mend(unread, &serde_json::to_string(&day(2)).expect("writes"));
        restore(&a, [1, 2, 3]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 1 session and your settings. 1 session stored here that could not be read before reads now."
        );
        assert_eq!(days(&store.load_sessions()), [1, 2, 3], "each once");
        assert_eq!(a.shown(), [1, 2, 3]);
        assert_eq!(a.id_of(2), unread, "under its own name");
    }

    /// A record this build could not read, deleted in another copy of the
    /// app, is no longer held as stored once a look finds it gone: a
    /// restore whose backup holds a session from its moment brings that
    /// session back at once.
    #[test]
    fn a_record_unread_here_and_gone_since_refuses_no_restore() {
        let store = holding([1]);
        let unread = store.plant_unreadable_at(stamp(2), &day_2_as_a_later_build_stores_it());
        let a = Tab::on(&store);
        store.lose(unread);
        restore(&a, [2]);
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 1 session and your settings."
        );
        assert_eq!(days(&store.load_sessions()), [1, 2]);
        assert!(a.act(|ctx| ctx.sessions.peek().unread().is_empty()));
    }

    /// A restore that reads such a record again from a store that answers a
    /// moment later, as a browser's database does, is under way until it
    /// is made: one asked for meanwhile waits its turn, and both are made.
    #[test]
    fn a_restore_asked_for_while_another_reads_a_record_again_waits_for_it() {
        let store = holding([1]);
        store.plant_unreadable_at(stamp(2), &day_2_as_a_later_build_stores_it());
        let mut a = Tab::on(&store);
        store.reads_later.set(true);
        restore(&a, [1, 2, 3]);
        restore(&a, [4]);
        assert_eq!(store.reads_waiting(), 1, "the first reads");
        assert_eq!(days(&store.load_sessions()), [1], "neither made yet");
        store.answer_reads();
        a.pump();
        assert_eq!(days(&store.load_sessions()), [1, 3, 4], "each made");
        assert_eq!(
            a.toast().expect("told").text,
            "Restored 1 session and your settings."
        );
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
        let toast = a.toast().expect("told");
        assert!(
            toast.text.starts_with("The restore could not be undone:"),
            "{toast:?}"
        );
        assert_eq!(
            toast.action,
            Some(ToastAction::UndoRestore),
            "offered again"
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
    /// undoing it takes none out. Undoing the first takes them out. The
    /// store takes no lock on restores, as a browser without Web Locks
    /// takes none: with one, this tab's restore waits until the first's
    /// Undo is offered no more.
    #[test]
    fn undoing_a_restore_leaves_what_another_tabs_restore_added() {
        let store = holding([3]);
        store.unlockable.set(true);
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

    /// A restore says how many of the backup's sessions this version could
    /// not read, and left out, and never that every session in it was here
    /// already: those it left out may not be. A backup none of whose
    /// sessions it could read was said to hold only what was here.
    #[test]
    fn a_restore_tells_of_the_sessions_it_could_not_read() {
        let store = holding([1]);
        let a = Tab::on(&store);
        let settings = serde_json::to_string(&TrainingSettings::default()).unwrap();
        let here = serde_json::to_string(&day(1)).unwrap();
        let raw = format!(
            r#"{{"dustBackup":1,"settings":{settings},"sessions":[{here},{{"date":5}},{{"date":6}}]}}"#
        );
        let backup = crate::persist::backup::read_backup(&raw).expect("a backup");
        a.act(|ctx| ctx.restore_backup(backup));
        assert_eq!(
            a.toast().expect("told").text,
            "Restored your settings. 2 sessions in it could not be read by this version and were left out."
        );
        let raw = r#"{"dustBackup":1,"sessions":[{"date":5}]}"#;
        let backup = crate::persist::backup::read_backup(raw).expect("a backup");
        a.act(|ctx| ctx.restore_backup(backup));
        assert_eq!(
            a.toast().expect("told").text,
            "Restored no sessions. 1 session in it could not be read by this version and was left out."
        );
        assert_eq!(days(&store.load_sessions()), [1]);
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
                    left_out: 0,
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

    /// A browser that keeps the history whole in its page storage
    /// remembers deletions by key, so a session a restore filled in, and
    /// the one its Undo put back, is not remembered as deleted: another tab
    /// takes up the session in its place once, and stores it again should
    /// it go missing in passing. The one taken out for it used to be
    /// remembered as deleted by hand, under the key of the session kept in
    /// its place, and a tab that found that one missing let it go for good.
    #[test]
    fn a_session_filled_in_in_page_storage_is_never_taken_for_deleted() {
        let run = |log: bool| {
            let mut run =
                crate::testing::stored_run("2026-10-02", &[("K5XYZ", "214", "K5XYZ", "214")]);
            run.timestamp = stamp(2);
            if !log {
                run.contest = None;
            }
            run
        };
        let shelf = PageShelf::default();
        shelf.keep(&[day(1), run(false)]);
        let (a, b) = (
            Tab::on(&Rc::new(MemoryStore::in_page(&shelf))),
            Tab::on(&Rc::new(MemoryStore::in_page(&shelf))),
        );
        let logged = |tab: &Tab| tab.act(|ctx| ctx.sessions.peek()[1].contest.is_some());
        a.act(|ctx| {
            ctx.restore_backup(Backup {
                saved_at: 0,
                settings: None,
                sessions: vec![run(true)],
                left_out: 0,
            })
        });
        assert_eq!(a.toast().expect("told").text, "Filled in 1 session.");
        assert_eq!(days(&shelf.sessions()), [1, 2]);
        assert!(shelf.sessions()[1].contest.is_some(), "filled in");
        assert_eq!(shelf.deleted(), [], "nothing deleted by hand");
        b.sync();
        assert_eq!(b.shown(), [1, 2], "taken up once");
        assert!(logged(&b));
        // Lost in passing, as tabs writing the history whole lose what
        // another stored: stored again.
        shelf.lose(stamp(2));
        b.sync();
        assert_eq!(days(&shelf.sessions()), [1, 2]);
        assert!(shelf.sessions()[1].contest.is_some());
        assert_eq!(b.shown(), [1, 2]);

        a.act(AppCtx::undo_restore);
        assert_eq!(days(&shelf.sessions()), [1, 2]);
        assert!(shelf.sessions()[1].contest.is_none(), "as it was");
        assert_eq!(shelf.deleted(), [], "nothing deleted by hand");
        b.sync();
        assert_eq!(b.shown(), [1, 2], "taken up once");
        assert!(!logged(&b));
        shelf.lose(stamp(2));
        a.sync();
        assert_eq!(days(&shelf.sessions()), [1, 2], "stored again");
        assert_eq!(a.shown(), [1, 2]);
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

    /// A deletion the files have made, though its record cannot be taken
    /// away yet, as one held open elsewhere cannot be on Windows, is told
    /// of as made, with Undo, and the next launch agrees; Undo puts the
    /// session back, and the next launch agrees with that too. It used to
    /// be told of as nothing deleted, and the session, shown as kept with
    /// nothing to undo, went at the next launch.
    #[cfg(all(unix, feature = "native-runtime"))]
    #[test]
    fn a_deletion_made_is_told_of_as_made_though_its_record_stays_a_while() {
        use std::os::unix::fs::PermissionsExt;
        if !TempDir::permissions_hold() {
            eprintln!("skipped: a folder's permissions do not hold whoever runs this");
            return;
        }
        let dir = TempDir::new("record-stays");
        let files = || crate::persist::files_at(&dir.0);
        let tab = Tab::on_store(files());
        tab.finish(day(1));
        tab.finish(day(2));
        let records = dir.0.join("history").join("sessions");
        let held = |mode| {
            std::fs::set_permissions(&records, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        let launched = || Tab::on_store(files()).shown();
        // Its records can be read, and none taken away.
        held(0o555);
        tab.delete(1);
        let toast = tab.toast().expect("told");
        assert_eq!(toast.text, "1 session deleted");
        assert_eq!(toast.action, Some(ToastAction::UndoDelete));
        assert_eq!(tab.shown(), [2]);
        assert_eq!(launched(), [2]);
        tab.act(AppCtx::undo_delete);
        assert_eq!(tab.shown(), [1, 2]);
        assert_eq!(launched(), [1, 2]);
        held(0o755);
        assert_eq!(
            std::fs::read_dir(&records).unwrap().count(),
            2,
            "one record a session, and nothing beside"
        );
    }

    /// The history written on a thread of its own, as the app writes it: a
    /// session kept shows at once, its mark held until the thread has
    /// stored it; and the app, closing, waits for the thread and takes up
    /// what it did, so the session is stored and its mark gone before the
    /// app is, and the next launch has nothing cut off to tell of. A
    /// deletion is told of once it is made, and only then.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_session_kept_as_the_app_closes_is_stored_before_it_goes() {
        let dir = crate::persist::TempDir::new("closing-on-the-worker");
        let tab = Tab::on_store(crate::persist::files_on_worker_at(&dir.0));
        let marks = || {
            std::fs::read_dir(dir.0.join("open"))
                .map(|marks| marks.count())
                .unwrap_or(0)
        };
        // Marked as a session is, its end held with its mark before it is
        // kept.
        let open = tab.act(|ctx| ctx.mark_running(false));
        let so_far = SoFar {
            result: day(1),
            settings: None,
        };
        tab.act(|ctx| ctx.mark_so_far(&open, so_far));
        tab.finish(day(1));
        assert_eq!(tab.shown(), [1], "shown at once");
        assert!(marks() > 0, "held until the thread has stored it");
        tab.act(AppCtx::flush_history);
        assert_eq!(marks(), 0, "stored, and its mark gone");
        let next = crate::persist::files_at(&dir.0);
        let mut cut = next.take_cut_off();
        let asked = &mut std::task::Context::from_waker(std::task::Waker::noop());
        assert_eq!(
            cut.as_mut().poll(asked),
            std::task::Poll::Ready(Vec::new()),
            "nothing cut off"
        );
        assert_eq!(Tab::on_store(next).shown(), [1]);

        tab.finish(day(2));
        tab.act(AppCtx::flush_history);
        tab.delete(1);
        assert_eq!(tab.shown(), [2]);
        assert_eq!(tab.toast(), None, "not told of before it is made");
        tab.act(AppCtx::flush_history);
        let toast = tab.toast().expect("told");
        assert_eq!(toast.text, "1 session deleted");
        assert_eq!(Tab::on_store(crate::persist::files_at(&dir.0)).shown(), [2]);
    }

    /// A session kept as the app stops, which the store took before the app
    /// heard that it had, as a page closed under a browser's database leaves
    /// it: the next launch finds its mark over and its session stored, and
    /// tells of nothing cut off. Its mark used to say it still ran, so the
    /// next launch told of a session cut off, Dust closed while it ran. One
    /// the store had not taken is kept from its mark, and told of as kept.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_session_kept_as_the_app_stopped_is_not_told_of_as_cut_off() {
        let dir = TempDir::new("kept-as-it-stopped");
        let tab = Tab::on_store(crate::persist::files_on_worker_at(&dir.0));
        let open = tab.act(|ctx| ctx.mark_running(false));
        let so_far = SoFar {
            result: day(1),
            settings: None,
        };
        tab.act(|ctx| ctx.mark_so_far(&open, so_far));
        tab.finish(day(1));
        // The store has it, and the app stops before it hears so.
        tab.act(|ctx| ctx.app().store.flush_history());
        drop(tab);

        let mut next = Tab::on_store(crate::persist::files_at(&dir.0));
        next.act(AppCtx::tell_of_cut_off);
        next.pump();
        assert_eq!(next.shown(), [1]);
        assert_eq!(next.notices(), [], "nothing was cut off");

        // Stopped before the store had it: kept from its mark, and told of.
        let lost = TempDir::new("kept-from-its-mark");
        let store = crate::persist::files_at(&lost.0);
        let tab = Tab::on_store(store);
        let open = tab.act(|ctx| ctx.mark_running(false));
        let so_far = SoFar {
            result: day(2),
            settings: None,
        };
        tab.act(|ctx| ctx.mark_so_far(&open, so_far));
        tab.act(AppCtx::mark_kept);
        drop(tab);
        let mut next = Tab::on_store(crate::persist::files_at(&lost.0));
        next.act(AppCtx::tell_of_cut_off);
        next.pump();
        assert_eq!(next.shown(), [2], "kept from its mark");
        assert!(
            matches!(&next.notices()[..], [Notice::CutOff(CutOff::Marked(open))] if open.over),
            "told of as kept: {:?}",
            next.notices()
        );
    }

    /// A session cut off, whose mark a copy of the app took as it opened
    /// and stopped under before it held anything of it, is kept by the next
    /// copy to open, and told of there: the mark is left where it is until
    /// the copy that took it holds what it carried. It used to be deleted
    /// as it was taken, and the session was lost for good.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_session_cut_off_is_kept_though_the_copy_that_took_its_mark_stopped() {
        let dir = TempDir::new("taken-then-stopped");
        let tab = Tab::on_store(crate::persist::files_at(&dir.0));
        let open = tab.act(|ctx| ctx.mark_running(false));
        let so_far = SoFar {
            result: day(2),
            settings: None,
        };
        tab.act(|ctx| ctx.mark_so_far(&open, so_far));
        tab.act(AppCtx::mark_kept);
        drop(tab);

        // Takes the mark as it opens, and stops before it does anything
        // with it.
        let took = Tab::on_store(crate::persist::files_at(&dir.0));
        took.act(AppCtx::tell_of_cut_off);
        drop(took);

        let mut next = Tab::on_store(crate::persist::files_at(&dir.0));
        next.act(AppCtx::tell_of_cut_off);
        next.pump();
        assert_eq!(next.shown(), [2], "kept from its mark");
        assert!(
            matches!(&next.notices()[..], [Notice::CutOff(CutOff::Marked(open))] if open.over),
            "told of: {:?}",
            next.notices()
        );
        assert_eq!(Tab::on_store(crate::persist::files_at(&dir.0)).shown(), [2]);
    }

    /// Leave in `store` the mark of a copy of the app that kept `session`
    /// as it stopped, before the store had it.
    fn cut_off_keeping(store: &Rc<MemoryStore>, session: SessionResult) {
        let tab = Tab::on(store);
        let open = tab.act(|ctx| ctx.mark_running(false));
        let so_far = SoFar {
            result: session,
            settings: None,
        };
        tab.act(|ctx| ctx.mark_so_far(&open, so_far));
        tab.act(AppCtx::mark_kept);
    }

    /// A session of groups cut off counts towards its level's run only once
    /// the mark that carried it, with the settings it ran on, is gone, as a
    /// browser takes it away a moment after it is asked to: a copy that
    /// stops before then leaves the mark to the next, which counts it then,
    /// and one that stops after leaves it in its own mark, as a session
    /// counted already. It used to be counted while the mark was still
    /// there, and a copy that stopped in between had it counted twice.
    #[test]
    fn a_session_cut_off_counts_towards_its_level_only_once_its_mark_is_gone() {
        let mut settings = TrainingSettings::default();
        settings.auto_level.auto_adjust_level = true;
        settings.auto_level.auto_adjust_above_threshold_count = 3;
        settings.curriculum.num_groups = 2;
        let key = cw_core::CountersKey::of(&settings);
        let mut session = day(2);
        session.accuracy = 1.0;
        session.groups = vec![cw_core::GroupResult {
            sent: "KM".into(),
            received: "KM".into(),
            correct: true,
        }];
        let store = Rc::new(MemoryStore::default());
        let tab = Tab::on(&store);
        let open = tab.act(|ctx| ctx.mark_running(false));
        let so_far = SoFar {
            result: session,
            settings: Some(settings),
        };
        tab.act(|ctx| ctx.mark_so_far(&open, so_far));
        drop(tab);

        store.releases_later.set(true);
        let mut next = Tab::on(&store);
        next.act(AppCtx::tell_of_cut_off);
        next.pump();
        assert!(store.taken().is_some(), "its mark not gone yet");
        assert_eq!(
            store.load_auto_counters(&key),
            cw_core::AutoLevelCounters::default(),
            "not counted while its mark is there"
        );
        assert_eq!(days(&store.load_sessions()), Vec::<u64>::new(), "nor kept");
        assert_eq!(
            store.open().map(|open| days(&open.unstored)),
            Some(vec![2]),
            "held by this copy's mark meanwhile"
        );
        store.answer_releases();
        next.pump();
        assert_eq!(store.taken(), None, "gone");
        assert_eq!(store.load_auto_counters(&key).above, 1, "counted once");
        assert_eq!(days(&store.load_sessions()), [2], "and kept");
        assert_eq!(next.shown(), [2]);
    }

    /// A mark one copy took stays held by it, whatever another copy lets go
    /// of: one that took nothing, as the look a session's start makes may
    /// take nothing, lets go of nothing. The store used to let go of the
    /// mark taken at any copy's letting go, and the copy that took it was
    /// taken for holding none.
    #[test]
    fn a_mark_one_copy_took_stays_held_whatever_another_lets_go_of() {
        let store = Rc::new(MemoryStore::default());
        cut_off_keeping(&store, day(2));
        let mut took = Tab::on(&store);
        let mut other = Tab::on(&store);
        took.act(AppCtx::tell_of_cut_off);
        assert!(store.taken().is_some(), "taken");
        other.act(|ctx| ctx.mark_running(false));
        other.pump();
        assert!(
            store.taken().is_some(),
            "held still by the copy that took it"
        );
        took.pump();
        assert_eq!(store.taken(), None, "let go of once it is carried");
        assert_eq!(took.shown(), [2]);
    }

    /// The mark of a session cut off is held by the copy that took it, and
    /// let go of as carried only once that copy's mark holds what it
    /// carried. A look that finds this copy's own mark lets go of it as it
    /// was, and it is the mark still; one that finds none lets go of none.
    #[test]
    fn a_mark_cut_off_is_let_go_only_once_this_copys_mark_holds_what_it_carried() {
        let store = Rc::new(MemoryStore::default());
        cut_off_keeping(&store, day(2));
        store.forgotten_later.set(true);
        store.mark_log.take();
        let mut next = Tab::on(&store);
        next.act(AppCtx::tell_of_cut_off);
        assert!(store.taken().is_some(), "held by the copy that took it");
        next.pump();
        let log = store.mark_log.take();
        assert_eq!(
            log[..2],
            [
                MarkEvent::Marked(vec![stamp(2)]),
                MarkEvent::Released {
                    carried: 1,
                    left: 0
                }
            ],
            "{log:?}"
        );
        assert_eq!(store.taken(), None);
        store.answer_forgotten();
        next.pump();
        assert_eq!(store.open(), None, "stored, so no longer marked");
        assert_eq!(store.mark_log.take(), [MarkEvent::Unmarked]);

        let open = next.act(|ctx| ctx.mark_running(false));
        next.pump();
        next.act(AppCtx::tell_of_cut_off);
        next.pump();
        assert_eq!(
            store.mark_log.take(),
            [
                MarkEvent::Marked(Vec::new()),
                MarkEvent::Released {
                    carried: 0,
                    left: 0
                },
                MarkEvent::Released {
                    carried: 0,
                    left: 1
                },
            ]
        );
        assert_eq!(store.open(), Some(open), "its own, marked still");
    }

    /// A session cut off is held by this copy's mark as soon as it is
    /// kept, before the store has said whether it was deleted by hand: its
    /// own mark was taken from the store as it was told of, so the app
    /// stopping while the store answered lost it for good. One the store
    /// says was deleted goes again, from the history and from the mark.
    #[test]
    fn a_session_cut_off_is_held_while_the_store_looks_for_its_deletion() {
        let store = Rc::new(MemoryStore::default());
        cut_off_keeping(&store, day(2));
        store.forgotten_later.set(true);
        let mut next = Tab::on(&store);
        next.act(AppCtx::tell_of_cut_off);
        next.pump();
        let held = |store: &MemoryStore| {
            store
                .open()
                .map(|open| days(&open.unstored))
                .unwrap_or_default()
        };
        assert_eq!(held(&store), [2], "held by this copy's mark");
        assert_eq!(next.shown(), [2]);
        store.answer_forgotten();
        next.pump();
        assert_eq!(days(&store.load_sessions()), [2]);
        assert_eq!(store.open(), None, "stored, so no longer marked");

        // Deleted by hand under the record it would be stored as.
        let mut gone = day(3);
        gone.score = gone.points();
        let record =
            Record::of(&gone, |json| RecordId::placed(gone.timestamp, json)).expect("a record");
        let forget = HistoryChange {
            forget: vec![(record.id, Deleted::of(&gone, 7))],
            ..HistoryChange::default()
        };
        let mut pending = Store::change_history(&*store, forget);
        let mut asked = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(pending.as_mut().poll(&mut asked).is_ready());
        cut_off_keeping(&store, day(3));
        let mut next = Tab::on(&store);
        next.act(AppCtx::tell_of_cut_off);
        next.pump();
        assert_eq!(held(&store), [3], "held while the store looks");
        store.answer_forgotten();
        next.pump();
        assert_eq!(next.shown(), [2], "deleted by hand, it goes");
        assert_eq!(days(&store.load_sessions()), [2]);
        assert_eq!(store.open(), None, "nor held");
    }

    /// A session cut off is held by this copy's mark as soon as the mark
    /// that carried it is taken from the store, while what the store holds
    /// is taken up first: that look, made on a thread of its own on a
    /// computer or a phone, and a moment later in a browser, came before
    /// anything held the session, and the app stopping meanwhile lost it
    /// for good.
    #[test]
    fn a_session_cut_off_is_held_while_the_store_is_taken_up() {
        let store = holding([1]);
        cut_off_keeping(&store, day(2));
        let mut next = Tab::on(&store);
        // The look at the store waits on whether the session lost from
        // under it was deleted by hand.
        store.lose(next.id_of(1));
        store.forgotten_later.set(true);
        next.act(AppCtx::tell_of_cut_off);
        next.pump();
        let held = store
            .open()
            .map(|open| days(&open.unstored))
            .unwrap_or_default();
        assert_eq!(held, [2], "held by this copy's mark meanwhile");
        store.answer_forgotten();
        next.pump();
        store.answer_forgotten();
        next.pump();
        assert_eq!(days(&store.load_sessions()), [1, 2]);
        assert_eq!(next.shown(), [1, 2]);
        assert_eq!(store.open(), None, "stored, so no longer marked");
    }

    /// A session another copy of the app kept, which the store took before
    /// that copy stopped, its mark left behind, is stored once: a copy open
    /// all along, that has yet to take it up, takes it up as it finds the
    /// mark, and tells of nothing cut off. It used to look only at what it
    /// held itself, store the session again under a record of its own, and
    /// show it twice.
    #[test]
    fn a_session_another_copy_stored_as_it_stopped_is_stored_once() {
        let store = Rc::new(MemoryStore::default());
        let (a, mut b) = (Tab::on(&store), Tab::on(&store));
        let open = a.act(|ctx| ctx.mark_running(false));
        let so_far = SoFar {
            result: day(1),
            settings: None,
        };
        a.act(|ctx| ctx.mark_so_far(&open, so_far));
        // The store takes it, and the copy stops before it hears so.
        store.deferred.set(true);
        a.finish(day(1));
        store.settle();
        drop(a);
        store.deferred.set(false);
        assert_eq!(days(&store.load_sessions()), [1]);

        b.act(|ctx| ctx.mark_running(false));
        b.pump();
        assert_eq!(days(&store.load_sessions()), [1], "stored once");
        assert_eq!(b.shown(), [1]);
        assert_eq!(b.notices(), [], "kept, not cut off");
    }

    /// The one backup file saved in `folder`, read.
    #[cfg(feature = "native-runtime")]
    fn the_backup_in(folder: &TempDir) -> crate::persist::backup::Backup {
        let saved: Vec<std::path::PathBuf> = std::fs::read_dir(&folder.0)
            .expect("a folder")
            .map(|entry| entry.expect("an entry").path())
            .collect();
        assert_eq!(saved.len(), 1, "{saved:?}");
        let raw = std::fs::read_to_string(&saved[0]).expect("a file");
        crate::persist::backup::read_backup(&raw).expect("a backup")
    }

    /// A session deleted is in no backup saved right after, though the
    /// history is written on a thread of its own, and the backup asked for
    /// before the session was stored, or its deletion heard of as made: the
    /// thread makes each change in the order it was asked for, the
    /// deletion before the backup. Restoring the backup would bring the
    /// session back.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_session_deleted_is_in_no_backup_saved_right_after() {
        let dir = TempDir::new("deleted-then-backed-up");
        let folder = TempDir::new("deleted-then-backed-up-folder");
        let tab = Tab::on_store(crate::persist::files_on_worker_backing_up(
            &dir.0, &folder.0,
        ));
        tab.finish(day(1));
        tab.finish(day(2));
        tab.delete(1);
        tab.act(AppCtx::save_backup);
        // The changes, and the look the backup waits on; then the backup.
        tab.act(AppCtx::flush_history);
        tab.act(AppCtx::flush_history);
        assert_eq!(days(&the_backup_in(&folder).sessions), [2]);
        let toast = tab.toast().expect("told");
        assert!(toast.text.starts_with("Backup saved as "), "{}", toast.text);
        assert_eq!(Tab::on_store(crate::persist::files_at(&dir.0)).shown(), [2]);
    }

    /// Files that cannot be listed, as the backup is written on the
    /// history's own thread, still give a backup of every session shown,
    /// the one they would not take among them: the thread saves nothing of
    /// its own, and the app saves what it holds instead.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_backup_holds_every_session_shown_where_the_files_cannot_be_listed() {
        let dir = TempDir::new("unlisted-then-backed-up");
        let folder = TempDir::new("unlisted-then-backed-up-folder");
        let tab = Tab::on_store(crate::persist::files_on_worker_backing_up(
            &dir.0, &folder.0,
        ));
        tab.finish(day(1));
        tab.finish(day(2));
        tab.act(AppCtx::flush_history);
        // The folder of sessions is a file now, which cannot be listed nor
        // take a session, whoever runs the tests: root too.
        let sessions = dir.0.join("history").join("sessions");
        std::fs::rename(&sessions, dir.0.join("history").join("moved")).unwrap();
        std::fs::write(&sessions, "").unwrap();
        tab.finish(day(3));
        tab.act(AppCtx::save_backup);
        // The session not taken, and the look; the backup the thread could
        // not write; the backup of what the app holds.
        for _ in 0..3 {
            tab.act(AppCtx::flush_history);
        }
        assert_eq!(tab.shown(), [1, 2, 3]);
        assert_eq!(
            tab.act(|ctx| ctx.sessions.peek().unsaved().len()),
            1,
            "the third is only here"
        );
        assert_eq!(days(&the_backup_in(&folder).sessions), [1, 2, 3]);
        let toast = tab.toast().expect("told");
        assert!(toast.text.starts_with("Backup saved as "), "{}", toast.text);
    }

    /// A session stored whose file cannot be read as the backup is
    /// written, though it read as the app started, goes in as the app
    /// holds it. It used to be left out, and told of as damaged.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_session_whose_file_cannot_be_read_is_backed_up_as_shown() {
        let dir = TempDir::new("unread-then-backed-up");
        let folder = TempDir::new("unread-then-backed-up-folder");
        let tab = Tab::on_store(crate::persist::files_on_worker_backing_up(
            &dir.0, &folder.0,
        ));
        tab.finish(day(1));
        tab.finish(day(2));
        tab.act(AppCtx::flush_history);
        // Its file is a folder now: listed, and not read, by root too.
        let first = dir
            .0
            .join("history")
            .join("sessions")
            .join(format!("{}.json", tab.id_of(1)));
        std::fs::remove_file(&first).unwrap();
        std::fs::create_dir(&first).unwrap();
        tab.act(AppCtx::save_backup);
        for _ in 0..3 {
            tab.act(AppCtx::flush_history);
        }
        assert_eq!(days(&the_backup_in(&folder).sessions), [1, 2]);
        let toast = tab.toast().expect("told");
        assert!(!toast.text.contains("could not go in it"), "{}", toast.text);
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
