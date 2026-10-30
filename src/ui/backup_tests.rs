//! Backups on the History tab, in the whole app: saving one, the word when
//! one is due, restoring one, and putting things back after a restore.
//!
//! What the store holds is read alongside the page, since a restore that
//! only reached the screen would be gone at the next launch.

use std::rc::Rc;

use cw_core::{SessionResult, TrainingSettings};

use crate::persist::Store;
use crate::persist::backup::{read_backup, write_backup};
use crate::persist::memory::MemoryStore;
use crate::testing::{Ui, run, stored_session, test_settings};

/// `count` sessions a day apart, oldest first, the way the store keeps
/// them.
fn history(count: u64) -> Vec<SessionResult> {
    (1..=count)
        .map(|day| {
            let mut session = stored_session(&format!("2026-09-{day:02}"), &[("KM", "KM")]);
            session.timestamp = day * 86_400_000;
            session
        })
        .collect()
}

fn stored_dates(ui: &Ui) -> Vec<String> {
    ui.store
        .load_sessions()
        .into_iter()
        .map(|session| session.date)
        .collect()
}

/// The app on the History tab, launched on `store` holding `sessions`.
fn open(store: MemoryStore, sessions: &[SessionResult]) -> Ui {
    store.save_settings(&test_settings());
    store.save_sessions(sessions);
    store.mark_tutorial_done();
    let (mut ui, _recorder) = Ui::launch(Rc::new(store));
    ui.click("nav-stats");
    ui.click("seg-history");
    ui
}

/// A backup holds the history and the settings; the card then says when it
/// was saved, and the store remembers.
#[test]
fn a_backup_is_saved_with_everything_and_remembered() {
    run(|| async {
        let mut ui = open(MemoryStore::default(), &history(5));
        assert!(ui.has("No backup yet"));
        assert!(!ui.has("backup-card due"), "five sessions are not many");

        ui.click("btn-save-a-backup");
        let files = ui.store.backup_files.borrow().clone();
        assert_eq!(files.len(), 1);
        let (name, contents) = &files[0];
        assert!(
            name.starts_with("dust-backup-") && name.ends_with(".json"),
            "{name}"
        );
        let backup = read_backup(contents).expect("a backup");
        assert_eq!(backup.sessions.len(), 5);
        assert_eq!(backup.settings, Some(test_settings().clamp()));
        assert_eq!(ui.store.last_backup(), Some(backup.saved_at));
        assert!(ui.has(&format!("Backup saved as {name}, in your downloads.")));
        assert!(ui.has("nothing new since"));
    });
}

/// Ten sessions without a backup, and the card says so; a backup quiets it.
#[test]
fn the_card_says_when_a_backup_is_due() {
    run(|| async {
        let mut ui = open(MemoryStore::default(), &history(12));
        assert!(ui.has("backup-card due"));
        assert!(ui.has("12 sessions and no backup yet"));
        ui.click("btn-save-a-backup");
        assert!(!ui.has("backup-card due"));

        // Backed up after the ninth session: three since, which is not many.
        let store = MemoryStore::default();
        store.mark_backup(9 * 86_400_000 + 1);
        let ui = open(store, &history(12));
        assert!(ui.has("3 sessions since"));
        assert!(!ui.has("backup-card due"));

        // Backed up after the first: eleven since.
        let store = MemoryStore::default();
        store.mark_backup(86_400_000 + 1);
        let ui = open(store, &history(12));
        assert!(ui.has("backup-card due"));
        assert!(ui.has("11 sessions since your last backup"));
    });
}

