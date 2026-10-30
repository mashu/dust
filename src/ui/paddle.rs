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
use crate::state::{AppCtx, AppState, Screen, Toasts, in_root};
use crate::time::mono_ms;
#[cfg(not(all(feature = "desktop", not(test))))]
use crate::time::sleep_ms;
use crate::ui::training::TOUCH_BUILD;
use crate::ui::widgets::control_id;

/// How many decoded characters the practice pad keeps: about a line's worth.
const HEARD_KEEP: usize = 32;

/// Which of the key events that reach the page's root it keys the paddles
/// from. Another listener keys the rest, so that none is keyed twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootKeys {
    /// Every one: tests drive the root.
    All,
    /// Those the window drops: the desktop on Linux, whose window hears
    /// the rest itself, so a USB keyer works without the webview's focus
    /// and without waiting on its IPC ([`root_keys`]).
    DroppedByWindow,
    /// None: the web and mobile builds, and the desktop on macOS and
    /// Windows, hear them from the whole document, so they work with
    /// nothing focused at all — a HID paddle on a phone keys with nothing
    /// tapped first, and one on a computer with nothing clicked. The
    /// desktop's window hears no keys there, the webview taking them all:
    /// WKWebView as the window's first responder, WebView2 in a window of
    /// its own inside it ([`listen_page_paddles`]).
    Nothing,
}

/// Which key events the page's root keys the paddles from, in this build.
const ROOT_KEYS: RootKeys = if cfg!(test) {
    RootKeys::All
} else if cfg!(all(feature = "desktop", target_os = "linux")) {
    RootKeys::DroppedByWindow
} else {
    RootKeys::Nothing
};

/// Whether the page's root keys the paddles from an event of the key at
/// `code`, typing `key`, as `root` has it. `plain_types` is whether that
/// key pressed alone, on the first layout, types a character, as the
/// window's keymap says ([`plain_key_types`]): None when it cannot say.
///
/// tao on Linux reads a key by what it types pressed alone on the first
/// layout, and drops the event, whatever it types, if that is nothing: a
/// dead key, as the dit key is on a French, Belgian or Spanish keyboard
/// and the dah key on a Portuguese, Swiss German or Nordic one. Every
/// event of such a key is dropped, the `[` AltGr types on the Spanish dit
/// key among them, and under any layout chosen after the first. The root
/// keys each event of such a key, press and release alike, and no other:
/// a dead accent that a key typing a character alone types under a second
/// layout is the window's to key, by place. A key the keymap has nothing
/// for, tao reads by what each event types, and the root keys an event
/// typing nothing it can name: a dead key, or one it cannot identify.
/// Only the brackets' own keys are asked about: a bracket typed elsewhere
/// takes AltGr on a digit or a letter, which the window hears.
fn root_keys(root: RootKeys, key: &Key, code: &str, plain_types: Option<bool>) -> bool {
    match root {
        RootKeys::All => true,
        RootKeys::Nothing => false,
        RootKeys::DroppedByWindow => {
            let dropped = match plain_types {
                Some(types) => !types,
                None => matches!(key, Key::Dead | Key::Unidentified),
            };
            dropped && matches!(code, "BracketLeft" | "BracketRight")
        }
    }
}

/// Whether the key at `code`, one of the brackets' own, types a character
/// pressed alone on the first layout: what tao on Linux reads it by, the
/// keyval GDK's keymap has for it at group 0, level 0. None for another
/// key, or one the keymap has no such keyval for, which tao reads by the
/// keyval of each event instead.
#[cfg(all(feature = "desktop", target_os = "linux", not(test)))]
fn plain_key_types(code: &str) -> Option<bool> {
    use gtk::gdk;
    // How X11 numbers the keys, and Wayland after it: evdev's code, and 8.
    let keycode = match code {
        "BracketLeft" => 34,
        "BracketRight" => 35,
        _ => return None,
    };
    let keymap = gdk::Display::default().and_then(|display| gdk::Keymap::for_display(&display))?;
    let (_, keyval) = keymap
        .entries_for_keycode(keycode)
        .into_iter()
        .find(|(at, _)| at.group() == 0 && at.level() == 0)?;
    let text = gdk::keys::Key::from(keyval).to_unicode();
    Some(text.is_some_and(|ch| ch >= ' ' && ch != '\x7f'))
}

#[cfg(not(all(feature = "desktop", target_os = "linux", not(test))))]
fn plain_key_types(_code: &str) -> Option<bool> {
    None
}

/// Which paddle contacts are closed, and by which key: the code of the key
/// that closed each.
///
/// A key's release opens the contact it closed, whatever it types by then.
/// A bracket typed with AltGr or Option — `[` is AltGr+8 on a German
/// keyboard — whose modifier is let go first, is let go as `8`, and was
/// lost: the paddle stayed down, the keyer sending on and on, and a
/// training session waited on its letter for good.
///
/// A press of a key that has closed a contact is dropped, and so is a
/// press of a contact already closed: tao on Linux never marks a held
/// key's repeated presses as repeats. GTK asks X11 for detectable
/// auto-repeat, and Wayland repeats in the client, so a held key sends one
/// release, at its real key-up, which is taken at once. A repeat is the
/// key's whatever paddle it reads as by then: `[` held while the paddles
/// are swapped repeats as the dah, and took it, and its one release opened
/// only the dit, the dah keying on and on.
#[derive(Clone, Debug, Default)]
struct Contacts {
    dit: Option<String>,
    dah: Option<String>,
}

