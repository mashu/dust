//! The records and tombstones as every store reads them.

use super::*;
use crate::persist::Deleted;
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
