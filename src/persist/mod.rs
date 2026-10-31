//! What the app keeps between launches: the settings, the theme, the history
//! of sessions and the auto-level counters — and the backup file the user
//! keeps of them, outside the store ([`backup`]).
//!
//! One [`Store`] per platform — files on desktop and mobile, `localStorage`
//! in a browser — built once and handed to the app, which never asks which
//! one it has. Reading is forgiving: a store that cannot read something gives
//! back the most it can, and never writes over what it could not read.

#[cfg(all(target_os = "android", not(test)))]
mod android;
pub mod backup;
#[cfg(feature = "native-runtime")]
mod desktop;
#[cfg(feature = "native-runtime")]
mod files;
mod legacy;
#[cfg(test)]
pub mod memory;
pub mod record;
#[cfg(feature = "web")]
mod web;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use cw_core::{
    AutoLevelCounters, AutoLevelProgress, CountersKey, RF_GAIN_MAX_DB, RF_GAIN_MIN_DB,
    SessionResult, TrainingSettings, auto_level_progress,
};
use serde::Deserialize;

use record::{HistoryChange, HistoryLoad, Pending, Record, RecordId};

/// How much history is kept: years of daily practice.
pub const MAX_SESSIONS: usize = 500;

pub trait Store {
    fn load_theme(&self) -> String;
    fn save_theme(&self, theme: &str);
    /// Whether the guided setup has been opened on this install: it opens by
    /// itself on a first launch only.
    fn tutorial_done(&self) -> bool;
    fn mark_tutorial_done(&self);
    /// Whether the learner has been told that the keys right of P are the
    /// paddles, a character they type having keyed one: told once on this
    /// install, as a USB paddle keys through them every day.
    fn told_of_paddle_keys(&self) -> bool;
    fn mark_told_of_paddle_keys(&self);
    /// Whether the learner has been asked, on this install, whether to keep
    /// Lock typing while sending on, or has turned it on or off since: the
    /// default before Dust 0.16, which every save keeps, so a choice
    /// cannot be told from the old default without asking.
    fn asked_about_lock(&self) -> bool;
    fn mark_asked_about_lock(&self);
    /// The stored settings, clamped; `None` when nothing stored reads as
    /// settings at all: a first launch, or a store that cannot be read.
    ///
    /// Settings stored that do not read at all are set aside first
    /// ([`Store::take_set_aside`]), and until they are, held: written over
    /// by nothing.
    fn read_settings(&self) -> Option<TrainingSettings>;
    /// The stored settings, clamped; defaults for whatever could not be read.
    fn load_settings(&self) -> TrainingSettings {
        self.read_settings().unwrap_or_default()
    }
    /// Store the settings, or say why they could not be stored.
    fn write_settings(&self, settings: &TrainingSettings) -> Result<(), String>;
    /// Store the settings, whatever becomes of them: how a test sets up
    /// what a store holds.
    #[cfg(test)]
    fn save_settings(&self, settings: &TrainingSettings) {
        let _ = self.write_settings(settings);
    }
    /// Every stored session that can still be read, oldest first; `None`
    /// when no history is stored, or none that can be read at all, which
    /// is not the same as one every session was deleted from.
    ///
    /// Each read also says whether a save may go over what is stored: a
    /// history this read could not read, nor set aside, is held until a
    /// later read can ([`HistorySaved::Held`]).
    fn read_sessions(&self) -> Option<Vec<SessionResult>>;
    /// Every stored session that can still be read, oldest first.
    fn load_sessions(&self) -> Vec<SessionResult> {
        self.read_sessions().unwrap_or_default()
    }
    /// Store the newest of `sessions`, up to [`MAX_SESSIONS`].
    fn save_sessions(&self, sessions: &[SessionResult]) -> HistorySaved;
    /// The sessions deleted by hand that the store remembers ([`Deleted`]);
    /// none where it remembers none, as a store from before it did.
    fn read_deleted(&self) -> Vec<Deleted>;
    /// Remember `deleted` as the sessions deleted by hand, in place of what
    /// was remembered.
    fn save_deleted(&self, deleted: &[Deleted]);
    /// What reading the store has set aside, as it could not read it, since
    /// this was last asked: taken, so the user is told of each once.
    fn take_set_aside(&self) -> Vec<SetAside> {
        Vec::new()
    }
    fn load_auto_counters(&self, key: &CountersKey) -> AutoLevelCounters;
    fn save_auto_counters(&self, key: &CountersKey, counters: AutoLevelCounters);
    fn clear_auto_counters(&self, keys: &[CountersKey]);
    /// When a backup was last saved, in milliseconds since the epoch.
    fn last_backup(&self) -> Option<u64>;
    fn mark_backup(&self, at: u64);
    /// Whether this build can hand the user a backup file at all.
    fn saves_backups(&self) -> bool;
    /// Hand `contents` to the user as a file called `name`, outside the
    /// store, somewhere they can reach it: a download in a browser, a file
    /// in Downloads on a computer or an Android phone, one in the Files app
    /// on an iPhone. Says what became of it, in words for the user, or why
    /// it could not go.
    fn save_backup_file(&self, name: &str, contents: &str) -> BackupSaving;
    /// Whether a backup file this store says it saved is known to be
    /// saved. A browser hands a download over and hears no more of it:
    /// blocked or turned down, it says nothing.
    fn confirms_backups(&self) -> bool {
        true
    }
    /// Whether a backup file this store saves is kept inside the app, and
    /// deleted with it, until the user moves it out: and if so, how they
    /// do, in words that follow "move it out". An iPhone keeps it in the
    /// app's own Documents, and Android before 10 in the app's own folder
    /// on the shared storage; a backup there is no backup against deleting
    /// the app.
    fn backups_inside_app(&self) -> Option<&'static str> {
        None
    }
    /// Ask the platform to keep what is stored rather than clear it when it
    /// needs room. Only a browser clears a store on its own, and it may ask
    /// the user first.
    fn ask_to_keep(&self) {}
    /// Mark this copy of the app's session or run open while it runs
    /// (`Some`), or over (`None`): it ended, kept or let go. Each copy marks
    /// its own and holds it, a tab or a window of its own; one whose copy
    /// stopped while it was marked was cut off ([`Store::take_cut_off`]).
    fn mark_open(&self, open: Option<OpenSession>);
    /// The sessions or runs copies of the app left marked open, and stopped
    /// under: closed, crashed, killed, reloaded. Taken, so each is told of
    /// once; one a copy is still running is left to it.
    fn take_cut_off(&self) -> CutOffTaking;
    // The app moves onto these next: until then only tests call them.
    /// The whole history, by record ([`record`]): moved first, where an
    /// older build kept it whole, and with whatever an older build stored
    /// there since taken up.
    #[cfg_attr(not(test), allow(dead_code))]
    fn load_history(&self) -> HistoryLoad;
    /// Make `change` to the history, and say what became of it. A store
    /// that cannot make it all makes none of it that would lose a session:
    /// what it stored stays stored, and nothing is deleted before what
    /// fills it in is stored.
    #[cfg_attr(not(test), allow(dead_code))]
    fn change_history(&self, change: HistoryChange) -> Pending<HistorySaved>;
    /// Every record stored, those deleted left out: what another copy of
    /// the app stored and deleted shows up here. None where the store
    /// cannot be listed, which is no word that anything is missing.
    #[cfg_attr(not(test), allow(dead_code))]
    fn history_listing(&self) -> Pending<Option<Vec<RecordId>>>;
    /// The records `ids`, as they are stored. One that cannot be read
    /// comes back as its id.
    #[cfg_attr(not(test), allow(dead_code))]
    fn read_records(&self, ids: Vec<RecordId>) -> Pending<Vec<Result<Record, RecordId>>>;
    /// Which of `ids` were deleted by hand: of those missing from the
    /// listing, the ones to let go. Any other was lost in passing, and is
    /// to be stored again: missing is never deleted.
    #[cfg_attr(not(test), allow(dead_code))]
    fn forgotten(&self, ids: Vec<RecordId>) -> Pending<Vec<RecordId>>;
    /// Whether the history may have changed, elsewhere, since this copy
    /// last read or listed it: whether it is worth listing again.
    #[cfg_attr(not(test), allow(dead_code))]
    fn history_changed(&self) -> bool;
}

