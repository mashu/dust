//! What a player has to rebuild when the settings move, as a value it can
//! compare.
//!
//! The band — its noise, its static, its stations, the receiver they are
//! heard through and the AGC riding them — is a running generator, built at
//! random: building it again is a new band, and costs the knee's measurement.
//! The fading is not part of it: it is applied to the Morse alone, a send at a
//! time ([`super::Qsb`]), so moving the fading never touches the band.

use super::calibration::{noise_level, qrn_level};
use super::memo::Exact;
use crate::settings::{BandSettings, FilterShape, STATIONS_MIN};

/// Everything the band is built from. Two settings with the same key sound
/// the same, so a player rebuilds its band only when this changes.
///
/// Levels are as heard: a layer switched off is the same band whatever its
/// level control says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandKey {
    /// The receiver: the side-tone range it has to pass, and the width and
    /// shape asked for.
    side_tone_min: Exact,
    side_tone_max: Exact,
    filter_width_hz: Exact,
    filter_shape: FilterShape,
    /// The quietest the station you are copying is sent, which the band's
    /// own stations keep clear of; nothing to a band with none.
    volume_min: Option<Exact>,
    noise: Exact,
    qrn: Exact,
    stations_max: u32,
    agc_enabled: bool,
    /// The AGC's threshold, which moves its knee; nothing with the AGC off.
    agc_threshold_db: Option<Exact>,
}

impl BandKey {
    pub fn of(band: &BandSettings) -> Self {
        Self {
            side_tone_min: Exact(band.side_tone_min),
            side_tone_max: Exact(band.side_tone_max),
            filter_width_hz: Exact(band.filter_width_hz),
            filter_shape: band.filter_shape,
            volume_min: (band.stations_max > STATIONS_MIN).then_some(Exact(band.volume_min)),
            noise: Exact(noise_level(band)),
            qrn: Exact(qrn_level(band)),
            stations_max: band.stations_max,
            agc_enabled: band.agc_enabled,
            agc_threshold_db: band.agc_enabled.then_some(Exact(band.agc_threshold_db)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::band::Qsb;
    use crate::settings::TrainingSettings;

    /// Every setting the band is built from moves its key, and nothing else
    /// does: not the fading, which plays on the Morse, nor the speed, nor
    /// the RF gain, which the player turns as the band runs.
    #[test]
    fn the_band_key_moves_with_what_the_band_is_built_from() {
        let base = TrainingSettings::default();
        let key = |edit: fn(&mut TrainingSettings)| {
            let mut changed = base.clone();
            edit(&mut changed);
            (BandKey::of(&changed.band), Qsb::of(&changed.band))
        };
        let (band, qsb) = key(|_| {});
        let mut seen = vec![band];
        let builds: Vec<fn(&mut TrainingSettings)> = vec![
            |s| s.band.side_tone_min = 333.0,
            |s| s.band.side_tone_max = 999.0,
            |s| s.band.qrn_enabled = false,
            |s| s.band.qrn_level = 0.9,
            |s| s.band.noise_enabled = false,
            |s| s.band.noise_level = 0.9,
            // The band's own traffic is capped by how many may call at once,
            // and keeps clear of the wanted station by its quietest level.
            |s| s.band.stations_max = 3,
            |s| {
                s.band.stations_max = 3;
                s.band.volume_min = 0.3;
            },
            |s| s.band.agc_enabled = false,
            |s| s.band.agc_threshold_db = 10.0,
            |s| s.band.filter_width_hz = 250.0,
            |s| s.band.filter_shape = FilterShape::Soft,
        ];
        for edit in builds {
            let (changed, fading) = key(edit);
            assert!(!seen.contains(&changed), "band key repeated: {changed:?}");
            assert_eq!(fading, qsb, "{changed:?} moved the fading");
            seen.push(changed);
        }
        let fades: Vec<fn(&mut TrainingSettings)> = vec![
            |s| s.band.qsb_enabled = false,
            |s| s.band.qsb_depth = 0.9,
            |s| s.band.qsb_rate_hz = 0.9,
        ];
        let mut fadings = vec![qsb];
        for edit in fades {
            let (changed, fading) = key(edit);
            assert_eq!(changed, band, "the fading moved the band");
            assert!(!fadings.contains(&fading), "fading repeated: {fading:?}");
            fadings.push(fading);
        }
        // Speed is not a band setting, and the pile-up's spread and level are
        // the send's, not the band's.
        for edit in [
            (|s| s.playback.char_wpm_min = 40.0) as fn(&mut TrainingSettings),
            |s| s.band.pileup_spread_hz = 500.0,
            |s| s.band.pileup_level_db = 3.0,
            |s| s.band.rf_gain_db = -12.0,
        ] {
            assert_eq!(key(edit), (band, qsb));
        }
    }

    /// A layer switched off is the same band at any level, fading switched
    /// off is the same at any depth or rate, and a band with no stations of
    /// its own has none to keep clear of the one you are copying.
    #[test]
    fn what_is_switched_off_does_not_move_a_key() {
        let mut quiet = BandSettings {
            noise_enabled: false,
            qrn_enabled: false,
            qsb_enabled: false,
            ..BandSettings::default()
        };
        let (band, qsb) = (BandKey::of(&quiet), Qsb::of(&quiet));
        quiet.noise_level = 0.9;
        quiet.qrn_level = 0.9;
        quiet.volume_min = 0.2;
        quiet.qsb_depth = 0.9;
        quiet.qsb_rate_hz = 1.0;
        assert_eq!((BandKey::of(&quiet), Qsb::of(&quiet)), (band, qsb));
        quiet.qsb_enabled = true;
        quiet.qsb_depth = 0.0;
        assert_eq!(Qsb::of(&quiet), qsb);
    }
}
