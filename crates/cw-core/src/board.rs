//! The score board: the sessions kept on this device, best first, and how
//! their scores and the record have moved over time.
//!
//! A session's score is what its groups earn added up, so a longer session
//! scores more for the same copy. The board ranks by what a session earns a
//! group instead — the mean, so that every miss counts, where a median would
//! let a session with a few groups lost tie one with none — and only sessions
//! long enough for that mean to say something: [`BOARD_MIN_GROUPS`] groups or
//! more, as RufzXP counts only a whole run.
//!
//! How a learner is copying these days is the median of their last few
//! sessions' ([`FORM_SESSIONS`]): one bad day does not drag it down the way it
//! would an average.
//!
//! A contest run is ranked among runs of its own contest, and only one that
//! says something of a whole run goes on: a log this build reads, five
//! minutes or more of a limit of five or more, a QSO logged clean, and Show
//! copy off.
//!
//! There is one player, so a place on the board is a session's, not a
//! person's. It is read off the stored history, so a deleted session leaves
//! the board with it. Two equal scores are placed in the order they were set:
//! the first to reach a score holds the higher place, as on any record list.

use crate::contest::ContestKind;
use crate::session::SessionResult;
use crate::settings::CharSetMode;

/// The fewest groups a session needs to go on the board.
pub const BOARD_MIN_GROUPS: usize = 10;

/// The shortest contest run that goes on the board, and the shortest limit
/// it ran to, in seconds: a handful of quick QSOs could top a longer one.
pub const BOARD_RUN_MIN_S: u64 = 300;

/// How many of the latest sessions the form is the median of.
pub const FORM_SESSIONS: usize = 10;

/// One session's place on the board, and what it was.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardEntry {
    /// 1 for the best.
    pub place: usize,
    /// Where the session sits in the history it was read from.
    pub index: usize,
    /// What it earned a group: what it is ranked by.
    pub per_group: f64,
    /// What it earned in all.
    pub score: f64,
    pub timestamp: u64,
    pub date: String,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    pub accuracy: f64,
    pub groups: usize,
    pub mode: CharSetMode,
}

/// What a session earned a group, if it can go on the board: a score that
/// could be read and earned something, over enough groups, and for a
/// contest run, a whole run.
pub fn per_group(session: &SessionResult) -> Option<f64> {
    let groups = session.groups.len();
    (session.score.is_finite()
        && session.score > 0.0
        && groups >= BOARD_MIN_GROUPS
        && (session.char_set_mode != CharSetMode::Contest || a_whole_run(session)))
    .then(|| session.score / groups as f64)
}

/// Whether a contest run says something of a whole run: its log reads, it
/// ran five minutes or more to a limit of five or more, it logged a QSO
/// clean, and Show copy was off. A run whose log an older build dropped is
/// left off.
fn a_whole_run(session: &SessionResult) -> bool {
    let groups = &session.groups;
    let run = session
        .contest
        .as_ref()
        .and_then(|log| log.run(groups.len()));
    run.is_some_and(|run| {
        !run.assisted
            && u64::from(run.limit_s) >= BOARD_RUN_MIN_S
            && run.run_ms >= BOARD_RUN_MIN_S * 1_000
            && run.clean_qsos(groups).is_some_and(|clean| clean > 0)
    })
}

/// Which sessions a board ranks against each other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BoardOf {
    /// Every session, whatever it practised.
    #[default]
    All,
    /// The sessions of one character set: for Contest, the runs of every
    /// contest.
    Mode(CharSetMode),
    /// The runs of one contest, as its mark names it.
    Contest(ContestKind),
}

impl BoardOf {
    /// The board a session is placed on: its own mode's, and a contest
    /// run's own contest's.
    pub fn of(session: &SessionResult) -> Self {
        session
            .contest_kind()
            .map_or(Self::Mode(session.char_set_mode), Self::Contest)
    }

    fn holds(self, session: &SessionResult) -> bool {
        match self {
            Self::All => true,
            Self::Mode(mode) => session.char_set_mode == mode,
            Self::Contest(kind) => session.contest_kind() == Some(kind),
        }
    }
}

/// The sessions `of` holds that go on the board, each with where it sits in
/// the history and what it earned a group.
fn qualifying(sessions: &[SessionResult], of: BoardOf) -> Vec<(usize, &SessionResult, f64)> {
    sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| of.holds(session))
        .filter_map(|(index, session)| per_group(session).map(|rating| (index, session, rating)))
        .collect()
}