/// A session of groups or a contest run, as it is marked open while it runs
/// ([`Store::mark_open`]): so that one the app stops under is never lost
/// without a word, nor what it had done by then.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSession {
    /// When it started, in milliseconds since the epoch.
    pub started_at: u64,
    /// A contest run, rather than a session of groups.
    pub run: bool,
    /// What it would keep were it ended now, as of its last answered group
    /// or logged QSO: kept in its place should the app stop under it. None
    /// before then, in a mark from before marks held it, and in one whose
    /// copy of it no longer reads.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "read_or_none"
    )]
    pub so_far: Option<Box<SoFar>>,
}

/// What a session or run marked open would keep were it ended now: the
/// session Exit would store, outside the history, which is stored only once
/// it ends ([`OpenSession::so_far`]).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoFar {
    pub result: SessionResult,
    /// The settings a session of groups started with, for auto level to
    /// judge it by as it would have at its end. None for a contest run,
    /// which moves no level, and where they no longer read.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "read_or_none"
    )]
    pub settings: Option<TrainingSettings>,
}

/// A part of a mark that does not read, read as none: the rest of the mark
/// still tells of what was cut off, and a session whose settings do not
/// read is kept all the same.
fn read_or_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(raw).ok())
}

/// A session or run the app stopped under, as its mark tells of it.
#[derive(Clone, Debug, PartialEq)]
pub enum CutOff {
    Marked(OpenSession),
    /// The mark is there and cannot be read: cut short itself, by a power
    /// cut, say. A session was running all the same.
    Unread,
}

impl CutOff {
    /// What it would have kept, had it been ended as it was cut off.
    pub fn so_far(&self) -> Option<&SoFar> {
        match self {
            Self::Marked(open) => open.so_far.as_deref(),
            Self::Unread => None,
        }
    }

    /// The one to tell of, of all those cut off: the newest that can be
    /// read, or failing that, one that cannot.
    pub fn newest(cut: impl IntoIterator<Item = Self>) -> Option<Self> {
        cut.into_iter().max_by_key(|cut| match cut {
            Self::Marked(open) => Some(open.started_at),
            Self::Unread => None,
        })
    }

    /// A mark as it was read.
    fn read(raw: &[u8]) -> Self {
        serde_json::from_slice(raw).map_or(Self::Unread, Self::Marked)
    }
}

/// What a read set aside, as it could not read it, beside what it was
/// written over by ([`Store::take_set_aside`]). `kept` is where the copy is,
/// where the user can reach it: a file on a computer; nowhere they can on
/// a phone or in a browser, where it stays in the app's own store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetAside {
    /// The settings, whole: the app went on with its defaults.
    Settings { kept: Option<String> },
    /// The history, whole: the app went on without one.
    History { kept: Option<String> },
    /// `count` sessions of the history: the app went on without them.
    Sessions { count: usize, kept: Option<String> },
}

/// The sessions cut off, being looked for: already, or still, where only the
/// platform can tell which copies are running, as in a browser. Every one,
/// for what each kept so far.
pub type CutOffTaking = Pin<Box<dyn Future<Output = Vec<CutOff>>>>;

/// A backup file on its way: saved already, or still being saved where only
/// the platform's own thread can save it, as on Android.
pub type BackupSaving = Pin<Box<dyn Future<Output = Result<String, String>>>>;

/// A backup file saved, or not, already: everywhere but Android.
#[cfg_attr(all(target_os = "android", not(test)), allow(dead_code))]
fn saved_now(result: Result<String, String>) -> BackupSaving {
    Box::pin(std::future::ready(result))
}

/// What the user is told of a backup file saved as `place`.
#[cfg(any(feature = "native-runtime", test))]
fn saved_as(place: &str) -> String {
    format!("Backup saved as {place}.")
}

/// Why settings were not stored: those stored could not be read, nor set
/// aside, when they were last read, so they are kept as they are rather
/// than written over. In words that follow "could not be saved:".
#[cfg(any(feature = "web", feature = "native-runtime"))]
const SETTINGS_HELD: &str = "the settings stored could not be read, so they were left as they are";

/// What became of a history handed to [`Store::save_sessions`], or of a
/// change made to one by record ([`record::HistoryChange`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistorySaved {
    /// Stored: the newest `kept` sessions, which is all of them up to
    /// [`MAX_SESSIONS`]. Of a change, `kept` is how many records it stored.
    Stored { kept: usize },
    /// Stored, but the store had room for only the newest `kept`. Only a
    /// browser runs out of room: files are kept to the count alone. `runs`
    /// says the history held a contest run, whose log takes more room than
    /// a session of groups.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    Squeezed { kept: usize, runs: bool },
    /// Not stored. The history already there could not be read, nor moved
    /// out of the way, when it was last read, so it is kept as it is rather
    /// than written over.
    Held,
    /// Not stored: the store has no room left. The session is kept here,
    /// in the app, until it has.
    #[cfg_attr(feature = "web", allow(dead_code))]
    Full,
    /// Not stored: a newer Dust keeps the history here in a way this one
    /// would harm, so it writes nothing to it ([`record::NEWER_LAYOUT`]).
    #[cfg_attr(feature = "web", allow(dead_code))]
    Newer,
    /// Not stored, for the reason given.
    Failed(String),
}

impl HistorySaved {
    /// How many of the newest sessions the store now holds, when it stored
    /// any.
    pub fn kept(&self) -> Option<usize> {
        match self {
            Self::Stored { kept } | Self::Squeezed { kept, .. } => Some(*kept),
            Self::Held | Self::Full | Self::Newer | Self::Failed(_) => None,
        }
    }

    /// What to tell the user, when there is anything to tell.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Stored { .. } => None,
            Self::Squeezed { kept, runs } => {
                let why = if *runs {
                    " Long contest runs take more room."
                } else {
                    ""
                };
                Some(format!(
                    "Only your newest {kept} sessions fit in this browser's storage; older ones were let go.{why}"
                ))
            }
            Self::Held => Some(
                "Your saved history could not be read, so it was left as it is and this session was not saved."
                    .into(),
            ),
            Self::Full => Some(
                if cfg!(feature = "web") {
                    "This browser has no room left for this site: this session is kept here until there is. Save a backup to keep it safe."
                } else {
                    "This device has no room left: this session is kept here until there is. Save a backup to keep it safe."
                }
                .into(),
            ),
            Self::Newer => Some(record::NEWER_LAYOUT.into()),
            Self::Failed(reason) => Some(format!("This session could not be saved: {reason}")),
        }
    }
}

