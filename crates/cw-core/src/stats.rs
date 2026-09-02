//! Letter diagnostics, bigrams, confusion, and sampling snapshots for the stats UI.

use std::collections::BTreeMap;

use crate::alignment::{align_group, calculate_group_letter_accuracy, LetterAccuracy};
use crate::morse::LCWO_SEQUENCE;
use crate::pool::compute_char_pool;
use crate::sampling::{
    belief_for, beta_posterior_mean_error, compute_raw_sampling_weights, config_from_settings,
    normalize_weights,
};
use crate::session::SessionResult;
use crate::settings::TrainingSettings;

pub const MASTERED_MIN_ATTEMPTS: u32 = 5;
pub const MASTERED_MIN_ACCURACY: f64 = 0.9;
pub const BUILDING_MIN_ACCURACY: f64 = 0.7;
pub const SLOW_AVG_MS: f64 = 800.0;
pub const GROUP_START_BIGRAM_TOKEN: char = '▸';

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasteryStatus {
    Mastered,
    Building,
    Weak,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CharacterDiagnostic {
    pub letter: char,
    pub accuracy_pct: f64,
    pub avg_ms: f64,
    pub total: u32,
    pub correct: u32,
    pub status: MasteryStatus,
    pub is_slow: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccuracyPoint {
    pub date: String,
    pub accuracy_pct: f64,
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnigramStat {
    pub letter: char,
    pub total: u32,
    pub wrong: u32,
    pub rate: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BigramCell {
    pub row: char,
    pub col: char,
    pub rate: f64,
    pub total: u32,
    pub wrong: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BigramHeatmap {
    pub letters: Vec<char>,
    pub cells: Vec<BigramCell>,
    pub max_rate: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfusionEntry {
    pub sent: char,
    pub typed: Option<char>,
    pub count: u32,
    pub percentage: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SamplingRow {
    pub character: char,
    pub p_error: f64,
    pub sampling_prob: f64,
    pub alpha: f64,
    pub beta: f64,
    pub is_letter: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionHistoryRow {
    pub date: String,
    pub accuracy_pct: f64,
    pub score: f64,
    pub groups: u32,
    pub correct_groups: u32,
    pub total_chars: u32,
    pub timestamp: u64,
}

fn lcwo_index(ch: char) -> usize {
    LCWO_SEQUENCE
        .iter()
        .position(|c| *c == ch)
        .unwrap_or(usize::MAX)
}

fn session_letter_accuracy(session: &SessionResult) -> BTreeMap<char, LetterAccuracy> {
    if !session.letter_accuracy.is_empty() {
        return session.letter_accuracy.clone();
    }
    let pairs: Vec<(String, String)> = session
        .groups
        .iter()
        .map(|g| (g.sent.clone(), g.received.clone()))
        .collect();
    calculate_group_letter_accuracy(&pairs)
}

pub fn classify_mastery(accuracy01: f64, attempts: u32) -> MasteryStatus {
    if attempts < MASTERED_MIN_ATTEMPTS {
        MasteryStatus::Weak
    } else if accuracy01 >= MASTERED_MIN_ACCURACY {
        MasteryStatus::Mastered
    } else if accuracy01 >= BUILDING_MIN_ACCURACY {
        MasteryStatus::Building
    } else {
        MasteryStatus::Weak
    }
}

pub fn accuracy_chart(sessions: &[SessionResult]) -> Vec<AccuracyPoint> {
    let mut rows: Vec<&SessionResult> = sessions.iter().collect();
    rows.sort_by_key(|s| s.timestamp);
    rows.into_iter()
        .map(|s| AccuracyPoint {
            date: s.date.clone(),
            accuracy_pct: s.accuracy * 100.0,
            timestamp: s.timestamp,
        })
        .collect()
}

pub fn character_diagnostics(sessions: &[SessionResult]) -> Vec<CharacterDiagnostic> {
    let mut totals: BTreeMap<char, (u32, u32)> = BTreeMap::new();
    let mut timings: BTreeMap<char, Vec<f64>> = BTreeMap::new();
    for session in sessions {
        for (letter, stats) in session_letter_accuracy(session) {
            let entry = totals.entry(letter).or_insert((0, 0));
            entry.0 += stats.correct;
            entry.1 += stats.total;
        }
        for (group, timing) in session.groups.iter().zip(session.group_timings.iter()) {
            if timing.time_to_complete_ms <= 0.0 {
                continue;
            }
            let sent: Vec<char> = group.sent.to_ascii_uppercase().chars().collect();
            if sent.is_empty() {
                continue;
            }
            let per = timing.time_to_complete_ms / sent.len() as f64;
            for ch in sent {
                timings.entry(ch).or_default().push(per);
            }
        }
    }
    let mut out: Vec<CharacterDiagnostic> = totals
        .into_iter()
        .map(|(letter, (correct, total))| {
            let accuracy01 = if total == 0 {
                0.0
            } else {
                f64::from(correct) / f64::from(total)
            };
            let samples = timings.get(&letter);
            let avg_ms = samples
                .map(|s| s.iter().sum::<f64>() / s.len() as f64)
                .unwrap_or(0.0);
            let status = classify_mastery(accuracy01, total);
            let is_slow = matches!(status, MasteryStatus::Mastered | MasteryStatus::Building)
                && avg_ms >= SLOW_AVG_MS;
            CharacterDiagnostic {
                letter,
                accuracy_pct: accuracy01 * 100.0,
                avg_ms,
                total,
                correct,
                status,
                is_slow,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        let rank = |s: MasteryStatus| match s {
            MasteryStatus::Weak => 0,
            MasteryStatus::Building => 1,
            MasteryStatus::Mastered => 2,
        };
        rank(a.status)
            .cmp(&rank(b.status))
            .then(a.accuracy_pct.partial_cmp(&b.accuracy_pct).unwrap_or(std::cmp::Ordering::Equal))
    });
    out
}

pub fn unigram_stats(sessions: &[SessionResult]) -> Vec<UnigramStat> {
    let mut counts: BTreeMap<char, (u32, u32)> = BTreeMap::new();
    for session in sessions {
        for group in &session.groups {
            let sent: Vec<char> = group.sent.to_ascii_uppercase().chars().collect();
            let rec: Vec<char> = group.received.to_ascii_uppercase().chars().collect();
            for (i, ch) in sent.iter().enumerate() {
                let typed = rec.get(i).copied();
                let entry = counts.entry(*ch).or_insert((0, 0));
                entry.1 += 1;
                if typed != Some(*ch) {
                    entry.0 += 1;
                }
            }
        }
    }
    let mut rows: Vec<UnigramStat> = counts
        .into_iter()
        .map(|(letter, (wrong, total))| UnigramStat {
            letter,
            total,
            wrong,
            rate: if total == 0 {
                0.0
            } else {
                f64::from(wrong) / f64::from(total)
            },
        })
        .collect();
    rows.sort_by_key(|r| lcwo_index(r.letter));
    rows
}

pub fn bigram_heatmap(sessions: &[SessionResult]) -> BigramHeatmap {
    let mut letters = Vec::new();
    let mut counts: BTreeMap<(char, char), (u32, u32)> = BTreeMap::new();
    for session in sessions {
        for group in &session.groups {
            let sent: Vec<char> = group.sent.to_ascii_uppercase().chars().collect();
            let rec: Vec<char> = group.received.to_ascii_uppercase().chars().collect();
            for (i, curr) in sent.iter().enumerate() {
                let prev = if i == 0 {
                    GROUP_START_BIGRAM_TOKEN
                } else {
                    sent[i - 1]
                };
                if !letters.contains(&prev) {
                    letters.push(prev);
                }
                if !letters.contains(curr) {
                    letters.push(*curr);
                }
                let entry = counts.entry((prev, *curr)).or_insert((0, 0));
                entry.1 += 1;
                if rec.get(i).copied() != Some(*curr) {
                    entry.0 += 1;
                }
            }
        }
    }
    letters.sort_by_key(|c| {
        if *c == GROUP_START_BIGRAM_TOKEN {
            0
        } else {
            lcwo_index(*c) + 1
        }
    });
    let mut max_rate: f64 = 0.0;
    let mut cells = Vec::new();
    for &row in &letters {
        for &col in &letters {
            let (wrong, total) = counts.get(&(row, col)).copied().unwrap_or((0, 0));
            let rate = if total == 0 {
                0.0
            } else {
                f64::from(wrong) / f64::from(total)
            };
            max_rate = max_rate.max(rate);
            cells.push(BigramCell {
                row,
                col,
                rate,
                total,
                wrong,
            });
        }
    }
    BigramHeatmap {
        letters,
        cells,
        max_rate,
    }
}

pub fn confusion_entries(sessions: &[SessionResult], limit: usize) -> Vec<ConfusionEntry> {
    let mut counts: BTreeMap<(char, Option<char>), u32> = BTreeMap::new();
    let mut totals: BTreeMap<char, u32> = BTreeMap::new();
    for session in sessions {
        for group in &session.groups {
            for pair in align_group(&group.sent, &group.received) {
                let Some(sent) = pair.sent_char else {
                    continue;
                };
                *totals.entry(sent).or_insert(0) += 1;
                if !pair.matched {
                    *counts.entry((sent, pair.received_char)).or_insert(0) += 1;
                }
            }
        }
    }
    let mut rows: Vec<ConfusionEntry> = counts
        .into_iter()
        .map(|((sent, typed), count)| {
            let total = totals.get(&sent).copied().unwrap_or(1);
            ConfusionEntry {
                sent,
                typed,
                count,
                percentage: 100.0 * f64::from(count) / f64::from(total),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then(b.percentage.partial_cmp(&a.percentage).unwrap_or(std::cmp::Ordering::Equal))
    });
    rows.truncate(limit);
    rows
}

pub fn sampling_rows(settings: &TrainingSettings, sessions: &[SessionResult]) -> Vec<SamplingRow> {
    let owned: Vec<BTreeMap<char, LetterAccuracy>> =
        sessions.iter().map(session_letter_accuracy).collect();
    let history: Vec<&BTreeMap<char, LetterAccuracy>> = owned.iter().collect();
    let state = crate::sampling::create_initial_sampling_state(&history);
    let pool = compute_char_pool(settings);
    let mut config = config_from_settings(settings);
    config.thompson_sampling = false;
    let mut rng = crate::rng::FastrandRng::default();
    let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
    let probs = normalize_weights(&pool, &weights);
    pool.into_iter()
        .map(|character| {
            let belief = belief_for(&state, character);
            SamplingRow {
                character,
                p_error: beta_posterior_mean_error(belief),
                sampling_prob: probs.get(&character).copied().unwrap_or(0.0),
                alpha: belief.alpha,
                beta: belief.beta,
                is_letter: !character.is_ascii_digit(),
            }
        })
        .collect()
}

pub fn session_history(sessions: &[SessionResult]) -> Vec<SessionHistoryRow> {
    let mut rows: Vec<SessionHistoryRow> = sessions
        .iter()
        .map(|s| SessionHistoryRow {
            date: s.date.clone(),
            accuracy_pct: s.accuracy * 100.0,
            score: s.score,
            groups: s.groups.len() as u32,
            correct_groups: s.groups.iter().filter(|g| g.correct).count() as u32,
            total_chars: s.total_chars,
            timestamp: s.timestamp,
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{GroupResult, SessionTiming};
    use crate::settings::CharSetMode;

    fn session() -> SessionResult {
        SessionResult {
            date: "2026-09-01".into(),
            timestamp: 1,
            started_at: 0,
            finished_at: 1,
            groups: vec![GroupResult {
                sent: "KM".into(),
                received: "K?".into(),
                correct: false,
            }],
            group_timings: vec![SessionTiming {
                time_to_complete_ms: 400.0,
                per_char_ms: 200.0,
                char_wpm: None,
            }],
            accuracy: 0.5,
            letter_accuracy: {
                let mut m = BTreeMap::new();
                m.insert(
                    'K',
                    crate::alignment::LetterAccuracy {
                        correct: 1,
                        total: 1,
                    },
                );
                m.insert(
                    'M',
                    crate::alignment::LetterAccuracy {
                        correct: 0,
                        total: 1,
                    },
                );
                m
            },
            alphabet_size: 2,
            avg_response_ms: 400.0,
            total_chars: 2,
            effective_alphabet_size: 2.0,
            score: 1.0,
            koch_level: 1,
            digits_level: 1,
            char_set_mode: CharSetMode::Koch,
            char_wpm: 18.0,
            effective_wpm: 18.0,
        }
    }

    #[test]
    fn diagnostics_and_confusion() {
        let sessions = [session()];
        let diag = character_diagnostics(&sessions);
        assert_eq!(diag.len(), 2);
        let conf = confusion_entries(&sessions, 8);
        assert!(!conf.is_empty());
        assert_eq!(unigram_stats(&sessions).len(), 2);
    }
}
