//! A whole app runtime, driven by hand.
//!
//! The session runtime is asynchronous by nature — sends, gaps, timeouts and
//! auto-confirms all run as spawned tasks — so the only honest way to test it
//! is to run it. This harness gives those tasks a real Dioxus runtime to live
//! in, a fake player in place of a sound card, and a paused clock that is
//! advanced a tick at a time, so a whole training session runs in milliseconds
//! and lands on exactly the same instants every time.

use std::rc::Rc;
use std::time::Duration;

use cw_core::{GroupSession, SessionEvent, SessionResult, TrainingSettings};
use dioxus::core::{ElementId, NoOpMutations};
use dioxus::prelude::*;

use crate::audio::fake::{Behaviour, Call, Recorder};
use crate::engine::{AppState, Screen};
use crate::session_runtime::{boot_machine_session, send_command, spawn_effects};
use crate::time::POLL_MS;

/// Clock step per pump. Smaller than one poll interval, so nothing is skipped
/// over.
const STEP_MS: u64 = 4;

pub struct Harness {
    dom: VirtualDom,
    pub app: AppState,
    pub recorder: Rc<Recorder>,
    pub settings: Signal<TrainingSettings>,
    pub sessions: Signal<Vec<SessionResult>>,
    pub screen: Signal<Screen>,
    pub runtime: Signal<Option<GroupSession>>,
    pub result: Signal<Option<SessionResult>>,
    pub auto_message: Signal<Option<String>>,
    pub toast: Signal<Option<String>>,
}

/// One finished session, the way the app stores it.
pub fn stored_session(date: &str, sent: &[(&str, &str)]) -> SessionResult {
    let settings = test_settings();
    let mut session = GroupSession::new(1, 0, sent.len().max(1), settings.clone());
    for (index, (text, answer)) in sent.iter().enumerate() {
        session.set_group(index, (*text).to_string());
        session.begin_group(index, 1_000 * index as u64);
        session.confirm(index, (*answer).to_string(), 1_000 * index as u64 + 500);
    }
    cw_core::build_session_result(&session, &settings, 1_000, date.to_string())
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
    settings.playback.lock_input_during_group_playback = true;
    settings.playback.group_repeat_min = 1;
    settings.playback.group_repeat_max = 1;
    settings.playback.link_group_repeat = true;
    settings.auto_level.auto_adjust_level = false;
    settings.band.qrn_enabled = false;
    settings.band.qrm_enabled = false;
    settings.band.qsb_enabled = false;
    settings
}

impl Harness {
    pub fn new() -> Self {
        Self::with_settings(test_settings())
    }

