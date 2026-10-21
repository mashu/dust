//! The board on screen: medals, places, the timeline's mark, the choice
//! of mode, and what an empty board says.

use super::*;
use crate::testing::{Ui, stored_run, stored_session};

/// A session of ten groups on the `day`th of October 2026, that earned
/// `per_group` each.
fn run_of(day: u32, per_group: f64, mode: CharSetMode) -> SessionResult {
    let mut session = stored_session(&format!("2026-10-{day:02}"), &[("KM", "KM"); 10]);
    session.timestamp = 1_790_000_000_000 + u64::from(day) * 86_400_000;
    session.score = per_group * 10.0;
    session.char_set_mode = mode;
    session
}

fn koch(day: u32, per_group: f64) -> SessionResult {
    run_of(day, per_group, CharSetMode::Koch)
}

fn board(sessions: Vec<SessionResult>) -> Ui {
    Ui::new(ScoresTab, ScoresTabProps { sessions })
}

#[test]
fn the_best_session_gets_the_gold_and_the_rest_follow() {
    let ui = board(vec![
        koch(1, 30.0),
        koch(2, 90.0),
        koch(3, 50.0),
        koch(4, 10.0),
    ]);
    assert!(ui.has("Personal best"));
    assert!(ui.has(">90.0<"));
    assert!(ui.has("Friday 2 October 2026"));
    assert_eq!(ui.controls("board-").len(), 4);
    for place in ["1st", "2nd", "3rd"] {
        assert!(ui.has(&format!("aria-label=\"{place} place\"")), "{place}");
    }
    assert!(!ui.has("aria-label=\"4th place\""), "medals stop at third");
    assert!(ui.has("class=\"board-number\">4<"));
    // The record was set on the 1st and the 2nd.
    assert!(ui.has("the record set 2 times"));
    assert!(ui.has("record-line"));
    // The timeline marks the personal best until a row is chosen.
    assert!(ui.has("1st</span> · 90.0 a group"));
}

/// Pointing at a row marks when it was set; a tap keeps it marked.
#[test]
fn a_row_marks_its_session_on_the_timeline() {
    let mut ui = board(vec![
        koch(1, 30.0),
        koch(2, 90.0),
        koch(3, 50.0),
        koch(4, 10.0),
    ]);
    ui.point_at("board-3", "mouse", (0.0, 0.0), (0.0, 0.0));
    assert!(ui.has("3rd</span> · 30.0 a group"));
    ui.point_away("board-3");
    assert!(ui.has("1st</span> · 90.0 a group"));

    ui.click("board-2");
    assert!(ui.input_tag("board-2").contains("pinned"));
    assert!(ui.has("2nd</span> · 50.0 a group"));
    ui.point_at("board-4", "mouse", (0.0, 0.0), (0.0, 0.0));
    assert!(ui.has("4th</span> · 10.0 a group"));
    ui.point_away("board-4");
    assert!(
        ui.has("2nd</span> · 50.0 a group"),
        "the tapped row stays marked"
    );
    ui.click("board-2");
    assert!(ui.has("1st</span> · 90.0 a group"));
    // A finger does not point.
    ui.point_at("board-4", "touch", (0.0, 0.0), (0.0, 0.0));
    assert!(ui.has("1st</span> · 90.0 a group"));
}

#[test]
fn a_board_can_be_narrowed_to_one_mode() {
    let mut ui = board(vec![
        koch(1, 30.0),
        run_of(2, 90.0, CharSetMode::Callsign),
        koch(3, 50.0),
    ]);
    assert!(ui.shows("pill-all-modes"));
    assert!(ui.shows("pill-koch"));
    assert!(ui.shows("pill-callsigns"));
    assert!(!ui.shows("pill-words"), "only modes with scores");
    assert_eq!(ui.controls("board-").len(), 3);
    assert!(
        ui.has("class=\"chip\">Callsigns<"),
        "the best names its mode"
    );

    ui.click("pill-koch");
    assert_eq!(ui.controls("board-").len(), 2);
    assert!(ui.has("1st</span> · 50.0 a group"));
    ui.click("pill-all-modes");
    assert_eq!(ui.controls("board-").len(), 3);
}

