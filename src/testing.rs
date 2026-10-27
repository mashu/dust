//! A whole app runtime, driven by hand.
//!
//! The session runtime is asynchronous by nature — sends, gaps, timeouts and
//! auto-confirms all run as spawned tasks — so the only honest way to test it
//! is to run it. This harness gives those tasks a real Dioxus runtime to live
//! in, a fake player in place of a sound card, and a paused clock that is
//! advanced a tick at a time, so a whole training session runs in milliseconds
//! and lands on exactly the same instants every time.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cw_core::{
    GroupSource, MonoMs, SendStart, SessionEffect, SessionEvent, SessionId, SessionMachine,
    SessionResult, TrainingSettings,
};
use dioxus::core::{ElementId, NoOpMutations};
use dioxus::prelude::*;

use crate::app::SETTINGS_SAVE_DELAY_MS;
use crate::audio::fake::{Behaviour, Call, Recorder};
use crate::persist::Store;
use crate::persist::memory::MemoryStore;
use crate::player::Lease;
use crate::session_runtime::{send_event, start_session};
use crate::state::{AppCtx, AppState, Notice, Screen};
use crate::time::POLL_MS;
use crate::ui::widgets::DISCLOSURE;

/// Clock step per pump. Smaller than one poll interval, so nothing is skipped
/// over.
const STEP_MS: u64 = 4;

/// How deep a disclosure may nest before the harness calls it a mistake.
const NESTED_DISCLOSURES: usize = 8;

/// How many rounds of rendering one event may set off before the page is
/// said never to settle. Real chains are a handful deep.
const SETTLE_PASSES: usize = 32;

