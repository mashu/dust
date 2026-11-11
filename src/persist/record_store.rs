//! The history's records as every store reads them, whatever keeps them:
//! the records read told apart, as sessions, records this build cannot read
//! and records that could not be read just then ([`sort`]); and what the
//! tombstones say, of which records were deleted by hand, when, and which
//! sessions they held ([`keys_of_tombstones`], [`deletions_of`],
//! [`deleted_sessions_of`]). Each store reads them its own way: the files
//! ([`super::files`]), a browser's database and the test store.

use std::collections::{HashMap, HashSet};

use cw_core::SessionResult;

use super::record::{
    Deletions, Record, RecordId, Unopened, Unreadable, keyed, tombstone_at, tombstone_key,
};
use super::session::SessionKey;

pub(super) mod notes;
#[cfg(any(feature = "native-runtime", test))]
pub(super) mod steps;

/// What is stored of a record, as a store reads it.
#[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
pub(super) enum Raw {
    /// Its text.
    Text(String),
    /// So many bytes that are no text, as damage to a disk leaves them: no
    /// build reads them, now or later.
    NoText(u64),
    /// Nothing, for now: it could not be read just then.
    Failed,
}

#[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
impl Raw {
    /// What is stored, where it is text.
    pub(super) fn text(self) -> Option<String> {
        match self {
            Self::Text(raw) => Some(raw),
            Self::NoText(_) | Self::Failed => None,
        }
    }

    pub(super) fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// What a read of record `id` hands back of what is stored
    /// ([`super::Store::read_records`]): bytes that are no text are a
    /// damaged record's, as the history read whole takes such a record.
    pub(super) fn record(self, id: RecordId) -> Result<Record, Unopened> {
        match self {
            Self::Text(json) => Ok(Record { id, json }),
            Self::NoText(_) => Err(Unopened::Damaged(id)),
            Self::Failed => Err(Unopened::Failed(id)),
        }
    }
}

/// A record as a store read it: the session it holds, or, where it holds
/// none this build reads, what it says of itself ([`Unreadable`]). None,
/// where it could not be read at all just then.
pub(super) type RecordRead = Option<Result<SessionResult, Unreadable>>;

/// The key of each record read so far, by its id: what an older build's
/// deletions are told apart by, without reading every record again each
/// time what it changed is taken up.
pub(super) type KeyCache = HashMap<RecordId, SessionKey>;

/// Records read, told apart ([`sort`]), each kind in the order read.
#[derive(Default)]
pub(super) struct Sorted {
    /// Every session that reads, by its record.
    pub(super) sessions: Vec<(RecordId, SessionResult)>,
    /// Every record that holds no session this build reads, with what it
    /// says of itself.
    pub(super) unread: Vec<(RecordId, Unreadable)>,
    /// Of those, the ones a later build stored ([`Unreadable::later`]),
    /// each beside the session it says it holds.
    pub(super) later: Vec<(RecordId, SessionKey)>,
    /// Those that could not be read at all just then.
    pub(super) failed: Vec<RecordId>,
}

/// `read`, each record as a store read it, told apart, the key of each that
/// holds a session noted in `keys`: one that reads holds its own, and one a
/// later build stored holds the one it says it holds, so an older build's
/// copy of that session is no session to take up beside it. A damaged one
/// holds none, though it may say one ([`Unreadable::later_key`]).
pub(super) fn sort(
    read: impl IntoIterator<Item = (RecordId, RecordRead)>,
    keys: &mut KeyCache,
) -> Sorted {
    let mut sorted = Sorted::default();
    for (id, read) in read {
        match read {
            Some(Ok(session)) => {
                keys.insert(id, SessionKey::of(&session));
                sorted.sessions.push((id, session));
            }
            Some(Err(said)) => {
                if let Some(key) = said.later_key() {
                    keys.insert(id, key.clone());
                    sorted.later.push((id, key.clone()));
                }
                sorted.unread.push((id, said));
            }
            None => sorted.failed.push(id),
        }
    }
    sorted
}

