//! The history on the Stats screen, in the whole app: deleting one session,
//! choosing several, deleting them all, and putting them back.
//!
//! What the store holds is read alongside the page, since a deletion that
//! only reached the screen would come back at the next launch.

use cw_core::{CharSetMode, SessionHistoryRow, SessionResult, TrainingSettings};
use dioxus::prelude::*;

use crate::persist::Store;
use crate::testing::{Ui, run, stored_run, stored_session, test_settings};
use crate::ui::figures::History;
use crate::ui::stats::StatsView;
use crate::ui::stats_detail::HISTORY_PAGE;

/// Five sessions a day apart, oldest first, the way the store keeps them.
fn history() -> Vec<SessionResult> {
    (1..=5u64)
        .map(|day| {
            let mut session = stored_session(&format!("2026-10-0{day}"), &[("KM", "KM")]);
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

/// The app on the History tab of the Stats screen, over `sessions`.
fn open_history(sessions: &[SessionResult]) -> (Ui, std::rc::Rc<crate::audio::fake::Recorder>) {
    let (mut ui, recorder) = Ui::app_with_history(test_settings(), sessions);
    ui.click("nav-stats");
    ui.click("seg-history");
    (ui, recorder)
}

/// The board is read off what is kept: a deleted session leaves it, and
/// Undo puts it back.
#[test]
fn a_deleted_session_leaves_the_score_board() {
    run(|| async {
        let long: Vec<SessionResult> = (1..=5u64)
            .map(|day| {
                let mut session = stored_session(&format!("2026-10-0{day}"), &[("KM", "KM"); 10]);
                session.timestamp = day * 86_400_000;
                session
            })
            .collect();
        let (mut ui, _recorder) = open_history(&long);
        ui.click("seg-scores");
        assert_eq!(ui.controls("board-").len(), 5);
        assert!(ui.has("Fri 2 Oct 2026"));

        ui.click("seg-history");
        ui.click("history-delete-1");
        ui.click("seg-scores");
        assert_eq!(ui.controls("board-").len(), 4);
        assert!(!ui.has("Fri 2 Oct 2026"), "the 2nd of October is gone");

        ui.click("toast-undo");
        assert_eq!(ui.controls("board-").len(), 5);
    });
}

#[test]
fn a_session_is_deleted_from_its_row_and_undo_puts_it_back() {
    run(|| async {
        let (mut ui, _recorder) = open_history(&history());
        assert!(ui.has("5 sessions on this device"));

        // The 2nd of October is the second session stored.
        ui.click("history-delete-1");
        assert!(!ui.has("2026-10-02"));
        assert!(ui.has("4 sessions on this device"));
        assert_eq!(
            stored_dates(&ui),
            ["2026-10-01", "2026-10-03", "2026-10-04", "2026-10-05"]
        );
        assert!(ui.has("1 session deleted"));

        ui.click("toast-undo");
        assert!(ui.has("2026-10-02"));
        assert_eq!(ui.store.load_sessions(), history(), "back where it was");
        assert!(!ui.has("class=\"toast\""), "the undo toast goes once used");
    });
}

#[test]
fn several_sessions_are_chosen_and_deleted_together() {
    run(|| async {
        let (mut ui, _recorder) = open_history(&history());
        assert!(
            !ui.shows("history-delete-chosen"),
            "no bar until select mode"
        );
        ui.click("history-select");
        assert!(ui.has("0 selected"));
        assert!(
            ui.input_tag("history-delete-chosen").contains("disabled"),
            "nothing chosen, nothing to delete"
        );
        assert!(!ui.shows("history-delete-0"), "rows are for choosing now");

        ui.click("history-pick-0");
        ui.click("history-pick-3");
        assert!(ui.has("2 selected"));
        assert!(
            ui.input_tag("history-pick-3")
                .contains("aria-checked=\"true\"")
        );
        // A second tap lets it go again.
        ui.click("history-pick-3");
        assert!(ui.has("1 selected"));
        assert!(
            ui.input_tag("history-pick-3")
                .contains("aria-checked=\"false\"")
        );

        ui.click("history-pick-4");
        ui.click("history-delete-chosen");
        assert_eq!(
            stored_dates(&ui),
            ["2026-10-02", "2026-10-03", "2026-10-04"]
        );
        assert!(ui.has("2 sessions deleted"));
        assert!(
            !ui.shows("history-delete-chosen"),
            "select mode ends with the delete"
        );
        assert!(ui.shows("history-delete-1"));

        ui.click("toast-undo");
        assert_eq!(ui.store.load_sessions(), history());
    });
}

#[test]
fn deleting_everything_asks_once_more_and_can_be_undone() {
    run(|| async {
        let (mut ui, _recorder) = open_history(&history());
        ui.click("history-select");
        ui.click("history-choose-all");
        assert!(ui.has("5 selected"));
        assert!(ui.has("Select none"));

        ui.click("history-delete-chosen");
        assert!(ui.has("Delete all 5?"));
        assert_eq!(
            ui.store.load_sessions().len(),
            5,
            "nothing goes before the answer"
        );
        ui.click("history-keep");
        assert!(ui.has("5 selected"));

        ui.click("history-delete-chosen");
        ui.click("history-delete-all");
        assert!(ui.store.load_sessions().is_empty());
        assert!(ui.has("No sessions stored yet."));
        assert!(ui.has("5 sessions deleted"));
        assert!(!ui.shows("history-select"), "nothing left to choose from");

        ui.click("toast-undo");
        assert_eq!(ui.store.load_sessions(), history());
        assert!(ui.has("5 sessions on this device"));
    });
}

#[test]
fn choosing_all_and_none_toggles_every_session() {
    run(|| async {
        let (mut ui, _recorder) = open_history(&history());
        ui.click("history-select");
        ui.click("history-choose-all");
        ui.click("history-choose-all");
        assert!(ui.has("0 selected"));
        ui.click("history-pick-2");
        ui.click("history-done");
        assert!(!ui.shows("history-delete-chosen"));
        // Coming back starts with nothing chosen.
        ui.click("history-select");
        assert!(ui.has("0 selected"));
        assert_eq!(ui.store.load_sessions(), history(), "Done deletes nothing");
    });
}

/// The undo toast stays up longer than a plain one, then goes, and the
/// deletion stands.
#[test]
fn the_undo_toast_waits_then_the_deletion_stands() {
    run(|| async {
        let (mut ui, _recorder) = open_history(&history());
        ui.click("history-delete-0");
        ui.advance(5_000).await;
        assert!(ui.shows("toast-undo"), "still time to undo");
        assert!(ui.run_until(5_000, |ui| !ui.has("class=\"toast\"")).await);
        assert_eq!(ui.store.load_sessions().len(), 4);
    });
}

/// A store that cannot take the change leaves the history as it was, on
/// screen as well as on disk, and says so.
#[test]
fn a_store_that_cannot_take_the_change_keeps_every_session() {
    run(|| async {
        let (mut ui, _recorder) = open_history(&history());
        ui.store.held.set(true);
        ui.click("history-delete-0");
        assert!(ui.has("Nothing was deleted"));
        assert!(ui.has("2026-10-01"));
        assert!(ui.has("5 sessions on this device"));
        assert_eq!(ui.store.load_sessions(), history());
        assert!(!ui.shows("toast-undo"), "nothing to undo");
    });
}

/// Two sessions that finished in the same millisecond are still two rows,
/// and deleting one leaves the other.
#[test]
fn sessions_that_finished_together_are_deleted_one_at_a_time() {
    run(|| async {
        let mut twins = history();
        twins[1].timestamp = twins[0].timestamp;
        let (mut ui, _recorder) = open_history(&twins);
        ui.click("history-delete-0");
        assert_eq!(
            stored_dates(&ui),
            ["2026-10-02", "2026-10-03", "2026-10-04", "2026-10-05"]
        );
    });
}

#[test]
fn a_long_history_is_listed_a_page_at_a_time() {
    run(|| async {
        let long: Vec<SessionResult> = (0..45u64)
            .map(|i| {
                let mut session = stored_session("2026-09-01", &[("KM", "KM")]);
                session.timestamp = 1_000 + i;
                session
            })
            .collect();
        let (mut ui, _recorder) = open_history(&long);
        assert_eq!(ui.controls("history-delete-").len(), 40);
        assert!(ui.has("Show 5 more"));
        ui.click("history-more");
        assert_eq!(ui.controls("history-delete-").len(), 45);
        assert!(!ui.shows("history-more"));
        // Choosing all takes the whole history, listed or not.
        ui.click("history-select");
        ui.click("history-choose-all");
        assert!(ui.has("45 selected"));
    });
}

/// Every row says what it practised, whatever is chosen now. A contest run
/// a later version kept reads Contest, known by its mark even once an older
/// build has dropped its log.
#[test]
fn every_row_names_its_mode() {
    run(|| async {
        let koch = stored_session("2026-10-01", &[("KM", "KM")]);
        let mut calls = stored_session("2026-10-02", &[("K1ABC", "K1ABC")]);
        calls.char_set_mode = CharSetMode::Callsign;
        let mut written = serde_json::to_value(stored_session("2026-10-03", &[("K5XYZ", "K5XY")]))
            .expect("a session writes");
        written["charSetMode"] = serde_json::json!("callsign");
        written["alphabetFingerprint"] = serde_json::json!("contest:wpx");
        let contest: SessionResult = serde_json::from_value(written).expect("a run reads");
        let (ui, _recorder) = open_history(&[koch, calls, contest]);
        assert!(ui.has("Koch · 1/1 groups"));
        assert!(ui.has("Callsigns · 1/1 groups"));
        assert!(ui.has("Contest · CQ\u{a0}WPX · log not kept by an older version"));
        assert!(ui.controls("history-open-").is_empty(), "no log to open");
    });
}

/// A run with Show copy on says it was assisted on its row, by its log or,
/// once an older build has dropped that, by its mark, which still reads as
/// its contest; and its results say so too.
#[test]
fn an_assisted_run_says_so_on_its_row() {
    run(|| async {
        let kept = crate::testing::assisted(stored_run(
            "2026-10-04",
            &[("K5XYZ", "214", "K5XYZ", "214"), ("W1AW", "5", "W1AW", "5")],
        ));
        let dropped = SessionResult {
            contest: None,
            date: "2026-10-03".into(),
            timestamp: kept.timestamp - 1,
            ..kept.clone()
        };
        let (mut ui, _recorder) = open_history(&[dropped, kept]);
        assert!(
            ui.has("Contest · CQ\u{a0}WPX · assisted · 2/2\u{a0}QSOs"),
            "{}",
            ui.html()
        );
        assert!(ui.has("Contest · CQ\u{a0}WPX · assisted · log not kept by an older version"));
        ui.click("history-open-1");
        assert!(ui.has("Assisted: Show copy was on."));
        assert!(ui.shows("btn-history"));
    });
}

/// A Simple QSO run's row gives the minutes a QSO took in place of a rate,
/// as a contest without a score is told, and opens its results.
#[test]
fn a_simple_qso_row_reads_its_pace() {
    run(|| async {
        let contest = crate::testing::stored_simple_qso_run(
            "2026-10-20",
            &[
                [("K5XYZ", "K5XYZ"), ("579", "579"), ("BOB", "BOB")],
                [("DL2XYZ", "DL2XYZ"), ("599", "599"), ("HANS", "HANZ")],
            ],
        );
        let (mut ui, _recorder) = open_history(&[contest]);
        assert!(
            ui.has("Contest · Simple\u{a0}QSO · 1/2\u{a0}QSOs · 5.0\u{a0}min/QSO"),
            "{}",
            ui.html()
        );
        assert!(!ui.has("/h<"));
        ui.click("history-open-0");
        assert!(ui.has("Accuracy per field"));
    });
}

/// An NA Sprint run's row reads as a WPX run's does, with its rate, and
/// opens its results with its contest score.
#[test]
fn a_sprint_row_reads_as_a_run() {
    run(|| async {
        let contest = crate::testing::stored_sprint_run(
            "2026-10-21",
            &[
                [
                    ("W7XYZ", "W7XYZ"),
                    ("145", "145"),
                    ("BOB", "BOB"),
                    ("OR", "OR"),
                ],
                [
                    ("VE3ABC", "VE3ABC"),
                    ("23", "23"),
                    ("DAVE", "DAVE"),
                    ("ON", "QC"),
                ],
            ],
        );
        let (mut ui, _recorder) = open_history(&[contest]);
        assert!(
            ui.has("Contest · NA\u{a0}Sprint · 1/2\u{a0}QSOs · 12/h"),
            "{}",
            ui.html()
        );
        ui.click("history-open-0");
        assert!(ui.has("Contest score"));
        assert!(ui.has("1\u{a0}wrong\u{a0}ST"));
    });
}

/// A contest run's row reads as a run — its contest, its QSOs clean of
/// those logged, its rate — and its points say so; it opens the run's
/// results, to be read and left again, with no run started from them.
#[test]
fn a_contest_row_opens_the_runs_results() {
    run(|| async {
        let koch = stored_session("2026-10-01", &[("KM", "KM")]);
        let contest = stored_run(
            "2026-10-20",
            &[
                ("K5XYZ", "214", "K5XYZ", "214"),
                ("DL2XYZ", "13", "DL2XYZ", "31"),
            ],
        );
        let pts = format!("{} pts", contest.score.round());
        let (mut ui, recorder) = open_history(&[koch, contest]);
        assert!(
            ui.has("Contest · CQ\u{a0}WPX · 1/2\u{a0}QSOs · 12/h"),
            "{}",
            ui.html()
        );
        assert!(ui.has(&pts), "{pts}");
        assert!(ui.has(" pts<"), "every row's points say so");
        assert_eq!(ui.controls("history-open-"), ["history-open-1"]);

        // Named by what the row shows, as every row is read.
        assert!(!ui.input_tag("history-open-1").contains("aria-label"));
        ui.click("history-open-1");
        assert!(ui.has("Run complete"));
        assert!(ui.has("CQ WPX · 10:00 · Tue\u{a0}20\u{a0}Oct\u{a0}2026"));
        assert!(ui.has("31→13"), "what was logged, and really sent");
        assert!(!ui.shows("btn-run-again"), "read only");
        assert!(
            ui.controls("history-delete-").is_empty(),
            "the list gave way to the results"
        );
        ui.click("btn-back");
        assert!(ui.has("2 sessions on this device"));
        assert_eq!(ui.screen(), "stats");
        // The way back at the top goes the same way.
        ui.click("history-open-1");
        ui.click("btn-history");
        assert!(ui.has("2 sessions on this device"));
        assert!(recorder.calls().is_empty(), "nothing was started");
    });
}

/// A long history makes the rows of the sessions it shows, and no others:
/// a row reads its run's log, and a page more makes the rows of that page.
#[test]
fn a_long_history_makes_only_the_rows_it_shows() {
    run(|| async {
        let long: Vec<SessionResult> = (0..100u64)
            .map(|day| {
                let mut session = stored_session("2026-10-01", &[("KM", "KM")]);
                session.timestamp = day * 86_400_000;
                session
            })
            .collect();
        let (mut ui, _recorder) = open_history(&long);
        let made = |ui: &mut Ui, shown: usize| {
            ui.with_app(|_| {
                let history = crate::state::in_root(consume_context::<Memo<History>>);
                let history = history.peek().clone();
                // Found made already, or none at all.
                history
                    .figure("history rows", shown, |_| Vec::<SessionHistoryRow>::new())
                    .len()
            })
        };
        assert_eq!(made(&mut ui, HISTORY_PAGE), HISTORY_PAGE);
        assert_eq!(ui.controls("history-delete-").len(), HISTORY_PAGE);
        ui.click("history-more");
        assert_eq!(made(&mut ui, 2 * HISTORY_PAGE), 2 * HISTORY_PAGE);
        assert_eq!(ui.take_worked_out("history rows"), 2);
        // Choosing in the list makes none again.
        ui.click("history-select");
        ui.click("history-pick-99");
        assert_eq!(ui.take_worked_out("history rows"), 0);
    });
}

#[component]
fn ReadOnlyStats() -> Element {
    rsx! {
        StatsView { settings: TrainingSettings::default(), sessions: history() }
    }
}

/// Without anywhere for deletions to go, the history is only shown.
#[test]
fn without_a_place_to_delete_to_the_history_is_only_shown() {
    let mut ui = Ui::new(ReadOnlyStats, ());
    ui.click("seg-history");
    assert!(ui.has("5 sessions on this device"));
    assert!(ui.controls("history-delete-").is_empty());
    assert!(!ui.shows("history-select"));
}
