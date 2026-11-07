//! Sync in the app: the card's moves, and the syncs the app makes by
//! itself, against a folder and a sign-in of a test's own.

use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::persist::Deleted;
use crate::persist::memory::MemoryStore;
use crate::persist::record::{HistoryChange, Pending, Record, RecordId};
use crate::persist::tests::at;
use crate::sync::drive::fake::FakeAuth;
use crate::sync::remote::{MemoryRemote, RemoteFile};
use crate::sync::run::{join, look, sync_once};
use crate::sync::seal::Stretch;
use crate::testing::{Harness, run};

/// The app, linked to `remote` through `auth`.
fn linked(harness: &mut Harness, remote: &Rc<MemoryRemote>, auth: &Rc<FakeAuth>) -> AppCtx {
    linked_to(harness, Rc::clone(remote) as Rc<dyn Remote>, auth)
}

/// The app, linked to `remote`, whatever folder it is, through `auth`.
fn linked_to(harness: &mut Harness, remote: Rc<dyn Remote>, auth: &Rc<FakeAuth>) -> AppCtx {
    linked_through(harness, remote, Rc::clone(auth) as Rc<dyn Auth>)
}

/// The app, linked to `remote` through `auth`, whatever sign-in it is.
fn linked_through(harness: &mut Harness, remote: Rc<dyn Remote>, auth: Rc<dyn Auth>) -> AppCtx {
    let link = Link { auth, remote };
    let ctx = harness.in_app(|| harness.ctx.link_sync(link));
    harness.ctx = ctx;
    ctx
}

/// A sign-in that finishes only once a test lets go, as one does whose
/// code is being traded for a token, and then as `auth` signs in.
struct Finishing {
    auth: Rc<FakeAuth>,
    held: Rc<Cell<bool>>,
}

impl Finishing {
    fn of(auth: &Rc<FakeAuth>) -> Rc<Self> {
        Rc::new(Self {
            auth: Rc::clone(auth),
            held: Rc::new(Cell::new(false)),
        })
    }

    fn hold(&self, held: bool) {
        self.held.set(held);
    }
}

impl Auth for Finishing {
    fn token(&self, fresh: bool) -> Pending<Result<String, AuthError>> {
        self.auth.token(fresh)
    }

    fn sign_in(&self) -> Pending<Result<(), AuthError>> {
        let (auth, held) = (Rc::clone(&self.auth), Rc::clone(&self.held));
        Box::pin(async move {
            while held.get() {
                sleep_ms(POLL_MS).await;
            }
            auth.sign_in().await
        })
    }

    fn kept(&self) -> Option<String> {
        self.auth.kept()
    }

    fn keep(&self, kept: Option<String>) {
        self.auth.keep(kept);
    }

    fn forget(&self) {
        self.auth.forget();
    }
}

/// A folder that answers nothing while a test holds it, and then answers
/// as `remote` does: a sync under way, for as long as the test likes. Held
/// at its writes alone, it answers what is read of it at once: a sync under
/// way past its reading, while the card's sign-in looks at the folder.
struct Held {
    remote: Rc<MemoryRemote>,
    held: Rc<Cell<bool>>,
    writes: Rc<Cell<bool>>,
}

impl Held {
    fn of(remote: &Rc<MemoryRemote>) -> Rc<Self> {
        Rc::new(Self {
            remote: Rc::clone(remote),
            held: Rc::new(Cell::new(false)),
            writes: Rc::new(Cell::new(false)),
        })
    }

    fn hold(&self, held: bool) {
        self.held.set(held);
    }

    fn hold_writes(&self, held: bool) {
        self.writes.set(held);
    }

    /// What `asked` of the folder comes to, asked once the test lets go:
    /// of what it holds, where `writes`.
    fn once_let_go<T: 'static>(
        &self,
        writes: bool,
        asked: impl FnOnce(&MemoryRemote) -> Pending<T> + 'static,
    ) -> Pending<T> {
        let remote = Rc::clone(&self.remote);
        let (held, writes_held) = (Rc::clone(&self.held), Rc::clone(&self.writes));
        Box::pin(async move {
            while held.get() || (writes && writes_held.get()) {
                sleep_ms(POLL_MS).await;
            }
            asked(&remote).await
        })
    }
}

