//! Sync in the app: the card's moves, and the syncs the app makes by
//! itself, against a folder and a sign-in of a test's own.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::*;
use crate::persist::backup::Backup;
use crate::persist::memory::MemoryStore;
use crate::persist::record::{HistoryChange, Pending, Record, RecordId};
use crate::persist::tests::at;
use crate::persist::{Deleted, Lock};
use crate::sync::drive::fake::FakeAuth;
use crate::sync::pack::{Pack, pack_name, seal_pack};
use crate::sync::remote::{MemoryRemote, RemoteFile};
use crate::sync::run::{join, look, sync_once};
use crate::sync::seal::{DataKey, Keys, Stretch};
use crate::sync::settings::{SETTINGS_PREFIX, SettingsFile, StampedSettings};
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

/// A sign-in each asking of which waits for the test to answer it, as one
/// waiting on the user does, in whatever order the test answers them: one
/// answered as signed in then signs in as `auth` does.
struct Answered {
    auth: Rc<FakeAuth>,
    askings: RefCell<Vec<Answer>>,
}

/// What an asking for a sign-in comes to, once the test answers it.
type Answer = Rc<Cell<Option<Result<(), AuthError>>>>;

impl Answered {
    fn of(auth: &Rc<FakeAuth>) -> Rc<Self> {
        Rc::new(Self {
            auth: Rc::clone(auth),
            askings: RefCell::new(Vec::new()),
        })
    }

    /// How many times a sign-in was asked for: the number the next asking
    /// is answered by.
    fn asked(&self) -> usize {
        self.askings.borrow().len()
    }

    /// Answer the asking `nth`, counted from nought.
    fn answer(&self, nth: usize, answer: Result<(), AuthError>) {
        self.askings.borrow()[nth].set(Some(answer));
    }
}

impl Auth for Answered {
    fn token(&self, fresh: bool) -> Pending<Result<String, AuthError>> {
        self.auth.token(fresh)
    }

    fn sign_in(&self) -> Pending<Result<(), AuthError>> {
        let answer = Rc::new(Cell::new(None));
        self.askings.borrow_mut().push(Rc::clone(&answer));
        let auth = Rc::clone(&self.auth);
        Box::pin(async move {
            loop {
                match answer.take() {
                    Some(Ok(())) => return auth.sign_in().await,
                    Some(Err(err)) => return Err(err),
                    None => sleep_ms(POLL_MS).await,
                }
            }
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

/// A folder whose requests asked while a test holds it shut answer only
/// once the test opens it, while those asked once it is no longer shut
/// answer at once, as `remote` does: one request under way for long, and
/// others answered beside it.
struct Gated {
    remote: Rc<MemoryRemote>,
    shut: Rc<Cell<bool>>,
    opened: Rc<Cell<bool>>,
}

impl Gated {
    fn of(remote: &Rc<MemoryRemote>) -> Rc<Self> {
        Rc::new(Self {
            remote: Rc::clone(remote),
            shut: Rc::new(Cell::new(false)),
            opened: Rc::new(Cell::new(false)),
        })
    }

    /// What `asked` of the folder comes to: once the test opens it, where
    /// it was asked while the folder was shut.
    fn ask<T: 'static>(
        &self,
        asked: impl FnOnce(&MemoryRemote) -> Pending<T> + 'static,
    ) -> Pending<T> {
        let remote = Rc::clone(&self.remote);
        let (waits, opened) = (self.shut.get(), Rc::clone(&self.opened));
        Box::pin(async move {
            while waits && !opened.get() {
                sleep_ms(POLL_MS).await;
            }
            asked(&remote).await
        })
    }
}

impl Remote for Gated {
    fn list(&self) -> Pending<Result<Vec<RemoteFile>, RemoteError>> {
        self.ask(|remote| remote.list())
    }

    fn get(&self, id: &str) -> Pending<Result<Vec<u8>, RemoteError>> {
        let id = id.to_string();
        self.ask(move |remote| remote.get(&id))
    }

    fn create(&self, name: &str, bytes: Vec<u8>) -> Pending<Result<String, RemoteError>> {
        let name = name.to_string();
        self.ask(move |remote| remote.create(&name, bytes))
    }

    fn update(&self, id: &str, bytes: Vec<u8>) -> Pending<Result<(), RemoteError>> {
        let id = id.to_string();
        self.ask(move |remote| remote.update(&id, bytes))
    }

    fn delete(&self, id: &str) -> Pending<Result<(), RemoteError>> {
        let id = id.to_string();
        self.ask(move |remote| remote.delete(&id))
    }
}

fn status(harness: &Harness) -> SyncStatus {
    harness.in_app(|| harness.ctx.sync.status.peek().clone())
}

/// Another copy of the app on the harness's store, a tab or a window, as
/// it opens: linked to `remote` through a sign-in of its own.
fn another_copy(harness: &Harness, remote: &Rc<MemoryRemote>) -> AppCtx {
    another_copy_through(
        harness,
        Rc::clone(remote) as Rc<dyn Remote>,
        FakeAuth::giving(&["t"]) as Rc<dyn Auth>,
    )
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

/// Delete the record `id` by hand in `store`, another device's, at `at`:
/// the session it held.
fn delete_in(store: &MemoryStore, id: RecordId, at: u64) -> cw_core::SessionResult {
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
    session
}

/// Put `session` back by hand as the record `id` in `store`, another
/// device's, at `at`, and note it in what that device's sync keeps,
/// `state`, as the app notes an Undo.
fn put_back_in(
    store: &MemoryStore,
    state: &mut SyncState,
    id: RecordId,
    session: &cw_core::SessionResult,
    at: u64,
) {
    let json = serde_json::to_string(session).expect("JSON");
    assert!(
        now(store.change_history(HistoryChange {
            put: vec![Record { id, json }],
            unforget: vec![id],
            ..HistoryChange::default()
        }))
        .kept()
        .is_some()
    );
    let at = state.stamp_change(id, at);
    state.revived.insert(id, at);
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

/// A sign-in asked for again while sync is on, still under way as sync is
/// turned off, keeps nothing once it finishes: sync is off, and no one is
/// signed in. It used to keep its sign-in for as long as the app ran.
#[test]
fn a_sign_in_under_way_as_sync_is_turned_off_keeps_nothing() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t", "u"]);
        let finishing = Finishing::of(&auth);
        let ctx = linked_through(
            &mut harness,
            Rc::clone(&remote) as Rc<dyn Remote>,
            Rc::clone(&finishing) as Rc<dyn Auth>,
        );
        harness.in_app(|| ctx.sync_sign_in());
        harness.advance(100).await;
        let passphrase = || "a long passphrase".to_string();
        harness.in_app(|| ctx.sync_passphrase(passphrase(), Some(passphrase())));
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        finishing.hold(true);
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_turn_off(false));
        finishing.hold(false);
        harness.advance(100).await;
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });
        assert!(!auth.signed_in.get(), "forgotten as it finished");
    });
}

/// Asked for again while the first waits on the user, by a second tap on
/// "Sign in to sync" or on the corner's cloud, a sign-in Android answers at
/// once, as one waits already, leaves the first to go on: once the user
/// signs in there, the card asks for no sign-in and tells of no trouble,
/// and sync goes on. The first used to be given up as the second was asked
/// for, the user's sign-in with it: the card asked for a sign-in still,
/// told of the second's failing, and nothing synced.
#[test]
fn a_sign_in_asked_for_again_while_the_first_waits_on_the_user_goes_on() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let answered = Answered::of(&auth);
        let ctx = turned_on_answering(&mut harness, &remote, &answered).await;
        remote.signed_out.set(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(matches!(
            status(&harness),
            SyncStatus::On {
                needs_sign_in: true,
                ..
            }
        ));
        remote.signed_out.set(false);

        let (first, lists) = (answered.asked(), remote.lists.get());
        harness.in_app(|| ctx.sync_sign_in());
        harness.in_app(|| ctx.sync_sign_in());
        answered.answer(first + 1, Err(AuthError::Failed("busy".into())));
        harness.advance(50).await;
        answered.answer(first, Ok(()));
        harness.advance(100).await;
        assert!(
            matches!(
                status(&harness),
                SyncStatus::On {
                    syncing: false,
                    problem: None,
                    needs_sign_in: false,
                    ..
                }
            ),
            "{:?}",
            status(&harness)
        );
        assert_eq!(remote.lists.get(), lists + 1, "synced");
    });
}

/// A sign-in that fails once another has signed in since it was asked for
/// says nothing of it: the card tells of no trouble while sync is on, and
/// still asks for the passphrase as sync is turned on.
#[test]
fn a_sign_in_failing_after_another_signed_in_says_nothing() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let answered = Answered::of(&auth);
        let ctx = turned_on_answering(&mut harness, &remote, &answered).await;
        remote.signed_out.set(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        remote.signed_out.set(false);
        let first = answered.asked();
        harness.in_app(|| ctx.sync_sign_in());
        harness.in_app(|| ctx.sync_sign_in());
        answered.answer(first, Ok(()));
        harness.advance(100).await;
        answered.answer(first + 1, Err(AuthError::Failed("busy".into())));
        harness.advance(100).await;
        assert!(
            matches!(
                status(&harness),
                SyncStatus::On {
                    problem: None,
                    needs_sign_in: false,
                    ..
                }
            ),
            "{:?}",
            status(&harness)
        );

        harness.in_app(|| ctx.sync_turn_off(false));
        let first = answered.asked();
        harness.in_app(|| ctx.sync_sign_in());
        harness.in_app(|| ctx.sync_sign_in());
        answered.answer(first, Ok(()));
        harness.advance(100).await;
        answered.answer(first + 1, Err(AuthError::Failed("busy".into())));
        harness.advance(100).await;
        assert_eq!(
            status(&harness),
            SyncStatus::Passphrase {
                new: false,
                problem: None
            }
        );
    });
}

