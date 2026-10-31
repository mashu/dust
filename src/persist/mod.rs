//! What the app keeps between launches: the settings, the theme, the history
//! of sessions and the auto-level counters — and the backup file the user
//! keeps of them, outside the store ([`backup`]).
//!
//! One [`Store`] per platform — files on desktop and mobile, a browser's
//! database and `localStorage` in a browser — built once and handed to the
//! app, which never asks which one it has. Reading is forgiving: a store
//! that cannot read something gives back the most it can, and never writes
//! over what it could not read.

#[cfg(all(target_os = "android", not(test)))]
mod android;
pub mod backup;
#[cfg(feature = "native-runtime")]
mod desktop;
#[cfg(feature = "native-runtime")]
mod files;
#[cfg(any(feature = "web", test))]
mod idb;
mod legacy;
#[cfg(test)]
pub mod memory;
pub mod record;
#[cfg(feature = "web")]
mod web;
#[cfg(feature = "native-runtime")]
mod worker;

#[cfg(all(test, feature = "native-runtime"))]
pub(crate) use desktop::tests::TempDir;

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use cw_core::{
    AutoLevelCounters, AutoLevelProgress, CountersKey, RF_GAIN_MAX_DB, RF_GAIN_MIN_DB,
    SessionResult, TrainingSettings, auto_level_progress,
};
use serde::Deserialize;

use record::{HistoryChange, HistoryLoad, Pending, Record, RecordId};

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
    /// Hand `backup` to the user as a file called `name`, outside the
    /// store, somewhere they can reach it: a download in a browser, a file
    /// in Downloads on a computer or an Android phone, one in the Files app
    /// on an iPhone. It holds every session the store holds, each as it is
    /// stored, written a session at a time where the store can
    /// ([`backup::BackupFile`]). Says what became of it, in words for the
    /// user, or why it could not go: where the store could not read all it
    /// holds, nothing went, and the backup is wanted again holding every
    /// session the app does ([`backup::NotSaved::Unread`]).
    fn save_backup(&self, name: &str, backup: backup::BackupFile) -> BackupSaving;
    /// Whether a backup file this store says it saved is known to be
    /// saved. A browser hands a download over and hears no more of it:
    /// blocked or turned down, it says nothing.
    fn confirms_backups(&self) -> bool {
        true
    }
    /// Whether the history has grown past what the platform's own backup of
    /// the app takes, or nearly: then only a backup file the user saves
    /// keeps all of it, should the device be lost or replaced. Android
    /// backs up no app of more than 25 MB at all.
    fn outgrows_device_backup(&self) -> bool {
        false
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
    /// The whole history, by record ([`record`]): moved first, where an
    /// older build kept it whole, and with whatever an older build stored
    /// there since taken up.
    fn load_history(&self) -> HistoryLoad;
    /// Make `change` to the history, and say what became of it. A store
    /// that cannot make it all makes none of it that would lose a session:
    /// what it stored stays stored, and nothing is deleted before what
    /// fills it in is stored.
    fn change_history(&self, change: HistoryChange) -> Pending<HistorySaved>;
    /// Every record stored, those deleted left out: what another copy of
    /// the app stored and deleted shows up here. None where the store
    /// cannot be listed, which is no word that anything is missing.
    fn history_listing(&self) -> Pending<Option<Vec<RecordId>>>;
    /// The records `ids`, as they are stored. One that cannot be read
    /// comes back as its id.
    fn read_records(&self, ids: Vec<RecordId>) -> Pending<Vec<Result<Record, RecordId>>>;
    /// Which of `ids` were deleted by hand: of those missing from the
    /// listing, the ones to let go. Any other was lost in passing, and is
    /// to be stored again: missing is never deleted.
    fn forgotten(&self, ids: Vec<RecordId>) -> Pending<Vec<RecordId>>;
    /// Whether the history may have changed, elsewhere, since this copy
    /// last read or listed it: whether it is worth listing again.
    fn history_changed(&self) -> bool;
    /// A wait that ends as another copy of the app tells of a change it
    /// made to the history, so that a screen showing it takes the change up
    /// at once. None where no copy tells, as only a browser's tabs do: the
    /// history is looked at again as a screen that shows it opens, and as
    /// the page comes back into sight.
    fn history_news(&self) -> Option<Pending<()>> {
        None
    }
    /// Wait, a moment at most, for every change asked of the history so
    /// far to be made: as the app closes, where the store makes them on a
    /// thread of its own, which the app's going would cut short.
    fn flush_history(&self) {}
}

