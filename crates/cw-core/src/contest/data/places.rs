//! Where North American stations are: the US state, or the Canadian
//! province or territory, a station's call puts it in, and the codes the
//! contests send for them.
//!
//! A US call's digit is its FCC call district (`W7` is one of the eight
//! western states), and Alaska and Hawaii sign prefixes of their own, KL7
//! and KH6. Within a district a caller's state is drawn by its people, from
//! the 2020 US Census counts, as the shares of licences each holds are not
//! counted yet: the design (§11.2) has them counted from the FCC's licence
//! data in phase 3. A Canadian call's digit is its province, for VE and the
//! other prefixes Canada issues, but VO and VY, whose own digits name
//! Newfoundland and Labrador, the territories and Prince Edward Island; a
//! call-area digit after the slash is a province by VE's digits whatever
//! the prefix. A special-event prefix's digit means what it means
//! under the prefix it stands in for, which can change from one event to
//! the next: CH and CI stood in for VO and VY in 2017, so Dust reads no
//! place from their digits, only that they are Canada's.
//!
//! The call table holds only some of these prefixes, and reads Hawaii's KH
//! and Puerto Rico's KP for every digit, so where a station is at home is
//! read here, from the prefixes the US gives Alaska, Hawaii and its other
//! islands and every prefix Canada issues, and so is the country a contest
//! scores it in ([`home_country`]); and so is the ARRL DX contest's side,
//! W/VE: the 48 contiguous states, DC and Canada, with Alaska, Hawaii and
//! the islands DX.
//!
//! Labrador, VO2, is part of Newfoundland and Labrador, NL, as the NA
//! Sprint counts it; the ARRL DX contest counts it apart, as LB.

use crate::callsign::{CallArea, GUANTANAMO, call_area, country_of, parse_callsign, pick_weighted};
use crate::rng::Rng;

/// What a station outside North America sends for its place.
pub(crate) const DX: &str = "DX";

/// Every place a North American contest sends a code for, with its name:
/// the 50 states and the District of Columbia, then Canada's 13 provinces
/// and territories.
#[rustfmt::skip]
const PLACES: &[(&str, &str)] = &[
    ("AL", "Alabama"), ("AK", "Alaska"), ("AZ", "Arizona"), ("AR", "Arkansas"),
    ("CA", "California"), ("CO", "Colorado"), ("CT", "Connecticut"),
    ("DE", "Delaware"), ("DC", "District of Columbia"), ("FL", "Florida"),
    ("GA", "Georgia"), ("HI", "Hawaii"), ("ID", "Idaho"), ("IL", "Illinois"),
    ("IN", "Indiana"), ("IA", "Iowa"), ("KS", "Kansas"), ("KY", "Kentucky"),
    ("LA", "Louisiana"), ("ME", "Maine"), ("MD", "Maryland"),
    ("MA", "Massachusetts"), ("MI", "Michigan"), ("MN", "Minnesota"),
    ("MS", "Mississippi"), ("MO", "Missouri"), ("MT", "Montana"),
    ("NE", "Nebraska"), ("NV", "Nevada"), ("NH", "New Hampshire"),
    ("NJ", "New Jersey"), ("NM", "New Mexico"), ("NY", "New York"),
    ("NC", "North Carolina"), ("ND", "North Dakota"), ("OH", "Ohio"),
    ("OK", "Oklahoma"), ("OR", "Oregon"), ("PA", "Pennsylvania"),
    ("RI", "Rhode Island"), ("SC", "South Carolina"), ("SD", "South Dakota"),
    ("TN", "Tennessee"), ("TX", "Texas"), ("UT", "Utah"), ("VT", "Vermont"),
    ("VA", "Virginia"), ("WA", "Washington"), ("WV", "West Virginia"),
    ("WI", "Wisconsin"), ("WY", "Wyoming"),
    ("AB", "Alberta"), ("BC", "British Columbia"), ("MB", "Manitoba"),
    ("NB", "New Brunswick"), ("NL", "Newfoundland and Labrador"),
    ("NS", "Nova Scotia"), ("NT", "Northwest Territories"), ("NU", "Nunavut"),
    ("ON", "Ontario"), ("PE", "Prince Edward Island"), ("QC", "Quebec"),
    ("SK", "Saskatchewan"), ("YT", "Yukon"),
];