/// Every record of `sessions`, and of `later`, those a later build stored,
/// by the session each holds: what an older build's copy of a session is
/// told apart from, as it is taken up ([`super::legacy::imported`]).
pub(super) fn live_of(
    sessions: &[(RecordId, SessionResult)],
    later: &[(RecordId, SessionKey)],
) -> HashMap<SessionKey, Vec<RecordId>> {
    let mut live = keyed(sessions.iter().map(|(id, session)| (*id, session)));
    for (id, key) in later {
        live.entry(key.clone()).or_default().push(*id);
    }
    live
}

/// A tombstone, as a store read it: the record it deletes, where its name
/// names one, and what it holds.
pub(super) struct Tombstone {
    #[cfg_attr(not(any(feature = "web", test)), allow(dead_code))]
    pub(super) id: Option<RecordId>,
    pub(super) read: TombstoneRead,
}

/// What a tombstone holds, as a store read it.
pub(super) enum TombstoneRead {
    /// What was written for it ([`super::Deleted`]): one that is empty, or
    /// does not read, deletes its record all the same, and names no
    /// session.
    Bytes(Vec<u8>),
    /// A value that is no text, as a later build may store in a browser's
    /// database: it names no session.
    #[cfg_attr(not(any(feature = "web", test)), allow(dead_code))]
    NoText,
    /// Nothing, for now, for the reason given: it is there, and could not
    /// be read just then, as a folder in its place cannot be, or a file
    /// held open. It may name a session to keep deleted.
    #[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
    Failed(String),
}

/// How a tombstone that could not be read just then is taken
/// ([`keys_of_tombstones`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reading {
    /// As one that may name a session to keep deleted: what waits on the
    /// sessions deleted here, a take-up of what an older build changed and
    /// a move made over records, waits for it to read.
    Strict,
    /// As one that names none, as any that does not read: which sessions
    /// a tombstone remembers is answered without it
    /// ([`super::Store::deleted_keys`]).
    Lenient,
}

/// The key of every session `tombstones` name. One that names none, as an
/// empty one, or one whose value is no text, names none; one that could
/// not be read just then names none where it is read [`Reading::Lenient`],
/// and holds the answer back where it is read [`Reading::Strict`]: why it
/// could not be read, of the first.
pub(super) fn keys_of_tombstones(
    tombstones: &[Tombstone],
    reading: Reading,
) -> Result<HashSet<SessionKey>, String> {
    let mut keys = HashSet::new();
    for tombstone in tombstones {
        match &tombstone.read {
            TombstoneRead::Bytes(raw) => keys.extend(tombstone_key(raw)),
            TombstoneRead::NoText => {}
            TombstoneRead::Failed(why) => {
                if reading == Reading::Strict {
                    return Err(why.clone());
                }
            }
        }
    }
    Ok(keys)
}

/// Which of the sessions `keys` are among `gone`, the sessions the
/// tombstones name, in the order of `keys` ([`super::Store::deleted_keys`]).
pub(super) fn deleted_keys_of(
    gone: &HashSet<SessionKey>,
    keys: impl IntoIterator<Item = SessionKey>,
) -> Vec<SessionKey> {
    keys.into_iter().filter(|key| gone.contains(key)).collect()
}

/// The records a look found deleted by hand, each beside what its
/// tombstone holds: none where it holds nothing that could be read, as one
/// there that could not be read just then, or whose value is no text.
pub(super) type TombstonesOf = Vec<(RecordId, Option<Vec<u8>>)>;

/// Of `found`, the records with a tombstone, when each says it was deleted:
/// none for one that does not say, as one that is empty, or does not read
/// ([`super::Store::deletions`]).
pub(super) fn deletions_of(found: TombstonesOf) -> Deletions {
    found
        .into_iter()
        .map(|(id, raw)| (id, raw.as_deref().and_then(tombstone_at)))
        .collect()
}

/// Of `found`, the records with a tombstone, each beside the session its
/// tombstone remembers: one whose tombstone does not say, as one that is
/// empty, or does not read, is left out ([`super::Store::deleted_sessions`]).
pub(super) fn deleted_sessions_of(found: TombstonesOf) -> Vec<(RecordId, SessionKey)> {
    found
        .into_iter()
        .filter_map(|(id, raw)| Some((id, tombstone_key(&raw?)?)))
        .collect()
}

#[cfg(test)]
mod tests;
