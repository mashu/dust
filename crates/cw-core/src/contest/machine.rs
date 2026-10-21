//! A contest run as a pure state machine, beside the group session's
//! [`crate::machine::SessionMachine`] and built the same way: `apply(event,
//! now)` hands back effects for the runtime to carry out, a timer is
//! cancelled by forgetting its id, and every draw comes from the run's seed,
//! through a generator of its own for each caller, so a seed and a list of
//! events always make the same run.
//!
//! Each thing you send is a [`Turn`]: your message and every reply to it,
//! decided when you key it. The runtime plays it and says when its sound
//! started, ended, was cancelled or failed. Callers wait for you as
//! MorseRunner's do ([`super::caller`]); when one has waited long enough the
//! machine's patience timer runs out and it calls again, in a turn of its
//! own.
//!
//! In a pile-up several call at once, as many as the Band card's stations
//! range draws after each CQ or TU. A caller still sending when you key over
//! it never hears you, and goes on sending over you; only the best match
//! answers a call or a partial; and now and then a caller misses your
//! exchange, or one worked before calls again.
//!
//! What you copy is kept as you keyed it, for the run's groups
//! ([`super::record`]): each call when you first sent it with your
//! exchange, each field as typed, and how often you asked for either.

use super::caller::{CallMatch, Caller, CallerState, Likeness, Says, match_call};
use super::check::{QsoCheck, QsoText, check_qso};
use super::clock::RunClock;
use super::esm::{EntryField, Fields, MessagePart, QsoState, esm, loggable, message_text};
use super::exchange::{Field, keyed_token};
use super::parse::{Reading, normalize_call, parse_exchange};
use super::place::{filter_capacity, place_contest_caller};
use super::record::{ContestRun, LAYOUT, LOST, NIL, QsoRecord};
use super::rules::{ContestRules, Fill, RULES, Side};
use super::score::rate_at;
use super::turn::{Fade, Segment, Sender, Turn, char_spans, sending_sec, sounding_at, your_voice};
use super::{CallSource, MS_PER_MINUTE, RunStyle};
use crate::alignment::CopyTally;
use crate::band::Qsb;
use crate::callsign::{country_of, parse_callsign};
use crate::machine::COUNTDOWN_STEP_MS;
use crate::rng::{Rng, SplitMix64};
use crate::session::{
    Answered, GroupResult, MIN_RESPONSE_MS_PER_CHAR, MonoMs, SessionResult, SessionTiming, whole_ms,
};
use crate::settings::{STATIONS_MAX, STATIONS_MIN, TrainingSettings, WPM_MAX, WPM_MIN};
use crate::timing::StationVoice;

/// Why a run is paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseWhy {
    /// You paused it.
    You,
    /// The page was hidden or the phone locked. It never resumes by itself.
    Hidden,
    /// The sound failed after every retry.
    Sound,
}

/// Where a run is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContestPhase {
    /// Counting down to the first CQ, `left` seconds to go.
    Countdown {
        left: u32,
    },
    Running,
    /// Silent, the clock held, every timer forgotten.
    Paused {
        why: PauseWhy,
    },
    Finished,
    Aborted,
}

/// What you can do in a run, as keys and buttons (§4.5 of the design).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKey {
    /// Enter: what the QSO needs next ([`esm`]).
    Esm,
    Cq,
    /// Your exchange.
    Exchange,
    /// TU without logging, as MorseRunner's F3.
    Tu,
    MyCall,
    /// Their call as the Call field holds it.
    HisCall,
    HisCallAndExchange,
    B4,
    /// `?`: send it again.
    Again,
    Nil,
    NrQuery,
    /// TU, and log the QSO.
    TuAndLog,
    /// Log the QSO without sending anything.
    LogOnly,
    /// Clear both fields and start the QSO's keying again.
    Wipe,
    /// Esc: stop sending.
    StopTx,
}

