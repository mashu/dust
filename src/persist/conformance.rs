//! What every store must do with the history, tested once for all of them:
//! each store's tests run these cases on it ([`conformance_tests`]), the
//! files on a folder of their own, a browser's database on the tests'
//! stand-in for one, and the test store, each reached as the app reaches it
//! ([`Harness`]). A store that cannot meet a case leaves it out by name,
//! saying why, beside where it runs the rest.

use cw_core::SessionResult;

use super::record::{
    Deletions, HistoryChange, HistoryLoad, HistoryNotice, Record, RecordId, scored,
};
use super::saved::HistorySaved;
use super::session::{Deleted, SessionKey};

/// A store under test, as every case reaches it.
pub(crate) trait Harness {
    /// A store that holds no history yet, beside the history an older
    /// build keeps whole, holding `whole`.
    fn holding(whole: &[SessionResult]) -> Self;
    /// The whole history, as this copy reads it, as the app opens.
    fn load(&mut self) -> HistoryLoad;
    fn apply(&self, change: HistoryChange) -> HistorySaved;
    fn listing(&self) -> Option<Vec<RecordId>>;
    /// What each record holds, as it is stored: none for one that could
    /// not be read.
    fn read(&self, ids: Vec<RecordId>) -> Vec<Option<String>>;
    fn deletions(&self, ids: Vec<RecordId>) -> Option<Deletions>;
    fn deleted_sessions(&self, ids: Vec<RecordId>) -> Option<Vec<(RecordId, SessionKey)>>;
    fn deleted_keys(&self, keys: Vec<SessionKey>) -> Option<Vec<SessionKey>>;
    fn still_unread(&self, ids: Vec<RecordId>) -> Vec<HistoryNotice>;
    fn changed(&self) -> bool;
    /// An older build keeps `sessions` whole, over what it kept.
    fn keep_whole(&self, sessions: &[SessionResult]);
    /// An older build remembers `deleted`, as deleted by hand there.
    fn keep_deleted(&self, deleted: &[Deleted]);
    /// `json` stored as the record `id`, as another build, or damage,
    /// leaves one.
    fn plant(&self, id: RecordId, json: &str);
    /// A tombstone for the record `id`, as another build, or damage,
    /// leaves one: holding `held`, or, given none, what is no text, as a
    /// later build may store in a browser's database, or damage leaves of
    /// a file.
    fn plant_tombstone(&self, id: RecordId, held: Option<&str>);
}

/// The cases this suite holds, each a test of the store `$harness`, named
/// as the case is.
macro_rules! conformance_tests {
    ($harness:ty: $($case:ident),* $(,)?) => {
        $(
            #[test]
            fn $case() {
                crate::persist::conformance::$case::<$harness>();
            }
        )*
    };
}
pub(crate) use conformance_tests;

const DAY: u64 = 86_400_000;

/// A session finished `days` days ago, on a day of its own, scored as
/// this build stores it.
fn ago(days: u64) -> SessionResult {
    let at = crate::time::now_ms() - days * DAY;
    scored(SessionResult {
        timestamp: at,
        ..crate::persist::tests::session(&format!("day {at}"))
    })
}

fn sessions(load: &HistoryLoad) -> Vec<SessionResult> {
    load.sessions
        .iter()
        .map(|(_, session)| session.clone())
        .collect()
}

/// The history an older build kept whole moves in once, in its order:
/// a copy that reads it again finds it moved, and stores nothing more.
pub(crate) fn a_history_kept_whole_moves_in_once<H: Harness>() {
    let whole = [ago(30), ago(20), ago(10)];
    let mut store = H::holding(&whole);
    let load = store.load();
    assert!(load.complete, "moved");
    assert_eq!(sessions(&load), whole);
    let again = store.load();
    assert_eq!(sessions(&again), whole, "moved once");
    assert_eq!(store.listing().map(|listed| listed.len()), Some(3));
}

/// A session deleted by hand is gone from the history, and its tombstone
/// says when, and which session it held, whatever its record is named.
pub(crate) fn a_deletion_is_a_tombstone_that_names_its_session<H: Harness>() {
    let whole = [ago(30), ago(20)];
    let mut store = H::holding(&whole);
    let load = store.load();
    let (gone, kept) = (load.sessions[0].0, load.sessions[1].0);
    let at = crate::time::now_ms();
    let forget = HistoryChange {
        forget: vec![(gone, Deleted::of(&whole[0], at))],
        ..HistoryChange::default()
    };
    assert_eq!(store.apply(forget), HistorySaved::Stored { kept: 0 });
    assert_eq!(store.listing(), Some(vec![kept]));
    assert_eq!(
        store.deletions(vec![gone, kept]),
        Some(vec![(gone, Some(at))])
    );
    let key = SessionKey::of(&whole[0]);
    assert_eq!(
        store.deleted_sessions(vec![gone, kept]),
        Some(vec![(gone, key.clone())])
    );
    let keys = vec![key.clone(), SessionKey::of(&whole[1])];
    assert_eq!(store.deleted_keys(keys), Some(vec![key]));
    assert_eq!(sessions(&store.load()), [whole[1].clone()]);
}

