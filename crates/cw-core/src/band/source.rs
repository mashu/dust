//! Everything the band puts into the receiver.

use super::activity::{BandActivity, Tuning};
use super::filter::FilterDesign;
use super::noise::{Atmospherics, NoiseFloor};
use crate::settings::BandSettings;

/// Everything the band puts into the receiver, before the receiver: the noise
/// floor, the static and the other stations. Both backends play this — the
/// native player runs it through its filter sample by sample, the browser
/// streams it in buffers into a filter made of nodes.
#[derive(Clone, Debug)]
pub struct BandSource {
    floor: NoiseFloor,
    atmospherics: Atmospherics,
    /// None when the only station that may call is yours.
    activity: Option<BandActivity>,
}

impl BandSource {
    /// The band `band` describes, heard through `receiver`: the stations
    /// keep clear of the one you are copying as the receiver passes them.
    /// Each layer turns `seed` into one of its own, so the same seed is the
    /// same band, and no two layers share their noise.
    pub fn new(sample_rate: u32, band: &BandSettings, receiver: &FilterDesign, seed: u64) -> Self {
        Self {
            floor: NoiseFloor::for_band(sample_rate, band, seed),
            atmospherics: Atmospherics::for_band(sample_rate, band, seed),
            activity: BandActivity::for_band(sample_rate, band, receiver, seed),
        }
    }

    /// The same band, listened to from `tuning`: a receiver turned to it in
    /// place of the one it was built for. The noise and the static are the
    /// band's whatever hears them; the stations keep on as they are.
    pub(super) fn retune(&mut self, tuning: Tuning) {
        if let Some(activity) = &mut self.activity {
            activity.retune(tuning);
        }
    }

    /// A pile-up is calling with this many stations besides yours, and they
    /// count against how many of the band's own may key at once.
    pub fn reserve_stations(&mut self, callers: usize) {
        if let Some(activity) = &mut self.activity {
            activity.reserve(callers);
        }
    }

    pub fn next_sample(&mut self) -> f64 {
        let stations = self
            .activity
            .as_mut()
            .map_or(0.0, BandActivity::next_sample);
        self.floor.next_sample() + self.atmospherics.next_sample() + stations
    }

    pub fn fill(&mut self, out: &mut [f32]) {
        for slot in out {
            *slot = self.next_sample() as f32;
        }
    }
}
