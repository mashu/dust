//! Backups on the History tab, in the whole app: saving one, the word when
//! one is due, restoring one, and putting things back after a restore.
//!
//! What the store holds is read alongside the page, since a restore that
//! only reached the screen would be gone at the next launch.

use std::rc::Rc;

use cw_core::{SessionResult, TrainingSettings};
use dioxus::prelude::*;

use crate::persist::backup::{read_backup, write_backup};
use crate::persist::memory::MemoryStore;
use crate::persist::{OpenSession, Store};
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

/// A session stamped ahead of the clock is not new since every backup
/// until its stamp comes: it counts as finished just after the latest that
/// was not, so a backup saved after that one holds it. History still lists
/// it, and the store keeps it, as it was stamped.
#[test]
fn a_session_stamped_ahead_of_the_clock_is_not_new_since_every_backup() {
    run(|| async {
        let mut sessions = history(12);
        let ahead = crate::time::now_ms() + 365 * 86_400_000;
        sessions[11].timestamp = ahead;
        let store = MemoryStore::default();
        store.mark_backup(11 * 86_400_000 + 2);
        let ui = open(store, &sessions);
        assert!(ui.has("nothing new since"), "{}", ui.html());
        assert!(ui.has("12 sessions on this device"));
        assert_eq!(ui.store.load_sessions()[11].timestamp, ahead);
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
/// the card says why. Too large is past a gigabyte: a backup of fifty
/// megabytes or more, which used to be refused, is a history of years.
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
        ui.choose_file("file-backup", "film.mkv", "{}", 1_100_000_000);
        ui.advance(20).await;
        assert!(ui.has("film.mkv is far larger than any backup: nothing was restored."));
        assert_eq!(stored_dates(&ui).len(), 2);
    });
}

/// A large backup is read and restored, however large, up to a gigabyte:
/// the card says, as it reads it, that one so large takes a while, and the
/// message says the sessions are being restored until they are, then what
/// was restored, with Undo.
#[test]
fn a_large_backup_says_it_takes_a_while_and_restores_all_the_same() {
    run(|| async {
        let all = history(5);
        let store = MemoryStore::default();
        let mut ui = open(store, &all[..1]);
        let file = write_backup(&test_settings(), &all, 0);
        let large = 600_000_000;
        assert_eq!(
            super::backup::reading_for("big.json", large),
            "Reading big.json, 600 MB: a backup this large takes a while to restore."
        );
        assert_eq!(
            super::backup::reading_for("small.json", file.len() as u64),
            "Reading small.json…"
        );
        // The store answers a moment later, as the history's own thread does.
        ui.store.deferred.set(true);
        ui.choose_file("file-backup", "big.json", &file, large);
        ui.advance(20).await;
        assert!(ui.has("Restoring 4 sessions…"), "{}", ui.html());
        assert_eq!(stored_dates(&ui).len(), 1, "not stored yet");
        ui.store.settle();
        ui.advance(20).await;
        assert!(
            ui.has("Restored 4 sessions and your settings."),
            "{}",
            ui.html()
        );
        assert!(ui.shows("toast-undo"));
        assert_eq!(stored_dates(&ui).len(), 5);
    });
}

/// A backup chosen is the app's to restore from the moment it is chosen:
/// the card says it is being read at once, and leaving the tab, or the
/// screen, while it is read leaves it restoring all the same, its message
/// said wherever the app is by then. Restoring was the card's own, and was
/// dropped, unsaid, with the tab.
#[test]
fn a_restore_goes_on_when_its_tab_is_left() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[3..]);
        let mut saved = test_settings();
        saved.curriculum.level = 9;
        let file = write_backup(&saved, &all, 0);
        ui.hold_file_reads(true);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        assert!(ui.has("Reading dust-backup.json…"), "{}", ui.html());
        ui.click("seg-overview");
        ui.click("nav-practice");
        ui.hold_file_reads(false);
        ui.advance(100).await;
        assert!(
            ui.has("Restored 3 sessions and your settings."),
            "{}",
            ui.html()
        );
        assert_eq!(stored_dates(&ui).len(), 5);
        assert_eq!(ui.store.load_settings().curriculum.level, 9);
        ui.click("nav-stats");
        ui.click("seg-history");
        assert!(!ui.has("Reading dust-backup.json"), "{}", ui.html());
    });
}

