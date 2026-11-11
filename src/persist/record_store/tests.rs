//! The records and tombstones as every store reads them.

use std::cell::Cell;
use std::collections::VecDeque;

use super::*;
use crate::persist::record::read_record;
use crate::persist::record::unreadable;
use crate::persist::tests::session;

fn finished(at: u64) -> SessionResult {
    SessionResult {
        timestamp: at,
        ..session(&format!("day {at}"))
    }
}

fn id(order: u64) -> RecordId {
    RecordId { order, nonce: 1 }
}

fn written(deleted: &Deleted) -> Vec<u8> {
    serde_json::to_vec(deleted).unwrap()
}

/// Records read are told apart by what each holds, in the order read, and
/// the key of each that holds a session is noted: its own, or the one a
/// later build that stored it says it holds. A damaged record holds none,
/// though it says one, and one that could not be read is neither.
#[test]
fn records_read_are_told_apart_and_the_sessions_they_hold_noted() {
    let (first, second) = (finished(1_000), finished(2_000));
    let later = format!(
        r#"{{"format":{},"timestamp":3000,"date":"day 3000"}}"#,
        cw_core::SESSION_FORMAT + 1
    );
    let damaged = r#"{"timestamp":4000,"date":"day 4000"}"#;
    let read = [
        (id(1), Some(Ok(first.clone()))),
        (id(2), None),
        (id(3), Some(Err(unreadable(&later)))),
        (id(4), Some(Err(unreadable(damaged)))),
        (id(5), Some(Ok(second.clone()))),
    ];
    let mut keys = KeyCache::new();
    keys.insert(id(9), SessionKey::of(&first));
    let sorted = sort(read, &mut keys);
    assert_eq!(
        sorted.sessions,
        [(id(1), first.clone()), (id(5), second.clone())]
    );
    let unread: Vec<RecordId> = sorted.unread.iter().map(|(id, _)| *id).collect();
    assert_eq!(unread, [id(3), id(4)]);
    let said_later = SessionKey::new(3000, "day 3000".into());
    assert_eq!(sorted.later, [(id(3), said_later.clone())]);
    assert_eq!(sorted.failed, [id(2)]);
    let mut noted: Vec<(RecordId, SessionKey)> = keys.into_iter().collect();
    noted.sort_by_key(|(id, _)| *id);
    assert_eq!(
        noted,
        [
            (id(1), SessionKey::of(&first)),
            (id(3), said_later.clone()),
            (id(5), SessionKey::of(&second)),
            (id(9), SessionKey::of(&first)),
        ],
        "a damaged record's and a failed one's none"
    );
    let live = live_of(&sorted.sessions, &sorted.later);
    assert_eq!(live.len(), 3);
    assert_eq!(live[&said_later], [id(3)]);
    assert_eq!(live[&SessionKey::of(&first)], [id(1)]);
}

/// What the tombstones name: each that reads names its session; an empty
/// one, one that does not read and one whose value is no text name none.
/// One that could not be read just then holds back an answer that waits on
/// it, and is passed over by one that does not.
#[test]
fn the_tombstones_name_their_sessions_read_strictly_or_leniently() {
    let gone = Deleted::of(&finished(1_000), 7);
    let tombstone = |read| Tombstone {
        id: Some(id(1)),
        read,
    };
    let readable = [
        tombstone(TombstoneRead::Bytes(written(&gone))),
        tombstone(TombstoneRead::Bytes(b"{}".to_vec())),
        tombstone(TombstoneRead::Bytes(b"not a tombstone".to_vec())),
        tombstone(TombstoneRead::NoText),
    ];
    let named = HashSet::from([gone.key()]);
    for reading in [Reading::Strict, Reading::Lenient] {
        assert_eq!(keys_of_tombstones(&readable, reading), Ok(named.clone()));
    }
    let mut held = readable;
    held[1] = tombstone(TombstoneRead::Failed("held open".into()));
    assert_eq!(
        keys_of_tombstones(&held, Reading::Strict),
        Err("held open".into())
    );
    assert_eq!(keys_of_tombstones(&held, Reading::Lenient), Ok(named));
    assert_eq!(keys_of_tombstones(&[], Reading::Strict), Ok(HashSet::new()));
}

/// Of the sessions asked of, those the tombstones name, in the order asked.
#[test]
fn the_sessions_deleted_are_answered_in_the_order_asked() {
    let keys: Vec<SessionKey> = (1..=4).map(|n| SessionKey::of(&finished(n))).collect();
    let gone = HashSet::from([keys[3].clone(), keys[1].clone()]);
    assert_eq!(
        deleted_keys_of(&gone, keys.clone()),
        [keys[1].clone(), keys[3].clone()]
    );
    assert_eq!(deleted_keys_of(&HashSet::new(), keys), []);
}

