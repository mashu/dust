//! A contest run as the history keeps it.
//!
//! A run is a [`crate::SessionResult`] like any other: each call and
//! exchange field copied is one of its groups, so every stat counts it. Its
//! log rides beside the groups, and is written so that no build loses the run.
//! A build that knows no contests reads it as a callsign session; one that
//! knows an older log layout keeps the newer log untouched.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::check::{QsoCheck, QsoError, QsoText, check_qso};
use super::exchange::Field;
use super::score::{ContestScore, Rates, ScoredQso, contest_score};
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
    /// The exchange you logged, a token for each of the run's fields: a
    /// NIL's always, and otherwise where no group holds it: a QSO's whose
    /// exchange never reached you (its field groups are misses), or whose
    /// station's copy an earlier QSO holds.
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
    /// How many repeats you asked of the station and heard, as the live
    /// log counts them. Its groups' sends count only the repeats charged to
    /// the copy, which leave out one asked while the field already read
    /// right.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub a: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl QsoRecord {
    /// Whether the first of the QSO's groups is the station's call. It is,
    /// unless the QSO owns none or an earlier QSO holds the station's copy;
    /// its field groups follow.
    fn owns_call_group(&self) -> bool {
        self.g > 0 && self.sc.is_empty()
    }
}

/// One QSO of a stored run, as its results list it: what you logged, and
/// beside it what the station really sent.
#[derive(Clone, Debug, PartialEq)]
pub struct RunQso {
    /// Its number in the log, from 1; none for a QSO never logged.
    pub n: Option<u32>,
    /// When it was logged or lost, in run time.
    pub at_ms: u64,
    /// The call as you logged it, or for a QSO never logged, as you copied
    /// it.
    pub call: String,
    /// The station's call; empty for a NIL logged with nobody heard.
    pub station: String,
    /// Each field: what you logged, and what the station sent, empty where
    /// it sent nothing.
    pub fields: Vec<(Field, String, String)>,
    /// The station's speed.
    pub wpm: Option<f64>,
    /// How many repeats you asked for, as the live log counted them.
    pub asked: u32,
    /// How it checked; none for a QSO never logged.
    pub check: Option<QsoCheck>,
}

/// What a run's checks found, counted, for the line under its score.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunErrors {
    pub busted_calls: u32,
    /// Each field the run copies, with how many were wrong.
    pub wrong: Vec<(Field, u32)>,
    pub nils: u32,
    /// Stations logged again, which are no error.
    pub dupes: u32,
}

/// A stored run in figures, as its History row and its results give them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RunSummary {
    /// QSOs logged, dupes and NILs among them.
    pub qsos: usize,
    /// Those logged clean, a dupe copied right among them.
    pub clean: usize,
    /// QSOs an hour over the whole run.
    pub rate: Option<f64>,
    /// QSOs logged that you asked a repeat for.
    pub repeated: usize,
    /// Stations worked and never logged.
    pub lost: usize,
}

impl RunSummary {
    /// Minutes a QSO over the whole run, the pace a contest without a score
    /// is told by; none before a QSO is logged.
    pub fn minutes_per_qso(&self) -> Option<f64> {
        self.rate.filter(|rate| *rate > 0.0).map(|rate| 60.0 / rate)
    }
}

/// What became of a contest run's log, as the history says it.
#[derive(Clone, Debug, PartialEq)]
pub enum StoredLog {
    /// It reads.
    Read(RunSummary),
    /// An older version dropped it: the run's mark alone is left.
    Dropped,
    /// It is one this version cannot read: a later layout, or a contest
    /// that has not shipped here.
    Unreadable,
}

impl SessionResult {
    /// The contest a run was of, by its mark: `wpx` of `contest:wpx`,
    /// known to this build or not. None for any other session.
    pub fn contest_name(&self) -> Option<&str> {
        if self.char_set_mode != CharSetMode::Contest {
            return None;
        }
        let marked = self.alphabet_fingerprint.strip_prefix(CONTEST_MARK)?;
        marked.split(':').next()
    }

    /// The contest a run was of, when this build knows it.
    pub fn contest_kind(&self) -> Option<ContestKind> {
        self.contest_name().and_then(ContestKind::from_name)
    }

