//! The browser store: `localStorage` for all but the history, which the
//! browser's database keeps ([`super::idb`]), opened before the app starts
//! ([`preload`]).
//!
//! A browser keeps them, their room and its Web Locks for the whole origin,
//! and a GitHub Pages project site shares its origin with every other one
//! of the account (docs/platforms.md). So every name kept or locked here
//! starts with `dust_`, and only names of Dust's own are ever taken away: by
//! exact name, or by a prefix of its own
//! ([`keep_aside`](super::legacy::keep_aside), the marks' page script,
//! `src/persist/marks.js`).

use std::cell::{Cell, OnceCell, RefCell};
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::Poll;

use cw_core::{AutoLevelCounters, CountersKey, TrainingSettings};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::backup::{BackedUp, BackupFile, NotSaved};
use super::idb::{BrowserHistory, Script, waited_ms};
use super::legacy::{Aside, BlobHistory, BlobKeys, HISTORY_BUDGET, Kept, set_aside_from};
use super::marks::MARKS_JS;
use super::progress::{BOOT_NOTE_AFTER_MS, BOOT_NOTE_ID, LoadProgress};
use super::record::{
    HistoryChange, HistoryLoad, HistoryNotice, Pending, Record, RecordId, Unopened, done,
    failed_for_now,
};
use super::{
    BackupSaving, CutOff, CutOffTaking, HistorySaved, Lock, Locking, MarkId, OpenSession,
    SETTINGS_HELD, SetAside, Store, TakenMark, over_stored, read_settings_from,
};

const THEME_KEY: &str = "dust_theme";
const TUTORIAL_KEY: &str = "dust_tutorial_done";
const TOLD_OF_PADDLE_KEYS_KEY: &str = "dust_told_of_paddle_keys";
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
/// What sync keeps ([`Store::load_sync`]).
const SYNC_KEY: &str = "dust_sync";
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
    /// and takes it away with its lock, and what says it is in place
    /// ([`WebStore::hold_mark`]).
    open_held: RefCell<Option<JsValue>>,
    /// The marks' page script ([`MARKS_JS`]), once it has run: none where
    /// it would not.
    marks: OnceCell<Option<JsValue>>,
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
/// showing meanwhile, and saying how far the read has come once it has
/// taken a while ([`unless_stalled`]). The history kept whole in the page
/// storage is moved in first, where it was not ([`BrowserHistory::open_with`]).
/// Where the database will not open, will not take the history, or stops
/// answering as it is read, the page storage keeps it this visit
/// ([`WebStore::default`]).
pub async fn preload() {
    let progress = Arc::new(LoadProgress::default());
    let counted = Arc::clone(&progress);
    let opening = async move {
        match Script::open().await {
            Ok((script, read_only)) => {
                BrowserHistory::open_with(script, read_only, Local, BLOB_KEYS, counted)
                    .await
                    .map(Rc::new)
                    .map_err(|not| (not.why, not.set_aside))
            }
            Err(why) => Err((why, Vec::new())),
        }
    };
    let opened = unless_stalled(opening, &progress)
        .await
        .unwrap_or_else(|| Err(("it did not answer".into(), Vec::new())));
    PRELOADED.with(|preloaded| *preloaded.borrow_mut() = Some(opened));
}

/// How long the browser's database may keep the page waiting on one thing
/// it was asked, as the page opens, before the page storage keeps the
/// history this visit instead: far longer than one read of it takes, as a
/// long history is read a page at a time, but a database that has stopped
/// answering, as a browser's now and then does, no longer leaves the app
/// on its boot screen for good. Opening it waits no longer than the page
/// script's own wait, `OPEN_WAIT_MS`.
const STALLED_MS: f64 = 20_000.0;

/// How often the page looks at what it waits on as it opens: whether the
/// database has stalled, and how far the read has come.
const LOOK_MS: u32 = 250;

