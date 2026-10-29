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
#[cfg(test)]
pub mod memory;
#[cfg(feature = "web")]
mod web;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use cw_core::{
    AutoLevelCounters, AutoLevelProgress, CountersKey, RF_GAIN_MAX_DB, RF_GAIN_MIN_DB,
    SessionResult, TrainingSettings, auto_level_progress,
};
use serde::Deserialize;

/// How much history is kept: years of daily practice.
pub const MAX_SESSIONS: usize = 500;

pub trait Store {
    fn load_theme(&self) -> String;
    fn save_theme(&self, theme: &str);
    /// Whether the guided setup has been opened on this install: it opens by
    /// itself on a first launch only.
    fn tutorial_done(&self) -> bool;
    fn mark_tutorial_done(&self);
    /// The stored settings, clamped; `None` when nothing stored reads as
    /// settings at all: a first launch, or a store that cannot be read.
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
    fn load_auto_counters(&self, key: &CountersKey) -> AutoLevelCounters;
    fn save_auto_counters(&self, key: &CountersKey, counters: AutoLevelCounters);
    fn clear_auto_counters(&self, keys: &[CountersKey]);
    /// When a backup was last saved, in milliseconds since the epoch.
    fn last_backup(&self) -> Option<u64>;
    fn mark_backup(&self, at: u64);
    /// Whether this build can hand the user a backup file: an app on a
    /// phone has nowhere outside itself to put one.
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
}

/// A session of groups or a contest run, as it is marked open while it runs
/// ([`Store::mark_open`]): so that one the app stops under is never lost
/// without a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSession {
    /// When it started, in milliseconds since the epoch.
    pub started_at: u64,
    /// A contest run, rather than a session of groups.
    pub run: bool,
}

/// A session or run the app stopped under, as its mark tells of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutOff {
    Marked(OpenSession),
    /// The mark is there and cannot be read: cut short itself, by a power
    /// cut, say. A session was running all the same.
    Unread,
}

