//! The floating window: the stream's groups over whatever else is on screen.
//!
//! A small window of its own, without a frame, kept above every other window
//! and see-through where the desktop allows it, so the groups can be checked
//! while you work in another program. It is desktop only: a browser tab and a
//! phone app cannot put anything over other programs.
//!
//! The two windows are separate virtual DOMs, and a signal of one does not
//! wake the other. So the main window writes what is to be shown into a
//! plain shared [`Feed`], and the floating one looks at it a few times a
//! second. It holds the feed weakly: once the main window is gone, so is the
//! feed, and the floating window closes itself rather than keep the app
//! running with nothing behind it.
//!
//! Locked, the window lets every click through to whatever is under it, so
//! it can sit over the program you are working in. It cannot be clicked
//! then, so it is unlocked from the main window.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use dioxus::desktop::{Config, DesktopService, LogicalSize, WindowBuilder, window};
use dioxus::prelude::*;

use crate::playback::HeardGroup;
use crate::state::AppCtx;
use crate::time::sleep_ms;

/// How often the floating window looks for a new group.
const FEED_POLL_MS: u32 = 100;
/// How often the main window checks the floating one is still there, for a
/// floating window closed from the keyboard rather than its own button.
const ALIVE_POLL_MS: u32 = 500;
/// Groups shown under the newest one.
const EARLIER_SHOWN: usize = 3;

/// What the floating window shows.
#[derive(Clone, Debug, Default, PartialEq)]
struct FeedView {
    /// Newest last.
    heard: Vec<HeardGroup>,
    streaming: bool,
    /// Clicks go through it.
    locked: bool,
}

#[derive(Default)]
struct Feed {
    view: RefCell<FeedView>,
}

/// The floating window's hold on the feed, compared by identity.
#[derive(Clone)]
struct FeedRef(Weak<Feed>);

impl PartialEq for FeedRef {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}

/// The main window's handle on the floating one.
#[derive(Clone, Copy, PartialEq)]
pub struct FloatingWindow {
    feed: CopyValue<Rc<Feed>>,
    window: CopyValue<Option<Weak<DesktopService>>>,
    /// Whether it is open, or on its way.
    open: Signal<bool>,
    /// Whether clicks go through it.
    locked: Signal<bool>,
}

impl FloatingWindow {
    pub fn is_open(self) -> bool {
        (self.open)()
    }

    pub fn is_locked(self) -> bool {
        (self.locked)()
    }

    /// Let clicks through it, or take them again.
    pub fn toggle_lock(mut self) {
        let locked = !*self.locked.peek();
        if let Some(shown) = self.shown() {
            // Ignoring the pointer is all the window manager needs to know:
            // a window with no input area cannot be clicked, dragged or
            // focused, and everything goes to the window underneath.
            let _ = shown.window.set_ignore_cursor_events(locked);
            self.locked.set(locked);
        }
    }

    fn shown(self) -> Option<Rc<DesktopService>> {
        self.window.peek().as_ref().and_then(Weak::upgrade)
    }

    /// Open it, or close it if it is open.
    pub fn toggle(mut self) {
        if *self.open.peek() {
            if let Some(shown) = self.shown() {
                shown.close();
            }
            self.window.set(None);
            self.open.set(false);
            self.locked.set(false);
            return;
        }
        let feed = FeedRef(Rc::downgrade(&self.feed.peek()));
        let dom = VirtualDom::new_with_props(Floating, FloatingProps { feed });
        let pending = window().new_window(dom, floating_config());
        self.open.set(true);
        spawn(async move {
            let Ok(shown) = pending.try_resolve().await else {
                self.open.set(false);
                return;
            };
            if *self.open.peek() {
                self.window.set(Some(Rc::downgrade(&shown)));
            } else {
                // Closed again before it had even opened.
                shown.close();
            }
        });
    }
}

/// The floating window's handle, kept up to date with what the stream sends.
/// Called once, from the app's root.
pub fn use_floating_window(ctx: AppCtx) -> FloatingWindow {
    let floating = use_context_provider(|| FloatingWindow {
        feed: CopyValue::new(Rc::new(Feed::default())),
        window: CopyValue::new(None),
        open: Signal::new(false),
        locked: Signal::new(false),
    });
    use_effect(move || {
        let view = FeedView {
            heard: ctx.stream_heard.read().clone(),
            streaming: ctx.preview.read().is_stream(),
            locked: (floating.locked)(),
        };
        *floating.feed.peek().view.borrow_mut() = view;
    });
    // A window closed with Alt+F4, or by the window manager, says nothing.
    use_future(move || {
        let mut floating = floating;
        async move {
            loop {
                sleep_ms(ALIVE_POLL_MS).await;
                let gone = floating
                    .window
                    .peek()
                    .as_ref()
                    .is_some_and(|shown| shown.upgrade().is_none());
                if gone {
                    floating.window.set(None);
                    floating.open.set(false);
                    floating.locked.set(false);
                }
            }
        }
    });
    floating
}

