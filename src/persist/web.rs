//! The browser store: `localStorage` for all but the history, which the
//! browser's database keeps ([`super::idb`]), opened before the app starts
//! ([`preload`]).
//!
//! A browser keeps them, their room and its Web Locks for the whole origin,
//! and a GitHub Pages project site shares its origin with every other one
//! of the account (docs/platforms.md). So every name kept or locked here
//! starts with `dust_`, and only names of Dust's own are ever taken away: by
//! exact name, or by a prefix of its own ([`keep_aside`], [`take_marks`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cw_core::{AutoLevelCounters, CountersKey, TrainingSettings};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::idb::{BrowserHistory, Script};
use super::legacy::{Aside, BlobHistory, BlobKeys, HISTORY_BUDGET, Kept, set_aside_from};
use super::record::{HistoryChange, HistoryLoad, HistoryNotice, Pending, Record, RecordId, done};
use super::{
    BackupSaving, CutOff, CutOffTaking, HistorySaved, OpenSession, SETTINGS_HELD, SetAside, Store,
    over_stored, read_settings_from, saved_now,
};

const THEME_KEY: &str = "dust_theme";
const TUTORIAL_KEY: &str = "dust_tutorial_done";
const TOLD_OF_PADDLE_KEYS_KEY: &str = "dust_told_of_paddle_keys";
const ASKED_ABOUT_LOCK_KEY: &str = "dust_asked_about_lock";
const SETTINGS_KEY: &str = "dust_settings";
const UNREADABLE_SETTINGS_KEY: &str = "dust_settings_unreadable";
const SESSIONS_KEY: &str = "dust_sessions";
const UNREADABLE_SESSIONS_KEY: &str = "dust_sessions_unreadable";
/// The sessions deleted by hand ([`super::Deleted`]), as builds before
/// records remember them, and the history kept whole still does.
const DELETED_KEY: &str = "dust_deleted";
/// Where builds before records keep the history whole, and where this one
/// keeps it for a visit the browser's database will not.
const BLOB_KEYS: BlobKeys = BlobKeys {
    history: SESSIONS_KEY,
    deleted: DELETED_KEY,
    aside: UNREADABLE_SESSIONS_KEY,
};
const AUTO_PREFIX: &str = "dust_auto_adjust_";
const LAST_BACKUP_KEY: &str = "dust_last_backup";
/// The sessions and runs marked open ([`Store::mark_open`]): each page's
/// under its own key, while the page holds a Web Lock of the same name. A
/// page that goes, closed, reloaded or crashed, lets go of its lock, so a
/// mark whose lock nobody holds was cut off, and one held belongs to a tab
/// still running it.
const OPEN_PREFIX: &str = "dust_open_session_";

pub struct WebStore {
    /// The stored settings could not be read, nor copied out of the way,
    /// the last time they were read, so they are not to be written over
    /// until a read says otherwise.
    settings_held: Cell<bool>,
    /// What reads set aside, not yet told of ([`Store::take_set_aside`]).
    set_aside: RefCell<Vec<SetAside>>,
    /// The browser has been asked to keep the store, this visit.
    asked_to_keep: Cell<bool>,
    /// This page's mark, while its session is marked open: what writes it,
    /// and takes it away with its lock ([`hold_mark`]).
    open_held: RefCell<Option<js_sys::Function>>,
    /// Where the history is kept this visit.
    history: History,
}

/// Where the history is kept this visit.
enum History {
    /// The browser's database, by record, with no room to keep it to but
    /// what the browser gives the site.
    Database(Rc<BrowserHistory<Script, Local>>),
    /// The browser's page storage, kept whole and to its room, as builds
    /// before kept it, where the database would not open, or would not
    /// take the history: and why not, told of as the app opens.
    Page {
        whole: BlobHistory<Local>,
        why: RefCell<Option<String>>,
    },
}

/// What the browser's database made of the history as the page opened
/// ([`preload`]): the history opened, or why the page storage keeps it this
/// visit, with what reading it set aside meanwhile.
type Preloaded = Result<Rc<BrowserHistory<Script, Local>>, (String, Vec<SetAside>)>;

