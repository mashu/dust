//! Letter diagnostics, bigrams, confusion, and sampling snapshots for the stats UI.

use std::collections::BTreeMap;

use crate::alignment::align_group;
use crate::belief::EstimatePoint;
use crate::contest::{ContestKind, StoredLog};
use crate::morse::LCWO_SEQUENCE;
use crate::pool::compute_char_pool;
use crate::sampling::{
    CharSamplingConfig, CharSamplingState, adaptive_bounds, adapts_speed, belief_for,
    beta_posterior_mean_error, compute_raw_sampling_weights, config_from_settings,
    generate_callsign_group, generate_contest_call, generate_word_group, group_size_bounds,
    normalize_weights,
};
use crate::session::SessionResult;
use crate::settings::{CharSetMode, TrainingSettings};
use crate::span::{LengthForecast, SpanBelief};
use crate::speed::{SpeedBelief, SpeedForecast, SpeedPath, forecast_speeds, station_shares};

pub const MASTERED_MIN_ATTEMPTS: u32 = 5;
pub const MASTERED_MIN_ACCURACY: f64 = 0.9;
pub const BUILDING_MIN_ACCURACY: f64 = 0.7;
pub const SLOW_AVG_MS: f64 = 800.0;

/// What the Letters card says once the history holds a contest run: its
/// letters count, but not its times.
pub const CONTEST_TIMES_NOTE: &str = "Contest runs count towards accuracy here, but their times \
     are left out of the average time that marks a letter slow: they include waiting and typing, \
     not only copying.";
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
    /// The chance it is drawn next; nothing for a character the current
    /// set does not draw.
    pub sampling_prob: f64,
    pub alpha: f64,
    pub beta: f64,
    pub is_letter: bool,
    /// Whether the current set draws it at all.
    pub drawn_now: bool,
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
    /// What the session practised: every row says, since the history holds
    /// every mode whatever is chosen now.
    pub mode: CharSetMode,
    /// The contest a run was of, when this build knows it.
    pub contest: Option<ContestKind>,
    /// What became of a run's log; none for a session of groups.
    pub log: Option<StoredLog>,
    /// A contest run had Show copy on.
    pub assisted: bool,
    /// Where the session sits in the history the rows were made from, so a
    /// row names one session even when two finished in the same millisecond.
    pub index: usize,
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
        for (letter, stats) in session.letter_accuracy().iter() {
            let entry = totals.entry(*letter).or_insert((0, 0));
            entry.0 += stats.correct;
            entry.1 += stats.total;
        }
        // A contest's times run from a station's send to an edit, waiting
        // and typing included: not answers after a send (§10 of the design).
        if session.char_set_mode == CharSetMode::Contest {
            continue;
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

/// What the mistakes screen shows: which characters are taken for which,
/// and which pairs of characters are hard together.
#[derive(Clone, Debug, PartialEq)]
pub struct MistakeStats {
    /// The most common confusions, most common first.
    pub confusions: Vec<ConfusionEntry>,
    pub bigrams: BigramHeatmap,
}

/// Both mistake views from one alignment of every group, rather than one
/// each: the history can be hundreds of sessions long.
pub fn mistake_stats(sessions: &[SessionResult], confusion_limit: usize) -> MistakeStats {
    let mut confused: BTreeMap<(char, Option<char>), u32> = BTreeMap::new();
    let mut sent_totals: BTreeMap<char, u32> = BTreeMap::new();
    let mut letters = Vec::new();
    let mut pairs: BTreeMap<(char, char), (u32, u32)> = BTreeMap::new();
    for group in sessions.iter().flat_map(|session| &session.groups) {
        let mut previous = GROUP_START_BIGRAM_TOKEN;
        for pair in align_group(&group.sent, &group.received) {
            let Some(sent) = pair.sent_char else {
                continue;
            };
            *sent_totals.entry(sent).or_insert(0) += 1;
            if !pair.matched {
                *confused.entry((sent, pair.received_char)).or_insert(0) += 1;
            }
            for ch in [previous, sent] {
                if !letters.contains(&ch) {
                    letters.push(ch);
                }
            }
            let entry = pairs.entry((previous, sent)).or_insert((0, 0));
            entry.1 += 1;
            if !pair.matched {
                entry.0 += 1;
            }
            previous = sent;
        }
    }
    MistakeStats {
        confusions: confusion_rows(confused, &sent_totals, confusion_limit),
        bigrams: bigram_grid(letters, &pairs),
    }
}

fn confusion_rows(
    confused: BTreeMap<(char, Option<char>), u32>,
    sent_totals: &BTreeMap<char, u32>,
    limit: usize,
) -> Vec<ConfusionEntry> {
    let mut rows: Vec<ConfusionEntry> = confused
        .into_iter()
        .map(|((sent, typed), count)| {
            let total = sent_totals.get(&sent).copied().unwrap_or(1);
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

fn bigram_grid(
    mut letters: Vec<char>,
    pairs: &BTreeMap<(char, char), (u32, u32)>,
) -> BigramHeatmap {
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
    let mut cells = Vec::with_capacity(letters.len() * letters.len());
    for &row in &letters {
        for &col in &letters {
            let (wrong, total) = pairs.get(&(row, col)).copied().unwrap_or((0, 0));
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

/// How often each character actually turns up, measured by running the real
/// callsign generator rather than modelling it. Nothing else can answer this:
/// the share of `Q` depends on the prefix tables and the tier, not on a weight.
fn whole_group_character_shares(
    settings: &TrainingSettings,
    state: &CharSamplingState,
    config: &CharSamplingConfig,
    pool: &[char],
    rng: &mut impl crate::rng::Rng,
) -> BTreeMap<char, f64> {
    const DRAWS: usize = 1_000;
    let mut counts: BTreeMap<char, f64> = pool.iter().map(|c| (*c, 0.0)).collect();
    let mut total = 0.0;
    let mut sampling = state.clone();
    for _ in 0..DRAWS {
        let call = match settings.curriculum.char_set_mode {
            CharSetMode::Words => generate_word_group(settings, &mut sampling, config, rng),
            // A contest's callers' calls; their exchanges are not drawn.
            CharSetMode::Contest => generate_contest_call(settings, &mut sampling, config, rng),
            _ => generate_callsign_group(settings, &mut sampling, config, rng),
        };
        for ch in call.chars() {
            if let Some(count) = counts.get_mut(&ch) {
                *count += 1.0;
                total += 1.0;
            }
        }
    }
    if total <= 0.0 {
        return counts;
    }
    for count in counts.values_mut() {
        *count /= total;
    }
    counts
}

/// What the sampler makes of each character: the chance it is drawn next,
/// and what the history says of how often it is missed. Every character the
/// history knows is listed, whatever set it was practised in, those the
/// current set does not draw after the rest: no setting hides what was
/// learned.
pub fn sampling_rows(settings: &TrainingSettings, sessions: &[SessionResult]) -> Vec<SamplingRow> {
    let state = CharSamplingState::from_history(sessions);
    let pool = compute_char_pool(settings);
    let mut config = config_from_settings(settings);
    config.thompson_sampling = false;
    let mut rng = crate::rng::SplitMix64::default();
    let probs = if !settings.curriculum.char_set_mode.draws_characters() {
        // Callsigns and words are not drawn character by character, so a
        // per-character weight would not describe them. Ask the real
        // generator instead.
        whole_group_character_shares(settings, &state, &config, &pool, &mut rng)
    } else {
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        normalize_weights(&pool, &weights)
    };
    let mut others: Vec<char> = state
        .beliefs
        .keys()
        .copied()
        .filter(|character| !pool.contains(character))
        .collect();
    // In the order Koch training unlocks them, then the rest.
    others.sort_by_key(|character| {
        let unlocked = LCWO_SEQUENCE.iter().position(|each| each == character);
        (unlocked.unwrap_or(usize::MAX), *character)
    });
    let row = |character: char, drawn_now: bool| {
        let belief = belief_for(&state, character);
        SamplingRow {
            character,
            p_error: beta_posterior_mean_error(belief),
            sampling_prob: probs.get(&character).copied().unwrap_or(0.0),
            alpha: belief.alpha,
            beta: belief.beta,
            is_letter: !character.is_ascii_digit(),
            drawn_now,
        }
    };
    pool.iter()
        .map(|character| row(*character, true))
        .chain(others.into_iter().map(|character| row(character, false)))
        .collect()
}

pub fn session_history(sessions: &[SessionResult]) -> Vec<SessionHistoryRow> {
    let mut rows: Vec<SessionHistoryRow> = sessions
        .iter()
        .enumerate()
        .map(|(index, s)| SessionHistoryRow {
            date: s.date.clone(),
            accuracy_pct: s.accuracy * 100.0,
            score: s.score,
            groups: s.groups.len() as u32,
            correct_groups: s.groups.iter().filter(|g| g.correct).count() as u32,
            total_chars: s.total_chars,
            timestamp: s.timestamp,
            mode: s.char_set_mode,
            contest: s.contest_kind(),
            log: s.stored_log(),
            assisted: s.assisted(),
            index,
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
    rows
}

/// The character sets `sessions` hold any practice in, in the order the
/// settings list them: whatever is chosen now, each keeps its history.
pub fn practised_modes(sessions: &[SessionResult]) -> Vec<CharSetMode> {
    CharSetMode::ALL
        .into_iter()
        .filter(|mode| {
            sessions
                .iter()
                .any(|session| session.char_set_mode == *mode && !session.groups.is_empty())
        })
        .collect()
}

/// Your copy span after each session of the character set `settings`
/// trains, oldest first. Empty for callsigns, which are as long as they are.
pub fn span_history(settings: &TrainingSettings, sessions: &[SessionResult]) -> Vec<EstimatePoint> {
    SpanBelief::history(sessions, settings)
}

/// Your copy speed after each session of the character set `settings`
/// trains, oldest first: the speed at which you copy the share of characters
/// their "Aim for" asks, in WPM.
pub fn speed_history(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
) -> Vec<EstimatePoint> {
    SpeedBelief::history(sessions, settings, settings.curriculum.aim.chars_right())
}

/// Each group length the next session can send: how likely it is to come
/// next, and how often a group that long is copied whole. With adaptive group
/// size off, every length is as likely as the next. Empty for callsigns and
/// words, which are as long as they are.
pub fn group_length_forecast(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
) -> Vec<LengthForecast> {
    if !settings.curriculum.char_set_mode.draws_characters() {
        return Vec::new();
    }
    let (shortest, longest) = group_size_bounds(settings);
    let target = adaptive_bounds(settings).map(|_| settings.curriculum.aim.groups_whole());
    SpanBelief::from_history(sessions, settings).forecast(shortest, longest, target)
}

/// Round speeds across the speed settings, and for each how likely the next
/// group is to be sent at about that speed, and the share of characters you
/// copy right at it. With adaptive speed off, the speed is the station's to
/// draw, evenly across the range.
pub fn speed_forecast(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
) -> Vec<SpeedForecast> {
    let (low, high) = SpeedPath::of(settings).bounds();
    let speeds = forecast_speeds(low, high);
    let belief = SpeedBelief::from_history(sessions, settings);
    let drawn = if adapts_speed(settings) {
        belief.drawn_near(&speeds, low, high, settings.curriculum.aim.chars_right())
    } else {
        station_shares(settings, &speeds)
    };
    speeds
        .into_iter()
        .zip(drawn)
        .map(|(wpm, drawn)| SpeedForecast {
            wpm,
            drawn,
            right: belief.right_at(wpm),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alignment::{CopyTally, LetterAccuracy};
    use crate::session::{GroupResult, SessionTiming};
    use crate::settings::CharSetMode;

    fn session() -> SessionResult {
        SessionResult {
            timestamp: 1,
            groups: vec![GroupResult {
                sent: "KM".into(),
                received: "K?".into(),
                correct: false,
            }],
            group_timings: vec![SessionTiming {
                time_to_complete_ms: 400.0,
                char_wpm: None,
                effective_wpm: None,
                sends: 1,
            }],
            accuracy: 0.5,
            tally: CopyTally::of("KM", "K?"),
            avg_response_ms: 400.0,
            total_chars: 2,
            score: 1.0,
            ..SessionResult::empty_for_test()
        }
    }

    /// A contest run's letters count for accuracy, but its times, which
    /// include waiting and typing, leave the average time alone.
    #[test]
    fn contest_times_stay_out_of_the_letters_times() {
        use crate::contest::bot::script::{Habit, run_settings, work};
        use crate::contest::bot::{Operator, Runtime};
        let mut calls = SessionResult::empty_for_test();
        calls.char_set_mode = CharSetMode::Callsign;
        for _ in 0..10 {
            calls.groups.push(GroupResult {
                sent: "K5XYZ".into(),
                received: "K5XYZ".into(),
                correct: true,
            });
            calls.group_timings.push(SessionTiming {
                time_to_complete_ms: 1_500.0,
                char_wpm: Some(25.0),
                effective_wpm: Some(25.0),
                sends: 1,
            });
            calls.tally.add("K5XYZ", "K5XYZ");
        }
        let mut settings = run_settings(25.0);
        settings.contest.contest_minutes = 2;
        let sampler = crate::Sampler::new(&[], &settings, 4);
        let mut runtime = Runtime::start(4, settings, sampler);
        work(&mut runtime, &mut Operator::new(Habit::HumanPaced), &[]);
        let run = runtime.machine.result(2, "2026-10-20".into());
        assert!(
            run.group_timings
                .iter()
                .any(|t| t.time_to_complete_ms > 1_500.0)
        );
        let letter = |history: &[SessionResult], ch: char| {
            character_diagnostics(history)
                .into_iter()
                .find(|diag| diag.letter == ch)
        };
        let alone = letter(std::slice::from_ref(&calls), 'K').expect("K");
        let both = [calls.clone(), run.clone()];
        let with_run = letter(&both, 'K').expect("K");
        assert_eq!(with_run.avg_ms, alone.avg_ms);
        let counted = run.letter_accuracy().get(&'K').map_or(0, |k| k.total);
        assert_eq!(with_run.total, alone.total + counted);
        // A letter only the run has shows no time at all.
        let only = run
            .groups
            .iter()
            .flat_map(|group| group.sent.chars())
            .find(|ch| !"K5XYZ".contains(*ch))
            .expect("a letter K5XYZ lacks");
        assert_eq!(letter(&both, only).map(|diag| diag.avg_ms), Some(0.0));
    }

    /// A run with Show copy on is assisted, and counts in every stat all
    /// the same: the calendar, accuracy, Letters, Mistakes and copy speed.
    #[test]
    fn an_assisted_run_counts_in_every_stat() {
        use crate::contest::bot::script::{Habit, run_settings, work};
        use crate::contest::bot::{Operator, Runtime};
        let mut settings = run_settings(25.0);
        settings.contest.contest_minutes = 2;
        settings.contest.contest_show_copy = true;
        let sampler = crate::Sampler::new(&[], &settings, 4);
        let mut runtime = Runtime::start(4, settings.clone(), sampler);
        // A digit off in every number: mistakes to count.
        work(&mut runtime, &mut Operator::new(Habit::DigitOff), &[]);
        let run = runtime.machine.result(2, "2026-10-20".into());
        assert!(run.assisted(), "{}", run.alphabet_fingerprint);
        let history = [run.clone()];
        let days = crate::heatmap::aggregate_heatmap(&history);
        assert_eq!(days.get("2026-10-20").map(|day| day.sessions), Some(1));
        assert_eq!(accuracy_chart(&history).len(), 1);
        let letters = character_diagnostics(&history);
        let counted: u32 = letters.iter().map(|letter| letter.total).sum();
        let sent: u32 = run.tally.sent.values().map(|letter| letter.total).sum();
        assert_eq!(counted, sent, "every character it sent");
        assert!(
            mistake_stats(&history, 8)
                .confusions
                .iter()
                .any(|confusion| confusion.sent.is_ascii_digit()),
            "the digits typed wrong"
        );
        assert_eq!(speed_history(&settings, &history).len(), 1);
        let row = &session_history(&history)[0];
        assert_eq!(row.contest, Some(ContestKind::Wpx));
        assert!(row.assisted, "History says so");
    }

    #[test]
    fn diagnostics_and_confusion() {
        let sessions = [session()];
        let diag = character_diagnostics(&sessions);
        assert_eq!(diag.len(), 2);
        assert!(!mistake_stats(&sessions, 8).confusions.is_empty());
    }

    fn plain(date: &str, timestamp: u64, groups: &[(&str, &str)]) -> SessionResult {
        let mut out = session();
        out.date = date.to_string();
        out.timestamp = timestamp;
        out.tally = CopyTally::default();
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
                char_wpm: Some(20.0),
                effective_wpm: None,
                sends: 1,
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
    fn bigrams_rank_the_group_start_first_and_unknown_characters_last() {
        // `+` is a Morse character the LCWO sequence never teaches; it must not
        // overflow the sort key.
        let session = plain("2026-09-01", 1, &[("K+", "KM")]);
        let heat = mistake_stats(&[session], 8).bigrams;
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
        assert!(mistake_stats(&[], 8).bigrams.letters.is_empty());
    }

    #[test]
    fn confusion_is_ordered_and_capped() {
        let session = plain(
            "2026-09-01",
            1,
            &[("KMU", "XMU"), ("KMU", "XMU"), ("KMU", "KMR")],
        );
        let rows = mistake_stats(&[session], 8).confusions;
        assert_eq!(rows[0].sent, 'K');
        assert_eq!(rows[0].typed, Some('X'));
        assert_eq!(rows[0].count, 2);
        // Two of the three K sends were copied as X.
        assert!((rows[0].percentage - 200.0 / 3.0).abs() < 1e-9);
        assert!(mistake_stats(&[], 8).confusions.is_empty());
        // The limit is honoured.
        let long = plain("2026-09-02", 2, &[("KMURE", "XXXXX")]);
        assert_eq!(mistake_stats(&[long], 2).confusions.len(), 2);
    }

    #[test]
    fn a_dropped_character_is_reported_as_no_answer() {
        let session = plain("2026-09-01", 1, &[("KM", "K")]);
        let rows = mistake_stats(&[session], 8).confusions;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sent, 'M');
        assert_eq!(rows[0].typed, None);
    }

    #[test]
    fn characters_that_were_never_sent_are_left_out_of_every_table() {
        // An extra character was typed in the middle of the group.
        let session = plain("2026-09-01", 1, &[("KM", "KXM")]);
        let diagnostics = character_diagnostics(std::slice::from_ref(&session));
        assert!(diagnostics.iter().all(|d| d.letter != 'X'));
        let mistakes = mistake_stats(&[session], 8);
        assert!(!mistakes.bigrams.letters.contains(&'X'));
        assert!(mistakes.confusions.iter().all(|c| c.sent != 'X'));
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
        // Each row still names the session it came from, and its mode.
        assert_eq!((rows[0].index, rows[1].index), (1, 0));
        assert_eq!(rows[0].mode, CharSetMode::Koch);
    }

    #[test]
    fn sampling_rows_follow_the_pool_it_falls_back_to() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.mixed_letters_percent = 100;
        settings.curriculum.custom_sequence = vec!['0'];
        // A letters-only mixed pool over a digits-only sequence sends digits.
        let rows = sampling_rows(&settings, &[]);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| !row.is_letter));
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
    fn callsign_sampling_shows_what_callsigns_actually_contain() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MAX;
        let rows = sampling_rows(&settings, &[]);

        let pool = compute_char_pool(&settings);
        assert_eq!(rows.len(), pool.len());
        let total: f64 = rows.iter().map(|row| row.sampling_prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-6,
            "the shares should account for every character sent, got {total}"
        );
        assert!(
            rows.iter().all(|row| row.sampling_prob > 0.0),
            "every character a callsign can contain should turn up"
        );
        // A share is measured, not modelled: the separator is a digit on every
        // single call, so digits cannot be the rare ones here.
        let digits: f64 = rows
            .iter()
            .filter(|row| row.character.is_ascii_digit())
            .map(|row| row.sampling_prob)
            .sum();
        assert!(digits > 0.1, "digits carry every separator, got {digits}");

        // A tier that sends no portables must never show a slash.
        let mut tier_one = settings;
        tier_one.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MIN;
        assert!(
            !sampling_rows(&tier_one, &[])
                .iter()
                .any(|row| row.character == '/'),
            "tier 1 has no portable calls"
        );
    }

    /// A contest's snapshot is of its callers' calls, drawn as the run draws
    /// them: few portables and stations abroad, where the top tier sends
    /// plenty for practice. Drawn plainly, with no lean on any character,
    /// the difference is the spec's alone.
    #[test]
    fn contest_sampling_shows_what_its_callers_send() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MAX;
        settings.auto_level.error_weight_strength = 0.0;
        settings.auto_level.char_sampling_coverage_strength = 0.0;
        let slash = |mode| {
            let mut settings = settings.clone();
            settings.curriculum.char_set_mode = mode;
            let rows = sampling_rows(&settings, &[]);
            let total: f64 = rows.iter().map(|row| row.sampling_prob).sum();
            assert!((total - 1.0).abs() < 1e-6, "{mode:?}: {total}");
            rows.iter()
                .find(|row| row.character == '/')
                .map_or(0.0, |row| row.sampling_prob)
        };
        let calls = slash(CharSetMode::Callsign);
        let callers = slash(CharSetMode::Contest);
        assert!(
            callers > 0.0,
            "a contest still hears a portable now and then"
        );
        assert!(callers < calls / 2.0, "{callers} against {calls}");
    }

    #[test]
    fn callsign_sampling_shows_mean_weights_even_with_thompson_sampling_on() {
        // The table describes the average draw, so random Thompson draws must
        // not leak into it, whatever the setting says.
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MAX;
        settings.auto_level.char_sampling_thompson = false;
        let shares = |rows: Vec<SamplingRow>| -> Vec<(char, f64)> {
            rows.into_iter()
                .map(|row| (row.character, row.sampling_prob))
                .collect()
        };
        let without = shares(sampling_rows(&settings, &[]));
        settings.auto_level.char_sampling_thompson = true;
        assert_eq!(shares(sampling_rows(&settings, &[])), without);
    }

    /// A character's history counts wherever it was practised: a K missed
    /// in a digits session is as missed in Koch.
    #[test]
    fn sampling_rows_count_every_char_set_mode() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        let mut koch = session();
        koch.char_set_mode = CharSetMode::Koch;
        koch.tally = CopyTally::default();
        koch.tally.sent.insert(
            'K',
            LetterAccuracy {
                correct: 10,
                total: 10,
            },
        );
        let mut digits = koch.clone();
        digits.char_set_mode = CharSetMode::Digits;
        digits.tally.sent.insert(
            'K',
            LetterAccuracy {
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
        let (alpha, beta) = k_belief(&sampling_rows(&settings, &[koch.clone()]));
        let both = k_belief(&sampling_rows(&settings, &[koch, digits]));
        assert_eq!(both, (alpha, beta + 10.0));
    }

    /// A character the current set does not draw is still listed, after the
    /// rest, with what its history says and no chance of being drawn.
    #[test]
    fn characters_the_set_does_not_draw_are_still_listed() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        let mut practised = session();
        for (character, correct) in [('Q', 1), ('5', 8)] {
            practised
                .tally
                .sent
                .insert(character, LetterAccuracy { correct, total: 10 });
        }
        let rows = sampling_rows(&settings, &[practised]);
        let drawn: Vec<char> = rows
            .iter()
            .filter(|row| row.drawn_now)
            .map(|row| row.character)
            .collect();
        assert_eq!(drawn, compute_char_pool(&settings));
        let others: Vec<&SamplingRow> = rows.iter().filter(|row| !row.drawn_now).collect();
        assert_eq!(
            others.iter().map(|row| row.character).collect::<Vec<_>>(),
            ['5', 'Q'],
            "in the order Koch unlocks them"
        );
        assert!(others.iter().all(|row| row.sampling_prob == 0.0));
        assert!(others[1].p_error > 0.8, "Q is mostly missed");
        assert!(rows.iter().take_while(|row| row.drawn_now).count() == drawn.len());
    }

    /// Picking another unlock order — the guided setup does — keeps what the
    /// history says of each character: a K always missed under LCWO's order
    /// is still the hardest under MorseMania's.
    #[test]
    fn a_new_unlock_order_keeps_what_was_learned_of_each_character() {
        let mut lcwo = TrainingSettings::default();
        lcwo.curriculum.char_set_mode = CharSetMode::Koch;
        let mut missed = session();
        missed.alphabet_fingerprint = lcwo.alphabet_fingerprint();
        missed.tally.sent.insert(
            'K',
            LetterAccuracy {
                correct: 0,
                total: 20,
            },
        );
        let mut mania = lcwo.clone();
        mania.choose_sequence(crate::sequences::SequenceChoice::Preset(
            crate::sequences::PresetId::MorseMania,
        ));
        assert_ne!(lcwo.alphabet_fingerprint(), mania.alphabet_fingerprint());
        let rows = sampling_rows(&mania, &[missed]);
        let k = rows
            .iter()
            .find(|row| row.character == 'K')
            .expect("K in MorseMania's pool");
        assert!(k.p_error > 0.9, "{}", k.p_error);
        assert!(
            rows.iter()
                .all(|row| row.character == 'K' || row.sampling_prob < k.sampling_prob),
            "K is drawn most"
        );
    }

    fn length_settings(adaptive: bool) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 7;
        settings.curriculum.adaptive_group_size = adaptive;
        settings
    }

    #[test]
    fn with_adaptive_length_off_every_length_is_as_likely() {
        let rows = group_length_forecast(&length_settings(false), &[session()]);
        assert_eq!(
            rows.iter().map(|row| row.length).collect::<Vec<_>>(),
            [2, 3, 4, 5, 6, 7]
        );
        assert!(rows.iter().all(|row| (row.drawn - 1.0 / 6.0).abs() < 1e-12));
        // What is copied whole is still the model's to say.
        assert!(rows.windows(2).all(|pair| pair[0].whole > pair[1].whole));
    }

    #[test]
    fn with_adaptive_length_on_the_lengths_lean_and_still_add_up() {
        let rows = group_length_forecast(&length_settings(true), &[session()]);
        let total: f64 = rows.iter().map(|row| row.drawn).sum();
        assert!((total - 1.0).abs() < 1e-9);
        assert!(rows.iter().any(|row| (row.drawn - 1.0 / 6.0).abs() > 0.01));
    }

    fn speed_settings(adaptive: bool) -> TrainingSettings {
        let mut settings = length_settings(false);
        settings.playback.char_wpm_min = 10.0;
        settings.playback.char_wpm_max = 50.0;
        settings.playback.link_char_to_effective = true;
        settings.playback.adaptive_speed = adaptive;
        settings
    }

    #[test]
    fn with_adaptive_speed_off_the_station_draws_evenly() {
        let rows = speed_forecast(&speed_settings(false), &[session()]);
        assert_eq!(
            rows.iter().map(|row| row.wpm).collect::<Vec<_>>(),
            [10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0]
        );
        assert!((rows.iter().map(|row| row.drawn).sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(
            rows[1..8]
                .iter()
                .all(|row| (row.drawn - 0.125).abs() < 1e-9)
        );
        // What is copied right is still the model's to say.
        assert!(rows.windows(2).all(|pair| pair[0].right > pair[1].right));
    }

    #[test]
    fn with_adaptive_speed_on_the_speeds_lean_and_still_add_up() {
        let rows = speed_forecast(&speed_settings(true), &[session()]);
        assert!((rows.iter().map(|row| row.drawn).sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(rows.iter().any(|row| (row.drawn - 0.125).abs() > 0.05));
    }

    /// In Contest mode the forecast is the callers': drawn from your copy
    /// speed while their switch is on, whatever group training's says, and
    /// evenly across the range while it is off.
    #[test]
    fn a_contests_forecast_follows_the_callers_switch() {
        for group in [false, true] {
            let mut contest = speed_settings(group);
            contest.curriculum.char_set_mode = CharSetMode::Contest;
            // Drawn evenly, every speed between the ends is an eighth.
            let leans = |settings: &TrainingSettings| {
                let rows = speed_forecast(settings, &[]);
                assert!((rows.iter().map(|row| row.drawn).sum::<f64>() - 1.0).abs() < 1e-9);
                rows[1..rows.len() - 1]
                    .iter()
                    .any(|row| (row.drawn - 0.125).abs() > 0.05)
            };
            assert!(leans(&contest), "group training's {group}");
            contest.contest.contest_adapt_speed = false;
            assert!(!leans(&contest), "group training's {group}");
        }
    }

    #[test]
    fn one_speed_is_the_only_one_sent() {
        let mut settings = speed_settings(true);
        settings.playback.char_wpm_max = 10.0;
        let rows = speed_forecast(&settings, &[session()]);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].wpm, rows[0].drawn), (10.0, 1.0));
    }

    #[test]
    fn every_set_with_practice_in_it_is_listed_in_order() {
        let of = |mode: CharSetMode| SessionResult {
            char_set_mode: mode,
            ..session()
        };
        let empty = SessionResult {
            char_set_mode: CharSetMode::Digits,
            groups: Vec::new(),
            ..session()
        };
        assert_eq!(
            practised_modes(&[of(CharSetMode::Words), of(CharSetMode::Koch), empty]),
            [CharSetMode::Koch, CharSetMode::Words]
        );
        assert!(practised_modes(&[]).is_empty());
    }

    #[test]
    fn callsigns_have_no_group_length_to_show() {
        let mut settings = length_settings(true);
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        assert!(group_length_forecast(&settings, &[session()]).is_empty());
        assert!(span_history(&settings, &[session()]).is_empty());
    }

    #[test]
    fn the_span_history_has_a_point_for_each_session_of_the_set() {
        let mut later = session();
        later.timestamp = 2;
        let points = span_history(&length_settings(false), &[session(), later]);
        assert_eq!(points.len(), 2);
    }
}
