pub mod auto_level;
pub mod backup;
pub mod band;
pub mod bigram;
pub mod chart;
pub mod confusion;
pub mod contest;
pub mod contest_keys;
pub mod contest_results;
pub mod credits;
pub mod elsewhere;
pub mod endurance;
pub mod envelope;
pub mod file_pick;
#[cfg(feature = "desktop")]
pub mod floating;
pub mod forecast;
pub mod format;
pub mod hear_qso;
pub mod heatmap;
pub mod help;
pub mod home;
pub mod letters;
pub mod listen;
pub mod paddle;
pub mod privacy;
pub mod results;
pub mod sampling;
pub mod scores;
pub mod settings;
pub mod settings_charset;
pub mod settings_station;
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