    /// A contest run's log, when this build can read it.
    pub fn contest_run(&self) -> Option<ContestRun> {
        self.contest.as_ref()?.run(self.groups.len())
    }

    /// What became of a contest run's log; none for any other session.
    pub fn stored_log(&self) -> Option<StoredLog> {
        if self.char_set_mode != CharSetMode::Contest {
            return None;
        }
        let Some(log) = &self.contest else {
            return Some(StoredLog::Dropped);
        };
        let summary = log
            .run(self.groups.len())
            .and_then(|run| run.summary(&self.groups));
        Some(summary.map_or(StoredLog::Unreadable, StoredLog::Read))
    }
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
        Some(self.summary(groups)?.clean)
    }

    /// The run in figures, its QSOs checked against its session's
    /// `groups`; none for groups that are not this run's.
    pub fn summary(&self, groups: &[GroupResult]) -> Option<RunSummary> {
        let walked = self.walk(groups)?;
        let logged: Vec<(&QsoRecord, &QsoCheck)> = walked
            .iter()
            .filter_map(|walked| Some((walked.qso, &walked.logged.as_ref()?.1)))
            .collect();
        let times: Vec<u64> = logged.iter().map(|(qso, _)| qso.t).collect();
        Some(RunSummary {
            qsos: logged.len(),
            clean: logged.iter().filter(|(_, check)| check.is_clean()).count(),
            rate: Rates::of(&times, self.run_ms).whole_run,
            repeated: logged.iter().filter(|(qso, _)| qso.a > 0).count(),
            lost: walked.len() - logged.len(),
        })
    }

    /// Every QSO of the run, logged or not, as its results list it, with
    /// what its station really sent beside what you logged: its session's
    /// `groups` hold both, and their `timings` the stations' speeds. None
    /// for groups that are not this run's.
    pub fn log(&self, groups: &[GroupResult], timings: &[SessionTiming]) -> Option<Vec<RunQso>> {
        let fields = self.copied()?;
        let mut logged = 0;
        let mut out = Vec::new();
        for walked in self.walk(groups)? {
            let Walked {
                qso,
                call_group,
                field_groups,
                ..
            } = walked;
            let token = |held: &[String], i: usize, group: fn(&GroupResult) -> &String| {
                held.get(i)
                    .or_else(|| field_groups.get(i).map(group))
                    .cloned()
                    .unwrap_or_default()
            };
            let (n, call, check) = match walked.logged {
                Some((call, check)) => {
                    logged += 1;
                    (Some(logged), call, Some(check))
                }
                None => {
                    let call = call_group.map(|group| group.received.clone());
                    (None, call.unwrap_or_default(), None)
                }
            };
            out.push(RunQso {
                n,
                at_ms: qso.t,
                call,
                station: call_group.map_or_else(|| qso.sc.clone(), |group| group.sent.clone()),
                fields: fields
                    .iter()
                    .enumerate()
                    .map(|(i, field)| {
                        let typed = token(&qso.l, i, |group| &group.received);
                        let sent = token(&qso.s, i, |group| &group.sent);
                        (*field, typed, sent)
                    })
                    .collect(),
                wpm: timings
                    .get(walked.groups)
                    .and_then(<[SessionTiming]>::first)
                    .and_then(|timing| timing.char_wpm),
                asked: qso.a,
                check,
            });
        }
        Some(out)
    }

    /// How well each part of a QSO was copied over the run: the calls,
    /// none, then each field it copies, as the share of their characters
    /// copied right, lost copies included. A part with no group is left
    /// out. None for groups that are not this run's.
    pub fn accuracy_by_field(&self, groups: &[GroupResult]) -> Option<Vec<(Option<Field>, f64)>> {
        let copies = self.group_copies();
        if copies.len() != groups.len() {
            return None;
        }
        let parts = std::iter::once(None).chain(self.copied()?.into_iter().map(Some));
        Some(
            parts
                .filter_map(|part| {
                    let mut tally = CopyTally::default();
                    let mut any = false;
                    for (copy, group) in copies.iter().zip(groups) {
                        let held = match copy {
                            GroupCopy::Call => None,
                            GroupCopy::Field(field) => Some(*field),
                            GroupCopy::Other => continue,
                        };
                        if held == part {
                            tally.add(&group.sent, &group.received);
                            any = true;
                        }
                    }
                    any.then(|| (part, tally.accuracy()))
                })
                .collect(),
        )
    }

    /// What the checks of the run's logged QSOs found, counted; none for
    /// groups that are not this run's.
    pub fn errors(&self, groups: &[GroupResult]) -> Option<RunErrors> {
        let mut errors = RunErrors {
            wrong: self.copied()?.into_iter().map(|field| (field, 0)).collect(),
            ..RunErrors::default()
        };
        for (_, _, check) in self.checks(groups)? {
            errors.dupes += u32::from(check.dupe);
            for error in &check.errors {
                match error {
                    QsoError::Nil => errors.nils += 1,
                    QsoError::Call { .. } => errors.busted_calls += 1,
                    QsoError::Field(field) => {
                        if let Some((_, count)) =
                            errors.wrong.iter_mut().find(|(held, _)| held == field)
                        {
                            *count += 1;
                        }
                    }
                }
            }
        }
        Some(errors)
    }

    /// Each logged QSO, checked as it was while the run went on: when it
    /// was logged, the call as logged, and the check.
    fn checks(&self, groups: &[GroupResult]) -> Option<Vec<(u64, String, QsoCheck)>> {
        let walked = self.walk(groups)?;
        Some(
            walked
                .into_iter()
                .filter_map(|walked| {
                    let (call, check) = walked.logged?;
                    Some((walked.qso.t, call, check))
                })
                .collect(),
        )
    }

    /// What each of the run's groups holds, in order, as its QSOs own them
    /// ([`QsoRecord::owns_call_group`]). A run read by [`ContestLog::run`]
    /// knows every field and owns exactly its session's groups, so this is
    /// one for each of them.
    pub(crate) fn group_copies(&self) -> Vec<GroupCopy> {
        let mut copies = Vec::new();
        for qso in &self.qsos {
            let call = qso.owns_call_group();
            if call {
                copies.push(GroupCopy::Call);
            }
            // A group past the fields holds nothing a field names.
            let fields = qso.g as usize - usize::from(call);
            copies.extend((0..fields).map(|at| {
                self.fields
                    .get(at)
                    .and_then(|name| Field::from_name(name))
                    .map_or(GroupCopy::Other, GroupCopy::Field)
            }));
        }
        copies
    }

    /// The fields copied from each caller; none if this build does not
    /// know one.
    fn copied(&self) -> Option<Vec<Field>> {
        self.fields
            .iter()
            .map(|name| Field::from_name(name))
            .collect()
    }

    /// Each QSO of the run with the groups it owns among `groups`, each
    /// logged one checked as it was while the run went on against what its
    /// station sent, which its groups or its record hold. None for groups
    /// that are not this run's.
    fn walk<'a>(&'a self, groups: &'a [GroupResult]) -> Option<Vec<Walked<'a>>> {
        let fields = self.copied()?;
        let mut walked = Vec::new();
        let mut clean: Vec<&str> = Vec::new();
        let mut at = 0;
        for qso in &self.qsos {
            let range = at..at + qso.g as usize;
            at = range.end;
            let owned = groups.get(range.clone())?;
            // Its field groups follow its call group, unless the record
            // holds the exchange.
            let call_group = owned.first().filter(|_| qso.owns_call_group());
            let field_groups = &owned[usize::from(call_group.is_some())..];
            let logged = if qso.k == LOST {
                None
            } else {
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
                Some((call, check))
            };
            walked.push(Walked {
                qso,
                groups: range,
                call_group,
                field_groups,
                logged,
            });
        }
        Some(walked)
    }
}

