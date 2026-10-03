#![cfg_attr(
    all(feature = "desktop", target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod audio;
mod persist;
mod playback;
mod routes;
mod session_runtime;
mod state;
#[cfg(test)]
mod testing;
mod theme;
mod time;
mod ui;

use crate::app::App;

#[cfg(feature = "desktop")]
fn themed_document_head() -> String {
    let mut head = String::from("<style>");
    head.push_str(crate::app::STYLESHEET);
    head.push_str("</style>");
    head.push_str(
        r#"<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Fraunces:opsz,wght@9..144,600;700&family=Figtree:wght@400;500;600;700&family=IBM+Plex+Mono:wght@500;700&display=optional" media="print" onload="this.media='all'">"#,
    );
    head
}

#[cfg(all(test, feature = "desktop"))]
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

#[cfg(feature = "desktop")]
fn launch_desktop() {
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
