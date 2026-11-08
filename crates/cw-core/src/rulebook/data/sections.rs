//! The ARRL and RAC sections a Field Day station sends: the 85 of them as
//! of RAC's changes of 1 January 2023 (MAR became NB and NS, NT became TER,
//! GTA became GH), listed by call area as the design lists them (§11.4).
//! This list is versioned data: a section split or renamed is a new list.
//!
//! RAC gave Yukon a section of its own on 1 July 2025, which ARRL contests
//! do not count yet: a Yukon station still sends TER, as the 2026 Field Day
//! entries show, and its own code is read as TER. Sources disagree on that
//! code. A map of the sections kept up with RAC's announcements gives YT,
//! and calls the YUK first reported for it a mistake; news reports of the
//! change give YUK. ARRL's and RAC's own lists could not be reached from
//! here, so Dust reads YT, as the design has it, and the choice is to be
//! checked against them. No other old or new code is read: NFL is
//! Northern Florida, never Newfoundland.

use super::places::{DX, place_name};

/// Every section, by call area, the Canadian ones last.
#[rustfmt::skip]
const SECTIONS: [(&str, &[&str]); 11] = [
    ("1", &["CT", "EMA", "ME", "NH", "RI", "VT", "WMA"]),
    ("2", &["ENY", "NLI", "NNJ", "NNY", "SNJ", "WNY"]),
    ("3", &["DE", "EPA", "MDC", "WPA"]),
    ("4", &["AL", "GA", "KY", "NC", "NFL", "PR", "SC", "SFL", "TN", "VA", "VI", "WCF"]),
    ("5", &["AR", "LA", "MS", "NM", "NTX", "OK", "STX", "WTX"]),
    ("6", &["EB", "LAX", "ORG", "PAC", "SB", "SCV", "SDG", "SF", "SJV", "SV"]),
    ("7", &["AK", "AZ", "EWA", "ID", "MT", "NV", "OR", "UT", "WWA", "WY"]),
    ("8", &["MI", "OH", "WV"]),
    ("9", &["IL", "IN", "WI"]),
    ("0", &["CO", "IA", "KS", "MN", "MO", "ND", "NE", "SD"]),
    ("Canada", &["AB", "BC", "GH", "MB", "NB", "NL", "NS", "ONE", "ONN", "ONS", "PE", "QC",
                 "SK", "TER"]),
];

/// The name of each section that is not a whole state, province or
/// territory under the same code; every other is named as its place is
/// (`places.rs`).
#[rustfmt::skip]
const NAMES: &[(&str, &str)] = &[
    ("EMA", "Eastern Massachusetts"), ("WMA", "Western Massachusetts"),
    ("ENY", "Eastern New York"), ("NLI", "New York City and Long Island"),
    ("NNY", "Northern New York"), ("WNY", "Western New York"),
    ("NNJ", "Northern New Jersey"), ("SNJ", "Southern New Jersey"),
    ("EPA", "Eastern Pennsylvania"), ("WPA", "Western Pennsylvania"),
    ("MDC", "Maryland and DC"),
    ("NFL", "Northern Florida"), ("SFL", "Southern Florida"),
    ("WCF", "West Central Florida"), ("PR", "Puerto Rico"), ("VI", "Virgin Islands"),
    ("NTX", "North Texas"), ("STX", "South Texas"), ("WTX", "West Texas"),
    ("EB", "East Bay"), ("LAX", "Los Angeles"), ("ORG", "Orange"), ("PAC", "Pacific"),
    ("SB", "Santa Barbara"), ("SCV", "Santa Clara Valley"), ("SDG", "San Diego"),
    ("SF", "San Francisco"), ("SJV", "San Joaquin Valley"), ("SV", "Sacramento Valley"),
    ("EWA", "Eastern Washington"), ("WWA", "Western Washington"),
    ("GH", "Golden Horseshoe"), ("ONE", "Ontario East"), ("ONN", "Ontario North"),
    ("ONS", "Ontario South"), ("TER", "Territories"),
];

/// Yukon's own section, which ARRL contests count as part of the
/// Territories.
const YUKON: (&str, &str) = ("YT", "TER");

/// Every section's code, in the order [`SECTIONS`] lists them.
fn listed() -> impl Iterator<Item = &'static str> {
    SECTIONS.iter().flat_map(|(_, codes)| codes.iter().copied())
}

/// `token` as the code a station sends for its section: one of the 85,
/// Yukon's [`YUKON`] code, or [`DX`]. None for anything else. Yukon is read
/// as itself; an ARRL contest reads it as TER ([`fold_yukon`]).
pub(crate) fn listed_section(token: &str) -> Option<&'static str> {
    if token == DX {
        return Some(DX);
    }
    if token == YUKON.0 {
        return Some(YUKON.0);
    }
    listed().find(|code| *code == token)
}

/// `code` as an ARRL contest reads it: Yukon's is TER, and every other code
/// itself.
pub(crate) fn fold_yukon(code: &str) -> &str {
    if code == YUKON.0 { YUKON.1 } else { code }
}

