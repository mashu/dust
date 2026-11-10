//! What one sync does, worked out from what the drive holds and what this
//! device does ([`plan`]): which records to take from the drive, which to
//! let go here, and which to send.
//!
//! Every pack in the drive put together says, of each record, whether it
//! is there, and whether it is deleted ([`DriveState`]): deleted where the
//! latest deletion of it is later than the latest time it was put back. A
//! record is never taken off the drive, and a deletion never forgotten, so
//! the drive's word on a record only ever moves forward, and every device
//! that reads every pack comes to the same word, whatever order it read
//! them in.
//!
//! This device's side is the store's: the records it lists, those of them
//! it deleted by hand and when ([`crate::persist::Store::deletions`]), and
//! two notes sync keeps of its own ([`super::state::SyncState`]): the
//! deletions here the drive holds, those it took from the drive and those
//! made here by hand that it holds, and the sessions put back here by hand
//! since the last sync. A record the store does not list is never deleted
//! on that account alone: missing is never deleted.

use std::collections::{BTreeMap, BTreeSet};

use super::pack::Pack;
use crate::persist::record::RecordId;

/// What the drive holds, every pack read put together: of each record,
/// that it is there, and the latest time it was deleted and put back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DriveState {
    /// The records some pack holds.
    pub puts: BTreeSet<RecordId>,
    /// The latest deletion of each record deleted.
    pub tombs: BTreeMap<RecordId, u64>,
    /// The latest time each record put back was put back.
    pub revives: BTreeMap<RecordId, u64>,
}

impl DriveState {
    /// Take `pack` in. An entry whose record's name does not read is
    /// passed over.
    pub fn take(&mut self, pack: &Pack) {
        for put in &pack.put {
            if let Some(id) = put.record_id() {
                self.puts.insert(id);
            }
        }
        for forget in &pack.forget {
            if let Some(id) = forget.record_id() {
                latest(&mut self.tombs, id, forget.at);
            }
        }
        for revive in &pack.revive {
            if let Some(id) = revive.record_id() {
                latest(&mut self.revives, id, revive.at);
            }
        }
    }

    /// The drive says `id` is deleted: its latest deletion is later than
    /// its latest putting back, if any.
    pub fn deleted(&self, id: RecordId) -> bool {
        self.tombs
            .get(&id)
            .is_some_and(|&tomb| self.revives.get(&id).is_none_or(|&revive| tomb > revive))
    }

    /// The drive holds `id`, and does not say it is deleted.
    pub fn alive(&self, id: RecordId) -> bool {
        (self.puts.contains(&id) || self.revives.contains_key(&id)) && !self.deleted(id)
    }

    /// Every record the drive holds and does not say is deleted.
    pub fn alive_ids(&self) -> BTreeSet<RecordId> {
        self.puts
            .iter()
            .chain(self.revives.keys())
            .copied()
            .filter(|id| !self.deleted(*id))
            .collect()
    }

    /// Take in what `other` says of the drive, as had every pack it was
    /// read from been read here too: the records either holds, and the
    /// latest deletion and putting back of each that either knows. Two
    /// copies of the app on one device each read packs of their own, and
    /// what both read is put together ([`super::state::SyncState::merged`]).
    pub fn join(&mut self, other: &DriveState) {
        self.puts.extend(other.puts.iter().copied());
        for (&id, &at) in &other.tombs {
            latest(&mut self.tombs, id, at);
        }
        for (&id, &at) in &other.revives {
            latest(&mut self.revives, id, at);
        }
    }

    /// When a change to the record `id` made at `now` is stamped, as far
    /// as this says of the drive: at `now`, or just after the latest
    /// deletion or putting back of it, where that is later
    /// ([`super::state::SyncState::stamp_change`]).
    pub fn stamp_change(&self, id: RecordId, now: u64) -> u64 {
        let seen = self.tombs.get(&id).max(self.revives.get(&id));
        seen.map_or(now, |&seen| now.max(seen.saturating_add(1)))
    }
}

/// `at` for `id` in `times`, where it is later than what is there.
fn latest(times: &mut BTreeMap<RecordId, u64>, id: RecordId, at: u64) {
    let time = times.entry(id).or_insert(at);
    *time = (*time).max(at);
}