impl OpKey {
    /// The key that sends `part` on its own: what Enter presses, part by
    /// part, as MorseRunner lights the keys Enter sends. Enter's TU logs, as
    /// TU and log does.
    pub fn sending(part: &MessagePart) -> Self {
        match part {
            MessagePart::Cq => Self::Cq,
            MessagePart::HisCall(_) => Self::HisCall,
            MessagePart::Exchange => Self::Exchange,
            MessagePart::Again => Self::Again,
            MessagePart::Tu => Self::TuAndLog,
            MessagePart::MyCall => Self::MyCall,
            MessagePart::B4 => Self::B4,
            MessagePart::Nil => Self::Nil,
            MessagePart::NrQuery => Self::NrQuery,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContestEvent {
    /// A key, once any paddle letter in progress has finished, with the
    /// fields as they are then.
    Key {
        key: OpKey,
        fields: Fields,
    },
    /// A turn's sound has started, on each attempt, retries too:
    /// `heard_from` is when it reaches the ear.
    TurnStarted {
        id: u64,
        heard_from: MonoMs,
    },
    TurnEnded {
        id: u64,
    },
    /// A turn was stopped, or a newer one replaced it: always ignored.
    TurnCancelled {
        id: u64,
    },
    /// A turn's sound failed after every retry.
    TurnFailed {
        id: u64,
    },
    TimerFired {
        id: u64,
    },
    /// Pause, from `at`: a pause noticed late is stamped when it began.
    Pause {
        why: PauseWhy,
        at: MonoMs,
    },
    Resume,
    /// − / +: your speed and later callers', in WPM.
    Speed {
        delta: i32,
    },
    FinishNow,
    Abort,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContestEffect {
    /// Play this turn. Starting it releases whatever was playing.
    PlayTurn(Turn),
    StopAudio,
    /// Let go of the player altogether, so the band falls silent and the
    /// screen may sleep.
    ReleaseAudio,
    /// Send [`ContestEvent::TimerFired`] with this id once `ms` have passed.
    StartTimer {
        id: u64,
        ms: u32,
    },
    ClearFields,
    Focus(EntryField),
    /// Say this to a screen reader, once the air is quiet.
    Announce(String),
    PersistAndShowResults,
    /// "No stations worked — nothing kept".
    ShowNothingKept,
    AbortToHome,
}

/// What is on the air: when it reached the ear, your own message, and when
/// each station sends, without what they send.
#[derive(Clone, Debug, PartialEq)]
pub struct AirView {
    pub heard_from: MonoMs,
    pub my_text: String,
    /// Where each character of your message that sounds stops sounding, in
    /// seconds from the start of the turn, by its byte in `my_text`.
    pub my_chars: Vec<(usize, f64)>,
    pub spans: Vec<(Sender, f64, f64)>,
}

impl AirView {
    /// How far into the turn the ear is at `now`, in seconds; nothing has
    /// reached it before its sound does.
    fn heard_sec(&self, now: MonoMs) -> f64 {
        now.since(self.heard_from) as f64 / 1_000.0
    }

    /// What is still being sent of your message at `now`: from the
    /// character sounding then, as a key now would send it again. Empty
    /// once it has all gone out.
    pub fn still_sending(&self, now: MonoMs) -> &str {
        let at_sec = self.heard_sec(now);
        self.my_chars
            .iter()
            .find(|(_, end)| *end > at_sec)
            .map_or("", |(at, _)| &self.my_text[*at..])
    }

    /// How many stations are sending at `now`. What they send is never
    /// shown.
    pub fn calling(&self, now: MonoMs) -> usize {
        let at_sec = self.heard_sec(now);
        self.spans
            .iter()
            .filter(|(who, start, end)| *who != Sender::You && *start <= at_sec && at_sec < *end)
            .count()
    }
}

/// Where a logged QSO's check stands.
#[derive(Clone, Debug, PartialEq)]
pub enum RowStatus {
    /// The station has not finished with you yet: `…`.
    Pending,
    Checked(QsoCheck),
}

/// One logged QSO, as the live log shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct LogRow {
    /// Its number in the log, from 1.
    pub n: u32,
    /// When it was logged, in run time.
    pub at_ms: u64,
    pub call: String,
    /// What you logged of its exchange, each field's value.
    pub fields: Vec<(Field, String)>,
    /// The station's speed, once its check is shown.
    pub wpm: Option<f64>,
    /// How many repeats you asked for.
    pub asked: u32,
    pub status: RowStatus,
}

/// A run as the screen shows it. It holds nothing of what a station really
/// sent until that QSO's check is shown.
#[derive(Clone, Debug, PartialEq)]
pub struct ContestView {
    pub phase: ContestPhase,
    /// The run's clock, once it has started: the run bar reads it itself.
    pub clock: Option<RunClock>,
    pub limit_ms: u64,
    /// The speed you send at.
    pub my_wpm: f64,
    /// What Enter reads: the call last sent, whether your exchange was
    /// taken, and whether the station corrected you.
    pub qso: QsoState,
    pub air: Option<AirView>,
    /// The log, oldest first.
    pub log: Vec<LogRow>,
    /// QSOs logged, dupes and NILs among them, and those checked clean.
    pub qsos: u32,
    pub clean: u32,
    /// Your serial for the next QSO, as your exchange sends it; 0 in a
    /// contest without one.
    pub serial: u32,
}

impl ContestView {
    /// Run time at `now`, as the run bar's clock shows it: never past the
    /// limit, and nothing before the clock has started. A look, which
    /// stamps nothing ([`RunClock::peek_ms`]).
    pub fn run_ms(&self, now: MonoMs) -> u64 {
        self.clock
            .map_or(0, |clock| clock.peek_ms(now))
            .min(self.limit_ms)
    }

    /// Minutes a QSO at `now`, over the run so far, the pace a contest
    /// without a score is told by; none before a QSO is logged.
    pub fn minutes_per_qso(&self, now: MonoMs) -> Option<f64> {
        (self.qsos > 0)
            .then(|| self.run_ms(now) as f64 / MS_PER_MINUTE as f64 / f64::from(self.qsos))
    }

    /// When a QSO with `call` was first logged clean, if one was, for the
    /// screen to say "B4 07:31" as it is typed again: working it again is a
    /// dupe. A QSO waiting for its check counts once it is checked.
    pub fn worked_before(&self, call: &str) -> Option<u64> {
        let call = normalize_call(call);
        self.log
            .iter()
            .find(|row| {
                row.call == call
                    && matches!(&row.status, RowStatus::Checked(check) if check.is_clean())
            })
            .map(|row| row.at_ms)
    }

    /// QSOs an hour at `now`, over the last five minutes of run time, once
    /// the run is a minute old.
    pub fn rate(&self, now: MonoMs) -> Option<f64> {
        let run_ms = self.run_ms(now);
        let times: Vec<u64> = self.log.iter().map(|row| row.at_ms).collect();
        (run_ms >= RATE_AFTER_MS)
            .then(|| rate_at(&times, run_ms))
            .flatten()
    }
}

/// How long after Resume the callers come back: a moment to get the ear in.
const RESUME_DELAY_MS: u64 = 1_000;

/// Once this many QSOs are logged, [`DUPE_SHARE`] of the callers who join
/// are stations worked before (§4.7 of the design).
const DUPES_AFTER: u32 = 5;
const DUPE_SHARE: f64 = 0.02;

/// In a pile-up, your TU signs with your call every this many QSOs, as
/// MorseRunner's does (`Contest.pas:468-474`), so a station tuning in
/// knows who is running.
const TU_SIGNS_EVERY: u32 = 3;

/// The stream, forked off the run's generator, that the run's own draws
/// come from: how many call, and who calls again. The callers' streams are
/// forked by their numbers, which never reach it.
const RUN_STREAM: u64 = u64::MAX;

/// The rate shows once the run is a minute old.
const RATE_AFTER_MS: u64 = MS_PER_MINUTE;

/// How many times the runtime tries a send, and how long past a turn's
/// length it waits for each attempt before it tries again (`player.rs:28`,
/// `wait.rs:18`).
const SEND_ATTEMPTS: u64 = 3;
const ATTEMPT_GRACE_MS: u64 = 2_000;

/// What you have heard of a station and asked of it: what its groups are
/// built from.
#[derive(Clone, Debug, Default, PartialEq)]
struct Heard {
    /// When it was first heard, in run time.
    first: Option<u64>,
    /// A send of its call has begun to reach you: one you ask for now is a
    /// repeat.
    call_begun: bool,
    /// The ends of the sends of its call heard whole that its copy may be
    /// timed from, in order: the first, and each a key of yours drew (CQ,
    /// TU, `?`, a partial), but not one sent on its timeout.
    call_ends: Vec<u64>,
    /// For each repeat of its call you asked for and heard, before you sent
    /// it with your exchange, the Call field as read at the key, without
    /// `?`.
    call_asks: Vec<String>,
    /// How long the copy of its call took as you last sent it, before you
    /// sent it with your exchange.
    call_ms: Option<u64>,
    /// The call you first sent it with your exchange, and how long its copy
    /// took: from the end of its call heard last before the Call field's
    /// last edit, to that edit.
    first_call: Option<(String, u64)>,
    /// Its exchange has begun to reach you: one you ask for now is a
    /// repeat, and what you log of it is a copy.
    exch_begun: bool,
    /// The ends of its exchanges heard whole that their copy may be timed
    /// from, in order: the first, and each a key of yours drew.
    exch_ends: Vec<u64>,
    /// For each repeat of its exchange a key of yours drew and you heard,
    /// the Exchange field as read at the key.
    exch_asks: Vec<Reading>,
    /// A call you sent was taken for its own, or it answered your partial.
    engaged: bool,
    /// It has its groups: its QSO was logged, or written off as lost, or a
    /// NIL's groups are against it.
    recorded: bool,
    /// The Call field as last seen while you worked it, without `?`.
    typed: String,
    /// The call last sent that it took for its own or answered, without
    /// `?`: what a message of yours with no call in it goes with.
    call_sent: String,
}

impl Heard {
    fn note_typed(&mut self, call: &str) {
        let typed: String = normalize_call(call).chars().filter(|c| *c != '?').collect();
        if !typed.is_empty() {
            self.typed = typed;
        }
    }

    /// The repeats of its call you asked for while the Call field read
    /// otherwise than `copy`: one asked with the call already typed as it
    /// was copied changes nothing, as for a field.
    fn call_repeats(&self, copy: &str) -> u32 {
        let asks = self.call_asks.iter().filter(|then| *then != copy).count();
        u32::try_from(asks).unwrap_or(u32::MAX)
    }

    /// The repeats of its call and of its exchange you asked for and heard.
    fn asked(&self) -> u32 {
        u32::try_from(self.call_asks.len() + self.exch_asks.len()).unwrap_or(u32::MAX)
    }

    /// A group for each field of `sent`, logged as `logged` and read as
    /// `reading`, the field first read complete at `done_run`: as keyed,
    /// once its exchange has begun to reach you, and a miss before that,
    /// as a guess is no copy.
    fn field_groups(
        &self,
        sent: &[(Field, String)],
        logged: &QsoText,
        reading: &Reading,
        done_run: u64,
        voice: &StationVoice,
    ) -> Vec<(GroupResult, SessionTiming)> {
        if !self.exch_begun {
            return misses(sent, voice);
        }
        // The exchange is timed from the end of the last one heard that the
        // clock runs from, before the field first read complete, shared
        // among the fields by length.
        let exch_ms = copy_ms(&self.exch_ends, done_run);
        let length = sent
            .iter()
            .map(|(_, token)| token.chars().count())
            .sum::<usize>()
            .max(1) as u64;
        sent.iter()
            .map(|(field, token)| {
                let typed = logged.value(*field).unwrap_or_default();
                let value = reading.value(*field);
                // A repeat counts for a field empty, or read differently,
                // after it.
                let asks = self
                    .exch_asks
                    .iter()
                    .filter(|asked| asked.value(*field).is_none_or(|then| Some(then) != value))
                    .count();
                let share = exch_ms * token.chars().count() as u64 / length;
                group(
                    token,
                    &keyed_token(*field, token, typed),
                    share,
                    1 + u32::try_from(asks).unwrap_or(u32::MAX),
                    voice,
                )
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Station {
    caller: Caller,
    heard: Heard,
}

/// The ids of the timers that are running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Timers {
    countdown: Option<u64>,
    run_end: Option<u64>,
    patience: Option<u64>,
    resume: Option<u64>,
}

/// The fields as a key read them: what a reply it drew is charged by.
#[derive(Clone, Debug, PartialEq)]
struct Keyed {
    /// The Call field, without `?`.
    call: String,
    exch: Reading,
}

/// A caller that sends in the next turn.
#[derive(Clone, Debug, PartialEq)]
struct Replying {
    station: usize,
    /// It answers your asking for it again: `?`, `NR?`, a partial, or a
    /// CQ or TU that brings a call you heard begin again.
    asked: bool,
    /// A key of yours drew it, with the fields as read then; none for a
    /// send on its timeout, or one sent again after Resume that had begun
    /// to reach you.
    keyed: Option<Keyed>,
}

impl Replying {
    /// A send of `station` that no key of yours drew.
    fn unasked(station: usize) -> Self {
        Self {
            station,
            asked: false,
            keyed: None,
        }
    }
}

/// How much of a reply on the air has been counted: nothing yet, its start
/// (what it is charged, and that it began to reach you), or all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Counted {
    Nothing,
    Begun,
    Whole,
}

/// The callers as they were before they heard the message keyed last, so
/// that Esc can take back what of it had not gone out whole. A message that
/// logs has none: a logged QSO is final.
#[derive(Clone, Debug, PartialEq)]
struct Unheard {
    /// Each station still on the frequency then, and the one your TU
    /// closed, by its place: no other hears anything, nor loses its QSO.
    stations: Vec<(usize, Station)>,
    /// How many stations there were: one the message brought goes with it.
    count: usize,
    working: Option<usize>,
    thanked: Option<usize>,
    /// What Enter read; none once Wipe, or an empty Call field, has started
    /// the QSO's keying again since.
    qso: Option<QsoState>,
    /// Where the message starts among the turn's parts: those before it
    /// carry on from one keyed earlier, which the callers heard then.
    from: usize,
    /// The callers it was keyed over, still sending, who never heard it.
    deaf: Vec<usize>,
    /// The fields it was keyed with, and the run time of the call's last
    /// edit.
    fields: Fields,
    edited_run: u64,
    /// Where the QSOs the message itself lost begin in the log, and where
    /// the log ended after it. Those it lost go with it; a QSO logged or
    /// lost by a key since is final, and the callers stay as they are.
    lost_from: usize,
    entries: usize,
}

/// The turn on the air.
#[derive(Clone, Debug, PartialEq)]
struct Air {
    turn: Turn,
    /// The moment it was keyed; the turn holds its run time.
    keyed_at: MonoMs,
    /// When its sound reached the ear, as last stamped.
    heard_from: MonoMs,
    /// When its first attempt started.
    first_start: Option<MonoMs>,
    /// Your message's parts, each with the byte in your text it ends before.
    parts: Vec<(MessagePart, usize)>,
    /// Where each character of your message stops sounding, for the screen
    /// to show what is still to go.
    my_chars: Vec<(usize, f64)>,
    /// Its callers' segments, in order, and what each says.
    replies: Vec<(Replying, Says)>,
    /// How much of each reply has been counted: a key that logs without
    /// sending counts what has reached you so far.
    counted: Vec<Counted>,
    /// Log rows whose check shows once it is off the air.
    pending: Vec<usize>,
    unheard: Option<Unheard>,
}

impl Air {
    /// Run time when its sound reached the ear. No pause can come between
    /// its key and its start without taking it off the air.
    fn heard_run(&self) -> u64 {
        self.turn.at_run_ms + self.heard_from.since(self.keyed_at)
    }

    /// The callers a pause `why` at `run_now` cuts short, to send again
    /// after Resume. A send that had not begun to reach you goes as it was
    /// keyed, so a repeat you asked for is charged once you hear it. One
    /// that had begun goes as no send you asked for, unless you chose the
    /// pause: then, as one heard whole, it is owed on its timeout, so a
    /// pause is no free `?`.
    ///
    /// A pile-up caller carried on from an earlier turn had begun already,
    /// even when its rest had not reached the ear yet.
    fn cut_short(&self, run_now: u64, why: PauseWhy) -> Vec<Replying> {
        let heard_run = self.heard_run();
        let callers = self
            .turn
            .segments
            .iter()
            .filter(|segment| segment.who != Sender::You);
        callers
            .zip(self.replies.iter().zip(&self.counted))
            .filter_map(|(segment, ((reply, _), counted))| {
                let start = heard_run + whole_ms(segment.start_sec);
                let end = heard_run + whole_ms(segment.end_sec);
                if start >= run_now && *counted == Counted::Nothing {
                    Some(reply.clone())
                } else if end <= run_now || why == PauseWhy::You {
                    None
                } else {
                    Some(Replying::unasked(reply.station))
                }
            })
            .collect()
    }

    /// Each caller that sent in it by `run_now`, with when that send ended,
    /// or `run_now` for one still sending: when its wait for you starts.
    fn sent_by(&self, run_now: u64) -> Vec<(usize, u64)> {
        let heard_run = self.heard_run();
        let callers = self
            .turn
            .segments
            .iter()
            .filter(|segment| segment.who != Sender::You);
        callers
            .zip(&self.replies)
            .filter(|(segment, _)| heard_run + whole_ms(segment.start_sec) < run_now)
            .map(|(segment, (reply, _))| {
                let end = heard_run + whole_ms(segment.end_sec);
                (reply.station, end.min(run_now))
            })
            .collect()
    }

    /// While the runtime may be trying its sound again at `now`, when its
    /// last attempt would have failed: an attempt after the first has
    /// started, or the one playing has run past its length and grace. Till
    /// then a failure may still give the time since its first start back
    /// as paused.
    fn retrying_until(&self, now: MonoMs) -> Option<MonoMs> {
        let attempt_ms = whole_ms(self.turn.duration_sec()) + ATTEMPT_GRACE_MS;
        let first = self.first_start.unwrap_or(self.keyed_at);
        let retried = self.heard_from > first;
        let overdue = now.since(self.heard_from) > attempt_ms;
        let until = first.plus(SEND_ATTEMPTS * attempt_ms);
        ((retried || overdue) && now < until).then_some(until)
    }
}

/// What a turn taken off the air leaves to send: the rest of your message
/// and its parts, the log rows that wait on it, and in a pile-up the
/// callers still sending.
#[derive(Clone, Debug, Default, PartialEq)]
struct Carried {
    text: String,
    parts: Vec<(MessagePart, usize)>,
    pending: Vec<usize>,
    callers: Vec<Overlapping>,
}

/// A pile-up caller still sending when you key over it: it goes on from the
/// character sounding then, sent again whole, as your own message does, so
/// the two of you are heard over each other.
#[derive(Clone, Debug, PartialEq)]
struct Overlapping {
    reply: Replying,
    says: Says,
    text: String,
}

/// A logged QSO, as the log shows it.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    call: String,
    fields: Vec<(Field, String)>,
    wpm: Option<f64>,
    asked: u32,
    check: QsoCheck,
    /// Its check waits until the turn carrying its TU is off the air.
    pending: bool,
}

/// One QSO of the run's log, with its groups; logged ones have a row.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    record: QsoRecord,
    groups: Vec<(GroupResult, SessionTiming)>,
    row: Option<Row>,
    /// The station it was with, or its groups are against.
    station: Option<usize>,
}

/// What a key does: what it sends, whether it logs or wipes, and where the
/// cursor goes.
#[derive(Clone, Debug, Default, PartialEq)]
struct Action {
    message: Vec<MessagePart>,
    logs: bool,
    wipe: bool,
    focus: Option<EntryField>,
}

#[derive(Clone, Debug)]
pub struct ContestMachine<S> {
    /// The settings the run started with.
    settings: TrainingSettings,
    rules: &'static ContestRules,
    side: Side,
    my_call: String,
    /// Your name, for a contest whose exchange sends it.
    my_name: String,
    /// One caller at a time, or a pile-up.
    style: RunStyle,
    /// The band's fading, which each caller follows from a point of its
    /// own; none with the Band card's QSB off.
    qsb: Option<Qsb>,
    source: S,
    /// Each caller draws from a generator forked off this one by its number.
    rng: SplitMix64,
    /// The run's own draws: how many call, and who calls again.
    draws: SplitMix64,
    phase: ContestPhase,
    /// None until the countdown is over.
    clock: Option<RunClock>,
    /// Each pause that is over: when it ended, and how long it lasted.
    pauses: Vec<(MonoMs, u64)>,
    limit_ms: u64,
    /// One counter for every id handed out: turns and timers.
    next_id: u64,
    timers: Timers,
    air: Option<Air>,
    qso: QsoState,
    /// The station you are working, by its place in `stations`.
    working: Option<usize>,
    stations: Vec<Station>,
    /// The callers a pause or a failed send cut short, who send again after
    /// Resume.
    cut_short: Vec<Replying>,
    /// The station your TU without logging closed, its QSO waiting to be
    /// logged until another is, or you work another station.
    thanked: Option<usize>,
    entries: Vec<Entry>,
    /// How many of the QSOs the call source has been taught.
    taught: usize,
    /// Your next serial: QSOs logged, plus one.
    serial: u32,
    /// − / + for the run, in WPM.
    speed_offset: f64,
    /// The speed of each message you sent.
    sent_wpm: Vec<f64>,
    /// Run time and time paused, once the run is over.
    ended: Option<(u64, u64)>,
}

impl<S: CallSource> ContestMachine<S> {
    /// Start a run of `settings`, its callers drawn from `seed` and their
    /// calls from `source`: the first CQ goes out once the countdown the
    /// settings ask for has run, at once if they ask for none.
    pub fn start(
        seed: u64,
        now: MonoMs,
        settings: TrainingSettings,
        source: S,
    ) -> (Self, Vec<ContestEffect>) {
        // A contest whose phase has not shipped runs as the first that has;
        // no screen starts one.
        let rules = settings.contest.contest_kind.rules().unwrap_or(&RULES[0]);
        let my_call = settings.contest.my_call.clone();
        let my_name = settings.contest.my_name.clone();
        let countdown = settings.playback.countdown_sec;
        let rng = SplitMix64::new(seed);
        let mut machine = Self {
            side: Side::of_call(&my_call),
            limit_ms: u64::from(settings.contest.limit_minutes()) * MS_PER_MINUTE,
            style: settings.contest.contest_style,
            qsb: Qsb::of(&settings.band),
            settings,
            rules,
            my_call,
            my_name,
            source,
            draws: rng.fork(RUN_STREAM),
            rng,
            phase: ContestPhase::Countdown { left: countdown },
            clock: None,
            pauses: Vec::new(),
            next_id: 1,
            timers: Timers::default(),
            air: None,
            qso: QsoState::default(),
            working: None,
            stations: Vec::new(),
            cut_short: Vec::new(),
            thanked: None,
            entries: Vec::new(),
            taught: 0,
            serial: 1,
            speed_offset: 0.0,
            sent_wpm: Vec::new(),
            ended: None,
        };
        let effects = if countdown == 0 {
            machine.begin_run(now)
        } else {
            vec![machine.countdown_step()]
        };
        machine.note_callers();
        (machine, effects)
    }

    pub fn phase(&self) -> ContestPhase {
        self.phase
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, ContestPhase::Finished | ContestPhase::Aborted)
    }

    #[must_use = "the effects are the run's instructions to the runtime"]
    pub fn apply(&mut self, event: ContestEvent, now: MonoMs) -> Vec<ContestEffect> {
        if self.is_terminal() {
            return Vec::new();
        }
        let effects = self.handle(event, now);
        self.teach();
        self.note_callers();
        effects
    }

    /// Note for Enter whether anyone is calling you or working with you.
    fn note_callers(&mut self) {
        self.qso.nobody_calling = !self.stations.iter().any(|s| s.caller.state.active());
    }

    fn handle(&mut self, event: ContestEvent, now: MonoMs) -> Vec<ContestEffect> {
        match event {
            // A start says only when the sound reached the ear; checking the
            // limit on it would end a run on a retry's stamp.
            ContestEvent::TurnStarted { id, heard_from } => {
                if let Some(air) = self.air.as_mut().filter(|air| air.turn.id == id) {
                    air.heard_from = heard_from;
                    air.first_start.get_or_insert(heard_from);
                }
                return Vec::new();
            }
            ContestEvent::Abort => return self.abort(),
            // Both are credited as paused before the limit is looked at, so
            // time that was not run time cannot end the run.
            ContestEvent::TurnFailed { id } if self.on_air(id) => return self.sound_failed(now),
            ContestEvent::Pause { why, at } => return self.pause(why, at, now),
            _ => {}
        }
        // While the sound on the air may be being tried again, the limit is
        // only looked at: a failure would give the retries back as paused,
        // so then only what keeps its reading ends the run.
        if self.time_up(now) && (self.keeps_reading(&event) || self.retrying_until(now).is_none()) {
            return self.finish(now);
        }
        match event {
            ContestEvent::Key { key, fields } => self.key(key, &fields, now),
            ContestEvent::TurnEnded { id } if self.on_air(id) => self.turn_ended(now),
            ContestEvent::TimerFired { id } => self.timer_fired(id, now),
            ContestEvent::Resume => self.resume(now),
            ContestEvent::Speed { delta } => {
                self.shift_speed(delta);
                Vec::new()
            }
            ContestEvent::FinishNow => self.finish(now),
            // Another turn's news, or a cancel: a stopped or replaced send.
            _ => Vec::new(),
        }
    }

    /// The run as the screen shows it. Nothing in it moves with the time
    /// alone: the screen reads the clock and the rate off it as it ticks.
    pub fn view(&self) -> ContestView {
        let log: Vec<LogRow> = self
            .entries
            .iter()
            .filter_map(|entry| Some((entry, entry.row.as_ref()?)))
            .zip(1..)
            .map(|((entry, row), n)| LogRow {
                n,
                at_ms: entry.record.t,
                call: row.call.clone(),
                fields: row.fields.clone(),
                wpm: row.wpm.filter(|_| !row.pending),
                asked: row.asked,
                status: if row.pending {
                    RowStatus::Pending
                } else {
                    RowStatus::Checked(row.check.clone())
                },
            })
            .collect();
        let clean = log
            .iter()
            .filter(|row| matches!(&row.status, RowStatus::Checked(check) if check.is_clean()))
            .count();
        ContestView {
            phase: self.phase,
            clock: self.clock,
            limit_ms: self.limit_ms,
            my_wpm: self.my_wpm(),
            qso: self.qso.clone(),
            air: self.air.as_ref().map(|air| AirView {
                heard_from: air.heard_from,
                my_text: air
                    .turn
                    .segments
                    .iter()
                    .find(|segment| segment.who == Sender::You)
                    .map(|segment| segment.text.clone())
                    .unwrap_or_default(),
                my_chars: air.my_chars.clone(),
                spans: air
                    .turn
                    .segments
                    .iter()
                    .map(|segment| (segment.who, segment.start_sec, segment.end_sec))
                    .collect(),
            }),
            qsos: u32::try_from(log.len()).unwrap_or(u32::MAX),
            clean: u32::try_from(clean).unwrap_or(u32::MAX),
            serial: self.serial_sent(),
            log,
        }
    }

    /// The finished run, as it is stored: `finished_at` is wall time, the
    /// moment it ended; `date` the local day it counts towards.
    pub fn result(&self, finished_at: u64, date: String) -> SessionResult {
        let (run_ms, paused_ms) = self.ended.unwrap_or_default();
        let my_wpm = if self.sent_wpm.is_empty() {
            self.my_wpm()
        } else {
            self.sent_wpm.iter().sum::<f64>() / self.sent_wpm.len() as f64
        };
        let run = ContestRun {
            v: LAYOUT,
            kind: self.rules.kind.name().to_string(),
            style: self.style.name().to_string(),
            callers: match self.style {
                RunStyle::Single => [1, 1],
                RunStyle::Pileup => {
                    let (fewest, most) = self.stations_range();
                    [fewest, most]
                }
            },
            side: if self.rules.sided() {
                self.side.name().to_string()
            } else {
                String::new()
            },
            my_country: country_of(&self.my_call).unwrap_or_default().to_string(),
            // Continents and zones come with the contests that score by
            // them; WPX does not.
            my_continent: String::new(),
            my_zone: 0,
            fields: self
                .rules
                .copied(self.side)
                .iter()
                .map(|field| field.name().to_string())
                .collect(),
            limit_s: u32::try_from(self.limit_ms / 1_000).unwrap_or(u32::MAX),
            run_ms,
            paused_ms,
            my_wpm: tenth(my_wpm),
            assisted: false,
            qsos: self
                .entries
                .iter()
                .map(|entry| entry.record.clone())
                .collect(),
        };
        let groups = self
            .entries
            .iter()
            .flat_map(|entry| entry.groups.iter().cloned())
            .collect();
        run.result(groups, &self.settings, finished_at, date)
    }

    /// For each station you worked, or that a QSO of the run is with, how
    /// many of the run's QSOs own groups for it: one station, one copy, so
    /// exactly one once the run is over.
    #[cfg(test)]
    pub(crate) fn copies_by_station(&self) -> std::collections::BTreeMap<usize, usize> {
        let mut copies: std::collections::BTreeMap<usize, usize> = (0..self.stations.len())
            .filter(|index| self.stations[*index].heard.engaged)
            .map(|index| (index, 0))
            .collect();
        for entry in &self.entries {
            if let Some(station) = entry.station {
                *copies.entry(station).or_insert(0) += usize::from(!entry.groups.is_empty());
            }
        }
        copies
    }

    fn on_air(&self, id: u64) -> bool {
        self.air.as_ref().is_some_and(|air| air.turn.id == id)
    }

    /// Whether `event` keeps the run time it reads: a key, End run, or the
    /// end of the turn on the air. Anything else only looks (6.3).
    fn keeps_reading(&self, event: &ContestEvent) -> bool {
        match event {
            ContestEvent::Key { .. } | ContestEvent::FinishNow => true,
            ContestEvent::TurnEnded { id } => self.on_air(*id),
            _ => false,
        }
    }

    /// While the runtime may be trying the sound on the air again at `now`,
    /// when its last attempt would have failed.
    fn retrying_until(&self, now: MonoMs) -> Option<MonoMs> {
        self.air.as_ref().and_then(|air| air.retrying_until(now))
    }

    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn start_timer(&mut self, ms: u64) -> (u64, ContestEffect) {
        let id = self.fresh_id();
        let ms = u32::try_from(ms).unwrap_or(u32::MAX).max(1);
        (id, ContestEffect::StartTimer { id, ms })
    }

    fn countdown_step(&mut self) -> ContestEffect {
        let (id, effect) = self.start_timer(u64::from(COUNTDOWN_STEP_MS));
        self.timers.countdown = Some(id);
        effect
    }

    /// Run time at `now`, pauses left out; none before the run starts.
    fn run_ms(&mut self, now: MonoMs) -> u64 {
        self.clock.as_mut().map_or(0, |clock| clock.elapsed_ms(now))
    }

    /// Run time at `stamp`, a moment before `now` the fields were stamped
    /// at: the time between them, less the pauses in it. The fields are
    /// locked while paused, but what was typed before a pause can be sent
    /// after it.
    fn run_at(&mut self, stamp: MonoMs, now: MonoMs) -> u64 {
        let paused: u64 = self
            .pauses
            .iter()
            .map(|(ended, length)| {
                let began = MonoMs(ended.0.saturating_sub(*length));
                (*ended).min(now).since(began.max(stamp))
            })
            .sum();
        self.run_ms(now)
            .saturating_sub(now.since(stamp).saturating_sub(paused))
    }

    /// Whether the limit has passed at `now`. Only a look at the clock: an
    /// event that stamps nothing (a forgotten timer waking, a stale send's
    /// news) leaves a failed send free to give back its retries.
    fn time_up(&self, now: MonoMs) -> bool {
        self.clock
            .is_some_and(|clock| clock.peek_ms(now) >= self.limit_ms)
    }

    /// The speed you send at: the middle of the character speed range,
    /// moved by − / +.
    fn my_wpm(&self) -> f64 {
        let playback = &self.settings.playback;
        (playback.char_wpm_min + playback.char_wpm_max) / 2.0 + self.speed_offset
    }

    /// The settings a caller who arrives now is drawn from: the speed
    /// ranges moved by − / +.
    fn callers_settings(&self) -> TrainingSettings {
        let mut settings = self.settings.clone();
        let playback = &mut settings.playback;
        playback.char_wpm_min += self.speed_offset;
        playback.char_wpm_max += self.speed_offset;
        playback.effective_wpm_min += self.speed_offset;
        playback.effective_wpm_max += self.speed_offset;
        settings
    }

    /// Move − / + by `delta` WPM, as far as keeps both speed ranges inside
    /// the Speed card's bounds.
    fn shift_speed(&mut self, delta: i32) {
        let playback = &self.settings.playback;
        let lowest = WPM_MIN - playback.char_wpm_min.min(playback.effective_wpm_min);
        let highest = WPM_MAX - playback.char_wpm_max.max(playback.effective_wpm_max);
        self.speed_offset =
            (self.speed_offset + f64::from(delta)).clamp(lowest.min(0.0), highest.max(0.0));
    }

    /// The clock starts, the run's end is set, and your first CQ goes out.
    fn begin_run(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        self.clock = Some(RunClock::start(now));
        self.phase = ContestPhase::Running;
        let (id, end) = self.start_timer(self.limit_ms);
        self.timers.run_end = Some(id);
        let mut effects = vec![end];
        effects.extend(self.send(vec![MessagePart::Cq], &Fields::default(), now, false));
        effects
    }

    fn timer_fired(&mut self, id: u64, now: MonoMs) -> Vec<ContestEffect> {
        let timers = self.timers;
        if timers.countdown == Some(id) {
            self.timers.countdown = None;
            return match self.phase {
                ContestPhase::Countdown { left } if left > 1 => {
                    self.phase = ContestPhase::Countdown { left: left - 1 };
                    vec![self.countdown_step()]
                }
                ContestPhase::Countdown { .. } => self.begin_run(now),
                _ => Vec::new(),
            };
        }
        if timers.run_end == Some(id) {
            // The sound may yet fail and give its retries back: the run
            // waits for the turn to end or fail, or for its last attempt.
            if let Some(until) = self.retrying_until(now) {
                let (id, end) = self.start_timer(until.since(now));
                self.timers.run_end = Some(id);
                return vec![end];
            }
            return self.finish(now);
        }
        let quiet = self.phase == ContestPhase::Running && self.air.is_none();
        if timers.patience == Some(id) {
            self.timers.patience = None;
            if quiet {
                return self.patience_ran_out(now);
            }
        }
        if timers.resume == Some(id) {
            self.timers.resume = None;
            if quiet {
                return self.callers_again(now);
            }
        }
        Vec::new()
    }

    fn key(&mut self, key: OpKey, fields: &Fields, now: MonoMs) -> Vec<ContestEffect> {
        if self.phase != ContestPhase::Running {
            return Vec::new();
        }
        if key == OpKey::StopTx {
            return self.stop_sending(now);
        }
        if let Some(working) = self.working {
            self.stations[working].heard.note_typed(&fields.call);
        }
        // The Call field emptied starts the QSO's keying again, as
        // MorseRunner's `Edit1Change` resets C and N (`Main.pas:2063-2068`).
        if normalize_call(&fields.call).is_empty() {
            self.forget_qso();
        }
        let action = self.action(key, fields);
        let mut effects = Vec::new();
        if action.wipe {
            self.forget_qso();
            effects.push(ContestEffect::ClearFields);
        }
        if !action.message.is_empty() {
            effects.extend(self.send(action.message, fields, now, action.logs));
        } else if action.logs {
            // Logged without sending: what of the turn on the air has reached
            // you by now counts first, and the check shows at once.
            let run_ms = self.run_ms(now);
            if let Some(mut air) = self.air.take() {
                self.count_heard(&mut air, Some(run_ms), run_ms, &mut effects);
                self.air = Some(air);
            }
            let row = self.log(fields, run_ms, now);
            self.resolve(&[row], &mut effects);
        }
        if action.logs {
            effects.push(ContestEffect::ClearFields);
            effects.push(ContestEffect::Focus(EntryField::Call));
        } else if let Some(field) = action.focus {
            effects.push(ContestEffect::Focus(field));
        }
        effects
    }

    /// Enter starts the QSO afresh: no call sent, no exchange taken,
    /// nothing corrected; and Esc does not bring back what it read.
    fn forget_qso(&mut self) {
        self.qso = QsoState::default();
        if let Some(unheard) = self.air.as_mut().and_then(|air| air.unheard.as_mut()) {
            unheard.qso = None;
        }
    }

    /// What `key` does with the fields as `fields` hold them.
    fn action(&self, key: OpKey, fields: &Fields) -> Action {
        let call = normalize_call(&fields.call);
        let whole = parse_callsign(&call).is_some();
        let loggable = loggable(fields, self.rules, self.side);
        let only = |part: MessagePart| Action {
            message: vec![part],
            ..Action::default()
        };
        match key {
            OpKey::Esm => {
                let enter = esm(&self.qso, fields, self.rules, self.side);
                Action {
                    logs: enter.logs(),
                    focus: enter.focus,
                    message: enter.message,
                    wipe: false,
                }
            }
            OpKey::Cq => only(MessagePart::Cq),
            OpKey::Exchange => only(MessagePart::Exchange),
            OpKey::Tu => only(MessagePart::Tu),
            OpKey::MyCall => only(MessagePart::MyCall),
            // A dupe calling has not heard its call from you yet: B4 sends
            // the call typed first, as TU and log does, so it knows it is
            // meant.
            OpKey::B4 => {
                let mut message = Vec::new();
                if whole && call != self.qso.call_sent {
                    message.push(MessagePart::HisCall(call));
                }
                message.push(MessagePart::B4);
                Action {
                    message,
                    ..Action::default()
                }
            }
            OpKey::Again => only(MessagePart::Again),
            OpKey::Nil => only(MessagePart::Nil),
            OpKey::NrQuery => only(MessagePart::NrQuery),
            OpKey::HisCall if call.is_empty() => Action::default(),
            OpKey::HisCall => only(MessagePart::HisCall(call)),
            OpKey::HisCallAndExchange => {
                let mut message = Vec::new();
                if !call.is_empty() {
                    message.push(MessagePart::HisCall(call));
                }
                message.push(MessagePart::Exchange);
                Action {
                    message,
                    focus: whole.then_some(EntryField::Exchange),
                    ..Action::default()
                }
            }
            // TU and log sends the call again first when it has changed
            // since it went out, as Enter does; after F3 its TU went to this
            // call, which MorseRunner still counts as sent.
            OpKey::TuAndLog if loggable => {
                let mut message = Vec::new();
                if call != self.qso.call_sent && call != self.qso.thanked {
                    message.push(MessagePart::HisCall(call));
                }
                message.push(MessagePart::Tu);
                Action {
                    message,
                    logs: true,
                    ..Action::default()
                }
            }
            OpKey::LogOnly if loggable => Action {
                logs: true,
                ..Action::default()
            },
            OpKey::Wipe => Action {
                wipe: true,
                focus: Some(EntryField::Call),
                ..Action::default()
            },
            OpKey::TuAndLog | OpKey::LogOnly | OpKey::StopTx => Action::default(),
        }
    }

    /// Send `message`: the callers hear it, a QSO is logged if it `logs`,
    /// and the turn with their replies goes out. A turn on the air gives
    /// way to it: your message carries on from the character sounding at
    /// the key, and a single caller stops and hears you. A pile-up caller
    /// still sending goes on over you and never hears it, but for a CQ, a
    /// TU or a NIL, which it hears from a fragment (`DxStn.pas:240-253`).
    fn send(
        &mut self,
        message: Vec<MessagePart>,
        fields: &Fields,
        now: MonoMs,
        logs: bool,
    ) -> Vec<ContestEffect> {
        let run_ms = self.run_ms(now);
        let edited_run = self.run_at(fields.call_edited, now);
        // Your exchange carries this QSO's serial, taken before a TU in the
        // same message logs it, and so does the number of the QSO its TU
        // closes.
        let serial = self.serial_sent();
        let number = self.serial;
        let mut effects = Vec::new();
        let carried = match self.air.take() {
            Some(air) => self.cut(air, run_ms, &mut effects),
            None => Carried::default(),
        };
        let fragment = message
            .iter()
            .any(|part| matches!(part, MessagePart::Cq | MessagePart::Tu | MessagePart::Nil));
        let deaf: Vec<usize> = if fragment {
            Vec::new()
        } else {
            carried
                .callers
                .iter()
                .map(|over| over.reply.station)
                .collect()
        };
        // What you send is heard instead of the callers coming back after
        // a Resume, and they wait for you afresh: nobody's timeout runs while
        // you send, and each starts its next once it has sent, or heard you
        // (`DxStn.pas:204-208, 292-301`).
        self.timers.patience = None;
        self.timers.resume = None;
        self.cut_short.clear();
        for station in &mut self.stations {
            station.caller.deadline = None;
        }
        // The one your TU closed is kept too: a message that works another
        // station loses its QSO, which Esc takes back.
        let mut before = Unheard {
            stations: self
                .stations
                .iter()
                .enumerate()
                .filter(|(index, station)| {
                    station.caller.state.active() || self.thanked == Some(*index)
                })
                .map(|(index, station)| (index, station.clone()))
                .collect(),
            count: self.stations.len(),
            working: self.working,
            thanked: self.thanked,
            qso: Some(self.qso.clone()),
            from: carried.parts.len(),
            deaf: deaf.clone(),
            fields: fields.clone(),
            edited_run,
            lost_from: 0,
            entries: 0,
        };
        // A `?` with the Call field empty is for anyone still calling.
        let asks = message
            .iter()
            .any(|part| matches!(part, MessagePart::Again | MessagePart::NrQuery));
        if asks && normalize_call(&fields.call).is_empty() {
            for station in &mut self.stations {
                station.caller.forget_other_qso();
            }
        }
        let worked = self.working;
        self.note_sent(&message);
        let (mut replying, took) = self.hear(&message, fields, edited_run, &deaf);
        if took {
            self.qso.exchange_sent = true;
        }
        // The station you worked, or one you called in this message, sent
        // away by what you sent (B4, NIL, or its last try spent), ends the
        // QSO as its leaving on its timeouts does.
        let gone = |index: usize| self.stations[index].caller.state == CallerState::Gone;
        if [worked, self.working].into_iter().flatten().any(gone) {
            self.qso = QsoState::default();
        }
        if !logs {
            self.thank(worked, &message, fields, true);
        }
        let mut pending = carried.pending;
        if logs {
            pending.push(self.log(fields, run_ms, now));
        }
        if self.brings_callers(&message) {
            // Their first call: nothing to time it from, nothing asked.
            let sounding: Vec<usize> = carried
                .callers
                .iter()
                .map(|over| over.reply.station)
                .collect();
            let joined = self.arrivals(&mut before.stations, &sounding, run_ms);
            replying.extend(joined.into_iter().map(Replying::unasked));
        }
        before.lost_from = self.entries.len();
        self.close_finished(run_ms);
        before.entries = self.entries.len();
        let (mut text, mut parts) = (carried.text, carried.parts);
        // A TU that greets the station by name greets it as you copied it.
        let his_name = parse_exchange(self.rules, self.side, &fields.exch)
            .value(Field::Name)
            .unwrap_or_default()
            .to_string();
        let me = Fill {
            my_call: &self.my_call,
            my_name: &self.my_name,
            serial,
            his_name: &his_name,
        };
        // In a pile-up, a TU signs with your call every third QSO.
        let signs = self.style == RunStyle::Pileup
            && self.rules.tu_calls
            && number.is_multiple_of(TU_SIGNS_EVERY)
            && !self.my_call.is_empty();
        for part in message {
            let mut said = message_text(std::slice::from_ref(&part), self.rules, self.side, &me);
            if part == MessagePart::Tu && signs {
                said = format!("{said} {}", self.my_call);
            }
            if said.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&said);
            parts.push((part, text.len()));
        }
        let mine = Carried {
            text,
            parts,
            pending,
            callers: carried.callers,
        };
        let unheard = (!logs).then_some(before);
        effects.extend(self.play(mine, unheard, replying, &deaf, run_ms, now));
        effects
    }

    /// Whether `message` calls for the next caller: a CQ, or a TU in a
    /// contest, where it works as one.
    fn calls(&self, message: &[MessagePart]) -> bool {
        message.iter().any(|part| match part {
            MessagePart::Cq => true,
            MessagePart::Tu => self.rules.tu_calls,
            _ => false,
        })
    }

    /// Whether new callers join after `message`: after a CQ, and a TU where
    /// it works as one, and in a pile-up after your call alone once a QSO
    /// is in the log, as MorseRunner's do (`Contest.pas:1001-1025`).
    fn brings_callers(&self, message: &[MessagePart]) -> bool {
        let my_call = self.style == RunStyle::Pileup
            && self.serial > 1
            && message.contains(&MessagePart::MyCall);
        self.calls(message) || my_call
    }

    /// What Enter reads after `message`: a CQ or TU ends the QSO, as
    /// logging does; a call is the call last sent, and not corrected yet.
    /// Whether your exchange was taken is for the callers to say.
    fn note_sent(&mut self, message: &[MessagePart]) {
        for part in message {
            match part {
                MessagePart::Cq | MessagePart::Tu => self.qso = QsoState::default(),
                MessagePart::HisCall(call) => {
                    self.qso.call_sent = call.clone();
                    self.qso.corrected = false;
                    self.qso.thanked.clear();
                }
                _ => {}
            }
        }
    }

    /// The callers hear `message`, keyed with `fields`, the call last edited
    /// at `edited_run`, all but the `deaf`, who were still sending: who you
    /// work now, the call each took with your exchange, and who replies;
    /// and whether any of them took your exchange. A call goes only to the
    /// stations it fits best ([`Likeness`]), among all on the frequency,
    /// those still sending too, as MorseRunner ranks them
    /// (`StnColl.pas:171-194`): a call keyed over its own station is no
    /// near call's.
    fn hear(
        &mut self,
        message: &[MessagePart],
        fields: &Fields,
        edited_run: u64,
        deaf: &[usize],
    ) -> (Vec<Replying>, bool) {
        let keyed = Keyed {
            call: normalize_call(&fields.call).replace('?', ""),
            exch: parse_exchange(self.rules, self.side, &fields.exch),
        };
        let call_sent = message.iter().rev().find_map(|part| match part {
            MessagePart::HisCall(call) => Some(call.replace('?', "")),
            _ => None,
        });
        // A single caller still calling that answers your CQ or TU, or
        // your call alone, brings its call again, as a `?` would; pile-up
        // callers, calling anyway, do not.
        let calls = self.style == RunStyle::Single
            && (self.calls(message) || message.contains(&MessagePart::MyCall));
        let hears = |index: &usize| !deaf.contains(index);
        let best: Vec<Option<Likeness>> = message
            .iter()
            .map(|part| match part {
                MessagePart::HisCall(sent) => self
                    .stations
                    .iter()
                    .map(|station| &station.caller)
                    .filter(|caller| caller.state.active())
                    .filter_map(|caller| caller.likeness(sent))
                    .reduce(|best, next| if next > best { next } else { best }),
                _ => None,
            })
            .collect();
        let mut replying = Vec::new();
        let mut took = false;
        for index in (0..self.stations.len()).filter(hears) {
            let station = &mut self.stations[index];
            let reaction = station.caller.hear(message, &best);
            let calls_again = calls && station.caller.state == CallerState::NeedQso;
            let heard = &mut station.heard;
            // A single caller is engaged once it answers anything of its
            // call. In a pile-up several may answer a partial and you work
            // one: a station is engaged once it hears its call from you, or
            // takes your exchange.
            let single = self.style == RunStyle::Single;
            heard.engaged |=
                reaction.took_exchange || (reaction.matched && (single || reaction.called));
            if reaction.matched {
                heard.note_typed(&fields.call);
                if let Some(call) = &call_sent {
                    heard.call_sent.clone_from(call);
                }
                self.working = Some(index);
                // Its call went out: how long the copy took, should it go
                // no further.
                if heard.first_call.is_none() {
                    heard.call_ms = Some(copy_ms(&heard.call_ends, edited_run));
                }
            }
            took |= reaction.took_exchange;
            // The call it took with your exchange is its copy: the one last
            // sent to it, should the message hold none.
            if reaction.took_exchange && self.working == Some(index) && heard.first_call.is_none() {
                let took_ms = copy_ms(&heard.call_ends, edited_run);
                let call = if heard.call_sent.is_empty() {
                    &heard.typed
                } else {
                    &heard.call_sent
                };
                heard.first_call = Some((call.clone(), took_ms));
            }
            if reaction.replies {
                replying.push(Replying {
                    station: index,
                    asked: reaction.asked || calls_again,
                    keyed: Some(keyed.clone()),
                });
            }
        }
        (replying, took)
    }

    /// After `message`, keyed with `fields`, which logs nothing: if its TU
    /// closed the QSO with the station you worked, `worked`, that QSO waits
    /// to be logged, as MorseRunner's log looks for a station done with you
    /// (`Log.pas:745-755`); and if `notes`, Enter logs it on its call.
    fn thank(
        &mut self,
        worked: Option<usize>,
        message: &[MessagePart],
        fields: &Fields,
        notes: bool,
    ) {
        let done = |index: &usize| self.stations[*index].caller.state == CallerState::Done;
        let Some(index) = worked
            .filter(done)
            .filter(|_| message.contains(&MessagePart::Tu))
        else {
            return;
        };
        self.thanked = Some(index);
        if notes {
            let call = normalize_call(&fields.call);
            self.qso.thanked = if call.is_empty() {
                self.stations[index].heard.call_sent.clone()
            } else {
                call
            };
        }
    }

    /// Your serial as your exchange sends it; none in a contest without one.
    fn serial_sent(&self) -> u32 {
        if self.rules.serial { self.serial } else { 0 }
    }

    /// Put a turn on the air: `mine`, then each of `replying` after its
    /// delay; `unheard`, the callers before they heard the message keyed
    /// last, and `deaf`, those it was keyed over who never heard it. A turn
    /// with nothing in it is not sent.
    fn play(
        &mut self,
        mine: Carried,
        unheard: Option<Unheard>,
        replying: Vec<Replying>,
        deaf: &[usize],
        run_ms: u64,
        now: MonoMs,
    ) -> Vec<ContestEffect> {
        let mut segments = Vec::new();
        let mut your_end = 0.0;
        let mut my_chars = Vec::new();
        if !mine.text.is_empty() {
            let voice = your_voice(&self.settings, self.my_wpm());
            your_end = sending_sec(&mine.text, &self.settings, &voice);
            my_chars = char_spans(&mine.text, &self.settings, &voice)
                .into_iter()
                .map(|(at, _, end)| (at, end))
                .collect();
            self.sent_wpm.push(voice.char_wpm);
            segments.push(Segment {
                who: Sender::You,
                text: mine.text,
                voice,
                start_sec: 0.0,
                end_sec: your_end,
                fade: None,
            });
        }
        let mut replies = Vec::new();
        let mut counted = Vec::new();
        // Callers you keyed over go on from where they were, heard over you,
        // their start charged already, and send nothing more till they are
        // done. One that never heard you and was done first sends again a
        // moment after you.
        let mut busy = Vec::new();
        let mut garbled = Vec::new();
        for over in mine.callers {
            let station = over.reply.station;
            let caller = &self.stations[station].caller;
            let end_sec = sending_sec(&over.text, &self.settings, &caller.voice);
            segments.push(Segment {
                who: Sender::Caller(u32::try_from(station).unwrap_or(u32::MAX)),
                text: over.text,
                voice: caller.voice,
                start_sec: 0.0,
                end_sec,
                fade: self.fade(caller, run_ms, 0.0),
            });
            replies.push((over.reply, over.says));
            counted.push(Counted::Begun);
            busy.push((station, end_sec));
            if deaf.contains(&station) && end_sec < your_end {
                garbled.push(station);
            }
        }
        let garbled: Vec<Replying> = garbled
            .into_iter()
            .filter(|station| self.stations[*station].caller.keyed_over())
            .map(Replying::unasked)
            .collect();
        for replying in replying.into_iter().chain(garbled) {
            let caller = &mut self.stations[replying.station].caller;
            let Some((said, says)) = caller.reply(self.rules, self.side, &self.my_call, run_ms)
            else {
                continue;
            };
            let free_sec = busy
                .iter()
                .filter(|(station, _)| *station == replying.station)
                .fold(your_end, |free, (_, end)| free.max(*end));
            let start_sec = free_sec + caller.draw_delay_sec();
            let end_sec = start_sec + sending_sec(&said, &self.settings, &caller.voice);
            let caller = &self.stations[replying.station].caller;
            segments.push(Segment {
                who: Sender::Caller(u32::try_from(replying.station).unwrap_or(u32::MAX)),
                text: said,
                voice: caller.voice,
                start_sec,
                end_sec,
                fade: self.fade(caller, run_ms, start_sec),
            });
            replies.push((replying, says));
            counted.push(Counted::Nothing);
        }
        let mut effects = Vec::new();
        if segments.is_empty() {
            self.resolve(&mine.pending, &mut effects);
            effects.extend(self.arm_patience(run_ms, &[]));
            return effects;
        }
        let turn = Turn {
            id: self.fresh_id(),
            at_run_ms: run_ms,
            segments,
        };
        self.air = Some(Air {
            turn: turn.clone(),
            keyed_at: now,
            heard_from: now,
            first_start: None,
            parts: mine.parts,
            my_chars,
            counted,
            replies,
            pending: mine.pending,
            unheard,
        });
        effects.push(ContestEffect::PlayTurn(turn));
        effects
    }

    /// Take a turn off the air at `run_now`: what was heard of it by then
    /// counts. What is left of your message from the character sounding
    /// then, sent again whole, carries on with its parts, and so do the log
    /// rows waiting on it while it still holds their TU; otherwise their
    /// checks show now.
    ///
    /// The old send stops at once and a new one starts a moment later, so
    /// the rest starts from the character sounding at the key, not when the
    /// new send can start. `heard_from` includes the player's lead and
    /// latency, so this errs toward sending a character again, never
    /// toward losing one.
    fn cut(&mut self, mut air: Air, run_now: u64, effects: &mut Vec<ContestEffect>) -> Carried {
        self.count_heard(&mut air, Some(run_now), run_now, effects);
        let heard_run = air.heard_run();
        let callers = if self.style == RunStyle::Pileup {
            self.overlapping(&air, run_now)
        } else {
            Vec::new()
        };
        let Air {
            turn,
            parts,
            pending,
            ..
        } = air;
        let rest = turn
            .segments
            .into_iter()
            .find(|segment| segment.who == Sender::You)
            .and_then(|yours| {
                let spans = char_spans(&yours.text, &self.settings, &yours.voice);
                let at_sec = (run_now as f64 - heard_run as f64) / 1_000.0 - yours.start_sec;
                let from = sounding_at(&spans, at_sec)?;
                let parts = parts
                    .into_iter()
                    .filter(|(_, end)| *end > from)
                    .map(|(part, end)| (part, end - from))
                    .collect();
                Some((yours.text[from..].to_string(), parts))
            });
        let (text, parts): (String, Vec<(MessagePart, usize)>) = rest.unwrap_or_default();
        let pending = if parts.iter().any(|(part, _)| *part == MessagePart::Tu) {
            pending
        } else {
            self.resolve(&pending, effects);
            Vec::new()
        };
        Carried {
            text,
            parts,
            pending,
            callers,
        }
    }

    /// The callers in `air` still sending at `run_now`, each with what is
    /// left of its message from the character sounding then. One carried on
    /// from an earlier turn whose rest has not reached the ear yet goes on
    /// whole: it began before, and the player's lead is no gap in it.
    fn overlapping(&self, air: &Air, run_now: u64) -> Vec<Overlapping> {
        let heard_run = air.heard_run();
        let callers = air
            .turn
            .segments
            .iter()
            .filter(|segment| segment.who != Sender::You);
        callers
            .zip(air.replies.iter().zip(&air.counted))
            .filter_map(|(segment, ((reply, says), counted))| {
                let start = heard_run + whole_ms(segment.start_sec);
                let end = heard_run + whole_ms(segment.end_sec);
                if start >= run_now && *counted != Counted::Nothing {
                    return Some(Overlapping {
                        reply: reply.clone(),
                        says: *says,
                        text: segment.text.clone(),
                    });
                }
                if start >= run_now || end <= run_now {
                    return None;
                }
                let spans = char_spans(&segment.text, &self.settings, &segment.voice);
                let at_sec = (run_now - start) as f64 / 1_000.0;
                let from = sounding_at(&spans, at_sec)?;
                Some(Overlapping {
                    reply: reply.clone(),
                    says: *says,
                    text: segment.text[from..].to_string(),
                })
            })
            .collect()
    }

    /// The fading of a segment of `caller`'s starting `start_sec` into a
    /// turn keyed at `run_ms`: the band's, read from the caller's own point
    /// in it.
    fn fade(&self, caller: &Caller, run_ms: u64, start_sec: f64) -> Option<Fade> {
        self.qsb.map(|qsb| Fade {
            qsb,
            from_sec: caller.fade_from_sec + run_ms as f64 / 1_000.0 + start_sec,
        })
    }

    /// Count what was heard of the callers in `air` and not counted yet:
    /// every segment, or what of each had reached the ear by `upto`. A
    /// repeat is paid for once it has begun to reach you, even if you key
    /// over the rest of it, and only if a send of the same had begun to
    /// reach you before: a call you asked for again, or an exchange a key
    /// of yours drew again.
    fn count_heard(
        &mut self,
        air: &mut Air,
        upto: Option<u64>,
        run_now: u64,
        effects: &mut Vec<ContestEffect>,
    ) {
        let heard_run = air.heard_run();
        let Air {
            turn,
            replies,
            counted,
            ..
        } = air;
        let callers = turn
            .segments
            .iter()
            .filter(|segment| segment.who != Sender::You);
        for ((segment, (reply, says)), counted) in callers.zip(replies.iter()).zip(counted) {
            let start = heard_run + whole_ms(segment.start_sec);
            let end = heard_run + whole_ms(segment.end_sec);
            let heard = &mut self.stations[reply.station].heard;
            if *counted == Counted::Nothing {
                if upto.is_some_and(|upto| start >= upto) {
                    continue;
                }
                *counted = Counted::Begun;
                match (says, &reply.keyed) {
                    // Its call, alone or putting yours right, as you asked.
                    (Says::Call | Says::Correction, Some(keyed))
                        if reply.asked && heard.call_begun && heard.first_call.is_none() =>
                    {
                        heard.call_asks.push(keyed.call.clone());
                    }
                    (Says::Exchange, Some(keyed)) if heard.exch_begun => {
                        heard.exch_asks.push(keyed.exch.clone());
                    }
                    _ => {}
                }
                match says {
                    Says::Call | Says::Correction => heard.call_begun = true,
                    Says::Exchange => heard.exch_begun = true,
                    Says::Query | Says::Signoff => {}
                }
            }
            if *counted == Counted::Whole || upto.is_some_and(|upto| end > upto) {
                continue;
            }
            *counted = Counted::Whole;
            heard.first.get_or_insert(start.min(run_now));
            let end = end.min(run_now);
            // A copy is timed from the first send heard, or from one a key
            // of yours drew; one it sent on its timeout is waiting, which
            // costs its time.
            let restarts = reply.keyed.is_some();
            match says {
                Says::Call if restarts || heard.call_ends.is_empty() => heard.call_ends.push(end),
                Says::Exchange if restarts || heard.exch_ends.is_empty() => {
                    heard.exch_ends.push(end);
                }
                Says::Correction => {
                    if reply.asked && restarts {
                        heard.call_ends.push(end);
                    }
                    // It puts right a whole call you sent, even in answer to
                    // a `?`; its call answering your partial, or a piece of
                    // its call, puts nothing right.
                    let sent = &self.qso.call_sent;
                    let wrong = parse_callsign(sent).is_some()
                        && *sent != self.stations[reply.station].caller.call.text;
                    if self.working == Some(reply.station) && (wrong || !reply.asked) {
                        self.qso.corrected = true;
                        effects.push(ContestEffect::Announce(
                            "They corrected you: fix the call, then Enter".to_string(),
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    /// Esc: the sound stops at once, and a call or exchange of yours that
    /// had not gone out whole counts as not sent (`Main.pas:884-891`). The
    /// callers hear only what of the message keyed last went out whole,
    /// unless a QSO has been logged since it was keyed, or lost by a key
    /// since: what the message itself lost, and a caller it brought, go
    /// with it, as MorseRunner's callers hear nothing of an aborted send
    /// (`MyStn.pas`, `AbortSend`).
    fn stop_sending(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        let Some(mut air) = self.air.take() else {
            return Vec::new();
        };
        let run_ms = self.run_ms(now);
        let mut effects = vec![ContestEffect::StopAudio];
        let unheard = air.unheard.take();
        // Where the message keyed last starts: parts carried on from an
        // earlier one were heard at its key, and are not taken back.
        let keyed_from = unheard.as_ref().map_or(0, |unheard| unheard.from);
        let parts: Vec<MessagePart> = air.parts.iter().map(|(part, _)| part.clone()).collect();
        let sent_by = air.sent_by(run_ms);
        let carried = self.cut(air, run_ms, &mut effects);
        // What is carried is the tail of the parts; the rest went out whole.
        let sent = parts.len() - carried.parts.len();
        let unheard =
            unheard.filter(|unheard| sent < parts.len() && unheard.entries == self.entries.len());
        if let Some(unheard) = unheard {
            let from = unheard.from.min(sent);
            self.unhear(unheard, &parts[from..sent], run_ms);
        }
        for (at, (part, _)) in carried.parts.iter().enumerate() {
            if sent + at < keyed_from {
                continue;
            }
            match part {
                MessagePart::HisCall(_) => self.qso.call_sent.clear(),
                MessagePart::Exchange => self.qso.exchange_sent = false,
                _ => {}
            }
        }
        self.resolve(&carried.pending, &mut effects);
        effects.extend(self.arm_patience(run_ms, &sent_by));
        effects
    }

    /// The callers as `unheard` holds them, before they heard a message Esc
    /// stopped, then hearing only the parts of it that went out whole,
    /// `sent`. What they owe for those comes on their timeouts.
    fn unhear(&mut self, unheard: Unheard, sent: &[MessagePart], run_ms: u64) {
        self.entries.truncate(unheard.lost_from);
        self.taught = self.taught.min(self.entries.len());
        self.stations.truncate(unheard.count);
        for (index, station) in unheard.stations {
            self.stations[index] = station;
        }
        self.working = unheard.working;
        self.thanked = unheard.thanked;
        let notes = unheard.qso.is_some();
        if let Some(qso) = unheard.qso {
            self.qso = qso;
            self.note_sent(sent);
        }
        let (_, took) = self.hear(sent, &unheard.fields, unheard.edited_run, &unheard.deaf);
        if took && notes {
            self.qso.exchange_sent = true;
        }
        self.thank(unheard.working, sent, &unheard.fields, notes);
        self.close_finished(run_ms);
    }

    fn turn_ended(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        let Some(mut air) = self.air.take() else {
            return Vec::new();
        };
        let run_ms = self.run_ms(now);
        let mut effects = Vec::new();
        self.count_heard(&mut air, None, run_ms, &mut effects);
        self.resolve(&air.pending, &mut effects);
        effects.extend(self.arm_patience(run_ms, &air.sent_by(run_ms)));
        effects
    }

    /// Everyone still on the frequency waits for you, each its own time, as
    /// MorseRunner's callers do (`DxStn.pas:204-208`): one that sent, as
    /// `sent` has it, from the end of its send; one that heard you and has
    /// no wait running, from now; any other keeps the wait it had, as
    /// another caller's send is nothing to it. A pile-up caller that heard
    /// you work someone else waits for that QSO to end, with no timeout.
    /// The timer runs to the first of them to give up waiting.
    fn arm_patience(&mut self, run_ms: u64, sent: &[(usize, u64)]) -> Option<ContestEffect> {
        for (index, station) in self.stations.iter_mut().enumerate() {
            let caller = &mut station.caller;
            if !caller.state.active() {
                continue;
            }
            if caller.waits() {
                caller.deadline = None;
                continue;
            }
            let ended = sent
                .iter()
                .filter(|(who, _)| *who == index)
                .map(|(_, end)| *end)
                .max();
            match ended {
                Some(end) => caller.deadline = Some(end + caller.draw_timeout_ms()),
                None if caller.deadline.is_none() => {
                    caller.deadline = Some(run_ms + caller.draw_timeout_ms());
                }
                None => {}
            }
        }
        self.rearm_patience(run_ms)
    }

    /// The patience timer, to the soonest deadline still running.
    fn rearm_patience(&mut self, run_ms: u64) -> Option<ContestEffect> {
        let soonest = self
            .stations
            .iter()
            .filter(|station| station.caller.state.active())
            .filter_map(|station| station.caller.deadline)
            .min()?;
        let (id, effect) = self.start_timer(soonest.saturating_sub(run_ms));
        self.timers.patience = Some(id);
        Some(effect)
    }

    /// Callers whose wait is over try again, or give up. The station you
    /// worked leaving is said, and Enter starts afresh, as after a CQ.
    fn patience_ran_out(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        let run_ms = self.run_ms(now);
        let mut replying = Vec::new();
        for (index, station) in self.stations.iter_mut().enumerate() {
            let caller = &mut station.caller;
            let due = caller.deadline.is_some_and(|deadline| deadline <= run_ms);
            if caller.state.active() && due && caller.time_out() {
                replying.push(Replying::unasked(index));
            }
        }
        let worked = self.working;
        self.close_finished(run_ms);
        let mut effects = Vec::new();
        let gone = |index: &usize| self.stations[*index].caller.state == CallerState::Gone;
        if let Some(index) = worked.filter(gone) {
            let typed = &self.stations[index].heard.typed;
            let who = if parse_callsign(typed).is_some() {
                typed.as_str()
            } else {
                "The station"
            };
            effects.push(ContestEffect::Announce(format!("{who} has gone")));
            self.qso = QsoState::default();
        }
        if replying.is_empty() {
            effects.extend(self.rearm_patience(run_ms));
        } else {
            effects.extend(self.play(Carried::default(), None, replying, &[], run_ms, now));
        }
        effects
    }

    /// After Resume: the callers a pause or a failed send cut short send
    /// again what they owe, each as [`Air::cut_short`] or
    /// [`Self::sound_failed`] left it. The others wait for you afresh, as
    /// after a turn: one heard whole is sent again only on its timeout, or
    /// at your asking, so a pause is no free `?`.
    fn callers_again(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        let run_ms = self.run_ms(now);
        for station in &mut self.stations {
            station.caller.deadline = None;
        }
        let mut cut_short = std::mem::take(&mut self.cut_short);
        cut_short.sort_by_key(|reply| reply.station);
        cut_short.dedup_by_key(|reply| reply.station);
        let replying: Vec<Replying> = cut_short
            .into_iter()
            .filter(|reply| self.stations[reply.station].caller.state.active())
            .collect();
        if replying.is_empty() {
            return self.arm_patience(run_ms, &[]).into_iter().collect();
        }
        self.play(Carried::default(), None, replying, &[], run_ms, now)
    }

    /// The callers who join after a CQ, or a TU that works as one: one when
    /// nobody is on the frequency, in a run of single callers; in a pile-up,
    /// as many as bring the number calling up to a fresh draw from the Band
    /// card's stations range, as far as the filter has room, the rest
    /// waiting, and clear of the callers `sounding` on from before your
    /// key. A station worked before that calls again at `run_ms` is kept in
    /// `before` as it was, for Esc to put back.
    fn arrivals(
        &mut self,
        before: &mut Vec<(usize, Station)>,
        sounding: &[usize],
        run_ms: u64,
    ) -> Vec<usize> {
        let calling = self
            .stations
            .iter()
            .filter(|station| station.caller.state.active())
            .count();
        let wanted = match self.style {
            RunStyle::Single => usize::from(calling == 0),
            RunStyle::Pileup => {
                let (fewest, most) = self.stations_range();
                let drawn = self.draws.usize_in(fewest as usize, most as usize);
                drawn
                    .min(filter_capacity(&self.settings))
                    .saturating_sub(calling)
            }
        };
        let mut joined = Vec::new();
        for _ in 0..wanted {
            match self.join(before, sounding, run_ms) {
                Some(index) => joined.push(index),
                None => break,
            }
        }
        joined
    }

    /// The pitches of the callers on the frequency, calling or worked, and
    /// of those `sounding` still, which a newcomer keeps clear of too.
    fn taken(&self, sounding: &[usize]) -> Vec<f64> {
        (0..self.stations.len())
            .filter(|index| self.stations[*index].caller.state.active() || sounding.contains(index))
            .map(|index| self.stations[index].caller.voice.tone_hz)
            .collect()
    }

    /// Station `index`, worked before, calls again at `run_ms`, where the
    /// filter has room for it beside the callers on the frequency or
    /// `sounding`: a dupe, kept in `before` as it was. Its copy
    /// is kept already, in the QSO it was logged in, so whatever comes of
    /// this one owns no groups.
    fn rejoin(
        &mut self,
        index: usize,
        before: &mut Vec<(usize, Station)>,
        sounding: &[usize],
        run_ms: u64,
    ) -> Option<usize> {
        let settings = self.callers_settings();
        let taken = self.taken(sounding);
        let placed = place_contest_caller(&settings, self.style, &taken, &mut self.draws)?;
        before.push((index, self.stations[index].clone()));
        let station = &mut self.stations[index];
        station.caller.call_again(placed.tone_hz, run_ms);
        station.heard = Heard {
            engaged: true,
            recorded: true,
            ..Heard::default()
        };
        Some(index)
    }

    /// How many call at once in a pile-up: the Band card's stations range.
    fn stations_range(&self) -> (u32, u32) {
        let band = &self.settings.band;
        let most = band.stations_max.clamp(STATIONS_MIN, STATIONS_MAX);
        (band.stations_min.clamp(STATIONS_MIN, most), most)
    }

    /// One caller joins at `run_ms` where the filter has room for it beside
    /// the callers on the frequency or `sounding`, or none when it has none:
    /// in a contest that scores, once five QSOs are
    /// logged, now and then a station worked before, a dupe, kept in
    /// `before` as it was; otherwise a new one, its call drawn from the
    /// source, none already on the frequency or in the log as far as a few
    /// draws go. Simple QSO, which is no contest, has no dupes.
    fn join(
        &mut self,
        before: &mut Vec<(usize, Station)>,
        sounding: &[usize],
        run_ms: u64,
    ) -> Option<usize> {
        let logged = self.serial - 1;
        let dupes = self.rules.scoring.is_some() && logged >= DUPES_AFTER;
        if dupes && self.draws.f64() < DUPE_SHARE {
            // Stations done with you and logged with a QSO, but none that was
            // on the frequency at your key or your TU closed: a station you
            // have just thanked does not call straight back.
            let keyed: Vec<usize> = before.iter().map(|(index, _)| *index).collect();
            let worked: Vec<usize> = (0..self.stations.len())
                .filter(|index| !keyed.contains(index))
                .filter(|index| self.stations[*index].caller.state == CallerState::Done)
                .filter(|index| {
                    self.entries.iter().any(|entry| {
                        entry.station == Some(*index)
                            && entry.row.is_some()
                            && entry.record.k.is_empty()
                    })
                })
                .collect();
            if !worked.is_empty() {
                let index = worked[self.draws.usize_in(0, worked.len() - 1)];
                return self.rejoin(index, before, sounding, run_ms);
            }
        }
        let index = self.stations.len();
        let mut rng = self.rng.fork(index as u64);
        let settings = self.callers_settings();
        let placed = place_contest_caller(&settings, self.style, &self.taken(sounding), &mut rng)?;
        let calls: Vec<String> = self
            .stations
            .iter()
            .filter(|station| station.caller.state.active())
            .map(|station| station.caller.call.text.clone())
            .chain(self.entries.iter().flat_map(|entry| {
                let station = entry
                    .station
                    .map(|index| self.stations[index].caller.call.text.clone());
                station
                    .into_iter()
                    .chain(entry.row.iter().map(|row| row.call.clone()))
            }))
            .collect();
        let avoid = |call: &str| calls.iter().any(|taken| taken == call);
        let call = self
            .source
            .next_call(&settings, self.rules.callers(self.side), &avoid);
        let caller = Caller::arrive(
            call,
            &settings,
            self.rules,
            self.settings.contest.limit_minutes(),
            self.style,
            placed,
            rng,
        );
        self.stations.push(Station {
            caller,
            heard: Heard::default(),
        });
        Some(index)
    }

    /// A station you worked that is done or gone without its QSO in the log
    /// is a lost copy, but for one your TU closed until you work another;
    /// the one you work, once it leaves, is no longer it.
    fn close_finished(&mut self, run_ms: u64) {
        if self
            .working
            .is_some_and(|working| Some(working) != self.thanked)
        {
            self.thanked = None;
        }
        for index in 0..self.stations.len() {
            let station = &self.stations[index];
            if station.caller.state.active() {
                continue;
            }
            if station.heard.engaged && !station.heard.recorded && self.thanked != Some(index) {
                self.lose(index, run_ms);
            }
            if self.working == Some(index) {
                self.working = None;
            }
        }
    }

    /// Log the QSO in the fields at `run_ms`, checked against the station
    /// you work, or else the one your TU closed, if the call is its own or
    /// close to it; otherwise not in its log. Returns its entry.
    fn log(&mut self, fields: &Fields, run_ms: u64, now: MonoMs) -> usize {
        let reading = parse_exchange(self.rules, self.side, &fields.exch);
        let logged = QsoText {
            call: normalize_call(&fields.call),
            fields: reading
                .fields
                .iter()
                .map(|read| (read.field, read.token.clone()))
                .collect(),
        };
        let theirs = |index: &usize| {
            let call = &self.stations[*index].caller.call.text;
            matches!(
                match_call(&logged.call, call),
                CallMatch::Exact | CallMatch::Close
            )
        };
        let worked = self
            .working
            .filter(theirs)
            .or_else(|| self.thanked.filter(theirs));
        // Another QSO logged: the one your TU closed is lost.
        if let Some(thanked) = self.thanked.take().filter(|index| worked != Some(*index))
            && !self.stations[thanked].heard.recorded
        {
            self.lose(thanked, run_ms);
        }
        let edited_run = self.run_at(fields.call_edited, now);
        let done_run = fields
            .exch_complete
            .map_or(run_ms, |at| self.run_at(at, now));
        let entry = match worked {
            Some(index) => {
                self.worked_entry(index, &logged, &reading, done_run, edited_run, run_ms)
            }
            None => self.nil_entry(&logged, &reading, done_run, edited_run, run_ms),
        };
        self.entries.push(entry);
        // A QSO logged is final, and so is every one before it.
        self.teach();
        self.serial += 1;
        self.qso = QsoState::default();
        self.working = None;
        self.entries.len() - 1
    }

    /// Teach the call source every group of the QSOs that are final: all
    /// but those the message on the air lost, which Esc can take back.
    fn teach(&mut self) {
        let upto = match self.air.as_ref().and_then(|air| air.unheard.as_ref()) {
            Some(unheard) if unheard.entries == self.entries.len() => unheard.lost_from,
            _ => self.entries.len(),
        };
        let source = &mut self.source;
        for entry in self.entries.get(self.taught..upto).unwrap_or_default() {
            for (group, timing) in &entry.groups {
                learn(source, group, timing);
            }
        }
        self.taught = self.taught.max(upto);
    }

    /// The calls already logged clean, by the station's own call.
    fn logged_clean(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| entry.record.k != NIL)
            .filter(|entry| entry.row.as_ref().is_some_and(|row| row.check.is_clean()))
            .filter_map(|entry| entry.station)
            .map(|index| self.stations[index].caller.call.text.clone())
            .collect()
    }

    /// A QSO logged with station `index`: a group for the call, as first
    /// sent with your exchange, and one for each field: as keyed, once its
    /// exchange has begun to reach you, and a miss before that, as a guess
    /// is no copy. A station logged before, or charged with a NIL, has its
    /// copy kept already: one station, one copy. What no group holds, the
    /// record does, for the check.
    fn worked_entry(
        &mut self,
        index: usize,
        logged: &QsoText,
        reading: &Reading,
        done_run: u64,
        edited_run: u64,
        run_ms: u64,
    ) -> Entry {
        let clean = self.logged_clean();
        let nr = self.serial_sent();
        let (rules, side) = (self.rules, self.side);
        let copied = rules.copied(side);
        let station = &mut self.stations[index];
        let again = station.heard.recorded;
        station.heard.recorded = true;
        let sent = QsoText {
            call: station.caller.call.text.clone(),
            fields: station.caller.exchange(rules, side, run_ms).to_vec(),
        };
        let check = check_qso(logged, Some(&sent), |call| clean.iter().any(|c| c == call));
        let heard = &station.heard;
        let voice = station.caller.voice;
        // Never sent with your exchange: the call as logged, timed as last
        // sent, or as typed for the log.
        let (first_call, call_ms) = heard.first_call.clone().unwrap_or_else(|| {
            let ms = heard
                .call_ms
                .unwrap_or_else(|| copy_ms(&heard.call_ends, edited_run));
            (logged.call.clone(), ms)
        });
        let mut groups = Vec::new();
        if !again {
            groups.push(group(
                &sent.call,
                &first_call,
                call_ms,
                1 + heard.call_repeats(&first_call),
                &voice,
            ));
            groups.extend(heard.field_groups(&sent.fields, logged, reading, done_run, &voice));
        }
        let copies_fields = !again && heard.exch_begun;
        let by_field = |text: &QsoText| -> Vec<String> {
            copied
                .iter()
                .map(|field| text.value(*field).unwrap_or_default().to_string())
                .collect()
        };
        let (l, s) = if copies_fields {
            (Vec::new(), Vec::new())
        } else {
            (by_field(logged), by_field(&sent))
        };
        let lc = if !again && logged.call == first_call {
            String::new()
        } else {
            logged.call.clone()
        };
        Entry {
            record: QsoRecord {
                t: run_ms,
                h: heard.first.unwrap_or(run_ms).min(run_ms),
                g: u32::try_from(groups.len()).unwrap_or(u32::MAX),
                nr,
                k: String::new(),
                c: station.caller.call.country.to_string(),
                l,
                lc,
                s,
                sc: if again { sent.call } else { String::new() },
                a: heard.asked(),
            },
            row: Some(Row {
                call: logged.call.clone(),
                fields: values(reading),
                wpm: Some(tenth(voice.char_wpm)),
                asked: heard.asked(),
                check,
                pending: true,
            }),
            groups,
            station: Some(index),
        }
    }

    /// The station a NIL logged as `call` is against: the one you work;
    /// else the one whose call it is, or close to it; else, in a run of
    /// single callers, the single caller, the newest heard. None when that
    /// station has its copy kept already, or no station was heard; in a
    /// pile-up, a call like nobody's is charged to nobody, as the newest
    /// caller is no likelier its station than any other.
    fn nil_target(&self, call: &str) -> Option<usize> {
        let fresh = |index: &usize| !self.stations[*index].heard.recorded;
        if let Some(working) = self.working.filter(fresh) {
            return Some(working);
        }
        let heard: Vec<usize> = (0..self.stations.len())
            .filter(|index| self.stations[*index].heard.first.is_some())
            .collect();
        let likeness = |index: usize| match match_call(call, &self.stations[index].caller.call.text)
        {
            CallMatch::Exact => 2,
            CallMatch::Close => 1,
            CallMatch::Partial { .. } | CallMatch::No => 0,
        };
        let best = heard
            .iter()
            .map(|index| (likeness(*index), fresh(index), *index))
            .filter(|(likeness, ..)| *likeness > 0)
            .max();
        match best {
            Some((_, fresh, index)) => fresh.then_some(index),
            None if self.style == RunStyle::Single => heard.last().copied().filter(fresh),
            None => None,
        }
    }

    /// A QSO logged with nobody matching worked: not in their log. The
    /// station [`Self::nil_target`] finds owns its call group, as copied by
    /// the call logged, and a group for each field as a QSO logged with it
    /// has them: as keyed once its exchange has begun to reach you, and a
    /// miss before that. With none, `lc` holds the call.
    fn nil_entry(
        &mut self,
        logged: &QsoText,
        reading: &Reading,
        done_run: u64,
        edited_run: u64,
        run_ms: u64,
    ) -> Entry {
        let check = check_qso(logged, None, |_| false);
        let against = self.nil_target(&logged.call);
        let nr = self.serial_sent();
        let (rules, side) = (self.rules, self.side);
        let (groups, first, wpm) = match against {
            Some(index) => {
                let station = &mut self.stations[index];
                station.heard.recorded = true;
                let exchange = station.caller.exchange(rules, side, run_ms).to_vec();
                let heard = &station.heard;
                let took = copy_ms(&heard.call_ends, edited_run);
                let voice = station.caller.voice;
                let mut groups = vec![group(
                    &station.caller.call.text,
                    &logged.call,
                    took,
                    1 + heard.call_repeats(&logged.call),
                    &voice,
                )];
                groups.extend(heard.field_groups(&exchange, logged, reading, done_run, &voice));
                (groups, heard.first, Some(tenth(voice.char_wpm)))
            }
            None => (Vec::new(), None, None),
        };
        Entry {
            record: QsoRecord {
                t: run_ms,
                h: first.unwrap_or(run_ms).min(run_ms),
                g: u32::try_from(groups.len()).unwrap_or(u32::MAX),
                nr,
                k: NIL.to_string(),
                c: String::new(),
                l: rules
                    .copied(side)
                    .iter()
                    .map(|field| logged.value(*field).unwrap_or_default().to_string())
                    .collect(),
                // With no group to hold it, the record does.
                lc: if groups.is_empty() {
                    logged.call.clone()
                } else {
                    String::new()
                },
                s: Vec::new(),
                sc: String::new(),
                // No QSO with the station it is against: nothing asked of
                // it was for this one.
                a: 0,
            },
            row: Some(Row {
                call: logged.call.clone(),
                fields: values(reading),
                wpm,
                asked: 0,
                check,
                pending: true,
            }),
            groups,
            station: against,
        }
    }

    /// Station `index`'s QSO ended unlogged: one call group, copied as the
    /// call first sent with your exchange, as a logged QSO keeps it, or
    /// else as the Call field last read while you worked it, timed as last
    /// sent; and a miss for each field, heard or not, as a QSO logged
    /// before its exchange came has. Walking away pays no better than
    /// finishing, and hides no copy.
    fn lose(&mut self, index: usize, run_ms: u64) {
        let nr = self.serial_sent();
        let (rules, side) = (self.rules, self.side);
        let station = &mut self.stations[index];
        station.heard.recorded = true;
        let exchange = station.caller.exchange(rules, side, run_ms).to_vec();
        let heard = &station.heard;
        let (received, call_ms) = match &heard.first_call {
            Some((call, ms)) => (call, *ms),
            None => (&heard.typed, heard.call_ms.unwrap_or(0)),
        };
        let mut groups = vec![group(
            &station.caller.call.text,
            received,
            call_ms,
            1 + heard.call_repeats(received),
            &station.caller.voice,
        )];
        groups.extend(misses(&exchange, &station.caller.voice));
        self.entries.push(Entry {
            record: QsoRecord {
                t: run_ms,
                h: heard.first.unwrap_or(run_ms).min(run_ms),
                g: u32::try_from(groups.len()).unwrap_or(u32::MAX),
                nr,
                k: LOST.to_string(),
                c: station.caller.call.country.to_string(),
                l: Vec::new(),
                lc: String::new(),
                s: Vec::new(),
                sc: String::new(),
                a: heard.asked(),
            },
            groups,
            row: None,
            station: Some(index),
        });
    }

    /// Show the checks of the log rows `rows`.
    fn resolve(&mut self, rows: &[usize], effects: &mut Vec<ContestEffect>) {
        for index in rows {
            let Some(row) = self
                .entries
                .get_mut(*index)
                .and_then(|entry| entry.row.as_mut())
                .filter(|row| row.pending)
            else {
                continue;
            };
            row.pending = false;
            effects.push(ContestEffect::Announce(format!(
                "{} logged, {}",
                row.call,
                row.check.words()
            )));
        }
    }

    /// The sound failed after every retry: the run pauses, the retries
    /// counted as paused rather than as run time.
    fn sound_failed(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        let Some(air) = self.air.take() else {
            return Vec::new();
        };
        let from = air.first_start.unwrap_or(air.keyed_at).min(now);
        if let Some(clock) = &mut self.clock {
            clock.pause(from);
        }
        // Nothing of it was heard: every caller in it sends again, as it
        // was keyed, so a repeat you asked for is charged once you hear it;
        // as no send you asked for if a key that logged counted it already.
        let again = air
            .replies
            .iter()
            .zip(&air.counted)
            .map(|((reply, _), counted)| {
                if *counted == Counted::Nothing {
                    reply.clone()
                } else {
                    Replying::unasked(reply.station)
                }
            });
        self.cut_short.extend(again);
        let mut effects = Vec::new();
        // Nothing of a send that failed was heard; the QSOs it logged stand.
        self.resolve(&air.pending, &mut effects);
        self.timers = Timers::default();
        if self.time_up(now) {
            effects.extend(self.finish(now));
            return effects;
        }
        self.phase = ContestPhase::Paused {
            why: PauseWhy::Sound,
        };
        effects.insert(0, ContestEffect::ReleaseAudio);
        effects.push(ContestEffect::Announce(
            "Sound stopped — Resume to carry on".to_string(),
        ));
        effects
    }

    fn pause(&mut self, why: PauseWhy, at: MonoMs, now: MonoMs) -> Vec<ContestEffect> {
        match self.phase {
            ContestPhase::Countdown { .. } => {
                self.timers = Timers::default();
                self.phase = ContestPhase::Paused { why };
                vec![
                    ContestEffect::ReleaseAudio,
                    ContestEffect::Announce("Paused".to_string()),
                ]
            }
            ContestPhase::Running => {
                if let Some(clock) = &mut self.clock {
                    clock.pause(at);
                }
                if self.time_up(now) {
                    return self.finish(now);
                }
                // Held now: the run time the pause began at.
                let run_ms = self.run_ms(now);
                let mut effects = vec![ContestEffect::StopAudio, ContestEffect::ReleaseAudio];
                // A pause with nothing on the air, in the moment after
                // Resume, keeps who the last one cut short.
                if let Some(air) = self.air.take() {
                    self.cut_short.extend(air.cut_short(run_ms, why));
                    let carried = self.cut(air, run_ms, &mut effects);
                    self.resolve(&carried.pending, &mut effects);
                }
                self.timers = Timers::default();
                self.phase = ContestPhase::Paused { why };
                effects.push(ContestEffect::Announce("Paused".to_string()));
                effects
            }
            _ => Vec::new(),
        }
    }

    fn resume(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        if !matches!(self.phase, ContestPhase::Paused { .. }) {
            return Vec::new();
        }
        let Some(clock) = &mut self.clock else {
            // Paused in the countdown: it starts again.
            self.phase = ContestPhase::Countdown {
                left: self.settings.playback.countdown_sec.max(1),
            };
            return vec![
                self.countdown_step(),
                ContestEffect::Announce("Resumed".to_string()),
            ];
        };
        // The pause it ends, for a copy typed before it and sent after.
        let paused_before = clock.paused_before_ms();
        clock.resume(now);
        let length = clock.paused_before_ms() - paused_before;
        if length > 0 {
            self.pauses.push((now, length));
        }
        self.phase = ContestPhase::Running;
        let left = self.limit_ms.saturating_sub(self.run_ms(now));
        let (id, end) = self.start_timer(left);
        self.timers.run_end = Some(id);
        let (id, wake) = self.start_timer(RESUME_DELAY_MS);
        self.timers.resume = Some(id);
        vec![end, wake, ContestEffect::Announce("Resumed".to_string())]
    }

    fn abort(&mut self) -> Vec<ContestEffect> {
        self.air = None;
        self.timers = Timers::default();
        self.phase = ContestPhase::Aborted;
        vec![ContestEffect::StopAudio, ContestEffect::AbortToHome]
    }

    /// The run is over: at its limit, as soon as anything notices it has
    /// passed, or when you end it. A QSO whose TU went out is in the log
    /// already; every other open QSO is a lost copy, as a real log does not
    /// count a QSO nobody finished.
    fn finish(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        let elapsed = self.run_ms(now);
        let time_up = self.clock.is_some() && elapsed >= self.limit_ms;
        // Time up, the run ended at its limit, whenever it was noticed.
        let run_ms = elapsed.min(self.limit_ms);
        let mut effects = vec![ContestEffect::StopAudio];
        if let Some(air) = self.air.take() {
            let carried = self.cut(air, run_ms, &mut effects);
            self.resolve(&carried.pending, &mut effects);
        }
        for index in 0..self.stations.len() {
            let heard = &self.stations[index].heard;
            if heard.engaged && !heard.recorded {
                self.lose(index, run_ms);
            }
        }
        // Run time and time paused add up to the time from the start to the
        // end: at the limit, a pause going on began after it.
        let paused_ms = match &self.clock {
            Some(clock) if time_up => clock.paused_before_ms(),
            Some(clock) => clock.paused_ms(now),
            None => 0,
        };
        self.ended = Some((run_ms, paused_ms));
        self.timers = Timers::default();
        self.phase = ContestPhase::Finished;
        if time_up {
            effects.push(ContestEffect::Announce("Time's up".to_string()));
        }
        // A run that copied nothing has nothing for any stat: a NIL logged
        // with no station heard is no station worked.
        let kept = self.entries.iter().any(|entry| !entry.groups.is_empty());
        effects.push(if kept {
            ContestEffect::PersistAndShowResults
        } else {
            ContestEffect::ShowNothingKept
        });
        effects
    }
}

/// One group of the run, with its timing: never quicker than a person
/// types, at the station's speeds to a tenth of a WPM.
fn group(
    sent: &str,
    received: &str,
    ms: u64,
    sends: u32,
    voice: &StationVoice,
) -> (GroupResult, SessionTiming) {
    let floor = MIN_RESPONSE_MS_PER_CHAR * sent.chars().count().max(1) as f64;
    (
        GroupResult {
            sent: sent.to_string(),
            received: received.to_string(),
            correct: sent == received,
        },
        SessionTiming {
            time_to_complete_ms: (ms as f64).max(floor),
            char_wpm: Some(tenth(voice.char_wpm)),
            effective_wpm: Some(tenth(voice.effective_wpm)),
            sends,
        },
    )
}

/// A missed group for each field of `exchange`, sent at `voice`: what a
/// station's copy holds of an exchange logged before it was heard, as a
/// guess is no copy, or never logged at all, as walking away is none.
fn misses(exchange: &[(Field, String)], voice: &StationVoice) -> Vec<(GroupResult, SessionTiming)> {
    exchange
        .iter()
        .map(|(_, token)| group(token, "", 0, 1, voice))
        .collect()
}

/// How long a copy finished at `done` took: from the end of the last send
/// of it, among `ends`, heard by then, or from the first if none was.
fn copy_ms(ends: &[u64], done: u64) -> u64 {
    ends.iter()
        .rev()
        .find(|end| **end <= done)
        .or(ends.first())
        .map_or(0, |end| done.saturating_sub(*end))
}

/// Each field `reading` reads, with its value: what the log shows.
fn values(reading: &Reading) -> Vec<(Field, String)> {
    reading
        .fields
        .iter()
        .map(|read| (read.field, read.value.clone()))
        .collect()
}

/// Teach `source` what was copied of a group.
fn learn(source: &mut impl CallSource, group: &GroupResult, timing: &SessionTiming) {
    let copied = CopyTally::of(&group.sent, &group.received);
    source.learn(Answered {
        sent: &group.sent,
        received: &group.received,
        copied: &copied,
        played: timing.char_wpm.zip(timing.effective_wpm),
    });
}

/// To a tenth.
fn tenth(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::QsoError;
    use crate::contest::bot::{Copied, Runtime, ScriptCalls, copy_of, run_settings};
    use crate::contest::turn::sending_sec;

    /// A run worked by hand, its callers sending `calls` in turn.
    fn runtime(calls: &[&str]) -> Runtime<ScriptCalls> {
        Runtime::start(1, run_settings(25.0), ScriptCalls::new(calls))
    }

    /// What callers send in `turn`.
    fn replies(turn: &Turn) -> Vec<&str> {
        turn.segments
            .iter()
            .filter(|segment| segment.who != Sender::You)
            .map(|segment| segment.text.as_str())
            .collect()
    }

    /// Your message in `turn`.
    fn yours(turn: &Turn) -> &str {
        turn.segments
            .iter()
            .find(|segment| segment.who == Sender::You)
            .map_or("", |segment| segment.text.as_str())
    }

    /// The serial a caller sent in its exchange: its last word.
    fn serial(text: &str) -> String {
        text.split_whitespace()
            .last()
            .unwrap_or_default()
            .to_string()
    }

    fn view(runtime: &Runtime<ScriptCalls>) -> ContestView {
        runtime.machine.view()
    }

    fn result(runtime: &Runtime<ScriptCalls>) -> (SessionResult, ContestRun) {
        let result = runtime.machine.result(1_000, "2026-10-20".into());
        let run = result
            .contest
            .as_ref()
            .and_then(|log| log.run(result.groups.len()))
            .expect("the log reads back");
        (result, run)
    }

    /// Deliver what is due until a new turn goes out, and hand it back.
    fn next_turn(runtime: &mut Runtime<ScriptCalls>) -> Turn {
        let played = runtime.turns.len();
        while runtime.turns.len() == played {
            assert!(runtime.step(), "nothing more is due");
        }
        runtime.last_turn().clone()
    }

    /// Work K5XYZ as far as its exchange, heard whole: what it sent.
    fn to_its_exchange(runtime: &mut Runtime<ScriptCalls>) -> String {
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let exchange = replies(runtime.last_turn())[0].to_string();
        runtime.finish_turn();
        exchange
    }

    fn timers(effects: &[ContestEffect]) -> Vec<u32> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                ContestEffect::StartTimer { ms, .. } => Some(*ms),
                _ => None,
            })
            .collect()
    }

    fn played(effects: &[ContestEffect]) -> Option<&Turn> {
        effects.iter().find_map(|effect| match effect {
            ContestEffect::PlayTurn(turn) => Some(turn),
            _ => None,
        })
    }

    #[test]
    fn the_basic_qso() {
        let mut runtime = runtime(&["K5XYZ"]);
        assert!(matches!(
            runtime.effects[..],
            [
                ContestEffect::StartTimer { ms: 600_000, .. },
                ContestEffect::PlayTurn(_)
            ]
        ));
        let cq = runtime.last_turn().clone();
        assert_eq!(yours(&cq), "CQ DL1ABC TEST");
        assert_eq!(replies(&cq), ["K5XYZ"]);
        assert!(cq.segments[1].start_sec > cq.segments[0].end_sec + 0.09);
        runtime.finish_turn();
        let effects = runtime.key(OpKey::Esm, "K5XYZ", "");
        assert!(effects.contains(&ContestEffect::Focus(EntryField::Exchange)));
        let exchange = replies(runtime.last_turn())[0].to_string();
        assert_eq!(yours(runtime.last_turn()), "K5XYZ 5NN TT1");
        let nr = serial(&exchange);
        assert!(exchange == format!("TU 5NN {nr}") || exchange == format!("R 5NN {nr}"));
        assert!(view(&runtime).qso.exchange_sent);
        runtime.finish_turn();
        let effects = runtime.key(OpKey::Esm, "K5XYZ", &nr);
        let tu = played(&effects).expect("TU goes out").clone();
        assert_eq!(yours(&tu), "TU");
        assert_eq!(replies(&tu), ["W2AW"], "a TU brings the next caller");
        assert!(effects.contains(&ContestEffect::ClearFields));
        assert!(effects.contains(&ContestEffect::Focus(EntryField::Call)));
        // Until the station has your TU, its check is not shown.
        let waiting = view(&runtime);
        assert_eq!(waiting.qsos, 1);
        assert_eq!(waiting.clean, 0);
        assert_eq!(waiting.log[0].status, RowStatus::Pending);
        assert_eq!(waiting.log[0].wpm, None, "nothing of the station shows yet");
        assert_eq!(waiting.qso, QsoState::default());
        runtime.finish_turn();
        let done = view(&runtime);
        assert_eq!(done.log[0].status, RowStatus::Checked(QsoCheck::default()));
        assert_eq!(done.log[0].call, "K5XYZ");
        assert_eq!(done.log[0].fields, [(Field::Nr, nr.clone())]);
        assert_eq!(done.log[0].wpm, Some(25.0));
        assert_eq!(done.clean, 1);
        runtime.now += 1_000;
        let effects = runtime.apply(ContestEvent::FinishNow);
        assert_eq!(effects.last(), Some(&ContestEffect::PersistAndShowResults));
        let (result, run) = result(&runtime);
        assert_eq!(result.char_set_mode, crate::CharSetMode::Contest);
        assert_eq!(result.alphabet_fingerprint, "contest:wpx");
        let copies: Vec<(&str, &str)> = result
            .groups
            .iter()
            .map(|group| (group.sent.as_str(), group.received.as_str()))
            .collect();
        assert_eq!(copies, [("K5XYZ", "K5XYZ"), (nr.as_str(), nr.as_str())]);
        assert!(result.group_timings.iter().all(|timing| timing.sends == 1));
        assert!(
            result
                .group_timings
                .iter()
                .all(|t| t.char_wpm == Some(25.0))
        );
        assert_eq!(run.qsos.len(), 1);
        let qso = &run.qsos[0];
        assert_eq!(
            (qso.g, qso.nr, qso.k.as_str(), qso.lc.as_str()),
            (2, 1, "", "")
        );
        assert!(qso.h < qso.t && qso.t <= run.run_ms);
        assert_eq!(run.fields, ["nr"]);
        assert_eq!(run.my_country, "DL");
        assert!(result.score > 0.0);
        let score = run.score(&result.groups).expect("WPX scores");
        assert_eq!(score.verified.qso_points, 1);
        assert_eq!(
            runtime.machine.source.learned.len(),
            2,
            "the sampler learns both"
        );
    }

    /// A Simple QSO, line by line as the design has it: your CQ, the
    /// caller's call to you, its call and your exchange with your name, its
    /// report and name, your TU by its name, and its 73, which brings
    /// nobody: you call CQ again. Its groups are the call, the report and
    /// the name, and nothing scores it but the copy.
    #[test]
    fn a_simple_qso_goes_as_an_everyday_qso_does() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_kind = crate::contest::ContestKind::SimpleQso;
        let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
        let cq = runtime.last_turn().clone();
        assert_eq!(yours(&cq), "CQ CQ DE DL1ABC DL1ABC K");
        assert_eq!(replies(&cq), ["DL1ABC DE K5XYZ K5XYZ K"]);
        runtime.finish_turn();
        let effects = runtime.key(OpKey::Esm, "K5XYZ", "");
        assert!(effects.contains(&ContestEffect::Focus(EntryField::Exchange)));
        assert_eq!(
            yours(runtime.last_turn()),
            "K5XYZ DE DL1ABC UR 599 599 NAME MAT MAT BK"
        );
        let exchange = replies(runtime.last_turn())[0].to_string();
        let words: Vec<&str> = exchange.split_whitespace().collect();
        let (rst, name) = (words[3], words[6]);
        assert_eq!(
            exchange,
            format!("R TNX UR {rst} {rst} NAME {name} {name} BK")
        );
        // Plain digits, as callers with easy habits send them.
        assert!(rst.chars().all(|c| c.is_ascii_digit()), "{rst}");
        assert!(Field::Rst.canonical(rst).is_some() && Field::Name.canonical(name).is_some());
        runtime.finish_turn();
        // The report is part of the copy: the name alone asks again.
        assert_eq!(
            esm(
                &view(&runtime).qso,
                &Fields {
                    call: "K5XYZ".into(),
                    exch: name.into(),
                    ..Fields::default()
                },
                runtime.machine.rules,
                Side::Dx
            )
            .message,
            [MessagePart::Again]
        );
        let effects = runtime.key(OpKey::Esm, "K5XYZ", &format!("{rst} {name}"));
        let tu = played(&effects).expect("TU goes out").clone();
        assert_eq!(yours(&tu), format!("TU {name} 73 EE"));
        assert_eq!(replies(&tu), ["73 EE"], "it signs off, and nobody calls");
        runtime.finish_turn();
        let done = view(&runtime);
        assert_eq!(done.log[0].status, RowStatus::Checked(QsoCheck::default()));
        assert_eq!(
            done.log[0].fields,
            [
                (Field::Rst, rst.to_string()),
                (Field::Name, name.to_string())
            ]
        );
        assert_eq!(done.serial, 0, "no serial to send");
        // Nobody is left waiting: only the run's end is due, and Enter's CQ
        // is for the next station.
        assert_eq!(runtime.next_due(), Some(600_000));
        assert!(done.qso.nobody_calling);
        assert_eq!(
            esm(
                &done.qso,
                &Fields::default(),
                runtime.machine.rules,
                Side::Dx
            )
            .hint,
            "Enter calls CQ for the next station"
        );
        runtime.key(OpKey::Esm, "", "");
        let next = runtime.last_turn().clone();
        assert_eq!(yours(&next), "CQ CQ DE DL1ABC DL1ABC K");
        assert_eq!(replies(&next), ["DL1ABC DE W2AW W2AW K"]);
        assert!(!view(&runtime).qso.nobody_calling, "the CQ brings one");
        runtime.now += 1_000;
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(result.alphabet_fingerprint, "contest:simpleQso");
        let copies: Vec<(&str, &str)> = result
            .groups
            .iter()
            .map(|group| (group.sent.as_str(), group.received.as_str()))
            .collect();
        assert_eq!(copies, [("K5XYZ", "K5XYZ"), (rst, rst), (name, name)]);
        assert_eq!(run.fields, ["rst", "name"]);
        assert_eq!((run.qsos.len(), run.qsos[0].nr), (1, 0));
        assert_eq!(run.score(&result.groups), None);
        assert!(result.score > 0.0, "the copy still earns its points");
    }

    /// In a Simple QSO, Enter or TU and log after F3 sends that TU again,
    /// by the name you copied, without the call, and logs the QSO.
    #[test]
    fn a_simple_qso_tu_without_logging_is_logged_with_its_tu_alone() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_kind = crate::contest::ContestKind::SimpleQso;
        for key in [OpKey::Esm, OpKey::TuAndLog] {
            let mut runtime = Runtime::start(1, settings.clone(), ScriptCalls::new(&["K5XYZ"]));
            let exchange = to_its_exchange(&mut runtime);
            let words: Vec<&str> = exchange.split_whitespace().collect();
            let copy = format!("{} {}", words[3], words[6]);
            runtime.key(OpKey::Tu, "K5XYZ", &copy);
            runtime.finish_turn();
            assert_eq!(view(&runtime).qsos, 0, "F3 logs nothing");
            runtime.key(key, "K5XYZ", &copy);
            assert_eq!(
                yours(runtime.last_turn()),
                format!("TU {} 73 EE", words[6]),
                "{key:?}"
            );
            runtime.finish_turn();
            assert_eq!(view(&runtime).qsos, 1, "{key:?}");
        }
    }

    /// Each part Enter sends has its key, which sends that part alone, so
    /// the key bar can light what Enter presses; Enter's TU is TU and log,
    /// which logs as Enter's does.
    #[test]
    fn each_part_enter_sends_has_a_key_that_sends_it() {
        let mut runtime = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut runtime);
        let fields = Fields {
            call: "K5XYZ".to_string(),
            exch: serial(&exchange),
            ..Fields::default()
        };
        for part in [
            MessagePart::Cq,
            MessagePart::HisCall("K5XYZ".to_string()),
            MessagePart::Exchange,
            MessagePart::Again,
            MessagePart::Tu,
            MessagePart::MyCall,
            MessagePart::B4,
            MessagePart::Nil,
            MessagePart::NrQuery,
        ] {
            let key = OpKey::sending(&part);
            let action = runtime.machine.action(key, &fields);
            assert_eq!(action.logs, part == MessagePart::Tu, "{key:?}");
            assert_eq!(action.message, [part], "{key:?}");
        }
    }

    /// Your exchange went and theirs is not copied: Enter asks with `?`,
    /// and the station sends its exchange again.
    #[test]
    fn a_question_on_an_empty_exchange() {
        let mut runtime = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut runtime);
        runtime.key(OpKey::Esm, "K5XYZ", "");
        assert_eq!(yours(runtime.last_turn()), "?");
        assert_eq!(
            replies(runtime.last_turn()),
            [format!("5NN {}", serial(&exchange))]
        );
    }

