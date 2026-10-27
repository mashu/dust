//! Pure session state machine. The UI runtime applies [`SessionEffect`]s; this
//! module never sleeps, plays audio, or touches Dioxus.
//!
//! The machine owns everything a session decides: which group comes next
//! (through its [`GroupSource`]), what a timer running out means, and what
//! the sampler learns from each answer. The runtime only plays, waits, and
//! reports back.

use crate::session::{
    Answered, GroupSession, MonoMs, SendStart, SessionId, SessionPhase, answer_length_matches,
};
use crate::settings::TrainingSettings;
use crate::timing::{compute_after_group_gap_ms, compute_group_gap_for_wpm};

pub const AUTO_CONFIRM_DELAY_MS: u32 = 300;

/// One step of the count down before the first group.
pub const COUNTDOWN_STEP_MS: u32 = 1_000;

/// Where a session's groups come from, and where what was copied of them
/// goes back to.
pub trait GroupSource {
    /// The next group to send, never empty, and how many times to send it
    /// before the answer window opens.
    fn next_group(&mut self, settings: &TrainingSettings) -> (String, u32);
    /// A group the user has just answered.
    fn learn(&mut self, answered: Answered<'_>);
    /// The character and effective speed to send the group just drawn at,
    /// if the source chooses it. Otherwise the station sending it does.
    fn next_speed(&mut self, _settings: &TrainingSettings) -> Option<(f64, f64)> {
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    /// A send of the group has started, on each attempt, retries too: its
    /// speeds, and when its characters go to the output.
    PlaybackStarted {
        index: usize,
        start: SendStart,
    },
    /// A send played to its end, `on_time` unless the audio held it up on
    /// its way: a suspended clock, a stall.
    PlaybackEnded {
        index: usize,
        duration_sec: f64,
        on_time: bool,
    },
    /// The send stopped before it finished — cancelled from outside, or the
    /// audio failed. Either way the session ends with what it has scored.
    PlaybackStopped {
        index: usize,
    },
    /// What the box for `index` holds, and when each of its characters
    /// went in ([`crate::KeyLog`]): none where it cannot be timed.
    Input {
        index: usize,
        text: String,
        keys: Vec<Option<MonoMs>>,
    },
    Confirm,
    /// A timer the machine started has run out. What it was for is the
    /// machine's to remember; a timer it has since forgotten is ignored.
    TimerFired {
        id: u64,
    },
    FinishNow,
    Abort,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEffect {
    Play {
        index: usize,
        text: String,
    },
    StopAudio,
    /// Send [`SessionEvent::TimerFired`] with this id once `ms` have passed.
    StartTimer {
        id: u64,
        ms: u32,
    },
    PersistAndShowResults,
    AbortToHome,
}

/// How an event closes the answer to the group being copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerClose {
    /// Whatever is entered by now is the answer: Confirm, End session, the
    /// answer window running out.
    Deadline,
    /// Auto-confirm: the answer reached full length a moment ago and has sat
    /// still since. Anything still being entered means it has not.
    Settled,
    /// The answer window running out on an answer nobody has begun. It
    /// moves on from a group that was not copied at all, and never from one
    /// being answered: a pause to think is not a character skipped. Anything
    /// still being entered means it has begun.
    Unbegun,
}

/// What the phase's timer is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhaseTimer {
    /// The word space between two sends, or the pause before the next group.
    Gap,
    /// The answer window closing.
    AnswerWindow,
    /// One second of the count down before the first group.
    Countdown,
}

/// An answer that reached its group's length, waiting a moment in case the
/// user is still typing.
#[derive(Clone, Debug, PartialEq)]
struct PendingConfirm {
    id: u64,
    index: usize,
    value: String,
}

#[derive(Clone, Debug)]
pub struct SessionMachine<S> {
    session: GroupSession,
    source: S,
    next_timer_id: u64,
    phase_timer: Option<(u64, PhaseTimer)>,
    auto_confirm: Option<PendingConfirm>,
    /// Sends still owed for the group being played, including the one in flight.
    repeats_left: u32,
    /// Audio time already spent on this group: every send plus the gaps between them.
    group_elapsed_ms: u64,
    /// Something has been typed into the group being answered, so the answer
    /// window no longer closes it: it waits for the answer to be finished,
    /// or for Enter.
    answer_begun: bool,
}

impl<S: GroupSource> SessionMachine<S> {
    /// Start a session: draw its first group, and send it once the count
    /// down the settings ask for has run — at once, if they ask for none.
    pub fn start(
        session_id: SessionId,
        now: MonoMs,
        settings: TrainingSettings,
        mut source: S,
    ) -> (Self, Vec<SessionEffect>) {
        let groups = settings.curriculum.num_groups.max(1) as usize;
        let countdown = settings.playback.countdown_sec;
        let (text, repeats) = source.next_group(&settings);
        let speed = source.next_speed(&settings);
        let mut session = GroupSession::new(session_id, groups, settings);
        session.set_group(0, text, repeats);
        session.set_group_speed(0, speed);
        let mut machine = Self {
            repeats_left: session.group_repeats(0),
            session,
            source,
            next_timer_id: 1,
            phase_timer: None,
            auto_confirm: None,
            group_elapsed_ms: 0,
            answer_begun: false,
        };
        let effects = if countdown == 0 {
            machine.begin_group(0, now)
        } else {
            machine.set_phase(SessionPhase::Countdown {
                remaining: countdown,
            });
            vec![machine.start_phase_timer(PhaseTimer::Countdown, COUNTDOWN_STEP_MS)]
        };
        (machine, effects)
    }

    pub fn session(&self) -> &GroupSession {
        &self.session
    }

    pub fn phase(&self) -> SessionPhase {
        self.session.phase()
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.phase(), SessionPhase::Finished | SessionPhase::Aborted)
    }

    fn set_phase(&mut self, phase: SessionPhase) {
        self.session.set_phase(phase);
    }

    fn start_timer(&mut self, ms: u32) -> (u64, SessionEffect) {
        let id = self.next_timer_id;
        self.next_timer_id += 1;
        (id, SessionEffect::StartTimer { id, ms })
    }

