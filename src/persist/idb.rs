//! The history in a browser's database, IndexedDB `dust_history`, a record
//! a session ([`super::record`]): how a browser keeps it from this version
//! on, with no limit but the room the browser gives the site.
//!
//! The database is reached through a page script ([`IDB_JS`]), which keeps
//! every key and value a string, so that what a build writes comes back
//! byte for byte, and makes each change one transaction, whole or not at
//! all ([`Glue`]). It opens before the app does, and the whole history is
//! read then, a page at a time ([`BrowserHistory::open`]).
//!
//! A browser kept the history whole before, in its page storage under
//! `dust_sessions`. That is moved in once, in one transaction, which two
//! tabs opening at once make only once: the records a computer's move makes
//! ([`legacy::moved`]), and how the history is kept beside them.
//! `dust_sessions` itself is left as it was, for older builds, which keep
//! their history in it still; what one stores there after the move is taken
//! up here as on a computer ([`legacy::imported`]).
//!
//! Where the database will not open, or will not take the move, the history
//! is kept whole in the page storage for the visit, as it was before, to its
//! room ([`legacy::BlobHistory`]), and the user is told
//! ([`HistoryNotice::PageStorage`]).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use cw_core::SessionResult;

use super::legacy::{self, Aside, BlobKeys, Fingerprint, Gate, Kept, Layout, keep_aside};
use super::record::{
    self, HistoryChange, HistoryLoad, HistoryNotice, Pending, Record, RecordId, read_record,
};
use super::{Deleted, HistorySaved, SessionKey, SetAside};

/// The page script that reaches the database: the body of a function of
/// `options`, which hands back what the history does with it.
pub(super) const IDB_JS: &str = include_str!("idb.js");

/// How many records are read at a time, as the history is read whole.
const PAGE: usize = 2000;

/// How many times the move is tried as the page opens, where another tab
/// moves the history first each time: once is all it takes, as every tab
/// moves the same records.
const MOVE_TRIES: usize = 3;

/// Why the database did not do what was asked, as the page script says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Refused {
    /// The browser has no room left for the site.
    Full,
    /// A newer Dust keeps the history in it, and this one may only read it.
    Newer,
    /// How the history is kept does not read.
    Unread,
    /// Another tab changed how the history is kept first: nothing was done.
    Raced,
    /// A newer Dust opened the database in another tab: this page has to
    /// be reloaded to write to it.
    Replaced,
    /// For the reason given, in the browser's words.
    Failed(String),
}

impl Refused {
    /// The refusal the page script names `why`, the browser saying
    /// `message`.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    pub(super) fn said(why: &str, message: String) -> Self {
        match why {
            "full" => Self::Full,
            "newer" => Self::Newer,
            "unread" => Self::Unread,
            "raced" => Self::Raced,
            "replaced" => Self::Replaced,
            _ => Self::Failed(message),
        }
    }

    /// Why, in words that follow "the browser's database would not
    /// take it:".
    pub(super) fn words(&self) -> String {
        match self {
            Self::Full => "it has no room left for this site".into(),
            Self::Newer => "a newer Dust keeps the history in it".into(),
            Self::Unread => "how the history is kept could not be read".into(),
            Self::Raced => "other tabs kept changing it".into(),
            Self::Replaced => {
                "Dust was updated in another tab, so this page has to be reloaded first".into()
            }
            Self::Failed(why) => why.trim_end_matches('.').to_string(),
        }
    }

    /// What became of a change the database refused.
    fn saved(&self) -> HistorySaved {
        match self {
            Self::Full => HistorySaved::Full,
            Self::Newer => HistorySaved::Newer,
            other => HistorySaved::Failed(other.words()),
        }
    }
}

/// What the database holds beside the records, as text: how the history is
/// kept, and the records this build could not read and has told of. Each
/// none where none is held.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Head {
    pub(super) layout: Option<String>,
    pub(super) told: Option<String>,
}

/// The name of every record, and of every tombstone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Listed {
    pub(super) sessions: Vec<String>,
    pub(super) deleted: Vec<String>,
}

/// A change as the database makes it: in one transaction, whole or not at
/// all. Records by name, each with what it holds; tombstones by the name of
/// the record deleted, each with what it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Writes {
    pub(super) put: Vec<(String, String)>,
    pub(super) forget: Vec<(String, String)>,
    pub(super) unforget: Vec<String>,
    /// How the history is kept, written as this says.
    pub(super) layout: Option<LayoutWrite>,
    /// Anything else noted beside the history: what was told, a layout
    /// that did not read, set aside.
    pub(super) meta: Vec<(String, String)>,
}

/// How a change writes how the history is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LayoutWrite {
    /// Written where none is held, as after the browser cleared the
    /// database; left as it is where one is.
    Missing(String),
    /// Written over `expect`, which none is where nothing is held: or else
    /// nothing of the change is made, as another tab changed how the
    /// history is kept first ([`Refused::Raced`]).
    Over {
        expect: Option<String>,
        write: String,
    },
}

/// What a store of the database holds, or part of it: each key, and what it
/// holds, none where it holds no text.
pub(super) type Stored = Vec<(String, Option<String>)>;

