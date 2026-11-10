//! The page's paddle script ([`PAGE_PADDLES_SCRIPT`]) run on a page, in
//! Node: the only way the paddles reach the keyer on macOS, Windows and the
//! phones, held to what the web build's own listeners do with the same keys
//! ([`listen_document_paddles`]).

use super::*;
use crate::testing::{PageKey, PageStep, on_fake_page};

/// What happens on the page, as the paddles meet it.
#[derive(Clone, Debug)]
enum Step {
    Key(PageKey),
    /// An input method ends a composition.
    CompositionEnd,
    /// The window loses the focus.
    Blur,
    /// The settings read the keys by place, or not ([`by_place_script`]).
    ByPlace(bool),
}

impl Step {
    fn on_page(&self) -> PageStep {
        match self {
            Self::Key(key) => PageStep::Key(key.clone()),
            Self::CompositionEnd => PageStep::CompositionEnd,
            Self::Blur => PageStep::Blur,
            Self::ByPlace(by_place) => PageStep::run(by_place_script(*by_place)),
        }
    }
}

impl From<PageKey> for Step {
    fn from(key: PageKey) -> Self {
        Self::Key(key)
    }
}

fn down(key: &str, code: &str) -> PageKey {
    PageKey::down(key, code)
}

fn up(key: &str, code: &str) -> PageKey {
    PageKey::up(key, code)
}

/// The paddles the contacts have closed, by the key that closed each.
fn closed(contacts: &Contacts) -> (Option<String>, Option<String>) {
    (contacts.dit.clone(), contacts.dah.clone())
}

/// Take `steps` on a page with the paddle script on it, and, beside it, as
/// the web build's listeners do: after each step, the script has kept the
/// page's default exactly when they do, and what it has handed over, read
/// as [`page_key`] reads it, has left the same contacts closed as they
/// have. Every word it hands over is one `page_key` can read.
fn keys_as_the_web_build_does(steps: &[Step]) {
    let mut page = vec![PageStep::run(PAGE_PADDLES_SCRIPT)];
    page.extend(steps.iter().map(Step::on_page));
    let Some(heard) = on_fake_page(&page) else {
        return;
    };
    // Unset, the page's flag reads the keys by place, as a new install does.
    let mut reading = Reading::DEFAULT;
    let mut web = Contacts::default();
    let mut input_methods = InputMethods::default();
    let mut handed = Contacts::default();
    for (step, heard) in steps.iter().zip(&heard[1..]) {
        let kept = match step {
            Step::Key(key) if key.kind == "keydown" => {
                let held = Held {
                    ctrl: key.ctrl_key,
                    alt: key.alt_key,
                    meta: key.meta_key,
                };
                let paddle = paddle_for_press(&key.key, &key.code, held, reading);
                let left = input_methods.left_to_it(
                    paddle.is_some(),
                    key.key == "Dead",
                    key.is_composing,
                    Some(key.key_code),
                );
                if let Some(paddle) = paddle
                    && !key.repeat
                    && !left
                {
                    web.press(paddle, &key.code);
                }
                paddle.is_some() && !left
            }
            Step::Key(key) => {
                let reads_as = paddle_for_key(&key.key, &key.code, reading);
                let opened = web.release(&key.code, reads_as);
                if opened.is_none() && key.key == "Meta" {
                    web.open_all();
                }
                opened.is_some()
            }
            Step::CompositionEnd => {
                input_methods.ended();
                false
            }
            Step::Blur => {
                web.open_all();
                input_methods.ended();
                false
            }
            Step::ByPlace(by_place) => {
                reading.by_place = *by_place;
                false
            }
        };
        for word in &heard.sent {
            let (kind, key, code): (String, String, String) = serde_json::from_value(word.clone())
                .unwrap_or_else(|_| panic!("{word} is no word page_key reads"));
            match kind.as_str() {
                "down" => {
                    if let Some(paddle) = paddle_for_press(&key, &code, Held::default(), reading) {
                        handed.press(paddle, &code);
                    }
                }
                "up" => {
                    handed.release(&code, paddle_for_key(&key, &code, reading));
                }
                "open" => handed.open_all(),
                _ => {}
            }
        }
        assert_eq!(heard.prevented, kept, "{step:?}: {heard:?}");
        assert_eq!(closed(&handed), closed(&web), "{step:?}: {heard:?}");
    }
}

