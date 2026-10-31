//! Backups, on the History tab: a file of the history and the settings the
//! user keeps outside the app, saved and restored from here, and a word when
//! the last one is many sessions old.

use dioxus::prelude::*;

use crate::persist::backup::{
    BACKUP_DUE_AFTER, BACKUP_LARGE_BYTES, BACKUP_MAX_BYTES, Backup, read_backup_apart,
};
use crate::ui::file_pick::{ChosenFile, FileButton};
use crate::ui::format::{counted, day_of, short_date};
use crate::ui::widgets::{CardHead, Icon, IconName, control_id};

/// What the History tab needs to save and restore backups.
#[derive(Clone, PartialEq)]
pub struct Backups {
    /// When the last backup was saved, if ever.
    pub last: Option<u64>,
    /// This build can hand over a backup file. One that cannot can still
    /// restore.
    pub can_save: bool,
    /// A backup file this build hands over is known to be saved. A
    /// browser's download is only handed over: it may block it unsaid.
    pub confirmed: bool,
    /// A backup file this build saves is kept inside the app, and deleted
    /// with it, until it is moved out as this says.
    pub inside_app: Option<&'static str>,
    pub on_save: EventHandler<()>,
    pub on_restore: EventHandler<Backup>,
}

/// What could be lost without a backup, in the words of where it is kept.
const AT_RISK: &str = if cfg!(target_arch = "wasm32") {
    "if this browser's data is cleared"
} else {
    "if this device's data is lost"
};

/// What a restore does: no more, and no less.
const RESTORING: &str = "adds the sessions missing here and takes its settings, level included, though not this device's volume and paddle keys. Undo, in the message that follows, puts both back";

/// The files offered to restore from. An Android picker that filters by
/// type hides a backup that another app stored untyped, so on Android —
/// the app, or the web app in a browser there — every file is offered, and
/// one that is not a backup is refused once it is read.
fn backup_accept() -> &'static str {
    if on_android() {
        ""
    } else {
        ".json,application/json"
    }
}

/// Whether the app runs on Android: built for it, or the web app in a
/// browser that says it is there.
fn on_android() -> bool {
    #[cfg(feature = "web")]
    {
        web_sys::window()
            .and_then(|window| window.navigator().user_agent().ok())
            .is_some_and(|agent| agent.contains("Android"))
    }
    #[cfg(not(feature = "web"))]
    {
        cfg!(target_os = "android")
    }
}

/// The backup a chosen file holds, or why it could not be restored from:
/// read on a thread of its own where there is one, as a backup of years of
/// sessions takes seconds to read.
async fn backup_in(file: ChosenFile) -> Result<Backup, String> {
    let ChosenFile { name, size, text } = file;
    if size > BACKUP_MAX_BYTES {
        return Err(format!(
            "{name} is far larger than any backup: nothing was restored."
        ));
    }
    let text = text.ok_or_else(|| format!("{name} could not be read: nothing was restored."))?;
    read_backup_apart(text)
        .await
        .map_err(|not| format!("{name} {}: nothing was restored.", not.reason()))
}

/// What the card says while `file` is read as a backup.
fn reading(file: &ChosenFile) -> String {
    reading_for(&file.name, file.size)
}

/// What the card says while a file called `name`, of `size` bytes, is read
/// as a backup: and of a large one, that it takes a while.
pub(super) fn reading_for(name: &str, size: u64) -> String {
    if size > BACKUP_LARGE_BYTES {
        format!(
            "Reading {name}, {}: a backup this large takes a while to restore.",
            megabytes(size)
        )
    } else {
        format!("Reading {name}…")
    }
}

/// `bytes`, in whole megabytes, rounded up.
fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1_000_000))
}

/// What to say, before a backup of `size` bytes is restored, where this
/// browser says it has less room left for the site: nothing where it has
/// room enough, or will not say. A backup that does not fit is not
/// restored at all, and nothing of it is lost: the file is still there.
#[cfg(feature = "web")]
async fn room_for(size: u64) -> Option<String> {
    use wasm_bindgen::JsValue;
    let navigator = web_sys::window()?.navigator();
    // Older browsers have no storage manager at all.
    if !js_sys::Reflect::has(&navigator, &JsValue::from_str("storage")).unwrap_or(false) {
        return None;
    }
    let asked = navigator.storage().estimate().ok()?;
    let estimate = wasm_bindgen_futures::JsFuture::from(asked).await.ok()?;
    let field = |key: &str| {
        js_sys::Reflect::get(&estimate, &JsValue::from_str(key))
            .ok()?
            .as_f64()
    };
    let left = (field("quota")? - field("usage")?).max(0.0) as u64;
    (left < size).then(|| {
        format!(
            "This browser says it has room for about {} more for this site: if the backup does not fit, none of it is restored.",
            megabytes(left)
        )
    })
}