/// A sign-in asked for again stops the one before it, in a browser and on
/// a computer: the card waits on the second, and tells nothing of the
/// first's stopping, whether sync is being turned on or is on, signed out.
#[test]
fn a_sign_in_stopped_by_another_asked_for_says_nothing() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let answered = Answered::of(&auth);
        let ctx = linked_through(
            &mut harness,
            Rc::clone(&remote) as Rc<dyn Remote>,
            Rc::clone(&answered) as Rc<dyn Auth>,
        );
        harness.in_app(|| ctx.sync_sign_in());
        harness.in_app(|| ctx.sync_sign_in());
        answered.answer(0, Err(AuthError::Cancelled));
        harness.advance(100).await;
        assert_eq!(status(&harness), SyncStatus::SigningIn, "the second's");
        answered.answer(1, Ok(()));
        harness.advance(100).await;
        assert_eq!(
            status(&harness),
            SyncStatus::Passphrase {
                new: true,
                problem: None
            }
        );
        let passphrase = || "a long passphrase".to_string();
        harness.in_app(|| ctx.sync_passphrase(passphrase(), Some(passphrase())));
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));

        remote.signed_out.set(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        remote.signed_out.set(false);
        harness.in_app(|| ctx.sync_sign_in());
        harness.in_app(|| ctx.sync_sign_in());
        answered.answer(2, Err(AuthError::Failed("stopped".into())));
        harness.advance(100).await;
        assert!(
            matches!(
                status(&harness),
                SyncStatus::On {
                    problem: None,
                    needs_sign_in: true,
                    ..
                }
            ),
            "{:?}",
            status(&harness)
        );
        answered.answer(3, Ok(()));
        harness.advance(100).await;
        assert!(
            matches!(
                status(&harness),
                SyncStatus::On {
                    problem: None,
                    needs_sign_in: false,
                    ..
                }
            ),
            "{:?}",
            status(&harness)
        );
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

/// Files in the drive a later build wrote, a pack and a settings file, are
/// told of as a newer Dust's, with what to do, not as damaged ones.
#[test]
fn files_a_later_build_wrote_are_told_of_as_a_newer_dusts() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let kept = harness.store.sync.borrow().clone().expect("kept");
        let key = DataKey::from_hex(&SyncState::read(&kept).expect("reads").key).expect("a key");
        let keys = Keys::of(&key);
        let name = pack_name().expect("a name");
        let pack = Pack {
            dust_pack: 2,
            device: "later".into(),
            ..Pack::default()
        };
        now(remote.create(&name, seal_pack(&keys, &name, &pack).expect("sealed"))).expect("made");
        let name = keys.name(SETTINGS_PREFIX, "settings", "later");
        let file = SettingsFile {
            dust_settings: 2,
            ..SettingsFile::new("later", 1, StampedSettings::new())
        };
        let raw = serde_json::to_vec(&file).expect("JSON");
        now(remote.create(&name, keys.seal(&name, &raw).expect("sealed"))).expect("made");
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(
            matches!(&status(&harness), SyncStatus::On { problem: Some(p), .. } if p == "2 files in your Google Drive were written by a newer Dust: update Dust here to read them."),
            "{:?}",
            status(&harness)
        );
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

/// A setting another copy of the app on the same store, a tab or a window,
/// changed and stored while a sync here was under way stays, laid over
/// what that sync took from another device, as one changed here meanwhile
/// does, and reaches that device next: the sync's end takes up what the
/// store holds before it lays what it took over the settings. It used to
/// lay what it took over the settings this copy held, which knew nothing of
/// the other copy's change, and store that over it, the older value
/// winning.
#[test]
fn a_setting_stored_in_another_copy_while_it_syncs_stays_and_goes_next() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.take_up_store());
        // Another device joins, and changes a setting there.
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let mut theirs = harness.in_app(|| ctx.settings.peek().clone().clamp());
        theirs.curriculum.num_groups = 4;
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        std::thread::sleep(std::time::Duration::from_millis(5));

        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        // Changed in the other window meanwhile, after the other device's,
        // and stored as the app stores it.
        harness.in_app(|| {
            let mut settings = window.settings;
            settings.write().curriculum.num_groups = 7;
            window.store_settings();
        });
        let store = Rc::clone(&harness.store);
        let stored = move || store.read_settings().map(|s| s.curriculum.num_groups);
        assert_eq!(stored(), Some(7), "stored there");
        held.hold(false);
        harness.advance(100).await;
        assert_eq!(stored(), Some(7), "the other window's, the later");
        let here = harness.in_app(|| ctx.settings.peek().curriculum.num_groups);
        assert_eq!(here, 7, "taken up here");

        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        let synced = now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        assert_eq!(
            synced
                .settings
                .map(|settings| settings.curriculum.num_groups),
            Some(7),
            "the drive's word"
        );
        assert_eq!(stored(), Some(7));
        let there = harness.in_app(|| window.settings.peek().curriculum.num_groups);
        assert_eq!(there, 7);
    });
}

/// Sync turned off here in another copy of the app on the same store, a
/// tab or a window, is off in this one as well as it next syncs, or would
/// keep what sync keeps, and the card says why: it syncs nothing more and
/// keeps nothing, so sync stays off for every copy. Each copy used to write
/// back the whole of what it read of sync as it opened, turning sync back
/// on in the store; and taking up sync turned off, the card said nothing
/// of why it was off.
#[test]
fn sync_turned_off_in_another_copy_of_the_app_stays_off() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        assert!(harness.in_app(|| window.sync.status.peek().is_on()));
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| window.sync_turn_off(false));
        held.hold(false);
        harness.advance(100).await;
        let off_elsewhere = |status: SyncStatus| matches!(status, SyncStatus::Off { problem: Some(p) } if p == "Sync was turned off in another tab or window of Dust.");
        assert!(off_elsewhere(status(&harness)), "{:?}", status(&harness));
        assert_eq!(*harness.store.sync.borrow(), None, "kept off");

        // On again here, then off in another window: the sync due here next
        // does not start.
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.sync_turn_off(false));
        let lists = remote.lists.get();
        keep(&harness, &[5]);
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(remote.lists.get(), lists, "nothing synced");
        assert!(off_elsewhere(status(&harness)), "{:?}", status(&harness));
        assert_eq!(*harness.store.sync.borrow(), None);
    });
}

/// Sync turned off in another copy of the app, and turned on anew there
/// just as this copy looks whether the store still holds what sync keeps,
/// is on as that copy kept it: this copy turns sync off here, and clears
/// nothing from the store, which held nothing as it looked. It used to
/// clear the store as it turned sync off, the other copy's new sync with
/// it.
#[test]
fn sync_taken_up_as_off_clears_nothing_another_copy_keeps() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.sync_turn_off(false));
        let anew = SyncState::new("anew".into(), "00".repeat(32), "keyring".into(), None)
            .written()
            .expect("written");
        *harness.store.sync_kept_after_look.borrow_mut() = Some(anew.clone());
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(
            matches!(status(&harness), SyncStatus::Off { .. }),
            "{:?}",
            status(&harness)
        );
        assert_eq!(
            harness.store.sync.borrow().as_deref(),
            Some(anew.as_str()),
            "the other copy's, kept"
        );
    });
}

/// Sync turned off in another copy of the app and turned on anew there, as
/// with another account or passphrase, is the sync the store keeps: this
/// copy, still on the sync it found, takes it as changed there, turns sync
/// off here saying why, and writes nothing of its own over it; the app
/// opens again on the sync turned on there. This copy used to write what it
/// kept over it as it next synced, and the sync turned on there was gone at
/// the next launch.
#[test]
fn sync_turned_on_anew_in_another_copy_of_the_app_is_the_sync_kept() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.sync_turn_off(false));
        harness.in_app(|| window.sync_sign_in());
        harness.pump();
        harness.in_app(|| window.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(harness.in_app(|| window.sync.status.peek().is_on()));
        let device =
            |kept: Option<String>| SyncState::read(&kept.expect("kept")).expect("reads").device;
        let theirs = device(harness.store.sync.borrow().clone());

        let lists = remote.lists.get();
        harness.in_app(|| ctx.sync_now());
        harness.advance(100).await;
        assert_eq!(remote.lists.get(), lists, "nothing synced");
        assert!(
            matches!(&status(&harness), SyncStatus::Off { problem: Some(p) } if p == "Sync was turned on anew in another tab or window of Dust. Open Dust again here to sync."),
            "{:?}",
            status(&harness)
        );
        assert_eq!(
            device(harness.store.sync.borrow().clone()),
            theirs,
            "the other copy's"
        );
        let again = harness
            .in_app(|| AppCtx::new(harness.app.clone(), Default::default(), Default::default()));
        let link = Link {
            auth: Rc::clone(&auth) as Rc<dyn Auth>,
            remote: Rc::clone(&remote) as Rc<dyn Remote>,
        };
        let again = harness.in_app(|| again.link_sync(link));
        let opened = harness.in_app(|| {
            again
                .sync
                .state
                .peek()
                .as_ref()
                .map(|state| state.device.clone())
        });
        assert_eq!(opened, Some(theirs), "opened on the sync turned on there");
    });
}

/// Sync turned off here once another copy of the app on the same store has
/// turned it off and on anew there is off here, saying why, and the store
/// keeps what that copy kept: that copy syncs on. Turning sync off here
/// used to clear the store, and that copy's sync went off at its next sync.
#[test]
fn sync_turned_off_here_leaves_the_sync_another_copy_turned_on_anew() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.sync_turn_off(false));
        harness.in_app(|| window.sync_sign_in());
        harness.pump();
        harness.in_app(|| window.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        let theirs = kept_device(&harness);
        assert!(theirs.is_some(), "kept there");

        harness.in_app(|| ctx.sync_turn_off(false));
        assert_eq!(kept_device(&harness), theirs, "the other copy's");
        assert!(
            matches!(&status(&harness), SyncStatus::Off { problem: Some(p) } if p == "Sync was turned on anew in another tab or window of Dust. Open Dust again here to sync."),
            "{:?}",
            status(&harness)
        );
        harness.in_app(|| window.sync_now());
        harness.advance(100).await;
        let there = harness.in_app(|| window.sync.status.peek().clone());
        assert!(
            matches!(there, SyncStatus::On { problem: None, .. }),
            "{there:?}"
        );
    });
}

/// A sync ending the whole sync here, the copy gone from the drive, clears
/// nothing from the store that another copy of the app on it kept there,
/// where this copy's own was never kept there, the store refusing it: that
/// copy's sync stays as it kept it. It used to be cleared from the store.
#[test]
fn a_sync_ended_here_clears_nothing_another_copy_kept() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        harness.store.sync_refused.set(true);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        harness.store.sync_refused.set(false);
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.sync_sign_in());
        harness.pump();
        harness.in_app(|| window.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        let theirs = kept_device(&harness);
        assert!(theirs.is_some(), "kept there");

        now(delete_copy(&*remote)).expect("deleted");
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert_eq!(kept_device(&harness), theirs, "the other copy's");
        assert!(
            matches!(&status(&harness), SyncStatus::Off { problem: Some(p) } if p.contains("gone")),
            "{:?}",
            status(&harness)
        );
    });
}

