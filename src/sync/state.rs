//! What sync keeps on this device, between syncs and launches: the key,
//! what it has read of the drive, and what it has yet to send
//! ([`SyncState`]). Kept by the store, as one small file or key
//! ([`crate::persist::Store::load_sync`]); none at all while sync is off.
//!
//! What it has read of the drive is only so as not to read it all again:
//! were it lost, the next sync reads every pack again and comes to the same.
//! The two notes the plan reads ([`super::plan::Here`]) are what this
//! device did, and it alone knows: kept with the rest, and carried forward
//! by every sync.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::plan::DriveState;
use crate::persist::record::RecordId;

/// The layout of what sync keeps.
const STATE_LAYOUT: u32 = 1;

/// What sync keeps on this device while it is on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub dust_sync_state: u32,
    /// This device, at random: what its packs and its settings file go by.
    pub device: String,
    /// The data key, in hex ([`super::seal::DataKey`]).
    pub key: String,
    /// The keyring it came from, as the drive knows it: one found there in
    /// its place is a sync begun anew, which this key does not open.
    #[serde(default)]
    pub keyring: String,
    /// What the sign-in keeps to sign in again with no one asked, where it
    /// keeps anything: a refresh token, on a computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<String>,
    /// When this device last synced, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<u64>,
    /// The packs read, by name.
    #[serde(default)]
    pub read: BTreeSet<String>,
    /// The packs a later build wrote, by name, which a build of the layout
    /// beside each could not read: fetched again only by a build of a
    /// later layout ([`super::pack::PACK_LAYOUT`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub later: BTreeMap<String, u32>,
    /// What those packs put together say.
    #[serde(default, with = "drive_state")]
    pub drive: DriveState,
    /// The deletions taken from the drive and made here, each with the
    /// time of the drive's deletion ([`super::plan::Here::applied`]).
    #[serde(default, with = "times")]
    pub applied: BTreeMap<RecordId, u64>,
    /// The sessions put back here by hand since the last sync, and when
    /// ([`super::plan::Here::revived`]).
    #[serde(default, with = "times")]
    pub revived: BTreeMap<RecordId, u64>,
    /// The settings as they were at the last sync, to tell what changed
    /// since: none before the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Map<String, Value>>,
    /// When each setting was last changed, as of the last sync.
    #[serde(default)]
    pub stamps: BTreeMap<String, u64>,
}

impl SyncState {
    /// A device just joined to the sync whose data key is `key`.
    pub fn new(device: String, key: String, keyring: String, sign_in: Option<String>) -> Self {
        Self {
            dust_sync_state: STATE_LAYOUT,
            device,
            key,
            keyring,
            sign_in,
            ..Self::default()
        }
    }

    /// What sync kept, `raw`, if it is any this build reads.
    pub fn read(raw: &str) -> Option<Self> {
        let state: Self = serde_json::from_str(raw).ok()?;
        (state.dust_sync_state == STATE_LAYOUT && !state.device.is_empty()).then_some(state)
    }

    pub fn written(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    /// When a change this device makes to the record `id` at `now`, by its
    /// own clock, is stamped: a deletion by hand, or a putting back. At
    /// `now`, or just after the latest deletion or putting back of it this
    /// device knows the drive to hold, where that is later: the drive's
    /// word on a record goes by which came last ([`super::plan`]), and a
    /// change made after another is so the later, however far apart the
    /// clocks of the devices that made them are.
    pub fn stamp_change(&self, id: RecordId, now: u64) -> u64 {
        let seen = self.drive.tombs.get(&id).max(self.drive.revives.get(&id));
        seen.map_or(now, |&seen| now.max(seen.saturating_add(1)))
    }
}

/// Records by name, as JSON keys must be strings.
mod times {
    use super::*;

    pub fn serialize<S: serde::Serializer>(
        times: &BTreeMap<RecordId, u64>,
        out: S,
    ) -> Result<S::Ok, S::Error> {
        out.collect_map(times.iter().map(|(id, at)| (id.name(), at)))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        input: D,
    ) -> Result<BTreeMap<RecordId, u64>, D::Error> {
        let named = BTreeMap::<String, u64>::deserialize(input)?;
        Ok(named
            .into_iter()
            .filter_map(|(name, at)| Some((RecordId::parse(&name)?, at)))
            .collect())
    }
}

/// What the drive says, by name.
mod drive_state {
    use super::*;

    #[derive(Default, Serialize, Deserialize)]
    struct Named {
        #[serde(default)]
        puts: Vec<String>,
        #[serde(default)]
        tombs: BTreeMap<String, u64>,
        #[serde(default)]
        revives: BTreeMap<String, u64>,
    }

    pub fn serialize<S: serde::Serializer>(drive: &DriveState, out: S) -> Result<S::Ok, S::Error> {
        Named {
            puts: drive.puts.iter().map(|id| id.name()).collect(),
            tombs: drive
                .tombs
                .iter()
                .map(|(id, at)| (id.name(), *at))
                .collect(),
            revives: drive
                .revives
                .iter()
                .map(|(id, at)| (id.name(), *at))
                .collect(),
        }
        .serialize(out)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(input: D) -> Result<DriveState, D::Error> {
        let named = Named::deserialize(input)?;
        let parse = |pairs: BTreeMap<String, u64>| {
            pairs
                .into_iter()
                .filter_map(|(name, at)| Some((RecordId::parse(&name)?, at)))
                .collect()
        };
        Ok(DriveState {
            puts: named
                .puts
                .iter()
                .filter_map(|name| RecordId::parse(name))
                .collect(),
            tombs: parse(named.tombs),
            revives: parse(named.revives),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What sync keeps reads back as it was written.
    #[test]
    fn what_sync_keeps_reads_back() {
        let id = RecordId {
            order: 1_790_000_000_000_000,
            nonce: 0xabc,
        };
        let mut state = SyncState::new(
            "device".into(),
            "00".repeat(32),
            "file1".into(),
            Some("refresh".into()),
        );
        state.last = Some(5);
        state.read.insert("dust-p-1".into());
        state.later.insert("dust-p-2".into(), 1);
        state.drive.puts.insert(id);
        state.drive.tombs.insert(id, 7);
        state.drive.revives.insert(id, 8);
        state.applied.insert(id, 7);
        state.revived.insert(id, 9);
        state.settings = Some(Map::from_iter([("numGroups".to_string(), Value::from(5))]));
        state.stamps.insert("numGroups".into(), 3);
        let written = state.written().expect("written");
        assert_eq!(SyncState::read(&written), Some(state));
    }

    /// Kept by another layout, or by nothing that reads, it is no state.
    #[test]
    fn what_sync_keeps_in_a_layout_it_does_not_read_is_none() {
        assert_eq!(SyncState::read("not json"), None);
        assert_eq!(
            SyncState::read(r#"{"dustSyncState":2,"device":"d","key":""}"#),
            None
        );
        assert_eq!(
            SyncState::read(r#"{"dustSyncState":1,"device":"","key":""}"#),
            None
        );
        let least = SyncState::read(r#"{"dustSyncState":1,"device":"d","key":"k"}"#)
            .expect("the least it keeps");
        assert_eq!(
            least,
            SyncState::new("d".into(), "k".into(), String::new(), None)
        );
    }
}
