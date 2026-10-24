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

use crate::alignment::CopyTally;
use crate::session::{GroupResult, SessionResult};
use crate::settings::CharSetMode;

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
    /// How many sessions got this far.
    pub sessions: usize,
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
    /// The first stretch that falls [`DROP_POINTS`] or more below the start,
    /// if one does.
    pub fn first_drop(&self) -> Option<&EndurancePoint> {
        self.points
            .iter()
            .find(|point| point.change <= -DROP_POINTS)
    }

    /// How far into a session copy holds within [`DROP_POINTS`] of its
    /// start: the end of the last stretch before the first drop, or of the
    /// last stretch drawn when nothing drops.
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
            // cut off by the clock, would be a stretch of a few QSOs.
            let whole = (run.run_ms / stretch_ms) as usize;
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

/// Character accuracy over `groups`, from 0 to 1; none when nothing was
/// sent in them.
fn accuracy(groups: &[&GroupResult]) -> Option<f64> {
    let mut tally = CopyTally::default();
    for group in groups {
        tally.add(&group.sent, &group.received);
    }
    tally.accuracy_where(|_| true)
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
    let curves: Vec<Vec<Option<f64>>> = ordered
        .into_iter()
        .map(|session| {
            stretches(session, unit)
                .iter()
                .map(|groups| accuracy(groups))
                .collect::<Vec<_>>()
        })
        .filter(|curve| curve.len() >= 2 && curve[0].is_some())
        .take(SESSIONS_READ)
        .collect();
    if curves.len() < ENDURANCE_MIN_SESSIONS {
        return None;
    }
    let opening: Vec<f64> = curves.iter().filter_map(|curve| curve[0]).collect();
    let opening_pct = 100.0 * opening.iter().sum::<f64>() / opening.len() as f64;
    let longest = curves.iter().map(Vec::len).max().unwrap_or(0);
    let size = unit.size();
    let mut points = Vec::new();
    for at in 0..longest {
        let changes: Vec<f64> = curves
            .iter()
            .filter_map(|curve| Some(100.0 * (curve.get(at).copied()?? - curve[0]?)))
            .collect();
        if changes.len() < ENDURANCE_MIN_SESSIONS {
            break;
        }
        let at = at as u32;
        let (from, to) = match unit {
            StretchUnit::Groups => (at * size + 1, (at + 1) * size),
            StretchUnit::Minutes => (at * size, (at + 1) * size),
        };
        points.push(EndurancePoint {
            from,
            to,
            change: changes.iter().sum::<f64>() / changes.len() as f64,
            sessions: changes.len(),
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
                recognition_ms: Vec::new(),
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
        assert_eq!(curve.first_drop().map(|p| p.from), Some(11));
        assert_eq!(curve.holds_to(), 10);
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

    /// A contest run is cut by run time: each QSO's groups go to the
    /// stretch it was logged in, and the stretch the clock cut short is
    /// left out.
    #[test]
    fn a_contest_run_is_cut_by_minutes_of_run_time() {
        // Four QSOs of two groups: two logged in the first two minutes, one
        // in the next two, and one after five minutes, past the last whole
        // stretch of a run of five and a half.
        let qso = |t: u64| QsoRecord {
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
        };
        let run = |timestamp: u64, late_wrong: bool| {
            let mut out = session(
                timestamp,
                &[true, true, true, true, !late_wrong, true, false, false],
            );
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
                run_ms: 330_000,
                paused_ms: 0,
                my_wpm: 30.0,
                assisted: false,
                qsos: vec![qso(30_000), qso(100_000), qso(150_000), qso(310_000)],
            }));
            out
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
}