/// Of the records found deleted, when each was, and which session each
/// held: a tombstone that does not say says neither, and is still a
/// deletion; one that does not say which session is left out of those.
#[test]
fn deletions_say_when_and_of_which_session_where_their_tombstones_do() {
    let gone = Deleted::of(&finished(1_000), 7);
    let found = || -> TombstonesOf {
        vec![
            (id(1), Some(written(&gone))),
            (id(2), Some(b"{}".to_vec())),
            (id(3), None),
            (id(4), Some(b"[".to_vec())),
        ]
    };
    assert_eq!(
        deletions_of(found()),
        [
            (id(1), Some(7)),
            (id(2), None),
            (id(3), None),
            (id(4), None)
        ]
    );
    assert_eq!(deleted_sessions_of(found()), [(id(1), gone.key())]);
    assert_eq!(deletions_of(Vec::new()), []);
}

/// The names a note holds, as it is written.
fn noted(names: &[RecordId]) -> Vec<u8> {
    let names: BTreeSet<String> = names.iter().map(|id| id.name()).collect();
    serde_json::to_vec(&names).unwrap()
}

fn names_of(ids: &[RecordId]) -> BTreeSet<String> {
    ids.iter().map(|id| id.name()).collect()
}

/// A record this build cannot read is told of once, by the note of what
/// was told until this build first writes its note of damage, and by that
/// note from then on, which is written the first time, though it names
/// none, where the note of what was told is there; the note of what was
/// told only gains names, and a failure is told of only where it failed at
/// the last read too, and is noted as failing for the next.
#[test]
fn what_is_told_at_a_read_goes_by_the_notes_kind_by_kind() {
    use notes::{Held, Note, at_open};
    let damaged = (id(1), Unreadable::default());
    let older = noted(&[id(1), id(7)]);
    // Told of by a build before formats: not again, and its note of what
    // it told keeps every name, with this build's note of damage written.
    let told = at_open(
        std::slice::from_ref(&damaged),
        &[],
        Held {
            told: Some(&older),
            ..Held::default()
        },
    );
    assert_eq!(told.notices, []);
    assert_eq!(told.noted, [(Note::DamagedTold, Some(names_of(&[id(1)])))]);
    // This build's note of damage written, it alone holds a notice back.
    let none = noted(&[]);
    let told = at_open(
        std::slice::from_ref(&damaged),
        &[],
        Held {
            told: Some(&older),
            damaged_told: Some(&none),
            ..Held::default()
        },
    );
    assert_eq!(told.notices, [HistoryNotice::Damaged { count: 1 }]);
    assert_eq!(told.noted, [(Note::DamagedTold, Some(names_of(&[id(1)])))]);
    // A failure is told of where it failed at the last read too, noted as
    // told of, and in the note of what was told, which only gains names.
    let failing = noted(&[id(2)]);
    let told = at_open(
        &[],
        &[id(2), id(3)],
        Held {
            told: Some(&older),
            failing: Some(&failing),
            damaged_told: Some(&none),
            ..Held::default()
        },
    );
    assert_eq!(told.notices, [HistoryNotice::Unread { count: 1 }]);
    assert_eq!(
        told.noted,
        [
            (Note::Told, Some(names_of(&[id(1), id(2), id(7)]))),
            (Note::UnreadTold, Some(names_of(&[id(2)]))),
            (Note::Failing, Some(names_of(&[id(2), id(3)]))),
        ]
    );
    // Nothing failing, nothing told: the note of failures goes.
    let told = at_open(
        &[],
        &[],
        Held {
            failing: Some(&failing),
            ..Held::default()
        },
    );
    assert_eq!(told.notices, []);
    assert_eq!(told.noted, [(Note::Failing, None)]);
}

/// What a read and a listing take out of the notes: a note left naming
/// none goes, but for the note of damage, which stays; one that names none
/// of them is left as it is.
#[test]
fn a_record_read_or_gone_is_taken_out_of_this_builds_notes() {
    use notes::{LISTED, Note, after_read, gone_from, no_more};
    let read = [
        Ok(Record {
            id: id(1),
            json: "{}".into(),
        }),
        Err(Unopened::Damaged(id(2))),
        Err(Unopened::Failed(id(3))),
    ];
    let [damage, failing, unread] = after_read(&read);
    assert_eq!(damage, (Note::DamagedTold, HashSet::from([id(1).name()])));
    let either = HashSet::from([id(1).name(), id(2).name()]);
    assert_eq!(failing, (Note::Failing, either.clone()));
    assert_eq!(unread, (Note::UnreadTold, either.clone()));
    let held = noted(&[id(1), id(2)]);
    assert_eq!(no_more(Note::Failing, &held, &either), Some(None));
    assert_eq!(
        no_more(Note::DamagedTold, &held, &either),
        Some(Some(BTreeSet::new()))
    );
    assert_eq!(
        no_more(Note::UnreadTold, &held, &HashSet::from([id(1).name()])),
        Some(Some(names_of(&[id(2)])))
    );
    assert_eq!(
        no_more(Note::Failing, &held, &HashSet::from([id(3).name()])),
        None
    );
    assert_eq!(
        gone_from(&held, &HashSet::from([id(2).name()])),
        HashSet::from([id(1).name()])
    );
    assert_eq!(LISTED, [Note::Failing, Note::UnreadTold, Note::DamagedTold]);
}

