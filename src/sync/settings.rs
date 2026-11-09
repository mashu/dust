//! Everything a device uses beside its history, synced, so that every
//! device synced is set up as every other ([`Using`]): the settings, every
//! one of them, how loud and how the paddles key among them; the counts
//! auto level keeps towards the next level, run by run; the theme; and
//! whether the guided setup and the word on the paddle keys have been
//! shown.
//!
//! Each device keeps a file of its own in the drive, holding all of it,
//! each key with when it was last changed, and takes the newest of each
//! key from every device's file and its own ([`merge`]). A device writes
//! only its own file, so two that sync at once never write over each
//! other; and a setting changed on one device and another on another both
//! come through. The settings keep their own names; the counters go by
//! [`LEVEL`] and their run's key, and the rest by `mark:`, which no
//! setting's name holds.
//!
//! A device that joins a sync the drive holds already takes all of it
//! there, as restoring a backup takes a backup's settings, and adds nothing
//! of its own until it changes something: it is the drive's that are
//! wanted on it.

use std::collections::BTreeMap;

use cw_core::{AutoLevelCounters, CountersKey, TrainingSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::persist::Store;

/// What every device's settings file is called before its keyed hash
/// ([`super::seal::Keys::name`]).
pub const SETTINGS_PREFIX: &str = "dust-s-";
const SETTINGS_LAYOUT: u32 = 1;

/// What each run of auto-level counters goes by, before its key.
pub const LEVEL: &str = "level:";
/// The theme chosen.
pub const THEME: &str = "mark:theme";
/// The guided setup has been opened.
pub const TOUR: &str = "mark:tour";
/// The word on the paddle keys has been shown.
pub const PADDLE_KEYS: &str = "mark:paddleKeys";

/// A key, and when it was last changed, in milliseconds since the epoch:
/// zero for one never changed since this device took up sync. One taken
/// away, as Contest's `contestActive` is when another character set is
/// chosen, or a run of counters a level reset clears, is null.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stamped {
    pub v: Value,
    pub at: u64,
}

/// Every key, stamped.
pub type StampedSettings = BTreeMap<String, Stamped>;

/// A device's settings file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsFile {
    pub dust_settings: u32,
    pub device: String,
    pub at: u64,
    pub keys: StampedSettings,
}

impl SettingsFile {
    pub fn new(device: &str, at: u64, keys: StampedSettings) -> Self {
        Self {
            dust_settings: SETTINGS_LAYOUT,
            device: device.to_string(),
            at,
            keys,
        }
    }

    /// The file `raw` holds, if it is one this build reads.
    pub fn read(raw: &[u8]) -> Option<Self> {
        let file: Self = serde_json::from_slice(raw).ok()?;
        (file.dust_settings == SETTINGS_LAYOUT).then_some(file)
    }
}

/// `settings` as the map of keys a save holds.
pub fn as_keys(settings: &TrainingSettings) -> Map<String, Value> {
    match serde_json::to_value(settings) {
        Ok(Value::Object(keys)) => keys,
        _ => Map::new(),
    }
}