/// A backup read while a session is under way is restored once it is over,
/// as a sync is, never under it: a restore takes its settings, the level
/// among them, which a session is not to have changed beneath it.
#[test]
fn a_backup_read_during_a_session_is_restored_once_it_is_over() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[3..]);
        let mut saved = test_settings();
        saved.curriculum.level = 9;
        let file = write_backup(&saved, &all, 0);
        ui.hold_file_reads(true);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        ui.click("nav-practice");
        ui.click("btn-start-training");
        ui.hold_file_reads(false);
        ui.advance(3_000).await;
        assert_eq!(stored_dates(&ui).len(), 2, "not under the session");
        assert_ne!(ui.with_app(|ctx| ctx.settings.peek().curriculum.level), 9);
        ui.click("btn-end-session");
        ui.advance(3_000).await;
        assert_eq!(stored_dates(&ui).len(), 5, "{}", ui.html());
        assert_eq!(ui.with_app(|ctx| ctx.settings.peek().curriculum.level), 9);
    });
}

/// A file that is no backup, read while its tab is left, is said in a
/// message, as the card is not there to say it; the card, opened again,
/// says nothing of it.
#[test]
fn a_file_found_no_backup_once_its_tab_is_left_is_said_in_a_message() {
    run(|| async {
        let mut ui = open(MemoryStore::default(), &history(2));
        ui.hold_file_reads(true);
        ui.choose_file("file-backup", "notes.txt", "hello", 5);
        ui.click("seg-overview");
        ui.hold_file_reads(false);
        ui.advance(100).await;
        let said = "notes.txt is not a Dust backup: nothing was restored.";
        let toast = ui.with_app(|ctx| ctx.toasts.current().map(|toast| toast.text));
        assert_eq!(toast.as_deref(), Some(said), "{}", ui.html());
        ui.click("seg-history");
        assert!(!ui.has("backup-problem"), "{}", ui.html());
        assert_eq!(stored_dates(&ui).len(), 2);
    });
}

/// One backup is restored at a time: another chosen while one is read takes
/// its place, the card saying at once that it is the one read, and it alone
/// is restored, whenever the first is done. Both were read, and the first
/// done said nothing was under way while the other was; then both were
/// merged in, with both their settings. Then the second was left alone
/// while the first was read, never restored, and nothing said so.
#[test]
fn a_backup_chosen_while_another_is_read_takes_its_place() {
    run(|| async {
        let all = history(7);
        let mut ui = open(MemoryStore::default(), &all[3..5]);
        let mut first = test_settings();
        first.curriculum.level = 9;
        let first = write_backup(&first, &all[..3], 0);
        let mut second = test_settings();
        second.curriculum.level = 4;
        let second = write_backup(&second, &all[5..], 0);
        ui.hold_file_reads(true);
        ui.choose_file("file-backup", "first.json", &first, first.len() as u64);
        ui.choose_file("file-backup", "second.json", &second, second.len() as u64);
        assert!(ui.has("Reading second.json…"), "{}", ui.html());
        assert!(!ui.has("Reading first.json"), "{}", ui.html());
        ui.hold_file_reads(false);
        ui.advance(100).await;
        assert!(ui.has("Restored 2 sessions and your settings."));
        assert_eq!(stored_dates(&ui).len(), 4, "the second backup's alone");
        assert_eq!(ui.store.load_settings().curriculum.level, 4);
        assert!(!ui.has("Reading"), "{}", ui.html());
        // Once it is done, another is restored as any is.
        ui.choose_file("file-backup", "first.json", &first, first.len() as u64);
        ui.advance(100).await;
        assert_eq!(stored_dates(&ui).len(), 7);
        assert_eq!(ui.store.load_settings().curriculum.level, 9);
    });
}

