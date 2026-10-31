//! The history as the app holds it: every session, in the order it was
//! stored, each by its record ([`RecordId`]), and which of them the store
//! has yet to take or could not read.
//!
//! Sessions are found by their record, never by their place: a place moves
//! as the history changes under a screen, a record never does. So a session
//! deleted goes back exactly where it was, and one changed elsewhere is
//! told from one alike in when it finished.

use std::collections::{BTreeSet, HashMap};
use std::ops::Deref;
use std::task::{Context, Poll, Waker};

use cw_core::SessionResult;

use crate::persist::SessionKey;
use crate::persist::record::{HistoryLoad, Pending, Record, RecordId, records_for_blob};

/// The history, oldest first, each session beside its record.
///
/// `ids` runs parallel to `sessions` and in order, so the history is in the
/// order of its records and a record is found by a binary search. `by_key`
/// holds every record by the session's [`SessionKey`], for what is told
/// apart by when and on what day a session finished: a session cut off
/// that is kept already, or one deleted elsewhere.
#[derive(Clone, Debug, Default)]
pub struct StoredHistory {
    sessions: Vec<SessionResult>,
    ids: Vec<RecordId>,
    /// Records here the store has yet to say it took.
    unsaved: BTreeSet<RecordId>,
    /// Records the store holds that this build cannot read: there all the
    /// same, so none is taken for one missing.
    unread: BTreeSet<RecordId>,
    by_key: HashMap<SessionKey, Vec<RecordId>>,
    /// One more with each change, so a screen tells a changed history from
    /// the same one without comparing the two.
    generation: u64,
}

impl StoredHistory {
    /// The history a store read.
    pub fn from_load(load: HistoryLoad) -> Self {
        let mut history = Self {
            unread: load.unread.into_iter().collect(),
            ..Self::default()
        };
        for (id, session) in load.sessions {
            history.insert(id, session);
        }
        history
    }

    /// Put `session` in its place, under `id`, and say where. A record
    /// held already is held once: it is the same session.
    pub fn insert(&mut self, id: RecordId, session: SessionResult) -> usize {
        match self.ids.binary_search(&id) {
            Ok(at) => at,
            Err(at) => {
                self.by_key
                    .entry(SessionKey::of(&session))
                    .or_default()
                    .push(id);
                self.ids.insert(at, id);
                self.sessions.insert(at, session);
                self.generation += 1;
                at
            }
        }
    }

    /// Put `session` after every other, under the record it would be stored
    /// as, the way a session just finished here goes: how a test adds one.
    #[cfg(test)]
    pub fn push(&mut self, session: SessionResult) -> RecordId {
        let newest = self.newest_id();
        let id = Record::of(&session, |json| {
            RecordId::next(session.timestamp, newest, json)
        })
        .expect("a session writes")
        .id;
        self.insert(id, session);
        id
    }

    /// Take the record `id` out, and hand back its session, if it was here.
    pub fn remove(&mut self, id: RecordId) -> Option<SessionResult> {
        let at = self.index_of(id)?;
        self.ids.remove(at);
        let session = self.sessions.remove(at);
        let key = SessionKey::of(&session);
        if let Some(ids) = self.by_key.get_mut(&key) {
            ids.retain(|held| *held != id);
            if ids.is_empty() {
                self.by_key.remove(&key);
            }
        }
        self.unsaved.remove(&id);
        self.generation += 1;
        Some(session)
    }

    /// The record of the session at `index`.
    pub fn id_at(&self, index: usize) -> Option<RecordId> {
        self.ids.get(index).copied()
    }

    /// The session of the record `id`, if it is here.
    pub fn get(&self, id: RecordId) -> Option<&SessionResult> {
        self.index_of(id).map(|at| &self.sessions[at])
    }

    /// The record `id` as it is to be stored: its session, written as this
    /// build writes it, under the same name.
    pub fn record(&self, id: RecordId) -> Option<Record> {
        let json = serde_json::to_string(self.get(id)?).ok()?;
        Some(Record { id, json })
    }

    /// Where the record `id` is, if it is here.
    pub fn index_of(&self, id: RecordId) -> Option<usize> {
        self.ids.binary_search(&id).ok()
    }

    /// The newest record, after which the next session goes.
    pub fn newest_id(&self) -> Option<RecordId> {
        self.ids.last().copied()
    }

    /// Every record, in the order of the history.
    pub fn ids(&self) -> &[RecordId] {
        &self.ids
    }

