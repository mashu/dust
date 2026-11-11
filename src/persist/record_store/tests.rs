//! The records and tombstones as every store reads them.

use std::collections::BTreeSet;

use super::*;
use crate::persist::Deleted;
use crate::persist::record::{HistoryNotice, unreadable};
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
