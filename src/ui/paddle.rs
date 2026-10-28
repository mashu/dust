//! The paddle keyer wired to `[` / `]` — USB HID paddles included.
//!
//! The app has one keyer, so a paddle works on every screen and its sidetone
//! does not wait on a training session to have opened the player. While a
//! training group's answer box is open the keyer types into it, and during
//! a contest run into the field with the cursor, where a word space keyed is
//! Space; on Settings what it decodes shows on the practice pad. Its timing
//! is cw-core's [`Keyer`]; this module gives it keys, a clock and a sound.

use std::rc::Rc;

use cw_core::{
    KeyedLetter, Keyer, KeyerMode, KeyerOutput, MonoMs, Paddle, ToneEdge, TrainingSettings,
    paddle_from_bracket,
};
use dioxus::prelude::*;

use crate::page_hidden::{PADDLES_HIDDEN, use_page_hidden};
use crate::state::{AppState, Screen, in_root};
use crate::time::mono_ms;
#[cfg(not(all(feature = "desktop", not(test))))]
use crate::time::sleep_ms;
use crate::ui::widgets::control_id;

/// How many decoded characters the practice pad keeps: about a line's worth.
const HEARD_KEEP: usize = 32;

/// Whether the key events that reach the page's root are what key the
/// paddles. The desktop build hears them from its window instead, so a USB
/// keyer works without the webview's focus and without waiting on its IPC,
/// and the web and mobile builds from the whole document, so they work with
/// nothing focused at all: a HID paddle on a phone keys with nothing tapped
/// first. Tests drive the root.
const ROOT_KEYS_THE_PADDLES: bool = cfg!(test);

/// Which paddle contacts are closed, as the desktop build's raw key events
/// leave them. tao on Linux never marks a held key's repeated presses as
/// repeats, so a press of a contact already closed is dropped; GTK asks X11
/// for detectable auto-repeat, and Wayland repeats in the client, so a held
/// key sends one release, at its real key-up, which is taken at once. Only the
/// desktop build listens to raw key events, so only it — and the tests — need
/// one.
#[cfg(any(feature = "desktop", test))]
#[derive(Clone, Copy, Debug, Default)]
struct Contacts {
    dit: bool,
    dah: bool,
}

#[cfg(any(feature = "desktop", test))]
impl Contacts {
    fn slot(&mut self, paddle: Paddle) -> &mut bool {
        match paddle {
            Paddle::Dit => &mut self.dit,
            Paddle::Dah => &mut self.dah,
        }
    }

    /// Whether a press closed the contact, rather than repeated one held.
    fn press(&mut self, paddle: Paddle) -> bool {
        !std::mem::replace(self.slot(paddle), true)
    }

    /// Whether a release opened the contact, rather than one already open.
    fn release(&mut self, paddle: Paddle) -> bool {
        std::mem::replace(self.slot(paddle), false)
    }

    /// Every contact open, as when the window lost the focus and the key
    /// releases with it.
    fn open_all(&mut self) {
        *self = Self::default();
    }
}

/// Wait `ms` on the keyer's clock.
async fn sleep_keyer(ms: u64) {
    let ms = u32::try_from(ms).unwrap_or(u32::MAX).max(1);
    // The UI runtime shares a thread with WebKit. A timer there fires late, so
    // a dah is cut off before it has finished sounding. Sleep on a worker.
    #[cfg(all(feature = "desktop", not(test)))]
    {
        let _ = tokio::task::spawn_blocking(move || {
            std::thread::sleep(std::time::Duration::from_millis(u64::from(ms)));
        })
        .await;
    }
    #[cfg(not(all(feature = "desktop", not(test))))]
    {
        sleep_ms(ms).await;
    }
}

/// The paddle a key is, if it is one: `[` or `]`, by the character it types
/// or else by where it sits on the keyboard — and only with no Ctrl, Alt or
/// Meta held, which make it a shortcut. `swap` turns the paddles round.
fn paddle_for_key(key: &str, code: &str, shortcut: bool, swap: bool) -> Option<Paddle> {
    if shortcut {
        return None;
    }
    let bracket = match key {
        "[" => '[',
        "]" => ']',
        _ => match code {
            "BracketLeft" => '[',
            "BracketRight" => ']',
            _ => return None,
        },
    };
    paddle_from_bracket(bracket, swap)
}

