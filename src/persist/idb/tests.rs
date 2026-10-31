//! The history in a browser's database ([`BrowserHistory`]), on a database
//! made up in memory, as the page script makes it (its own tests run it on
//! a page, in `page_tests.rs`), with the page storage beside it.

use std::future::Future;
use std::ops::Bound;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use super::*;
use crate::persist::tests::session;

/// What a future made of answers given at once comes to.
fn done<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("it waits, on nothing that answers"),
    }
}

const KEYS: BlobKeys = BlobKeys {
    history: "dust_sessions",
    deleted: "dust_deleted",
    aside: "dust_sessions_unreadable",
};

/// A browser's page storage, as far as the history goes, shared by every
/// tab: what it holds, and whether it can be read.
#[derive(Clone, Default)]
struct Page(Rc<PageHeld>);

#[derive(Default)]
struct PageHeld {
    items: RefCell<BTreeMap<String, String>>,
    unreadable: Cell<bool>,
}

impl Page {
    fn holding(sessions: &[SessionResult]) -> Self {
        let page = Self::default();
        page.keep(sessions);
        page
    }

    /// Keep `sessions` as the history kept whole, as an older build does.
    fn keep(&self, sessions: &[SessionResult]) {
        let raw = serde_json::to_string(sessions).expect("sessions write");
        self.set(KEYS.history, &raw);
    }

    fn held(&self, key: &str) -> Option<String> {
        self.0.items.borrow().get(key).cloned()
    }

    /// Remember `deleted` as an older build's deletions.
    fn deleted(&self, deleted: &[Deleted]) {
        let raw = serde_json::to_string(deleted).expect("deletions write");
        self.set(KEYS.deleted, &raw);
    }
}

impl Aside for Page {
    fn keys(&self) -> Result<Vec<String>, ()> {
        if self.0.unreadable.get() {
            return Err(());
        }
        Ok(self.0.items.borrow().keys().cloned().collect())
    }

    fn get(&self, key: &str) -> Result<Option<String>, ()> {
        if self.0.unreadable.get() {
            return Err(());
        }
        Ok(self.held(key))
    }

    fn set(&self, key: &str, raw: &str) -> bool {
        self.0.items.borrow_mut().insert(key.into(), raw.into());
        true
    }

    fn remove(&self, key: &str) {
        self.0.items.borrow_mut().remove(key);
    }
}

/// A browser's database, as far as the history goes: what every tab
/// shares.
#[derive(Default)]
struct Shelves {
    sessions: BTreeMap<String, String>,
    deleted: BTreeMap<String, String>,
    meta: BTreeMap<String, String>,
    /// How many changes were made, and how many other tabs told of.
    changes: u64,
    news: u64,
    /// No room is left for any change.
    full: bool,
    /// Nothing can be read or written, for the reason given.
    failing: Option<String>,
    /// What another tab does just before the next change is made here.
    first: Option<Before>,
}

/// What another tab does to the database before this one's next change.
type Before = Box<dyn FnOnce(&mut Shelves)>;

/// One tab's way to the database, and to the page storage beside it.
#[derive(Clone, Default)]
struct Fake {
    db: Rc<RefCell<Shelves>>,
    page: Page,
}

impl Fake {
    fn on(page: &Page) -> Self {
        Self {
            db: Rc::default(),
            page: page.clone(),
        }
    }