/// A session deleted and put back, as Undo puts one back, is in the
/// history again, under its own name, and deleted no more.
pub(crate) fn a_session_put_back_is_there_again<H: Harness>() {
    let whole = [ago(30), ago(20)];
    let mut store = H::holding(&whole);
    let load = store.load();
    let (id, session) = load.sessions[0].clone();
    let record = Record {
        id,
        json: store.read(vec![id]).remove(0).expect("it reads"),
    };
    let forget = HistoryChange {
        forget: vec![(id, Deleted::of(&session, crate::time::now_ms()))],
        ..HistoryChange::default()
    };
    store.apply(forget);
    let undo = HistoryChange {
        put: vec![record],
        unforget: vec![id],
        ..HistoryChange::default()
    };
    assert_eq!(store.apply(undo), HistorySaved::Stored { kept: 1 });
    assert_eq!(
        store.listing().map(|listed| listed.contains(&id)),
        Some(true)
    );
    assert_eq!(store.deletions(vec![id]), Some(Vec::new()));
    assert_eq!(sessions(&store.load()), whole);
}

/// What an older build stores in the history it keeps whole after the move
/// is taken up, once, as the history is next read.
pub(crate) fn what_an_older_build_stores_after_the_move_is_taken_up_once<H: Harness>() {
    let whole = [ago(30), ago(20)];
    let mut store = H::holding(&whole);
    store.load();
    assert!(!store.changed(), "nothing changed since");
    let since = ago(5);
    store.keep_whole(&[whole[0].clone(), whole[1].clone(), since.clone()]);
    assert!(store.changed(), "an older build wrote");
    let load = store.load();
    assert_eq!(sessions(&load), [whole[0].clone(), whole[1].clone(), since]);
    assert!(!store.changed(), "taken up");
    assert_eq!(store.load().sessions.len(), 3, "taken up once");
    assert_eq!(store.listing().map(|listed| listed.len()), Some(3));
}

/// A deletion an older build makes after the move is made here too.
pub(crate) fn a_deletion_an_older_build_made_after_the_move_is_honoured<H: Harness>() {
    let whole = [ago(30), ago(20)];
    let mut store = H::holding(&whole);
    let load = store.load();
    let gone = load.sessions[1].0;
    store.keep_whole(std::slice::from_ref(&whole[0]));
    let at = crate::time::now_ms();
    store.keep_deleted(&[Deleted::of(&whole[1], at)]);
    assert_eq!(sessions(&store.load()), [whole[0].clone()]);
    assert_eq!(store.deletions(vec![gone]), Some(vec![(gone, Some(at))]));
}

/// A record this build cannot read is left where it is, and told of once:
/// as a later build's, or as damaged.
pub(crate) fn a_record_this_build_cannot_read_is_told_of_once<H: Harness>() {
    let mut store = H::holding(&[ago(30)]);
    store.load();
    let later = RecordId {
        order: 5_000,
        nonce: 9,
    };
    let held = format!(
        r#"{{"format":{},"date":"later","timestamp":5}}"#,
        cw_core::SESSION_FORMAT + 1
    );
    store.plant(later, &held);
    let damaged = RecordId {
        order: 6_000,
        nonce: 9,
    };
    store.plant(damaged, "{ damaged");
    let load = store.load();
    assert_eq!(load.unread, [later, damaged]);
    assert_eq!(load.later, [(later, SessionKey::new(5, "later".into()))]);
    assert_eq!(
        load.notices,
        [
            HistoryNotice::Later { count: 1 },
            HistoryNotice::Damaged { count: 1 }
        ]
    );
    assert_eq!(store.load().notices, [], "told once");
    assert_eq!(store.read(vec![later]), [Some(held)], "left where it is");
}

/// A record still unread after the app's tries is told of once, where the
/// store notes it.
pub(crate) fn records_still_unread_are_told_of_once<H: Harness>() {
    let mut store = H::holding(&[ago(30)]);
    let id = store.load().sessions[0].0;
    assert_eq!(
        store.still_unread(vec![id]),
        [HistoryNotice::Unread { count: 1 }]
    );
    assert_eq!(store.still_unread(vec![id]), [], "told once");
}

