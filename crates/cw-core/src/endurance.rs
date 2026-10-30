//! How copy holds up as a session goes on: character accuracy through each
//! stretch of a session, held against the session's own start.
//!
//! Every session is measured against itself, so what differs from one to
//! the next (the level, the speed, the day) drops out, and what is left is
//! what the length of the session does: a warm-up where accuracy climbs,
//! fatigue where it falls. A session of groups is cut into stretches of
//! [`GROUPS_A_STRETCH`] groups; a contest run, which is timed, into stretches
//! of [`MINUTES_A_STRETCH`] minutes of run time, by when each QSO was
//! logged.
//!
//! A few sessions hold few groups a stretch, and a stretch of a steady
//! learner's copy wanders a long way from the start by chance alone. So a
//! stretch is only called a drop, or a warm-up, when it stands further from
//! the start than chance would put it (a few times its noise),
//! each group counting as one try: the characters of a group are lost
//! together as often as not.

use crate::alignment::CopyTally;
use crate::session::{GroupResult, SessionResult};
use crate::settings::{CONTEST_MINUTES_MAX, CharSetMode};

/// Groups in a stretch of a session.
pub const GROUPS_A_STRETCH: u32 = 5;
/// Minutes of run time in a stretch of a contest run.
pub const MINUTES_A_STRETCH: u32 = 2;
/// How many sessions have to reach a stretch before it is drawn: fewer is
/// one good or bad day.
pub const ENDURANCE_MIN_SESSIONS: usize = 3;
/// How far below its start a stretch has to fall to count as a drop, in
/// points of accuracy.
pub const DROP_POINTS: f64 = 5.0;
/// How far above its start a stretch has to climb to count as a warm-up,
/// in points of accuracy.
pub const WARM_UP_POINTS: f64 = 3.0;
/// How many times its noise a stretch has to stand from the start to be
/// called a drop or a warm-up: chance alone puts one that far about once in
/// two hundred.
const CLEAR_SPREADS: f64 = 2.58;
/// The most recent sessions read: endurance grows with practice, and what
/// it was months ago is not what it is.
const SESSIONS_READ: usize = 30;

/// What a stretch is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StretchUnit {
    Groups,
    Minutes,
}

impl StretchUnit {
    fn size(self) -> u32 {
        match self {
            Self::Groups => GROUPS_A_STRETCH,
            Self::Minutes => MINUTES_A_STRETCH,
        }
    }
}

/// One stretch of a session, over every session that got that far.
#[derive(Clone, Debug, PartialEq)]
pub struct EndurancePoint {
    /// Where the stretch starts and ends: groups counted from one, both
    /// ends in; or minutes of run time, from the first to the last.
    pub from: u32,
    pub to: u32,
    /// How far character accuracy stood from the session's opening stretch
    /// here, in points, averaged over the sessions that got this far.
    pub change: f64,
    /// How far chance alone moves `change`, in points: its standard error
    /// for a learner whose copy holds steady, from how many groups the
    /// stretch and the opening hold in each session.
    pub noise: f64,
    /// How many sessions got this far.
    pub sessions: usize,
}

impl EndurancePoint {
    /// Whether it falls [`DROP_POINTS`] or more below the start, and further
    /// than chance would put it.
    pub fn drops(&self) -> bool {
        self.change <= -DROP_POINTS && self.change + CLEAR_SPREADS * self.noise < 0.0
    }

    /// Whether it climbs [`WARM_UP_POINTS`] or more above the start, and
    /// further than chance would put it.
    pub fn climbs(&self) -> bool {
        self.change >= WARM_UP_POINTS && self.change - CLEAR_SPREADS * self.noise > 0.0
    }
}

/// How copy holds up through the sessions of one character set.
#[derive(Clone, Debug, PartialEq)]
pub struct Endurance {
    pub unit: StretchUnit,
    /// Character accuracy over the opening stretch, averaged over the
    /// sessions read, in percent.
    pub opening_pct: f64,
    /// The opening stretch, at no change, then every one reached by enough
    /// sessions, in order.
    pub points: Vec<EndurancePoint>,
    /// How many sessions were read: those long enough to have a second
    /// stretch.
    pub sessions: usize,
}

impl Endurance {
    /// The first stretch that drops ([`EndurancePoint::drops`]), if one does.
    pub fn first_drop(&self) -> Option<&EndurancePoint> {
        self.points.iter().find(|point| point.drops())
    }