/// What the database does for the history, as the page script makes it
/// ([`IDB_JS`]), or a test's stand-in in memory. What waits answers a
/// moment later; the rest at once.
pub(super) trait Glue {
    /// How the history is kept, and the records told of.
    fn head(&self) -> Pending<Result<Head, Refused>>;
    /// Up to `count` records after the one named `after`, or from the
    /// first, in the order of their names, which is the history's: each
    /// name, and what it holds, none where it holds no text.
    fn page(&self, after: Option<String>, count: usize) -> Pending<Result<Stored, Refused>>;
    /// Every tombstone, and what it holds.
    fn tombstones(&self) -> Pending<Result<Stored, Refused>>;
    fn list(&self) -> Pending<Result<Listed, Refused>>;
    /// What the records `names` hold, in order, each none where it is not
    /// there.
    fn read(&self, names: Vec<String>) -> Pending<Result<Vec<Option<String>>, Refused>>;
    /// Which of `names` were deleted by hand: those with a tombstone.
    fn deleted(&self, names: Vec<String>) -> Pending<Result<Vec<String>, Refused>>;
    /// Make `writes`, whole or not at all. Deletions undone first, then the
    /// records, then the deletions: a record deleted already stays deleted
    /// unless the change undoes that, a record is never written over (one
    /// whose name other bytes have goes under the next name), and a
    /// tombstone held already stays as it is. A layout a newer Dust wrote,
    /// or one that does not read, lets nothing be written but a layout over
    /// it. Once made, it is told to every other tab.
    fn apply(&self, writes: Writes) -> Pending<Result<(), Refused>>;
    /// The stamp the last change any tab made left, which changes with
    /// every change: none where none has.
    fn stamp(&self) -> Option<String>;
    /// How many changes other tabs have told of, this visit.
    fn news(&self) -> u64;
    /// A wait that ends as another tab tells of a change.
    fn next_news(&self) -> Pending<()>;
    /// How long the page storage's `key` is, in the browser's characters,
    /// the units of UTF-16: none where it holds nothing; `Err` where it
    /// cannot be read.
    fn legacy_len(&self, key: &str) -> Result<Option<u64>, ()>;
}

/// The history in the database, by record, reached through `glue`; and
/// `shelf`, the page storage, where an older build keeps it whole under
/// `keys`.
pub(super) struct BrowserHistory<G, S> {
    glue: G,
    shelf: S,
    keys: BlobKeys,
    /// This build may write to the history: not where a newer Dust keeps
    /// it.
    writable: Cell<bool>,
    /// How the history is kept, as it was read or last written, beside the
    /// text it is stored as: none before the move.
    layout: RefCell<Option<(Layout, String)>>,
    /// The history as it was read as the page opened, until the app takes
    /// it.
    loaded: RefCell<Option<HistoryLoad>>,
    /// What the move set aside of the history kept whole, as it could not
    /// read it, not yet handed over to be told of.
    aside: RefCell<Vec<SetAside>>,
    /// The key of each record read so far: what an older build's deletions
    /// are told apart by.
    session_keys: RefCell<HashMap<RecordId, SessionKey>>,
    /// The stamp of the last change, and how many changes other tabs had
    /// told of, when this copy last looked.
    seen: RefCell<Option<Looked>>,
}

/// What a look at the history saw ([`BrowserHistory::changed`]).
type Looked = (Option<String>, u64);

/// Why the database does not keep the history this visit, and what reading
/// the page storage set aside meanwhile, as it could not read it.
#[derive(Debug, PartialEq)]
pub(super) struct NotOpened {
    pub(super) why: String,
    pub(super) set_aside: Vec<SetAside>,
}

/// Why a move was not made.
enum Moving {
    /// Another tab made it first.
    Raced,
    /// The database would not take it, for the reason given.
    Refused(String),
}

/// The records of the database, read whole.
#[derive(Default)]
struct Read {
    /// Every session that reads, by its record, in the order of the
    /// history.
    sessions: Vec<(RecordId, SessionResult)>,
    /// Every record that does not.
    unread: Vec<RecordId>,
    /// What each record holds, as it is stored, where a move is to be made
    /// over them.
    raw: BTreeMap<RecordId, String>,
}

/// The key a record's session finished at, read without the rest of it.
#[derive(serde::Deserialize)]
struct KeyOnly {
    timestamp: u64,
    date: String,
}

