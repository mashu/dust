//! A contest run as the history keeps it.
//!
//! A run is a [`crate::SessionResult`] like any other: each call and
//! exchange field copied is one of its groups, so every stat counts it. Its
//! log rides beside the groups, and is written so that no build loses the run.
//! A build that knows no contests reads it as a callsign session; one that
//! knows an older log layout keeps the newer log untouched.

use serde::{Deserialize, Serialize};

use super::check::{QsoCheck, QsoText, check_qso};
use super::exchange::Field;
use super::score::{ContestScore, ScoredQso, contest_score};
use super::{ContestKind, RunStyle};
use crate::alignment::CopyTally;
use crate::score::{calculate_total_chars, compute_average_response_ms};
use crate::session::{GroupResult, SessionResult, SessionTiming};
use crate::settings::{CharSetMode, LevelAxis, TrainingSettings};

/// What a run's alphabet fingerprint starts with: `contest:wpx`. A build
/// older than contest mode drops the log on its next save but keeps the
/// fingerprint, so the mark is what still makes the run a run after that.
pub(crate) const CONTEST_MARK: &str = "contest:";

/// The log layout this build writes and reads. It rises only for a change
/// an older reader would misread; a key added with a default does not
/// raise it.
pub(super) const LAYOUT: u32 = 1;

/// A QSO's kind, as `k` keeps it: logged is empty.
pub(super) const LOST: &str = "lost";
pub(super) const NIL: &str = "nil";

/// A run's log, exactly as it was read.
///
/// Kept as plain JSON rather than parsed into a struct, because a struct
/// that reads any layout leniently cannot write a newer layout back
/// unchanged: a field it does not know would be lost, a renamed one
/// defaulted. Held as a value, a log written by a later version goes back to
/// disk as it came, whatever this version makes of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContestLog(pub serde_json::Value);

/// A run's log as this build reads it. Strict: every key without a default
/// below must be there, so a layout this build does not know is not read as
/// one it does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContestRun {
    /// The layout, [`LAYOUT`].
    pub v: u32,
    /// The contest, by [`ContestKind::name`].
    pub kind: String,
    /// Single caller or pile-up, by [`RunStyle::name`].
    pub style: String,
    /// The stations range the run used.
    pub callers: [u32; 2],
    /// Your side, `wve` or `dx`, or empty where sides do not apply.
    pub side: String,
    /// The facts about you the score uses, kept because the tables that
    /// derive them from your call change.
    pub my_country: String,
    pub my_continent: String,
    pub my_zone: u32,
    /// The fields copied from each caller, by [`Field::name`]: `["nr"]`.
    pub fields: Vec<String>,
    /// The run's length limit, in seconds.
    pub limit_s: u32,
    /// Run time, pauses left out.
    pub run_ms: u64,
    pub paused_ms: u64,
    /// The speed you sent at.
    pub my_wpm: f64,
    /// Show copy was on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub assisted: bool,
    pub qsos: Vec<QsoRecord>,
}

/// One QSO of a run: logged, lost, or not in the other station's log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QsoRecord {
    /// When it was logged or lost, in run time.
    pub t: u64,
    /// When the station was first heard, in run time.
    pub h: u64,
    /// How many of the session's groups it owns, in order after the QSOs
    /// before it.
    pub g: u32,
    /// Your serial; 0 in a contest without one.
    pub nr: u32,
    /// Empty when logged; `nil` or `lost` otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub k: String,
    /// The station's country, for multipliers.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub c: String,
    /// The exchange you logged, a token for each of the run's fields,
    /// where no group holds it: a NIL's, or a QSO's whose exchange never
    /// reached you, or whose station's copy an earlier QSO holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub l: Vec<String>,
    /// The call you logged, where its call group does not hold it: the
    /// group keeps the call as first sent with your exchange, and the log
    /// the call corrected since. Empty when they are the same.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lc: String,
    /// What the station sent of its exchange, beside `l`, where no group
    /// holds it, so the check can be worked out again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub s: Vec<String>,
    /// The station's call, where no group of this QSO holds it: a station
    /// logged again, whose copy an earlier QSO holds. `lc` holds the call
    /// logged.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sc: String,
}

