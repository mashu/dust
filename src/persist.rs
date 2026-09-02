use cw_core::{
    fit_settings_to_alphabet, AutoAdjustMode, AutoLevelCounters, SessionResult, TrainingSettings,
};

const MAX_SESSIONS: usize = 200;

fn finalize_settings(mut settings: TrainingSettings) -> TrainingSettings {
    settings = settings.clamp();
    fit_settings_to_alphabet(&mut settings);
    settings
}

#[cfg(feature = "web")]
mod backend {
    use super::*;

    const SETTINGS_KEY: &str = "dust_settings";
    const SESSIONS_KEY: &str = "dust_sessions";
    const AUTO_PREFIX: &str = "dust_auto_adjust_";

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn load_settings() -> TrainingSettings {
        let Some(store) = storage() else {
            return TrainingSettings::default();
        };
        let Ok(Some(raw)) = store.get_item(SETTINGS_KEY) else {
            return TrainingSettings::default();
        };
        finalize_settings(
            serde_json::from_str::<TrainingSettings>(&raw).unwrap_or_default(),
        )
    }

    pub fn save_settings(settings: &TrainingSettings) {
        let Some(store) = storage() else {
            return;
        };
        if let Ok(raw) = serde_json::to_string(settings) {
            let _ = store.set_item(SETTINGS_KEY, &raw);
        }
    }

    pub fn load_sessions() -> Vec<SessionResult> {
        let Some(store) = storage() else {
            return Vec::new();
        };
        let Ok(Some(raw)) = store.get_item(SESSIONS_KEY) else {
            return Vec::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save_sessions(sessions: &[SessionResult]) {
        let Some(store) = storage() else {
            return;
        };
        let trimmed: Vec<&SessionResult> = sessions.iter().rev().take(MAX_SESSIONS).collect();
        let chronological: Vec<SessionResult> = trimmed.into_iter().rev().cloned().collect();
        if let Ok(raw) = serde_json::to_string(&chronological) {
            let _ = store.set_item(SESSIONS_KEY, &raw);
        }
    }

    pub fn load_auto_counters(settings: &TrainingSettings) -> AutoLevelCounters {
        let Some(store) = storage() else {
            return AutoLevelCounters::default();
        };
        let mode = AutoAdjustMode::from_char_set(settings.char_set_mode);
        let key = format!("{AUTO_PREFIX}{}", mode.storage_key_for(settings));
        let Ok(Some(raw)) = store.get_item(&key) else {
            return AutoLevelCounters::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save_auto_counters(settings: &TrainingSettings, counters: AutoLevelCounters) {
        let Some(store) = storage() else {
            return;
        };
        let mode = AutoAdjustMode::from_char_set(settings.char_set_mode);
        let key = format!("{AUTO_PREFIX}{}", mode.storage_key_for(settings));
        if let Ok(raw) = serde_json::to_string(&counters) {
            let _ = store.set_item(&key, &raw);
        }
    }

    pub fn clear_auto_counters(keys: &[String]) {
        let Some(store) = storage() else {
            return;
        };
        for key in keys {
            let _ = store.remove_item(&format!("{AUTO_PREFIX}{key}"));
        }
    }
}

#[cfg(feature = "desktop")]
mod backend {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    fn data_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("dust")
    }

    fn ensure_dir() -> PathBuf {
        let dir = data_dir();
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn read_json<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
        let path = data_dir().join(name);
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn write_json(name: &str, value: &impl serde::Serialize) {
        let path = ensure_dir().join(name);
        if let Ok(raw) = serde_json::to_string_pretty(value) {
            let _ = fs::write(path, raw);
        }
    }

    pub fn load_settings() -> TrainingSettings {
        finalize_settings(read_json::<TrainingSettings>("settings.json").unwrap_or_default())
    }

    pub fn save_settings(settings: &TrainingSettings) {
        write_json("settings.json", settings);
    }

    pub fn load_sessions() -> Vec<SessionResult> {
        read_json("sessions.json").unwrap_or_default()
    }

    pub fn save_sessions(sessions: &[SessionResult]) {
        let trimmed: Vec<&SessionResult> = sessions.iter().rev().take(MAX_SESSIONS).collect();
        let chronological: Vec<SessionResult> = trimmed.into_iter().rev().cloned().collect();
        write_json("sessions.json", &chronological);
    }

    fn auto_path() -> PathBuf {
        data_dir().join("auto_adjust.json")
    }

    fn load_all_counters() -> BTreeMap<String, AutoLevelCounters> {
        fs::read_to_string(auto_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_all_counters(map: &BTreeMap<String, AutoLevelCounters>) {
        write_json("auto_adjust.json", map);
    }

    pub fn load_auto_counters(settings: &TrainingSettings) -> AutoLevelCounters {
        let mode = AutoAdjustMode::from_char_set(settings.char_set_mode);
        load_all_counters()
            .get(&mode.storage_key_for(settings))
            .copied()
            .unwrap_or_default()
    }

    pub fn save_auto_counters(settings: &TrainingSettings, counters: AutoLevelCounters) {
        let mut map = load_all_counters();
        let mode = AutoAdjustMode::from_char_set(settings.char_set_mode);
        map.insert(mode.storage_key_for(settings), counters);
        save_all_counters(&map);
    }

    pub fn clear_auto_counters(keys: &[String]) {
        let mut map = load_all_counters();
        for key in keys {
            map.remove(key);
        }
        save_all_counters(&map);
    }
}

pub use backend::*;
