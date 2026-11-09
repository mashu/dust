//! A sync, start to end, against a store and a folder: finding the keyring
//! as sync is turned on ([`look`], [`join`]), and each sync after
//! ([`sync_once_with`]). Nothing here knows which store or folder it has,
//! nor touches the app: it says what it did, and the app takes it up.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cw_core::{SessionResult, TrainingSettings};
use serde_json::Map;

use super::pack::{
    Forget, PACK_LAYOUT, PACK_PREFIX, Pack, Put, Revive, Unopened, open_pack, pack_name, packs_of,
    seal_pack,
};
use super::plan::{DriveState, Here, Plan, plan};
use super::remote::{Remote, RemoteError, RemoteFile};
use super::seal::{DataKey, KEYRING, Keys, Stretch, Unlock, new_keyring, open_keyring};
use super::settings::{
    SETTINGS_PREFIX, SettingsFile, StampedSettings, Using, merge, stamp, stamps_of, taken,
};
use super::state::SyncState;
use crate::persist::record::{HistoryChange, Record, RecordId, read_record};
use crate::persist::{Deleted, Store};

/// What turning sync on found in the drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// Nothing of Dust's: this is the first device to sync.
    Nothing,
    /// A keyring: another device syncs already.
    Keyring { file: RemoteFile, raw: Vec<u8> },
}

/// Why sync was not turned on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// The passphrase does not open the keyring in the drive.
    Wrong,
    /// The keyring in the drive is none this build opens.
    Unreadable,
    /// Another device turned sync on at the same moment, with another
    /// passphrase.
    Raced,
    /// The device has no randomness to make a key with.
    NoRandom,
    Remote(RemoteError),
}

impl From<RemoteError> for JoinError {
    fn from(err: RemoteError) -> Self {
        Self::Remote(err)
    }
}

impl JoinError {
    /// What to tell the user.
    pub fn told(&self) -> String {
        match self {
            Self::Wrong => "That passphrase does not open the synced copy in your Google Drive.".into(),
            Self::Unreadable => "The synced copy in your Google Drive was made by a newer Dust: update Dust here to sync with it.".into(),
            Self::Raced => "Another device turned sync on at the same moment, with another passphrase. Enter the passphrase you chose there.".into(),
            Self::NoRandom => "This device could not make a key for sync.".into(),
            Self::Remote(err) => format!("Sync was not turned on: {}.", err.why()),
        }
    }
}

/// The keyring every device goes by, of those in `files`: the first made,
/// where two devices each made one at once.
fn first_keyring(files: &[RemoteFile]) -> Option<&RemoteFile> {
    files
        .iter()
        .filter(|file| file.name == KEYRING)
        .min_by(|a, b| (&a.created, &a.id).cmp(&(&b.created, &b.id)))
}

/// What the drive holds of Dust's, as sync is turned on.
pub async fn look(remote: &dyn Remote) -> Result<Found, RemoteError> {
    let files = remote.list().await?;
    match first_keyring(&files) {
        None => Ok(Found::Nothing),
        Some(file) => Ok(Found::Keyring {
            file: file.clone(),
            raw: remote.get(&file.id).await?,
        }),
    }
}

/// Join the sync `found` in the drive with `passphrase`, or begin one with
/// it where there is none, and give back the data key and the keyring it is
/// in. Where another device began one at the same moment, the first made is
/// the one every device goes by, and this device's own is taken away again.
pub async fn join(
    remote: &dyn Remote,
    passphrase: &str,
    found: &Found,
    stretch: Stretch,
) -> Result<(DataKey, String), JoinError> {
    if let Found::Keyring { file, raw } = found {
        return match open_keyring(raw, passphrase) {
            Ok(key) => Ok((key, file.id.clone())),
            Err(Unlock::Wrong) => Err(JoinError::Wrong),
            Err(Unlock::Unreadable) => Err(JoinError::Unreadable),
        };
    }
    let (raw, key) = new_keyring(passphrase, stretch).ok_or(JoinError::NoRandom)?;
    let ours = remote.create(KEYRING, raw).await?;
    let files = remote.list().await?;
    match first_keyring(&files) {
        Some(first) if first.id != ours => {
            remote.delete(&ours).await?;
            let raw = remote.get(&first.id).await?;
            match open_keyring(&raw, passphrase) {
                Ok(key) => Ok((key, first.id.clone())),
                Err(_) => Err(JoinError::Raced),
            }
        }
        _ => Ok((key, ours)),
    }
}

