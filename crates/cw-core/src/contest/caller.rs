//! The stations that answer your CQ: how long one waits for you, how many
//! times it calls before it gives up, and what each of your messages makes
//! it do.
//!
//! A caller hears each of your messages whole, in order, and ends up in one
//! of a few states; what it sends next follows from the state. Which
//! of your calls it takes for its own is [`match_call`].

use super::data::{names::draw_name, reports::draw_report};
use super::esm::MessagePart;
use super::exchange::{Field, send_report, send_serial};
use super::place::Placement;
use super::rules::{ContestRules, Side};
use super::serial::SerialModel;
use super::{CallerHabits, RunStyle};
use crate::alignment::edit_distance;
use crate::callsign::{GeneratedCall, parse_callsign};
use crate::rng::{Rng, SplitMix64};
use crate::settings::TrainingSettings;
use crate::timing::{StationVoice, resolve_station};

/// Where a caller is in its QSO with you, as MorseRunner's operators are
/// (`DxOper.pas:17-88`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallerState {
    /// Calling you, not answered yet.
    NeedQso,
    /// You sent its call; it waits for your exchange.
    NeedNr,
    /// It has your exchange, but you sent its call wrong.
    NeedCall,
    /// You sent its call wrong, and it still waits for your exchange.
    NeedCallNr,
    /// It has all it needs: it sends its exchange and waits for your TU.
    NeedEnd,
    /// The QSO is over and in its log.
    Done,
    /// It gave up, or its QSO failed, and it left.
    Gone,
}

impl CallerState {
    /// Whether it is in a QSO with you, past calling and not yet over.
    pub(super) fn worked(self) -> bool {
        matches!(
            self,
            Self::NeedNr | Self::NeedCall | Self::NeedCallNr | Self::NeedEnd
        )
    }

    /// Whether it is still on the frequency, calling or worked.
    pub(super) fn active(self) -> bool {
        self == Self::NeedQso || self.worked()
    }
}

/// How a call you sent compares with a station's (§4.7 of the design).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallMatch {
    /// Its call, exactly.
    Exact,
    /// Its call with a slip in it, or a piece of it.
    Close,
    /// A partial with `?` in it that fits its call, and how many characters
    /// of the partial it fits beyond the `?`s: the more, the surer.
    Partial { fits: usize },
    /// Not its call.
    No,
}

/// How `sent` compares with `call`. A `?` stands for one character, and a
/// trailing one for any tail, matched anywhere in the call, as MorseRunner's
/// pattern is (`DxOper.pas:546-590`). Without a `?`, the edit distance
/// decides: none is exact; up to `(length − 1) / 2` edits of the call's
/// length, or the whole of `sent` inside the call, is close
/// (`DxOper.pas:597-631`).
pub(super) fn match_call(sent: &str, call: &str) -> CallMatch {
    if sent.contains('?') {
        return partial_fits(sent, call).map_or(CallMatch::No, |fits| CallMatch::Partial { fits });
    }
    let distance = edit_distance(sent, call) as usize;
    let length = call.chars().count();
    if distance == 0 {
        CallMatch::Exact
    } else if distance <= length.saturating_sub(1) / 2 || (!sent.is_empty() && call.contains(sent))
    {
        CallMatch::Close
    } else {
        CallMatch::No
    }
}

/// How `sent` is heard by the station signing `call`: as [`match_call`]
/// has it, but for a piece of its call that is no call itself, which asks
/// for the rest as a partial does, with nothing to put right. A piece fits
/// with every character it has; a slip that is no call, such as a dropped
/// digit, with the characters of the call it gets right, as MorseRunner
/// ranks it by edit distance (`DxOper.pas:597-631`), so it goes to the call
/// it is nearest.
fn heard_as(sent: &str, call: &str) -> CallMatch {
    match match_call(sent, call) {
        CallMatch::Close if parse_callsign(sent).is_none() => CallMatch::Partial {
            fits: if call.contains(sent) {
                sent.chars().count()
            } else {
                call.chars()
                    .count()
                    .saturating_sub(edit_distance(sent, call) as usize)
            },
        },
        found => found,
    }
}

/// How well a call you sent fits a station's, to rank the stations on the
/// frequency by: its own call first, then a slip in it, then a partial.
/// Within a kind, the station you work comes first, so a partial or a slip
/// that fits it goes to it, as you would mean it; then the fewer edits for
/// the call's length the better, or the larger the share of its call the
/// partial's characters cover. Only the best answer (§4.7 of the design).
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub(super) struct Likeness {
    /// 2 for its own call, 1 for a slip, 0 for a partial.
    tier: u8,
    /// Whether you are working it.
    worked: bool,
    /// How much of its call the match covers, from 0 to 1.
    share: f64,
}

impl Likeness {
    /// How well `sent`, heard as `found`, fits `call`, of a station you are
    /// working or not; none when it is not its call at all.
    fn of(found: CallMatch, sent: &str, call: &str, worked: bool) -> Option<Self> {
        let length = call.chars().count().max(1) as f64;
        let (tier, share) = match found {
            CallMatch::Exact => (2, 1.0),
            CallMatch::Close => (1, 1.0 - edit_distance(sent, call) as f64 / length),
            CallMatch::Partial { fits } => (0, fits as f64 / length),
            CallMatch::No => return None,
        };
        Some(Self {
            tier,
            worked,
            share,
        })
    }
}

/// How many characters of the partial `pattern` fit `call` beyond its
/// `?`s, if it fits anywhere in it. Matched anywhere, a trailing "any tail"
/// fits wherever the rest does, so it adds nothing to the search.
fn partial_fits(pattern: &str, call: &str) -> Option<usize> {
    let pattern: Vec<char> = pattern.chars().collect();
    let body = match pattern.split_last() {
        Some(('?', body)) => body,
        _ => &pattern[..],
    };
    let call: Vec<char> = call.chars().collect();
    let fits_at = |start: usize| {
        call.get(start..start + body.len()).is_some_and(|window| {
            body.iter()
                .zip(window)
                .all(|(wanted, heard)| *wanted == '?' || wanted == heard)
        })
    };
    (0..=call.len())
        .any(fits_at)
        .then(|| body.iter().filter(|c| **c != '?').count())
}

/// What a caller sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Says {
    /// Its call, calling you.
    Call,
    /// `NR?` or `AGN`: it wants your exchange.
    Query,
    /// Its call, correcting yours: `DE K5XYZ`.
    Correction,
    /// Its exchange.
    Exchange,
    /// What it sends after your TU, where the contest has it sign off:
    /// heard, and never logged.
    Signoff,
}

/// What a caller does after one of your messages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Reaction {
    /// It sends, after its reply delay.
    pub(super) replies: bool,
    /// What it sends answers your asking for it again: `?`, `NR?`, a
    /// partial. A call sent again so is a send you pay for; an exchange
    /// sent again is one whatever key drew it.
    pub(super) asked: bool,
    /// It took a call you sent for its own, exactly or close, or answered
    /// your partial or a piece of its call: you are working it now.
    pub(super) matched: bool,
    /// It took a call you sent for its own, exactly or close: it heard its
    /// call from you, not only a question about it.
    pub(super) called: bool,
    /// It took your exchange as sent to it, while you work it.
    pub(super) took_exchange: bool,
    /// Your exchange was for it, but it missed it and asks for it again.
    pub(super) missed_exchange: bool,
}

/// How many tries a caller makes before it gives up: `3 + RndRayleigh(3)`,
/// rounded, as MorseRunner draws it (`DxOper.pas:496-498`), four to
/// fourteen with a mean near seven.
const PATIENCE_BASE: u32 = 3;
const PATIENCE_RAYLEIGH_SCALE: f64 = 3.0;
const PATIENCE_MIN: u32 = 4;
const PATIENCE_MAX: u32 = 14;

/// How many of its reply timeouts in a row the station you work sits
/// through with nothing from you before it leaves, as MorseRunner's Caller
/// Stays (`DxOper.pas:319-335`).
const SILENT_TIMEOUTS: u32 = 3;

/// How long after your message a caller starts to reply, in seconds
/// (`DxOper.pas:225-232`).
const REPLY_DELAY_SEC: (f64, f64) = (0.1, 0.6);

