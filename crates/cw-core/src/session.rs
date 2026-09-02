//! Group-training session runtime and result construction.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::alignment::{
    calculate_group_letter_accuracy, calculate_overall_character_accuracy, LetterAccuracy,
};
use crate::score::{
    calculate_alphabet_size, calculate_effective_alphabet_size, calculate_total_chars,
    compute_average_response_ms, compute_session_score, ScoreConstants,
};
use crate::settings::{CharSetMode, TrainingSettings};

fn session_level_default() -> u32 {
    1
}

fn session_mode_legacy() -> CharSetMode {
    CharSetMode::Koch
}

fn session_wpm_default() -> f64 {
    18.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeStatus {
    Starting,
    PlayingGroup,
    WaitingForAnswer,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupResult {
    pub sent: String,
    pub received: String,
    pub correct: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTiming {
    pub time_to_complete_ms: f64,
    pub per_char_ms: f64,
    #[serde(default)]
    pub char_wpm: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResult {
    pub date: String,
    pub timestamp: u64,
    pub started_at: u64,
    pub finished_at: u64,
    pub groups: Vec<GroupResult>,
    pub group_timings: Vec<SessionTiming>,
    pub accuracy: f64,
    pub letter_accuracy: BTreeMap<char, LetterAccuracy>,
    pub alphabet_size: u32,
    pub avg_response_ms: f64,
    pub total_chars: u32,
    pub effective_alphabet_size: f64,
    pub score: f64,
    #[serde(alias = "kochLevel", default = "session_level_default")]
    pub level: u32,
    #[serde(default = "session_level_default")]
    pub digits_level: u32,
    #[serde(default = "session_mode_legacy")]
    pub char_set_mode: CharSetMode,
    #[serde(default = "session_wpm_default")]
    pub char_wpm: f64,
    #[serde(default = "session_wpm_default")]
    pub effective_wpm: f64,
    /// Full progress alphabet at the time of the session. Empty on legacy saves.
    #[serde(default)]
    pub alphabet_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub accuracy: f64,
    pub groups: Vec<GroupResult>,
    pub avg_response_ms: f64,
    pub score: f64,
}

#[derive(Clone, Debug)]
pub struct GroupSession {
    pub status: RuntimeStatus,
    pub session_id: u64,
    pub started_at: u64,
    pub groups: Vec<String>,
    pub user_input: Vec<String>,
    pub confirmed: Vec<bool>,
    pub current_group: usize,
    pub focused_group: usize,
    pub group_start_at: Vec<u64>,
    pub group_end_at: Vec<u64>,
    pub group_answer_at: Vec<u64>,
    pub group_char_wpm: Vec<f64>,
    pub group_effective_wpm: Vec<f64>,
    pub error_message: Option<String>,
    /// Settings the session was started with. Scoring and auto-level must use this
    /// snapshot, not whatever the live settings signal happens to hold.
    pub settings: TrainingSettings,
}

impl GroupSession {
    pub fn new(session_id: u64, started_at: u64, num_groups: usize, settings: TrainingSettings) -> Self {
        Self {
            status: RuntimeStatus::Starting,
            session_id,
            started_at,
            groups: vec![String::new(); num_groups],
            user_input: vec![String::new(); num_groups],
            confirmed: vec![false; num_groups],
            current_group: 0,
            focused_group: 0,
            group_start_at: vec![0; num_groups],
            group_end_at: vec![0; num_groups],
            group_answer_at: vec![0; num_groups],
            group_char_wpm: vec![0.0; num_groups],
            group_effective_wpm: vec![0.0; num_groups],
            error_message: None,
            settings,
        }
    }

    pub fn set_group(&mut self, index: usize, text: String) {
        if let Some(slot) = self.groups.get_mut(index) {
            *slot = text;
        }
        if !self.confirmed.get(index).copied().unwrap_or(false) {
            if let Some(slot) = self.user_input.get_mut(index) {
                slot.clear();
            }
        }
    }

    pub fn begin_group(&mut self, index: usize, now_ms: u64) {
        self.current_group = index;
        self.focused_group = index;
        self.status = RuntimeStatus::PlayingGroup;
        if let Some(slot) = self.group_start_at.get_mut(index) {
            *slot = now_ms;
        }
    }

    pub fn end_playback(&mut self, index: usize, now_ms: u64, char_wpm: f64, effective_wpm: f64) {
        if let Some(slot) = self.group_end_at.get_mut(index) {
            *slot = now_ms;
        }
        if let Some(slot) = self.group_char_wpm.get_mut(index) {
            *slot = char_wpm;
        }
        if let Some(slot) = self.group_effective_wpm.get_mut(index) {
            *slot = effective_wpm;
        }
        self.status = RuntimeStatus::WaitingForAnswer;
    }

    pub fn set_input(&mut self, index: usize, value: String) {
        if index != self.current_group {
            return;
        }
        if self.confirmed.get(index).copied().unwrap_or(false) {
            return;
        }
        if let Some(slot) = self.user_input.get_mut(index) {
            *slot = value;
        }
    }

    /// Returns true the first time this group is confirmed.
    pub fn confirm(&mut self, index: usize, value: String, answered_at: u64) -> bool {
        let Some(flag) = self.confirmed.get_mut(index) else {
            return false;
        };
        if *flag {
            return false;
        }
        *flag = true;
        if let Some(slot) = self.user_input.get_mut(index) {
            *slot = value;
        }
        if let Some(slot) = self.group_answer_at.get_mut(index) {
            if *slot == 0 {
                *slot = answered_at;
            }
        }
        let next = (index + 1).min(self.groups.len().saturating_sub(1));
        self.focused_group = next;
        true
    }

    pub fn record_answer_time_if_empty(&mut self, index: usize, answered_at: u64) {
        if let Some(slot) = self.group_answer_at.get_mut(index) {
            if *slot == 0 {
                *slot = answered_at;
            }
        }
    }

    /// Drop a length-match stamp if the user backspaces before confirm.
    pub fn clear_answer_time(&mut self, index: usize) {
        if self.confirmed.get(index).copied().unwrap_or(false) {
            return;
        }
        if let Some(slot) = self.group_answer_at.get_mut(index) {
            *slot = 0;
        }
    }

    pub fn input_locked(&self, index: usize) -> bool {
        self.settings.lock_input_during_group_playback
            && self.status == RuntimeStatus::PlayingGroup
            && index == self.current_group
    }

    pub fn all_groups_confirmed(&self) -> bool {
        !self.groups.is_empty()
            && self.groups.iter().enumerate().all(|(index, sent)| {
                !sent.is_empty() && self.confirmed.get(index).copied().unwrap_or(false)
            })
    }

    pub fn build_timings(&self, fallback_timeout_ms: f64) -> Vec<SessionTiming> {
        self.groups
            .iter()
            .enumerate()
            .map(|(index, sent)| {
                let end_at = self.group_end_at.get(index).copied().unwrap_or(0);
                let raw_answer = self.group_answer_at.get(index).copied().unwrap_or(0);
                let fallback = if end_at > 0 && fallback_timeout_ms > 0.0 {
                    end_at + fallback_timeout_ms as u64
                } else {
                    0
                };
                let answer_at = if raw_answer > 0 { raw_answer } else { fallback };
                // Copy-during-play can stamp answer_at before Morse ends. Treat that
                // as an immediate response so it is not dropped as a 0 ms sample.
                let delta = if answer_at > 0 && end_at > 0 && answer_at < end_at {
                    1.0
                } else {
                    answer_at.saturating_sub(end_at) as f64
                };
                let per_char = if sent.is_empty() {
                    0.0
                } else {
                    (delta / sent.chars().count() as f64).round()
                };
                let wpm = self.group_char_wpm.get(index).copied().filter(|v| *v > 0.0);
                SessionTiming {
                    time_to_complete_ms: delta,
                    per_char_ms: per_char,
                    char_wpm: wpm,
                }
            })
            .collect()
    }
}

fn group_was_scored(session: &GroupSession, index: usize) -> bool {
    session.confirmed.get(index).copied().unwrap_or(false)
        && session
            .groups
            .get(index)
            .map(|sent| !sent.is_empty())
            .unwrap_or(false)
}

pub fn answer_length_matches(sent: &str, received: &str) -> bool {
    !sent.is_empty() && received.trim().chars().count() == sent.chars().count()
}

pub fn build_session_result(
    session: &GroupSession,
    settings: &TrainingSettings,
    finished_at: u64,
    date: String,
) -> SessionResult {
    let answers: Vec<String> = session
        .user_input
        .iter()
        .map(|a| a.trim().to_ascii_uppercase())
        .collect();
    let pairs: Vec<(String, String)> = session
        .groups
        .iter()
        .zip(answers.iter())
        .enumerate()
        .filter(|(index, _)| group_was_scored(session, *index))
        .map(|(_, (sent, recv))| (sent.clone(), recv.clone()))
        .collect();

    let groups: Vec<GroupResult> = pairs
        .iter()
        .map(|(sent, received)| GroupResult {
            sent: sent.clone(),
            received: received.clone(),
            correct: sent == received,
        })
        .collect();

    let letter_accuracy = calculate_group_letter_accuracy(&pairs);
    let accuracy = calculate_overall_character_accuracy(&pairs);
    let sent_only: Vec<&str> = pairs.iter().map(|(s, _)| s.as_str()).collect();
    let alphabet_size = calculate_alphabet_size(&sent_only);
    let effective_alphabet_size = calculate_effective_alphabet_size(&sent_only);
    let all_timings = session.build_timings(settings.group_timeout * 1000.0);
    let timings: Vec<SessionTiming> = all_timings
        .into_iter()
        .enumerate()
        .filter(|(index, _)| group_was_scored(session, *index))
        .map(|(_, timing)| timing)
        .collect();
    let avg_response_ms =
        compute_average_response_ms(&timings.iter().map(|t| t.per_char_ms).collect::<Vec<_>>());
    let total_chars = calculate_total_chars(&sent_only);
    let score = compute_session_score(
        effective_alphabet_size,
        accuracy,
        avg_response_ms,
        total_chars,
        ScoreConstants::default(),
    );
    let played_wpm: Vec<f64> = session
        .group_char_wpm
        .iter()
        .enumerate()
        .filter(|(index, wpm)| group_was_scored(session, *index) && **wpm > 0.0)
        .map(|(_, wpm)| *wpm)
        .collect();
    let char_wpm = if played_wpm.is_empty() {
        settings.char_wpm_min
    } else {
        played_wpm.iter().sum::<f64>() / played_wpm.len() as f64
    };
    let played_effective: Vec<f64> = session
        .group_effective_wpm
        .iter()
        .enumerate()
        .filter(|(index, wpm)| group_was_scored(session, *index) && **wpm > 0.0)
        .map(|(_, wpm)| *wpm)
        .collect();
    let effective_wpm = if played_effective.is_empty() {
        settings.effective_wpm_min.min(char_wpm)
    } else {
        played_effective.iter().sum::<f64>() / played_effective.len() as f64
    };

    SessionResult {
        date,
        timestamp: finished_at,
        started_at: session.started_at,
        finished_at,
        groups,
        group_timings: timings,
        accuracy,
        letter_accuracy,
        alphabet_size,
        avg_response_ms,
        total_chars,
        effective_alphabet_size,
        score,
        level: match settings.char_set_mode {
            CharSetMode::Digits => settings.digits_level,
            _ => settings.level,
        },
        digits_level: settings.digits_level,
        char_set_mode: settings.char_set_mode,
        char_wpm,
        effective_wpm,
        alphabet_fingerprint: settings.alphabet_fingerprint(),
    }
}

impl SessionResult {
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            accuracy: self.accuracy,
            groups: self.groups.clone(),
            avg_response_ms: self.avg_response_ms,
            score: self.score,
        }
    }

    /// Whether this session's letter stats should seed sampling for `settings`.
    pub fn usable_for_sampling(&self, settings: &TrainingSettings) -> bool {
        if self.char_set_mode != settings.char_set_mode {
            return false;
        }
        self.alphabet_fingerprint.is_empty()
            || self.alphabet_fingerprint == settings.alphabet_fingerprint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_copy_is_full_accuracy() {
        let mut session = GroupSession::new(1, 0, 2, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.set_group(1, "UK".into());
        session.confirm(0, "KM".into(), 1000);
        session.confirm(1, "UK".into(), 2000);
        session.group_end_at[0] = 500;
        session.group_end_at[1] = 1500;
        let settings = TrainingSettings::default();
        let result = build_session_result(&session, &settings, 3000, "2026-09-01".into());
        assert!((result.accuracy - 1.0).abs() < 1e-9);
        assert!(result.groups.iter().all(|g| g.correct));
        assert!((result.char_wpm - 18.0).abs() < 1e-9);
    }

    #[test]
    fn unconfirmed_generated_groups_are_not_scored() {
        let mut session = GroupSession::new(1, 0, 3, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.set_group(1, "UK".into());
        session.set_group(2, "RS".into());
        session.confirm(0, "KM".into(), 1000);
        session.group_end_at[0] = 500;
        session.group_char_wpm[0] = 22.0;
        let settings = TrainingSettings::default();
        let result = build_session_result(&session, &settings, 3000, "2026-09-01".into());
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.group_timings.len(), 1);
        assert!(result.groups[0].correct);
        assert!((result.accuracy - 1.0).abs() < 1e-9);
        assert!((result.char_wpm - 22.0).abs() < 1e-9);
    }

    #[test]
    fn confirm_is_idempotent() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        assert!(session.confirm(0, "KM".into(), 100));
        assert!(!session.confirm(0, "XX".into(), 200));
        assert_eq!(session.user_input[0], "KM");
        session.set_input(0, "ZZ".into());
        assert_eq!(session.user_input[0], "KM");
    }

    #[test]
    fn set_input_ignores_other_groups() {
        let mut session = GroupSession::new(1, 0, 2, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.set_group(1, "UK".into());
        session.current_group = 0;
        session.set_input(1, "UK".into());
        assert_eq!(session.user_input[1], "");
        session.set_input(0, "KM".into());
        assert_eq!(session.user_input[0], "KM");
    }

    #[test]
    fn all_groups_confirmed_requires_every_sent_group() {
        let mut session = GroupSession::new(1, 0, 2, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.set_group(1, "UK".into());
        session.confirm(0, "KM".into(), 1);
        assert!(!session.all_groups_confirmed());
        session.confirm(1, "".into(), 2);
        assert!(session.all_groups_confirmed());
    }

    #[test]
    fn timeout_empty_answer_is_scored_wrong() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.confirm(0, "".into(), 1000);
        session.group_end_at[0] = 500;
        let result = build_session_result(&session, &TrainingSettings::default(), 3000, "2026-09-01".into());
        assert_eq!(result.groups.len(), 1);
        assert!(!result.groups[0].correct);
        assert!(result.accuracy < 0.01);
    }

    #[test]
    fn answer_length_uses_characters() {
        assert!(answer_length_matches("KM", "km"));
        assert!(answer_length_matches("KM", " km "));
        assert!(!answer_length_matches("KM", "K"));
        assert!(!answer_length_matches("", "KM"));
    }

    #[test]
    fn set_group_clears_unconfirmed_input() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_input(0, "XX".into());
        session.set_group(0, "KM".into());
        assert_eq!(session.user_input[0], "");
        session.set_input(0, "KM".into());
        session.confirm(0, "KM".into(), 1);
        session.set_group(0, "UK".into());
        assert_eq!(session.user_input[0], "KM");
    }

    #[test]
    fn answer_during_playback_counts_as_immediate() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.group_end_at[0] = 2000;
        session.confirm(0, "KM".into(), 1500);
        let timings = session.build_timings(10_000.0);
        assert_eq!(timings.len(), 1);
        assert!((timings[0].time_to_complete_ms - 1.0).abs() < 1e-9);
        assert!((timings[0].per_char_ms - 1.0).abs() < 1e-9);
    }

    #[test]
    fn result_uses_played_effective_wpm() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.confirm(0, "KM".into(), 1000);
        session.group_end_at[0] = 500;
        session.group_char_wpm[0] = 25.0;
        session.group_effective_wpm[0] = 12.0;
        let mut settings = TrainingSettings::default();
        settings.effective_wpm_min = 18.0;
        settings.effective_wpm_max = 25.0;
        let result = build_session_result(&session, &settings, 3000, "2026-09-01".into());
        assert!((result.char_wpm - 25.0).abs() < 1e-9);
        assert!((result.effective_wpm - 12.0).abs() < 1e-9);
        assert_eq!(result.alphabet_fingerprint, settings.alphabet_fingerprint());
    }

    #[test]
    fn sampling_history_requires_matching_mode_and_alphabet() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Koch;
        let mut koch = build_session_result(
            &GroupSession::new(1, 0, 1, settings.clone()),
            &settings,
            1,
            "2026-09-01".into(),
        );
        koch.char_set_mode = CharSetMode::Koch;
        koch.alphabet_fingerprint = settings.alphabet_fingerprint();
        assert!(koch.usable_for_sampling(&settings));

        let mut digits = koch.clone();
        digits.char_set_mode = CharSetMode::Digits;
        assert!(!digits.usable_for_sampling(&settings));

        let mut other_seq = settings.clone();
        other_seq.custom_sequence = crate::sequences::TRADITIONAL_KOCH_SEQUENCE.to_vec();
        koch.alphabet_fingerprint = settings.alphabet_fingerprint();
        assert!(!koch.usable_for_sampling(&other_seq));

        let mut legacy = koch.clone();
        legacy.char_set_mode = CharSetMode::Koch;
        legacy.alphabet_fingerprint.clear();
        assert!(legacy.usable_for_sampling(&settings));
    }

    #[test]
    fn input_locked_uses_session_settings() {
        let mut settings = TrainingSettings::default();
        settings.lock_input_during_group_playback = true;
        let mut session = GroupSession::new(1, 0, 1, settings);
        session.begin_group(0, 0);
        assert!(session.input_locked(0));
        session.settings.lock_input_during_group_playback = false;
        assert!(!session.input_locked(0));
        session.end_playback(0, 1, 20.0, 18.0);
        session.settings.lock_input_during_group_playback = true;
        assert!(!session.input_locked(0));
    }

    #[test]
    fn digits_session_records_digits_level() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Digits;
        settings.digits_level = 4;
        settings.level = 1;
        let mut session = GroupSession::new(1, 0, 1, settings.clone());
        session.set_group(0, "01".into());
        session.confirm(0, "01".into(), 1000);
        session.group_end_at[0] = 500;
        let result = build_session_result(&session, &settings, 3000, "2026-09-01".into());
        assert_eq!(result.level, 4);
        assert_eq!(result.digits_level, 4);
        assert_eq!(result.char_set_mode, CharSetMode::Digits);
    }

    #[test]
    fn shortening_answer_clears_completion_stamp() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.group_end_at[0] = 1000;
        session.record_answer_time_if_empty(0, 1100);
        session.clear_answer_time(0);
        session.confirm(0, "KM".into(), 1400);
        let timings = session.build_timings(10_000.0);
        assert!((timings[0].time_to_complete_ms - 400.0).abs() < 1e-9);
    }

    #[test]
    fn legacy_session_json_fills_new_fields() {
        let timing: SessionTiming =
            serde_json::from_str(r#"{"timeToCompleteMs":1,"perCharMs":1}"#).unwrap();
        assert_eq!(timing.char_wpm, None);
        let raw = r#"{
            "date":"2026-01-01","timestamp":1,"startedAt":0,"finishedAt":1,
            "groups":[],"groupTimings":[],"accuracy":1,"letterAccuracy":{},
            "alphabetSize":0,"avgResponseMs":0,"totalChars":0,
            "effectiveAlphabetSize":0,"score":0,"level":1
        }"#;
        let result: SessionResult = serde_json::from_str(raw).unwrap();
        assert_eq!(result.digits_level, 1);
        assert_eq!(result.char_set_mode, CharSetMode::Koch);
        assert_eq!(result.char_wpm, 18.0);
        assert_eq!(result.effective_wpm, 18.0);
        assert!(result.alphabet_fingerprint.is_empty());
    }
}
