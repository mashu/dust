//! Each contest as one row of a table: what you send, what you copy, who
//! calls and how it scores. A new contest is a row and its data.

use super::ContestKind;
use super::exchange::{Field, cut_serial};
use super::serial::{SerialFit, WPX_SERIALS};
use super::turn::Sender;
use crate::callsign::{CallSpec, Countries, country_of};

/// Your side of the ARRL DX contest, from your call: W/VE works only DX, and
/// DX only W/VE. Every other contest is the same from either side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// The USA, without Alaska, Hawaii or the islands, and Canada.
    WVe,
    /// Everywhere else.
    Dx,
}

impl Side {
    /// Your side, from the country your call is in. A call that cannot be
    /// read is DX: W/VE is the side that has to be shown.
    pub fn of_call(call: &str) -> Self {
        match country_of(call) {
            Some(country) if Countries::WVe.admits(country) => Self::WVe,
            _ => Self::Dx,
        }
    }

    /// The name a run's log keeps it under.
    pub fn name(self) -> &'static str {
        match self {
            Self::WVe => "wve",
            Self::Dx => "dx",
        }
    }
}

/// A value that can differ with your [`Side`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerSide<T> {
    pub wve: T,
    pub dx: T,
}

impl<T: Copy> PerSide<T> {
    /// The same from either side.
    const fn both(value: T) -> Self {
        Self {
            wve: value,
            dx: value,
        }
    }

    pub fn on(&self, side: Side) -> T {
        match side {
            Side::WVe => self.wve,
            Side::Dx => self.dx,
        }
    }
}

/// What makes a QSO count more than once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Multiplier {
    /// Each different WPX prefix, as CQ WPX counts them
    /// ([`crate::wpx_prefix`]).
    WpxPrefix,
}

/// How a contest scores: points for each QSO that counts, times the
/// multipliers worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scoring {
    pub points: u32,
    pub multiplier: Multiplier,
}

/// One contest. In your texts, `{my}` stands for your call, `{myname}` for
/// your name, `{rst}` for your report, `{nr}` for your serial, and in your TU
/// `{hisname}` for the name you copied of the station you work. In a
/// caller's, `{my}` is your call, `{his}` its own, `{rst}` its report, and
/// each field you copy its name in braces: `{nr}`, `{name}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContestRules {
    pub kind: ContestKind,
    /// Your CQ.
    pub cq: &'static str,
    /// What ends a QSO.
    pub tu: &'static str,
    /// What you ask for the station's exchange again with (F12): `NR?`
    /// where the exchange holds a number, `AGN` where it does not.
    pub query: &'static str,
    /// What you send after their call.
    pub exchange: PerSide<&'static str>,
    /// Your report, as your exchange sends it.
    pub report: &'static str,
    /// Whether a report is part of the exchange, yours and the callers'.
    /// Unless you copy it, theirs is [`Self::report`] too, prefilled and
    /// never checked, as loggers do.
    pub rst: bool,
    /// Whether your serial is part of it.
    pub serial: bool,
    /// What you copy from each caller, in the order a log lists it.
    pub copied: PerSide<&'static [Field]>,
    /// Who calls.
    pub callers: PerSide<CallSpec>,
    /// How a caller answers your CQ.
    pub answer: &'static str,
    /// A caller's exchange as it sends it.
    pub their_exchange: &'static str,
    /// What a caller sends before its exchange the first time: the first
    /// of these as often as `caller.rs`'s `FIRST_LEAD` has it, the second
    /// otherwise. Sent again, the exchange goes alone.
    pub their_lead: [&'static str; 2],
    /// What a caller that has its call but not your exchange asks with:
    /// the first as often as `caller.rs`'s `FIRST_QUERY` has it, the second
    /// otherwise.
    pub their_query: [&'static str; 2],
    /// What the station you worked sends after your TU, heard and never
    /// logged; none where it falls silent.
    pub signoff: Option<&'static str>,
    /// Whether your TU brings the next caller, as a CQ does.
    pub tu_calls: bool,
    /// How big callers' logs are, and so the serials they send, in a
    /// contest whose callers send one.
    pub serials: Option<SerialFit>,
    /// How it scores; none for a contest that does not.
    pub scoring: Option<Scoring>,
}

/// What your messages are filled in with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fill<'a> {
    pub my_call: &'a str,
    pub my_name: &'a str,
    /// Your serial for the QSO; none in a contest without one.
    pub serial: u32,
    /// The name you copied of the station you work, as the Exchange field
    /// reads it, for a TU that greets it by name. Empty for none.
    pub his_name: &'a str,
}