/// A caller of skill `s` waits `(6 − s)` seconds for you, give or take half
/// of that (`DxOper.pas:282-287`).
const TIMEOUT_BASE_SEC: f64 = 6.0;
const TIMEOUT_SPREAD: f64 = 0.5;

/// How many pile-up callers send their call twice, as MorseRunner's do
/// (`DxOper.pas:500-502`). A single caller never does.
const CALLS_TWICE: f64 = 0.1;

/// How often a pile-up caller misses your exchange and asks for it again,
/// as MorseRunner's callers take it nine times in ten (`DxOper.pas:762-770`).
const MISSES_EXCHANGE: f64 = 0.1;

/// How far into its fading a caller starts, at most, in seconds: each is on
/// a path of its own, so no two fade together.
const FADE_OFFSET_MAX_SEC: f64 = 3_600.0;

/// How often the first exchange a caller sends starts with the first of
/// the contest's words for it rather than the second: `TU 5NN 214` rather
/// than `R 5NN 214`.
const FIRST_LEAD: f64 = 0.9;

/// How often a caller waiting for your exchange asks with the first of the
/// contest's queries rather than the second: `NR?` rather than `AGN`
/// (`DxOper.pas`, `GetReply`).
const FIRST_QUERY: f64 = 0.3;

/// One station calling you.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Caller {
    pub(super) call: GeneratedCall,
    /// How it sounds, drawn once: a repeat is the same operator again.
    pub(super) voice: StationVoice,
    pub(super) state: CallerState,
    /// When its reply timeout runs out, in run time, while it waits.
    pub(super) deadline: Option<u64>,
    /// 1 to 3: how quickly it gives up on a silence.
    skill: u32,
    /// Tries left while it calls.
    patience: u32,
    /// It heard you work someone else since it last called, so a bare `?`
    /// is not for it, as MorseRunner's `osNeedPrevEnd` (`DxOper.pas:744-748,
    /// 793`). Your CQ, TU, B4 or NIL, its own call, its next call, or a `?`
    /// with the Call field empty clears it.
    other_qso: bool,
    /// It answered your partial while it called, so your exchange now is
    /// for it, as MorseRunner's near match makes it `osNeedCallNr`
    /// (`DxOper.pas:734-742`). Your CQ or TU, or another call, clears it.
    answered_partial: bool,
    /// Its timeouts in a row with nothing from you, while you work it.
    silent: u32,
    serial: SerialModel,
    /// The serial its last exchange sent, and the run time it was fixed
    /// at: what a dupe's next serial follows from.
    serial_sent: Option<(u32, u64)>,
    /// Its exchange as it sends it, fixed the first time it is needed.
    exchange: Option<Vec<(Field, String)>>,
    /// How many times it has sent its exchange.
    exchanges_sent: u32,
    /// Whether your TU works as a CQ for it, as in a contest; in an
    /// everyday QSO it does not.
    tu_calls: bool,
    /// What it sends after the TU that closes its QSO, if anything.
    signoff: Option<&'static str>,
    /// Your TU closed its QSO, and it signs off next.
    signs_off: bool,
    /// It calls in a pile-up, where a caller now and then misses your
    /// exchange.
    pileup: bool,
    /// It sends its call twice when it calls.
    twice: bool,
    /// Where on the band's fading it is at the start of the run, in
    /// seconds: its own path, faded apart from every other caller's.
    pub(super) fade_from_sec: f64,
    rng: SplitMix64,
}

impl Caller {
    /// A station calling with `call` in a run of `style`, where `placed`
    /// puts it, its voice and habits drawn from `rng`, its speed from
    /// `settings` (the run's, shifted by − / +).
    pub(super) fn arrive(
        call: GeneratedCall,
        settings: &TrainingSettings,
        rules: &ContestRules,
        run_minutes: u32,
        style: RunStyle,
        placed: Placement,
        mut rng: SplitMix64,
    ) -> Self {
        let mut voice = resolve_station(settings, &mut rng);
        voice.tone_hz = placed.tone_hz;
        voice.volume = placed.volume;
        let skill = rng.usize_in(1, 3) as u32;
        let patience = draw_patience(&mut rng);
        // A contest whose callers send no serial never reads this one.
        let fit = rules.serials.unwrap_or(super::WPX_SERIALS);
        let serial = SerialModel::draw(fit, run_minutes, &mut rng);
        let pileup = style == RunStyle::Pileup;
        let twice = pileup && rng.f64() < CALLS_TWICE;
        let fade_from_sec = rng.pick_in_range(0.0, FADE_OFFSET_MAX_SEC);
        Self {
            call,
            voice,
            state: CallerState::NeedQso,
            deadline: None,
            skill,
            patience,
            other_qso: false,
            answered_partial: false,
            silent: 0,
            serial,
            serial_sent: None,
            exchange: None,
            exchanges_sent: 0,
            tu_calls: rules.tu_calls,
            signoff: rules.signoff,
            signs_off: false,
            pileup,
            twice,
            fade_from_sec,
            rng,
        }
    }

    /// A station worked before calls again at `run_ms`, a dupe, at
    /// `tone_hz`: the same operator, with fresh patience and a QSO to start.
    /// Its exchange is the one it sent before, but for its serial, which is
    /// the next in its log, and later the longer since (§11.6 of the
    /// design).
    pub(super) fn call_again(&mut self, tone_hz: f64, run_ms: u64) {
        self.voice.tone_hz = tone_hz;
        self.state = CallerState::NeedQso;
        self.deadline = None;
        self.patience = draw_patience(&mut self.rng);
        self.other_qso = false;
        self.answered_partial = false;
        self.silent = 0;
        self.exchanges_sent = 0;
        self.signs_off = false;
        let Some((before, at)) = self.serial_sent else {
            return;
        };
        let serial = self
            .serial
            .after(before, run_ms.saturating_sub(at), &mut self.rng);
        self.serial_sent = Some((serial, run_ms));
        let fields = self.exchange.iter_mut().flatten();
        for (_, token) in fields.filter(|(field, _)| *field == Field::Nr) {
            *token = send_serial(serial, CallerHabits::Easy, &mut self.rng);
        }
    }

    /// How well `sent` fits its call, to rank it among the stations on the
    /// frequency.
    pub(super) fn likeness(&self, sent: &str) -> Option<Likeness> {
        let call = &self.call.text;
        Likeness::of(heard_as(sent, call), sent, call, self.state.worked())
    }

    /// In a pile-up, whether it heard you start a QSO with someone else and
    /// waits, silent, for it to end, as MorseRunner's `osNeedPrevEnd` never
    /// sends (`DxOper.pas:227-228`): no timeout, and no try spent, until your
    /// CQ, TU, B4 or NIL, its own call, or a `?` with the Call field empty.
    /// A single caller calls again on its timeout, kinder to a learner.
    pub(super) fn waits(&self) -> bool {
        self.pileup && self.state == CallerState::NeedQso && self.other_qso
    }

    /// In a pile-up, it was still sending when you keyed over it, heard
    /// only your message's tail, and finished first: it sends again a
    /// moment after you, as MorseRunner's callers do on hearing garbage
    /// (`DxStn.pas:249-253`). A caller spends a try on it; the station you
    /// work stays, as you are keying. Whether it still sends.
    pub(super) fn keyed_over(&mut self) -> bool {
        match self.state {
            CallerState::NeedQso => self.spend_try(),
            state if state.worked() => self.silent = 0,
            _ => {}
        }
        self.state.active()
    }

    /// In a pile-up, whether it misses the exchange it is waiting for, and
    /// asks for it again.
    fn misses_exchange(&mut self) -> bool {
        self.pileup && self.rng.f64() < MISSES_EXCHANGE
    }

