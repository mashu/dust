//! `localStorage`: the browser store.

use std::cell::{Cell, RefCell};

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::{
    BackupSaving, CutOff, CutOffTaking, HistorySaved, OpenSession, Store, newest, newest_that_fit,
    parse_sessions, read_settings_from, saved_now,
};

const THEME_KEY: &str = "dust_theme";
const TUTORIAL_KEY: &str = "dust_tutorial_done";
const SETTINGS_KEY: &str = "dust_settings";
const SESSIONS_KEY: &str = "dust_sessions";
const UNREADABLE_SESSIONS_KEY: &str = "dust_sessions_unreadable";
const AUTO_PREFIX: &str = "dust_auto_adjust_";
const LAST_BACKUP_KEY: &str = "dust_last_backup";
/// The sessions and runs marked open ([`Store::mark_open`]): each page's
/// under its own key, while the page holds a Web Lock of the same name. A
/// page that goes, closed, reloaded or crashed, lets go of its lock, so a
/// mark whose lock nobody holds was cut off, and one held belongs to a tab
/// still running it.
const OPEN_PREFIX: &str = "dust_open_session_";

/// How much of the site's storage the history may take, in characters.
/// Browsers give a site about five million; the rest is left for the
/// settings, the counters and the room a write needs.
const HISTORY_BUDGET: usize = 3_000_000;

