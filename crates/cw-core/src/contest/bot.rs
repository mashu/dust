//! Operators that work a run as a person would, and the runtime they work
//! it through.
//!
//! The operators hear the turns and never peek (§13 of the design): what a
//! caller sent reaches them only as the text of a [`ContestEffect::PlayTurn`]
//! segment, once that segment has been heard whole. The [`Runtime`] does
//! what the app's runtime does with effects: it plays each turn for exactly
//! its planned length, fires each timer when it is due, and says when a turn
//! started, ended or was cancelled. The machine's tests drive runs through
//! it too, and the soak copies callers as the operators do ([`copy_of`]).

use std::collections::VecDeque;

use super::machine::{ContestEffect, ContestEvent, ContestMachine, OpKey, PauseWhy};
use super::turn::{Sender, Turn, plan_turn};
use super::{CallSource, Field, Fields};
use crate::callsign::{CallSpec, GeneratedCall, country_of, parse_callsign};
use crate::session::{Answered, MonoMs};
use crate::settings::{CharSetMode, TrainingSettings};

/// Hands out the calls it is given, then `W1AW`, `W2AW`… for ever, and
/// remembers what it was taught.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScriptCalls {
    calls: VecDeque<String>,
    handed: u32,
    pub(crate) learned: Vec<(String, String)>,
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

/// What the app's runtime does with a machine's effects, on a clock of its
/// own: each turn plays for its planned length, each timer fires when due.
pub(crate) struct Runtime<S> {
    pub(crate) machine: ContestMachine<S>,
    settings: TrainingSettings,
    /// Now, in milliseconds on the monotonic clock.
    pub(crate) now: u64,
    /// What the runtime owes the machine, each with when it is due.
    due: Vec<(u64, ContestEvent)>,
    /// The turn playing, and when it started.
    playing: Option<(u64, Turn)>,
    /// Every turn put on the air, in order.
    pub(crate) turns: Vec<Turn>,
    /// Every effect, in order.
    pub(crate) effects: Vec<ContestEffect>,
    /// Callers' texts heard whole, for an operator to read.
    pub(crate) heard: Vec<String>,
    /// The fields were cleared since an operator last looked.
    pub(crate) cleared: bool,
    /// The run reached its results, its nothing-kept notice, or home.
    pub(crate) over: bool,
}

impl<S: CallSource> Runtime<S> {
    pub(crate) fn start(seed: u64, settings: TrainingSettings, source: S) -> Self {
        let (machine, effects) = ContestMachine::start(seed, MonoMs(0), settings.clone(), source);
        let mut runtime = Self {
            machine,
            settings,
            now: 0,
            due: Vec::new(),
            playing: None,
            turns: Vec::new(),
            effects: Vec::new(),
            heard: Vec::new(),
            cleared: false,
            over: false,
        };
        runtime.carry_out(effects);
        runtime
    }

    /// Apply `event` now, and carry out what comes of it.
    pub(crate) fn apply(&mut self, event: ContestEvent) -> Vec<ContestEffect> {
        if let ContestEvent::TurnEnded { id } = event
            && self.playing.as_ref().is_some_and(|(_, turn)| turn.id == id)
            && let Some((_, turn)) = self.playing.take()
        {
            self.heard.extend(callers_text(&turn, f64::INFINITY));
        }
        let effects = self.machine.apply(event, MonoMs(self.now));
        self.carry_out(effects.clone());
        effects
    }

    /// Press `key` now, with `call` and `exch` in the fields, both typed
    /// just now.
    pub(crate) fn key(&mut self, key: OpKey, call: &str, exch: &str) -> Vec<ContestEffect> {
        let fields = Fields {
            call: call.to_string(),
            exch: exch.to_string(),
            call_edited: MonoMs(self.now),
            exch_complete: (!exch.is_empty()).then_some(MonoMs(self.now)),
        };
        self.apply(ContestEvent::Key { key, fields })
    }

    /// When the next thing owed is due.
    pub(crate) fn next_due(&self) -> Option<u64> {
        self.due.iter().map(|(at, _)| *at).min()
    }

    /// Deliver the next thing owed, at its time.
    pub(crate) fn step(&mut self) -> bool {
        let Some(next) = self.next_due() else {
            return false;
        };
        let Some(position) = self.due.iter().position(|(at, _)| *at == next) else {
            return false;
        };
        let (_, event) = self.due.remove(position);
        self.now = self.now.max(next);
        self.apply(event);
        true
    }

    /// Move on to `at`, delivering everything due by then.
    pub(crate) fn run_to(&mut self, at: u64) {
        while !self.over && self.next_due().is_some_and(|next| next <= at) {
            self.step();
        }
        self.now = self.now.max(at);
    }

