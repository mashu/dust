//! How long a group you can copy whole: a Bayesian estimate of your copy span,
//! and group lengths drawn near its edge.
//!
//! The model is a psychometric curve over group length, held as a grid
//! belief ([`crate::belief`]). A group of `L` characters is copied whole with
//! probability
//!
//! ```text
//! P(whole | L) = (1 − λ) · σ((S − L) / w)
//! ```
//!
//! `S` is the span, the length copied whole about half the time; `w` is how
//! sharply that falls away past it; `λ` is a small lapse rate, so one slip on
//! a short group is not read as a short span.
//!
//! Each length is then drawn by Thompson sampling: one `(S, w)` is drawn from
//! the posterior, and the draw leans towards the lengths whose chance of
//! being copied whole brings the groups drawn to the target
//! ([`crate::aim`]). While the posterior is wide that explores; as it
//! narrows it settles.

use std::sync::OnceLock;

use crate::aim::{self, Leaning, Spread};
use crate::alignment::CopyTally;
use crate::belief::{Estimate, EstimatePoint, GridBelief, Tally, sessions_in_order};
use crate::rng::Rng;
use crate::session::SessionResult;
use crate::settings::{GROUP_SIZE_MAX, TrainingSettings};

/// The chance of fumbling a group however short it is.
const LAPSE: f64 = 0.03;
/// The spans the grid holds: 0.5 to 20.5 characters, a tenth apart.
const SPAN_FROM: f64 = 0.5;
const SPAN_STEP: f64 = 0.1;
const SPAN_POINTS: usize = 201;
/// How sharply copying falls away past the span, in characters.
const SPREADS: [f64; 5] = [0.4, 0.7, 1.0, 1.5, 2.5];
const CELLS: usize = SPAN_POINTS * SPREADS.len();
/// How far the first guess at a span may be off, in characters.
const PRIOR_SD: f64 = 3.0;
/// How the lengths lean towards the aim, in characters: within about one of
/// it, a tenth of them spread evenly over the whole range so every length in
/// it still comes round now and then, and the aim free to go three past
/// either end.
const LEANING: Leaning = Leaning {
    width: 1.0,
    variety: 0.1,
    spread: Spread::Even,
    reach: 3.0,
};

fn span_of(cell: usize) -> f64 {
    SPAN_FROM + (cell / SPREADS.len()) as f64 * SPAN_STEP
}

fn spread_of(cell: usize) -> f64 {
    SPREADS[cell % SPREADS.len()]
}

/// The chance of copying a group of `length` whole, for one span and spread.
fn p_whole(length: f64, span: f64, spread: f64) -> f64 {
    (1.0 - LAPSE) / (1.0 + ((length - span) / spread).exp())
}

/// `ln P(whole)` and `ln P(not whole)` for a group of `length`, for one cell.
fn log_chances(length: f64, cell: usize) -> [f64; 2] {
    let p = p_whole(length, span_of(cell), spread_of(cell)).clamp(1e-12, 1.0 - 1e-12);
    [p.ln(), (1.0 - p).ln()]
}

/// [`log_chances`] for every length a group can be and every cell, worked
/// out once, so that replaying a history is additions rather than
/// logarithms.
fn log_chance_table() -> &'static [[f64; 2]] {
    static TABLE: OnceLock<Vec<[f64; 2]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (1..=GROUP_SIZE_MAX as usize)
            .flat_map(|length| (0..CELLS).map(move |cell| log_chances(length as f64, cell)))
            .collect()
    })
}

/// One group length in the range, as the next group sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LengthForecast {
    pub length: usize,
    /// The chance this is the next group's length.
    pub drawn: f64,
    /// The chance a group this long is copied whole, and how far the
    /// posterior spreads it.
    pub whole: f64,
    pub whole_sd: f64,
}

/// What the history and this session say about your copy span.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanBelief {
    belief: GridBelief,
}

impl SpanBelief {
    /// A belief with nothing learned yet, its first guess at the span being
    /// `centre` characters.
    pub fn new(centre: f64) -> Self {
        let prior = (0..CELLS)
            .map(|cell| -0.5 * ((span_of(cell) - centre) / PRIOR_SD).powi(2))
            .collect();
        Self {
            belief: GridBelief::new(prior),
        }
    }

