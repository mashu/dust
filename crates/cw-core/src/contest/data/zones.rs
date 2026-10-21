//! CQ zones and continents: where a station is, as the CQ WW contest's
//! exchange sends it and its points count it.
//!
//! Outside North America a station's zone is its country's, or its
//! prefix's where cty.dat puts that prefix in another zone (China's BY0 is
//! 23), from the call table's facts (`callsign/prefixes.rs`); Australia's
//! VK6 and VK8 are zone 29 and the rest of VK 30, entered here by hand, as
//! that cty.dat has no CQ zone of their own for them. Asiatic Russia is one
//! zone, as the table keeps no override that turns on a suffix's letter.
//! Its continent is its country's, or its prefix's where cty.dat puts that
//! prefix on another (European Turkey's TA1 is in Europe).
//!
//! The US and Canada go by state and province, entered here by hand from
//! the CQ zone list (§11.3 of the design), as cty.dat gives Canada zone 5
//! but for a few northern prefixes. Quebec is zone 5 but for its far north,
//! about one station in a hundred, in zone 2; Nunavut is zone 2 but for its
//! west, about one in ten, in zone 1. A caller there is drawn by those
//! shares; your own zone from your call is the one most stations there are
//! in, with the myZone setting for a zone your call cannot tell.

use super::places::{Whereabouts, draw_place, home_country, in_us_or_canada, whereabouts};
use crate::callsign::{CallArea, call_area, continent_in, zone_in};
use crate::rng::Rng;

/// Every US state, DC, and Canadian province and territory, with Labrador,
/// and the CQ zone most of its stations are in.
#[rustfmt::skip]
const PLACE_ZONES: [(&str, u32); 65] = [
    // Zone 3: the western states.
    ("WA", 3), ("OR", 3), ("CA", 3), ("NV", 3), ("ID", 3), ("UT", 3), ("AZ", 3),
    // Zone 5: the eastern states.
    ("ME", 5), ("NH", 5), ("VT", 5), ("MA", 5), ("RI", 5), ("CT", 5), ("NY", 5),
    ("NJ", 5), ("PA", 5), ("DE", 5), ("MD", 5), ("DC", 5), ("VA", 5), ("WV", 5),
    ("NC", 5), ("SC", 5), ("GA", 5), ("FL", 5),
    // Zone 4: the other contiguous states.
    ("AL", 4), ("AR", 4), ("CO", 4), ("IL", 4), ("IN", 4), ("IA", 4), ("KS", 4),
    ("KY", 4), ("LA", 4), ("MI", 4), ("MN", 4), ("MS", 4), ("MO", 4), ("MT", 4),
    ("NE", 4), ("NM", 4), ("ND", 4), ("OH", 4), ("OK", 4), ("SD", 4), ("TN", 4),
    ("TX", 4), ("WI", 4), ("WY", 4),
    // Alaska and Hawaii.
    ("AK", 1), ("HI", 31),
    // Canada.
    ("BC", 3), ("AB", 4), ("SK", 4), ("MB", 4), ("ON", 4), ("NS", 5), ("NB", 5),
    ("PE", 5), ("NL", 5), ("LB", 2), ("QC", 5), ("NU", 2), ("YT", 1), ("NT", 1),
];

/// The places split between two zones: the zone the rest of their stations
/// are in, and their share of them.
const SPLIT_ZONES: [(&str, u32, f64); 2] = [("QC", 2, 0.01), ("NU", 1, 0.10)];

/// Australia's call areas in zone 29, the west; the rest of VK is 30.
const VK_WEST: [char; 2] = ['6', '8'];
const VK_WEST_ZONE: u32 = 29;

/// The zone most stations in `place` are in.
fn place_zone(place: &str) -> Option<u32> {
    PLACE_ZONES
        .iter()
        .find(|(code, _)| *code == place)
        .map(|(_, zone)| *zone)
}

/// The zone of a station in `place`, drawn from `rng` where the place is
/// split between two.
fn draw_place_zone(place: &str, rng: &mut impl Rng) -> Option<u32> {
    let zone = place_zone(place)?;
    Some(
        match SPLIT_ZONES.iter().find(|(code, _, _)| *code == place) {
            Some((_, other, share)) if rng.f64() < *share => *other,
            _ => zone,
        },
    )
}

/// A station's prefix and the digit its zone and continent go by: a
/// call-area digit after the slash, or else the prefix's own (`VK2AB/6` is
/// `VK6`).
fn prefix_text(area: &CallArea<'_>) -> String {
    area.prefix
        .chars()
        .chain(area.marker.or(area.digit))
        .collect()
}

/// The zone of a station signing `call` outside the US and Canada: its
/// prefix's or its country's ([`home_country`]), Australia's west apart.
fn country_zone(call: &str) -> Option<u32> {
    let area = call_area(call)?;
    let country = home_country(call)?;
    let digit = area.marker.or(area.digit);
    if country == "VK" && digit.is_some_and(|digit| VK_WEST.contains(&digit)) {
        return Some(VK_WEST_ZONE);
    }
    zone_in(country, &prefix_text(&area))
}

