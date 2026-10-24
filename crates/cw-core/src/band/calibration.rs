//! What the band's levels are measured against: one station, one filter and
//! one floor, so that a decibel means the same thing in every part of it.
//!
//! The noise floor is set by its signal-to-noise ratio against a full-level
//! station in a [`REFERENCE_BANDWIDTH_HZ`] filter, as CW signal reports are
//! quoted. The static and the other stations are set against the reference
//! floor in that filter: absolute, like the station you are copying, so that
//! turning the hiss down does not take the storm or the band's traffic down
//! with it. The AGC's knee holds at what the reference floor peaks at.

use std::f64::consts::SQRT_2;

use super::dsp::db_to_gain;
use crate::settings::BandSettings;
use crate::timing::DEFAULT_TARGET_GAIN;

/// Key-down RMS of a station at full level: what every signal-to-noise figure
/// is measured against.
pub const SIGNAL_RMS: f64 = DEFAULT_TARGET_GAIN / SQRT_2;

/// The bandwidth noise levels are quoted in, as CW signal reports are.
pub const REFERENCE_BANDWIDTH_HZ: f64 = 500.0;

/// The setting of the noise control the band is calibrated against: the
/// middle of it, where the floor is a station 21 dB down in the reference
/// bandwidth. The static and the stations are set against this floor whether
/// or not the floor itself is switched on, and the AGC's knee never sits
/// lower than this floor puts it.
pub const REFERENCE_NOISE_LEVEL: f64 = 0.5;

/// Signal-to-noise in the reference bandwidth at each end of the noise
/// control. Forty decibels is a hiss you hear only between elements; two is
/// a station barely out of the noise, where copying is genuinely hard but a
/// trained ear still gets most of it.
const NOISE_SNR_QUIET_DB: f64 = 40.0;
const NOISE_SNR_LOUD_DB: f64 = 2.0;

/// How far the noise control moves for one decibel of signal-to-noise. The
/// control is linear in decibels ([`noise_floor_snr_db`]), so this is the
/// step it moves in and the levels it is held to: every step of the slider
/// is one decibel, wherever it stands.
pub const NOISE_LEVEL_PER_DB: f64 = 1.0 / (NOISE_SNR_QUIET_DB - NOISE_SNR_LOUD_DB);

/// `level` held to the nearest whole decibel of signal-to-noise, as the
/// control moves it.
pub fn whole_db_noise_level(level: f64) -> f64 {
    (level / NOISE_LEVEL_PER_DB).round() * NOISE_LEVEL_PER_DB
}

/// How loud the band's noise floor is set, from 0 to 1: nothing when it is
/// switched off, whatever its level says.
pub fn noise_level(band: &BandSettings) -> f64 {
    if band.noise_enabled {
        band.noise_level
    } else {
        0.0
    }
}

/// How bad the static is set, from 0 to 1: nothing when it is switched off.
pub fn qrn_level(band: &BandSettings) -> f64 {
    if band.qrn_enabled {
        band.qrn_level
    } else {
        0.0
    }
}

/// Signal-to-noise of the floor at `level`, in decibels in the reference
/// bandwidth.
///
/// Linear in decibels, because loudness is: every step of the slider is the
/// same audible step, rather than all the action bunched at one end.
pub fn noise_floor_snr_db(level: f64) -> f64 {
    let level = level.clamp(0.0, 1.0);
    NOISE_SNR_QUIET_DB + (NOISE_SNR_LOUD_DB - NOISE_SNR_QUIET_DB) * level
}

/// RMS of the floor at `level` through a filter [`REFERENCE_BANDWIDTH_HZ`]
/// wide. Zero means off.
pub fn noise_floor_rms(level: f64) -> f64 {
    if level.is_nan() || level <= 0.0 {
        return 0.0;
    }
    SIGNAL_RMS * db_to_gain(-noise_floor_snr_db(level))
}

/// RMS of the reference floor ([`REFERENCE_NOISE_LEVEL`]) through the
/// reference bandwidth: the level the static and the stations are set against.
pub fn reference_floor_rms() -> f64 {
    noise_floor_rms(REFERENCE_NOISE_LEVEL)
}

