use cw_core::{
    fit_settings_to_alphabet, AutoAdjustMode, AutoLevelCounters, SessionResult, TrainingSettings,
};

const MAX_SESSIONS: usize = 200;

pub trait Store {
    fn load_theme(&self) -> String;
    fn save_theme(&self, theme: &str);
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
    fn load_theme(&self) -> String {
        storage()
            .and_then(|store| store.get_item(THEME_KEY).ok().flatten())
            .unwrap_or_default()
    }

    fn save_theme(&self, theme: &str) {
        if let Some(store) = storage() {
            let _ = store.set_item(THEME_KEY, theme);
        }
    }

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
const THEME_KEY: &str = "dust_theme";
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

#[cfg(feature = "native-runtime")]
pub struct DesktopStore;

#[cfg(feature = "native-runtime")]
impl Store for DesktopStore {
    fn load_theme(&self) -> String {
        std::fs::read_to_string(data_dir().join("theme.txt"))
            .map(|raw| raw.trim().to_string())
            .unwrap_or_default()
    }

    fn save_theme(&self, theme: &str) {
        let dir = ensure_dir();
        let _ = std::fs::write(dir.join("theme.txt"), theme);
    }

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

/// Android hands an app no `HOME`, so `dirs` has nothing to work from. The
/// first entry of `/proc/self/cmdline` is the package name, and
/// `/data/data/<package>/files` is the private directory the app owns.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn package_from_cmdline(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let name = text.split('\0').next()?.trim();
    // Some processes are named `<package>:<process>`; the data dir is the package.
    let name = name.split(':').next()?;
    let valid = !name.is_empty()
        && name.contains('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
    valid.then(|| name.to_string())
}

#[cfg(all(feature = "native-runtime", target_os = "android"))]
fn android_data_dir() -> Option<std::path::PathBuf> {
    let raw = std::fs::read("/proc/self/cmdline").ok()?;
    let package = package_from_cmdline(&raw)?;
    let dir = std::path::PathBuf::from(format!("/data/data/{package}/files/dust"));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(feature = "native-runtime")]
fn data_dir() -> std::path::PathBuf {
    #[cfg(target_os = "android")]
    if let Some(dir) = android_data_dir() {
        return dir;
    }
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("dust")
}

#[cfg(feature = "native-runtime")]
fn ensure_dir() -> std::path::PathBuf {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(feature = "native-runtime")]
fn read_json<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    let path = data_dir().join(name);
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(feature = "native-runtime")]
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

#[cfg(feature = "native-runtime")]
fn auto_path() -> std::path::PathBuf {
    data_dir().join("auto_adjust.json")
}

#[cfg(feature = "native-runtime")]
fn load_all_counters() -> std::collections::BTreeMap<String, AutoLevelCounters> {
    std::fs::read_to_string(auto_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

#[cfg(feature = "native-runtime")]
fn save_all_counters(map: &std::collections::BTreeMap<String, AutoLevelCounters>) {
    write_json("auto_adjust.json", map);
}

#[cfg(feature = "web")]
pub fn default_store() -> WebStore {
    WebStore
}

#[cfg(feature = "native-runtime")]
pub fn default_store() -> DesktopStore {
    DesktopStore
}

pub fn load_theme() -> String {
    default_store().load_theme()
}

pub fn save_theme(theme: &str) {
    default_store().save_theme(theme)
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

#[cfg(test)]
mod tests {
    use super::{
        finalize_settings, package_from_cmdline, recover_sessions, trim_sessions, MAX_SESSIONS,
    };
    use cw_core::{CharSetMode, SessionResult, TrainingSettings};

    fn session(date: &str) -> SessionResult {
        SessionResult {
            date: date.to_string(),
            timestamp: 1,
            started_at: 0,
            finished_at: 1,
            groups: Vec::new(),
            group_timings: Vec::new(),
            accuracy: 1.0,
            letter_accuracy: Default::default(),
            alphabet_size: 1,
            avg_response_ms: 1.0,
            total_chars: 1,
            effective_alphabet_size: 1.0,
            score: 1.0,
            level: 1,
            digits_level: 1,
            char_set_mode: CharSetMode::Mixed,
            char_wpm: 20.0,
            effective_wpm: 18.0,
            alphabet_fingerprint: String::new(),
        }
    }

    #[test]
    fn history_keeps_the_newest_sessions() {
        let all: Vec<SessionResult> = (0..MAX_SESSIONS + 20)
            .map(|i| session(&format!("2026-01-{i:03}")))
            .collect();
        let trimmed = trim_sessions(&all);
        assert_eq!(trimmed.len(), MAX_SESSIONS);
        // The oldest go, the order stays chronological.
        assert_eq!(trimmed.first().unwrap().date, all[20].date);
        assert_eq!(trimmed.last().unwrap().date, all.last().unwrap().date);
        assert_eq!(trim_sessions(&[]).len(), 0);
    }

    #[test]
    fn a_corrupt_session_is_dropped_instead_of_losing_the_file() {
        let good = serde_json::to_string(&session("2026-09-01")).unwrap();
        let raw = format!("[{good}, {{\"date\": \"nonsense\"}}]");
        let recovered = recover_sessions(&raw);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].date, "2026-09-01");
        // A file that is not even a list gives an empty history, not a panic.
        assert!(recover_sessions("{}").is_empty());
        assert!(recover_sessions("").is_empty());
    }

    #[test]
    fn loaded_settings_are_clamped_and_fitted() {
        let mut stored = TrainingSettings::default();
        stored.playback.char_wpm_min = 500.0;
        stored.curriculum.num_groups = 0;
        let settings = finalize_settings(stored);
        assert_eq!(settings.playback.char_wpm_min, 80.0);
        assert_eq!(settings.curriculum.num_groups, 1);
    }

    #[test]
    fn reads_the_package_name_from_cmdline() {
        assert_eq!(
            package_from_cmdline(b"dev.dust.morse\0").as_deref(),
            Some("dev.dust.morse")
        );
        assert_eq!(
            package_from_cmdline(b"dev.dust.morse\0/system/bin/app_process\0").as_deref(),
            Some("dev.dust.morse")
        );
        assert_eq!(
            package_from_cmdline(b"dev.dust.morse:remote\0").as_deref(),
            Some("dev.dust.morse")
        );
    }

    #[test]
    fn rejects_anything_that_is_not_a_package() {
        assert_eq!(package_from_cmdline(b""), None);
        assert_eq!(package_from_cmdline(b"\0"), None);
        // A desktop process name has no dots and must not become a /data/data path.
        assert_eq!(package_from_cmdline(b"dust\0"), None);
        assert_eq!(package_from_cmdline(b"/usr/bin/dust\0"), None);
    }
}
