//! The history in a browser's database, IndexedDB `dust_history`, a record
//! a session ([`super::record`]): how a browser keeps it from this version
//! on, with no limit but the room the browser gives the site.
//!
//! The database is reached through a page script ([`IDB_JS`]), which keeps
//! every key and value a string, so that what a build writes comes back
//! byte for byte, and makes each change one transaction, whole or not at
//! all ([`Glue`]). It opens before the app does, and the whole history is
//! read then, a page at a time ([`BrowserHistory::open_with`]).
//!
//! What is read and written, in what order, and what is made of it, is the
//! record store's, as for every store ([`record_store`]): this module makes
//! its reads and its transactions in the database ([`InDb`]).
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
//!
//! [`HistoryNotice::PageStorage`]: super::record::HistoryNotice::PageStorage

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use super::backup::{BackedUp, BackupFile, NotSaved};
use super::legacy::{self, Aside, BlobKeys, Fingerprint, Gate, Kept, Layout, keep_aside};
use super::progress::LoadProgress;
use super::record::{
    self, Deletions, HistoryChange, HistoryLoad, Pending, Record, RecordId, read_record, unreadable,
};
use super::record_store::notes::Note;
use super::record_store::{
    self, Backend, By, KeyCache, Laid, Legacy, LegacyLook, Listing, Made, NotOpened, Opening,
    Order, Raw, Refusal, Tombstone, TombstoneRead, TombstonesOf, WholeRead, Writes,
};
use super::saved::HistorySaved;
use super::session::{Deleted, SessionKey};
use super::set_aside::SetAside;

/// The page script that reaches the database: the body of a function of
/// `options`, which hands back what the history does with it.
pub(super) const IDB_JS: &str = include_str!("idb.js");

/// How many records are read at a time, as the history is read whole.
const PAGE: usize = 2000;

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
}

impl Refusal for Refused {
    /// Why, in words that follow "the browser's database would not
    /// take it:".
    fn words(&self) -> String {
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

    fn raced(&self) -> bool {
        *self == Self::Raced
    }
}

/// What the database holds beside the records, as text: how the history is
/// kept, and the records this build could not read and has told of. Each
/// none where none is held. And how many records there are, where the
/// database says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Head {
    pub(super) layout: Option<String>,
    pub(super) told: Option<String>,
    pub(super) count: Option<u64>,
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
pub(super) struct Transaction {
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
    /// Which of `names` were deleted by hand: those with a tombstone, each
    /// beside what its tombstone holds, none where it holds no text.
    fn tombstones_of(&self, names: Vec<String>) -> Pending<Result<Stored, Refused>>;
    /// Make `writes`, whole or not at all. Deletions undone first, then the
    /// records, then the deletions: a record deleted already stays deleted
    /// unless the change undoes that, a record is never written over (one
    /// whose name other bytes have goes under the next name), and a
    /// tombstone held already stays as it is. A layout a newer Dust wrote,
    /// or one that does not read, lets nothing be written but a layout over
    /// it. Once made, it is told to every other tab. The name each record
    /// went under, in order: none for one left deleted.
    fn apply(&self, writes: Transaction) -> Pending<Result<Vec<Option<String>>, Refused>>;
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
    /// How the history is kept, as the text the database held as it was
    /// opened, where it is to be moved again over that: none, or one that
    /// does not read.
    held: RefCell<Option<String>>,
    /// The records told of, as the database noted them as it was opened.
    told: RefCell<Option<String>>,
    /// The key of each record read so far: what an older build's deletions
    /// are told apart by.
    session_keys: RefCell<KeyCache>,
    /// The stamp of the last change, and how many changes other tabs had
    /// told of, when this copy last looked.
    seen: RefCell<Option<Looked>>,
    /// How far the read as the page opens, or the move before it, has come
    /// ([`Self::open_with`]).
    progress: Arc<LoadProgress>,
}

/// What a look at the history saw ([`BrowserHistory::changed`]).
type Looked = (Option<String>, u64);

impl<G: Glue, S: Aside> BrowserHistory<G, S> {
    /// The history in the database `glue` opened, read whole: the history
    /// kept whole in `shelf` moved in first, where it was not, and what an
    /// older build stored there since taken up. `read_only` says a newer
    /// Dust keeps the database, at a version past this build's, so it is
    /// only read.
    ///
    /// Or why the page storage is to keep the history this visit instead:
    /// the database would not take the move, or could not be read.
    #[cfg(test)]
    pub(super) async fn open(
        glue: G,
        read_only: bool,
        shelf: S,
        keys: BlobKeys,
    ) -> Result<Self, NotOpened> {
        Self::open_with(glue, read_only, shelf, keys, Arc::default()).await
    }

