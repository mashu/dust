//! The history's records, kept the one way whatever keeps them: the files
//! ([`super::files`]), a browser's database and the test store each do what
//! their own protocol makes of a read or a write ([`Backend`]), and this
//! decides the rest, once, for every one of them. It reads the history
//! whole, making the move from a history kept whole where none is made
//! ([`load`]); makes a change ([`apply`]); lists the records, taking up what
//! an older build stored or deleted since ([`listing`]); reads them, and
//! says which were deleted ([`read`], [`deletions`]); and tells of those it
//! cannot read, once ([`notes`]).
//!
//! What every store does alike with what it read is here too: the records
//! read told apart, as sessions, records this build cannot read and records
//! that could not be read just then ([`sort`]); and what the tombstones
//! say, of which records were deleted by hand, when, and which sessions
//! they held ([`keys_of_tombstones`], [`deletions_of`],
//! [`deleted_sessions_of`]).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use cw_core::SessionResult;

use super::legacy::{self, Fingerprint, Gate, Layout, read_history};
use super::record::{
    Deletions, HistoryChange, HistoryLoad, HistoryNotice, Record, RecordId, Unopened, Unreadable,
    held_key, key_of, keyed, taken_up, tombstone_at, tombstone_key, unread_notices,
};
use super::saved::HistorySaved;
use super::session::{Deleted, SessionKey};
use super::set_aside::SetAside;
use notes::Note;

pub(super) mod notes;
#[cfg(any(feature = "native-runtime", test))]
pub(super) mod steps;

/// What is stored of a record, as a store reads it.
#[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
pub(super) enum Raw {
    /// Its text.
    Text(String),
    /// So many bytes that are no text, as damage to a disk leaves them: no
    /// build reads them, now or later.
    NoText(u64),
    /// Nothing, for now: it could not be read just then.
    Failed,
    /// Nothing: it is not there, as a browser's database says of a record
    /// gone since it was listed.
    #[cfg_attr(not(any(feature = "web", test)), allow(dead_code))]
    Missing,
}

#[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
impl Raw {
    /// What is stored, where it is text.
    pub(super) fn text(self) -> Option<String> {
        match self {
            Self::Text(raw) => Some(raw),
            Self::NoText(_) | Self::Failed | Self::Missing => None,
        }
    }

    pub(super) fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// What a read of record `id` hands back of what is stored
    /// ([`super::Store::read_records`]): bytes that are no text are a
    /// damaged record's, as the history read whole takes such a record.
    pub(super) fn record(self, id: RecordId) -> Result<Record, Unopened> {
        match self {
            Self::Text(json) => Ok(Record { id, json }),
            Self::NoText(_) => Err(Unopened::Damaged(id)),
            Self::Failed | Self::Missing => Err(Unopened::Failed(id)),
        }
    }
}

/// A record as a store read it: the session it holds, or, where it holds
/// none this build reads, what it says of itself ([`Unreadable`]). None,
/// where it could not be read at all just then.
pub(super) type RecordRead = Option<Result<SessionResult, Unreadable>>;

/// The key of each record read so far, by its id: what an older build's
/// deletions are told apart by, without reading every record again each
/// time what it changed is taken up.
pub(super) type KeyCache = HashMap<RecordId, SessionKey>;

/// Records read, told apart ([`sort`]), each kind in the order read.
#[derive(Default)]
pub(super) struct Sorted {
    /// Every session that reads, by its record.
    pub(super) sessions: Vec<(RecordId, SessionResult)>,
    /// Every record that holds no session this build reads, with what it
    /// says of itself.
    pub(super) unread: Vec<(RecordId, Unreadable)>,
    /// Of those, the ones a later build stored ([`Unreadable::later`]),
    /// each beside the session it says it holds.
    pub(super) later: Vec<(RecordId, SessionKey)>,
    /// Those that could not be read at all just then.
    pub(super) failed: Vec<RecordId>,
}

/// `read`, each record as a store read it, told apart, the key of each that
/// holds a session noted in `keys`: one that reads holds its own, and one a
/// later build stored holds the one it says it holds, so an older build's
/// copy of that session is no session to take up beside it. A damaged one
/// holds none, though it may say one ([`Unreadable::later_key`]).
pub(super) fn sort(
    read: impl IntoIterator<Item = (RecordId, RecordRead)>,
    keys: &mut KeyCache,
) -> Sorted {
    let mut sorted = Sorted::default();
    for (id, read) in read {
        match read {
            Some(Ok(session)) => {
                keys.insert(id, SessionKey::of(&session));
                sorted.sessions.push((id, session));
            }
            Some(Err(said)) => {
                if let Some(key) = said.later_key() {
                    keys.insert(id, key.clone());
                    sorted.later.push((id, key.clone()));
                }
                sorted.unread.push((id, said));
            }
            None => sorted.failed.push(id),
        }
    }
    sorted
}

/// Every record of `sessions`, and of `later`, those a later build stored,
/// by the session each holds: what an older build's copy of a session is
/// told apart from, as it is taken up ([`super::legacy::imported`]).
pub(super) fn live_of(
    sessions: &[(RecordId, SessionResult)],
    later: &[(RecordId, SessionKey)],
) -> HashMap<SessionKey, Vec<RecordId>> {
    let mut live = keyed(sessions.iter().map(|(id, session)| (*id, session)));
    for (id, key) in later {
        live.entry(key.clone()).or_default().push(*id);
    }
    live
}

/// A tombstone, as a store read it: the record it deletes, where its name
/// names one, and what it holds.
pub(super) struct Tombstone {
    #[cfg_attr(not(any(feature = "web", test)), allow(dead_code))]
    pub(super) id: Option<RecordId>,
    pub(super) read: TombstoneRead,
}

/// What a tombstone holds, as a store read it.
pub(super) enum TombstoneRead {
    /// What was written for it ([`super::Deleted`]): one that is empty, or
    /// does not read, deletes its record all the same, and names no
    /// session.
    Bytes(Vec<u8>),
    /// A value that is no text, as a later build may store in a browser's
    /// database: it names no session.
    #[cfg_attr(not(any(feature = "web", test)), allow(dead_code))]
    NoText,
    /// Nothing, for now, for the reason given: it is there, and could not
    /// be read just then, as a folder in its place cannot be, or a file
    /// held open. It may name a session to keep deleted.
    #[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
    Failed(String),
}

