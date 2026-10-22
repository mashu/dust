//! A logged QSO checked against what the station really sent, as
//! MorseRunner checks a log: not in their log, a busted call, a dupe, and
//! each field copied wrong. Values are compared canonically, as the contest
//! reads them ([`ContestRules::same`]), so `7` logged for `TT7` is right.

use super::exchange::Field;
use super::parse::normalize_call;
use super::rules::ContestRules;

/// A QSO's call and exchange: as you logged it, or as the station sent it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QsoText {
    pub call: String,
    pub fields: Vec<(Field, String)>,
}

impl QsoText {
    /// The value or token held for `field`.
    pub fn value(&self, field: Field) -> Option<&str> {
        self.fields
            .iter()
            .find(|(held, _)| *held == field)
            .map(|(_, value)| value.as_str())
    }
}

/// One thing the check found wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QsoError {
    /// Not in their log: nobody matching was worked.
    Nil,
    /// A busted call: the station was `correct`.
    Call { correct: String },
    /// A field copied wrong.
    Field(Field),
}

/// What the check of one logged QSO found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QsoCheck {
    /// Everything wrong with it, in the order a log reads: the call, then
    /// each field. A NIL is only that.
    pub errors: Vec<QsoError>,
    /// The station was already logged clean: the QSO scores nothing, and is
    /// no error, as contest rules remove dupes without penalty.
    pub dupe: bool,
}

impl QsoCheck {
    /// Whether it was copied right: a dupe copied right is clean.
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }

    /// The check as the log says it: "clean", "busted call, was K5XYZ".
    pub fn words(&self) -> String {
        if self.is_clean() {
            return if self.dupe { "dupe" } else { "clean" }.to_string();
        }
        let words: Vec<String> = self
            .errors
            .iter()
            .map(|error| match error {
                QsoError::Nil => "not in their log".to_string(),
                QsoError::Call { correct } => format!("busted call, was {correct}"),
                QsoError::Field(field) => format!("wrong {}", field.label()),
            })
            .collect();
        words.join(", ")
    }
}

