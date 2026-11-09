//! Devices syncing through one folder: each a store of its own and a state
//! of its own, as each install has, the folder shared.

use std::cell::RefCell;
use std::rc::Rc;

use cw_core::{SessionResult, TrainingSettings};

use super::*;
use crate::persist::Store;
use crate::persist::memory::MemoryStore;
use crate::persist::record::{HistoryChange, Pending, Record, RecordId, done};
use crate::persist::tests::at;
use crate::sync::remote::MemoryRemote;
use crate::sync::seal::Stretch;
use crate::sync::state::SyncState;

/// What `pending` comes to, which a memory store and folder settle at once.
fn now<T>(pending: impl Future<Output = T>) -> T {
    use std::task::{Context, Poll, Waker};
    let mut pending = std::pin::pin!(pending);
    match pending
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("a memory store answers at once"),
    }
}

/// A device: its store, what sync keeps on it, and its settings.
struct Device {
    store: Rc<MemoryStore>,
    state: SyncState,
    settings: TrainingSettings,
}

impl Device {
    /// A device that keeps `sessions`, sync not yet on.
    fn keeping(sessions: &[SessionResult]) -> Self {
        let store = Rc::new(MemoryStore::default());
        keep(&store, sessions);
        Self {
            store,
            state: SyncState::default(),
            settings: TrainingSettings::default(),
        }
    }

    /// Turn sync on with `passphrase`, as the card does.
    fn turn_on(&mut self, remote: &MemoryRemote, passphrase: &str) -> Result<(), JoinError> {
        let found = now(look(remote))?;
        let (key, keyring) = now(join(remote, passphrase, &found, Stretch::QUICK))?;
        let device = crate::sync::text::random_hex::<16>().expect("random");
        self.state = SyncState::new(device, key.to_hex(), keyring, None);
        Ok(())
    }

    fn sync(&mut self, remote: &MemoryRemote, at: u64) -> Result<Synced, SyncFailed> {
        let synced = now(sync_once(
            &*self.store,
            remote,
            &mut self.state,
            &self.settings,
            at,
        ))?;
        if let Some(settings) = &synced.settings {
            self.settings = settings.clone();
        }
        Ok(synced)
    }

    /// The stamps of the sessions it holds, in order.
    fn stamps(&self) -> Vec<u64> {
        self.store
            .load_history()
            .sessions
            .iter()
            .map(|(_, session)| session.timestamp)
            .collect()
    }

    fn id_of(&self, timestamp: u64) -> RecordId {
        self.store
            .load_history()
            .sessions
            .iter()
            .find(|(_, session)| session.timestamp == timestamp)
            .map(|(id, _)| *id)
            .expect("held")
    }

    /// The session the record `id` holds.
    fn session(&self, id: RecordId) -> SessionResult {
        self.store
            .load_history()
            .sessions
            .into_iter()
            .find(|(held, _)| *held == id)
            .expect("held")
            .1
    }

    /// Delete the session finished at `timestamp` by hand.
    fn delete(&self, timestamp: u64, when: u64) {
        let id = self.id_of(timestamp);
        let session = self
            .store
            .load_history()
            .sessions
            .into_iter()
            .find(|(held, _)| *held == id)
            .expect("held")
            .1;
        let saved = now(self.store.change_history(HistoryChange {
            forget: vec![(id, Deleted::of(&session, when))],
            ..HistoryChange::default()
        }));
        assert!(saved.kept().is_some());
    }

    /// Put back by hand what was deleted, as Undo does, and note it as
    /// the app does for sync.
    fn undo(&mut self, id: RecordId, session: &SessionResult, when: u64) {
        let json = serde_json::to_string(session).expect("JSON");
        let saved = now(self.store.change_history(HistoryChange {
            put: vec![Record { id, json }],
            unforget: vec![id],
            ..HistoryChange::default()
        }));
        assert!(saved.kept().is_some());
        let at = self.state.stamp_change(id, when);
        self.state.revived.insert(id, at);
    }
}

/// Store `sessions` in `store`, each after the one before.
fn keep(store: &MemoryStore, sessions: &[SessionResult]) {
    let mut newest = store.load_history().sessions.last().map(|(id, _)| *id);
    let put: Vec<Record> = sessions
        .iter()
        .map(|session| {
            let record = Record::of(session, |json| {
                RecordId::next(session.timestamp, newest, json)
            })
            .expect("a record");
            newest = Some(record.id);
            record
        })
        .collect();
    let saved = now(store.change_history(HistoryChange {
        put,
        ..HistoryChange::default()
    }));
    assert!(saved.kept().is_some(), "{saved:?}");
}

fn sessions(stamps: &[u64]) -> Vec<SessionResult> {
    stamps.iter().map(|&stamp| at(stamp)).collect()
}

/// The first device to turn sync on sends its whole history, sealed: the
/// drive holds nothing of it in the clear.
#[test]
fn the_first_device_sends_its_history_sealed() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2, 3]));
    phone.turn_on(&remote, "passphrase").expect("on");
    let synced = phone.sync(&remote, 100).expect("synced");
    assert_eq!(synced.sent, 3);
    assert_eq!((synced.took, synced.let_go, synced.unreadable), (0, 0, 0));
    assert_eq!(synced.settings, None, "its own settings, untouched");
    assert_eq!(phone.state.last, Some(100));
    let names = remote.names();
    assert_eq!(names[0], "dust-keyring");
    assert!(names.iter().any(|name| name.starts_with("dust-p-")));
    assert!(names.iter().any(|name| name.starts_with("dust-s-")));
    let everything = remote.every_byte();
    for clear in [&b"2026-10-0"[..], b"timestamp", b"numGroups", b"charWpm"] {
        assert!(
            !everything.windows(clear.len()).any(|w| w == clear),
            "{} in the clear",
            String::from_utf8_lossy(clear)
        );
    }
    // Synced again, nothing new is sent.
    let writes = remote.writes.get();
    let again = phone.sync(&remote, 200).expect("synced");
    assert_eq!(again, Synced::default());
    assert_eq!(remote.writes.get(), writes);
}