impl Contacts {
    fn slot(&mut self, paddle: Paddle) -> &mut Option<String> {
        match paddle {
            Paddle::Dit => &mut self.dit,
            Paddle::Dah => &mut self.dah,
        }
    }

    /// The paddle whose contact the key at `code` closed, if it closed one.
    /// A key the page cannot place could be any key, and is none of them.
    fn closed_by(&self, code: &str) -> Option<Paddle> {
        let placed = !code.is_empty() && !code.contains("Unidentified");
        let closer = |paddle| match paddle {
            Paddle::Dit => self.dit.as_deref(),
            Paddle::Dah => self.dah.as_deref(),
        };
        [Paddle::Dit, Paddle::Dah]
            .into_iter()
            .find(|paddle| placed && closer(*paddle) == Some(code))
    }

    /// Whether the key at `code` closed `paddle`'s contact, rather than
    /// being held, its presses repeating, or finding the contact closed by
    /// another key of the paddle.
    fn press(&mut self, paddle: Paddle, code: &str) -> bool {
        if self.closed_by(code).is_some() {
            return false;
        }
        let slot = self.slot(paddle);
        if slot.is_some() {
            return false;
        }
        *slot = Some(code.to_string());
        true
    }

    /// The paddle a key's release at `code` opens: the one that key closed,
    /// whatever it types now, or else `reads_as`, the paddle the key reads
    /// as. Its contact is open after, if it was not already.
    fn release(&mut self, code: &str, reads_as: Option<Paddle>) -> Option<Paddle> {
        let paddle = self.closed_by(code).or(reads_as)?;
        *self.slot(paddle) = None;
        Some(paddle)
    }

    /// Every contact open, as when the window lost the focus and the key
    /// releases with it.
    fn open_all(&mut self) {
        *self = Self::default();
    }
}

/// How keys are read as paddles, as the settings say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reading {
    /// `[` is the dah and `]` the dit.
    swap: bool,
    /// The two keys right of P are the paddles, whatever they type
    /// ([`cw_core::PlaybackSettings::paddle_keys_by_place`]).
    by_place: bool,
}

impl Reading {
    /// As a new install reads them: the paddles on the keys right of P,
    /// `[` the dit.
    #[cfg(test)]
    const DEFAULT: Self = Self {
        swap: false,
        by_place: true,
    };
}

/// Ctrl, Alt and Meta, as held with a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Held {
    ctrl: bool,
    alt: bool,
    meta: bool,
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

/// What a key event types, as text — nothing for a key that types none —
/// and where the key sits.
fn key_and_code(e: &Event<KeyboardData>) -> (String, String) {
    let key = match e.key() {
        Key::Character(text) => text,
        _ => String::new(),
    };
    (key, e.code().to_string())
}

/// The paddle a key is, if it is one: `[` or `]` ([`bracket_for_key`]),
/// the paddles turned round if they are swapped.
fn paddle_for_key(key: &str, code: &str, reading: Reading) -> Option<Paddle> {
    paddle_from_bracket(bracket_for_key(key, code, reading.by_place)?, reading.swap)
}

/// The paddle a key pressed with `held` down is, if it is one and not a
/// shortcut. Meta makes any key one. Ctrl or Alt make the brackets' own keys
/// one, but not a bracket typed elsewhere: that took AltGr, which Windows
/// holds as Ctrl and Alt, or a Mac's Option, which is Alt — `[` is AltGr+8
/// on a German keyboard, Option+5 on a German Mac — so they are how it was
/// typed. It keys the paddle, as on Linux, where AltGr is neither and it
/// always did: on a keyboard whose bracket keys type something else, it is
/// the way to key without a USB paddle.
fn paddle_for_press(key: &str, code: &str, held: Held, reading: Reading) -> Option<Paddle> {
    let paddle = paddle_for_key(key, code, reading)?;
    let on_bracket_key = matches!(code, "BracketLeft" | "BracketRight");
    let shortcut = held.meta || ((held.ctrl || held.alt) && on_bracket_key);
    (!shortcut).then_some(paddle)
}

/// The bracket a key typing `key` at `code` is, if it is one: `[` or `]` by
/// the character it types, or, `by_place`, by where it sits, on the
/// brackets' own keys, whatever it types there.
///
/// A USB paddle sends those keys, and the layout decides what they type:
/// `ü` and `+` on a German keyboard, `+` and a dead accent on a Portuguese
/// one, `/` and `=` on a Dvorak. Nothing in a key event tells the paddle
/// from the keyboard, so which wins is the learner's to say: by place the
/// paddle works on any layout, and a `+` those keys type is the paddle's;
/// not by place, they type, and only a `[` or `]` keys a paddle. In a field
/// the paddles key nothing into they type all the same
/// ([`PaddleKeys::reading`]).
fn bracket_for_key(key: &str, code: &str, by_place: bool) -> Option<char> {
    match (key, code) {
        ("[", _) => Some('['),
        ("]", _) => Some(']'),
        (_, "BracketLeft") if by_place => Some('['),
        (_, "BracketRight") if by_place => Some(']'),
        _ => None,
    }
}