impl Remote for Held {
    fn list(&self) -> Pending<Result<Vec<RemoteFile>, RemoteError>> {
        self.once_let_go(false, |remote| remote.list())
    }

    fn get(&self, id: &str) -> Pending<Result<Vec<u8>, RemoteError>> {
        let id = id.to_string();
        self.once_let_go(false, move |remote| remote.get(&id))
    }

    fn create(&self, name: &str, bytes: Vec<u8>) -> Pending<Result<String, RemoteError>> {
        let name = name.to_string();
        self.once_let_go(true, move |remote| remote.create(&name, bytes))
    }

    fn update(&self, id: &str, bytes: Vec<u8>) -> Pending<Result<(), RemoteError>> {
        let id = id.to_string();
        self.once_let_go(true, move |remote| remote.update(&id, bytes))
    }

    fn delete(&self, id: &str) -> Pending<Result<(), RemoteError>> {
        let id = id.to_string();
        self.once_let_go(true, move |remote| remote.delete(&id))
    }
}

fn status(harness: &Harness) -> SyncStatus {
    harness.in_app(|| harness.ctx.sync.status.peek().clone())
}

/// What the app holds of the history, by stamp.
fn stamps(harness: &Harness) -> Vec<u64> {
    harness.in_app(|| {
        harness
            .ctx
            .sessions
            .peek()
            .iter()
            .map(|session| session.timestamp)
            .collect()
    })
}

/// Another device that synced `stamps` into `remote` with `passphrase`.
fn another_device(
    remote: &MemoryRemote,
    passphrase: &str,
    stamps: &[u64],
) -> (Rc<MemoryStore>, SyncState) {
    let store = Rc::new(MemoryStore::default());
    let mut newest = None;
    let put = stamps
        .iter()
        .map(|&stamp| {
            let session = at(stamp);
            let record =
                Record::of(&session, |json| RecordId::next(stamp, newest, json)).expect("a record");
            newest = Some(record.id);
            record
        })
        .collect();
    assert!(
        now(store.change_history(HistoryChange {
            put,
            ..HistoryChange::default()
        }))
        .kept()
        .is_some()
    );
    let found = now(look(remote)).expect("looked");
    let (key, keyring) = now(join(remote, passphrase, &found, Stretch::QUICK)).expect("joined");
    let mut state = SyncState::new("other".into(), key.to_hex(), keyring, None);
    now(sync_once(
        &*store,
        remote,
        &mut state,
        &cw_core::TrainingSettings::default(),
        1,
    ))
    .expect("synced");
    (store, state)
}

/// Delete the record `id` by hand in `store`, another device's, at `at`.
fn delete_in(store: &MemoryStore, id: RecordId, at: u64) {
    let session = store
        .load_history()
        .sessions
        .into_iter()
        .find(|(held, _)| *held == id)
        .expect("held")
        .1;
    assert!(
        now(store.change_history(HistoryChange {
            forget: vec![(id, Deleted::of(&session, at))],
            ..HistoryChange::default()
        }))
        .kept()
        .is_some()
    );
}

fn now<T>(pending: impl Future<Output = T>) -> T {
    use std::task::{Context, Poll, Waker};
    let mut pending = std::pin::pin!(pending);
    match pending
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the fakes answer at once"),
    }
}

/// A build with no client for where it runs offers no sync at all.
#[test]
fn a_build_that_cannot_sync_offers_none() {
    let harness = Harness::new();
    assert_eq!(status(&harness), SyncStatus::Unavailable(None));
    // Nothing it does reaches for any.
    harness.in_app(|| harness.ctx.sync_now());
    harness.in_app(|| harness.ctx.sync_soon());
    assert_eq!(*harness.store.sync.borrow(), None);
}

