//! Each contest as one row of a table: what you send, what you copy, who
//! calls and how it scores. A new contest is a row and its data.

use super::ContestKind;
use super::exchange::{Field, cut_serial};
use super::serial::{SerialFit, WPX_SERIALS};
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

/// One contest. In the texts, `{my}` stands for your call, `{rst}` for your
/// report and `{nr}` for your serial.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContestRules {
    pub kind: ContestKind,
    /// Your CQ.
    pub cq: &'static str,
    /// What ends a QSO.
    pub tu: &'static str,
    /// What you send after their call.
    pub exchange: PerSide<&'static str>,
    /// Whether a report is part of your exchange. Callers send theirs as
    /// `5NN`, prefilled and never checked, as loggers do.
    pub rst: bool,
    /// Whether your serial is part of it.
    pub serial: bool,
    /// What you copy from each caller, in the order a log lists it.
    pub copied: PerSide<&'static [Field]>,
    /// Who calls.
    pub callers: PerSide<CallSpec>,
    /// How big callers' logs are, and so the serials they send, in a
    /// contest whose callers send one.
    pub serials: Option<SerialFit>,
    /// How it scores; none for a contest that does not.
    pub scoring: Option<Scoring>,
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
        exchange: PerSide::both("{rst} {nr}"),
        rst: true,
        serial: true,
        copied: PerSide::both(&[Field::Nr]),
        callers: PerSide::both(CallSpec::contest(Countries::Any)),
        serials: Some(WPX_SERIALS),
        scoring: Some(Scoring {
            points: 1,
            multiplier: Multiplier::WpxPrefix,
        }),
    },
];

/// The report every exchange sends, yours and the callers'.
pub(super) const REPORT: &str = "5NN";

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

    /// Your CQ, as sent.
    pub fn cq_text(&self, my_call: &str) -> String {
        fill(self.cq, my_call, 0)
    }

    /// Your TU, as sent.
    pub fn tu_text(&self, my_call: &str) -> String {
        fill(self.tu, my_call, 0)
    }

    /// Your exchange as sent, with `serial` as your serial.
    pub fn exchange_text(&self, side: Side, my_call: &str, serial: u32) -> String {
        fill(self.exchange.on(side), my_call, serial)
    }
}

/// `template` with your call, report and serial in it.
fn fill(template: &str, my_call: &str, serial: u32) -> String {
    template
        .replace("{my}", my_call)
        .replace("{rst}", REPORT)
        .replace("{nr}", &cut_serial(serial))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wpx() -> &'static ContestRules {
        ContestKind::Wpx.rules().expect("WPX has shipped")
    }

    /// WPX on the air, as the design has it.
    #[test]
    fn wpx_sends_what_the_contest_sends() {
        let rules = wpx();
        assert_eq!(rules.cq_text("DL1ABC"), "CQ DL1ABC TEST");
        assert_eq!(rules.exchange_text(Side::Dx, "DL1ABC", 7), "5NN TT7");
        assert_eq!(rules.exchange_text(Side::WVe, "K1ABC", 195), "5NN 1N5");
        assert_eq!(rules.tu_text("DL1ABC"), "TU");
        assert_eq!(rules.copied(Side::Dx), [Field::Nr]);
        assert_eq!(rules.callers(Side::WVe), CallSpec::contest(Countries::Any));
        assert!(!rules.sided());
        assert_eq!(
            rules.scoring,
            Some(Scoring {
                points: 1,
                multiplier: Multiplier::WpxPrefix
            })
        );
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
            }
            assert!(rules.cq.contains("{my}"), "{:?}", rules.kind);
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