impl<G: Glue, S: Aside> BrowserHistory<G, S> {
    /// The history in the database `glue` opened, read whole: the history
    /// kept whole in `shelf` moved in first, where it was not, and what an
    /// older build stored there since taken up. `read_only` says a newer
    /// Dust keeps the database, at a version past this build's, so it is
    /// only read.
    ///
    /// Or why the page storage is to keep the history this visit instead:
    /// the database would not take the move, or could not be read.
    pub(super) async fn open(
        glue: G,
        read_only: bool,
        shelf: S,
        keys: BlobKeys,
    ) -> Result<Self, NotOpened> {
        let history = Self {
            glue,
            shelf,
            keys,
            writable: Cell::new(!read_only),
            layout: RefCell::new(None),
            loaded: RefCell::new(None),
            aside: RefCell::new(Vec::new()),
            session_keys: RefCell::new(HashMap::new()),
            seen: RefCell::new(None),
        };
        let mut tries = 0;
        let made = loop {
            tries += 1;
            match history.read_in().await {
                Err(Moving::Raced) if tries < MOVE_TRIES => {}
                Err(Moving::Raced) => break Err(Refused::Raced.words()),
                Err(Moving::Refused(why)) => break Err(why),
                Ok(load) => break Ok(load),
            }
        };
        let set_aside = std::mem::take(&mut *history.aside.borrow_mut());
        match made {
            Ok(mut load) => {
                load.set_aside = set_aside;
                history.loaded.replace(Some(load));
                Ok(history)
            }
            Err(why) => Err(NotOpened { why, set_aside }),
        }
    }

    /// Read the history in, making the move first where it was not made.
    async fn read_in(&self) -> Result<HistoryLoad, Moving> {
        let mut load = HistoryLoad::default();
        // As of before it is read: what another tab changes after that is
        // a change to take up.
        self.seen.replace(Some(self.looking()));
        let read_only = !self.writable.get();
        let refused = |refused: Refused| Moving::Refused(refused.words());
        let head = match self.glue.head().await {
            Ok(head) => head,
            // A newer Dust's database need hold nothing this build knows:
            // nothing is read of it, and nothing written.
            Err(_) if read_only => {
                load.notices.push(HistoryNotice::Newer);
                return Ok(load);
            }
            Err(refusal) => return Err(refused(refusal)),
        };
        let gate = if read_only {
            Gate::Newer
        } else {
            Gate::of(head.layout.as_deref().map(str::as_bytes))
        };
        let layout = match gate {
            Gate::Open(layout) => head.layout.clone().map(|raw| (layout, raw)),
            Gate::Newer => {
                self.writable.set(false);
                load.notices.push(HistoryNotice::Newer);
                None
            }
            Gate::Unmoved | Gate::Unread => None,
        };
        let writable = self.writable.get();
        let moving = writable && layout.is_none();
        let tombstones = match self.glue.tombstones().await {
            Ok(tombstones) => tombstones,
            Err(_) if !writable => Vec::new(),
            Err(refusal) => return Err(refused(refusal)),
        };
        let gone: BTreeSet<RecordId> = tombstones
            .iter()
            .filter_map(|(name, _)| RecordId::parse(name))
            .collect();
        let mut read = match self.read_all(&gone, moving).await {
            Ok(read) => read,
            Err(_) if !writable => Read::default(),
            Err(refusal) => return Err(refused(refusal)),
        };
        let layout = if moving {
            let gone_keys = tombstoned_keys(&tombstones);
            let moved = self
                .move_in(head.layout.as_deref(), &read, &gone, &gone_keys, &mut load)
                .await?;
            moved.map(|(layout, written, records)| {
                let change = HistoryChange {
                    put: records,
                    ..HistoryChange::default()
                };
                taken_up(&mut read.sessions, &change);
                (layout, written)
            })
        } else {
            layout
        };
        if let Some(count) = self.tell(&read.unread, head.told.as_deref()).await {
            load.notices.push(HistoryNotice::Unread { count });
        }
        load.complete = layout.is_some();
        self.layout.replace(layout);
        if load.complete {
            let live = keyed(read.sessions.iter().map(|(id, session)| (*id, session)));
            if let Some(change) = self.import(Some(live)).await {
                taken_up(&mut read.sessions, &change);
            }
        }
        load.sessions = read.sessions;
        load.unread = read.unread;
        Ok(load)
    }

    /// Every record in the database, read a page at a time, in order, but
    /// those deleted, `gone`, which read as deleted. `keep` keeps what each
    /// holds as it is stored, for a move made over them. A name that names
    /// no record is left alone.
    async fn read_all(&self, gone: &BTreeSet<RecordId>, keep: bool) -> Result<Read, Refused> {
        let mut read = Read::default();
        let mut after = None;
        loop {
            let page = self.glue.page(after.clone(), PAGE).await?;
            let more = page.len() == PAGE;
            {
                let mut keys = self.session_keys.borrow_mut();
                for (name, raw) in page {
                    let id = RecordId::parse(&name);
                    after = Some(name);
                    let Some(id) = id.filter(|id| !gone.contains(id)) else {
                        continue;
                    };
                    match raw.as_deref().and_then(read_record) {
                        Some(session) => {
                            keys.insert(id, SessionKey::of(&session));
                            read.sessions.push((id, session));
                        }
                        None => read.unread.push(id),
                    }
                    if keep && let Some(raw) = raw {
                        read.raw.insert(id, raw);
                    }
                }
            }
            if !more {
                return Ok(read);
            }
        }
    }