/// A sync turned on here that the store never took, the store refusing it
/// as it was turned on, is kept as its sync ends, over another sync turned
/// on in another copy of the app meanwhile, as the one last chosen here, as
/// a copy keeping its whole state kept it: this copy never takes the other
/// copy's sync for its own.
#[test]
fn a_sync_the_store_never_took_is_kept_whole_as_it_ends() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        harness.store.sync_refused.set(true);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        harness.store.sync_refused.set(false);
        let ours = device_of(&harness, ctx);
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.sync_sign_in());
        harness.pump();
        harness.in_app(|| window.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        let theirs = kept_device(&harness);
        assert!(theirs.is_some() && theirs != ours, "the other copy's kept");

        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert_eq!(kept_device(&harness), ours, "this copy's, kept whole");
        assert_eq!(device_of(&harness, ctx), ours, "still its own");
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
    });
}

/// The device of the sync `ctx`, a copy of the app, holds, if it is on.
fn device_of(harness: &Harness, ctx: AppCtx) -> Option<String> {
    harness.in_app(|| {
        ctx.sync
            .state
            .peek()
            .as_ref()
            .map(|state| state.device.clone())
    })
}

/// The device of the sync the store keeps, if it keeps one.
fn kept_device(harness: &Harness) -> Option<String> {
    let kept = harness.store.sync.borrow().clone()?;
    Some(SyncState::read(&kept).expect("reads").device)
}

/// What a sync took from the drive and stored before a later request
/// failed, the drive full as it sent what this device kept, is shown: the
/// store has it, and the history here takes it up. It used to stay hidden
/// until the app opened again, as a store counts its own change as seen.
#[test]
fn what_a_sync_took_before_it_failed_is_shown() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let _ = another_device(&remote, "a long passphrase", &[1, 2]);
        keep(&harness, &[3]);
        remote.full.set(true);
        harness.in_app(|| ctx.sync_now());
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        assert_eq!(stamps(&harness), [1, 2, 3]);
    });
}

/// What sync keeps, there but unreadable for a moment, as a file another
/// program holds open is, is no sync turned off in another copy: sync
/// stays on, and what it keeps stays where it is. A read that failed used
/// to read as nothing kept, and sync forgot its key and its sign-in.
#[test]
fn sync_whose_state_cannot_be_read_for_a_moment_stays_on() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, _held) = turned_on_held(&mut harness, &remote, &auth).await;
        let kept = harness.store.sync.borrow().clone();
        assert!(kept.is_some(), "kept as it was turned on");
        harness.store.sync_unreadable.set(true);
        harness.in_app(|| ctx.sync_now());
        harness.advance(100).await;
        harness.store.sync_unreadable.set(false);
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        assert!(harness.store.sync.borrow().is_some(), "still kept");
    });
}

/// What sync keeps, which the store would not take as sync was turned on,
/// is not taken for sync turned off in another copy: sync stays on, and
/// what it keeps is kept once the store takes it.
#[test]
fn sync_the_store_would_not_keep_at_first_stays_on() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        harness.store.sync_refused.set(true);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        assert_eq!(*harness.store.sync.borrow(), None);
        harness.store.sync_refused.set(false);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        assert!(harness.store.sync.borrow().is_some(), "kept now");
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

/// Signed in again where the store would not take what sync keeps at once,
/// as a full disk will not, the new sign-in is the one the next sync keeps,
/// and the one signed in with still: it is a change made here, which the
/// store has yet to take, though the sync begins from this copy's state,
/// which holds it. That sync, as it ended, used to take the store's older
/// sign-in for the one to keep, write it, and sign in with it, dropping the
/// token just got, often as the old one was turned down.
#[test]
fn a_sign_in_the_store_would_not_take_at_once_is_kept_by_the_next_sync() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        assert_eq!(kept_state(&harness).sign_in.as_deref(), Some("kept"));
        held.hold(true);
        harness.store.sync_refused.set(true);
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        assert_eq!(kept_state(&harness).sign_in.as_deref(), Some("kept"));
        harness.store.sync_refused.set(false);
        held.hold(false);
        harness.advance(1_000).await;
        assert_eq!(
            auth.kept().as_deref(),
            Some("kept 2"),
            "signed in with still"
        );
        assert_eq!(
            kept_state(&harness).sign_in.as_deref(),
            Some("kept 2"),
            "kept"
        );
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

/// A session deleted here by hand, then put back by Undo while the sync
/// sending that deletion is under way, stays, though another device with
/// its clock an hour ahead deleted it and put it back before: the putting
/// back is stamped after the deletion that sync sent, and the drive says so
/// to every device. It used to be stamped by what sync knew as that sync
/// began, no later than the deletion the sync went on to send, and taken
/// away again by it at the next sync, here and everywhere.
#[test]
fn undo_while_its_deletion_is_sent_keeps_the_session_after_changes_ahead() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        // Deleted and put back on another device, its clock ahead.
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        let ahead = now_ms() + 3_600_000;
        let session = delete_in(&other, two, ahead);
        let settings = cw_core::TrainingSettings::default();
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &settings,
            ahead,
        ))
        .expect("synced");
        put_back_in(&other, &mut other_state, two, &session, ahead + 10);
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &settings,
            ahead + 10,
        ))
        .expect("synced");
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert_eq!(stamps(&harness), [1, 2]);

        // Deleted here, and put back while the deletion is being sent.
        harness.in_app(|| ctx.delete_sessions(&[two]));
        held.hold_writes(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| ctx.undo_delete());
        held.hold_writes(false);
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(stamps(&harness), [1, 2], "not deleted again");
        let (joined, joined_state) = another_device(&remote, "a long passphrase", &[]);
        let theirs: Vec<u64> = joined
            .load_history()
            .sessions
            .iter()
            .map(|(_, s)| s.timestamp)
            .collect();
        assert_eq!(theirs, [1, 2], "the drive's word");
        assert!(
            joined_state.drive.revives[&two] > joined_state.drive.tombs[&two],
            "put back after the deletion sent"
        );
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

/// The same, with the other device's deletion stamped an hour ahead of
/// this device's clock: the putting back, noted under the sync turned on
/// anew before it had read anything of the drive, is stamped again after
/// that sync's first read, as one noted knowing the drive would be, and
/// stands, here and in the drive. It used to keep the bare stamp of this
/// device's clock, earlier than the deletion, and was let go again.
#[test]
fn undo_while_a_sync_from_before_sync_was_turned_on_anew_ends_outlasts_a_deletion_ahead() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        // Deleted on another device, its clock ahead, and here.
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
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
        let (joined, _) = another_device(&remote, "a long passphrase", &[]);
        let theirs: Vec<u64> = joined
            .load_history()
            .sessions
            .iter()
            .map(|(_, s)| s.timestamp)
            .collect();
        assert_eq!(theirs, [1, 2, 5], "the drive's word");
    });
}

/// A run as it was kept, with its log or, as an older build kept it,
/// without: a restore of the one with its log fills in the other.
fn a_run(stamp: u64, log: bool) -> cw_core::SessionResult {
    let mut run = crate::testing::stored_run("2026-10-02", &[("K5XYZ", "214", "K5XYZ", "214")]);
    run.timestamp = stamp;
    if !log {
        run.contest = None;
    }
    run
}

/// A session a restore filled in, and the sync a moment after it synced,
/// stays where the restore's Undo puts it back as it was, here and on every
/// device: the record put back is noted as put back by hand, after the
/// deletion of it that sync sent. It used to be put back with no such note,
/// so the next sync let it go as the drive's deletion, and sent the
/// deletion of the filled record the Undo took out: the session was gone
/// everywhere.
#[test]
fn the_undo_of_a_session_filled_in_and_synced_keeps_it_everywhere() {
    run(|| async {
        let mut harness = Harness::new();
        let stamp = 1_790_000_000_000;
        harness.in_app(|| {
            harness
                .ctx
                .keep_finished(a_run(stamp, false), |saved| assert!(saved.kept().is_some()))
        });
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        assert_eq!(joined_stamps(&remote), [stamp], "synced");
        harness.in_app(|| {
            ctx.restore_backup(Backup {
                saved_at: 0,
                settings: None,
                sessions: vec![a_run(stamp, true)],
                left_out: 0,
            })
        });
        harness.pump();
        assert_eq!(
            harness.in_app(|| harness.toast()).as_deref(),
            Some("Filled in 1 session.")
        );
        // The sync a moment after the restore sends the filled record, and
        // the deletion of the one it took the place of.
        harness.advance(u64::from(SYNC_SOON_MS) + 1_000).await;
        let (joined, _) = another_device(&remote, "a long passphrase", &[]);
        let theirs = joined.load_sessions();
        assert_eq!(theirs.len(), 1);
        assert!(
            theirs[0].contest.is_some(),
            "the drive's word is the filled one"
        );
        // Undone while its message is up.
        harness.in_app(|| ctx.undo_restore());
        harness.pump();
        let here = harness.store.load_sessions();
        assert_eq!(here.len(), 1);
        assert!(here[0].contest.is_none(), "put back as it was");
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        harness.in_app(|| ctx.sync_now());
        harness.advance(1_000).await;
        assert_eq!(stored_stamps(&harness), [stamp], "kept here");
        assert_eq!(stamps(&harness), [stamp]);
        let (joined, _) = another_device(&remote, "a long passphrase", &[]);
        let theirs = joined.load_sessions();
        assert_eq!(theirs.len(), 1, "the drive's word");
        assert!(theirs[0].contest.is_none(), "as it was before the restore");
    });
}

/// A session put back by Undo while a sync waits on the drive, before it
/// lists the store, stays: the sync lets go of nothing put back since it
/// began, though it finds the session listed and its deletion, sent before,
/// in the drive, with no note of the putting back in what it plans from;
/// the sync after sends it as put back. That sync used to let it go again,
/// as the drive's deletion, and the next to forget its putting back, here
/// and everywhere.
#[test]
fn a_session_put_back_while_a_sync_reads_the_drive_is_not_let_go_by_it() {
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
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        let sent = harness.in_app(|| {
            ctx.sync
                .state
                .peek()
                .as_ref()
                .is_some_and(|state| state.drive.deleted(two))
        });
        assert!(sent, "the deletion sent");

        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| ctx.undo_delete());
        held.hold(false);
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(stamps(&harness), [1, 2], "not let go");
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

/// The same, with the session deleted and put back in another copy of the
/// app on the same store, a tab or a window, while this copy's sync waits
/// on the drive: the sync lets go of nothing put back since it began, in
/// whichever copy, and the next sync sends it as put back. The sync used to
/// let it go, as it read nothing of the other copy's putting back, here and
/// everywhere.
#[test]
fn a_session_put_back_in_another_copy_while_this_one_syncs_is_not_let_go_by_it() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.take_up_store());
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        harness.in_app(|| window.delete_sessions(&[two]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(kept_state(&harness).drive.deleted(two), "the deletion sent");

        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| window.undo_delete());
        held.hold(false);
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        harness.in_app(|| ctx.take_up_store());
        assert_eq!(stored_stamps(&harness), [1, 2], "not let go");
        assert_eq!(stamps(&harness), [1, 2]);
        assert_eq!(joined_stamps(&remote), [1, 2], "the drive's word");
    });
}

