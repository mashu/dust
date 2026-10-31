//! The history's page script ([`IDB_JS`]) run on a page, in Node, on a
//! browser's database made up in memory (`src/fake_idb.mjs`): what the
//! browser's own database makes of each thing the history asks of it.

use serde_json::{Value, json};

use super::IDB_JS;
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
    assert_eq!(sent[0], json!({ "ok": null }));
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
    assert_eq!(sent[2], json!({ "ok": null }));
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
dioxus.send(await tab.deleted([name(10, 2), name(30)]));
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
    assert_eq!(sent[4], json!({ "ok": [c] }));
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
    assert!(answers.contains(&json!({ "ok": null })), "{answers:?}");
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
    let answers = sent[4].as_array().expect("both answered");
    assert!(answers.iter().any(|answer| answer["why"] == "raced"));
    assert_eq!(sent[5], r#"{"layout":1,"compatible":1,"migratedAt":6}"#);
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
    assert_eq!(sent[2], json!({ "ok": null }));
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
    assert_eq!(sent[2], json!({ "ok": null }));
    assert_eq!(sent[3], json!({ "ok": null }));
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
