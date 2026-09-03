use cw_core::{
    fit_settings_to_alphabet, AutoAdjustMode, AutoLevelCounters, SessionResult, TrainingSettings,
};

const MAX_SESSIONS: usize = 200;

pub trait Store {
    fn load_settings(&self) -> TrainingSettings;
    fn save_settings(&self, settings: &TrainingSettings);
    fn load_sessions(&self) -> Vec<SessionResult>;
    fn save_sessions(&self, sessions: &[SessionResult]);
    fn load_auto_counters(&self, settings: &TrainingSettings) -> AutoLevelCounters;
    fn save_auto_counters(&self, settings: &TrainingSettings, counters: AutoLevelCounters);
    fn clear_auto_counters(&self, keys: &[String]);
}

fn recover_sessions(raw: &str) -> Vec<SessionResult> {
    let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

fn finalize_settings(mut settings: TrainingSettings) -> TrainingSettings {
    settings = settings.clamp();
    fit_settings_to_alphabet(&mut settings);
    settings
}

fn trim_sessions(sessions: &[SessionResult]) -> Vec<SessionResult> {
    let trimmed: Vec<&SessionResult> = sessions.iter().rev().take(MAX_SESSIONS).collect();
    trimmed.into_iter().rev().cloned().collect()
}

#[cfg(feature = "web")]
pub struct WebStore;

#[cfg(feature = "web")]
impl Store for WebStore {
    fn load_settings(&self) -> TrainingSettings {
        let Some(store) = storage() else {
            return TrainingSettings::default();
        };
        let Ok(Some(raw)) = store.get_item(SETTINGS_KEY) else {
            return TrainingSettings::default();
        };
        finalize_settings(serde_json::from_str::<TrainingSettings>(&raw).unwrap_or_default())
    }

    fn save_settings(&self, settings: &TrainingSettings) {
        let Some(store) = storage() else {
            return;
        };
        if let Ok(raw) = serde_json::to_string(settings) {
            let _ = store.set_item(SETTINGS_KEY, &raw);
        }
    }

    fn load_sessions(&self) -> Vec<SessionResult> {
        let Some(store) = storage() else {
            return Vec::new();
        };
        let Ok(Some(raw)) = store.get_item(SESSIONS_KEY) else {
            return Vec::new();
        };
        recover_sessions(&raw)
    }

    fn save_sessions(&self, sessions: &[SessionResult]) {
        let Some(store) = storage() else {
            return;
        };
        if let Ok(raw) = serde_json::to_string(&trim_sessions(sessions)) {
            let _ = store.set_item(SESSIONS_KEY, &raw);
        }
    }

    fn load_auto_counters(&self, settings: &TrainingSettings) -> AutoLevelCounters {
        let Some(store) = storage() else {
            return AutoLevelCounters::default();
        };
        let mode = AutoAdjustMode::from_char_set(settings.curriculum.char_set_mode);
        let key = format!("{AUTO_PREFIX}{}", mode.storage_key_for(settings));
        let Ok(Some(raw)) = store.get_item(&key) else {
            return AutoLevelCounters::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    fn save_auto_counters(&self, settings: &TrainingSettings, counters: AutoLevelCounters) {
        let Some(store) = storage() else {
            return;
        };
        let mode = AutoAdjustMode::from_char_set(settings.curriculum.char_set_mode);
        let key = format!("{AUTO_PREFIX}{}", mode.storage_key_for(settings));
        if let Ok(raw) = serde_json::to_string(&counters) {
            let _ = store.set_item(&key, &raw);
        }
    }

    fn clear_auto_counters(&self, keys: &[String]) {
        let Some(store) = storage() else {
            return;
        };
        for key in keys {
            let _ = store.remove_item(&format!("{AUTO_PREFIX}{key}"));
        }
    }
}

#[cfg(feature = "web")]
const SETTINGS_KEY: &str = "dust_settings";
#[cfg(feature = "web")]
const SESSIONS_KEY: &str = "dust_sessions";
#[cfg(feature = "web")]
const AUTO_PREFIX: &str = "dust_auto_adjust_";

#[cfg(feature = "web")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

#[cfg(feature = "desktop")]
pub struct DesktopStore;

#[cfg(feature = "desktop")]
impl Store for DesktopStore {
    fn load_settings(&self) -> TrainingSettings {
        finalize_settings(read_json::<TrainingSettings>("settings.json").unwrap_or_default())
    }

    fn save_settings(&self, settings: &TrainingSettings) {
        write_json("settings.json", settings);
    }

    fn load_sessions(&self) -> Vec<SessionResult> {
        let path = data_dir().join("sessions.json");
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        recover_sessions(&raw)
    }

    fn save_sessions(&self, sessions: &[SessionResult]) {
        write_json("sessions.json", &trim_sessions(sessions));
    }

    fn load_auto_counters(&self, settings: &TrainingSettings) -> AutoLevelCounters {
        let mode = AutoAdjustMode::from_char_set(settings.curriculum.char_set_mode);
        load_all_counters()
            .get(&mode.storage_key_for(settings))
            .copied()
            .unwrap_or_default()
    }

    fn save_auto_counters(&self, settings: &TrainingSettings, counters: AutoLevelCounters) {
        let mut map = load_all_counters();
        let mode = AutoAdjustMode::from_char_set(settings.curriculum.char_set_mode);
        map.insert(mode.storage_key_for(settings), counters);
        save_all_counters(&map);
    }

    fn clear_auto_counters(&self, keys: &[String]) {
        let mut map = load_all_counters();
        for key in keys {
            map.remove(key);
        }
        save_all_counters(&map);
    }
}

#[cfg(feature = "desktop")]
fn data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("dust")
}

#[cfg(feature = "desktop")]
fn ensure_dir() -> std::path::PathBuf {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(feature = "desktop")]
fn read_json<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    let path = data_dir().join(name);
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(feature = "desktop")]
fn write_json(name: &str, value: &impl serde::Serialize) {
    let dir = ensure_dir();
    let path = dir.join(name);
    let tmp = dir.join(format!(".{name}.tmp"));
    let Ok(raw) = serde_json::to_string_pretty(value) else {
        return;
    };
    if std::fs::write(&tmp, raw).is_ok() {
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(tmp);
        }
    }
}

