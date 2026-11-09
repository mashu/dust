//! A backup: the history and the settings in one file the user keeps,
//! outside wherever this build stores them, to put back after that store is
//! lost — a browser's data cleared — or on another device.
//!
//! Reading one is as forgiving as reading the store: a session that no longer
//! parses is left out, a setting that no longer reads falls back to its
//! default, and every session is scored the way sessions score today.
//!
//! A history may be as long as the user has practised, so a backup is never
//! held whole on its way out: the store writes it a session at a time, as
//! each is stored ([`BackupFile`]). Read back, only its sessions are held,
//! each read from where it lies in the file ([`read_backup`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{self, Write};

use cw_core::{SessionResult, TrainingSettings};
use serde::Deserialize;
use serde_json::value::RawValue;

#[cfg(test)]
use super::legacy::parse_sessions;
use super::record::{Pending, Record, RecordId, scored};
use super::session::SessionKey;
use super::settings::load_settings_from;

/// The key that marks a file as a backup, and the version of its layout.
const MARK: &str = "dustBackup";
const LAYOUT: u32 = 1;

/// How many sessions may go unbacked before the History tab says so.
pub const BACKUP_DUE_AFTER: usize = 10;

/// The largest file read as a backup: a gigabyte, far more than the longest
/// history comes to, at some seven kilobytes a session. Any history
/// restores; a file far larger, a film say, is not read whole only to find
/// it is no backup.
pub const BACKUP_MAX_BYTES: u64 = 1_000_000_000;

/// How large a backup has to be for restoring it to be said to take a
/// while: fifty megabytes, some seven thousand sessions.
pub const BACKUP_LARGE_BYTES: u64 = 50_000_000;

/// The longest a session in a backup may be and still be read: four
/// megabytes. A session is some seven kilobytes, and the longest session
/// of groups there can be, two hundred of twenty characters each timed,
/// some a hundred; a contest run of an hour, worked at twenty QSOs a
/// minute, far faster than anyone works, comes to a megabyte and a half
/// with its log. One longer is left out unread.
const SESSION_MAX_BYTES: usize = 4_000_000;

/// How much memory what the sessions in a backup hold that this build does
/// not read may take, in all ([`unread_bytes`]): two hundred and fifty-six
/// megabytes, room for a key a later build added to each of the twenty
/// groups of sixteen thousand sessions. Kept as it was read, to be written
/// back with them, it takes many times as much memory as it does in the
/// file, a small object some ninety times, so a session that would take it
/// past this is left out, as one that does not read is ([`each_session`]).
const UNREAD_MAX_BYTES: usize = 256_000_000;

/// How much memory the logs of the contest runs in a backup may take, in
/// all ([`log_bytes`]): three hundred megabytes. A run of an hour logs some
/// eighty kilobytes worked at twenty QSOs a minute, far faster than anyone
/// works, and some sixteen at 235 an hour; a log is kept whole, as it was
/// read, whatever it holds ([`cw_core::ContestLog`]), and takes some
/// seventeen times as much memory as it does in the file, so this is two
/// hundred runs of the one, or a thousand of the other. A run that would
/// take it past this is left out, as a session that does not read is
/// ([`each_session`]).
const RUN_LOGS_MAX_BYTES: usize = 300_000_000;

/// How much memory what this build reads of the sessions in a backup may
/// take, in all, for each byte of the list that holds them
/// ([`typed_bytes`]): three. Sessions as Dust has stored them come to at
/// most some two and a quarter, in the first builds' shape with groups of
/// one character left unanswered, with no character's times kept and keys
/// no build reads any more; and to one or one and a half as this build
/// stores them. The room their list holds as it grows adds two tenths at
/// most. A list of nothing but bare timings, `{"timeToCompleteMs":0}`,
/// comes to five and a half, and is held to this.
const TYPED_PER_BYTE: usize = 3;

/// The same, in all, for a list so short that its bytes allow less:
/// sixteen megabytes.
const TYPED_FLOOR: usize = 16_000_000;

/// The longest the settings in a backup may be and still be read: a
/// megabyte, far more than settings come to, a word list of two thousand
/// words among them. Longer, they are read as settings that do not read.
const SETTINGS_MAX_BYTES: usize = 1_000_000;

/// What a backup holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Backup {
    /// When it was saved, in milliseconds since the epoch; zero if it does
    /// not say.
    pub saved_at: u64,
    /// None where it holds no settings: a history on its own.
    pub settings: Option<TrainingSettings>,
    /// Oldest first.
    pub sessions: Vec<SessionResult>,
    /// How many of its sessions this version could not read, and left out.
    pub left_out: usize,
}

impl Backup {
    /// What to say of the sessions in it this version could not read, and
    /// left out, as it is restored: nothing, where there are none.
    pub fn told_left_out(left_out: usize) -> Option<String> {
        match left_out {
            0 => None,
            1 => Some(
                "1 session in it could not be read by this version and was left out.".to_string(),
            ),
            count => Some(format!(
                "{count} sessions in it could not be read by this version and were left out."
            )),
        }
    }
}

#[cfg(test)]
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Written<'a> {
    dust_backup: u32,
    saved_at: u64,
    settings: &'a TrainingSettings,
    sessions: &'a [SessionResult],
}

/// A backup of `settings` and `sessions`, saved at `saved_at`, as the file
/// holds it, written whole, as builds before wrote it: what a backup
/// written a session at a time comes to, byte for byte ([`BackupFile`]).
#[cfg(test)]
pub fn write_backup(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
    saved_at: u64,
) -> String {
    serde_json::to_string(&Written {
        dust_backup: LAYOUT,
        saved_at,
        settings,
        sessions,
    })
    .unwrap_or_default()
}

/// A backup on its way to a file: the settings and when it was saved, and
/// the sessions here that the store has yet to take. The store writes it
/// with every session it holds, each as it holds it, a session at a time
/// ([`BackupFile::write`]), so a history of any length is never held whole
/// to be backed up. The file is a backup as [`write_backup`] writes one,
/// which every version of Dust reads.
///
/// A store that cannot read all it holds saves nothing and says so
/// ([`NotSaved::Unread`]), and the app saves the backup again holding every
/// session here ([`BackupFile::holding_every_session`]): it then holds what
/// the app shows, whatever became of the store.
#[derive(Clone, Debug, PartialEq)]
pub struct BackupFile {
    /// What comes before the sessions: the mark, when it was saved, and the
    /// settings.
    head: String,
    /// The sessions here that go in beside what the store holds, by record,
    /// in order: those it has yet to take, or every one here.
    held: Vec<Record>,
    /// Whether `held` is every session here, for a store that could not
    /// read all it holds.
    every: bool,
    /// The records stored that this build could not read when it read
    /// them: only the store holds them, so one it cannot read now is left
    /// out, and the app is not asked for it.
    unread: BTreeSet<RecordId>,
}

/// What a backup file holds of the sessions ([`BackupFile::write`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackedUp {
    /// The sessions in it.
    pub sessions: usize,
    /// The sessions stored that could not go in it: what is stored of each
    /// could not be read at all, or is not even JSON, and the app holds
    /// none of it.
    pub left_out: usize,
}

/// Why a backup file was not saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotSaved {
    /// The store could not read all it holds: its history could not be
    /// listed, or a session in it that the app may hold could not be read.
    /// Nothing was saved. Saved again holding every session here
    /// ([`BackupFile::holding_every_session`]), it holds what the app does.
    Unread,
    /// Why not, in words that follow "could not be saved:".
    Failed(String),
}

impl NotSaved {
    /// Why not, in words that follow "could not be saved:".
    pub fn why(&self) -> &str {
        match self {
            Self::Unread => "the history stored here could not be read",
            Self::Failed(why) => why,
        }
    }
}

impl From<io::Error> for NotSaved {
    fn from(err: io::Error) -> Self {
        Self::Failed(err.to_string())
    }
}

impl BackupFile {
    /// A backup of `settings`, saved at `saved_at`, holding `unsaved`, the
    /// sessions here the store has yet to take, beside what it holds.
    pub fn new(settings: &TrainingSettings, saved_at: u64, mut unsaved: Vec<Record>) -> Self {
        unsaved.sort_by_key(|record| record.id);
        let mut head = format!("{{\"{MARK}\":{LAYOUT},\"savedAt\":{saved_at},");
        // Settings that will not be written are left out, and a restore
        // takes none: never the defaults in their place.
        if let Ok(settings) = serde_json::to_string(settings) {
            head.push_str("\"settings\":");
            head.push_str(&settings);
            head.push(',');
        }
        head.push_str("\"sessions\":[");
        Self {
            head,
            held: unsaved,
            every: false,
            unread: BTreeSet::new(),
        }
    }

    /// The same backup, where `unread` are the records stored that this
    /// build could not read when it read them, which only the store holds.
    pub fn beside_unread(self, unread: BTreeSet<RecordId>) -> Self {
        Self { unread, ..self }
    }

    /// The same backup, the sessions it was made with every one here: for
    /// a store that could not read all it holds ([`NotSaved::Unread`]).
    /// Each goes in where the store holds none of it that reads, but for
    /// one the store holds as deleted; and where the store cannot list what
    /// it holds at all, they are the backup.
    pub fn holding_every_session(self) -> Self {
        Self {
            every: true,
            ..self
        }
    }

    /// Write the backup to `out`, with `stored`, every record the store
    /// holds, deleted ones left out, in the order of the history: each as
    /// it is stored, which keeps whatever a later build added to it, and
    /// those here that go in beside it in their places among them. A
    /// record the store holds and the app has yet to hear it took is one
    /// session, as stored. One here that the store holds as `deleted` stays
    /// out, whatever the app has yet to hear of it: restoring the backup
    /// would bring it back.
    ///
    /// A record this build cannot read goes in as it is, so a build that
    /// reads it restores it. One that cannot be read at all, or is not even
    /// JSON, goes in as the app holds it. Where the app may hold it and has
    /// not given it, the backup stops there, to be saved by no store, and
    /// the app is asked for it ([`NotSaved::Unread`]); where it holds none,
    /// the session cannot go in a file of JSON, and is counted as left out.
    pub fn write(
        &self,
        out: &mut dyn Write,
        stored: impl IntoIterator<Item = Result<Record, RecordId>>,
        deleted: impl Fn(RecordId) -> bool,
    ) -> Result<BackedUp, NotSaved> {
        out.write_all(self.head.as_bytes())?;
        let mut backed = BackedUp::default();
        let mut here = self.held.iter().peekable();
        for stored in stored {
            let (id, json) = match stored {
                Ok(record) => (record.id, Some(record.json)),
                Err(id) => (id, None),
            };
            while let Some(record) = here.next_if(|record| record.id < id) {
                if !deleted(record.id) {
                    put_held(out, record, &mut backed)?;
                }
            }
            let held = here.next_if(|record| record.id == id);
            match (json.filter(|json| is_json(json)), held) {
                (Some(json), _) => put(out, &json, &mut backed)?,
                // What the app holds of it, where the store's cannot be read.
                (None, Some(record)) => put_held(out, record, &mut backed)?,
                (None, None) if self.every || self.unread.contains(&id) => backed.left_out += 1,
                (None, None) => return Err(NotSaved::Unread),
            }
        }
        for record in here {
            if !deleted(record.id) {
                put_held(out, record, &mut backed)?;
            }
        }
        out.write_all(b"]}")?;
        Ok(backed)
    }

    /// Write the backup to `out` for a store whose history could not be
    /// listed: of every session here, where it holds them, those stored
    /// that this build could not read counted as left out; or else nothing,
    /// and every session here is asked for ([`NotSaved::Unread`]).
    pub fn write_unlisted(&self, out: &mut dyn Write) -> Result<BackedUp, NotSaved> {
        if !self.every {
            return Err(NotSaved::Unread);
        }
        out.write_all(self.head.as_bytes())?;
        let mut backed = BackedUp {
            left_out: self.unread.len(),
            ..BackedUp::default()
        };
        for record in &self.held {
            put_held(out, record, &mut backed)?;
        }
        out.write_all(b"]}")?;
        Ok(backed)
    }