/// Every session `of` holds on the board, best first.
pub fn score_board(sessions: &[SessionResult], of: BoardOf) -> Vec<BoardEntry> {
    let mut ranked = qualifying(sessions, of);
    ranked.sort_by(|(a_index, a, a_rating), (b_index, b, b_rating)| {
        b_rating
            .total_cmp(a_rating)
            .then(a.timestamp.cmp(&b.timestamp))
            .then(a_index.cmp(b_index))
    });
    ranked
        .into_iter()
        .enumerate()
        .map(|(rank, (index, session, rating))| BoardEntry {
            place: rank + 1,
            index,
            per_group: rating,
            score: session.score,
            timestamp: session.timestamp,
            date: session.date.clone(),
            char_wpm: session.char_wpm,
            effective_wpm: session.effective_wpm,
            accuracy: session.accuracy,
            groups: session.groups.len(),
            mode: session.char_set_mode,
        })
        .collect()
}

/// Where the session finished at `timestamp` stands among the sessions of
/// its own mode on the board, and a contest run among runs of its own
/// contest: its place, and how many there are. Nothing for a session that
/// is not on it.
pub fn board_place(sessions: &[SessionResult], timestamp: u64) -> Option<(usize, usize)> {
    let session = sessions
        .iter()
        .find(|session| session.timestamp == timestamp)?;
    let board = score_board(sessions, BoardOf::of(session));
    let place = board
        .iter()
        .position(|entry| entry.timestamp == timestamp)?;
    Some((place + 1, board.len()))
}

/// The modes the board has sessions of, in the order the trainer offers
/// them.
pub fn board_modes(sessions: &[SessionResult]) -> Vec<CharSetMode> {
    CharSetMode::ALL
        .into_iter()
        .filter(|mode| {
            sessions
                .iter()
                .any(|session| session.char_set_mode == *mode && per_group(session).is_some())
        })
        .collect()
}

/// The contests the board has runs of, in the order the card lists them.
pub fn board_kinds(sessions: &[SessionResult]) -> Vec<ContestKind> {
    ContestKind::ALL
        .into_iter()
        .filter(|kind| {
            sessions.iter().any(|session| {
                session.contest_kind() == Some(*kind) && per_group(session).is_some()
            })
        })
        .collect()
}

/// The middle of `values`, or the mean of the two middle ones.
fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => sorted.get(mid).copied(),
        _ => Some((sorted.get(mid - 1)? + sorted.get(mid)?) / 2.0),
    }
}

/// One session on the board's timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelinePoint {
    pub index: usize,
    pub timestamp: u64,
    /// What it earned a group.
    pub per_group: f64,
    /// The best a group up to and including this session.
    pub record: f64,
    /// Whether this session set that record: beat every session before it.
    pub set_record: bool,
    /// The form after it: the median of the last [`FORM_SESSIONS`] sessions,
    /// this one among them.
    pub form: f64,
}

