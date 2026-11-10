//! Sync in the app: what the card on the History tab shows of it
//! ([`SyncStatus`]), the moves the card makes (turning sync on, syncing now,
//! turning it off), and when the app syncs by itself: as it opens, a moment
//! after the history or the settings change here, and as it comes back
//! into sight, if it has not synced for a while. Never while a session or
//! a run is under way, which waits on nothing: once it is over.
//!
//! Copies of the app on one device, a browser's tabs or a computer's
//! windows, keep what sync keeps in one place, the store's. Each lays what
//! it did over what the store holds as it writes, rather than writing its
//! own whole over what another kept ([`SyncState::merged`]); and each sync
//! begins from what the store holds, and lays what it did over the store as
//! it ends.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use super::AppCtx;
use crate::history::settle_or_spawn;
use crate::persist::record::{Pending, RecordId, done};
use crate::persist::{Lock, LockHeld, Locking, Store, merge_settings};
use crate::sync::auth::{Auth, AuthError};
use crate::sync::remote::{Remote, RemoteError, RemoteFile};
use crate::sync::run::{Found, SyncFailed, Synced, delete_copy, join, look, sync_once_with};
use crate::sync::seal::{MIN_PASSPHRASE, Stretch};
use crate::sync::settings::{as_keys, settings_keys};
use crate::sync::state::SyncState;
use crate::sync::text::random_hex;
use crate::time::{POLL_MS, mono_ms, now_ms, sleep_ms};

/// How long after a change here the app syncs it: long enough for the
/// changes a moment brings, a session kept and its settings, to go as one.
pub const SYNC_SOON_MS: u32 = 3_000;
/// How long the app goes without syncing before coming back into sight
/// syncs it.
pub const SYNC_STALE_MS: u64 = 2 * 60_000;
/// How long a sync that would start mid-session waits before it looks
/// again.
const SESSION_WAIT_MS: u32 = 5_000;

/// What the card shows of sync.
#[derive(Clone, Debug, PartialEq)]
pub enum SyncStatus {
    /// This build cannot sync, or this browser cannot this visit: why, in
    /// words, where it is the browser.
    Unavailable(Option<&'static str>),
    Off {
        problem: Option<String>,
    },
    /// Waiting on Google's sign-in.
    SigningIn,
    /// Signed in, and asked for the passphrase: `new` where the drive holds
    /// no sync yet, and one is chosen.
    Passphrase {
        new: bool,
        problem: Option<String>,
    },
    /// Stretching the passphrase, and opening or making the keyring.
    Unlocking,
    On {
        /// When this device last synced.
        last: Option<u64>,
        syncing: bool,
        problem: Option<String>,
        /// Only the user can sign in again: a browser's token ran out, or
        /// the access was taken back.
        needs_sign_in: bool,
    },
    /// Deleting the synced copy from the drive.
    TurningOff,
}

impl SyncStatus {
    pub fn is_on(&self) -> bool {
        matches!(self, Self::On { .. })
    }
}

/// How this device reaches the drive: the sign-in, and the folder it opens.
#[derive(Clone)]
pub struct Link {
    pub auth: Rc<dyn Auth>,
    pub remote: Rc<dyn Remote>,
}

impl Link {
    /// This build's: Google Drive, through its sign-in, where it has one.
    fn default_link() -> Option<Self> {
        // A test links its own ([`AppCtx::link_sync`]).
        if cfg!(test) {
            return None;
        }
        let auth = crate::sync::auth::Counted::of(crate::sync::auth::default_auth()?);
        let drive =
            crate::sync::drive::Drive::new(crate::sync::http::default_http(), Rc::clone(&auth));
        Some(Self {
            auth,
            remote: Rc::new(drive),
        })
    }
}

/// Sync's part of the app.
#[derive(Clone, Copy)]
pub struct SyncCtx {
    pub status: Signal<SyncStatus>,
    /// The theme a sync took from another device, for the app's shell to
    /// show: stored already.
    pub theme: Signal<Option<String>>,
    link: CopyValue<Option<Link>>,
    /// What sync keeps, while it is on, as this copy of the app holds it:
    /// what the store held as this copy last read or kept it there
    /// ([`SyncCtx::base`]), with what this copy did since laid over it.
    state: CopyValue<Option<SyncState>>,
    /// What the store held for sync as this copy of the app last read or
    /// kept it there: none where the store has never taken this copy's
    /// sync, as one just turned on here. Other copies on the same store, a
    /// tab or a window, keep what they do there as well, each laying it
    /// over what the store holds as it writes ([`SyncState::merged`]): gone
    /// from the store since, sync was turned off by one of them, and
    /// another sync there was turned on anew by one.
    base: CopyValue<Option<SyncState>>,
    /// Which turning on of sync `state` is of: moved on as sync is turned
    /// off here, and as it is turned on, so that a sync under way across
    /// either keeps nothing of what it did.
    epoch: CopyValue<u64>,
    /// What the sign-in found in the drive, for the passphrase to open.
    found: CopyValue<Option<Found>>,
    /// Which asking for a sign-in is the latest: moved on as each is asked
    /// for, and as one signs in, so that one under way from before either
    /// says nothing of failing, the later asking or the sign-in made having
    /// the say. Signed in, it goes on all the same: on Android the user
    /// answers the first asked, and a later one is told at once that it
    /// waits.
    attempt: CopyValue<u64>,
    /// The latest asking for a sign-in given up, by Cancel or as sync is
    /// turned off: none asked for by then goes any further as it finishes.
    given_up: CopyValue<u64>,
    /// A sync is under way, and another is asked for after it.
    running: CopyValue<bool>,
    again: CopyValue<bool>,
    /// A sync is waiting to start ([`AppCtx::sync_soon`]).
    soon: CopyValue<bool>,
    /// Sessions put back by hand, each with when by the clock, not yet in
    /// what sync keeps: each goes in as it is noted, stamped after what
    /// this copy then knows of the drive ([`SyncState::stamp_change`]),
    /// but while a sync has taken in those noted before it
    /// ([`SyncCtx::taking_in`]), it waits for that sync's end, and is
    /// stamped after what it read and sent. Turning sync off here forgets
    /// them with the rest, while taking up sync turned off, or on anew, in
    /// another copy keeps them, with those not yet sent ([`Notes::Kept`]):
    /// those a sync under way took in, as it hands them back, unless it
    /// sent them ([`SyncCtx::hand_back`]). The sync under way lets go of
    /// none of them all the same ([`SyncCtx::put_back`]).
    revived: CopyValue<BTreeMap<RecordId, u64>>,
    /// A sync of sync as it is on now has taken in the sessions put back
    /// before it, and has yet to keep what it comes to.
    taking_in: CopyValue<bool>,
    /// The sessions put back by hand that the sync under way took in as it
    /// began, each at its stamp then: it alone knows which of them it sent.
    /// Emptied as it ends, or as this copy of the app forgets sync
    /// meanwhile, turned off here, or taken for off or on anew in another
    /// copy ([`SyncCtx::hand_back`]).
    taken_in: CopyValue<BTreeMap<RecordId, u64>>,
    /// Sync was taken for off, or on anew, in another copy of the app while
    /// the sync under way held notes it took in: it hands back those it did
    /// not send, which go as the notes kept then go ([`Notes::Kept`]).
    hand_back: CopyValue<bool>,
    /// Every session put back by hand here while a sync is under way,
    /// whatever became of sync since: that sync lets none of them go, as it
    /// lets go of none another copy put back meanwhile, which the store's
    /// notes show ([`AppCtx::put_back_since`]). Emptied as each begins.
    put_back: CopyValue<BTreeSet<RecordId>>,
}

impl SyncCtx {
    /// Sync as `store` left it, on this build's link.
    pub fn new(store: &dyn Store) -> Self {
        Self::linked(store, Link::default_link())
    }