    /// Move the history kept whole in the page storage into the database,
    /// over `held`, the layout there, if any, which does not read; `read`
    /// is what the database holds already, and `gone` and `gone_keys` its
    /// tombstones, by name and by key.
    ///
    /// Every session that reads becomes a record, written with how the
    /// history is kept in one transaction: a move cut short is no move, and
    /// two tabs making it at once make it once. What does not read is set
    /// aside first, copied beside it: the history kept whole is never
    /// written, as older builds keep theirs in it, and holds what does not
    /// read all the same where there is no room for a copy.
    ///
    /// How the history is kept, as written, and the records written; none
    /// where the page storage cannot be read now, which the next launch
    /// tries again.
    async fn move_in(
        &self,
        held: Option<&str>,
        read: &Read,
        gone: &BTreeSet<RecordId>,
        gone_keys: &HashSet<SessionKey>,
        load: &mut HistoryLoad,
    ) -> Result<Option<(Layout, String, Vec<Record>)>, Moving> {
        let Ok(raw) = self.shelf.get(self.keys.history) else {
            load.notices.push(HistoryNotice::MovePending(
                "this browser's page storage could not be read".into(),
            ));
            return Ok(None);
        };
        let fingerprint = raw.as_deref().map(fingerprint_of);
        let sessions = match raw.as_deref().map(legacy::read_history) {
            None => Vec::new(),
            Some(None) => {
                self.set_aside(
                    raw.as_deref().unwrap_or_default(),
                    SetAside::History { kept: None },
                );
                Vec::new()
            }
            Some(Some(whole)) => {
                if let Some(unread) = whole.unread_json() {
                    let count = whole.unread.len();
                    self.set_aside(&unread, SetAside::Sessions { count, kept: None });
                }
                whole.sessions
            }
        };
        let now = crate::time::now_ms();
        let (records, layout) = legacy::moved(&sessions, fingerprint, &self.legacy_deleted(), now);
        let records = not_moved_yet(records, read, gone, gone_keys);
        let Some(written) = layout_text(&layout) else {
            return Err(Moving::Refused(
                "how the history is kept could not be written down".into(),
            ));
        };
        let mut meta = Vec::new();
        if let Some(held) = held {
            meta.push((format!("layout.unreadable-{now}"), held.to_string()));
        }
        let writes = Writes {
            put: named(&records),
            layout: Some(LayoutWrite::Over {
                expect: held.map(str::to_string),
                write: written.clone(),
            }),
            meta,
            ..Writes::default()
        };
        match self.write(writes).await {
            Ok(()) => Ok(Some((layout, written, records))),
            Err(Refused::Raced) => Err(Moving::Raced),
            Err(refused) => Err(Moving::Refused(refused.words())),
        }
    }

    /// Copy `unread`, what the history kept whole holds that does not
    /// read, aside, and note `what` to tell of, unless it was set aside
    /// before: kept or not, it stays where it is too.
    fn set_aside(&self, unread: &str, what: SetAside) {
        if keep_aside(&self.shelf, self.keys.aside, unread) != Kept::Already {
            self.aside.borrow_mut().push(what);
        }
    }

    /// Take up what an older build stored in, or deleted from, the history
    /// kept whole since it was last taken up ([`legacy::imported`]), `live`
    /// being every record here by its session's key, where it is known
    /// already; and hand back the change made of it. Nothing, where nothing
    /// changed there, or what changed cannot be read or made now: it is
    /// tried again the next time.
    ///
    /// What was taken up is noted in how the history is kept, written in
    /// the same transaction, and only over the layout it was worked out
    /// from: two tabs taking it up at once take it up once.
    async fn import(
        &self,
        live: Option<HashMap<SessionKey, Vec<RecordId>>>,
    ) -> Option<HistoryChange> {
        if !self.writable.get() {
            return None;
        }
        let (layout, held) = self.layout.borrow().clone()?;
        let raw = self.shelf.get(self.keys.history).ok()?;
        let fingerprint = raw.as_deref().map(fingerprint_of);
        if fingerprint == layout.legacy {
            return None;
        }
        let sessions = raw
            .as_deref()
            .and_then(legacy::read_history)
            .map(|whole| whole.sessions)
            .unwrap_or_default();
        let deleted = self.legacy_deleted();
        let gone = tombstoned_keys(&self.glue.tombstones().await.ok()?);
        let live = match live {
            Some(live) => live,
            None => self.live_keys().await?,
        };
        let (change, next) =
            legacy::imported(&layout, fingerprint, &sessions, &deleted, &live, &gone)?;
        let written = layout_text(&next)?;
        let writes = Writes {
            layout: Some(LayoutWrite::Over {
                expect: Some(held),
                write: written.clone(),
            }),
            ..writes_of(&change)
        };
        match self.write(writes).await {
            Ok(()) => {
                self.note(&change);
                self.layout.replace(Some((next, written)));
                Some(change)
            }
            Err(Refused::Raced) => {
                self.read_layout().await;
                None
            }
            Err(Refused::Newer) => {
                self.writable.set(false);
                None
            }
            Err(_) => None,
        }
    }

    /// Read how the history is kept again, as another tab changed it.
    async fn read_layout(&self) {
        let Ok(head) = self.glue.head().await else {
            return;
        };
        match Gate::of(head.layout.as_deref().map(str::as_bytes)) {
            Gate::Open(layout) => {
                self.layout.replace(head.layout.map(|raw| (layout, raw)));
            }
            Gate::Newer => self.writable.set(false),
            Gate::Unmoved | Gate::Unread => {}
        }
    }

