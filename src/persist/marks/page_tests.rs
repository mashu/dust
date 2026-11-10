//! The marks' page script ([`MARKS_JS`]) run on a page, in Node, on the Web
//! Locks and page storage made up in memory (`src/fake_idb.mjs`): what a
//! tab makes of its own mark, and of those of tabs that have gone.

use serde_json::{Value, json};

use super::MARKS_JS;
use crate::testing::{PageStep, on_fake_page};

/// What `script` sends the app, run to its end on a page where
/// `marks(locks)` runs the marks' script for a tab whose Web Locks are
/// `locks`, `leftBy(locks)` leaves the mark of a tab that ran a session on
/// those locks, as a tab of any build leaves it, and `settle()` lets what
/// the page has queued happen. None on a machine with no Node.
fn on_page(script: &str) -> Option<Vec<Value>> {
    let glue = serde_json::to_string(MARKS_JS).expect("the script, as a string");
    let script = format!(
        r#"const marks = (locks) =>
  new Function("prefix", "options", {glue})("dust_open_session_", {{ locks }});
const settle = async () => {{
  for (let i = 0; i < 20; i += 1) await new Promise((done) => setImmediate(done));
}};
const MARK = '{{"startedAt":7,"run":false}}';
const leftBy = async (locks, name) => {{
  locks.request(name, () => new Promise(() => {{}}));
  await settle();
  localStorage.setItem(name, MARK);
}};
{script}"#
    );
    let heard = on_fake_page(&[PageStep::wait(script)])?;
    Some(heard.into_iter().next().expect("a step").sent)
}

