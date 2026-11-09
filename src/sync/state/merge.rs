//! What two copies of the app on one device keep of one sync, merged
//! ([`SyncState::merged`]): a browser's tabs, or a computer's windows, keep
//! one state in the store between them, and each lays what it did over what
//! the other kept, rather than writing its own over it.
//!
//! Each note is merged by what each copy did to it since this copy last
//! read or kept the state. What this copy dropped is gone: its sync sent
//! it, or found it was no more to note, and knew it. What the other copy
//! dropped goes only on evidence, in what the copies read of the drive,
//! that it was sent or spent: a browser's tab can read what sync keeps from
//! before another tab's last write, and write without what that one noted,
//! and nothing tells such a write from one that dropped a note on purpose.
//! So no note still wanted is lost to a tab that read late, and none dropped
//! on purpose stays once each copy has synced; one dropped as no more to
//! note, with nothing to show it, can come back for a while, until the copy
//! that held it syncs, which drops it again.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use super::SyncState;
use crate::persist::record::RecordId;

impl SyncState {
    /// What sync keeps, as this copy holds it, laid over `theirs`, as the
    /// store holds it now, once another copy of the app on the same store
    /// kept its own since this one last read or kept it there, as `base`:
    /// none where the store never took this copy's state.
    ///
    /// - What the copies read of the drive is put together: the packs either
    ///   read, and what they say ([`crate::sync::plan::DriveState::join`]);
    ///   a pack a later build wrote stays noted as such until either reads
    ///   it. The last sync is the later of the two.
    /// - The sessions put back here by hand ([`SyncState::revived`]), the
    ///   deletions the drive holds ([`SyncState::applied`]) and the packs
    ///   whose answer never came ([`SyncState::unanswered`]) go note by
    ///   note: one either noted since stays, at the later stamp where both
    ///   did; one this copy dropped since is gone, unless the other noted it
    ///   again; one the other copy dropped goes only on evidence. A putting
    ///   back goes once the drive shows it sent, at its stamp or later, or a
    ///   deletion of its session after it is noted; a deletion, once the
    ///   drive has put its session back since, as late or later, and holds
    ///   it so, as a sync took it back here then; a pack, once each deletion
    ///   in it is noted, or put back since. A note at an older stamp than
    ///   this copy last saw is one written without having read it, as a
    ///   stamp only ever rises. A putting back sent or spent goes whoever
    ///   holds it. A deletion goes on that evidence only where the other
    ///   copy left it out: two copies syncing at once can read the drive
    ///   between them as putting its session back after one let it go,
    ///   though no sync here took it back, and without the note the next
    ///   would take the session for one deleted by hand and send its
    ///   deletion.
    /// - The settings as of the last sync, with when each was changed, go
    ///   together, as of the later sync; of one sync, this copy's where it
    ///   changed them. The sign-in is this copy's where it signed in again
    ///   since, and otherwise the store's. The deletions an earlier build
    ///   made are noted where either copy noted them.
    /// - What a later build keeps here, which this one does not know, is the
    ///   store's: this copy never changes it, so a key the store no longer
    ///   holds was let go, and stays gone.
    ///
    /// Of another sync in the store than this copy's, as one turned on anew
    /// in another copy, the store's is taken as it is: this copy's notes are
    /// of a sync that is no more.
    pub fn merged(&self, base: Option<&SyncState>, theirs: &SyncState) -> SyncState {
        if !self.of_one_sync(theirs) {
            return theirs.clone();
        }
        let none = SyncState::default();
        let base = base
            .filter(|base| base.of_one_sync(theirs))
            .unwrap_or(&none);
        let mut drive = theirs.drive.clone();
        drive.join(&self.drive);
        let read: BTreeSet<String> = self.read.union(&theirs.read).cloned().collect();
        let mut later = theirs.later.clone();
        for (name, &layout) in &self.later {
            let noted = later.entry(name.clone()).or_insert(layout);
            *noted = (*noted).max(layout);
        }
        later.retain(|name, _| !read.contains(name));
        let put_back_since =
            |id: &RecordId, at: u64| drive.revives.get(id).is_some_and(|&back| back >= at);
        let applied = merge_notes(
            &base.applied,
            &self.applied,
            &theirs.applied,
            false,
            |id, at| put_back_since(&id, at) && !drive.deleted(id),
        );
        let revived = merge_notes(
            &base.revived,
            &self.revived,
            &theirs.revived,
            true,
            |id, at| put_back_since(&id, at) || applied.get(&id).is_some_and(|&gone| gone > at),
        );
        let unanswered = merge_unanswered(
            &base.unanswered,
            &self.unanswered,
            &theirs.unanswered,
            |deletions| {
                deletions.iter().all(|(id, &at)| {
                    applied.get(id).is_some_and(|&noted| noted >= at) || put_back_since(id, at)
                })
            },
        );
        let synced_later = match self.last.cmp(&theirs.last) {
            Ordering::Greater => self,
            Ordering::Less => theirs,
            Ordering::Equal if (&self.settings, &self.stamps) != (&base.settings, &base.stamps) => {
                self
            }
            Ordering::Equal => theirs,
        };
        let sign_in = if self.sign_in != base.sign_in {
            &self.sign_in
        } else {
            &theirs.sign_in
        };
        let unread = theirs.unread.clone();
        SyncState {
            dust_sync_state: theirs.dust_sync_state,
            device: theirs.device.clone(),
            key: theirs.key.clone(),
            keyring: theirs.keyring.clone(),
            sign_in: sign_in.clone(),
            last: self.last.max(theirs.last),
            read,
            later,
            drive,
            applied,
            unanswered,
            revived,
            deletions_noted: self.deletions_noted || theirs.deletions_noted,
            settings: synced_later.settings.clone(),
            stamps: synced_later.stamps.clone(),
            unread,
        }
    }

