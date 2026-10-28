//! Where a contest's callers sit, and how loud they come in (§5.4 of the
//! design).
//!
//! A group session's pile-up is built around the one station you want,
//! which has to stay loudest and nearest the middle
//! ([`crate::Transmission::calling`]). A contest has no such station: every
//! caller is one you might work. So each is placed against the receiver
//! alone. A single caller sits near the centre, as MorseRunner's Single
//! Calls do. A pile-up spreads out as far as the Band card's spread lets it,
//! no further than the filter's skirt passes a caller, and keeps its callers
//! apart, so that what the ear hears is several stations rather than one
//! that beats. How many the filter has room for is its capacity; the rest
//! wait.

use super::RunStyle;
use crate::band::FilterDesign;
use crate::rng::Rng;
use crate::settings::{
    PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN, SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ, TrainingSettings,
};
use crate::timing::{PILEUP_CALLER_SEPARATION_HZ, PILEUP_SKIRT_DB, free_room, pick_in_room};

/// How far either side of the receiver's centre a single caller sits
/// (`DxStn.pas:102-105`).
const SINGLE_PITCH_SPREAD_HZ: f64 = 50.0;

/// How many places a pile-up caller is offered before it takes the lowest
/// one left, which always leaves room for the rest.
const PLACE_TRIES: usize = 8;

/// Where a caller sits, and how loud it is before the receiver.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placement {
    pub(super) tone_hz: f64,
    pub(super) volume: f64,
}

/// The pitches a pile-up caller may take: no further from the centre than
/// the spread either side, and no further down the filter's skirt than
/// [`PILEUP_SKIRT_DB`], where a caller is still plainly heard.
fn pileup_room(settings: &TrainingSettings) -> (f64, f64) {
    let receiver = FilterDesign::from_settings(settings);
    let centre = settings.side_tone_center();
    let spread = settings
        .band
        .pileup_spread_hz
        .clamp(PILEUP_SPREAD_MIN, PILEUP_SPREAD_MAX);
    let (reach_low, reach_high) = receiver.edges_at(PILEUP_SKIRT_DB);
    (
        (centre - spread).max(reach_low),
        (centre + spread).min(reach_high),
    )
}

/// How many callers fit in `pieces` of room, each
/// [`PILEUP_CALLER_SEPARATION_HZ`] from the next: one at each end of a
/// piece, and as many as the separation allows between. A piece a rounding
/// error short of a whole number of separations holds that number.
fn fits(pieces: &[(f64, f64)]) -> usize {
    pieces
        .iter()
        .map(|(low, high)| ((high - low) / PILEUP_CALLER_SEPARATION_HZ + 1e-9).floor() as usize + 1)
        .sum()
}

/// How many pile-up callers the receiver `settings` build has room for at
/// once: the width of the room they may take, over the separation they
/// keep, and one more, as a fence has a post more than it has gaps. About
/// six through the default 300 Hz filter.
pub fn filter_capacity(settings: &TrainingSettings) -> usize {
    let (low, high) = pileup_room(settings);
    if high < low { 0 } else { fits(&[(low, high)]) }
}

/// A place for the next caller of a run of `style`, beside the pile-up
/// callers already at `taken`; none when the filter has no room left for
/// one, and it waits. Its strength is U-shaped across the volume range, as
/// MorseRunner draws it (`DxStn.pas:101`): most callers are strong or weak,
/// fewer in between, and none under the range's floor.
pub(super) fn place_contest_caller(
    settings: &TrainingSettings,
    style: RunStyle,
    taken: &[f64],
    rng: &mut impl Rng,
) -> Option<Placement> {
    let tone_hz = match style {
        RunStyle::Single => {
            let centre = settings.side_tone_center();
            rng.pick_in_range(
                centre - SINGLE_PITCH_SPREAD_HZ,
                centre + SINGLE_PITCH_SPREAD_HZ,
            )
            .clamp(SIDE_TONE_MIN_HZ, SIDE_TONE_MAX_HZ)
        }
        RunStyle::Pileup => pileup_pitch(settings, taken, rng)?,
    };
    let range = settings.volume_range_db();
    let (floor, top) = (*range.start(), *range.end());
    // MorseRunner's `RndUShaped`, `sin(π(u − ½))`, from −1 to 1, across
    // the range in decibels, as it is shown.
    let swing = (std::f64::consts::PI * (rng.f64() - 0.5)).sin();
    let volume = settings.volume_at_db(floor + (top - floor) * (1.0 + swing) / 2.0);
    Some(Placement { tone_hz, volume })
}

/// How far either side of the receiver's centre a station sending QRM on
/// your frequency strays, at most, as MorseRunner's do (`QrmStn.pas`):
/// nearer the centre more often than not.
const QRM_PITCH_SPREAD_HZ: f64 = 300.0;

