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
//! that held it syncs, which drops it again. What no merge can do, for a
//! sync that plans from a note another copy made or dropped since it read
//! the state, is said at [`SyncState::merged`].

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
    /// Of another sync in the store than this copy's, the store's is taken
    /// as it is where this copy's sync is the one it last read or kept
    /// there, as when another copy turned sync on anew: this copy's notes
    /// are of a sync that is no more. Where the store never took this copy's
    /// sync, as one just turned on here, this copy's is kept whole, as the
    /// one last chosen here.
    ///
    /// What it cannot do: a sync plans from what sync keeps as it read it as
    /// it began, and no merge at its end changes what it planned. One that
    /// runs beside another copy's sync, or began from a late read, plans
    /// without a deletion's note the other copy made since, or with one it
    /// dropped as it took the session back: it can send the deletion of a
    /// session the other let go here, deleting everywhere a putting back
    /// made elsewhere, or take back one deleted here by hand since. In this
    /// module's model of three copies, a deletion went out again in about
    /// three runs in five where syncs ran at once, and one in thirty-six
    /// where tabs read late; in none where they synced one at a time from
    /// what the store held. Only that keeps both out.
    pub fn merged(&self, base: Option<&SyncState>, theirs: &SyncState) -> SyncState {
        // What this copy last read or kept of a sync before one turned on
        // here says nothing of this one.
        let base = base.filter(|base| base.of_one_sync(self));
        if !self.of_one_sync(theirs) {
            return if base.is_some() {
                theirs.clone()
            } else {
                self.clone()
            };
        }
        let none = SyncState::default();
        let base = base.unwrap_or(&none);
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
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use super::SyncState;
    use crate::persist::record::RecordId;
    use crate::sync::plan::{DriveState, Here, plan};

    const COPIES: usize = 3;
    /// Sessions put back here by hand.
    const SESSIONS: u64 = 3;
    /// Sessions the other devices delete and put back in the drive, which
    /// the copies' syncs let go here and take back.
    const ELSEWHERE: u64 = 2;

    fn id(n: u64) -> RecordId {
        RecordId {
            order: n * 1000,
            nonce: n,
        }
    }

    /// The `k`th of the sessions changed [`ELSEWHERE`].
    fn elsewhere(k: u64) -> RecordId {
        id(SESSIONS + k)
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

    /// One of the sessions changed elsewhere, as the store here holds it.
    #[derive(Clone, Copy)]
    struct Stored {
        /// Here, not deleted.
        listed: bool,
        /// Deleted here by hand, a deletion the drive does not hold yet.
        by_hand: bool,
    }

    /// A copy's sync under way.
    #[derive(Clone)]
    struct UnderWay {
        /// What it began from: what the copy held, laid over what it read.
        began: SyncState,
        /// What it holds as it goes: the drive as it read it, and what it
        /// did as it listed the store.
        synced: SyncState,
        /// It has listed the store, and changed it as it planned.
        listed: bool,
        /// The puttings back noted meanwhile.
        noted: BTreeMap<RecordId, u64>,
    }

    /// Copies of the app on one store, each with what it last read or kept
    /// (`base`), what it holds (`ours`) and its sync under way; and the
    /// drive, where other devices delete and put back the sessions changed
    /// elsewhere.
    struct Copies {
        draw: Draw,
        /// Every state the store held, in order.
        writes: Vec<SyncState>,
        /// The latest write each copy has read or made.
        seen: [usize; COPIES],
        base: Vec<SyncState>,
        ours: Vec<SyncState>,
        syncs: Vec<Option<UnderWay>>,
        /// Whether each session put back by hand is here, or deleted again
        /// by hand.
        listed: [bool; SESSIONS as usize],
        /// Whether each putting back noted is one the drive is to hear of,
        /// or one with nothing in the drive to put back over.
        wanted: HashMap<(RecordId, u64), bool>,
        /// The latest putting back of each session, any copy's.
        latest: HashMap<RecordId, u64>,
        /// The drive's word on the sessions changed elsewhere.
        drive: DriveState,
        /// The sessions changed elsewhere, as the store here holds them.
        stored: [Stored; ELSEWHERE as usize],
        /// A sync sent the deletion of a session changed elsewhere that went
        /// here only as the drive deleted it: a putting back made elsewhere
        /// since, deleted everywhere.
        resent: bool,
        /// A sync took back a session changed elsewhere that was deleted
        /// here by hand since it was last here: the deletion undone.
        undone: bool,
        clock: u64,
        /// Tabs may read what sync keeps from before another's last write.
        late: bool,
    }

    impl Copies {
        fn new(seed: u64, late: bool) -> Self {
            let state = SyncState::new("device".into(), "00".repeat(32), "file1".into(), None);
            let mut drive = DriveState::default();
            drive.puts.extend((0..ELSEWHERE).map(elsewhere));
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
                drive,
                stored: [Stored {
                    listed: true,
                    by_hand: false,
                }; ELSEWHERE as usize],
                resent: false,
                undone: false,
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
                Some(sync) => {
                    sync.noted.insert(session, at);
                }
                None => {
                    self.ours[c].revived.insert(session, at);
                    self.keep(c);
                }
            }
        }

        /// Copy `c`'s sync begins: it reads what sync keeps, and the drive.
        fn begin(&mut self, c: usize) {
            let theirs = self.read(c);
            self.ours[c] = self.ours[c].merged(Some(&self.base[c]), &theirs);
            self.base[c] = theirs;
            let began = self.ours[c].clone();
            let mut synced = began.clone();
            synced.drive.join(&self.drive);
            self.syncs[c] = Some(UnderWay {
                began,
                synced,
                listed: false,
                noted: BTreeMap::new(),
            });
        }

        /// Copy `c`'s sync lists the store, once, and changes it.
        fn list(&mut self, c: usize) {
            if let Some(mut sync) = self.syncs[c].take() {
                if !sync.listed {
                    self.change(&mut sync);
                }
                self.syncs[c] = Some(sync);
            }
        }

        /// A sync lists the store and changes it as it plans ([`plan`]),
        /// as sync_once does, for the sessions changed elsewhere: it lets go
        /// one the drive deleted, noting the deletion; takes back one gone
        /// here that the drive put back since such a note; and sends the
        /// deletion of one gone here with none, as deleted by hand. One
        /// gone here that the drive holds as deleted is the drive's word.
        fn change(&mut self, sync: &mut UnderWay) {
            sync.listed = true;
            let (began, state) = (&sync.began, &mut sync.synced);
            let listed: BTreeSet<RecordId> = (0..ELSEWHERE)
                .filter(|&k| self.stored[k as usize].listed)
                .map(elsewhere)
                .collect();
            state.applied.retain(|id, _| !listed.contains(id));
            // Of those not here, those the sync asks the store about, as
            // sync_once does: those the drive holds alive, and those it
            // deleted since this copy last read it, with no note. Each went
            // here as deleted, by hand or let go, so the store says each was.
            let forgotten: BTreeSet<RecordId> = (0..ELSEWHERE)
                .map(elsewhere)
                .filter(|&x| {
                    !listed.contains(&x)
                        && (state.drive.alive(x)
                            || state.drive.deleted(x)
                                && !began.drive.deleted(x)
                                && !state.applied.contains_key(&x))
                })
                .collect();
            let planned = plan(
                &state.drive,
                Here {
                    listed: &listed,
                    forgotten: &forgotten,
                    applied: &state.applied,
                    revived: &state.revived,
                },
            );
            let slot = |x: RecordId| (x.nonce - SESSIONS) as usize;
            for &(x, tomb) in planned.let_go.iter().chain(&planned.deleted_too) {
                self.stored[slot(x)] = Stored {
                    listed: false,
                    by_hand: false,
                };
                state.applied.insert(x, tomb);
            }
            for &x in &planned.take_back {
                self.undone |= self.stored[slot(x)].by_hand;
                self.stored[slot(x)] = Stored {
                    listed: true,
                    by_hand: false,
                };
                state.applied.remove(&x);
            }
            for &x in &planned.send_deleted {
                self.resent |= !self.stored[slot(x)].by_hand;
                self.stored[slot(x)].by_hand = false;
                self.clock += 1;
                let at = state.stamp_change(x, self.clock);
                for drive in [&mut self.drive, &mut state.drive] {
                    let tomb = drive.tombs.entry(x).or_insert(at);
                    *tomb = (*tomb).max(at);
                }
                state.applied.insert(x, at);
            }
            for x in (0..ELSEWHERE).map(elsewhere) {
                if !self.stored[slot(x)].listed && state.drive.deleted(x) {
                    self.stored[slot(x)].by_hand = false;
                }
            }
        }

        /// Copy `c`'s sync ends, having listed the store: each putting back
        /// it began with is sent, or dropped as no more to note, its session
        /// deleted again (the deletion noted in applied, later than it, or
        /// as the drive's own, earlier) or with nothing in the drive to put
        /// back over.
        fn end(&mut self, c: usize) {
            let Some(mut sync) = self.syncs[c].take() else {
                return;
            };
            if !sync.listed {
                self.change(&mut sync);
            }
            let UnderWay {
                began,
                mut synced,
                noted,
                ..
            } = sync;
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

        /// A deletion, or a putting back, of one of the sessions changed
        /// elsewhere, made on another device and sent to the drive.
        fn changed_elsewhere(&mut self, put_back: bool) {
            let x = elsewhere(self.draw.below(ELSEWHERE));
            let at = self.tick();
            let times = if put_back {
                &mut self.drive.revives
            } else {
                &mut self.drive.tombs
            };
            let time = times.entry(x).or_insert(at);
            *time = (*time).max(at);
        }

        fn run(&mut self, steps: usize, overlapping: bool) {
            for _ in 0..steps {
                let c = self.draw.below(COPIES as u64) as usize;
                match self.draw.below(100) {
                    0..30 => {
                        let n = self.draw.below(SESSIONS);
                        self.put_back(c, n);
                    }
                    30..38 => {
                        let n = self.draw.below(SESSIONS);
                        self.listed[n as usize] = false;
                    }
                    38..44 => self.changed_elsewhere(false),
                    44..50 => self.changed_elsewhere(true),
                    50..56 => {
                        let k = self.draw.below(ELSEWHERE) as usize;
                        if self.stored[k].listed {
                            self.stored[k] = Stored {
                                listed: false,
                                by_hand: true,
                            };
                        }
                    }
                    56..64 => self.keep(c),
                    // Closed and opened again: what it held is what the
                    // store holds. Where tabs read each other late, one
                    // closed just after another left its note out loses
                    // it, as no merge can help.
                    64..68 if !self.late && self.syncs[c].is_none() => {
                        let opened = self.read(c);
                        self.base[c] = opened.clone();
                        self.ours[c] = opened;
                    }
                    68..84
                        if self.syncs[c].is_none()
                            && (overlapping || self.syncs.iter().all(Option::is_none)) =>
                    {
                        self.begin(c)
                    }
                    84..92 => self.list(c),
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

    /// Three copies of the app on one store, as above, while other devices
    /// delete sessions in the drive and put them back, and each copy's sync
    /// lists the store between reading what sync keeps and keeping its own,
    /// letting a session go as the drive deleted it, taking one back as the
    /// drive put it back since, and sending the deletion of one deleted here
    /// by hand: syncing one at a time, from what the store holds, no sync
    /// sends the deletion of a session that went here only as the drive
    /// deleted it, which would delete everywhere a putting back made
    /// elsewhere, and none takes back one deleted here by hand since it was
    /// last here. R1's union, laying back a note a take-back dropped, undoes
    /// a deletion by hand; a merge that drops every deletion's note the
    /// drive shows put back since, though deleted again after, sends one
    /// again.
    ///
    /// Where syncs overlap, or a tab reads the store late, no merge keeps
    /// either out: a sync plans from the notes it read, and one another copy
    /// made since, or wrote over, is not among them
    /// ([`SyncState::merged`]).
    #[test]
    fn no_deletion_of_a_session_let_go_here_is_sent_by_copies_syncing_one_at_a_time() {
        for seed in 0..2000 {
            let mut copies = Copies::new(seed, false);
            copies.run(70, false);
            assert!(
                !copies.resent,
                "seed {seed}: the deletion of a session let go here was sent"
            );
            assert!(
                !copies.undone,
                "seed {seed}: a deletion by hand here was undone"
            );
        }
    }
}