thread_local! {
    /// The fields the page has asked to give the cursor to, by id, each
    /// with the script a browser would run for it, that no page has looked
    /// for yet.
    static FOCUS_ASKED: std::cell::RefCell<Vec<(String, String)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// How many times the page has asked to be scrolled to its top since a
    /// test last looked.
    static SCROLLS_ASKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// What listens for a click anywhere off a widget's own squares.
    static ELSEWHERE: std::cell::RefCell<Vec<Callback<()>>> = const { std::cell::RefCell::new(Vec::new()) };
    /// How many times each component that tells of it has rendered since a
    /// test last looked, by name.
    static RENDERS: std::cell::RefCell<Vec<(&'static str, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The page asks for the field with this id to have the cursor, by
/// `script`. In a browser the script runs at once, on the page as drawn so
/// far; here the [`Ui`] runs it when the page next settles, keeping the
/// script to say where in the field the cursor went
/// ([`Ui::take_focus_scripts`]).
pub fn ask_focus(id: &str, script: &str) {
    FOCUS_ASKED.with_borrow_mut(|asked| asked.push((id.to_string(), script.to_string())));
}

/// The page asks to be scrolled back to its top. In a browser that is a
/// script; here a [`Ui`] counts it ([`Ui::take_scrolls`]).
pub fn ask_scroll_to_top() {
    SCROLLS_ASKED.set(SCROLLS_ASKED.get() + 1);
}

/// The page listens for a click anywhere off a widget's own squares, and for
/// Escape. In a browser a script listens on the document; here a [`Ui`] calls
/// `on_elsewhere` when a test clicks elsewhere ([`Ui::click_elsewhere`]).
pub fn listen_elsewhere(on_elsewhere: Callback<()>) {
    ELSEWHERE.with_borrow_mut(|heard| heard.push(on_elsewhere));
}

/// A component costly to draw has rendered: a test can count how often
/// ([`Ui::take_renders`]), and so tell a hover that redraws a mark from one
/// that redraws a whole tab.
pub fn rendered(component: &'static str) {
    RENDERS.with_borrow_mut(|renders| {
        match renders.iter_mut().find(|(name, _)| *name == component) {
            Some((_, count)) => *count += 1,
            None => renders.push((component, 1)),
        }
    });
}

pub struct Harness {
    dom: VirtualDom,
    /// The app's state, as every screen sees it.
    pub ctx: AppCtx,
    /// What the app holds outside it: the player and the session.
    pub app: AppState,
    pub recorder: Rc<Recorder>,
    /// What the app has stored.
    pub store: Rc<MemoryStore>,
}

/// Hands a session the groups it is given, in order.
struct Scripted(std::collections::VecDeque<String>);

impl GroupSource for Scripted {
    fn next_group(&mut self, _: &TrainingSettings) -> (String, u32) {
        (self.0.pop_front().unwrap_or_else(|| "KM".into()), 1)
    }

    fn learn(&mut self, _: cw_core::Answered<'_>) {}
}

/// One finished session, the way the app stores it: each group sent, and
/// answered half a second after its send.
pub fn stored_session(date: &str, sent: &[(&str, &str)]) -> SessionResult {
    let mut settings = test_settings();
    settings.curriculum.num_groups = sent.len().max(1) as u32;
    let texts = sent.iter().map(|(text, _)| (*text).to_string()).collect();
    let (mut machine, _) =
        SessionMachine::start(SessionId::new(1), MonoMs(0), settings, Scripted(texts));
    let mut now = MonoMs(0);
    for (index, (text, answer)) in sent.iter().enumerate() {
        // Its characters end evenly over its half second; nothing typed is
        // timed.
        let chars = text.chars().count().max(1) as u32;
        let start = SendStart {
            char_wpm: 40.0,
            effective_wpm: 40.0,
            output_at: now,
            char_ends_ms: (1..=chars).map(|at| at * 500 / chars).collect(),
            latency_ms: 0,
        };
        let _ = machine.apply(SessionEvent::PlaybackStarted { index, start }, now);
        now = now.plus(500);
        let _ = machine.apply(
            SessionEvent::PlaybackEnded {
                index,
                duration_sec: 0.5,
            },
            now,
        );
        now = now.plus(500);
        let typed = SessionEvent::Input {
            index,
            text: (*answer).to_string(),
            keys: cw_core::Keys::default(),
        };
        let _ = machine.apply(typed, now);
        for effect in machine.apply(SessionEvent::Confirm, now) {
            if let SessionEffect::StartTimer { id, .. } = effect {
                let _ = machine.apply(SessionEvent::TimerFired { id }, now);
            }
        }
    }
    machine.session().result(1_000, date.to_string())
}

/// A finished ten-minute WPX run, the way the app stores it: each QSO's
/// call and number as the station sent them and as you logged them, one a
/// minute.
pub fn stored_run(date: &str, qsos: &[(&str, &str, &str, &str)]) -> SessionResult {
    let copies: Vec<Vec<(&str, &str)>> = qsos
        .iter()
        .map(|(call, nr, logged_call, logged_nr)| vec![(*call, *logged_call), (*nr, *logged_nr)])
        .collect();
    stored_contest_run(date, "wpx", &["nr"], &copies)
}

/// `run` as it is kept with Show copy on: marked assisted, and logged so.
pub fn assisted(mut run: SessionResult) -> SessionResult {
    run.alphabet_fingerprint.push_str(":a");
    if let Some(log) = run.contest.as_mut() {
        log.0["assisted"] = serde_json::json!(true);
    }
    run
}

/// A finished ten-minute Simple QSO run, the way the app stores it: each
/// QSO's call, report and name as the station sent them, each beside what
/// you logged, one a minute.
pub fn stored_simple_qso_run(date: &str, qsos: &[[(&str, &str); 3]]) -> SessionResult {
    let copies: Vec<Vec<(&str, &str)>> = qsos.iter().map(|qso| qso.to_vec()).collect();
    stored_contest_run(date, "simpleQso", &["rst", "name"], &copies)
}

/// A finished ten-minute NA Sprint run, the way the app stores it: each
/// QSO's call, serial, name and state as the station sent them, each beside
/// what you logged, one a minute.
pub fn stored_sprint_run(date: &str, qsos: &[[(&str, &str); 4]]) -> SessionResult {
    let copies: Vec<Vec<(&str, &str)>> = qsos.iter().map(|qso| qso.to_vec()).collect();
    stored_contest_run(date, "sprint", &["nr", "name", "st"], &copies)
}

/// A finished ten-minute Field Day run, the way the app stores it: each
/// QSO's call, class and section as the station sent them, each beside
/// what you logged, one a minute.
pub fn stored_field_day_run(date: &str, qsos: &[[(&str, &str); 3]]) -> SessionResult {
    let copies: Vec<Vec<(&str, &str)>> = qsos.iter().map(|qso| qso.to_vec()).collect();
    stored_contest_run(date, "fieldDay", &["cl", "sec"], &copies)
}

/// A finished ten-minute run of the contest `kind`, copying `fields`: each
/// QSO its groups, sent beside received, one a minute.
fn stored_contest_run(
    date: &str,
    kind: &str,
    fields: &[&str],
    qsos: &[Vec<(&str, &str)>],
) -> SessionResult {
    let group = |sent: &str, received: &str| cw_core::GroupResult {
        sent: sent.to_string(),
        received: received.to_string(),
        correct: sent == received,
    };
    let groups: Vec<cw_core::GroupResult> = qsos
        .iter()
        .flat_map(|qso| qso.iter().map(|(sent, received)| group(sent, received)))
        .collect();
    let mut tally = cw_core::CopyTally::default();
    for group in &groups {
        tally.add(&group.sent, &group.received);
    }
    let serials = fields.contains(&"nr");
    let records: Vec<serde_json::Value> = (1..=qsos.len() as u64)
        .zip(qsos)
        .map(|(n, qso)| {
            let nr = if serials { n } else { 0 };
            serde_json::json!({ "t": n * 60_000, "h": n * 60_000 - 20_000, "g": qso.len(), "nr": nr })
        })
        .collect();
    let log = serde_json::json!({
        "v": 1,
        "kind": kind,
        "style": "single",
        "callers": [1, 1],
        "side": "",
        "myCountry": "DL",
        "myContinent": "",
        "myZone": 0,
        "fields": fields,
        "limitS": 600,
        "runMs": 600_000,
        "pausedMs": 0,
        "myWpm": 26.0,
        "qsos": records,
    });
    let mut run = SessionResult {
        date: date.to_string(),
        timestamp: 1_760_000_000_000,
        group_timings: groups
            .iter()
            .map(|_| cw_core::SessionTiming {
                time_to_complete_ms: 900.0,
                char_wpm: Some(26.0),
                effective_wpm: Some(26.0),
                sends: 1,
                ..cw_core::SessionTiming::default()
            })
            .collect(),
        groups,
        accuracy: tally.accuracy(),
        tally,
        char_set_mode: cw_core::CharSetMode::Contest,
        alphabet_fingerprint: format!("contest:{kind}"),
        contest: Some(cw_core::ContestLog(log)),
        char_wpm: 26.0,
        effective_wpm: 26.0,
        ..SessionResult::empty_for_test()
    };
    run.score = run.points();
    run
}

/// Settings that make a session short and predictable to test.
pub fn test_settings() -> TrainingSettings {
    let mut settings = TrainingSettings::default();
    settings.curriculum.char_set_mode = cw_core::CharSetMode::Koch;
    settings.curriculum.level = 1;
    settings.curriculum.num_groups = 2;
    settings.curriculum.min_group_size = 2;
    settings.curriculum.max_group_size = 2;
    settings.playback.char_wpm_min = 40.0;
    settings.playback.char_wpm_max = 40.0;
    settings.playback.effective_wpm_min = 40.0;
    settings.playback.effective_wpm_max = 40.0;
    settings.playback.group_timeout = 5.0;
    // Sessions start sending at once; the count down has tests of its own.
    settings.playback.countdown_sec = 0;
    settings.playback.lock_input_during_group_playback = true;
    settings.playback.group_repeat_min = 1;
    settings.playback.group_repeat_max = 1;
    settings.playback.link_group_repeat = true;
    settings.auto_level.auto_adjust_level = false;
    settings.band.qrn_enabled = false;
    settings.band.noise_enabled = false;
    settings.band.qsb_enabled = false;
    settings
}

/// The line under a call, as Your station and Home write it: "Reads as"
/// and `parts`, each kept on one line, a dot between them.
pub fn reads_as(parts: &[&str]) -> String {
    let parts: Vec<String> = parts
        .iter()
        .map(|part| part.replace(' ', "\u{a0}"))
        .collect();
    format!("Reads as {}", parts.join("\u{a0}· "))
}

/// Settings that run a contest at once: WPX as DL1ABC, every speed 30 WPM,
/// a quiet band with the fading on, which the run's turns leave off.
pub fn contest_settings() -> TrainingSettings {
    let mut settings = test_settings();
    settings.curriculum.char_set_mode = cw_core::CharSetMode::Contest;
    settings.contest.my_call = "DL1ABC".to_string();
    settings.playback.char_wpm_min = 30.0;
    settings.playback.char_wpm_max = 30.0;
    settings.playback.effective_wpm_min = 30.0;
    settings.playback.effective_wpm_max = 30.0;
    settings.band.qsb_enabled = true;
    settings.band.qsb_depth = 0.6;
    settings
}

impl Harness {
    pub fn new() -> Self {
        Self::with_settings(test_settings())
    }

    pub fn with_settings(settings: TrainingSettings) -> Self {
        let dom = VirtualDom::new(|| rsx! {});
        let recorder = Recorder::new();
        let store = Rc::new(MemoryStore::default());
        let app = AppState::with(recorder.factory(), store.clone());
        let ctx = dom.in_scope(ScopeId::ROOT, || {
            AppCtx::new(app.clone(), settings, Vec::new())
        });
        Self {
            dom,
            ctx,
            app,
            recorder,
            store,
        }
    }

    /// Run `f` with a Dioxus runtime and scope in place, the way a component
    /// callback runs.
    pub fn in_app<T>(&self, f: impl FnOnce() -> T) -> T {
        self.dom.in_scope(ScopeId::ROOT, f)
    }

    /// Let every task that is ready run, then settle the virtual DOM.
    pub fn pump(&mut self) {
        self.dom.process_events();
        self.dom.render_immediate(&mut NoOpMutations);
    }

    /// Move the clock forward, pumping as it goes.
    pub async fn advance(&mut self, ms: u64) {
        let steps = ms.div_ceil(STEP_MS).max(1);
        for _ in 0..steps {
            tokio::time::advance(Duration::from_millis(STEP_MS)).await;
            tokio::task::yield_now().await;
            self.pump();
        }
    }

    /// Count each time `signal` is written from here on, waking whatever
    /// reads it: a screen that shows it would be drawn again.
    pub fn count_writes<T: 'static>(&self, signal: Signal<T>) -> Arc<AtomicUsize> {
        let writes = Arc::new(AtomicUsize::new(0));
        let counted = writes.clone();
        self.in_app(|| {
            let watcher = dioxus::core::ReactiveContext::new_with_callback(
                move || {
                    counted.fetch_add(1, Ordering::Relaxed);
                },
                ScopeId::ROOT,
                std::panic::Location::caller(),
            );
            watcher.run_in(|| {
                let _ = signal.read();
            });
        });
        writes
    }

    /// Start a session the way the Practice screen's button does: through
    /// the same call.
    pub fn start_training(&mut self) {
        let ctx = self.ctx;
        self.in_app(|| start_session(ctx));
        self.pump();
    }

    /// The running session's hold on the audio.
    pub fn lease(&self) -> Lease {
        self.app
            .session
            .borrow()
            .as_ref()
            .map(|live| live.lease().clone())
            .expect("a session is running")
    }

    /// Send one event the way a component callback does.
    pub fn send(&mut self, event: SessionEvent) {
        let ctx = self.ctx;
        self.in_app(|| send_event(ctx, event));
        self.pump();
    }

    pub fn type_answer(&mut self, index: usize, text: &str) {
        self.send(SessionEvent::Input {
            index,
            text: text.to_string(),
            keys: cw_core::Keys::default(),
        });
    }

    /// What the trainer sent for `index`, as the user would have heard it.
    pub fn sent(&self, index: usize) -> String {
        self.ctx
            .runtime
            .peek()
            .as_ref()
            .and_then(|session| session.group(index).map(|g| g.sent().to_string()))
            .unwrap_or_default()
    }

    pub fn phase(&self) -> Option<cw_core::SessionPhase> {
        self.ctx
            .runtime
            .peek()
            .as_ref()
            .map(|session| session.phase())
    }

    pub fn screen(&self) -> Screen {
        *self.ctx.screen.peek()
    }

    pub fn toast(&self) -> Option<String> {
        self.ctx.toasts.current().map(|toast| toast.text)
    }

    /// What became of the sessions before, where it was not by your hand.
    pub fn notices(&self) -> Vec<Notice> {
        self.ctx.notices.peek().clone()
    }

    pub fn calls(&self) -> Vec<Call> {
        self.recorder.calls()
    }

    pub fn texts(&self) -> Vec<String> {
        self.recorder.texts()
    }

    pub fn set_behaviour(&self, behaviour: Behaviour) {
        self.recorder.set(behaviour);
    }

    /// Send the page to the background, or bring it back — a tab switch, a
    /// locked phone. The audio clock parks while it is away.
    pub fn set_page_hidden(&self, hidden: bool) {
        self.recorder.set_page_hidden(hidden);
    }

    /// Run until `done` or the budget runs out. Returns whether it finished.
    pub async fn run_until(&mut self, budget_ms: u64, mut done: impl FnMut(&Self) -> bool) -> bool {
        let mut waited = 0;
        while waited < budget_ms {
            if done(self) {
                return true;
            }
            self.advance(u64::from(POLL_MS)).await;
            waited += u64::from(POLL_MS);
        }
        done(self)
    }

    /// Answer every group correctly until the session ends.
    pub async fn play_through(&mut self, budget_ms: u64) {
        let mut waited = 0;
        let mut answered = vec![false; self.sent_count()];
        while waited < budget_ms && self.screen() != Screen::Results {
            if let Some(index) = self.awaiting_answer()
                && !answered.get(index).copied().unwrap_or(true)
            {
                answered[index] = true;
                let text = self.sent(index);
                self.type_answer(index, &text);
            }
            self.advance(u64::from(POLL_MS)).await;
            waited += u64::from(POLL_MS);
        }
    }

    fn sent_count(&self) -> usize {
        self.ctx
            .runtime
            .peek()
            .as_ref()
            .map(|session| session.group_count())
            .unwrap_or(0)
    }

    /// The group whose answer box is open, if any.
    pub fn awaiting_answer(&self) -> Option<usize> {
        match self.phase() {
            Some(cw_core::SessionPhase::AwaitingAnswer { index }) => Some(index),
            _ => None,
        }
    }
}

/// Run a test body on a paused clock, its seeds fresh unless it pins them
/// ([`crate::time::pin_seeds`]).
pub fn run<F: std::future::Future<Output = ()>>(body: impl FnOnce() -> F) {
    crate::time::reset_mono_clock();
    crate::time::pin_seeds(None);
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("test runtime")
        .block_on(body());
}

/// A rendered page, addressed the way its markup names things.
///
/// Dioxus mounts elements in an order of its own, so counting listeners is no
/// way to find a button. Every interactive control in the app carries an `id`
/// instead, built by [`crate::ui::widgets::control_id`], and the renderer
/// reports those ids against the live elements — so a test presses
/// `btn-start-training`, not "click number four". An id written as a plain
/// literal is part of the template and never reported, which is why they are
/// all built at runtime.
pub struct Ui {
    dom: VirtualDom,
    /// Live elements by the `id` their markup gives them.
    ids: Vec<(String, ElementId)>,
    /// Mutations since the last time anybody asked.
    work: usize,
    /// The fields given the cursor since the last time anybody asked, by
    /// id, each with its focus script. A focus script finds only a field
    /// that is drawn when it runs, so one asked for before the page has it
    /// is not among them.
    focused: Vec<(String, String)>,
    /// What the app on the page has stored.
    pub store: Rc<MemoryStore>,
}

/// Ids and removals, as the renderer reports them.
#[derive(Default)]
struct IdLog {
    named: Vec<(String, ElementId)>,
    gone: Vec<ElementId>,
    /// Mutations the renderer would have had to carry out. On the desktop
    /// build every one of these crosses to the webview, so it is the closest
    /// thing a headless test has to "how much did that cost".
    work: usize,
}

impl dioxus::core::WriteMutations for IdLog {
    fn append_children(&mut self, _: ElementId, _: usize) {
        self.work += 1;
    }
    fn assign_node_id(&mut self, _: &'static [u8], _: ElementId) {}
    fn create_placeholder(&mut self, _: ElementId) {}
    fn create_text_node(&mut self, _: &str, _: ElementId) {
        self.work += 1;
    }
    fn load_template(&mut self, _: dioxus::core::Template, _: usize, _: ElementId) {
        self.work += 1;
    }
    fn replace_node_with(&mut self, id: ElementId, _: usize) {
        self.work += 1;
        self.gone.push(id);
    }
    fn replace_placeholder_with_nodes(&mut self, _: &'static [u8], _: usize) {}
    fn insert_nodes_after(&mut self, _: ElementId, _: usize) {}
    fn insert_nodes_before(&mut self, _: ElementId, _: usize) {}
    fn set_attribute(
        &mut self,
        name: &'static str,
        _: Option<&'static str>,
        value: &dioxus::core::AttributeValue,
        id: ElementId,
    ) {
        self.work += 1;
        if name != "id" {
            return;
        }
        if let dioxus::core::AttributeValue::Text(text) = value {
            self.named.push((text.clone(), id));
        }
    }
    fn set_node_text(&mut self, _: &str, _: ElementId) {
        self.work += 1;
    }
    fn create_event_listener(&mut self, _: &'static str, _: ElementId) {}
    fn remove_event_listener(&mut self, _: &'static str, _: ElementId) {}
    fn remove_node(&mut self, id: ElementId) {
        self.work += 1;
        self.gone.push(id);
    }
    fn push_root(&mut self, _: ElementId) {}
}

impl Ui {
    pub fn new<P: Clone + 'static, M: 'static>(
        component: impl dioxus::core::ComponentFunction<P, M>,
        props: P,
    ) -> Self {
        Self::from_dom(VirtualDom::new_with_props(component, props), Rc::default())
    }

    /// The whole app, with a player that records instead of one that needs a
    /// sound card.
    pub fn app() -> (Self, Rc<Recorder>) {
        Self::app_with_settings(TrainingSettings::default())
    }

    pub fn app_with_settings(settings: TrainingSettings) -> (Self, Rc<Recorder>) {
        Self::app_with_history(settings, &[])
    }

    pub fn app_with_history(
        settings: TrainingSettings,
        sessions: &[SessionResult],
    ) -> (Self, Rc<Recorder>) {
        let store = Rc::new(MemoryStore::default());
        store.save_settings(&settings);
        store.save_sessions(sessions);
        store.settings_saves.set(0);
        // Past the first launch, which opens the guided setup by itself.
        store.mark_tutorial_done();
        Self::launch(store)
    }

    /// The app, launched on whatever `store` holds: a first launch on one
    /// that holds nothing.
    pub fn launch(store: Rc<MemoryStore>) -> (Self, Rc<Recorder>) {
        let recorder = Recorder::new();
        let state = AppState::with(recorder.factory(), store.clone());
        (
            Self::from_dom(
                VirtualDom::new(crate::app::App).with_root_context(state),
                store,
            ),
            recorder,
        )
    }

    fn from_dom(dom: VirtualDom, store: Rc<MemoryStore>) -> Self {
        // What an earlier page asked for is not this one's to do.
        FOCUS_ASKED.take();
        SCROLLS_ASKED.take();
        ELSEWHERE.take();
        let mut ui = Self {
            dom,
            ids: Vec::new(),
            work: 0,
            focused: Vec::new(),
            store,
        };
        let mut log = IdLog::default();
        ui.dom.rebuild(&mut log);
        ui.apply(log);
        ui.settle();
        ui
    }

    fn apply(&mut self, log: IdLog) {
        self.work += log.work;
        for id in log.gone {
            self.ids.retain(|(_, held)| *held != id);
        }
        // An element id is live only once, so a new name for one retires
        // whatever was recorded against it before, and the other way round.
        for (name, id) in log.named {
            self.ids
                .retain(|(held_name, held_id)| *held_id != id && *held_name != name);
            self.ids.push((name, id));
        }
    }

    /// Settle everything the last event set off: the render it dirtied, the
    /// effects that render queues, and whatever those effects dirty in turn,
    /// until nothing is left that could run.
    ///
    /// A page that never gets there — an effect that keeps waking itself —
    /// fails here, rather than passing every test that happened to look at
    /// it between two of its passes.
    pub fn settle(&mut self) {
        for _ in 0..SETTLE_PASSES {
            let work = self.has_work();
            self.run_focus_scripts();
            if !work {
                return;
            }
            let mut log = IdLog::default();
            self.dom.render_immediate(&mut log);
            self.apply(log);
        }
        panic!("the page never settled: something keeps waking itself");
    }

    /// Run the focus scripts the page has asked for, on the page as it is
    /// drawn now: before the render an event or a callback sets off, and
    /// after the one an effect follows.
    fn run_focus_scripts(&mut self) {
        for (id, script) in FOCUS_ASKED.take() {
            if self.shows(&id) {
                self.focused.push((id, script));
            }
        }
    }

    /// The fields the page gave the cursor to since this or
    /// [`Self::take_focus_scripts`] was last called, by id, in order.
    pub fn take_focused(&mut self) -> Vec<String> {
        self.take_focus_scripts()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// The same, each with the script that gave it the cursor: whether it
    /// went to the end of what the field holds, say.
    pub fn take_focus_scripts(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.focused)
    }

    /// How many times the page asked to be scrolled to its top since this
    /// was last called.
    pub fn take_scrolls(&mut self) -> usize {
        SCROLLS_ASKED.take()
    }

    /// How many times `component` rendered since this was last called for
    /// it ([`rendered`]).
    pub fn take_renders(&mut self, component: &str) -> usize {
        RENDERS.with_borrow_mut(|renders| {
            renders
                .iter_mut()
                .find(|(name, _)| *name == component)
                .map_or(0, |(_, count)| std::mem::take(count))
        })
    }

    /// Whether anything is ready to run: a task woken, an effect queued, a
    /// component marked for redrawing. Runs the tasks and effects it finds,
    /// as the renderer would before drawing.
    fn has_work(&mut self) -> bool {
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let work = std::pin::pin!(self.dom.wait_for_work());
        work.poll(&mut cx).is_ready()
    }

    /// The settings as stored, once an edit has had time to be.
    pub async fn stored_settings(&mut self) -> TrainingSettings {
        self.advance(u64::from(SETTINGS_SAVE_DELAY_MS) + STEP_MS)
            .await;
        self.store.load_settings()
    }

    /// Move the clock forward, settling the page as it goes.
    pub async fn advance(&mut self, ms: u64) {
        let steps = ms.div_ceil(STEP_MS).max(1);
        for _ in 0..steps {
            tokio::time::advance(Duration::from_millis(STEP_MS)).await;
            tokio::task::yield_now().await;
            self.settle();
        }
    }

    /// Run until the page shows what we are waiting for, or give up.
    pub async fn run_until(&mut self, budget_ms: u64, mut done: impl FnMut(&Self) -> bool) -> bool {
        let mut waited = 0;
        while waited < budget_ms {
            if done(self) {
                return true;
            }
            self.advance(u64::from(POLL_MS)).await;
            waited += u64::from(POLL_MS);
        }
        done(self)
    }

    /// Run `f` on the app's state, the way one of its own callbacks does,
    /// then settle what it set off.
    pub fn with_app<T>(&mut self, f: impl FnOnce(AppCtx) -> T) -> T {
        let out = self
            .dom
            .in_scope(ScopeId::APP, || f(consume_context::<AppCtx>()));
        self.settle();
        out
    }

    /// How much the renderer has been asked to do since this was last called.
    ///
    /// The unit is one mutation — a template loaded, an attribute set, a text
    /// node written. On the desktop build each of them crosses to the webview,
    /// so this is what a keystroke actually costs.
    pub fn take_work(&mut self) -> usize {
        std::mem::take(&mut self.work)
    }

    pub fn html(&self) -> String {
        dioxus_ssr::render(&self.dom)
    }

    pub fn has(&self, needle: &str) -> bool {
        self.html().contains(needle)
    }

    /// The opening tag of an element the markup names, so a test can see
    /// `disabled` / `readonly` / `autofocus` without scraping the whole page.
    pub fn input_tag(&self, id: &str) -> String {
        let html = self.html();
        let needle = format!("id=\"{id}\"");
        let start = html
            .find(&needle)
            .unwrap_or_else(|| panic!("no element with {needle}"));
        let open = html[..start]
            .rfind('<')
            .unwrap_or_else(|| panic!("{id} tag has no opening bracket"));
        let close = html[start..]
            .find('>')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("{id} tag is unclosed"));
        html[open..=close].to_string()
    }

    /// The ids of everything on the page, and a check that each is unique: two
    /// elements answering to one name would make both the page and this
    /// harness ambiguous.
    pub fn page_ids(&self) -> Vec<String> {
        let html = self.html();
        let mut seen: Vec<String> = Vec::new();
        for part in html.split("id=\"").skip(1) {
            let Some(name) = part.split('"').next() else {
                continue;
            };
            assert!(
                !seen.iter().any(|held| held == name),
                "two elements are called {name:?}"
            );
            seen.push(name.to_string());
        }
        seen
    }

    /// Every `.ch` on the page holds exactly one character.
    ///
    /// It is a fixed 21px box built for one glyph, so anything longer runs out
    /// of it and across whatever is next to it — which is how the callsign
    /// preview first shipped, four calls overlapping into mush. Checked on
    /// every screen the sweeps visit, because the mistake is easy to repeat
    /// and impossible to see from a passing test.
    pub fn character_boxes_hold_single_characters(&self) {
        let html = self.html();
        for part in html.split("class=\"ch").skip(1) {
            // `class="chars"` starts the same way and is the container.
            if !part.starts_with('"') && !part.starts_with(' ') {
                continue;
            }
            let Some(rest) = part.split_once('>') else {
                continue;
            };
            let Some(text) = rest.1.split('<').next() else {
                continue;
            };
            let text = text.trim();
            assert!(
                text.chars().count() <= 1,
                "a character box holds {text:?}, which will overlap its neighbours"
            );
        }
    }

    /// Which screen the shell is showing.
    pub fn screen(&self) -> String {
        self.html()
            .split("id=\"screen-")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or("none")
            .to_string()
    }

    /// Whether the page is showing a control with this id.
    pub fn shows(&self, id: &str) -> bool {
        self.has(&format!("id=\"{id}\""))
    }

    /// Every control on the page whose id starts with `prefix`, in the order
    /// the markup names them.
    pub fn controls(&self, prefix: &str) -> Vec<String> {
        let live = self.page_ids();
        let mut out: Vec<String> = self
            .ids
            .iter()
            .map(|(name, _)| name.clone())
            .filter(|name| name.starts_with(prefix) && live.contains(name))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn element(&self, id: &str) -> ElementId {
        self.ids
            .iter()
            .find(|(name, _)| name == id)
            .map(|(_, element)| *element)
            .unwrap_or_else(|| {
                let mut seen: Vec<&str> = self.ids.iter().map(|(name, _)| name.as_str()).collect();
                seen.sort_unstable();
                panic!("the page has no control called {id:?}; it has {seen:?}")
            })
    }

    /// Deliver an event the way a renderer does: a platform payload that the
    /// html crate turns into the typed data the listener expects. Whether
    /// the page left the browser to do what it would do next.
    fn fire(&mut self, event: &str, id: &str, data: Box<dyn std::any::Any>) -> bool {
        install_event_converter();
        let element = self.element(id);
        let payload: std::rc::Rc<dyn std::any::Any> =
            std::rc::Rc::new(dioxus::html::PlatformEventData::new(data));
        // As a browser has it: entering, leaving and focus are told to the
        // element alone, not to those around it.
        let bubbles = !matches!(event, "pointerenter" | "pointerleave" | "focus");
        let delivered = dioxus::core::Event::new(payload, bubbles);
        self.dom
            .runtime()
            .handle_event(event, delivered.clone(), element);
        self.settle();
        delivered.default_action_enabled()
    }

    /// Open every panel the page keeps behind a toggle, and keep going until
    /// nothing new appears — one disclosure may hold another.
    ///
    /// A sweep that skips this only ever sees the controls a screen happens to
    /// start with, which is how a whole card of sliders can go untested.
    pub fn open_disclosures(&mut self) {
        for _ in 0..NESTED_DISCLOSURES {
            let closed: Vec<String> = self
                .controls(DISCLOSURE)
                .into_iter()
                .filter(|id| !self.is_open(id))
                .collect();
            if closed.is_empty() {
                return;
            }
            for id in &closed {
                self.click(id);
                assert!(
                    self.is_open(id),
                    "{id:?} did not open; a disclosure has to show an `open` class \
                     while its panel is up, or nothing can tell that it is"
                );
            }
        }
        panic!("disclosures kept opening onto more disclosures");
    }

    /// Whether a disclosure is showing what it hides. The markup says so with
    /// an `open` class, the same one the stylesheet turns the chevron on.
    fn is_open(&self, id: &str) -> bool {
        self.html()
            .split(&format!("id=\"{id}\""))
            .nth(1)
            .and_then(|rest| rest.split('>').next())
            .is_some_and(|tag| tag.contains("open"))
    }

    /// Press the control with this id.
    pub fn click(&mut self, id: &str) {
        self.fire(
            "click",
            id,
            Box::new(dioxus::html::SerializedMouseData::default()),
        );
    }

    /// Click the page off every widget's own squares, or press Escape: what
    /// their listeners on the document hear.
    pub fn click_elsewhere(&mut self) {
        let heard = ELSEWHERE.with_borrow(Clone::clone);
        self.dom.in_scope(ScopeId::APP, || {
            for on_elsewhere in heard {
                on_elsewhere.call(());
            }
        });
        self.settle();
    }

    /// Press a mouse button down on the control with this id, and nothing
    /// after it: whether the browser would move the focus there, from the
    /// field being typed in, as it does unless the page stops it.
    pub fn mouse_down(&mut self, id: &str) -> bool {
        self.fire(
            "mousedown",
            id,
            Box::new(dioxus::html::SerializedMouseData::default()),
        )
    }

    /// A pointer of `kind` ("mouse", "touch") at `at` in the window, which is
    /// `in_control` inside the control it is over.
    fn pointer(
        kind: &str,
        at: (f64, f64),
        in_control: (f64, f64),
    ) -> dioxus::html::SerializedPointerData {
        let mut data = serde_json::to_value(dioxus::html::SerializedMouseData::default())
            .expect("mouse data serializes");
        let fields = data.as_object_mut().expect("mouse data is an object");
        for (name, value) in [
            ("client_x", serde_json::json!(at.0)),
            ("client_y", serde_json::json!(at.1)),
            ("offset_x", serde_json::json!(in_control.0)),
            ("offset_y", serde_json::json!(in_control.1)),
            ("pointer_id", serde_json::json!(1)),
            ("width", serde_json::json!(1.0)),
            ("height", serde_json::json!(1.0)),
            ("pressure", serde_json::json!(0.5)),
            ("tangential_pressure", serde_json::json!(0.0)),
            ("tilt_x", serde_json::json!(0)),
            ("tilt_y", serde_json::json!(0)),
            ("twist", serde_json::json!(0)),
            ("pointer_type", serde_json::json!(kind)),
            ("is_primary", serde_json::json!(true)),
        ] {
            fields.insert(name.to_string(), value);
        }
        serde_json::from_value(data).expect("pointer data deserializes")
    }

    /// Put a finger down on the control with this id, and nothing after it:
    /// a touch that slid as it tapped, which the browser sends no click for.
    pub fn touch_down(&mut self, id: &str) {
        let pointer = Self::pointer("touch", (0.0, 0.0), (0.0, 0.0));
        self.fire("pointerdown", id, Box::new(pointer));
    }

    /// Move a pointer of `kind` onto the control with this id, at `at` in the
    /// window and `in_control` inside the control.
    pub fn point_at(&mut self, id: &str, kind: &str, at: (f64, f64), in_control: (f64, f64)) {
        let pointer = Self::pointer(kind, at, in_control);
        self.fire("pointerenter", id, Box::new(pointer));
    }

    /// Move a mouse off the control with this id.
    pub fn point_away(&mut self, id: &str) {
        let pointer = Self::pointer("mouse", (0.0, 0.0), (0.0, 0.0));
        self.fire("pointerleave", id, Box::new(pointer));
    }

    /// Tap the control with this id with a finger: down, then the click the
    /// browser sends after it.
    pub fn tap(&mut self, id: &str) {
        self.touch_down(id);
        self.click(id);
    }

    /// Type into the field with this id.
    pub fn type_into(&mut self, id: &str, value: &str) {
        self.fire("input", id, Self::form(value));
    }

    /// Give the field with this id the cursor, the way a click, a tap or Tab
    /// into it does.
    pub fn focus(&mut self, id: &str) {
        self.fire(
            "focus",
            id,
            Box::new(dioxus::html::SerializedFocusData::default()),
        );
    }

    /// Commit a field, the way leaving it does.
    pub fn commit(&mut self, id: &str, value: &str) {
        self.fire("change", id, Self::form(value));
    }

    /// Choose a file called `name` in the file input with this id, the way
    /// the platform's picker hands one over. `size` is what the file says it
    /// is, which a test can make larger than what it holds.
    pub fn choose_file(&mut self, id: &str, name: &str, contents: &str, size: u64) {
        let file = dioxus::html::SerializedFileData {
            path: std::path::PathBuf::from(name),
            size,
            last_modified: 0,
            content_type: Some("text/plain".to_string()),
            contents: Some(contents.as_bytes().to_vec().into()),
        };
        let form = dioxus::html::SerializedFormData::new(
            String::new(),
            vec![dioxus::html::SerializedFormObject {
                key: "file".to_string(),
                text: None,
                file: Some(file),
            }],
        );
        self.fire("change", id, Box::new(form));
    }

    pub fn press_enter(&mut self, id: &str) {
        self.press_key(id, Key::Enter, dioxus::prelude::Code::Enter);
    }

    /// Press `key` in the element `id`: whether the page left the key to
    /// do what it would, a character typed among it, rather than keeping
    /// it.
    pub fn press_key(&mut self, id: &str, key: Key, code: dioxus::prelude::Code) -> bool {
        self.press_key_held(id, key, code, dioxus::prelude::Modifiers::empty())
    }

    /// Press `key` with `held`, Shift or Ctrl and the like, held down:
    /// whether the page left the key to do what it would.
    pub fn press_key_held(
        &mut self,
        id: &str,
        key: Key,
        code: dioxus::prelude::Code,
        held: dioxus::prelude::Modifiers,
    ) -> bool {
        self.key_down(id, key, code, held, false)
    }

    /// `key` repeating in the element `id`, as a key held down repeats:
    /// whether the page left the repeat to do what it would.
    pub fn repeat_key(&mut self, id: &str, key: Key, code: dioxus::prelude::Code) -> bool {
        self.key_down(id, key, code, dioxus::prelude::Modifiers::empty(), true)
    }

    fn key_down(
        &mut self,
        id: &str,
        key: Key,
        code: dioxus::prelude::Code,
        held: dioxus::prelude::Modifiers,
        repeating: bool,
    ) -> bool {
        self.fire(
            "keydown",
            id,
            Box::new(dioxus::html::SerializedKeyboardData::new(
                key,
                code,
                dioxus::prelude::Location::Standard,
                repeating,
                held,
                false,
            )),
        )
    }

    pub fn release_key(&mut self, id: &str, key: Key, code: dioxus::prelude::Code) {
        self.release_key_held(id, key, code, dioxus::prelude::Modifiers::empty());
    }

    /// Release `key` with `held` down.
    pub fn release_key_held(
        &mut self,
        id: &str,
        key: Key,
        code: dioxus::prelude::Code,
        held: dioxus::prelude::Modifiers,
    ) {
        self.fire(
            "keyup",
            id,
            Box::new(dioxus::html::SerializedKeyboardData::new(
                key,
                code,
                dioxus::prelude::Location::Standard,
                false,
                held,
                false,
            )),
        );
    }

    fn form(value: &str) -> Box<dyn std::any::Any> {
        Box::new(dioxus::html::SerializedFormData::new(
            value.to_string(),
            Vec::new(),
        ))
    }
}

fn install_event_converter() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        dioxus::html::set_event_converter(Box::new(dioxus::html::SerializedHtmlEventConverter));
    });
}

#[cfg(test)]
mod harness_tests {
    use super::*;

    use crate::ui::widgets::control_id;

    #[component]
    fn Counter() -> Element {
        let mut count = use_signal(|| 0);
        let mut text = use_signal(String::new);
        rsx! {
            button {
                id: control_id("btn", "bump"),
                onclick: move |_| count += 1,
                "count {count}"
            }
            if count() < 2 {
                button {
                    id: control_id("btn", "only at first"),
                    onclick: move |_| count.set(9),
                    "gone later"
                }
            }
            input {
                id: control_id("field", "text"),
                value: "{text}",
                oninput: move |e| text.set(e.value()),
                onchange: move |e| text.set(format!("committed {}", e.value())),
            }
            p { "text is {text}" }
        }
    }

    #[test]
    fn a_control_is_pressed_by_the_id_its_markup_gives_it() {
        let mut ui = Ui::new(Counter, ());
        assert!(ui.has("count 0"));
        ui.click("btn-bump");
        assert!(ui.has("count 1"));
        ui.type_into("field-text", "KM");
        assert!(ui.has("text is KM"));
        ui.commit("field-text", "KM");
        assert!(ui.has("text is committed KM"));
    }

    #[test]
    fn a_control_that_has_gone_is_no_longer_on_the_page() {
        let mut ui = Ui::new(Counter, ());
        assert!(ui.shows("btn-only-at-first"));
        assert_eq!(
            ui.controls("btn-only"),
            vec!["btn-only-at-first".to_string()]
        );
        ui.click("btn-only-at-first");
        assert!(!ui.shows("btn-only-at-first"));
        assert!(ui.controls("btn-only").is_empty());
        // The ones that stayed still answer.
        assert_eq!(
            ui.controls(""),
            vec!["btn-bump".to_string(), "field-text".to_string()]
        );
        ui.click("btn-bump");
        assert!(ui.has("count 10"));
    }

    /// An effect that wakes itself every time it runs.
    #[component]
    fn Restless() -> Element {
        let mut count = use_signal(|| 0u32);
        use_effect(move || {
            let seen = count();
            count.set(seen + 1);
        });
        rsx! { p { "{count}" } }
    }

    #[test]
    #[should_panic(expected = "never settled")]
    fn a_page_that_never_settles_fails_the_test() {
        let _ui = Ui::new(Restless, ());
    }

    #[test]
    #[should_panic(expected = "no control called")]
    fn pressing_something_that_is_not_there_says_so() {
        let mut ui = Ui::new(Counter, ());
        ui.click("nonsense");
    }
}
