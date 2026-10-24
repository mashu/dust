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
//! copy off. A whole run with Show copy on is assisted: the board lists it
//! greyed, with no place, and it sets no record and moves no form. Every
//! run of a contest, whole or not, is on its Runs card ([`contest_runs`]).
//!
//! A contest's own board ranks by the contest's own measure ([`Ranking`]):
//! the verified score, of what was logged clean, among runs set to the
//! same length, as a longer run scores more for the same copy; or the QSOs
//! logged clean an hour, fair between runs of any length, and the only
//! measure of a contest with no score. Points a group rank the boards that
//! mix runs with sessions of groups.
//!
//! There is one player, so a place on the board is a session's, not a
//! person's. It is read off the stored history, so a deleted session leaves
//! the board with it. Two equal scores are placed in the order they were set:
//! the first to reach a score holds the higher place, as on any record list.

use crate::contest::{ContestKind, StoredLog};
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
    /// 1 for the best; 0 for an assisted run, which the board lists with
    /// no place ([`assisted_runs`]).
    pub place: usize,
    /// Where the session sits in the history it was read from.
    pub index: usize,
    /// What it is ranked by ([`Ranking`]): what it earned a group, or a
    /// run's verified score or clean QSOs an hour.
    pub rating: f64,
    /// What it earned in all.
    pub score: f64,
    pub timestamp: u64,
    pub date: String,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    pub accuracy: f64,
    pub groups: usize,
    pub mode: CharSetMode,
    /// The contest a run was of, when this build knows it.
    pub contest: Option<ContestKind>,
    /// The QSOs a run logged, and those logged clean, where its log reads.
    pub qsos: Option<usize>,
    pub clean: Option<usize>,
    /// The length a run was set to, in seconds, where its log reads.
    pub limit_s: Option<u32>,
}

/// What a session earned a group, if it can go on the board: a score that
/// could be read and earned something, over enough groups, and for a
/// contest run, a whole run.
pub fn per_group(session: &SessionResult) -> Option<f64> {
    earned(session, false)
}

/// What a session earned a group, if it would go on the board as a
/// session that was `assisted` or not: for a contest run, a whole run with
/// Show copy on or off as `assisted` says.
fn earned(session: &SessionResult, assisted: bool) -> Option<f64> {
    let groups = session.groups.len();
    (session.score.is_finite()
        && session.score > 0.0
        && groups >= BOARD_MIN_GROUPS
        && if session.char_set_mode == CharSetMode::Contest {
            a_whole_run(session, assisted)
        } else {
            !assisted
        })
    .then(|| session.score / groups as f64)
}

/// Whether a contest run says something of a whole run: its log reads, it
/// ran five minutes or more to a limit of five or more, and it logged a
/// QSO clean; and whether Show copy was on is as `assisted` says. A run
/// whose log an older build dropped is left off.
fn a_whole_run(session: &SessionResult, assisted: bool) -> bool {
    let groups = &session.groups;
    let run = session
        .contest
        .as_ref()
        .and_then(|log| log.run(groups.len()));
    run.is_some_and(|run| {
        run.assisted == assisted
            && u64::from(run.limit_s) >= BOARD_RUN_MIN_S
            && run.run_ms >= BOARD_RUN_MIN_S * 1_000
            && run.clean_qsos(groups).is_some_and(|clean| clean > 0)
    })
}

/// What a board ranks the sessions it holds by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ranking {
    /// What a session earned a group: the one measure every mode shares.
    #[default]
    PerGroup,
    /// A run's verified contest score, among the runs set to this length,
    /// in seconds: a longer run scores more for the same copy.
    Verified(u32),
    /// The QSOs a run logged clean, an hour of the run: fair between runs
    /// of any length.
    CleanRate,
}

impl Ranking {
    /// What `session` is ranked by on its own board: a run by its
    /// contest's score among runs of its length, or by its clean rate in a
    /// contest without a score; any other session by what it earned a
    /// group.
    pub fn of(session: &SessionResult) -> Self {
        let scored = session.contest_kind().is_some_and(ContestKind::scored);
        match session.contest_run() {
            Some(run) if scored => Self::Verified(run.limit_s),
            Some(_) => Self::CleanRate,
            None => Self::PerGroup,
        }
    }
}