    /// The same, counting in `progress` how far the read, or the move
    /// before it, has come, for the page's boot screen to say.
    pub(super) async fn open_with(
        glue: G,
        read_only: bool,
        shelf: S,
        keys: BlobKeys,
        progress: Arc<LoadProgress>,
    ) -> Result<Self, NotOpened> {
        let history = Self {
            glue,
            shelf,
            keys,
            writable: Cell::new(!read_only),
            layout: RefCell::new(None),
            loaded: RefCell::new(None),
            held: RefCell::new(None),
            told: RefCell::new(None),
            session_keys: RefCell::new(KeyCache::new()),
            seen: RefCell::new(None),
            progress,
        };
        let load = record_store::load(&InDb(&history)).await?;
        history.loaded.replace(Some(load));
        Ok(history)
    }

    /// Read how the history is kept again, as another tab changed it.
    async fn read_layout(&self) {
        let Ok(head) = self.glue.head().await else {
            return;
        };
        match Gate::of(head.layout.as_deref().map(str::as_bytes)) {
            Gate::Open(layout) => {
                self.layout.replace(head.layout.map(|raw| (*layout, raw)));
            }
            Gate::Newer => self.writable.set(false),
            Gate::Unmoved | Gate::Unread => {}
        }
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
        record_store::apply(&InDb(self), &change).await
    }

    /// Every record there is, deleted ones left out, once whatever an
    /// older build stored since has been taken up. None where the database
    /// cannot be listed.
    pub(super) async fn listing(&self) -> Option<Vec<RecordId>> {
        record_store::listing(&InDb(self)).await
    }

    /// The records `ids`, as they are stored: one that is not there, or
    /// cannot be read, comes back as its id. Their keys are not noted for
    /// the take-up: whether one holds the session it says it does takes
    /// reading it whole, which a backup of every record need not.
    pub(super) async fn read(&self, ids: Vec<RecordId>) -> Vec<Result<Record, RecordId>> {
        let read = record_store::read(&InDb(self), &ids).await;
        read.into_iter()
            .zip(ids)
            .map(|(read, id)| read.map_err(|_| id))
            .collect()
    }

    /// Which of `ids` were deleted by hand: those with a tombstone, each
    /// beside when it says the record was deleted, none for one that does
    /// not say. None where the tombstones cannot be looked at: that is no
    /// word that a record was not deleted. One with no tombstone was not:
    /// missing is never deleted.
    pub(super) async fn deletions(&self, ids: Vec<RecordId>) -> Option<Deletions> {
        record_store::deletions(&InDb(self), &ids).await
    }

    /// Which of `ids` were deleted by hand, each beside the session its
    /// tombstone remembers ([`Self::deletions`]): one whose tombstone does
    /// not say is left out. None where the tombstones cannot be looked at.
    pub(super) async fn deleted_sessions(
        &self,
        ids: Vec<RecordId>,
    ) -> Option<Vec<(RecordId, SessionKey)>> {
        record_store::deleted_sessions(&InDb(self), &ids).await
    }

    /// Which of the sessions `keys` a tombstone remembers, whatever record
    /// it named ([`super::Store::deleted_keys`]): one whose tombstone holds
    /// no text, or names no session, is left out. None where the tombstones
    /// cannot be looked at.
    pub(super) async fn deleted_keys(&self, keys: Vec<SessionKey>) -> Option<Vec<SessionKey>> {
        record_store::deleted_keys(&InDb(self), keys).await
    }

