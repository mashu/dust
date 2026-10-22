//! Each contest as one row of a table: what you send, what you copy, who
//! calls and how it scores. A new contest is a row and its data.

use super::ContestKind;
use super::data::places::{fold_labrador, w_ve};
use super::exchange::{self, Field, cut_power, cut_serial, cut_zone};
use super::serial::{SPRINT_SERIALS, SerialFit, WPX_SERIALS};
use super::turn::Sender;
use crate::callsign::{CallSpec, Countries};

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
    /// Your side, from where your call puts you: W/VE in the 48 contiguous
    /// states, DC and Canada, by every prefix they issue, and DX anywhere
    /// else, Alaska, Hawaii and the islands among it. A call Dust cannot
    /// place is DX: W/VE is the side that has to be shown.
    pub fn of_call(call: &str) -> Self {
        if w_ve(call) { Self::WVe } else { Self::Dx }
    }

    /// The side a run's log keeps as `name`: W/VE for `wve`, and DX for
    /// anything else, where sides do not apply among it.
    pub fn from_name(name: &str) -> Self {
        if name == Self::WVe.name() {
            Self::WVe
        } else {
            Self::Dx
        }
    }

    /// The name a run's log keeps it under.
    pub fn name(self) -> &'static str {
        match self {
            Self::WVe => "wve",
            Self::Dx => "dx",
        }
    }

    /// What the screens call it: `DX`, `W/VE`.
    pub fn label(self) -> &'static str {
        match self {
            Self::WVe => "W/VE",
            Self::Dx => "DX",
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
    /// Each different US state, DC, and Canadian province or territory
    /// logged, as the NA Sprint counts them, Alaska and Hawaii among the
    /// states and Labrador part of NL. DX counts none.
    Place,
    /// Each CQ zone logged, and each DXCC country of a call logged, as CQ
    /// WW counts them: Alaska and Hawaii are countries of their own, and
    /// your own country counts too.
    ZoneAndCountry,
    /// Each DXCC country of a call logged outside W/VE, as a W/VE station
    /// counts them in the ARRL DX contest.
    Country,
    /// Each of the 48 contiguous states, DC, and Canada's provinces and
    /// territories logged, Labrador apart, 63 in all, as a DX station counts
    /// them in the ARRL DX contest.
    WVePlace,
    /// Nothing multiplies the points: the log is one multiplier, as
    /// MorseRunner counts Field Day's, so the score is the QSO points.
    Nothing,
}

/// What a QSO is worth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Points {
    /// The same for every QSO.
    Each(u32),
    /// So much for a QSO with a call on the other side from yours, and
    /// nothing for one on your own, as the ARRL DX contest has W/VE work
    /// only DX.
    OtherSide(u32),
    /// By how far the station is from you, as CQ WW counts it: nothing in
    /// your own country, one point on your continent, two between North
    /// American countries, and three on another continent.
    ByDistance,
}

/// How a contest scores: points for each QSO that counts, times the
/// multipliers worked, on your side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scoring {
    pub points: Points,
    pub multiplier: PerSide<Multiplier>,
}

/// One contest. In your texts, `{my}` stands for your call, `{myname}` for
/// your name, `{mystate}` for your state, province or territory, `{myzone}`
/// for your CQ zone, `{mypwr}` for your power, `{myclass}` for your Field
/// Day class, `{mysec}` for your section, `{rst}` for your report, `{nr}`
/// for your serial, and in your TU `{hisname}` for the name you copied of
/// the station you work. In a caller's, `{my}` is your call,
/// `{his}` its own, `{rst}` its report, and each field you copy its name in
/// braces: `{nr}`, `{name}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContestRules {
    pub kind: ContestKind,
    /// Your CQ.
    pub cq: &'static str,
    /// What ends a QSO.
    pub tu: &'static str,
    /// What you ask for the station's exchange again with (F12): `NR?` in
    /// a contest, as MorseRunner's F12 sends it, `AGN` in an everyday QSO,
    /// whose exchange holds no number.
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
    /// Whether your serial goes out cut, `TT7`, as MorseRunner sends yours,
    /// rather than in plain digits, `7`, as the NA Sprint's goes.
    pub serial_cut: bool,
    /// What you copy from each caller, in the order a log lists it.
    pub copied: PerSide<&'static [Field]>,
    /// Who calls.
    pub callers: PerSide<CallSpec>,
    /// How a caller answers your CQ.
    pub answer: &'static str,
    /// A caller's exchange as it sends it, on your side.
    pub their_exchange: PerSide<&'static str>,
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
    /// Whether Labrador, LB, is a place of its own, as the ARRL DX contest
    /// counts it, rather than part of Newfoundland and Labrador, NL, as the
    /// Sprint counts it.
    pub labrador: bool,
    /// Who you work from each side, as the card says it after the side
    /// you are on; none in a contest the same from either side.
    pub works: Option<PerSide<&'static str>>,
    /// What Dust leaves out of the real contest, as the card says it; none
    /// where it says nothing.
    pub simplified: Option<&'static str>,
}