/// Turned on as the first device: signed in, a new passphrase chosen, and
/// asked twice, then the history sent, sealed; and kept on as the app
/// opens again.
#[test]
fn sync_is_turned_on_with_a_new_passphrase() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = linked(&mut harness, &remote, &auth);
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });

        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        assert_eq!(auth.asked.get(), 1);
        assert_eq!(
            status(&harness),
            SyncStatus::Passphrase {
                new: true,
                problem: None
            }
        );
        harness.in_app(|| ctx.sync_passphrase("short".into(), Some("short".into())));
        assert!(
            matches!(status(&harness), SyncStatus::Passphrase { problem: Some(p), .. } if p.contains("8 characters"))
        );
        harness
            .in_app(|| ctx.sync_passphrase("a long passphrase".into(), Some("another one".into())));
        assert!(
            matches!(status(&harness), SyncStatus::Passphrase { problem: Some(p), .. } if p.contains("not the same"))
        );
        harness.in_app(|| {
            ctx.sync_passphrase(
                "a long passphrase".into(),
                Some(" a long passphrase".into()),
            )
        });
        assert_eq!(status(&harness), SyncStatus::Unlocking);
        harness.advance(100).await;
        assert!(
            matches!(
                status(&harness),
                SyncStatus::On {
                    last: Some(_),
                    syncing: false,
                    problem: None,
                    needs_sign_in: false
                }
            ),
            "{:?}",
            status(&harness)
        );
        assert!(
            remote
                .names()
                .iter()
                .any(|name| name.starts_with("dust-p-"))
        );
        let kept = harness.store.sync.borrow().clone().expect("kept");
        let state = SyncState::read(&kept).expect("reads");
        assert_eq!(state.sign_in.as_deref(), Some("kept"));
        assert!(
            !kept.contains("a long passphrase"),
            "the passphrase is never kept"
        );

        // The app opened again on the same store: on, and synced soon.
        let again = harness
            .in_app(|| AppCtx::new(harness.app.clone(), Default::default(), Default::default()));
        let link = Link {
            auth: Rc::clone(&auth) as Rc<dyn Auth>,
            remote: Rc::clone(&remote) as Rc<dyn Remote>,
        };
        let again = harness.in_app(|| again.link_sync(link));
        assert!(harness.in_app(|| again.sync.status.peek().is_on()));
    });
}

/// Joined from another device: the passphrase chosen there, and no other,
/// opens it; the history and the settings there come here.
#[test]
fn sync_is_joined_with_the_passphrase_chosen_elsewhere() {
    run(|| async {
        let remote = Rc::new(MemoryRemote::default());
        another_device(&remote, "chosen elsewhere", &[1, 2]);
        let mut harness = Harness::new();
        keep(&harness, &[3]);
        let auth = FakeAuth::giving(&["t"]);
        let ctx = linked(&mut harness, &remote, &auth);
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        assert_eq!(
            status(&harness),
            SyncStatus::Passphrase {
                new: false,
                problem: None
            }
        );
        harness.in_app(|| ctx.sync_passphrase("wrong".into(), None));
        harness.advance(100).await;
        assert!(
            matches!(status(&harness), SyncStatus::Passphrase { new: false, problem: Some(p) } if p.contains("does not open"))
        );
        harness.in_app(|| ctx.sync_passphrase("chosen elsewhere".into(), None));
        harness.advance(100).await;
        assert!(status(&harness).is_on());
        assert_eq!(stamps(&harness), [1, 2, 3], "the drive's taken up at once");
    });
}