/// What a sync did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Synced {
    /// The settings the drive's and this device's come to, where they
    /// differ from this device's, for the app to take up.
    pub settings: Option<TrainingSettings>,
    /// The theme the drive's and this device's come to, where it differs
    /// from this device's: stored already, for the app to show.
    pub theme: Option<String>,
    /// The records taken from the drive, put back among them.
    pub took: usize,
    /// The records deleted here as the drive deleted them.
    pub let_go: usize,
    /// The records, deletions and sessions put back sent to the drive.
    pub sent: usize,
    /// Files in the drive this key could not open, damaged say.
    pub unreadable: usize,
    /// Files in the drive a later build wrote, which this one does not
    /// read: read once it is updated.
    pub later: usize,
}

impl Synced {
    /// The history here changed.
    pub fn changed_history(&self) -> bool {
        self.took > 0 || self.let_go > 0
    }
}

/// Why a sync stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncFailed {
    Remote(RemoteError),
    /// The store here would not take what the drive holds, for the reason
    /// given, in words that follow "Sync stopped:".
    Store(String),
    /// The synced copy is gone from the drive: deleted from another device,
    /// or from the drive's own settings.
    Gone,
    /// The drive holds a sync begun anew since this device joined, whose
    /// key it does not have.
    Replaced,
    /// What this device keeps of the key does not read.
    NoKey,
}

impl From<RemoteError> for SyncFailed {
    fn from(err: RemoteError) -> Self {
        Self::Remote(err)
    }
}

impl SyncFailed {
    /// What to tell the user.
    pub fn told(&self) -> String {
        match self {
            Self::Remote(err) => format!("Sync stopped: {}.", err.why()),
            Self::Store(why) => format!("Sync stopped: {why}."),
            Self::Gone => "The synced copy is gone from your Google Drive, so sync is off here. Turn it on again to sync anew.".into(),
            Self::Replaced => "Sync was begun anew from another device, with another key, so it is off here. Turn it on again with the passphrase chosen there.".into(),
            Self::NoKey => "The key for sync kept here could not be read, so sync is off here. Turn it on again with your passphrase.".into(),
        }
    }

    /// The sync this device belonged to is over: it stops, rather than
    /// trying again.
    pub fn ends_sync(&self) -> bool {
        matches!(self, Self::Gone | Self::Replaced | Self::NoKey)
    }
}

/// The packs of `files` not yet read, read into `drive`, each noted in
/// `read`; the records they hold gathered into `bytes`, each as it was
/// sent, a session put back taking the place of the same record put
/// before. Says how many files would not open, and how many a later build
/// wrote: one of those is noted in `later` rather than as read, and is
/// fetched again only by a build of a later layout than this one, which
/// may read it.
async fn read_packs(
    remote: &dyn Remote,
    keys: &Keys,
    files: &[RemoteFile],
    read: &mut BTreeSet<String>,
    later: &mut BTreeMap<String, u32>,
    drive: &mut DriveState,
    bytes: &mut HashMap<RecordId, String>,
) -> Result<(usize, usize), RemoteError> {
    let (mut unreadable, mut newer) = (0, 0);
    for file in files {
        if !file.name.starts_with(PACK_PREFIX) || read.contains(&file.name) {
            continue;
        }
        if later
            .get(&file.name)
            .is_some_and(|&layout| layout >= PACK_LAYOUT)
        {
            newer += 1;
            continue;
        }
        let sealed = match remote.get(&file.id).await {
            Ok(sealed) => sealed,
            // Deleted as it was read, by a device deleting the whole copy:
            // the keyring's going is told of at the next sync.
            Err(RemoteError::Gone) => continue,
            Err(err) => return Err(err),
        };
        match open_pack(keys, &file.name, &sealed) {
            Ok(pack) => {
                drive.take(&pack);
                for put in pack.put {
                    if let Some(id) = put.record_id() {
                        bytes.entry(id).or_insert(put.s);
                    }
                }
                for revive in pack.revive {
                    if let Some(id) = revive.record_id() {
                        bytes.insert(id, revive.s);
                    }
                }
                later.remove(&file.name);
            }
            Err(Unopened::Damaged) => unreadable += 1,
            // What it holds arrives once this build is updated.
            Err(Unopened::Later) => {
                later.insert(file.name.clone(), PACK_LAYOUT);
                newer += 1;
                continue;
            }
        }
        // Read, whether it opened or not: one damaged does not open the
        // next time either.
        read.insert(file.name.clone());
    }
    Ok((unreadable, newer))
}

