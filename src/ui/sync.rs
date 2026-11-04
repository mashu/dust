//! Sync, on the History tab beside Backup: turning it on, with a sign-in to
//! Google and a passphrase; syncing now; and turning it off, here or with
//! the copy in the Drive. What the card says it does, the privacy policy
//! says in the same words ([`SYNC_ABOUT`]).

use dioxus::prelude::*;

use crate::sync::ctx::SyncStatus;
use crate::sync::seal::MIN_PASSPHRASE;
use crate::ui::format::{ago, clock, day_of, short_date};
use crate::ui::widgets::{CardHead, IconName, control_id, html_bool};

/// What sync is, said on the card and in the policy alike.
pub const SYNC_ABOUT: &str = "Sync keeps your history and settings the same on each device you \
     turn it on, through a folder of Dust's own in your Google Drive. Everything is encrypted on \
     the device, with a passphrase only you know, before it goes: Google keeps it but cannot read \
     it, and the developer never receives any of it.";

/// What signing in asks Google for, said on the card and in the policy.
pub const SYNC_SCOPE: &str = "Signing in asks Google to let Dust see its own folder only: none \
     of your files, and nothing of any other app's.";

/// What a new passphrase is for, and what losing it costs.
const NEW_PASSPHRASE: &str = "Choose a passphrase for sync. It encrypts everything Dust puts in \
     your Drive, and each device you turn sync on asks for it once. Nobody can recover it for \
     you: without it the synced copy cannot be read, though each device keeps what it holds.";

/// What joining a sync does.
const JOIN: &str = "Your Google Drive holds a synced copy of Dust already. Enter the passphrase \
     you chose for it. This device then takes the sessions and settings there, as a restore \
     does, but for its own volume and paddle keys, and adds its own sessions to them.";

/// What turning sync off does here, and with the copy.
const TURN_OFF: &str = "Turning sync off here forgets the key and the sign-in on this device. \
     What this device holds stays, and so does the copy in your Drive, for your other devices. \
     Deleting the copy too stops sync on every device; what each holds stays.";

/// What the card needs of sync.
#[derive(Clone, PartialEq)]
pub struct SyncView {
    pub status: SyncStatus,
    /// Sign in to Google: to turn sync on, or to sync again.
    pub on_sign_in: EventHandler<()>,
    /// The passphrase, and for a new one, the same again.
    pub on_passphrase: EventHandler<(String, Option<String>)>,
    pub on_cancel: EventHandler<()>,
    pub on_sync_now: EventHandler<()>,
    /// Turn sync off here, deleting the copy in the drive too where true.
    pub on_turn_off: EventHandler<bool>,
}

/// When this device last synced, as the card's head says it.
fn synced_when(last: Option<u64>, now: u64) -> String {
    let Some(last) = last else {
        return "Not synced yet".into();
    };
    let (Some(day), Some(today), Some(time)) = (day_of(last), day_of(now), clock(last)) else {
        return "Synced".into();
    };
    match ago(&day, &today) {
        Some(when) if when == "today" || when == "yesterday" => format!("Synced {when} at {time}"),
        _ => format!("Synced {} at {time}", short_date(&day)),
    }
}