/// Codes still sent for a place that has a newer one: Quebec was PQ, and
/// Newfoundland NF.
const OLD_CODES: [(&str, &str); 2] = [("PQ", "QC"), ("NF", "NL")];

/// Labrador, which the ARRL DX contest counts apart from Newfoundland.
const LABRADOR: (&str, &str) = ("LB", "Labrador");

/// What Labrador is part of where it is not counted apart.
const LABRADOR_IN: &str = "NL";

/// The places a DX station counts in the ARRL DX contest, each once: the
/// 48 contiguous states and DC, then Canada's 13 provinces and territories
/// and Labrador. Alaska and Hawaii are DX.
#[rustfmt::skip]
const W_VE_PLACES: [&str; 63] = [
    "AL", "AZ", "AR", "CA", "CO", "CT", "DE", "DC", "FL", "GA", "ID", "IL",
    "IN", "IA", "KS", "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO",
    "MT", "NE", "NV", "NH", "NJ", "NM", "NY", "NC", "ND", "OH", "OK", "OR",
    "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV", "WI",
    "WY",
    "NB", "NS", "QC", "ON", "MB", "SK", "AB", "BC", "NT", "NL", "LB", "NU",
    "YT", "PE",
];

/// Each US call district's states, by its digit, with their people in
/// thousands at the 2020 Census: what a caller's state is drawn by within
/// its district.
#[rustfmt::skip]
const DISTRICTS: [&[(&str, f64)]; 10] = [
    &[("CO", 5_774.0), ("IA", 3_190.0), ("KS", 2_938.0), ("MN", 5_706.0),
      ("MO", 6_155.0), ("ND", 779.0), ("NE", 1_962.0), ("SD", 887.0)],
    &[("CT", 3_606.0), ("ME", 1_362.0), ("MA", 7_030.0), ("NH", 1_378.0),
      ("RI", 1_097.0), ("VT", 643.0)],
    &[("NJ", 9_289.0), ("NY", 20_201.0)],
    &[("DE", 990.0), ("DC", 690.0), ("MD", 6_177.0), ("PA", 13_003.0)],
    &[("AL", 5_024.0), ("FL", 21_538.0), ("GA", 10_712.0), ("KY", 4_506.0),
      ("NC", 10_439.0), ("SC", 5_118.0), ("TN", 6_911.0), ("VA", 8_631.0)],
    &[("AR", 3_012.0), ("LA", 4_658.0), ("MS", 2_961.0), ("NM", 2_118.0),
      ("OK", 3_959.0), ("TX", 29_146.0)],
    &[("CA", 39_538.0)],
    &[("AZ", 7_152.0), ("ID", 1_839.0), ("MT", 1_084.0), ("NV", 3_105.0),
      ("OR", 4_237.0), ("UT", 3_272.0), ("WA", 7_705.0), ("WY", 577.0)],
    &[("MI", 10_077.0), ("OH", 11_799.0), ("WV", 1_794.0)],
    &[("IL", 12_813.0), ("IN", 6_786.0), ("WI", 5_894.0)],
];

/// The prefixes Canada issues whose digit is the province: VA, VB, VC, VE
/// and VG; CF, CG, CJ and CK; XJ to XO.
const CANADA: [&str; 15] = [
    "VA", "VB", "VC", "VE", "VG", "CF", "CG", "CJ", "CK", "XJ", "XK", "XL", "XM", "XN", "XO",
];

/// Canada's other prefixes: VO and VY, whose digits name places of their
/// own, and CH and CI, whose digits name none Dust can say.
const CANADA_ELSE: [&str; 4] = ["VO", "VY", "CH", "CI"];

/// The prefixes the US gives Alaska, whatever their digit.
const ALASKA: [&str; 4] = ["AL", "KL", "NL", "WL"];

