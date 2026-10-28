//! Keeps the screen on while a session runs: through the count down, every
//! send and every answer. A phone that dims and locks part way through a
//! group loses the group, and the screen is where the answer goes. A contest
//! run holds it while it is live, counting down or running; paused, it lets
//! the screen sleep, and a run is never lost to that. Hear a QSO holds it
//! while it plays, its lines on the screen: a page out of sight stops it.
//!
//! Android sets its window's own keep-screen-on flag, which the system holds
//! for as long as the window is in front and lets go of with it. Everywhere
//! else the page asks for a screen wake lock where the browser has one: the
//! web build in any current browser, and the system webview on iOS 18.4 and
//! later. A page loses its lock whenever it is hidden, so it asks again each
//! time it comes back into view.

use cw_core::{ContestPhase, ContestView};
use dioxus::prelude::*;

use crate::playback::Preview;
use crate::state::Screen;

/// Hold the screen on for exactly as long as the training screen is up with
/// a session on it, or a contest run on it is live, or Hear a QSO plays.
pub fn use_keep_awake(
    screen: Signal<Screen>,
    contest: Signal<Option<ContestView>>,
    preview: Signal<Preview>,
) {
    let wanted = use_memo(move || match contest.read().as_ref() {
        Some(run) => matches!(
            run.phase,
            ContestPhase::Countdown { .. } | ContestPhase::Running
        ),
        None => screen() == Screen::Training || preview.read().is_qsos(),
    });
    // What was last asked for, so starting up off asks for nothing.
    let mut held = use_hook(|| CopyValue::new(false));
    use_effect(move || {
        let on = wanted();
        if *held.peek() != on {
            held.set(on);
            keep_awake(on);
        }
    });
}

fn keep_awake(on: bool) {
    #[cfg(test)]
    tests::ASKED.with(|asked| asked.borrow_mut().push(on));
    #[cfg(all(target_os = "android", not(test)))]
    android::keep_screen_on(on);
    #[cfg(all(not(target_os = "android"), not(test)))]
    spawn(async move {
        let _ = document::eval(&wake_lock_script(on)).await;
    });
}

/// The script that takes the page's screen wake lock, or lets it go.
///
/// The lock and what is wanted of it live on the window, so each call finds
/// what the last one left. A request still on its way when the lock stops
/// being wanted lets the lock go as soon as it arrives. Every build runs what
/// it is given to eval as the body of a function, so the script may return
/// early.
#[cfg_attr(target_os = "android", allow(dead_code))]
fn wake_lock_script(on: bool) -> String {
    format!(
        r#"const held = window.__dustWake || (window.__dustWake = {{ want: false, lock: null, asking: false, watching: false }});
held.want = {on};
if (!("wakeLock" in navigator)) {{
    return;
}}
const ask = async () => {{
    if (!held.want || held.lock || held.asking || document.visibilityState !== "visible") {{
        return;
    }}
    held.asking = true;
    try {{
        const lock = await navigator.wakeLock.request("screen");
        lock.addEventListener("release", () => {{
            if (held.lock === lock) {{
                held.lock = null;
            }}
        }});
        held.lock = lock;
        if (!held.want) {{
            held.lock = null;
            lock.release().catch(() => {{}});
        }}
    }} catch (_) {{
        // Refused — a battery saver, a policy, a page not in front. The
        // next return to the page asks again.
    }} finally {{
        held.asking = false;
    }}
}};
if (!held.watching) {{
    held.watching = true;
    document.addEventListener("visibilitychange", ask);
}}
if (held.want) {{
    ask();
}} else if (held.lock) {{
    const lock = held.lock;
    held.lock = null;
    lock.release().catch(() => {{}});
}}"#
    )
}

#[cfg(all(target_os = "android", not(test)))]
mod android {
    use dioxus::mobile::wry::prelude::{JNIEnv, JObject, dispatch, jni};

    /// `WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON`.
    const FLAG_KEEP_SCREEN_ON: i32 = 0x0000_0080;

