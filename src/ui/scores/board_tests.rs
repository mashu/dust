//! The board on screen: medals, places, the timeline's mark, the choice
//! of mode, and what an empty board says.

use super::*;
use crate::testing::{Ui, assisted, stored_run, stored_session};
use crate::ui::format::ordinal;

/// A session of ten groups of five on the `day`th of October 2026, that
/// earned `per_group` each.
fn run_of(day: u32, per_group: f64, mode: CharSetMode) -> SessionResult {
    let mut session = stored_session(&format!("2026-10-{day:02}"), &[("KMRSU", "KMRSU"); 10]);
    session.timestamp = 1_790_000_000_000 + u64::from(day) * 86_400_000;
    session.score = per_group * 10.0;
    session.char_set_mode = mode;
    session
}

fn koch(day: u32, per_group: f64) -> SessionResult {
    run_of(day, per_group, CharSetMode::Koch)
}

fn board(sessions: Vec<SessionResult>) -> Ui {
    Ui::new(
        ScoresTab,
        ScoresTabProps {
            sessions: sessions.into(),
        },
    )
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

/// Pointing at a row redraws the rows it moves between, the mark and what
/// is said of it, and nothing else: the tab reads every stored run's log to
/// build the board, and the timeline draws a dot a session, so neither is
/// drawn again for a mouse crossing the board. Under All modes as under a
/// contest's own board.
#[test]
fn pointing_at_a_row_redraws_only_what_marks_it() {
    let mut sessions: Vec<SessionResult> = (1..=8)
        .map(|day| koch(day, f64::from(day) * 10.0))
        .collect();
    sessions.extend((9..=14).map(|day| wpx_run(day, 6 + day as usize)));
    let mut ui = board(sessions);
    for pill in [None, Some("pill-contest")] {
        if let Some(pill) = pill {
            ui.click(pill);
        }
        ui.take_work();
        for name in ["ScoresTab", "ScoreTimeline", "BoardRow"] {
            ui.take_renders(name);
        }
        for place in 3..=5 {
            let row = format!("board-{place}");
            ui.point_at(&row, "mouse", (0.0, 0.0), (0.0, 0.0));
            assert!(ui.input_tag(&row).contains("focused"), "{pill:?} {row}");
            assert!(
                ui.has(&format!("{}</span> · ", ordinal(place))),
                "{pill:?} {row}"
            );
            ui.point_away(&row);
        }
        assert!(
            ui.input_tag("board-1").contains("focused"),
            "back to the best"
        );
        assert_eq!(ui.take_renders("ScoresTab"), 0, "{pill:?}");
        assert_eq!(ui.take_renders("ScoreTimeline"), 0, "{pill:?}");
        // Six moves, each between the best's row and another.
        assert_eq!(ui.take_renders("BoardRow"), 12, "{pill:?}");
        // The rows' classes, the guide's and the ring's places, the line
        // said of the session.
        let work = ui.take_work();
        assert!(work <= 6 * 10, "{pill:?}: {work} mutations");
    }
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

/// The Contest pill is there before any run is: it says how a run gets
/// on a board of its own.
#[test]
fn the_contest_pill_is_there_before_any_run() {
    let mut ui = board(vec![koch(1, 30.0), koch(2, 50.0)]);
    assert!(ui.shows("pill-all-modes") && ui.shows("pill-koch"));
    ui.click("pill-contest");
    assert!(ui.has(">No contest runs yet<"), "{}", ui.html());
    assert!(ui.has("Choose Contest under Settings → Character set"));
    assert!(!ui.shows("rank-verified-score"), "nothing to rank yet");
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

/// A board of runs alone names the rule they went on by, and no word of
/// group sessions that are not there, its pills offering only Contest, whose
/// Runs card lists them; one that mixes them with group sessions says where
/// the runs sit.
#[test]
fn a_board_of_runs_alone_names_the_run_rule() {
    let ui = board((1..=3).map(|day| wpx_run(day, 5)).collect());
    assert_eq!(ui.controls("board-").len(), 3);
    assert_eq!(ui.controls("pill-"), ["pill-all-modes", "pill-contest"]);
    assert!(
        ui.has("over runs of 5 minutes or more that hold 10 groups or more"),
        "{}",
        ui.html()
    );
    assert!(!ui.has("over sessions of"));
    assert!(!ui.has("Runs are rated here as any session is"));

    let ui = board(vec![wpx_run(1, 5), koch(2, 40.0)]);
    assert!(ui.has("Runs are rated here as any session is"));
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

/// A whole run with Show copy on is assisted: on the board it is listed
/// greyed after the places, with no place and nothing to mark on the
/// timeline, however well it scored, and the board says why.
#[test]
fn an_assisted_run_is_greyed_and_unplaced() {
    let mut better = assisted(wpx_run(2, 8));
    better.score *= 3.0;
    let ui = board(vec![wpx_run(1, 5), better]);
    assert_eq!(ui.controls("board-"), ["board-1"], "one place");
    assert!(ui.has(r#"class="board-row assisted""#), "{}", ui.html());
    assert!(ui.has(r#"aria-label="Not placed""#));
    assert!(ui.has("· assisted</span>"));
    assert!(ui.has("Runs with Show copy on are assisted"));
    assert!(
        ui.has("your one session"),
        "the form is the placed run's alone"
    );
    // A board of assisted runs alone places nothing and says why.
    let ui = board(vec![assisted(wpx_run(2, 8))]);
    assert!(ui.has("No placed run yet"), "{}", ui.html());
    assert!(ui.has(r#"class="board-row assisted""#));
    assert!(ui.controls("board-").is_empty());
}

/// Under the Contest pill, the Runs card lists every run of the contest
/// chosen, newest first: the best verified run of its length marked, an
/// assisted run marked and never best, a run whose log an older version
/// dropped listed as History lists it. A row whose log reads opens its
/// results in the tab's place, with the way back to Scores.
#[test]
fn the_runs_card_lists_every_run_and_opens_its_results() {
    let mut best = wpx_run(1, 6);
    best.timestamp += 1;
    let short = SessionResult {
        timestamp: best.timestamp + 86_400_000,
        ..wpx_run(2, 3)
    };
    let mut better = assisted(wpx_run(3, 9));
    better.score *= 2.0;
    let dropped = SessionResult {
        contest: None,
        ..wpx_run(4, 5)
    };
    let mut ui = board(vec![koch(5, 40.0), best, short, better, dropped]);
    assert!(!ui.has("runs-card"), "only under the Contest pill");
    ui.click("pill-contest");
    assert!(ui.has("runs-card"), "{}", ui.html());
    assert!(ui.has("4 runs of CQ WPX on this device, newest first"));
    // Newest first; the dropped run opens nothing.
    assert_eq!(
        ui.controls("runs-open-"),
        ["runs-open-1", "runs-open-2", "runs-open-3"]
    );
    let html = ui.html();
    let at = |needle: &str| html.find(needle).unwrap_or_else(|| panic!("no {needle}"));
    // Each dated as the board dates it, with the time, so runs of one day
    // tell apart.
    let runs = &html[at("runs-card")..];
    let on = |day: &str| {
        runs.find(&format!("{day} · "))
            .unwrap_or_else(|| panic!("no {day}"))
    };
    assert!(on("Sun 4 Oct 2026") < on("Sat 3 Oct 2026"));
    assert!(on("Sat 3 Oct 2026") < on("Thu 1 Oct 2026"));
    assert!(html.contains("log not kept by an older version"));
    // With no contest score to show, its Points go under a dash: the
    // column of scores holds scores alone.
    assert!(
        runs.contains(r#"<span class="value">—</span><span class="muted">"#),
        "{runs}"
    );
    assert_eq!(
        html.matches(r#"class="chip good">Best<"#).count(),
        1,
        "one length, one best"
    );
    assert_eq!(html.matches(r#"class="chip quiet">assisted<"#).count(), 1);
    // The best is the six-QSO run, not the better assisted one.
    let row = |id: &str| {
        let tag = ui.input_tag(id);
        let from = html.find(&tag).expect("drawn");
        html[from..from + html[from..].find("</button>").expect("closed")].to_string()
    };
    assert!(
        row("runs-open-1").contains("Best"),
        "{}",
        row("runs-open-1")
    );
    assert!(row("runs-open-3").contains("assisted") && !row("runs-open-3").contains("Best"));
    // Each part of a row's figures kept whole, so a phone's line breaks
    // only between them.
    assert!(
        row("runs-open-1").contains("10\u{a0}min · 6/6\u{a0}QSOs"),
        "{}",
        row("runs-open-1")
    );
    ui.click("runs-open-1");
    assert!(ui.has("Run complete"));
    assert!(ui.shows("btn-scores"));
    ui.click("btn-scores");
    assert!(ui.has("runs-card"));
    // An assisted run's results say so.
    ui.click("runs-open-3");
    assert!(ui.has("Assisted: Show copy was on."), "{}", ui.html());
}

/// The Runs card lists a page of runs at a time, as History does, the best
/// of each length wherever it falls, and offers the rest.
#[test]
fn the_runs_card_lists_a_page_at_a_time() {
    let runs: Vec<SessionResult> = (1..=45).map(|day| wpx_run(day, 5)).collect();
    let mut ui = board(runs);
    ui.click("pill-contest");
    assert!(ui.has("45 runs of CQ WPX on this device, newest first"));
    // The newest forty, and the oldest, which set the best first.
    let listed = ui.controls("runs-open-");
    assert_eq!(listed.len(), 41);
    assert!(listed.contains(&"runs-open-0".to_string()), "{listed:?}");
    assert!(!listed.contains(&"runs-open-1".to_string()), "{listed:?}");
    assert!(ui.has(r#"class="chip good">Best<"#));
    assert!(ui.has("Show 4 more"), "{}", ui.html());
    ui.click("runs-more");
    assert_eq!(ui.controls("runs-open-").len(), 45);
    assert!(!ui.shows("runs-more"));
}
