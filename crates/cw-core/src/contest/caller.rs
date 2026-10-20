//! The stations that answer your CQ. They behave as MorseRunner's callers
//! do, re-written from what those do (`DxOper.pas`, MorseRunner CE,
//! credited): how long one waits for you, how many times it calls before it
//! gives up, and what each of your messages makes it do.
//!
//! A caller hears each of your messages whole, in order, and ends up in one
//! of MorseRunner's states; what it sends next follows from the state. Which
//! of your calls it takes for its own is [`match_call`].

use std::f64::consts::FRAC_PI_2;

use super::CallerHabits;
use super::esm::MessagePart;
use super::exchange::{Field, send_serial};
use super::rules::{ContestRules, REPORT, Side};
use super::serial::SerialModel;
use crate::alignment::edit_distance;
use crate::callsign::{GeneratedCall, parse_callsign};
use crate::rng::{Rng, SplitMix64};
use crate::settings::{SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ, TrainingSettings};
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
    /// It took your exchange as sent to it, while you work it.
    pub(super) took_exchange: bool,
}

/// How many tries a caller makes before it gives up: `3 + Rayleigh(3)`,
/// four to fourteen with a mean near six (`DxOper.pas:496-498`).
const PATIENCE_BASE: u32 = 3;
const PATIENCE_RAYLEIGH_MEAN: f64 = 3.0;
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

/// How far either side of the receiver's centre a single caller sits
/// (`DxStn.pas:102-105`).
const SINGLE_PITCH_SPREAD_HZ: f64 = 50.0;

/// How often the first exchange a caller sends starts with `TU` rather
/// than `R`, as `TU 5NN 214` and `R 5NN 214`.
const TU_FIRST: f64 = 0.9;

/// How often a caller waiting for your exchange asks with `NR?` rather than
/// `AGN` (`DxOper.pas`, `GetReply`).
const ASKS_NR: f64 = 0.3;

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
    /// 793`). Your CQ or TU, its own call, its next call, or a `?` with the
    /// Call field empty clears it.
    other_qso: bool,
    /// It answered your partial while it called, so your exchange now is
    /// for it, as MorseRunner's near match makes it `osNeedCallNr`
    /// (`DxOper.pas:734-742`). Your CQ or TU, or another call, clears it.
    answered_partial: bool,
    /// Its timeouts in a row with nothing from you, while you work it.
    silent: u32,
    serial: SerialModel,
    /// Its exchange as it sends it, fixed the first time it is needed.
    exchange: Option<Vec<(Field, String)>>,
    /// How many times it has sent its exchange.
    exchanges_sent: u32,
    rng: SplitMix64,
}

