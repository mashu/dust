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
}

#[cfg(feature = "desktop")]
fn launch_desktop() {
    #[cfg(target_os = "linux")]
    {
        // GTK client-side decorations draw a thick header with the window title.
        // Prefer the window manager's normal title bar instead.
        // Safety: process start, before other threads exist.
        unsafe {
            std::env::set_var("GTK_CSD", "0");
        }
    }
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            Config::new()
                .with_menu(None)
                .with_background_color((243, 234, 217, 255))
                .with_custom_head(themed_document_head())
                .with_window(
                    WindowBuilder::new()
                        .with_title("Dust")
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