    /// Sync as `store` left it, on `link`: none where this build cannot
    /// sync.
    fn linked(store: &dyn Store, link: Option<Link>) -> Self {
        let state = link
            .as_ref()
            .and_then(|_| store.load_sync())
            .and_then(|raw| SyncState::read(&raw));
        let status = match (&link, &state) {
            (None, _) => SyncStatus::Unavailable(None),
            (Some(_), _) if !store.syncs_history() => SyncStatus::Unavailable(Some(
                "This browser keeps your history in its small page storage this visit, which cannot sync.",
            )),
            (Some(link), Some(state)) => {
                link.auth.keep(state.sign_in.clone());
                SyncStatus::On {
                    last: state.last,
                    syncing: false,
                    problem: None,
                    needs_sign_in: false,
                }
            }
            (Some(_), None) => SyncStatus::Off { problem: None },
        };
        Self {
            status: Signal::new(status),
            theme: Signal::new(None),
            link: CopyValue::new(link),
            base: CopyValue::new(state.clone()),
            state: CopyValue::new(state),
            epoch: CopyValue::new(0),
            found: CopyValue::new(None),
            attempt: CopyValue::new(0),
            given_up: CopyValue::new(0),
            running: CopyValue::new(false),
            again: CopyValue::new(false),
            soon: CopyValue::new(false),
            revived: CopyValue::new(BTreeMap::new()),
            taking_in: CopyValue::new(false),
            taken_in: CopyValue::new(BTreeMap::new()),
            hand_back: CopyValue::new(false),
            put_back: CopyValue::new(BTreeSet::new()),
        }
    }
}

/// Why sync is off here, once another copy of the app on the same store
/// turned it off.
const OFF_ELSEWHERE: &str = "Sync was turned off in another tab or window of Dust.";
/// Why sync is off here, once another copy of the app on the same store
/// turned it on anew, and keeps its own there.
const ANEW_ELSEWHERE: &str =
    "Sync was turned on anew in another tab or window of Dust. Open Dust again here to sync.";

/// What the store keeps for sync, as it was read just now ([`read_kept`]).
enum Kept {
    /// Nothing: sync was turned off, by whichever copy of the app.
    Cleared,
    /// It could not be read just now, as a file another program holds open
    /// cannot be: no word of anything.
    Unread,
    /// What sync keeps, as this build reads it.
    State(Box<SyncState>),
    /// What this build does not read: a later build's, say.
    Unknown,
}

/// What becomes of the sessions put back by hand that sync has yet to
/// send ([`SyncCtx::revived`], [`SyncState::revived`]) as sync is forgotten
/// in this copy of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Notes {
    /// Forgotten with the rest: sync turned off here, or ended for good,
    /// as its copy in the drive is gone.
    Forgotten,
    /// Kept: sync was turned off in another copy, or on anew there, and the
    /// history they were put back in is this device's still. They go with
    /// the sync turned on anew there ([`AppCtx::give_notes_to`]), or with
    /// the next one turned on here, as noted before it, stamped again after
    /// its first read of the drive; closed first, this copy loses them.
    /// Either way each is put back after every deletion of its session that
    /// sync knows of, or finds at that first read, whenever it was made: a
    /// deletion another device made after the putting back loses to it, as
    /// what this copy knew of the drive as it noted it goes with its sync.
    /// Those a sync under way took in go as it ends, those it did not send
    /// alone ([`SyncCtx::hand_back`]): one it sent is no note to keep.
    Kept,
}

/// What `store` keeps for sync now.
fn read_kept(store: &dyn Store) -> Kept {
    if store.sync_cleared() {
        return Kept::Cleared;
    }
    match store.load_sync() {
        None => Kept::Unread,
        Some(raw) => {
            SyncState::read(&raw).map_or(Kept::Unknown, |state| Kept::State(Box::new(state)))
        }
    }
}

/// Whether `kept` is a state of the sync `ours` is of: of the same device,
/// joined to the same keyring. One turned on anew is of another device.
fn same_sync(kept: &SyncState, ours: &SyncState) -> bool {
    kept.device == ours.device && kept.keyring == ours.keyring
}

/// Note `id` as put back by hand at `at` in `notes`: at the later, where it
/// was noted already.
fn note(notes: &mut BTreeMap<RecordId, u64>, id: RecordId, at: u64) {
    let noted = notes.entry(id).or_insert(at);
    *noted = (*noted).max(at);
}

/// How long a copy of the app waits for another to let go of the lock on
/// what sync keeps, which each holds only while it reads, lays over and
/// writes, before it goes on without it.
const STATE_LOCK_WAIT_MS: u64 = 3_000;
/// How often a sync waiting for another copy's tries the lock on syncing
/// again.
const RUN_LOCK_POLL_MS: u32 = 250;
/// How long a sync waits for another copy's to end, before it goes on
/// without the lock: ten minutes, as one copy hung keeps every other from
/// syncing no longer.
const RUN_LOCK_WAIT_MS: u64 = 10 * 60_000;

/// `lock`, which every copy of the app on `store` shares, once this copy
/// holds it: tried again every `every_ms` while another copy holds it,
/// `within_ms` at most, and then none, so the work goes on without it, as
/// it did before copies locked; none at once where the store takes no such
/// lock. It never blocks the thread: a wait is a sleep.
async fn lock_within(
    store: &dyn Store,
    lock: Lock,
    every_ms: u32,
    within_ms: u64,
) -> Option<LockHeld> {
    let began = mono_ms();
    loop {
        match store.try_lock(lock).await {
            Locking::Held(held) => return Some(held),
            Locking::Unlockable => return None,
            Locking::Busy if mono_ms().saturating_sub(began) >= within_ms => return None,
            Locking::Busy => sleep_ms(every_ms).await,
        }
    }
}

/// The lock on what sync keeps, as a copy reads, lays over and writes it
/// ([`Lock::SyncState`]): a browser takes none.
async fn lock_state(store: &dyn Store) -> Option<LockHeld> {
    lock_within(store, Lock::SyncState, POLL_MS, STATE_LOCK_WAIT_MS).await
}

/// What the card says of a passphrase that will not do for a new sync, if
/// anything.
pub fn passphrase_problem(passphrase: &str, repeated: &str) -> Option<String> {
    if passphrase.trim().chars().count() < MIN_PASSPHRASE {
        Some(format!(
            "A passphrase needs {MIN_PASSPHRASE} characters at least: a few words are best."
        ))
    } else if passphrase.trim() != repeated.trim() {
        Some("The two passphrases are not the same.".into())
    } else {
        None
    }
}

impl AppCtx {
    fn sync_link(self) -> Option<Link> {
        self.sync.link.peek().clone()
    }

    fn set_sync_status(self, status: SyncStatus) {
        let mut shown = self.sync.status;
        if *shown.peek() != status {
            shown.set(status);
        }
    }

