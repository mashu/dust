//! Backups, on the History tab: a file of the history and the settings the
//! user keeps outside the app, saved and restored from here, and a word when
//! the last one is many sessions old.

use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use crate::format::{counted, day_of, short_date};
use crate::persist::backup::{
    BACKUP_DUE_AFTER, BACKUP_LARGE_BYTES, BACKUP_MAX_BYTES, Backup, read_backup_apart,
};
use crate::state::{AppCtx, RESTORE_LOCK_POLL_MS, in_root};
use crate::ui::file_pick::{ChosenFile, FileButton, PickedFile};
use crate::ui::widgets::{CardHead, Icon, IconName, control_id};

/// What the History tab needs to save backups, and to say of them. A backup
/// chosen to restore is the app's own to restore, whatever is on screen by
/// the time it has been read.
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
    /// The history has grown past what the device's own backup of the app
    /// takes ([`crate::persist::Store::outgrows_device_backup`]).
    pub outgrows_device_backup: bool,
    pub on_save: EventHandler<()>,
}

/// What could be lost without a backup, in the words of where it is kept.
const AT_RISK: &str = if cfg!(target_arch = "wasm32") {
    "if this browser's data is cleared"
} else {
    "if this device's data is lost"
};

/// What the card says where the history has grown past what Android's own
/// backup of the app takes, which then backs up none of it.
const OUTGROWN: &str = "Your history here is past 20 MB, and Android's own backup of an app stops at 25 MB: save a backup file now and then, so a lost or new phone still has every session.";

/// What the card says while a backup read waits for another window's
/// restore, or the Undo of it, which has every window's restores
/// ([`AppCtx::restore_blocked`]).
const WAITING_ELSEWHERE: &str = "Waiting for a restore in another window…";

/// How long the card waits between looks at a backup it handed to the app
/// while a session or run has the app, which the restore waits for: as
/// long as the app waits between its own looks at the session, rather than
/// waking the app's thread every [`crate::time::POLL_MS`] through it.
const SESSION_WAIT_MS: u32 = 1_000;

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

/// What a chosen file holds, as text, with its name; or why it could not
/// be restored from: far larger than any backup, or not read.
async fn text_in(file: PickedFile) -> Result<(String, String), String> {
    let ChosenFile { name, size, text } = file.read(BACKUP_MAX_BYTES).await;
    if size > BACKUP_MAX_BYTES {
        return Err(format!(
            "{name} is far larger than any backup: nothing was restored."
        ));
    }
    let text = text.ok_or_else(|| format!("{name} could not be read: nothing was restored."))?;
    Ok((name, text))
}

