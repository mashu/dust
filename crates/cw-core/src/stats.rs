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

/// Sort rank for a character: its place in the LCWO sequence, or last for a
/// character that sequence never teaches (`+` is a Morse character but is not
/// part of LCWO, and any custom alphabet can hold others).
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
        rank(a.status).cmp(&rank(b.status)).then(
            a.accuracy_pct
                .partial_cmp(&b.accuracy_pct)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    out
}

pub fn unigram_stats(sessions: &[SessionResult]) -> Vec<UnigramStat> {
    let mut counts: BTreeMap<char, (u32, u32)> = BTreeMap::new();
    for session in sessions {
        for group in &session.groups {
            for pair in align_group(&group.sent, &group.received) {
                let Some(ch) = pair.sent_char else {
                    continue;
                };
                let entry = counts.entry(ch).or_insert((0, 0));
                entry.1 += 1;
                if !pair.matched {
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
            let mut sent_prev = None::<char>;
            for pair in align_group(&group.sent, &group.received) {
                let Some(curr) = pair.sent_char else {
                    continue;
                };
                let prev = sent_prev.unwrap_or(GROUP_START_BIGRAM_TOKEN);
                if !letters.contains(&prev) {
                    letters.push(prev);
                }
                if !letters.contains(&curr) {
                    letters.push(curr);
                }
                let entry = counts.entry((prev, curr)).or_insert((0, 0));
                entry.1 += 1;
                if !pair.matched {
                    entry.0 += 1;
                }
                sent_prev = Some(curr);
            }
        }
    }
    // The start-of-group token sorts first, so every real character is shifted
    // one rank up. `lcwo_index` saturates for characters outside the sequence,
    // which would otherwise overflow here and panic in a debug build.
    letters.sort_by_key(|c| {
        if *c == GROUP_START_BIGRAM_TOKEN {
            0
        } else {
            lcwo_index(*c).saturating_add(1)
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
        b.count.cmp(&a.count).then(
            b.percentage
                .partial_cmp(&a.percentage)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    rows.truncate(limit);
    rows
}

pub fn sampling_rows(settings: &TrainingSettings, sessions: &[SessionResult]) -> Vec<SamplingRow> {
    let owned: Vec<BTreeMap<char, LetterAccuracy>> = sessions
        .iter()
        .filter(|session| session.usable_for_sampling(settings))
        .map(session_letter_accuracy)
        .collect();
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
            level: 1,
            digits_level: 1,
            char_set_mode: CharSetMode::Koch,
            char_wpm: 18.0,
            effective_wpm: 18.0,
            alphabet_fingerprint: String::new(),
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

    fn plain(date: &str, timestamp: u64, groups: &[(&str, &str)]) -> SessionResult {
        let mut out = session();
        out.date = date.to_string();
        out.timestamp = timestamp;
        out.letter_accuracy.clear();
        out.groups = groups
            .iter()
            .map(|(sent, received)| GroupResult {
                sent: (*sent).to_string(),
                received: (*received).to_string(),
                correct: sent == received,
            })
            .collect();
        out.group_timings = groups
            .iter()
            .map(|(sent, _)| SessionTiming {
                time_to_complete_ms: 1000.0 * sent.chars().count() as f64,
                per_char_ms: 1000.0,
                char_wpm: Some(20.0),
            })
            .collect();
        out
    }

    #[test]
    fn letter_accuracy_falls_back_to_the_stored_groups() {
        // Sessions written before letter_accuracy existed still show per-letter rows.
        let legacy = plain("2026-09-01", 1, &[("KM", "KX")]);
        let diag = character_diagnostics(&[legacy]);
        let m = diag.iter().find(|d| d.letter == 'M').expect("M row");
        assert_eq!((m.correct, m.total), (0, 1));
        let k = diag.iter().find(|d| d.letter == 'K').expect("K row");
        assert_eq!((k.correct, k.total), (1, 1));
    }

    #[test]
    fn mastery_needs_both_attempts_and_accuracy() {
        assert_eq!(classify_mastery(1.0, 4), MasteryStatus::Weak);
        assert_eq!(
            classify_mastery(MASTERED_MIN_ACCURACY, MASTERED_MIN_ATTEMPTS),
            MasteryStatus::Mastered
        );
        assert_eq!(
            classify_mastery(BUILDING_MIN_ACCURACY, MASTERED_MIN_ATTEMPTS),
            MasteryStatus::Building
        );
        assert_eq!(classify_mastery(0.1, 50), MasteryStatus::Weak);
    }

    #[test]
    fn diagnostics_rank_weak_letters_first_and_flag_slow_ones() {
        // Ten sends of each letter: K always right and slow, M always wrong.
        let groups: Vec<(&str, &str)> = vec![("KM", "KX"); 10];
        let session = plain("2026-09-01", 1, &groups);
        let diag = character_diagnostics(&[session]);
        assert_eq!(diag.first().map(|d| d.letter), Some('M'));
        let k = diag.iter().find(|d| d.letter == 'K').expect("K row");
        assert_eq!(k.status, MasteryStatus::Mastered);
        // 2000 ms over two characters is 1000 ms each, past the slow mark.
        assert!(k.is_slow);
        assert!(k.avg_ms >= SLOW_AVG_MS);
        let m = diag.iter().find(|d| d.letter == 'M').expect("M row");
        assert_eq!(m.status, MasteryStatus::Weak);
        // A weak letter is never called slow on top of it.
        assert!(!m.is_slow);
    }

    #[test]
    fn zero_length_timings_are_not_averaged_in() {
        let mut session = plain("2026-09-01", 1, &[("KM", "KM")]);
        session.group_timings[0].time_to_complete_ms = 0.0;
        let diag = character_diagnostics(&[session]);
        assert!(diag.iter().all(|d| d.avg_ms == 0.0));
    }

    #[test]
    fn the_accuracy_chart_is_ordered_by_time() {
        let newest = plain("2026-09-03", 30, &[("KM", "KM")]);
        let oldest = plain("2026-09-01", 10, &[("KM", "KX")]);
        let chart = accuracy_chart(&[newest, oldest]);
        assert_eq!(
            chart.iter().map(|p| p.timestamp).collect::<Vec<_>>(),
            vec![10, 30]
        );
        assert_eq!(chart[0].date, "2026-09-01");
        assert_eq!(chart[0].accuracy_pct, 50.0);
        assert!(accuracy_chart(&[]).is_empty());
    }

    #[test]
    fn unigrams_count_every_sent_character_in_sequence_order() {
        let session = plain("2026-09-01", 1, &[("MK", "MX"), ("KM", "KM")]);
        let rows = unigram_stats(&[session]);
        // K comes before M in the LCWO sequence, whatever order they were sent in.
        assert_eq!(
            rows.iter().map(|r| r.letter).collect::<Vec<_>>(),
            ['K', 'M']
        );
        let k = rows.iter().find(|r| r.letter == 'K').expect("K row");
        assert_eq!((k.total, k.wrong), (2, 1));
        assert_eq!(k.rate, 0.5);
        assert!(unigram_stats(&[]).is_empty());
    }

    #[test]
    fn bigrams_rank_the_group_start_first_and_unknown_characters_last() {
        // `+` is a Morse character the LCWO sequence never teaches; it must not
        // overflow the sort key.
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Custom;
        settings.curriculum.custom_set = vec!['K', '+'];
        let session = plain("2026-09-01", 1, &[("K+", "KM")]);
        let heat = bigram_heatmap(&[session]);
        assert_eq!(heat.letters.first(), Some(&GROUP_START_BIGRAM_TOKEN));
        assert_eq!(heat.letters.last(), Some(&'+'));
        assert_eq!(heat.cells.len(), heat.letters.len() * heat.letters.len());
        // K→+ was copied as K→M: one attempt, one error.
        let cell = heat
            .cells
            .iter()
            .find(|c| c.row == 'K' && c.col == '+')
            .expect("K+ cell");
        assert_eq!((cell.total, cell.wrong), (1, 1));
        assert_eq!(cell.rate, 1.0);
        assert_eq!(heat.max_rate, 1.0);
        // An empty history draws no grid at all.
        assert!(bigram_heatmap(&[]).letters.is_empty());
    }

    #[test]
    fn confusion_is_ordered_and_capped() {
        let session = plain(
            "2026-09-01",
            1,
            &[("KMU", "XMU"), ("KMU", "XMU"), ("KMU", "KMR")],
        );
        let rows = confusion_entries(&[session], 8);
        assert_eq!(rows[0].sent, 'K');
        assert_eq!(rows[0].typed, Some('X'));
        assert_eq!(rows[0].count, 2);
        // Two of the three K sends were copied as X.
        assert!((rows[0].percentage - 200.0 / 3.0).abs() < 1e-9);
        assert_eq!(confusion_entries(&[], 8).len(), 0);
        // The limit is honoured.
        let long = plain("2026-09-02", 2, &[("KMURE", "XXXXX")]);
        assert_eq!(confusion_entries(&[long], 2).len(), 2);
    }

    #[test]
    fn a_dropped_character_is_reported_as_no_answer() {
        let session = plain("2026-09-01", 1, &[("KM", "K")]);
        let rows = confusion_entries(&[session], 8);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sent, 'M');
        assert_eq!(rows[0].typed, None);
    }

    #[test]
    fn characters_that_were_never_sent_are_left_out_of_every_table() {
        // An extra character was typed in the middle of the group.
        let session = plain("2026-09-01", 1, &[("KM", "KXM")]);
        let rows = unigram_stats(std::slice::from_ref(&session));
        assert_eq!(
            rows.iter().map(|r| r.letter).collect::<Vec<_>>(),
            ['K', 'M']
        );
        let heat = bigram_heatmap(std::slice::from_ref(&session));
        assert!(!heat.letters.contains(&'X'));
        let confusion = confusion_entries(&[session], 8);
        assert!(confusion.iter().all(|c| c.sent != 'X'));
    }

    #[test]
    fn history_is_newest_first() {
        let a = plain("2026-09-01", 10, &[("KM", "KM"), ("KM", "KX")]);
        let b = plain("2026-09-02", 20, &[("KM", "KM")]);
        let rows = session_history(&[a, b]);
        assert_eq!(rows[0].date, "2026-09-02");
        assert_eq!(rows[1].groups, 2);
        assert_eq!(rows[1].correct_groups, 1);
        assert_eq!(rows[1].total_chars, 2);
        assert_eq!(rows[1].accuracy_pct, 50.0);
        assert!(session_history(&[]).is_empty());
    }

    #[test]
    fn sampling_rows_are_empty_when_the_pool_is() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.mixed_letters_percent = 100;
        settings.curriculum.custom_sequence = vec!['0'];
        // A letters-only mixed pool over a digits-only sequence has nothing in it.
        assert!(sampling_rows(&settings, &[]).is_empty());
    }

    #[test]
    fn sampling_rows_mark_digits_and_sum_to_one() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.level = 1;
        settings.curriculum.digits_level = 1;
        let rows = sampling_rows(&settings, &[]);
        assert!(rows.iter().any(|r| !r.is_letter));
        assert!(rows.iter().any(|r| r.is_letter));
        let total: f64 = rows.iter().map(|r| r.sampling_prob).sum();
        assert!((total - 1.0).abs() < 1e-9);
        // With no history every character starts on the same flat prior.
        assert!(rows.iter().all(|r| (r.p_error - 0.5).abs() < 1e-9));
    }

    #[test]
    fn sampling_rows_ignore_other_char_set_modes() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        let mut koch = session();
        koch.char_set_mode = CharSetMode::Koch;
        koch.letter_accuracy.clear();
        koch.letter_accuracy.insert(
            'K',
            crate::alignment::LetterAccuracy {
                correct: 10,
                total: 10,
            },
        );
        let mut digits = koch.clone();
        digits.char_set_mode = CharSetMode::Digits;
        digits.letter_accuracy.insert(
            'K',
            crate::alignment::LetterAccuracy {
                correct: 0,
                total: 10,
            },
        );
        let k_belief = |rows: &[SamplingRow]| {
            rows.iter()
                .find(|row| row.character == 'K')
                .map(|row| (row.alpha, row.beta))
                .expect("K in pool")
        };
        let only_koch = k_belief(&sampling_rows(&settings, &[koch.clone()]));
        let mixed_history = k_belief(&sampling_rows(&settings, &[koch, digits]));
        assert_eq!(only_koch, mixed_history);
    }
}
