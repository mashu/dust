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
//! for every digit, so where a station is at home is read here, from the
//! prefixes the US gives Alaska and Hawaii and every prefix Canada issues.

use crate::callsign::{CallArea, call_area, country_of, pick_weighted};
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
/// they are Pacific islands no state holds (KH2 is Guam).
const HAWAII: [&str; 4] = ["AH", "KH", "NH", "WH"];

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
/// [`PLACES`], an older code read as its newer one (PQ is QC, NF is NL), or
/// [`DX`]. None for anything else.
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
        .map(|(code, _)| *code)
        .find(|code| *code == token)
}

/// What a place's code stands for: "Massachusetts". None for a code that is
/// no place, [`DX`] among them.
pub fn place_name(code: &str) -> Option<&'static str> {
    PLACES
        .iter()
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
    /// Anywhere else: a station there sends DX.
    Abroad,
}

/// Where the station `area` places, signing `call`, is at home: by the
/// prefixes of Alaska, Hawaii and Canada, which the call table does not
/// all hold, and else by the table. None where neither can say.
fn home(call: &str, area: &CallArea<'_>) -> Option<Home> {
    let prefix = area.prefix;
    if ALASKA.contains(&prefix) {
        return Some(Home::Alaska);
    }
    if HAWAII.contains(&prefix) {
        let hawaii = matches!(area.digit, Some('6' | '7'));
        return Some(if hawaii { Home::Hawaii } else { Home::Abroad });
    }
    if CANADA.contains(&prefix) || CANADA_ELSE.contains(&prefix) {
        return Some(Home::Canada);
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
    home(call, &area).map(|home| home != Home::Abroad)
}

/// The state, province or territory a station signing `call` is in: by its
/// call area ([`call_area`]), the US state drawn from `rng` among its
/// district's by their people. A call-area digit after the slash that names
/// no place (`VE3ABC/0`) leaves the station where its own call is. None for
/// a call outside the US and Canada.
pub(in crate::contest) fn draw_place(call: &str, rng: &mut impl Rng) -> Option<&'static str> {
    let area = call_area(call)?;
    let home = home(call, &area)?;
    if let Some(marker) = area.marker
        && let Some(place) = place_in(home, area.prefix, marker, true, rng)
    {
        return Some(place);
    }
    place_in(home, area.prefix, area.digit?, false, rng)
}

/// Every place a station signing `call` can be drawn in, for tests to
/// check a caller's state against its call.
#[cfg(test)]
pub(crate) fn places_for(call: &str) -> Vec<&'static str> {
    let mut rng = crate::rng::SplitMix64::new(0);
    let mut places: Vec<&str> = (0..500)
        .filter_map(|_| draw_place(call, &mut rng))
        .collect();
    places.sort_unstable();
    places.dedup();
    places
}

/// The place a call at `home` with `prefix` and `digit` is in, the digit a
/// call area after the slash where `marked`.
fn place_in(
    home: Home,
    prefix: &str,
    digit: char,
    marked: bool,
    rng: &mut impl Rng,
) -> Option<&'static str> {
    let digit = digit.to_digit(10)? as usize;
    match home {
        Home::Us => pick_weighted(DISTRICTS[digit], rng).copied(),
        Home::Alaska => Some("AK"),
        Home::Hawaii => Some("HI"),
        Home::Canada => match prefix {
            _ if marked || CANADA.contains(&prefix) => PROVINCES[digit],
            "VO" => matches!(digit, 1 | 2).then_some("NL"),
            "VY" => VY.get(digit).copied(),
            _ => None,
        },
        Home::Abroad => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        for nothing in ["", "XX", "LB", "or", "ORE", "MASS", "5"] {
            assert_eq!(place_code(nothing), None, "{nothing:?}");
        }
        assert_eq!(place_name("MA"), Some("Massachusetts"));
        assert_eq!(place_name("DX"), None);
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
                let place = draw_place(&call, &mut rng).expect("a state");
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
            let fourth = draw_place("W1AW/4", &mut rng).expect("a state");
            assert!(DISTRICTS[4].iter().any(|(state, _)| *state == fourth));
            let first = draw_place("W1AW/P", &mut rng).expect("a state");
            assert!(DISTRICTS[1].iter().any(|(state, _)| *state == first));
        }
        for call in ["KL7XYZ", "AL7AB", "KL0AB"] {
            assert_eq!(draw_place(call, &mut rng), Some("AK"), "{call}");
        }
        for call in ["KH6XYZ", "NH6AB", "KH7AB"] {
            assert_eq!(draw_place(call, &mut rng), Some("HI"), "{call}");
        }
        for call in ["DL1ABC", "KP4AB", "XE1AB", "DL/W1AW", "ABC"] {
            assert_eq!(draw_place(call, &mut rng), None, "{call}");
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
                assert_eq!(draw_place(&call, &mut rng), *province, "{call}");
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
            assert_eq!(draw_place(call, &mut rng), Some(province), "{call}");
        }
        // CH and CI stood in for VO and VY: Canada's, by no digit Dust can
        // read.
        for call in ["CH1ABC", "CI1ABC"] {
            assert_eq!(in_us_or_canada(call), Some(true), "{call}");
            assert_eq!(draw_place(call, &mut rng), None, "{call}");
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
            assert_eq!(draw_place(call, &mut SplitMix64::new(1)), None, "{call}");
        }
    }
}