    /// Asking again adds a send to what is re-sent; a station sending again
    /// on its own, when it has waited long enough, adds none.
    #[test]
    fn again_adds_a_send_and_a_timeout_adds_none() {
        let sends = |asked: bool| {
            let mut runtime = runtime(&["K5XYZ"]);
            let exchange = to_its_exchange(&mut runtime);
            if asked {
                runtime.key(OpKey::Again, "K5XYZ", "");
            } else {
                let turn = next_turn(&mut runtime);
                assert_eq!(yours(&turn), "", "a turn of the caller alone");
            }
            assert_eq!(
                replies(runtime.last_turn()),
                [format!("5NN {}", serial(&exchange))]
            );
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            let sends: Vec<u32> = result.group_timings.iter().map(|t| t.sends).collect();
            let asked = view(&runtime).log[0].asked;
            (sends, asked)
        };
        assert_eq!(sends(true), (vec![1, 2], 1));
        assert_eq!(sends(false), (vec![1, 1], 0));
    }

    /// A repeat you asked for costs its send once it has begun to reach
    /// you, even if you key over its end; stopped before it began, it was
    /// never heard and costs nothing.
    #[test]
    fn a_repeat_costs_its_send_once_it_begins() {
        let sends = |keyed: &str| {
            let mut runtime = runtime(&["K5XYZ"]);
            let exchange = to_its_exchange(&mut runtime);
            runtime.key(OpKey::Again, "K5XYZ", "");
            let reply = runtime.last_turn().segments[1].clone();
            match keyed {
                "after" => runtime.finish_turn(),
                "over" => {
                    let end = runtime.now + (reply.end_sec * 1_000.0) as u64;
                    runtime.run_to(end - 50);
                }
                _ => {
                    runtime.key(OpKey::StopTx, "K5XYZ", "");
                }
            }
            runtime.key(OpKey::TuAndLog, "K5XYZ", &serial(&exchange));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            let sends: Vec<u32> = result.group_timings.iter().map(|t| t.sends).collect();
            (sends, view(&runtime).log[0].asked)
        };
        assert_eq!(sends("after"), (vec![1, 2], 1));
        assert_eq!(sends("over"), (vec![1, 2], 1), "keyed over its end");
        assert_eq!(sends("stopped"), (vec![1, 1], 0), "stopped before it began");
    }

