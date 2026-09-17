#![cfg_attr(
    all(feature = "desktop", target_os = "windows", not(debug_assertions)),
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

// Two renderers both owning `main` would launch twice, or launch the wrong one
// depending on which `cfg` block returns first. They are alternatives, so say
// so here rather than find out at run time.
#[cfg(all(feature = "gpu", feature = "desktop"))]
compile_error!("`gpu` and `desktop` are two renderers for the same app: pick one");

#[cfg(feature = "desktop")]
fn themed_document_head() -> String {
    let mut head = String::from("<style>");
    head.push_str(include_str!("../assets/styles.css"));
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

fn main() {
    // Blitz owns its own window and draws through wgpu, so none of the webview
    // window configuration below applies — and the stylesheet goes in through
    // the document rather than a custom head.
    #[cfg(feature = "gpu")]
    {
        dioxus_native::launch(App);
    }
    #[cfg(feature = "desktop")]
    {
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
    // Android and iOS run the same webview stack, but the OS owns the window,
    // so there is nothing to configure — and the web build launches the same way.
    #[cfg(not(feature = "desktop"))]
    {
        dioxus::launch(App);
    }
}
