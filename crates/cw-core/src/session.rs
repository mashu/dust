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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeStatus {
    Idle,
    Starting,
    PlayingGroup,
    WaitingForAnswer,
    Completing,
    Results,
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
    pub koch_level: u32,
    pub digits_level: u32,
    pub char_set_mode: CharSetMode,
    pub char_wpm: f64,
    pub effective_wpm: f64,
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
    pub error_message: Option<String>,
}

impl GroupSession {
    pub fn new(session_id: u64, started_at: u64, num_groups: usize) -> Self {
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
            error_message: None,
        }
    }

    pub fn set_group(&mut self, index: usize, text: String) {
        if let Some(slot) = self.groups.get_mut(index) {
            *slot = text;
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

    pub fn end_playback(&mut self, index: usize, now_ms: u64, char_wpm: f64) {
        if let Some(slot) = self.group_end_at.get_mut(index) {
            *slot = now_ms;
        }
        if let Some(slot) = self.group_char_wpm.get_mut(index) {
            *slot = char_wpm;
        }
        self.status = RuntimeStatus::WaitingForAnswer;
    }

    pub fn set_input(&mut self, index: usize, value: String) {
        if let Some(slot) = self.user_input.get_mut(index) {
            *slot = value;
        }
    }

    pub fn confirm(&mut self, index: usize, value: String, answered_at: u64) {
        if let Some(slot) = self.user_input.get_mut(index) {
            *slot = value;
        }
        if let Some(slot) = self.confirmed.get_mut(index) {
            *slot = true;
        }
        if let Some(slot) = self.group_answer_at.get_mut(index) {
            if *slot == 0 {
                *slot = answered_at;
            }
        }
        let next = (index + 1).min(self.groups.len().saturating_sub(1));
        self.focused_group = next;
    }

    pub fn record_answer_time_if_empty(&mut self, index: usize, answered_at: u64) {
        if let Some(slot) = self.group_answer_at.get_mut(index) {
            if *slot == 0 {
                *slot = answered_at;
            }
        }
    }

    pub fn input_locked(&self, index: usize, settings: &TrainingSettings) -> bool {
        settings.lock_input_during_group_playback
            && self.status == RuntimeStatus::PlayingGroup
            && index == self.current_group
    }

    pub fn all_groups_answered(&self) -> bool {
        self.groups.iter().enumerate().all(|(i, sent)| {
            let received = self.user_input.get(i).map(String::as_str).unwrap_or("");
            !sent.is_empty() && received.len() == sent.len()
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
                let delta = answer_at.saturating_sub(end_at) as f64;
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
        .filter(|(sent, _)| !sent.is_empty())
        .map(|(sent, recv)| (sent.clone(), recv.clone()))
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
    let timings = session.build_timings(settings.group_timeout * 1000.0);
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
        koch_level: settings.koch_level,
        digits_level: settings.digits_level,
        char_set_mode: settings.char_set_mode,
        char_wpm: settings.char_wpm_min,
        effective_wpm: settings.effective_wpm_min,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_copy_is_full_accuracy() {
        let mut session = GroupSession::new(1, 0, 2);
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
    }
}