    /// Deliver everything owed until the turn on the air has ended.
    pub(crate) fn finish_turn(&mut self) {
        while self.playing.is_some() && !self.over && self.step() {}
    }

    /// The id of the turn on the air.
    pub(crate) fn playing_id(&self) -> Option<u64> {
        self.playing.as_ref().map(|(_, turn)| turn.id)
    }

    /// The last turn played.
    pub(crate) fn last_turn(&self) -> &Turn {
        self.turns.last().expect("a turn has been played")
    }

    /// Everything heard whole since the last look, taken.
    pub(crate) fn take_heard(&mut self) -> Vec<String> {
        std::mem::take(&mut self.heard)
    }

    fn carry_out(&mut self, effects: Vec<ContestEffect>) {
        for effect in effects {
            match &effect {
                ContestEffect::PlayTurn(turn) => {
                    self.leave_air();
                    let planned = plan_turn(turn, &self.settings);
                    let ms = (planned.wanted.duration_sec * 1_000.0).ceil() as u64;
                    let id = turn.id;
                    self.due.push((
                        self.now,
                        ContestEvent::TurnStarted {
                            id,
                            heard_from: MonoMs(self.now),
                        },
                    ));
                    self.due
                        .push((self.now + ms, ContestEvent::TurnEnded { id }));
                    self.playing = Some((self.now, turn.clone()));
                    self.turns.push(turn.clone());
                }
                ContestEffect::StopAudio | ContestEffect::ReleaseAudio => self.leave_air(),
                ContestEffect::StartTimer { id, ms } => self.due.push((
                    self.now + u64::from(*ms),
                    ContestEvent::TimerFired { id: *id },
                )),
                ContestEffect::ClearFields => self.cleared = true,
                ContestEffect::Focus(_) | ContestEffect::Announce(_) => {}
                ContestEffect::PersistAndShowResults
                | ContestEffect::ShowNothingKept
                | ContestEffect::AbortToHome => self.over = true,
            }
            self.effects.push(effect);
        }
    }

    /// The turn on the air stops: what was heard whole of it by now is
    /// heard, and its send comes back cancelled.
    fn leave_air(&mut self) {
        let Some((started, turn)) = self.playing.take() else {
            return;
        };
        let id = turn.id;
        self.due.retain(|(_, event)| {
            !matches!(
                event,
                ContestEvent::TurnStarted { id: due, .. } | ContestEvent::TurnEnded { id: due }
                    if *due == id
            )
        });
        let into = (self.now - started) as f64 / 1_000.0;
        self.heard.extend(callers_text(&turn, into));
        self.due
            .push((self.now, ContestEvent::TurnCancelled { id }));
    }
}

/// What callers sent in `turn` that had been heard whole `into` seconds
/// after it started.
fn callers_text(turn: &Turn, into: f64) -> Vec<String> {
    turn.segments
        .iter()
        .filter(|segment| segment.who != Sender::You && segment.end_sec <= into)
        .map(|segment| segment.text.clone())
        .collect()
}

/// What a listener copies from a caller's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Copied<'a> {
    /// A caller calling: its call alone, or `DL1ABC DE K5XYZ K5XYZ K`.
    Call(&'a str),
    /// A call put right, `DE K5XYZ` or `K5XYZ K5XYZ`, with the station's
    /// exchange where it sends it after: `DE K5XYZ 5NN 214`.
    Correction(&'a str, Option<String>),
    /// An exchange, as it goes in the Exchange field: a contest's serial,
    /// its last word; an everyday QSO's report and name, `579 BOB`; a
    /// Sprint's serial, name and state, `145 BOB OR`.
    Exchange(String),
}

/// What a caller's `text` gives a listener to type, if anything: how the
/// operators and the soak copy what they hear.
pub(crate) fn copy_of(text: &str) -> Option<Copied<'_>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["DE", call] => Some(Copied::Correction(call, None)),
        [one, two] if one == two && parse_callsign(one).is_some() => {
            Some(Copied::Correction(one, None))
        }
        [call] if parse_callsign(call).is_some() => Some(Copied::Call(call)),
        [_, "DE", call, again, "K"] if call == again => Some(Copied::Call(call)),
        // The call put right, then the exchange.
        ["DE", call, rest @ ..] => {
            exchange_of(rest).map(|exch| Copied::Correction(call, Some(exch)))
        }
        [one, two, rest @ ..] if one == two && parse_callsign(one).is_some() => {
            exchange_of(rest).map(|exch| Copied::Correction(one, Some(exch)))
        }
        _ => exchange_of(&words).map(Copied::Exchange),
    }
}

