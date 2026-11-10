//! What sync keeps on this device, between syncs and launches: the key,
//! what it has read of the drive, and what it has yet to send
//! ([`SyncState`]). Kept by the store, as one small file or key
//! ([`crate::persist::Store::load_sync`]); none at all while sync is off.
//!
//! What it has read of the drive is only so as not to read it all again:
//! were it lost, the next sync reads every pack again and comes to the same.
//! The two notes the plan reads ([`super::plan::Here`]) are what this
//! device did, and it alone knows: kept with the rest, and carried forward
//! by every sync. Two copies of the app on one device, tabs or windows,
//! keep one state between them, each laying what it did over what the
//! other kept ([`SyncState::merged`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::plan::DriveState;
use crate::persist::record::RecordId;

mod merge;

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
    /// The deletions here the drive holds, each with the time of the
    /// drive's deletion ([`super::plan::Here::applied`]): those taken from
    /// the drive, and those made here by hand once the drive holds them.
    /// Each until a sync finds its session here again, or put back here by
    /// hand since the one before.
    #[serde(default, with = "times")]
    pub applied: BTreeMap<RecordId, u64>,
    /// The packs sent whose answer never came, by name, each with the
    /// deletions by hand it holds and their stamps: the drive may have made
    /// one all the same. A sync that finds the pack there notes them in
    /// [`Self::applied`], as the answer would have; one that finds no such
    /// file drops the pack, never made, and sends them again.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "unanswered"
    )]
    pub unanswered: BTreeMap<String, BTreeMap<RecordId, u64>>,
    /// The sessions put back here by hand since the last sync, and when
    /// ([`super::plan::Here::revived`]): each stamped as it was noted
    /// ([`Self::stamp_change`]), and those noted before any pack was read
    /// stamped again once the first is.
    #[serde(default, with = "times")]
    pub revived: BTreeMap<RecordId, u64>,
    /// The deletions by hand a build before this one made here, and noted
    /// nothing of in [`Self::applied`], have been looked for and noted:
    /// those the drive held as deleted, sent from here or found there
    /// already. Looked for once, at the first sync after an update.
    #[serde(default)]
    pub deletions_noted: bool,
    /// The settings as they were at the last sync, to tell what changed
    /// since: none before the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Map<String, Value>>,
    /// When each setting was last changed, as of the last sync.
    #[serde(default)]
    pub stamps: BTreeMap<String, u64>,
    /// What a later build keeps here that this one does not know, by key,
    /// as it was read: written back as it was, so that what a later build
    /// notes outlasts this build's writes, as a record's keys do.
    #[serde(flatten)]
    pub unread: Map<String, Value>,
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
        self.drive.stamp_change(id, now)
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
        Ok(by_id(BTreeMap::<String, u64>::deserialize(input)?))
    }

    /// `named` by record: a name that is none is passed over.
    pub fn by_id(named: BTreeMap<String, u64>) -> BTreeMap<RecordId, u64> {
        named
            .into_iter()
            .filter_map(|(name, at)| Some((RecordId::parse(&name)?, at)))
            .collect()
    }
}

/// The packs whose answer never came, each with its records by name.
mod unanswered {
    use super::*;

