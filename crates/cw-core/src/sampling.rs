//! Error-weighted + coverage-balanced character sampling for group generation.

use std::collections::BTreeMap;

use crate::alignment::align_group;
use crate::pool::{compute_char_pool, digits_subset, letters_subset};
use crate::rng::{weighted_random_pick, Rng};
use crate::settings::{CharSetMode, TrainingSettings};

pub const CHAR_SAMPLING_PRIOR_ALPHA: f64 = 1.0;
pub const CHAR_SAMPLING_PRIOR_BETA: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharBetaBelief {
    pub alpha: f64,
    pub beta: f64,
}

impl Default for CharBetaBelief {
    fn default() -> Self {
        Self {
            alpha: CHAR_SAMPLING_PRIOR_ALPHA,
            beta: CHAR_SAMPLING_PRIOR_BETA,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharSamplingState {
    pub beliefs: BTreeMap<char, CharBetaBelief>,
    pub session_sample_counts: BTreeMap<char, u32>,
}

#[derive(Clone, Debug)]
pub struct CharSamplingConfig {
    pub error_weight_strength: f64,
    pub coverage_strength: f64,
    pub thompson_sampling: bool,
    pub mixed_letters_percent: u32,
    pub char_set_mode: CharSetMode,
}

pub fn config_from_settings(settings: &TrainingSettings) -> CharSamplingConfig {
    CharSamplingConfig {
        error_weight_strength: settings.auto_level.error_weight_strength,
        coverage_strength: settings.auto_level.char_sampling_coverage_strength,
        thompson_sampling: settings.auto_level.char_sampling_thompson,
        mixed_letters_percent: settings.curriculum.mixed_letters_percent,
        char_set_mode: settings.curriculum.char_set_mode,
    }
}

pub fn belief_for(state: &CharSamplingState, character: char) -> CharBetaBelief {
    state.beliefs.get(&character).copied().unwrap_or_default()
}

pub fn aggregate_historical_beliefs(
    letter_accuracy: &[&BTreeMap<char, crate::alignment::LetterAccuracy>],
) -> BTreeMap<char, CharBetaBelief> {
    let mut beliefs = BTreeMap::new();
    for session in letter_accuracy {
        for (character, stats) in *session {
            let errors = stats.total.saturating_sub(stats.correct);
            let existing: CharBetaBelief = beliefs.get(character).copied().unwrap_or_default();
            beliefs.insert(
                *character,
                CharBetaBelief {
                    alpha: existing.alpha + f64::from(stats.correct),
                    beta: existing.beta + f64::from(errors),
                },
            );
        }
    }
    beliefs
}

pub fn create_initial_sampling_state(
    letter_accuracy: &[&BTreeMap<char, crate::alignment::LetterAccuracy>],
) -> CharSamplingState {
    CharSamplingState {
        beliefs: aggregate_historical_beliefs(letter_accuracy),
        session_sample_counts: BTreeMap::new(),
    }
}

pub fn beta_posterior_mean_error(belief: CharBetaBelief) -> f64 {
    let denom = belief.alpha + belief.beta;
    if denom <= 0.0 {
        0.5
    } else {
        belief.beta / denom
    }
}

fn coverage_factors(
    pool: &[char],
    session_sample_counts: &BTreeMap<char, u32>,
    coverage_strength: f64,
) -> BTreeMap<char, f64> {
    if coverage_strength <= 0.0 {
        return pool.iter().map(|c| (*c, 1.0)).collect();
    }
    let total_samples: u32 = pool
        .iter()
        .map(|c| session_sample_counts.get(c).copied().unwrap_or(0))
        .sum();
    let expected = if pool.is_empty() {
        0.0
    } else {
        f64::from(total_samples) / pool.len() as f64
    };
    let mut factors = BTreeMap::new();
    for character in pool {
        let seen = f64::from(session_sample_counts.get(character).copied().unwrap_or(0));
        let deficit = if expected > 0.0 {
            (expected - seen).max(0.0) / expected
        } else {
            1.0
        };
        factors.insert(*character, 1.0 + coverage_strength * deficit);
    }
    factors
}

fn difficulty_factor(p_error: f64, error_weight_strength: f64) -> f64 {
    if error_weight_strength <= 0.0 {
        1.0
    } else {
        1.0 + p_error * error_weight_strength
    }
}

fn random_normal(rng: &mut impl Rng) -> f64 {
    let mut u = 0.0;
    let mut v = 0.0;
    while u <= f64::EPSILON {
        u = rng.f64();
    }
    while v <= f64::EPSILON {
        v = rng.f64();
    }
    (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
}

fn sample_gamma(shape: f64, rng: &mut impl Rng) -> f64 {
    if shape <= 0.0 {
        return 0.0;
    }
    if shape < 1.0 {
        let boosted = sample_gamma(shape + 1.0, rng);
        return boosted * rng.f64().powf(1.0 / shape);
    }
    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    loop {
        let mut x;
        let mut v;
        loop {
            x = random_normal(rng);
            v = 1.0 + c * x;
            if v > 0.0 {
                break;
            }
        }
        v = v * v * v;
        let u = rng.f64();
        if u < 1.0 - 0.0331 * x * x * x * x {
            return d * v;
        }
        if u.ln() < 0.5 * x * x + d * (1.0 - v + v.ln()) {
            return d * v;
        }
    }
}

pub fn sample_beta(alpha: f64, beta: f64, rng: &mut impl Rng) -> f64 {
    let x = sample_gamma(alpha, rng);
    let y = sample_gamma(beta, rng);
    let denom = x + y;
    if denom <= 0.0 {
        0.5
    } else {
        x / denom
    }
}

pub fn compute_raw_sampling_weights(
    pool: &[char],
    state: &CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
) -> BTreeMap<char, f64> {
    let coverage = coverage_factors(pool, &state.session_sample_counts, config.coverage_strength);
    let mut weights = BTreeMap::new();
    for character in pool {
        let belief = belief_for(state, *character);
        let p_error = if config.thompson_sampling {
            // Beta(α, β) is P(correct); difficulty weighting needs P(error).
            (1.0 - sample_beta(belief.alpha, belief.beta, rng)).clamp(0.0, 1.0)
        } else {
            beta_posterior_mean_error(belief)
        };
        let coverage_factor = coverage.get(character).copied().unwrap_or(1.0);
        weights.insert(
            *character,
            difficulty_factor(p_error, config.error_weight_strength) * coverage_factor,
        );
    }
    weights
}

pub fn normalize_weights(pool: &[char], weights: &BTreeMap<char, f64>) -> BTreeMap<char, f64> {
    let total: f64 = pool
        .iter()
        .map(|c| weights.get(c).copied().unwrap_or(0.0))
        .sum();
    let mut out = BTreeMap::new();
    if total <= 0.0 {
        let uniform = if pool.is_empty() {
            0.0
        } else {
            1.0 / pool.len() as f64
        };
        for c in pool {
            out.insert(*c, uniform);
        }
        return out;
    }
    for c in pool {
        out.insert(*c, weights.get(c).copied().unwrap_or(0.0) / total);
    }
    out
}

fn pick_from_pool(pool: &[char], weights: &BTreeMap<char, f64>, rng: &mut impl Rng) -> char {
    if pool.is_empty() {
        return '\0';
    }
    let weight_list: Vec<f64> = pool
        .iter()
        .map(|c| weights.get(c).copied().unwrap_or(1.0))
        .collect();
    weighted_random_pick(pool, &weight_list, rng)
}

pub fn sample_training_group(
    pool: &[char],
    group_size: usize,
    state: &CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
) -> (String, CharSamplingState) {
    if pool.is_empty() || group_size == 0 {
        return (String::new(), state.clone());
    }
    let raw_weights = compute_raw_sampling_weights(pool, state, config, rng);
    let letters = letters_subset(pool);
    let digits = digits_subset(pool);
    let use_mixed_split =
        config.char_set_mode == CharSetMode::Mixed && !letters.is_empty() && !digits.is_empty();
    let mixed_letters_pct = (config.mixed_letters_percent.min(100) as f64) / 100.0;

    let mut group = String::new();
    for _ in 0..group_size {
        let ch = if use_mixed_split {
            let pick_letters = rng.f64() < mixed_letters_pct;
            let subset = if pick_letters { &letters } else { &digits };
            let fallback = if subset.is_empty() {
                pool
            } else {
                subset.as_slice()
            };
            pick_from_pool(fallback, &raw_weights, rng)
        } else {
            pick_from_pool(pool, &raw_weights, rng)
        };
        if ch != '\0' {
            group.push(ch);
        }
    }
    if group.is_empty() {
        for ch in pool.iter().copied().take(group_size.max(1)) {
            group.push(ch);
        }
    }

    let mut session_sample_counts = state.session_sample_counts.clone();
    for character in group.chars() {
        *session_sample_counts.entry(character).or_insert(0) += 1;
    }
    (
        group,
        CharSamplingState {
            beliefs: state.beliefs.clone(),
            session_sample_counts,
        },
    )
}

pub fn update_sampling_state_from_answer(
    state: &CharSamplingState,
    sent: &str,
    received: &str,
) -> CharSamplingState {
    let mut beliefs = state.beliefs.clone();
    for pair in align_group(sent, received) {
        let Some(character) = pair.sent_char else {
            continue;
        };
        let belief = beliefs
            .get(&character)
            .copied()
            .unwrap_or_else(|| belief_for(state, character));
        beliefs.insert(
            character,
            if pair.matched {
                CharBetaBelief {
                    alpha: belief.alpha + 1.0,
                    beta: belief.beta,
                }
            } else {
                CharBetaBelief {
                    alpha: belief.alpha,
                    beta: belief.beta + 1.0,
                }
            },
        );
    }
    CharSamplingState {
        beliefs,
        session_sample_counts: state.session_sample_counts.clone(),
    }
}

pub fn generate_training_group(
    settings: &TrainingSettings,
    state: &CharSamplingState,
    rng: &mut impl Rng,
) -> (String, CharSamplingState) {
    let pool = compute_char_pool(settings);
    let span = settings
        .curriculum
        .max_group_size
        .saturating_sub(settings.curriculum.min_group_size)
        + 1;
    let group_size = settings.curriculum.min_group_size as usize
        + rng.usize_in(0, span.saturating_sub(1) as usize);
    let config = config_from_settings(settings);
    sample_training_group(&pool, group_size, state, &config, rng)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::FastrandRng;

    #[test]
    fn group_length_respects_fixed_size() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        settings.curriculum.min_group_size = 3;
        settings.curriculum.max_group_size = 3;
        let state = CharSamplingState::default();
        let mut rng = FastrandRng::default();
        let (group, _) = generate_training_group(&settings, &state, &mut rng);
        assert_eq!(group.len(), 3);
        for ch in group.chars() {
            assert!(ch == 'K' || ch == 'M');
        }
    }

    #[test]
    fn thompson_weights_are_positive() {
        let pool = ['K', 'M'];
        let state = CharSamplingState::default();
        let config = CharSamplingConfig {
            error_weight_strength: 3.0,
            coverage_strength: 1.0,
            thompson_sampling: true,
            mixed_letters_percent: 70,
            char_set_mode: CharSetMode::Koch,
        };
        let mut rng = FastrandRng::default();
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        assert!(weights.values().all(|w| *w > 0.0));
    }

    #[test]
    fn generated_group_is_never_empty_for_koch() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        settings.curriculum.min_group_size = 1;
        settings.curriculum.max_group_size = 1;
        let state = CharSamplingState::default();
        let mut rng = FastrandRng::default();
        let (group, _) = generate_training_group(&settings, &state, &mut rng);
        assert!(!group.is_empty());
    }

    #[test]
    fn repeated_letters_update_belief_once_per_occurrence() {
        let state = CharSamplingState::default();
        let next = update_sampling_state_from_answer(&state, "KK", "KK");
        let k = next.beliefs.get(&'K').copied().unwrap_or_default();
        assert!((k.alpha - (CHAR_SAMPLING_PRIOR_ALPHA + 2.0)).abs() < 1e-9);
        assert!((k.beta - CHAR_SAMPLING_PRIOR_BETA).abs() < 1e-9);
    }

    #[test]
    fn mismatch_increments_beta() {
        let state = CharSamplingState::default();
        let next = update_sampling_state_from_answer(&state, "KM", "KX");
        let m = next.beliefs.get(&'M').copied().unwrap_or_default();
        assert!((m.beta - (CHAR_SAMPLING_PRIOR_BETA + 1.0)).abs() < 1e-9);
    }

    #[test]
    fn thompson_weights_hard_letters_above_easy() {
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        state.beliefs.insert(
            'K',
            CharBetaBelief {
                alpha: 40.0,
                beta: 1.0,
            },
        );
        state.beliefs.insert(
            'M',
            CharBetaBelief {
                alpha: 1.0,
                beta: 40.0,
            },
        );
        let config = CharSamplingConfig {
            error_weight_strength: 3.0,
            coverage_strength: 0.0,
            thompson_sampling: true,
            mixed_letters_percent: 70,
            char_set_mode: CharSetMode::Koch,
        };
        let mut rng = FastrandRng::default();
        let mut k_sum = 0.0;
        let mut m_sum = 0.0;
        for _ in 0..400 {
            let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
            k_sum += weights.get(&'K').copied().unwrap_or(0.0);
            m_sum += weights.get(&'M').copied().unwrap_or(0.0);
        }
        assert!(m_sum > k_sum);
    }

    #[test]
    fn mixed_100_percent_never_samples_digits() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.mixed_letters_percent = 100;
        settings.curriculum.level = 5;
        settings.curriculum.digits_level = 9;
        settings.curriculum.min_group_size = 4;
        settings.curriculum.max_group_size = 4;
        let mut state = CharSamplingState::default();
        let mut rng = FastrandRng::default();
        for _ in 0..40 {
            let (group, next) = generate_training_group(&settings, &state, &mut rng);
            assert!(!group.is_empty());
            assert!(group.chars().all(|c| !c.is_ascii_digit()), "{group}");
            state = next;
        }
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use crate::rng::FastrandRng;

    /// Returns a fixed sequence of "random" numbers, so every branch that
    /// depends on a draw can be aimed at deliberately.
    struct ScriptedRng {
        values: Vec<f64>,
        next: usize,
    }

    impl ScriptedRng {
        fn new(values: &[f64]) -> Self {
            Self {
                values: values.to_vec(),
                next: 0,
            }
        }
    }

    impl Rng for ScriptedRng {
        fn f64(&mut self) -> f64 {
            let value = self.values[self.next % self.values.len()];
            self.next += 1;
            value
        }
    }

    fn config(mode: CharSetMode, pct: u32) -> CharSamplingConfig {
        CharSamplingConfig {
            error_weight_strength: 3.0,
            coverage_strength: 1.0,
            thompson_sampling: false,
            mixed_letters_percent: pct,
            char_set_mode: mode,
        }
    }

    #[test]
    fn a_belief_with_no_mass_is_a_coin_flip() {
        assert_eq!(
            beta_posterior_mean_error(CharBetaBelief {
                alpha: 0.0,
                beta: 0.0
            }),
            0.5
        );
    }

    #[test]
    fn an_untouched_character_starts_on_the_flat_prior() {
        let state = CharSamplingState::default();
        let belief = belief_for(&state, 'K');
        assert_eq!(belief.alpha, CHAR_SAMPLING_PRIOR_ALPHA);
        assert_eq!(belief.beta, CHAR_SAMPLING_PRIOR_BETA);
        assert_eq!(beta_posterior_mean_error(belief), 0.5);
    }

    #[test]
    fn coverage_lifts_the_characters_that_have_been_seen_least() {
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        state.session_sample_counts.insert('K', 10);
        let config = config(CharSetMode::Koch, 70);
        let mut rng = FastrandRng::default();
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        assert!(weights[&'M'] > weights[&'K']);

        // With coverage off both characters weigh the same.
        let mut flat = config.clone();
        flat.coverage_strength = 0.0;
        let weights = compute_raw_sampling_weights(&pool, &state, &flat, &mut rng);
        assert_eq!(weights[&'M'], weights[&'K']);
    }

    #[test]
    fn a_fresh_session_gives_every_character_the_same_coverage_lift() {
        // Nothing sampled yet, so there is no deficit to compute a ratio from.
        let pool = ['K', 'M'];
        let state = CharSamplingState::default();
        let config = config(CharSetMode::Koch, 70);
        let mut rng = FastrandRng::default();
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        assert_eq!(weights[&'K'], weights[&'M']);
        assert!(weights[&'K'] > 0.0);
    }

    #[test]
    fn error_weighting_can_be_switched_off() {
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        state.beliefs.insert(
            'M',
            CharBetaBelief {
                alpha: 1.0,
                beta: 20.0,
            },
        );
        let mut config = config(CharSetMode::Koch, 70);
        config.error_weight_strength = 0.0;
        config.coverage_strength = 0.0;
        let mut rng = FastrandRng::default();
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        assert_eq!(weights[&'K'], weights[&'M']);
    }

    #[test]
    fn thompson_draws_stay_inside_the_unit_interval() {
        let mut rng = FastrandRng(12345);
        for _ in 0..200 {
            let x = sample_beta(0.4, 2.5, &mut rng);
            assert!((0.0..=1.0).contains(&x), "beta draw out of range: {x}");
        }
        // A degenerate pair cannot divide by zero.
        assert_eq!(sample_beta(0.0, 0.0, &mut rng), 0.5);
        assert_eq!(sample_beta(-1.0, -1.0, &mut rng), 0.5);
    }

    #[test]
    fn weights_normalise_to_one_or_fall_back_to_uniform() {
        let pool = ['K', 'M', 'U'];
        let mut weights = BTreeMap::new();
        weights.insert('K', 1.0);
        weights.insert('M', 3.0);
        let normalized = normalize_weights(&pool, &weights);
        assert!((normalized.values().sum::<f64>() - 1.0).abs() < 1e-9);
        assert_eq!(normalized[&'U'], 0.0);

        // All-zero weights spread evenly rather than dividing by zero.
        let zeroed = normalize_weights(&pool, &BTreeMap::new());
        assert!(zeroed.values().all(|v| (*v - 1.0 / 3.0).abs() < 1e-9));
        assert!(normalize_weights(&[], &BTreeMap::new()).is_empty());
    }

    #[test]
    fn sampling_an_empty_pool_returns_an_empty_group() {
        let state = CharSamplingState::default();
        let config = config(CharSetMode::Koch, 70);
        let mut rng = FastrandRng::default();
        let (group, next) = sample_training_group(&[], 3, &state, &config, &mut rng);
        assert!(group.is_empty());
        assert_eq!(next, state);
        let (group, _) = sample_training_group(&['K'], 0, &state, &config, &mut rng);
        assert!(group.is_empty());
    }

    #[test]
    fn a_mixed_group_follows_the_letter_share() {
        let pool = ['K', 'M', '0', '1'];
        let state = CharSamplingState::default();
        let config = config(CharSetMode::Mixed, 50);
        // Draws below 0.5 pick letters, above pick digits. Each character costs
        // two draws: one for the subset, one for the weighted pick.
        let mut rng = ScriptedRng::new(&[0.1, 0.0, 0.9, 0.0]);
        let (group, next) = sample_training_group(&pool, 2, &state, &config, &mut rng);
        assert_eq!(group.chars().count(), 2);
        let mut chars = group.chars();
        assert!(!chars.next().unwrap().is_ascii_digit());
        assert!(chars.next().unwrap().is_ascii_digit());
        assert_eq!(next.session_sample_counts.values().sum::<u32>(), 2);
    }

    #[test]
    fn a_mixed_group_with_only_letters_unlocked_still_fills() {
        // No digits in the pool, so the digit half falls back to the whole pool.
        let pool = ['K', 'M'];
        let state = CharSamplingState::default();
        let config = config(CharSetMode::Mixed, 50);
        let mut rng = ScriptedRng::new(&[0.9, 0.0]);
        let (group, _) = sample_training_group(&pool, 2, &state, &config, &mut rng);
        assert_eq!(group.chars().count(), 2);
        assert!(group.chars().all(|c| c == 'K' || c == 'M'));
    }

    #[test]
    fn sample_counts_accumulate_across_groups() {
        let pool = ['K'];
        let state = CharSamplingState::default();
        let config = config(CharSetMode::Koch, 70);
        let mut rng = FastrandRng::default();
        let (_, first) = sample_training_group(&pool, 2, &state, &config, &mut rng);
        let (_, second) = sample_training_group(&pool, 2, &first, &config, &mut rng);
        assert_eq!(second.session_sample_counts.get(&'K'), Some(&4));
    }

    #[test]
    fn an_answer_moves_only_the_characters_that_were_sent() {
        let state = CharSamplingState::default();
        // "KM" copied as "KX": K right, M wrong, X never sent.
        let next = update_sampling_state_from_answer(&state, "KM", "KX");
        assert_eq!(next.beliefs[&'K'].alpha, CHAR_SAMPLING_PRIOR_ALPHA + 1.0);
        assert_eq!(next.beliefs[&'K'].beta, CHAR_SAMPLING_PRIOR_BETA);
        assert_eq!(next.beliefs[&'M'].beta, CHAR_SAMPLING_PRIOR_BETA + 1.0);
        assert!(!next.beliefs.contains_key(&'X'));
    }

    #[test]
    fn history_from_earlier_sessions_seeds_the_beliefs() {
        let mut first = BTreeMap::new();
        first.insert(
            'K',
            crate::alignment::LetterAccuracy {
                correct: 3,
                total: 5,
            },
        );
        let mut second = BTreeMap::new();
        second.insert(
            'K',
            crate::alignment::LetterAccuracy {
                correct: 1,
                total: 1,
            },
        );
        let state = create_initial_sampling_state(&[&first, &second]);
        let belief = state.beliefs[&'K'];
        assert_eq!(belief.alpha, CHAR_SAMPLING_PRIOR_ALPHA + 4.0);
        assert_eq!(belief.beta, CHAR_SAMPLING_PRIOR_BETA + 2.0);
        assert!(state.session_sample_counts.is_empty());
    }

    #[test]
    fn group_size_stays_inside_the_configured_range() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 4;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 5;
        let state = CharSamplingState::default();
        let mut rng = FastrandRng(99);
        for _ in 0..50 {
            let (group, _) = generate_training_group(&settings, &state, &mut rng);
            let len = group.chars().count();
            assert!(
                (2..=5).contains(&len),
                "group {group:?} has {len} characters"
            );
        }
    }
}