/// The character Dust sends that a key typing `key` at `code` types, if it
/// keys a paddle only by where it sits: a German keyboard's `+`, a Dvorak's
/// `/`, `=`, `?` and `+`.
fn sent_by_place(key: &str, code: &str, reading: Reading) -> Option<char> {
    let mut chars = key.chars();
    let (Some(typed), None) = (chars.next(), chars.next()) else {
        return None;
    };
    let by_place = bracket_for_key(key, code, false).is_none()
        && bracket_for_key(key, code, reading.by_place).is_some();
    (by_place && cw_core::morse_for(typed).is_some()).then_some(typed)
}

/// What the paddles say the first time a key right of P keys one into a box
/// while it types `typed`, a character Dust sends: a letter lands there in
/// its place, and how to type it instead, on the keypad too where a group's
/// box has one, which holds every character a group can.
fn by_place_hint(typed: char, group_box: bool) -> String {
    let keypad = if TOUCH_BUILD && group_box {
        ", or tap it on the keypad"
    } else {
        ""
    };
    format!(
        "That {typed} keyed a paddle: the keys right of P are the paddles. To type {typed}, turn off Paddles on the keys right of P under Settings → Session shape{keypad}."
    )
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
    /// The paddle contacts the keys have closed, as the listeners keying
    /// the paddles hear them.
    contacts: CopyValue<Contacts>,
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
    /// Which key events the page's root keys the paddles from.
    root: RootKeys,
    /// The field the paddles key nothing into that has the cursor, if one
    /// has ([`use_typed_field`]).
    typed_field: CopyValue<Option<ScopeId>>,
    /// Where the app's messages go, to say why a key right of P keyed a
    /// paddle into a box: none for a screen drawn on its own.
    toasts: Option<Toasts>,
    /// Whether that has been said since the app opened.
    told_by_place: CopyValue<bool>,
}

