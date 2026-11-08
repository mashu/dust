//! Which stored session is which: the key that tells one session of the
//! history from another ([`SessionKey`]), and what a store remembers of
//! one deleted by hand, beside its record ([`Deleted`]).

use cw_core::SessionResult;

/// What tells one session of the history from another: when it finished,
/// and on what day. Written as `[timestamp, "date"]`, as the move to
/// records lists the sessions it took ([`super::legacy::Layout`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SessionKey(u64, String);

impl SessionKey {
    pub fn of(session: &SessionResult) -> Self {
        Self(session.timestamp, session.date.clone())
    }

    /// The key of a session finished at `timestamp`, on `date`.
    pub fn new(timestamp: u64, date: String) -> Self {
        Self(timestamp, date)
    }
}

/// A session deleted by hand, as the store remembers it beside its record
/// (a tombstone): what tells a session another copy of the app deleted from
/// one lost in passing, which is stored again. Kept for good: a copy left
/// open on the history from before it is never too old to be told.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    pub timestamp: u64,
    pub date: String,
    /// When it was deleted, in milliseconds since the epoch.
    pub at: u64,
    /// What a later build added to it, carried along.
    #[serde(flatten)]
    pub unread: serde_json::Map<String, serde_json::Value>,
}

impl Deleted {
    /// `session`, deleted `at`.
    pub fn of(session: &SessionResult, at: u64) -> Self {
        Self {
            timestamp: session.timestamp,
            date: session.date.clone(),
            at,
            unread: serde_json::Map::new(),
        }
    }

    pub fn key(&self) -> SessionKey {
        SessionKey(self.timestamp, self.date.clone())
    }
}