/// Records still unread after the app's tries are told of once, and noted
/// as told of, where a note gains a name by them.
#[test]
fn records_still_unread_are_told_of_once() {
    use notes::{noted_unread, still_unread};
    let told = noted(&[id(1)]);
    assert_eq!(still_unread(&[id(1)], Some(&told)), (Vec::new(), None));
    assert_eq!(
        still_unread(&[id(1), id(2)], Some(&told)),
        (
            vec![HistoryNotice::Unread { count: 1 }],
            Some(names_of(&[id(1), id(2)]))
        )
    );
    assert_eq!(noted_unread(&[id(1)], Some(&told)), None);
    assert_eq!(
        noted_unread(&[id(2)], Some(b"not a note")),
        Some(names_of(&[id(2)]))
    );
}

// The order every store's reads and writes are made in, over a store that
// answers from a script.

/// Why the scripted store refused: in its words, raced or not.
#[derive(Clone, Debug, PartialEq)]
struct Said {
    words: &'static str,
    raced: bool,
}

impl Refusal for Said {
    fn saved(&self) -> HistorySaved {
        HistorySaved::Failed(self.words.into())
    }

    fn words(&self) -> String {
        self.words.into()
    }

    fn raced(&self) -> bool {
        self.raced
    }
}

const REFUSED: Said = Said {
    words: "refused",
    raced: false,
};

const RACED: Said = Said {
    words: "another copy moved it first",
    raced: true,
};

/// A store that answers every call from what a test set, and notes each,
/// by name, in the order made. `READ_FIRST` says whether it reads its
/// records before the move, as a browser's database does, falling back to
/// another store where it is refused, and noting nothing of records still
/// unread; or after.
struct Script<const READ_FIRST: bool> {
    calls: RefCell<Vec<String>>,
    keys: RefCell<KeyCache>,
    gate: RefCell<Result<Gate, Said>>,
    newer_unread: bool,
    legacy: Result<Legacy, String>,
    changed: Cell<bool>,
    records: RefCell<BTreeMap<RecordId, String>>,
    gone: RefCell<BTreeMap<RecordId, TombstoneRead>>,
    refuse_list: Cell<bool>,
    notes: RefCell<BTreeMap<String, Vec<u8>>>,
    deleted: Vec<Deleted>,
    commits: RefCell<VecDeque<Result<(), Said>>>,
    moves: RefCell<VecDeque<Result<(), Said>>>,
    /// How the history is kept, as the last change laid it.
    laid: RefCell<Option<Layout>>,
    /// How many records each read asked for, in the order asked.
    reads: RefCell<Vec<usize>>,
}

impl<const READ_FIRST: bool> Script<READ_FIRST> {
    fn new(gate: Result<Gate, Said>) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            keys: RefCell::new(KeyCache::new()),
            gate: RefCell::new(gate),
            newer_unread: false,
            legacy: Ok(None),
            changed: Cell::new(false),
            records: RefCell::new(BTreeMap::new()),
            gone: RefCell::new(BTreeMap::new()),
            refuse_list: Cell::new(false),
            notes: RefCell::new(BTreeMap::new()),
            deleted: Vec::new(),
            commits: RefCell::new(VecDeque::new()),
            moves: RefCell::new(VecDeque::new()),
            laid: RefCell::new(None),
            reads: RefCell::new(Vec::new()),
        }
    }

    fn call(&self, name: impl Into<String>) {
        self.calls.borrow_mut().push(name.into());
    }

    /// The calls made since this was last asked.
    fn calls(&self) -> Vec<String> {
        std::mem::take(&mut *self.calls.borrow_mut())
    }

    fn hold(&self, id: RecordId, json: &str) {
        self.records.borrow_mut().insert(id, json.into());
    }

    fn delete(&self, id: RecordId, read: TombstoneRead) {
        self.gone.borrow_mut().insert(id, read);
    }

    fn listing(&self) -> Listing<()> {
        Listing {
            records: self.records.borrow().keys().copied().collect(),
            gone: self.gone.borrow().keys().copied().collect(),
            skipped: false,
            listed: (),
        }
    }

    fn read_of(&self, id: RecordId) -> RecordRead {
        let json = self.records.borrow().get(&id).cloned()?;
        Some(read_record(&json).ok_or_else(|| unreadable(&json)))
    }
}

