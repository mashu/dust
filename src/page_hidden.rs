//! Hears the page go out of sight: a tab left, a phone locked, a window
//! minimised. Whatever plays to the learner must not play on to nobody: a
//! session of groups and a contest run pause, and the band preview, the
//! letters, a sample and Hear a QSO stop, and so does a phone's stream of
//! groups. And an edit to the settings still waiting to be stored is
//! stored, as the page may never come back.
//!
//! And hears it come back into sight: the history is brought in line with
//! what another copy of the app stored meanwhile, and what the store had
//! no room for is tried again ([`use_page_shown`]).
//!
//! Each screen that listens keeps its listener on the page under a slot of
//! its own, so that one screen taking its listener off, as it goes, never
//! takes another's.

use dioxus::prelude::*;

/// Where the run screen, and the Settings and Listen screens, which play
/// the previews, keep their listeners on the page; the app itself, which
/// stores the settings; and the paddles, which open when the page goes, as
/// their key releases go with it.
pub const RUN_HIDDEN: &str = "__dustRunHidden";
pub const SESSION_HIDDEN: &str = "__dustSessionHidden";
pub const PADDLES_HIDDEN: &str = "__dustPaddlesHidden";
pub const SETTINGS_HIDDEN: &str = "__dustSettingsHidden";
pub const LISTEN_HIDDEN: &str = "__dustListenHidden";
pub const APP_HIDDEN: &str = "__dustAppHidden";
/// Where the app keeps its listener for the page coming back into sight.
pub const APP_SHOWN: &str = "__dustAppShown";

/// While the calling component is mounted, `on_hidden` hears each time the
/// page goes out of sight, through the listener kept at `slot`.
pub fn use_page_hidden(slot: &'static str, mut on_hidden: impl FnMut() + 'static) {
    let on_hidden = use_callback(move |()| on_hidden());
    listen_visibility(slot, on_hidden, true);
    listen_minimised(on_hidden);
}

/// While the calling component is mounted, `on_shown` hears each time the
/// page comes back into sight, through the listener kept at `slot`: a tab
/// shown again, a phone unlocked or the app brought back, a window given
/// the focus again.
pub fn use_page_shown(slot: &'static str, mut on_shown: impl FnMut() + 'static) {
    let on_shown = use_callback(move |()| on_shown());
    listen_visibility(slot, on_shown, false);
    listen_focused(on_shown);
}

/// The script that says whether the page is hidden, each time that
/// changes, through the listener kept at `slot`. A listener left there
/// before is taken off first.
fn visibility_script(slot: &str) -> String {
    format!(
        r#"{}
const tell = () => dioxus.send(document.hidden);
document.addEventListener("visibilitychange", tell);
window.{slot} = () => document.removeEventListener("visibilitychange", tell);"#,
        stop_watching(slot)
    )
}

/// The script that takes the listener kept at `slot` off again.
fn stop_watching(slot: &str) -> String {
    format!(
        r#"if (window.{slot}) {{
    window.{slot}();
    window.{slot} = null;
}}"#
    )
}

/// Hear the page go out of sight (`hidden` true) or come back into it
/// (`hidden` false), through the listener kept at `slot`.
fn listen_visibility(slot: &'static str, on_change: Callback<()>, hidden: bool) {
    // Tests have no page to run it on.
    #[cfg(not(test))]
    {
        use_hook(move || {
            spawn(async move {
                let mut page = document::eval(&visibility_script(slot));
                while let Ok(now_hidden) = page.recv::<bool>().await {
                    if now_hidden == hidden {
                        on_change.call(());
                    }
                }
            })
        });
        use_drop(move || {
            dioxus::core::spawn_forever(async move {
                let _ = document::eval(&stop_watching(slot)).await;
            });
        });
    }
    // A test hides the page by hand ([`hide_page`]), or shows it again
    // ([`show_page`]), and each listener hears it for as long as its screen
    // is up.
    #[cfg(test)]
    {
        let _ = slot;
        let listeners = if hidden { &LISTENERS } else { &SHOWN };
        use_hook(move || listeners.with(|all| all.borrow_mut().push(on_change)));
        use_drop(move || {
            listeners.with(|all| all.borrow_mut().retain(|heard| *heard != on_change));
        });
    }
}

