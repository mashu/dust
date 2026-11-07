//! Sync in the app: what the card on the History tab shows of it
//! ([`SyncStatus`]), the moves the card makes (turning sync on, syncing now,
//! turning it off), and when the app syncs by itself: as it opens, a moment
//! after the history or the settings change here, and as it comes back
//! into sight, if it has not synced for a while. Never while a session or
//! a run is under way, which waits on nothing: once it is over.

use std::collections::BTreeMap;
use std::rc::Rc;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use super::auth::{Auth, AuthError};
use super::remote::{Remote, RemoteError, RemoteFile};
use super::run::{Found, SyncFailed, Synced, delete_copy, join, look, sync_once};
use super::seal::{MIN_PASSPHRASE, Stretch};
use super::settings::{as_keys, settings_keys};
use super::state::SyncState;
use super::text::random_hex;
use crate::persist::record::{Pending, RecordId, done};
use crate::persist::{Store, merge_settings};
use crate::state::AppCtx;
use crate::time::{POLL_MS, now_ms, sleep_ms};

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
        let auth = super::auth::default_auth()?;
        let drive = super::drive::Drive::new(super::http::default_http(), Rc::clone(&auth));
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
    /// What sync keeps, while it is on.
    state: CopyValue<Option<SyncState>>,
    /// The store holds what sync keeps as this copy of the app found it or
    /// last kept it there: gone from the store since, it was turned off by
    /// another copy on the same store, a tab or a window.
    stored: CopyValue<bool>,
    /// Which turning on of sync `state` is of: moved on as sync is turned
    /// off here, and as it is turned on, so that a sync under way across
    /// either keeps nothing of what it did.
    epoch: CopyValue<u64>,
    /// What the sign-in found in the drive, for the passphrase to open.
    found: CopyValue<Option<Found>>,
    /// Which asking for a sign-in is the latest: moved on as each is asked
    /// for, and as one is cancelled, so that one finishing after either
    /// goes no further.
    attempt: CopyValue<u64>,
    /// A sync is under way, and another is asked for after it.
    running: CopyValue<bool>,
    again: CopyValue<bool>,
    /// A sync is waiting to start ([`AppCtx::sync_soon`]).
    soon: CopyValue<bool>,
    /// Sessions put back by hand while a sync was under way: noted once it
    /// is over, or by the next, where that one was of a turning on of sync
    /// gone since.
    revived: CopyValue<BTreeMap<RecordId, u64>>,
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
            stored: CopyValue::new(state.is_some()),
            state: CopyValue::new(state),
            epoch: CopyValue::new(0),
            found: CopyValue::new(None),
            attempt: CopyValue::new(0),
            running: CopyValue::new(false),
            again: CopyValue::new(false),
            soon: CopyValue::new(false),
            revived: CopyValue::new(BTreeMap::new()),
        }
    }
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

    /// Keep what sync keeps, as it is now: told of where it cannot be.
    /// Where another copy of the app turned sync off since, nothing is
    /// kept, and sync is off here too ([`Self::take_up_sync_off`]).
    fn keep_sync_state(self) {
        if self.take_up_sync_off() {
            return;
        }
        let written = self.sync.state.peek().as_ref().and_then(SyncState::written);
        if let Some(written) = written {
            match self.app().store.save_sync(Some(&written)) {
                Ok(()) => put(self.sync.stored, true),
                Err(why) => self.toasts.show(format!(
                    "What sync keeps could not be saved: {}.",
                    why.trim_end_matches('.')
                )),
            }
        }
    }

    /// Turn sync off here, where another copy of the app on the same store,
    /// a tab or a window, has turned it off since this one found or last
    /// kept what sync keeps there; and say whether it was. Each copy keeps
    /// the whole of what sync keeps, so one keeping it again would turn
    /// sync back on in the store, for every copy.
    fn take_up_sync_off(self) -> bool {
        let off = *self.sync.stored.peek()
            && self.sync.state.peek().is_some()
            && self.app().store.sync_cleared();
        if off {
            self.sync_forget();
            self.set_sync_status(SyncStatus::Off { problem: None });
        }
        off
    }

    /// Ask the user to sign in to Google: to turn sync on, or to sync again
    /// once a browser's sign-in has run out. Called from the press of the
    /// card's button, as a browser lets only that open Google's window.
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
                    Err(err) => {
                        self.change_on_status(|_, _, problem, _| *problem = Some(err.told()))
                    }
                }
                return;
            }
            if let Err(err) = signed_in {
                let problem = (err != AuthError::Cancelled).then(|| err.told());
                self.set_sync_status(SyncStatus::Off { problem });
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

    /// Whether the sign-in `attempt` was cancelled, or another asked for,
    /// since it was asked for: then it goes no further. What it signed in
    /// as it finished is forgotten, as Cancel forgot what it had before,
    /// where sync is off and no other sign-in is under way.
    fn sign_in_given_up(self, attempt: u64, link: &Link) -> bool {
        if *self.sync.attempt.peek() == attempt {
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

    /// Stop turning sync on: the sign-in waiting, or the passphrase asked
    /// for. A sign-in finishing as it is cancelled goes no further.
    pub fn sync_cancel(self) {
        self.next_sign_in();
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
            self.take_up_sync_off();
            let epoch = *self.sync.epoch.peek();
            // Turned off before it began, here or in another copy.
            let Some(mut state) = self.sync.state.peek().clone() else {
                self.sync_over();
                return;
            };
            state.revived.extend(take(self.sync.revived));
            let here = self.settings.peek().clone().clamp();
            let store = self.app().store;
            let remote = WhileCurrent {
                remote: &*link.remote,
                current: move || *self.sync.epoch.peek() == epoch,
            };
            let synced = sync_once(&*store, &remote, &mut state, &here, now_ms()).await;
            self.take_up_sync_off();
            if *self.sync.epoch.peek() != epoch {
                // Sync was turned off meanwhile, here or in another copy,
                // or turned on anew: what this sync stored is taken up,
                // and none of what it would keep is kept. What was put
                // back by hand since is sync's as it is now, and waits for
                // its next sync. One stopped as it asked the drive for
                // more may have stored what it took before then.
                match &synced {
                    Ok(synced) => self.take_up_synced(synced),
                    Err(_) => self.take_up_store(),
                }
                self.sync_over();
                return;
            }
            // What it stored before it failed, if it did, is in the store:
            // taken up, as the store counts its own change as seen.
            if synced.is_err() {
                self.take_up_store();
            }
            // Put back by hand meanwhile: noted with the rest, each stamped
            // again after what this sync read of it and sent, the deletion
            // by hand it put back among them.
            for (id, at) in take(self.sync.revived) {
                let at = state.stamp_change(id, at);
                state.revived.insert(id, at);
            }
            // The sign-in is no sync's to keep: one made meanwhile stays.
            state.sign_in = self
                .sync
                .state
                .peek()
                .as_ref()
                .and_then(|kept| kept.sign_in.clone());
            match synced {
                Err(failed) if failed.ends_sync() => {
                    self.sync_forget();
                    self.set_sync_status(SyncStatus::Off {
                        problem: Some(failed.told()),
                    });
                }
                Err(failed) => {
                    put(self.sync.state, Some(state));
                    self.keep_sync_state();
                    let signed_out = failed == SyncFailed::Remote(RemoteError::SignIn);
                    self.change_on_status(|_, syncing, problem, needs| {
                        *syncing = false;
                        *needs = signed_out;
                        *problem = (!signed_out).then(|| failed.told());
                    });
                }
                Ok(synced) => {
                    let last = state.last;
                    put(self.sync.state, Some(state));
                    self.keep_sync_state();
                    self.take_up_synced(&synced);
                    let unreadable = passed_over(&synced);
                    // What the sync took is laid over the settings as they
                    // are now, not as they were as it began: one changed
                    // here meanwhile stays, and goes with the next sync.
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
            self.sync_over();
        });
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
    /// clock is set back after it, is taken for one a while ago.
    pub fn sync_if_stale(self) {
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
    /// of it this device knows of ([`SyncState::stamp_change`]); while a
    /// sync is under way, after what that sync read and sent as well, as
    /// it ends.
    pub fn sync_revived(self, ids: impl IntoIterator<Item = RecordId>) {
        let now = now_ms();
        let revived: Vec<(RecordId, u64)> = match self.sync.state.peek().as_ref() {
            Some(state) => ids
                .into_iter()
                .map(|id| (id, state.stamp_change(id, now)))
                .collect(),
            None => return,
        };
        if *self.sync.running.peek() {
            let mut noted = self.sync.revived;
            noted.write().extend(revived);
        } else {
            let mut state = self.sync.state;
            if let Some(state) = state.write().as_mut() {
                state.revived.extend(revived);
            }
            self.keep_sync_state();
        }
        self.sync_soon();
    }

    /// Turn sync off here: forget the key and the sign-in. With `and_copy`,
    /// delete the synced copy from the drive first, which turns sync off on
    /// every device as each next syncs.
    pub fn sync_turn_off(self, and_copy: bool) {
        let Some(link) = self.sync_link() else {
            return;
        };
        if !and_copy {
            self.sync_forget();
            self.set_sync_status(SyncStatus::Off { problem: None });
            return;
        }
        self.set_sync_status(SyncStatus::TurningOff);
        spawn_forever(async move {
            while *self.sync.running.peek() {
                sleep_ms(POLL_MS).await;
            }
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

    /// Forget everything sync keeps here.
    fn sync_forget(self) {
        if let Some(link) = self.sync_link() {
            link.auth.forget();
        }
        // A sign-in still under way is given up too: finishing after this,
        // it keeps nothing.
        self.next_sign_in();
        self.next_sync_epoch();
        put(self.sync.state, None);
        put(self.sync.stored, false);
        put(self.sync.found, None);
        put(self.sync.revived, BTreeMap::new());
        if let Err(why) = self.app().store.save_sync(None) {
            self.toasts.show(format!(
                "What sync kept could not be cleared: {}.",
                why.trim_end_matches('.')
            ));
        }
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
        self
    }
}

/// What the card says of the files in the drive a sync `synced` passed
/// over, if any: those that would not open, damaged perhaps, and those a
/// later build wrote, which an update here reads.
fn passed_over(synced: &Synced) -> Option<String> {
    let files = |count: usize| crate::ui::format::counted(count, "file");
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
/// with a key that account's sync does not go by.
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
