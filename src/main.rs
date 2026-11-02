#![cfg_attr(
    all(feature = "desktop", target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

// The targets are alternatives, not layers: each brings its own runtime and
// its own audio backend. Say so plainly rather than fail later on a type
// mismatch three modules away.
#[cfg(not(any(feature = "native-audio", feature = "web", feature = "silent-audio")))]
compile_error!("Choose a build: `--features web`, `desktop`, `mobile` or `mobile-silent`.");
#[cfg(all(feature = "web", feature = "native-runtime"))]
compile_error!("`web` is the browser build; build it on its own, not with `desktop` or `mobile`.");
#[cfg(all(feature = "native-audio", feature = "silent-audio"))]
compile_error!(
    "`mobile-silent` replaces the audio backend; build it on its own, not with `desktop` or `mobile`."
);

mod app;
mod audio;
mod contest_runtime;
mod history;
mod keep_awake;
#[cfg(all(feature = "desktop", target_os = "linux"))]
mod linux_webview;
#[cfg(feature = "desktop")]
mod menus;
mod page_closed;
mod page_hidden;
mod persist;
mod playback;
mod player;
mod routes;
mod session_runtime;
mod stall_log;
mod state;
// A phone's Back and a browser's; tests press it on a page of their own.
#[cfg(any(test, feature = "web", target_os = "android"))]
mod system_back;
#[cfg(test)]
mod testing;
mod theme;
mod time;
#[cfg(test)]
mod timing_tests;
mod ui;

use crate::app::App;

/// Keeps the app's tasks running while its page is out of sight: a window
/// minimised or covered, a phone locked or another app in front. Every
/// webview build has it in the page's head, ahead of the renderer.
#[cfg(not(feature = "web"))]
pub(crate) const KEEP_RUNNING_JS: &str = include_str!("keep_running.js");

/// The paper the app is drawn on, as a webview paints it before the page
/// has loaded: the light theme's page colour, so a window or a phone's
/// screen never shows white, or black, before the app's own colours.
#[cfg(any(feature = "desktop", all(target_os = "android", not(feature = "web"))))]
const PAPER: (u8, u8, u8, u8) = (243, 234, 217, 255);

/// The app's stylesheet, inlined in a webview's head. The webview reads the
/// head before it paints anything, so the first frame, and every one after
/// it, is drawn styled however long the app takes to start; put in from the
/// app once it has mounted, as the phones used to have it, the sheet came
/// after the screen it styles, which drew raw for a moment.
#[cfg(not(feature = "web"))]
fn stylesheet_head() -> String {
    format!("<style>{}</style>", crate::app::STYLESHEET)
}

/// The type faces' sheet, linked from a webview's head, so the faces are
/// asked for as the page loads rather than once the app has mounted: they
/// are `display=optional`, used only if they arrive in time for the first
/// text. It must not hold up the first paint, which a phone with no network
/// would wait on, so it loads as print media and switches itself on once it
/// has arrived; until then, and offline, the system's faces stand in.
#[cfg(not(feature = "web"))]
fn font_head() -> String {
    format!(
        r#"<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin><link rel="stylesheet" href="{}" media="print" onload="this.media='all'">"#,
        crate::app::FONT_CSS_URL
    )
}

#[cfg(feature = "desktop")]
fn themed_document_head() -> String {
    let mut head = String::from("<script>");
    head.push_str(KEEP_RUNNING_JS);
    head.push_str(menus::FIELD_MENU_JS);
    head.push_str("</script>");
    head.push_str(&stylesheet_head());
    // WebKitGTK composites a fixed root background as a window-sized layer of
    // its own on every frame, and with compositing off it repaints the whole
    // window on every scroll step. Letting the paper scroll with the document
    // shows nowhere: .app-root covers the page, and before it mounts there is
    // nothing to scroll. Only an overscroll could reveal the difference, and
    // no-repeat keeps the paper from tiling there. It stays Linux-only: the
    // cost was measured in WebKitGTK, and macOS rubber-bands. It comes after
    // the stylesheet, which it overrides on source order alone.
    #[cfg(target_os = "linux")]
    head.push_str(
        "<style>html { background-attachment: scroll; background-repeat: no-repeat; }</style>",
    );
    head.push_str(&font_head());
    head
}

/// The phone's page head: what keeps the app running out of sight, the
/// viewport the app is laid out in, its stylesheet and its fonts, all there
/// before the first paint. The viewport is the app's own, edge to edge
/// (`viewport-fit=cover`), from the start: given only once the app had
/// mounted, an iPhone laid the first frames out inside the safe area and
/// then moved everything under the notch.
#[cfg(not(any(feature = "desktop", feature = "web")))]
fn mobile_document_head() -> String {
    let mut head = format!("<script>{KEEP_RUNNING_JS}</script>");
    head.push_str(&format!(
        r#"<meta name="viewport" content="{}">"#,
        crate::app::VIEWPORT
    ));
    head.push_str(&stylesheet_head());
    head.push_str(&font_head());
    head
}

#[cfg(all(test, not(any(feature = "desktop", feature = "web"))))]
mod mobile_tests {
    /// A phone's page out of sight must not stop the app either: Android's
    /// and iOS's webviews give a hidden page no animation frames, and a
    /// render left waiting on one held back the pause the page's going
    /// asks for.
    #[test]
    fn the_mobile_head_does_not_wait_on_frames_a_hidden_page_never_gets() {
        let head = super::mobile_document_head();
        let script = &head[..head.find("</script>").expect("a script first")];
        assert!(script.starts_with("<script>"));
        assert!(script.contains("window.requestAnimationFrame ="));
        assert!(script.contains("document.hidden"));
    }

    /// A phone draws its first frame styled: the stylesheet is in the
    /// page's head, whole, before the app mounts, and the font sheet with
    /// it, which never holds up that frame. The viewport is the app's from
    /// the start.
    #[test]
    fn the_mobile_head_carries_the_stylesheet_the_fonts_and_the_viewport() {
        let head = super::mobile_document_head();
        let sheet = format!("<style>{}</style>", crate::app::STYLESHEET);
        assert!(head.contains(&sheet), "the app stylesheet is inlined whole");
        assert!(head.contains(crate::app::FONT_CSS_URL));
        assert!(head.contains("media=\"print\""), "the fonts never block");
        assert!(head.contains(&format!(
            r#"<meta name="viewport" content="{}">"#,
            crate::app::VIEWPORT
        )));
        assert!(crate::app::VIEWPORT.contains("viewport-fit=cover"));
    }
}

#[cfg(all(test, feature = "desktop"))]
mod tests {
    #[test]
    fn the_desktop_head_carries_the_stylesheet_and_the_fonts() {
        let head = super::themed_document_head();
        assert!(head.starts_with("<script>"));
        let sheet = format!("<style>{}</style>", crate::app::STYLESHEET);
        assert!(head.contains(&sheet), "the app stylesheet is inlined whole");
        assert!(head.contains(crate::app::FONT_CSS_URL));
        // The font sheet must not block the first paint.
        assert!(head.contains("media=\"print\""));
    }

    /// A window out of sight must not stop the app: renders cannot wait on
    /// animation frames a hidden webview never gets.
    #[test]
    fn the_desktop_head_does_not_wait_on_frames_a_hidden_window_never_gets() {
        let head = super::themed_document_head();
        assert!(head.contains("window.requestAnimationFrame ="));
        assert!(head.contains("document.hidden"));
    }

    /// A field keeps its own context menu, with Paste, in a release build
    /// as in a debug one: the script that lets it is in the page's head.
    #[test]
    fn the_desktop_head_lets_a_field_keep_its_context_menu() {
        let head = super::themed_document_head();
        let script = &head[..head.find("</script>").expect("a script first")];
        assert!(script.contains(crate::menus::FIELD_MENU_JS));
    }

    /// On Linux the paper scrolls with the page instead of being composited
    /// as a fixed layer. The override has the same specificity as the
    /// stylesheet's `html` rule, so it only wins by coming after it.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_head_lets_the_paper_scroll_with_the_page() {
        let head = super::themed_document_head();
        let sheet = head
            .find(crate::app::STYLESHEET)
            .expect("the app stylesheet is inlined whole");
        let after = &head[sheet + crate::app::STYLESHEET.len()..];
        assert!(
            after.contains(
                "<style>html { background-attachment: scroll; background-repeat: no-repeat; }</style>"
            ),
            "the override follows the stylesheet"
        );
    }
}