/// What `opening` comes to; or none, once the database has kept it waiting
/// for [`STALLED_MS`], looked at every [`LOOK_MS`] while it waits. Once the
/// page has shown its boot screen for [`BOOT_NOTE_AFTER_MS`], each look
/// says on it how far the read has come, as `progress` counts it.
async fn unless_stalled<T>(opening: impl Future<Output = T>, progress: &LoadProgress) -> Option<T> {
    let mut opening = pin!(opening);
    loop {
        let mut look = pin!(gloo_timers::future::TimeoutFuture::new(LOOK_MS));
        let opened = std::future::poll_fn(|asked| match opening.as_mut().poll(asked) {
            Poll::Ready(opened) => Poll::Ready(Some(opened)),
            Poll::Pending => look.as_mut().poll(asked).map(|()| None),
        })
        .await;
        if opened.is_some() {
            return opened;
        }
        // The page's own clock, which starts as the page loads: the boot
        // screen shows from its first frame.
        if crate::time::mono_ms() >= BOOT_NOTE_AFTER_MS {
            boot_note(&progress.now().said());
        }
        if waited_ms() > STALLED_MS {
            return None;
        }
    }
}

/// Say `said` on the page's boot screen, under its mark (`index.html`):
/// written from here, so the page runs no script of its own for it.
fn boot_note(said: &str) {
    let note = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(BOOT_NOTE_ID));
    if let Some(note) = note
        && note.text_content().as_deref() != Some(said)
    {
        note.set_text_content(Some(said));
    }
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
            marks: OnceCell::new(),
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

/// The script's `name`, called with `args`: what it gave back, or none where
/// it has no such function, or it threw.
fn call(script: &JsValue, name: &str, args: &[JsValue]) -> Option<JsValue> {
    let function: js_sys::Function = js_sys::Reflect::get(script, &JsValue::from_str(name))
        .ok()?
        .dyn_into()
        .ok()?;
    let args: js_sys::Array = args.iter().collect();
    function.apply(script, &args).ok()
}

/// What this tab does with the Web Locks it shares with the site's other
/// tabs, and what it hears of their writes of what sync keeps: the tabs'
/// script ([`crate::persist::lock::TABS_JS`]), run once for the page, as the
/// page is the tab.
mod tabs {
    use std::cell::RefCell;

    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::call;
    use crate::persist::lock::TABS_JS;
    use crate::persist::record::{Pending, done};
    use crate::persist::{LockHeld, Locking};

    thread_local! {
        /// The script, once run: none before, or where it would not run.
        static SCRIPT: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    }

    /// A wait that ends at another tab's next word on what sync keeps, a
    /// `storage` event for it: none where the script would not run.
    pub(super) fn heard() -> Option<Pending<()>> {
        let tabs = script()?;
        let waiting: js_sys::Promise = call(&tabs, "heard", &[])?.dyn_into().ok()?;
        Some(Box::pin(async move {
            let _ = JsFuture::from(waiting).await;
        }))
    }

    /// Try the Web Lock `name`, never waiting for it.
    pub(super) fn try_lock(name: &str) -> Pending<Locking> {
        let Some(tabs) = script() else {
            return done(Locking::Unlockable);
        };
        let Some(asked) = call(&tabs, "tryLock", &[JsValue::from_str(name)]) else {
            return done(Locking::Unlockable);
        };
        let answer = js_sys::Promise::resolve(&asked);
        let mut waiting = Unanswered {
            tabs,
            answer: Some(answer.clone()),
        };
        Box::pin(async move {
            let answer = JsFuture::from(answer).await;
            // Answered, and taken: nothing to hand back.
            waiting.answer.take();
            locking(answer)
        })
    }

    /// The script, run as it is first wanted.
    fn script() -> Option<JsValue> {
        SCRIPT.with(|script| {
            let mut script = script.borrow_mut();
            if script.is_none() {
                let make = js_sys::Function::new_with_args("options", TABS_JS);
                *script = make.call1(&JsValue::NULL, &JsValue::UNDEFINED).ok();
            }
            script.clone()
        })
    }

    /// What the script's answer to a lock tried says: a function to let go
    /// of it with, held; `false`, busy; anything else, as `null` from a
    /// browser without Web Locks, no lock.
    fn locking(answer: Result<JsValue, JsValue>) -> Locking {
        match answer.map(JsCast::dyn_into::<js_sys::Function>) {
            Ok(Ok(release)) => Locking::Held(LockHeld::new(Release(release))),
            Ok(Err(busy)) if busy == JsValue::FALSE => Locking::Busy,
            _ => Locking::Unlockable,
        }
    }