/// Every contest that can be run. The others join as their phase ships.
pub(super) const RULES: &[ContestRules] = &[
    // CQ WPX: `CQ DL1ABC TEST` · `K5XYZ` · `K5XYZ 5NN TT7` · `TU 5NN 214` ·
    // `TU`. The real contest scores one to six points by continent and band;
    // MorseRunner scores one a QSO, and so does Dust, so scores compare with
    // the runs its users share.
    ContestRules {
        kind: ContestKind::Wpx,
        cq: "CQ {my} TEST",
        tu: "TU",
        query: "NR?",
        exchange: PerSide::both("{rst} {nr}"),
        report: "5NN",
        rst: true,
        serial: true,
        copied: PerSide::both(&[Field::Nr]),
        callers: PerSide::both(CallSpec::contest(Countries::Any)),
        answer: "{his}",
        their_exchange: "{rst} {nr}",
        their_lead: ["TU", "R"],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        serials: Some(WPX_SERIALS),
        scoring: Some(Scoring {
            points: 1,
            multiplier: Multiplier::WpxPrefix,
        }),
    },
    // An everyday QSO, a first real one for a learner: `CQ CQ DE DL1ABC
    // DL1ABC K` · `DL1ABC DE K5XYZ K5XYZ K` · `K5XYZ DE DL1ABC UR 599 599
    // NAME MAT MAT BK` · `R TNX UR 579 579 NAME BOB BOB BK` · `TU BOB 73 EE`
    // · `73 EE`. The report is copied, as the name is. A TU ends the QSO and
    // no more: you call CQ for the next, and nothing scores it but the copy.
    ContestRules {
        kind: ContestKind::SimpleQso,
        cq: "CQ CQ DE {my} {my} K",
        tu: "TU {hisname} 73 EE",
        query: "AGN",
        exchange: PerSide::both("DE {my} UR {rst} {rst} NAME {myname} {myname} BK"),
        report: "599",
        rst: true,
        serial: false,
        copied: PerSide::both(&[Field::Rst, Field::Name]),
        callers: PerSide::both(CallSpec::contest(Countries::Any)),
        answer: "{my} DE {his} {his} K",
        their_exchange: "UR {rst} {rst} NAME {name} {name} BK",
        their_lead: ["R TNX", "R TNX"],
        their_query: ["AGN", "AGN"],
        signoff: Some("73 EE"),
        tu_calls: false,
        serials: None,
        scoring: None,
    },
];