    /// One pack's deletions, as [`super::times`] writes them.
    struct Named<'a>(&'a BTreeMap<RecordId, u64>);

    impl Serialize for Named<'_> {
        fn serialize<S: serde::Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
            times::serialize(self.0, out)
        }
    }

    pub fn serialize<S: serde::Serializer>(
        packs: &BTreeMap<String, BTreeMap<RecordId, u64>>,
        out: S,
    ) -> Result<S::Ok, S::Error> {
        out.collect_map(packs.iter().map(|(pack, held)| (pack, Named(held))))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        input: D,
    ) -> Result<BTreeMap<String, BTreeMap<RecordId, u64>>, D::Error> {
        let named = BTreeMap::<String, BTreeMap<String, u64>>::deserialize(input)?;
        Ok(named
            .into_iter()
            .map(|(pack, held)| (pack, times::by_id(held)))
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
        Ok(DriveState {
            puts: named
                .puts
                .iter()
                .filter_map(|name| RecordId::parse(name))
                .collect(),
            tombs: times::by_id(named.tombs),
            revives: times::by_id(named.revives),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u64) -> RecordId {
        RecordId {
            order: n * 1000,
            nonce: n,
        }
    }

    fn times(pairs: &[(u64, u64)]) -> BTreeMap<RecordId, u64> {
        pairs.iter().map(|&(n, at)| (id(n), at)).collect()
    }

    fn names(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn groups(n: u64) -> Map<String, Value> {
        Map::from_iter([("numGroups".to_string(), Value::from(n))])
    }

    /// One device's state of one sync.
    fn state() -> SyncState {
        SyncState::new(
            "device".into(),
            "00".repeat(32),
            "file1".into(),
            Some("refresh".into()),
        )
    }

    /// What two copies of the app keep of one sync is merged note by note:
    /// what either noted since stays, and what this copy's own sync sent, or
    /// found was no more to note, is gone, unless the other copy noted it
    /// again since; the packs read and what they say are put together; the
    /// settings are those of the later sync, the sign-in the one made since,
    /// and the rest the later or either's. Laid either way, it comes to the
    /// same.
    #[test]
    fn what_two_copies_keep_is_merged_note_by_note() {
        let mut base = state();
        base.last = Some(100);
        base.read = names(&["dust-p-1"]);
        base.drive.tombs = times(&[(1, 5), (2, 5), (5, 50), (6, 60)]);
        base.revived = times(&[(1, 10), (2, 20)]);
        base.applied = times(&[(5, 50), (6, 60)]);
        base.settings = Some(groups(5));
        base.stamps = BTreeMap::from([("numGroups".to_string(), 90)]);

        // This copy synced since: it sent 2 as put back, took 6 back as the
        // drive put it back, let 7 go and read a pack; and 3 was put back.
        let mut ours = base.clone();
        ours.last = Some(200);
        ours.read.insert("dust-p-2".into());
        ours.drive.revives.extend(times(&[(2, 20), (6, 65)]));
        ours.drive.tombs.insert(id(7), 70);
        ours.revived.remove(&id(2));
        ours.revived.insert(id(3), 30);
        ours.applied.remove(&id(6));
        ours.applied.insert(id(7), 70);
        ours.deletions_noted = true;
        ours.settings = Some(groups(7));
        ours.stamps = BTreeMap::from([("numGroups".to_string(), 190)]);

        // The other copy, meanwhile: 1 put back again, and 4; signed in
        // again; and an earlier sync of its own, which read a pack a later
        // build wrote, and one this copy read.
        let mut theirs = base.clone();
        theirs.last = Some(150);
        theirs.sign_in = Some("again".into());
        theirs.read.insert("dust-p-3".into());
        theirs.later = BTreeMap::from([("dust-p-9".to_string(), 1), ("dust-p-2".to_string(), 1)]);
        theirs.revived.extend(times(&[(1, 15), (4, 40)]));
        theirs.drive.tombs.insert(id(8), 80);
        theirs.applied.insert(id(8), 80);
        theirs.settings = Some(groups(6));
        theirs.stamps = BTreeMap::from([("numGroups".to_string(), 140)]);

        for merged in [
            ours.merged(Some(&base), &theirs),
            theirs.merged(Some(&base), &ours),
        ] {
            assert_eq!(
                merged.revived,
                times(&[(1, 15), (3, 30), (4, 40)]),
                "2 went, as sent; 1 at the later"
            );
            assert_eq!(
                merged.applied,
                times(&[(5, 50), (7, 70), (8, 80)]),
                "6 went, as taken back"
            );
            assert_eq!(merged.read, names(&["dust-p-1", "dust-p-2", "dust-p-3"]));
            assert_eq!(
                merged.later,
                BTreeMap::from([("dust-p-9".to_string(), 1)]),
                "one read is no later build's"
            );
            let mut drive = base.drive.clone();
            drive.revives.extend(times(&[(2, 20), (6, 65)]));
            drive.tombs.extend(times(&[(7, 70), (8, 80)]));
            assert_eq!(merged.drive, drive);
            assert_eq!(merged.last, Some(200), "the later");
            assert_eq!(
                (merged.settings, merged.stamps),
                (ours.settings.clone(), ours.stamps.clone()),
                "as of the later sync"
            );
            assert_eq!(merged.sign_in.as_deref(), Some("again"), "made since");
            assert!(merged.deletions_noted, "looked for, by either");
        }
    }

    /// Of two copies' syncs that ran at once, as where nothing locks, the one
    /// that began later can have read the drive before the other sent a change
    /// of a setting there: the settings each came to go together by the later
    /// word on each, by its stamp, as the drive keeps the newest of each, not
    /// whole as of the sync that began later. Taken whole from that one, the
    /// change the other sent was not in what sync keeps, and a setting changed
    /// by hand since, back to what that one held, read as no change at the next
    /// sync, which took the drive's older word over it.
    #[test]
    fn settings_of_two_syncs_run_at_once_go_by_the_later_stamp_of_each() {
        let mut base = state();
        base.last = Some(100);
        base.settings = Some(groups(36));
        base.stamps = BTreeMap::from([("numGroups".to_string(), 30)]);
        // This copy's sync began at 180, and sent 39, changed here.
        let mut ours = base.clone();
        ours.last = Some(180);
        ours.settings = Some(groups(39));
        ours.stamps = BTreeMap::from([("numGroups".to_string(), 180)]);
        // The other copy's began at 200, read the drive before that, and ended
        // first, taking nothing.
        let mut theirs = base.clone();
        theirs.last = Some(200);
        let merged = ours.merged(Some(&base), &theirs);
        assert_eq!(merged.settings, Some(groups(39)), "the later word on it");
        assert_eq!(merged.stamps.get("numGroups"), Some(&180));
    }

    /// Of one sync, as where neither copy's last sync is the later, the
    /// settings as of it go together setting by setting at the later stamp
    /// of each, as any two copies' do (`settings_by_stamp`), and at one
    /// stamp they are this copy's where it changed them: laid over a state
    /// another copy wrote since, which changed none of them, this copy's
    /// changes stay, one made at a later stamp and one at the stamp the
    /// setting had, as where two devices set it at once, stamped alike, and
    /// the drive told them apart by what they hold. Where this copy changed
    /// none of them, they are the other's. Gone to the other copy at one
    /// stamp whatever changed, the second change was lost.
    #[test]
    fn of_one_sync_the_settings_are_this_copys_where_it_changed_them() {
        let settings = |groups: u64, size: u64| {
            Map::from_iter([
                ("numGroups".to_string(), Value::from(groups)),
                ("groupSizeTarget".to_string(), Value::from(size)),
            ])
        };
        let stamped = |groups: u64, size: u64| {
            BTreeMap::from([
                ("numGroups".to_string(), groups),
                ("groupSizeTarget".to_string(), size),
            ])
        };
        let mut base = state();
        base.last = Some(100);
        base.settings = Some(settings(5, 5));
        base.stamps = stamped(90, 60);
        // Changed here: one at a later stamp, one at the stamp it had.
        let mut ours = base.clone();
        ours.settings = Some(settings(7, 4));
        ours.stamps = stamped(190, 60);
        let mut theirs = base.clone();
        theirs.revived = times(&[(1, 10)]);
        let merged = ours.merged(Some(&base), &theirs);
        assert_eq!(merged.last, Some(100));
        assert_eq!(
            (merged.settings, merged.stamps),
            (ours.settings.clone(), ours.stamps.clone()),
            "this copy's, changed since, at a later stamp and at one"
        );
        assert_eq!(merged.revived, theirs.revived, "and the other's note");
        let merged = theirs.merged(Some(&base), &ours);
        assert_eq!(
            (merged.settings, merged.stamps),
            (ours.settings, ours.stamps),
            "the other's, where this copy changed none"
        );
    }

    /// What another copy's write leaves out stays, as a tab can read what
    /// sync keeps from before another tab's last write, and write without
    /// what that one noted; it goes only on evidence. A putting back goes
    /// once the drive shows it sent, or a deletion of its session after it
    /// is noted; a deletion the drive holds, once the drive has put its
    /// session back since, and holds it so; a pack whose answer never came,
    /// once each deletion in it is noted. A note at an older stamp than
    /// this copy last saw is as one left out. Left out by this copy, which
    /// knows what it dropped, a putting back and a pack are gone; a
    /// deletion, only on the same evidence, as a sync here drops it as its
    /// session is put back here, before the putting back is in the drive.
    #[test]
    fn what_another_copy_left_out_goes_only_on_evidence() {
        let mut base = state();
        base.revived = times(&[(1, 10), (2, 20), (3, 30), (4, 40)]);
        base.applied = times(&[(5, 50), (6, 60), (7, 70)]);
        base.unanswered = BTreeMap::from([
            ("dust-p-1".to_string(), times(&[(8, 80)])),
            ("dust-p-2".to_string(), times(&[(9, 90)])),
        ]);
        base.drive.tombs = times(&[(5, 50), (6, 60), (7, 70), (8, 80), (9, 90)]);
        let ours = base.clone();

        let mut theirs = state();
        theirs.drive = base.drive.clone();
        theirs.revived = times(&[(4, 35)]);
        theirs.drive.revives.insert(id(2), 20);
        theirs.drive.tombs.insert(id(3), 31);
        theirs.applied = times(&[(3, 31), (9, 90)]);
        theirs.drive.revives.extend(times(&[(6, 65), (7, 75)]));
        theirs.drive.tombs.insert(id(7), 76);
        let merged = ours.merged(Some(&base), &theirs);
        assert_eq!(
            merged.revived,
            times(&[(1, 10), (4, 40)]),
            "1, nothing saying it went; 4, at the stamp seen here"
        );
        assert_eq!(
            merged.applied,
            times(&[(3, 31), (5, 50), (7, 70), (9, 90)]),
            "6 put back since, and so held; 7 deleted again after"
        );
        assert_eq!(
            merged.unanswered.keys().collect::<Vec<_>>(),
            ["dust-p-1"],
            "the deletion in dust-p-2 is noted"
        );

        let mut synced = base.clone();
        synced.revived.clear();
        synced.applied.clear();
        synced.unanswered.clear();
        // 6 put back here, and the putting back sent.
        synced.drive.revives.insert(id(6), 61);
        let merged = synced.merged(Some(&base), &base);
        assert!(
            merged.revived.is_empty() && merged.unanswered.is_empty(),
            "{merged:?}"
        );
        assert_eq!(
            merged.applied,
            times(&[(5, 50), (7, 70)]),
            "6 put back in the drive; 5 and 7 not yet"
        );
    }

    /// A deletion's note this copy dropped goes on the evidence asked of one
    /// the other copy dropped, and only on it.
    #[test]
    fn a_deletion_this_copy_left_out_goes_on_evidence_alone() {
        let mut base = state();
        base.applied = times(&[(5, 50), (6, 60)]);
        let mut ours = base.clone();
        ours.applied.clear();
        ours.drive.revives.insert(id(6), 61);
        let merged = ours.merged(Some(&base), &base);
        assert_eq!(
            merged.applied,
            times(&[(5, 50)]),
            "6 put back in the drive; 5 not yet"
        );
    }

    /// What a later build keeps for sync, which this one does not know, is
    /// written back as it was read, and as two copies' states are merged,
    /// as the store holds it, since this copy never changes it: so a later
    /// build's notes survive this build's writes, and one it let go stays
    /// gone though a copy of this build that read it is still open. A key
    /// this build knows is written once.
    #[test]
    fn what_a_later_build_keeps_for_sync_is_written_back() {
        let raw = r#"{"dustSyncState":1,"device":"d","key":"k","laterNote":{"a":[1,2]},"laterFlag":true}"#;
        let state = SyncState::read(raw).expect("read");
        let written: Value =
            serde_json::from_str(&state.written().expect("written")).expect("JSON");
        assert_eq!(written["laterNote"], json!({ "a": [1, 2] }));
        assert_eq!(written["laterFlag"], json!(true));

        let ours = SyncState::read(
            r#"{"dustSyncState":1,"device":"d","key":"k","laterFlag":false,"laterOld":1}"#,
        )
        .expect("read");
        let merged = ours.merged(Some(&ours), &state).written().expect("written");
        assert_eq!(merged.matches(r#""device""#).count(), 1, "{merged}");
        let merged: Value = serde_json::from_str(&merged).expect("JSON");
        assert_eq!(merged["laterFlag"], json!(true), "the store's");
        assert_eq!(merged["laterNote"], json!({ "a": [1, 2] }));
        assert!(merged.get("laterOld").is_none(), "the store let it go");
    }

    /// A state of another sync in the store, as one turned on anew in
    /// another copy of the app, is taken as it is: this copy's notes are of
    /// a sync that is no more.
    #[test]
    fn another_sync_in_the_store_is_taken_as_it_is() {
        let mut ours = state();
        ours.revived = times(&[(1, 10)]);
        let anew = SyncState::new("another".into(), "11".repeat(32), "file2".into(), None);
        assert_eq!(ours.merged(Some(&ours.clone()), &anew), anew);
        let mut begun_anew = state();
        begun_anew.keyring = "file2".into();
        assert_eq!(ours.merged(Some(&ours.clone()), &begun_anew), begun_anew);
    }

    /// A sync turned on here that the store never took is kept whole over
    /// another the store holds, as the one last chosen here, as a whole
    /// write keeps it: so is one turned on anew here since this copy last
    /// read or kept the store's, which was of the sync before.
    #[test]
    fn a_sync_turned_on_here_the_store_never_took_is_kept_whole() {
        let kept = state();
        let mut on_here = state();
        on_here.keyring = "file2".into();
        on_here.revived = times(&[(1, 10)]);
        assert_eq!(on_here.merged(None, &kept), on_here, "never kept");
        assert_eq!(
            on_here.merged(Some(&kept), &kept),
            on_here,
            "turned on anew since"
        );
    }

    /// A state the store never took from this copy, as one just turned on,
    /// is merged as against nothing: what either holds, this copy's where
    /// each has its own.
    #[test]
    fn a_state_never_kept_is_merged_as_against_nothing() {
        let mut ours = state();
        ours.revived = times(&[(1, 10)]);
        ours.sign_in = Some("mine".into());
        let mut theirs = state();
        theirs.revived = times(&[(1, 12), (2, 20)]);
        theirs.sign_in = Some("theirs".into());
        let merged = ours.merged(None, &theirs);
        assert_eq!(merged.revived, times(&[(1, 12), (2, 20)]));
        assert_eq!(merged.sign_in.as_deref(), Some("mine"));
    }

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
        state
            .unanswered
            .insert("dust-p-3".into(), BTreeMap::from([(id, 6)]));
        state.revived.insert(id, 9);
        state.deletions_noted = true;
        state.settings = Some(Map::from_iter([("numGroups".to_string(), Value::from(5))]));
        state.stamps.insert("numGroups".into(), 3);
        state.unread.insert("laterKey".into(), json!({ "a": 1 }));
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
        assert!(!least.deletions_noted, "looked for at the next sync");
    }
}