/// Where a backup file is saved inside the app, as on an iPhone, it goes
/// with the app until it is moved out: the message and the card say so,
/// and how. It used to count as safe, the card saying nothing new since.
#[test]
fn a_backup_kept_inside_the_app_says_to_move_it_out() {
    run(|| async {
        let store = MemoryStore::default();
        store.backups_inside.set(Some("with the Files app,"));
        let mut ui = open(store, &history(12));
        assert!(ui.has(
            "12 sessions and no backup yet. Save one and move it out of Dust&#39;s folder with the Files app, to keep them safe"
        ));
        ui.click("btn-save-a-backup");
        assert!(
            ui.has("It goes if Dust is deleted: move it out with the Files app, to keep it."),
            "{}",
            ui.html()
        );
        assert!(
            ui.has("in Dust&#39;s folder · nothing new since"),
            "{}",
            ui.html()
        );
        assert!(ui.has(
            "A backup file is saved in Dust&#39;s own folder, which goes if Dust is deleted: move each one out with the Files app, to keep your history and settings safe."
        ));
        assert!(!ui.has("backup-card due"));
    });
}

/// A browser's download is only handed over, and the browser says nothing
/// if it blocks it: the card tells of the last backup as handed to the
/// browser, not as saved, and the reminder goes quiet all the same.
#[test]
fn a_download_is_told_of_as_handed_over() {
    run(|| async {
        let store = MemoryStore::default();
        store.unconfirmed_backups.set(true);
        let mut ui = open(store, &history(12));
        assert!(ui.has("backup-card due"));
        ui.click("btn-save-a-backup");
        assert!(ui.has("Last handed to your browser"));
        assert!(!ui.has("Last saved"));
        assert!(!ui.has("backup-card due"));
    });
}

/// A restore adds the sessions missing here and takes the backup's
/// settings; Undo puts both back as they were.
#[test]
fn a_restore_puts_back_what_is_missing_and_can_be_undone() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[3..]);
        assert_eq!(stored_dates(&ui), ["2026-09-04", "2026-09-05"]);

        let mut saved = test_settings();
        saved.curriculum.level = 9;
        let saved_at = 6 * 86_400_000;
        let file = write_backup(&saved, &all, saved_at);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        ui.advance(20).await;
        assert_eq!(
            stored_dates(&ui),
            [
                "2026-09-01",
                "2026-09-02",
                "2026-09-03",
                "2026-09-04",
                "2026-09-05"
            ]
        );
        assert!(ui.has("Restored 3 sessions and your settings from"));
        assert!(ui.has("5 sessions on this device"));
        assert_eq!(ui.store.load_settings().curriculum.level, 9);
        assert_eq!(ui.store.last_backup(), Some(saved_at));
        assert_eq!(ui.store.keep_asks.get(), 1);

        ui.click("toast-undo");
        ui.advance(20).await;
        assert_eq!(stored_dates(&ui), ["2026-09-04", "2026-09-05"]);
        assert_eq!(
            ui.store.load_settings().curriculum.level,
            test_settings().curriculum.level
        );
    });
}

/// The card says what a restore does; a restore takes the backup's
/// settings, the level among them, but keeps this device's volume and
/// paddle keys, and says so. The card said it put back whatever was missing
/// here, and a restore set a computer's volume and paddles as a phone had
/// them.
#[test]
fn a_restore_keeps_this_devices_volume_and_paddle_keys() {
    run(|| async {
        let all = history(2);
        let mut ui = open(MemoryStore::default(), &all[1..]);
        assert!(
            ui.has("Restoring one adds the sessions missing here and takes its settings, level included, though not this device&#39;s volume and paddle keys."),
            "{}",
            ui.html()
        );
        let here = test_settings().clamp();
        let mut saved = test_settings();
        saved.curriculum.level = 9;
        saved.band.master_volume = here.band.master_volume / 2.0;
        saved.playback.paddle_swap = !here.playback.paddle_swap;
        let file = write_backup(&saved, &all, 0);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        ui.advance(20).await;
        assert!(
            ui.has("Restored 1 session and your settings, keeping this device&#39;s volume and paddle keys."),
            "{}",
            ui.html()
        );
        let stored = ui.store.load_settings();
        assert_eq!(stored.curriculum.level, 9);
        assert_eq!(stored.band.master_volume, here.band.master_volume);
        assert_eq!(stored.playback.paddle_swap, here.playback.paddle_swap);
    });
}

