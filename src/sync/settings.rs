//! The settings, synced: each device keeps a file of its own in the drive,
//! holding every setting with when it was last changed, and takes the
//! newest of each setting from every device's file and its own
//! ([`merge`]). A device writes only its own file, so two that sync at once
//! never write over each other; and a setting changed on one device and
//! another on another both come through.
//!
//! A device that joins a sync the drive holds already takes the settings
//! there, as restoring a backup takes a backup's, and adds nothing of its
//! own until it changes something: it is the drive's that are wanted on
//! it. What is set for a device and not for the training, how loud it
//! plays and how its keys and paddles key, stays each device's own
//! ([`TrainingSettings::keep_device_settings`]).

use std::collections::BTreeMap;

use cw_core::TrainingSettings;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// What every device's settings file is called before its keyed hash
/// ([`super::seal::Keys::name`]).
pub const SETTINGS_PREFIX: &str = "dust-s-";
const SETTINGS_LAYOUT: u32 = 1;

/// A setting, and when it was last changed, in milliseconds since the
/// epoch: zero for one never changed since this device took up sync. A
/// setting taken away, as Contest's `contestActive` is when another
/// character set is chosen, is null.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stamped {
    pub v: Value,
    pub at: u64,
}

/// Every setting, stamped.
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

/// This device's settings, `here`, stamped: each that changed since the
/// last sync, from `base`, as of `now`; each other as it was stamped then,
/// in `stamps`. One `base` had and `here` has not is taken away as of now.
pub fn stamp(
    here: &Map<String, Value>,
    base: &Map<String, Value>,
    stamps: &BTreeMap<String, u64>,
    now: u64,
) -> StampedSettings {
    let mut stamped: StampedSettings = here
        .iter()
        .map(|(key, value)| {
            let at = if base.get(key) == Some(value) {
                stamps.get(key).copied().unwrap_or(0)
            } else {
                now
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
                at: now,
            },
        );
    }
    stamped
}

/// The newest of each setting of `all`. Two stamped alike are told apart by
/// what they hold, so every device that merges the same files comes to the
/// same settings, whichever is its own.
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

/// The settings `merged` comes to, but for what `here` sets for this
/// device, clamped as a store's are read: none where they do not read as
/// settings at all.
pub fn settings_of(merged: &StampedSettings, here: &TrainingSettings) -> Option<TrainingSettings> {
    let keys: Map<String, Value> = merged
        .iter()
        .filter(|(_, stamped)| !stamped.v.is_null())
        .map(|(key, stamped)| (key.clone(), stamped.v.clone()))
        .collect();
    let raw = serde_json::to_string(&Value::Object(keys)).ok()?;
    let mut settings = crate::persist::settings_from(&raw)?;
    settings.keep_device_settings(here);
    Some(settings)
}

/// When each setting of `merged` was last changed.
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
        let settings = settings_of(&merged, &base).expect("settings");
        assert_eq!(settings.curriculum.num_groups, 9);
        assert_eq!(settings.playback.char_wpm_min, 30.0);
        // The same setting changed on both: the later change.
        let mut later = base.clone();
        later.curriculum.num_groups = 4;
        let later = stamp(&keys_of(&later), &base_keys, &BTreeMap::new(), 300);
        let merged = merge([&phone, &later, &computer]);
        assert_eq!(
            settings_of(&merged, &base)
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

    /// What is set for a device stays this device's own, whatever another
    /// device set; and a setting taken away is left out.
    #[test]
    fn this_devices_volume_and_paddles_stay_its_own() {
        let mut here = TrainingSettings::default();
        here.band.master_volume = 0.3;
        here.playback.paddle_swap = true;
        let mut there = TrainingSettings::default();
        there.band.master_volume = 0.9;
        there.curriculum.num_groups = 6;
        let stamped = stamp(&keys_of(&there), &Map::new(), &BTreeMap::new(), 9);
        let settings = settings_of(&stamped, &here).expect("settings");
        assert_eq!(settings.band.master_volume, 0.3);
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
        let settings = settings_of(&gone, &here).expect("settings");
        assert_eq!(
            settings.curriculum.num_groups,
            TrainingSettings::default().curriculum.num_groups
        );
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
        let settings = settings_of(&keys, &TrainingSettings::default()).expect("settings");
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