/// The prefixes the US gives Hawaii with a 6 or a 7; with another digit
/// they are Pacific islands no state holds (KH2 is Guam), which the call
/// table does not hold.
const HAWAII: [&str; 4] = ["AH", "KH", "NH", "WH"];

/// The prefixes the US gives its Caribbean islands, and the country each
/// digit is in, by the call table's keys: Puerto Rico's 3 and 4, the Virgin
/// Islands' 2. The table holds neither Navassa's 1 nor Desecheo's 5.
const CARIBBEAN: [&str; 3] = ["KP", "NP", "WP"];
const CARIBBEAN_DIGITS: [(char, &str); 3] = [('2', "KP2"), ('3', "KP4"), ('4', "KP4")];

/// The province or territory each digit of [`CANADA`]'s prefixes names,
/// and each call-area digit after a Canadian call's slash; 0 names none.
const PROVINCES: [Option<&str>; 10] = [
    None,
    Some("NS"),
    Some("QC"),
    Some("ON"),
    Some("MB"),
    Some("SK"),
    Some("AB"),
    Some("BC"),
    Some("NT"),
    Some("NB"),
];

/// VY's digits: Nunavut, Yukon, Prince Edward Island.
const VY: [&str; 3] = ["NU", "YT", "PE"];

/// `token` as the code a North American contest sends for a place: one of
/// [`PLACES`], an older code read as its newer one (PQ is QC, NF is NL),
/// [`LABRADOR`], or [`DX`]. None for anything else. Labrador is read as
/// itself; a contest that does not count it apart reads it as NL
/// ([`fold_labrador`]).
pub(crate) fn place_code(token: &str) -> Option<&'static str> {
    let token = OLD_CODES
        .iter()
        .find(|(old, _)| *old == token)
        .map_or(token, |(_, new)| new);
    if token == DX {
        return Some(DX);
    }
    PLACES
        .iter()
        .chain([&LABRADOR])
        .map(|(code, _)| *code)
        .find(|code| *code == token)
}

/// `code` as a contest that counts Labrador as part of Newfoundland and
/// Labrador reads it: LB is NL, and every other code itself.
pub(crate) fn fold_labrador(code: &str) -> &str {
    if code == LABRADOR.0 {
        LABRADOR_IN
    } else {
        code
    }
}

/// Whether `code` is a multiplier for a DX station in the ARRL DX contest:
/// one of the 48 contiguous states, DC, or Canada's provinces and
/// territories, Labrador apart.
pub(crate) fn w_ve_place(code: &str) -> bool {
    W_VE_PLACES.contains(&code)
}

/// What a place's code stands for: "Massachusetts", "Labrador". None for a
/// code that is no place, [`DX`] among them.
pub fn place_name(code: &str) -> Option<&'static str> {
    PLACES
        .iter()
        .chain([&LABRADOR])
        .find(|(listed, _)| *listed == code)
        .map(|(_, name)| *name)
}

/// Where a station is at home, as the places tell the countries apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Home {
    /// The US's call districts.
    Us,
    Alaska,
    Hawaii,
    Canada,
    /// Somewhere else the US issues calls, which the call table misreads:
    /// its other islands and Guantanamo Bay, by the table's key for the
    /// country it is, where the table holds one. A station there sends DX.
    Possession(Option<&'static str>),
    /// Anywhere else: a station there sends DX.
    Abroad,
}