    /// Every record there is by its session's key: those read before as
    /// they were read, the rest read now for their keys alone.
    async fn live_keys(&self) -> Option<HashMap<SessionKey, Vec<RecordId>>> {
        let ids = live_ids(&self.glue.list().await.ok()?);
        let unknown: Vec<RecordId> = {
            let keys = self.session_keys.borrow();
            ids.iter()
                .filter(|id| !keys.contains_key(id))
                .copied()
                .collect()
        };
        if !unknown.is_empty() {
            let names = unknown.iter().map(|id| id.name()).collect();
            let held = self.glue.read(names).await.ok()?;
            let mut keys = self.session_keys.borrow_mut();
            for (id, raw) in unknown.into_iter().zip(held) {
                if let Some(key) = raw.as_deref().and_then(key_of) {
                    keys.insert(id, key);
                }
            }
        }
        let keys = self.session_keys.borrow();
        let mut live: HashMap<SessionKey, Vec<RecordId>> = HashMap::new();
        for id in ids {
            if let Some(key) = keys.get(&id) {
                live.entry(key.clone()).or_default().push(id);
            }
        }
        Some(live)
    }

    /// Say how many of `unread`, the records this build cannot read, are
    /// new to it, `told` being those told of before: none, once each has
    /// been. They are noted as told, where this build may write.
    async fn tell(&self, unread: &[RecordId], told: Option<&str>) -> Option<usize> {
        let told: HashSet<String> = told
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        let new = unread
            .iter()
            .filter(|id| !told.contains(&id.name()))
            .count();
        if new == 0 {
            return None;
        }
        let names: Vec<String> = unread.iter().map(|id| id.name()).collect();
        if self.writable.get()
            && let Ok(raw) = serde_json::to_string(&names)
        {
            let noted = Writes {
                meta: vec![("told".into(), raw)],
                ..Writes::default()
            };
            // Not noted, it is told of again: no more.
            let _ = self.write(noted).await;
        }
        Some(new)
    }

    /// The history as it was read as the page opened. Asked again, as no
    /// app does, it holds nothing, and the next look at the store takes
    /// up every record ([`Self::changed`]).
    pub(super) fn load(&self) -> HistoryLoad {
        let loaded = self.loaded.borrow_mut().take();
        loaded.unwrap_or_else(|| {
            self.seen.replace(None);
            HistoryLoad::default()
        })
    }

    /// Make `change` to the history, and say what became of it. How the
    /// history is kept rides along, to be written again should the browser
    /// have cleared the database meanwhile.
    pub(super) async fn apply(&self, change: HistoryChange) -> HistorySaved {
        if !self.writable.get() {
            return HistorySaved::Newer;
        }
        let layout = self
            .layout
            .borrow()
            .as_ref()
            .map(|(_, raw)| LayoutWrite::Missing(raw.clone()));
        let writes = Writes {
            layout,
            ..writes_of(&change)
        };
        match self.write(writes).await {
            Ok(()) => {
                self.note(&change);
                HistorySaved::Stored {
                    kept: change.put.len(),
                }
            }
            Err(refused) => {
                if refused == Refused::Newer {
                    self.writable.set(false);
                }
                refused.saved()
            }
        }
    }

    /// Every record there is, deleted ones left out, once whatever an
    /// older build stored since has been taken up. None where the database
    /// cannot be listed.
    pub(super) async fn listing(&self) -> Option<Vec<RecordId>> {
        self.import(None).await;
        let looked = self.looking();
        let listed = self.glue.list().await.ok()?;
        self.seen.replace(Some(looked));
        Some(live_ids(&listed))
    }

    /// The records `ids`, as they are stored: one that is not there, or
    /// cannot be read, comes back as its id.
    pub(super) async fn read(&self, ids: Vec<RecordId>) -> Vec<Result<Record, RecordId>> {
        let names = ids.iter().map(|id| id.name()).collect();
        let Ok(held) = self.glue.read(names).await else {
            return ids.into_iter().map(Err).collect();
        };
        let mut keys = self.session_keys.borrow_mut();
        ids.into_iter()
            .zip(held.into_iter().chain(std::iter::repeat(None)))
            .map(|(id, json)| {
                let json = json.ok_or(id)?;
                if let Some(key) = key_of(&json) {
                    keys.insert(id, key);
                }
                Ok(Record { id, json })
            })
            .collect()
    }

    /// Which of `ids` were deleted by hand. Where the tombstones cannot be
    /// looked at, none: missing is never deleted.
    pub(super) async fn forgotten(&self, ids: Vec<RecordId>) -> Vec<RecordId> {
        let names = ids.iter().map(|id| id.name()).collect();
        let Ok(gone) = self.glue.deleted(names).await else {
            return Vec::new();
        };
        let gone: HashSet<String> = gone.into_iter().collect();
        ids.into_iter()
            .filter(|id| gone.contains(&id.name()))
            .collect()
    }