/// What a screen taking paddled letters lends the keyer: whether it is
/// taking them at this moment, where they go, and for a screen of fields,
/// what a word space keyed does there.
#[derive(Clone, Copy)]
struct AnswerBox {
    open: bool,
    letter: Callback<BoxLetter>,
    space: Option<Callback>,
}

/// A letter for a box, and when it was keyed: the first contact of its
/// letter, on the app's clock. None when it was typed for you, which is
/// never timed.
type BoxLetter = (char, Option<MonoMs>);

/// The app's paddle keyer, as the shell and the screens reach it.
///
/// Every part is a handle, so this is `Copy` and travels by context.
#[derive(Clone, Copy)]
pub struct PaddleKeys {
    keyer: CopyValue<Keyer>,
    /// The wake the clock is waiting for. A timer that finds another one here
    /// has been superseded and does nothing.
    armed: CopyValue<Option<u64>>,
    answer: CopyValue<Option<AnswerBox>>,
    app: CopyValue<Rc<AppState>>,
    settings: Signal<TrainingSettings>,
    screen: Signal<Screen>,
    /// What the practice pad shows: the last few letters keyed on Settings.
    heard: Signal<String>,
    /// Why the sidetone cannot sound, once a try has found out.
    sidetone_error: Signal<Option<String>>,
}

/// Start the app's keyer, and hand it to every screen below.
pub fn use_paddle_keys(
    settings: Signal<TrainingSettings>,
    screen: Signal<Screen>,
    app: Rc<AppState>,
) -> PaddleKeys {
    // In the root scope, where the keyer's timer runs.
    let mut keys = use_context_provider(|| {
        in_root(|| {
            let playback = &settings.peek().playback;
            PaddleKeys {
                keyer: CopyValue::new(Keyer::new(playback.keyer_mode, playback.keyer_wpm)),
                armed: CopyValue::new(None),
                answer: CopyValue::new(None),
                app: CopyValue::new(app),
                settings,
                screen,
                heard: Signal::new(String::new()),
                sidetone_error: Signal::new(None),
            }
        })
    });

    let mode = use_memo(move || settings.read().playback.keyer_mode);
    use_effect(move || {
        let out = keys.keyer.write().set_mode(mode(), mono_ms());
        keys.act(out);
    });

    // A new screen starts the keyer clean: a letter half keyed on one screen
    // is not the next one's. Settings is where the paddles are tried, so
    // arriving there opens the sidetone ahead of the first squeeze — the
    // first element is then one audio buffer late, not a stream start — and
    // finds out whether it can sound at all, and the output is kept awake
    // there. Anywhere else it sleeps once quiet, and the first squeeze
    // wakes it.
    use_effect(move || {
        let on_settings = screen() == Screen::Settings;
        keys.stop();
        let mut heard = keys.heard;
        if !heard.peek().is_empty() {
            heard.set(String::new());
        }
        keys.app.read().keep_output_ready(on_settings);
        if on_settings {
            keys.open_sidetone();
        }
    });

    // The key releases go with the page: open the paddles, so none stays
    // keyed, its sidetone sounding, until it is touched again.
    use_page_hidden(PADDLES_HIDDEN, move || keys.release_all());
    listen_native_paddles(keys);
    listen_document_paddles(keys);
    listen_page_paddles(keys);
    keys
}

impl PaddleKeys {
    /// The page root's keydown. `[` and `]` are paddles, never text — on the
    /// desktop the window keys them, but the answer box must not type them.
    pub fn keydown(self, e: Event<KeyboardData>) {
        let Some(paddle) = self.paddle_for(&e, true) else {
            return;
        };
        e.prevent_default();
        if ROOT_KEYS_THE_PADDLES && !e.is_auto_repeating() {
            self.press(paddle);
        }
    }

    /// The page root's keyup.
    pub fn keyup(self, e: Event<KeyboardData>) {
        let Some(paddle) = self.paddle_for(&e, false) else {
            return;
        };
        e.prevent_default();
        if ROOT_KEYS_THE_PADDLES {
            self.release(paddle);
        }
    }