/// What `session` is ranked by under `ranking`, if it goes on a board that
/// ranks so: it has to go on the board at all ([`per_group`]), with Show
/// copy as `assisted` says; and under a contest's own ranking be a run,
/// with a score, of the length ranked.
fn rated(session: &SessionResult, ranking: Ranking, assisted: bool) -> Option<f64> {
    let per_group = earned(session, assisted)?;
    let run = || session.contest_run();
    match ranking {
        Ranking::PerGroup => Some(per_group),
        Ranking::Verified(limit_s) => {
            let run = run().filter(|run| run.limit_s == limit_s)?;
            Some(f64::from(run.score(&session.groups)?.verified.score))
        }
        Ranking::CleanRate => {
            let run = run()?;
            let clean = run.clean_qsos(&session.groups)?;
            Some(clean as f64 * 3_600_000.0 / run.run_ms.max(1) as f64)
        }
    }
}

/// What `session` is ranked by under `ranking`, if it is placed on a board
/// that ranks so.
pub fn rating(session: &SessionResult, ranking: Ranking) -> Option<f64> {
    rated(session, ranking, false)
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

    /// Whether `session` is one of the sessions this board ranks.
    pub fn holds(self, session: &SessionResult) -> bool {
        match self {
            Self::All => true,
            Self::Mode(mode) => session.char_set_mode == mode,
            Self::Contest(kind) => session.contest_kind() == Some(kind),
        }
    }
}

/// The sessions `of` holds that go on a board ranked by `ranking`, each
/// with where it sits in the history and what it is ranked by.
fn qualifying(
    sessions: &[SessionResult],
    of: BoardOf,
    ranking: Ranking,
) -> Vec<(usize, &SessionResult, f64)> {
    sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| of.holds(session))
        .filter_map(|(index, session)| {
            rated(session, ranking, false).map(|rating| (index, session, rating))
        })
        .collect()
}

/// Every session `of` holds on the board, best by `ranking` first.
pub fn score_board(sessions: &[SessionResult], of: BoardOf, ranking: Ranking) -> Vec<BoardEntry> {
    let mut ranked = qualifying(sessions, of, ranking);
    ranked.sort_by(|(a_index, a, a_rating), (b_index, b, b_rating)| {
        b_rating
            .total_cmp(a_rating)
            .then(a.timestamp.cmp(&b.timestamp))
            .then(a_index.cmp(b_index))
    });
    ranked
        .into_iter()
        .enumerate()
        .map(|(rank, (index, session, rating))| entry(rank + 1, index, session, rating))
        .collect()
}

/// Session `index`, `session`, on the board at `place`, ranked by `rating`.
fn entry(place: usize, index: usize, session: &SessionResult, rating: f64) -> BoardEntry {
    let run = session.contest_run();
    BoardEntry {
        place,
        index,
        rating,
        score: session.score,
        timestamp: session.timestamp,
        date: session.date.clone(),
        char_wpm: session.char_wpm,
        effective_wpm: session.effective_wpm,
        accuracy: session.accuracy,
        groups: session.groups.len(),
        mode: session.char_set_mode,
        contest: session.contest_kind(),
        qsos: run
            .as_ref()
            .map(|run| run.qsos.iter().filter(|qso| qso.k.is_empty()).count()),
        clean: run.as_ref().and_then(|run| run.clean_qsos(&session.groups)),
        limit_s: run.as_ref().map(|run| run.limit_s),
    }
}

/// The whole runs `of` holds that had Show copy on, newest first: the board
/// lists them greyed, each with place 0, as they are never placed, and
/// they set no record and move no form.
pub fn assisted_runs(sessions: &[SessionResult], of: BoardOf, ranking: Ranking) -> Vec<BoardEntry> {
    let mut assisted: Vec<BoardEntry> = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| of.holds(session))
        .filter_map(|(index, session)| {
            rated(session, ranking, true).map(|rating| entry(0, index, session, rating))
        })
        .collect();
    assisted.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then(b.index.cmp(&a.index)));
    assisted
}

/// Where the session finished at `timestamp` stands among the sessions of
/// its own mode on the board, and a contest run among runs of its own
/// contest, as its own board ranks it ([`Ranking::of`]): its place, and
/// how many there are. Nothing for a session that is not on it.
pub fn board_place(sessions: &[SessionResult], timestamp: u64) -> Option<(usize, usize)> {
    let session = sessions
        .iter()
        .find(|session| session.timestamp == timestamp)?;
    let board = score_board(sessions, BoardOf::of(session), Ranking::of(session));
    let place = board
        .iter()
        .position(|entry| entry.timestamp == timestamp)?;
    Some((place + 1, board.len()))
}