fn copy_of(read: &TombstoneRead) -> TombstoneRead {
    match read {
        TombstoneRead::Bytes(raw) => TombstoneRead::Bytes(raw.clone()),
        TombstoneRead::NoText => TombstoneRead::NoText,
        TombstoneRead::Failed(why) => TombstoneRead::Failed(why.clone()),
    }
}

fn named(note: Note) -> String {
    format!("{note:?}")
}

impl<const READ_FIRST: bool> Backend for Script<READ_FIRST> {
    type Refused = Said;
    type Look = usize;
    type Listed = ();
    const ORDER: Order = if READ_FIRST {
        Order::ReadThenMove
    } else {
        Order::MoveThenRead
    };
    const FALLS_BACK: bool = READ_FIRST;

    fn keys(&self) -> &RefCell<KeyCache> {
        &self.keys
    }

    async fn open_gate(&self) -> Result<Opening, Said> {
        self.call("open_gate");
        if self.newer_unread {
            return Ok(Opening::NewerUnread);
        }
        self.gate.borrow().clone().map(Opening::Gate)
    }

    async fn gate(&self) -> Result<Gate, Said> {
        self.call("gate");
        self.gate.borrow().clone()
    }

    async fn list(&self) -> Result<Listing<()>, Said> {
        self.call("list");
        if self.refuse_list.get() {
            return Err(REFUSED);
        }
        Ok(self.listing())
    }

    async fn read_listed(&self, listing: &Listing<()>) -> Vec<(RecordId, RecordRead)> {
        self.call("read_listed");
        listing
            .records
            .difference(&listing.gone)
            .map(|id| (*id, self.read_of(*id)))
            .collect()
    }

    async fn read_whole(&self, keep: bool) -> Result<WholeRead<()>, Said> {
        self.call(format!("read_whole({keep})"));
        if self.refuse_list.get() {
            return Err(REFUSED);
        }
        let listing = self.listing();
        let read = listing
            .records
            .difference(&listing.gone)
            .map(|id| (*id, self.read_of(*id)))
            .collect();
        let kept = if keep {
            self.records.borrow().clone()
        } else {
            BTreeMap::new()
        };
        let tombstones = self
            .gone
            .borrow()
            .iter()
            .map(|(id, read)| Tombstone {
                id: Some(*id),
                read: copy_of(read),
            })
            .collect();
        Ok(WholeRead {
            listing,
            read,
            kept,
            tombstones,
        })
    }

    async fn read(&self, ids: &[RecordId], listed: Option<&()>) -> Vec<Raw> {
        self.call(if listed.is_some() {
            "read(listed)"
        } else {
            "read"
        });
        self.reads.borrow_mut().push(ids.len());
        let records = self.records.borrow();
        ids.iter()
            .map(|id| match records.get(id) {
                Some(json) => Raw::Text(json.clone()),
                None => Raw::Missing,
            })
            .collect()
    }

    async fn tombstones(&self) -> Result<Vec<Tombstone>, Said> {
        self.call("tombstones");
        Ok(self
            .gone
            .borrow()
            .iter()
            .map(|(id, read)| Tombstone {
                id: Some(*id),
                read: copy_of(read),
            })
            .collect())
    }

    async fn tombstones_of(&self, ids: &[RecordId]) -> Option<TombstonesOf> {
        self.call("tombstones_of");
        let gone = self.gone.borrow();
        Some(
            ids.iter()
                .filter_map(|id| {
                    let held = match gone.get(id)? {
                        TombstoneRead::Bytes(raw) => Some(raw.clone()),
                        _ => None,
                    };
                    Some((*id, held))
                })
                .collect(),
        )
    }

    async fn note(&self, note: Note) -> Option<Vec<u8>> {
        self.call(format!("note({note:?})"));
        self.notes.borrow().get(&named(note)).cloned()
    }

    async fn write_note(&self, note: Note, names: Option<&BTreeSet<String>>) {
        self.call(format!("write_note({note:?})"));
        let mut notes = self.notes.borrow_mut();
        match names {
            Some(names) => {
                notes.insert(named(note), serde_json::to_vec(names).unwrap());
            }
            None => {
                notes.remove(&named(note));
            }
        }
    }

    async fn legacy_look(&self, _layout: &Layout) -> LegacyLook {
        self.call("legacy_look");
        if !self.changed.get() {
            return LegacyLook::Same;
        }
        LegacyLook::Changed(READ_FIRST.then(|| self.legacy.clone().unwrap_or_default()))
    }