#[derive(Default)]
pub struct WebStore {
    /// The stored history could not be read, nor copied out of the way, so
    /// it is not to be written over.
    history_held: Cell<bool>,
    /// The browser has been asked to keep the store, this visit.
    asked_to_keep: Cell<bool>,
    /// This page's mark, while its session is marked open: its name, and
    /// what lets go of its lock.
    open_held: RefCell<Option<(String, js_sys::Function)>>,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Hold the Web Lock `name` until the function handed back is called; none
/// in a browser without Web Locks, where nothing is marked, so no running
/// tab is ever taken for one cut off. Asked for at once, as the call
/// returns: a lock asked for is as good as held to a page looking for marks.
fn hold_lock(name: &str) -> Option<js_sys::Function> {
    let hold = js_sys::Function::new_with_args(
        "name",
        "if (!navigator.locks) return null;
        let release;
        const held = new Promise((done) => { release = done; });
        navigator.locks.request(name, () => held);
        return release;",
    );
    hold.call1(&JsValue::NULL, &JsValue::from_str(name))
        .ok()?
        .dyn_into()
        .ok()
}

/// Take every mark whose page has gone, and hand back what each said: one
/// look at a time across tabs, so two opening at once tell of it once.
async fn take_marks() -> Vec<String> {
    let take = js_sys::Function::new_with_args(
        "prefix",
        "if (!navigator.locks) return Promise.resolve([]);
        return navigator.locks.request(prefix + 'look', async () => {
            const state = await navigator.locks.query();
            const live = new Set([...state.held, ...state.pending].map((lock) => lock.name));
            const keys = [];
            for (let i = 0; i < localStorage.length; i++) {
                const key = localStorage.key(i);
                if (key && key.startsWith(prefix) && !live.has(key)) keys.push(key);
            }
            return keys.map((key) => {
                const mark = localStorage.getItem(key) ?? '';
                localStorage.removeItem(key);
                return mark;
            });
        }).catch(() => []);",
    );
    let Ok(promise) = take.call1(&JsValue::NULL, &JsValue::from_str(OPEN_PREFIX)) else {
        return Vec::new();
    };
    let Ok(marks) = JsFuture::from(js_sys::Promise::from(promise)).await else {
        return Vec::new();
    };
    js_sys::Array::from(&marks)
        .iter()
        .map(|mark| mark.as_string().unwrap_or_default())
        .collect()
}

fn counters_key(key: &CountersKey) -> String {
    format!("{AUTO_PREFIX}{}", key.as_str())
}

/// Hand `contents` over as a download called `name`, the way a link to a
/// file does when it is clicked.
fn download(name: &str, contents: &str) -> Option<()> {
    let document = web_sys::window()?.document()?;
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/json");
    let parts = js_sys::Array::of1(&JsValue::from_str(contents));
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &options).ok()?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).ok()?;
    let link: web_sys::HtmlAnchorElement = document.create_element("a").ok()?.dyn_into().ok()?;
    link.set_href(&url);
    link.set_download(name);
    document.body()?.append_child(&link).ok()?;
    link.click();
    link.remove();
    // The browser has the file once the click returns; the address it was
    // read from can go a moment later.
    wasm_bindgen_futures::spawn_local(async move {
        crate::time::sleep_ms(10_000).await;
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    Some(())
}

/// Ask the browser to keep this site's store rather than clear it when it
/// needs room, unless it already does. Chrome decides by itself; Firefox
/// asks the user, once.
async fn ask_for_persistence() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let navigator = window.navigator();
    // Older browsers have no storage manager at all.
    if !js_sys::Reflect::has(&navigator, &JsValue::from_str("storage")).unwrap_or(false) {
        return;
    }
    let manager = navigator.storage();
    let kept = match manager.persisted() {
        Ok(promise) => JsFuture::from(promise).await.ok(),
        Err(_) => return,
    };
    if kept.and_then(|kept| kept.as_bool()) == Some(true) {
        return;
    }
    if let Ok(promise) = manager.persist() {
        let _ = JsFuture::from(promise).await;
    }
}

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

    /// A browser that keeps nothing counts it as done: the guide would
    /// otherwise open on every visit.
    fn tutorial_done(&self) -> bool {
        storage().is_none_or(|store| store.get_item(TUTORIAL_KEY).ok().flatten().is_some())
    }

    fn mark_tutorial_done(&self) {
        if let Some(store) = storage() {
            let _ = store.set_item(TUTORIAL_KEY, "done");
        }
    }

    fn read_settings(&self) -> Option<TrainingSettings> {
        let raw = storage()?.get_item(SETTINGS_KEY).ok().flatten()?;
        read_settings_from(&raw)
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
        parse_sessions(&raw).unwrap_or_else(|| {
            // Keep the unreadable copy where the next save cannot reach it —
            // and only once it is safely there take it out of the way. A copy
            // that would not fit is no copy, and the original stays put.
            if store.set_item(UNREADABLE_SESSIONS_KEY, &raw).is_ok() {
                let _ = store.remove_item(SESSIONS_KEY);
            } else {
                self.history_held.set(true);
            }
            Vec::new()
        })
    }

    fn save_sessions(&self, sessions: &[SessionResult]) -> HistorySaved {
        if self.history_held.get() {
            return HistorySaved::Held;
        }
        let Some(store) = storage() else {
            return HistorySaved::Failed("this browser has no storage for it.".into());
        };
        let newest = newest(sessions);
        // The newest that fit the budget; and if the browser has less room
        // than that, the newest that fit in half as much, and so on.
        let mut budget = HISTORY_BUDGET;
        loop {
            let (raw, kept) = newest_that_fit(newest, budget);
            match store.set_item(SESSIONS_KEY, &raw) {
                Ok(()) if kept < newest.len() => {
                    let runs = newest.iter().any(|session| session.contest.is_some());
                    return HistorySaved::Squeezed { kept, runs };
                }
                Ok(()) => return HistorySaved::Stored { kept },
                Err(_) if kept > 1 => budget = raw.len() / 2,
                Err(_) => {
                    return HistorySaved::Failed("this browser's storage is full.".into());
                }
            }
        }
    }

    fn load_auto_counters(&self, key: &CountersKey) -> AutoLevelCounters {
        storage()
            .and_then(|store| store.get_item(&counters_key(key)).ok().flatten())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_auto_counters(&self, key: &CountersKey, counters: AutoLevelCounters) {
        let Some(store) = storage() else {
            return;
        };
        if let Ok(raw) = serde_json::to_string(&counters) {
            let _ = store.set_item(&counters_key(key), &raw);
        }
    }

    fn clear_auto_counters(&self, keys: &[CountersKey]) {
        let Some(store) = storage() else {
            return;
        };
        for key in keys {
            let _ = store.remove_item(&counters_key(key));
        }
    }

    fn last_backup(&self) -> Option<u64> {
        storage()?
            .get_item(LAST_BACKUP_KEY)
            .ok()
            .flatten()?
            .parse()
            .ok()
    }

    fn mark_backup(&self, at: u64) {
        if let Some(store) = storage() {
            let _ = store.set_item(LAST_BACKUP_KEY, &at.to_string());
        }
    }

    fn saves_backups(&self) -> bool {
        true
    }

    fn save_backup_file(&self, name: &str, contents: &str) -> BackupSaving {
        saved_now(
            download(name, contents)
                .map(|()| format!("{name}, in your downloads"))
                .ok_or_else(|| "this browser would not hand over the file".into()),
        )
    }

    fn ask_to_keep(&self) {
        if !self.asked_to_keep.replace(true) {
            wasm_bindgen_futures::spawn_local(ask_for_persistence());
        }
    }

    fn mark_open(&self, open: Option<OpenSession>) {
        let Some(store) = storage() else {
            return;
        };
        let mut held = self.open_held.borrow_mut();
        match open {
            Some(open) => {
                // The lock first: a page looking for marks then never finds
                // this one without it.
                let mark = held.take().or_else(|| {
                    let name = format!("{OPEN_PREFIX}{:016x}", crate::time::seed_rng());
                    hold_lock(&name).map(|release| (name, release))
                });
                let Some((name, release)) = mark else {
                    return;
                };
                if let Ok(raw) = serde_json::to_string(&open) {
                    let _ = store.set_item(&name, &raw);
                }
                *held = Some((name, release));
            }
            None => {
                if let Some((name, release)) = held.take() {
                    let _ = store.remove_item(&name);
                    let _ = release.call0(&JsValue::NULL);
                }
            }
        }
    }

    fn take_cut_off(&self) -> CutOffTaking {
        Box::pin(async {
            let marks = take_marks().await;
            CutOff::newest(marks.iter().map(|mark| CutOff::read(mark.as_bytes())))
        })
    }
}
