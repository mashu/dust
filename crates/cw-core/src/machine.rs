//! Pure session state machine. The UI runtime applies [`SessionEffect`]s; this
//! module never sleeps, plays audio, or touches Dioxus.

use crate::session::{answer_length_matches, GroupSession, SessionId, SessionView};
use crate::settings::TrainingSettings;
use crate::timing::compute_group_gap_for_wpm;

pub const AUTO_CONFIRM_DELAY_MS: u32 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    Playing { index: usize },
    InterGroupGap { next: usize },
    AwaitingAnswer { index: usize },
    Finished,
    Aborted,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    PlaybackEnded {
        index: usize,
        duration_sec: f64,
        char_wpm: f64,
        effective_wpm: f64,
    },
    PlaybackCancelled {
        index: usize,
    },
    PlaybackFailed {
        index: usize,
    },
    Input {
        index: usize,
        text: String,
    },
    Confirm,
    Timeout,
    AutoConfirmDue {
        id: u64,
        index: usize,
        value: String,
    },
    GapElapsed,
    FinishNow,
    Abort,
    Focus {
        index: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEffect {
    Play {
        index: usize,
        text: String,
    },
    NeedGroup {
        index: usize,
    },
    StopAudio,
    Sleep {
        id: u64,
        ms: u32,
    },
    AutoConfirm {
        id: u64,
        index: usize,
        value: String,
        ms: u32,
    },
    Focus {
        index: usize,
    },
    PersistAndShowResults,
    AbortToHome,
}

#[derive(Clone, Debug)]
pub struct SessionMachine {
    session: GroupSession,
    phase: SessionPhase,
    next_effect_id: u64,
    pending_sleep: Option<u64>,
    pending_auto_confirm: Option<u64>,
}

impl SessionMachine {
    pub fn start(
        session_id: SessionId,
        started_at: u64,
        settings: TrainingSettings,
        first_group: String,
    ) -> (Self, Vec<SessionEffect>) {
        let n = settings.curriculum.num_groups.max(1) as usize;
        let mut session = GroupSession::new(session_id, started_at, n, settings);
        session.set_group(0, first_group);
        session.begin_group(0, started_at);
        let text = session
            .group(0)
            .map(|g| g.sent().to_string())
            .unwrap_or_default();
        let machine = Self {
            session,
            phase: SessionPhase::Playing { index: 0 },
            next_effect_id: 1,
            pending_sleep: None,
            pending_auto_confirm: None,
        };
        (
            machine,
            vec![
                SessionEffect::Focus { index: 0 },
                SessionEffect::Play { index: 0, text },
            ],
        )
    }

    pub fn session(&self) -> &GroupSession {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut GroupSession {
        &mut self.session
    }

    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    pub fn view(&self) -> SessionView {
        self.session.view()
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, SessionPhase::Finished | SessionPhase::Aborted)
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_effect_id;
        self.next_effect_id += 1;
        id
    }

    fn cancel_timers(&mut self) {
        self.pending_sleep = None;
        self.pending_auto_confirm = None;
    }

    pub fn apply(&mut self, event: SessionEvent, now_ms: u64) -> Vec<SessionEffect> {
        if self.is_terminal() {
            return Vec::new();
        }
        match event {
            SessionEvent::Abort => self.abort(),
            SessionEvent::PlaybackCancelled { index } => self.playback_cancelled(index),
            SessionEvent::PlaybackFailed { index } => self.playback_failed(index),
            SessionEvent::PlaybackEnded {
                index,
                duration_sec,
                char_wpm,
                effective_wpm,
            } => self.playback_ended(index, now_ms, duration_sec, char_wpm, effective_wpm),
            SessionEvent::Input { index, text } => self.input(index, text, now_ms),
            SessionEvent::Confirm => self.confirm_current(now_ms, None),
            SessionEvent::Timeout => self.timeout(now_ms),
            SessionEvent::AutoConfirmDue { id, index, value } => {
                self.auto_confirm_due(id, index, value, now_ms)
            }
            SessionEvent::GapElapsed => self.gap_elapsed(now_ms),
            SessionEvent::FinishNow => self.finish_now(now_ms),
            SessionEvent::Focus { index } => {
                self.session.set_focused_group(index);
                Vec::new()
            }
        }
    }

    fn abort(&mut self) -> Vec<SessionEffect> {
        self.cancel_timers();
        self.phase = SessionPhase::Aborted;
        vec![SessionEffect::StopAudio, SessionEffect::AbortToHome]
    }

    fn playing_index(&self) -> Option<usize> {
        match self.phase {
            SessionPhase::Playing { index } => Some(index),
            _ => None,
        }
    }

    fn playback_cancelled(&mut self, index: usize) -> Vec<SessionEffect> {
        if self.playing_index() != Some(index) {
            return Vec::new();
        }
        // Same as a hard audio failure: keep scored groups instead of dropping them.
        self.fail_audio()
    }

    fn playback_failed(&mut self, index: usize) -> Vec<SessionEffect> {
        if self.playing_index() != Some(index) {
            return Vec::new();
        }
        self.fail_audio()
    }

    fn fail_audio(&mut self) -> Vec<SessionEffect> {
        self.cancel_timers();
        if self.session.any_confirmed() {
            self.phase = SessionPhase::Finished;
            vec![
                SessionEffect::StopAudio,
                SessionEffect::PersistAndShowResults,
            ]
        } else {
            self.phase = SessionPhase::Aborted;
            vec![SessionEffect::StopAudio, SessionEffect::AbortToHome]
        }
    }

    fn playback_ended(
        &mut self,
        index: usize,
        now_ms: u64,
        duration_sec: f64,
        char_wpm: f64,
        effective_wpm: f64,
    ) -> Vec<SessionEffect> {
        if self.playing_index() != Some(index) {
            return Vec::new();
        }
        let ended = now_ms.max(
            self.session.group_start_ms(index).unwrap_or(now_ms)
                + (duration_sec * 1000.0).round() as u64,
        );
        self.session
            .end_playback(index, ended, char_wpm, effective_wpm);
        self.phase = SessionPhase::AwaitingAnswer { index };
        let mut effects = vec![SessionEffect::Focus { index }];
        let timeout_ms = (self.session.settings().playback.group_timeout * 1000.0).round() as u32;
        if timeout_ms > 0 {
            let id = self.alloc_id();
            self.pending_sleep = Some(id);
            effects.push(SessionEffect::Sleep { id, ms: timeout_ms });
        }
        effects
    }

    fn input(&mut self, index: usize, text: String, now_ms: u64) -> Vec<SessionEffect> {
        let current = match self.phase {
            SessionPhase::Playing { index } | SessionPhase::AwaitingAnswer { index } => index,
            _ => return Vec::new(),
        };
        if index != current || self.session.input_locked(index) {
            return Vec::new();
        }
        let sent = self
            .session
            .group(index)
            .map(|g| g.sent().to_string())
            .unwrap_or_default();
        self.session.set_input(index, text.clone());
        if answer_length_matches(&sent, &text) {
            self.session.record_answer_time_if_empty(index, now_ms);
            let id = self.alloc_id();
            self.pending_auto_confirm = Some(id);
            vec![SessionEffect::AutoConfirm {
                id,
                index,
                value: text,
                ms: AUTO_CONFIRM_DELAY_MS,
            }]
        } else {
            self.session.clear_answer_time(index);
            self.pending_auto_confirm = None;
            Vec::new()
        }
    }

    fn auto_confirm_due(
        &mut self,
        id: u64,
        index: usize,
        value: String,
        now_ms: u64,
    ) -> Vec<SessionEffect> {
        if self.pending_auto_confirm != Some(id) {
            return Vec::new();
        }
        self.pending_auto_confirm = None;
        if self.session.current_group() != index {
            return Vec::new();
        }
        if self.session.group(index).map(|g| g.input()) != Some(value.as_str()) {
            return Vec::new();
        }
        self.confirm_current(now_ms, Some(value))
    }

    fn timeout(&mut self, now_ms: u64) -> Vec<SessionEffect> {
        let SessionPhase::AwaitingAnswer { .. } = self.phase else {
            return Vec::new();
        };
        self.confirm_current(now_ms, None)
    }

    fn confirm_current(
        &mut self,
        now_ms: u64,
        override_value: Option<String>,
    ) -> Vec<SessionEffect> {
        let index = self.session.current_group();
        if self.session.input_locked(index) {
            return Vec::new();
        }
        let value = override_value
            .unwrap_or_else(|| {
                self.session
                    .group(index)
                    .map(|g| g.input().to_string())
                    .unwrap_or_default()
            })
            .trim()
            .to_ascii_uppercase();
        if !self.session.confirm(index, value, now_ms) {
            return Vec::new();
        }
        self.after_confirm()
    }

    fn finish_now(&mut self, now_ms: u64) -> Vec<SessionEffect> {
        let index = self.session.current_group();
        if !self.session.input_locked(index) {
            let typed = self
                .session
                .group(index)
                .is_some_and(|g| !g.input().trim().is_empty() && !g.confirmed());
            if typed {
                let _ = self.confirm_current(now_ms, None);
            }
        }
        self.cancel_timers();
        if self.session.any_confirmed() {
            self.phase = SessionPhase::Finished;
            vec![
                SessionEffect::StopAudio,
                SessionEffect::PersistAndShowResults,
            ]
        } else {
            self.phase = SessionPhase::Aborted;
            vec![SessionEffect::StopAudio, SessionEffect::AbortToHome]
        }
    }

    fn after_confirm(&mut self) -> Vec<SessionEffect> {
        self.cancel_timers();
        let next = self.session.current_group() + 1;
        if next >= self.session.group_count() || self.session.all_groups_confirmed() {
            self.phase = SessionPhase::Finished;
            return vec![
                SessionEffect::StopAudio,
                SessionEffect::Focus {
                    index: self.session.focused_group(),
                },
                SessionEffect::PersistAndShowResults,
            ];
        }
        self.phase = SessionPhase::InterGroupGap { next };
        let gap_ms = self.group_gap_ms(next);
        let id = self.alloc_id();
        self.pending_sleep = Some(id);
        vec![
            SessionEffect::StopAudio,
            SessionEffect::NeedGroup { index: next },
            SessionEffect::Focus {
                index: self.session.focused_group(),
            },
            SessionEffect::Sleep { id, ms: gap_ms },
        ]
    }

    fn group_gap_ms(&self, next: usize) -> u32 {
        let prev = next.saturating_sub(1);
        match self.session.played_wpm(prev) {
            Some((char_wpm, effective_wpm)) => compute_group_gap_for_wpm(
                char_wpm,
                effective_wpm,
                self.session.settings().playback.extra_word_space_multiplier,
            ),
            None => crate::timing::compute_group_gap_ms(self.session.settings()),
        }
    }

    fn gap_elapsed(&mut self, now_ms: u64) -> Vec<SessionEffect> {
        let SessionPhase::InterGroupGap { next } = self.phase else {
            return Vec::new();
        };
        self.pending_sleep = None;
        self.begin_next(next, now_ms)
    }

    fn begin_next(&mut self, index: usize, now_ms: u64) -> Vec<SessionEffect> {
        self.session.begin_group(index, now_ms);
        self.phase = SessionPhase::Playing { index };
        let text = self
            .session
            .group(index)
            .map(|g| g.sent().to_string())
            .unwrap_or_default();
        vec![
            SessionEffect::Focus { index },
            SessionEffect::Play { index, text },
        ]
    }

    pub fn set_group_text(&mut self, index: usize, text: String) {
        self.session.set_group(index, text);
    }

    pub fn sleep_is_current(&self, id: u64) -> bool {
        self.pending_sleep == Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::TrainingSettings;

    fn machine() -> SessionMachine {
        let mut settings = TrainingSettings::default();
        settings.curriculum.num_groups = 2;
        settings.playback.group_timeout = 10.0;
        settings.playback.lock_input_during_group_playback = true;
        let (m, _) = SessionMachine::start(SessionId::new(1), 0, settings, "KM".into());
        m
    }

    #[test]
    fn abort_during_play_goes_home() {
        let mut m = machine();
        let effects = m.apply(SessionEvent::Abort, 10);
        assert_eq!(m.phase(), SessionPhase::Aborted);
        assert!(effects.contains(&SessionEffect::AbortToHome));
        assert!(effects.contains(&SessionEffect::StopAudio));
    }

    #[test]
    fn audio_fail_with_no_answers_aborts() {
        let mut m = machine();
        let effects = m.apply(SessionEvent::PlaybackFailed { index: 0 }, 10);
        assert_eq!(m.phase(), SessionPhase::Aborted);
        assert!(effects.contains(&SessionEffect::AbortToHome));
    }

    #[test]
    fn playback_then_timeout_confirms_empty() {
        let mut m = machine();
        let effects = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.5,
                char_wpm: 20.0,
                effective_wpm: 18.0,
            },
            500,
        );
        assert!(matches!(
            m.phase(),
            SessionPhase::AwaitingAnswer { index: 0 }
        ));
        assert!(effects
            .iter()
            .any(|e| matches!(e, SessionEffect::Sleep { .. })));
        let after = m.apply(SessionEvent::Timeout, 10_500);
        assert!(m.session().group(0).unwrap().confirmed());
        assert!(
            after
                .iter()
                .any(|e| matches!(e, SessionEffect::Sleep { .. }))
                || after.contains(&SessionEffect::PersistAndShowResults)
        );
    }

    #[test]
    fn auto_confirm_advances_after_length_match() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.num_groups = 1;
        settings.playback.group_timeout = 0.0;
        settings.playback.lock_input_during_group_playback = false;
        let (mut m, _) = SessionMachine::start(SessionId::new(1), 0, settings, "KM".into());
        let _ = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.2,
                char_wpm: 18.0,
                effective_wpm: 18.0,
            },
            200,
        );
        let effects = m.apply(
            SessionEvent::Input {
                index: 0,
                text: "KM".into(),
            },
            300,
        );
        let Some(SessionEffect::AutoConfirm { id, value, .. }) = effects
            .into_iter()
            .find(|e| matches!(e, SessionEffect::AutoConfirm { .. }))
        else {
            panic!("expected auto-confirm");
        };
        let done = m.apply(
            SessionEvent::AutoConfirmDue {
                id,
                index: 0,
                value,
            },
            600,
        );
        assert_eq!(m.phase(), SessionPhase::Finished);
        assert!(done.contains(&SessionEffect::PersistAndShowResults));
    }

    #[test]
    fn stale_auto_confirm_is_ignored() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.num_groups = 1;
        settings.playback.lock_input_during_group_playback = false;
        let (mut m, _) = SessionMachine::start(SessionId::new(1), 0, settings, "KM".into());
        let _ = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.2,
                char_wpm: 18.0,
                effective_wpm: 18.0,
            },
            200,
        );
        let first = m.apply(
            SessionEvent::Input {
                index: 0,
                text: "KM".into(),
            },
            300,
        );
        let id = match first
            .into_iter()
            .find(|e| matches!(e, SessionEffect::AutoConfirm { .. }))
        {
            Some(SessionEffect::AutoConfirm { id, .. }) => id,
            _ => panic!("auto-confirm"),
        };
        let _ = m.apply(
            SessionEvent::Input {
                index: 0,
                text: "K".into(),
            },
            320,
        );
        let effects = m.apply(
            SessionEvent::AutoConfirmDue {
                id,
                index: 0,
                value: "KM".into(),
            },
            600,
        );
        assert!(effects.is_empty());
        assert!(!m.session().group(0).unwrap().confirmed());
    }

    #[test]
    fn finish_now_mid_session_persists_if_any_confirmed() {
        let mut m = machine();
        let _ = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.5,
                char_wpm: 20.0,
                effective_wpm: 18.0,
            },
            500,
        );
        let _ = m.apply(
            SessionEvent::Input {
                index: 0,
                text: "KM".into(),
            },
            600,
        );
        let effects = m.apply(SessionEvent::FinishNow, 700);
        assert_eq!(m.phase(), SessionPhase::Finished);
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
        assert!(m.session().any_confirmed());
    }

    #[test]
    fn confirm_emits_need_group_before_next_play() {
        let mut m = machine();
        let _ = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.2,
                char_wpm: 18.0,
                effective_wpm: 18.0,
            },
            200,
        );
        let effects = m.apply(SessionEvent::Confirm, 300);
        let need = effects
            .iter()
            .position(|e| matches!(e, SessionEffect::NeedGroup { index: 1 }));
        let play = effects
            .iter()
            .position(|e| matches!(e, SessionEffect::Play { .. }));
        assert!(need.is_some());
        assert!(play.is_none(), "play must wait until NeedGroup is applied");
        assert!(effects.contains(&SessionEffect::StopAudio));
        assert!(effects
            .iter()
            .any(|e| matches!(e, SessionEffect::Sleep { .. })));
    }

    #[test]
    fn cancelled_play_for_another_group_does_not_abort() {
        let mut m = machine();
        let _ = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.2,
                char_wpm: 18.0,
                effective_wpm: 18.0,
            },
            200,
        );
        let _ = m.apply(SessionEvent::Confirm, 300);
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
        let effects = m.apply(SessionEvent::PlaybackCancelled { index: 0 }, 310);
        assert!(effects.is_empty());
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
    }

    #[test]
    fn cancelled_play_for_current_group_aborts() {
        let mut m = machine();
        let effects = m.apply(SessionEvent::PlaybackCancelled { index: 0 }, 10);
        assert_eq!(m.phase(), SessionPhase::Aborted);
        assert!(effects.contains(&SessionEffect::AbortToHome));
    }

    #[test]
    fn cancelled_play_after_confirmed_group_persists() {
        let mut m = machine();
        let _ = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.2,
                char_wpm: 18.0,
                effective_wpm: 18.0,
            },
            200,
        );
        let _ = m.apply(SessionEvent::Confirm, 300);
        m.set_group_text(1, "UK".into());
        let _ = m.apply(SessionEvent::GapElapsed, 800);
        assert!(matches!(m.phase(), SessionPhase::Playing { index: 1 }));
        let effects = m.apply(SessionEvent::PlaybackCancelled { index: 1 }, 900);
        assert_eq!(m.phase(), SessionPhase::Finished);
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
        assert!(m.session().group(0).unwrap().confirmed());
    }

    #[test]
    fn playback_ended_for_stale_index_is_ignored() {
        let mut m = machine();
        let _ = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.2,
                char_wpm: 18.0,
                effective_wpm: 18.0,
            },
            200,
        );
        let effects = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.2,
                char_wpm: 18.0,
                effective_wpm: 18.0,
            },
            400,
        );
        assert!(effects.is_empty());
        assert!(matches!(
            m.phase(),
            SessionPhase::AwaitingAnswer { index: 0 }
        ));
    }

    #[test]
    fn confirm_during_play_stops_audio() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.num_groups = 2;
        settings.playback.lock_input_during_group_playback = false;
        let (mut m, _) = SessionMachine::start(SessionId::new(1), 0, settings, "KM".into());
        let effects = m.apply(SessionEvent::Confirm, 50);
        assert!(effects.contains(&SessionEffect::StopAudio));
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
        let late = m.apply(SessionEvent::PlaybackCancelled { index: 0 }, 80);
        assert!(late.is_empty());
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
        assert_eq!(
            m.session().view().status,
            crate::RuntimeStatus::WaitingForAnswer
        );
        let timings = m.session().build_timings(10_000.0);
        assert!((timings[0].time_to_complete_ms - 1.0).abs() < 1e-9);
    }

    #[test]
    fn start_emits_play_even_for_empty_text() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.num_groups = 1;
        let (_, effects) = SessionMachine::start(SessionId::new(1), 0, settings, String::new());
        assert!(effects.iter().any(|e| matches!(
            e,
            SessionEffect::Play {
                index: 0,
                text
            } if text.is_empty()
        )));
    }
}