/// How a tombstone that could not be read just then is taken
/// ([`keys_of_tombstones`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reading {
    /// As one that may name a session to keep deleted: what waits on the
    /// sessions deleted here, a take-up of what an older build changed and
    /// a move made over records, waits for it to read.
    Strict,
    /// As one that names none, as any that does not read: which sessions
    /// a tombstone remembers is answered without it
    /// ([`super::Store::deleted_keys`]).
    Lenient,
}

/// The key of every session `tombstones` name. One that names none, as an
/// empty one, or one whose value is no text, names none; one that could
/// not be read just then names none where it is read [`Reading::Lenient`],
/// and holds the answer back where it is read [`Reading::Strict`]: why it
/// could not be read, of the first.
pub(super) fn keys_of_tombstones(
    tombstones: &[Tombstone],
    reading: Reading,
) -> Result<HashSet<SessionKey>, String> {
    let mut keys = HashSet::new();
    for tombstone in tombstones {
        match &tombstone.read {
            TombstoneRead::Bytes(raw) => keys.extend(tombstone_key(raw)),
            TombstoneRead::NoText => {}
            TombstoneRead::Failed(why) => {
                if reading == Reading::Strict {
                    return Err(why.clone());
                }
            }
        }
    }
    Ok(keys)
}

/// Which of the sessions `keys` are among `gone`, the sessions the
/// tombstones name, in the order of `keys` ([`super::Store::deleted_keys`]).
pub(super) fn deleted_keys_of(
    gone: &HashSet<SessionKey>,
    keys: impl IntoIterator<Item = SessionKey>,
) -> Vec<SessionKey> {
    keys.into_iter().filter(|key| gone.contains(key)).collect()
}

/// The records a look found deleted by hand, each beside what its
/// tombstone holds: none where it holds nothing that could be read, as one
/// there that could not be read just then, or whose value is no text.
pub(super) type TombstonesOf = Vec<(RecordId, Option<Vec<u8>>)>;

/// Of `found`, the records with a tombstone, when each says it was deleted:
/// none for one that does not say, as one that is empty, or does not read
/// ([`super::Store::deletions`]).
pub(super) fn deletions_of(found: TombstonesOf) -> Deletions {
    found
        .into_iter()
        .map(|(id, raw)| (id, raw.as_deref().and_then(tombstone_at)))
        .collect()
}

/// Of `found`, the records with a tombstone, each beside the session its
/// tombstone remembers: one whose tombstone does not say, as one that is
/// empty, or does not read, is left out ([`super::Store::deleted_sessions`]).
pub(super) fn deleted_sessions_of(found: TombstonesOf) -> Vec<(RecordId, SessionKey)> {
    found
        .into_iter()
        .filter_map(|(id, raw)| Some((id, tombstone_key(&raw?)?)))
        .collect()
}

/// How many times the move is tried as the history is read, where another
/// copy moves it first each time ([`Refusal::raced`]): once is all it
/// takes, as every copy moves the same records.
pub(super) const MOVE_TRIES: usize = 3;

/// Why a store does not keep the history this visit, as a browser's
/// database would not ([`Backend::FALLS_BACK`]), and what reading the
/// history kept whole set aside meanwhile, as it could not read it.
#[derive(Debug, PartialEq)]
pub(super) struct NotOpened {
    pub(super) why: String,
    pub(super) set_aside: Vec<SetAside>,
}

/// Why a move was not made.
enum Moving {
    /// Another copy made it first, in the store's words.
    Raced(String),
    /// The store would not take it, for the reason given.
    Refused(String),
}

/// A take-up of what an older build changed that could not be made just
/// now ([`take_up`]): it is still to take up ([`Backend::held_back`]).
struct HeldBack;

/// Every record here by the session it holds.
type Live = HashMap<SessionKey, Vec<RecordId>>;

/// Why a store did not do what was asked, as it says it.
pub(super) trait Refusal {
    /// What a change refused so comes to ([`HistorySaved`]).
    fn saved(&self) -> HistorySaved;
    /// Why, in words: what a move pending says, or a store that keeps the
    /// history elsewhere this visit.
    fn words(&self) -> String;
    /// Another copy changed how the history is kept first: nothing was
    /// done, and how it is kept is read again ([`Backend::raced`]).
    fn raced(&self) -> bool {
        false
    }
}

/// When a store reads its records, beside the move from the history kept
/// whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Order {
    /// The move first, each step of it made durable on its own, and then
    /// the records, what it made among them: the files, the test store.
    #[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
    MoveThenRead,
    /// The records first, and the move over them, in one change that two
    /// copies making it at once make once: a browser's database.
    ReadThenMove,
}

/// How a store opens as the history is read whole ([`Backend::open_gate`]).
pub(super) enum Opening {
    /// By the gate how the history is kept sets.
    Gate(Gate),
    /// A newer Dust keeps it, and even how it is kept could not be read:
    /// nothing is read of it, and nothing written.
    #[cfg_attr(not(any(feature = "web", test)), allow(dead_code))]
    NewerUnread,
}

/// The history kept whole, as an older build keeps it, and what tells it
/// from another state of it: none, where there is none.
pub(super) type Legacy = Option<(Vec<u8>, Fingerprint)>;

/// Whether the history kept whole changed since it was last taken up
/// ([`Backend::legacy_look`]).
pub(super) enum LegacyLook {
    /// No; or it cannot be looked at now: nothing is taken up.
    Same,
    /// Yes, with what it holds, where the look read it.
    Changed(Option<Legacy>),
}

/// What a store holds, listed: every record, loose or packed, in order,
/// and every tombstone.
pub(super) struct Listing<L> {
    pub(super) records: BTreeSet<RecordId>,
    pub(super) gone: BTreeSet<RecordId>,
    /// Some of it could not be opened, as a pack of records, and what it
    /// holds is listed nowhere.
    pub(super) skipped: bool,
    /// What the listing found beside, for a read of what it listed
    /// ([`Backend::read`]).
    pub(super) listed: L,
}