/// Restoring the same backup again changes nothing, and says so.
#[test]
fn a_backup_already_restored_adds_nothing() {
    run(|| async {
        let all = history(3);
        let mut ui = open(MemoryStore::default(), &all);
        let file = write_backup(&test_settings(), &all, 0);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        ui.advance(20).await;
        assert!(ui.has("Every session in it was already here."));
        assert_eq!(stored_dates(&ui).len(), 3);
    });
}

/// A run whose log an older version dropped takes it back from a backup
/// that kept it, and the message counts it beside what was added.
#[test]
fn a_restore_fills_in_a_runs_dropped_log() {
    run(|| async {
        let mut kept =
            crate::testing::stored_run("2026-09-02", &[("K5XYZ", "214", "K5XYZ", "214")]);
        kept.timestamp = 2 * 86_400_000;
        let dropped = SessionResult {
            contest: None,
            ..kept.clone()
        };
        let here = vec![history(1).remove(0), dropped];
        let mut ui = open(MemoryStore::default(), &here);
        let mut backup = history(3);
        backup[1] = kept;
        let file = write_backup(&test_settings(), &backup, 0);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        ui.advance(20).await;
        assert!(
            ui.has("Restored 1 session and filled in 1, with your settings."),
            "{}",
            ui.html()
        );
        let stored = ui.store.load_sessions();
        assert_eq!(stored.len(), 3);
        assert!(stored[1].contest.is_some(), "the run has its log back");
    });
}

/// A backup that only fills in a log adds no session, and says what it
/// filled in.
#[test]
fn a_restore_that_only_fills_in_says_so() {
    run(|| async {
        let mut kept =
            crate::testing::stored_run("2026-09-01", &[("K5XYZ", "214", "K5XYZ", "214")]);
        kept.timestamp = 86_400_000;
        let dropped = SessionResult {
            contest: None,
            ..kept.clone()
        };
        let mut ui = open(MemoryStore::default(), &[dropped]);
        let file = write_backup(&test_settings(), &[kept], 0);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        ui.advance(20).await;
        assert!(
            ui.has("Filled in 1 session and restored your settings."),
            "{}",
            ui.html()
        );
        let stored = ui.store.load_sessions();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].contest.is_some(), "the run has its log back");
    });
}

/// A file that is not a backup, or far too large to be one, is refused, and
/// the card says why.
#[test]
fn a_file_that_is_not_a_backup_is_refused() {
    run(|| async {
        let mut ui = open(MemoryStore::default(), &history(2));
        ui.choose_file("file-backup", "notes.txt", "hello", 5);
        ui.advance(20).await;
        assert!(ui.has("notes.txt is not a Dust backup: nothing was restored."));
        let settings = serde_json::to_string(&TrainingSettings::default()).unwrap();
        ui.choose_file(
            "file-backup",
            "settings.json",
            &settings,
            settings.len() as u64,
        );
        ui.advance(20).await;
        assert!(ui.has("settings.json is not a Dust backup"));
        ui.choose_file("file-backup", "film.mkv", "{}", 900_000_000);
        ui.advance(20).await;
        assert!(ui.has("film.mkv is far larger than any backup: nothing was restored."));
        assert_eq!(stored_dates(&ui).len(), 2);
    });
}

/// An app with nowhere to put a file can still restore one, and does not
/// ask for a backup it cannot make.
#[test]
fn a_build_that_cannot_save_a_backup_can_still_restore_one() {
    run(|| async {
        let store = MemoryStore::default();
        store.no_backups.set(true);
        let ui = open(store, &history(12));
        assert!(!ui.shows("btn-save-a-backup"));
        assert!(ui.shows("file-backup"));
        assert!(!ui.has("backup-card due"));
        assert!(ui.has("Bring in a backup saved by the web app"));
    });
}

/// Off Android the picker offers backups, the JSON files. On Android, the
/// app or the web app in a browser there, it offers every file: a picker
/// there that filters by type hides a backup another app stored untyped.
#[test]
fn the_restore_picker_offers_backups_off_android() {
    run(|| async {
        let ui = open(MemoryStore::default(), &history(1));
        let input = ui.input_tag("file-backup");
        assert!(
            input.contains(r#"accept=".json,application/json""#),
            "{input}"
        );
    });
}