/// Save a backup, or restore one; and when `since` sessions have gone
/// unbacked for long enough, say so.
#[component]
pub fn BackupCard(
    backups: Backups,
    /// The sessions finished since the last backup, or all of them.
    since: usize,
) -> Element {
    let mut problem = use_signal(|| None::<String>);
    // What the card says while a chosen file is read.
    let mut under_way = use_signal(|| None::<String>);
    // Asked once: the browser says where it runs as the page opens.
    let accept = use_hook(backup_accept);
    let Backups {
        last,
        can_save,
        confirmed,
        inside_app,
        on_save,
        on_restore,
    } = backups;
    let due = can_save && since >= BACKUP_DUE_AFTER;
    let last_day = last.and_then(day_of).map(|day| short_date(&day));
    let saved = |day: &str| match (confirmed, inside_app) {
        (false, _) => format!("Last handed to your browser {day}"),
        (true, Some(_)) => format!("Last saved {day} in Dust's folder"),
        (true, None) => format!("Last saved {day}"),
    };
    let status = match (&last_day, since) {
        (Some(day), 0) => format!("{} · nothing new since", saved(day)),
        (Some(day), _) => format!("{} · {} since", saved(day), counted(since, "session")),
        (None, _) => "No backup yet".to_string(),
    };
    let note = if let (true, Some(how)) = (can_save, inside_app) {
        // Saved inside the app, a file goes with it: safe once moved out.
        let move_out = format!("move it out of Dust's folder {how} to keep them safe");
        if due && last.is_some() {
            format!(
                "{} since your last backup. Save a new one and {move_out}.",
                counted(since, "session")
            )
        } else if due {
            format!(
                "{} and no backup yet. Save one and {move_out}, and to move them to another device.",
                counted(since, "session")
            )
        } else {
            format!(
                "A backup file is saved in Dust's own folder, which goes if Dust is deleted: move each one out {how} to keep your history and settings safe. Restoring one {RESTORING}."
            )
        }
    } else if !can_save {
        format!(
            "Bring in a backup saved by the web app or on a computer: restoring it {RESTORING}."
        )
    } else if due && last.is_some() {
        format!(
            "{} since your last backup. A new one keeps them safe {AT_RISK}.",
            counted(since, "session")
        )
    } else if due {
        format!(
            "{} and no backup yet. A backup file keeps them safe {AT_RISK}, and moves them to another device.",
            counted(since, "session")
        )
    } else {
        format!(
            "A backup file keeps your history and settings safe {AT_RISK}. Restoring one {RESTORING}."
        )
    };
    rsx! {
        div { class: if due { "card stack-sm backup-card due" } else { "card stack-sm backup-card" },
            CardHead { icon: IconName::Download, title: "Backup", note: status }
            p { class: "backup-note", "{note}" }
            div { class: "backup-actions",
                if can_save {
                    button {
                        id: control_id("btn", "save a backup"),
                        class: if due { "btn btn-primary btn-sm" } else { "btn btn-secondary btn-sm" },
                        r#type: "button",
                        onclick: move |_| on_save.call(()),
                        Icon { name: IconName::Download }
                        "Save a backup"
                    }
                }
                FileButton {
                    id: control_id("file", "backup"),
                    accept,
                    max_bytes: BACKUP_MAX_BYTES,
                    icon: IconName::Upload,
                    label: "Restore a backup",
                    on_choose: move |file: ChosenFile| {
                        problem.set(None);
                        under_way.set(Some(reading(&file)));
                        spawn(async move {
                            // A browser reads it on the page's own thread:
                            // what the card says is shown first, with how
                            // much room the browser says it has.
                            #[cfg(feature = "web")]
                            {
                                if let Some(room) = room_for(file.size).await {
                                    under_way.set(Some(format!("{} {room}", reading(&file))));
                                }
                                crate::time::sleep_ms(crate::time::POLL_MS).await;
                            }
                            let read = backup_in(file).await;
                            under_way.set(None);
                            match read {
                                Ok(backup) => on_restore.call(backup),
                                Err(why) => problem.set(Some(why)),
                            }
                        });
                    },
                }
            }
            if let Some(under_way) = under_way() {
                p { class: "backup-status", role: "status", "{under_way}" }
            }
            if let Some(problem) = problem() {
                p { class: "backup-problem", role: "alert", "{problem}" }
            }
        }
    }
}
