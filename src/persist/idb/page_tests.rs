//! The history's page script ([`IDB_JS`]) run on a page, in Node, on a
//! browser's database made up in memory (`src/fake_idb.mjs`): what the
//! browser's own database makes of each thing the history asks of it.

use std::cell::RefCell;
use std::collections::BTreeMap;

use cw_core::{Rng, SplitMix64};
use serde_json::{Value, json};

use super::IDB_JS;
use super::tests::{Making, Shelves};
use crate::persist::record::{Record, RecordId};
use crate::persist::record_store::steps::{BUMPS, make_steps};
use crate::testing::{PageStep, on_fake_page};

/// What `script` sends the app, run to its end on a page where
/// `glue(options)` runs the page script for a tab of its own, `name(order,
/// nonce)` names a record, and `settle()` lets what the page has queued
/// happen. None on a machine with no Node.
fn on_page(script: &str) -> Option<Vec<Value>> {
    let glue = serde_json::to_string(IDB_JS).expect("the script, as a string");
    let script = format!(
        r#"const glue = (options) => new Function("options", {glue})(options);
const name = (order, nonce = 1) =>
  String(order).padStart(20, "0") + "-" + nonce.toString(16).padStart(16, "0");
const settle = async () => {{
  for (let i = 0; i < 20; i += 1) await new Promise((done) => setImmediate(done));
}};
const change = (fields) => ({{ put: [], forget: [], unforget: [], meta: [], layout: null, ...fields }});
const LAYOUT = '{{"layout":1,"compatible":1,"migratedAt":5}}';
{script}"#
    );
    let heard = on_fake_page(&[PageStep::wait(script)])?;
    Some(heard.into_iter().next().expect("a step").sent)
}

/// One change is one transaction, made whole or not at all: a change cut
/// short leaves nothing of itself, the records it stored before it was
/// refused included.
#[test]
fn a_change_is_one_transaction() {
    let Some(sent) = on_page(
        r#"const tab = glue();
await tab.open();
const before = fakeIdb.committed;
dioxus.send(await tab.apply(change({
  put: [[name(1), "a"], [name(2), "b"]],
  forget: [[name(9), "{}"]],
  meta: [["told", "[]"]],
})));
dioxus.send(fakeIdb.committed - before);
// Every name the next record could go under is taken by other bytes, so
// it cannot be stored: nor is the one before it.
for (let nonce = 1; nonce <= 70; nonce += 1) fakeIdb.plant("dust_history", "sessions", name(5, nonce), "other");
dioxus.send(await tab.apply(change({ put: [[name(3), "c"], [name(5), "mine"]] })));
dioxus.send(fakeIdb.committed - before);
dioxus.send(Object.keys(fakeIdb.contents("dust_history", "sessions")).includes(name(3)));
dioxus.send(fakeIdb.contents("dust_history", "deleted"));
dioxus.send(fakeIdb.contents("dust_history", "meta"));"#,
    ) else {
        return;
    };
    assert_eq!(
        sent[0],
        json!({ "ok": ["00000000000000000001-0000000000000001", "00000000000000000002-0000000000000001"] })
    );
    assert_eq!(sent[1], json!(1), "one transaction");
    assert_eq!(sent[2]["why"], "failed");
    assert!(
        sent[2]["message"]
            .as_str()
            .is_some_and(|said| said.contains("no name was free")),
        "{}",
        sent[2]
    );
    assert_eq!(sent[3], json!(1), "nothing more made");
    assert_eq!(sent[4], json!(false), "nor the record before it");
    assert_eq!(
        sent[5],
        json!({ "00000000000000000009-0000000000000001": "{}" })
    );
    assert_eq!(sent[6], json!({ "told": "[]" }));
}