/// Another device joins with the passphrase, and only with it: it takes
/// the drive's sessions among its own, and the drive's settings, every one;
/// and the first takes its sessions next.
#[test]
fn another_device_joins_with_the_passphrase() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 3]));
    phone.settings.curriculum.num_groups = 9;
    phone.settings.band.master_volume = 0.2;
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");

    let mut computer = Device::keeping(&sessions(&[2]));
    computer.settings.band.master_volume = 0.8;
    assert_eq!(
        computer.turn_on(&remote, "Passphrase"),
        Err(JoinError::Wrong)
    );
    computer.turn_on(&remote, "passphrase").expect("on");
    let synced = computer.sync(&remote, 200).expect("synced");
    assert_eq!((synced.took, synced.sent), (2, 1));
    assert_eq!(computer.stamps(), [1, 2, 3]);
    assert_eq!(computer.settings.curriculum.num_groups, 9, "the drive's");
    assert_eq!(
        computer.settings.band.master_volume, 0.2,
        "every setting, volume too"
    );

    let synced = phone.sync(&remote, 300).expect("synced");
    assert_eq!(synced.took, 1);
    assert_eq!(phone.stamps(), [1, 2, 3]);
    // Both hold the very same records.
    assert_eq!(listing(&phone.store), listing(&computer.store));
}

/// A session deleted on one device is deleted on the other; and one
/// deleted on the other before it ever synced is deleted on the first.
#[test]
fn a_deletion_reaches_every_device() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2, 3]));
    phone.turn_on(&remote, "pass phrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "pass phrase").expect("on");
    computer.sync(&remote, 110).expect("synced");
    assert_eq!(computer.stamps(), [1, 2, 3]);

    let two = phone.id_of(2);
    phone.delete(2, 120);
    assert_eq!(phone.sync(&remote, 130).expect("synced").sent, 1);
    let synced = computer.sync(&remote, 140).expect("synced");
    assert_eq!(synced.let_go, 1);
    assert_eq!(computer.stamps(), [1, 3]);
    assert!(computer.store.tombstone(two).is_some(), "the same record");

    computer.delete(3, 150);
    computer.sync(&remote, 160).expect("synced");
    phone.sync(&remote, 170).expect("synced");
    assert_eq!(phone.stamps(), [1]);
    // Neither has anything left to do, nor brings either back.
    assert_eq!(phone.sync(&remote, 180).expect("synced"), Synced::default());
    assert_eq!(
        computer.sync(&remote, 190).expect("synced"),
        Synced::default()
    );
    assert_eq!(computer.stamps(), [1]);
}

/// Every record `store` lists.
fn listing(store: &MemoryStore) -> Vec<RecordId> {
    now(store.history_listing()).expect("listed")
}

/// A session put back by Undo after its deletion was synced comes back on
/// every device: a deletion the drive holds is not the last word.
#[test]
fn a_session_put_back_after_its_deletion_comes_back_everywhere() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");

    let id = phone.id_of(2);
    let session = phone
        .store
        .load_history()
        .sessions
        .into_iter()
        .find(|(held, _)| *held == id)
        .expect("held")
        .1;
    phone.delete(2, 120);
    phone.sync(&remote, 130).expect("synced");
    computer.sync(&remote, 140).expect("synced");
    assert_eq!(computer.stamps(), [1]);

    phone.undo(id, &session, 150);
    let synced = phone.sync(&remote, 160).expect("synced");
    assert_eq!(synced.sent, 1);
    assert!(phone.state.revived.is_empty(), "sent, and let go");
    assert_eq!(
        phone.stamps(),
        [1, 2],
        "not deleted again by its own deletion"
    );
    let synced = computer.sync(&remote, 170).expect("synced");
    assert_eq!(synced.took, 1);
    assert_eq!(computer.stamps(), [1, 2]);
    // And it stays.
    phone.sync(&remote, 180).expect("synced");
    computer.sync(&remote, 190).expect("synced");
    assert_eq!(
        (phone.stamps(), computer.stamps()),
        (vec![1, 2], vec![1, 2])
    );
}

/// A session deleted by hand after another device put it back, with that
/// device's clock an hour ahead, is deleted on every device: the deletion
/// is stamped after the putting back it follows. It used to be stamped by
/// the clock behind alone, earlier than the putting back, and to reach no
/// other device until that clock caught up.
#[test]
fn a_deletion_after_a_session_put_back_ahead_reaches_every_device() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    // The computer's clock is an hour ahead of the phone's.
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 3_600_110).expect("synced");
    let id = computer.id_of(2);
    let session = computer
        .store
        .load_history()
        .sessions
        .into_iter()
        .find(|(held, _)| *held == id)
        .expect("held")
        .1;
    computer.delete(2, 3_600_120);
    computer.sync(&remote, 3_600_130).expect("synced");
    phone.sync(&remote, 140).expect("synced");
    assert_eq!(phone.stamps(), [1]);
    computer.undo(id, &session, 3_600_150);
    computer.sync(&remote, 3_600_160).expect("synced");
    phone.sync(&remote, 170).expect("synced");
    assert_eq!(phone.stamps(), [1, 2], "put back");

    phone.delete(2, 180);
    phone.sync(&remote, 190).expect("synced");
    computer.sync(&remote, 3_600_200).expect("synced");
    assert_eq!(computer.stamps(), [1], "deleted there too");
    phone.sync(&remote, 210).expect("synced");
    assert_eq!(phone.stamps(), [1]);
}

/// Sessions a device let go as the drive deleted them, put back there by
/// hand since, by a restore say, and deleted there by hand after, stay
/// deleted, and the deletions reach every device: the deletion the device
/// took from the drive is no longer the last word on a session once it is
/// back, whether its putting back went to the drive or the drive had it
/// back already. It used to be, and the drive's putting back, later than
/// it, brought each session back at the next sync, the deletion by hand
/// undone and never sent.
#[test]
fn sessions_let_go_then_put_back_and_deleted_by_hand_stay_deleted() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2, 3]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");
    let held = |device: &Device, id: RecordId| {
        device
            .store
            .load_history()
            .sessions
            .into_iter()
            .find(|(held, _)| *held == id)
            .expect("held")
            .1
    };
    let (two, three) = (computer.id_of(2), computer.id_of(3));
    let (session_two, session_three) = (held(&computer, two), held(&computer, three));
    phone.delete(2, 120);
    phone.delete(3, 120);
    phone.sync(&remote, 130).expect("synced");
    computer.sync(&remote, 140).expect("synced");
    assert_eq!(computer.stamps(), [1], "let go");
    // 3 put back on the phone, and so in the drive before the computer
    // puts it back: the computer has nothing of it to send.
    phone.undo(three, &session_three, 150);
    phone.sync(&remote, 160).expect("synced");

    // Both put back on the computer by hand, then deleted there by hand.
    computer.undo(two, &session_two, 170);
    computer.undo(three, &session_three, 170);
    computer.sync(&remote, 180).expect("synced");
    computer.delete(2, 190);
    computer.delete(3, 190);
    computer.sync(&remote, 200).expect("synced");
    assert_eq!(computer.stamps(), [1], "neither taken back");
    phone.sync(&remote, 210).expect("synced");
    assert_eq!(phone.stamps(), [1], "deleted there too");
}