/// The same, with sync turned off here before the sync goes on, the Undo
/// made before the turning off or after it: the sync under way lets go of
/// nothing put back since it began, whatever became of sync meanwhile, and
/// the session stays here, though sync, off, sends nothing of it. That sync
/// used to let it go again, as the drive's deletion, the toast's Undo spent:
/// what it asked of was what sync noted, which the turning off forgot, or
/// which sync, off by then, never noted.
#[test]
fn a_session_put_back_while_a_sync_reads_the_drive_is_not_let_go_by_it_as_sync_is_turned_off() {
    for undo_first in [true, false] {
        run(|| async move {
            let case = if undo_first {
                "Undo, then sync turned off"
            } else {
                "sync turned off, then Undo"
            };
            let mut harness = Harness::new();
            keep(&harness, &[1, 2]);
            let remote = Rc::new(MemoryRemote::default());
            let auth = FakeAuth::giving(&["t"]);
            let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
            let two = harness
                .in_app(|| harness.ctx.sessions.peek().id_at(1))
                .expect("two");
            harness.in_app(|| ctx.delete_sessions(&[two]));
            harness.in_app(|| ctx.sync_now());
            harness.pump();
            let sent = harness.in_app(|| {
                ctx.sync
                    .state
                    .peek()
                    .as_ref()
                    .is_some_and(|state| state.drive.deleted(two))
            });
            assert!(sent, "the deletion sent: {case}");

            held.hold(true);
            harness.in_app(|| ctx.sync_now());
            harness.pump();
            if undo_first {
                harness.in_app(|| ctx.undo_delete());
                harness.in_app(|| ctx.sync_turn_off(false));
            } else {
                harness.in_app(|| ctx.sync_turn_off(false));
                harness.in_app(|| ctx.undo_delete());
            }
            held.hold(false);
            harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
            assert_eq!(stamps(&harness), [1, 2], "not let go: {case}");
            assert_eq!(
                status(&harness),
                SyncStatus::Off { problem: None },
                "{case}"
            );
        });
    }
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

/// What sync keeps is kept as a sync sends a deletion by hand, before the
/// drive answers, the deletion noted against its pack: an app stopped
/// before the sync ends, a tab closed say, opens knowing of the pack, which
/// the drive may have made ([`SyncState::unanswered`]). The sign-in kept is
/// the app's, one made meanwhile among it. Answered, the deletion is the
/// drive's as the sync ends. It used to be kept only then, and an app
/// stopped before opened knowing nothing of the pack.
#[test]
fn a_deletion_by_hand_is_kept_as_its_pack_is_sent() {
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
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        held.hold_writes(true);
        held.hold(false);
        harness.advance(100).await;
        let kept = |harness: &Harness| {
            let raw = harness.store.sync.borrow().clone().expect("kept");
            SyncState::read(&raw).expect("reads")
        };
        let sending = kept(&harness);
        let unanswered: Vec<RecordId> = sending
            .unanswered
            .values()
            .flat_map(|deletions| deletions.keys().copied())
            .collect();
        assert_eq!(unanswered, [two], "kept as its pack goes");
        assert_eq!(
            sending.sign_in.as_deref(),
            Some("kept 2"),
            "the app's sign-in"
        );
        held.hold_writes(false);
        harness.advance(100).await;
        let answered = kept(&harness);
        assert!(answered.unanswered.is_empty(), "answered");
        assert!(answered.applied.contains_key(&two), "the drive's");
        assert_eq!(answered.sign_in.as_deref(), Some("kept 2"));
    });
}

/// A sync that goes on to send a deletion by hand once sync was turned off
/// keeps nothing as it sends it: not over the sync turned on anew here
/// since, nor where another copy of the app turned sync off, which stays
/// off in the store. Kept, it would have turned sync off here, as turned
/// on anew elsewhere, or back on in the store for every copy.
#[test]
fn a_sync_sending_a_deletion_once_sync_was_turned_off_keeps_nothing() {
    for anew_here in [true, false] {
        run(|| async move {
            let case = if anew_here {
                "turned off and on anew here"
            } else {
                "turned off in another copy"
            };
            let mut harness = Harness::new();
            keep(&harness, &[1, 2]);
            let remote = Rc::new(MemoryRemote::default());
            let auth = FakeAuth::giving(&["t"]);
            let ctx = turned_on(&mut harness, &remote, &auth).await;
            let two = harness
                .in_app(|| harness.ctx.sessions.peek().id_at(1))
                .expect("two");
            harness.in_app(|| ctx.delete_sessions(&[two]));
            // The sync waits on the store as it plans, before it sends.
            harness.store.forgotten_later.set(true);
            harness.in_app(|| ctx.sync_now());
            harness.pump();
            if anew_here {
                harness.in_app(|| ctx.sync_turn_off(false));
                harness.in_app(|| ctx.sync_sign_in());
                harness.pump();
                harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), None));
                harness.advance(100).await;
                assert!(status(&harness).is_on(), "{case}: {:?}", status(&harness));
            } else {
                let window = another_copy(&harness, &remote);
                harness.in_app(|| window.sync_turn_off(false));
            }
            let kept = kept_device(&harness);
            harness.store.forgotten_later.set(false);
            harness.store.answer_forgotten();
            harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
            assert_eq!(kept_device(&harness), kept, "{case}");
            assert_eq!(status(&harness).is_on(), anew_here, "{case}");
        });
    }
}

/// A session put back by Undo in another copy of the app on the same store,
/// a tab or a window, while this copy's sync sends its pack stays, here and
/// in the drive: the sync, as it ends, lays what it did over what the other
/// copy kept meanwhile, and this copy's next sync sends it as put back. So
/// it does in a browser without Web Locks, where nothing locks. The sync
/// used to keep this copy's whole state over the other's as it ended, the
/// note of the putting back lost with it, and the next sync let the session
/// go again, here and everywhere.
#[test]
fn a_session_put_back_in_another_copy_outlasts_this_copys_sync() {
    for unlockable in [false, true] {
        run(|| async move {
            let mut harness = Harness::new();
            harness.store.unlockable.set(unlockable);
            keep(&harness, &[1, 2]);
            let remote = Rc::new(MemoryRemote::default());
            let auth = FakeAuth::giving(&["t"]);
            let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
            let window = another_copy(&harness, &remote);
            harness.in_app(|| window.take_up_store());
            let two = harness
                .in_app(|| harness.ctx.sessions.peek().id_at(1))
                .expect("two");
            harness.in_app(|| window.delete_sessions(&[two]));
            harness.in_app(|| ctx.sync_now());
            harness.pump();
            assert!(kept_state(&harness).drive.deleted(two), "the deletion sent");

            keep(&harness, &[5]);
            held.hold_writes(true);
            harness.in_app(|| ctx.sync_now());
            harness.pump();
            harness.in_app(|| window.undo_delete());
            held.hold_writes(false);
            harness.advance(100).await;
            harness.in_app(|| ctx.sync_now());
            harness.advance(100).await;
            assert_eq!(stored_stamps(&harness), [1, 2, 5], "not let go");
            assert_eq!(stamps(&harness), [1, 2, 5]);
            assert_eq!(joined_stamps(&remote), [1, 2, 5], "the drive's word");
        });
    }
}

/// A setting changed on another device after another copy of the app on
/// the same store synced a change of its own is taken here: this copy's
/// sync begins from what that copy kept, the settings as that sync left
/// them among it, and so takes that copy's change for no change of its own.
/// It used to begin from what it read as it opened, take the other copy's
/// change, stored since, for one made here, and send it over the later one.
#[test]
fn a_setting_changed_on_another_device_after_another_copy_synced_is_taken() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| {
            let mut settings = ctx.settings;
            settings.write().curriculum.num_groups = 9;
            ctx.store_settings();
        });
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        let synced_here = kept_state(&harness).stamps["numGroups"];

        // Another device joins, and changes it after.
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let mut theirs = harness.in_app(|| ctx.settings.peek().clone().clamp());
        theirs.curriculum.num_groups = 4;
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            synced_here + 1,
        ))
        .expect("synced");

        // The other copy takes up what was stored here, and syncs later by
        // the clock than either.
        harness.in_app(|| window.take_up_store());
        std::thread::sleep(std::time::Duration::from_millis(5));
        harness.in_app(|| window.sync_now());
        harness.pump();
        let there = harness.in_app(|| window.settings.peek().curriculum.num_groups);
        assert_eq!(there, 4, "the other device's, the later");
        assert_eq!(kept_state(&harness).stamps["numGroups"], synced_here + 1);
    });
}

/// A copy of the app on the same store, a tab or a window, that has not
/// looked at the store since another changed a setting, stored it and
/// synced it, takes that change as its own sync begins: the sync begins
/// from the settings the store holds, as it begins from what the other copy
/// kept for sync, the settings as that copy's sync left them among it. It
/// used to begin from the settings this copy held, older than those, take
/// them for a change made now, and send them stamped after the other
/// copy's, which undid that change everywhere, the two copies undoing each
/// other's at every sync.
#[test]
fn a_copy_behind_on_the_settings_does_not_send_them_back() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let held = harness.in_app(|| ctx.settings.peek().clone());
        harness.store.save_settings(&held);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        // Another window, opened as the app opens: on the stored settings.
        let opened = harness.in_app(|| {
            AppCtx::new(
                harness.app.clone(),
                harness.store.load_settings(),
                Default::default(),
            )
        });
        let window = harness.in_app(|| {
            opened.link_sync(Link {
                auth: FakeAuth::giving(&["t"]) as Rc<dyn Auth>,
                remote: Rc::clone(&remote) as Rc<dyn Remote>,
            })
        });
        let before = harness.in_app(|| window.settings.peek().curriculum.num_groups);
        harness.in_app(|| {
            let mut settings = ctx.settings;
            settings.write().curriculum.num_groups = before + 7;
            ctx.store_settings();
        });
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        // Stamped by the clock: the window's sync is the later.
        std::thread::sleep(std::time::Duration::from_millis(5));
        // The window syncs, not having looked at the store since.
        harness.in_app(|| window.sync_now());
        harness.pump();
        let there = harness.in_app(|| window.settings.peek().curriculum.num_groups);
        assert_eq!(
            there,
            before + 7,
            "the change made in the other window, taken"
        );
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        let here = harness.in_app(|| ctx.settings.peek().curriculum.num_groups);
        assert_eq!(here, before + 7, "not taken back here");
    });
}