    /// A call sent with a slip is corrected; the call as first sent is the
    /// copy, and the correction costs no send. The log keeps the call put
    /// right.
    #[test]
    fn a_corrected_call_keeps_the_first_call_as_its_copy() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYW", "");
        let correction = replies(runtime.last_turn())[0].to_string();
        assert!(correction == "DE K5XYZ" || correction == "K5XYZ K5XYZ");
        assert!(!view(&runtime).qso.corrected, "not heard yet");
        runtime.finish_turn();
        assert!(view(&runtime).qso.corrected);
        runtime.key(OpKey::Esm, "K5XYZ", "");
        assert_eq!(yours(runtime.last_turn()), "K5XYZ ?");
        assert!(!view(&runtime).qso.corrected);
        let exchange = replies(runtime.last_turn())[0].to_string();
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(result.groups[0].sent, "K5XYZ");
        assert_eq!(result.groups[0].received, "K5XYW");
        assert_eq!(result.group_timings[0].sends, 1);
        assert_eq!(run.qsos[0].lc, "K5XYZ");
        assert_eq!(view(&runtime).clean, 1, "logged right, it is clean");
        let score = run.score(&result.groups).expect("WPX scores");
        assert_eq!(
            score.verified.qso_points, 1,
            "the stored run scores as it ran"
        );
    }

    /// F5 then F2 while your call is still going out: both go, your call
    /// carried on from the character sounding, and the station answers the
    /// whole with its exchange.
    #[test]
    fn his_call_then_your_exchange_during_your_call_sends_both() {
        for (after_ms, rest) in [(100, "K5XYZ 5NN TT1"), (700, "5XYZ 5NN TT1")] {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::HisCall, "K5XYZ", "");
            assert_eq!(yours(runtime.last_turn()), "K5XYZ");
            let first = runtime.last_turn().id;
            runtime.now += after_ms;
            runtime.key(OpKey::Exchange, "K5XYZ", "");
            let turn = runtime.last_turn().clone();
            assert_ne!(turn.id, first);
            assert_eq!(yours(&turn), rest, "{after_ms} ms in");
            let reply = replies(&turn)[0];
            assert!(
                reply.starts_with("TU 5NN") || reply.starts_with("R 5NN"),
                "{reply}"
            );
            let state = view(&runtime).qso;
            assert_eq!(state.call_sent, "K5XYZ");
            assert!(state.exchange_sent);
            // The replaced send comes back cancelled, and is ignored.
            assert!(
                runtime
                    .apply(ContestEvent::TurnCancelled { id: first })
                    .is_empty()
            );
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", &serial(reply));
            runtime.finish_turn();
            assert_eq!(view(&runtime).clean, 1);
        }
    }

    /// However late in your call the next key comes, what was heard whole
    /// of the call and what is sent again together hold every character:
    /// the one sounding at the key goes again.
    #[test]
    fn a_continuation_keeps_every_character_wherever_it_is_keyed() {
        let settings = run_settings(25.0);
        let voice = your_voice(&settings, 25.0);
        let spans = char_spans("K5XYZ", &settings, &voice);
        let call_ms = (sending_sec("K5XYZ", &settings, &voice) * 1_000.0) as u64;
        for after_ms in (0..call_ms + 100).step_by(10) {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::HisCall, "K5XYZ", "");
            runtime.now += after_ms;
            runtime.key(OpKey::Exchange, "K5XYZ", "");
            let heard: String = spans
                .iter()
                .filter(|(_, _, end)| *end <= after_ms as f64 / 1_000.0)
                .map(|(at, ..)| &"K5XYZ"[*at..=*at])
                .collect();
            let again = yours(runtime.last_turn()).replace(' ', "");
            assert_eq!(format!("{heard}{again}"), "K5XYZ5NNTT1", "{after_ms} ms in");
        }
    }

    /// Your exchange sent in one message with a TU that logs carries this
    /// QSO's serial, and the next QSO sends the next one.
    #[test]
    fn an_exchange_sent_with_the_tu_carries_this_qsos_serial() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        let effects = runtime.key(OpKey::Esm, "K5XYZ", "123");
        assert_eq!(yours(played(&effects).expect("a turn")), "5NN TT1 TU");
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "W2AW", "");
        assert_eq!(yours(runtime.last_turn()), "W2AW 5NN TT2");
        runtime.apply(ContestEvent::FinishNow);
        let (_, run) = result(&runtime);
        assert_eq!(run.qsos[0].nr, 1);
    }

    /// TU and log keyed while the station is still sending: the station
    /// stops and hears it, and the QSO is checked once the TU is out.
    #[test]
    fn a_tu_keyed_during_replies_is_finished_and_checked() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let turn = runtime.last_turn().clone();
        let reply = &turn.segments[1];
        // Copied as it came, keyed just before its last character.
        runtime.run_to(runtime.now + (reply.end_sec * 1_000.0) as u64 - 100);
        let effects = runtime.key(OpKey::TuAndLog, "K5XYZ", &serial(&reply.text));
        let tu = played(&effects).expect("TU goes out").clone();
        assert_eq!(yours(&tu), "TU", "your message was over; only the TU goes");
        assert_eq!(view(&runtime).log[0].status, RowStatus::Pending);
        runtime.finish_turn();
        assert_eq!(
            view(&runtime).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );
        // Ended before the TU is out, the QSO is final and checked all the
        // same.
        let mut cut = self::runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut cut);
        cut.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        cut.apply(ContestEvent::FinishNow);
        let view = cut.machine.view();
        assert_eq!(view.log[0].status, RowStatus::Checked(QsoCheck::default()));
        let (_, run) = result(&cut);
        assert_eq!(run.qsos.len(), 1, "no lost copy beside it");
    }

    /// The check of a QSO shows once the turn carrying its TU is off the
    /// air: at once when a key replaces that turn after the TU is out, and
    /// with the turn that sends the TU again when it was still sounding.
    #[test]
    fn a_rows_check_shows_once_its_tu_is_out() {
        let mut runtime = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut runtime);
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        let tu = runtime.last_turn().clone();
        assert_eq!(yours(&tu), "TU");
        // Keyed while the next caller is still calling, the TU long over.
        let call = &tu.segments[1];
        runtime.run_to(runtime.now + (call.end_sec * 1_000.0) as u64 - 20);
        runtime.key(OpKey::Esm, "W2AW", "");
        assert_eq!(yours(runtime.last_turn()), "W2AW 5NN TT2");
        assert_eq!(
            view(&runtime).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );
        let mut early = self::runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut early);
        early.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        early.key(OpKey::Esm, "W2AW", "");
        assert_eq!(yours(early.last_turn()), "TU W2AW 5NN TT2");
        assert_eq!(view(&early).log[0].status, RowStatus::Pending);
        early.finish_turn();
        assert_eq!(
            view(&early).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );
    }

    /// Esc: a call or exchange of yours not yet sent whole counts as not
    /// sent.
    #[test]
    fn esc_resets_c_and_n() {
        let settings = run_settings(25.0);
        let call_sec = sending_sec("K5XYZ", &settings, &your_voice(&settings, 25.0));
        for (after_ms, call_sent, exchange_sent) in [
            (0, "", false),
            ((call_sec * 1_000.0) as u64 + 50, "K5XYZ", false),
            (20_000, "K5XYZ", true),
        ] {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            runtime.now += after_ms.min((runtime.last_turn().duration_sec() * 1_000.0) as u64 - 1);
            let effects = runtime.key(OpKey::StopTx, "K5XYZ", "");
            assert_eq!(effects[0], ContestEffect::StopAudio, "{after_ms} ms in");
            let state = view(&runtime).qso;
            assert_eq!(state.call_sent, call_sent, "{after_ms} ms in");
            assert_eq!(state.exchange_sent, exchange_sent, "{after_ms} ms in");
            assert!(view(&runtime).air.is_none());
        }
        // Esc with nothing on the air does nothing.
        let mut quiet = runtime(&["K5XYZ"]);
        quiet.finish_turn();
        assert!(quiet.key(OpKey::StopTx, "", "").is_empty());
    }

    /// Esc takes back what had not gone out: the callers hear only the
    /// parts sent whole, and a call stopped before it went is not the copy.
    #[test]
    fn esc_takes_back_what_had_not_gone_out() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        // A slip, stopped at once: the station never heard it, and calls
        // again on its timeout.
        runtime.key(OpKey::Esm, "K5XYW", "");
        runtime.key(OpKey::StopTx, "K5XYW", "");
        assert_eq!(replies(&next_turn(&mut runtime)), ["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let exchange = replies(runtime.last_turn())[0].to_string();
        assert!(
            exchange.starts_with("TU 5NN") || exchange.starts_with("R 5NN"),
            "{exchange}"
        );
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(result.groups[0].received, "K5XYZ", "not the call stopped");
        assert_eq!(run.qsos[0].lc, "");
        // Stopped once its call was out, the station has its call and asks
        // for your exchange.
        let settings = run_settings(25.0);
        let call_sec = sending_sec("K5XYZ", &settings, &your_voice(&settings, 25.0));
        let mut half = self::runtime(&["K5XYZ"]);
        half.finish_turn();
        half.key(OpKey::Esm, "K5XYZ", "");
        half.now += (call_sec * 1_000.0) as u64 + 50;
        half.key(OpKey::StopTx, "K5XYZ", "");
        let ask = next_turn(&mut half);
        assert!(
            matches!(replies(&ask)[..], ["NR?"] | ["AGN"]),
            "{:?}",
            replies(&ask)
        );
        // Logged while the message was going out, the QSO is final: Esc
        // takes nothing back, and the station sends the exchange it owes.
        let mut logged = self::runtime(&["K5XYZ"]);
        logged.finish_turn();
        logged.key(OpKey::Esm, "K5XYZ", "");
        logged.key(OpKey::LogOnly, "K5XYZ", "123");
        logged.key(OpKey::StopTx, "", "");
        let owed = next_turn(&mut logged);
        assert!(replies(&owed)[0].contains("5NN"), "{:?}", replies(&owed));
        logged.apply(ContestEvent::FinishNow);
        let (_, run) = self::result(&logged);
        assert_eq!(run.qsos.len(), 1, "logged once, and not lost as well");
    }

    fn paused_by_sound(runtime: &Runtime<ScriptCalls>) -> bool {
        runtime.machine.phase()
            == ContestPhase::Paused {
                why: PauseWhy::Sound,
            }
    }

    /// Only the sound failing on the turn on the air pauses the run for
    /// sound, and the retries before it are not run time, near the limit
    /// too.
    #[test]
    fn only_a_current_turn_failing_pauses_for_sound() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        runtime.key(OpKey::StopTx, "K5XYZ", "");
        assert!(!paused_by_sound(&runtime));
        runtime.key(OpKey::Wipe, "K5XYZ", "");
        assert!(!paused_by_sound(&runtime));
        runtime.apply(ContestEvent::Pause {
            why: PauseWhy::You,
            at: MonoMs(runtime.now),
        });
        assert!(!paused_by_sound(&runtime));
        runtime.apply(ContestEvent::Resume);
        assert!(!paused_by_sound(&runtime));

        // A minute's run, a turn keyed near its end, the sound retried past
        // it before failing.
        let mut settings = run_settings(25.0);
        settings.contest.contest_minutes = 1;
        let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
        runtime.finish_turn();
        runtime.now = 50_000;
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let stale = runtime.last_turn().id;
        runtime.key(OpKey::Exchange, "K5XYZ", "");
        let current = runtime.last_turn().id;
        assert!(
            runtime
                .apply(ContestEvent::TurnFailed { id: stale })
                .is_empty()
        );
        assert_eq!(runtime.machine.phase(), ContestPhase::Running);
        runtime.apply(ContestEvent::TurnStarted {
            id: current,
            heard_from: MonoMs(50_100),
        });
        runtime.apply(ContestEvent::TurnStarted {
            id: current,
            heard_from: MonoMs(55_000),
        });
        runtime.now = 62_000;
        let effects = runtime.apply(ContestEvent::TurnFailed { id: current });
        assert_eq!(effects[0], ContestEffect::ReleaseAudio);
        assert!(paused_by_sound(&runtime));
        let clock = view(&runtime)
            .clock
            .map(|mut clock| clock.elapsed_ms(MonoMs(62_000)));
        assert_eq!(
            clock,
            Some(50_100),
            "run time stopped where the turn first started"
        );
        runtime.now = 70_000;
        let effects = runtime.apply(ContestEvent::Resume);
        assert_eq!(timers(&effects), [9_900, 1_000]);
    }

    /// A start only stamps when the sound reached the ear: no effects, and
    /// no end of the run on it, even past the limit.
    #[test]
    fn a_turn_starting_emits_nothing() {
        let mut runtime = runtime(&["K5XYZ"]);
        let id = runtime.last_turn().id;
        runtime.now = 700_000;
        let effects = runtime.apply(ContestEvent::TurnStarted {
            id,
            heard_from: MonoMs(700_050),
        });
        assert!(effects.is_empty());
        assert_eq!(runtime.machine.phase(), ContestPhase::Running);
        assert_eq!(
            view(&runtime).air.map(|air| air.heard_from),
            Some(MonoMs(700_050))
        );
        let effects = runtime.apply(ContestEvent::TurnCancelled { id: 99 });
        assert_eq!(effects.last(), Some(&ContestEffect::ShowNothingKept));
    }

    /// A pause in the countdown is said as one in the run is, and so is
    /// carrying on.
    #[test]
    fn a_pause_in_the_countdown_starts_it_again() {
        let mut settings = run_settings(25.0);
        settings.playback.countdown_sec = 3;
        let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
        assert_eq!(runtime.machine.phase(), ContestPhase::Countdown { left: 3 });
        runtime.run_to(1_500);
        assert_eq!(runtime.machine.phase(), ContestPhase::Countdown { left: 2 });
        let effects = runtime.apply(ContestEvent::Pause {
            why: PauseWhy::Hidden,
            at: MonoMs(1_500),
        });
        assert_eq!(
            effects,
            [
                ContestEffect::ReleaseAudio,
                ContestEffect::Announce("Paused".into())
            ]
        );
        runtime.run_to(20_000);
        assert_eq!(
            runtime.machine.phase(),
            ContestPhase::Paused {
                why: PauseWhy::Hidden
            },
            "a hidden page never resumes by itself"
        );
        assert!(runtime.turns.is_empty());
        let effects = runtime.apply(ContestEvent::Resume);
        assert_eq!(timers(&effects), [1_000]);
        assert_eq!(
            effects.last(),
            Some(&ContestEffect::Announce("Resumed".into()))
        );
        assert_eq!(runtime.machine.phase(), ContestPhase::Countdown { left: 3 });
        runtime.run_to(23_000);
        assert_eq!(runtime.machine.phase(), ContestPhase::Running);
        assert_eq!(yours(runtime.last_turn()), "CQ DL1ABC TEST");
    }

    /// Resume sets the run's end again for the time left; the old end, now
    /// forgotten, does not end it early.
    #[test]
    fn resume_sets_the_run_end_again() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.run_to(100_000);
        let effects = runtime.apply(ContestEvent::Pause {
            why: PauseWhy::You,
            at: MonoMs(100_000),
        });
        assert_eq!(
            effects[..2],
            [ContestEffect::StopAudio, ContestEffect::ReleaseAudio]
        );
        runtime.run_to(130_000);
        let effects = runtime.apply(ContestEvent::Resume);
        assert_eq!(timers(&effects), [500_000, 1_000]);
        runtime.run_to(629_999);
        assert!(!runtime.over, "the old end is forgotten");
        runtime.run_to(630_000);
        assert!(runtime.over);
        assert_eq!(runtime.machine.phase(), ContestPhase::Finished);
        let (result, run) = result(&runtime);
        assert_eq!((run.run_ms, run.paused_ms), (600_000, 30_000));
        assert!(result.groups.is_empty() || run.qsos.iter().all(|qso| qso.t <= 600_000));
    }

    /// Hidden mid-run, the run stays paused however long it waits.
    #[test]
    fn a_hidden_page_never_resumes_by_itself() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let effects = runtime.apply(ContestEvent::Pause {
            why: PauseWhy::Hidden,
            at: MonoMs(runtime.now),
        });
        assert!(effects.contains(&ContestEffect::ReleaseAudio));
        let played = runtime.turns.len();
        runtime.run_to(runtime.now + 3_600_000);
        let hidden = ContestPhase::Paused {
            why: PauseWhy::Hidden,
        };
        assert_eq!(runtime.machine.phase(), hidden);
        assert_eq!(runtime.turns.len(), played);
        assert!(!runtime.over);
        // Keys do nothing while paused: the fields are locked.
        assert!(runtime.key(OpKey::Esm, "K5XYZ", "214").is_empty());
    }

    /// A key before the resume timer fires: what you send is heard instead.
    #[test]
    fn a_key_forgets_the_resume_timer() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.apply(ContestEvent::Pause {
            why: PauseWhy::You,
            at: MonoMs(runtime.now),
        });
        runtime.now += 5_000;
        runtime.apply(ContestEvent::Resume);
        runtime.now += 400;
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let keyed = runtime.turns.len();
        runtime.run_to(runtime.now + 700);
        assert_eq!(runtime.turns.len(), keyed, "the resume timer did nothing");
        // Left alone, the resume timer brings back the station the pause
        // cut short.
        let mut alone = self::runtime(&["K5XYZ"]);
        alone.apply(ContestEvent::Pause {
            why: PauseWhy::You,
            at: MonoMs(alone.now),
        });
        alone.apply(ContestEvent::Resume);
        let before = alone.turns.len();
        alone.run_to(alone.now + 1_000);
        assert_eq!(alone.turns.len(), before + 1);
        assert_eq!(replies(alone.last_turn()), ["K5XYZ"]);
    }

    /// After Resume, a key that sends nothing leaves the resume timer to
    /// bring back the callers the pause cut short.
    #[test]
    fn a_key_that_sends_nothing_keeps_the_resume_timer() {
        for key in [OpKey::StopTx, OpKey::Wipe, OpKey::LogOnly, OpKey::HisCall] {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.apply(ContestEvent::Pause {
                why: PauseWhy::You,
                at: MonoMs(runtime.now),
            });
            runtime.now += 5_000;
            runtime.apply(ContestEvent::Resume);
            runtime.now += 200;
            assert!(played(&runtime.key(key, "", "")).is_none(), "{key:?}");
            let before = runtime.turns.len();
            runtime.run_to(runtime.now + 800);
            assert_eq!(runtime.turns.len(), before + 1, "{key:?}");
            assert_eq!(replies(runtime.last_turn()), ["K5XYZ"]);
        }
    }

    /// A call typed before a pause and sent after it is timed in run time:
    /// the pause is neither charged nor credited to its copy.
    #[test]
    fn a_pause_between_typing_and_the_key_leaves_the_copy_time_alone() {
        let call_ms = |pause_ms: u64| {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            let typed = runtime.now + 2_000;
            runtime.now = typed + 500;
            if pause_ms > 0 {
                runtime.apply(ContestEvent::Pause {
                    why: PauseWhy::Hidden,
                    at: MonoMs(runtime.now),
                });
                runtime.now += pause_ms;
                runtime.apply(ContestEvent::Resume);
                runtime.now += 300;
            }
            let fields = Fields {
                call: "K5XYZ".into(),
                call_edited: MonoMs(typed),
                ..Fields::default()
            };
            runtime.apply(ContestEvent::Key {
                key: OpKey::Esm,
                fields,
            });
            let exchange = replies(runtime.last_turn())[0].to_string();
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            result(&runtime).0.group_timings[0].time_to_complete_ms
        };
        let unpaused = call_ms(0);
        assert!((1_990.0..=2_010.0).contains(&unpaused), "{unpaused}");
        assert_eq!(call_ms(60_000), unpaused);
    }

    /// Asked again three times, wiped, then CQ: the station's QSO ends
    /// unlogged, a lost copy, and a new caller answers.
    #[test]
    fn again_three_times_wipe_and_cq_brings_a_new_caller() {
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        to_its_exchange(&mut runtime);
        for _ in 0..3 {
            runtime.key(OpKey::Again, "K5XYZ", "");
            assert_eq!(replies(runtime.last_turn()).len(), 1);
            runtime.finish_turn();
        }
        let effects = runtime.key(OpKey::Wipe, "K5XYZ", "");
        assert_eq!(
            effects,
            [
                ContestEffect::ClearFields,
                ContestEffect::Focus(EntryField::Call)
            ]
        );
        assert_eq!(view(&runtime).qso, QsoState::default());
        runtime.key(OpKey::Esm, "", "");
        assert_eq!(yours(runtime.last_turn()), "CQ DL1ABC TEST");
        assert_eq!(replies(runtime.last_turn()), ["G4ABC"]);
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(run.qsos.len(), 1);
        assert_eq!(run.qsos[0].k, "lost");
        assert_eq!(result.groups[0].sent, "K5XYZ");
        assert_eq!(result.groups[0].received, "K5XYZ", "as last typed");
    }

    /// A station whose call you sent, given up on: a lost copy, its call
    /// as last typed and a miss for its number.
    #[test]
    fn a_wipe_after_hearing_a_call_adds_a_lost_copy() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XY", "");
        runtime.finish_turn();
        runtime.key(OpKey::Wipe, "K5XY", "");
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(run.qsos.len(), 1);
        assert_eq!((run.qsos[0].k.as_str(), run.qsos[0].g), ("lost", 2));
        assert_eq!(result.groups.len(), 2);
        assert_eq!(result.groups[0].sent, "K5XYZ");
        assert_eq!(result.groups[0].received, "K5XY");
        assert_eq!(result.groups[1].received, "", "a miss");
        assert!(
            view(&runtime).log.is_empty(),
            "a lost copy is not in the log"
        );
    }

    /// A call logged that nobody matching was worked with: not in their
    /// log, a call group and a miss for each field against the station
    /// there.
    #[test]
    fn a_nil_adds_a_call_group_and_a_miss_for_each_field() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "W9ZZZ", "");
        assert!(
            replies(runtime.last_turn()).is_empty(),
            "not its call: silence"
        );
        runtime.finish_turn();
        runtime.key(OpKey::TuAndLog, "W9ZZZ", "123");
        runtime.finish_turn();
        let row = &view(&runtime).log[0];
        assert_eq!(
            row.status,
            RowStatus::Checked(QsoCheck {
                errors: vec![QsoError::Nil],
                dupe: false
            })
        );
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(run.qsos.len(), 1);
        let qso = &run.qsos[0];
        assert_eq!((qso.k.as_str(), qso.g), ("nil", 2));
        assert_eq!(qso.l, ["123"]);
        assert_eq!(result.groups[0].sent, "K5XYZ");
        assert_eq!(result.groups[0].received, "W9ZZZ");
        assert_eq!(result.groups[1].received, "", "a miss");
        let score = run.score(&result.groups).expect("WPX scores");
        assert_eq!((score.raw.qso_points, score.verified.qso_points), (1, 0));
    }

    /// Logged without sending anything: checked at once.
    #[test]
    fn a_qso_logged_without_sending_is_checked_at_once() {
        let mut runtime = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut runtime);
        let effects = runtime.key(OpKey::LogOnly, "K5XYZ", &serial(&exchange));
        assert!(played(&effects).is_none());
        assert!(effects.contains(&ContestEffect::ClearFields));
        assert_eq!(
            view(&runtime).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );
        // Nothing to log: nothing happens.
        assert!(runtime.key(OpKey::LogOnly, "K5", "1").is_empty());
    }

    /// Time up ends the run at once; a QSO still open is a lost copy.
    #[test]
    fn time_up_ends_the_run_and_loses_open_qsos() {
        let mut runtime = runtime(&["K5XYZ"]);
        to_its_exchange(&mut runtime);
        // Kept waiting, with a word now and then, to the very end.
        runtime.now = 599_000;
        runtime.key(OpKey::Again, "K5XYZ", "");
        assert!(!runtime.over);
        runtime.run_to(600_000);
        assert!(runtime.over);
        assert!(
            runtime
                .effects
                .contains(&ContestEffect::Announce("Time's up".into()))
        );
        let (_, run) = result(&runtime);
        assert_eq!(run.qsos.len(), 1);
        assert_eq!((run.qsos[0].k.as_str(), run.qsos[0].t), ("lost", 600_000));
        assert_eq!(run.run_ms, 600_000);
        // An event that notices the limit late still ends the run there.
        let mut late = self::runtime(&["K5XYZ"]);
        late.finish_turn();
        late.now = 605_000;
        late.key(OpKey::Esm, "K5XYZ", "");
        let (_, run) = result(&late);
        assert_eq!((run.run_ms, run.paused_ms), (600_000, 0));
    }

    #[test]
    fn an_empty_run_stores_nothing() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let effects = runtime.apply(ContestEvent::FinishNow);
        assert_eq!(
            effects,
            [ContestEffect::StopAudio, ContestEffect::ShowNothingKept]
        );
        assert!(
            runtime
                .machine
                .result(0, "2026-10-20".into())
                .groups
                .is_empty()
        );
        assert!(
            runtime.apply(ContestEvent::Resume).is_empty(),
            "finished is final"
        );
        let mut aborted = self::runtime(&["K5XYZ"]);
        let effects = aborted.apply(ContestEvent::Abort);
        assert_eq!(
            effects,
            [ContestEffect::StopAudio, ContestEffect::AbortToHome]
        );
        assert_eq!(aborted.machine.phase(), ContestPhase::Aborted);
    }

    /// − / + moves your speed and later callers', within the Speed card's
    /// bounds; callers already calling keep theirs.
    #[test]
    fn speed_moves_your_message_and_later_callers() {
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        runtime.finish_turn();
        runtime.apply(ContestEvent::Speed { delta: 2 });
        assert_eq!(view(&runtime).my_wpm, 27.0);
        let exchange = {
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let turn = runtime.last_turn().clone();
            assert_eq!(turn.segments[0].voice.char_wpm, 27.0);
            assert_eq!(turn.segments[1].voice.char_wpm, 25.0, "already calling");
            runtime.finish_turn();
            turn.segments[1].text.clone()
        };
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        assert_eq!(runtime.last_turn().segments[1].voice.char_wpm, 27.0);
        runtime.apply(ContestEvent::Speed { delta: -200 });
        assert_eq!(view(&runtime).my_wpm, crate::settings::WPM_MIN);
        runtime.apply(ContestEvent::Speed { delta: 200 });
        assert_eq!(view(&runtime).my_wpm, crate::settings::WPM_MAX);
    }

    /// A caller left waiting tries again on its timeout, and gives up when
    /// its patience runs out; never answered, it leaves no copy.
    #[test]
    fn a_caller_left_waiting_gives_up() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let mut calls = 1;
        while let Some(next) = runtime.next_due() {
            if next >= 300_000 {
                break;
            }
            let before = runtime.turns.len();
            runtime.step();
            if runtime.turns.len() > before {
                assert_eq!(replies(runtime.last_turn()), ["K5XYZ"]);
                calls += 1;
            }
        }
        assert!((4..=14).contains(&calls), "{calls} calls");
        runtime.apply(ContestEvent::FinishNow);
        assert!(runtime.machine.result(0, "x".into()).groups.is_empty());
    }

    /// The rate shows once the run is a minute old, the pace from the first
    /// QSO, and the clock the run bar reads stops at the limit.
    #[test]
    fn the_rate_shows_after_the_first_minute() {
        let mut runtime = runtime(&["K5XYZ"]);
        assert_eq!(view(&runtime).minutes_per_qso(MonoMs(60_000)), None);
        let exchange = to_its_exchange(&mut runtime);
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        let shown = view(&runtime);
        assert_eq!(shown.rate(MonoMs(runtime.now)), None);
        assert_eq!(shown.rate(MonoMs(60_000)), Some(60.0));
        assert_eq!(shown.minutes_per_qso(MonoMs(90_000)), Some(1.5));
        assert_eq!(shown.run_ms(MonoMs(60_000)), 60_000);
        assert_eq!(shown.run_ms(MonoMs(9_000_000)), shown.limit_ms);
        assert_eq!(
            shown.minutes_per_qso(MonoMs(9_000_000)),
            Some(shown.limit_ms as f64 / 60_000.0),
            "the pace stops with the clock"
        );
        assert_eq!(shown.serial, 2, "the next QSO's serial");
    }

    /// The air shows what is still to go of your message, from the
    /// character sounding, and how many are calling, never what they send.
    #[test]
    fn the_air_shows_what_is_still_to_go_and_who_is_calling() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let keyed = runtime.now;
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let air = view(&runtime).air.expect("on the air");
        let at = |ms: u64| MonoMs(keyed + ms);
        assert!(air.my_text.starts_with("K5XYZ 5NN"), "{}", air.my_text);
        assert_eq!(air.still_sending(at(0)), air.my_text);
        let (second, _) = air.my_chars[1];
        let first_end = air.my_chars[0].1;
        let into_second = at((first_end * 1_000.0) as u64 + 1);
        assert_eq!(air.still_sending(into_second), &air.my_text[second..]);
        let (_, your_end) = *air.my_chars.last().expect("characters");
        let after = at((your_end * 1_000.0) as u64 + 1);
        assert_eq!(air.still_sending(after), "");
        assert_eq!(air.calling(at(0)), 0, "only you, to start");
        let (_, start, end) = air.spans[1];
        assert_eq!(air.calling(at(((start + end) * 500.0) as u64)), 1);
        assert_eq!(air.calling(at((end * 1_000.0) as u64 + 1)), 0);
    }

    /// The call group, the field groups and the sends of each, as the run
    /// stored them.
    fn sends(runtime: &Runtime<ScriptCalls>) -> Vec<u32> {
        let (result, _) = result(runtime);
        result.group_timings.iter().map(|t| t.sends).collect()
    }

    /// `?` keyed while their first exchange is still sounding asks for it
    /// again, just as one keyed after it ends: each costs its send.
    #[test]
    fn asking_during_their_first_exchange_costs_its_send() {
        let asked = |into: Option<f64>| {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let reply = runtime.last_turn().segments[1].clone();
            match into {
                // A share of the way into it.
                Some(share) => {
                    let at = reply.start_sec + share * (reply.end_sec - reply.start_sec);
                    runtime.run_to(runtime.now + (at * 1_000.0) as u64);
                }
                None => runtime.finish_turn(),
            }
            runtime.key(OpKey::Again, "K5XYZ", "");
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&reply.text));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            (sends(&runtime), view(&runtime).log[0].asked)
        };
        assert_eq!(asked(None), (vec![1, 2], 1), "after its end");
        assert_eq!(asked(Some(0.98)), (vec![1, 2], 1), "just before its end");
        assert_eq!(asked(Some(0.5)), (vec![1, 2], 1), "halfway through");
    }

    /// The run keeps the repeats you asked for as the live log counts
    /// them, though one asked with the field already right costs its copy
    /// nothing: the results read the same `?×n` as the run did.
    #[test]
    fn the_log_keeps_the_repeats_you_asked_for() {
        let mut runtime = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut runtime);
        let number = serial(&exchange);
        runtime.key(OpKey::Again, "K5XYZ", &number);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &number);
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        assert_eq!(sends(&runtime), [1, 1], "the field already read right");
        assert_eq!(view(&runtime).log[0].asked, 1);
        let (result, run) = result(&runtime);
        assert_eq!(run.qsos[0].a, 1);
        let log = run
            .log(&result.groups, &result.group_timings)
            .expect("the run's groups");
        assert_eq!(log[0].asked, 1);
    }

    /// A `?` before anything of the station has reached you is no repeat:
    /// the call it brings is the first heard.
    #[test]
    fn a_call_heard_once_costs_one_send() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.now += 300;
        runtime.key(OpKey::Again, "", "");
        assert_eq!(yours(runtime.last_turn()), "CQ DL1ABC TEST ?");
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let exchange = replies(runtime.last_turn())[0].to_string();
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        assert_eq!(sends(&runtime), [1, 1]);
    }

    /// Whatever key draws their exchange again, `?`, your exchange again or
    /// their call again, the repeat costs its send, and its copy is timed
    /// from it.
    #[test]
    fn a_repeat_costs_its_send_whatever_key_draws_it() {
        let price = |key: OpKey| {
            let mut runtime = runtime(&["K5XYZ"]);
            let exchange = to_its_exchange(&mut runtime);
            runtime.key(key, "K5XYZ", "");
            assert_eq!(
                replies(runtime.last_turn()),
                [format!("5NN {}", serial(&exchange))],
                "{key:?}"
            );
            runtime.finish_turn();
            runtime.now += 1_000;
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            let timing = &result.group_timings[1];
            (timing.sends, timing.time_to_complete_ms)
        };
        let asked = price(OpKey::Again);
        assert_eq!(asked.0, 2);
        assert!((995.0..=1_005.0).contains(&asked.1), "{}", asked.1);
        assert_eq!(price(OpKey::Exchange), asked, "F2");
        assert_eq!(price(OpKey::HisCall), asked, "F5");
        assert_eq!(price(OpKey::NrQuery), asked, "F12");
    }

    /// A `?` asked once the exchange already reads complete changes neither
    /// its time nor its sends: the copy was done before it.
    #[test]
    fn a_question_after_the_exchange_is_copied_leaves_its_copy_alone() {
        let fields = |exch: &str, edited: u64, done: u64| Fields {
            call: "K5XYZ".into(),
            exch: exch.into(),
            call_edited: MonoMs(edited),
            exch_complete: Some(MonoMs(done)),
        };
        let nr_copy = |ask: bool| {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            let edited = runtime.now;
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let nr = serial(replies(runtime.last_turn())[0]);
            runtime.finish_turn();
            runtime.now += 2_000;
            let typed = runtime.now;
            if ask {
                let fields = fields(&nr, edited, typed);
                runtime.apply(ContestEvent::Key {
                    key: OpKey::Again,
                    fields,
                });
                runtime.finish_turn();
            }
            let fields = fields(&nr, edited, typed);
            runtime.apply(ContestEvent::Key {
                key: OpKey::Esm,
                fields,
            });
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            let timing = &result.group_timings[1];
            (timing.time_to_complete_ms, timing.sends)
        };
        let plain = nr_copy(false);
        assert!((1_995.0..=2_005.0).contains(&plain.0), "{}", plain.0);
        assert_eq!(nr_copy(true), plain);
    }

    /// A call lost, never sent with your exchange, is timed as it was
    /// copied when you last sent it, not as instant.
    #[test]
    fn a_lost_call_keeps_its_copy_time() {
        let fields = |edited: u64| Fields {
            call: "K5XYZ".into(),
            call_edited: MonoMs(edited),
            ..Fields::default()
        };
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        runtime.finish_turn();
        runtime.now += 6_000;
        let edited = runtime.now;
        for key in [OpKey::HisCall, OpKey::Cq] {
            let fields = fields(edited);
            runtime.apply(ContestEvent::Key { key, fields });
            runtime.finish_turn();
        }
        runtime.apply(ContestEvent::FinishNow);
        let (lost, run) = result(&runtime);
        assert_eq!(run.qsos[0].k, "lost");
        let took = lost.group_timings[0].time_to_complete_ms;
        assert!((5_995.0..=6_005.0).contains(&took), "{took}");
    }

    /// Once a CQ or a TU has ended the QSO, Enter starts the next one
    /// afresh: the next caller gets its call and your exchange.
    #[test]
    fn a_cq_or_tu_ends_the_qso_for_enter_too() {
        for (key, call) in [(OpKey::Esm, ""), (OpKey::Cq, "K5XYZ"), (OpKey::Tu, "K5XYZ")] {
            let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
            to_its_exchange(&mut runtime);
            runtime.key(key, call, "");
            assert_eq!(replies(runtime.last_turn()), ["G4ABC"], "{key:?}");
            runtime.finish_turn();
            // F3 keeps the call it went to, for logging that QSO.
            let thanked = if key == OpKey::Tu { "K5XYZ" } else { "" };
            let qso = QsoState {
                thanked: thanked.into(),
                ..QsoState::default()
            };
            assert_eq!(view(&runtime).qso, qso, "{key:?}");
            runtime.key(OpKey::Esm, "G4ABC", "");
            assert_eq!(yours(runtime.last_turn()), "G4ABC 5NN TT1", "{key:?}");
        }
    }

    /// Your exchange sent with a call nobody takes for its own is owed
    /// still: the next call goes out with it, and Enter never asks `?` for
    /// a number the station waits for you to send.
    #[test]
    fn an_exchange_nobody_took_is_owed_still() {
        for (call, slip) in [("K5XYZ", "N8XUQ"), ("K5XY", "K5YX")] {
            let mut runtime = runtime(&[call]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, slip, "");
            assert_eq!(yours(runtime.last_turn()), format!("{slip} 5NN TT1"));
            assert!(replies(runtime.last_turn()).is_empty(), "not its call");
            assert!(!view(&runtime).qso.exchange_sent);
            assert_eq!(replies(&next_turn(&mut runtime)), [call]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, call, "");
            assert_eq!(yours(runtime.last_turn()), format!("{call} 5NN TT1"));
            assert!(replies(runtime.last_turn())[0].contains("5NN"));
        }
    }

    /// A partial while you work the station asks for its call: the station
    /// sends its call, and the number is not charged for it.
    #[test]
    fn a_partial_while_you_work_the_station_brings_its_call() {
        let mut runtime = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut runtime);
        runtime.key(OpKey::Esm, "K5X?Z", "");
        assert_eq!(yours(runtime.last_turn()), "K5X?Z");
        let back = replies(runtime.last_turn())[0].to_string();
        assert!(back.contains("K5XYZ"), "asked for the call, got {back}");
        runtime.finish_turn();
        assert!(!view(&runtime).qso.corrected, "an answer, not a correction");
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        assert_eq!(yours(runtime.last_turn()), "K5XYZ TU");
        runtime.finish_turn();
        assert_eq!(view(&runtime).clean, 1);
        runtime.apply(ContestEvent::FinishNow);
        assert_eq!(sends(&runtime), [1, 1]);
        // Waiting for your exchange, it sends its call too.
        let mut waiting = self::runtime(&["K5XYZ"]);
        waiting.finish_turn();
        waiting.key(OpKey::HisCall, "K5XYZ", "");
        waiting.finish_turn();
        waiting.key(OpKey::Esm, "K5X?", "");
        let back = replies(waiting.last_turn())[0].to_string();
        assert!(back.contains("K5XYZ"), "asked for the call, got {back}");
    }

    /// CQ after CQ spends the caller's tries, and then someone else
    /// answers.
    #[test]
    fn cq_after_cq_moves_the_frequency_on() {
        let mut runtime = runtime(&["K5XYZ", "W7ABC"]);
        runtime.finish_turn();
        let mut answered = Vec::new();
        for _ in 0..15 {
            runtime.key(OpKey::Esm, "", "");
            answered.push(replies(runtime.last_turn()).join(","));
            runtime.finish_turn();
        }
        let k5xyz = answered.iter().take_while(|who| *who == "K5XYZ").count();
        assert!((3..=13).contains(&k5xyz), "{answered:?}");
        assert_eq!(answered[k5xyz], "W7ABC", "{answered:?}");
    }

    /// A station is copied once, however often it is logged: a call logged
    /// again, a station lost then logged, a call logged with nobody heard,
    /// and a station logged as a dupe each add no group.
    #[test]
    fn one_station_has_one_copy() {
        // The same call logged again and again.
        let mut again = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut again);
        for _ in 0..6 {
            again.key(OpKey::LogOnly, "K5XYZ", &serial(&exchange));
        }
        again.apply(ContestEvent::FinishNow);
        let (result, run) = result(&again);
        assert_eq!(result.groups.len(), 2, "{:?}", result.groups);
        assert_eq!(run.qsos.len(), 6);
        assert!(run.qsos[1..].iter().all(|qso| qso.g == 0));
        assert_eq!(again.machine.source.learned.len(), 2, "taught once");
        // Asked for its exchange, then left after three silent timeouts:
        // lost, and the log after that is a NIL with no group of its own.
        let mut left = self::runtime(&["K5XYZ"]);
        left.finish_turn();
        left.key(OpKey::HisCall, "K5XYZ", "");
        left.finish_turn();
        left.run_to(left.now + 60_000);
        left.key(OpKey::LogOnly, "K5XYZ", "7");
        left.apply(ContestEvent::FinishNow);
        let (result, run) = self::result(&left);
        let kinds: Vec<(&str, u32)> = run.qsos.iter().map(|q| (q.k.as_str(), q.g)).collect();
        assert_eq!(kinds, [("lost", 2), ("nil", 0)]);
        assert_eq!(result.groups.len(), 2);
        // Nobody heard: the CQ stopped before the call came.
        let mut nobody = self::runtime(&["K5XYZ"]);
        nobody.now += 300;
        nobody.key(OpKey::StopTx, "", "");
        let effects = nobody.key(OpKey::LogOnly, "W9ZZZ", "123");
        assert!(effects.contains(&ContestEffect::ClearFields));
        let (result, run) = self::result(&nobody);
        assert!(result.groups.is_empty());
        assert_eq!(
            (run.qsos[0].k.as_str(), run.qsos[0].lc.as_str()),
            ("nil", "W9ZZZ")
        );
        // No station worked: nothing is kept.
        let effects = nobody.apply(ContestEvent::FinishNow);
        assert_eq!(effects.last(), Some(&ContestEffect::ShowNothingKept));
        // Logged, then its call sent and logged again: dupes, which keep
        // what the check needs in the record.
        let mut dupes = self::runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut dupes);
        dupes.key(OpKey::LogOnly, "K5XYZ", &serial(&exchange));
        for _ in 0..3 {
            dupes.key(OpKey::HisCall, "K5XYZ", "");
            dupes.finish_turn();
            dupes.key(OpKey::LogOnly, "K5XYZ", &serial(&exchange));
        }
        let view = view(&dupes);
        assert!(view.log[1..].iter().all(|row| row.status
            == RowStatus::Checked(QsoCheck {
                errors: Vec::new(),
                dupe: true
            })));
        dupes.apply(ContestEvent::FinishNow);
        let (result, run) = self::result(&dupes);
        assert_eq!(result.groups.len(), 2);
        assert_eq!(run.qsos[1].sc, "K5XYZ");
        assert_eq!(run.qsos[1].s, [serial(&exchange)]);
        let score = run.score(&result.groups).expect("WPX scores");
        assert_eq!((score.raw.qso_points, score.verified.qso_points), (1, 1));
        assert_eq!(dupes.machine.copies_by_station().get(&0), Some(&1));
    }

    /// The station you worked left after three silent timeouts: logging it
    /// then is a NIL, and that station keeps its one lost copy.
    #[test]
    fn a_station_lost_then_logged_has_one_copy() {
        let mut runtime = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut runtime);
        while runtime.next_due().is_some_and(|due| due < 100_000) {
            runtime.step();
        }
        runtime.key(OpKey::TuAndLog, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        let kinds: Vec<(&str, u32)> = run.qsos.iter().map(|q| (q.k.as_str(), q.g)).collect();
        assert_eq!(kinds, [("lost", 2), ("nil", 0)]);
        assert_eq!(result.groups.len(), 2, "{:?}", result.groups);
    }

    /// A NIL for a station already lost is never charged to the caller who
    /// answered since, and that caller's own QSO is still lost when it
    /// goes unlogged.
    #[test]
    fn a_nil_is_never_charged_to_the_next_caller() {
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        let exchange = to_its_exchange(&mut runtime);
        runtime.key(OpKey::Cq, "K5XYZ", "");
        runtime.finish_turn();
        runtime.key(OpKey::TuAndLog, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "G4ABC", "");
        runtime.finish_turn();
        runtime.run_to(600_000);
        let (result, run) = result(&runtime);
        assert!(
            result
                .groups
                .iter()
                .all(|g| g.sent != "G4ABC" || g.received == "G4ABC"),
            "{:?}",
            result.groups
        );
        let kinds: Vec<(&str, &str)> = run
            .qsos
            .iter()
            .map(|q| (q.k.as_str(), q.c.as_str()))
            .collect();
        assert_eq!(kinds, [("lost", "K"), ("nil", ""), ("lost", "G")]);
        assert!(
            !runtime
                .machine
                .source
                .learned
                .contains(&("G4ABC".into(), "K5XYZ".into()))
        );
    }

    /// A QSO logged before the station's exchange reached you copied no
    /// number: its field group is a miss, as a guess is no copy, and the
    /// record holds what the check needs.
    #[test]
    fn a_number_logged_before_it_was_heard_is_a_miss() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        let ask = replies(runtime.last_turn());
        assert!(matches!(ask[..], ["AGN"] | ["NR?"]), "{ask:?}");
        runtime.finish_turn();
        runtime.key(OpKey::TuAndLog, "K5XYZ", "123");
        runtime.finish_turn();
        let row = &view(&runtime).log[0];
        assert_eq!(
            row.status,
            RowStatus::Checked(QsoCheck {
                errors: vec![QsoError::Field(Field::Nr)],
                dupe: false
            })
        );
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(result.groups.len(), 2, "{:?}", result.groups);
        assert_eq!(result.groups[1].received, "", "a miss");
        assert_eq!(result.group_timings[1].sends, 1);
        assert_eq!(run.qsos[0].l, ["123"]);
        assert_eq!(run.qsos[0].s.len(), 1);
        assert_eq!(runtime.machine.source.learned.len(), 2);
        let score = run.score(&result.groups).expect("WPX scores");
        assert_eq!((score.raw.qso_points, score.verified.qso_points), (1, 0));
    }

    /// After a correction, Enter on the call as sent waits for it to be
    /// fixed rather than asking `?` for a number that will not come.
    #[test]
    fn after_a_correction_enter_waits_for_the_call() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYW", "");
        runtime.finish_turn();
        assert!(view(&runtime).qso.corrected);
        let effects = runtime.key(OpKey::Esm, "K5XYW", "");
        assert!(played(&effects).is_none());
        assert!(effects.contains(&ContestEffect::Focus(EntryField::Call)));
        let fields = Fields {
            call: "K5XYW".into(),
            ..Fields::default()
        };
        let enter = esm(&view(&runtime).qso, &fields, &RULES[0], Side::Dx);
        assert_eq!(enter.label, "Fix call");
    }

    /// Esc stops a CQ before it went out: no caller answers it.
    #[test]
    fn a_cq_stopped_before_it_went_out_brings_no_caller() {
        let mut runtime = runtime(&["K5XYZ"]);
        assert_eq!(yours(runtime.last_turn()), "CQ DL1ABC TEST");
        runtime.key(OpKey::StopTx, "", "");
        let stopped = runtime.turns.len();
        runtime.run_to(runtime.now + 60_000);
        assert_eq!(runtime.turns.len(), stopped, "answered a CQ never sent");
        // The next CQ brings a caller as any does.
        runtime.key(OpKey::Esm, "", "");
        assert_eq!(replies(runtime.last_turn()).len(), 1);
    }

    /// Esc stops a CQ keyed by mistake: the QSO it would have ended is as
    /// it was, and logging it is clean.
    #[test]
    fn esc_takes_back_an_accidental_cq() {
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        let exchange = to_its_exchange(&mut runtime);
        runtime.key(OpKey::Cq, "K5XYZ", &serial(&exchange));
        runtime.now += 20;
        runtime.key(OpKey::StopTx, "K5XYZ", &serial(&exchange));
        assert!(view(&runtime).qso.exchange_sent, "as before the CQ");
        runtime.key(OpKey::TuAndLog, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        assert_eq!(
            view(&runtime).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(run.qsos.len(), 1, "{:?}", run.qsos);
        assert_eq!(result.groups.len(), 2);
        assert_eq!(
            runtime.machine.source.learned.len(),
            2,
            "nothing lost taught"
        );
    }

    /// Wipe, then Esc: the QSO's keying started again, and Esc does not
    /// bring back what Enter read before.
    #[test]
    fn wipe_then_esc_brings_nothing_back() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYW", "");
        runtime.finish_turn();
        assert!(view(&runtime).qso.corrected);
        runtime.key(OpKey::Esm, "K5XYZ", "");
        runtime.key(OpKey::Wipe, "K5XYZ", "");
        runtime.key(OpKey::StopTx, "", "");
        assert_eq!(view(&runtime).qso, QsoState::default());
    }

    /// A timer forgotten when you keyed, waking while the turn plays, reads
    /// the clock without keeping the reading: a sound failure afterwards
    /// still gives back every retry.
    #[test]
    fn a_forgotten_timer_waking_mid_turn_leaves_the_retries_out_of_run_time() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let stale = runtime.next_due().expect("the patience timer");
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let (id, keyed) = (runtime.last_turn().id, runtime.now);
        runtime.step();
        runtime.run_to(stale);
        assert_eq!(runtime.playing_id(), Some(id), "still on the air");
        runtime.now = keyed + 12_000;
        runtime.apply(ContestEvent::TurnFailed { id });
        assert!(paused_by_sound(&runtime));
        let shown = view(&runtime)
            .clock
            .map(|mut clock| clock.elapsed_ms(MonoMs(runtime.now)));
        assert_eq!(shown, Some(keyed), "the retries are not run time");
    }

    /// Pause and Resume is no free `?`: the station whose exchange you
    /// heard whole waits for its timeout, as it would have without the
    /// pause, and only a send the pause cut short comes again at once.
    #[test]
    fn pause_and_resume_is_no_free_again() {
        let mut runtime = runtime(&["K5XYZ"]);
        to_its_exchange(&mut runtime);
        runtime.apply(ContestEvent::Pause {
            why: PauseWhy::You,
            at: MonoMs(runtime.now),
        });
        runtime.now += 5_000;
        runtime.apply(ContestEvent::Resume);
        let resumed = runtime.now;
        let again = next_turn(&mut runtime);
        assert!(replies(&again)[0].contains("5NN"));
        // The shortest wait any caller has is a second and a half.
        assert!(
            runtime.now >= resumed + RESUME_DELAY_MS + 1_500,
            "sent again {} ms after Resume",
            runtime.now - resumed
        );
        assert_eq!(yours(&again), "", "on its timeout");
        // Cut short by the pause, it sends again once the resume timer is up.
        let mut cut = self::runtime(&["K5XYZ"]);
        cut.finish_turn();
        cut.key(OpKey::Esm, "K5XYZ", "");
        cut.apply(ContestEvent::Pause {
            why: PauseWhy::You,
            at: MonoMs(cut.now),
        });
        cut.apply(ContestEvent::Resume);
        let resumed = cut.now;
        let again = next_turn(&mut cut);
        assert_eq!(cut.now, resumed + RESUME_DELAY_MS);
        assert!(replies(&again)[0].contains("5NN"));
    }

    /// A minute's run with a turn keyed ten seconds before its end, whose
    /// first attempt stalled and whose retry has started: the machine, the
    /// ids of its run end, of a patience timer since forgotten, of the CQ
    /// and of the turn.
    fn retrying_near_the_end() -> (ContestMachine<ScriptCalls>, u64, u64, u64, u64) {
        let first_timer = |effects: &[ContestEffect]| {
            effects.iter().find_map(|effect| match effect {
                ContestEffect::StartTimer { id, .. } => Some(*id),
                _ => None,
            })
        };
        let mut settings = run_settings(25.0);
        settings.contest.contest_minutes = 1;
        let (mut machine, effects) =
            ContestMachine::start(1, MonoMs(0), settings, ScriptCalls::new(&["K5XYZ"]));
        let run_end = first_timer(&effects).expect("the run end");
        let cq = played(&effects).expect("the CQ").id;
        let ended = machine.apply(ContestEvent::TurnEnded { id: cq }, MonoMs(5_000));
        let patience = first_timer(&ended).expect("the patience timer");
        let fields = Fields {
            call: "K5XYZ".into(),
            call_edited: MonoMs(50_000),
            ..Fields::default()
        };
        let key = ContestEvent::Key {
            key: OpKey::Esm,
            fields,
        };
        let turn = played(&machine.apply(key, MonoMs(50_000)))
            .expect("a turn")
            .id;
        for (heard_from, now) in [(50_100, 50_050), (56_100, 56_050)] {
            let started = ContestEvent::TurnStarted {
                id: turn,
                heard_from: MonoMs(heard_from),
            };
            let _ = machine.apply(started, MonoMs(now));
        }
        (machine, run_end, patience, cq, turn)
    }

    /// Retries of a sound are not run time (5.2): the limit passing while
    /// the turn on the air is tried again does not end the run, nor does a
    /// stale send's news or a forgotten timer then. Its failure pauses the
    /// run with the retries given back; its end, or its last attempt
    /// running out, ends the run at the limit.
    #[test]
    fn the_run_end_waits_for_a_sound_being_retried() {
        let (mut failed, run_end, patience, cq, turn) = retrying_near_the_end();
        let rearmed = failed.apply(ContestEvent::TimerFired { id: run_end }, MonoMs(60_000));
        assert!(
            matches!(rearmed[..], [ContestEffect::StartTimer { .. }]),
            "{rearmed:?}"
        );
        let stale = ContestEvent::TurnCancelled { id: cq };
        assert!(failed.apply(stale, MonoMs(61_000)).is_empty());
        let forgotten = ContestEvent::TimerFired { id: patience };
        assert!(failed.apply(forgotten, MonoMs(61_500)).is_empty());
        assert_eq!(failed.phase(), ContestPhase::Running);
        let _ = failed.apply(ContestEvent::TurnFailed { id: turn }, MonoMs(68_000));
        assert_eq!(
            failed.phase(),
            ContestPhase::Paused {
                why: PauseWhy::Sound
            }
        );
        let clock = failed.view().clock;
        assert_eq!(
            clock.map(|mut clock| clock.elapsed_ms(MonoMs(68_000))),
            Some(50_100),
            "run time stopped where the turn first started"
        );

        // The retry plays after all: its end ends the run, at the limit.
        let (mut ended, run_end, _, _, turn) = retrying_near_the_end();
        let _ = ended.apply(ContestEvent::TimerFired { id: run_end }, MonoMs(60_000));
        let effects = ended.apply(ContestEvent::TurnEnded { id: turn }, MonoMs(67_500));
        assert!(effects.contains(&ContestEffect::Announce("Time's up".into())));
        assert_eq!(ended.ended.map(|(run_ms, _)| run_ms), Some(60_000));

        // Neither comes: the run ends once its last attempt would have
        // failed.
        let (mut silent, run_end, _, _, _) = retrying_near_the_end();
        let rearmed = silent.apply(ContestEvent::TimerFired { id: run_end }, MonoMs(60_000));
        let [ContestEffect::StartTimer { id, ms }] = rearmed[..] else {
            panic!("{rearmed:?}");
        };
        let _ = silent.apply(
            ContestEvent::TimerFired { id },
            MonoMs(60_000 + u64::from(ms)),
        );
        assert_eq!(silent.phase(), ContestPhase::Finished);
        assert_eq!(silent.ended.map(|(run_ms, _)| run_ms), Some(60_000));
    }

    /// Logged without sending while a reply still plays, a QSO counts what
    /// of it had reached you first, as TU and log does: its exchange is a
    /// copy, and a repeat you asked for is charged.
    #[test]
    fn logging_without_sending_counts_what_has_reached_you_first() {
        let groups = |key: OpKey, whole: bool| {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let reply = runtime.last_turn().segments[1].clone();
            let end = runtime.now + whole_ms(reply.end_sec);
            if whole {
                // Heard whole, the turn's end not yet come.
                runtime.now = end;
            } else {
                runtime.run_to(end - 100);
            }
            runtime.key(key, "K5XYZ", &serial(&reply.text));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            let copies: Vec<(String, String)> = result
                .groups
                .iter()
                .map(|group| (group.sent.clone(), group.received.clone()))
                .collect();
            (copies, runtime.machine.source.learned.len())
        };
        let logged = groups(OpKey::TuAndLog, false);
        assert_eq!(logged.0.len(), 2);
        assert!(logged.0.iter().all(|(sent, received)| sent == received));
        assert_eq!(groups(OpKey::LogOnly, false), logged);
        assert_eq!(groups(OpKey::LogOnly, true), logged);
        // A `?` whose repeat still plays.
        let asked = |key: OpKey| {
            let mut runtime = runtime(&["K5XYZ"]);
            let exchange = to_its_exchange(&mut runtime);
            runtime.key(OpKey::Again, "K5XYZ", "");
            let repeat = runtime.last_turn().segments[1].clone();
            runtime.now += whole_ms(repeat.end_sec) - 100;
            runtime.key(key, "K5XYZ", &serial(&exchange));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            (sends(&runtime), view(&runtime).log[0].asked)
        };
        assert_eq!(asked(OpKey::TuAndLog), (vec![1, 2], 1));
        assert_eq!(asked(OpKey::LogOnly), (vec![1, 2], 1));
    }

    /// A pause in the moment after Resume, with nothing on the air yet,
    /// keeps who the pause or sound failure before it cut short: they send
    /// again a moment after the next Resume.
    #[test]
    fn a_pause_just_after_resume_keeps_who_was_cut_short() {
        for failed in [false, true] {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            if failed {
                runtime.now += 4_000;
                let id = runtime.last_turn().id;
                runtime.apply(ContestEvent::TurnFailed { id });
            } else {
                runtime.apply(ContestEvent::Pause {
                    why: PauseWhy::You,
                    at: MonoMs(runtime.now),
                });
            }
            runtime.now += 2_000;
            runtime.apply(ContestEvent::Resume);
            runtime.now += 300;
            runtime.apply(ContestEvent::Pause {
                why: PauseWhy::Hidden,
                at: MonoMs(runtime.now),
            });
            runtime.now += 2_000;
            runtime.apply(ContestEvent::Resume);
            let resumed = runtime.now;
            let again = next_turn(&mut runtime);
            assert!(replies(&again)[0].contains("5NN"), "failed: {failed}");
            assert_eq!(runtime.now - resumed, RESUME_DELAY_MS, "failed: {failed}");
        }
    }

    /// A repeat you asked for, cut by a pause or a sound failure before it
    /// began to reach you, comes after Resume as you asked for it, and is
    /// charged once heard.
    #[test]
    fn a_repeat_cut_before_it_began_is_charged_once_heard() {
        for failed in [false, true] {
            let mut runtime = runtime(&["K5XYZ"]);
            let exchange = to_its_exchange(&mut runtime);
            runtime.key(OpKey::Again, "K5XYZ", "");
            if failed {
                runtime.now += 4_000;
                let id = runtime.last_turn().id;
                runtime.apply(ContestEvent::TurnFailed { id });
            } else {
                runtime.now += 50;
                runtime.apply(ContestEvent::Pause {
                    why: PauseWhy::You,
                    at: MonoMs(runtime.now),
                });
            }
            runtime.now += 3_000;
            runtime.apply(ContestEvent::Resume);
            runtime.run_to(runtime.now + RESUME_DELAY_MS);
            assert_eq!(
                replies(runtime.last_turn()),
                [format!("5NN {}", serial(&exchange))]
            );
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            assert_eq!(
                (sends(&runtime), view(&runtime).log[0].asked),
                (vec![1, 2], 1),
                "failed: {failed}"
            );
        }
    }

    /// A pause you choose that cuts a send already reaching you is no free
    /// repeat: as a send heard whole, it comes again on the station's
    /// timeout. Only a hidden page or a failed sound brings it back a
    /// moment after Resume.
    #[test]
    fn a_pause_you_choose_mid_send_brings_no_quick_repeat() {
        for why in [PauseWhy::You, PauseWhy::Hidden] {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let reply = runtime.last_turn().segments[1].clone();
            let at = reply.start_sec + 0.95 * (reply.end_sec - reply.start_sec);
            runtime.run_to(runtime.now + (at * 1_000.0) as u64);
            runtime.apply(ContestEvent::Pause {
                why,
                at: MonoMs(runtime.now),
            });
            runtime.now += 3_000;
            runtime.apply(ContestEvent::Resume);
            let resumed = runtime.now;
            let again = next_turn(&mut runtime);
            assert!(replies(&again)[0].contains("5NN"), "{why:?}");
            let after = runtime.now - resumed;
            if why == PauseWhy::You {
                // The shortest wait any caller has is a second and a half.
                assert!(after >= RESUME_DELAY_MS + 1_500, "{after} ms after Resume");
            } else {
                assert_eq!(after, RESUME_DELAY_MS);
            }
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&reply.text));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            assert_eq!(sends(&runtime), [1, 1], "no repeat you asked for");
        }
    }

    /// Your exchange sent without a call (F2) goes with the call last sent
    /// to that station, even once the Call field emptied or Wipe started
    /// the QSO's keying again: a clean QSO keeps a clean call copy.
    #[test]
    fn a_message_without_a_call_goes_with_the_call_last_sent_to_the_station() {
        for reset in [OpKey::Again, OpKey::Wipe] {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::HisCall, "K5XYZ", "");
            runtime.finish_turn();
            if reset == OpKey::Again {
                // `?` with the Call field empty.
                runtime.key(OpKey::Again, "", "");
                runtime.finish_turn();
            } else {
                runtime.key(OpKey::Wipe, "K5XYZ", "");
            }
            runtime.key(OpKey::Exchange, "K5XYZ", "");
            let exchange = replies(runtime.last_turn())[0].to_string();
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
            runtime.finish_turn();
            assert_eq!(
                view(&runtime).log[0].status,
                RowStatus::Checked(QsoCheck::default())
            );
            runtime.apply(ContestEvent::FinishNow);
            let (result, run) = result(&runtime);
            assert_eq!(result.groups[0].received, "K5XYZ", "{reset:?}");
            assert_eq!(run.qsos[0].lc, "", "{reset:?}");
        }
    }

    /// A NIL that matches no station goes to the newest heard, and to no
    /// older one when that one already has its copy.
    #[test]
    fn a_nil_matching_nobody_goes_only_to_the_newest_station_heard() {
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        runtime.finish_turn();
        while replies(runtime.last_turn()) != ["G4ABC"] {
            runtime.key(OpKey::Cq, "", "");
            runtime.finish_turn();
        }
        runtime.key(OpKey::Esm, "G4ABC", "");
        let exchange = replies(runtime.last_turn())[0].to_string();
        runtime.finish_turn();
        runtime.key(OpKey::LogOnly, "G4ABC", &serial(&exchange));
        runtime.key(OpKey::LogOnly, "W9ZZZ", "1");
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert!(
            result.groups.iter().all(|group| group.sent != "K5XYZ"),
            "{:?}",
            result.groups
        );
        assert_eq!(run.qsos[1].lc, "W9ZZZ", "the record holds the call");
    }

    /// A partial while the station waits for your exchange asks for its
    /// call: the answer costs the call a send, and its copy is timed from
    /// it, as when the station still calls.
    #[test]
    fn a_partial_while_the_station_waits_is_charged_and_timed() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        let effects = runtime.key(OpKey::Esm, "K5X?", "");
        assert!(replies(runtime.last_turn())[0].contains("K5XYZ"));
        runtime.finish_turn();
        assert!(!view(&runtime).qso.corrected, "an answer, not a correction");
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, ContestEffect::Announce(_)))
        );
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let exchange = replies(runtime.last_turn())[0].to_string();
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        let call = &result.group_timings[0];
        assert_eq!(call.sends, 2);
        assert!(call.time_to_complete_ms < 1_000.0, "{call:?}");
        assert_eq!(view(&runtime).log[0].asked, 1);
    }

    /// TU without logging (F3), then a log of that call, is checked against
    /// the station the TU closed, as MorseRunner's log finds a station done
    /// with you, a CQ between them or not; Enter or TU and log on its call
    /// then sends the TU alone and logs. Work another station instead, and
    /// that QSO is lost.
    #[test]
    fn a_tu_without_logging_then_a_log_is_checked_against_that_station() {
        for key in [OpKey::LogOnly, OpKey::Esm, OpKey::TuAndLog, OpKey::Cq] {
            let mut runtime = runtime(&["K5XYZ"]);
            let exchange = to_its_exchange(&mut runtime);
            let nr = serial(&exchange);
            runtime.key(OpKey::Tu, "K5XYZ", &nr);
            runtime.finish_turn();
            if key == OpKey::Cq {
                runtime.key(OpKey::Cq, "", "");
                runtime.finish_turn();
            }
            let key = if key == OpKey::Cq {
                OpKey::LogOnly
            } else {
                key
            };
            runtime.key(key, "K5XYZ", &nr);
            if key != OpKey::LogOnly {
                // Its call still counts as sent, as MorseRunner's does.
                assert_eq!(yours(runtime.last_turn()), "TU", "{key:?}");
            }
            runtime.finish_turn();
            assert_eq!(
                view(&runtime).log[0].status,
                RowStatus::Checked(QsoCheck::default()),
                "{key:?}"
            );
            runtime.apply(ContestEvent::FinishNow);
            let (_, run) = result(&runtime);
            let kinds: Vec<&str> = run.qsos.iter().map(|qso| qso.k.as_str()).collect();
            assert_eq!(kinds, [""], "{key:?}");
        }
        let mut moved_on = runtime(&["K5XYZ", "G4ABC"]);
        let exchange = to_its_exchange(&mut moved_on);
        moved_on.key(OpKey::Tu, "K5XYZ", &serial(&exchange));
        moved_on.finish_turn();
        moved_on.key(OpKey::Esm, "G4ABC", "");
        let theirs = replies(moved_on.last_turn())[0].to_string();
        moved_on.finish_turn();
        moved_on.key(OpKey::Esm, "G4ABC", &serial(&theirs));
        moved_on.finish_turn();
        moved_on.apply(ContestEvent::FinishNow);
        let (result, run) = result(&moved_on);
        let kinds: Vec<(&str, &str)> = run
            .qsos
            .iter()
            .map(|qso| (qso.k.as_str(), qso.c.as_str()))
            .collect();
        assert_eq!(kinds, [("lost", "K"), ("", "G")]);
        assert_eq!(result.groups[0].received, "K5XYZ");
        assert_eq!(view(&moved_on).clean, 1);
    }

    /// Esc takes back the TU of F3 only while it had not gone out whole:
    /// either way, logging the QSO then is clean, and it is logged once.
    #[test]
    fn esc_after_a_tu_without_logging_leaves_that_qso_to_log() {
        for whole in [false, true] {
            let mut runtime = runtime(&["K5XYZ"]);
            let exchange = to_its_exchange(&mut runtime);
            let nr = serial(&exchange);
            runtime.key(OpKey::Tu, "K5XYZ", &nr);
            if whole {
                let tu = runtime.last_turn().clone();
                runtime.now += whole_ms(tu.segments[0].end_sec) + 20;
            }
            runtime.key(OpKey::StopTx, "K5XYZ", &nr);
            let thanked = if whole { "K5XYZ" } else { "" };
            assert_eq!(view(&runtime).qso.thanked, thanked);
            runtime.key(OpKey::LogOnly, "K5XYZ", &nr);
            assert_eq!(
                view(&runtime).log[0].status,
                RowStatus::Checked(QsoCheck::default()),
                "whole: {whole}"
            );
            runtime.apply(ContestEvent::FinishNow);
            let (_, run) = result(&runtime);
            assert_eq!(run.qsos.len(), 1, "{:?}", run.qsos);
        }
    }

    /// Your exchange after a partial the caller answered, in the same
    /// message or the next, is for it: it puts its call right, and a `?`
    /// after that is answered too.
    #[test]
    fn your_exchange_after_an_answered_partial_is_answered() {
        let mut together = runtime(&["K5XYZ"]);
        together.finish_turn();
        together.key(OpKey::HisCallAndExchange, "K5?", "");
        assert_eq!(yours(together.last_turn()), "K5? 5NN TT1");
        assert!(replies(together.last_turn())[0].contains("K5XYZ"));
        let mut after = runtime(&["K5XYZ"]);
        after.finish_turn();
        after.key(OpKey::Esm, "K5?", "");
        assert_eq!(replies(after.last_turn()), ["K5XYZ"]);
        after.finish_turn();
        after.key(OpKey::Exchange, "K5?", "");
        assert!(
            replies(after.last_turn())[0].contains("K5XYZ"),
            "F2 drew nothing"
        );
        after.finish_turn();
        after.key(OpKey::Again, "K5?", "");
        assert!(!replies(after.last_turn()).is_empty(), "? drew nothing");
        after.finish_turn();
        // Its call put right, your TU logs the QSO clean.
        after.key(OpKey::Esm, "K5XYZ", "");
        assert_eq!(yours(after.last_turn()), "K5XYZ ?");
        let exchange = replies(after.last_turn())[0].to_string();
        after.finish_turn();
        after.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        after.finish_turn();
        assert_eq!(view(&after).clean, 1);
    }

    /// `?` with the Call field empty brings back a caller that heard you
    /// send another call, or your exchange alone, as MorseRunner's
    /// `osNeedPrevEnd` comes back when the field is empty.
    #[test]
    fn a_question_with_the_call_field_empty_brings_the_caller_back() {
        for first in [OpKey::Esm, OpKey::Exchange] {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            if first == OpKey::Esm {
                runtime.key(OpKey::Esm, "N8XUQ", "");
                runtime.finish_turn();
                runtime.key(OpKey::Wipe, "N8XUQ", "");
            } else {
                runtime.key(OpKey::Exchange, "", "");
                runtime.finish_turn();
            }
            assert!(runtime.key(OpKey::Again, "K5", "").iter().all(|effect| {
                !matches!(effect, ContestEffect::PlayTurn(turn) if !replies(turn).is_empty())
            }));
            runtime.finish_turn();
            let effects = runtime.key(OpKey::Again, "", "");
            assert_eq!(
                replies(played(&effects).expect("? goes out")),
                ["K5XYZ"],
                "{first:?}"
            );
        }
    }

    /// A piece of a call sent alone, `5XYZ` or `K5`, asks for the rest as a
    /// partial does: the answer costs a send and a try, and is no
    /// correction.
    #[test]
    fn a_piece_of_a_call_sent_alone_asks_as_a_partial() {
        let run = |typed: &str| {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, typed, "");
            assert!(replies(runtime.last_turn())[0].contains("K5XYZ"), "{typed}");
            runtime.finish_turn();
            assert!(!view(&runtime).qso.corrected, "{typed}");
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let exchange = replies(runtime.last_turn())[0].to_string();
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            sends(&runtime)
        };
        assert_eq!(run("?5XYZ"), [2, 1]);
        assert_eq!(run("K5?"), [2, 1]);
        assert_eq!(run("5XYZ"), run("?5XYZ"));
        assert_eq!(run("K5"), run("K5?"));
        // Sent again and again, it spends the caller's patience.
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let mut answered = 0;
        for _ in 0..15 {
            runtime.key(OpKey::Esm, "K5", "");
            answered += usize::from(!replies(runtime.last_turn()).is_empty());
            runtime.finish_turn();
        }
        assert!(answered < 15, "answered {answered} of 15");
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
    }

    /// A QSO given up after a correction keeps the call first sent with
    /// your exchange as its copy, as logging it would: walking away pays no
    /// better than finishing.
    #[test]
    fn a_lost_qso_keeps_the_call_first_sent_with_your_exchange() {
        let run = |log: bool| {
            let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYW", "");
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let nr = serial(replies(runtime.last_turn())[0]);
            runtime.finish_turn();
            let key = if log { OpKey::Esm } else { OpKey::Cq };
            runtime.key(key, "K5XYZ", &nr);
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            (result.groups[0].received.clone(), result.score)
        };
        let (logged, logged_points) = run(true);
        let (lost, lost_points) = run(false);
        assert_eq!(logged, "K5XYW");
        assert_eq!(lost, logged);
        assert!(
            lost_points <= logged_points,
            "{lost_points} > {logged_points}"
        );
        // Sent right with your exchange, then checked with a partial: the
        // copy is the call sent with your exchange.
        let mut checked = runtime(&["K5XYZ"]);
        checked.finish_turn();
        checked.key(OpKey::Esm, "K5XYZ", "");
        checked.finish_turn();
        checked.key(OpKey::Esm, "K5X?Z", "");
        checked.finish_turn();
        checked.run_to(600_000);
        let (result, run) = result(&checked);
        assert_eq!(run.qsos[0].k, "lost");
        assert_eq!(result.groups[0].received, "K5XYZ");
    }

    /// Enter, Enter, then Esc: the station heard the first message at its
    /// key and has your exchange, so Esc takes back only the `?`, and Enter
    /// sends TU once its number is copied.
    #[test]
    fn esc_takes_back_nothing_heard_at_an_earlier_key() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        runtime.now += 60;
        runtime.key(OpKey::Esm, "K5XYZ", "");
        assert_eq!(yours(runtime.last_turn()), "K5XYZ 5NN TT1 ?");
        runtime.now += 60;
        runtime.key(OpKey::StopTx, "K5XYZ", "");
        let state = view(&runtime).qso;
        assert_eq!(
            (state.call_sent.as_str(), state.exchange_sent),
            ("K5XYZ", true)
        );
        // The station sends its number on its timeout.
        let nr = serial(replies(&next_turn(&mut runtime))[0]);
        runtime.finish_turn();
        let effects = runtime.key(OpKey::Esm, "K5XYZ", &nr);
        assert_eq!(yours(played(&effects).expect("TU goes out")), "TU");
        runtime.finish_turn();
        assert_eq!(view(&runtime).clean, 1);
    }

    /// A CQ that brings again a call already reaching you is a `?` by
    /// another key: the call costs its send.
    #[test]
    fn a_cq_that_brings_a_call_again_costs_its_send() {
        let run = |key: OpKey| {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(key, "", "");
            assert_eq!(replies(runtime.last_turn()), ["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let exchange = replies(runtime.last_turn())[0].to_string();
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            sends(&runtime)
        };
        assert_eq!(run(OpKey::Again), [2, 1]);
        assert_eq!(run(OpKey::Esm), run(OpKey::Again), "Enter's CQ");
        assert_eq!(run(OpKey::Cq), run(OpKey::Again), "F1");
    }

    /// The station you work leaving after its silent timeouts is said, and
    /// Enter starts afresh rather than asking or thanking nobody.
    #[test]
    fn the_station_you_work_leaving_is_said_and_enter_starts_afresh() {
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let nr = serial(replies(runtime.last_turn())[0]);
        runtime.finish_turn();
        let from = runtime.effects.len();
        runtime.run_to(runtime.now + 40_000);
        assert_eq!(runtime.machine.phase(), ContestPhase::Running);
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
        assert!(
            runtime.effects[from..].contains(&ContestEffect::Announce("K5XYZ has gone".into()))
        );
        // Nobody calls now: Enter's CQ is for the next station.
        let afresh = QsoState {
            nobody_calling: true,
            ..QsoState::default()
        };
        assert_eq!(view(&runtime).qso, afresh);
        let fields = Fields {
            call: "K5XYZ".into(),
            exch: nr,
            ..Fields::default()
        };
        let enter = esm(&view(&runtime).qso, &fields, &RULES[0], Side::Dx);
        assert_ne!(enter.label, "TU & log");
        assert_ne!(enter.label, "? Again");
    }

    /// A `?` asked with the call already typed as it is sent with your
    /// exchange costs it nothing, as a `?` asked with the number already
    /// typed costs the number nothing.
    #[test]
    fn asking_again_for_a_call_already_typed_costs_it_nothing() {
        let fields = |call_edited: u64, exch: &str, done: Option<u64>| Fields {
            call: "K5XYZ".into(),
            exch: exch.into(),
            call_edited: MonoMs(call_edited),
            exch_complete: done.map(MonoMs),
        };
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.now += 1_500;
        let typed = runtime.now;
        for key in [OpKey::Again, OpKey::Esm] {
            let fields = fields(typed, "", None);
            runtime.apply(ContestEvent::Key { key, fields });
            runtime.finish_turn();
        }
        let nr = serial(replies(&runtime.turns[runtime.turns.len() - 1])[0]);
        runtime.now += 1_500;
        let done = runtime.now;
        for key in [OpKey::Again, OpKey::Esm] {
            let fields = fields(typed, &nr, Some(done));
            runtime.apply(ContestEvent::Key { key, fields });
            runtime.finish_turn();
        }
        runtime.apply(ContestEvent::FinishNow);
        assert_eq!(sends(&runtime), [1, 1]);
        assert_eq!(view(&runtime).log[0].asked, 2, "both asked for");
    }

    /// A bare `?` typed in the Call field is the `?` F7 sends: answered,
    /// but nobody's call, so nobody is worked and nothing is kept.
    #[test]
    fn a_bare_question_in_the_call_field_is_a_question() {
        let run = |typed: bool| {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            if typed {
                runtime.key(OpKey::Esm, "?", "");
            } else {
                runtime.key(OpKey::Again, "", "");
            }
            assert_eq!(yours(runtime.last_turn()), "?");
            assert_eq!(replies(runtime.last_turn()), ["K5XYZ"]);
            runtime.finish_turn();
            runtime.now += 2_000;
            let effects = runtime.apply(ContestEvent::FinishNow);
            let result = runtime.machine.result(1_000, "2026-10-20".into());
            (effects.last().cloned(), result.groups.len())
        };
        assert_eq!(run(false), (Some(ContestEffect::ShowNothingKept), 0));
        assert_eq!(run(true), run(false));
    }

    /// The next call keyed after a TU without logging (F3), then stopped
    /// with Esc before it went out, puts that QSO back as it was: logged
    /// now, it keeps its station's copy, and a run of that one clean QSO is
    /// kept.
    #[test]
    fn esc_of_the_next_call_after_a_tu_without_logging_keeps_that_qsos_copy() {
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        let exchange = to_its_exchange(&mut runtime);
        let nr = serial(&exchange);
        runtime.key(OpKey::Tu, "K5XYZ", &nr);
        runtime.finish_turn();
        // The serial has not moved on: the QSO F3 closed was never logged.
        runtime.key(OpKey::Esm, "G4ABC", "");
        assert_eq!(yours(runtime.last_turn()), "G4ABC 5NN TT1");
        runtime.now += 100;
        runtime.key(OpKey::StopTx, "G4ABC", "");
        assert_eq!(view(&runtime).qso.thanked, "K5XYZ", "Esc put F3's QSO back");
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        assert_eq!(
            view(&runtime).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );
        let effects = runtime.apply(ContestEvent::FinishNow);
        assert_eq!(effects.last(), Some(&ContestEffect::PersistAndShowResults));
        let (result, run) = result(&runtime);
        let copies: Vec<(&str, &str)> = result
            .groups
            .iter()
            .map(|group| (group.sent.as_str(), group.received.as_str()))
            .collect();
        assert_eq!(copies, [("K5XYZ", "K5XYZ"), (nr.as_str(), nr.as_str())]);
        assert_eq!(run.qsos[0].sc, "");
    }

    /// Esc of the next call after a TU without logging, then that station
    /// worked and logged instead: the QSO F3 closed is lost, as it is
    /// without the Esc.
    #[test]
    fn esc_of_the_next_call_after_a_tu_without_logging_still_loses_that_qso_unlogged() {
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        let exchange = to_its_exchange(&mut runtime);
        runtime.key(OpKey::Tu, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "G4ABC", "");
        runtime.now += 100;
        runtime.key(OpKey::StopTx, "G4ABC", "");
        runtime.key(OpKey::Esm, "G4ABC", "");
        let theirs = replies(runtime.last_turn())[0].to_string();
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "G4ABC", &serial(&theirs));
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        let kinds: Vec<(&str, &str)> = run
            .qsos
            .iter()
            .map(|qso| (qso.k.as_str(), qso.c.as_str()))
            .collect();
        assert_eq!(kinds, [("lost", "K"), ("", "G")]);
        assert_eq!(result.groups[0].sent, "K5XYZ");
        for (station, copies) in runtime.machine.copies_by_station() {
            assert_eq!(copies, 1, "station {station}");
        }
    }

    /// A correction the station sends in answer to a `?` still puts right
    /// the call you sent: it is announced, and Enter reads "Fix call", not
    /// "? Again", which would only bring the correction again.
    #[test]
    fn a_correction_drawn_by_a_question_reads_fix_call() {
        let enter = |runtime: &Runtime<ScriptCalls>, call: &str| {
            let fields = Fields {
                call: call.into(),
                ..Fields::default()
            };
            esm(&view(runtime).qso, &fields, &RULES[0], Side::Dx).label
        };
        // Enter, then Enter again before the correction has sounded.
        let mut twice = runtime(&["K5XYZ"]);
        twice.finish_turn();
        twice.key(OpKey::Esm, "K5XYW", "");
        twice.now += 60;
        twice.key(OpKey::Esm, "K5XYW", "");
        assert_eq!(yours(twice.last_turn()), "K5XYW 5NN TT1 ?");
        let correction = replies(twice.last_turn())[0].to_string();
        assert!(
            correction == "DE K5XYZ" || correction == "K5XYZ K5XYZ",
            "{correction}"
        );
        let from = twice.effects.len();
        twice.finish_turn();
        assert!(view(&twice).qso.corrected, "the correction went unnoticed");
        assert!(twice.effects[from..].contains(&ContestEffect::Announce(
            "They corrected you: fix the call, then Enter".into()
        )));
        assert_eq!(enter(&twice, "K5XYW"), "Fix call", "Enter, Enter");
        // Corrected, then the correction misheard as another close call.
        let mut misheard = runtime(&["K5XYZ"]);
        misheard.finish_turn();
        misheard.key(OpKey::Esm, "K5XYW", "");
        misheard.finish_turn();
        assert_eq!(enter(&misheard, "K5XYW"), "Fix call");
        misheard.key(OpKey::Esm, "K5XZZ", "");
        assert_eq!(yours(misheard.last_turn()), "K5XZZ ?");
        let back = replies(misheard.last_turn())[0].to_string();
        assert!(back == "DE K5XYZ" || back == "K5XYZ K5XYZ", "{back}");
        misheard.finish_turn();
        assert_eq!(enter(&misheard, "K5XZZ"), "Fix call", "misheard");
        // The correction costs the call no send.
        misheard.key(OpKey::Esm, "K5XYZ", "");
        let exchange = replies(misheard.last_turn())[0].to_string();
        misheard.finish_turn();
        misheard.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        misheard.finish_turn();
        misheard.apply(ContestEvent::FinishNow);
        assert_eq!(sends(&misheard), [1, 1]);
    }

    /// B4 or NIL sends the station you work away: Enter starts afresh, as
    /// after any station you work leaves, rather than thanking nobody.
    #[test]
    fn b4_or_nil_sending_the_station_away_starts_enter_afresh() {
        // Nobody calls then: Enter's CQ is for the next station.
        let afresh = QsoState {
            nobody_calling: true,
            ..QsoState::default()
        };
        for key in [OpKey::B4, OpKey::Nil] {
            let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
            let exchange = to_its_exchange(&mut runtime);
            runtime.key(key, "K5XYZ", "");
            runtime.finish_turn();
            assert_eq!(
                runtime.machine.stations[0].caller.state,
                CallerState::Gone,
                "{key:?}"
            );
            assert_eq!(view(&runtime).qso, afresh, "{key:?}");
            let fields = Fields {
                call: "K5XYZ".into(),
                exch: serial(&exchange),
                ..Fields::default()
            };
            let enter = esm(&view(&runtime).qso, &fields, &RULES[0], Side::Dx);
            assert_ne!(enter.label, "TU & log", "{key:?}");
        }
        // A station that answered your partial, still calling: the NIL that
        // spends its last try sends it away just the same.
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5?", "");
        runtime.finish_turn();
        assert_eq!(view(&runtime).qso.call_sent, "K5?");
        // Its patience is fourteen tries at most.
        for _ in 0..14 {
            if runtime.machine.stations[0].caller.state == CallerState::Gone {
                break;
            }
            runtime.key(OpKey::Nil, "K5?", "");
            runtime.finish_turn();
        }
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
        assert_eq!(view(&runtime).qso, afresh, "a partial");
    }

    /// A QSO given up once both exchanges are out owns a missed group for
    /// each field beside its call group, as one logged before its exchange
    /// came: walking away costs what logging without a copy costs.
    #[test]
    fn a_lost_qso_owns_a_missed_group_for_each_field() {
        for heard in [true, false] {
            let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let exchange = serial(replies(runtime.last_turn())[0]);
            if heard {
                runtime.finish_turn();
            }
            // CQ: the QSO is given up, once its exchange was heard or the
            // moment your exchange went out.
            runtime.key(OpKey::Cq, "K5XYZ", "");
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, run) = result(&runtime);
            assert_eq!((run.qsos[0].k.as_str(), run.qsos[0].g), ("lost", 2));
            let copies: Vec<(&str, &str)> = result
                .groups
                .iter()
                .map(|group| (group.sent.as_str(), group.received.as_str()))
                .collect();
            assert_eq!(copies, [("K5XYZ", "K5XYZ"), (exchange.as_str(), "")]);
            assert_eq!(result.group_timings[1].sends, 1);
            assert!(result.accuracy < 1.0, "heard: {heard}");
        }
    }

    /// A NIL logged on a caller's call before working it charges that
    /// station its call and a miss for each field; the QSO logged with it
    /// afterwards owns no groups, as the station's copy is kept already.
    #[test]
    fn a_nil_before_working_the_station_costs_its_fields_too() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::LogOnly, "K5XYZ", "1");
        let exchange = to_its_exchange(&mut runtime);
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        let kinds: Vec<(&str, u32)> = run.qsos.iter().map(|q| (q.k.as_str(), q.g)).collect();
        assert_eq!(kinds, [("nil", 2), ("", 0)]);
        assert_eq!(result.groups[1].received, "", "a miss");
        assert_eq!(result.groups[1].sent, serial(&exchange));
    }

    /// A NIL charged to the station you work once its exchange has reached
    /// you keeps each field as logged: its number heard and typed right is
    /// a copy, not a miss, as in a QSO logged with its call.
    #[test]
    fn a_nil_after_the_exchange_was_heard_copies_its_fields_as_logged() {
        let mut runtime = runtime(&["K5XYZ"]);
        let nr = serial(&to_its_exchange(&mut runtime));
        runtime.key(OpKey::LogOnly, "W9ZZZ", &nr);
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!((run.qsos[0].k.as_str(), run.qsos[0].g), ("nil", 2));
        let copies: Vec<(&str, &str)> = result
            .groups
            .iter()
            .map(|group| (group.sent.as_str(), group.received.as_str()))
            .collect();
        assert_eq!(copies, [("K5XYZ", "W9ZZZ"), (nr.as_str(), nr.as_str())]);
    }

    /// A pile-up of between `fewest` and `most` callers, sending `calls` in
    /// turn, through a filter wide open, so that the filter is no limit.
    fn pileup(calls: &[&str], fewest: u32, most: u32) -> Runtime<ScriptCalls> {
        pileup_seed(1, calls, fewest, most)
    }

    /// The same, its callers drawn from `seed`.
    fn pileup_seed(seed: u64, calls: &[&str], fewest: u32, most: u32) -> Runtime<ScriptCalls> {
        let mut settings = run_settings(25.0);
        settings.contest.contest_style = RunStyle::Pileup;
        settings.band.stations_min = fewest;
        settings.band.stations_max = most;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        Runtime::start(seed, settings.clamp(), ScriptCalls::new(calls))
    }

    /// Each segment station `station` sends in `turn`, in order.
    fn sent_by(turn: &Turn, station: u32) -> Vec<&Segment> {
        turn.segments
            .iter()
            .filter(|segment| segment.who == Sender::Caller(station))
            .collect()
    }

    /// Run time in milliseconds from seconds into a turn.
    fn ms(seconds: f64) -> u64 {
        (seconds * 1_000.0) as u64
    }

    /// What station `station` sends in `turn`, if anything.
    fn said_by(turn: &Turn, station: u32) -> Option<&str> {
        turn.segments
            .iter()
            .find(|segment| segment.who == Sender::Caller(station))
            .map(|segment| segment.text.as_str())
    }

    /// Work `call`, station `station`, with Enter: its call and your
    /// exchange, again for as long as it misses your exchange, then its
    /// number and TU, logged. Its number.
    fn work_with_enter(runtime: &mut Runtime<ScriptCalls>, call: &str, station: u32) -> String {
        let nr = its_number(runtime, call, station);
        runtime.key(OpKey::Esm, call, &nr);
        runtime.finish_turn();
        nr
    }

    /// Send `call`, station `station`, its call and your exchange with
    /// Enter, again for as long as it misses your exchange, until it sends
    /// its number. Its number, not logged yet.
    fn its_number(runtime: &mut Runtime<ScriptCalls>, call: &str, station: u32) -> String {
        runtime.finish_turn();
        for _ in 0..20 {
            runtime.key(OpKey::Esm, call, "");
            let reply = said_by(runtime.last_turn(), station).map(str::to_string);
            runtime.finish_turn();
            if let Some(Copied::Exchange(nr)) = reply.as_deref().and_then(copy_of) {
                return nr;
            }
        }
        panic!("{call} never sent its exchange");
    }

    /// After each CQ or TU, callers join until as many are calling as a
    /// fresh draw from the stations range; those still calling call again,
    /// and nobody joins while as many are calling as were drawn. Each sits
    /// at a pitch of its own, sixty hertz or more from the next.
    #[test]
    fn a_pile_up_calls_as_many_as_the_stations_range_draws() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        let cq = runtime.last_turn().clone();
        let calls: Vec<&str> = replies(&cq);
        assert_eq!(calls.len(), 3, "{calls:?}");
        for call in ["K5XYZ", "W7ABC", "G4ABC"] {
            assert!(
                calls
                    .iter()
                    .any(|said| said.split(' ').next() == Some(call)),
                "{calls:?}"
            );
        }
        let mut pitches: Vec<f64> = cq
            .segments
            .iter()
            .filter(|segment| segment.who != Sender::You)
            .map(|segment| segment.voice.tone_hz)
            .collect();
        pitches.sort_by(f64::total_cmp);
        assert!(
            pitches.windows(2).all(|w| w[1] - w[0] >= 60.0 - 1e-9),
            "{pitches:?}"
        );
        runtime.finish_turn();
        runtime.key(OpKey::Cq, "", "");
        assert_eq!(
            replies(runtime.last_turn()).len(),
            3,
            "the same three again"
        );
        assert_eq!(runtime.machine.stations.len(), 3, "and nobody new");
        // Drawn afresh each time from a range, the count moves about in it.
        let mut runtime = pileup(&[], 1, 5);
        let mut counts = std::collections::BTreeSet::new();
        for _ in 0..40 {
            runtime.finish_turn();
            counts.insert(replies(runtime.last_turn()).len());
            runtime.key(OpKey::Wipe, "", "");
            runtime.key(OpKey::Cq, "", "");
        }
        assert!(
            counts.iter().all(|count| (1..=5).contains(count)),
            "{counts:?}"
        );
        assert!(counts.len() >= 3, "{counts:?}");
    }

    /// No more call at once than the filter fits; the rest wait.
    #[test]
    fn a_narrow_filter_fits_fewer_callers() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_style = RunStyle::Pileup;
        settings.band.stations_min = 5;
        settings.band.stations_max = 5;
        settings.band.side_tone_min = 500.0;
        settings.band.side_tone_max = 500.0;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MIN;
        let settings = settings.clamp();
        let fits = filter_capacity(&settings);
        assert!(fits < 5, "{fits}");
        let runtime = Runtime::start(1, settings, ScriptCalls::new(&[]));
        assert_eq!(replies(runtime.last_turn()).len(), fits);
    }

    /// A partial goes only to the callers whose call it covers the largest
    /// share of; the others take it for someone else's.
    #[test]
    fn a_partial_is_answered_by_the_best_match_only() {
        let mut runtime = pileup(&["K5XYZ", "K5AB"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5?", "");
        let asked = runtime.last_turn().clone();
        assert_eq!(said_by(&asked, 0), None, "K5XYZ: two of five");
        assert!(
            said_by(&asked, 1).is_some_and(|said| said.contains("K5AB")),
            "K5AB: two of four"
        );
        assert_eq!(runtime.machine.working, Some(1));
    }

    /// A pile-up caller still sending when you key never hears what you
    /// key: it goes on from where it was, heard over you, and having heard
    /// only your message's tail, done before you are, calls again a moment
    /// after you. A caller that had finished hears you. A CQ, a TU or a NIL
    /// is heard from a fragment: the caller goes on, heard over you, and
    /// answers once it is done.
    #[test]
    fn a_caller_still_sending_when_you_key_never_hears_you() {
        // Station 0 finished first; key its call while station 1 sends.
        let keyed_over = |key: OpKey| {
            let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
            let cq = runtime.last_turn().clone();
            let span = |station: u32| {
                let segment = sent_by(&cq, station)[0];
                (segment.start_sec, segment.end_sec)
            };
            let ((_, first_end), (second_start, second_end)) = (span(0), span(1));
            let (first, second, ends) = if first_end <= second_end {
                (0, 1, (first_end.max(second_start), second_end))
            } else {
                (1, 0, (second_end.max(span(0).0), first_end))
            };
            let call = ["K5XYZ", "W7ABC"][first as usize];
            let tries = runtime.machine.stations[second as usize].clone();
            runtime.run_to(runtime.now + ms((ends.0 + ends.1) / 2.0));
            runtime.key(key, call, "");
            (runtime, first, second, tries, cq)
        };
        let (runtime, first, second, _, cq) = keyed_over(OpKey::HisCallAndExchange);
        let turn = runtime.last_turn().clone();
        let sends = sent_by(&turn, second);
        let over = sends[0];
        assert_eq!(over.start_sec, 0.0, "from the key, over you");
        let whole = said_by(&cq, second).expect("it called");
        assert!(
            whole.ends_with(&over.text) && over.text.len() < whole.len(),
            "{}",
            over.text
        );
        let station = &runtime.machine.stations[second as usize];
        assert_eq!(
            station.caller.state,
            CallerState::NeedQso,
            "it heard nothing"
        );
        assert!(!station.heard.engaged && station.heard.call_sent.is_empty());
        assert!(!station.caller.waits(), "nor you work anyone else");
        let your_end = turn.segments[0].end_sec;
        if over.end_sec < your_end {
            let again = sends.get(1).expect("it calls again after you");
            assert!(again.start_sec >= your_end + 0.1 - 1e-9, "after you");
            assert!(again.start_sec <= your_end + 0.6 + 1e-9, "a moment after");
            assert_eq!(again.text, whole, "its call, whole");
        } else {
            assert_eq!(sends.len(), 1, "still sending as you end");
        }
        assert!(
            said_by(&turn, first).is_some(),
            "the station you called answers"
        );
        // A CQ is heard from a fragment: it goes on, and calls after it.
        let (runtime, _, second, _, cq) = keyed_over(OpKey::Cq);
        let turn = runtime.last_turn();
        let sends = sent_by(turn, second);
        assert_eq!(sends[0].start_sec, 0.0, "it goes on, over your CQ");
        assert!(said_by(&cq, second).is_some_and(|whole| whole.ends_with(&sends[0].text)));
        let after = sends.get(1).expect("it calls");
        assert!(after.start_sec > turn.segments[0].end_sec, "after your CQ");
        assert!(after.start_sec > sends[0].end_sec, "and after its own");
        // So is a NIL: it spends a try on it, and calls again.
        let (runtime, _, second, before, _) = keyed_over(OpKey::Nil);
        let station = &runtime.machine.stations[second as usize];
        assert_ne!(station.caller, before.caller, "it heard the NIL");
        assert_eq!(sent_by(runtime.last_turn(), second).len(), 2);
    }

    /// Keyed over again and again, a caller still sending goes on through
    /// each continuation, never heard of again from its start.
    #[test]
    fn a_collision_carries_through_continuations() {
        let mut runtime = pileup(&["K5XYZ"], 1, 1);
        let cq = runtime.last_turn().clone();
        let (start, end) = (cq.segments[1].start_sec, cq.segments[1].end_sec);
        runtime.run_to(runtime.now + ((start + 0.02) * 1_000.0) as u64);
        let mut rest = cq.segments[1].text.clone();
        for _ in 0..3 {
            runtime.key(OpKey::Again, "", "");
            let turn = runtime.last_turn().clone();
            let over = turn
                .segments
                .iter()
                .find(|segment| segment.who == Sender::Caller(0))
                .expect("still calling");
            assert_eq!(over.start_sec, 0.0);
            assert!(rest.ends_with(&over.text), "{rest} then {}", over.text);
            rest = over.text.clone();
            runtime.now += 30;
        }
        // Each key fell while it was still sending.
        assert!(start + 0.02 + 3.0 * 0.03 < end);
        // It never heard a `?`: its tries are as they were.
        runtime.finish_turn();
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedQso
        );
    }

    /// A single caller you key over stops and hears you.
    #[test]
    fn a_single_caller_keyed_over_stops_and_hears_you() {
        let mut runtime = runtime(&["K5XYZ"]);
        let cq = runtime.last_turn().clone();
        let start = cq.segments[1].start_sec;
        runtime.run_to(runtime.now + ((start + 0.05) * 1_000.0) as u64);
        runtime.key(OpKey::Again, "", "");
        let turn = runtime.last_turn();
        let call = turn
            .segments
            .iter()
            .find(|segment| segment.who == Sender::Caller(0))
            .expect("it answers");
        assert!(call.start_sec > turn.segments[0].end_sec, "after your `?`");
        assert_eq!(call.text, "K5XYZ", "whole");
    }

    /// A pile-up caller that answered every CQ before you worked it is
    /// charged nothing for them: calling anyway is no repeat you asked for.
    #[test]
    fn a_pile_up_caller_worked_on_its_third_cq_keeps_one_send() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        for _ in 0..2 {
            runtime.finish_turn();
            runtime.key(OpKey::Cq, "", "");
            assert!(said_by(runtime.last_turn(), 0).is_some(), "it calls again");
        }
        work_with_enter(&mut runtime, "K5XYZ", 0);
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        assert_eq!(result.groups[0].sent, "K5XYZ");
        assert_eq!(result.group_timings[0].sends, 1);
        assert_eq!(view(&runtime).log[0].asked, 0);
    }

    /// Now and then a pile-up caller misses your exchange and asks for it:
    /// it was not taken, so Enter sends the call and exchange again, and
    /// the QSO goes on.
    #[test]
    fn a_missed_exchange_is_sent_again_by_enter() {
        let mut missed = 0;
        for seed in 0..60 {
            let mut settings = run_settings(25.0);
            settings.contest.contest_style = RunStyle::Pileup;
            let mut runtime = Runtime::start(seed, settings, ScriptCalls::new(&["K5XYZ"]));
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let reply = said_by(runtime.last_turn(), 0)
                .expect("a reply")
                .to_string();
            runtime.finish_turn();
            if reply == "NR?" || reply == "AGN" {
                missed += 1;
                assert!(!view(&runtime).qso.exchange_sent, "seed {seed}");
                runtime.key(OpKey::Esm, "K5XYZ", "");
                assert_eq!(yours(runtime.last_turn()), "K5XYZ 5NN TT1", "seed {seed}");
            } else {
                assert!(view(&runtime).qso.exchange_sent, "seed {seed}");
            }
        }
        assert!((1..=20).contains(&missed), "{missed} of 60");
    }

    /// In a pile-up, every third QSO's TU signs with your call.
    #[test]
    fn a_pile_up_tu_signs_with_your_call_every_third_qso() {
        let mut runtime = pileup(&[], 1, 1);
        let mut tus = Vec::new();
        for n in 0..6u32 {
            let call = format!("W{}AW", n + 1);
            work_with_enter(&mut runtime, &call, n);
            tus.push(
                runtime
                    .turns
                    .iter()
                    .rev()
                    .map(yours)
                    .find(|text| text.starts_with("TU"))
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        assert_eq!(tus, ["TU", "TU", "TU DL1ABC", "TU", "TU", "TU DL1ABC"]);
        // A single caller's never does.
        let mut single = self::runtime(&["K5XYZ", "W7ABC", "G4ABC"]);
        for (n, call) in ["K5XYZ", "W7ABC", "G4ABC"].into_iter().enumerate() {
            work_with_enter(&mut single, call, n as u32);
        }
        assert!(single.turns.iter().all(|turn| yours(turn) != "TU DL1ABC"));
    }

    /// Each caller fades on its own path, from its own point in the band's
    /// fading; your message never fades, and with the Band card's QSB off
    /// nobody does.
    #[test]
    fn each_caller_fades_on_its_own_path() {
        let runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        let cq = runtime.last_turn();
        assert_eq!(cq.segments[0].fade, None, "your CQ");
        let fades: Vec<Fade> = cq.segments[1..]
            .iter()
            .map(|segment| segment.fade.expect("fading is on"))
            .collect();
        assert_eq!(fades.len(), 2);
        assert_ne!(fades[0].from_sec, fades[1].from_sec);
        let mut settings = run_settings(25.0);
        settings.band.qsb_enabled = false;
        let quiet = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
        assert!(quiet.last_turn().segments.iter().all(|s| s.fade.is_none()));
    }

    /// Your call alone, F4, brings the callers back as a CQ does: a single
    /// caller's call brought so is a repeat you asked for, as a CQ's is, and
    /// a pile-up caller's is not.
    #[test]
    fn your_call_alone_brings_the_callers_back() {
        let mut single = self::runtime(&["K5XYZ"]);
        single.finish_turn();
        single.key(OpKey::MyCall, "", "");
        assert_eq!(yours(single.last_turn()), "DL1ABC");
        assert_eq!(replies(single.last_turn()), ["K5XYZ"]);
        work_with_enter(&mut single, "K5XYZ", 0);
        single.apply(ContestEvent::FinishNow);
        assert_eq!(sends(&single)[0], 2);
        let mut pileup = pileup(&["K5XYZ"], 1, 1);
        pileup.finish_turn();
        pileup.key(OpKey::MyCall, "", "");
        assert!(said_by(pileup.last_turn(), 0).is_some());
        work_with_enter(&mut pileup, "K5XYZ", 0);
        pileup.apply(ContestEvent::FinishNow);
        assert_eq!(sends(&pileup)[0], 1);
    }

    /// NIL to a caller still calling costs it a try, and it calls again,
    /// as after any QSO that ends.
    #[test]
    fn a_nil_to_a_caller_still_calling_costs_it_a_try() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let tries = runtime.machine.stations[0].caller.clone();
        runtime.key(OpKey::Nil, "", "");
        assert_eq!(yours(runtime.last_turn()), "NIL");
        assert_eq!(replies(runtime.last_turn()), ["K5XYZ"]);
        assert_ne!(runtime.machine.stations[0].caller, tries);
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedQso
        );
    }

    /// A station worked before calls again: typed, its call shows when it
    /// was logged, B4 sends it its call and `QSO B4` and it leaves, with no
    /// copy lost; worked instead, it sends the serial after its last, and
    /// its QSO is a dupe: clean, no error, scoring nothing and owning no
    /// groups.
    #[test]
    fn a_dupe_shows_b4_and_scores_nothing() {
        let dupe = || {
            let mut runtime = pileup(&["K5XYZ", "W7ABC"], 1, 1);
            let first = work_with_enter(&mut runtime, "K5XYZ", 0);
            let logged_at = view(&runtime).log[0].at_ms;
            assert_eq!(view(&runtime).worked_before("k5xyz"), Some(logged_at));
            assert_eq!(view(&runtime).worked_before("W7ABC"), None);
            let mut before = Vec::new();
            let run_ms = view(&runtime).run_ms(MonoMs(runtime.now));
            assert_eq!(runtime.machine.rejoin(0, &mut before, &[], run_ms), Some(0));
            assert_eq!(before.len(), 1, "kept as it was, for Esc");
            runtime.finish_turn();
            runtime.key(OpKey::Cq, "", "");
            assert_eq!(
                said_by(runtime.last_turn(), 0),
                Some("K5XYZ"),
                "it calls again"
            );
            runtime.finish_turn();
            (runtime, first)
        };
        let (mut runtime, _) = dupe();
        runtime.key(OpKey::B4, "K5XYZ", "");
        assert_eq!(yours(runtime.last_turn()), "K5XYZ QSO B4");
        runtime.finish_turn();
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
        assert_eq!(view(&runtime).qso.call_sent, "", "Enter starts afresh");
        runtime.apply(ContestEvent::FinishNow);
        let (_, run) = result(&runtime);
        assert!(run.qsos.iter().all(|qso| qso.k != "lost"), "{:?}", run.qsos);

        let (mut runtime, first) = dupe();
        let second = work_with_enter(&mut runtime, "K5XYZ", 0);
        let (first, second): (u32, u32) = (first.parse().unwrap(), second.parse().unwrap());
        assert!(second > first, "{first} then {second}");
        let row = &view(&runtime).log[1];
        assert_eq!(
            row.status,
            RowStatus::Checked(QsoCheck {
                errors: Vec::new(),
                dupe: true
            })
        );
        assert_eq!(view(&runtime).clean, 2, "a dupe copied right is clean");
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!((run.qsos[1].g, run.qsos[1].sc.as_str()), (0, "K5XYZ"));
        let score = run.score(&result.groups).expect("WPX scores");
        assert_eq!((score.raw.qso_points, score.verified.qso_points), (1, 1));
        assert_eq!(runtime.machine.copies_by_station().get(&0), Some(&1));
    }

    /// The run keeps its style, and in a pile-up the stations range it used.
    #[test]
    fn a_run_keeps_its_style_and_its_callers() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 2, 3);
        work_with_enter(&mut runtime, "K5XYZ", 0);
        runtime.apply(ContestEvent::FinishNow);
        let (_, run) = result(&runtime);
        assert_eq!((run.style.as_str(), run.callers), ("pileup", [2, 3]));
        let mut single = self::runtime(&["K5XYZ"]);
        work_with_enter(&mut single, "K5XYZ", 0);
        single.apply(ContestEvent::FinishNow);
        let (_, run) = result(&single);
        assert_eq!((run.style.as_str(), run.callers), ("single", [1, 1]));
    }

    /// B4 and NIL send away only the station they are for. A B4 with
    /// another station's call sends that one away, and the station you
    /// started with, which heard you go to someone else, calls again; no
    /// copy of it is lost. A NIL goes to the station you work last.
    #[test]
    fn b4_and_nil_send_away_only_the_station_they_are_for() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedNr
        );
        runtime.key(OpKey::B4, "W7ABC", "");
        assert_eq!(yours(runtime.last_turn()), "W7ABC QSO B4");
        assert!(said_by(runtime.last_turn(), 0).is_some(), "it calls again");
        runtime.finish_turn();
        let states: Vec<CallerState> = runtime
            .machine
            .stations
            .iter()
            .map(|station| station.caller.state)
            .collect();
        assert_eq!(states, [CallerState::NeedQso, CallerState::Gone]);
        assert!(
            runtime
                .machine
                .entries
                .iter()
                .all(|entry| entry.station != Some(0)),
            "no copy of K5XYZ lost"
        );

        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "W7ABC", "");
        runtime.finish_turn();
        assert_eq!(runtime.machine.working, Some(1));
        runtime.key(OpKey::Nil, "W7ABC", "");
        runtime.finish_turn();
        let states: Vec<CallerState> = runtime
            .machine
            .stations
            .iter()
            .map(|station| station.caller.state)
            .collect();
        assert_eq!(states, [CallerState::NeedQso, CallerState::Gone]);
    }

    /// A partial while you work a station is for that station, though a
    /// caller still calling has a call it covers more of: the station you
    /// work sends its call, the caller keeps quiet, and you are working
    /// the same station still, to log it clean. Its own call from a caller
    /// beats a slip of the call you work.
    #[test]
    fn a_partial_while_you_work_a_station_is_for_it_in_a_pile_up() {
        let mut runtime = pileup(&["K5XYZ", "K5AB"], 2, 2);
        let nr = its_number(&mut runtime, "K5XYZ", 0);
        runtime.key(OpKey::Esm, "K5?", "");
        let asked = runtime.last_turn().clone();
        assert_eq!(yours(&asked), "K5?");
        assert!(said_by(&asked, 0).is_some_and(|said| said.contains("K5XYZ")));
        assert_eq!(said_by(&asked, 1), None, "K5AB keeps quiet");
        assert_eq!(runtime.machine.working, Some(0));
        runtime.finish_turn();
        runtime.key(OpKey::LogOnly, "K5XYZ", &nr);
        assert_eq!(
            view(&runtime).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );

        // K5XY is a caller's own call, and only a slip of K5XYZ's.
        let mut runtime = pileup(&["K5XYZ", "K5XY"], 2, 2);
        its_number(&mut runtime, "K5XYZ", 0);
        runtime.key(OpKey::HisCall, "K5XY", "");
        assert!(said_by(runtime.last_turn(), 1).is_some(), "K5XY answers");
        assert_eq!(said_by(runtime.last_turn(), 0), None);
        assert_eq!(runtime.machine.working, Some(1));
    }

    /// A caller carried on into a continuation is still sending when you
    /// key again, even before its rest has reached the ear: it goes on
    /// whole, never hearing you. A hidden page then charges a repeat it
    /// sent once, not twice.
    #[test]
    fn a_caller_carried_on_before_it_reaches_the_ear_goes_on_whole() {
        let mut runtime = pileup(&["K5XYZ"], 1, 1);
        let cq = runtime.last_turn().clone();
        let start = cq.segments[1].start_sec;
        runtime.run_to(runtime.now + ms(start + 0.02));
        runtime.key(OpKey::Again, "", "");
        let carried = runtime.last_turn().clone();
        let rest = said_by(&carried, 0).expect("it goes on").to_string();
        // Its sound reaches the ear with the player's lead and latency.
        runtime.apply(ContestEvent::TurnStarted {
            id: carried.id,
            heard_from: MonoMs(runtime.now + 120),
        });
        let heard = runtime.machine.stations[0].heard.clone();
        runtime.now += 50;
        runtime.key(OpKey::Again, "", "");
        let again = runtime.last_turn();
        let over = sent_by(again, 0)[0];
        assert_eq!((over.start_sec, over.text.as_str()), (0.0, rest.as_str()));
        assert_eq!(runtime.machine.stations[0].heard, heard, "it heard nothing");
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedQso
        );

        // A repeat you asked for, begun and carried on, is charged once
        // however a hidden page cuts it.
        let mut runtime = runtime_with(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Again, "", "");
        let asked = runtime.last_turn().clone();
        let start = sent_by(&asked, 0)[0].start_sec;
        runtime.run_to(runtime.now + ms(start + 0.05));
        runtime.key(OpKey::Again, "", "");
        let carried = runtime.last_turn().clone();
        assert_eq!(sent_by(&carried, 0)[0].start_sec, 0.0);
        runtime.apply(ContestEvent::TurnStarted {
            id: carried.id,
            heard_from: MonoMs(runtime.now + 120),
        });
        runtime.now += 50;
        runtime.apply(ContestEvent::Pause {
            why: PauseWhy::Hidden,
            at: MonoMs(runtime.now),
        });
        runtime.now += 1_000;
        runtime.apply(ContestEvent::Resume);
        runtime.run_to(runtime.now + 1_000);
        runtime.finish_turn();
        assert_eq!(runtime.machine.stations[0].heard.call_asks.len(), 1);
    }

    /// A pile-up of one, so a caller's turns are its own.
    fn runtime_with(calls: &[&str]) -> Runtime<ScriptCalls> {
        pileup(calls, 1, 1)
    }

    /// Once five QSOs are logged, about one caller in fifty who joins is a
    /// station worked before, and never before: only one done with you and
    /// logged with a QSO, not one logged as a NIL or lost, nor one on the
    /// frequency at your key or closed by your TU.
    #[test]
    fn dupes_are_one_arrival_in_fifty_once_five_qsos_are_logged() {
        let mut runtime = pileup(&[], 1, 1);
        for n in 0..4 {
            work_with_enter(&mut runtime, &format!("W{}AW", n + 1), n);
        }
        // Each newcomer goes again at once, to keep the frequency clear.
        let joins = |runtime: &mut Runtime<ScriptCalls>, keyed: &[usize], times: usize| {
            let run_ms = view(runtime).run_ms(MonoMs(runtime.now));
            let mut dupes = Vec::new();
            for _ in 0..times {
                let machine = &mut runtime.machine;
                let count = machine.stations.len();
                let mut before: Vec<(usize, Station)> = keyed
                    .iter()
                    .map(|index| (*index, machine.stations[*index].clone()))
                    .collect();
                let joined = machine.join(&mut before, &[], run_ms).expect("room");
                if joined < count {
                    dupes.push(joined);
                    machine.stations[joined].caller.state = CallerState::Done;
                } else {
                    machine.stations.truncate(count);
                }
            }
            dupes
        };
        assert!(joins(&mut runtime, &[], 2_000).is_empty(), "four logged");
        work_with_enter(&mut runtime, "W5AW", 4);
        // Station 5 is lost: done with you, its exchange out, never logged.
        its_number(&mut runtime, "W6AW", 5);
        runtime.key(OpKey::Cq, "", "");
        // Station 6 is logged as a NIL before you work it, then done.
        runtime.finish_turn();
        runtime.key(OpKey::LogOnly, "W7AW", "1");
        its_number(&mut runtime, "W7AW", 6);
        runtime.key(OpKey::Cq, "", "");
        runtime.finish_turn();
        let kinds: Vec<(Option<usize>, &str)> = runtime
            .machine
            .entries
            .iter()
            .map(|entry| (entry.station, entry.record.k.as_str()))
            .collect();
        assert_eq!(&kinds[5..], [(Some(5), LOST), (Some(6), NIL)]);
        for index in [5, 6] {
            assert_eq!(
                runtime.machine.stations[index].caller.state,
                CallerState::Done
            );
        }
        // Station 1 is on the frequency at the key, station 2 closed by its
        // TU: neither comes back.
        let draws = 20_000;
        let dupes = joins(&mut runtime, &[1, 2], draws);
        assert!(
            dupes.iter().all(|index| [0, 3, 4].contains(index)),
            "{dupes:?}"
        );
        let share = dupes.len() as f64 / draws as f64;
        assert!((share - 0.02).abs() < 0.004, "{share:.4}");
        for index in [0, 3, 4] {
            assert!(dupes.contains(&index), "{index} came back too");
        }
    }

    /// Each caller waits for you on a clock of its own, from the end of its
    /// own send: on a quiet frequency every one calls again in turn, not
    /// only the quickest.
    #[test]
    fn on_a_quiet_frequency_every_caller_calls_on_its_own_timeout() {
        for seed in 0..20 {
            let mut runtime = pileup_seed(seed, &["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
            runtime.finish_turn();
            let played = runtime.turns.len();
            runtime.run_to(runtime.now + 20_000);
            let calls: Vec<usize> = (0..3)
                .map(|station| {
                    runtime.turns[played..]
                        .iter()
                        .filter(|turn| said_by(turn, station).is_some())
                        .count()
                })
                .collect();
            assert!(
                calls.iter().all(|count| *count > 0),
                "seed {seed}: {calls:?}"
            );
        }
    }

    /// A pile-up caller you key over, done before you are, heard only your
    /// message's tail: it calls again a moment after you, as on hearing
    /// garbage, and not on a timeout drawn after somebody else's.
    #[test]
    fn a_caller_keyed_over_calls_again_a_moment_after_you() {
        for seed in 0..20 {
            let mut runtime = pileup_seed(seed, &["K5XYZ"], 1, 1);
            let cq = runtime.last_turn().clone();
            let call = sent_by(&cq, 0)[0];
            let third = call.start_sec + (call.end_sec - call.start_sec) / 3.0;
            runtime.run_to(runtime.now + ms(third));
            runtime.key(OpKey::HisCallAndExchange, "K5XYZ", "");
            let turn = runtime.last_turn();
            let your_end = turn.segments[0].end_sec;
            let sends = sent_by(turn, 0);
            assert!(sends[0].end_sec < your_end, "seed {seed}: done first");
            let gap = sends.get(1).map(|again| again.start_sec - your_end);
            assert!(
                gap.is_some_and(|gap| (0.1 - 1e-9..=0.6 + 1e-9).contains(&gap)),
                "seed {seed}: {gap:?}"
            );
        }
    }

    /// A pile-up caller that heard you work someone else waits for that QSO
    /// to end: silent while you copy, with no timeout to call on, and deaf
    /// to a `?` meant for the station you work. Your TU brings it back.
    #[test]
    fn callers_wait_while_you_work_someone_else() {
        for seed in 0..20 {
            let mut runtime = pileup_seed(seed, &["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            runtime.finish_turn();
            let played = runtime.turns.len();
            runtime.run_to(runtime.now + 5_000);
            runtime.finish_turn();
            runtime.key(OpKey::Again, "K5XYZ", "");
            assert!(
                runtime.turns[played..]
                    .iter()
                    .all(|turn| said_by(turn, 1).is_none() && said_by(turn, 2).is_none()),
                "seed {seed}"
            );
            assert!(said_by(runtime.last_turn(), 0).is_some(), "seed {seed}");
            let nr = its_number(&mut runtime, "K5XYZ", 0);
            runtime.key(OpKey::Esm, "K5XYZ", &nr);
            let tu = runtime.last_turn();
            assert!(yours(tu).starts_with("TU"), "seed {seed}");
            assert!(
                said_by(tu, 1).is_some() && said_by(tu, 2).is_some(),
                "seed {seed}: they call after your TU"
            );
        }
    }

    /// A station you called and then left for another goes back to calling,
    /// as MorseRunner's does: it asks nothing over your next QSO, a `?` to
    /// the other is not for it, and the other's TU brings it back calling,
    /// not done.
    #[test]
    fn a_station_you_leave_for_another_calls_again() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        runtime.key(OpKey::HisCallAndExchange, "W7ABC", "");
        assert_eq!(said_by(runtime.last_turn(), 0), None);
        runtime.finish_turn();
        runtime.key(OpKey::Again, "W7ABC", "");
        assert_eq!(said_by(runtime.last_turn(), 0), None);
        work_with_enter(&mut runtime, "W7ABC", 1);
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedQso
        );
        let tu = runtime
            .turns
            .iter()
            .rev()
            .find(|turn| yours(turn).starts_with("TU"))
            .expect("a TU");
        assert!(said_by(tu, 0).is_some(), "it calls after the TU");
        let kinds: Vec<&str> = runtime
            .machine
            .entries
            .iter()
            .map(|entry| entry.record.k.as_str())
            .collect();
        assert_eq!(kinds, [""], "nothing lost yet");
        // One that had sent its exchange takes the QSO as complete.
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        its_number(&mut runtime, "K5XYZ", 0);
        runtime.key(OpKey::HisCall, "W7ABC", "");
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Done);
    }

    /// Callers that tie for your partial and are never worked keep no lost
    /// copy: in a pile-up only a station that heard its call from you, or
    /// took your exchange, is engaged.
    #[test]
    fn callers_that_only_answered_a_partial_keep_no_lost_copy() {
        let mut runtime = pileup(&["K5XYZ", "K5ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5?", "");
        assert_eq!(replies(runtime.last_turn()).len(), 2, "both answer");
        work_with_enter(&mut runtime, "K5ABC", 1);
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert!(run.qsos.iter().all(|qso| qso.k != "lost"), "{:?}", run.qsos);
        assert_eq!(result.groups[0].sent, "K5ABC");
    }

    /// In a pile-up a NIL whose call is like nobody's is charged to nobody,
    /// not to the newest caller: the call stays in the record, and a clean
    /// QSO logged later with that caller keeps its copy.
    #[test]
    fn a_nil_like_nobodys_call_in_a_pile_up_is_charged_to_nobody() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        runtime.finish_turn();
        runtime.key(OpKey::LogOnly, "JA1ZZZ", "123");
        let nr = work_with_enter(&mut runtime, "G4ABC", 2);
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(
            (
                run.qsos[0].k.as_str(),
                run.qsos[0].g,
                run.qsos[0].lc.as_str()
            ),
            ("nil", 0, "JA1ZZZ")
        );
        let copies: Vec<(&str, &str)> = result
            .groups
            .iter()
            .map(|group| (group.sent.as_str(), group.received.as_str()))
            .collect();
        assert_eq!(copies, [("G4ABC", "G4ABC"), (nr.as_str(), nr.as_str())]);
    }

    /// After B4 or NIL the rest of the pile-up calls again at once, as the
    /// QSO you were in is over.
    #[test]
    fn after_b4_or_nil_the_pile_up_calls_again() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        runtime.finish_turn();
        runtime.key(OpKey::B4, "K5XYZ", "");
        let b4 = runtime.last_turn();
        assert_eq!(yours(b4), "K5XYZ QSO B4");
        assert_eq!(said_by(b4, 0), None, "it leaves");
        assert!(said_by(b4, 1).is_some() && said_by(b4, 2).is_some());

        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        runtime.key(OpKey::Nil, "K5XYZ", "");
        let nil = runtime.last_turn();
        assert_eq!(yours(nil), "NIL");
        assert_eq!(said_by(nil, 0), None, "it leaves");
        assert!(said_by(nil, 1).is_some() && said_by(nil, 2).is_some());
    }

    /// A call keyed over its own station, still sending, is no near call's:
    /// the ranking counts every station on the frequency, and a near call
    /// that has finished keeps quiet.
    #[test]
    fn a_call_keyed_over_its_station_is_no_near_calls() {
        let mut tried = 0;
        for seed in 0..30 {
            let mut runtime = pileup_seed(seed, &["K5XYZ", "K5XYA"], 2, 2);
            let cq = runtime.last_turn().clone();
            let (own, near) = (sent_by(&cq, 0)[0], sent_by(&cq, 1)[0]);
            if near.end_sec >= own.end_sec {
                continue;
            }
            tried += 1;
            let at = near.end_sec.max(own.start_sec);
            runtime.run_to(runtime.now + ms((at + own.end_sec) / 2.0));
            runtime.key(OpKey::HisCallAndExchange, "K5XYZ", "");
            assert_eq!(said_by(runtime.last_turn(), 1), None, "seed {seed}");
        }
        assert!(tried >= 5, "{tried}");
    }

    /// In a pile-up your call alone brings new callers once a QSO is in the
    /// log, as a CQ does.
    #[test]
    fn your_call_alone_brings_new_callers_in_a_pile_up() {
        let mut runtime = pileup(&["K5XYZ"], 1, 1);
        work_with_enter(&mut runtime, "K5XYZ", 0);
        while runtime
            .machine
            .stations
            .iter()
            .any(|s| s.caller.state.active())
        {
            assert!(runtime.step());
        }
        runtime.finish_turn();
        let count = runtime.machine.stations.len();
        runtime.key(OpKey::MyCall, "", "");
        assert_eq!(yours(runtime.last_turn()), "DL1ABC");
        assert_eq!(replies(runtime.last_turn()).len(), 1);
        assert_eq!(runtime.machine.stations.len(), count + 1, "a newcomer");
    }
}