/// Where the station `area` places, signing `call`, is at home: by the
/// prefixes of Alaska, Hawaii, the US's other islands and Canada, which the
/// call table does not all hold or reads as another, and else by the
/// table. None where neither can say.
fn home(call: &str, area: &CallArea<'_>) -> Option<Home> {
    let prefix = area.prefix;
    if ALASKA.contains(&prefix) {
        return Some(Home::Alaska);
    }
    if HAWAII.contains(&prefix) {
        let hawaii = matches!(area.digit, Some('6' | '7'));
        return Some(if hawaii {
            Home::Hawaii
        } else {
            Home::Possession(None)
        });
    }
    if CARIBBEAN.contains(&prefix) {
        let country = CARIBBEAN_DIGITS
            .iter()
            .find(|(digit, _)| area.digit == Some(*digit))
            .map(|(_, country)| *country);
        return Some(Home::Possession(country));
    }
    if CANADA.contains(&prefix) || CANADA_ELSE.contains(&prefix) {
        return Some(Home::Canada);
    }
    // Guantanamo Bay's KG4 with two letters, which the table does not hold:
    // it counts the rest of KG4 as the US's fourth call district.
    let (kg, four, letters) = GUANTANAMO;
    if parse_callsign(call).is_some_and(|parts| {
        parts.host.is_none()
            && parts.prefix == kg
            && parts.separator == four
            && parts.suffix.len() == letters
    }) {
        return Some(Home::Possession(None));
    }
    Some(match country_of(call)? {
        "K" => Home::Us,
        "VE" => Home::Canada,
        _ => Home::Abroad,
    })
}

/// Whether a station signing `call` is in the US or Canada, Alaska and
/// Hawaii among them, and so sends a state, province or territory, rather
/// than DX; None for a call Dust cannot place, which may be either.
pub(crate) fn in_us_or_canada(call: &str) -> Option<bool> {
    let area = call_area(call)?;
    home(call, &area)
        .map(|home| matches!(home, Home::Us | Home::Alaska | Home::Hawaii | Home::Canada))
}

/// The country a station signing `call` is in, as a contest scores it, by
/// the call table's keys (`K`, `KL`, `VE`, `DL`): by where it is at home,
/// for every prefix the US and Canada issue, which the table does not all
/// hold (`NL7` is Alaska, `VG3` Canada) or reads as another (`KP2` is the
/// Virgin Islands, not Puerto Rico); and else by the table. None for a call
/// Dust cannot place, an island the table holds no country for among it
/// (`KH2`, Guam).
pub fn home_country(call: &str) -> Option<&'static str> {
    let area = call_area(call)?;
    match home(call, &area)? {
        Home::Us => Some("K"),
        Home::Alaska => Some("KL"),
        Home::Hawaii => Some("KH6"),
        Home::Canada => Some("VE"),
        Home::Possession(country) => country,
        Home::Abroad => country_of(call),
    }
}

/// Whether a station signing `call` is W/VE, as the ARRL DX contest has
/// the sides: in the 48 contiguous states, DC or Canada, by every prefix
/// they issue. Alaska's prefixes (KL, AL, NL, WL), Hawaii's and the Pacific
/// islands' (KH, AH, NH, WH), the Caribbean islands' (KP, NP, WP) and
/// Guantanamo Bay's (KG4 with two letters) are DX, as is any call outside
/// the US and Canada and any Dust cannot place, Sable and St Paul Islands'
/// CY0 and CY9 among them.
pub(crate) fn w_ve(call: &str) -> bool {
    call_area(call)
        .and_then(|area| home(call, &area))
        .is_some_and(|home| matches!(home, Home::Us | Home::Canada))
}

/// Where a station is, as far as its call says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::contest) enum Whereabouts {
    /// A state, province or territory.
    Place(&'static str),
    /// One of a US call district's states, each with its people.
    District(&'static [(&'static str, f64)]),
}

/// Where a station signing `call` is, by its call area ([`call_area`]): a
/// state, province or territory, or for a US call one of its district's
/// states. A call-area digit after the slash that names no place
/// (`VE3ABC/0`) leaves the station where its own call is. Labrador is LB
/// where `labrador` counts it apart, and NL otherwise. None for a call
/// outside the US and Canada.
pub(in crate::contest) fn whereabouts(call: &str, labrador: bool) -> Option<Whereabouts> {
    let area = call_area(call)?;
    let home = home(call, &area)?;
    if let Some(marker) = area.marker
        && let Some(found) = place_in(home, area.prefix, marker, true, labrador)
    {
        return Some(found);
    }
    place_in(home, area.prefix, area.digit?, false, labrador)
}