    /// Hear one of your messages, whole and in order, and settle what it
    /// does next. `best` holds, for each part of it, how well the call in
    /// it fits the station it fits best among those on the frequency: a
    /// call that fits another better is someone else's, so only the best
    /// match answers a partial. Nothing there, nobody else competes.
    pub(super) fn hear(&mut self, message: &[MessagePart], best: &[Option<Likeness>]) -> Reaction {
        use CallerState::{Done, Gone, NeedCall, NeedCallNr, NeedEnd, NeedNr, NeedQso};
        if self.state.worked() {
            // It stays while you keep keying.
            self.silent = 0;
        }
        let mut reaction = Reaction::default();
        // Whether it replies, where a part decides; otherwise its state does.
        let mut replies = None;
        // Whether the last call in the message was its own: an exchange or
        // a `?` after someone else's call is not for it.
        let mut addressed = None;
        for (at, part) in message.iter().enumerate() {
            let part = match part {
                // A call with no character in it, `?` or `??`, asks as the
                // `?` of F7 does: nobody's call.
                MessagePart::HisCall(sent)
                    if !sent.is_empty() && sent.chars().all(|c| c == '?') =>
                {
                    &MessagePart::Again
                }
                part => part,
            };
            match part {
                MessagePart::Cq => {
                    self.other_qso = false;
                    self.answered_partial = false;
                    match self.state {
                        NeedQso => replies = Some(true),
                        // Its exchange is out: as far as it knows the QSO is
                        // complete. Before that, a CQ fails it.
                        NeedEnd => self.state = Done,
                        NeedNr | NeedCall | NeedCallNr => self.state = Gone,
                        Done | Gone => {}
                    }
                }
                MessagePart::Tu => {
                    self.other_qso = false;
                    self.answered_partial = false;
                    match self.state {
                        // It calls again with fresh patience, as MorseRunner's
                        // TU puts a caller back to calling (`DxOper.pas:776-777`):
                        // passed over QSO after QSO, it stays while you run.
                        NeedQso if self.tu_calls => {
                            self.patience = draw_patience(&mut self.rng);
                            replies = Some(true);
                        }
                        state if state.worked() => {
                            self.state = Done;
                            self.signs_off = self.signoff.is_some();
                        }
                        _ => {}
                    }
                }
                MessagePart::HisCall(sent) if self.state.active() => {
                    // A piece of its call that is no call itself asks for
                    // the rest, as a partial does: nothing to put right.
                    let heard = heard_as(sent, &self.call.text);
                    // A call that fits another station better is that
                    // station's.
                    let own = Likeness::of(heard, sent, &self.call.text, self.state.worked());
                    let beaten = best
                        .get(at)
                        .copied()
                        .flatten()
                        .is_some_and(|best| own.is_none_or(|own| own < best));
                    let found = if beaten { CallMatch::No } else { heard };
                    addressed = Some(found != CallMatch::No);
                    reaction.matched |= found != CallMatch::No;
                    reaction.called |= matches!(found, CallMatch::Exact | CallMatch::Close);
                    self.other_qso = found == CallMatch::No;
                    if found == CallMatch::No {
                        self.answered_partial = false;
                    }
                    match (found, self.state) {
                        // A partial while you work it asks for its call: it
                        // puts the call right, as MorseRunner's `mcAlmost`
                        // (`DxOper.pas:734-742`).
                        (CallMatch::Partial { .. }, NeedNr) => {
                            self.state = NeedCallNr;
                            replies = Some(true);
                            reaction.asked = true;
                        }
                        (CallMatch::Partial { .. }, NeedEnd) => {
                            self.state = NeedCall;
                            replies = Some(true);
                            reaction.asked = true;
                        }
                        // A partial it fits best answers with its call: the
                        // single caller any it fits, as the only one there.
                        (CallMatch::Partial { .. }, state) => {
                            replies = Some(true);
                            reaction.asked = true;
                            self.answered_partial |= state == NeedQso;
                        }
                        (CallMatch::Exact, NeedQso | NeedCallNr) => self.state = NeedNr,
                        (CallMatch::Exact, NeedCall) => self.state = NeedEnd,
                        (CallMatch::Close, NeedQso | NeedNr) => self.state = NeedCallNr,
                        (CallMatch::Close, NeedEnd) => self.state = NeedCall,
                        // In a pile-up, someone else's call ends the QSO for
                        // the station you worked, as MorseRunner's
                        // (`DxOper.pas:744-748`): one that has sent its
                        // exchange takes the QSO as complete; one still
                        // waiting goes back to calling, and waits for that
                        // other QSO to end.
                        (CallMatch::No, NeedEnd) if self.pileup => {
                            self.state = Done;
                            replies = Some(false);
                        }
                        (CallMatch::No, NeedNr | NeedCall | NeedCallNr) if self.pileup => {
                            self.state = NeedQso;
                            replies = Some(false);
                        }
                        // Someone else's call: it waits, and calls again
                        // when its timeout runs out, or in a pile-up when
                        // that QSO ends.
                        (CallMatch::No, _) => replies = Some(false),
                        _ => {}
                    }
                }
                MessagePart::Exchange if addressed != Some(false) => {
                    let waits = match self.state {
                        NeedNr | NeedCallNr => true,
                        NeedQso => addressed == Some(true) || self.answered_partial,
                        _ => false,
                    };
                    // In a pile-up it now and then misses your exchange, and
                    // asks for it again; one that answered your partial puts
                    // its call right meanwhile, as it would for a slip.
                    if waits && self.misses_exchange() {
                        if self.state == NeedQso {
                            self.state = NeedCallNr;
                        }
                        reaction.missed_exchange = true;
                        replies = Some(true);
                        continue;
                    }
                    reaction.took_exchange |= self.state.worked();
                    match self.state {
                        NeedNr => self.state = NeedEnd,
                        NeedCallNr => self.state = NeedCall,
                        // Sent after a partial it answered, your exchange is
                        // for it: it has your exchange, and puts its call
                        // right, as MorseRunner's `osNeedCallNr` hearing
                        // your number (`DxOper.pas:768-776`).
                        NeedQso if addressed == Some(true) || self.answered_partial => {
                            self.state = NeedCall;
                            reaction.took_exchange = true;
                            replies = Some(true);
                        }
                        // Your exchange went to someone: it falls silent.
                        NeedQso => {
                            replies = Some(false);
                            self.other_qso = true;
                        }
                        _ => {}
                    }
                }
                MessagePart::Again | MessagePart::NrQuery
                    if self.state.active()
                        && addressed != Some(false)
                        && !(self.state == NeedQso && self.other_qso) =>
                {
                    replies = Some(true);
                    reaction.asked = true;
                }
                // Your call alone: a caller still calling, and not waiting
                // on a QSO it heard you start, calls you again, as after a
                // CQ.
                MessagePart::MyCall if self.state == NeedQso && !self.other_qso => {
                    replies = Some(true);
                }
                // The QSO you were in is over: a caller still calling, the
                // rest of the pile-up among them, calls again, as
                // MorseRunner's B4 and NIL put it back to calling
                // (`DxOper.pas:716-724, 751-753`). B4 gives it fresh
                // patience, then each costs the try any answer does.
                MessagePart::B4 | MessagePart::Nil if self.state == NeedQso => {
                    self.other_qso = false;
                    self.answered_partial = false;
                    if *part == MessagePart::B4 {
                        self.patience = draw_patience(&mut self.rng);
                    }
                    replies = Some(true);
                }
                // The station you work leaves: not one a B4 after someone
                // else's call was not for, nor one that heard you work
                // someone else. One you sent a slip of its call takes a B4
                // for that slip's, and puts its call right, as MorseRunner's
                // does (`DxOper.pas:755`).
                MessagePart::B4 if matches!(self.state, NeedCall | NeedCallNr) => {}
                MessagePart::B4 | MessagePart::Nil
                    if self.state.worked() && addressed != Some(false) && !self.other_qso =>
                {
                    self.state = Gone;
                }
                _ => {}
            }
        }
        // A caller still calling spends a try on each message it answers,
        // as on each timeout (`DxOper.pas:709, 825`): CQ after CQ moves the
        // frequency on to someone else.
        if self.state == NeedQso && replies == Some(true) {
            self.spend_try();
        }
        reaction.replies = match self.state {
            NeedQso => replies.unwrap_or(false),
            // The station you work answers whatever you send it with what it
            // still owes, unless you sent someone else's call.
            state if state.worked() => replies.unwrap_or(true),
            // The TU that closed its QSO, answered where it signs off.
            Done => self.signs_off,
            _ => false,
        };
        reaction.asked &= reaction.replies;
        reaction
    }

    /// A `?` is coming with the Call field empty: whoever you worked, it is
    /// for anyone still calling, as MorseRunner's `osNeedPrevEnd` comes back
    /// when the Call field is empty (`DxOper.pas:793`).
    pub(super) fn forget_other_qso(&mut self) {
        self.other_qso = false;
    }