    /// The belief the history leaves for `settings`, its first guess the
    /// middle of the group size range.
    pub fn from_history(history: &[SessionResult], settings: &TrainingSettings) -> Self {
        Self::replay(history, settings, |_, _| {})
    }

    /// The belief a new session starts from: the history's, with all of it
    /// one session older, so the last session counts for a little less than
    /// the one about to be sent.
    pub fn for_session(history: &[SessionResult], settings: &TrainingSettings) -> Self {
        let mut span = Self::from_history(history, settings);
        span.belief.fade();
        span
    }

    /// The span after each session of the history that counts towards it,
    /// oldest first.
    pub fn history(history: &[SessionResult], settings: &TrainingSettings) -> Vec<EstimatePoint> {
        let mut points = Vec::new();
        Self::replay(history, settings, |session, span| {
            points.push(EstimatePoint {
                timestamp: session.timestamp,
                estimate: span.estimate(),
            });
        });
        points
    }

    /// Learn from the history in the order it was practised, telling `after`
    /// where the belief stands after each session. Only sessions of the
    /// character set `settings` trains count: a callsign is as long as it
    /// is, and digits are not held the way letters are.
    fn replay(
        history: &[SessionResult],
        settings: &TrainingSettings,
        mut after: impl FnMut(&SessionResult, &Self),
    ) -> Self {
        let (shortest, longest) = crate::sampling::group_size_bounds(settings);
        let mut span = Self::new((shortest + longest) as f64 / 2.0);
        let mode = settings.curriculum.char_set_mode;
        if !mode.draws_characters() {
            return span;
        }
        for session in sessions_in_order(history, mode) {
            span.belief.fade();
            let mut tally = Tally::new();
            for group in &session.groups {
                let whole = f64::from(u8::from(group.correct));
                tally.add(group.sent.chars().count(), whole, 1.0 - whole);
            }
            for (length, whole, missed) in tally.rows() {
                span.observe(length, whole, missed);
            }
            after(session, &span);
        }
        span
    }

    /// Groups of `length` characters: `whole` of them copied whole, `missed`
    /// not.
    fn observe(&mut self, length: usize, whole: f64, missed: f64) {
        if length == 0 || whole + missed <= 0.0 {
            return;
        }
        // A length past the table, from a save older than today's limit, is
        // worked out as it comes.
        let row = log_chance_table().get((length - 1) * CELLS..length * CELLS);
        let chances = (0..CELLS).map(|cell| {
            row.and_then(|row| row.get(cell).copied())
                .unwrap_or_else(|| log_chances(length as f64, cell))
        });
        self.belief.observe(chances, whole, missed);
    }

    /// Learn from one group of `length` characters.
    pub fn learn(&mut self, length: usize, whole: bool) {
        let whole = f64::from(u8::from(whole));
        self.observe(length, whole, 1.0 - whole);
    }

    /// Learn from what was copied of one group: how long it was, and whether
    /// every character came back with nothing added.
    pub fn learn_copy(&mut self, copied: &CopyTally) {
        let sent: u32 = copied.sent.values().map(|stats| stats.total).sum();
        let right: u32 = copied.sent.values().map(|stats| stats.correct).sum();
        self.learn(sent as usize, right == sent && copied.inserted.is_empty());
    }

    /// The span, in characters.
    pub fn estimate(&self) -> Estimate {
        self.belief.estimate(span_of)
    }

    /// The next group's length, between `shortest` and `longest` characters,
    /// aiming for `target` of groups copied whole.
    pub fn draw_length(
        &self,
        shortest: usize,
        longest: usize,
        target: f64,
        rng: &mut impl Rng,
    ) -> usize {
        let (shortest, longest) = (shortest.min(longest), shortest.max(longest));
        if shortest == longest {
            return shortest;
        }
        let cell = self.belief.draw(rng);
        let weights = length_weights(shortest, longest, target, cell);
        shortest + crate::belief::pick_index(&weights, rng)
    }