fn floating_config() -> Config {
    let builder = WindowBuilder::new()
        .with_title("Dust — groups")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top(true)
        .with_resizable(false)
        .with_inner_size(LogicalSize::new(340.0, 132.0));
    #[cfg(target_os = "linux")]
    let builder = {
        use dioxus::desktop::tao::platform::unix::WindowBuilderExtUnix;
        builder.with_skip_taskbar(true)
    };
    Config::new()
        .with_menu(None)
        .with_background_color((0, 0, 0, 0))
        .with_custom_head(floating_head())
        .with_window(builder)
}

fn floating_head() -> String {
    format!(
        "<script>{}</script><style>{}</style>",
        crate::KEEP_RUNNING_JS,
        FLOATING_CSS
    )
}

const FLOATING_CSS: &str = r#"
html, body { margin: 0; height: 100%; overflow: hidden; background: transparent; }
.floating {
  box-sizing: border-box;
  height: 100vh;
  padding: 0.6rem 1rem;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  position: relative;
  border-radius: 14px;
  background: rgba(30, 24, 18, 0.74);
  color: #f6efe3;
  font-family: "IBM Plex Mono", "DejaVu Sans Mono", ui-monospace, monospace;
  cursor: move;
  user-select: none;
  -webkit-user-select: none;
}
.floating-group { font-size: 2.5rem; line-height: 1.1; letter-spacing: 0.14em; white-space: nowrap; }
.floating-speed { margin-top: 0.3rem; font-size: 0.8rem; letter-spacing: 0.08em; opacity: 0.75; }
.floating-earlier { margin-top: 0.3rem; font-size: 0.85rem; letter-spacing: 0.1em; opacity: 0.5; white-space: nowrap; }
.floating-close {
  position: absolute; top: 4px; right: 6px;
  border: 0; background: none; color: inherit;
  font: inherit; font-size: 1rem; line-height: 1;
  padding: 4px 6px; opacity: 0.45; cursor: pointer;
}
.floating-close:hover { opacity: 1; }
.floating.locked { cursor: default; }
.floating-lock { position: absolute; top: 6px; right: 10px; font-size: 0.7rem; letter-spacing: 0.08em; opacity: 0.4; }
"#;

/// The floating window's own app.
#[component]
fn Floating(feed: FeedRef) -> Element {
    let mut view = use_signal(FeedView::default);
    use_future(move || {
        let feed = feed.clone();
        async move {
            loop {
                let Some(shared) = feed.0.upgrade() else {
                    // The main window has gone, and the app with it.
                    window().close();
                    return;
                };
                let now = shared.view.borrow().clone();
                drop(shared);
                if *view.peek() != now {
                    view.set(now);
                }
                sleep_ms(FEED_POLL_MS).await;
            }
        }
    });
    let FeedView {
        heard,
        streaming,
        locked,
    } = view();
    let latest = heard.last();
    let earlier: Vec<&str> = heard
        .iter()
        .rev()
        .skip(1)
        .take(EARLIER_SHOWN)
        .map(|group| group.text.as_str())
        .collect();
    let earlier = earlier.join("  ");
    rsx! {
        div {
            class: if locked { "floating locked" } else { "floating" },
            onmousedown: move |_| {
                if !locked {
                    window().drag();
                }
            },
            if locked {
                span { class: "floating-lock", title: "Locked: unlock it from the main window", "LOCKED" }
            } else {
                button {
                    class: "floating-close",
                    title: "Close",
                    onmousedown: move |event| event.stop_propagation(),
                    onclick: move |_| window().close(),
                    "×"
                }
            }
            match (latest, streaming) {
                (Some(group), _) => rsx! {
                    div { class: "floating-group", "{group.text}" }
                    div { class: "floating-speed",
                        if streaming { "{group.speed_label()}" } else { "Stopped · {group.speed_label()}" }
                    }
                    if !earlier.is_empty() {
                        div { class: "floating-earlier", "{earlier}" }
                    }
                },
                (None, true) => rsx! {
                    div { class: "floating-group", "·−" }
                    div { class: "floating-speed", "Listening…" }
                },
                (None, false) => rsx! {
                    div { class: "floating-group", "·−" }
                    div { class: "floating-speed", "Start a stream on Listen" }
                },
            }
        }
    }
}
