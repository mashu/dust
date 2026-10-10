//! Other stations on the band.
//!
//! A real CW segment is never empty. Behind the station you are copying there
//! is faint Morse at other pitches and speeds, coming and going, now and then
//! a steady carrier. Without it a background is a noise generator — which is
//! what the ear hears as wind.
//!
//! Placeholder: silent until the generator lands.

use crate::settings::TrainingSettings;

/// Background stations, as excitation at audio frequency before the receiver
/// filter. Allocation-free per sample.
#[derive(Clone, Debug)]
pub struct BandActivity {
    level: f64,
}

impl BandActivity {
    pub fn new(sample_rate: u32, level: f64, seed: u64) -> Self {
        let _ = (sample_rate, seed);
        Self {
            level: if level.is_finite() {
                level.clamp(0.0, 1.0)
            } else {
                0.0
            },
        }
    }

    pub fn from_settings(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let level = if settings.band.activity_enabled {
            settings.band.activity_level
        } else {
            0.0
        };
        Self::new(sample_rate, level, seed)
    }

    /// Silent until the generator lands, whatever the level.
    pub fn is_silent(&self) -> bool {
        let _ = self.level;
        true
    }

    pub fn next_sample(&mut self) -> f64 {
        0.0
    }
}
