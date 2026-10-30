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
mod ui;

use crate::app::App;

/// Keeps the app's tasks running while its page is out of sight: a window
/// minimised or covered, a phone locked or another app in front. Every
/// webview build has it in the page's head, ahead of the renderer.
#[cfg(not(feature = "web"))]
pub(crate) const KEEP_RUNNING_JS: &str = include_str!("keep_running.js");

#[cfg(feature = "desktop")]
fn themed_document_head() -> String {
    let mut head = String::from("<script>");
    head.push_str(KEEP_RUNNING_JS);
    head.push_str(menus::FIELD_MENU_JS);
    head.push_str("</script><style>");
    head.push_str(crate::app::STYLESHEET);
    head.push_str("</style>");
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
    // The font sheet must not hold up the first paint: it loads as print
    // media and switches itself on once it has arrived.
    head.push_str(&format!(
        r#"<link rel="stylesheet" href="{}" media="print" onload="this.media='all'">"#,
        crate::app::FONT_CSS_URL
    ));
    head
}

/// The phone's page head: only what keeps the app running out of sight.
/// The stylesheet and the fonts go in from the app itself (`AppHead`).
#[cfg(not(any(feature = "desktop", feature = "web")))]
fn mobile_document_head() -> String {
    format!("<script>{KEEP_RUNNING_JS}</script>")
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
        assert!(head.starts_with("<script>"));
        assert!(head.ends_with("</script>"));
        assert!(head.contains("window.requestAnimationFrame ="));
        assert!(head.contains("document.hidden"));
    }
}

#[cfg(all(test, feature = "desktop"))]
mod tests {
    #[test]
    fn the_desktop_head_carries_the_stylesheet_and_the_fonts() {
        let head = super::themed_document_head();
        assert!(head.starts_with("<script>"));
        assert!(head.contains("<style>"));
        assert!(head.contains(".app-root"), "the app stylesheet is inlined");
        assert!(head.contains("fonts.googleapis.com"));
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
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            Config::new()
                // Its own way would turn the graphics card off for every
                // Wayland session (src/linux_webview.rs).
                .with_disable_dma_buf_on_wayland(false)
                // The Mac's, with Edit; none elsewhere (src/menus.rs).
                .with_menu(menus::menu_bar())
                .with_background_color((243, 234, 217, 255))
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
    dioxus::LaunchBuilder::mobile()
        .with_cfg(dioxus::mobile::Config::new().with_custom_head(mobile_document_head()))
        .launch(App);
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
        launch_mobile();
    }
    // A browser applies each render as it comes, frames or none.
    #[cfg(feature = "web")]
    {
        dioxus::launch(App);
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