/// The records a store read whole, as a browser's database reads them a
/// page at a time ([`Backend::read_whole`]).
pub(super) struct WholeRead<L> {
    pub(super) listing: Listing<L>,
    /// Every record not deleted, as it read, in order.
    pub(super) read: Vec<(RecordId, RecordRead)>,
    /// With `keep`, what every record holds as it is stored, one beside its
    /// tombstone among them, for a move made over them.
    pub(super) kept: BTreeMap<RecordId, String>,
    /// The tombstones, as they were read before the records.
    pub(super) tombstones: Vec<Tombstone>,
}

/// A change as a store makes it: the change, and what each of its
/// tombstones holds, as it is written ([`super::Deleted`]), each record
/// deleted once, as the first of those that name it.
pub(super) struct Writes<'a> {
    pub(super) change: &'a HistoryChange,
    pub(super) forget: Vec<(RecordId, String)>,
}

impl<'a> Writes<'a> {
    /// `change` as a store makes it: a tombstone that cannot be written
    /// down fails the change.
    pub(super) fn of(change: &'a HistoryChange) -> serde_json::Result<Self> {
        let mut named = HashSet::new();
        let mut forget = Vec::with_capacity(change.forget.len());
        for (id, deleted) in &change.forget {
            if named.insert(*id) {
                forget.push((*id, serde_json::to_string(deleted)?));
            }
        }
        Ok(Self { change, forget })
    }

    /// The tombstones, each with what it holds.
    #[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
    pub(super) fn tombstones(&self) -> Vec<(RecordId, &str)> {
        self.forget
            .iter()
            .map(|(id, held)| (*id, held.as_str()))
            .collect()
    }
}

/// How the history is kept, as a change writes it ([`Backend::commit`]).
pub(super) enum Laid<'a> {
    /// As it is: written again only where a store lost it, as a browser's
    /// cleared database does.
    AsItIs,
    /// As `next` says, once the change is made: what a take-up took is
    /// noted in it. `from` is how the history was kept as the take-up was
    /// worked out from it: a store that may take up more than one at once
    /// writes it over nothing else ([`Backend::commit`]).
    Over {
        #[cfg_attr(not(any(feature = "web", test)), allow(dead_code))]
        from: &'a Layout,
        next: &'a Layout,
    },
}

/// Whose change a store makes ([`Backend::commit`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum By {
    /// The app's: the store's own look is no account of it, but as each
    /// store keeps that account.
    App,
    /// The store's own, as it reads or lists the history: no change for it
    /// to take up.
    Store,
}

/// What became of a change a store made ([`Backend::commit`]): where each
/// of its records went, as far as it got, none for one left deleted; and
/// what it came to, or why it was not made.
pub(super) struct Made<R> {
    pub(super) placed: Vec<Option<RecordId>>,
    pub(super) saved: Result<HistorySaved, R>,
}

/// What a store holds as the move is made over it ([`Here::not_moved_yet`]):
/// every record, every tombstone by name and by the session it names, and
/// what records hold, those under a name the move does not give among
/// them.
#[derive(Default)]
pub(super) struct Here {
    pub(super) records: BTreeSet<RecordId>,
    pub(super) gone: BTreeSet<RecordId>,
    pub(super) texts: BTreeMap<RecordId, String>,
    pub(super) gone_keys: HashSet<SessionKey>,
}

impl Here {
    /// Of `moved`, the records a move makes, those to write
    /// ([`legacy::not_moved_yet`]): all of them, where nothing is here.
    fn not_moved_yet(&self, moved: Vec<Record>) -> Vec<Record> {
        if self.records.is_empty() && self.gone.is_empty() && self.gone_keys.is_empty() {
            return moved;
        }
        let names: BTreeSet<RecordId> = moved.iter().map(|record| record.id).collect();
        let elsewhere = self
            .texts
            .iter()
            .filter(|(id, _)| !names.contains(id))
            .map(|(_, json)| json);
        legacy::not_moved_yet(
            moved,
            |id| self.records.contains(&id),
            &self.gone,
            elsewhere,
            &self.gone_keys,
        )
    }
}

/// What a store does with what keeps the history: its reads, its writes,
/// and its own account of what it has seen, each as its protocol makes it;
/// the record store decides the rest, once, for every store.
pub(super) trait Backend {
    /// Why it did not do what was asked.
    type Refused: Refusal;
    /// What a look at it saw ([`Self::look`]).
    type Look;
    /// What a listing found beside the records ([`Listing::listed`]).
    type Listed: Default;
    /// When it reads its records beside the move.
    const ORDER: Order;
    /// A refused read as the history is read whole is no history read: the
    /// store falls back to another for the visit ([`NotOpened`]).
    const FALLS_BACK: bool;
    /// It notes the records still unread after the app's tries, so each is
    /// told of once ([`still_unread`]).
    #[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
    const NOTES_STILL_UNREAD: bool;

