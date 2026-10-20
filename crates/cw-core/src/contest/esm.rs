//! Enter Sends Message: what Enter sends next, worked out from the QSO and
//! the two fields, as MorseRunner's Enter does it.
//!
//! Four things decide it. **C**: the Call field still reads the call last
//! sent. **N**: the station you work took your exchange. **V**: the call is
//! whole, with no `?`. **E**: how much of the exchange the Exchange field
//! holds. Each of MorseRunner's rows that holds adds its part, and the parts
//! go out as one message: `K5XYZ` typed sends `K5XYZ 5NN TT7`; then `?`
//! while the exchange is empty; then `TU`, which logs. Where MorseRunner
//! refuses an exchange it cannot read, Dust sends nothing at all: the fault
//! is in the typing. So too once the station has corrected the call still
//! in the Call field: sent again, it would only be corrected again.
//!
//! It is pure, so the machine runs it on a key and the screen on its own
//! field text, for the key's label and the hint.

use super::exchange::Field;
use super::parse::{ExchangeState, normalize_call, parse_exchange};
use super::rules::{ContestRules, Side};
use crate::callsign::parse_callsign;
use crate::session::MonoMs;

/// What Enter needs to know of the QSO in progress.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QsoState {
    /// The call last sent, as it went out: C holds while the Call field
    /// still reads it. Empty before any.
    pub call_sent: String,
    /// Whether the station you work took your exchange: N. An exchange
    /// nobody took is owed still.
    pub exchange_sent: bool,
    /// The station corrected the call last sent, and no call has gone out
    /// since.
    pub corrected: bool,
    /// The call your TU without logging (F3) went to, while that QSO waits
    /// to be logged: Enter on it, with the exchange copied, sends TU and
    /// logs it, as MorseRunner's C and N still hold after F3. Empty
    /// otherwise.
    pub thanked: String,
}

/// The two fields as typed, with when they were typed: a copy is timed from
/// the end of what it copies to the edit that finished it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields {
    pub call: String,
    pub exch: String,
    /// When the Call field was last edited.
    pub call_edited: MonoMs,
    /// When the Exchange field first read complete, if it has since it was
    /// last cleared.
    pub exch_complete: Option<MonoMs>,
}

impl Fields {
    /// Both fields cleared at `now`, which counts as an edit of the call.
    pub fn cleared(now: MonoMs) -> Self {
        Self {
            call_edited: now,
            ..Self::default()
        }
    }

    /// `field` now holds `text`, typed at `now`. The call is stamped at
    /// every edit. The exchange, read by `rules` from `side`, is stamped
    /// the first time it reads complete since it was last empty: a copy is
    /// timed to there, however much is typed after it.
    pub fn edit(
        &mut self,
        field: EntryField,
        text: String,
        now: MonoMs,
        rules: &ContestRules,
        side: Side,
    ) {
        match field {
            EntryField::Call => {
                self.call = text;
                self.call_edited = now;
            }
            EntryField::Exchange => {
                if text.trim().is_empty() {
                    self.exch_complete = None;
                } else if self.exch_complete.is_none()
                    && parse_exchange(rules, side, &text).state == ExchangeState::Complete
                {
                    self.exch_complete = Some(now);
                }
                self.exch = text;
            }
        }
    }
}

/// The field that has the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryField {
    Call,
    Exchange,
}

/// One part of a message you send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessagePart {
    Cq,
    /// Their call as typed: whole, or a partial that asks (`K5?`).
    HisCall(String),
    /// Your exchange.
    Exchange,
    /// `?`: send it again.
    Again,
    /// TU, which ends the QSO.
    Tu,
    /// Your call alone.
    MyCall,
    /// `QSO B4`: worked before.
    B4,
    /// `NIL`: not in your log.
    Nil,
    /// `NR?`: send your exchange again.
    NrQuery,
}