/// A browser out of room for the site is told as full, and keeps nothing
/// of the change; with room again, the same change is made.
#[test]
fn a_quota_error_is_told_as_full() {
    let Some(sent) = on_page(
        r#"const tab = glue();
await tab.open();
fakeIdb.quota = 50;
const put = change({ put: [[name(1), "x".repeat(100)]] });
dioxus.send(await tab.apply(put));
dioxus.send(fakeIdb.contents("dust_history", "sessions"));
fakeIdb.quota = Infinity;
dioxus.send(await tab.apply(put));
dioxus.send(Object.keys(fakeIdb.contents("dust_history", "sessions")));"#,
    ) else {
        return;
    };
    assert_eq!(sent[0]["why"], "full");
    assert_eq!(sent[1], json!({}));
    assert_eq!(
        sent[2],
        json!({ "ok": ["00000000000000000001-0000000000000001"] })
    );
    assert_eq!(sent[3], json!(["00000000000000000001-0000000000000001"]));
}

/// Records come back in the order of their names, which is the history's,
/// a page at a time from where the last page ended, whatever order they
/// were stored in; listed, with the tombstones beside them.
#[test]
fn keys_come_back_in_history_order() {
    let Some(sent) = on_page(
        r#"const tab = glue();
await tab.open();
await tab.apply(change({ put: [[name(30), "c"], [name(10, 2), "b"], [name(10, 1), "a"], [name(200), "d"]] }));
await tab.apply(change({ forget: [[name(30), "{}"]] }));
const first = await tab.page(null, 2);
dioxus.send(first);
dioxus.send(await tab.page(first.ok.keys[1], 2));
dioxus.send(await tab.list());
dioxus.send(await tab.read([name(10, 2), name(30)]));
dioxus.send(await tab.tombstonesOf([name(10, 2), name(30)]));
dioxus.send(await tab.tombstones());"#,
    ) else {
        return;
    };
    let (a, b, c, d) = (
        "00000000000000000010-0000000000000001",
        "00000000000000000010-0000000000000002",
        "00000000000000000030-0000000000000001",
        "00000000000000000200-0000000000000001",
    );
    assert_eq!(
        sent[0],
        json!({ "ok": { "keys": [a, b], "values": ["a", "b"] } })
    );
    assert_eq!(sent[1], json!({ "ok": { "keys": [d], "values": ["d"] } }));
    assert_eq!(
        sent[2],
        json!({ "ok": { "sessions": [a, b, d], "deleted": [c] } })
    );
    assert_eq!(sent[3], json!({ "ok": ["b", null] }));
    assert_eq!(
        sent[4],
        json!({ "ok": { "keys": [c], "values": ["{}"] } }),
        "the tombstones of those deleted, and what each holds"
    );
    assert_eq!(sent[5], json!({ "ok": { "keys": [c], "values": ["{}"] } }));
}

