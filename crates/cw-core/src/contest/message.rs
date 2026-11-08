//! The parts a message you send is made of.

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
    /// `NR?`, or the contest's own word for it: send your exchange again.
    NrQuery,
}