    fn shelves(&self) -> std::cell::RefMut<'_, Shelves> {
        self.db.borrow_mut()
    }

    fn failing(&self) -> Result<(), Refused> {
        match &self.db.borrow().failing {
            Some(why) => Err(Refused::Failed(why.clone())),
            None => Ok(()),
        }
    }

    /// What the records hold, by id.
    fn records(&self) -> BTreeMap<RecordId, String> {
        self.db
            .borrow()
            .sessions
            .iter()
            .map(|(name, json)| (RecordId::parse(name).expect("a name"), json.clone()))
            .collect()
    }

    fn changes(&self) -> u64 {
        self.db.borrow().changes
    }

    /// How the history is kept, as the database holds it.
    fn layout(&self) -> Option<Layout> {
        let raw = self.db.borrow().meta.get("layout").cloned()?;
        match Gate::of(Some(raw.as_bytes())) {
            Gate::Open(layout) => Some(layout),
            gate => panic!("{gate:?}"),
        }
    }

    /// Make `writes` on `shelves`, as the page script makes them; what is
    /// made of a change refused is put back by the caller.
    fn make(shelves: &mut Shelves, writes: Writes) -> Result<(), Refused> {
        let held = shelves.meta.get("layout").cloned();
        let over = matches!(writes.layout, Some(LayoutWrite::Over { .. }));
        if let Some(LayoutWrite::Over { expect, .. }) = &writes.layout
            && *expect != held
        {
            return Err(Refused::Raced);
        }
        match held.as_deref().map(|raw| Gate::of(Some(raw.as_bytes()))) {
            Some(Gate::Newer) => return Err(Refused::Newer),
            Some(Gate::Unread) if !over => return Err(Refused::Unread),
            _ => {}
        }
        if shelves.full {
            return Err(Refused::Full);
        }
        let wrote = !writes.put.is_empty()
            || !writes.forget.is_empty()
            || !writes.unforget.is_empty()
            || over;
        let undone: HashSet<&String> = writes.unforget.iter().collect();
        for name in &writes.unforget {
            shelves.deleted.remove(name);
        }
        for (name, json) in &writes.put {
            let undoing = undone.contains(name);
            if !undoing && shelves.deleted.contains_key(name) {
                continue;
            }
            let mut id = RecordId::parse(name).expect("a name");
            loop {
                match shelves.sessions.get(&id.name()) {
                    None => {
                        shelves.sessions.insert(id.name(), json.clone());
                        break;
                    }
                    Some(there) if there == json || (undoing && id.name() == *name) => break,
                    Some(_) => id = id.bumped(),
                }
            }
        }
        for (name, tombstone) in &writes.forget {
            shelves
                .deleted
                .entry(name.clone())
                .or_insert_with(|| tombstone.clone());
            shelves.sessions.remove(name);
        }
        for (key, value) in &writes.meta {
            shelves.meta.insert(key.clone(), value.clone());
        }
        match writes.layout {
            Some(LayoutWrite::Over { write, .. }) => {
                shelves.meta.insert("layout".into(), write);
            }
            Some(LayoutWrite::Missing(write)) => {
                shelves.meta.entry("layout".into()).or_insert(write);
            }
            None => {}
        }
        if wrote {
            shelves.changes += 1;
        }
        Ok(())
    }
}

fn answer<T: 'static>(value: T) -> Pending<T> {
    record::done(value)
}

impl Glue for Fake {
    fn head(&self) -> Pending<Result<Head, Refused>> {
        let shelves = self.db.borrow();
        answer(self.failing().map(|()| Head {
            layout: shelves.meta.get("layout").cloned(),
            told: shelves.meta.get("told").cloned(),
        }))
    }

    fn page(&self, after: Option<String>, count: usize) -> Pending<Result<Stored, Refused>> {
        let shelves = self.db.borrow();
        let from = after.map_or(Bound::Unbounded, Bound::Excluded);
        answer(self.failing().map(|()| {
            shelves
                .sessions
                .range((from, Bound::Unbounded))
                .take(count)
                .map(|(name, json)| (name.clone(), Some(json.clone())))
                .collect()
        }))
    }

    fn tombstones(&self) -> Pending<Result<Stored, Refused>> {
        let shelves = self.db.borrow();
        answer(self.failing().map(|()| {
            shelves
                .deleted
                .iter()
                .map(|(name, held)| (name.clone(), Some(held.clone())))
                .collect()
        }))
    }

    fn list(&self) -> Pending<Result<Listed, Refused>> {
        let shelves = self.db.borrow();
        answer(self.failing().map(|()| Listed {
            sessions: shelves.sessions.keys().cloned().collect(),
            deleted: shelves.deleted.keys().cloned().collect(),
        }))
    }

    fn read(&self, names: Vec<String>) -> Pending<Result<Vec<Option<String>>, Refused>> {
        let shelves = self.db.borrow();
        answer(self.failing().map(|()| {
            names
                .iter()
                .map(|name| shelves.sessions.get(name).cloned())
                .collect()
        }))
    }

    fn deleted(&self, names: Vec<String>) -> Pending<Result<Vec<String>, Refused>> {
        let shelves = self.db.borrow();
        answer(self.failing().map(|()| {
            names
                .into_iter()
                .filter(|name| shelves.deleted.contains_key(name))
                .collect()
        }))
    }

    fn apply(&self, writes: Writes) -> Pending<Result<(), Refused>> {
        let mut shelves = self.shelves();
        if let Some(first) = shelves.first.take() {
            first(&mut shelves);
        }
        if let Err(refused) = self.failing_now(&shelves) {
            return answer(Err(refused));
        }
        // Whole or not at all: made on a copy, kept only once made.
        let mut copy = Shelves {
            sessions: shelves.sessions.clone(),
            deleted: shelves.deleted.clone(),
            meta: shelves.meta.clone(),
            changes: shelves.changes,
            news: shelves.news,
            full: shelves.full,
            ..Shelves::default()
        };
        let made = Self::make(&mut copy, writes);
        if made.is_ok() {
            shelves.sessions = copy.sessions;
            shelves.deleted = copy.deleted;
            shelves.meta = copy.meta;
            shelves.changes = copy.changes;
        }
        answer(made)
    }