    /// The paddle a key is, if it is one, pressed or else released. Ctrl,
    /// Alt or Meta held make a press a shortcut, but never a release: a
    /// paddle held when one of them went down still has to open.
    fn paddle_for(self, e: &Event<KeyboardData>, press: bool) -> Option<Paddle> {
        let key = match e.key() {
            Key::Character(text) => text,
            _ => String::new(),
        };
        let mods = e.modifiers();
        let shortcut = press && (mods.ctrl() || mods.alt() || mods.meta());
        let swap = self.settings.peek().playback.paddle_swap;
        paddle_for_key(&key, &e.code().to_string(), shortcut, swap)
    }

    /// A paddle closed.
    fn press(mut self, paddle: Paddle) {
        if !self.taking_paddles() {
            return;
        }
        // A squeeze is the moment to find out whether the sidetone can sound:
        // a device plugged in since the last one is picked up here, and one
        // that is still missing costs one look per squeeze, not one per
        // element.
        if !self.keyer.peek().sounding() {
            self.open_sidetone();
        }
        let wpm = self.settings.peek().playback.keyer_wpm;
        let out = {
            let mut keyer = self.keyer.write();
            keyer.set_wpm(wpm);
            keyer.press(paddle, mono_ms())
        };
        self.act(out);
    }

    /// A paddle opened. Always heard, whatever is taking paddles now: a
    /// release lost is a paddle stuck down.
    fn release(self, paddle: Paddle) {
        self.release_at(paddle, mono_ms());
    }

    /// A paddle opened at `at`: both of them at the one moment, when every
    /// contact opens together.
    fn release_at(mut self, paddle: Paddle, at: u64) {
        let out = self.keyer.write().release(paddle, at);
        self.act(out);
    }

    /// Open both paddles: the window lost the focus, or the page went out
    /// of sight, and the key releases with it. The element sounding
    /// finishes, so the letter it ends is still heard.
    pub fn release_all(self) {
        let now = mono_ms();
        for paddle in [Paddle::Dit, Paddle::Dah] {
            self.release_at(paddle, now);
        }
    }

    /// Whether a letter is being keyed and has not ended yet.
    pub fn letter_open(self) -> bool {
        self.keyer.peek().letter_open()
    }

    /// End the letter keyed into the answer box now, if it is keyed in full,
    /// rather than once its letter space has run out.
    pub fn finish_letter(mut self) {
        let out = self.keyer.write().finish_letter();
        self.act(out);
    }

    /// Drop whatever is half keyed and fall silent: a new group, a new screen.
    pub fn stop(mut self) {
        let out = self.keyer.write().stop(mono_ms());
        self.act(out);
    }

    /// Key into an answer box rather than the practice pad, for as long as
    /// the calling component is mounted, and only while `open`. A group has
    /// no spaces, so a word space keyed there does nothing.
    pub fn use_answer_box(
        self,
        open: bool,
        mut letter: impl FnMut(char, Option<MonoMs>) + 'static,
    ) {
        let letter = use_callback(move |(ch, at): BoxLetter| letter(ch, at));
        self.use_lent(open, letter, None);
    }

    /// The same for a screen of fields, the run's Call and Exchange, where a
    /// word space keyed — the paddles quiet for six dits after a letter's
    /// last element — does what Space does: `space`.
    pub fn use_fields(
        self,
        open: bool,
        mut letter: impl FnMut(char, Option<MonoMs>) + 'static,
        mut space: impl FnMut() + 'static,
    ) {
        let letter = use_callback(move |(ch, at): BoxLetter| letter(ch, at));
        let space = use_callback(move |()| space());
        self.use_lent(open, letter, Some(space));
    }

    /// Lend the keyer the calling screen's box, while it is mounted.
    fn use_lent(self, open: bool, letter: Callback<BoxLetter>, space: Option<Callback>) {
        let mut answer = self.answer;
        use_hook(move || {
            answer.set(Some(AnswerBox {
                open,
                letter,
                space,
            }));
        });
        // Whether it is taking letters now, said into the box itself rather
        // than a value of this screen's: the keyer reads it from its own
        // timer, and from the shell, neither of which is under this screen.
        if let Some(held) = answer.write().as_mut()
            && held.letter == letter
        {
            held.open = open;
        }
        use_drop(move || {
            // Only its own: a box that has already taken over stays.
            if answer.peek().is_some_and(|held| held.letter == letter) {
                answer.set(None);
            }
            self.stop();
        });
    }