impl ContestLog {
    /// The run, when this build can read its log: the layout is
    /// [`LAYOUT`], the contest is one that has shipped, the style and every
    /// field are known, and its QSOs own exactly the session's `groups`.
    /// Otherwise none, and the log stays as it was read.
    pub fn run(&self, groups: usize) -> Option<ContestRun> {
        let run = ContestRun::deserialize(&self.0).ok()?;
        let known = run.v == LAYOUT
            && ContestKind::from_name(&run.kind).is_some_and(|kind| kind.rules().is_some())
            && RunStyle::from_name(&run.style).is_some()
            && run
                .fields
                .iter()
                .all(|name| Field::from_name(name).is_some());
        let owned: u64 = run.qsos.iter().map(|qso| u64::from(qso.g)).sum();
        (known && owned == groups as u64).then_some(run)
    }
}

impl ContestRun {
    /// The session a run is kept as: `groups` are its QSOs' groups in order,
    /// each with its timing, `settings` those it ran with. Scored, tallied
    /// and timed as a group session is, its speeds the callers' and its
    /// level the callsign tier, with its log beside the groups.
    pub(crate) fn result(
        &self,
        groups: Vec<(GroupResult, SessionTiming)>,
        settings: &TrainingSettings,
        finished_at: u64,
        date: String,
    ) -> SessionResult {
        let (groups, timings): (Vec<GroupResult>, Vec<SessionTiming>) = groups.into_iter().unzip();
        let mut tally = CopyTally::default();
        for group in &groups {
            tally.add(&group.sent, &group.received);
        }
        let sent: Vec<&str> = groups.iter().map(|group| group.sent.as_str()).collect();
        let per_char: Vec<f64> = groups
            .iter()
            .zip(&timings)
            .map(|(group, timing)| {
                timing.time_to_complete_ms / group.sent.chars().count().max(1) as f64
            })
            .collect();
        let mean = |speeds: Vec<f64>| {
            (!speeds.is_empty()).then(|| speeds.iter().sum::<f64>() / speeds.len() as f64)
        };
        let char_wpm = mean(timings.iter().filter_map(|t| t.char_wpm).collect());
        let effective_wpm = mean(timings.iter().filter_map(|t| t.effective_wpm).collect());
        let mut result = SessionResult {
            date,
            timestamp: finished_at,
            accuracy: tally.accuracy(),
            tally,
            avg_response_ms: compute_average_response_ms(&per_char),
            total_chars: calculate_total_chars(&sent),
            score: 0.0,
            level: settings.level(LevelAxis::Callsign),
            digits_level: settings.level(LevelAxis::Digits),
            char_set_mode: CharSetMode::Contest,
            char_wpm: char_wpm.unwrap_or(self.my_wpm),
            effective_wpm: effective_wpm.unwrap_or(self.my_wpm),
            alphabet_fingerprint: format!("{CONTEST_MARK}{}", self.kind),
            contest: Some(ContestLog::from(self)),
            groups: groups
                .into_iter()
                .map(|group| GroupResult {
                    correct: group.sent == group.received,
                    ..group
                })
                .collect(),
            group_timings: timings,
        };
        result.score = result.points();
        result
    }

    /// The run's contest score, worked out again from its log and its
    /// session's `groups`, as it was while it ran: every logged QSO checked
    /// against what its station sent, which its groups hold. None for a
    /// contest without a score, or groups that are not this run's.
    pub fn score(&self, groups: &[GroupResult]) -> Option<ContestScore> {
        let rules = ContestKind::from_name(&self.kind)?.rules()?;
        let scoring = rules.scoring?;
        let logged = self.checks(groups)?;
        let scored: Vec<ScoredQso<'_>> = logged
            .iter()
            .map(|(at_ms, call, check)| ScoredQso {
                at_ms: *at_ms,
                call,
                check,
            })
            .collect();
        Some(contest_score(&scoring, &scored, self.run_ms))
    }

    /// How many of the run's QSOs were logged clean, a dupe copied right
    /// among them; none for groups that are not this run's.
    pub fn clean_qsos(&self, groups: &[GroupResult]) -> Option<usize> {
        let logged = self.checks(groups)?;
        Some(
            logged
                .iter()
                .filter(|(_, _, check)| check.is_clean())
                .count(),
        )
    }