    /// The best of the stretches after the opening that climb above it
    /// ([`EndurancePoint::climbs`]), if any does: where a warm-up peaks.
    pub fn warm_up(&self) -> Option<&EndurancePoint> {
        self.points
            .iter()
            .skip(1)
            .filter(|point| point.climbs())
            .max_by(|a, b| a.change.total_cmp(&b.change))
    }

    /// How far into a session copy holds: the end of the last stretch
    /// before the first drop, or of the last stretch drawn when nothing
    /// drops.
    pub fn holds_to(&self) -> u32 {
        let before = match self.first_drop() {
            Some(drop) => self
                .points
                .iter()
                .take_while(|point| point.from < drop.from)
                .last(),
            None => self.points.last(),
        };
        before.map_or(0, |point| point.to)
    }
}

/// The groups of a session in its stretches: each a run of groups, in order.
fn stretches(session: &SessionResult, unit: StretchUnit) -> Vec<Vec<&GroupResult>> {
    match unit {
        StretchUnit::Groups => session
            .groups
            .chunks(GROUPS_A_STRETCH as usize)
            .filter(|chunk| chunk.len() == GROUPS_A_STRETCH as usize)
            .map(|chunk| chunk.iter().collect())
            .collect(),
        StretchUnit::Minutes => {
            let Some(run) = session
                .contest
                .as_ref()
                .and_then(|log| log.run(session.groups.len()))
            else {
                return Vec::new();
            };
            let stretch_ms = u64::from(MINUTES_A_STRETCH) * 60_000;
            // Only the stretches the run lasted to the end of: the last,
            // cut off by the clock, would be a stretch of a few QSOs. No run
            // goes past the longest a run can be set to, whatever a log
            // read back says.
            let longest_ms = u64::from(CONTEST_MINUTES_MAX) * 60_000;
            let whole = (run.run_ms.min(longest_ms) / stretch_ms) as usize;
            let mut out: Vec<Vec<&GroupResult>> = vec![Vec::new(); whole];
            let mut groups = session.groups.iter();
            for qso in &run.qsos {
                let owned: Vec<&GroupResult> = groups.by_ref().take(qso.g as usize).collect();
                if let Some(stretch) = out.get_mut((qso.t / stretch_ms) as usize) {
                    stretch.extend(owned);
                }
            }
            out
        }
    }
}

/// What was copied over a stretch of one session.
#[derive(Clone, Copy, Debug)]
struct Copied {
    /// Character accuracy, from 0 to 1.
    right: f64,
    /// How many groups it was read from.
    groups: f64,
}

/// What was copied over `groups`; none when nothing was sent in them.
fn copied(groups: &[&GroupResult]) -> Option<Copied> {
    let mut tally = CopyTally::default();
    for group in groups {
        tally.add(&group.sent, &group.received);
    }
    Some(Copied {
        right: tally.accuracy_where(|_| true)?,
        groups: groups.len() as f64,
    })
}

/// How far chance alone moves the change from the opening to a stretch,
/// averaged over `pairs` of the two, one a session, in points: a
/// two-proportion test's standard error, each group one try at the accuracy
/// of them all together.
fn chance_noise(pairs: &[(Copied, Copied)]) -> f64 {
    let groups: f64 = pairs.iter().map(|(o, s)| o.groups + s.groups).sum();
    let right = pairs
        .iter()
        .map(|(o, s)| o.right * o.groups + s.right * s.groups)
        .sum::<f64>()
        / groups;
    let tries: f64 = pairs
        .iter()
        .map(|(o, s)| 1.0 / o.groups + 1.0 / s.groups)
        .sum();
    100.0 * (right * (1.0 - right) * tries).sqrt() / pairs.len() as f64
}