    /// Put `letter` in the open answer box, the way a keyed letter goes in,
    /// but untimed: how the guided setup's Show me copies a group. Whether a
    /// box took it.
    pub fn type_letter(self, letter: char) -> bool {
        let Some(answer) = *self.answer.peek() else {
            return false;
        };
        if !answer.open {
            return false;
        }
        answer.letter.call((letter, None));
        true
    }

    fn taking_paddles(self) -> bool {
        match *self.answer.peek() {
            Some(answer) => answer.open,
            // Training keys into its answer box or not at all.
            None => *self.screen.peek() != Screen::Training,
        }
    }

    /// Try the sidetone, and keep what the try found out for the pad to show.
    fn open_sidetone(self) {
        let found = self
            .app
            .read()
            .ensure_live_sidetone(&self.settings.peek())
            .err()
            .map(|err| err.to_string());
        let mut error = self.sidetone_error;
        if *error.peek() != found {
            error.set(found);
        }
    }

    fn act(self, out: KeyerOutput) {
        if let Some(edge) = out.tone {
            self.sound(edge);
        }
        if let Some(keyed) = out.letter {
            self.deliver(keyed);
        }
        if out.space {
            self.deliver_space();
        }
        self.arm(out.wake_at);
    }

    fn sound(self, edge: ToneEdge) {
        // Only when the sidetone was there at the start of the squeeze. The
        // player opens itself on demand, so asking a missing device for every
        // element of it would be a device search twice an element. Nothing
        // can be sounding to turn off: the try comes before the tone goes on.
        if self.sidetone_error.peek().is_some() {
            return;
        }
        self.app.read().set_live_tone(edge, &self.settings.peek());
    }

    fn deliver(mut self, keyed: KeyedLetter) {
        if let Some(answer) = *self.answer.peek() {
            if answer.open {
                answer
                    .letter
                    .call((keyed.letter, Some(MonoMs(keyed.began))));
            }
            return;
        }
        if *self.screen.peek() != Screen::Settings {
            return;
        }
        let mut heard = self.heard.write();
        heard.push(keyed.letter);
        let excess = heard.chars().count().saturating_sub(HEARD_KEEP);
        if let Some((cut, _)) = heard.char_indices().nth(excess) {
            heard.drain(..cut);
        }
    }

    /// A word space keyed, for a screen of fields taking letters now. An
    /// answer box and the practice pad have no use for one.
    fn deliver_space(self) {
        if let Some(AnswerBox {
            open,
            space: Some(space),
            ..
        }) = *self.answer.peek()
            && open
        {
            space.call(());
        }
    }

    /// Wake the keyer at `wake`, or not at all.
    fn arm(mut self, wake: Option<u64>) {
        let mut armed = self.armed;
        if *armed.peek() == wake {
            return;
        }
        armed.set(wake);
        let Some(at) = wake else {
            return;
        };
        // In the root scope: the keyer outlives whichever screen pressed it.
        dioxus::core::spawn_forever(async move {
            sleep_keyer(at.saturating_sub(mono_ms())).await;
            if *armed.peek() != Some(at) {
                return;
            }
            armed.set(None);
            let out = self.keyer.write().tick(mono_ms());
            self.act(out);
        });
    }
}

#[cfg(all(feature = "desktop", not(test)))]
fn listen_native_paddles(keys: PaddleKeys) {
    use std::cell::RefCell;

    let contacts = use_hook(|| Rc::new(RefCell::new(Contacts::default())));
    let unfocused = Rc::clone(&contacts);
    let dispatch = use_callback(move |(paddle, pressed): (Paddle, bool)| {
        if pressed {
            if contacts.borrow_mut().press(paddle) {
                keys.press(paddle);
            }
        } else if contacts.borrow_mut().release(paddle) {
            keys.release(paddle);
        }
    });
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{ElementState, Event, WindowEvent};
        use dioxus::desktop::tao::keyboard::{Key, KeyCode};
        // Linux and macOS send no key release once the window has lost the
        // focus, so the paddles open with it.
        if let Event::WindowEvent {
            event: WindowEvent::Focused(false),
            ..
        } = event
        {
            unfocused.borrow_mut().open_all();
            keys.release_all();
            return;
        }
        let Event::WindowEvent {
            event: WindowEvent::KeyboardInput { event: key, .. },
            ..
        } = event
        else {
            return;
        };
        if key.repeat {
            return;
        }
        let ch = match key.physical_key {
            KeyCode::BracketLeft => '[',
            KeyCode::BracketRight => ']',
            _ => match &key.logical_key {
                Key::Character(s) if *s == "[" => '[',
                Key::Character(s) if *s == "]" => ']',
                _ => return,
            },
        };
        let swap = keys.settings.peek().playback.paddle_swap;
        let Some(paddle) = paddle_from_bracket(ch, swap) else {
            return;
        };
        match key.state {
            ElementState::Pressed => dispatch.call((paddle, true)),
            ElementState::Released => dispatch.call((paddle, false)),
            _ => {}
        }
    });
}