/// The exchange in a caller's `words`, as it goes in the Exchange field.
fn exchange_of(words: &[&str]) -> Option<String> {
    let after = |word: &str| {
        let at = words.iter().position(|said| *said == word)?;
        words.get(at + 1).copied()
    };
    match (after("UR"), after("NAME")) {
        (Some(rst), Some(name)) => Some(format!("{rst} {name}")),
        _ if words.contains(&"5NN") => words.last().map(|serial| (*serial).to_string()),
        _ => sprint_exchange(words),
    }
}

/// A Sprint's exchange, after your call: its serial, name and state.
fn sprint_exchange(words: &[&str]) -> Option<String> {
    match words {
        [.., nr, name, st]
            if nr.chars().all(|c| c.is_ascii_digit()) && Field::St.canonical(st).is_some() =>
        {
            Some(format!("{nr} {name} {st}"))
        }
        _ => None,
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

/// One operator: what it has typed, and what it means to press next.
#[derive(Clone, Debug)]
pub(crate) struct Operator {
    habit: Habit,
    call: String,
    exch: String,
    /// Characters typed since the last key.
    typing: usize,
    /// The call it asked for with a partial, waiting to hear it again.
    asked: Option<String>,
    /// Keys in a row that brought no answer.
    unanswered: u32,
}

/// How long a human-paced operator takes over each key it types.
const HUMAN_KEY_MS: u64 = 1_500;

impl Operator {
    pub(crate) fn new(habit: Habit) -> Self {
        Self {
            habit,
            call: String::new(),
            exch: String::new(),
            typing: 0,
            asked: None,
            unanswered: 0,
        }
    }

    fn type_call(&mut self, call: &str) {
        if self.call != call {
            self.typing += call.len();
            self.call = call.to_string();
        }
    }

    /// Take in what a caller sent.
    fn hear(&mut self, text: &str) {
        self.unanswered = 0;
        match copy_of(text) {
            Some(Copied::Call(call))
                if self.habit == Habit::Unsure && self.asked.as_deref() != Some(call) =>
            {
                // Not sure of it: ask for it again with what it is sure of.
                self.asked = Some(call.to_string());
                let partial: String = call.chars().take(2).chain(['?']).collect();
                self.type_call(&partial);
            }
            Some(Copied::Call(call)) => self.type_call(call),
            Some(Copied::Correction(call, exch)) => {
                self.type_call(call);
                if let Some(exch) = exch {
                    self.type_exch(exch);
                }
            }
            Some(Copied::Exchange(exch)) => self.type_exch(exch),
            None => {}
        }
    }

    fn type_exch(&mut self, exch: String) {
        let exch = if self.habit == Habit::DigitOff {
            one_digit_off(&exch)
        } else {
            exch
        };
        if self.exch != exch {
            self.typing += exch.len();
            self.exch = exch;
        }
    }

    /// Its next key, with the fields as typed, pressed `at` once it has
    /// typed them.
    fn next_key(&mut self, now: u64) -> (u64, OpKey, Fields) {
        let per_key = if self.habit == Habit::HumanPaced {
            HUMAN_KEY_MS
        } else {
            0
        };
        let typed_at = now + per_key * self.typing as u64;
        self.typing = 0;
        self.unanswered += 1;
        // Twice unanswered: the station has gone. Start again.
        let key = if self.unanswered > 2 {
            self.unanswered = 0;
            self.call.clear();
            self.exch.clear();
            OpKey::Wipe
        } else {
            OpKey::Esm
        };
        let fields = Fields {
            call: self.call.clone(),
            exch: self.exch.clone(),
            call_edited: MonoMs(typed_at),
            exch_complete: (!self.exch.is_empty()).then_some(MonoMs(typed_at)),
        };
        (typed_at + per_key, key, fields)
    }
}

/// `exch` with its last digit one more.
fn one_digit_off(exch: &str) -> String {
    let mut digits: Vec<char> = exch.chars().collect();
    if let Some(last) = digits.iter_mut().rev().find(|c| c.is_ascii_digit()) {
        let digit = last.to_digit(10).unwrap_or(0);
        *last = char::from_digit((digit + 1) % 10, 10).unwrap_or('0');
    }
    digits.into_iter().collect()
}

/// Let `operator` work the run until it is over: each time the air falls
/// quiet it reads what it heard and keys what comes next. With `pauses`, it
/// pauses at each first time and resumes at each second.
pub(crate) fn work<S: CallSource>(
    runtime: &mut Runtime<S>,
    operator: &mut Operator,
    pauses: &[(u64, u64)],
) {
    let mut pauses: VecDeque<(u64, u64)> = pauses.iter().copied().collect();
    let mut paused_until: Option<u64> = None;
    let mut pending: Option<(u64, OpKey, Fields)> = None;
    for _ in 0..100_000 {
        if runtime.over {
            return;
        }
        if runtime.cleared {
            runtime.cleared = false;
            operator.call.clear();
            operator.exch.clear();
            operator.asked = None;
        }
        for text in runtime.take_heard() {
            operator.hear(&text);
        }
        if pending.is_none() && paused_until.is_none() && runtime.playing_id().is_none() {
            pending = Some(operator.next_key(runtime.now));
        }
        let key_at = pending.as_ref().map(|(at, _, _)| *at);
        let pause_at = paused_until.or(pauses.front().map(|(at, _)| *at));
        let next = [runtime.next_due(), key_at, pause_at]
            .into_iter()
            .flatten()
            .min();
        let Some(next) = next else {
            return;
        };
        if pause_at == Some(next) {
            runtime.run_to(next);
            if paused_until.take().is_some() {
                runtime.apply(ContestEvent::Resume);
            } else if let Some((at, until)) = pauses.pop_front() {
                runtime.apply(ContestEvent::Pause {
                    why: PauseWhy::You,
                    at: MonoMs(at),
                });
                paused_until = Some(until);
                pending = None;
            }
        } else if key_at == Some(next) && runtime.next_due().is_none_or(|due| due >= next) {
            runtime.run_to(next);
            if let Some((_, key, fields)) = pending.take() {
                runtime.apply(ContestEvent::Key { key, fields });
            }
        } else {
            runtime.step();
        }
    }
    panic!("the run never ended");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::RowStatus;
    use crate::rng::SplitMix64;
    use crate::sampling::Sampler;
    use crate::score::{GroupCopy, group_points};

    /// A ten-minute run worked by `habit` at `wpm`, through the sampler.
    fn worked(habit: Habit, wpm: f64, seed: u64) -> Runtime<Sampler> {
        let settings = run_settings(wpm);
        let sampler = Sampler::new(&[], &settings, seed);
        let mut runtime = Runtime::start(seed, settings, sampler);
        work(&mut runtime, &mut Operator::new(habit), &[]);
        runtime
    }

    /// How many of the run's QSOs were dupes.
    fn dupes(view: &crate::contest::ContestView) -> u32 {
        let dupes = view
            .log
            .iter()
            .filter(|row| matches!(&row.status, RowStatus::Checked(check) if check.dupe))
            .count();
        u32::try_from(dupes).unwrap_or(u32::MAX)
    }

    /// The length of every turn played whole or in part, in milliseconds:
    /// the run's time, when the operator keys the moment the air falls
    /// quiet.
    fn air_time(runtime: &Runtime<Sampler>, limit_ms: u64) -> u64 {
        let settings = run_settings(25.0);
        let total: u64 = runtime
            .turns
            .iter()
            .map(|turn| (plan_turn(turn, &settings).wanted.duration_sec * 1_000.0).ceil() as u64)
            .sum();
        total.min(limit_ms)
    }

    #[test]
    fn a_perfect_operator_is_verified_throughout_at_the_planned_rate() {
        for seed in [1, 2, 3] {
            let runtime = worked(Habit::Perfect, 25.0, seed);
            let view = runtime.machine.view();
            assert!(view.qsos > 20, "seed {seed}: {} QSOs", view.qsos);
            assert_eq!(view.clean, view.qsos, "seed {seed}");
            let result = runtime.machine.result(0, "2026-10-20".into());
            let run = result
                .contest
                .as_ref()
                .and_then(|log| log.run(result.groups.len()));
            let run = run.expect("the log reads back");
            let score = run.score(&result.groups).expect("WPX scores");
            // Every QSO scores, but a dupe, which scores nothing.
            assert_eq!(
                score.verified.qso_points + dupes(&view),
                view.qsos,
                "seed {seed}"
            );
            // The rate the turns allow: nothing waits between them.
            let planned =
                3_600_000.0 * f64::from(view.qsos) / air_time(&runtime, run.run_ms) as f64;
            let rate = 3_600_000.0 * f64::from(view.qsos) / run.run_ms as f64;
            assert!(
                (rate / planned - 1.0).abs() < 0.05,
                "seed {seed}: {rate:.0} an hour against {planned:.0} planned"
            );
            assert!(
                (180.0..=280.0).contains(&rate),
                "seed {seed}: {rate:.0} an hour"
            );
            // Every copy is right, but the number of a QSO the run's end cut
            // short, which was never logged.
            let mut at = 0;
            for qso in &run.qsos {
                let owned = &result.groups[at..at + qso.g as usize];
                at += qso.g as usize;
                let copied = if qso.k.is_empty() { owned } else { &owned[..1] };
                assert!(copied.iter().all(|group| group.correct), "seed {seed}");
            }
            assert!(
                run.qsos[..run.qsos.len() - 1]
                    .iter()
                    .all(|qso| qso.k.is_empty()),
                "seed {seed}: only the last QSO may be cut short"
            );
            assert!(result.group_timings.iter().all(|timing| timing.sends == 1));
        }
    }

    /// A pile-up of one to three worked perfectly: every QSO logged clean,
    /// and every one scoring but a dupe, at a rate near a single caller's,
    /// though callers miss your exchange and call over you now and then.
    #[test]
    fn a_perfect_operator_works_a_pile_up_clean() {
        for seed in [11, 12] {
            let mut settings = run_settings(25.0);
            settings.contest.contest_style = super::super::RunStyle::Pileup;
            settings.band.stations_max = 3;
            let settings = settings.clamp();
            let sampler = Sampler::new(&[], &settings, seed);
            let mut runtime = Runtime::start(seed, settings, sampler);
            work(&mut runtime, &mut Operator::new(Habit::Perfect), &[]);
            let view = runtime.machine.view();
            assert!(view.qsos > 20, "seed {seed}: {} QSOs", view.qsos);
            assert_eq!(view.clean, view.qsos, "seed {seed}");
            let result = runtime.machine.result(0, "2026-10-20".into());
            let run = result
                .contest
                .as_ref()
                .and_then(|log| log.run(result.groups.len()))
                .expect("the log reads back");
            assert_eq!(run.style, "pileup");
            let score = run.score(&result.groups).expect("WPX scores");
            assert_eq!(score.verified.qso_points + dupes(&view), view.qsos);
            // More than one called at once, as a pile-up does.
            let most = runtime
                .turns
                .iter()
                .map(|turn| {
                    turn.segments
                        .iter()
                        .filter(|s| s.who != Sender::You)
                        .count()
                })
                .max();
            assert!(most.is_some_and(|most| most >= 2), "seed {seed}: {most:?}");
        }
    }

    /// The dupe worker works every station that calls, dupes among them: a
    /// dupe logged right is clean and no error, and scores nothing, raw or
    /// verified, and owns no groups. Dupes come once five QSOs are logged.
    #[test]
    fn a_dupe_worker_scores_nothing_for_a_dupe_and_makes_no_error() {
        let mut seen = 0;
        for seed in 20..60 {
            let mut settings = run_settings(25.0);
            settings.contest.contest_style = super::super::RunStyle::Pileup;
            settings.band.stations_max = 3;
            let settings = settings.clamp();
            let sampler = Sampler::new(&[], &settings, seed);
            let mut runtime = Runtime::start(seed, settings, sampler);
            work(&mut runtime, &mut Operator::new(Habit::Perfect), &[]);
            let view = runtime.machine.view();
            let dupes = dupes(&view);
            if dupes == 0 {
                continue;
            }
            seen += dupes;
            for row in &view.log {
                let RowStatus::Checked(check) = &row.status else {
                    panic!("{row:?} is still waiting");
                };
                assert!(check.is_clean(), "seed {seed}: {row:?}");
                if check.dupe {
                    assert!(row.n > 5, "seed {seed}: a dupe after {} QSOs", row.n - 1);
                    let first = view.log.iter().find(|other| other.call == row.call);
                    assert!(first.is_some_and(|first| first.n < row.n
                        && !matches!(
                            &first.status,
                            RowStatus::Checked(check) if check.dupe
                        )));
                }
            }
            let result = runtime.machine.result(0, "2026-10-20".into());
            let run = result
                .contest
                .as_ref()
                .and_then(|log| log.run(result.groups.len()))
                .expect("the log reads back");
            let score = run.score(&result.groups).expect("WPX scores");
            assert_eq!(score.raw.qso_points, view.qsos - dupes, "seed {seed}");
            assert_eq!(score.verified.qso_points, view.qsos - dupes, "seed {seed}");
            let errors = run.errors(&result.groups).expect("its checks");
            assert_eq!(errors.dupes, dupes);
            assert_eq!((errors.busted_calls, errors.nils), (0, 0));
            for (qso, row) in run
                .qsos
                .iter()
                .filter(|qso| qso.k.is_empty())
                .zip(&view.log)
            {
                let dupe = matches!(&row.status, RowStatus::Checked(check) if check.dupe);
                assert_eq!(dupe, qso.g == 0, "seed {seed}: {qso:?}");
            }
            if seen >= 3 {
                return;
            }
        }
        panic!("only {seen} dupes across the seeds");
    }

    /// A Simple QSO run worked perfectly: every QSO logged clean, each with
    /// its call, report and name copied right, and nobody calling after a
    /// TU until the operator calls CQ again. Nothing scores it but the copy.
    #[test]
    fn a_perfect_operator_works_simple_qsos_clean() {
        for seed in [1, 2] {
            let mut settings = run_settings(25.0);
            settings.contest.contest_kind = super::super::ContestKind::SimpleQso;
            let sampler = Sampler::new(&[], &settings, seed);
            let mut runtime = Runtime::start(seed, settings, sampler);
            work(&mut runtime, &mut Operator::new(Habit::Perfect), &[]);
            let view = runtime.machine.view();
            assert!(view.qsos >= 8, "seed {seed}: {} QSOs", view.qsos);
            assert_eq!(view.clean, view.qsos, "seed {seed}");
            let result = runtime.machine.result(0, "2026-10-20".into());
            let run = result
                .contest
                .as_ref()
                .and_then(|log| log.run(result.groups.len()))
                .expect("the log reads back");
            assert_eq!(run.kind, "simpleQso");
            assert_eq!(run.score(&result.groups), None);
            let summary = run.summary(&result.groups).expect("its figures");
            let pace = summary.minutes_per_qso().expect("a pace");
            assert!(
                (0.3..=1.2).contains(&pace),
                "seed {seed}: {pace:.2} minutes a QSO"
            );
            let mut at = 0;
            for qso in &run.qsos {
                let owned = &result.groups[at..at + qso.g as usize];
                at += qso.g as usize;
                assert_eq!(qso.g, 3, "seed {seed}: a call, a report and a name");
                let copied = if qso.k.is_empty() { owned } else { &owned[..1] };
                assert!(copied.iter().all(|group| group.correct), "seed {seed}");
            }
            // Each TU was answered by its station's 73 alone.
            for turn in &runtime.turns {
                let mine = turn.segments.iter().find(|s| s.who == Sender::You);
                if mine.is_some_and(|mine| mine.text.starts_with("TU ")) {
                    let theirs: Vec<&str> = turn
                        .segments
                        .iter()
                        .filter(|s| s.who != Sender::You)
                        .map(|s| s.text.as_str())
                        .collect();
                    assert_eq!(theirs, ["73 EE"], "seed {seed}");
                }
            }
        }
    }

    /// The NA Sprint worked perfectly, one caller at a time and in a
    /// pile-up: every QSO logged clean with its number, name and state, each
    /// caller in the US or Canada sending the state its call is in, your
    /// serial plain and your TU signed, and the score a point a QSO times
    /// the places worked, each once.
    #[test]
    fn a_perfect_operator_works_the_sprint_clean() {
        use super::super::data::places::places_for;
        use super::super::{ContestKind, Field, RunStyle};
        use crate::callsign::Countries;
        for (seed, style) in [(31, RunStyle::Single), (32, RunStyle::Pileup)] {
            let mut settings = run_settings(25.0);
            settings.contest.contest_kind = ContestKind::Sprint;
            settings.contest.contest_style = style;
            settings.contest.my_call = "K1ABC".to_string();
            settings.contest.my_name = "MIKE".to_string();
            settings.contest.my_state = "MA".to_string();
            settings.curriculum.callsign_level = 6;
            settings.band.stations_max = 3;
            let settings = settings.clamp();
            let sampler = Sampler::new(&[], &settings, seed);
            let mut runtime = Runtime::start(seed, settings, sampler);
            work(&mut runtime, &mut Operator::new(Habit::Perfect), &[]);
            let view = runtime.machine.view();
            assert!(view.qsos > 15, "{style:?}: {} QSOs", view.qsos);
            assert_eq!(view.clean, view.qsos, "{style:?}");
            // What you send: the contest's CQ, both calls and your serial
            // plain, and a TU signed once, every third QSO of a pile-up too.
            let yours: Vec<&str> = runtime
                .turns
                .iter()
                .flat_map(|turn| turn.segments.iter())
                .filter(|segment| segment.who == Sender::You)
                .map(|segment| segment.text.as_str())
                .collect();
            assert!(yours.contains(&"CQ NA K1ABC"), "{yours:?}");
            assert!(
                yours.iter().any(|text| text.ends_with(" K1ABC 1 MIKE MA")),
                "{yours:?}"
            );
            assert!(
                yours.iter().any(|text| text.ends_with(" K1ABC 10 MIKE MA")),
                "the tenth serial, plain: {yours:?}"
            );
            assert!(yours.iter().all(|text| !text.contains("K1ABC K1ABC")));
            assert!(yours.iter().any(|text| text.contains("TU K1ABC")));
            let result = runtime.machine.result(0, "2026-10-21".into());
            let run = result
                .contest
                .as_ref()
                .and_then(|log| log.run(result.groups.len()))
                .expect("the log reads back");
            assert_eq!(
                (run.kind.as_str(), run.style.as_str()),
                ("sprint", style.name())
            );
            assert_eq!(run.fields, ["nr", "name", "st"]);
            // Each station worked owns a call and three fields; its state
            // is one its call is in.
            let mut places = std::collections::BTreeSet::new();
            let mut at = 0;
            let mut dupes = 0;
            for qso in &run.qsos {
                let owned = &result.groups[at..at + qso.g as usize];
                at += qso.g as usize;
                if qso.g == 0 {
                    dupes += 1;
                    continue;
                }
                assert_eq!(qso.g, 4, "{style:?}: {qso:?}");
                let (call, st) = (&owned[0].sent, &owned[3].sent);
                assert!(
                    Countries::UsAndCanada.admits(&qso.c),
                    "{style:?}: {call} of {}",
                    qso.c
                );
                assert!(
                    places_for(call).contains(&st.as_str()),
                    "{style:?}: {call} sent {st}"
                );
                if qso.k.is_empty() {
                    assert!(owned.iter().all(|group| group.correct), "{style:?}");
                    places.insert(st.clone());
                }
            }
            let score = run.score(&result.groups).expect("the Sprint scores");
            assert_eq!(score.verified.qso_points + dupes, view.qsos, "{style:?}");
            assert_eq!(score.verified, score.raw, "{style:?}: all clean");
            assert_eq!(score.verified.mults as usize, places.len(), "{style:?}");
            assert_eq!(
                score.verified.score,
                score.verified.qso_points * score.verified.mults
            );
            let errors = run.errors(&result.groups).expect("its checks");
            assert_eq!(
                errors.wrong,
                [(Field::Nr, 0), (Field::Name, 0), (Field::St, 0)]
            );
            // Callers send their exchange with your call before it, and no
            // word: the call went out when it called.
            assert!(runtime.turns.iter().all(|turn| {
                turn.segments
                    .iter()
                    .filter(|segment| segment.who != Sender::You)
                    .all(|segment| {
                        !segment.text.starts_with("TU ") && !segment.text.starts_with("R ")
                    })
            }));
        }
    }

    /// What a listener copies from each thing a caller sends, in either
    /// contest.
    #[test]
    fn a_listener_copies_calls_and_exchanges() {
        assert_eq!(copy_of("K5XYZ"), Some(Copied::Call("K5XYZ")));
        assert_eq!(
            copy_of("DL1ABC DE K5XYZ K5XYZ K"),
            Some(Copied::Call("K5XYZ"))
        );
        assert_eq!(copy_of("DE K5XYZ"), Some(Copied::Correction("K5XYZ", None)));
        assert_eq!(
            copy_of("K5XYZ K5XYZ"),
            Some(Copied::Correction("K5XYZ", None))
        );
        // A call put right with the exchange after it, in each contest.
        for (fix, exch) in [
            ("DE K5XYZ 5NN 214", "214"),
            ("K5XYZ K5XYZ 5NN 214", "214"),
            ("DE K5XYZ UR 579 579 NAME BOB BOB BK", "579 BOB"),
            ("DE K5XYZ K1ABC 145 BOB OR", "145 BOB OR"),
            ("K5XYZ K5XYZ K1ABC 145 BOB OR", "145 BOB OR"),
        ] {
            assert_eq!(
                copy_of(fix),
                Some(Copied::Correction("K5XYZ", Some(exch.to_string()))),
                "{fix}"
            );
        }
        assert_eq!(
            copy_of("TU 5NN 214"),
            Some(Copied::Exchange("214".to_string()))
        );
        for exchange in [
            "R TNX UR 579 579 NAME BOB BOB BK",
            "UR 579 579 NAME BOB BOB BK",
        ] {
            assert_eq!(
                copy_of(exchange),
                Some(Copied::Exchange("579 BOB".to_string()))
            );
        }
        // A report cut to 5NN is still an everyday QSO's.
        assert_eq!(
            copy_of("UR 5NN 5NN NAME BOB BOB BK"),
            Some(Copied::Exchange("5NN BOB".to_string()))
        );
        // A Sprint's, with your call before it or without.
        for exchange in ["K1ABC 145 BOB OR", "145 BOB OR"] {
            assert_eq!(
                copy_of(exchange),
                Some(Copied::Exchange("145 BOB OR".to_string()))
            );
        }
        for nothing in ["73 EE", "AGN", "NR?", ""] {
            assert_eq!(copy_of(nothing), None, "{nothing:?}");
        }
    }

    #[test]
    fn a_human_paced_operator_works_sixty_to_a_hundred_and_fifty_an_hour() {
        for seed in [4, 5] {
            let settings = default_run_settings();
            let sampler = Sampler::new(&[], &settings, seed);
            let mut runtime = Runtime::start(seed, settings, sampler);
            work(&mut runtime, &mut Operator::new(Habit::HumanPaced), &[]);
            let view = runtime.machine.view();
            let hours = 10.0 / 60.0;
            let rate = f64::from(view.qsos) / hours;
            assert!(
                (60.0..=150.0).contains(&rate),
                "seed {seed}: {rate:.0} an hour"
            );
        }
    }

    #[test]
    fn one_digit_off_verifies_nothing() {
        let runtime = worked(Habit::DigitOff, 25.0, 6);
        let view = runtime.machine.view();
        assert!(view.qsos > 10);
        assert_eq!(view.clean, 0);
        let result = runtime.machine.result(0, "2026-10-20".into());
        let run = result
            .contest
            .as_ref()
            .and_then(|log| log.run(result.groups.len()));
        let score = run
            .and_then(|run| run.score(&result.groups))
            .expect("a score");
        assert_eq!(score.verified.score, 0);
        assert!(score.raw.score > 0);
        for row in &view.log {
            let RowStatus::Checked(check) = &row.status else {
                panic!("{row:?} is still waiting");
            };
            assert_eq!(
                check.errors,
                [crate::contest::QsoError::Field(crate::contest::Field::Nr)]
            );
        }
    }

    /// Each call asked for once more: its points halve, and the rest of the
    /// run scores as a perfect one's would.
    #[test]
    fn an_unsure_operator_pays_half_for_each_call() {
        let runtime = worked(Habit::Unsure, 25.0, 7);
        let view = runtime.machine.view();
        assert!(view.qsos > 10);
        assert_eq!(view.clean, view.qsos);
        let result = runtime.machine.result(0, "2026-10-20".into());
        let run = result
            .contest
            .as_ref()
            .and_then(|log| log.run(result.groups.len()));
        let run = run.expect("the log reads back");
        let mut at = 0;
        for qso in &run.qsos {
            let groups = &result.group_timings[at..at + qso.g as usize];
            at += qso.g as usize;
            // A QSO lost, or a dupe, which owns no groups.
            if !qso.k.is_empty() || groups.is_empty() {
                continue;
            }
            assert_eq!(groups[0].sends, 2, "the call, asked for again");
            assert!(groups[1..].iter().all(|timing| timing.sends == 1));
            let call = &result.groups[at - qso.g as usize];
            let points = |sends| {
                group_points(&GroupCopy::of(
                    &call.sent,
                    &call.received,
                    25.0,
                    100.0,
                    sends,
                ))
            };
            assert!((points(groups[0].sends) / points(1) - 0.5).abs() < 1e-9);
        }
    }

    /// Paused and resumed again and again, the run still lasts its limit in
    /// run time, and every pause is kept apart from it.
    #[test]
    fn a_pauser_runs_exactly_the_unpaused_time() {
        let settings = run_settings(25.0);
        let sampler = Sampler::new(&[], &settings, 8);
        let mut runtime = Runtime::start(8, settings, sampler);
        let mut rng = SplitMix64::new(8);
        let pauses: Vec<(u64, u64)> = (1..12)
            .map(|n| {
                let at = n * 50_000 + crate::rng::Rng::usize_in(&mut rng, 0, 9_000) as u64;
                (at, at + 10_000)
            })
            .collect();
        work(&mut runtime, &mut Operator::new(Habit::Perfect), &pauses);
        let result = runtime.machine.result(0, "2026-10-20".into());
        let run = result
            .contest
            .as_ref()
            .and_then(|log| log.run(result.groups.len()));
        let run = run.expect("the log reads back");
        assert_eq!(run.run_ms, 600_000);
        // The pauses that came before the run's end, which every pause
        // pushes back by its length.
        let mut paused = 0;
        for (at, until) in &pauses {
            if at - paused < 600_000 {
                paused += until - at;
            }
        }
        assert!(paused > 0);
        assert_eq!(run.paused_ms, paused);
        assert_eq!(run.run_ms + run.paused_ms, runtime.now);
        assert!(run.qsos.iter().all(|qso| qso.t <= run.run_ms));
    }
}