/// Start the app's keyer, and hand it to every screen below.
pub fn use_paddle_keys(
    settings: Signal<TrainingSettings>,
    screen: Signal<Screen>,
    app: Rc<AppState>,
) -> PaddleKeys {
    let toasts = try_use_context::<AppCtx>().map(|ctx| ctx.toasts);
    // In the root scope, where the keyer's timer runs.
    let mut keys = use_context_provider(|| {
        in_root(|| {
            let playback = &settings.peek().playback;
            PaddleKeys {
                keyer: CopyValue::new(Keyer::new(playback.keyer_mode, playback.keyer_wpm)),
                contacts: CopyValue::new(Contacts::default()),
                armed: CopyValue::new(None),
                answer: CopyValue::new(None),
                app: CopyValue::new(app),
                settings,
                screen,
                heard: Signal::new(String::new()),
                sidetone_error: Signal::new(None),
                root: ROOT_KEYS,
                typed_field: CopyValue::new(None),
                toasts,
                told_by_place: CopyValue::new(false),
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
    /// The page root's keydown. `[` and `]` are paddles, never text —
    /// another listener may key them ([`root_keys`]), but the answer box
    /// must not type them.
    pub fn keydown(self, e: Event<KeyboardData>) {
        let (key, code) = key_and_code(&e);
        let mods = e.modifiers();
        let held = Held {
            ctrl: mods.ctrl(),
            alt: mods.alt(),
            meta: mods.meta(),
        };
        let paddle = if self.root_keys(&e, &code) {
            self.key_down(&key, &code, held, e.is_auto_repeating())
        } else {
            paddle_for_press(&key, &code, held, self.reading()).is_some()
        };
        if paddle {
            e.prevent_default();
        }
    }

    /// The page root's keyup.
    pub fn keyup(self, e: Event<KeyboardData>) {
        let (key, code) = key_and_code(&e);
        let rooted = self.root_keys(&e, &code);
        let paddle = if rooted {
            self.key_up(&key, &code)
        } else {
            paddle_for_key(&key, &code, self.reading()).is_some()
        };
        if paddle {
            e.prevent_default();
        } else if rooted && e.key() == Key::Meta {
            // A Mac sends no key release for a key let go while Command
            // was down, as its browsers do not.
            self.release_all();
        }
    }

    /// Whether the root keys the paddles from `e`, an event of the key at
    /// `code` ([`root_keys`]).
    fn root_keys(self, e: &Event<KeyboardData>, code: &str) -> bool {
        root_keys(self.root, &e.key(), code, plain_key_types(code))
    }

    /// The keyer as the desktop's root has it on Linux, keying only the
    /// keys the window drops.
    #[cfg(test)]
    pub(super) fn keyed_as_on_linux(self) -> Self {
        Self {
            root: RootKeys::DroppedByWindow,
            ..self
        }
    }

    /// A key typing `key` at `code` pressed with `held` down, or repeating
    /// while held: whether it is a paddle, to keep from the page. A press
    /// closes the paddle's contact, and remembers which key did.
    fn key_down(mut self, key: &str, code: &str, held: Held, repeat: bool) -> bool {
        let reading = self.reading();
        let Some(paddle) = paddle_for_press(key, code, held, reading) else {
            return false;
        };
        if !repeat && self.contacts.write().press(paddle, code) {
            self.press(paddle);
            if let Some(typed) = sent_by_place(key, code, reading) {
                self.tell_by_place(typed);
            }
        }
        true
    }

    /// A key right of P typing `typed`, a character Dust sends, has keyed a
    /// paddle: if that keys into a box, the first time since the app opened,
    /// say why a letter lands there and how to type `typed` instead. A
    /// learner who meant it would otherwise find only a dit or a dah's letter
    /// in its place.
    fn tell_by_place(self, typed: char) {
        let Some(answer) = *self.answer.peek() else {
            return;
        };
        let mut told = self.told_by_place;
        if !answer.open || *told.peek() {
            return;
        }
        if let Some(toasts) = self.toasts {
            told.set(true);
            // A run's fields take a word space; a group's box has none.
            toasts.show_long(by_place_hint(typed, answer.space.is_none()));
        }
    }

    /// A key typing `key` at `code` let go: whether it was a paddle's. It
    /// opens the contact it closed, whatever it types now and whatever is
    /// held — a paddle held when Ctrl went down still has to open — or else
    /// the paddle it reads as.
    fn key_up(mut self, key: &str, code: &str) -> bool {
        let reads_as = paddle_for_key(key, code, self.reading());
        let opened = self.contacts.write().release(code, reads_as);
        if let Some(paddle) = opened {
            self.release(paddle);
        }
        opened.is_some()
    }

    /// How the keys are read as paddles, as the settings stand: by place
    /// only while no field the paddles key nothing into has the cursor,
    /// where the keys right of P type.
    fn reading(self) -> Reading {
        let playback = &self.settings.peek().playback;
        Reading {
            swap: playback.paddle_swap,
            by_place: playback.paddle_keys_by_place && self.typed_field.peek().is_none(),
        }
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
    pub fn release_all(mut self) {
        self.contacts.write().open_all();
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
        for edge in out.edges() {
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

/// A field you type into that the paddles key nothing into: the word list,
/// the unlock order, a call. While it has the cursor the keys right of P
/// type what they type there, by place or not — a `+` on a German keyboard,
/// a `/` on a Dvorak — and only `[` and `]` are paddles. The field says so on
/// the page as well, with `data-typed`, for the page's own listeners
/// ([`PAGE_PADDLES_SCRIPT`]). Drawn without the app's keyer, it is a field
/// like any other.
#[derive(Clone, Copy)]
pub struct TypedField {
    keys: Option<PaddleKeys>,
    /// The field's own scope, which the keyer holds while it has the cursor.
    field: ScopeId,
}

/// The calling component's field, as one the paddles key nothing into
/// ([`TypedField`]), for as long as it is mounted.
pub fn use_typed_field() -> TypedField {
    let keys = try_use_context::<PaddleKeys>();
    let field = use_hook(dioxus::core::current_scope_id);
    let typed = TypedField { keys, field };
    // A field taken off the page with the cursor in it may never hear it go.
    use_drop(move || typed.blur());
    typed
}

impl TypedField {
    /// The field has the cursor.
    pub fn focus(self) {
        if let Some(keys) = self.keys {
            let mut held = keys.typed_field;
            held.set(Some(self.field));
        }
    }

    /// The field has given the cursor up. Only its own: another field that
    /// has taken it since stays.
    pub fn blur(self) {
        if let Some(keys) = self.keys {
            let mut held = keys.typed_field;
            if *held.peek() == Some(self.field) {
                held.set(None);
            }
        }
    }
}

/// The desktop's window: on Linux it keys the paddles, as it hears them,
/// and everywhere it opens them as it loses the focus. Elsewhere the page's
/// script keys them ([`listen_page_paddles`]).
#[cfg(all(feature = "desktop", not(test)))]
fn listen_native_paddles(keys: PaddleKeys) {
    let heard = use_callback(move |(key, code, pressed): (String, String, bool)| {
        window_key(keys, &key, &code, pressed);
    });
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{ElementState, Event, WindowEvent};
        use dioxus::desktop::tao::keyboard::Key;
        let Event::WindowEvent { event, .. } = event else {
            return;
        };
        let key = match event {
            // Linux and macOS send no key release once the window has lost
            // the focus, so the paddles open with it.
            WindowEvent::Focused(false) => {
                keys.release_all();
                return;
            }
            // On Linux it keys every one it hears, and the root only those
            // it drops. Elsewhere it hears none, the webview taking them,
            // and must key none, or the page's would be keyed twice.
            WindowEvent::KeyboardInput { event: key, .. } if cfg!(target_os = "linux") => key,
            _ => return,
        };
        if key.repeat {
            return;
        }
        // Read as the page reads it, so the paddle it keys is the bracket
        // the page keeps from the box: what it types, and where it sits.
        let typed = match &key.logical_key {
            Key::Character(text) => *text,
            _ => "",
        };
        let code = key.physical_key.to_string();
        match key.state {
            ElementState::Pressed => heard.call((typed.to_string(), code, true)),
            ElementState::Released => heard.call((typed.to_string(), code, false)),
            _ => {}
        }
    });
}

#[cfg(not(all(feature = "desktop", not(test))))]
fn listen_native_paddles(_keys: PaddleKeys) {}

/// A key the desktop's window heard, typing `key` at `code`, pressed or
/// let go. A press of a paddle's key keys it whatever modifiers are held,
/// for the window cannot say which are: tao on Linux tells of them only as
/// a modifier's own key goes down or up in the window, and then of that
/// key alone, so Ctrl held and Shift tapped read as nothing held, and a
/// modifier let go in another window read as held for good. Alt+Tab away
/// and back, and every press of a paddle was taken for a shortcut and
/// dropped, while the root, which reads the modifiers each key was really
/// pressed with, kept the key from the page. So only the root tells a
/// shortcut, for the page: a bracket pressed with Ctrl or Alt is left to
/// the page, which types nothing for it, and keys its paddle all the same.
#[cfg(any(feature = "desktop", test))]
pub(super) fn window_key(keys: PaddleKeys, key: &str, code: &str, pressed: bool) {
    if pressed {
        keys.key_down(key, code, Held::default(), false);
    } else {
        keys.key_up(key, code);
    }
}

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
        let down: Listener = Closure::new(move |e: KeyboardEvent| {
            let held = Held {
                ctrl: e.ctrl_key(),
                alt: e.alt_key(),
                meta: e.meta_key(),
            };
            if keys.key_down(&e.key(), &e.code(), held, e.repeat()) {
                e.prevent_default();
            }
        });
        let up: Listener = Closure::new(move |e: KeyboardEvent| {
            if keys.key_up(&e.key(), &e.code()) {
                e.prevent_default();
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
/// has focus, and a tapped button takes none on iOS; and the desktop's on
/// macOS and Windows takes every key from its window. So, as the web
/// build's ([`listen_document_paddles`]), they hear the paddles from the
/// whole document: the page's script keeps a bracket from the page, read as
/// [`bracket_for_key`] reads one, by place as long as the window's flag
/// does not say otherwise ([`by_place_script`]) and the key is not typed in
/// a field the paddles key nothing into ([`TypedField`]), and hands it
/// over, a press that is a shortcut ([`paddle_for_press`]) or a repeat
/// being nothing; a release is always heard, and so is the release of a
/// key that closed a paddle, whatever it types by then; and the window
/// losing the focus, or Command let go, opens both.
#[cfg(any(
    all(not(feature = "desktop"), not(feature = "web")),
    all(feature = "desktop", not(target_os = "linux")),
    test
))]
const PAGE_PADDLES_SCRIPT: &str = r#"if (window.__dustPaddleKeys) {
    window.__dustPaddleKeys();
}
const byPlace = () => window.__dustPaddlesByPlace !== false;
const bracketKey = (e) => e.code === "BracketLeft" || e.code === "BracketRight";
const typedField = (e) => Boolean(e.target && e.target.closest && e.target.closest("[data-typed]"));
const bracket = (e) => e.key === "[" || e.key === "]" || (byPlace() && bracketKey(e) && !typedField(e));
const shortcut = (e) => e.metaKey || ((e.ctrlKey || e.altKey) && bracketKey(e));
const held = new Set();
const down = (e) => {
    if (bracket(e) && !shortcut(e)) {
        e.preventDefault();
        if (!e.repeat) {
            if (e.code) {
                held.add(e.code);
            }
            dioxus.send(["down", e.key, e.code]);
        }
    }
};
const up = (e) => {
    if (bracket(e) || held.has(e.code)) {
        held.delete(e.code);
        e.preventDefault();
        dioxus.send(["up", e.key, e.code]);
    } else if (e.key === "Meta") {
        held.clear();
        dioxus.send(["open", "", ""]);
    }
};
const blur = () => {
    held.clear();
    dioxus.send(["open", "", ""]);
};
document.addEventListener("keydown", down);
document.addEventListener("keyup", up);
window.addEventListener("blur", blur);
window.__dustPaddleKeys = () => {
    document.removeEventListener("keydown", down);
    document.removeEventListener("keyup", up);
    window.removeEventListener("blur", blur);
    window.__dustPaddleKeys = null;
};"#;

/// The script that tells the page's paddle listener whether the keys right
/// of P are the paddles, whatever they type.
#[cfg(any(
    all(not(feature = "desktop"), not(feature = "web")),
    all(feature = "desktop", not(target_os = "linux")),
    test
))]
fn by_place_script(by_place: bool) -> String {
    format!("window.__dustPaddlesByPlace = {by_place};")
}

/// The script that takes the page's paddle listener off again.
#[cfg(any(
    all(not(feature = "desktop"), not(feature = "web")),
    all(feature = "desktop", not(target_os = "linux")),
    test
))]
const STOP_PAGE_PADDLES: &str = r#"if (window.__dustPaddleKeys) {
    window.__dustPaddleKeys();
}"#;