/// A session deleted by hand on one device and put back on another after
/// that deletion, by a restore once it was let go there or by Undo of a
/// deletion of its own, comes back on the device that deleted it too, and
/// stays on both and in the drive: that device's deletion, once the drive
/// holds it, is the drive's, and a putting back after it stands. The
/// device that deleted it used to send its deletion again as it found the
/// session back in the drive, and the session was gone from both.
#[test]
fn a_session_deleted_on_one_device_and_put_back_on_another_comes_back_on_both() {
    for let_go_first in [true, false] {
        let remote = MemoryRemote::default();
        let mut phone = Device::keeping(&sessions(&[1, 2]));
        phone.turn_on(&remote, "passphrase").expect("on");
        phone.sync(&remote, 100).expect("synced");
        let mut computer = Device::keeping(&[]);
        computer.turn_on(&remote, "passphrase").expect("on");
        computer.sync(&remote, 110).expect("synced");
        let two = computer.id_of(2);
        let session = computer.session(two);
        phone.delete(2, 120);
        phone.sync(&remote, 130).expect("synced");
        if let_go_first {
            computer.sync(&remote, 140).expect("synced");
            assert_eq!(computer.stamps(), [1], "let go");
        } else {
            computer.delete(2, 140);
        }
        computer.undo(two, &session, 150);
        computer.sync(&remote, 160).expect("synced");
        assert_eq!(computer.stamps(), [1, 2], "put back");

        phone.sync(&remote, 170).expect("synced");
        assert_eq!(phone.stamps(), [1, 2], "taken back");
        computer.sync(&remote, 180).expect("synced");
        assert_eq!(computer.stamps(), [1, 2], "not deleted again");
        let mut joined = Device::keeping(&[]);
        joined.turn_on(&remote, "passphrase").expect("on");
        joined.sync(&remote, 190).expect("synced");
        assert_eq!(joined.stamps(), [1, 2], "the drive's word");
    }
}

/// A session deleted by hand on two devices before either synced, and put
/// back after on one, comes back on the other too: the drive holding the
/// first's deletion as the second synced, the second's was the same, with
/// nothing of its own to send, and a putting back after it stands. The
/// second used to send its deletion as it found the session back in the
/// drive, and the session was gone from both.
#[test]
fn a_session_deleted_on_two_devices_and_put_back_on_one_comes_back_on_both() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");
    let two = phone.id_of(2);
    let session = phone.session(two);
    phone.delete(2, 120);
    computer.delete(2, 125);
    phone.sync(&remote, 130).expect("synced");
    assert_eq!(computer.sync(&remote, 140).expect("synced").sent, 0);

    phone.undo(two, &session, 150);
    phone.sync(&remote, 160).expect("synced");
    computer.sync(&remote, 170).expect("synced");
    assert_eq!(computer.stamps(), [1, 2], "taken back");
    phone.sync(&remote, 180).expect("synced");
    assert_eq!(phone.stamps(), [1, 2], "not deleted again");
}

/// What sync keeps, `state`, as a build before deletions by hand were
/// noted kept it: with no note of the deletion of `id`, which this build
/// noted, nor of having looked for such deletions since.
fn as_an_earlier_build_kept(state: &SyncState, id: RecordId) -> SyncState {
    let mut kept: serde_json::Value =
        serde_json::from_str(&state.written().expect("written")).expect("JSON");
    let fields = kept.as_object_mut().expect("fields");
    let notes = fields
        .get_mut("applied")
        .and_then(|notes| notes.as_object_mut())
        .expect("notes");
    assert!(notes.remove(&id.name()).is_some(), "noted by this build");
    fields.remove("deletionsNoted");
    SyncState::read(&kept.to_string()).expect("read")
}

/// A session deleted by hand on one device with an earlier build, its
/// deletion sent from there or held by the drive already, and put back on
/// another device after, comes back on the first once it is updated, and
/// stays on both: the first sync after the update notes each deletion by
/// hand the drive held that the earlier build kept no note of, as this
/// build notes its own. A deletion by hand not yet sent is sent all the
/// same. The first device used to send its deletion again as it found the
/// session back in the drive, and the session was gone from both.
#[test]
fn a_session_deleted_with_an_earlier_build_and_put_back_elsewhere_comes_back() {
    for sent_from_here in [true, false] {
        let remote = MemoryRemote::default();
        let mut phone = Device::keeping(&sessions(&[1, 2, 3]));
        phone.turn_on(&remote, "passphrase").expect("on");
        phone.sync(&remote, 100).expect("synced");
        let mut computer = Device::keeping(&[]);
        computer.turn_on(&remote, "passphrase").expect("on");
        computer.sync(&remote, 110).expect("synced");
        let two = computer.id_of(2);
        let session = computer.session(two);
        if sent_from_here {
            phone.delete(2, 120);
            phone.sync(&remote, 130).expect("synced");
            computer.sync(&remote, 140).expect("synced");
            assert_eq!(computer.stamps(), [1, 3], "let go");
        } else {
            computer.delete(2, 120);
            computer.sync(&remote, 130).expect("synced");
            phone.delete(2, 135);
            assert_eq!(phone.sync(&remote, 140).expect("synced").sent, 0);
        }
        phone.delete(3, 145);
        // Updated since: what sync keeps, as the earlier build kept it.
        phone.state = as_an_earlier_build_kept(&phone.state, two);
        computer.undo(two, &session, 150);
        computer.sync(&remote, 160).expect("synced");

        phone.sync(&remote, 170).expect("synced");
        assert_eq!(phone.stamps(), [1, 2], "taken back");
        computer.sync(&remote, 180).expect("synced");
        assert_eq!(computer.stamps(), [1, 2], "not deleted again");
        let mut joined = Device::keeping(&[]);
        joined.turn_on(&remote, "passphrase").expect("on");
        joined.sync(&remote, 190).expect("synced");
        assert_eq!(joined.stamps(), [1, 2], "the drive's word");
    }
}

