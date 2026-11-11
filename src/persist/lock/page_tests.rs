//! The tabs' page script ([`TABS_JS`]) run on a page, in Node, on Web Locks
//! made up in memory (`src/fake_idb.mjs`): what a browser's own makes of each
//! lock the app tries.

use serde_json::{Value, json};

use super::TABS_JS;
use crate::testing::{PageStep, on_fake_page};

/// What `script` sends the app, run to its end on a page where
/// `glue(options)` runs the page script for a tab, `letGo(release)` lets go
/// of a lock it holds, and `settle()` lets what the page has queued happen.
/// None on a machine with no Node.
fn on_page(script: &str) -> Option<Vec<Value>> {
    let glue = serde_json::to_string(TABS_JS).expect("the script, as a string");
    let script = format!(
        r#"const glue = (options) => new Function("options", {glue})(options);
const letGo = (release) => typeof release === "function" && release();
const settle = async () => {{
  for (let i = 0; i < 20; i += 1) await new Promise((done) => setImmediate(done));
}};
{script}"#
    );
    let heard = on_fake_page(&[PageStep::wait(script)])?;
    Some(heard.into_iter().next().expect("a step").sent)
}

/// A Web Lock is held by one tab at a time. Tried while another tab holds
/// it, or waits for it, it is busy; once let go of, or once the tab that
/// held it is closed, the next tab to try it holds it. Each name is a lock
/// of its own.
#[test]
fn a_web_lock_is_held_by_one_tab_at_a_time() {
    let Some(sent) = on_page(
        r#"const one = glue();
const otherTab = fakeLocks.tab();
const two = glue({ locks: otherTab });
const release = await one.tryLock("dust_sync_run");
dioxus.send(typeof release);
dioxus.send(await two.tryLock("dust_sync_run"));
const other = await two.tryLock("dust_other");
dioxus.send(typeof other);
dioxus.send(await navigator.locks.query());
letGo(release);
await settle();
const now = await two.tryLock("dust_sync_run");
dioxus.send(typeof now);
dioxus.send(await one.tryLock("dust_sync_run"));
// Two's tab is closed under it: what it held is let go of.
fakeLocks.close(otherTab);
const after = await one.tryLock("dust_sync_run");
dioxus.send(typeof after);
// A tab waiting for it, as one that will wait to sync, keeps it busy.
const waiting = fakeLocks.tab();
waiting.request("dust_sync_run", () => {});
dioxus.send(await glue({ locks: fakeLocks.tab() }).tryLock("dust_sync_run"));
letGo(after);
await settle();
dioxus.send(await navigator.locks.query());"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!("function"), "the first to try it holds it");
    assert_eq!(sent[1], json!(false), "busy, as another tab holds it");
    assert_eq!(sent[2], json!("function"), "another name is another lock");
    assert_eq!(
        sent[3],
        json!({
            "held": [
                { "name": "dust_sync_run", "mode": "exclusive", "clientId": "tab-1" },
                { "name": "dust_other", "mode": "exclusive", "clientId": "tab-2" },
            ],
            "pending": [],
        }),
        "held as long as it is, and nothing waits for it: tried, it is never waited for"
    );
    assert_eq!(sent[4], json!("function"), "free once let go of");
    assert_eq!(sent[5], json!(false));
    assert_eq!(sent[6], json!("function"), "let go of as its tab closed");
    assert_eq!(sent[7], json!(false), "busy while another tab waits for it");
    assert_eq!(
        sent[8],
        json!({ "held": [], "pending": [] }),
        "the tab that waited had it, and let go of it"
    );
}

/// A browser without Web Locks, as Safari before 15.4, takes none, and so
/// does one that will not say: the app goes on without them, as before.
#[test]
fn a_browser_without_web_locks_takes_none() {
    let Some(sent) = on_page(
        r#"const page = navigator.locks;
navigator.locks = undefined;
dioxus.send(await glue().tryLock("dust_sync_run"));
navigator.locks = page;
const refusing = { request: () => Promise.reject(new DOMException("no", "SecurityError")) };
dioxus.send(await glue({ locks: refusing }).tryLock("dust_sync_run"));
const throwing = { request: () => { throw new TypeError("no"); } };
dioxus.send(await glue({ locks: throwing }).tryLock("dust_sync_run"));"#,
    ) else {
        return;
    };
    assert_eq!(sent, [json!(null), json!(null), json!(null)]);
}

/// A lock granted once the app has stopped waiting for it, as where what
/// waited on it went before the browser answered, is let go of at once,
/// rather than held by the tab until it closes.
#[test]
fn a_web_lock_no_one_waits_for_is_let_go_of() {
    let Some(sent) = on_page(
        r#"const one = glue();
const two = glue({ locks: fakeLocks.tab() });
one.dropped(one.tryLock("dust_sync_run"));
await settle();
const release = await two.tryLock("dust_sync_run");
dioxus.send(typeof release);
// Busy, and so nothing to let go of.
two.dropped(one.tryLock("dust_sync_run"));
await settle();
dioxus.send(await two.tryLock("dust_sync_run"));"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!("function"), "let go of, as no one waited");
    assert_eq!(sent[1], json!(false), "still held by the tab that has it");
}

/// Another tab's word on what sync keeps is heard: a `storage` event for
/// `dust_sync`, which the browser fires in every other tab of the site as
/// one writes it, ends the wait, and so does one for every key at once, as
/// `localStorage.clear()` fires; one for another key does not. A page with
/// no such events never ends it.
#[test]
fn another_tabs_word_on_what_sync_keeps_is_heard() {
    let Some(sent) = on_page(
        r#"const target = new EventTarget();
const tab = glue({ target });
const told = (key) => {
  const event = new Event("storage");
  Object.defineProperty(event, "key", { value: key });
  target.dispatchEvent(event);
};
let heard = 0;
tab.heard().then(() => { heard += 1; });
told("dust_settings");
await settle();
dioxus.send(heard);
told("dust_sync");
await settle();
dioxus.send(heard);
tab.heard().then(() => { heard += 1; });
told(null);
await settle();
dioxus.send(heard);
let never = true;
glue({ target: {} }).heard().then(() => { never = false; });
await settle();
dioxus.send(never);"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!(0), "another key's word is none of sync's");
    assert_eq!(sent[1], json!(1), "heard");
    assert_eq!(sent[2], json!(2), "every key cleared, this one among them");
    assert_eq!(sent[3], json!(true), "no word where no events come");
}
