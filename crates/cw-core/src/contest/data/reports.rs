//! The signal reports callers give in an everyday QSO, where the report is
//! copied rather than prefilled: Dust's estimate of how often each is sent,
//! written for Dust (§11.5 of the design). Most stations are given 599 out
//! of habit; the rest spread down the strengths a QSO on a quiet band hears.

use crate::callsign::pick_weighted;
use crate::rng::Rng;

/// Each report, with how often it is sent, in percent.
const REPORTS: &[(u32, f64)] = &[
    (599, 40.0),
    (579, 20.0),
    (589, 15.0),
    (569, 10.0),
    (559, 8.0),
    (449, 4.0),
    (339, 3.0),
];

/// A report a caller gives, from `rng`.
pub(in crate::contest) fn draw_report(rng: &mut impl Rng) -> u32 {
    pick_weighted(REPORTS, rng).copied().unwrap_or(599)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;

    /// The shares add up to every report, and each comes about as often as
    /// its share says.
    #[test]
    fn reports_come_as_often_as_their_shares() {
        let total: f64 = REPORTS.iter().map(|(_, share)| share).sum();
        assert!((total - 100.0).abs() < 1e-9);
        let mut rng = SplitMix64::new(9);
        let draws: Vec<u32> = (0..20_000).map(|_| draw_report(&mut rng)).collect();
        for (report, share) in REPORTS {
            let seen = draws.iter().filter(|drawn| *drawn == report).count() as f64;
            let seen = seen / draws.len() as f64 * 100.0;
            assert!(
                (seen - share).abs() < 1.5,
                "{report}: {seen:.1}% for {share}%"
            );
        }
    }
}