    fn stamp(&self) -> Option<String> {
        let changes = self.db.borrow().changes;
        (changes > 0).then(|| changes.to_string())
    }

    fn news(&self) -> u64 {
        self.db.borrow().news
    }

    fn next_news(&self) -> Pending<()> {
        Box::pin(std::future::pending())
    }

    fn legacy_len(&self, key: &str) -> Result<Option<u64>, ()> {
        if self.page.0.unreadable.get() {
            return Err(());
        }
        Ok(self
            .page
            .held(key)
            .map(|raw| raw.encode_utf16().count() as u64))
    }
}

impl Fake {
    fn failing_now(&self, shelves: &Shelves) -> Result<(), Refused> {
        match &shelves.failing {
            Some(why) => Err(Refused::Failed(why.clone())),
            None => Ok(()),
        }
    }
}

type Tab = BrowserHistory<Fake, Page>;

/// A tab opened on `fake`'s database and page storage.
fn open(fake: &Fake) -> Result<Tab, NotOpened> {
    done(BrowserHistory::open(
        fake.clone(),
        false,
        fake.page.clone(),
        KEYS,
    ))
}

fn opened(fake: &Fake) -> Tab {
    open(fake).unwrap_or_else(|not| panic!("{not:?}"))
}

/// A session finished `days` days before the tests started, on a day of
/// its own, and scored as it reads back: the same session each time it is
/// asked for.
fn ago(days: u64) -> SessionResult {
    static START: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let start = *START.get_or_init(crate::time::now_ms);
    record::scored(SessionResult {
        timestamp: start - days * 86_400_000,
        ..session(&format!(
            "2026-{:02}-{:02}",
            days / 28 % 12 + 1,
            days % 28 + 1
        ))
    })
}

fn sessions_of(load: &HistoryLoad) -> Vec<SessionResult> {
    load.sessions
        .iter()
        .map(|(_, session)| session.clone())
        .collect()
}

/// `session` as a record of its own, finished just now here.
fn record_of(session: &SessionResult) -> Record {
    Record::of(session, |json| {
        RecordId::next(session.timestamp, None, json)
    })
    .expect("a record")
}

/// A browser's history kept whole moves in as it was: each session a
/// record, named as a computer's move names it, in its order, with how the
/// history is kept beside them, in one change. The history kept whole is
/// left byte for byte as it was, for older builds.
#[test]
fn a_history_kept_whole_moves_in_as_it_was() {
    // The second stamped before the first, by a clock set back.
    let held = [ago(30), ago(40), ago(2)];
    let page = Page::holding(&held);
    page.deleted(&[Deleted::of(&ago(50), 1_234)]);
    let before = page.held(KEYS.history).expect("held");
    let fake = Fake::on(&page);
    let load = opened(&fake).load();
    assert_eq!(sessions_of(&load), held.to_vec(), "in its order");
    assert!(load.complete);
    assert!(load.notices.is_empty() && load.set_aside.is_empty());
    let moved: BTreeMap<RecordId, String> = record::records_for_blob(&held)
        .into_iter()
        .map(|record| (record.id, record.json))
        .collect();
    assert_eq!(fake.records(), moved, "as a computer moves it");
    let ids: Vec<RecordId> = load.sessions.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, moved.keys().copied().collect::<Vec<_>>());
    assert_eq!(fake.changes(), 1, "in one change");
    let layout = fake.layout().expect("how it is kept");
    assert_eq!(layout.legacy, Some(fingerprint_of(&before)));
    assert_eq!(layout.imported_keys.len(), 3);
    assert_eq!(layout.deleted_seen, 1_234);
    assert_eq!(page.held(KEYS.history), Some(before), "left as it was");
    // Opened again, it is moved already.
    let again = opened(&fake).load();
    assert_eq!(again.sessions, load.sessions);
    assert_eq!(fake.changes(), 1);
    // A first visit with nothing kept whole moves in how the history is
    // kept, and nothing else.
    let empty = Fake::default();
    assert_eq!(opened(&empty).load().sessions, []);
    assert!(empty.layout().is_some_and(|layout| layout.legacy.is_none()));
    assert!(empty.records().is_empty());
}

