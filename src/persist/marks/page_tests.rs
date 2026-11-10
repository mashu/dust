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
const released = ours.release(["dust_open_session_a"], ["dust_open_session_c"], own);
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

/// A mark carried by a tab whose own mark will not take the write that
/// carries it, as page storage with no room left takes none, stays where it
/// is, held by the tab, until a write of the tab's own mark goes in: what it
/// carried is in no other mark meanwhile. It used to go once the tab's own
/// mark had been placed, whatever became of the writes after, and the
/// session it carried was then in no mark at all.
#[test]
fn a_mark_carried_stays_while_the_tabs_own_mark_will_not_take_a_write() {
    let Some(sent) = on_page(
        r#"const ran = fakeLocks.tab();
await leftBy(ran, "dust_open_session_a");
fakeLocks.close(ran);
const ours = marks(fakeLocks.tab());
const own = ours.hold("dust_open_session_b");
own.write('{"startedAt":9,"run":false}');
await settle();
dioxus.send((await ours.take()).map(([key]) => key));
// No room left for the write that carries it.
localStorage.setItem = () => {
  throw new DOMException("no room left", "QuotaExceededError");
};
// As web.rs calls it, which lets a throw be.
try {
  own.write('{"startedAt":9,"run":false,"unstored":["a"]}');
} catch (error) {}
const released = ours.release(["dust_open_session_a"], [], own);
await settle();
dioxus.send([localStorage.getItem("dust_open_session_a"), localStorage.getItem("dust_open_session_b")]);
dioxus.send((await marks(fakeLocks.tab()).take()).map(([key]) => key));
// Room again: the next write goes in, and holds it.
delete localStorage.setItem;
own.write('{"startedAt":9,"run":false,"unstored":["a","b"]}');
await released;
dioxus.send([localStorage.getItem("dust_open_session_a"), localStorage.getItem("dust_open_session_b")]);"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!(["dust_open_session_a"]), "taken");
    assert_eq!(
        sent[1],
        json!([
            r#"{"startedAt":7,"run":false}"#,
            r#"{"startedAt":9,"run":false}"#
        ]),
        "kept while the tab's own mark holds what it held before"
    );
    assert_eq!(sent[2], json!([]), "held by the tab meanwhile");
    assert_eq!(
        sent[3],
        json!([null, r#"{"startedAt":9,"run":false,"unstored":["a","b"]}"#]),
        "gone once a write goes in"
    );
}

/// A tab's own mark that has no room as its lock is granted goes in with
/// the next write that has, and a mark it carries goes only then. The
/// mark used to stay out of the page storage for good, every write after
/// held back for a placing that never came.
#[test]
fn a_tabs_own_mark_with_no_room_as_it_is_placed_goes_in_with_the_next_write() {
    let Some(sent) = on_page(
        r#"const ran = fakeLocks.tab();
await leftBy(ran, "dust_open_session_a");
fakeLocks.close(ran);
const ours = marks(fakeLocks.tab());
dioxus.send((await ours.take()).map(([key]) => key));
localStorage.setItem = () => {
  throw new DOMException("no room left", "QuotaExceededError");
};
const own = ours.hold("dust_open_session_b");
own.write('{"startedAt":9,"run":false,"unstored":["a"]}');
const released = ours.release(["dust_open_session_a"], [], own);
await settle();
dioxus.send([localStorage.getItem("dust_open_session_a"), localStorage.getItem("dust_open_session_b")]);
delete localStorage.setItem;
own.write('{"startedAt":9,"run":false,"unstored":["a","b"]}');
await released;
dioxus.send([localStorage.getItem("dust_open_session_a"), localStorage.getItem("dust_open_session_b")]);"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!(["dust_open_session_a"]), "taken");
    assert_eq!(
        sent[1],
        json!([r#"{"startedAt":7,"run":false}"#, null]),
        "kept while the tab's own mark is in no place"
    );
    assert_eq!(
        sent[2],
        json!([null, r#"{"startedAt":9,"run":false,"unstored":["a","b"]}"#]),
        "in place with the next write, and the one it carried gone"
    );
}

/// A letting go is over once the tab's own mark has been tried with the
/// text that carries what it lets go of, and not before: where that went
/// in, the marks carried are gone by then; where it did not, they stay,
/// held, and go with the next write that goes in, while the app goes on to
/// keep what they carried. It used to be over only once they had gone,
/// which a page storage with no room left might never let them.
#[test]
fn a_letting_go_is_over_once_the_tabs_own_mark_was_tried() {
    let Some(sent) = on_page(
        r#"const ran = fakeLocks.tab();
await leftBy(ran, "dust_open_session_a");
fakeLocks.close(ran);
const ours = marks(fakeLocks.tab());
await ours.take();
// Another tab looks for marks, and holds the look a while.
const looking = fakeLocks.tab();
let looked;
looking.request("dust_open_session_look", () => new Promise((done) => { looked = done; }));
await settle();
const own = ours.hold("dust_open_session_b");
own.write('{"startedAt":9,"run":false,"unstored":["a"]}');
let over = false;
ours.release(["dust_open_session_a"], [], own).then(() => {
  over = true;
});
await settle();
dioxus.send(over);
localStorage.setItem = () => {
  throw new DOMException("no room left", "QuotaExceededError");
};
looked();
await settle();
dioxus.send([over, localStorage.getItem("dust_open_session_a"), localStorage.getItem("dust_open_session_b")]);
delete localStorage.setItem;
own.write('{"startedAt":9,"run":false,"unstored":["a","b"]}');
await settle();
dioxus.send([localStorage.getItem("dust_open_session_a"), localStorage.getItem("dust_open_session_b")]);"#,
    ) else {
        return;
    };
    assert_eq!(sent[0], json!(false), "not over before the mark is tried");
    assert_eq!(
        sent[1],
        json!([true, r#"{"startedAt":7,"run":false}"#, null]),
        "over once it was tried, the mark carried kept"
    );
    assert_eq!(
        sent[2],
        json!([null, r#"{"startedAt":9,"run":false,"unstored":["a","b"]}"#]),
        "gone with the write that went in"
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
await ours.release(["dust_open_session_a"], [], null);
dioxus.send(localStorage.getItem("dust_open_session_a"));
const own = ours.hold("dust_open_session_b");
own.write('{"startedAt":9,"run":false}');
own.write(null);
await ours.release(["dust_open_session_c"], [], own);
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