/// Joined, the app takes up the theme, the level counters and the marks
/// of the device that synced first: the shell shows its theme at once.
#[test]
fn joining_takes_up_the_theme_counters_and_marks() {
    run(|| async {
        use cw_core::{AutoLevelCounters, CountersKey};
        let remote = Rc::new(MemoryRemote::default());
        let other = Rc::new(MemoryStore::default());
        let koch = CountersKey::named("koch_3_abc");
        other.save_auto_counters(&koch, AutoLevelCounters { above: 2, below: 0 });
        other.save_theme("dark");
        other.mark_tutorial_done();
        let found = now(look(&*remote)).expect("looked");
        let (key, keyring) =
            now(join(&*remote, "chosen elsewhere", &found, Stretch::QUICK)).expect("joined");
        let mut state = SyncState::new("other".into(), key.to_hex(), keyring, None);
        now(sync_once(
            &*other,
            &*remote,
            &mut state,
            &cw_core::TrainingSettings::default(),
            1,
        ))
        .expect("synced");

        let mut harness = Harness::new();
        let auth = FakeAuth::giving(&["t"]);
        let ctx = linked(&mut harness, &remote, &auth);
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_passphrase("chosen elsewhere".into(), None));
        harness.advance(100).await;
        assert!(status(&harness).is_on());
        assert_eq!(
            harness
                .in_app(|| harness.ctx.sync.theme.peek().clone())
                .as_deref(),
            Some("dark")
        );
        assert_eq!(harness.store.load_theme(), "dark");
        assert!(harness.store.tutorial_done());
        assert_eq!(
            harness.store.load_auto_counters(&koch),
            AutoLevelCounters { above: 2, below: 0 }
        );
    });
}

/// Turned down at Google, or cancelled, sync stays off.
#[test]
fn a_sign_in_turned_down_leaves_sync_off() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        auth.refuses.set(true);
        let ctx = linked(&mut harness, &remote, &auth);
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        auth.refuses.set(false);
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_cancel());
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        assert!(remote.names().is_empty());
    });
}

/// Cancelled as the sign-in finishes, its code traded for a token or the
/// drive looked at, sync stays off, and the sign-in is forgotten: Cancel is
/// the last word. The sign-in used to go on to ask for the passphrase once
/// it had finished, signed in still.
#[test]
fn a_sign_in_cancelled_as_it_finishes_stays_cancelled() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let held = Held::of(&remote);
        let auth = FakeAuth::giving(&["t"]);
        let finishing = Finishing::of(&auth);
        let ctx = linked_through(
            &mut harness,
            Rc::clone(&held) as Rc<dyn Remote>,
            Rc::clone(&finishing) as Rc<dyn Auth>,
        );
        finishing.hold(true);
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        assert_eq!(status(&harness), SyncStatus::SigningIn);
        harness.in_app(|| ctx.sync_cancel());
        finishing.hold(false);
        harness.advance(100).await;
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        assert!(!auth.signed_in.get(), "forgotten as it finished");

        held.hold(true);
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        assert_eq!(status(&harness), SyncStatus::SigningIn, "looking");
        harness.in_app(|| ctx.sync_cancel());
        held.hold(false);
        harness.advance(100).await;
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        assert!(!auth.signed_in.get(), "forgotten as it looked");

        // Asked for again, it goes on as ever.
        harness.in_app(|| ctx.sync_sign_in());
        harness.advance(100).await;
        assert_eq!(
            status(&harness),
            SyncStatus::Passphrase {
                new: true,
                problem: None
            }
        );
        assert!(remote.names().is_empty());
    });
}

/// On, the app syncs by itself a moment after a session is kept, and
/// waits for a session under way to end first.
#[test]
fn a_session_kept_is_synced_a_moment_later() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let packs = || {
            remote
                .names()
                .iter()
                .filter(|name| name.starts_with("dust-p-"))
                .count()
        };
        let before = packs();
        harness.in_app(|| ctx.keep_finished(at(5), |_| {}));
        harness.advance(u64::from(SYNC_SOON_MS) / 2).await;
        assert_eq!(packs(), before, "not yet");
        harness.advance(u64::from(SYNC_SOON_MS)).await;
        assert_eq!(packs(), before + 1);
    });
}

