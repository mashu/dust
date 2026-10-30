//! The run's key script ([`keys_script`]) run on a page, in Node: the only
//! way a run hears its keys, on every platform.

use super::*;
use crate::testing::{PageKey, PageStep, on_fake_page};
use crate::ui::paddle::bracket_for_key;

/// Where the key typing `key` sits on a US keyboard, and whether it takes
/// Shift there.
fn us_place(key: &str) -> (&'static str, bool) {
    match key {
        ";" => ("Semicolon", false),
        "." => ("Period", false),
        "," => ("Comma", false),
        "+" => ("Equal", true),
        _ => panic!("where is {key} on a US keyboard?"),
    }
}

/// The press that gives each of the run's keys ([`KEYS`]), on a US
/// keyboard, and what it does.
fn each_key() -> Vec<(PageKey, RunAction)> {
    KEYS.iter()
        .map(|(known, action)| {
            let press = match *known {
                Known::Alone(code) => PageKey::down(code, code),
                Known::Alt(code) => {
                    let letter = code.trim_start_matches("Key").to_lowercase();
                    PageKey::down(&letter, code).alt()
                }
                Known::Typed(key) => match us_place(key) {
                    (code, true) => PageKey::down(key, code).shift(),
                    (code, false) => PageKey::down(key, code),
                },
                Known::HeldEnter => PageKey::down("Enter", "Enter").shift(),
            };
            (press, *action)
        })
        .collect()
}

/// Take each press on a page with the run's key script on it, the paddles
/// on their keys by place or not, and check that the script does what is
/// `expected` of it: tells the run the key, once, by a name that does that
/// ([`action_for`]), and keeps it from the browser; or, with nothing
/// expected, neither. Every word it sends is one the run reads.
fn tells_the_run(by_place: bool, expected: &[(PageKey, Option<RunAction>)]) {
    let mut page = vec![PageStep::run(keys_script(by_place))];
    page.extend(
        expected
            .iter()
            .map(|(press, _)| PageStep::Key(press.clone())),
    );
    let Some(heard) = on_fake_page(&page) else {
        return;
    };
    for ((press, action), heard) in expected.iter().zip(&heard[1..]) {
        let told: Vec<Option<RunAction>> = heard
            .sent
            .iter()
            .map(|word| {
                let name: String = serde_json::from_value(word.clone())
                    .unwrap_or_else(|_| panic!("{word} is no name the run reads"));
                action_for(&name)
            })
            .collect();
        let wanted: Vec<Option<RunAction>> = action.iter().map(|action| Some(*action)).collect();
        assert_eq!(told, wanted, "{press:?}: {heard:?}");
        assert_eq!(heard.prevented, action.is_some(), "{press:?}");
    }
}

/// Each of the run's keys, pressed as on a keyboard, is told to the run by
/// a name that does what MorseRunner has it do, and kept from the browser;
/// Enter with Ctrl too, and `.` and `+` off the numeric pad.
#[test]
fn the_page_tells_the_run_each_of_its_keys() {
    let mut presses: Vec<(PageKey, Option<RunAction>)> = each_key()
        .into_iter()
        .map(|(press, action)| (press, Some(action)))
        .collect();
    presses.extend([
        (
            PageKey::down("Enter", "NumpadEnter").ctrl(),
            Some(RunAction::Key(OpKey::LogOnly)),
        ),
        (
            PageKey::down(".", "NumpadDecimal"),
            Some(RunAction::Key(OpKey::TuAndLog)),
        ),
        (
            PageKey::down("+", "NumpadAdd"),
            Some(RunAction::Key(OpKey::TuAndLog)),
        ),
    ]);
    for by_place in [true, false] {
        tells_the_run(by_place, &presses);
    }
}

/// A key held down is kept from the browser on every repeat, but told only
/// once; and a key typed while an input method composes a character is the
/// input method's.
#[test]
fn the_page_tells_a_held_key_once() {
    let Some(heard) = on_fake_page(&[
        PageStep::run(keys_script(true)),
        PageKey::down("F1", "F1").into(),
        PageKey::down("F1", "F1").repeat().into(),
        PageKey::down(";", "Semicolon").composing().into(),
    ]) else {
        return;
    };
    assert_eq!(heard[1].sent, [serde_json::json!("F1")]);
    assert!(heard[2].sent.is_empty() && heard[2].prevented);
    assert!(heard[3].sent.is_empty() && !heard[3].prevented);
}

/// What is not the run's is passed by: a key held with the Command key, or
/// with Ctrl but Enter, Shift with anything but a character, Alt with
/// anything but the letters it takes, a letter alone, Enter alone, Space
/// and Tab, F11, and a character the paddles' own keys type while those
/// keys are the paddles by place ([`bracket_for_key`]), which is the run's
/// otherwise.
#[test]
fn the_page_passes_by_what_is_not_the_runs() {
    for by_place in [true, false] {
        let mut presses: Vec<(PageKey, Option<RunAction>)> = Vec::new();
        for (press, _) in each_key() {
            presses.push((press.clone().meta(), None));
            if press.code != "Enter" {
                presses.push((press.ctrl(), None));
            }
        }
        for press in [
            PageKey::down("F1", "F1").shift(),
            PageKey::down("Escape", "Escape").shift(),
            PageKey::down("F1", "F1").alt(),
            PageKey::down(".", "Period").alt(),
            PageKey::down("s", "KeyS"),
            PageKey::down("S", "KeyS").shift(),
            PageKey::down("Enter", "Enter"),
            PageKey::down(" ", "Space"),
            PageKey::down("Tab", "Tab"),
            PageKey::down("F11", "F11"),
            PageKey::down("F9", "F9"),
        ] {
            presses.push((press, None));
        }
        // A German keyboard's `+` right of P, a Portuguese one's left of it.
        for (key, code) in [("+", "BracketRight"), ("+", "BracketLeft")] {
            let paddles = bracket_for_key(key, code, by_place).is_some();
            let action = (!paddles).then(|| action_for(key)).flatten();
            presses.push((PageKey::down(key, code), action));
        }
        tells_the_run(by_place, &presses);
    }
}

/// The script run again takes the listener a run before left off first, so
/// a key is told once; taken off, it tells nothing more.
#[test]
fn the_page_tells_a_key_once_and_nothing_once_taken_off() {
    let Some(heard) = on_fake_page(&[
        PageStep::run(keys_script(true)),
        PageStep::run(keys_script(true)),
        PageKey::down("F2", "F2").into(),
        PageStep::run(STOP_LISTENING),
        PageKey::down("F2", "F2").into(),
    ]) else {
        return;
    };
    assert_eq!(heard[2].sent, [serde_json::json!("F2")]);
    assert!(heard[4].sent.is_empty() && !heard[4].prevented);
}