impl ContestRules {
    /// What you copy from each caller, on your side.
    pub fn copied(&self, side: Side) -> &'static [Field] {
        self.copied.on(side)
    }

    /// Who calls you, on your side.
    pub fn callers(&self, side: Side) -> CallSpec {
        self.callers.on(side)
    }

    /// Whether your side changes anything, so a run's log has one to keep.
    pub fn sided(&self) -> bool {
        self.exchange.wve != self.exchange.dx
            || self.copied.wve != self.copied.dx
            || self.callers.wve != self.callers.dx
    }

    /// Whether your exchange sends your name, so a run needs one.
    pub fn sends_name(&self) -> bool {
        [self.exchange.wve, self.exchange.dx]
            .iter()
            .any(|exchange| exchange.contains("{myname}"))
    }

    /// Whether callers send the report prefilled, never checked: a report
    /// is part of the exchange, and you do not copy it.
    pub(super) fn prefills_report(&self, side: Side) -> bool {
        self.rst && !self.copied(side).contains(&Field::Rst)
    }

    /// Your CQ, as sent.
    pub fn cq_text(&self, me: &Fill<'_>) -> String {
        self.fill(self.cq, me)
    }

    /// Your TU, as sent.
    pub fn tu_text(&self, me: &Fill<'_>) -> String {
        self.fill(self.tu, me)
    }

    /// Your exchange as sent.
    pub fn exchange_text(&self, side: Side, me: &Fill<'_>) -> String {
        self.fill(self.exchange.on(side), me)
    }

    /// What your exchange holds, in a few words: `5NN + serial`, `599 +
    /// name`.
    pub fn sends(&self) -> String {
        let report = self.rst.then_some(self.report);
        let name = self.sends_name().then_some("name");
        let serial = self.serial.then_some("serial");
        report
            .into_iter()
            .chain(name)
            .chain(serial)
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// How a caller signing `his_call` answers your CQ, as `my_call`. One
    /// that calls `twice` sends its call twice where the answer has it
    /// once; an answer that sends it twice already is sent as it is.
    pub(super) fn answer_text(&self, my_call: &str, his_call: &str, twice: bool) -> String {
        let his = if twice && self.answer.matches("{his}").count() == 1 {
            format!("{his_call} {his_call}")
        } else {
            his_call.to_string()
        };
        squeeze(&self.answer.replace("{my}", my_call).replace("{his}", &his))
    }

    /// A caller's exchange as it sends it, each field it holds the token in
    /// `fields`, without the word before its first.
    pub(super) fn their_exchange_text(
        &self,
        my_call: &str,
        his_call: &str,
        fields: &[(Field, String)],
    ) -> String {
        let report = fields
            .iter()
            .find(|(field, _)| *field == Field::Rst)
            .map_or(self.report, |(_, token)| token.as_str());
        let mut text = self
            .their_exchange
            .replace("{my}", my_call)
            .replace("{his}", his_call)
            .replace("{rst}", report);
        for (field, token) in fields {
            text = text.replace(&format!("{{{}}}", field.name()), token);
        }
        squeeze(&text)
    }

    /// A first QSO of this contest as it goes out, with `me` as you: who
    /// sends each line, and what. A caller answers your CQ, you send its
    /// call and your exchange, it sends its own, your TU ends it, and it
    /// signs off if it does. The caller is a placeholder from the countries
    /// that call you, its exchange each copied field's example, as callers
    /// send them.
    pub fn example_qso(&self, side: Side, me: &Fill<'_>) -> Vec<(Sender, String)> {
        let caller = match self.callers(side).countries {
            Countries::NotWVe => "G4XYZ",
            _ => "K5XYZ",
        };
        let fields: Vec<(Field, String)> = self
            .copied(side)
            .iter()
            .map(|field| (*field, field.example().to_string()))
            .collect();
        let his_name = if self.copied(side).contains(&Field::Name) {
            Field::Name.example()
        } else {
            ""
        };
        let first = Fill { serial: 1, ..*me };
        let them = Sender::Caller(0);
        let mut qso = vec![
            (Sender::You, self.cq_text(me)),
            (them, self.answer_text(me.my_call, caller, false)),
            (
                Sender::You,
                format!("{caller} {}", self.exchange_text(side, &first)),
            ),
            (
                them,
                format!(
                    "{} {}",
                    self.their_lead[0],
                    self.their_exchange_text(me.my_call, caller, &fields)
                ),
            ),
            (Sender::You, self.tu_text(&Fill { his_name, ..first })),
        ];
        qso.extend(self.signoff.map(|signoff| (them, signoff.to_string())));
        qso
    }

    /// `template`, one of your texts, filled in with `me`: your call, name,
    /// report and serial, and the name of the station you work. A name not
    /// there leaves no gap.
    fn fill(&self, template: &str, me: &Fill<'_>) -> String {
        squeeze(
            &template
                .replace("{my}", me.my_call)
                .replace("{myname}", me.my_name)
                .replace("{hisname}", me.his_name)
                .replace("{rst}", self.report)
                .replace("{nr}", &cut_serial(me.serial)),
        )
    }
}