/// The backup `text`, what the file `name` holds, reads as, or why it could
/// not be restored from: read on a thread of its own where there is one, as
/// a backup of years of sessions takes seconds to read.
async fn backup_in(name: &str, text: String) -> Result<Backup, String> {
    #[cfg(test)]
    crate::testing::rendered("backup read");
    read_backup_apart(text)
        .await
        .map_err(|not| format!("{name} {}: nothing was restored.", not.reason()))
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

/// A backup being restored: the app's own from the moment its file is
/// chosen, so that leaving the History tab, or the screen, while a file of
/// years of sessions is read leaves it reading and restoring all the same.
/// One at a time: a file chosen while another is still read takes its
/// place, and the card says so at once; what the other holds is never read
/// as a backup, nor restored, however long its read takes, and a read that
/// never ends, as a browser's of a file moved after it was chosen, holds
/// nothing off.
#[derive(Clone, Copy)]
struct Restoring {
    /// What the card says while a file is read and restored.
    under_way: Signal<Option<String>>,
    /// Why the file chosen last could not be restored, said on the card.
    problem: Signal<Option<String>>,
    /// How many backup cards are drawn: with none, why a file could not be
    /// restored is said in a message instead.
    cards: CopyValue<usize>,
    /// How many files have been chosen: the last is the one restored.
    chosen: CopyValue<u64>,
}

impl Restoring {
    /// The app's, made in its root scope the first time a card asks for it,
    /// so that it lasts as long as the app does.
    fn use_app() -> Self {
        use_hook(|| {
            in_root(|| {
                try_consume_context::<Self>().unwrap_or_else(|| {
                    provide_context(Self {
                        under_way: Signal::new(None),
                        problem: Signal::new(None),
                        cards: CopyValue::new(0),
                        chosen: CopyValue::new(0),
                    })
                })
            })
        })
    }

    /// Read `file` as a backup and restore what it holds, in a task of the
    /// app's own: the card says it is being read at once, and until it has
    /// been restored, and why not, if it could not be; with no card drawn
    /// by then, a message does, once no session or run has the app. Unless
    /// another file is chosen before it is restored: then that one is, and
    /// nothing of this is said. The app makes it in its turn
    /// ([`AppCtx::restore_backup`]): once no session or run, nor another
    /// restore or the Undo of one, is under way.
    fn start(self, ctx: AppCtx, file: PickedFile) {
        let Self {
            mut under_way,
            mut problem,
            cards,
            mut chosen,
        } = self;
        let this = *chosen.peek() + 1;
        chosen.set(this);
        let chosen_last = move || *chosen.peek() == this;
        problem.set(None);
        under_way.set(Some(reading_for(&file.name, file.size)));
        spawn_forever(async move {
            // A browser reads it on the page's own thread: what the card
            // says is shown first, with how much room the browser says it
            // has.
            #[cfg(feature = "web")]
            {
                if let Some(room) = room_for(file.size).await
                    && chosen_last()
                {
                    let reading = reading_for(&file.name, file.size);
                    under_way.set(Some(format!("{reading} {room}")));
                }
                crate::time::sleep_ms(crate::time::POLL_MS).await;
            }
            // Another file chosen meanwhile is read in its place: this one
            // is not read at all, or, chosen over while it is read, not read
            // as a backup, which on a computer takes a thread of its own.
            if !chosen_last() {
                return;
            }
            let read = match text_in(file).await {
                Ok(_) if !chosen_last() => return,
                Ok((name, text)) => backup_in(&name, text).await,
                Err(why) => Err(why),
            };
            // Another file chosen meanwhile is restored in its place.
            if !chosen_last() {
                return;
            }
            let backup = match read {
                Ok(backup) => backup,
                Err(why) => {
                    under_way.set(None);
                    // No card is drawn over a session or run, and no
                    // message is said over it either: it is said once that
                    // is over, where a card is drawn by then, on the card.
                    while *cards.peek() == 0 && ctx.session_running() {
                        crate::time::sleep_ms(SESSION_WAIT_MS).await;
                    }
                    if !chosen_last() {
                        return;
                    }
                    if *cards.peek() > 0 {
                        problem.set(Some(why));
                    } else {
                        ctx.toasts.show(why);
                    }
                    return;
                }
            };
            // Said on the card until the store has said what became of this
            // restore, as a browser's database takes a moment to: of this
            // one, not of an Undo clicked as soon as it is told of. Another
            // file chosen while it still waits its turn is restored in its
            // place, once read: this one is made no more. While it waits for
            // another window's restore, or the Undo of it, the card says so,
            // and looks at it as often as the app looks at that window's
            // hold on the restores. While it waits for a session or run to
            // be over, it is looked at once a second, as the app looks at
            // the session.
            let reading = under_way.peek().clone();
            let ticket = ctx.restore_backup(backup);
            while !ctx.restore_told(ticket) {
                #[cfg(test)]
                crate::testing::rendered("restore wait");
                if !chosen_last() {
                    ctx.withdraw_restore(ticket);
                    return;
                }
                let said = if ctx.restore_blocked(ticket) {
                    Some(WAITING_ELSEWHERE)
                } else {
                    reading.as_deref()
                };
                if under_way.peek().as_deref() != said {
                    under_way.set(said.map(str::to_string));
                }
                let wait = if ctx.session_running() {
                    SESSION_WAIT_MS
                } else if said == Some(WAITING_ELSEWHERE) {
                    RESTORE_LOCK_POLL_MS
                } else {
                    crate::time::POLL_MS
                };
                crate::time::sleep_ms(wait).await;
            }
            if chosen_last() {
                under_way.set(None);
            }
        });
    }
}

/// Save a backup, or restore one; and when `since` sessions have gone
/// unbacked for long enough, say so.
#[component]
pub fn BackupCard(
    backups: Backups,
    /// The sessions finished since the last backup, or all of them.
    since: usize,
) -> Element {
    let ctx = use_context::<AppCtx>();
    let restoring = Restoring::use_app();
    // Drawn, a card says why a file could not be restored, for as long as
    // one is drawn.
    use_hook(move || {
        let mut cards = restoring.cards;
        let drawn = *cards.peek() + 1;
        cards.set(drawn);
    });
    use_drop(move || {
        let (mut cards, mut problem) = (restoring.cards, restoring.problem);
        let drawn = cards.peek().saturating_sub(1);
        cards.set(drawn);
        if drawn == 0 {
            problem.set(None);
        }
    });
    let Restoring {
        under_way, problem, ..
    } = restoring;
    // Asked once: the browser says where it runs as the page opens.
    let accept = use_hook(backup_accept);
    let Backups {
        last,
        can_save,
        confirmed,
        inside_app,
        outgrows_device_backup,
        on_save,
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
            if can_save && outgrows_device_backup {
                p { class: "backup-note", "{OUTGROWN}" }
            }
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
                    on_pick: move |file: PickedFile| restoring.start(ctx, file),
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