/// A sign-in made in another copy of the app on the same store while this
/// copy's sync is under way is the one kept: the sync, as it ends, keeps
/// the sign-in the store holds, as it changed none. It used to keep this
/// copy's own over it.
#[test]
fn a_sign_in_made_in_another_copy_is_kept_by_this_copys_sync() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let window_auth = FakeAuth::giving(&["u"]);
        // Its next sign-in keeps a token of its own.
        window_auth.asked.set(4);
        let window = another_copy_through(
            &harness,
            Rc::clone(&remote) as Rc<dyn Remote>,
            Rc::clone(&window_auth) as Rc<dyn Auth>,
        );
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| window.sync_sign_in());
        harness.pump();
        assert_eq!(kept_state(&harness).sign_in.as_deref(), Some("kept 5"));
        held.hold(false);
        harness.advance(100).await;
        assert_eq!(
            kept_state(&harness).sign_in.as_deref(),
            Some("kept 5"),
            "the other copy's"
        );
    });
}

/// A sign-in another copy of the app on the same store made while a sync
/// here was under way, taken up here, then made again there, is the later
/// one kept as that sync ends, and the one both copies sign in with: the
/// one taken up here is no sign-in made here. The sync's end used to take
/// any sign-in here that differed from the store's as the sync began for
/// one made here, and kept the one taken up over the later.
#[test]
fn a_sign_in_made_again_in_another_copy_while_this_one_syncs_is_kept() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let held = Held::of(&remote);
        let window_auth = FakeAuth::giving(&["t"]);
        let window = another_copy_through(
            &harness,
            Rc::clone(&held) as Rc<dyn Remote>,
            Rc::clone(&window_auth) as Rc<dyn Auth>,
        );
        harness.in_app(|| window.take_up_store());
        held.hold(true);
        harness.in_app(|| window.sync_now());
        harness.pump();
        // Signed in again here, then taken up there as it comes into
        // sight, then signed in again here.
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        let first = auth.kept();
        harness.in_app(|| window.sync_if_stale());
        assert_eq!(window_auth.kept(), first, "taken up there");
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        let later = auth.kept();
        assert_ne!(later, first);
        assert_eq!(kept_state(&harness).sign_in, later, "kept");
        held.hold(false);
        harness.advance(100).await;
        assert_eq!(kept_state(&harness).sign_in, later, "the later, as it ends");
        harness.in_app(|| window.sync_if_stale());
        assert_eq!(window_auth.kept(), later, "signed in with there");
        harness.in_app(|| ctx.sync_if_stale());
        assert_eq!(auth.kept(), later, "and here");
    });
}

/// The record of the session that finished at `stamp`, as `ctx`, a copy of
/// the app, holds it.
fn record_of(harness: &Harness, ctx: AppCtx, stamp: u64) -> RecordId {
    harness.in_app(|| {
        let history = ctx.sessions.peek();
        let at = history
            .iter()
            .position(|session| session.timestamp == stamp)
            .expect("held");
        history.id_at(at).expect("a record")
    })
}

/// A session deleted here and sent, its deletion noted as the drive's, then
/// put back by Undo; and another copy of the app on the same store, a tab or
/// a window, that read what sync keeps meanwhile, while the sync sending the
/// putting back was under way: a later deletion of that session by hand
/// stands, here and on every device. The sync kept its state as it sent a
/// deletion by hand, before the putting back was in the drive, with the
/// deletion's note dropped; the other copy kept the note, on no evidence
/// that it was spent, took it from then on for one of its own, and wrote it
/// back after the putting back was sent. The next sync then took the later
/// deletion by hand for the drive's, put back since, and took the session
/// back: the deletion was undone everywhere.
///
/// With `meanwhile`, the other copy keeps something of its own while that
/// sync is still under way, laying the note back as it stood then: the
/// sync's end lets it go, once the putting back it sent is in the drive.
fn a_deletion_noted_as_the_drives_is_not_laid_back_once_put_back(tabs: bool, meanwhile: bool) {
    run(|| async move {
        let mut harness = Harness::new();
        harness.store.browser.set(tabs);
        harness.store.hears_news.set(tabs);
        keep(&harness, &[1, 2, 3, 4]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let other = another_copy(&harness, &remote);
        harness.in_app(|| other.take_up_store());
        let [two, three, four] = [2, 3, 4].map(|stamp| record_of(&harness, ctx, stamp));
        let take_up = |harness: &mut Harness| {
            if tabs {
                harness.store.tell_sync_news();
                harness.pump();
            } else {
                harness.in_app(|| other.sync_if_stale());
            }
        };
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.in_app(|| ctx.sync_now());
        harness.advance(1_000).await;
        assert!(kept_state(&harness).applied.contains_key(&two), "noted");
        take_up(&mut harness);
        // Put back, and 3 deleted by hand, so that the next sync keeps what
        // it has come to as it sends that deletion.
        harness.in_app(|| ctx.undo_delete());
        std::thread::sleep(std::time::Duration::from_millis(3));
        harness.in_app(|| ctx.delete_sessions(&[three]));
        harness.pump();
        held.hold_writes(true);
        harness.in_app(|| ctx.sync_now());
        harness.advance(100).await;
        take_up(&mut harness);
        // The other copy keeps something of its own, then or after.
        let keep_its_own = |harness: &mut Harness| {
            harness.in_app(|| other.take_up_store());
            harness.in_app(|| other.delete_sessions(&[four]));
            harness.in_app(|| other.undo_delete());
            harness.pump();
        };
        if meanwhile {
            keep_its_own(&mut harness);
        }
        held.hold_writes(false);
        harness.advance(1_000).await;
        if !meanwhile {
            keep_its_own(&mut harness);
        }
        assert!(
            !kept_state(&harness).applied.contains_key(&two),
            "the deletion's note, spent, laid back"
        );
        // 2 deleted by hand again, the last word on it.
        std::thread::sleep(std::time::Duration::from_millis(3));
        harness.in_app(|| ctx.take_up_store());
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.advance(u64::from(SYNC_SOON_MS) * 3).await;
        for copy in [ctx, other, ctx] {
            harness.in_app(|| copy.sync_now());
            harness.advance(1_000).await;
        }
        assert_eq!(stored_stamps(&harness), [1, 4], "2 and 3 deleted");
        assert_eq!(joined_stamps(&remote), [1, 4], "the drive's word");
    });
}

#[test]
fn a_deletion_noted_as_the_drives_is_not_laid_back_once_put_back_by_a_window() {
    a_deletion_noted_as_the_drives_is_not_laid_back_once_put_back(false, false);
}

#[test]
fn a_deletion_noted_as_the_drives_is_not_laid_back_once_put_back_by_a_tab() {
    a_deletion_noted_as_the_drives_is_not_laid_back_once_put_back(true, false);
}

#[test]
fn a_deletion_noted_as_the_drives_kept_while_its_putting_back_is_sent_goes_once_sent() {
    a_deletion_noted_as_the_drives_is_not_laid_back_once_put_back(false, true);
}

/// A deletion by hand another copy of the app on the same store sent is
/// noted as the drive's in what this copy keeps as well, though this copy
/// read what sync keeps before it was sent: a putting back of that session
/// made on another device after it is taken back here as this copy syncs.
/// This copy used to write its whole state over the note as it noted a
/// putting back of its own, and its sync, finding the session deleted here
/// with no note of the drive holding the deletion, sent it again, deleting
/// everywhere what the other device put back.
#[test]
fn a_deletion_another_copy_sent_is_kept_by_this_copys_write() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2, 3]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.take_up_store());
        let (two, three) = harness.in_app(|| {
            let history = harness.ctx.sessions.peek();
            (
                history.id_at(1).expect("two"),
                history.id_at(2).expect("three"),
            )
        });
        let session = harness.store.load_sessions()[2].clone();
        // Deleted here, the other copy's own deletion undone there.
        harness.in_app(|| window.delete_sessions(&[two]));
        harness.in_app(|| ctx.delete_sessions(&[three]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(kept_state(&harness).applied.contains_key(&three), "sent");
        harness.in_app(|| window.undo_delete());
        assert!(
            kept_state(&harness).applied.contains_key(&three),
            "still noted"
        );

        // Put back on another device, after.
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let later = now_ms() + 1_000;
        put_back_in(&other, &mut other_state, three, &session, later);
        let settings = cw_core::TrainingSettings::default();
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &settings,
            later,
        ))
        .expect("synced");

        harness.in_app(|| window.sync_now());
        harness.pump();
        assert_eq!(stored_stamps(&harness), [1, 2, 3], "taken back");
        assert_eq!(joined_stamps(&remote), [1, 2, 3], "the drive's word");
    });
}

/// Two copies of the app open at once on one store, a tab and another, or
/// two windows, each deleting sessions, putting its own back by Undo, and
/// syncing in turn, lose none of each other's deletions or puttings back:
/// each copy's write lays what it did over what the other kept, and each
/// sync begins from what the store holds, so every putting back is sent,
/// whichever copy syncs, and every deletion stays. Each copy used to write
/// its whole state over the other's: one copy's sync, with no note of the
/// putting back the other made, let the session go again, here and
/// everywhere.
#[test]
fn two_copies_open_at_once_lose_none_of_each_others_deletions_or_puttings_back() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2, 3, 4]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.take_up_store());
        let ids = harness.in_app(|| harness.ctx.sessions.peek().ids().to_vec());
        // Each deletes one, and this one's sync sends both deletions.
        harness.in_app(|| window.delete_sessions(&[ids[1]]));
        harness.in_app(|| ctx.delete_sessions(&[ids[2]]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        // Each puts its own back; the other syncs, then deletes another,
        // which this one's sync sends.
        harness.in_app(|| window.undo_delete());
        harness.in_app(|| ctx.undo_delete());
        harness.in_app(|| window.sync_now());
        harness.pump();
        harness.in_app(|| window.take_up_store());
        harness.in_app(|| window.delete_sessions(&[ids[3]]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| ctx.take_up_store());
        assert_eq!(
            stored_stamps(&harness),
            [1, 2, 3],
            "each copy's putting back kept, and the deletion"
        );
        assert_eq!(stamps(&harness), [1, 2, 3]);
        assert_eq!(joined_stamps(&remote), [1, 2, 3], "the drive's word");
    });
}