    /// Whether the history may have changed since this copy last looked:
    /// another tab told of a change, or left its stamp, or an older build
    /// wrote to the history it keeps whole. A look at the stamp and at the
    /// length of that history, never a read of either.
    pub(super) fn changed(&self) -> bool {
        let now = self.looking();
        let told = self.seen.replace(Some(now.clone())) != Some(now);
        told || self.legacy_changed()
    }

    /// A wait that ends as another tab tells of a change it made.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    pub(super) fn news(&self) -> Pending<()> {
        self.glue.next_news()
    }

    fn looking(&self) -> Looked {
        (self.glue.stamp(), self.glue.news())
    }

    /// Make `writes`, and note the stamp they leave as seen, where no other
    /// tab left one since this copy last looked: a change of its own is no
    /// change for it to take up.
    async fn write(&self, writes: Writes) -> Result<(), Refused> {
        let before = self.glue.stamp();
        self.glue.apply(writes).await?;
        if let Some((stamp, _)) = self.seen.borrow_mut().as_mut()
            && *stamp == before
        {
            *stamp = self.glue.stamp();
        }
        Ok(())
    }

    /// Whether the history kept whole is no longer the length it was when
    /// last taken up. One written to the same length shows up at the next
    /// listing, which reads it ([`Self::import`]).
    fn legacy_changed(&self) -> bool {
        let layout = self.layout.borrow();
        let Some((layout, _)) = layout.as_ref() else {
            return false;
        };
        self.glue
            .legacy_len(self.keys.history)
            .is_ok_and(|len| len != layout.legacy.map(|held| held.len))
    }

    /// The deletions an older build remembers: none, where they cannot be
    /// read, which costs a session it deleted staying here, never one lost.
    fn legacy_deleted(&self) -> Vec<Deleted> {
        self.shelf
            .get(self.keys.deleted)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Note the keys of what `change`, made, stored and deleted.
    fn note(&self, change: &HistoryChange) {
        let mut keys = self.session_keys.borrow_mut();
        for record in &change.put {
            if let Some(key) = key_of(&record.json) {
                keys.insert(record.id, key);
            }
        }
        for (id, _) in &change.forget {
            keys.remove(id);
        }
    }
}

/// What tells one state of the history kept whole from another, as a
/// browser keeps no time it was written: its length, in the browser's
/// characters, as the page reads it without reading it whole
/// ([`Glue::legacy_len`]), and the hash of what it holds.
fn fingerprint_of(raw: &str) -> Fingerprint {
    Fingerprint {
        len: raw.encode_utf16().count() as u64,
        mtime: record::fnv64(raw.as_bytes()),
    }
}

/// `layout` as it is stored, as text.
fn layout_text(layout: &Layout) -> Option<String> {
    String::from_utf8(layout.written().ok()?).ok()
}

/// The key of the session `json` holds, read without the rest of it.
fn key_of(json: &str) -> Option<SessionKey> {
    serde_json::from_str::<KeyOnly>(json)
        .ok()
        .map(|key| SessionKey::new(key.timestamp, key.date))
}

/// The key of every session the tombstones name.
fn tombstoned_keys(tombstones: &[(String, Option<String>)]) -> HashSet<SessionKey> {
    tombstones
        .iter()
        .filter_map(|(_, raw)| serde_json::from_str::<Deleted>(raw.as_deref()?).ok())
        .map(|gone| gone.key())
        .collect()
}

/// Every record listed, but those deleted.
fn live_ids(listed: &Listed) -> Vec<RecordId> {
    let gone: HashSet<&String> = listed.deleted.iter().collect();
    listed
        .sessions
        .iter()
        .filter(|name| !gone.contains(name))
        .filter_map(|name| RecordId::parse(name))
        .collect()
}

/// Every record by its name, with what it holds.
fn named(records: &[Record]) -> Vec<(String, String)> {
    records
        .iter()
        .map(|record| (record.id.name(), record.json.clone()))
        .collect()
}

/// `change` as the database makes it.
fn writes_of(change: &HistoryChange) -> Writes {
    Writes {
        put: named(&change.put),
        forget: change
            .forget
            .iter()
            .map(|(id, gone)| {
                let tombstone = serde_json::to_string(gone).unwrap_or_else(|_| "{}".into());
                (id.name(), tombstone)
            })
            .collect(),
        unforget: change.unforget.iter().map(|id| id.name()).collect(),
        ..Writes::default()
    }
}