    fn start_phase_timer(&mut self, timer: PhaseTimer, ms: u32) -> SessionEffect {
        let (id, effect) = self.start_timer(ms);
        self.phase_timer = Some((id, timer));
        effect
    }

    fn cancel_timers(&mut self) {
        self.phase_timer = None;
        self.auto_confirm = None;
    }

    /// Whether, and how, `event` would close the answer to the group being
    /// copied.
    ///
    /// The runtime asks before it applies the event. At a deadline, what the
    /// user has finished entering by then is part of the answer. Auto-confirm
    /// is no deadline: it waits for the answer to sit still, so an entry
    /// still under way puts it off. The answer window closes only an answer
    /// nobody has begun, and an entry under way is one begun. Anything else
    /// — a send ending, a gap — leaves the answer open for more.
    ///
    /// Typing never closes the answer, and must not be made to. Closing it
    /// has the runtime ask the answer box to finish what is entered, and the
    /// box can hand the session what it finishes as an
    /// [`SessionEvent::Input`] from inside that very dispatch. An input that
    /// closed the answer would ask the box to finish again while it is still
    /// finishing, and a UI callback called from inside itself panics: the
    /// app would go down mid-session.
    pub fn closes_answer(&self, event: &SessionEvent) -> Option<AnswerClose> {
        if self.is_terminal() {
            return None;
        }
        match event {
            SessionEvent::Confirm | SessionEvent::FinishNow => Some(AnswerClose::Deadline),
            SessionEvent::TimerFired { id } => {
                if self
                    .auto_confirm
                    .as_ref()
                    .is_some_and(|pending| pending.id == *id)
                {
                    return Some(AnswerClose::Settled);
                }
                let window = matches!(
                    (self.phase_timer, self.phase()),
                    (Some((timer, PhaseTimer::AnswerWindow)), SessionPhase::AwaitingAnswer { .. })
                        if timer == *id
                );
                (window && !self.answer_begun).then_some(AnswerClose::Unbegun)
            }
            SessionEvent::PlaybackStarted { .. }
            | SessionEvent::PlaybackEnded { .. }
            | SessionEvent::PlaybackStopped { .. }
            | SessionEvent::Input { .. }
            | SessionEvent::Abort => None,
        }
    }

    #[must_use = "the effects are the session's instructions to the runtime"]
    pub fn apply(&mut self, event: SessionEvent, now: MonoMs) -> Vec<SessionEffect> {
        if self.is_terminal() {
            return Vec::new();
        }
        match event {
            SessionEvent::Abort => self.end(false),
            SessionEvent::PlaybackStopped { index } => {
                if self.playing_index() != Some(index) {
                    return Vec::new();
                }
                // Keep whatever was scored rather than dropping it.
                self.end(self.session.any_confirmed())
            }
            SessionEvent::PlaybackStarted { index, start } => {
                if self.playing_index() == Some(index) {
                    self.session.start_playback(index, &start);
                }
                Vec::new()
            }
            SessionEvent::PlaybackEnded {
                index,
                duration_sec,
                on_time,
            } => self.playback_ended(index, now, duration_sec, on_time),
            SessionEvent::Input { index, text, keys } => self.input(index, &text, &keys, now),
            SessionEvent::Confirm => self.confirm_current(now),
            SessionEvent::TimerFired { id } => self.timer_fired(id, now),
            SessionEvent::FinishNow => self.finish_now(now),
        }
    }

    /// Stop, and either keep the session (`keep`) or throw it away.
    fn end(&mut self, keep: bool) -> Vec<SessionEffect> {
        self.cancel_timers();
        if keep {
            self.set_phase(SessionPhase::Finished);
            vec![
                SessionEffect::StopAudio,
                SessionEffect::PersistAndShowResults,
            ]
        } else {
            self.set_phase(SessionPhase::Aborted);
            vec![SessionEffect::StopAudio, SessionEffect::AbortToHome]
        }
    }

    fn playing_index(&self) -> Option<usize> {
        match self.phase() {
            SessionPhase::Playing { index } => Some(index),
            _ => None,
        }
    }

    /// Send `index` ended at `now`, having lasted `duration_sec`, on time or
    /// not.
    fn playback_ended(
        &mut self,
        index: usize,
        now: MonoMs,
        duration_sec: f64,
        on_time: bool,
    ) -> Vec<SessionEffect> {
        if self.playing_index() != Some(index) {
            return Vec::new();
        }
        self.session.note_play_finished(index, on_time);
        let played_ms = if duration_sec.is_finite() {
            (duration_sec.max(0.0) * 1000.0).round() as u64
        } else {
            0
        };
        self.group_elapsed_ms = self.group_elapsed_ms.saturating_add(played_ms);
        if self.repeats_left > 1 {
            self.repeats_left -= 1;
            let playback = &self.session.settings().playback;
            let (char_wpm, effective_wpm) = self
                .session
                .played_wpm(index)
                .unwrap_or((playback.char_wpm_min, playback.effective_wpm_min));
            let gap = compute_group_gap_for_wpm(
                char_wpm,
                effective_wpm,
                playback.extra_word_space_multiplier,
            );
            self.group_elapsed_ms = self.group_elapsed_ms.saturating_add(u64::from(gap));
            self.set_phase(SessionPhase::RepeatGap { index });
            return vec![self.start_phase_timer(PhaseTimer::Gap, gap)];
        }
        // The send cannot have ended before its audio did, whatever the
        // runtime's clock made of a send that stalled and caught up.
        let earliest = self
            .session
            .group_start(index)
            .map_or(now, |start| start.plus(self.group_elapsed_ms));
        self.session.end_playback(index, now.max(earliest));
        self.set_phase(SessionPhase::AwaitingAnswer { index });
        let timeout_ms = (self.session.settings().playback.group_timeout * 1000.0).round() as u32;
        if timeout_ms == 0 {
            return Vec::new();
        }
        vec![self.start_phase_timer(PhaseTimer::AnswerWindow, timeout_ms)]
    }

