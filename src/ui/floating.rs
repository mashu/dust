//! The floating window: the stream's groups over whatever else is on screen.
//!
//! A small window of its own, see-through where the desktop allows it, so the
//! groups can be checked while you work in another program. It is desktop
//! only: a browser tab and a phone app cannot put anything over other
//! programs.
//!
//! It is an ordinary window with a title bar, and it does not try to keep
//! itself on top: a Wayland client cannot, and a window that asked would be
//! at the mercy of whichever window had focus. The desktop already has the
//! control for it — Always on Top in the title bar's menu on GNOME, Keep
//! Above Others on KDE — and a window put there by the user stays there
//! whatever happens to the main one.
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
    Config::new()
        .with_menu(None)
        .with_background_color((0, 0, 0, 0))
        .with_custom_head(floating_head())
        .with_window(floating_builder())
}

/// A title bar, so the desktop's own window menu can keep it on top.
fn floating_builder() -> WindowBuilder {
    WindowBuilder::new()
        .with_title("Dust — groups")
        .with_transparent(true)
        .with_inner_size(LogicalSize::new(340.0, 150.0))
        .with_min_inner_size(LogicalSize::new(220.0, 100.0))
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
  background: rgba(30, 24, 18, 0.74);
  color: #f6efe3;
  font-family: "IBM Plex Mono", "DejaVu Sans Mono", ui-monospace, monospace;
  user-select: none;
  -webkit-user-select: none;
}
.floating-group { font-size: 2.5rem; line-height: 1.1; letter-spacing: 0.14em; white-space: nowrap; }
.floating-speed { margin-top: 0.3rem; font-size: 0.8rem; letter-spacing: 0.08em; opacity: 0.75; }
.floating-earlier { margin-top: 0.3rem; font-size: 0.85rem; letter-spacing: 0.1em; opacity: 0.5; white-space: nowrap; }
.floating-lock { position: absolute; top: 6px; right: 10px; font-size: 0.7rem; letter-spacing: 0.08em; opacity: 0.4; }
"#;

/// The floating window's own app.
#[component]
fn Floating(feed: FeedRef) -> Element {
    // What there is already, so it opens on the stream rather than blank.
    let mut view = use_signal(|| {
        feed.0
            .upgrade()
            .map(|shared| shared.view.borrow().clone())
            .unwrap_or_default()
    });
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
            if locked {
                span { class: "floating-lock", title: "Locked: unlock it from the main window", "LOCKED" }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn group(text: &str) -> HeardGroup {
        HeardGroup {
            text: text.into(),
            char_wpm: 25.0,
            effective_wpm: 18.0,
        }
    }

    /// The floating window as it first draws, over `view`.
    fn render(view: FeedView) -> String {
        let feed = Rc::new(Feed::default());
        *feed.view.borrow_mut() = view;
        let props = FloatingProps {
            feed: FeedRef(Rc::downgrade(&feed)),
        };
        let mut dom = VirtualDom::new_with_props(Floating, props);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn it_shows_the_newest_group_its_speed_and_a_few_before_it() {
        let heard = ["K1", "M2", "KM", "MK", "1M"].map(group).to_vec();
        let html = render(FeedView {
            heard,
            streaming: true,
            locked: false,
        });
        assert!(
            html.contains(r#"<div class="floating-group">1M</div>"#),
            "{html}"
        );
        assert!(html.contains("25 WPM · 18 eff."));
        // The three before the newest, newest first; the oldest has gone.
        assert!(html.contains("MK  KM  M2"), "{html}");
        assert!(!html.contains("K1"));
        assert!(!html.contains("LOCKED"));
    }

    #[test]
    fn it_says_when_the_stream_has_stopped_or_not_begun() {
        let stopped = render(FeedView {
            heard: vec![group("KM")],
            streaming: false,
            locked: false,
        });
        assert!(stopped.contains("Stopped · 25 WPM"), "{stopped}");
        let waiting = render(FeedView {
            heard: Vec::new(),
            streaming: true,
            locked: false,
        });
        assert!(waiting.contains("Listening"), "{waiting}");
        let idle = render(FeedView::default());
        assert!(idle.contains("Start a stream on Listen"), "{idle}");
    }

    /// Locked, it cannot be clicked, so it says where it is unlocked.
    #[test]
    fn a_locked_window_says_so() {
        let html = render(FeedView {
            heard: vec![group("KM")],
            streaming: true,
            locked: true,
        });
        assert!(html.contains("floating locked"), "{html}");
        assert!(html.contains("LOCKED"));
        assert!(html.contains("unlock it from the main window"));
    }

    /// Keeping it on top is the desktop's job, done from its title bar.
    #[test]
    fn it_is_a_window_the_desktop_can_keep_on_top() {
        let window = floating_builder().window;
        assert!(window.decorations, "no title bar, no window menu");
        assert!(
            !window.always_on_top,
            "the user puts it on top, not the app"
        );
        assert!(window.resizable);
    }

    #[test]
    fn its_page_does_not_wait_on_frames_and_lets_the_desktop_through() {
        let head = floating_head();
        assert!(head.contains("window.requestAnimationFrame ="));
        assert!(head.contains("background: transparent"));
    }

    #[test]
    fn a_feed_is_its_own_and_no_other() {
        let one = Rc::new(Feed::default());
        let other = Rc::new(Feed::default());
        assert!(FeedRef(Rc::downgrade(&one)) == FeedRef(Rc::downgrade(&one)));
        assert!(FeedRef(Rc::downgrade(&one)) != FeedRef(Rc::downgrade(&other)));
    }
}