/// What a section's code stands for: "Eastern Massachusetts", "Oregon".
/// None for a code that is no section, DX and Yukon's among them.
pub fn section_name(code: &str) -> Option<&'static str> {
    listed().find(|listed| *listed == code)?;
    NAMES
        .iter()
        .find(|(named, _)| *named == code)
        .map(|(_, name)| *name)
        .or_else(|| place_name(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::data::counted::PLACE_SECTIONS;
    use crate::contest::data::places::place_code;

    /// The 85 sections, each once, by call area as the design lists them.
    #[test]
    fn the_85_sections_are_listed_by_call_area() {
        let codes: Vec<&str> = listed().collect();
        assert_eq!(codes.len(), 85);
        let mut unique = codes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 85, "a section listed twice");
        let counts: Vec<(&str, usize)> = SECTIONS
            .iter()
            .map(|(area, codes)| (*area, codes.len()))
            .collect();
        assert_eq!(
            counts,
            [
                ("1", 7),
                ("2", 6),
                ("3", 4),
                ("4", 12),
                ("5", 8),
                ("6", 10),
                ("7", 10),
                ("8", 3),
                ("9", 3),
                ("0", 8),
                ("Canada", 14)
            ]
        );
        for code in codes {
            assert_eq!(listed_section(code), Some(code), "{code}");
            assert!(section_name(code).is_some(), "{code} has no name");
        }
        assert_eq!(section_name("EMA"), Some("Eastern Massachusetts"));
        assert_eq!(section_name("OR"), Some("Oregon"));
        assert_eq!(section_name("NL"), Some("Newfoundland and Labrador"));
        assert_eq!(section_name("MDC"), Some("Maryland and DC"));
        // Every name given is a section's.
        for (code, _) in NAMES {
            assert!(listed_section(code).is_some(), "{code}");
        }
    }

    /// RAC's changes of 2023: the Maritimes split into New Brunswick and
    /// Nova Scotia, the Northwest Territories became the Territories, and
    /// the Greater Toronto Area the Golden Horseshoe, beside Ontario's
    /// other three.
    #[test]
    fn the_sections_are_those_after_racs_2023_changes() {
        for code in ["NB", "NS", "TER", "GH", "ONE", "ONN", "ONS"] {
            assert!(listed_section(code).is_some(), "{code}");
        }
        for old in ["MAR", "NT", "GTA", "ON"] {
            assert_eq!(listed_section(old), None, "{old}");
        }
    }

    /// Yukon's code reads as itself and counts as the Territories; DX is
    /// DX; nothing else is a section, an old place code among them.
    #[test]
    fn yukon_is_the_territories_and_no_other_alias_is_read() {
        assert_eq!(listed_section("YT"), Some("YT"));
        assert_eq!(fold_yukon("YT"), "TER");
        assert_eq!(fold_yukon("TER"), "TER");
        assert_eq!(fold_yukon("EMA"), "EMA");
        assert_eq!(listed_section("DX"), Some("DX"));
        assert_eq!(section_name("YT"), None);
        assert_eq!(section_name("DX"), None);
        // NFL is Northern Florida, not Newfoundland's old code.
        assert_eq!(listed_section("NFL"), Some("NFL"));
        assert_eq!(section_name("NFL"), Some("Northern Florida"));
        for none in [
            "", "NF", "PQ", "YUK", "MD", "DC", "TX", "CA", "HI", "ema", "EMAX",
        ] {
            assert_eq!(listed_section(none), None, "{none:?}");
        }
    }

    /// The counted shares and the list agree: every place counted is a
    /// state, province or territory, its sections listed ones that add up to
    /// every one of its stations; every section is some place's, but Puerto
    /// Rico's and the Virgin Islands', which a station's call gives; and
    /// every state, DC, province and territory is counted.
    #[test]
    fn every_place_has_sections_whose_shares_add_up() {
        let mut covered = Vec::new();
        for (place, sections) in PLACE_SECTIONS {
            assert!(place_code(place).is_some(), "{place} is no place");
            let total: f64 = sections.iter().map(|(_, share)| share).sum();
            assert!((total - 100.0).abs() < 1e-9, "{place}: {total}");
            for (section, share) in *sections {
                assert!(listed_section(section).is_some(), "{place}: {section}");
                assert!(*share > 0.0, "{place}: {section} at {share}");
                covered.push(*section);
            }
        }
        for code in listed() {
            assert_eq!(
                covered.contains(&code),
                !matches!(code, "PR" | "VI"),
                "{code}"
            );
        }
        assert_eq!(PLACE_SECTIONS.len(), 64, "50 states, DC and Canada's 13");
        let share = |place: &str, section: &str| {
            PLACE_SECTIONS
                .iter()
                .find(|(listed, _)| *listed == place)
                .and_then(|(_, sections)| sections.iter().find(|(code, _)| *code == section))
                .map(|(_, share)| *share)
        };
        // Texas's three sections under Texas, DC under MDC with Maryland,
        // and the territories under TER.
        assert!(share("TX", "NTX").is_some() && share("TX", "STX").is_some());
        assert!(share("TX", "WTX").is_some());
        assert_eq!(share("DC", "MDC"), Some(100.0));
        assert_eq!(share("MD", "MDC"), Some(100.0));
        for territory in ["NT", "NU", "YT"] {
            assert_eq!(share(territory, "TER"), Some(100.0), "{territory}");
        }
        assert_eq!(share("HI", "PAC"), Some(100.0));
        assert_eq!(share("AK", "AK"), Some(100.0));
        // A state of one section is all in it.
        assert_eq!(share("OR", "OR"), Some(100.0));
    }
}