    /// The key of each record read so far ([`KeyCache`]).
    fn keys(&self) -> &RefCell<KeyCache>;
    /// How the history is kept, as it is read whole.
    async fn open_gate(&self) -> Result<Opening, Self::Refused> {
        self.gate().await.map(Opening::Gate)
    }
    /// How the history is kept: whether this build may write to it.
    async fn gate(&self) -> Result<Gate, Self::Refused>;
    /// Whether this build may write to it now.
    async fn may_write(&self) -> bool {
        matches!(self.gate().await, Ok(Gate::Open(_) | Gate::Unmoved))
    }
    /// Every record and tombstone there is.
    async fn list(&self) -> Result<Listing<Self::Listed>, Self::Refused>;
    /// The records `listing` found but those deleted, read whole, as the
    /// history is read whole ([`Order::MoveThenRead`]).
    async fn read_listed(&self, _listing: &Listing<Self::Listed>) -> Vec<(RecordId, RecordRead)> {
        Vec::new()
    }
    /// Every record read whole, the tombstones first, with what each holds
    /// where `keep` is set ([`Order::ReadThenMove`]).
    async fn read_whole(&self, _keep: bool) -> Result<WholeRead<Self::Listed>, Self::Refused> {
        let listing = self.list().await?;
        let read = self.read_listed(&listing).await;
        Ok(WholeRead {
            listing,
            read,
            kept: BTreeMap::new(),
            tombstones: Vec::new(),
        })
    }
    /// What is stored for each of `ids`, in their order: by what `listed`
    /// found, where it is given, as what a listing found is read.
    async fn read(&self, ids: &[RecordId], listed: Option<&Self::Listed>) -> Vec<Raw>;
    /// Every tombstone there is, and what each holds.
    async fn tombstones(&self) -> Result<Vec<Tombstone>, Self::Refused>;
    /// The tombstone of each of `ids` that has one, and what it holds: no
    /// word at all, where one could not be looked for.
    async fn tombstones_of(&self, ids: &[RecordId]) -> Option<TombstonesOf>;
    /// What `note` holds: none, where it is not there, or this store keeps
    /// no such note.
    async fn note(&self, note: Note) -> Option<Vec<u8>>;
    /// Have `note` hold `names`, or, given none, take it away: nothing,
    /// where this store keeps no such note.
    async fn write_note(&self, note: Note, names: Option<&BTreeSet<String>>);
    /// Whether the history kept whole changed since `layout` took it up.
    async fn legacy_look(&self, layout: &Layout) -> LegacyLook;
    /// The history kept whole, or why it cannot be read now.
    async fn legacy(&self) -> Result<Legacy, String>;
    /// The deletions an older build remembers.
    async fn legacy_deleted(&self) -> Vec<Deleted>;
    /// Set aside `raw`, a history kept whole that does not read, as the
    /// move finds it: what to tell of, unless it was set aside before, and
    /// whether it is gone from where older builds keep it.
    async fn set_aside_history(&self, raw: &[u8]) -> (Option<SetAside>, bool);
    /// Set aside `unread`, `count` sessions of the history kept whole that
    /// do not read: what to tell of, unless they were set aside before; or
    /// why they could not be.
    async fn set_aside_sessions(
        &self,
        unread: &str,
        count: usize,
    ) -> Result<Option<SetAside>, String>;
    /// Every record and tombstone there is, by name, as a move made over
    /// them looks at them ([`Order::MoveThenRead`]).
    async fn here(&self) -> Result<(BTreeSet<RecordId>, BTreeSet<RecordId>), Self::Refused> {
        Ok(Default::default())
    }
    /// What each of `ids` holds, as text, as a move made over them reads
    /// them: one gone, or whose bytes are no text, holds nothing; one that
    /// cannot be read now holds the move back.
    async fn texts(&self, _ids: &[RecordId]) -> Result<Vec<(RecordId, String)>, Self::Refused> {
        Ok(Vec::new())
    }
    /// The move is being made.
    fn moving(&self) {}
    /// What a change cut short, as Dust stopped, left: what to tell of.
    async fn cut_short(&self) -> Vec<HistoryNotice> {
        Vec::new()
    }
    /// Take away `ids`, records left beside their tombstones.
    async fn unlink(&self, _ids: &[RecordId]) {}
    /// Make `writes`, and lay how the history is kept as `laid` says.
    async fn commit(&self, writes: &Writes<'_>, laid: Laid<'_>, by: By) -> Made<Self::Refused>;
    /// Store `records`, the move's, and `layout` with them, last: where
    /// each went, in their order ([`Made::placed`]).
    async fn write_move(
        &self,
        records: &[Record],
        layout: &Layout,
    ) -> Result<Vec<Option<RecordId>>, Self::Refused>;
    /// Another copy changed how the history is kept first.
    async fn raced(&self) {}
    /// What a look at the store sees now.
    fn look(&self) -> Self::Look;
    /// Note what `look` saw as seen: what changes after is to take up.
    fn saw(&self, look: Self::Look);
    /// What an older build changed is still to take up, though a look saw
    /// it ([`HeldBack`]).
    fn held_back(&self) {}
    /// The history read, before it is handed over.
    fn loaded(&self, _load: &mut HistoryLoad) {}
}

/// `records` under the names they went under, as `placed` says, in their
/// order ([`Made::placed`]): one left deleted is none of them.
fn as_placed(records: &[Record], placed: &[Option<RecordId>]) -> Vec<Record> {
    records
        .iter()
        .zip(placed)
        .filter_map(|(record, id)| {
            Some(Record {
                id: (*id)?,
                json: record.json.clone(),
            })
        })
        .collect()
}

/// Note what `change`, as `placed` says its records went, made: the key of
/// each record stored, and, where it was made, the deletions.
fn noted(
    keys: &RefCell<KeyCache>,
    change: &HistoryChange,
    placed: &[Option<RecordId>],
    made: bool,
) {
    let mut keys = keys.borrow_mut();
    for (record, id) in change.put.iter().zip(placed) {
        if let Some(id) = id
            && let Some(key) = key_of(&record.json)
        {
            keys.insert(*id, key);
        }
    }
    if made {
        for (id, _) in &change.forget {
            keys.remove(id);
        }
    }
}

/// The whole history ([`super::Store::load_history`]): moved first, where
/// the store holds none yet, and with what an older build stored since
/// taken up. Or, from a store that falls back to another, why it does not
/// keep it this visit.
pub(super) async fn load<B: Backend>(store: &B) -> Result<HistoryLoad, NotOpened> {
    let mut set_aside = Vec::new();
    let mut tries = 0;
    loop {
        tries += 1;
        match read_in(store, &mut set_aside).await {
            Ok(mut load) => {
                load.set_aside = set_aside;
                store.loaded(&mut load);
                return Ok(load);
            }
            Err(Moving::Raced(_)) if tries < MOVE_TRIES => {}
            Err(Moving::Raced(why) | Moving::Refused(why)) => {
                return Err(NotOpened { why, set_aside });
            }
        }
    }
}