/// What your messages are filled in with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fill<'a> {
    pub my_call: &'a str,
    pub my_name: &'a str,
    /// Your state, province or territory, or DX, where the exchange sends
    /// it.
    pub my_state: &'a str,
    /// Your CQ zone, where the exchange sends it, in digits, which go out
    /// cut; anything else, a stand-in, goes out as it is.
    pub my_zone: &'a str,
    /// Your power, where the exchange sends it: watts, or `K`, `KW` or `1K`
    /// for a kilowatt.
    pub my_power: &'a str,
    /// Your Field Day class and your ARRL or RAC section, or DX, where the
    /// exchange sends them.
    pub my_class: &'a str,
    pub my_section: &'a str,
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
        serial_cut: true,
        copied: PerSide::both(&[Field::Nr]),
        callers: PerSide::both(CallSpec::contest(Countries::Any)),
        answer: "{his}",
        their_exchange: PerSide::both("{rst} {nr}"),
        their_lead: ["TU", "R"],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        serials: Some(WPX_SERIALS),
        scoring: Some(Scoring {
            points: Points::Each(1),
            multiplier: PerSide::both(Multiplier::WpxPrefix),
        }),
        labrador: false,
        works: None,
        simplified: None,
    },
    // The NCJ North American Sprint without its QSY rule: `CQ NA K1ABC` ·
    // `W7XYZ` · `W7XYZ K1ABC 7 MIKE MA` · `K1ABC 145 BOB OR` · `TU K1ABC`.
    // The real CQ station must move after each QSO; Dust keeps you on one
    // frequency, so you sign last, as the station keeping one does, and your
    // TU brings the next caller. Your serial goes out plain. One band, so a
    // call is a dupe after one QSO. A QSO is a point, times the states
    // (KL7 and KH6 among them), DC and the 13 provinces and territories;
    // other North American countries count too, but no caller is in one.
    // In Dust every station outside the US and Canada sends DX; the real
    // contest has only those outside North America send it, the rest their
    // country. That list is the design's (§3): NCJ's rules could not be
    // fetched from here, and excerpts of its 2026 rules a web search found
    // agree with it, so it is still to be checked against them before a
    // release.
    ContestRules {
        kind: ContestKind::Sprint,
        cq: "CQ NA {my}",
        tu: "TU {my}",
        query: "NR?",
        exchange: PerSide::both("{my} {nr} {myname} {mystate}"),
        report: "",
        rst: false,
        serial: true,
        serial_cut: false,
        copied: PerSide::both(&[Field::Nr, Field::Name, Field::St]),
        callers: PerSide::both(CallSpec::contest(Countries::UsAndCanada)),
        answer: "{his}",
        their_exchange: PerSide::both("{my} {nr} {name} {st}"),
        their_lead: ["", ""],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        serials: Some(SPRINT_SERIALS),
        scoring: Some(Scoring {
            points: Points::Each(1),
            multiplier: PerSide::both(Multiplier::Place),
        }),
        labrador: false,
        works: None,
        simplified: Some(
            "Simplified: you keep your frequency (in the real contest you QSY after each QSO), so you sign last; one band, so a station counts once; and outside the US and Canada you send DX (in the real contest, the rest of North America sends its country).",
        ),
    },
    // CQ World Wide DX: `CQ DL1ABC TEST` · `JA1XYZ` · `JA1XYZ 5NN A4` ·
    // `TU 5NN 25` · `TU`. Your zone goes out cut, as MorseRunner sends it,
    // and callers from anywhere send theirs. A QSO is worth nothing in your
    // own country, a point on your continent, two between North American
    // countries and three on another continent, times the zones and the
    // DXCC countries worked, Alaska and Hawaii among them (MorseRunner folds
    // them into the USA). One band, so each counts once; and no WAE
    // countries, only DXCC ones.
    ContestRules {
        kind: ContestKind::Cqww,
        cq: "CQ {my} TEST",
        tu: "TU",
        query: "NR?",
        exchange: PerSide::both("{rst} {myzone}"),
        report: "5NN",
        rst: true,
        serial: false,
        serial_cut: false,
        copied: PerSide::both(&[Field::Zn]),
        callers: PerSide::both(CallSpec::contest(Countries::Any)),
        answer: "{his}",
        their_exchange: PerSide::both("{rst} {zn}"),
        their_lead: ["TU", "R"],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        serials: None,
        scoring: Some(Scoring {
            points: Points::ByDistance,
            multiplier: PerSide::both(Multiplier::ZoneAndCountry),
        }),
        labrador: false,
        works: None,
        simplified: Some(
            "Simplified: one band, so a station, a zone and a country each count once; and no WAE countries, only DXCC ones.",
        ),
    },
    // The ARRL International DX contest: W/VE works only DX, and DX only
    // W/VE. As DX: `CQ DL1ABC TEST` · `W7XYZ` · `W7XYZ 5NN 1TT` (your 100 W,
    // cut) · `R 5NN OR` · `TU`. As W/VE: `G4XYZ 5NN MA` · `TU 5NN KW`. Three
    // points a QSO with the other side, and none with your own, times the
    // DXCC countries a W/VE station works, or the 48 states, DC and
    // Canada's provinces and territories a DX station does, Labrador (VO2)
    // apart as LB; Alaska and Hawaii are DX. One band, so each counts once.
    ContestRules {
        kind: ContestKind::ArrlDx,
        cq: "CQ {my} TEST",
        tu: "TU",
        query: "NR?",
        exchange: PerSide {
            wve: "{rst} {mystate}",
            dx: "{rst} {mypwr}",
        },
        report: "5NN",
        rst: true,
        serial: false,
        serial_cut: false,
        copied: PerSide {
            wve: &[Field::Pwr],
            dx: &[Field::St],
        },
        callers: PerSide {
            wve: CallSpec::contest(Countries::NotWVe),
            dx: CallSpec::contest(Countries::WVe),
        },
        answer: "{his}",
        their_exchange: PerSide {
            wve: "{rst} {pwr}",
            dx: "{rst} {st}",
        },
        their_lead: ["TU", "R"],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        serials: None,
        scoring: Some(Scoring {
            points: Points::OtherSide(3),
            multiplier: PerSide {
                wve: Multiplier::Country,
                dx: Multiplier::WVePlace,
            },
        }),
        labrador: true,
        works: Some(PerSide {
            wve: "you work DX only (Alaska and Hawaii count as DX)",
            dx: "you work US and Canada only",
        }),
        simplified: Some("Simplified: one band, so a station and a multiplier each count once."),
    },
    // ARRL Field Day: `CQ FD K1ABC` · `W7XYZ` · `W7XYZ 1D EMA` (his call,
    // your class, your section) · `R 3A OR` · `TU`. No report and no
    // serial. Callers are North American, with a few DX among them, who
    // send DX for their section. Two points a CW QSO; the power multiplier
    // and the bonus points score the entry, not the copying, so Dust
    // leaves them out and the score is the QSO points. One band, so a call
    // is a dupe after one QSO.
    ContestRules {
        kind: ContestKind::FieldDay,
        cq: "CQ FD {my}",
        tu: "TU",
        query: "NR?",
        exchange: PerSide::both("{myclass} {mysec}"),
        report: "",
        rst: false,
        serial: false,
        serial_cut: false,
        copied: PerSide::both(&[Field::Cl, Field::Sec]),
        callers: PerSide::both(CallSpec::contest(Countries::FieldDay)),
        answer: "{his}",
        their_exchange: PerSide::both("{cl} {sec}"),
        their_lead: ["R", "TU"],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        serials: None,
        scoring: Some(Scoring {
            points: Points::Each(2),
            multiplier: PerSide::both(Multiplier::Nothing),
        }),
        labrador: false,
        works: None,
        simplified: Some(
            "Simplified: one band, so a station counts once; and no power multiplier or bonus points, which score the entry, not the copying.",
        ),
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
        serial_cut: false,
        copied: PerSide::both(&[Field::Rst, Field::Name]),
        callers: PerSide::both(CallSpec::contest(Countries::Any)),
        answer: "{my} DE {his} {his} K",
        their_exchange: PerSide::both("UR {rst} {rst} NAME {name} {name} BK"),
        their_lead: ["R TNX", "R TNX"],
        their_query: ["AGN", "AGN"],
        signoff: Some("73 EE"),
        tu_calls: false,
        serials: None,
        scoring: None,
        labrador: false,
        works: None,
        simplified: None,
    },
];

