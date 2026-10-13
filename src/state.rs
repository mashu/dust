//! What the app is: its state, reactive and not, and the moves every screen
//! shares.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use cw_core::{GroupSession, SessionResult, TrainingSettings};
use dioxus::prelude::*;

use crate::persist::Store;
use crate::playback::Preview;
use crate::player::{AudioError, BackendFactory, Player};
use crate::session_runtime::{AnswerOutbox, LiveSession};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Settings,
    Training,
    Results,
    Stats,
    Listen,
}

impl Screen {
    /// The screen's name in the page, which also keys its wrapper so that
    /// moving between screens mounts each afresh.
    pub fn key(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Settings => "settings",
            Self::Training => "training",
            Self::Results => "results",
            Self::Stats => "stats",
            Self::Listen => "listen",
        }
    }
}

/// What the app holds outside the reactive tree: the audio player, the
/// session being run on it, and where what it learns is kept. Cheap to
/// clone; every clone is the same app.
#[derive(Clone)]
pub struct AppState {
    pub player: Player,
    pub store: Rc<dyn Store>,
    pub session: Rc<RefCell<Option<LiveSession>>>,
}

impl AppState {
    /// The app as it ships: this build's audio, this build's store.
    pub fn new() -> Self {
        Self::with(
            Rc::new(crate::audio::default_backend),
            crate::persist::default_store(),
        )
    }

    pub fn with(make_player: BackendFactory, store: Rc<dyn Store>) -> Self {
        Self {
            player: Player::new(make_player),
            store,
            session: Rc::new(RefCell::new(None)),
        }
    }

    /// Gate a live sidetone for paddle practice.
    pub fn set_live_tone(&self, on: bool, settings: &TrainingSettings) {
        self.player.set_live_tone(on, settings);
    }

    /// Open the sidetone stream ahead of the first squeeze.
    pub fn ensure_live_sidetone(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        self.player.ensure_live_sidetone(settings)
    }
}

/// Everything a screen can read, and the moves every screen shares. Copy, so
/// each screen takes it from context and each callback carries its own.
///
/// A screen subscribes to the signals it reads and to nothing else, so the
/// shell above it redraws for the shell's own state only.
#[derive(Clone, Copy)]
pub struct AppCtx {
    pub screen: Signal<Screen>,
    pub settings: Signal<TrainingSettings>,
    pub sessions: Signal<Vec<SessionResult>>,
    /// The session in flight, as its screen shows it.
    pub runtime: Signal<Option<GroupSession>>,
    /// The session just finished, for the results screen.
    pub result: Signal<Option<SessionResult>>,
    /// What the session just finished did to the level, if anything.
    pub auto_message: Signal<Option<String>>,
    pub toasts: Toasts,
    /// Whatever is playing outside a session.
    pub preview: Signal<Preview>,
    /// The groups the stream has sent, newest last.
    pub stream_heard: Signal<Vec<crate::playback::HeardGroup>>,
    /// The open answer box's text, on its way to the session.
    pub answer: AnswerOutbox,
    app: CopyValue<AppState>,
}

impl AppCtx {
    /// A fresh app around `app`, starting from what was stored. Made in the
    /// scope that owns the app, as everything in it lives as long as that.
    pub fn new(app: AppState, settings: TrainingSettings, sessions: Vec<SessionResult>) -> Self {
        Self {
            screen: Signal::new(Screen::Home),
            settings: Signal::new(settings),
            sessions: Signal::new(sessions),
            runtime: Signal::new(None),
            result: Signal::new(None),
            auto_message: Signal::new(None),
            toasts: Toasts::new(),
            preview: Signal::new(Preview::Idle),
            stream_heard: Signal::new(Vec::new()),
            answer: AnswerOutbox::new(),
            app: CopyValue::new(app),
        }
    }

    pub fn app(self) -> AppState {
        self.app.cloned()
    }

    /// Whether a session has the app: on its screen, or about to be.
    pub fn session_running(self) -> bool {
        matches!(*self.screen.peek(), Screen::Training) || self.runtime.peek().is_some()
    }

    /// Stop whatever is playing outside a session.
    pub fn stop_audio(mut self) {
        if self.session_running() {
            return;
        }
        self.app().player.release();
        self.preview.set(Preview::Idle);
    }

    /// Leave for another screen. A session in progress holds you; otherwise
    /// whatever was sounding stops before the screen changes, wherever you
    /// are going.
    pub fn go_to(mut self, to: Screen) {
        if self.session_running() {
            return;
        }
        self.app().player.release();
        self.preview.set(Preview::Idle);
        self.stream_heard.set(Vec::new());
        self.screen.set(to);
    }
}

/// How long a message stays up.
pub const TOAST_MS: u32 = 4_000;

/// A message at the foot of the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    /// Which raise this was. The same words raised twice are two toasts,
    /// each up for its own full time.
    serial: u64,
}

/// Where messages are raised and shown: one at a time, the newest in place
/// of whatever was up.
#[derive(Clone, Copy, PartialEq)]
pub struct Toasts(Signal<Option<Toast>>);

impl Toasts {
    /// Messages for the scope this is made in.
    pub fn new() -> Self {
        Self(Signal::new(None))
    }

    pub fn show(self, text: impl Into<String>) {
        static RAISED: AtomicU64 = AtomicU64::new(0);
        let mut shown = self.0;
        shown.set(Some(Toast {
            text: text.into(),
            serial: RAISED.fetch_add(1, Ordering::Relaxed),
        }));
    }

    /// The message up now. Read from a component, it subscribes it.
    pub fn current(self) -> Option<Toast> {
        self.0.cloned()
    }

    /// Take `toast` down, unless a newer one has taken its place.
    pub fn dismiss(self, toast: &Toast) {
        let mut shown = self.0;
        if shown.peek().as_ref() == Some(toast) {
            shown.set(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Screen;

    #[test]
    fn every_screen_has_its_own_key() {
        let keys: Vec<&str> = [
            Screen::Home,
            Screen::Settings,
            Screen::Training,
            Screen::Results,
            Screen::Stats,
            Screen::Listen,
        ]
        .into_iter()
        .map(Screen::key)
        .collect();
        // The key remounts the screen wrapper, so it has to differ per screen.
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len());
    }
}