/// `text` with one space between words, and none at either end.
fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wpx() -> &'static ContestRules {
        ContestKind::Wpx.rules().expect("WPX has shipped")
    }

    fn simple_qso() -> &'static ContestRules {
        ContestKind::SimpleQso
            .rules()
            .expect("Simple QSO has shipped")
    }

    /// You, as DL1ABC called MAT, sending `serial`, working `his_name`.
    fn me(serial: u32, his_name: &str) -> Fill<'_> {
        Fill {
            my_call: "DL1ABC",
            my_name: "MAT",
            serial,
            his_name,
        }
    }

    /// WPX on the air, as the design has it.
    #[test]
    fn wpx_sends_what_the_contest_sends() {
        let rules = wpx();
        assert_eq!(rules.cq_text(&me(0, "")), "CQ DL1ABC TEST");
        assert_eq!(rules.exchange_text(Side::Dx, &me(7, "")), "5NN TT7");
        let k1abc = Fill {
            my_call: "K1ABC",
            serial: 195,
            ..Fill::default()
        };
        assert_eq!(rules.exchange_text(Side::WVe, &k1abc), "5NN 1N5");
        assert_eq!(rules.tu_text(&me(7, "BOB")), "TU");
        assert_eq!(rules.sends(), "5NN + serial");
        assert!(!rules.sends_name());
        assert!(rules.prefills_report(Side::Dx));
        assert_eq!(rules.copied(Side::Dx), [Field::Nr]);
        assert_eq!(rules.callers(Side::WVe), CallSpec::contest(Countries::Any));
        assert_eq!(rules.answer_text("DL1ABC", "K5XYZ", false), "K5XYZ");
        assert_eq!(rules.answer_text("DL1ABC", "K5XYZ", true), "K5XYZ K5XYZ");
        let theirs = [(Field::Nr, "214".to_string())];
        assert_eq!(
            rules.their_exchange_text("DL1ABC", "K5XYZ", &theirs),
            "5NN 214"
        );
        assert!(rules.tu_calls && rules.signoff.is_none());
        assert_eq!(rules.their_query, ["NR?", "AGN"]);
        assert!(!rules.sided());
        assert_eq!(
            rules.scoring,
            Some(Scoring {
                points: 1,
                multiplier: Multiplier::WpxPrefix
            })
        );
    }

    /// Simple QSO on the air, as the design has it: your name and a report
    /// of 599 in your exchange, theirs copied, a TU that greets them by name
    /// and brings nobody, and no score.
    #[test]
    fn a_simple_qso_sends_what_an_everyday_qso_sends() {
        let rules = simple_qso();
        assert_eq!(rules.cq_text(&me(0, "")), "CQ CQ DE DL1ABC DL1ABC K");
        assert_eq!(
            rules.exchange_text(Side::Dx, &me(0, "")),
            "DE DL1ABC UR 599 599 NAME MAT MAT BK"
        );
        assert_eq!(rules.tu_text(&me(0, "BOB")), "TU BOB 73 EE");
        // No name copied: no gap where it would be.
        assert_eq!(rules.tu_text(&me(0, "")), "TU 73 EE");
        // Its call is twice in it already, whoever calls.
        for twice in [false, true] {
            assert_eq!(
                rules.answer_text("DL1ABC", "K5XYZ", twice),
                "DL1ABC DE K5XYZ K5XYZ K"
            );
        }
        let theirs = [
            (Field::Rst, "579".to_string()),
            (Field::Name, "BOB".to_string()),
        ];
        assert_eq!(
            rules.their_exchange_text("DL1ABC", "K5XYZ", &theirs),
            "UR 579 579 NAME BOB BOB BK"
        );
        assert_eq!(rules.their_lead, ["R TNX", "R TNX"]);
        assert_eq!(rules.their_query, ["AGN", "AGN"], "no serial to ask for");
        assert_eq!(rules.signoff, Some("73 EE"));
        assert_eq!(rules.sends(), "599 + name");
        assert!(rules.sends_name() && !rules.tu_calls);
        assert!(!rules.prefills_report(Side::Dx), "the report is copied");
        assert_eq!(rules.copied(Side::WVe), [Field::Rst, Field::Name]);
        assert_eq!(rules.scoring, None);
        assert!(rules.serials.is_none() && !rules.serial);
    }

    /// The card's QSO is the row's: its CQ, your exchange and TU, and the
    /// caller's answer and exchange as callers send them.
    #[test]
    fn a_first_qso_is_shown_as_the_contest_goes() {
        let you = Sender::You;
        let they = Sender::Caller(0);
        assert_eq!(
            wpx().example_qso(Side::Dx, &me(0, "")),
            [
                (you, "CQ DL1ABC TEST".to_string()),
                (they, "K5XYZ".to_string()),
                (you, "K5XYZ 5NN TT1".to_string()),
                (they, "TU 5NN 214".to_string()),
                (you, "TU".to_string()),
            ]
        );
        assert_eq!(
            simple_qso().example_qso(Side::Dx, &me(0, "")),
            [
                (you, "CQ CQ DE DL1ABC DL1ABC K".to_string()),
                (they, "DL1ABC DE K5XYZ K5XYZ K".to_string()),
                (
                    you,
                    "K5XYZ DE DL1ABC UR 599 599 NAME MAT MAT BK".to_string()
                ),
                (they, "R TNX UR 579 579 NAME BOB BOB BK".to_string()),
                (you, "TU BOB 73 EE".to_string()),
                (they, "73 EE".to_string()),
            ]
        );
        for rules in RULES {
            for side in [Side::WVe, Side::Dx] {
                let k1abc = Fill {
                    my_call: "K1ABC",
                    ..Fill::default()
                };
                let qso = rules.example_qso(side, &k1abc);
                assert_eq!(qso[0].1, rules.cq_text(&k1abc), "{:?}", rules.kind);
                assert_eq!(qso[4].0, you, "{:?}", rules.kind);
                // What they send reads as what you copy.
                let reading = crate::contest::parse_exchange(rules, side, &qso[3].1);
                let copied: Vec<Field> = reading.fields.iter().map(|read| read.field).collect();
                assert_eq!(copied, rules.copied(side), "{:?}", rules.kind);
            }
        }
    }

    /// The flags say what the templates send, so neither can drift from the
    /// other as rows are added.
    #[test]
    fn every_row_is_consistent() {
        for rules in RULES {
            for side in [Side::WVe, Side::Dx] {
                let exchange = rules.exchange.on(side);
                assert_eq!(exchange.contains("{rst}"), rules.rst, "{:?}", rules.kind);
                assert_eq!(exchange.contains("{nr}"), rules.serial, "{:?}", rules.kind);
                assert!(!rules.copied(side).is_empty(), "{:?}", rules.kind);
                assert_eq!(
                    rules.copied(side).contains(&Field::Nr),
                    rules.serials.is_some(),
                    "{:?}: callers send a serial exactly when one is copied",
                    rules.kind
                );
                // Callers send every field you copy, and their report if
                // the exchange holds one.
                for field in rules.copied(side) {
                    let held = format!("{{{}}}", field.name());
                    assert!(rules.their_exchange.contains(&held), "{:?}", rules.kind);
                }
                assert_eq!(
                    rules.their_exchange.contains("{rst}"),
                    rules.rst,
                    "{:?}",
                    rules.kind
                );
                // Your TU greets them by a name only you copy.
                assert!(
                    !rules.tu.contains("{hisname}") || rules.copied(side).contains(&Field::Name),
                    "{:?}",
                    rules.kind
                );
            }
            assert!(rules.cq.contains("{my}"), "{:?}", rules.kind);
            assert!(rules.answer.contains("{his}"), "{:?}", rules.kind);
        }
        for kind in ContestKind::ALL {
            let rows = RULES.iter().filter(|rules| rules.kind == kind).count();
            assert!(rows <= 1, "{kind:?} has {rows} rows");
        }
    }

    #[test]
    fn your_side_comes_from_your_call() {
        for (call, side) in [
            ("K1ABC", Side::WVe),
            ("VE3ABC", Side::WVe),
            ("W1AW/4", Side::WVe),
            ("KH6XYZ", Side::Dx),
            ("KL7XYZ", Side::Dx),
            ("DL1ABC", Side::Dx),
            ("DL/W1AW", Side::Dx),
            // A host that reads as a call is still where the station is.
            ("VP2E/K1ABC", Side::Dx),
            ("CE0Y/K1ABC", Side::Dx),
            ("3D2R/W1AW", Side::Dx),
            // So are the prefixes W/VE counts as DX that the table lacks.
            ("NL7AB", Side::Dx),
            ("WL7AB", Side::Dx),
            ("CY0AA", Side::Dx),
            ("", Side::Dx),
        ] {
            assert_eq!(Side::of_call(call), side, "{call:?}");
        }
        assert_eq!(Side::WVe.name(), "wve");
        assert_eq!(Side::Dx.name(), "dx");
    }
}