    fn input(
        &mut self,
        index: usize,
        text: &str,
        keys: &[Option<MonoMs>],
        now: MonoMs,
    ) -> Vec<SessionEffect> {
        let current = match self.phase() {
            SessionPhase::Playing { index }
            | SessionPhase::RepeatGap { index }
            | SessionPhase::AwaitingAnswer { index } => index,
            _ => return Vec::new(),
        };
        if index != current || self.session.input_locked(index) {
            return Vec::new();
        }
        let Some(group) = self.session.group(index) else {
            return Vec::new();
        };
        let previous = group.input().to_string();
        self.session.set_input(index, text, keys);
        let Some(group) = self.session.group(index) else {
            return Vec::new();
        };
        let (sent, value) = (group.sent(), group.input().to_string());
        self.answer_begun |= !value.is_empty();
        let complete = answer_length_matches(sent, &value);
        self.session.mark_answer_complete(index, complete, now);
        if !complete {
            self.auto_confirm = None;
            return Vec::new();
        }
        // The renderer can re-fire Input with the same text on every paint.
        // Restarting the timer then would mean the group never confirms and
        // the next send never starts.
        if self.auto_confirm.is_some() && previous == value {
            return Vec::new();
        }
        let (id, effect) = self.start_timer(AUTO_CONFIRM_DELAY_MS);
        self.auto_confirm = Some(PendingConfirm { id, index, value });
        vec![effect]
    }

    fn timer_fired(&mut self, id: u64, now: MonoMs) -> Vec<SessionEffect> {
        if let Some(pending) = self.auto_confirm.take_if(|pending| pending.id == id) {
            // Only if the answer is still the one that started it, in the
            // group that is still current.
            let unchanged = self.session.current_group() == pending.index
                && self.session.group(pending.index).map(|g| g.input()) == Some(&pending.value);
            return if unchanged {
                self.confirm_current(now)
            } else {
                Vec::new()
            };
        }
        let Some((_, timer)) = self.phase_timer.take_if(|(pending, _)| *pending == id) else {
            return Vec::new();
        };
        match (timer, self.phase()) {
            (PhaseTimer::Gap, SessionPhase::InterGroupGap { next }) => self.begin_group(next, now),
            (PhaseTimer::Gap, SessionPhase::RepeatGap { index }) => {
                self.set_phase(SessionPhase::Playing { index });
                vec![self.play(index)]
            }
            (PhaseTimer::Countdown, SessionPhase::Countdown { remaining }) if remaining > 1 => {
                self.set_phase(SessionPhase::Countdown {
                    remaining: remaining - 1,
                });
                vec![self.start_phase_timer(PhaseTimer::Countdown, COUNTDOWN_STEP_MS)]
            }
            (PhaseTimer::Countdown, SessionPhase::Countdown { .. }) => self.begin_group(0, now),
            // An answer under way is waited for, however long the pause in
            // it: only Enter, or the answer reaching its length, closes it.
            (PhaseTimer::AnswerWindow, SessionPhase::AwaitingAnswer { .. })
                if self.answer_begun =>
            {
                Vec::new()
            }
            (PhaseTimer::AnswerWindow, SessionPhase::AwaitingAnswer { .. }) => {
                self.confirm_current(now)
            }
            _ => Vec::new(),
        }
    }

    fn play(&self, index: usize) -> SessionEffect {
        let text = self
            .session
            .group(index)
            .map(|g| g.sent().to_string())
            .unwrap_or_default();
        SessionEffect::Play { index, text }
    }

    /// Answer the current group and teach the source what was copied.
    /// Whether it was answered just now.
    fn record_answer(&mut self, now: MonoMs) -> bool {
        let index = self.session.current_group();
        if self.session.input_locked(index) {
            return false;
        }
        match self.session.confirm(index, now) {
            Some(answered) => {
                self.source.learn(answered);
                true
            }
            None => false,
        }
    }

    fn confirm_current(&mut self, now: MonoMs) -> Vec<SessionEffect> {
        if !self.record_answer(now) {
            return Vec::new();
        }
        self.cancel_timers();
        let next = self.session.current_group() + 1;
        if next >= self.session.group_count() {
            self.set_phase(SessionPhase::Finished);
            return vec![
                SessionEffect::StopAudio,
                SessionEffect::PersistAndShowResults,
            ];
        }
        let (text, repeats) = self.source.next_group(self.session.settings());
        let speed = self.source.next_speed(self.session.settings());
        self.session.set_group(next, text, repeats);
        self.session.set_group_speed(next, speed);
        self.set_phase(SessionPhase::InterGroupGap { next });
        let gap_ms = self.group_gap_ms(next);
        vec![
            SessionEffect::StopAudio,
            self.start_phase_timer(PhaseTimer::Gap, gap_ms),
        ]
    }

    fn finish_now(&mut self, now: MonoMs) -> Vec<SessionEffect> {
        // Something typed but not yet confirmed is kept.
        let typed = self
            .session
            .group(self.session.current_group())
            .is_some_and(|g| !g.input().is_empty() && !g.confirmed());
        if typed {
            self.record_answer(now);
        }
        self.end(self.session.any_confirmed())
    }

    fn group_gap_ms(&self, next: usize) -> u32 {
        let playback = &self.session.settings().playback;
        let (char_wpm, effective_wpm) = self
            .session
            .played_wpm(next.saturating_sub(1))
            .unwrap_or((playback.char_wpm_min, playback.effective_wpm_min));
        compute_after_group_gap_ms(
            char_wpm,
            effective_wpm,
            playback.extra_word_space_multiplier,
            playback.group_pause_sec,
        )
    }

