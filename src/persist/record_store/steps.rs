//! A change to the history made step by step, as every store but a
//! browser's makes it ([`make_steps`]): deletions undone first, then the
//! records, then the deletions, each step made durable before the next,
//! where the store makes its writes durable at all. A change cut short so
//! never loses a session, though it may leave one deleted that was being
//! put back, or one beside what was filling it in. A browser's database
//! makes the same steps in one transaction, in its page script, which a
//! test holds to these.

use std::collections::HashSet;

use super::super::record::{Record, RecordId};

/// How many names past its own a record is tried under, where other bytes
/// have its name already ([`RecordId::bumped`]): its own and these, as the
/// page script tries them.
pub(crate) const BUMPS: u64 = 64;

/// What a store holds under a record's name, as a record is to be stored
/// there ([`place`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum There {
    /// Nothing.
    Free,
    /// The very bytes to store.
    Same,
    /// Other bytes.
    Other,
}

/// What a store does at each step of a change ([`make_steps`]).
pub(crate) trait Steps {
    /// Why a step could not be made.
    type Error;
    /// Whether record `id` has a tombstone.
    fn tombstoned(&self, id: RecordId) -> bool;
    /// Whether a record is stored under `id`, whatever it holds.
    fn holds(&self, id: RecordId) -> bool;
    /// What is stored under `id`, beside `json`, the bytes to store there.
    fn there(&self, id: RecordId, json: &str) -> Result<There, Self::Error>;
    /// Store `json` as the record `id`, where nothing is stored.
    fn add(&self, id: RecordId, json: &str) -> Result<(), Self::Error>;
    /// Why the record stored as `id` could not be stored: every name it
    /// could go under is held by other bytes.
    fn no_name_free(&self, id: RecordId) -> Self::Error;
    /// Undo the deletion of each of `ids`, its tombstone taken away, and
    /// make that durable.
    fn unforget(&self, ids: &[RecordId]) -> Result<(), Self::Error>;
    /// Make the records stored durable.
    fn records_durable(&self) -> Result<(), Self::Error>;
    /// Store each of `new`, a tombstone not there yet, with what it holds,
    /// and make them durable: all of them, or, where one cannot be, none.
    fn tombstone(&self, new: &[(RecordId, &str)]) -> Result<(), Self::Error>;
    /// Take away each of `ids`, a record its tombstone deletes now, as far
    /// as it goes: one that will not go stays beside its tombstone, which
    /// deletes it all the same, until the history is next read.
    fn unlink(&self, ids: &[RecordId]);
}

/// Make a change with `steps`: `unforget`, the deletions undone, first;
/// then `put`, each stored where it is not stored already, one deleted
/// already left deleted unless this change undoes that; then `forget`, the
/// deletions, each with what its tombstone holds, the first of each record
/// where one is named twice. A record is never written over: one put back
/// whose record was never taken away, as a deletion whose record would not
/// go leaves it, is back as it was stored, by its own name; any other goes
/// under its own name or one past it ([`place`]). The tombstones are the
/// deletion: none of them stays unless all of them do, and a record they
/// delete that will not go is made no less deleted by it.
///
/// Where each of `put` went, in their order, none for one left deleted, as
/// far as the change got; and whether it was made.
pub(crate) fn make_steps<S: Steps>(
    steps: &S,
    put: &[Record],
    forget: &[(RecordId, &str)],
    unforget: &[RecordId],
) -> (Vec<Option<RecordId>>, Result<(), S::Error>) {
    let mut placed = Vec::with_capacity(put.len());
    if let Err(err) = steps.unforget(unforget) {
        return (placed, Err(err));
    }
    let undone: HashSet<RecordId> = unforget.iter().copied().collect();
    for record in put {
        let undoing = undone.contains(&record.id);
        // One deleted already stays deleted, unless this change undoes
        // that.
        if !undoing && steps.tombstoned(record.id) {
            placed.push(None);
            continue;
        }
        let id = if undoing && steps.holds(record.id) {
            record.id
        } else {
            match place(steps, record) {
                Ok(id) => id,
                Err(err) => return (placed, Err(err)),
            }
        };
        placed.push(Some(id));
    }
    if !put.is_empty()
        && let Err(err) = steps.records_durable()
    {
        return (placed, Err(err));
    }
    if forget.is_empty() {
        return (placed, Ok(()));
    }
    // One there already may hold what a later build added to it.
    let mut named = HashSet::new();
    let new: Vec<(RecordId, &str)> = forget
        .iter()
        .filter(|(id, _)| named.insert(*id) && !steps.tombstoned(*id))
        .copied()
        .collect();
    if let Err(err) = steps.tombstone(&new) {
        return (placed, Err(err));
    }
    let gone: Vec<RecordId> = forget.iter().map(|(id, _)| *id).collect();
    steps.unlink(&gone);
    (placed, Ok(()))
}

/// Store `record` under its name, or, where other bytes have that name
/// already, the next free one of the [`BUMPS`] past it: no record is ever
/// written over. The name it is stored under: one holding the very bytes
/// is the record stored already.
pub(crate) fn place<S: Steps>(steps: &S, record: &Record) -> Result<RecordId, S::Error> {
    let mut id = record.id;
    for _ in 0..=BUMPS {
        match steps.there(id, &record.json)? {
            There::Same => return Ok(id),
            There::Other => id = id.bumped(),
            There::Free => {
                steps.add(id, &record.json)?;
                return Ok(id);
            }
        }
    }
    Err(steps.no_name_free(record.id))
}