    /// Whether a session finished when and on the day `key` says is here.
    pub fn has_key(&self, key: &SessionKey) -> bool {
        self.by_key.contains_key(key)
    }

    /// The records of every session finished when and on the day `key`
    /// says.
    pub fn ids_of(&self, key: &SessionKey) -> &[RecordId] {
        self.by_key.get(key).map_or(&[], Vec::as_slice)
    }

    /// The record `id` is here, and the store has yet to take it.
    pub fn mark_unsaved(&mut self, id: RecordId) {
        if self.index_of(id).is_some() {
            self.unsaved.insert(id);
        }
    }

    /// The store took the record `id`.
    pub fn mark_saved(&mut self, id: RecordId) {
        self.unsaved.remove(&id);
    }

    /// The records the store has yet to take.
    pub fn unsaved(&self) -> &BTreeSet<RecordId> {
        &self.unsaved
    }

    /// The sessions the store has yet to take, in the order of the history.
    pub fn unsaved_sessions(&self) -> impl Iterator<Item = &SessionResult> {
        self.unsaved.iter().filter_map(|id| self.get(*id))
    }

    /// The records the store holds that this build cannot read.
    pub fn unread(&self) -> &BTreeSet<RecordId> {
        &self.unread
    }

    /// The record `id` is stored, and this build cannot read it.
    pub fn mark_unread(&mut self, id: RecordId) {
        self.unread.insert(id);
    }

    /// How many changes the history has had.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Deref for StoredHistory {
    type Target = [SessionResult];

    fn deref(&self) -> &[SessionResult] {
        &self.sessions
    }
}

/// A history of `sessions`, oldest first, each under the record a move from
/// a history kept whole gives it: how a test sets one up.
impl From<Vec<SessionResult>> for StoredHistory {
    fn from(sessions: Vec<SessionResult>) -> Self {
        let mut history = Self::default();
        for (record, session) in records_for_blob(&sessions).into_iter().zip(sessions) {
            history.insert(record.id, session);
        }
        history
    }
}

