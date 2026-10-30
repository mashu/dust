//! Pure session state machine. The UI runtime applies [`SessionEffect`]s; this
//! module never sleeps, plays audio, or touches Dioxus.
//!
//! The machine owns everything a session decides: which group comes next
//! (through its [`GroupSource`]), what a timer running out means, and what
//! the sampler learns from each answer. The runtime only plays, waits, and
//! reports back.

use crate::contest::PauseWhy;
use crate::recognition::Keys;
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
    /// A send played to its end.
    PlaybackEnded {
        index: usize,
        duration_sec: f64,
    },
    /// A send of the group was held up on its way, its audio parked or its
    /// clock fallen behind, however it ended: it went out later than
    /// planned, and a first send so cannot be timed from.
    PlaybackHeldUp {
        index: usize,
    },
    /// Some of the send of the group playing went unheard: the browser fell
    /// so far behind its audio clock that it skipped it. Said as soon as it
    /// is found, before the send ends, which is held up as well. A group
    /// heard in part is not answered: once the send ends, the group goes out
    /// again from the start, as after a pause. The send counts as no
    /// hearing of the group, however it ends.
    PlaybackMissed {
        index: usize,
    },
    /// The send stopped before it finished — the audio failed after every
    /// retry, or something stopped it from outside. Either way the session
    /// pauses, as a run does: its sound comes back from the tap on Resume,
    /// which sends the group again from the start.
    PlaybackStopped {
        index: usize,
    },
    /// What the box for `index` holds, and when each of its characters
    /// went in ([`crate::KeyLog`]): none where it cannot be timed.
    Input {
        index: usize,
        text: String,
        keys: Keys<MonoMs>,
    },
    Confirm,
    /// Nobody can hear the session for now — the page went out of sight —
    /// so it holds where it is rather than send groups to no one and close
    /// their answers empty.
    Pause,
    /// Carry on from a pause: a group partly sent goes out again from the
    /// start, after the count down the settings ask for; one sent in full is
    /// waited on again, with a fresh answer window.
    Resume,
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
    /// Stop everything the session plays, the band too, and give the audio
    /// back: a paused session plays nothing, and lets the output sleep.
    /// [`SessionEvent::Resume`] comes with the audio claimed afresh.
    ReleaseAudio,
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
    /// The pause before a group heard in part goes out again.
    Again,
}

/// What a paused session goes on with once it is resumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resume {
    /// Send group `index`, from the start, after the count down.
    Send(usize),
    /// Wait again for the answer to group `index`, sent in full already.
    Answer(usize),
}

