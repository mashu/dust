//! Locks the copies of the app on one device share through their store: a
//! browser's tabs, each a copy, and the copies a computer runs, a window
//! each ([`Lock`]). A lock is held by one copy at a time, and a copy that
//! stops, however it stops, lets go of what it held. A lock is only ever
//! tried, never waited on ([`crate::persist::Store::try_lock`]): a copy that
//! finds one held tries again later, or goes on without it, as copies did
//! before there were locks.
//!
//! On a computer or a phone, each is a file beside what sync keeps, locked
//! as the system locks a file for whoever opened it ([`Lock::file_name`]).
//! In a browser it is a Web Lock of the site's ([`Lock::web_name`]), asked
//! for through a page script of its own (`src/persist/tabs.js`).

use std::any::Any;
use std::fmt;

/// A lock the copies of the app on one device share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lock {
    /// One sync at a time, of every copy on the store: held for the whole
    /// of a sync, and while the synced copy is deleted.
    SyncRun,
    /// What sync keeps ([`crate::sync::state::SyncState`]), read, laid over
    /// and written back by one copy at a time: held only across work that
    /// waits on nothing.
    SyncState,
}

impl Lock {
    /// The Web Lock's name, for a lock a browser takes. Like every name the
    /// app keeps or locks in a browser, it starts with `dust_`, as the site
    /// may share its origin with others (docs/platforms.md).
    ///
    /// None for what sync keeps: a tab reads and writes it then and there,
    /// in `localStorage`, while a Web Lock is granted only a moment after it
    /// is asked for, so a change kept under one would wait, and be lost with
    /// a tab closed in that moment. Nor would one help: a tab can read what
    /// another wrote late whatever lock it holds, as the browser hands each
    /// tab the other tabs' writes when it gets to it, which sync's merge
    /// allows for ([`crate::sync::state::SyncState::merged`]).
    pub fn web_name(self) -> Option<&'static str> {
        match self {
            Self::SyncRun => Some("dust_sync_run"),
            Self::SyncState => None,
        }
    }

    /// The lock file's name, in the folder that holds what sync keeps
    /// (`sync.json`): a file of its own, rather than that one locked, as
    /// on Windows a file locked cannot be read through another handle, and
    /// every copy reads what sync keeps.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::SyncRun => "sync-run.lock",
            Self::SyncState => "sync.lock",
        }
    }
}

/// What trying a lock came to.
#[derive(Debug)]
pub enum Locking {
    /// This copy holds it now, until what holds it is dropped.
    Held(LockHeld),
    /// Another copy holds it, or in a browser, waits for it.
    Busy,
    /// The store cannot lock it: a browser without Web Locks, a folder that
    /// takes no lock (on some network shares), or a store that keeps none.
    Unlockable,
}

/// A lock this copy holds, let go of as this is dropped.
pub struct LockHeld {
    _held: Box<dyn Any>,
}

impl LockHeld {
    /// The lock that `held` holds until it is dropped: a file kept open,
    /// say.
    pub fn new(held: impl Any) -> Self {
        Self {
            _held: Box::new(held),
        }
    }
}

impl fmt::Debug for LockHeld {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("LockHeld")
    }
}

/// What a tab does with the Web Locks every tab of the site shares: the
/// body of a function of `options`, run on the page.
#[cfg(any(feature = "web", test))]
pub(super) const TABS_JS: &str = include_str!("tabs.js");

#[cfg(test)]
mod page_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Each lock is a file of its own, and in a browser a Web Lock of its
    /// own, whose name starts with `dust_`, as every name the app keeps or
    /// locks there does; a browser takes no lock on what sync keeps.
    #[test]
    fn every_lock_has_a_name_of_its_own() {
        let locks = [Lock::SyncRun, Lock::SyncState];
        let files: std::collections::BTreeSet<&str> = locks.map(Lock::file_name).into();
        assert_eq!(files.len(), locks.len(), "{files:?}");
        assert!(
            files.iter().all(|name| name.ends_with(".lock")),
            "{files:?}"
        );
        assert_eq!(Lock::SyncRun.web_name(), Some("dust_sync_run"));
        assert_eq!(Lock::SyncState.web_name(), None);
    }
}
