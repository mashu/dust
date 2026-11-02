//! Each contest as one row of a table: what you send, what you copy, who
//! calls and how it scores. A new contest is a row and its data.

use super::data::places::{DX, fold_labrador, in_us_or_canada, w_ve};
use super::exchange::{self, Field, cut_power, cut_serial, cut_zone};
use super::serial::{SPRINT_SERIALS, SerialFit, WPX_SERIALS};
use super::turn::Sender;
use super::{ContestKind, TuCall};
use crate::callsign::{CallSpec, Countries};

/// Your side of the ARRL DX contest, from your call: W/VE works only DX, and
/// DX only W/VE. In POTA, an activator in the US or Canada sends its state
/// and one anywhere else its report alone ([`Sides`]). Every other contest
/// is the same from either side.
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

/// Where a contest that has sides puts the line between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sides {
    /// As the ARRL DX contest has them ([`Side::of_call`]): W/VE is the
    /// 48 contiguous states, DC and Canada.
    WVe,
    /// The US, Alaska and Hawaii among it, and Canada, whose stations have
    /// a state, province or territory to send, on the W/VE side; everyone
    /// else, and a call Dust cannot place, on the DX side, as POTA has
    /// stations outside the US send their report alone.
    UsAndCanada,
}

impl Sides {
    /// The side a station signing `call` is on.
    pub fn of_call(self, call: &str) -> Side {
        match self {
            Self::WVe => Side::of_call(call),
            Self::UsAndCanada if in_us_or_canada(call) == Some(true) => Side::WVe,
            Self::UsAndCanada => Side::Dx,
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
/// for your serial, `{greet}` for the greeting of the time of day (`GM`,
/// `GA`, `GE`), and in your TU `{hisname}` and `{hisstate}` for the name and
/// the state you copied of the station you work, and `{bye}` for your
/// farewell, `73` and now and then `73 44` ([`FAREWELLS`]). In a caller's, `{my}` is
/// your call, `{his}` its own, `{rst}` its report, and each field you copy
/// its name in braces: `{nr}`, `{name}`. A caller's lead may hold `{my}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContestRules {
    pub kind: ContestKind,
    /// Your CQ.
    pub cq: &'static str,
    /// What ends a QSO, on your side.
    pub tu: PerSide<&'static str>,
    /// A long CQ, as a station that is not calling you sends one on your
    /// frequency, `{his}` its call: the QRM of rough habits, as
    /// MorseRunner's QRM stations send theirs (`Contest.pas:491`,
    /// `ArrlFd.pas:444`).
    pub long_cq: &'static str,
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
    /// Whether serials go out cut, `TT7`, as MorseRunner sends yours and
    /// callers with real habits send theirs, rather than in plain digits,
    /// `7`, as the NA Sprint's go, yours and the callers' alike.
    pub serial_cut: bool,
    /// What you copy from each caller, in the order a log lists it.
    pub copied: PerSide<&'static [Field]>,
    /// The fields you copy that a caller may not send, as a POTA hunter
    /// outside the US and Canada sends no state: an exchange is complete
    /// without them, and one a station did not send is not checked.
    pub optional: &'static [Field],
    /// Where the line between the sides is, in a contest that has them.
    pub sides: Sides,
    /// Who calls.
    pub callers: PerSide<CallSpec>,
    /// How a caller answers your CQ.
    pub answer: &'static str,
    /// A caller's exchange as it sends it, on your side.
    pub their_exchange: PerSide<&'static str>,
    /// The whole replies callers keep to, one each, where they vary as you
    /// set them, as POTA's hunters do: each a corner of the shape you place
    /// the dot in ([`Self::reply_shares`]), in order clockwise from the top
    /// right ([`shape_corners`]).
    /// Empty where every caller sends [`Self::their_exchange`] after a lead
    /// drawn from [`Self::their_lead`].
    pub their_replies: &'static [Reply],
    /// What a caller sends before its exchange the first time, each with
    /// the share of callers that lead with it, the shares making one,
    /// `{my}` your call in it, where callers do not keep to replies of
    /// their own. Sent again, the exchange goes alone.
    pub their_lead: &'static [(&'static str, f64)],
    /// What a caller that has its call but not your exchange asks with:
    /// the first as often as `caller.rs`'s `FIRST_QUERY` has it, the second
    /// otherwise.
    pub their_query: [&'static str; 2],
    /// What the station you worked sends after your TU, heard and never
    /// logged; none where it falls silent.
    pub signoff: Option<&'static str>,
    /// Whether your TU brings the next caller, as a CQ does.
    pub tu_calls: bool,
    /// Whether your call may follow your TU as you choose ([`TuCall`]),
    /// where the TU brings the next caller and does not sign by its own
    /// rules: not in POTA, whose TU ends with `EE`, the hunter's to answer.
    pub tu_call: bool,
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
    /// The state, province or territory you copied of the station you work,
    /// as the Exchange field reads it, for a TU that sends it back, as a
    /// POTA activator's does. Empty for none, and DX goes back as none.
    pub his_state: &'a str,
    /// The greeting of the time of day where you are ([`greeting`]), for an
    /// exchange that holds one, as a POTA activator's does: `GA`.
    pub greeting: &'a str,
    /// Your farewell, for a TU that holds one, as a POTA activator's does:
    /// one of [`FAREWELLS`]. Empty for the first, `73`.
    pub farewell: &'a str,
    /// Your TU signs with your call, though the contest's does not by its
    /// own rules: a pile-up's, as often as you choose
    /// ([`ContestRules::tu_adds_call`]).
    pub tu_signed: bool,
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
        tu: PerSide::both("TU"),
        long_cq: "CQ CQ TEST {his} {his} TEST",
        query: "NR?",
        exchange: PerSide::both("{rst} {nr}"),
        report: "5NN",
        rst: true,
        serial: true,
        serial_cut: true,
        copied: PerSide::both(&[Field::Nr]),
        optional: &[],
        sides: Sides::WVe,
        callers: PerSide::both(CallSpec::contest(Countries::Any)),
        answer: "{his}",
        their_exchange: PerSide::both("{rst} {nr}"),
        their_replies: &[],
        their_lead: &[("TU", 0.9), ("R", 0.1)],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        tu_call: true,
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
        tu: PerSide::both("TU {my}"),
        long_cq: "CQ CQ NA {his} {his} NA",
        query: "NR?",
        exchange: PerSide::both("{my} {nr} {myname} {mystate}"),
        report: "",
        rst: false,
        serial: true,
        serial_cut: false,
        copied: PerSide::both(&[Field::Nr, Field::Name, Field::St]),
        optional: &[],
        sides: Sides::WVe,
        callers: PerSide::both(CallSpec::contest(Countries::UsAndCanada)),
        answer: "{his}",
        their_exchange: PerSide::both("{my} {nr} {name} {st}"),
        their_replies: &[],
        their_lead: &[("", 1.0)],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        tu_call: true,
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
        tu: PerSide::both("TU"),
        long_cq: "CQ CQ TEST {his} {his} TEST",
        query: "NR?",
        exchange: PerSide::both("{rst} {myzone}"),
        report: "5NN",
        rst: true,
        serial: false,
        serial_cut: false,
        copied: PerSide::both(&[Field::Zn]),
        optional: &[],
        sides: Sides::WVe,
        callers: PerSide::both(CallSpec::contest(Countries::Any)),
        answer: "{his}",
        their_exchange: PerSide::both("{rst} {zn}"),
        their_replies: &[],
        their_lead: &[("TU", 0.9), ("R", 0.1)],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        tu_call: true,
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
        tu: PerSide::both("TU"),
        long_cq: "CQ CQ TEST {his} {his} TEST",
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
        optional: &[],
        sides: Sides::WVe,
        callers: PerSide {
            wve: CallSpec::contest(Countries::NotWVe),
            dx: CallSpec::contest(Countries::WVe),
        },
        answer: "{his}",
        their_exchange: PerSide {
            wve: "{rst} {pwr}",
            dx: "{rst} {st}",
        },
        their_replies: &[],
        their_lead: &[("TU", 0.9), ("R", 0.1)],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        tu_call: true,
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
        tu: PerSide::both("TU"),
        long_cq: "CQ CQ FD {his} {his} FD",
        query: "NR?",
        exchange: PerSide::both("{myclass} {mysec}"),
        report: "",
        rst: false,
        serial: false,
        serial_cut: false,
        copied: PerSide::both(&[Field::Cl, Field::Sec]),
        optional: &[],
        sides: Sides::WVe,
        callers: PerSide::both(CallSpec::contest(Countries::FieldDay)),
        answer: "{his}",
        their_exchange: PerSide::both("{cl} {sec}"),
        their_replies: &[],
        their_lead: &[("R", 0.9), ("TU", 0.1)],
        their_query: ["NR?", "AGN"],
        signoff: None,
        tu_calls: true,
        tu_call: true,
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
    // DL1ABC K` · `DL1ABC DE K5XYZ K` · `K5XYZ DE DL1ABC UR 599 599 NAME
    // MAT MAT BK` · `R TNX UR 579 579 NAME BOB BOB BK` · `TU BOB 73 EE` ·
    // `73 EE`. The report is copied, as the name is. A TU ends the QSO and
    // no more: you call CQ for the next, and nothing scores it but the copy.
    // A caller signs once, as MorseRunner's single callers do, and a pile-up
    // caller that calls twice signs twice.
    ContestRules {
        kind: ContestKind::SimpleQso,
        cq: "CQ CQ DE {my} {my} K",
        tu: PerSide::both("TU {hisname} 73 EE"),
        long_cq: "CQ CQ CQ DE {his} {his} K",
        query: "AGN",
        exchange: PerSide::both("DE {my} UR {rst} {rst} NAME {myname} {myname} BK"),
        report: "599",
        rst: true,
        serial: false,
        serial_cut: false,
        copied: PerSide::both(&[Field::Rst, Field::Name]),
        optional: &[],
        sides: Sides::WVe,
        callers: PerSide::both(CallSpec::contest(Countries::Any)),
        answer: "{my} DE {his} K",
        their_exchange: PerSide::both("UR {rst} {rst} NAME {name} {name} BK"),
        their_replies: &[],
        their_lead: &[("R TNX", 1.0)],
        their_query: ["AGN", "AGN"],
        signoff: Some("73 EE"),
        tu_calls: false,
        tu_call: true,
        serials: None,
        scoring: None,
        labrador: false,
        works: None,
        simplified: None,
    },
    // Parks on the Air, from the activator's side: `CQ CQ POTA DE K1ABC K`
    // · `W7XYZ` · `W7XYZ TU GA UR 5NN 5NN MA MA BK` · `BK TU UR 579 579 OR OR
    // BK` · `BK TU OR OR 73 EE` · `EE`. You greet the hunter by the time of
    // day, and sign off with `73`, now and then `73 44`. Hunters in the US
    // and Canada send their report and their state or province, which you
    // copy, and your TU sends the state back; about one in four is DX and
    // sends its report alone, as DX stations do, so its state is optional.
    // Each hunter keeps to one of eight whole replies: it starts `BK`, your
    // call and `TU`, or `BK TU`, `R TU` or `TU`, and sends its report and
    // state twice or each once. How often each comes is where you put the
    // dot in the eight-cornered shape on the card. Your TU ends with `EE`, never your call, for the
    // hunter to answer with its own `EE`. From outside the US and
    // Canada, Alaska and Hawaii being in them, you send your report alone,
    // and your TU sends no state back: `BK TU 73 EE`. Your TU brings the
    // next hunter once the one you worked has signed off, as a pile-up of
    // hunters calls; nothing scores a run, which is no contest, and a
    // hunter you worked does not call again.
    ContestRules {
        kind: ContestKind::Pota,
        cq: "CQ CQ POTA DE {my} K",
        tu: PerSide {
            wve: "BK TU {hisstate} {hisstate} {bye} EE",
            dx: "BK TU {bye} EE",
        },
        long_cq: "CQ CQ POTA DE {his} {his} K",
        query: "AGN",
        exchange: PerSide {
            wve: "TU {greet} UR {rst} {rst} {mystate} {mystate} BK",
            dx: "TU {greet} UR {rst} {rst} BK",
        },
        report: "5NN",
        rst: true,
        serial: false,
        serial_cut: false,
        copied: PerSide::both(&[Field::Rst, Field::St]),
        optional: &[Field::St],
        sides: Sides::UsAndCanada,
        callers: PerSide::both(CallSpec::contest(Countries::Pota)),
        answer: "{his}",
        their_exchange: PerSide::both("UR {rst} {rst} {st} {st} BK"),
        // Clockwise from the top right, so the shape reads as a map: each
        // way of starting with report and state twice down the right, then
        // back up the left with each once, so that each row across has one
        // way of starting, naming you at the top and TU alone at the
        // bottom, the right side twice and the left once.
        their_replies: &[
            Reply {
                lead: "BK {my} TU",
                exchange: "UR {rst} {rst} {st} {st} BK",
            },
            Reply {
                lead: "BK TU",
                exchange: "UR {rst} {rst} {st} {st} BK",
            },
            Reply {
                lead: "R TU",
                exchange: "UR {rst} {rst} {st} {st} BK",
            },
            Reply {
                lead: "TU",
                exchange: "UR {rst} {rst} {st} {st} BK",
            },
            Reply {
                lead: "TU",
                exchange: "UR {rst} {st} BK",
            },
            Reply {
                lead: "R TU",
                exchange: "UR {rst} {st} BK",
            },
            Reply {
                lead: "BK TU",
                exchange: "UR {rst} {st} BK",
            },
            Reply {
                lead: "BK {my} TU",
                exchange: "UR {rst} {st} BK",
            },
        ],
        their_lead: &[("BK {my} TU", 1.0)],
        their_query: ["AGN", "AGN"],
        signoff: Some("EE"),
        tu_calls: true,
        tu_call: false,
        serials: None,
        scoring: None,
        labrador: false,
        works: Some(PerSide {
            wve: "you send your report and state, and send back the hunter's state",
            dx: "you send your report alone, and no state back",
        }),
        simplified: Some(
            "Hunters in the US and Canada send their report and state; about one in four is DX and sends a report alone, so leave its state out. Simplified: no park references.",
        ),
    },
];

/// The greeting of the time of day at `hour`, local time, as operators send
/// one: `GM` in the morning, from 4, `GA` in the afternoon, from noon, and
/// `GE` from 6 in the evening on through the night.
pub fn greeting(hour: u32) -> &'static str {
    match hour {
        4..=11 => "GM",
        12..=17 => "GA",
        _ => "GE",
    }
}

