//! The history as records: each session one record, written once and never
//! written over, named by where it falls in the history and by what it
//! holds ([`RecordId`]). A session deleted is a tombstone beside it, by the
//! same name.
//!
//! A history kept whole, as one file or one browser key, was read whole,
//! merged whole and written whole every time a session ended, and had to
//! be kept to a size it could be written in: records are written one at a
//! time, so it can grow for as long as the user practises. Nothing a copy
//! of the app writes goes over what another wrote, so copies running at
//! once need no merge: each takes up the records and tombstones the other
//! left ([`super::Store::history_listing`]).

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use cw_core::SessionResult;

use super::{Deleted, SetAside};

/// What names a record: `order`, where it falls in the history, which is
/// the order sessions finished in, and `nonce`, what it holds
/// ([`fnv64`] of exactly the bytes written).
///
/// The order is the stamp in milliseconds times a thousand, so a session
/// finished later falls later, with room for a thousand stamped in one
/// millisecond. Where the clock was set back, a new session goes after the
/// newest anyway ([`RecordId::next`]): it finished after it.
/// Names sort as their ids do, so a folder listed in name order is the
/// history in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordId {
    pub order: u64,
    pub nonce: u64,
}

/// How long a record's name is: twenty digits of order, a dash, sixteen
/// hex digits of nonce.
#[cfg_attr(feature = "web", allow(dead_code))]
const NAME_LEN: usize = 20 + 1 + 16;

impl RecordId {
    /// The record's name, as a file or a key is called: `order` in twenty
    /// digits, a dash, and `nonce` in sixteen lowercase hex digits.
    pub fn name(self) -> String {
        format!("{:020}-{:016x}", self.order, self.nonce)
    }

    /// The id a name names, if it is one: exactly as [`Self::name`] writes
    /// it, so one id has one name and a name of anything else is no id.
    #[cfg_attr(feature = "web", allow(dead_code))]
    pub fn parse(name: &str) -> Option<Self> {
        let bytes = name.as_bytes();
        if bytes.len() != NAME_LEN || bytes[20] != b'-' {
            return None;
        }
        let (order, nonce) = (&name[..20], &name[21..]);
        let digits = order.bytes().all(|b| b.is_ascii_digit());
        let hex = nonce
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !digits || !hex {
            return None;
        }
        Some(Self {
            order: order.parse().ok()?,
            nonce: u64::from_str_radix(nonce, 16).ok()?,
        })
    }

    /// A session just finished at `timestamp`, written as `json`, after
    /// `newest`, the newest record of the history: at its stamp, or just
    /// after the newest where a clock set back stamped it before that. It
    /// finished after every session there is, whatever the clock said.
    pub fn next(timestamp: u64, newest: Option<RecordId>, json: &str) -> Self {
        let after = newest.map_or(0, |newest| newest.order.saturating_add(1));
        Self {
            order: (timestamp.saturating_mul(1000)).max(after),
            nonce: fnv64(json.as_bytes()),
        }
    }

    /// A session finished at `timestamp` that comes from elsewhere — a
    /// backup restored, a copy the app was cut off in, an older build's
    /// history — written as `json`: where it finished, after any session
    /// finished in the same millisecond here. The same session placed twice
    /// is the same record, so two copies restoring one backup store each
    /// session once.
    pub fn placed(timestamp: u64, json: &str) -> Self {
        Self {
            order: timestamp.saturating_mul(1000).saturating_add(999),
            nonce: fnv64(json.as_bytes()),
        }
    }

    /// A session that takes the place of the record `was`, filled in, say,
    /// by a restore, written as `json`: in its place, and named by what it
    /// holds now.
    pub fn beside(was: RecordId, json: &str) -> Self {
        Self {
            order: was.order,
            nonce: fnv64(json.as_bytes()),
        }
    }

