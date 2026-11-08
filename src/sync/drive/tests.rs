use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::fake::{FakeAuth, FakeDrive};
use super::*;
use crate::persist::Store;
use crate::persist::memory::MemoryStore;
use crate::persist::record::{HistoryChange, Record, RecordId};
use crate::persist::tests::at;
use crate::sync::run::{join, look, sync_once};
use crate::sync::seal::Stretch;
use crate::sync::state::SyncState;

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

fn drive(fake: &Rc<FakeDrive>, auth: &Rc<FakeAuth>) -> Rc<Drive> {
    Drive::new(
        Rc::new(Rc::clone(fake)),
        Counted::of(Rc::clone(auth) as Rc<dyn Auth>),
    )
}

/// Files made, listed a page at a time, read back and changed, then
/// deleted: each as Drive's API asks.
#[test]
fn files_go_in_and_come_out_of_the_app_folder() {
    let fake = FakeDrive::taking("t");
    let auth = FakeAuth::signed_in(&["t"]);
    let drive = drive(&fake, &auth);
    let mut ids = Vec::new();
    for n in 0..5u8 {
        // Bytes of every value, a would-be boundary among them.
        let mut bytes: Vec<u8> = (0..=255).collect();
        bytes.extend_from_slice(b"\r\n--dust-");
        bytes.push(n);
        ids.push(now(drive.create(&format!("dust-p-{n}"), bytes)).expect("made"));
    }
    fake.page.set(Some(2));
    let listed = now(drive.list()).expect("listed");
    assert_eq!(listed.len(), 5, "every page");
    assert_eq!(
        fake.asked
            .borrow()
            .iter()
            .filter(|asked| asked.starts_with("GET") && asked.contains("spaces="))
            .count(),
        3
    );
    assert!(listed.iter().all(|file| !file.created.is_empty()));
    let first = &listed
        .iter()
        .find(|file| file.name == "dust-p-0")
        .expect("listed")
        .id;
    let read = now(drive.get(first)).expect("read");
    assert_eq!(read.len(), 256 + 9 + 1);
    now(drive.update(first, b"changed".to_vec())).expect("changed");
    assert_eq!(now(drive.get(first)).expect("read"), b"changed");
    now(drive.delete(first)).expect("deleted");
    assert_eq!(now(drive.get(first)), Err(RemoteError::Gone));
    assert_eq!(now(drive.delete(first)), Err(RemoteError::Gone));
    assert_eq!(now(drive.list()).expect("listed").len(), 4);
}

/// A token refused is asked for afresh once, with no one asked; where the
/// fresh one is refused too, or there is none, sync needs a sign-in.
#[test]
fn a_refused_token_is_asked_for_afresh_once() {
    let fake = FakeDrive::taking("new");
    let auth = FakeAuth::signed_in(&["old", "new"]);
    let drive = drive(&fake, &auth);
    assert!(now(drive.list()).is_ok());
    assert_eq!(fake.asked.borrow().len(), 2, "old, then new");
    fake.tokens.borrow_mut().clear();
    assert_eq!(now(drive.list()), Err(RemoteError::SignIn));
    auth.signed_in.set(false);
    assert_eq!(now(drive.list()), Err(RemoteError::SignIn));
    assert_eq!(auth.asked.get(), 0, "the user is never asked from here");
}

