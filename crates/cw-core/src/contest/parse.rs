//! What you type, read: the call as the Call field holds it, and the
//! exchange as its fields.
//!
//! Typing is never corrected as you type: the text stays as typed, and is
//! read on every edit. The reading is what is checked and logged.

use super::exchange::Field;
use super::rules::{ContestRules, Side};

/// The Call field's text as a call: upper case, letters, digits, `/` and the
/// `?` that asks for a part, and nothing else, spaces included.
pub fn normalize_call(text: &str) -> String {
    text.chars()
        .map(|c| c.to_ascii_uppercase())
        .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '/' | '?'))
        .collect()
}

/// How much of an exchange the Exchange field holds: Enter Sends Message's
/// E.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExchangeState {
    /// Nothing typed.
    Empty,
    /// Some of the fields, not all.
    Missing,
    /// A token nothing can read.
    Invalid,
    /// Every field.
    Complete,
}

/// One field read: the token as typed, and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadField {
    pub field: Field,
    pub token: String,
    /// The canonical value ([`Field::canonical`]).
    pub value: String,
}

/// The Exchange field, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    /// The fields read, in the order the contest copies them.
    pub fields: Vec<ReadField>,
    /// Tokens a field could have taken but did not: one a later token of
    /// the same kind replaced. Shown greyed.
    pub unused: Vec<String>,
    /// Tokens nothing can read, which make the exchange Invalid: what
    /// "Reads as" shows as the fault.
    pub unreadable: Vec<String>,
    pub state: ExchangeState,
}

impl Reading {
    /// The value read for `field`.
    pub fn value(&self, field: Field) -> Option<&str> {
        self.fields
            .iter()
            .find(|read| read.field == field)
            .map(|read| read.value.as_str())
    }
}

/// The reports a caller sends and loggers prefill. Typed in an exchange
/// they are the report, not a number, unless they are all there is.
const REPORTS: [&str; 3] = ["5NN", "ENN", "599"];

/// How a field finds its token among what is typed: the three passes of the
/// deterministic parser, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    /// A closed list: a state, a section, a class, a power. It takes the
    /// last token on its list.
    List,
    /// A number: a serial, a zone, a report. It takes the last number left.
    Number,
    /// Free text: a name. It fills from what is left, right to left.
    Text,
}

impl Pass {
    const ALL: [Self; 3] = [Self::List, Self::Number, Self::Text];

    fn of(field: Field) -> Self {
        match field {
            Field::St | Field::Sec | Field::Cl | Field::Pwr => Self::List,
            Field::Nr | Field::Zn | Field::Rst => Self::Number,
            Field::Name => Self::Text,
        }
    }
}

/// The Exchange field's text, read for the fields `rules` copy on `side`.
///
/// Items may come in any order, and a later item of a kind replaces an
/// earlier one, so a correction typed last wins. One deterministic parser
/// reads every contest, in three passes: each closed-list field takes the
/// last token on its list (`TN` is Tennessee); each numeric field the last
/// number left, where a serial is not exactly a report (`5NN`, `ENN`,
/// `599`) in an exchange that holds one, unless nothing else is there, and
/// a copied report is the last token shaped as one (`57N` is 579); and the
/// free-text fields fill from what is left, right to left, a name being an
/// all-letter token that is not a report where one is copied. Where a name
/// is copied too, a serial or a zone is letters alone only when nothing
/// with a digit is there and another token is left for the name: `ANNE` is
/// a name, not 1995, and `TNN BOB` is 99 and BOB.
pub fn parse_exchange(rules: &ContestRules, side: Side, text: &str) -> Reading {
    let tokens: Vec<String> = text
        .split_whitespace()
        .map(str::to_ascii_uppercase)
        .collect();
    let copied = rules.copied(side);
    let others = Others {
        copied,
        report: rules.rst,
    };
    let mut taken = vec![false; tokens.len()];
    let mut read: Vec<(usize, ReadField)> = Vec::new();
    for pass in Pass::ALL {
        let fields = copied
            .iter()
            .enumerate()
            .filter(|(_, field)| Pass::of(**field) == pass);
        // Free text fills right to left: the last of its fields takes the
        // last token left.
        let fields: Vec<(usize, &Field)> = if pass == Pass::Text {
            fields.rev().collect()
        } else {
            fields.collect()
        };
        for (order, &field) in fields {
            if let Some((at, value)) = take(field, &tokens, &taken, &others) {
                taken[at] = true;
                read.push((
                    order,
                    ReadField {
                        field,
                        token: tokens[at].clone(),
                        value,
                    },
                ));
            }
        }
    }
    read.sort_by_key(|(order, _)| *order);
    let fields: Vec<ReadField> = read.into_iter().map(|(_, read)| read).collect();
    let mut unused = Vec::new();
    let mut unreadable = Vec::new();
    for (token, _) in tokens.iter().zip(&taken).filter(|(_, taken)| !**taken) {
        if rules.prefills_report(side) && REPORTS.contains(&token.as_str()) {
            // The report typed out of habit: read, and never checked.
            continue;
        }
        if copied.iter().any(|field| field.canonical(token).is_some()) {
            unused.push(token.clone());
        } else {
            unreadable.push(token.clone());
        }
    }
    let state = if tokens.is_empty() {
        ExchangeState::Empty
    } else if !unreadable.is_empty() {
        ExchangeState::Invalid
    } else if fields.len() == copied.len() {
        ExchangeState::Complete
    } else {
        ExchangeState::Missing
    };
    Reading {
        fields,
        unused,
        unreadable,
        state,
    }
}

