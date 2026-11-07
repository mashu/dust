//! Hears the app go for good, where the platform says so first: the
//! desktop window closed, a Mac's Quit, the browser page left, closed or
//! reloaded. A session or run going on is ended there and then as Exit
//! ends it, kept as Exit keeps it, rather than left for the next launch to
//! find cut off, and the settings are stored with it, a level it earned
//! included.
//! A phone takes an app back with no word, and a crash says none: then the
//! next launch keeps what the session's mark held
//! ([`crate::state::AppCtx::tell_of_cut_off`]).

use dioxus::prelude::*;

use crate::app::store_settings_now;
use crate::session_runtime::finish_run;
use crate::state::AppCtx;

/// While the app is up, end whatever has the training screen as the app
/// goes, as Exit would, and store the settings as they are then.
pub fn use_end_on_close(ctx: AppCtx) {
    let on_closed = use_callback(move |()| {
        if ctx.session_running() {
            finish_run(ctx);
        }
        // What the end moved in the settings, a level earned or a run's
        // speed, goes to the store with the counters the end stored, as
        // does an edit still waiting on the settings screen. The app's
        // settings effect would store them only after this: a closed
        // desktop window is gone by then, and a page left may be.
        store_settings_now(ctx);
        // The history is written on a thread of its own, which the app's
        // going would cut short: what the end kept is waited for.
        ctx.flush_history();
    });
    listen_closed(on_closed);
}

/// The desktop window's close button, or the system closing it: heard
/// before the window goes, and the store written to at once. A Mac's
/// Quit closes no window first: heard as the event loop ends, the window
/// and the app in it still up.
#[cfg(all(feature = "desktop", not(test)))]
fn listen_closed(on_closed: Callback<()>) {
    let mut heard = false;
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        if heard_going(event, &mut heard) {
            on_closed.call(());
        }
    });
}

/// Whether `event` tells of the app going, and is the first word of it
/// since `heard` was last false: its window asked to close, or its event
/// loop ending. A Mac's Quit, from its menu, Cmd+Q, the Dock or a log-out,
/// asks no window to close: tao hears it only as the loop ends. A window
/// closed ends the loop too, after it, so whichever comes first is the
/// one heard.
#[cfg(feature = "desktop")]
fn heard_going<T>(event: &dioxus::desktop::tao::event::Event<'_, T>, heard: &mut bool) -> bool {
    use dioxus::desktop::tao::event::{Event, WindowEvent};
    let going = matches!(
        event,
        Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } | Event::LoopDestroyed
    );
    let first = going && !*heard;
    *heard |= going;
    first
}

/// The page going, left, closed or reloaded: `pagehide` is what every
/// browser says as a page goes, a phone's included, and `localStorage`
/// takes what is written to it there and then.
#[cfg(all(feature = "web", not(test)))]
fn listen_closed(on_closed: Callback<()>) {
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let listener = use_hook(|| {
        let window = web_sys::window()?;
        let heard: Closure<dyn FnMut(web_sys::Event)> =
            Closure::new(move |_: web_sys::Event| on_closed.call(()));
        window
            .add_event_listener_with_callback("pagehide", heard.as_ref().unchecked_ref())
            .ok()?;
        Some(Rc::new((window, heard)))
    });
    use_drop(move || {
        if let Some(held) = listener.as_deref() {
            let (window, heard) = held;
            let _ = window
                .remove_event_listener_with_callback("pagehide", heard.as_ref().unchecked_ref());
        }
    });
}

/// A phone's app has no word of going.
#[cfg(not(any(feature = "desktop", feature = "web", test)))]
fn listen_closed(_on_closed: Callback<()>) {}

/// A test closes the app by hand ([`close_app`]), and the app hears it for
/// as long as it is up.
#[cfg(test)]
fn listen_closed(on_closed: Callback<()>) {
    use_hook(move || LISTENERS.with(|all| all.borrow_mut().push(on_closed)));
    use_drop(move || {
        LISTENERS.with(|all| all.borrow_mut().retain(|heard| *heard != on_closed));
    });
}

#[cfg(test)]
thread_local! {
    /// What listens for the app a test runs to go.
    static LISTENERS: std::cell::RefCell<Vec<Callback<()>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Close the app a test runs, as its window's close button or a page left
/// does: every app up hears so.
#[cfg(test)]
pub fn close_app() {
    let listeners = LISTENERS.with(|all| all.borrow().clone());
    for on_closed in listeners {
        on_closed.call(());
    }
}

#[cfg(all(test, feature = "desktop"))]
mod tests {
    use super::*;
    use dioxus::desktop::tao::event::Event;

    /// A Mac's Quit, from its menu, Cmd+Q, the Dock or a log-out, ends the
    /// event loop with no window asked to close: that is heard as the app
    /// going, as the close button is, and only the first word of it is,
    /// however many follow. The loop going round is no word of it.
    #[test]
    fn the_event_loop_ending_is_heard_as_the_app_going_once() {
        let mut heard = false;
        assert!(!heard_going(&Event::<()>::MainEventsCleared, &mut heard));
        assert!(!heard, "nothing heard yet");
        assert!(heard_going(&Event::<()>::LoopDestroyed, &mut heard));
        assert!(
            !heard_going(&Event::<()>::LoopDestroyed, &mut heard),
            "once"
        );
    }
}