/// A session of groups or a contest run, as it is marked open while it runs
/// ([`Store::mark_open`]): so that one the app stops under is never lost
/// without a word, nor what it had done by then.
///
/// A mark also holds what the history has yet to store: a session is
/// stored as it ends, and the mark goes only once the store has taken it,
/// so a session the store has no room for, or answers for only later, is
/// kept from the mark should the app stop first.
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
    /// copy of it no longer reads. Once it has ended, as it was ended, until
    /// the store has taken it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "read_or_none"
    )]
    pub so_far: Option<Box<SoFar>>,
    /// Sessions finished before, here, that the store has yet to take, as
    /// they were kept: their level counted already. Builds before it ignore
    /// it, and a session of it that no longer reads is passed over.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_each"
    )]
    pub unstored: Vec<SessionResult>,
    /// Its session or run is over, and the mark is held only for what the
    /// store has yet to take.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub over: bool,
}

impl OpenSession {
    /// A session (`run` false) or a contest run starting `now`, nothing
    /// done yet.
    pub fn starting(run: bool, now: u64) -> Self {
        Self {
            started_at: now,
            run,
            so_far: None,
            unstored: Vec::new(),
            over: false,
        }
    }
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

/// A list in a mark, each of it that reads: one that does not is passed
/// over, and the rest is kept all the same. A list that is no list reads as
/// none.
fn read_each<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    let serde_json::Value::Array(each) = raw else {
        return Ok(Vec::new());
    };
    Ok(each
        .into_iter()
        .filter_map(|one| serde_json::from_value(one).ok())
        .collect())
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

    /// The sessions finished before it that the store had yet to take.
    pub fn unstored(&self) -> &[SessionResult] {
        match self {
            Self::Marked(open) => &open.unstored,
            Self::Unread => &[],
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

/// A backup file on its way: saved already, as a browser hands one over, or
/// still being written on the history's own thread, as on a computer or a
/// phone.
pub type BackupSaving = Pin<Box<dyn Future<Output = Result<String, backup::NotSaved>>>>;

/// A backup file saved, or not, already: as a test's store hands one over.
#[cfg(test)]
fn saved_now(result: Result<String, backup::NotSaved>) -> BackupSaving {
    Box::pin(std::future::ready(result))
}

/// What the user is told of a backup file saved as `place`, holding what
/// `backed` says it holds: the sessions it could not, where any.
#[cfg(any(feature = "native-runtime", test))]
fn saved_as(place: &str, backed: backup::BackedUp) -> String {
    match backed.told() {
        Some(told) => format!("Backup saved as {place}. {told}"),
        None => format!("Backup saved as {place}."),
    }
}

/// Why settings were not stored: those stored could not be read, nor set
/// aside, when they were last read, so they are kept as they are rather
/// than written over. In words that follow "could not be saved:".
#[cfg(any(feature = "web", feature = "native-runtime"))]
const SETTINGS_HELD: &str = "the settings stored could not be read, so they were left as they are";

/// What became of a change made to the history by record
/// ([`record::HistoryChange`]).
///
/// Whatever was not stored stays in the app, and in the mark of the session
/// it came from ([`OpenSession::unstored`]), and is stored with the next
/// change the store takes: nothing is let go for want of room, but where a
/// browser's page storage keeps the history whole ([`HistorySaved::Squeezed`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistorySaved {
    /// Stored: `kept` is how many records the change stored.
    Stored { kept: usize },
    /// Stored, but the browser's page storage, where it keeps the history
    /// whole for want of a database ([`legacy::BlobHistory`]), had room for
    /// only the newest `kept` sessions: `trimmed` are the oldest, which it
    /// no longer holds. Records have no such limit. `runs` says the history
    /// held a contest run, whose log takes more room than a session of
    /// groups.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    Squeezed {
        kept: usize,
        runs: bool,
        trimmed: Vec<RecordId>,
    },
    /// Not stored. The history a browser keeps whole could not be read, nor
    /// moved out of the way, when it was last read, so it is kept as it is
    /// rather than written over.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    Held,
    /// Not stored: the store has no room left. The session is kept here,
    /// in the app, until it has.
    Full,
    /// Not stored: a newer Dust keeps the history here in a way this one
    /// would harm, so it writes nothing to it ([`record::NEWER_LAYOUT`]).
    Newer,
    /// Not stored, for the reason given.
    Failed(String),
}

impl HistorySaved {
    /// How many records the store took, when it took the change at all.
    pub fn kept(&self) -> Option<usize> {
        match self {
            Self::Stored { kept } | Self::Squeezed { kept, .. } => Some(*kept),
            Self::Held | Self::Full | Self::Newer | Self::Failed(_) => None,
        }
    }