/// The history read in, the move made first where none is made: from a
/// store that reads its records first, over what they hold
/// ([`Order::ReadThenMove`]).
async fn read_in<B: Backend>(
    store: &B,
    set_aside: &mut Vec<SetAside>,
) -> Result<HistoryLoad, Moving> {
    let read_first = B::ORDER == Order::ReadThenMove;
    let mut load = HistoryLoad::default();
    // As of before it is read: what another copy changes after that is a
    // change to take up.
    if read_first {
        store.saw(store.look());
    }
    let gate = match store.open_gate().await {
        Ok(Opening::Gate(gate)) => Ok(gate),
        // A newer Dust's store need hold nothing this build knows: nothing
        // is read of it, and nothing written.
        Ok(Opening::NewerUnread) => {
            load.notices.push(HistoryNotice::Newer);
            return Ok(load);
        }
        Err(refused) => Err(refused),
    };
    // How the history is kept, once moved, and whether this build may
    // write to it at all.
    let (mut layout, writable) = match gate {
        Ok(Gate::Open(layout)) => (Some(*layout), true),
        Ok(Gate::Unmoved) => (None, true),
        Ok(Gate::Newer) => {
            load.notices.push(HistoryNotice::Newer);
            (None, false)
        }
        Ok(Gate::Unread) => (None, false),
        Err(refused) if B::FALLS_BACK => return Err(Moving::Refused(refused.words())),
        Err(refused) => {
            eprintln!(
                "could not read how the history is kept: {}",
                refused.words()
            );
            (None, false)
        }
    };
    let moving = writable && layout.is_none();
    if moving && !read_first {
        layout = move_in(store, None, &mut load, set_aside)
            .await?
            .map(|(layout, _)| layout);
    }
    if writable {
        load.notices.extend(store.cut_short().await);
    }
    // What the tombstones name before the records are read, for a take-up
    // of what an older build changed: a session another copy deletes
    // meanwhile is either still here or deleted after, and one it puts back
    // meanwhile is either deleted before or here.
    let before = match &layout {
        Some(layout) if !read_first => match store.legacy_look(layout).await {
            LegacyLook::Changed(_) => Some(store.tombstones().await),
            LegacyLook::Same => None,
        },
        _ => None,
    };
    let (sorted, skipped, before) = if read_first {
        let whole = match store.read_whole(moving).await {
            Ok(whole) => whole,
            Err(refused) if B::FALLS_BACK && writable => {
                return Err(Moving::Refused(refused.words()));
            }
            Err(refused) => {
                eprintln!("could not list the history: {}", refused.words());
                return Ok(load);
            }
        };
        let mut sorted = sort(whole.read, &mut store.keys().borrow_mut());
        if moving {
            // Read again after the records: a session another copy deleted
            // meanwhile, under a name the move does not give it, is no
            // session to move in. Nor is one it put back meanwhile, by Undo
            // or a restore, there under such a name: the tombstones read
            // before the records name it by its key. By name the later read
            // alone says what is deleted: a record put back before the
            // records were read is here, read with them.
            let after = match store.tombstones().await {
                Ok(after) => after,
                Err(refused) => return Err(Moving::Refused(refused.words())),
            };
            let mut gone_keys =
                keys_of_tombstones(&whole.tombstones, Reading::Strict).map_err(Moving::Refused)?;
            gone_keys.extend(keys_of_tombstones(&after, Reading::Strict).map_err(Moving::Refused)?);
            let here = Here {
                records: whole.kept.keys().copied().collect(),
                gone: after.iter().filter_map(|tombstone| tombstone.id).collect(),
                texts: whole.kept,
                gone_keys,
            };
            if let Some((moved, records)) = move_in(store, Some(here), &mut load, set_aside).await?
            {
                let change = HistoryChange {
                    put: records,
                    ..HistoryChange::default()
                };
                taken_up(&mut sorted.sessions, &change);
                layout = Some(moved);
            }
        }
        let before = keys_of_tombstones(&whole.tombstones, Reading::Strict);
        (sorted, whole.listing.skipped, Some(before))
    } else {
        // As the folders were before they were read: whatever another copy
        // changes from here on is taken up at the next look. Where they
        // cannot be listed, nothing was seen, and the next look lists them.
        // Noted as seen once they are listed, so that what this read stores
        // itself, as it finishes a deletion cut short or takes up what an
        // older build stored, is noted as its own change, none to take up.
        let looked = store.look();
        let listing = match store.list().await {
            Ok(listing) => listing,
            Err(refused) => {
                eprintln!("could not list the history: {}", refused.words());
                return Ok(load);
            }
        };
        store.saw(looked);
        // Deleted, and the record left behind: a deletion cut short, which
        // this copy finishes, where it may write.
        let beside: Vec<RecordId> = listing
            .records
            .intersection(&listing.gone)
            .copied()
            .collect();
        if writable && !beside.is_empty() {
            store.unlink(&beside).await;
        }
        let read = store.read_listed(&listing).await;
        let before = before.map(|before| {
            before
                .map_err(|refused| refused.words())
                .and_then(|tombstones| keys_of_tombstones(&tombstones, Reading::Strict))
        });
        (
            sort(read, &mut store.keys().borrow_mut()),
            listing.skipped,
            before,
        )
    };
    load.notices
        .extend(tell(store, &sorted.unread, &sorted.failed, writable).await);
    let mut sessions = sorted.sessions;
    // Nothing is taken up from an older build while a record here cannot
    // be read for now, nor all of them listed: it may hold a session the
    // older build holds too. A later look takes it up, once every record
    // reads ([`listing`]).
    if let Some(layout) = &layout
        && sorted.failed.is_empty()
        && !skipped
    {
        let live = live_of(&sessions, &sorted.later);
        let taken = match before.unwrap_or_else(|| Ok(HashSet::new())) {
            Ok(before) => take_up(store, layout, Some((live, before))).await,
            Err(why) => {
                eprintln!("could not read what was deleted here: {why}");
                Err(HeldBack)
            }
        };
        match taken {
            Ok(Some(change)) => taken_up(&mut sessions, &change),
            Ok(None) => {}
            Err(HeldBack) => store.held_back(),
        }
    }
    load.sessions = sessions;
    load.unread = sorted.unread.into_iter().map(|(id, _)| id).collect();
    load.later = sorted.later;
    load.failed = sorted.failed;
    load.complete = layout.is_some();
    Ok(load)
}