/// How copy holds up through the sessions of `mode`: in stretches of groups,
/// or of run time for a contest. Nothing until enough sessions have a second
/// stretch to draw.
pub fn endurance(sessions: &[SessionResult], mode: CharSetMode) -> Option<Endurance> {
    let unit = if mode == CharSetMode::Contest {
        StretchUnit::Minutes
    } else {
        StretchUnit::Groups
    };
    // Each session's accuracy through its stretches, newest sessions first,
    // as long as it has two stretches to compare.
    let mut ordered: Vec<&SessionResult> = sessions
        .iter()
        .filter(|session| session.char_set_mode == mode)
        .collect();
    ordered.sort_by_key(|session| std::cmp::Reverse(session.timestamp));
    let curves: Vec<Vec<Option<Copied>>> = ordered
        .into_iter()
        .map(|session| {
            stretches(session, unit)
                .iter()
                .map(|groups| copied(groups))
                .collect::<Vec<_>>()
        })
        .filter(|curve| curve.len() >= 2 && curve[0].is_some())
        .take(SESSIONS_READ)
        .collect();
    if curves.len() < ENDURANCE_MIN_SESSIONS {
        return None;
    }
    let opening: Vec<f64> = curves
        .iter()
        .filter_map(|curve| Some(curve[0]?.right))
        .collect();
    let opening_pct = 100.0 * opening.iter().sum::<f64>() / opening.len() as f64;
    let longest = curves.iter().map(Vec::len).max().unwrap_or(0);
    let size = unit.size();
    let mut points = Vec::new();
    for at in 0..longest {
        let pairs: Vec<(Copied, Copied)> = curves
            .iter()
            .filter_map(|curve| Some((curve[0]?, curve.get(at).copied()??)))
            .collect();
        if pairs.len() < ENDURANCE_MIN_SESSIONS {
            break;
        }
        let change = pairs
            .iter()
            .map(|(opening, stretch)| 100.0 * (stretch.right - opening.right))
            .sum::<f64>()
            / pairs.len() as f64;
        let at = at as u32;
        let (from, to) = match unit {
            StretchUnit::Groups => (at * size + 1, (at + 1) * size),
            StretchUnit::Minutes => (at * size, (at + 1) * size),
        };
        points.push(EndurancePoint {
            from,
            to,
            change,
            noise: chance_noise(&pairs),
            sessions: pairs.len(),
        });
    }
    (points.len() >= 2).then_some(Endurance {
        unit,
        opening_pct,
        points,
        sessions: curves.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::{ContestLog, ContestRun, QsoRecord};
    use crate::session::SessionTiming;

    /// A session of `mode` finished at `timestamp`, its groups copied as
    /// `copied` says: each group "KMRS", copied whole or with every
    /// character wrong.
    fn session(timestamp: u64, copied: &[bool]) -> SessionResult {
        let mut out: SessionResult = serde_json::from_value(serde_json::json!({
            "date": "2026-09-01",
            "timestamp": timestamp,
            "groups": [],
            "groupTimings": [],
            "accuracy": 1.0,
            "avgResponseMs": 0.0,
            "totalChars": 0,
            "score": 0.0,
            "charSetMode": "koch"
        }))
        .expect("a session");
        out.groups = copied
            .iter()
            .map(|right| GroupResult {
                sent: "KMRS".to_string(),
                received: if *right { "KMRS" } else { "EEEE" }.to_string(),
                correct: *right,
            })
            .collect();
        out.group_timings = copied
            .iter()
            .map(|_| SessionTiming {
                time_to_complete_ms: 1_000.0,
                char_wpm: Some(20.0),
                effective_wpm: None,
                sends: 1,
                ..SessionTiming::default()
            })
            .collect();
        out
    }

    /// `stretches` of five groups, each with this many of them copied wrong.
    fn missed(stretches: &[usize]) -> Vec<bool> {
        stretches
            .iter()
            .flat_map(|wrong| (0..GROUPS_A_STRETCH as usize).map(move |n| n >= *wrong))
            .collect()
    }

    /// Every session against its own start: a session that starts badly
    /// and one that starts well both count only how far they moved.
    #[test]
    fn each_session_is_held_against_its_own_start() {
        let sessions = [
            session(1, &missed(&[0, 0, 1, 2])),
            session(2, &missed(&[1, 1, 2, 3])),
            session(3, &missed(&[0, 0, 1, 2])),
        ];
        let curve = endurance(&sessions, CharSetMode::Koch).expect("a curve");
        assert_eq!(curve.unit, StretchUnit::Groups);
        assert_eq!(curve.sessions, 3);
        // Openings of 100%, 80% and 100%.
        assert!((curve.opening_pct - 280.0 / 3.0).abs() < 1e-9);
        let changes: Vec<f64> = curve.points.iter().map(|p| p.change).collect();
        assert_eq!(changes, [0.0, 0.0, -20.0, -40.0]);
        let ranges: Vec<(u32, u32)> = curve.points.iter().map(|p| (p.from, p.to)).collect();
        assert_eq!(ranges, [(1, 5), (6, 10), (11, 15), (16, 20)]);
        // Fifteen groups a stretch: a steady learner's copy could fall that
        // far by chance, so it is not yet called a drop.
        assert!(curve.first_drop().is_none(), "{:?}", curve.points);
        assert_eq!(curve.holds_to(), 20);
    }

    /// A drop is called once enough sessions show it: a stretch a fifth
    /// below the start in twelve sessions is past what chance would do.
    #[test]
    fn a_drop_is_called_once_enough_sessions_show_it() {
        let sessions: Vec<SessionResult> = (0..12)
            .map(|n| session(n, &missed(&[0, 0, 1, 2])))
            .collect();
        let curve = endurance(&sessions, CharSetMode::Koch).expect("a curve");
        let drop = curve.first_drop().expect("a drop");
        assert_eq!(drop.from, 11);
        assert!((drop.change + 20.0).abs() < 1e-9);
        assert!(drop.noise > 0.0 && drop.change + 2.0 * drop.noise < 0.0);
        assert_eq!(curve.holds_to(), 10);
        assert!(curve.warm_up().is_none());
        // And a climb past chance is a warm-up, at its best stretch.
        let warming: Vec<SessionResult> = (0..12)
            .map(|n| session(n, &missed(&[2, 0, 1, 1])))
            .collect();
        let curve = endurance(&warming, CharSetMode::Koch).expect("a curve");
        assert!(curve.first_drop().is_none());
        let warm = curve.warm_up().expect("a warm-up");
        assert_eq!(warm.from, 6);
        assert!((warm.change - 40.0).abs() < 1e-9);
    }

    /// A learner who copies a steady nine characters in ten all the way
    /// through is almost never told they fade or warm up, however many
    /// sessions are read, even when every miss loses a whole group; one
    /// who tires is told so once the sessions add up.
    #[test]
    fn a_steady_learner_is_almost_never_said_to_fade_or_warm_up() {
        use crate::rng::{Rng, SplitMix64};
        // Sessions of twenty groups of two or three characters, each group
        // lost whole when it is missed, with the chance of copying the
        // `n`th one at `right(n)`.
        let history = |rng: &mut SplitMix64, sessions: u64, right: &dyn Fn(usize) -> f64| {
            (0..sessions)
                .map(|timestamp| {
                    let mut out = session(timestamp, &[]);
                    out.groups = (0..20)
                        .map(|n| {
                            let sent = "KMRSU"[..rng.usize_in(2, 3)].to_string();
                            let correct = rng.f64() < right(n);
                            let received = if correct {
                                sent.clone()
                            } else {
                                "E".repeat(sent.len())
                            };
                            GroupResult {
                                sent,
                                received,
                                correct,
                            }
                        })
                        .collect();
                    out
                })
                .collect::<Vec<_>>()
        };
        let mut rng = SplitMix64::new(62);
        for sessions in [3, 5, 10, 30] {
            let said = (0..200)
                .filter(|_| {
                    let curve =
                        endurance(&history(&mut rng, sessions, &|_| 0.9), CharSetMode::Koch)
                            .expect("a curve");
                    curve.first_drop().is_some() || curve.warm_up().is_some()
                })
                .count();
            assert!(said <= 10, "{said} of 200 at {sessions} sessions");
        }
        let tiring = |n: usize| if n < 10 { 0.92 } else { 0.75 };
        let told = (0..50)
            .filter(|_| {
                endurance(&history(&mut rng, 30, &tiring), CharSetMode::Koch)
                    .expect("a curve")
                    .first_drop()
                    .is_some_and(|drop| drop.from == 11)
            })
            .count();
        assert!(told >= 45, "{told} of 50");
    }

    /// A stretch too few sessions reach is not drawn, nor anything after
    /// it; a session too short to compare is not read; nor is another
    /// character set's.
    #[test]
    fn only_what_enough_sessions_reach_is_drawn() {
        let mut sessions = vec![
            session(1, &missed(&[0, 0, 0, 0, 0, 0])),
            session(2, &missed(&[0, 0, 0, 0])),
            session(3, &missed(&[0, 0, 0, 0])),
            // One stretch and a bit: nothing to compare.
            session(4, &missed(&[0])),
        ];
        sessions[3].groups.truncate(8);
        let mut digits = session(5, &missed(&[0, 5, 5, 5]));
        digits.char_set_mode = CharSetMode::Digits;
        sessions.push(digits);
        let curve = endurance(&sessions, CharSetMode::Koch).expect("a curve");
        assert_eq!(curve.sessions, 3);
        assert_eq!(curve.points.len(), 4, "the sixth stretch has one session");
        assert!(curve.first_drop().is_none());
        assert_eq!(curve.holds_to(), 20);
        // Two sessions are not enough to say anything.
        assert!(endurance(&sessions[..2], CharSetMode::Koch).is_none());
        assert!(endurance(&sessions, CharSetMode::Digits).is_none());
        assert!(endurance(&[], CharSetMode::Koch).is_none());
    }

    /// Only the most recent sessions are read.
    #[test]
    fn only_the_latest_sessions_count() {
        let mut sessions: Vec<SessionResult> =
            (0..40).map(|n| session(n, &missed(&[0, 5]))).collect();
        for n in 40..70 {
            sessions.push(session(n, &missed(&[0, 0])));
        }
        let curve = endurance(&sessions, CharSetMode::Koch).expect("a curve");
        assert_eq!(curve.sessions, 30);
        assert_eq!(curve.points[1].change, 0.0, "the old drops are past");
    }

    /// A QSO of two groups logged `t` milliseconds into the run.
    fn qso(t: u64) -> QsoRecord {
        QsoRecord {
            t,
            h: 0,
            g: 2,
            nr: 1,
            k: String::new(),
            c: String::new(),
            l: Vec::new(),
            lc: String::new(),
            s: Vec::new(),
            sc: String::new(),
            a: 0,
        }
    }

    /// A contest run finished at `timestamp`, its groups copied as `copied`
    /// says, of `run_ms` of run time with `qsos` logged.
    fn contest_run(
        timestamp: u64,
        copied: &[bool],
        run_ms: u64,
        qsos: Vec<QsoRecord>,
    ) -> SessionResult {
        let mut out = session(timestamp, copied);
        out.char_set_mode = CharSetMode::Contest;
        out.contest = Some(ContestLog::from(&ContestRun {
            v: crate::contest::LAYOUT,
            kind: "wpx".into(),
            style: "single".into(),
            callers: [1, 1],
            side: String::new(),
            my_country: "DL".into(),
            my_continent: "EU".into(),
            my_zone: 14,
            fields: vec!["nr".into()],
            limit_s: 600,
            run_ms,
            paused_ms: 0,
            my_wpm: 30.0,
            assisted: false,
            qsos,
        }));
        out
    }

    /// A contest run is cut by run time: each QSO's groups go to the
    /// stretch it was logged in, and the stretch the clock cut short is
    /// left out.
    #[test]
    fn a_contest_run_is_cut_by_minutes_of_run_time() {
        // Four QSOs of two groups: two logged in the first two minutes, one
        // in the next two, and one after five minutes, past the last whole
        // stretch of a run of five and a half.
        let run = |timestamp: u64, late_wrong: bool| {
            contest_run(
                timestamp,
                &[true, true, true, true, !late_wrong, true, false, false],
                330_000,
                vec![qso(30_000), qso(100_000), qso(150_000), qso(310_000)],
            )
        };
        let sessions = [run(1, true), run(2, true), run(3, false)];
        assert!(
            sessions[0]
                .contest
                .as_ref()
                .and_then(|log| log.run(8))
                .is_some(),
            "the log reads"
        );
        let curve = endurance(&sessions, CharSetMode::Contest).expect("a curve");
        assert_eq!(curve.unit, StretchUnit::Minutes);
        let ranges: Vec<(u32, u32)> = curve.points.iter().map(|p| (p.from, p.to)).collect();
        assert_eq!(ranges, [(0, 2), (2, 4)]);
        assert_eq!(curve.opening_pct, 100.0);
        // The third QSO's first group wrong in two runs of three: half its
        // stretch's characters each time.
        let expected = -100.0 * (2.0 * 0.5) / 3.0;
        assert!((curve.points[1].change - expected).abs() < 1e-9);
    }

    /// A run read back from a log that says it ran for ever, a backup's
    /// made by hand, is cut at the longest a run can be set to: the card
    /// is drawn rather than asking for more memory than there is.
    #[test]
    fn a_run_that_says_it_ran_for_ever_is_cut_at_the_longest_run() {
        let run = |timestamp: u64| {
            contest_run(
                timestamp,
                &[true; 6],
                u64::MAX,
                vec![qso(30_000), qso(150_000), qso(u64::MAX)],
            )
        };
        let sessions = [run(1), run(2), run(3)];
        let curve = endurance(&sessions, CharSetMode::Contest).expect("a curve");
        let ranges: Vec<(u32, u32)> = curve.points.iter().map(|p| (p.from, p.to)).collect();
        assert_eq!(ranges, [(0, 2), (2, 4)]);
        let longest = stretches(&sessions[0], StretchUnit::Minutes).len();
        assert_eq!(longest as u32, CONTEST_MINUTES_MAX / MINUTES_A_STRETCH);
    }
}