    /// The card's status while sync is on, with `change` made to it.
    fn change_on_status(
        self,
        change: impl FnOnce(&mut Option<u64>, &mut bool, &mut Option<String>, &mut bool),
    ) {
        let mut status = self.sync.status.peek().clone();
        if let SyncStatus::On {
            last,
            syncing,
            problem,
            needs_sign_in,
        } = &mut status
        {
            change(last, syncing, problem, needs_sign_in);
            self.set_sync_status(status);
        }
    }

    /// Keep what sync keeps, as this copy holds it now, laid over what the
    /// store holds, which another copy of the app on the same store, a tab
    /// or a window, may have kept since this one last read or kept it: what
    /// each did stays ([`SyncState::merged`]). The sessions put back by hand
    /// noted meanwhile go in with it ([`SyncCtx::revived`]), unless a sync
    /// has taken in those noted before it, whose end keeps them. Told of
    /// where it cannot be kept. Where another copy turned sync off since, or
    /// on anew, nothing is kept, and sync is off here
    /// ([`Self::take_up_kept`]).
    ///
    /// Read, laid over and written back under the lock on what sync keeps
    /// ([`Lock::SyncState`]), so that no other copy writes between: at once
    /// where it is free, or where the store takes no such lock, as a
    /// browser's does not; otherwise once the copy holding it lets go, a
    /// moment later ([`STATE_LOCK_WAIT_MS`] at most).
    fn keep_sync_state(self) {
        let store = self.app().store;
        settle_or_spawn(
            Box::pin(async move {
                let _held = lock_state(&*store).await;
                self.keep_now();
            }),
            |()| {},
        );
    }

    /// Keep what sync keeps, as [`Self::keep_sync_state`] does, now. Where
    /// sync is off here, as taken for off or on anew in another copy of the
    /// app, the sessions put back here that sync has yet to send go with
    /// the sync the store keeps, if it keeps one ([`Self::give_notes_to`]).
    fn keep_now(self) {
        let Some(ours) = self.sync.state.peek().clone() else {
            if !self.sync.revived.peek().is_empty() {
                self.give_notes_to(read_kept(&*self.app().store));
            }
            return;
        };
        let kept = read_kept(&*self.app().store);
        if let Some(why) = self.changed_elsewhere(&kept) {
            self.take_up_change_elsewhere(why);
            self.give_notes_to(kept);
            return;
        }
        let base = self.sync.base.peek().clone();
        let (mut merged, theirs) = match kept {
            Kept::State(theirs) => (ours.merged(base.as_ref(), &theirs), Some(*theirs)),
            // Nothing this copy can lay what it holds over: kept whole, as
            // it was before copies merged.
            Kept::Cleared | Kept::Unread | Kept::Unknown => (ours, None),
        };
        if !*self.sync.taking_in.peek() {
            for (id, at) in take(self.sync.revived) {
                let at = merged.stamp_change(id, at);
                note(&mut merged.revived, id, at);
            }
        }
        self.write_kept(merged, theirs);
    }

    /// Write `merged` for sync, laid over `theirs`, what the store held of
    /// this sync as it was read just before: once written, it is what this
    /// copy holds and what the store holds. Where it could not be, it is
    /// what this copy holds all the same, `theirs` what the store holds, so
    /// that the next write lays what this copy did over the store again;
    /// and the user is told.
    fn write_kept(self, merged: SyncState, theirs: Option<SyncState>) {
        self.take_up_sign_in(&merged);
        let saved = match merged.written() {
            Some(written) => self.app().store.save_sync(Some(&written)),
            None => Err("it could not be written out".into()),
        };
        match saved {
            Ok(()) => put(self.sync.base, Some(merged.clone())),
            Err(why) => {
                if let Some(theirs) = theirs.filter(|theirs| same_sync(theirs, &merged)) {
                    put(self.sync.base, Some(theirs));
                }
                self.toasts.show(format!(
                    "What sync keeps could not be saved: {}.",
                    why.trim_end_matches('.')
                ));
            }
        }
        put(self.sync.state, Some(merged));
    }

    /// Keep `sending`, what the sync under way of the turning on of sync
    /// `epoch` has come to, as it sends deletions by hand: with what it
    /// took into the store, and the pack it is sending, which the drive may
    /// make though the app stops before the sync ends
    /// ([`SyncState::unanswered`]). It is laid over what the store holds,
    /// as against `base`, what the sync began from or last kept this way,
    /// which becomes `sending` once kept: the sync's end lays what it does
    /// after over the store in turn ([`Self::keep_synced`]). The sign-in
    /// kept is the store's, or one made here that the store has yet to take,
    /// before the sync began or since ([`Self::sign_in_made_here`]): one
    /// taken up from another copy meanwhile is that copy's, which the store
    /// holds as that copy last made it. Nothing is kept where sync was turned
    /// off or on anew since, here or in another copy of the app, which the
    /// sync's end takes up; nor is a failing told of, which the sync's end
    /// tells. Under the lock on what sync keeps, as every keep is
    /// ([`Self::keep_sync_state`]).
    async fn keep_sync_under_way(self, epoch: u64, sending: &SyncState, base: &RefCell<SyncState>) {
        let _held = lock_state(&*self.app().store).await;
        if *self.sync.epoch.peek() != epoch {
            return;
        }
        let Some(here) = self.sync.state.peek().clone() else {
            return;
        };
        let store = self.app().store;
        let kept = read_kept(&*store);
        if self.changed_elsewhere(&kept).is_some() {
            return;
        }
        // A sync the store never took is laid over nothing of another's.
        let taken = self.sync.base.peek().is_some();
        let mut merged = match &kept {
            Kept::State(theirs) => sending.merged(taken.then_some(&*base.borrow()), theirs),
            Kept::Cleared | Kept::Unread | Kept::Unknown => sending.clone(),
        };
        if let Some(sign_in) = self.sign_in_made_here(&here, &base.borrow()) {
            merged.sign_in = sign_in;
        }
        if let Some(written) = merged.written()
            && store.save_sync(Some(&written)).is_ok()
        {
            // A deletion's note the sync dropped since, as its session was
            // put back here, and the write kept, as the drive shows no
            // putting back of it yet ([`SyncState::merged`]), is one the
            // sync's end lays what it did over still: dropped then, once the
            // drive shows it, and not taken for one the store gained.
            let mut kept = sending.clone();
            for (&id, &at) in &base.borrow().applied {
                if !kept.applied.contains_key(&id) && merged.applied.get(&id) == Some(&at) {
                    kept.applied.insert(id, at);
                }
            }
            *base.borrow_mut() = kept;
        }
    }

    /// The sign-in `here` holds, this copy's state, where it is one made
    /// here that the store has yet to take, as against `base`, what a sync
    /// under way began from or last kept as it went: one that differs from
    /// `base`'s, and from the sign-in this copy last read or kept in the
    /// store. One taken up from another copy since the sync began is none,
    /// though it differs from `base`'s: that copy may have signed in again
    /// since, and the later is the one kept. One the store would not take
    /// is, as this copy last read the store's ([`Self::write_kept`]).
    fn sign_in_made_here(self, here: &SyncState, base: &SyncState) -> Option<Option<String>> {
        let seen = self
            .sync
            .base
            .peek()
            .as_ref()
            .map(|seen| seen.sign_in.clone());
        (here.sign_in != base.sign_in && seen.as_ref() != Some(&here.sign_in))
            .then(|| here.sign_in.clone())
    }