    fn begin_group(&mut self, index: usize, now: MonoMs) -> Vec<SessionEffect> {
        self.session.begin_group(index, now);
        self.set_phase(SessionPhase::Playing { index });
        self.repeats_left = self.session.group_repeats(index);
        self.group_elapsed_ms = 0;
        self.answer_begun = false;
        vec![self.play(index)]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::alignment::CopyTally;

    /// Hands out the groups it is given, then "KM" for ever, and remembers
    /// what it was taught.
    #[derive(Clone, Debug, Default)]
    struct Script {
        texts: VecDeque<String>,
        repeats: u32,
        learned: Vec<CopyTally>,
        /// The speed it chooses for every group, if it chooses one.
        speed: Option<(f64, f64)>,
        /// What it was taught about speed: sent, typed, and played at.
        heard: Vec<(String, String, (f64, f64))>,
    }

    impl GroupSource for Script {
        fn next_group(&mut self, _: &TrainingSettings) -> (String, u32) {
            let text = self.texts.pop_front().unwrap_or_else(|| "KM".into());
            (text, self.repeats.max(1))
        }

        fn learn(&mut self, answered: Answered<'_>) {
            self.learned.push(answered.copied.clone());
            if let Some(played) = answered.played {
                self.heard.push((
                    answered.sent.to_string(),
                    answered.received.to_string(),
                    played,
                ));
            }
        }

        fn next_speed(&mut self, _: &TrainingSettings) -> Option<(f64, f64)> {
            self.speed
        }
    }

    fn script(texts: &[&str], repeats: u32) -> Script {
        Script {
            texts: texts.iter().map(|t| (*t).to_string()).collect(),
            repeats,
            ..Script::default()
        }
    }

    fn settings(groups: u32, lock: bool, timeout: f64) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.curriculum.num_groups = groups;
        s.playback.group_timeout = timeout;
        s.playback.lock_input_during_group_playback = lock;
        // The count down has tests of its own; everywhere else the first
        // group goes out at once.
        s.playback.countdown_sec = 0;
        s
    }

    fn start_with(settings: TrainingSettings, source: Script) -> SessionMachine<Script> {
        SessionMachine::start(SessionId::new(7), MonoMs(0), settings, source).0
    }

    /// Two groups, KM then UK, the box locked while sending, a ten-second
    /// answer window.
    fn machine() -> SessionMachine<Script> {
        start_with(settings(2, true, 10.0), script(&["KM", "UK"], 1))
    }

    fn ended(index: usize) -> SessionEvent {
        SessionEvent::PlaybackEnded {
            index,
            duration_sec: 0.5,
            on_time: true,
        }
    }

    /// Send `index` played out at 20 WPM, spaced to 18, ending at `now`: its
    /// start, then its end, and what the end brought.
    fn play_out(m: &mut SessionMachine<Script>, index: usize, now: MonoMs) -> Vec<SessionEffect> {
        let start = SendStart {
            char_wpm: 20.0,
            effective_wpm: 18.0,
            output_at: MonoMs(now.0.saturating_sub(500)),
            char_ends_ms: Vec::new(),
            latency_ms: 0,
        };
        let _ = m.apply(SessionEvent::PlaybackStarted { index, start }, now);
        m.apply(ended(index), now)
    }

    fn typed(index: usize, text: &str) -> SessionEvent {
        SessionEvent::Input {
            index,
            text: text.into(),
            keys: Vec::new(),
        }
    }

    /// The one timer among `effects`, and how long it runs.
    fn timer(effects: &[SessionEffect]) -> (u64, u32) {
        let timers: Vec<(u64, u32)> = effects
            .iter()
            .filter_map(|effect| match effect {
                SessionEffect::StartTimer { id, ms } => Some((*id, *ms)),
                _ => None,
            })
            .collect();
        assert_eq!(timers.len(), 1, "expected one timer in {effects:?}");
        timers[0]
    }

    fn fire(
        m: &mut SessionMachine<Script>,
        effects: &[SessionEffect],
        at: u64,
    ) -> Vec<SessionEffect> {
        let (id, _) = timer(effects);
        m.apply(SessionEvent::TimerFired { id }, MonoMs(at))
    }

    #[test]
    fn a_session_starts_by_sending_its_first_group() {
        let (m, effects) = SessionMachine::start(
            SessionId::new(7),
            MonoMs(0),
            settings(2, true, 10.0),
            script(&["KM"], 1),
        );
        assert_eq!(
            effects,
            vec![SessionEffect::Play {
                index: 0,
                text: "KM".into()
            }]
        );
        assert_eq!(m.phase(), SessionPhase::Playing { index: 0 });
        assert_eq!(m.session().session_id().raw(), 7);
        assert!(m.session().input_locked(0));
    }

    /// A count down holds the first group back a second a step, with the
    /// box closed, and then sends it as if the session had started then.
    #[test]
    fn a_count_down_comes_before_the_first_group() {
        let mut ask = settings(2, false, 10.0);
        ask.playback.countdown_sec = 3;
        let (mut m, effects) =
            SessionMachine::start(SessionId::new(7), MonoMs(0), ask, script(&["KM"], 1));
        assert_eq!(m.phase(), SessionPhase::Countdown { remaining: 3 });
        assert_eq!(timer(&effects).1, COUNTDOWN_STEP_MS);
        let view = m.session().view();
        assert_eq!(view.countdown, Some(3));
        assert!(!view.playing);
        // The group is drawn already, but nothing is sent, and nothing can be
        // typed into it, however the lock is set.
        assert_eq!(m.session().group(0).map(|g| g.sent()), Some("KM"));
        assert!(m.session().input_locked(0));
        assert!(m.apply(typed(0, "K"), MonoMs(10)).is_empty());
        assert_eq!(m.session().group(0).map(|g| g.input()), Some(""));
        assert!(m.apply(SessionEvent::Confirm, MonoMs(20)).is_empty());
        assert!(play_out(&mut m, 0, MonoMs(30)).is_empty());
        assert_eq!(
            m.closes_answer(&SessionEvent::Confirm),
            Some(AnswerClose::Deadline)
        );

        let effects = fire(&mut m, &effects, 1_000);
        assert_eq!(m.phase(), SessionPhase::Countdown { remaining: 2 });
        let effects = fire(&mut m, &effects, 2_000);
        assert_eq!(m.session().view().countdown, Some(1));
        let effects = fire(&mut m, &effects, 3_000);
        assert_eq!(
            effects,
            vec![SessionEffect::Play {
                index: 0,
                text: "KM".into()
            }]
        );
        assert_eq!(m.phase(), SessionPhase::Playing { index: 0 });
        assert_eq!(m.session().view().countdown, None);
        assert!(!m.session().input_locked(0), "the lock is off");
        // The send is timed from the end of the count down, not the start.
        assert_eq!(m.session().group_start(0), Some(MonoMs(3_000)));
    }