/// The Dust mark on the window. GNOME's launcher uses the `.desktop` icon;
/// this is what the title bar shows.
#[cfg(feature = "desktop")]
pub(crate) fn window_icon() -> dioxus::desktop::tao::window::Icon {
    dioxus::desktop::icon_from_memory(include_bytes!("../assets/icon.png"))
        .expect("assets/icon.png")
}

#[cfg(feature = "desktop")]
fn launch_desktop() {
    // Safety: process start, before other threads exist.
    #[cfg(target_os = "linux")]
    unsafe {
        linux_webview::prepare();
    }
    // The history is read on a thread of its own while the window comes
    // up: only now, as the environment is set up for the webview.
    persist::preload();
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            Config::new()
                // Its own way would turn the graphics card off for every
                // Wayland session (src/linux_webview.rs).
                .with_disable_dma_buf_on_wayland(false)
                // The Mac's, with Edit; none elsewhere (src/menus.rs).
                .with_menu(menus::menu_bar())
                .with_background_color(PAPER)
                .with_custom_head(themed_document_head())
                .with_window(
                    WindowBuilder::new()
                        .with_title("Dust")
                        .with_window_icon(Some(window_icon()))
                        .with_inner_size(LogicalSize::new(560.0, 860.0))
                        .with_min_inner_size(LogicalSize::new(420.0, 640.0)),
                ),
        )
        .launch(App);
}