    /// The whole backup, with `stored`, every record the store holds, or
    /// none where it could not list them, as a string: for a store that
    /// hands a file over whole, as a browser does.
    #[cfg_attr(not(any(feature = "web", test)), allow(dead_code))]
    pub fn whole(
        &self,
        stored: Option<Vec<Record>>,
        deleted: impl Fn(RecordId) -> bool,
    ) -> Result<(String, BackedUp), NotSaved> {
        let mut out = Vec::new();
        let backed = match stored {
            Some(stored) => self.write(&mut out, stored.into_iter().map(Ok), deleted)?,
            None => self.write_unlisted(&mut out)?,
        };
        Ok((String::from_utf8(out).unwrap_or_default(), backed))
    }
}

/// Whether `json`, a session as it is stored, is JSON at all.
fn is_json(json: &str) -> bool {
    serde_json::from_str::<serde::de::IgnoredAny>(json).is_ok()
}

/// Put `json`, a session in JSON, into the backup on its way to `out`,
/// after the sessions before it.
fn put(out: &mut dyn Write, json: &str, backed: &mut BackedUp) -> io::Result<()> {
    if backed.sessions > 0 {
        out.write_all(b",")?;
    }
    out.write_all(json.as_bytes())?;
    backed.sessions += 1;
    Ok(())
}

/// Put `record`, a session here, into the backup on its way to `out`: if
/// it is JSON at all, as every session this build writes is.
fn put_held(out: &mut dyn Write, record: &Record, backed: &mut BackedUp) -> io::Result<()> {
    if !is_json(&record.json) {
        backed.left_out += 1;
        return Ok(());
    }
    put(out, &record.json, backed)
}

impl BackedUp {
    /// What to say of a backup saved, after where it was saved, when there
    /// is anything to say: the sessions it could not hold.
    pub fn told(&self) -> Option<String> {
        match self.left_out {
            0 => None,
            1 => {
                Some("1 session stored here could not be read, and could not go in it.".to_string())
            }
            count => Some(format!(
                "{count} sessions stored here could not be read, and could not go in it."
            )),
        }
    }
}

/// Why a file could not be restored from, in words that follow its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotRestored {
    /// It is not a backup: some other file, or not JSON at all.
    NotABackup,
    /// It says it is one, but its sessions cannot be read.
    Unreadable,
    /// A history on its own, none of whose sessions this version reads.
    NoSessions,
}

impl NotRestored {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::NotABackup => "is not a Dust backup",
            Self::Unreadable => "is a Dust backup whose sessions cannot be read",
            Self::NoSessions => "holds no session this version of Dust can read",
        }
    }
}

/// A backup file as it is read: only what tells it is one, and each part
/// left as it lies in the file until it is read on its own. Even the mark
/// and the date are read only as whole numbers ([`whole_number`]): a list
/// of a gigabyte in their place was built up whole first, many times over.
#[derive(Deserialize)]
struct OnDisk<'a> {
    #[serde(rename = "dustBackup", default, borrow)]
    mark: Option<&'a RawValue>,
    #[serde(rename = "savedAt", default, borrow)]
    saved_at: Option<&'a RawValue>,
    /// Whatever it holds, `null` too: a backup with settings of any kind
    /// takes them, as read ([`settings_in`]).
    #[serde(default, borrow, deserialize_with = "present")]
    settings: Option<&'a RawValue>,
    #[serde(default, borrow)]
    sessions: Option<&'a RawValue>,
}

/// A part of a backup that is there, whatever it holds, `null` too.
fn present<'de, D>(deserializer: D) -> Result<Option<&'de RawValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <&RawValue>::deserialize(deserializer).map(Some)
}

/// Read a backup file: a backup Dust saved, or a history on its own, a
/// list of sessions as the store keeps them.
///
/// A history on its own is what the store sets aside when it cannot read
/// all of it ([`super::SetAside`]): restored once it is mended, or in a
/// later version that reads what this one could not, it puts its sessions
/// back. It holds no settings, and none are taken from it.
///
/// Only the sessions are held as they are read: each is read from where it
/// lies in the file, and one that does not read is left out alone. No part
/// is read whole that is far longer than its kind ever is: settings longer
/// than [`SETTINGS_MAX_BYTES`], or a session longer than
/// [`SESSION_MAX_BYTES`]. And what the sessions read take in memory is
/// held to bounds in all ([`each_session`]): what this build reads of them
/// to [`TYPED_PER_BYTE`] times the length of their list, or
/// [`TYPED_FLOOR`] for a short one; and beside it, of what is held as it
/// was read, to be written back with them, whatever it holds, what they
/// hold that this build does not read to [`UNREAD_MAX_BYTES`], and the
/// logs of their contest runs to [`RUN_LOGS_MAX_BYTES`]. So whatever a
/// file holds, its sessions take at most about three times its length in
/// memory, or sixteen megabytes for a short one, and some five hundred and
/// fifty megabytes beside.
pub fn read_backup(raw: &str) -> Result<Backup, NotRestored> {
    // A mark of the encoding, as an editor saving a file may put first, is
    // no part of what it holds.
    let json = raw.trim_start_matches(['\u{feff}', ' ', '\t', '\n', '\r']);
    if json.starts_with('[') {
        let (sessions, left_out) = each_session(json).ok_or(NotRestored::NotABackup)?;
        if sessions.is_empty() {
            return Err(NotRestored::NoSessions);
        }
        return Ok(Backup {
            saved_at: 0,
            settings: None,
            sessions,
            left_out,
        });
    }
    if !json.starts_with('{') {
        return Err(NotRestored::NotABackup);
    }
    let file: OnDisk = serde_json::from_str(json).map_err(|_| NotRestored::NotABackup)?;
    if whole_number(file.mark).is_none() {
        return Err(NotRestored::NotABackup);
    }
    let settings = file.settings.map(|settings| settings_in(settings.get()));
    let (sessions, left_out) = file
        .sessions
        .and_then(|sessions| each_session(sessions.get()))
        .ok_or(NotRestored::Unreadable)?;
    Ok(Backup {
        saved_at: whole_number(file.saved_at).unwrap_or(0),
        settings,
        sessions,
        left_out,
    })
}

/// The whole number `raw`, a part of a backup as it lies in the file, is:
/// none where it is anything else. A part longer than the longest whole
/// number, twenty digits, is none without being read at all.
fn whole_number(raw: Option<&RawValue>) -> Option<u64> {
    let raw = raw?.get();
    if raw.len() > 20 {
        return None;
    }
    serde_json::from_str(raw).ok()
}

/// The settings `raw`, as they lie in a backup, read as the store reads its
/// own ([`load_settings_from`]), where they are an object no longer than
/// settings come to ([`SETTINGS_MAX_BYTES`]). Anything else is read as
/// settings that do not read, as the store reads them, without being read
/// at all: read whole, a list or an object of a gigabyte was built up many
/// times over.
fn settings_in(raw: &str) -> TrainingSettings {
    if raw.starts_with('{') && raw.len() <= SETTINGS_MAX_BYTES {
        load_settings_from(raw)
    } else {
        TrainingSettings::default().clamp()
    }
}

/// Every session of `raw`, a JSON list, that reads, each read from where
/// it lies as the list is read, and scored as sessions score today: one
/// that does not read is left out alone, and nothing is held of it, and so
/// is one far longer than any session ([`SESSION_MAX_BYTES`]), unread.
///
/// What a session holds that this build does not read is kept as it was
/// read, to be written back with it, and so is a contest run's log, any
/// JSON at all; and either takes many times as much memory as it does in
/// the file ([`held_bytes`]). A session that would take the memory kept of
/// the one past [`UNREAD_MAX_BYTES`], in all, or of the other past
/// [`RUN_LOGS_MAX_BYTES`], is left out too, rather than kept without it.
/// Bounded a session at a time, a file of sixty sessions each of sixteen
/// megabytes of keys no build reads kept a gigabyte of them; with logs not
/// counted at all, one of two hundred and fifty runs, each with a log of
/// nearly four megabytes, kept as much; and with both counted as JSON, a
/// small object, `{"a":0}`, counted seven bytes where it takes some seven
/// hundred, so a file of twenty megabytes of runs whose logs were lists of
/// them came to well over a gigabyte.
///
/// And the memory what this build reads of them takes, with the room the
/// list holds for more of them, is kept only up to [`TYPED_PER_BYTE`]
/// times the bytes of the list, or [`TYPED_FLOOR`] for a short one: a
/// session that would take it past that is left out too.
/// Sessions as Dust has stored them come to well under that
/// ([`typed_bytes`]), but a list of a gigabyte of bare timings,
/// `{"timeToCompleteMs":0}`, each twenty-three bytes and a hundred and
/// twenty-eight in memory, was read into some five and a half gigabytes.
///
/// None where `raw` is no list. Beside them, how many of its elements were
/// left out.
fn each_session(raw: &str) -> Option<(Vec<SessionResult>, usize)> {
    use serde::Deserializer as _;
    let typed_most = raw.len().saturating_mul(TYPED_PER_BYTE).max(TYPED_FLOOR);
    let mut list = serde_json::Deserializer::from_str(raw);
    let read = (&mut list)
        .deserialize_seq(EachSession { typed_most })
        .ok()?;
    list.end().ok()?;
    Some(read)
}

/// What reads a list of sessions an element at a time ([`each_session`]),
/// so that a list holds no more memory than the sessions in it take: a
/// file of a gigabyte of nothing but `0,` was a list of five hundred
/// million elements held at once, some eight gigabytes, and the app was
/// stopped for want of them. What they take of what this build reads is
/// kept to `typed_most` ([`typed_bytes`]), with the room the list holds
/// for sessions to come, as it doubles to hold more.
struct EachSession {
    typed_most: usize,
}

impl<'de> serde::de::Visitor<'de> for EachSession {
    type Value = (Vec<SessionResult>, usize);

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a list of sessions")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut list: A) -> Result<Self::Value, A::Error> {
        let (mut sessions, mut left_out) = (Vec::new(), 0);
        // The memory taken so far by what they hold that this build does
        // not read, by their runs' logs, and by what it reads.
        let (mut unread, mut logs, mut typed) = (0, 0, 0);
        while let Some(session) = list.next_element::<&RawValue>()? {
            let session = session.get();
            let read = (session.len() <= SESSION_MAX_BYTES)
                .then(|| serde_json::from_str::<SessionResult>(session).ok())
                .flatten();
            let read = read.map(|session| {
                let sizes = (
                    unread_bytes(&session),
                    log_bytes(&session),
                    typed_bytes(&session),
                );
                (sizes, session)
            });
            // The room the list holds for sessions to come once it holds
            // this one: grown to twice as many, where it is full.
            let grow = match sessions.len() == sessions.capacity() {
                true => sessions.capacity().max(4),
                false => 0,
            };
            let room = sessions.capacity() + grow - sessions.len() - 1;
            let room = room * std::mem::size_of::<SessionResult>();
            match read {
                Some(((more, log, size), session))
                    if unread + more <= UNREAD_MAX_BYTES
                        && logs + log <= RUN_LOGS_MAX_BYTES
                        && typed + size + room <= self.typed_most =>
                {
                    unread += more;
                    logs += log;
                    typed += size;
                    sessions.reserve_exact(grow);
                    sessions.push(scored(session));
                }
                _ => left_out += 1,
            }
        }
        Ok((sessions, left_out))
    }
}

