//! Software paddle keyer wired to `[` / `]` — USB HID paddles included.
//!
//! Training owns a copy so it can type decoded letters into the answer box.
//! The rest of the app shares one instance so a paddle works on Settings and
//! Home without that box being focused, and so the sidetone is not waiting on
//! a training session to have opened the player.

use std::cell::RefCell;
use std::rc::Rc;

use cw_core::{
    dit_ms_for_wpm, paddle_from_bracket, KeyerMode, Paddle, PaddleKeyer, LETTER_GAP_DITS,
};
use dioxus::prelude::*;

use crate::state::{AppState, Screen};
use crate::time::mono_ms;
#[cfg(not(all(feature = "desktop", not(test))))]
use crate::time::sleep_ms;
use crate::ui::widgets::{control_id, html_bool};

/// Squeeze poll while waiting for a letter gap. Coarser than this and iambic
/// squeezes feel late; this is not the sidetone path.
const KEYER_POLL_MS: u32 = 4;

/// X11 key auto-repeat is a fake release+press pair. tao on Linux never marks
/// those as `repeat`, so a leftover press after you let go keeps the keyer
/// running. Wait this long for the fake press; a real re-squeeze is slower.
#[cfg_attr(not(all(feature = "desktop", not(test))), allow(dead_code))]
const REPEAT_RELEASE_MS: u32 = 16;

/// Held-paddle contacts after X11 auto-repeat has been stripped.
#[derive(Clone, Copy, Debug, Default)]
struct RepeatFilter {
    dit: bool,
    dah: bool,
    dit_release: u64,
    dah_release: u64,
}

impl RepeatFilter {
    fn slot(&mut self, paddle: Paddle) -> (&mut bool, &mut u64) {
        match paddle {
            Paddle::Dit => (&mut self.dit, &mut self.dit_release),
            Paddle::Dah => (&mut self.dah, &mut self.dah_release),
        }
    }

    /// A press that is not the matching half of a fake release. `true` means
    /// the paddle just closed.
    fn press(&mut self, paddle: Paddle) -> bool {
        let (down, gen) = self.slot(paddle);
        *gen = gen.wrapping_add(1);
        if *down {
            false
        } else {
            *down = true;
            true
        }
    }

    /// Start a delayed release. The generation is what [`confirm_release`]
    /// needs so a following fake press can cancel it.
    fn release(&mut self, paddle: Paddle) -> Option<u64> {
        let (down, gen) = self.slot(paddle);
        if !*down {
            return None;
        }
        *gen = gen.wrapping_add(1);
        Some(*gen)
    }

    fn confirm_release(&mut self, paddle: Paddle, gen: u64) -> bool {
        let (down, current) = self.slot(paddle);
        if *current != gen {
            return false;
        }
        *down = false;
        true
    }
}