#[test]
fn one_mode_needs_no_choice_of_mode() {
    let ui = board(vec![koch(1, 30.0), koch(2, 50.0)]);
    assert!(!ui.shows("pill-all-modes"));
}

#[test]
fn a_long_board_shows_the_top_ten_until_asked() {
    let mut ui = board(
        (1..=12)
            .map(|day| koch(day, f64::from(day) * 10.0))
            .collect(),
    );
    assert_eq!(
        ui.controls("board-").len(),
        10 + 1,
        "ten rows and the toggle"
    );
    assert!(ui.has("Show all 12"));
    ui.click("board-all");
    assert_eq!(ui.controls("board-").len(), 12 + 1);
    assert!(ui.has("Show the top 10"));
}

/// A session too short to say much is kept, but not placed, and the
/// empty board says why.
#[test]
fn a_short_session_is_not_on_the_board() {
    let mut short = koch(1, 50.0);
    short.groups.truncate(BOARD_MIN_GROUPS - 1);
    let ui = board(vec![short.clone()]);
    assert!(ui.has("No session long enough yet"));
    assert!(ui.controls("board-").is_empty());
    let ui = board(vec![short, koch(2, 40.0)]);
    assert_eq!(ui.controls("board-").len(), 1);
    assert!(ui.has("Form"));
    assert!(ui.has("your one session"));
}

#[test]
fn no_score_no_board() {
    let ui = board(vec![koch(1, 0.0)]);
    assert!(ui.has("No scores yet"));
    assert!(ui.controls("board-").is_empty());
    assert!(board(Vec::new()).has("No scores yet"));
}

/// A ten-minute WPX run on the `day`th of October 2026 of `qsos` QSOs, each
/// logged clean.
fn wpx_run(day: u32, qsos: usize) -> SessionResult {
    let mut run = stored_run(
        &format!("2026-10-{day:02}"),
        &vec![("K5XYZ", "214", "K5XYZ", "214"); qsos],
    );
    run.timestamp = 1_790_000_000_000 + u64::from(day) * 86_400_000;
    run
}

/// A board of runs alone names the rule they went on by, with no pill to
/// choose and no word of group sessions that are not there; one that mixes
/// them with group sessions says where the runs sit.
#[test]
fn a_board_of_runs_alone_names_the_run_rule() {
    let ui = board((1..=3).map(|day| wpx_run(day, 5)).collect());
    assert_eq!(ui.controls("board-").len(), 3);
    assert!(!ui.shows("pill-all-modes") && !ui.shows("pill-contest"));
    assert!(
        ui.has("over runs of 5 minutes or more that hold 10 groups or more"),
        "{}",
        ui.html()
    );
    assert!(!ui.has("over sessions of"));
    assert!(!ui.has("A WPX run earns about 30% less"));

    let ui = board(vec![wpx_run(1, 5), koch(2, 40.0)]);
    assert!(ui.has("A WPX run earns about 30% less"));
    assert!(ui.has("over sessions of 10 groups or more"));
}

/// Four WPX QSOs are eight groups: the run is kept but not placed, and the
/// empty board gives the run rule and the setting that meets it, not a
/// count of groups a run has no field for.
#[test]
fn a_run_too_short_for_the_board_says_the_run_rule() {
    let ui = board(vec![wpx_run(1, 4)]);
    assert!(ui.has("No run long enough yet"), "{}", ui.html());
    assert!(ui.has("hold 10 groups or more"));
    assert!(ui.has("Set Minutes per run to 5 or more"));
    assert!(!ui.has("Set a longer session"));
    // A group session among them is the board that a longer session fills.
    let mut short = koch(2, 50.0);
    short.groups.truncate(BOARD_MIN_GROUPS - 1);
    assert!(board(vec![wpx_run(1, 4), short]).has("No session long enough yet"));
}
