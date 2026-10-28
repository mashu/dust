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
mod keep_awake;
#[cfg(all(feature = "desktop", target_os = "linux"))]
mod linux_webview;
mod page_hidden;
mod persist;
mod playback;
mod player;
mod routes;
mod session_runtime;
mod state;
#[cfg(test)]
mod testing;
mod theme;
mod time;
mod ui;

use crate::app::App;

/// Keeps the app's tasks running while the window is minimised or covered.
#[cfg(feature = "desktop")]
pub(crate) const KEEP_RUNNING_JS: &str = include_str!("keep_running.js");

#[cfg(feature = "desktop")]
fn themed_document_head() -> String {
    let mut head = String::from("<script>");
    head.push_str(KEEP_RUNNING_JS);
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
                .with_menu(None)
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

fn main() {
    #[cfg(feature = "desktop")]
    {
        launch_desktop();
    }
    // Android, iOS and the web build still use a webview the OS owns.
    #[cfg(not(feature = "desktop"))]
    {
        dioxus::launch(App);
    }
}