    async fn legacy(&self) -> Result<Legacy, String> {
        self.call("legacy");
        self.legacy.clone()
    }

    async fn legacy_deleted(&self) -> Vec<Deleted> {
        self.call("legacy_deleted");
        self.deleted.clone()
    }

    async fn set_aside_history(&self, _raw: &[u8]) -> (Option<SetAside>, bool) {
        self.call("set_aside_history");
        (Some(SetAside::History { kept: None }), false)
    }

    async fn set_aside_sessions(
        &self,
        _unread: &str,
        count: usize,
    ) -> Result<Option<SetAside>, String> {
        self.call("set_aside_sessions");
        Ok(Some(SetAside::Sessions { count, kept: None }))
    }

    async fn here(&self) -> Result<(BTreeSet<RecordId>, BTreeSet<RecordId>), Said> {
        self.call("here");
        let listing = self.listing();
        Ok((listing.records, listing.gone))
    }

    async fn texts(&self, ids: &[RecordId]) -> Result<Vec<(RecordId, String)>, Said> {
        self.call("texts");
        let records = self.records.borrow();
        Ok(ids
            .iter()
            .filter_map(|id| Some((*id, records.get(id)?.clone())))
            .collect())
    }

    fn moving(&self) {
        self.call("moving");
    }

    async fn cut_short(&self) -> Vec<HistoryNotice> {
        self.call("cut_short");
        Vec::new()
    }

    async fn unlink(&self, ids: &[RecordId]) {
        self.call("unlink");
        for id in ids {
            self.records.borrow_mut().remove(id);
        }
    }

    async fn commit(&self, writes: &Writes<'_>, laid: Laid<'_>) -> Made<Said> {
        let laid = match laid {
            Laid::AsItIs => "as it is",
            Laid::Over { next, .. } => {
                *self.laid.borrow_mut() = Some(next.clone());
                "over"
            }
        };
        self.call(format!("commit({laid})"));
        if let Some(refused) = self.commits.borrow_mut().pop_front().and_then(Result::err) {
            return Made {
                placed: Vec::new(),
                saved: Err(refused),
            };
        }
        let change = writes.change;
        let mut placed = Vec::new();
        for record in &change.put {
            self.hold(record.id, &record.json);
            placed.push(Some(record.id));
        }
        for (id, held) in writes.tombstones() {
            self.delete(id, TombstoneRead::Bytes(held.as_bytes().to_vec()));
            self.records.borrow_mut().remove(&id);
        }
        Made {
            placed,
            saved: Ok(HistorySaved::Stored {
                kept: change.put.len(),
            }),
        }
    }

    async fn write_layout(&self, _from: &Layout, next: &Layout) -> Result<(), Said> {
        self.call("write_layout");
        *self.laid.borrow_mut() = Some(next.clone());
        Ok(())
    }

    async fn write_move(
        &self,
        records: &[Record],
        _layout: &Layout,
    ) -> Result<Vec<Option<RecordId>>, Said> {
        self.call(format!("write_move({})", records.len()));
        if let Some(refused) = self.moves.borrow_mut().pop_front().and_then(Result::err) {
            return Err(refused);
        }
        for record in records {
            self.hold(record.id, &record.json);
        }
        Ok(records.iter().map(|record| Some(record.id)).collect())
    }

    async fn raced(&self) {
        self.call("raced");
    }

    fn look(&self) -> usize {
        self.call("look");
        0
    }

    fn saw(&self, _look: usize) {
        self.call("saw");
    }

    fn held_back(&self) {
        self.call("held_back");
    }

    fn loaded(&self, _load: &mut HistoryLoad) {
        self.call("loaded");
    }
}

/// Done then and there, as every scripted call answers at once.
fn now<T>(op: impl std::future::Future<Output = T>) -> T {
    at_once(op).expect("answered at once")
}

fn calls(of: &[&str]) -> Vec<String> {
    of.iter().map(|call| call.to_string()).collect()
}

fn moved_layout() -> Layout {
    legacy::moved(&[], None, false, &[], 1_000).1
}

/// A history kept whole, of `sessions`, as an older build writes it.
fn kept_whole(sessions: &[SessionResult]) -> Legacy {
    let raw = serde_json::to_vec(sessions).unwrap();
    let fingerprint = Fingerprint {
        len: raw.len() as u64,
        mtime: 1,
    };
    Some((raw, fingerprint))
}