/// Make the move from the history kept whole, over `here`, what the store
/// holds already, where it read it with the records; and say how the
/// history is kept now, with the records the move wrote. None, where it
/// cannot be made yet: why is told of, and the next read tries again.
///
/// What does not read is set aside first, as a history kept whole always
/// was: one that is no list is set aside, and the sessions that do not
/// read are copied out beside it. Every session that reads becomes a
/// record, but what is here already ([`Here::not_moved_yet`]), and the
/// layout is written last, or with them, so a move cut short is made again,
/// to the same records. The history kept whole is never written: older
/// builds keep theirs in it.
async fn move_in<B: Backend>(
    store: &B,
    here: Option<Here>,
    load: &mut HistoryLoad,
    set_aside: &mut Vec<SetAside>,
) -> Result<Option<(Layout, Vec<Record>)>, Moving> {
    let legacy = match store.legacy().await {
        Ok(legacy) => legacy,
        Err(why) => {
            load.notices.push(HistoryNotice::MovePending(why));
            return Ok(None);
        }
    };
    store.moving();
    let (raw, mut fingerprint) = match legacy {
        Some((raw, fingerprint)) => (Some(raw), Some(fingerprint)),
        None => (None, None),
    };
    let read = raw
        .as_deref()
        .map(|raw| std::str::from_utf8(raw).ok().and_then(read_history));
    let kept_whole = matches!(read, Some(Some(_)));
    let sessions = match read {
        None => Vec::new(),
        Some(None) => {
            let (aside, gone) = store
                .set_aside_history(raw.as_deref().unwrap_or_default())
                .await;
            if gone {
                fingerprint = None;
            }
            set_aside.extend(aside);
            Vec::new()
        }
        Some(Some(read)) => {
            if let Some(unread) = read.unread_json() {
                match store.set_aside_sessions(&unread, read.unread.len()).await {
                    Ok(aside) => set_aside.extend(aside),
                    Err(why) => {
                        load.notices.push(HistoryNotice::MovePending(why));
                        return Ok(None);
                    }
                }
            }
            read.sessions
        }
    };
    let deleted = store.legacy_deleted().await;
    let (records, layout) = legacy::moved(
        &sessions,
        fingerprint,
        kept_whole,
        &deleted,
        crate::time::now_ms(),
    );
    let here = match here {
        Some(here) => here,
        None => match here_of(store, &records).await {
            Ok(here) => here,
            Err(why) => {
                load.notices.push(HistoryNotice::MovePending(why));
                return Ok(None);
            }
        },
    };
    let records = here.not_moved_yet(records);
    match store.write_move(&records, &layout).await {
        Ok(placed) => Ok(Some((layout, as_placed(&records, &placed)))),
        Err(refused) if refused.raced() => Err(Moving::Raced(refused.words())),
        Err(refused) if B::FALLS_BACK => Err(Moving::Refused(refused.words())),
        Err(refused) => {
            load.notices
                .push(HistoryNotice::MovePending(refused.words()));
            Ok(None)
        }
    }
}

/// What a store that moves before it reads holds, as the move made over it
/// looks at it ([`Here`]), the move making `moved`: nothing, for a move
/// that has nothing to write, which so looks at nothing, and waits for
/// nothing. Why it cannot be looked at now, as a tombstone, or a record
/// under a name the move does not give, that cannot be read just then: the
/// move waits.
async fn here_of<B: Backend>(store: &B, moved: &[Record]) -> Result<Here, String> {
    if moved.is_empty() {
        return Ok(Here::default());
    }
    let (records, gone) = store.here().await.map_err(|refused| refused.words())?;
    if records.is_empty() && gone.is_empty() {
        return Ok(Here::default());
    }
    let tombstones = store
        .tombstones()
        .await
        .map_err(|refused| refused.words())?;
    let gone_keys = keys_of_tombstones(&tombstones, Reading::Strict)?;
    let names: BTreeSet<RecordId> = moved.iter().map(|record| record.id).collect();
    let elsewhere: Vec<RecordId> = records.difference(&names).copied().collect();
    let texts = store
        .texts(&elsewhere)
        .await
        .map_err(|refused| refused.words())?;
    Ok(Here {
        records,
        gone,
        texts: texts.into_iter().collect(),
        gone_keys,
    })
}