    /// One try spent while it calls; the last sends it away.
    fn spend_try(&mut self) {
        self.patience = self.patience.saturating_sub(1);
        if self.patience == 0 {
            self.state = CallerState::Gone;
        }
    }

    /// Its reply timeout ran out with nothing from you. Whether it sends
    /// again: a caller tries while its patience lasts, and the station you
    /// work leaves after [`SILENT_TIMEOUTS`] in a row.
    pub(super) fn time_out(&mut self) -> bool {
        self.deadline = None;
        self.wait_over();
        self.state.active()
    }

    /// Whether it sends again when its reply timeout runs out, with
    /// `pending` more of its timeouts already sending and not yet heard:
    /// one that would spend its last try, or sit through its last silence,
    /// leaves instead.
    pub(super) fn calls_on_time_out(&self, pending: u32) -> bool {
        match self.state {
            CallerState::NeedQso => self.patience > pending + 1,
            state if state.worked() => self.silent + pending + 1 < SILENT_TIMEOUTS,
            _ => false,
        }
    }

    /// What a timeout costs it: a caller spends a try, and calls again, so
    /// a `?` now is for it; the station you work sits through one more
    /// silence, and leaves after [`SILENT_TIMEOUTS`] in a row.
    pub(super) fn wait_over(&mut self) {
        match self.state {
            CallerState::NeedQso => {
                self.spend_try();
                self.other_qso = false;
            }
            state if state.worked() => {
                self.silent += 1;
                if self.silent >= SILENT_TIMEOUTS {
                    self.state = CallerState::Gone;
                }
            }
            _ => {}
        }
    }

    /// What it sends now to you, `my_call`, as its state has it, at
    /// `run_ms` into the run; nothing once it is done or gone, but its sign
    /// off after the TU that closed its QSO, where the contest has one.
    pub(super) fn reply(
        &mut self,
        rules: &ContestRules,
        side: Side,
        my_call: &str,
        run_ms: u64,
    ) -> Option<(String, Says)> {
        let call = self.call.text.clone();
        let said = match self.state {
            CallerState::NeedQso => (rules.answer_text(my_call, &call, self.twice), Says::Call),
            CallerState::NeedNr => {
                let query = if self.rng.f64() < FIRST_QUERY {
                    rules.their_query[0]
                } else {
                    rules.their_query[1]
                };
                (query.to_string(), Says::Query)
            }
            CallerState::NeedCall | CallerState::NeedCallNr => {
                let correction = if self.rng.f64() < 0.5 {
                    format!("DE {call}")
                } else {
                    format!("{call} {call}")
                };
                (correction, Says::Correction)
            }
            CallerState::NeedEnd => {
                let body = self.exchange_text(rules, side, my_call, run_ms);
                // The first time with a word in front of it, as operators
                // send it; a repeat is the exchange alone.
                let text = if self.exchanges_sent > 0 {
                    body
                } else if self.rng.f64() < FIRST_LEAD {
                    format!("{} {body}", rules.their_lead[0])
                } else {
                    format!("{} {body}", rules.their_lead[1])
                };
                self.exchanges_sent += 1;
                (text, Says::Exchange)
            }
            CallerState::Done if self.signs_off => {
                self.signs_off = false;
                (self.signoff?.to_string(), Says::Signoff)
            }
            CallerState::Done | CallerState::Gone => return None,
        };
        Some(said)
    }

    /// Its exchange, each copied field with the token it sends for it,
    /// fixed the first time it is asked for: a station sends one serial in
    /// a QSO however often it repeats it.
    pub(super) fn exchange(
        &mut self,
        rules: &ContestRules,
        side: Side,
        run_ms: u64,
    ) -> &[(Field, String)] {
        if self.exchange.is_none() {
            // Plain digits until the Caller habits setting comes (phase 4).
            let habits = CallerHabits::Easy;
            // A field this build has no values for is not sent; only those
            // of the contests that have shipped are copied.
            let fields = rules
                .copied(side)
                .iter()
                .filter_map(|field| {
                    let token = match field {
                        Field::Nr => {
                            let serial = self.serial.serial_at(run_ms);
                            self.serial_sent = Some((serial, run_ms));
                            send_serial(serial, habits, &mut self.rng)
                        }
                        Field::Rst => {
                            send_report(draw_report(&mut self.rng), habits, &mut self.rng)
                        }
                        // The operator's own, whatever the call: a call comes
                        // back with another name.
                        Field::Name => draw_name(self.call.country, &mut self.rng).to_string(),
                        _ => return None,
                    };
                    Some((*field, token))
                })
                .collect();
            self.exchange = Some(fields);
        }
        self.exchange.as_deref().unwrap_or_default()
    }

    /// Its exchange as it goes out to you, `my_call`: as the contest sends
    /// one, each field its token.
    fn exchange_text(
        &mut self,
        rules: &ContestRules,
        side: Side,
        my_call: &str,
        run_ms: u64,
    ) -> String {
        let fields = self.exchange(rules, side, run_ms).to_vec();
        rules.their_exchange_text(my_call, &self.call.text, &fields)
    }

    /// How long it waits for you before it tries again, in milliseconds,
    /// drawn afresh for each wait.
    pub(super) fn draw_timeout_ms(&mut self) -> u64 {
        let mean = TIMEOUT_BASE_SEC - f64::from(self.skill);
        let spread = mean * TIMEOUT_SPREAD;
        // Half the spread is one standard deviation; the tails are cut at
        // the whole of it.
        let seconds = (mean + self.rng.normal() * spread / 2.0).clamp(mean - spread, mean + spread);
        (seconds * 1000.0).round() as u64
    }

    /// How long after your message it starts, in seconds.
    pub(super) fn draw_delay_sec(&mut self) -> f64 {
        self.rng.pick_in_range(REPLY_DELAY_SEC.0, REPLY_DELAY_SEC.1)
    }
}

