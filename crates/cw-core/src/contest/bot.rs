//! Operators that work a run as a person would, and the runtime they work
//! it through.
//!
//! The operators hear the turns and never peek (§13 of the design): what a
//! caller sent reaches them only as the text of a [`ContestEffect::PlayTurn`]
//! segment, once that segment has been heard whole, and never one sent
//! while the receiver was muted for your message. The [`Runtime`] does
//! what the app's runtime does with effects: it plays each turn for exactly
//! its planned length, from the moment a continuation is to start, fires
//! each timer when it is due, and says when a turn started, ended or was
//! cancelled. Hear a QSO is a few QSOs the perfect
//! operator works so ([`hear_qsos`]), for a learner to hear before working
//! one. The machine's tests drive runs through it too, and the soak copies
//! callers as the operators do ([`copy_of`]).

use std::collections::VecDeque;

use super::exchange::{CUT_599, ENN};
use super::machine::{ContestEffect, ContestEvent, ContestMachine, OpKey, PauseWhy};
use super::turn::{Sender, Turn, plan_turn};
use super::{CallSource, Field, Fields};
use crate::callsign::parse_callsign;
use crate::session::MonoMs;
use crate::settings::{CharSetMode, TrainingSettings};

#[cfg(test)]
pub(crate) mod script;

/// What the app's runtime does with a machine's effects, on a clock of its
/// own: each turn plays for its planned length, each timer fires when due.
pub(crate) struct Runtime<S> {
    pub(crate) machine: ContestMachine<S>,
    settings: TrainingSettings,
    /// Now, in milliseconds on the monotonic clock.
    pub(crate) now: u64,
    /// What the runtime owes the machine, each with when it is due.
    due: Vec<(u64, ContestEvent)>,
    /// The turn playing, and when it started, or is to start.
    playing: Option<(u64, Turn)>,
    /// Every turn put on the air, in order.
    pub(crate) turns: Vec<Turn>,
    /// Every effect, in order.
    #[cfg(test)]
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
            #[cfg(test)]
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
            let qsk = self.settings.contest.contest_qsk;
            self.heard.extend(callers_text(&turn, f64::INFINITY, qsk));
        }
        let effects = self.machine.apply(event, MonoMs(self.now));
        self.carry_out(effects.clone());
        effects
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

    /// The id of the turn on the air.
    pub(crate) fn playing_id(&self) -> Option<u64> {
        self.playing.as_ref().map(|(_, turn)| turn.id)
    }

    /// Everything heard whole since the last look, taken.
    pub(crate) fn take_heard(&mut self) -> Vec<String> {
        std::mem::take(&mut self.heard)
    }

    fn carry_out(&mut self, effects: Vec<ContestEffect>) {
        for effect in effects {
            match &effect {
                ContestEffect::PlayTurn(turn) => {
                    // A continuation starts where the turn before leaves
                    // off, which plays on till then; any other at once.
                    let starts = turn.starts_at.map_or(self.now, |at| at.0.max(self.now));
                    self.leave_air(starts);
                    let planned = plan_turn(turn, &self.settings);
                    let ms = (planned.wanted.duration_sec * 1_000.0).ceil() as u64;
                    let id = turn.id;
                    self.due.push((
                        self.now,
                        ContestEvent::TurnStarted {
                            id,
                            heard_from: MonoMs(starts),
                        },
                    ));
                    self.due.push((starts + ms, ContestEvent::TurnEnded { id }));
                    self.playing = Some((starts, turn.clone()));
                    self.turns.push(turn.clone());
                }
                ContestEffect::StopAudio | ContestEffect::ReleaseAudio => self.leave_air(self.now),
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
            #[cfg(test)]
            self.effects.push(effect);
        }
    }

    /// The turn on the air stops at `until`: what was heard whole of it by
    /// then is heard, and its send comes back cancelled.
    fn leave_air(&mut self, until: u64) {
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
        let into = until.saturating_sub(started) as f64 / 1_000.0;
        let qsk = self.settings.contest.contest_qsk;
        self.heard.extend(callers_text(&turn, into, qsk));
        self.due
            .push((self.now, ContestEvent::TurnCancelled { id }));
    }
}