/// A place for a station sending QRM on your frequency: near your pitch,
/// within what the filter passes plainly, so it is heard as callers are,
/// and anywhere across the volume range, as MorseRunner draws a QRM
/// station's strength evenly (`QrmStn.pas`).
pub(super) fn place_qrm(settings: &TrainingSettings, rng: &mut impl Rng) -> Placement {
    let receiver = FilterDesign::from_settings(settings);
    let centre = settings.side_tone_center();
    let (reach_low, reach_high) = receiver.edges_at(PILEUP_SKIRT_DB);
    let reach = QRM_PITCH_SPREAD_HZ
        .min(centre - reach_low)
        .min(reach_high - centre)
        .max(0.0);
    let tone_hz = rng
        .normal_within(centre, reach)
        .clamp(SIDE_TONE_MIN_HZ, SIDE_TONE_MAX_HZ);
    let range = settings.volume_range_db();
    let volume = settings.volume_at_db(rng.pick_in_range(*range.start(), *range.end()));
    Placement { tone_hz, volume }
}

/// A pile-up caller's pitch, somewhere in the room the callers at `taken`
/// leave, and never where it would leave too little for the filter's
/// capacity: placed at random, callers would otherwise wedge the room shut
/// well before it was full.
fn pileup_pitch(settings: &TrainingSettings, taken: &[f64], rng: &mut impl Rng) -> Option<f64> {
    let room = pileup_room(settings);
    let free = free_room(room, taken);
    let lowest = free.first()?.0;
    let leave = filter_capacity(settings).saturating_sub(taken.len() + 1);
    let leaves_room = |pitch: f64| {
        let mut placed = taken.to_vec();
        placed.push(pitch);
        fits(&free_room(room, &placed)) >= leave
    };
    // The lowest place left takes only one caller's room from what is left,
    // so it is always one that leaves enough.
    let pitch = (0..PLACE_TRIES)
        .map(|_| pick_in_room(&free, rng))
        .find(|pitch| leaves_room(*pitch))
        .unwrap_or(lowest);
    Some(pitch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;
    use crate::settings::{DEFAULT_FILTER_WIDTH_HZ, FILTER_WIDTH_MAX, FILTER_WIDTH_MIN};

    fn settings(width: f64) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.band.filter_width_hz = width;
        s.clamp()
    }

    /// Callers placed one after another until the filter has no room left.
    fn fill(settings: &TrainingSettings, seed: u64) -> Vec<Placement> {
        let mut rng = SplitMix64::new(seed);
        let mut placed: Vec<Placement> = Vec::new();
        while let Some(next) = place_contest_caller(
            settings,
            RunStyle::Pileup,
            &placed.iter().map(|p| p.tone_hz).collect::<Vec<_>>(),
            &mut rng,
        ) {
            placed.push(next);
            assert!(placed.len() < 100, "the room never filled");
        }
        placed
    }

    /// Every pile-up caller lands within the spread, where the filter's
    /// skirt still passes it, and at least sixty hertz from every other.
    #[test]
    fn pile_up_callers_land_inside_the_reach_and_apart() {
        for width in [
            FILTER_WIDTH_MIN,
            DEFAULT_FILTER_WIDTH_HZ,
            600.0,
            FILTER_WIDTH_MAX,
        ] {
            let s = settings(width);
            let receiver = FilterDesign::from_settings(&s);
            let skirt = 10f64.powf(PILEUP_SKIRT_DB / 20.0);
            let centre = s.side_tone_center();
            for seed in 0..100 {
                let placed = fill(&s, seed);
                for (at, caller) in placed.iter().enumerate() {
                    assert!(
                        (caller.tone_hz - centre).abs() <= s.band.pileup_spread_hz + 1e-9,
                        "{width} Hz, seed {seed}: {:.0} Hz strayed",
                        caller.tone_hz
                    );
                    assert!(
                        receiver.response_at(caller.tone_hz) >= skirt * (1.0 - 1e-6),
                        "{width} Hz, seed {seed}: {:.0} Hz is under the skirt",
                        caller.tone_hz
                    );
                    for other in &placed[..at] {
                        let apart = (caller.tone_hz - other.tone_hz).abs();
                        assert!(
                            apart >= PILEUP_CALLER_SEPARATION_HZ - 1e-9,
                            "{width} Hz, seed {seed}: two callers {apart:.1} Hz apart"
                        );
                    }
                }
            }
        }
    }

    /// The filter fits its reach over sixty hertz, and one more: about six
    /// through the default filter, more as it opens up, never fewer than
    /// one. And that many always fit, however the first of them land.
    #[test]
    fn capacity_follows_the_reach_over_sixty_hertz_and_one() {
        let default = filter_capacity(&settings(DEFAULT_FILTER_WIDTH_HZ));
        assert!((5..=7).contains(&default), "{default} through 300 Hz");
        let widths = [
            FILTER_WIDTH_MIN,
            DEFAULT_FILTER_WIDTH_HZ,
            500.0,
            FILTER_WIDTH_MAX,
        ];
        let capacities = widths.map(|width| filter_capacity(&settings(width)));
        assert!(
            capacities.windows(2).all(|w| w[0] <= w[1]),
            "{capacities:?}"
        );
        assert!(capacities[0] >= 1);
        for width in widths {
            let s = settings(width);
            let (low, high) = pileup_room(&s);
            let expected = ((high - low) / PILEUP_CALLER_SEPARATION_HZ).floor() as usize + 1;
            assert_eq!(filter_capacity(&s), expected, "{width} Hz");
            for seed in 0..50 {
                assert_eq!(fill(&s, seed).len(), expected, "{width} Hz, seed {seed}");
            }
        }
        // A wide spread is held in by the filter; a narrow one by itself.
        let mut narrow = settings(FILTER_WIDTH_MAX);
        narrow.band.pileup_spread_hz = PILEUP_SPREAD_MIN;
        let (low, high) = pileup_room(&narrow);
        assert!((high - low - 2.0 * PILEUP_SPREAD_MIN).abs() < 1e-9);
    }

    /// A single caller sits within fifty hertz of the centre, whoever else
    /// is on the frequency.
    #[test]
    fn a_single_caller_sits_near_the_centre() {
        let s = settings(DEFAULT_FILTER_WIDTH_HZ);
        let mut rng = SplitMix64::new(4);
        for _ in 0..500 {
            let placed = place_contest_caller(&s, RunStyle::Single, &[500.0], &mut rng)
                .expect("a single caller always has its place");
            assert!((placed.tone_hz - s.side_tone_center()).abs() <= SINGLE_PITCH_SPREAD_HZ);
        }
    }

    /// Strength is U-shaped across the volume range in decibels, as it is
    /// shown, the way MorseRunner draws it: two thirds of callers in its
    /// outer quarters, and none below its floor or above its top.
    #[test]
    fn strength_is_u_shaped_and_never_below_the_floor() {
        let mut s = settings(DEFAULT_FILTER_WIDTH_HZ);
        s.band.volume_min = 0.2;
        s.band.volume_max = 1.0;
        let mut s = s.clamp();
        let mut rng = SplitMix64::new(9);
        let volumes: Vec<f64> = (0..20_000)
            .filter_map(|_| place_contest_caller(&s, RunStyle::Single, &[], &mut rng))
            .map(|placed| placed.volume)
            .collect();
        assert!(volumes.iter().all(|v| (0.2..=1.0).contains(v)));
        let floor = crate::band::gain_to_db(0.2);
        let quarter = -floor / 4.0;
        let outer = volumes
            .iter()
            .map(|v| crate::band::gain_to_db(*v))
            .filter(|db| *db < floor + quarter || *db > -quarter)
            .count();
        let share = outer as f64 / volumes.len() as f64;
        assert!(
            (share - 2.0 / 3.0).abs() < 0.02,
            "outer quarters {share:.3}"
        );
        // Linked, every caller comes in at the one level.
        s.band.link_volume = true;
        let linked = place_contest_caller(&s.clamp(), RunStyle::Single, &[], &mut rng);
        assert_eq!(linked.map(|placed| placed.volume), Some(0.2));
    }

    /// A station sending QRM sits near your pitch, within what the filter
    /// passes plainly, more often near than far, and anywhere across the
    /// volume range.
    #[test]
    fn qrm_sits_near_your_pitch_inside_the_reach() {
        for width in [FILTER_WIDTH_MIN, DEFAULT_FILTER_WIDTH_HZ, FILTER_WIDTH_MAX] {
            let s = settings(width);
            let (low, high) = FilterDesign::from_settings(&s).edges_at(PILEUP_SKIRT_DB);
            let centre = s.side_tone_center();
            let range = s.volume_range();
            let mut rng = SplitMix64::new(11);
            let placed: Vec<Placement> = (0..2_000).map(|_| place_qrm(&s, &mut rng)).collect();
            for at in &placed {
                assert!(
                    (low - 1e-6..=high + 1e-6).contains(&at.tone_hz),
                    "{width}: {at:?}"
                );
                assert!((at.tone_hz - centre).abs() <= QRM_PITCH_SPREAD_HZ + 1e-6);
                assert!(range.contains(&at.volume), "{width}: {at:?}");
            }
            let reach = (centre - low).min(high - centre).min(QRM_PITCH_SPREAD_HZ);
            let near = placed
                .iter()
                .filter(|at| (at.tone_hz - centre).abs() < reach / 2.0)
                .count();
            assert!(near > 1_200, "{width}: only {near} of 2000 near your pitch");
        }
    }
}