/// A sync asked for while another copy of the app on the same store, a tab
/// or a window, syncs waits for that one to end, showing "Syncing..."
/// meanwhile, and then begins from what it kept: it sends nothing that one
/// sent. The two used to sync at once, each sending what the other sent.
#[test]
fn a_sync_waits_for_another_copys_and_starts_from_what_it_kept() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.take_up_store());
        keep(&harness, &[5]);
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        let (lists, sent) = (remote.lists.get(), packs(&remote));
        harness.in_app(|| window.sync_now());
        harness.advance(2_000).await;
        assert_eq!(remote.lists.get(), lists, "waits for this copy's sync");
        let there = harness.in_app(|| window.sync.status.peek().clone());
        assert!(
            matches!(there, SyncStatus::On { syncing: true, .. }),
            "{there:?}"
        );
        held.hold(false);
        harness.advance(500).await;
        assert_eq!(
            remote.lists.get(),
            lists + 2,
            "this copy's, then the other's"
        );
        assert_eq!(packs(&remote), sent + 1, "the other sends nothing more");
        let there = harness.in_app(|| window.sync.status.peek().clone());
        assert!(
            matches!(
                there,
                SyncStatus::On {
                    syncing: false,
                    problem: None,
                    ..
                }
            ),
            "{there:?}"
        );
    });
}

/// A sync that waits for another copy's to end does not start once the
/// synced copy is being deleted here meanwhile, as none asked for then
/// does: the deletion goes once the other copy's sync is over, and syncs
/// nothing first. The waiting sync used to go on to sync the drive just
/// before it was deleted.
#[test]
fn a_sync_waiting_for_another_copys_does_not_start_once_the_copy_is_being_deleted() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        let lists = remote.lists.get();
        harness.in_app(|| window.sync_now());
        harness.advance(1_000).await;
        harness.in_app(|| window.sync_turn_off(true));
        held.hold(false);
        harness.advance(2_000).await;
        assert!(remote.names().is_empty(), "{:?}", remote.names());
        assert_eq!(
            remote.lists.get(),
            lists + 2,
            "this copy's sync, and the deletion: the waiting sync never started"
        );
        let there = harness.in_app(|| window.sync.status.peek().clone());
        assert_eq!(there, SyncStatus::Off { problem: None });
    });
}

/// A sync waits ten minutes at most for another copy's to end, showing
/// "Syncing..." meanwhile, and then goes on without it, as copies did before
/// they waited for each other: a copy hung, or stopped while the system
/// still holds its lock, keeps none from syncing for long.
#[test]
fn a_sync_waits_ten_minutes_at_most_for_another_copys() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let hung = harness.store.hold_lock(Lock::SyncRun);
        let lists = remote.lists.get();
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        leap(&mut harness, 9 * 60_000).await;
        assert_eq!(remote.lists.get(), lists, "waits");
        assert!(
            matches!(status(&harness), SyncStatus::On { syncing: true, .. }),
            "{:?}",
            status(&harness)
        );
        leap(&mut harness, 2 * 60_000).await;
        assert_eq!(remote.lists.get(), lists + 1, "goes on without it");
        assert!(
            matches!(
                status(&harness),
                SyncStatus::On {
                    syncing: false,
                    problem: None,
                    ..
                }
            ),
            "{:?}",
            status(&harness)
        );
        drop(hung);
    });
}

/// Turning sync off with the synced copy, in a copy of the app whose
/// window or tab is not the one syncing, waits for the other copy's sync to
/// end before it deletes anything, then deletes every file, the pack that
/// sync sent among them; and the other copy finds sync off as it next
/// syncs. The copy used to be deleted while the other copy's sync went on
/// writing into the drive, which kept the pack it sent after.
#[test]
fn a_copy_deleting_the_synced_copy_waits_for_another_copys_sync() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        keep(&harness, &[5]);
        held.hold_writes(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        let names = remote.names();
        harness.in_app(|| window.sync_turn_off(true));
        harness.advance(2_000).await;
        assert_eq!(remote.names(), names, "nothing deleted while it syncs");
        held.hold_writes(false);
        harness.advance(500).await;
        assert!(remote.names().is_empty(), "{:?}", remote.names());
        let there = harness.in_app(|| window.sync.status.peek().clone());
        assert_eq!(there, SyncStatus::Off { problem: None });
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(
            matches!(&status(&harness), SyncStatus::Off { problem: Some(p) } if p == "Sync was turned off in another tab or window of Dust."),
            "{:?}",
            status(&harness)
        );
    });
}

/// A copy of the app reads what sync keeps, lays what it did over it and
/// writes it back while no other copy does, on a computer or a phone, each
/// holding the lock on what sync keeps as it does: a putting back noted here
/// while another window lays its own over what sync keeps waits for that one
/// to finish, and is laid over what it wrote. It used to be written at once,
/// and the other window's write, from what it read before, lost it.
#[test]
fn what_sync_keeps_is_laid_over_by_one_copy_at_a_time() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2, 3]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let (two, three) = harness.in_app(|| {
            let history = harness.ctx.sessions.peek();
            (
                history.id_at(1).expect("two"),
                history.id_at(2).expect("three"),
            )
        });
        harness.in_app(|| ctx.delete_sessions(&[two]));
        // Another window lays a putting back of its own over what sync
        // keeps, under the lock.
        let window = harness.store.hold_lock(Lock::SyncState);
        let mut theirs = kept_state(&harness);
        harness.in_app(|| ctx.undo_delete());
        theirs.revived.insert(three, now_ms());
        *harness.store.sync.borrow_mut() = theirs.written();
        drop(window);
        harness.advance(100).await;
        let kept = kept_state(&harness);
        assert!(kept.revived.contains_key(&two), "this copy's, laid over");
        assert!(kept.revived.contains_key(&three), "the other window's");
    });
}

/// In a browser a tab takes no lock on what sync keeps: a putting back is
/// kept as it is noted, as `localStorage` is read and written then and
/// there, and a Web Lock is granted only a moment after it is asked for, so
/// that one noted as the tab closes would be lost. It takes the lock on
/// syncing all the same.
#[test]
fn a_tab_keeps_what_sync_keeps_as_it_is_noted() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        harness.store.browser.set(true);
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        harness.in_app(|| ctx.delete_sessions(&[two]));
        let _held = harness.store.hold_lock(Lock::SyncState);
        harness.in_app(|| ctx.undo_delete());
        assert!(
            kept_state(&harness).revived.contains_key(&two),
            "kept at once"
        );
        let other_tab = harness.store.hold_lock(Lock::SyncRun);
        let lists = remote.lists.get();
        harness.in_app(|| ctx.sync_now());
        harness.advance(1_000).await;
        assert_eq!(remote.lists.get(), lists, "waits for the other tab's sync");
        drop(other_tab);
        harness.advance(500).await;
        assert_eq!(remote.lists.get(), lists + 1);
    });
}

/// Sync turned off in another tab of the app is off in this one as soon as
/// the browser tells this one of it, as it tells every other tab of a write
/// to `localStorage`, saying why: nothing is synced meanwhile. It used to
/// stay on here until this tab next synced or kept what sync keeps.
#[test]
fn sync_turned_off_in_another_tab_is_taken_up_as_it_is_told_of() {
    run(|| async {
        let mut harness = Harness::new();
        harness.store.hears_news.set(true);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let tab = another_copy(&harness, &remote);
        let lists = remote.lists.get();
        harness.in_app(|| ctx.sync_turn_off(false));
        harness.store.tell_sync_news();
        harness.pump();
        let there = harness.in_app(|| tab.sync.status.peek().clone());
        assert!(
            matches!(&there, SyncStatus::Off { problem: Some(p) } if p == "Sync was turned off in another tab or window of Dust."),
            "{there:?}"
        );
        assert_eq!(remote.lists.get(), lists, "nothing synced");
    });
}

/// Sync turned off in another window of the app is off in this one as it
/// comes back into sight, saying why, on every platform, though nothing
/// told it: it looks at what the store keeps for sync first, and syncs
/// nothing. It used to stay on until it next synced or kept what sync keeps.
#[test]
fn sync_turned_off_in_another_window_is_taken_up_as_this_one_comes_into_sight() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        let lists = remote.lists.get();
        harness.in_app(|| ctx.sync_turn_off(false));
        harness.in_app(|| window.sync_if_stale());
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        let there = harness.in_app(|| window.sync.status.peek().clone());
        assert!(
            matches!(&there, SyncStatus::Off { problem: Some(p) } if p == "Sync was turned off in another tab or window of Dust."),
            "{there:?}"
        );
        assert_eq!(remote.lists.get(), lists, "nothing synced");
    });
}

/// A sync another copy of the app made is this copy's last sync too: the
/// card shows it, and this copy, coming back into sight, does not sync
/// again so soon after it. It used to go by its own last sync alone, and
/// sync again just after the other copy had.
#[test]
fn a_sync_made_in_another_copy_is_this_copys_last() {
    run(|| async {
        let mut harness = Harness::new();
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let window = another_copy(&harness, &remote);
        // As far as the window knows, it synced long ago.
        harness.in_app(|| {
            let mut state = window.sync.state;
            if let Some(state) = state.write().as_mut() {
                state.last = Some(now_ms() - 10 * 60_000);
            }
        });
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        let synced = kept_state(&harness).last;
        let lists = remote.lists.get();
        harness.in_app(|| window.sync_if_stale());
        harness.advance(u64::from(SYNC_SOON_MS) * 2).await;
        assert_eq!(remote.lists.get(), lists, "synced a moment ago, there");
        let there = harness.in_app(|| window.sync.status.peek().clone());
        assert!(
            matches!(there, SyncStatus::On { last, .. } if last == synced),
            "{there:?}"
        );
    });
}

/// A sign-in made in another copy of the app, a refresh token got there
/// after the one kept here was turned down, say, is the one this copy signs
/// in with as soon as it takes up what that copy kept. It used to go on with
/// its own until the app opened again.
#[test]
fn a_sign_in_another_copy_made_is_this_copys_too() {
    run(|| async {
        let mut harness = Harness::new();
        harness.store.hears_news.set(true);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let tab_auth = FakeAuth::giving(&["u"]);
        tab_auth.asked.set(4);
        let tab = another_copy_through(
            &harness,
            Rc::clone(&remote) as Rc<dyn Remote>,
            Rc::clone(&tab_auth) as Rc<dyn Auth>,
        );
        harness.in_app(|| tab.sync_sign_in());
        harness.pump();
        harness.store.tell_sync_news();
        harness.pump();
        assert_eq!(auth.kept().as_deref(), Some("kept 5"), "signed in as there");
        let here = harness.in_app(|| {
            ctx.sync
                .state
                .peek()
                .as_ref()
                .and_then(|state| state.sign_in.clone())
        });
        assert_eq!(here.as_deref(), Some("kept 5"));
    });
}