/// A backup chosen over another still read is the only one read as a
/// backup: the other's file, once read, is let go without being read as
/// one. It was read as a backup all the same, a file of up to a gigabyte
/// on a thread of its own on a computer, and only then let go.
#[test]
fn a_backup_chosen_over_another_is_the_only_one_read_as_a_backup() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[3..]);
        let file = write_backup(&test_settings(), &all, 0);
        ui.hold_file_reads(true);
        ui.choose_file("file-backup", "first.json", &file, file.len() as u64);
        ui.choose_file("file-backup", "second.json", &file, file.len() as u64);
        ui.take_renders("backup read");
        ui.hold_file_reads(false);
        ui.advance(100).await;
        assert!(
            ui.has("Restored 3 sessions and your settings."),
            "{}",
            ui.html()
        );
        assert_eq!(ui.take_renders("backup read"), 1, "the second's alone");
    });
}

/// The card says a backup is under way until the store has said what
/// became of it, as a browser's database takes a moment to: a file chosen
/// meanwhile is not restored over it. The card stopped saying so as the
/// store was asked, with nothing restored yet.
#[test]
fn the_card_says_a_backup_is_under_way_until_it_is_restored() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[3..]);
        let file = write_backup(&test_settings(), &all, 0);
        ui.store.deferred.set(true);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        ui.advance(100).await;
        assert_eq!(stored_dates(&ui).len(), 2, "not stored yet");
        assert!(ui.has("Reading dust-backup.json…"), "{}", ui.html());
        ui.store.settle();
        ui.advance(100).await;
        assert!(
            ui.has("Restored 3 sessions and your settings."),
            "{}",
            ui.html()
        );
        assert!(!ui.has("Reading"), "{}", ui.html());
        assert_eq!(stored_dates(&ui).len(), 5);
    });
}

/// The card stops saying a backup is being read once its own restore has
/// been told of, though the restore's Undo is clicked at once, before the
/// card looks again: an Undo under way is no restore of the card's. The
/// card went on saying it read the backup until the Undo was over.
#[test]
fn the_card_says_its_restore_is_over_though_an_undo_follows_at_once() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[3..]);
        let file = write_backup(&test_settings(), &all, 0);
        ui.store.deferred.set(true);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        ui.advance(100).await;
        assert!(ui.has("Reading dust-backup.json…"), "{}", ui.html());
        // Told, and its Undo clicked, with no time passing for the card.
        ui.store.settle();
        ui.settle();
        assert!(
            ui.has("Restored 3 sessions and your settings."),
            "{}",
            ui.html()
        );
        ui.click("toast-undo");
        ui.advance(100).await;
        assert!(!ui.has("Reading"), "{}", ui.html());
        ui.store.settle();
        ui.advance(100).await;
        assert_eq!(stored_dates(&ui), ["2026-09-04", "2026-09-05"], "undone");
    });
}

/// A backup chosen while another is being restored is restored once that
/// is over, merged with the history it restored: its message says what it
/// brought itself, and its Undo takes back that alone. Merged with a
/// history short of the first's sessions, it took them for its own, said
/// so, and its Undo deleted them too.
#[test]
fn a_backup_chosen_while_another_is_restored_waits_for_it() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[4..]);
        let first = write_backup(&test_settings(), &all[..2], 0);
        let second = write_backup(&test_settings(), &all[..3], 0);
        ui.store.deferred.set(true);
        ui.choose_file("file-backup", "first.json", &first, first.len() as u64);
        ui.advance(100).await;
        ui.choose_file("file-backup", "second.json", &second, second.len() as u64);
        ui.advance(100).await;
        assert_eq!(ui.store.waiting(), 1, "the second waits for the first");
        assert!(ui.has("Reading second.json…"), "{}", ui.html());
        ui.store.settle();
        ui.advance(100).await;
        ui.store.settle();
        ui.advance(100).await;
        assert!(
            ui.has("Restored 1 session and your settings."),
            "{}",
            ui.html()
        );
        assert!(!ui.has("Reading"), "{}", ui.html());
        assert_eq!(stored_dates(&ui).len(), 4);
        ui.store.deferred.set(false);
        ui.click("toast-undo");
        ui.advance(20).await;
        assert_eq!(
            stored_dates(&ui),
            ["2026-09-01", "2026-09-02", "2026-09-05"]
        );
    });
}