/// By place, as a new install reads them, the brackets' own keys key the
/// paddles whatever they type, a held key's repeats close nothing more,
/// and a bracket typed elsewhere keys its paddle too: AltGr+8 on a German
/// keyboard, which Windows holds as Ctrl and Alt, its release heard as the
/// 8 it types once AltGr is let go first.
#[test]
fn the_page_keys_the_paddles_by_place_as_the_app_does() {
    keys_as_the_web_build_does(&[
        down("[", "BracketLeft").into(),
        down("[", "BracketLeft").repeat().into(),
        up("[", "BracketLeft").into(),
        down("ü", "BracketLeft").into(),
        down("+", "BracketRight").into(),
        up("+", "BracketRight").into(),
        up("ü", "BracketLeft").into(),
        down("]", "BracketLeft").into(),
        up("]", "BracketLeft").into(),
        down("[", "Digit8").ctrl().alt().into(),
        up("8", "Digit8").into(),
        down("[", "Digit5").alt().into(),
        up("[", "Digit5").into(),
        down("a", "KeyA").into(),
        up("a", "KeyA").into(),
    ]);
}

/// Not by place, the brackets' own keys type what they type, and only a
/// `[` or `]` keys a paddle; a key that closed a paddle by place is heard
/// let go after the reading changes under it.
#[test]
fn the_page_keys_the_paddles_by_what_they_type_as_the_app_does() {
    keys_as_the_web_build_does(&[
        Step::ByPlace(false),
        down("ü", "BracketLeft").into(),
        up("ü", "BracketLeft").into(),
        down("+", "BracketRight").into(),
        up("+", "BracketRight").into(),
        down("]", "BracketRight").into(),
        up("]", "BracketRight").into(),
        down("[", "Digit8").ctrl().alt().into(),
        up("8", "Digit8").into(),
        Step::ByPlace(true),
        down("ü", "BracketLeft").into(),
        Step::ByPlace(false),
        up("ü", "BracketLeft").into(),
    ]);
}

/// A press that is a shortcut keys nothing and is left to the page: Meta
/// with any key, Ctrl or Alt with the brackets' own; its release is still
/// heard. Command let go opens both paddles, as a Mac sends no release for
/// a key let go while it was down, and so does the window losing the focus.
#[test]
fn the_page_leaves_shortcuts_and_opens_the_paddles_as_the_app_does() {
    keys_as_the_web_build_does(&[
        down("[", "BracketLeft").ctrl().into(),
        up("[", "BracketLeft").ctrl().into(),
        down("ü", "BracketLeft").alt().into(),
        up("ü", "BracketLeft").into(),
        down("]", "BracketRight").meta().into(),
        up("]", "BracketRight").into(),
        down("[", "Digit8").ctrl().alt().meta().into(),
        down("]", "BracketRight").into(),
        down("Meta", "MetaLeft").meta().into(),
        up("Meta", "MetaLeft").into(),
        down("[", "BracketLeft").into(),
        down("]", "BracketRight").into(),
        Step::Blur,
        up("[", "BracketLeft").into(),
    ]);
}

/// The script run again takes the listener it left before off first, so a
/// key is handed over once; taken off, it hands over nothing more.
#[test]
fn the_page_hands_a_key_over_once_and_nothing_once_taken_off() {
    let Some(heard) = on_fake_page(&[
        PageStep::run(PAGE_PADDLES_SCRIPT),
        PageStep::run(PAGE_PADDLES_SCRIPT),
        down("[", "BracketLeft").into(),
        PageStep::run(STOP_PAGE_PADDLES),
        up("[", "BracketLeft").into(),
        PageStep::Blur,
    ]) else {
        return;
    };
    assert_eq!(
        heard[2].sent,
        [serde_json::json!(["down", "[", "BracketLeft"])]
    );
    assert!(heard[4].sent.is_empty() && !heard[4].prevented);
    assert!(heard[5].sent.is_empty());
}

/// What the page should do with a step: the words it hands the app, and
/// whether it keeps the page's own default.
type Does = (Step, &'static [[&'static str; 3]], bool);

fn does(step: impl Into<Step>, sent: &'static [[&'static str; 3]], prevented: bool) -> Does {
    (step.into(), sent, prevented)
}

/// Nothing handed over, and the page's default left alone.
const NOTHING: &[[&str; 3]] = &[];

/// Take `steps` on a page with the paddle script on it, the keys read by
/// place or not as `by_place` says, and check that it does with each what
/// is expected of it.
fn hands_over(by_place: bool, steps: &[Does]) {
    let mut page = vec![
        PageStep::run(PAGE_PADDLES_SCRIPT),
        PageStep::run(by_place_script(by_place)),
    ];
    page.extend(steps.iter().map(|(step, ..)| step.on_page()));
    let Some(heard) = on_fake_page(&page) else {
        return;
    };
    for ((step, sent, prevented), heard) in steps.iter().zip(&heard[2..]) {
        let sent: Vec<serde_json::Value> =
            sent.iter().map(|word| serde_json::json!(word)).collect();
        assert_eq!(
            (&heard.sent, heard.prevented),
            (&sent, *prevented),
            "by place {by_place}: {step:?}"
        );
    }
}