/// A sync with nothing put back by hand while it runs, kept as it ends, as
/// a test syncs a device: [`sync_once_with`].
#[cfg(test)]
pub async fn sync_once(
    store: &dyn Store,
    remote: &dyn Remote,
    state: &mut SyncState,
    here: &TrainingSettings,
    now: u64,
) -> Result<Synced, SyncFailed> {
    sync_once_with(store, remote, state, here, now, &BTreeSet::new, &|_| {}).await
}

/// Sync the history in `store` and the settings `here` with the drive
/// `remote`, as `state` says this device stands, and say what it did. What
/// it changes of `state` is changed as it goes, whether it ends well or
/// not: the caller keeps it either way.
///
/// What the drive holds is taken into the store first, in one change,
/// before anything is sent: a sync cut short leaves the store as it was or
/// with all of it, and the next sends what this one did not. Packs read
/// are noted only once what they hold is in the store.
///
/// `put_back_since` says which records were put back by hand here since
/// the sync began, after it took in the notes it plans from
/// ([`SyncState::revived`]): none of them is let go, whatever the drive
/// says. The app notes those as the sync ends, and the next sync sends
/// them as put back. It is asked once the sync has read all it reads of
/// the store and the drive, just before it changes the store.
///
/// `keep` keeps `state` as it stands, which the caller does in any case as
/// the sync ends: it is asked as a pack holding deletions by hand goes,
/// once they are noted against it ([`send_pack`]), so that an app stopped
/// before the sync ends knows of the pack all the same.
pub async fn sync_once_with(
    store: &dyn Store,
    remote: &dyn Remote,
    state: &mut SyncState,
    here: &TrainingSettings,
    now: u64,
    put_back_since: &dyn Fn() -> BTreeSet<RecordId>,
    keep: &dyn Fn(&SyncState),
) -> Result<Synced, SyncFailed> {
    let key = DataKey::from_hex(&state.key).ok_or(SyncFailed::NoKey)?;
    let keys = Keys::of(&key);
    let files = remote.list().await?;
    match first_keyring(&files) {
        None => return Err(SyncFailed::Gone),
        Some(keyring) if !state.keyring.is_empty() && keyring.id != state.keyring => {
            return Err(SyncFailed::Replaced);
        }
        Some(_) => {}
    }
    let mut synced = Synced::default();

    // What the drive holds, every pack not read before put together with
    // what was read of it before.
    let mut read = state.read.clone();
    let mut drive = state.drive.clone();
    let mut bytes = HashMap::new();
    let later = &mut state.later;
    (synced.unreadable, synced.later) = read_packs(
        remote, &keys, &files, &mut read, later, &mut drive, &mut bytes,
    )
    .await?;
    take_unanswered(state, &files, &read, &drive);
    // A sync that has read nothing of the drive yet, as one just turned on
    // anew, stamped each putting back it notes knowing nothing of the
    // drive: what sync knew before it was turned on anew went with it. Each
    // is stamped again after what the drive holds of its record, as one
    // noted now would be. One noted once this device knew the drive keeps
    // its stamp: stamped again, it would beat a later deletion made
    // elsewhere.
    if state.read.is_empty() {
        for (&id, at) in &mut state.revived {
            *at = drive.stamp_change(id, *at);
        }
    }

    let listed: BTreeSet<RecordId> = store
        .history_listing()
        .await
        .ok_or_else(|| SyncFailed::Store("the history here could not be read".into()))?
        .into_iter()
        .collect();
    // A deletion the drive holds is the last word here only while the
    // session stays gone: one listed here again, or put back here by hand
    // since the last sync, and deleted after, was deleted by hand, which
    // the drive is to hear of, or which a deletion it holds accounts for.
    state
        .applied
        .retain(|id, _| !listed.contains(id) && !state.revived.contains_key(id));
    // Once, at the first sync after an update from a build that noted no
    // deletion by hand: those it made that the drive holds, noted.
    if !state.deletions_noted {
        note_deletions_by_hand(store, &listed, state).await;
        state.deletions_noted = true;
    }
    let mut planned = planned(store, &drive, &listed, state).await;
    // A record to take that only a pack read before holds, as where the
    // store here lost one: every pack read again, once.
    let wanting = |planned: &Plan, bytes: &HashMap<RecordId, String>| {
        planned
            .take
            .iter()
            .chain(&planned.take_back)
            .any(|id| !bytes.contains_key(id))
    };
    if wanting(&planned, &bytes) && !state.read.is_empty() {
        read.clear();
        drive = DriveState::default();
        bytes.clear();
        let later = &mut state.later;
        (synced.unreadable, synced.later) = read_packs(
            remote, &keys, &files, &mut read, later, &mut drive, &mut bytes,
        )
        .await?;
        planned = self::planned(store, &drive, &listed, state).await;
    }

    // The deletions made here by hand to send, the twins' below left out.
    let by_hand: BTreeSet<RecordId> = planned.send_deleted.iter().copied().collect();

    // One session under two names, here and in the drive, as a backup
    // restored on one device names its sessions anew: the smaller name is
    // kept on every device alike, and the other let go everywhere, but only
    // once the one kept is in the drive for every device to take.
    let mut twins_here = Vec::new();
    let mut not_taken = BTreeSet::new();
    for twin in twins(store, &planned, &bytes, &listed).await {
        if twin.theirs < twin.ours {
            twins_here.push((twin.ours, twin.json));
            planned.send_deleted.push(twin.ours);
        } else {
            not_taken.insert(twin.theirs);
            if drive.alive(twin.ours) {
                planned.send_deleted.push(twin.theirs);
            }
        }
    }
    let let_go_here: BTreeSet<RecordId> = twins_here.iter().map(|(id, _)| *id).collect();
    planned.take.retain(|id| !not_taken.contains(id));
    planned.send.retain(|id| !let_go_here.contains(id));

    // What the drive holds, taken here.
    let here_bytes = read_here(store, &planned).await;
    // Put back by hand since this sync began, after it took in the notes
    // it planned from: not this sync's to let go. Asked after its last
    // read, as nothing comes between this and the change.
    let since = put_back_since();
    planned.let_go.retain(|(id, _)| !since.contains(id));
    let mut change = HistoryChange::default();
    for &id in planned.take.iter().chain(&planned.take_back) {
        if let Some(json) = bytes.get(&id) {
            change.put.push(Record {
                id,
                json: json.clone(),
            });
        }
    }
    // A deletion undone only with the record that fills it in.
    change.unforget = planned
        .take_back
        .iter()
        .copied()
        .filter(|id| bytes.contains_key(id))
        .collect();
    for &(id, at) in &planned.let_go {
        let session = here_bytes.get(&id).and_then(|json| read_record(json));
        let deleted = match session {
            Some(session) => Deleted::of(&session, at),
            None => Deleted {
                timestamp: 0,
                date: String::new(),
                at,
                unread: Map::new(),
            },
        };
        change.forget.push((id, deleted));
    }
    for (id, json) in &twins_here {
        if let Some(session) = read_record(json) {
            change.forget.push((*id, Deleted::of(&session, now)));
        }
    }
    if !change.is_empty() {
        let saved = store.change_history(change.clone()).await;
        if saved.kept().is_none() {
            let why = saved
                .problem()
                .unwrap_or_else(|| "this device could not store the sessions".into());
            return Err(SyncFailed::Store(why.trim_end_matches('.').to_string()));
        }
        synced.took = change.put.len();
        synced.let_go = change.forget.len();
    }
    // The deletions the drive holds that account for a session gone here,
    // noted: those let go here, and those deleted here by hand that the
    // drive deleted as well.
    for &(id, at) in planned.let_go.iter().chain(&planned.deleted_too) {
        state.applied.insert(id, at);
    }
    for id in &change.unforget {
        state.applied.remove(id);
    }
    state.read = read;
    state.drive = drive;

    // What is here, sent: what the drive holds of the sessions put back
    // here is all it needs of them, so each note of one goes once sent, or
    // once there is nothing to send of it.
    let put: Vec<Put> = planned
        .send
        .iter()
        .filter_map(|id| {
            Some(Put {
                id: id.name(),
                s: here_bytes.get(id)?.clone(),
            })
        })
        .collect();
    let forget: Vec<Forget> = planned
        .send_deleted
        .iter()
        .map(|&id| Forget {
            id: id.name(),
            at: state.stamp_change(id, now),
        })
        .collect();
    let revive: Vec<Revive> = planned
        .send_back
        .iter()
        .filter_map(|&(id, at)| {
            Some(Revive {
                id: id.name(),
                at,
                s: here_bytes.get(&id)?.clone(),
            })
        })
        .collect();
    let sent_back: BTreeSet<RecordId> = planned.send_back.iter().map(|(id, _)| *id).collect();
    state
        .revived
        .retain(|id, _| sent_back.contains(id) && listed.contains(id));
    for pack in packs_of(&state.device, now, put, forget, revive) {
        send_pack(remote, &keys, state, &pack, &by_hand, keep).await?;
        synced.sent += pack.put.len() + pack.forget.len() + pack.revive.len();
        for revive in &pack.revive {
            if let Some(id) = revive.record_id() {
                state.revived.remove(&id);
            }
        }
    }

    sync_settings(store, remote, &keys, &files, state, here, now, &mut synced).await?;
    state.last = Some(now);
    Ok(synced)
}