/// Coming back into sight syncs where the last sync is stamped ahead of the
/// clock, as it is once the clock is set back after it: no sync of a moment
/// ago, however long ago it was. It used to be taken for one until the
/// clock passed it again.
#[test]
fn a_last_sync_stamped_ahead_of_the_clock_is_synced_again() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let lists = remote.lists.get();
        harness.in_app(|| ctx.sync_if_stale());
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(remote.lists.get(), lists, "synced a moment ago");

        // The clock set back an hour since.
        harness.in_app(|| {
            let mut state = ctx.sync.state;
            if let Some(state) = state.write().as_mut() {
                state.last = Some(now_ms() + 3_600_000);
            }
        });
        harness.in_app(|| ctx.sync_if_stale());
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(remote.lists.get(), lists + 1);
    });
}

/// Signed out at Google, the card asks for a sign-in, and syncs once
/// given one.
#[test]
fn a_sign_in_run_out_is_asked_for_again() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        remote.signed_out.set(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(matches!(
            status(&harness),
            SyncStatus::On {
                needs_sign_in: true,
                problem: None,
                ..
            }
        ));
        // The app does not keep asking by itself.
        harness.in_app(|| ctx.sync_soon());
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        remote.signed_out.set(false);
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        assert!(matches!(
            status(&harness),
            SyncStatus::On {
                needs_sign_in: false,
                syncing: false,
                ..
            }
        ));
    });
}

/// Deleted from the drive by another device, sync goes off here, saying
/// so, and forgets what it kept; turned off with its copy, it deletes it.
#[test]
fn sync_turned_off_forgets_what_it_kept() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        now(delete_copy(&*remote)).expect("deleted");
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(
            matches!(status(&harness), SyncStatus::Off { problem: Some(p) } if p.contains("gone"))
        );
        assert_eq!(*harness.store.sync.borrow(), None);
        assert!(!auth.signed_in.get());
        assert_eq!(stamps(&harness), [1], "what this device holds stays");

        let ctx = turned_on(&mut harness, &remote, &auth).await;
        assert!(!remote.names().is_empty());
        harness.in_app(|| ctx.sync_turn_off(true));
        harness.pump();
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        assert!(remote.names().is_empty());

        let ctx = turned_on(&mut harness, &remote, &auth).await;
        harness.in_app(|| ctx.sync_turn_off(false));
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        assert!(
            !remote.names().is_empty(),
            "the copy stays for the other devices"
        );
        assert_eq!(*harness.store.sync.borrow(), None);
    });
}

/// Turned off here while a sync is under way, sync stays off: the sync, as
/// it ends, keeps nothing of what sync kept, nor syncs again; turning sync
/// on asks for the passphrase again; and the app opens with it off.
#[test]
fn sync_turned_off_while_it_syncs_stays_off() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(matches!(
            status(&harness),
            SyncStatus::On { syncing: true, .. }
        ));
        // Another asked for after it, then sync turned off.
        harness.in_app(|| ctx.sync_now());
        harness.in_app(|| ctx.sync_turn_off(false));
        assert_eq!(*harness.store.sync.borrow(), None);
        let lists = remote.lists.get();
        held.hold(false);
        harness.advance(100).await;
        assert_eq!(remote.lists.get(), lists + 1, "the one under way alone");
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        assert_eq!(*harness.store.sync.borrow(), None, "nothing kept again");

        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        assert_eq!(
            status(&harness),
            SyncStatus::Passphrase {
                new: false,
                problem: None
            }
        );
        harness.in_app(|| ctx.sync_cancel());
        let again = harness
            .in_app(|| AppCtx::new(harness.app.clone(), Default::default(), Default::default()));
        let link = Link {
            auth: Rc::clone(&auth) as Rc<dyn Auth>,
            remote: Rc::clone(&held) as Rc<dyn Remote>,
        };
        let again = harness.in_app(|| again.link_sync(link));
        assert_eq!(
            harness.in_app(|| again.sync.status.peek().clone()),
            SyncStatus::Off { problem: None }
        );
    });
}