impl Caller {
    /// A station calling with `call`, its voice and habits drawn from `rng`,
    /// its speed from `settings` (the run's, shifted by − / +).
    pub(super) fn arrive(
        call: GeneratedCall,
        settings: &TrainingSettings,
        rules: &ContestRules,
        run_minutes: u32,
        mut rng: SplitMix64,
    ) -> Self {
        let mut voice = resolve_station(settings, &mut rng);
        let centre = settings.side_tone_center();
        voice.tone_hz = rng
            .pick_in_range(
                centre - SINGLE_PITCH_SPREAD_HZ,
                centre + SINGLE_PITCH_SPREAD_HZ,
            )
            .clamp(SIDE_TONE_MIN_HZ, SIDE_TONE_MAX_HZ);
        let skill = rng.usize_in(1, 3) as u32;
        let patience = draw_patience(&mut rng);
        // A contest whose callers send no serial never reads this one.
        let fit = rules.serials.unwrap_or(super::WPX_SERIALS);
        let serial = SerialModel::draw(fit, run_minutes, &mut rng);
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
            exchange: None,
            exchanges_sent: 0,
            rng,
        }
    }

    /// Hear one of your messages, whole and in order, and settle what it
    /// does next.
    pub(super) fn hear(&mut self, message: &[MessagePart]) -> Reaction {
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
        for part in message {
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
                        NeedQso => replies = Some(true),
                        state if state.worked() => self.state = Done,
                        _ => {}
                    }
                }
                MessagePart::HisCall(sent) if self.state.active() => {
                    let found = match match_call(sent, &self.call.text) {
                        // A piece of its call that is no call itself asks
                        // for the rest, as a partial does: nothing to put
                        // right.
                        CallMatch::Close if parse_callsign(sent).is_none() => CallMatch::Partial {
                            fits: sent.chars().count(),
                        },
                        found => found,
                    };
                    addressed = Some(found != CallMatch::No);
                    reaction.matched |= found != CallMatch::No;
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
                        // The single caller answers any partial: it is the
                        // only one there to ask.
                        (CallMatch::Partial { .. }, state) => {
                            replies = Some(true);
                            reaction.asked = true;
                            self.answered_partial |= state == NeedQso;
                        }
                        (CallMatch::Exact, NeedQso | NeedCallNr) => self.state = NeedNr,
                        (CallMatch::Exact, NeedCall) => self.state = NeedEnd,
                        (CallMatch::Close, NeedQso | NeedNr) => self.state = NeedCallNr,
                        (CallMatch::Close, NeedEnd) => self.state = NeedCall,
                        // Someone else's call: it waits, and calls again
                        // when its timeout runs out.
                        (CallMatch::No, _) => replies = Some(false),
                        _ => {}
                    }
                }
                MessagePart::Exchange if addressed != Some(false) => {
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
                MessagePart::Nil if self.state == NeedQso => self.spend_try(),
                MessagePart::B4 | MessagePart::Nil if self.state.worked() => self.state = Gone,
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
        match self.state {
            CallerState::NeedQso => {
                self.spend_try();
                // It calls again: a `?` now is for it.
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
        self.state.active()
    }

    /// What it sends now, as its state has it, at `run_ms` into the run;
    /// nothing once it is done or gone.
    pub(super) fn reply(
        &mut self,
        rules: &ContestRules,
        side: Side,
        run_ms: u64,
    ) -> Option<(String, Says)> {
        let call = self.call.text.clone();
        let said = match self.state {
            CallerState::NeedQso => (call, Says::Call),
            CallerState::NeedNr => {
                let query = if self.rng.f64() < ASKS_NR {
                    "NR?"
                } else {
                    "AGN"
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
                let body = self.exchange_text(rules, side, run_ms);
                // The first time with a word in front of it, as operators
                // send it; a repeat is the exchange alone.
                let text = if self.exchanges_sent > 0 {
                    body
                } else if self.rng.f64() < TU_FIRST {
                    format!("TU {body}")
                } else {
                    format!("R {body}")
                };
                self.exchanges_sent += 1;
                (text, Says::Exchange)
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
            let serial = self.serial.serial_at(run_ms);
            // Plain digits until the Caller habits setting comes (phase 4).
            let sent = send_serial(serial, CallerHabits::Easy, &mut self.rng);
            // A field this build has no values for is not sent; only those
            // of the contests that have shipped are copied.
            let fields = rules
                .copied(side)
                .iter()
                .filter(|field| **field == Field::Nr)
                .map(|field| (*field, sent.clone()))
                .collect();
            self.exchange = Some(fields);
        }
        self.exchange.as_deref().unwrap_or_default()
    }

    /// Its exchange as it goes out: the report, then each field.
    fn exchange_text(&mut self, rules: &ContestRules, side: Side, run_ms: u64) -> String {
        let report = rules.rst.then(|| REPORT.to_string());
        let tokens: Vec<String> = report
            .into_iter()
            .chain(
                self.exchange(rules, side, run_ms)
                    .iter()
                    .map(|(_, token)| token.clone()),
            )
            .collect();
        tokens.join(" ")
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
    // A Rayleigh draw of the mean asked for: its scale is the mean over
    // √(π/2).
    let scale = PATIENCE_RAYLEIGH_MEAN / FRAC_PI_2.sqrt();
    let u = (1.0 - rng.f64()).clamp(f64::EPSILON, 1.0);
    let rayleigh = scale * (-2.0 * u.ln()).sqrt();
    (PATIENCE_BASE + rayleigh.round() as u32).clamp(PATIENCE_MIN, PATIENCE_MAX)
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
        Caller::arrive(call, &settings, rules(), 10, rng.fork(1))
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

    /// Four to fourteen tries, a mean near six.
    #[test]
    fn patience_is_four_to_fourteen_tries_with_a_mean_near_six() {
        let mut rng = SplitMix64::new(42);
        let draws: Vec<u32> = (0..10_000).map(|_| draw_patience(&mut rng)).collect();
        assert!(draws.iter().all(|p| (4..=14).contains(p)));
        let mean = draws.iter().sum::<u32>() as f64 / draws.len() as f64;
        assert!((mean - 6.0).abs() < 0.3, "mean {mean:.2}");
        assert!(
            draws.iter().any(|p| *p >= 10),
            "a patient caller now and then"
        );
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
        let reaction = caller.hear(&[Cq]);
        assert!(reaction.replies && !reaction.asked && !reaction.matched);
        assert_eq!(
            caller.reply(rules(), Side::Dx, 0).map(|r| r.1),
            Some(Says::Call)
        );
        let reaction = caller.hear(&[call("K5XYZ"), Exchange]);
        assert_eq!(caller.state, CallerState::NeedEnd);
        assert!(reaction.replies && reaction.matched);
        let (text, says) = caller.reply(rules(), Side::Dx, 60_000).expect("a reply");
        assert_eq!(says, Says::Exchange);
        let serial = caller.exchange(rules(), Side::Dx, 0)[0].1.clone();
        assert!(text == format!("TU 5NN {serial}") || text == format!("R 5NN {serial}"));
        // Asked again, it sends the same serial, alone.
        let reaction = caller.hear(&[Again]);
        assert!(reaction.replies && reaction.asked);
        let again = caller.reply(rules(), Side::Dx, 120_000).expect("a reply");
        assert_eq!(again.0, format!("5NN {serial}"));
        assert!(!caller.hear(&[Tu]).replies);
        assert_eq!(caller.state, CallerState::Done);
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
                assert!(!caller.hear(&message).replies, "{state:?} {message:?}");
                assert_eq!(caller.state, state);
            }
            assert!(caller.reply(rules(), Side::Dx, 0).is_none());
            assert!(!caller.time_out());
        }
    }

    /// A CQ ends the QSO: done if the station has sent its exchange, failed
    /// before that.
    #[test]
    fn a_cq_fails_the_worked_qso_until_its_exchange_is_out() {
        let mut waiting = caller(3);
        waiting.hear(&[call("K5XYZ")]);
        assert_eq!(waiting.state, CallerState::NeedNr);
        waiting.hear(&[Cq]);
        assert_eq!(waiting.state, CallerState::Gone);
        let mut corrected = caller(3);
        corrected.hear(&[call("K5XYW"), Exchange]);
        assert_eq!(corrected.state, CallerState::NeedCall);
        corrected.hear(&[Cq]);
        assert_eq!(corrected.state, CallerState::Gone);
        let mut complete = caller(3);
        complete.hear(&[call("K5XYZ"), Exchange]);
        complete.hear(&[Cq]);
        assert_eq!(complete.state, CallerState::Done);
    }

    /// A call close to its own is corrected, and the right one goes on.
    #[test]
    fn a_close_call_is_corrected() {
        let mut caller = caller(4);
        let reaction = caller.hear(&[call("K5XYW"), Exchange]);
        assert!(reaction.replies && reaction.matched && !reaction.asked);
        let (text, says) = caller.reply(rules(), Side::Dx, 0).expect("a reply");
        assert_eq!(says, Says::Correction);
        assert!(text == "DE K5XYZ" || text == "K5XYZ K5XYZ", "{text}");
        caller.hear(&[call("K5XYZ"), Tu]);
        assert_eq!(caller.state, CallerState::Done);
        // Close before your exchange, it waits for both.
        let mut early = self::caller(4);
        early.hear(&[call("K5XY")]);
        assert_eq!(early.state, CallerState::NeedCallNr);
        early.hear(&[call("K5XYZ"), Exchange]);
        assert_eq!(early.state, CallerState::NeedEnd);
    }

    /// Someone else's call: it falls silent and calls again on its timeout.
    #[test]
    fn another_call_silences_a_caller_until_its_timeout() {
        let mut caller = caller(5);
        let reaction = caller.hear(&[call("DL1ABC"), Exchange]);
        assert_eq!(reaction, Reaction::default());
        assert_eq!(caller.state, CallerState::NeedQso);
        assert!(caller.time_out(), "it calls again");
        // Your exchange alone, to nobody it knows of: silent too.
        assert!(!caller.hear(&[Exchange]).replies);
    }

    /// A partial or `?` brings its call again, and you asked for that.
    #[test]
    fn a_partial_or_a_question_brings_the_call_again() {
        let mut caller = caller(6);
        let reaction = caller.hear(&[call("K5?")]);
        assert!(reaction.replies && reaction.asked && reaction.matched);
        assert_eq!(caller.state, CallerState::NeedQso);
        let reaction = caller.hear(&[Again]);
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
            caller.hear(&message);
            assert_eq!(caller.state, before);
            let reaction = caller.hear(&[call("K5X?")]);
            assert!(reaction.replies && reaction.asked && reaction.matched);
            assert_eq!(caller.state, after);
            let (text, says) = caller.reply(rules(), Side::Dx, 0).expect("a reply");
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
        assert!(!caller.hear(&[call("W1AW"), Again]).replies);
        assert!(!caller.hear(&[Again]).replies, "it heard you work W1AW");
        let mut other = self::caller(2);
        other.hear(&[call("W1AW"), Exchange]);
        assert!(!other.hear(&[Again]).replies);
        assert!(other.time_out(), "it calls again");
        assert!(other.hear(&[Again]).replies);
        // After your CQ, a `?` is for it again.
        let mut cq = self::caller(3);
        cq.hear(&[call("W1AW")]);
        cq.hear(&[Cq]);
        assert!(cq.hear(&[Again]).replies);
        // The station you work answers a `?`, but not one after another
        // call.
        let mut worked = self::caller(4);
        worked.hear(&[call("K5XYZ"), Exchange]);
        assert!(!worked.hear(&[call("W1AW"), Again]).replies);
        assert!(worked.hear(&[Again]).replies);
    }

    /// Each message a caller still calling answers costs it a try, as each
    /// timeout does: CQ after CQ, or `?` after `?`, sends it away in the
    /// end.
    #[test]
    fn each_message_a_caller_answers_costs_a_try() {
        let mut caller = caller(3);
        let tries = caller.patience;
        assert!(caller.hear(&[Cq]).replies);
        assert!(caller.hear(&[Again]).replies);
        assert_eq!(caller.patience, tries - 2);
        let mut answered = 2;
        while caller.hear(&[Cq]).replies {
            answered += 1;
        }
        assert_eq!(answered, tries - 1);
        assert_eq!(caller.state, CallerState::Gone);
        // A message it does not answer costs nothing.
        let mut silent = self::caller(3);
        silent.hear(&[call("W1AW"), Exchange]);
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
        caller.hear(&[call("K5XYZ"), Exchange]);
        // Keying in between keeps it, however long it goes on.
        for _ in 0..20 {
            assert!(caller.time_out());
            assert!(caller.time_out());
            caller.hear(&[Again]);
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
        let reaction = together.hear(&[call("K5?"), Exchange]);
        assert!(reaction.replies && reaction.matched && reaction.took_exchange);
        assert_eq!(together.state, CallerState::NeedCall);
        assert!(!together.other_qso);
        let mut after = caller(1);
        after.hear(&[call("K5?")]);
        let reaction = after.hear(&[Exchange]);
        assert!(reaction.replies && reaction.took_exchange);
        assert_eq!(after.state, CallerState::NeedCall);
        assert_eq!(
            after.reply(rules(), Side::Dx, 0).map(|r| r.1),
            Some(Says::Correction)
        );
        let mut moved = caller(1);
        moved.hear(&[call("K5?")]);
        moved.hear(&[call("W1AW")]);
        assert!(!moved.hear(&[Exchange]).replies);
        assert_eq!(moved.state, CallerState::NeedQso);
    }

    /// A piece of its call that is no call itself asks for the rest, as a
    /// partial does: answered with its call, a try spent, nothing put
    /// right. A bare `?` in its place is the `?` of F7: nobody's call.
    #[test]
    fn a_piece_of_the_call_asks_and_a_bare_question_is_nobodys_call() {
        let mut piece = caller(2);
        let tries = piece.patience;
        let reaction = piece.hear(&[call("K5")]);
        assert!(reaction.replies && reaction.asked && reaction.matched);
        assert_eq!(piece.state, CallerState::NeedQso);
        assert_eq!(piece.patience, tries - 1);
        assert_eq!(
            piece.reply(rules(), Side::Dx, 0).map(|r| r.1),
            Some(Says::Call)
        );
        // A slip that is a whole call is still put right.
        let mut slip = caller(2);
        slip.hear(&[call("K5XYW")]);
        assert_eq!(slip.state, CallerState::NeedCallNr);
        let mut bare = caller(2);
        let reaction = bare.hear(&[call("?")]);
        assert!(reaction.replies && reaction.asked && !reaction.matched);
        let mut worked = caller(2);
        worked.hear(&[call("K5XYZ")]);
        worked.hear(&[call("??")]);
        assert_eq!(worked.state, CallerState::NeedNr, "nothing to put right");
    }

    /// With the Call field empty, a `?` is for a caller that heard you work
    /// someone else.
    #[test]
    fn a_question_with_the_field_empty_is_for_anyone_calling() {
        let mut caller = caller(3);
        caller.hear(&[call("W1AW")]);
        assert!(!caller.hear(&[Again]).replies);
        caller.forget_other_qso();
        assert!(caller.hear(&[Again]).replies);
    }
}