impl CutOff {
    /// The one to tell of, of all those cut off: the newest that can be
    /// read, or failing that, one that cannot.
    fn newest(cut: impl IntoIterator<Item = Self>) -> Option<Self> {
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

/// The sessions cut off, being looked for: already, or still, where only the
/// platform can tell which copies are running, as in a browser.
pub type CutOffTaking = Pin<Box<dyn Future<Output = Option<CutOff>>>>;

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

/// What became of a history handed to [`Store::save_sessions`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistorySaved {
    /// Stored: the newest `kept` sessions, which is all of them up to
    /// [`MAX_SESSIONS`].
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
    /// Not stored, for the reason given.
    Failed(String),
}

impl HistorySaved {
    /// How many of the newest sessions the store now holds, when it stored
    /// any.
    pub fn kept(&self) -> Option<usize> {
        match self {
            Self::Stored { kept } | Self::Squeezed { kept, .. } => Some(*kept),
            Self::Held | Self::Failed(_) => None,
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

/// Read a stored history, keeping every session that still parses, and
/// what is stored of every one that does not.
///
/// `None` means the history as a whole is unreadable — not a list at all.
/// That is different from an empty one: the caller has to set the stored copy
/// aside before anything writes over it, or the first session finished after
/// a bad write would replace years of history with itself. So too with the
/// sessions that do not parse, which the next save would leave out.
///
/// Every session is scored again as it is read, from what is stored of it, so
/// the history is on one scale however the score has been worked out over the
/// versions: sessions saved before speed counted read as they would score
/// today.
fn read_history(raw: &str) -> Option<ReadHistory> {
    // Almost always the whole list reads; only when it does not is it worth
    // reading again a session at a time to keep the ones that do.
    let (mut sessions, unread) = match serde_json::from_str::<Vec<SessionResult>>(raw) {
        Ok(sessions) => (sessions, Vec::new()),
        Err(_) => {
            let (mut sessions, mut unread) = (Vec::new(), Vec::new());
            for value in serde_json::from_str::<Vec<serde_json::Value>>(raw).ok()? {
                match SessionResult::deserialize(&value) {
                    Ok(session) => sessions.push(session),
                    Err(_) => unread.push(value),
                }
            }
            (sessions, unread)
        }
    };
    for session in &mut sessions {
        session.score = session.points();
    }
    Some(ReadHistory { sessions, unread })
}

/// The sessions of a stored history that still parse ([`read_history`]),
/// where nothing is ever written over it: a backup file.
fn parse_sessions(raw: &str) -> Option<Vec<SessionResult>> {
    read_history(raw).map(|read| read.sessions)
}

/// A stored history as it was read ([`read_history`]).
#[derive(Debug, PartialEq)]
struct ReadHistory {
    /// Every session that still parses, oldest first.
    sessions: Vec<SessionResult>,
    /// Every one that does not, as it is stored.
    unread: Vec<serde_json::Value>,
}

impl ReadHistory {
    /// The sessions that could not be read, as one JSON list to set aside;
    /// none when every one could.
    #[cfg(any(feature = "web", feature = "native-runtime", test))]
    fn unread_json(&self) -> Option<String> {
        if self.unread.is_empty() {
            return None;
        }
        serde_json::to_string(&self.unread).ok()
    }
}

/// How many copies of what could not be read a browser keeps set aside, at
/// most: the newest. Its room runs out well before many more would fit.
#[cfg(any(feature = "web", test))]
const MAX_SET_ASIDE: usize = 5;

/// Where a browser sets aside what it could not read: its `localStorage`,
/// or a test's stand-in for it.
#[cfg(any(feature = "web", test))]
trait Aside {
    /// Every key it holds; `Err` when they cannot be listed.
    fn keys(&self) -> Result<Vec<String>, ()>;
    /// What it holds under `key`; `Err` when that cannot be read.
    fn get(&self, key: &str) -> Result<Option<String>, ()>;
    /// Hold `raw` under `key`, and say whether it took.
    fn set(&self, key: &str, raw: &str) -> bool;
    fn remove(&self, key: &str);
}

/// Which copy set aside under `prefix` `key` is, counting from one: the
/// first is `prefix` itself, the ones after it `prefix_2`, `prefix_3`…
#[cfg(any(feature = "web", test))]
fn aside_number(prefix: &str, key: &str) -> Option<u64> {
    match key.strip_prefix(prefix)? {
        "" => Some(1),
        rest => {
            let digits = rest.strip_prefix('_')?;
            let number = digits.parse().ok()?;
            (digits.bytes().all(|b| b.is_ascii_digit()) && number > 1).then_some(number)
        }
    }
}

/// Keep `raw`, what could not be read of a history, set aside in `store`
/// under `prefix`, and say whether it is kept. A copy set aside before is
/// never written over, and one set aside already is not kept twice.
///
/// Each copy goes under the number after the newest, so the numbers say
/// which is newer, and only the newest [`MAX_SET_ASIDE`] are kept. One that
/// does not fit lets the oldest go, one at a time, until it does; if it
/// does not fit even then, those go back as they were. Nothing reads a
/// copy set aside back but a person, while a history that cannot be set
/// aside holds back every session finished after it: so old copies give
/// way to a new one, the oldest first.
#[cfg(any(feature = "web", test))]
fn keep_aside(store: &impl Aside, prefix: &str, raw: &str) -> bool {
    let Ok(keys) = store.keys() else {
        return false;
    };
    let mut numbered: Vec<(u64, String)> = keys
        .into_iter()
        .filter_map(|key| Some((aside_number(prefix, &key)?, key)))
        .collect();
    numbered.sort_unstable();
    let Some(next) = numbered
        .last()
        .map_or(Some(1), |(number, _)| number.checked_add(1))
    else {
        return false;
    };
    // Each copy already set aside, oldest first, and what it holds.
    let mut older = Vec::with_capacity(numbered.len());
    for (_, key) in numbered {
        match store.get(&key) {
            Ok(Some(kept)) if kept == raw => return true,
            Ok(Some(kept)) => older.push((key, kept)),
            Ok(None) => {}
            Err(()) => return false,
        }
    }
    let key = match next {
        1 => prefix.to_string(),
        n => format!("{prefix}_{n}"),
    };
    let mut gone = 0;
    while !store.set(&key, raw) {
        let Some((oldest, _)) = older.get(gone) else {
            for (key, kept) in &older {
                store.set(key, kept);
            }
            return false;
        };
        store.remove(oldest);
        gone += 1;
    }
    let first_kept = older.len().saturating_sub(MAX_SET_ASIDE - 1).max(gone);
    for (key, _) in &older[gone..first_kept] {
        store.remove(key);
    }
    true
}

/// Load settings without throwing away everything for one bad field.
///
/// A strict parse fails on a single unreadable value — a hand-edited file, or a
/// field whose type changed between versions — and the user would silently get
/// factory settings back. So fall back to applying the stored keys one at a
/// time and keeping the ones that still deserialize.
fn recover_settings(raw: &str) -> TrainingSettings {
    let Ok(serde_json::Value::Object(mut stored)) = serde_json::from_str::<serde_json::Value>(raw)
    else {
        return TrainingSettings::default();
    };
    upgrade_receiver(&mut stored);
    upgrade_agc_threshold(&mut stored);
    let stored = serde_json::Value::Object(stored);
    if let Ok(settings) = serde_json::from_value::<TrainingSettings>(stored.clone()) {
        return settings;
    }
    let serde_json::Value::Object(stored) = stored else {
        return TrainingSettings::default();
    };
    let mut kept = serde_json::Map::new();
    for (key, value) in stored {
        let mut candidate = kept.clone();
        candidate.insert(key, value);
        if serde_json::from_value::<TrainingSettings>(serde_json::Value::Object(candidate.clone()))
            .is_ok()
        {
            kept = candidate;
        }
    }
    serde_json::from_value(serde_json::Value::Object(kept)).unwrap_or_default()
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
/// and on what day.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionKey(u64, String);

impl SessionKey {
    pub fn of(session: &SessionResult) -> Self {
        Self(session.timestamp, session.date.clone())
    }
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
pub fn merge_history(
    base: &[SessionResult],
    ours: &[SessionResult],
    theirs: &[SessionResult],
) -> Vec<SessionResult> {
    fn each(sessions: &[SessionResult]) -> Vec<&SessionResult> {
        sessions.iter().collect()
    }
    let was_here = pair_sessions(&each(base), &each(ours));
    let was_there = pair_sessions(&each(base), &each(theirs));
    // Each session of `base` as it is stored now, if it still is.
    let mut stored = vec![None; base.len()];
    for (session, was) in theirs.iter().zip(&was_there) {
        if let Some(was) = was {
            stored[*was] = Some(session);
        }
    }
    let mut merged = Vec::with_capacity(theirs.len().max(ours.len()));
    let mut added_here = Vec::new();
    for (session, was) in ours.iter().zip(&was_here) {
        match was {
            None => {
                merged.push(session.clone());
                added_here.push(session);
            }
            Some(was) => match stored[*was] {
                // Deleted elsewhere.
                None => {}
                // Changed here, or else as it is stored.
                Some(stored) if *session == base[*was] => merged.push(stored.clone()),
                Some(_) => merged.push(session.clone()),
            },
        }
    }
    // Added elsewhere, unless it was added here too, as two copies
    // restoring one backup each add the same session.
    let added_there: Vec<&SessionResult> = theirs
        .iter()
        .zip(&was_there)
        .filter(|(_, was)| was.is_none())
        .map(|(session, _)| session)
        .collect();
    let before = merged.len();
    merged.extend(
        added_there
            .iter()
            .zip(pair_sessions(&added_here, &added_there))
            .filter(|(_, same)| same.is_none())
            .map(|(session, _)| (*session).clone()),
    );
    // In among the rest, by when each finished.
    if merged.len() > before {
        merged.sort_by_key(|session| session.timestamp);
    }
    merged
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

/// Store the newest of `sessions` that fit, with `set`, which writes a
/// history and says whether the store took it: as many as fit `budget`, and
/// if the store has less room than that, as many as fit in half as much,
/// and so on, down to the newest alone.
///
/// Never fewer than that: a store with no room even for the newest session
/// is full, and nothing is written. Halving used to go on below the newest
/// session's size, down to an empty list, which a store always takes; so a
/// browser short of room wrote an empty history over the whole of the one
/// it held, and said it had kept the newest none.
#[cfg(any(feature = "web", test))]
fn store_newest_that_fit(
    sessions: &[SessionResult],
    budget: usize,
    mut set: impl FnMut(&str) -> bool,
) -> HistorySaved {
    let full = || HistorySaved::Failed("this browser's storage is full.".into());
    // The room the newest session takes on its own, as a list of one.
    let newest_alone = sessions
        .last()
        .and_then(|session| serde_json::to_string(session).ok())
        .map_or(0, |json| json.len() + "[]".len());
    let mut budget = budget;
    loop {
        let (raw, kept) = newest_that_fit(sessions, budget);
        if kept == 0 && !sessions.is_empty() {
            return full();
        }
        if set(&raw) {
            return if kept < sessions.len() {
                let runs = sessions.iter().any(|session| session.contest.is_some());
                HistorySaved::Squeezed { kept, runs }
            } else {
                HistorySaved::Stored { kept }
            };
        }
        if kept <= 1 {
            return full();
        }
        budget = (raw.len() / 2).max(newest_alone);
    }
}

/// The newest of `sessions` whose JSON fits in `budget` bytes, as one JSON
/// list, and how many it holds.
///
/// A browser gives a site a few megabytes, and a session's size follows its
/// number of groups, so the cap on a browser's history is room, not count.
#[cfg(any(feature = "web", test))]
fn newest_that_fit(sessions: &[SessionResult], budget: usize) -> (String, usize) {
    let mut parts: Vec<String> = Vec::new();
    let mut used = "[]".len();
    for session in sessions.iter().rev() {
        let Ok(json) = serde_json::to_string(session) else {
            continue;
        };
        let cost = json.len() + usize::from(!parts.is_empty());
        if used + cost > budget {
            break;
        }
        used += cost;
        parts.push(json);
    }
    let kept = parts.len();
    parts.reverse();
    (format!("[{}]", parts.join(",")), kept)
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
        a.char_sampling_thompson = true;
        s.recognition = cw_core::RecognitionBands {
            fast_ms: 350,
            icr_ms: 550,
            slow_ms: 900,
        };
        let k = &mut s.contest;
        k.contest_kind = cw_core::ContestKind::Sprint;
        k.contest_style = cw_core::RunStyle::Pileup;
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
        assert_eq!(backup.settings, moved);
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
    fn at(timestamp: u64) -> SessionResult {
        SessionResult {
            timestamp,
            ..session(&format!("2026-10-{timestamp:02}"))
        }
    }

    fn stamps(sessions: &[SessionResult]) -> Vec<u64> {
        sessions.iter().map(|session| session.timestamp).collect()
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
        assert_eq!(stamps(&merge_history(&base, &ours, &theirs)), [1, 4, 5]);
        // Nothing stored elsewhere: this copy's history, as it is.
        assert_eq!(merge_history(&base, &ours, &base), ours);
        // Nothing changed here: what is stored, as it is.
        assert_eq!(merge_history(&base, &base, &theirs), theirs);
        // Both added the same session, restoring the same backup: once.
        assert_eq!(stamps(&merge_history(&[], &[at(7)], &[at(7)])), [7]);
    }

    /// A session filled in on one side — a run's log put back by a restore —
    /// is kept filled in, whichever side did it.
    #[test]
    fn a_history_merge_keeps_a_session_filled_in_on_either_side() {
        let mut filled = at(1);
        filled.accuracy = 0.5;
        let merged = merge_history(&[at(1)], std::slice::from_ref(&filled), &[at(1)]);
        assert_eq!(merged, [filled.clone()]);
        let merged = merge_history(&[at(1)], &[at(1)], std::slice::from_ref(&filled));
        assert_eq!(merged, [filled]);
    }

    /// Sessions alike in when and on what day they finished are told apart
    /// by their order: deleting one leaves the other.
    #[test]
    fn a_history_merge_tells_sessions_alike_apart() {
        let first = at(1);
        let mut second = at(1);
        second.accuracy = 0.5;
        let both = [first.clone(), second.clone()];
        let merged = merge_history(&both, std::slice::from_ref(&second), &both);
        assert_eq!(merged, [second]);
        let merged = merge_history(&both, &both, std::slice::from_ref(&first));
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
        assert_eq!(merge_history(&base, &ours, &theirs), [filled, third]);
    }

    /// The same session added on both sides is kept once, and two alike in
    /// their key but not in what they hold are both kept.
    #[test]
    fn sessions_added_on_both_sides_are_kept_once() {
        let mut other = at(7);
        other.accuracy = 0.5;
        assert_eq!(
            merge_history(&[], &[at(7)], &[at(7), other.clone()]),
            [at(7), other]
        );
    }

    pub(super) fn session(date: &str) -> SessionResult {
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
    fn a_corrupt_session_is_dropped_instead_of_losing_the_file() {
        let good = serde_json::to_string(&session("2026-09-01")).unwrap();
        let raw = format!("[{good}, {{\"date\": \"nonsense\"}}]");
        let recovered = parse_sessions(&raw).expect("a list");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].date, "2026-09-01");
        // What it dropped is kept as it was stored, to be set aside.
        let read = read_history(&raw).expect("a list");
        assert_eq!(read.unread, [serde_json::json!({ "date": "nonsense" })]);
        assert_eq!(
            read.unread_json().as_deref(),
            Some(r#"[{"date":"nonsense"}]"#)
        );
        assert_eq!(
            read_history(&format!("[{good}]")).unwrap().unread_json(),
            None
        );
        // A whole list reads in one go.
        let both = format!("[{good},{good}]");
        assert_eq!(parse_sessions(&both).map(|s| s.len()), Some(2));
        // A file that is not even a list is unreadable, which is not the same
        // as empty: the caller has to keep it safe.
        assert!(parse_sessions("{}").is_none());
        assert!(parse_sessions("").is_none());
        assert_eq!(parse_sessions("[]"), Some(Vec::new()));
    }

    /// A browser's store, as far as setting aside goes: `room` characters
    /// in all, as a browser's quota has, and keys of other kinds beside.
    struct Shelf {
        held: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
        room: std::cell::Cell<usize>,
        unreadable: std::cell::Cell<bool>,
    }

    impl Shelf {
        fn with_room(room: usize) -> Self {
            let shelf = Self {
                held: Default::default(),
                room: std::cell::Cell::new(room),
                unreadable: Default::default(),
            };
            shelf.set("history", "[]");
            shelf
        }

        /// What is set aside, by key.
        fn aside(&self) -> Vec<(String, String)> {
            let held = self.held.borrow();
            held.iter()
                .filter(|(key, _)| key.starts_with("aside"))
                .map(|(key, raw)| (key.clone(), raw.clone()))
                .collect()
        }
    }

    impl Aside for Shelf {
        fn keys(&self) -> Result<Vec<String>, ()> {
            if self.unreadable.get() {
                return Err(());
            }
            Ok(self.held.borrow().keys().cloned().collect())
        }

        fn get(&self, key: &str) -> Result<Option<String>, ()> {
            if self.unreadable.get() {
                return Err(());
            }
            Ok(self.held.borrow().get(key).cloned())
        }

        fn set(&self, key: &str, raw: &str) -> bool {
            let mut held = self.held.borrow_mut();
            let used: usize = held
                .iter()
                .filter(|(held, _)| *held != key)
                .map(|(_, raw)| raw.len())
                .sum();
            if used + raw.len() > self.room.get() {
                return false;
            }
            held.insert(key.into(), raw.into());
            true
        }

        fn remove(&self, key: &str) {
            self.held.borrow_mut().remove(key);
        }
    }

    fn shelved(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, raw)| (key.to_string(), raw.to_string()))
            .collect()
    }

    /// What could not be read is set aside under a key of its own: never
    /// over a copy set aside before, and never twice.
    #[test]
    fn what_could_not_be_read_is_set_aside_beside_what_was_before() {
        let shelf = Shelf::with_room(usize::MAX);
        assert!(keep_aside(&shelf, "aside", "first"));
        assert!(keep_aside(&shelf, "aside", "second"));
        assert!(keep_aside(&shelf, "aside", "first"), "kept already");
        assert_eq!(
            shelf.aside(),
            shelved(&[("aside", "first"), ("aside_2", "second")])
        );
        // A store that cannot be read: not kept, and nothing changed.
        shelf.unreadable.set(true);
        assert!(!keep_aside(&shelf, "aside", "third"));
        shelf.unreadable.set(false);
        assert_eq!(shelf.aside().len(), 2);
        assert_eq!(aside_number("aside", "asidea"), None);
        assert_eq!(aside_number("aside", "aside_1"), None);
        assert_eq!(aside_number("aside", "aside_+3"), None);
        assert_eq!(aside_number("aside", "aside_12"), Some(12));
    }

    /// Only the newest copies are kept. Every slot a browser had used to
    /// stay full for good, and the next history that could not be read
    /// could not be set aside, so nothing was saved over it again.
    #[test]
    fn only_the_newest_copies_set_aside_are_kept() {
        let shelf = Shelf::with_room(usize::MAX);
        // As twenty slots were filled before there was a limit.
        shelf.set("aside", "1");
        for n in 2..=20 {
            shelf.set(&format!("aside_{n}"), &n.to_string());
        }
        assert!(keep_aside(&shelf, "aside", "new"));
        assert_eq!(
            shelf.aside(),
            shelved(&[
                ("aside_17", "17"),
                ("aside_18", "18"),
                ("aside_19", "19"),
                ("aside_20", "20"),
                ("aside_21", "new"),
            ])
        );
        assert!(keep_aside(&shelf, "aside", "newer"));
        assert_eq!(shelf.aside().len(), MAX_SET_ASIDE);
        assert_eq!(
            shelf.aside().last(),
            Some(&("aside_22".into(), "newer".into()))
        );
    }

    /// A copy that does not fit lets the oldest go until it does; one that
    /// would not fit even then leaves every copy as it was.
    #[test]
    fn a_copy_set_aside_makes_room_by_letting_the_oldest_go() {
        // The history's own room, and a little short of room for three.
        let shelf = Shelf::with_room(2 + 4 + 4 + 5);
        assert!(keep_aside(&shelf, "aside", "aaaa"));
        assert!(keep_aside(&shelf, "aside", "bbbb"));
        assert!(keep_aside(&shelf, "aside", "cccccc"), "room made");
        assert_eq!(
            shelf.aside(),
            shelved(&[("aside_2", "bbbb"), ("aside_3", "cccccc")])
        );
        assert!(!keep_aside(&shelf, "aside", "far too long for it"));
        assert_eq!(
            shelf.aside(),
            shelved(&[("aside_2", "bbbb"), ("aside_3", "cccccc")]),
            "put back as they were"
        );
    }

    /// A browser's history is capped by room. Large sessions — two hundred
    /// groups each — used to overflow the quota by count, and the oldest half
    /// went at a time until something fitted, with nobody told.
    /// A history saved under an older score reads on today's scale: every
    /// session is scored again from what is stored of it.
    #[test]
    fn a_history_reads_scored_the_way_sessions_score_today() {
        let mut stored = session("2026-01-01");
        stored.groups = vec![cw_core::GroupResult {
            sent: "KMURE".into(),
            received: "KMURE".into(),
            correct: true,
        }];
        stored.group_timings = vec![cw_core::SessionTiming {
            time_to_complete_ms: 500.0,
            char_wpm: Some(35.0),
            effective_wpm: None,
            sends: 1,
            ..cw_core::SessionTiming::default()
        }];
        stored.char_wpm = 35.0;
        stored.effective_wpm = 35.0;
        stored.score = 1.0;
        let raw = serde_json::to_string(&[stored.clone()]).unwrap();
        let back = parse_sessions(&raw).expect("a list");
        assert_eq!(back[0].score, stored.points());
        assert!(back[0].score > 1.0, "{}", back[0].score);
    }

    /// A contest run a later version wrote reads as a run, and goes back to
    /// the store as it was written: a callsign session with its mark, and
    /// the log this version cannot read yet.
    #[test]
    fn a_contest_run_from_a_later_version_is_kept_whole() {
        let mut written = serde_json::to_value(session("2026-10-20")).unwrap();
        written["charSetMode"] = serde_json::json!("callsign");
        written["alphabetFingerprint"] = serde_json::json!("contest:wpx");
        written["contest"] = serde_json::json!({ "v": 9, "laterKey": [1, 2.5, "x"] });
        let raw = serde_json::to_string(&[written.clone()]).unwrap();
        let back = parse_sessions(&raw).expect("a list");
        assert_eq!(back[0].char_set_mode, CharSetMode::Contest);
        let (saved, kept) = newest_that_fit(&back, usize::MAX);
        assert_eq!(kept, 1);
        let saved: Vec<serde_json::Value> = serde_json::from_str(&saved).unwrap();
        assert_eq!(saved[0]["charSetMode"], "callsign");
        assert_eq!(saved[0]["alphabetFingerprint"], "contest:wpx");
        assert_eq!(saved[0]["contest"], written["contest"]);
    }

    #[test]
    fn only_the_newest_sessions_that_fit_are_kept() {
        let all: Vec<SessionResult> = (0..10)
            .map(|i| session(&format!("2026-01-{i:02}")))
            .collect();
        let one = serde_json::to_string(&all[0]).unwrap().len();
        let (raw, kept) = newest_that_fit(&all, 2 + 3 * one + 2);
        assert_eq!(kept, 3);
        let back = parse_sessions(&raw).expect("a list");
        assert_eq!(
            back.iter().map(|s| s.date.as_str()).collect::<Vec<_>>(),
            ["2026-01-07", "2026-01-08", "2026-01-09"],
            "the newest, oldest first"
        );
        assert!(raw.len() <= 2 + 3 * one + 2);

        let (raw, kept) = newest_that_fit(&all, usize::MAX);
        assert_eq!(
            (kept, parse_sessions(&raw).map(|s| s.len())),
            (10, Some(10))
        );
        assert_eq!(newest_that_fit(&all, 3), ("[]".to_string(), 0));
    }

    /// A store short of room keeps the newest sessions that fit, down to
    /// the newest alone, and with no room even for that writes nothing:
    /// halving its room used to end at an empty list, which went over the
    /// whole history as "the newest 0".
    #[test]
    fn a_store_short_of_room_never_writes_an_empty_history() {
        let mut all: Vec<SessionResult> = (0..5)
            .map(|i| session(&format!("2026-01-{i:02}")))
            .collect();
        // The newest is far larger than the others, as a long run is.
        all[4].alphabet_fingerprint = "x".repeat(20_000);
        let newest = serde_json::to_string(&all[4]).unwrap().len() + 2;
        let writes = std::cell::RefCell::new(Vec::<String>::new());
        let written = &writes;
        let with_room = |room: usize| {
            move |raw: &str| {
                written.borrow_mut().push(raw.to_string());
                raw.len() <= room
            }
        };
        // Room for the newest alone, but not twice it.
        let saved = store_newest_that_fit(&all, usize::MAX, with_room(newest + 10));
        assert_eq!(
            saved,
            HistorySaved::Squeezed {
                kept: 1,
                runs: false
            }
        );
        let last = written.borrow().last().cloned().unwrap();
        assert_eq!(parse_sessions(&last).map(|s| s.len()), Some(1));
        assert!(written.borrow().iter().all(|raw| raw != "[]"));
        // No room even for that: nothing is written over what is there.
        written.borrow_mut().clear();
        let saved = store_newest_that_fit(&all, usize::MAX, with_room(newest - 1));
        assert!(matches!(saved, HistorySaved::Failed(_)), "{saved:?}");
        assert!(written.borrow().iter().all(|raw| raw != "[]"));
        // Room for everything: everything.
        let saved = store_newest_that_fit(&all, usize::MAX, |_| true);
        assert_eq!(saved, HistorySaved::Stored { kept: 5 });
        // A history every session was deleted from is stored empty.
        let saved = store_newest_that_fit(&[], usize::MAX, |raw| raw == "[]");
        assert_eq!(saved, HistorySaved::Stored { kept: 0 });
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
        assert_eq!(backup.settings.band.rf_gain_db, -15.0);

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