/// About how much memory what this build reads of `session` takes: all of
/// it but what it holds that this build does not read ([`unread_bytes`])
/// and a run's log ([`log_bytes`]), bounded apart. Each part counts as
/// large as its type, with the room it holds for its lists and text, and
/// an entry of the tally as [`TALLY_ENTRY_BYTES`]; and so do the places
/// it keeps, one for each group, timing or group's times, for what each
/// holds that this build does not read, wherever any of them holds some.
fn typed_bytes(session: &SessionResult) -> usize {
    use std::mem::size_of;
    let groups: usize = session
        .groups
        .iter()
        .map(|group| group.sent.capacity() + group.received.capacity())
        .sum();
    let timings: usize = session
        .group_timings
        .iter()
        .filter_map(|timing| timing.char_times.as_ref())
        .map(|times| {
            times.ends.capacity() * size_of::<u32>()
                + times.keys.capacity() * size_of::<Option<u32>>()
                + times.corrected.capacity() * size_of::<usize>()
        })
        .sum();
    let unread = &session.unread;
    let places =
        unread.groups.capacity() + unread.timings.capacity() + unread.char_times.capacity();
    let tally = &session.tally;
    size_of::<SessionResult>()
        + session.date.capacity()
        + session.alphabet_fingerprint.capacity()
        + session.groups.capacity() * size_of::<cw_core::GroupResult>()
        + groups
        + session.group_timings.capacity() * size_of::<cw_core::SessionTiming>()
        + timings
        + (tally.sent.len() + tally.inserted.len()) * TALLY_ENTRY_BYTES
        + places * size_of::<serde_json::Map<String, serde_json::Value>>()
}

/// How much memory an entry of a session's tally takes, a letter's and an
/// inserted character's alike: its key and its counts, and its share of
/// the map's nodes, generously.
const TALLY_ENTRY_BYTES: usize = 32;

/// About how much memory what `session` holds that this build does not
/// read takes, wherever in it it is held ([`cw_core::UnreadKeys`]): each
/// object it is kept in ([`object_bytes`]), and the nodes of the map that
/// keeps those of the tally's letters, by letter. None where it holds
/// nothing of the kind. Their places by group and by timing count with
/// what this build reads ([`typed_bytes`]).
fn unread_bytes(session: &SessionResult) -> usize {
    let unread = &session.unread;
    let kept: usize = std::iter::once(&unread.session)
        .chain(&unread.groups)
        .chain(&unread.timings)
        .chain(&unread.char_times)
        .chain(unread.letters.values())
        .map(object_bytes)
        .sum();
    kept + map_bytes::<char, serde_json::Map<String, serde_json::Value>>(unread.letters.len())
}

/// About how much memory the log of `session`, a contest run, takes
/// ([`held_bytes`]): none where it keeps none.
fn log_bytes(session: &SessionResult) -> usize {
    session.contest.as_ref().map_or(0, |log| held_bytes(&log.0))
}

/// About how much memory `value` takes as it is held ([`serde_json::Value`]),
/// beside its own place, which whatever holds it counts: a string its
/// text; a list its places, and what each holds; an object the nodes of
/// its map ([`map_bytes`]), its keys, and what each holds. Each block the
/// allocator hands out counts as it keeps it ([`block`]). Counted as JSON
/// instead, `{"a":0}` is seven bytes, where it takes some seven hundred,
/// most of them a node of the map that holds its one key.
fn held_bytes(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
        Value::String(text) => block(text.capacity()),
        Value::Array(list) => {
            block(list.capacity() * std::mem::size_of::<Value>())
                + list.iter().map(held_bytes).sum::<usize>()
        }
        Value::Object(object) => object_bytes(object),
    }
}

/// About how much memory the object `object` takes beside its own place
/// ([`held_bytes`]): none where it holds nothing.
fn object_bytes(object: &serde_json::Map<String, serde_json::Value>) -> usize {
    map_bytes::<String, serde_json::Value>(object.len())
        + object
            .iter()
            .map(|(key, value)| block(key.capacity()) + held_bytes(value))
            .sum::<usize>()
}

/// About how much memory the nodes of an ordered map of `entries` entries,
/// each a `K` and a `V`, take, as the standard library holds them: a node
/// has room for eleven, and holds as few as five once there are more than
/// eleven, so it counts one node up to eleven, and one more for every five
/// past the first.
fn map_bytes<K, V>(entries: usize) -> usize {
    use std::mem::size_of;
    let nodes = match entries {
        0 => 0,
        1..=11 => 1,
        _ => 1 + (entries - 1) / 5,
    };
    nodes * block(11 * (size_of::<K>() + size_of::<V>()) + 2 * size_of::<usize>())
}

/// About how much memory a block of `bytes` the allocator hands out takes:
/// two words more, for what it keeps of its own, and four words at least.
/// So a key of one letter counts as the thirty-two bytes it takes, not as
/// one.
fn block(bytes: usize) -> usize {
    let word = std::mem::size_of::<usize>();
    match bytes {
        0 => 0,
        _ => (bytes + 2 * word).max(4 * word),
    }
}

/// How Dust 0.16.2 read a backup, kept as it was: a backup this version
/// writes must still restore there. It counted none of the sessions it
/// left out.
#[cfg(test)]
pub fn read_backup_as_0_16_2(raw: &str) -> Result<Backup, NotRestored> {
    let file = match serde_json::from_str(raw) {
        Ok(serde_json::Value::Object(file)) => file,
        Ok(serde_json::Value::Array(_)) => {
            let sessions = parse_sessions(raw).unwrap_or_default();
            if sessions.is_empty() {
                return Err(NotRestored::NoSessions);
            }
            return Ok(Backup {
                saved_at: 0,
                settings: None,
                sessions,
                left_out: 0,
            });
        }
        _ => return Err(NotRestored::NotABackup),
    };
    if !file.get(MARK).is_some_and(serde_json::Value::is_u64) {
        return Err(NotRestored::NotABackup);
    }
    let settings = file
        .get("settings")
        .map(|settings| load_settings_from(&settings.to_string()));
    let sessions = file
        .get("sessions")
        .and_then(|sessions| parse_sessions(&sessions.to_string()))
        .ok_or(NotRestored::Unreadable)?;
    Ok(Backup {
        saved_at: file
            .get("savedAt")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        settings,
        sessions,
        left_out: 0,
    })
}

/// Read a backup file, `raw`, as [`read_backup`] reads it, on a thread of
/// its own where there is one to start: a backup of many thousand sessions
/// takes seconds to read, which the app's own thread does not wait on.
/// In a browser, and in a test, it is read then and there.
pub fn read_backup_apart(raw: String) -> Pending<Result<Backup, NotRestored>> {
    #[cfg(all(feature = "native-runtime", not(test)))]
    {
        read_on_a_thread(raw)
    }
    #[cfg(not(all(feature = "native-runtime", not(test))))]
    {
        super::record::done(read_backup(&raw))
    }
}

/// Read `raw` on a thread of its own, or, should none start, then and
/// there.
#[cfg(feature = "native-runtime")]
fn read_on_a_thread(raw: String) -> Pending<Result<Backup, NotRestored>> {
    let (give, taken) = std::sync::mpsc::channel::<String>();
    let (answer, answered) = tokio::sync::oneshot::channel();
    let reading = std::thread::Builder::new()
        .name("dust-restore".into())
        .spawn(move || {
            if let Ok(raw) = taken.recv() {
                let _ = answer.send(read_backup(&raw));
            }
        });
    if let Err(err) = reading {
        eprintln!("a backup is read on the app's own thread, as none other would start: {err}");
        return super::record::done(read_backup(&raw));
    }
    let _ = give.send(raw);
    // A thread that stopped short of an answer read nothing of it.
    Box::pin(async move { answered.await.unwrap_or(Err(NotRestored::Unreadable)) })
}

/// What a backup saved on `date` is called: "dust-backup-2026-10-19.json".
pub fn backup_name(date: &str) -> String {
    format!("dust-backup-{date}.json")
}

/// What a restore brings to the history ([`merge_sessions`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    /// The sessions of the backup not here yet, in the order they were
    /// practised.
    pub added: Vec<SessionResult>,
    /// The sessions here it fills in, in the order of the history.
    pub filled: Vec<Filled>,
}

/// A session a restore filled in: where it is in the history, and what it
/// was and is now.
#[derive(Clone, Debug, PartialEq)]
pub struct Filled {
    pub at: usize,
    pub was: SessionResult,
    pub now: SessionResult,
}

/// What `restored`, a backup's sessions, brings to `current`, the history:
/// every session not already here, and every session here that it fills
/// in. A session is the same session if it finished at the same moment on
/// the same day ([`SessionKey`]). A contest run whose log an older version
/// dropped is filled in from a copy that kept it, and so are the
/// per-character records ([`cw_core::CharTimes`]) of the same groups.
///
/// It takes time in proportion to the two together, and copies of the
/// history only what it fills in: a restore of thousands of sessions into
/// thousands more is a moment's work. Where each goes, the app works out
/// from when it finished ([`super::record::RecordId::placed`]).
pub fn merge_sessions(current: &[SessionResult], restored: Vec<SessionResult>) -> Merged {
    let mut here: HashMap<SessionKey, usize> = HashMap::with_capacity(current.len());
    for (at, session) in current.iter().enumerate() {
        here.entry(SessionKey::of(session)).or_insert(at);
    }
    let mut added: Vec<SessionResult> = Vec::new();
    let mut new: HashMap<SessionKey, usize> = HashMap::new();
    // Each session here filled in, as it is now, by where it is.
    let mut filled: BTreeMap<usize, SessionResult> = BTreeMap::new();
    for session in restored {
        let key = SessionKey::of(&session);
        if let Some(&at) = here.get(&key) {
            // One filled in twice is filled in from what it is by then.
            let kept = filled.get(&at).unwrap_or(&current[at]);
            if let Some(now) = filled_from(kept, session) {
                filled.insert(at, now);
            }
        } else if let Some(&at) = new.get(&key) {
            // One the backup holds twice is added once, as filled in.
            if let Some(now) = filled_from(&added[at], session) {
                added[at] = now;
            }
        } else {
            new.insert(key, added.len());
            added.push(session);
        }
    }
    added.sort_by_key(|session| session.timestamp);
    Merged {
        added,
        filled: filled
            .into_iter()
            .map(|(at, now)| Filled {
                at,
                was: current[at].clone(),
                now,
            })
            .collect(),
    }
}

/// `kept` filled in from `restored`, the same session, where `restored`
/// holds what an older version dropped of it: none, where it holds nothing
/// `kept` has not. Times of a group `kept` holds in a shape this version
/// cannot read, a later version's, give way to those it takes, which it
/// can: only those are stored ([`cw_core::UnreadKeys`]). Times it takes
/// come with what they hold that this version does not read.
fn filled_from(kept: &SessionResult, restored: SessionResult) -> Option<SessionResult> {
    if kept.contest.is_none() && restored.contest.is_some() {
        return Some(with_log(kept, restored));
    }
    if !dropped_char_times(kept, &restored) {
        return None;
    }
    let mut now = kept.clone();
    let places = now.group_timings.len();
    let mut restored_inside = restored.unread.char_times;
    let (unread, inside) = (&mut now.unread.timings, &mut now.unread.char_times);
    let timings = now.group_timings.iter_mut().zip(restored.group_timings);
    for (at, (timing, restored)) in timings.enumerate() {
        if timing.char_times.is_none() && restored.char_times.is_some() {
            timing.char_times = restored.char_times;
            if let Some(unread) = unread.get_mut(at) {
                unread.remove("charTimes");
            }
            if let Some(keys) = restored_inside.get_mut(at).filter(|keys| !keys.is_empty()) {
                inside.resize_with(places, serde_json::Map::new);
                inside[at] = std::mem::take(keys);
            }
        }
    }
    for by_place in [unread, inside] {
        if by_place.iter().all(serde_json::Map::is_empty) {
            by_place.clear();
        }
    }
    Some(now)
}

/// The keys under which a group's timing says what became of its times
/// ([`cw_core::SessionTiming::holds`]): the times, or why they went
/// untimed.
const TIMES_SAID: [&str; 2] = ["charTimes", "untimed"];