/// Note the deletions by hand made here that a build before this one,
/// which noted none, left with no note: those of the sessions the drive
/// held as deleted as this device last synced, each deleted by hand here
/// and sent from here, or found deleted in the drive already. Each is noted
/// with the drive's deletion, as this build notes one it finds the drive
/// holding ([`Plan::deleted_too`]): a putting back the drive holds after it
/// brings the session back here, rather than this device sending its
/// deletion again. One put back here by hand since the last sync is left
/// out: deleted again, it is a deletion by hand as any other.
async fn note_deletions_by_hand(
    store: &dyn Store,
    listed: &BTreeSet<RecordId>,
    state: &mut SyncState,
) {
    let unnoted: Vec<RecordId> = state
        .drive
        .tombs
        .keys()
        .copied()
        .filter(|&id| {
            state.drive.deleted(id)
                && !listed.contains(&id)
                && !state.applied.contains_key(&id)
                && !state.revived.contains_key(&id)
        })
        .collect();
    if unnoted.is_empty() {
        return;
    }
    for id in store.forgotten(unnoted).await {
        if let Some(&at) = state.drive.tombs.get(&id) {
            state.applied.insert(id, at);
        }
    }
}

/// The plan for this sync, the store asked which of what the drive holds
/// and it does not list it deleted by hand: of what the drive holds alive,
/// and of what it holds as deleted that this device has no note of, where
/// the drive deleted it since the last sync, as a session deleted on two
/// devices before either synced, or this device put it back by hand since
/// and deleted it again.
async fn planned(
    store: &dyn Store,
    drive: &DriveState,
    listed: &BTreeSet<RecordId>,
    state: &SyncState,
) -> Plan {
    let deleted_since = drive.tombs.keys().copied().filter(|&id| {
        drive.deleted(id)
            && (!state.drive.deleted(id) || state.revived.contains_key(&id))
            && !state.applied.contains_key(&id)
    });
    let missing: Vec<RecordId> = drive
        .alive_ids()
        .into_iter()
        .chain(deleted_since)
        .filter(|id| !listed.contains(id))
        .collect();
    let forgotten: BTreeSet<RecordId> = if missing.is_empty() {
        BTreeSet::new()
    } else {
        store.forgotten(missing).await.into_iter().collect()
    };
    plan(
        drive,
        Here {
            listed,
            forgotten: &forgotten,
            applied: &state.applied,
            revived: &state.revived,
        },
    )
}

