//! A run's keys on a keyboard (§4.5 of `docs/contest-mode.md`), as
//! MorseRunner has them wherever a browser lets it: F1 calls CQ, F7 sends
//! `?`, Esc or Alt+S stops sending, Alt+W wipes the fields, PgUp and PgDn
//! move your speed, Alt+P pauses and carries on, Alt+E ends the run. Enter,
//! and Space in the Call field, are the fields' own; Tab moves on as on any
//! page. F11 stays the app's fullscreen and `[` `]` the paddles.
//!
//! While the run screen is up, a listener on the document hears these keys
//! wherever the focus is, nowhere at all included, and keeps the browser
//! from doing what it would with them: help on F1, caret browsing on F7, a
//! scroll on PgUp, a menu on Alt. In fullscreen it asks for Esc as well,
//! which a browser otherwise keeps to leave fullscreen with.

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

/// Each key, by where it sits on the keyboard, whether Alt is held, and what
/// it does. A letter is known by its place, not what it types: Alt+S types
/// ß on a Mac.
const KEYS: [(&str, bool, RunAction); 9] = [
    ("F1", false, RunAction::Key(OpKey::Cq)),
    ("F7", false, RunAction::Key(OpKey::Again)),
    ("Escape", false, RunAction::Key(OpKey::StopTx)),
    ("KeyS", true, RunAction::Key(OpKey::StopTx)),
    ("KeyW", true, RunAction::Key(OpKey::Wipe)),
    ("PageUp", false, RunAction::Speed(SPEED_STEP_WPM)),
    ("PageDown", false, RunAction::Speed(-SPEED_STEP_WPM)),
    ("KeyP", true, RunAction::Pause),
    ("KeyE", true, RunAction::EndRun),
];

/// What the key at `code` does, with Alt held or not. Ctrl, Shift and the
/// Command key make a shortcut of any key, so the listener passes those by.
fn action_for(code: &str, alt: bool) -> Option<RunAction> {
    KEYS.iter()
        .find(|(key, with_alt, _)| *key == code && *with_alt == alt)
        .map(|(_, _, action)| *action)
}

/// A key the listener heard, by the place it sits and whether Alt was held:
/// do what it does in the run.
fn pressed(ctx: AppCtx, code: &str, alt: bool) {
    let Some(action) = action_for(code, alt) else {
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

/// The codes of the run's keys pressed alone, or with Alt, as a script
/// writes a list.
fn codes(alt: bool) -> String {
    let codes: Vec<String> = KEYS
        .iter()
        .filter(|(_, with_alt, _)| *with_alt == alt)
        .map(|(code, _, _)| format!("\"{code}\""))
        .collect();
    format!("[{}]", codes.join(", "))
}

/// The script that hears the run's keys on the document, keeps them from
/// the browser and tells the run, and locks Esc in fullscreen where the
/// browser can. A key held down is kept from the browser too, but told
/// once. A listener left by a run before is taken off first.
fn keys_script() -> String {
    format!(
        r#"{STOP_LISTENING}
const plain = new Set({plain});
const withAlt = new Set({alt});
const heard = (e) => {{
    if (e.ctrlKey || e.metaKey || e.shiftKey || e.isComposing) {{
        return;
    }}
    if (!(e.altKey ? withAlt : plain).has(e.code)) {{
        return;
    }}
    e.preventDefault();
    if (!e.repeat) {{
        dioxus.send([e.code, e.altKey]);
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
        plain = codes(false),
        alt = codes(true),
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
                while let Ok((code, alt)) = page.recv::<(String, bool)>().await {
                    pressed(ctx, &code, alt);
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

    #[test]
    fn each_key_does_what_morserunner_has_it_do() {
        assert_eq!(action_for("F1", false), Some(RunAction::Key(OpKey::Cq)));
        assert_eq!(action_for("F7", false), Some(RunAction::Key(OpKey::Again)));
        assert_eq!(
            action_for("Escape", false),
            Some(RunAction::Key(OpKey::StopTx))
        );
        assert_eq!(
            action_for("KeyS", true),
            Some(RunAction::Key(OpKey::StopTx))
        );
        assert_eq!(action_for("KeyW", true), Some(RunAction::Key(OpKey::Wipe)));
        assert_eq!(action_for("PageUp", false), Some(RunAction::Speed(2)));
        assert_eq!(action_for("PageDown", false), Some(RunAction::Speed(-2)));
        assert_eq!(action_for("KeyP", true), Some(RunAction::Pause));
        assert_eq!(action_for("KeyE", true), Some(RunAction::EndRun));
        // A letter alone is typing, and F11 and the paddles are the app's.
        for (code, alt) in [
            ("KeyS", false),
            ("KeyP", false),
            ("F1", true),
            ("F11", false),
            ("BracketLeft", false),
            ("BracketRight", false),
            ("Enter", false),
            ("Space", false),
            ("Tab", false),
        ] {
            assert_eq!(action_for(code, alt), None, "{code} {alt}");
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
            script.contains(r#"new Set(["F1", "F7", "Escape", "PageUp", "PageDown"])"#),
            "{script}"
        );
        assert!(
            script.contains(r#"new Set(["KeyS", "KeyW", "KeyP", "KeyE"])"#),
            "{script}"
        );
        assert!(!script.contains("F11") && !script.contains("Bracket"));
        let prevent = script.find("e.preventDefault()").expect("kept");
        let send = script
            .find("dioxus.send([e.code, e.altKey])")
            .expect("told");
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
            ui.with_app(|ctx| pressed(ctx, "F1", false));
            let cq = recorder
                .starts()
                .last()
                .map(|start| start.transcript.text.clone());
            assert_eq!(recorder.starts().len(), sent + 1);
            assert!(cq.is_some_and(|text| text.starts_with("CQ DL1ABC")));

            let wpm = |ui: &Ui, wpm: u32| ui.has(&format!("run-wpm-value\">{wpm}<"));
            assert!(wpm(&ui, 30), "{}", ui.html());
            ui.with_app(|ctx| pressed(ctx, "PageUp", false));
            assert!(wpm(&ui, 32));
            ui.with_app(|ctx| pressed(ctx, "PageDown", false));
            ui.with_app(|ctx| pressed(ctx, "PageDown", false));
            assert!(wpm(&ui, 28));

            ui.with_app(|ctx| pressed(ctx, "KeyP", true));
            assert!(ui.shows("btn-resume"), "Alt+P pauses");
            // Paused, your speed stays as it is, as − / + are disabled.
            ui.with_app(|ctx| pressed(ctx, "PageUp", false));
            assert!(wpm(&ui, 28));
            assert!(ui.input_tag("more-wpm").contains("disabled"));
            ui.with_app(|ctx| pressed(ctx, "KeyP", true));
            assert!(!ui.shows("btn-resume"), "and carries on");
            assert!(ui.shows("btn-pause"));

            ui.with_app(|ctx| pressed(ctx, "KeyE", true));
            assert_eq!(ui.screen(), Screen::Results.key());
        });
    }
}