    /// What to tell the user of a session kept, when there is anything to
    /// tell.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Stored { .. } => None,
            Self::Squeezed { kept, runs, .. } => {
                let why = if *runs {
                    " Long contest runs take more room."
                } else {
                    ""
                };
                Some(format!(
                    "This browser keeps your history in its small page storage, which has room for only your newest {kept} sessions; older ones were let go.{why}"
                ))
            }
            Self::Held => Some(
                "Your saved history could not be read, so it was left as it is: this session is kept here until it can be saved. Save a backup to keep it safe."
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
            Self::Failed(reason) => Some(format!(
                "This session could not be saved: {}. It is kept here, and saved as soon as it can be.",
                reason.trim_end_matches('.')
            )),
        }
    }
}

#[cfg(feature = "web")]
pub use web::preload;

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

/// Start reading the history as the app starts, on a thread of its own, so
/// that the app's window comes up meanwhile and opens on it: the store the
/// app opens takes what was read ([`default_store`]). A browser has its
/// own, which the page waits on before the app starts ([`web::preload`]).
#[cfg(feature = "native-runtime")]
pub fn preload() {
    desktop::preload();
}

/// The store a computer or phone keeps, its files under `root` and no
/// backups: for a test of the app on the files themselves, which are read
/// and written then and there.
#[cfg(all(test, feature = "native-runtime"))]
pub fn files_at(root: &std::path::Path) -> Rc<dyn Store> {
    Rc::new(desktop::DesktopStore::at(root))
}

/// The same, read and written on a thread of their own, as the app's are.
#[cfg(all(test, feature = "native-runtime"))]
pub fn files_on_worker_at(root: &std::path::Path) -> Rc<dyn Store> {
    Rc::new(desktop::DesktopStore::on_worker_at(root))
}

/// The same, saving backup files into `folder`.
#[cfg(all(test, feature = "native-runtime"))]
pub fn files_on_worker_backing_up(
    root: &std::path::Path,
    folder: &std::path::Path,
) -> Rc<dyn Store> {
    Rc::new(desktop::DesktopStore::on_worker_at(root).with_backup_folder(folder))
}

/// How the auto-level run for `settings` stands.
pub fn auto_progress(store: &dyn Store, settings: &TrainingSettings) -> Option<AutoLevelProgress> {
    auto_level_progress(
        settings,
        store.load_auto_counters(&CountersKey::of(settings)),
    )
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
    pub fn new(timestamp: u64, date: String) -> Self {
        Self(timestamp, date)
    }
}