/// Two tabs opening at once, each moving the history kept whole in, move
/// it once: the move is written only where nothing says how the history is
/// kept, and the second finds the first's.
#[test]
fn two_tabs_migrating_at_once_migrate_once() {
    let Some(sent) = on_page(
        r#"const [one, two] = [glue(), glue()];
await Promise.all([one.open(), two.open()]);
const before = fakeIdb.committed;
const move = change({
  put: [[name(1), "a"], [name(2), "b"]],
  layout: { write: LAYOUT, over: true, expect: null },
});
dioxus.send(await Promise.all([one.apply(move), two.apply(move)]));
dioxus.send(fakeIdb.committed - before);
dioxus.send(fakeIdb.contents("dust_history", "sessions"));
dioxus.send(await two.head());
// Over the layout it holds, as a take-up from an older build writes it.
const next = '{"layout":1,"compatible":1,"migratedAt":6}';
const over = (expect) => change({ layout: { write: next, over: true, expect } });
dioxus.send(await Promise.all([one.apply(over(LAYOUT)), two.apply(over(LAYOUT))]));
dioxus.send((await one.head()).ok.layout);"#,
    ) else {
        return;
    };
    let answers = sent[0].as_array().expect("both answered");
    let moved = json!({ "ok": ["00000000000000000001-0000000000000001", "00000000000000000002-0000000000000001"] });
    assert!(answers.contains(&moved), "{answers:?}");
    assert!(
        answers.iter().any(|answer| answer["why"] == "raced"),
        "{answers:?}"
    );
    assert_eq!(sent[1], json!(1), "moved once");
    assert_eq!(
        sent[2],
        json!({
            "00000000000000000001-0000000000000001": "a",
            "00000000000000000002-0000000000000001": "b",
        })
    );
    assert_eq!(
        sent[3]["ok"]["layout"],
        r#"{"layout":1,"compatible":1,"migratedAt":5}"#
    );
    assert_eq!(sent[3]["ok"]["count"], 2, "as many records as it holds");
    let answers = sent[4].as_array().expect("both answered");
    assert!(answers.iter().any(|answer| answer["why"] == "raced"));
    assert_eq!(sent[5], r#"{"layout":1,"compatible":1,"migratedAt":6}"#);
}

/// What was noted of the records told of comes back with how the history
/// is kept as it is opened, read under the name builds read it by, `told`
/// beside the layout, which 0.16.4 to 0.16.9 go by too; null where none is
/// held. Read under another name, every record this build cannot read was
/// told of at every launch, and its next note dropped what such a build
/// noted there.
#[test]
fn the_head_hands_back_what_was_told_under_the_name_builds_read_it_by() {
    let Some(sent) = on_page(
        r#"const tab = glue();
await tab.open();
dioxus.send((await tab.head()).ok.told);
fakeIdb.plant("dust_history", "meta", "told", '["x"]');
dioxus.send((await tab.head()).ok.told);"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], Value::Null);
    assert_eq!(sent[1], json!(r#"["x"]"#));
}

/// A newer Dust opening the database in another tab, to change how the
/// history is kept, has this tab let go of it: this one then writes
/// nothing, and says the page has to be reloaded. A tab of this build
/// opened after that reads the database as the newer Dust left it, and
/// writes nothing to it.
#[test]
fn a_versionchange_closes_the_database() {
    let Some(sent) = on_page(
        r#"const tab = glue();
await tab.open();
await tab.apply(change({ put: [[name(1), "a"]], layout: { write: LAYOUT, over: true, expect: null } }));
dioxus.send(fakeIdb.upgrade("dust_history", 2));
dioxus.send(await tab.apply(change({ put: [[name(2), "b"]] })));
dioxus.send(await tab.list());
const late = glue();
dioxus.send(await late.open());
dioxus.send(await late.page(null, 10));
dioxus.send(await late.apply(change({ put: [[name(2), "b"]] })));
dioxus.send(Object.keys(fakeIdb.contents("dust_history", "sessions")));"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!(true), "let go of");
    assert_eq!(sent[1]["why"], "replaced");
    assert_eq!(sent[2]["why"], "replaced");
    assert_eq!(sent[3], json!({ "ok": "newer" }));
    assert_eq!(
        sent[4],
        json!({ "ok": { "keys": ["00000000000000000001-0000000000000001"], "values": ["a"] } })
    );
    assert_eq!(sent[5]["why"], "newer");
    assert_eq!(sent[6], json!(["00000000000000000001-0000000000000001"]));
}

/// The browser clearing the site's data closes the database under the
/// page: the next thing asked of it opens it again, and the next change,
/// carrying every session the app holds that the database no longer does,
/// fills it again, how the history is kept and all.
#[test]
fn an_evicted_database_is_reopened_and_refilled() {
    let Some(sent) = on_page(
        r#"const [tab, other] = [glue(), glue()];
await Promise.all([tab.open(), other.open()]);
await tab.apply(change({ put: [[name(1), "a"]], layout: { write: LAYOUT, over: true, expect: null } }));
fakeIdb.evict("dust_history");
await settle();
dioxus.send(tab.news());
dioxus.send(await tab.list());
dioxus.send(await tab.apply(change({
  put: [[name(1), "a"], [name(2), "b"]],
  layout: { write: LAYOUT, over: false, expect: null },
})));
dioxus.send(await other.list());
dioxus.send(fakeIdb.contents("dust_history", "meta"));
// Written where none is held only: one held stays as it is.
await tab.apply(change({ layout: { write: "{}", over: false, expect: null } }));
dioxus.send((await other.head()).ok.layout);"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!(1), "the app is told to look again");
    assert_eq!(sent[1], json!({ "ok": { "sessions": [], "deleted": [] } }));
    assert_eq!(
        sent[2],
        json!({ "ok": ["00000000000000000001-0000000000000001", "00000000000000000002-0000000000000001"] })
    );
    assert_eq!(
        sent[3]["ok"]["sessions"],
        json!([
            "00000000000000000001-0000000000000001",
            "00000000000000000002-0000000000000001"
        ])
    );
    assert_eq!(
        sent[4],
        json!({ "layout": r#"{"layout":1,"compatible":1,"migratedAt":5}"# })
    );
    assert_eq!(sent[5], r#"{"layout":1,"compatible":1,"migratedAt":5}"#);
}

/// No record is ever written over: one whose name other bytes have goes
/// under the next name, the same bytes again write nothing. One deleted by
/// hand stays deleted when stored again, unless the change undoes that, and
/// its tombstone, held already, stays as it was.
#[test]
fn a_record_is_never_written_over_nor_one_deleted_brought_back() {
    let Some(sent) = on_page(
        r#"const tab = glue();
await tab.open();
const sessions = () => fakeIdb.contents("dust_history", "sessions");
await tab.apply(change({ put: [[name(1), "a"]] }));
await tab.apply(change({ put: [[name(1), "a"]] }));
await tab.apply(change({ put: [[name(1), "other"]] }));
dioxus.send(sessions());
await tab.apply(change({ forget: [[name(1), "first"]] }));
await tab.apply(change({ forget: [[name(1), "second"]] }));
await tab.apply(change({ put: [[name(1), "a"]] }));
dioxus.send(sessions());
dioxus.send(fakeIdb.contents("dust_history", "deleted"));
await tab.apply(change({ put: [[name(1), "a"]], unforget: [name(1)] }));
dioxus.send(sessions());
dioxus.send(fakeIdb.contents("dust_history", "deleted"));"#,
    ) else {
        return;
    };
    let both = json!({
        "00000000000000000001-0000000000000001": "a",
        "00000000000000000001-0000000000000002": "other",
    });
    assert_eq!(sent[0], both, "beside it, not over it");
    assert_eq!(
        sent[1],
        json!({ "00000000000000000001-0000000000000002": "other" }),
        "still deleted"
    );
    assert_eq!(
        sent[2],
        json!({ "00000000000000000001-0000000000000001": "first" }),
        "the first tombstone kept"
    );
    assert_eq!(sent[3], both, "put back");
    assert_eq!(sent[4], json!({}));
}

/// A change's deletions are made after its records, as the files make
/// them, however each record went: added, there already, kept out as one
/// deleted, or moved to the next name, as the change's answer says, name by
/// name. A change that stores a record and
/// deletes it leaves no record of it, and its tombstone. The deletion was
/// asked for at once, and the record, added once the answers it waited on
/// came, stayed beside its tombstone.
#[test]
fn a_change_deletes_after_it_stores() {
    let Some(sent) = on_page(
        r#"const tab = glue();
await tab.open();
fakeIdb.plant("dust_history", "sessions", name(3), "c");
fakeIdb.plant("dust_history", "sessions", name(4), "other");
fakeIdb.plant("dust_history", "deleted", name(5), "old");
dioxus.send(await tab.apply(change({
  put: [[name(1), "a"], [name(3), "c"], [name(4), "d"], [name(5), "e"]],
  forget: [[name(1), "gone"]],
})));
dioxus.send(fakeIdb.contents("dust_history", "sessions"));
dioxus.send(fakeIdb.contents("dust_history", "deleted"));
// With no record to store, the deletion is made at once.
dioxus.send(await tab.apply(change({ forget: [[name(3), "gone too"]] })));
dioxus.send(fakeIdb.contents("dust_history", "sessions"));"#,
    ) else {
        return;
    };
    // Where each went: added, there already, moved to the next name, or
    // kept out as one deleted.
    assert_eq!(
        sent[0],
        json!({ "ok": ["00000000000000000001-0000000000000001", "00000000000000000003-0000000000000001", "00000000000000000004-0000000000000002", null] })
    );
    assert_eq!(
        sent[1],
        json!({
            "00000000000000000003-0000000000000001": "c",
            "00000000000000000004-0000000000000001": "other",
            "00000000000000000004-0000000000000002": "d",
        }),
        "no record of the one deleted"
    );
    assert_eq!(
        sent[2],
        json!({
            "00000000000000000001-0000000000000001": "gone",
            "00000000000000000005-0000000000000001": "old",
        })
    );
    assert_eq!(sent[3], json!({ "ok": [] }));
    assert_eq!(
        sent[4],
        json!({
            "00000000000000000004-0000000000000001": "other",
            "00000000000000000004-0000000000000002": "d",
        })
    );
}

/// Every other tab hears of a change once it is made: on the channel at
/// once, and from the stamp it leaves, which a tab asleep reads as it
/// wakes. What only notes what was told is no change to the history.
#[test]
fn another_tab_hears_of_each_change() {
    let Some(sent) = on_page(
        r#"const [tab, other] = [glue(), glue()];
await Promise.all([tab.open(), other.open()]);
let heard = false;
other.nextNews().then(() => { heard = true; });
const stamp = other.stamp();
await tab.apply(change({ put: [[name(1), "a"]] }));
await settle();
dioxus.send([heard, other.news(), tab.news(), other.stamp() !== stamp, other.stamp() !== null]);
const after = other.stamp();
await tab.apply(change({ meta: [["told", "[]"]] }));
await settle();
dioxus.send([other.news(), other.stamp() === after]);"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!([true, 1, 0, true, true]));
    assert_eq!(sent[1], json!([1, true]), "nothing said of a note");
}

/// A layout a newer Dust wrote, or one that does not read, lets nothing be
/// written to the history, but a layout written over it.
#[test]
fn a_layout_this_build_cannot_write_to_lets_nothing_be_written() {
    let Some(sent) = on_page(
        r#"const tab = glue();
await tab.open();
const put = change({ put: [[name(1), "a"]] });
fakeIdb.plant("dust_history", "meta", "layout", '{"layout":3,"compatible":2}');
dioxus.send(await tab.apply(put));
fakeIdb.plant("dust_history", "meta", "layout", "{ damaged");
dioxus.send(await tab.apply(put));
dioxus.send(await tab.apply(change({ put: [[name(1), "a"]], layout: { write: LAYOUT, over: true, expect: "{ damaged" } })));
dioxus.send(await tab.apply(put));
dioxus.send(Object.keys(fakeIdb.contents("dust_history", "sessions")));"#,
    ) else {
        return;
    };
    assert_eq!(sent[0]["why"], "newer");
    assert_eq!(sent[1]["why"], "unread");
    assert_eq!(
        sent[2],
        json!({ "ok": ["00000000000000000001-0000000000000001"] })
    );
    assert_eq!(
        sent[3],
        json!({ "ok": ["00000000000000000001-0000000000000001"] }),
        "there already"
    );
    assert_eq!(sent[4], json!(["00000000000000000001-0000000000000001"]));
}

/// A database that will not open says why, and so does one that never
/// answers, after a while, or a browser with none; the page goes on
/// without it.
#[test]
fn a_database_that_will_not_open_says_why() {
    let Some(sent) = on_page(
        r#"fakeIdb.failOpen = "InvalidStateError";
dioxus.send(await glue().open());
fakeIdb.failOpen = null;
fakeIdb.hangOpen = true;
dioxus.send(await glue({ openWaitMs: 5 }).open());
fakeIdb.hangOpen = false;
const held = globalThis.indexedDB;
globalThis.indexedDB = undefined;
dioxus.send(await glue().open());
globalThis.indexedDB = held;
dioxus.send(await glue().open());"#,
    ) else {
        return;
    };
    assert_eq!(sent[0]["why"], "failed");
    assert_eq!(sent[0]["message"], "the browser would not open it");
    assert_eq!(
        sent[1],
        json!({ "why": "failed", "message": "it did not answer" })
    );
    assert_eq!(
        sent[2],
        json!({ "why": "failed", "message": "this browser has none" })
    );
    assert_eq!(sent[3], json!({ "ok": "open" }));
}

/// The history kept whole is measured as the browser counts its
/// characters, without being read into the app: none where there is none.
#[test]
fn the_history_kept_whole_is_measured_where_it_lies() {
    let Some(sent) = on_page(
        r#"const tab = glue();
dioxus.send(tab.legacyLength("dust_sessions"));
localStorage.setItem("dust_sessions", '[{"date":"é"}]');
dioxus.send(tab.legacyLength("dust_sessions"));"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!(-1));
    assert_eq!(sent[1], json!(r#"[{"date":"é"}]"#.encode_utf16().count()));
}

/// One change, and the database it is made on, for the page script and for
/// the steps every other store makes it in ([`make_steps`]).
struct Case {
    sessions: BTreeMap<String, String>,
    deleted: BTreeMap<String, String>,
    put: Vec<Record>,
    forget: Vec<(RecordId, String)>,
    unforget: Vec<RecordId>,
}

/// The records a case is made of: each in a place of its own, so that the
/// names one is tried under never reach another's. Where they would, the
/// page script refuses the change whole, where those steps store each
/// record once ([`two_records_of_one_change_under_one_name_are_refused_whole`]).
fn records() -> Vec<Record> {
    [(10, 1), (20, 1_000), (30, 2_000)]
        .into_iter()
        .map(|(order, nonce)| Record {
            id: RecordId { order, nonce },
            json: format!(r#"{{"timestamp":{order},"date":"day {order}"}}"#),
        })
        .collect()
}

/// Case `seed`: each record stored already, or its name held by other
/// bytes, and some names past it, up to as many as a record is tried under
/// and one more, it after them or not, or not there; deleted or not; and a
/// change that stores, deletes and puts back some of them, each deleted
/// once, as a store is handed a change.
fn case(seed: u64) -> Case {
    let mut rng = SplitMix64::new(seed);
    let some = |rng: &mut SplitMix64| -> Vec<Record> {
        let density = [0.0, 0.3, 0.6, 1.0][rng.usize_in(0, 3)];
        records()
            .into_iter()
            .filter(|_| rng.f64() < density)
            .collect()
    };
    let (mut sessions, mut deleted) = (BTreeMap::new(), BTreeMap::new());
    for record in records() {
        match rng.usize_in(0, 5) {
            0 | 1 => {
                sessions.insert(record.id.name(), record.json.clone());
            }
            2 | 3 => {
                let held = match rng.usize_in(0, 9) {
                    0 => BUMPS,
                    1 => BUMPS + 1,
                    _ => rng.usize_in(1, 3) as u64,
                };
                let mut id = record.id;
                for n in 0..held {
                    sessions.insert(id.name(), format!("other {n}"));
                    id = id.bumped();
                }
                if rng.f64() < 0.3 {
                    sessions.insert(id.name(), record.json.clone());
                }
            }
            _ => {}
        }
        match rng.usize_in(0, 4) {
            0 => {
                deleted.insert(
                    record.id.name(),
                    r#"{"timestamp":1,"date":"d","at":5}"#.into(),
                );
            }
            1 => {
                deleted.insert(record.id.name(), "{}".into());
            }
            _ => {}
        }
    }
    let put = some(&mut rng);
    let forget = some(&mut rng)
        .into_iter()
        .map(|record| {
            (
                record.id,
                format!(
                    r#"{{"timestamp":1,"date":"d","at":{}}}"#,
                    rng.usize_in(6, 9)
                ),
            )
        })
        .collect();
    let unforget = some(&mut rng).into_iter().map(|record| record.id).collect();
    Case {
        sessions,
        deleted,
        put,
        forget,
        unforget,
    }
}

/// What a store that makes the change step by step holds after it, and
/// where each record went, by name, none for one left deleted: none where
/// it refuses the change, and so keeps nothing of it, as the database keeps
/// nothing of a change it refuses.
fn made_steps(case: &Case) -> Option<(Stores, Vec<Option<String>>)> {
    let mut shelves = Shelves::default();
    shelves.sessions = case.sessions.clone();
    shelves.deleted = case
        .deleted
        .iter()
        .map(|(name, held)| (name.clone(), Some(held.clone())))
        .collect();
    let forget: Vec<(RecordId, &str)> = case
        .forget
        .iter()
        .map(|(id, held)| (*id, held.as_str()))
        .collect();
    let (placed, made) = make_steps(
        &Making(RefCell::new(&mut shelves)),
        &case.put,
        &forget,
        &case.unforget,
    );
    let names = placed
        .into_iter()
        .map(|id| id.map(RecordId::name))
        .collect();
    let deleted = shelves
        .deleted
        .into_iter()
        .map(|(name, held)| (name, held.expect("text")))
        .collect();
    made.ok().map(|()| ((shelves.sessions, deleted), names))
}

/// What the records and the tombstones of a database hold, by name.
type Stores = (BTreeMap<String, String>, BTreeMap<String, String>);

/// The page script makes a change as every other store makes it, step by
/// step ([`make_steps`]): deletions undone, then the records, each left out
/// where it is deleted and not put back, kept where it is put back and its
/// record stayed, stored where the same bytes are not there already under
/// its name or one of the names past it, then the deletions, a tombstone
/// held already kept as it is. On seeded changes over records stored, held
/// by other bytes under as many names as a record is tried under and one
/// more, deleted or not, what the database holds after each, or that it
/// refused it, is what those steps make of it, and so is where it says
/// each record went: for every change whose records each have a place of
/// their own, as every change the app makes has ([`records`]).
#[test]
fn the_page_script_makes_a_change_as_every_other_store_makes_it() {
    const CASES: u64 = 2_000;
    let cases: Vec<Case> = (0..CASES).map(case).collect();
    let pairs = |held: &BTreeMap<String, String>| -> Value {
        held.iter()
            .map(|(name, json)| json!([name, json]))
            .collect()
    };
    let asked: Vec<Value> = cases
        .iter()
        .map(|case| {
            json!({
                "sessions": pairs(&case.sessions),
                "deleted": pairs(&case.deleted),
                "put": case.put.iter().map(|record| json!([record.id.name(), record.json])).collect::<Vec<_>>(),
                "forget": case.forget.iter().map(|(id, held)| json!([id.name(), held])).collect::<Vec<_>>(),
                "unforget": case.unforget.iter().map(|id| id.name()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let Some(sent) = on_page(&format!(
        r#"const cases = {};
for (const asked of cases) {{
  fakeIdb.evict("dust_history");
  const tab = glue();
  await tab.open();
  for (const [name, json] of asked.sessions) fakeIdb.plant("dust_history", "sessions", name, json);
  for (const [name, held] of asked.deleted) fakeIdb.plant("dust_history", "deleted", name, held);
  const answer = await tab.apply(change({{ put: asked.put, forget: asked.forget, unforget: asked.unforget }}));
  dioxus.send({{
    made: answer.why === undefined,
    names: answer.ok === undefined ? null : answer.ok,
    sessions: fakeIdb.contents("dust_history", "sessions"),
    deleted: fakeIdb.contents("dust_history", "deleted"),
  }});
}}"#,
        Value::Array(asked)
    )) else {
        return;
    };
    assert_eq!(sent.len(), cases.len(), "an answer to each");
    let held = |of: &Value| -> BTreeMap<String, String> {
        of.as_object()
            .expect("a store's contents")
            .iter()
            .map(|(name, json)| (name.clone(), json.as_str().expect("text").to_string()))
            .collect()
    };
    let mut refused = 0;
    for (seed, (case, sent)) in cases.iter().zip(&sent).enumerate() {
        let after = (held(&sent["sessions"]), held(&sent["deleted"]));
        match made_steps(case) {
            Some((made, names)) => {
                assert_eq!(sent["made"], json!(true), "case {seed}: {sent}");
                assert_eq!(after, made, "case {seed}");
                assert_eq!(sent["names"], json!(names), "case {seed}: where each went");
            }
            None => {
                refused += 1;
                assert_eq!(sent["made"], json!(false), "case {seed}: {sent}");
                assert_eq!(
                    after,
                    (case.sessions.clone(), case.deleted.clone()),
                    "case {seed}: kept nothing"
                );
            }
        }
    }
    assert!(
        refused > 0 && refused < cases.len(),
        "{refused} of {} refused",
        cases.len()
    );
}

/// Two records of one change that would go under one name, as one record
/// twice, or one that other bytes bump onto the name of the next, are
/// refused whole by the page script, which looks for each record's place
/// while it looks for every other's: both are added under that name, and
/// the database will not hold two. The steps every other store makes
/// ([`make_steps`]) place one record after the other, and store each once.
/// No change the app makes holds such records: each is named by its time
/// and the hash of what it holds, and each change holds a record once.
#[test]
fn two_records_of_one_change_under_one_name_are_refused_whole() {
    let Some(sent) = on_page(
        r#"const tab = glue();
await tab.open();
fakeIdb.plant("dust_history", "sessions", name(5, 1), "other");
dioxus.send(await tab.apply(change({ put: [[name(5, 1), "a"], [name(5, 2), "b"]] })));
dioxus.send(await tab.apply(change({ put: [[name(6), "c"], [name(6), "c"]] })));
dioxus.send(fakeIdb.contents("dust_history", "sessions"));"#,
    ) else {
        return;
    };
    for refused in &sent[..2] {
        assert_eq!(refused["why"], json!("failed"), "{refused}");
        assert_eq!(refused["name"], json!("ConstraintError"), "{refused}");
    }
    let id = |order, nonce| RecordId { order, nonce };
    let other = BTreeMap::from([(id(5, 1).name(), "other".to_string())]);
    let held: BTreeMap<String, String> =
        serde_json::from_value(sent[2].clone()).expect("the records");
    assert_eq!(held, other, "nothing of either change");
    // Step by step, each is stored once.
    let record = |at: RecordId, json: &str| Record {
        id: at,
        json: json.into(),
    };
    let mut shelves = Shelves::default();
    shelves.sessions = other;
    let put = [record(id(5, 1), "a"), record(id(5, 2), "b")];
    let (placed, made) = make_steps(&Making(RefCell::new(&mut shelves)), &put, &[], &[]);
    assert!(made.is_ok());
    assert_eq!(placed, [Some(id(5, 2)), Some(id(5, 3))]);
    let twice = [record(id(6, 1), "c"), record(id(6, 1), "c")];
    let (placed, made) = make_steps(&Making(RefCell::new(&mut shelves)), &twice, &[], &[]);
    assert!(made.is_ok());
    assert_eq!(placed, [Some(id(6, 1)), Some(id(6, 1))]);
    assert_eq!(
        shelves.sessions.len(),
        4,
        "each once, beside the other bytes"
    );
}