/// A sync under way as sync is turned off asks nothing more of the drive
/// than the request it waits on: by its next, the sign-in may be another
/// account's, whose drive it would write into, sealed with a key that
/// account's sync does not go by. It used to go on to its end.
#[test]
fn a_sync_under_way_as_sync_is_turned_off_asks_nothing_more_of_the_drive() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        keep(&harness, &[5]);
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| ctx.sync_turn_off(false));
        let asked = || (remote.lists.get(), remote.gets.get(), remote.writes.get());
        let (lists, gets, writes) = asked();
        held.hold(false);
        harness.advance(100).await;
        assert_eq!(
            asked(),
            (lists + 1, gets, writes),
            "the one waited on alone"
        );
    });
}

/// What a sync stored before sync was turned off is shown, though the sync
/// stops as it asks the drive for more: the sessions it took from the drive
/// are here.
#[test]
fn what_a_sync_stopped_by_turning_sync_off_took_is_shown() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        another_device(&remote, "a long passphrase", &[1, 2]);
        keep(&harness, &[5]);
        held.hold_writes(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| ctx.sync_turn_off(false));
        held.hold_writes(false);
        harness.advance(100).await;
        assert_eq!(stamps(&harness), [1, 2, 5]);
    });
}

/// A sync due a moment after a change does not start while the copy is
/// being deleted: every file of Dust's goes, and sync is off without a
/// word of trouble.
#[test]
fn a_sync_due_while_the_copy_is_deleted_does_not_start() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        harness.in_app(|| ctx.keep_finished(at(5), |_| {}));
        held.hold(true);
        harness.in_app(|| ctx.sync_turn_off(true));
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(status(&harness), SyncStatus::TurningOff);
        let lists = remote.lists.get();
        held.hold(false);
        harness.advance(100).await;
        assert_eq!(remote.lists.get(), lists + 1, "the deletion's alone");
        assert!(remote.names().is_empty(), "{:?}", remote.names());
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        assert_eq!(*harness.store.sync.borrow(), None);
    });
}

/// The copy not deleted, the card says so, and shows sync as it stands:
/// no sync left under way that ended as the deletion waited for it.
#[test]
fn a_copy_not_deleted_is_told_of_and_no_sync_is_left_showing() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(matches!(
            status(&harness),
            SyncStatus::On { syncing: true, .. }
        ));
        harness.in_app(|| ctx.sync_turn_off(true));
        *remote.failing.borrow_mut() = Some(RemoteError::Failed("offline".into()));
        held.hold(false);
        harness.advance(100).await;
        assert!(
            matches!(
                status(&harness),
                SyncStatus::On {
                    last: Some(_),
                    syncing: false,
                    problem: Some(p),
                    needs_sign_in: false,
                } if p.contains("not deleted: offline")
            ),
            "{:?}",
            status(&harness)
        );
        assert!(harness.store.sync.borrow().is_some(), "still on here");
        // Back online, it syncs as ever.
        *remote.failing.borrow_mut() = None;
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(matches!(
            status(&harness),
            SyncStatus::On {
                syncing: false,
                problem: None,
                ..
            }
        ));
    });
}

/// The copy not deleted, a sync follows by itself a moment later: a session
/// kept while the deletion was under way, whose sync could not start then,
/// goes with it. Nothing used to sync it until the next change.
#[test]
fn a_copy_not_deleted_syncs_what_was_kept_meanwhile() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let packs = || {
            remote
                .names()
                .iter()
                .filter(|name| name.starts_with("dust-p-"))
                .count()
        };
        let before = packs();
        held.hold(true);
        harness.in_app(|| ctx.sync_turn_off(true));
        harness.in_app(|| ctx.keep_finished(at(5), |_| {}));
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(status(&harness), SyncStatus::TurningOff);
        *remote.failing.borrow_mut() = Some(RemoteError::Failed("offline".into()));
        held.hold(false);
        harness.advance(100).await;
        assert!(
            matches!(&status(&harness), SyncStatus::On { problem: Some(p), .. } if p.contains("not deleted")),
            "{:?}",
            status(&harness)
        );
        *remote.failing.borrow_mut() = None;
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(packs(), before + 1, "the session kept meanwhile");
        assert!(matches!(
            status(&harness),
            SyncStatus::On {
                syncing: false,
                problem: None,
                ..
            }
        ));
    });
}