/// How long after a group starts an empty Enter is taken for one meant for
/// the group before it — a key held down and repeating, or a second press
/// that lands after the short gap between groups — rather than a skip.
pub const EMPTY_CONFIRM_GUARD_MS: u64 = 600;

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
    /// The group the count down leads to: the first, or the one a paused
    /// session goes on with.
    countdown_to: usize,
    /// What a paused session goes on with, and since when it has been
    /// paused; none unless it is paused.
    resume: Option<(Resume, MonoMs)>,
    /// Some of the send in flight went unheard
    /// ([`SessionEvent::PlaybackMissed`]): once it ends, the group goes out
    /// again.
    missed: bool,
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
            countdown_to: 0,
            resume: None,
            missed: false,
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
            | SessionEvent::PlaybackHeldUp { .. }
            | SessionEvent::PlaybackMissed { .. }
            | SessionEvent::PlaybackStopped { .. }
            | SessionEvent::Input { .. }
            | SessionEvent::Pause
            | SessionEvent::Resume
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
                self.pause(PauseWhy::Sound, now)
            }
            SessionEvent::PlaybackStarted { index, start } => {
                if self.playing_index() == Some(index) {
                    self.session.start_playback(index, &start);
                    // An attempt tried again after one that failed is heard
                    // afresh.
                    self.missed = false;
                }
                Vec::new()
            }
            SessionEvent::PlaybackMissed { index } => {
                if self.playing_index() == Some(index) {
                    // No hearing, however it ends: cut by a pause, typed
                    // along, or sent again.
                    self.session.drop_send(index);
                    self.missed = true;
                }
                Vec::new()
            }
            SessionEvent::PlaybackEnded {
                index,
                duration_sec,
            } => self.playback_ended(index, now, duration_sec),
            // Whenever it comes, the group answered or not: its timing is
            // read once the session is kept.
            SessionEvent::PlaybackHeldUp { index } => {
                self.session.note_held_up(index);
                Vec::new()
            }
            SessionEvent::Input { index, text, keys } => self.input(index, &text, &keys, now),
            SessionEvent::Confirm if self.confirm_too_soon(now) => Vec::new(),
            SessionEvent::Confirm => self.confirm_current(now),
            SessionEvent::Pause => self.pause(PauseWhy::Hidden, now),
            SessionEvent::Resume => self.resume(now),
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

    /// Send `index` ended at `now`, having lasted `duration_sec`.
    fn playback_ended(
        &mut self,
        index: usize,
        now: MonoMs,
        duration_sec: f64,
    ) -> Vec<SessionEffect> {
        if self.playing_index() != Some(index) {
            return Vec::new();
        }
        // Heard in part, the group is not answered but sent again, the send
        // no send heard; unless it was answered in full as it played, as a
        // pause leaves a group so.
        let answered = self
            .auto_confirm
            .as_ref()
            .is_some_and(|pending| pending.index == index);
        if std::mem::take(&mut self.missed) && !answered {
            return self.send_again(index);
        }
        self.session.note_play_finished(index);
        let played_ms = if duration_sec.is_finite() {
            (duration_sec.max(0.0) * 1000.0).round() as u64
        } else {
            0
        };
        self.group_elapsed_ms = self.group_elapsed_ms.saturating_add(played_ms);
        // The send cannot have ended before its audio did, whatever the
        // runtime's clock made of a send that stalled and caught up. An
        // answer after it, in the gap or along the next send, is timed from
        // it.
        let earliest = self
            .session
            .group_start(index)
            .map_or(now, |start| start.plus(self.group_elapsed_ms));
        self.session.end_playback(index, now.max(earliest));
        if self.repeats_left > 1 {
            self.repeats_left -= 1;
            let (char_wpm, effective_wpm) = self.session.sent_wpm(index);
            let gap = compute_group_gap_for_wpm(
                char_wpm,
                effective_wpm,
                self.session.settings().playback.extra_word_space_multiplier,
            );
            self.group_elapsed_ms = self.group_elapsed_ms.saturating_add(u64::from(gap));
            self.set_phase(SessionPhase::RepeatGap { index });
            return vec![self.start_phase_timer(PhaseTimer::Gap, gap)];
        }
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
        keys: &Keys<MonoMs>,
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
            (PhaseTimer::Countdown, SessionPhase::Countdown { .. }) => {
                self.begin_group(self.countdown_to, now)
            }
            (PhaseTimer::Again, SessionPhase::RepeatGap { index }) => self.begin_group(index, now),
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
    ///
    /// The box being locked — while a group is sent, counting down, or
    /// paused — refuses an answer, unless `finishing`: End session keeps what
    /// was typed before the lock, whenever it is pressed.
    fn record_answer(&mut self, now: MonoMs, finishing: bool) -> bool {
        let index = self.session.current_group();
        if self.session.input_locked(index) && !finishing {
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
        if !self.record_answer(now, false) {
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
        // Ended while paused waiting on an answer: the pause is no time
        // spent answering, as on Resume. Ended while paused part way through
        // a group, what was typed is answered as it stood at the pause.
        let answered_at = match self.resume {
            Some((Resume::Answer(index), since)) => {
                self.session.note_paused(index, now.since(since));
                now
            }
            Some((Resume::Send(_), since)) => since,
            None => now,
        };
        // Something typed but not yet confirmed is kept.
        let typed = self
            .session
            .group(self.session.current_group())
            .is_some_and(|g| !g.input().is_empty() && !g.confirmed());
        if typed {
            self.record_answer(answered_at, true);
        }
        self.end(self.session.any_confirmed())
    }

    /// Whether an Enter now, with nothing typed, comes too soon after the
    /// group being sent started to be meant for it ([`EMPTY_CONFIRM_GUARD_MS`]).
    fn confirm_too_soon(&self, now: MonoMs) -> bool {
        let SessionPhase::Playing { index } = self.phase() else {
            return false;
        };
        let typed = self
            .session
            .group(index)
            .is_some_and(|g| !g.input().is_empty());
        let started = self.session.group_start(index);
        !typed && started.is_some_and(|start| now.since(start) < EMPTY_CONFIRM_GUARD_MS)
    }

    /// Hold the session where it is, for `why`: the sound and the band
    /// stopped, every timer let go. A group answered in full, its
    /// auto-confirm still waiting, needs no sending again: it is confirmed
    /// once resumed. A send cut short counts as heard once its first
    /// character had sounded, unless some of it had gone unheard.
    fn pause(&mut self, why: PauseWhy, now: MonoMs) -> Vec<SessionEffect> {
        let complete = self.auto_confirm.as_ref().map(|pending| pending.index);
        let resume = match self.phase() {
            SessionPhase::Countdown { .. } => Resume::Send(self.countdown_to),
            SessionPhase::Playing { index } | SessionPhase::RepeatGap { index }
                if complete == Some(index) =>
            {
                Resume::Answer(index)
            }
            SessionPhase::Playing { index } | SessionPhase::RepeatGap { index } => {
                Resume::Send(index)
            }
            SessionPhase::AwaitingAnswer { index } => Resume::Answer(index),
            SessionPhase::InterGroupGap { next } => Resume::Send(next),
            SessionPhase::Paused { .. } | SessionPhase::Finished | SessionPhase::Aborted => {
                return Vec::new();
            }
        };
        let (Resume::Send(index) | Resume::Answer(index)) = resume;
        self.session.cut_send(index, now);
        self.cancel_timers();
        self.resume = Some((resume, now));
        self.set_phase(SessionPhase::Paused { index, why });
        vec![SessionEffect::ReleaseAudio]
    }

    /// Carry on from a pause, as [`SessionEvent::Resume`] says.
    fn resume(&mut self, now: MonoMs) -> Vec<SessionEffect> {
        let Some((resume, since)) = self.resume.take() else {
            return Vec::new();
        };
        match resume {
            Resume::Answer(index) => {
                // The pause is no time spent answering, and keys typed
                // either side of it cannot be timed.
                self.session.note_paused(index, now.since(since));
                self.set_phase(SessionPhase::AwaitingAnswer { index });
                let Some(group) = self.session.group(index) else {
                    return Vec::new();
                };
                let (sent, value) = (group.sent().to_string(), group.input().to_string());
                if answer_length_matches(&sent, &value) {
                    let (id, effect) = self.start_timer(AUTO_CONFIRM_DELAY_MS);
                    self.auto_confirm = Some(PendingConfirm { id, index, value });
                    return vec![effect];
                }
                let timeout_ms =
                    (self.session.settings().playback.group_timeout * 1000.0).round() as u32;
                if timeout_ms == 0 {
                    return Vec::new();
                }
                vec![self.start_phase_timer(PhaseTimer::AnswerWindow, timeout_ms)]
            }
            Resume::Send(index) => {
                // Nor is the pause time spent answering what was typed
                // before it, should the session end in the count down,
                // before the group goes out again.
                self.session.note_paused(index, now.since(since));
                let countdown = self.session.settings().playback.countdown_sec;
                if countdown == 0 {
                    return self.begin_group(index, now);
                }
                self.count_down(index, countdown)
            }
        }
    }

    /// Count down `from` seconds to group `index`.
    fn count_down(&mut self, index: usize, from: u32) -> Vec<SessionEffect> {
        self.count_down_to(index);
        self.set_phase(SessionPhase::Countdown { remaining: from });
        vec![self.start_phase_timer(PhaseTimer::Countdown, COUNTDOWN_STEP_MS)]
    }

    /// The count down is to group `index`.
    fn count_down_to(&mut self, index: usize) {
        self.countdown_to = index;
        self.session.set_countdown_to(index);
    }

    /// Send group `index` again from the start, some of it unheard: after
    /// the count down the settings ask for, as after a pause; with none,
    /// after the pause between groups, its box as it is while it is sent.
    fn send_again(&mut self, index: usize) -> Vec<SessionEffect> {
        let countdown = self.session.settings().playback.countdown_sec;
        if countdown > 0 {
            return self.count_down(index, countdown);
        }
        self.set_phase(SessionPhase::RepeatGap { index });
        let gap_ms = self.group_gap_ms(index + 1);
        vec![self.start_phase_timer(PhaseTimer::Again, gap_ms)]
    }

    fn group_gap_ms(&self, next: usize) -> u32 {
        let playback = &self.session.settings().playback;
        let (char_wpm, effective_wpm) = self.session.sent_wpm(next.saturating_sub(1));
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
        self.missed = false;
        // Sent again after a pause, what was typed before it is kept.
        self.answer_begun = self
            .session
            .group(index)
            .is_some_and(|group| !group.input().is_empty());
        vec![self.play(index)]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::alignment::CopyTally;
    use crate::session::{CountdownTo, SessionResult};

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
            first_char_ms: 0,
            latency_ms: 0,
        };
        let _ = m.apply(SessionEvent::PlaybackStarted { index, start }, now);
        m.apply(ended(index), now)
    }

    fn typed(index: usize, text: &str) -> SessionEvent {
        SessionEvent::Input {
            index,
            text: text.into(),
            keys: Keys::default(),
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

    /// A group partly sent when the session is paused is stopped, nothing
    /// runs while it is held, and it goes out again from the start once it
    /// is resumed, after the count down; what was typed into it is kept.
    #[test]
    fn a_paused_group_is_sent_again_after_the_count_down() {
        let mut ask = settings(2, false, 10.0);
        ask.playback.countdown_sec = 2;
        let (mut m, effects) =
            SessionMachine::start(SessionId::new(7), MonoMs(0), ask, script(&["KM"], 1));
        let effects = fire(&mut m, &effects, 1_000);
        let _ = fire(&mut m, &effects, 2_000);
        assert_eq!(m.phase(), SessionPhase::Playing { index: 0 });
        let _ = m.apply(typed(0, "K"), MonoMs(2_500));

        let effects = m.apply(SessionEvent::Pause, MonoMs(2_600));
        assert_eq!(effects, vec![SessionEffect::ReleaseAudio]);
        assert_eq!(
            m.phase(),
            SessionPhase::Paused {
                index: 0,
                why: PauseWhy::Hidden
            }
        );
        let view = m.session().view();
        assert!(view.paused && view.locked && !view.playing);
        // Held: nothing it was waiting on moves it, nothing can be typed,
        // and Enter answers nothing.
        assert!(play_out(&mut m, 0, MonoMs(3_000)).is_empty());
        assert!(m.apply(typed(0, "KM"), MonoMs(3_100)).is_empty());
        assert!(m.apply(SessionEvent::Confirm, MonoMs(3_200)).is_empty());
        assert!(m.apply(SessionEvent::Pause, MonoMs(3_300)).is_empty());
        assert_eq!(m.session().group(0).map(|g| g.input()), Some("K"));

        let effects = m.apply(SessionEvent::Resume, MonoMs(60_000));
        assert_eq!(m.phase(), SessionPhase::Countdown { remaining: 2 });
        let effects = fire(&mut m, &effects, 61_000);
        let effects = fire(&mut m, &effects, 62_000);
        assert_eq!(
            effects,
            vec![SessionEffect::Play {
                index: 0,
                text: "KM".into()
            }]
        );
        assert_eq!(m.phase(), SessionPhase::Playing { index: 0 });
        assert_eq!(m.session().group_start(0), Some(MonoMs(62_000)));
        assert_eq!(m.session().group(0).map(|g| g.input()), Some("K"));
        assert!(m.apply(SessionEvent::Resume, MonoMs(62_100)).is_empty());
    }

    /// A group heard in full when the session is paused is not sent again:
    /// its answer is waited on afresh, with a whole answer window, and the
    /// window that was running when it paused closes nothing.
    #[test]
    fn a_group_heard_in_full_waits_for_its_answer_after_a_pause() {
        let mut m = machine();
        let window = play_out(&mut m, 0, MonoMs(500));
        assert_eq!(m.phase(), SessionPhase::AwaitingAnswer { index: 0 });
        let _ = m.apply(SessionEvent::Pause, MonoMs(600));
        assert_eq!(
            m.phase(),
            SessionPhase::Paused {
                index: 0,
                why: PauseWhy::Hidden
            }
        );
        assert!(fire(&mut m, &window, 20_000).is_empty());
        let fresh = m.apply(SessionEvent::Resume, MonoMs(30_000));
        assert_eq!(m.phase(), SessionPhase::AwaitingAnswer { index: 0 });
        assert_eq!(timer(&fresh).1, 10_000);
        let next = fire(&mut m, &fresh, 40_000);
        assert_eq!(m.phase(), SessionPhase::InterGroupGap { next: 1 });
        assert!(next.contains(&SessionEffect::StopAudio));
    }

    /// Paused between two groups, the session goes on with the next one,
    /// straight away with no count down asked for.
    #[test]
    fn a_session_paused_between_groups_goes_on_with_the_next() {
        let mut m = start_with(settings(2, false, 10.0), script(&["KM", "UK"], 1));
        let _ = play_out(&mut m, 0, MonoMs(500));
        let _ = m.apply(typed(0, "KM"), MonoMs(600));
        let _ = m.apply(SessionEvent::Confirm, MonoMs(700));
        assert_eq!(m.phase(), SessionPhase::InterGroupGap { next: 1 });
        let _ = m.apply(SessionEvent::Pause, MonoMs(800));
        assert_eq!(
            m.phase(),
            SessionPhase::Paused {
                index: 1,
                why: PauseWhy::Hidden
            }
        );
        let effects = m.apply(SessionEvent::Resume, MonoMs(9_000));
        assert_eq!(
            effects,
            vec![SessionEffect::Play {
                index: 1,
                text: "UK".into()
            }]
        );
        assert_eq!(m.phase(), SessionPhase::Playing { index: 1 });
    }

    /// End session while paused keeps what was typed into the group the
    /// pause held, as it does with no pause: here on the first group, which
    /// without it would leave nothing to keep, and the session would go.
    #[test]
    fn ending_a_paused_session_keeps_what_was_typed() {
        let mut m = start_with(settings(2, false, 10.0), script(&["KM", "UK"], 1));
        let _ = m.apply(typed(0, "K"), MonoMs(300));
        let _ = m.apply(SessionEvent::Pause, MonoMs(400));
        let effects = m.apply(SessionEvent::FinishNow, MonoMs(5_000));
        assert!(
            effects.contains(&SessionEffect::PersistAndShowResults),
            "{effects:?}"
        );
        assert_eq!(m.phase(), SessionPhase::Finished);
        assert!(m.session().group(0).is_some_and(|g| g.confirmed()));
        assert_eq!(m.session().group(0).map(|g| g.input()), Some("K"));
    }

    /// A pause in the answer window is no time spent answering: the group is
    /// timed from its end as if the pause had not been, and its characters,
    /// keyed either side of the pause, are not timed against when they
    /// ended.
    #[test]
    fn a_pause_in_the_answer_window_is_not_answering_time() {
        let mut m = start_with(settings(1, false, 10.0), script(&["KMR"], 1));
        let start = SendStart {
            char_wpm: 20.0,
            effective_wpm: 18.0,
            output_at: MonoMs(0),
            char_ends_ms: vec![200, 500, 800],
            first_char_ms: 0,
            latency_ms: 0,
        };
        let _ = m.apply(SessionEvent::PlaybackStarted { index: 0, start }, MonoMs(0));
        let _ = m.apply(ended(0), MonoMs(800));
        let _ = m.apply(typed(0, "K"), MonoMs(850));
        let _ = m.apply(SessionEvent::Pause, MonoMs(900));
        let _ = m.apply(SessionEvent::Resume, MonoMs(60_900));
        let _ = m.apply(typed(0, "KMR"), MonoMs(61_400));
        let effects = m.apply(SessionEvent::Confirm, MonoMs(61_500));
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
        let result = m.session().result(0, "2026-01-01".into());
        let timing = &result.group_timings[0];
        assert!(
            (timing.time_to_complete_ms - 600.0).abs() < 1.0,
            "{} ms to answer",
            timing.time_to_complete_ms
        );
        assert_eq!(timing.char_times, None, "timed across the pause");
    }

    /// `KMR` sent from 0 and heard in full by 800, as the pause tests hear it.
    fn heard_kmr() -> SessionMachine<Script> {
        let mut m = start_with(settings(1, false, 10.0), script(&["KMR"], 1));
        let start = SendStart {
            char_wpm: 20.0,
            effective_wpm: 18.0,
            output_at: MonoMs(0),
            char_ends_ms: vec![200, 500, 800],
            first_char_ms: 0,
            latency_ms: 0,
        };
        let _ = m.apply(SessionEvent::PlaybackStarted { index: 0, start }, MonoMs(0));
        let _ = m.apply(ended(0), MonoMs(800));
        m
    }

    /// End session while paused in the answer window takes the pause off the
    /// answer's time, as Resume does: the group was timed at the whole
    /// pause, ten minutes here, and scored as answered that slowly.
    #[test]
    fn ending_a_session_paused_in_the_answer_window_is_not_answering_time() {
        let mut m = heard_kmr();
        let _ = m.apply(typed(0, "KM"), MonoMs(1_000));
        let _ = m.apply(SessionEvent::Pause, MonoMs(1_100));
        let effects = m.apply(SessionEvent::FinishNow, MonoMs(601_100));
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
        let result = m.session().result(0, "2026-01-01".into());
        let timing = &result.group_timings[0];
        assert!(
            (timing.time_to_complete_ms - 300.0).abs() < 1.0,
            "{} ms to answer",
            timing.time_to_complete_ms
        );
        assert_eq!(timing.char_times, None, "timed across the pause");
    }

    /// An answer complete when paused and corrected after it is timed from
    /// the end of the send less the pause, and its characters, keyed either
    /// side of the pause, are not timed against when they ended. It kept
    /// its stamp from before the pause until corrected, then took a new one
    /// with the pause in it.
    #[test]
    fn an_answer_complete_at_a_pause_and_corrected_after_is_not_timed_across_it() {
        let mut m = heard_kmr();
        let _ = m.apply(typed(0, "KMX"), MonoMs(1_000));
        let _ = m.apply(SessionEvent::Pause, MonoMs(1_100));
        let _ = m.apply(SessionEvent::Resume, MonoMs(601_100));
        let _ = m.apply(typed(0, "KM"), MonoMs(601_200));
        let _ = m.apply(typed(0, "KMR"), MonoMs(601_300));
        let effects = m.apply(SessionEvent::Confirm, MonoMs(601_400));
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
        let result = m.session().result(0, "2026-01-01".into());
        let timing = &result.group_timings[0];
        assert!(
            (timing.time_to_complete_ms - 500.0).abs() < 1.0,
            "{} ms to answer",
            timing.time_to_complete_ms
        );
        assert_eq!(timing.char_times, None, "timed across the pause");

        // Left as it was, it stands as it was timed, before the pause.
        let mut m = heard_kmr();
        let _ = m.apply(typed(0, "KMR"), MonoMs(1_400));
        let _ = m.apply(SessionEvent::Pause, MonoMs(1_500));
        let auto = m.apply(SessionEvent::Resume, MonoMs(601_500));
        let _ = fire(&mut m, &auto, 601_500 + u64::from(AUTO_CONFIRM_DELAY_MS));
        let result = m.session().result(0, "2026-01-01".into());
        let timing = &result.group_timings[0];
        assert!(
            (timing.time_to_complete_ms - 600.0).abs() < 1.0,
            "{} ms to answer",
            timing.time_to_complete_ms
        );
    }

    /// A pause while an answer typed in full waits to confirm itself sends
    /// nothing again: resumed, the answer confirms itself as it was about
    /// to, without Enter.
    #[test]
    fn a_complete_answer_paused_confirms_itself_once_resumed() {
        let mut m = start_with(settings(2, false, 10.0), script(&["KM", "UK"], 1));
        let _ = play_out(&mut m, 0, MonoMs(500));
        let _ = m.apply(typed(0, "KM"), MonoMs(600));
        let _ = m.apply(SessionEvent::Pause, MonoMs(700));
        let auto = m.apply(SessionEvent::Resume, MonoMs(9_000));
        assert_eq!(timer(&auto).1, AUTO_CONFIRM_DELAY_MS);
        let next = fire(&mut m, &auto, 9_000 + u64::from(AUTO_CONFIRM_DELAY_MS));
        assert!(next.contains(&SessionEffect::StopAudio), "{next:?}");
        assert_eq!(m.phase(), SessionPhase::InterGroupGap { next: 1 });
        // And typed along while the group was still being sent, the same.
        let mut m = start_with(settings(2, false, 10.0), script(&["KM", "UK"], 1));
        let _ = m.apply(typed(0, "KM"), MonoMs(300));
        let _ = m.apply(SessionEvent::Pause, MonoMs(400));
        assert_eq!(
            m.phase(),
            SessionPhase::Paused {
                index: 0,
                why: PauseWhy::Hidden
            }
        );
        let auto = m.apply(SessionEvent::Resume, MonoMs(9_000));
        assert_eq!(timer(&auto).1, AUTO_CONFIRM_DELAY_MS);
        assert!(!auto.iter().any(|e| matches!(e, SessionEffect::Play { .. })));
    }

    /// A send a pause cuts short counts as a hearing once its first
    /// character has sounded: the group goes out again from the start, and
    /// is scored as heard that once more. One cut before anything of it
    /// sounded gave nothing away, and costs nothing. Either way the answer
    /// is timed from the send heard whole after the pause.
    #[test]
    fn a_send_a_pause_cuts_counts_once_its_first_character_sounded() {
        let sends = |cut: u64| {
            let mut m = start_with(settings(1, false, 10.0), script(&["KMR"], 1));
            let _ = m.apply(kmr_started(0), MonoMs(0));
            let _ = m.apply(SessionEvent::Pause, MonoMs(cut));
            let again = m.apply(SessionEvent::Resume, MonoMs(10_000));
            assert!(
                again
                    .iter()
                    .any(|e| matches!(e, SessionEffect::Play { .. })),
                "{again:?}"
            );
            let _ = m.apply(kmr_started(10_000), MonoMs(10_000));
            let _ = m.apply(kmr_ended(), MonoMs(10_800));
            let _ = m.apply(typed(0, "KMR"), MonoMs(11_000));
            let _ = m.apply(SessionEvent::Confirm, MonoMs(11_000));
            let timing = &m.session().result(0, "2026-10-30".into()).group_timings[0];
            assert_eq!(timing.time_to_complete_ms, 300.0, "cut at {cut}");
            timing.sends
        };
        assert_eq!(sends(100), 1, "cut before its first character ended");
        assert_eq!(sends(300), 2, "cut after it");
    }

    /// End session while paused part way through a group answers what was
    /// typed as it stood at the pause: timed from the end of the send heard
    /// whole before it, not across the pause, and heard as often as it had
    /// been by then, the send the pause cut included.
    #[test]
    fn ending_a_session_paused_part_way_through_a_group_answers_it_as_at_the_pause() {
        let mut m = start_with(settings(1, false, 10.0), script(&["KMR"], 2));
        let _ = m.apply(kmr_started(0), MonoMs(0));
        let gap = m.apply(kmr_ended(), MonoMs(800));
        let _ = fire(&mut m, &gap, 1_300);
        let _ = m.apply(kmr_started(1_400), MonoMs(1_300));
        let _ = m.apply(typed(0, "KM"), MonoMs(1_500));
        let _ = m.apply(SessionEvent::Pause, MonoMs(1_700));
        let effects = m.apply(SessionEvent::FinishNow, MonoMs(601_700));
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
        let timing = &m.session().result(0, "2026-10-30".into()).group_timings[0];
        assert_eq!((timing.sends, timing.time_to_complete_ms), (2, 900.0));
    }

    /// End session in the count down a resume brings, before the group
    /// heard in part goes out again, answers what was typed before the
    /// pause timed from the end of the send heard whole before it, less the
    /// pause; as does End session in a second pause taken in that count
    /// down, as at that pause.
    #[test]
    fn ending_a_session_in_the_count_down_after_a_pause_leaves_the_pause_out() {
        let paused = || {
            let mut ask = settings(1, false, 10.0);
            ask.playback.countdown_sec = 2;
            let (mut m, effects) =
                SessionMachine::start(SessionId::new(7), MonoMs(0), ask, script(&["KMR"], 2));
            let effects = fire(&mut m, &effects, 1_000);
            let _ = fire(&mut m, &effects, 2_000);
            let _ = m.apply(kmr_started(2_000), MonoMs(2_000));
            let _ = m.apply(kmr_ended(), MonoMs(2_800));
            let _ = m.apply(typed(0, "KM"), MonoMs(3_000));
            let _ = m.apply(SessionEvent::Pause, MonoMs(3_100));
            let _ = m.apply(SessionEvent::Resume, MonoMs(603_100));
            assert_eq!(m.phase(), SessionPhase::Countdown { remaining: 2 });
            m
        };
        let timing = |m: &SessionMachine<Script>| {
            let timing = &m.session().result(0, "2026-10-30".into()).group_timings[0];
            (timing.sends, timing.time_to_complete_ms)
        };

        let mut m = paused();
        let effects = m.apply(SessionEvent::FinishNow, MonoMs(604_000));
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
        assert_eq!(timing(&m), (1, 1_200.0), "300 before the pause, 900 after");

        let mut m = paused();
        let _ = m.apply(SessionEvent::Pause, MonoMs(603_600));
        let effects = m.apply(SessionEvent::FinishNow, MonoMs(1_203_600));
        assert!(effects.contains(&SessionEffect::PersistAndShowResults));
        assert_eq!(timing(&m), (1, 800.0), "300 before the pause, 500 after");
    }

    /// The count down a resume brings says which group it leads to: the
    /// first, one not heard yet, or one heard in part and sent again.
    #[test]
    fn a_count_down_says_which_group_it_leads_to() {
        let mut ask = settings(2, false, 10.0);
        ask.playback.countdown_sec = 1;
        let (mut m, effects) = SessionMachine::start(
            SessionId::new(7),
            MonoMs(0),
            ask.clone(),
            script(&["KM", "UK"], 1),
        );
        assert_eq!(m.session().view().countdown_to, CountdownTo::First);
        let _ = fire(&mut m, &effects, 1_000);
        let start = SendStart {
            char_wpm: 20.0,
            effective_wpm: 18.0,
            output_at: MonoMs(1_000),
            char_ends_ms: Vec::new(),
            first_char_ms: 0,
            latency_ms: 0,
        };
        let _ = m.apply(
            SessionEvent::PlaybackStarted { index: 0, start },
            MonoMs(1_000),
        );
        let _ = m.apply(SessionEvent::Pause, MonoMs(1_100));
        let _ = m.apply(SessionEvent::Resume, MonoMs(2_000));
        assert_eq!(m.session().view().countdown_to, CountdownTo::Again);
        let (mut m, effects) =
            SessionMachine::start(SessionId::new(7), MonoMs(0), ask, script(&["KM", "UK"], 1));
        let _ = fire(&mut m, &effects, 1_000);
        let _ = play_out(&mut m, 0, MonoMs(1_500));
        let _ = m.apply(typed(0, "KM"), MonoMs(1_600));
        let _ = m.apply(SessionEvent::Confirm, MonoMs(1_700));
        assert_eq!(m.phase(), SessionPhase::InterGroupGap { next: 1 });
        let _ = m.apply(SessionEvent::Pause, MonoMs(1_800));
        let _ = m.apply(SessionEvent::Resume, MonoMs(3_000));
        assert_eq!(m.phase(), SessionPhase::Countdown { remaining: 1 });
        assert_eq!(m.session().view().countdown_to, CountdownTo::Next);
    }

    /// An Enter with nothing typed, straight after a group starts, is taken
    /// for one meant for the group before — a key held down and repeating —
    /// not a skip; once the group has been going a moment it skips it.
    #[test]
    fn an_empty_enter_straight_after_a_group_starts_skips_nothing() {
        let mut m = start_with(settings(2, false, 10.0), script(&["KM", "UK"], 1));
        let _ = play_out(&mut m, 0, MonoMs(500));
        let _ = m.apply(typed(0, "KM"), MonoMs(600));
        let gap = m.apply(SessionEvent::Confirm, MonoMs(700));
        let _ = fire(&mut m, &gap, 1_000);
        assert_eq!(m.phase(), SessionPhase::Playing { index: 1 });
        assert!(m.apply(SessionEvent::Confirm, MonoMs(1_100)).is_empty());
        assert_eq!(m.phase(), SessionPhase::Playing { index: 1 });
        let skipped = m.apply(
            SessionEvent::Confirm,
            MonoMs(1_000 + EMPTY_CONFIRM_GUARD_MS),
        );
        assert!(skipped.contains(&SessionEffect::PersistAndShowResults));
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

    /// A send of `KMR` starting to go out at `at`, its characters ending
    /// 200, 500 and 800 ms into it.
    fn kmr_started(at: u64) -> SessionEvent {
        let start = SendStart {
            char_wpm: 20.0,
            effective_wpm: 18.0,
            output_at: MonoMs(at),
            char_ends_ms: vec![200, 500, 800],
            first_char_ms: 0,
            latency_ms: 0,
        };
        SessionEvent::PlaybackStarted { index: 0, start }
    }

    /// A send of `KMR` played to its end.
    fn kmr_ended() -> SessionEvent {
        SessionEvent::PlaybackEnded {
            index: 0,
            duration_sec: 0.8,
        }
    }

    /// `KMR` sent twice with the box open, the first send from 0 to 800 and
    /// the second from 2000 to 2800, typed whole at `at` and left to confirm
    /// itself: the session it makes.
    fn kmr_twice_typed_at(at: u64) -> SessionResult {
        let mut m = start_with(settings(1, false, 10.0), script(&["KMR"], 2));
        let _ = m.apply(kmr_started(0), MonoMs(0));
        let gap = m.apply(kmr_ended(), MonoMs(800));
        if at >= 1_900 {
            let _ = fire(&mut m, &gap, 1_900);
            let _ = m.apply(kmr_started(2_000), MonoMs(1_900));
        }
        if at >= 2_800 {
            let _ = m.apply(kmr_ended(), MonoMs(2_800));
        }
        let auto = m.apply(typed(0, "KMR"), MonoMs(at));
        let _ = fire(&mut m, &auto, at + u64::from(AUTO_CONFIRM_DELAY_MS));
        assert!(m.session().any_confirmed(), "typed at {at}");
        m.session().result(0, "2026-10-30".into())
    }

    /// Every hearing of a group before its answer costs it, as hearing a
    /// call again does in RufzXP. Typed along its second send once a
    /// character of it has sounded, it was heard twice, and is timed from
    /// the end of the first, not taken as instant; in the gap, or before
    /// the second send gave anything away, it was heard once, timed the
    /// same. Typing along a repeat earns no more than waiting for its end.
    #[test]
    fn every_send_heard_before_the_answer_costs_it() {
        let sends_and_time = |at| {
            let timing = &kmr_twice_typed_at(at).group_timings[0];
            (timing.sends, timing.time_to_complete_ms)
        };
        assert_eq!(sends_and_time(1_400), (1, 600.0), "in the gap");
        assert_eq!(sends_and_time(2_100), (1, 1_300.0), "nothing of it heard");
        assert_eq!(sends_and_time(2_300), (2, 1_500.0), "along the second");
        assert_eq!(sends_and_time(3_000), (2, 300.0), "after the second");
        let (along, after) = (kmr_twice_typed_at(2_300), kmr_twice_typed_at(3_000));
        assert!(
            along.score <= after.score,
            "{} typed along against {} after",
            along.score,
            after.score
        );
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

    /// A send that stops before its end — its sound failed — pauses the
    /// session, as a run is paused, rather than end it: nothing answered is
    /// thrown away, and Resume sends the group again from the start. It
    /// says why it is paused, so the screen does not blame a hidden page.
    #[test]
    fn a_send_stopping_pauses_the_session_and_resume_sends_it_again() {
        // Nothing answered yet: nothing is thrown away either.
        let mut m = machine();
        let effects = m.apply(SessionEvent::PlaybackStopped { index: 0 }, MonoMs(10));
        assert_eq!(effects, vec![SessionEffect::ReleaseAudio]);
        assert_eq!(
            m.phase(),
            SessionPhase::Paused {
                index: 0,
                why: PauseWhy::Sound
            }
        );
        let view = m.session().view();
        assert!(view.paused && view.sound_stopped && view.locked);
        let effects = m.apply(SessionEvent::Resume, MonoMs(5_000));
        assert_eq!(
            effects,
            vec![SessionEffect::Play {
                index: 0,
                text: "KM".into()
            }]
        );
        assert!(!m.session().view().sound_stopped);

        // One group answered: the next one stopping holds the session on it.
        let mut m = machine();
        let _ = play_out(&mut m, 0, MonoMs(200));
        let gap = m.apply(SessionEvent::Confirm, MonoMs(300));
        let _ = fire(&mut m, &gap, 800);
        assert!(matches!(m.phase(), SessionPhase::Playing { index: 1 }));
        let effects = m.apply(SessionEvent::PlaybackStopped { index: 1 }, MonoMs(900));
        assert_eq!(effects, vec![SessionEffect::ReleaseAudio]);
        assert_eq!(
            m.phase(),
            SessionPhase::Paused {
                index: 1,
                why: PauseWhy::Sound
            }
        );
        assert!(m.session().any_confirmed());
        // Hidden as well, it is still the sound that holds it.
        assert!(m.apply(SessionEvent::Pause, MonoMs(950)).is_empty());
        assert!(m.session().view().sound_stopped);
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
            first_char_ms: 0,
            latency_ms: 0,
        };
        let _ = m.apply(
            SessionEvent::PlaybackStarted { index: 0, start },
            MonoMs(10),
        );
        let typed = SessionEvent::Input {
            index: 0,
            text: "KM".into(),
            keys: Keys {
                at: vec![Some(MonoMs(450)), Some(MonoMs(800))],
                corrected: Vec::new(),
            },
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

    /// A first send held up on its way cannot be timed, whether it was
    /// heard out or the group was answered and the send stopped before the
    /// news came; its speed is kept all the same.
    #[test]
    fn a_send_held_up_on_its_way_is_not_timed() {
        for answered_first in [false, true] {
            let mut m = start_with(settings(1, false, 10.0), script(&["K"], 1));
            let start = SendStart {
                char_wpm: 22.0,
                effective_wpm: 22.0,
                output_at: MonoMs(20),
                char_ends_ms: vec![150],
                first_char_ms: 0,
                latency_ms: 0,
            };
            let _ = m.apply(
                SessionEvent::PlaybackStarted { index: 0, start },
                MonoMs(10),
            );
            if answered_first {
                let _ = m.apply(typed(0, "K"), MonoMs(400));
                let effects = m.apply(SessionEvent::Confirm, MonoMs(420));
                assert!(effects.contains(&SessionEffect::StopAudio));
                let _ = m.apply(SessionEvent::PlaybackHeldUp { index: 0 }, MonoMs(450));
                let _ = m.apply(SessionEvent::PlaybackStopped { index: 0 }, MonoMs(450));
            } else {
                let _ = m.apply(SessionEvent::PlaybackHeldUp { index: 0 }, MonoMs(900));
                let ended = SessionEvent::PlaybackEnded {
                    index: 0,
                    duration_sec: 0.15,
                };
                let _ = m.apply(ended, MonoMs(900));
                let _ = m.apply(typed(0, "K"), MonoMs(1_200));
                let _ = m.apply(SessionEvent::Confirm, MonoMs(1_200));
            }
            let timing = &m.session().result(1_300, "2026-09-01".into()).group_timings[0];
            assert_eq!(
                (timing.char_wpm, timing.char_times.clone()),
                (Some(22.0), None),
                "answered first: {answered_first}"
            );
        }
    }

    /// A send some of which went unheard, its sound skipped by a browser
    /// fallen behind its clock, is no group heard: the answer window does
    /// not open on it, nothing can be answered, and once the send ends the
    /// group goes out again from the start, after the pause between groups
    /// with no count down asked for, the box locked meanwhile as it is
    /// while the group is sent. Heard whole the second time, it is answered
    /// and scored once, from that send: a sound that failed costs only time.
    /// A send only held up, heard late but whole, is answered as ever.
    #[test]
    fn a_group_heard_in_part_is_sent_again_not_answered() {
        let mut m = start_with(settings(2, true, 10.0), script(&["KM", "UK"], 1));
        let start = |at: u64| SendStart {
            char_wpm: 20.0,
            effective_wpm: 18.0,
            output_at: MonoMs(at),
            char_ends_ms: vec![300, 500],
            first_char_ms: 0,
            latency_ms: 0,
        };
        let _ = m.apply(
            SessionEvent::PlaybackStarted {
                index: 0,
                start: start(20),
            },
            MonoMs(10),
        );
        let missed = m.apply(SessionEvent::PlaybackMissed { index: 0 }, MonoMs(300));
        assert!(missed.is_empty(), "the send plays on: {missed:?}");
        let _ = m.apply(SessionEvent::PlaybackHeldUp { index: 0 }, MonoMs(520));
        let again = m.apply(ended(0), MonoMs(520));
        assert_eq!(m.phase(), SessionPhase::RepeatGap { index: 0 });
        let (_, gap_ms) = timer(&again);
        assert_eq!(gap_ms, m.group_gap_ms(1), "the pause between groups");
        let view = m.session().view();
        assert!(view.locked && view.playing, "{view:?}");
        assert!(m.apply(typed(0, "KM"), MonoMs(600)).is_empty());
        assert!(m.apply(SessionEvent::Confirm, MonoMs(700)).is_empty());
        assert!(m.source.learned.is_empty(), "nothing answered");

        let resent = fire(&mut m, &again, 520 + u64::from(gap_ms));
        assert_eq!(
            resent,
            vec![SessionEffect::Play {
                index: 0,
                text: "KM".into()
            }]
        );
        assert_eq!(m.phase(), SessionPhase::Playing { index: 0 });
        let _ = m.apply(
            SessionEvent::PlaybackStarted {
                index: 0,
                start: start(2_000),
            },
            MonoMs(1_990),
        );
        let window = m.apply(ended(0), MonoMs(2_500));
        assert_eq!(m.phase(), SessionPhase::AwaitingAnswer { index: 0 });
        assert_eq!(timer(&window).1, 10_000, "the answer window, at last");
        let _ = m.apply(typed(0, "KM"), MonoMs(2_800));
        let next = m.apply(SessionEvent::Confirm, MonoMs(2_900));
        assert_eq!(m.source.learned.len(), 1, "answered once");
        let timing = &m.session().result(3_000, "2026-10-29".into()).group_timings[0];
        assert_eq!(timing.sends, 1, "heard whole once");
        assert_eq!(
            timing.time_to_complete_ms, 300.0,
            "from the send heard whole"
        );
        assert_eq!(timing.char_times, None, "sent twice: not timed");

        // Held up alone, heard whole: answered as ever.
        let _ = fire(&mut m, &next, 4_000);
        let _ = m.apply(
            SessionEvent::PlaybackStarted {
                index: 1,
                start: start(4_010),
            },
            MonoMs(4_000),
        );
        let _ = m.apply(SessionEvent::PlaybackHeldUp { index: 1 }, MonoMs(4_900));
        let _ = m.apply(ended(1), MonoMs(4_900));
        assert_eq!(m.phase(), SessionPhase::AwaitingAnswer { index: 1 });
    }

    /// A group heard in part goes out again after the count down the
    /// settings ask for, as after a pause, its box locked meanwhile, and
    /// the count down says it is the same group again.
    #[test]
    fn a_group_heard_in_part_is_counted_down_to_again() {
        let mut ask = settings(1, false, 10.0);
        ask.playback.countdown_sec = 2;
        let (mut m, effects) =
            SessionMachine::start(SessionId::new(7), MonoMs(0), ask, script(&["KM"], 2));
        let effects = fire(&mut m, &effects, 1_000);
        let _ = fire(&mut m, &effects, 2_000);
        // The first send heard whole, the second in part.
        let gap = play_out(&mut m, 0, MonoMs(2_500));
        let _ = fire(&mut m, &gap, 3_000);
        let _ = m.apply(SessionEvent::PlaybackMissed { index: 0 }, MonoMs(3_200));
        let counted = m.apply(ended(0), MonoMs(3_500));
        assert_eq!(m.phase(), SessionPhase::Countdown { remaining: 2 });
        let view = m.session().view();
        assert!(view.locked, "nothing typed while counting down");
        assert_eq!(view.countdown_to, crate::session::CountdownTo::Again);
        let effects = fire(&mut m, &counted, 4_500);
        let effects = fire(&mut m, &effects, 5_500);
        assert_eq!(
            effects,
            vec![SessionEffect::Play {
                index: 0,
                text: "KM".into()
            }]
        );
        // Both sends of it again, from the first.
        assert_eq!(m.session().view().repeat_done, 0);
        let gap = play_out(&mut m, 0, MonoMs(6_000));
        assert_eq!(m.phase(), SessionPhase::RepeatGap { index: 0 });
        let _ = fire(&mut m, &gap, 6_500);
        let _ = play_out(&mut m, 0, MonoMs(7_000));
        assert_eq!(m.phase(), SessionPhase::AwaitingAnswer { index: 0 });
    }

    /// Only what the send that ends missed sends the group again: an
    /// attempt that failed after missing some, tried again and heard whole,
    /// is answered; and a group answered in full as it was sent, its
    /// confirm waiting, is not sent again, as a pause leaves it.
    #[test]
    fn a_group_heard_whole_after_all_is_not_sent_again() {
        let mut m = machine();
        let _ = m.apply(SessionEvent::PlaybackMissed { index: 0 }, MonoMs(200));
        let _ = play_out(&mut m, 0, MonoMs(600));
        assert_eq!(m.phase(), SessionPhase::AwaitingAnswer { index: 0 });

        let mut open = start_with(settings(1, false, 10.0), script(&["KM"], 1));
        let _ = open.apply(typed(0, "KM"), MonoMs(300));
        let _ = open.apply(SessionEvent::PlaybackMissed { index: 0 }, MonoMs(350));
        let _ = open.apply(ended(0), MonoMs(400));
        assert_eq!(open.phase(), SessionPhase::AwaitingAnswer { index: 0 });
    }

    /// A send some of which went unheard is no hearing, however it ends,
    /// even once its first character was due to have sounded: where the
    /// skip fell is not known, and a sound that failed costs only time.
    /// Sent again, and paused in the pause before it goes out again; cut
    /// by a pause; or typed along: each is scored as heard once.
    #[test]
    fn a_send_some_of_which_went_unheard_is_no_hearing() {
        let heard_again = |m: &mut SessionMachine<Script>, at: u64| {
            let again = m.apply(SessionEvent::Resume, MonoMs(at));
            assert!(
                again
                    .iter()
                    .any(|e| matches!(e, SessionEffect::Play { .. })),
                "{again:?}"
            );
            let _ = m.apply(kmr_started(at), MonoMs(at));
            let _ = m.apply(kmr_ended(), MonoMs(at + 800));
            let _ = m.apply(typed(0, "KMR"), MonoMs(at + 1_000));
            let _ = m.apply(SessionEvent::Confirm, MonoMs(at + 1_000));
        };
        let sends = |m: &SessionMachine<Script>| {
            let timing = &m.session().result(0, "2026-10-30".into()).group_timings[0];
            (timing.sends, timing.time_to_complete_ms)
        };

        // Skipped once its first character had sounded, then sent again.
        let mut m = start_with(settings(1, false, 10.0), script(&["KMR"], 1));
        let _ = m.apply(kmr_started(0), MonoMs(0));
        let _ = m.apply(SessionEvent::PlaybackMissed { index: 0 }, MonoMs(600));
        let _ = m.apply(kmr_ended(), MonoMs(800));
        assert_eq!(m.phase(), SessionPhase::RepeatGap { index: 0 });
        let _ = m.apply(SessionEvent::Pause, MonoMs(900));
        heard_again(&mut m, 10_000);
        assert_eq!(sends(&m), (1, 300.0), "sent again");

        // Skipped from the start, then cut by a pause once its first
        // character was due.
        let mut m = start_with(settings(1, false, 10.0), script(&["KMR"], 1));
        let _ = m.apply(kmr_started(0), MonoMs(0));
        let _ = m.apply(SessionEvent::PlaybackMissed { index: 0 }, MonoMs(100));
        let _ = m.apply(SessionEvent::Pause, MonoMs(600));
        heard_again(&mut m, 10_000);
        assert_eq!(sends(&m), (1, 300.0), "cut by a pause");

        // Its second send skipped, and typed along once its first character
        // was due: timed from the first.
        let mut m = start_with(settings(1, false, 10.0), script(&["KMR"], 2));
        let _ = m.apply(kmr_started(0), MonoMs(0));
        let gap = m.apply(kmr_ended(), MonoMs(800));
        let _ = fire(&mut m, &gap, 1_900);
        let _ = m.apply(kmr_started(2_000), MonoMs(1_900));
        let _ = m.apply(SessionEvent::PlaybackMissed { index: 0 }, MonoMs(2_100));
        let auto = m.apply(typed(0, "KMR"), MonoMs(2_300));
        let _ = fire(&mut m, &auto, 2_300 + u64::from(AUTO_CONFIRM_DELAY_MS));
        assert!(m.session().any_confirmed());
        assert_eq!(sends(&m), (1, 1_500.0), "typed along");
    }
}
