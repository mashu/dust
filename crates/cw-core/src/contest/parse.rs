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

/// The Exchange field's text, read for the fields `rules` copy on `side`.
///
/// Items may come in any order, and a later item of a kind replaces an
/// earlier one, so a correction typed last wins. Each field takes a token by
/// its own rule: a serial is the last token that reads as a number and is
/// not exactly a report (`5NN`, `ENN`, `599`), or the report when nothing
/// else is there; a copied report is the last token shaped as one (`57N` is
/// 579); a name the last all-letter token that is not a report; any other
/// field takes the last token it can read that no field before it took.
/// Sprint's parser comes with that contest.
pub fn parse_exchange(rules: &ContestRules, side: Side, text: &str) -> Reading {
    let tokens: Vec<String> = text
        .split_whitespace()
        .map(str::to_ascii_uppercase)
        .collect();
    let copied = rules.copied(side);
    let mut taken = vec![false; tokens.len()];
    let mut fields = Vec::new();
    for &field in copied {
        if let Some((at, value)) = take(field, &tokens, &taken) {
            taken[at] = true;
            fields.push(ReadField {
                field,
                token: tokens[at].clone(),
                value,
            });
        }
    }
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

/// The token `field` takes of those not yet `taken`, and its value.
fn take(field: Field, tokens: &[String], taken: &[bool]) -> Option<(usize, String)> {
    let last = |accept: fn(&str) -> bool| {
        tokens
            .iter()
            .enumerate()
            .rev()
            .filter(|(at, token)| !taken[*at] && accept(token.as_str()))
            .find_map(|(at, token)| Some((at, field.canonical(token)?)))
    };
    match field {
        // A report reads as a number too, but is a serial only when it is
        // all there is.
        Field::Nr => last(|token| !REPORTS.contains(&token)).or_else(|| last(|_| true)),
        // `ENN` is all letters, but a report.
        Field::Name => last(|token| Field::Rst.canonical(token).is_none()),
        _ => last(|_| true),
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
            '1', '2', '5', '9', '0', 'T', 'N', 'E', 'A', 'O', 'B', 'X', '?', '/', ' ', ' ', 'é',
            'n', '\t',
        ];
        let mut rng = SplitMix64::new(11);
        for round in 0..10_000 {
            let kind = [ContestKind::Wpx, ContestKind::SimpleQso][round % 2];
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
