//! The floating window: the stream's groups over whatever else is on screen.
//!
//! A small window of its own, see-through where the desktop allows it, so the
//! groups can be checked while you work in another program. It is desktop
//! only: a browser tab and a phone app cannot put anything over other
//! programs.
//!
//! It has no title bar, and it does not try to keep itself on top: a Wayland
//! client cannot, and a window that asked would be at the mercy of whichever
//! window had focus. The desktop already has the control for it — Always on
//! Top on GNOME, Keep Above Others on KDE — in the window's menu, and a
//! right-click opens that menu without a title bar to hold it. A window put
//! there by the user stays there whatever happens to the main one. Dragging
//! it moves it.
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

use dioxus::core::spawn_forever;
use dioxus::desktop::{Config, DesktopService, LogicalSize, WindowBuilder, window};
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;

use crate::playback::HeardGroup;
use crate::state::{AppCtx, in_root};
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
    /// Moved on by every open and every close. A window takes a while to
    /// open, and by the time it has, it may have been closed and opened
    /// again; this is how it finds out.
    generation: CopyValue<u64>,
}

/// What becomes of an open once the window it asked for has settled.
#[derive(Debug, PartialEq)]
enum Settled<W> {
    /// The window is the one asked for: it is the one to track.
    Track(W),
    /// The window opened after it was closed again, or after a newer open
    /// was asked for. It is closed; the window tracked now is left alone.
    Close(W),
    /// The window never opened, and nothing has been asked for since: it is
    /// not open after all.
    Failed,
    /// The window never opened, but it had been superseded anyway: whatever
    /// was asked for since stands.
    Stale,
}

/// Decide what an open asked for at generation `asked` comes to, once its
/// window has opened (`Some`) or failed to (`None`), with the handle now at
/// generation `current` and `open` as it is now.
fn settle<W>(opened: Option<W>, asked: u64, current: u64, open: bool) -> Settled<W> {
    // Every close moves the generation on, so a matching one is still open;
    // `open` is checked as well rather than left to that reasoning.
    let still_wanted = asked == current && open;
    match (opened, still_wanted) {
        (Some(window), true) => Settled::Track(window),
        (Some(window), false) => Settled::Close(window),
        (None, true) => Settled::Failed,
        (None, false) => Settled::Stale,
    }
}

impl FloatingWindow {
    /// Closed, with nothing to show yet. Its signals belong to the root
    /// scope, which outlives every screen that opens it, and is where the
    /// task that opens it runs.
    fn new() -> Self {
        in_root(|| Self {
            feed: CopyValue::new(Rc::new(Feed::default())),
            window: CopyValue::new(None),
            open: Signal::new(false),
            locked: Signal::new(false),
            generation: CopyValue::new(0),
        })
    }

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

    /// Start a new open or close, which supersedes any open still on its
    /// way. Returns the generation it starts.
    fn supersede(mut self) -> u64 {
        let next = *self.generation.peek() + 1;
        self.generation.set(next);
        next
    }

    /// Open it, or close it if it is open.
    pub fn toggle(mut self) {
        if *self.open.peek() {
            if let Some(shown) = self.shown() {
                shown.close();
            }
            self.supersede();
            self.window.set(None);
            self.open.set(false);
            self.locked.set(false);
            return;
        }
        let feed = FeedRef(Rc::downgrade(&self.feed.peek()));
        let dom = VirtualDom::new_with_props(Floating, FloatingProps { feed });
        let pending = window().new_window(dom, floating_config());
        self.open_as(async move { pending.try_resolve().await.ok() });
    }

    /// Mark it open, and settle what the open comes to once `opening` has
    /// given the window asked for, or nothing.
    fn open_as(mut self, opening: impl Future<Output = Option<Rc<DesktopService>>> + 'static) {
        let asked = self.supersede();
        self.open.set(true);
        // In the root scope, not the caller's. The button that opens it is on
        // Listen, and a task of that screen's is dropped when it is left:
        // leave Listen before the window has opened, and it would open all
        // the same, with nothing here to close or lock it. What this touches
        // is the handle's own, made in the root scope too.
        spawn_forever(async move {
            let opened = opening.await;
            // Each read in a statement of its own: a guard left alive in the
            // match below would still be held when an arm writes `open`.
            let current = *self.generation.peek();
            let open = *self.open.peek();
            match settle(opened, asked, current, open) {
                Settled::Track(shown) => self.window.set(Some(Rc::downgrade(&shown))),
                // Closed again, or opened again, before it had even opened.
                Settled::Close(shown) => shown.close(),
                Settled::Failed => self.open.set(false),
                Settled::Stale => {}
            }
        });
    }
}