/// A record the drive holds that is a session this device holds already
/// under another name, alike in everything: as one device keeps a session
/// under the name it was kept by, and another that restored it from a
/// backup under the name the restore gave it ([`RecordId::placed`]).
struct Twin {
    theirs: RecordId,
    ours: RecordId,
    /// What this device stored of it.
    json: String,
}

/// The twins of the records `planned` takes, the drive's read into `bytes`:
/// each looked for only among the records here of the same millisecond, as
/// a session's name falls where it finished, and only where one of the
/// drive's and one of this device's are alike, so that sessions alike in
/// everything twice over stay as many as they are.
async fn twins(
    store: &dyn Store,
    planned: &Plan,
    bytes: &HashMap<RecordId, String>,
    listed: &BTreeSet<RecordId>,
) -> Vec<Twin> {
    // Those going here, or put back, are no twin of any.
    let busy: BTreeSet<RecordId> = planned
        .let_go
        .iter()
        .chain(&planned.send_back)
        .map(|(id, _)| *id)
        .collect();
    let mut near: Vec<(RecordId, Vec<RecordId>)> = Vec::new();
    for &theirs in &planned.take {
        let from = theirs.order / 1000 * 1000;
        let start = RecordId {
            order: from,
            nonce: 0,
        };
        let end = RecordId {
            order: from.saturating_add(999),
            nonce: u64::MAX,
        };
        let here: Vec<RecordId> = listed
            .range(start..=end)
            .copied()
            .filter(|id| !busy.contains(id))
            .collect();
        if !here.is_empty() {
            near.push((theirs, here));
        }
    }
    if near.is_empty() {
        return Vec::new();
    }
    let wanted: BTreeSet<RecordId> = near.iter().flat_map(|(_, here)| here.clone()).collect();
    let held: HashMap<RecordId, (SessionResult, String)> = store
        .read_records(wanted.into_iter().collect())
        .await
        .into_iter()
        .flatten()
        .filter_map(|record| Some((record.id, (read_record(&record.json)?, record.json))))
        .collect();
    let mut found: Vec<(RecordId, RecordId)> = Vec::new();
    for (theirs, here) in near {
        let Some(session) = bytes.get(&theirs).and_then(|json| read_record(json)) else {
            continue;
        };
        let alike: Vec<RecordId> = here
            .into_iter()
            .filter(|id| held.get(id).is_some_and(|(ours, _)| *ours == session))
            .collect();
        if let [ours] = alike[..] {
            found.push((theirs, ours));
        }
    }
    // One of this device's alike to two of the drive's is a twin of neither.
    let mut matched: HashMap<RecordId, usize> = HashMap::new();
    for (_, ours) in &found {
        *matched.entry(*ours).or_default() += 1;
    }
    let mut twins = Vec::new();
    for &(theirs, ours) in &found {
        if matched[&ours] == 1
            && let Some((_, json)) = held.get(&ours)
        {
            twins.push(Twin {
                theirs,
                ours,
                json: json.clone(),
            });
        }
    }
    twins
}