    /// Keep `synced`, what the sync of the turning on of sync `epoch` came
    /// to, as it ends, laid over what the store holds now, as against
    /// `base`, what it began from or last kept as it sent deletions by hand
    /// ([`Self::keep_sync_under_way`]): what another copy of the app on the
    /// same store kept meanwhile stays, and what this sync did is kept over
    /// it ([`SyncState::merged`]). A session put back by hand meanwhile,
    /// here or in another copy, is stamped again after what this sync read
    /// and sent ([`SyncState::stamp_change`]), as it was put back after the
    /// sync began: after the deletion this sync sent, which it may undo,
    /// and so after a deletion of it made elsewhere later by the clock that
    /// this sync read, which it then beats (`docs/sync.md`). One deleted by
    /// hand again since is spent, and is no more noted
    /// ([`Self::deleted_again`]): the deletion is this sync's, or the next
    /// one's, to send, stamped as it was made. Noted again, after the
    /// deletion this sync sent and noted, the next sync took it for a
    /// putting back since, dropped that note, and sent the deletion again,
    /// stamped after all this one read: after a putting back made elsewhere
    /// later than the deletion by the clock. The sign-in is no sync's to
    /// keep: the store's, or one
    /// made here that the store has yet to take, before the sync began or
    /// meanwhile ([`Self::sign_in_made_here`]), not one taken up from another
    /// copy meanwhile. Nothing is kept where sync was turned off or on anew
    /// meanwhile, here or in another copy: off here as well, then.
    /// Under the lock on what sync keeps, as every keep is
    /// ([`Self::keep_sync_state`]): a putting back noted while it waits for
    /// it waits with it.
    async fn keep_synced(self, epoch: u64, base: &SyncState, synced: SyncState) {
        let _held = lock_state(&*self.app().store).await;
        // Asked before what sync keeps is read, so that nothing comes
        // between that read and the write.
        let deleted_again = self.deleted_again(base).await;
        put(self.sync.taking_in, false);
        if *self.sync.epoch.peek() != epoch {
            // Taken for off, or on anew, in another copy as it waited: what
            // it took in and did not send goes as the notes kept then go.
            if take(self.sync.hand_back) {
                self.note_waiting(&synced.revived);
                self.keep_now();
            }
            return;
        }
        let kept = read_kept(&*self.app().store);
        if let Some(why) = self.changed_elsewhere(&kept) {
            // What it took in and did not send waits with what was noted
            // since, as the rest of what this copy noted does.
            self.note_waiting(&synced.revived);
            self.take_up_change_elsewhere(why);
            self.give_notes_to(kept);
            return;
        }
        // A sync the store never took is laid over nothing of another's:
        // kept whole over another sync the store holds, as the one last
        // turned on here.
        let taken = self.sync.base.peek().is_some();
        let (mut merged, theirs) = match kept {
            Kept::State(theirs) => (synced.merged(taken.then_some(base), &theirs), Some(*theirs)),
            Kept::Cleared | Kept::Unread | Kept::Unknown => (synced, None),
        };
        let made_here = self
            .sync
            .state
            .peek()
            .as_ref()
            .and_then(|here| self.sign_in_made_here(here, base));
        if let Some(sign_in) = made_here {
            merged.sign_in = sign_in;
        }
        // Noted in another copy since: where it still stands.
        let elsewhere: Vec<(RecordId, u64)> = theirs
            .iter()
            .flat_map(|theirs| &theirs.revived)
            .filter(|&(id, &at)| base.revived.get(id).is_none_or(|&was| at > was))
            .filter(|(id, _)| merged.revived.contains_key(id))
            .map(|(&id, &at)| (id, at))
            .collect();
        for (id, at) in elsewhere.into_iter().chain(take(self.sync.revived)) {
            if deleted_again.contains(&id) {
                merged.revived.remove(&id);
                continue;
            }
            let at = merged.stamp_change(id, at);
            note(&mut merged.revived, id, at);
        }
        self.write_kept(merged, theirs);
    }

    /// Of the sessions put back by hand while a sync was under way, here or
    /// in another copy of the app, as the notes waiting here and those the
    /// store kept since `base` show them, those deleted by hand again since:
    /// a putting back takes the session's tombstone away, so one the store
    /// holds of it now was made after. None where the store cannot say.
    async fn deleted_again(self, base: &SyncState) -> BTreeSet<RecordId> {
        let store = self.app().store;
        let mut put_back: BTreeSet<RecordId> = self.sync.revived.peek().keys().copied().collect();
        if let Kept::State(kept) = read_kept(&*store)
            && same_sync(&kept, base)
        {
            put_back.extend(
                kept.revived
                    .iter()
                    .filter(|&(id, &at)| base.revived.get(id).is_none_or(|&was| at > was))
                    .map(|(&id, _)| id),
            );
        }
        if put_back.is_empty() {
            return BTreeSet::new();
        }
        store
            .deletions(put_back.into_iter().collect())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Take up what another copy of the app on the same store, a tab or a
    /// window, kept for sync since this one last read or kept it there,
    /// laid under what this copy did since ([`SyncState::merged`]): its
    /// notes, when this device last synced, which the card shows, and a
    /// sign-in made there, which this copy signs in with from then on. Or,
    /// where it turned sync off since, or on anew and kept its own, turn
    /// sync off here as well, saying why ([`Self::changed_elsewhere`]). Says
    /// whether sync is on here still. It only reads.
    fn take_up_kept(self) -> bool {
        let Some(ours) = self.sync.state.peek().clone() else {
            return false;
        };
        let kept = read_kept(&*self.app().store);
        if let Some(why) = self.changed_elsewhere(&kept) {
            match kept {
                // Taken up as this copy keeps what it holds, under the lock,
                // so that the sessions put back here go with that sync.
                Kept::State(_) => self.keep_sync_state(),
                Kept::Cleared | Kept::Unread | Kept::Unknown => self.take_up_change_elsewhere(why),
            }
            return false;
        }
        let base = self.sync.base.peek().clone();
        if let (Kept::State(theirs), Some(base)) = (kept, base)
            && *theirs != base
        {
            let merged = ours.merged(Some(&base), &theirs);
            self.take_up_sign_in(&merged);
            let last = merged.last;
            put(self.sync.state, Some(merged));
            put(self.sync.base, Some(*theirs));
            self.change_on_status(|shown, _, _, _| *shown = last);
        }
        true
    }

    /// Sign in with what `merged` keeps to, where it is a sign-in another
    /// copy of the app made, as a merge takes it from the store: a refresh
    /// token another window got after this one's was turned down, say. As
    /// the app opening takes up the one kept ([`SyncCtx::new`]).
    fn take_up_sign_in(self, merged: &SyncState) {
        let here = self
            .sync
            .state
            .peek()
            .as_ref()
            .and_then(|state| state.sign_in.clone());
        if merged.sign_in.is_some()
            && merged.sign_in != here
            && let Some(link) = self.sync_link()
        {
            link.auth.keep(merged.sign_in.clone());
        }
    }

    /// Take up what another copy of the app kept for sync as it tells of
    /// it, where the store hears of such words ([`Store::sync_news`]), as a
    /// browser's tabs do: a tab left open beside another shows sync turned
    /// off there at once, and takes up what that one synced and signed in
    /// with.
    pub(super) fn hear_sync_news(self) {
        let store = self.app().store;
        let Some(first) = store.sync_news() else {
            return;
        };
        spawn_forever(async move {
            let mut news = first;
            loop {
                news.await;
                self.take_up_kept();
                match store.sync_news() {
                    Some(next) => news = next,
                    None => return,
                }
            }
        });
    }

    /// Turn sync off here, saying `why`: another copy of the app on the
    /// same store turned it off, or on anew. What the store holds is left
    /// as it is: nothing, or the other copy's. The sessions put back by hand
    /// here that sync has yet to send are kept ([`Notes::Kept`]), for the
    /// sync turned on anew there ([`Self::give_notes_to`]), or the next one
    /// turned on here.
    fn take_up_change_elsewhere(self, why: &'static str) {
        self.forget_sync_here(Notes::Kept);
        self.set_sync_status(SyncStatus::Off {
            problem: Some(why.into()),
        });
    }

    /// Note `notes`, sessions put back by hand, each at its stamp, among
    /// those waiting to go into what sync keeps ([`SyncCtx::revived`]).
    fn note_waiting(self, notes: &BTreeMap<RecordId, u64>) {
        let mut noted = self.sync.revived;
        let mut noted = noted.write();
        for (&id, &at) in notes {
            note(&mut noted, id, at);
        }
    }

    /// Lay the sessions put back by hand here that sync has yet to send,
    /// kept as sync was turned on anew in another copy of the app, over that
    /// sync's state, `kept`, as the store holds it, and write it: the
    /// history is the device's, and so the putting back stands against
    /// whichever sync the store keeps, which sends it next. Each is put back
    /// after what that sync knows of its record ([`SyncState::stamp_change`]).
    /// Under the lock on what sync keeps, which the caller holds. Where the
    /// store holds nothing, or what this build does not read, or will not
    /// take it, they wait with this copy, for the next sync turned on here.
    fn give_notes_to(self, kept: Kept) {
        let Kept::State(mut theirs) = kept else {
            return;
        };
        let notes = self.sync.revived.peek().clone();
        if notes.is_empty() {
            return;
        }
        for (id, at) in notes {
            let at = theirs.stamp_change(id, at);
            note(&mut theirs.revived, id, at);
        }
        if let Some(written) = theirs.written()
            && self.app().store.save_sync(Some(&written)).is_ok()
        {
            put(self.sync.revived, BTreeMap::new());
        }
    }

    /// Why sync is no longer as this copy of the app holds it, where
    /// another copy has changed it in the store, `kept`, since this one last
    /// read or kept its own there: turned off, or turned on anew, which
    /// keeps another device and keyring, or what this build does not read.
    /// A store that could not be read is no word of either; nor is anything
    /// the store holds while it has never taken this copy's.
    fn changed_elsewhere(self, kept: &Kept) -> Option<&'static str> {
        let base = self.sync.base.peek();
        let base = base.as_ref()?;
        match kept {
            Kept::Unread => None,
            Kept::State(theirs) if same_sync(theirs, base) => None,
            Kept::Cleared => Some(OFF_ELSEWHERE),
            Kept::State(_) | Kept::Unknown => Some(ANEW_ELSEWHERE),
        }
    }

