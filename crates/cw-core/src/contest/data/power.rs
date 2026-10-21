//! The power DX stations send in the ARRL DX contest's exchange, as they
//! send it: `100`, `KW`, `K`.
//!
//! The shares were counted from contest-log summaries of 3,779 DX stations,
//! as last edited in March 2025 (§11.5 of the design): counts only, no
//! calls, rounded to a tenth of a percent. A hundred watts and a kilowatt are most of them; `KW` and `K`
//! are each kept as stations send them, as are the odd powers in the tail.

use crate::callsign::pick_weighted;
use crate::rng::Rng;

/// Each power as it is sent, with its share of DX stations, in percent.
#[rustfmt::skip]
const POWERS: &[(&str, f64)] = &[
    ("100", 34.7), ("KW", 25.0), ("500", 12.8), ("K", 10.9), ("400", 4.6),
    ("200", 3.0), ("5", 2.4),
    // The others, 6.6% between them.
    ("300", 1.2), ("50", 1.2), ("700", 1.0), ("99", 0.4), ("600", 0.4),
    ("750", 0.4), ("150", 0.3), ("1K", 0.3), ("800", 0.2), ("80", 0.2),
    ("70", 0.1), ("10", 0.1), ("90", 0.1), ("75", 0.1), ("250", 0.1),
    ("25", 0.1), ("20", 0.1), ("95", 0.1),
];

/// The power a DX caller sends, from `rng`.
pub(in crate::contest) fn draw_power(rng: &mut impl Rng) -> &'static str {
    pick_weighted(POWERS, rng).copied().unwrap_or("100")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::Field;
    use crate::rng::SplitMix64;

    /// The counted shares: a hundred watts a third of the time, a kilowatt
    /// written two ways a third more, and the others a few percent between
    /// them; every power one the field reads.
    #[test]
    fn powers_come_as_often_as_the_logs_had_them() {
        let share = |power: &str| {
            POWERS
                .iter()
                .find(|(each, _)| *each == power)
                .map_or(0.0, |(_, share)| *share)
        };
        for (power, counted) in [
            ("100", 34.7),
            ("KW", 25.0),
            ("500", 12.8),
            ("K", 10.9),
            ("400", 4.6),
            ("200", 3.0),
            ("5", 2.4),
        ] {
            assert_eq!(share(power), counted, "{power}");
        }
        let others: f64 = POWERS[7..].iter().map(|(_, share)| share).sum();
        assert!((others - 6.6).abs() < 0.3, "{others:.1}% others");
        let total: f64 = POWERS.iter().map(|(_, share)| share).sum();
        assert!((total - 100.0).abs() < 0.3, "{total:.1}%");
        for (power, _) in POWERS {
            assert!(Field::Pwr.canonical(power).is_some(), "{power}");
        }
        let mut rng = SplitMix64::new(12);
        let draws: Vec<&str> = (0..20_000).map(|_| draw_power(&mut rng)).collect();
        for (power, counted) in &POWERS[..7] {
            let seen = draws.iter().filter(|drawn| *drawn == power).count() as f64;
            let seen = seen / draws.len() as f64 * 100.0;
            assert!(
                (seen - counted).abs() < 1.5,
                "{power}: {seen:.1}% for {counted}%"
            );
        }
    }
}