    /// Which of `ids` were deleted by hand ([`Self::deletions`]): none,
    /// where that could not be looked for.
    #[cfg(test)]
    pub(super) async fn forgotten(&self, ids: Vec<RecordId>) -> Vec<RecordId> {
        let found = self.deletions(ids).await.unwrap_or_default();
        found.into_iter().map(|(id, _)| id).collect()
    }

    /// `backup` whole, with every record the database holds, deleted ones
    /// left out, each as it is stored ([`BackupFile::write`]); or, where it
    /// cannot be listed, with what the app holds alone
    /// ([`BackupFile::write_unlisted`]). A session the app holds that the
    /// database holds as deleted stays out, as another tab may have deleted
    /// it since the app last looked: restoring the backup would bring it
    /// back. Nothing is noted as looked at: the app has not taken this up.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    pub(super) async fn back_up(
        &self,
        backup: &BackupFile,
    ) -> Result<(String, BackedUp), NotSaved> {
        let mut out = Vec::new();
        let backed = match self.glue.list().await {
            Ok(listed) => {
                let gone: BTreeSet<RecordId> = listed
                    .deleted
                    .iter()
                    .filter_map(|name| RecordId::parse(name))
                    .collect();
                let stored = self.read(live_ids(&listed)).await;
                backup.write(&mut out, stored, |id| gone.contains(&id))?
            }
            Err(_) => backup.write_unlisted(&mut out)?,
        };
        Ok((String::from_utf8(out).unwrap_or_default(), backed))
    }

