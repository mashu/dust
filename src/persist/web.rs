//! `localStorage`: the browser store.

use std::cell::{Cell, RefCell};

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::{
    Aside, BackupSaving, CutOff, CutOffTaking, HistorySaved, OpenSession, Store, keep_aside,
    newest, over_stored, read_history, read_settings_from, saved_now, store_newest_that_fit,
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
    /// The stored history could not be read, nor copied out of the way, the
    /// last time it was read, so it is not to be written over until a read
    /// says otherwise.
    history_held: Cell<bool>,
    /// The browser has been asked to keep the store, this visit.
    asked_to_keep: Cell<bool>,
    /// This page's mark, while its session is marked open: what writes it,
    /// and takes it away with its lock ([`hold_mark`]).
    open_held: RefCell<Option<js_sys::Function>>,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// `localStorage`, as what is set aside is kept in it ([`keep_aside`]).
struct Shelf<'a>(&'a web_sys::Storage);

impl Aside for Shelf<'_> {
    fn keys(&self) -> Result<Vec<String>, ()> {
        let count = self.0.length().map_err(|_| ())?;
        (0..count)
            .map(|at| self.0.key(at).map_err(|_| ()))
            .filter_map(Result::transpose)
            .collect()
    }

    fn get(&self, key: &str) -> Result<Option<String>, ()> {
        self.0.get_item(key).map_err(|_| ())
    }

    fn set(&self, key: &str, raw: &str) -> bool {
        self.0.set_item(key, raw).is_ok()
    }

    fn remove(&self, key: &str) {
        let _ = self.0.remove_item(key);
    }
}

/// Hold the Web Lock `name`, and keep the mark of the same name in
/// `localStorage` only while the lock is held. The function handed back
/// takes the mark's text, or, given none, takes the mark away and lets go
/// of the lock. None in a browser without Web Locks, where nothing is
/// marked, so no running tab is ever taken for one cut off.
///
/// A lock asked for is not held at once: the browser grants it a moment
/// later, and only then does a page looking for marks see it, held or
/// waiting. A mark written as the lock was asked for could be found in
/// that moment without it, taken for one cut off, told of in the page that
/// looked, and deleted from under this one as it ran on. So the mark goes
/// in once the lock is granted, and under the lock pages look under
/// ([`take_marks`]): a look finds no mark, or one whose lock is held. It
/// is taken away before the lock is let go of.
fn hold_mark(name: &str) -> Option<js_sys::Function> {
    let hold = js_sys::Function::new_with_args(
        "name, look",
        "if (!navigator.locks) return null;
        let release;
        const held = new Promise((done) => { release = done; });
        let text = null;
        let placed = false;
        let gone = false;
        const place = () => {
            if (!gone && text !== null) localStorage.setItem(name, text);
            placed = true;
        };
        navigator.locks
            .request(name, () => navigator.locks.request(look, place).then(() => held, () => held))
            .catch(() => {});
        return (raw) => {
            if (typeof raw !== 'string') {
                gone = true;
                localStorage.removeItem(name);
                release();
                return;
            }
            text = raw;
            if (placed && !gone) localStorage.setItem(name, raw);
        };",
    );
    let look = format!("{OPEN_PREFIX}look");
    hold.call2(
        &JsValue::NULL,
        &JsValue::from_str(name),
        &JsValue::from_str(&look),
    )
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

    fn write_settings(&self, settings: &TrainingSettings) -> Result<(), String> {
        let store = storage().ok_or("this browser has no storage for them")?;
        let raw = serde_json::to_string(settings).map_err(|err| err.to_string())?;
        store
            .set_item(SETTINGS_KEY, &raw)
            .map_err(|_| "this browser's storage would not take them".into())
    }

    /// Whether the history is held is this read's to say, whatever the last
    /// one said: a copy there was no room to set aside then may fit now.
    fn read_sessions(&self) -> Option<Vec<SessionResult>> {
        self.history_held.set(false);
        let store = storage()?;
        let raw = store.get_item(SESSIONS_KEY).ok().flatten()?;
        // Copies set aside go beside any set aside before, each under a key
        // of its own, as far as the room goes.
        let aside = Shelf(&store);
        let Some(read) = read_history(&raw) else {
            // Keep the unreadable copy where the next save cannot reach it —
            // and only once it is safely there take it out of the way. A copy
            // that would not fit is no copy, and the original stays put.
            if keep_aside(&aside, UNREADABLE_SESSIONS_KEY, &raw) {
                let _ = store.remove_item(SESSIONS_KEY);
            } else {
                self.history_held.set(true);
            }
            return None;
        };
        // The sessions that do not read are set aside before the next save
        // leaves them out; where they cannot be, nothing is saved over them.
        if let Some(unread) = read.unread_json()
            && !keep_aside(&aside, UNREADABLE_SESSIONS_KEY, &unread)
        {
            self.history_held.set(true);
        }
        Some(read.sessions)
    }

    fn save_sessions(&self, sessions: &[SessionResult]) -> HistorySaved {
        if self.history_held.get() {
            return HistorySaved::Held;
        }
        let Some(store) = storage() else {
            return HistorySaved::Failed("this browser has no storage for it.".into());
        };
        store_newest_that_fit(newest(sessions), HISTORY_BUDGET, |raw| {
            store.set_item(SESSIONS_KEY, raw).is_ok()
        })
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
        let key = counters_key(key);
        let Ok(written) = serde_json::to_value(counters) else {
            return;
        };
        // Over what it held, so what a later build added to it stays.
        let stored = store
            .get_item(&key)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let _ = store.set_item(&key, &over_stored(written, stored).to_string());
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

    /// A browser says nothing of a download it blocks, or one the user
    /// turns down, so the file is told of as handed over, not as saved
    /// ([`Store::confirms_backups`]).
    fn save_backup_file(&self, name: &str, contents: &str) -> BackupSaving {
        saved_now(
            download(name, contents)
                .map(|()| {
                    format!(
                        "Backup handed to your browser to save as {name}: look for it in your downloads."
                    )
                })
                .ok_or_else(|| "this browser would not hand over the file".into()),
        )
    }

    fn confirms_backups(&self) -> bool {
        false
    }

    fn ask_to_keep(&self) {
        if !self.asked_to_keep.replace(true) {
            wasm_bindgen_futures::spawn_local(ask_for_persistence());
        }
    }

    fn mark_open(&self, open: Option<OpenSession>) {
        if storage().is_none() {
            return;
        }
        let mut held = self.open_held.borrow_mut();
        match open {
            Some(open) => {
                // The lock first: a page looking for marks then never finds
                // this one without it.
                let mark = held.take().or_else(|| {
                    hold_mark(&format!("{OPEN_PREFIX}{:016x}", crate::time::seed_rng()))
                });
                let Some(mark) = mark else {
                    return;
                };
                if let Ok(raw) = serde_json::to_string(&open) {
                    let _ = mark.call1(&JsValue::NULL, &JsValue::from_str(&raw));
                }
                *held = Some(mark);
            }
            None => {
                if let Some(mark) = held.take() {
                    let _ = mark.call0(&JsValue::NULL);
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