/// Of `moved`, the records a move makes, those to write: all of them, into
/// a database that holds nothing. A move made over records there already,
/// `read` — one cut off in a visit that could not read the page storage, or
/// made again over a layout that no longer reads — writes no session twice:
/// one deleted since, by name or by key, stays deleted, and one taken up
/// from an older build since, under the name it was taken up by, is not
/// written a second time under another.
fn not_moved_yet(
    moved: Vec<Record>,
    read: &Read,
    gone: &BTreeSet<RecordId>,
    gone_keys: &HashSet<SessionKey>,
) -> Vec<Record> {
    if read.raw.is_empty() && gone.is_empty() {
        return moved;
    }
    let names: BTreeSet<RecordId> = moved.iter().map(|record| record.id).collect();
    // What each record here under a name the move does not give holds: a
    // session that needs no record of the move's.
    let mut elsewhere: HashMap<&str, usize> = HashMap::new();
    for (id, json) in &read.raw {
        if !names.contains(id) {
            *elsewhere.entry(json.as_str()).or_default() += 1;
        }
    }
    moved
        .into_iter()
        .filter(|record| {
            if read.raw.contains_key(&record.id) {
                return true;
            }
            if gone.contains(&record.id) {
                return false;
            }
            if let Some(count) = elsewhere
                .get_mut(record.json.as_str())
                .filter(|count| **count > 0)
            {
                *count -= 1;
                return false;
            }
            !key_of(&record.json).is_some_and(|key| gone_keys.contains(&key))
        })
        .collect()
}

/// Every record of `sessions` by its session's key.
fn keyed<'a>(
    sessions: impl Iterator<Item = (RecordId, &'a SessionResult)>,
) -> HashMap<SessionKey, Vec<RecordId>> {
    let mut live: HashMap<SessionKey, Vec<RecordId>> = HashMap::new();
    for (id, session) in sessions {
        live.entry(SessionKey::of(session)).or_default().push(id);
    }
    live
}

/// `sessions`, in the order of their records, with `change`, made, made to
/// them too.
fn taken_up(sessions: &mut Vec<(RecordId, SessionResult)>, change: &HistoryChange) {
    let forgotten: HashSet<RecordId> = change.forget.iter().map(|(id, _)| *id).collect();
    sessions.retain(|(id, _)| !forgotten.contains(id));
    for record in &change.put {
        let Some(session) = read_record(&record.json) else {
            continue;
        };
        if let Err(at) = sessions.binary_search_by_key(&record.id, |(id, _)| *id) {
            sessions.insert(at, (record.id, session));
        }
    }
}

#[cfg(feature = "web")]
pub(super) use script::Script;

/// The page script, run on the page.
#[cfg(feature = "web")]
mod script {
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{Glue, Head, IDB_JS, LayoutWrite, Listed, Pending, Refused, Stored, Writes};

    /// What the page script hands back: what the history does with the
    /// database ([`IDB_JS`]).
    pub(in crate::persist) struct Script(JsValue);

    impl Script {
        /// Run the page script and open the database: the script, and
        /// whether a newer Dust keeps the database, so that this one only
        /// reads it; or why it would not open.
        pub(in crate::persist) async fn open() -> Result<(Self, bool), String> {
            let make = js_sys::Function::new_with_args("options", IDB_JS);
            let glue = make
                .call1(&JsValue::NULL, &JsValue::UNDEFINED)
                .map_err(|_| "its script would not run".to_string())?;
            let opened = asked(glue.clone(), "open", Vec::new())
                .await
                .map_err(|refused| refused.words())?;
            let read_only = opened.as_string().as_deref() == Some("newer");
            Ok((Self(glue), read_only))
        }

        /// The script's `name`, called at once with `args`.
        fn now(&self, name: &str, args: &[JsValue]) -> JsValue {
            let Some(function) = function(&self.0, name) else {
                return JsValue::UNDEFINED;
            };
            let args: js_sys::Array = args.iter().collect();
            function.apply(&self.0, &args).unwrap_or(JsValue::UNDEFINED)
        }
    }

    /// The script's `name`, if it has one.
    fn function(glue: &JsValue, name: &str) -> Option<js_sys::Function> {
        js_sys::Reflect::get(glue, &JsValue::from_str(name))
            .ok()?
            .dyn_into()
            .ok()
    }

    /// Ask the script's `name`, with `args`, and wait for its answer.
    async fn asked(glue: JsValue, name: &str, args: Vec<JsValue>) -> Result<JsValue, Refused> {
        let function = function(&glue, name)
            .ok_or_else(|| Refused::Failed(format!("its script has no {name}")))?;
        let args: js_sys::Array = args.into_iter().collect();
        let promise = function
            .apply(&glue, &args)
            .map_err(|error| Refused::Failed(said(&error)))?;
        let answer = JsFuture::from(js_sys::Promise::resolve(&promise))
            .await
            .map_err(|error| Refused::Failed(said(&error)))?;
        if let Some(why) = field(&answer, "why").as_string() {
            let message = field(&answer, "message").as_string().unwrap_or_default();
            return Err(Refused::said(&why, message));
        }
        Ok(field(&answer, "ok"))
    }