/// A session deleted by hand on one device, then put back there by hand
/// and deleted there by hand again before it syncs, is deleted on every
/// device where another device put it back before that: the deletion
/// after the putting back here is sent, whatever note there is of the
/// deletion before it. Where the drive still holds that deletion, the new
/// one is the drive's as well, and a putting back made after it brings the
/// session back. The note of the first deletion used to stand, and the
/// other device's putting back brought the session back on the device that
/// deleted it last.
#[test]
fn a_session_put_back_and_deleted_again_before_a_sync_is_deleted_everywhere() {
    for put_back_there_first in [true, false] {
        let remote = MemoryRemote::default();
        let mut phone = Device::keeping(&sessions(&[1, 2]));
        phone.turn_on(&remote, "passphrase").expect("on");
        phone.sync(&remote, 100).expect("synced");
        let mut computer = Device::keeping(&[]);
        computer.turn_on(&remote, "passphrase").expect("on");
        computer.sync(&remote, 110).expect("synced");
        let two = phone.id_of(2);
        let session = phone.session(two);
        phone.delete(2, 120);
        phone.sync(&remote, 130).expect("synced");
        computer.sync(&remote, 140).expect("synced");
        assert_eq!(computer.stamps(), [1], "let go");
        let put_back_there = |computer: &mut Device, at: u64| {
            computer.undo(two, &session, at);
            computer.sync(&remote, at + 5).expect("synced");
        };
        if put_back_there_first {
            put_back_there(&mut computer, 150);
        }

        // Put back on the phone, from a backup say, and deleted again.
        phone.undo(two, &session, 160);
        phone.delete(2, 165);
        phone.sync(&remote, 170).expect("synced");
        assert_eq!(phone.stamps(), [1], "not taken back");
        if put_back_there_first {
            computer.sync(&remote, 180).expect("synced");
            assert_eq!(computer.stamps(), [1], "deleted there too");
        } else {
            put_back_there(&mut computer, 180);
            phone.sync(&remote, 190).expect("synced");
            assert_eq!(phone.stamps(), [1, 2], "put back after it");
            computer.sync(&remote, 200).expect("synced");
            assert_eq!(computer.stamps(), [1, 2], "not deleted again");
        }
        let mut joined = Device::keeping(&[]);
        joined.turn_on(&remote, "passphrase").expect("on");
        joined.sync(&remote, 210).expect("synced");
        assert_eq!(joined.stamps(), phone.stamps(), "the drive's word");
    }
}

/// A setting changed on one device reaches the other, and one changed on
/// the other comes back, each the newest of its own.
#[test]
fn a_setting_changed_anywhere_reaches_every_device() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&[]);
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");

    phone.settings.curriculum.num_groups = 7;
    computer.settings.playback.keyer_wpm = 31.0;
    phone.sync(&remote, 120).expect("synced");
    let synced = computer.sync(&remote, 130).expect("synced");
    assert_eq!(synced.settings.map(|s| s.curriculum.num_groups), Some(7));
    assert_eq!(computer.settings.playback.keyer_wpm, 31.0, "its own kept");
    phone.sync(&remote, 140).expect("synced");
    assert_eq!(phone.settings.playback.keyer_wpm, 31.0);
    assert_eq!(phone.settings.curriculum.num_groups, 7);
    // Changed back on the phone later: the phone's is the newest.
    phone.settings.curriculum.num_groups = 5;
    phone.sync(&remote, 150).expect("synced");
    computer.sync(&remote, 160).expect("synced");
    assert_eq!(computer.settings.curriculum.num_groups, 5);
    // Nothing changed, nothing written.
    let writes = remote.writes.get();
    phone.sync(&remote, 170).expect("synced");
    computer.sync(&remote, 180).expect("synced");
    assert_eq!(remote.writes.get(), writes);
}

/// A setting changed on a device whose clock is behind another's, after the
/// other changed it, is the one every device keeps: it is stamped after the
/// change it follows. It used to lose to that change, stamped by the clock
/// ahead, and be put back on the device it was made on as it synced.
#[test]
fn a_setting_changed_on_a_clock_behind_is_kept_everywhere() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&[]);
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    // The computer's clock is an hour ahead of the phone's.
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 3_600_110).expect("synced");

    computer.settings.curriculum.num_groups = 7;
    computer.sync(&remote, 3_600_120).expect("synced");
    phone.sync(&remote, 130).expect("synced");
    assert_eq!(phone.settings.curriculum.num_groups, 7);
    phone.settings.curriculum.num_groups = 5;
    phone.sync(&remote, 140).expect("synced");
    assert_eq!(phone.settings.curriculum.num_groups, 5, "not put back");
    computer.sync(&remote, 3_600_150).expect("synced");
    assert_eq!(computer.settings.curriculum.num_groups, 5);
}

/// What a device uses beside its history reaches every other: the runs of
/// auto-level counters, the theme, and that the guided setup and the word
/// on the paddle keys were shown; and a run cleared on one is cleared on
/// the other.
#[test]
fn counters_theme_and_marks_reach_every_device() {
    use cw_core::{AutoLevelCounters, CountersKey};
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&[]);
    let koch = CountersKey::named("koch_3_abc");
    let words = CountersKey::named("words_2");
    phone
        .store
        .save_auto_counters(&koch, AutoLevelCounters { above: 2, below: 0 });
    phone
        .store
        .save_auto_counters(&words, AutoLevelCounters { above: 0, below: 1 });
    phone.store.save_theme("dark");
    phone.store.mark_tutorial_done();
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");

    let mut computer = Device::keeping(&[]);
    computer.store.mark_told_of_paddle_keys();
    computer.turn_on(&remote, "passphrase").expect("on");
    let synced = computer.sync(&remote, 110).expect("synced");
    assert_eq!(synced.theme.as_deref(), Some("dark"), "for the app to show");
    assert_eq!(computer.store.load_theme(), "dark");
    assert!(computer.store.tutorial_done());
    assert_eq!(
        computer.store.load_auto_counters(&koch),
        AutoLevelCounters { above: 2, below: 0 }
    );
    assert_eq!(
        computer.store.load_auto_counters(&words),
        AutoLevelCounters { above: 0, below: 1 }
    );

    // A session on the computer moves a run; a level reset clears another.
    computer
        .store
        .save_auto_counters(&koch, AutoLevelCounters { above: 3, below: 0 });
    computer
        .store
        .clear_auto_counters(std::slice::from_ref(&words));
    computer.store.save_theme("light");
    computer.sync(&remote, 120).expect("synced");
    let synced = phone.sync(&remote, 130).expect("synced");
    assert_eq!(
        phone.store.load_auto_counters(&koch),
        AutoLevelCounters { above: 3, below: 0 }
    );
    assert_eq!(
        phone.store.every_auto_counter().len(),
        1,
        "the cleared run cleared"
    );
    assert_eq!(synced.theme.as_deref(), Some("light"));
    assert!(
        phone.store.told_of_paddle_keys(),
        "the computer's word on the keys"
    );
    // Nothing left to take up, either way.
    assert_eq!(phone.sync(&remote, 140).expect("synced"), Synced::default());
    assert_eq!(
        computer.sync(&remote, 150).expect("synced"),
        Synced::default()
    );
}