thread_local! {
    static PRELOADED: RefCell<Option<Preloaded>> = const { RefCell::new(None) };
}

/// Open the history in the browser's database before the app starts, and
/// read it whole, so that the app opens on it at once, the boot screen
/// showing meanwhile. The history kept whole in the page storage is moved
/// in first, where it was not ([`BrowserHistory::open`]). Where the
/// database will not open, or will not take the history, the page storage
/// keeps it this visit ([`WebStore::default`]).
pub async fn preload() {
    let opened = match Script::open().await {
        Ok((script, read_only)) => BrowserHistory::open(script, read_only, Local, BLOB_KEYS)
            .await
            .map(Rc::new)
            .map_err(|not| (not.why, not.set_aside)),
        Err(why) => Err((why, Vec::new())),
    };
    PRELOADED.with(|preloaded| *preloaded.borrow_mut() = Some(opened));
}

impl Default for WebStore {
    /// The store, on the history [`preload`] opened; on the page storage,
    /// saying why, where it opened none.
    fn default() -> Self {
        let preloaded = PRELOADED.with(|preloaded| preloaded.borrow().clone());
        let (history, set_aside) = match preloaded {
            Some(Ok(database)) => (History::Database(database), Vec::new()),
            Some(Err((why, set_aside))) => (History::page(why), set_aside),
            None => (History::page("it was not opened".into()), Vec::new()),
        };
        Self {
            settings_held: Cell::default(),
            set_aside: RefCell::new(set_aside),
            asked_to_keep: Cell::default(),
            open_held: RefCell::default(),
            history,
        }
    }
}

