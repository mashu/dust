//! What a Field Day caller sends: its class, and the ARRL or RAC section it
//! is in.
//!
//! A class is drawn as often as Field Day's entries send it. A section
//! comes from where the caller's call puts it: Alaska's, Hawaii's and the
//! Caribbean islands' prefixes give their own, a station outside the
//! sections sends DX, and in the US's districts and Canada its state,
//! province or territory is drawn first (`places.rs`), then one of that
//! place's sections, as often as its entries are in each. The shares are
//! counted (`counted.rs`).

use super::counted::{CLASSES, PLACE_SECTIONS};
use super::places::{DX, call_section, draw_place};
use crate::callsign::pick_weighted;
use crate::rng::Rng;

/// The class a caller sends, from `rng`.
pub(in crate::contest) fn draw_class(rng: &mut impl Rng) -> &'static str {
    pick_weighted(CLASSES, rng).copied().unwrap_or("1D")
}

/// The section a caller signing `call` sends, from `rng`: the one its call
/// gives by itself, or else one of the sections of the place drawn for it,
/// or DX for a station Dust cannot place.
pub(in crate::contest) fn draw_section(call: &str, rng: &mut impl Rng) -> &'static str {
    if let Some(section) = call_section(call) {
        return section;
    }
    draw_place(call, false, rng)
        .and_then(|place| {
            let (_, sections) = PLACE_SECTIONS.iter().find(|(listed, _)| *listed == place)?;
            pick_weighted(sections, rng).copied()
        })
        .unwrap_or(DX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::Field;
    use crate::contest::data::places::places_for;
    use crate::rng::SplitMix64;

    /// The counted class shares: one transmitter at home four times in ten,
    /// the classes the design names at their shares, the rest about 4%
    /// between them; every class one the field reads.
    #[test]
    fn classes_come_as_often_as_the_entries_had_them() {
        let share = |class: &str| {
            CLASSES
                .iter()
                .find(|(each, _)| *each == class)
                .map_or(0.0, |(_, share)| *share)
        };
        for (class, counted) in [
            ("1D", 41.9),
            ("1E", 14.0),
            ("1B", 13.1),
            ("2A", 6.7),
            ("3A", 6.7),
            ("4A", 3.5),
            ("1A", 2.9),
            ("5A", 1.8),
        ] {
            assert_eq!(share(class), counted, "{class}");
        }
        for class in ["1C", "2D", "2F", "2B", "2E"] {
            assert!((share(class) - 1.0).abs() <= 0.25, "{class}");
        }
        let rest: f64 = CLASSES[13..].iter().map(|(_, share)| share).sum();
        assert!((rest - 4.0).abs() < 0.3, "{rest:.1}% the rest");
        let total: f64 = CLASSES.iter().map(|(_, share)| share).sum();
        assert!((total - 100.0).abs() < 0.3, "{total:.1}%");
        for (class, _) in CLASSES {
            assert_eq!(Field::Cl.canonical(class).as_deref(), Some(*class));
        }
        let mut rng = SplitMix64::new(13);
        let draws: Vec<&str> = (0..20_000).map(|_| draw_class(&mut rng)).collect();
        for (class, counted) in &CLASSES[..8] {
            let seen = draws.iter().filter(|drawn| *drawn == class).count() as f64;
            let seen = seen / draws.len() as f64 * 100.0;
            assert!(
                (seen - counted).abs() < 1.5,
                "{class}: {seen:.1}% for {counted}%"
            );
        }
    }

    /// A caller's section is one of its place's: a W7 call's an Oregon,
    /// Washington or other western state's, Massachusetts' two as often as
    /// they were counted; Alaska's, Hawaii's, Puerto Rico's and the Virgin
    /// Islands' from their prefixes; DX abroad.
    #[test]
    fn a_callers_section_is_one_of_its_places() {
        let mut rng = SplitMix64::new(14);
        for (call, place) in [("W7XYZ", "OR"), ("W1ABC", "MA"), ("VE3ABC", "ON")] {
            let mut seen = std::collections::BTreeSet::new();
            for _ in 0..2_000 {
                seen.insert(draw_section(call, &mut rng));
            }
            let places = places_for(call, false);
            for section in &seen {
                let in_a_place = PLACE_SECTIONS.iter().any(|(listed, sections)| {
                    places.contains(listed) && sections.iter().any(|(code, _)| code == section)
                });
                assert!(in_a_place, "{call} sent {section}");
            }
            let (_, theirs) = PLACE_SECTIONS
                .iter()
                .find(|(listed, _)| *listed == place)
                .expect("a counted place");
            for (section, _) in *theirs {
                assert!(seen.contains(section), "{call} never sent {section}");
            }
        }
        let east = (0..10_000)
            .filter(|_| draw_section("W1ABC", &mut rng) == "EMA")
            .count();
        let west = (0..10_000)
            .filter(|_| draw_section("W1ABC", &mut rng) == "WMA")
            .count();
        assert!(east > 2 * west, "{east} EMA, {west} WMA");
        for (call, section) in [
            ("KL7AB", "AK"),
            ("KH6AB", "PAC"),
            ("KP4AB", "PR"),
            ("KP2AB", "VI"),
            ("VE8AB", "TER"),
            ("VY1AB", "TER"),
            ("VO2AB", "NL"),
            ("K3DC", "MDC"),
            ("DL1ABC", "DX"),
            ("C31AB", "DX"),
        ] {
            // K3DC is in the third district, so Maryland, Delaware,
            // Pennsylvania or DC: only MDC is checked to come up.
            let seen: Vec<&str> = (0..500).map(|_| draw_section(call, &mut rng)).collect();
            assert!(seen.contains(&section), "{call}: {seen:?}");
            if call != "K3DC" {
                assert!(seen.iter().all(|seen| *seen == section), "{call}");
            }
        }
    }
}