/// The modes the board offers a pill for, in the order the trainer offers
/// them: those it has sessions of, and Contest always, as the pill that
/// opens the contests' own boards says how a run gets on them before there
/// is one, and lists every run on its Runs card ([`run_kinds`]).
pub fn board_modes(sessions: &[SessionResult]) -> Vec<CharSetMode> {
    CharSetMode::ALL
        .into_iter()
        .filter(|mode| {
            *mode == CharSetMode::Contest
                || sessions
                    .iter()
                    .any(|session| session.char_set_mode == *mode && per_group(session).is_some())
        })
        .collect()
}

/// The contests the history has runs of, placed or not, in the order the
/// card lists them: the Runs card's ([`contest_runs`]).
pub fn run_kinds(sessions: &[SessionResult]) -> Vec<ContestKind> {
    ContestKind::ALL
        .into_iter()
        .filter(|kind| {
            sessions
                .iter()
                .any(|session| session.contest_kind() == Some(*kind))
        })
        .collect()
}

/// One run of a contest, as its Runs card lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct RunEntry {
    /// Where the run sits in the history it was read from.
    pub index: usize,
    pub timestamp: u64,
    pub date: String,
    /// What became of its log: one this build reads gives its QSOs, rate
    /// and error share, and the figures below; one it cannot, or that an
    /// older version dropped, is listed as History lists it.
    pub log: StoredLog,
    /// The length it was set to, and how long it ran, pauses left out, in
    /// seconds; none where its log does not read.
    pub limit_s: Option<u32>,
    pub run_s: Option<u64>,
    /// Points for its copy.
    pub points: f64,
    /// Its contest score, raw and verified; none in a contest without one,
    /// or where its log does not read.
    pub raw: Option<u32>,
    pub verified: Option<u32>,
    /// Show copy was on.
    pub assisted: bool,
    /// The best verified score among the runs of its contest set to its
    /// length, Show copy off: the first to reach it, as on any record list.
    pub best: bool,
}

/// Every run of `kind` the history holds, newest first, with the best
/// verified run of each length marked.
pub fn contest_runs(sessions: &[SessionResult], kind: ContestKind) -> Vec<RunEntry> {
    let mut runs: Vec<RunEntry> = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| session.contest_kind() == Some(kind))
        .map(|(index, session)| {
            let run = session.contest_run();
            let score = run.as_ref().and_then(|run| run.score(&session.groups));
            RunEntry {
                index,
                timestamp: session.timestamp,
                date: session.date.clone(),
                log: session.stored_log().unwrap_or(StoredLog::Dropped),
                limit_s: run.as_ref().map(|run| run.limit_s),
                run_s: run.as_ref().map(|run| run.run_ms / 1_000),
                points: session.score,
                raw: score.as_ref().map(|score| score.raw.score),
                verified: score.as_ref().map(|score| score.verified.score),
                assisted: session.assisted(),
                best: false,
            }
        })
        .collect();
    // The best of each length: the highest verified score, the first to
    // reach it holding it.
    let mut by_age: Vec<usize> = (0..runs.len()).collect();
    by_age.sort_by_key(|at| (runs[*at].timestamp, runs[*at].index));
    let mut best: Vec<(u32, usize)> = Vec::new();
    for at in by_age {
        let run = &runs[at];
        let (Some(limit), Some(verified)) = (run.limit_s, run.verified) else {
            continue;
        };
        if run.assisted || verified == 0 {
            continue;
        }
        match best.iter_mut().find(|(length, _)| *length == limit) {
            Some((_, held)) if runs[*held].verified >= Some(verified) => {}
            Some((_, held)) => *held = at,
            None => best.push((limit, at)),
        }
    }
    for (_, at) in best {
        runs[at].best = true;
    }
    runs.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then(b.index.cmp(&a.index)));
    runs
}

/// The lengths the runs of `kind` on its board were set to, in seconds,
/// shortest first, assisted runs among them: under its verified score, the
/// board ranks the runs of one length against each other.
pub fn run_lengths(sessions: &[SessionResult], kind: ContestKind) -> Vec<u32> {
    let mut lengths: Vec<u32> = sessions
        .iter()
        .filter(|session| session.contest_kind() == Some(kind))
        .filter(|session| earned(session, false).or(earned(session, true)).is_some())
        .filter_map(|session| session.contest_run().map(|run| run.limit_s))
        .collect();
    lengths.sort_unstable();
    lengths.dedup();
    lengths
}

