mod app;
mod audio;
mod engine;
mod persist;
mod routes;
mod session_runtime;
mod time;
mod ui;

use crate::app::App;

fn main() {
    #[cfg(feature = "desktop")]
    {
        use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
        dioxus::LaunchBuilder::desktop()
            .with_cfg(
                Config::new()
                    .with_menu(None)
                    .with_background_color((243, 234, 217, 255))
                    .with_window(
                        WindowBuilder::new()
                            .with_title("Dust")
                            .with_inner_size(LogicalSize::new(560.0, 860.0))
                            .with_min_inner_size(LogicalSize::new(420.0, 640.0)),
                    ),
            )
            .launch(App);
    }
    #[cfg(not(feature = "desktop"))]
    {
        dioxus::launch(App);
    }
}