    pub fn with_settings(settings: TrainingSettings) -> Self {
        crate::persist::memory::reset();
        let dom = VirtualDom::new(|| rsx! {});
        let recorder = Recorder::new();
        let app = AppState::with_backend(recorder.factory());
        let (settings, sessions, screen, runtime, result, auto_message, toast) =
            dom.in_scope(ScopeId::ROOT, || {
                (
                    Signal::new(settings),
                    Signal::new(Vec::new()),
                    Signal::new(Screen::Home),
                    Signal::new(None),
                    Signal::new(None),
                    Signal::new(None),
                    Signal::new(None),
                )
            });
        Self {
            dom,
            app,
            recorder,
            settings,
            sessions,
            screen,
            runtime,
            result,
            auto_message,
            toast,
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

    /// Start a session the way the Practice screen's button does.
    pub fn start_training(&mut self) {
        let settings_now = self.settings.peek().clone().clamp();
        let history = self.sessions.peek().clone();
        let (app, runtime, screen) = (self.app.clone(), self.runtime, self.screen);
        let gen = self
            .in_app(|| app.takeover_audio(&settings_now))
            .expect("the fake player always opens");
        let effects = self
            .in_app(|| {
                boot_machine_session(settings_now.clone(), &history, &app, gen, runtime, screen)
            })
            .expect("a fresh session always boots");
        self.in_app(|| {
            spawn_effects(
                effects,
                settings_now,
                app.clone(),
                gen,
                runtime,
                screen,
                self.result,
                self.auto_message,
                self.sessions,
                self.settings,
                self.toast,
            )
        });
        self.pump();
    }

    /// Send one event the way a component callback does.
    pub fn send(&mut self, event: SessionEvent) {
        let app = self.app.clone();
        self.in_app(|| {
            send_command(
                app,
                self.runtime,
                self.screen,
                self.result,
                self.auto_message,
                self.sessions,
                self.settings,
                self.toast,
                event,
            )
        });
        self.pump();
    }

    pub fn type_answer(&mut self, index: usize, text: &str) {
        self.send(SessionEvent::Input {
            index,
            text: text.to_string(),
        });
    }

    /// What the trainer sent for `index`, as the user would have heard it.
    pub fn sent(&self, index: usize) -> String {
        self.runtime
            .peek()
            .as_ref()
            .and_then(|session| session.group(index).map(|g| g.sent().to_string()))
            .unwrap_or_default()
    }

    pub fn phase(&self) -> Option<cw_core::SessionPhase> {
        self.app.machine.borrow().as_ref().map(|m| m.phase())
    }

    pub fn screen(&self) -> Screen {
        *self.screen.peek()
    }

    pub fn toast(&self) -> Option<String> {
        self.toast.peek().clone()
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
            if let Some(index) = self.awaiting_answer() {
                if !answered.get(index).copied().unwrap_or(true) {
                    answered[index] = true;
                    let text = self.sent(index);
                    self.type_answer(index, &text);
                }
            }
            self.advance(u64::from(POLL_MS)).await;
            waited += u64::from(POLL_MS);
        }
    }

    fn sent_count(&self) -> usize {
        self.runtime
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

/// Run a test body on a paused clock.
pub fn run<F: std::future::Future<Output = ()>>(body: impl FnOnce() -> F) {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("test runtime")
        .block_on(body());
}

/// A component under test, with its event listeners addressable.
///
/// Rendering alone proves a screen draws; this also presses its buttons, so the
/// callbacks behind them are exercised rather than merely constructed.
pub struct Ui {
    dom: VirtualDom,
    listeners: Vec<(String, ElementId)>,
}

#[derive(Default)]
struct ListenerLog {
    added: Vec<(String, ElementId)>,
    removed: Vec<(String, ElementId)>,
    /// Elements that went away. Dioxus does not report the listeners on a
    /// removed subtree one by one, so the ids have to be forgotten wholesale.
    gone: Vec<ElementId>,
}

impl dioxus::core::WriteMutations for ListenerLog {
    fn append_children(&mut self, _: ElementId, _: usize) {}
    fn assign_node_id(&mut self, _: &'static [u8], _: ElementId) {}
    fn create_placeholder(&mut self, _: ElementId) {}
    fn create_text_node(&mut self, _: &str, _: ElementId) {}
    fn load_template(&mut self, _: dioxus::core::Template, _: usize, _: ElementId) {}
    fn replace_node_with(&mut self, id: ElementId, _: usize) {
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
        let _ = (name, value, id);
    }
    fn set_node_text(&mut self, _: &str, _: ElementId) {}
    fn create_event_listener(&mut self, name: &'static str, id: ElementId) {
        self.added.push((name.to_string(), id));
    }
    fn remove_event_listener(&mut self, name: &'static str, id: ElementId) {
        self.removed.push((name.to_string(), id));
    }
    fn remove_node(&mut self, id: ElementId) {
        self.gone.push(id);
    }
    fn push_root(&mut self, _: ElementId) {}
}

impl Ui {
    pub fn new<P: Clone + 'static, M: 'static>(
        component: impl dioxus::core::ComponentFunction<P, M>,
        props: P,
    ) -> Self {
        Self::from_dom(VirtualDom::new_with_props(component, props))
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
        crate::persist::memory::reset();
        crate::persist::save_settings(&settings);
        crate::persist::save_sessions(sessions);
        let recorder = Recorder::new();
        let state = AppState::with_backend(recorder.factory());
        let dom = VirtualDom::new(crate::app::App).with_root_context(state);
        (Self::from_dom(dom), recorder)
    }

    /// Move the clock forward, settling the app as it goes.
    pub async fn advance(&mut self, ms: u64) {
        let steps = ms.div_ceil(STEP_MS).max(1);
        for _ in 0..steps {
            tokio::time::advance(Duration::from_millis(STEP_MS)).await;
            tokio::task::yield_now().await;
            self.settle();
        }
    }

    /// Run until the page says what we are waiting for, or give up.
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

    pub fn has(&self, needle: &str) -> bool {
        self.html().contains(needle)
    }

    fn from_dom(dom: VirtualDom) -> Self {
        let mut ui = Self {
            dom,
            listeners: Vec::new(),
        };
        let mut log = ListenerLog::default();
        ui.dom.rebuild(&mut log);
        ui.apply(log);
        ui
    }

    fn apply(&mut self, log: ListenerLog) {
        for entry in log.removed {
            self.listeners.retain(|held| *held != entry);
        }
        for id in log.gone {
            self.listeners.retain(|(_, held)| *held != id);
        }
        // An element id is only ever live once, so a new listener on an id
        // retires whatever was recorded for it before.
        for entry in log.added {
            self.listeners
                .retain(|(name, id)| !(*id == entry.1 && *name == entry.0));
            self.listeners.push(entry);
        }
    }

    /// Settle everything the last event set off: the render it dirtied, the
    /// effects that render queues, and whatever those effects dirty in turn.
    pub fn settle(&mut self) {
        for _ in 0..6 {
            self.dom.process_events();
            let mut log = ListenerLog::default();
            self.dom.render_immediate(&mut log);
            self.apply(log);
        }
    }

    pub fn html(&self) -> String {
        dioxus_ssr::render(&self.dom)
    }

    pub fn count(&self, name: &str) -> usize {
        self.listeners
            .iter()
            .filter(|(held, _)| held == name)
            .count()
    }

    fn nth(&self, name: &str, index: usize) -> ElementId {
        self.listeners
            .iter()
            .filter(|(held, _)| held == name)
            .map(|(_, id)| *id)
            .nth(index)
            .unwrap_or_else(|| {
                panic!(
                    "no {name} listener number {index}; there are {}",
                    self.count(name)
                )
            })
    }

    /// Deliver an event the way a renderer does: a platform payload that the
    /// html crate converts into the typed data the listener expects.
    fn fire(&mut self, name: &str, index: usize, data: Box<dyn std::any::Any>) {
        let id = self.nth(name, index);
        self.fire_at(name, id, data);
    }

    fn fire_at(&mut self, name: &str, id: ElementId, data: Box<dyn std::any::Any>) {
        install_event_converter();
        let payload = std::rc::Rc::new(dioxus::html::PlatformEventData::new(data));
        self.dom
            .runtime()
            .handle_event(name, dioxus::core::Event::new(payload, true), id);
        self.settle();
    }

    pub fn click(&mut self, index: usize) {
        self.fire(
            "click",
            index,
            Box::new(dioxus::html::SerializedMouseData::default()),
        );
    }

    pub fn input(&mut self, index: usize, value: &str) {
        self.fire(
            "input",
            index,
            Box::new(dioxus::html::SerializedFormData::new(
                value.to_string(),
                Vec::new(),
            )),
        );
    }

    pub fn change(&mut self, index: usize, value: &str) {
        self.fire(
            "change",
            index,
            Box::new(dioxus::html::SerializedFormData::new(
                value.to_string(),
                Vec::new(),
            )),
        );
    }

    pub fn focus(&mut self, index: usize) {
        self.fire(
            "focus",
            index,
            Box::new(dioxus::html::SerializedFocusData::default()),
        );
    }

    pub fn keydown(&mut self, index: usize, key: Key) {
        self.fire(
            "keydown",
            index,
            Box::new(dioxus::html::SerializedKeyboardData::new(
                key,
                dioxus::prelude::Code::Enter,
                dioxus::prelude::Location::Standard,
                false,
                dioxus::prelude::Modifiers::empty(),
                false,
            )),
        );
    }
}

fn install_event_converter() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        dioxus::html::set_event_converter(Box::new(dioxus::html::SerializedHtmlEventConverter));
    });
}