/// This device's side of a sync.
#[derive(Clone, Copy, Debug)]
pub struct Here<'a> {
    /// Every record the store lists.
    pub listed: &'a BTreeSet<RecordId>,
    /// Of the records the drive holds and the store does not list, those it
    /// deleted by hand: of those the drive holds alive, and of those it
    /// deleted since the last sync, or since this device put them back by
    /// hand.
    pub forgotten: &'a BTreeSet<RecordId>,
    /// Of those `forgotten`, when each was deleted here, by this device's
    /// clock, where its tombstone says.
    pub made: &'a BTreeMap<RecordId, u64>,
    /// The deletions here the drive holds, each with the time of the
    /// drive's deletion: those taken from the drive and made here, and
    /// those made here by hand once the drive holds them, sent from here or
    /// made on another device as well.
    pub applied: &'a BTreeMap<RecordId, u64>,
    /// The sessions put back here by hand since the last sync, and when.
    pub revived: &'a BTreeMap<RecordId, u64>,
}

/// What one sync does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Records the drive holds that this device never had: to store here.
    pub take: Vec<RecordId>,
    /// Records this device let go as the drive deleted them, and the drive
    /// has put back since: to store here again, the deletion undone.
    pub take_back: Vec<RecordId>,
    /// Records the drive deleted that this device still has, each with when
    /// the drive deleted it: to delete here.
    pub let_go: Vec<(RecordId, u64)>,
    /// Records this device has and the drive does not: to send.
    pub send: Vec<RecordId>,
    /// Records the drive holds that this device deleted by hand: to send
    /// the deletion of. And those the drive deleted as well, where this
    /// device deleted them after it, by its own clock: sent, so that a
    /// putting back made between the two loses to the later on every
    /// device, as the drive reads them.
    pub send_deleted: Vec<RecordId>,
    /// Records this device deleted by hand that the drive deleted as well,
    /// no earlier, or at a time the store cannot say, each with when the
    /// drive deleted it: nothing to send, and the deletion noted here as
    /// one taken from the drive.
    pub deleted_too: Vec<(RecordId, u64)>,
    /// Records the drive deleted that this device put back by hand since,
    /// each with when: to send that they are back.
    pub send_back: Vec<(RecordId, u64)>,
}

impl Plan {
    /// The sync changes nothing, here or in the drive.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.take.is_empty()
            && self.take_back.is_empty()
            && self.let_go.is_empty()
            && self.send.is_empty()
            && self.send_deleted.is_empty()
            && self.send_back.is_empty()
    }

    /// The sync sends anything to the drive.
    #[cfg(test)]
    pub fn sends(&self) -> bool {
        !(self.send.is_empty() && self.send_deleted.is_empty() && self.send_back.is_empty())
    }
}

