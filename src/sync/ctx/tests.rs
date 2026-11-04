//! Sync in the app: the card's moves, and the syncs the app makes by
//! itself, against a folder and a sign-in of a test's own.

use std::rc::Rc;

use super::*;
use crate::persist::memory::MemoryStore;
use crate::persist::record::{HistoryChange, Record, RecordId};
use crate::persist::tests::at;
use crate::sync::drive::fake::FakeAuth;
use crate::sync::remote::MemoryRemote;
use crate::sync::run::{join, look, sync_once};
use crate::sync::seal::Stretch;
use crate::testing::{Harness, run};

/// The app, linked to `remote` through `auth`.
fn linked(harness: &mut Harness, remote: &Rc<MemoryRemote>, auth: &Rc<FakeAuth>) -> AppCtx {
    let link = Link {
        auth: Rc::clone(auth) as Rc<dyn Auth>,
        remote: Rc::clone(remote) as Rc<dyn Remote>,
    };
    let ctx = harness.in_app(|| harness.ctx.link_sync(link));
    harness.ctx = ctx;
    ctx
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

/// Turn sync on here as the first device.
async fn turned_on(
    harness: &mut Harness,
    remote: &Rc<MemoryRemote>,
    auth: &Rc<FakeAuth>,
) -> AppCtx {
    let ctx = linked(harness, remote, auth);
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
