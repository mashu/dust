//! The sync card in each state, and what each of its buttons asks for.

use std::cell::RefCell;

use super::*;
use crate::testing::Ui;

thread_local! {
    static ASKED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn asked(what: String) {
    ASKED.with(|asked| asked.borrow_mut().push(what));
}

fn take_asked() -> Vec<String> {
    ASKED.with(|asked| std::mem::take(&mut *asked.borrow_mut()))
}

/// The card on `status`, its every move noted.
#[component]
fn Card(status: SyncStatus) -> Element {
    let view = SyncView {
        status,
        on_sign_in: EventHandler::new(|()| asked("sign in".into())),
        on_passphrase: EventHandler::new(|(typed, again): (String, Option<String>)| {
            asked(format!("passphrase {typed:?} {again:?}"));
        }),
        on_cancel: EventHandler::new(|()| asked("cancel".into())),
        on_sync_now: EventHandler::new(|()| asked("sync now".into())),
        on_turn_off: EventHandler::new(|copy: bool| asked(format!("turn off {copy}"))),
    };
    rsx! { SyncCard { view } }
}

fn card(status: SyncStatus) -> Ui {
    take_asked();
    Ui::new(Card, CardProps { status })
}

fn on(last: Option<u64>) -> SyncStatus {
    SyncStatus::On {
        last,
        syncing: false,
        problem: None,
        needs_sign_in: false,
    }
}

/// Off, the card says what sync is and what it asks of Google, and turns
/// it on with a sign-in.
#[test]
fn off_the_card_says_what_sync_is() {
    let mut ui = card(SyncStatus::Off { problem: None });
    // As the page writes it: an apostrophe escaped.
    let shown = |text: &str| text.replace('\'', "&#39;");
    assert!(ui.has(&shown(SYNC_ABOUT)), "{}", ui.html());
    assert!(ui.has(&shown(SYNC_SCOPE)));
    assert!(ui.has("cannot read it"));
    ui.click("btn-turn-on-sync");
    assert_eq!(take_asked(), ["sign in"]);
    let ui = card(SyncStatus::Off {
        problem: Some("Signing in to Google failed: offline.".into()),
    });
    assert!(ui.has("Signing in to Google failed: offline."));
}

/// A new passphrase is asked for twice, and goes with Enter as with the
/// button; joining asks for it once.
#[test]
fn a_passphrase_is_asked_for() {
    let mut ui = card(SyncStatus::Passphrase {
        new: true,
        problem: None,
    });
    assert!(ui.has("Nobody can recover it for you"));
    assert!(
        ui.input_tag("sync-passphrase")
            .contains("type=\"password\"")
    );
    ui.type_into("sync-passphrase", "one two three");
    ui.type_into("sync-passphrase-again", "one two three");
    ui.click("btn-open-sync");
    let sent = r#"passphrase "one two three" Some("one two three")"#;
    assert_eq!(take_asked(), [sent]);
    // Handed over, it is no longer held by the card, nor shown.
    assert!(!ui.has("one two three"));
    assert!(ui.input_tag("sync-passphrase").contains("value=\"\""));
    ui.type_into("sync-passphrase", "four five six");
    ui.type_into("sync-passphrase-again", "four five six");
    ui.press_enter("sync-passphrase-again");
    assert_eq!(
        take_asked(),
        [r#"passphrase "four five six" Some("four five six")"#]
    );
    ui.click("btn-cancel-sync");
    assert_eq!(take_asked(), ["cancel"]);

    let mut ui = card(SyncStatus::Passphrase {
        new: false,
        problem: Some("That passphrase does not open the synced copy in your Google Drive.".into()),
    });
    assert!(ui.has("holds a synced copy of Dust already"));
    assert!(!ui.has("sync-passphrase-again"));
    assert!(ui.has("does not open"));
    ui.type_into("sync-passphrase", "mine");
    ui.click("btn-open-sync");
    assert_eq!(take_asked(), [r#"passphrase "mine" None"#]);
}

/// On, the card says when it last synced, syncs now, and asks before
/// turning off, here or with the copy.
#[test]
fn on_the_card_syncs_and_turns_off() {
    let mut ui = card(on(Some(crate::time::now_ms())));
    assert!(ui.has("Synced today at"), "{}", ui.html());
    ui.click("btn-sync-now");
    assert_eq!(take_asked(), ["sync now"]);
    ui.click("btn-turn-off-sync");
    assert!(ui.has("forgets the key and the sign-in"));
    ui.click("btn-keep-sync");
    assert!(!ui.has("forgets the key"));
    ui.click("btn-turn-off-sync");
    ui.click("btn-turn-off-sync-here");
    ui.click("btn-turn-off-sync");
    ui.click("btn-delete-synced-copy");
    assert_eq!(take_asked(), ["turn off false", "turn off true"]);

    assert!(card(on(None)).has("Not synced yet"));
    let syncing = card(SyncStatus::On {
        last: None,
        syncing: true,
        problem: None,
        needs_sign_in: false,
    });
    assert!(syncing.has("Syncing…"));
    assert!(syncing.input_tag("btn-sync-now").contains("disabled"));
}

/// Signed out, the card asks for a sign-in in place of syncing.
#[test]
fn signed_out_the_card_asks_for_a_sign_in() {
    let mut ui = card(SyncStatus::On {
        last: None,
        syncing: false,
        problem: None,
        needs_sign_in: true,
    });
    assert!(ui.has("Sign in to Google again to sync."));
    assert!(!ui.has("btn-sync-now"));
    ui.click("btn-sign-in-to-sync");
    assert_eq!(take_asked(), ["sign in"]);
}

/// Every other state says what is under way.
#[test]
fn the_card_says_what_is_under_way() {
    assert!(card(SyncStatus::SigningIn).has("Finish signing in with Google"));
    assert!(card(SyncStatus::Unlocking).has("Unlocking the synced copy"));
    assert!(card(SyncStatus::TurningOff).has("Deleting the synced copy"));
    assert!(card(SyncStatus::Unavailable(Some("Not this visit."))).has("Not this visit."));
}

/// The History tab, or the Stats page, given sync, holding one session.
#[component]
fn Stats(status: SyncStatus, history: bool) -> Element {
    let view = SyncView {
        status,
        on_sign_in: EventHandler::new(|()| {}),
        on_passphrase: EventHandler::new(|_: (String, Option<String>)| {}),
        on_cancel: EventHandler::new(|()| {}),
        on_sync_now: EventHandler::new(|()| {}),
        on_turn_off: EventHandler::new(|_: bool| {}),
    };
    let sessions = vec![crate::testing::stored_session(
        "2026-11-01",
        &[("KM", "KM")],
    )];
    if history {
        rsx! { crate::ui::stats_detail::HistoryTab { sessions, sync: view } }
    } else {
        rsx! {
            crate::ui::stats::StatsView {
                settings: cw_core::TrainingSettings::default(),
                sessions,
                sync: view,
            }
        }
    }
}

/// Synced, the history and the Stats page are every synced device's, and
/// say so; otherwise this device's.
#[test]
fn synced_the_history_is_every_devices() {
    for history in [true, false] {
        let synced = Ui::new(
            Stats,
            StatsProps {
                status: on(None),
                history,
            },
        );
        assert!(synced.has("on your synced devices"), "{}", synced.html());
        assert!(!synced.has("on this device"), "{}", synced.html());
        let off = Ui::new(
            Stats,
            StatsProps {
                status: SyncStatus::Off { problem: None },
                history,
            },
        );
        assert!(off.has("on this device"), "{}", off.html());
        assert!(!off.has("synced devices"));
    }
}

#[test]
fn when_it_synced_is_said_plainly() {
    let now = crate::time::now_ms();
    assert_eq!(synced_when(None, now), "Not synced yet");
    assert!(synced_when(Some(now), now).starts_with("Synced today at "));
    assert!(synced_when(Some(now - 86_400_000), now).starts_with("Synced yesterday at "));
    let long_ago = synced_when(Some(now - 20 * 86_400_000), now);
    assert!(
        long_ago.starts_with("Synced ") && long_ago.contains(" at "),
        "{long_ago}"
    );
}