/// The hour a greeting goes by where the local time is not known: the
/// afternoon, `GA`, as the greeting most often heard in examples.
pub const GREETING_HOUR: u32 = 12;

/// A whole reply a caller keeps to, where callers vary as you set them: the
/// word it leads its first exchange with, `{my}` your call in it, and its
/// exchange, each field its name in braces, as [`ContestRules::their_exchange`]
/// has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    pub lead: &'static str,
    pub exchange: &'static str,
}

/// Where the dot stands until you move it: up and right of the middle,
/// toward the first reply and the second, so most hunters send those, as
/// heard on the air, and every reply comes: of POTA's eight, about 34, 25,
/// 9, 4, 3, 3, 6 and 16 in a hundred, half of them naming you and about
/// one in four sending each field once.
pub const STYLE_ON_AIR: [f64; 2] = [0.3, -0.47];

/// The middle of the shape: every reply as often as the others.
pub const STYLE_EVEN: [f64; 2] = [0.0, 0.0];

/// The corners of a regular shape of `corners` corners, its middle at
/// nothing and each corner one from it, standing on an edge with an edge
/// at the top: the first just right of the top and the rest clockwise, as
/// a screen draws it, down being positive. Of an even number, each corner
/// on the right has one level with it on the left, the last first.
pub fn shape_corners(corners: usize) -> Vec<[f64; 2]> {
    (0..corners)
        .map(|at| {
            let angle = std::f64::consts::TAU * (at as f64 + 0.5) / corners as f64;
            [angle.sin(), -angle.cos()]
        })
        .collect()
}