/// Two tabs opening at once move the history once: the one that comes
/// second finds it moved as it goes to move it, and reads it as moved.
#[test]
fn two_tabs_moving_at_once_move_once() {
    let held = [ago(3), ago(2)];
    let page = Page::holding(&held);
    let fake = Fake::on(&page);
    let theirs = held.clone();
    let fingerprint = page.held(KEYS.history).as_deref().map(fingerprint_of);
    fake.shelves().first = Some(Box::new(move |shelves: &mut Shelves| {
        let (records, layout) = legacy::moved(&theirs, fingerprint, &[], crate::time::now_ms());
        for record in records {
            shelves.sessions.insert(record.id.name(), record.json);
        }
        let written = layout_text(&layout).expect("written");
        shelves.meta.insert("layout".into(), written);
        shelves.changes += 1;
    }));
    let load = opened(&fake).load();
    assert_eq!(sessions_of(&load), held.to_vec(), "each once");
    assert!(load.complete);
    assert_eq!(fake.records().len(), 2);
    assert_eq!(fake.changes(), 1, "moved once, by the other tab");
}

/// What the history kept whole holds that does not read is copied aside,
/// told of once, and not moved; the history kept whole stays as it was,
/// holding it too. One that does not read at all moves nothing in.
#[test]
fn what_does_not_read_is_set_aside_and_not_moved() {
    let good = serde_json::to_string(&ago(2)).expect("writes");
    let raw = format!(r#"[{good},{{"date":"nonsense"}}]"#);
    let page = Page::default();
    page.set(KEYS.history, &raw);
    let fake = Fake::on(&page);
    let load = opened(&fake).load();
    assert_eq!(load.sessions.len(), 1);
    assert_eq!(
        load.set_aside,
        [SetAside::Sessions {
            count: 1,
            kept: None
        }]
    );
    assert_eq!(
        page.held(KEYS.aside).as_deref(),
        Some(r#"[{"date":"nonsense"}]"#)
    );
    assert_eq!(page.held(KEYS.history), Some(raw), "left as it was");
    assert_eq!(fake.records().len(), 1);

    let page = Page::default();
    page.set(KEYS.history, "{ damaged");
    let fake = Fake::on(&page);
    let load = opened(&fake).load();
    assert_eq!(load.sessions, []);
    assert_eq!(load.set_aside, [SetAside::History { kept: None }]);
    assert_eq!(page.held(KEYS.aside).as_deref(), Some("{ damaged"));
    assert_eq!(page.held(KEYS.history).as_deref(), Some("{ damaged"));
    assert!(fake.records().is_empty() && fake.layout().is_some());
    // Told of once: what is set aside already is not again.
    let second = Fake::on(&page);
    assert_eq!(opened(&second).load().set_aside, []);
}

/// A page storage that cannot be read now leaves the move for the next
/// launch, and says so: the database still takes what is stored meanwhile,
/// and the move made then writes no session twice.
#[test]
fn a_history_kept_whole_that_cannot_be_read_now_is_moved_later() {
    let held = [ago(5), ago(4)];
    let page = Page::holding(&held);
    page.0.unreadable.set(true);
    let fake = Fake::on(&page);
    let tab = opened(&fake);
    let load = tab.load();
    assert!(!load.complete);
    assert!(
        matches!(&load.notices[..], [HistoryNotice::MovePending(why)] if why.contains("page storage")),
        "{:?}",
        load.notices
    );
    assert_eq!(fake.layout(), None, "not moved");
    // A session finished meanwhile is stored all the same.
    let change = HistoryChange {
        put: vec![record_of(&ago(0))],
        ..HistoryChange::default()
    };
    assert_eq!(done(tab.apply(change)), HistorySaved::Stored { kept: 1 });
    // And one the page storage holds was stored by a copy that could read
    // it, under the move's own name.
    let first = record::records_for_blob(&held).remove(0);
    fake.shelves().sessions.insert(first.id.name(), first.json);
    page.0.unreadable.set(false);
    let load = opened(&fake).load();
    assert!(load.complete && load.notices.is_empty());
    let mut all = held.to_vec();
    all.push(ago(0));
    assert_eq!(sessions_of(&load), all);
    assert_eq!(fake.records().len(), 3, "none twice");
    assert!(fake.layout().is_some());
}

/// A database that will not take the move leaves the history to the page
/// storage for the visit, and says why: nothing of the move is made, and
/// what reading the page storage set aside is still told of.
#[test]
fn a_move_the_database_will_not_take_leaves_the_history_to_the_page_storage() {
    let good = serde_json::to_string(&ago(2)).expect("writes");
    let page = Page::default();
    page.set(KEYS.history, &format!(r#"[{good},{{"date":"x"}}]"#));
    let fake = Fake::on(&page);
    fake.shelves().full = true;
    let not = open(&fake).err().expect("not opened");
    assert_eq!(not.why, "it has no room left for this site");
    assert_eq!(
        not.set_aside,
        [SetAside::Sessions {
            count: 1,
            kept: None
        }]
    );
    assert!(fake.records().is_empty() && fake.layout().is_none());
    // A database that cannot be read at all, likewise.
    let fake = Fake::on(&Page::holding(&[ago(1)]));
    fake.shelves().failing = Some("it was closed".into());
    assert_eq!(
        open(&fake).err().map(|not| not.why).as_deref(),
        Some("it was closed")
    );
}

/// A history a newer Dust keeps in a way this one would harm is read as
/// far as it reads, said to be newer, and written by nothing; so is one
/// whose database a newer Dust took to a version past this one's.
#[test]
fn a_history_a_newer_dust_keeps_is_only_read() {
    let fake = Fake::on(&Page::holding(&[ago(9)]));
    let kept = record_of(&ago(3));
    {
        let mut shelves = fake.shelves();
        shelves
            .meta
            .insert("layout".into(), r#"{"layout":4,"compatible":2}"#.into());
        shelves.sessions.insert(kept.id.name(), kept.json);
    }
    let tab = opened(&fake);
    let load = tab.load();
    assert_eq!(sessions_of(&load), [ago(3)].to_vec());
    assert_eq!(load.notices, [HistoryNotice::Newer]);
    assert!(!load.complete);
    let change = HistoryChange {
        put: vec![record_of(&ago(0))],
        ..HistoryChange::default()
    };
    assert_eq!(done(tab.apply(change)), HistorySaved::Newer);
    assert_eq!(fake.records().len(), 1, "nothing written");
    assert_eq!(fake.changes(), 0);
    // Opened at whatever version a newer Dust left it, with nothing this
    // build can read in it.
    fake.shelves().failing = Some("there is no store sessions".into());
    let page = fake.page.clone();
    let read_only = done(BrowserHistory::open(fake, true, page, KEYS)).expect("opened, to be read");
    let load = read_only.load();
    assert_eq!(load.notices, [HistoryNotice::Newer]);
    assert_eq!(load.sessions, []);
    assert_eq!(
        done(read_only.apply(HistoryChange::default())),
        HistorySaved::Newer
    );
}

/// A layout that no longer reads is set aside beside the history, and the
/// move made again over it: it writes no session twice, nor one deleted
/// since, nor one taken up from an older build under a name of its own.
#[test]
fn a_layout_that_does_not_read_is_set_aside_and_the_move_made_again() {
    let held = [ago(9), ago(8), ago(7)];
    let page = Page::holding(&held);
    let fake = Fake::on(&page);
    let tab = opened(&fake);
    let ids: Vec<RecordId> = tab.load().sessions.iter().map(|(id, _)| *id).collect();
    // One deleted, by name.
    let gone = HistoryChange {
        forget: vec![(ids[0], Deleted::of(&held[0], 5))],
        ..HistoryChange::default()
    };
    assert_eq!(done(tab.apply(gone)), HistorySaved::Stored { kept: 0 });
    // One stored again under a name of its own, as one taken up is.
    let elsewhere =
        Record::of(&held[1], |json| RecordId::placed(held[1].timestamp, json)).expect("a record");
    {
        let mut shelves = fake.shelves();
        shelves.sessions.remove(&ids[1].name());
        shelves
            .sessions
            .insert(elsewhere.id.name(), elsewhere.json.clone());
        shelves.meta.insert("layout".into(), "{ damaged".into());
    }
    let load = opened(&fake).load();
    assert!(load.complete);
    assert_eq!(sessions_of(&load), held[1..].to_vec());
    let names: Vec<RecordId> = fake.records().into_keys().collect();
    assert_eq!(names, [elsewhere.id, ids[2]], "none twice, none back");
    let shelves = fake.db.borrow();
    let aside: Vec<&String> = shelves
        .meta
        .keys()
        .filter(|key| key.starts_with("layout.unreadable-"))
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(shelves.meta[aside[0]], "{ damaged", "kept as it was");
    assert!(matches!(
        Gate::of(shelves.meta.get("layout").map(String::as_bytes)),
        Gate::Open(_)
    ));
}

/// Sessions an older build stored in the history kept whole after the
/// move are taken up as the history is next listed, each once; those from
/// before, which it keeps for itself, are never brought back, nor one
/// deleted here; and a deletion it made since is made here too. Two tabs
/// taking them up at once take them up once.
#[test]
fn what_an_older_build_stores_after_the_move_is_taken_up_once() {
    let before = [ago(10), ago(9)];
    let page = Page::holding(&before);
    let fake = Fake::on(&page);
    let tab = opened(&fake);
    let ids: Vec<RecordId> = tab.load().sessions.iter().map(|(id, _)| *id).collect();
    // Deleted here.
    let gone = HistoryChange {
        forget: vec![(ids[1], Deleted::of(&before[1], crate::time::now_ms()))],
        ..HistoryChange::default()
    };
    done(tab.apply(gone));
    assert!(!tab.changed(), "nothing elsewhere");
    // An older build, still holding both, adds one, and deletes the first.
    let added = ago(0);
    page.keep(&[before[0].clone(), before[1].clone(), added.clone()]);
    page.deleted(&[Deleted::of(&before[0], crate::time::now_ms() + 1)]);
    assert!(tab.changed(), "an older build wrote");
    let listed = done(tab.listing()).expect("listed");
    let placed = Record::of(&added, |json| RecordId::placed(added.timestamp, json))
        .expect("a record")
        .id;
    assert_eq!(listed, [placed], "the new one in, the deleted ones out");
    assert_eq!(
        done(tab.read(vec![placed]))
            .remove(0)
            .ok()
            .map(|record| record.json),
        fake.records().get(&placed).cloned()
    );
    assert_eq!(done(tab.forgotten(ids.clone())), ids, "both deleted");
    assert!(!tab.changed());
    // Taken up once: again, nothing more.
    let changes = fake.changes();
    assert_eq!(done(tab.listing()), Some(vec![placed]));
    assert_eq!(fake.changes(), changes);
    let layout = fake.layout().expect("kept");
    assert!(layout.imported_keys.contains(&SessionKey::of(&added)));
    // Another tab, opened on the same database, finds it taken up.
    assert_eq!(sessions_of(&opened(&fake).load()), [added].to_vec());

    // Two tabs taking up one change at once: the second finds it taken.
    let page = Page::holding(&[ago(10)]);
    let fake = Fake::on(&page);
    let (first, second) = (opened(&fake), opened(&fake));
    page.keep(&[ago(10), ago(0)]);
    assert!(first.changed() && second.changed());
    assert!(done(first.listing()).is_some_and(|listed| listed.len() == 2));
    assert!(done(second.listing()).is_some_and(|listed| listed.len() == 2));
    assert_eq!(fake.records().len(), 2, "taken up once");
}

/// A change is stored whole, deletions remembered by tombstone: a session
/// deleted reads as deleted to every tab, one put back is back under its
/// own name, and one lost without a tombstone is not taken for deleted.
#[test]
fn a_change_is_stored_and_a_deletion_remembered() {
    let fake = Fake::default();
    let tab = opened(&fake);
    let (a, b) = (record_of(&ago(2)), record_of(&ago(1)));
    let stored = done(tab.apply(HistoryChange {
        put: vec![a.clone(), b.clone()],
        ..HistoryChange::default()
    }));
    assert_eq!(stored, HistorySaved::Stored { kept: 2 });
    assert_eq!(done(tab.listing()), Some(vec![a.id, b.id]));
    let forget = HistoryChange {
        forget: vec![(a.id, Deleted::of(&ago(2), 7))],
        ..HistoryChange::default()
    };
    done(tab.apply(forget));
    assert_eq!(done(tab.listing()), Some(vec![b.id]));
    assert_eq!(done(tab.forgotten(vec![a.id, b.id])), [a.id]);
    // Stored again, as a tab that missed the deletion would: still deleted.
    done(tab.apply(HistoryChange {
        put: vec![a.clone()],
        ..HistoryChange::default()
    }));
    assert_eq!(done(tab.listing()), Some(vec![b.id]));
    // Put back by Undo.
    done(tab.apply(HistoryChange {
        put: vec![a.clone()],
        unforget: vec![a.id],
        ..HistoryChange::default()
    }));
    assert_eq!(done(tab.listing()), Some(vec![a.id, b.id]));
    assert_eq!(done(tab.forgotten(vec![a.id])), []);
    // Lost, with no tombstone: missing, never deleted.
    fake.shelves().sessions.remove(&b.id.name());
    assert_eq!(done(tab.forgotten(vec![b.id])), []);
    assert_eq!(done(tab.read(vec![a.id, b.id])), [Ok(a.clone()), Err(b.id)]);
    // A database that cannot be read: nothing read, nothing deleted.
    fake.shelves().failing = Some("closed".into());
    assert_eq!(done(tab.listing()), None);
    assert_eq!(done(tab.forgotten(vec![a.id])), []);
    assert_eq!(done(tab.read(vec![a.id])), [Err(a.id)]);
}

/// A database with no room keeps nothing of a change, and says it is full;
/// one that refuses for another reason says why. The session is kept in
/// the app, and stored with the next change once there is room.
#[test]
fn a_change_the_database_has_no_room_for_keeps_nothing_of_it() {
    let fake = Fake::default();
    let tab = opened(&fake);
    let a = record_of(&ago(1));
    fake.shelves().full = true;
    let put = || HistoryChange {
        put: vec![a.clone()],
        ..HistoryChange::default()
    };
    assert_eq!(done(tab.apply(put())), HistorySaved::Full);
    assert!(fake.records().is_empty());
    fake.shelves().full = false;
    fake.shelves().failing = Some("UnknownError: the disk went away.".into());
    assert_eq!(
        done(tab.apply(put())),
        HistorySaved::Failed("UnknownError: the disk went away".into())
    );
    fake.shelves().failing = None;
    assert_eq!(done(tab.apply(put())), HistorySaved::Stored { kept: 1 });
    // A newer Dust opened the database in another tab meanwhile.
    let replaced = Refused::said("replaced", String::new()).saved();
    let said = replaced.problem().expect("told");
    assert!(said.contains("reloaded"), "{said}");
}

/// A record this build cannot read is left where it is, told of once, and
/// never taken for one missing.
#[test]
fn a_record_this_build_cannot_read_is_left_and_told_of_once() {
    let fake = Fake::default();
    opened(&fake);
    let id = RecordId {
        order: 5_000,
        nonce: 9,
    };
    fake.shelves()
        .sessions
        .insert(id.name(), r#"{"date":"later"}"#.into());
    let load = opened(&fake).load();
    assert_eq!(load.unread, [id]);
    assert_eq!(load.notices, [HistoryNotice::Unread { count: 1 }]);
    assert_eq!(opened(&fake).load().notices, [], "told once");
    assert!(fake.records().contains_key(&id), "left where it is");
}

/// The history is read whole, a page at a time, however long it is: more
/// than any page storage held, with nothing let go.
#[test]
fn a_history_of_any_length_is_read_whole() {
    let fake = Fake::default();
    let tab = opened(&fake);
    let long = ago(1);
    let mut put = Vec::new();
    for at in 0..(2 * PAGE + 7) as u64 {
        let session = SessionResult {
            timestamp: long.timestamp - at,
            alphabet_fingerprint: "x".repeat(1_500),
            ..long.clone()
        };
        put.push(record_of(&session));
    }
    let count = put.len();
    let size: usize = put.iter().map(|record| record.json.len()).sum();
    assert!(size > 3_000_000, "more than the page storage held: {size}");
    let stored = done(tab.apply(HistoryChange {
        put,
        ..HistoryChange::default()
    }));
    assert_eq!(stored, HistorySaved::Stored { kept: count });
    assert_eq!(stored.problem(), None, "nothing let go, nothing said");
    let load = opened(&fake).load();
    assert_eq!(load.sessions.len(), count);
    assert!(load.sessions.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

/// A backup holds every record the database does, each as it is stored,
/// with what a later build added to it, and in their places those the app
/// has yet to see stored; never one deleted, though the app still holds
/// it, as it does until it hears another tab deleted it. Where the database
/// cannot be listed, nothing is written until the app gives every session
/// it holds, and those are the backup.
#[test]
fn a_backup_holds_every_record_stored_and_none_deleted() {
    let fake = Fake::default();
    let tab = opened(&fake);
    let settings = cw_core::TrainingSettings::default();
    let sessions = [ago(4), ago(3), ago(2), ago(1)];
    let records: Vec<Record> = sessions.iter().map(record_of).collect();
    let mut later = records[0].clone();
    later.json = later.json.replacen('{', r#"{"later":[1,2],"#, 1);
    let first = read_record(&later.json).expect("it reads");
    assert_ne!(first, sessions[0], "with what the later build added");
    done(tab.apply(HistoryChange {
        put: vec![later, records[1].clone(), records[2].clone()],
        ..HistoryChange::default()
    }));
    done(tab.apply(HistoryChange {
        forget: vec![(records[1].id, Deleted::of(&sessions[1], 7))],
        ..HistoryChange::default()
    }));
    let changes = fake.changes();
    let unsaved = vec![records[1].clone(), records[3].clone()];
    let backup = BackupFile::new(&settings, 42, unsaved);
    let (raw, backed) = done(tab.back_up(&backup)).expect("written");
    assert_eq!(
        backed,
        BackedUp {
            sessions: 3,
            left_out: 0
        }
    );
    assert!(raw.contains(r#""later":[1,2]"#), "kept as stored: {raw}");
    let read = crate::persist::backup::read_backup(&raw).expect("a backup");
    assert_eq!(
        read.sessions,
        [first, sessions[2].clone(), sessions[3].clone()]
    );
    assert_eq!(read.saved_at, 42);
    assert_eq!(fake.changes(), changes, "nothing written");

    fake.shelves().failing = Some("closed".into());
    assert_eq!(done(tab.back_up(&backup)), Err(NotSaved::Unread));
    let every = BackupFile::new(&settings, 42, records).holding_every_session();
    let (raw, backed) = done(tab.back_up(&every)).expect("written");
    assert_eq!(backed.sessions, 4);
    let read = crate::persist::backup::read_backup(&raw).expect("a backup");
    assert_eq!(read.sessions, sessions.to_vec(), "what the app holds");
}

/// The history may have changed as another tab tells of a change or leaves
/// its stamp, or as an older build writes the history kept whole: each is
/// said once, until it changes again.
#[test]
fn the_history_changed_as_another_tab_or_an_older_build_says() {
    let page = Page::holding(&[ago(4)]);
    let fake = Fake::on(&page);
    let tab = opened(&fake);
    assert!(!tab.changed(), "what it moved in is its own");
    let tab = opened(&fake);
    assert!(!tab.changed());
    fake.shelves().news += 1;
    assert!(tab.changed(), "told of");
    assert!(!tab.changed(), "once");
    let other = opened(&fake);
    done(other.apply(HistoryChange {
        put: vec![record_of(&ago(0))],
        ..HistoryChange::default()
    }));
    assert!(tab.changed(), "a stamp left");
    assert!(!tab.changed());
    // Finished after the move, by an older build.
    let older = SessionResult {
        date: "2026-12-30".into(),
        ..ago(0)
    };
    page.keep(&[ago(4), older]);
    assert!(tab.changed(), "the history kept whole is longer");
    done(tab.listing());
    assert!(!tab.changed(), "taken up");
    // The app takes the history once; asked again, it holds nothing, and
    // the next look takes up every record.
    let tab = opened(&fake);
    assert_eq!(tab.load().sessions.len(), 3);
    assert_eq!(tab.load().sessions, []);
    assert!(tab.changed());
}

/// The browser clearing the database under the page loses what it held:
/// how the history is kept goes back in with the next change, and the app,
/// finding its records missing with no tombstone, stores them again.
#[test]
fn a_database_cleared_under_the_page_has_how_it_is_kept_written_back() {
    let fake = Fake::on(&Page::holding(&[ago(3)]));
    let tab = opened(&fake);
    let ids: Vec<RecordId> = tab.load().sessions.iter().map(|(id, _)| *id).collect();
    let layout = fake.layout();
    *fake.shelves() = Shelves::default();
    assert_eq!(done(tab.listing()), Some(Vec::new()));
    assert_eq!(done(tab.forgotten(ids)), [], "none deleted");
    let now = record_of(&ago(0));
    done(tab.apply(HistoryChange {
        put: vec![now.clone()],
        ..HistoryChange::default()
    }));
    assert_eq!(fake.layout(), layout, "written back");
    assert_eq!(fake.records().into_keys().collect::<Vec<_>>(), [now.id]);
}

/// A move is made over nothing but what is not there already: a record
/// there by its own name is written again, which writes nothing; one
/// deleted by name or by key, or there under another name, is not.
#[test]
fn a_move_made_again_writes_no_session_twice() {
    let held = [ago(9), ago(8), ago(7), ago(6)];
    let moved = record::records_for_blob(&held);
    let elsewhere =
        Record::of(&held[2], |json| RecordId::placed(held[2].timestamp, json)).expect("a record");
    let read = Read {
        raw: [
            (moved[0].id, moved[0].json.clone()),
            (elsewhere.id, elsewhere.json),
        ]
        .into_iter()
        .collect(),
        ..Read::default()
    };
    let gone: BTreeSet<RecordId> = [moved[1].id].into_iter().collect();
    let gone_keys: HashSet<SessionKey> = [SessionKey::of(&held[3])].into_iter().collect();
    let left = not_moved_yet(moved.clone(), &read, &gone, &gone_keys);
    assert_eq!(left, [moved[0].clone()]);
    // Into a database that holds nothing, all of it.
    assert_eq!(
        not_moved_yet(
            moved.clone(),
            &Read::default(),
            &BTreeSet::new(),
            &gone_keys
        ),
        moved
    );
}