#[component]
pub fn SyncCard(view: SyncView) -> Element {
    let mut passphrase = use_signal(String::new);
    let mut repeated = use_signal(String::new);
    let mut confirming = use_signal(|| false);
    let SyncView {
        status,
        on_sign_in,
        on_passphrase,
        on_cancel,
        on_sync_now,
        on_turn_off,
    } = view;
    let note = match &status {
        SyncStatus::Unavailable(_) => "Not here".to_string(),
        SyncStatus::Off { .. } => "Off".into(),
        SyncStatus::SigningIn => "Signing in".into(),
        SyncStatus::Passphrase { new: true, .. } => "Choose a passphrase".into(),
        SyncStatus::Passphrase { new: false, .. } => "Enter your passphrase".into(),
        SyncStatus::Unlocking => "Unlocking".into(),
        SyncStatus::On { syncing: true, .. } => "Syncing…".into(),
        SyncStatus::On { last, .. } => synced_when(*last, crate::time::now_ms()),
        SyncStatus::TurningOff => "Turning off".into(),
    };
    let problem = match &status {
        SyncStatus::Off { problem }
        | SyncStatus::Passphrase { problem, .. }
        | SyncStatus::On { problem, .. } => problem.clone(),
        _ => None,
    };
    // Handed over, and not held by the card a moment longer: a wrong one
    // is typed again.
    let mut send = move |new: bool| {
        let typed = std::mem::take(&mut *passphrase.write());
        let again = new.then(|| std::mem::take(&mut *repeated.write()));
        on_passphrase.call((typed, again));
    };
    rsx! {
        div { class: "card stack-sm backup-card sync-card",
            CardHead { icon: IconName::Cloud, title: "Sync", note }
            match status {
                SyncStatus::Unavailable(why) => rsx! {
                    p { class: "backup-note", "{why.unwrap_or_default()}" }
                },
                SyncStatus::Off { .. } => rsx! {
                    p { class: "backup-note", "{SYNC_ABOUT}" }
                    p { class: "backup-note", "{SYNC_SCOPE}" }
                    div { class: "backup-actions",
                        button {
                            id: control_id("btn", "turn on sync"),
                            class: "btn btn-primary btn-sm",
                            r#type: "button",
                            onclick: move |_| on_sign_in.call(()),
                            "Turn on sync"
                        }
                    }
                },
                SyncStatus::SigningIn => rsx! {
                    p { class: "backup-status", role: "status", "Finish signing in with Google, then come back here." }
                    div { class: "backup-actions",
                        button {
                            id: control_id("btn", "cancel sync"),
                            class: "btn btn-secondary btn-sm",
                            r#type: "button",
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                    }
                },
                SyncStatus::Passphrase { new, .. } => rsx! {
                    p { class: "backup-note", if new { "{NEW_PASSPHRASE}" } else { "{JOIN}" } }
                    div {
                        class: "stack-sm",
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter {
                                e.prevent_default();
                                send(new);
                            }
                        },
                        div { class: "field",
                            label { r#for: control_id("sync", "passphrase"), "Passphrase" }
                            input {
                                id: control_id("sync", "passphrase"),
                                r#type: "password",
                                autocomplete: if new { "new-password" } else { "current-password" },
                                minlength: if new { "{MIN_PASSPHRASE}" } else { "1" },
                                value: "{passphrase}",
                                oninput: move |e| passphrase.set(e.value()),
                            }
                        }
                        if new {
                            div { class: "field",
                                label { r#for: control_id("sync", "passphrase again"), "The same again" }
                                input {
                                    id: control_id("sync", "passphrase again"),
                                    r#type: "password",
                                    autocomplete: "new-password",
                                    value: "{repeated}",
                                    oninput: move |e| repeated.set(e.value()),
                                }
                            }
                        }
                        div { class: "backup-actions",
                            button {
                                id: control_id("btn", "open sync"),
                                class: "btn btn-primary btn-sm",
                                r#type: "button",
                                onclick: move |_| send(new),
                                "Turn on sync"
                            }
                            button {
                                id: control_id("btn", "cancel sync"),
                                class: "btn btn-secondary btn-sm",
                                r#type: "button",
                                onclick: move |_| on_cancel.call(()),
                                "Cancel"
                            }
                        }
                    }
                },
                SyncStatus::Unlocking => rsx! {
                    p { class: "backup-status", role: "status", "Unlocking the synced copy…" }
                },
                SyncStatus::TurningOff => rsx! {
                    p { class: "backup-status", role: "status", "Deleting the synced copy from your Google Drive…" }
                },
                SyncStatus::On { syncing, needs_sign_in, .. } => rsx! {
                    p { class: "backup-note",
                        "Your history and settings sync through a folder of Dust's own in your Google Drive, encrypted with your passphrase."
                    }
                    if needs_sign_in {
                        p { class: "backup-note", "Sign in to Google again to sync." }
                    }
                    if confirming() {
                        p { class: "backup-note", "{TURN_OFF}" }
                        div { class: "backup-actions",
                            button {
                                id: control_id("btn", "turn off sync here"),
                                class: "btn btn-secondary btn-sm",
                                r#type: "button",
                                onclick: move |_| {
                                    confirming.set(false);
                                    on_turn_off.call(false);
                                },
                                "Turn off here"
                            }
                            button {
                                id: control_id("btn", "delete synced copy"),
                                class: "btn btn-danger btn-sm",
                                r#type: "button",
                                onclick: move |_| {
                                    confirming.set(false);
                                    on_turn_off.call(true);
                                },
                                "Turn off and delete the copy"
                            }
                            button {
                                id: control_id("btn", "keep sync"),
                                class: "btn btn-secondary btn-sm",
                                r#type: "button",
                                onclick: move |_| confirming.set(false),
                                "Keep sync on"
                            }
                        }
                    } else {
                        div { class: "backup-actions",
                            if needs_sign_in {
                                button {
                                    id: control_id("btn", "sign in to sync"),
                                    class: "btn btn-primary btn-sm",
                                    r#type: "button",
                                    onclick: move |_| on_sign_in.call(()),
                                    "Sign in to sync"
                                }
                            } else {
                                button {
                                    id: control_id("btn", "sync now"),
                                    class: "btn btn-secondary btn-sm",
                                    r#type: "button",
                                    disabled: html_bool(syncing),
                                    onclick: move |_| on_sync_now.call(()),
                                    "Sync now"
                                }
                            }
                            button {
                                id: control_id("btn", "turn off sync"),
                                class: "btn btn-secondary btn-sm",
                                r#type: "button",
                                onclick: move |_| confirming.set(true),
                                "Turn off…"
                            }
                        }
                    }
                },
            }
            if let Some(problem) = problem {
                p { class: "backup-problem", role: "alert", "{problem}" }
            }
        }
    }
}

#[cfg(test)]
mod tests;
