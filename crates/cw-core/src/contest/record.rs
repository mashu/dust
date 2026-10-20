//! A contest run as the history keeps it.
//!
//! A run is a [`crate::SessionResult`] like any other: each call and
//! exchange field copied is one of its groups, so every stat counts it. Its
//! log rides beside the groups, and is written so that no build loses the run.
//! A build that knows no contests reads it as a callsign session; one that
//! knows an older log layout keeps the newer log untouched.

use serde::{Deserialize, Serialize};

use super::exchange::Field;
use super::{ContestKind, RunStyle};

/// What a run's alphabet fingerprint starts with: `contest:wpx`. A build
/// older than contest mode drops the log on its next save but keeps the
/// fingerprint, so the mark is what still makes the run a run after that.
pub(crate) const CONTEST_MARK: &str = "contest:";

/// The log layout this build writes and reads. It rises only for a change
/// an older reader would misread; a key added with a default does not
/// raise it.
const LAYOUT: u32 = 1;

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
    /// A NIL only: the exchange you logged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub l: Vec<String>,
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