/// Take up what an older build stored in, or deleted from, the history
/// kept whole since `layout` took it up ([`legacy::imported`]), `known`
/// being every record here by key and the sessions the tombstones named as
/// they were read before those records were, where both are known already;
/// and hand back what was made of it: nothing, where nothing changed there,
/// or every record here cannot be read now. Held back where what changed,
/// or a tombstone here, cannot be read now, or the change made: it is
/// still to take up.
///
/// The tombstones are read again after the records: a session another
/// copy deletes in between is either still here or deleted after, and one
/// it puts back in between, by Undo or a restore, is either deleted before
/// or here, once its Undo has put it back.
///
/// It is no restore: what was taken up is noted only once it is stored,
/// in how the history is kept, written last or with it, so a take-up cut
/// short is made again by the next look, and nobody need be told.
async fn take_up<B: Backend>(
    store: &B,
    layout: &Layout,
    known: Option<(Live, HashSet<SessionKey>)>,
) -> Result<Option<HistoryChange>, HeldBack> {
    let LegacyLook::Changed(mut read) = store.legacy_look(layout).await else {
        return Ok(None);
    };
    // Read with the history kept whole, where the look read that.
    let mut deleted = None;
    if read.is_some() {
        deleted = Some(store.legacy_deleted().await);
    }
    let (live, before) = match known {
        Some(known) => known,
        None => {
            let before = match store.tombstones().await {
                Ok(tombstones) => keys_of_tombstones(&tombstones, Reading::Strict),
                Err(refused) => Err(refused.words()),
            };
            let before = before.map_err(|why| {
                eprintln!("could not read what was deleted here: {why}");
                HeldBack
            })?;
            let Some(live) = live_keys(store).await else {
                return Ok(None);
            };
            // A store that read nothing of the history kept whole yet looks
            // at it again, the records read.
            if read.is_none() {
                match store.legacy_look(layout).await {
                    LegacyLook::Changed(again) => read = again,
                    LegacyLook::Same => return Ok(None),
                }
            }
            (live, before)
        }
    };
    let legacy = match read {
        Some(legacy) => legacy,
        None => store.legacy().await.map_err(|why| {
            eprintln!("could not read the history an older build keeps: {why}");
            HeldBack
        })?,
    };
    let (raw, fingerprint) = match legacy {
        Some((raw, fingerprint)) => (Some(raw), Some(fingerprint)),
        None => (None, None),
    };
    let sessions = raw
        .as_deref()
        .and_then(|raw| std::str::from_utf8(raw).ok())
        .and_then(read_history)
        .map(|read| read.sessions)
        .unwrap_or_default();
    let deleted = match deleted {
        Some(deleted) => deleted,
        None => store.legacy_deleted().await,
    };
    // A tombstone that cannot be read now may name a session to keep
    // deleted: nothing is taken up until it reads.
    let after = match store.tombstones().await {
        Ok(tombstones) => keys_of_tombstones(&tombstones, Reading::Strict),
        Err(refused) => Err(refused.words()),
    };
    let mut gone = after.map_err(|why| {
        eprintln!("could not read what was deleted here: {why}");
        HeldBack
    })?;
    gone.extend(before);
    let Some((change, next)) =
        legacy::imported(layout, fingerprint, &sessions, &deleted, &live, &gone)
    else {
        return Ok(None);
    };
    if !change.is_empty() && !store.may_write().await {
        return Err(HeldBack);
    }
    let writes = Writes::of(&change).map_err(|_| HeldBack)?;
    let laid = Laid::Over {
        from: layout,
        next: &next,
    };
    let made = store.commit(&writes, laid, By::Store).await;
    noted(store.keys(), &change, &made.placed, made.saved.is_ok());
    match made.saved {
        // Each session where it went, to be read there.
        Ok(_) => Ok(Some(HistoryChange {
            put: as_placed(&change.put, &made.placed),
            ..change
        })),
        Err(refused) => {
            if refused.raced() {
                store.raced().await;
            }
            Err(HeldBack)
        }
    }
}

/// Every record there is by its session's key: those read before as they
/// were read, the rest read now for their keys alone. One that is damaged
/// has none, though it may say one, as one whose bytes are no text has
/// none: only one that reads, or that a later build stored, holds its
/// session ([`held_key`]). None where the history cannot be listed, or a
/// record in it cannot be read for now, or all of them listed: it may hold
/// a session an older build holds too, which would be taken up a second
/// time beside it.
async fn live_keys<B: Backend>(store: &B) -> Option<Live> {
    let listing = store.list().await.ok()?;
    if listing.skipped {
        return None;
    }
    let ids: Vec<RecordId> = listing.records.difference(&listing.gone).copied().collect();
    let unknown: Vec<RecordId> = {
        let keys = store.keys().borrow();
        ids.iter()
            .filter(|id| !keys.contains_key(id))
            .copied()
            .collect()
    };
    if !unknown.is_empty() {
        let read = store.read(&unknown, Some(&listing.listed)).await;
        let mut keys = store.keys().borrow_mut();
        for (id, raw) in unknown.into_iter().zip(read) {
            match raw {
                Raw::Text(raw) => {
                    if let Some(key) = held_key(&raw) {
                        keys.insert(id, key);
                    }
                }
                Raw::NoText(_) | Raw::Missing => {}
                Raw::Failed => return None,
            }
        }
    }
    let keys = store.keys().borrow();
    let mut live: Live = HashMap::new();
    for id in ids {
        if let Some(key) = keys.get(&id) {
            live.entry(key.clone()).or_default().push(id);
        }
    }
    Some(live)
}

/// What to tell of `unread`, the records this build cannot read, each with
/// what it says of itself, and of `failed`, those that could not be read at
/// all just now, by the store's notes of what it told ([`notes::at_open`]);
/// `writable` is whether what was told may be noted, so it is told of once,
/// and what failed, for the next read to tell of. The notes are read before
/// any is written.
async fn tell<B: Backend>(
    store: &B,
    unread: &[(RecordId, Unreadable)],
    failed: &[RecordId],
    writable: bool,
) -> Vec<HistoryNotice> {
    let failing = store.note(Note::Failing).await;
    let told = store.note(Note::Told).await;
    let unread_told = store.note(Note::UnreadTold).await;
    let damaged_told = store.note(Note::DamagedTold).await;
    let held = notes::Held {
        told: told.as_deref(),
        failing: failing.as_deref(),
        unread_told: unread_told.as_deref(),
        damaged_told: damaged_told.as_deref(),
    };
    let told = notes::at_open(unread, failed, held);
    if writable {
        for (note, names) in &told.noted {
            store.write_note(*note, names.as_ref()).await;
        }
    }
    told.notices
}

/// Take `gone`, records by name, out of `note`, as [`notes::no_more`] says:
/// only where this build may write to the history, as the note is made.
async fn no_more<B: Backend>(store: &B, note: Note, gone: &HashSet<String>) {
    let Some(raw) = store.note(note).await else {
        return;
    };
    let Some(left) = notes::no_more(note, &raw, gone) else {
        return;
    };
    if store.may_write().await {
        store.write_note(note, left.as_ref()).await;
    }
}

/// Make `change` ([`super::Store::change_history`]), and say what became of
/// it: deletions undone first, then the records, then the deletions, as
/// the store makes them, each step durable before the next, or all of them
/// at once.
pub(super) async fn apply<B: Backend>(store: &B, change: &HistoryChange) -> HistorySaved {
    match store.gate().await {
        Ok(Gate::Open(_) | Gate::Unmoved) => {}
        Ok(Gate::Newer) => return HistorySaved::Newer,
        Ok(Gate::Unread) => {
            return HistorySaved::Failed("how the history is kept could not be read.".into());
        }
        Err(refused) => return refused.saved(),
    }
    let writes = match Writes::of(change) {
        Ok(writes) => writes,
        Err(err) => return HistorySaved::Failed(err.to_string()),
    };
    let made = store.commit(&writes, Laid::AsItIs, By::App).await;
    noted(store.keys(), change, &made.placed, made.saved.is_ok());
    made.saved.unwrap_or_else(|refused| refused.saved())
}