/// The store this build keeps its data in.
pub fn default_store() -> Rc<dyn Store> {
    #[cfg(test)]
    {
        // Nothing a test does may reach the user's own data.
        Rc::new(memory::MemoryStore::default())
    }
    #[cfg(all(feature = "web", not(test)))]
    {
        Rc::new(web::WebStore::default())
    }
    #[cfg(all(feature = "native-runtime", not(test)))]
    {
        Rc::new(desktop::DesktopStore::new())
    }
}

/// How the auto-level run for `settings` stands.
pub fn auto_progress(store: &dyn Store, settings: &TrainingSettings) -> Option<AutoLevelProgress> {
    auto_level_progress(
        settings,
        store.load_auto_counters(&CountersKey::of(settings)),
    )
}

/// The newest [`MAX_SESSIONS`] of `sessions`: what a store keeps.
fn newest(sessions: &[SessionResult]) -> &[SessionResult] {
    &sessions[sessions.len().saturating_sub(MAX_SESSIONS)..]
}

/// `written`, a record as this build writes it, laid over `stored`, the
/// same record as it was stored: whatever a later build added to it stays.
#[cfg(any(feature = "web", feature = "native-runtime"))]
fn over_stored(written: serde_json::Value, stored: Option<serde_json::Value>) -> serde_json::Value {
    match (written, stored) {
        (serde_json::Value::Object(mut written), Some(serde_json::Value::Object(stored))) => {
            for (key, value) in stored {
                written.entry(key).or_insert(value);
            }
            serde_json::Value::Object(written)
        }
        (written, _) => written,
    }
}

/// Load settings without throwing away everything for one bad field.
///
/// A strict parse fails on a single unreadable value — a hand-edited file, or a
/// field whose type changed between versions — and the user would silently get
/// factory settings back. So fall back to applying the stored keys one at a
/// time and keeping the ones that still deserialize.
///
/// Only a key this build reads can fail to, so only those are tried one at a
/// time: a few dozen, however large the save. Every other key is carried along
/// as it is ([`TrainingSettings::unread`]), and goes in with the rest at the
/// end. Each key used to be tried on top of every key kept before it, which
/// took time in the square of their number: a file of tens of thousands of
/// keys, a backup made that way, held the app up for minutes.
fn recover_settings(raw: &str) -> TrainingSettings {
    let Ok(serde_json::Value::Object(mut stored)) = serde_json::from_str::<serde_json::Value>(raw)
    else {
        return TrainingSettings::default();
    };
    upgrade_receiver(&mut stored);
    upgrade_agc_threshold(&mut stored);
    if let Ok(settings) = TrainingSettings::deserialize(&stored) {
        return settings;
    }
    let read = TrainingSettings::read_keys();
    let (tried, carried): (Vec<_>, Vec<_>) = stored
        .into_iter()
        .partition(|(key, _)| read.contains(key.as_str()));
    let mut kept = serde_json::Map::new();
    for (key, value) in tried {
        kept.insert(key.clone(), value);
        if TrainingSettings::deserialize(&kept).is_err() {
            kept.remove(&key);
        }
    }
    let settings = TrainingSettings::deserialize(&kept).unwrap_or_default();
    kept.extend(carried);
    TrainingSettings::deserialize(&kept).unwrap_or(settings)
}

/// A save from before the receiver was quoted by its 6 dB width has no
/// `filterWidthHz`. Its filter shape named a different receiver — almost
/// always the old sharp default, stored because every save stores it — so it
/// is not carried over either: such a save comes back on today's receiver,
/// shape and width both, rather than keeping one half of the old one.
fn upgrade_receiver(stored: &mut serde_json::Map<String, serde_json::Value>) {
    if !stored.contains_key("filterWidthHz") {
        stored.remove("filterShape");
    }
}

/// A save from before the AGC threshold went may have one, `agcThresholdDb`.
/// The AGC judges everything against its knee, so a threshold raised X dB
/// is the RF gain backed off X dB, but for a tenth of that in volume: such
/// a save comes back with its RF gain backed off as far, as far as the RF
/// gain goes. A threshold taken down into the hiss has nothing to come back
/// as, and nor has one under an AGC switched off, which it never moved.
fn upgrade_agc_threshold(stored: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(threshold) = stored
        .remove("agcThresholdDb")
        .and_then(|threshold| threshold.as_f64())
    else {
        return;
    };
    let agc = stored
        .get("agcEnabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if !agc || threshold <= 0.0 {
        return;
    }
    let rf = stored
        .get("rfGainDb")
        .and_then(serde_json::Value::as_f64)
        .filter(|db| db.is_finite())
        .unwrap_or(RF_GAIN_MAX_DB)
        .clamp(RF_GAIN_MIN_DB, RF_GAIN_MAX_DB);
    let backed_off = (rf - threshold).round().max(RF_GAIN_MIN_DB);
    stored.insert("rfGainDb".into(), serde_json::json!(backed_off));
}

/// Stored settings, as the app may use them.
fn load_settings_from(raw: &str) -> TrainingSettings {
    recover_settings(raw).clamp()
}

/// Stored settings, as the app may use them, when what is stored is
/// settings at all: a JSON object, however much of it still reads.
#[cfg(any(feature = "web", feature = "native-runtime"))]
fn read_settings_from(raw: &str) -> Option<TrainingSettings> {
    matches!(
        serde_json::from_str::<serde_json::Value>(raw),
        Ok(serde_json::Value::Object(_))
    )
    .then(|| load_settings_from(raw))
}

/// `theirs`, the settings stored now, with every setting `ours` changed
/// from `base` laid over it.
///
/// `base` is the settings as this copy of the app last read or wrote them.
/// Another tab, window or copy of the app may have stored changes of its
/// own since, and a copy that wrote all of its settings back would put each
/// of those back as it was: only what changed here is written, setting by
/// setting, as a save names them.
pub fn merge_settings(
    base: &TrainingSettings,
    ours: &TrainingSettings,
    theirs: &TrainingSettings,
) -> TrainingSettings {
    let keys = |settings: &TrainingSettings| match serde_json::to_value(settings) {
        Ok(serde_json::Value::Object(keys)) => keys,
        _ => serde_json::Map::new(),
    };
    let (base, mine, mut merged) = (keys(base), keys(ours), keys(theirs));
    // Every key either side has: one only some settings write, as Contest
    // writes `contestActive`, goes when this copy left it behind.
    let changed: Vec<&String> = base
        .keys()
        .chain(mine.keys())
        .filter(|key| base.get(*key) != mine.get(*key))
        .collect();
    for key in changed {
        match mine.get(key) {
            Some(value) => merged.insert(key.clone(), value.clone()),
            None => merged.remove(key),
        };
    }
    serde_json::from_value::<TrainingSettings>(serde_json::Value::Object(merged))
        .map_or_else(|_| ours.clone(), TrainingSettings::clamp)
}

/// What tells one session of the history from another: when it finished,
/// and on what day. Written as `[timestamp, "date"]`, as the move to
/// records lists the sessions it took ([`legacy::Layout`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SessionKey(u64, String);

impl SessionKey {
    pub fn of(session: &SessionResult) -> Self {
        Self(session.timestamp, session.date.clone())
    }

    /// The key of a session finished at `timestamp`, on `date`.
    #[cfg_attr(feature = "web", allow(dead_code))]
    pub fn new(timestamp: u64, date: String) -> Self {
        Self(timestamp, date)
    }
}

/// A session deleted by hand, as the store remembers it, by its
/// [`SessionKey`]: what tells a session another copy of the app deleted
/// from one it never stored, or lost in passing ([`merge_history`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    pub timestamp: u64,
    pub date: String,
    /// When it was deleted, in milliseconds since the epoch.
    pub at: u64,
    /// What a later build added to it, carried along.
    #[serde(flatten)]
    pub unread: serde_json::Map<String, serde_json::Value>,
}