    /// Each length between `shortest` and `longest`: how likely it is to be
    /// drawn next, and how likely it is to be copied whole, over everything
    /// the posterior allows. With no `target` the lengths are drawn evenly,
    /// and the leaning is not worked out at all.
    pub fn forecast(
        &self,
        shortest: usize,
        longest: usize,
        target: Option<f64>,
    ) -> Vec<LengthForecast> {
        let (shortest, longest) = (shortest.min(longest), shortest.max(longest));
        let even = 1.0 / (longest - shortest + 1) as f64;
        let mut rows: Vec<LengthForecast> = (shortest..=longest)
            .map(|length| LengthForecast {
                length,
                drawn: if target.is_some() { 0.0 } else { even },
                whole: 0.0,
                whole_sd: 0.0,
            })
            .collect();
        // The second moment of each chance whole, for its spread.
        let mut squares = vec![0.0; rows.len()];
        for (cell, p) in self.belief.posterior().into_iter().enumerate() {
            if p < 1e-12 {
                continue;
            }
            if let Some(target) = target {
                let weights = length_weights(shortest, longest, target, cell);
                for (row, weight) in rows.iter_mut().zip(weights) {
                    row.drawn += p * weight;
                }
            }
            for (row, square) in rows.iter_mut().zip(&mut squares) {
                let whole = p_whole(row.length as f64, span_of(cell), spread_of(cell));
                row.whole += p * whole;
                *square += p * whole * whole;
            }
        }
        for (row, square) in rows.iter_mut().zip(squares) {
            row.whole_sd = (square - row.whole * row.whole).max(0.0).sqrt();
        }
        rows
    }
}

