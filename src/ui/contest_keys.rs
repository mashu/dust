//! A run's keys on a keyboard (§4.5 of `docs/contest-mode.md`), as
//! MorseRunner has them wherever a browser lets it: F1 calls CQ, F2 sends
//! your exchange, F3 TU without logging, F4 your call, F5 their call, F6
//! `QSO B4`, F7 `?`, F8 `NIL`, F12 `NR?` (`AGN` in Simple QSO, which has no
//! number), `;` (or Insert) their call and your exchange; `.`, `,` or `+`
//! send TU and log, and Enter held with Shift, Ctrl or Alt logs without
//! sending. Esc or Alt+S stops sending, Alt+W wipes the fields, PgUp and
//! PgDn move your speed, Alt+P pauses and carries on, Alt+E ends the run.
//! Enter alone, and Space, which takes the cursor between Call and
//! Exchange, are the fields' own; Tab moves on as on any page. F11 stays
//! the app's fullscreen and `[` `]` the paddles.
//!
//! While the run screen is up, a listener on the document hears these keys
//! wherever the focus is, nowhere at all included, and keeps the browser
//! from doing what it would with them: help on F1, find on F3, a reload on
//! F5, caret browsing on F7, the developer tools on F12, a scroll on PgUp,
//! a menu on Alt, `.` typed into a field. In fullscreen it asks for Esc as
//! well, which a browser otherwise keeps to leave fullscreen with.

use cw_core::{ContestPhase, OpKey, PauseWhy};
use dioxus::prelude::*;

use crate::contest_runtime::{change_speed, pause_run, press_key, resume_run};
use crate::session_runtime::finish_run;
use crate::state::AppCtx;

/// How far − / + move your speed, in WPM: MorseRunner's step.
pub const SPEED_STEP_WPM: i32 = 2;

/// What a key does in a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunAction {
    Key(OpKey),
    Speed(i32),
    /// Pause, or carry on after a pause.
    Pause,
    EndRun,
}