/// The settings' own of `keys`, the counters and the marks left out.
pub fn settings_keys(keys: &Map<String, Value>) -> Map<String, Value> {
    keys.iter()
        .filter(|(key, _)| !key.contains(':'))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// What a device uses beside its history.
#[derive(Clone, Debug, PartialEq)]
pub struct Using {
    pub settings: TrainingSettings,
    pub counters: Vec<(CountersKey, AutoLevelCounters)>,
    pub theme: String,
    pub tour_done: bool,
    pub told_of_paddle_keys: bool,
}

impl Using {
    /// What `store` holds of it, beside `settings`, the app's as they are.
    pub fn of(store: &dyn Store, settings: &TrainingSettings) -> Self {
        Self {
            settings: settings.clone(),
            counters: store.every_auto_counter(),
            theme: store.load_theme(),
            tour_done: store.tutorial_done(),
            told_of_paddle_keys: store.told_of_paddle_keys(),
        }
    }

    /// All of it as the keys sync stamps. A mark not yet set is no key:
    /// one is only ever set, never unset, so none is sent until it is.
    pub fn keys(&self) -> Map<String, Value> {
        let mut keys = as_keys(&self.settings);
        for (key, counters) in &self.counters {
            if let Ok(value) = serde_json::to_value(counters) {
                keys.insert(format!("{LEVEL}{}", key.as_str()), value);
            }
        }
        if !self.theme.is_empty() {
            keys.insert(THEME.into(), Value::String(self.theme.clone()));
        }
        if self.tour_done {
            keys.insert(TOUR.into(), Value::Bool(true));
        }
        if self.told_of_paddle_keys {
            keys.insert(PADDLE_KEYS.into(), Value::Bool(true));
        }
        keys
    }

    /// The same, with what a sync `took` taken up.
    pub fn with(&self, took: &Taken) -> Self {
        let mut counters: BTreeMap<CountersKey, AutoLevelCounters> =
            self.counters.iter().cloned().collect();
        for key in &took.cleared {
            counters.remove(key);
        }
        counters.extend(took.counters.iter().cloned());
        Self {
            settings: took
                .settings
                .clone()
                .unwrap_or_else(|| self.settings.clone()),
            counters: counters.into_iter().collect(),
            theme: took.theme.clone().unwrap_or_else(|| self.theme.clone()),
            tour_done: self.tour_done || took.tour_done,
            told_of_paddle_keys: self.told_of_paddle_keys || took.told_of_paddle_keys,
        }
    }
}

/// What a sync takes up here of what the drive and this device come to:
/// each part only where it differs from what is used here now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Taken {
    pub settings: Option<TrainingSettings>,
    /// Runs of counters to store, each as it is to be.
    pub counters: Vec<(CountersKey, AutoLevelCounters)>,
    /// Runs of counters to clear, as another device cleared them.
    pub cleared: Vec<CountersKey>,
    pub theme: Option<String>,
    pub tour_done: bool,
    pub told_of_paddle_keys: bool,
}

impl Taken {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// This device's keys, `here`, stamped: each that changed since the last
/// sync, from `base`, as of `now`; each other as it was stamped then, in
/// `stamps`. One `base` had and `here` has not is taken away as of now.
///
/// A change is stamped after the stamp `stamps` holds of its key, where
/// that is `now` or later: made after the change this device last took up
/// of it, it is the newer, though another device's clock is ahead of this
/// one's.
pub fn stamp(
    here: &Map<String, Value>,
    base: &Map<String, Value>,
    stamps: &BTreeMap<String, u64>,
    now: u64,
) -> StampedSettings {
    let changed = |key: &str| {
        stamps
            .get(key)
            .map_or(now, |&seen| now.max(seen.saturating_add(1)))
    };
    let mut stamped: StampedSettings = here
        .iter()
        .map(|(key, value)| {
            let at = if base.get(key) == Some(value) {
                stamps.get(key).copied().unwrap_or(0)
            } else {
                changed(key)
            };
            (
                key.clone(),
                Stamped {
                    v: value.clone(),
                    at,
                },
            )
        })
        .collect();
    for key in base.keys().filter(|key| !here.contains_key(*key)) {
        stamped.insert(
            key.clone(),
            Stamped {
                v: Value::Null,
                at: changed(key),
            },
        );
    }
    stamped
}

/// The newest of each key of `all`. Two stamped alike are told apart by
/// what they hold, so every device that merges the same files comes to the
/// same, whichever is its own.
pub fn merge<'a>(all: impl IntoIterator<Item = &'a StampedSettings>) -> StampedSettings {
    let mut merged = StampedSettings::new();
    for settings in all {
        for (key, stamped) in settings {
            let newer = merged.get(key).is_none_or(|held| {
                (stamped.at, stamped.v.to_string()) > (held.at, held.v.to_string())
            });
            if newer {
                merged.insert(key.clone(), stamped.clone());
            }
        }
    }
    merged
}

/// The settings `merged` comes to, clamped as a store's are read: none
/// where it holds no settings at all, or none that read as settings.
pub fn settings_of(merged: &StampedSettings) -> Option<TrainingSettings> {
    let keys: Map<String, Value> = merged
        .iter()
        .filter(|(key, stamped)| !key.contains(':') && !stamped.v.is_null())
        .map(|(key, stamped)| (key.clone(), stamped.v.clone()))
        .collect();
    if keys.is_empty() {
        return None;
    }
    let raw = serde_json::to_string(&Value::Object(keys)).ok()?;
    crate::persist::settings_from(&raw)
}

