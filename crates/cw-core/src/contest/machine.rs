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
//! MorseRunner's do ([`super::caller`]); one whose wait runs out while a
//! turn is on calls in it, planned with the turn, and on a quiet frequency
//! the machine's patience timer runs out and it calls again, in a turn of
//! its own.
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

use std::collections::BTreeSet;

use super::caller::{CallMatch, Caller, CallerState, Likeness, Says, exchange_at, match_call};
use super::check::{QsoCheck, QsoText, check_qso};
use super::clock::RunClock;
use super::data::places::home_country;
use super::data::zones::home_continent;
use super::esm::{
    EntryField, Fields, MessagePart, QsoState, call_copied, esm, loggable, message_text,
};
use super::exchange::Field;
use super::parse::{Reading, normalize_call, parse_exchange};
use super::place::{filter_capacity, place_contest_caller};
use super::qrm::Qrm;
use super::record::{ContestRun, LAYOUT, LOST, NIL, QsoRecord};
use super::rules::{ContestRules, Fill, Side};
use super::score::rate_at;
use super::turn::{
    Fade, Segment, Sender, Turn, char_spans, sending_sec, sounding_at, word_gap_sec, your_voice,
};
use super::{CallSource, CallerHabits, MS_PER_MINUTE, RunStyle};
use crate::alignment::CopyTally;
use crate::band::Qsb;
use crate::callsign::parse_callsign;
use crate::machine::COUNTDOWN_STEP_MS;
use crate::recognition::{CharTimes, Keys};
use crate::rng::{Rng, SplitMix64};
use crate::score::MIN_RESPONSE_MS_PER_CHAR;
use crate::session::{Answered, GroupResult, MonoMs, SessionResult, SessionTiming, whole_ms};
use crate::settings::{STATIONS_MAX, STATIONS_MIN, TrainingSettings, WPM_MAX, WPM_MIN};
use crate::speed::tells_speed;
use crate::timing::CharSpan;
use crate::timing::{SEND_RELEASE_SEC, StationVoice};

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
    /// fields as they are then: boxed, as they are the most any event
    /// carries.
    Key {
        key: OpKey,
        fields: Box<Fields>,
    },
    /// A turn's sound has started, on each attempt, retries too:
    /// `output_from` is when it goes to the audio output, `heard_from` when
    /// it reaches the ear, as far as the device says.
    TurnStarted {
        id: u64,
        heard_from: MonoMs,
        output_from: MonoMs,
    },
    TurnEnded {
        id: u64,
    },
    /// A turn's sound was held up on its way, its audio parked or its clock
    /// fallen behind, however it ended: it went out later than planned, and
    /// nothing heard in it can be timed from.
    TurnHeldUp {
        id: u64,
    },
    /// Some of a turn's sound went unheard, `from_sec` to `until_sec` into
    /// it: the browser fell so far behind its audio clock that it skipped
    /// it. Said as soon as it is found, before the turn ends, which is held
    /// up as well. A caller's send any of which it skipped is heard no more
    /// than one sent under your message is.
    TurnMissed {
        id: u64,
        from_sec: f64,
        until_sec: f64,
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
    /// − / +: your speed, in WPM, from your next message on. The callers'
    /// speeds are the Speed card's.
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

    /// How many callers are sending at `now`: a station on your frequency
    /// that is not calling you is none. What they send is never shown.
    pub fn calling(&self, now: MonoMs) -> usize {
        let at_sec = self.heard_sec(now);
        self.spans
            .iter()
            .filter(|(who, start, end)| who.is_caller() && *start <= at_sec && at_sec < *end)
            .count()
    }
}

/// With Show copy on, what a station on your frequency sent, and when it
/// shows on the screen: a second after its send has ended.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownText {
    pub at: MonoMs,
    /// What it sent, the whole of it, whatever of it a key of yours
    /// carried over into a turn of its own.
    pub text: String,
    /// It is a station on your frequency that is not calling you.
    pub qrm: bool,
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
    /// The station's speed, once its check is shown: the slower of what its
    /// call and its exchange are credited at. None for a QSO that owns no
    /// groups, a station logged again or one nobody was heard for.
    pub wpm: Option<f64>,
    /// How many repeats you asked for.
    pub asked: u32,
    pub status: RowStatus,
}

/// A run as the screen shows it. It holds nothing of what a station really
/// sent until that QSO's check is shown, but for what Show copy shows, each
/// with when it may.
#[derive(Clone, Debug, PartialEq)]
pub struct ContestView {
    pub phase: ContestPhase,
    /// The run's clock, once it has started: the run bar reads it itself.
    pub clock: Option<RunClock>,
    pub limit_ms: u64,
    /// The speed you send at: your speed as set, or the middle of the
    /// callers' range, as − / + have left it, a whole speed once moved.
    pub my_wpm: f64,
    /// What Enter reads: the call last sent, whether your exchange was
    /// taken, and whether the station corrected you.
    pub qso: QsoState,
    pub air: Option<AirView>,
    /// With Show copy on, what the stations sent in the turn on the air, or
    /// the turn last on it, each with when it shows; nothing with Show copy
    /// off. The screen shows each once its moment has come.
    pub shown: Vec<ShownText>,
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

/// With Show copy on, how long after a station's send ends its text shows:
/// room to type the character heard last, so that what you copied by ear is
/// in the fields when the text shows and your copy of it is kept (§9.1 of
/// the design).
const SHOWN_AFTER_MS: u64 = 1_000;

/// Once this many QSOs are logged, [`DUPE_SHARE`] of the callers who join
/// are stations worked before (§4.7 of the design).
const DUPES_AFTER: u32 = 5;
const DUPE_SHARE: f64 = 0.02;

/// The stream, forked off the run's generator, that the run's own draws
/// come from: how many call, and who calls again. The callers' streams are
/// forked by their numbers, which never reach it.
const RUN_STREAM: u64 = u64::MAX;

/// The stream, forked off the run's generator, that the run's QRM draws
/// from under rough habits, apart from every other, so the rest of a run
/// draws the same whatever its habits.
const QRM_STREAM: u64 = u64::MAX - 1;

/// The rate shows once the run is a minute old.
const RATE_AFTER_MS: u64 = MS_PER_MINUTE;

/// How many times the runtime tries a send, and how long past a turn's
/// length it waits for each attempt before it tries again (`player.rs:28`,
/// `wait.rs:18`).
const SEND_ATTEMPTS: u64 = 3;
const ATTEMPT_GRACE_MS: u64 = 2_000;

/// The longest lead a splice waits out, in milliseconds: past any lead and
/// latency a player believes (half a second of latency at most, and a
/// fraction of one of lead), so a start held up longer, as by a device
/// opening, is not taken for the player's lead and does not hold up every
/// continuation after it.
const LEAD_MOST_MS: u64 = 1_000;

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
    /// The sends of its call, and of its exchange, that began to reach you,
    /// those on its timeouts too.
    call_sends: Sends,
    exch_sends: Sends,
    /// Its call's first sending, and each field's token in its exchange's,
    /// heard once, whole and alone: what their characters are timed from.
    call_sounded: Option<Sounded>,
    exch_sounded: Vec<(Field, Sounded)>,
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
    first_call: Option<(Typed, u64)>,
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
    typed: Typed,
    /// The call last sent that it took for its own or answered, without
    /// `?`: what a message of yours with no call in it goes with.
    call_sent: String,
    /// The slowest a send of its call, and of its exchange, began to reach
    /// you at: a caller that slowed down for you (QRS, §4.3 of the design)
    /// credits each copy at the speed it may have come from, and one sent
    /// only at its own speed at that.
    call_voice: Option<StationVoice>,
    exch_voice: Option<StationVoice>,
    /// With Show copy on, when its call's text, and its exchange's, first
    /// showed on the screen.
    call_shown: Option<Shown>,
    exch_shown: Option<Shown>,
    /// What you had of its call, and of its exchange, when their text first
    /// showed, read at your next key ([`ContestMachine::freeze_shown`]):
    /// the copy its groups keep, later edits counting only for the log.
    /// Each with how many of the repeats asked of it came before.
    call_frozen: Option<(String, usize)>,
    exch_frozen: Option<(Reading, usize)>,
}

/// When a station's text shows on the screen with Show copy on: the moment,
/// and the run time then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shown {
    at: MonoMs,
    run: u64,
}

/// How many sends of a station's call, or of its exchange, began to reach
/// you, and from when on a key says nothing of how soon you recognised it,
/// in run time: its second sending began to go out, or the run paused.
/// Run time stands still while paused, so a key after a pause would leave
/// the pause out of its time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sends {
    count: u32,
    until: Option<u64>,
}

impl Sends {
    /// Another began to reach you, gone out at `out_run`.
    fn began(&mut self, out_run: u64) {
        self.count = self.count.saturating_add(1);
        if self.count == 2 {
            self.cut(out_run);
        }
    }

    /// The run paused at `run`, once any of it had reached you.
    fn paused(&mut self, run: u64) {
        if self.count > 0 {
            self.cut(run);
        }
    }

    fn cut(&mut self, at: u64) {
        self.until = Some(self.until.map_or(at, |until| until.min(at)));
    }
}

/// A token's first sending, heard once, whole and alone, as it went to the
/// output: what its copy's characters are timed from (docs/recognition.md).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sounded {
    /// The turn it went out in, which may yet turn out late or retried.
    turn: u64,
    /// When the send it is in began to go out, in run time.
    from: u64,
    /// Where each of its characters ended, in milliseconds after `from`.
    ends: Vec<u32>,
    /// The delay the device reports from its output to the ear.
    latency_ms: u32,
}

impl Sounded {
    /// Its copy `copy`'s times, its keys saying nothing from `until` on
    /// ([`Sends`]), if the rule could read any. The fields take keys all
    /// through a run.
    fn timed(&self, copy: &Typed, until: Option<u64>) -> Option<Timed> {
        let times = CharTimes::of(
            self.from,
            &self.ends,
            &copy.keys,
            until,
            self.latency_ms,
            true,
        );
        times.can_be_timed().then_some(Timed {
            turn: self.turn,
            times,
        })
    }

    /// When the last of it reached you, in run time.
    fn heard_by(&self) -> u64 {
        let last = self.ends.last().copied().unwrap_or(0);
        self.from + u64::from(last) + u64::from(self.latency_ms)
    }
}

/// A copy as a key read it: its text, and its keys, in run time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Typed {
    text: String,
    keys: Keys<u64>,
}

impl Typed {
    /// This, as a copy of `text`: with its keys only if it is that text.
    fn as_copy(&self, text: &str) -> Self {
        Self {
            text: text.to_string(),
            keys: if self.text == text {
                self.keys.clone()
            } else {
                Keys::default()
            },
        }
    }
}

/// The Call field as a key read it, as copied, and when it was last
/// edited, in run time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CallRead {
    typed: Typed,
    edited_run: u64,
}

/// A group's recognition record, and the turn its sound went out in.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Timed {
    turn: u64,
    times: CharTimes,
}

/// A group a QSO keeps, and the turn its timing comes from, if it is timed:
/// a turn found late or retried takes that back ([`Kept::stored`]).
#[derive(Clone, Debug, PartialEq)]
struct Kept {
    group: GroupResult,
    timing: SessionTiming,
    heard_in: Option<u64>,
}

impl Kept {
    /// As the run keeps it: untimed where its sound's turn is `untrusted`.
    fn stored(&self, untrusted: &[u64]) -> (GroupResult, SessionTiming) {
        let mut timing = self.timing.clone();
        if self.heard_in.is_some_and(|turn| untrusted.contains(&turn)) {
            timing.char_times = None;
        }
        (self.group.clone(), timing)
    }
}

impl Heard {
    fn note_typed(&mut self, typed: &Typed) {
        if !typed.text.is_empty() {
            self.typed.clone_from(typed);
        }
    }

    /// The repeats of its call among `asks` you asked for while the Call
    /// field read otherwise than `copy`: one asked with the call already
    /// typed as it was copied changes nothing, as for a field.
    fn call_repeats(asks: &[String], copy: &str) -> u32 {
        let asks = asks.iter().filter(|then| *then != copy).count();
        u32::try_from(asks).unwrap_or(u32::MAX)
    }

    /// Its call group, given the copy kept without Show copy: what you had
    /// when its text showed, if it showed before you copied it, with the
    /// showing a send of its own and only the repeats asked before it, and
    /// untimed; else the copy as kept, timed as `ms`, and its characters
    /// from its first sending, if that was heard so.
    fn call_group(&self, sent: &str, kept: &Typed, ms: u64, voice: &StationVoice) -> Kept {
        let (copy, ms, asks, shown) = match &self.call_frozen {
            Some((had, asked)) => (
                Typed {
                    text: had.clone(),
                    keys: Keys::default(),
                },
                0,
                &self.call_asks[..(*asked).min(self.call_asks.len())],
                1,
            ),
            None => (kept.clone(), ms, &self.call_asks[..], 0),
        };
        let sends = 1 + shown + Self::call_repeats(asks, &copy.text);
        let timed = self
            .call_sounded
            .as_ref()
            .and_then(|sounded| sounded.timed(&copy, self.call_sends.until));
        group(sent, &copy.text, ms, sends, voice, timed)
    }

    /// A send of `says` began to reach you in `voice`: of a call put right
    /// with an exchange after it, the call, its exchange noted apart once
    /// that begins too.
    fn note_voice(&mut self, says: Says, voice: StationVoice) {
        let slowest = |kept: &mut Option<StationVoice>| {
            if kept.is_none_or(|kept| slower(voice, kept)) {
                *kept = Some(voice);
            }
        };
        match says {
            Says::Call | Says::Correction | Says::CorrectionAndExchange => {
                slowest(&mut self.call_voice);
            }
            Says::Exchange => slowest(&mut self.exch_voice),
            Says::Query | Says::Signoff => {}
        }
    }

    /// What its call, and its exchange, are credited at: the slowest a send
    /// of each began to reach you at, or `own` where none did.
    fn voices(&self, own: StationVoice) -> (StationVoice, StationVoice) {
        (
            self.call_voice.unwrap_or(own),
            self.exch_voice.unwrap_or(own),
        )
    }

    /// What reached you of it, as `now` has it, kept over this, the
    /// station as it was before a message Esc took back: Esc takes your
    /// message back from the callers, not what they sent from you. What it
    /// took from you, and whether it has its groups, stay as they were.
    fn keep_heard(&mut self, now: &Self) {
        self.first = now.first;
        self.call_begun = now.call_begun;
        self.call_ends.clone_from(&now.call_ends);
        self.call_sends = now.call_sends;
        self.exch_sends = now.exch_sends;
        self.call_sounded.clone_from(&now.call_sounded);
        self.exch_sounded.clone_from(&now.exch_sounded);
        self.call_asks.clone_from(&now.call_asks);
        self.exch_begun = now.exch_begun;
        self.exch_ends.clone_from(&now.exch_ends);
        self.exch_asks.clone_from(&now.exch_asks);
        self.call_voice = now.call_voice;
        self.exch_voice = now.exch_voice;
        self.call_shown = now.call_shown;
        self.exch_shown = now.exch_shown;
    }

    /// Forget what was heard of it after run time `run`, where the run's
    /// clock holds: a turn whose sound failed gives the time since it first
    /// started back as paused, and nothing heard may lie ahead of the
    /// clock, or a copy would be timed from a moment yet to come.
    fn forget_after(&mut self, run: u64) {
        self.first = self.first.filter(|first| *first <= run);
        self.call_ends.retain(|end| *end <= run);
        self.exch_ends.retain(|end| *end <= run);
        self.call_sounded = self
            .call_sounded
            .take()
            .filter(|sounded| sounded.heard_by() <= run);
        self.exch_sounded
            .retain(|(_, sounded)| sounded.heard_by() <= run);
    }

    /// The repeats of its call and of its exchange you asked for and heard.
    fn asked(&self) -> u32 {
        u32::try_from(self.call_asks.len() + self.exch_asks.len()).unwrap_or(u32::MAX)
    }

    /// A group for each field of `sent`, as `read` at the key that logs:
    /// as keyed, once its exchange has begun to reach you, and a miss
    /// before that, as a guess is no copy. With Show copy, an exchange
    /// whose text showed before you logged it keeps the fields as they read
    /// then, the showing a send of its own, and only the repeats asked
    /// before it. `rules` say what values are the same. A field typed as
    /// it was sent is timed from its token's first sending, if that was
    /// heard so.
    fn field_groups(
        &self,
        rules: &ContestRules,
        sent: &[(Field, String)],
        read: &LogRead,
        voice: &StationVoice,
    ) -> Vec<Kept> {
        if !self.exch_begun {
            return misses(sent, voice);
        }
        let (reading, asks, shown) = match &self.exch_frozen {
            Some((had, asked)) => (
                had,
                &self.exch_asks[..(*asked).min(self.exch_asks.len())],
                1,
            ),
            None => (&read.reading, &self.exch_asks[..], 0),
        };
        // The exchange is timed from the end of the last one heard that the
        // clock runs from, before the field first read complete, shared
        // among the fields by length; one kept from when its text showed
        // was done by then.
        let exch_ms = if shown > 0 {
            0
        } else {
            copy_ms(&self.exch_ends, read.done_run)
        };
        let length = sent
            .iter()
            .map(|(_, token)| token.chars().count())
            .sum::<usize>()
            .max(1) as u64;
        sent.iter()
            .map(|(field, token)| {
                let typed = reading
                    .fields
                    .iter()
                    .find(|read| read.field == *field)
                    .map_or("", |read| read.token.as_str());
                let value = reading.value(*field);
                // A repeat counts for a field empty, or read differently,
                // after it.
                let asks = asks
                    .iter()
                    .filter(|asked| asked.value(*field).is_none_or(|then| Some(then) != value))
                    .count();
                let share = exch_ms * token.chars().count() as u64 / length;
                let copy = rules.keyed_token(*field, token, typed);
                // Its keys are its characters' only as typed, and only as
                // read at the key that logs.
                let keyed = read
                    .exch
                    .iter()
                    .find(|(read, _)| read == field)
                    .map(|(_, keyed)| keyed.as_copy(&copy))
                    .filter(|_| shown == 0 && copy == typed);
                let timed = keyed
                    .zip(self.sounded(*field))
                    .and_then(|(keyed, sounded)| sounded.timed(&keyed, self.exch_sends.until));
                group(
                    token,
                    &copy,
                    share,
                    1 + shown + u32::try_from(asks).unwrap_or(u32::MAX),
                    voice,
                    timed,
                )
            })
            .collect()
    }

    /// Its exchange's first sending of `field`'s token, if it was heard
    /// once, whole and alone.
    fn sounded(&self, field: Field) -> Option<&Sounded> {
        self.exch_sounded
            .iter()
            .find(|(sounded, _)| *sounded == field)
            .map(|(_, sounded)| sounded)
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
    /// The next moment the turn on the air reaches you that Enter and the
    /// screen are to hear of ([`ContestMachine::arm_heard`]).
    heard: Option<u64>,
}

/// The fields as the key that logs read them, in run time: what the QSO's
/// groups are copied from.
#[derive(Clone, Debug, PartialEq)]
struct LogRead {
    /// The QSO as logged.
    logged: QsoText,
    reading: Reading,
    /// Each field read, as keyed.
    exch: Vec<(Field, Typed)>,
    call: CallRead,
    /// When the exchange first read complete, or else when it was logged.
    done_run: u64,
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
    /// It answers your `?` or `NR?`, which ask for what it owes: an
    /// exchange it sends with its call put right is a repeat you asked for
    /// only then.
    again: bool,
    /// It answers your exchange, which it took for the first time: the
    /// exchange it sends is owed, no repeat, though a careless operator
    /// sent it early with its call put right.
    first_exchange: bool,
    /// A key of yours drew it, with the fields as read then; none for a
    /// send on its timeout, or one sent again after Resume that had begun
    /// to reach you.
    keyed: Option<Keyed>,
    /// It calls on its timeout, its wait for you run out while the turn
    /// is on ([`ContestMachine::time_outs`]): what the timeout costs it is
    /// spent once the call begins to reach you, so one your key cuts before
    /// then costs nothing.
    on_time_out: bool,
}

impl Replying {
    /// A send of `station` that no key of yours drew.
    fn unasked(station: usize) -> Self {
        Self {
            station,
            asked: false,
            again: false,
            first_exchange: false,
            keyed: None,
            on_time_out: false,
        }
    }

    /// A call of `station` on its timeout, while the turn is on.
    fn on_time_out(station: usize) -> Self {
        Self {
            on_time_out: true,
            ..Self::unasked(station)
        }
    }
}

/// How much of a reply on the air has been counted: nothing yet, its start
/// (what it is charged, and that it began to reach you), all of it, or, for
/// one sent while the receiver was muted, its cost alone. A call put right
/// with the station's exchange after it has the exchange's start counted
/// apart, once that reaches you: what you log of the exchange is a copy
/// only then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Counted {
    Nothing,
    /// Its call put right has begun to reach you, and the exchange after
    /// it, from the byte `exchange_from` of its text on, not yet.
    CallBegun {
        exchange_from: usize,
    },
    Begun,
    Whole,
    /// It went out while you sent, the receiver muted (§5.3 of the design):
    /// nothing of it reached you, nor will. What it costs its caller is
    /// spent all the same, as it was sent.
    Muted,
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
    /// The fields it was keyed with, and the Call field as it read them.
    fields: Fields,
    call: CallRead,
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
    /// The moment it starts: when it was keyed, or, for a continuation,
    /// when it takes over from the turn before. The turn holds its run time.
    keyed_at: MonoMs,
    /// When its sound reached the ear, as last stamped.
    heard_from: MonoMs,
    /// When its sound went to the audio output, as last stamped: what its
    /// characters are timed from.
    output_from: MonoMs,
    /// When its first attempt started.
    first_start: Option<MonoMs>,
    /// Your message's parts, each with the byte in your text it ends before.
    parts: Vec<(MessagePart, usize)>,
    /// Where each character of your message stops sounding, for the screen
    /// to show what is still to go.
    my_chars: Vec<(usize, f64)>,
    /// Its callers' segments, in order, and what each says.
    replies: Vec<(Replying, Says)>,
    /// How many of `replies`, the first, go on from a turn keyed over:
    /// what they send began to go out there.
    carried: usize,
    /// What each of its segments sends, the whole of it, by its place among
    /// them: a rest carried on from an earlier turn is the rest of this,
    /// what Show copy shows once it ends.
    texts: Vec<String>,
    /// How much of each reply has been counted: a key that logs without
    /// sending counts what has reached you so far.
    counted: Vec<Counted>,
    /// The place among its segments of QRM carried on from a turn taken off
    /// the air, some of which went out under your message there, the
    /// receiver muted: no send heard, though it sounds here, as a caller's
    /// so is ([`Counted::Muted`]).
    qrm_muted: Option<usize>,
    /// Log rows whose check shows once their TU is out
    /// ([`Self::checks_at_sec`]), or once it is off the air.
    pending: Vec<usize>,
    /// When the rows waiting on it show their checks, in seconds into the
    /// turn: once your TU has gone out, and the sign-off it brought.
    checks_at_sec: f64,
    unheard: Option<Unheard>,
    /// Each caller's wait for you once it is over, as the turn planned
    /// it with the timeouts in it ([`ContestMachine::time_outs`]), in
    /// milliseconds from its start.
    waits: Vec<(usize, u64)>,
}

impl Air {
    /// Run time when its sound reached the ear. No pause can come between
    /// its key and its start without taking it off the air.
    fn heard_run(&self) -> u64 {
        self.turn.at_run_ms + self.heard_from.since(self.keyed_at)
    }

    /// The delay the device reports from its output to the ear.
    fn latency_ms(&self) -> u32 {
        u32::try_from(self.heard_from.since(self.output_from)).unwrap_or(u32::MAX)
    }

    /// Run time when its sound went to the output: what its characters are
    /// timed from.
    fn output_run(&self) -> u64 {
        self.heard_run()
            .saturating_sub(u64::from(self.latency_ms()))
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
            .filter(|segment| segment.who.is_caller());
        callers
            .zip(self.replies.iter().zip(&self.counted))
            .filter_map(|(segment, ((reply, _), counted))| {
                let start = heard_run + whole_ms(segment.start_sec);
                let end = heard_run + whole_ms(segment.end_sec);
                if start >= run_now && *counted == Counted::Nothing {
                    // A call on its timeout yet to come waits afresh, as
                    // everyone does after Resume.
                    (!reply.on_time_out).then(|| reply.clone())
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
            .filter(|segment| segment.who.is_caller());
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
/// and its parts, the log rows that wait on it, the callers still sending,
/// and any QRM still sending, which never stops for you; and, for a
/// continuation, where the next turn takes over from it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Carried {
    text: String,
    parts: Vec<(MessagePart, usize)>,
    pending: Vec<usize>,
    callers: Vec<Overlapping>,
    qrm: Option<QrmRest>,
    splice: Option<Splice>,
}

/// What is left of the QRM still sending as a turn is taken off the air
/// ([`ContestMachine::qrm_going_on`]).
#[derive(Clone, Debug, PartialEq)]
struct QrmRest {
    /// The rest of it, from the character sounding then.
    rest: Segment,
    /// The whole of what it sends, which Show copy shows once it ends.
    whole: String,
    /// Some of it went out under your message, the receiver muted without
    /// full break-in: it is no send heard, though its rest sounds after an
    /// Esc, as a caller's so is ([`Counted::Muted`]).
    muted: bool,
}

/// Where a key splices its continuation onto the turn on the air, your
/// message in it still going out (§5.2 of the design): at the start of the
/// first of its characters that the player can still start the
/// continuation at, its lead after the key and the turn on the air ridden
/// down by then, so the turn on the air sends every character before that
/// to its end, the continuation sends the rest from there, and no character
/// goes out twice nor waits. With none of them left to start so, the turn
/// on the air finishes your message and the continuation takes over in the
/// word space after it, your next message starting a word space after the
/// last, as though the two were one message, or as soon after as the
/// player can.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Splice {
    /// Run time when the continuation takes over.
    run: u64,
    /// The byte of your message the rest goes on from; none where only its
    /// last character was left, finished by the turn on the air.
    from: Option<usize>,
    /// Where your message starts in the continuation, in seconds: what is
    /// left of the word space where the turn on the air finished your
    /// message, none where the rest goes on.
    lead_sec: f64,
}

/// A caller still sending when you key over it: it goes on from the
/// character sounding then, sent again whole, as your own message does, so
/// the two of you are heard over each other.
#[derive(Clone, Debug, PartialEq)]
struct Overlapping {
    reply: Replying,
    says: Says,
    /// How much of it has been counted, its start at least.
    counted: Counted,
    text: String,
    /// The whole of what it sends, of which `text` is the rest.
    whole: String,
    /// How far into `text` it goes on from ([`Segment::skip_sec`]).
    skip_sec: f64,
    /// How it was sending: a caller that slowed down for you since goes on
    /// at the speed its send began at.
    voice: StationVoice,
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
    groups: Vec<Kept>,
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
    /// Your state, province or territory, or DX, for a contest whose
    /// exchange sends it.
    my_state: String,
    /// Your CQ zone, for a contest whose exchange sends it; 0 where none
    /// is known.
    my_zone: u32,
    /// Your power, for a contest whose exchange sends it.
    my_power: String,
    /// Your Field Day class and your section, or DX, for a contest whose
    /// exchange sends them.
    my_class: String,
    my_section: String,
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
    /// The stations on your frequency that are not calling you, under
    /// rough habits; none under easier ones.
    qrm: Option<Qrm>,
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
    /// The station your last call or partial reached, by its place in
    /// `stations`; the station you work once it is in a QSO with you
    /// ([`Self::qso_station`]).
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
    /// The speed you send at, in WPM: your speed as set, moved by − / +.
    my_wpm: f64,
    /// The speed of each message you sent.
    sent_wpm: Vec<f64>,
    /// How long after a key its turn's sound reaches the ear, in
    /// milliseconds, as the last turn started at once found it: the
    /// player's lead and latency, and the moment it took to start. No
    /// continuation can take over from the turn on the air any sooner
    /// ([`Self::splice`]).
    lead_ms: u64,
    /// Run time and time paused, once the run is over.
    ended: Option<(u64, u64)>,
    /// With Show copy on, what the stations sent in the turn last on the
    /// air, kept once it ends naturally, till the next turn or a pause.
    shown_last: Vec<ShownText>,
    /// The turns whose sound nothing can be timed from: tried again, or
    /// played late.
    untrusted: Vec<u64>,
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
        let rules = settings.contest.contest_kind.rules();
        let my_call = settings.contest.my_call.clone();
        let my_name = settings.contest.my_name.clone();
        let my_state = settings.contest.state_sent().to_string();
        let my_zone = settings.contest.zone().unwrap_or(0);
        let my_power = settings.contest.my_power.clone();
        let my_class = settings.contest.my_class.clone();
        let my_section = settings.contest.section_sent().to_string();
        let countdown = settings.playback.countdown_sec;
        let my_wpm = settings.my_wpm();
        let pileup = settings.contest.contest_style == RunStyle::Pileup;
        let rng = SplitMix64::new(seed);
        let side = Side::of_call(&my_call);
        let qrm = (settings.contest.contest_habits == CallerHabits::Rough)
            .then(|| Qrm::new(&settings, rules, side, &my_call, rng.fork(QRM_STREAM)));
        let mut machine = Self {
            side,
            qrm,
            limit_ms: u64::from(settings.contest.limit_minutes()) * MS_PER_MINUTE,
            style: settings.contest.contest_style,
            qsb: Qsb::of(&settings.band),
            settings,
            rules,
            my_call,
            my_name,
            my_state,
            my_zone,
            my_power,
            my_class,
            my_section,
            source,
            draws: rng.fork(RUN_STREAM),
            rng,
            phase: ContestPhase::Countdown { left: countdown },
            clock: None,
            pauses: Vec::new(),
            next_id: 1,
            timers: Timers::default(),
            air: None,
            qso: QsoState {
                pileup,
                ..QsoState::default()
            },
            working: None,
            stations: Vec::new(),
            cut_short: Vec::new(),
            thanked: None,
            entries: Vec::new(),
            taught: 0,
            serial: 1,
            my_wpm,
            sent_wpm: Vec::new(),
            lead_ms: 0,
            ended: None,
            shown_last: Vec::new(),
            untrusted: Vec::new(),
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

    /// Enter's QSO is over: nothing sent, taken or corrected, in a run of
    /// the same style. A call whose station has gone stays one.
    fn end_qso(&mut self) {
        self.qso = QsoState {
            pileup: self.qso.pileup,
            gone: std::mem::take(&mut self.qso.gone),
            heard_gone: std::mem::take(&mut self.qso.heard_gone),
            ..QsoState::default()
        };
    }

    /// Enter's QSO is over, its station `index` gone: every call the Call
    /// field could still hold for it is one Enter no longer sends, as nobody
    /// would answer it ([`QsoState::gone`]). Its own call is among them once
    /// it has reached you, as you may have put the field right since
    /// without a key: it has left, and its copy is closed, so that tells
    /// nothing a score could still use. The name the station goes by: its
    /// own call once it put yours right, as you heard it say; else the call
    /// you had for it, while that is whole and no partial; else "The
    /// station".
    fn station_gone(&mut self, index: usize) -> String {
        let partial = self.qso.call_sent.contains('?');
        let corrected = self.qso.corrected;
        self.end_qso();
        let station = &self.stations[index];
        let heard = &station.heard;
        let own = &station.caller.call.text;
        let mut gone = vec![heard.typed.text.clone(), heard.call_sent.clone()];
        if heard.call_begun {
            gone.push(own.clone());
        }
        gone.retain(|call| !call.is_empty());
        gone.sort();
        gone.dedup();
        let name = if corrected {
            own.clone()
        } else if parse_callsign(&heard.typed.text).is_some() && !partial {
            heard.typed.text.clone()
        } else {
            "The station".to_string()
        };
        self.qso.gone = gone;
        name
    }

    /// Note for Enter whether anyone is calling you or working with you. In
    /// a pile-up a caller waiting for a QSO to end never calls by itself, so
    /// once the station you work has gone, nobody calls until you CQ. With
    /// it, the calls of the stations that have gone that nobody would
    /// answer ([`Self::note_heard_gone`]).
    fn note_callers(&mut self) {
        self.qso.nobody_calling = !self
            .stations
            .iter()
            .any(|s| s.caller.state.active() && !s.caller.waits());
        self.note_heard_gone();
    }

    /// Note for Enter the call of every station that has gone once its call
    /// had reached you, a caller that gave up before you worked it among
    /// them, that no station still on the frequency would take for its own
    /// ([`QsoState::heard_gone`]): nobody would answer it. A station that
    /// calls with it again, a new caller drawn with that call, takes it.
    /// And whether the call last sent, a partial or a slip among them, fits
    /// one of those stations and none on the frequency
    /// ([`QsoState::sent_fits_gone`]).
    fn note_heard_gone(&mut self) {
        let taken = |call: &str| {
            self.stations
                .iter()
                .filter(|station| station.caller.state.active())
                .any(|station| station.caller.likeness(call).is_some())
        };
        let gone: Vec<&Caller> = self
            .stations
            .iter()
            .filter(|station| station.caller.state == CallerState::Gone && station.heard.call_begun)
            .map(|station| &station.caller)
            .collect();
        let mut heard_gone: Vec<String> = gone
            .iter()
            .map(|caller| caller.call.text.clone())
            .filter(|call| !taken(call))
            .collect();
        heard_gone.sort();
        heard_gone.dedup();
        // A call with no character but `?` in it is nobody's.
        let sent = &self.qso.call_sent;
        let sent_fits_gone = sent.chars().any(|c| c != '?')
            && !taken(sent)
            && gone.iter().any(|caller| caller.likeness(sent).is_some());
        self.qso.heard_gone = heard_gone;
        self.qso.sent_fits_gone = sent_fits_gone;
    }

    fn handle(&mut self, event: ContestEvent, now: MonoMs) -> Vec<ContestEffect> {
        match event {
            // A start says only when the sound reached the ear; checking the
            // limit on it would end a run on a retry's stamp.
            ContestEvent::TurnStarted {
                id,
                heard_from,
                output_from,
            } => {
                if let Some(air) = self.air.as_mut().filter(|air| air.turn.id == id) {
                    // A turn started at once, on its first try, says how
                    // far behind the key the player's sound is. One tried
                    // again went out twice, or in part: nothing in it can be
                    // timed.
                    let retried = air.first_start.is_some();
                    if !retried && air.turn.starts_at.is_none() {
                        let lead = heard_from.0.saturating_sub(air.keyed_at.0);
                        self.lead_ms = lead.min(LEAD_MOST_MS);
                    }
                    air.heard_from = heard_from;
                    air.output_from = output_from;
                    air.first_start.get_or_insert(heard_from);
                    if retried {
                        self.untrust(id);
                    }
                }
                return Vec::new();
            }
            ContestEvent::Abort => return self.abort(),
            // Both are credited as paused before the limit is looked at, so
            // time that was not run time cannot end the run.
            ContestEvent::TurnFailed { id } if self.on_air(id) => return self.sound_failed(now),
            ContestEvent::Pause { why, at } => return self.pause(why, at, now),
            // On the air or not, its QSOs logged or not: the run's groups
            // are read once it is kept.
            ContestEvent::TurnHeldUp { id } => {
                self.untrust(id);
                return Vec::new();
            }
            ContestEvent::TurnMissed {
                id,
                from_sec,
                until_sec,
            } => {
                self.untrust(id);
                if self.on_air(id) {
                    self.missed(from_sec, until_sec);
                }
                return Vec::new();
            }
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
            my_wpm: self.my_wpm,
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
                spans: self.heard_spans(&air.turn),
            }),
            shown: match &self.air {
                Some(air) => self.shown_of(air),
                None => self.shown_last.clone(),
            },
            qsos: u32::try_from(log.len()).unwrap_or(u32::MAX),
            clean: u32::try_from(clean).unwrap_or(u32::MAX),
            serial: self.serial_sent(),
            log,
        }
    }

    /// When each station sends in `turn`, as far as you can hear it:
    /// without full break-in, from the end of your message on, the receiver
    /// muted till then, so RX counts nobody you cannot hear.
    fn heard_spans(&self, turn: &Turn) -> Vec<(Sender, f64, f64)> {
        let open_from = turn
            .muted_until(self.settings.contest.contest_qsk)
            .unwrap_or(0.0);
        turn.segments
            .iter()
            .map(|segment| {
                let start = if segment.who == Sender::You {
                    segment.start_sec
                } else {
                    segment.start_sec.max(open_from)
                };
                (segment.who, start, segment.end_sec)
            })
            .filter(|(_, start, end)| start < end)
            .collect()
    }

    /// The finished run, as it is stored: `finished_at` is wall time, the
    /// moment it ended; `date` the local day it counts towards.
    pub fn result(&self, finished_at: u64, date: String) -> SessionResult {
        let (run_ms, paused_ms) = self.ended.unwrap_or_default();
        let my_wpm = if self.sent_wpm.is_empty() {
            self.my_wpm
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
            my_country: home_country(&self.my_call).unwrap_or_default().to_string(),
            my_continent: home_continent(&self.my_call)
                .unwrap_or_default()
                .to_string(),
            my_zone: self.my_zone,
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
            assisted: self.settings.contest.contest_show_copy,
            qsos: self
                .entries
                .iter()
                .map(|entry| entry.record.clone())
                .collect(),
        };
        let groups = self
            .entries
            .iter()
            .flat_map(|entry| &entry.groups)
            .map(|kept| kept.stored(&self.untrusted))
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

    /// Whether `who` is a careless operator waiting for its call put right
    /// and for your exchange, which puts the call right with its own
    /// exchange: what it sends as its state stands is an exchange sent
    /// early.
    #[cfg(test)]
    pub(crate) fn sends_exchange_early(&self, who: Sender) -> bool {
        let Sender::Caller(index) = who else {
            return false;
        };
        self.stations.get(index as usize).is_some_and(|station| {
            station.caller.sends_early() && station.caller.state == CallerState::NeedCallNr
        })
    }

    /// Each copy lost or charged as a NIL, with its station, as the call it
    /// holds.
    #[cfg(test)]
    pub(crate) fn nil_and_lost_copies(&self) -> Vec<(usize, String)> {
        self.nil_and_lost()
            .map(|(station, received)| (station, received.to_string()))
            .collect()
    }

    /// Each copy lost or charged as a NIL, with its station, as the call it
    /// holds.
    #[cfg(test)]
    fn nil_and_lost(&self) -> impl Iterator<Item = (usize, &str)> {
        self.entries
            .iter()
            .filter(|entry| entry.record.k == LOST || entry.record.k == NIL)
            .filter_map(|entry| {
                Some((
                    entry.station?,
                    entry.groups.first()?.group.received.as_str(),
                ))
            })
    }

    /// In a pile-up, the copies lost or charged as a NIL since the run held
    /// `before` (as [`Self::nil_and_lost_copies`] had them) whose call
    /// another station took from you for its own, when this one never took
    /// it and it is not this one's own: the call you last sent that
    /// station, or first sent it with your exchange, or its own call once
    /// it had heard one from you. That would be the call typed for the next
    /// station, or logged for the one you left, kept as the copy of
    /// another. Looked at as each copy is lost or charged, so a station
    /// that joins, or takes that call, later is no witness against a
    /// reading made before. The soak finds none.
    #[cfg(test)]
    pub(crate) fn crossed_since(&self, before: &[(usize, String)]) -> Vec<(usize, String)> {
        if self.style != RunStyle::Pileup {
            return Vec::new();
        }
        let mut before: Vec<(usize, &str)> = before
            .iter()
            .map(|(station, received)| (*station, received.as_str()))
            .collect();
        let mut crossed = Vec::new();
        for (index, received) in self.nil_and_lost() {
            if let Some(at) = before.iter().position(|old| *old == (index, received)) {
                before.swap_remove(at);
                continue;
            }
            // Nothing copied, as Show copy keeps of a call you had none of
            // when it showed, is no call at all.
            if received.is_empty() {
                continue;
            }
            let took = |heard: &Heard| {
                heard.call_sent == received
                    || heard
                        .first_call
                        .as_ref()
                        .is_some_and(|(call, _)| call.text == received)
            };
            let theirs = self.stations.iter().enumerate().any(|(other, station)| {
                other != index
                    && (took(&station.heard)
                        || (station.heard.engaged && station.caller.call.text == received))
            });
            let station = &self.stations[index];
            if theirs && !took(&station.heard) && station.caller.call.text != received {
                crossed.push((index, received.to_string()));
            }
        }
        crossed
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

    /// Turn `id`'s sound went out otherwise than planned: nothing heard in
    /// it is timed.
    fn untrust(&mut self, id: u64) {
        if !self.untrusted.contains(&id) {
            self.untrusted.push(id);
        }
    }

    /// What of the turn on the air was skipped, `from_sec` to `until_sec`
    /// into it, never reached you: a caller's send any of which it was is
    /// not heard, nor will be, as one sent under your message is not
    /// ([`Counted::Muted`]), a call or an exchange heard in part being none
    /// heard. Nothing more of it counts, for Enter or a copy, Show copy
    /// never shows it, and its station sends it again, when asked, as one
    /// you never heard; what it costs its caller is spent all the same, as
    /// it was sent. The news comes at the player's first look after the
    /// stall, which the heard timer for anything in it, and the turn's end,
    /// come after; what a key pressed in that moment counted stands.
    fn missed(&mut self, from_sec: f64, until_sec: f64) {
        let Some(air) = self.air.as_mut() else {
            return;
        };
        let callers = air
            .turn
            .segments
            .iter()
            .filter(|segment| segment.who.is_caller());
        for ((segment, (reply, _)), counted) in callers.zip(&air.replies).zip(&mut air.counted) {
            if segment.end_sec <= from_sec || segment.start_sec >= until_sec {
                continue;
            }
            // A call on its timeout costs it the try once it is on its way,
            // heard or not, as counting it would have.
            if *counted == Counted::Nothing && reply.on_time_out {
                self.stations[reply.station].caller.wait_over();
            }
            *counted = Counted::Muted;
        }
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

    /// Move your speed by `delta` WPM, from the whole speed nearest it,
    /// within the Speed card's bounds. The callers' speeds stay the Speed
    /// card's: your speed is kept from run to run, and a shift of theirs
    /// would last only the run.
    fn shift_speed(&mut self, delta: i32) {
        self.my_wpm = (self.my_wpm.round() + f64::from(delta)).clamp(WPM_MIN, WPM_MAX);
    }

    /// The clock starts, the run's end is set, and your first CQ goes out.
    fn begin_run(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        self.clock = Some(RunClock::start(now));
        self.phase = ContestPhase::Running;
        let (id, end) = self.start_timer(self.limit_ms);
        self.timers.run_end = Some(id);
        let mut effects = vec![end];
        let call = CallRead::default();
        effects.extend(self.send(vec![MessagePart::Cq], &Fields::default(), &call, now, false));
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
        if timers.heard == Some(id) {
            self.timers.heard = None;
            if self.phase == ContestPhase::Running {
                return self.heard_so_far(now);
            }
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
        let mut effects = Vec::new();
        // What of the turn on the air has reached you counts first: Enter
        // decides on what you have heard, whether or not another caller is
        // still sounding.
        let run_ms = self.run_ms(now);
        if let Some(mut air) = self.air.take() {
            self.count_heard(&mut air, Some(run_ms), run_ms, &mut effects);
            self.air = Some(air);
        }
        if self.settings.contest.contest_show_copy {
            self.freeze_shown(fields, now);
        }
        // The Call field emptied starts the QSO's keying again, as
        // MorseRunner's `Edit1Change` resets C and N (`Main.pas:2063-2068`).
        let call = normalize_call(&fields.call);
        if call.is_empty() {
            self.forget_qso();
        }
        let action = self.action(key, fields);
        // Enter clears the fields for a station gone that was not with
        // Enter's QSO as it left: a caller that gave up before you worked
        // it, typed late, or the one a partial or a slip you sent to nobody
        // fits. Nothing may have said it leaving, so it is said now, in the
        // hint's words. The station Enter's QSO was with was said as it
        // left ([`QsoState::gone`]), unless your own B4, NIL or CQ sent it
        // away.
        if key == OpKey::Esm && action.wipe && !self.qso.gone.contains(&call.replace('?', "")) {
            let who = if self.qso.heard_gone.contains(&call) {
                call.as_str()
            } else {
                "The station"
            };
            effects.push(ContestEffect::Announce(format!("{who} has gone")));
        }
        // A key that sends nothing reads the Call field as a copy of the
        // station you work, a wipe before it clears it; one that sends has
        // it read once the callers have heard who you work ([`Self::went_on`]),
        // and one that logs reads it for the QSO it logs.
        let call = self.read_call(fields, now);
        if action.message.is_empty()
            && !action.logs
            && let Some(working) = self.working
        {
            self.note_typed(working, &call.typed);
        }
        if action.wipe {
            self.forget_qso();
            effects.push(ContestEffect::ClearFields);
        }
        if !action.message.is_empty() {
            effects.extend(self.send(action.message, fields, &call, now, action.logs));
        } else if action.logs {
            // Logged without sending, once what has reached you has
            // counted: the check shows at once.
            let row = self.log(fields, &call, run_ms, now);
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
    /// nothing corrected, and no call kept for the station it was with,
    /// gone ([`QsoState::gone`]), though the calls heard of stations gone
    /// stay ([`QsoState::heard_gone`]); and Esc does not bring back what it
    /// read.
    fn forget_qso(&mut self) {
        self.end_qso();
        self.qso.gone.clear();
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
                    wipe: enter.clears,
                }
            }
            OpKey::Cq => only(MessagePart::Cq),
            OpKey::Exchange => only(MessagePart::Exchange),
            OpKey::Tu => only(MessagePart::Tu),
            OpKey::MyCall => only(MessagePart::MyCall),
            // A dupe calling has not heard its call from you yet: B4 sends
            // the call typed first, as TU and log does, so it knows it is
            // meant. So it does when the call went out and the station you
            // work never took it: a dupe still calling as it went out never
            // heard it, and would take the B4 alone for the rest of the
            // pile-up's cue to call again.
            OpKey::B4 => {
                let taken = self
                    .qso_station()
                    .is_some_and(|index| self.stations[index].heard.call_sent == call);
                let mut message = Vec::new();
                if whole && (call != self.qso.call_sent || !taken) {
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
    /// the key. A caller still sending, single or in a pile-up, goes on
    /// over you and never hears it, but for a CQ, a
    /// TU or a NIL, which it hears from a fragment once it stops before
    /// your message ends ([`Self::deaf`]).
    fn send(
        &mut self,
        message: Vec<MessagePart>,
        fields: &Fields,
        call: &CallRead,
        now: MonoMs,
        logs: bool,
    ) -> Vec<ContestEffect> {
        let run_ms = self.run_ms(now);
        // Your exchange carries this QSO's serial, taken before a TU in the
        // same message logs it, and so does the number of the QSO its TU
        // closes.
        let serial = self.serial_sent();
        let number = self.serial;
        let mut effects = Vec::new();
        // A caller you talk over, single or in a pile-up, goes on with what
        // it is sending and hears your message only as `deaf` says, as
        // MorseRunner's callers do (`DxStn.pas:240-253`).
        let carried = match self.air.take() {
            Some(air) => self.cut(air, run_ms, true, &mut effects),
            None => Carried::default(),
        };
        // Where the message keyed now starts among the turn's parts, after
        // the rest of one keyed earlier.
        let from = carried.parts.len();
        let (mut text, mut parts) = (carried.text, carried.parts);
        // A TU that greets the station by name greets it as you copied it.
        let his_name = parse_exchange(self.rules, self.side, &fields.exch)
            .value(Field::Name)
            .unwrap_or_default()
            .to_string();
        let me = Fill {
            my_call: &self.my_call,
            my_name: &self.my_name,
            my_state: &self.my_state,
            my_zone: &self.my_zone.to_string(),
            my_power: &self.my_power,
            my_class: &self.my_class,
            my_section: &self.my_section,
            serial,
            his_name: &his_name,
            // A TU signs with your call as often as you choose, every QSO
            // unless you choose otherwise, where the contest's TU does not
            // sign by its own rules.
            tu_signed: self
                .rules
                .tu_adds_call(self.settings.contest.contest_tu_call, number),
        };
        for part in &message {
            let said = message_text(std::slice::from_ref(part), self.rules, self.side, &me);
            if said.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&said);
            parts.push((part.clone(), text.len()));
        }
        let lead_sec = carried.splice.map_or(0.0, |splice| splice.lead_sec);
        let your_end = self.your_end_sec(&text, lead_sec);
        let deaf = self.deaf(&message, &carried.callers, your_end);
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
            from,
            deaf: deaf.clone(),
            fields: fields.clone(),
            call: call.clone(),
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
        let (mut replying, took) = self.hear(&message, fields, call, &deaf, logs);
        self.went_on(worked, took, &call.typed, true);
        let garbled = self.keyed_over(&carried.callers, your_end, &deaf);
        // The station you worked, or one you called in this message, sent
        // away by what you sent (B4, NIL, a CQ before its exchange, or its
        // last try spent), ends the QSO as its leaving on its timeouts does.
        // One still calling, that had only answered your partial, left on
        // its last try, as on a timeout, and is said as it would be then; a
        // station in a QSO with you leaves only on your B4, NIL or CQ, which
        // you know you sent.
        let gone = |index: &usize| self.stations[*index].caller.state == CallerState::Gone;
        if let Some(index) = [self.working, worked].into_iter().flatten().find(gone) {
            let calling = before
                .stations
                .iter()
                .any(|(at, station)| *at == index && station.caller.state == CallerState::NeedQso);
            let who = self.station_gone(index);
            if calling {
                effects.push(ContestEffect::Announce(format!("{who} has gone")));
            }
        }
        if !logs {
            self.thank(worked, &message, fields, true);
        }
        let mut pending = carried.pending;
        if logs {
            pending.push(self.log(fields, call, run_ms, now));
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
        let mine = Carried {
            text,
            parts,
            pending,
            callers: carried.callers,
            qrm: carried.qrm,
            splice: carried.splice,
        };
        let unheard = (!logs).then_some(before);
        effects.extend(self.play(mine, unheard, replying, garbled, run_ms, now));
        effects
    }

    /// Where your message `text` ends in a turn it starts `lead_sec` into,
    /// in seconds from the turn's start; at its start with none.
    fn your_end_sec(&self, text: &str, lead_sec: f64) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        let voice = your_voice(&self.settings, self.my_wpm);
        lead_sec + sending_sec(text, &self.settings, &voice)
    }

    /// Where the rest a caller you keyed over goes on with ends, in seconds
    /// from the start of the turn it goes on in.
    fn rest_end_sec(&self, over: &Overlapping) -> f64 {
        sending_sec(&over.text, &self.settings, &over.voice) - over.skip_sec
    }

    /// The callers you keyed over, `callers`, that never hear `message`,
    /// ending `your_end` into its turn: a station sending pays no heed to
    /// your message, as MorseRunner's does not (`DxStn.pas:240-253`). One
    /// that stops sending before your message ends copies a CQ, a TU or a
    /// NIL from the fragment it heard, and anything else as garbage
    /// ([`Self::keyed_over`]); one still sending when it ends hears
    /// nothing of it, and calls again only on its timeout, but for a
    /// station putting its call right, which takes your TU all the same
    /// (`DxStn.pas:277-289`).
    fn deaf(&self, message: &[MessagePart], callers: &[Overlapping], your_end: f64) -> Vec<usize> {
        let fragment = message
            .iter()
            .any(|part| matches!(part, MessagePart::Cq | MessagePart::Tu | MessagePart::Nil));
        let thanks = message.contains(&MessagePart::Tu);
        callers
            .iter()
            .filter(|over| !(fragment && self.rest_end_sec(over) < your_end))
            .filter(|over| {
                !(thanks && self.stations[over.reply.station].caller.state == CallerState::NeedCall)
            })
            .map(|over| over.reply.station)
            .collect()
    }

    /// The callers you keyed over, `callers`, that never heard
    /// your message, ending `your_end` into its turn, being `deaf` to it,
    /// and are done before it is: having heard only its tail, each takes
    /// more patience and spends a try, as MorseRunner's callers do on
    /// hearing garbage (`DxStn.pas:249-253`), and calls again a moment
    /// after you. The station you work stays, as you are keying.
    fn keyed_over(
        &mut self,
        callers: &[Overlapping],
        your_end: f64,
        deaf: &[usize],
    ) -> Vec<Replying> {
        let done_first: Vec<usize> = callers
            .iter()
            .filter(|over| deaf.contains(&over.reply.station))
            .filter(|over| self.rest_end_sec(over) < your_end)
            .map(|over| over.reply.station)
            .collect();
        done_first
            .into_iter()
            .filter(|station| self.stations[*station].caller.heard_garbage())
            .map(Replying::unasked)
            .collect()
    }

    /// The station you work: the one your last call or partial reached,
    /// once it is in a QSO with you. In a pile-up a caller that only
    /// answered your partial is no QSO yet (§4.7 of the design), as several
    /// may answer and you work one; a single caller is the only one there.
    fn qso_station(&self) -> Option<usize> {
        self.working.filter(|index| {
            self.style == RunStyle::Single
                || self.stations[*index].caller.state != CallerState::NeedQso
        })
    }

    /// After the callers heard a message keyed with the Call field `call`: if the
    /// station your call reached is not the one it reached before,
    /// `worked`, you went on to another station, or in a pile-up left it
    /// for a call that fits nobody, and Enter's QSO is the new one's: your
    /// exchange is taken only if it took it, `took`. That is noted for
    /// Enter only if `notes`, as Wipe, or the Call field emptied, has not
    /// started the QSO's keying again since. The Call field is read as a
    /// copy of the station you still work, but never of one the message
    /// left: the call you typed for the next station is no copy of the
    /// last.
    fn went_on(&mut self, worked: Option<usize>, took: bool, call: &Typed, notes: bool) {
        if self.working == worked {
            if let Some(index) = worked {
                self.note_typed(index, call);
            }
            if notes && took {
                self.qso.exchange_sent = true;
            }
        } else if notes {
            self.qso.exchange_sent = took;
        }
    }

    /// Read the Call field, as `call` holds it, as a copy of station
    /// `index`, the one you work. In a pile-up a call another station
    /// signs, or took from you for its own, was typed for that station, and
    /// is no copy of this one: you are about to go on to it, or have just
    /// been working it. A single caller is the only station there.
    fn note_typed(&mut self, index: usize, call: &Typed) {
        if self.typed_for_another(index, &call.text) {
            return;
        }
        self.stations[index].heard.note_typed(call);
    }

    /// The Call field as `fields` hold it at `now`: the call as copied, its
    /// keys and its last edit in run time, as nothing that comes later can
    /// convert them (an Esc hears the message again after a pause).
    fn read_call(&mut self, fields: &Fields, now: MonoMs) -> CallRead {
        let keys = fields.call_keys.keys().map(|key| self.run_at(key, now));
        CallRead {
            typed: Typed {
                text: call_copied(&fields.call),
                keys,
            },
            edited_run: self.run_at(fields.call_edited, now),
        }
    }

    /// Whether `typed`, a call without `?`, is one another station than
    /// `index` signs, or took from you for its own, in a pile-up: typed for
    /// that station, it is no copy of this one. A single caller is the only
    /// station calling.
    fn typed_for_another(&self, index: usize, typed: &str) -> bool {
        self.style == RunStyle::Pileup && self.signed_by_another(index, typed)
    }

    /// Whether `typed`, a call without `?`, is one another station than
    /// `index` signs, or took from you for its own.
    fn signed_by_another(&self, index: usize, typed: &str) -> bool {
        self.stations.iter().enumerate().any(|(other, station)| {
            other != index
                && (station.caller.call.text == typed || station.heard.call_sent == typed)
        })
    }

    /// With Show copy on, what you had of each station's call and exchange
    /// when its text first showed, read at your key at `now` from `fields`,
    /// which keep what each field held then: the copy its groups keep, as
    /// the text was there to read after it, and later edits count only for
    /// the log. A call you sent with your exchange, or a QSO you logged,
    /// before its text showed was copied by ear, and keeps that copy.
    ///
    /// A call in the Call field then that another station signs or took
    /// from you, or that a station gone left there, was typed for that
    /// one, and of this one you had nothing, whatever the style: a call
    /// stays in the field past its QSO (one F3 thanked and not yet logged,
    /// one gone till Enter clears it) as the next caller's text shows. A
    /// single caller, the only one calling, keeps any other call as you had
    /// it, as its copy does without Show copy ([`Self::note_typed`]), but
    /// for one you had typed for a station worked before. In a pile-up, a
    /// call neither this station's nor close to it was typed for another
    /// caller.
    fn freeze_shown(&mut self, fields: &Fields, now: MonoMs) {
        let (rules, side) = (self.rules, self.side);
        for index in 0..self.stations.len() {
            let station = &self.stations[index];
            let heard = &station.heard;
            if heard.recorded {
                continue;
            }
            let shown = |shown: Option<Shown>| shown.filter(|shown| shown.at <= now);
            let call = shown(heard.call_shown)
                .filter(|_| heard.call_frozen.is_none() && heard.first_call.is_none())
                .map(|shown| {
                    let typed = call_copied(fields.text_at(EntryField::Call, shown.at));
                    let another =
                        self.signed_by_another(index, &typed) || self.qso.gone.contains(&typed);
                    let not_this = match self.style {
                        RunStyle::Single => {
                            self.stations.iter().enumerate().any(|(other, station)| {
                                other != index && station.heard.typed.text == typed
                            })
                        }
                        RunStyle::Pileup => !matches!(
                            match_call(&typed, &station.caller.call.text),
                            CallMatch::Exact | CallMatch::Close
                        ),
                    };
                    if another || not_this {
                        String::new()
                    } else {
                        typed
                    }
                });
            let exch = shown(heard.exch_shown)
                .filter(|_| heard.exch_frozen.is_none())
                .map(|shown| {
                    let typed = fields.text_at(EntryField::Exchange, shown.at);
                    parse_exchange(rules, side, typed)
                });
            let heard = &mut self.stations[index].heard;
            if let Some(call) = call {
                heard.call_frozen = Some((call, heard.call_asks.len()));
            }
            if let Some(exch) = exch {
                heard.exch_frozen = Some((exch, heard.exch_asks.len()));
            }
        }
    }

    /// Station `index`'s call as Show copy kept it, settled as its copy is
    /// made: in a pile-up, a call another station has taken from you since,
    /// or signs having heard it from you, and this one never took, was
    /// typed for that one, and of this one you had nothing.
    fn settle_frozen(&mut self, index: usize) {
        let Some((had, _)) = &self.stations[index].heard.call_frozen else {
            return;
        };
        if had.is_empty() || self.style != RunStyle::Pileup {
            return;
        }
        let took = |heard: &Heard| {
            heard.call_sent == *had
                || heard
                    .first_call
                    .as_ref()
                    .is_some_and(|(first, _)| first.text == *had)
        };
        let station = &self.stations[index];
        let own = took(&station.heard) || station.caller.call.text == *had;
        let theirs = self.stations.iter().enumerate().any(|(other, station)| {
            other != index
                && (took(&station.heard)
                    || (station.heard.engaged && station.caller.call.text == *had))
        });
        if theirs
            && !own
            && let Some((had, _)) = &mut self.stations[index].heard.call_frozen
        {
            had.clear();
        }
    }

    /// With Show copy on, what each station sent in `air`, the whole of it,
    /// and when it shows: a moment after its send ends, in the order they
    /// show. Nothing with Show copy off, nor of a send that went out under
    /// your message, the receiver muted, as nothing of it reached you, a
    /// caller's or QRM's rest going on after an Esc among them.
    fn shown_of(&self, air: &Air) -> Vec<ShownText> {
        if !self.settings.contest.contest_show_copy {
            return Vec::new();
        }
        let qsk = self.settings.contest.contest_qsk;
        // A caller's rest, or QRM's, that went out in part under your
        // message before an Esc is never heard, though it sounds after it.
        let mut counted = air.counted.iter();
        let mut shown: Vec<ShownText> = air
            .turn
            .segments
            .iter()
            .zip(&air.texts)
            .enumerate()
            .filter(|(at, (segment, _))| {
                let unheard = (segment.who.is_caller() && counted.next() == Some(&Counted::Muted))
                    || air.qrm_muted == Some(*at);
                segment.who != Sender::You && !air.turn.muted(segment, qsk) && !unheard
            })
            .map(|(_, (segment, text))| ShownText {
                at: air
                    .heard_from
                    .plus(whole_ms(segment.end_sec) + SHOWN_AFTER_MS),
                text: text.clone(),
                qrm: segment.who == Sender::Qrm,
            })
            .collect();
        shown.sort_by_key(|text| text.at);
        shown
    }

    /// The texts Show copy shows of the turn last on the air give way to
    /// the next turn's, or to a pause, at `run_now`: one yet to show never
    /// will, so it is no copy's cut-off, and its station's next send shows
    /// it.
    fn give_way(&mut self, run_now: u64) {
        self.shown_last.clear();
        let unshown = |shown: &mut Option<Shown>| {
            if shown.is_some_and(|shown| shown.run > run_now) {
                *shown = None;
            }
        };
        for station in &mut self.stations {
            unshown(&mut station.heard.call_shown);
            unshown(&mut station.heard.exch_shown);
        }
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
    /// logging does; a call is the call last sent, and not corrected yet,
    /// and once a call goes out that is none of a gone station's, Enter
    /// sends that station's calls again. Whether your exchange was taken is
    /// for the callers to say.
    fn note_sent(&mut self, message: &[MessagePart]) {
        for part in message {
            match part {
                MessagePart::Cq | MessagePart::Tu => self.end_qso(),
                MessagePart::HisCall(call) => {
                    self.qso.call_sent = call.clone();
                    self.qso.corrected = false;
                    self.qso.call_taken = false;
                    self.qso.thanked.clear();
                    // Sent again (B4 once more, or F5), the call is still
                    // that of a station that has gone.
                    if !self.qso.gone.contains(&call.replace('?', "")) {
                        self.qso.gone.clear();
                    }
                }
                _ => {}
            }
        }
    }

    /// The callers hear `message`, keyed with `fields`, the call last edited
    /// at `edited_run`, all but the `deaf`, who were still sending: who you
    /// work now, the call each took with your exchange, and who replies;
    /// and whether the station you work now took your exchange, as one
    /// that also took it in a tie, or the station just logged, is no QSO of
    /// Enter's. A call goes only to the stations it fits best
    /// ([`Likeness`]), among all on the frequency, those still sending too,
    /// as MorseRunner ranks them (`StnColl.pas:171-194`): a call keyed over
    /// its own station is no near call's. A message that `logs` asks no
    /// caller to call again.
    fn hear(
        &mut self,
        message: &[MessagePart],
        fields: &Fields,
        call: &CallRead,
        deaf: &[usize],
        logs: bool,
    ) -> (Vec<Replying>, bool) {
        let keyed = Keyed {
            call: call.typed.text.clone(),
            exch: parse_exchange(self.rules, self.side, &fields.exch),
        };
        let call_sent = message.iter().rev().find_map(|part| match part {
            MessagePart::HisCall(call) => Some(call.replace('?', "")),
            _ => None,
        });
        // A single caller still calling that answers your CQ or TU, or
        // your call alone, brings its call again, as a `?` would; pile-up
        // callers, calling anyway, do not. The TU of a key that logs is
        // there to log: after your TU without logging (F3), Enter's TU and
        // log brings the caller that TU brought again, which you did not
        // ask for, so it costs only time (§4.6 of the design).
        let calls = self.style == RunStyle::Single
            && !logs
            && (self.calls(message) || message.contains(&MessagePart::MyCall));
        let hears = |index: &usize| !deaf.contains(index);
        // In a pile-up a call that is not the station you work's own ends
        // its QSO with you (`DxOper.pas:744-748`): you work it no more, and
        // the call goes to whoever takes it, or to nobody.
        let single = self.style == RunStyle::Single;
        let names_someone = message.iter().any(
            |part| matches!(part, MessagePart::HisCall(sent) if sent.chars().any(|c| c != '?')),
        );
        let worked = self.working;
        let mut left = false;
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
        let mut took = Vec::new();
        for index in (0..self.stations.len()).filter(hears) {
            let station = &mut self.stations[index];
            let reaction = station.caller.hear(message, &best);
            let calls_again = calls && station.caller.state == CallerState::NeedQso;
            left |= Some(index) == worked && !single && names_someone && !reaction.matched;
            let heard = &mut station.heard;
            // A single caller is engaged once it answers anything of its
            // call. In a pile-up several may answer a partial and you work
            // one: a station is engaged once it hears its call from you, or
            // takes your exchange.
            heard.engaged |=
                reaction.took_exchange || (reaction.matched && (single || reaction.called));
            if reaction.matched {
                heard.note_typed(&call.typed);
                if let Some(call) = &call_sent {
                    heard.call_sent.clone_from(call);
                }
                self.working = Some(index);
                // Its call went out: how long the copy took, should it go
                // no further.
                if heard.first_call.is_none() {
                    heard.call_ms = Some(copy_ms(&heard.call_ends, call.edited_run));
                }
            }
            if reaction.took_exchange {
                took.push(index);
            }
            // The call it took with your exchange is its copy, or the one
            // that went with an exchange it missed: the one last sent to
            // it, should the message hold none.
            let with_exchange = reaction.took_exchange || reaction.missed_exchange;
            if with_exchange && self.working == Some(index) && heard.first_call.is_none() {
                let took_ms = copy_ms(&heard.call_ends, call.edited_run);
                let copy = if heard.call_sent.is_empty() {
                    heard.typed.clone()
                } else {
                    call.typed.as_copy(&heard.call_sent)
                };
                heard.first_call = Some((copy, took_ms));
            }
            if reaction.replies {
                replying.push(Replying {
                    station: index,
                    asked: reaction.asked || calls_again,
                    again: reaction.again,
                    first_exchange: reaction.first_exchange,
                    keyed: Some(keyed.clone()),
                    on_time_out: false,
                });
            }
        }
        if left && self.working == worked {
            self.working = None;
        }
        let took = self.working.is_some_and(|working| took.contains(&working));
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
    /// delay, and each caller `garbled` by your message
    /// ([`Self::keyed_over`]) after its rest, and the calls of those whose
    /// wait runs out before it is over ([`Self::time_outs`]); `unheard`, the
    /// callers before they heard the message keyed last. A turn with
    /// nothing in it is not sent. Keyed at `run_ms`, it starts then, or, a
    /// continuation spliced onto the turn before, where that one leaves
    /// off ([`Splice`]).
    fn play(
        &mut self,
        mine: Carried,
        unheard: Option<Unheard>,
        replying: Vec<Replying>,
        garbled: Vec<Replying>,
        run_ms: u64,
        now: MonoMs,
    ) -> Vec<ContestEffect> {
        // When it starts, in run time, and where your message starts in it.
        let start_run = mine.splice.map_or(run_ms, |splice| splice.run);
        let lead_sec = mine.splice.map_or(0.0, |splice| splice.lead_sec);
        self.give_way(start_run);
        let mut segments = Vec::new();
        // What each segment sends, the whole of it, for Show copy.
        let mut texts = Vec::new();
        let mut your_end = 0.0;
        let mut my_chars = Vec::new();
        let heard_you = !mine.text.is_empty();
        if heard_you {
            let voice = your_voice(&self.settings, self.my_wpm);
            your_end = lead_sec + sending_sec(&mine.text, &self.settings, &voice);
            my_chars = char_spans(&mine.text, &self.settings, &voice)
                .into_iter()
                .map(|span| (span.at, lead_sec + span.end_sec))
                .collect();
            self.sent_wpm.push(voice.char_wpm);
            segments.push(Segment {
                who: Sender::You,
                text: mine.text,
                voice,
                start_sec: lead_sec,
                end_sec: your_end,
                fade: None,
                skip_sec: 0.0,
            });
            texts.push(String::new());
        }
        let mut replies = Vec::new();
        let mut counted = Vec::new();
        // Callers you keyed over go on from where they were, heard over you,
        // their start charged already, and send nothing more till they are
        // done.
        let mut busy = Vec::new();
        let carried = mine.callers.len();
        for over in mine.callers {
            let station = over.reply.station;
            let caller = &self.stations[station].caller;
            let end_sec = sending_sec(&over.text, &self.settings, &over.voice) - over.skip_sec;
            segments.push(Segment {
                who: sender(station),
                text: over.text,
                voice: over.voice,
                start_sec: 0.0,
                end_sec,
                fade: self.fade(caller, start_run, 0.0),
                skip_sec: over.skip_sec,
            });
            texts.push(over.whole);
            replies.push((over.reply, over.says));
            counted.push(over.counted);
            busy.push((station, end_sec));
        }
        // A station leaving the frequency, your message sending it on its
        // way while it was still sending, is heard out before anyone calls:
        // the rest it goes on with, and its sign-off. Calling across it,
        // the next caller made one more station at once than the Band card
        // lets call.
        let leaving: Vec<bool> = self
            .stations
            .iter()
            .map(|station| !station.caller.state.active())
            .collect();
        let mut left_sec = busy
            .iter()
            .filter(|(station, _)| leaving[*station])
            .fold(0.0, |left, (_, end)| f64::max(left, *end));
        let (gone, staying): (Vec<Replying>, Vec<Replying>) = replying
            .into_iter()
            .chain(garbled)
            .partition(|replying| leaving[replying.station]);
        for replying in gone.into_iter().chain(staying) {
            let goes = leaving[replying.station];
            let caller = &mut self.stations[replying.station].caller;
            let Some((said, says)) = caller.reply(self.rules, self.side, &self.my_call, start_run)
            else {
                continue;
            };
            let free_sec = busy
                .iter()
                .filter(|(station, _)| *station == replying.station)
                .fold(your_end, |free, (_, end)| free.max(*end));
            let free_sec = if goes {
                free_sec
            } else {
                free_sec.max(left_sec)
            };
            let start_sec = free_sec + caller.draw_delay_sec();
            let voice = caller.sending_voice();
            let end_sec = start_sec + sending_sec(&said, &self.settings, &voice);
            if goes {
                left_sec = left_sec.max(end_sec);
            }
            let caller = &self.stations[replying.station].caller;
            texts.push(said.clone());
            segments.push(Segment {
                who: sender(replying.station),
                text: said,
                voice,
                start_sec,
                end_sec,
                fade: self.fade(caller, start_run, start_sec),
                skip_sec: 0.0,
            });
            replies.push((replying, says));
            counted.push(Counted::Nothing);
        }
        let mut effects = Vec::new();
        // QRM going on alone, past an Esc, is a turn too.
        if segments.is_empty() && mine.qrm.is_none() {
            self.timers.heard = None;
            self.resolve(&mine.pending, &mut effects);
            effects.extend(self.arm_patience(run_ms, &[]));
            return effects;
        }
        // QRM sends in the turn as it comes due, over whoever is sending,
        // and runs on to its end, as it never stops for you; the callers'
        // waits are planned with it, so it never cuts the turn short. A rest
        // of it carried over your key goes on first, and nothing more of it
        // sends over that rest.
        let mut qrm_muted = None;
        if let Some(qrm) = self.qrm.as_mut() {
            if let Some(going_on) = mine.qrm {
                qrm.carried(start_run, going_on.rest.end_sec);
                qrm_muted = going_on.muted.then_some(segments.len());
                segments.push(going_on.rest);
                texts.push(going_on.whole);
            }
            let length_sec = segments
                .iter()
                .map(|segment| segment.end_sec)
                .fold(0.0, f64::max);
            segments.extend(qrm.sends(start_run, length_sec, &self.settings));
        }
        let message_end = heard_you.then_some(your_end);
        let waits = self.time_outs(
            &mut segments,
            &mut replies,
            &mut counted,
            start_run,
            message_end,
        );
        // The QRM it sends and the calls on timeouts send the whole of
        // their text.
        let planned = texts.len();
        texts.extend(
            segments[planned..]
                .iter()
                .map(|segment| segment.text.clone()),
        );
        // The checks of the rows waiting on the turn show once your TU has
        // gone out and the station it closed has signed off, as MorseRunner
        // shows one once the station is done (`Contest.pas:855-895`): not
        // once every caller after it has fallen quiet.
        let checks_at_sec = checks_at_sec(&mine.parts, &my_chars, your_end, &segments, &replies);
        // A continuation starts where the turn before leaves off, its sound
        // reaching the ear then: the player starts it at that moment.
        let starts = now.plus(start_run - run_ms);
        let turn = Turn {
            id: self.fresh_id(),
            at_run_ms: start_run,
            starts_at: mine.splice.map(|_| starts),
            segments,
        };
        self.air = Some(Air {
            turn: turn.clone(),
            keyed_at: starts,
            heard_from: starts,
            output_from: starts,
            first_start: None,
            parts: mine.parts,
            my_chars,
            counted,
            qrm_muted,
            replies,
            carried,
            texts,
            pending: mine.pending,
            checks_at_sec,
            unheard,
            waits,
        });
        effects.push(ContestEffect::PlayTurn(turn));
        effects.extend(self.arm_heard(run_ms));
        effects
    }

    /// The heard timer, to the next moment after `run_now` the turn on the
    /// air reaches you that Enter and the screen are to hear of: the end of
    /// a send of the station you work, whose answer or correction moves
    /// Enter, or the moment the checks waiting on the turn show. Timed from
    /// when the turn was keyed, it may fire before the player's lead has
    /// passed, and then counts what has reached you and waits again.
    fn arm_heard(&mut self, run_now: u64) -> Option<ContestEffect> {
        self.timers.heard = None;
        let air = self.air.as_ref()?;
        let heard_run = air.heard_run();
        let working = self.working.map(sender);
        let qsk = self.settings.contest.contest_qsk;
        // A send under your message, the receiver muted, tells you nothing.
        let ends = air
            .turn
            .segments
            .iter()
            .filter(|segment| Some(segment.who) == working && !air.turn.muted(segment, qsk))
            .map(|segment| heard_run + whole_ms(segment.end_sec));
        let checks = (!air.pending.is_empty()).then(|| heard_run + whole_ms(air.checks_at_sec));
        let next = ends.chain(checks).filter(|at| *at > run_now).min()?;
        let (id, effect) = self.start_timer(next - run_now);
        self.timers.heard = Some(id);
        Some(effect)
    }

    /// The turn on the air has reached you as far as `now`: what of it has
    /// counts, for Enter and the screen, and the checks whose TU is out
    /// show. Only a look at the clock, as a timer's is (6.3 of the design).
    fn heard_so_far(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        let Some(mut air) = self.air.take() else {
            return Vec::new();
        };
        let run_now = self.clock.map_or(0, |clock| clock.peek_ms(now));
        let mut effects = Vec::new();
        self.count_heard(&mut air, Some(run_now), run_now, &mut effects);
        self.air = Some(air);
        effects.extend(self.arm_heard(run_now));
        effects
    }

    /// The waits of the callers on the frequency in a turn keyed at
    /// `run_ms`, `segments` with `replies`, and the calls of those whose wait
    /// runs out before it is over. A caller that sends in it waits for you
    /// from the end of its last send; one that heard your message, ending
    /// at `message_end`, and sends nothing, from its end; in a turn of
    /// callers alone, one that sends nothing keeps the wait it has. One
    /// whose wait runs out while the turn is still going calls in it then,
    /// over whoever is sending, as MorseRunner's stations each count down
    /// their own timeouts (`DxStn.pas:210-238`): so no turn has to give way
    /// to a timeout, which would send again a character already sounding.
    /// A timeout that would send its caller away sends nothing, and comes
    /// once the turn is over. Returns each wait the turn sets, as it stands
    /// at the turn's end, in milliseconds from its start.
    fn time_outs(
        &mut self,
        segments: &mut Vec<Segment>,
        replies: &mut Vec<(Replying, Says)>,
        counted: &mut Vec<Counted>,
        run_ms: u64,
        message_end: Option<f64>,
    ) -> Vec<(usize, u64)> {
        let count = self.stations.len();
        // When each caller's wait runs out, in milliseconds into the turn,
        // and whether the turn sets it.
        let mut due: Vec<Option<u64>> = vec![None; count];
        let mut sets = vec![false; count];
        for index in 0..count {
            let sent = segments
                .iter()
                .filter(|segment| segment.who == sender(index))
                .map(|segment| segment.end_sec)
                .reduce(f64::max);
            let caller = &mut self.stations[index].caller;
            if !caller.times_out() {
                continue;
            }
            match sent.or(message_end) {
                Some(from_sec) => {
                    due[index] = Some(whole_ms(from_sec) + caller.draw_timeout_ms());
                    sets[index] = true;
                }
                None => due[index] = caller.deadline.map(|at| at.saturating_sub(run_ms)),
            }
        }
        // Its calls on its timeout in the turn, each yet to cost it a try.
        let mut pending = vec![0; count];
        loop {
            let end = whole_ms(
                segments
                    .iter()
                    .map(|segment| segment.end_sec)
                    .fold(0.0, f64::max),
            );
            let next = (0..count)
                .filter_map(|index| Some((due[index]?, index)))
                .filter(|(at, index)| {
                    *at < end
                        && self.stations[*index]
                            .caller
                            .calls_on_time_out(pending[*index])
                })
                .min();
            let Some((at, index)) = next else {
                break;
            };
            let caller = &mut self.stations[index].caller;
            let Some((said, says)) = caller.reply(self.rules, self.side, &self.my_call, run_ms)
            else {
                break;
            };
            let start_sec = at as f64 / 1_000.0 + caller.draw_delay_sec();
            let voice = caller.sending_voice();
            let end_sec = start_sec + sending_sec(&said, &self.settings, &voice);
            due[index] = Some(whole_ms(end_sec) + caller.draw_timeout_ms());
            sets[index] = true;
            pending[index] += 1;
            let caller = &self.stations[index].caller;
            segments.push(Segment {
                who: sender(index),
                text: said,
                voice,
                start_sec,
                end_sec,
                fade: self.fade(caller, run_ms, start_sec),
                skip_sec: 0.0,
            });
            replies.push((Replying::on_time_out(index), says));
            counted.push(Counted::Nothing);
        }
        (0..count)
            .filter(|index| sets[*index])
            .filter_map(|index| Some((index, due[index]?)))
            .collect()
    }

    /// Take a turn off the air at `run_now`, or, for a key that `splices`
    /// its continuation onto it while your message in it is still going
    /// out, where the continuation takes over ([`Self::splice`]): what was
    /// heard of it by then counts. What is left of your message carries on
    /// with its parts, and so do the log rows waiting on it while it still
    /// holds their TU; otherwise their checks show now.
    ///
    /// A splice leaves the turn on the air to play out the character
    /// sounding at the key, and any the player can no longer take from it,
    /// and the rest goes on from the next, at the very moment it would have,
    /// as the player starts the continuation there ([`Turn::starts_at`]).
    /// What the turn plays till then counts at the key, as planned:
    /// everything is decided when you key, so an Esc or a pause before the
    /// splice leaves counted what the turn would have played up to it.
    /// Anything else takes the turn off the air at once, and what is left
    /// of your message goes on from the character sounding then, sent again
    /// whole. The callers still sending then are carried too
    /// ([`Self::overlapping`]): over a key, or past an Esc, they go on, in
    /// any run ([`Self::send`]).
    fn cut(
        &mut self,
        mut air: Air,
        run_now: u64,
        splices: bool,
        effects: &mut Vec<ContestEffect>,
    ) -> Carried {
        let splice = splices.then(|| self.splice(&air, run_now)).flatten();
        let at = splice.map_or(run_now, |splice| splice.run);
        self.count_heard(&mut air, Some(at), at, effects);
        self.give_way(at);
        // A reply yet to begin goes no further: nothing in it was sent.
        for ((reply, says), counted) in air.replies.iter().zip(&air.counted) {
            if *counted == Counted::Nothing {
                self.stations[reply.station].caller.unsent(*says);
            }
        }
        let heard_run = air.heard_run();
        // Whether it has sounded by then: the player has said it started,
        // or it plays on to the splice, as planned. Till the player says so
        // it is timed from its key, but nothing of it has reached you, so
        // nothing of it went out under your message.
        let sounded = splice.is_some() || air.first_start.is_some();
        let callers = self.overlapping(&air, at, sounded);
        let qrm = self.qrm_going_on(&air, at, sounded);
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
                let from = match splice {
                    Some(splice) => splice.from?,
                    None => {
                        let spans = char_spans(&yours.text, &self.settings, &yours.voice);
                        let at_sec =
                            (run_now as f64 - heard_run as f64) / 1_000.0 - yours.start_sec;
                        sounding_at(&spans, at_sec)?
                    }
                };
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
            qrm,
            splice,
        }
    }

    /// Where a key at `run_now` splices its continuation onto `air`
    /// ([`Splice`]). The player's sound is [`Self::lead_ms`] behind the key,
    /// and the turn on the air has to be ridden down before the next starts
    /// ([`SEND_RELEASE_SEC`]), so the continuation takes over no sooner than
    /// both after the key: at the start of the first character of your
    /// message that late or later, which the turn on the air has not begun
    /// to play; with none, in the word space after your message, half of it
    /// in at the soonest, so the turn on the air is ridden down in silence.
    /// None once your message has reached you whole, or with none in the
    /// turn: the next turn starts as soon as it can.
    fn splice(&self, air: &Air, run_now: u64) -> Option<Splice> {
        let yours = air
            .turn
            .segments
            .iter()
            .find(|segment| segment.who == Sender::You)?;
        let heard_run = air.heard_run();
        let spans = char_spans(&yours.text, &self.settings, &yours.voice);
        let at_sec = (run_now as f64 - heard_run as f64) / 1_000.0 - yours.start_sec;
        let soonest = at_sec + self.lead_ms as f64 / 1_000.0 + SEND_RELEASE_SEC;
        let run_at = |sec: f64| (heard_run + whole_ms(yours.start_sec + sec)).max(run_now);
        if let Some(next) = spans.iter().find(|span| span.start_sec >= soonest) {
            return Some(Splice {
                run: run_at(next.start_sec),
                from: Some(next.at),
                lead_sec: 0.0,
            });
        }
        let end = spans.last()?.end_sec;
        if end <= at_sec {
            return None;
        }
        let gap = word_gap_sec(&self.settings, &yours.voice);
        let run = run_at(soonest.max(end + gap / 2.0));
        // Your next message starts as one message would have it, or as
        // soon as the player can start it.
        let takes_over_sec = (run - heard_run) as f64 / 1_000.0 - yours.start_sec;
        Some(Splice {
            run,
            from: None,
            lead_sec: (end + gap - takes_over_sec).max(0.0),
        })
    }

    /// What is left at `run_now` of the QRM sending in `air`, from the
    /// character sounding then, sent again whole, as a pile-up caller's
    /// rest is: a station on your frequency that is not calling you does
    /// not stop for you. One at the start of the turn whose sound has not
    /// reached the ear yet goes on as it was, and one whose first character
    /// is still sounding from where it is ([`Self::going_on_from`]); one
    /// that had not begun is gone with the turn. Without full break-in, one
    /// some of which went out under your message, the turn having `sounded`
    /// by `run_now`, goes on unheard, as a caller's rest does
    /// ([`Self::overlapping`]), and so does one that went on so already.
    fn qrm_going_on(&self, air: &Air, run_now: u64, sounded: bool) -> Option<QrmRest> {
        let heard_run = air.heard_run();
        let (at, (segment, whole)) =
            air.turn
                .segments
                .iter()
                .zip(&air.texts)
                .enumerate()
                .find(|(_, (segment, _))| {
                    segment.who == Sender::Qrm && {
                        let start = heard_run + whole_ms(segment.start_sec);
                        let end = heard_run + whole_ms(segment.end_sec);
                        end > run_now && (start < run_now || segment.start_sec == 0.0)
                    }
                })?;
        let start = heard_run + whole_ms(segment.start_sec);
        let qsk = self.settings.contest.contest_qsk;
        let muted = air.qrm_muted == Some(at)
            || (sounded && start < run_now && air.turn.muted(segment, qsk));
        let elapsed = run_now.saturating_sub(start) as f64 / 1_000.0;
        let (from, skip_sec) = self.going_on_from(segment, elapsed)?;
        let text = segment.text[from..].to_string();
        let rest = Segment {
            who: Sender::Qrm,
            end_sec: sending_sec(&text, &self.settings, &segment.voice) - skip_sec,
            text,
            voice: segment.voice,
            start_sec: 0.0,
            fade: segment.fade.map(|fade| Fade {
                from_sec: fade.from_sec + elapsed,
                ..fade
            }),
            skip_sec,
        };
        Some(QrmRest {
            rest,
            whole: whole.clone(),
            muted,
        })
    }

    /// Where a caller's or QRM's `segment` goes on from, carried on
    /// `elapsed` seconds after its sound began to reach you: the byte of its
    /// text the rest starts at, and how far into that rest it starts
    /// ([`Segment::skip_sec`]). The character sounding then goes again
    /// whole; but a rest the turn began with, whose first character a key
    /// sent again whole already, carried on while that character is still
    /// going goes on from where it is. Keys over a station sending a
    /// character slower than yours would otherwise start that character
    /// over at every splice, and never let it get any further. None once
    /// all of it has been sent.
    fn going_on_from(&self, segment: &Segment, elapsed: f64) -> Option<(usize, f64)> {
        let spans = char_spans(&segment.text, &self.settings, &segment.voice);
        let at_sec = elapsed.max(0.0) + segment.skip_sec;
        let from = sounding_at(&spans, at_sec)?;
        let first = spans.first().map(|span| span.at);
        if segment.start_sec == 0.0 && Some(from) == first {
            Some((0, at_sec))
        } else {
            Some((from, 0.0))
        }
    }

    /// The callers in `air` still sending at `run_now`, each with what is
    /// left of its message from the character sounding then, or, still on
    /// the character a splice before sent again, from where it is
    /// ([`Self::going_on_from`]). One carried on from an earlier turn whose
    /// rest has not reached the ear yet goes on as it was: it began before,
    /// and the player's lead is no gap in it. A
    /// station goes on with one send: a later one, counted as begun before
    /// a retry of the sound moved the ear back, is no second voice of it.
    /// A send some of which went out under your message, the receiver
    /// muted without full break-in, goes on unheard ([`Counted::Muted`]): a
    /// send heard in part is no send heard, even once its rest sounds with
    /// nothing of yours going out, after an Esc; none did of a turn that
    /// has not `sounded` by `run_now` ([`Self::cut`]).
    fn overlapping(&self, air: &Air, run_now: u64, sounded: bool) -> Vec<Overlapping> {
        let heard_run = air.heard_run();
        let qsk = self.settings.contest.contest_qsk;
        let callers = air
            .turn
            .segments
            .iter()
            .zip(&air.texts)
            .filter(|(segment, _)| segment.who.is_caller());
        let mut going_on = BTreeSet::new();
        callers
            .zip(air.replies.iter().zip(&air.counted))
            .filter_map(|((segment, whole), ((reply, says), counted))| {
                let start = heard_run + whole_ms(segment.start_sec);
                let end = heard_run + whole_ms(segment.end_sec);
                if start >= run_now && *counted != Counted::Nothing {
                    return Some(Overlapping {
                        reply: reply.clone(),
                        says: *says,
                        counted: *counted,
                        text: segment.text.clone(),
                        whole: whole.clone(),
                        skip_sec: segment.skip_sec,
                        voice: segment.voice,
                    });
                }
                if start >= run_now || end <= run_now {
                    return None;
                }
                let elapsed = (run_now - start) as f64 / 1_000.0;
                let (from, skip_sec) = self.going_on_from(segment, elapsed)?;
                // An exchange yet to reach you starts as far into what is
                // left.
                let counted = match *counted {
                    _ if sounded && air.turn.muted(segment, qsk) => Counted::Muted,
                    Counted::CallBegun { exchange_from } => Counted::CallBegun {
                        exchange_from: exchange_from.saturating_sub(from),
                    },
                    counted => counted,
                };
                Some(Overlapping {
                    reply: reply.clone(),
                    says: *says,
                    counted,
                    text: segment.text[from..].to_string(),
                    whole: whole.clone(),
                    skip_sec,
                    voice: segment.voice,
                })
            })
            .filter(|over| going_on.insert(over.reply.station))
            .collect()
    }

    /// The fading of a segment of `caller`'s starting `start_sec` into a
    /// turn starting at `run_ms`: the band's, and its own flutter, read
    /// from the caller's own point in them.
    fn fade(&self, caller: &Caller, run_ms: u64, start_sec: f64) -> Option<Fade> {
        Fade::along(
            self.qsb,
            caller.flutter,
            caller.fade_from_sec,
            run_ms,
            start_sec,
        )
    }

    /// Count what was heard of the callers in `air` and not counted yet:
    /// every segment, or what of each had reached the ear by `upto`. A
    /// repeat is paid for once it has begun to reach you, even if you key
    /// over the rest of it, and only if a send of the same had begun to
    /// reach you before: a call you asked for again, or an exchange a key
    /// of yours drew again. The checks of the rows waiting on it show once
    /// their TU has reached you.
    ///
    /// Without full break-in, a send any of which went out while you sent,
    /// the receiver muted, is not heard at all ([`Turn::muted`]): nothing of
    /// it counts, for Enter, the copies or Show copy, though what it costs
    /// its caller is spent once it is on its way, as it was sent.
    fn count_heard(
        &mut self,
        air: &mut Air,
        upto: Option<u64>,
        run_now: u64,
        effects: &mut Vec<ContestEffect>,
    ) {
        let heard_run = air.heard_run();
        let output_run = air.output_run();
        let latency_ms = air.latency_ms();
        let heard_from = air.heard_from;
        let show_copy = self.settings.contest.contest_show_copy;
        let qsk = self.settings.contest.contest_qsk;
        // What Show copy shows is read off the screen, and a turn tried
        // again or played late went out otherwise than planned: neither
        // times a copy.
        let timeable = !show_copy && !self.untrusted.contains(&air.turn.id);
        if upto.is_none_or(|upto| heard_run + whole_ms(air.checks_at_sec) <= upto) {
            let pending = std::mem::take(&mut air.pending);
            self.resolve(&pending, effects);
        }
        let Air {
            turn,
            replies,
            counted,
            texts,
            ..
        } = air;
        let callers = turn
            .segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| segment.who.is_caller());
        for (((at, segment), (reply, says)), counted) in callers.zip(replies.iter()).zip(counted) {
            let start = heard_run + whole_ms(segment.start_sec);
            let end = heard_run + whole_ms(segment.end_sec);
            // When it began to go out to the output.
            let out_start = output_run + whole_ms(segment.start_sec);
            let muted = turn.muted(segment, qsk);
            let station = &mut self.stations[reply.station];
            let heard = &mut station.heard;
            if *counted == Counted::Nothing {
                if upto.is_some_and(|upto| start >= upto) {
                    continue;
                }
                // A call on its timeout costs it the try, or the silence,
                // once it is on its way, heard or not.
                if reply.on_time_out {
                    station.caller.wait_over();
                }
                if muted {
                    *counted = Counted::Muted;
                    continue;
                }
                *counted = Counted::Begun;
                if let Some(keyed) = &reply.keyed {
                    // Its call, alone or putting yours right, as you asked.
                    let call = matches!(
                        says,
                        Says::Call | Says::Correction | Says::CorrectionAndExchange
                    );
                    if call && reply.asked && heard.call_begun && heard.first_call.is_none() {
                        heard.call_asks.push(keyed.call.clone());
                    }
                    // Its exchange, whatever key drew it, but the one your
                    // exchange drew as it first took it, which it owes, a
                    // careless operator's sent early before it; one sent
                    // with the call put right counts once it begins, below.
                    if *says == Says::Exchange && heard.exch_begun && !reply.first_exchange {
                        heard.exch_asks.push(keyed.exch.clone());
                    }
                }
                heard.note_voice(*says, segment.voice);
                match says {
                    Says::Call | Says::Correction => {
                        heard.call_begun = true;
                        heard.call_sends.began(out_start);
                    }
                    Says::Exchange => {
                        heard.exch_begun = true;
                        heard.exch_sends.began(out_start);
                    }
                    // Its call, and its exchange once that begins too.
                    Says::CorrectionAndExchange => {
                        heard.call_begun = true;
                        heard.call_sends.began(out_start);
                        if let Some(exchange_from) = exchange_at(&segment.text) {
                            *counted = Counted::CallBegun { exchange_from };
                        }
                    }
                    Says::Query | Says::Signoff => {}
                }
                // The station you work sending its exchange, or asking for
                // yours, took the call last sent for its own, as you hear:
                // a slip it would have put right.
                let answers = matches!(says, Says::Exchange | Says::Query);
                if answers
                    && self.working == Some(reply.station)
                    && heard.call_sent == self.qso.call_sent
                {
                    self.qso.call_taken = true;
                }
            }
            // The rest of a send carried on under your message, its start
            // heard before you keyed over it, never reaches you whole.
            if muted || *counted == Counted::Muted {
                continue;
            }
            // The exchange sent with the call put right, once it begins to
            // reach you: what you log of it is a copy, and it is a repeat
            // you asked for only when your `?` or `NR?` drew it, as a
            // partial or a slip asks for the call alone.
            if let Counted::CallBegun { exchange_from } = *counted {
                let spans = char_spans(&segment.text, &self.settings, &segment.voice);
                let begins_sec = spans
                    .iter()
                    .find(|span| span.at >= exchange_from)
                    .map_or(segment.end_sec - segment.start_sec, |span| {
                        (span.start_sec - segment.skip_sec).max(0.0)
                    });
                if upto.is_none_or(|upto| start + whole_ms(begins_sec) < upto) {
                    *counted = Counted::Begun;
                    if let Some(keyed) = &reply.keyed
                        && reply.again
                        && heard.exch_begun
                    {
                        heard.exch_asks.push(keyed.exch.clone());
                    }
                    heard.note_voice(Says::Exchange, segment.voice);
                    heard.exch_begun = true;
                    heard.exch_sends.began(out_start + whole_ms(begins_sec));
                }
            }
            if *counted == Counted::Whole || upto.is_some_and(|upto| end > upto) {
                continue;
            }
            *counted = Counted::Whole;
            heard.first.get_or_insert(start.min(run_now));
            // The first sending of its call, and of its exchange, heard
            // once, whole and alone, is what their copies' characters are
            // timed from: not a rest carried on from a turn you keyed over.
            if timeable && segment.skip_sec == 0.0 && texts.get(at) == Some(&segment.text) {
                let spans = char_spans(&segment.text, &self.settings, &segment.voice);
                let sounded = |within: std::ops::Range<usize>, token: &str| {
                    let ends = sounded_alone(turn, at, &spans, within, token)?;
                    Some(Sounded {
                        turn: turn.id,
                        from: out_start,
                        ends,
                        latency_ms,
                    })
                };
                let length = segment.text.len();
                let exchange_from = match says {
                    Says::Exchange => Some(0),
                    Says::CorrectionAndExchange => exchange_at(&segment.text),
                    _ => None,
                };
                let call_to = match says {
                    Says::Call | Says::Correction => Some(length),
                    Says::CorrectionAndExchange => Some(exchange_from.unwrap_or(length)),
                    _ => None,
                };
                if let Some(call_to) = call_to
                    && heard.call_sends.count == 1
                    && heard.call_sounded.is_none()
                {
                    heard.call_sounded = sounded(0..call_to, &station.caller.call.text);
                }
                if let Some(from) = exchange_from
                    && heard.exch_sends.count == 1
                    && heard.exch_sounded.is_empty()
                    && let Some(tokens) = station.caller.sent_exchange()
                {
                    heard.exch_sounded = tokens
                        .iter()
                        .filter_map(|(field, token)| Some((*field, sounded(from..length, token)?)))
                        .collect();
                }
            }
            // With Show copy on, its text shows a moment after it ends:
            // what you had of its call, or its exchange, by then is the
            // copy its groups keep.
            if show_copy {
                let shown = Shown {
                    at: heard_from.plus(whole_ms(segment.end_sec) + SHOWN_AFTER_MS),
                    run: end + SHOWN_AFTER_MS,
                };
                if matches!(
                    says,
                    Says::Call | Says::Correction | Says::CorrectionAndExchange
                ) {
                    heard.call_shown.get_or_insert(shown);
                }
                if matches!(says, Says::Exchange | Says::CorrectionAndExchange) {
                    heard.exch_shown.get_or_insert(shown);
                }
            }
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
                Says::Correction | Says::CorrectionAndExchange => {
                    if reply.asked && restarts {
                        heard.call_ends.push(end);
                    }
                    // Its exchange, sent with the call put right, is timed
                    // as one sent alone.
                    let exchange = *says == Says::CorrectionAndExchange;
                    if exchange && (restarts || heard.exch_ends.is_empty()) {
                        heard.exch_ends.push(end);
                    }
                    let it_took = heard.call_sent.clone();
                    // It puts right a whole call you sent, even in answer to
                    // a `?`; its call answering your partial, or a piece of
                    // its call, puts nothing right.
                    let sent = &self.qso.call_sent;
                    let own = &self.stations[reply.station].caller.call.text;
                    let missed_fix = sent == own;
                    let wrong = parse_callsign(sent).is_some() && !missed_fix;
                    let working = self.working == Some(reply.station);
                    // Said once: the correction again, on its timeout or
                    // over a fix it never heard, is no news.
                    let news = !self.qso.corrected;
                    if working && missed_fix {
                        // It corrects you though you sent its call right: it
                        // never heard the call you put right, sent over its
                        // correction. Enter's QSO is as it was before you
                        // sent that call, so Enter sends it again rather than
                        // wait for a fix already made.
                        self.qso.call_sent = it_took;
                        self.qso.corrected = true;
                        self.qso.call_taken = false;
                    } else if working && (wrong || !reply.asked) {
                        self.qso.corrected = true;
                        self.qso.call_taken = false;
                        if news {
                            effects.push(ContestEffect::Announce(
                                "They corrected you: fix the call, then Enter".to_string(),
                            ));
                        }
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
    /// with it. Your message cut short, the callers listening copy the rest
    /// as garbage and answer it at once ([`Self::garbled`]).
    ///
    /// Esc stops your station alone, as MorseRunner's `AbortSend` does
    /// (`MyStn.pas:108-116`): the callers still sending, a single caller
    /// among them, and any QRM go on from where they were, as over a key
    /// ([`Self::cut`]), in a turn with nothing of yours in it, and each
    /// waits from the end of its send.
    ///
    /// With nothing of yours left to go out, your station is sending
    /// nothing, and Esc stops nothing: the turn plays on as it is, the
    /// sends in it never cut and every reply still to come on time, as
    /// `AbortSend` touches no other station's send. MorseRunner's has the
    /// callers listening answer garbage then too, of a message that never
    /// sounded; Dust's do not (§15.1 of the design).
    fn stop_sending(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        let run_ms = self.run_ms(now);
        let sending = self
            .air
            .as_ref()
            .is_some_and(|air| self.yours_to_go(air, run_ms));
        if !sending {
            return Vec::new();
        }
        let Some(mut air) = self.air.take() else {
            return Vec::new();
        };
        let mut effects = vec![ContestEffect::StopAudio];
        let unheard = air.unheard.take();
        // Where the message keyed last starts: parts carried on from an
        // earlier one were heard at its key, and are not taken back.
        let keyed_from = unheard.as_ref().map_or(0, |unheard| unheard.from);
        let parts: Vec<MessagePart> = air.parts.iter().map(|(part, _)| part.clone()).collect();
        let sent_by = air.sent_by(run_ms);
        let carried = self.cut(air, run_ms, false, &mut effects);
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
        let garbled = if sent < parts.len() {
            self.garbled(&carried.callers)
        } else {
            Vec::new()
        };
        self.start_waits(run_ms, &sent_by);
        let going_on = Carried {
            callers: carried.callers,
            qrm: carried.qrm,
            ..Carried::default()
        };
        if garbled.is_empty() && going_on.callers.is_empty() && going_on.qrm.is_none() {
            effects.extend(self.rearm_patience(run_ms));
        } else {
            effects.extend(self.play(going_on, None, Vec::new(), garbled, run_ms, now));
        }
        effects
    }

    /// Whether some of your message in `air` is still to go out at
    /// `run_now`: a character of it sounding then, or yet to. None of a
    /// turn of callers alone is.
    fn yours_to_go(&self, air: &Air, run_now: u64) -> bool {
        let Some(yours) = air
            .turn
            .segments
            .iter()
            .find(|segment| segment.who == Sender::You)
        else {
            return false;
        };
        let spans = char_spans(&yours.text, &self.settings, &yours.voice);
        let at_sec = (run_now as f64 - air.heard_run() as f64) / 1_000.0 - yours.start_sec;
        sounding_at(&spans, at_sec).is_some()
    }

    /// Esc cut your message short: each caller on the frequency that was
    /// listening, not `sounding` still, nor waiting out your QSO with
    /// someone else, copies what it heard as garbage, as MorseRunner's
    /// callers copy an aborted send (`MyStn.pas:108-116` makes it
    /// `msgGarbage` and says the send is over), and answers it a moment
    /// after (`DxOper.pas:805-817`, `DxStn.pas:240-275`): a caller takes
    /// more patience, spends a try and calls again, as when you key over
    /// it, and the station you work sends again what it owes. Nobody asked
    /// for it, so it costs only time.
    fn garbled(&mut self, sounding: &[Overlapping]) -> Vec<Replying> {
        let listening: Vec<usize> = (0..self.stations.len())
            .filter(|index| !sounding.iter().any(|over| over.reply.station == *index))
            .filter(|index| {
                let caller = &self.stations[*index].caller;
                caller.state.active() && !caller.waits()
            })
            .collect();
        listening
            .into_iter()
            .filter(|station| self.stations[*station].caller.heard_garbage())
            .map(Replying::unasked)
            .collect()
    }

    /// The callers as `unheard` holds them, before they heard a message Esc
    /// stopped, then hearing only the parts of it that went out whole,
    /// `sent`. What reached you since of the stations on the frequency at
    /// the key, from a caller you keyed over that went on over your message,
    /// stays heard, and so does what it told Enter: that the station you
    /// work took the call, or corrected it. A dupe the message brought back
    /// goes back as it was, whole: what it had of you is its old QSO's.
    fn unhear(&mut self, unheard: Unheard, sent: &[MessagePart], run_ms: u64) {
        self.entries.truncate(unheard.lost_from);
        self.taught = self.taught.min(self.entries.len());
        self.stations.truncate(unheard.count);
        for (index, mut station) in unheard.stations {
            let there = station.caller.state.active() || unheard.thanked == Some(index);
            if there {
                station.heard.keep_heard(&self.stations[index].heard);
            }
            self.stations[index] = station;
        }
        self.working = unheard.working;
        self.thanked = unheard.thanked;
        let notes = unheard.qso.is_some();
        if let Some(qso) = unheard.qso {
            let (taken, corrected) = (self.qso.call_taken, self.qso.corrected);
            self.qso = QsoState {
                call_taken: qso.call_taken || taken,
                corrected: qso.corrected || corrected,
                ..qso
            };
            self.note_sent(sent);
        }
        // Esc takes back only a message that logged nothing.
        let (_, took) = self.hear(sent, &unheard.fields, &unheard.call, &unheard.deaf, false);
        self.went_on(unheard.working, took, &unheard.call.typed, notes);
        self.thank(unheard.working, sent, &unheard.fields, notes);
        self.close_finished(run_ms);
    }

    fn turn_ended(&mut self, now: MonoMs) -> Vec<ContestEffect> {
        let Some(mut air) = self.air.take() else {
            return Vec::new();
        };
        let run_ms = self.run_ms(now);
        let mut effects = Vec::new();
        // All of it counts, and the checks still waiting on it show.
        self.count_heard(&mut air, None, run_ms, &mut effects);
        // What Show copy shows of it stays till the next turn.
        self.shown_last = self.shown_of(&air);
        // Each caller waits as the turn planned it, from the end of its
        // last send in it, or of your message, as its sound reached you.
        let heard_run = air.heard_run();
        for (station, after) in air.waits {
            self.stations[station].caller.deadline = Some(heard_run + after);
        }
        effects.extend(self.arm_patience(run_ms, &[]));
        effects
    }

    /// Everyone still on the frequency waits for you, each its own time,
    /// and the timer runs to the first of them to give up waiting
    /// ([`Self::start_waits`]).
    fn arm_patience(&mut self, run_ms: u64, sent: &[(usize, u64)]) -> Option<ContestEffect> {
        self.start_waits(run_ms, sent);
        self.rearm_patience(run_ms)
    }

    /// Everyone still on the frequency waits for you, each its own time, as
    /// MorseRunner's callers do (`DxStn.pas:204-208`): one that sent, as
    /// `sent` has it, from the end of its send; one that heard you and has
    /// no wait running, from now; any other keeps the wait it had, as
    /// another caller's send is nothing to it. A pile-up caller that heard
    /// you work someone else waits for that QSO to end, with no timeout,
    /// and so does the station you work once it stays silent for your
    /// exchange.
    fn start_waits(&mut self, run_ms: u64, sent: &[(usize, u64)]) {
        for (index, station) in self.stations.iter_mut().enumerate() {
            let caller = &mut station.caller;
            if !caller.times_out() {
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
            let who = self.station_gone(index);
            effects.push(ContestEffect::Announce(format!("{who} has gone")));
        }
        if replying.is_empty() {
            effects.extend(self.rearm_patience(run_ms));
        } else {
            effects.extend(self.play(Carried::default(), None, replying, Vec::new(), run_ms, now));
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
        self.play(Carried::default(), None, replying, Vec::new(), run_ms, now)
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
        let taken = self.taken(sounding);
        let placed = place_contest_caller(&self.settings, self.style, &taken, &mut self.draws)?;
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
        let taken = self.taken(sounding);
        let placed = place_contest_caller(&self.settings, self.style, &taken, &mut rng)?;
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
            .next_call(&self.settings, self.rules.callers(self.side), &avoid);
        let mut caller = Caller::arrive(
            call,
            &self.settings,
            self.rules,
            self.settings.contest.limit_minutes(),
            self.style,
            placed,
            rng,
        );
        // A newcomer's speed, where the callers' speed follows your copy;
        // those already calling keep theirs.
        if let Some(speed) = self.source.next_speed(&self.settings) {
            caller.send_at(speed);
        }
        self.stations.push(Station {
            caller,
            heard: Heard::default(),
        });
        Some(index)
    }

    /// A station you worked that is done or gone without its QSO in the log
    /// is a lost copy, but for one your TU closed until you work another
    /// (not one that only answered your partial, in a pile-up); the one you
    /// work, once it leaves, is no longer it, and its QSO is over.
    fn close_finished(&mut self, run_ms: u64) {
        if self
            .qso_station()
            .is_some_and(|station| Some(station) != self.thanked)
        {
            self.thanked = None;
        }
        for index in 0..self.stations.len() {
            let station = &self.stations[index];
            if station.caller.state.active() {
                continue;
            }
            let engaged = station.heard.engaged;
            if engaged && !station.heard.recorded && self.thanked != Some(index) {
                self.lose(index, run_ms);
            }
            if self.working == Some(index) {
                self.working = None;
                if engaged {
                    self.next_round(Some(index));
                }
            }
        }
    }

    /// The QSO with station `with`, or a QSO logged with nobody, is over: in
    /// a pile-up the callers still calling forget the repeats of their
    /// calls you asked for before it, as the call each sends after it is
    /// no repeat you asked for, and is the one you copy. A `?` to the whole
    /// pile-up is charged to the QSO it was asked in, not to every one
    /// after.
    fn next_round(&mut self, with: Option<usize>) {
        if self.style != RunStyle::Pileup {
            return;
        }
        for (index, station) in self.stations.iter_mut().enumerate() {
            let calling = station.caller.state == CallerState::NeedQso;
            if calling && Some(index) != with && station.heard.first_call.is_none() {
                station.heard.call_asks.clear();
            }
        }
    }

    /// Log the QSO in the fields at `run_ms`, checked against the station
    /// you work, or else the one your TU closed, if the call is its own or
    /// close to it; otherwise not in its log. Returns its entry.
    fn log(&mut self, fields: &Fields, call: &CallRead, run_ms: u64, now: MonoMs) -> usize {
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
            .qso_station()
            .filter(theirs)
            .or_else(|| self.thanked.filter(theirs));
        // Another QSO logged: the one your TU closed is lost.
        if let Some(thanked) = self.thanked.take().filter(|index| worked != Some(*index))
            && !self.stations[thanked].heard.recorded
        {
            self.lose(thanked, run_ms);
        }
        let done_run = fields
            .exch_complete
            .map_or(run_ms, |at| self.run_at(at, now));
        let exch = reading
            .fields
            .iter()
            .map(|read| {
                let keys = fields
                    .exch_word_keys(read.word)
                    .map(|key| self.run_at(key, now));
                let text = read.token.clone();
                (read.field, Typed { text, keys })
            })
            .collect();
        let read = LogRead {
            logged,
            reading,
            exch,
            call: call.clone(),
            done_run,
        };
        let entry = match worked {
            Some(index) => self.worked_entry(index, &read, run_ms),
            None => self.nil_entry(&read, run_ms),
        };
        let with = entry.station;
        self.entries.push(entry);
        // A QSO logged is final, and so is every one before it.
        self.teach();
        self.serial += 1;
        self.end_qso();
        self.working = None;
        self.next_round(with);
        self.entries.len() - 1
    }

    /// Teach the call source every group of the QSOs that are final: all
    /// but those the message on the air lost, which Esc can take back. Its
    /// calls and numbers teach it your copy speed too, as the copy-speed
    /// series will read them once the run is stored, so the callers who
    /// come next are drawn from what the run has shown so far.
    fn teach(&mut self) {
        let upto = match self.air.as_ref().and_then(|air| air.unheard.as_ref()) {
            Some(unheard) if unheard.entries == self.entries.len() => unheard.lost_from,
            _ => self.entries.len(),
        };
        let copied = self.rules.copied(self.side);
        let source = &mut self.source;
        for entry in self.entries.get(self.taught..upto).unwrap_or_default() {
            let copies = entry.record.group_copies(|at| copied.get(at).copied());
            for (kept, copy) in entry.groups.iter().zip(copies) {
                learn(source, &kept.group, &kept.timing, tells_speed(copy));
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
    fn worked_entry(&mut self, index: usize, read: &LogRead, run_ms: u64) -> Entry {
        let (logged, reading) = (&read.logged, &read.reading);
        let clean = self.logged_clean();
        let nr = self.serial_sent();
        let (rules, side) = (self.rules, self.side);
        let copied = rules.copied(side);
        self.settle_frozen(index);
        let station = &mut self.stations[index];
        let again = station.heard.recorded;
        station.heard.recorded = true;
        let sent = QsoText {
            call: station.caller.call.text.clone(),
            fields: station.caller.exchange(rules, side, run_ms).to_vec(),
        };
        let check = check_qso(rules, logged, Some(&sent), |call| {
            clean.iter().any(|c| c == call)
        });
        let heard = &station.heard;
        let (call_voice, exch_voice) = heard.voices(station.caller.voice);
        // Never sent with your exchange: the call as logged, timed as last
        // sent, or as typed for the log.
        let (first_call, call_ms) = heard.first_call.clone().unwrap_or_else(|| {
            let ms = heard
                .call_ms
                .unwrap_or_else(|| copy_ms(&heard.call_ends, read.call.edited_run));
            (read.call.typed.as_copy(&logged.call), ms)
        });
        let call = heard.call_group(&sent.call, &first_call, call_ms, &call_voice);
        let call_copy = call.group.received.clone();
        let mut groups = Vec::new();
        if !again {
            groups.push(call);
            groups.extend(heard.field_groups(rules, &sent.fields, read, &exch_voice));
        }
        // The field groups hold the exchange as logged, unless Show copy
        // kept them as they read when its text showed: then the record
        // holds what was logged, and what was sent beside it, for the check.
        let copies_fields = !again && heard.exch_begun && heard.exch_frozen.is_none();
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
        let lc = if !again && logged.call == call_copy {
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
                // A station logged again shows no speed: its copy, and the
                // speed that is credited at, are the earlier QSO's.
                wpm: (!again).then(|| row_wpm(call_voice, exch_voice)),
                asked: heard.asked(),
                check,
                pending: true,
            }),
            groups,
            station: Some(index),
        }
    }

    /// The station a NIL logged as `call` is against: the one that took
    /// that call from you for its own; else the heard station whose call it
    /// is, or is close to, whoever you work now, as a QSO logged for a
    /// station you left, a slip and all, is no copy of the next; else the
    /// one you work ([`Self::qso_station`]), whose own call [`Self::log`]
    /// found it is not; else, in a run of single callers, the single
    /// caller: the one on the frequency, or else the newest heard, never an
    /// older one. None when the first of these found has its copy kept
    /// already, or no station was heard; in a pile-up, a call like nobody's
    /// is charged to nobody, as the newest caller is no likelier its
    /// station than any other.
    fn nil_target(&self, call: &str) -> Option<usize> {
        let fresh = |index: &usize| !self.stations[*index].heard.recorded;
        let heard: Vec<usize> = (0..self.stations.len())
            .filter(|index| self.stations[*index].heard.first.is_some())
            .collect();
        let took = |index: &usize| {
            let heard = &self.stations[*index].heard;
            heard.call_sent == call
                || heard
                    .first_call
                    .as_ref()
                    .is_some_and(|(first, _)| first.text == call)
                || (heard.engaged && self.stations[*index].caller.call.text == call)
        };
        if let Some(owner) = (0..self.stations.len()).find(took) {
            return Some(owner).filter(fresh);
        }
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
        if let Some((_, fresh, index)) = best {
            return fresh.then_some(index);
        }
        if let Some(station) = self.qso_station() {
            return Some(station).filter(fresh);
        }
        match self.style {
            // A dupe calls again at its old place: the newest heard is the
            // one heard last, not the one that joined last.
            RunStyle::Single => {
                let on_frequency = (0..self.stations.len())
                    .find(|index| self.stations[*index].caller.state.active());
                let newest = heard
                    .iter()
                    .copied()
                    .max_by_key(|index| self.stations[*index].heard.first);
                on_frequency
                    .or(newest)
                    .filter(|index| heard.contains(index))
                    .filter(fresh)
            }
            RunStyle::Pileup => None,
        }
    }

    /// A QSO logged with nobody matching worked: not in their log. The
    /// station [`Self::nil_target`] finds owns its call group, as copied by
    /// the call logged, and a group for each field as a QSO logged with it
    /// has them: as keyed once its exchange has begun to reach you, and a
    /// miss before that. With none, `lc` holds the call.
    fn nil_entry(&mut self, read: &LogRead, run_ms: u64) -> Entry {
        let (logged, reading) = (&read.logged, &read.reading);
        let check = check_qso(self.rules, logged, None, |_| false);
        let against = self.nil_target(&logged.call);
        let nr = self.serial_sent();
        let (rules, side) = (self.rules, self.side);
        let (groups, first, wpm) = match against {
            Some(index) => {
                self.settle_frozen(index);
                let station = &mut self.stations[index];
                station.heard.recorded = true;
                let exchange = station.caller.exchange(rules, side, run_ms).to_vec();
                let heard = &station.heard;
                let took = copy_ms(&heard.call_ends, read.call.edited_run);
                let (call_voice, exch_voice) = heard.voices(station.caller.voice);
                let copy = read.call.typed.as_copy(&logged.call);
                let mut groups =
                    vec![heard.call_group(&station.caller.call.text, &copy, took, &call_voice)];
                groups.extend(heard.field_groups(rules, &exchange, read, &exch_voice));
                (groups, heard.first, Some(row_wpm(call_voice, exch_voice)))
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
                // With no group to hold it, the record does, and so it does
                // where Show copy kept another copy in its call group.
                lc: if groups
                    .first()
                    .is_none_or(|call| call.group.received != logged.call)
                {
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
    /// else as the Call field last read while you worked it
    /// ([`Self::note_typed`]), timed as last sent; and a miss for each
    /// field, heard or not, as a QSO logged before its exchange came has.
    /// Walking away pays no better than finishing, and hides no copy.
    fn lose(&mut self, index: usize, run_ms: u64) {
        let nr = self.serial_sent();
        let (rules, side) = (self.rules, self.side);
        self.settle_frozen(index);
        let station = &mut self.stations[index];
        station.heard.recorded = true;
        let exchange = station.caller.exchange(rules, side, run_ms).to_vec();
        let heard = &station.heard;
        let (received, call_ms) = match &heard.first_call {
            Some((call, ms)) => (call, *ms),
            None => (&heard.typed, heard.call_ms.unwrap_or(0)),
        };
        let (call_voice, exch_voice) = heard.voices(station.caller.voice);
        let mut groups =
            vec![heard.call_group(&station.caller.call.text, received, call_ms, &call_voice)];
        groups.extend(misses(&exchange, &exch_voice));
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
        // What Show copy was to show of it never showed, as nothing of it
        // was heard: no copy's cut-off, the station's next send shows it.
        let run_ms = self.run_ms(now);
        self.give_way(run_ms);
        // Nothing of it was heard: every caller in it sends again, as it
        // was keyed, so a repeat you asked for is charged once you hear it;
        // as no send you asked for if a key that logged counted it already.
        // A call on its timeout that never began waits afresh, as everyone
        // does after Resume.
        let again = air
            .replies
            .iter()
            .zip(&air.counted)
            .filter_map(|((reply, _), counted)| match counted {
                Counted::Nothing if reply.on_time_out => None,
                Counted::Nothing => Some(reply.clone()),
                Counted::CallBegun { .. } | Counted::Begun | Counted::Whole | Counted::Muted => {
                    Some(Replying::unasked(reply.station))
                }
            });
        self.cut_short.extend(again);
        // Nor did anything it planned go out; a send carried on from a turn
        // keyed over began there.
        for (reply, says) in air.replies.iter().skip(air.carried) {
            self.stations[reply.station].caller.unsent(*says);
        }
        let mut effects = Vec::new();
        // Nothing of a send that failed was heard; the QSOs it logged stand.
        self.resolve(&air.pending, &mut effects);
        self.hold(PauseWhy::Sound, run_ms);
        if self.time_up(now) {
            effects.extend(self.finish(now));
            return effects;
        }
        effects.insert(0, ContestEffect::ReleaseAudio);
        effects.push(ContestEffect::Announce(
            "Sound stopped — Resume to carry on".to_string(),
        ));
        effects
    }

    /// The run pauses for `why`, its clock held at run time `run_ms` and the
    /// turn that was on the air dealt with: nothing heard lies after it, a
    /// key after it says nothing of a sound heard before it ([`Sends`]),
    /// and no timer runs. Pause and a failed sound both come here.
    fn hold(&mut self, why: PauseWhy, run_ms: u64) {
        for station in &mut self.stations {
            let heard = &mut station.heard;
            heard.forget_after(run_ms);
            heard.call_sends.paused(run_ms);
            heard.exch_sends.paused(run_ms);
        }
        self.timers = Timers::default();
        self.phase = ContestPhase::Paused { why };
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
                // What Show copy showed of the turn last on the air gives
                // way to the pause, the air quiet or not.
                self.give_way(run_ms);
                let mut effects = vec![ContestEffect::StopAudio, ContestEffect::ReleaseAudio];
                // A pause with nothing on the air, in the moment after
                // Resume, keeps who the last one cut short.
                if let Some(air) = self.air.take() {
                    self.cut_short.extend(air.cut_short(run_ms, why));
                    let carried = self.cut(air, run_ms, false, &mut effects);
                    self.resolve(&carried.pending, &mut effects);
                }
                self.hold(why, run_ms);
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
            let carried = self.cut(air, run_ms, false, &mut effects);
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
    timed: Option<Timed>,
) -> Kept {
    let floor = MIN_RESPONSE_MS_PER_CHAR * sent.chars().count().max(1) as f64;
    let (heard_in, char_times) = timed.map(|timed| (timed.turn, timed.times)).unzip();
    Kept {
        group: GroupResult {
            sent: sent.to_string(),
            received: received.to_string(),
            correct: sent == received,
        },
        timing: SessionTiming {
            time_to_complete_ms: (ms as f64).max(floor),
            char_wpm: Some(tenth(voice.char_wpm)),
            effective_wpm: Some(tenth(voice.effective_wpm)),
            sends,
            char_times,
        },
        heard_in,
    }
}

/// Whether `voice` sends slower than `than`.
fn slower(voice: StationVoice, than: StationVoice) -> bool {
    (voice.char_wpm, voice.effective_wpm) < (than.char_wpm, than.effective_wpm)
}

/// A log row's speed, to 0.1 WPM: the slower of what its call and its
/// exchange are credited at.
fn row_wpm(call: StationVoice, exch: StationVoice) -> f64 {
    tenth(if slower(exch, call) { exch } else { call }.char_wpm)
}

/// A missed group for each field of `exchange`, sent at `voice`: what a
/// station's copy holds of an exchange logged before it was heard, as a
/// guess is no copy, or never logged at all, as walking away is none.
fn misses(exchange: &[(Field, String)], voice: &StationVoice) -> Vec<Kept> {
    exchange
        .iter()
        .map(|(_, token)| group(token, "", 0, 1, voice, None))
        .collect()
}

/// Where each character of `token` ends in segment `at` of `turn`, whose
/// characters sound at `spans`, in milliseconds after the segment starts,
/// when it is there once as a word within the bytes `within` of its text,
/// and nothing else in the turn, a caller, QRM or your own message, sounds
/// while it does: a token heard twice in one send, or under another, is no
/// measure of hearing it.
fn sounded_alone(
    turn: &Turn,
    at: usize,
    spans: &[CharSpan],
    within: std::ops::Range<usize>,
    token: &str,
) -> Option<Vec<u32>> {
    let segment = turn.segments.get(at)?;
    let part = segment.text.get(within.clone())?;
    let word = |(from, _): &(usize, &str)| {
        let before = part[..*from].chars().next_back();
        let after = part[from + token.len()..].chars().next();
        before.is_none_or(char::is_whitespace) && after.is_none_or(char::is_whitespace)
    };
    let mut found = part.match_indices(token).filter(word);
    let (from, _) = found.next().filter(|_| !token.is_empty())?;
    if found.next().is_some() {
        return None;
    }
    let bytes = within.start + from..within.start + from + token.len();
    let spans: Vec<&CharSpan> = spans
        .iter()
        .filter(|span| bytes.contains(&span.at))
        .collect();
    let (first, last) = (spans.first()?, spans.last()?);
    if spans.len() != token.chars().count() {
        return None;
    }
    let (begins, ends) = (
        segment.start_sec + first.start_sec,
        segment.start_sec + last.end_sec,
    );
    let alone = turn
        .segments
        .iter()
        .enumerate()
        .all(|(other, over)| other == at || over.end_sec <= begins || over.start_sec >= ends);
    alone.then(|| {
        spans
            .iter()
            .map(|span| u32::try_from(whole_ms(span.end_sec)).unwrap_or(u32::MAX))
            .collect()
    })
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

/// Station `index` as the sender of its segments.
fn sender(index: usize) -> Sender {
    Sender::Caller(u32::try_from(index).unwrap_or(u32::MAX))
}

/// When the checks of the rows waiting on a turn show, in seconds into
/// it: once the last TU among your message's `parts` has gone out, by when
/// each of `my_chars` stops sounding, and every sign-off among the callers'
/// `segments`, as `replies` has them, has ended. With no TU, at
/// `your_end`.
fn checks_at_sec(
    parts: &[(MessagePart, usize)],
    my_chars: &[(usize, f64)],
    your_end: f64,
    segments: &[Segment],
    replies: &[(Replying, Says)],
) -> f64 {
    let tu_end = parts
        .iter()
        .filter(|(part, _)| *part == MessagePart::Tu)
        .map(|(_, before)| *before)
        .max()
        .and_then(|tu_before| {
            my_chars
                .iter()
                .filter(|(at, _)| *at < tu_before)
                .map(|(_, end)| *end)
                .reduce(f64::max)
        })
        .unwrap_or(your_end);
    segments
        .iter()
        .filter(|segment| segment.who.is_caller())
        .zip(replies)
        .filter(|(_, (_, says))| *says == Says::Signoff)
        .map(|(segment, _)| segment.end_sec)
        .fold(tu_end, f64::max)
}

/// Each field `reading` reads, with its value: what the log shows.
fn values(reading: &Reading) -> Vec<(Field, String)> {
    reading
        .fields
        .iter()
        .map(|read| (read.field, read.value.clone()))
        .collect()
}

/// Teach `source` what was copied of a group, and, where it tells, how fast
/// you copy.
fn learn(source: &mut impl CallSource, group: &GroupResult, timing: &SessionTiming, speed: bool) {
    let copied = CopyTally::of(&group.sent, &group.received);
    let answered = Answered {
        sent: &group.sent,
        received: &group.received,
        copied: &copied,
        played: timing.char_wpm.zip(timing.effective_wpm),
    };
    source.learn(answered);
    if speed {
        source.learn_speed(answered);
    }
}

/// To a tenth.
fn tenth(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::QsoError;
    use crate::contest::bot::script::{
        Habit, ScriptCalls, default_run_settings, run_settings, work,
    };
    use crate::contest::bot::{Copied, Operator, Runtime, copy_of};
    use crate::contest::exchange::read_number;
    use crate::contest::rules::RULES;
    use crate::contest::turn::{plan_turn, sending_sec};
    use crate::sampling::Sampler;
    use crate::timing::ToneEvent;

    /// A run worked by hand, its callers sending `calls` in turn.
    fn runtime(calls: &[&str]) -> Runtime<ScriptCalls> {
        Runtime::start(1, run_settings(25.0), ScriptCalls::new(calls))
    }

    /// What callers send in `turn`.
    fn replies(turn: &Turn) -> Vec<&str> {
        turn.segments
            .iter()
            .filter(|segment| segment.who.is_caller())
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

    /// A run worked by hand, its callers sending `calls` in turn, on the
    /// first seed whose first caller puts your call right with its
    /// exchange, once it has yours, or without, as `with_exchange` says.
    fn runtime_fixing(calls: &[&str], with_exchange: bool) -> Runtime<ScriptCalls> {
        (1..100)
            .map(|seed| Runtime::start(seed, run_settings(25.0), ScriptCalls::new(calls)))
            .find(|runtime| {
                runtime.machine.stations[0].caller.fixes_with_exchange() == with_exchange
            })
            .expect("an operator of each kind")
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
        assert_eq!(yours(&tu), "TU DL1ABC");
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

    /// In a Simple QSO a logged QSO's check waits for the station's sign-off
    /// after your TU: the row reads `…` while its `73 EE` sounds, and is
    /// checked once that has reached you.
    #[test]
    fn a_simple_qsos_check_shows_once_the_station_has_signed_off() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_kind = crate::contest::ContestKind::SimpleQso;
        let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
        let exchange = to_its_exchange(&mut runtime);
        let words: Vec<&str> = exchange.split_whitespace().collect();
        let at = runtime.now;
        runtime.key(OpKey::Esm, "K5XYZ", &format!("{} {}", words[3], words[6]));
        let signoff = sent_by(runtime.last_turn(), 0)[0].clone();
        assert_eq!(signoff.text, "73 EE");
        runtime.run_to(at + ms(signoff.start_sec + 0.1));
        assert_eq!(view(&runtime).log[0].status, RowStatus::Pending, "TU out");
        runtime.run_to(at + ms(signoff.end_sec) + 1);
        assert_eq!(
            view(&runtime).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );
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
    /// right. A station that sends its exchange with the correction is
    /// logged with `K5XYZ TU`, its exchange a copy, timed from the
    /// correction's end as one sent alone is from its own.
    #[test]
    fn a_corrected_call_keeps_the_first_call_as_its_copy() {
        // When the station's first send in the last turn ended, in run time.
        let ended = |turn: &Turn| turn.at_run_ms + whole_ms(sent_by(turn, 0)[0].end_sec);
        for with_exchange in [false, true] {
            let mut runtime = runtime_fixing(&["K5XYZ"], with_exchange);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYW", "");
            let correction = replies(runtime.last_turn())[0].to_string();
            let carried = match copy_of(&correction) {
                Some(Copied::Correction("K5XYZ", carried)) => carried,
                other => panic!("{correction}: {other:?}"),
            };
            assert_eq!(carried.is_some(), with_exchange, "{correction}");
            assert!(!view(&runtime).qso.corrected, "not heard yet");
            runtime.finish_turn();
            assert!(view(&runtime).qso.corrected);
            let (nr, heard_at) = match carried {
                Some(nr) => (nr, ended(runtime.last_turn())),
                None => {
                    runtime.key(OpKey::Esm, "K5XYZ", "");
                    assert_eq!(yours(runtime.last_turn()), "K5XYZ ?");
                    assert!(!view(&runtime).qso.corrected);
                    let exchange = replies(runtime.last_turn())[0].to_string();
                    runtime.finish_turn();
                    (serial(&exchange), ended(runtime.last_turn()))
                }
            };
            // The number takes two seconds to type once its turn is over.
            runtime.run_to(runtime.now + 2_000);
            let typed_at = view(&runtime).run_ms(MonoMs(runtime.now));
            runtime.key(OpKey::Esm, "K5XYZ", &nr);
            if with_exchange {
                assert_eq!(yours(runtime.last_turn()), "K5XYZ TU DL1ABC");
            }
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, run) = result(&runtime);
            assert_eq!(
                copies(&result),
                [("K5XYZ", "K5XYW"), (nr.as_str(), nr.as_str())]
            );
            assert_eq!(sends(&runtime), [1, 1], "{correction}");
            assert_eq!(
                result.group_timings[1].time_to_complete_ms,
                (typed_at - heard_at) as f64,
                "{correction}"
            );
            assert_eq!(run.qsos[0].lc, "K5XYZ");
            assert_eq!(view(&runtime).clean, 1, "logged right, it is clean");
            let score = run.score(&result.groups).expect("WPX scores");
            assert_eq!(
                score.verified.qso_points, 1,
                "the stored run scores as it ran"
            );
        }
    }

    /// F5 then F2 while your call is still going out: both go, your call
    /// going on from the character after the one sounding, which the turn
    /// on the air finishes, and the station answers the whole with its
    /// exchange.
    #[test]
    fn his_call_then_your_exchange_during_your_call_sends_both() {
        for (after_ms, rest) in [(100, "5XYZ 5NN TT1"), (700, "XYZ 5NN TT1")] {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::HisCall, "K5XYZ", "");
            assert_eq!(yours(runtime.last_turn()), "K5XYZ");
            let first = runtime.last_turn().id;
            runtime.now += after_ms;
            runtime.key(OpKey::Exchange, "K5XYZ", "");
            let turn = runtime.last_turn().clone();
            assert_ne!(turn.id, first);
            assert!(turn.starts_at.is_some(), "spliced on");
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

    /// However late in your call the next key comes, what the turn on the
    /// air sends of the call and what the continuation sends together hold
    /// every character once: the one sounding at the key goes out whole
    /// before the splice, as does one starting before the turn on the air
    /// could be ridden down, and the rest goes on from the next at the very
    /// moment it would have started, no character sent twice and no gap.
    /// With no character left to start so, the continuation takes over half
    /// a word space after the call, ridden down in silence, and your
    /// exchange starts a word space after it; once the call has gone out,
    /// the exchange goes at once.
    #[test]
    fn a_continuation_keeps_every_character_wherever_it_is_keyed() {
        let settings = run_settings(25.0);
        let voice = your_voice(&settings, 25.0);
        let spans = char_spans("K5XYZ", &settings, &voice);
        let call_sec = sending_sec("K5XYZ", &settings, &voice);
        let call_ms = (call_sec * 1_000.0) as u64;
        let gap = word_gap_sec(&settings, &voice);
        for after_ms in (0..call_ms + 100).step_by(10) {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::HisCall, "K5XYZ", "");
            let call = runtime.last_turn().clone();
            runtime.now += after_ms;
            runtime.key(OpKey::Exchange, "K5XYZ", "");
            let turn = runtime.last_turn().clone();
            let at_sec = after_ms as f64 / 1_000.0;
            // The runtime's sound is heard at once, so the continuation can
            // take over a release after the key.
            let soonest = at_sec + SEND_RELEASE_SEC;
            // What the turn on the air sends of the call: every character
            // begun before the continuation can take over, to its end.
            let before: String = spans
                .iter()
                .filter(|span| span.start_sec < soonest)
                .map(|span| &"K5XYZ"[span.at..=span.at])
                .collect();
            let again = yours(&turn).replace(' ', "");
            assert_eq!(
                format!("{before}{again}"),
                "K5XYZ5NNTT1",
                "{after_ms} ms in"
            );
            let mine = turn
                .segments
                .iter()
                .find(|segment| segment.who == Sender::You)
                .expect("your message");
            match spans
                .iter()
                .find(|span| span.start_sec >= soonest)
                .map(|span| span.start_sec)
            {
                // The rest goes on where its first character starts.
                Some(start) => {
                    assert_eq!(turn.at_run_ms, call.at_run_ms + whole_ms(start));
                    assert_eq!(turn.starts_at, Some(MonoMs(turn.at_run_ms)));
                    assert_eq!(mine.start_sec, 0.0);
                }
                // The call finished by the turn on the air: half a word
                // space after it, your exchange a word space after the call.
                None if at_sec < call_sec => {
                    let half = call.at_run_ms + whole_ms(call_sec + gap / 2.0);
                    assert_eq!(turn.at_run_ms, half, "{after_ms} ms in");
                    assert!(turn.starts_at.is_some());
                    let begins = turn.at_run_ms as f64 / 1_000.0 + mine.start_sec;
                    let meant = call.at_run_ms as f64 / 1_000.0 + call_sec + gap;
                    assert!((begins - meant).abs() < 1e-9, "{begins} for {meant}");
                }
                None => {
                    assert_eq!(turn.at_run_ms, runtime.now, "at once");
                    assert_eq!(turn.starts_at, None);
                    assert_eq!(mine.start_sec, 0.0);
                }
            }
        }
    }

    /// A turn started at once says how far behind the key the player's
    /// sound is, and a key mid-message splices its continuation on no
    /// sooner than that and a release after it: the turn on the air plays
    /// every character starting before then, which the player has already
    /// begun to play, and the rest goes on from the first that starts
    /// later. With the call all but out by then, the continuation takes
    /// over that late, and your exchange starts a word space after the
    /// call, or as soon as the player can start it. A retry's start says
    /// nothing of the lead.
    #[test]
    fn a_splice_waits_out_the_players_lead() {
        let settings = run_settings(25.0);
        let voice = your_voice(&settings, 25.0);
        let spans = char_spans("K5XYZ", &settings, &voice);
        let call_sec = sending_sec("K5XYZ", &settings, &voice);
        let gap = word_gap_sec(&settings, &voice);
        let lead_ms = 150;
        let fields = |at: u64| Fields {
            call: "K5XYZ".to_string(),
            exch: String::new(),
            call_edited: MonoMs(at),
            exch_complete: None,
            ..Fields::default()
        };
        for after_ms in (0..(call_sec * 1_000.0) as u64).step_by(40) {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            let machine = &mut runtime.machine;
            let keyed = runtime.now;
            let effects = machine.apply(
                ContestEvent::Key {
                    key: OpKey::HisCall,
                    fields: Box::new(fields(keyed)),
                },
                MonoMs(keyed),
            );
            let call = played(&effects).expect("your call").clone();
            assert_eq!(call.starts_at, None);
            let heard_from = MonoMs(keyed + lead_ms);
            let started = ContestEvent::TurnStarted {
                id: call.id,
                heard_from,
                output_from: heard_from,
            };
            assert!(machine.apply(started, MonoMs(keyed)).is_empty());
            assert_eq!(machine.lead_ms, lead_ms);
            let at = keyed + lead_ms + after_ms;
            let effects = machine.apply(
                ContestEvent::Key {
                    key: OpKey::Exchange,
                    fields: Box::new(fields(at)),
                },
                MonoMs(at),
            );
            let turn = played(&effects).expect("your exchange").clone();
            let starts = turn.starts_at.expect("spliced on").0;
            assert!(
                starts >= at + lead_ms + whole_ms(SEND_RELEASE_SEC),
                "{after_ms} ms in: starts {} ms after the key",
                starts - at
            );
            // Where the call is, as the player can still take over from it.
            let soonest = (after_ms + lead_ms) as f64 / 1_000.0 + SEND_RELEASE_SEC;
            let before: String = spans
                .iter()
                .filter(|span| span.start_sec < soonest)
                .map(|span| &"K5XYZ"[span.at..=span.at])
                .collect();
            assert_eq!(
                format!("{before}{}", yours(&turn).replace(' ', "")),
                "K5XYZ5NNTT1",
                "{after_ms} ms in"
            );
            let heard = heard_from.0 as f64 / 1_000.0;
            match spans
                .iter()
                .find(|span| span.start_sec >= soonest)
                .map(|span| span.start_sec)
            {
                Some(start) => {
                    assert_eq!(starts, heard_from.0 + whole_ms(start), "{after_ms} ms in");
                }
                None => {
                    let mine = turn
                        .segments
                        .iter()
                        .find(|segment| segment.who == Sender::You)
                        .expect("your exchange");
                    let begins = starts as f64 / 1_000.0 + mine.start_sec;
                    let meant = (heard + call_sec + gap).max(starts as f64 / 1_000.0);
                    assert!((begins - meant).abs() < 1e-9, "{begins} for {meant}");
                }
            }
            // Its start says nothing of the lead, being put off on purpose,
            // and nor does a retry's, later still.
            let started = ContestEvent::TurnStarted {
                id: turn.id,
                heard_from: MonoMs(starts + 40),
                output_from: MonoMs(starts + 40),
            };
            assert!(machine.apply(started, MonoMs(at)).is_empty());
            assert_eq!(machine.lead_ms, lead_ms);
        }
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let keyed = runtime.now;
        let effects = runtime.key(OpKey::HisCall, "K5XYZ", "");
        let id = played(&effects).expect("your call").id;
        // Its first start, at once; then a retry's.
        runtime.run_to(keyed);
        let retried = ContestEvent::TurnStarted {
            id,
            heard_from: MonoMs(keyed + 2_000),
            output_from: MonoMs(keyed + 2_000),
        };
        assert!(
            runtime
                .machine
                .apply(retried, MonoMs(keyed + 1_000))
                .is_empty()
        );
        assert_eq!(runtime.machine.lead_ms, 0, "a retry");
        // A first start held up for seconds is not all lead.
        let mut held = self::runtime(&["K5XYZ"]);
        held.finish_turn();
        let effects = held.key(OpKey::HisCall, "K5XYZ", "");
        let id = played(&effects).expect("your call").id;
        let held_up = ContestEvent::TurnStarted {
            id,
            heard_from: MonoMs(held.now + 5_000),
            output_from: MonoMs(held.now + 5_000),
        };
        assert!(held.apply(held_up).is_empty());
        assert_eq!(held.machine.lead_ms, LEAD_MOST_MS);
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
        assert_eq!(
            yours(played(&effects).expect("a turn")),
            "5NN TT1 TU DL1ABC"
        );
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "W2AW", "");
        assert_eq!(yours(runtime.last_turn()), "W2AW 5NN TT2");
        runtime.apply(ContestEvent::FinishNow);
        let (_, run) = result(&runtime);
        assert_eq!(run.qsos[0].nr, 1);
    }

    /// TU and log keyed just before the station's reply ends: it stops before
    /// your TU does and takes it from the fragment, as MorseRunner's
    /// (`DxStn.pas:240-253`), and the QSO is checked once the TU is out.
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
        assert_eq!(
            yours(&tu),
            "TU DL1ABC",
            "your message was over; only the TU goes"
        );
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
        assert_eq!(yours(&tu), "TU DL1ABC");
        // Keyed while the next caller is still calling, the TU long over.
        let call = &tu.segments[1];
        runtime.run_to(runtime.now + (call.end_sec * 1_000.0) as u64 - 20);
        runtime.key(OpKey::Esm, "W2AW", "");
        assert_eq!(yours(runtime.last_turn()), "W2AW 5NN TT2");
        assert_eq!(
            view(&runtime).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );
        // The T begun as you key goes out with the TU's turn, which the
        // player cannot take over from any sooner; the rest of the TU goes
        // on with the next message.
        let mut early = self::runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut early);
        early.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        early.key(OpKey::Esm, "W2AW", "");
        assert_eq!(yours(early.last_turn()), "U DL1ABC W2AW 5NN TT2");
        assert_eq!(view(&early).log[0].status, RowStatus::Pending);
        early.finish_turn();
        assert_eq!(
            view(&early).log[0].status,
            RowStatus::Checked(QsoCheck::default())
        );
    }

    /// Esc: a call or exchange of yours not yet sent whole counts as not
    /// sent. With your message out whole, Esc stops nothing, and both stay
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
            let state = view(&runtime).qso;
            assert_eq!(state.call_sent, call_sent, "{after_ms} ms in");
            assert_eq!(state.exchange_sent, exchange_sent, "{after_ms} ms in");
            if exchange_sent {
                assert!(effects.is_empty(), "{after_ms} ms in: {effects:?}");
                continue;
            }
            assert_eq!(effects[0], ContestEffect::StopAudio, "{after_ms} ms in");
            // Nothing of yours goes on: only the callers, answering what
            // they heard of it or going on with what they were sending.
            let air = view(&runtime).air;
            assert!(
                air.is_none_or(|air| air.my_text.is_empty()),
                "{after_ms} ms in"
            );
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
        // again at once, as it would after any garbage.
        runtime.key(OpKey::Esm, "K5XYW", "");
        runtime.key(OpKey::StopTx, "K5XYW", "");
        assert_eq!(replies(runtime.last_turn()), ["K5XYZ"]);
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
        let ask = half.last_turn().clone();
        assert_eq!(yours(&ask), "");
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
        let owed = logged.last_turn().clone();
        assert_eq!(yours(&owed), "");
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
            output_from: MonoMs(50_100),
        });
        runtime.apply(ContestEvent::TurnStarted {
            id: current,
            heard_from: MonoMs(55_000),
            output_from: MonoMs(55_000),
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
            output_from: MonoMs(700_050),
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
                fields: Box::new(fields),
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

    /// − / + moves your speed, from your next message on, within the Speed
    /// card's bounds; the callers' speeds stay the Speed card's, those
    /// already calling and those who come later alike.
    #[test]
    fn speed_moves_your_message_and_not_the_callers() {
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
        let tu = runtime.last_turn();
        assert_eq!(tu.segments[0].voice.char_wpm, 27.0, "your TU");
        assert_eq!(tu.segments[1].voice.char_wpm, 25.0, "the next caller");
        runtime.apply(ContestEvent::Speed { delta: -200 });
        assert_eq!(view(&runtime).my_wpm, crate::settings::WPM_MIN);
        runtime.apply(ContestEvent::Speed { delta: 200 });
        assert_eq!(view(&runtime).my_wpm, crate::settings::WPM_MAX);
        // The stored run records what you sent: a CQ at 25, then your
        // exchange and your TU at 27.
        runtime.apply(ContestEvent::FinishNow);
        assert_eq!(result(&runtime).1.my_wpm, 26.3);
    }

    /// You send at your speed as set, whatever the callers' range; until
    /// you set one, at the middle of their range, and − / + then moves it
    /// from the whole speed nearest that.
    #[test]
    fn you_send_at_your_speed_or_the_callers_middle() {
        let mut settings = run_settings(25.0);
        settings.contest.my_wpm = 34;
        let set = Runtime::start(1, settings.clamp(), ScriptCalls::new(&["K5XYZ"]));
        assert_eq!(view(&set).my_wpm, 34.0);
        assert_eq!(set.last_turn().segments[0].voice.char_wpm, 34.0, "your CQ");
        assert_eq!(
            set.last_turn().segments[1].voice.char_wpm,
            25.0,
            "the caller"
        );
        let mut settings = run_settings(25.0);
        settings.playback.char_wpm_min = 20.0;
        settings.playback.effective_wpm_min = 20.0;
        let mut middle = Runtime::start(1, settings.clamp(), ScriptCalls::new(&["K5XYZ"]));
        assert_eq!(view(&middle).my_wpm, 22.5);
        assert_eq!(middle.last_turn().segments[0].voice.char_wpm, 22.5);
        middle.apply(ContestEvent::Speed { delta: 2 });
        assert_eq!(
            view(&middle).my_wpm,
            25.0,
            "from 23, the nearest whole speed"
        );
        middle.apply(ContestEvent::Speed { delta: -2 });
        assert_eq!(view(&middle).my_wpm, 23.0);
    }

    /// Settings for a run whose callers' speed follows your copy, within
    /// `low` to `high` WPM, in `style`, under `habits`: through a filter
    /// wide open, so that a pile-up's filter is no limit.
    fn adapting(low: f64, high: f64, style: RunStyle, habits: CallerHabits) -> TrainingSettings {
        let mut settings = default_run_settings();
        settings.playback.char_wpm_min = low;
        settings.playback.char_wpm_max = high;
        settings.playback.effective_wpm_min = low;
        settings.playback.effective_wpm_max = high;
        settings.set_contest_style(style);
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        settings.contest.contest_habits = habits;
        settings.contest.contest_adapt_speed = true;
        settings.clamp()
    }

    /// A run of `settings` from `seed` through the sampler, worked by
    /// `habit` to its end.
    fn worked_by(habit: Habit, settings: &TrainingSettings, seed: u64) -> Runtime<Sampler> {
        let sampler = Sampler::new(&[], settings, seed);
        let mut runtime = Runtime::start(seed, settings.clone(), sampler);
        work(&mut runtime, &mut Operator::new(habit), &[]);
        runtime
    }

    /// Each caller's character speeds in every send of `turns`, by station,
    /// in the order the callers first sent.
    fn callers_speeds(turns: &[Turn]) -> Vec<(u32, Vec<f64>)> {
        let mut speeds: Vec<(u32, Vec<f64>)> = Vec::new();
        for segment in turns.iter().flat_map(|turn| &turn.segments) {
            let Sender::Caller(station) = segment.who else {
                continue;
            };
            match speeds.iter_mut().find(|(at, _)| *at == station) {
                Some((_, sent)) => sent.push(segment.voice.char_wpm),
                None => speeds.push((station, vec![segment.voice.char_wpm])),
            }
        }
        speeds
    }

    fn mean(speeds: &[f64]) -> f64 {
        speeds.iter().sum::<f64>() / speeds.len() as f64
    }

    /// With the callers' speed following your copy, a run whose calls and
    /// numbers are copied clean brings faster callers as it goes on, and
    /// one whose numbers are missed slower ones, smoothly, as the belief
    /// moves with each QSO logged: by the run's end the callers it would
    /// draw next are faster than at its start after clean copy, and slower
    /// after misses. Every caller comes within the Speed card's range.
    #[test]
    fn callers_speed_up_as_you_copy_clean_and_slow_down_as_you_miss() {
        let settings = adapting(15.0, 40.0, RunStyle::Single, CallerHabits::Real);
        let next = |source: &mut Sampler| {
            let speeds: Vec<f64> = (0..400)
                .map(|_| source.next_speed(&settings).expect("adapting").0)
                .collect();
            mean(&speeds)
        };
        for seed in 1..=3 {
            let fresh = next(&mut Sampler::new(&[], &settings, seed));
            let worked = |habit: Habit| {
                let mut runtime = worked_by(habit, &settings, seed);
                let speeds: Vec<f64> = callers_speeds(&runtime.turns)
                    .into_iter()
                    .map(|(_, sent)| sent[0])
                    .collect();
                assert!(
                    speeds.iter().all(|wpm| (15.0..=40.0).contains(wpm)),
                    "seed {seed} {habit:?}: {speeds:?}"
                );
                assert!(speeds.len() >= 15, "seed {seed} {habit:?}: {speeds:?}");
                let later = mean(&speeds[speeds.len() / 2..]);
                (later, next(&mut runtime.machine.source))
            };
            let (clean_later, clean_next) = worked(Habit::Perfect);
            let (missed_later, missed_next) = worked(Habit::DigitOff);
            assert!(
                clean_next > fresh + 3.0,
                "seed {seed}: {clean_next} from {fresh}"
            );
            assert!(
                missed_next < fresh - 3.0,
                "seed {seed}: {missed_next} from {fresh}"
            );
            assert!(
                clean_later > missed_later + 5.0,
                "seed {seed}: callers at {clean_later} copied clean, {missed_later} missed"
            );
        }
    }

    /// Callers already calling keep the speed they came with, however the
    /// belief moves while they call: only a newcomer's speed is drawn from
    /// it, and newcomers' differ. A dupe is the same operator, at its old
    /// speed.
    #[test]
    fn callers_already_calling_keep_their_speed() {
        let mut settings = adapting(15.0, 40.0, RunStyle::Pileup, CallerHabits::Real);
        settings.band.stations_min = 2;
        settings.band.stations_max = 5;
        for seed in 1..=3 {
            let runtime = worked_by(Habit::Perfect, &settings, seed);
            let speeds = callers_speeds(&runtime.turns);
            for (station, sent) in &speeds {
                assert!(
                    sent.iter().all(|wpm| wpm == &sent[0]),
                    "seed {seed}: caller {station} sent at {sent:?}"
                );
            }
            let sends = |at: usize| speeds.iter().filter(|(_, sent)| sent.len() > at).count();
            assert!(sends(3) > 5, "seed {seed}: callers call again and again");
            let first: Vec<f64> = speeds.iter().map(|(_, sent)| sent[0]).collect();
            assert!(
                first.iter().any(|wpm| wpm != &first[0]),
                "seed {seed}: {first:?}"
            );
        }
    }

    /// Under easy habits a caller asked for its call again and again still
    /// slows down for you, from the speed it came with, drawn from your copy
    /// speed: the belief picks a newcomer's speed and never touches a caller
    /// once it calls, and QRS slows one caller within its QSO and is gone
    /// with it, so the two never pull against each other. Its call is
    /// credited at its slowest, which is what the belief learns from; the
    /// next caller comes within the range again.
    #[test]
    fn a_caller_whose_speed_was_drawn_still_slows_down_for_you() {
        let settings = adapting(15.0, 40.0, RunStyle::Single, CallerHabits::Easy);
        let sampler = Sampler::new(&[], &settings, 1);
        let mut runtime = Runtime::start(1, settings, sampler);
        let calling = runtime.last_turn().segments[1].clone();
        let own = calling.voice;
        assert!((15.0..=40.0).contains(&own.char_wpm), "{own:?}");
        let call = calling.text.split_whitespace().next().expect("its call");
        let partial: String = call.chars().take(2).chain(['?']).collect();
        runtime.finish_turn();
        let mut speeds = Vec::new();
        for _ in 0..4 {
            runtime.key(OpKey::Esm, &partial, "");
            speeds.push(runtime.last_turn().segments[1].voice.char_wpm);
            runtime.finish_turn();
        }
        assert_eq!(speeds[..2], [own.char_wpm; 2], "{speeds:?}");
        assert!(speeds[3] < own.char_wpm, "{speeds:?}");
        assert!(
            speeds[3] >= own.char_wpm * (1.0 - crate::contest::QRS_MOST) - 1e-9,
            "{speeds:?}"
        );
        runtime.key(OpKey::Esm, call, "");
        let exchange = runtime.last_turn().clone();
        assert_eq!(exchange.segments[1].voice, own, "back on track");
        runtime.finish_turn();
        let effects = runtime.key(OpKey::Esm, call, &serial(&exchange.segments[1].text));
        let tu = played(&effects).expect("TU goes out").clone();
        let next = tu.segments.iter().find(|segment| segment.who.is_caller());
        let next = next.expect("the next caller calls").voice.char_wpm;
        assert!((15.0..=40.0).contains(&next), "{next}");
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        let result = runtime.machine.result(1_000, "2026-10-22".into());
        assert_eq!(result.group_timings[0].char_wpm, Some(tenth(speeds[3])));
    }

    /// A seed and the keys pressed give one run, the callers' speeds drawn
    /// from your copy among it.
    #[test]
    fn a_run_whose_callers_adapt_is_the_same_run_from_the_same_seed() {
        let settings = adapting(15.0, 40.0, RunStyle::Pileup, CallerHabits::Rough);
        let turns = |seed| worked_by(Habit::Perfect, &settings, seed).turns;
        assert_eq!(turns(4), turns(4));
        assert_ne!(turns(4), turns(5));
    }

    /// The sampler, but never choosing a caller's speed, as no source did
    /// before the callers' speed could follow your copy.
    struct Even(Sampler);

    impl CallSource for Even {
        fn next_call(
            &mut self,
            settings: &TrainingSettings,
            spec: crate::callsign::CallSpec,
            avoid: &dyn Fn(&str) -> bool,
        ) -> crate::callsign::GeneratedCall {
            self.0.next_call(settings, spec, avoid)
        }

        fn learn(&mut self, answered: Answered<'_>) {
            CallSource::learn(&mut self.0, answered);
        }
    }

    /// With the switch off, the callers' speeds are drawn evenly from the
    /// Speed card's ranges, as before: the run is the very run of a source
    /// that chooses no speed.
    #[test]
    fn with_the_switch_off_callers_come_as_they_always_have() {
        let mut off = adapting(15.0, 40.0, RunStyle::Pileup, CallerHabits::Easy);
        off.contest.contest_adapt_speed = false;
        let on = adapting(15.0, 40.0, RunStyle::Pileup, CallerHabits::Easy);
        for seed in 1..=2 {
            let fixed = worked_by(Habit::Perfect, &off, seed);
            let mut even = Runtime::start(seed, on.clone(), Even(Sampler::new(&[], &on, seed)));
            work(&mut even, &mut Operator::new(Habit::Perfect), &[]);
            assert_eq!(fixed.turns, even.turns, "seed {seed}");
            let adapted = worked_by(Habit::Perfect, &on, seed);
            assert_ne!(fixed.turns, adapted.turns, "seed {seed}");
        }
    }

    /// The call source learns your copy speed from a QSO's call and its
    /// number, as the copy-speed series reads a run, and not from a report
    /// or a name, which are half guessed; it learns the characters of all
    /// of them, once the QSO is logged.
    #[test]
    fn the_copy_speed_is_taught_by_calls_and_numbers() {
        let mut wpx = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut wpx);
        assert!(wpx.machine.source.learned_speed.is_empty(), "nothing final");
        wpx.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
        let taught = |pairs: &[(String, String)]| -> Vec<String> {
            pairs.iter().map(|(sent, _)| sent.clone()).collect()
        };
        assert_eq!(
            taught(&wpx.machine.source.learned_speed),
            ["K5XYZ".to_string(), serial(&exchange)]
        );
        assert_eq!(wpx.machine.source.learned.len(), 2);

        let mut settings = run_settings(25.0);
        settings.contest.contest_kind = crate::contest::ContestKind::SimpleQso;
        let mut simple = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
        let exchange = to_its_exchange(&mut simple);
        let words: Vec<&str> = exchange.split_whitespace().collect();
        simple.key(OpKey::Esm, "K5XYZ", &format!("{} {}", words[3], words[6]));
        assert_eq!(taught(&simple.machine.source.learned_speed), ["K5XYZ"]);
        assert_eq!(
            simple.machine.source.learned.len(),
            3,
            "the call, report and name"
        );
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

    /// `?` keyed after their first exchange ends asks for it again, and
    /// costs its send. One keyed while it is still sounding is lost on the
    /// station, sending, as on MorseRunner's (`DxStn.pas:240-253`): it asks
    /// for nothing, and costs nothing. It used to stop the station and ask.
    #[test]
    fn asking_during_their_first_exchange_is_lost_on_them() {
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
        assert_eq!(asked(Some(0.98)), (vec![1, 1], 0), "just before its end");
        assert_eq!(asked(Some(0.5)), (vec![1, 1], 0), "halfway through");
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
        // The C of the CQ sounding, the rest goes on from the Q.
        assert_eq!(yours(runtime.last_turn()), "Q DL1ABC TEST ?");
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
            ..Fields::default()
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
                    fields: Box::new(fields),
                });
                runtime.finish_turn();
            }
            let fields = fields(&nr, edited, typed);
            runtime.apply(ContestEvent::Key {
                key: OpKey::Esm,
                fields: Box::new(fields),
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
            runtime.apply(ContestEvent::Key {
                key,
                fields: Box::new(fields),
            });
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
    /// sends its call, and the number is not charged for it, whether the
    /// call comes alone or with the number again, though the Exchange field
    /// was still empty at the partial.
    #[test]
    fn a_partial_while_you_work_the_station_brings_its_call() {
        for with_exchange in [false, true] {
            let mut runtime = runtime_fixing(&["K5XYZ"], with_exchange);
            let nr = serial(&to_its_exchange(&mut runtime));
            runtime.key(OpKey::Esm, "K5X?Z", "");
            assert_eq!(yours(runtime.last_turn()), "K5X?Z");
            let back = replies(runtime.last_turn())[0].to_string();
            assert!(back.contains("K5XYZ"), "asked for the call, got {back}");
            assert_eq!(back.ends_with(&nr), with_exchange, "{back}");
            runtime.finish_turn();
            assert!(!view(&runtime).qso.corrected, "an answer, not a correction");
            runtime.key(OpKey::Esm, "K5XYZ", &nr);
            assert_eq!(yours(runtime.last_turn()), "K5XYZ TU DL1ABC");
            runtime.finish_turn();
            assert_eq!(view(&runtime).clean, 1);
            runtime.apply(ContestEvent::FinishNow);
            assert_eq!(sends(&runtime), [1, 1], "{back}");
        }
        // Waiting for your exchange, it sends its call too.
        let mut waiting = self::runtime(&["K5XYZ"]);
        waiting.finish_turn();
        waiting.key(OpKey::HisCall, "K5XYZ", "");
        waiting.finish_turn();
        waiting.key(OpKey::Esm, "K5X?", "");
        let back = replies(waiting.last_turn())[0].to_string();
        assert!(back.contains("K5XYZ"), "asked for the call, got {back}");
    }

    /// An exchange a station sends with its call put right costs a send
    /// when your `?` brings it again, as any exchange you ask for does; the
    /// first, drawn by your slip, costs nothing.
    #[test]
    fn an_exchange_sent_with_a_correction_costs_a_send_when_you_ask_again() {
        let mut runtime = runtime_fixing(&["K5XYZ"], true);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYW", "");
        let fix = replies(runtime.last_turn())[0].to_string();
        let Some(Copied::Correction("K5XYZ", Some(nr))) = copy_of(&fix) else {
            panic!("{fix}");
        };
        runtime.finish_turn();
        runtime.key(OpKey::Again, "K5XYW", "");
        assert_eq!(replies(runtime.last_turn()), [fix.as_str()], "sent again");
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        assert_eq!(yours(runtime.last_turn()), "K5XYZ TU DL1ABC");
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        assert_eq!(sends(&runtime), [1, 2]);
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
        // Its exchange sent, then left after three silent timeouts: lost,
        // and the log after that is a NIL with no group of its own.
        let mut left = self::runtime(&["K5XYZ"]);
        to_its_exchange(&mut left);
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
            fields: Box::new(fields),
        };
        let turn = played(&machine.apply(key, MonoMs(50_000)))
            .expect("a turn")
            .id;
        for (heard_from, now) in [(50_100, 50_050), (56_100, 56_050)] {
            let started = ContestEvent::TurnStarted {
                id: turn,
                heard_from: MonoMs(heard_from),
                output_from: MonoMs(heard_from),
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
                assert_eq!(yours(runtime.last_turn()), "TU DL1ABC", "{key:?}");
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

    /// Enter or TU and log after your TU without logging (F3) sends the TU
    /// again and logs, as MorseRunner's does; the next caller, brought by
    /// the first TU, calls again on the second, which you keyed to log and
    /// not to hear it again: re-sends you did not ask for cost only time,
    /// so worked then its call costs one send, and no `?×1` (§4.6).
    #[test]
    fn a_log_after_a_tu_without_logging_asks_the_next_caller_nothing() {
        for key in [OpKey::Esm, OpKey::TuAndLog] {
            let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
            let exchange = to_its_exchange(&mut runtime);
            let nr = serial(&exchange);
            runtime.key(OpKey::Tu, "K5XYZ", &nr);
            assert_eq!(replies(runtime.last_turn()), ["G4ABC"], "{key:?}");
            runtime.finish_turn();
            runtime.key(key, "K5XYZ", &nr);
            assert_eq!(yours(runtime.last_turn()), "TU DL1ABC", "{key:?}");
            assert_eq!(replies(runtime.last_turn()), ["G4ABC"], "{key:?}");
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "G4ABC", "");
            let theirs = replies(runtime.last_turn())[0].to_string();
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "G4ABC", &serial(&theirs));
            runtime.finish_turn();
            let asked: Vec<u32> = view(&runtime).log.iter().map(|row| row.asked).collect();
            assert_eq!(asked, [0, 0], "{key:?}");
            runtime.apply(ContestEvent::FinishNow);
            let (result, run) = result(&runtime);
            assert!(run.qsos.iter().all(|qso| qso.a == 0), "{key:?}");
            assert!(
                result.group_timings.iter().all(|timing| timing.sends == 1),
                "{key:?}: {:?}",
                result.group_timings
            );
        }
    }

    /// A caller asked for its call again and again slows down for you from
    /// the third partial, or the fourth if it is a skilled operator, as
    /// MorseRunner's (§4.3): its own sends only, never yours. Its call heard
    /// right, its exchange comes at its own speed again. Its call is
    /// credited at the slowest it began to reach you at, as it may have
    /// been copied from that send, and its exchange, heard only at its own
    /// speed, at that; the log row reads the slower.
    #[test]
    fn a_caller_asked_for_its_call_again_and_again_slows_down() {
        let mut runtime = runtime(&["K5XYZ"]);
        let own = runtime.last_turn().segments[1].voice;
        runtime.finish_turn();
        let mut speeds = Vec::new();
        for _ in 0..4 {
            runtime.key(OpKey::Esm, "K5?", "");
            let turn = runtime.last_turn().clone();
            assert_eq!(turn.segments[0].voice.char_wpm, 25.0, "you send as before");
            speeds.push(turn.segments[1].voice.char_wpm);
            runtime.finish_turn();
        }
        assert_eq!(speeds[..2], [own.char_wpm; 2], "{speeds:?}");
        assert!(speeds[3] < own.char_wpm, "{speeds:?}");
        assert!(speeds[3] <= speeds[2], "{speeds:?}");
        assert!(speeds[3] >= own.char_wpm * 0.78 - 1e-9, "{speeds:?}");
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let exchange = runtime.last_turn().clone();
        assert_eq!(exchange.segments[1].voice, own, "back on track");
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange.segments[1].text));
        runtime.finish_turn();
        assert_eq!(view(&runtime).log[0].wpm, Some(tenth(speeds[3])));
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        let credited: Vec<Option<f64>> = result
            .group_timings
            .iter()
            .map(|timing| timing.char_wpm)
            .collect();
        assert_eq!(
            credited,
            [Some(tenth(speeds[3])), Some(tenth(own.char_wpm))],
            "the call at its slowest, the number at its own"
        );
    }

    /// A slow reply you keyed over before it began never reached you: the
    /// QSO is credited at the speed of the sends that did, its own.
    #[test]
    fn a_slow_reply_keyed_over_before_it_began_is_not_credited() {
        let mut runtime = runtime(&["K5XYZ"]);
        let own = runtime.last_turn().segments[1].voice;
        runtime.finish_turn();
        let at = runtime.now;
        let slow = loop {
            let at = runtime.now;
            runtime.key(OpKey::Esm, "K5?", "");
            let turn = runtime.last_turn().clone();
            if turn.segments[1].voice != own {
                break (at, turn);
            }
            runtime.finish_turn();
        };
        let (keyed, turn) = slow;
        assert!(keyed > at, "two partials answered at its own speed first");
        let (yours, reply) = (&turn.segments[0], &turn.segments[1]);
        assert!(yours.end_sec < reply.start_sec, "{turn:?}");
        // Keyed over in the quiet between your partial and its reply.
        runtime.run_to(keyed + ms((yours.end_sec + reply.start_sec) / 2.0));
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let exchange = runtime.last_turn().clone();
        assert!(
            exchange
                .segments
                .iter()
                .all(|segment| segment.voice.char_wpm == 25.0 || segment.voice == own),
            "{exchange:?}"
        );
        runtime.finish_turn();
        let nr = serial(replies(&exchange)[0]);
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        assert_eq!(view(&runtime).log[0].wpm, Some(tenth(own.char_wpm)));
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        assert!(
            result
                .group_timings
                .iter()
                .all(|timing| timing.char_wpm == Some(tenth(own.char_wpm))),
            "{:?}",
            result.group_timings
        );
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
        assert_eq!(yours(runtime.last_turn()), "5XYZ 5NN TT1 ?");
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
        assert_eq!(yours(played(&effects).expect("TU goes out")), "TU DL1ABC");
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
        // Nobody calls now: Enter's CQ is for the next station, in place
        // of the call it would only send to nobody.
        let afresh = QsoState {
            nobody_calling: true,
            gone: vec!["K5XYZ".into()],
            heard_gone: vec!["K5XYZ".into()],
            ..QsoState::default()
        };
        assert_eq!(view(&runtime).qso, afresh);
        let fields = Fields {
            call: "K5XYZ".into(),
            exch: nr,
            ..Fields::default()
        };
        let enter = esm(&view(&runtime).qso, &fields, &RULES[0], Side::Dx);
        assert_eq!(
            (enter.label.as_str(), enter.hint.as_str()),
            ("CQ", "K5XYZ has gone: Enter calls CQ")
        );
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
            ..Fields::default()
        };
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.now += 1_500;
        let typed = runtime.now;
        for key in [OpKey::Again, OpKey::Esm] {
            let fields = fields(typed, "", None);
            runtime.apply(ContestEvent::Key {
                key,
                fields: Box::new(fields),
            });
            runtime.finish_turn();
        }
        let nr = serial(replies(&runtime.turns[runtime.turns.len() - 1])[0]);
        runtime.now += 1_500;
        let done = runtime.now;
        for key in [OpKey::Again, OpKey::Esm] {
            let fields = fields(typed, &nr, Some(done));
            runtime.apply(ContestEvent::Key {
                key,
                fields: Box::new(fields),
            });
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
        assert_eq!(yours(twice.last_turn()), "5XYW 5NN TT1 ?");
        let correction = replies(twice.last_turn())[0].to_string();
        assert!(
            matches!(copy_of(&correction), Some(Copied::Correction("K5XYZ", _))),
            "{correction}"
        );
        let from = twice.effects.len();
        twice.finish_turn();
        assert!(view(&twice).qso.corrected, "the correction went unnoticed");
        assert!(twice.effects[from..].contains(&ContestEffect::Announce(
            "They corrected you: fix the call, then Enter".into()
        )));
        assert_eq!(enter(&twice, "K5XYW"), "Fix call", "Enter, Enter");
        // Corrected, then the correction misheard as another close call,
        // by a station that sends its call alone.
        let mut misheard = runtime_fixing(&["K5XYZ"], false);
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
        // Nobody calls then: Enter's CQ is for the next station, and never
        // the call of the station sent away.
        let afresh = QsoState {
            nobody_calling: true,
            gone: vec!["K5XYZ".into()],
            heard_gone: vec!["K5XYZ".into()],
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
            assert_eq!(enter.label, "CQ", "{key:?}");
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
        // What the partial asked for is as gone as the call it brought.
        let afresh = QsoState {
            gone: vec!["K5".into(), "K5XYZ".into()],
            ..afresh
        };
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
        Runtime::start(seed, pileup_settings(fewest, most), ScriptCalls::new(calls))
    }

    /// A pile-up's settings: as many callers as `fewest` to `most`, on the
    /// widest filter.
    fn pileup_settings(fewest: u32, most: u32) -> TrainingSettings {
        let mut settings = run_settings(25.0);
        settings.contest.contest_style = RunStyle::Pileup;
        settings.band.stations_min = fewest;
        settings.band.stations_max = most;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        settings.clamp()
    }

    /// A pile-up worked with full break-in: callers sending across your
    /// message are heard through it.
    fn pileup_qsk(calls: &[&str], fewest: u32, most: u32) -> Runtime<ScriptCalls> {
        let mut settings = pileup_settings(fewest, most);
        settings.contest.contest_qsk = true;
        Runtime::start(1, settings, ScriptCalls::new(calls))
    }

    /// The callers that send in `turn`, each once: a caller whose wait
    /// runs out before the turn is over calls again in it.
    fn senders(turn: &Turn) -> std::collections::BTreeSet<u32> {
        turn.segments
            .iter()
            .filter_map(|segment| match segment.who {
                Sender::Caller(station) => Some(station),
                Sender::You | Sender::Qrm => None,
            })
            .collect()
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
            .filter(|segment| segment.who.is_caller())
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
            counts.insert(senders(runtime.last_turn()).len());
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
    /// after you. A caller that had finished hears you. A CQ or a NIL is
    /// heard from a fragment by a caller that stops before your message
    /// ends: it goes on, heard over you, and answers after you.
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
        // A CQ is heard from a fragment: it goes on, stops before your CQ
        // ends, and calls after it.
        let (runtime, _, second, _, cq) = keyed_over(OpKey::Cq);
        let turn = runtime.last_turn();
        let sends = sent_by(turn, second);
        assert_eq!(sends[0].start_sec, 0.0, "it goes on, over your CQ");
        assert!(said_by(&cq, second).is_some_and(|whole| whole.ends_with(&sends[0].text)));
        assert!(sends[0].end_sec < turn.segments[0].end_sec, "done first");
        let after = sends.get(1).expect("it calls");
        assert!(after.start_sec > turn.segments[0].end_sec, "after your CQ");
        assert!(after.start_sec > sends[0].end_sec, "and after its own");
        // So is a NIL: it spends a try on it, and calls again.
        let (runtime, _, second, before, _) = keyed_over(OpKey::Nil);
        let turn = runtime.last_turn();
        let station = &runtime.machine.stations[second as usize];
        assert!(sent_by(turn, second)[0].end_sec < turn.segments[0].end_sec);
        assert_ne!(station.caller, before.caller, "it heard the NIL");
        assert_eq!(sent_by(turn, second).len(), 2);
    }

    /// One caller at 12 WPM, and you at 40 WPM, so that a send of its
    /// outlasts a message of yours keyed early in it, in a run of `style`
    /// from `seed`.
    fn slow_caller(style: RunStyle, seed: u64) -> Runtime<ScriptCalls> {
        let mut settings = run_settings(12.0);
        settings.contest.contest_style = style;
        settings.contest.my_wpm = 40;
        settings.band.stations_min = 1;
        settings.band.stations_max = 1;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        Runtime::start(seed, settings.clamp(), ScriptCalls::new(&["K5XYZ"]))
    }

    /// A pile-up caller still sending when your CQ, NIL or TU ends never
    /// hears it, as MorseRunner's station sending pays no heed to your
    /// message (`DxStn.pas:240-253`): it goes on, then waits its reply
    /// timeout from the end of its send before it calls again, as it would
    /// with no message of yours at all. One that stops first takes the
    /// fragment it heard and calls a moment after you. A caller sending
    /// through the whole of your message used to take it from a fragment
    /// all the same, and answer at once.
    #[test]
    fn a_caller_sending_through_your_cq_nil_or_tu_never_hears_it() {
        for key in [OpKey::Cq, OpKey::Nil, OpKey::Tu] {
            let mut runtime = slow_caller(RunStyle::Pileup, 1);
            let cq = runtime.last_turn().clone();
            let call = sent_by(&cq, 0)[0].clone();
            runtime.run_to(ms(call.start_sec + 0.2));
            runtime.key(key, "", "");
            let turn = runtime.last_turn().clone();
            let your_end = turn.your_end_sec().expect("your message");
            let sends = sent_by(&turn, 0);
            assert_eq!(sends[0].start_sec, 0.0, "{key:?}: it goes on");
            assert!(sends[0].end_sec > your_end, "{key:?}: through your message");
            assert_eq!(sends.len(), 1, "{key:?}: it answers nothing");
            let end = turn.at_run_ms + whole_ms(sends[0].end_sec);
            runtime.finish_turn();
            let station = &runtime.machine.stations[0];
            assert_eq!(station.caller.state, CallerState::NeedQso, "{key:?}");
            let deadline = station.caller.deadline.expect("it waits");
            assert!(deadline >= end + 1_500, "{key:?}: on its timeout");
            let next = next_turn(&mut runtime);
            assert_eq!(said_by(&next, 0), Some(call.text.as_str()), "{key:?}");
            assert!(next.at_run_ms >= deadline, "{key:?}");
        }
    }

    /// The station you work, still sending its exchange when your TU ends,
    /// never hears it, as MorseRunner's (`DxStn.pas:240-253`): it is not
    /// done, and once its send is over it waits its reply timeout and sends
    /// its exchange again, as it would had you sent nothing. One that
    /// stops before your TU ends takes it from the fragment and is done. A
    /// station putting its call right takes your TU even while it sends,
    /// as MorseRunner's (`DxStn.pas:277-289`). A single caller is the same:
    /// it used to stop for your TU and hear it. Every station you keyed a
    /// TU over used to take it.
    #[test]
    fn the_station_sending_its_exchange_through_your_tu_is_not_done() {
        let working = |style: RunStyle| {
            let mut runtime = slow_caller(style, 1);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let exchange = sent_by(runtime.last_turn(), 0)[0].clone();
            (runtime, exchange)
        };
        // Through it, then as with no TU at all.
        let (mut runtime, exchange) = working(RunStyle::Pileup);
        let (mut alone, _) = working(RunStyle::Pileup);
        runtime.run_to(runtime.now + ms(exchange.start_sec + 0.2));
        let nr = serial(&exchange.text);
        runtime.key(OpKey::TuAndLog, "K5XYZ", &nr);
        let tu = runtime.last_turn().clone();
        let rest = sent_by(&tu, 0);
        assert!(rest[0].end_sec > tu.your_end_sec().expect("your TU"));
        assert_eq!(rest.len(), 1);
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedEnd,
            "not done"
        );
        runtime.finish_turn();
        let again = next_turn(&mut runtime);
        alone.finish_turn();
        let unstopped = next_turn(&mut alone);
        let sent = said_by(&again, 0).expect("its exchange again");
        assert_eq!(Some(sent), said_by(&unstopped, 0));
        assert!(exchange.text.ends_with(sent), "{sent}");
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        assert_eq!(view(&runtime).clean, 1, "logged all the same");
        // Stopped before your TU ends, it takes the TU.
        let (mut runtime, exchange) = working(RunStyle::Pileup);
        runtime.run_to(runtime.now + ms(exchange.end_sec - 0.05));
        runtime.key(OpKey::TuAndLog, "K5XYZ", &nr);
        let tu = runtime.last_turn().clone();
        assert!(sent_by(&tu, 0)[0].end_sec < tu.your_end_sec().expect("your TU"));
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Done);
        // A single caller sends on through it too, and is not done.
        let (mut runtime, exchange) = working(RunStyle::Single);
        runtime.run_to(runtime.now + ms(exchange.start_sec + 0.2));
        runtime.key(OpKey::TuAndLog, "K5XYZ", &nr);
        let tu = runtime.last_turn().clone();
        let rest = sent_by(&tu, 0);
        assert_eq!(rest.len(), 1, "it goes on");
        assert!(rest[0].end_sec > tu.your_end_sec().expect("your TU"));
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedEnd,
            "not done"
        );
        // Putting its call right, it takes your TU while it sends.
        let fixing = (1..40)
            .map(|seed| {
                let mut runtime = slow_caller(RunStyle::Pileup, seed);
                runtime.finish_turn();
                runtime.key(OpKey::Esm, "K5XYW", "");
                runtime
            })
            .find(|runtime| runtime.machine.stations[0].caller.state == CallerState::NeedCall);
        let mut runtime = fixing.expect("a station putting its call right");
        let correction = sent_by(runtime.last_turn(), 0)[0].clone();
        runtime.run_to(runtime.now + ms(correction.start_sec + 0.2));
        runtime.key(OpKey::Tu, "K5XYZ", "");
        let tu = runtime.last_turn().clone();
        assert!(sent_by(&tu, 0)[0].end_sec > tu.your_end_sec().expect("your TU"));
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Done);
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

    /// Keyed over again and again while it sends a character slower than
    /// yours, a caller still sending goes on all the same: the character
    /// sounding at a splice goes again whole once, and a splice while it
    /// does carries it on from where it is rather than starting it over.
    /// Its call gets through however long you keep keying, where it used to
    /// send that one character for as long as the keys kept coming.
    #[test]
    fn keys_over_a_slow_caller_never_hold_it_back() {
        let mut settings = run_settings(12.0);
        settings.contest.contest_style = RunStyle::Pileup;
        settings.contest.my_wpm = 40;
        settings.band.stations_min = 1;
        settings.band.stations_max = 1;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        let mut runtime = Runtime::start(1, settings.clamp(), ScriptCalls::new(&["K5XYZ"]));
        let cq = runtime.last_turn().clone();
        let call = sent_by(&cq, 0)[0].clone();
        let spans = char_spans(&call.text, &runtime.machine.settings, &call.voice);
        // Halfway into its second character, longer than your own.
        let (from, to) = (spans[1].start_sec, spans[1].end_sec);
        assert!(to - from > 0.8, "{:?}", spans[1]);
        runtime.run_to(runtime.now + ms(call.start_sec + (from + to) / 2.0));
        let keyed = runtime.now;
        let whole = call.end_sec - call.start_sec;
        loop {
            runtime.key(OpKey::Again, "", "");
            let carried = sent_by(runtime.last_turn(), 0)
                .iter()
                .any(|segment| segment.start_sec == 0.0);
            if !carried {
                break;
            }
            runtime.run_to(runtime.now + 100);
            let sending = (runtime.now - keyed) as f64 / 1_000.0;
            assert!(
                sending < whole + 2.0,
                "still sending its call {sending:.1} s after the first key, {whole:.1} s long"
            );
        }
    }

    /// One caller at a time is one: your TU keyed over the station you
    /// work, still sending its exchange, sends it on its way and brings
    /// the next caller, who calls once that exchange is over rather than
    /// across it. Both stations heard at once was two callers at once,
    /// with the Band card holding them to one.
    #[test]
    fn the_next_caller_waits_for_a_station_leaving_to_finish() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 1, 1);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let turn = runtime.last_turn().clone();
        let exchange = sent_by(&turn, 0)[0].clone();
        // Halfway through its exchange, your TU.
        runtime.run_to(runtime.now + ms((exchange.start_sec + exchange.end_sec) / 2.0));
        runtime.key(OpKey::Tu, "K5XYZ", "");
        let turn = runtime.last_turn().clone();
        let rest = sent_by(&turn, 0)
            .into_iter()
            .find(|segment| segment.start_sec == 0.0)
            .expect("its exchange goes on over your TU")
            .clone();
        assert!(
            !runtime.machine.stations[0].caller.state.active(),
            "it heard your TU"
        );
        let next = sent_by(&turn, 1)
            .first()
            .map(|call| (*call).clone())
            .expect("the next caller calls");
        assert_eq!(next.text, "W7ABC");
        assert!(
            next.start_sec >= rest.end_sec + 0.1 - 1e-9,
            "{} calls at {:.2} s over the end of {:?} at {:.2} s",
            next.text,
            next.start_sec,
            rest.text,
            rest.end_sec
        );
    }

    /// A single caller you key over goes on with its call and never hears
    /// you, as MorseRunner's does (`DxStn.pas:240-253`): a `?` sent over
    /// its call is lost on it, and it calls again only on its reply
    /// timeout, its call whole. It used to stop and hear you; the `?` key
    /// is there for once it has finished.
    #[test]
    fn a_single_caller_keyed_over_goes_on_and_misses_you() {
        let mut runtime = runtime(&["K5XYZ"]);
        let cq = runtime.last_turn().clone();
        let start = cq.segments[1].start_sec;
        runtime.run_to(runtime.now + ((start + 0.05) * 1_000.0) as u64);
        runtime.key(OpKey::Again, "", "");
        let turn = runtime.last_turn().clone();
        let sends = sent_by(&turn, 0);
        assert_eq!(sends.len(), 1, "it answers nothing in this turn");
        let rest = sends[0];
        assert_eq!(rest.start_sec, 0.0, "it goes on, over your `?`");
        // From the character sounding at the key, sent again whole: here
        // still its first.
        assert!("K5XYZ".ends_with(&rest.text), "{}", rest.text);
        assert!(
            rest.end_sec > turn.segments[0].end_sec,
            "still sending as your `?` ends"
        );
        let station = &runtime.machine.stations[0];
        assert_eq!(
            station.caller.state,
            CallerState::NeedQso,
            "it heard nothing"
        );
        // It calls again on its own timeout, its call whole.
        let ends = runtime.now + ms(rest.end_sec);
        let again = next_turn(&mut runtime);
        assert_eq!(said_by(&again, 0), Some("K5XYZ"));
        assert!(
            again.at_run_ms >= ends + 1_500,
            "on its timeout: {} after its send ended at {ends}",
            again.at_run_ms
        );
        // Asked once it has finished, it hears the `?` and sends its call.
        runtime.finish_turn();
        runtime.key(OpKey::Again, "", "");
        let asked = runtime.last_turn();
        let call = sent_by(asked, 0)[0];
        assert!(call.start_sec > asked.segments[0].end_sec, "after your `?`");
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

    /// Out of the box every QSO's TU signs with your call, in a pile-up and
    /// with one caller at a time alike.
    #[test]
    fn every_tu_signs_with_your_call_out_of_the_box() {
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
        assert_eq!(tus, ["TU DL1ABC"; 6]);
        // A single caller's does too.
        let mut single = self::runtime(&["K5XYZ", "W7ABC", "G4ABC"]);
        for (n, call) in ["K5XYZ", "W7ABC", "G4ABC"].into_iter().enumerate() {
            work_with_enter(&mut single, call, n as u32);
        }
        let tus: Vec<&str> = single
            .turns
            .iter()
            .map(yours)
            .filter(|text| text.starts_with("TU"))
            .collect();
        assert_eq!(tus, ["TU DL1ABC"; 3]);
    }

    /// Your call signs your TU as you choose, in a pile-up and with one
    /// caller at a time alike: every QSO's, every third's, or none; and the
    /// callers call after each TU, signed or not, as MorseRunner's do. (The
    /// Sprint's signs every time by its rules, whatever is chosen: its row
    /// says so, and the choice is not its, `tu_call_chosen`.)
    #[test]
    fn your_call_signs_the_tu_as_you_choose() {
        use crate::contest::TuCall;
        let tus = |style: RunStyle, choice: TuCall| {
            let mut settings = run_settings(25.0);
            settings.contest.contest_style = style;
            settings.contest.contest_tu_call = choice;
            settings.contest.my_call = "K1ABC".to_string();
            settings.band.stations_min = 1;
            settings.band.stations_max = 1;
            settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
            let mut runtime = Runtime::start(1, settings.clamp(), ScriptCalls::new(&[]));
            let mut tus = Vec::new();
            for n in 0..4u32 {
                let call = format!("W{}AW", n + 1);
                work_with_enter(&mut runtime, &call, n);
                let tu = runtime
                    .turns
                    .iter()
                    .rev()
                    .find(|turn| yours(turn).starts_with("TU"))
                    .expect("a TU")
                    .clone();
                assert!(!replies(&tu).is_empty(), "{choice:?}: the next one calls");
                tus.push(yours(&tu).to_string());
            }
            tus
        };
        let (pileup, single) = (RunStyle::Pileup, RunStyle::Single);
        assert_eq!(tus(pileup, TuCall::Every), ["TU K1ABC"; 4]);
        assert_eq!(tus(pileup, TuCall::Third), ["TU", "TU", "TU K1ABC", "TU"]);
        assert_eq!(tus(pileup, TuCall::Never), ["TU"; 4]);
        assert_eq!(tus(single, TuCall::Every), ["TU K1ABC"; 4]);
        assert_eq!(tus(single, TuCall::Third), ["TU", "TU", "TU K1ABC", "TU"]);
        assert_eq!(tus(single, TuCall::Never), ["TU"; 4]);
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
        // Its sound reaches the ear later than it was started, as a retry
        // puts it off: the player could still take over at its start.
        runtime.run_to(runtime.now);
        runtime.apply(ContestEvent::TurnStarted {
            id: carried.id,
            heard_from: MonoMs(runtime.now + 120),
            output_from: MonoMs(runtime.now + 120),
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
            output_from: MonoMs(runtime.now + 120),
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
    /// only the quickest, unless its patience is spent and it has gone.
    #[test]
    fn on_a_quiet_frequency_every_caller_calls_on_its_own_timeout() {
        let (mut called, mut stations) = (0, 0);
        for seed in 0..20 {
            let mut runtime = pileup_seed(seed, &["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
            runtime.finish_turn();
            let played = runtime.turns.len();
            runtime.run_to(runtime.now + 20_000);
            // Each caller calls again, unless its patience ran out calling
            // on in the first turn and it left, as MorseRunner's do.
            let calls: Vec<(usize, bool)> = (0..3)
                .map(|station| {
                    let again = runtime.turns[played..]
                        .iter()
                        .filter(|turn| said_by(turn, station).is_some())
                        .count();
                    let left = runtime.machine.stations[station as usize].caller.state
                        == CallerState::Gone;
                    (again, left)
                })
                .collect();
            assert!(
                calls.iter().all(|(again, left)| *again > 0 || *left),
                "seed {seed}: {calls:?}"
            );
            called += calls.iter().filter(|(again, _)| *again > 0).count();
            stations += calls.len();
        }
        assert!(
            called * 10 >= stations * 9,
            "{called} of {stations} called again"
        );
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

    /// Every group the run stores, as (sent, received).
    fn copies(result: &SessionResult) -> Vec<(&str, &str)> {
        result
            .groups
            .iter()
            .map(|group| (group.sent.as_str(), group.received.as_str()))
            .collect()
    }

    /// A station you sent its call alone, then left for another caller,
    /// keeps the call you sent it as its lost copy, never the call you
    /// typed for the other: you were not working it then.
    #[test]
    fn a_station_left_for_another_keeps_the_call_you_sent_it() {
        let left = |logs: bool| {
            let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
            runtime.finish_turn();
            runtime.key(OpKey::HisCall, "K5XYZ", "");
            runtime.finish_turn();
            assert_eq!(
                runtime.machine.stations[0].caller.state,
                CallerState::NeedNr
            );
            runtime.key(OpKey::Esm, "W7ABC", "");
            assert_eq!(runtime.machine.working, Some(1));
            assert_eq!(
                runtime.machine.stations[0].heard.typed.text, "K5XYZ",
                "the call typed for W7ABC is no reading of K5XYZ"
            );
            runtime.finish_turn();
            if logs {
                work_with_enter(&mut runtime, "W7ABC", 1);
            }
            runtime.apply(ContestEvent::FinishNow);
            let (result, run) = result(&runtime);
            assert!(run.qsos.iter().any(|qso| qso.k == LOST), "{:?}", run.qsos);
            let copies = copies(&result);
            let k5xyz = copies.iter().find(|(sent, _)| *sent == "K5XYZ");
            assert_eq!(
                k5xyz,
                Some(&("K5XYZ", "K5XYZ")),
                "the call you sent it: {copies:?}"
            );
        };
        left(true);
        left(false);
    }

    /// A station that missed your exchange, and that you then left for
    /// another caller, keeps the call you sent it with your exchange.
    #[test]
    fn a_station_that_missed_your_exchange_and_was_left_keeps_its_call() {
        let mut tried = 0;
        for seed in 0..400 {
            let mut runtime = pileup_seed(seed, &["K5XYZ", "W7ABC"], 2, 2);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let asked = said_by(runtime.last_turn(), 0).map(str::to_string);
            runtime.finish_turn();
            if runtime.machine.stations[0].caller.state != CallerState::NeedNr
                || asked.as_deref().and_then(copy_of).is_some()
                || asked.is_none()
            {
                continue;
            }
            tried += 1;
            // It missed your exchange and asked for it: you go on to W7ABC.
            runtime.key(OpKey::Esm, "W7ABC", "");
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            let call_copy = result
                .groups
                .iter()
                .find(|group| group.sent == "K5XYZ")
                .map(|group| group.received.as_str());
            assert_eq!(
                call_copy,
                Some("K5XYZ"),
                "seed {seed}: {:?}",
                copies(&result)
            );
        }
        assert!(tried >= 3, "{tried}");
    }

    /// In a pile-up a station that keyed its correction over your fix never
    /// heard the fix, and corrects you again: Enter, with the call now
    /// right, sends it again rather than waiting on "Fix call", and nobody
    /// tells you to fix a call you have already put right.
    #[test]
    fn a_fix_keyed_over_the_correction_leaves_enter_working() {
        let corrected =
            ContestEffect::Announce("They corrected you: fix the call, then Enter".into());
        for seed in 0..20 {
            let mut runtime = pileup_seed(seed, &["K5XYZ"], 1, 1);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYW", "");
            let fix = sent_by(runtime.last_turn(), 0)[0].clone();
            assert!(fix.text.contains("K5XYZ"), "seed {seed}: {}", fix.text);
            runtime.run_to(runtime.now + ms(fix.start_sec + (fix.end_sec - fix.start_sec) / 4.0));
            let from = runtime.effects.len();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            // Its call, with your exchange should it have missed it.
            let sent = yours(runtime.last_turn()).to_string();
            assert!(
                sent == "K5XYZ ?" || sent == "K5XYZ 5NN TT1",
                "seed {seed}: {sent}"
            );
            runtime.finish_turn();
            assert!(
                !runtime.effects[from..].contains(&corrected),
                "seed {seed}: told to fix the call put right"
            );
            let played = runtime.turns.len();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            assert!(
                runtime.turns.len() > played,
                "seed {seed}: Enter sends nothing: \"Fix call\" with the call right"
            );
        }
    }

    /// A station you leave with a call that fits nobody is no longer the
    /// one you work: a NIL logged on that call is charged to nobody, and
    /// the clean QSO you log with the station later keeps its copy.
    #[test]
    fn a_nil_after_leaving_a_station_is_charged_to_nobody() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "JA1ZZZ", "");
        runtime.finish_turn();
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedQso
        );
        runtime.key(OpKey::TuAndLog, "JA1ZZZ", "123");
        runtime.finish_turn();
        let nr = work_with_enter(&mut runtime, "K5XYZ", 0);
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!((run.qsos[0].k.as_str(), run.qsos[0].g), (NIL, 0));
        assert_eq!(
            copies(&result),
            [("K5XYZ", "K5XYZ"), (nr.as_str(), nr.as_str())]
        );
    }

    /// B4 with a slip of a caller's call is no B4 for it: it puts its call
    /// right, as MorseRunner's caller does (`DxOper.pas:755`), and stays.
    #[test]
    fn b4_with_a_slip_of_a_callers_call_is_put_right() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::B4, "K5XYW", "");
        let b4 = runtime.last_turn().clone();
        assert_eq!(yours(&b4), "K5XYW QSO B4");
        runtime.finish_turn();
        assert_ne!(
            runtime.machine.stations[0].caller.state,
            CallerState::Gone,
            "K5XYZ sent away by a B4 meant for K5XYW"
        );
        assert!(said_by(&b4, 0).is_some_and(|said| said.contains("K5XYZ")));
    }

    /// A caller whose timeout runs out while other callers send calls then,
    /// over them, as on a real frequency, not once their sends are over:
    /// whether they call on their own timeouts or answer your CQ.
    #[test]
    fn a_timeout_while_another_caller_sends_is_not_put_off() {
        // Each call more than its reply delay past its caller's timeout, as
        // the timeouts stood before the event that put it on the air.
        let late_calls = |runtime: &mut Runtime<ScriptCalls>, for_ms: u64| {
            let mut late = Vec::new();
            let until = runtime.now + for_ms;
            while runtime.now < until {
                let deadlines: Vec<Option<u64>> = runtime
                    .machine
                    .stations
                    .iter()
                    .map(|station| station.caller.deadline)
                    .collect();
                let played = runtime.turns.len();
                if !runtime.step() {
                    break;
                }
                if runtime.turns.len() == played {
                    continue;
                }
                let turn = runtime.last_turn();
                for station in senders(turn) {
                    let Some(Some(due)) = deadlines.get(station as usize) else {
                        continue;
                    };
                    // Its first call in the turn; its reply delay is
                    // 0.1–0.6 s.
                    let first = sent_by(turn, station)[0];
                    let starts = turn.at_run_ms + ms(first.start_sec);
                    if starts > due + 600 {
                        late.push((station, starts - due));
                    }
                }
            }
            late
        };
        let mut late = Vec::new();
        // Callers calling on their own after your first CQ.
        for seed in 0..5 {
            let mut runtime = pileup_seed(seed, &["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
            runtime.finish_turn();
            late.extend(
                late_calls(&mut runtime, 20_000)
                    .into_iter()
                    .map(|l| (seed, l)),
            );
        }
        // Five answering each CQ, the quickest done long before the last,
        // and the calls going on until well after the turn would be over.
        for seed in 0..20 {
            let calls = ["K5XYZ", "W7ABC", "G4ABC", "JA1XYZ", "VK2ABC"];
            let mut runtime = pileup_seed(seed, &calls, 5, 5);
            for _ in 0..3 {
                runtime.key(OpKey::Cq, "", "");
                late.extend(
                    late_calls(&mut runtime, 30_000)
                        .into_iter()
                        .map(|l| (seed, l)),
                );
            }
        }
        assert!(
            late.is_empty(),
            "seed, (station, ms past its timeout): {late:?}"
        );
    }

    /// A turn of callers is never taken off the air but by a key of yours:
    /// a caller whose wait runs out while it is on calls in it, so no
    /// character already sounding is cut and sent again.
    #[test]
    fn a_turn_of_callers_is_never_cut_short_without_a_key() {
        for seed in 0..20 {
            let calls = ["K5XYZ", "W7ABC", "G4ABC", "JA1XYZ", "VK2ABC"];
            let mut runtime = pileup_seed(seed, &calls, 3, 5);
            runtime.finish_turn();
            let from = runtime.turns.len();
            runtime.run_to(runtime.now + 30_000);
            let turns = &runtime.turns[from - 1..];
            assert!(turns.len() >= 2, "seed {seed}: callers call again");
            for pair in turns.windows(2) {
                let ends = pair[0].at_run_ms + ms(pair[0].duration_sec());
                assert!(
                    pair[1].at_run_ms >= ends,
                    "seed {seed}: turn {} cut at {} ms, {} ms before its end",
                    pair[0].id,
                    pair[1].at_run_ms,
                    ends - pair[1].at_run_ms
                );
            }
        }
    }

    /// A `?` to the whole pile-up is charged only to the QSO it was asked
    /// in: a caller that repeated then, and that you work two QSOs on from
    /// a call it sent unasked, keeps one send.
    #[test]
    fn a_question_before_another_qso_is_not_charged_to_a_later_one() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::Again, "", "");
        assert_eq!(replies(runtime.last_turn()).len(), 2, "both repeat");
        work_with_enter(&mut runtime, "K5XYZ", 0);
        work_with_enter(&mut runtime, "W7ABC", 1);
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        assert_eq!(result.groups[2].sent, "W7ABC");
        assert_eq!(
            result.group_timings[2].sends, 1,
            "W7ABC's call, two QSOs on"
        );
    }

    /// In a pile-up, leaving a station that took your exchange for another
    /// caller: the station you work now has not taken your exchange, so
    /// Enter sends it its call and your exchange, not `?`, which it would
    /// answer with `NR?` for as long as you press Enter.
    #[test]
    fn switching_stations_sends_the_new_one_your_exchange() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        // K5XYZ takes your exchange and sends its own.
        its_number(&mut runtime, "K5XYZ", 0);
        assert!(view(&runtime).qso.exchange_sent);
        let enter = esm(
            &view(&runtime).qso,
            &Fields {
                call: "W7ABC".into(),
                ..Fields::default()
            },
            &RULES[0],
            Side::Dx,
        );
        assert_eq!(enter.label, "Send W7ABC + exch");
        // You could not copy it; W7ABC is calling, so you work W7ABC.
        runtime.key(OpKey::Esm, "W7ABC", "");
        assert_eq!(runtime.machine.working, Some(1));
        assert_eq!(yours(runtime.last_turn()), "W7ABC 5NN TT1");
    }

    /// In a pile-up, once the station you work has gone, the rest wait for a
    /// QSO that is over, and nobody calls until you CQ: Enter's hint says
    /// so, as it does once a single caller has gone.
    #[test]
    fn after_the_station_you_work_leaves_a_pile_up_the_hint_says_cq() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        its_number(&mut runtime, "K5XYZ", 0);
        let from = runtime.effects.len();
        // No TU: K5XYZ gives up after its silent timeouts.
        runtime.run_to(runtime.now + 60_000);
        let gone = ContestEffect::Announce("K5XYZ has gone".into());
        assert!(runtime.effects[from..].contains(&gone));
        let after = runtime.turns.len();
        runtime.run_to(runtime.now + 60_000);
        assert_eq!(runtime.turns.len(), after, "a minute of dead air");
        let enter = esm(&view(&runtime).qso, &Fields::default(), &RULES[0], Side::Dx);
        assert_eq!(enter.hint, "Enter calls CQ for the next station");
    }

    /// A typo that is no call (its digit dropped) goes, as MorseRunner
    /// ranks it by edit distance (`DxOper.pas:597-631`), to the call it is
    /// nearest: one edit from K5XYZ, two from K5XYA. Only K5XYZ answers.
    #[test]
    fn a_typo_that_is_no_call_goes_to_the_nearest_call() {
        let mut runtime = pileup(&["K5XYZ", "K5XYA"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "KXYZ", "");
        let asked = runtime.last_turn().clone();
        assert!(
            said_by(&asked, 0).is_some(),
            "K5XYZ, one edit away, answers"
        );
        assert_eq!(
            said_by(&asked, 1),
            None,
            "K5XYA, two edits away, keeps quiet"
        );
    }

    /// A caller on its last try that answers your partial, and that you
    /// then key over, stays in the QSO for Enter: the partial gave it full
    /// patience, and being keyed over sends nobody away. It calls again,
    /// and takes your call and exchange sent again.
    #[test]
    fn a_caller_keyed_over_on_its_last_try_stays_in_the_qso() {
        let mut runtime = pileup(&["K5XYZ"], 1, 1);
        runtime.finish_turn();
        while runtime.machine.stations[0].caller.calls_on_time_out(0) {
            runtime.key(OpKey::Cq, "", "");
            runtime.finish_turn();
        }
        runtime.key(OpKey::Esm, "K5?", "");
        let answer = sent_by(runtime.last_turn(), 0)[0].clone();
        assert_eq!(answer.text, "K5XYZ");
        assert_eq!(runtime.machine.working, Some(0));
        // Over the start of its answer: its call and your exchange.
        runtime.run_to(runtime.now + ms(answer.start_sec + 0.05));
        runtime.key(OpKey::HisCallAndExchange, "K5XYZ", "");
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedQso
        );
        assert_eq!(runtime.machine.working, Some(0), "still worked");
        assert_eq!(view(&runtime).qso.call_sent, "K5XYZ");
        let again = sent_by(runtime.last_turn(), 0);
        assert_eq!(again.last().map(|s| s.text.as_str()), Some("K5XYZ"));
        runtime.finish_turn();
        its_number(&mut runtime, "K5XYZ", 0);
    }

    /// In a pile-up a caller that only answered your partial is no QSO yet:
    /// a NIL whose call is another heard station's is not charged to it, and
    /// the clean QSO you log with it later keeps its copy.
    #[test]
    fn a_caller_that_only_answered_a_partial_is_charged_no_nil() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "G4?", "");
        assert!(said_by(runtime.last_turn(), 2).is_some_and(|said| said.contains("G4ABC")));
        runtime.finish_turn();
        runtime.key(OpKey::LogOnly, "W7ABC", "123");
        let nr = work_with_enter(&mut runtime, "G4ABC", 2);
        assert_eq!(
            view(&runtime).log[1].status,
            RowStatus::Checked(QsoCheck::default())
        );
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        let copies = copies(&result);
        assert!(
            !copies.contains(&("G4ABC", "W7ABC")),
            "G4ABC never heard its call, and the NIL's call is W7ABC's: {copies:?}"
        );
        assert!(
            copies.contains(&("G4ABC", "G4ABC")) && copies.contains(&(nr.as_str(), nr.as_str())),
            "G4ABC's clean QSO keeps its copy: {copies:?}"
        );
    }

    /// A partial to the next caller after your TU without logging is no
    /// QSO with it yet: the QSO the TU closed still waits to be logged, and
    /// logs clean.
    #[test]
    fn a_partial_after_tu_without_logging_keeps_that_qso_to_log() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 2, 2);
        let nr = its_number(&mut runtime, "K5XYZ", 0);
        runtime.key(OpKey::Tu, "K5XYZ", &nr);
        runtime.finish_turn();
        assert_eq!(runtime.machine.thanked, Some(0));
        runtime.key(OpKey::Esm, "W7?", "");
        assert!(said_by(runtime.last_turn(), 1).is_some_and(|said| said.contains("W7ABC")));
        runtime.finish_turn();
        runtime.key(OpKey::LogOnly, "K5XYZ", &nr);
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        let kinds: Vec<&str> = run.qsos.iter().map(|qso| qso.k.as_str()).collect();
        assert_eq!(
            view(&runtime).log[0].status,
            RowStatus::Checked(QsoCheck::default()),
            "{kinds:?} {:?}",
            copies(&result)
        );
    }

    /// A call typed for another station, at a key that sends no call, is
    /// that station's, never a copy of the one you work: once the station
    /// you work has gone, it is named by the call you had for it, and keeps
    /// that call as its copy, whether the key was a wipe, a `?`, a log
    /// that cannot log yet, or a call to the other taken back with Esc.
    #[test]
    fn a_call_typed_for_another_station_is_no_copy_of_the_one_you_work() {
        // Esc stands for Enter on W7ABC taken back at once.
        for key in [OpKey::Wipe, OpKey::Again, OpKey::LogOnly, OpKey::StopTx] {
            let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
            runtime.finish_turn();
            runtime.key(OpKey::HisCall, "K5XYZ", "");
            runtime.finish_turn();
            assert_eq!(
                runtime.machine.stations[0].caller.state,
                CallerState::NeedNr
            );
            match key {
                OpKey::StopTx => {
                    runtime.key(OpKey::Esm, "W7ABC", "");
                    runtime.key(OpKey::StopTx, "W7ABC", "");
                }
                key => {
                    runtime.key(key, "W7ABC", "");
                }
            }
            runtime.finish_turn();
            assert_eq!(
                runtime.machine.stations[0].heard.typed.text, "K5XYZ",
                "{key:?}"
            );
            // K5XYZ, waiting for your exchange, would wait on: a NIL with the
            // Call field empty sends it away, and W7ABC is still there.
            runtime.key(OpKey::Nil, "", "");
            runtime.finish_turn();
            assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
            assert_eq!(
                runtime.machine.stations[1].caller.state,
                CallerState::NeedQso
            );
            assert_eq!(view(&runtime).qso.gone, ["K5XYZ"], "{key:?}");
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            let copies = copies(&result);
            let k5xyz = copies.iter().find(|(sent, _)| *sent == "K5XYZ");
            assert_eq!(k5xyz, Some(&("K5XYZ", "K5XYZ")), "{key:?}: {copies:?}");
        }
    }

    /// A station you sent its call alone, then left with a slip of the next
    /// caller's call, which that caller took as close: the station you left
    /// keeps the call you sent it, read before the slip went out to the
    /// other, never the slip.
    #[test]
    fn a_station_left_with_a_slip_of_the_next_call_keeps_the_call_you_sent_it() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "W7ABD", "");
        assert_eq!(runtime.machine.working, Some(1), "W7ABC takes the slip");
        assert_eq!(runtime.machine.stations[0].heard.typed.text, "K5XYZ");
        runtime.finish_turn();
        let lost = runtime.machine.nil_and_lost_copies();
        runtime.apply(ContestEvent::FinishNow);
        assert!(runtime.machine.crossed_since(&lost).is_empty());
        let (result, _) = result(&runtime);
        let copies = copies(&result);
        let k5xyz = copies.iter().find(|(sent, _)| *sent == "K5XYZ");
        assert_eq!(k5xyz, Some(&("K5XYZ", "K5XYZ")), "{copies:?}");
    }

    /// A reading of the station you work is checked against the stations
    /// there when its copy is lost: one that joins later and is sent that
    /// call is no witness that it was typed for it.
    #[test]
    fn a_copy_is_crossed_only_by_a_station_there_when_it_is_lost() {
        let mut runtime = pileup(&["K5XYA", "W7ABC", "K5XZ"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYA", "");
        runtime.finish_turn();
        // K5XZ is no call on the frequency yet: a reading of K5XYA.
        runtime.key(OpKey::Again, "K5XZ", "");
        runtime.finish_turn();
        let lost = runtime.machine.nil_and_lost_copies();
        // A NIL with the Call field empty sends it away.
        runtime.key(OpKey::Nil, "", "");
        runtime.finish_turn();
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
        assert!(runtime.machine.crossed_since(&lost).is_empty());
        let lost = runtime.machine.nil_and_lost_copies();
        assert_eq!(lost, [(0, "K5XZ".to_string())]);
        runtime.key(OpKey::Cq, "", "");
        runtime.finish_turn();
        assert!(runtime.machine.stations.len() > 2, "K5XZ joins");
        runtime.key(OpKey::HisCall, "K5XZ", "");
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        assert!(runtime.machine.crossed_since(&lost).is_empty());
        // Looked at only now, K5XZ would be taken for a witness.
        assert!(
            runtime
                .machine
                .crossed_since(&[])
                .contains(&(0, "K5XZ".to_string()))
        );
    }

    /// A slip as near two callers' calls reaches both, and you work the
    /// last: N is whether that station took your exchange, not whether the
    /// other did, so Enter sends it its call and your exchange rather than
    /// `?` for as long as you press it.
    #[test]
    fn in_a_tie_n_is_whether_the_station_you_work_took_your_exchange() {
        let mut tried = 0;
        for seed in 0..200 {
            let mut runtime = pileup_seed(seed, &["W7ABC", "G4ABC"], 2, 2);
            runtime.finish_turn();
            // Two edits from each: a tie.
            runtime.key(OpKey::Esm, "F5ABC", "");
            assert_eq!(runtime.machine.working, Some(1), "seed {seed}");
            let states: Vec<CallerState> = runtime
                .machine
                .stations
                .iter()
                .map(|station| station.caller.state)
                .collect();
            // W7ABC took your exchange; G4ABC, the one you work, missed it.
            if states != [CallerState::NeedCall, CallerState::NeedCallNr] {
                continue;
            }
            tried += 1;
            assert!(!view(&runtime).qso.exchange_sent, "seed {seed}");
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "G4ABC", "");
            assert_eq!(yours(runtime.last_turn()), "G4ABC 5NN TT1", "seed {seed}");
        }
        assert!(tried >= 3, "{tried}");
    }

    /// Your exchange sent with nobody worked, as F2 after logging without
    /// sending while the station logged still waits for your TU, goes to no
    /// QSO of Enter's: Enter sends the next caller its call and your
    /// exchange.
    #[test]
    fn your_exchange_with_nobody_worked_leaves_n_alone() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        let nr = its_number(&mut runtime, "K5XYZ", 0);
        runtime.key(OpKey::LogOnly, "K5XYZ", &nr);
        assert_eq!(runtime.machine.working, None);
        runtime.key(OpKey::Exchange, "", "");
        assert_eq!(yours(runtime.last_turn()), "5NN TT2");
        assert!(!view(&runtime).qso.exchange_sent);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "W7ABC", "");
        assert_eq!(yours(runtime.last_turn()), "W7ABC 5NN TT2");
    }

    /// Once the station you work has sent its exchange for the call you
    /// sent, that call is its own: a near call is another caller's, so
    /// Enter sends it its call and your exchange, never a TU that logs it
    /// with the last station's number, nor a `?` it would answer with AGN.
    #[test]
    fn a_near_call_once_the_station_took_yours_is_another_callers() {
        let mut runtime = pileup(&["K5XYZ", "K5XYA"], 2, 2);
        let nr = its_number(&mut runtime, "K5XYZ", 0);
        let qso = view(&runtime).qso;
        assert!(qso.exchange_sent && qso.call_taken);
        let enter = |call: &str, exch: &str| {
            let fields = Fields {
                call: call.into(),
                exch: exch.into(),
                ..Fields::default()
            };
            esm(&qso, &fields, &RULES[0], Side::Dx).label
        };
        assert_eq!(enter("K5XYA", &nr), "Send K5XYA + exch");
        assert_eq!(enter("K5XYA", ""), "Send K5XYA + exch");
        assert_eq!(enter("K5XYZ", &nr), "TU & log");
        runtime.key(OpKey::Esm, "K5XYA", &nr);
        assert_eq!(yours(runtime.last_turn()), "K5XYA 5NN TT1");
        assert_eq!(runtime.machine.working, Some(1));
        assert!(view(&runtime).log.is_empty(), "nothing logged");
        // Before it answers, a near call may be the slip put right.
        assert!(!view(&runtime).qso.call_taken);
    }

    /// A slip sent with your exchange is the call's copy even when the
    /// station missed the exchange: it is still the call first sent with
    /// your exchange, put right after.
    #[test]
    fn a_slip_sent_with_an_exchange_the_station_missed_is_its_copy() {
        let mut tried = 0;
        for seed in 0..300 {
            let mut runtime = pileup_seed(seed, &["K5XYZ"], 1, 1);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYW", "");
            runtime.finish_turn();
            // Taken, it would want its call put right with your exchange in
            // hand; missed, it still waits for both.
            if runtime.machine.stations[0].caller.state != CallerState::NeedCallNr {
                continue;
            }
            tried += 1;
            work_with_enter(&mut runtime, "K5XYZ", 0);
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            assert_eq!(
                (
                    result.groups[0].sent.as_str(),
                    result.groups[0].received.as_str()
                ),
                ("K5XYZ", "K5XYW"),
                "seed {seed}"
            );
        }
        assert!(tried >= 3, "{tried}");
    }

    /// A caller still calling that answers your partial is given fresh
    /// patience before the try it spends, as MorseRunner's near match puts
    /// it in a QSO with full patience (`DxOper.pas:496-498, 736`): even on
    /// its last try, the station your partial asked for answers it.
    #[test]
    fn a_caller_on_its_last_try_answers_your_partial() {
        let mut runtime = pileup(&["K5XYZ"], 1, 1);
        runtime.finish_turn();
        // Each CQ costs it a try, as MorseRunner's DecPatience: CQ until it
        // has made its last call.
        while runtime.machine.stations[0].caller.calls_on_time_out(0) {
            runtime.key(OpKey::Cq, "", "");
            runtime.finish_turn();
        }
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedQso
        );
        // You heard part of its call: K5?
        runtime.key(OpKey::Esm, "K5?", "");
        assert!(
            said_by(runtime.last_turn(), 0).is_some_and(|said| said.contains("K5XYZ")),
            "the partial's only match answers: {:?}",
            replies(runtime.last_turn())
        );
        assert_ne!(runtime.machine.stations[0].caller.state, CallerState::Gone);
    }

    /// A caller you key over on its last call is not sent away: hearing
    /// only your message's tail, it takes more patience first, as
    /// MorseRunner's callers do on garbage (`DxOper.pas:805-808`), and
    /// calls again a moment after you.
    #[test]
    fn a_caller_keyed_over_on_its_last_call_calls_again() {
        let mut runtime = pileup(&["K5XYZ"], 1, 1);
        runtime.finish_turn();
        // CQ until its answer to a CQ is its last call.
        loop {
            runtime.key(OpKey::Cq, "", "");
            if !runtime.machine.stations[0].caller.calls_on_time_out(0) {
                break;
            }
            runtime.finish_turn();
        }
        let answer = sent_by(runtime.last_turn(), 0)[0].clone();
        assert_eq!(answer.text, "K5XYZ");
        // Its call and your exchange, keyed over the start of its call.
        runtime.run_to(runtime.now + ms(answer.start_sec + 0.05));
        runtime.key(OpKey::HisCallAndExchange, "K5XYZ", "");
        assert_ne!(
            runtime.machine.stations[0].caller.state,
            CallerState::Gone,
            "keyed over, it stays"
        );
        assert_eq!(
            sent_by(runtime.last_turn(), 0).len(),
            2,
            "its rest, then its call again a moment after you"
        );
    }

    /// A pile-up caller that waited out your QSO with another station comes
    /// back to your CQ with fresh patience, as MorseRunner's `osNeedPrevEnd`
    /// does (`DxOper.pas:708`): one on its last try before that QSO began
    /// still calls.
    #[test]
    fn a_cq_after_another_qso_gives_the_callers_who_waited_fresh_patience() {
        let mut checked = 0;
        for seed in 0..40 {
            let mut runtime = pileup_seed(seed, &["K5XYZ", "W7ABC"], 2, 2);
            runtime.finish_turn();
            // W7ABC down to its last call, K5XYZ still calling.
            while runtime.machine.stations[1].caller.calls_on_time_out(0)
                && runtime.machine.stations[0].caller.calls_on_time_out(0)
            {
                runtime.key(OpKey::Cq, "", "");
                runtime.finish_turn();
            }
            if runtime.machine.stations[1].caller.calls_on_time_out(0)
                || runtime.machine.stations[1].caller.state != CallerState::NeedQso
                || runtime.machine.stations[0].caller.state != CallerState::NeedQso
            {
                continue;
            }
            checked += 1;
            // You work K5XYZ; W7ABC waits for that QSO to end.
            runtime.key(OpKey::Esm, "K5XYZ", "");
            runtime.finish_turn();
            assert!(runtime.machine.stations[1].caller.waits(), "seed {seed}");
            // You give up on K5XYZ and call CQ.
            runtime.key(OpKey::Cq, "", "");
            assert_ne!(
                runtime.machine.stations[1].caller.state,
                CallerState::Gone,
                "seed {seed}: W7ABC leaves on your CQ"
            );
            assert!(
                said_by(runtime.last_turn(), 1).is_some(),
                "seed {seed}: W7ABC, who waited, calls after your CQ"
            );
        }
        assert!(checked > 0);
    }

    /// A station that took your exchange with a slip of its call puts the
    /// call right with its exchange, five operators in six, as
    /// MorseRunner's `osNeedCall` reply does (`DxOper.pas:851-867`).
    #[test]
    fn a_correction_after_your_exchange_carries_its_exchange() {
        let mut corrections = Vec::new();
        for seed in 0..30 {
            let mut runtime = pileup_seed(seed, &["K5XYZ"], 1, 1);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYW", "");
            assert!(yours(runtime.last_turn()).starts_with("K5XYW 5NN"));
            if runtime.machine.stations[0].caller.state != CallerState::NeedCall {
                continue; // it missed your exchange
            }
            corrections.extend(said_by(runtime.last_turn(), 0).map(str::to_string));
        }
        assert!(
            corrections.iter().any(|said| said.contains("5NN")),
            "no correction carries its exchange: {corrections:?}"
        );
    }

    /// Step on in tenths of a second, and key Enter with `call` in the Call
    /// field as soon as station 0 is silent while one of `others` sends.
    /// Whether it keyed.
    fn key_while_others_call(
        runtime: &mut Runtime<ScriptCalls>,
        others: std::ops::Range<u32>,
        call: &str,
    ) -> bool {
        for _ in 0..400 {
            runtime.run_to(runtime.now + 100);
            let Some(air) = runtime.machine.air.as_ref() else {
                continue;
            };
            let at = (runtime.now - air.heard_from.0) as f64 / 1_000.0;
            let sending = |station: u32| {
                air.turn.segments.iter().any(|s| {
                    s.who == Sender::Caller(station) && s.start_sec <= at && at < s.end_sec
                })
            };
            if !sending(0) && others.clone().any(sending) {
                runtime.key(OpKey::Esm, call, "");
                return true;
            }
        }
        false
    }

    /// Enter decides on what you have heard of the turn on the air, not on
    /// what had been counted when it began: once the station you work has
    /// sent its exchange, heard whole, a near call is another caller's,
    /// whether or not another caller is still sounding.
    #[test]
    fn enter_reads_the_answer_heard_while_the_turn_is_still_on() {
        let mut tried = 0;
        for seed in 0..200 {
            let calls = ["K5XYZ", "K5XYA", "W7ABC", "G4ABC", "JA1XYZ"];
            let mut runtime = pileup_seed(seed, &calls, 5, 5);
            runtime.finish_turn();
            if !key_while_others_call(&mut runtime, 2..5, "K5XYZ") {
                continue;
            }
            let turn = runtime.last_turn().clone();
            let Some(answer) = sent_by(&turn, 0).first().copied().cloned() else {
                continue;
            };
            let Some(Copied::Exchange(nr)) = copy_of(&answer.text) else {
                continue;
            };
            // The turn goes on after K5XYZ's exchange: a caller you keyed
            // over calls again.
            if turn.duration_sec() < answer.end_sec + 0.5 {
                continue;
            }
            tried += 1;
            runtime.run_to(runtime.now + ms(answer.end_sec + 0.2));
            assert_eq!(runtime.playing_id(), Some(turn.id), "seed {seed}");
            // The screen has heard it too: Enter's label follows the audio.
            assert!(view(&runtime).qso.call_taken, "seed {seed}");
            runtime.key(OpKey::Esm, "K5XYA", &nr);
            assert_eq!(
                (yours(runtime.last_turn()), view(&runtime).log.len()),
                ("K5XYA 5NN TT1", 0),
                "seed {seed}: K5XYZ answered with its exchange, heard whole"
            );
        }
        assert!(tried >= 1, "{tried}");
    }

    /// Enter reads the answer of the station you work from the moment it
    /// begins to reach you, as the key counts what has, not once the heard
    /// timer, a UI timer that may fire late, has counted it: a near call is
    /// then another caller's, and nothing is logged.
    #[test]
    fn enter_reads_an_answer_that_has_begun_before_its_timer() {
        let mut runtime = pileup(&["K5XYZ", "K5XYA"], 2, 2);
        runtime.finish_turn();
        let mut answered = None;
        for _ in 0..20 {
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let answer = sent_by(runtime.last_turn(), 0).first().copied().cloned();
            if let Some(answer) = answer
                && let Some(Copied::Exchange(nr)) = copy_of(&answer.text)
            {
                answered = Some((answer, nr));
                break;
            }
            runtime.finish_turn();
        }
        let (answer, nr) = answered.expect("K5XYZ sends its exchange");
        // Its number sounds, and nothing due since has been delivered: the
        // heard timer is late.
        runtime.run_to(runtime.now);
        runtime.now += ms(answer.start_sec + 0.3);
        assert!(!view(&runtime).qso.call_taken, "nothing has counted it yet");
        runtime.key(OpKey::Esm, "K5XYA", &nr);
        assert_eq!(
            (yours(runtime.last_turn()), view(&runtime).log.len()),
            ("K5XYA 5NN TT1", 0)
        );
    }

    /// A logged QSO's check shows once your TU is out and the station has
    /// had its say, not when the pile-up that goes on calling after it
    /// falls quiet.
    #[test]
    fn a_logged_qsos_check_shows_while_the_pile_up_calls_on() {
        let mut late = Vec::new();
        for seed in 0..30 {
            let mut runtime = pileup_seed(seed, &["K5XYZ"], 4, 5);
            let nr = its_number(&mut runtime, "K5XYZ", 0);
            let at = runtime.now;
            runtime.key(OpKey::Esm, "K5XYZ", &nr);
            assert!(yours(runtime.last_turn()).starts_with("TU"));
            // Nothing more from you for six seconds.
            runtime.run_to(at + 6_000);
            if view(&runtime).log[0].status == RowStatus::Pending {
                late.push((seed, runtime.last_turn().duration_sec()));
            }
        }
        assert!(late.is_empty(), "(seed, TU turn's length in s): {late:?}");
    }

    /// The band's own traffic stands by for as many callers as sound at
    /// once in a turn, not for every send planned into it: two callers
    /// calling on, call after call, in one turn are two.
    #[test]
    fn a_turn_never_clears_the_band_for_more_callers_than_call_at_once() {
        let mut cleared = Vec::new();
        for seed in 0..20 {
            // The Band card's range 2–5: the band's own traffic may key four
            // stations at once.
            let mut runtime = pileup_seed(seed, &["K5XYZ", "W7ABC"], 2, 5);
            let settings = runtime.machine.settings.clone();
            runtime.run_to(runtime.now + 40_000);
            for turn in &runtime.turns {
                let planned = crate::contest::turn::plan_turn(turn, &settings);
                // Each caller from its segment's start to its last fall:
                // sounding till then, past its last key-up.
                let callers: Vec<(f64, f64)> = turn
                    .segments
                    .iter()
                    .filter(|s| s.who != Sender::You)
                    .zip(&planned.others)
                    .map(|(segment, plan)| {
                        let fell = plan
                            .events
                            .last()
                            .map_or(segment.end_sec, ToneEvent::end_sec);
                        (segment.start_sec, fell.max(segment.end_sec))
                    })
                    .collect();
                // The most callers sounding at any moment of the turn.
                let at_once = callers
                    .iter()
                    .map(|(start, _)| {
                        callers
                            .iter()
                            .filter(|(from, to)| from <= start && start < to)
                            .count()
                    })
                    .max()
                    .unwrap_or(0);
                // What the players hand the band to make room for.
                let reserved = planned.others_at_once();
                if reserved != at_once {
                    cleared.push((seed, turn.id, reserved, at_once));
                }
            }
        }
        assert!(
            cleared.is_empty(),
            "(seed, turn, callers the band stands by for, most calling at once): {cleared:?}"
        );
    }

    /// A QSO logged for a station you left, done with you on another call,
    /// is that station's QSO, as MorseRunner's log finds any station done
    /// whose call it is (`Log.pas:752-760`): never a NIL charged to the
    /// station you work, whose own clean QSO keeps its copy. So is one
    /// logged with a slip of its call: the call is still its, not the
    /// station's you work.
    #[test]
    fn a_qso_logged_for_a_station_you_left_is_no_copy_of_the_one_you_work() {
        for logged in ["K5XYZ", "K5XYW", "K5XZ"] {
            let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
            let nr = its_number(&mut runtime, "K5XYZ", 0);
            runtime.key(OpKey::Esm, "W7ABC", "");
            runtime.finish_turn();
            assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Done);
            assert_eq!(runtime.machine.working, Some(1));
            runtime.key(OpKey::LogOnly, logged, &nr);
            let w7abc = its_number(&mut runtime, "W7ABC", 1);
            runtime.key(OpKey::Esm, "W7ABC", &w7abc);
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            let (result, run) = result(&runtime);
            let copies = copies(&result);
            let (nr, w7abc) = (nr.as_str(), w7abc.as_str());
            assert!(
                !copies.contains(&("W7ABC", logged)) && !copies.contains(&(w7abc, nr)),
                "{logged}: K5XYZ's QSO kept as W7ABC's copy: {copies:?}"
            );
            assert!(
                copies.contains(&("W7ABC", "W7ABC")) && copies.contains(&(w7abc, w7abc)),
                "{logged}: W7ABC's own copy: {copies:?}"
            );
            assert_eq!(run.qsos[1].lc, logged, "the record holds the call");
        }
    }

    /// A NIL with a call like nobody's, logged while a dupe calls, is
    /// charged to nobody: not to the dupe, whose copy is kept already, nor
    /// to an older caller that left unanswered (§9.1 of the design).
    #[test]
    fn a_nil_while_a_dupe_calls_is_charged_to_no_older_caller() {
        let mut runtime = runtime(&["K5XYZ", "W7ABC"]);
        work_with_enter(&mut runtime, "K5XYZ", 0);
        // W7ABC calls after the TU, and gives up unanswered.
        while runtime
            .machine
            .stations
            .get(1)
            .is_none_or(|station| station.caller.state != CallerState::Gone)
        {
            assert!(runtime.step(), "W7ABC never gave up");
        }
        let mut before = Vec::new();
        let run_ms = view(&runtime).run_ms(MonoMs(runtime.now));
        assert_eq!(runtime.machine.rejoin(0, &mut before, &[], run_ms), Some(0));
        runtime.key(OpKey::Cq, "", "");
        assert_eq!(replies(runtime.last_turn()), ["K5XYZ"], "the dupe calls");
        runtime.finish_turn();
        runtime.key(OpKey::LogOnly, "DL9ZZ", "123");
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert!(
            result.groups.iter().all(|group| group.sent != "W7ABC"),
            "W7ABC charged a NIL: {:?}",
            copies(&result)
        );
        assert_eq!(run.qsos[1].lc, "DL9ZZ");
    }

    /// A NIL logged while you work a dupe adds nothing to the dupe, whose
    /// copy is kept already: one with a call like nobody's is charged to no
    /// one, its call kept in the record; one with a slip of a caller's call
    /// is that caller's, as one logged on a caller before you work it is
    /// (§9.1 of the design).
    #[test]
    fn a_nil_while_you_work_a_dupe_is_charged_to_no_one_but_the_caller_it_fits() {
        for logged in ["JA1ZZZ", "W7ABD"] {
            let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
            let nr = work_with_enter(&mut runtime, "K5XYZ", 0);
            let mut before = Vec::new();
            let run_ms = view(&runtime).run_ms(MonoMs(runtime.now));
            assert_eq!(runtime.machine.rejoin(0, &mut before, &[], run_ms), Some(0));
            runtime.key(OpKey::Cq, "", "");
            its_number(&mut runtime, "K5XYZ", 0);
            assert_eq!(runtime.machine.qso_station(), Some(0), "the dupe is worked");
            runtime.key(OpKey::LogOnly, logged, "123");
            runtime.apply(ContestEvent::FinishNow);
            let (result, run) = result(&runtime);
            let copies = copies(&result);
            assert_eq!(
                copies[..2],
                [("K5XYZ", "K5XYZ"), (nr.as_str(), nr.as_str())]
            );
            let nil = &run.qsos[1];
            if logged == "W7ABD" {
                assert_eq!((nil.g, copies[2]), (2, ("W7ABC", "W7ABD")), "{copies:?}");
                assert_eq!(copies[3].1, "", "its number never reached you");
            } else {
                assert_eq!((nil.g, nil.lc.as_str(), copies.len()), (0, logged, 2));
            }
        }
    }

    /// A caller you keyed over goes on with one send when you key again,
    /// even once the sound has been tried again since its next send was
    /// counted as begun: a station never sends over itself.
    #[test]
    fn a_station_goes_on_with_one_send_when_the_sound_is_tried_again() {
        let mut runtime = pileup(&["K5XYZ"], 1, 1);
        let cq = runtime.last_turn().clone();
        let call = sent_by(&cq, 0)[0].clone();
        runtime.run_to(runtime.now + ms(call.start_sec + (call.end_sec - call.start_sec) / 3.0));
        // Keyed over: its rest, then its call again a moment after you.
        runtime.key(OpKey::HisCallAndExchange, "K5XYZ", "");
        let turn = runtime.last_turn().clone();
        let sends: Vec<Segment> = sent_by(&turn, 0).into_iter().cloned().collect();
        assert_eq!(sends.len(), 2);
        // Its call again has begun to reach you, and a key that sends
        // nothing counts it; then the sound is tried again from its start.
        runtime.run_to(runtime.now + ms(sends[1].start_sec + 0.05));
        runtime.key(OpKey::Wipe, "", "");
        runtime.apply(ContestEvent::TurnStarted {
            id: turn.id,
            heard_from: MonoMs(runtime.now),
            output_from: MonoMs(runtime.now),
        });
        runtime.key(OpKey::Cq, "", "");
        let after = sent_by(runtime.last_turn(), 0);
        for (at, send) in after.iter().enumerate() {
            for earlier in &after[..at] {
                assert!(
                    send.start_sec >= earlier.end_sec,
                    "K5XYZ sends over itself: {after:?}"
                );
            }
        }
    }

    /// A correction heard while the turn is still on, other callers calling
    /// after it, is said then, and Enter reads "Fix call" then, not once the
    /// pile-up falls quiet.
    #[test]
    fn a_correction_is_heard_while_the_turn_is_still_on() {
        let corrected =
            ContestEffect::Announce("They corrected you: fix the call, then Enter".into());
        let mut tried = 0;
        for seed in 0..200 {
            let calls = ["K5XYZ", "W7ABC", "G4ABC", "JA1XYZ", "VK2ABC"];
            let mut runtime = pileup_seed(seed, &calls, 5, 5);
            runtime.finish_turn();
            // Keyed over another caller, which calls again after you.
            if !key_while_others_call(&mut runtime, 1..5, "K5XYW") {
                continue;
            }
            let turn = runtime.last_turn().clone();
            let Some(fix) = sent_by(&turn, 0).first().copied().cloned() else {
                continue;
            };
            if !matches!(copy_of(&fix.text), Some(Copied::Correction("K5XYZ", _)))
                || turn.duration_sec() < fix.end_sec + 0.5
            {
                continue;
            }
            tried += 1;
            let from = runtime.effects.len();
            runtime.run_to(runtime.now + ms(fix.end_sec + 0.2));
            assert_eq!(runtime.playing_id(), Some(turn.id), "seed {seed}");
            assert!(view(&runtime).qso.corrected, "seed {seed}");
            assert!(runtime.effects[from..].contains(&corrected), "seed {seed}");
            let fields = Fields {
                call: "K5XYW".into(),
                ..Fields::default()
            };
            let enter = esm(&view(&runtime).qso, &fields, &RULES[0], Side::Dx);
            assert_eq!(enter.label, "Fix call", "seed {seed}");
        }
        assert!(tried >= 1, "{tried}");
    }

    /// A dupe still calling when B4 is keyed never hears its call or the
    /// B4. B4 again then goes out alone, as its call counts as sent, and
    /// the dupe, still calling, takes it as the pile-up's cue to call
    /// again, with fresh patience: it never leaves.
    #[test]
    fn b4_keyed_over_a_dupe_still_calling_sends_its_call_again() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 1, 1);
        work_with_enter(&mut runtime, "K5XYZ", 0);
        let mut before = Vec::new();
        let run_ms = view(&runtime).run_ms(MonoMs(runtime.now));
        assert_eq!(runtime.machine.rejoin(0, &mut before, &[], run_ms), Some(0));
        runtime.finish_turn();
        let at = runtime.now;
        runtime.key(OpKey::Cq, "", "");
        let dupe = sent_by(runtime.last_turn(), 0)[0].clone();
        assert!(dupe.text.starts_with("K5XYZ"), "{}", dupe.text);
        // F6 while the dupe still calls: it never hears its call or the B4.
        runtime.run_to(at + ms((dupe.start_sec + dupe.end_sec) / 2.0));
        runtime.key(OpKey::B4, "K5XYZ", "");
        assert_eq!(yours(runtime.last_turn()), "K5XYZ QSO B4");
        runtime.finish_turn();
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::NeedQso
        );
        // B4 again, the dupe's call still in the field.
        runtime.key(OpKey::B4, "K5XYZ", "");
        let b4 = runtime.last_turn().clone();
        runtime.finish_turn();
        assert_eq!(
            runtime.machine.stations[0].caller.state,
            CallerState::Gone,
            "sent {:?}; the dupe answered {:?}",
            yours(&b4),
            said_by(&b4, 0)
        );
    }

    /// Esc on a message before any of it has gone out whole: MorseRunner's
    /// callers copy the aborted send as garbage (`MyStn.pas:108-116` sets
    /// `Msg := [msgGarbage]` and fires `evMsgSent`) and answer it after
    /// their reply delay, 0.1-0.6 s (`DxStn.pas:240-275`,
    /// `DxOper.pas:805-817`), so the pile-up comes straight back.
    #[test]
    fn esc_brings_the_pile_up_back_at_once_as_garbage_does() {
        for seed in 0..20 {
            let mut runtime = pileup_seed(seed, &["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            runtime.now += 50;
            let esc_at = runtime.now;
            let turns = runtime.turns.len();
            runtime.key(OpKey::StopTx, "K5XYZ", "");
            while runtime.turns.len() == turns {
                assert!(runtime.step(), "seed {seed}: nobody calls again");
            }
            let first = runtime
                .last_turn()
                .segments
                .iter()
                .map(|segment| segment.start_sec)
                .fold(f64::INFINITY, f64::min);
            let quiet = runtime.now - esc_at + ms(first);
            assert!(
                quiet <= 1_000,
                "seed {seed}: the pile-up keeps quiet {quiet} ms after Esc"
            );
        }
    }

    /// Esc stops your station alone, as MorseRunner's `AbortSend` does
    /// (`MyStn.pas:108-116`): a CQ keyed over three callers and stopped
    /// half a second later, all three still sending, leaves each going on
    /// at once from where it was, in a turn with nothing of yours, and
    /// waiting from the end of its send before it calls again. A second
    /// Esc, nothing of yours on the air, stops nothing: they go on as they
    /// are. Esc used to silence all three, and with nobody left listening
    /// to answer the garbage, the frequency kept quiet for a whole reply
    /// timeout.
    #[test]
    fn esc_leaves_the_callers_you_keyed_over_sending() {
        let mut runtime = pileup_qsk(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        let cq = runtime.last_turn().clone();
        let last = (0..3)
            .map(|station| sent_by(&cq, station)[0].start_sec)
            .fold(0.0, f64::max);
        runtime.run_to(ms(last + 0.1));
        let effects = runtime.key(OpKey::Cq, "", "");
        let over = played(&effects).expect("your CQ").clone();
        runtime.now += 500;
        let effects = runtime.key(OpKey::StopTx, "", "");
        assert_eq!(effects[0], ContestEffect::StopAudio);
        let going = played(&effects).expect("the callers go on").clone();
        assert_eq!(yours(&going), "", "nothing of yours");
        assert_eq!(going.at_run_ms, over.at_run_ms + 500, "at once");
        assert_eq!(going.starts_at, None);
        assert_eq!(senders(&going), BTreeSet::from([0, 1, 2]));
        let mut rests = Vec::new();
        for station in 0..3 {
            let was = sent_by(&over, station)[0];
            assert!(was.start_sec == 0.0 && was.end_sec > 0.5, "{was:?}");
            let sends = sent_by(&going, station);
            assert_eq!(sends.len(), 1, "station {station} sends nothing more in it");
            let rest = sends[0].clone();
            assert_eq!(rest.start_sec, 0.0);
            assert!(
                was.text.ends_with(&rest.text) && rest.text.len() < was.text.len(),
                "{} then {}",
                was.text,
                rest.text
            );
            // From where it was, as over a key: the character sounding sent
            // again whole, or in a space the next one at once, never from
            // its start.
            let space = word_gap_sec(&runtime.machine.settings, &was.voice);
            assert!(
                rest.end_sec < was.end_sec && rest.end_sec >= was.end_sec - 0.5 - space,
                "station {station}: {:.3} s left of {:.3} s",
                rest.end_sec,
                was.end_sec
            );
            assert_eq!(rest.voice, was.voice);
            rests.push(rest);
        }
        let callers = &runtime.machine.stations;
        assert!(
            callers
                .iter()
                .all(|station| station.caller.state == CallerState::NeedQso)
        );
        // Again, 0.3 s on: nothing of yours to stop, so they go on as they
        // are.
        runtime.run_to(runtime.now + 300);
        assert!(runtime.key(OpKey::StopTx, "", "").is_empty());
        assert_eq!(runtime.playing_id(), Some(going.id));
        // Each waits from the end of its send, then calls again.
        runtime.finish_turn();
        let heard = runtime.take_heard();
        for (station, rest) in (0..3).zip(&rests) {
            assert!(heard.contains(&rest.text), "station {station}: {heard:?}");
            let end = going.at_run_ms + whole_ms(rest.end_sec);
            let deadline = runtime.machine.stations[station as usize]
                .caller
                .deadline
                .expect("it waits");
            assert!(deadline >= end + 1_500, "station {station}");
        }
        let next = next_turn(&mut runtime);
        assert!(!senders(&next).is_empty());
    }

    /// Esc once your CQ is out stops nothing, as MorseRunner's `AbortSend`
    /// stops only your own station (`MyStn.pas:108-116`) and leaves a
    /// station's reply to come (`DxStn.pas:235-237, 273-274`): the turn
    /// plays on as it is, whether the replies have yet to begin or are
    /// sounding, each heard as it would have been with no Esc at all. Esc
    /// used to drop a reply yet to begin, which came only on its caller's
    /// timeout, and to cut those sounding, each sent again from its
    /// character sounding.
    #[test]
    fn esc_after_your_message_leaves_the_turn_playing() {
        let calls = ["K5XYZ", "W7ABC", "G4ABC"];
        for into in [-0.05, 0.2] {
            let mut runtime = pileup(&calls, 3, 3);
            let mut alone = pileup(&calls, 3, 3);
            let cq = runtime.last_turn().clone();
            let first = cq
                .segments
                .iter()
                .filter(|segment| segment.who.is_caller())
                .map(|segment| segment.start_sec)
                .fold(f64::INFINITY, f64::min);
            assert!(first + into > cq.your_end_sec().expect("your CQ"));
            runtime.run_to(ms(first + into));
            assert!(runtime.key(OpKey::StopTx, "", "").is_empty(), "{into}");
            assert_eq!(runtime.playing_id(), Some(cq.id), "{into}");
            runtime.finish_turn();
            alone.finish_turn();
            assert_eq!(runtime.take_heard(), alone.take_heard(), "{into}");
            for station in senders(&cq) {
                let send = sent_by(&cq, station)[0];
                let ends = &runtime.machine.stations[station as usize].heard.call_ends;
                assert_eq!(ends[0], cq.at_run_ms + whole_ms(send.end_sec), "{into}");
                let unstopped = &alone.machine.stations[station as usize].heard.call_ends;
                assert_eq!(ends, unstopped, "{into}");
            }
        }
    }

    /// Esc with your message out whole leaves a single caller's reply
    /// coming, still to begin or sounding, as MorseRunner's Esc stops only
    /// your own station. A single caller stops for a message keyed over
    /// it, to hear it; Esc gives it nothing to hear. Its exchange is heard
    /// as it was to end and logged clean, no repeat charged. Esc used to
    /// drop a reply yet to begin, which came some 2.6 s late on its
    /// timeout and cost the station a silent timeout, and to cut one
    /// sounding, sent again from its character sounding.
    #[test]
    fn esc_after_your_message_leaves_a_single_callers_reply_coming() {
        for into in [-0.05, 0.3] {
            let mut runtime = runtime(&["K5XYZ"]);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let turn = runtime.last_turn().clone();
            let reply = sent_by(&turn, 0)[0].clone();
            let your_end = turn.your_end_sec().expect("your call");
            assert!(reply.start_sec + into > your_end, "{into}");
            runtime.run_to(runtime.now + ms(reply.start_sec + into));
            assert!(runtime.key(OpKey::StopTx, "K5XYZ", "").is_empty());
            assert_eq!(runtime.playing_id(), Some(turn.id));
            runtime.finish_turn();
            assert!(runtime.take_heard().contains(&reply.text), "{into}");
            let heard = &runtime.machine.stations[0].heard;
            assert_eq!(
                heard.exch_ends,
                [turn.at_run_ms + whole_ms(reply.end_sec)],
                "{into}"
            );
            runtime.key(OpKey::Esm, "K5XYZ", &serial(&reply.text));
            runtime.finish_turn();
            runtime.apply(ContestEvent::FinishNow);
            assert_eq!(view(&runtime).clean, 1, "{into}");
            assert_eq!(sends(&runtime), [1, 1], "{into}: no repeat charged");
        }
    }

    /// Without full break-in a caller's rest that went out in part under
    /// your message goes on after an Esc, sounding, but is no send heard,
    /// as a call heard in part is none: it ends no copy and Show copy never
    /// shows it. With full break-in it was heard throughout, and its call
    /// is heard as it ends.
    #[test]
    fn a_rest_the_receiver_cut_into_stays_unheard_after_esc() {
        for qsk in [false, true] {
            let mut settings = pileup_settings(1, 1);
            settings.contest.contest_qsk = qsk;
            settings.contest.contest_show_copy = true;
            let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
            let cq = runtime.last_turn().clone();
            let call = sent_by(&cq, 0)[0].clone();
            runtime.run_to(ms(call.start_sec + 0.3));
            runtime.key(OpKey::Again, "", "");
            let asked = runtime.last_turn().clone();
            assert_eq!(asked.muted(sent_by(&asked, 0)[0], qsk), !qsk);
            runtime.run_to(runtime.now + 300);
            let effects = runtime.key(OpKey::StopTx, "", "");
            let going = played(&effects).expect("it goes on").clone();
            assert_eq!(yours(&going), "");
            let rest = sent_by(&going, 0)[0].clone();
            assert_eq!(rest.start_sec, 0.0);
            assert!(!going.muted(&rest, qsk), "{qsk}: it sounds");
            assert_eq!(view(&runtime).shown.len(), usize::from(qsk), "{qsk}");
            runtime.finish_turn();
            let ends = &runtime.machine.stations[0].heard.call_ends;
            let heard: &[u64] = if qsk {
                &[going.at_run_ms + whole_ms(rest.end_sec)]
            } else {
                &[]
            };
            assert_eq!(ends, heard, "{qsk}");
        }
    }

    /// Esc a moment after a key, before the player has said the turn
    /// started, finds nothing of it sounded, though the turn is timed from
    /// the key till then: a caller's rest carried under your message never
    /// went out under it, so without full break-in it goes on heard, shown
    /// with Show copy and its call heard as it ends. It used to go on
    /// unheard, as if the receiver had been muted for it.
    #[test]
    fn esc_before_the_turn_sounds_leaves_a_rest_heard() {
        let mut settings = pileup_settings(1, 1);
        settings.contest.contest_show_copy = true;
        let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
        let cq = runtime.last_turn().clone();
        let call = sent_by(&cq, 0)[0].clone();
        runtime.run_to(ms(call.start_sec + 0.3));
        runtime.key(OpKey::Again, "", "");
        let asked = runtime.last_turn().clone();
        assert!(asked.muted(sent_by(&asked, 0)[0], false));
        // Its start not yet said.
        runtime.now += 5;
        let effects = runtime.key(OpKey::StopTx, "", "");
        let going = played(&effects).expect("it goes on").clone();
        assert_eq!(yours(&going), "");
        let rest = sent_by(&going, 0)[0].clone();
        assert_eq!(view(&runtime).shown.len(), 1);
        runtime.finish_turn();
        let ends = &runtime.machine.stations[0].heard.call_ends;
        assert_eq!(ends, &[going.at_run_ms + whole_ms(rest.end_sec)]);
    }

    /// Once the station you work has gone, whatever call the fields still
    /// hold for it, the screen says it has gone and Enter calls CQ, where
    /// it would send that call to nobody while the rest of the pile-up
    /// waits in silence: its call and number as copied; a slip it put
    /// right, or the call put right in the field since with no key
    /// pressed; a partial it answered, `?` and all, or its call typed
    /// whole since. The CQ clears the fields for the next station, and the
    /// pile-up answers it.
    #[test]
    fn after_the_station_you_work_leaves_with_its_call_typed_the_hint_says_cq() {
        let follow = |runtime: &mut Runtime<ScriptCalls>, call: &str, exch: &str, hint: &str| {
            let typed = Fields {
                call: call.into(),
                exch: exch.into(),
                ..Fields::default()
            };
            let enter = esm(&view(runtime).qso, &typed, &RULES[0], Side::Dx);
            assert_eq!(
                (enter.label.as_str(), enter.hint.as_str()),
                ("CQ", hint),
                "{call:?}"
            );
            let after = runtime.turns.len();
            let effects = runtime.key(OpKey::Esm, call, exch);
            assert!(effects.contains(&ContestEffect::ClearFields), "{call:?}");
            assert_eq!(yours(&runtime.turns[after]), "CQ DL1ABC TEST", "{call:?}");
            let answered = runtime.turns[after..]
                .iter()
                .any(|turn| turn.segments.iter().any(|s| s.who != Sender::You));
            assert!(answered, "{call:?}: the pile-up answers the CQ");
        };
        let leaves = |runtime: &mut Runtime<ScriptCalls>, said: &str| {
            let from = runtime.effects.len();
            runtime.run_to(runtime.now + 90_000);
            assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
            let gone = ContestEffect::Announce(format!("{said} has gone"));
            assert!(runtime.effects[from..].contains(&gone), "{said}");
        };
        // Its call and number, as copied.
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        let nr = its_number(&mut runtime, "K5XYZ", 0);
        leaves(&mut runtime, "K5XYZ");
        follow(&mut runtime, "K5XYZ", &nr, "K5XYZ has gone: Enter calls CQ");
        // A slip it put right, named by the call it put right; the field
        // put right since, or not.
        for (call, hint) in [
            ("K5XYZ", "K5XYZ has gone: Enter calls CQ"),
            ("K5XYW", "K5XYW has gone: Enter calls CQ"),
        ] {
            let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYW", "");
            let fix = said_by(runtime.last_turn(), 0).unwrap_or_default();
            assert!(fix.contains("K5XYZ"), "{fix:?}");
            runtime.finish_turn();
            leaves(&mut runtime, "K5XYZ");
            follow(&mut runtime, call, "", hint);
        }
        // A partial it answered, and its call typed whole since.
        for (partial, call, hint) in [
            ("K5?", "K5?", "The station has gone: Enter calls CQ"),
            ("XYZ?", "XYZ?", "The station has gone: Enter calls CQ"),
            ("K5X?", "K5X?", "The station has gone: Enter calls CQ"),
            ("K5?", "K5XYZ", "K5XYZ has gone: Enter calls CQ"),
        ] {
            let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, partial, "");
            assert_eq!(replies(runtime.last_turn()), ["K5XYZ"], "{partial}");
            runtime.finish_turn();
            leaves(&mut runtime, "The station");
            follow(&mut runtime, call, "", hint);
        }
    }

    /// The same once B4 has sent a dupe away, its call still typed. The
    /// rest of the pile-up calls again, so a CQ would only go over them:
    /// Enter clears the fields rather than send the dupe's call, which
    /// would leave them all waiting on a QSO with a station that has gone.
    /// B4 pressed again sends that call again, still the dupe's, and F5
    /// the call of a station that has given up: neither brings back
    /// Enter's call to nobody.
    #[test]
    fn after_b4_sends_a_dupe_away_with_its_call_typed_enter_clears_it() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        work_with_enter(&mut runtime, "K5XYZ", 0);
        let mut before = Vec::new();
        let run_ms = view(&runtime).run_ms(MonoMs(runtime.now));
        assert_eq!(runtime.machine.rejoin(0, &mut before, &[], run_ms), Some(0));
        runtime.key(OpKey::Cq, "", "");
        runtime.finish_turn();
        let typed = Fields {
            call: "K5XYZ".into(),
            ..Fields::default()
        };
        for b4 in 0..2 {
            runtime.key(OpKey::B4, "K5XYZ", "");
            assert_eq!(yours(runtime.last_turn()), "K5XYZ QSO B4", "B4 {b4}");
            assert!(
                said_by(runtime.last_turn(), 1).is_some(),
                "B4 {b4}: W7ABC calls again"
            );
            runtime.finish_turn();
            assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
            let enter = esm(&view(&runtime).qso, &typed, &RULES[0], Side::Dx);
            assert_eq!(
                (enter.label.as_str(), enter.hint.as_str()),
                ("Wipe", "K5XYZ has gone: Enter clears the fields"),
                "B4 {b4}"
            );
        }
        let turns = runtime.turns.len();
        let effects = runtime.key(OpKey::Esm, "K5XYZ", "");
        assert!(effects.contains(&ContestEffect::ClearFields));
        assert_eq!(runtime.turns.len(), turns, "nothing goes out");
        // Typed afresh, the next station's call goes out as ever.
        runtime.key(OpKey::Esm, "W7ABC", "");
        assert_eq!(yours(runtime.last_turn()), "W7ABC 5NN TT2");
        assert!(said_by(runtime.last_turn(), 1).is_some(), "W7ABC answers");
        // F5 with the call of a station that has given up: Enter still
        // calls CQ, and never sends its TU to log it.
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        let nr = its_number(&mut runtime, "K5XYZ", 0);
        runtime.run_to(runtime.now + 60_000);
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
        runtime.key(OpKey::HisCall, "K5XYZ", &nr);
        runtime.finish_turn();
        let typed = Fields {
            call: "K5XYZ".into(),
            exch: nr,
            ..Fields::default()
        };
        let enter = esm(&view(&runtime).qso, &typed, &RULES[0], Side::Dx);
        assert_eq!(
            (enter.label.as_str(), enter.hint.as_str()),
            ("CQ", "K5XYZ has gone: Enter calls CQ")
        );
    }

    /// Once a CQ has brought a caller after the station you work has
    /// gone, its call still typed, Enter sends no CQ over the caller, which
    /// would charge a single caller's call a repeat you never asked for: it
    /// clears the fields, and the hint says why.
    #[test]
    fn after_a_cq_brings_a_caller_the_call_of_a_station_gone_is_cleared() {
        let mut runtime = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut runtime);
        let nr = serial(&exchange);
        runtime.run_to(runtime.now + 60_000);
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
        // F1 with the fields as they were: a new caller answers.
        runtime.key(OpKey::Cq, "K5XYZ", &nr);
        let next = replies(runtime.last_turn())[0].to_string();
        runtime.finish_turn();
        let typed = Fields {
            call: "K5XYZ".into(),
            exch: nr.clone(),
            ..Fields::default()
        };
        let enter = esm(&view(&runtime).qso, &typed, &RULES[0], Side::Dx);
        assert_eq!(
            (enter.label.as_str(), enter.hint.as_str()),
            ("Wipe", "K5XYZ has gone: Enter clears the fields")
        );
        let turns = runtime.turns.len();
        let effects = runtime.key(OpKey::Esm, "K5XYZ", &nr);
        assert!(effects.contains(&ContestEffect::ClearFields));
        assert_eq!(runtime.turns.len(), turns, "nothing goes out");
        runtime.key(OpKey::Esm, &next, "");
        let theirs = serial(replies(runtime.last_turn())[0]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, &next, &theirs);
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        let charged = result
            .groups
            .iter()
            .zip(&result.group_timings)
            .find(|(group, _)| group.sent == next)
            .map(|(_, timing)| timing.sends);
        assert_eq!(charged, Some(1), "{next}'s call charged a repeat");
    }

    /// Deliver what is due, nothing keyed, until another caller gives up:
    /// which.
    fn next_to_give_up(runtime: &mut Runtime<ScriptCalls>) -> usize {
        let gone = |runtime: &Runtime<ScriptCalls>| -> Vec<bool> {
            let stations = &runtime.machine.stations;
            stations
                .iter()
                .map(|station| station.caller.state == CallerState::Gone)
                .collect()
        };
        let before = gone(runtime);
        loop {
            assert!(runtime.step(), "nothing more is due");
            let now = gone(runtime);
            let given_up =
                (0..now.len()).find(|index| now[*index] && before.get(*index) != Some(&true));
            if let Some(index) = given_up {
                return index;
            }
        }
    }

    /// How many callers are still calling.
    fn calling(runtime: &Runtime<ScriptCalls>) -> usize {
        let stations = &runtime.machine.stations;
        stations
            .iter()
            .filter(|station| station.caller.state == CallerState::NeedQso)
            .count()
    }

    /// The third browser test: a pile-up of five in CQ WW, nothing keyed
    /// while the callers call and give up one by one as their patience runs
    /// out. The call of one that has just stopped, typed late, went to
    /// nobody, the callers still calling fell silent to wait on that QSO,
    /// and Enter offered the call again and again. Now Enter sends nothing
    /// over the callers still calling: it clears the fields, the hint says
    /// why, the screen reader hears it, and the rest call on. Once all have
    /// given up, Enter calls CQ in its place, which the pile-up answers.
    #[test]
    fn answering_a_caller_that_has_given_up_sends_nothing_into_the_silence() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_kind = crate::contest::ContestKind::Cqww;
        settings.contest.contest_style = RunStyle::Pileup;
        settings.band.stations_min = 5;
        settings.band.stations_max = 5;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        let calls = ["K4MH", "R8AI", "N2GH", "W7ST", "K3YC"];
        let mut runtime = Runtime::start(1, settings.clamp(), ScriptCalls::new(&calls));
        assert_eq!(senders(runtime.last_turn()).len(), 5, "five call");
        let rules = crate::contest::ContestKind::Cqww.rules();
        let enter = |runtime: &Runtime<ScriptCalls>, call: &str| {
            let typed = Fields {
                call: call.into(),
                ..Fields::default()
            };
            esm(&view(runtime).qso, &typed, rules, Side::Dx)
        };
        let call = calls[next_to_give_up(&mut runtime)];
        assert!(calling(&runtime) > 0, "the rest still call");
        assert!(view(&runtime).qso.heard_gone.contains(&call.to_string()));
        let gone = enter(&runtime, call);
        assert_eq!(
            (gone.label, gone.hint),
            (
                "Wipe".into(),
                format!("{call} has gone: Enter clears the fields")
            )
        );
        let turns = runtime.turns.len();
        let effects = runtime.key(OpKey::Esm, call, "");
        assert_eq!(runtime.turns.len(), turns, "nothing goes out");
        assert!(effects.contains(&ContestEffect::ClearFields));
        assert!(effects.contains(&ContestEffect::Announce(format!("{call} has gone"))));
        // The rest call on, none of them waiting on a QSO with nobody.
        let stations = &runtime.machine.stations;
        assert!(!stations.iter().any(|station| station.caller.waits()));
        assert!(!replies(&next_turn(&mut runtime)).is_empty());
        // Its call stays gone once Enter has cleared it, and so does every
        // call that gives up after it.
        while calling(&runtime) > 0 {
            next_to_give_up(&mut runtime);
        }
        let mut gone_calls = calls;
        gone_calls.sort_unstable();
        assert_eq!(view(&runtime).qso.heard_gone, gone_calls);
        // Nobody calls now: Enter calls CQ in place of any of them.
        let gone = enter(&runtime, call);
        assert_eq!(
            (gone.label, gone.hint),
            ("CQ".into(), format!("{call} has gone: Enter calls CQ"))
        );
        let effects = runtime.key(OpKey::Esm, call, "");
        assert!(effects.contains(&ContestEffect::ClearFields));
        assert!(effects.contains(&ContestEffect::Announce(format!("{call} has gone"))));
        assert_eq!(yours(runtime.last_turn()), "CQ DL1ABC TEST");
        assert!(
            !replies(runtime.last_turn()).is_empty(),
            "the pile-up answers"
        );
    }

    /// A pile-up of `calls`, nothing keyed, on the first seed whose first
    /// caller gives up first, the second still calling.
    fn first_gives_up_first(calls: &[&str], fewest: u32, most: u32) -> Runtime<ScriptCalls> {
        (1..100)
            .find_map(|seed| {
                let mut runtime = pileup_seed(seed, calls, fewest, most);
                let first = next_to_give_up(&mut runtime) == 0;
                let second = runtime.machine.stations[1].caller.state == CallerState::NeedQso;
                (first && second).then_some(runtime)
            })
            .expect("a seed where it does")
    }

    /// Around a caller that gave up before you worked it: a call that fits
    /// a caller still calling goes to it, its slip and all; a call nobody
    /// was heard to send, a partial, and a slip of the call gone go out as
    /// typed; and once a caller calls with the call gone again, it takes
    /// it, and Enter sends it its call and your exchange as ever.
    #[test]
    fn around_a_caller_that_gave_up_enter_sends_as_ever() {
        let calls = ["K5XYZ", "K5XYA", "K5XYZ"];
        let enter = |runtime: &Runtime<ScriptCalls>, call: &str| {
            let typed = Fields {
                call: call.into(),
                ..Fields::default()
            };
            esm(&view(runtime).qso, &typed, &RULES[0], Side::Dx).label
        };
        let mut runtime = first_gives_up_first(&calls, 2, 2);
        // Its call is K5XYA's with a slip: K5XYA, still calling, takes it.
        assert!(view(&runtime).qso.heard_gone.is_empty());
        for (typed, label) in [
            ("K5XYZ", "Send K5XYZ + exch"),
            ("ZL4ZZZ", "Send ZL4ZZZ + exch"),
            ("?XYZ", "Ask ?XYZ"),
            ("K5XYZ?", "Ask K5XYZ?"),
        ] {
            assert_eq!(enter(&runtime, typed), label);
        }
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let fix = said_by(runtime.last_turn(), 1).unwrap_or_default();
        assert!(fix.contains("K5XYA"), "K5XYA puts it right: {fix:?}");
        // Unlike K5XYA's, a call that fits no caller still calling.
        let calls = ["K5XYZ", "G4ABC", "K5XYZ"];
        let mut runtime = first_gives_up_first(&calls, 2, 2);
        assert_eq!(view(&runtime).qso.heard_gone, ["K5XYZ"]);
        assert_eq!(enter(&runtime, "K5XYZ"), "Wipe");
        for (typed, label) in [("K5XYW", "Send K5XYW + exch"), ("K5X?", "Ask K5X?")] {
            assert_eq!(enter(&runtime, typed), label);
        }
        // A CQ brings a caller with the call gone: it fits again.
        runtime.key(OpKey::Cq, "", "");
        let back = said_by(runtime.last_turn(), 2).unwrap_or_default();
        assert!(back.starts_with("K5XYZ"), "{back:?}");
        runtime.finish_turn();
        let heard_gone = view(&runtime).qso.heard_gone;
        assert!(!heard_gone.contains(&"K5XYZ".to_string()), "{heard_gone:?}");
        assert_eq!(enter(&runtime, "K5XYZ"), "Send K5XYZ + exch");
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let reply = said_by(runtime.last_turn(), 2).unwrap_or_default();
        assert!(
            matches!(copy_of(reply), Some(Copied::Exchange(_))),
            "{reply:?}"
        );
    }

    /// A dupe calling again is answered as ever, B4 and all; once it gives
    /// up, its call is gone as any caller's is.
    #[test]
    fn a_dupe_that_gives_up_is_gone_too() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        work_with_enter(&mut runtime, "K5XYZ", 0);
        let mut before = Vec::new();
        let run_ms = view(&runtime).run_ms(MonoMs(runtime.now));
        assert_eq!(runtime.machine.rejoin(0, &mut before, &[], run_ms), Some(0));
        runtime.key(OpKey::Cq, "", "");
        assert!(said_by(runtime.last_turn(), 0).is_some(), "the dupe calls");
        runtime.finish_turn();
        let typed = Fields {
            call: "K5XYZ".into(),
            ..Fields::default()
        };
        let label = |runtime: &Runtime<ScriptCalls>| {
            esm(&view(runtime).qso, &typed, &RULES[0], Side::Dx).label
        };
        assert_eq!(label(&runtime), "Send K5XYZ + exch");
        assert!(view(&runtime).worked_before("K5XYZ").is_some());
        while runtime.machine.stations[0].caller.state != CallerState::Gone {
            next_to_give_up(&mut runtime);
        }
        assert!(view(&runtime).qso.heard_gone.contains(&"K5XYZ".to_string()));
        let gone = if calling(&runtime) > 0 { "Wipe" } else { "CQ" };
        assert_eq!(label(&runtime), gone);
    }

    /// A single caller that gives up, never answered: with nobody calling
    /// Enter calls CQ in place of its call, which the next caller answers;
    /// with that caller calling, Enter clears its call, sending nothing.
    #[test]
    fn a_single_caller_that_gave_up_is_answered_with_a_cq() {
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        next_to_give_up(&mut runtime);
        assert_eq!(view(&runtime).qso.heard_gone, ["K5XYZ"]);
        assert!(view(&runtime).qso.nobody_calling);
        let effects = runtime.key(OpKey::Esm, "K5XYZ", "");
        assert!(effects.contains(&ContestEffect::ClearFields));
        assert!(effects.contains(&ContestEffect::Announce("K5XYZ has gone".into())));
        assert_eq!(yours(runtime.last_turn()), "CQ DL1ABC TEST");
        assert_eq!(replies(runtime.last_turn()), ["G4ABC"]);
        runtime.finish_turn();
        let turns = runtime.turns.len();
        let effects = runtime.key(OpKey::Esm, "K5XYZ", "");
        assert!(effects.contains(&ContestEffect::ClearFields));
        assert_eq!(runtime.turns.len(), turns, "nothing goes out");
        // G4ABC is worked as ever.
        runtime.key(OpKey::Esm, "G4ABC", "");
        let reply = replies(runtime.last_turn())[0];
        assert!(
            matches!(copy_of(reply), Some(Copied::Exchange(_))),
            "{reply:?}"
        );
    }

    /// What `effects` say to the screen reader.
    fn announced(effects: &[ContestEffect]) -> Vec<&str> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                ContestEffect::Announce(said) => Some(said.as_str()),
                _ => None,
            })
            .collect()
    }

    /// A partial, or a slip, of the call of a caller that gave up goes out
    /// as typed while the rest of the pile-up calls, as any call that fits
    /// nobody there does, and they wait on that QSO. Once nobody calls,
    /// Enter on it calls CQ, the fields cleared, and the hint and the
    /// screen reader say the station has gone, naming none; the pile-up
    /// answers. A single caller that gave up is the same: its partial goes
    /// out once, into the silence, and then Enter calls CQ.
    #[test]
    fn a_partial_or_a_slip_of_a_caller_that_gave_up_leaves_no_dead_end() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_kind = crate::contest::ContestKind::Cqww;
        settings.contest.contest_style = RunStyle::Pileup;
        settings.band.stations_min = 5;
        settings.band.stations_max = 5;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        let calls = ["K4MH", "R8AI", "N2GH", "W7ST", "K3YC"];
        let rules = crate::contest::ContestKind::Cqww.rules();
        let enter = |runtime: &Runtime<ScriptCalls>, call: &str, rules: &ContestRules| {
            let typed = Fields {
                call: call.into(),
                ..Fields::default()
            };
            esm(&view(runtime).qso, &typed, rules, Side::Dx)
        };
        for slip in [false, true] {
            let start = Runtime::start(1, settings.clone().clamp(), ScriptCalls::new(&calls));
            let mut runtime = start;
            let gone = calls[next_to_give_up(&mut runtime)];
            assert!(calling(&runtime) > 0, "the rest still call");
            let typed = if slip {
                format!("{}X", &gone[..gone.len() - 1])
            } else {
                format!("{}?", &gone[..3])
            };
            let label = if slip {
                format!("Send {typed} + exch")
            } else {
                format!("Ask {typed}")
            };
            assert_eq!(enter(&runtime, &typed, rules).label, label);
            let turns = runtime.turns.len();
            let effects = runtime.key(OpKey::Esm, &typed, "");
            assert!(yours(runtime.last_turn()).starts_with(&typed), "{typed}");
            assert!(runtime.turns.len() > turns, "{typed} goes out");
            assert!(announced(&effects).is_empty(), "{typed}: nothing said yet");
            // Nobody takes it: the rest wait on that QSO, silent.
            runtime.run_to(runtime.now + 25_000);
            assert!(view(&runtime).qso.nobody_calling, "{typed}");
            let cq = enter(&runtime, &typed, rules);
            assert_eq!(
                (cq.message, cq.label, cq.hint, cq.clears),
                (
                    vec![MessagePart::Cq],
                    "CQ".to_string(),
                    "The station has gone: Enter calls CQ".to_string(),
                    true
                ),
                "{typed}"
            );
            let effects = runtime.key(OpKey::Esm, &typed, "");
            assert!(effects.contains(&ContestEffect::ClearFields), "{typed}");
            assert_eq!(announced(&effects), ["The station has gone"], "{typed}");
            assert_eq!(yours(runtime.last_turn()), "CQ DL1ABC TEST");
            assert!(
                !replies(runtime.last_turn()).is_empty(),
                "the pile-up answers"
            );
        }
        let mut runtime = runtime(&["K5XYZ", "G4ABC"]);
        next_to_give_up(&mut runtime);
        assert_eq!(enter(&runtime, "K5X?", &RULES[0]).label, "Ask K5X?");
        runtime.key(OpKey::Esm, "K5X?", "");
        assert_eq!(yours(runtime.last_turn()), "K5X?");
        assert!(replies(runtime.last_turn()).is_empty(), "nobody answers");
        runtime.finish_turn();
        assert_eq!(enter(&runtime, "K5X?", &RULES[0]).label, "CQ");
        runtime.key(OpKey::Esm, "K5X?", "");
        assert_eq!(replies(runtime.last_turn()), ["G4ABC"]);
    }

    /// A caller that answered your partial, no QSO yet, and spends its last
    /// try on your next `?` leaves as on a timeout, and is said to, by the
    /// partial you had for it. Its call typed after is one Enter no longer
    /// sends, and Enter says no more of it. Of a station worked that
    /// leaves on its timeouts, said as it leaves, the same.
    #[test]
    fn a_caller_that_gives_up_on_your_partial_is_said_to_have_gone() {
        let mut runtime = pileup(&["K5XYZ", "G4ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5X?", "");
        assert!(said_by(runtime.last_turn(), 0).is_some(), "K5XYZ answers");
        runtime.finish_turn();
        let mut left = Vec::new();
        for _ in 0..20 {
            let effects = runtime.key(OpKey::Again, "K5X?", "");
            runtime.finish_turn();
            if runtime.machine.stations[0].caller.state == CallerState::Gone {
                left = effects;
                break;
            }
            assert!(announced(&effects).is_empty(), "it answered");
        }
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Gone);
        assert_eq!(announced(&left), ["The station has gone"]);
        assert!(view(&runtime).qso.gone.contains(&"K5XYZ".to_string()));
        let typed = Fields {
            call: "K5XYZ".into(),
            ..Fields::default()
        };
        let enter = esm(&view(&runtime).qso, &typed, &RULES[0], Side::Dx);
        assert_eq!(enter.hint, "K5XYZ has gone: Enter calls CQ");
        let effects = runtime.key(OpKey::Esm, "K5XYZ", "");
        assert!(announced(&effects).is_empty(), "said as it left");
        assert_eq!(yours(runtime.last_turn()), "CQ DL1ABC TEST");
        assert!(said_by(runtime.last_turn(), 1).is_some(), "G4ABC calls");

        // Worked, and gone on its timeouts.
        let mut runtime = pileup(&["K5XYZ", "G4ABC"], 2, 2);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        runtime.finish_turn();
        let from = runtime.effects.len();
        while runtime.machine.stations[0].caller.state != CallerState::Gone {
            assert!(runtime.step(), "nothing more is due");
        }
        assert_eq!(announced(&runtime.effects[from..]), ["K5XYZ has gone"]);
        let effects = runtime.key(OpKey::Esm, "K5XYZ", "");
        assert!(effects.contains(&ContestEffect::ClearFields));
        assert!(announced(&effects).is_empty(), "said as it left");
    }

    /// A station logged and gone from the frequency is no station that
    /// gave up: its call is in the log, the B4 tag shows for it, and Enter
    /// sends it as typed, the rest of the pile-up waiting on that QSO; B4
    /// tells them it is over, and they call again.
    #[test]
    fn a_logged_station_typed_again_goes_out_and_b4_brings_the_rest_back() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        work_with_enter(&mut runtime, "K5XYZ", 0);
        assert_eq!(runtime.machine.stations[0].caller.state, CallerState::Done);
        assert!(view(&runtime).worked_before("K5XYZ").is_some());
        assert!(view(&runtime).qso.heard_gone.is_empty());
        let typed = Fields {
            call: "K5XYZ".into(),
            ..Fields::default()
        };
        let enter = esm(&view(&runtime).qso, &typed, &RULES[0], Side::Dx);
        assert_eq!(enter.label, "Send K5XYZ + exch");
        runtime.key(OpKey::Esm, "K5XYZ", "");
        assert!(replies(runtime.last_turn()).is_empty(), "nobody answers");
        runtime.finish_turn();
        assert!(view(&runtime).qso.nobody_calling, "the rest wait");
        runtime.key(OpKey::B4, "K5XYZ", "");
        assert_eq!(yours(runtime.last_turn()), "K5XYZ QSO B4");
        assert!(
            said_by(runtime.last_turn(), 1).is_some(),
            "W7ABC calls again"
        );
    }

    /// After Esc the station you work sends what it owes at once, as it
    /// does after any garbage, and as no repeat you asked for: it costs
    /// only time. A caller waiting out your QSO with it keeps quiet.
    #[test]
    fn after_esc_the_station_you_work_sends_what_it_owes_at_no_charge() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC"], 2, 2);
        let nr = its_number(&mut runtime, "K5XYZ", 0);
        assert!(runtime.machine.stations[1].caller.waits(), "W7ABC waits");
        runtime.key(OpKey::Again, "K5XYZ", "");
        runtime.now += 30;
        runtime.key(OpKey::StopTx, "K5XYZ", "");
        let owed = runtime.last_turn().clone();
        assert_eq!((yours(&owed), senders(&owed)), ("", BTreeSet::from([0])));
        assert_eq!(
            said_by(&owed, 0).and_then(copy_of),
            Some(Copied::Exchange(nr.clone()))
        );
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        runtime.apply(ContestEvent::FinishNow);
        assert_eq!(sends(&runtime), [1, 1]);
    }

    /// A pile-up caller that missed your exchange and asked for it stops
    /// asking after its silent timeouts, but stays, as MorseRunner's
    /// station waiting for your number does: your exchange, twenty seconds
    /// later, is still taken and answered.
    #[test]
    fn a_caller_that_missed_your_exchange_still_takes_it_late() {
        let mut tried = 0;
        for seed in 0..200 {
            let mut runtime = pileup_seed(seed, &["K5XYZ"], 1, 1);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let Some(asks) = said_by(runtime.last_turn(), 0).map(str::to_string) else {
                continue;
            };
            if asks != "NR?" && asks != "AGN" {
                continue;
            }
            tried += 1;
            runtime.finish_turn();
            // Twenty seconds before you send it again.
            runtime.run_to(runtime.now + 20_000);
            runtime.key(OpKey::Esm, "K5XYZ", "");
            assert_eq!(yours(runtime.last_turn()), "K5XYZ 5NN TT1");
            assert!(
                said_by(runtime.last_turn(), 0).is_some(),
                "seed {seed}: it asked {asks}, then left ({:?}) before your exchange came again",
                runtime.machine.stations[0].caller.state
            );
            break;
        }
        assert!(tried > 0);
    }

    /// Esc takes your message back from the callers, not what they sent
    /// from you: a call heard whole over a partial you then stopped is
    /// still the end its copy is timed from.
    #[test]
    fn esc_keeps_a_call_heard_whole_over_your_message() {
        let mut runtime = pileup_qsk(&["K5XYZ"], 1, 1);
        let cq = runtime.last_turn().clone();
        let call = sent_by(&cq, 0)[0].clone();
        // A partial keyed over the end of its first call, which goes on
        // over it.
        runtime.run_to(ms(call.start_sec + 1.6));
        runtime.key(OpKey::Esm, "K5X?", "");
        let turn = runtime.last_turn().clone();
        let rest = sent_by(&turn, 0)[0].clone();
        assert_eq!((rest.start_sec, yours(&turn)), (0.0, "K5X?"));
        // Its call ends, heard whole, while your partial is still going
        // out; then Esc.
        let ended = runtime.now + ms(rest.end_sec);
        runtime.run_to(ended + 100);
        let air = view(&runtime).air.expect("on the air");
        assert!(!air.still_sending(MonoMs(runtime.now)).is_empty());
        runtime.key(OpKey::StopTx, "K5X?", "");
        // It answers what it heard of your partial, unasked; half a second
        // after, the call is typed whole, and sent.
        runtime.finish_turn();
        runtime.now += 500;
        runtime.key(OpKey::Esm, "K5XYZ", "");
        assert_eq!(yours(runtime.last_turn()), "K5XYZ 5NN TT1");
        let (copy, took) = runtime.machine.stations[0]
            .heard
            .first_call
            .clone()
            .expect("it took your exchange");
        assert_eq!(copy.text, "K5XYZ");
        let typed = runtime.now - ended;
        assert!(
            took + 50 >= typed,
            "copy timed at {took} ms, typed {typed} ms after the call ended"
        );
    }

    /// Without full break-in a pile-up caller still sending as you key goes
    /// on under your message with the receiver muted: nothing of it reaches
    /// you, so its call is no copy's end, RX counts it only once your
    /// message is out, an operator hears none of it and Show copy never
    /// shows it, while its call after your message, a moment after you, is
    /// heard, and its copy timed from that. With full break-in it is heard
    /// through your message.
    #[test]
    fn a_caller_sending_across_your_message_is_not_heard_without_full_break_in() {
        for qsk in [false, true] {
            let mut settings = pileup_settings(1, 1);
            settings.contest.contest_qsk = qsk;
            settings.contest.contest_show_copy = true;
            let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
            let cq = runtime.last_turn().clone();
            let call = sent_by(&cq, 0)[0].clone();
            runtime.run_to(ms(call.start_sec + 1.6));
            runtime.take_heard();
            runtime.key(OpKey::Esm, "K5X?", "");
            let turn = runtime.last_turn().clone();
            let your_end = turn.your_end_sec().expect("your partial");
            let sends = sent_by(&turn, 0);
            let (rest, again) = (sends[0].clone(), sends[1].clone());
            assert!(rest.start_sec == 0.0 && rest.end_sec < your_end, "{qsk}");
            assert!(again.start_sec > your_end, "it calls again after you");
            assert_eq!(turn.muted(&rest, qsk), !qsk);
            assert!(!turn.muted(&again, qsk));
            let air = view(&runtime).air.expect("on the air");
            let mid = air.heard_from.plus_sec(rest.end_sec / 2.0);
            assert_eq!(air.calling(mid), usize::from(qsk), "RX with QSK {qsk}");
            let shown = |runtime: &Runtime<ScriptCalls>| -> Vec<String> {
                view(runtime)
                    .shown
                    .into_iter()
                    .map(|text| text.text)
                    .collect()
            };
            // Its call keyed over, shown whole, and its call after you; or
            // only the one heard.
            let expected = if qsk { 2 } else { 1 };
            assert_eq!(shown(&runtime).len(), expected, "{:?}", shown(&runtime));
            runtime.finish_turn();
            let heard = runtime.take_heard();
            assert_eq!(heard.contains(&rest.text), qsk, "{heard:?}");
            assert!(heard.contains(&again.text));
            // Its copy is timed from the first call heard whole: through
            // your message with full break-in, after it without.
            let first = if qsk { &rest } else { &again };
            let ends = &runtime.machine.stations[0].heard.call_ends;
            assert_eq!(ends, &[turn.at_run_ms + whole_ms(first.end_sec)], "{qsk}");
        }
    }

    /// Without full break-in the station you work, keyed over while its
    /// call put right is still going out, goes on with it under your
    /// message, and nothing of that rest reaches you: the correction tells
    /// Enter nothing and the hint says none, the exchange after it is not
    /// begun (no repeat, no speed, no copy), no copy's end is stamped, and
    /// the heard timer waits for nothing of it. With full break-in all of
    /// it is heard as it ends.
    #[test]
    fn a_correction_sent_on_under_your_message_tells_you_nothing_without_full_break_in() {
        let fixing = |qsk: bool| {
            let mut settings = pileup_settings(1, 1);
            settings.contest.contest_qsk = qsk;
            (1..100)
                .map(|seed| Runtime::start(seed, settings.clone(), ScriptCalls::new(&["K5XYZ"])))
                .find(|runtime| runtime.machine.stations[0].caller.fixes_with_exchange())
                .expect("an operator who puts the call right with its exchange")
        };
        for qsk in [false, true] {
            let mut runtime = fixing(qsk);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYW", "");
            let slip = runtime.last_turn().clone();
            let fix = sent_by(&slip, 0)[0].clone();
            assert!(fix.text.starts_with("K5XYZ"), "{}", fix.text);
            // Its call put right has begun to reach you, its exchange not.
            runtime.run_to(slip.at_run_ms + ms(fix.start_sec + 0.15));
            assert!(!view(&runtime).qso.corrected);
            let effects = runtime.key(OpKey::Again, "K5XYW", "");
            let turn = played(&effects).expect("your ?").clone();
            let rest = sent_by(&turn, 0)[0].clone();
            assert_eq!(rest.start_sec, 0.0, "it goes on over you");
            assert_eq!(turn.muted(&rest, qsk), !qsk);
            // The heard timer is set for the end of its rest only if it is
            // heard.
            let rest_ends = u32::try_from(whole_ms(rest.end_sec)).expect("a short wait");
            assert_eq!(timers(&effects).contains(&rest_ends), qsk, "{qsk}");
            let before = runtime.machine.stations[0].heard.clone();
            runtime.finish_turn();
            let heard = &runtime.machine.stations[0].heard;
            let announced = runtime.effects.iter().any(|effect| {
                matches!(effect, ContestEffect::Announce(said) if said.starts_with("They corrected"))
            });
            assert_eq!(view(&runtime).qso.corrected, qsk, "{qsk}: the correction");
            assert_eq!(announced, qsk, "{qsk}: the hint");
            assert_eq!(heard.exch_begun, qsk, "{qsk}: its exchange begun");
            assert_eq!(before.exch_voice, None);
            assert_eq!(heard.exch_voice.is_some(), qsk, "{qsk}: its speed");
            assert_eq!(
                heard.exch_ends.is_empty(),
                !qsk,
                "{qsk}: its exchange's end"
            );
            assert_eq!(heard.exch_asks, before.exch_asks, "no repeat drawn");
        }
    }

    /// Esc keeps what reached you since the key only of the stations on
    /// the frequency then: a dupe the message brought back goes back as it
    /// was, whole, its old QSO's ends and voices with it, so a NIL logged
    /// after with a slip of its call is charged to nobody, never to the
    /// station you work.
    #[test]
    fn esc_puts_back_a_dupe_the_message_brought_as_it_was() {
        let mut runtime = pileup(&[], 1, 1);
        for n in 0..5 {
            work_with_enter(&mut runtime, &format!("W{}AW", n + 1), n);
        }
        // The next caller, its call sent: the station you work.
        runtime.key(OpKey::HisCall, "W6AW", "");
        runtime.finish_turn();
        assert_eq!(runtime.machine.qso_station(), Some(5));
        let worked: Vec<Heard> = (0..5)
            .map(|index| runtime.machine.stations[index].heard.clone())
            .collect();
        // CQs taken back at once, until one brings a station worked before.
        let dupe = (0..2_000)
            .find_map(|_| {
                runtime.key(OpKey::Cq, "W6AW", "");
                let dupe = (0..5).find(|index| {
                    runtime.machine.stations[*index].caller.state == CallerState::NeedQso
                });
                runtime.now += 30;
                runtime.key(OpKey::StopTx, "W6AW", "");
                runtime.finish_turn();
                dupe
            })
            .expect("a dupe calls");
        let station = &runtime.machine.stations[dupe];
        assert_eq!(station.caller.state, CallerState::Done);
        assert_eq!(station.heard, worked[dupe], "station {dupe}");
        assert_eq!(runtime.machine.qso_station(), Some(5));
        // A slip of its call, logged with a number: a NIL, whose copy is
        // nobody's, its own kept already.
        let slip = format!("W{}AX", dupe + 1);
        runtime.key(OpKey::LogOnly, &slip, "5");
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        let copies = copies(&result);
        assert!(
            copies.iter().all(|(_, copy)| *copy != slip),
            "the NIL is charged: {copies:?}"
        );
        assert_eq!(run.qsos[5].lc, slip, "the record holds the call");
    }

    /// The results list each QSO at the speed its live log row showed: the
    /// slower of its call's and its exchange's, so a caller that slowed
    /// down for you reads the same in both, and a station logged again,
    /// whose copy the first QSO holds, shows none in either.
    #[test]
    fn a_stored_runs_log_reads_the_wpm_its_live_log_showed() {
        let mut runtime = runtime(&["K5XYZ"]);
        let exchange = to_its_exchange(&mut runtime);
        let own = runtime.machine.stations[0].caller.voice;
        let mut slowed = false;
        for _ in 0..5 {
            runtime.key(OpKey::Again, "K5XYZ", "");
            slowed = runtime.last_turn().segments[1].voice.char_wpm < own.char_wpm;
            runtime.finish_turn();
            if slowed {
                break;
            }
        }
        assert!(slowed, "it slowed down");
        let nr = serial(&exchange);
        runtime.key(OpKey::LogOnly, "K5XYZ", &nr);
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        runtime.key(OpKey::LogOnly, "K5XYZ", &nr);
        let live: Vec<Option<f64>> = view(&runtime).log.iter().map(|row| row.wpm).collect();
        runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        let stored: Vec<Option<f64>> = run
            .log(&result.groups, &result.group_timings)
            .expect("the run lists its QSOs")
            .iter()
            .map(|qso| qso.wpm)
            .collect();
        assert_eq!(stored, live, "the results against the live log");
    }

    /// A number logged while only the call put right of a correction
    /// that carries the station's exchange has reached you is a miss, as a
    /// guess is no copy; logged once the exchange after it has begun, it is
    /// a copy, as for an exchange sent alone.
    #[test]
    fn a_number_logged_before_the_correction_reaches_it_is_a_miss() {
        let settings = run_settings(25.0);
        for begun in [false, true] {
            let mut runtime = runtime_fixing(&["K5XYZ"], true);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYW", "");
            let at = runtime.now;
            let fix = sent_by(runtime.last_turn(), 0)[0].clone();
            assert!(fix.text.starts_with("DE K5XYZ "), "{}", fix.text);
            let nr = serial(&fix.text);
            // 300 ms into it, the D of DE; or a moment into its exchange,
            // found as the count of what is heard finds it.
            let into = if begun {
                let from = exchange_at(&fix.text).expect("its exchange");
                let spans = char_spans(&fix.text, &settings, &fix.voice);
                let start = spans
                    .iter()
                    .find(|span| span.at >= from)
                    .expect("sent")
                    .start_sec;
                ms(start) + 50
            } else {
                300
            };
            runtime.run_to(at + ms(fix.start_sec) + into);
            runtime.key(OpKey::LogOnly, "K5XYZ", &nr);
            runtime.apply(ContestEvent::FinishNow);
            let (result, _) = result(&runtime);
            assert_eq!(result.groups[1].sent, nr);
            let copied = if begun { nr.as_str() } else { "" };
            assert_eq!(result.groups[1].received, copied, "begun: {begun}");
        }
    }

    /// A WPX run worked by hand from `seed`, its callers sending `calls`
    /// in turn, with the callers' `habits`.
    fn habits_runtime(seed: u64, calls: &[&str], habits: CallerHabits) -> Runtime<ScriptCalls> {
        let mut settings = run_settings(25.0);
        settings.contest.contest_habits = habits;
        Runtime::start(seed, settings, ScriptCalls::new(calls))
    }

    /// The QRM in `turn`.
    fn qrm_in(turn: &Turn) -> Vec<&Segment> {
        turn.segments
            .iter()
            .filter(|segment| segment.who == Sender::Qrm)
            .collect()
    }

    /// Under rough habits a station that is not calling you sends on your
    /// frequency in a turn, from its start when it came due while the
    /// frequency was quiet: heard through the receiver as a caller is, but
    /// no caller. It never answers nor is answered, the count of stations
    /// calling and Enter's hint leave it out, and the QSO goes on as
    /// without it. Under easier habits there is none.
    #[test]
    fn qrm_sends_in_a_turn_and_is_never_a_caller() {
        for habits in [CallerHabits::Easy, CallerHabits::Real] {
            assert!(habits_runtime(1, &[], habits).machine.qrm.is_none());
        }
        let mut runtime = habits_runtime(1, &["K5XYZ"], CallerHabits::Rough);
        runtime.finish_turn();
        let qrm = runtime
            .machine
            .qrm
            .as_mut()
            .expect("rough habits bring QRM");
        qrm.start_at(0);
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let turn = runtime.last_turn().clone();
        let sent = qrm_in(&turn);
        assert!(!sent.is_empty(), "{turn:?}");
        assert_eq!(sent[0].start_sec, 0.0, "it came due while all was quiet");
        let text = sent[0].text.clone();
        assert!(
            text.starts_with("QRL?") || text.starts_with("CQ CQ TEST ") || text == "DL1ABC QSY QSY",
            "{text}"
        );
        assert!((30.0..=49.0).contains(&sent[0].voice.char_wpm));
        // Through the receiver, as the replies go.
        let planned = plan_turn(&turn, &runtime.machine.settings);
        assert_eq!(
            planned.others.len(),
            1 + sent.len(),
            "the reply and the QRM"
        );
        // The station you work answers as ever, and is the only station.
        assert_eq!(replies(&turn).len(), 1);
        assert_eq!(runtime.machine.stations.len(), 1);
        // With only the QRM sounding beside your message, nobody calls.
        let air = view(&runtime).air.expect("on the air");
        let early = air.heard_from.plus_sec(0.05);
        assert_eq!(air.calling(early), 0);
        assert!(!view(&runtime).qso.nobody_calling, "the station you work");
        let nr = serial(replies(&turn)[0]);
        runtime.finish_turn();
        let fields = Fields {
            call: "K5XYZ".into(),
            exch: nr.clone(),
            ..Fields::default()
        };
        let enter = esm(&view(&runtime).qso, &fields, &RULES[0], Side::Dx);
        assert_eq!(enter.label, "TU & log", "the hint knows nothing of it");
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        assert_eq!(view(&runtime).clean, 1);
    }

    /// QRM still sending when you key goes on in the next turn from the
    /// character sounding then, as a station on your frequency does not
    /// stop for you; one done before you key is gone with its turn.
    #[test]
    fn qrm_goes_on_over_your_next_message() {
        let mut runtime = habits_runtime(1, &["K5XYZ"], CallerHabits::Rough);
        runtime.finish_turn();
        let qrm = runtime
            .machine
            .qrm
            .as_mut()
            .expect("rough habits bring QRM");
        qrm.start_at(0);
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let first = qrm_in(runtime.last_turn())[0].clone();
        runtime.run_to(runtime.now + 600);
        runtime.key(OpKey::Again, "K5XYZ", "");
        let next = runtime.last_turn().clone();
        let going_on = qrm_in(&next);
        assert_eq!(going_on.len(), 1, "{next:?}");
        assert_eq!(going_on[0].start_sec, 0.0);
        assert!(
            first.text.ends_with(&going_on[0].text),
            "{first:?} {going_on:?}"
        );
        assert!(
            going_on[0].text.len() < first.text.len(),
            "from where it was"
        );
        assert_eq!(going_on[0].voice, first.voice);
    }

    /// A run's settings under rough habits, which bring QRM, with Show copy
    /// on, and full break-in as `qsk` says.
    fn qrm_settings(qsk: bool) -> TrainingSettings {
        let mut settings = run_settings(25.0);
        settings.contest.contest_habits = CallerHabits::Rough;
        settings.contest.contest_show_copy = true;
        settings.contest.contest_qsk = qsk;
        settings
    }

    /// A run under [`qrm_settings`] from `seed` whose first CQ, after a
    /// second of countdown, has QRM sending under it from its start.
    fn qrm_under_the_first_cq(seed: u64, qsk: bool) -> Runtime<ScriptCalls> {
        let mut settings = qrm_settings(qsk);
        settings.playback.countdown_sec = 1;
        let mut runtime = Runtime::start(seed, settings, ScriptCalls::new(&["K5XYZ"]));
        runtime
            .machine
            .qrm
            .as_mut()
            .expect("rough habits bring QRM")
            .start_at(0);
        runtime.run_to(1_000);
        runtime
    }

    /// Esc stops your station, not a station on your frequency that is not
    /// calling you. Your message cut short, QRM still sending goes on from
    /// where it was, as it does over a key, alone in a turn of its own
    /// when nobody else is sending: here your first CQ goes back, and the
    /// caller it brought with it. Your message out, Esc stops nothing, and
    /// the QRM sends on in the turn as it was.
    #[test]
    fn qrm_goes_on_past_an_esc() {
        for seed in 1..20 {
            let mut runtime = qrm_under_the_first_cq(seed, false);
            let cq = runtime.last_turn().clone();
            assert_eq!(yours(&cq), "CQ DL1ABC TEST");
            let first = qrm_in(&cq)[0].clone();
            assert_eq!(first.start_sec, 0.0, "seed {seed}");
            runtime.run_to(runtime.now + 800);
            let effects = runtime.key(OpKey::StopTx, "", "");
            let going = played(&effects).expect("the QRM goes on").clone();
            assert_eq!(yours(&going), "", "seed {seed}");
            assert!(senders(&going).is_empty(), "seed {seed}: it goes on alone");
            let rest = qrm_in(&going)[0].clone();
            assert_eq!(rest.start_sec, 0.0, "seed {seed}");
            assert!(
                first.text.ends_with(&rest.text) && rest.text.len() < first.text.len(),
                "seed {seed}: {} then {}",
                first.text,
                rest.text
            );
            assert_eq!(rest.voice, first.voice);
        }
        let mut stopped = 0;
        for seed in 1..40 {
            let mut runtime = habits_runtime(seed, &["K5XYZ"], CallerHabits::Rough);
            runtime.finish_turn();
            let qrm = runtime
                .machine
                .qrm
                .as_mut()
                .expect("rough habits bring QRM");
            qrm.start_at(0);
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let turn = runtime.last_turn().clone();
            let first = qrm_in(&turn)[0].clone();
            let your_end = turn.your_end_sec().expect("your call");
            if first.end_sec < your_end + 0.5 {
                continue;
            }
            // Your message out, the reply a tenth of a second away at the
            // soonest.
            runtime.run_to(runtime.now + ms(your_end + 0.05));
            assert!(runtime.key(OpKey::StopTx, "K5XYZ", "").is_empty());
            assert_eq!(runtime.playing_id(), Some(turn.id), "seed {seed}");
            stopped += 1;
        }
        assert!(stopped > 0, "no QRM outlasted your message");
    }

    /// Without full break-in, QRM some of which went out under your
    /// message is no send heard, as a caller's is not: past an Esc that
    /// cuts your message short its rest sounds, the receiver open again,
    /// but Show copy shows none of it, whether it goes on alone or beside a
    /// caller answering the garbage. With full break-in it was heard
    /// throughout, and Show copy shows the whole of it. Show copy used to
    /// show the whole of it either way.
    #[test]
    fn qrm_the_receiver_cut_into_stays_unshown_after_esc() {
        let qrm_shown = |runtime: &Runtime<ScriptCalls>| -> Vec<String> {
            view(runtime)
                .shown
                .into_iter()
                .filter(|text| text.qrm)
                .map(|text| text.text)
                .collect()
        };
        let mut tried = [0; 2];
        for qsk in [false, true] {
            for seed in 1..20 {
                // Alone: your first CQ goes back with the caller it brought.
                let alone = qrm_under_the_first_cq(seed, qsk);
                // Beside K5XYZ, which answers the garbage of your call and
                // exchange.
                let calls = ScriptCalls::new(&["K5XYZ"]);
                let mut beside = Runtime::start(seed, qrm_settings(qsk), calls);
                beside.finish_turn();
                let qrm = beside.machine.qrm.as_mut().expect("rough habits bring QRM");
                qrm.start_at(0);
                beside.key(OpKey::Esm, "K5XYZ", "");
                for (mut runtime, calling) in [(alone, false), (beside, true)] {
                    let turn = runtime.last_turn().clone();
                    let first = qrm_in(&turn)[0].clone();
                    // A station that came earlier may still hold the
                    // frequency.
                    if first.start_sec > 0.0 {
                        continue;
                    }
                    tried[usize::from(calling)] += 1;
                    assert_eq!(turn.muted(&first, qsk), !qsk);
                    runtime.run_to(runtime.now + 800);
                    let effects = runtime.key(OpKey::StopTx, "", "");
                    let going = played(&effects).expect("it goes on").clone();
                    assert_eq!(senders(&going).is_empty(), !calling, "seed {seed}");
                    let sends = qrm_in(&going);
                    assert!(first.text.ends_with(&sends[0].text));
                    assert!(!going.muted(sends[0], qsk), "it sounds");
                    // Any QRM sends after it in the turn are heard whole.
                    let shown: Vec<String> = qsk
                        .then(|| first.text.clone())
                        .into_iter()
                        .chain(sends[1..].iter().map(|send| send.text.clone()))
                        .collect();
                    let case = format!("seed {seed}, qsk {qsk}, calling {calling}");
                    assert_eq!(qrm_shown(&runtime), shown, "{case}");
                    runtime.finish_turn();
                    assert_eq!(qrm_shown(&runtime), shown, "{case}: once it ends");
                }
            }
        }
        assert!(tried.iter().all(|tried| *tried >= 20), "{tried:?}");
    }

    /// QRM carried over your key, its rest timed from where it was heard,
    /// never sends over itself: here its sound started six seconds late, a
    /// retried start, so its rest runs on well past where it was planned to
    /// end, and its next message, or the next station's, waits for it.
    #[test]
    fn qrm_carried_over_a_key_never_sends_over_itself() {
        let mut carried = 0;
        for seed in 1..34 {
            let mut runtime = habits_runtime(seed, &["K5XYZ"], CallerHabits::Rough);
            runtime.finish_turn();
            let qrm = runtime
                .machine
                .qrm
                .as_mut()
                .expect("rough habits bring QRM");
            qrm.start_at(0);
            runtime.key(OpKey::Esm, "K5XYZ", "");
            let id = runtime.last_turn().id;
            let keyed = runtime.now;
            runtime.run_to(keyed);
            runtime.apply(ContestEvent::TurnStarted {
                id,
                heard_from: MonoMs(keyed + 6_000),
                output_from: MonoMs(keyed + 6_000),
            });
            runtime.now = keyed + 7_000;
            runtime.key(OpKey::Again, "K5XYZ", "");
            runtime.finish_turn();
            for turn in &runtime.turns {
                let sent = qrm_in(turn);
                for (at, one) in sent.iter().enumerate() {
                    for other in &sent[..at] {
                        assert!(
                            one.start_sec >= other.end_sec || other.start_sec >= one.end_sec,
                            "seed {seed}: {other:?} and {one:?}"
                        );
                    }
                }
            }
            carried += usize::from(qrm_in(runtime.last_turn()).len() > 1);
        }
        assert!(
            carried > 5,
            "only {carried} turns with a rest and more after it"
        );
    }

    /// Under rough habits a caller now and then fumbles a digit of its
    /// serial, sends `EEEEE` and the serial right: what it sent is the
    /// serial right, so logged so the QSO is clean, its group keeping the
    /// token sent right; logged as fumbled, its number is wrong.
    #[test]
    fn a_fumbled_serial_is_copied_as_put_right() {
        let fumbling = |seed: &u64| {
            let runtime = habits_runtime(*seed, &["K5XYZ"], CallerHabits::Rough);
            runtime.machine.stations[0].caller.fumbles()
        };
        let seeds: Vec<u64> = (1..400).filter(fumbling).take(8).collect();
        let mut fumbled = 0;
        for seed in seeds {
            for logged_right in [true, false] {
                let mut runtime = habits_runtime(seed, &["K5XYZ"], CallerHabits::Rough);
                let exchange = to_its_exchange(&mut runtime);
                let Some(wrong) = exchange
                    .split_whitespace()
                    .find_map(|word| word.strip_suffix("EEEEE"))
                else {
                    // Its serial had no digit to fumble in its last two.
                    continue;
                };
                fumbled += 1;
                let right = serial(&exchange);
                let logged = if logged_right {
                    right.clone()
                } else {
                    wrong.to_string()
                };
                runtime.key(OpKey::Esm, "K5XYZ", &logged);
                runtime.finish_turn();
                let check = match &view(&runtime).log[0].status {
                    RowStatus::Checked(check) => check.clone(),
                    RowStatus::Pending => panic!("seed {seed}: still waiting"),
                };
                if logged_right {
                    assert!(check.is_clean(), "seed {seed}: {exchange} {check:?}");
                } else {
                    assert_eq!(check.errors, [QsoError::Field(Field::Nr)], "seed {seed}");
                }
                runtime.now += 1_000;
                let _ = runtime.apply(ContestEvent::FinishNow);
                let (result, _) = result(&runtime);
                assert_eq!(
                    result.groups[1].sent, right,
                    "seed {seed}: sent is the serial right"
                );
            }
        }
        assert!(fumbled >= 4, "only {fumbled} fumbled");
    }

    /// Under rough habits a caller now and then takes a slip of its call
    /// for its own: it sends its exchange with no correction, so Enter logs
    /// the slip, and the check shows the call busted, with the station's
    /// own, as MorseRunner's does.
    #[test]
    fn a_slip_a_careless_caller_took_is_logged_busted() {
        let mut taken = 0;
        for seed in 1..300 {
            let mut runtime = habits_runtime(seed, &["K5XYZ"], CallerHabits::Rough);
            runtime.finish_turn();
            runtime.key(OpKey::Esm, "K5XYW", "");
            let answer = replies(runtime.last_turn())[0].to_string();
            if answer.contains("K5XYZ") {
                continue;
            }
            taken += 1;
            runtime.finish_turn();
            let nr = serial(&answer);
            let fields = Fields {
                call: "K5XYW".into(),
                exch: nr.clone(),
                ..Fields::default()
            };
            let enter = esm(&view(&runtime).qso, &fields, &RULES[0], Side::Dx);
            assert_eq!(enter.label, "TU & log", "seed {seed}: {answer}");
            assert!(!enter.corrected);
            runtime.key(OpKey::Esm, "K5XYW", &nr);
            runtime.finish_turn();
            let row = &view(&runtime).log[0];
            assert_eq!(row.call, "K5XYW");
            assert_eq!(
                row.status,
                RowStatus::Checked(QsoCheck {
                    errors: vec![QsoError::Call {
                        correct: "K5XYZ".to_string()
                    }],
                    dupe: false,
                }),
                "seed {seed}"
            );
            if taken == 3 {
                break;
            }
        }
        assert_eq!(taken, 3, "only {taken} slips taken");
    }

    /// Under rough habits a careless operator sends its exchange with its
    /// call put right before it has yours, after a slip of its call or a
    /// partial: heard, it reads as a correction, the exchange in it a copy
    /// once it begins to reach you; your call and exchange then bring the
    /// exchange again, which it owed and you did not ask for, and the QSO
    /// logs clean, with no repeat of it charged: only the call a partial
    /// asked for.
    #[test]
    fn an_exchange_sent_early_is_heard_and_the_qso_logs_clean() {
        let early = |seed: &u64| {
            let runtime = habits_runtime(*seed, &["K5XYZ"], CallerHabits::Rough);
            runtime.machine.stations[0].caller.sends_early()
        };
        let seeds: Vec<u64> = (1..4_000).filter(early).take(5).collect();
        assert_eq!(seeds.len(), 5, "careless operators");
        for seed in seeds {
            for (key, sent, asked) in [(OpKey::HisCall, "K5XYW", 0), (OpKey::Esm, "K5X?", 1)] {
                let case = format!("seed {seed}, {sent}");
                let mut runtime = habits_runtime(seed, &["K5XYZ"], CallerHabits::Rough);
                runtime.finish_turn();
                runtime.key(key, sent, "");
                let fix = replies(runtime.last_turn())[0].to_string();
                let Some(Copied::Correction(call, Some(exch))) = copy_of(&fix) else {
                    // It took the slip for its call, as one in 25 does.
                    assert_eq!(sent, "K5XYW", "{case}: {fix}");
                    continue;
                };
                assert_eq!(call, "K5XYZ", "{case}");
                runtime.finish_turn();
                runtime.key(OpKey::Esm, "K5XYZ", &exch);
                assert_eq!(yours(runtime.last_turn()), "K5XYZ 5NN TT1", "{case}");
                runtime.finish_turn();
                runtime.key(OpKey::Esm, "K5XYZ", &exch);
                runtime.finish_turn();
                let log = &view(&runtime).log;
                assert_eq!(view(&runtime).clean, 1, "{case}: {log:?}");
                assert_eq!(log[0].asked, asked, "{case}");
                runtime.now += 1_000;
                let _ = runtime.apply(ContestEvent::FinishNow);
                let (result, run) = result(&runtime);
                let summary = run.summary(&result.groups).expect("its own groups");
                assert_eq!(summary.clean, 1, "{case}");
                assert_eq!(summary.repeated, asked as usize, "{case}");
                // Its exchange was copied as first sent, never asked for.
                let nr = result.group_timings.last().expect("its serial");
                assert_eq!(nr.sends, 1, "{case}");
            }
        }
    }

    /// Under real habits callers send their numbers cut, and a copy typed
    /// in plain digits is read by its value: a logger's 7 for `TT7`.
    #[test]
    fn a_cut_serial_copied_in_digits_is_clean() {
        let mut cut = 0;
        for seed in 1..40 {
            let mut runtime = habits_runtime(seed, &["K5XYZ"], CallerHabits::Real);
            let exchange = to_its_exchange(&mut runtime);
            let sent = serial(&exchange);
            let value = read_number(&sent).expect("a number").to_string();
            cut += usize::from(sent != value);
            runtime.key(OpKey::Esm, "K5XYZ", &value);
            runtime.finish_turn();
            assert_eq!(view(&runtime).clean, 1, "seed {seed}: {sent} as {value}");
        }
        assert!(cut > 10, "only {cut} cut");
    }

    /// A run worked by hand with Show copy on, its callers sending `calls`
    /// in turn.
    fn shown_runtime(calls: &[&str]) -> Runtime<ScriptCalls> {
        let mut settings = run_settings(25.0);
        settings.contest.contest_show_copy = true;
        Runtime::start(1, settings, ScriptCalls::new(calls))
    }

    /// Fields typed as `edits` have them: each field's text, and when.
    fn typed(edits: &[(EntryField, &str, u64)]) -> Fields {
        let mut fields = Fields::default();
        for (field, text, at) in edits {
            fields.edit(
                *field,
                (*text).to_string(),
                MonoMs(*at),
                Some(MonoMs(*at)),
                crate::contest::ContestKind::Wpx.rules(),
                Side::Dx,
            );
        }
        fields
    }

    /// With Show copy on, each caller's text shows a moment after its send
    /// ends, kept once the turn is over till the next; with it off the
    /// screen is told nothing of it.
    #[test]
    fn show_copy_shows_each_text_a_moment_after_it_ends() {
        // Off, the screen is told nothing.
        let mut off = runtime(&["K5XYZ"]);
        assert!(view(&off).shown.is_empty());
        off.finish_turn();
        assert!(view(&off).shown.is_empty());
        let mut runtime = shown_runtime(&["K5XYZ"]);
        let cq = runtime.last_turn().clone();
        let shown = view(&runtime).shown;
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert_eq!(shown[0].text, "K5XYZ");
        assert!(!shown[0].qrm);
        let end = whole_ms(cq.segments[1].end_sec);
        assert_eq!(shown[0].at, MonoMs(end + SHOWN_AFTER_MS));
        runtime.finish_turn();
        assert_eq!(view(&runtime).shown, shown, "kept once the turn is over");
        // The next turn's give way to it.
        runtime.now += 2_000;
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let exchange = replies(runtime.last_turn())[0].to_string();
        let shown = view(&runtime).shown;
        assert_eq!(
            shown
                .iter()
                .map(|text| text.text.as_str())
                .collect::<Vec<_>>(),
            [exchange.as_str()]
        );
        // And to a pause, the air quiet or not.
        runtime.finish_turn();
        assert!(!view(&runtime).shown.is_empty());
        runtime.apply(ContestEvent::Pause {
            why: PauseWhy::You,
            at: MonoMs(runtime.now),
        });
        assert!(view(&runtime).shown.is_empty());
    }

    /// With Show copy on, a call and an exchange whose text showed before
    /// you copied them keep what you had when it showed: a later edit never
    /// changes that copy, and the showing counts as a send. The log keeps
    /// what you logged, and checks it, and the run is marked assisted.
    #[test]
    fn show_copy_keeps_what_you_had_when_the_text_showed() {
        use EntryField::{Call, Exchange};
        let mut runtime = shown_runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let shown_at = view(&runtime).shown[0].at.0;
        // "K5X" by ear as the call ended, the rest put right from the
        // screen after it showed.
        runtime.now = shown_at + 1_500;
        let fields = typed(&[
            (Call, "K5X", shown_at - 200),
            (Call, "K5XYZ", shown_at + 900),
        ]);
        runtime.apply(ContestEvent::Key {
            key: OpKey::Esm,
            fields: Box::new(fields.clone()),
        });
        let exchange = replies(runtime.last_turn())[0].to_string();
        let nr = serial(&exchange);
        runtime.finish_turn();
        let exch_shown = view(&runtime).shown[0].at.0;
        // One digit by ear, the rest after the exchange showed.
        let first = &nr[..1];
        let mut fields = fields;
        fields.edit(
            Exchange,
            first.to_string(),
            MonoMs(exch_shown - 100),
            Some(MonoMs(exch_shown - 100)),
            crate::contest::ContestKind::Wpx.rules(),
            Side::Dx,
        );
        fields.edit(
            Exchange,
            nr.clone(),
            MonoMs(exch_shown + 400),
            Some(MonoMs(exch_shown + 400)),
            crate::contest::ContestKind::Wpx.rules(),
            Side::Dx,
        );
        runtime.now = exch_shown + 1_000;
        runtime.apply(ContestEvent::Key {
            key: OpKey::Esm,
            fields: Box::new(fields),
        });
        runtime.finish_turn();
        assert_eq!(view(&runtime).clean, 1, "the log keeps what you logged");
        runtime.now += 1_000;
        let _ = runtime.apply(ContestEvent::FinishNow);
        let (result, run) = result(&runtime);
        assert_eq!(result.alphabet_fingerprint, "contest:wpx:a");
        assert!(run.assisted && result.assisted());
        let copies: Vec<(&str, &str, u32)> = result
            .groups
            .iter()
            .zip(&result.group_timings)
            .take(2)
            .map(|(group, timing)| (group.sent.as_str(), group.received.as_str(), timing.sends))
            .collect();
        assert_eq!(copies, [("K5XYZ", "K5X", 2), (nr.as_str(), first, 2)]);
        // The record holds what was logged, so the check and the score are
        // worked out again as they were.
        assert_eq!(run.qsos[0].lc, "K5XYZ");
        assert_eq!(run.qsos[0].l, std::slice::from_ref(&nr));
        assert_eq!(run.clean_qsos(&result.groups), Some(1));
        let log = run
            .log(&result.groups, &result.group_timings)
            .expect("its log");
        assert_eq!(log[0].call, "K5XYZ");
        assert_eq!(log[0].fields[0].1, nr);
        let score = run.score(&result.groups).expect("a score");
        assert_eq!(score.verified, score.raw);
    }

    /// With Show copy on, what you copied and sent before its text showed
    /// was copied by ear, and keeps that copy, with no send for a showing
    /// that came after.
    #[test]
    fn a_copy_sent_before_its_text_showed_is_kept() {
        let mut runtime = shown_runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let shown_at = view(&runtime).shown[0].at.0;
        assert!(
            runtime.now < shown_at,
            "the turn ends before its text shows"
        );
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let nr = serial(replies(runtime.last_turn())[0]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        runtime.now += 1_000;
        let _ = runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        let sends: Vec<u32> = result
            .group_timings
            .iter()
            .take(2)
            .map(|timing| timing.sends)
            .collect();
        assert_eq!(sends, [1, 1]);
        assert_eq!(result.groups[0].received, "K5XYZ");
        // Assisted all the same: the text was there to read.
        assert_eq!(result.alphabet_fingerprint, "contest:wpx:a");
    }

    /// With Show copy on and full break-in, a pile-up caller you key over
    /// is shown whole once its send ends, not the rest it went on with.
    #[test]
    fn a_caller_keyed_over_is_shown_whole() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_style = RunStyle::Pileup;
        settings.contest.contest_show_copy = true;
        settings.contest.contest_qsk = true;
        settings.band.stations_min = 1;
        settings.band.stations_max = 1;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        let mut runtime = Runtime::start(1, settings.clamp(), ScriptCalls::new(&["K5XYZ"]));
        let cq = runtime.last_turn().clone();
        let start = cq.segments[1].start_sec;
        // Into its second character.
        runtime.run_to(runtime.now + ((start + 0.6) * 1_000.0) as u64);
        runtime.key(OpKey::Again, "", "");
        let over = runtime
            .last_turn()
            .segments
            .iter()
            .find(|segment| segment.who == Sender::Caller(0))
            .expect("it goes on")
            .clone();
        assert_ne!(over.text, "K5XYZ", "a rest of it");
        let shown: Vec<String> = view(&runtime)
            .shown
            .into_iter()
            .map(|text| text.text)
            .collect();
        assert_eq!(
            shown.first().map(String::as_str),
            Some("K5XYZ"),
            "{shown:?}"
        );
    }

    /// Each group of a run's result: what was sent, what it kept as
    /// received, and its sends.
    fn kept(result: &SessionResult) -> Vec<(&str, &str, u32)> {
        result
            .groups
            .iter()
            .zip(&result.group_timings)
            .map(|(group, timing)| (group.sent.as_str(), group.received.as_str(), timing.sends))
            .collect()
    }

    /// When the text Show copy shows beginning with `call` shows.
    fn shown_at(runtime: &Runtime<ScriptCalls>, call: &str) -> u64 {
        view(runtime)
            .shown
            .iter()
            .find(|text| text.text.starts_with(call))
            .unwrap_or_else(|| panic!("{call} shown"))
            .at
            .0
    }

    /// Work W1AW by ear once it is calling, and end the run: its number.
    fn work_w1aw(runtime: &mut Runtime<ScriptCalls>) -> String {
        runtime.key(OpKey::Esm, "W1AW", "");
        let nr = serial(replies(runtime.last_turn())[0]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "W1AW", &nr);
        runtime.finish_turn();
        runtime.now += 1_000;
        runtime.apply(ContestEvent::FinishNow);
        nr
    }

    /// With Show copy on, a single caller whose text shows while the Call
    /// field still holds the call of the station F3 thanked, not logged
    /// yet, had nothing copied of it: that call is the other station's,
    /// and no copy of this one.
    #[test]
    fn a_call_thanked_and_not_yet_logged_is_no_copy_of_the_next_caller() {
        let mut runtime = shown_runtime(&["K5XYZ", "W1AW"]);
        let nr = serial(&to_its_exchange(&mut runtime));
        runtime.key(OpKey::Tu, "K5XYZ", &nr);
        runtime.finish_turn();
        // W1AW answers the TU, and its call shows; K5XYZ is logged after.
        runtime.now = shown_at(&runtime, "W1AW") + 200;
        runtime.key(OpKey::LogOnly, "K5XYZ", &nr);
        let w1aw = work_w1aw(&mut runtime);
        let (result, run) = result(&runtime);
        assert_eq!(
            kept(&result)[2..],
            [("W1AW", "", 2), (w1aw.as_str(), w1aw.as_str(), 1)]
        );
        assert_eq!(run.clean_qsos(&result.groups), Some(2), "both logged right");
    }

    /// With Show copy on, a single caller whose text shows while the Call
    /// field still holds the call of a station gone had nothing copied of
    /// it either.
    #[test]
    fn a_gone_stations_call_is_no_copy_of_the_next_caller() {
        let mut runtime = shown_runtime(&["K5XYZ", "W1AW"]);
        runtime.finish_turn();
        runtime.key(OpKey::HisCall, "K5XYZ", "");
        runtime.finish_turn();
        // A CQ before its exchange sends it away; W1AW answers the CQ.
        runtime.key(OpKey::Cq, "K5XYZ", "");
        assert_eq!(view(&runtime).qso.gone, ["K5XYZ"]);
        runtime.finish_turn();
        runtime.now = shown_at(&runtime, "W1AW") + 500;
        // Enter clears the fields of the station gone.
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let w1aw = work_w1aw(&mut runtime);
        let (result, _) = result(&runtime);
        let w1aw_groups: Vec<_> = kept(&result)
            .into_iter()
            .filter(|(sent, ..)| *sent == "W1AW" || *sent == w1aw)
            .collect();
        assert_eq!(
            w1aw_groups,
            [("W1AW", "", 2), (w1aw.as_str(), w1aw.as_str(), 1)]
        );
    }

    /// With Show copy on, in a pile-up the calls that show while you type
    /// one of them had nothing copied of them: what was in the Call field
    /// was typed for another. A caller worked later keeps that empty copy,
    /// the showing a send of its own.
    #[test]
    fn a_pile_up_caller_shown_while_you_type_another_keeps_nothing() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_style = RunStyle::Pileup;
        settings.contest.contest_show_copy = true;
        settings.band.stations_min = 3;
        settings.band.stations_max = 3;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        let calls = ["K5XYZ", "W7ABC", "G4ABC"];
        let mut runtime = Runtime::start(1, settings.clamp(), ScriptCalls::new(&calls));
        runtime.finish_turn();
        let shown = view(&runtime).shown;
        assert_eq!(shown.len(), 3, "{shown:?}");
        let (first, last) = (shown[0].at.0, shown[2].at.0);
        runtime.now = last + 500;
        runtime.apply(ContestEvent::Key {
            key: OpKey::Esm,
            fields: Box::new(typed(&[(EntryField::Call, "K5XYZ", first - 300)])),
        });
        let frozen: Vec<Option<&str>> = runtime
            .machine
            .stations
            .iter()
            .map(|station| {
                station
                    .heard
                    .call_frozen
                    .as_ref()
                    .map(|(had, _)| had.as_str())
            })
            .collect();
        assert_eq!(frozen, [Some("K5XYZ"), Some(""), Some("")]);
        work_with_enter(&mut runtime, "K5XYZ", 0);
        work_with_enter(&mut runtime, "W7ABC", 1);
        runtime.now += 1_000;
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        let calls: Vec<_> = kept(&result)
            .into_iter()
            .filter(|(sent, ..)| calls.contains(sent))
            .collect();
        assert_eq!(calls, [("K5XYZ", "K5XYZ", 2), ("W7ABC", "", 2)]);
    }

    /// With Show copy on, a text that has not shown when the next turn
    /// goes out never shows: a `?` keyed in the moment after a call ends
    /// leaves the copy to the call it brings, cut off when that one shows.
    #[test]
    fn a_text_not_yet_shown_gives_way_to_the_next_send() {
        use EntryField::Call;
        let mut runtime = shown_runtime(&["K5XYZ"]);
        runtime.finish_turn();
        let first = shown_at(&runtime, "K5XYZ");
        runtime.now = first - SHOWN_AFTER_MS + 300;
        runtime.apply(ContestEvent::Key {
            key: OpKey::Again,
            fields: Box::new(typed(&[(Call, "K5X", first - 1_500)])),
        });
        assert_eq!(runtime.machine.stations[0].heard.call_shown, None);
        runtime.finish_turn();
        let again = shown_at(&runtime, "K5XYZ");
        assert!(again > first, "the call it brought");
        runtime.now = again + 500;
        runtime.apply(ContestEvent::Key {
            key: OpKey::Esm,
            fields: Box::new(typed(&[
                (Call, "K5X", first - 1_500),
                (Call, "K5XY", again - 100),
                (Call, "K5XYZ", again + 200),
            ])),
        });
        let nr = serial(replies(runtime.last_turn())[0]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        runtime.now += 1_000;
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        // The `?` asked with "K5X" in the field is a repeat before it.
        assert_eq!(kept(&result)[0], ("K5XYZ", "K5XY", 3));
    }

    /// With Show copy on, in a pile-up a call kept when its text showed
    /// that another station then takes from you, and this one never took,
    /// was typed for that one: of this one you had nothing.
    #[test]
    fn a_kept_call_another_station_takes_is_no_copy() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_style = RunStyle::Pileup;
        settings.contest.contest_show_copy = true;
        settings.band.stations_min = 2;
        settings.band.stations_max = 2;
        settings.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        let mut runtime =
            Runtime::start(1, settings.clamp(), ScriptCalls::new(&["K5XYZ", "K5XYW"]));
        runtime.finish_turn();
        let shown = view(&runtime).shown;
        assert_eq!(shown.len(), 2, "{shown:?}");
        runtime.now = shown[1].at.0 + 500;
        // "K5XW" is close to both, and goes out with your exchange: K5XYW,
        // the nearer, takes it.
        runtime.apply(ContestEvent::Key {
            key: OpKey::Esm,
            fields: Box::new(typed(&[(EntryField::Call, "K5XW", shown[0].at.0 - 300)])),
        });
        let (taker, other) = (1, 0);
        assert_eq!(runtime.machine.working, Some(taker));
        let frozen = |runtime: &Runtime<ScriptCalls>, index: usize| {
            runtime.machine.stations[index]
                .heard
                .call_frozen
                .clone()
                .map(|(had, _)| had)
        };
        assert_eq!(frozen(&runtime, other).as_deref(), Some("K5XW"));
        // It takes it as it answers.
        runtime.finish_turn();
        runtime.machine.settle_frozen(taker);
        runtime.machine.settle_frozen(other);
        assert_eq!(frozen(&runtime, taker).as_deref(), Some("K5XW"), "its own");
        assert_eq!(frozen(&runtime, other).as_deref(), Some(""));
    }

    /// With Show copy on, a text the sound failed before it showed never
    /// showed: a key just after Resume keeps no copy of it, and the copy
    /// by ear of the repeat stands.
    #[test]
    fn a_text_whose_sound_failed_never_showed() {
        let mut runtime = shown_runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let turn = runtime.last_turn().clone();
        let nr = serial(replies(&turn)[0]);
        let reply = sent_by(&turn, 0)[0].end_sec;
        let end = runtime.now + whole_ms(reply);
        runtime.sound_fails(turn.id);
        runtime.run_to(end);
        assert_eq!(runtime.playing_id(), Some(turn.id), "still on the air");
        assert!(runtime.machine.stations[0].heard.exch_shown.is_some());
        runtime.now = end + SHOWN_AFTER_MS / 2;
        runtime.apply(ContestEvent::TurnFailed { id: turn.id });
        assert!(paused_by_sound(&runtime));
        assert!(view(&runtime).shown.is_empty());
        runtime.now += 3_000;
        runtime.apply(ContestEvent::Resume);
        runtime.now += 200;
        runtime.key(OpKey::NrQuery, "K5XYZ", "");
        // Its number typed by ear as the repeat ends, and logged then.
        let repeat = runtime.last_turn().clone();
        let end = runtime.now + whole_ms(sent_by(&repeat, 0)[0].end_sec);
        runtime.run_to(end);
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        runtime.now += 1_000;
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        assert_eq!(kept(&result)[1], (nr.as_str(), nr.as_str(), 2));
    }

    /// When each character of the word `token` ends in `segment` of a turn
    /// whose sound went out at `from`, in run time.
    fn token_ends(
        runtime: &Runtime<ScriptCalls>,
        from: u64,
        segment: &Segment,
        token: &str,
    ) -> Vec<u64> {
        let at = segment
            .text
            .split(' ')
            .scan(0, |next, word| {
                let at = *next;
                *next += word.len() + 1;
                Some((at, word))
            })
            .find(|(_, word)| *word == token)
            .map(|(at, _)| at)
            .unwrap_or_else(|| panic!("{token} in {:?}", segment.text));
        char_spans(&segment.text, &runtime.machine.settings, &segment.voice)
            .into_iter()
            .filter(|span| (at..at + token.len()).contains(&span.at))
            .map(|span| from + whole_ms(segment.start_sec) + whole_ms(span.end_sec))
            .collect()
    }

    /// `text` keyed into `field` a character at a time, each `after` ms
    /// after the moment beside it ends: the edits [`typed`] takes.
    fn keyed_along(
        field: EntryField,
        before: &str,
        text: &str,
        ends: &[u64],
        after: u64,
    ) -> Vec<(EntryField, String, u64)> {
        text.char_indices()
            .zip(ends)
            .map(|((at, ch), end)| {
                let held = format!("{before}{}", &text[..at + ch.len_utf8()]);
                (field, held, end + after)
            })
            .collect()
    }

    /// K5XYZ's call keyed along as it is heard, then its exchange as that
    /// is, `after` ms after each character ends, and logged: the run's
    /// result, and what it sent for its exchange.
    fn worked_keying_along(
        runtime: &mut Runtime<ScriptCalls>,
        after: u64,
    ) -> (SessionResult, String) {
        let cq = runtime.last_turn().clone();
        let call = sent_by(&cq, 0)[0].clone();
        assert_eq!(call.text, "K5XYZ", "a call alone");
        let mut edits = keyed_along(
            EntryField::Call,
            "",
            "K5XYZ",
            &token_ends(runtime, 0, &call, "K5XYZ"),
            after,
        );
        runtime.finish_turn();
        let typed_by = edits.last().map_or(0, |(_, _, at)| *at);
        runtime.now = runtime.now.max(typed_by + 10);
        let as_typed = |edits: &[(EntryField, String, u64)]| {
            let edits: Vec<(EntryField, &str, u64)> = edits
                .iter()
                .map(|(field, text, at)| (*field, text.as_str(), *at))
                .collect();
            typed(&edits)
        };
        runtime.apply(ContestEvent::Key {
            key: OpKey::Esm,
            fields: Box::new(as_typed(&edits)),
        });
        let sent_at = runtime.now;
        let turn = runtime.last_turn().clone();
        let reply = sent_by(&turn, 0)[0].clone();
        let tokens: Vec<String> = runtime.machine.stations[0]
            .caller
            .sent_exchange()
            .expect("its exchange drawn")
            .iter()
            .map(|(_, token)| token.clone())
            .collect();
        let mut exchange = String::new();
        for token in &tokens {
            let before = if exchange.is_empty() {
                String::new()
            } else {
                format!("{exchange} ")
            };
            let ends = token_ends(runtime, sent_at, &reply, token);
            edits.extend(keyed_along(
                EntryField::Exchange,
                &before,
                token,
                &ends,
                after,
            ));
            exchange = format!("{before}{token}");
        }
        runtime.finish_turn();
        let typed_by = edits.last().map_or(0, |(_, _, at)| *at);
        runtime.now = runtime.now.max(typed_by + 10);
        runtime.apply(ContestEvent::Key {
            key: OpKey::Esm,
            fields: Box::new(as_typed(&edits)),
        });
        runtime.finish_turn();
        runtime.now += 1_000;
        runtime.apply(ContestEvent::FinishNow);
        (result(runtime).0, exchange)
    }

    /// Each group's recognition times, as Letters reads them.
    fn recognised(result: &SessionResult) -> Vec<Vec<Option<u32>>> {
        result
            .groups
            .iter()
            .zip(&result.group_timings)
            .map(|(group, timing)| {
                timing.char_times.as_ref().map_or_else(Vec::new, |times| {
                    crate::recognition::recognise(&group.sent, &group.received, times)
                })
            })
            .collect()
    }

    /// A caller's call heard alone and once, and its exchange so, keyed
    /// along as each character is heard: every character is timed from its
    /// sound going out to its key, as a group's are.
    #[test]
    fn a_call_and_an_exchange_keyed_along_are_timed_by_the_character() {
        let mut runtime = runtime(&["K5XYZ"]);
        let (result, exchange) = worked_keying_along(&mut runtime, 300);
        let times = recognised(&result);
        assert_eq!(result.groups[0].received, "K5XYZ");
        assert_eq!(times.len(), 1 + exchange.split(' ').count(), "{times:?}");
        for (group, times) in result.groups.iter().zip(&times) {
            assert_eq!(times.len(), group.sent.chars().count(), "{group:?} untimed");
            for took in times {
                let took = took.unwrap_or_else(|| panic!("{group:?}: {times:?}"));
                assert!((299..=302).contains(&took), "{group:?}: {times:?}");
            }
        }
    }

    /// A key after a pause says nothing of a call heard before it: run time
    /// stands still while paused, and would leave the pause out of its time.
    #[test]
    fn a_key_after_a_pause_is_not_timed_for_a_call_heard_before_it() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        assert!(runtime.machine.stations[0].heard.call_sounded.is_some());
        let at = MonoMs(runtime.now);
        runtime.apply(ContestEvent::Pause {
            why: PauseWhy::You,
            at,
        });
        runtime.now += 60_000;
        runtime.apply(ContestEvent::Resume);
        runtime.now += 300;
        runtime.apply(ContestEvent::Key {
            key: OpKey::Esm,
            fields: Box::new(typed(&[(EntryField::Call, "K5XYZ", runtime.now)])),
        });
        let nr = serial(replies(runtime.last_turn())[0]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        runtime.now += 1_000;
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        // Nothing of its keys says how soon it was recognised, so nothing of
        // it is kept.
        assert_eq!(result.group_timings[0].char_times, None);
    }

    /// The sound failing pauses the run as Pause does: a key after Resume
    /// says nothing of a call heard before the failure either.
    #[test]
    fn a_key_after_a_sound_failure_is_not_timed_for_a_call_heard_before_it() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        assert!(runtime.machine.stations[0].heard.call_sounded.is_some());
        runtime.key(OpKey::Cq, "", "");
        let cq = runtime.last_turn().id;
        runtime.sound_fails(cq);
        runtime.run_to(runtime.now + 100);
        assert_eq!(runtime.playing_id(), Some(cq), "still on the air");
        runtime.apply(ContestEvent::TurnFailed { id: cq });
        assert!(paused_by_sound(&runtime));
        runtime.now += 60_000;
        runtime.apply(ContestEvent::Resume);
        runtime.now += 300;
        runtime.apply(ContestEvent::Key {
            key: OpKey::Esm,
            fields: Box::new(typed(&[(EntryField::Call, "K5XYZ", runtime.now)])),
        });
        let nr = serial(replies(runtime.last_turn())[0]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        runtime.now += 1_000;
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        assert_eq!(result.group_timings[0].char_times, None);
    }

    /// A failed turn gives the time since it first started back as paused,
    /// and what was heard in that time with it: an exchange heard whole on
    /// a retry lies ahead of the clock taken back, so the copy of it sent
    /// again after Resume is timed from the send heard then, not cut to the
    /// fastest a person types.
    #[test]
    fn nothing_heard_in_a_failed_turn_lies_ahead_of_the_clock() {
        let mut runtime = runtime(&["K5XYZ"]);
        runtime.finish_turn();
        runtime.key(OpKey::Esm, "K5XYZ", "");
        let turn = runtime.last_turn().clone();
        let keyed = runtime.now;
        runtime.sound_fails(turn.id);
        runtime.run_to(keyed + 100);
        // Tried again 2.5 s on, its exchange heard whole on the retry.
        let retry = keyed + 2_500;
        runtime.now = retry;
        runtime.apply(ContestEvent::TurnStarted {
            id: turn.id,
            heard_from: MonoMs(retry),
            output_from: MonoMs(retry),
        });
        let end = retry + whole_ms(sent_by(&turn, 0)[0].end_sec);
        runtime.run_to(end);
        assert_eq!(runtime.playing_id(), Some(turn.id), "still on the air");
        assert_eq!(runtime.machine.stations[0].heard.exch_ends.len(), 1);
        runtime.now = end + 100;
        runtime.apply(ContestEvent::TurnFailed { id: turn.id });
        assert!(paused_by_sound(&runtime));
        let held = view(&runtime).run_ms(MonoMs(runtime.now));
        let heard = &runtime.machine.stations[0].heard;
        assert!(
            heard
                .exch_ends
                .iter()
                .chain(&heard.call_ends)
                .chain(&heard.first)
                .all(|at| *at <= held),
            "{heard:?} past {held}"
        );
        runtime.now += 3_000;
        runtime.apply(ContestEvent::Resume);
        let again = next_turn(&mut runtime);
        let nr = serial(replies(&again)[0]);
        let end = runtime.now + whole_ms(sent_by(&again, 0)[0].end_sec);
        runtime.run_to(end);
        runtime.now = end + 800;
        runtime.key(OpKey::Esm, "K5XYZ", &nr);
        runtime.finish_turn();
        runtime.now += 1_000;
        runtime.apply(ContestEvent::FinishNow);
        let (result, _) = result(&runtime);
        assert_eq!(kept(&result)[1], (nr.as_str(), nr.as_str(), 1));
        assert_eq!(result.group_timings[1].time_to_complete_ms, 800.0);
    }

    /// An exchange counts as sent once it is on its way: one a pause cut
    /// before it began, or one lost with a failed sound, comes again after
    /// Resume with a word in front, as it would have gone out.
    #[test]
    fn an_exchange_that_never_went_out_keeps_its_lead_word() {
        let led = |runtime: &Runtime<ScriptCalls>, text: &str| {
            let first = text.split_whitespace().next().unwrap_or_default();
            runtime.machine.rules.their_lead.contains(&first)
        };
        // Paused before it began.
        let mut paused = runtime(&["K5XYZ"]);
        paused.finish_turn();
        paused.key(OpKey::Esm, "K5XYZ", "");
        let planned = replies(paused.last_turn())[0].to_string();
        assert!(led(&paused, &planned), "{planned}");
        paused.run_to(paused.now + 100);
        paused.apply(ContestEvent::Pause {
            why: PauseWhy::You,
            at: MonoMs(paused.now),
        });
        paused.now += 3_000;
        paused.apply(ContestEvent::Resume);
        let again = next_turn(&mut paused);
        assert!(led(&paused, replies(&again)[0]), "{:?}", replies(&again));

        // Heard whole, then the sound failed.
        let mut failed = runtime(&["K5XYZ"]);
        failed.finish_turn();
        failed.key(OpKey::Esm, "K5XYZ", "");
        let turn = failed.last_turn().clone();
        failed.sound_fails(turn.id);
        let end = failed.now + whole_ms(sent_by(&turn, 0)[0].end_sec);
        failed.run_to(end);
        assert_eq!(failed.machine.stations[0].heard.exch_ends.len(), 1);
        failed.now = end + 100;
        failed.apply(ContestEvent::TurnFailed { id: turn.id });
        failed.now += 3_000;
        failed.apply(ContestEvent::Resume);
        let again = next_turn(&mut failed);
        assert!(led(&failed, replies(&again)[0]), "{:?}", replies(&again));
    }

    /// With Show copy, what you copy may be read off the screen: nothing of
    /// the run is timed.
    #[test]
    fn nothing_is_timed_with_show_copy() {
        let mut settings = run_settings(25.0);
        settings.contest.contest_show_copy = true;
        let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
        let (result, _) = worked_keying_along(&mut runtime, 300);
        assert!(!result.groups.is_empty());
        assert!(
            result
                .group_timings
                .iter()
                .all(|timing| timing.char_times.is_none()),
            "{:?}",
            result.group_timings
        );
    }

    /// A turn whose sound was tried again, or played late, went out
    /// otherwise than planned: nothing heard in it is timed.
    #[test]
    fn nothing_heard_in_a_turn_tried_again_or_played_late_is_timed() {
        // Tried again as it plays.
        let mut retried = runtime(&["K5XYZ"]);
        let cq = retried.last_turn().id;
        retried.step();
        retried.apply(ContestEvent::TurnStarted {
            id: cq,
            heard_from: MonoMs(retried.now),
            output_from: MonoMs(retried.now),
        });
        let (result, _) = worked_keying_along(&mut retried, 300);
        assert_eq!(result.group_timings[0].char_times, None, "the call");
        assert!(result.group_timings[1].char_times.is_some(), "the exchange");

        // Played late.
        let mut late = runtime(&["K5XYZ"]);
        let cq = late.last_turn().clone();
        let end = whole_ms(cq.duration_sec());
        late.run_to(end.saturating_sub(1));
        late.now = end;
        late.apply(ContestEvent::TurnHeldUp { id: cq.id });
        late.apply(ContestEvent::TurnEnded { id: cq.id });
        assert!(late.machine.untrusted.contains(&cq.id));
        assert_eq!(late.machine.stations[0].heard.call_sounded, None);
    }

    /// A group timed from a turn found late after it was logged is kept
    /// untimed.
    #[test]
    fn a_group_kept_from_a_turn_found_late_is_untimed() {
        let voice = your_voice(&run_settings(25.0), 25.0);
        let times = CharTimes {
            ends: vec![300],
            keys: vec![Some(700)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        let kept = group(
            "K",
            "K",
            400,
            1,
            &voice,
            Some(Timed {
                turn: 7,
                times: times.clone(),
            }),
        );
        assert_eq!(kept.stored(&[]).1.char_times, Some(times));
        assert_eq!(kept.stored(&[3, 7]).1.char_times, None);
    }

    /// What of a turn a browser fallen behind its clock skipped never
    /// reached you: a caller's send any of which it skipped is heard no
    /// more than one sent under your message. Show copy never shows it, it
    /// begins nothing for Enter or a copy, and asked for again it is the
    /// first you hear of it, no repeat; nothing in the turn is timed. A
    /// skip over your own CQ alone takes nothing of the call after it.
    #[test]
    fn a_callers_send_skipped_in_part_is_not_heard() {
        for skipped in [false, true] {
            for show_copy in [false, true] {
                let mut settings = run_settings(25.0);
                settings.contest.contest_show_copy = show_copy;
                let mut runtime = Runtime::start(1, settings, ScriptCalls::new(&["K5XYZ"]));
                let cq = runtime.last_turn().clone();
                let call = sent_by(&cq, 0)[0].clone();
                let (from_sec, until_sec) = if skipped {
                    (call.start_sec + 0.1, call.start_sec + 0.3)
                } else {
                    (0.2, (0.6f64).min(call.start_sec))
                };
                let case = format!("skipped {skipped}, Show copy {show_copy}");
                runtime.run_to(runtime.now + ms(until_sec) + 50);
                assert_eq!(runtime.playing_id(), Some(cq.id), "{case}");
                runtime.apply(ContestEvent::TurnMissed {
                    id: cq.id,
                    from_sec,
                    until_sec,
                });
                runtime.finish_turn();
                assert!(runtime.machine.untrusted.contains(&cq.id), "{case}");
                let heard = &runtime.machine.stations[0].heard;
                assert_eq!(heard.call_begun, !skipped, "{case}");
                assert_eq!(heard.call_ends.is_empty(), skipped, "{case}");
                let shown = view(&runtime).shown;
                assert_eq!(shown.len(), usize::from(show_copy && !skipped), "{case}");
                if show_copy {
                    continue;
                }
                // Asked for, its call comes again, and is heard: a repeat
                // only of a call heard before.
                runtime.now += 500;
                runtime.key(OpKey::Again, "", "");
                assert_eq!(replies(runtime.last_turn()), ["K5XYZ"], "{case}");
                runtime.finish_turn();
                runtime.key(OpKey::Esm, "K5XYZ", "");
                let exchange = replies(runtime.last_turn())[0].to_string();
                runtime.finish_turn();
                runtime.key(OpKey::Esm, "K5XYZ", &serial(&exchange));
                runtime.finish_turn();
                runtime.apply(ContestEvent::FinishNow);
                let call_sends = if skipped { 1 } else { 2 };
                assert_eq!(sends(&runtime), [call_sends, 1], "{case}");
            }
        }
    }

    /// A token is timed only from a send it is in once, as a word, with
    /// nothing else sounding over it.
    #[test]
    fn a_token_is_timed_only_heard_once_and_alone() {
        let settings = run_settings(25.0);
        let voice = your_voice(&settings, 25.0);
        let segment = |who: Sender, text: &str, start_sec: f64| {
            let end_sec = start_sec + sending_sec(text, &settings, &voice);
            Segment {
                who,
                text: text.to_string(),
                voice,
                start_sec,
                end_sec,
                fade: None,
                skip_sec: 0.0,
            }
        };
        let turn = |segments: Vec<Segment>| Turn {
            id: 1,
            at_run_ms: 0,
            starts_at: None,
            segments,
        };
        let caller = Sender::Caller(0);
        let whole = |turn: &Turn, token: &str| {
            let segment = &turn.segments[0];
            let spans = char_spans(&segment.text, &settings, &segment.voice);
            sounded_alone(turn, 0, &spans, 0..segment.text.len(), token)
        };
        let alone = turn(vec![segment(caller, "DE K5XYZ", 0.0)]);
        let ends = whole(&alone, "K5XYZ").expect("heard once, alone");
        assert_eq!(ends.len(), 5);
        assert!(ends.windows(2).all(|pair| pair[0] < pair[1]), "{ends:?}");
        let spans = char_spans("DE K5XYZ", &settings, &voice);
        assert_eq!(
            ends.last().copied(),
            Some(whole_ms(spans[spans.len() - 1].end_sec) as u32)
        );
        // Twice in the send, or not as a word of its own.
        assert_eq!(
            whole(&turn(vec![segment(caller, "K5XYZ K5XYZ", 0.0)]), "K5XYZ"),
            None
        );
        assert_eq!(
            whole(&turn(vec![segment(caller, "K5XYZW", 0.0)]), "K5XYZ"),
            None
        );
        // Under QRM, or under your own message with full break-in.
        for over in [Sender::Qrm, Sender::You] {
            let under = turn(vec![
                segment(caller, "K5XYZ", 0.0),
                segment(over, "CQ TEST", 0.2),
            ]);
            assert_eq!(whole(&under, "K5XYZ"), None, "{over:?}");
        }
        // Something sounding only before or after it is no matter.
        let after = turn(vec![
            segment(caller, "K5XYZ", 0.0),
            segment(Sender::Qrm, "CQ", 5.0),
        ]);
        assert!(whole(&after, "K5XYZ").is_some());
    }

    /// In a pile-up the callers call over one another: no call heard under
    /// another is timed.
    #[test]
    fn a_call_heard_under_another_is_not_timed() {
        let mut runtime = pileup(&["K5XYZ", "W7ABC", "G4ABC"], 3, 3);
        let cq = runtime.last_turn().clone();
        runtime.finish_turn();
        let callers: Vec<&Segment> = cq.segments.iter().filter(|s| s.who.is_caller()).collect();
        let mut overlapped = 0;
        for segment in &callers {
            let Sender::Caller(station) = segment.who else {
                continue;
            };
            let over = cq.segments.iter().any(|other| {
                !std::ptr::eq(other, *segment)
                    && other.start_sec < segment.end_sec
                    && other.end_sec > segment.start_sec
            });
            if over {
                overlapped += 1;
                let heard = &runtime.machine.stations[station as usize].heard;
                assert_eq!(heard.call_sounded, None, "{:?}", segment.text);
            }
        }
        assert!(overlapped > 0, "no caller called over another");
    }
}