/// The mark of a tab gone is taken and held by the tab that took it, and
/// left where it is: a tab that goes before it lets go of it leaves it to
/// the next to look, which takes it again. It used to be taken out of the
/// page storage as it was taken, and what it carried went with a tab that
/// went before it held it anywhere else.
#[test]
fn a_mark_taken_by_a_tab_that_goes_before_letting_go_is_found_again() {
    let Some(sent) = on_page(
        r#"const ran = fakeLocks.tab();
await leftBy(ran, "dust_open_session_a");
fakeLocks.close(ran);
const tookTab = fakeLocks.tab();
dioxus.send(await marks(tookTab).take());
// Another tab looks while it holds it.
dioxus.send(await marks(fakeLocks.tab()).take());
// The tab that took it goes before it holds what it carried.
fakeLocks.close(tookTab);
dioxus.send(await marks(fakeLocks.tab()).take());"#,
    ) else {
        return;
    };
    let taken = json!([["dust_open_session_a", r#"{"startedAt":7,"run":false}"#]]);
    assert_eq!(
        sent[2], taken,
        "taken again, once the tab that took it went"
    );
    assert_eq!(sent[0], taken, "taken, by its name");
    assert_eq!(sent[1], json!([]), "held by the tab that took it");
}

/// A mark taken, and carried by the tab that took it, goes from the page
/// storage only once that tab's own mark is in place, holding what it
/// carried: until then the tab's own mark waits for the look another tab
/// makes. One let go of as it was, as a tab's own mark is, stays for the
/// next look.
#[test]
fn a_mark_carried_goes_once_the_tabs_own_mark_is_in_place() {
    let Some(sent) = on_page(
        r#"const ran = fakeLocks.tab();
await leftBy(ran, "dust_open_session_a");
await leftBy(ran, "dust_open_session_c");
fakeLocks.close(ran);
const tab = fakeLocks.tab();
const ours = marks(tab);
dioxus.send((await ours.take()).map(([key]) => key));
// Another tab looks for marks, and holds the look a while.
const looking = fakeLocks.tab();
let looked;
looking.request("dust_open_session_look", () => new Promise((done) => { looked = done; }));
await settle();
const own = ours.hold("dust_open_session_b");
own.write('{"startedAt":9,"run":false,"over":true}');
const released = ours.release(["dust_open_session_a"], ["dust_open_session_c"], own.placed);
await settle();
dioxus.send([localStorage.getItem("dust_open_session_a"), localStorage.getItem("dust_open_session_b")]);
looked();
await released;
dioxus.send([localStorage.getItem("dust_open_session_a"), localStorage.getItem("dust_open_session_b")]);
dioxus.send((await marks(fakeLocks.tab()).take()).map(([key]) => key));
const query = await navigator.locks.query();
dioxus.send(query.held.map((lock) => lock.name).sort());"#,
    ) else {
        return;
    };
    assert_eq!(
        sent[0],
        json!(["dust_open_session_a", "dust_open_session_c"]),
        "both taken"
    );
    assert_eq!(
        sent[1],
        json!([r#"{"startedAt":7,"run":false}"#, null]),
        "kept while the tab's own is not in place"
    );
    assert_eq!(
        sent[2],
        json!([null, r#"{"startedAt":9,"run":false,"over":true}"#]),
        "gone once it is"
    );
    assert_eq!(
        sent[3],
        json!(["dust_open_session_c"]),
        "the one let go of as it was, for the next look"
    );
    assert_eq!(
        sent[4],
        json!(["dust_open_session_b", "dust_open_session_c"]),
        "the tab's own, and the one the last look took: none let go of is held"
    );
}

/// A tab with no mark of its own lets go of a mark it carried at once, as
/// what it carried is in the history: as it does where its own mark has
/// gone, as a mark goes once there is nothing left for it to carry.
#[test]
fn a_mark_carried_by_a_tab_with_no_mark_of_its_own_goes_at_once() {
    let Some(sent) = on_page(
        r#"const ran = fakeLocks.tab();
await leftBy(ran, "dust_open_session_a");
await leftBy(ran, "dust_open_session_c");
fakeLocks.close(ran);
const ours = marks(fakeLocks.tab());
await ours.take();
await ours.release(["dust_open_session_a"], [], Promise.resolve());
dioxus.send(localStorage.getItem("dust_open_session_a"));
const own = ours.hold("dust_open_session_b");
own.write('{"startedAt":9,"run":false}');
own.write(null);
await ours.release(["dust_open_session_c"], [], own.placed);
dioxus.send([localStorage.getItem("dust_open_session_b"), localStorage.getItem("dust_open_session_c")]);
// The tab's own lock, granted after its mark went, is let go of as it is.
await settle();
const query = await navigator.locks.query();
dioxus.send(query.held.map((lock) => lock.name));"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], Value::Null, "gone at once");
    assert_eq!(
        sent[1],
        json!([null, null]),
        "gone once the tab's own mark went"
    );
    assert_eq!(sent[2], json!([]), "nothing held");
}

/// A mark a tab holds, having taken it, is left be by a tab of a build
/// before marks were held as they were taken: that build's look leaves
/// every mark whose lock is held, and this tab holds the lock of each it
/// takes. The look is the one builds 0.16.6 to 0.16.9 make.
#[test]
fn a_tab_of_an_older_build_leaves_a_mark_this_tab_took() {
    let Some(sent) = on_page(
        r#"const OLDER_TAKE = `if (!navigator.locks) return Promise.resolve([]);
        return navigator.locks.request(prefix + 'look', async () => {
            const state = await navigator.locks.query();
            const live = new Set([...state.held, ...state.pending].map((lock) => lock.name));
            const keys = [];
            for (let i = 0; i < localStorage.length; i++) {
                const key = localStorage.key(i);
                if (key && key.startsWith(prefix) && !live.has(key)) keys.push(key);
            }
            return keys.map((key) => {
                const mark = localStorage.getItem(key) ?? '';
                localStorage.removeItem(key);
                return mark;
            });
        }).catch(() => []);`;
const olderTake = () => new Function("prefix", OLDER_TAKE)("dust_open_session_");
const ran = fakeLocks.tab();
await leftBy(ran, "dust_open_session_a");
fakeLocks.close(ran);
const tab = fakeLocks.tab();
dioxus.send((await marks(tab).take()).map(([key]) => key));
dioxus.send(await olderTake());
dioxus.send(localStorage.getItem("dust_open_session_a"));
fakeLocks.close(tab);
dioxus.send(await olderTake());"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!(["dust_open_session_a"]));
    assert_eq!(sent[1], json!([]), "held: left be");
    assert_eq!(sent[2], json!(r#"{"startedAt":7,"run":false}"#));
    assert_eq!(
        sent[3],
        json!([r#"{"startedAt":7,"run":false}"#]),
        "taken by the older build once the tab that held it went"
    );
}