/// What the rest of an exchange asks of a token, so each field reads only
/// what is its own.
struct Others {
    /// The fields copied.
    copied: &'static [Field],
    /// A report is part of the exchange, so `5NN` typed is it.
    report: bool,
}

impl Others {
    /// Whether one of the closed lists copied holds `token`, which then is
    /// never read as cut digits: `TN` is Tennessee, not 9.
    fn listed(&self, token: &str) -> bool {
        self.copied
            .iter()
            .any(|field| Pass::of(*field) == Pass::List && field.canonical(token).is_some())
    }

    fn copies(&self, field: Field) -> bool {
        self.copied.contains(&field)
    }

    /// Whether `token` can be the name: letters, and not a report where
    /// one is copied (`ENN`).
    fn names(&self, token: &str) -> bool {
        Field::Name.canonical(token).is_some()
            && !(self.copies(Field::Rst) && Field::Rst.canonical(token).is_some())
    }
}

/// The tokens not yet `taken` that `accept` takes, with where they are.
fn open<'a>(
    tokens: &'a [String],
    taken: &'a [bool],
    accept: &'a dyn Fn(&str) -> bool,
) -> impl DoubleEndedIterator<Item = (usize, &'a str)> {
    tokens
        .iter()
        .map(String::as_str)
        .enumerate()
        .filter(move |(at, token)| !taken[*at] && accept(token))
}