/// Run `then` on what `pending` comes to: at once, where the store did it
/// then and there, as every store but a browser's database does; or once it
/// is done, in a task that outlives the screen that asked. So the app does
/// what follows a change in the same turn wherever it can, and never waits
/// on a store that has to.
pub fn settle_or_spawn<T: 'static>(mut pending: Pending<T>, then: impl FnOnce(T) + 'static) {
    let mut asked = Context::from_waker(Waker::noop());
    match pending.as_mut().poll(&mut asked) {
        Poll::Ready(value) => then(value),
        Poll::Pending => {
            dioxus::core::spawn_forever(async move { then(pending.await) });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use dioxus::prelude::*;

    use super::*;
    use crate::persist::record::{Record, done};
    use crate::persist::tests::session;

    /// A session finished at `timestamp`, on a day of its own.
    fn at(timestamp: u64) -> SessionResult {
        SessionResult {
            timestamp,
            ..session(&format!("2026-10-{:02}", timestamp % 28 + 1))
        }
    }

    /// Everything the history holds agrees: its records run parallel to
    /// its sessions and in order, and every record is found by its key.
    fn holds_together(history: &StoredHistory) {
        assert_eq!(history.ids.len(), history.sessions.len());
        assert!(history.ids.windows(2).all(|pair| pair[0] < pair[1]));
        let keyed: usize = history.by_key.values().map(Vec::len).sum();
        assert_eq!(keyed, history.ids.len());
        for (at, (id, session)) in history.ids.iter().zip(history.iter()).enumerate() {
            assert!(history.ids_of(&SessionKey::of(session)).contains(id));
            assert_eq!(history.index_of(*id), Some(at));
            assert_eq!(history.id_at(at), Some(*id));
        }
        assert!(
            history
                .unsaved
                .iter()
                .all(|id| history.index_of(*id).is_some())
        );
    }

    /// Sessions go in by their records and come out by them, whatever is
    /// done in between: a session removed and put back is where it was.
    #[test]
    fn the_history_keeps_its_records_beside_its_sessions() {
        let mut history = StoredHistory::from(vec![at(1), at(2), at(3), at(3)]);
        holds_together(&history);
        assert_eq!(history.len(), 4);
        let generation = history.generation();
        let second = history.id_at(1).expect("a record");
        let gone = history.remove(second).expect("held");
        holds_together(&history);
        assert!(history.generation() > generation);
        assert_eq!(history.remove(second), None, "gone once");
        assert_eq!(history.insert(second, gone.clone()), 1, "back where it was");
        holds_together(&history);
        assert_eq!(history[1], gone);
        // The same record twice is held once.
        let generation = history.generation();
        assert_eq!(history.insert(second, gone), 1);
        assert_eq!(history.len(), 4);
        assert_eq!(history.generation(), generation, "nothing changed");

        // Two sessions alike in their key are both found by it.
        let alike = history.ids_of(&SessionKey::of(&at(3)));
        assert_eq!(alike.len(), 2);
        assert!(history.has_key(&SessionKey::of(&at(1))));
        assert!(!history.has_key(&SessionKey::of(&at(9))));
        let third = history.id_at(2).expect("a record");
        history.remove(third);
        assert_eq!(history.ids_of(&SessionKey::of(&at(3))).len(), 1);
        history.remove(history.id_at(2).expect("a record"));
        assert!(!history.has_key(&SessionKey::of(&at(3))));
        holds_together(&history);
    }

    /// A new session goes after the newest; one the store has yet to take
    /// is marked so until it does, and while it is here.
    #[test]
    fn a_session_is_unsaved_until_the_store_takes_it() {
        let mut history = StoredHistory::from(vec![at(1), at(2)]);
        let newest = history.newest_id();
        let session = at(1);
        let record = Record::of(&session, |json| {
            RecordId::next(session.timestamp, newest, json)
        })
        .expect("a record");
        let at_end = history.insert(record.id, session);
        assert_eq!(at_end, 2, "after the newest, though its clock was behind");
        history.mark_unsaved(record.id);
        assert!(history.unsaved().contains(&record.id));
        holds_together(&history);
        history.mark_saved(record.id);
        assert!(history.unsaved().is_empty());
        history.mark_unsaved(record.id);
        history.remove(record.id);
        assert!(
            history.unsaved().is_empty(),
            "nothing unsaved that is not here"
        );
        history.mark_unsaved(record.id);
        assert!(history.unsaved().is_empty());
        holds_together(&history);
    }

    /// A history read from a store holds what it read, in order, and knows
    /// the records it could not read.
    #[test]
    fn a_history_read_holds_what_the_store_read() {
        let records = records_for_blob(&[at(1), at(2), at(3)]);
        let unread = RecordId { order: 1, nonce: 9 };
        let load = HistoryLoad {
            sessions: vec![
                (records[2].id, at(3)),
                (records[0].id, at(1)),
                (records[1].id, at(2)),
            ],
            unread: vec![unread],
            complete: true,
            ..HistoryLoad::default()
        };
        let mut history = StoredHistory::from_load(load);
        holds_together(&history);
        assert_eq!(
            history.iter().map(|s| s.timestamp).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        let ids: Vec<RecordId> = records.iter().map(|record| record.id).collect();
        assert_eq!(history.ids(), ids);
        assert!(history.unread().contains(&unread));
        assert_eq!(history.newest_id(), Some(records[2].id));
        // One found since, that does not read either, is known as such.
        let later = RecordId { order: 2, nonce: 9 };
        history.mark_unread(later);
        assert_eq!(history.unread().len(), 2);
        assert_eq!(history.len(), 3, "and is no session here");
    }

    /// What follows a change done then and there is done at once, with no
    /// runtime to wait in; one still going is done once it is.
    #[test]
    fn what_follows_a_change_done_at_once_is_done_at_once() {
        let got = Rc::new(Cell::new(None));
        let into = got.clone();
        settle_or_spawn(done(7), move |value| into.set(Some(value)));
        assert_eq!(got.get(), Some(7));

        // One done only on its second look, in the app's runtime.
        let mut app = VirtualDom::new(|| rsx! {});
        let got: Rc<RefCell<Vec<u32>>> = Rc::default();
        let looks = Rc::new(Cell::new(0));
        let (into, counted) = (got.clone(), looks.clone());
        app.in_scope(ScopeId::ROOT, move || {
            let later: Pending<u32> = Box::pin(std::future::poll_fn(move |_| {
                counted.set(counted.get() + 1);
                if counted.get() < 2 {
                    Poll::Pending
                } else {
                    Poll::Ready(9)
                }
            }));
            settle_or_spawn(later, move |value| into.borrow_mut().push(value));
        });
        assert!(got.borrow().is_empty(), "not done on the first look");
        app.process_events();
        assert_eq!(*got.borrow(), [9]);
        assert_eq!(looks.get(), 2);
    }
}