/// What callers sent in `turn` that had been heard whole `into` seconds
/// after it started, but for what went out while the receiver was muted for
/// your message, without full break-in (`qsk`). QRM is heard too, but an
/// operator knows it is no caller, as it would by its pitch and its speed,
/// and leaves it.
fn callers_text(turn: &Turn, into: f64, qsk: bool) -> Vec<String> {
    turn.segments
        .iter()
        .filter(|segment| segment.who.is_caller() && segment.end_sec <= into)
        .filter(|segment| !turn.muted(segment, qsk))
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
    /// Sprint's serial, name and state, `145 BOB OR`; a Field Day class and
    /// section, `3A OR`.
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
        // After the report every caller sends, `5NN` or now and then
        // `ENN`: a serial, a zone or a power, its last word.
        _ if words.iter().any(|word| [CUT_599, ENN].contains(word)) => {
            words.last().map(|serial| (*serial).to_string())
        }
        _ => sprint_exchange(words).or_else(|| field_day_exchange(words)),
    }
}

/// A Field Day exchange, after the word that leads it: its class and
/// section.
fn field_day_exchange(words: &[&str]) -> Option<String> {
    match words {
        [.., cl, sec]
            if Field::Cl.canonical(cl).is_some() && Field::Sec.canonical(sec).is_some() =>
        {
            Some(format!("{cl} {sec}"))
        }
        _ => None,
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

/// One operator: how it works, what it has typed, and what it means to
/// press next.
#[derive(Clone, Debug)]
pub(crate) struct Operator {
    /// How long it takes over each key it types: nothing for one that keys
    /// at once.
    per_key_ms: u64,
    /// It types one digit of each number wrong.
    digit_off: bool,
    /// It asks for each call with a partial first.
    unsure: bool,
    call: String,
    exch: String,
    /// Characters typed since the last key.
    typing: usize,
    /// The call it asked for with a partial, waiting to hear it again.
    asked: Option<String>,
    /// Keys in a row that brought no answer.
    unanswered: u32,
}

impl Operator {
    /// The operator who copies every character and keys the moment the air
    /// falls quiet: the one Hear a QSO plays.
    pub(crate) fn perfect() -> Self {
        Self {
            per_key_ms: 0,
            digit_off: false,
            unsure: false,
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
            Some(Copied::Call(call)) if self.unsure && self.asked.as_deref() != Some(call) => {
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
        let exch = if self.digit_off {
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
        let per_key = self.per_key_ms;
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
            earlier: Vec::new(),
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

/// Let `operator` work the run until it is over, or until `done` says it
/// has done enough, looked at before each key it would press: each time the
/// air falls quiet it reads what it heard and keys what comes next. With
/// `pauses`, it pauses at each first time and resumes at each second.
/// Whether it got there: a run that keeps it busy far longer than any run
/// can last gives up.
pub(crate) fn work_until<S: CallSource>(
    runtime: &mut Runtime<S>,
    operator: &mut Operator,
    pauses: &[(u64, u64)],
    done: impl Fn(&Runtime<S>) -> bool,
) -> bool {
    let mut pauses: VecDeque<(u64, u64)> = pauses.iter().copied().collect();
    let mut paused_until: Option<u64> = None;
    let mut pending: Option<(u64, OpKey, Fields)> = None;
    for _ in 0..100_000 {
        if runtime.over {
            return true;
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
            if done(runtime) {
                return true;
            }
            pending = Some(operator.next_key(runtime.now));
        }
        let key_at = pending.as_ref().map(|(at, _, _)| *at);
        let pause_at = paused_until.or(pauses.front().map(|(at, _)| *at));
        let next = [runtime.next_due(), key_at, pause_at]
            .into_iter()
            .flatten()
            .min();
        let Some(next) = next else {
            return true;
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
    false
}

/// What QSOs heard went out as, one turn after another.
#[derive(Clone, Debug, PartialEq)]
pub struct HeardQsos {
    /// Every turn that went on the air, in order, each after the quiet the
    /// operator left before it.
    pub turns: Vec<HeardTurn>,
    /// Your station as they send it, where some of it stands in for yours,
    /// not given yet: `DL1ABC`, `K1ABC MIKE MA`. Nothing when all of it is
    /// yours.
    pub stand_in: Option<String>,
}

/// One turn of QSOs heard.
#[derive(Clone, Debug, PartialEq)]
pub struct HeardTurn {
    /// The quiet before it, after the turn before had ended, in
    /// milliseconds.
    pub quiet_ms: u64,
    pub turn: Turn,
}

/// `qsos` QSOs of the contest `settings` choose, its run style, speeds and
/// band, worked by the perfect operator, for a learner to hear before
/// working one: every turn that went on the air, from the first CQ to the
/// TU that closed the last of them, with its sign-off where the contest has
/// one. Callers that TU brings are left out, so what is heard ends on the
/// last QSO, not on a station calling for one to come. The operator hears
/// the turns and never peeks, as it does in a run.
///
/// Nothing of it is a run: no result is made, and only `source` hears what
/// the operator copied, a source the caller keeps or lets go of. Your
/// station is yours where `settings` give enough of it for a run, and
/// stands in for yours where they do not ([`HeardQsos::stand_in`]).
pub fn hear_qsos<S: CallSource>(
    seed: u64,
    settings: &TrainingSettings,
    source: S,
    qsos: u32,
) -> HeardQsos {
    let (runtime, heard, stand_in) = worked_to_hear(seed, settings, source, qsos);
    let mut turns = Vec::new();
    let mut ended_ms = 0;
    for turn in runtime.turns {
        let quiet_ms = turn.at_run_ms.saturating_sub(ended_ms);
        let planned = plan_turn(&turn, &heard).wanted.duration_sec;
        ended_ms = turn.at_run_ms + (planned * 1_000.0).ceil() as u64;
        turns.push(HeardTurn { quiet_ms, turn });
    }
    // Whoever the last TU brings to call is for a QSO not heard: of the
    // last turn, only your TU and the sign-off of the station it logged
    // are kept, however the callers it brought call, once or twice.
    let signoff = heard.contest.contest_kind.rules().signoff;
    if let Some(last) = turns.last_mut() {
        last.turn
            .segments
            .retain(|segment| segment.who == Sender::You || Some(segment.text.as_str()) == signoff);
    }
    HeardQsos { turns, stand_in }
}

/// The run [`hear_qsos`] plays, worked by the perfect operator until
/// `qsos` QSOs are in its log: the runtime it went through, the settings it
/// ran on, and your station where some of it stands in for yours.
fn worked_to_hear<S: CallSource>(
    seed: u64,
    settings: &TrainingSettings,
    source: S,
    qsos: u32,
) -> (Runtime<S>, TrainingSettings, Option<String>) {
    let mut heard = settings.clone();
    heard.curriculum.char_set_mode = CharSetMode::Contest;
    // The first CQ at once, and never the run's end before the last QSO.
    heard.playback.countdown_sec = 0;
    heard.contest.contest_minutes = 0;
    let stand_in = heard.contest.stand_in();
    let heard = heard.clamp();
    let mut runtime = Runtime::start(seed, heard.clone(), source);
    // However it stops, what went on the air is what is heard.
    work_until(&mut runtime, &mut Operator::perfect(), &[], |runtime| {
        runtime.machine.view().qsos >= qsos
    });
    (runtime, heard, stand_in)
}

#[cfg(test)]
mod tests {
    use super::script::{Habit, default_run_settings, run_settings, work};
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
            let (runtime, run, result, yours) =
                perfect_run(ContestKind::Sprint, style, "K1ABC", ("MA", ""), seed);
            let view = runtime.machine.view();
            assert!(view.qsos > 15, "{style:?}: {} QSOs", view.qsos);
            assert_eq!(view.clean, view.qsos, "{style:?}");
            // What you send: the contest's CQ, both calls and your serial
            // plain, and a TU signed once, every third QSO of a pile-up too.
            assert!(yours.iter().any(|text| text == "CQ NA K1ABC"), "{yours:?}");
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
                    places_for(call, false).contains(&st.as_str()),
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

    /// A run of `kind` as `call`, called MIKE, from `state` and `section`,
    /// worked perfectly in `style` from `seed`, with callers from every tier
    /// and up to three at once: the runtime, its stored run and session, and
    /// every message you sent.
    fn perfect_run(
        kind: super::super::ContestKind,
        style: super::super::RunStyle,
        call: &str,
        (state, section): (&str, &str),
        seed: u64,
    ) -> (
        Runtime<Sampler>,
        crate::contest::ContestRun,
        crate::session::SessionResult,
        Vec<String>,
    ) {
        let habits = super::super::CallerHabits::Easy;
        perfect_run_with(habits, kind, style, call, (state, section), seed)
    }

    /// The same, with the callers' `habits`.
    fn perfect_run_with(
        habits: super::super::CallerHabits,
        kind: super::super::ContestKind,
        style: super::super::RunStyle,
        call: &str,
        (state, section): (&str, &str),
        seed: u64,
    ) -> (
        Runtime<Sampler>,
        crate::contest::ContestRun,
        crate::session::SessionResult,
        Vec<String>,
    ) {
        let mut settings = run_settings(25.0);
        settings.contest.contest_habits = habits;
        settings.contest.contest_kind = kind;
        settings.contest.contest_style = style;
        settings.contest.my_call = call.to_string();
        settings.contest.my_name = "MIKE".to_string();
        settings.contest.my_state = state.to_string();
        settings.contest.my_section = section.to_string();
        settings.curriculum.callsign_level = 6;
        settings.band.stations_max = 3;
        let settings = settings.clamp();
        assert_eq!(settings.contest.missing(), None, "{kind:?} as {call}");
        let sampler = Sampler::new(&[], &settings, seed);
        let mut runtime = Runtime::start(seed, settings, sampler);
        work(&mut runtime, &mut Operator::new(Habit::Perfect), &[]);
        let result = runtime.machine.result(0, "2026-10-21".into());
        let run = result
            .contest
            .as_ref()
            .and_then(|log| log.run(result.groups.len()))
            .expect("the log reads back");
        let yours = runtime
            .turns
            .iter()
            .flat_map(|turn| turn.segments.iter())
            .filter(|segment| segment.who == Sender::You)
            .map(|segment| segment.text.clone())
            .collect();
        (runtime, run, result, yours)
    }

    /// CQ WW worked perfectly from Europe and from North America, one
    /// caller at a time and in a pile-up: every QSO clean, your zone sent
    /// cut, each caller's zone one its call is in, and the score its
    /// points by distance times the zones and countries worked, each once.
    #[test]
    fn a_perfect_operator_works_cqww_clean_from_anywhere() {
        use super::super::data::places::home_country;
        use super::super::data::zones::{draw_zone, home_continent};
        use super::super::{ContestKind, RunStyle};
        for (seed, style, call, sent, home) in [
            (41, RunStyle::Single, "DL1ABC", " 5NN A4", ("DL", "EU", 14)),
            (42, RunStyle::Pileup, "DL1ABC", " 5NN A4", ("DL", "EU", 14)),
            (43, RunStyle::Single, "K1ABC", " 5NN 5", ("K", "NA", 5)),
            (44, RunStyle::Pileup, "K1ABC", " 5NN 5", ("K", "NA", 5)),
        ] {
            let case = format!("{call} {style:?}");
            let (runtime, run, result, yours) =
                perfect_run(ContestKind::Cqww, style, call, ("", ""), seed);
            let view = runtime.machine.view();
            assert!(view.qsos > 15, "{case}: {} QSOs", view.qsos);
            assert_eq!(view.clean, view.qsos, "{case}");
            assert!(yours.contains(&format!("CQ {call} TEST")), "{case}");
            assert!(
                yours.iter().any(|text| text.ends_with(sent)),
                "{case}: {yours:?}"
            );
            assert_eq!(run.fields, ["zn"], "{case}");
            assert_eq!(run.side, "", "{case}: the same from either side");
            assert_eq!(
                (
                    run.my_country.as_str(),
                    run.my_continent.as_str(),
                    run.my_zone
                ),
                home,
                "{case}"
            );
            let mut points = 0;
            let mut mults = std::collections::BTreeSet::new();
            let mut at = 0;
            let mut rng = SplitMix64::new(seed);
            for qso in &run.qsos {
                let owned = &result.groups[at..at + qso.g as usize];
                at += qso.g as usize;
                if qso.g == 0 {
                    continue;
                }
                assert_eq!(qso.g, 2, "{case}: {qso:?}");
                let (his, zone) = (&owned[0].sent, &owned[1].sent);
                let zones: std::collections::BTreeSet<u32> =
                    (0..300).filter_map(|_| draw_zone(his, &mut rng)).collect();
                let zone: u32 = zone.parse().expect("a zone in plain digits");
                assert!(zones.contains(&zone), "{case}: {his} sent zone {zone}");
                // A station still being worked when time ran out is lost.
                if !qso.k.is_empty() {
                    continue;
                }
                assert!(owned.iter().all(|group| group.correct), "{case}");
                let country = home_country(his).expect("a placed caller");
                points += match home_continent(his) {
                    _ if country == home.0 => 0,
                    Some(continent) if continent == home.1 && continent == "NA" => 2,
                    Some(continent) if continent == home.1 => 1,
                    _ => 3,
                };
                mults.insert(format!("zone {zone}"));
                mults.insert(country.to_string());
            }
            let score = run.score(&result.groups).expect("CQ WW scores");
            assert_eq!(score.verified, score.raw, "{case}: all clean");
            assert_eq!(score.verified.qso_points, points, "{case}");
            assert_eq!(score.verified.mults as usize, mults.len(), "{case}");
            assert_eq!(
                score.verified.score,
                score.verified.qso_points * score.verified.mults
            );
        }
    }

    /// ARRL DX worked perfectly from either side, one caller at a time and
    /// in a pile-up: as DX, only W/VE calls, each sending the state or
    /// province its call is in, Labrador apart, and your power cut; as W/VE,
    /// only DX calls, each sending its power, and your state; three points
    /// a QSO times what your side counts, each once.
    #[test]
    fn a_perfect_operator_works_arrl_dx_clean_from_either_side() {
        use super::super::data::places::{places_for, w_ve_place};
        use super::super::{ContestKind, Field, RunStyle};
        use crate::callsign::Countries;
        for (seed, style, call, state, side) in [
            (51, RunStyle::Single, "DL1ABC", "", "dx"),
            (52, RunStyle::Pileup, "DL1ABC", "", "dx"),
            (53, RunStyle::Single, "K1ABC", "MA", "wve"),
            (54, RunStyle::Pileup, "K1ABC", "MA", "wve"),
        ] {
            let case = format!("{call} {style:?}");
            let (runtime, run, result, yours) =
                perfect_run(ContestKind::ArrlDx, style, call, (state, ""), seed);
            let view = runtime.machine.view();
            assert!(view.qsos > 15, "{case}: {} QSOs", view.qsos);
            assert_eq!(view.clean, view.qsos, "{case}");
            assert_eq!(run.side, side, "{case}");
            let dx = side == "dx";
            let ending = if dx { " 5NN 1TT" } else { " 5NN MA" };
            assert!(
                yours.iter().any(|text| text.ends_with(ending)),
                "{case}: {yours:?}"
            );
            assert_eq!(run.fields, [if dx { "st" } else { "pwr" }], "{case}");
            let mut mults = std::collections::BTreeSet::new();
            let mut at = 0;
            let mut qsos = 0;
            for qso in &run.qsos {
                let owned = &result.groups[at..at + qso.g as usize];
                at += qso.g as usize;
                if qso.g == 0 {
                    continue;
                }
                let (his, sent) = (&owned[0].sent, &owned[1].sent);
                assert_eq!(
                    Countries::WVe.admits(&qso.c),
                    dx,
                    "{case}: {his} of {}",
                    qso.c
                );
                if dx {
                    assert!(
                        places_for(his, true).contains(&sent.as_str()),
                        "{case}: {his} sent {sent}"
                    );
                    assert!(w_ve_place(sent), "{case}: {sent}");
                } else {
                    assert!(Field::Pwr.canonical(sent).is_some(), "{case}: {sent}");
                }
                // A station still being worked when time ran out is lost.
                if !qso.k.is_empty() {
                    continue;
                }
                qsos += 1;
                mults.insert(if dx { sent.clone() } else { qso.c.clone() });
            }
            let score = run.score(&result.groups).expect("ARRL DX scores");
            assert_eq!(score.verified, score.raw, "{case}: all clean");
            assert_eq!(score.verified.qso_points, 3 * qsos, "{case}");
            assert_eq!(score.verified.mults as usize, mults.len(), "{case}");
        }
    }

    /// Field Day worked perfectly, one caller at a time and in a pile-up,
    /// from the US and from abroad: every QSO clean, your class and section
    /// sent after the call, each caller's class one Field Day stations send
    /// and its section one its call can be in; two points a QSO and nothing
    /// to multiply them, so the score is the points.
    #[test]
    fn a_perfect_operator_works_field_day_clean() {
        use super::super::data::fd::draw_section;
        use super::super::{ContestKind, Field, RunStyle};
        for (seed, style, call, section, ending) in [
            (61, RunStyle::Single, "K1ABC", "EMA", " 1D EMA"),
            (62, RunStyle::Pileup, "K1ABC", "EMA", " 1D EMA"),
            (63, RunStyle::Single, "KH6XYZ", "", " 1D PAC"),
            (64, RunStyle::Pileup, "DL1ABC", "", " 1D DX"),
        ] {
            let case = format!("{call} {style:?}");
            let (runtime, run, result, yours) =
                perfect_run(ContestKind::FieldDay, style, call, ("", section), seed);
            let view = runtime.machine.view();
            assert!(view.qsos > 20, "{case}: {} QSOs", view.qsos);
            assert_eq!(view.clean, view.qsos, "{case}");
            assert!(yours.contains(&format!("CQ FD {call}")), "{case}");
            assert!(
                yours.iter().any(|text| text.ends_with(ending)),
                "{case}: {yours:?}"
            );
            assert_eq!(run.fields, ["cl", "sec"], "{case}");
            let mut rng = SplitMix64::new(seed);
            let mut at = 0;
            let mut qsos = 0;
            for qso in &run.qsos {
                let owned = &result.groups[at..at + qso.g as usize];
                at += qso.g as usize;
                if qso.g == 0 {
                    continue;
                }
                assert_eq!(qso.g, 3, "{case}: {qso:?}");
                let (his, cl, sec) = (&owned[0].sent, &owned[1].sent, &owned[2].sent);
                assert!(Field::Cl.canonical(cl).is_some(), "{case}: {his} sent {cl}");
                let sections: std::collections::BTreeSet<&str> =
                    (0..300).map(|_| draw_section(his, &mut rng)).collect();
                assert!(sections.contains(sec.as_str()), "{case}: {his} sent {sec}");
                if qso.k.is_empty() {
                    qsos += 1;
                }
            }
            let score = run.score(&result.groups).expect("Field Day scores");
            assert_eq!(score.verified, score.raw, "{case}: all clean");
            assert_eq!(score.verified.qso_points, 2 * qsos, "{case}");
            assert_eq!(score.verified.score, score.verified.qso_points, "{case}");
        }
    }

    /// Hear a QSO: three QSOs of each contest, in either run style, worked
    /// by the perfect operator from the first CQ to the TU of the third,
    /// with its sign-off where the contest has one, and nobody calling after
    /// it, a pile-up caller that calls twice among them (seed 3 brings one
    /// in a WPX and a Sprint pile-up). The turns follow one another as they
    /// went out, and the same seed hears the same QSOs.
    #[test]
    fn hear_a_qso_plays_a_few_qsos_from_the_first_cq_to_the_last_tu() {
        use super::super::{ContestKind, RunStyle};
        for kind in ContestKind::ALL {
            let signoff = kind.rules().signoff;
            for style in RunStyle::ALL {
                let mut settings = TrainingSettings::default();
                settings.contest.contest_kind = kind;
                settings.set_contest_style(style);
                let hear = |seed| hear_qsos(seed, &settings, Sampler::new(&[], &settings, seed), 3);
                assert_eq!(hear(5), hear(5), "{kind:?} {style:?}");
                for seed in 0..24 {
                    let case = format!("{kind:?} {style:?} seed {seed}");
                    let heard = hear(seed);
                    let yours: Vec<&str> = heard
                        .turns
                        .iter()
                        .flat_map(|heard| heard.turn.segments.iter())
                        .filter(|segment| segment.who == Sender::You)
                        .map(|segment| segment.text.as_str())
                        .collect();
                    assert!(yours[0].starts_with("CQ "), "{case}: {yours:?}");
                    let tus = yours
                        .iter()
                        .filter(|text| text.split_whitespace().any(|word| word == "TU"))
                        .count();
                    assert_eq!(tus, 3, "{case}: {yours:?}");
                    let last = &heard.turns.last().expect("turns").turn;
                    assert!(yours.last().is_some_and(|text| text.contains("TU")));
                    let theirs: Vec<&str> = last
                        .segments
                        .iter()
                        .filter(|segment| segment.who != Sender::You)
                        .map(|segment| segment.text.as_str())
                        .collect();
                    assert_eq!(theirs, Vec::from_iter(signoff), "{case}");
                    // One after another: the first at once, and none after a
                    // long wait.
                    assert_eq!(heard.turns[0].quiet_ms, 0, "{case}");
                    assert!(
                        heard.turns.iter().all(|heard| heard.quiet_ms < 2_000),
                        "{case}"
                    );
                    let ats: Vec<u64> = heard
                        .turns
                        .iter()
                        .map(|heard| heard.turn.at_run_ms)
                        .collect();
                    assert!(ats.windows(2).all(|pair| pair[0] < pair[1]), "{ats:?}");
                }
            }
        }
    }

    /// Hear a QSO plays your station where enough of it is given for a run,
    /// and stands in for what is not: a call, and a name and a state where
    /// the contest sends them, a North American call where it sends a
    /// state.
    #[test]
    fn hear_a_qso_stands_in_for_your_station_until_you_give_it() {
        use super::super::ContestKind;
        let cq = |heard: &HeardQsos| heard.turns[0].turn.segments[0].text.clone();
        for (kind, call, name, stand_in, sent) in [
            (ContestKind::Wpx, "", "", Some("DL1ABC"), "CQ DL1ABC TEST"),
            (ContestKind::Wpx, "G4ABC", "", None, "CQ G4ABC TEST"),
            (
                ContestKind::SimpleQso,
                "",
                "",
                Some("DL1ABC MIKE"),
                "CQ CQ DE DL1ABC DL1ABC K",
            ),
            (
                ContestKind::SimpleQso,
                "G4ABC",
                "ANN",
                None,
                "CQ CQ DE G4ABC G4ABC K",
            ),
            (
                ContestKind::Sprint,
                "",
                "",
                Some("K1ABC MIKE MA"),
                "CQ NA K1ABC",
            ),
            (
                ContestKind::Sprint,
                "DL1ABC",
                "",
                Some("DL1ABC MIKE DX"),
                "CQ NA DL1ABC",
            ),
            (
                ContestKind::Sprint,
                "W7ABC",
                "BOB",
                Some("W7ABC BOB MA"),
                "CQ NA W7ABC",
            ),
            (ContestKind::Cqww, "", "", Some("DL1ABC"), "CQ DL1ABC TEST"),
            (ContestKind::Cqww, "JA1XYZ", "", None, "CQ JA1XYZ TEST"),
            // A call that does not say its zone has one stand in.
            (
                ContestKind::Cqww,
                "C31AB",
                "",
                Some("C31AB zone 14"),
                "CQ C31AB TEST",
            ),
            // ARRL DX: DX sends its power, always given; W/VE its state.
            (
                ContestKind::ArrlDx,
                "",
                "",
                Some("DL1ABC"),
                "CQ DL1ABC TEST",
            ),
            (ContestKind::ArrlDx, "G4ABC", "", None, "CQ G4ABC TEST"),
            (
                ContestKind::ArrlDx,
                "K1ABC",
                "",
                Some("K1ABC MA"),
                "CQ K1ABC TEST",
            ),
            // Field Day: your class is always given, and your section
            // stands in where your call does not tell it.
            (
                ContestKind::FieldDay,
                "",
                "",
                Some("K1ABC 1D EMA"),
                "CQ FD K1ABC",
            ),
            (
                ContestKind::FieldDay,
                "W7ABC",
                "",
                Some("W7ABC 1D EMA"),
                "CQ FD W7ABC",
            ),
            (ContestKind::FieldDay, "KL7AB", "", None, "CQ FD KL7AB"),
            (ContestKind::FieldDay, "DL1ABC", "", None, "CQ FD DL1ABC"),
        ] {
            let mut settings = TrainingSettings::default();
            settings.contest.contest_kind = kind;
            settings.contest.my_call = call.to_string();
            settings.contest.my_name = name.to_string();
            let heard = hear_qsos(1, &settings, Sampler::new(&[], &settings, 1), 1);
            assert_eq!(heard.stand_in.as_deref(), stand_in, "{kind:?} {call}");
            assert_eq!(cq(&heard), sent, "{kind:?} {call}");
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
            ("DE K5XYZ 3A OR", "3A OR"),
            ("K5XYZ K5XYZ 1D EMA", "1D EMA"),
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
        // A Field Day class and section, led by R or TU or alone.
        for exchange in ["R 3A OR", "TU 3A OR", "3A OR"] {
            assert_eq!(
                copy_of(exchange),
                Some(Copied::Exchange("3A OR".to_string()))
            );
        }
        assert_eq!(
            copy_of("R 1D DX"),
            Some(Copied::Exchange("1D DX".to_string()))
        );
        for nothing in ["73 EE", "AGN", "NR?", "", "R 3G OR"] {
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

    /// Callers of every habit are worked clean by the perfect operator, in
    /// every contest and both run styles: it reads numbers cut and `ENN`
    /// by their value, a fumbled serial as it was put right, and leaves the
    /// QRM of rough habits alone, as no caller of its.
    #[test]
    fn a_perfect_operator_works_callers_of_every_habit_clean() {
        use super::super::{CallerHabits, ContestKind, RunStyle};
        let mut qrm = 0;
        for (at, habits) in CallerHabits::ALL.into_iter().enumerate() {
            for kind in ContestKind::ALL {
                for style in RunStyle::ALL {
                    let home = matches!(kind, ContestKind::Sprint | ContestKind::FieldDay);
                    let call = if home { "K1ABC" } else { "DL1ABC" };
                    let seed = 70 + at as u64;
                    let (runtime, ..) =
                        perfect_run_with(habits, kind, style, call, ("MA", "EMA"), seed);
                    let case = format!("{habits:?} {kind:?} {style:?}");
                    let view = runtime.machine.view();
                    assert!(view.qsos >= 3, "{case}: {} QSOs", view.qsos);
                    assert_eq!(view.clean, view.qsos, "{case}: {:?}", view.log);
                    let sent = runtime
                        .turns
                        .iter()
                        .flat_map(|turn| turn.segments.iter())
                        .filter(|segment| segment.who == Sender::Qrm)
                        .count();
                    assert!(sent == 0 || habits == CallerHabits::Rough, "{case}");
                    qrm += sent;
                }
            }
        }
        assert!(qrm > 20, "only {qrm} messages of QRM in the rough runs");
    }

    /// Hear a QSO under each of the callers' habits still plays three
    /// clean QSOs, from the first CQ to the third TU and its sign-off, with
    /// nobody calling after it and no QRM in its last turn, on as many
    /// seeds as Hear a QSO is held to under easy habits.
    #[test]
    fn hear_a_qso_stays_clean_under_every_habit() {
        use super::super::{CallerHabits, ContestKind, RunStyle};
        for habits in CallerHabits::ALL {
            for kind in ContestKind::ALL {
                let signoff = kind.rules().signoff;
                for style in RunStyle::ALL {
                    let mut settings = TrainingSettings::default();
                    settings.contest.contest_kind = kind;
                    settings.contest.contest_habits = habits;
                    settings.set_contest_style(style);
                    for seed in 0..24 {
                        let case = format!("{habits:?} {kind:?} {style:?} seed {seed}");
                        let source = || Sampler::new(&[], &settings, seed);
                        let (runtime, ..) = worked_to_hear(seed, &settings, source(), 3);
                        let view = runtime.machine.view();
                        assert!(view.qsos >= 3, "{case}");
                        assert_eq!(view.clean, view.qsos, "{case}: {:?}", view.log);
                        let heard = hear_qsos(seed, &settings, source(), 3);
                        let last = &heard.turns.last().expect("turns").turn;
                        let theirs: Vec<&str> = last
                            .segments
                            .iter()
                            .filter(|segment| segment.who != Sender::You)
                            .map(|segment| segment.text.as_str())
                            .collect();
                        assert_eq!(theirs, Vec::from_iter(signoff), "{case}");
                    }
                }
            }
        }
    }

    /// In the Sprint, whose serials go plain, callers of every habit send
    /// theirs in plain digits: cut, a serial of nines and zeros would be
    /// letters alone (`NT` for 90), read as a state beside the name, or the
    /// name `ANNE` read as the serial, and the QSO would never log. So Hear
    /// a QSO reaches its three clean QSOs, quickly, on every seed (seeds 68
    /// and 97 once brought a caller sending `NT`).
    #[test]
    fn sprint_callers_send_plain_serials_and_every_qso_logs() {
        use super::super::{CallerHabits, ContestKind, RunStyle};
        for habits in CallerHabits::ALL {
            for style in RunStyle::ALL {
                let mut settings = TrainingSettings::default();
                settings.contest.contest_kind = ContestKind::Sprint;
                settings.contest.contest_habits = habits;
                settings.set_contest_style(style);
                for seed in 0..120 {
                    let case = format!("{habits:?} {style:?} seed {seed}");
                    let source = Sampler::new(&[], &settings, seed);
                    let (runtime, ..) = worked_to_hear(seed, &settings, source, 3);
                    let view = runtime.machine.view();
                    assert!(view.qsos >= 3, "{case}: {} QSOs", view.qsos);
                    assert_eq!(view.clean, view.qsos, "{case}: {:?}", view.log);
                    assert!(
                        runtime.turns.len() < 40,
                        "{case}: {} turns",
                        runtime.turns.len()
                    );
                    let sent = runtime
                        .turns
                        .iter()
                        .flat_map(|turn| turn.segments.iter())
                        .filter(|segment| segment.who.is_caller());
                    let mut exchanges = 0;
                    for segment in sent {
                        let words: Vec<&str> = segment.text.split_whitespace().collect();
                        exchanges += usize::from(sprint_exchange(&words).is_some());
                    }
                    assert!(
                        exchanges >= 3,
                        "{case}: {exchanges} exchanges in plain digits"
                    );
                }
            }
        }
    }
}