/// What the store holds of each record the plan sends, or deletes here for
/// the drive: each as stored, so whatever a later build added to it goes
/// too. One that cannot be read is left out.
async fn read_here(store: &dyn Store, planned: &Plan) -> BTreeMap<RecordId, String> {
    let ids: Vec<RecordId> = planned
        .send
        .iter()
        .copied()
        .chain(planned.send_back.iter().map(|(id, _)| *id))
        .chain(planned.let_go.iter().map(|(id, _)| *id))
        .collect();
    if ids.is_empty() {
        return BTreeMap::new();
    }
    store
        .read_records(ids)
        .await
        .into_iter()
        .flatten()
        .map(|record| (record.id, record.json))
        .collect()
}

/// Send `pack`, and take it as read: what it holds is here already.
///
/// A deletion by hand it holds, of those `by_hand`, is the drive's once the
/// drive holds it, noted as one taken from it: a putting back the drive
/// holds after it brings the session back here, as on every other device,
/// rather than this device sending it again. A twin's is not: its session
/// is kept under the other name. Until the drive answers, they are noted
/// against the pack's name ([`SyncState::unanswered`]): made though the
/// answer was lost, the pack is found at the next sync, and they are the
/// drive's then ([`take_unanswered`]). The note is kept (`keep`) before the
/// pack goes, so that it holds though the app stops before the sync ends.
async fn send_pack(
    remote: &dyn Remote,
    keys: &Keys,
    state: &mut SyncState,
    pack: &Pack,
    by_hand: &BTreeSet<RecordId>,
    keep: &dyn Fn(&SyncState),
) -> Result<(), SyncFailed> {
    let name =
        pack_name().ok_or_else(|| SyncFailed::Store("this device could not name a file".into()))?;
    let sealed = seal_pack(keys, &name, pack)
        .ok_or_else(|| SyncFailed::Store("this device could not seal what it sends".into()))?;
    let deletions: BTreeMap<RecordId, u64> = pack
        .forget
        .iter()
        .filter_map(|forget| Some((forget.record_id()?, forget.at)))
        .filter(|(id, _)| by_hand.contains(id))
        .collect();
    if !deletions.is_empty() {
        state.unanswered.insert(name.clone(), deletions);
        keep(state);
    }
    remote.create(&name, sealed).await?;
    state
        .applied
        .extend(state.unanswered.remove(&name).unwrap_or_default());
    state.read.insert(name);
    state.drive.take(pack);
    Ok(())
}