    /// Ask the user to sign in to Google: to turn sync on, or to sync again
    /// once a browser's sign-in has run out. Called from the press of the
    /// card's button, as a browser lets only that open Google's window.
    /// Asked for again meanwhile, a sign-in under way goes on should it
    /// sign in, but says nothing should it fail ([`SyncCtx::attempt`]).
    pub fn sync_sign_in(self) {
        let Some(link) = self.sync_link() else {
            return;
        };
        let attempt = self.next_sign_in();
        let was_on = self.sync.state.peek().is_some();
        let signing_in = link.auth.sign_in();
        if !was_on {
            self.set_sync_status(SyncStatus::SigningIn);
        }
        spawn_forever(async move {
            let signed_in = signing_in.await;
            if self.sign_in_given_up(attempt, &link) {
                return;
            }
            // Failing, it says so only where none was asked for since, nor
            // signed in: the later one, or the sign-in made, has the say.
            let latest = *self.sync.attempt.peek() == attempt;
            if signed_in.is_ok() {
                self.next_sign_in();
            }
            if was_on {
                match signed_in {
                    Ok(()) => {
                        let kept = link.auth.kept();
                        let mut state = self.sync.state;
                        if let Some(state) = state.write().as_mut() {
                            state.sign_in = kept;
                        }
                        self.keep_sync_state();
                        self.change_on_status(|_, _, problem, needs| {
                            *problem = None;
                            *needs = false;
                        });
                        self.sync_now();
                    }
                    Err(err) if latest => {
                        self.change_on_status(|_, _, problem, _| *problem = Some(err.told()))
                    }
                    Err(_) => {}
                }
                return;
            }
            if let Err(err) = signed_in {
                if latest {
                    let problem = (err != AuthError::Cancelled).then(|| err.told());
                    self.set_sync_status(SyncStatus::Off { problem });
                }
                return;
            }
            let found = look(&*link.remote).await;
            if self.sign_in_given_up(attempt, &link) {
                return;
            }
            match found {
                Ok(found) => {
                    let new = found == Found::Nothing;
                    put(self.sync.found, Some(found));
                    self.set_sync_status(SyncStatus::Passphrase { new, problem: None });
                }
                Err(err) => {
                    link.auth.forget();
                    self.set_sync_status(SyncStatus::Off {
                        problem: Some(format!("Sync was not turned on: {}.", err.why())),
                    });
                }
            }
        });
    }

    /// Open the sync the sign-in found with `passphrase`, or begin one with
    /// it, and sync. A new passphrase is asked for twice, `repeated` the
    /// second.
    pub fn sync_passphrase(self, passphrase: String, repeated: Option<String>) {
        let Some(link) = self.sync_link() else {
            return;
        };
        let Some(found) = self.sync.found.peek().clone() else {
            return;
        };
        let new = found == Found::Nothing;
        if new
            && let Some(problem) =
                passphrase_problem(&passphrase, repeated.as_deref().unwrap_or_default())
        {
            self.set_sync_status(SyncStatus::Passphrase {
                new,
                problem: Some(problem),
            });
            return;
        }
        self.set_sync_status(SyncStatus::Unlocking);
        spawn_forever(async move {
            // What the card says is drawn before the passphrase is
            // stretched, which takes the app's thread a moment.
            sleep_ms(POLL_MS).await;
            match join(&*link.remote, &passphrase, &found, stretch()).await {
                Ok((key, keyring)) => {
                    let Some(device) = random_hex::<16>() else {
                        self.set_sync_status(SyncStatus::Off {
                            problem: Some("This device could not make a key for sync.".into()),
                        });
                        return;
                    };
                    put(self.sync.found, None);
                    self.next_sync_epoch();
                    // The store has never taken this sync: kept whole, over
                    // whatever another copy keeps there.
                    put(self.sync.base, None);
                    put(
                        self.sync.state,
                        Some(SyncState::new(
                            device,
                            key.to_hex(),
                            keyring,
                            link.auth.kept(),
                        )),
                    );
                    self.keep_sync_state();
                    self.set_sync_status(SyncStatus::On {
                        last: None,
                        syncing: false,
                        problem: None,
                        needs_sign_in: false,
                    });
                    self.sync_now();
                }
                Err(err) => self.set_sync_status(SyncStatus::Passphrase {
                    new,
                    problem: Some(err.told()),
                }),
            }
        });
    }