    /// Set or clear the flag on the activity's window. A window belongs to
    /// the thread that made it, so this runs there, on the activity's own
    /// thread.
    pub fn keep_screen_on(on: bool) {
        dispatch(move |env, activity, _webview| {
            if set_flag(env, activity, on).is_err() {
                // A Java exception left pending would fail every JNI call
                // after it, the webview's own among them.
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_clear();
                }
            }
        });
    }

    fn set_flag(env: &mut JNIEnv, activity: &JObject, on: bool) -> jni::errors::Result<()> {
        let window = env
            .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
            .l()?;
        let method = if on { "addFlags" } else { "clearFlags" };
        env.call_method(
            &window,
            method,
            "(I)V",
            &[jni::objects::JValue::Int(FLAG_KEEP_SCREEN_ON)],
        )?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;

    use super::wake_lock_script;
    use crate::audio::fake::Call;
    use crate::testing::{Ui, contest_settings, run, test_settings};

    thread_local! {
        /// Every change asked for, in order: on is true.
        pub(crate) static ASKED: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    }

    fn asked() -> Vec<bool> {
        ASKED.with(|asked| asked.borrow().clone())
    }

    /// On as the session starts, off once it is over, and nothing asked
    /// before it: the app starts with the screen left to the system.
    #[test]
    fn the_screen_stays_on_for_the_session_and_no_longer() {
        run(|| async {
            ASKED.with(|asked| asked.borrow_mut().clear());
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            assert_eq!(asked(), Vec::<bool>::new());
            ui.click("btn-start-training");
            assert_eq!(asked(), vec![true]);
            ui.click("btn-discard");
            ui.click("btn-discard-session");
            assert_eq!(ui.screen(), "home");
            assert_eq!(asked(), vec![true, false]);
        });
    }

    /// A contest run holds the screen on while it is live, and lets it go
    /// while paused, the audio given back with it.
    #[test]
    fn a_paused_run_lets_the_screen_sleep() {
        run(|| async {
            ASKED.with(|asked| asked.borrow_mut().clear());
            let (mut ui, recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert_eq!(asked(), vec![true]);
            ui.advance(500).await;
            ui.click("btn-pause");
            assert_eq!(asked(), vec![true, false]);
            assert_eq!(recorder.calls().last(), Some(&Call::Shutdown));
            ui.click("btn-resume");
            assert_eq!(asked(), vec![true, false, true]);
            ui.click("btn-end-run");
            assert_eq!(ui.screen(), "results", "nobody was worked: nothing kept");
            assert_eq!(asked(), vec![true, false, true, false]);
        });
    }

    /// Hear a QSO holds the screen on while it plays, as its lines are on
    /// the screen and a page out of sight stops it, and lets it go when it
    /// stops.
    #[test]
    fn the_screen_stays_on_while_hear_a_qso_plays() {
        run(|| async {
            ASKED.with(|asked| asked.borrow_mut().clear());
            let (mut ui, _recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-listen-to-letters");
            ui.click("btn-hear-a-qso");
            assert_eq!(asked(), vec![true]);
            ui.advance(2_000).await;
            ui.click("btn-stop");
            assert_eq!(asked(), vec![true, false]);
        });
    }

    #[test]
    fn the_wake_lock_is_asked_for_again_whenever_the_page_comes_back() {
        let on = wake_lock_script(true);
        assert!(on.contains("held.want = true;"), "{on}");
        assert!(
            on.contains(r#"navigator.wakeLock.request("screen")"#),
            "{on}"
        );
        assert!(
            on.contains(r#"document.addEventListener("visibilitychange", ask);"#),
            "{on}"
        );
        // A browser without the API is left alone.
        let missing = on
            .find(r#"if (!("wakeLock" in navigator))"#)
            .expect("the API is looked for");
        let request = on.find("navigator.wakeLock.request").expect("asked for");
        assert!(missing < request, "{on}");
        let off = wake_lock_script(false);
        assert!(off.contains("held.want = false;"), "{off}");
        assert!(off.contains("lock.release()"), "{off}");
    }
}