    /// Whether `other` is a state of the same sync as this: of the same
    /// device, joined to the same keyring, as the app tells a sync turned
    /// on anew in another copy.
    fn of_one_sync(&self, other: &SyncState) -> bool {
        self.device == other.device && self.keyring == other.keyring
    }
}

/// One kind of stamped note, by record, of two copies' states merged: as
/// this copy last read or kept it, `base`; as it holds it now, `ours`; and
/// as the store holds it now, `theirs` ([`SyncState::merged`]). `spent` says
/// a note, at its stamp, was sent or spent, as the copies' drive shows it:
/// one the other copy left out goes on that alone, and, where `drop_spent`,
/// one either holds as well.
fn merge_notes(
    base: &BTreeMap<RecordId, u64>,
    ours: &BTreeMap<RecordId, u64>,
    theirs: &BTreeMap<RecordId, u64>,
    drop_spent: bool,
    spent: impl Fn(RecordId, u64) -> bool,
) -> BTreeMap<RecordId, u64> {
    let ids: BTreeSet<RecordId> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .copied()
        .collect();
    ids.into_iter()
        .filter_map(|id| {
            let at = merge_note(
                base.get(&id).copied(),
                ours.get(&id).copied(),
                theirs.get(&id).copied(),
                |at| spent(id, at),
            )?;
            (!(drop_spent && spent(id, at))).then_some((id, at))
        })
        .collect()
}

/// One note, at its stamp, of two copies' states merged ([`merge_notes`]).
fn merge_note(
    base: Option<u64>,
    ours: Option<u64>,
    theirs: Option<u64>,
    spent: impl Fn(u64) -> bool,
) -> Option<u64> {
    // An older stamp than this copy last saw is one written without having
    // read what this copy saw: the note as this copy saw it.
    let theirs = match (base, theirs) {
        (Some(seen), Some(at)) if at < seen => Some(seen),
        _ => theirs,
    };
    if ours == base {
        // Left as it was here: as the store holds it, but where the store
        // left it out with nothing to say it was sent or spent.
        match (base, theirs) {
            (Some(seen), None) if !spent(seen) => Some(seen),
            _ => theirs,
        }
    } else if theirs == base {
        // Changed here alone, or dropped: as this copy holds it.
        ours
    } else {
        // Changed by both: the later, where either holds it.
        ours.max(theirs)
    }
}