/// The continent of a station signing `call`, by the two letters contests
/// use (`EU`, `NA`): its prefix's or its country's ([`home_country`]).
/// None for a call Dust cannot place.
pub fn home_continent(call: &str) -> Option<&'static str> {
    let area = call_area(call)?;
    continent_in(home_country(call)?, &prefix_text(&area))
}

/// Your CQ zone, from your call: in the US or Canada, by `place`, your
/// state, province or territory, where you give one, or else by where your
/// call puts you, the zone most stations there are in, and none where its
/// digit names no place (`CH1`, `VE0`); outside them, your prefix's or your
/// country's. None for a call Dust cannot place.
pub(crate) fn zone_of(call: &str, place: Option<&str>) -> Option<u32> {
    if in_us_or_canada(call) != Some(true) {
        return country_zone(call);
    }
    if let Some(zone) = place.and_then(place_zone) {
        return Some(zone);
    }
    match whereabouts(call, true)? {
        Whereabouts::Place(place) => place_zone(place),
        // The zone most of the district's people are in.
        Whereabouts::District(states) => {
            let mut zones: Vec<(u32, f64)> = Vec::new();
            for (state, people) in states {
                let zone = place_zone(state)?;
                match zones.iter_mut().find(|(each, _)| *each == zone) {
                    Some((_, total)) => *total += people,
                    None => zones.push((zone, *people)),
                }
            }
            zones
                .into_iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(zone, _)| zone)
        }
    }
}

/// The CQ zone a caller signing `call` sends: in the US and Canada, its
/// state's or province's ([`draw_place`]), a split place's zone drawn by
/// its shares; elsewhere its prefix's or its country's. None for a call
/// Dust cannot place.
pub(in crate::contest) fn draw_zone(call: &str, rng: &mut impl Rng) -> Option<u32> {
    match draw_place(call, true, rng) {
        Some(place) => draw_place_zone(place, rng),
        None => country_zone(call),
    }
}

#[cfg(test)]
mod tests {
    use super::super::places::place_name;
    use super::*;
    use crate::rng::SplitMix64;

    /// The North American table, row by row, as the CQ zone list has it.
    #[test]
    fn the_north_american_zones_are_the_cq_zone_lists() {
        let zone_of_each =
            |codes: &str| -> Vec<Option<u32>> { codes.split(' ').map(place_zone).collect() };
        let all = |codes: &str, zone: u32| {
            for (code, found) in codes.split(' ').zip(zone_of_each(codes)) {
                assert_eq!(found, Some(zone), "{code}");
            }
        };
        all("WA OR CA NV ID UT AZ", 3);
        all("ME NH VT MA RI CT NY NJ PA DE MD DC VA WV NC SC GA FL", 5);
        all(
            "AL AR CO IL IN IA KS KY LA MI MN MS MO MT NE NM ND OH OK SD TN TX WI WY",
            4,
        );
        all("AK", 1);
        all("HI", 31);
        all("BC", 3);
        all("AB SK MB ON", 4);
        all("NS NB PE NL", 5);
        all("LB", 2);
        all("QC", 5);
        all("NU", 2);
        all("YT NT", 1);
        // Every row once: the 50 states and DC, Canada's 13 and Labrador.
        let mut codes: Vec<&str> = PLACE_ZONES.iter().map(|(code, _)| *code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 51 + 13 + 1);
        for code in &codes {
            assert!(place_name(code).is_some(), "{code}");
        }
        assert_eq!(SPLIT_ZONES, [("QC", 2, 0.01), ("NU", 1, 0.10)]);
    }

    /// A split place's callers come from its other zone about as often as
    /// its share says.
    #[test]
    fn a_split_place_sends_its_other_zone_by_its_share() {
        let mut rng = SplitMix64::new(31);
        for (call, home_zone, other, share) in [("VE2ABC", 5, 2, 0.01), ("VY0AB", 2, 1, 0.10)] {
            let draws: Vec<u32> = (0..20_000)
                .map(|_| draw_zone(call, &mut rng).expect("a zone"))
                .collect();
            assert!(draws.iter().all(|zone| [home_zone, other].contains(zone)));
            let seen = draws.iter().filter(|zone| **zone == other).count() as f64 / 20_000.0;
            assert!((seen - share).abs() < share / 3.0, "{call}: {seen:.3}");
        }
    }