/// Every session `of` holds on the board, oldest first, with the record and
/// the form as they stood after each.
pub fn score_timeline(sessions: &[SessionResult], of: BoardOf) -> Vec<TimelinePoint> {
    let mut sorted = qualifying(sessions, of);
    sorted.sort_by(|(a_index, a, _), (b_index, b, _)| {
        a.timestamp.cmp(&b.timestamp).then(a_index.cmp(b_index))
    });
    let ratings: Vec<f64> = sorted.iter().map(|(_, _, rating)| *rating).collect();
    let mut record = 0.0_f64;
    sorted
        .iter()
        .enumerate()
        .map(|(n, (index, session, rating))| {
            let set_record = *rating > record;
            record = record.max(*rating);
            let recent = &ratings[(n + 1).saturating_sub(FORM_SESSIONS)..=n];
            TimelinePoint {
                index: *index,
                timestamp: session.timestamp,
                per_group: *rating,
                record,
                set_record,
                form: median(recent).unwrap_or(*rating),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::bot::Runtime;
    use crate::contest::bot::script::{ScriptCalls, run_settings};
    use crate::contest::{ContestEvent, OpKey, Sender};
    use crate::session::GroupResult;

    /// A session of `groups` groups that earned `score` in all.
    fn session(timestamp: u64, score: f64, groups: usize, mode: CharSetMode) -> SessionResult {
        SessionResult {
            timestamp,
            score,
            char_set_mode: mode,
            groups: vec![GroupResult::default(); groups],
            date: format!("2026-10-{:02}", 1 + timestamp % 28),
            ..SessionResult::empty_for_test()
        }
    }

    /// A Koch session of twenty groups that earned `per_group` each.
    fn koch(timestamp: u64, per_group: f64) -> SessionResult {
        session(timestamp, per_group * 20.0, 20, CharSetMode::Koch)
    }

    #[test]
    fn the_best_a_group_comes_first() {
        let sessions = [koch(1, 30.0), koch(2, 90.0), koch(3, 50.0)];
        let board = score_board(&sessions, BoardOf::All);
        let ratings: Vec<f64> = board.iter().map(|entry| entry.per_group).collect();
        assert_eq!(ratings, [90.0, 50.0, 30.0]);
        let places: Vec<usize> = board.iter().map(|entry| entry.place).collect();
        assert_eq!(places, [1, 2, 3]);
        assert_eq!(board[0].index, 1, "the entry points back at its session");
        assert_eq!(board[0].score, 1_800.0, "and keeps what it earned in all");
    }

    /// A long session does not outrank a better short one by its length.
    #[test]
    fn a_longer_session_is_not_a_better_one() {
        let long = session(1, 50.0 * 60.0, 60, CharSetMode::Koch);
        let short = session(2, 70.0 * 12.0, 12, CharSetMode::Koch);
        let board = score_board(&[long, short], BoardOf::All);
        assert_eq!(board[0].timestamp, 2);
        assert_eq!(board[0].per_group, 70.0);
    }

    /// Too few groups say too little: a lucky handful is not a place.
    #[test]
    fn a_session_too_short_is_not_placed() {
        let short = session(1, 900.0, BOARD_MIN_GROUPS - 1, CharSetMode::Koch);
        let just = session(2, 100.0, BOARD_MIN_GROUPS, CharSetMode::Koch);
        assert_eq!(per_group(&short), None);
        assert_eq!(per_group(&just), Some(10.0));
        let board = score_board(&[short, just], BoardOf::All);
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].timestamp, 2);
    }

    /// The first to reach a score holds the higher place.
    #[test]
    fn an_equal_score_ranks_below_the_one_set_first() {
        let sessions = [koch(20, 50.0), koch(10, 50.0)];
        let board = score_board(&sessions, BoardOf::All);
        assert_eq!(board[0].timestamp, 10);
        assert_eq!(board[1].timestamp, 20);
    }

    /// A contest run goes on the board only as a whole run: five minutes or
    /// more, of a limit of five or more, with a QSO logged clean and a log
    /// this build reads. It is placed among runs of its own contest.
    #[test]
    fn a_contest_run_is_placed_only_as_a_whole_run() {
        use crate::contest::bot::Operator;
        use crate::contest::bot::script::{Habit, work};
        let worked = |minutes: u32, habit: Habit, timestamp: u64| {
            let mut settings = run_settings(25.0);
            settings.contest.contest_minutes = minutes;
            let sampler = crate::Sampler::new(&[], &settings, 3);
            let mut runtime = Runtime::start(3, settings, sampler);
            work(&mut runtime, &mut Operator::new(habit), &[]);
            runtime.machine.result(timestamp, "2026-10-20".into())
        };
        let short = worked(2, Habit::Perfect, 1);
        assert!(short.groups.len() >= BOARD_MIN_GROUPS && short.score > 0.0);
        assert_eq!(per_group(&short), None, "a two-minute run");
        let whole = worked(5, Habit::Perfect, 2);
        assert!(per_group(&whole).is_some());
        let busted = worked(5, Habit::DigitOff, 3);
        assert!(busted.groups.len() >= BOARD_MIN_GROUPS && busted.score > 0.0);
        assert_eq!(per_group(&busted), None, "no QSO logged clean");
        let stripped = SessionResult {
            contest: None,
            timestamp: 4,
            ..whole.clone()
        };
        assert_eq!(per_group(&stripped), None, "its log dropped");
        let sessions = [short, whole.clone(), busted, stripped];
        let board = score_board(&sessions, BoardOf::Mode(CharSetMode::Contest));
        assert_eq!(board.len(), 1);
        assert_eq!(board_place(&sessions, 2), Some((1, 1)));
        // A better run of another contest, as its mark says, is not above
        // it on its contest's board.
        let other = SessionResult {
            alphabet_fingerprint: "contest:simpleQso".into(),
            score: whole.score * 2.0,
            timestamp: 5,
            ..whole
        };
        let sessions = [sessions[1].clone(), other];
        assert_eq!(
            score_board(&sessions, BoardOf::Mode(CharSetMode::Contest)).len(),
            2
        );
        assert_eq!(board_place(&sessions, 2), Some((1, 1)));
        assert_eq!(board_place(&sessions, 5), Some((1, 1)));
        // Each contest has a board of its own, in the order the card lists
        // them.
        assert_eq!(
            board_kinds(&sessions),
            [ContestKind::Wpx, ContestKind::SimpleQso]
        );
        let wpx = score_board(&sessions, BoardOf::Contest(ContestKind::Wpx));
        assert_eq!(
            wpx.iter().map(|entry| entry.timestamp).collect::<Vec<_>>(),
            [2]
        );
        let timeline = score_timeline(&sessions, BoardOf::Contest(ContestKind::SimpleQso));
        assert_eq!(timeline.len(), 1);
        assert_eq!(
            BoardOf::of(&sessions[0]),
            BoardOf::Contest(ContestKind::Wpx)
        );
        assert!(board_kinds(&[koch(1, 50.0)]).is_empty());
        assert_eq!(
            BoardOf::of(&koch(1, 50.0)),
            BoardOf::Mode(CharSetMode::Koch)
        );
    }

    /// A broken or empty score is not a place on the board.
    #[test]
    fn only_a_score_that_earned_something_is_placed() {
        let sessions = [
            koch(1, 0.0),
            koch(2, f64::NAN),
            koch(3, f64::INFINITY),
            koch(4, -5.0),
            koch(5, 40.0),
        ];
        let board = score_board(&sessions, BoardOf::All);
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].timestamp, 5);
        assert!(score_board(&[], BoardOf::All).is_empty());
    }

    #[test]
    fn a_board_can_hold_one_mode() {
        let sessions = [
            koch(1, 30.0),
            session(2, 1_800.0, 20, CharSetMode::Callsign),
            koch(3, 50.0),
            session(4, 900.0, 3, CharSetMode::Words),
        ];
        let calls = score_board(&sessions, BoardOf::Mode(CharSetMode::Callsign));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].place, 1);
        let koch_board = score_board(&sessions, BoardOf::Mode(CharSetMode::Koch));
        assert_eq!(koch_board.len(), 2);
        assert_eq!(koch_board[0].per_group, 50.0);
        assert_eq!(
            board_modes(&sessions),
            [CharSetMode::Koch, CharSetMode::Callsign],
            "words have only a session too short"
        );
    }

    /// A session is placed among its own mode's: a callsign run against
    /// callsign runs.
    #[test]
    fn a_session_knows_its_place() {
        let sessions = [
            koch(1, 30.0),
            session(2, 1_800.0, 20, CharSetMode::Callsign),
            koch(3, 50.0),
            koch(4, 40.0),
            session(5, 900.0, 3, CharSetMode::Koch),
        ];
        assert_eq!(board_place(&sessions, 3), Some((1, 3)));
        assert_eq!(board_place(&sessions, 4), Some((2, 3)));
        assert_eq!(board_place(&sessions, 2), Some((1, 1)));
        assert_eq!(board_place(&sessions, 5), None, "too short to place");
        assert_eq!(board_place(&sessions, 99), None, "no such session");
    }

    #[test]
    fn the_timeline_runs_oldest_first_and_marks_each_new_record() {
        let sessions = [
            koch(30, 80.0),
            koch(10, 30.0),
            koch(20, 20.0),
            koch(40, 90.0),
        ];
        let timeline = score_timeline(&sessions, BoardOf::All);
        let at: Vec<u64> = timeline.iter().map(|point| point.timestamp).collect();
        assert_eq!(at, [10, 20, 30, 40]);
        let records: Vec<f64> = timeline.iter().map(|point| point.record).collect();
        assert_eq!(records, [30.0, 30.0, 80.0, 90.0]);
        let set: Vec<bool> = timeline.iter().map(|point| point.set_record).collect();
        assert_eq!(set, [true, false, true, true]);
        // Equalling the record does not set it.
        let tie = score_timeline(&[koch(1, 30.0), koch(2, 30.0)], BoardOf::All);
        assert!(!tie[1].set_record);
    }

    /// The form is the median of the latest sessions: one bad day barely
    /// moves it, and sessions older than the window do not count.
    #[test]
    fn the_form_is_the_median_of_the_latest_sessions() {
        let mut sessions: Vec<SessionResult> = (0..9).map(|n| koch(n, 60.0)).collect();
        sessions.push(koch(9, 5.0));
        let timeline = score_timeline(&sessions, BoardOf::All);
        assert_eq!(timeline.last().map(|point| point.form), Some(60.0));
        // Two sessions: the mean of the two.
        let pair = score_timeline(&[koch(1, 40.0), koch(2, 60.0)], BoardOf::All);
        assert_eq!(pair[1].form, 50.0);
        // Only the last ten count.
        let mut long: Vec<SessionResult> = (0..10).map(|n| koch(n, 10.0)).collect();
        long.extend((10..20).map(|n| koch(n, 70.0)));
        let timeline = score_timeline(&long, BoardOf::All);
        assert_eq!(timeline.last().map(|point| point.form), Some(70.0));
        assert_eq!(timeline[0].form, 10.0);
    }

    #[test]
    fn a_median_takes_the_middle() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0]), Some(3.0));
        assert_eq!(median(&[9.0, 1.0, 5.0]), Some(5.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }

    /// The callers' last send in the turn just played.
    fn last_heard(runtime: &Runtime<ScriptCalls>) -> String {
        runtime
            .last_turn()
            .segments
            .iter()
            .rfind(|segment| segment.who != Sender::You)
            .map(|segment| segment.text.clone())
            .expect("a caller sent")
    }

    /// A five-minute run at 25 WPM, kept at `timestamp`: each caller's call
    /// heard and sent back with your exchange by Enter, then what `each`
    /// does with that QSO, given its call and how many came before it.
    fn five_minute_run(
        timestamp: u64,
        each: impl Fn(&mut Runtime<ScriptCalls>, &str, usize),
    ) -> SessionResult {
        let calls: Vec<String> = (0..60)
            .map(|n| format!("K{}X{}Z", n % 10, (b'A' + n / 10) as char))
            .collect();
        let calls: Vec<&str> = calls.iter().map(String::as_str).collect();
        let mut settings = run_settings(25.0);
        settings.contest.contest_minutes = 5;
        let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&calls));
        let mut worked = 0;
        while runtime.now < 270_000 {
            runtime.finish_turn();
            let call = last_heard(&runtime);
            runtime.key(OpKey::Esm, &call, "");
            each(&mut runtime, &call, worked);
            worked += 1;
        }
        runtime.run_to(300_001);
        if !runtime.over {
            runtime.apply(ContestEvent::FinishNow);
        }
        runtime.machine.result(timestamp, "2026-10-20".into())
    }

    /// Its exchange heard whole, then logged as heard.
    fn log_as_heard(runtime: &mut Runtime<ScriptCalls>, call: &str) {
        let exchange = last_heard(runtime);
        runtime.finish_turn();
        let nr = exchange.split_whitespace().last().unwrap_or_default();
        runtime.key(OpKey::Esm, call, nr);
    }

    /// Logging each QSO before its exchange reached you, with a guessed
    /// number, is no way up the board: each guess is a missed field, not a
    /// group the run never had.
    #[test]
    fn logging_before_the_exchange_is_no_way_up_the_board() {
        let worked = |guess: bool, timestamp: u64| {
            five_minute_run(timestamp, |runtime, call, worked| {
                if guess && worked > 0 {
                    // TU and log while your exchange still goes out.
                    runtime.key(OpKey::TuAndLog, call, "1");
                } else {
                    log_as_heard(runtime, call);
                }
            })
        };
        let (copied, guessed) = (worked(false, 1), worked(true, 2));
        assert!(per_group(&guessed).is_some(), "a whole run, one QSO clean");
        assert!(guessed.accuracy < copied.accuracy);
        let board = score_board(&[copied, guessed], BoardOf::Mode(CharSetMode::Contest));
        assert_eq!(board[0].timestamp, 1);
    }

    /// Walking away from each QSO once the exchanges are out, with CQ, is no
    /// way up the board: each field never logged is a miss, so it pays no
    /// better than finishing, and hides no copy from accuracy.
    #[test]
    fn walking_away_is_no_way_up_the_board() {
        let worked = |walk_away: bool, timestamp: u64| {
            five_minute_run(timestamp, |runtime, call, worked| {
                if walk_away && worked > 0 {
                    // Its exchange heard whole, then CQ: the next caller
                    // comes.
                    runtime.finish_turn();
                    runtime.key(OpKey::Cq, call, "");
                } else {
                    log_as_heard(runtime, call);
                }
            })
        };
        // Every QSO finished, copied perfectly; and one QSO finished, every
        // other walked away from once its exchange was heard.
        let (finished, walked) = (worked(false, 1), worked(true, 2));
        assert!(per_group(&finished).is_some() && per_group(&walked).is_some());
        assert_eq!(finished.accuracy, 1.0);
        assert!(walked.accuracy < finished.accuracy);
        let board = score_board(&[finished, walked], BoardOf::Mode(CharSetMode::Contest));
        let ranked: Vec<(u64, f64)> = board
            .iter()
            .map(|entry| (entry.timestamp, entry.per_group))
            .collect();
        assert_eq!(
            board[0].timestamp, 1,
            "the run that walked away ranks first: {ranked:?}"
        );
    }
}