/// `restored`, a copy of `kept` that kept the log `kept` lost, with what
/// `kept` holds that it does not: what `kept` holds that this version does
/// not read, wherever in it, a key at a time; and, where the two hold the
/// same groups, what `kept` says of the times of a group that `restored`
/// says nothing of the times of ([`TIMES_SAID`]), in a shape this version
/// reads or in one it does not, kept as it was: only there, so the two
/// never say different things of a group's times side by side. Nothing
/// `restored` holds changes. A version that drops the log drops every key
/// it does not read, and has no times the run did not, so for what every
/// version so far stored this is `restored` as it is.
fn with_log(kept: &SessionResult, mut restored: SessionResult) -> SessionResult {
    let (here, now) = (&kept.unread, &mut restored.unread);
    take_missing(&mut now.session, &here.session);
    for (letter, keys) in &here.letters {
        if !keys.is_empty() && restored.tally.sent.contains_key(letter) {
            take_missing(now.letters.entry(*letter).or_default(), keys);
        }
    }
    let alike =
        kept.groups == restored.groups && kept.group_timings.len() == restored.group_timings.len();
    if !alike {
        return restored;
    }
    let places = kept.groups.len();
    for (at, keys) in here.groups.iter().enumerate() {
        take_missing_at(&mut now.groups, at, places, keys);
    }
    let places = kept.group_timings.len();
    let timings = restored.group_timings.iter_mut().zip(&kept.group_timings);
    for (at, (timing, held)) in timings.enumerate() {
        // Whether the copy says anything of the group's times, in a shape
        // this version reads or kept as it was.
        let says = TIMES_SAID.iter().any(|key| timing.holds(key))
            || now
                .timings
                .get(at)
                .is_some_and(|raw| TIMES_SAID.iter().any(|key| raw.contains_key(*key)));
        if !says {
            timing.char_times.clone_from(&held.char_times);
        }
        if let Some(keys) = here.timings.get(at) {
            let keys = keys.iter().filter(|(key, _)| {
                !timing.holds(key) && !(says && TIMES_SAID.contains(&key.as_str()))
            });
            take_missing_at(&mut now.timings, at, places, keys);
        }
        if let Some(keys) = here
            .char_times
            .get(at)
            .filter(|_| timing.char_times.is_some())
        {
            take_missing_at(&mut now.char_times, at, places, keys);
        }
    }
    restored
}

/// Each of `keys` that `into` holds none of, put into it: a key both hold
/// keeps the value `into` holds.
fn take_missing<'a>(
    into: &mut serde_json::Map<String, serde_json::Value>,
    keys: impl IntoIterator<Item = (&'a String, &'a serde_json::Value)>,
) {
    for (key, value) in keys {
        if !into.contains_key(key) {
            into.insert(key.clone(), value.clone());
        }
    }
}

/// The same, into what the part at `at` holds that this version does not
/// read, of `places` parts kept by place ([`cw_core::UnreadKeys`]): one map
/// a part once any holds a key, none at all while none does.
fn take_missing_at<'a>(
    by_place: &mut Vec<serde_json::Map<String, serde_json::Value>>,
    at: usize,
    places: usize,
    keys: impl IntoIterator<Item = (&'a String, &'a serde_json::Value)>,
) {
    for (key, value) in keys {
        if by_place.get(at).is_some_and(|held| held.contains_key(key)) {
            continue;
        }
        if by_place.len() < places {
            by_place.resize_with(places, serde_json::Map::new);
        }
        if let Some(held) = by_place.get_mut(at) {
            held.insert(key.clone(), value.clone());
        }
    }
}

/// Whether `one` and `other` are one session, the one filled in from the
/// other as a restore fills a session in ([`merge_sessions`]), whichever of
/// them it was: what a copy of the app holds of a session another copy
/// filled in since, or put back as it was by undoing the restore.
pub fn filled_in(one: &SessionResult, other: &SessionResult) -> bool {
    let fills = |kept: &SessionResult, restored: &SessionResult| {
        (kept.contest.is_none() && restored.contest.is_some()) || dropped_char_times(kept, restored)
    };
    SessionKey::of(one) == SessionKey::of(other) && (fills(one, other) || fills(other, one))
}

/// Whether `restored`, the same groups as `kept`, holds per-character
/// records `kept` has not: an older version drops them.
fn dropped_char_times(kept: &SessionResult, restored: &SessionResult) -> bool {
    kept.groups == restored.groups
        && kept.group_timings.len() == restored.group_timings.len()
        && kept
            .group_timings
            .iter()
            .zip(&restored.group_timings)
            .any(|(kept, restored)| kept.char_times.is_none() && restored.char_times.is_some())
}