/// The token `field` takes of those not yet `taken`, and its value.
fn take(
    field: Field,
    tokens: &[String],
    taken: &[bool],
    others: &Others,
) -> Option<(usize, String)> {
    let read = |(at, token): (usize, &str)| Some((at, field.canonical(token)?));
    let last = |accept: &dyn Fn(&str) -> bool| open(tokens, taken, accept).rev().find_map(read);
    let first = |accept: &dyn Fn(&str) -> bool| open(tokens, taken, accept).find_map(read);
    match field {
        Field::Nr | Field::Zn => {
            let number = |token: &str| !others.listed(token);
            // A report reads as a number too, but is a serial only when it
            // is all there is.
            let not_report = |token: &str| !(others.report && REPORTS.contains(&token));
            let by = |accept: &dyn Fn(&str) -> bool| {
                last(&|token| number(token) && accept(token) && not_report(token))
                    .or_else(|| last(&|token| number(token) && accept(token)))
            };
            if !others.copies(Field::Name) {
                return by(&|_| true);
            }
            // Beside a name, a number has a digit in it; cut letters alone
            // are one only while another token is left for the name.
            let digits = |token: &str| token.chars().any(|c| c.is_ascii_digit());
            by(&digits).or_else(|| {
                let (at, value) = by(&|token| !digits(token))?;
                let named =
                    open(tokens, taken, &|token| others.names(token)).any(|(other, _)| other != at);
                named.then_some((at, value))
            })
        }
        // `ENN` is all letters, but a report where one is copied. A token a
        // list holds is a name only when nothing else is left, and then the
        // first of them, so a state put right leaves the name as it was.
        Field::Name => last(&|token| others.names(token) && !others.listed(token))
            .or_else(|| first(&|token| others.names(token))),
        _ => last(&|_| true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::ContestKind;
    use crate::rng::{Rng, SplitMix64};

    fn wpx(text: &str) -> Reading {
        let rules = ContestKind::Wpx.rules().expect("WPX has shipped");
        parse_exchange(rules, Side::Dx, text)
    }

    /// The WPX rows of the design's table.
    #[test]
    fn wpx_reads_as_the_table_says() {
        for (typed, nr) in [
            ("599", "599"),
            ("5NN 145", "145"),
            ("TT1", "1"),
            ("5NN TT1", "1"),
            ("145 5NN", "145"),
            ("ENN", "599"),
            ("5nn 1n5", "195"),
        ] {
            let reading = wpx(typed);
            assert_eq!(reading.value(Field::Nr), Some(nr), "{typed}");
            assert_eq!(reading.state, ExchangeState::Complete, "{typed}");
            assert!(reading.unreadable.is_empty(), "{typed}");
        }
    }

    fn simple_qso(text: &str) -> Reading {
        let rules = ContestKind::SimpleQso
            .rules()
            .expect("Simple QSO has shipped");
        parse_exchange(rules, Side::Dx, text)
    }

    /// The Simple QSO rows of the design's table, and more like them: the
    /// report is the last token shaped as one, the name the last all-letter
    /// token that is not a report, in either order.
    #[test]
    fn a_simple_qso_reads_as_the_table_says() {
        for (typed, rst, name) in [
            ("57N BOB", "579", "BOB"),
            ("579 BOB", "579", "BOB"),
            ("BOB 579", "579", "BOB"),
            ("5NN AL", "599", "AL"),
            ("ENN hans", "599", "HANS"),
            ("599 579 BOB", "579", "BOB"),
            ("BOB TOM 449", "449", "TOM"),
        ] {
            let reading = simple_qso(typed);
            assert_eq!(reading.value(Field::Rst), Some(rst), "{typed}");
            assert_eq!(reading.value(Field::Name), Some(name), "{typed}");
            assert_eq!(reading.state, ExchangeState::Complete, "{typed}");
            assert!(reading.unreadable.is_empty(), "{typed}");
        }
        // The report is copied, so one typed before the last is unused, not
        // read as the prefilled one.
        assert_eq!(simple_qso("599 579 BOB").unused, ["599"]);
        assert_eq!(simple_qso("BOB TOM 449").unused, ["BOB"]);
        // `ENN` is a report, never a name.
        let reading = simple_qso("ENN");
        assert_eq!(reading.value(Field::Rst), Some("599"));
        assert_eq!(reading.value(Field::Name), None);
        assert_eq!(reading.state, ExchangeState::Missing);
        assert_eq!(simple_qso("BOB").state, ExchangeState::Missing);
        assert_eq!(simple_qso("").state, ExchangeState::Empty);
        for typed in ["57 BOB", "579 B0B", "699 BOB", "579 BOB?"] {
            assert_eq!(simple_qso(typed).state, ExchangeState::Invalid, "{typed}");
        }
    }

    fn sprint(text: &str) -> Reading {
        let rules = ContestKind::Sprint.rules().expect("Sprint has shipped");
        parse_exchange(rules, Side::WVe, text)
    }

    /// The Sprint rows of the design's table: the state is the last token
    /// on its list, the number the last number left, the name what is left.
    #[test]
    fn a_sprint_reads_as_the_table_says() {
        for (typed, nr, name, st, unused) in [
            ("145 BOB TN", "145", "BOB", "TN", &[][..]),
            ("145 AL AL", "145", "AL", "AL", &[]),
            ("145 BOB TN 146", "146", "BOB", "TN", &["145"]),
            // In any order, and a correction typed last wins.
            ("OR BOB 145", "145", "BOB", "OR", &[]),
            ("145 BOB TN OR", "145", "BOB", "OR", &["TN"]),
            ("145 BOB OR TOM", "145", "TOM", "OR", &["BOB"]),
            // A name that is a state's code too stays the name when the
            // state is put right.
            ("145 AL OR TN", "145", "AL", "TN", &["OR"]),
            ("145 BOB AL OR", "145", "BOB", "OR", &["AL"]),
            // A state that reads as cut digits is a state, and a name of cut
            // letters a name where a number with a digit is there.
            ("145 BOB NE OR", "145", "BOB", "OR", &["NE"]),
            ("145 ANNE ON", "145", "ANNE", "ON", &[]),
            ("TN BOB 1N5", "195", "BOB", "TN", &[]),
            // A serial cut to letters alone, beside a name.
            ("TNN BOB OR", "99", "BOB", "OR", &[]),
            ("TTN BOB OR TNN", "99", "BOB", "OR", &["TTN"]),
            // Old codes read as the new, and Labrador as the Sprint counts
            // it.
            ("7 MIKE PQ", "7", "MIKE", "QC", &[]),
            ("7 MIKE NF", "7", "MIKE", "NL", &[]),
            ("7 MIKE LB", "7", "MIKE", "NL", &[]),
            ("7 hans dx", "7", "HANS", "DX", &[]),
            // No report in the Sprint: 599 is a number like any other.
            ("145 599 BOB TN", "599", "BOB", "TN", &["145"]),
        ] {
            let reading = sprint(typed);
            assert_eq!(reading.value(Field::Nr), Some(nr), "{typed}");
            assert_eq!(reading.value(Field::Name), Some(name), "{typed}");
            assert_eq!(reading.value(Field::St), Some(st), "{typed}");
            assert_eq!(reading.unused, unused, "{typed}");
            assert_eq!(reading.state, ExchangeState::Complete, "{typed}");
            // Read in the order the log lists them.
            let order: Vec<Field> = reading.fields.iter().map(|read| read.field).collect();
            assert_eq!(order, [Field::Nr, Field::Name, Field::St], "{typed}");
        }
        assert_eq!(sprint("145 BOB").state, ExchangeState::Missing);
        assert_eq!(sprint("TN").value(Field::Nr), None, "Tennessee, not 9");
        assert_eq!(sprint("145 TN").value(Field::Name), None);
        // Cut letters alone, with nothing left for the name, are the name.
        let anne = sprint("ANNE ON");
        assert_eq!(
            (anne.value(Field::Nr), anne.value(Field::Name)),
            (None, Some("ANNE"))
        );
        assert_eq!(sprint("").state, ExchangeState::Empty);
        for typed in ["145 B0B TN", "145 BOB TN?", "1-4 BOB TN"] {
            assert_eq!(sprint(typed).state, ExchangeState::Invalid, "{typed}");
        }
    }

    #[test]
    fn a_later_number_replaces_an_earlier_one() {
        let reading = wpx("145 146");
        assert_eq!(reading.value(Field::Nr), Some("146"));
        assert_eq!(reading.unused, ["145"]);
        assert_eq!(reading.state, ExchangeState::Complete);
        assert_eq!(reading.fields[0].token, "146");
    }

    #[test]
    fn what_cannot_be_read_makes_it_invalid() {
        for typed in ["BOB", "14?", "145 TU", "R 5NN 214", "1-4"] {
            let reading = wpx(typed);
            assert_eq!(reading.state, ExchangeState::Invalid, "{typed}");
            assert!(!reading.unreadable.is_empty(), "{typed}");
        }
        let reading = wpx("145 TU");
        assert_eq!(reading.unreadable, ["TU"]);
        assert_eq!(
            reading.value(Field::Nr),
            Some("145"),
            "the rest still reads"
        );
    }

    #[test]
    fn nothing_typed_is_empty() {
        for typed in ["", "   ", "\t\n"] {
            let reading = wpx(typed);
            assert_eq!(reading.state, ExchangeState::Empty, "{typed:?}");
            assert!(reading.fields.is_empty());
        }
    }

    #[test]
    fn the_call_field_keeps_only_what_a_call_can_hold() {
        assert_eq!(normalize_call(" k5x?z "), "K5X?Z");
        assert_eq!(normalize_call("dl/w1aw"), "DL/W1AW");
        assert_eq!(normalize_call("K5-XYZ.,é"), "K5XYZ");
        assert_eq!(normalize_call(""), "");
    }

    /// Random typing never panics, a reading is what it says it is, and
    /// reading its own values again reads the same.
    #[test]
    fn parsing_survives_anything_and_settles_on_its_own_output() {
        const TYPED: &[char] = &[
            '1', '2', '5', '9', '0', 'T', 'N', 'E', 'A', 'O', 'B', 'X', 'L', 'R', '?', '/', ' ',
            ' ', ' ', 'é', 'n', '\t',
        ];
        let mut rng = SplitMix64::new(11);
        for round in 0..10_000 {
            let kind = [
                ContestKind::Wpx,
                ContestKind::SimpleQso,
                ContestKind::Sprint,
            ][round % 3];
            let rules = kind.rules().expect("shipped");
            let len = rng.usize_in(0, 16);
            let text: String = (0..len)
                .map(|_| TYPED[rng.usize_in(0, TYPED.len() - 1)])
                .collect();
            let reading = parse_exchange(rules, Side::Dx, &text);
            assert_eq!(
                reading.state == ExchangeState::Empty,
                text.trim().is_empty(),
                "{text:?}"
            );
            for read in &reading.fields {
                assert_eq!(
                    read.field.canonical(&read.token).as_deref(),
                    Some(read.value.as_str())
                );
            }
            let call = normalize_call(&text);
            assert_eq!(normalize_call(&call), call, "{text:?}");
            if reading.state == ExchangeState::Complete {
                let values: Vec<&str> = reading.fields.iter().map(|f| f.value.as_str()).collect();
                let again = parse_exchange(rules, Side::Dx, &values.join(" "));
                assert_eq!(again.state, ExchangeState::Complete, "{text:?}");
                assert_eq!(again.fields.len(), reading.fields.len());
                for (first, second) in reading.fields.iter().zip(&again.fields) {
                    assert_eq!(first.value, second.value, "{text:?}");
                    assert_eq!(second.token, second.value, "a value reads as itself");
                }
            }
        }
    }
}
