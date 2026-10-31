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
use super::{SessionKey, load_settings_from};

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
/// left as it lies in the file until it is read on its own.
#[derive(Deserialize)]
struct OnDisk<'a> {
    #[serde(rename = "dustBackup", default)]
    mark: Option<serde_json::Value>,
    #[serde(rename = "savedAt", default)]
    saved_at: Option<serde_json::Value>,
    /// Whatever it holds, `null` too: a backup with settings of any kind
    /// takes them, as read ([`load_settings_from`]).
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
/// lies in the file, and one that does not read is left out alone. A file
/// of a gigabyte is read in about as much memory again.
pub fn read_backup(raw: &str) -> Result<Backup, NotRestored> {
    // A mark of the encoding, as an editor saving a file may put first, is
    // no part of what it holds.
    let json = raw.trim_start_matches(['\u{feff}', ' ', '\t', '\n', '\r']);
    if json.starts_with('[') {
        let sessions = each_session(json).ok_or(NotRestored::NotABackup)?;
        if sessions.is_empty() {
            return Err(NotRestored::NoSessions);
        }
        return Ok(Backup {
            saved_at: 0,
            settings: None,
            sessions,
        });
    }
    if !json.starts_with('{') {
        return Err(NotRestored::NotABackup);
    }
    let file: OnDisk = serde_json::from_str(json).map_err(|_| NotRestored::NotABackup)?;
    if !file.mark.as_ref().is_some_and(serde_json::Value::is_u64) {
        return Err(NotRestored::NotABackup);
    }
    let settings = file
        .settings
        .map(|settings| load_settings_from(settings.get()));
    let sessions = file
        .sessions
        .and_then(|sessions| each_session(sessions.get()))
        .ok_or(NotRestored::Unreadable)?;
    Ok(Backup {
        saved_at: file
            .saved_at
            .as_ref()
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        settings,
        sessions,
    })
}

/// Every session of `raw`, a JSON list, that reads, each read from where
/// it lies, and scored as sessions score today: one that does not read is
/// left out alone. None where `raw` is no list.
fn each_session(raw: &str) -> Option<Vec<SessionResult>> {
    let each: Vec<&RawValue> = serde_json::from_str(raw).ok()?;
    Some(
        each.into_iter()
            .filter_map(|session| serde_json::from_str(session.get()).ok().map(scored))
            .collect(),
    )
}

/// How Dust 0.16.2 read a backup, kept as it was: a backup this version
/// writes must still restore there.
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
/// `kept` has not.
fn filled_from(kept: &SessionResult, restored: SessionResult) -> Option<SessionResult> {
    if kept.contest.is_none() && restored.contest.is_some() {
        return Some(restored);
    }
    if !dropped_char_times(kept, &restored) {
        return None;
    }
    let mut now = kept.clone();
    for (timing, restored) in now.group_timings.iter_mut().zip(restored.group_timings) {
        if timing.char_times.is_none() {
            timing.char_times = restored.char_times;
        }
    }
    Some(now)
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
    use crate::persist::record::records_for_blob;
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
    /// is a backup, what is not, and what of a damaged one is kept.
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
        ];
        for raw in &cases {
            assert_eq!(read_backup(raw), read_backup_as_0_16_2(raw), "{raw}");
        }
        // And a mark of the encoding first, as an editor saving it may put,
        // which used to make it no backup at all.
        let marked = format!("\u{feff}{}", cases[0]);
        assert_eq!(read_backup(&marked), read_backup(&cases[0]));
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
}