/// A backup chosen while the Undo of a restore is under way, as a
/// browser's database makes it a moment after it is asked for, is restored
/// once the Undo is over, merged with the history it left: what both hold
/// is restored. Merged with a history that still held it, it was taken for
/// sessions already here, and the Undo deleted them.
#[test]
fn a_backup_chosen_while_a_restore_is_undone_waits_for_it() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[4..]);
        let first = write_backup(&test_settings(), &all[..2], 0);
        let second = write_backup(&test_settings(), &all[..3], 0);
        ui.choose_file("file-backup", "first.json", &first, first.len() as u64);
        ui.advance(20).await;
        assert_eq!(stored_dates(&ui).len(), 3);
        ui.store.deferred.set(true);
        ui.click("toast-undo");
        ui.choose_file("file-backup", "second.json", &second, second.len() as u64);
        ui.advance(100).await;
        assert_eq!(ui.store.waiting(), 1, "the second waits for the Undo");
        assert!(ui.has("Reading second.json…"), "{}", ui.html());
        ui.store.settle();
        ui.advance(100).await;
        ui.store.settle();
        ui.advance(100).await;
        assert!(
            ui.has("Restored 3 sessions and your settings."),
            "{}",
            ui.html()
        );
        assert!(!ui.has("Reading"), "{}", ui.html());
        assert_eq!(
            stored_dates(&ui),
            ["2026-09-01", "2026-09-02", "2026-09-03", "2026-09-05"]
        );
    });
}

/// A backup chosen while the Undo of a restore is under way is handed to
/// the app as soon as it is read, and waits its turn there, as every
/// restore asked for does: one way to make restores one at a time, the
/// app's. The card held it back itself until nothing was under way, so the
/// app's own wait was reached by no restore the card made.
#[test]
fn a_backup_chosen_while_a_restore_is_undone_waits_in_the_apps_queue() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[4..]);
        let first = write_backup(&test_settings(), &all[..2], 0);
        let second = write_backup(&test_settings(), &all[..3], 0);
        ui.choose_file("file-backup", "first.json", &first, first.len() as u64);
        ui.advance(20).await;
        ui.store.deferred.set(true);
        ui.click("toast-undo");
        ui.choose_file("file-backup", "second.json", &second, second.len() as u64);
        ui.advance(100).await;
        assert!(
            ui.with_app(|ctx| ctx.restore_waits()),
            "waiting in the app's queue"
        );
        assert!(ui.has("Reading second.json…"), "{}", ui.html());
        ui.store.settle();
        ui.advance(100).await;
        ui.store.settle();
        ui.advance(100).await;
        assert!(
            ui.has("Restored 3 sessions and your settings."),
            "{}",
            ui.html()
        );
        assert!(!ui.has("Reading"), "{}", ui.html());
    });
}

/// An Undo that fails while a backup chosen meanwhile waits for it puts
/// back no Undo of the restore it could not undo: the backup waiting is
/// the last restore asked for, made once the failure is told of, and has
/// an Undo of its own, which takes back what it brought alone.
#[test]
fn an_undo_that_fails_while_a_backup_waits_offers_no_undo_again() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[4..]);
        let first = write_backup(&test_settings(), &all[..2], 0);
        let second = write_backup(&test_settings(), &all[..3], 0);
        ui.choose_file("file-backup", "first.json", &first, first.len() as u64);
        ui.advance(20).await;
        assert_eq!(stored_dates(&ui).len(), 3);
        ui.store.deferred.set(true);
        ui.click("toast-undo");
        ui.choose_file("file-backup", "second.json", &second, second.len() as u64);
        ui.advance(100).await;
        // The Undo finds no room to be made in.
        ui.store.full.set(true);
        ui.store.settle();
        ui.store.full.set(false);
        ui.advance(100).await;
        ui.store.settle();
        ui.advance(100).await;
        assert!(
            ui.has("Restored 1 session and your settings."),
            "{}",
            ui.html()
        );
        assert!(ui.shows("toast-undo"), "the second's own Undo");
        ui.store.deferred.set(false);
        ui.click("toast-undo");
        ui.advance(20).await;
        assert_eq!(
            stored_dates(&ui),
            ["2026-09-01", "2026-09-02", "2026-09-05"],
            "what the second brought, taken back alone"
        );
    });
}