/// What a sync does, from what the drive holds, `drive`, and what this
/// device does, `here`.
pub fn plan(drive: &DriveState, here: Here) -> Plan {
    let mut plan = Plan::default();
    for &id in here.listed {
        if drive.deleted(id) {
            let tomb = drive.tombs[&id];
            match here.revived.get(&id) {
                // Put back here no earlier than the drive's latest deletion
                // of it: as late, it stands, as the drive reads the two.
                Some(&at) if at >= tomb => plan.send_back.push((id, at)),
                _ => plan.let_go.push((id, tomb)),
            }
        } else if !drive.puts.contains(&id) && !drive.revives.contains_key(&id) {
            plan.send.push(id);
        }
    }
    for id in drive.alive_ids() {
        if here.listed.contains(&id) {
            continue;
        }
        if !here.forgotten.contains(&id) {
            plan.take.push(id);
            continue;
        }
        // Deleted here: as the drive deleted it, and the drive has put it
        // back since, as late as that deletion or later; or by hand, which
        // the drive is to hear of.
        let put_back_since = here
            .applied
            .get(&id)
            .is_some_and(|&took| drive.revives.get(&id).is_some_and(|&back| back >= took));
        if put_back_since {
            plan.take_back.push(id);
        } else if !here.applied.contains_key(&id) {
            plan.send_deleted.push(id);
        }
    }
    // Deleted here by hand, and in the drive as well, by another device
    // say: the drive's deletion is this device's word on it too, and a
    // putting back as late or later brings it back here as for one let go.
    // But one made here after the drive's, by this device's clock, is the
    // later word, and is sent: a putting back between the two loses to it.
    for &id in here.forgotten {
        if drive.deleted(id) && !here.listed.contains(&id) && !here.applied.contains_key(&id) {
            let tomb = drive.tombs[&id];
            if here.made.get(&id).is_some_and(|&made| made > tomb) {
                plan.send_deleted.push(id);
            } else {
                plan.deleted_too.push((id, tomb));
            }
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::pack::{Forget, Put, Revive};

    fn id(n: u64) -> RecordId {
        RecordId {
            order: n * 1000,
            nonce: n,
        }
    }

    fn ids(ns: &[u64]) -> BTreeSet<RecordId> {
        ns.iter().map(|&n| id(n)).collect()
    }

    fn times(pairs: &[(u64, u64)]) -> BTreeMap<RecordId, u64> {
        pairs.iter().map(|&(n, at)| (id(n), at)).collect()
    }

    fn pack(put: &[u64], forget: &[(u64, u64)], revive: &[(u64, u64)]) -> Pack {
        Pack {
            put: put
                .iter()
                .map(|&n| Put {
                    id: id(n).name(),
                    s: "{}".into(),
                })
                .collect(),
            forget: forget
                .iter()
                .map(|&(n, at)| Forget {
                    id: id(n).name(),
                    at,
                })
                .collect(),
            revive: revive
                .iter()
                .map(|&(n, at)| Revive {
                    id: id(n).name(),
                    at,
                    s: "{}".into(),
                })
                .collect(),
            ..Pack::default()
        }
    }

    fn drive(packs: &[Pack]) -> DriveState {
        let mut drive = DriveState::default();
        for pack in packs {
            drive.take(pack);
        }
        drive
    }

    struct Side {
        listed: BTreeSet<RecordId>,
        forgotten: BTreeSet<RecordId>,
        made: BTreeMap<RecordId, u64>,
        applied: BTreeMap<RecordId, u64>,
        revived: BTreeMap<RecordId, u64>,
    }

    impl Side {
        fn listing(ns: &[u64]) -> Self {
            Self {
                listed: ids(ns),
                forgotten: BTreeSet::new(),
                made: BTreeMap::new(),
                applied: BTreeMap::new(),
                revived: BTreeMap::new(),
            }
        }

        fn plan(&self, drive: &DriveState) -> Plan {
            plan(
                drive,
                Here {
                    listed: &self.listed,
                    forgotten: &self.forgotten,
                    made: &self.made,
                    applied: &self.applied,
                    revived: &self.revived,
                },
            )
        }
    }

    /// The drive's word on a record is the same whatever order its packs
    /// are read in: deleted where its latest deletion is later than its
    /// latest putting back.
    #[test]
    fn the_drive_says_the_same_in_any_order() {
        let packs = [
            pack(&[1, 2, 3], &[], &[]),
            pack(&[], &[(2, 10), (3, 10)], &[]),
            pack(&[], &[], &[(3, 20)]),
            pack(&[], &[(3, 15)], &[]),
            pack(&[4], &[(4, 30)], &[(4, 30)]),
        ];
        let forwards = drive(&packs);
        let mut reversed = packs.to_vec();
        reversed.reverse();
        assert_eq!(drive(&reversed), forwards);
        assert!(forwards.alive(id(1)) && !forwards.deleted(id(1)));
        assert!(forwards.deleted(id(2)) && !forwards.alive(id(2)));
        assert!(forwards.alive(id(3)), "put back after its latest deletion");
        assert!(!forwards.deleted(id(4)), "put back as it was deleted");
        assert_eq!(forwards.tombs[&id(3)], 15);
        assert_eq!(forwards.alive_ids(), ids(&[1, 3, 4]));
        // A name that does not read is passed over.
        let mut odd = pack(&[], &[], &[]);
        odd.put.push(Put {
            id: "not a name".into(),
            s: "{}".into(),
        });
        assert_eq!(drive(&[odd]), DriveState::default());
    }

    /// A first sync sends everything here, and takes everything there.
    #[test]
    fn a_first_sync_sends_what_is_here_and_takes_what_is_there() {
        let there = drive(&[pack(&[1, 2], &[], &[])]);
        let plan = Side::listing(&[2, 3]).plan(&there);
        assert_eq!(plan.take, [id(1)]);
        assert_eq!(plan.send, [id(3)]);
        assert!(plan.let_go.is_empty() && plan.send_deleted.is_empty());
        // Nothing to do once both hold the same.
        assert!(Side::listing(&[1, 2]).plan(&there).is_empty());
        assert!(!Side::listing(&[1, 2]).plan(&there).sends());
        assert!(Side::listing(&[]).plan(&DriveState::default()).is_empty());
    }

    /// A deletion made elsewhere is made here; one made here by hand is
    /// sent; and one this device took from the drive is not sent back.
    #[test]
    fn a_deletion_goes_wherever_the_record_is() {
        let there = drive(&[pack(&[1, 2], &[(1, 50)], &[])]);
        let here = Side::listing(&[1, 2]);
        assert_eq!(here.plan(&there).let_go, [(id(1), 50)]);

        let mut here = Side::listing(&[1]);
        here.forgotten = ids(&[2]);
        let plan = here.plan(&there);
        assert_eq!(plan.send_deleted, [id(2)], "deleted here by hand");
        assert_eq!(plan.let_go, [(id(1), 50)]);
        assert!(plan.take.is_empty(), "never taken again");

        // Once made here, the drive's deletion is left as it is.
        let mut here = Side::listing(&[2]);
        here.forgotten = ids(&[1]);
        here.applied = times(&[(1, 50)]);
        assert!(here.plan(&there).is_empty());
    }

    /// A session put back by hand after a deletion the drive holds is sent
    /// as put back; one put back before the drive's deletion goes.
    #[test]
    fn a_session_put_back_here_is_sent_as_back() {
        let there = drive(&[pack(&[1], &[(1, 50)], &[])]);
        let mut here = Side::listing(&[1]);
        here.revived = times(&[(1, 60)]);
        assert_eq!(here.plan(&there).send_back, [(id(1), 60)]);
        assert!(here.plan(&there).let_go.is_empty());
        here.revived = times(&[(1, 40)]);
        assert_eq!(here.plan(&there).let_go, [(id(1), 50)], "deleted since");
        assert!(here.plan(&there).send_back.is_empty());
    }

    /// A putting back as late as the drive's latest deletion stands, as the
    /// drive itself reads the two: one made here is sent as back, and one
    /// the drive holds as late as the deletion this device took brings the
    /// session back. Each used to let the session go here, though the drive
    /// says it is there.
    #[test]
    fn a_putting_back_as_late_as_a_deletion_stands() {
        let there = drive(&[pack(&[1], &[(1, 50)], &[])]);
        let mut here = Side::listing(&[1]);
        here.revived = times(&[(1, 50)]);
        assert_eq!(here.plan(&there).send_back, [(id(1), 50)]);
        assert!(here.plan(&there).let_go.is_empty());

        let there = drive(&[pack(&[1], &[(1, 50)], &[(1, 50)])]);
        assert!(there.alive(id(1)), "the drive's word");
        let mut here = Side::listing(&[]);
        here.forgotten = ids(&[1]);
        here.applied = times(&[(1, 50)]);
        assert_eq!(here.plan(&there).take_back, [id(1)]);
    }

    /// A session the drive put back after the deletion this device took
    /// from it comes back here; one this device deleted by hand, whatever
    /// the drive says, is sent as deleted.
    #[test]
    fn a_session_put_back_elsewhere_comes_back() {
        let there = drive(&[pack(&[1], &[(1, 50)], &[(1, 60)])]);
        let mut here = Side::listing(&[]);
        here.forgotten = ids(&[1]);
        here.applied = times(&[(1, 50)]);
        assert_eq!(here.plan(&there).take_back, [id(1)]);
        here.applied.clear();
        assert_eq!(here.plan(&there).send_deleted, [id(1)]);
        // Never had here: taken.
        assert_eq!(Side::listing(&[]).plan(&there).take, [id(1)]);
    }

    /// A session deleted here by hand that the drive deleted as well has
    /// nothing to send, and is noted with the drive's deletion: noted, the
    /// drive putting it back after brings it back here.
    #[test]
    fn a_session_deleted_here_and_in_the_drive_is_noted_as_the_drives() {
        let there = drive(&[pack(&[1], &[(1, 50)], &[])]);
        let mut here = Side::listing(&[]);
        here.forgotten = ids(&[1]);
        let plan = here.plan(&there);
        assert_eq!(plan.deleted_too, [(id(1), 50)]);
        assert!(!plan.sends());
        here.applied = times(&[(1, 50)]);
        let again = here.plan(&there);
        assert!(
            again.is_empty() && again.deleted_too.is_empty(),
            "noted once"
        );

        let there = drive(&[pack(&[1], &[(1, 50)], &[(1, 60)])]);
        assert_eq!(here.plan(&there).take_back, [id(1)]);
    }

    /// A session deleted here by hand after the drive's deletion of it, by
    /// this device's clock, has its deletion sent, so that a putting back
    /// made elsewhere between the two loses to it on every device; one
    /// deleted here no later than the drive's, or at a time its tombstone
    /// does not say, is the drive's. The later one used to be taken for the
    /// drive's as well, and such a putting back brought the session back
    /// here.
    #[test]
    fn a_deletion_made_here_after_the_drives_is_sent() {
        let there = drive(&[pack(&[1], &[(1, 50)], &[])]);
        let mut here = Side::listing(&[]);
        here.forgotten = ids(&[1]);
        here.made = times(&[(1, 60)]);
        let plan = here.plan(&there);
        assert_eq!(plan.send_deleted, [id(1)], "the later deletion");
        assert!(plan.deleted_too.is_empty());
        for made in [&[(1, 50)][..], &[(1, 40)], &[]] {
            here.made = times(made);
            let plan = here.plan(&there);
            assert_eq!(plan.deleted_too, [(id(1), 50)], "made {made:?}");
            assert!(!plan.sends(), "made {made:?}");
        }
    }

    /// What two copies of the app read of the drive, put together, says
    /// what every pack either read says, whichever is taken into which, and
    /// again and again.
    #[test]
    fn what_two_copies_read_of_the_drive_is_joined() {
        let packs = [
            pack(&[1, 2, 3], &[], &[]),
            pack(&[], &[(2, 10), (3, 10)], &[]),
            pack(&[], &[], &[(3, 20)]),
            pack(&[4], &[(3, 15), (4, 30)], &[(2, 5)]),
        ];
        let (one, other) = (drive(&packs[..2]), drive(&packs[2..]));
        let mut joined = one.clone();
        joined.join(&other);
        assert_eq!(joined, drive(&packs), "every pack's word");
        let mut the_other_way = other.clone();
        the_other_way.join(&one);
        assert_eq!(the_other_way, joined);
        let mut again = joined.clone();
        again.join(&one);
        assert_eq!(again, joined, "taken in twice, the same");
    }

    /// Two devices that each sync until neither has anything to do end up
    /// with the same records, and the drive with every deletion either
    /// made: the drive's word moves only forward.
    #[test]
    fn two_devices_come_to_the_same_history() {
        // Phone: 1, 2, 3; deleted 2 by hand. Computer: 3, 4.
        let mut phone = Side::listing(&[1, 3]);
        phone.forgotten = ids(&[2]);
        let mut computer = Side::listing(&[3, 4]);
        let mut packs: Vec<Pack> = Vec::new();
        let mut phone_had_two = true;
        for _ in 0..4 {
            for (side, had_two) in [
                (&mut phone, &mut phone_had_two),
                (&mut computer, &mut false),
            ] {
                let there = drive(&packs);
                let plan = side.plan(&there);
                // The phone sends 2's deletion only once the drive holds 2.
                let sent_deleted: Vec<u64> = plan.send_deleted.iter().map(|id| id.nonce).collect();
                for take in plan.take.iter().chain(&plan.take_back) {
                    side.listed.insert(*take);
                }
                for (gone, at) in &plan.let_go {
                    side.listed.remove(gone);
                    side.forgotten.insert(*gone);
                    side.applied.insert(*gone, *at);
                }
                let mut sent = pack(
                    &plan.send.iter().map(|id| id.nonce).collect::<Vec<_>>(),
                    &sent_deleted.iter().map(|&n| (n, 100)).collect::<Vec<_>>(),
                    &[],
                );
                // The phone's 2, still in an old pack of its own.
                if *had_two {
                    sent.put.push(Put {
                        id: id(2).name(),
                        s: "{}".into(),
                    });
                    *had_two = false;
                }
                packs.push(sent);
            }
        }
        assert_eq!(phone.listed, ids(&[1, 3, 4]));
        assert_eq!(computer.listed, phone.listed);
        let there = drive(&packs);
        assert!(there.deleted(id(2)));
        assert!(phone.plan(&there).is_empty() && computer.plan(&there).is_empty());
    }
}