/// What `merged` comes to here, where it differs from `using`.
pub fn taken(merged: &StampedSettings, using: &Using) -> Taken {
    let mut took = Taken {
        settings: settings_of(merged).filter(|settings| *settings != using.settings),
        ..Taken::default()
    };
    let here: BTreeMap<&str, AutoLevelCounters> = using
        .counters
        .iter()
        .map(|(key, counters)| (key.as_str(), *counters))
        .collect();
    for (key, stamped) in merged {
        let Some(run) = key.strip_prefix(LEVEL) else {
            continue;
        };
        if stamped.v.is_null() {
            if here.contains_key(run) {
                took.cleared.push(CountersKey::named(run));
            }
        } else if let Ok(counters) = serde_json::from_value::<AutoLevelCounters>(stamped.v.clone())
            && here.get(run) != Some(&counters)
        {
            took.counters.push((CountersKey::named(run), counters));
        }
    }
    took.theme = merged
        .get(THEME)
        .and_then(|stamped| stamped.v.as_str())
        .filter(|theme| !theme.is_empty() && *theme != using.theme)
        .map(str::to_string);
    let set = |key: &str| {
        merged
            .get(key)
            .is_some_and(|stamped| stamped.v == Value::Bool(true))
    };
    took.tour_done = set(TOUR) && !using.tour_done;
    took.told_of_paddle_keys = set(PADDLE_KEYS) && !using.told_of_paddle_keys;
    took
}