/// A file whose read never ends, as a browser's of a file moved after it
/// was chosen never does, leaves the next one chosen to be restored: the
/// card says at once that the newer is being read, and what it holds is
/// restored. The first held off every restore after it until the app was
/// opened again, the card saying for good that it was being read.
#[test]
fn a_backup_chosen_while_another_is_never_read_is_restored() {
    run(|| async {
        let all = history(5);
        let mut ui = open(MemoryStore::default(), &all[3..]);
        let mut saved = test_settings();
        saved.curriculum.level = 9;
        let file = write_backup(&saved, &all, 0);
        ui.never_finish_reading("moved.json");
        ui.choose_file("file-backup", "moved.json", &file, file.len() as u64);
        ui.advance(100).await;
        assert!(ui.has("Reading moved.json…"), "{}", ui.html());
        ui.hold_file_reads(true);
        ui.choose_file("file-backup", "dust-backup.json", &file, file.len() as u64);
        assert!(ui.has("Reading dust-backup.json…"), "{}", ui.html());
        assert!(!ui.has("Reading moved.json"), "{}", ui.html());
        ui.hold_file_reads(false);
        ui.advance(100).await;
        assert!(
            ui.has("Restored 3 sessions and your settings."),
            "{}",
            ui.html()
        );
        assert_eq!(stored_dates(&ui).len(), 5);
        assert_eq!(ui.store.load_settings().curriculum.level, 9);
        assert!(!ui.has("Reading"), "{}", ui.html());
    });
}

/// A history past what Android's own backup of the app takes is said so
/// on the card, which says to save a backup file; one that is not, and an
/// app with nowhere to save one, say nothing of it.
#[test]
fn a_history_past_androids_own_backup_says_to_save_a_backup() {
    run(|| async {
        let said = "Android&#39;s own backup of an app stops at 25 MB";
        let ui = open(MemoryStore::default(), &history(3));
        assert!(!ui.has(said));
        let store = MemoryStore::default();
        store.device_backup_outgrown.set(true);
        let ui = open(store, &history(3));
        assert!(
            ui.has("Your history here is past 20 MB, and Android&#39;s own backup of an app stops at 25 MB: save a backup file now and then"),
            "{}",
            ui.html()
        );
        let store = MemoryStore::default();
        store.device_backup_outgrown.set(true);
        store.no_backups.set(true);
        assert!(!open(store, &history(3)).has(said));
    });
}

/// Where the history stored cannot be read, a backup holds every session
/// shown all the same, the one the store had no room for among them: a
/// backup is how that one is kept safe meanwhile. Saving one used to fail,
/// saying the history stored here could not be read.
#[test]
fn a_backup_holds_every_session_shown_where_the_store_cannot_be_read() {
    run(|| async {
        let store = MemoryStore::default();
        store.full.set(true);
        // Cut off as the app stopped, finished and not yet stored: kept as
        // the app starts, and not stored for want of room.
        let mut kept = stored_session("2026-09-20", &[("KM", "KM")]);
        kept.timestamp = 20 * 86_400_000;
        store.mark_open(Some(OpenSession {
            unstored: vec![kept.clone()],
            over: true,
            ..OpenSession::starting(false, kept.timestamp)
        }));
        let mut ui = open(store, &history(3));
        assert_eq!(stored_dates(&ui).len(), 3, "the fourth only shown");
        ui.store.unlistable.set(true);

        ui.click("btn-save-a-backup");
        let files = ui.store.backup_files.borrow().clone();
        assert_eq!(files.len(), 1);
        let (name, contents) = &files[0];
        let backup = read_backup(contents).expect("a backup");
        let dates: Vec<String> = backup.sessions.into_iter().map(|s| s.date).collect();
        assert_eq!(
            dates,
            ["2026-09-01", "2026-09-02", "2026-09-03", "2026-09-20"]
        );
        assert!(
            ui.has(&format!("Backup saved as {name}, in your downloads.")),
            "{}",
            ui.html()
        );
        assert!(!ui.has("could not be saved"));
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