/// A store that moves before it reads makes the move, each of its steps,
/// and only then reads what it made: the history kept whole, where it
/// sets aside what does not read, the deletions it remembers, what is here
/// already, and the records with how the history is kept; then what a
/// change cut short left, the records listed and read, as of a look taken
/// before they were listed, and the notes of what was told, read before
/// any is written.
#[test]
fn a_store_that_moves_first_makes_the_move_and_then_reads_what_it_made() {
    let mut script = Script::<false>::new(Ok(Gate::Unmoved));
    script.legacy = Ok(kept_whole(&[finished(1_000)]));
    let load = now(load(&script)).expect("read");
    assert!(load.complete);
    assert_eq!(load.sessions.len(), 1);
    assert_eq!(
        script.calls(),
        calls(&[
            "open_gate",
            "legacy",
            "moving",
            "legacy_deleted",
            "here",
            "write_move(1)",
            "cut_short",
            "legacy_look",
            "look",
            "list",
            "saw",
            "read_listed",
            "note(Failing)",
            "note(Told)",
            "note(UnreadTold)",
            "note(DamagedTold)",
            "legacy_look",
            "loaded",
        ])
    );
}

fn record_of(at: u64) -> Record {
    let session = finished(at);
    Record::of(&session, |json| RecordId::next(at, None, json)).unwrap()
}

/// A store that moves first, with a history moved already, an older
/// build's change to take up, and a record left beside its tombstone: the
/// tombstones are read before the records are listed, the record beside
/// its tombstone is taken away once they are, and the take-up reads the
/// history kept whole after the records, then the deletions it remembers,
/// then the tombstones again, and makes its change, the gate read again
/// first, as its own.
#[test]
fn a_store_that_moves_first_takes_up_after_the_records_reading_the_tombstones_before_and_after() {
    let mut script = Script::<false>::new(Ok(Gate::Open(Box::new(moved_layout()))));
    script.legacy = Ok(kept_whole(&[finished(crate::time::now_ms())]));
    script.changed.set(true);
    let (kept, beside) = (record_of(1_000), record_of(2_000));
    script.hold(kept.id, &kept.json);
    script.hold(beside.id, &beside.json);
    script.delete(beside.id, TombstoneRead::Bytes(b"{}".to_vec()));
    let load = now(load(&script)).expect("read");
    assert_eq!(load.sessions.len(), 2, "the one here and the one taken up");
    let laid = script.laid.borrow().clone().expect("laid");
    let fingerprint = script
        .legacy
        .clone()
        .unwrap()
        .map(|(_, fingerprint)| fingerprint);
    assert_eq!(
        laid.taken_up(),
        Some(fingerprint),
        "what was taken up, noted"
    );
    assert_eq!(
        script.calls(),
        calls(&[
            "open_gate",
            "cut_short",
            "legacy_look",
            "tombstones",
            "look",
            "list",
            "saw",
            "unlink",
            "read_listed",
            "note(Failing)",
            "note(Told)",
            "note(UnreadTold)",
            "note(DamagedTold)",
            "legacy_look",
            "legacy",
            "legacy_deleted",
            "tombstones",
            "gate",
            "commit(over)",
            "loaded",
        ])
    );
}

/// A take-up reads the records it knows no session of a part at a time,
/// for their keys, each part's texts let go of before the next is read:
/// however many another copy stored, no more than [`READ_PART`] are held
/// at once. Read all at once, as since 95e0361, every text another copy
/// stored was held at once, as the files had never held them.
#[test]
fn a_take_up_reads_the_records_it_knows_no_session_of_a_part_at_a_time() {
    let script = Script::<false>::new(Ok(Gate::Open(Box::new(moved_layout()))));
    script.changed.set(true);
    for at in 1..=600 {
        let record = record_of(at * 1_000);
        script.hold(record.id, &record.json);
    }
    now(listing(&script)).expect("listed");
    let reads = script.reads.borrow();
    assert_eq!(reads.iter().sum::<usize>(), 600, "each read once");
    assert!(reads.iter().all(|&asked| asked <= READ_PART), "{reads:?}");
    assert_eq!(script.keys.borrow().len(), 600, "each one's key noted");
}

/// A store that reads first notes its look before anything is read, reads
/// the records with the tombstones before them, and reads the tombstones
/// again after them where it makes the move, over what it read, which it
/// writes with its own look kept up; then tells, and takes up, the history
/// kept whole read with its look.
#[test]
fn a_store_that_reads_first_reads_the_records_and_then_moves_over_them() {
    let mut script = Script::<true>::new(Ok(Gate::Unmoved));
    script.legacy = Ok(kept_whole(&[finished(1_000)]));
    let load = now(load(&script)).expect("read");
    assert!(load.complete);
    assert_eq!(load.sessions.len(), 1, "what was moved, among what is read");
    assert_eq!(
        script.calls(),
        calls(&[
            "look",
            "saw",
            "open_gate",
            "cut_short",
            "read_whole(true)",
            "tombstones",
            "legacy",
            "moving",
            "legacy_deleted",
            "write_move(1)",
            "note(Failing)",
            "note(Told)",
            "note(UnreadTold)",
            "note(DamagedTold)",
            "legacy_look",
            "loaded",
        ])
    );
    // Moved already, with an older build's change to take up: the history
    // kept whole is read with the look, and the deletions it remembers,
    // before the tombstones and the records, those the move wrote read for
    // the sessions they hold, as a move notes none; and, as the session it
    // holds is here, only what was looked at is noted.
    script.changed.set(true);
    *script.gate.borrow_mut() = Ok(Gate::Open(Box::new(moved_layout())));
    now(listing(&script)).expect("listed");
    assert_eq!(
        script.calls(),
        calls(&[
            "gate",
            "legacy_look",
            "legacy_deleted",
            "tombstones",
            "list",
            "read(listed)",
            "tombstones",
            "write_layout",
            "look",
            "list",
            "saw",
            "note(Failing)",
            "note(UnreadTold)",
            "note(DamagedTold)",
        ])
    );
}