    /// Whether the sign-in `attempt` was given up since it was asked for,
    /// by Cancel or as sync was turned off: then it goes no further. What
    /// it signed in as it finished is forgotten, as Cancel forgot what it
    /// had before, where sync is off and no other sign-in is under way.
    fn sign_in_given_up(self, attempt: u64, link: &Link) -> bool {
        if attempt > *self.sync.given_up.peek() {
            return false;
        }
        if self.sync.state.peek().is_none()
            && matches!(*self.sync.status.peek(), SyncStatus::Off { .. })
        {
            link.auth.forget();
        }
        true
    }

    /// Move on to the next asking for a sign-in ([`SyncCtx::attempt`]), and
    /// say which it is.
    fn next_sign_in(self) -> u64 {
        let next = *self.sync.attempt.peek() + 1;
        put(self.sync.attempt, next);
        next
    }

    /// Give up every sign-in asked for so far ([`SyncCtx::given_up`]).
    fn give_up_sign_ins(self) {
        put(self.sync.given_up, *self.sync.attempt.peek());
    }

    /// Stop turning sync on: the sign-in waiting, or the passphrase asked
    /// for. A sign-in finishing as it is cancelled goes no further.
    pub fn sync_cancel(self) {
        self.give_up_sign_ins();
        if let Some(link) = self.sync_link() {
            link.auth.cancel();
            if self.sync.state.peek().is_none() {
                link.auth.forget();
            }
        }
        put(self.sync.found, None);
        if matches!(
            *self.sync.status.peek(),
            SyncStatus::SigningIn | SyncStatus::Passphrase { .. }
        ) {
            self.set_sync_status(SyncStatus::Off { problem: None });
        }
    }

    /// Sync now: or, where a sync is under way, once more after it. None
    /// starts while the copy is being deleted.
    pub fn sync_now(self) {
        if self.sync.state.peek().is_none() || *self.sync.status.peek() == SyncStatus::TurningOff {
            return;
        }
        if *self.sync.running.peek() {
            put(self.sync.again, true);
            return;
        }
        let Some(link) = self.sync_link() else {
            return;
        };
        put(self.sync.running, true);
        self.change_on_status(|_, syncing, _, _| *syncing = true);
        spawn_forever(async move {
            // One sync at a time of every copy of the app on the store: one
            // under way in another tab or window is waited for, showing
            // "Syncing..." meanwhile, ten minutes at most.
            let store = self.app().store;
            let held =
                lock_within(&*store, Lock::SyncRun, RUN_LOCK_POLL_MS, RUN_LOCK_WAIT_MS).await;
            self.sync_through(&link).await;
            // What it took in is its own no longer: handed back, or kept.
            put(self.sync.taken_in, BTreeMap::new());
            put(self.sync.hand_back, false);
            // Let go of before the next asked for meanwhile, which takes it.
            drop(held);
            self.sync_over();
        });
    }

    /// Sync through `link`, as [`Self::sync_now`] does, once it holds the
    /// lock on syncing, or has waited for it long enough.
    async fn sync_through(self, link: &Link) {
        // None starts while the copy is being deleted, as none asked for
        // then does: one that waited for another copy's sync meanwhile
        // gives way to the deletion.
        if *self.sync.status.peek() == SyncStatus::TurningOff {
            return;
        }
        // Begun from what the store holds: what another copy of the app
        // kept since, laid under what this one holds; unless sync was
        // turned off before it began, here or in another copy.
        if !self.take_up_kept() {
            return;
        }
        let epoch = *self.sync.epoch.peek();
        let Some(mut state) = self.sync.state.peek().clone() else {
            return;
        };
        // Put back by hand before it, each stamped after what this copy
        // knows of the drive: those put back from here on wait for its end.
        put(self.sync.taking_in, true);
        for (id, at) in take(self.sync.revived) {
            let at = state.stamp_change(id, at);
            note(&mut state.revived, id, at);
        }
        put(self.sync.taken_in, state.revived.clone());
        put(self.sync.put_back, BTreeSet::new());
        // What it began from, or last kept as it went: what it does is laid
        // over the store as against it. The sign-in, which no sync changes,
        // as against the store's: one made here that the store has yet to
        // take, as it would not take it at once, is this copy's change still.
        let stored_sign_in = self
            .sync
            .base
            .peek()
            .as_ref()
            .and_then(|stored| stored.sign_in.clone());
        let base = Rc::new(RefCell::new(SyncState {
            sign_in: stored_sign_in,
            ..state.clone()
        }));
        // The settings here are taken for changed where they differ from
        // those as of the last sync, which may be another copy's, taken up
        // just now: those another copy stored since are taken up here first,
        // so that what this copy has yet to see is no change made here.
        self.take_up_stored_settings();
        let here = self.settings.peek().clone().clamp();
        let store = self.app().store;
        let remote = WhileCurrent {
            remote: &*link.remote,
            current: move || *self.sync.epoch.peek() == epoch,
        };
        // Put back by hand since it took the notes in, here or in another
        // copy: none let go.
        let began = state.clone();
        let since = || self.put_back_since(&began);
        // Kept as it sends deletions by hand, should the app stop.
        let keep = |sending: &SyncState| -> Pending<()> {
            let (sending, base) = (sending.clone(), Rc::clone(&base));
            Box::pin(async move { self.keep_sync_under_way(epoch, &sending, &base).await })
        };
        let synced =
            sync_once_with(&*store, &remote, &mut state, &here, &now_ms, &since, &keep).await;
        if *self.sync.epoch.peek() != epoch {
            // Sync was turned off meanwhile, here or in another copy, or
            // turned on anew: what this sync stored is taken up, and none of
            // what it would keep is kept. What was put back by hand since is
            // sync's as it is now, and is kept with it, where it is on. One
            // stopped as it asked the drive for more may have stored what it
            // took before then. Taken for off, or on anew, in another copy:
            // what it took in and did not send goes as the notes kept then
            // go ([`Notes::Kept`]).
            match &synced {
                Ok(synced) => self.take_up_synced(synced),
                Err(_) => self.take_up_store(),
            }
            put(self.sync.taking_in, false);
            if take(self.sync.hand_back) {
                self.note_waiting(&state.revived);
            }
            if !self.sync.revived.peek().is_empty() {
                self.keep_sync_state();
            }
            return;
        }
        // What it stored before it failed, if it did, is in the store: taken
        // up, as the store counts its own change as seen.
        if synced.is_err() {
            self.take_up_store();
        }
        let base = base.borrow().clone();
        match synced {
            Err(failed) if failed.ends_sync() => {
                self.sync_forget();
                self.set_sync_status(SyncStatus::Off {
                    problem: Some(failed.told()),
                });
            }
            Err(failed) => {
                self.keep_synced(epoch, &base, state).await;
                let signed_out = failed == SyncFailed::Remote(RemoteError::SignIn);
                self.change_on_status(|_, syncing, problem, needs| {
                    *syncing = false;
                    *needs = signed_out;
                    *problem = (!signed_out).then(|| failed.told());
                });
            }
            Ok(synced) => {
                self.keep_synced(epoch, &base, state).await;
                self.take_up_synced(&synced);
                // Turned off, or on anew, in another copy meanwhile: off here
                // as well, its settings left as they are.
                let Some(last) = self.sync.state.peek().as_ref().map(|state| state.last) else {
                    return;
                };
                let unreadable = passed_over(&synced);
                // What the sync took is laid over the settings as they are
                // now, not as they were as it began: one changed here
                // meanwhile stays, and goes with the next sync. So does one
                // another copy stored meanwhile, taken up first: laid over
                // what this copy held, which knew nothing of it, and stored,
                // what the sync took would undo it.
                self.take_up_stored_settings();
                let now = self.settings.peek().clone().clamp();
                let (kept, synced_as) = match synced.settings {
                    Some(taken) => (merge_settings(&here, &now, &taken), taken),
                    None => (now.clone(), here),
                };
                if kept != now {
                    let mut shown = self.settings;
                    shown.set(kept.clone());
                    self.store_settings();
                }
                if kept != synced_as {
                    self.sync_soon();
                }
                self.change_on_status(|shown_last, syncing, problem, needs| {
                    *shown_last = last;
                    *syncing = false;
                    *problem = unreadable;
                    *needs = false;
                });
            }
        }
    }

