//! Enter Sends Message: what Enter sends next, worked out from the QSO and
//! the two fields, as MorseRunner's Enter does it.
//!
//! Four things decide it. **C**: the Call field still reads the call last
//! sent. **N**: your exchange went out. **V**: the call is whole, with no
//! `?`. **E**: how much of the exchange the Exchange field holds. Each of
//! MorseRunner's rows that holds adds its part, and the parts go out as one
//! message: `K5XYZ` typed sends `K5XYZ 5NN TT7`; then `?` while the exchange
//! is empty; then `TU`, which logs. Where MorseRunner refuses an exchange it
//! cannot read, Dust sends nothing at all: the fault is in the typing.
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
    /// Whether your exchange went out: N.
    pub exchange_sent: bool,
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