    /// The next name in the same place, for a record whose own name some
    /// other bytes already have. Only bytes damaged where they lie, or two
    /// sessions alike in their place and their hash, ever have it.
    #[cfg_attr(feature = "web", allow(dead_code))]
    pub fn bumped(self) -> Self {
        Self {
            nonce: self.nonce.wrapping_add(1),
            ..self
        }
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// The 64-bit FNV-1a hash of `bytes`: what a record's nonce is of what it
/// holds. Written out here rather than taken from a crate, as it is part of
/// what every record is named by, and must never change with a dependency.
pub fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A session as it is stored: its id, and the bytes written for it, which
/// are what this build writes for it ([`SessionResult`]'s own JSON). They
/// are written once and never again, so whatever a later build added to a
/// session stays with it however many copies of older builds read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: RecordId,
    pub json: String,
}

impl Record {
    /// `session` as this build writes it, named by `id` from what it
    /// writes. None only where the session cannot be written at all, which
    /// a session read or finished by this build always can.
    pub fn of(session: &SessionResult, id: impl FnOnce(&str) -> RecordId) -> Option<Self> {
        let json = serde_json::to_string(session).ok()?;
        Some(Self {
            id: id(&json),
            json,
        })
    }
}

/// Every session of a history kept whole, oldest first, as records in the
/// same order: what the move from such a history writes.
///
/// Each falls at its stamp, or just after the one before it where its
/// stamp is earlier, as a clock set back stamped it: a history kept whole
/// is in the order its sessions finished, and so are the records. Sessions
/// alike in everything are still two records, a place apart. The same
/// history always comes out as the same records, so a move cut short and
/// made again writes nothing new.
pub fn records_for_blob(sessions: &[SessionResult]) -> Vec<Record> {
    let mut last: Option<RecordId> = None;
    sessions
        .iter()
        .filter_map(|session| {
            let record = Record::of(session, |json| {
                RecordId::next(session.timestamp, last, json)
            })?;
            last = Some(record.id);
            Some(record)
        })
        .collect()
}

/// A stored session read, `raw`, as it is read whatever it is read from: a
/// record, or one element of a history kept whole ([`super::legacy`]).
///
/// It is scored again as it is read, from what is stored of it, so the
/// history is on one scale however the score has been worked out over the
/// versions: sessions saved before speed counted read as they would score
/// today. Its stamp and day are read as they were stored, whatever the
/// clock says now. What a later build added to it is carried along
/// ([`cw_core::UnreadKeys`]). None where it does not read as a session.
pub fn read_record(raw: &str) -> Option<SessionResult> {
    serde_json::from_str(raw).ok().map(scored)
}

/// `session`, scored as sessions score today.
pub(super) fn scored(mut session: SessionResult) -> SessionResult {
    session.score = session.points();
    session
}

/// A change to the history, made whole by the store or not at all: what is
/// stored afresh, what is deleted, and what deleted before is put back.
///
/// A store makes them in an order that never loses a session to a change
/// cut short: deletions undone first, then the records stored, then the
/// deletions, so a session filled in is stored before the one it replaces
/// goes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryChange {
    /// Records to store, where they are not stored already.
    pub put: Vec<Record>,
    /// Records deleted by hand, each with what is remembered of it.
    pub forget: Vec<(RecordId, Deleted)>,
    /// Records whose deletion is undone: put back with the same id.
    pub unforget: Vec<RecordId>,
    /// The change restores a backup: should Dust stop before it is made,
    /// the user is told, so they can restore it again
    /// ([`HistoryNotice::CutShort`]). No other change is told of so: what
    /// the app stores of its own it keeps until stored, and what is taken
    /// up from an older build is taken up again at the next read.
    #[cfg_attr(feature = "web", allow(dead_code))]
    pub restore: bool,
}

impl HistoryChange {
    #[cfg_attr(feature = "web", allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.put.is_empty() && self.forget.is_empty() && self.unforget.is_empty()
    }
}

/// The history as a store read it ([`super::Store::load_history`]).
#[derive(Debug, Default, PartialEq)]
pub struct HistoryLoad {
    /// Every session that reads, by its record, in the order of the
    /// history.
    pub sessions: Vec<(RecordId, SessionResult)>,
    /// Records there that this build cannot read: left where they are, for
    /// a build that can.
    pub unread: Vec<RecordId>,
    /// What the move from a history kept whole set aside, as it could not
    /// read it.
    pub set_aside: Vec<SetAside>,
    /// What to tell the user of the history.
    pub notices: Vec<HistoryNotice>,
    /// Every record there is was listed, and the move from a history kept
    /// whole is made: what is missing is not stored, not merely unseen.
    pub complete: bool,
}

/// Something about the history to tell the user, found as it was read
/// ([`HistoryLoad::notices`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "web", allow(dead_code))]
pub enum HistoryNotice {
    /// `count` records this build cannot read, found for the first time.
    /// They are left where they are: a build that reads them shows them.
    Unread { count: usize },
    /// A restore of `meant` sessions stopped before it was done, as Dust
    /// stopped: `stored` of them were stored, or are deleted by hand and
    /// left so ([`HistoryChange::restore`]).
    CutShort { meant: usize, stored: usize },
    /// The history kept whole by an older build could not be read for now,
    /// for the reason given, so it is not brought over yet: the next
    /// launch tries again.
    MovePending(String),
    /// A newer Dust keeps the history here in a way this one cannot write:
    /// what it holds is read, and nothing is written.
    Newer,
}