/// Hand the page's word, `kind` of a key typing `key` at `code`, to the
/// paddles: a press, a release, or both opened. The page has taken the
/// shortcuts and the repeats out already.
#[cfg(any(
    all(not(feature = "desktop"), not(feature = "web")),
    all(feature = "desktop", not(target_os = "linux")),
    test
))]
pub(super) fn page_key(keys: PaddleKeys, kind: &str, key: &str, code: &str) {
    match kind {
        "down" => {
            keys.key_down(key, code, Held::default(), false);
        }
        "up" => {
            keys.key_up(key, code);
        }
        "open" => keys.release_all(),
        _ => {}
    }
}

#[cfg(all(
    any(
        all(not(feature = "desktop"), not(feature = "web")),
        all(feature = "desktop", not(target_os = "linux"))
    ),
    not(test)
))]
fn listen_page_paddles(keys: PaddleKeys) {
    let by_place = use_memo(move || keys.settings.read().playback.paddle_keys_by_place);
    use_effect(move || {
        let script = by_place_script(by_place());
        spawn(async move {
            let _ = document::eval(&script).await;
        });
    });
    use_hook(move || {
        spawn(async move {
            let mut page = document::eval(PAGE_PADDLES_SCRIPT);
            while let Ok((kind, key, code)) = page.recv::<(String, String, String)>().await {
                page_key(keys, &kind, &key, &code);
            }
        })
    });
    use_drop(|| {
        dioxus::core::spawn_forever(async {
            let _ = document::eval(STOP_PAGE_PADDLES).await;
        });
    });
}