/// Deleted from the drive, the copy is gone for every device; begun anew
/// with another key, it is none this device opens.
#[test]
fn a_copy_deleted_or_begun_anew_ends_the_sync_here() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");

    assert!(now(delete_copy(&remote)).expect("deleted") >= 3);
    assert!(remote.names().is_empty());
    let failed = phone.sync(&remote, 120).expect_err("gone");
    assert_eq!(failed, SyncFailed::Gone);
    assert!(failed.ends_sync());
    assert_eq!(phone.stamps(), [1], "what each device holds stays");

    computer.turn_on(&remote, "another").expect("on anew");
    computer.sync(&remote, 130).expect("synced");
    assert_eq!(phone.sync(&remote, 140), Err(SyncFailed::Replaced));
    // Its key lost, a device says so rather than syncing.
    phone.state.key = "not a key".into();
    assert_eq!(phone.sync(&remote, 150), Err(SyncFailed::NoKey));
}

/// Two devices that turn sync on at the same moment end on one keyring:
/// the first made, with the same passphrase; with another, the second is
/// told so.
#[test]
fn two_devices_turning_sync_on_at_once_end_on_one_keyring() {
    let remote = MemoryRemote::default();
    // Each looked before either made one.
    let found_phone = now(look(&remote)).expect("looked");
    let found_computer = now(look(&remote)).expect("looked");
    assert_eq!(found_phone, Found::Nothing);
    let (phone_key, phone_ring) =
        now(join(&remote, "passphrase", &found_phone, Stretch::QUICK)).expect("on");
    let (computer_key, computer_ring) =
        now(join(&remote, "passphrase", &found_computer, Stretch::QUICK)).expect("on");
    assert_eq!((phone_key, phone_ring), (computer_key, computer_ring));
    assert_eq!(remote.names(), ["dust-keyring"]);

    let remote = MemoryRemote::default();
    let found = now(look(&remote)).expect("looked");
    now(join(&remote, "one", &found, Stretch::QUICK)).expect("on");
    assert_eq!(
        now(join(&remote, "two", &found, Stretch::QUICK)),
        Err(JoinError::Raced)
    );
    assert_eq!(remote.names(), ["dust-keyring"]);
}

/// A damaged file is told of and passed over; the rest syncs.
#[test]
fn a_damaged_pack_is_passed_over() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    keep(&phone.store, &sessions(&[2]));
    phone.sync(&remote, 110).expect("synced");
    let first_pack = remote
        .names()
        .into_iter()
        .find(|name| name.starts_with("dust-p-"))
        .expect("a pack");
    remote.spoil(&first_pack);

    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    let synced = computer.sync(&remote, 120).expect("synced");
    assert_eq!(synced.unreadable, 1);
    assert_eq!(computer.stamps(), [2]);
    // Told once: a file that would not open is not read again.
    assert_eq!(computer.sync(&remote, 130).expect("synced").unreadable, 0);
}

/// Store `sessions` in `store` as a restore stores a backup's: each named
/// where it finished, as a session from elsewhere ([`RecordId::placed`]).
fn restore(store: &MemoryStore, sessions: &[SessionResult]) {
    let put = sessions
        .iter()
        .map(|session| {
            Record::of(session, |json| RecordId::placed(session.timestamp, json)).expect("a record")
        })
        .collect();
    let saved = now(store.change_history(HistoryChange {
        put,
        ..HistoryChange::default()
    }));
    assert!(saved.kept().is_some(), "{saved:?}");
}

/// A history kept on one device and restored from its backup on another
/// before either synced is one history once they do, each session once,
/// under one name on both: never held twice, nor gone from either at any
/// sync, whichever syncs first.
#[test]
fn a_session_under_two_names_ends_as_one_on_every_device() {
    for computer_first in [false, true] {
        let remote = MemoryRemote::default();
        let phone = Device::keeping(&sessions(&[1, 2, 3]));
        let computer = Device::keeping(&[]);
        restore(&computer.store, &sessions(&[1, 2, 3]));
        assert!(
            listing(&phone.store)
                .iter()
                .all(|id| !listing(&computer.store).contains(id))
        );
        let mut devices = if computer_first {
            [computer, phone]
        } else {
            [phone, computer]
        };
        for device in &mut devices {
            device.turn_on(&remote, "passphrase").expect("on");
        }
        for round in 0..3 {
            for (n, device) in devices.iter_mut().enumerate() {
                device
                    .sync(&remote, 100 + round * 10 + n as u64)
                    .expect("synced");
                assert_eq!(device.stamps(), [1, 2, 3], "never twice, never gone");
            }
        }
        assert_eq!(listing(&devices[0].store), listing(&devices[1].store));
        for device in &mut devices {
            assert_eq!(
                device.sync(&remote, 200).expect("synced"),
                Synced::default()
            );
        }
    }
}

/// Of a session under two names, the one let go is let go only once the
/// one kept is in the drive: a device whose only record of it is the other
/// is never told to let it go before it can take the one kept.
#[test]
fn a_twin_is_let_go_only_once_the_one_kept_is_in_the_drive() {
    let remote = MemoryRemote::default();
    let mut computer = Device::keeping(&[]);
    restore(&computer.store, &sessions(&[1]));
    let restored = listing(&computer.store)[0];
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 100).expect("synced");
    let mut phone = Device::keeping(&sessions(&[1]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 110).expect("synced");
    assert_eq!(phone.stamps(), [1], "not taken twice");
    assert!(
        !phone.state.drive.deleted(restored),
        "the phone's own was not in the drive yet"
    );
    phone.sync(&remote, 120).expect("synced");
    assert!(phone.state.drive.deleted(restored));
    computer.sync(&remote, 130).expect("synced");
    assert_eq!(computer.stamps(), [1]);
    assert_eq!(listing(&computer.store), listing(&phone.store));
}