    /// Take up what a sync stored here: the history it changed, and the
    /// theme it took, for the app's shell to show.
    fn take_up_synced(self, synced: &Synced) {
        if synced.changed_history() {
            self.take_up_store();
        }
        if let Some(theme) = &synced.theme {
            let mut shown = self.sync.theme;
            shown.set(Some(theme.clone()));
        }
    }

    /// A sync is over: another starts, where one was asked for meanwhile.
    fn sync_over(self) {
        put(self.sync.running, false);
        if take(self.sync.again) {
            self.sync_now();
        }
    }

    /// Sync a moment from now, as the app does after a change here: once,
    /// however many changes ask in the meantime, and once any session under
    /// way is over.
    pub fn sync_soon(self) {
        if !self.syncs_unasked() || *self.sync.soon.peek() {
            return;
        }
        put(self.sync.soon, true);
        spawn_forever(async move {
            sleep_ms(SYNC_SOON_MS).await;
            while self.session_running() {
                sleep_ms(SESSION_WAIT_MS).await;
            }
            put(self.sync.soon, false);
            // Unless sync was turned off meanwhile, or is being.
            if self.syncs_unasked() {
                self.sync_now();
            }
        });
    }

    /// Sync is on, and signed in: the app syncs by itself.
    fn syncs_unasked(self) -> bool {
        matches!(
            *self.sync.status.peek(),
            SyncStatus::On {
                needs_sign_in: false,
                ..
            }
        )
    }

    /// Sync soon, where the last sync was a while ago: as the app comes
    /// back into sight. One stamped ahead of the clock, as it is once the
    /// clock is set back after it, is taken for one a while ago. What
    /// another copy of the app kept meanwhile is taken up first, a sync it
    /// made, or sync turned off there, among it ([`Self::take_up_kept`]).
    pub fn sync_if_stale(self) {
        if !self.take_up_kept() {
            return;
        }
        let last = self.sync.state.peek().as_ref().and_then(|state| state.last);
        let now = now_ms();
        if last.is_none_or(|last| last > now || now - last >= SYNC_STALE_MS) {
            self.sync_soon();
        }
    }

    /// The settings were just stored as `stored`: synced soon, unless they
    /// are what the last sync left them as.
    pub fn sync_settings_stored(self, stored: &cw_core::TrainingSettings) {
        let synced = self.sync.state.peek().as_ref().is_some_and(|state| {
            state
                .settings
                .as_ref()
                .is_some_and(|keys| settings_keys(keys) == as_keys(stored))
        });
        if !synced {
            self.sync_soon();
        }
    }

    /// The sessions `ids` were put back by hand, after a deletion, by Undo
    /// or a restore: the drive is told, so that a deletion it already holds
    /// does not take them away again. Each is put back after the deletion
    /// of it this device knows of as it is kept ([`SyncState::stamp_change`]),
    /// at once, with what another copy of the app kept meanwhile; while a
    /// sync is under way, as it ends, after what that sync read and sent as
    /// well. A sync under way lets none of them go, though sync is off by
    /// then, or is turned off before that sync goes on
    /// ([`SyncCtx::put_back`]).
    pub fn sync_revived(self, ids: impl IntoIterator<Item = RecordId>) {
        let ids: Vec<RecordId> = ids.into_iter().collect();
        if *self.sync.running.peek() {
            let mut put_back = self.sync.put_back;
            put_back.write().extend(ids.iter().copied());
        }
        if self.sync.state.peek().is_none() {
            return;
        }
        let now = now_ms();
        {
            let mut noted = self.sync.revived;
            let mut noted = noted.write();
            for id in ids {
                note(&mut noted, id, now);
            }
        }
        if !*self.sync.taking_in.peek() {
            self.keep_sync_state();
        }
        self.sync_soon();
    }

    /// The sessions put back by hand since the sync under way took in the
    /// notes made before it, `began`: here, whatever became of sync since,
    /// and in another copy of the app on the same store, a tab or a window,
    /// as the note of each it kept there for this sync shows, one this sync
    /// did not begin with, or at a later stamp. None of them is that sync's
    /// to let go, though the drive holds a deletion of it and what the sync
    /// plans from has no note of its putting back, nor to take back; and
    /// one deleted by hand again is that sync's to send as deleted by hand,
    /// whatever deletion of it the sync noted before
    /// ([`crate::sync::run::sync_once_with`]).
    fn put_back_since(self, began: &SyncState) -> BTreeSet<RecordId> {
        let mut since = self.sync.put_back.peek().clone();
        if let Kept::State(kept) = read_kept(&*self.app().store)
            && same_sync(&kept, began)
        {
            since.extend(
                kept.revived
                    .iter()
                    .filter(|&(id, &at)| began.revived.get(id).is_none_or(|&was| at > was))
                    .map(|(&id, _)| id),
            );
        }
        since
    }

    /// Turn sync off here: forget the key and the sign-in. With `and_copy`,
    /// delete the synced copy from the drive first, which turns sync off on
    /// every device as each next syncs.
    pub fn sync_turn_off(self, and_copy: bool) {
        let Some(link) = self.sync_link() else {
            return;
        };
        if !and_copy {
            // Turned off, or on anew, in another copy of the app since: off
            // here as well, saying so, the store left as that copy left it.
            match self.changed_elsewhere(&read_kept(&*self.app().store)) {
                Some(why) => {
                    self.forget_sync_here(Notes::Forgotten);
                    self.set_sync_status(SyncStatus::Off {
                        problem: Some(why.into()),
                    });
                }
                None => {
                    self.sync_forget();
                    self.set_sync_status(SyncStatus::Off { problem: None });
                }
            }
            return;
        }
        self.set_sync_status(SyncStatus::TurningOff);
        spawn_forever(async move {
            while *self.sync.running.peek() {
                sleep_ms(POLL_MS).await;
            }
            // Another copy's sync is waited for as well, holding the lock on
            // syncing until sync is off here: one begun after finds it off.
            let store = self.app().store;
            let _held =
                lock_within(&*store, Lock::SyncRun, RUN_LOCK_POLL_MS, RUN_LOCK_WAIT_MS).await;
            match delete_copy(&*link.remote).await {
                Ok(_) => {
                    self.sync_forget();
                    self.set_sync_status(SyncStatus::Off { problem: None });
                    self.toasts.show(
                        "Sync is off, and the synced copy is deleted from your Google Drive.",
                    );
                }
                // Said of sync as it stands now: a sync under way as the
                // deletion was asked for has ended since. None asked for
                // meanwhile started, so one starts soon, where sync is on.
                Err(err) => {
                    let problem = Some(format!("The synced copy was not deleted: {}.", err.why()));
                    let last = self.sync.state.peek().as_ref().map(|state| state.last);
                    self.set_sync_status(match last {
                        Some(last) => SyncStatus::On {
                            last,
                            syncing: *self.sync.running.peek(),
                            problem,
                            needs_sign_in: err == RemoteError::SignIn,
                        },
                        None => SyncStatus::Off { problem },
                    });
                    self.sync_soon();
                }
            }
        });
    }