/// The length the latest run of `kind` on its board was set to, in
/// seconds: the one its verified score is shown for first.
pub fn latest_run_length(sessions: &[SessionResult], kind: ContestKind) -> Option<u32> {
    sessions
        .iter()
        .filter(|session| session.contest_kind() == Some(kind))
        .filter(|session| earned(session, false).or(earned(session, true)).is_some())
        .max_by_key(|session| session.timestamp)
        .and_then(|session| session.contest_run().map(|run| run.limit_s))
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
pub(crate) fn median(values: &[f64]) -> Option<f64> {
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
    /// What it is ranked by ([`Ranking`]).
    pub rating: f64,
    /// The best rating up to and including this session.
    pub record: f64,
    /// Whether this session set that record: beat every session before it.
    pub set_record: bool,
    /// The form after it: the median of the last [`FORM_SESSIONS`] sessions,
    /// this one among them.
    pub form: f64,
}

/// Every session `of` holds on the board ranked by `ranking`, oldest
/// first, with the record and the form as they stood after each.
pub fn score_timeline(
    sessions: &[SessionResult],
    of: BoardOf,
    ranking: Ranking,
) -> Vec<TimelinePoint> {
    let mut sorted = qualifying(sessions, of, ranking);
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
                rating: *rating,
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
        let board = score_board(&sessions, BoardOf::All, Ranking::PerGroup);
        let ratings: Vec<f64> = board.iter().map(|entry| entry.rating).collect();
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
        let board = score_board(&[long, short], BoardOf::All, Ranking::PerGroup);
        assert_eq!(board[0].timestamp, 2);
        assert_eq!(board[0].rating, 70.0);
    }

    /// Too few groups say too little: a lucky handful is not a place.
    #[test]
    fn a_session_too_short_is_not_placed() {
        let short = session(1, 900.0, BOARD_MIN_GROUPS - 1, CharSetMode::Koch);
        let just = session(2, 100.0, BOARD_MIN_GROUPS, CharSetMode::Koch);
        assert_eq!(per_group(&short), None);
        assert_eq!(per_group(&just), Some(10.0));
        let board = score_board(&[short, just], BoardOf::All, Ranking::PerGroup);
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].timestamp, 2);
    }

    /// The first to reach a score holds the higher place.
    #[test]
    fn an_equal_score_ranks_below_the_one_set_first() {
        let sessions = [koch(20, 50.0), koch(10, 50.0)];
        let board = score_board(&sessions, BoardOf::All, Ranking::PerGroup);
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
        let board = score_board(
            &sessions,
            BoardOf::Mode(CharSetMode::Contest),
            Ranking::PerGroup,
        );
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
            score_board(
                &sessions,
                BoardOf::Mode(CharSetMode::Contest),
                Ranking::PerGroup
            )
            .len(),
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
        let wpx = score_board(
            &sessions,
            BoardOf::Contest(ContestKind::Wpx),
            Ranking::PerGroup,
        );
        assert_eq!(
            wpx.iter().map(|entry| entry.timestamp).collect::<Vec<_>>(),
            [2]
        );
        let timeline = score_timeline(
            &sessions,
            BoardOf::Contest(ContestKind::SimpleQso),
            Ranking::PerGroup,
        );
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
        let board = score_board(&sessions, BoardOf::All, Ranking::PerGroup);
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].timestamp, 5);
        assert!(score_board(&[], BoardOf::All, Ranking::PerGroup).is_empty());
    }

    #[test]
    fn a_board_can_hold_one_mode() {
        let sessions = [
            koch(1, 30.0),
            session(2, 1_800.0, 20, CharSetMode::Callsign),
            koch(3, 50.0),
            session(4, 900.0, 3, CharSetMode::Words),
        ];
        let calls = score_board(
            &sessions,
            BoardOf::Mode(CharSetMode::Callsign),
            Ranking::PerGroup,
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].place, 1);
        let koch_board = score_board(
            &sessions,
            BoardOf::Mode(CharSetMode::Koch),
            Ranking::PerGroup,
        );
        assert_eq!(koch_board.len(), 2);
        assert_eq!(koch_board[0].rating, 50.0);
        assert_eq!(
            board_modes(&sessions),
            [
                CharSetMode::Koch,
                CharSetMode::Callsign,
                CharSetMode::Contest
            ],
            "words have only a session too short; Contest is always offered"
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
        let timeline = score_timeline(&sessions, BoardOf::All, Ranking::PerGroup);
        let at: Vec<u64> = timeline.iter().map(|point| point.timestamp).collect();
        assert_eq!(at, [10, 20, 30, 40]);
        let records: Vec<f64> = timeline.iter().map(|point| point.record).collect();
        assert_eq!(records, [30.0, 30.0, 80.0, 90.0]);
        let set: Vec<bool> = timeline.iter().map(|point| point.set_record).collect();
        assert_eq!(set, [true, false, true, true]);
        // Equalling the record does not set it.
        let tie = score_timeline(
            &[koch(1, 30.0), koch(2, 30.0)],
            BoardOf::All,
            Ranking::PerGroup,
        );
        assert!(!tie[1].set_record);
    }

    /// The form is the median of the latest sessions: one bad day barely
    /// moves it, and sessions older than the window do not count.
    #[test]
    fn the_form_is_the_median_of_the_latest_sessions() {
        let mut sessions: Vec<SessionResult> = (0..9).map(|n| koch(n, 60.0)).collect();
        sessions.push(koch(9, 5.0));
        let timeline = score_timeline(&sessions, BoardOf::All, Ranking::PerGroup);
        assert_eq!(timeline.last().map(|point| point.form), Some(60.0));
        // Two sessions: the mean of the two.
        let pair = score_timeline(
            &[koch(1, 40.0), koch(2, 60.0)],
            BoardOf::All,
            Ranking::PerGroup,
        );
        assert_eq!(pair[1].form, 50.0);
        // Only the last ten count.
        let mut long: Vec<SessionResult> = (0..10).map(|n| koch(n, 10.0)).collect();
        long.extend((10..20).map(|n| koch(n, 70.0)));
        let timeline = score_timeline(&long, BoardOf::All, Ranking::PerGroup);
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
        let board = score_board(
            &[copied, guessed],
            BoardOf::Mode(CharSetMode::Contest),
            Ranking::PerGroup,
        );
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
        let board = score_board(
            &[finished, walked],
            BoardOf::Mode(CharSetMode::Contest),
            Ranking::PerGroup,
        );
        let ranked: Vec<(u64, f64)> = board
            .iter()
            .map(|entry| (entry.timestamp, entry.rating))
            .collect();
        assert_eq!(
            board[0].timestamp, 1,
            "the run that walked away ranks first: {ranked:?}"
        );
    }

    /// A run of `minutes` minutes on seed `seed`, worked by an operator of
    /// `habit`, with Show copy as `show_copy` says, kept at `timestamp`.
    fn worked_run(
        seed: u64,
        minutes: u32,
        habit: crate::contest::bot::script::Habit,
        show_copy: bool,
        timestamp: u64,
    ) -> SessionResult {
        use crate::contest::bot::Operator;
        use crate::contest::bot::script::work;
        let mut settings = run_settings(25.0);
        settings.contest.contest_minutes = minutes;
        settings.contest.contest_show_copy = show_copy;
        let sampler = crate::Sampler::new(&[], &settings, seed);
        let mut runtime = Runtime::start(seed, settings, sampler);
        work(&mut runtime, &mut Operator::new(habit), &[]);
        runtime.machine.result(timestamp, "2026-10-20".into())
    }

    /// A contest's own board ranks its runs by the contest's own measure:
    /// the verified score among runs set to the same length, or the QSOs
    /// logged clean an hour, whatever the length. A run's place is said as
    /// its contest ranks it first: by its score, among runs of its length.
    #[test]
    fn a_contest_board_ranks_by_the_contests_own_measure() {
        use crate::contest::bot::script::Habit;
        let fast = worked_run(3, 5, Habit::Perfect, false, 1);
        let slow = worked_run(3, 5, Habit::HumanPaced, false, 2);
        let long = worked_run(3, 10, Habit::HumanPaced, false, 3);
        let sessions = [fast.clone(), slow.clone(), long];
        let wpx = BoardOf::Contest(ContestKind::Wpx);
        assert_eq!(run_lengths(&sessions, ContestKind::Wpx), [300, 600]);
        assert_eq!(latest_run_length(&sessions, ContestKind::Wpx), Some(600));
        assert!(run_lengths(&sessions, ContestKind::Sprint).is_empty());
        let verified = |session: &SessionResult| {
            let run = session.contest_run().expect("a log");
            f64::from(run.score(&session.groups).expect("a score").verified.score)
        };
        assert!(verified(&fast) > verified(&slow), "more QSOs, more score");
        let five = score_board(&sessions, wpx, Ranking::Verified(300));
        assert_eq!(
            five.iter()
                .map(|entry| (entry.timestamp, entry.rating))
                .collect::<Vec<_>>(),
            [(1, verified(&fast)), (2, verified(&slow))]
        );
        assert_eq!(five[0].limit_s, Some(300));
        assert!(five[0].clean.is_some_and(|clean| clean > 0));
        let ten = score_board(&sessions, wpx, Ranking::Verified(600));
        assert_eq!(ten.len(), 1);
        assert_eq!(ten[0].timestamp, 3);
        // Clean QSOs an hour put every length on one board.
        let rate = score_board(&sessions, wpx, Ranking::CleanRate);
        assert_eq!(rate.len(), 3);
        assert_eq!(rate[0].timestamp, 1);
        let clean = fast
            .contest_run()
            .and_then(|run| run.clean_qsos(&fast.groups));
        let hours = fast.contest_run().expect("a log").run_ms as f64 / 3_600_000.0;
        assert!((rate[0].rating - clean.expect("clean QSOs") as f64 / hours).abs() < 1e-9);
        assert_eq!(
            score_timeline(&sessions, wpx, Ranking::Verified(300)).len(),
            2
        );
        // Placed as its own contest ranks it.
        assert_eq!(Ranking::of(&fast), Ranking::Verified(300));
        assert_eq!(board_place(&sessions, 2), Some((2, 2)));
        assert_eq!(board_place(&sessions, 3), Some((1, 1)));
        let unscored = SessionResult {
            alphabet_fingerprint: "contest:simpleQso".into(),
            ..fast
        };
        assert_eq!(Ranking::of(&unscored), Ranking::CleanRate);
        assert_eq!(Ranking::of(&koch(9, 50.0)), Ranking::PerGroup);
        // Points a group are no contest's measure: a run of groups is
        // never on a board ranked by one.
        assert!(score_board(&[koch(9, 50.0)], BoardOf::All, Ranking::CleanRate).is_empty());
    }

    /// A whole run with Show copy on is assisted: never placed, however it
    /// scores, but listed greyed apart from the places on every board that
    /// holds it, and out of the record and the form.
    #[test]
    fn an_assisted_run_is_listed_but_never_placed() {
        use crate::contest::bot::script::Habit;
        let plain = worked_run(3, 5, Habit::Perfect, false, 1);
        let assisted = worked_run(3, 5, Habit::Perfect, true, 2);
        assert!(assisted.assisted() && !plain.assisted());
        assert_eq!(assisted.alphabet_fingerprint, "contest:wpx:a");
        assert!(assisted.score >= plain.score, "it copied as well");
        assert_eq!(per_group(&assisted), None);
        let sessions = [plain, assisted];
        for of in [
            BoardOf::All,
            BoardOf::Mode(CharSetMode::Contest),
            BoardOf::Contest(ContestKind::Wpx),
        ] {
            let placed: Vec<u64> = score_board(&sessions, of, Ranking::PerGroup)
                .iter()
                .map(|entry| entry.timestamp)
                .collect();
            assert_eq!(placed, [1], "{of:?}");
            let listed: Vec<(u64, usize)> = assisted_runs(&sessions, of, Ranking::PerGroup)
                .iter()
                .map(|entry| (entry.timestamp, entry.place))
                .collect();
            assert_eq!(listed, [(2, 0)], "{of:?}");
            let timeline = score_timeline(&sessions, of, Ranking::PerGroup);
            assert_eq!(timeline.len(), 1, "no record, no form: {of:?}");
        }
        assert_eq!(board_place(&sessions, 2), None);
        assert_eq!(board_kinds(&sessions), [ContestKind::Wpx]);
        assert!(
            assisted_runs(
                &sessions,
                BoardOf::Mode(CharSetMode::Koch),
                Ranking::PerGroup
            )
            .is_empty()
        );
        assert!(
            assisted_runs(
                &sessions,
                BoardOf::Contest(ContestKind::Sprint),
                Ranking::PerGroup
            )
            .is_empty()
        );
        // A run too short for the board is no assisted run on it either,
        // but the Contest pill shows for it, with its Runs card.
        let short = worked_run(3, 2, Habit::Perfect, true, 3);
        assert!(
            assisted_runs(
                std::slice::from_ref(&short),
                BoardOf::All,
                Ranking::PerGroup
            )
            .is_empty()
        );
        assert!(board_kinds(std::slice::from_ref(&short)).is_empty());
        assert_eq!(run_kinds(std::slice::from_ref(&short)), [ContestKind::Wpx]);
        assert_eq!(
            board_modes(&[short, koch(4, 50.0)]),
            [CharSetMode::Koch, CharSetMode::Contest]
        );
    }

    /// The Runs card lists every run of its contest, newest first: its
    /// length, both scores and the share of QSOs with an error; the best
    /// verified run of each length marked, the first to reach it holding
    /// it; an assisted run marked and never best; and a run whose log this
    /// build cannot read, or an older one dropped, listed as History lists
    /// it.
    #[test]
    fn the_runs_card_lists_every_run_of_its_contest() {
        use crate::contest::bot::script::Habit;
        let first = worked_run(3, 5, Habit::Perfect, false, 1);
        let assisted = SessionResult {
            score: first.score * 2.0,
            ..worked_run(3, 5, Habit::Perfect, true, 2)
        };
        let short = worked_run(5, 2, Habit::Perfect, false, 3);
        let busted = worked_run(3, 5, Habit::DigitOff, false, 4);
        let dropped = SessionResult {
            contest: None,
            timestamp: 5,
            ..first.clone()
        };
        let mut unreadable = SessionResult {
            timestamp: 6,
            ..first.clone()
        };
        if let Some(log) = unreadable.contest.as_mut() {
            log.0["v"] = serde_json::json!(2);
        }
        // The same run again later: it ties the first, which holds it.
        let again = SessionResult {
            timestamp: 7,
            ..first.clone()
        };
        let koch = koch(8, 50.0);
        let sessions = [
            first.clone(),
            assisted,
            short,
            busted,
            dropped,
            unreadable,
            again,
            koch,
        ];
        assert_eq!(run_kinds(&sessions), [ContestKind::Wpx]);
        assert!(contest_runs(&sessions, ContestKind::Sprint).is_empty());
        let runs = contest_runs(&sessions, ContestKind::Wpx);
        let at: Vec<u64> = runs.iter().map(|run| run.timestamp).collect();
        assert_eq!(at, [7, 6, 5, 4, 3, 2, 1], "newest first, Koch left out");
        let lengths: Vec<Option<u32>> = runs.iter().map(|run| run.limit_s).collect();
        assert_eq!(
            lengths,
            [
                Some(300),
                None,
                None,
                Some(300),
                Some(120),
                Some(300),
                Some(300)
            ]
        );
        let best: Vec<u64> = runs
            .iter()
            .filter(|run| run.best)
            .map(|run| run.timestamp)
            .collect();
        assert_eq!(best, [3, 1], "one a length; the assisted run never");
        let (assisted, first_row) = (&runs[5], &runs[6]);
        assert!(assisted.assisted && !assisted.best);
        assert!(!first_row.assisted);
        assert!(matches!(first_row.log, StoredLog::Read(_)));
        assert_eq!(runs[2].log, StoredLog::Dropped);
        assert_eq!(runs[1].log, StoredLog::Unreadable);
        assert_eq!((runs[1].raw, runs[1].verified), (None, None));
        let share = |run: &RunEntry| match &run.log {
            StoredLog::Read(summary) => summary.error_share(),
            _ => None,
        };
        assert_eq!(share(first_row), Some(0.0));
        assert_eq!(first_row.points, first.score);
        assert!(first_row.verified.is_some_and(|verified| verified > 0));
        assert!(first_row.raw >= first_row.verified);
        // A digit off in every number: every QSO has an error.
        assert_eq!(share(&runs[3]), Some(1.0));
        assert_eq!(runs[3].verified, Some(0));
        assert!(!runs[3].best);
        assert_eq!(
            first_row.run_s,
            first.contest_run().map(|run| run.run_ms / 1_000)
        );
    }
}
