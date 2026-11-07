//! A sync, start to end, against a store and a folder: finding the keyring
//! as sync is turned on ([`look`], [`join`]), and each sync after
//! ([`sync_once`]). Nothing here knows which store or folder it has, nor
//! touches the app: it says what it did, and the app takes it up.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cw_core::TrainingSettings;
use serde_json::Map;

use super::pack::{
    Forget, PACK_PREFIX, Pack, Put, Revive, Unopened, open_pack, pack_name, packs_of, seal_pack,
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
/// before. Says how many files would not open, or not read: one a later
/// build wrote is not noted, and is read again at the next sync.
async fn read_packs(
    remote: &dyn Remote,
    keys: &Keys,
    files: &[RemoteFile],
    read: &mut BTreeSet<String>,
    drive: &mut DriveState,
    bytes: &mut HashMap<RecordId, String>,
) -> Result<usize, RemoteError> {
    let mut unreadable = 0;
    for file in files {
        if !file.name.starts_with(PACK_PREFIX) || read.contains(&file.name) {
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
            }
            Err(Unopened::Damaged) => unreadable += 1,
            // What it holds arrives once this build is updated.
            Err(Unopened::Later) => {
                unreadable += 1;
                continue;
            }
        }
        // Read, whether it opened or not: one damaged does not open the
        // next time either.
        read.insert(file.name.clone());
    }
    Ok(unreadable)
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
pub async fn sync_once(
    store: &dyn Store,
    remote: &dyn Remote,
    state: &mut SyncState,
    here: &TrainingSettings,
    now: u64,
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
    synced.unreadable +=
        read_packs(remote, &keys, &files, &mut read, &mut drive, &mut bytes).await?;

    let listed: BTreeSet<RecordId> = store
        .history_listing()
        .await
        .ok_or_else(|| SyncFailed::Store("the history here could not be read".into()))?
        .into_iter()
        .collect();
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
        synced.unreadable =
            read_packs(remote, &keys, &files, &mut read, &mut drive, &mut bytes).await?;
        planned = self::planned(store, &drive, &listed, state).await;
    }

    // What the drive holds, taken here.
    let here_bytes = read_here(store, &planned).await;
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
    for &(id, at) in &planned.let_go {
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
        .map(|id| Forget {
            id: id.name(),
            at: now,
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
        send_pack(remote, &keys, state, &pack).await?;
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

/// The plan for this sync, the store asked which of what the drive holds
/// and it does not list it deleted by hand.
async fn planned(
    store: &dyn Store,
    drive: &DriveState,
    listed: &BTreeSet<RecordId>,
    state: &SyncState,
) -> Plan {
    let missing: Vec<RecordId> = drive
        .alive_ids()
        .into_iter()
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
async fn send_pack(
    remote: &dyn Remote,
    keys: &Keys,
    state: &mut SyncState,
    pack: &Pack,
) -> Result<(), SyncFailed> {
    let name =
        pack_name().ok_or_else(|| SyncFailed::Store("this device could not name a file".into()))?;
    let sealed = seal_pack(keys, &name, pack)
        .ok_or_else(|| SyncFailed::Store("this device could not seal what it sends".into()))?;
    remote.create(&name, sealed).await?;
    state.read.insert(name);
    state.drive.take(pack);
    Ok(())
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
        let Some(read) = keys
            .open(&file.name, &sealed)
            .and_then(|raw| SettingsFile::read(&raw))
        else {
            synced.unreadable += 1;
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