#[cfg(test)]
thread_local! {
    /// The listeners on the page a test runs, for it going out of sight.
    static LISTENERS: std::cell::RefCell<Vec<Callback<()>>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// And for it coming back.
    static SHOWN: std::cell::RefCell<Vec<Callback<()>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Send the page a test runs out of sight: every listener on it hears so,
/// in the order they were put on.
#[cfg(test)]
pub fn hide_page() {
    let listeners = LISTENERS.with(|all| all.borrow().clone());
    for on_hidden in listeners {
        on_hidden.call(());
    }
}

/// Bring the page a test runs back into sight: every listener for that
/// hears so, in the order they were put on.
#[cfg(test)]
pub fn show_page() {
    let listeners = SHOWN.with(|all| all.borrow().clone());
    for on_shown in listeners {
        on_shown.call(());
    }
}

/// A minimised window is never hidden as a page, so the desktop build asks
/// the window itself whenever it is resized or loses the focus.
#[cfg(all(feature = "desktop", not(test)))]
fn listen_minimised(on_hidden: Callback<()>) {
    let desktop = use_hook(dioxus::desktop::window);
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{Event, WindowEvent};
        let Event::WindowEvent {
            window_id,
            event: WindowEvent::Resized(_) | WindowEvent::Focused(_),
            ..
        } = event
        else {
            return;
        };
        if *window_id == desktop.window.id() && desktop.window.is_minimized() {
            on_hidden.call(());
        }
    });
}

#[cfg(not(all(feature = "desktop", not(test))))]
fn listen_minimised(_on_hidden: Callback<()>) {}

/// A window brought back from the taskbar, or from behind another, is
/// never shown again as a page is, so the desktop build hears the window
/// itself take the focus.
#[cfg(all(feature = "desktop", not(test)))]
fn listen_focused(on_shown: Callback<()>) {
    let desktop = use_hook(dioxus::desktop::window);
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{Event, WindowEvent};
        if let Event::WindowEvent {
            window_id,
            event: WindowEvent::Focused(true),
            ..
        } = event
            && *window_id == desktop.window.id()
        {
            on_shown.call(());
        }
    });
}

#[cfg(not(all(feature = "desktop", not(test))))]
fn listen_focused(_on_shown: Callback<()>) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The harness never mounts anything on a page, so the script is the
    /// part a test can see: the page tells the screen each time it is
    /// hidden or shown, and the listener comes off again, one left before
    /// first. Each screen's listener is its own: another's leaves it be.
    #[test]
    fn the_page_says_when_it_is_hidden_to_each_screen_apart() {
        let script = visibility_script(RUN_HIDDEN);
        assert!(script.contains("dioxus.send(document.hidden)"), "{script}");
        assert!(
            script.contains(r#"document.addEventListener("visibilitychange", tell)"#),
            "{script}"
        );
        assert!(
            script.contains(r#"document.removeEventListener("visibilitychange", tell)"#),
            "{script}"
        );
        let off = script.find("window.__dustRunHidden();").expect("taken off");
        let on = script.find("addEventListener").expect("put on");
        assert!(off < on, "{script}");
        assert!(stop_watching(RUN_HIDDEN).contains("window.__dustRunHidden = null;"));
        let slots = [
            RUN_HIDDEN,
            SESSION_HIDDEN,
            SETTINGS_HIDDEN,
            LISTEN_HIDDEN,
            APP_HIDDEN,
            APP_SHOWN,
        ];
        for slot in slots {
            let script = visibility_script(slot);
            assert!(
                script.contains(&format!("window.{slot} = () =>")),
                "{script}"
            );
            for other in slots.into_iter().filter(|other| *other != slot) {
                assert!(!script.contains(other), "{slot}: {script}");
            }
        }
    }
}
