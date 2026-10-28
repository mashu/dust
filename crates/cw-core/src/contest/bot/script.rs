//! What the tests work runs with: calls handed out as scripted, the
//! settings a run starts from, operators of every habit, and keys pressed
//! by hand. Hear a QSO needs none of it.

use std::collections::VecDeque;

use super::{Operator, Runtime, work_until};
use crate::callsign::{CallSpec, GeneratedCall, country_of};
use crate::contest::machine::{ContestEffect, ContestEvent, OpKey};
use crate::contest::turn::Turn;
use crate::contest::{CallSource, Fields};
use crate::session::{Answered, MonoMs};
use crate::settings::{CharSetMode, TrainingSettings};

/// Hands out the calls it is given, then `W1AW`, `W2AW`… for ever, and
/// remembers what it was taught, and what of it taught your copy speed. It
/// never chooses a caller's speed.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScriptCalls {
    calls: VecDeque<String>,
    handed: u32,
    pub(crate) learned: Vec<(String, String)>,
    pub(crate) learned_speed: Vec<(String, String)>,
}

impl ScriptCalls {
    pub(crate) fn new(calls: &[&str]) -> Self {
        Self {
            calls: calls.iter().map(|call| (*call).to_string()).collect(),
            ..Self::default()
        }
    }
}

impl CallSource for ScriptCalls {
    fn next_call(
        &mut self,
        _: &TrainingSettings,
        _: CallSpec,
        _: &dyn Fn(&str) -> bool,
    ) -> GeneratedCall {
        self.handed += 1;
        let text = self
            .calls
            .pop_front()
            .unwrap_or_else(|| format!("W{}AW", self.handed % 10));
        GeneratedCall {
            country: country_of(&text).unwrap_or_default(),
            digit: text.chars().rfind(char::is_ascii_digit).unwrap_or('0'),
            host: None,
            marker: None,
            text,
        }
    }

    fn learn(&mut self, answered: Answered<'_>) {
        self.learned
            .push((answered.sent.to_string(), answered.received.to_string()));
    }

    fn learn_speed(&mut self, answered: Answered<'_>) {
        self.learned_speed
            .push((answered.sent.to_string(), answered.received.to_string()));
    }
}

/// Settings for a run: WPX as DL1ABC, called MAT, ten minutes, no
/// countdown, the speed ranges as they come.
pub(crate) fn default_run_settings() -> TrainingSettings {
    let mut settings = TrainingSettings::default();
    settings.curriculum.char_set_mode = CharSetMode::Contest;
    settings.contest.my_call = "DL1ABC".to_string();
    settings.contest.my_name = "MAT".to_string();
    settings.playback.countdown_sec = 0;
    settings.clamp()
}

/// The same, every speed `wpm`.
pub(crate) fn run_settings(wpm: f64) -> TrainingSettings {
    let mut settings = default_run_settings();
    settings.playback.char_wpm_min = wpm;
    settings.playback.char_wpm_max = wpm;
    settings.playback.effective_wpm_min = wpm;
    settings.playback.effective_wpm_max = wpm;
    settings.clamp()
}

impl<S: CallSource> Runtime<S> {
    /// Press `key` now, with `call` and `exch` in the fields, both typed
    /// just now.
    pub(crate) fn key(&mut self, key: OpKey, call: &str, exch: &str) -> Vec<ContestEffect> {
        let fields = Fields {
            call: call.to_string(),
            exch: exch.to_string(),
            call_edited: MonoMs(self.now),
            exch_complete: (!exch.is_empty()).then_some(MonoMs(self.now)),
            ..Fields::default()
        };
        self.apply(ContestEvent::Key {
            key,
            fields: Box::new(fields),
        })
    }

    /// Deliver everything owed until the turn on the air has ended.
    pub(crate) fn finish_turn(&mut self) {
        while self.playing.is_some() && !self.over && self.step() {}
    }

    /// The sound of turn `id` is to fail rather than end: the player never
    /// says it ended, and says it failed instead, whenever the test does.
    pub(crate) fn sound_fails(&mut self, id: u64) {
        self.due
            .retain(|(_, event)| *event != ContestEvent::TurnEnded { id });
    }

    /// The last turn played.
    pub(crate) fn last_turn(&self) -> &Turn {
        self.turns.last().expect("a turn has been played")
    }
}

/// How an operator works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Habit {
    /// Copies everything, keys at once.
    Perfect,
    /// Copies everything, a second and a half for each key typed.
    HumanPaced,
    /// Copies everything but types one digit of each number wrong.
    DigitOff,
    /// Asks for each call with a partial first: `K5?`.
    Unsure,
}

/// How long a human-paced operator takes over each key it types.
const HUMAN_KEY_MS: u64 = 1_500;

impl Operator {
    pub(crate) fn new(habit: Habit) -> Self {
        Self {
            per_key_ms: if habit == Habit::HumanPaced {
                HUMAN_KEY_MS
            } else {
                0
            },
            digit_off: habit == Habit::DigitOff,
            unsure: habit == Habit::Unsure,
            ..Self::perfect()
        }
    }
}

/// Let `operator` work the run until it is over: each time the air falls
/// quiet it reads what it heard and keys what comes next. With `pauses`, it
/// pauses at each first time and resumes at each second.
pub(crate) fn work<S: CallSource>(
    runtime: &mut Runtime<S>,
    operator: &mut Operator,
    pauses: &[(u64, u64)],
) {
    assert!(
        work_until(runtime, operator, pauses, |_| false),
        "the run never ended"
    );
}