/// A session put back by Undo just as this copy of the app finds sync
/// turned off in another, a tab or a window, is put back all the same once
/// sync is turned on here again: the note of it waits with this copy, and
/// the first sync sends it as put back, after the drive's deletion, here
/// and everywhere. The note used to go as sync was taken for off, and that
/// first sync let the session go again.
#[test]
fn a_session_put_back_as_sync_is_turned_off_in_another_copy_is_kept_when_turned_on_here() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(kept_state(&harness).drive.deleted(two), "the deletion sent");

        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.sync_turn_off(false));
        harness.in_app(|| ctx.undo_delete());
        assert!(
            matches!(&status(&harness), SyncStatus::Off { problem: Some(p) } if p == "Sync was turned off in another tab or window of Dust."),
            "{:?}",
            status(&harness)
        );
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        assert_eq!(stamps(&harness), [1, 2], "not let go");
        assert_eq!(joined_stamps(&remote), [1, 2], "the drive's word");
    });
}

/// A session put back by Undo just as this copy of the app finds sync turned
/// on anew in another, a tab or a window, goes with the sync turned on
/// there: the history is the device's, and so the putting back stands
/// against whichever sync the store keeps. That copy's next sync sends it as
/// put back. The note used to go with this copy's sync, and that copy's next
/// sync let the session go again, here and everywhere.
#[test]
fn a_session_put_back_as_sync_is_turned_on_anew_in_another_copy_goes_with_that_sync() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(kept_state(&harness).drive.deleted(two), "the deletion sent");

        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.sync_turn_off(false));
        harness.in_app(|| window.sync_sign_in());
        harness.pump();
        harness.in_app(|| window.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(harness.in_app(|| window.sync.status.peek().is_on()));
        harness.in_app(|| ctx.undo_delete());
        assert!(
            matches!(&status(&harness), SyncStatus::Off { problem: Some(p) } if p == "Sync was turned on anew in another tab or window of Dust. Open Dust again here to sync."),
            "{:?}",
            status(&harness)
        );
        harness.in_app(|| window.sync_now());
        harness.pump();
        harness.in_app(|| window.take_up_store());
        let there: Vec<u64> = harness.in_app(|| {
            window
                .sessions
                .peek()
                .iter()
                .map(|session| session.timestamp)
                .collect()
        });
        assert_eq!(there, [1, 2], "not let go");
        assert_eq!(stored_stamps(&harness), [1, 2]);
        assert_eq!(joined_stamps(&remote), [1, 2], "the drive's word");
    });
}

/// A putting back this copy's sync sent, as that sync ends finding sync
/// turned off in another copy of the app, is no note to keep: once sync is
/// on here again, a deletion of that session another device made after it
/// stands, here and everywhere. The take-up used to keep the note as this
/// copy held it before the sync, which sent it, and the first sync turned on
/// here sent it again, stamped after every deletion of it the drive held.
#[test]
fn a_putting_back_sent_as_sync_is_turned_off_in_another_copy_is_not_sent_again() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let ctx = turned_on(&mut harness, &remote, &auth).await;
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(kept_state(&harness).drive.deleted(two), "the deletion sent");
        harness.in_app(|| ctx.undo_delete());
        assert!(kept_state(&harness).revived.contains_key(&two), "noted");

        // This copy's sync sends the putting back; as it ends, another
        // window, holding the lock on what sync keeps, has turned sync off.
        let window = harness.store.hold_lock(Lock::SyncState);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert_eq!(joined_stamps(&remote), [1, 2], "the putting back sent");
        harness.store.save_sync(None).expect("cleared");
        drop(window);
        harness.advance(100).await;
        assert!(
            matches!(&status(&harness), SyncStatus::Off { problem: Some(p) } if p == "Sync was turned off in another tab or window of Dust."),
            "{:?}",
            status(&harness)
        );

        // Another device deletes it a minute later.
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let later = now_ms() + 60_000;
        delete_in(&other, two, later);
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &cw_core::TrainingSettings::default(),
            later,
        ))
        .expect("synced");
        assert_eq!(joined_stamps(&remote), [1], "deleted there, after");

        // Sync turned on here again, joining the same drive.
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        assert_eq!(joined_stamps(&remote), [1], "the later deletion stands");
        assert_eq!(stamps(&harness), [1], "let go here");
    });
}

/// A putting back that a sync took in as it began, while the keep of its
/// note waited for the lock on what sync keeps, is kept for the next sync
/// turned on here, though sync is taken for off, mid-way through that sync,
/// as turned off in another copy of the app: the sync hands back the notes
/// it took in and did not send. Only what this copy held before the sync
/// used to be kept, and the note, which the sync had taken from those
/// waiting, went with it: the first sync turned on here let the session go
/// again, here and everywhere.
#[test]
fn a_putting_back_a_sync_took_in_is_kept_as_sync_turned_off_in_another_copy_is_taken_up_mid_way() {
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
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(kept_state(&harness).drive.deleted(two), "the deletion sent");

        // Put back while another window holds the lock on what sync keeps:
        // the keep waits, and the sync asked for then takes the note in.
        let window = harness.store.hold_lock(Lock::SyncState);
        harness.in_app(|| ctx.undo_delete());
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        // That window turns sync off; this one comes back into sight.
        harness.store.save_sync(None).expect("cleared");
        harness.in_app(|| ctx.sync_if_stale());
        drop(window);
        held.hold(false);
        harness.advance(5_000).await;
        assert!(
            matches!(&status(&harness), SyncStatus::Off { problem: Some(p) } if p == "Sync was turned off in another tab or window of Dust."),
            "{:?}",
            status(&harness)
        );
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        assert_eq!(stamps(&harness), [1, 2], "not let go");
        assert_eq!(joined_stamps(&remote), [1, 2], "the drive's word");
    });
}

/// A putting back that a sync took in as it began goes with the sync turned
/// on anew in another copy of the app, a tab or a window, though this copy
/// takes that up mid-way through its sync, as one put back just as it takes
/// it up does: the sync, ending, hands the note it did not send to the sync
/// the store keeps, whose next sync sends it as put back.
#[test]
fn a_putting_back_a_sync_took_in_goes_with_sync_turned_on_anew_in_another_copy_mid_way() {
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
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(kept_state(&harness).drive.deleted(two), "the deletion sent");
        harness.in_app(|| ctx.undo_delete());
        assert!(kept_state(&harness).revived.contains_key(&two), "noted");

        // This copy's sync takes the note in, and waits on the drive.
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        // Another window turns sync off and on anew, joining the same
        // drive: its sync waits for this copy's.
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.sync_turn_off(false));
        harness.in_app(|| window.sync_sign_in());
        harness.pump();
        harness.in_app(|| window.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(harness.in_app(|| window.sync.status.peek().is_on()));
        // This one comes back into sight, and finds it so.
        harness.in_app(|| ctx.sync_if_stale());
        assert!(
            matches!(&status(&harness), SyncStatus::Off { problem: Some(p) } if p == "Sync was turned on anew in another tab or window of Dust. Open Dust again here to sync."),
            "{:?}",
            status(&harness)
        );
        held.hold(false);
        harness.advance(u64::from(SYNC_SOON_MS) * 3).await;
        harness.in_app(|| window.take_up_store());
        let there: Vec<u64> = harness.in_app(|| {
            window
                .sessions
                .peek()
                .iter()
                .map(|session| session.timestamp)
                .collect()
        });
        assert_eq!(there, [1, 2], "not let go");
        assert_eq!(stored_stamps(&harness), [1, 2]);
        assert_eq!(joined_stamps(&remote), [1, 2], "the drive's word");
    });
}

/// A putting back that a sync took in as it began is forgotten as sync is
/// turned off here, though that sync, waiting on the drive all the while,
/// ends only once sync was taken for off from another copy of the app, then
/// turned on again here and off: turning sync off here forgets what it
/// noted, and the sync under way hands back none of what it took in.
#[test]
fn a_putting_back_a_sync_took_in_is_forgotten_as_sync_is_turned_off_here_before_it_ends() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let gated = Gated::of(&remote);
        let ctx = turned_on_through(&mut harness, Rc::clone(&gated) as Rc<dyn Remote>, &auth).await;
        let two = harness
            .in_app(|| harness.ctx.sessions.peek().id_at(1))
            .expect("two");
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        assert!(kept_state(&harness).drive.deleted(two), "the deletion sent");
        harness.in_app(|| ctx.undo_delete());

        // This copy's sync takes the note in, and waits on the drive's
        // answer to its first request.
        gated.shut.set(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        gated.shut.set(false);
        // Another window turns sync off; this one comes back into sight,
        // turns it on again, and off.
        harness.store.save_sync(None).expect("cleared");
        harness.in_app(|| ctx.sync_if_stale());
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        harness.in_app(|| ctx.sync_turn_off(false));
        gated.opened.set(true);
        harness.advance(u64::from(SYNC_SOON_MS) * 3).await;
        assert_eq!(status(&harness), SyncStatus::Off { problem: None });

        // Turned on again: the putting back went with the rest.
        harness.in_app(|| ctx.sync_sign_in());
        harness.pump();
        harness.in_app(|| ctx.sync_passphrase("a long passphrase".into(), None));
        harness.advance(100).await;
        assert!(status(&harness).is_on(), "{:?}", status(&harness));
        assert_eq!(stamps(&harness), [1], "let go: the drive's deletion");
        assert_eq!(joined_stamps(&remote), [1], "the drive's word");
    });
}

/// Move the clock on by `ms` at once, as a long wait does, letting what was
/// waiting on it run.
async fn leap(harness: &mut Harness, ms: u64) {
    tokio::time::advance(std::time::Duration::from_millis(ms)).await;
    tokio::task::yield_now().await;
    harness.pump();
    harness.advance(1_000).await;
}

/// How many packs the drive `remote` holds.
fn packs(remote: &MemoryRemote) -> usize {
    remote
        .names()
        .iter()
        .filter(|name| name.starts_with("dust-p-"))
        .count()
}

