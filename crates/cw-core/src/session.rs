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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl From<u64> for SessionId {
    fn from(raw: u64) -> Self {
        Self(raw)
    }
}

impl SessionId {
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeStatus {
    Starting,
    PlayingGroup,
    WaitingForAnswer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    sent: String,
    input: String,
    confirmed: bool,
    start_at: u64,
    end_at: u64,
    answer_at: u64,
    char_wpm: f64,
    effective_wpm: f64,
}

impl Group {
    fn blank() -> Self {
        Self {
            sent: String::new(),
            input: String::new(),
            confirmed: false,
            start_at: 0,
            end_at: 0,
            answer_at: 0,
            char_wpm: 0.0,
            effective_wpm: 0.0,
        }
    }

    pub fn sent(&self) -> &str {
        &self.sent
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn confirmed(&self) -> bool {
        self.confirmed
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionView {
    pub session_id: SessionId,
    pub status: RuntimeStatus,
    pub current: usize,
    pub focused: usize,
    pub sent: Vec<String>,
    pub inputs: Vec<String>,
    pub confirmed: Vec<bool>,
    pub locked: bool,
}

#[derive(Clone, Debug)]
pub struct GroupSession {
    status: RuntimeStatus,
    session_id: SessionId,
    started_at: u64,
    groups: Vec<Group>,
    current_group: usize,
    focused_group: usize,
    settings: TrainingSettings,
}

impl GroupSession {
    pub fn new(
        session_id: impl Into<SessionId>,
        started_at: u64,
        num_groups: usize,
        settings: TrainingSettings,
    ) -> Self {
        Self {
            status: RuntimeStatus::Starting,
            session_id: session_id.into(),
            started_at,
            groups: vec![Group::blank(); num_groups.max(1)],
            current_group: 0,
            focused_group: 0,
            settings,
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn started_at(&self) -> u64 {
        self.started_at
    }

    pub fn status(&self) -> RuntimeStatus {
        self.status
    }

    pub fn current_group(&self) -> usize {
        self.current_group
    }

    pub fn focused_group(&self) -> usize {
        self.focused_group
    }

    pub fn set_focused_group(&mut self, index: usize) {
        if index == self.current_group {
            self.focused_group = index;
        }
    }

    pub fn settings(&self) -> &TrainingSettings {
        &self.settings
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn group(&self, index: usize) -> Option<&Group> {
        self.groups.get(index)
    }

    pub fn sent_texts(&self) -> Vec<String> {
        self.groups.iter().map(|g| g.sent.clone()).collect()
    }

    pub fn inputs(&self) -> Vec<String> {
        self.groups.iter().map(|g| g.input.clone()).collect()
    }

    pub fn confirmed_flags(&self) -> Vec<bool> {
        self.groups.iter().map(|g| g.confirmed).collect()
    }

    pub fn any_confirmed(&self) -> bool {
        self.groups.iter().any(|g| g.confirmed)
    }

    pub fn view(&self) -> SessionView {
        SessionView {
            session_id: self.session_id,
            status: self.status,
            current: self.current_group,
            focused: self.focused_group,
            sent: self.sent_texts(),
            inputs: self.inputs(),
            confirmed: self.confirmed_flags(),
            locked: self.input_locked(self.current_group),
        }
    }

    pub fn played_wpm(&self, index: usize) -> Option<(f64, f64)> {
        let group = self.groups.get(index)?;
        (group.char_wpm > 0.0 && group.effective_wpm > 0.0)
            .then_some((group.char_wpm, group.effective_wpm))
    }

    pub fn group_start_ms(&self, index: usize) -> Option<u64> {
        self.groups.get(index).map(|g| g.start_at)
    }

    pub fn set_group(&mut self, index: usize, text: String) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        group.sent = text;
        if !group.confirmed {
            group.input.clear();
        }
    }

    pub fn begin_group(&mut self, index: usize, now_ms: u64) {
        if index >= self.groups.len() {
            return;
        }
        self.current_group = index;
        self.focused_group = index;
        self.status = RuntimeStatus::PlayingGroup;
        if let Some(group) = self.groups.get_mut(index) {
            group.start_at = now_ms;
        }
    }

    pub fn end_playback(&mut self, index: usize, now_ms: u64, char_wpm: f64, effective_wpm: f64) {
        if let Some(group) = self.groups.get_mut(index) {
            group.end_at = now_ms;
            group.char_wpm = char_wpm;
            group.effective_wpm = effective_wpm;
        }
        self.status = RuntimeStatus::WaitingForAnswer;
    }

    pub fn set_input(&mut self, index: usize, value: String) {
        if index != self.current_group {
            return;
        }
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        if group.confirmed {
            return;
        }
        group.input = value;
    }

    /// Returns true the first time this group is confirmed.
    pub fn confirm(&mut self, index: usize, value: String, answered_at: u64) -> bool {
        if index != self.current_group {
            return false;
        }
        let Some(group) = self.groups.get_mut(index) else {
            return false;
        };
        if group.confirmed {
            return false;
        }
        if group.end_at == 0 {
            group.end_at = answered_at;
        }
        group.confirmed = true;
        group.input = value;
        if group.answer_at == 0 {
            group.answer_at = answered_at;
        }
        if self.status == RuntimeStatus::PlayingGroup {
            self.status = RuntimeStatus::WaitingForAnswer;
        }
        let next = (index + 1).min(self.groups.len().saturating_sub(1));
        self.focused_group = next;
        true
    }

    pub fn record_answer_time_if_empty(&mut self, index: usize, answered_at: u64) {
        if let Some(group) = self.groups.get_mut(index) {
            if group.answer_at == 0 {
                group.answer_at = answered_at;
            }
        }
    }

    /// Drop a length-match stamp if the user backspaces before confirm.
    pub fn clear_answer_time(&mut self, index: usize) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        if group.confirmed {
            return;
        }
        group.answer_at = 0;
    }

    pub fn input_locked(&self, index: usize) -> bool {
        self.settings.playback.lock_input_during_group_playback
            && self.status == RuntimeStatus::PlayingGroup
            && index == self.current_group
    }

    pub fn all_groups_confirmed(&self) -> bool {
        !self.groups.is_empty()
            && self
                .groups
                .iter()
                .all(|group| !group.sent.is_empty() && group.confirmed)
    }

    pub fn build_timings(&self, fallback_timeout_ms: f64) -> Vec<SessionTiming> {
        self.groups
            .iter()
            .map(|group| {
                let end_at = group.end_at;
                let raw_answer = group.answer_at;
                let fallback = if end_at > 0 && fallback_timeout_ms > 0.0 {
                    end_at + fallback_timeout_ms as u64
                } else {
                    0
                };
                let answer_at = if raw_answer > 0 { raw_answer } else { fallback };
                let delta = if answer_at > 0 && end_at > 0 && answer_at <= end_at {
                    1.0
                } else {
                    answer_at.saturating_sub(end_at) as f64
                };
                let per_char = if group.sent.is_empty() {
                    0.0
                } else {
                    (delta / group.sent.chars().count() as f64).round()
                };
                let wpm = (group.char_wpm > 0.0).then_some(group.char_wpm);
                SessionTiming {
                    time_to_complete_ms: delta,
                    per_char_ms: per_char,
                    char_wpm: wpm,
                }
            })
            .collect()
    }

    #[cfg(test)]
    fn force_timing(&mut self, index: usize, end_at: u64, char_wpm: f64, effective_wpm: f64) {
        if let Some(group) = self.groups.get_mut(index) {
            group.end_at = end_at;
            group.char_wpm = char_wpm;
            group.effective_wpm = effective_wpm;
        }
    }

    #[cfg(test)]
    fn set_lock_for_test(&mut self, lock: bool) {
        self.settings.playback.lock_input_during_group_playback = lock;
    }
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

fn group_was_scored(session: &GroupSession, index: usize) -> bool {
    session
        .groups
        .get(index)
        .is_some_and(|group| group.confirmed && !group.sent.is_empty())
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
    let pairs: Vec<(String, String)> = session
        .groups
        .iter()
        .enumerate()
        .filter(|(index, _)| group_was_scored(session, *index))
        .map(|(_, group)| (group.sent.clone(), group.input.trim().to_ascii_uppercase()))
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
    let all_timings = session.build_timings(settings.playback.group_timeout * 1000.0);
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
        .groups
        .iter()
        .enumerate()
        .filter(|(index, group)| group_was_scored(session, *index) && group.char_wpm > 0.0)
        .map(|(_, group)| group.char_wpm)
        .collect();
    let char_wpm = if played_wpm.is_empty() {
        settings.playback.char_wpm_min
    } else {
        played_wpm.iter().sum::<f64>() / played_wpm.len() as f64
    };
    let played_effective: Vec<f64> = session
        .groups
        .iter()
        .enumerate()
        .filter(|(index, group)| group_was_scored(session, *index) && group.effective_wpm > 0.0)
        .map(|(_, group)| group.effective_wpm)
        .collect();
    let effective_wpm = if played_effective.is_empty() {
        settings.playback.effective_wpm_min.min(char_wpm)
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
        level: match settings.curriculum.char_set_mode {
            CharSetMode::Digits => settings.curriculum.digits_level,
            _ => settings.curriculum.level,
        },
        digits_level: settings.curriculum.digits_level,
        char_set_mode: settings.curriculum.char_set_mode,
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
        if self.char_set_mode != settings.curriculum.char_set_mode {
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
        session.begin_group(0, 0);
        session.confirm(0, "KM".into(), 1000);
        session.force_timing(0, 500, 18.0, 18.0);
        session.begin_group(1, 1000);
        session.confirm(1, "UK".into(), 2000);
        session.force_timing(1, 1500, 18.0, 18.0);
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
        session.begin_group(0, 0);
        session.confirm(0, "KM".into(), 1000);
        session.force_timing(0, 500, 22.0, 22.0);
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
        assert_eq!(session.group(0).unwrap().input(), "KM");
        session.set_input(0, "ZZ".into());
        assert_eq!(session.group(0).unwrap().input(), "KM");
    }

    #[test]
    fn set_input_ignores_other_groups() {
        let mut session = GroupSession::new(1, 0, 2, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.set_group(1, "UK".into());
        session.set_input(1, "UK".into());
        assert_eq!(session.group(1).unwrap().input(), "");
        session.set_input(0, "KM".into());
        assert_eq!(session.group(0).unwrap().input(), "KM");
    }

    #[test]
    fn all_groups_confirmed_requires_every_sent_group() {
        let mut session = GroupSession::new(1, 0, 2, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.set_group(1, "UK".into());
        session.confirm(0, "KM".into(), 1);
        assert!(!session.all_groups_confirmed());
        session.begin_group(1, 1);
        session.confirm(1, "".into(), 2);
        assert!(session.all_groups_confirmed());
    }

    #[test]
    fn timeout_empty_answer_is_scored_wrong() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.confirm(0, "".into(), 1000);
        session.force_timing(0, 500, 0.0, 0.0);
        let result = build_session_result(
            &session,
            &TrainingSettings::default(),
            3000,
            "2026-09-01".into(),
        );
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
        assert_eq!(session.group(0).unwrap().input(), "");
        session.set_input(0, "KM".into());
        session.confirm(0, "KM".into(), 1);
        session.set_group(0, "UK".into());
        assert_eq!(session.group(0).unwrap().input(), "KM");
    }

    #[test]
    fn answer_during_playback_counts_as_immediate() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.force_timing(0, 2000, 18.0, 18.0);
        session.confirm(0, "KM".into(), 1500);
        let timings = session.build_timings(10_000.0);
        assert_eq!(timings.len(), 1);
        assert!((timings[0].time_to_complete_ms - 1.0).abs() < 1e-9);
        assert!((timings[0].per_char_ms - 1.0).abs() < 1e-9);
    }

    #[test]
    fn confirm_before_end_playback_is_immediate() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.begin_group(0, 0);
        session.confirm(0, "KM".into(), 400);
        assert_eq!(session.view().status, RuntimeStatus::WaitingForAnswer);
        let timings = session.build_timings(10_000.0);
        assert_eq!(timings.len(), 1);
        assert!((timings[0].time_to_complete_ms - 1.0).abs() < 1e-9);
    }

    #[test]
    fn result_uses_played_effective_wpm() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.confirm(0, "KM".into(), 1000);
        session.force_timing(0, 500, 25.0, 12.0);
        let mut settings = TrainingSettings::default();
        settings.playback.effective_wpm_min = 18.0;
        settings.playback.effective_wpm_max = 25.0;
        let result = build_session_result(&session, &settings, 3000, "2026-09-01".into());
        assert!((result.char_wpm - 25.0).abs() < 1e-9);
        assert!((result.effective_wpm - 12.0).abs() < 1e-9);
        assert_eq!(result.alphabet_fingerprint, settings.alphabet_fingerprint());
    }

    #[test]
    fn sampling_history_requires_matching_mode_and_alphabet() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
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
        other_seq.curriculum.custom_sequence = crate::sequences::TRADITIONAL_KOCH_SEQUENCE.to_vec();
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
        settings.playback.lock_input_during_group_playback = true;
        let mut session = GroupSession::new(1, 0, 1, settings);
        session.begin_group(0, 0);
        assert!(session.input_locked(0));
        session.set_lock_for_test(false);
        assert!(!session.input_locked(0));
        session.end_playback(0, 1, 20.0, 18.0);
        session.set_lock_for_test(true);
        assert!(!session.input_locked(0));
    }

    #[test]
    fn digits_session_records_digits_level() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Digits;
        settings.curriculum.digits_level = 4;
        settings.curriculum.level = 1;
        let mut session = GroupSession::new(1, 0, 1, settings.clone());
        session.set_group(0, "01".into());
        session.confirm(0, "01".into(), 1000);
        session.force_timing(0, 500, 0.0, 0.0);
        let result = build_session_result(&session, &settings, 3000, "2026-09-01".into());
        assert_eq!(result.level, 4);
        assert_eq!(result.digits_level, 4);
        assert_eq!(result.char_set_mode, CharSetMode::Digits);
    }

    #[test]
    fn shortening_answer_clears_completion_stamp() {
        let mut session = GroupSession::new(1, 0, 1, TrainingSettings::default());
        session.set_group(0, "KM".into());
        session.force_timing(0, 1000, 0.0, 0.0);
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