/// The state, province or territory a station signing `call` is in, by
/// [`whereabouts`], the US state drawn from `rng` among its district's by
/// their people. None for a call outside the US and Canada.
pub(in crate::contest) fn draw_place(
    call: &str,
    labrador: bool,
    rng: &mut impl Rng,
) -> Option<&'static str> {
    match whereabouts(call, labrador)? {
        Whereabouts::Place(place) => Some(place),
        Whereabouts::District(states) => pick_weighted(states, rng).copied(),
    }
}

/// Every place a station signing `call` can be drawn in, for tests to
/// check a caller's state against its call.
#[cfg(test)]
pub(crate) fn places_for(call: &str, labrador: bool) -> Vec<&'static str> {
    let mut rng = crate::rng::SplitMix64::new(0);
    let mut places: Vec<&str> = (0..500)
        .filter_map(|_| draw_place(call, labrador, &mut rng))
        .collect();
    places.sort_unstable();
    places.dedup();
    places
}

/// Where a call at `home` with `prefix` and `digit` is, the digit a call
/// area after the slash where `marked`, Labrador apart where `labrador`.
fn place_in(
    home: Home,
    prefix: &str,
    digit: char,
    marked: bool,
    labrador: bool,
) -> Option<Whereabouts> {
    let digit = digit.to_digit(10)? as usize;
    let place = match home {
        Home::Us => return Some(Whereabouts::District(DISTRICTS[digit])),
        Home::Alaska => Some("AK"),
        Home::Hawaii => Some("HI"),
        Home::Canada => match prefix {
            _ if marked || CANADA.contains(&prefix) => PROVINCES[digit],
            "VO" if digit == 2 && labrador => Some(LABRADOR.0),
            "VO" => matches!(digit, 1 | 2).then_some(LABRADOR_IN),
            "VY" => VY.get(digit).copied(),
            _ => None,
        },
        Home::Possession(_) | Home::Abroad => None,
    };
    place.map(Whereabouts::Place)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callsign::{CallSpec, Countries, generate_call};
    use crate::rng::SplitMix64;

    /// How many of [`PLACES`] are in the USA, DC among them: the rest are
    /// Canada's.
    const US_PLACES: usize = 51;

    /// Whether `code` is a US state, or DC, rather than a Canadian province
    /// or territory.
    fn in_the_us(code: &str) -> bool {
        PLACES[..US_PLACES]
            .iter()
            .any(|(listed, _)| *listed == code)
    }

    /// The list holds the 50 states, DC, and Canada's 13 provinces and
    /// territories, each once.
    #[test]
    fn every_state_province_and_territory_is_listed_once() {
        assert_eq!(PLACES.len(), US_PLACES + 13);
        let mut codes: Vec<&str> = PLACES.iter().map(|(code, _)| *code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), PLACES.len(), "a code listed twice");
        for code in ["AK", "HI", "DC", "TX", "WY"] {
            assert!(in_the_us(code), "{code}");
        }
        for code in [
            "AB", "BC", "MB", "NB", "NL", "NS", "NT", "NU", "ON", "PE", "QC", "SK", "YT",
        ] {
            assert!(!in_the_us(code) && place_name(code).is_some(), "{code}");
        }
        // Every state is in exactly one call district, DC with them, and
        // only Alaska and Hawaii in none.
        for (code, _) in &PLACES[..US_PLACES] {
            let districts = DISTRICTS
                .iter()
                .filter(|states| states.iter().any(|(state, _)| state == code))
                .count();
            let expected = usize::from(!matches!(*code, "AK" | "HI"));
            assert_eq!(districts, expected, "{code}");
        }
        assert!(
            DISTRICTS
                .iter()
                .flat_map(|states| states.iter())
                .all(|(state, people)| in_the_us(state) && *people > 0.0)
        );
    }

    /// A code reads as itself, an old one as the new, and DX as DX; nothing
    /// else reads as a place.
    #[test]
    fn a_place_reads_by_its_code() {
        assert_eq!(place_code("OR"), Some("OR"));
        assert_eq!(place_code("PQ"), Some("QC"));
        assert_eq!(place_code("NF"), Some("NL"));
        assert_eq!(place_code("DX"), Some("DX"));
        // Labrador reads as itself, and as NL where it is not counted
        // apart.
        assert_eq!(place_code("LB"), Some("LB"));
        assert_eq!(fold_labrador("LB"), "NL");
        assert_eq!(fold_labrador("NL"), "NL");
        assert_eq!(fold_labrador("QC"), "QC");
        for nothing in ["", "XX", "LBX", "or", "ORE", "MASS", "5"] {
            assert_eq!(place_code(nothing), None, "{nothing:?}");
        }
        assert_eq!(place_name("MA"), Some("Massachusetts"));
        assert_eq!(place_name("LB"), Some("Labrador"));
        assert_eq!(place_name("DX"), None);
    }

    /// A DX station's ARRL DX multipliers: the 48 contiguous states, DC,
    /// and Canada's provinces and territories with Labrador apart, 63 in
    /// all, each once; Alaska, Hawaii and DX are none.
    #[test]
    fn the_arrl_dx_places_are_the_48_states_dc_and_canada() {
        assert_eq!(W_VE_PLACES.len(), 63);
        let mut codes = W_VE_PLACES.to_vec();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 63, "a code listed twice");
        let states = W_VE_PLACES.iter().filter(|code| in_the_us(code)).count();
        assert_eq!(states, 49, "the 48 contiguous states and DC");
        for code in [
            "NB", "NS", "QC", "ON", "MB", "SK", "AB", "BC", "NT", "NL", "LB", "NU", "YT", "PE",
        ] {
            assert!(w_ve_place(code), "{code}");
        }
        for code in ["AK", "HI", "DX", "PQ", ""] {
            assert!(!w_ve_place(code), "{code:?}");
        }
        for code in W_VE_PLACES {
            assert!(place_name(code).is_some(), "{code}");
        }
    }

    /// The ARRL DX sides: the 48 states, DC and Canada are W/VE, by every
    /// prefix they issue; every prefix the design names DX is DX, and so is
    /// every call outside the US and Canada, or one Dust cannot place.
    #[test]
    fn w_ve_is_the_48_states_dc_and_canada() {
        for call in [
            "K1ABC", "W7XYZ", "N0AB", "AA4AB", "KG4ABC", "W1AW/4", "VE3ABC", "VA2ABC", "VO1AB",
            "VO2AB", "VY0AB", "VY1AB", "VY2AB", "VG3ABC", "CF3ABC", "XO3ABC", "CK2AB", "W1AW/VE3",
            "VE3/W1AW",
        ] {
            assert!(w_ve(call), "{call}");
        }
        for prefix in [
            "KH", "KL", "KP", "AH", "AL", "NH", "NL", "NP", "WH", "WL", "WP",
        ] {
            for digit in '0'..='9' {
                let call = format!("{prefix}{digit}AB");
                assert!(!w_ve(&call), "{call}");
            }
        }
        for call in [
            "CY0AA", "CY9AA", "DL1ABC", "DL/K1ABC", "W1AW/KH6", "W1AW/KP4", "XE1ABC", "C31AB",
            "KG4AB", "ABC", "",
        ] {
            assert!(!w_ve(call), "{call:?}");
        }
    }

    /// The country a contest scores a station in: by every prefix the US
    /// and Canada issue, which the call table does not all hold or reads as
    /// another, and else by the table; none for a place the table does not
    /// hold. Every call a contest draws is in the country the generator
    /// says it is.
    #[test]
    fn a_station_is_scored_in_the_country_it_is_at_home_in() {
        for (call, country) in [
            ("K1ABC", Some("K")),
            ("W1AW/4", Some("K")),
            ("KG4ABC", Some("K")),
            ("KG4A", Some("K")),
            ("VE3ABC", Some("VE")),
            ("VG3ABC", Some("VE")),
            ("CF3ABC", Some("VE")),
            ("XM3ABC", Some("VE")),
            ("CH1ABC", Some("VE")),
            ("VO2AB", Some("VE")),
            ("KL7AB", Some("KL")),
            ("NL7AB", Some("KL")),
            ("WL7AB", Some("KL")),
            ("KH6XYZ", Some("KH6")),
            ("NH7AB", Some("KH6")),
            ("W1AW/KH6", Some("KH6")),
            ("KP4AB", Some("KP4")),
            ("WP3AB", Some("KP4")),
            ("KP2AB", Some("KP2")),
            ("NP2AB", Some("KP2")),
            ("DL1ABC", Some("DL")),
            ("DL/K1ABC", Some("DL")),
            // Guantanamo Bay, the Pacific islands KH signs with another
            // digit than 6 or 7, Navassa and Desecheo: none the table holds.
            ("KG4AB", None),
            ("KH2AB", None),
            ("KH0AB", None),
            ("KH8AB", None),
            ("AH2AB", None),
            ("KP1AB", None),
            ("KP5AB", None),
            ("C31AB", None),
            ("ABC", None),
            ("", None),
        ] {
            assert_eq!(home_country(call), country, "{call:?}");
        }
        for countries in [
            Countries::Any,
            Countries::WVe,
            Countries::NotWVe,
            Countries::NorthAmerica,
            Countries::UsAndCanada,
        ] {
            let mut rng = SplitMix64::new(7);
            for _ in 0..3_000 {
                let call = generate_call(6, CallSpec::contest(countries), &mut rng);
                assert_eq!(home_country(&call.text), Some(call.country), "{call:?}");
            }
        }
    }

    /// Labrador is LB where it is counted apart, and NL elsewhere; the
    /// rest of Newfoundland and Labrador is NL either way.
    #[test]
    fn labrador_is_apart_only_where_it_is_counted_apart() {
        let mut rng = SplitMix64::new(23);
        for (call, apart, kept) in [
            ("VO2AB", "LB", "NL"),
            ("W1AW/VO2", "LB", "NL"),
            ("VO1AB", "NL", "NL"),
            ("VE2ABC", "QC", "QC"),
        ] {
            assert_eq!(draw_place(call, true, &mut rng), Some(apart), "{call}");
            assert_eq!(draw_place(call, false, &mut rng), Some(kept), "{call}");
        }
        assert_eq!(places_for("VO2AB", true), ["LB"]);
    }

    /// Every US call district gives only its own states, each of them now
    /// and then, the populous ones most; Alaska and Hawaii their own.
    #[test]
    fn every_us_district_gives_only_its_states() {
        let mut rng = SplitMix64::new(21);
        for (digit, states) in DISTRICTS.iter().enumerate() {
            let call = format!("W{digit}XYZ");
            let mut seen = std::collections::BTreeMap::new();
            for _ in 0..4_000 {
                let place = draw_place(&call, false, &mut rng).expect("a state");
                *seen.entry(place).or_insert(0u32) += 1;
            }
            for place in seen.keys() {
                assert!(
                    states.iter().any(|(state, _)| state == place),
                    "{call} gave {place}"
                );
            }
            assert_eq!(seen.len(), states.len(), "{call}: {seen:?}");
            let total: f64 = states.iter().map(|(_, people)| people).sum();
            for (state, people) in *states {
                let share = f64::from(seen[state]) / 4_000.0;
                assert!(
                    (share - people / total).abs() < 0.03,
                    "{state}: {share:.3} of {call}"
                );
            }
        }
        // A portable is where its call area says, a station abroad nowhere
        // in the US.
        for _ in 0..200 {
            let fourth = draw_place("W1AW/4", false, &mut rng).expect("a state");
            assert!(DISTRICTS[4].iter().any(|(state, _)| *state == fourth));
            let first = draw_place("W1AW/P", false, &mut rng).expect("a state");
            assert!(DISTRICTS[1].iter().any(|(state, _)| *state == first));
        }
        for call in ["KL7XYZ", "AL7AB", "KL0AB"] {
            assert_eq!(draw_place(call, false, &mut rng), Some("AK"), "{call}");
        }
        for call in ["KH6XYZ", "NH6AB", "KH7AB"] {
            assert_eq!(draw_place(call, false, &mut rng), Some("HI"), "{call}");
        }
        for call in ["DL1ABC", "KP4AB", "XE1AB", "DL/W1AW", "ABC"] {
            assert_eq!(draw_place(call, false, &mut rng), None, "{call}");
        }
    }

    /// Every prefix Canada issues in place of VE gives the province of its
    /// digit, whether the call table holds it or not; VO and VY those of
    /// their own; and a call-area digit after the slash is a province by
    /// VE's digits whatever the prefix.
    #[test]
    fn every_canadian_prefix_gives_its_province() {
        let mut rng = SplitMix64::new(22);
        for prefix in CANADA {
            for (digit, province) in PROVINCES.iter().enumerate() {
                let call = format!("{prefix}{digit}ABC");
                assert_eq!(draw_place(&call, false, &mut rng), *province, "{call}");
            }
        }
        for (call, province) in [
            ("VE1ABC", "NS"),
            ("VA2ABC", "QC"),
            ("VE3ABC", "ON"),
            ("VE4ABC", "MB"),
            ("VE5ABC", "SK"),
            ("VA6ABC", "AB"),
            ("VE7ABC", "BC"),
            ("VE8ABC", "NT"),
            ("VE9ABC", "NB"),
            ("VC3AB", "ON"),
            ("CK2AB", "QC"),
            ("VB7AB", "BC"),
            ("VO1AB", "NL"),
            ("VO2AB", "NL"),
            ("VY0AB", "NU"),
            ("VY1AB", "YT"),
            ("VY2AB", "PE"),
            ("VG3ABC", "ON"),
            ("XM7ABC", "BC"),
            // A portable is where its call area says, by VE's digits; one
            // that names no place is where its own call is.
            ("VE3ABC/7", "BC"),
            ("VE3ABC/0", "ON"),
            ("VY1ABC/2", "QC"),
            ("VO1ABC/3", "ON"),
            ("VY2ABC/0", "PE"),
            ("W1AW/VE3", "ON"),
            ("W1AW/VO1", "NL"),
        ] {
            assert_eq!(draw_place(call, false, &mut rng), Some(province), "{call}");
        }
        // CH and CI stood in for VO and VY: Canada's, by no digit Dust can
        // read.
        for call in ["CH1ABC", "CI1ABC"] {
            assert_eq!(in_us_or_canada(call), Some(true), "{call}");
            assert_eq!(draw_place(call, false, &mut rng), None, "{call}");
        }
    }

    /// A station is in the US or Canada by every prefix they issue, the
    /// call table's or not, Alaska's and Hawaii's among them; elsewhere by
    /// the table, Hawaii's prefixes with another digit too; and nowhere Dust
    /// can say for a prefix it does not know.
    #[test]
    fn a_call_says_whether_it_is_in_the_us_or_canada() {
        for (call, home) in [
            ("K1ABC", Some(true)),
            ("W1AW/4", Some(true)),
            ("VE3ABC", Some(true)),
            ("VG3ABC", Some(true)),
            ("CF3ABC", Some(true)),
            ("XO3ABC", Some(true)),
            ("VY1ABC", Some(true)),
            ("KL7AB", Some(true)),
            ("WL7AB", Some(true)),
            ("NL7AB", Some(true)),
            ("KH6AB", Some(true)),
            ("NH7AB", Some(true)),
            ("W1AW/VE3", Some(true)),
            ("DL1ABC", Some(false)),
            ("KP4AB", Some(false)),
            ("KH2AB", Some(false)),
            ("AH0AB", Some(false)),
            ("KP2AB", Some(false)),
            ("KP1AB", Some(false)),
            ("KG4AB", Some(false)),
            ("KG4ABC", Some(true)),
            ("KH9AB", Some(false)),
            ("W1AW/KH2", Some(false)),
            ("DL/K1ABC", Some(false)),
            ("PA/N8XYZ", Some(false)),
            ("C31AB", None),
            ("ABC", None),
            ("", None),
        ] {
            assert_eq!(in_us_or_canada(call), home, "{call}");
        }
        for call in ["KH2AB", "KH0AB", "KH8AB"] {
            assert_eq!(
                draw_place(call, false, &mut SplitMix64::new(1)),
                None,
                "{call}"
            );
        }
    }
}