/// The same, with the two copies' syncs asked for at once: the other's
/// waits for this one's, which waits on the drive, while each puts its own
/// session back and the other deletes another. Nothing either did is lost:
/// this copy's sync lets go of neither putting back, the other's begins
/// from what this one kept, and between them they send both puttings back
/// and the deletion, here and everywhere.
#[test]
fn two_copies_syncing_at_once_lose_none_of_each_others_deletions_or_puttings_back() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2, 3, 4]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        let window = another_copy_through(
            &harness,
            Rc::clone(&held) as Rc<dyn Remote>,
            FakeAuth::giving(&["t"]) as Rc<dyn Auth>,
        );
        harness.in_app(|| window.take_up_store());
        let ids = harness.in_app(|| harness.ctx.sessions.peek().ids().to_vec());
        harness.in_app(|| window.delete_sessions(&[ids[1]]));
        harness.in_app(|| ctx.delete_sessions(&[ids[2]]));
        harness.in_app(|| ctx.sync_now());
        harness.pump();

        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        harness.in_app(|| window.sync_now());
        harness.pump();
        harness.in_app(|| window.undo_delete());
        harness.in_app(|| ctx.undo_delete());
        harness.in_app(|| window.delete_sessions(&[ids[3]]));
        held.hold(false);
        harness.advance(u64::from(SYNC_SOON_MS) * 3).await;
        harness.in_app(|| ctx.take_up_store());
        harness.in_app(|| window.take_up_store());
        let there: Vec<u64> = harness.in_app(|| {
            window
                .sessions
                .peek()
                .iter()
                .map(|session| session.timestamp)
                .collect()
        });
        assert_eq!(
            stored_stamps(&harness),
            [1, 2, 3],
            "both put back, one deleted"
        );
        assert_eq!(stamps(&harness), [1, 2, 3]);
        assert_eq!(there, [1, 2, 3]);
        assert_eq!(joined_stamps(&remote), [1, 2, 3], "the drive's word");
    });
}

/// A session deleted here by hand while a sync waits on the drive, before
/// that sync lists the history, after another device deleted it and put it
/// back since the sync began: the deletion here is the later word by the
/// clock, and stands everywhere. The sync took the tombstone, made after
/// the sync began, for one made with the clock set ahead, stamped it as the
/// sync began, before that putting back, and noted it as the drive's; the
/// next sync took the session back on every device.
#[test]
fn a_deletion_made_while_a_sync_reads_the_drive_outlasts_an_earlier_putting_back() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        harness.advance(1_000).await;
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let theirs = harness.in_app(|| ctx.settings.peek().clone().clamp());
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        let two = harness
            .in_app(|| ctx.sessions.peek().id_at(1))
            .expect("two");
        // This copy's sync begins, and waits on the drive.
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        // Meanwhile the other device deletes 2, and puts it back.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let session = delete_in(&other, two, now_ms());
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        std::thread::sleep(std::time::Duration::from_millis(5));
        put_back_in(&other, &mut other_state, two, &session, now_ms());
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        // Then, later by the clock, 2 is deleted here by hand.
        std::thread::sleep(std::time::Duration::from_millis(5));
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.pump();
        held.hold(false);
        harness.advance(1_000).await;
        harness.advance(u64::from(SYNC_SOON_MS) * 3).await;
        harness.in_app(|| ctx.sync_now());
        harness.advance(1_000).await;
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        assert_eq!(stored_stamps(&harness), [1], "deleted here, the later word");
        assert_eq!(joined_stamps(&remote), [1], "the drive's word");
        assert_eq!(held_by(&other), [1], "the other device");
    });
}

/// A session deleted here and sent, put back on another device, then put
/// back here by Undo while this copy's sync waits on the drive and deleted
/// by hand again in another window, later by the clock than the other
/// device's putting back: the deletion here is the last word, and stands.
/// The sync, which planned from the deletion it had noted as the drive's
/// and knew nothing of the Undo, took the new deletion for that one, put
/// back since in the drive, and took the session back on every device.
#[test]
fn a_deletion_by_hand_after_an_undo_during_a_sync_outlasts_an_earlier_putting_back() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        harness.advance(1_000).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.take_up_store());
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let theirs = harness.in_app(|| ctx.settings.peek().clone().clamp());
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        let two = harness
            .in_app(|| ctx.sessions.peek().id_at(1))
            .expect("two");
        // 2 deleted here and sent; the other device lets it go.
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.pump();
        harness.in_app(|| ctx.sync_now());
        harness.advance(1_000).await;
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        // This copy's next sync waits on the drive. Meanwhile the other
        // device puts 2 back, and syncs; then 2 is put back here by Undo and,
        // later still, deleted by hand in the other window.
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        std::thread::sleep(std::time::Duration::from_millis(5));
        put_back_in(&other, &mut other_state, two, &at(2), now_ms());
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        std::thread::sleep(std::time::Duration::from_millis(5));
        harness.in_app(|| ctx.undo_delete());
        harness.pump();
        std::thread::sleep(std::time::Duration::from_millis(5));
        harness.in_app(|| window.take_up_store());
        harness.in_app(|| window.delete_sessions(&[two]));
        harness.pump();
        held.hold(false);
        harness.advance(1_000).await;
        harness.advance(u64::from(SYNC_SOON_MS) * 3).await;
        for copy in [ctx, window, ctx] {
            harness.in_app(|| copy.take_up_store());
            harness.in_app(|| copy.sync_now());
            harness.advance(1_000).await;
        }
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        assert_eq!(
            stored_stamps(&harness),
            [1],
            "deleted by hand here, the last word"
        );
        assert_eq!(joined_stamps(&remote), [1], "the drive's word");
        assert_eq!(held_by(&other), [1], "the other device");
    });
}

/// A session put back here by Undo before a sync; deleted by hand, put back
/// by Undo and deleted by hand again in another window while that sync waits
/// on the drive; and put back on another device after that by the clock:
/// that putting back is the later word, and stands everywhere. The sync
/// sent the deletion and noted it, but as it ended stamped the Undo made
/// meanwhile again after what it read, though the session was deleted
/// since; the next sync took that for a putting back since, dropped the
/// note, and sent the deletion again, stamped after everything the sync
/// before read: after the other device's putting back.
#[test]
fn a_putting_back_elsewhere_after_a_deletion_by_hand_made_during_a_sync_stands() {
    run(|| async {
        let mut harness = Harness::new();
        keep(&harness, &[1, 2]);
        let remote = Rc::new(MemoryRemote::default());
        let auth = FakeAuth::giving(&["t"]);
        let (ctx, held) = turned_on_held(&mut harness, &remote, &auth).await;
        harness.advance(1_000).await;
        let window = another_copy(&harness, &remote);
        harness.in_app(|| window.take_up_store());
        let (other, mut other_state) = another_device(&remote, "a long passphrase", &[]);
        let theirs = harness.in_app(|| ctx.settings.peek().clone().clamp());
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        let two = harness
            .in_app(|| ctx.sessions.peek().id_at(1))
            .expect("two");
        // 2 deleted here and sent; the other device lets it go. Then put back
        // here by Undo, for the next sync to send.
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.pump();
        harness.in_app(|| ctx.sync_now());
        harness.advance(1_000).await;
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        std::thread::sleep(std::time::Duration::from_millis(5));
        harness.in_app(|| ctx.undo_delete());
        harness.pump();
        // That sync waits on the drive; meanwhile 2 is deleted by hand here,
        // put back by Undo, and deleted by hand again in the other window.
        held.hold(true);
        harness.in_app(|| ctx.sync_now());
        harness.pump();
        std::thread::sleep(std::time::Duration::from_millis(5));
        harness.in_app(|| ctx.delete_sessions(&[two]));
        harness.pump();
        std::thread::sleep(std::time::Duration::from_millis(5));
        harness.in_app(|| ctx.undo_delete());
        harness.pump();
        std::thread::sleep(std::time::Duration::from_millis(5));
        harness.in_app(|| window.take_up_store());
        harness.in_app(|| window.delete_sessions(&[two]));
        harness.pump();
        // Later by the clock, the other device puts 2 back, and syncs.
        std::thread::sleep(std::time::Duration::from_millis(5));
        put_back_in(&other, &mut other_state, two, &at(2), now_ms());
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        held.hold(false);
        harness.advance(1_000).await;
        harness.advance(u64::from(SYNC_SOON_MS) * 3).await;
        for copy in [ctx, window, ctx] {
            harness.in_app(|| copy.take_up_store());
            harness.in_app(|| copy.sync_now());
            harness.advance(1_000).await;
        }
        now(sync_once(
            &*other,
            &*remote,
            &mut other_state,
            &theirs,
            now_ms(),
        ))
        .expect("synced");
        assert_eq!(
            joined_stamps(&remote),
            [1, 2],
            "the drive's word: put back, the later"
        );
        assert_eq!(held_by(&other), [1, 2], "the other device");
        assert_eq!(stored_stamps(&harness), [1, 2], "here");
    });
}

/// What the store keeps for sync now, which it holds.
fn kept_state(harness: &Harness) -> SyncState {
    let raw = harness.store.sync.borrow().clone().expect("kept");
    SyncState::read(&raw).expect("reads")
}

/// The sessions the store holds, by stamp.
fn stored_stamps(harness: &Harness) -> Vec<u64> {
    harness
        .store
        .load_sessions()
        .iter()
        .map(|session| session.timestamp)
        .collect()
}

/// The sessions `store`, another device's, holds, by stamp.
fn held_by(store: &MemoryStore) -> Vec<u64> {
    store
        .load_history()
        .sessions
        .iter()
        .map(|(_, session)| session.timestamp)
        .collect()
}

/// The sessions a device joining the sync in `remote` now takes, by stamp:
/// the drive's word.
fn joined_stamps(remote: &MemoryRemote) -> Vec<u64> {
    let (joined, _) = another_device(remote, "a long passphrase", &[]);
    joined
        .load_history()
        .sessions
        .iter()
        .map(|(_, session)| session.timestamp)
        .collect()
}

/// Another copy of the app on the harness's store, a tab or a window, as
/// it opens: linked to `remote` through `auth`.
fn another_copy_through(harness: &Harness, remote: Rc<dyn Remote>, auth: Rc<dyn Auth>) -> AppCtx {
    let copy =
        harness.in_app(|| AppCtx::new(harness.app.clone(), Default::default(), Default::default()));
    harness.in_app(|| copy.link_sync(Link { auth, remote }))
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

/// Turn sync on here as the first device, signed in through `answered`,
/// its asking answered at once.
async fn turned_on_answering(
    harness: &mut Harness,
    remote: &Rc<MemoryRemote>,
    answered: &Rc<Answered>,
) -> AppCtx {
    let ctx = linked_through(
        harness,
        Rc::clone(remote) as Rc<dyn Remote>,
        Rc::clone(answered) as Rc<dyn Auth>,
    );
    let first = answered.asked();
    harness.in_app(|| ctx.sync_sign_in());
    answered.answer(first, Ok(()));
    harness.advance(100).await;
    let passphrase = || "a long passphrase".to_string();
    harness.in_app(|| ctx.sync_passphrase(passphrase(), Some(passphrase())));
    harness.advance(100).await;
    assert!(status(harness).is_on(), "{:?}", status(harness));
    ctx
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