async fn sleep_element(ms: u32) {
    let ms = ms.max(1);
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

pub fn bump_epoch(mut epoch: Signal<u64>) -> u64 {
    let next = epoch.peek().saturating_add(1);
    epoch.set(next);
    next
}

pub fn bracket_from_key(e: &Event<KeyboardData>) -> Option<char> {
    match e.key() {
        Key::Character(ref s) if s == "[" => Some('['),
        Key::Character(ref s) if s == "]" => Some(']'),
        _ => match e.code() {
            Code::BracketLeft => Some('['),
            Code::BracketRight => Some(']'),
            _ => None,
        },
    }
}

/// DOM paddle handlers run in tests and on web. Desktop production reads the
/// paddles from the native window so a USB keyer works without a focused input
/// and without waiting on WebKitGTK IPC.
pub fn capture_dom_paddles() -> bool {
    cfg!(test) || !cfg!(feature = "desktop")
}

fn modifiers_block_paddle(e: &Event<KeyboardData>) -> bool {
    let mods = e.modifiers();
    mods.ctrl() || mods.alt() || mods.meta()
}

#[allow(clippy::too_many_arguments)]
pub fn paddle_down(
    paddle: Paddle,
    dit_ms: u64,
    enabled: bool,
    mut keyer: Signal<PaddleKeyer>,
    mut keying: Signal<bool>,
    gen: Signal<u64>,
    on_tone: EventHandler<bool>,
    on_letter: EventHandler<char>,
) {
    if !enabled {
        return;
    }
    let dit_ms = dit_ms.max(1);
    if keyer.peek().mode().is_straight() {
        let already = keyer.peek().any_held();
        let _ = bump_epoch(gen);
        keyer.write().press_at(paddle, mono_ms());
        if !already {
            on_tone.call(true);
        }
        return;
    }
    keyer.write().press(paddle);
    if *keying.peek() {
        return;
    }
    keying.set(true);
    let run = bump_epoch(gen);
    // The first element has to start in this keydown, not after spawn is
    // polled — a dit at 20 WPM is 60 ms, and a tick of delay is the whole
    // character feeling late.
    let Some(first) = keyer.write().next_element() else {
        keying.set(false);
        return;
    };
    on_tone.call(true);
    keyer.write().decoder_mut().push(first);
    spawn(async move {
        sleep_element(dit_ms.saturating_mul(first.dits()).max(1) as u32).await;
        if *gen.peek() != run {
            on_tone.call(false);
            keying.set(false);
            return;
        }
        on_tone.call(false);
        sleep_element(dit_ms as u32).await;
        run_iambic_loop(dit_ms, keyer, keying, gen, run, on_tone, on_letter).await;
    });
}

async fn run_iambic_loop(
    dit_ms: u64,
    mut keyer: Signal<PaddleKeyer>,
    mut keying: Signal<bool>,
    gen: Signal<u64>,
    run: u64,
    on_tone: EventHandler<bool>,
    on_letter: EventHandler<char>,
) {
    loop {
        if *gen.peek() != run {
            on_tone.call(false);
            keying.set(false);
            return;
        }
        let next = keyer.write().next_element();
        if let Some(paddle) = next {
            on_tone.call(true);
            keyer.write().decoder_mut().push(paddle);
            sleep_element(dit_ms.saturating_mul(paddle.dits()).max(1) as u32).await;
            if *gen.peek() != run {
                on_tone.call(false);
                keying.set(false);
                return;
            }
            on_tone.call(false);
            sleep_element(dit_ms.max(1) as u32).await;
            continue;
        }
        let mut left = dit_ms.saturating_mul(LETTER_GAP_DITS).max(1);
        let mut squeezed = false;
        while left > 0 {
            if *gen.peek() != run {
                keying.set(false);
                return;
            }
            if keyer.peek().any_held() {
                squeezed = true;
                break;
            }
            let step = left.min(u64::from(KEYER_POLL_MS)).max(1);
            sleep_element(step as u32).await;
            left = left.saturating_sub(step);
        }
        if squeezed {
            continue;
        }
        if *gen.peek() != run {
            keying.set(false);
            return;
        }
        keying.set(false);
        if let Some(ch) = keyer.write().decoder_mut().take_letter() {
            on_letter.call(ch);
        }
        return;
    }
}

pub fn paddle_up(
    paddle: Paddle,
    dit_ms: u64,
    mut keyer: Signal<PaddleKeyer>,
    gen: Signal<u64>,
    on_tone: EventHandler<bool>,
    on_letter: EventHandler<char>,
) {
    let dit_ms = dit_ms.max(1);
    if keyer.peek().mode().is_straight() {
        keyer.write().release(paddle);
        if keyer.peek().any_held() {
            return;
        }
        on_tone.call(false);
        let Some(element) = keyer.write().take_straight(mono_ms(), dit_ms) else {
            return;
        };
        keyer.write().decoder_mut().push(element);
        let run = bump_epoch(gen);
        spawn(async move {
            let mut left = dit_ms.saturating_mul(LETTER_GAP_DITS).max(1);
            while left > 0 {
                if *gen.peek() != run {
                    return;
                }
                if keyer.peek().any_held() {
                    return;
                }
                let step = left.min(u64::from(KEYER_POLL_MS)).max(1);
                sleep_element(step as u32).await;
                left = left.saturating_sub(step);
            }
            if *gen.peek() != run {
                return;
            }
            if let Some(ch) = keyer.write().decoder_mut().take_letter() {
                on_letter.call(ch);
            }
        });
        return;
    }
    keyer.write().release(paddle);
}

/// Latest training paddle handlers. Native window keys have to reach the
/// session keyer; `use_wry_event_handler` keeps the first closure it is given,
/// so TrainingView writes the current handlers here each render.
#[derive(Clone)]
pub struct TrainingPaddleSink {
    down: Rc<RefCell<EventHandler<Paddle>>>,
    up: Rc<RefCell<EventHandler<Paddle>>>,
}

impl Default for TrainingPaddleSink {
    fn default() -> Self {
        Self {
            down: Rc::new(RefCell::new(EventHandler::new(|_| {}))),
            up: Rc::new(RefCell::new(EventHandler::new(|_| {}))),
        }
    }
}

impl TrainingPaddleSink {
    pub fn bind(&self, down: EventHandler<Paddle>, up: EventHandler<Paddle>) {
        *self.down.borrow_mut() = down;
        *self.up.borrow_mut() = up;
    }

    pub fn clear(&self) {
        *self.down.borrow_mut() = EventHandler::new(|_| {});
        *self.up.borrow_mut() = EventHandler::new(|_| {});
    }

    #[cfg_attr(test, allow(dead_code))]
    fn call_down(&self, paddle: Paddle) {
        self.down.borrow().call(paddle);
    }

    #[cfg_attr(test, allow(dead_code))]
    fn call_up(&self, paddle: Paddle) {
        self.up.borrow().call(paddle);
    }
}

/// Shared keyer for every screen that is not a live training answer.
pub struct PaddleKeys {
    pub keydown: EventHandler<Event<KeyboardData>>,
    pub keyup: EventHandler<Event<KeyboardData>>,
    pub down: EventHandler<Paddle>,
    pub up: EventHandler<Paddle>,
    pub heard: Signal<String>,
}

pub fn use_paddle_keys(
    settings: Signal<cw_core::TrainingSettings>,
    screen: Signal<Screen>,
    app: Rc<AppState>,
) -> PaddleKeys {
    let mut keyer = use_signal(|| PaddleKeyer::with_mode(settings.peek().playback.keyer_mode));
    let mut keying = use_signal(|| false);
    let gen = use_signal(|| 0u64);
    let mut heard = use_signal(String::new);
    let training = use_hook(TrainingPaddleSink::default);
    use_hook({
        let training = training.clone();
        move || provide_context(training)
    });
    let app_tone = app.clone();
    let app_warm = app.clone();

    use_effect(use_reactive!(|settings| {
        let mode = settings().playback.keyer_mode;
        if keyer.peek().mode() != mode {
            keyer.write().set_mode(mode);
        }
    }));

    use_effect(use_reactive!(|screen| {
        if matches!(screen(), Screen::Training) {
            let _ = bump_epoch(gen);
            keyer.write().reset();
            keying.set(false);
            app_tone.set_live_tone(false, &settings.peek());
        }
    }));

    use_effect(use_reactive!(|screen, settings| {
        if !matches!(screen(), Screen::Settings) {
            return;
        }
        let _ = app_warm.ensure_live_sidetone(&settings());
    }));

    let on_tone = EventHandler::new({
        move |on: bool| {
            app.set_live_tone(on, &settings.peek());
        }
    });
    let on_letter = EventHandler::new(move |ch: char| {
        heard.write().push(ch);
    });

    let down = EventHandler::new({
        move |paddle: Paddle| {
            if matches!(screen(), Screen::Training) {
                return;
            }
            let dit_ms = dit_ms_for_wpm(settings.peek().playback.keyer_wpm);
            paddle_down(paddle, dit_ms, true, keyer, keying, gen, on_tone, on_letter);
        }
    });
    let up = EventHandler::new({
        move |paddle: Paddle| {
            if matches!(screen(), Screen::Training) {
                return;
            }
            let dit_ms = dit_ms_for_wpm(settings.peek().playback.keyer_wpm);
            paddle_up(paddle, dit_ms, keyer, gen, on_tone, on_letter);
        }
    });

    listen_native_paddles(settings, screen, training, down, up);

    let keydown = EventHandler::new({
        move |e: Event<KeyboardData>| {
            if !capture_dom_paddles() || modifiers_block_paddle(&e) {
                return;
            }
            let Some(ch) = bracket_from_key(&e) else {
                return;
            };
            if e.is_auto_repeating() {
                e.prevent_default();
                return;
            }
            let swap = settings.peek().playback.paddle_swap;
            let Some(paddle) = paddle_from_bracket(ch, swap) else {
                return;
            };
            e.prevent_default();
            down.call(paddle);
        }
    });
    let keyup = EventHandler::new({
        move |e: Event<KeyboardData>| {
            if !capture_dom_paddles() || modifiers_block_paddle(&e) {
                return;
            }
            let Some(ch) = bracket_from_key(&e) else {
                return;
            };
            let swap = settings.peek().playback.paddle_swap;
            let Some(paddle) = paddle_from_bracket(ch, swap) else {
                return;
            };
            e.prevent_default();
            up.call(paddle);
        }
    });

    PaddleKeys {
        keydown,
        keyup,
        down,
        up,
        heard,
    }
}

#[cfg(all(feature = "desktop", not(test)))]
fn listen_native_paddles(
    settings: Signal<cw_core::TrainingSettings>,
    screen: Signal<Screen>,
    training: TrainingPaddleSink,
    down: EventHandler<Paddle>,
    up: EventHandler<Paddle>,
) {
    let contacts = use_hook(|| Rc::new(RefCell::new(RepeatFilter::default())));
    let dispatch = use_callback({
        let contacts = Rc::clone(&contacts);
        move |(paddle, pressed): (Paddle, bool)| {
            let in_training = matches!(*screen.peek(), Screen::Training);
            if pressed {
                if !contacts.borrow_mut().press(paddle) {
                    return;
                }
                if in_training {
                    training.call_down(paddle);
                } else {
                    down.call(paddle);
                }
                return;
            }
            let Some(gen) = contacts.borrow_mut().release(paddle) else {
                return;
            };
            let contacts = Rc::clone(&contacts);
            let training = training.clone();
            spawn(async move {
                sleep_element(REPEAT_RELEASE_MS).await;
                if !contacts.borrow_mut().confirm_release(paddle, gen) {
                    return;
                }
                if matches!(*screen.peek(), Screen::Training) {
                    training.call_up(paddle);
                } else {
                    up.call(paddle);
                }
            });
        }
    });
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use tao::event::{ElementState, Event, WindowEvent};
        use tao::keyboard::{Key, KeyCode};
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
        let swap = settings.peek().playback.paddle_swap;
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
fn listen_native_paddles(
    _settings: Signal<cw_core::TrainingSettings>,
    _screen: Signal<Screen>,
    _training: TrainingPaddleSink,
    _down: EventHandler<Paddle>,
    _up: EventHandler<Paddle>,
) {
}

#[component]
pub fn PaddlePad(
    heard: String,
    mode: KeyerMode,
    #[props(default)] capture_keys: bool,
    #[props(default)] on_down: EventHandler<Paddle>,
    #[props(default)] on_up: EventHandler<Paddle>,
    #[props(default)] swap: bool,
) -> Element {
    let hint = match mode {
        KeyerMode::Straight => "Hold [ or ] as a straight key — a short press is a dit, a long one a dah.",
        KeyerMode::Ultimatic => {
            "Hold [ for dits and ] for dahs. The last paddle you close repeats. The sidetone plays here, not only in training."
        }
        KeyerMode::IambicA | KeyerMode::IambicB => {
            "Hold [ for dits and ] for dahs. Squeeze both to alternate. The sidetone plays here, not only in training."
        }
    };
    let shown = if heard.is_empty() {
        "· −".to_string()
    } else {
        heard
    };
    rsx! {
        div {
            id: control_id("paddle", "practice"),
            class: "paddle-practice",
            tabindex: "0",
            autofocus: html_bool(false),
            onkeydown: move |e| {
                if !capture_keys {
                    return;
                }
                let Some(ch) = bracket_from_key(&e) else {
                    return;
                };
                e.prevent_default();
                e.stop_propagation();
                if e.is_auto_repeating() {
                    return;
                }
                if let Some(paddle) = paddle_from_bracket(ch, swap) {
                    on_down.call(paddle);
                }
            },
            onkeyup: move |e| {
                if !capture_keys {
                    return;
                }
                let Some(ch) = bracket_from_key(&e) else {
                    return;
                };
                e.prevent_default();
                e.stop_propagation();
                if let Some(paddle) = paddle_from_bracket(ch, swap) {
                    on_up.call(paddle);
                }
            },
            p { class: "muted", style: "margin: 0; font-size: 0.8rem;", "{hint}" }
            p {
                id: "paddle-heard",
                class: "paddle-heard",
                aria_live: "polite",
                "{shown}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fake_press_after_release_cancels_the_up() {
        let mut contacts = RepeatFilter::default();
        assert!(contacts.press(Paddle::Dah));
        assert!(!contacts.press(Paddle::Dah));
        let gen = contacts.release(Paddle::Dah).expect("was down");
        assert!(!contacts.press(Paddle::Dah));
        assert!(!contacts.confirm_release(Paddle::Dah, gen));
        let later = contacts.release(Paddle::Dah).expect("still down");
        assert!(contacts.confirm_release(Paddle::Dah, later));
        assert!(contacts.release(Paddle::Dah).is_none());
    }

    #[test]
    fn a_real_release_is_confirmed_when_no_press_follows() {
        let mut contacts = RepeatFilter::default();
        assert!(contacts.press(Paddle::Dit));
        let gen = contacts.release(Paddle::Dit).expect("was down");
        assert!(contacts.confirm_release(Paddle::Dit, gen));
        assert!(contacts.press(Paddle::Dit));
    }
}