    /// Forget everything sync keeps here, and what it keeps in the store,
    /// where the store may hold it still: this copy's sync, or what could
    /// not be read just now. Not where another copy of the app on the same
    /// store has turned sync off since, or on anew and kept its own there,
    /// which stays, whether this copy took that up already or not
    /// ([`Self::changed_elsewhere`]). Looked at and cleared under the lock
    /// on what sync keeps, as every keep is ([`Self::keep_sync_state`]).
    fn sync_forget(self) {
        let ours = self.sync.state.peek().clone();
        self.forget_sync_here(Notes::Forgotten);
        let Some(ours) = ours else {
            return;
        };
        let store = self.app().store;
        settle_or_spawn(
            Box::pin(async move {
                let _held = lock_state(&*store).await;
                let still_ours = match read_kept(&*store) {
                    Kept::Unread => true,
                    Kept::State(kept) => same_sync(&kept, &ours),
                    Kept::Cleared | Kept::Unknown => false,
                };
                if still_ours && let Err(why) = store.save_sync(None) {
                    self.toasts.show(format!(
                        "What sync kept could not be cleared: {}.",
                        why.trim_end_matches('.')
                    ));
                }
            }),
            |()| {},
        );
    }

    /// Forget everything sync keeps in this copy of the app, and nothing in
    /// the store, which other copies share: the sessions put back by hand
    /// and not yet sent among it, or not ([`Notes`]).
    fn forget_sync_here(self, notes: Notes) {
        if let Some(link) = self.sync_link() {
            link.auth.forget();
        }
        // A sign-in still under way is given up too: finishing after this,
        // it keeps nothing.
        self.give_up_sign_ins();
        self.next_sync_epoch();
        match notes {
            Notes::Forgotten => {
                put(self.sync.revived, BTreeMap::new());
                // Those a sync under way took in go too: it hands none back.
                put(self.sync.taken_in, BTreeMap::new());
                put(self.sync.hand_back, false);
            }
            Notes::Kept => {
                // Those a sync under way took in are its to hand back, as it
                // alone knows which it sent; one noted again since, at a
                // later stamp, is kept here.
                let taken_in = take(self.sync.taken_in);
                if !taken_in.is_empty() {
                    put(self.sync.hand_back, true);
                }
                let unsent: BTreeMap<RecordId, u64> = self
                    .sync
                    .state
                    .peek()
                    .iter()
                    .flat_map(|state| &state.revived)
                    .filter(|&(id, &at)| taken_in.get(id).is_none_or(|&was| at > was))
                    .map(|(&id, &at)| (id, at))
                    .collect();
                self.note_waiting(&unsent);
            }
        }
        put(self.sync.state, None);
        put(self.sync.base, None);
        put(self.sync.found, None);
        put(self.sync.taking_in, false);
    }

    /// Move on to the next turning on of sync: a sync under way keeps
    /// nothing of what it did ([`SyncCtx::epoch`]).
    fn next_sync_epoch(self) {
        let mut epoch = self.sync.epoch;
        *epoch.write() += 1;
    }

    /// Sync as the app opens, where it is on.
    pub fn sync_at_launch(self) {
        if self.sync.state.peek().is_some() {
            self.sync_soon();
        }
    }

    /// Sync through `link` in place of this build's: a test's.
    #[cfg(test)]
    pub fn link_sync(mut self, link: Link) -> Self {
        let store = self.app().store;
        self.sync = crate::state::in_root(|| SyncCtx::linked(&*store, Some(link)));
        crate::state::in_root(|| self.hear_sync_news());
        self
    }
}

/// What the card says of the files in the drive a sync `synced` passed
/// over, if any: those that would not open, damaged perhaps, and those a
/// later build wrote, which an update here reads.
fn passed_over(synced: &Synced) -> Option<String> {
    let files = |count: usize| crate::format::counted(count, "file");
    let were = |count: usize| if count == 1 { "was" } else { "were" };
    let damaged = (synced.unreadable > 0).then(|| {
        format!(
            "{} in your Google Drive could not be read, damaged perhaps, and {} passed over.",
            files(synced.unreadable),
            were(synced.unreadable)
        )
    });
    let later = (synced.later > 0).then(|| {
        format!(
            "{} in your Google Drive {} written by a newer Dust: update Dust here to read {}.",
            files(synced.later),
            were(synced.later),
            if synced.later == 1 { "it" } else { "them" }
        )
    });
    match (damaged, later) {
        (Some(damaged), Some(later)) => Some(format!("{damaged} {later}")),
        (damaged, later) => damaged.or(later),
    }
}

/// The folder as a sync sees it, `remote`, while `current` says sync is
/// still as it was as the sync began: once it is turned off, or on anew,
/// nothing more is asked of the folder. The sign-in may by then be another
/// account's, whose drive a sync begun before must not write into, sealed
/// with a key that account's sync does not go by. A request under way by
/// then goes no further than the sign-in it began with, as the drive sends
/// it ([`crate::sync::drive::Drive`]).
struct WhileCurrent<'a, F> {
    remote: &'a dyn Remote,
    current: F,
}

impl<F: Fn() -> bool> WhileCurrent<'_, F> {
    /// What `asked` of the folder comes to, while sync is as it was.
    fn ask<T: 'static>(
        &self,
        asked: impl FnOnce(&dyn Remote) -> Pending<Result<T, RemoteError>>,
    ) -> Pending<Result<T, RemoteError>> {
        if (self.current)() {
            asked(self.remote)
        } else {
            done(Err(RemoteError::Failed("sync was turned off".into())))
        }
    }
}

impl<F: Fn() -> bool> Remote for WhileCurrent<'_, F> {
    fn list(&self) -> Pending<Result<Vec<RemoteFile>, RemoteError>> {
        self.ask(|remote| remote.list())
    }

    fn get(&self, id: &str) -> Pending<Result<Vec<u8>, RemoteError>> {
        self.ask(|remote| remote.get(id))
    }

    fn create(&self, name: &str, bytes: Vec<u8>) -> Pending<Result<String, RemoteError>> {
        self.ask(|remote| remote.create(name, bytes))
    }

    fn update(&self, id: &str, bytes: Vec<u8>) -> Pending<Result<(), RemoteError>> {
        self.ask(|remote| remote.update(id, bytes))
    }

    fn delete(&self, id: &str) -> Pending<Result<(), RemoteError>> {
        self.ask(|remote| remote.delete(id))
    }
}

/// Set `at` to `value`.
fn put<T: 'static>(mut at: CopyValue<T>, value: T) {
    at.set(value);
}

/// What `at` holds, left as it stands by default.
fn take<T: Default + 'static>(mut at: CopyValue<T>) -> T {
    std::mem::take(&mut *at.write())
}

/// How hard a passphrase is stretched: as the app stretches it, but in a
/// test, which turns sync on by the dozen.
fn stretch() -> Stretch {
    #[cfg(test)]
    {
        Stretch::QUICK
    }
    #[cfg(not(test))]
    {
        Stretch::DEFAULT
    }
}

#[cfg(test)]
mod tests;
