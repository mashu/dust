pub mod auto_level;
pub mod band;
pub mod envelope;
#[cfg(feature = "desktop")]
pub mod floating;
pub mod heatmap;
pub mod home;
pub mod listen;
pub mod paddle;
pub mod results;
pub mod settings;
pub mod settings_charset;
pub mod stats;
pub mod stats_detail;
pub mod tips;
pub mod training;
pub mod widgets;
#[cfg(feature = "desktop")]
mod window_menu;

#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod styles_tests;
