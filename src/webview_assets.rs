//! What a webview build puts in its windows beside the app: the script that
//! keeps the app running while its page is out of sight, and the desktop
//! window's icon.

/// Keeps the app's tasks running while its page is out of sight: a window
/// minimised or covered, a phone locked or another app in front. Every
/// webview build has it in the page's head, ahead of the renderer.
#[cfg(not(feature = "web"))]
pub(crate) const KEEP_RUNNING_JS: &str = include_str!("keep_running.js");

/// The Dust mark on the window. GNOME's launcher uses the `.desktop` icon;
/// this is what the title bar shows.
#[cfg(feature = "desktop")]
pub(crate) fn window_icon() -> dioxus::desktop::tao::window::Icon {
    dioxus::desktop::icon_from_memory(include_bytes!("../assets/icon.png"))
        .expect("assets/icon.png")
}