/// How many of `sessions` finished after the backup saved at `last`: all of
/// them, when there has been none.
pub fn sessions_since(sessions: &[SessionResult], last: Option<u64>) -> usize {
    match last {
        Some(at) => sessions
            .iter()
            .filter(|session| session.timestamp > at)
            .count(),
        None => sessions.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::StoredHistory;
    use crate::persist::record::{read_record, records_for_blob};
    use crate::persist::tests::session;

    fn at(timestamp: u64, date: &str) -> SessionResult {
        SessionResult {
            timestamp,
            ..session(date)
        }
    }

    /// The history `current` comes to once `merged` is restored into it,
    /// as the app restores it: each session added where it finished, each
    /// filled in in its place.
    fn restored(current: &[SessionResult], merged: Merged) -> Vec<SessionResult> {
        let mut history = StoredHistory::from(current.to_vec());
        let filled: Vec<(RecordId, SessionResult)> = merged
            .filled
            .into_iter()
            .map(|fill| (history.id_at(fill.at).expect("here"), fill.now))
            .collect();
        for (was, now) in filled {
            history.remove(was);
            let json = serde_json::to_string(&now).expect("writes");
            history.insert(RecordId::beside(was, &json), now);
        }
        for session in merged.added {
            let json = serde_json::to_string(&session).expect("writes");
            history.insert(RecordId::placed(session.timestamp, &json), session);
        }
        history.to_vec()
    }

    fn timestamps(sessions: &[SessionResult]) -> Vec<u64> {
        sessions.iter().map(|session| session.timestamp).collect()
    }

    #[test]
    fn a_backup_reads_back_what_it_was_saved_with() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 14;
        settings.playback.char_wpm_min = 22.0;
        let sessions = vec![at(1, "2026-10-01"), at(2, "2026-10-02")];
        let raw = write_backup(&settings, &sessions, 1_790_000_000_000);
        let backup = read_backup(&raw).expect("a backup");
        assert_eq!(backup.saved_at, 1_790_000_000_000);
        assert_eq!(backup.settings, Some(settings.clamp()));
        assert_eq!(backup.sessions.len(), 2);
        assert_eq!(backup.sessions[1].date, "2026-10-02");
    }

    #[test]
    fn only_a_backup_is_read_as_one() {
        assert_eq!(read_backup("not json"), Err(NotRestored::NotABackup));
        assert_eq!(read_backup("7"), Err(NotRestored::NotABackup));
        assert_eq!(read_backup("[]"), Err(NotRestored::NoSessions));
        assert_eq!(
            read_backup(r#"[{"date":"broken"}]"#),
            Err(NotRestored::NoSessions)
        );
        // A settings file is JSON, but not a backup.
        let settings = serde_json::to_string(&TrainingSettings::default()).unwrap();
        assert_eq!(read_backup(&settings), Err(NotRestored::NotABackup));
        assert_eq!(
            read_backup(r#"{"dustBackup":1,"sessions":{}}"#),
            Err(NotRestored::Unreadable)
        );
        assert!(
            NotRestored::NotABackup
                .reason()
                .contains("not a Dust backup")
        );
    }

    /// One bad setting or session costs that one alone, as in the store.
    #[test]
    fn a_damaged_backup_gives_back_what_it_can() {
        let good = serde_json::to_string(&at(5, "2026-10-05")).unwrap();
        let raw = format!(
            r#"{{"dustBackup":1,"settings":{{"level":9,"groupTimeout":"nonsense"}},
                "sessions":[{good},{{"date":"broken"}}]}}"#
        );
        let backup = read_backup(&raw).expect("a backup");
        assert_eq!(backup.settings.map(|s| s.curriculum.level), Some(9));
        assert_eq!(backup.sessions.len(), 1);
        assert_eq!(backup.saved_at, 0, "it does not say when");
        // A newer layout is still read for what it has.
        assert!(read_backup(r#"{"dustBackup":7,"sessions":[]}"#).is_ok());
    }

    /// A history on its own — the store's, or what it set aside as it could
    /// not read it, mended since or read by a later version — restores its
    /// sessions, and holds no settings to take. One the store set aside
    /// could not be restored before: it is no backup.
    #[test]
    fn a_history_on_its_own_restores_its_sessions() {
        let good = serde_json::to_string(&at(5, "2026-10-05")).unwrap();
        let raw = format!(r#"[{good},{{"date":"broken"}}]"#);
        let backup = read_backup(&raw).expect("a history");
        assert_eq!(backup.sessions.len(), 1);
        assert_eq!(backup.sessions[0].date, "2026-10-05");
        assert_eq!((backup.settings, backup.saved_at), (None, 0));
        // A backup with no settings in it takes none either.
        let backup = read_backup(r#"{"dustBackup":1,"sessions":[]}"#).expect("a backup");
        assert_eq!(backup.settings, None);
    }

    #[test]
    fn restoring_adds_only_the_sessions_not_already_here() {
        let current = vec![at(10, "2026-10-10"), at(30, "2026-10-30")];
        let restored = vec![
            at(10, "2026-10-10"),
            at(20, "2026-10-20"),
            at(30, "2026-10-30"),
            at(5, "2026-10-05"),
        ];
        let merged = merge_sessions(&current, restored.clone());
        assert_eq!(merged.added, [at(5, "2026-10-05"), at(20, "2026-10-20")]);
        assert!(merged.filled.is_empty());
        let merged = self::restored(&current, merged);
        assert_eq!(
            timestamps(&merged),
            [5, 10, 20, 30],
            "in the order they were practised"
        );
        assert_eq!(
            merge_sessions(&merged, restored),
            Merged {
                added: Vec::new(),
                filled: Vec::new()
            },
            "restoring twice does nothing"
        );
    }

    /// A restore puts each session it adds before the first here that
    /// finished after it, and leaves those here in the order they were
    /// stored: one a clock set back stamped stays last, rather than go to
    /// the front, the first the history lets go when it has no room.
    #[test]
    fn restoring_leaves_the_sessions_here_in_their_order() {
        let current = vec![at(10, "a"), at(20, "b"), at(5, "c")];
        let merged = merge_sessions(&current, vec![at(15, "d"), at(1, "e")]);
        assert_eq!(merged.added, [at(1, "e"), at(15, "d")]);
        assert_eq!(timestamps(&restored(&current, merged)), [1, 10, 15, 20, 5]);
    }

    /// A run whose log an older version dropped takes it back from a copy
    /// that kept it, where it sits; a run that has its log keeps it, and a
    /// copy without one takes nothing away.
    #[test]
    fn restoring_fills_in_a_runs_dropped_log() {
        let run = |timestamp: u64, log: bool| {
            let qso = [("K5XYZ", "214", "K5XYZ", "214")];
            let mut run = crate::testing::stored_run("2026-10-10", &qso);
            run.timestamp = timestamp;
            if !log {
                run.contest = None;
            }
            run
        };
        let current = vec![run(10, false), at(20, "2026-10-20"), run(30, true)];
        let backup = vec![run(10, true), run(30, false), run(40, true)];
        let merged = merge_sessions(&current, backup.clone());
        assert_eq!(merged.added, [run(40, true)]);
        assert_eq!(
            merged.filled,
            [Filled {
                at: 0,
                was: run(10, false),
                now: run(10, true)
            }],
            "where it is, as it was, and as it is now"
        );
        let merged = restored(&current, merged);
        assert_eq!(timestamps(&merged), [10, 20, 30, 40]);
        assert!(merged[0].contest.is_some(), "filled in");
        assert!(merged[2].contest.is_some(), "kept");
        assert_eq!(merged[0].char_set_mode, cw_core::CharSetMode::Contest);
        let again = merge_sessions(&merged, backup).filled;
        assert!(
            again.is_empty(),
            "a log already here is not filled in twice"
        );
        // A backup holding the run twice, its log dropped from the first:
        // added once, as it is filled in.
        let twice = merge_sessions(&[], vec![run(10, false), run(10, true)]);
        assert_eq!(twice.added, [run(10, true)]);
        assert!(twice.filled.is_empty());
    }

    /// A run filled in with the log a copy kept keeps what the run here
    /// held that the copy does not: a later version's keys, on the run, a
    /// group or inside a letter's entry of the tally, and the times of a
    /// group the copy says nothing of the times of, in whatever shape. What
    /// the copy holds stays as it is, its own value under a key both hold
    /// among it, and its word on a group's times. The copy was taken whole,
    /// and the rest lost from the history; then, where either said
    /// something of a group's times in a shape this version does not read,
    /// the run here's times went in beside the copy's word on them.
    #[test]
    fn restoring_a_runs_log_keeps_what_the_session_here_held_beside_it() {
        let run = |log: bool| {
            let qso = [("K5XYZ", "214", "K5XYZ", "214")];
            let mut run = crate::testing::stored_run("2026-10-10", &qso);
            run.timestamp = 10;
            if !log {
                run.contest = None;
            }
            run
        };
        let times = cw_core::CharTimes {
            ends: vec![100, 200, 300, 400, 500],
            keys: vec![Some(700); 5],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        let mut here = serde_json::to_value({
            let mut here = run(false);
            here.group_timings[1].char_times = Some(times.clone());
            here
        })
        .expect("writes");
        here["laterKey"] = serde_json::json!(1);
        here["bothKey"] = serde_json::json!("here");
        here["groups"][0]["laterGroupKey"] = serde_json::json!([2]);
        here["letterAccuracy"]["K"]["laterInside"] = serde_json::json!(3);
        here["groupTimings"][1]["charTimes"]["laterTimes"] = serde_json::json!(4);
        let here = read_record(&here.to_string()).expect("a later version's reads");
        let mut copy = serde_json::to_value(run(true)).expect("writes");
        copy["bothKey"] = serde_json::json!("copy");
        let copy = read_record(&copy.to_string()).expect("reads");

        let merged = merge_sessions(std::slice::from_ref(&here), vec![copy.clone()]);
        let [fill] = &merged.filled[..] else {
            panic!("filled in once: {merged:?}");
        };
        let mut both = serde_json::to_value(&copy).expect("writes");
        both["laterKey"] = serde_json::json!(1);
        both["groups"][0]["laterGroupKey"] = serde_json::json!([2]);
        both["letterAccuracy"]["K"]["laterInside"] = serde_json::json!(3);
        both["groupTimings"][1]["charTimes"] = serde_json::to_value(&times).expect("writes");
        both["groupTimings"][1]["charTimes"]["laterTimes"] = serde_json::json!(4);
        let json = serde_json::to_string(&fill.now).expect("writes");
        let now: serde_json::Value = serde_json::from_str(&json).expect("JSON");
        assert_eq!(now, both, "the copy, with what was here beside it");
        assert_eq!(
            read_record(&json).as_ref(),
            Some(&fill.now),
            "as it is held"
        );
        assert!(filled_in(&here, &fill.now), "one session");

        // Where the copy says anything of a group's times, the run here's
        // times of it stay out, whatever shape either says it in: a reason
        // it went untimed this version reads, or one it does not, beside
        // times this version reads, or ones it does not. Where the copy
        // says nothing, they come in, in whatever shape they were kept.
        let later_times = serde_json::json!({ "v2": [1, 2] });
        let qsos = [
            ("K5XYZ", "214", "K5XYZ", "214"),
            ("W1AW", "215", "W1AW", "215"),
        ];
        let run = |log: bool| {
            let mut run = crate::testing::stored_run("2026-10-10", &qsos);
            run.timestamp = 10;
            if !log {
                run.contest = None;
            }
            serde_json::to_value(run).expect("writes")
        };
        let mut here = run(false);
        here["groupTimings"][0]["charTimes"] = later_times.clone();
        here["groupTimings"][1]["charTimes"] = serde_json::to_value(&times).expect("writes");
        here["groupTimings"][2]["untimed"] = serde_json::json!("laterReason");
        here["groupTimings"][3]["charTimes"] = later_times.clone();
        here["groupTimings"][3]["untimed"] = serde_json::json!("laterReason");
        let here = read_record(&here.to_string()).expect("a later version's reads");
        let mut copy = run(true);
        copy["groupTimings"][0]["untimed"] = serde_json::json!("heldUp");
        copy["groupTimings"][1]["untimed"] = serde_json::json!("laterReason");
        copy["groupTimings"][2]["charTimes"] = serde_json::to_value(&times).expect("writes");
        let copy = read_record(&copy.to_string()).expect("a later version's reads");
        let merged = merge_sessions(std::slice::from_ref(&here), vec![copy.clone()]);
        let [fill] = &merged.filled[..] else {
            panic!("filled in once: {merged:?}");
        };
        let mut both = serde_json::to_value(&copy).expect("writes");
        both["groupTimings"][3]["charTimes"] = later_times;
        both["groupTimings"][3]["untimed"] = serde_json::json!("laterReason");
        let json = serde_json::to_string(&fill.now).expect("writes");
        let now: serde_json::Value = serde_json::from_str(&json).expect("JSON");
        for at in 0..4 {
            assert_eq!(
                now["groupTimings"][at], both["groupTimings"][at],
                "group {at}: the copy's word on its times stands"
            );
        }
        assert_eq!(
            now, both,
            "the copy, with the times only it says nothing of"
        );
    }

    /// A session whose per-character records an older version dropped takes
    /// them back from a copy that kept them; one that has them keeps its
    /// own.
    #[test]
    fn restoring_fills_in_dropped_character_times() {
        let times = |key: u32| cw_core::CharTimes {
            ends: vec![300],
            keys: vec![Some(key)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        let session = |kept: Option<u32>| {
            let mut session =
                crate::testing::stored_session("2026-10-10", &[("K", "K"), ("M", "M")]);
            session.timestamp = 10;
            session.group_timings[1].char_times = kept.map(times);
            session
        };
        let here = [session(None)];
        let merged = merge_sessions(&here, vec![session(Some(700))]);
        assert_eq!((merged.added.len(), merged.filled.len()), (0, 1));
        let merged = restored(&here, merged);
        assert_eq!(merged[0].group_timings[1].char_times, Some(times(700)));
        let again = merge_sessions(&merged, vec![session(Some(900))]);
        assert!(again.filled.is_empty(), "its own kept");
    }

    /// Times a restore fills in come with what a later version wrote inside
    /// them that this one does not read, in their place, and are stored with
    /// it. It was left behind, and lost from the history.
    #[test]
    fn times_a_restore_fills_in_come_with_what_they_hold_that_this_version_does_not_read() {
        let times = |key: u32| cw_core::CharTimes {
            ends: vec![300],
            keys: vec![Some(key)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        let session = |kept: Option<u32>| {
            let mut session =
                crate::testing::stored_session("2026-10-10", &[("K", "K"), ("M", "M")]);
            session.timestamp = 10;
            session.group_timings[1].char_times = kept.map(times);
            session
        };
        let mut later = serde_json::to_value(session(Some(700))).expect("writes");
        later["groupTimings"][1]["charTimes"]["laterInside"] = serde_json::json!({ "v": 1 });
        let backup = read_record(&later.to_string()).expect("a later version's reads");
        let merged = merge_sessions(&[session(None)], vec![backup]);
        let [fill] = &merged.filled[..] else {
            panic!("filled in once: {merged:?}");
        };
        assert_eq!(fill.now.group_timings[1].char_times, Some(times(700)));
        let json = serde_json::to_string(&fill.now).expect("writes");
        let stored: serde_json::Value = serde_json::from_str(&json).expect("JSON");
        assert_eq!(
            stored["groupTimings"][1]["charTimes"],
            later["groupTimings"][1]["charTimes"]
        );
        assert_eq!(
            read_record(&json).as_ref(),
            Some(&fill.now),
            "as it is held"
        );
    }

    /// A session whose times a later version wrote in a shape this one
    /// cannot read takes the times a backup holds of it that it can, and is
    /// stored as it is held: with those times, the others let go of. Both
    /// used to be written under one key, and the session filled in, stored
    /// in place of the one it fills in, read as no session at all, in any
    /// version.
    #[test]
    fn a_session_filled_in_over_times_it_could_not_read_reads_back() {
        let times = |key: u32| cw_core::CharTimes {
            ends: vec![300],
            keys: vec![Some(key)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        let session = |kept: Option<u32>| {
            let mut session =
                crate::testing::stored_session("2026-10-10", &[("K", "K"), ("M", "M")]);
            session.timestamp = 10;
            session.group_timings[1].char_times = kept.map(times);
            session
        };
        let mut later = serde_json::to_value(session(None)).expect("writes");
        later["groupTimings"][1]["charTimes"] = serde_json::json!({ "at": [1, 2] });
        let here = [read_record(&later.to_string()).expect("a later version's reads")];
        assert_eq!(here[0].group_timings[1].char_times, None);
        let merged = merge_sessions(&here, vec![session(Some(700))]);
        let [fill] = &merged.filled[..] else {
            panic!("filled in once: {merged:?}");
        };
        let was = RecordId::placed(10, "");
        let record = Record::of(&fill.now, |json| RecordId::beside(was, json)).expect("writes");
        let stored = read_record(&record.json).expect("reads back");
        assert_eq!(stored.group_timings[1].char_times, Some(times(700)));
        assert_eq!(stored, fill.now, "as it is held");
    }

    /// A session and itself filled in are one session, whichever of them is
    /// held and whichever found, as a restore filled it in or its Undo put
    /// it back as it was. Sessions that hold anything else apart, or
    /// finished at another moment, are two, and so are two alike in
    /// everything, which are told apart otherwise.
    #[test]
    fn a_session_and_itself_filled_in_are_one() {
        let run = |timestamp: u64, log: bool| {
            let qso = [("K5XYZ", "214", "K5XYZ", "214")];
            let mut run = crate::testing::stored_run("2026-10-10", &qso);
            run.timestamp = timestamp;
            if !log {
                run.contest = None;
            }
            run
        };
        assert!(filled_in(&run(10, false), &run(10, true)));
        assert!(filled_in(&run(10, true), &run(10, false)));
        assert!(
            !filled_in(&run(10, false), &run(20, true)),
            "another moment"
        );
        assert!(!filled_in(&run(10, true), &run(10, true)), "alike");
        let times = |key: u32| cw_core::CharTimes {
            ends: vec![300],
            keys: vec![Some(key)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        let session = |sent: &[(&str, &str)], kept: Option<u32>| {
            let mut session = crate::testing::stored_session("2026-10-10", sent);
            session.timestamp = 10;
            session.group_timings[0].char_times = kept.map(times);
            session
        };
        let both = [("K", "K"), ("M", "M")];
        assert!(filled_in(&session(&both, None), &session(&both, Some(700))));
        assert!(filled_in(&session(&both, Some(700)), &session(&both, None)));
        assert!(
            !filled_in(&session(&both, Some(700)), &session(&both, Some(900))),
            "each its own"
        );
        assert!(
            !filled_in(&session(&both[..1], None), &session(&both, Some(700))),
            "other groups"
        );
    }

    /// An assisted run, `contest:wpx:a`, read back from a backup is still a
    /// WPX run, its log dropped or not, and is filled in as any run is.
    #[test]
    fn an_assisted_run_is_restored_as_its_contest() {
        let qso = [("K5XYZ", "214", "K5XYZ", "214")];
        let kept = crate::testing::assisted(crate::testing::stored_run("2026-10-10", &qso));
        // As an older version writes it back: its mode Callsigns and its
        // log dropped, its mark kept.
        let mut older = serde_json::to_value(&kept).expect("a run writes");
        if let Some(run) = older.as_object_mut() {
            run.remove("contest");
        }
        let raw = format!(r#"{{"dustBackup":1,"savedAt":0,"sessions":[{older}]}}"#);
        let backup = read_backup(&raw).expect("a backup");
        let read = &backup.sessions[0];
        assert_eq!(read.alphabet_fingerprint, "contest:wpx:a");
        assert_eq!(read.char_set_mode, cw_core::CharSetMode::Contest);
        assert_eq!(read.contest_kind(), Some(cw_core::ContestKind::Wpx));
        assert!(read.assisted(), "by its mark");
        let merged = merge_sessions(&backup.sessions, vec![kept.clone()]);
        assert_eq!((merged.added.len(), merged.filled.len()), (0, 1));
        assert_eq!(merged.filled[0].now, kept);
    }

    #[test]
    fn sessions_since_a_backup_are_counted() {
        let sessions = vec![at(10, "a"), at(20, "b"), at(30, "c")];
        assert_eq!(sessions_since(&sessions, None), 3);
        assert_eq!(sessions_since(&sessions, Some(20)), 1);
        assert_eq!(sessions_since(&sessions, Some(99)), 0);
        assert_eq!(backup_name("2026-10-19"), "dust-backup-2026-10-19.json");
    }

    /// A backup read a session at a time, from where each lies in the file,
    /// reads as a backup read whole did, every case above among them: what
    /// is a backup, what is not, and what of a damaged one is kept. How
    /// many of its sessions were left out is set aside: 0.16.2 counted
    /// none.
    #[test]
    fn a_backup_read_where_it_lies_reads_as_one_read_whole() {
        let good = serde_json::to_string(&at(5, "2026-10-05")).unwrap();
        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 14;
        let run = crate::testing::stored_run("2026-10-10", &[("K5XYZ", "214", "K5XYZ", "214")]);
        let cases = [
            write_backup(&settings, &[at(1, "2026-10-01"), at(2, "2026-10-02")], 7),
            write_backup(&settings, &[], 7),
            write_backup(&settings, &[run], 0),
            "not json".into(),
            "7".into(),
            "[]".into(),
            "null".into(),
            r#"[{"date":"broken"}]"#.into(),
            serde_json::to_string(&TrainingSettings::default()).unwrap(),
            r#"{"dustBackup":1,"sessions":{}}"#.into(),
            r#"{"dustBackup":1,"sessions":null}"#.into(),
            r#"{"dustBackup":1}"#.into(),
            r#"{"dustBackup":"1","sessions":[]}"#.into(),
            r#"{"dustBackup":1.5,"sessions":[]}"#.into(),
            r#"{"dustBackup":1,"savedAt":"soon","settings":null,"sessions":[]}"#.into(),
            r#"{"dustBackup":1,"settings":[1],"sessions":[]}"#.into(),
            format!(
                r#"{{"dustBackup":1,"settings":{{"level":9,"groupTimeout":"nonsense"}},
                "sessions":[{good},{{"date":"broken"}}]}}"#
            ),
            r#"{"dustBackup":7,"sessions":[]}"#.into(),
            format!(r#"[{good},{{"date":"broken"}}]"#),
            format!("  \n[{good}]"),
            format!(r#"{{"dustBackup":1,"sessions":[{good}]}} trailing"#),
            // Lists of anything but sessions, long ones among them: read an
            // element at a time, and none of it held.
            format!("[{}]", ["0"; 100_000].join(",")),
            format!(r#"[0,{good},[],{{}},"x",null,{good}]"#),
            format!(
                r#"{{"dustBackup":1,"sessions":[{}]}}"#,
                ["0"; 100_000].join(",")
            ),
            format!(r#"{{"dustBackup":1,"sessions":[{good},0,{good}]}}"#),
            format!("[{good},0"),
            format!("[{good}]]"),
        ];
        for raw in &cases {
            assert_eq!(
                read_backup(raw).map(uncounted),
                read_backup_as_0_16_2(raw),
                "{raw}"
            );
        }
        // And a mark of the encoding first, as an editor saving it may put,
        // which used to make it no backup at all.
        let marked = format!("\u{feff}{}", cases[0]);
        assert_eq!(read_backup(&marked), read_backup(&cases[0]));
    }

    /// A part of a backup far larger than any backup's is never read
    /// whole: a mark or a date that is no number is none, whatever it
    /// holds; settings longer than any are read as settings that do not
    /// read; and a session longer than any is left out unread. Each was
    /// read whole first, and what it held built up many times over, so a
    /// file of a gigabyte of any of them stopped the app for want of
    /// memory. Deeper than a value is ever built up, a date made the file
    /// no backup at all.
    #[test]
    fn a_part_far_larger_than_any_backups_is_not_read_whole() {
        let good = serde_json::to_string(&at(5, "2026-10-05")).unwrap();
        let deep = format!("{}{}", "[".repeat(1_000), "]".repeat(1_000));
        let raw = format!(r#"{{"dustBackup":1,"savedAt":{deep},"sessions":[{good}]}}"#);
        let backup = read_backup(&raw).expect("a backup");
        assert_eq!((backup.saved_at, backup.sessions.len()), (0, 1));
        let raw = format!(r#"{{"dustBackup":{deep},"sessions":[{good}]}}"#);
        assert_eq!(read_backup(&raw), Err(NotRestored::NotABackup));
        let long = format!("[{}]", ["0"; 100_000].join(","));
        let raw = format!(r#"{{"dustBackup":1,"savedAt":{long},"sessions":[{good}]}}"#);
        assert_eq!(read_backup(&raw).expect("a backup").saved_at, 0);

        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 14;
        let mut long = serde_json::to_value(&settings).unwrap();
        long["later"] = "x".repeat(SETTINGS_MAX_BYTES).into();
        let raw = format!(r#"{{"dustBackup":1,"settings":{long},"sessions":[{good}]}}"#);
        assert_eq!(
            read_backup(&raw).expect("a backup").settings,
            Some(TrainingSettings::default().clamp())
        );

        let mut session = serde_json::to_value(at(6, "2026-10-06")).unwrap();
        session["later"] = "x".repeat(SESSION_MAX_BYTES).into();
        let raw = format!("[{good},{session}]");
        let backup = read_backup(&raw).expect("a history");
        let dates: Vec<&str> = backup.sessions.iter().map(|s| s.date.as_str()).collect();
        assert_eq!(dates, ["2026-10-05"]);
        assert_eq!(backup.left_out, 1);
    }

    /// `count` small objects, `{"a":0}`, in a list, as JSON: some seven
    /// hundred bytes of memory each, kept as they are read
    /// ([`held_bytes`]), for eight in the file.
    fn small_objects(count: usize) -> String {
        format!("[{}]", vec![r#"{"a":0}"#; count].join(","))
    }

    /// `session` as JSON, with `held`, JSON too, where it holds the text
    /// `"held"`: a large part put in as it is, rather than built up as a
    /// value only to be written out again.
    fn holding_in_place(session: &serde_json::Value, held: &str) -> String {
        let json = session.to_string();
        assert_eq!(json.matches(r#""held""#).count(), 1, "{json}");
        json.replacen(r#""held""#, held, 1)
    }

    /// A backup of sessions as `holding` makes them, the first on day 1,
    /// each holding what `counted` counts, as many as `most` holds and one
    /// more, then one that holds none, read: as many as the bound holds
    /// are read, in order, the one past them is left out, and the last is
    /// read all the same. What is read is given back, to be looked into.
    fn read_up_to_the_bound(
        holding: impl Fn(u64) -> String,
        counted: fn(&SessionResult) -> usize,
        most: usize,
    ) -> Backup {
        let each = counted(&serde_json::from_str(&holding(1)).expect("a session"));
        let fit = (most / each) as u64;
        assert!(fit >= 2, "each counts {each} of {most}");
        let mut list: Vec<String> = (1..=fit + 1).map(holding).collect();
        list.push(serde_json::to_string(&at(fit + 2, "2026-10-28")).unwrap());
        let backup = read_backup(&format!("[{}]", list.join(","))).expect("a history");
        let read: Vec<u64> = (1..=fit).chain([fit + 2]).collect();
        assert_eq!(timestamps(&backup.sessions), read, "as many as fit");
        assert_eq!(backup.left_out, 1);
        let kept: usize = backup.sessions.iter().map(counted).sum();
        assert!(kept <= most, "{kept} bytes kept, of {most}");
        backup
    }

    /// What the sessions of a backup hold that this build does not read is
    /// kept, to be written back with them, only up to a bound in all of the
    /// memory it takes: past it, a session that holds any is left out, as
    /// one too long to read is, and one that holds none is read all the
    /// same, whether a session holds it under a key of its own or under a
    /// key on each of its groups. And a session of five megabytes, far
    /// longer than any session or run comes to, is too long to read. It was
    /// counted as JSON, where a small object takes some ninety times as
    /// much memory as it does in the file: a file of some two million
    /// groups each holding `{"a":0}`, sixteen megabytes of them counted,
    /// held nearly two gigabytes.
    #[test]
    fn what_sessions_hold_that_this_build_does_not_read_is_kept_to_a_bound() {
        {
            let own = |day: u64| {
                let mut session =
                    serde_json::to_value(at(day, &format!("2026-10-{day:02}"))).unwrap();
                session["later"] = "held".into();
                holding_in_place(&session, &small_objects(60_000))
            };
            let backup = read_up_to_the_bound(own, unread_bytes, UNREAD_MAX_BYTES);
            let kept = backup.sessions[0].unread.session["later"].as_array();
            assert_eq!(kept.map(Vec::len), Some(60_000), "kept as it was");
        }
        {
            let group = r#"{"sent":"K","received":"K","correct":true,"a":0}"#;
            let groups = format!("[{}]", vec![group; 70_000].join(","));
            let on_each_group = |day: u64| {
                let mut session =
                    serde_json::to_value(at(day, &format!("2026-10-{day:02}"))).unwrap();
                session["groups"] = "held".into();
                holding_in_place(&session, &groups)
            };
            let backup = read_up_to_the_bound(on_each_group, unread_bytes, UNREAD_MAX_BYTES);
            let mut first = backup.sessions.into_iter().next().expect("read");
            assert_eq!(first.unread.groups.len(), 70_000, "kept as it was");
            let each = first.unread.groups.iter().all(|kept| kept["a"] == 0);
            assert!(each, "kept as it was");
            // Their places, one for each group, count with what this build
            // reads, as many as the session has groups.
            let typed = typed_bytes(&first);
            first.unread.groups = Vec::new();
            let place = std::mem::size_of::<serde_json::Map<String, serde_json::Value>>();
            assert_eq!(typed - typed_bytes(&first), 70_000 * place, "their places");
        }
        let holding = |day: u64, bytes: usize| {
            let mut session = serde_json::to_value(at(day, &format!("2026-10-{day:02}"))).unwrap();
            session["later"] = "x".repeat(bytes).into();
            session.to_string()
        };
        let raw = format!("[{},{}]", holding(8, 5_000_000), holding(9, 1_000));
        let backup = read_backup(&raw).expect("a history");
        let days: Vec<u64> = backup.sessions.iter().map(|s| s.timestamp).collect();
        assert_eq!((days, backup.left_out), (vec![9], 1));
    }

    /// What this build reads of the sessions in a backup is kept only up to
    /// a bound in all, in proportion to the file: past it, a session is
    /// left out and told of, as one too long to read is. Bounded a session
    /// at a time, a file of sessions holding nothing but bare timings,
    /// `{"timeToCompleteMs":0}`, some twenty-three bytes each and five and
    /// a half times as much in memory, was read whole: a gigabyte of them
    /// came to some five and a half gigabytes.
    #[test]
    fn a_backup_of_bare_timings_is_read_only_to_a_budget_in_all() {
        let bare = |day: u64| {
            let mut session = serde_json::to_value(at(day, "2026-10-10")).unwrap();
            let timing = serde_json::json!({ "timeToCompleteMs": 0 });
            session["groupTimings"] = serde_json::Value::Array(vec![timing; 20_000]);
            session.to_string()
        };
        let list: Vec<String> = (1..=40).map(bare).collect();
        let raw = format!("[{}]", list.join(","));
        assert!(
            raw.len() * TYPED_PER_BYTE > TYPED_FLOOR,
            "bound by the file"
        );
        let backup = read_backup(&raw).expect("a history");
        let kept = backup.sessions.len();
        let held = kept * 20_000 * std::mem::size_of::<cw_core::SessionTiming>();
        assert!(
            held <= raw.len() * TYPED_PER_BYTE,
            "{kept} of 40 read: {held} bytes of timings from a file of {}",
            raw.len()
        );
        assert!(kept > 0, "as many as the bound holds");
        let first: Vec<u64> = (1..=kept as u64).collect();
        assert_eq!(timestamps(&backup.sessions), first, "the first, in order");
        assert_eq!(backup.left_out, 40 - kept);
        assert_eq!(
            Backup::told_left_out(backup.left_out),
            Some(format!(
                "{} sessions in it could not be read by this version and were left out.",
                40 - kept
            ))
        );
    }

    /// The room the list of a backup's sessions holds for sessions to come,
    /// as it grows, counts to the same bound as what this build reads of
    /// them ([`TYPED_PER_BYTE`]). The list doubles as it grows, and its room
    /// was not counted: a file of the smallest sessions there are, three
    /// times its length of them, was held in nearly five.
    #[test]
    fn the_room_the_list_of_sessions_grows_into_counts_to_the_bound() {
        let least = r#"{"date":"","timestamp":0,"groups":[],"groupTimings":[],"accuracy":0,"avgResponseMs":0,"totalChars":0}"#;
        let raw = format!("[{}]", vec![least; 200_000].join(","));
        let backup = read_backup(&raw).expect("a history");
        assert!(backup.left_out > 0, "bound by the file");
        let typed: usize = backup.sessions.iter().map(typed_bytes).sum();
        let room = backup.sessions.capacity() - backup.sessions.len();
        let room = room * std::mem::size_of::<SessionResult>();
        let most = raw.len() * TYPED_PER_BYTE;
        assert!(
            typed + room <= most,
            "{typed} bytes of sessions and {room} of room, of {most}"
        );
    }

    /// Histories as Dust has stored them take well under the memory a
    /// backup's sessions are read to ([`TYPED_PER_BYTE`]), so a backup of
    /// any of them restores whole, however long: each shape is tried on its
    /// own. Sessions of twenty groups of five, timed character by character
    /// as this build keeps them; the same untimed, as older builds, or audio
    /// that could not be timed, keep them; and the shape of the first
    /// builds, with keys no build reads any more, the densest, with groups
    /// of five and of one, and of one left unanswered, the densest of all,
    /// at some two and a quarter a byte.
    #[test]
    fn a_real_history_comes_to_well_under_the_bound_its_backup_is_read_to() {
        let session = |at: u64, timed: bool| {
            let mut session = crate::testing::stored_session("2026-10-10", &[]);
            session.timestamp = 1_700_000_000_000 + at * 600_000;
            session.groups.clear();
            session.group_timings.clear();
            session.tally = cw_core::CopyTally::default();
            for group in 0..20_u64 {
                let sent: String = (0..5)
                    .map(|place| char::from(b'A' + ((at * 7 + group * 5 + place) % 26) as u8))
                    .collect();
                session.tally.add(&sent, &sent);
                session.groups.push(cw_core::GroupResult {
                    sent: sent.clone(),
                    received: sent,
                    correct: true,
                });
                let ends: Vec<u32> = (1..=5).map(|end| end * 260).collect();
                session.group_timings.push(cw_core::SessionTiming {
                    time_to_complete_ms: 1_234.5,
                    char_wpm: Some(25.0),
                    effective_wpm: Some(18.0),
                    sends: 1,
                    char_times: timed.then(|| cw_core::CharTimes {
                        keys: ends.iter().map(|end| Some(end + 410)).collect(),
                        ends,
                        corrected: Vec::new(),
                        latency_ms: 40,
                        box_open: true,
                    }),
                    untimed: None,
                });
            }
            serde_json::to_string(&session).unwrap()
        };
        // Each group answered as sent, or left unanswered: nothing typed,
        // nothing timed, nothing scored.
        let first = |at: u64, sent: &str, answered: bool| {
            let (received, time, mean, right) = match answered {
                true => (sent, "3456", "691.2", 1),
                false => ("", "0", "0", 0),
            };
            let group =
                format!(r#"{{"sent":"{sent}","received":"{received}","correct":{answered}}}"#);
            let timing = format!(r#"{{"timeToCompleteMs":{time},"perCharMs":{mean}}}"#);
            let letters: Vec<String> = sent
                .chars()
                .map(|letter| format!(r#""{letter}":{{"correct":{},"total":20}}"#, right * 20))
                .collect();
            format!(
                r#"{{"date":"2026-01-01","timestamp":{at},"startedAt":0,"finishedAt":{at},"groups":[{}],"groupTimings":[{}],"accuracy":{right},"letterAccuracy":{{{}}},"alphabetSize":5,"avgResponseMs":{mean},"totalChars":100,"effectiveAlphabetSize":5,"score":{},"kochLevel":3}}"#,
                [group.as_str(); 20].join(","),
                [timing.as_str(); 20].join(","),
                letters.join(","),
                right * 100
            )
        };
        let shapes: [(&str, Vec<String>); 5] = [
            ("timed", (0..500).map(|at| session(at, true)).collect()),
            ("untimed", (0..500).map(|at| session(at, false)).collect()),
            (
                "first",
                (0..500).map(|at| first(at, "KMRSU", true)).collect(),
            ),
            (
                "first, of one",
                (0..500).map(|at| first(at, "K", true)).collect(),
            ),
            (
                "first, of one, unanswered",
                (0..500).map(|at| first(at, "K", false)).collect(),
            ),
        ];
        for (shape, sessions) in shapes {
            let raw = format!("[{}]", sessions.join(","));
            let backup = read_backup(&raw).expect("a history");
            assert_eq!(
                (backup.sessions.len(), backup.left_out),
                (500, 0),
                "{shape}"
            );
            let typed: usize = backup.sessions.iter().map(typed_bytes).sum();
            let per_byte = typed as f64 / raw.len() as f64;
            // A quarter of the bound and more to spare.
            let most = TYPED_PER_BYTE as f64 * 0.75;
            assert!(per_byte <= most, "{shape}: {per_byte:.2} a byte");
        }
    }

    /// What sessions hold that this build does not read inside a group's
    /// times, or inside a letter's entry of the tally, is kept as it was
    /// read, as what they hold beside them is, and counts to the same bound
    /// in all: uncounted, a file of sessions holding it there would keep a
    /// gigabyte of it.
    #[test]
    fn what_sessions_hold_inside_their_times_and_tally_counts_to_the_bound() {
        for inside in ["charTimes", "letterAccuracy"] {
            let holding = |day: u64| {
                let mut session =
                    crate::testing::stored_session(&format!("2026-10-{day:02}"), &[("K", "K")]);
                session.timestamp = day;
                session.group_timings[0].char_times = Some(cw_core::CharTimes {
                    ends: vec![300],
                    keys: vec![Some(700)],
                    corrected: Vec::new(),
                    latency_ms: 0,
                    box_open: true,
                });
                let mut session = serde_json::to_value(session).unwrap();
                let place = if inside == "charTimes" {
                    &mut session["groupTimings"][0]["charTimes"]
                } else {
                    &mut session["letterAccuracy"]["K"]
                };
                place["later"] = "held".into();
                holding_in_place(&session, &small_objects(60_000))
            };
            let backup = read_up_to_the_bound(holding, unread_bytes, UNREAD_MAX_BYTES);
            let kept = &backup.sessions[0].unread;
            let kept = if inside == "charTimes" {
                kept.char_times[0]["later"].as_array()
            } else {
                kept.letters[&'K']["later"].as_array()
            };
            assert_eq!(kept.map(Vec::len), Some(60_000), "{inside}: kept as it was");
        }
    }

    /// The logs of the runs in a backup are kept, each whole as it was
    /// read, only up to a bound in all of the memory they take: past it, a
    /// run is left out, as a session too long to read is, and a session
    /// with no log is read all the same. A log is any JSON, and was counted
    /// as JSON, where a small object takes some ninety times as much memory
    /// as it does in the file: a file of runs whose logs were lists of
    /// `{"a":0}`, sixteen megabytes of them counted, held well over a
    /// gigabyte.
    #[test]
    fn the_logs_of_the_runs_in_a_backup_are_kept_to_a_bound() {
        let run = |day: u64| {
            let mut run = serde_json::to_value(at(day, &format!("2026-10-{day:02}"))).unwrap();
            run["contest"] = "held".into();
            holding_in_place(&run, &small_objects(70_000))
        };
        let backup = read_up_to_the_bound(run, log_bytes, RUN_LOGS_MAX_BYTES);
        let kept = backup.sessions[0].contest.as_ref();
        let kept = kept.and_then(|log| log.0.as_array()).map(Vec::len);
        assert_eq!(kept, Some(70_000), "kept as it was");
    }

    /// What a long history holds, as Dust stores it, fits the bounds of
    /// what a backup's sessions may keep as JSON: the logs of two hundred
    /// contest runs of an hour, worked at twenty QSOs a minute, far faster
    /// than anyone works, or of a thousand at 235 an hour
    /// ([`RUN_LOGS_MAX_BYTES`]); and a key a later build adds to each group
    /// of sixteen thousand sessions of twenty groups ([`UNREAD_MAX_BYTES`]).
    #[test]
    fn the_logs_and_a_later_builds_keys_of_a_long_history_fit_their_bounds() {
        let run = |qsos: u64| -> SessionResult {
            let log = cw_core::ContestRun {
                v: 1,
                kind: "wpx".into(),
                style: "single".into(),
                callers: [1, 1],
                side: String::new(),
                my_country: "DL".into(),
                my_continent: "EU".into(),
                my_zone: 14,
                fields: vec!["nr".into()],
                limit_s: 3_600,
                run_ms: 3_600_000,
                paused_ms: 0,
                my_wpm: 25.0,
                assisted: false,
                qsos: (1..=qsos)
                    .map(|n| cw_core::QsoRecord {
                        t: n * 3_600_000 / qsos,
                        h: n * 3_600_000 / qsos - 1_000,
                        g: 2,
                        nr: n as u32,
                        k: String::new(),
                        c: "K".into(),
                        lp: vec!["K".into(), "NA".into()],
                        l: Vec::new(),
                        lc: String::new(),
                        s: Vec::new(),
                        sc: String::new(),
                        a: 0,
                    })
                    .collect(),
            };
            let mut run = serde_json::to_value(at(1, "2026-10-10")).unwrap();
            run["contest"] = serde_json::to_value(cw_core::ContestLog::from(&log)).unwrap();
            serde_json::from_str(&run.to_string()).expect("a run")
        };
        let fast = log_bytes(&run(1_200));
        assert!(200 * fast <= RUN_LOGS_MAX_BYTES, "{fast} bytes a run");
        let real = log_bytes(&run(235));
        assert!(1_000 * real <= RUN_LOGS_MAX_BYTES, "{real} bytes a run");

        let sent = [("KMRSU", "KMRSU"); 20];
        let session = crate::testing::stored_session("2026-10-10", &sent);
        let mut session = serde_json::to_value(session).unwrap();
        for group in session["groups"].as_array_mut().unwrap() {
            group["laterKey"] = 1.into();
        }
        let session: SessionResult = serde_json::from_str(&session.to_string()).unwrap();
        let keys = unread_bytes(&session);
        assert!(16_000 * keys <= UNREAD_MAX_BYTES, "{keys} bytes a session");
    }

    /// `backup` with none of its sessions counted as left out, as Dust
    /// 0.16.2 counted none ([`read_backup_as_0_16_2`]).
    fn uncounted(backup: Backup) -> Backup {
        Backup {
            left_out: 0,
            ..backup
        }
    }

    /// A backup says how many of its sessions this version could not read,
    /// and left out: each element of its list that reads as no session,
    /// whatever it holds. It said nothing of them, so a backup none of
    /// whose sessions this version reads was read as one with none in it.
    #[test]
    fn a_backup_counts_the_sessions_it_leaves_out() {
        let good = serde_json::to_string(&at(5, "2026-10-05")).unwrap();
        let read =
            |raw: &str| read_backup(raw).map(|backup| (backup.sessions.len(), backup.left_out));
        let raw = format!(r#"{{"dustBackup":1,"sessions":[{good},{{"date":5}},0,{good}]}}"#);
        assert_eq!(read(&raw), Ok((2, 2)));
        assert_eq!(
            read(r#"{"dustBackup":1,"sessions":[{"date":5}]}"#),
            Ok((0, 1))
        );
        assert_eq!(read(&format!(r#"[{{"date":5}},{good}]"#)), Ok((1, 1)));
        let whole = write_backup(&TrainingSettings::default(), &[at(1, "2026-10-01")], 7);
        assert_eq!(read(&whole), Ok((1, 0)));
        assert_eq!(Backup::told_left_out(0), None);
        assert_eq!(
            Backup::told_left_out(1).as_deref(),
            Some("1 session in it could not be read by this version and was left out.")
        );
        assert_eq!(
            Backup::told_left_out(2).as_deref(),
            Some("2 sessions in it could not be read by this version and were left out.")
        );
    }

    /// The backup `backup` comes to, written with `stored`, and what it
    /// holds; or why nothing of it was.
    fn written(
        backup: &BackupFile,
        stored: Vec<Result<Record, RecordId>>,
        deleted: &[RecordId],
    ) -> Result<(String, BackedUp), NotSaved> {
        let mut out = Vec::new();
        let backed = backup.write(&mut out, stored, |id| deleted.contains(&id))?;
        Ok((String::from_utf8(out).expect("text"), backed))
    }

    /// A backup written a session at a time is the backup written whole of
    /// the same history, byte for byte: those stored as they are stored,
    /// those here the store has yet to take in their places among them,
    /// and one both stored and here once, as stored.
    #[test]
    fn a_backup_written_a_session_at_a_time_is_the_whole_one() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 9;
        let history: Vec<SessionResult> = (1..=6)
            .map(|day| at(day, &format!("2026-10-0{day}")))
            .collect();
        let records = records_for_blob(&history);
        // Here and not stored: the first, the fourth, and the last, which
        // is stored too.
        let unsaved = vec![records[5].clone(), records[0].clone(), records[3].clone()];
        let backup = BackupFile::new(&settings, 42, unsaved);
        let stored = [1, 2, 4, 5].map(|at| records[at].clone()).to_vec();
        let (written, backed) = backup.whole(Some(stored), |_| false).expect("written");
        assert_eq!(written, write_backup(&settings, &history, 42));
        assert_eq!(
            backed,
            BackedUp {
                sessions: 6,
                left_out: 0
            }
        );
        assert_eq!(read_backup(&written), read_backup_as_0_16_2(&written));
        assert_eq!(BackedUp::default().told(), None);
    }

    /// A session the store cannot read, or holds as no JSON at all, goes
    /// in as the app holds it. Where the app may hold it and has not given
    /// it, nothing is written, and the app is asked for every session it
    /// holds; given them, the backup is whole. Only a session the app holds
    /// none of, one this build could not read when it read the store, is
    /// left out, and counted. It used to be left out and called damaged
    /// though the app held it.
    #[test]
    fn a_session_the_store_cannot_read_goes_in_as_the_app_holds_it() {
        let settings = TrainingSettings::default();
        let history: Vec<SessionResult> = (1..=6)
            .map(|day| at(day, &format!("2026-10-0{day}")))
            .collect();
        let records = records_for_blob(&history);
        let unsaved = vec![records[0].clone(), records[3].clone()];
        // The second cannot be read where it is stored; the fourth cannot
        // either, but the app has yet to hear the store took it; the fifth
        // is stored as no JSON at all.
        let damaged = || -> Vec<Result<Record, RecordId>> {
            vec![
                Err(records[1].id),
                Ok(records[2].clone()),
                Err(records[3].id),
                Ok(Record {
                    id: records[4].id,
                    json: "{ half".into(),
                }),
                Ok(records[5].clone()),
            ]
        };
        let backup = BackupFile::new(&settings, 42, unsaved);
        assert_eq!(
            written(&backup, damaged(), &[]),
            Err(NotSaved::Unread),
            "the app may hold the second and the fifth"
        );
        assert_eq!(
            NotSaved::Unread.why(),
            "the history stored here could not be read"
        );

        let every = BackupFile::new(&settings, 42, records.clone()).holding_every_session();
        let (whole, backed) = written(&every, damaged(), &[]).expect("written");
        assert_eq!(whole, write_backup(&settings, &history, 42));
        assert_eq!(backed.left_out, 0);

        // Read as nothing when the app read the store: only the store
        // holds them.
        let unread = backup.beside_unread([records[1].id, records[4].id].into());
        let (file, backed) = written(&unread, damaged(), &[]).expect("written");
        assert_eq!(
            backed,
            BackedUp {
                sessions: 4,
                left_out: 2
            }
        );
        assert_eq!(
            backed.told().as_deref(),
            Some("2 sessions stored here could not be read, and could not go in it.")
        );
        let kept = [0, 2, 3, 5].map(|at| history[at].clone());
        assert_eq!(file, write_backup(&settings, &kept, 42));
    }

    /// A store whose history cannot be listed writes nothing of a backup
    /// until the app gives every session it holds; then the backup is
    /// those, and the sessions stored that this build could not read are
    /// told of as left out.
    #[test]
    fn a_history_that_cannot_be_listed_is_backed_up_from_the_app() {
        let settings = TrainingSettings::default();
        let history: Vec<SessionResult> = (1..=3)
            .map(|day| at(day, &format!("2026-10-0{day}")))
            .collect();
        let records = records_for_blob(&history);
        let unsaved = BackupFile::new(&settings, 42, vec![records[2].clone()]);
        assert_eq!(unsaved.whole(None, |_| false), Err(NotSaved::Unread));
        let mut out = Vec::new();
        assert_eq!(unsaved.write_unlisted(&mut out), Err(NotSaved::Unread));
        assert!(out.is_empty(), "nothing written");

        let every = BackupFile::new(&settings, 42, records).holding_every_session();
        let (written, backed) = every.whole(None, |_| false).expect("written");
        assert_eq!(written, write_backup(&settings, &history, 42));
        assert_eq!(backed.left_out, 0);
        let later = RecordId {
            order: 9_000_000_000,
            nonce: 1,
        };
        let (_, backed) = every
            .beside_unread([later].into())
            .whole(None, |_| false)
            .expect("written");
        assert_eq!(
            backed,
            BackedUp {
                sessions: 3,
                left_out: 1
            }
        );
    }

    /// A session here that the store holds as deleted stays out of a
    /// backup, though the app has yet to hear the deletion was made, or
    /// has yet to hear its Undo was not: restoring the backup would bring
    /// it back. One the store neither holds nor remembers deleting goes in
    /// as the app holds it: missing is never deleted.
    #[test]
    fn a_session_the_store_holds_as_deleted_stays_out_of_a_backup() {
        let settings = TrainingSettings::default();
        let history: Vec<SessionResult> = (1..=4)
            .map(|day| at(day, &format!("2026-10-0{day}")))
            .collect();
        let records = records_for_blob(&history);
        let stored = vec![Ok(records[0].clone())];
        let gone = [records[1].id, records[3].id];
        let unsaved = BackupFile::new(&settings, 42, vec![records[1].clone()]);
        let (file, backed) = written(&unsaved, stored.clone(), &gone).expect("written");
        assert_eq!(file, write_backup(&settings, &history[..1], 42));
        assert_eq!(backed.left_out, 0);

        let every = BackupFile::new(&settings, 42, records).holding_every_session();
        let (file, backed) = written(&every, stored, &gone).expect("written");
        let kept = [0, 2].map(|at| history[at].clone());
        assert_eq!(file, write_backup(&settings, &kept, 42));
        assert_eq!(backed.left_out, 0);
    }

    /// A backup is read on a thread of its own, as it is in the app, and
    /// reads there as it reads here.
    #[cfg(feature = "native-runtime")]
    #[test]
    fn a_backup_read_on_a_thread_of_its_own_reads_as_here() {
        let raw = write_backup(&TrainingSettings::default(), &[at(1, "2026-10-01")], 3);
        let read = crate::persist::worker::wait(read_on_a_thread(raw.clone()));
        assert_eq!(read, read_backup(&raw));
        let not = crate::persist::worker::wait(read_on_a_thread("not json".into()));
        assert_eq!(not, Err(NotRestored::NotABackup));
    }

    /// A backup of a hundred megabytes, some twenty-three thousand sessions
    /// of thirty groups each, restores into a history of five thousand:
    /// read, and what it brings worked out, in seconds, and in about as
    /// much memory as the file again.
    #[test]
    #[ignore = "a timing, of a hundred megabytes: run by hand"]
    fn a_hundred_megabyte_backup_restores() {
        let groups: Vec<(String, String)> = (0..30)
            .map(|n| (format!("K{n}M"), format!("K{n}M")))
            .collect();
        let sent: Vec<(&str, &str)> = groups
            .iter()
            .map(|(text, typed)| (text.as_str(), typed.as_str()))
            .collect();
        let one = crate::testing::stored_session("2026-10-01", &sent);
        let history: Vec<SessionResult> = (0..23_500u64)
            .map(|n| SessionResult {
                timestamp: 1_700_000_000_000 + n * 600_000,
                date: format!("day {n}"),
                ..one.clone()
            })
            .collect();
        let records = records_for_blob(&history);
        let backup = BackupFile::new(&TrainingSettings::default(), 1, Vec::new());
        let (raw, backed) = backup.whole(Some(records), |_| false).expect("written");
        eprintln!("{} sessions, {} MB", backed.sessions, raw.len() / 1_000_000);
        let started = std::time::Instant::now();
        let read = read_backup(&raw).expect("a backup");
        let current = &history[..5_000];
        let merged = merge_sessions(current, read.sessions);
        let took = started.elapsed();
        eprintln!("read and merged in {took:?}");
        assert!(raw.len() >= 100_000_000, "{}", raw.len());
        assert_eq!(merged.added.len(), 18_500);
        assert!(merged.filled.is_empty());
        assert!(took < std::time::Duration::from_secs(60), "{took:?}");
    }

    /// A history of sixteen thousand sessions, a later build's key on each
    /// of their twenty groups, restores whole: what they hold that this
    /// build does not read fits its bound ([`UNREAD_MAX_BYTES`]).
    #[test]
    #[ignore = "a hundred megabytes: run by hand"]
    fn a_long_history_with_a_later_builds_key_on_every_group_restores_whole() {
        let sent = [("KMRSU", "KMRSU"); 20];
        let session = crate::testing::stored_session("2026-10-10", &sent);
        let mut session = serde_json::to_value(session).unwrap();
        for group in session["groups"].as_array_mut().unwrap() {
            group["laterKey"] = 1.into();
        }
        let list: Vec<String> = (0..16_000_u64)
            .map(|n| {
                session["timestamp"] = (1_700_000_000_000 + n * 600_000).into();
                session.to_string()
            })
            .collect();
        let raw = format!("[{}]", list.join(","));
        let backup = read_backup(&raw).expect("a history");
        assert_eq!((backup.sessions.len(), backup.left_out), (16_000, 0));
        let keys: usize = backup.sessions.iter().map(unread_bytes).sum();
        assert!(keys <= UNREAD_MAX_BYTES, "{keys}");
    }
}