#[cfg(feature = "desktop")]
fn auto_path() -> std::path::PathBuf {
    data_dir().join("auto_adjust.json")
}

#[cfg(feature = "desktop")]
fn load_all_counters() -> std::collections::BTreeMap<String, AutoLevelCounters> {
    std::fs::read_to_string(auto_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

#[cfg(feature = "desktop")]
fn save_all_counters(map: &std::collections::BTreeMap<String, AutoLevelCounters>) {
    write_json("auto_adjust.json", map);
}

#[cfg(feature = "web")]
pub fn default_store() -> WebStore {
    WebStore
}

#[cfg(feature = "desktop")]
pub fn default_store() -> DesktopStore {
    DesktopStore
}

pub fn load_settings() -> TrainingSettings {
    default_store().load_settings()
}

pub fn save_settings(settings: &TrainingSettings) {
    default_store().save_settings(settings)
}

pub fn load_sessions() -> Vec<SessionResult> {
    default_store().load_sessions()
}

pub fn save_sessions(sessions: &[SessionResult]) {
    default_store().save_sessions(sessions)
}

pub fn load_auto_counters(settings: &TrainingSettings) -> AutoLevelCounters {
    default_store().load_auto_counters(settings)
}

pub fn save_auto_counters(settings: &TrainingSettings, counters: AutoLevelCounters) {
    default_store().save_auto_counters(settings, counters)
}

pub fn clear_auto_counters(keys: &[String]) {
    default_store().clear_auto_counters(keys)
}