/// What a newer Dust's history says to this one ([`HistoryNotice::Newer`]).
pub const NEWER_LAYOUT: &str = "A newer version of Dust changed how history is kept here; update Dust to keep saving sessions.";

impl HistoryNotice {
    /// The notice in words.
    pub fn text(&self) -> String {
        match self {
            Self::Unread { count } => {
                let (a_session, left, them) = if *count == 1 {
                    ("1 session".to_string(), "it was left as it is", "it")
                } else {
                    (
                        format!("{count} sessions"),
                        "they were left as they are",
                        "them",
                    )
                };
                format!(
                    "{a_session} in your history could not be read, perhaps saved by a newer Dust, so {left}. A Dust that reads {them} shows {them} again."
                )
            }
            Self::CutShort { meant, stored } => format!(
                "Dust stopped while it stored {meant} sessions at once, as a restore does, and only {stored} of them were stored. Restore that backup again to store the rest: no session is added twice."
            ),
            Self::MovePending(why) => format!(
                "Your sessions from before this version of Dust could not be read just now ({why}). They are left as they were, and Dust tries again the next time it opens."
            ),
            Self::Newer => NEWER_LAYOUT.into(),
        }
    }
}

/// What a store does with the history, on its way: done already where the
/// store does it then and there, or still going where it waits on the
/// platform, as a browser's database makes it.
pub type Pending<T> = Pin<Box<dyn Future<Output = T>>>;