/// Two records of one session that are not alike in everything, as one a
/// restore filled in, are not taken for one: neither is let go.
#[test]
fn a_session_under_two_names_not_alike_is_kept_both_ways() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1]));
    let mut computer = Device::keeping(&[]);
    let mut filled = at(1);
    filled.avg_response_ms += 1.0;
    restore(&computer.store, &[filled]);
    phone.turn_on(&remote, "passphrase").expect("on");
    computer.turn_on(&remote, "passphrase").expect("on");
    for at in [100, 110, 120, 130] {
        phone.sync(&remote, at).expect("synced");
        computer.sync(&remote, at + 5).expect("synced");
    }
    assert_eq!(phone.stamps(), [1, 1]);
    assert_eq!(listing(&phone.store), listing(&computer.store));
}

/// A pack a later build wrote, which this one cannot read, is told of at
/// each sync as a later build's, not as damaged, and fetched once: not
/// passed over for good, it is read again once a build of a later layout
/// is here, and what it holds arrives then. It used to be fetched again,
/// and told of as damaged, at every sync.
#[test]
fn a_pack_of_a_later_build_is_told_of_and_read_once_dust_is_updated() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&[]);
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    // What a sync with nothing new to read fetches.
    let gets = remote.gets.get();
    phone.sync(&remote, 105).expect("synced");
    let each = remote.gets.get() - gets;
    let keys = Keys::of(&DataKey::from_hex(&phone.state.key).expect("a key"));
    let record = Record::of(&at(5), |json| RecordId::next(5, None, json)).expect("a record");
    let name = pack_name().expect("a name");
    let later = Pack {
        dust_pack: 2,
        device: "later".into(),
        at: 110,
        put: vec![Put {
            id: record.id.name(),
            s: record.json,
        }],
        ..Pack::default()
    };
    let file =
        now(remote.create(&name, seal_pack(&keys, &name, &later).expect("sealed"))).expect("made");
    let synced = phone.sync(&remote, 120).expect("synced");
    assert_eq!((synced.later, synced.unreadable), (1, 0));
    let gets = remote.gets.get();
    let synced = phone.sync(&remote, 130).expect("synced");
    assert_eq!((synced.later, synced.unreadable), (1, 0), "told of again");
    assert_eq!(remote.gets.get() - gets, each, "not fetched again");
    assert!(phone.stamps().is_empty());
    // Updated: the same pack, as this build reads it, as it reads to a
    // build that knows the later layout; and noted as by a build of a
    // layout before this one's, which could not read it.
    let readable = Pack {
        dust_pack: PACK_LAYOUT,
        ..later
    };
    now(remote.update(&file, seal_pack(&keys, &name, &readable).expect("sealed")))
        .expect("written");
    phone.state.later.insert(name, PACK_LAYOUT - 1);
    let synced = phone.sync(&remote, 140).expect("synced");
    assert_eq!((synced.took, synced.later, synced.unreadable), (1, 0, 0));
    assert_eq!(phone.stamps(), [5]);
    assert!(phone.state.later.is_empty());
}

/// A store with no room takes nothing, and reads nothing as read: the next
/// sync, with room, takes it all. Nothing is taken twice.
#[test]
fn a_store_with_no_room_takes_it_all_next_time() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.store.full.set(true);
    let failed = computer.sync(&remote, 110).expect_err("full");
    assert!(
        matches!(&failed, SyncFailed::Store(why) if why.contains("no room")),
        "{failed:?}"
    );
    assert!(computer.state.read.is_empty());
    computer.store.full.set(false);
    assert_eq!(computer.sync(&remote, 120).expect("synced").took, 2);
    assert_eq!(computer.sync(&remote, 130).expect("synced").took, 0);
    assert_eq!(computer.stamps(), [1, 2]);
}

/// A sync the drive stops partway through, signed out say, picks up where
/// it left off once signed in again; nothing sent twice is two sessions.
#[test]
fn a_sync_cut_short_picks_up_where_it_left_off() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    keep(&phone.store, &sessions(&[2]));
    remote.signed_out.set(true);
    let failed = phone.sync(&remote, 110).expect_err("signed out");
    assert_eq!(failed, SyncFailed::Remote(RemoteError::SignIn));
    assert!(!failed.ends_sync());
    assert!(failed.told().contains("sign in"));
    remote.signed_out.set(false);
    assert_eq!(phone.sync(&remote, 120).expect("synced").sent, 1);

    remote.full.set(true);
    keep(&phone.store, &sessions(&[3]));
    assert_eq!(
        phone.sync(&remote, 130),
        Err(SyncFailed::Remote(RemoteError::Full))
    );
    remote.full.set(false);
    phone.sync(&remote, 140).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 150).expect("synced");
    assert_eq!(computer.stamps(), [1, 2, 3]);
}

/// A session deleted by hand, whose pack the drive made though its answer
/// was lost on the way back, is the drive's deletion once the next sync
/// finds that pack there, as it would have been had the answer come: a
/// putting back made on another device after it brings the session back,
/// and the deletion is not sent again. It used to be sent again, later than
/// the putting back, and the session was gone from every device.
#[test]
fn a_deletion_in_a_pack_whose_upload_went_unanswered_is_the_drives_once_found() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");
    let two = phone.id_of(2);
    let session = computer.session(two);
    phone.delete(2, 120);
    remote.answer_lost.set(true);
    assert!(
        matches!(
            phone.sync(&remote, 130),
            Err(SyncFailed::Remote(RemoteError::Failed(_)))
        ),
        "no answer"
    );
    remote.answer_lost.set(false);
    let unanswered: Vec<&RecordId> = phone
        .state
        .unanswered
        .values()
        .flat_map(|deletions| deletions.keys())
        .collect();
    assert_eq!(unanswered, [&two], "noted until a sync finds the pack");
    assert!(!phone.state.applied.contains_key(&two));

    computer.sync(&remote, 140).expect("synced");
    assert_eq!(computer.stamps(), [1], "the pack was made");
    computer.undo(two, &session, 150);
    computer.sync(&remote, 155).expect("synced");

    phone.sync(&remote, 160).expect("synced");
    assert_eq!(phone.stamps(), [1, 2], "taken back");
    assert!(phone.state.unanswered.is_empty(), "found");
    computer.sync(&remote, 170).expect("synced");
    assert_eq!(computer.stamps(), [1, 2], "not deleted again");
    let mut joined = Device::keeping(&[]);
    joined.turn_on(&remote, "passphrase").expect("on");
    joined.sync(&remote, 180).expect("synced");
    assert_eq!(joined.stamps(), [1, 2], "the drive's word");
}