/// What one of a run's groups holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupCopy {
    /// A station's call.
    Call,
    /// One field of its exchange.
    Field(Field),
    /// Nothing the run's layout names.
    Other,
}

/// A QSO of a stored run, with the groups it owns.
struct Walked<'a> {
    qso: &'a QsoRecord,
    /// Where its groups sit among the session's.
    groups: Range<usize>,
    /// Its call group, unless an earlier QSO holds the station's copy.
    call_group: Option<&'a GroupResult>,
    /// Its field groups, unless the record holds the exchange.
    field_groups: &'a [GroupResult],
    /// Logged: the call as logged, and how it checked. None for a QSO
    /// never logged.
    logged: Option<(String, QsoCheck)>,
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
            a: 0,
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
        assert!(value["qsos"][0].get("a").is_none());
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

    /// A Simple QSO run reads back as one, with no contest score: its pace
    /// is minutes a QSO, and how well it went each part's accuracy.
    #[test]
    fn a_simple_qso_run_reads_back_with_no_score() {
        let stored = ContestRun {
            kind: "simpleQso".into(),
            fields: vec!["rst".into(), "name".into()],
            qsos: vec![
                qso(60_000, 3, 0),
                qso(150_000, 3, 0),
                QsoRecord {
                    k: LOST.into(),
                    ..qso(200_000, 3, 0)
                },
            ],
            ..run()
        };
        let groups = [
            group("K5XYZ", "K5XYZ"),
            group("579", "579"),
            group("BOB", "BOB"),
            group("DL2XYZ", "DL2XYZ"),
            group("599", "599"),
            group("HANS", "HANZ"),
            group("W1AW", "W1AW"),
            group("559", ""),
            group("JIM", ""),
        ];
        let read = ContestLog::from(&stored).run(groups.len());
        assert_eq!(read.as_ref(), Some(&stored));
        assert_eq!(
            stored.score(&groups),
            None,
            "nothing scores an everyday QSO"
        );
        let summary = stored.summary(&groups).expect("its figures");
        assert_eq!((summary.qsos, summary.clean, summary.lost), (2, 1, 1));
        // Two QSOs in ten minutes: five minutes each.
        assert_eq!(summary.minutes_per_qso(), Some(5.0));
        assert_eq!(RunSummary::default().minutes_per_qso(), None);
        let errors = stored.errors(&groups).expect("its checks");
        assert_eq!(errors.wrong, [(Field::Rst, 0), (Field::Name, 1)]);
        let accuracy = stored.accuracy_by_field(&groups).expect("each part");
        assert_eq!(
            accuracy,
            [
                (None, 1.0),
                (Some(Field::Rst), 6.0 / 9.0),
                (Some(Field::Name), 6.0 / 10.0),
            ]
        );
        assert_eq!(stored.accuracy_by_field(&groups[1..]), None);
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

    /// The results list every QSO with what its station sent beside what
    /// you logged, its speed and its repeats, and count what the checks
    /// found. A QSO never logged has no number and no check.
    #[test]
    fn a_stored_run_lists_its_qsos_with_what_was_really_sent() {
        let mut stored = run();
        stored.qsos = vec![
            qso(30_000, 2, 1),
            QsoRecord {
                lc: "DL2XYZ".into(),
                a: 1,
                ..qso(60_000, 2, 2)
            },
            QsoRecord {
                k: NIL.into(),
                l: vec!["12".into()],
                lc: "W1AQ".into(),
                ..qso(90_000, 0, 3)
            },
            QsoRecord {
                k: LOST.into(),
                ..qso(120_000, 2, 3)
            },
            QsoRecord {
                lc: "K5XYZ".into(),
                sc: "K5XYZ".into(),
                l: vec!["215".into()],
                s: vec!["215".into()],
                ..qso(150_000, 0, 3)
            },
        ];
        let groups = [
            group("K5XYZ", "K5XYZ"),
            group("214", "214"),
            group("DL2XYZ", "DL2XYW"),
            group("13", "31"),
            group("G4ABC", "G4AB"),
            group("TT7", ""),
        ];
        let timing = |wpm, sends| SessionTiming {
            time_to_complete_ms: 900.0,
            char_wpm: Some(wpm),
            effective_wpm: Some(wpm),
            sends,
        };
        // Its sends count only the repeats charged to the copy: the record
        // holds those you asked for.
        let timings = [
            timing(24.0, 1),
            timing(24.0, 1),
            timing(26.0, 1),
            timing(26.0, 3),
            timing(28.0, 1),
            timing(28.0, 1),
        ];
        let log = stored.log(&groups, &timings).expect("the run's groups");
        let numbers: Vec<Option<u32>> = log.iter().map(|qso| qso.n).collect();
        assert_eq!(numbers, [Some(1), Some(2), Some(3), None, Some(4)]);
        assert_eq!(log[0].fields, [(Field::Nr, "214".into(), "214".into())]);
        assert_eq!(
            log[0].check.as_ref().map(QsoCheck::words).as_deref(),
            Some("clean")
        );
        // Corrected on the air, logged right: the log keeps the call put
        // right, and the number copied wrong.
        assert_eq!(
            (log[1].call.as_str(), log[1].station.as_str()),
            ("DL2XYZ", "DL2XYZ")
        );
        assert_eq!(log[1].fields, [(Field::Nr, "31".into(), "13".into())]);
        assert_eq!((log[1].wpm, log[1].asked), (Some(26.0), 1));
        // A NIL with nobody heard: no station, nothing sent.
        assert_eq!(
            (log[2].call.as_str(), log[2].station.as_str()),
            ("W1AQ", "")
        );
        assert_eq!(log[2].fields, [(Field::Nr, "12".into(), String::new())]);
        // Lost: the call as copied, the number the station sent.
        assert_eq!(
            (log[3].call.as_str(), log[3].station.as_str()),
            ("G4AB", "G4ABC")
        );
        assert_eq!(log[3].fields, [(Field::Nr, String::new(), "TT7".into())]);
        assert_eq!(log[3].check, None);
        // A dupe, whose copy QSO 1 holds.
        assert_eq!(log[4].station, "K5XYZ");
        assert_eq!(log[4].fields, [(Field::Nr, "215".into(), "215".into())]);
        assert!(log[4].check.as_ref().is_some_and(|check| check.dupe));
        assert_eq!(log[4].wpm, None);

        assert_eq!(
            stored.errors(&groups),
            Some(RunErrors {
                busted_calls: 0,
                wrong: vec![(Field::Nr, 1)],
                nils: 1,
                dupes: 1,
            })
        );
        assert_eq!(
            stored.summary(&groups),
            Some(RunSummary {
                qsos: 4,
                clean: 2,
                rate: Some(24.0),
                repeated: 1,
                lost: 1,
            })
        );
        assert_eq!(stored.log(&groups[1..], &timings), None);
        assert_eq!(stored.errors(&groups[1..]), None);
        assert_eq!(stored.summary(&groups[1..]), None);
    }

    /// A run is known by its mark, and the history says whether its log
    /// reads, was dropped by an older version, or is one this version
    /// cannot read.
    #[test]
    fn the_history_says_what_became_of_a_runs_log() {
        let settings = TrainingSettings::default();
        let timing = SessionTiming {
            time_to_complete_ms: 900.0,
            char_wpm: Some(24.0),
            effective_wpm: Some(24.0),
            sends: 1,
        };
        let groups = ["K5XYZ", "214", "DL2XYZ", "TT7", "W1AW"]
            .map(|sent| (group(sent, sent), timing.clone()))
            .to_vec();
        let kept = run().result(groups, &settings, 9_000, "2026-10-20".into());
        assert_eq!(kept.contest_kind(), Some(ContestKind::Wpx));
        assert_eq!(kept.contest_run(), Some(run()));
        assert_eq!(
            kept.stored_log(),
            Some(StoredLog::Read(RunSummary {
                qsos: 2,
                clean: 2,
                rate: Some(12.0),
                repeated: 0,
                lost: 1,
            }))
        );
        let dropped = SessionResult {
            contest: None,
            ..kept.clone()
        };
        assert_eq!(dropped.contest_kind(), Some(ContestKind::Wpx));
        assert_eq!(dropped.stored_log(), Some(StoredLog::Dropped));
        let mut later = kept.clone();
        if let Some(log) = later.contest.as_mut() {
            log.0["v"] = json!(2);
        }
        assert_eq!(later.stored_log(), Some(StoredLog::Unreadable));
        let unknown = SessionResult {
            alphabet_fingerprint: "contest:cwt".into(),
            ..dropped
        };
        assert_eq!(
            (unknown.contest_name(), unknown.contest_kind()),
            (Some("cwt"), None)
        );
        let assisted = SessionResult {
            alphabet_fingerprint: "contest:wpx:a".into(),
            ..kept
        };
        assert_eq!(assisted.contest_kind(), Some(ContestKind::Wpx));
        let koch = SessionResult::empty_for_test();
        assert_eq!((koch.contest_kind(), koch.stored_log()), (None, None));
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