/// Android and iOS: the desktop's webview stack, in a window the system
/// owns.
#[cfg(not(any(feature = "desktop", feature = "web")))]
fn launch_mobile() {
    let config = dioxus::mobile::Config::new().with_custom_head(mobile_document_head());
    // Android's webview is white until the page has loaded; it is paper, as
    // the desktop window is. iOS keeps its own: wry turns the webview
    // translucent there to colour it, a change to how every frame is drawn
    // that no phone has tried.
    #[cfg(target_os = "android")]
    let config = config.with_background_color(PAPER);
    dioxus::LaunchBuilder::mobile().with_cfg(config).launch(App);
}

/// How often Android's event loop is woken whatever else wakes it: well
/// inside a session's two-second heartbeat gap, so a task held up until
/// the next kick is not taken for time the app was away.
#[cfg(any(test, target_os = "android"))]
const LOOP_KICK_MS: u64 = 250;

/// Wake the event loop every [`LOOP_KICK_MS`], on Android. Every task the
/// app has runs when Dioxus's word to poll it reaches tao's event loop, and
/// tao 0.34 lost that word whenever the looper woke for a lifecycle event
/// at the same moment: nothing ran again until something else woke the
/// loop. `vendor/tao` takes the word on every wake now
/// (docs/platforms.md, Android); this is the backstop, should a wake go
/// missing some other way, as each wake has tao take what is waiting.
///
/// `main` runs on the event loop's own thread, whose looper tao has made
/// ready before calling it.
#[cfg(all(target_os = "android", not(test)))]
fn kick_the_event_loop() {
    use dioxus::mobile::tao::platform::android::prelude::ndk::looper::ForeignLooper;

    let Some(looper) = ForeignLooper::for_thread() else {
        return;
    };
    let _ = kick_every(std::time::Duration::from_millis(LOOP_KICK_MS), move || {
        looper.wake();
        true
    });
}

/// Call `kick` every `period`, on a thread of its own, for as long as it
/// says to go on.
#[cfg(any(test, target_os = "android"))]
fn kick_every(
    period: std::time::Duration,
    kick: impl Fn() -> bool + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("dust-loop-kick".into())
        .spawn(move || {
            loop {
                std::thread::sleep(period);
                if !kick() {
                    break;
                }
            }
        })
}

fn main() {
    #[cfg(feature = "desktop")]
    {
        launch_desktop();
    }
    #[cfg(not(any(feature = "desktop", feature = "web")))]
    {
        #[cfg(all(target_os = "android", not(test)))]
        {
            kick_the_event_loop();
            stall_log::start();
        }
        // The history is read on a thread of its own while the webview
        // starts up. Android hands the app its folder before `main` runs.
        persist::preload();
        launch_mobile();
    }
    // A browser applies each render as it comes, frames or none. The
    // history is read from the browser's database first, which answers only
    // a moment later; the boot screen shows meanwhile.
    #[cfg(feature = "web")]
    {
        wasm_bindgen_futures::spawn_local(async {
            persist::preload().await;
            dioxus::launch(App);
        });
    }
}

#[cfg(test)]
mod kick_tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// The loop is kicked again and again, not once, and the kicks stop
    /// once there is nothing left to kick.
    #[test]
    fn the_event_loop_is_kicked_until_it_is_gone() {
        let (kicked, kicks) = mpsc::channel();
        let kicker = super::kick_every(Duration::from_millis(1), move || kicked.send(()).is_ok())
            .expect("a thread");
        for _ in 0..3 {
            kicks
                .recv_timeout(Duration::from_secs(10))
                .expect("kicked again");
        }
        // Gone: the next kick finds no one, and the thread ends.
        drop(kicks);
        let given_up = Instant::now() + Duration::from_secs(10);
        while !kicker.is_finished() {
            assert!(Instant::now() < given_up, "the kicks went on");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