    /// Ending the session during the count down keeps nothing: nothing has
    /// been sent.
    #[test]
    fn a_session_ended_during_its_count_down_is_not_kept() {
        let mut ask = settings(2, true, 10.0);
        ask.playback.countdown_sec = 2;
        let mut m = start_with(ask, script(&["KM"], 1));
        let effects = m.apply(SessionEvent::FinishNow, MonoMs(500));
        assert_eq!(
            effects,
            vec![SessionEffect::StopAudio, SessionEffect::AbortToHome]
        );
        assert_eq!(m.phase(), SessionPhase::Aborted);
    }

    /// Confirming, finishing, and auto-confirm running out close the answer.
    /// The answer window closes it only while nothing is typed. A send
    /// ending, or the gap between sends running out, leaves it open for more.
    /// Typing closes nothing, and must not: not even an answer as long as the
    /// group, the one auto-confirm waits on to settle.
    #[test]
    fn only_what_closes_the_answer_says_so() {
        use AnswerClose::{Deadline, Settled, Unbegun};
        let mut m = start_with(settings(2, false, 10.0), script(&["KM"], 2));
        assert_eq!(m.closes_answer(&ended(0)), None);
        let gap = play_out(&mut m, 0, MonoMs(500));
        let (gap_id, _) = timer(&gap);
        assert_eq!(
            m.closes_answer(&SessionEvent::TimerFired { id: gap_id }),
            None
        );
        let _ = m.apply(SessionEvent::TimerFired { id: gap_id }, MonoMs(900));
        let window = play_out(&mut m, 0, MonoMs(1_400));
        let (window_id, _) = timer(&window);
        let window = SessionEvent::TimerFired { id: window_id };
        assert_eq!(m.closes_answer(&window), Some(Unbegun));
        assert_eq!(m.closes_answer(&SessionEvent::Confirm), Some(Deadline));
        assert_eq!(m.closes_answer(&SessionEvent::FinishNow), Some(Deadline));
        assert_eq!(m.closes_answer(&typed(0, "K")), None);
        assert_eq!(m.closes_answer(&typed(0, "KM")), None);
        assert_eq!(m.closes_answer(&SessionEvent::Abort), None);
        // A full answer starts auto-confirm, which waits for it to settle.
        let auto = m.apply(typed(0, "KM"), MonoMs(1_500));
        let (auto_id, _) = timer(&auto);
        assert_eq!(
            m.closes_answer(&SessionEvent::TimerFired { id: auto_id }),
            Some(Settled)
        );
        // An answer begun is not the answer window's to close.
        assert_eq!(m.closes_answer(&window), None);
        // The same full answer again, with auto-confirm waiting on it, or a
        // different one, closes nothing either.
        assert_eq!(m.closes_answer(&typed(0, "KM")), None);
        assert_eq!(m.closes_answer(&typed(0, "MK")), None);
        // A timer the machine has forgotten closes nothing, and nothing does
        // once the session is over.
        assert_eq!(m.closes_answer(&SessionEvent::TimerFired { id: 999 }), None);
        let _ = m.apply(SessionEvent::Abort, MonoMs(1_600));
        assert_eq!(m.closes_answer(&SessionEvent::Confirm), None);
    }

    #[test]
    fn a_repeated_group_is_sent_again_after_a_word_space() {
        let mut m = start_with(settings(2, true, 10.0), script(&["KM"], 3));
        let effects = play_out(&mut m, 0, MonoMs(500));
        assert!(matches!(m.phase(), SessionPhase::RepeatGap { index: 0 }));
        assert_eq!(m.session().view().repeat_total, 3);
        assert_eq!(m.session().view().repeat_done, 1);
        // Still "playing" for the UI, so typing stays locked between sends.
        assert!(m.session().input_locked(0));
        assert_eq!(
            timer(&effects).1,
            compute_group_gap_for_wpm(20.0, 18.0, 1.0),
            "a repeat is a word space, not the reading pause"
        );

        let effects = fire(&mut m, &effects, 900);
        assert!(matches!(m.phase(), SessionPhase::Playing { index: 0 }));
        assert_eq!(
            effects,
            vec![SessionEffect::Play {
                index: 0,
                text: "KM".into()
            }]
        );

        let effects = play_out(&mut m, 0, MonoMs(1_400));
        let _ = fire(&mut m, &effects, 1_800);
        let effects = play_out(&mut m, 0, MonoMs(2_300));
        assert!(matches!(
            m.phase(),
            SessionPhase::AwaitingAnswer { index: 0 }
        ));
        assert_eq!(m.session().view().repeat_done, 3);
        assert!(!m.session().input_locked(0));
        assert_eq!(timer(&effects).1, 10_000, "the answer window opens");
    }

    #[test]
    fn the_repeat_count_starts_again_for_each_group() {
        let mut m = start_with(settings(2, true, 10.0), script(&["KM", "UK"], 2));
        let effects = play_out(&mut m, 0, MonoMs(500));
        let _ = fire(&mut m, &effects, 900);
        let _ = play_out(&mut m, 0, MonoMs(1_400));
        let effects = m.apply(SessionEvent::Confirm, MonoMs(1_500));
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
        let _ = fire(&mut m, &effects, 2_000);
        assert!(matches!(m.phase(), SessionPhase::Playing { index: 1 }));
        assert_eq!(m.session().view().repeat_done, 0);
        let effects = play_out(&mut m, 1, MonoMs(2_500));
        assert!(matches!(m.phase(), SessionPhase::RepeatGap { index: 1 }));
        assert_eq!(effects.len(), 1);
    }