#[cfg(not(all(feature = "desktop", not(test))))]
fn listen_native_paddles(_keys: PaddleKeys) {}

/// The browser delivers keys to whatever has focus, and with nothing focused
/// — a page just loaded, a click on the background — that is the document
/// body, outside the app's root. Listening on the document hears them all.
#[cfg(feature = "web")]
fn listen_document_paddles(keys: PaddleKeys) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::KeyboardEvent;

    type Listener = Closure<dyn FnMut(KeyboardEvent)>;
    let listeners = use_hook(|| {
        let window = web_sys::window()?;
        let document = window.document()?;
        // Ctrl, Alt or Meta make a press a shortcut, never a release.
        let paddle_for = move |e: &KeyboardEvent, press: bool| {
            let shortcut = press && (e.ctrl_key() || e.alt_key() || e.meta_key());
            let swap = keys.settings.peek().playback.paddle_swap;
            paddle_for_key(&e.key(), &e.code(), shortcut, swap)
        };
        let down: Listener = Closure::new(move |e: KeyboardEvent| {
            if let Some(paddle) = paddle_for(&e, true) {
                e.prevent_default();
                if !e.repeat() {
                    keys.press(paddle);
                }
            }
        });
        let up: Listener = Closure::new(move |e: KeyboardEvent| {
            if let Some(paddle) = paddle_for(&e, false) {
                e.prevent_default();
                keys.release(paddle);
            } else if e.key() == "Meta" {
                // A Mac's browser sends no key release for a key let go
                // while Command was down.
                keys.release_all();
            }
        });
        // Nor does any browser once the window has lost the focus.
        let blur: Closure<dyn FnMut(web_sys::Event)> =
            Closure::new(move |_: web_sys::Event| keys.release_all());
        let listen = |name: &str, listener: &Listener| {
            let _ =
                document.add_event_listener_with_callback(name, listener.as_ref().unchecked_ref());
        };
        listen("keydown", &down);
        listen("keyup", &up);
        let _ = window.add_event_listener_with_callback("blur", blur.as_ref().unchecked_ref());
        Some(Rc::new((window, document, down, up, blur)))
    });
    use_drop(move || {
        if let Some(held) = listeners.as_deref() {
            let (window, document, down, up, blur) = held;
            let _ = document
                .remove_event_listener_with_callback("keydown", down.as_ref().unchecked_ref());
            let _ =
                document.remove_event_listener_with_callback("keyup", up.as_ref().unchecked_ref());
            let _ =
                window.remove_event_listener_with_callback("blur", blur.as_ref().unchecked_ref());
        }
    });
}

#[cfg(not(feature = "web"))]
fn listen_document_paddles(_keys: PaddleKeys) {}

/// The mobile builds' webview delivers keys as a browser does, to whatever
/// has focus, and a tapped button takes none on iOS. So, as the web build's
/// ([`listen_document_paddles`]), they hear the paddles from the whole
/// document: the page's script keeps a bracket from the page and hands it
/// over, a press with Ctrl, Alt or Meta down being a shortcut, a repeat being
/// nothing, and a release always heard; and the window losing the focus, or
/// Command let go, opens both.
#[cfg(any(all(not(feature = "desktop"), not(feature = "web")), test))]
const PAGE_PADDLES_SCRIPT: &str = r#"if (window.__dustPaddleKeys) {
    window.__dustPaddleKeys();
}
const bracket = (e) => e.key === "[" || e.key === "]" || e.code === "BracketLeft" || e.code === "BracketRight";
const shortcut = (e) => e.ctrlKey || e.altKey || e.metaKey;
const down = (e) => {
    if (bracket(e) && !shortcut(e)) {
        e.preventDefault();
        if (!e.repeat) {
            dioxus.send(["down", e.key, e.code]);
        }
    }
};
const up = (e) => {
    if (bracket(e)) {
        e.preventDefault();
        dioxus.send(["up", e.key, e.code]);
    } else if (e.key === "Meta") {
        dioxus.send(["open", "", ""]);
    }
};
const blur = () => dioxus.send(["open", "", ""]);
document.addEventListener("keydown", down);
document.addEventListener("keyup", up);
window.addEventListener("blur", blur);
window.__dustPaddleKeys = () => {
    document.removeEventListener("keydown", down);
    document.removeEventListener("keyup", up);
    window.removeEventListener("blur", blur);
    window.__dustPaddleKeys = null;
};"#;