/// How strongly each length from `shortest` to `longest` is favoured for one
/// cell of the posterior: leaning towards an aim set so that, over the
/// lengths drawn, groups are copied whole `target` of the time.
fn length_weights(shortest: usize, longest: usize, target: f64, cell: usize) -> Vec<f64> {
    let (span, spread) = (span_of(cell), spread_of(cell));
    let lengths: Vec<f64> = (shortest..=longest).map(|length| length as f64).collect();
    let whole: Vec<f64> = lengths
        .iter()
        .map(|length| p_whole(*length, span, spread))
        .collect();
    aim::lean(&lengths, &whole, target, &LEANING)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;
    use crate::session::GroupResult;
    use crate::settings::CharSetMode;

    /// A learner whose span is `span`: copies a group whole with the model's
    /// own chance, so the estimate has a true value to find.
    fn copies(length: usize, span: f64, spread: f64, rng: &mut impl Rng) -> bool {
        rng.f64() < p_whole(length as f64, span, spread)
    }

    fn session(timestamp: u64, groups: &[(&str, bool)]) -> SessionResult {
        SessionResult {
            timestamp,
            groups: groups
                .iter()
                .map(|(sent, correct)| GroupResult {
                    sent: (*sent).to_string(),
                    received: if *correct {
                        (*sent).to_string()
                    } else {
                        String::new()
                    },
                    correct: *correct,
                })
                .collect(),
            ..SessionResult::empty_for_test()
        }
    }

    impl SpanBelief {
        fn faded(mut self) -> Self {
            self.belief.fade();
            self
        }
    }

    fn koch() -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 10;
        settings
    }

    #[test]
    fn with_nothing_learned_the_guess_is_the_middle_of_the_range() {
        // A little above the centre: the grid stops at half a character,
        // which cuts more off the bottom of the guess than the top.
        let estimate = SpanBelief::new(6.0).estimate();
        assert!((estimate.mean - 6.0).abs() < 0.3, "{estimate:?}");
        assert!(
            estimate.sd > 2.0,
            "a first guess is a wide one: {estimate:?}"
        );
    }

    #[test]
    fn long_groups_copied_whole_raise_the_span_and_misses_lower_it() {
        let mut up = SpanBelief::new(5.0);
        let mut down = SpanBelief::new(5.0);
        for _ in 0..10 {
            up.learn(8, true);
            down.learn(3, false);
        }
        assert!(up.estimate().mean > 8.0, "{:?}", up.estimate());
        assert!(down.estimate().mean < 3.0, "{:?}", down.estimate());
    }

    #[test]
    fn one_slip_on_a_short_group_is_not_a_short_span() {
        let mut belief = SpanBelief::new(5.0);
        for _ in 0..8 {
            belief.learn(6, true);
        }
        let before = belief.estimate().mean;
        belief.learn(2, false);
        assert!(before - belief.estimate().mean < 0.5);
    }

    #[test]
    fn a_copy_is_whole_only_when_nothing_was_missed_or_added() {
        let mut whole = SpanBelief::new(5.0);
        let mut padded = SpanBelief::new(5.0);
        for _ in 0..6 {
            whole.learn_copy(&CopyTally::of("ABCDEFGH", "ABCDEFGH"));
            padded.learn_copy(&CopyTally::of("ABCDEFGH", "ABCDEFGHX"));
        }
        assert!(whole.estimate().mean > padded.estimate().mean + 2.0);
        assert_eq!(whole, {
            let mut by_hand = SpanBelief::new(5.0);
            for _ in 0..6 {
                by_hand.learn(8, true);
            }
            by_hand
        });
    }

    /// A simulated learner of span 6 is found within half a character, and
    /// the groups it is sent come back whole about as often as asked.
    #[test]
    fn a_simulated_learner_is_found_and_copies_near_the_target() {
        let mut rng = SplitMix64::new(11);
        let mut belief = SpanBelief::new(6.0);
        let mut whole = 0;
        for group in 0..300 {
            let length = belief.draw_length(2, 10, 0.7, &mut rng);
            let copied = copies(length, 6.0, 0.8, &mut rng);
            belief.learn(length, copied);
            if group >= 100 {
                whole += u32::from(copied);
            }
        }
        let estimate = belief.estimate();
        assert!((estimate.mean - 6.0).abs() < 0.5, "{estimate:?}");
        let rate = f64::from(whole) / 200.0;
        assert!((0.6..=0.8).contains(&rate), "copied whole {rate}");
    }

    /// For a span that is known, the lengths drawn are copied whole as often
    /// as each target asks, and a range that is all too easy or too hard
    /// leans on the end nearest the learner.
    #[test]
    fn the_lengths_drawn_are_copied_whole_as_often_as_asked() {
        let cell = |span: f64, spread: f64| {
            (0..CELLS)
                .find(|&cell| {
                    (span_of(cell) - span).abs() < 1e-9 && (spread_of(cell) - spread).abs() < 1e-9
                })
                .expect("a cell on the grid")
        };
        let known = cell(6.0, 1.0);
        for target in [0.85, 0.7, 0.5] {
            let weights = length_weights(2, 10, target, known);
            let whole: f64 = (2..=10)
                .zip(&weights)
                .map(|(length, weight)| weight * p_whole(f64::from(length), 6.0, 1.0))
                .sum();
            assert!((whole - target).abs() < 1e-6, "{target}: {whole}");
        }
        let too_easy = length_weights(2, 5, 0.7, cell(15.0, 1.0));
        let too_hard = length_weights(6, 9, 0.7, cell(2.0, 1.0));
        assert!(too_easy[3] > 0.8, "{too_easy:?}");
        assert!(too_hard[0] > 0.8, "{too_hard:?}");
    }

    #[test]
    fn with_no_target_the_forecast_is_even_and_still_says_what_is_copied_whole() {
        let mut belief = SpanBelief::new(5.0);
        belief.learn(5, true);
        let even = belief.forecast(2, 9, None);
        let leaning = belief.forecast(2, 9, Some(0.7));
        assert!(even.iter().all(|row| (row.drawn - 1.0 / 8.0).abs() < 1e-12));
        for (even, leaning) in even.iter().zip(&leaning) {
            assert!((even.whole - leaning.whole).abs() < 1e-12);
        }
    }

    /// The table replaying a history reads from is the curve itself, and a
    /// length past it, from an older save, is still learned from.
    #[test]
    fn the_table_of_chances_is_the_curve_and_lengths_past_it_still_count() {
        for length in [1, 7, GROUP_SIZE_MAX as usize] {
            for cell in [0, 333, CELLS - 1] {
                let from_table = log_chance_table()[(length - 1) * CELLS + cell];
                assert_eq!(from_table, log_chances(length as f64, cell));
            }
        }
        let mut past = SpanBelief::new(5.0);
        past.learn(GROUP_SIZE_MAX as usize + 5, true);
        let mut by_hand = SpanBelief::new(5.0);
        by_hand.belief.observe(
            (0..CELLS).map(|cell| log_chances(f64::from(GROUP_SIZE_MAX + 5), cell)),
            1.0,
            0.0,
        );
        assert_eq!(past, by_hand);
        assert!(past.estimate().mean > SpanBelief::new(5.0).estimate().mean);
    }

    /// The session about to be sent counts for more than the last one, and
    /// a session with no groups in it moves nothing.
    #[test]
    fn a_new_session_puts_the_history_one_session_further_back() {
        let history = [session(1, &[("ABCDEFGH", true); 10])];
        let mut current = SpanBelief::new(6.0);
        for _ in 0..10 {
            current.learn(8, true);
        }
        let stored = SpanBelief::for_session(&history, &koch());
        assert!(stored.estimate().sd > current.estimate().sd);
        assert_eq!(
            stored.estimate(),
            SpanBelief::from_history(&history, &koch())
                .faded()
                .estimate()
        );

        let with_empty = [session(1, &[("ABCDEFGH", true); 10]), session(2, &[])];
        assert_eq!(
            SpanBelief::from_history(&with_empty, &koch()),
            SpanBelief::from_history(&history, &koch())
        );
        assert_eq!(SpanBelief::history(&with_empty, &koch()).len(), 1);
    }

    #[test]
    fn lengths_stay_inside_the_range_whatever_the_generator_does() {
        struct Stuck(f64);
        impl Rng for Stuck {
            fn f64(&mut self) -> f64 {
                self.0
            }
        }
        let mut belief = SpanBelief::new(5.0);
        belief.learn(4, true);
        for stuck in [0.0, 0.5, 0.999_999, 1.0] {
            for (shortest, longest) in [(2, 9), (9, 2), (1, 20), (4, 4)] {
                let length = belief.draw_length(shortest, longest, 0.7, &mut Stuck(stuck));
                let (lo, hi) = (shortest.min(longest), shortest.max(longest));
                assert!((lo..=hi).contains(&length), "{length} outside {lo}..={hi}");
            }
        }
    }

    #[test]
    fn the_forecast_adds_up_and_longer_groups_are_harder() {
        let mut belief = SpanBelief::new(5.0);
        for _ in 0..5 {
            belief.learn(5, true);
            belief.learn(7, false);
        }
        let rows = belief.forecast(2, 9, Some(0.7));
        assert_eq!(rows.len(), 8);
        let drawn: f64 = rows.iter().map(|row| row.drawn).sum();
        assert!((drawn - 1.0).abs() < 1e-9, "{drawn}");
        assert!(rows.windows(2).all(|pair| pair[0].whole > pair[1].whole));
        // The lengths that land nearest the target are the likeliest.
        let likeliest = rows
            .iter()
            .max_by(|a, b| a.drawn.total_cmp(&b.drawn))
            .expect("a row");
        assert!((0.5..0.9).contains(&likeliest.whole), "{likeliest:?}");
    }

    #[test]
    fn the_history_is_learned_oldest_first_and_older_sessions_count_for_less() {
        let run = |from: u64, groups: &[(&str, bool)]| -> Vec<SessionResult> {
            (from..from + 15).map(|at| session(at, groups)).collect()
        };
        let long_ok = [("ABCDEFGHI", true); 10];
        let short_miss = [("ABC", false); 10];
        // Stored newest first: what was practised last must count most.
        let improving = [run(100, &long_ok), run(0, &short_miss)].concat();
        let slipping = [run(100, &short_miss), run(0, &long_ok)].concat();
        let up = SpanBelief::from_history(&improving, &koch()).estimate();
        let down = SpanBelief::from_history(&slipping, &koch()).estimate();
        assert!(up.mean > down.mean + 1.0, "{up:?} vs {down:?}");
    }

    #[test]
    fn only_sessions_of_the_set_being_trained_count() {
        let mut other = session(1, &[("ABCDEFGHIJ", true); 10]);
        other.char_set_mode = CharSetMode::Mixed;
        let belief = SpanBelief::from_history(&[other.clone()], &koch());
        assert_eq!(belief, SpanBelief::new(6.0));
        // A callsign is as long as it is, so callsigns teach nothing.
        let mut calls = koch();
        calls.curriculum.char_set_mode = CharSetMode::Callsign;
        other.char_set_mode = CharSetMode::Callsign;
        assert!(SpanBelief::history(&[other], &calls).is_empty());
    }

    #[test]
    fn the_history_gives_one_point_per_session() {
        let history = [
            session(1, &[("ABCD", true); 5]),
            session(2, &[("ABCDEF", true); 5]),
            session(3, &[("ABCDEFGH", true); 5]),
        ];
        let points = SpanBelief::history(&history, &koch());
        assert_eq!(
            points.iter().map(|p| p.timestamp).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(
            points
                .windows(2)
                .all(|pair| pair[1].estimate.mean > pair[0].estimate.mean)
        );
        assert_eq!(
            points.last().map(|p| p.estimate),
            Some(SpanBelief::from_history(&history, &koch()).estimate())
        );
    }
}