/// `at` brought inside the shape of `corners` corners: as it is where it
/// is inside, or the nearest point of the shape's edge where it is not. A
/// point that is no point, not a number, is the middle.
pub fn into_shape(corners: usize, at: [f64; 2]) -> [f64; 2] {
    if !at.iter().all(|value| value.is_finite()) {
        return STYLE_EVEN;
    }
    let shape = shape_corners(corners);
    let edges = || (0..corners).map(|i| (shape[i], shape[(i + 1) % corners]));
    // Inside, or on the edge to within what arithmetic leaves of a point
    // brought to it.
    if edges().all(|(a, b)| cross(a, b, at) >= -1e-12) {
        return at;
    }
    edges()
        .map(|(a, b)| nearest_on_edge(a, b, at))
        .min_by(|p, q| distance2(*p, at).total_cmp(&distance2(*q, at)))
        .unwrap_or(STYLE_EVEN)
}

/// How much of the dot at `at` each corner of the shape of `corners`
/// corners has: its Wachspress coordinates, which make one together, all
/// alike in the middle, all of a corner at it, and along an edge shared by
/// its two corners alone, by how near each is. A dot outside is taken to
/// the shape's edge first ([`into_shape`]).
pub fn corner_shares(corners: usize, at: [f64; 2]) -> Vec<f64> {
    let shape = shape_corners(corners);
    let at = into_shape(corners, at);
    let area = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| cross(a, b, c).abs() / 2.0;
    // On an edge, or at a corner: the edge's two corners share it.
    for i in 0..corners {
        let (a, b) = (shape[i], shape[(i + 1) % corners]);
        if area(at, a, b) < 1e-9 {
            let length = distance2(a, b);
            let along = ((at[0] - a[0]) * (b[0] - a[0]) + (at[1] - a[1]) * (b[1] - a[1])) / length;
            let along = along.clamp(0.0, 1.0);
            let mut shares = vec![0.0; corners];
            shares[i] = 1.0 - along;
            shares[(i + 1) % corners] += along;
            return shares;
        }
    }
    let weights: Vec<f64> = (0..corners)
        .map(|i| {
            let before = shape[(i + corners - 1) % corners];
            let after = shape[(i + 1) % corners];
            area(before, shape[i], after) / (area(at, before, shape[i]) * area(at, shape[i], after))
        })
        .collect();
    let sum: f64 = weights.iter().sum();
    weights.iter().map(|weight| weight / sum).collect()
}

/// Twice the area of the triangle `a`, `b`, `c`, signed: more than nothing
/// where `c` is right of the way from `a` to `b` on a screen, down being
/// positive, as each corner of the shape follows the one before.
fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])
}

fn distance2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

/// The point of the edge from `a` to `b` nearest `at`.
fn nearest_on_edge(a: [f64; 2], b: [f64; 2], at: [f64; 2]) -> [f64; 2] {
    let along = ((at[0] - a[0]) * (b[0] - a[0]) + (at[1] - a[1]) * (b[1] - a[1])) / distance2(a, b);
    let along = along.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * along, a[1] + (b[1] - a[1]) * along]
}

/// `shares` with the corners `off` taken out: each of theirs nothing, and
/// the rest scaled up alike to make one again, so they keep their
/// proportions to one another; in the middle, where all were alike, the
/// rest are alike too. Where nothing is left to share, the dot being at a
/// corner turned off, the rest share alike. With every corner off, or
/// none, the shares are as they were: one at least is always on.
pub fn shares_without(shares: &[f64], off: &[usize]) -> Vec<f64> {
    let on = |at: usize| !off.contains(&at);
    if off.is_empty() || !(0..shares.len()).any(on) {
        return shares.to_vec();
    }
    let kept: Vec<f64> = shares
        .iter()
        .enumerate()
        .map(|(at, share)| if on(at) { *share } else { 0.0 })
        .collect();
    let sum: f64 = kept.iter().sum();
    if sum > 1e-12 {
        return kept.iter().map(|share| share / sum).collect();
    }
    let left = (0..shares.len()).filter(|at| on(*at)).count() as f64;
    (0..shares.len())
        .map(|at| if on(at) { 1.0 / left } else { 0.0 })
        .collect()
}

/// `off` with `corner` turned the other way, of `corners` in all: on again
/// if it was off, off if it was on, unless it is the last on, which stays.
/// Kept in order, each once.
pub fn toggle_corner(off: &[usize], corner: usize, corners: usize) -> Vec<usize> {
    let mut off: Vec<usize> = off.iter().copied().filter(|at| *at < corners).collect();
    off.sort_unstable();
    off.dedup();
    if let Some(at) = off.iter().position(|held| *held == corner) {
        off.remove(at);
    } else if corner < corners && off.len() + 1 < corners {
        off.push(corner);
        off.sort_unstable();
    }
    off
}

/// The replies a contest's callers send, as the card shows them round the
/// shape you place the dot in: each first exchange whole, as sent to you,
/// with the share of callers that send it, in the order of the corners;
/// and each apart, as its corner names it: the words it starts with and
/// the exchange after them.
#[derive(Clone, Debug, PartialEq)]
pub struct CallerStyles {
    pub replies: Vec<(String, f64)>,
    /// The words each reply starts with, your call in them.
    pub leads: Vec<String>,
    /// Each reply's exchange after its first words.
    pub exchanges: Vec<String>,
}