/// A newer Dust's store whose head cannot be read is not read at all.
#[test]
fn a_newer_store_that_cannot_be_read_reads_nothing() {
    let mut script = Script::<true>::new(Ok(Gate::Newer));
    script.newer_unread = true;
    let load = now(load(&script)).expect("read");
    assert_eq!(load.notices, [HistoryNotice::Newer]);
    assert_eq!(
        script.calls(),
        calls(&["look", "saw", "open_gate", "loaded"])
    );
}

/// A move another copy makes first each time is tried as many times as
/// MOVE_TRIES says, what it set aside kept across the tries, and then the
/// store says why it keeps no history; a store that falls back is refused
/// so where a read is.
#[test]
fn a_move_raced_each_time_is_tried_again_and_then_given_up() {
    let mut script = Script::<true>::new(Ok(Gate::Unmoved));
    script.legacy = Ok(Some((
        b"{ not a list".to_vec(),
        Fingerprint { len: 12, mtime: 1 },
    )));
    script
        .moves
        .borrow_mut()
        .extend([Err(RACED), Err(RACED), Err(RACED)]);
    let not = now(load(&script)).expect_err("raced");
    assert_eq!(not.why, RACED.words);
    assert_eq!(not.set_aside.len(), MOVE_TRIES, "each try's");
    let tried = script.calls();
    assert_eq!(
        tried
            .iter()
            .filter(|call| call.starts_with("write_move"))
            .count(),
        MOVE_TRIES
    );
    let refused = Script::<true>::new(Ok(Gate::Unmoved));
    refused.refuse_list.set(true);
    assert_eq!(now(load(&refused)).expect_err("refused").why, REFUSED.words);
}

/// A take-up held back by a tombstone that cannot be read makes no change,
/// and is still to take up once the listing has been seen; a move made
/// over such a tombstone waits, and writes nothing.
#[test]
fn a_tombstone_that_cannot_be_read_holds_back_the_take_up_and_the_move() {
    let script = Script::<false>::new(Ok(Gate::Open(Box::new(moved_layout()))));
    script.changed.set(true);
    let gone = record_of(1_000);
    script.delete(gone.id, TombstoneRead::Failed("held open".into()));
    now(listing(&script)).expect("listed");
    assert_eq!(
        script.calls(),
        calls(&[
            "gate",
            "legacy_look",
            "tombstones",
            "look",
            "list",
            "saw",
            "held_back",
            "note(Failing)",
            "note(UnreadTold)",
            "note(DamagedTold)",
        ])
    );
    let mut moving = Script::<false>::new(Ok(Gate::Unmoved));
    moving.legacy = Ok(kept_whole(&[finished(1_000)]));
    let here = record_of(500);
    moving.hold(here.id, &here.json);
    moving.delete(gone.id, TombstoneRead::Failed("held open".into()));
    let load = now(load(&moving)).expect("read");
    assert_eq!(
        load.notices,
        [HistoryNotice::MovePending("held open".into())]
    );
    assert!(!load.complete);
    let calls = moving.calls();
    assert!(
        !calls.iter().any(|call| call.starts_with("write_move")),
        "{calls:?}"
    );
}