    /// Your zone from your call, and from your state where you give one.
    #[test]
    fn your_zone_comes_from_your_call_and_your_state() {
        for (call, place, zone) in [
            ("DL1ABC", None, Some(14)),
            ("JA1XYZ", None, Some(25)),
            ("G4XYZ", None, Some(14)),
            ("PY2ABC", None, Some(11)),
            ("ZS6AB", None, Some(38)),
            ("UA9ABC", None, Some(17)),
            ("R8AB", None, Some(17)),
            ("UA3ABC", None, Some(16)),
            ("XE1AB", None, Some(6)),
            ("KP4AB", None, Some(8)),
            ("KH6XYZ", None, Some(31)),
            ("KL7XYZ", None, Some(1)),
            // China's overrides, on a prefix and its digit.
            ("BY1AB", None, Some(24)),
            ("BY0AB", None, Some(23)),
            ("BG9AB", None, Some(23)),
            // Australia's west, entered by hand.
            ("VK6AB", None, Some(29)),
            ("VK8AB", None, Some(29)),
            ("VK2AB", None, Some(30)),
            ("VK2AB/6", None, Some(29)),
            // A US call: the zone most of its district is in.
            ("K1ABC", None, Some(5)),
            ("W2ABC", None, Some(5)),
            ("K3ABC", None, Some(5)),
            ("W4ABC", None, Some(5)),
            ("K5ABC", None, Some(4)),
            ("W6ABC", None, Some(3)),
            ("K7ABC", None, Some(3)),
            ("W8ABC", None, Some(4)),
            ("K9ABC", None, Some(4)),
            ("W0ABC", None, Some(4)),
            ("W1AW/6", None, Some(3)),
            // Your state settles a district split between zones.
            ("K7ABC", Some("MT"), Some(4)),
            ("W4ABC", Some("TN"), Some(4)),
            ("W8ABC", Some("WV"), Some(5)),
            // Canada by province: Labrador is zone 2.
            ("VE1ABC", None, Some(5)),
            ("VE2ABC", None, Some(5)),
            ("VE3ABC", None, Some(4)),
            ("VE7ABC", None, Some(3)),
            ("VE8ABC", None, Some(1)),
            ("VO1AB", None, Some(5)),
            ("VO2AB", None, Some(2)),
            ("VY0AB", None, Some(2)),
            ("VY1AB", None, Some(1)),
            ("VY2AB", None, Some(5)),
            ("VE2ABC", Some("QC"), Some(5)),
            // Canada's prefixes the table does not hold, and Alaska's.
            ("VG3ABC", None, Some(4)),
            ("XM3ABC", None, Some(4)),
            ("NL7AB", None, Some(1)),
            // A call in Canada whose digit names no place says no zone, and
            // takes your province's where you give one.
            ("CH1ABC", None, None),
            ("CH1ABC", Some("QC"), Some(5)),
            ("CI2AB", Some("NU"), Some(2)),
            ("VE0ABC", None, None),
            ("VE0ABC", Some("BC"), Some(3)),
            // The US's islands: the Virgin Islands are where the table has
            // them; the Pacific islands KH signs with another digit than 6
            // or 7 Dust cannot place, as the table reads them as Hawaii.
            ("KP2AB", None, Some(8)),
            ("NP2AB", None, Some(8)),
            ("KH2AB", None, None),
            ("KH0AB", None, None),
            ("AH8AB", None, None),
            ("KH2AB", Some("HI"), None),
            // European Turkey and the rest of Turkey share a zone.
            ("TA1ABC", None, Some(20)),
            // Abroad, where the host is.
            ("DL/W1AW", None, Some(14)),
            ("W1AW/VE3", None, Some(4)),
            // A state given outside North America changes nothing.
            ("DL1ABC", Some("MA"), Some(14)),
            // Nothing Dust can place.
            ("C31AB", None, None),
            ("ABC", None, None),
        ] {
            assert_eq!(zone_of(call, place), zone, "{call} {place:?}");
        }
    }

    /// A station's continent is its country's, or its prefix's where
    /// cty.dat puts it on another: European Turkey is in Europe. Alaska's
    /// and Canada's prefixes the call table does not hold are in North
    /// America; the Pacific islands KH signs with another digit Dust cannot
    /// place.
    #[test]
    fn a_continent_is_the_countrys_or_the_prefixs() {
        for (call, continent) in [
            ("DL1ABC", Some("EU")),
            ("JA1XYZ", Some("AS")),
            ("UA9ABC", Some("AS")),
            ("TA2ABC", Some("AS")),
            ("TA1ABC", Some("EU")),
            ("YM1AB", Some("EU")),
            ("TC1AB", Some("EU")),
            ("K1ABC", Some("NA")),
            ("NL7AB", Some("NA")),
            ("VG3ABC", Some("NA")),
            ("KH6XYZ", Some("OC")),
            ("KP2AB", Some("NA")),
            ("KH2AB", None),
            ("C31AB", None),
            ("", None),
        ] {
            assert_eq!(home_continent(call), continent, "{call:?}");
        }
    }

    /// A caller's zone is one of its place's or its country's.
    #[test]
    fn a_callers_zone_is_where_its_call_puts_it() {
        let mut rng = SplitMix64::new(32);
        for _ in 0..200 {
            assert!([3, 4].contains(&draw_zone("W7XYZ", &mut rng).expect("a zone")));
            assert!([4, 5].contains(&draw_zone("W4XYZ", &mut rng).expect("a zone")));
            assert_eq!(draw_zone("W1XYZ", &mut rng), Some(5));
            assert_eq!(draw_zone("DL1ABC", &mut rng), Some(14));
            assert_eq!(draw_zone("VO2AB", &mut rng), Some(2));
        }
        assert_eq!(draw_zone("C31AB", &mut rng), None);
    }
}