/// `value`, done already.
pub fn done<T: 'static>(value: T) -> Pending<T> {
    Box::pin(std::future::ready(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::tests::session;

    /// A session finished at `timestamp`, on a day of its own.
    fn at(timestamp: u64) -> SessionResult {
        SessionResult {
            timestamp,
            ..session(&format!("2026-10-{:02}", timestamp % 28 + 1))
        }
    }

    fn json(session: &SessionResult) -> String {
        serde_json::to_string(session).expect("a session writes")
    }

    /// A record's name says where it falls and what it holds, and reads
    /// back as the same id; nothing else reads as a name.
    #[test]
    fn a_record_is_named_by_its_place_and_what_it_holds() {
        let session = at(1_790_000_000_123);
        let written = json(&session);
        let id = RecordId::next(session.timestamp, None, &written);
        assert_eq!(id.order, 1_790_000_000_123_000);
        assert_eq!(id.nonce, fnv64(written.as_bytes()));
        let name = id.name();
        assert_eq!(name.len(), NAME_LEN);
        assert!(name.starts_with("00001790000000123000-"), "{name}");
        assert_eq!(RecordId::parse(&name), Some(id));
        assert_eq!(id.to_string(), name);
        // The bytes name it: other bytes, another name.
        let other = SessionResult {
            accuracy: 0.5,
            ..session
        };
        assert_ne!(RecordId::next(other.timestamp, None, &json(&other)), id);
        // Names sort as ids do.
        let later = RecordId {
            order: id.order + 1,
            nonce: 0,
        };
        assert!(id < later && name < later.name());
        let small = RecordId {
            order: 9,
            nonce: 0xab,
        };
        assert_eq!(small.name(), "00000000000000000009-00000000000000ab");
        // FNV-1a as published.
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv64(b"foobar"), 0x8594_4171_f739_67e8);
        for not_a_name in [
            "",
            "00000000000000000009-00000000000000AB",
            "0000000000000000009-000000000000000ab",
            "00000000000000000009_00000000000000ab",
            "0000000000000000000x-00000000000000ab",
            "99999999999999999999-00000000000000ab",
            "00000000000000000009-00000000000000ab.json",
        ] {
            assert_eq!(RecordId::parse(not_a_name), None, "{not_a_name}");
        }
    }

    /// A session a clock set back stamped before the newest goes after it,
    /// as it finished after it; one stamped ahead goes at its stamp.
    #[test]
    fn a_session_a_clock_set_back_stamped_goes_after_the_newest() {
        let newest = RecordId {
            order: 5_000_000,
            nonce: 7,
        };
        let behind = at(1_000);
        let id = RecordId::next(behind.timestamp, Some(newest), &json(&behind));
        assert_eq!(id.order, newest.order + 1);
        assert!(id > newest);
        let ahead = at(9_000);
        let id = RecordId::next(ahead.timestamp, Some(newest), &json(&ahead));
        assert_eq!(id.order, 9_000_000);
    }

    /// A session from elsewhere falls where it finished: after what
    /// finished in the same millisecond here, before what finished after.
    #[test]
    fn a_restored_session_falls_where_it_finished() {
        let (before, same, after) = (at(1_000), at(2_000), at(3_000));
        let ids: Vec<RecordId> = records_for_blob(&[before, same.clone(), after])
            .into_iter()
            .map(|record| record.id)
            .collect();
        let mut restored = same;
        restored.accuracy = 0.25;
        let placed = RecordId::placed(restored.timestamp, &json(&restored));
        assert!(ids[1] < placed && placed < ids[2], "{ids:?} {placed:?}");
        // Filled in, a record keeps its place, under a name of its own.
        let filled = RecordId::beside(ids[1], &json(&restored));
        assert_eq!(filled.order, ids[1].order);
        assert_ne!(filled, ids[1]);
        assert_eq!(ids[1].bumped().order, ids[1].order);
        assert_ne!(ids[1].bumped(), ids[1]);
    }

    /// The same session restored twice, or by two copies at once, is the
    /// same record.
    #[test]
    fn the_same_session_restored_twice_is_one_record() {
        // Scored as this build scores it, as every session it stores is.
        let session = scored(at(1_234));
        let first = Record::of(&session, |json| RecordId::placed(session.timestamp, json))
            .expect("a record");
        // Read back from a backup, as the next restore reads it.
        let read = read_record(&first.json).expect("reads");
        let again = Record::of(&read, |json| RecordId::placed(read.timestamp, json));
        assert_eq!(again, Some(first));
    }

    /// The records of a history kept whole keep its order, whatever the
    /// stamps say, and sessions alike in everything are two records still;
    /// made again, they come out the same.
    #[test]
    fn migration_ids_keep_the_blobs_order_even_for_sessions_alike() {
        let history = [at(5), at(3), at(5), at(5), at(9)];
        let records = records_for_blob(&history);
        assert_eq!(records.len(), history.len());
        let orders: Vec<u64> = records.iter().map(|record| record.id.order).collect();
        assert_eq!(orders, [5_000, 5_001, 5_002, 5_003, 9_000]);
        assert!(records.windows(2).all(|pair| pair[0].id < pair[1].id));
        assert_eq!(records[0].json, records[2].json, "alike");
        assert_ne!(records[0].id, records[2].id, "and still two");
        assert_eq!(records_for_blob(&history), records, "the same every time");
        let read: Vec<SessionResult> = records
            .iter()
            .map(|record| read_record(&record.json).expect("reads"))
            .collect();
        let scored_as_read: Vec<SessionResult> = history.into_iter().map(scored).collect();
        assert_eq!(read, scored_as_read);
    }

    /// A record reads back scored as sessions score today, stamped as it
    /// was stored, and with whatever a later build added to it.
    #[test]
    fn a_record_reads_back_scored_as_today_with_what_a_later_build_added() {
        let mut stored = session("2026-01-01");
        stored.groups = vec![cw_core::GroupResult {
            sent: "KMURE".into(),
            received: "KMURE".into(),
            correct: true,
        }];
        stored.group_timings = vec![cw_core::SessionTiming {
            time_to_complete_ms: 500.0,
            char_wpm: Some(35.0),
            sends: 1,
            ..cw_core::SessionTiming::default()
        }];
        stored.score = 1.0;
        stored.timestamp = 4_000_000_000_000;
        let mut raw = serde_json::to_value(&stored).expect("a session writes");
        raw["laterKey"] = serde_json::json!({ "kept": [1, 2.5] });
        let read = read_record(&raw.to_string()).expect("a session");
        assert_eq!(read.score, stored.points());
        assert!(read.score > 1.0);
        assert_eq!(read.timestamp, 4_000_000_000_000);
        assert_eq!(
            serde_json::to_value(&read).expect("writes")["laterKey"],
            raw["laterKey"]
        );
        assert_eq!(read_record("{\"date\":\"nonsense\"}"), None);
        assert_eq!(read_record("[]"), None);
    }

    /// Each notice says what happened and what to do, in plain words.
    #[test]
    fn what_became_of_the_history_is_said_plainly() {
        let one = HistoryNotice::Unread { count: 1 }.text();
        assert!(one.starts_with("1 session in your history"), "{one}");
        assert!(one.contains("so it was left as it is."), "{one}");
        let many = HistoryNotice::Unread { count: 3 }.text();
        assert!(many.starts_with("3 sessions"), "{many}");
        assert!(many.contains("shows them again"), "{many}");
        let cut = HistoryNotice::CutShort {
            meant: 40,
            stored: 12,
        }
        .text();
        assert!(
            cut.contains("40 sessions") && cut.contains("only 12"),
            "{cut}"
        );
        let pending = HistoryNotice::MovePending("permission denied".into()).text();
        assert!(pending.contains("(permission denied)"), "{pending}");
        assert_eq!(HistoryNotice::Newer.text(), NEWER_LAYOUT);
        assert!(HistoryChange::default().is_empty());
    }
}