/// A session an older build stored, taken up where other bytes have the
/// name the take-up gives it, goes under the next name, and is read there:
/// the load holds it there, beside what holds its own name, and an older
/// build's deletion of it, taken up by the same copy, deletes that record
/// and not the other.
pub(crate) fn a_session_taken_up_under_a_name_other_bytes_have_is_read_where_it_went<H: Harness>() {
    let before = [ago(30), ago(20)];
    let mut store = H::holding(&before);
    store.load();
    let since = ago(10);
    let asked = Record::of(&since, |json| RecordId::placed(since.timestamp, json))
        .expect("a record")
        .id;
    let other = serde_json::to_string(&ago(5)).expect("JSON");
    store.plant(asked, &other);
    store.keep_whole(&[before[0].clone(), before[1].clone(), since.clone()]);
    let load = store.load();
    let went = asked.bumped();
    assert!(
        load.sessions.contains(&(went, since.clone())),
        "where it went: {:?}",
        load.sessions
    );
    assert!(
        load.sessions.iter().any(|(id, _)| *id == asked),
        "beside the other bytes"
    );
    // The older build deletes it.
    store.keep_whole(&before);
    store.keep_deleted(&[Deleted::of(&since, crate::time::now_ms())]);
    let listed = store.listing().expect("listed");
    assert!(
        listed.contains(&asked) && !listed.contains(&went),
        "{listed:?}"
    );
    assert_eq!(store.read(vec![asked]), [Some(other)]);
    let gone: Vec<RecordId> = store
        .deletions(vec![asked, went])
        .expect("asked")
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(gone, [went]);
}

/// A session the app stores where other bytes have the name it asks for
/// goes under the next name, and is known by that name: an older build's
/// deletion of it, taken up by the same copy, deletes its record, and not
/// the other bytes'.
pub(crate) fn a_session_stored_under_a_name_other_bytes_have_is_known_by_where_it_went<
    H: Harness,
>() {
    let before = [ago(30)];
    let mut store = H::holding(&before);
    store.load();
    let since = ago(10);
    let asked =
        Record::of(&since, |json| RecordId::placed(since.timestamp, json)).expect("a record");
    let other = serde_json::to_string(&ago(5)).expect("JSON");
    store.plant(asked.id, &other);
    let put = HistoryChange {
        put: vec![asked.clone()],
        ..HistoryChange::default()
    };
    assert_eq!(store.apply(put), HistorySaved::Stored { kept: 1 });
    let went = asked.id.bumped();
    // An older build that holds it too stores another, and deletes it.
    let theirs = ago(3);
    store.keep_whole(&[before[0].clone(), theirs]);
    store.keep_deleted(&[Deleted::of(&since, crate::time::now_ms())]);
    let listed = store.listing().expect("listed");
    assert!(
        listed.contains(&asked.id) && !listed.contains(&went),
        "{listed:?}"
    );
    assert_eq!(
        store.read(vec![asked.id]),
        [Some(other)],
        "the other bytes stay"
    );
    let gone: Vec<RecordId> = store
        .deletions(vec![asked.id, went])
        .expect("asked")
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(gone, [went]);
}

/// A tombstone that names no session, as one that is empty, one that does
/// not read, or one that is no text, deletes its record all the same: the
/// load and the listing leave the record out, and the deletions name it,
/// though not when. It deletes no session by name: the sessions deleted
/// leave it out, and what an older build stores after is taken up, nothing
/// held back by it.
pub(crate) fn a_tombstone_that_names_no_session_still_deletes_its_record<H: Harness>() {
    let whole = [ago(40), ago(30), ago(20), ago(10)];
    let mut store = H::holding(&whole);
    let ids: Vec<RecordId> = store.load().sessions.iter().map(|(id, _)| *id).collect();
    store.plant_tombstone(ids[0], Some(""));
    store.plant_tombstone(ids[1], Some("{ not a tombstone"));
    store.plant_tombstone(ids[2], None);
    assert_eq!(sessions(&store.load()), [whole[3].clone()]);
    assert_eq!(store.listing(), Some(vec![ids[3]]));
    assert_eq!(
        store.deletions(ids.clone()),
        Some(vec![(ids[0], None), (ids[1], None), (ids[2], None)])
    );
    assert_eq!(store.deleted_sessions(ids), Some(Vec::new()));
    let keys: Vec<SessionKey> = whole.iter().map(SessionKey::of).collect();
    assert_eq!(store.deleted_keys(keys), Some(Vec::new()));
    // An older build stores another.
    let since = ago(5);
    let mut kept = whole.to_vec();
    kept.push(since.clone());
    store.keep_whole(&kept);
    assert_eq!(
        store.listing().map(|listed| listed.len()),
        Some(2),
        "taken up"
    );
    assert_eq!(sessions(&store.load()), [whole[3].clone(), since]);
}