    /// Each logged QSO, checked as it was while the run went on against
    /// what its station sent, which its groups or its record hold: when it
    /// was logged, the call as logged, and the check.
    fn checks(&self, groups: &[GroupResult]) -> Option<Vec<(u64, String, QsoCheck)>> {
        let fields: Vec<Field> = self
            .fields
            .iter()
            .map(|name| Field::from_name(name))
            .collect::<Option<_>>()?;
        let mut logged = Vec::new();
        let mut clean: Vec<&str> = Vec::new();
        let mut at = 0;
        for qso in &self.qsos {
            let owned = groups.get(at..at + qso.g as usize)?;
            at += qso.g as usize;
            if qso.k == LOST {
                continue;
            }
            // A logged QSO's call group comes first, unless an earlier QSO
            // holds the station's copy; its field groups follow, unless the
            // record holds the exchange.
            let call_group = owned.first().filter(|_| qso.sc.is_empty());
            let call = if qso.lc.is_empty() {
                call_group?.received.clone()
            } else {
                qso.lc.clone()
            };
            let check = match qso.k.as_str() {
                NIL => check_qso(
                    &QsoText {
                        call: call.clone(),
                        fields: Vec::new(),
                    },
                    None,
                    |_| false,
                ),
                _ => {
                    let station: &str = match call_group {
                        Some(group) => &group.sent,
                        None if !qso.sc.is_empty() => &qso.sc,
                        None => return None,
                    };
                    let field_groups = &owned[usize::from(call_group.is_some())..];
                    let tokens = |held: &[String]| -> Vec<(Field, String)> {
                        fields
                            .iter()
                            .zip(held)
                            .filter(|(_, token)| !token.is_empty())
                            .map(|(field, token)| (*field, token.clone()))
                            .collect()
                    };
                    let copied = |held: fn(&GroupResult) -> &String| {
                        fields
                            .iter()
                            .zip(field_groups)
                            .map(|(field, group)| (*field, held(group).clone()))
                            .collect()
                    };
                    let (typed, sent) = if qso.s.is_empty() {
                        (copied(|group| &group.received), copied(|group| &group.sent))
                    } else {
                        (tokens(&qso.l), tokens(&qso.s))
                    };
                    let sent = QsoText {
                        call: station.to_string(),
                        fields: sent,
                    };
                    let check = check_qso(
                        &QsoText {
                            call: call.clone(),
                            fields: typed,
                        },
                        Some(&sent),
                        |call| clean.contains(&call),
                    );
                    if check.is_clean() {
                        clean.push(station);
                    }
                    check
                }
            };
            logged.push((qso.t, call, check));
        }
        Some(logged)
    }
}