/// Every record there is, deleted ones left out, once whatever an older
/// build stored since has been taken up, where every record here can be
/// read ([`super::Store::history_listing`]). None where the history cannot
/// be listed. Of what this build noted of records it could not read, those
/// a listing no longer finds are gone, and go from each note
/// ([`notes::gone_from`]): a listing that could not list them all does not
/// say which are.
pub(super) async fn listing<B: Backend>(store: &B) -> Option<Vec<RecordId>> {
    let held_back = match store.gate().await {
        Ok(Gate::Open(layout)) => take_up(store, &layout, None).await.is_err(),
        _ => false,
    };
    // As the store was before it was looked at, the history kept whole
    // among it, and once what was taken up of it was stored, as this
    // copy's own change is none for it to take up: whatever another copy
    // changes from here on is taken up at the next look. What was held
    // back is still to take up.
    let looked = store.look();
    let listing = store.list().await;
    if listing.is_ok() {
        store.saw(looked);
    }
    if held_back {
        store.held_back();
    }
    let listing = listing.ok()?;
    let listed: Vec<RecordId> = listing.records.difference(&listing.gone).copied().collect();
    if !listing.skipped {
        let there: HashSet<String> = listed.iter().map(|id| id.name()).collect();
        for note in notes::LISTED {
            let Some(raw) = store.note(note).await else {
                continue;
            };
            let gone = notes::gone_from(&raw, &there);
            if !gone.is_empty() {
                no_more(store, note, &gone).await;
            }
        }
    }
    Some(listed)
}

/// The records `ids`, as they are stored ([`super::Store::read_records`]):
/// one that cannot be read comes back as why. Each read, or found damaged,
/// fails no longer, and is told of no more ([`notes::after_read`]).
pub(super) async fn read<B: Backend>(store: &B, ids: &[RecordId]) -> Vec<Result<Record, Unopened>> {
    let read: Vec<Result<Record, Unopened>> = store
        .read(ids, None)
        .await
        .into_iter()
        .zip(ids)
        .map(|(raw, &id)| raw.record(id))
        .collect();
    for (note, gone) in notes::after_read(&read) {
        no_more(store, note, &gone).await;
    }
    read
}

/// Which of `ids` were deleted by hand, and when each tombstone says
/// ([`super::Store::deletions`]).
pub(super) async fn deletions<B: Backend>(store: &B, ids: &[RecordId]) -> Option<Deletions> {
    store.tombstones_of(ids).await.map(deletions_of)
}

/// Which of `ids` were deleted by hand, each beside the session its
/// tombstone remembers ([`super::Store::deleted_sessions`]).
pub(super) async fn deleted_sessions<B: Backend>(
    store: &B,
    ids: &[RecordId],
) -> Option<Vec<(RecordId, SessionKey)>> {
    store.tombstones_of(ids).await.map(deleted_sessions_of)
}

/// Which of the sessions `keys` a tombstone remembers, whatever record it
/// named ([`super::Store::deleted_keys`]): one that does not read, or names
/// no session, is left out. None where the tombstones cannot be listed.
pub(super) async fn deleted_keys<B: Backend>(
    store: &B,
    keys: Vec<SessionKey>,
) -> Option<Vec<SessionKey>> {
    let tombstones = store.tombstones().await.ok()?;
    let gone = keys_of_tombstones(&tombstones, Reading::Lenient).ok()?;
    Some(deleted_keys_of(&gone, keys))
}

/// What to tell of `ids`, records listed that could not be read at all at
/// any of the app's tries since it found them so
/// ([`super::Store::still_unread`]): of those not told of before, how many,
/// each noted as told, where this build may write, so it is told of once
/// ([`notes::still_unread`]); every one of them, where the store notes
/// none ([`Backend::NOTES_STILL_UNREAD`]).
#[cfg_attr(not(any(feature = "native-runtime", test)), allow(dead_code))]
pub(super) async fn still_unread<B: Backend>(store: &B, ids: &[RecordId]) -> Vec<HistoryNotice> {
    if !B::NOTES_STILL_UNREAD {
        return unread_notices(ids.iter().map(|_| None));
    }
    let told = store.note(Note::UnreadTold).await;
    let (notices, unread_told) = notes::still_unread(ids, told.as_deref());
    let writable = store.may_write().await;
    if let Some(names) = unread_told
        && writable
    {
        store.write_note(Note::UnreadTold, Some(&names)).await;
        // Noted where a build before formats looks too, so such a build
        // does not tell of it a second time.
        if let Some(older) = notes::noted_unread(ids, store.note(Note::Told).await.as_deref()) {
            store.write_note(Note::Told, Some(&older)).await;
            // What this build notes there stands for no note of damage of
            // its own: from here on that note does.
            if store.note(Note::DamagedTold).await.is_none() {
                store
                    .write_note(Note::DamagedTold, Some(&BTreeSet::new()))
                    .await;
            }
        }
        // Noted failing as well, as this build's failures: told of no more
        // once they read.
        if let Some(failing) = notes::noted_unread(ids, store.note(Note::Failing).await.as_deref())
        {
            store.write_note(Note::Failing, Some(&failing)).await;
        }
    }
    notices
}

/// What `op` comes to, done then and there: none, where it waits on
/// something, as no store that reads and writes then and there waits.
#[cfg(any(feature = "native-runtime", test))]
pub(super) fn at_once<T>(op: impl std::future::Future<Output = T>) -> Option<T> {
    let mut op = std::pin::pin!(op);
    match op
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    {
        std::task::Poll::Ready(done) => Some(done),
        std::task::Poll::Pending => None,
    }
}

#[cfg(test)]
mod tests;
