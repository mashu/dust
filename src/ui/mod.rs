pub mod auto_level;
pub mod backup;
pub mod band;
pub mod chart;
pub mod contest;
pub mod credits;
pub mod envelope;
pub mod file_pick;
#[cfg(feature = "desktop")]
pub mod floating;
pub mod format;
pub mod heatmap;
pub mod home;
pub mod listen;
pub mod paddle;
pub mod privacy;
pub mod results;
pub mod scores;
pub mod settings;
pub mod settings_charset;
pub mod stats;
pub mod stats_detail;
pub mod tips;
pub mod tour;
pub mod training;
pub mod widgets;
#[cfg(feature = "desktop")]
mod window_menu;

#[cfg(test)]
mod backup_tests;
#[cfg(test)]
mod history_tests;
#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod styles_tests;