impl From<&ContestRun> for ContestLog {
    /// The log a run is written as. Plain data always makes a value, so the
    /// fallback is never reached.
    fn from(run: &ContestRun) -> Self {
        Self(serde_json::to_value(run).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::Tally;
    use serde_json::json;

    fn qso(t: u64, g: u32, nr: u32) -> QsoRecord {
        QsoRecord {
            t,
            h: t.saturating_sub(4_000),
            g,
            nr,
            k: String::new(),
            c: String::new(),
            l: Vec::new(),
            lc: String::new(),
            s: Vec::new(),
            sc: String::new(),
        }
    }

    /// A short WPX run: two QSOs logged, of two groups each, and a lost call.
    fn run() -> ContestRun {
        ContestRun {
            v: LAYOUT,
            kind: "wpx".into(),
            style: "single".into(),
            callers: [1, 1],
            side: String::new(),
            my_country: "DL".into(),
            my_continent: "EU".into(),
            my_zone: 14,
            fields: vec!["nr".into()],
            limit_s: 600,
            run_ms: 600_000,
            paused_ms: 12_000,
            my_wpm: 25.5,
            assisted: false,
            qsos: vec![
                qso(41_000, 2, 1),
                qso(80_000, 2, 2),
                QsoRecord {
                    k: "lost".into(),
                    ..qso(120_000, 1, 3)
                },
            ],
        }
    }

    #[test]
    fn a_run_written_by_this_build_reads_back() {
        let written = ContestLog::from(&run());
        assert_eq!(written.run(5), Some(run()));
        // Only what is there is written: no empty kinds, countries or
        // exchanges, and no assisted flag when Show copy was off.
        let value = &written.0;
        assert_eq!(value["v"], 1);
        assert_eq!(value["myCountry"], "DL");
        assert_eq!(value["limitS"], 600);
        assert!(value.get("assisted").is_none());
        assert!(value["qsos"][0].get("k").is_none());
        assert!(value["qsos"][0].get("l").is_none());
        assert_eq!(value["qsos"][2]["k"], "lost");
    }

    /// A log this build cannot read is not read, and stays as it came.
    #[test]
    fn a_log_this_build_cannot_read_is_left_as_it_was() {
        let written = ContestLog::from(&run()).0;
        let changed = |key: &str, value: serde_json::Value| {
            let mut log = written.clone();
            log[key] = value;
            ContestLog(log)
        };
        let unreadable = [
            changed("v", json!(2)),
            changed("v", json!(0)),
            // A kind this build has never heard of, and one whose phase has
            // not shipped here.
            changed("kind", json!("cwt")),
            changed("kind", json!("sprint")),
            changed("style", json!("swarm")),
            changed("fields", json!(["nr", "cwtNumber"])),
            changed("myWpm", json!(null)),
            ContestLog(json!({})),
            ContestLog(json!([1, 2])),
            ContestLog(json!({ "v": 2, "kindOfContest": "wpx", "q": [] })),
        ];
        for log in unreadable {
            let before = log.clone();
            assert_eq!(log.run(5), None, "{}", log.0);
            assert_eq!(log, before);
            assert_eq!(serde_json::to_value(&log).expect("serialize"), before.0);
        }
        // A key missing is a layout this build does not know.
        let mut missing = written.clone();
        missing.as_object_mut().expect("an object").remove("runMs");
        assert_eq!(ContestLog(missing).run(5), None);
        let mut missing_qso_key = written.clone();
        missing_qso_key["qsos"][0]
            .as_object_mut()
            .expect("an object")
            .remove("g");
        assert_eq!(ContestLog(missing_qso_key).run(5), None);
    }

    /// The QSOs own every group of the session, or the log is not this
    /// session's.
    #[test]
    fn the_groups_must_add_up() {
        let log = ContestLog::from(&run());
        assert!(log.run(5).is_some());
        assert_eq!(log.run(4), None);
        assert_eq!(log.run(6), None);
        assert_eq!(log.run(0), None);
    }

    /// A key a later phase adds with a default does not stop this build
    /// reading the rest.
    #[test]
    fn a_key_added_later_is_passed_over() {
        let mut log = ContestLog::from(&run()).0;
        log["laterKey"] = json!([1, 2.5, "x"]);
        log["qsos"][0]["z"] = json!(3);
        assert_eq!(ContestLog(log).run(5), Some(run()));
    }

    /// The call logged is written only where the call group does not hold
    /// it, and a log written before the key reads as if it held it.
    #[test]
    fn a_logged_call_is_kept_only_where_it_differs() {
        let mut corrected = run();
        corrected.qsos[1].lc = "K5XYZ".into();
        let log = ContestLog::from(&corrected);
        assert_eq!(log.0["qsos"][1]["lc"], "K5XYZ");
        assert!(log.0["qsos"][0].get("lc").is_none());
        assert_eq!(log.run(5), Some(corrected));
        assert_eq!(ContestLog::from(&run()).run(5), Some(run()));
    }

    /// A run's groups, as its session keeps them.
    fn group(sent: &str, received: &str) -> GroupResult {
        GroupResult {
            sent: sent.into(),
            received: received.into(),
            correct: sent == received,
        }
    }

    /// A short log worked by hand: a clean QSO, one whose call the station
    /// corrected, one with a wrong number, a NIL, a lost call and a dupe.
    #[test]
    fn a_stored_run_scores_as_its_log_says() {
        let mut stored = run();
        stored.qsos = vec![
            qso(30_000, 2, 1),
            QsoRecord {
                lc: "DL2XYZ".into(),
                ..qso(60_000, 2, 2)
            },
            qso(90_000, 2, 3),
            QsoRecord {
                k: NIL.into(),
                l: vec!["12".into()],
                ..qso(120_000, 1, 4)
            },
            QsoRecord {
                k: LOST.into(),
                ..qso(150_000, 1, 5)
            },
            qso(180_000, 2, 5),
        ];
        let groups = [
            group("K5XYZ", "K5XYZ"),
            group("214", "214"),
            // First sent as DL2XYW, then put right: the log is clean.
            group("DL2XYZ", "DL2XYW"),
            group("TT7", "7"),
            group("9A1AA", "9A1AA"),
            group("13", "31"),
            group("W1AW", "W1AQ"),
            group("G4ABC", "G4AB"),
            group("K5XYZ", "K5XYZ"),
            group("215", "215"),
        ];
        assert!(ContestLog::from(&stored).run(groups.len()).is_some());
        let score = stored.score(&groups).expect("WPX scores");
        // Raw: K5XYZ, DL2XYZ, 9A1AA and the NIL's W1AQ (the dupe dropped):
        // prefixes K5, DL2, 9A1 and W1.
        assert_eq!(
            score.raw,
            Tally {
                qso_points: 4,
                mults: 4,
                score: 16
            }
        );
        // Verified: K5XYZ and DL2XYZ; the dupe of K5XYZ scores nothing.
        assert_eq!(
            score.verified,
            Tally {
                qso_points: 2,
                mults: 2,
                score: 4
            }
        );
        // Groups that are not this run's are not scored.
        assert_eq!(stored.score(&groups[1..]), None);
    }

    /// Where no group holds what a QSO logged or what its station sent,
    /// the record does, and the check is worked out from that: a QSO
    /// logged before the exchange came, and a station logged again.
    #[test]
    fn a_qso_the_record_holds_scores_as_it_ran() {
        let mut stored = run();
        stored.qsos = vec![
            qso(30_000, 2, 1),
            // Logged before its exchange came: a number guessed.
            QsoRecord {
                l: vec!["12".into()],
                s: vec!["9".into()],
                ..qso(60_000, 1, 2)
            },
            // Logged before its exchange came, and guessed right.
            QsoRecord {
                l: vec!["7".into()],
                s: vec!["TT7".into()],
                ..qso(70_000, 1, 3)
            },
            // K5XYZ logged again: a dupe, whose copy QSO 1 holds.
            QsoRecord {
                lc: "K5XYZ".into(),
                sc: "K5XYZ".into(),
                l: vec!["214".into()],
                s: vec!["214".into()],
                ..qso(90_000, 0, 4)
            },
        ];
        let groups = [
            group("K5XYZ", "K5XYZ"),
            group("214", "214"),
            group("DL2XYZ", "DL2XYZ"),
            group("9A1AA", "9A1AA"),
        ];
        let log = ContestLog::from(&stored);
        assert_eq!(log.0["qsos"][3]["sc"], "K5XYZ");
        assert!(log.0["qsos"][0].get("s").is_none());
        assert_eq!(log.run(groups.len()), Some(stored.clone()));
        let score = stored.score(&groups).expect("WPX scores");
        // Raw: K5XYZ, DL2XYZ and 9A1AA, the call typed twice dropped.
        assert_eq!(score.raw.qso_points, 3);
        // Verified: K5XYZ and 9A1AA; DL2XYZ's number was wrong, and the
        // dupe scores nothing.
        assert_eq!(
            score.verified,
            Tally {
                qso_points: 2,
                mults: 2,
                score: 4
            }
        );
        assert_eq!(stored.clean_qsos(&groups), Some(3), "the dupe is clean");
    }

    /// The session a run is kept as: its groups scored and tallied as a
    /// group session's are, marked as a run, its log beside them.
    #[test]
    fn a_run_is_kept_as_a_session_of_its_groups() {
        let settings = TrainingSettings::default();
        let timing = |sends| SessionTiming {
            time_to_complete_ms: 900.0,
            char_wpm: Some(24.0),
            effective_wpm: Some(22.0),
            sends,
        };
        let groups = vec![
            (group("K5XYZ", "K5XYZ"), timing(1)),
            (group("214", "215"), timing(2)),
            (group("DL2XYZ", "DL2XYZ"), timing(1)),
            (group("TT7", "TT7"), timing(1)),
            (group("W1AW", ""), timing(1)),
        ];
        let result = run().result(groups, &settings, 9_000, "2026-10-20".into());
        assert_eq!(result.char_set_mode, CharSetMode::Contest);
        assert_eq!(result.alphabet_fingerprint, "contest:wpx");
        assert_eq!(result.level, settings.level(LevelAxis::Callsign));
        assert_eq!((result.char_wpm, result.effective_wpm), (24.0, 22.0));
        assert_eq!(result.groups.len(), 5);
        assert!(!result.groups[1].correct && !result.groups[4].correct);
        assert_eq!(result.score, result.points());
        assert!(result.score > 0.0);
        assert!(result.accuracy > 0.5 && result.accuracy < 1.0);
        assert_eq!(result.contest.and_then(|log| log.run(5)), Some(run()));
    }

    #[test]
    fn an_assisted_run_says_so() {
        let assisted = ContestRun {
            assisted: true,
            ..run()
        };
        let log = ContestLog::from(&assisted);
        assert_eq!(log.0["assisted"], true);
        assert_eq!(log.run(5), Some(assisted));
    }
}
