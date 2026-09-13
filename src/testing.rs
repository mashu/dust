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
use crate::engine::{AppState, Screen, SessionSignals};
use crate::session_runtime::{boot_machine_session, send_command, spawn_effects};
use crate::time::POLL_MS;
use crate::ui::widgets::DISCLOSURE;

/// Clock step per pump. Smaller than one poll interval, so nothing is skipped
/// over.
const STEP_MS: u64 = 4;

/// How deep a disclosure may nest before the harness calls it a mistake.
const NESTED_DISCLOSURES: usize = 8;

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

    /// The signals a session writes into, as the app assembles them.
    pub fn signals(&self) -> SessionSignals {
        SessionSignals {
            screen: self.screen,
            runtime: self.runtime,
            result: self.result,
            auto_message: self.auto_message,
            sessions: self.sessions,
            settings: self.settings,
            toast: self.toast,
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
        let (app, signals) = (self.app.clone(), self.signals());
        let gen = self
            .in_app(|| app.takeover_audio(&settings_now))
            .expect("the fake player always opens");
        let effects = self
            .in_app(|| boot_machine_session(settings_now.clone(), &history, &app, gen, signals))
            .expect("a fresh session always boots");
        self.in_app(|| spawn_effects(effects, settings_now, app.clone(), gen, signals));
        self.pump();
    }

    /// Send one event the way a component callback does.
    pub fn send(&mut self, event: SessionEvent) {
        let (app, signals) = (self.app.clone(), self.signals());
        self.in_app(|| send_command(app, signals, event));
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
}

/// Ids and removals, as the renderer reports them.
#[derive(Default)]
struct IdLog {
    named: Vec<(String, ElementId)>,
    gone: Vec<ElementId>,
}

impl dioxus::core::WriteMutations for IdLog {
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
        if name != "id" {
            return;
        }
        if let dioxus::core::AttributeValue::Text(text) = value {
            self.named.push((text.clone(), id));
        }
    }
    fn set_node_text(&mut self, _: &str, _: ElementId) {}
    fn create_event_listener(&mut self, _: &'static str, _: ElementId) {}
    fn remove_event_listener(&mut self, _: &'static str, _: ElementId) {}
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
        (
            Self::from_dom(VirtualDom::new(crate::app::App).with_root_context(state)),
            recorder,
        )
    }

    fn from_dom(dom: VirtualDom) -> Self {
        let mut ui = Self {
            dom,
            ids: Vec::new(),
        };
        let mut log = IdLog::default();
        ui.dom.rebuild(&mut log);
        ui.apply(log);
        ui.settle();
        ui
    }

    fn apply(&mut self, log: IdLog) {
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
    /// effects that render queues, and whatever those effects dirty in turn.
    pub fn settle(&mut self) {
        for _ in 0..6 {
            self.dom.process_events();
            let mut log = IdLog::default();
            self.dom.render_immediate(&mut log);
            self.apply(log);
        }
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

    pub fn html(&self) -> String {
        dioxus_ssr::render(&self.dom)
    }

    pub fn has(&self, needle: &str) -> bool {
        self.html().contains(needle)
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
    /// html crate turns into the typed data the listener expects.
    fn fire(&mut self, event: &str, id: &str, data: Box<dyn std::any::Any>) {
        install_event_converter();
        let element = self.element(id);
        let payload = std::rc::Rc::new(dioxus::html::PlatformEventData::new(data));
        self.dom
            .runtime()
            .handle_event(event, dioxus::core::Event::new(payload, true), element);
        self.settle();
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

    /// Type into the field with this id.
    pub fn type_into(&mut self, id: &str, value: &str) {
        self.fire("input", id, Self::form(value));
    }

    /// Commit a field, the way leaving it does.
    pub fn commit(&mut self, id: &str, value: &str) {
        self.fire("change", id, Self::form(value));
    }

    pub fn focus(&mut self, id: &str) {
        self.fire(
            "focus",
            id,
            Box::new(dioxus::html::SerializedFocusData::default()),
        );
    }

    pub fn press_enter(&mut self, id: &str) {
        self.fire(
            "keydown",
            id,
            Box::new(dioxus::html::SerializedKeyboardData::new(
                Key::Enter,
                dioxus::prelude::Code::Enter,
                dioxus::prelude::Location::Standard,
                false,
                dioxus::prelude::Modifiers::empty(),
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

    #[test]
    #[should_panic(expected = "no control called")]
    fn pressing_something_that_is_not_there_says_so() {
        let mut ui = Ui::new(Counter, ());
        ui.click("nonsense");
    }
}