impl History {
    /// The page storage, keeping the history this visit, as the database
    /// would not, for the reason given.
    fn page(why: String) -> Self {
        Self::Page {
            whole: BlobHistory::new(Local, BLOB_KEYS, HISTORY_BUDGET),
            why: RefCell::new(Some(why)),
        }
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// `localStorage`, looked up afresh each time it is used: a browser that
/// keeps nothing has none to read, and takes nothing.
struct Local;

impl Aside for Local {
    fn keys(&self) -> Result<Vec<String>, ()> {
        Shelf(&storage().ok_or(())?).keys()
    }

    fn get(&self, key: &str) -> Result<Option<String>, ()> {
        Shelf(&storage().ok_or(())?).get(key)
    }

    fn set(&self, key: &str, raw: &str) -> bool {
        storage().is_some_and(|store| Shelf(&store).set(key, raw))
    }

    fn remove(&self, key: &str) {
        if let Some(store) = storage() {
            Shelf(&store).remove(key);
        }
    }
}

/// `localStorage`, as what is set aside is kept in it ([`set_aside_from`]).
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

impl WebStore {
    /// Note `what` to tell of, if it was set aside now, and say whether it
    /// is kept aside, now or before.
    fn noted(&self, kept: Kept, what: SetAside) -> bool {
        if kept == Kept::Now {
            self.set_aside.borrow_mut().push(what);
        }
        kept != Kept::Not
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

    /// A browser that keeps nothing tells it once a visit.
    fn told_of_paddle_keys(&self) -> bool {
        storage().is_some_and(|store| {
            store
                .get_item(TOLD_OF_PADDLE_KEYS_KEY)
                .ok()
                .flatten()
                .is_some()
        })
    }

    fn mark_told_of_paddle_keys(&self) {
        if let Some(store) = storage() {
            let _ = store.set_item(TOLD_OF_PADDLE_KEYS_KEY, "told");
        }
    }

    /// A browser that keeps nothing counts it as asked: it holds no settings
    /// from before 0.16, so a lock on there is one chosen this visit.
    fn asked_about_lock(&self) -> bool {
        storage().is_none_or(|store| {
            store
                .get_item(ASKED_ABOUT_LOCK_KEY)
                .ok()
                .flatten()
                .is_some()
        })
    }

    fn mark_asked_about_lock(&self) {
        if let Some(store) = storage() {
            let _ = store.set_item(ASKED_ABOUT_LOCK_KEY, "asked");
        }
    }

    /// Settings that do not read at all are set aside before the defaults
    /// the app goes on with are written over them; until they are, nothing
    /// is.
    fn read_settings(&self) -> Option<TrainingSettings> {
        self.settings_held.set(false);
        let store = storage()?;
        let raw = store.get_item(SETTINGS_KEY).ok().flatten()?;
        let settings = read_settings_from(&raw);
        if settings.is_none() {
            let kept = set_aside_from(
                &Shelf(&store),
                SETTINGS_KEY,
                &raw,
                UNREADABLE_SETTINGS_KEY,
                &raw,
                None,
            );
            if !self.noted(kept, SetAside::Settings { kept: None }) {
                self.settings_held.set(true);
            }
        }
        settings
    }

    fn write_settings(&self, settings: &TrainingSettings) -> Result<(), String> {
        if self.settings_held.get() {
            return Err(SETTINGS_HELD.into());
        }
        let store = storage().ok_or("this browser has no storage for them")?;
        let raw = serde_json::to_string(settings).map_err(|err| err.to_string())?;
        store
            .set_item(SETTINGS_KEY, &raw)
            .map_err(|_| "this browser's storage would not take them".into())
    }

    /// Whether the history is held is this read's to say, whatever the last
    /// one said: a copy there was no room to set aside then may fit now.
    fn take_set_aside(&self) -> Vec<SetAside> {
        let mut set_aside = std::mem::take(&mut *self.set_aside.borrow_mut());
        if let History::Page { whole, .. } = &self.history {
            set_aside.extend(whole.take_set_aside());
        }
        set_aside
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
            marks
                .iter()
                .map(|mark| CutOff::read(mark.as_bytes()))
                .collect()
        })
    }

    /// The history read as the page opened ([`preload`]); or, kept in the
    /// page storage this visit, read now, and why told of once.
    fn load_history(&self) -> HistoryLoad {
        match &self.history {
            History::Database(database) => database.load(),
            History::Page { whole, why } => {
                let mut load = whole.load();
                if let Some(why) = why.borrow_mut().take() {
                    load.notices.push(HistoryNotice::PageStorage(why));
                }
                load
            }
        }
    }

    fn change_history(&self, change: HistoryChange) -> Pending<HistorySaved> {
        match &self.history {
            History::Database(database) => {
                let database = Rc::clone(database);
                Box::pin(async move { database.apply(change).await })
            }
            History::Page { .. } if storage().is_none() => done(HistorySaved::Failed(
                "this browser has no storage for it.".into(),
            )),
            History::Page { whole, .. } => done(whole.apply(&change)),
        }
    }

    fn history_listing(&self) -> Pending<Option<Vec<RecordId>>> {
        match &self.history {
            History::Database(database) => {
                let database = Rc::clone(database);
                Box::pin(async move { database.listing().await })
            }
            History::Page { whole, .. } => done(whole.listing()),
        }
    }

    fn read_records(&self, ids: Vec<RecordId>) -> Pending<Vec<Result<Record, RecordId>>> {
        match &self.history {
            History::Database(database) => {
                let database = Rc::clone(database);
                Box::pin(async move { database.read(ids).await })
            }
            History::Page { whole, .. } => done(whole.read(&ids)),
        }
    }

    fn forgotten(&self, ids: Vec<RecordId>) -> Pending<Vec<RecordId>> {
        match &self.history {
            History::Database(database) => {
                let database = Rc::clone(database);
                Box::pin(async move { database.forgotten(ids).await })
            }
            History::Page { whole, .. } => done(whole.forgotten(&ids)),
        }
    }

    fn history_changed(&self) -> bool {
        match &self.history {
            History::Database(database) => database.changed(),
            History::Page { whole, .. } => whole.changed(),
        }
    }

    /// Another tab's word of each change, as the database's channel gives
    /// it; none from the page storage, which is looked at again as the
    /// history is shown, as before.
    fn history_news(&self) -> Option<Pending<()>> {
        match &self.history {
            History::Database(database) => Some(database.news()),
            History::Page { .. } => None,
        }
    }
}