/// Take up the packs this device sent whose answer never came
/// ([`SyncState::unanswered`]), as the drive's `files` and the packs `read`
/// of them into `drive` show them now: one read was made all the same, and
/// each deletion by hand in it the drive holds, as late or later, is noted
/// as had the answer come ([`send_pack`]); one the drive does not list was
/// never made, and what it held is sent again as never sent. One listed
/// but not read yet waits.
fn take_unanswered(
    state: &mut SyncState,
    files: &[RemoteFile],
    read: &BTreeSet<String>,
    drive: &DriveState,
) {
    for (name, deletions) in std::mem::take(&mut state.unanswered) {
        if read.contains(&name) {
            for (id, at) in deletions {
                if drive.tombs.get(&id).is_some_and(|&tomb| tomb >= at) {
                    state.applied.insert(id, at);
                }
            }
        } else if files.iter().any(|file| file.name == name) {
            state.unanswered.insert(name, deletions);
        }
    }
}

/// Sync everything a device uses beside its history, the settings `here`
/// and what `store` holds of the rest ([`Using`]): every device's file
/// read, the newest of each key taken, and this device's own file written
/// where that changed what it holds. What it comes to is stored here, but
/// for the settings, which `synced` hands the app, as the app holds them.
#[allow(clippy::too_many_arguments)]
async fn sync_settings(
    store: &dyn Store,
    remote: &dyn Remote,
    keys: &Keys,
    files: &[RemoteFile],
    state: &mut SyncState,
    here: &TrainingSettings,
    now: u64,
    synced: &mut Synced,
) -> Result<(), SyncFailed> {
    let own_name = keys.name(SETTINGS_PREFIX, "settings", &state.device);
    let mut own: Vec<&RemoteFile> = Vec::new();
    let mut others: Vec<StampedSettings> = Vec::new();
    let mut held: Option<StampedSettings> = None;
    for file in files
        .iter()
        .filter(|file| file.name.starts_with(SETTINGS_PREFIX))
    {
        let sealed = match remote.get(&file.id).await {
            Ok(sealed) => sealed,
            Err(RemoteError::Gone) => continue,
            Err(err) => return Err(err.into()),
        };
        let Some(raw) = keys.open(&file.name, &sealed) else {
            synced.unreadable += 1;
            continue;
        };
        // Sealed with this key, but none this build reads: a later one's.
        let Some(read) = SettingsFile::read(&raw) else {
            synced.later += 1;
            continue;
        };
        if file.name == own_name {
            own.push(file);
            held = Some(match held {
                Some(earlier) => merge([&earlier, &read.keys]),
                None => read.keys,
            });
        } else {
            others.push(read.keys);
        }
    }
    let using = Using::of(store, here);
    let first = state.settings.is_none();
    let mine = if first && !others.is_empty() {
        // Joining: what the drive holds is what is wanted here.
        StampedSettings::new()
    } else {
        let base = state.settings.clone().unwrap_or_default();
        stamp(&using.keys(), &base, &state.stamps, now)
    };
    let merged = merge(others.iter().chain(held.iter()).chain([&mine]));
    let took = taken(&merged, &using);
    // Stored as the app stores each, the settings excepted: the app holds
    // those, and stores them itself as it takes them up.
    for (key, counters) in &took.counters {
        store.save_auto_counters(key, *counters);
    }
    if !took.cleared.is_empty() {
        store.clear_auto_counters(&took.cleared);
    }
    if let Some(theme) = &took.theme {
        store.save_theme(theme);
    }
    if took.tour_done {
        store.mark_tutorial_done();
    }
    if took.told_of_paddle_keys {
        store.mark_told_of_paddle_keys();
    }
    state.settings = Some(using.with(&took).keys());
    state.stamps = stamps_of(&merged);
    synced.settings = took.settings;
    synced.theme = took.theme;

    if held.as_ref() != Some(&merged) || own.len() > 1 {
        let file = SettingsFile::new(&state.device, now, merged);
        let raw = serde_json::to_vec(&file)
            .map_err(|_| SyncFailed::Store("the settings could not be written down".into()))?;
        let sealed = keys
            .seal(&own_name, &raw)
            .ok_or_else(|| SyncFailed::Store("this device could not seal what it sends".into()))?;
        match own.split_first() {
            Some((first, rest)) => {
                remote.update(&first.id, sealed).await?;
                // Two tabs of one browser that each made it at once: one
                // kept, holding what both held.
                for extra in rest {
                    match remote.delete(&extra.id).await {
                        Ok(()) | Err(RemoteError::Gone) => {}
                        Err(err) => return Err(err.into()),
                    }
                }
            }
            None => {
                remote.create(&own_name, sealed).await?;
            }
        }
    }
    Ok(())
}

/// Delete the synced copy from the drive: every file of Dust's, the
/// keyring first, so that a device syncing meanwhile stops as it finds it
/// gone rather than reading what is left. Says how many went.
pub async fn delete_copy(remote: &dyn Remote) -> Result<usize, RemoteError> {
    let mut files = remote.list().await?;
    files.retain(|file| file.name.starts_with("dust-"));
    files.sort_by_key(|file| file.name != KEYRING);
    let mut gone = 0;
    for file in files {
        match remote.delete(&file.id).await {
            Ok(()) | Err(RemoteError::Gone) => gone += 1,
            Err(err) => return Err(err),
        }
    }
    Ok(gone)
}

#[cfg(test)]
mod tests;