#[cfg(not(all(
    any(
        all(not(feature = "desktop"), not(feature = "web")),
        all(feature = "desktop", not(target_os = "linux"))
    ),
    not(test)
)))]
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
            p { class: "muted hint small",
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
            "const up = (e) => {\n    if (bracket(e) || held.has(e.code)) {",
            r#"} else if (e.key === "Meta") {"#,
        ] {
            assert!(script.contains(listened), "{listened}");
        }
    }

    /// The page's script takes a press for a shortcut as the app does
    /// ([`paddle_for_press`]), and remembers the key that closed a paddle,
    /// so that its release is heard whatever it types by then: forgotten
    /// as the paddles open, it is not heard again.
    #[test]
    fn the_page_remembers_which_key_closed_a_paddle() {
        let script = PAGE_PADDLES_SCRIPT;
        for listened in [
            "const shortcut = (e) => e.metaKey || ((e.ctrlKey || e.altKey) && bracketKey(e));",
            "const held = new Set();",
            "                held.add(e.code);",
            "        held.delete(e.code);",
            "const blur = () => {\n    held.clear();",
            "        held.clear();\n        dioxus.send([\"open\", \"\", \"\"]);",
        ] {
            assert!(script.contains(listened), "{listened}\n{script}");
        }
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
        let dah = Some(Paddle::Dah);
        assert!(contacts.press(Paddle::Dah, "BracketRight"));
        assert!(!contacts.press(Paddle::Dah, "BracketRight"), "repeated");
        assert_eq!(contacts.release("BracketRight", dah), dah);
        assert_eq!(contacts.dah, None);
        assert!(contacts.press(Paddle::Dah, "BracketRight"));
    }

    /// A held key keeps the contact it closed when the paddles are swapped
    /// under it: its next press, which tao on Linux does not mark as a
    /// repeat, reads as the other paddle and is a repeat all the same, and
    /// its one release opens the contact it closed, the other never closed.
    #[test]
    fn a_key_held_across_a_swap_repeats_rather_than_closing_the_other_paddle() {
        let mut contacts = Contacts::default();
        assert!(contacts.press(Paddle::Dit, "BracketLeft"));
        assert!(!contacts.press(Paddle::Dah, "BracketLeft"), "a repeat");
        assert_eq!(contacts.dah, None);
        assert_eq!(
            contacts.release("BracketLeft", Some(Paddle::Dah)),
            Some(Paddle::Dit)
        );
        assert_eq!(contacts.dit, None);
        assert_eq!(contacts.dah, None);
        // Keys the page cannot place are not taken for one another.
        assert!(contacts.press(Paddle::Dit, ""));
        assert!(contacts.press(Paddle::Dah, "Unidentified"));
    }

    /// A key let go opens the contact it closed, whatever it types by then:
    /// AltGr let go before the 8 it typed `[` with. Any key a paddle reads
    /// as opens that paddle, and one that is no paddle and closed none
    /// opens nothing.
    #[test]
    fn a_key_let_go_opens_the_contact_it_closed() {
        let mut contacts = Contacts::default();
        assert!(contacts.press(Paddle::Dit, "Digit8"));
        assert!(contacts.press(Paddle::Dah, "BracketRight"));
        assert_eq!(contacts.release("KeyA", None), None);
        assert_eq!(contacts.release("Digit8", None), Some(Paddle::Dit));
        assert_eq!(contacts.dit, None);
        assert_eq!(contacts.dah.as_deref(), Some("BracketRight"));
        // The other paddle's key, by what it types.
        assert_eq!(
            contacts.release("Digit9", Some(Paddle::Dah)),
            Some(Paddle::Dah)
        );
        assert_eq!(contacts.dah, None);
        // A key the page cannot place is no key in particular.
        assert!(contacts.press(Paddle::Dit, ""));
        assert_eq!(contacts.release("", None), None);
        assert_eq!(contacts.release("", Some(Paddle::Dit)), Some(Paddle::Dit));
    }

    /// The window losing the focus opens every contact: the next press is a
    /// press.
    #[test]
    fn losing_the_focus_opens_every_contact() {
        let mut contacts = Contacts::default();
        assert!(contacts.press(Paddle::Dah, "BracketRight"));
        contacts.open_all();
        assert!(
            contacts.press(Paddle::Dah, "BracketRight"),
            "open, so a press closes it"
        );
        assert_eq!(contacts.release("Digit8", None), None, "never closed");
    }

    const SWAPPED: Reading = Reading {
        swap: true,
        ..Reading::DEFAULT
    };

    const TYPED: Reading = Reading {
        by_place: false,
        ..Reading::DEFAULT
    };

    #[test]
    fn a_bracket_is_a_paddle_by_what_it_types_or_where_it_sits() {
        let read = Reading::DEFAULT;
        assert_eq!(paddle_for_key("[", "", read), Some(Paddle::Dit));
        assert_eq!(paddle_for_key("]", "", read), Some(Paddle::Dah));
        // A layout where the key types something else: its place decides.
        assert_eq!(paddle_for_key("ü", "BracketLeft", read), Some(Paddle::Dit));
        // What it types wins over where it sits.
        assert_eq!(paddle_for_key("]", "BracketLeft", read), Some(Paddle::Dah));
        assert_eq!(paddle_for_key("[", "", SWAPPED), Some(Paddle::Dah));
        assert_eq!(paddle_for_key("a", "KeyA", read), None);
    }

    /// By place, the brackets' own keys are the paddles whatever they type,
    /// as a USB paddle sends them on any layout: a German keyboard's `ü` and
    /// `+`, a Portuguese one's `+`, a Dvorak's `/` and `=`, a dead key. Not
    /// by place, those keys type, and only `[` and `]` are paddles, however
    /// they were typed.
    #[test]
    fn the_keys_right_of_p_are_paddles_by_place_or_else_type() {
        for (key, code, paddle) in [
            ("+", "BracketRight", Paddle::Dah),
            ("+", "BracketLeft", Paddle::Dit),
            ("/", "BracketLeft", Paddle::Dit),
            ("=", "BracketRight", Paddle::Dah),
            ("ü", "BracketLeft", Paddle::Dit),
            ("Dead", "BracketLeft", Paddle::Dit),
            ("", "BracketRight", Paddle::Dah),
        ] {
            assert_eq!(
                paddle_for_key(key, code, Reading::DEFAULT),
                Some(paddle),
                "{key}"
            );
            assert_eq!(paddle_for_key(key, code, TYPED), None, "{key}");
        }
        assert_eq!(paddle_for_key("[", "Digit8", TYPED), Some(Paddle::Dit));
        assert_eq!(
            paddle_for_key("]", "BracketRight", TYPED),
            Some(Paddle::Dah)
        );
        // Typed anywhere else, it was never a paddle.
        assert_eq!(paddle_for_key("+", "NumpadAdd", Reading::DEFAULT), None);
    }

    /// A key right of P that keys a paddle only by where it sits, typing a
    /// character Dust sends, is told by that character: a German `+`, a
    /// Dvorak's `/`, `=` and `?`. One that types a bracket, or something
    /// Dust never sends, or that types where it sits, is not.
    #[test]
    fn a_character_dust_sends_keyed_by_place_is_told_by_what_it_types() {
        for (key, code) in [
            ("+", "BracketRight"),
            ("/", "BracketLeft"),
            ("=", "BracketRight"),
            ("?", "BracketLeft"),
        ] {
            let typed = key.chars().next();
            assert_eq!(sent_by_place(key, code, Reading::DEFAULT), typed, "{key}");
            assert_eq!(sent_by_place(key, code, TYPED), None, "{key}");
        }
        for (key, code) in [
            ("[", "BracketLeft"),
            ("]", "BracketRight"),
            ("ü", "BracketLeft"),
            ("", "BracketRight"),
            ("+", "NumpadAdd"),
        ] {
            assert_eq!(sent_by_place(key, code, Reading::DEFAULT), None, "{key}");
        }
        assert!(
            by_place_hint('+', true).contains("To type +, turn off Paddles on the keys right of P")
        );
        // The keypad is a phone's, and a run's has no + to tap.
        assert_eq!(
            by_place_hint('+', true).contains("keypad"),
            TOUCH_BUILD,
            "{}",
            by_place_hint('+', true)
        );
        assert!(!by_place_hint('+', false).contains("keypad"));
    }

    /// The page's script reads a bracket key as the app does: by place,
    /// unless the window's flag, which the settings set, says not, or the
    /// key is typed in a field the paddles key nothing into.
    #[test]
    fn the_page_reads_a_bracket_key_as_the_app_does() {
        let script = PAGE_PADDLES_SCRIPT;
        for read in [
            "const byPlace = () => window.__dustPaddlesByPlace !== false;",
            r#"const bracketKey = (e) => e.code === "BracketLeft" || e.code === "BracketRight";"#,
            r#"const typedField = (e) => Boolean(e.target && e.target.closest && e.target.closest("[data-typed]"));"#,
            r#"const bracket = (e) => e.key === "[" || e.key === "]" || (byPlace() && bracketKey(e) && !typedField(e));"#,
        ] {
            assert!(script.contains(read), "{read}\n{script}");
        }
        assert_eq!(
            by_place_script(false),
            "window.__dustPaddlesByPlace = false;"
        );
        assert_eq!(by_place_script(true), "window.__dustPaddlesByPlace = true;");
    }

    /// Meta makes any press a shortcut, and Ctrl or Alt make one of the
    /// brackets' own keys, whatever they type.
    #[test]
    fn a_bracket_with_a_modifier_is_a_shortcut_not_a_paddle() {
        let none = Held::default();
        let ctrl = Held { ctrl: true, ..none };
        let alt = Held { alt: true, ..none };
        let meta = Held { meta: true, ..none };
        for held in [ctrl, alt, meta] {
            let read = Reading::DEFAULT;
            assert_eq!(paddle_for_press("[", "BracketLeft", held, read), None);
            assert_eq!(paddle_for_press("ü", "BracketLeft", held, read), None);
            assert_eq!(paddle_for_press("", "BracketRight", held, read), None);
        }
        assert_eq!(
            paddle_for_press("[", "BracketLeft", none, Reading::DEFAULT),
            Some(Paddle::Dit)
        );
    }

    /// A bracket typed off the brackets' keys took AltGr or Option to type,
    /// and keys its paddle on every platform: Windows holds AltGr as Ctrl
    /// and Alt, a Mac's Option is Alt, and Linux holds AltGr as neither.
    /// Meta with it is still a shortcut.
    #[test]
    fn a_bracket_typed_with_altgr_or_option_keys_its_paddle() {
        let windows = Held {
            ctrl: true,
            alt: true,
            meta: false,
        };
        let mac = Held {
            alt: true,
            ..Held::default()
        };
        for (held, read) in [windows, mac, Held::default()]
            .into_iter()
            .flat_map(|held| [(held, Reading::DEFAULT), (held, TYPED)])
        {
            assert_eq!(
                paddle_for_press("[", "Digit8", held, read),
                Some(Paddle::Dit),
                "{held:?} {read:?}"
            );
            assert_eq!(
                paddle_for_press("]", "Digit5", held, read),
                Some(Paddle::Dah),
                "{held:?} {read:?}"
            );
        }
        let command = Held { meta: true, ..mac };
        assert_eq!(
            paddle_for_press("[", "Digit5", command, Reading::DEFAULT),
            None
        );
    }

    /// Tests drive the paddles from the root.
    #[test]
    fn tests_key_the_paddles_from_the_root() {
        assert_eq!(ROOT_KEYS, RootKeys::All);
    }

    /// The root keys every event where it drives the paddles, as in tests,
    /// and none where the document's listener keys them.
    #[test]
    fn the_root_keys_all_or_nothing_off_linux() {
        let dah = Key::Character("]".into());
        for (key, code) in [
            (&Key::Dead, "BracketLeft"),
            (&dah, "BracketRight"),
            (&dah, "Digit9"),
            (&Key::Meta, "MetaLeft"),
        ] {
            for plain in [None, Some(true), Some(false)] {
                assert!(root_keys(RootKeys::All, key, code, plain), "{key:?}");
                assert!(!root_keys(RootKeys::Nothing, key, code, plain), "{key:?}");
            }
        }
    }

    /// On Linux the root keys the events of a bracket key that types
    /// nothing alone on the first layout, whatever each types — dead, or
    /// the `[` of AltGr on a Spanish keyboard's — for the window drops
    /// them all, and no event of a key that types something alone, which
    /// the window hears even when it types a dead accent now.
    #[test]
    fn on_linux_the_root_keys_a_bracket_key_the_window_drops() {
        let linux = RootKeys::DroppedByWindow;
        let dit = Key::Character("[".into());
        let dah = Key::Character("]".into());
        for key in [&Key::Dead, &dit, &Key::Unidentified] {
            assert!(root_keys(linux, key, "BracketLeft", Some(false)), "{key:?}");
            assert!(
                root_keys(linux, key, "BracketRight", Some(false)),
                "{key:?}"
            );
            assert!(!root_keys(linux, key, "BracketLeft", Some(true)), "{key:?}");
        }
        assert!(!root_keys(linux, &dah, "BracketRight", Some(true)));
        // Off the brackets' own keys, whatever it types, the window hears it.
        assert!(!root_keys(linux, &dit, "Digit8", Some(false)));
        assert!(!root_keys(linux, &Key::Dead, "Quote", None));
        assert!(!root_keys(linux, &Key::Meta, "MetaLeft", None));
    }

    /// A bracket key the keymap says nothing of, the window reads by what
    /// each event types, and drops one typing nothing it can name: the
    /// root keys a dead key, or one the page cannot identify, and nothing
    /// that types a character.
    #[test]
    fn on_linux_the_root_keys_a_dead_bracket_key_the_keymap_does_not_know() {
        let linux = RootKeys::DroppedByWindow;
        assert!(root_keys(linux, &Key::Dead, "BracketLeft", None));
        assert!(root_keys(linux, &Key::Unidentified, "BracketRight", None));
        for typed in ["[", "]", "+", "ü"] {
            let key = Key::Character(typed.into());
            assert!(!root_keys(linux, &key, "BracketLeft", None), "{typed}");
            assert!(!root_keys(linux, &key, "BracketRight", None), "{typed}");
        }
    }
}