/// How the listener knows a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Known {
    /// By where it sits, pressed alone: `F1`.
    Alone(&'static str),
    /// By where it sits, with Alt held: `KeyS`. A letter is known by its
    /// place, not what it types: Alt+S types ß on a Mac.
    Alt(&'static str),
    /// By the character it types, Shift held or not: `+` takes Shift on
    /// many a keyboard, `;` on others. A key the paddles sit on, `[` or
    /// `]`, stays theirs whatever it types.
    Typed(&'static str),
    /// Enter, with Shift, Ctrl or Alt held.
    HeldEnter,
}

impl Known {
    /// The name the listener tells the run the key by.
    fn name(self) -> String {
        match self {
            Self::Alone(code) | Self::Typed(code) => code.to_string(),
            Self::Alt(code) => format!("Alt+{code}"),
            Self::HeldEnter => HELD_ENTER.to_string(),
        }
    }
}

/// What the listener calls Enter held with Shift, Ctrl or Alt.
const HELD_ENTER: &str = "Shift+Enter";

/// Each key, and what it does.
const KEYS: [(Known, RunAction); 22] = [
    (Known::Alone("F1"), RunAction::Key(OpKey::Cq)),
    (Known::Alone("F2"), RunAction::Key(OpKey::Exchange)),
    (Known::Alone("F3"), RunAction::Key(OpKey::Tu)),
    (Known::Alone("F4"), RunAction::Key(OpKey::MyCall)),
    (Known::Alone("F5"), RunAction::Key(OpKey::HisCall)),
    (Known::Alone("F6"), RunAction::Key(OpKey::B4)),
    (Known::Alone("F7"), RunAction::Key(OpKey::Again)),
    (Known::Alone("F8"), RunAction::Key(OpKey::Nil)),
    (Known::Alone("F12"), RunAction::Key(OpKey::NrQuery)),
    (Known::Typed(";"), RunAction::Key(OpKey::HisCallAndExchange)),
    // Insert as well, as MorseRunner has it; `;` is the one the screen
    // names, as a screen reader keeps Insert for itself.
    (
        Known::Alone("Insert"),
        RunAction::Key(OpKey::HisCallAndExchange),
    ),
    (Known::Typed("."), RunAction::Key(OpKey::TuAndLog)),
    (Known::Typed(","), RunAction::Key(OpKey::TuAndLog)),
    (Known::Typed("+"), RunAction::Key(OpKey::TuAndLog)),
    (Known::HeldEnter, RunAction::Key(OpKey::LogOnly)),
    (Known::Alone("Escape"), RunAction::Key(OpKey::StopTx)),
    (Known::Alt("KeyS"), RunAction::Key(OpKey::StopTx)),
    (Known::Alt("KeyW"), RunAction::Key(OpKey::Wipe)),
    (Known::Alone("PageUp"), RunAction::Speed(SPEED_STEP_WPM)),
    (Known::Alone("PageDown"), RunAction::Speed(-SPEED_STEP_WPM)),
    (Known::Alt("KeyP"), RunAction::Pause),
    (Known::Alt("KeyE"), RunAction::EndRun),
];

/// What the key the listener names `name` does.
fn action_for(name: &str) -> Option<RunAction> {
    KEYS.iter()
        .find(|(known, _)| known.name() == name)
        .map(|(_, action)| *action)
}

/// A key the listener heard, by the name it tells it by: do what it does
/// in the run.
fn pressed(ctx: AppCtx, name: &str) {
    let Some(action) = action_for(name) else {
        return;
    };
    let phase = ctx.contest.peek().as_ref().map(|view| view.phase);
    match action {
        RunAction::Key(key) => press_key(ctx, key),
        // As − / + are, only while the run counts down or runs.
        RunAction::Speed(delta) => {
            if matches!(
                phase,
                Some(ContestPhase::Countdown { .. } | ContestPhase::Running)
            ) {
                change_speed(ctx, delta);
            }
        }
        RunAction::Pause => {
            if matches!(phase, Some(ContestPhase::Paused { .. })) {
                resume_run(ctx);
            } else {
                pause_run(ctx, PauseWhy::You);
            }
        }
        RunAction::EndRun => finish_run(ctx),
    }
}

/// The keys known by `kind`, as a script writes a list of what it matches
/// them by.
fn listed(kind: fn(Known) -> Option<&'static str>) -> String {
    let listed: Vec<String> = KEYS
        .iter()
        .filter_map(|(known, _)| kind(*known))
        .map(|key| format!("\"{key}\""))
        .collect();
    format!("[{}]", listed.join(", "))
}

/// The script that hears the run's keys on the document, keeps them from
/// the browser and tells the run, and locks Esc in fullscreen where the
/// browser can. A key held down is kept from the browser too, but told
/// once. Ctrl and the Command key make a shortcut of any other key, and
/// Shift of any but a character, so those are passed by. A listener left by
/// a run before is taken off first.
fn keys_script() -> String {
    format!(
        r#"{STOP_LISTENING}
const alone = new Set({alone});
const withAlt = new Set({alt});
const typed = new Set({typed});
const name = (e) => {{
    if (e.code === "Enter" || e.code === "NumpadEnter") {{
        return e.shiftKey || e.ctrlKey || e.altKey ? "{HELD_ENTER}" : null;
    }}
    if (e.ctrlKey) {{
        return null;
    }}
    if (e.altKey) {{
        return withAlt.has(e.code) ? "Alt+" + e.code : null;
    }}
    if (typed.has(e.key) && !e.code.startsWith("Bracket")) {{
        return e.key;
    }}
    return !e.shiftKey && alone.has(e.code) ? e.code : null;
}};
const heard = (e) => {{
    if (e.metaKey || e.isComposing) {{
        return;
    }}
    const key = name(e);
    if (key === null) {{
        return;
    }}
    e.preventDefault();
    if (!e.repeat) {{
        dioxus.send(key);
    }}
}};
const lock = () => {{
    if (document.fullscreenElement && navigator.keyboard && navigator.keyboard.lock) {{
        navigator.keyboard.lock(["Escape"]).catch(() => {{}});
    }}
}};
document.addEventListener("keydown", heard);
document.addEventListener("fullscreenchange", lock);
lock();
window.__dustRunKeys = () => {{
    document.removeEventListener("keydown", heard);
    document.removeEventListener("fullscreenchange", lock);
    if (navigator.keyboard && navigator.keyboard.unlock) {{
        navigator.keyboard.unlock();
    }}
}};"#,
        alone = listed(|known| match known {
            Known::Alone(code) => Some(code),
            _ => None,
        }),
        alt = listed(|known| match known {
            Known::Alt(code) => Some(code),
            _ => None,
        }),
        typed = listed(|known| match known {
            Known::Typed(key) => Some(key),
            _ => None,
        }),
    )
}

/// The script that takes the listener off again.
const STOP_LISTENING: &str = r#"if (window.__dustRunKeys) {
    window.__dustRunKeys();
    window.__dustRunKeys = null;
}"#;

/// While the calling component is mounted, the run hears its keys from the
/// whole document.
pub fn use_run_keys(ctx: AppCtx) {
    // Tests have no page to run it on: they press the keys through
    // [`pressed`].
    #[cfg(not(test))]
    {
        use_hook(move || {
            spawn(async move {
                let mut page = document::eval(&keys_script());
                while let Ok(name) = page.recv::<String>().await {
                    pressed(ctx, &name);
                }
            })
        });
        use_drop(|| {
            dioxus::core::spawn_forever(async {
                let _ = document::eval(STOP_LISTENING).await;
            });
        });
    }
    #[cfg(test)]
    let _ = ctx;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Screen;
    use crate::testing::{Ui, contest_settings, run};

    /// Each key does what MorseRunner has it do, told by the name the
    /// listener gives it.
    #[test]
    fn each_key_does_what_morserunner_has_it_do() {
        for (name, action) in [
            ("F1", RunAction::Key(OpKey::Cq)),
            ("F2", RunAction::Key(OpKey::Exchange)),
            ("F3", RunAction::Key(OpKey::Tu)),
            ("F4", RunAction::Key(OpKey::MyCall)),
            ("F5", RunAction::Key(OpKey::HisCall)),
            ("F6", RunAction::Key(OpKey::B4)),
            ("F7", RunAction::Key(OpKey::Again)),
            ("F8", RunAction::Key(OpKey::Nil)),
            ("F12", RunAction::Key(OpKey::NrQuery)),
            (";", RunAction::Key(OpKey::HisCallAndExchange)),
            ("Insert", RunAction::Key(OpKey::HisCallAndExchange)),
            (".", RunAction::Key(OpKey::TuAndLog)),
            (",", RunAction::Key(OpKey::TuAndLog)),
            ("+", RunAction::Key(OpKey::TuAndLog)),
            ("Shift+Enter", RunAction::Key(OpKey::LogOnly)),
            ("Escape", RunAction::Key(OpKey::StopTx)),
            ("Alt+KeyS", RunAction::Key(OpKey::StopTx)),
            ("Alt+KeyW", RunAction::Key(OpKey::Wipe)),
            ("PageUp", RunAction::Speed(2)),
            ("PageDown", RunAction::Speed(-2)),
            ("Alt+KeyP", RunAction::Pause),
            ("Alt+KeyE", RunAction::EndRun),
        ] {
            assert_eq!(action_for(name), Some(action), "{name}");
        }
        // A letter alone is typing, Space the fields' own (it moves the
        // cursor between them, or types the space between an exchange's
        // items), and F11 and the paddles are the app's.
        for name in [
            "KeyS",
            "KeyP",
            "Alt+F1",
            "F11",
            "BracketLeft",
            "BracketRight",
            "Enter",
            "Space",
            "Tab",
            "F9",
            "F10",
            "Shift+F4",
        ] {
            assert_eq!(action_for(name), None, "{name}");
        }
    }

    /// The harness never mounts anything on a page, so the script is the
    /// part a test can see: it hears exactly the run's keys, keeps them from
    /// the browser, tells the run once per press, locks Esc in fullscreen
    /// where it can, and comes off again, a run before's listener first.
    #[test]
    fn the_page_keeps_the_runs_keys_from_the_browser() {
        let script = keys_script();
        assert!(
            script.contains(
                r#"new Set(["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F12", "Insert", "Escape", "PageUp", "PageDown"])"#
            ),
            "{script}"
        );
        assert!(
            script.contains(r#"new Set(["KeyS", "KeyW", "KeyP", "KeyE"])"#),
            "{script}"
        );
        assert!(
            script.contains(r#"new Set([";", ".", ",", "+"])"#),
            "{script}"
        );
        // Enter alone stays the fields'; held with Shift, Ctrl or Alt, it
        // logs without sending.
        assert!(
            script
                .contains(r#"return e.shiftKey || e.ctrlKey || e.altKey ? "Shift+Enter" : null;"#),
            "{script}"
        );
        // A character the paddles' keys type is still the paddles'.
        assert!(script.contains(r#"!e.code.startsWith("Bracket")"#));
        assert!(!script.contains("F11"));
        let prevent = script.find("e.preventDefault()").expect("kept");
        let send = script.find("dioxus.send(key)").expect("told");
        assert!(prevent < send, "{script}");
        assert!(script.contains("if (!e.repeat)"), "{script}");
        assert!(
            script.contains(r#"navigator.keyboard.lock(["Escape"])"#),
            "{script}"
        );
        assert!(script.contains("navigator.keyboard.unlock()"), "{script}");
        assert!(
            script.contains(r#"document.removeEventListener("keydown", heard)"#),
            "{script}"
        );
        let off = script.find("window.__dustRunKeys();").expect("taken off");
        let on = script.find("addEventListener").expect("put on");
        assert!(off < on, "{script}");
        assert!(STOP_LISTENING.contains("window.__dustRunKeys = null;"));
    }

    /// The contester's keys do what MorseRunner has them do: F5 sends their
    /// call alone, F2 your exchange, `.` TU and logs; `;` their call and
    /// your exchange; Shift+Enter logs without sending, checked at once; F3
    /// sends TU and logs nothing, and Enter then logs that QSO. Enter held
    /// with Shift in a field is no Enter.
    #[test]
    fn the_contesters_keys_work_the_run() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-start-training");
            ui.advance(8_000).await;
            let sent = |recorder: &crate::audio::fake::Recorder| {
                recorder
                    .starts()
                    .last()
                    .map(|start| start.transcript.clone())
            };
            let call = recorder.starts()[0].transcript.others[0].text.clone();
            ui.type_into("field-call", &call);
            ui.with_app(|ctx| pressed(ctx, "F5"));
            assert_eq!(sent(&recorder).map(|turn| turn.text), Some(call.clone()));
            ui.advance(8_000).await;
            ui.with_app(|ctx| pressed(ctx, "F2"));
            let exchange = sent(&recorder).expect("your exchange");
            assert_eq!(exchange.text, "5NN TT1");
            ui.advance(8_000).await;
            let serial = exchange.others[0]
                .text
                .split_whitespace()
                .last()
                .unwrap_or_default()
                .to_string();
            ui.type_into("field-exchange", &serial);
            ui.with_app(|ctx| pressed(ctx, "."));
            let tu = sent(&recorder).expect("TU");
            assert_eq!(tu.text, "TU");
            assert_eq!(
                ui.with_app(|ctx| ctx.contest.peek().as_ref().map(|view| view.qsos)),
                Some(1)
            );

            // The next caller, answering the TU: its call and your exchange
            // at once, and its exchange logged without sending.
            ui.advance(8_000).await;
            let next = tu.others[0].text.clone();
            ui.type_into("field-call", &next);
            ui.with_app(|ctx| pressed(ctx, ";"));
            let both = sent(&recorder).expect("call and exchange");
            assert_eq!(both.text, format!("{next} 5NN TT2"));
            ui.advance(8_000).await;
            let serial = both.others[0]
                .text
                .split_whitespace()
                .last()
                .unwrap_or_default()
                .to_string();
            ui.type_into("field-exchange", &serial);
            let starts = recorder.starts().len();
            // Shift+Enter in the field is not Enter: the page's keys hear it.
            ui.press_key_held("field-exchange", Key::Enter, Code::Enter, Modifiers::SHIFT);
            assert_eq!(recorder.starts().len(), starts, "nothing sent");
            ui.with_app(|ctx| pressed(ctx, "Shift+Enter"));
            assert_eq!(recorder.starts().len(), starts, "logged without sending");
            assert_eq!(
                ui.with_app(|ctx| ctx.contest.peek().as_ref().map(|view| view.qsos)),
                Some(2)
            );
            let view = ui
                .with_app(|ctx| ctx.contest.peek().clone())
                .expect("a run");
            assert!(
                matches!(view.log[1].status, cw_core::RowStatus::Checked(_)),
                "checked at once"
            );

            // A third, closed with TU and nothing logged; Enter logs it.
            ui.with_app(|ctx| pressed(ctx, "F1"));
            ui.advance(8_000).await;
            let third = sent(&recorder).expect("CQ").others[0].text.clone();
            ui.type_into("field-call", &third);
            ui.with_app(|ctx| pressed(ctx, "Insert"));
            let both = sent(&recorder).expect("call and exchange");
            assert_eq!(both.text, format!("{third} 5NN TT3"));
            ui.advance(8_000).await;
            let serial = both.others[0]
                .text
                .split_whitespace()
                .last()
                .unwrap_or_default()
                .to_string();
            ui.type_into("field-exchange", &serial);
            ui.with_app(|ctx| pressed(ctx, "F3"));
            assert_eq!(
                sent(&recorder).map(|turn| turn.text),
                Some("TU".to_string())
            );
            assert_eq!(
                ui.with_app(|ctx| ctx.contest.peek().as_ref().map(|view| view.qsos)),
                Some(2)
            );
            assert!(
                ui.has("TU &amp; log</span>") || ui.has("TU &#38; log</span>"),
                "Enter logs it now"
            );
            ui.press_enter("field-exchange");
            assert_eq!(
                ui.with_app(|ctx| ctx.contest.peek().as_ref().map(|view| view.qsos)),
                Some(3)
            );
        });
    }

    /// F4 sends your call, F6 the call typed and `QSO B4`, F8 `NIL` and F12
    /// `NR?`, each as MorseRunner sends it.
    #[test]
    fn your_call_b4_nil_and_nr_keys_send_their_messages() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-start-training");
            ui.advance(8_000).await;
            let sent = |recorder: &crate::audio::fake::Recorder| {
                recorder
                    .starts()
                    .last()
                    .map(|start| start.transcript.text.clone())
                    .unwrap_or_default()
            };
            let call = recorder.starts()[0].transcript.others[0].text.clone();
            for (key, message) in [
                ("F4", "DL1ABC".to_string()),
                ("F8", "NIL".to_string()),
                ("F12", "NR?".to_string()),
            ] {
                ui.with_app(|ctx| pressed(ctx, key));
                assert_eq!(sent(&recorder), message, "{key}");
                ui.advance(8_000).await;
            }
            ui.type_into("field-call", &call);
            ui.with_app(|ctx| pressed(ctx, "F6"));
            assert_eq!(sent(&recorder), format!("{call} QSO B4"));
        });
    }

    /// The keys work the run as its buttons do: F1 calls CQ, PgUp and PgDn
    /// move your speed while − / + can, Alt+P pauses and carries on, Alt+E
    /// ends the run.
    #[test]
    fn the_keys_work_the_run() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-start-training");
            ui.advance(8_000).await;
            let sent = recorder.starts().len();
            ui.with_app(|ctx| pressed(ctx, "F1"));
            let cq = recorder
                .starts()
                .last()
                .map(|start| start.transcript.text.clone());
            assert_eq!(recorder.starts().len(), sent + 1);
            assert!(cq.is_some_and(|text| text.starts_with("CQ DL1ABC")));

            let wpm = |ui: &Ui, wpm: u32| ui.has(&format!("run-wpm-value\">{wpm}<"));
            assert!(wpm(&ui, 30), "{}", ui.html());
            ui.with_app(|ctx| pressed(ctx, "PageUp"));
            assert!(wpm(&ui, 32));
            ui.with_app(|ctx| pressed(ctx, "PageDown"));
            ui.with_app(|ctx| pressed(ctx, "PageDown"));
            assert!(wpm(&ui, 28));

            ui.take_scrolls();
            ui.with_app(|ctx| pressed(ctx, "Alt+KeyP"));
            assert!(ui.shows("btn-resume"), "Alt+P pauses");
            assert_eq!(ui.take_scrolls(), 0, "and scrolls nothing");
            // Paused, your speed stays as it is, as − / + are disabled.
            ui.with_app(|ctx| pressed(ctx, "PageUp"));
            assert!(wpm(&ui, 28));
            assert!(ui.input_tag("more-wpm").contains("disabled"));
            ui.with_app(|ctx| pressed(ctx, "Alt+KeyP"));
            assert!(!ui.shows("btn-resume"), "and carries on");
            assert_eq!(ui.take_scrolls(), 1, "from the run screen's top");
            assert!(ui.shows("btn-pause"));

            ui.with_app(|ctx| pressed(ctx, "Alt+KeyE"));
            assert_eq!(ui.screen(), Screen::Results.key());
        });
    }
}