/// When each key of `merged` was last changed.
pub fn stamps_of(merged: &StampedSettings) -> BTreeMap<String, u64> {
    merged
        .iter()
        .map(|(key, stamped)| (key.clone(), stamped.at))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_of(settings: &TrainingSettings) -> Map<String, Value> {
        as_keys(settings)
    }

    fn using(settings: TrainingSettings) -> Using {
        Using {
            settings,
            counters: Vec::new(),
            theme: String::new(),
            tour_done: false,
            told_of_paddle_keys: false,
        }
    }

    /// Only what changed since the last sync is stamped as of now.
    #[test]
    fn only_what_changed_is_stamped_now() {
        let base = TrainingSettings::default();
        let mut here = base.clone();
        here.curriculum.num_groups = 9;
        let stamps = BTreeMap::from([("charWpmMin".to_string(), 5)]);
        let stamped = stamp(&keys_of(&here), &keys_of(&base), &stamps, 100);
        assert_eq!(stamped["numGroups"].at, 100);
        assert_eq!(stamped["numGroups"].v, serde_json::json!(9));
        assert_eq!(stamped["charWpmMin"].at, 5);
        assert!(
            stamped
                .iter()
                .filter(|(k, _)| *k != "numGroups" && *k != "charWpmMin")
                .all(|(_, s)| s.at == 0)
        );
        // A key taken away here is taken away as of now.
        let mut base_keys = keys_of(&base);
        base_keys.insert("contestActive".into(), Value::Bool(true));
        let stamped = stamp(&keys_of(&base), &base_keys, &BTreeMap::new(), 100);
        assert_eq!(
            stamped["contestActive"],
            Stamped {
                v: Value::Null,
                at: 100
            }
        );
    }

    /// A key changed, or taken away, is stamped after the newest stamp of
    /// it this device has seen, where its clock is behind that: the change
    /// made here, after it, is the newer.
    #[test]
    fn a_change_is_stamped_after_the_newest_seen_of_it() {
        let base = TrainingSettings::default();
        let mut here = base.clone();
        here.curriculum.num_groups = 9;
        let mut base_keys = keys_of(&base);
        base_keys.insert("contestActive".into(), Value::Bool(true));
        // Changed last on a device whose clock is an hour ahead.
        let stamps = BTreeMap::from([
            ("numGroups".to_string(), 3_600_100),
            ("contestActive".to_string(), 3_600_100),
            ("charWpmMin".to_string(), 50),
        ]);
        let stamped = stamp(&keys_of(&here), &base_keys, &stamps, 100);
        assert_eq!(stamped["numGroups"].at, 3_600_101);
        assert_eq!(
            stamped["contestActive"],
            Stamped {
                v: Value::Null,
                at: 3_600_101
            }
        );
        assert_eq!(stamped["charWpmMin"].at, 50, "unchanged");
        // Seen stamped earlier than now, a change is stamped now.
        let stamps = BTreeMap::from([("numGroups".to_string(), 40)]);
        let stamped = stamp(&keys_of(&here), &keys_of(&base), &stamps, 100);
        assert_eq!(stamped["numGroups"].at, 100);
    }

    /// The newest of each setting wins, from whichever device; and every
    /// device that merges the same comes to the same, in any order.
    #[test]
    fn the_newest_of_each_setting_wins() {
        let base = TrainingSettings::default();
        let base_keys = keys_of(&base);
        let mut phone = base.clone();
        phone.curriculum.num_groups = 9;
        let mut computer = base.clone();
        computer.playback.char_wpm_min = 30.0;
        computer.playback.char_wpm_max = 30.0;
        let phone = stamp(&keys_of(&phone), &base_keys, &BTreeMap::new(), 100);
        let computer = stamp(&keys_of(&computer), &base_keys, &BTreeMap::new(), 200);
        let merged = merge([&phone, &computer]);
        assert_eq!(merged, merge([&computer, &phone]));
        let settings = settings_of(&merged).expect("settings");
        assert_eq!(settings.curriculum.num_groups, 9);
        assert_eq!(settings.playback.char_wpm_min, 30.0);
        // The same setting changed on both: the later change.
        let mut later = base.clone();
        later.curriculum.num_groups = 4;
        let later = stamp(&keys_of(&later), &base_keys, &BTreeMap::new(), 300);
        let merged = merge([&phone, &later, &computer]);
        assert_eq!(
            settings_of(&merged)
                .expect("settings")
                .curriculum
                .num_groups,
            4
        );
        // Stamped alike, what they hold tells them apart, the same each way.
        let tie_a = stamp(&keys_of(&base), &Map::new(), &BTreeMap::new(), 7);
        let mut other = base;
        other.curriculum.num_groups = 11;
        let tie_b = stamp(&keys_of(&other), &Map::new(), &BTreeMap::new(), 7);
        assert_eq!(merge([&tie_a, &tie_b]), merge([&tie_b, &tie_a]));
    }

    /// Every setting syncs, how loud and how the paddles key among them;
    /// and a setting taken away is left out.
    #[test]
    fn every_setting_syncs_volume_and_paddles_too() {
        let mut there = TrainingSettings::default();
        there.band.master_volume = 0.9;
        there.playback.paddle_swap = true;
        there.curriculum.num_groups = 6;
        let stamped = stamp(&keys_of(&there), &Map::new(), &BTreeMap::new(), 9);
        let settings = settings_of(&stamped).expect("settings");
        assert_eq!(settings.band.master_volume, 0.9);
        assert!(settings.playback.paddle_swap);
        assert_eq!(settings.curriculum.num_groups, 6);
        let mut gone = stamped;
        gone.insert(
            "numGroups".into(),
            Stamped {
                v: Value::Null,
                at: 10,
            },
        );
        let settings = settings_of(&gone).expect("settings");
        assert_eq!(
            settings.curriculum.num_groups,
            TrainingSettings::default().curriculum.num_groups
        );
        // Nothing of the settings at all is no settings, not the defaults.
        assert_eq!(settings_of(&StampedSettings::new()), None);
    }

    /// The counters, the theme and the marks go as keys of their own,
    /// which the settings never read; and what the drive holds of them is
    /// taken up here only where it differs.
    #[test]
    fn counters_theme_and_marks_go_as_keys_of_their_own() {
        let mut phone = using(TrainingSettings::default());
        phone.counters = vec![
            (
                CountersKey::named("koch_3_abc"),
                AutoLevelCounters { above: 2, below: 0 },
            ),
            (
                CountersKey::named("digits_1_x"),
                AutoLevelCounters { above: 0, below: 1 },
            ),
        ];
        phone.theme = "dark".into();
        phone.tour_done = true;
        let keys = phone.keys();
        assert_eq!(
            keys["level:koch_3_abc"],
            serde_json::json!({ "above": 2, "below": 0 })
        );
        assert_eq!(keys[THEME], "dark");
        assert_eq!(keys[TOUR], true);
        assert!(!keys.contains_key(PADDLE_KEYS), "a mark not set is no key");
        assert_eq!(settings_keys(&keys), keys_of(&phone.settings));

        let merged = stamp(&keys, &Map::new(), &BTreeMap::new(), 5);
        let settings = settings_of(&merged).expect("settings");
        assert_eq!(
            settings,
            TrainingSettings::default().clamp(),
            "no counter read as a setting"
        );

        let mut computer = using(TrainingSettings::default());
        computer.counters = vec![
            (
                CountersKey::named("koch_3_abc"),
                AutoLevelCounters { above: 2, below: 0 },
            ),
            (
                CountersKey::named("words_2"),
                AutoLevelCounters { above: 1, below: 0 },
            ),
        ];
        computer.told_of_paddle_keys = true;
        let mut merged = merged;
        merged.insert(
            "level:words_2".into(),
            Stamped {
                v: Value::Null,
                at: 6,
            },
        );
        let took = taken(&merged, &computer);
        assert_eq!(took.settings, None, "the same settings");
        assert_eq!(
            took.counters,
            [(
                CountersKey::named("digits_1_x"),
                AutoLevelCounters { above: 0, below: 1 }
            )]
        );
        assert_eq!(took.cleared, [CountersKey::named("words_2")]);
        assert_eq!(took.theme.as_deref(), Some("dark"));
        assert!(took.tour_done);
        assert!(!took.told_of_paddle_keys, "set here already");
        let after = computer.with(&took);
        assert_eq!(after.counters.len(), 2);
        assert_eq!(
            (
                after.theme.as_str(),
                after.tour_done,
                after.told_of_paddle_keys
            ),
            ("dark", true, true)
        );
        assert!(taken(&merged, &after).is_empty(), "taken up once");
    }

    /// A value a later build stamped that this build cannot read is taken
    /// up as it is, and sent back as it is at its own stamp, never as this
    /// build reads it: so it stays the later build's on every device until
    /// it is changed here, when the change wins. This build's reading of it
    /// used to go back at the later build's stamp and win the tie by its
    /// text, `sharp` over `notch`, on every device.
    #[test]
    fn a_setting_this_build_cannot_read_is_not_stamped_as_changed() {
        let mut later_keys = keys_of(&TrainingSettings::default());
        later_keys.insert("filterShape".into(), "notch".into());
        let later = stamp(&later_keys, &Map::new(), &BTreeMap::new(), 50);
        // Joining: what the drive holds is taken up here.
        let here = using(TrainingSettings::default());
        let merged = merge([&later]);
        let now = here.with(&taken(&merged, &here));
        let (base, stamps) = (now.keys(), stamps_of(&merged));

        // Its next sync, with nothing changed here.
        let mine = stamp(&now.keys(), &base, &stamps, 100);
        assert_eq!(mine["filterShape"].at, 50, "not stamped as changed");
        let merged = merge([&later, &mine]);
        assert_eq!(merged["filterShape"].v, "notch");
        assert_eq!(merged, merge([&mine, &later]));
        let after = now.with(&taken(&merged, &now));
        assert_eq!(
            after.keys()["filterShape"],
            "notch",
            "still the later build's"
        );

        // Changed here, the change wins.
        let mut changed = now;
        changed.settings.band.filter_shape = cw_core::FilterShape::Soft;
        changed.settings = changed.settings.clamp();
        let mine = stamp(&changed.keys(), &base, &stamps, 200);
        assert_eq!(mine["filterShape"].at, 200);
        assert_eq!(merge([&later, &mine])["filterShape"].v, "soft");
    }

    /// Settings out of bounds come back clamped, as a store reads them.
    #[test]
    fn settings_from_the_drive_are_clamped() {
        let mut keys = StampedSettings::new();
        keys.insert(
            "numGroups".into(),
            Stamped {
                v: serde_json::json!(0),
                at: 1,
            },
        );
        let settings = settings_of(&keys).expect("settings");
        assert_eq!(settings.curriculum.num_groups, 1);
    }

    #[test]
    fn a_settings_file_reads_back() {
        let file = SettingsFile::new(
            "d",
            3,
            stamp(
                &keys_of(&TrainingSettings::default()),
                &Map::new(),
                &BTreeMap::new(),
                3,
            ),
        );
        let raw = serde_json::to_vec(&file).expect("JSON");
        assert_eq!(SettingsFile::read(&raw), Some(file.clone()));
        let later = SettingsFile {
            dust_settings: 2,
            ..file
        };
        assert_eq!(
            SettingsFile::read(&serde_json::to_vec(&later).expect("JSON")),
            None
        );
        assert_eq!(SettingsFile::read(b"[]"), None);
    }
}