/// The script that takes the page's paddle listener off again.
#[cfg(any(all(not(feature = "desktop"), not(feature = "web")), test))]
const STOP_PAGE_PADDLES: &str = r#"if (window.__dustPaddleKeys) {
    window.__dustPaddleKeys();
}"#;

/// What the page's word about a key does to the paddles: one closes or
/// opens, or both open.
#[cfg(any(all(not(feature = "desktop"), not(feature = "web")), test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageKey {
    Press(Paddle),
    Release(Paddle),
    OpenBoth,
}

/// The page's word, `kind` of a key `key` at `code`, as the paddles take it:
/// none for a key that is no paddle.
#[cfg(any(all(not(feature = "desktop"), not(feature = "web")), test))]
fn page_key(kind: &str, key: &str, code: &str, swap: bool) -> Option<PageKey> {
    match (kind, paddle_for_key(key, code, false, swap)) {
        ("down", Some(paddle)) => Some(PageKey::Press(paddle)),
        ("up", Some(paddle)) => Some(PageKey::Release(paddle)),
        ("open", _) => Some(PageKey::OpenBoth),
        _ => None,
    }
}

#[cfg(all(not(feature = "desktop"), not(feature = "web"), not(test)))]
fn listen_page_paddles(keys: PaddleKeys) {
    use_hook(move || {
        spawn(async move {
            let mut page = document::eval(PAGE_PADDLES_SCRIPT);
            while let Ok((kind, key, code)) = page.recv::<(String, String, String)>().await {
                let swap = keys.settings.peek().playback.paddle_swap;
                match page_key(&kind, &key, &code, swap) {
                    Some(PageKey::Press(paddle)) => keys.press(paddle),
                    Some(PageKey::Release(paddle)) => keys.release(paddle),
                    Some(PageKey::OpenBoth) => keys.release_all(),
                    None => {}
                }
            }
        })
    });
    use_drop(|| {
        dioxus::core::spawn_forever(async {
            let _ = document::eval(STOP_PAGE_PADDLES).await;
        });
    });
}

#[cfg(not(all(not(feature = "desktop"), not(feature = "web"), not(test))))]
fn listen_page_paddles(_keys: PaddleKeys) {}

/// How to key with the paddles in `mode`: what the practice pad and the
/// training screen both tell you.
pub fn paddle_hint(mode: KeyerMode) -> &'static str {
    match mode {
        KeyerMode::Straight => {
            "Hold [ or ] as a straight key — a short press is a dit, a long one a dah, as long as the key speed says."
        }
        KeyerMode::Ultimatic => {
            "Hold [ for dits and ] for dahs. The last paddle you close repeats — squeeze for the middle of X or P."
        }
        KeyerMode::IambicA | KeyerMode::IambicB => {
            "Hold [ for dits and ] for dahs. Squeeze both to alternate; a hold repeats at the key speed."
        }
    }
}

