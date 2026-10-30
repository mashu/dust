//! Hears the app go for good, where the platform says so first: the
//! desktop window closed, the browser page left, closed or reloaded. A
//! session or run going on is ended there and then as Exit ends it, kept
//! as Exit keeps it, rather than left for the next launch to find cut off.
//! A phone takes an app back with no word, and a crash says none: then the
//! next launch keeps what the session's mark held
//! ([`crate::state::AppCtx::tell_of_cut_off`]).

use dioxus::prelude::*;

use crate::session_runtime::finish_run;
use crate::state::AppCtx;

/// While the app is up, end whatever has the training screen as the app
/// goes, as Exit would.
pub fn use_end_on_close(ctx: AppCtx) {
    let on_closed = use_callback(move |()| {
        if ctx.session_running() {
            finish_run(ctx);
        }
    });
    listen_closed(on_closed);
}

/// The desktop window's close button, or the system closing it: heard
/// before the window goes, and the store written to at once.
#[cfg(all(feature = "desktop", not(test)))]
fn listen_closed(on_closed: Callback<()>) {
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{Event, WindowEvent};
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            on_closed.call(());
        }
    });
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
