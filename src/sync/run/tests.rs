//! Devices syncing through one folder: each a store of its own and a state
//! of its own, as each install has, the folder shared.

use std::rc::Rc;

use cw_core::{SessionResult, TrainingSettings};

use super::*;
use crate::persist::Store;
use crate::persist::memory::MemoryStore;
use crate::persist::record::{HistoryChange, Record, RecordId};
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