/// How many tries a caller makes, from `rng`.
fn draw_patience(rng: &mut impl Rng) -> u32 {
    // MorseRunner's `RndRayleigh(m)` is `m·√(−ln u₁ − ln u₂)`
    // (`RndFunc.pas:55-58`), not a Rayleigh draw of mean `m`: its mean is
    // near 4, so few callers give up after the fewest tries. Delphi's
    // `Round` takes a half to the even side.
    let mut uniform = || (1.0 - rng.f64()).clamp(f64::EPSILON, 1.0);
    let draw = PATIENCE_RAYLEIGH_SCALE * (-uniform().ln() - uniform().ln()).sqrt();
    (PATIENCE_BASE + draw.round_ties_even() as u32).clamp(PATIENCE_MIN, PATIENCE_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callsign::{CallSpec, generate_call};
    use crate::contest::ContestKind;
    use MessagePart::{Again, Cq, Exchange, HisCall, Tu};

    fn rules() -> &'static ContestRules {
        ContestKind::Wpx.rules().expect("WPX has shipped")
    }

    fn caller(seed: u64) -> Caller {
        let mut rng = SplitMix64::new(seed);
        let mut call = generate_call(3, CallSpec::default(), &mut rng);
        call.text = "K5XYZ".to_string();
        let settings = TrainingSettings::default().clamp();
        Caller::arrive(
            call,
            &settings,
            rules(),
            10,
            RunStyle::Single,
            centre(),
            rng.fork(1),
        )
    }

    /// A place at the centre, at full strength.
    fn centre() -> Placement {
        Placement {
            tone_hz: 500.0,
            volume: 1.0,
        }
    }

    fn call(text: &str) -> MessagePart {
        HisCall(text.to_string())
    }

    #[test]
    fn calls_match_exactly_closely_or_not_at_all() {
        assert_eq!(match_call("K5XYZ", "K5XYZ"), CallMatch::Exact);
        // Up to (5 − 1) / 2 = 2 edits of a five-character call.
        assert_eq!(match_call("K5XYW", "K5XYZ"), CallMatch::Close);
        assert_eq!(match_call("K5ZYX", "K5XYZ"), CallMatch::Close);
        assert_eq!(match_call("W5ZYX", "K5XYZ"), CallMatch::No);
        // A piece of the call is close, however short.
        assert_eq!(match_call("XYZ", "K5XYZ"), CallMatch::Close);
        assert_eq!(match_call("5X", "K5XYZ"), CallMatch::Close);
        assert_eq!(match_call("", "K5XYZ"), CallMatch::No);
        assert_eq!(match_call("DL1ABC", "K5XYZ"), CallMatch::No);
        // A longer call allows more slips.
        assert_eq!(match_call("DL1ABD", "DL/K5XYZ"), CallMatch::No);
        assert_eq!(match_call("DL/K5XYW", "DL/K5XYZ"), CallMatch::Close);
    }

    /// `?` stands for one character inside a partial and for any tail at its
    /// end, matched anywhere in the call.
    #[test]
    fn partials_match_anywhere() {
        let fits = |sent: &str| match match_call(sent, "K5XYZ") {
            CallMatch::Partial { fits } => Some(fits),
            _ => None,
        };
        assert_eq!(fits("K5?"), Some(2));
        assert_eq!(fits("XY?"), Some(2), "anywhere in the call");
        assert_eq!(fits("K5X?Z"), Some(4), "one character inside");
        assert_eq!(fits("K5?YZ"), Some(4));
        assert_eq!(fits("?5XYZ"), Some(4));
        assert_eq!(fits("K5XYZ?"), Some(5), "an empty tail is a tail");
        assert_eq!(fits("K5??Z"), Some(3));
        assert_eq!(fits("?"), Some(0));
        assert_eq!(fits("K6?"), None);
        assert_eq!(fits("K5?Z"), None, "inside, ? is one character only");
        assert_eq!(fits("K5XYZA?"), None);
        // More characters that fit are a surer match.
        assert!(fits("K5X?Z") > fits("K5?"));
    }

    /// Four to fourteen tries, a mean near seven.
    #[test]
    fn patience_is_four_to_fourteen_tries_with_a_mean_near_seven() {
        let mut rng = SplitMix64::new(42);
        let draws: Vec<u32> = (0..10_000).map(|_| draw_patience(&mut rng)).collect();
        assert!(draws.iter().all(|p| (4..=14).contains(p)));
        let mean = draws.iter().sum::<u32>() as f64 / draws.len() as f64;
        assert!((mean - 7.0).abs() < 0.3, "mean {mean:.2}");
        assert!(
            draws.iter().any(|p| *p >= 10),
            "a patient caller now and then"
        );
    }

    /// Patience is drawn as MorseRunner draws it, `3 + Round(RndRayleigh(3))`
    /// with `RndRayleigh(m) = m·√(−ln u₁ − ln u₂)` (`RndFunc.pas:55-58`):
    /// four tries only 2.6% of the time, as `DxOper.pas:493` says, and five
    /// or fewer about 15%.
    #[test]
    fn few_callers_give_up_after_four_tries() {
        let mut rng = SplitMix64::new(42);
        let draws: Vec<u32> = (0..20_000).map(|_| draw_patience(&mut rng)).collect();
        let share =
            |most: u32| draws.iter().filter(|p| **p <= most).count() as f64 / draws.len() as f64;
        assert!(share(4) < 0.05, "four tries {:.1}%", share(4) * 100.0);
        assert!(share(5) < 0.25, "five or fewer {:.1}%", share(5) * 100.0);
    }

    /// `(6 − skill)` seconds, never more than half of that either way.
    #[test]
    fn a_reply_timeout_stays_within_half_of_its_mean() {
        for seed in 0..200 {
            let mut caller = caller(seed);
            let mean = (TIMEOUT_BASE_SEC - f64::from(caller.skill)) * 1000.0;
            for _ in 0..50 {
                let timeout = caller.draw_timeout_ms() as f64;
                assert!(timeout >= mean * 0.5 - 1.0 && timeout <= mean * 1.5 + 1.0);
            }
            let delay = caller.draw_delay_sec();
            assert!((0.1..=0.6).contains(&delay));
            assert!((1..=3).contains(&caller.skill));
        }
    }

    /// The basic QSO from the caller's side.
    #[test]
    fn a_caller_answers_its_call_and_your_exchange_then_your_tu() {
        let mut caller = caller(1);
        assert_eq!(caller.state, CallerState::NeedQso);
        let reaction = caller.hear(&[Cq], &[]);
        assert!(reaction.replies && !reaction.asked && !reaction.matched);
        assert_eq!(
            caller.reply(rules(), Side::Dx, "DL1ABC", 0).map(|r| r.1),
            Some(Says::Call)
        );
        let reaction = caller.hear(&[call("K5XYZ"), Exchange], &[]);
        assert_eq!(caller.state, CallerState::NeedEnd);
        assert!(reaction.replies && reaction.matched);
        let (text, says) = caller
            .reply(rules(), Side::Dx, "DL1ABC", 60_000)
            .expect("a reply");
        assert_eq!(says, Says::Exchange);
        let serial = caller.exchange(rules(), Side::Dx, 0)[0].1.clone();
        assert!(text == format!("TU 5NN {serial}") || text == format!("R 5NN {serial}"));
        // Asked again, it sends the same serial, alone.
        let reaction = caller.hear(&[Again], &[]);
        assert!(reaction.replies && reaction.asked);
        let again = caller
            .reply(rules(), Side::Dx, "DL1ABC", 120_000)
            .expect("a reply");
        assert_eq!(again.0, format!("5NN {serial}"));
        assert!(!caller.hear(&[Tu], &[]).replies);
        assert_eq!(caller.state, CallerState::Done);
    }

    /// In an everyday QSO a caller calls you by your call and its own
    /// twice, sends a report and a name of its own, signs off once after
    /// your TU, and takes no TU for a CQ.
    #[test]
    fn a_simple_qso_caller_sends_an_everyday_qsos_lines() {
        let rules = ContestKind::SimpleQso
            .rules()
            .expect("Simple QSO has shipped");
        let mut rng = SplitMix64::new(4);
        let mut generated = generate_call(3, CallSpec::default(), &mut rng);
        generated.text = "K5XYZ".to_string();
        generated.country = "K";
        let settings = TrainingSettings::default().clamp();
        let mut caller = Caller::arrive(
            generated,
            &settings,
            rules,
            10,
            RunStyle::Single,
            centre(),
            rng.fork(1),
        );
        assert!(!caller.hear(&[Tu], &[]).replies, "a TU is no CQ here");
        assert!(caller.hear(&[Cq], &[]).replies);
        assert_eq!(
            caller.reply(rules, Side::Dx, "DL1ABC", 0),
            Some(("DL1ABC DE K5XYZ K5XYZ K".to_string(), Says::Call))
        );
        caller.hear(&[call("K5XYZ"), Exchange], &[]);
        let (text, says) = caller.reply(rules, Side::Dx, "DL1ABC", 0).expect("a reply");
        assert_eq!(says, Says::Exchange);
        let fields = caller.exchange(rules, Side::Dx, 0).to_vec();
        let (rst, name) = (&fields[0], &fields[1]);
        assert_eq!((rst.0, name.0), (Field::Rst, Field::Name));
        assert!(Field::Rst.canonical(&rst.1).is_some(), "{}", rst.1);
        assert_eq!(
            text,
            format!("R TNX UR {} {} NAME {} {} BK", rst.1, rst.1, name.1, name.1)
        );
        // Sent again, the exchange goes without its first words.
        caller.hear(&[Again], &[]);
        let again = caller.reply(rules, Side::Dx, "DL1ABC", 0).expect("a reply");
        assert_eq!(
            again.0,
            format!("UR {} {} NAME {} {} BK", rst.1, rst.1, name.1, name.1)
        );
        let reaction = caller.hear(&[Tu], &[]);
        assert!(reaction.replies && !reaction.asked);
        assert_eq!(caller.state, CallerState::Done);
        assert_eq!(
            caller.reply(rules, Side::Dx, "DL1ABC", 0),
            Some(("73 EE".to_string(), Says::Signoff))
        );
        assert_eq!(caller.reply(rules, Side::Dx, "DL1ABC", 0), None, "once");
        assert!(!caller.hear(&[Tu], &[]).replies);
        // Its call alone, it asks for your exchange, and never for a serial
        // the QSO has none of.
        for seed in 0..40 {
            let mut rng = SplitMix64::new(seed);
            let mut generated = generate_call(3, CallSpec::default(), &mut rng);
            generated.text = "K5XYZ".to_string();
            let mut caller = Caller::arrive(
                generated,
                &settings,
                rules,
                10,
                RunStyle::Single,
                centre(),
                rng.fork(1),
            );
            caller.hear(&[Cq], &[]);
            assert!(caller.hear(&[call("K5XYZ")], &[]).replies);
            assert_eq!(caller.state, CallerState::NeedNr);
            assert_eq!(
                caller.reply(rules, Side::Dx, "DL1ABC", 0),
                Some(("AGN".to_string(), Says::Query)),
                "seed {seed}"
            );
        }
    }

    /// Nothing more comes from a station that is done or gone.
    #[test]
    fn nothing_is_sent_after_done_or_gone() {
        for state in [CallerState::Done, CallerState::Gone] {
            let mut caller = caller(2);
            caller.state = state;
            for message in [
                vec![Cq],
                vec![call("K5XYZ"), Exchange],
                vec![Again],
                vec![call("K5?")],
                vec![Tu],
            ] {
                assert!(!caller.hear(&message, &[]).replies, "{state:?} {message:?}");
                assert_eq!(caller.state, state);
            }
            assert!(caller.reply(rules(), Side::Dx, "DL1ABC", 0).is_none());
            assert!(!caller.time_out());
        }
    }

    /// A CQ ends the QSO: done if the station has sent its exchange, failed
    /// before that.
    #[test]
    fn a_cq_fails_the_worked_qso_until_its_exchange_is_out() {
        let mut waiting = caller(3);
        waiting.hear(&[call("K5XYZ")], &[]);
        assert_eq!(waiting.state, CallerState::NeedNr);
        waiting.hear(&[Cq], &[]);
        assert_eq!(waiting.state, CallerState::Gone);
        let mut corrected = caller(3);
        corrected.hear(&[call("K5XYW"), Exchange], &[]);
        assert_eq!(corrected.state, CallerState::NeedCall);
        corrected.hear(&[Cq], &[]);
        assert_eq!(corrected.state, CallerState::Gone);
        let mut complete = caller(3);
        complete.hear(&[call("K5XYZ"), Exchange], &[]);
        complete.hear(&[Cq], &[]);
        assert_eq!(complete.state, CallerState::Done);
    }

    /// A call close to its own is corrected, and the right one goes on.
    #[test]
    fn a_close_call_is_corrected() {
        let mut caller = caller(4);
        let reaction = caller.hear(&[call("K5XYW"), Exchange], &[]);
        assert!(reaction.replies && reaction.matched && !reaction.asked);
        let (text, says) = caller
            .reply(rules(), Side::Dx, "DL1ABC", 0)
            .expect("a reply");
        assert_eq!(says, Says::Correction);
        assert!(text == "DE K5XYZ" || text == "K5XYZ K5XYZ", "{text}");
        caller.hear(&[call("K5XYZ"), Tu], &[]);
        assert_eq!(caller.state, CallerState::Done);
        // Close before your exchange, it waits for both.
        let mut early = self::caller(4);
        early.hear(&[call("K5XY")], &[]);
        assert_eq!(early.state, CallerState::NeedCallNr);
        early.hear(&[call("K5XYZ"), Exchange], &[]);
        assert_eq!(early.state, CallerState::NeedEnd);
    }

    /// Someone else's call: it falls silent and calls again on its timeout.
    #[test]
    fn another_call_silences_a_caller_until_its_timeout() {
        let mut caller = caller(5);
        let reaction = caller.hear(&[call("DL1ABC"), Exchange], &[]);
        assert_eq!(reaction, Reaction::default());
        assert_eq!(caller.state, CallerState::NeedQso);
        assert!(caller.time_out(), "it calls again");
        // Your exchange alone, to nobody it knows of: silent too.
        assert!(!caller.hear(&[Exchange], &[]).replies);
    }

    /// A partial or `?` brings its call again, and you asked for that.
    #[test]
    fn a_partial_or_a_question_brings_the_call_again() {
        let mut caller = caller(6);
        let reaction = caller.hear(&[call("K5?")], &[]);
        assert!(reaction.replies && reaction.asked && reaction.matched);
        assert_eq!(caller.state, CallerState::NeedQso);
        let reaction = caller.hear(&[Again], &[]);
        assert!(reaction.replies && reaction.asked && !reaction.matched);
    }

    /// A partial while you work it asks for its call: it puts the call
    /// right, as MorseRunner's callers answer `mcAlmost`, whether it waits
    /// for your exchange or for your TU.
    #[test]
    fn a_partial_while_worked_is_answered_with_the_call() {
        for (message, before, after) in [
            (
                vec![call("K5XYZ")],
                CallerState::NeedNr,
                CallerState::NeedCallNr,
            ),
            (
                vec![call("K5XYZ"), Exchange],
                CallerState::NeedEnd,
                CallerState::NeedCall,
            ),
        ] {
            let mut caller = caller(9);
            caller.hear(&message, &[]);
            assert_eq!(caller.state, before);
            let reaction = caller.hear(&[call("K5X?")], &[]);
            assert!(reaction.replies && reaction.asked && reaction.matched);
            assert_eq!(caller.state, after);
            let (text, says) = caller
                .reply(rules(), Side::Dx, "DL1ABC", 0)
                .expect("a reply");
            assert_eq!(says, Says::Correction);
            assert!(text.contains("K5XYZ"), "{text}");
        }
    }

    /// A `?` after someone else's call, or once it heard your exchange go
    /// to someone else, is not for a caller still calling; it calls again
    /// on its timeout, and a `?` then is for it.
    #[test]
    fn a_question_after_someone_elses_call_is_not_for_it() {
        let mut caller = caller(1);
        assert!(!caller.hear(&[call("W1AW"), Again], &[]).replies);
        assert!(
            !caller.hear(&[Again], &[]).replies,
            "it heard you work W1AW"
        );
        let mut other = self::caller(2);
        other.hear(&[call("W1AW"), Exchange], &[]);
        assert!(!other.hear(&[Again], &[]).replies);
        assert!(other.time_out(), "it calls again");
        assert!(other.hear(&[Again], &[]).replies);
        // After your CQ, a `?` is for it again.
        let mut cq = self::caller(3);
        cq.hear(&[call("W1AW")], &[]);
        cq.hear(&[Cq], &[]);
        assert!(cq.hear(&[Again], &[]).replies);
        // The station you work answers a `?`, but not one after another
        // call.
        let mut worked = self::caller(4);
        worked.hear(&[call("K5XYZ"), Exchange], &[]);
        assert!(!worked.hear(&[call("W1AW"), Again], &[]).replies);
        assert!(worked.hear(&[Again], &[]).replies);
    }

    /// Each message a caller still calling answers costs it a try, as each
    /// timeout does: CQ after CQ, or `?` after `?`, sends it away in the
    /// end.
    #[test]
    fn each_message_a_caller_answers_costs_a_try() {
        let mut caller = caller(3);
        let tries = caller.patience;
        assert!(caller.hear(&[Cq], &[]).replies);
        assert!(caller.hear(&[Again], &[]).replies);
        assert_eq!(caller.patience, tries - 2);
        let mut answered = 2;
        while caller.hear(&[Cq], &[]).replies {
            answered += 1;
        }
        assert_eq!(answered, tries - 1);
        assert_eq!(caller.state, CallerState::Gone);
        // A message it does not answer costs nothing.
        let mut silent = self::caller(3);
        silent.hear(&[call("W1AW"), Exchange], &[]);
        assert_eq!(silent.patience, tries);
    }

    /// Each timeout costs a try; the last one sends it away.
    #[test]
    fn a_caller_gives_up_when_its_patience_runs_out() {
        let mut caller = caller(7);
        let tries = caller.patience;
        for _ in 1..tries {
            assert!(caller.time_out());
        }
        assert!(!caller.time_out());
        assert_eq!(caller.state, CallerState::Gone);
    }

    /// The station you work stays while you keep keying, and leaves after
    /// three silent timeouts in a row.
    #[test]
    fn a_worked_station_leaves_after_three_silent_timeouts() {
        let mut caller = caller(8);
        caller.hear(&[call("K5XYZ"), Exchange], &[]);
        // Keying in between keeps it, however long it goes on.
        for _ in 0..20 {
            assert!(caller.time_out());
            assert!(caller.time_out());
            caller.hear(&[Again], &[]);
        }
        assert!(caller.time_out());
        assert!(caller.time_out());
        assert!(!caller.time_out(), "the third in a row");
        assert_eq!(caller.state, CallerState::Gone);
    }

    /// Your exchange after a partial it answered, in the same message or
    /// the next, is for the caller: it has your exchange and puts its call
    /// right, as MorseRunner's `osNeedCallNr` does. After another call
    /// since, your exchange is someone else's.
    #[test]
    fn an_exchange_after_an_answered_partial_is_for_the_caller() {
        let mut together = caller(1);
        let reaction = together.hear(&[call("K5?"), Exchange], &[]);
        assert!(reaction.replies && reaction.matched && reaction.took_exchange);
        assert_eq!(together.state, CallerState::NeedCall);
        assert!(!together.other_qso);
        let mut after = caller(1);
        after.hear(&[call("K5?")], &[]);
        let reaction = after.hear(&[Exchange], &[]);
        assert!(reaction.replies && reaction.took_exchange);
        assert_eq!(after.state, CallerState::NeedCall);
        assert_eq!(
            after.reply(rules(), Side::Dx, "DL1ABC", 0).map(|r| r.1),
            Some(Says::Correction)
        );
        let mut moved = caller(1);
        moved.hear(&[call("K5?")], &[]);
        moved.hear(&[call("W1AW")], &[]);
        assert!(!moved.hear(&[Exchange], &[]).replies);
        assert_eq!(moved.state, CallerState::NeedQso);
    }

    /// A piece of its call that is no call itself asks for the rest, as a
    /// partial does: answered with its call, a try spent, nothing put
    /// right. A bare `?` in its place is the `?` of F7: nobody's call.
    #[test]
    fn a_piece_of_the_call_asks_and_a_bare_question_is_nobodys_call() {
        let mut piece = caller(2);
        let tries = piece.patience;
        let reaction = piece.hear(&[call("K5")], &[]);
        assert!(reaction.replies && reaction.asked && reaction.matched);
        assert_eq!(piece.state, CallerState::NeedQso);
        assert_eq!(piece.patience, tries - 1);
        assert_eq!(
            piece.reply(rules(), Side::Dx, "DL1ABC", 0).map(|r| r.1),
            Some(Says::Call)
        );
        // A slip that is a whole call is still put right.
        let mut slip = caller(2);
        slip.hear(&[call("K5XYW")], &[]);
        assert_eq!(slip.state, CallerState::NeedCallNr);
        let mut bare = caller(2);
        let reaction = bare.hear(&[call("?")], &[]);
        assert!(reaction.replies && reaction.asked && !reaction.matched);
        let mut worked = caller(2);
        worked.hear(&[call("K5XYZ")], &[]);
        worked.hear(&[call("??")], &[]);
        assert_eq!(worked.state, CallerState::NeedNr, "nothing to put right");
    }

    /// A caller of a run of `style` signing `call`, from `seed`.
    fn caller_as(seed: u64, call: &str, style: RunStyle) -> Caller {
        let mut rng = SplitMix64::new(seed);
        let mut generated = generate_call(3, CallSpec::default(), &mut rng);
        generated.text = call.to_string();
        let settings = TrainingSettings::default().clamp();
        Caller::arrive(
            generated,
            &settings,
            rules(),
            10,
            style,
            centre(),
            rng.fork(1),
        )
    }

    fn pileup_caller(seed: u64, call: &str) -> Caller {
        caller_as(seed, call, RunStyle::Pileup)
    }

    /// Only the best match answers: a partial goes to the callers whose
    /// call it covers the largest share of, a slip to the closest call, and
    /// a call to its own station, however well it fits another. To the
    /// others it is someone else's call.
    #[test]
    fn only_the_best_match_answers() {
        let best = |sent: &str, calls: &[&str]| -> Vec<bool> {
            let mut callers: Vec<Caller> = calls
                .iter()
                .enumerate()
                .map(|(seed, call)| pileup_caller(seed as u64, call))
                .collect();
            let message = [call(sent)];
            let best = callers
                .iter()
                .filter_map(|caller| caller.likeness(sent))
                .reduce(|a, b| if b > a { b } else { a });
            callers
                .iter_mut()
                .map(|caller| caller.hear(&message, &[best]).replies)
                .collect()
        };
        // `K5?` covers two of K5AB's four, and two of K5XYZ's five.
        assert_eq!(best("K5?", &["K5XYZ", "K5AB"]), [false, true]);
        // Covering as much of both, both answer.
        assert_eq!(best("K5?", &["K5XYZ", "K5ABC"]), [true, true]);
        assert_eq!(best("K5XY?", &["K5XYZ", "K5ABC"]), [true, false]);
        // A slip goes to the call it is nearest; its own call beats any
        // slip, and a piece that is no call asks as a partial does.
        // Callers that never miss your exchange, to see who takes it.
        let slip = |calls: &[&str]| {
            let mut callers: Vec<Caller> = calls
                .iter()
                .enumerate()
                .map(|(seed, call)| caller_as(seed as u64, call, RunStyle::Single))
                .collect();
            let sent = "K5XYW";
            let best = callers
                .iter()
                .filter_map(|caller| caller.likeness(sent))
                .reduce(|a, b| if b > a { b } else { a });
            let message = [call(sent), MessagePart::Exchange];
            callers
                .iter_mut()
                .map(|caller| {
                    caller.hear(&message, &[best, None]);
                    caller.state
                })
                .collect::<Vec<_>>()
        };
        // Two edits off K5AAW, one off K5XYZ.
        assert_eq!(
            slip(&["K5XYZ", "K5AAW"]),
            [CallerState::NeedCall, CallerState::NeedQso]
        );
        assert_eq!(
            slip(&["K5XYZ", "K5XYW"]),
            [CallerState::NeedQso, CallerState::NeedEnd]
        );
        // The station you work keeps a partial that fits it, though a
        // caller's call it covers more of; a caller's own call beats a slip
        // of the call you work.
        let worked = |sent: &str, calls: &[&str]| -> Vec<bool> {
            let mut callers: Vec<Caller> = calls
                .iter()
                .enumerate()
                .map(|(seed, call)| pileup_caller(seed as u64, call))
                .collect();
            // Callers that never miss your exchange, to see who takes it.
            callers[0] = caller_as(0, calls[0], RunStyle::Single);
            callers[0].hear(&[call(calls[0]), MessagePart::Exchange], &[]);
            let message = [call(sent)];
            let best = callers
                .iter()
                .filter_map(|caller| caller.likeness(sent))
                .reduce(|a, b| if b > a { b } else { a });
            callers
                .iter_mut()
                .map(|caller| caller.hear(&message, &[best]).replies)
                .collect()
        };
        assert_eq!(worked("K5?", &["K5XYZ", "K5AB"]), [true, false]);
        assert_eq!(worked("K5XY", &["K5XYZ", "K5XY"]), [false, true]);
        // A slip that is no call, its digit dropped, goes to the call it is
        // fewest edits from, as a piece goes to the call it covers most of.
        assert_eq!(best("KXYZ", &["K5XYZ", "K5XYA"]), [true, false]);
        assert_eq!(best("KXYA", &["K5XYZ", "K5XYA"]), [false, true]);
        assert_eq!(best("XYZ", &["K5XYZ", "W1XYZ"]), [true, true]);
        // With nobody to compare, any fit answers, as the single caller
        // does.
        let mut alone = pileup_caller(1, "K5XYZ");
        assert!(alone.hear(&[call("K5?")], &[]).replies);
        assert!(alone.likeness("W1AW").is_none());
    }

    /// In a pile-up a caller now and then misses your exchange and asks for
    /// it again, `NR?` or `AGN`, about one time in ten; a single caller
    /// never misses it.
    #[test]
    fn a_pile_up_caller_now_and_then_misses_your_exchange() {
        let mut missed = 0;
        for seed in 0..2_000 {
            let mut caller = pileup_caller(seed, "K5XYZ");
            caller.hear(&[call("K5XYZ")], &[]);
            let reaction = caller.hear(&[Exchange], &[]);
            assert!(reaction.replies);
            if caller.state == CallerState::NeedNr {
                missed += 1;
                assert!(!reaction.took_exchange, "seed {seed}");
                let (text, says) = caller
                    .reply(rules(), Side::Dx, "DL1ABC", 0)
                    .expect("a reply");
                assert_eq!(says, Says::Query);
                assert!(text == "NR?" || text == "AGN", "{text}");
            } else {
                assert_eq!(caller.state, CallerState::NeedEnd, "seed {seed}");
                assert!(reaction.took_exchange);
            }
        }
        let share = f64::from(missed) / 2_000.0;
        assert!((share - 0.1).abs() < 0.02, "missed {share:.3}");
        for seed in 0..500 {
            let mut caller = caller(seed);
            caller.hear(&[call("K5XYZ")], &[]);
            caller.hear(&[Exchange], &[]);
            assert_eq!(caller.state, CallerState::NeedEnd, "seed {seed}");
        }
    }

    /// One pile-up caller in ten sends its call twice; a single caller never
    /// does.
    #[test]
    fn one_pile_up_caller_in_ten_calls_twice() {
        let mut twice = 0;
        for seed in 0..2_000 {
            let mut caller = pileup_caller(seed, "K5XYZ");
            caller.hear(&[Cq], &[]);
            match caller.reply(rules(), Side::Dx, "DL1ABC", 0) {
                Some((text, Says::Call)) if text == "K5XYZ K5XYZ" => twice += 1,
                Some((text, Says::Call)) => assert_eq!(text, "K5XYZ"),
                other => panic!("seed {seed}: {other:?}"),
            }
        }
        let share = f64::from(twice) / 2_000.0;
        assert!((share - 0.1).abs() < 0.02, "twice {share:.3}");
        for seed in 0..500 {
            let mut caller = caller(seed);
            caller.hear(&[Cq], &[]);
            let reply = caller.reply(rules(), Side::Dx, "DL1ABC", 0);
            assert_eq!(reply, Some(("K5XYZ".to_string(), Says::Call)));
        }
    }

    /// Your call alone brings a caller still calling back, as a CQ does,
    /// unless it heard you start a QSO with someone else; the station you
    /// work sends what it owes.
    #[test]
    fn your_call_alone_brings_the_callers_back() {
        let mut calling = caller(1);
        let tries = calling.patience;
        let reaction = calling.hear(&[MessagePart::MyCall], &[]);
        assert!(reaction.replies && !reaction.matched);
        assert_eq!(calling.patience, tries - 1, "a try spent");
        let mut waiting = caller(2);
        waiting.hear(&[call("W1AW")], &[]);
        assert!(!waiting.hear(&[MessagePart::MyCall], &[]).replies);
        let mut worked = caller(3);
        worked.hear(&[call("K5XYZ"), Exchange], &[]);
        assert!(worked.hear(&[MessagePart::MyCall], &[]).replies);
        assert_eq!(worked.state, CallerState::NeedEnd);
    }

    /// A station worked before calls again with a QSO to start, and sends
    /// the serial after the one it sent you, or later; the rest of its
    /// exchange is as it was: the same operator, the same name.
    #[test]
    fn a_dupe_calls_again_with_its_next_serial() {
        let mut caller = caller(4);
        caller.hear(&[call("K5XYZ"), Exchange], &[]);
        let first = caller.exchange(rules(), Side::Dx, 60_000)[0].1.clone();
        caller.hear(&[Tu], &[]);
        assert_eq!(caller.state, CallerState::Done);
        caller.call_again(612.0, 120_000);
        assert_eq!(caller.state, CallerState::NeedQso);
        assert_eq!(caller.voice.tone_hz, 612.0);
        assert!(caller.hear(&[Cq], &[]).replies);
        caller.hear(&[call("K5XYZ"), Exchange], &[]);
        let again = caller.exchange(rules(), Side::Dx, 180_000)[0].1.clone();
        let (first, again): (u32, u32) = (first.parse().unwrap(), again.parse().unwrap());
        assert!(again > first, "{first} then {again}");
        let simple = ContestKind::SimpleQso
            .rules()
            .expect("Simple QSO has shipped");
        for seed in 0..50 {
            let mut rng = SplitMix64::new(seed);
            let mut generated = generate_call(3, CallSpec::default(), &mut rng);
            generated.text = "K5XYZ".to_string();
            let settings = TrainingSettings::default().clamp();
            let mut caller = Caller::arrive(
                generated,
                &settings,
                simple,
                10,
                RunStyle::Single,
                centre(),
                rng.fork(1),
            );
            let first = caller.exchange(simple, Side::Dx, 0).to_vec();
            caller.call_again(612.0, 60_000);
            assert_eq!(
                caller.exchange(simple, Side::Dx, 60_000),
                first,
                "seed {seed}"
            );
        }
    }

    /// A TU gives a caller still calling fresh patience before it spends
    /// the try any answer does, as MorseRunner's TU puts it back to calling:
    /// passed over QSO after QSO, it stays while you run. A CQ only costs.
    #[test]
    fn a_tu_gives_a_caller_still_calling_fresh_patience() {
        let mut caller = pileup_caller(3, "K5XYZ");
        for _ in 0..50 {
            caller.hear(&[call("W1AW"), Exchange], &[]);
            assert!(caller.hear(&[Tu], &[]).replies);
        }
        assert_eq!(caller.state, CallerState::NeedQso);
        let tries = caller.patience;
        assert!(caller.hear(&[Cq], &[]).replies);
        assert_eq!(caller.patience, tries - 1);
    }

    /// In a pile-up, someone else's call ends the QSO for the station you
    /// worked: back to calling, waiting for that QSO to end, if it still
    /// waited for your exchange; done, if it had sent its own. A single
    /// caller stays as it was.
    #[test]
    fn someone_elses_call_ends_a_pile_up_qso() {
        let mut waiting = pileup_caller(1, "K5XYZ");
        waiting.hear(&[call("K5XYZ")], &[]);
        waiting.hear(&[call("W1AW"), Exchange], &[]);
        assert_eq!(waiting.state, CallerState::NeedQso);
        assert!(waiting.waits(), "till that QSO ends");
        assert!(waiting.hear(&[Tu], &[]).replies, "your TU ends it");
        assert!(!waiting.waits());
        let mut complete = pileup_caller(1, "K5XYZ");
        complete.hear(&[call("K5XYZ"), Exchange], &[]);
        while complete.state != CallerState::NeedEnd {
            complete.hear(&[Exchange], &[]);
        }
        complete.hear(&[call("W1AW")], &[]);
        assert_eq!(complete.state, CallerState::Done);
        let mut single = caller(1);
        single.hear(&[call("K5XYZ")], &[]);
        single.hear(&[call("W1AW")], &[]);
        assert_eq!(single.state, CallerState::NeedNr);
        assert!(!single.waits());
    }

    /// B4 sends away the station it is for, but one you sent a slip of its
    /// call takes the B4 for that slip's, and puts its call right, as
    /// MorseRunner's does; a caller still calling calls again.
    #[test]
    fn b4_with_a_slip_of_the_call_is_put_right() {
        let mut worked = caller(1);
        worked.hear(&[call("K5XYZ"), Exchange], &[]);
        worked.hear(&[MessagePart::B4], &[]);
        assert_eq!(worked.state, CallerState::Gone);
        for (message, state) in [
            (
                vec![call("K5XYW"), MessagePart::B4],
                CallerState::NeedCallNr,
            ),
            (
                vec![call("K5XYW"), Exchange, MessagePart::B4],
                CallerState::NeedCall,
            ),
        ] {
            let mut slip = caller(1);
            let reaction = slip.hear(&message, &[]);
            assert_eq!(slip.state, state, "{message:?}");
            assert!(reaction.replies);
            assert_eq!(
                slip.reply(rules(), Side::Dx, "DL1ABC", 0).map(|r| r.1),
                Some(Says::Correction)
            );
        }
        let mut calling = caller(1);
        assert!(calling.hear(&[call("W1AW"), MessagePart::B4], &[]).replies);
        assert_eq!(calling.state, CallerState::NeedQso);
    }

    /// With the Call field empty, a `?` is for a caller that heard you work
    /// someone else.
    #[test]
    fn a_question_with_the_field_empty_is_for_anyone_calling() {
        let mut caller = caller(3);
        caller.hear(&[call("W1AW")], &[]);
        assert!(!caller.hear(&[Again], &[]).replies);
        caller.forget_other_qso();
        assert!(caller.hear(&[Again], &[]).replies);
    }
}