/// The floating window's handle, kept up to date with what the stream sends.
/// Called once, from the app's root.
pub fn use_floating_window(ctx: AppCtx) -> FloatingWindow {
    let floating = use_context_provider(FloatingWindow::new);
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

/// No title bar: the window's menu is a right-click away instead.
fn floating_builder() -> WindowBuilder {
    WindowBuilder::new()
        .with_title("Dust — groups")
        .with_window_icon(Some(crate::window_icon()))
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_inner_size(LogicalSize::new(340.0, 132.0))
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
.floating.locked { cursor: default; }
.floating-close {
  position: absolute; top: 4px; right: 6px;
  border: 0; background: none; color: inherit;
  font: inherit; font-size: 1rem; line-height: 1;
  padding: 4px 6px; opacity: 0; cursor: pointer;
  transition: opacity 0.15s;
}
.floating:hover .floating-close { opacity: 0.45; }
.floating-close:hover { opacity: 1 !important; }
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
            title: if locked { "" } else { "Drag to move · right-click for Always on Top" },
            onmousedown: move |event| {
                if !locked && event.trigger_button() == Some(MouseButton::Primary) {
                    window().drag();
                }
            },
            oncontextmenu: move |event| {
                event.prevent_default();
                if !locked {
                    let client = event.client_coordinates();
                    let screen = event.screen_coordinates();
                    super::window_menu::show(
                        &window(),
                        super::window_menu::Press {
                            x: client.x,
                            y: client.y,
                            x_root: screen.x,
                            y_root: screen.y,
                        },
                    );
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::{Ui, run};
    use crate::ui::widgets::control_id;

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
        assert!(html.contains("floating-close"));
        assert!(html.contains("right-click for Always on Top"));
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
        assert!(!html.contains("floating-close"));
    }

    /// Keeping it on top is the desktop's job, done from the window's menu,
    /// which a right-click opens: there is no title bar to hold it.
    #[test]
    fn it_has_no_title_bar_and_leaves_on_top_to_the_user() {
        let window = floating_builder().window;
        assert!(!window.decorations);
        assert!(
            !window.always_on_top,
            "the user puts it on top, not the app"
        );
    }

    #[test]
    fn its_page_does_not_wait_on_frames_and_lets_the_desktop_through() {
        let head = floating_head();
        assert!(head.contains("window.requestAnimationFrame ="));
        assert!(head.contains("background: transparent"));
    }

    #[test]
    fn a_window_that_opens_as_asked_is_the_one_tracked() {
        assert_eq!(settle(Some("window"), 1, 1, true), Settled::Track("window"));
        // A failure leaves nothing open.
        assert_eq!(settle(None::<&str>, 1, 1, true), Settled::Failed);
    }

    /// Opened, closed and opened again faster than a window can appear: the
    /// first window is no longer wanted when it arrives. It used to be
    /// tracked, then lost behind the second, and it stayed on screen with
    /// nothing in the main window able to close or lock it.
    #[test]
    fn a_window_that_opens_after_it_was_superseded_is_closed() {
        // Asked at 1, closed at 2, asked again at 3. Each settles against the
        // handle as it is when it arrives, so the order they arrive in makes
        // no difference.
        let now = 3;
        // Closing names the late window only: the one tracked is not touched.
        assert_eq!(settle(Some("first"), 1, now, true), Settled::Close("first"));
        assert_eq!(
            settle(Some("second"), 3, now, true),
            Settled::Track("second")
        );
        // Closed and not opened again.
        assert_eq!(settle(Some("first"), 1, 2, false), Settled::Close("first"));
    }

    /// A late failure of an open that was superseded must not mark the
    /// newer window as closed.
    #[test]
    fn a_superseded_open_that_fails_leaves_the_newer_one_alone() {
        // Asked at 1, closed at 2, asked again at 3: the third stays open.
        assert_eq!(settle(None::<&str>, 1, 3, true), Settled::Stale);
        // Closed since, and not asked again: it is closed either way.
        assert_eq!(settle(None::<&str>, 1, 2, false), Settled::Stale);
    }

    /// How long the window [`Listening`] asks for takes to come to nothing.
    const OPENING_MS: u32 = 200;

    /// The app's root, holding the handle, over a screen that opens the
    /// window the way Listen does and can be left.
    #[component]
    fn LeavesListen() -> Element {
        let floating = use_hook(FloatingWindow::new);
        let mut listening = use_signal(|| true);
        rsx! {
            if listening() {
                Listening { floating }
            }
            button {
                id: control_id("btn", "leave"),
                onclick: move |_| listening.set(false),
                "leave"
            }
            p { "open: {floating.is_open()}" }
        }
    }

    /// Asks for a window that fails to open a while later: no window can be
    /// made without a desktop, and an open that settles is all this needs.
    #[component]
    fn Listening(floating: FloatingWindow) -> Element {
        rsx! {
            button {
                id: control_id("btn", "float"),
                onclick: move |_| {
                    floating.open_as(async {
                        sleep_ms(OPENING_MS).await;
                        None
                    });
                },
                "float"
            }
        }
    }

    /// An open still settles after the screen that asked for it has gone.
    /// It used to go with that screen: leave Listen before the window had
    /// opened, and it opened untracked, with nothing in the main window able
    /// to close or lock it.
    #[test]
    fn an_open_settles_after_the_screen_that_asked_for_it_has_gone() {
        run(|| async {
            let mut ui = Ui::new(LeavesListen, ());
            ui.click("btn-float");
            assert!(ui.has("open: true"), "{}", ui.html());
            ui.click("btn-leave");
            assert!(!ui.shows("btn-float"));
            ui.advance(u64::from(OPENING_MS) * 2).await;
            assert!(
                ui.has("open: false"),
                "the open went with the screen: {}",
                ui.html()
            );
        });
    }

    #[test]
    fn a_feed_is_its_own_and_no_other() {
        let one = Rc::new(Feed::default());
        let other = Rc::new(Feed::default());
        assert!(FeedRef(Rc::downgrade(&one)) == FeedRef(Rc::downgrade(&one)));
        assert!(FeedRef(Rc::downgrade(&one)) != FeedRef(Rc::downgrade(&other)));
    }
}