    #[test]
    fn a_confirm_learns_from_the_answer_then_draws_and_waits_for_the_next_group() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(500));
        let _ = m.apply(typed(0, "kx"), MonoMs(600));
        let effects = m.apply(SessionEvent::Confirm, MonoMs(700));
        assert_eq!(effects[0], SessionEffect::StopAudio);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, SessionEffect::Play { .. }))
        );
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
        // The next group is drawn now, so it is known before it is sent.
        assert_eq!(m.session().group(1).map(|g| g.sent()), Some("UK"));
        assert_eq!(m.source.learned, vec![CopyTally::of("KM", "KX")]);

        let effects = fire(&mut m, &effects, 3_000);
        assert_eq!(
            effects,
            vec![SessionEffect::Play {
                index: 1,
                text: "UK".into()
            }]
        );
        assert_eq!(m.session().current_group(), 1);
    }

    /// A source that chooses each group's speed has it kept with the group,
    /// for the sender to use, and learns from each answer at the speed it
    /// was actually played.
    #[test]
    fn a_chosen_speed_is_kept_with_its_group_and_learned_from() {
        let mut source = script(&["KM", "UK"], 1);
        source.speed = Some((27.0, 21.0));
        let mut m = start_with(settings(2, true, 10.0), source);
        assert_eq!(m.session().planned_wpm(0), Some((27.0, 21.0)));
        let _ = play_out(&mut m, 0, MonoMs(500));
        let _ = m.apply(typed(0, "kx"), MonoMs(600));
        let _ = m.apply(SessionEvent::Confirm, MonoMs(700));
        assert_eq!(m.session().planned_wpm(1), Some((27.0, 21.0)));
        assert_eq!(
            m.source.heard,
            vec![("KM".to_string(), "KX".to_string(), (20.0, 18.0))],
            "learned at the speed it was played"
        );
        // A source that chooses none leaves the speed to the station.
        assert_eq!(machine().session().planned_wpm(0), None);
    }

    #[test]
    fn the_answer_window_closing_on_an_empty_box_moves_on() {
        let mut m = machine();
        let effects = play_out(&mut m, 0, MonoMs(500));
        let after = fire(&mut m, &effects, 10_500);
        assert!(m.session().group(0).is_some_and(|g| g.confirmed()));
        assert_eq!(m.source.learned, vec![CopyTally::of("KM", "")]);
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
        assert_eq!(timer(&after).1, m.group_gap_ms(1));
    }

    /// A pause in the middle of an answer is time to think, not a character
    /// skipped: the answer window leaves an answer under way open, however
    /// long the pause, and only Enter or the answer reaching its length
    /// closes it.
    #[test]
    fn the_answer_window_waits_for_an_answer_under_way() {
        let mut m = start_with(settings(2, true, 10.0), script(&["KMR", "UK"], 1));
        let window = play_out(&mut m, 0, MonoMs(500));
        assert!(m.apply(typed(0, "K"), MonoMs(1_000)).is_empty());
        assert!(fire(&mut m, &window, 10_500).is_empty());
        assert_eq!(m.phase(), SessionPhase::AwaitingAnswer { index: 0 });
        assert!(m.session().group(0).is_some_and(|g| !g.confirmed()));
        // A minute later it is still the same answer, and the rest of it is
        // taken.
        let auto = m.apply(typed(0, "KMR"), MonoMs(70_000));
        assert_eq!(timer(&auto).1, AUTO_CONFIRM_DELAY_MS);
        let _ = fire(&mut m, &auto, 70_300);
        assert_eq!(m.source.learned, vec![CopyTally::of("KMR", "KMR")]);
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
    }

    /// Typed and then rubbed out is still an answer begun: the window does
    /// not close on a box being retyped.
    #[test]
    fn an_answer_rubbed_out_is_still_waited_for() {
        let mut m = machine();
        let window = play_out(&mut m, 0, MonoMs(500));
        let _ = m.apply(typed(0, "K"), MonoMs(1_000));
        let _ = m.apply(typed(0, ""), MonoMs(1_200));
        assert!(fire(&mut m, &window, 10_500).is_empty());
        assert_eq!(m.phase(), SessionPhase::AwaitingAnswer { index: 0 });
        // Enter takes what there is.
        let _ = m.apply(SessionEvent::Confirm, MonoMs(11_000));
        assert_eq!(m.source.learned, vec![CopyTally::of("KM", "")]);
    }

    /// Each group gets its own window: an answer begun for one says nothing
    /// about the next.
    #[test]
    fn the_next_group_has_a_window_of_its_own() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(500));
        let _ = m.apply(typed(0, "K"), MonoMs(600));
        let gap = m.apply(SessionEvent::Confirm, MonoMs(700));
        let _ = fire(&mut m, &gap, 3_000);
        let window = play_out(&mut m, 1, MonoMs(3_500));
        let _ = fire(&mut m, &window, 13_500);
        assert_eq!(m.phase(), SessionPhase::Finished);
    }

    #[test]
    fn a_zero_timeout_leaves_the_answer_window_open() {
        let mut m = start_with(settings(2, true, 0.0), script(&["KM"], 1));
        assert!(play_out(&mut m, 0, MonoMs(500)).is_empty());
        assert_eq!(m.phase(), SessionPhase::AwaitingAnswer { index: 0 });
    }

    #[test]
    fn auto_confirm_advances_after_length_match() {
        let mut m = start_with(settings(1, false, 0.0), script(&["KM"], 1));
        let _ = play_out(&mut m, 0, MonoMs(200));
        let effects = m.apply(typed(0, "KM"), MonoMs(300));
        assert_eq!(timer(&effects).1, AUTO_CONFIRM_DELAY_MS);
        let done = fire(&mut m, &effects, 600);
        assert_eq!(m.phase(), SessionPhase::Finished);
        assert!(done.contains(&SessionEffect::PersistAndShowResults));
    }

    #[test]
    fn typing_and_then_correcting_cancels_the_pending_auto_confirm() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(1_000));
        let effects = m.apply(typed(0, "KM"), MonoMs(1_100));
        // A backspace makes the length wrong again and withdraws the confirm.
        assert!(m.apply(typed(0, "K"), MonoMs(1_200)).is_empty());
        assert!(fire(&mut m, &effects, 1_300).is_empty());
        assert!(!m.session().any_confirmed());
    }

    #[test]
    fn repeating_the_same_full_answer_does_not_restart_auto_confirm() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(1_000));
        let first = m.apply(typed(0, "KM"), MonoMs(1_100));
        assert!(m.apply(typed(0, "KM"), MonoMs(1_150)).is_empty());
        let done = fire(&mut m, &first, 1_400);
        assert!(m.session().any_confirmed());
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
        assert_eq!(timer(&done).1, m.group_gap_ms(1));
    }

    #[test]
    fn an_auto_confirm_whose_text_changed_does_not_fire() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(1_000));
        let effects = m.apply(typed(0, "KM"), MonoMs(1_100));
        // Same length, different text: the pending confirm is stale.
        let newer = m.apply(typed(0, "MK"), MonoMs(1_150));
        assert!(fire(&mut m, &effects, 1_200).is_empty());
        assert!(!m.session().any_confirmed());
        // The newer one still does.
        let _ = fire(&mut m, &newer, 1_500);
        assert!(m.session().any_confirmed());
    }

    #[test]
    fn an_auto_confirm_for_a_group_that_has_moved_on_does_not_fire() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(1_000));
        let pending = m.apply(typed(0, "KM"), MonoMs(1_100));
        // The answer is confirmed by hand first, so group 0 is no longer current.
        let gap = m.apply(SessionEvent::Confirm, MonoMs(1_150));
        let _ = fire(&mut m, &gap, 1_200);
        assert_eq!(m.session().current_group(), 1);
        assert!(fire(&mut m, &pending, 1_300).is_empty());
        assert_eq!(m.phase(), SessionPhase::Playing { index: 1 });
    }

    /// A timer is the machine's to interpret. One it has forgotten — a gap
    /// cut short by a confirm, a window closed by an answer — does nothing,
    /// whatever phase the session is in by the time it fires.
    #[test]
    fn a_timer_the_machine_has_forgotten_does_nothing() {
        let mut m = machine();
        let window = play_out(&mut m, 0, MonoMs(500));
        let gap = m.apply(SessionEvent::Confirm, MonoMs(600));
        assert!(
            fire(&mut m, &window, 700).is_empty(),
            "the window was closed"
        );
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
        let _ = fire(&mut m, &gap, 800);
        assert!(fire(&mut m, &gap, 900).is_empty(), "a timer fires once");
        assert!(
            m.apply(SessionEvent::TimerFired { id: 999 }, MonoMs(1_000))
                .is_empty()
        );
        assert_eq!(m.phase(), SessionPhase::Playing { index: 1 });
    }

    #[test]
    fn typing_is_ignored_while_locked_out_of_turn_or_between_groups() {
        let mut m = machine();
        assert!(m.apply(typed(0, "KM"), MonoMs(5)).is_empty(), "locked");
        assert_eq!(m.session().group(0).map(|g| g.input()), Some(""));

        let mut m = start_with(settings(2, false, 10.0), script(&["KM"], 1));
        assert!(
            m.apply(typed(1, "XX"), MonoMs(5)).is_empty(),
            "not its turn"
        );
        assert_eq!(m.session().group(1).map(|g| g.input()), Some(""));

        let _ = play_out(&mut m, 0, MonoMs(1_000));
        let _ = m.apply(SessionEvent::Confirm, MonoMs(1_100));
        assert!(
            m.apply(typed(1, "X"), MonoMs(1_200)).is_empty(),
            "in the gap"
        );
    }

    #[test]
    fn confirming_while_the_group_is_still_sending_is_ignored_when_locked() {
        let mut m = machine();
        assert!(m.apply(SessionEvent::Confirm, MonoMs(100)).is_empty());
        assert!(!m.session().any_confirmed());
    }

    #[test]
    fn confirming_during_the_send_when_unlocked_stops_the_audio() {
        let mut m = start_with(settings(2, false, 10.0), script(&["KM"], 1));
        let _ = m.apply(typed(0, "KM"), MonoMs(40));
        let effects = m.apply(SessionEvent::Confirm, MonoMs(50));
        assert!(effects.contains(&SessionEffect::StopAudio));
        assert!(matches!(m.phase(), SessionPhase::InterGroupGap { next: 1 }));
        assert!(!m.session().view().playing);
        // The send stopping late is the confirm's doing, not a failure.
        assert!(
            m.apply(SessionEvent::PlaybackStopped { index: 0 }, MonoMs(80))
                .is_empty()
        );
        let result = m.session().result(100, "2026-09-01".into());
        assert_eq!(
            result.group_timings[0].time_to_complete_ms,
            2.0 * crate::score::MIN_RESPONSE_MS_PER_CHAR
        );
    }

    #[test]
    fn a_second_confirm_for_the_same_group_does_nothing() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(1_000));
        assert!(!m.apply(SessionEvent::Confirm, MonoMs(1_100)).is_empty());
        assert!(m.apply(SessionEvent::Confirm, MonoMs(1_200)).is_empty());
        assert_eq!(m.source.learned.len(), 1);
    }

    #[test]
    fn a_stale_playback_report_is_ignored() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(200));
        assert!(play_out(&mut m, 0, MonoMs(400)).is_empty());
        assert!(matches!(
            m.phase(),
            SessionPhase::AwaitingAnswer { index: 0 }
        ));
        assert!(
            m.apply(SessionEvent::PlaybackStopped { index: 1 }, MonoMs(500))
                .is_empty()
        );
    }

    #[test]
    fn a_send_stopping_ends_the_session_keeping_what_was_scored() {
        // Nothing answered yet: nothing to keep.
        let mut m = machine();
        let effects = m.apply(SessionEvent::PlaybackStopped { index: 0 }, MonoMs(10));
        assert_eq!(m.phase(), SessionPhase::Aborted);
        assert_eq!(
            effects,
            vec![SessionEffect::StopAudio, SessionEffect::AbortToHome]
        );

        // One group answered: the session is kept.
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(200));
        let gap = m.apply(SessionEvent::Confirm, MonoMs(300));
        let _ = fire(&mut m, &gap, 800);
        assert!(matches!(m.phase(), SessionPhase::Playing { index: 1 }));
        let effects = m.apply(SessionEvent::PlaybackStopped { index: 1 }, MonoMs(900));
        assert_eq!(m.phase(), SessionPhase::Finished);
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
    }

    #[test]
    fn ending_a_session_keeps_an_answer_typed_but_not_confirmed() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(1_000));
        let _ = m.apply(typed(0, " km "), MonoMs(1_100));
        let effects = m.apply(SessionEvent::FinishNow, MonoMs(1_200));
        assert_eq!(m.phase(), SessionPhase::Finished);
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
        assert!(m.session().any_confirmed());
        // Tidied on the way in.
        assert_eq!(m.session().group(0).map(|g| g.input()), Some("KM"));
        // And it is the last group drawn: ending does not reach for another.
        assert_eq!(m.session().group(1).map(|g| g.sent()), Some(""));
    }

    #[test]
    fn ending_a_session_with_nothing_typed_goes_home() {
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(1_000));
        let _ = m.apply(typed(0, "   "), MonoMs(1_100));
        let effects = m.apply(SessionEvent::FinishNow, MonoMs(1_200));
        assert_eq!(m.phase(), SessionPhase::Aborted);
        assert!(effects.contains(&SessionEffect::AbortToHome));
    }

    #[test]
    fn abort_during_play_goes_home() {
        let mut m = machine();
        let effects = m.apply(SessionEvent::Abort, MonoMs(10));
        assert_eq!(m.phase(), SessionPhase::Aborted);
        assert!(effects.contains(&SessionEffect::AbortToHome));
        assert!(effects.contains(&SessionEffect::StopAudio));
    }

    #[test]
    fn nothing_at_all_happens_once_the_session_is_over() {
        let mut m = machine();
        let _ = m.apply(SessionEvent::Abort, MonoMs(10));
        assert!(m.is_terminal());
        for event in [
            SessionEvent::Confirm,
            SessionEvent::TimerFired { id: 1 },
            SessionEvent::FinishNow,
            SessionEvent::Abort,
            SessionEvent::PlaybackStopped { index: 0 },
            ended(0),
        ] {
            assert!(m.apply(event.clone(), MonoMs(20)).is_empty(), "{event:?}");
        }
    }

    #[test]
    fn the_last_answer_finishes_the_session() {
        let mut m = start_with(settings(1, true, 10.0), script(&["KM"], 1));
        let _ = play_out(&mut m, 0, MonoMs(500));
        let effects = m.apply(SessionEvent::Confirm, MonoMs(600));
        assert_eq!(
            effects,
            vec![
                SessionEffect::StopAudio,
                SessionEffect::PersistAndShowResults
            ]
        );
        assert_eq!(m.phase(), SessionPhase::Finished);
    }

    #[test]
    fn the_gap_after_a_silent_group_falls_back_to_the_settings_speed() {
        // A group whose playback never reported a speed leaves no measured WPM,
        // so the gap is computed from the settings instead.
        let mut m = machine();
        let _ = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 0.0,
                on_time: true,
            },
            MonoMs(10),
        );
        let effects = m.apply(SessionEvent::Confirm, MonoMs(20));
        let playback = &m.session().settings().playback;
        assert_eq!(
            timer(&effects).1,
            compute_after_group_gap_ms(
                playback.char_wpm_min,
                playback.effective_wpm_min,
                playback.extra_word_space_multiplier,
                playback.group_pause_sec,
            )
        );
    }

    /// A send that stalls and catches up can report its end before its audio
    /// could have finished. The answer is timed from where the audio ends.
    #[test]
    fn a_send_never_ends_before_its_audio_does() {
        let mut m = start_with(settings(2, true, 10.0), script(&["KMURE"], 1));
        let _ = m.apply(
            SessionEvent::PlaybackEnded {
                index: 0,
                duration_sec: 2.0,
                on_time: true,
            },
            MonoMs(500),
        );
        let _ = m.apply(typed(0, "KMURE"), MonoMs(4_000));
        let _ = m.apply(SessionEvent::Confirm, MonoMs(4_000));
        let result = m.session().result(5_000, "2026-09-01".into());
        assert_eq!(result.group_timings[0].time_to_complete_ms, 2_000.0);
    }

    /// A group typed along and confirmed before its send is seen to end
    /// keeps what its characters are timed from, and the speed it was sent
    /// at: both came with the send's start.
    #[test]
    fn a_group_confirmed_before_its_send_ends_keeps_its_timing() {
        let mut m = start_with(settings(1, false, 10.0), script(&["KM"], 1));
        let start = SendStart {
            char_wpm: 27.0,
            effective_wpm: 27.0,
            output_at: MonoMs(20),
            char_ends_ms: vec![180, 540],
            latency_ms: 0,
        };
        let _ = m.apply(
            SessionEvent::PlaybackStarted { index: 0, start },
            MonoMs(10),
        );
        let typed = SessionEvent::Input {
            index: 0,
            text: "KM".into(),
            keys: vec![Some(MonoMs(450)), Some(MonoMs(800))],
        };
        let _ = m.apply(typed, MonoMs(800));
        let effects = m.apply(SessionEvent::Confirm, MonoMs(820));
        assert!(effects.contains(&SessionEffect::StopAudio));
        let result = m.session().result(900, "2026-09-01".into());
        let timing = &result.group_timings[0];
        assert_eq!(timing.char_wpm, Some(27.0));
        let times = timing.char_times.as_ref().expect("timed");
        assert_eq!(crate::recognise("KM", "KM", times), [Some(250), Some(240)]);
    }

    /// A send that played late cannot be timed; its speed is kept all the
    /// same.
    #[test]
    fn a_send_that_played_late_is_not_timed() {
        let mut m = start_with(settings(1, true, 10.0), script(&["K"], 1));
        let start = SendStart {
            char_wpm: 22.0,
            effective_wpm: 22.0,
            output_at: MonoMs(20),
            char_ends_ms: vec![150],
            latency_ms: 0,
        };
        let _ = m.apply(
            SessionEvent::PlaybackStarted { index: 0, start },
            MonoMs(10),
        );
        let late = SessionEvent::PlaybackEnded {
            index: 0,
            duration_sec: 0.15,
            on_time: false,
        };
        let _ = m.apply(late, MonoMs(900));
        let _ = m.apply(typed(0, "K"), MonoMs(1_200));
        let _ = m.apply(SessionEvent::Confirm, MonoMs(1_200));
        let timing = &m.session().result(1_300, "2026-09-01".into()).group_timings[0];
        assert_eq!(
            (timing.char_wpm, timing.char_times.clone()),
            (Some(22.0), None)
        );
    }
}