/// What Enter does now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Esm {
    /// What it sends, as one message. Nothing for an exchange that cannot
    /// be read.
    pub message: Vec<MessagePart>,
    /// The key's label: `CQ`, `Send K5XYZ + exch`, `? Again`, `TU & log`.
    pub label: String,
    /// One line on what to do next.
    pub hint: String,
    /// Where the cursor goes after it, if it moves: to Exchange when Enter
    /// does not log and the call is whole, as MorseRunner advances; back to
    /// Call once the QSO is logged.
    pub focus: Option<EntryField>,
}

impl Esm {
    /// Whether Enter logs the QSO, as its TU always does.
    pub fn logs(&self) -> bool {
        self.message.contains(&MessagePart::Tu)
    }
}

/// What Enter does with the QSO at `qso` and the fields as `fields` hold
/// them, in the contest `rules` describe, from your `side`.
pub fn esm(qso: &QsoState, fields: &Fields, rules: &ContestRules, side: Side) -> Esm {
    let call = normalize_call(&fields.call);
    if call.is_empty() {
        return Esm {
            message: vec![MessagePart::Cq],
            label: "CQ".to_string(),
            hint: "Type the call you hear, then Enter".to_string(),
            focus: None,
        };
    }
    let reading = parse_exchange(rules, side, &fields.exch);
    let e = reading.state;
    let c = call == qso.call_sent;
    let n = qso.exchange_sent;
    let v = parse_callsign(&call).is_some();
    // Your TU went out without logging: Enter logs that QSO, with a TU.
    if !qso.thanked.is_empty() && call == qso.thanked && v && e == ExchangeState::Complete {
        let message = vec![MessagePart::Tu];
        return Esm {
            label: label(&message),
            hint: hint(&message, &call, e, &[]),
            message,
            focus: Some(EntryField::Call),
        };
    }
    // Sent again as it stands, the call would only be corrected again.
    if qso.corrected && c && v {
        return Esm {
            message: Vec::new(),
            label: "Fix call".to_string(),
            hint: "They corrected you: fix the call, then Enter".to_string(),
            focus: Some(EntryField::Call),
        };
    }
    if n && v && e == ExchangeState::Invalid {
        return Esm {
            message: Vec::new(),
            label: label(&[]),
            hint: "Exchange cannot be read: fix it, then Enter".to_string(),
            focus: Some(EntryField::Exchange),
        };
    }
    let mut message = Vec::new();
    if !c || !v || (!n && e == ExchangeState::Empty) {
        message.push(MessagePart::HisCall(call.clone()));
    }
    if !n && v {
        message.push(MessagePart::Exchange);
    }
    if n && v && matches!(e, ExchangeState::Empty | ExchangeState::Missing) {
        message.push(MessagePart::Again);
    }
    if e == ExchangeState::Complete && v && (c || n) {
        message.push(MessagePart::Tu);
    }
    let missing: Vec<Field> = rules
        .copied(side)
        .iter()
        .copied()
        .filter(|field| reading.value(*field).is_none())
        .collect();
    let focus = if message.contains(&MessagePart::Tu) {
        Some(EntryField::Call)
    } else {
        v.then_some(EntryField::Exchange)
    };
    Esm {
        label: label(&message),
        hint: hint(&message, &call, e, &missing),
        message,
        focus,
    }
}