/// Where the paddles are tried out on Settings: a focus target with the
/// letters they key, and why there is no sidetone when there is none. The
/// keys themselves are the app's: they reach the keyer from the page root.
#[component]
pub fn PaddlePad(mode: KeyerMode) -> Element {
    let keys = use_context::<PaddleKeys>();
    let hint = paddle_hint(mode);
    let heard = keys.heard.read();
    let shown = if heard.is_empty() {
        "· −"
    } else {
        heard.as_str()
    };
    rsx! {
        div {
            id: control_id("paddle", "practice"),
            class: "paddle-practice",
            tabindex: "0",
            p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                "{hint} The sidetone plays here, not only in training."
            }
            p {
                id: "paddle-heard",
                class: "paddle-heard",
                aria_live: "polite",
                "{shown}"
            }
            if let Some(error) = keys.sidetone_error.read().as_deref() {
                p { class: "muted", style: "margin: 0; font-size: 0.8rem; color: var(--warn);",
                    "No sidetone: {error}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mobile builds' page listener keeps the brackets from the page and
    /// hands them over, a shortcut press and a repeat aside, every release
    /// heard, the focus lost opening both; and it comes off again, one left
    /// before taken off first.
    #[test]
    fn the_page_hands_over_the_paddles() {
        let script = PAGE_PADDLES_SCRIPT;
        assert!(script.starts_with(STOP_PAGE_PADDLES), "{script}");
        for listened in [
            r#"document.addEventListener("keydown", down)"#,
            r#"document.addEventListener("keyup", up)"#,
            r#"window.addEventListener("blur", blur)"#,
            r#"document.removeEventListener("keydown", down)"#,
            r#"document.removeEventListener("keyup", up)"#,
            r#"window.removeEventListener("blur", blur)"#,
            "if (bracket(e) && !shortcut(e))",
            "if (!e.repeat)",
            // Every release heard, a modifier held or not, and the Command
            // key's own, after which a browser sends no other, opening both.
            "const up = (e) => {\n    if (bracket(e)) {",
            r#"} else if (e.key === "Meta") {"#,
        ] {
            assert!(script.contains(listened), "{listened}");
        }
        assert_eq!(paddle_for_key("[", "", false, false), Some(Paddle::Dit));
        assert_eq!(
            paddle_for_key("", "BracketRight", false, false),
            Some(Paddle::Dah)
        );
        assert_eq!(
            page_key("down", "[", "BracketLeft", false),
            Some(PageKey::Press(Paddle::Dit))
        );
        assert_eq!(
            page_key("up", "]", "BracketRight", false),
            Some(PageKey::Release(Paddle::Dah))
        );
        assert_eq!(page_key("open", "", "", false), Some(PageKey::OpenBoth));
        assert_eq!(page_key("up", "a", "KeyA", false), None);
    }

    #[test]
    fn the_straight_key_says_the_key_speed_times_it() {
        assert!(paddle_hint(KeyerMode::Straight).contains("key speed"));
    }

    /// A held key's repeated presses close its contact once, and its one
    /// release opens it at once.
    #[test]
    fn a_held_key_closes_its_contact_once_and_its_release_opens_it() {
        let mut contacts = Contacts::default();
        assert!(contacts.press(Paddle::Dah));
        assert!(!contacts.press(Paddle::Dah), "repeated");
        assert!(contacts.release(Paddle::Dah));
        assert!(!contacts.release(Paddle::Dah), "already open");
        assert!(contacts.press(Paddle::Dah));
    }

    /// The window losing the focus opens every contact: the next press is a
    /// press.
    #[test]
    fn losing_the_focus_opens_every_contact() {
        let mut contacts = Contacts::default();
        assert!(contacts.press(Paddle::Dah));
        contacts.open_all();
        assert!(contacts.press(Paddle::Dah), "open, so a press closes it");
        assert!(!contacts.release(Paddle::Dit), "never closed");
    }

    #[test]
    fn a_bracket_is_a_paddle_by_what_it_types_or_where_it_sits() {
        assert_eq!(paddle_for_key("[", "", false, false), Some(Paddle::Dit));
        assert_eq!(paddle_for_key("]", "", false, false), Some(Paddle::Dah));
        // A layout where the key types something else: its place decides.
        assert_eq!(
            paddle_for_key("ü", "BracketLeft", false, false),
            Some(Paddle::Dit)
        );
        // What it types wins over where it sits.
        assert_eq!(
            paddle_for_key("]", "BracketLeft", false, false),
            Some(Paddle::Dah)
        );
        assert_eq!(paddle_for_key("[", "", false, true), Some(Paddle::Dah));
        assert_eq!(paddle_for_key("a", "KeyA", false, false), None);
    }

    #[test]
    fn a_bracket_with_a_modifier_is_a_shortcut_not_a_paddle() {
        assert_eq!(paddle_for_key("[", "BracketLeft", true, false), None);
    }
}