/// The same where the app stops once the drive has made the pack, before
/// the sync ends and the app keeps what it came to, as a tab closed then
/// does: what sync keeps is kept as such a pack goes, its deletions by hand
/// noted against it, and the app opens again knowing of it, so the next
/// sync finds the pack there as it would one whose answer was lost. It used
/// to open with what sync kept before that sync, which knew nothing of the
/// pack, and sent the deletion again, later than a putting back made
/// elsewhere since: the session was gone from every device.
#[test]
fn a_deletion_in_a_pack_made_as_the_app_stopped_is_the_drives_once_found() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");
    let two = phone.id_of(2);
    let session = computer.session(two);
    phone.delete(2, 120);

    // What the app keeps as the sync asks; stopped before the sync ends, it
    // opens with what it kept last, from before that sync where the sync
    // asked nothing.
    let kept = RefCell::new(phone.state.clone());
    now(sync_once_with(
        &*phone.store,
        &remote,
        &mut phone.state,
        &phone.settings,
        130,
        &BTreeSet::new,
        &|state| {
            *kept.borrow_mut() = state.clone();
            done(())
        },
    ))
    .expect("synced");
    phone.state = kept.into_inner();

    computer.sync(&remote, 140).expect("synced");
    assert_eq!(computer.stamps(), [1], "the pack was made");
    computer.undo(two, &session, 150);
    computer.sync(&remote, 155).expect("synced");

    phone.sync(&remote, 160).expect("synced");
    assert_eq!(phone.stamps(), [1, 2], "taken back");
    assert!(phone.state.unanswered.is_empty(), "found");
    computer.sync(&remote, 170).expect("synced");
    assert_eq!(computer.stamps(), [1, 2], "not deleted again");
    let mut joined = Device::keeping(&[]);
    joined.turn_on(&remote, "passphrase").expect("on");
    joined.sync(&remote, 180).expect("synced");
    assert_eq!(joined.stamps(), [1, 2], "the drive's word");
}

/// A pack the drive never made, full say, is sent again by the next sync,
/// its deletions by hand with it: none is taken for the drive's for having
/// been asked for once.
#[test]
fn a_pack_whose_upload_failed_is_sent_again() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");
    let two = phone.id_of(2);
    phone.delete(2, 120);
    remote.full.set(true);
    assert_eq!(
        phone.sync(&remote, 130),
        Err(SyncFailed::Remote(RemoteError::Full))
    );
    assert_eq!(phone.state.unanswered.len(), 1, "until a sync looks");
    remote.full.set(false);
    assert_eq!(phone.sync(&remote, 140).expect("synced").sent, 1);
    assert!(phone.state.unanswered.is_empty(), "never made, so dropped");
    assert!(phone.state.applied.contains_key(&two), "sent, and answered");
    computer.sync(&remote, 150).expect("synced");
    assert_eq!(computer.stamps(), [1], "let go");
}

/// A session put back by hand under a sync turned on anew, before that
/// sync has read the drive, is put back after the drive's deletion of it,
/// stamped ahead on another device though that deletion is: the putting
/// back was stamped knowing nothing of the drive, what sync knew before it
/// was turned on anew gone with it, and is stamped again once the first
/// read finds the deletion, as one made after that read would be. It used
/// to keep the bare stamp of this device's clock, earlier than the
/// deletion, and its sync let the session go again.
#[test]
fn a_session_put_back_before_a_new_syncs_first_read_is_put_back_after_the_drives_deletion() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    // The computer's clock is an hour ahead of the phone's.
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 3_600_110).expect("synced");
    let two = phone.id_of(2);
    let session = phone.session(two);
    computer.delete(2, 3_600_120);
    computer.sync(&remote, 3_600_130).expect("synced");
    // Deleted on the phone too, and synced: the drive's deletion known.
    phone.delete(2, 150);
    phone.sync(&remote, 160).expect("synced");
    assert!(phone.state.stamp_change(two, 200) > 3_600_130);

    // Turned on anew, and the session put back before that sync reads.
    phone.turn_on(&remote, "passphrase").expect("on anew");
    phone.undo(two, &session, 200);
    phone.sync(&remote, 210).expect("synced");
    assert_eq!(phone.stamps(), [1, 2], "not let go");
    assert!(
        phone.state.drive.revives[&two] > phone.state.drive.tombs[&two],
        "put back after the deletion"
    );
    computer.sync(&remote, 3_600_220).expect("synced");
    assert_eq!(computer.stamps(), [1, 2], "taken back");
    let mut joined = Device::keeping(&[]);
    joined.turn_on(&remote, "passphrase").expect("on");
    joined.sync(&remote, 230).expect("synced");
    assert_eq!(joined.stamps(), [1, 2], "the drive's word");
}

/// A folder that does what a test asks as it is first listed: what the
/// user does here as a sync begins, before that sync lists the store.
struct ListedAfter<'a> {
    remote: &'a MemoryRemote,
    first: RefCell<Option<Box<dyn FnOnce() + 'a>>>,
}

impl Remote for ListedAfter<'_> {
    fn list(&self) -> Pending<Result<Vec<RemoteFile>, RemoteError>> {
        let first = self.first.borrow_mut().take();
        if let Some(first) = first {
            first();
        }
        self.remote.list()
    }

    fn get(&self, id: &str) -> Pending<Result<Vec<u8>, RemoteError>> {
        self.remote.get(id)
    }

    fn create(&self, name: &str, bytes: Vec<u8>) -> Pending<Result<String, RemoteError>> {
        self.remote.create(name, bytes)
    }

    fn update(&self, id: &str, bytes: Vec<u8>) -> Pending<Result<(), RemoteError>> {
        self.remote.update(id, bytes)
    }

    fn delete(&self, id: &str) -> Pending<Result<(), RemoteError>> {
        self.remote.delete(id)
    }
}