/// A sync asked for just as sync is turned off never starts, and sync
/// turned on again syncs as ever.
#[test]
fn a_sync_asked_for_as_sync_is_turned_off_never_starts() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let lists = remote.lists.get();
        harness.in_app(|| {
            ctx.sync_now();
            ctx.sync_turn_off(false);
        });
        harness.pump();
        assert_eq!(remote.lists.get(), lists);
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        assert_eq!(*harness.store.sync.borrow(), None);

        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(
            matches!(
                status(&harness),
                SyncStatus::On {
                    last: Some(_),
                    syncing: false,
                    ..
                }
            ),
            "{:?}",
            status(&harness)
        );
    });
}

/// A setting changed here while a sync is under way stays, laid over what
/// that sync takes from another device, and reaches that device next.
#[test]
fn a_setting_changed_while_it_syncs_stays_and_goes_next() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        // Another device joins, and changes a setting there.
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let mut theirs = harness.in_app(|| ctx.settings.peek().clone().clamp());
        theirs.curriculum.num_groups = 9;
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms() + 1_000,
        ))
        .expect("synced");

        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        // Changed here meanwhile, and stored as the app stores it.
        harness.in_app(|| {
            let mut settings = ctx.settings;
            settings.write().playback.keyer_wpm = 31.0;
            ctx.store_settings();
        });
        held.hold(false);
        harness.advance(100).await;
        let here = harness.in_app(|| ctx.settings.peek().clone());
        assert_eq!(here.curriculum.num_groups, 9, "the other device's");
        assert_eq!(here.playback.keyer_wpm, 31.0, "as changed here");
        let stored = harness.store.read_settings().expect("stored");
        assert_eq!(
            (stored.curriculum.num_groups, stored.playback.keyer_wpm),
            (9, 31.0)
        );

        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        let synced = now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms() + 2_000,
        ))
        .expect("synced");
        assert_eq!(
            synced.settings.map(|settings| settings.playback.keyer_wpm),
            Some(31.0)
        );
    });
}

/// Signed in again while a sync is under way, the new sign-in is the one
/// kept: the sync, as it ends, does not put back the one it began with.
#[test]
fn a_sign_in_made_while_it_syncs_is_the_one_kept() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        held.hold(false);
        harness.advance(100).await;
        assert!(
            matches!(
                status(&harness),
                SyncStatus::On {
                    last: Some(_),
                    syncing: false,
                    problem: None,
                    needs_sign_in: false
                }
            ),
            "{:?}",
            status(&harness)
        );
        let kept = harness.store.sync.borrow().clone().expect("kept");
        let state = SyncState::read(&kept).expect("reads");
        assert_eq!(state.sign_in.as_deref(), Some("kept 2"));
    });
}

/// A session deleted and put back by Undo after the deletion was synced
/// stays, here and on every device.
#[test]
fn undo_after_a_synced_deletion_keeps_the_session_everywhere() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| ctx.undo_delete());
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(stamps(&harness), [1, 2]);
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &Default::default(),
            now_ms(),
        ))
        .expect("synced");
        let theirs: Vec<u64> = other
            .load_history()
            .sessions
            .iter()
            .map(|(_, s)| s.timestamp)
            .collect();
        assert_eq!(theirs, [1, 2]);
        // And syncing again here does not take it away.
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert_eq!(stamps(&harness), [1, 2]);
    });
}