impl Deleted {
    /// `session`, deleted `at`.
    pub fn of(session: &SessionResult, at: u64) -> Self {
        Self {
            timestamp: session.timestamp,
            date: session.date.clone(),
            at,
            unread: serde_json::Map::new(),
        }
    }

    pub fn key(&self) -> SessionKey {
        SessionKey(self.timestamp, self.date.clone())
    }
}

/// How long a deletion is remembered: well past the time a copy of the app
/// left open on the history from before it is likely to store again.
const DELETED_KEPT_MS: u64 = 90 * 86_400_000;

/// How many deletions are remembered, at most: the newest. Two whole
/// histories deleted.
const MAX_DELETED: usize = 2 * MAX_SESSIONS;

/// `deleted`, oldest first, less what is no longer worth remembering at
/// `now`: deletions older than [`DELETED_KEPT_MS`], and all but the newest
/// [`MAX_DELETED`]. One stamped later than now, by a clock set ahead, is
/// let go by count alone. A copy that still holds a session whose deletion
/// is let go puts it back the next time it stores, rather than lose one.
pub fn prune_deleted(mut deleted: Vec<Deleted>, now: u64) -> Vec<Deleted> {
    deleted.retain(|gone| gone.at.saturating_add(DELETED_KEPT_MS) >= now);
    let over = deleted.len().saturating_sub(MAX_DELETED);
    deleted.drain(..over);
    deleted
}

/// `theirs`, the history stored now, with what `ours` changed from `base`
/// done to it.
///
/// `base` is the history as this copy of the app last read or wrote it.
/// Another tab, window or copy may have stored sessions since, or deleted
/// some, and a copy that wrote its whole history back would lose the one
/// and bring back the other. So every session added here is added, every
/// one deleted here is deleted, and one changed here — filled in by a
/// restore — is kept as it is here; whatever another copy added, deleted
/// or changed stands. Sessions are known as [`pair_sessions`] knows them.
///
/// A session is deleted elsewhere only if `deleted`, the sessions deleted
/// by hand ([`Deleted`]), holds it: one missing from the store alone is
/// kept. Copies store without a lock between them, so one can write over
/// what another stored a moment before, from a history it read before
/// that: a session the first copy kept then is missing from the store,
/// and used to be taken for one deleted elsewhere and lost for good. A
/// session stored, or put back, elsewhere and deleted by hand is dropped
/// for the same reason; one the store let go for want of room is neither,
/// kept, and let go again as the history is stored.
///
/// The history keeps the order sessions were stored in, which is the
/// order they finished in, whatever the clock said: the store lets go of
/// the first when it has no room, so a session stamped by a clock set back
/// must not move to the front.
pub fn merge_history(
    base: &[SessionResult],
    ours: &[SessionResult],
    theirs: &[SessionResult],
    deleted: &HashSet<SessionKey>,
) -> Vec<SessionResult> {
    fn each(sessions: &[SessionResult]) -> Vec<&SessionResult> {
        sessions.iter().collect()
    }
    let was_here = pair_sessions(&each(base), &each(ours));
    let was_there = pair_sessions(&each(base), &each(theirs));
    // Each session of `base` as it is stored now, if it still is, and where.
    let mut stored = vec![None; base.len()];
    for (there, (session, was)) in theirs.iter().zip(&was_there).enumerate() {
        if let Some(was) = was {
            stored[*was] = Some((there, session));
        }
    }
    // Each session kept, and where the store has it, if it does.
    let mut merged: Vec<(SessionResult, Option<usize>)> =
        Vec::with_capacity(theirs.len().max(ours.len()));
    // Each session added here, and where it is kept.
    let mut added_here = Vec::new();
    for (session, was) in ours.iter().zip(&was_here) {
        match was {
            None => {
                added_here.push((merged.len(), session));
                merged.push((session.clone(), None));
            }
            Some(was) => match stored[*was] {
                // Deleted elsewhere.
                None if deleted.contains(&SessionKey::of(session)) => {}
                // Missing from the store, and not deleted: lost there.
                None => merged.push((session.clone(), None)),
                // Changed here, or else as it is stored.
                Some((there, stored)) if *session == base[*was] => {
                    merged.push((stored.clone(), Some(there)));
                }
                Some((there, _)) => merged.push((session.clone(), Some(there))),
            },
        }
    }
    // Added elsewhere, unless deleted by hand, or added here too, as two
    // copies restoring one backup each add the same session.
    let added_there: Vec<(usize, &SessionResult)> = theirs
        .iter()
        .enumerate()
        .zip(&was_there)
        .filter(|((_, session), was)| was.is_none() && !deleted.contains(&SessionKey::of(session)))
        .map(|(added, _)| added)
        .collect();
    let here: Vec<&SessionResult> = added_here.iter().map(|(_, session)| *session).collect();
    let there: Vec<&SessionResult> = added_there.iter().map(|(_, session)| *session).collect();
    let mut new = Vec::new();
    for (&(at, session), same) in added_there.iter().zip(pair_sessions(&here, &there)) {
        match same {
            // Kept here already: where the store has it too.
            Some(same) => merged[added_here[same].0].1 = Some(at),
            None => new.push((at, session)),
        }
    }
    // Each where the store has it: after the session it follows there, and
    // in among what this copy added since by when it finished. Sorted by
    // when each finished, a session a clock set back stamped went to the
    // front, and was the first the history let go.
    let mut new = new.into_iter().peekable();
    let mut after = 0;
    for (at, session) in theirs.iter().enumerate() {
        if let Some(kept) = merged.iter().position(|(_, there)| *there == Some(at)) {
            after = kept + 1;
        } else if let Some((_, added)) = new.next_if(|(added, _)| *added == at) {
            let place = merged[after..]
                .iter()
                .position(|(kept, _)| kept.timestamp > session.timestamp)
                .map_or(merged.len(), |later| after + later);
            merged.insert(place, (added.clone(), Some(at)));
            after = place + 1;
        }
    }
    merged.into_iter().map(|(session, _)| session).collect()
}