/// Pinyin, or Kana, composing what a letter began: a `[` or a `]` turns a
/// page of candidates there.
fn pinyin() -> Vec<Does> {
    vec![
        does(down("n", "KeyN").taken_by_input_method(), NOTHING, false),
        does(down("[", "BracketLeft").composing(), NOTHING, false),
        does(
            up("[", "BracketLeft").composing(),
            &[["up", "[", "BracketLeft"]],
            true,
        ),
        does(down("]", "BracketRight").composing(), NOTHING, false),
        does(
            up("]", "BracketRight").composing(),
            &[["up", "]", "BracketRight"]],
            true,
        ),
        does(Step::CompositionEnd, NOTHING, false),
    ]
}

/// Within a composition no paddle key opened, Pinyin's or Kana's begun with
/// a letter, a press of a paddle's key is the input method's: it keys
/// nothing and is left to the page, by place or not; its release is heard,
/// as every release is. A dead paddle key still keys by place, as a French
/// keyboard's dit key and a Portuguese one's dah key, which Safari, Chrome
/// on a Mac and the macOS build hand over with the input method's key code:
/// its first press, which opens a composition of its own, and each press of
/// either paddle within that one, a squeeze among them, until it ends or
/// another key is typed in it. Not by place, the dead key types, and is no
/// paddle's.
#[test]
fn the_page_leaves_a_composition_no_paddle_key_opened_to_the_input_method() {
    let mut by_place = pinyin();
    by_place.extend([
        // A French keyboard's dit key, dead.
        does(
            down("Dead", "BracketLeft").taken_by_input_method(),
            &[["down", "Dead", "BracketLeft"]],
            true,
        ),
        does(
            up("Dead", "BracketLeft").composing(),
            &[["up", "Dead", "BracketLeft"]],
            true,
        ),
        does(
            down("Dead", "BracketLeft").composing(),
            &[["down", "Dead", "BracketLeft"]],
            true,
        ),
        does(
            down("$", "BracketRight").composing(),
            &[["down", "$", "BracketRight"]],
            true,
        ),
        does(
            up("Dead", "BracketLeft").composing(),
            &[["up", "Dead", "BracketLeft"]],
            true,
        ),
        does(
            up("$", "BracketRight"),
            &[["up", "$", "BracketRight"]],
            true,
        ),
        does(Step::CompositionEnd, NOTHING, false),
        // A composition begun by no key the page heard.
        does(down("[", "BracketLeft").composing(), NOTHING, false),
        does(
            up("[", "BracketLeft").composing(),
            &[["up", "[", "BracketLeft"]],
            true,
        ),
        does(Step::CompositionEnd, NOTHING, false),
        // A Portuguese keyboard's dah key, dead, and a letter typed in the
        // composition it opens, which is the input method's from then on.
        does(
            down("Dead", "BracketRight").taken_by_input_method(),
            &[["down", "Dead", "BracketRight"]],
            true,
        ),
        does(
            down("+", "BracketLeft").composing(),
            &[["down", "+", "BracketLeft"]],
            true,
        ),
        does(
            up("+", "BracketLeft").composing(),
            &[["up", "+", "BracketLeft"]],
            true,
        ),
        does(
            up("Dead", "BracketRight").composing(),
            &[["up", "Dead", "BracketRight"]],
            true,
        ),
        does(down("a", "KeyA").composing(), NOTHING, false),
        does(down("+", "BracketLeft").composing(), NOTHING, false),
        does(
            up("+", "BracketLeft").composing(),
            &[["up", "+", "BracketLeft"]],
            true,
        ),
        does(Step::CompositionEnd, NOTHING, false),
    ]);
    hands_over(true, &by_place);
    keys_as_the_web_build_does(
        &by_place
            .into_iter()
            .map(|(step, ..)| step)
            .collect::<Vec<_>>(),
    );
    let mut typed = pinyin();
    typed.extend([
        does(
            down("Dead", "BracketLeft").taken_by_input_method(),
            NOTHING,
            false,
        ),
        does(down("Dead", "BracketLeft").composing(), NOTHING, false),
        does(down("$", "BracketRight").composing(), NOTHING, false),
        does(up("Dead", "BracketLeft").composing(), NOTHING, false),
        does(up("$", "BracketRight"), NOTHING, false),
        does(Step::CompositionEnd, NOTHING, false),
    ]);
    hands_over(false, &typed);
    let typed: Vec<Step> = [Step::ByPlace(false)]
        .into_iter()
        .chain(typed.into_iter().map(|(step, ..)| step))
        .collect();
    keys_as_the_web_build_does(&typed);
}
