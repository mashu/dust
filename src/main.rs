#![cfg_attr(
    all(
        any(feature = "desktop", feature = "gpu", feature = "webview"),
        target_os = "windows",
        not(debug_assertions)
    ),
    windows_subsystem = "windows"
)]

mod app;
mod audio;
mod engine;
mod persist;
mod routes;
mod session_runtime;
#[cfg(test)]
mod testing;
mod theme;
mod time;
mod ui;

use crate::app::App;

// Native (wgpu) and the system webview both own `main`. They are alternatives.
#[cfg(all(any(feature = "desktop", feature = "gpu"), feature = "webview"))]
compile_error!("`desktop`/`gpu` (wgpu) and `webview` are two renderers for the same app: pick one");

#[cfg(feature = "webview")]
fn themed_document_head() -> String {
    let mut head = String::from("<style>");
    head.push_str(include_str!("../assets/styles.css"));
    head.push_str("</style>");
    head.push_str(
        r#"<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Fraunces:opsz,wght@9..144,600;700&family=Figtree:wght@400;500;600;700&family=IBM+Plex+Mono:wght@500;700&display=optional" media="print" onload="this.media='all'">"#,
    );
    head
}

#[cfg(all(test, feature = "webview"))]
mod tests {
    #[test]
    fn the_desktop_head_carries_the_stylesheet_and_the_fonts() {
        let head = super::themed_document_head();
        assert!(head.starts_with("<style>"));
        assert!(head.contains(".app-root"), "the app stylesheet is inlined");
        assert!(head.contains("fonts.googleapis.com"));
        // The font sheet must not block the first paint.
        assert!(head.contains("media=\"print\""));
    }
}

#[cfg(any(feature = "desktop", feature = "gpu"))]
fn launch_native() {
    use dioxus_native::{Config, LogicalSize, WindowAttributes};
    dioxus_native::launch_cfg(
        App,
        vec![],
        vec![Box::new(
            Config::default().with_window_attributes(
                WindowAttributes::default()
                    .with_title("Dust")
                    .with_inner_size(LogicalSize::new(560.0, 860.0)),
            ),
        )],
    );
}

#[cfg(feature = "webview")]
fn launch_webview() {
    #[cfg(target_os = "linux")]
    {
        // GTK client-side decorations draw a thick header with the window title.
        // Prefer the window manager's normal title bar instead.
        #[allow(unused_unsafe)]
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
    // Blitz paints through wgpu in-process. That is the desktop path: a
    // keystroke is layout and a GPU frame, not an IPC round-trip into a
    // webview that is sharing its thread with the answer box.
    #[cfg(any(feature = "desktop", feature = "gpu"))]
    {
        launch_native();
    }
    #[cfg(feature = "webview")]
    {
        launch_webview();
    }
    // Android, iOS and the web build still use a webview the OS owns.
    #[cfg(not(any(feature = "desktop", feature = "gpu", feature = "webview")))]
    {
        dioxus::launch(App);
    }
}