    /// A Web Lock this tab holds, let go of as this is dropped: the function
    /// the script handed over with it.
    struct Release(js_sys::Function);

    impl Drop for Release {
        fn drop(&mut self) {
            let _ = self.0.call0(&JsValue::NULL);
        }
    }

    /// The script's answer to a lock tried, while it is waited for: one no
    /// longer waited for is handed back to the script, which lets go of the
    /// lock should the browser grant it, so that no tab holds a lock it
    /// knows nothing of until it closes.
    struct Unanswered {
        tabs: JsValue,
        answer: Option<js_sys::Promise>,
    }

    impl Drop for Unanswered {
        fn drop(&mut self) {
            if let Some(answer) = self.answer.take() {
                let _ = call(&self.tabs, "dropped", &[answer.into()]);
            }
        }
    }
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
    /// The marks' page script ([`MARKS_JS`]), run as it is first wanted.
    fn marks(&self) -> Option<&JsValue> {
        self.marks
            .get_or_init(|| {
                let make = js_sys::Function::new_with_args("prefix, options", MARKS_JS);
                make.call2(
                    &JsValue::NULL,
                    &JsValue::from_str(OPEN_PREFIX),
                    &JsValue::UNDEFINED,
                )
                .ok()
            })
            .as_ref()
    }

    /// Hold the Web Lock `name`, and keep the mark of the same name in
    /// `localStorage` only while the lock is held: what is handed back
    /// writes the mark's text, or, given none, takes the mark away and lets
    /// go of the lock (`write`), and says when the mark holds the text it
    /// was last given, as a write the page storage has no room for leaves
    /// it holding what it held before (`inPlace`). None in a browser without
    /// Web Locks, where nothing is marked, so no running tab is ever taken
    /// for one cut off. The mark goes in only once the lock is granted,
    /// under the lock pages look for marks under (`src/persist/marks.js`).
    fn hold_mark(&self, name: &str) -> Option<JsValue> {
        call(self.marks()?, "hold", &[JsValue::from_str(name)]).filter(JsValue::is_object)
    }

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