    /// Whether the history may have changed since this copy last looked:
    /// another tab told of a change, or left its stamp, or an older build
    /// wrote to the history it keeps whole. A look at the stamp and at the
    /// length of that history, never a read of either.
    ///
    /// Nothing is noted: a change is told of each time it is asked, until a
    /// look that lists the history takes it up ([`Self::listing`]), so one
    /// whose look could not list it is told of again.
    pub(super) fn changed(&self) -> bool {
        *self.seen.borrow() != Some(self.looking()) || self.legacy_changed()
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
    /// change for it to take up. Where each record went, in order: none for
    /// one left deleted.
    async fn write(&self, writes: Transaction) -> Result<Vec<Option<RecordId>>, Refused> {
        let before = self.glue.stamp();
        let names = self.glue.apply(writes).await?;
        if let Some((stamp, _)) = self.seen.borrow_mut().as_mut()
            && *stamp == before
        {
            *stamp = self.glue.stamp();
        }
        Ok(names
            .iter()
            .map(|name| name.as_deref().and_then(RecordId::parse))
            .collect())
    }

    /// Whether the history kept whole is no longer the length it was when
    /// last taken up. One written to the same length shows up at the next
    /// listing, which reads it ([`InDb::legacy_look`]).
    fn legacy_changed(&self) -> bool {
        let layout = self.layout.borrow();
        let Some((layout, _)) = layout.as_ref() else {
            return false;
        };
        self.glue.legacy_len(self.keys.history).is_ok_and(|len| {
            layout
                .taken_up()
                .is_none_or(|taken| len != taken.map(|held| held.len))
        })
    }

    /// The deletions an older build remembers: none, where they cannot be
    /// read, which costs a session it deleted staying here, never one lost.
    fn legacy_deleted(&self) -> Vec<Deleted> {
        let raw = self.shelf.get(self.keys.deleted).ok().flatten();
        legacy::deleted_of(raw.as_deref().map(str::as_bytes))
    }

    /// Copy `unread`, what the history kept whole holds that does not
    /// read, aside: `what` to tell of, unless it was set aside before.
    fn set_aside(&self, unread: &str, what: SetAside) -> Option<SetAside> {
        (keep_aside(&self.shelf, self.keys.aside, unread) != Kept::Already).then_some(what)
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

/// The tombstones as the database holds them, each by the record its name
/// names, if it names one: a value that is no text names no session.
fn tombstones_in(stored: Stored) -> Vec<Tombstone> {
    stored
        .into_iter()
        .map(|(name, raw)| Tombstone {
            id: RecordId::parse(&name),
            read: raw.map_or(TombstoneRead::NoText, |raw| {
                TombstoneRead::Bytes(raw.into_bytes())
            }),
        })
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

/// The database as the record store reads and writes it ([`record_store`]):
/// each read and each change a request to the page script, awaited on the
/// page, the records read before the move, which is made over them in one
/// transaction, and the database left for the page storage where it will
/// not open or take the move ([`Backend::FALLS_BACK`]). It keeps one note
/// of what was told, `told`, read with how the history is kept; no other
/// ([`Backend::note`]).
struct InDb<'a, G, S>(&'a BrowserHistory<G, S>);

impl<G: Glue, S: Aside> Backend for InDb<'_, G, S> {
    type Refused = Refused;
    type Look = Looked;
    type Listed = ();
    const ORDER: Order = Order::ReadThenMove;
    const FALLS_BACK: bool = true;
    /// A record still unread after the app's tries is told of each time:
    /// the database notes no failure, as a refused read fails the whole
    /// read.
    const NOTES_STILL_UNREAD: bool = false;

    fn keys(&self) -> &RefCell<KeyCache> {
        &self.0.session_keys
    }

    /// By how the history is kept, as the database holds it beside the
    /// records told of, and how many records there are, counted for the
    /// boot screen: one a newer Dust keeps, as the page opened it at a
    /// version past this build's, is only read, and one whose head cannot
    /// be read at all is not read. One that does not read is moved again,
    /// over it ([`InDb::write_move`]).
    async fn open_gate(&self) -> Result<Opening, Refused> {
        let history = self.0;
        let read_only = !history.writable.get();
        let head = match history.glue.head().await {
            Ok(head) => head,
            // A newer Dust's database need hold nothing this build knows.
            Err(_) if read_only => return Ok(Opening::NewerUnread),
            Err(refusal) => return Err(refusal),
        };
        history.progress.reading(head.count);
        history.told.replace(head.told);
        let gate = if read_only {
            Gate::Newer
        } else {
            Gate::of(head.layout.as_deref().map(str::as_bytes))
        };
        history.layout.replace(None);
        history.held.replace(None);
        Ok(Opening::Gate(match gate {
            Gate::Open(layout) => {
                history
                    .layout
                    .replace(head.layout.map(|raw| ((*layout).clone(), raw)));
                Gate::Open(layout)
            }
            Gate::Newer => {
                history.writable.set(false);
                Gate::Newer
            }
            Gate::Unmoved | Gate::Unread => {
                history.held.replace(head.layout);
                Gate::Unmoved
            }
        }))
    }

    /// As the database was opened, or last written: one a newer Dust keeps
    /// is only read.
    async fn gate(&self) -> Result<Gate, Refused> {
        let history = self.0;
        if !history.writable.get() {
            return Ok(Gate::Newer);
        }
        Ok(match history.layout.borrow().as_ref() {
            Some((layout, _)) => Gate::Open(Box::new(layout.clone())),
            None => Gate::Unmoved,
        })
    }

    async fn list(&self) -> Result<Listing<()>, Refused> {
        let listed = self.0.glue.list().await?;
        let ids = |names: &[String]| {
            names
                .iter()
                .filter_map(|name| RecordId::parse(name))
                .collect()
        };
        Ok(Listing {
            records: ids(&listed.sessions),
            gone: ids(&listed.deleted),
            skipped: false,
            listed: (),
        })
    }

    /// The tombstones first, then every record, a page at a time, in
    /// order, each page counted ([`BrowserHistory::progress`]): those the
    /// tombstones delete read as deleted, and a name that names no record
    /// left alone. With `keep`, what each holds as it is stored, for a move
    /// made over them, a record left beside its tombstone among them, as
    /// the files count one: its session is here all the same, for the
    /// move, though the tombstone names none. A database only read reads
    /// what it can: no tombstones, where they cannot be read.
    async fn read_whole(&self, keep: bool) -> Result<WholeRead<()>, Refused> {
        let history = self.0;
        let tombstones = match history.glue.tombstones().await {
            Ok(tombstones) => tombstones_in(tombstones),
            Err(_) if !history.writable.get() => Vec::new(),
            Err(refusal) => return Err(refusal),
        };
        let gone: BTreeSet<RecordId> = tombstones
            .iter()
            .filter_map(|tombstone| tombstone.id)
            .collect();
        let mut records = BTreeSet::new();
        let mut read = Vec::new();
        let mut kept = BTreeMap::new();
        let mut after = None;
        let mut so_far = 0;
        loop {
            let page = history.glue.page(after.clone(), PAGE).await?;
            let more = page.len() == PAGE;
            so_far += page.len() as u64;
            for (name, held) in page {
                let id = RecordId::parse(&name);
                after = Some(name);
                let Some(id) = id else {
                    continue;
                };
                records.insert(id);
                if !gone.contains(&id) {
                    let session = held.as_deref().and_then(read_record);
                    let said = || held.as_deref().map(unreadable).unwrap_or_default();
                    read.push((id, Some(session.ok_or_else(said))));
                }
                if keep && let Some(held) = held {
                    kept.insert(id, held);
                }
            }
            history.progress.so_far(so_far);
            if !more {
                break;
            }
        }
        Ok(WholeRead {
            listing: Listing {
                records,
                gone,
                skipped: false,
                listed: (),
            },
            read,
            kept,
            tombstones,
        })
    }

    /// What each record named holds, as text; none where it is not there.
    /// One the database will not read reads as one that could not be read
    /// just then, each of them.
    async fn read(&self, ids: &[RecordId], _listed: Option<&()>) -> Vec<Raw> {
        let names = ids.iter().map(|id| id.name()).collect();
        let Ok(held) = self.0.glue.read(names).await else {
            return ids.iter().map(|_| Raw::Failed).collect();
        };
        held.into_iter()
            .chain(std::iter::repeat(None))
            .take(ids.len())
            .map(|held| held.map_or(Raw::Missing, Raw::Text))
            .collect()
    }

    async fn tombstones(&self) -> Result<Vec<Tombstone>, Refused> {
        Ok(tombstones_in(self.0.glue.tombstones().await?))
    }

    async fn tombstones_of(&self, ids: &[RecordId]) -> Option<TombstonesOf> {
        let names = ids.iter().map(|id| id.name()).collect();
        let held: HashMap<String, Option<String>> = self
            .0
            .glue
            .tombstones_of(names)
            .await
            .ok()?
            .into_iter()
            .collect();
        Some(
            ids.iter()
                .filter_map(|id| {
                    let raw = held.get(&id.name())?.clone();
                    Some((*id, raw.map(String::into_bytes)))
                })
                .collect(),
        )
    }

    /// The records told of, as read as the database was opened: the
    /// database keeps no other note. It notes no failure, as a refused read
    /// fails the whole read, and a record that reads in it is never damaged
    /// later, as text is all it holds.
    async fn note(&self, note: Note) -> Option<Vec<u8>> {
        match note {
            Note::Told => self.0.told.borrow().clone().map(String::into_bytes),
            Note::Failing | Note::UnreadTold | Note::DamagedTold => None,
        }
    }

    /// The records told of, noted beside what the note held: this build
    /// only adds names to it, as to told.json on a computer, so what a build
    /// before formats noted there, a record it cannot read that this one
    /// reads among them, stays, and such a build does not tell of it again.
    async fn write_note(&self, note: Note, names: Option<&BTreeSet<String>>) {
        let history = self.0;
        if note == Note::Told
            && let Some(names) = names
            && history.writable.get()
            && let Ok(raw) = serde_json::to_string(names)
        {
            let noted = Transaction {
                meta: vec![("told".into(), raw)],
                ..Transaction::default()
            };
            // Not noted, it is told of again: no more.
            let _ = history.write(noted).await;
        }
    }

    /// By the history kept whole, read whole, against how it was when last
    /// taken up: none of it, where the page storage cannot be read.
    async fn legacy_look(&self, layout: &Layout) -> LegacyLook {
        let Ok(raw) = self.0.shelf.get(self.0.keys.history) else {
            return LegacyLook::Same;
        };
        let fingerprint = raw.as_deref().map(fingerprint_of);
        if Some(fingerprint) == layout.taken_up() {
            return LegacyLook::Same;
        }
        LegacyLook::Changed(Some(kept_whole(raw)))
    }

    async fn legacy(&self) -> Result<Legacy, String> {
        let raw = self.0.shelf.get(self.0.keys.history);
        raw.map(kept_whole)
            .map_err(|()| "this browser's page storage could not be read".into())
    }

    async fn legacy_deleted(&self) -> Vec<Deleted> {
        self.0.legacy_deleted()
    }

    /// Copied aside, unless it was before: kept or not, it stays where it
    /// is too.
    async fn set_aside_history(&self, raw: &[u8]) -> (Option<SetAside>, bool) {
        let unread = std::str::from_utf8(raw).unwrap_or_default();
        let aside = self.0.set_aside(unread, SetAside::History { kept: None });
        (aside, false)
    }

    async fn set_aside_sessions(
        &self,
        unread: &str,
        count: usize,
    ) -> Result<Option<SetAside>, String> {
        Ok(self
            .0
            .set_aside(unread, SetAside::Sessions { count, kept: None }))
    }

    /// Made whole in one change, and not to be counted as it goes.
    fn moving(&self) {
        self.0.progress.moving(None);
    }

    /// In one transaction, how the history is kept with it, written only
    /// over the layout the database held as this copy last read or wrote
    /// it, where the change takes something up: two tabs taking it up at
    /// once take it up once ([`Refused::Raced`]). Any other change writes
    /// the layout again only should the browser have cleared the database
    /// meanwhile. Each record goes under the name the page script says it
    /// went under.
    async fn commit(&self, writes: &Writes<'_>, laid: Laid<'_>, _by: By) -> Made<Refused> {
        let history = self.0;
        let change = writes.change;
        let held = history.layout.borrow().as_ref().map(|(_, raw)| raw.clone());
        let (layout, next) = match laid {
            Laid::AsItIs => (held.map(LayoutWrite::Missing), None),
            Laid::Over(next) => {
                let Some(written) = layout_text(next) else {
                    return Made {
                        placed: Vec::new(),
                        saved: Err(Refused::Failed(UNWRITTEN.into())),
                    };
                };
                let layout = LayoutWrite::Over {
                    expect: held,
                    write: written.clone(),
                };
                (Some(layout), Some((next.clone(), written)))
            }
        };
        let transaction = Transaction {
            put: named(&change.put),
            forget: writes
                .forget
                .iter()
                .map(|(id, held)| (id.name(), held.clone()))
                .collect(),
            unforget: change.unforget.iter().map(|id| id.name()).collect(),
            layout,
            meta: Vec::new(),
        };
        match history.write(transaction).await {
            Ok(placed) => {
                if let Some(next) = next {
                    history.layout.replace(Some(next));
                }
                Made {
                    placed,
                    saved: Ok(HistorySaved::Stored {
                        kept: change.put.len(),
                    }),
                }
            }
            Err(refused) => {
                if refused == Refused::Newer {
                    history.writable.set(false);
                }
                Made {
                    placed: Vec::new(),
                    saved: Err(refused),
                }
            }
        }
    }

    /// In one transaction, with how the history is kept, written over the
    /// layout the database held as it was opened, which is set aside beside
    /// it where there was one: a move cut short is no move, and two tabs
    /// making it at once make it once.
    async fn write_move(
        &self,
        records: &[Record],
        layout: &Layout,
    ) -> Result<Vec<Option<RecordId>>, Refused> {
        let history = self.0;
        let Some(written) = layout_text(layout) else {
            return Err(Refused::Failed(UNWRITTEN.into()));
        };
        let held = history.held.borrow().clone();
        let mut meta = Vec::new();
        if let Some(held) = &held {
            let aside = format!("layout.unreadable-{}", layout.migrated_at);
            meta.push((aside, held.clone()));
        }
        let transaction = Transaction {
            put: named(records),
            layout: Some(LayoutWrite::Over {
                expect: held,
                write: written.clone(),
            }),
            meta,
            ..Transaction::default()
        };
        let placed = history.write(transaction).await?;
        history.layout.replace(Some((layout.clone(), written)));
        Ok(placed)
    }

    async fn raced(&self) {
        self.0.read_layout().await;
    }

    fn look(&self) -> Looked {
        self.0.looking()
    }

    fn saw(&self, look: Looked) {
        self.0.seen.replace(Some(look));
    }
}

/// Why a change that would note how the history is kept was not made.
const UNWRITTEN: &str = "how the history is kept could not be written down";

/// The history kept whole in the page storage, `raw`, as the record store
/// takes it, with what tells it from another state of it.
fn kept_whole(raw: Option<String>) -> Legacy {
    raw.map(|raw| {
        let fingerprint = fingerprint_of(&raw);
        (raw.into_bytes(), fingerprint)
    })
}

#[cfg(feature = "web")]
pub(super) use script::{Script, waited_ms};

/// The page script, run on the page.
#[cfg(feature = "web")]
mod script {
    use std::cell::Cell;

    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{
        Glue, Head, IDB_JS, LayoutWrite, Listed, Pending, Refusal, Refused, Stored, Transaction,
    };

    /// What the page script hands back: what the history does with the
    /// database ([`IDB_JS`]).
    pub(crate) struct Script(JsValue);

    impl Script {
        /// Run the page script and open the database: the script, and
        /// whether a newer Dust keeps the database, so that this one only
        /// reads it; or why it would not open.
        pub(crate) async fn open() -> Result<(Self, bool), String> {
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

    thread_local! {
        /// When the script was last asked something, or last answered, by
        /// the page's clock ([`waited_ms`]).
        static STIRRED: Cell<f64> = const { Cell::new(0.0) };
    }

    /// How long the script has kept the page waiting, in milliseconds: since
    /// it was last asked something, or last answered.
    pub(crate) fn waited_ms() -> f64 {
        js_sys::Date::now() - STIRRED.with(Cell::get)
    }

    /// Ask the script's `name`, with `args`, and wait for its answer.
    async fn asked(glue: JsValue, name: &str, args: Vec<JsValue>) -> Result<JsValue, Refused> {
        let function = function(&glue, name)
            .ok_or_else(|| Refused::Failed(format!("its script has no {name}")))?;
        let args: js_sys::Array = args.into_iter().collect();
        STIRRED.with(|stirred| stirred.set(js_sys::Date::now()));
        let promise = function
            .apply(&glue, &args)
            .map_err(|error| Refused::Failed(said(&error)))?;
        let answer = JsFuture::from(js_sys::Promise::resolve(&promise)).await;
        STIRRED.with(|stirred| stirred.set(js_sys::Date::now()));
        let answer = answer.map_err(|error| Refused::Failed(said(&error)))?;
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
    fn change(writes: &Transaction) -> JsValue {
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
                    count: field(&head, "count").as_f64().map(|count| count as u64),
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

        fn tombstones_of(&self, keys: Vec<String>) -> Pending<Result<Stored, Refused>> {
            let glue = self.0.clone();
            Box::pin(async move {
                Ok(pairs(
                    &asked(glue, "tombstonesOf", vec![names(&keys).into()]).await?,
                ))
            })
        }

        fn apply(&self, writes: Transaction) -> Pending<Result<Vec<Option<String>>, Refused>> {
            let glue = self.0.clone();
            Box::pin(async move { Ok(texts(&asked(glue, "apply", vec![change(&writes)]).await?)) })
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