/// Two accounts' drives behind Drive's one API, each request answered by
/// the drive of the account whose token it bears; and what happens as the
/// first request is under way, `meanwhile`.
struct Accounts {
    first: Rc<FakeDrive>,
    second: Rc<FakeDrive>,
    meanwhile: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl Http for Accounts {
    fn send(&self, request: Request) -> Pending<Result<Response, String>> {
        if let Some(meanwhile) = self.meanwhile.borrow_mut().take() {
            meanwhile();
        }
        let token = request
            .header_of("Authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default()
            .to_string();
        if self.second.tokens.borrow().contains(&token) {
            self.second.send(request)
        } else {
            self.first.send(request)
        }
    }
}

/// A request begun with one sign-in asks nothing more once sync is turned
/// off and on anew with another account as it is under way: neither a
/// token of the first refused and asked for afresh, nor the next page of a
/// listing, goes with the other account's token, to its drive, sealed with
/// a key its sync does not go by. Each used to.
#[test]
fn a_request_under_way_as_another_account_signs_in_asks_nothing_of_its_drive() {
    for listing in [false, true] {
        let first = FakeDrive::taking("ours");
        let second = FakeDrive::taking("theirs");
        let auth = FakeAuth::signed_in(&[if listing { "ours" } else { "spent" }, "ours"]);
        let signed_in = Counted::of(Rc::clone(&auth) as Rc<dyn Auth>);
        let (switching, app) = (Rc::clone(&auth), Rc::clone(&signed_in));
        let accounts = Rc::new(Accounts {
            first: Rc::clone(&first),
            second: Rc::clone(&second),
            // Sync turned off, as the app turns it off, and on anew there
            // with another account.
            meanwhile: RefCell::new(Some(Box::new(move || {
                app.forget();
                *switching.tokens.borrow_mut() = vec!["theirs".into()];
                now(app.sign_in()).expect("signed in");
            }))),
        });
        let drive = Drive::new(accounts, signed_in);
        if listing {
            let ours = self::drive(&first, &FakeAuth::signed_in(&["ours"]));
            for name in ["dust-p-1", "dust-p-2"] {
                now(ours.create(name, vec![1])).expect("made");
            }
            first.page.set(Some(1));
            assert_eq!(now(drive.list()), Err(RemoteError::SignIn));
        } else {
            assert_eq!(
                now(drive.create("dust-p-1", vec![1])),
                Err(RemoteError::SignIn)
            );
        }
        assert!(
            second.asked.borrow().is_empty(),
            "{:?}",
            second.asked.borrow()
        );
        assert!(second.names().is_empty());
    }
}

/// What Drive says of a failure is read as sync reads it.
#[test]
fn drives_failures_are_read_for_what_they_are() {
    let fake = FakeDrive::taking("t");
    let drive = drive(&fake, &FakeAuth::signed_in(&["t"]));
    fake.full.set(true);
    assert_eq!(
        now(drive.create("dust-p-1", vec![1])),
        Err(RemoteError::Full)
    );
    let answered = |status, body: &str| {
        read(Response {
            status,
            body: body.as_bytes().to_vec(),
        })
    };
    assert_eq!(
        answered(
            403,
            r#"{"error":{"errors":[{"reason":"insufficientPermissions"}],"message":"Insufficient Permission"}}"#
        ),
        Err(RemoteError::SignIn)
    );
    assert_eq!(
        answered(500, r#"{"error":{"message":"Backend Error"}}"#),
        Err(RemoteError::Failed(
            "Google Drive answered 500: Backend Error".into()
        ))
    );
    assert_eq!(
        answered(503, "<html>"),
        Err(RemoteError::Failed("Google Drive answered 503".into()))
    );
    assert!(answered(204, "").is_ok());
}

/// Two devices sync through Drive's API as through any folder: the
/// history and the settings of each reach the other, and Drive sees only
/// sealed bytes under names that say nothing.
#[test]
fn two_devices_sync_through_drives_api() {
    let fake = FakeDrive::taking("t");
    let auth = FakeAuth::signed_in(&["t"]);
    let drive = drive(&fake, &auth);
    let device = |stamps: &[u64]| {
        let store = Rc::new(MemoryStore::default());
        let mut newest = None;
        let put = stamps
            .iter()
            .map(|&stamp| {
                let session = at(stamp);
                let record = Record::of(&session, |json| RecordId::next(stamp, newest, json))
                    .expect("a record");
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
        store
    };
    let join_as = |name: &str| {
        let found = now(look(&drive)).expect("looked");
        let (key, keyring) =
            now(join(&drive, "passphrase", &found, Stretch::QUICK)).expect("joined");
        SyncState::new(name.into(), key.to_hex(), keyring, None)
    };
    let phone = device(&[1, 2]);
    let mut phone_state = join_as("phone");
    let mut settings = cw_core::TrainingSettings::default();
    settings.curriculum.num_groups = 8;
    now(sync_once(&*phone, &drive, &mut phone_state, &settings, 10)).expect("synced");

    let computer = device(&[3]);
    let mut computer_state = join_as("computer");
    let synced = now(sync_once(
        &*computer,
        &drive,
        &mut computer_state,
        &Default::default(),
        20,
    ))
    .expect("synced");
    assert_eq!(synced.took, 2);
    assert_eq!(synced.settings.map(|s| s.curriculum.num_groups), Some(8));
    now(sync_once(&*phone, &drive, &mut phone_state, &settings, 30)).expect("synced");
    let stamps = |store: &MemoryStore| -> Vec<u64> {
        store
            .load_history()
            .sessions
            .iter()
            .map(|(_, s)| s.timestamp)
            .collect()
    };
    assert_eq!(stamps(&phone), [1, 2, 3]);
    assert_eq!(stamps(&computer), [1, 2, 3]);
    let names = fake.names();
    assert_eq!(names[0], "dust-keyring");
    assert!(
        names[1..]
            .iter()
            .all(|name| name.starts_with("dust-p-") || name.starts_with("dust-s-")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .all(|name| !name.contains("phone") && !name.contains("computer"))
    );
    let asked = Cell::new(0);
    for line in fake.asked.borrow().iter() {
        assert!(
            line.starts_with("GET https://www.googleapis.com/")
                || line.starts_with("POST https://www.googleapis.com/upload/")
                || line.starts_with("PATCH https://www.googleapis.com/upload/"),
            "{line}"
        );
        asked.set(asked.get() + 1);
    }
    assert!(asked.get() > 0);
}