/// A read takes what it read out of the notes, the note of damage first,
/// the gate read only where something goes; asks of records still unread
/// read the gate whatever; deletions look at each record's tombstone, and
/// the sessions deleted at every tombstone; a change refused by the gate
/// is made nowhere.
#[test]
fn reads_and_asks_go_by_the_notes_and_the_tombstones_in_order() {
    let script = Script::<false>::new(Ok(Gate::Unmoved));
    let one = record_of(1_000);
    script.hold(one.id, &one.json);
    let names = |ids: &[RecordId]| {
        serde_json::to_vec(&ids.iter().map(|id| id.name()).collect::<BTreeSet<_>>()).unwrap()
    };
    script
        .notes
        .borrow_mut()
        .insert(named(Note::Failing), names(&[one.id]));
    now(read(&script, &[one.id]));
    assert_eq!(
        script.calls(),
        calls(&[
            "read",
            "note(DamagedTold)",
            "note(Failing)",
            "gate",
            "write_note(Failing)",
            "note(UnreadTold)",
        ])
    );
    now(still_unread(&script, &[one.id]));
    assert_eq!(
        script.calls(),
        calls(&[
            "note(UnreadTold)",
            "gate",
            "write_note(UnreadTold)",
            "note(Told)",
            "write_note(Told)",
            "note(DamagedTold)",
            "write_note(DamagedTold)",
            "note(Failing)",
            "write_note(Failing)",
        ])
    );
    now(still_unread(&script, &[one.id]));
    assert_eq!(
        script.calls(),
        calls(&["note(UnreadTold)", "gate"]),
        "told of once"
    );
    now(deletions(&script, &[one.id]));
    now(deleted_sessions(&script, &[one.id]));
    now(deleted_keys(&script, vec![]));
    assert_eq!(
        script.calls(),
        calls(&["tombstones_of", "tombstones_of", "tombstones"])
    );
    *script.gate.borrow_mut() = Ok(Gate::Newer);
    let change = HistoryChange {
        put: vec![one],
        ..HistoryChange::default()
    };
    assert_eq!(now(apply(&script, &change)), HistorySaved::Newer);
    assert_eq!(script.calls(), calls(&["gate"]));
    *script.gate.borrow_mut() = Ok(Gate::Unmoved);
    assert_eq!(
        now(apply(&script, &change)),
        HistorySaved::Stored { kept: 1 }
    );
    assert_eq!(script.calls(), calls(&["gate", "commit(as it is)"]));
}

/// A session finished on day `day`, scored as it reads back.
fn on_day(day: u64) -> SessionResult {
    crate::persist::record::scored(finished(day * 86_400_000))
}

/// A session an older build stored, as it writes one, with no format, is
/// there under another name for a move made again, wherever the format
/// would be written: none, or one carried at the end, as an older build
/// writes back this build's. This build writes the format in every session
/// ([`cw_core::SESSION_FORMAT`]), so its bytes were never an older
/// build's, and the move stored the session a second time.
#[test]
fn a_move_made_again_writes_no_session_an_older_build_stored_twice() {
    use crate::persist::tests::as_an_older_build_wrote;
    let held = [on_day(1), on_day(2), on_day(3)];
    let moved = crate::persist::record::records_for_blob(&held);
    let older = as_an_older_build_wrote(&held[1]);
    let carried = format!(
        "{},\"format\":{}}}",
        as_an_older_build_wrote(&held[2])
            .strip_suffix('}')
            .expect("an object"),
        cw_core::SESSION_FORMAT
    );
    let texts: BTreeMap<RecordId, String> = [&older, &carried]
        .into_iter()
        .zip([&held[1], &held[2]])
        .map(|(json, session)| (RecordId::placed(session.timestamp, json), json.clone()))
        .collect();
    let here = Here {
        records: texts.keys().copied().collect(),
        texts,
        ..Here::default()
    };
    assert_eq!(here.not_moved_yet(moved.clone()), [moved[0].clone()]);
}

/// A move is made over nothing but what is not there already: a record
/// there by its own name is written again, which writes nothing; one
/// deleted by name or by key, or there under another name, is not.
#[test]
fn a_move_made_again_writes_no_session_twice() {
    let held = [on_day(1), on_day(2), on_day(3), on_day(4)];
    let moved = crate::persist::record::records_for_blob(&held);
    let elsewhere =
        Record::of(&held[2], |json| RecordId::placed(held[2].timestamp, json)).expect("a record");
    let texts: BTreeMap<RecordId, String> = [
        (moved[0].id, moved[0].json.clone()),
        (elsewhere.id, elsewhere.json),
    ]
    .into_iter()
    .collect();
    let gone_keys: HashSet<SessionKey> = [SessionKey::of(&held[3])].into_iter().collect();
    let here = Here {
        records: texts.keys().copied().collect(),
        gone: [moved[1].id].into_iter().collect(),
        texts,
        gone_keys: gone_keys.clone(),
    };
    assert_eq!(here.not_moved_yet(moved.clone()), [moved[0].clone()]);
    // Into a store that holds nothing, all of it; but for a session a
    // tombstone named as the records were read, gone since, as an Undo
    // takes one away.
    assert_eq!(Here::default().not_moved_yet(moved.clone()), moved);
    let named = Here {
        gone_keys,
        ..Here::default()
    };
    assert_eq!(named.not_moved_yet(moved.clone()), moved[..3]);
}