/// The key's label for `message`. Nothing to send is an exchange to fix.
fn label(message: &[MessagePart]) -> String {
    let part = |part: &MessagePart| match part {
        MessagePart::Cq => "CQ".to_string(),
        MessagePart::HisCall(call) => call.clone(),
        MessagePart::Exchange => "exch".to_string(),
        MessagePart::Again => "?".to_string(),
        MessagePart::Tu => "TU & log".to_string(),
        MessagePart::MyCall => "my call".to_string(),
        MessagePart::B4 => "B4".to_string(),
        MessagePart::Nil => "NIL".to_string(),
        MessagePart::NrQuery => "NR?".to_string(),
    };
    match message {
        [] => "Fix exchange".to_string(),
        [MessagePart::Again] => "? Again".to_string(),
        [MessagePart::Cq] | [MessagePart::Tu] => part(&message[0]),
        [MessagePart::HisCall(call)] if call.contains('?') => format!("Ask {call}"),
        [before @ .., MessagePart::Tu] => {
            let before: Vec<String> = before.iter().map(part).collect();
            format!("Send {}, TU & log", before.join(" + "))
        }
        _ => {
            let parts: Vec<String> = message.iter().map(part).collect();
            format!("Send {}", parts.join(" + "))
        }
    }
}

/// One line on what to do next.
fn hint(message: &[MessagePart], call: &str, e: ExchangeState, missing: &[Field]) -> String {
    let nouns = |fields: &[Field]| {
        let nouns: Vec<&str> = fields.iter().map(|field| field.noun()).collect();
        match nouns.split_last() {
            None => String::new(),
            Some((last, [])) => (*last).to_string(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        }
    };
    let resends_call = message
        .iter()
        .any(|part| matches!(part, MessagePart::HisCall(_)));
    if message.contains(&MessagePart::Tu) {
        return if resends_call {
            "Enter sends the corrected call with TU, and logs".to_string()
        } else if message.contains(&MessagePart::Exchange) {
            "Enter sends your exchange with TU, and logs".to_string()
        } else {
            "Enter sends TU and logs the QSO".to_string()
        };
    }
    if message.contains(&MessagePart::Again) {
        return if e == ExchangeState::Missing {
            format!("Still to copy: their {}. Enter asks again", nouns(missing))
        } else {
            format!("Copy their {} into Exchange, then TU & log", nouns(missing))
        };
    }
    if message.contains(&MessagePart::Exchange) {
        return if resends_call {
            "Enter sends their call and your exchange".to_string()
        } else {
            "Enter sends your exchange".to_string()
        };
    }
    if call.contains('?') {
        "Enter asks for the call again: fill in what you hear".to_string()
    } else {
        "Not a whole call yet: finish it, or put ? where you missed some".to_string()
    }
}

/// `message` as it goes out on the air, with `serial` as your serial.
pub fn message_text(
    message: &[MessagePart],
    rules: &ContestRules,
    side: Side,
    my_call: &str,
    serial: u32,
) -> String {
    let parts: Vec<String> = message
        .iter()
        .map(|part| match part {
            MessagePart::Cq => rules.cq_text(my_call),
            MessagePart::HisCall(call) => call.clone(),
            MessagePart::Exchange => rules.exchange_text(side, my_call, serial),
            MessagePart::Again => "?".to_string(),
            MessagePart::Tu => rules.tu_text(my_call),
            MessagePart::MyCall => my_call.to_string(),
            MessagePart::B4 => "QSO B4".to_string(),
            MessagePart::Nil => "NIL".to_string(),
            MessagePart::NrQuery => "NR?".to_string(),
        })
        .collect();
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::ContestKind;
    use MessagePart::{Again, Cq, Exchange, HisCall, Tu};

    fn rules() -> &'static ContestRules {
        ContestKind::Wpx.rules().expect("WPX has shipped")
    }

    /// Enter for a QSO where `call_sent` went out, with your exchange if
    /// `exchange_sent`, and the fields as typed.
    fn enter(call_sent: &str, exchange_sent: bool, call: &str, exch: &str) -> Esm {
        let qso = QsoState {
            call_sent: call_sent.to_string(),
            exchange_sent,
            ..QsoState::default()
        };
        let fields = Fields {
            call: call.to_string(),
            exch: exch.to_string(),
            ..Fields::default()
        };
        esm(&qso, &fields, rules(), Side::Dx)
    }

    fn sent(esm: &Esm) -> String {
        message_text(&esm.message, rules(), Side::Dx, "DL1ABC", 7)
    }

    /// The call is stamped at every edit, the exchange the first time it
    /// reads complete since it was last empty.
    #[test]
    fn the_fields_keep_when_they_were_typed() {
        let edit = |fields: &mut Fields, field, text: &str, at| {
            fields.edit(field, text.to_string(), MonoMs(at), rules(), Side::Dx);
        };
        let mut fields = Fields::cleared(MonoMs(100));
        assert_eq!(fields.call_edited, MonoMs(100));
        edit(&mut fields, EntryField::Call, "K5", 200);
        edit(&mut fields, EntryField::Call, "K5XYZ", 300);
        assert_eq!(
            (fields.call.as_str(), fields.call_edited),
            ("K5XYZ", MonoMs(300))
        );

        edit(&mut fields, EntryField::Exchange, "BOB", 400);
        assert_eq!(fields.exch_complete, None, "an exchange it cannot read");
        edit(&mut fields, EntryField::Exchange, "21", 500);
        edit(&mut fields, EntryField::Exchange, "214", 600);
        assert_eq!(fields.exch, "214");
        assert_eq!(
            fields.exch_complete,
            Some(MonoMs(500)),
            "stamped when first complete"
        );
        assert_eq!(
            fields.call_edited,
            MonoMs(300),
            "the exchange moved the call"
        );

        edit(&mut fields, EntryField::Exchange, " ", 700);
        assert_eq!(fields.exch_complete, None);
        edit(&mut fields, EntryField::Exchange, "7", 800);
        assert_eq!(fields.exch_complete, Some(MonoMs(800)));
    }

    #[test]
    fn an_empty_call_field_calls_cq() {
        let esm = enter("", false, "", "");
        assert_eq!(esm.message, [Cq]);
        assert_eq!(esm.label, "CQ");
        assert_eq!(sent(&esm), "CQ DL1ABC TEST");
        assert_eq!(esm.focus, None);
        // Whatever else is going on.
        assert_eq!(enter("K5XYZ", true, " ", "214").message, [Cq]);
    }

    /// Not C: a call typed and not yet sent goes out with your exchange,
    /// as one message, and the cursor moves on to Exchange.
    #[test]
    fn a_new_call_goes_out_with_your_exchange() {
        let esm = enter("", false, "K5XYZ", "");
        assert_eq!(esm.message, [HisCall("K5XYZ".into()), Exchange]);
        assert_eq!(esm.label, "Send K5XYZ + exch");
        assert_eq!(sent(&esm), "K5XYZ 5NN TT7");
        assert_eq!(esm.focus, Some(EntryField::Exchange));
        assert!(!esm.logs());
    }

    /// Not V: a partial asks, alone, and the cursor stays.
    #[test]
    fn a_partial_goes_out_alone() {
        let esm = enter("", false, "K5?", "");
        assert_eq!(esm.message, [HisCall("K5?".into())]);
        assert_eq!(esm.label, "Ask K5?");
        assert_eq!(sent(&esm), "K5?");
        assert_eq!(esm.focus, None);
        // A call that is not whole yet is sent as it is.
        let unfinished = enter("", false, "K5", "");
        assert_eq!(unfinished.label, "Send K5");
        assert_eq!(unfinished.focus, None);
    }

    /// C N̄ V̄: the partial was sent; Enter sends it again, alone.
    #[test]
    fn a_partial_sent_before_your_exchange_goes_out_again_alone() {
        let esm = enter("K5?", false, "K5?", "");
        assert_eq!(esm.message, [HisCall("K5?".into())]);
        assert_eq!(esm.label, "Ask K5?");
        assert_eq!(esm.focus, None);
    }

    /// C N V̄: after your exchange, a partial still goes out alone.
    #[test]
    fn a_partial_sent_after_your_exchange_goes_out_again_alone() {
        let esm = enter("K5X?", true, "K5X?", "");
        assert_eq!(esm.message, [HisCall("K5X?".into())]);
        assert_eq!(esm.label, "Ask K5X?");
        assert_eq!(esm.focus, None);
        // And with something in Exchange too: nothing else goes with it.
        assert_eq!(enter("K5X?", true, "K5X?", "214").message.len(), 1);
    }

    /// N, V, E empty or missing: `?`.
    #[test]
    fn an_exchange_not_yet_copied_asks_again() {
        let esm = enter("K5XYZ", true, "K5XYZ", "");
        assert_eq!(esm.message, [Again]);
        assert_eq!(esm.label, "? Again");
        assert_eq!(sent(&esm), "?");
        assert_eq!(esm.hint, "Copy their number into Exchange, then TU & log");
        assert_eq!(esm.focus, Some(EntryField::Exchange));
    }

    /// N, V, E invalid: nothing goes out, and the cursor goes to the fault.
    #[test]
    fn an_exchange_that_cannot_be_read_sends_nothing() {
        let esm = enter("K5XYZ", true, "K5XYZ", "21?");
        assert!(esm.message.is_empty());
        assert_eq!(esm.label, "Fix exchange");
        assert_eq!(sent(&esm), "");
        assert_eq!(esm.focus, Some(EntryField::Exchange));
        // Even with the call changed since.
        assert!(enter("K5XYZ", true, "K5XYW", "BOB").message.is_empty());
    }

    /// E complete, V, and C or N: TU, and the QSO is logged.
    #[test]
    fn a_complete_exchange_sends_tu_and_logs() {
        let esm = enter("K5XYZ", true, "K5XYZ", "214");
        assert_eq!(esm.message, [Tu]);
        assert_eq!(esm.label, "TU & log");
        assert_eq!(sent(&esm), "TU");
        assert!(esm.logs());
        assert_eq!(esm.focus, Some(EntryField::Call));
    }

    /// A call corrected after your exchange goes out with the TU.
    #[test]
    fn a_call_corrected_after_your_exchange_goes_out_with_tu() {
        let esm = enter("K5XYW", true, "K5XYZ", "214");
        assert_eq!(esm.message, [HisCall("K5XYZ".into()), Tu]);
        assert_eq!(sent(&esm), "K5XYZ TU");
        assert_eq!(esm.label, "Send K5XYZ, TU & log");
        assert!(esm.logs());
        assert_eq!(esm.focus, Some(EntryField::Call));
        // Corrected with nothing copied yet: the call, then `?`.
        let asks = enter("K5XYW", true, "K5XYZ", "");
        assert_eq!(sent(&asks), "K5XYZ ?");
        assert_eq!(asks.label, "Send K5XYZ + ?");
    }

    /// The station corrected the call: while the Call field still reads the
    /// call sent, Enter sends nothing and the cursor goes back to it, since
    /// a `?` would only bring the correction again.
    #[test]
    fn after_a_correction_enter_waits_for_the_call_to_be_fixed() {
        let qso = QsoState {
            call_sent: "K5XYW".to_string(),
            exchange_sent: true,
            corrected: true,
            ..QsoState::default()
        };
        let fields = |call: &str, exch: &str| Fields {
            call: call.to_string(),
            exch: exch.to_string(),
            ..Fields::default()
        };
        for exch in ["", "214"] {
            let esm = esm(&qso, &fields("K5XYW", exch), rules(), Side::Dx);
            assert!(esm.message.is_empty(), "{exch:?}");
            assert_eq!(esm.label, "Fix call");
            assert_eq!(esm.hint, "They corrected you: fix the call, then Enter");
            assert_eq!(esm.focus, Some(EntryField::Call));
        }
        // Fixed, it goes out with what the QSO needs.
        let fixed = esm(&qso, &fields("K5XYZ", ""), rules(), Side::Dx);
        assert_eq!(sent(&fixed), "K5XYZ ?");
        let fixed = esm(&qso, &fields("K5XYZ", "214"), rules(), Side::Dx);
        assert_eq!(sent(&fixed), "K5XYZ TU");
    }

    /// The call went out without your exchange (an Esc cut it short): Enter
    /// sends the exchange, with the TU too once theirs is copied.
    #[test]
    fn an_exchange_cut_short_goes_out_again() {
        let empty = enter("K5XYZ", false, "K5XYZ", "");
        assert_eq!(
            sent(&empty),
            "K5XYZ 5NN TT7",
            "the call again, as nothing is copied"
        );
        let copied = enter("K5XYZ", false, "K5XYZ", "214");
        assert_eq!(sent(&copied), "5NN TT7 TU");
        assert_eq!(copied.label, "Send exch, TU & log");
        assert!(copied.logs());
        let unreadable = enter("K5XYZ", false, "K5XYZ", "21?");
        assert_eq!(
            sent(&unreadable),
            "5NN TT7",
            "your exchange is owed whatever you typed"
        );
        assert_eq!(unreadable.label, "Send exch");
    }

    /// After your TU without logging, Enter on the call it went to, with
    /// the exchange copied, sends TU and logs; any other call, or an
    /// exchange not yet copied, starts afresh.
    #[test]
    fn after_a_tu_without_logging_enter_logs_that_qso() {
        let qso = QsoState {
            thanked: "K5XYZ".to_string(),
            ..QsoState::default()
        };
        let fields = |call: &str, exch: &str| Fields {
            call: call.to_string(),
            exch: exch.to_string(),
            ..Fields::default()
        };
        let enter = esm(&qso, &fields("K5XYZ", "214"), rules(), Side::Dx);
        assert_eq!(enter.message, [Tu]);
        assert_eq!(enter.label, "TU & log");
        assert!(enter.logs());
        assert_eq!(
            sent(&esm(&qso, &fields("W2AW", "214"), rules(), Side::Dx)),
            "W2AW 5NN TT7"
        );
        assert_eq!(
            sent(&esm(&qso, &fields("K5XYZ", ""), rules(), Side::Dx)),
            "K5XYZ 5NN TT7"
        );
    }

    /// The fields are read as the Call field keeps them: lower case and
    /// spaces make no difference.
    #[test]
    fn the_call_field_is_read_as_a_call() {
        let esm = enter("K5XYZ", true, " k5xyz ", "214");
        assert_eq!(esm.message, [Tu]);
    }

    /// The messages only the keys send go out as operators send them.
    #[test]
    fn the_other_messages_go_out_as_operators_send_them() {
        use MessagePart::{B4, MyCall, Nil, NrQuery};
        for (part, text, key) in [
            (MyCall, "DL1ABC", "Send my call"),
            (B4, "QSO B4", "Send B4"),
            (Nil, "NIL", "Send NIL"),
            (NrQuery, "NR?", "Send NR?"),
        ] {
            let message = [part];
            assert_eq!(message_text(&message, rules(), Side::Dx, "DL1ABC", 7), text);
            assert_eq!(label(&message), key);
        }
    }

    #[test]
    fn every_situation_has_a_hint() {
        for (call_sent, exchange_sent, call, exch) in [
            ("", false, "", ""),
            ("", false, "K5XYZ", ""),
            ("", false, "K5?", ""),
            ("", false, "K5", ""),
            ("K5XYZ", true, "K5XYZ", ""),
            ("K5XYZ", true, "K5XYZ", "21?"),
            ("K5XYZ", true, "K5XYZ", "214"),
            ("K5XYW", true, "K5XYZ", "214"),
            ("K5XYZ", false, "K5XYZ", "214"),
        ] {
            let esm = enter(call_sent, exchange_sent, call, exch);
            assert!(!esm.hint.is_empty(), "{call:?} {exch:?}");
            assert!(!esm.label.is_empty(), "{call:?} {exch:?}");
        }
    }
}