/// A session put back by Undo after a deletion another device made with its
/// clock an hour ahead, which this device has synced, stays: it is put
/// back after the deletion it follows, whatever the clocks say, and the
/// drive says so to every device. It used to be stamped by this device's
/// clock alone, earlier than that deletion, and deleted again by it at the
/// next sync.
#[test]
fn undo_after_a_deletion_stamped_ahead_keeps_the_session() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        harness.in_app(|| ctx.delete_sessions(&[two]));
        let ahead = now_ms() + 3_600_000;
        delete_in(&other, two, ahead);
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &Default::default(),
            ahead,
        ))
        .expect("synced");
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| ctx.undo_delete());
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(stamps(&harness), [1, 2], "not deleted again");
        let (joined, _) = another_device(&remote, "a long passphrase", &[]);
        let theirs: Vec<u64> = joined
            .load_history()
            .sessions
            .iter()
            .map(|(_, s)| s.timestamp)
            .collect();
        assert_eq!(theirs, [1, 2], "the drive's word");
    });
}

/// A session put back by Undo while a sync begun before sync was turned off
/// and on again is still under way stays: the sync after it, of sync as it
/// is on now, sends it as put back, and the drive's deletion of it does
/// not take it away again. The sync under way used to take the note of it
/// as it ended, and keep nothing of it, as sync was turned on anew since.
#[test]
fn undo_while_a_sync_from_before_sync_was_turned_on_anew_ends_keeps_the_session() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        // Deleted on another device, and here.
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        delete_in(&other, two, 2);
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &Default::default(),
            2,
        ))
        .expect("synced");
        harness.in_app(|| ctx.delete_sessions(&[two]));
        keep(&harness, &[5]);
        held.hold_writes(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();

        harness.in_app(|| ctx.sync_turn_off(false));
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        harness.in_app(|| ctx.undo_delete());
        held.hold_writes(false);
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(stamps(&harness), [1, 2, 5], "not deleted again");
    });
}

/// What sync noted of a session put back while a sync was under way goes
/// with everything else sync keeps as sync is turned off here, as one put
/// back while none was would: turned on again, sync begins from nothing.
#[test]
fn a_session_put_back_while_it_syncs_is_forgotten_as_sync_is_turned_off() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        harness.in_app(|| ctx.delete_sessions(&[two]));
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| ctx.undo_delete());
        let noted = || harness.in_app(|| ctx.sync.revived.peek().len());
        assert_eq!(noted(), 1, "noted for once the sync is over");
        harness.in_app(|| ctx.sync_turn_off(false));
        assert_eq!(noted(), 0);
    });
}

/// Turn sync on here as the first device.
async fn turned_on(
    harness: &mut Harness,
    remote: &Rc<MemoryRemote>,
    auth: &Rc<FakeAuth>,
) -> AppCtx {
    turned_on_through(harness, Rc::clone(remote) as Rc<dyn Remote>, auth).await
}

/// Turn sync on here as the first device, through a folder the test can
/// hold ([`Held`]).
async fn turned_on_held(
    harness: &mut Harness,
    remote: &Rc<MemoryRemote>,
    auth: &Rc<FakeAuth>,
) -> (AppCtx, Rc<Held>) {
    let held = Held::of(remote);
    let ctx = turned_on_through(harness, Rc::clone(&held) as Rc<dyn Remote>, auth).await;
    (ctx, held)
}

/// Turn sync on here as the first device, through `remote`.
async fn turned_on_through(
    harness: &mut Harness,
    remote: Rc<dyn Remote>,
    auth: &Rc<FakeAuth>,
) -> AppCtx {
    let ctx = linked_to(harness, remote, auth);
    harness.in_app(|| ctx.sync_sign_in());
    harness.pump();
    let new = matches!(status(harness), SyncStatus::Passphrase { new: true, .. });
    let repeated = new.then(|| "a long passphrase".to_string());
    harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), repeated));
    harness.advance(100).await;
    assert!(status(harness).is_on(), "{:?}", status(harness));
    ctx
}

/// Keep sessions at `stamps` in the app's store, and in the app.
fn keep(harness: &Harness, stamps: &[u64]) {
    for &stamp in stamps {
        harness.in_app(|| {
            harness
                .ctx
                .keep_finished(at(stamp), |saved| assert!(saved.kept().is_some()))
        });
    }
}