/// A session deleted by hand, as the store remembers it beside its record
/// (a tombstone): what tells a session another copy of the app deleted from
/// one lost in passing, which is stored again. Kept for good: a copy left
/// open on the history from before it is never too old to be told.
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

    /// What became of a session is said plainly: that it is kept here
    /// until it is stored, where it is not; and only a browser's page
    /// storage, by name, ever says older sessions were let go.
    #[test]
    fn what_became_of_a_history_is_said_plainly() {
        assert_eq!(HistorySaved::Stored { kept: 4 }.problem(), None);
        let squeezed = |runs| HistorySaved::Squeezed {
            kept: 40,
            runs,
            trimmed: Vec::new(),
        };
        assert_eq!(squeezed(false).kept(), Some(40));
        let told = squeezed(false).problem().expect("told");
        assert!(told.contains("page storage"), "{told}");
        assert!(
            told.contains("newest 40") && !told.contains("contest"),
            "{told}"
        );
        assert!(
            squeezed(true)
                .problem()
                .is_some_and(|text| text.ends_with("Long contest runs take more room."))
        );
        assert_eq!(HistorySaved::Held.kept(), None);
        let held = HistorySaved::Held.problem().expect("told");
        assert!(held.contains("could not be read"), "{held}");
        assert!(held.contains("kept here until it can be saved"), "{held}");
        let failed = HistorySaved::Failed("disk full.".into())
            .problem()
            .expect("told");
        assert_eq!(
            failed,
            "This session could not be saved: disk full. It is kept here, and saved as soon as it can be."
        );
        for kept_here in [HistorySaved::Full, HistorySaved::Held, HistorySaved::Newer] {
            let told = kept_here.problem().expect("told");
            assert!(
                !told.contains("let go") && !told.contains("newest"),
                "{told}"
            );
        }
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
                so_far,
                ..OpenSession::starting(false, 5)
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
            so_far: Some(Box::new(kept.clone())),
            ..OpenSession::starting(false, 5)
        })
        .unwrap();
        assert_eq!(CutOff::read(&written), open(Some(Box::new(kept))));
    }

    /// A mark holds the sessions finished before its own that the store
    /// has yet to take, and whether its own is over: each that reads is
    /// kept, and a mark without them, as every build before them wrote,
    /// reads as holding none. One that holds none writes neither, so it
    /// reads in those builds as it always did.
    #[test]
    fn a_mark_carries_the_sessions_the_store_has_yet_to_take() {
        let first = session("2026-10-01");
        let second = session("2026-10-02");
        let open = OpenSession {
            unstored: vec![first.clone(), second.clone()],
            over: true,
            ..OpenSession::starting(false, 5)
        };
        let written = serde_json::to_vec(&open).expect("a mark writes");
        let read = CutOff::read(&written);
        assert_eq!(read, CutOff::Marked(open));
        assert_eq!(read.unstored(), [first.clone(), second]);
        assert_eq!(CutOff::Unread.unstored(), []);

        let mut raw: serde_json::Value = serde_json::from_slice(&written).expect("JSON");
        raw["unstored"][1] = serde_json::json!({ "date": 7 });
        let CutOff::Marked(read) = CutOff::read(raw.to_string().as_bytes()) else {
            panic!("a mark");
        };
        assert_eq!(read.unstored, [first], "the rest kept");
        assert!(read.over);
        raw["unstored"] = serde_json::json!("not a list");
        let CutOff::Marked(read) = CutOff::read(raw.to_string().as_bytes()) else {
            panic!("a mark");
        };
        assert!(read.unstored.is_empty());

        let plain = serde_json::to_string(&OpenSession::starting(true, 5)).expect("writes");
        assert_eq!(plain, r#"{"startedAt":5,"run":true}"#);
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