/// How you sign off in a TU that holds a farewell: `73`, and now and then
/// `73 44`, as often as [`FAREWELL_44`] has it.
pub const FAREWELLS: [&str; 2] = ["73", "73 44"];

/// How often your farewell is the second of [`FAREWELLS`], `73 44`.
pub(super) const FAREWELL_44: f64 = 0.25;

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

    /// The side a station signing `call` is on, as this contest draws the
    /// line ([`Sides`]).
    pub fn side_of(&self, call: &str) -> Side {
        self.sides.of_call(call)
    }

    /// Whether a caller may leave `field` out of its exchange
    /// ([`Self::optional`]).
    pub fn optional(&self, field: Field) -> bool {
        self.optional.contains(&field)
    }

    /// Whether your side changes anything, so a run's log has one to keep.
    pub fn sided(&self) -> bool {
        self.exchange.wve != self.exchange.dx
            || self.tu.wve != self.tu.dx
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
        self.tu.wve.contains("{my}") && self.tu.dx.contains("{my}")
    }

    /// Whether your TU signs with your call as you choose ([`TuCall`]): a
    /// TU that brings the next caller and does not sign by the contest's
    /// own rules. The Sprint's always signs, Simple QSO's greets the
    /// station by name and brings nobody, and POTA's ends with `EE`, the
    /// hunter's to answer ([`Self::tu_call`]).
    pub fn tu_call_chosen(&self) -> bool {
        self.tu_call && self.tu_calls && !self.tu_signs()
    }

    /// Whether the TU that closes your `number`th QSO adds your call, as
    /// [`Fill::tu_signed`] has it: as often as `choice` says, where the
    /// rules leave it to you, with one caller at a time as in a pile-up.
    pub fn tu_adds_call(&self, choice: TuCall, number: u32) -> bool {
        self.tu_call_chosen() && choice.signs(number)
    }

    /// Whether callers send the report prefilled, never checked: a report
    /// is part of the exchange, and you do not copy it.
    pub(super) fn prefills_report(&self, side: Side) -> bool {
        self.rst && !self.copied(side).contains(&Field::Rst)
    }

    /// A long CQ from `his_call`, a station on your frequency that is not
    /// calling you.
    pub(super) fn long_cq_text(&self, his_call: &str) -> String {
        squeeze(&self.long_cq.replace("{his}", his_call))
    }

    /// Your CQ, as sent.
    pub fn cq_text(&self, me: &Fill<'_>) -> String {
        self.fill(self.cq, me)
    }

    /// Your TU on `side`, as sent: signed with your call after it where `me`
    /// says so ([`Self::tu_adds_call`]).
    pub fn tu_text(&self, side: Side, me: &Fill<'_>) -> String {
        let tu = self.fill(self.tu.on(side), me);
        if me.tu_signed && !me.my_call.is_empty() {
            format!("{tu} {}", me.my_call)
        } else {
            tu
        }
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

    /// How callers with real habits send the numbers you hear from them on
    /// `side`, in a learner's words: the letters their digits are cut to,
    /// an example of each number you copy cut, and what comes now and then
    /// (`T for 0, N for 9 (TT7 for 7), now and then O for 0 and ENN for
    /// 5NN`). None where real habits cut nothing you hear, as in Field Day
    /// and the Sprint, whose serials go plain (§11.6 of the design).
    pub fn real_numbers(&self, side: Side) -> Option<String> {
        let copied = self.copied(side);
        let serial = self.serial_cut && copied.contains(&Field::Nr);
        let zone = copied.contains(&Field::Zn);
        let power = copied.contains(&Field::Pwr);
        let report = copied.contains(&Field::Rst);
        let letters: Vec<&str> = [
            (serial || zone || power, "T for 0"),
            (zone || power, "A for 1"),
            (serial || zone || power || report, "N for 9"),
        ]
        .into_iter()
        .filter_map(|(cut, letter)| cut.then_some(letter))
        .collect();
        let examples: Vec<&str> = copied
            .iter()
            .filter_map(|field| match field {
                Field::Nr if serial => Some("TT7 for 7"),
                Field::Zn => Some("A4 for 14"),
                Field::Pwr => Some("ATT for 100"),
                Field::Rst => Some("57N for 579"),
                _ => None,
            })
            .collect();
        let enn = if report { "ENN for 599" } else { "ENN for 5NN" };
        let now_and_then: Vec<&str> = [(serial || power, "O for 0"), (self.rst, enn)]
            .into_iter()
            .filter_map(|(sent, form)| sent.then_some(form))
            .collect();
        if letters.is_empty() && now_and_then.is_empty() {
            return None;
        }
        let mut said = letters.join(", ");
        if !examples.is_empty() {
            said = format!("{said} ({})", examples.join(", "));
        }
        if !now_and_then.is_empty() {
            let sep = if said.is_empty() { "" } else { ", " };
            said = format!("{said}{sep}now and then {}", now_and_then.join(" and "));
        }
        Some(said)
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
        self.their_reply_text(side, None, my_call, his_call, fields)
    }

    /// The same, as the `reply`th of [`Self::their_replies`] sends it where
    /// a caller keeps to one.
    pub(super) fn their_reply_text(
        &self,
        side: Side,
        reply: Option<usize>,
        my_call: &str,
        his_call: &str,
        fields: &[(Field, String)],
    ) -> String {
        let template = reply
            .and_then(|reply| self.their_replies.get(reply))
            .map_or_else(|| self.their_exchange.on(side), |reply| reply.exchange);
        let report = fields
            .iter()
            .find(|(field, _)| *field == Field::Rst)
            .map_or(self.report, |(_, token)| token.as_str());
        let mut text = template
            .replace("{my}", my_call)
            .replace("{his}", his_call)
            .replace("{rst}", report);
        for (field, token) in fields {
            text = text.replace(&format!("{{{}}}", field.name()), token);
        }
        // A field it does not send, as a DX hunter's state, leaves no gap.
        for field in self.copied(side) {
            text = text.replace(&format!("{{{}}}", field.name()), "");
        }
        squeeze(&text)
    }

    /// A caller's first exchange to you, `my_call`, `body`, after the word
    /// the contest leads it with, the `lead`th of [`Self::their_lead`];
    /// alone in a contest that leads it with none.
    pub(super) fn their_first_exchange(&self, lead: usize, my_call: &str, body: &str) -> String {
        self.their_first_exchange_led(self.their_lead[lead].0, my_call, body)
    }

    /// The same, led by `lead`, a reply's own ([`Reply::lead`]).
    pub(super) fn their_first_exchange_led(&self, lead: &str, my_call: &str, body: &str) -> String {
        let lead = lead.replace("{my}", my_call);
        squeeze(&format!("{lead} {body}"))
    }

    /// Which of [`Self::their_lead`] a caller leads with, for `draw`, drawn
    /// evenly from nothing up to one: each takes its share of the range, in
    /// order, so the first is the one drawn below its share.
    pub(super) fn lead_at(&self, draw: f64) -> usize {
        let mut upto = 0.0;
        for (at, (_, share)) in self.their_lead.iter().enumerate() {
            upto += share;
            if draw < upto {
                return at;
            }
        }
        self.their_lead.len() - 1
    }

    /// Whether its callers keep to whole replies that vary as you set them
    /// ([`Self::their_replies`]), as POTA's hunters do.
    pub fn varies(&self) -> bool {
        !self.their_replies.is_empty()
    }

    /// The share of callers that keep to each of [`Self::their_replies`],
    /// the dot standing `at` in the shape whose corners they are
    /// ([`corner_shares`]), the replies `off` sent by none
    /// ([`shares_without`]). Empty where callers do not vary.
    pub fn reply_shares(&self, at: [f64; 2], off: &[usize]) -> Vec<f64> {
        if !self.varies() {
            return Vec::new();
        }
        shares_without(&corner_shares(self.their_replies.len(), at), off)
    }

    /// Which of [`Self::their_replies`] a caller keeps to, the dot standing
    /// `at` and the replies `off` sent by none, for `draw`, drawn evenly
    /// from nothing up to one; none where callers do not vary.
    pub(super) fn reply_at(&self, draw: f64, at: [f64; 2], off: &[usize]) -> Option<usize> {
        let shares = self.reply_shares(at, off);
        let mut upto = 0.0;
        for (reply, share) in shares.iter().enumerate() {
            upto += share;
            if draw < upto {
                return Some(reply);
            }
        }
        shares.len().checked_sub(1)
    }

    /// The replies its callers send, the dot standing `at` and the replies
    /// `off` sent by none, as the card lists them, to you as `my_call` on
    /// `side`: each whole, from a caller in Oregon with the report and state
    /// the card's QSO has, with its share, in the order of the corners. None
    /// where they do not vary.
    pub fn caller_styles(
        &self,
        side: Side,
        my_call: &str,
        at: [f64; 2],
        off: &[usize],
    ) -> Option<CallerStyles> {
        if !self.varies() {
            return None;
        }
        let fields: Vec<(Field, String)> = self
            .copied(side)
            .iter()
            .map(|field| (*field, field.example().to_string()))
            .collect();
        let exchanges: Vec<String> = (0..self.their_replies.len())
            .map(|at| self.their_reply_text(side, Some(at), my_call, "W7XYZ", &fields))
            .collect();
        let leads: Vec<String> = self
            .their_replies
            .iter()
            .map(|reply| self.their_first_exchange_led(reply.lead, my_call, ""))
            .collect();
        let replies = leads
            .iter()
            .zip(&exchanges)
            .zip(self.reply_shares(at, off))
            .map(|((lead, body), share)| (squeeze(&format!("{lead} {body}")), share))
            .collect();
        Some(CallerStyles {
            replies,
            leads,
            exchanges,
        })
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
        let his_state = if self.copied(side).contains(&Field::St) {
            Field::St.example()
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
                    me.my_call,
                    &self.their_exchange_text(side, me.my_call, caller, &fields),
                ),
            ),
            (
                Sender::You,
                self.tu_text(
                    side,
                    &Fill {
                        his_name,
                        his_state,
                        ..first
                    },
                ),
            ),
        ];
        qso.extend(self.signoff.map(|signoff| (them, signoff.to_string())));
        qso
    }

    /// `template`, one of your texts, filled in with `me`: your call, name,
    /// state, zone, power, class, section, report, serial, greeting and
    /// farewell, and the name and state of the station you work. A name not there leaves
    /// no gap, and neither does DX for its state, which names no place to
    /// send back. Your zone and power go out cut, as MorseRunner sends
    /// yours.
    fn fill(&self, template: &str, me: &Fill<'_>) -> String {
        let his_state = if me.his_state == DX { "" } else { me.his_state };
        let farewell = if me.farewell.is_empty() {
            FAREWELLS[0]
        } else {
            me.farewell
        };
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
                .replace("{hisstate}", his_state)
                .replace("{greet}", me.greeting)
                .replace("{bye}", farewell)
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

    fn pota() -> &'static ContestRules {
        ContestKind::Pota.rules()
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
            his_state: "",
            greeting: "GA",
            farewell: "",
            tu_signed: false,
        }
    }

    /// You, as K1ABC called MIKE in Massachusetts, zone 5, sending
    /// `serial`, working BOB in Oregon in the afternoon.
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
            his_state: "OR",
            greeting: "GA",
            farewell: "",
            tu_signed: false,
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
        assert_eq!(rules.tu_text(Side::Dx, &me(7, "BOB")), "TU");
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
        assert_eq!(rules.tu_text(Side::Dx, &me(0, "BOB")), "TU BOB 73 EE");
        // No name copied: no gap where it would be.
        assert_eq!(rules.tu_text(Side::Dx, &me(0, "")), "TU 73 EE");
        // Its call once, or twice from a caller that calls twice.
        assert_eq!(
            rules.answer_text("DL1ABC", "K5XYZ", false),
            "DL1ABC DE K5XYZ K"
        );
        assert_eq!(
            rules.answer_text("DL1ABC", "K5XYZ", true),
            "DL1ABC DE K5XYZ K5XYZ K"
        );
        let theirs = [
            (Field::Rst, "579".to_string()),
            (Field::Name, "BOB".to_string()),
        ];
        assert_eq!(
            rules.their_exchange_text(Side::Dx, "DL1ABC", "K5XYZ", &theirs),
            "UR 579 579 NAME BOB BOB BK"
        );
        assert_eq!(rules.their_lead, [("R TNX", 1.0)]);
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
        assert_eq!(rules.tu_text(Side::WVe, &k1abc(7)), "TU K1ABC");
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
        assert_eq!(
            rules.their_first_exchange(0, "K1ABC", &sent),
            sent,
            "led by nothing"
        );
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
        assert_eq!(rules.tu_text(Side::Dx, &me(0, "")), "TU");
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
        assert_eq!(rules.tu_text(Side::WVe, &k1), "TU");
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
        assert_eq!(rules.their_first_exchange(0, "K1ABC", &sent), "R 3A OR");
        assert_eq!(rules.their_first_exchange(1, "K1ABC", &sent), "TU 3A OR");
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
    /// POTA on the air, as the exchange cheat sheet has it: `CQ CQ POTA DE
    /// K1ABC K` · `W7XYZ` · `W7XYZ TU GA UR 5NN 5NN MA MA BK` · `BK TU UR 579
    /// 579 OR OR BK` · `BK TU OR OR 73 EE` · `EE`. You greet the hunter by
    /// the time of day and send its state back, and now and then sign off
    /// `73 44`. Outside the US and Canada, Alaska and Hawaii being in them,
    /// you send your report alone and no state back, as DX does: `BK TU 73
    /// EE`. Hunters in the US and Canada send their report and state, and
    /// DX ones their report alone, so the state is optional. Your TU brings
    /// the next hunter, and nothing scores a run.
    #[test]
    fn pota_sends_what_an_activator_sends() {
        let rules = pota();
        assert_eq!(rules.cq_text(&k1abc(0)), "CQ CQ POTA DE K1ABC K");
        assert_eq!(
            rules.exchange_text(Side::WVe, &k1abc(0)),
            "TU GA UR 5NN 5NN MA MA BK"
        );
        let morning = Fill {
            greeting: greeting(9),
            ..k1abc(0)
        };
        assert_eq!(
            rules.exchange_text(Side::WVe, &morning),
            "TU GM UR 5NN 5NN MA MA BK"
        );
        // As DX: your report alone, and no state back, whatever you copied.
        assert_eq!(
            rules.exchange_text(Side::Dx, &me(0, "")),
            "TU GA UR 5NN 5NN BK"
        );
        let copied_or = Fill {
            his_state: "OR",
            ..me(0, "")
        };
        assert_eq!(rules.tu_text(Side::Dx, &copied_or), "BK TU 73 EE");
        // In the US or Canada: the hunter's state back.
        assert_eq!(rules.tu_text(Side::WVe, &k1abc(0)), "BK TU OR OR 73 EE");
        // A DX hunter's, with no state copied, or DX, which names no place:
        // no gap.
        for none in ["", "DX"] {
            let dx = Fill {
                his_state: none,
                ..k1abc(0)
            };
            assert_eq!(rules.tu_text(Side::WVe, &dx), "BK TU 73 EE", "{none:?}");
        }
        // Now and then `73 44`.
        let fond = Fill {
            farewell: FAREWELLS[1],
            ..k1abc(0)
        };
        assert_eq!(rules.tu_text(Side::WVe, &fond), "BK TU OR OR 73 44 EE");
        let fond_dx = Fill {
            farewell: FAREWELLS[1],
            my_call: "SA0KAM",
            ..me(0, "")
        };
        assert_eq!(rules.tu_text(Side::Dx, &fond_dx), "BK TU 73 44 EE");
        // It brings the next hunter, but never signs with your call: it
        // ends with `EE`, for the hunter to answer.
        assert!(rules.tu_calls && !rules.tu_call_chosen());
        for number in 1..=3 {
            assert!(!rules.tu_adds_call(TuCall::Every, number));
        }
        assert_eq!(rules.answer_text("K1ABC", "W7XYZ", false), "W7XYZ");
        assert_eq!(rules.answer_text("K1ABC", "W7XYZ", true), "W7XYZ W7XYZ");
        let theirs = [
            (Field::Rst, "57N".to_string()),
            (Field::St, "FL".to_string()),
        ];
        let body = rules.their_exchange_text(Side::WVe, "K1ABC", "W4XYZ", &theirs);
        assert_eq!(body, "UR 57N 57N FL FL BK");
        // The card's QSO names you after its BK.
        assert_eq!(
            rules.their_first_exchange(0, "K1ABC", &body),
            "BK K1ABC TU UR 57N 57N FL FL BK"
        );
        // Each hunter keeps to one of eight whole replies, a corner each.
        let replies: Vec<String> = (0..8)
            .map(|reply| {
                let body =
                    rules.their_reply_text(Side::WVe, Some(reply), "K1ABC", "W4XYZ", &theirs);
                rules.their_first_exchange_led(rules.their_replies[reply].lead, "K1ABC", &body)
            })
            .collect();
        assert_eq!(
            replies,
            [
                "BK K1ABC TU UR 57N 57N FL FL BK",
                "BK TU UR 57N 57N FL FL BK",
                "R TU UR 57N 57N FL FL BK",
                "TU UR 57N 57N FL FL BK",
                "TU UR 57N FL BK",
                "R TU UR 57N FL BK",
                "BK TU UR 57N FL BK",
                "BK K1ABC TU UR 57N FL BK"
            ]
        );
        assert!(rules.varies() && !wpx().varies());
        // A DX hunter's: its report alone, and no gap where a state would be.
        let dx_theirs = [(Field::Rst, "57N".to_string())];
        assert_eq!(
            rules.their_exchange_text(Side::Dx, "SA0KAM", "DL1XYZ", &dx_theirs),
            "UR 57N 57N BK"
        );
        assert_eq!(
            rules.their_reply_text(Side::Dx, Some(4), "SA0KAM", "DL1XYZ", &dx_theirs),
            "UR 57N BK"
        );
        assert_eq!(rules.signoff, Some("EE"));
        assert_eq!(rules.their_query, ["AGN", "AGN"], "no serial to ask for");
        assert_eq!(rules.query, "AGN");
        assert_eq!(rules.sends(Side::WVe), "5NN + state");
        assert_eq!(rules.sends(Side::Dx), "5NN");
        assert!(rules.sends_state(Side::WVe) && !rules.sends_state(Side::Dx));
        assert!(!rules.sends_name());
        assert!(!rules.prefills_report(Side::Dx), "the report is copied");
        for side in [Side::WVe, Side::Dx] {
            assert_eq!(rules.copied(side), [Field::Rst, Field::St]);
            assert_eq!(rules.callers(side), CallSpec::contest(Countries::Pota));
        }
        assert!(rules.optional(Field::St) && !rules.optional(Field::Rst));
        // The line between the sides is the US and Canada's, Alaska and
        // Hawaii among them.
        for (call, side) in [
            ("K1ABC", Side::WVe),
            ("VE3ABC", Side::WVe),
            ("KL7XYZ", Side::WVe),
            ("KH6XYZ", Side::WVe),
            ("SA0KAM", Side::Dx),
            ("SO5KM", Side::Dx),
            ("KP4AB", Side::Dx),
            ("", Side::Dx),
        ] {
            assert_eq!(rules.side_of(call), side, "{call:?}");
        }
        assert_eq!(
            wpx().side_of("KL7XYZ"),
            Side::Dx,
            "ARRL DX's line elsewhere"
        );
        assert!(rules.sided());
        assert_eq!(
            rules.side_note(Side::Dx, true).as_deref(),
            Some("You are DX: you send your report alone, and no state back")
        );
        assert_eq!(
            rules.side_note(Side::WVe, true).as_deref(),
            Some("You are W/VE: you send your report and state, and send back the hunter's state")
        );
        assert_eq!(rules.scoring, None);
        assert!(rules.serials.is_none() && !rules.serial);
        assert!(
            rules
                .long_cq_text("N4XYZ")
                .starts_with("CQ CQ POTA DE N4XYZ")
        );
    }

    /// The dot's place in the shape shares the callers among its corners:
    /// all alike in the middle, all to a corner at it, the two corners of
    /// an edge alone along it, and the shares always make one. A dot
    /// outside is brought to the edge.
    #[test]
    fn the_dot_shares_the_replies_among_the_corners() {
        let corners = shape_corners(5);
        assert!(corners[0][0] > 0.5 && corners[0][1] < -0.8, "{corners:?}");
        assert!(corners[1][0] > 0.9, "clockwise on a screen: {corners:?}");
        // An edge at the top, the first corner and the last level across it.
        assert!((corners[0][0] + corners[4][0]).abs() < 1e-12);
        assert!((corners[0][1] - corners[4][1]).abs() < 1e-12);
        // Of eight, each on the right has one level with it on the left.
        let eight = shape_corners(8);
        for at in 0..4 {
            let (right, left) = (eight[at], eight[7 - at]);
            assert!(
                right[0] > 0.3 && (right[0] + left[0]).abs() < 1e-12,
                "{eight:?}"
            );
            assert!((right[1] - left[1]).abs() < 1e-12, "{eight:?}");
        }
        for share in corner_shares(5, STYLE_EVEN) {
            assert!((share - 0.2).abs() < 1e-12, "{share}");
        }
        for (at, corner) in corners.iter().enumerate() {
            let shares = corner_shares(5, *corner);
            for (other, share) in shares.iter().enumerate() {
                let all = if other == at { 1.0 } else { 0.0 };
                assert!((share - all).abs() < 1e-9, "{at} {other}: {share}");
            }
        }
        // Halfway along the first edge: its two corners, half each.
        let half = [
            (corners[0][0] + corners[1][0]) / 2.0,
            (corners[0][1] + corners[1][1]) / 2.0,
        ];
        let shares = corner_shares(5, half);
        assert!((shares[0] - 0.5).abs() < 1e-9 && (shares[1] - 0.5).abs() < 1e-9);
        assert!(
            shares[2..].iter().all(|share| share.abs() < 1e-9),
            "{shares:?}"
        );
        // As heard on the air, among POTA's eight: every reply, the first
        // the commonest, half naming you, about one in four each field once.
        let on_air = corner_shares(8, STYLE_ON_AIR);
        assert!(on_air.iter().all(|share| *share > 0.02), "{on_air:?}");
        assert!(on_air[0] > 0.3 && on_air[0] > on_air[1], "{on_air:?}");
        assert!(
            (0.45..0.55).contains(&(on_air[0] + on_air[7])),
            "{on_air:?}"
        );
        let once: f64 = on_air[4..].iter().sum();
        assert!((0.2..0.35).contains(&once), "{on_air:?}");
        // Anywhere, they make one and none is below nothing; and a dot
        // outside comes to the edge, a point that is none to the middle.
        for x in -12..=12 {
            for y in -12..=12 {
                let at = [f64::from(x) / 10.0, f64::from(y) / 10.0];
                let shares = corner_shares(5, at);
                let sum: f64 = shares.iter().sum();
                assert!((sum - 1.0).abs() < 1e-9, "{at:?}: {sum}");
                assert!(
                    shares.iter().all(|share| *share >= 0.0),
                    "{at:?}: {shares:?}"
                );
                let inside = into_shape(5, at);
                assert_eq!(into_shape(5, inside), inside, "{at:?}");
            }
        }
        assert_eq!(into_shape(5, [0.1, 0.2]), [0.1, 0.2]);
        // Straight up, the top edge.
        let far = into_shape(5, [0.0, -9.0]);
        assert!(
            (far[0]).abs() < 1e-9 && (far[1] - corners[0][1]).abs() < 1e-9,
            "{far:?}"
        );
        assert_eq!(into_shape(5, [f64::NAN, 0.0]), STYLE_EVEN);
        // Callers that do not vary have none to share.
        assert!(wpx().reply_shares(STYLE_ON_AIR, &[]).is_empty());
        assert_eq!(wpx().reply_at(0.5, STYLE_ON_AIR, &[]), None);
        assert_eq!(pota().reply_at(0.0, STYLE_ON_AIR, &[]), Some(0));
        assert_eq!(pota().reply_at(0.999_999, STYLE_ON_AIR, &[]), Some(7));
        // A reply turned off is drawn by none.
        assert_eq!(pota().reply_at(0.0, STYLE_ON_AIR, &[0]), Some(1));
        assert_eq!(pota().reply_at(0.999_999, STYLE_ON_AIR, &[7]), Some(6));
    }

    /// A reply turned off is sent by none: the rest are scaled up alike, so
    /// they keep their proportions to one another; where none is left, the
    /// dot at its corner, the rest share alike; and the last reply on stays
    /// on.
    #[test]
    fn a_reply_turned_off_is_sent_by_none() {
        let on_air = corner_shares(8, STYLE_ON_AIR);
        let without = shares_without(&on_air, &[0]);
        assert_eq!(without[0], 0.0);
        let sum: f64 = without.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9);
        for at in 1..8 {
            assert!((without[at] - on_air[at] / (1.0 - on_air[0])).abs() < 1e-9);
        }
        let at_corner = corner_shares(8, shape_corners(8)[0]);
        let alike = shares_without(&at_corner, &[0, 4]);
        for (at, share) in alike.iter().enumerate() {
            let want = if [0, 4].contains(&at) { 0.0 } else { 1.0 / 6.0 };
            assert!((share - want).abs() < 1e-9, "{at}: {share}");
        }
        let all: Vec<usize> = (0..8).collect();
        assert_eq!(shares_without(&on_air, &all), on_air);
        // Turned off and on again; never the last one on.
        assert_eq!(toggle_corner(&[], 2, 8), [2]);
        assert_eq!(toggle_corner(&[2], 2, 8), Vec::<usize>::new());
        assert_eq!(toggle_corner(&[5, 2], 0, 8), [0, 2, 5]);
        let seven: Vec<usize> = (1..8).collect();
        assert_eq!(toggle_corner(&seven, 0, 8), seven, "the last one on stays");
        assert_eq!(toggle_corner(&[9, 3, 3], 1, 8), [1, 3]);
        let styles = pota()
            .caller_styles(Side::Dx, "SA0KAM", STYLE_ON_AIR, &[1, 2])
            .expect("POTA varies");
        assert_eq!(styles.replies[1].1, 0.0);
        assert_eq!(styles.replies[2].1, 0.0);
    }

    /// The card lists each reply a POTA hunter sends, whole, as sent to
    /// you, with its share, in the order of the corners: one reply at a
    /// corner, eight alike in the middle.
    #[test]
    fn the_card_shows_each_reply_with_its_share() {
        let replies = |at| {
            pota()
                .caller_styles(Side::Dx, "SA0KAM", at, &[])
                .expect("POTA's hunters vary")
                .replies
        };
        let on_air = replies(STYLE_ON_AIR);
        let texts: Vec<&str> = on_air.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "BK SA0KAM TU UR 579 579 OR OR BK",
                "BK TU UR 579 579 OR OR BK",
                "R TU UR 579 579 OR OR BK",
                "TU UR 579 579 OR OR BK",
                "TU UR 579 OR BK",
                "R TU UR 579 OR BK",
                "BK TU UR 579 OR BK",
                "BK SA0KAM TU UR 579 OR BK"
            ]
        );
        let sum: f64 = on_air.iter().map(|(_, share)| share).sum();
        assert!((sum - 1.0).abs() < 1e-9, "{sum}");
        // Each corner names its reply apart: the words it starts with, and
        // the exchange after them.
        let styles = pota()
            .caller_styles(Side::Dx, "SA0KAM", STYLE_ON_AIR, &[])
            .expect("POTA's hunters vary");
        assert_eq!(
            styles.leads,
            [
                "BK SA0KAM TU",
                "BK TU",
                "R TU",
                "TU",
                "TU",
                "R TU",
                "BK TU",
                "BK SA0KAM TU"
            ]
        );
        assert_eq!(styles.exchanges[0], "UR 579 579 OR OR BK");
        assert_eq!(styles.exchanges[7], "UR 579 OR BK");
        let one = replies(shape_corners(8)[0]);
        assert!((one[0].1 - 1.0).abs() < 1e-9);
        for (_, share) in replies(STYLE_EVEN) {
            assert!((share - 0.125).abs() < 1e-12);
        }
        assert!(
            wpx()
                .caller_styles(Side::Dx, "DL1ABC", STYLE_ON_AIR, &[])
                .is_none()
        );
    }

    /// The greeting goes by the hour: good morning from four, good
    /// afternoon from noon, good evening from six on through the night.
    #[test]
    fn the_greeting_goes_by_the_time_of_day() {
        let greetings: Vec<&str> = (0..24).map(greeting).collect();
        let mut expected = vec!["GE"; 4];
        expected.extend(["GM"; 8]);
        expected.extend(["GA"; 6]);
        expected.extend(["GE"; 6]);
        assert_eq!(greetings, expected);
        assert_eq!(greeting(GREETING_HOUR), "GA");
    }

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
                (they, "DL1ABC DE K5XYZ K".to_string()),
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
        // A POTA hunter is where the state it sends is, and your TU sends
        // it back; it signs off after.
        assert_eq!(
            pota().example_qso(Side::WVe, &k1abc(0)),
            [
                (you, "CQ CQ POTA DE K1ABC K".to_string()),
                (they, "W7XYZ".to_string()),
                (you, "W7XYZ TU GA UR 5NN 5NN MA MA BK".to_string()),
                (they, "BK K1ABC TU UR 579 579 OR OR BK".to_string()),
                (you, "BK TU OR OR 73 EE".to_string()),
                (they, "EE".to_string()),
            ]
        );
        // From outside the US and Canada: your report alone, and no state
        // back.
        assert_eq!(
            pota().example_qso(Side::Dx, &me(0, ""))[2..5],
            [
                (you, "W7XYZ TU GA UR 5NN 5NN BK".to_string()),
                (they, "BK DL1ABC TU UR 579 579 OR OR BK".to_string()),
                (you, "BK TU 73 EE".to_string()),
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

    /// Your call in TU is yours to choose in every contest whose TU works
    /// as a CQ and does not sign by its own rules: not the Sprint's, which
    /// always signs, nor Simple QSO's, which brings nobody, nor POTA's,
    /// which ends with `EE`. Chosen, your TU
    /// adds it as often as you choose, and the TU sent, the example's too,
    /// reads `TU K1ABC`.
    #[test]
    fn your_call_in_tu_is_chosen_where_the_rules_leave_it() {
        for kind in ContestKind::ALL {
            let chosen = !matches!(
                kind,
                ContestKind::Sprint | ContestKind::SimpleQso | ContestKind::Pota
            );
            let rules = kind.rules();
            assert_eq!(rules.tu_call_chosen(), chosen, "{kind:?}");
            for number in 1..=3 {
                assert_eq!(
                    rules.tu_adds_call(TuCall::Every, number),
                    chosen,
                    "{kind:?}"
                );
                assert!(!rules.tu_adds_call(TuCall::Never, number));
            }
        }
        let rules = wpx();
        assert!(!rules.tu_adds_call(TuCall::Third, 2));
        assert!(rules.tu_adds_call(TuCall::Third, 3));
        let signed = Fill {
            tu_signed: true,
            ..k1abc(7)
        };
        assert_eq!(rules.tu_text(Side::WVe, &signed), "TU K1ABC");
        assert_eq!(rules.example_qso(Side::WVe, &signed)[4].1, "TU K1ABC");
        let nameless = Fill {
            my_call: "",
            ..signed
        };
        assert_eq!(
            rules.tu_text(Side::WVe, &nameless),
            "TU",
            "no call to sign with"
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
                // Your TU greets them by a name only you copy, and sends
                // back a state only you copy.
                let tu = rules.tu.on(side);
                assert!(
                    !tu.contains("{hisname}") || rules.copied(side).contains(&Field::Name),
                    "{:?}",
                    rules.kind
                );
                assert!(
                    !tu.contains("{hisstate}") || rules.copied(side).contains(&Field::St),
                    "{:?}",
                    rules.kind
                );
                // A field may be left out only where it is copied.
                for field in rules.optional {
                    assert!(rules.copied(side).contains(field), "{:?}", rules.kind);
                }
                // Every reply a caller keeps to sends what the exchange does.
                for reply in rules.their_replies {
                    for field in rules.copied(side) {
                        let held = format!("{{{}}}", field.name());
                        assert!(reply.exchange.contains(&held), "{:?}", rules.kind);
                    }
                    assert_eq!(
                        reply.exchange.contains("{rst}"),
                        rules.rst,
                        "{:?}",
                        rules.kind
                    );
                }
            }
            assert!(rules.cq.contains("{my}"), "{:?}", rules.kind);
            assert!(rules.answer.contains("{his}"), "{:?}", rules.kind);
            // A long CQ is someone else's: its own call, twice, and no
            // call of yours.
            assert_eq!(
                rules.long_cq.matches("{his}").count(),
                2,
                "{:?}",
                rules.kind
            );
            assert!(!rules.long_cq.contains("{my}"), "{:?}", rules.kind);
            // The leads' shares make one.
            let shares: f64 = rules.their_lead.iter().map(|(_, share)| share).sum();
            assert!((shares - 1.0).abs() < 1e-9, "{:?}: {shares}", rules.kind);
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

    /// What the Band card says callers with real habits cut follows the
    /// numbers you copy: a serial where serials go cut, a zone, a power, a
    /// report, and `ENN` wherever a report goes; nothing in Field Day or
    /// the Sprint, whose serials go plain. Each example is a form those
    /// callers send.
    #[test]
    fn real_habits_are_said_by_the_numbers_you_copy() {
        use crate::contest::CallerHabits::Real;
        use crate::contest::exchange::{send_power, send_report, send_serial, send_zone};
        use crate::rng::SplitMix64;
        let said = |kind: ContestKind, side| kind.rules().real_numbers(side);
        assert_eq!(
            said(ContestKind::Wpx, Side::Dx).as_deref(),
            Some("T for 0, N for 9 (TT7 for 7), now and then O for 0 and ENN for 5NN")
        );
        assert_eq!(
            said(ContestKind::Cqww, Side::Dx).as_deref(),
            Some("T for 0, A for 1, N for 9 (A4 for 14), now and then ENN for 5NN")
        );
        assert_eq!(
            said(ContestKind::ArrlDx, Side::WVe).as_deref(),
            Some("T for 0, A for 1, N for 9 (ATT for 100), now and then O for 0 and ENN for 5NN")
        );
        assert_eq!(
            said(ContestKind::ArrlDx, Side::Dx).as_deref(),
            Some("now and then ENN for 5NN")
        );
        for kind in [ContestKind::SimpleQso, ContestKind::Pota] {
            assert_eq!(
                said(kind, Side::Dx).as_deref(),
                Some("N for 9 (57N for 579), now and then ENN for 599"),
                "{kind:?}"
            );
        }
        for side in [Side::WVe, Side::Dx] {
            assert_eq!(said(ContestKind::Sprint, side), None);
            assert_eq!(said(ContestKind::FieldDay, side), None);
        }
        let mut rng = SplitMix64::new(2);
        let mut sent = std::collections::BTreeSet::new();
        for _ in 0..500 {
            sent.insert(send_serial(7, Real, true, &mut rng));
            sent.insert(send_zone(14, Real, &mut rng));
            sent.insert(send_power("100", Real, &mut rng));
            sent.insert(send_report(579, Real, &mut rng));
            sent.insert(send_report(599, Real, &mut rng));
        }
        for form in ["TT7", "A4", "ATT", "57N", "ENN"] {
            assert!(sent.contains(form), "{form} is never sent");
        }
    }
}