    fn field(value: &JsValue, name: &str) -> JsValue {
        js_sys::Reflect::get(value, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
    }

    /// What a thrown error says.
    fn said(error: &JsValue) -> String {
        error
            .as_string()
            .or_else(|| field(error, "message").as_string())
            .unwrap_or_else(|| "it went wrong".into())
    }

    /// Each of a list, as text: none for what is not.
    fn texts(value: &JsValue) -> Vec<Option<String>> {
        value
            .dyn_ref::<js_sys::Array>()
            .map(|list| list.iter().map(|one| one.as_string()).collect())
            .unwrap_or_default()
    }

    fn strings(value: &JsValue) -> Vec<String> {
        texts(value)
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect()
    }

    /// `{ keys, values }`, as pairs.
    fn pairs(value: &JsValue) -> Stored {
        strings(&field(value, "keys"))
            .into_iter()
            .zip(texts(&field(value, "values")))
            .collect()
    }

    fn list_of(pairs: &[(String, String)]) -> js_sys::Array {
        pairs
            .iter()
            .map(|(key, value)| {
                JsValue::from(js_sys::Array::of2(
                    &JsValue::from_str(key),
                    &JsValue::from_str(value),
                ))
            })
            .collect()
    }

    fn names(names: &[String]) -> js_sys::Array {
        names.iter().map(|name| JsValue::from_str(name)).collect()
    }

    fn set(object: &js_sys::Object, name: &str, value: &JsValue) {
        let _ = js_sys::Reflect::set(object, &JsValue::from_str(name), value);
    }

    /// `writes` as the script takes them.
    fn change(writes: &Writes) -> JsValue {
        let change = js_sys::Object::new();
        set(&change, "put", &list_of(&writes.put));
        set(&change, "forget", &list_of(&writes.forget));
        set(&change, "unforget", &names(&writes.unforget));
        set(&change, "meta", &list_of(&writes.meta));
        let layout = match &writes.layout {
            None => JsValue::NULL,
            Some(written) => {
                let layout = js_sys::Object::new();
                let (write, over, expect) = match written {
                    LayoutWrite::Missing(write) => (write, false, None),
                    LayoutWrite::Over { expect, write } => (write, true, expect.as_deref()),
                };
                set(&layout, "write", &JsValue::from_str(write));
                set(&layout, "over", &JsValue::from_bool(over));
                set(
                    &layout,
                    "expect",
                    &expect.map_or(JsValue::NULL, JsValue::from_str),
                );
                layout.into()
            }
        };
        set(&change, "layout", &layout);
        change.into()
    }

    impl Glue for Script {
        fn head(&self) -> Pending<Result<Head, Refused>> {
            let glue = self.0.clone();
            Box::pin(async move {
                let head = asked(glue, "head", Vec::new()).await?;
                Ok(Head {
                    layout: field(&head, "layout").as_string(),
                    told: field(&head, "told").as_string(),
                })
            })
        }

        fn page(&self, after: Option<String>, count: usize) -> Pending<Result<Stored, Refused>> {
            let glue = self.0.clone();
            Box::pin(async move {
                let after = after.as_deref().map_or(JsValue::NULL, JsValue::from_str);
                let count = JsValue::from_f64(count as f64);
                Ok(pairs(&asked(glue, "page", vec![after, count]).await?))
            })
        }

        fn tombstones(&self) -> Pending<Result<Stored, Refused>> {
            let glue = self.0.clone();
            Box::pin(async move { Ok(pairs(&asked(glue, "tombstones", Vec::new()).await?)) })
        }

        fn list(&self) -> Pending<Result<Listed, Refused>> {
            let glue = self.0.clone();
            Box::pin(async move {
                let listed = asked(glue, "list", Vec::new()).await?;
                Ok(Listed {
                    sessions: strings(&field(&listed, "sessions")),
                    deleted: strings(&field(&listed, "deleted")),
                })
            })
        }

        fn read(&self, keys: Vec<String>) -> Pending<Result<Vec<Option<String>>, Refused>> {
            let glue = self.0.clone();
            Box::pin(async move {
                Ok(texts(
                    &asked(glue, "read", vec![names(&keys).into()]).await?,
                ))
            })
        }

        fn deleted(&self, keys: Vec<String>) -> Pending<Result<Vec<String>, Refused>> {
            let glue = self.0.clone();
            Box::pin(async move {
                Ok(strings(
                    &asked(glue, "deleted", vec![names(&keys).into()]).await?,
                ))
            })
        }

        fn apply(&self, writes: Writes) -> Pending<Result<(), Refused>> {
            let glue = self.0.clone();
            Box::pin(async move {
                asked(glue, "apply", vec![change(&writes)]).await?;
                Ok(())
            })
        }

        fn stamp(&self) -> Option<String> {
            self.now("stamp", &[]).as_string()
        }

        fn news(&self) -> u64 {
            self.now("news", &[]).as_f64().unwrap_or(0.0) as u64
        }

        fn next_news(&self) -> Pending<()> {
            let waiting = self.now("nextNews", &[]);
            Box::pin(async move {
                if let Ok(promise) = waiting.dyn_into::<js_sys::Promise>() {
                    let _ = JsFuture::from(promise).await;
                } else {
                    // A script that cannot tell of changes tells of none.
                    std::future::pending::<()>().await;
                }
            })
        }

        fn legacy_len(&self, key: &str) -> Result<Option<u64>, ()> {
            let len = self.now("legacyLength", &[JsValue::from_str(key)]).as_f64();
            match len.map(|len| len as i64) {
                Some(len) if len >= 0 => Ok(Some(len as u64)),
                Some(-1) => Ok(None),
                _ => Err(()),
            }
        }
    }
}

#[cfg(test)]
mod page_tests;
#[cfg(test)]
mod tests;
