//! Session scoring: alphabet breadth, accuracy, speed, and volume.

use crate::morse::is_scored_char;

pub const MAX_SCORED_CHARS: u32 = 200;
const MAX_EFFECTIVE_ALPHABET: f64 = 48.0;

#[derive(Clone, Copy, Debug)]
pub struct ScoreConstants {
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
    pub k: f64,
    pub delta: f64,
}

impl Default for ScoreConstants {
    fn default() -> Self {
        Self {
            alpha: 1.5,
            beta: 2.0,
            gamma: 0.5,
            k: 1000.0,
            delta: 0.5,
        }
    }
}

fn count_valid_letters(text: &str) -> u32 {
    text.chars()
        .filter(|ch| is_scored_char(ch.to_ascii_uppercase()))
        .count() as u32
}

pub fn calculate_alphabet_size(sent_groups: &[impl AsRef<str>]) -> u32 {
    let mut unique = std::collections::BTreeSet::new();
    for g in sent_groups {
        for ch in g.as_ref().chars() {
            let up = ch.to_ascii_uppercase();
            if is_scored_char(up) {
                unique.insert(up);
            }
        }
    }
    unique.len().max(1) as u32
}

pub fn calculate_total_chars(sent_groups: &[impl AsRef<str>]) -> u32 {
    let total: u32 = sent_groups
        .iter()
        .map(|g| count_valid_letters(g.as_ref()))
        .sum();
    total.max(1)
}

/// Shannon entropy (natural log) converted to an effective alphabet size, with Miller–Madow bias correction.
pub fn calculate_effective_alphabet_size(sent_groups: &[impl AsRef<str>]) -> f64 {
    let mut counts = std::collections::BTreeMap::new();
    let mut total = 0u32;
    for g in sent_groups {
        for ch in g.as_ref().chars() {
            let up = ch.to_ascii_uppercase();
            if is_scored_char(up) {
                *counts.entry(up).or_insert(0u32) += 1;
                total += 1;
            }
        }
    }
    let k = counts.len();
    if total == 0 || k == 0 {
        return 1.0;
    }
    let mut h = 0.0;
    for count in counts.values() {
        let p = f64::from(*count) / f64::from(total);
        if p > 0.0 {
            h += -p * p.ln();
        }
    }
    h += f64::from((k.saturating_sub(1)) as u32) / (2.0 * f64::from(total));
    h.exp().clamp(1.0, MAX_EFFECTIVE_ALPHABET)
}

pub fn compute_average_response_ms(samples: &[f64]) -> f64 {
    let valid: Vec<f64> = samples
        .iter()
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect();
    if valid.is_empty() {
        ScoreConstants::default().k
    } else {
        valid.iter().sum::<f64>() / valid.len() as f64
    }
}

pub fn compute_session_score(
    effective_alphabet_size: f64,
    accuracy: f64,
    avg_response_ms: f64,
    total_chars: u32,
    constants: ScoreConstants,
) -> f64 {
    let n = effective_alphabet_size.max(1.0);
    let a = accuracy.clamp(0.0, 1.0);
    let t_avg = avg_response_ms.max(1.0).round();
    let c = total_chars.max(1).min(MAX_SCORED_CHARS) as f64;

    let term_alphabet = n.powf(constants.alpha);
    let term_accuracy = a.powf(constants.beta);
    let term_speed = (constants.k / t_avg).powf(constants.gamma);
    let term_volume = c.powf(constants.delta);
    let score = term_volume * term_alphabet * term_accuracy * term_speed;
    (score * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_cap_stops_grind() {
        let base_n = 10.0;
        let acc = 0.95;
        let t = 1000.0;
        let at_cap =
            compute_session_score(base_n, acc, t, MAX_SCORED_CHARS, ScoreConstants::default());
        let way_past = compute_session_score(
            base_n,
            acc,
            t,
            MAX_SCORED_CHARS * 50,
            ScoreConstants::default(),
        );
        assert_eq!(at_cap, way_past);
        let shorter = compute_session_score(base_n, acc, t, 50, ScoreConstants::default());
        assert!(at_cap > shorter);
    }

    #[test]
    fn accuracy_beats_capped_volume() {
        let grinder = compute_session_score(10.0, 0.8, 1000.0, 100_000, ScoreConstants::default());
        let sharp = compute_session_score(
            10.0,
            1.0,
            1000.0,
            MAX_SCORED_CHARS,
            ScoreConstants::default(),
        );
        assert!(sharp > grinder);
    }
}