/// Which of `from` each of `to` is, if any: the one with the same
/// [`SessionKey`] and everything else the same, or failing that, the first
/// with the same key still unclaimed.
///
/// Sessions alike in their key are told apart by what they hold before
/// their order: one deleted from a copy would otherwise shift every one
/// after it onto the session before, and a merge would keep one twice and
/// lose another. Only one changed since, filled in by a restore, falls back
/// to its order.
pub fn pair_sessions(from: &[&SessionResult], to: &[&SessionResult]) -> Vec<Option<usize>> {
    let mut by_key: HashMap<SessionKey, Vec<usize>> = HashMap::new();
    for (at, session) in from.iter().enumerate() {
        by_key.entry(SessionKey::of(session)).or_default().push(at);
    }
    let mut claimed = vec![false; from.len()];
    let mut paired = vec![None; to.len()];
    let alike: Vec<Option<&Vec<usize>>> = to
        .iter()
        .map(|session| by_key.get(&SessionKey::of(session)))
        .collect();
    // Those alike in everything first, then the rest by their order.
    for whole in [true, false] {
        for (at, session) in to.iter().enumerate() {
            let Some(alike) = alike[at].filter(|_| paired[at].is_none()) else {
                continue;
            };
            let found = alike
                .iter()
                .copied()
                .find(|&was| !claimed[was] && (!whole || from[was] == *session));
            if let Some(was) = found {
                claimed[was] = true;
                paired[at] = Some(was);
            }
        }
    }
    paired
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use cw_core::CharSetMode;

    /// Settings with every one of them moved off its default, and every
    /// value in bounds, so a save and a load have nothing to put back: what
    /// is read is what was written, setting by setting.
    pub(crate) fn every_setting_moved() -> TrainingSettings {
        let mut s = TrainingSettings::default();
        let c = &mut s.curriculum;
        c.level = 9;
        // Contest is written as Callsigns with `contestActive` beside it, so
        // this writes that key too.
        c.char_set_mode = CharSetMode::Contest;
        c.digits_level = 4;
        c.mixed_letters_percent = 33;
        c.custom_sequence = vec!['Z', 'Q', 'X'];
        c.sequence_is_custom = true;
        c.kept_sequence = vec!['K', 'M'];
        c.practice_window = cw_core::PracticeWindow::Last3;
        c.callsign_level = 5;
        c.word_list = vec!["CQ".into(), "TEST".into()];
        c.num_groups = 7;
        c.min_group_size = 4;
        c.max_group_size = 4;
        c.link_group_size = true;
        c.adaptive_group_size = true;
        c.aim = cw_core::Aim::Stretch;
        let p = &mut s.playback;
        p.char_wpm_min = 31.0;
        p.char_wpm_max = 31.0;
        p.link_char_wpm = true;
        p.effective_wpm_min = 11.0;
        p.effective_wpm_max = 11.0;
        p.link_effective_wpm = true;
        p.link_char_to_effective = false;
        p.adaptive_speed = true;
        p.extra_word_space_multiplier = 2.5;
        p.group_timeout = 30.0;
        p.group_pause_sec = 6.0;
        p.countdown_sec = 7;
        p.lock_input_during_group_playback = true;
        p.paddle_swap = true;
        p.paddle_keys_by_place = false;
        p.keyer_mode = cw_core::KeyerMode::Ultimatic;
        p.keyer_wpm = 32.0;
        p.fist_variation = 0.6;
        p.group_repeat_min = 2;
        p.group_repeat_max = 4;
        p.link_group_repeat = false;
        let b = &mut s.band;
        b.side_tone_min = 700.0;
        b.side_tone_max = 900.0;
        b.volume_min = 0.4;
        b.volume_max = 0.4;
        b.link_volume = true;
        b.master_volume = 0.5;
        b.steepness = 7.0;
        b.envelope_smoothing = 0.5;
        b.qsb_enabled = false;
        b.qsb_depth = 0.9;
        b.qsb_rate_hz = 0.9;
        b.qrn_enabled = false;
        b.qrn_level = 0.9;
        b.noise_enabled = false;
        b.noise_level = 1.0;
        b.rf_gain_db = -12.0;
        b.agc_enabled = false;
        b.agc_decay = cw_core::AgcDecay::Slow;
        b.filter_width_hz = 250.0;
        b.filter_shape = cw_core::FilterShape::Soft;
        b.stations_min = 2;
        b.stations_max = 4;
        b.pileup_spread_hz = 400.0;
        b.pileup_level_db = 9.0;
        let a = &mut s.auto_level;
        a.mixed_auto_level_next_axis = cw_core::MixedAutoLevelAxis::Digits;
        a.auto_adjust_level = false;
        a.auto_adjust_threshold = 77.0;
        a.auto_adjust_below_threshold_count = 4;
        a.auto_adjust_above_threshold_count = 9;
        a.error_weight_strength = 0.9;
        a.char_sampling_coverage_strength = 0.9;
        s.recognition = cw_core::RecognitionBands {
            fast_ms: 350,
            icr_ms: 550,
            slow_ms: 900,
        };
        let k = &mut s.contest;
        k.contest_kind = cw_core::ContestKind::Sprint;
        k.contest_style = cw_core::RunStyle::Pileup;
        k.stations_opened_by_pileup = true;
        k.contest_habits = cw_core::CallerHabits::Rough;
        k.contest_flutter = true;
        k.contest_minutes = 25;
        k.contest_show_copy = true;
        k.contest_tu_call = cw_core::TuCall::Third;
        k.contest_adapt_speed = false;
        k.my_wpm = 34;
        k.my_sending_db = -21;
        k.contest_qsk = true;
        k.my_call = "DL1ABC".into();
        k.my_name = "MAT".into();
        k.my_state = "MA".into();
        k.my_zone = 15;
        k.my_power = "500".into();
        k.my_class = "3A".into();
        k.my_section = "EMA".into();
        s
    }

    /// Every key a save holds is read back, each on its own: one moved in
    /// an otherwise fresh save comes back moved. A key a section did not
    /// read, or one another section took first, would come back as its
    /// default. A setting added and left out of [`every_setting_moved`] is
    /// named here too.
    #[test]
    fn every_setting_is_read_back_on_its_own() {
        let moved = serde_json::to_value(every_setting_moved()).expect("serialize");
        let fresh = serde_json::to_value(TrainingSettings::default()).expect("serialize");
        let (moved, fresh) = (
            moved.as_object().expect("an object"),
            fresh.as_object().expect("an object"),
        );
        let unmoved: Vec<&String> = fresh
            .iter()
            .filter(|(key, value)| moved.get(*key) == Some(*value))
            .map(|(key, _)| key)
            .collect();
        assert!(
            unmoved.is_empty(),
            "every_setting_moved leaves these as they ship: {unmoved:?}"
        );
        for key in fresh.keys() {
            let mut save = fresh.clone();
            save.insert(key.clone(), moved[key].clone());
            let read: TrainingSettings =
                serde_json::from_value(serde_json::Value::Object(save)).expect("a save loads");
            let written = serde_json::to_value(&read).expect("serialize");
            assert_eq!(
                written.get(key),
                Some(&moved[key]),
                "{key} was not read back"
            );
        }
    }

    /// And all of them together come back from a save exactly as they were
    /// written, through the same reading the stores and backups use.
    #[test]
    fn every_setting_comes_back_from_a_save() {
        let moved = every_setting_moved();
        assert_eq!(
            moved.clone().clamp(),
            moved,
            "the moved settings are in bounds"
        );
        let compact = serde_json::to_string(&moved).expect("serialize");
        assert_eq!(load_settings_from(&compact), moved);
        // Pretty, as the desktop writes them.
        let pretty = serde_json::to_string_pretty(&moved).expect("serialize");
        assert_eq!(load_settings_from(&pretty), moved);
        let backup = backup::read_backup(&backup::write_backup(&moved, &[], 1)).expect("a backup");
        assert_eq!(backup.settings, Some(moved));
    }

    /// A merge keeps what is stored, and lays over it only what this copy
    /// changed: each setting from whichever side moved it.
    #[test]
    fn a_merge_keeps_what_another_copy_stored() {
        let base = TrainingSettings::default();
        let mut ours = base.clone();
        ours.band.steepness = 12.0;
        let mut theirs = base.clone();
        theirs.curriculum.num_groups = 9;
        theirs.playback.char_wpm_min = 20.0;
        // Clamped, as a store reads them.
        let theirs = theirs.clamp();
        let merged = merge_settings(&base, &ours, &theirs);
        assert_eq!(merged.band.steepness, 12.0);
        assert_eq!(merged.curriculum.num_groups, 9);
        assert_eq!(merged.playback.char_wpm_min, 20.0);
        // Nothing changed here: what is stored, as it is.
        assert_eq!(merge_settings(&base, &base, &theirs), theirs);
        // Both changed one: this copy's change is the newer.
        ours.curriculum.num_groups = 4;
        assert_eq!(
            merge_settings(&base, &ours, &theirs).curriculum.num_groups,
            4
        );
    }

    /// Contest writes a key no other character set does. Into Contest here
    /// writes it; out of it here takes it away, whatever is stored.
    #[test]
    fn a_merge_carries_the_character_set_whole() {
        let on = |mode| {
            let mut settings = TrainingSettings::default();
            settings.curriculum.char_set_mode = mode;
            settings
        };
        let (contest, mixed) = (on(CharSetMode::Contest), on(CharSetMode::Mixed));
        let mut theirs = mixed.clone();
        theirs.curriculum.num_groups = 9;
        let into = merge_settings(&mixed, &contest, &theirs);
        assert_eq!(into.curriculum.char_set_mode, CharSetMode::Contest);
        assert_eq!(into.curriculum.num_groups, 9);

        let mut theirs = contest.clone();
        theirs.curriculum.num_groups = 9;
        let out = merge_settings(&contest, &on(CharSetMode::Koch), &theirs);
        assert_eq!(out.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(out.curriculum.num_groups, 9);
    }

    /// A session finished at `timestamp`, a day of its own.
    pub(crate) fn at(timestamp: u64) -> SessionResult {
        SessionResult {
            timestamp,
            ..session(&format!("2026-10-{timestamp:02}"))
        }
    }

    pub(crate) fn stamps(sessions: &[SessionResult]) -> Vec<u64> {
        sessions.iter().map(|session| session.timestamp).collect()
    }

    /// The sessions of `stamps` as deleted by hand.
    fn deleted(stamps: &[u64]) -> HashSet<SessionKey> {
        stamps
            .iter()
            .map(|&stamp| SessionKey::of(&at(stamp)))
            .collect()
    }

    /// A merge with nothing deleted by hand.
    fn merged(
        base: &[SessionResult],
        ours: &[SessionResult],
        theirs: &[SessionResult],
    ) -> Vec<SessionResult> {
        merge_history(base, ours, theirs, &HashSet::new())
    }

    /// A history merge does to what is stored what this copy did, and keeps
    /// what another copy did: each added, each deleted, in the order they
    /// were practised.
    #[test]
    fn a_history_merge_keeps_what_another_copy_added_and_deleted() {
        let base = [at(1), at(2), at(3)];
        // Here 2 was deleted and 4 added; elsewhere 3 deleted and 5 added.
        let ours = [at(1), at(3), at(4)];
        let theirs = [at(1), at(2), at(5)];
        let gone = deleted(&[2, 3]);
        assert_eq!(
            stamps(&merge_history(&base, &ours, &theirs, &gone)),
            [1, 4, 5]
        );
        // Nothing stored elsewhere: this copy's history, as it is.
        assert_eq!(merge_history(&base, &ours, &base, &gone), ours);
        // Nothing changed here: what is stored, as it is.
        assert_eq!(merge_history(&base, &base, &theirs, &gone), theirs);
        // Both added the same session, restoring the same backup: once.
        assert_eq!(stamps(&merged(&[], &[at(7)], &[at(7)])), [7]);
    }

    /// A session missing from the store, but deleted by nobody, is kept:
    /// another copy wrote over the store from a history it read before the
    /// session was stored. It used to be taken for one deleted elsewhere,
    /// and lost for good. One deleted by hand that such a write brought back
    /// goes again.
    #[test]
    fn a_session_lost_from_the_store_is_not_taken_for_one_deleted() {
        let base = [at(1), at(2)];
        let ours = [at(1), at(2), at(4)];
        // Another copy, which read the history before 2 was stored, wrote
        // it over with 3 of its own.
        let theirs = [at(1), at(3)];
        assert_eq!(stamps(&merged(&base, &ours, &theirs)), [1, 2, 3, 4]);
        // 1 was deleted here before; the other copy's write brought it back.
        let theirs = [at(1), at(2), at(3)];
        let merged_now = merge_history(&[at(2)], &[at(2)], &theirs, &deleted(&[1]));
        assert_eq!(stamps(&merged_now), [2, 3]);
    }

    /// A session another copy stored, stamped by a clock set back, stays
    /// where the store has it, after what it followed there, and what this
    /// copy adds goes after it. Sorted by its stamp, it went to the front,
    /// the first session the history lets go when it has no room.
    #[test]
    fn a_session_a_clock_set_back_stamped_keeps_its_place() {
        let base = [at(10), at(20)];
        let merged_now = merged(&base, &[at(10), at(20), at(30)], &[at(10), at(20), at(5)]);
        assert_eq!(stamps(&merged_now), [10, 20, 5, 30]);
        // Added elsewhere before the first session here: first.
        let merged_now = merged(&base, &base, &[at(7), at(10), at(20)]);
        assert_eq!(stamps(&merged_now), [7, 10, 20]);
    }

    /// A session filled in on one side — a run's log put back by a restore —
    /// is kept filled in, whichever side did it.
    #[test]
    fn a_history_merge_keeps_a_session_filled_in_on_either_side() {
        let mut filled = at(1);
        filled.accuracy = 0.5;
        let merged_here = merged(&[at(1)], std::slice::from_ref(&filled), &[at(1)]);
        assert_eq!(merged_here, [filled.clone()]);
        let merged_there = merged(&[at(1)], &[at(1)], std::slice::from_ref(&filled));
        assert_eq!(merged_there, [filled]);
    }

    /// Sessions alike in when and on what day they finished are told apart
    /// by their order: deleting one leaves the other.
    #[test]
    fn a_history_merge_tells_sessions_alike_apart() {
        let first = at(1);
        let mut second = at(1);
        second.accuracy = 0.5;
        let both = [first.clone(), second.clone()];
        let gone = deleted(&[1]);
        let merged = merge_history(&both, std::slice::from_ref(&second), &both, &gone);
        assert_eq!(merged, [second]);
        let merged = merge_history(&both, &both, std::slice::from_ref(&first), &gone);
        assert_eq!(merged, [first]);
    }

    /// Sessions alike in their key are told apart by what they hold, not
    /// by where they sit: one deleted elsewhere does not move the others
    /// up a place. Told apart by order, the first was taken for the one
    /// deleted, the second came back twice and the third went.
    #[test]
    fn a_deletion_elsewhere_does_not_shift_sessions_alike() {
        let alike = |accuracy| SessionResult { accuracy, ..at(1) };
        let (first, second, third) = (alike(0.1), alike(0.2), alike(0.3));
        let mut filled = second.clone();
        filled.avg_response_ms = 700.0;
        let base = [first.clone(), second.clone(), third.clone()];
        let ours = [first, filled.clone(), third.clone()];
        let theirs = [second, third.clone()];
        assert_eq!(
            merge_history(&base, &ours, &theirs, &deleted(&[1])),
            [filled, third]
        );
    }

    /// The same session added on both sides is kept once, and two alike in
    /// their key but not in what they hold are both kept.
    #[test]
    fn sessions_added_on_both_sides_are_kept_once() {
        let mut other = at(7);
        other.accuracy = 0.5;
        assert_eq!(
            merged(&[], &[at(7)], &[at(7), other.clone()]),
            [at(7), other]
        );
    }

    /// Deletions are remembered for a while and so many, the newest; one a
    /// clock set ahead stamped is let go by count alone.
    #[test]
    fn deletions_are_remembered_for_a_while() {
        let day = 86_400_000;
        let now = 200 * day;
        let gone = |at| Deleted::of(&at_stamp(at), at);
        let kept = prune_deleted(
            vec![gone(now - 100 * day), gone(now - day), gone(now + day)],
            now,
        );
        assert_eq!(kept, [gone(now - day), gone(now + day)]);
        let many: Vec<Deleted> = (0..MAX_DELETED as u64 + 3).map(|n| gone(now - n)).collect();
        let kept = prune_deleted(many.clone(), now);
        assert_eq!(kept.len(), MAX_DELETED);
        assert_eq!(kept.first(), many.get(3), "the oldest go");
    }

    /// A session finished at `timestamp`, by its stamp alone.
    fn at_stamp(timestamp: u64) -> SessionResult {
        SessionResult {
            timestamp,
            ..session("2026-10-01")
        }
    }

    pub(crate) fn session(date: &str) -> SessionResult {
        SessionResult {
            date: date.to_string(),
            timestamp: 1,
            accuracy: 1.0,
            avg_response_ms: 1.0,
            score: 1.0,
            char_set_mode: CharSetMode::Mixed,
            char_wpm: 20.0,
            ..SessionResult::empty_for_test()
        }
    }

    #[test]
    fn history_keeps_the_newest_sessions() {
        let all: Vec<SessionResult> = (0..MAX_SESSIONS + 20)
            .map(|i| session(&format!("2026-01-{i:03}")))
            .collect();
        let kept = newest(&all);
        assert_eq!(kept.len(), MAX_SESSIONS);
        // The oldest go, the order stays chronological.
        assert_eq!(kept.first().unwrap().date, all[20].date);
        assert_eq!(kept.last().unwrap().date, all.last().unwrap().date);
        assert!(newest(&[]).is_empty());
    }

    #[test]
    fn what_became_of_a_history_is_said_plainly() {
        assert_eq!(HistorySaved::Stored { kept: 4 }.problem(), None);
        let squeezed = |runs| HistorySaved::Squeezed { kept: 40, runs };
        assert_eq!(squeezed(false).kept(), Some(40));
        assert!(
            squeezed(false)
                .problem()
                .is_some_and(|text| text.contains("newest 40") && !text.contains("contest"))
        );
        assert!(
            squeezed(true)
                .problem()
                .is_some_and(|text| text.ends_with("Long contest runs take more room."))
        );
        assert_eq!(HistorySaved::Held.kept(), None);
        assert!(HistorySaved::Held.problem().is_some());
        assert!(
            HistorySaved::Failed("disk full".into())
                .problem()
                .is_some_and(|text| text.ends_with("disk full"))
        );
        // A full store keeps the session in the app, and says so.
        assert_eq!(HistorySaved::Full.kept(), None);
        let full = HistorySaved::Full.problem().expect("told");
        assert!(full.contains("no room left"), "{full}");
        assert!(full.contains("kept here until there is"), "{full}");
        assert!(full.ends_with("Save a backup to keep it safe."), "{full}");
        assert_eq!(HistorySaved::Newer.kept(), None);
        assert_eq!(
            HistorySaved::Newer.problem().as_deref(),
            Some(record::NEWER_LAYOUT)
        );
    }

    #[test]
    fn one_unreadable_field_does_not_reset_every_setting() {
        let mut stored = TrainingSettings::default();
        stored.playback.char_wpm_min = 12.0;
        stored.playback.char_wpm_max = 33.0;
        stored.curriculum.num_groups = 7;
        stored.band.qrn_enabled = false;
        let mut json = serde_json::to_value(&stored).unwrap();
        // Something wrote a string where a number belongs.
        json["groupTimeout"] = serde_json::json!("nonsense");
        let raw = serde_json::to_string(&json).unwrap();
        assert!(serde_json::from_str::<TrainingSettings>(&raw).is_err());

        let recovered = recover_settings(&raw);
        assert_eq!(recovered.playback.char_wpm_min, 12.0);
        assert_eq!(recovered.playback.char_wpm_max, 33.0);
        assert_eq!(recovered.curriculum.num_groups, 7);
        assert!(!recovered.band.qrn_enabled);
        // Only the unreadable field falls back to its default.
        assert_eq!(
            recovered.playback.group_timeout,
            TrainingSettings::default().playback.group_timeout
        );
    }

    /// Recovering a save takes time in proportion to its size. One bad value
    /// among a hundred thousand keys no build reads, as in a file made to
    /// hang the app, used to be tried key by key over all those kept before
    /// it: over an hour. What no build reads is carried along only so far.
    #[test]
    fn a_save_made_to_be_large_is_recovered_quickly() {
        let mut stored = serde_json::Map::new();
        for n in 0..100_000 {
            stored.insert(format!("filler{n}"), serde_json::json!(n));
        }
        stored.insert("groupTimeout".into(), serde_json::json!("nonsense"));
        stored.insert("numGroups".into(), serde_json::json!(7));
        let raw = serde_json::to_string(&stored).expect("serialize");
        let started = std::time::Instant::now();
        let settings = recover_settings(&raw);
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(10), "{took:?}");
        assert_eq!(settings.curriculum.num_groups, 7);
        assert_eq!(
            settings.playback.group_timeout,
            TrainingSettings::default().playback.group_timeout
        );
        let written = serde_json::to_string(&settings).expect("serialize").len();
        let fresh = serde_json::to_string(&TrainingSettings::default())
            .expect("serialize")
            .len();
        assert!(written < fresh + 20_000, "{written}");
    }

    /// Saves from before the receiver rework all name the old sharp filter,
    /// because every save stores its shape: they come back on today's
    /// receiver. A save made since, which chose sharp, keeps it.
    #[test]
    fn a_save_from_the_old_receiver_comes_back_on_the_new_one() {
        let old = r#"{"filterBandwidthHz":500,"filterShape":"sharp","numGroups":7}"#;
        let settings = recover_settings(old);
        let fresh = TrainingSettings::default();
        assert_eq!(settings.band.filter_shape, fresh.band.filter_shape);
        assert_eq!(settings.band.filter_width_hz, fresh.band.filter_width_hz);
        assert_eq!(
            settings.curriculum.num_groups, 7,
            "the rest of the save is kept"
        );

        let mut chosen = TrainingSettings::default();
        chosen.band.filter_shape = cw_core::FilterShape::Sharp;
        chosen.band.filter_width_hz = 400.0;
        let back = recover_settings(&serde_json::to_string(&chosen).expect("serialize"));
        assert_eq!(back.band.filter_shape, cw_core::FilterShape::Sharp);
        assert_eq!(back.band.filter_width_hz, 400.0);
    }

    /// A save from before the AGC threshold went comes back with a raised
    /// threshold as the RF gain backed off as far, which is how the AGC
    /// heard it. A threshold taken down, or one under an AGC switched off,
    /// is dropped. It is done once: the next save has no threshold.
    #[test]
    fn a_raised_agc_threshold_comes_back_as_rf_gain() {
        let rf = |raw: &str| load_settings_from(raw).band.rf_gain_db;
        let old = r#"{"agcThresholdDb":10,"rfGainDb":-5,"numGroups":7}"#;
        assert_eq!(rf(old), -15.0);
        assert_eq!(load_settings_from(old).curriculum.num_groups, 7);
        assert_eq!(rf(r#"{"agcThresholdDb":12}"#), -12.0, "from full");
        assert_eq!(
            rf(r#"{"agcThresholdDb":25,"rfGainDb":-12}"#),
            RF_GAIN_MIN_DB,
            "as far as the RF gain goes"
        );
        assert_eq!(rf(r#"{"agcThresholdDb":-8,"rfGainDb":-5}"#), -5.0);
        assert_eq!(
            rf(r#"{"agcThresholdDb":10,"rfGainDb":-5,"agcEnabled":false}"#),
            -5.0
        );
        assert_eq!(rf(r#"{"agcThresholdDb":"loud","rfGainDb":-5}"#), -5.0);

        // Once: what is saved after it reads back as it was saved.
        let upgraded = load_settings_from(old);
        let saved = serde_json::to_string(&upgraded).expect("serialize");
        assert!(!saved.contains("agcThresholdDb"));
        assert_eq!(load_settings_from(&saved), upgraded);

        // A backup reads its settings the same way.
        let backup = backup::read_backup(&format!(
            r#"{{"dustBackup":1,"settings":{old},"sessions":[]}}"#
        ))
        .expect("a backup");
        assert_eq!(backup.settings.map(|s| s.band.rf_gain_db), Some(-15.0));

        // And a merge with a copy that read the same old save keeps it,
        // beside a change made here.
        let mut ours = upgraded.clone();
        ours.curriculum.num_groups = 9;
        let merged = merge_settings(&upgraded, &ours, &load_settings_from(old));
        assert_eq!(
            (merged.band.rf_gain_db, merged.curriculum.num_groups),
            (-15.0, 9)
        );
    }

    #[test]
    fn settings_from_an_older_build_still_load() {
        // No repeat, envelope or theme fields existed then.
        let raw =
            r#"{"level":9,"charSetMode":"koch","charWpmMin":22,"charWpmMax":22,"numGroups":15}"#;
        let settings = recover_settings(raw);
        assert_eq!(settings.curriculum.level, 9);
        assert_eq!(settings.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(settings.playback.char_wpm_min, 22.0);
        assert_eq!(settings.curriculum.num_groups, 15);
        assert_eq!(settings.playback.group_repeat_min, 1);
        assert_eq!(settings.playback.keyer_wpm, 20.0);
        assert_eq!(settings.playback.group_pause_sec, 0.0);
        // Callsign mode did not exist then either, so its tier starts at one
        // rather than resetting everything around it.
        assert_eq!(settings.curriculum.callsign_level, 1);
    }

    /// The background switch has been called `qrm*`, then `receiver*`, and is
    /// now the band noise. Every save written under either old name keeps its
    /// on/off choice. The old level and the resonator model's knobs drove a
    /// sound that no longer exists, so they are dropped rather than
    /// reinterpreted — and dropping them must not cost anything else in the
    /// save. So is the filter width of those days, `filterBandwidthHz`: a
    /// width at 3 dB where the control now means 6 dB, it comes back as
    /// today's default.
    #[test]
    fn saves_from_the_old_receiver_models_still_load() {
        let raw = r#"{"qrmEnabled":false,"qrmLevel":0.75,"qrmProfile":"ringing","charWpmMin":19}"#;
        let settings = recover_settings(raw);
        assert!(!settings.band.noise_enabled);
        assert_eq!(
            settings.band.noise_level,
            TrainingSettings::default().band.noise_level
        );
        assert_eq!(settings.playback.char_wpm_min, 19.0);

        let raw = r#"{"receiverEnabled":false,"receiverLevel":0.2,"receiverProfile":"whistle",
            "receiverBackgroundGain":55,"receiverBackgroundResonance":120,
            "receiverBackgroundOffsetModDepthHz":45,"filterBandwidthHz":250,"qrnLevel":0.4}"#;
        let settings = recover_settings(raw);
        assert!(!settings.band.noise_enabled);
        assert_eq!(
            settings.band.filter_width_hz,
            cw_core::DEFAULT_FILTER_WIDTH_HZ
        );
        assert_eq!(settings.band.qrn_level, 0.4);
        assert_eq!(settings.band.filter_shape, cw_core::FilterShape::Sharp);

        // And the new ones round-trip.
        let mut fresh = TrainingSettings::default();
        fresh.band.filter_shape = cw_core::FilterShape::Soft;
        fresh.band.filter_width_hz = 250.0;
        fresh.band.noise_level = 0.8;
        let back = recover_settings(&serde_json::to_string(&fresh).expect("serialize"));
        assert_eq!(back.band.filter_shape, cw_core::FilterShape::Soft);
        assert_eq!(back.band.filter_width_hz, 250.0);
        assert_eq!(back.band.noise_level, 0.8);
    }

    /// A save written in callsign mode has to come back in callsign mode, tier
    /// and all — this is the field a user would lose on every restart if the
    /// serde default were wrong.
    #[test]
    fn a_callsign_save_comes_back_as_one() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level = 5;
        settings.curriculum.level = 12;
        let raw = serde_json::to_string(&settings).expect("settings serialize");
        let back = recover_settings(&raw);
        assert_eq!(back.curriculum.char_set_mode, CharSetMode::Callsign);
        assert_eq!(back.curriculum.callsign_level, 5);
        assert_eq!(back.curriculum.level, 12, "the Koch level rides along");
        assert_eq!(back.active_level(), 5);
    }

    #[test]
    fn unreadable_settings_fall_back_to_defaults() {
        assert_eq!(recover_settings("not json"), TrainingSettings::default());
        assert_eq!(recover_settings("[]"), TrainingSettings::default());
        assert_eq!(recover_settings(""), TrainingSettings::default());
    }

    #[test]
    fn loaded_settings_are_clamped_and_fitted() {
        let mut stored = TrainingSettings::default();
        stored.playback.char_wpm_min = 500.0;
        stored.curriculum.num_groups = 0;
        let settings = load_settings_from(&serde_json::to_string(&stored).unwrap());
        assert_eq!(settings.playback.char_wpm_min, 80.0);
        assert_eq!(settings.curriculum.num_groups, 1);
    }

    /// A mark from before marks held what their session did so far reads as
    /// it did; one whose session so far no longer reads still tells of the
    /// session cut off, and one whose settings no longer read keeps the
    /// session all the same.
    #[test]
    fn a_mark_reads_whatever_it_holds_of_its_session_so_far() {
        let open = |so_far| {
            CutOff::Marked(OpenSession {
                started_at: 5,
                run: false,
                so_far,
            })
        };
        assert_eq!(CutOff::read(br#"{"startedAt":5,"run":false}"#), open(None));
        assert_eq!(
            CutOff::read(br#"{"startedAt":5,"run":false,"soFar":{"result":7}}"#),
            open(None)
        );
        let result = SessionResult::empty_for_test();
        let raw = serde_json::json!({
            "startedAt": 5,
            "run": false,
            "soFar": { "result": result, "settings": "not settings" },
        });
        let kept = SoFar {
            result,
            settings: None,
        };
        assert_eq!(
            CutOff::read(raw.to_string().as_bytes()),
            open(Some(Box::new(kept.clone())))
        );
        let written = serde_json::to_vec(&OpenSession {
            started_at: 5,
            run: false,
            so_far: Some(Box::new(kept.clone())),
        })
        .unwrap();
        assert_eq!(CutOff::read(&written), open(Some(Box::new(kept))));
    }

    #[test]
    fn auto_level_progress_is_hidden_when_the_setting_is_off() {
        let store = memory::MemoryStore::default();
        let mut settings = TrainingSettings::default();
        settings.auto_level.auto_adjust_level = false;
        assert!(auto_progress(&store, &settings).is_none());
        settings.auto_level.auto_adjust_level = true;
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        assert!(auto_progress(&store, &settings).is_some());
    }
}