/// Check `logged` against `sent`, what the station worked really sent, or
/// against nobody when no station matched, as `rules` read the values.
/// `logged_clean` says whether a call is already in the log, copied right.
pub fn check_qso(
    rules: &ContestRules,
    logged: &QsoText,
    sent: Option<&QsoText>,
    logged_clean: impl Fn(&str) -> bool,
) -> QsoCheck {
    let Some(sent) = sent else {
        return QsoCheck {
            errors: vec![QsoError::Nil],
            dupe: false,
        };
    };
    let mut errors = Vec::new();
    if normalize_call(&logged.call) != normalize_call(&sent.call) {
        errors.push(QsoError::Call {
            correct: sent.call.clone(),
        });
    }
    for (field, value) in &sent.fields {
        if !logged
            .value(*field)
            .is_some_and(|logged| rules.same(*field, logged, value))
        {
            errors.push(QsoError::Field(*field));
        }
    }
    QsoCheck {
        errors,
        dupe: logged_clean(&sent.call),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::ContestKind;

    /// The checks of WPX, whose exchange is a number.
    fn check_qso(
        logged: &QsoText,
        sent: Option<&QsoText>,
        logged_clean: impl Fn(&str) -> bool,
    ) -> QsoCheck {
        let rules = ContestKind::Wpx.rules();
        super::check_qso(rules, logged, sent, logged_clean)
    }

    fn qso(call: &str, nr: &str) -> QsoText {
        QsoText {
            call: call.to_string(),
            fields: vec![(Field::Nr, nr.to_string())],
        }
    }

    fn nothing_logged(_: &str) -> bool {
        false
    }

    #[test]
    fn a_qso_copied_right_is_clean() {
        let check = check_qso(
            &qso("K5XYZ", "214"),
            Some(&qso("K5XYZ", "214")),
            nothing_logged,
        );
        assert!(check.is_clean());
        assert!(!check.dupe);
        // Canonical values: the logger's 7 for the TT7 that was sent.
        let cut = check_qso(
            &qso("K5XYZ", "7"),
            Some(&qso("K5XYZ", "TT7")),
            nothing_logged,
        );
        assert!(cut.is_clean());
    }

    /// The log says a check in words: clean, a dupe, or each thing wrong.
    #[test]
    fn a_check_reads_as_the_log_says_it() {
        assert_eq!(QsoCheck::default().words(), "clean");
        let dupe = QsoCheck {
            errors: Vec::new(),
            dupe: true,
        };
        assert_eq!(dupe.words(), "dupe");
        let both = check_qso(
            &qso("K5XYW", "13"),
            Some(&qso("K5XYZ", "31")),
            nothing_logged,
        );
        assert_eq!(both.words(), "busted call, was K5XYZ, wrong NR");
        let nil = check_qso(&qso("K5XYZ", "214"), None, |_| true);
        assert_eq!(nil.words(), "not in their log");
    }

    #[test]
    fn nobody_matching_is_a_nil() {
        let check = check_qso(&qso("K5XYZ", "214"), None, |_| true);
        assert_eq!(check.errors, [QsoError::Nil]);
        assert!(!check.is_clean());
        assert!(!check.dupe);
    }

    #[test]
    fn a_busted_call_names_the_right_one() {
        let check = check_qso(
            &qso("K5XYW", "214"),
            Some(&qso("K5XYZ", "214")),
            nothing_logged,
        );
        assert_eq!(
            check.errors,
            [QsoError::Call {
                correct: "K5XYZ".into()
            }]
        );
    }

    #[test]
    fn each_field_copied_wrong_is_named() {
        let check = check_qso(
            &qso("K5XYZ", "241"),
            Some(&qso("K5XYZ", "214")),
            nothing_logged,
        );
        assert_eq!(check.errors, [QsoError::Field(Field::Nr)]);
        // A busted call and a wrong number are two errors.
        let both = check_qso(
            &qso("K5XYW", "13"),
            Some(&qso("K5XYZ", "31")),
            nothing_logged,
        );
        assert_eq!(both.errors.len(), 2);
        // A field not logged at all is wrong too.
        let missing = QsoText {
            call: "K5XYZ".into(),
            fields: Vec::new(),
        };
        let check = check_qso(&missing, Some(&qso("K5XYZ", "214")), nothing_logged);
        assert_eq!(check.errors, [QsoError::Field(Field::Nr)]);
    }

    /// Labrador logged for Newfoundland and Labrador is right in the
    /// Sprint, which counts it as NL, and wrong in ARRL DX, which counts
    /// it apart.
    #[test]
    fn labrador_is_right_for_nl_only_where_it_is_part_of_it() {
        let qso = |st: &str| QsoText {
            call: "VO2AB".into(),
            fields: vec![(Field::St, st.to_string())],
        };
        let sprint = ContestKind::Sprint.rules();
        let arrl_dx = ContestKind::ArrlDx.rules();
        let check = |rules, logged, sent| {
            super::check_qso(rules, &qso(logged), Some(&qso(sent)), nothing_logged)
        };
        assert!(check(sprint, "LB", "NL").is_clean());
        assert_eq!(
            check(arrl_dx, "NL", "LB").errors,
            [QsoError::Field(Field::St)]
        );
        assert!(check(arrl_dx, "LB", "LB").is_clean());
    }

    /// A station already logged clean is a dupe, and a dupe copied right is
    /// clean: no error.
    #[test]
    fn a_station_already_logged_clean_is_a_dupe_and_no_error() {
        let worked = |call: &str| call == "K5XYZ";
        let check = check_qso(&qso("K5XYZ", "215"), Some(&qso("K5XYZ", "215")), worked);
        assert!(check.dupe);
        assert!(check.is_clean());
        let other = check_qso(&qso("W6XYZ", "9"), Some(&qso("W6XYZ", "9")), worked);
        assert!(!other.dupe);
    }
}