impl Scoring {
    /// Whether anything multiplies the points, on either side: all but
    /// Field Day's do, whose score is its QSO points.
    pub fn multiplied(&self) -> bool {
        self.multiplier.wve != Multiplier::Nothing || self.multiplier.dx != Multiplier::Nothing
    }
}

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
            || self.their_exchange.wve != self.their_exchange.dx
    }

    /// The side you are on and who you work from it, where sides change
    /// it: "You are DX: you work US and Canada only". Until your call reads
    /// (`call_reads`), your side is the one a call stands in for, and the
    /// line says so. None in a contest the same from either side.
    pub fn side_note(&self, side: Side, call_reads: bool) -> Option<String> {
        let works = self.works?.on(side);
        let side = side.label();
        Some(if call_reads {
            format!("You are {side}: {works}")
        } else {
            format!("Until you add your call you are {side}: {works}")
        })
    }

    /// Whether your exchange sends your name, so a run needs one.
    pub fn sends_name(&self) -> bool {
        self.exchange.wve.contains("{myname}") || self.exchange.dx.contains("{myname}")
    }

    /// Whether your exchange on `side` sends your state, province or
    /// territory, so a run needs one: DX, outside the US and Canada.
    pub fn sends_state(&self, side: Side) -> bool {
        self.exchange.on(side).contains("{mystate}")
    }

    /// Whether your exchange on `side` sends your CQ zone.
    pub fn sends_zone(&self, side: Side) -> bool {
        self.exchange.on(side).contains("{myzone}")
    }

    /// Whether your exchange on `side` sends your power.
    pub fn sends_power(&self, side: Side) -> bool {
        self.exchange.on(side).contains("{mypwr}")
    }

    /// Whether your exchange on `side` sends your Field Day class.
    pub fn sends_class(&self, side: Side) -> bool {
        self.exchange.on(side).contains("{myclass}")
    }

    /// Whether your exchange on `side` sends your ARRL or RAC section, so a
    /// run needs one: DX, outside the sections.
    pub fn sends_section(&self, side: Side) -> bool {
        self.exchange.on(side).contains("{mysec}")
    }

    /// `token` as this contest reads `field` in it ([`Field::canonical`]):
    /// Labrador's LB is NL where the contest does not count it apart.
    pub fn canonical(&self, field: Field, token: &str) -> Option<String> {
        let value = field.canonical(token)?;
        Some(if field == Field::St && !self.labrador {
            fold_labrador(&value).to_string()
        } else {
            value
        })
    }

    /// Whether two tokens hold the same value of `field` in this contest.
    pub fn same(&self, field: Field, a: &str, b: &str) -> bool {
        match (self.canonical(field, a), self.canonical(field, b)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// A field as a session's group keeps it ([`exchange::keyed_token`]):
    /// `sent` where what was typed holds the same value in this contest.
    pub(super) fn keyed_token(&self, field: Field, sent: &str, typed: &str) -> String {
        if self.same(field, sent, typed) {
            sent.to_string()
        } else {
            exchange::keyed_token(field, sent, typed)
        }
    }

    /// Whether your TU signs with your call every time, as the Sprint's
    /// does, so a pile-up's every third QSO adds nothing to it.
    pub(super) fn tu_signs(&self) -> bool {
        self.tu.contains("{my}")
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

    /// What your exchange on `side` holds, in a few words and the order
    /// it goes in: `5NN + serial`, `599 + name`, `serial + name + state`.
    pub fn sends(&self, side: Side) -> String {
        let exchange = self.exchange.on(side);
        let mut parts: Vec<(usize, &str)> = [
            ("{rst}", self.report),
            ("{nr}", "serial"),
            ("{myname}", "name"),
            ("{mystate}", "state"),
            ("{myzone}", "zone"),
            ("{mypwr}", "power"),
            ("{myclass}", "class"),
            ("{mysec}", "section"),
        ]
        .into_iter()
        .filter_map(|(held, part)| Some((exchange.find(held)?, part)))
        .collect();
        parts.sort_unstable();
        let parts: Vec<&str> = parts.into_iter().map(|(_, part)| part).collect();
        parts.join(" + ")
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

    /// A caller's exchange as it sends it to you on `side`, each field it
    /// holds the token in `fields`, without the word before its first.
    pub(super) fn their_exchange_text(
        &self,
        side: Side,
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
            .on(side)
            .replace("{my}", my_call)
            .replace("{his}", his_call)
            .replace("{rst}", report);
        for (field, token) in fields {
            text = text.replace(&format!("{{{}}}", field.name()), token);
        }
        squeeze(&text)
    }

    /// A caller's first exchange, `body`, after the word the contest leads
    /// it with, the `lead`th of [`Self::their_lead`]; alone in a contest
    /// that leads it with none.
    pub(super) fn their_first_exchange(&self, lead: usize, body: &str) -> String {
        squeeze(&format!("{} {body}", self.their_lead[lead]))
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
            // In the seventh call district, where the example's state or
            // section is.
            _ if self.copied(side).contains(&Field::St) => "W7XYZ",
            _ if self.copied(side).contains(&Field::Sec) => "W7XYZ",
            // In Japan, whose zone the example's is.
            _ if self.copied(side).contains(&Field::Zn) => "JA1XYZ",
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
                self.their_first_exchange(
                    0,
                    &self.their_exchange_text(side, me.my_call, caller, &fields),
                ),
            ),
            (Sender::You, self.tu_text(&Fill { his_name, ..first })),
        ];
        qso.extend(self.signoff.map(|signoff| (them, signoff.to_string())));
        qso
    }

    /// `template`, one of your texts, filled in with `me`: your call, name,
    /// state, zone, power, class, section, report and serial, and the name
    /// of the station you work. A name not there leaves no gap. Your zone
    /// and power go out cut, as MorseRunner sends yours.
    fn fill(&self, template: &str, me: &Fill<'_>) -> String {
        let serial = if self.serial_cut {
            cut_serial(me.serial)
        } else {
            me.serial.to_string()
        };
        squeeze(
            &template
                .replace("{my}", me.my_call)
                .replace("{myname}", me.my_name)
                .replace("{mystate}", me.my_state)
                .replace("{myzone}", &cut_zone_text(me.my_zone))
                .replace("{mypwr}", &cut_power(me.my_power))
                .replace("{myclass}", me.my_class)
                .replace("{mysec}", me.my_section)
                .replace("{hisname}", me.his_name)
                .replace("{rst}", self.report)
                .replace("{nr}", &serial),
        )
    }
}

/// A zone given as `text` as you send it: cut ([`cut_zone`]) where it is a
/// number, and as it is otherwise, a stand-in among it.
fn cut_zone_text(text: &str) -> String {
    text.parse().map_or_else(|_| text.to_string(), cut_zone)
}

/// `text` with one space between words, and none at either end.
fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wpx() -> &'static ContestRules {
        ContestKind::Wpx.rules()
    }

    fn simple_qso() -> &'static ContestRules {
        ContestKind::SimpleQso.rules()
    }

    fn sprint() -> &'static ContestRules {
        ContestKind::Sprint.rules()
    }

    fn cqww() -> &'static ContestRules {
        ContestKind::Cqww.rules()
    }

    fn arrl_dx() -> &'static ContestRules {
        ContestKind::ArrlDx.rules()
    }

    fn field_day() -> &'static ContestRules {
        ContestKind::FieldDay.rules()
    }

    /// You, as DL1ABC called MAT in zone 14 running 100 W, sending
    /// `serial`, working `his_name`.
    fn me(serial: u32, his_name: &str) -> Fill<'_> {
        Fill {
            my_call: "DL1ABC",
            my_name: "MAT",
            my_state: "DX",
            my_zone: "14",
            my_power: "100",
            my_class: "1D",
            my_section: "DX",
            serial,
            his_name,
        }
    }

    /// You, as K1ABC called MIKE in Massachusetts, zone 5, sending
    /// `serial`.
    fn k1abc(serial: u32) -> Fill<'static> {
        Fill {
            my_call: "K1ABC",
            my_name: "MIKE",
            my_state: "MA",
            my_zone: "5",
            my_power: "KW",
            my_class: "1D",
            my_section: "EMA",
            serial,
            his_name: "BOB",
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
        assert_eq!(rules.sends(Side::Dx), "5NN + serial");
        assert!(!rules.sends_state(Side::Dx) && !rules.tu_signs());
        assert!(!rules.sends_name());
        assert!(rules.prefills_report(Side::Dx));
        assert_eq!(rules.copied(Side::Dx), [Field::Nr]);
        assert_eq!(rules.callers(Side::WVe), CallSpec::contest(Countries::Any));
        assert_eq!(rules.answer_text("DL1ABC", "K5XYZ", false), "K5XYZ");
        assert_eq!(rules.answer_text("DL1ABC", "K5XYZ", true), "K5XYZ K5XYZ");
        let theirs = [(Field::Nr, "214".to_string())];
        assert_eq!(
            rules.their_exchange_text(Side::Dx, "DL1ABC", "K5XYZ", &theirs),
            "5NN 214"
        );
        assert!(rules.tu_calls && rules.signoff.is_none());
        assert_eq!(rules.their_query, ["NR?", "AGN"]);
        assert!(!rules.sided());
        assert_eq!(
            rules.scoring,
            Some(Scoring {
                points: Points::Each(1),
                multiplier: PerSide::both(Multiplier::WpxPrefix)
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
            rules.their_exchange_text(Side::Dx, "DL1ABC", "K5XYZ", &theirs),
            "UR 579 579 NAME BOB BOB BK"
        );
        assert_eq!(rules.their_lead, ["R TNX", "R TNX"]);
        assert_eq!(rules.their_query, ["AGN", "AGN"], "no serial to ask for");
        assert_eq!(rules.signoff, Some("73 EE"));
        assert_eq!(rules.sends(Side::Dx), "599 + name");
        assert!(rules.sends_name() && !rules.tu_calls);
        assert!(!rules.prefills_report(Side::Dx), "the report is copied");
        assert_eq!(rules.copied(Side::WVe), [Field::Rst, Field::Name]);
        assert_eq!(rules.scoring, None);
        assert!(rules.serials.is_none() && !rules.serial);
    }

    /// The NA Sprint on the air, as the design has it: both calls in your
    /// exchange, your serial plain, your name and state; theirs with your
    /// call and no word before it; a TU that signs with your call and brings
    /// the next caller; a point a QSO times the places worked.
    #[test]
    fn a_sprint_sends_what_the_na_sprint_sends() {
        let rules = sprint();
        assert_eq!(rules.cq_text(&k1abc(0)), "CQ NA K1ABC");
        assert_eq!(rules.exchange_text(Side::WVe, &k1abc(7)), "K1ABC 7 MIKE MA");
        assert_eq!(
            rules.exchange_text(Side::WVe, &k1abc(190)),
            "K1ABC 190 MIKE MA",
            "plain, never cut"
        );
        assert_eq!(rules.tu_text(&k1abc(7)), "TU K1ABC");
        assert!(rules.tu_signs() && rules.tu_calls && rules.signoff.is_none());
        assert_eq!(rules.sends(Side::WVe), "serial + name + state");
        assert!(rules.sends_name() && rules.sends_state(Side::WVe) && rules.sends_state(Side::Dx));
        assert!(!rules.rst && !rules.prefills_report(Side::WVe));
        assert_eq!(rules.copied(Side::Dx), [Field::Nr, Field::Name, Field::St]);
        assert_eq!(
            rules.callers(Side::Dx),
            CallSpec::contest(Countries::UsAndCanada)
        );
        assert_eq!(rules.answer_text("K1ABC", "W7XYZ", false), "W7XYZ");
        assert_eq!(rules.answer_text("K1ABC", "W7XYZ", true), "W7XYZ W7XYZ");
        let theirs = [
            (Field::Nr, "145".to_string()),
            (Field::Name, "BOB".to_string()),
            (Field::St, "OR".to_string()),
        ];
        let sent = rules.their_exchange_text(Side::WVe, "K1ABC", "W7XYZ", &theirs);
        assert_eq!(sent, "K1ABC 145 BOB OR");
        assert_eq!(rules.their_first_exchange(0, &sent), sent, "led by nothing");
        assert_eq!(rules.query, "NR?");
        assert_eq!(rules.serials, Some(SPRINT_SERIALS));
        assert!(!rules.sided());
        assert_eq!(
            rules.scoring,
            Some(Scoring {
                points: Points::Each(1),
                multiplier: PerSide::both(Multiplier::Place)
            })
        );
    }

    /// CQ WW on the air, as the design has it: your zone cut, theirs
    /// plain, callers from anywhere, a TU that brings the next, points by
    /// distance times zones and countries.
    #[test]
    fn cqww_sends_what_the_contest_sends() {
        let rules = cqww();
        assert_eq!(rules.cq_text(&me(0, "")), "CQ DL1ABC TEST");
        assert_eq!(rules.exchange_text(Side::Dx, &me(0, "")), "5NN A4");
        assert_eq!(rules.exchange_text(Side::WVe, &k1abc(0)), "5NN 5");
        assert_eq!(rules.tu_text(&me(0, "")), "TU");
        assert_eq!(rules.sends(Side::Dx), "5NN + zone");
        assert!(rules.sends_zone(Side::Dx) && rules.sends_zone(Side::WVe));
        assert!(!rules.sends_state(Side::WVe) && !rules.sends_power(Side::Dx));
        assert!(!rules.sends_name() && rules.prefills_report(Side::Dx));
        assert_eq!(rules.copied(Side::WVe), [Field::Zn]);
        assert_eq!(rules.callers(Side::WVe), CallSpec::contest(Countries::Any));
        let theirs = [(Field::Zn, "25".to_string())];
        assert_eq!(
            rules.their_exchange_text(Side::Dx, "DL1ABC", "JA1XYZ", &theirs),
            "5NN 25"
        );
        assert!(rules.tu_calls && rules.signoff.is_none() && rules.serials.is_none());
        assert!(!rules.sided() && rules.side_note(Side::Dx, true).is_none());
        assert_eq!(
            rules.scoring,
            Some(Scoring {
                points: Points::ByDistance,
                multiplier: PerSide::both(Multiplier::ZoneAndCountry)
            })
        );
        assert!(
            rules
                .simplified
                .is_some_and(|line| line.contains("one band") && line.contains("no WAE"))
        );
    }

    /// ARRL DX on the air, from either side: as DX you send your power cut
    /// and copy a state from W/VE callers; as W/VE you send your state and
    /// copy power from DX callers; three points a QSO, times what your side
    /// counts.
    #[test]
    fn arrl_dx_sends_what_the_contest_sends_from_either_side() {
        let rules = arrl_dx();
        assert!(rules.sided() && rules.labrador);
        assert_eq!(rules.cq_text(&me(0, "")), "CQ DL1ABC TEST");
        assert_eq!(rules.exchange_text(Side::Dx, &me(0, "")), "5NN 1TT");
        assert_eq!(rules.exchange_text(Side::WVe, &k1abc(0)), "5NN MA");
        assert_eq!(rules.sends(Side::Dx), "5NN + power");
        assert_eq!(rules.sends(Side::WVe), "5NN + state");
        assert!(rules.sends_power(Side::Dx) && !rules.sends_power(Side::WVe));
        assert!(rules.sends_state(Side::WVe) && !rules.sends_state(Side::Dx));
        assert_eq!(rules.copied(Side::Dx), [Field::St]);
        assert_eq!(rules.copied(Side::WVe), [Field::Pwr]);
        assert_eq!(rules.callers(Side::Dx), CallSpec::contest(Countries::WVe));
        assert_eq!(
            rules.callers(Side::WVe),
            CallSpec::contest(Countries::NotWVe)
        );
        let st = [(Field::St, "OR".to_string())];
        assert_eq!(
            rules.their_exchange_text(Side::Dx, "DL1ABC", "W7XYZ", &st),
            "5NN OR"
        );
        let pwr = [(Field::Pwr, "KW".to_string())];
        assert_eq!(
            rules.their_exchange_text(Side::WVe, "K1ABC", "G4XYZ", &pwr),
            "5NN KW"
        );
        assert_eq!(
            rules.side_note(Side::Dx, true).as_deref(),
            Some("You are DX: you work US and Canada only")
        );
        assert_eq!(
            rules.side_note(Side::WVe, true).as_deref(),
            Some("You are W/VE: you work DX only (Alaska and Hawaii count as DX)")
        );
        // With no call yet you are DX, as the call that stands in is.
        assert_eq!(
            rules.side_note(Side::Dx, false).as_deref(),
            Some("Until you add your call you are DX: you work US and Canada only")
        );
        assert_eq!(
            rules.scoring,
            Some(Scoring {
                points: Points::OtherSide(3),
                multiplier: PerSide {
                    wve: Multiplier::Country,
                    dx: Multiplier::WVePlace
                }
            })
        );
    }

    /// Field Day on the air, as the design has it: your class and section
    /// after their call, with no report and no serial; theirs led by R; a
    /// TU that brings the next caller; two points a QSO and nothing to
    /// multiply them.
    #[test]
    fn field_day_sends_what_the_contest_sends() {
        let rules = field_day();
        let k1 = k1abc(0);
        assert_eq!(rules.cq_text(&k1), "CQ FD K1ABC");
        assert_eq!(rules.exchange_text(Side::WVe, &k1), "1D EMA");
        assert_eq!(rules.exchange_text(Side::Dx, &me(0, "")), "1D DX");
        assert_eq!(rules.tu_text(&k1), "TU");
        assert_eq!(rules.sends(Side::WVe), "class + section");
        assert!(rules.sends_class(Side::Dx) && rules.sends_section(Side::WVe));
        assert!(!rules.sends_state(Side::WVe) && !rules.sends_name());
        assert!(!rules.sends_zone(Side::WVe) && !rules.sends_power(Side::Dx));
        assert!(!rules.rst && !rules.serial && !rules.prefills_report(Side::WVe));
        assert_eq!(rules.copied(Side::Dx), [Field::Cl, Field::Sec]);
        assert_eq!(
            rules.callers(Side::WVe),
            CallSpec::contest(Countries::FieldDay)
        );
        assert_eq!(rules.answer_text("K1ABC", "W7XYZ", false), "W7XYZ");
        let theirs = [
            (Field::Cl, "3A".to_string()),
            (Field::Sec, "OR".to_string()),
        ];
        let sent = rules.their_exchange_text(Side::WVe, "K1ABC", "W7XYZ", &theirs);
        assert_eq!(sent, "3A OR");
        assert_eq!(rules.their_first_exchange(0, &sent), "R 3A OR");
        assert_eq!(rules.their_first_exchange(1, &sent), "TU 3A OR");
        assert!(rules.tu_calls && rules.signoff.is_none() && rules.serials.is_none());
        assert!(!rules.sided() && rules.side_note(Side::Dx, true).is_none());
        assert_eq!(
            rules.scoring,
            Some(Scoring {
                points: Points::Each(2),
                multiplier: PerSide::both(Multiplier::Nothing)
            })
        );
        assert!(rules.scoring.is_some_and(|scoring| !scoring.multiplied()));
        assert!(
            rules
                .simplified
                .is_some_and(|line| line.contains("one band") && line.contains("power multiplier"))
        );
        let you = Sender::You;
        let they = Sender::Caller(0);
        assert_eq!(
            rules.example_qso(Side::WVe, &k1),
            [
                (you, "CQ FD K1ABC".to_string()),
                (they, "W7XYZ".to_string()),
                (you, "W7XYZ 1D EMA".to_string()),
                (they, "R 3A OR".to_string()),
                (you, "TU".to_string()),
            ]
        );
        for rules in RULES
            .iter()
            .filter(|rules| rules.kind != ContestKind::FieldDay)
        {
            assert!(
                rules.scoring.is_none_or(|scoring| scoring.multiplied()),
                "{:?}",
                rules.kind
            );
        }
    }

    /// Labrador is LB in ARRL DX, a place of its own, and NL in the Sprint;
    /// every other value reads alike in every contest.
    #[test]
    fn labrador_reads_apart_only_in_arrl_dx() {
        assert_eq!(arrl_dx().canonical(Field::St, "LB").as_deref(), Some("LB"));
        assert!(!arrl_dx().same(Field::St, "LB", "NL"));
        assert_eq!(sprint().canonical(Field::St, "LB").as_deref(), Some("NL"));
        assert!(sprint().same(Field::St, "LB", "NL"));
        assert_eq!(sprint().keyed_token(Field::St, "NL", "LB"), "NL");
        assert_eq!(arrl_dx().keyed_token(Field::St, "NL", "LB"), "LB");
        for rules in RULES {
            assert_eq!(rules.canonical(Field::Nr, "TT7").as_deref(), Some("7"));
            assert_eq!(rules.canonical(Field::St, "PQ").as_deref(), Some("QC"));
            assert!(rules.same(Field::Pwr, "K", "1000"));
            assert!(rules.same(Field::Zn, "A4", "14"));
            assert!(rules.same(Field::Sec, "YT", "TER"));
            assert!(rules.same(Field::Cl, "03A", "3A"));
        }
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
        // A CQ WW caller is in the zone it sends: Japan's.
        assert_eq!(
            cqww().example_qso(Side::Dx, &me(0, "")),
            [
                (you, "CQ DL1ABC TEST".to_string()),
                (they, "JA1XYZ".to_string()),
                (you, "JA1XYZ 5NN A4".to_string()),
                (they, "TU 5NN 25".to_string()),
                (you, "TU".to_string()),
            ]
        );
        // ARRL DX: a W/VE caller sends you its state, and a DX caller its
        // power.
        assert_eq!(
            arrl_dx().example_qso(Side::Dx, &me(0, ""))[2..4],
            [
                (you, "W7XYZ 5NN 1TT".to_string()),
                (they, "TU 5NN OR".to_string()),
            ]
        );
        assert_eq!(
            arrl_dx().example_qso(Side::WVe, &k1abc(0))[2..4],
            [
                (you, "G4XYZ 5NN MA".to_string()),
                (they, "TU 5NN KW".to_string()),
            ]
        );
        // A Sprint caller is where the state it sends is: W7 is Oregon's
        // district.
        assert_eq!(
            sprint().example_qso(Side::WVe, &k1abc(0)),
            [
                (you, "CQ NA K1ABC".to_string()),
                (they, "W7XYZ".to_string()),
                (you, "W7XYZ K1ABC 1 MIKE MA".to_string()),
                (they, "K1ABC 214 BOB OR".to_string()),
                (you, "TU K1ABC".to_string()),
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
                assert!(rules.serial || !rules.serial_cut, "{:?}", rules.kind);
                assert!(!rules.copied(side).is_empty(), "{:?}", rules.kind);
                assert_eq!(
                    rules.copied(side).contains(&Field::Nr),
                    rules.serials.is_some(),
                    "{:?}: callers send a serial exactly when one is copied",
                    rules.kind
                );
                // Callers send every field you copy, and their report if
                // the exchange holds one.
                let theirs = rules.their_exchange.on(side);
                for field in rules.copied(side) {
                    let held = format!("{{{}}}", field.name());
                    assert!(theirs.contains(&held), "{:?}", rules.kind);
                }
                assert_eq!(theirs.contains("{rst}"), rules.rst, "{:?}", rules.kind);
                // Your exchange holds what the flags say it sends.
                assert_eq!(
                    rules.sends_state(side),
                    exchange.contains("{mystate}"),
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
            // The card says who you work exactly where sides change it.
            assert_eq!(rules.works.is_some(), rules.sided(), "{:?}", rules.kind);
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
            ("CY9AA", Side::Dx),
            ("", Side::Dx),
            // Canada's prefixes are W/VE, the table's or not.
            ("VG3ABC", Side::WVe),
            ("VO2AB", Side::WVe),
            ("VY1AB", Side::WVe),
        ] {
            assert_eq!(Side::of_call(call), side, "{call:?}");
        }
        // Every prefix the design names DX is, with any digit
        // (`places.rs` pins them all): a spot check.
        for call in ["KH2ABC", "AH6AB", "WP4AB", "KG4AB"] {
            assert_eq!(Side::of_call(call), Side::Dx, "{call}");
        }
        assert_eq!(Side::WVe.name(), "wve");
        assert_eq!(Side::Dx.name(), "dx");
        for side in [Side::WVe, Side::Dx] {
            assert_eq!(Side::from_name(side.name()), side);
        }
        assert_eq!(Side::from_name(""), Side::Dx);
        assert_eq!((Side::WVe.label(), Side::Dx.label()), ("W/VE", "DX"));
    }
}