/// A session put back by hand here as a sync begins, its deletion sent
/// before, is not let go by that sync, though the sync finds it listed and
/// the drive holding its deletion, with no note of the putting back in
/// what it plans from: a sync lets go of nothing the app says was put back
/// since it began, and the next sends it as put back. The sync used to let
/// it go again as the drive's deletion.
#[test]
fn a_sync_lets_go_of_nothing_put_back_since_it_began() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let two = phone.id_of(2);
    let session = phone.session(two);
    phone.delete(2, 120);
    phone.sync(&remote, 130).expect("synced");

    // Put back as the next sync begins, and noted once it is over, as the
    // app notes one.
    let since = RefCell::new(BTreeSet::new());
    let store = Rc::clone(&phone.store);
    let folder = ListedAfter {
        remote: &remote,
        first: RefCell::new(Some(Box::new(|| {
            let json = serde_json::to_string(&session).expect("JSON");
            let saved = now(store.change_history(HistoryChange {
                put: vec![Record { id: two, json }],
                unforget: vec![two],
                ..HistoryChange::default()
            }));
            assert!(saved.kept().is_some());
            since.borrow_mut().insert(two);
        }))),
    };
    let synced = now(sync_once_with(
        &*phone.store,
        &folder,
        &mut phone.state,
        &phone.settings,
        140,
        &|| since.borrow().clone(),
        &|_| done(()),
    ))
    .expect("synced");
    assert_eq!(phone.stamps(), [1, 2], "not let go");
    assert_eq!(synced.let_go, 0);
    assert!(
        !phone.state.applied.contains_key(&two),
        "nor noted as the drive's"
    );
    let at = phone.state.stamp_change(two, 150);
    phone.state.revived.insert(two, at);
    assert_eq!(phone.sync(&remote, 160).expect("synced").sent, 1);
    let mut joined = Device::keeping(&[]);
    joined.turn_on(&remote, "passphrase").expect("on");
    joined.sync(&remote, 170).expect("synced");
    assert_eq!(joined.stamps(), [1, 2], "the drive's word");
}

/// A folder listed before what a test asks happens, as it is first listed:
/// what another device, or another copy of the app on this one, does once
/// a sync has listed the folder and before it reads what it listed. A
/// memory folder lists at once, so what the sync reads is what was there.
struct ListedBefore<'a> {
    remote: &'a MemoryRemote,
    then: RefCell<Option<Box<dyn FnOnce() + 'a>>>,
}

impl Remote for ListedBefore<'_> {
    fn list(&self) -> Pending<Result<Vec<RemoteFile>, RemoteError>> {
        let listed = self.remote.list();
        let then = self.then.borrow_mut().take();
        if let Some(then) = then {
            then();
        }
        listed
    }

    fn get(&self, id: &str) -> Pending<Result<Vec<u8>, RemoteError>> {
        self.remote.get(id)
    }

    fn create(&self, name: &str, bytes: Vec<u8>) -> Pending<Result<String, RemoteError>> {
        self.remote.create(name, bytes)
    }

    fn update(&self, id: &str, bytes: Vec<u8>) -> Pending<Result<(), RemoteError>> {
        self.remote.update(id, bytes)
    }

    fn delete(&self, id: &str) -> Pending<Result<(), RemoteError>> {
        self.remote.delete(id)
    }
}

/// Two copies of the app on the phone sync at once, each from what the
/// phone kept: one lets a session go as the drive deleted it, while the
/// other, which listed it here before it went, reads it put back on the
/// computer since and so takes nothing back. Whichever ends last lays what
/// it kept over the other's, and the note that the drive's deletion
/// accounts for the session gone here stays, though the drive the two read
/// between them shows it put back since: no sync here took it back. So the
/// next sync takes it back, and the putting back stands everywhere. The
/// note used to be dropped on that evidence alone, and the next sync found
/// the session gone here with nothing to account for it, as one deleted by
/// hand, and sent its deletion: the putting back was deleted everywhere.
#[test]
fn a_session_one_copy_lets_go_while_another_finds_it_put_back_is_taken_back() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");
    let two = computer.id_of(2);
    let session = computer.session(two);
    computer.delete(2, 120);
    computer.sync(&remote, 130).expect("synced");

    // One copy lists the folder; the computer puts 2 back; the other copy
    // syncs, finding 2 here, and reads it put back.
    let kept = phone.state.clone();
    let mut one = kept.clone();
    let other = RefCell::new(kept.clone());
    let store = Rc::clone(&phone.store);
    let settings = phone.settings.clone();
    {
        let folder = ListedBefore {
            remote: &remote,
            then: RefCell::new(Some(Box::new(|| {
                computer.undo(two, &session, 150);
                computer.sync(&remote, 160).expect("synced");
                let synced = now(sync_once(
                    &*store,
                    &remote,
                    &mut other.borrow_mut(),
                    &settings,
                    170,
                ))
                .expect("synced");
                assert_eq!(synced.took, 0, "2 here");
            }))),
        };
        // The first goes on with what it listed, which holds only the
        // deletion, and lets 2 go.
        now(sync_once(
            &*phone.store,
            &folder,
            &mut one,
            &phone.settings,
            140,
        ))
        .expect("synced");
    }
    assert_eq!(phone.stamps(), [1], "let go");
    let other = other.into_inner();
    let (gone, back) = (one.applied[&two], other.drive.revives[&two]);
    assert!(back >= gone, "put back since: {back} after {gone}");

    let laid = [
        one.merged(Some(&kept), &other),
        other.merged(Some(&kept), &one),
    ];
    for merged in &laid {
        assert_eq!(
            merged.applied.get(&two),
            Some(&gone),
            "the drive's deletion accounts for 2 gone here"
        );
    }
    phone.state = laid[0].clone();
    let synced = phone.sync(&remote, 180).expect("synced");
    assert_eq!((synced.took, synced.sent), (1, 0), "taken back, none sent");
    assert_eq!(phone.stamps(), [1, 2]);
    computer.sync(&remote, 190).expect("synced");
    assert_eq!(computer.stamps(), [1, 2], "the putting back stands");
    let mut joined = Device::keeping(&[]);
    joined.turn_on(&remote, "passphrase").expect("on");
    joined.sync(&remote, 200).expect("synced");
    assert_eq!(joined.stamps(), [1, 2], "the drive's word");
}

/// A device whose store lost what sync took, though sync remembers reading
/// it, reads every pack again and takes it back.
#[test]
fn a_store_that_lost_what_it_took_takes_it_again() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1, 2]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.sync(&remote, 100).expect("synced");
    let mut computer = Device::keeping(&[]);
    computer.turn_on(&remote, "passphrase").expect("on");
    computer.sync(&remote, 110).expect("synced");
    computer.store = Rc::new(MemoryStore::default());
    assert_eq!(computer.sync(&remote, 120).expect("synced").took, 2);
    assert_eq!(computer.stamps(), [1, 2]);
}

/// Sync stops, rather than guess, where the store cannot list its history.
#[test]
fn a_history_that_cannot_be_listed_is_not_synced() {
    let remote = MemoryRemote::default();
    let mut phone = Device::keeping(&sessions(&[1]));
    phone.turn_on(&remote, "passphrase").expect("on");
    phone.store.unlistable.set(true);
    assert!(matches!(
        phone.sync(&remote, 100),
        Err(SyncFailed::Store(_))
    ));
    assert!(
        !remote
            .names()
            .iter()
            .any(|name| name.starts_with("dust-p-"))
    );
}
