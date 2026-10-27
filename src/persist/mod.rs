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

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use cw_core::{
    AutoLevelCounters, AutoLevelProgress, CountersKey, SessionResult, TrainingSettings,
    auto_level_progress,
};

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
    fn save_settings(&self, settings: &TrainingSettings);
    /// Every stored session that can still be read, oldest first.
    fn load_sessions(&self) -> Vec<SessionResult>;
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
    /// on an iPhone. Says where it went, or why it could not go.
    fn save_backup_file(&self, name: &str, contents: &str) -> BackupSaving;
    /// Ask the platform to keep what is stored rather than clear it when it
    /// needs room. Only a browser clears a store on its own, and it may ask
    /// the user first.
    fn ask_to_keep(&self) {}
}

/// A backup file on its way: saved already, or still being saved where only
/// the platform's own thread can save it, as on Android.
pub type BackupSaving = Pin<Box<dyn Future<Output = Result<String, String>>>>;

/// A backup file saved, or not, already: everywhere but Android.
#[cfg_attr(all(target_os = "android", not(test)), allow(dead_code))]
fn saved_now(result: Result<String, String>) -> BackupSaving {
    Box::pin(std::future::ready(result))
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
    /// out of the way, so it is kept as it is rather than written over.
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

/// Read a stored history, keeping every session that still parses.
///
/// `None` means the history as a whole is unreadable — not a list at all.
/// That is different from an empty one: the caller has to set the stored copy
/// aside before anything writes over it, or the first session finished after
/// a bad write would replace years of history with itself.
///
/// Every session is scored again as it is read, from what is stored of it, so
/// the history is on one scale however the score has been worked out over the
/// versions: sessions saved before speed counted read as they would score
/// today.
fn parse_sessions(raw: &str) -> Option<Vec<SessionResult>> {
    // Almost always the whole list reads; only when it does not is it worth
    // reading again a session at a time to keep the ones that do.
    let mut sessions = match serde_json::from_str::<Vec<SessionResult>>(raw) {
        Ok(sessions) => sessions,
        Err(_) => serde_json::from_str::<Vec<serde_json::Value>>(raw)
            .ok()?
            .into_iter()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect(),
    };
    for session in &mut sessions {
        session.score = session.points();
    }
    Some(sessions)
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
        p.lock_input_during_group_playback = false;
        p.paddle_swap = true;
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
        b.agc_threshold_db = 12.0;
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
        a.recognition_fast_ms = 350;
        a.recognition_icr_ms = 550;
        a.recognition_slow_ms = 900;
        let k = &mut s.contest;
        k.contest_kind = cw_core::ContestKind::Sprint;
        k.contest_style = cw_core::RunStyle::Pileup;
        k.contest_habits = cw_core::CallerHabits::Rough;
        k.contest_flutter = true;
        k.contest_minutes = 25;
        k.contest_show_copy = true;
        k.contest_tu_call = cw_core::TuCall::Every;
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
        // A whole list reads in one go.
        let both = format!("[{good},{good}]");
        assert_eq!(parse_sessions(&both).map(|s| s.len()), Some(2));
        // A file that is not even a list is unreadable, which is not the same
        // as empty: the caller has to keep it safe.
        assert!(parse_sessions("{}").is_none());
        assert!(parse_sessions("").is_none());
        assert_eq!(parse_sessions("[]"), Some(Vec::new()));
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
            recognition_ms: Vec::new(),
            typed_along: false,
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