/// Signal-to-noise of the floor at `level` as heard through a receiver whose
/// noise bandwidth is `noise_bandwidth_hz`
/// ([`super::FilterDesign::noise_bandwidth_hz`]): the reference figure,
/// improved by however much less noise the receiver lets in. Halve the
/// width, gain 3 dB — which is the reason to reach for the filter at all.
///
/// The noise bandwidth rather than the width on the control, because that is
/// what the hiss actually comes through: the floor is white, so it comes out
/// of any filter with exactly that many hertz' worth of power, and a soft or
/// peaked filter lets in less of it than a sharp one the same width.
pub fn heard_snr_db(level: f64, noise_bandwidth_hz: f64) -> f64 {
    let bandwidth = if noise_bandwidth_hz.is_finite() && noise_bandwidth_hz > 0.0 {
        noise_bandwidth_hz
    } else {
        REFERENCE_BANDWIDTH_HZ
    };
    noise_floor_snr_db(level) + 10.0 * (REFERENCE_BANDWIDTH_HZ / bandwidth).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrower_filter_hears_a_better_signal_to_noise_ratio() {
        let reference = heard_snr_db(REFERENCE_NOISE_LEVEL, REFERENCE_BANDWIDTH_HZ);
        assert!((reference - noise_floor_snr_db(REFERENCE_NOISE_LEVEL)).abs() < 1e-12);
        assert!((heard_snr_db(REFERENCE_NOISE_LEVEL, 250.0) - reference - 3.0103).abs() < 1e-3);
        assert!(heard_snr_db(REFERENCE_NOISE_LEVEL, 2_000.0) < reference);
        assert!((heard_snr_db(REFERENCE_NOISE_LEVEL, f64::NAN) - reference).abs() < 1e-12);
        assert!(heard_snr_db(1.0, 500.0) < heard_snr_db(0.0, 500.0));
    }

    /// The reference floor is the one the control's middle makes: 21 dB
    /// under a full-level station in the reference bandwidth.
    #[test]
    fn the_reference_floor_is_a_station_21_db_down() {
        let snr = 20.0 * (SIGNAL_RMS / reference_floor_rms()).log10();
        assert!((snr - 21.0).abs() < 1e-9, "{snr} dB");
    }

    /// Out of the box the hiss is subtle: the Band card reads it as about
    /// 40 dB S/N through the receiver the defaults build.
    #[test]
    fn the_default_noise_floor_is_about_40_db_down() {
        let settings = crate::settings::TrainingSettings::default();
        let receiver = super::super::FilterDesign::from_settings(&settings);
        let snr = heard_snr_db(noise_level(&settings.band), receiver.noise_bandwidth_hz());
        assert!((40.0..41.5).contains(&snr), "{snr} dB");
    }

    /// A step of the noise control is a decibel, at either end and in the
    /// middle; held to whole decibels, a level stays put, and the reference
    /// floor is one of them.
    #[test]
    fn a_step_of_the_noise_control_is_one_decibel() {
        for level in [0.0, REFERENCE_NOISE_LEVEL, 1.0 - NOISE_LEVEL_PER_DB] {
            let step = noise_floor_snr_db(level) - noise_floor_snr_db(level + NOISE_LEVEL_PER_DB);
            assert!((step - 1.0).abs() < 1e-9, "{step} dB from {level}");
        }
        assert_eq!(
            whole_db_noise_level(REFERENCE_NOISE_LEVEL),
            REFERENCE_NOISE_LEVEL
        );
        let held = whole_db_noise_level(0.1);
        assert_eq!(whole_db_noise_level(held), held);
        assert!((noise_floor_snr_db(held) - 36.0).abs() < 1e-9);
    }

    #[test]
    fn a_switched_off_layer_is_silent_whatever_its_level() {
        let mut band = BandSettings {
            noise_level: 0.8,
            qrn_level: 0.6,
            ..BandSettings::default()
        };
        assert_eq!((noise_level(&band), qrn_level(&band)), (0.8, 0.6));
        band.noise_enabled = false;
        band.qrn_enabled = false;
        assert_eq!((noise_level(&band), qrn_level(&band)), (0.0, 0.0));
    }
}