/// The packs whose answer never came, by name, of two copies' states
/// merged, as [`merge_notes`] merges notes: one this copy dropped is gone,
/// its sync having found it in the drive or found it never made; one the
/// store left out stays, until each deletion in it is noted, or put back
/// since (`accounted`).
fn merge_unanswered(
    base: &BTreeMap<String, BTreeMap<RecordId, u64>>,
    ours: &BTreeMap<String, BTreeMap<RecordId, u64>>,
    theirs: &BTreeMap<String, BTreeMap<RecordId, u64>>,
    accounted: impl Fn(&BTreeMap<RecordId, u64>) -> bool,
) -> BTreeMap<String, BTreeMap<RecordId, u64>> {
    let names: BTreeSet<&String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    names
        .into_iter()
        .filter_map(|name| {
            let held = match (base.get(name), ours.get(name), theirs.get(name)) {
                (Some(_), None, _) | (None, None, None) => return None,
                (Some(_), Some(ours), None) if accounted(ours) => return None,
                (_, Some(one), None) | (None, None, Some(one)) => one.clone(),
                (_, Some(ours), Some(theirs)) => {
                    let mut both = theirs.clone();
                    for (&id, &at) in ours {
                        let held = both.entry(id).or_insert(at);
                        *held = (*held).max(at);
                    }
                    both
                }
            };
            Some((name.clone(), held))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::SyncState;
    use crate::persist::record::RecordId;

    const COPIES: usize = 3;
    const SESSIONS: u64 = 3;

    fn id(n: u64) -> RecordId {
        RecordId {
            order: n * 1000,
            nonce: n,
        }
    }

    /// A seeded draw, the same for a seed every run.
    struct Draw(u64);

    impl Draw {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        fn chance(&mut self, percent: u64) -> bool {
            self.below(100) < percent
        }
    }

    /// Copies of the app on one store, each with what it last read or kept
    /// (`base`), what it holds (`ours`) and its sync under way: what it
    /// began from, and the puttings back noted meanwhile.
    struct Copies {
        draw: Draw,
        /// Every state the store held, in order.
        writes: Vec<SyncState>,
        /// The latest write each copy has read or made.
        seen: [usize; COPIES],
        base: Vec<SyncState>,
        ours: Vec<SyncState>,
        syncs: Vec<Option<(SyncState, BTreeMap<RecordId, u64>)>>,
        /// Whether each session is here, or deleted again by hand.
        listed: [bool; SESSIONS as usize],
        /// Whether each putting back noted is one the drive is to hear of,
        /// or one with nothing in the drive to put back over.
        wanted: HashMap<(RecordId, u64), bool>,
        /// The latest putting back of each session, any copy's.
        latest: HashMap<RecordId, u64>,
        clock: u64,
        /// Tabs may read what sync keeps from before another's last write.
        late: bool,
    }

    impl Copies {
        fn new(seed: u64, late: bool) -> Self {
            let state = SyncState::new("device".into(), "00".repeat(32), "file1".into(), None);
            Self {
                draw: Draw(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
                writes: vec![state.clone()],
                seen: [0; COPIES],
                base: vec![state.clone(); COPIES],
                ours: vec![state; COPIES],
                syncs: vec![None; COPIES],
                listed: [true; SESSIONS as usize],
                wanted: HashMap::new(),
                latest: HashMap::new(),
                clock: 0,
                late,
            }
        }

        fn tick(&mut self) -> u64 {
            self.clock += 1;
            self.clock
        }

        /// What copy `c` reads of the store: the latest write, or, late, one
        /// of the two before it, never older than one it read or made.
        fn read(&mut self, c: usize) -> SyncState {
            let newest = self.writes.len() - 1;
            let mut at = newest;
            if self.late && self.draw.chance(30) {
                let oldest = self.seen[c].max(newest.saturating_sub(2));
                at = oldest + self.draw.below((newest - oldest + 1) as u64) as usize;
            }
            self.seen[c] = self.seen[c].max(at);
            self.writes[at].clone()
        }

        fn write(&mut self, c: usize, state: SyncState) {
            self.writes.push(state.clone());
            self.seen[c] = self.writes.len() - 1;
            self.base[c] = state.clone();
            self.ours[c] = state;
        }

        /// Copy `c` keeps what it holds, laid over what it reads.
        fn keep(&mut self, c: usize) {
            let theirs = self.read(c);
            let merged = self.ours[c].merged(Some(&self.base[c]), &theirs);
            self.write(c, merged);
        }

        /// Session `n` put back by hand in copy `c`.
        fn put_back(&mut self, c: usize, n: u64) {
            let (session, at) = (id(n), self.tick());
            let wanted = self.draw.chance(60);
            self.wanted.insert((session, at), wanted);
            self.latest.insert(session, at);
            self.listed[n as usize] = true;
            match &mut self.syncs[c] {
                Some((_, noted)) => {
                    noted.insert(session, at);
                }
                None => {
                    self.ours[c].revived.insert(session, at);
                    self.keep(c);
                }
            }
        }

        fn begin(&mut self, c: usize) {
            let theirs = self.read(c);
            self.ours[c] = self.ours[c].merged(Some(&self.base[c]), &theirs);
            self.base[c] = theirs;
            self.syncs[c] = Some((self.ours[c].clone(), BTreeMap::new()));
        }

        /// Copy `c`'s sync ends: each putting back it began with is sent, or
        /// dropped as no more to note, its session deleted again (the
        /// deletion noted in applied, later than it, or as the drive's own,
        /// earlier) or with nothing in the drive to put back over.
        fn end(&mut self, c: usize) {
            let Some((began, noted)) = self.syncs[c].take() else {
                return;
            };
            let mut synced = began.clone();
            for (&session, &at) in &began.revived {
                let here = self.listed[(session.nonce) as usize];
                if here && self.wanted[&(session, at)] {
                    let back = synced.drive.revives.entry(session).or_insert(at);
                    *back = (*back).max(at);
                } else if !here {
                    let gone = if self.draw.chance(50) {
                        self.tick()
                    } else {
                        at - 1
                    };
                    synced.drive.tombs.insert(session, gone);
                    synced.applied.insert(session, gone);
                }
                synced.revived.remove(&session);
            }
            let theirs = self.read(c);
            let mut merged = synced.merged(Some(&began), &theirs);
            for (session, at) in noted {
                let held = merged.revived.entry(session).or_insert(at);
                *held = (*held).max(at);
            }
            self.write(c, merged);
        }

        fn run(&mut self, steps: usize, overlapping: bool) {
            for _ in 0..steps {
                let c = self.draw.below(COPIES as u64) as usize;
                match self.draw.below(100) {
                    0..40 => {
                        let n = self.draw.below(SESSIONS);
                        self.put_back(c, n);
                    }
                    40..50 => {
                        let n = self.draw.below(SESSIONS);
                        self.listed[n as usize] = false;
                    }
                    50..60 => self.keep(c),
                    // Closed and opened again: what it held is what the
                    // store holds. Where tabs read each other late, one
                    // closed just after another left its note out loses
                    // it, as no merge can help.
                    60..65 if !self.late && self.syncs[c].is_none() => {
                        let opened = self.read(c);
                        self.base[c] = opened.clone();
                        self.ours[c] = opened;
                    }
                    65..85
                        if self.syncs[c].is_none()
                            && (overlapping || self.syncs.iter().all(Option::is_none)) =>
                    {
                        self.begin(c)
                    }
                    _ => self.end(c),
                }
            }
            for c in 0..COPIES {
                self.end(c);
            }
            // A quiet end: every copy keeps, then syncs, reading each other
            // as they are.
            self.late = false;
            for c in 0..COPIES {
                self.keep(c);
            }
            for c in 0..COPIES {
                self.begin(c);
                self.end(c);
            }
        }
    }

    /// Three copies of the app on one store, each keeping what sync keeps as
    /// it notes a putting back and as each of its syncs ends, in every order
    /// a seed draws, syncing one at a time or at once, closing and opening
    /// again, and reading the store as it is or, as a browser's tabs can,
    /// from before another's last write: no putting back still wanted is
    /// lost, and once every copy has synced, the store holds none of those
    /// sent or dropped. The design's three-way merge loses one where tabs
    /// read late, a union that drops a note only once the drive shows it
    /// sent leaves those dropped as no more to note for good, and a copy
    /// writing its own whole loses one.
    #[test]
    fn no_putting_back_still_wanted_is_lost_whatever_order_copies_keep_in() {
        for late in [false, true] {
            for overlapping in [false, true] {
                for seed in 0..500 {
                    let mut copies = Copies::new(seed, late);
                    copies.run(70, overlapping);
                    let kept = copies.writes.last().expect("a write");
                    for (&session, &at) in &copies.latest {
                        let wanted = copies.listed[(session.nonce) as usize]
                            && copies.wanted[&(session, at)];
                        let sent = kept
                            .drive
                            .revives
                            .get(&session)
                            .is_some_and(|&back| back >= at);
                        assert!(
                            !wanted || sent,
                            "seed {seed} (late {late}, overlapping {overlapping}): \
                             the putting back of {session:?} at {at} was lost"
                        );
                    }
                    assert!(
                        kept.revived.is_empty(),
                        "seed {seed} (late {late}, overlapping {overlapping}): \
                         left {:?}",
                        kept.revived
                    );
                }
            }
        }
    }
}