    /// Each key of the site's page storage under the counters' own prefix,
    /// and only those: what other sites keep there is never read.
    fn every_auto_counter(&self) -> Vec<(CountersKey, AutoLevelCounters)> {
        let Some(store) = storage() else {
            return Vec::new();
        };
        let count = store.length().unwrap_or(0);
        (0..count)
            .filter_map(|at| store.key(at).ok().flatten())
            .filter_map(|key| {
                let name = key.strip_prefix(AUTO_PREFIX)?.to_string();
                let raw = store.get_item(&key).ok().flatten()?;
                let counters = serde_json::from_str(&raw).ok()?;
                Some((CountersKey::named(name), counters))
            })
            .collect()
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
    ///
    /// A browser hands a file over whole, so the file is written whole: from
    /// the database, a record at a time as each is stored
    /// ([`BrowserHistory::back_up`]); or from the history the page storage
    /// keeps whole, where it keeps it this visit. Where neither can be
    /// read, as where the browser blocks the site's storage, the file holds
    /// what the app does, once the app has given every session it holds
    /// ([`NotSaved::Unread`]). A session deleted in the page storage is
    /// gone from the app as soon as it is deleted, here or, taken up first,
    /// in another tab.
    fn save_backup(&self, name: &str, backup: BackupFile) -> BackupSaving {
        let written: Pending<Result<(String, BackedUp), NotSaved>> = match &self.history {
            History::Database(database) => {
                let database = Rc::clone(database);
                Box::pin(async move { database.back_up(&backup).await })
            }
            History::Page { whole, .. } => done(backup.whole(whole.records(), |_| false)),
        };
        let name = name.to_owned();
        Box::pin(async move {
            let (contents, backed) = written.await?;
            download(&name, &contents)
                .map(|()| {
                    let handed = format!(
                        "Backup handed to your browser to save as {name}: look for it in your downloads."
                    );
                    match backed.told() {
                        Some(told) => format!("{handed} {told}"),
                        None => handed,
                    }
                })
                .ok_or_else(|| NotSaved::Failed("this browser would not hand over the file".into()))
        })
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
                    self.hold_mark(&format!("{OPEN_PREFIX}{:016x}", crate::time::seed_rng()))
                });
                let Some(mark) = mark else {
                    return;
                };
                if let Ok(raw) = serde_json::to_string(&open) {
                    let _ = call(&mark, "write", &[JsValue::from_str(&raw)]);
                }
                *held = Some(mark);
            }
            None => {
                if let Some(mark) = held.take() {
                    let _ = call(&mark, "write", &[JsValue::NULL]);
                }
            }
        }
    }

    /// Every mark whose page has gone, and no tab holds, taken and held by
    /// this one, and what each said: one look at a time across tabs, so two
    /// opening at once tell of it once (`src/persist/marks.js`).
    fn take_cut_off(&self) -> CutOffTaking {
        let marks = self.marks().cloned();
        Box::pin(async move {
            let Some(taking) = marks.and_then(|marks| call(&marks, "take", &[])) else {
                return Vec::new();
            };
            let Ok(marks) = JsFuture::from(js_sys::Promise::resolve(&taking)).await else {
                return Vec::new();
            };
            js_sys::Array::from(&marks)
                .iter()
                .filter_map(|mark| {
                    let mark = js_sys::Array::from(&mark);
                    let name = mark.get(0).as_string()?;
                    let said = mark.get(1).as_string().unwrap_or_default();
                    Some(TakenMark {
                        id: MarkId::new(name),
                        cut: CutOff::read(said.as_bytes()),
                    })
                })
                .collect()
        })
    }

    /// Those `left` are let go of there and then; those `carried` go once
    /// this page's mark holds the text it was last given, holding them, or
    /// at once where it has none (`src/persist/marks.js`): every write of a
    /// mark in place goes into `localStorage` then and there, or, where the
    /// page storage has no room for it, leaves the marks it carries for
    /// waiting on one that does.
    fn release_cut_off(&self, carried: Vec<MarkId>, left: Vec<MarkId>) {
        let Some(marks) = self.marks() else {
            return;
        };
        let names = |ids: &[MarkId]| -> js_sys::Array {
            ids.iter().map(|id| JsValue::from_str(id.name())).collect()
        };
        let own = self.open_held.borrow().clone().unwrap_or(JsValue::NULL);
        let _ = call(
            marks,
            "release",
            &[names(&carried).into(), names(&left).into(), own],
        );
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

    /// A browser's database reads each record whole or not at all, and the
    /// page storage holds text alone: one that cannot be read could not be
    /// read just now.
    fn read_records(&self, ids: Vec<RecordId>) -> Pending<Vec<Result<Record, Unopened>>> {
        match &self.history {
            History::Database(database) => {
                let database = Rc::clone(database);
                Box::pin(async move { failed_for_now(database.read(ids).await) })
            }
            History::Page { whole, .. } => done(failed_for_now(whole.read(&ids))),
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

    fn load_sync(&self) -> Option<String> {
        storage()?.get_item(SYNC_KEY).ok().flatten()
    }

    fn sync_cleared(&self) -> bool {
        storage().is_some_and(|store| matches!(store.get_item(SYNC_KEY), Ok(None)))
    }

    fn save_sync(&self, state: Option<&str>) -> Result<(), String> {
        let store = storage().ok_or("this browser keeps nothing for this site")?;
        match state {
            Some(state) => store
                .set_item(SYNC_KEY, state)
                .map_err(|_| "this browser's storage would not take it".into()),
            None => store
                .remove_item(SYNC_KEY)
                .map_err(|_| "this browser's storage would not let it go".into()),
        }
    }

    /// Only the browser's database keeps the history by record.
    fn syncs_history(&self) -> bool {
        matches!(self.history, History::Database(_))
    }

    /// A Web Lock of the site's ([`Lock::web_name`]), asked for only if it
    /// is free: answered a moment later, as the browser grants it.
    fn try_lock(&self, lock: Lock) -> Pending<Locking> {
        match lock.web_name() {
            Some(name) => tabs::try_lock(name),
            None => done(Locking::Unlockable),
        }
    }

    /// Another tab's word of each write of what sync keeps, a `storage`
    /// event the browser fires in every other tab of the site.
    fn sync_news(&self) -> Option<Pending<()>> {
        tabs::heard()
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
