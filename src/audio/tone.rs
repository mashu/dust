//! What the paddle's sidetone is asked to play, by whichever player plays
//! it: one request, and one rule for its pitch, its level and its edges.

use cw_core::band::SOFT_LIMIT_KNEE;
use cw_core::{
    SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ, STEEPNESS_MAX_MS, STEEPNESS_MIN_MS, TrainingSettings,
};

/// How loud the paddle's sidetone plays at full volume: a level of its own,
/// as a rig's sidetone has, whatever the band and its stations are doing.
/// Half the limiter's knee, about 6 dB under the loudest station the
/// receiver plays, so keying over one is heard without driving the two
/// together hard into the limiter.
pub const SIDETONE_LEVEL: f64 = SOFT_LIMIT_KNEE * 0.5;

/// The sidetone's edges are this much later than the instants they were
/// keyed at, over and above the longest stretch the output renders at
/// once: the time a late timer has to hand the player an edge before it
/// is due. Every edge is the same lead late, so each element and space
/// sounds as long as it was keyed.
#[cfg(any(feature = "native-audio", feature = "web"))]
pub const SIDETONE_LEAD_MARGIN_SEC: f64 = 0.002;

/// What the paddle's sidetone is asked to play, held to what either player
/// will play: a pitch inside the side-tone range the settings allow, a
/// level no louder than full scale, and an edge inside the keying
/// envelope's range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneRequest {
    pub on: bool,
    pub hz: f32,
    pub gain: f32,
    /// How long the gate takes to open or close, in seconds.
    pub rise_sec: f32,
    /// The edge's shape, as the keying envelope's ([`cw_core::edge_gain`]).
    pub smoothing: f32,
    /// How long ago the keyer meant the edge for: the player sounds it its
    /// constant lead after then, not after now.
    pub ago_sec: f64,
}

impl ToneRequest {
    /// The paddle's sidetone keyed under `settings`, going on or off at an
    /// edge `ago_sec` ago: at the sidetone's own level, turned down with the
    /// volume, the players adding it after their volume; and opening and
    /// closing as the keying envelope does ([`TrainingSettings::sidetone_rise_sec`]).
    pub fn keyed(on: bool, settings: &TrainingSettings, ago_sec: f64) -> Self {
        Self::new(
            on,
            settings.side_tone_center(),
            SIDETONE_LEVEL * settings.master_volume(),
        )
        .shaped(
            settings.sidetone_rise_sec(),
            settings.band.envelope_smoothing,
        )
        .due(ago_sec)
    }

    /// At `frequency_hz` and `gain`, now, with a 5 ms raised-cosine edge.
    pub fn new(on: bool, frequency_hz: f64, gain: f64) -> Self {
        let hz = if frequency_hz.is_finite() {
            frequency_hz.clamp(SIDE_TONE_MIN_HZ, SIDE_TONE_MAX_HZ)
        } else {
            SIDE_TONE_MIN_HZ
        };
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            on,
            hz: hz as f32,
            gain: gain as f32,
            rise_sec: 0.005,
            smoothing: 1.0,
            ago_sec: 0.0,
        }
    }

    /// With an edge `rise_sec` long, shaped `smoothing` of the way to a
    /// raised cosine.
    pub fn shaped(self, rise_sec: f64, smoothing: f64) -> Self {
        let rise_sec = if rise_sec.is_finite() {
            rise_sec.clamp(STEEPNESS_MIN_MS / 1_000.0, STEEPNESS_MAX_MS / 1_000.0)
        } else {
            STEEPNESS_MIN_MS / 1_000.0
        };
        let smoothing = if smoothing.is_finite() {
            smoothing.clamp(0.0, 1.0)
        } else {
            1.0
        };
        Self {
            rise_sec: rise_sec as f32,
            smoothing: smoothing as f32,
            ..self
        }
    }

    /// For an edge the keyer meant `ago_sec` ago. Never ahead of now: the
    /// keyer reports edges as they pass.
    pub fn due(self, ago_sec: f64) -> Self {
        Self {
            ago_sec: if ago_sec.is_finite() {
                ago_sec.max(0.0)
            } else {
                0.0
            },
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One rule for the sidetone's range on both players.
    #[test]
    fn the_sidetone_is_held_to_the_side_tone_range() {
        let tone = ToneRequest::new(true, 600.0, 0.4);
        assert_eq!((tone.hz, tone.gain), (600.0, 0.4));
        assert_eq!(
            ToneRequest::new(true, 20.0, 2.0).hz,
            SIDE_TONE_MIN_HZ as f32
        );
        assert_eq!(ToneRequest::new(true, 9e9, 2.0).gain, 1.0);
        assert_eq!(
            ToneRequest::new(true, 5_000.0, 0.1).hz,
            SIDE_TONE_MAX_HZ as f32
        );
        let nonsense = ToneRequest::new(true, f64::NAN, f64::NAN)
            .shaped(f64::NAN, f64::NAN)
            .due(f64::NAN);
        assert!(nonsense.hz.is_finite() && nonsense.gain == 0.0);
        assert_eq!((nonsense.rise_sec, nonsense.smoothing), (0.001, 1.0));
        assert_eq!(nonsense.ago_sec, 0.0);
        let shaped = ToneRequest::new(true, 600.0, 0.4)
            .shaped(9.0, -1.0)
            .due(-3.0);
        assert_eq!((shaped.rise_sec, shaped.smoothing), (0.05, 0.0));
        assert_eq!(shaped.ago_sec, 0.0, "an edge is never ahead of now");
    }

    /// The sidetone has a level of its own: Station strength, which sets
    /// how far the stations stand over the noise, does not move it; the
    /// volume turns it down with everything else; and at full volume it is
    /// half the limiter's knee, under the loudest station.
    #[test]
    fn the_sidetone_plays_at_its_own_level() {
        let mut settings = TrainingSettings::default();
        let level = |settings: &TrainingSettings| ToneRequest::keyed(true, settings, 0.0).gain;
        let full = level(&settings);
        assert_eq!(f64::from(full), (SOFT_LIMIT_KNEE * 0.5) as f32 as f64);
        for strength in [0.1, 0.7, 1.0] {
            settings.band.volume_min = strength;
            assert_eq!(level(&settings), full, "at strength {strength}");
        }
        settings.band.master_volume = 0.5;
        assert_eq!(level(&settings), (SIDETONE_LEVEL * 0.5) as f32);
    }

    /// The keyed sidetone takes the keying envelope's edge.
    #[test]
    fn the_sidetone_is_shaped_as_the_envelope_is() {
        let mut settings = TrainingSettings::default();
        settings.band.steepness = 4.0;
        settings.band.envelope_smoothing = 0.3;
        let tone = ToneRequest::keyed(false, &settings, 0.01);
        assert_eq!((tone.rise_sec, tone.smoothing), (0.004, 0.3));
        assert_eq!((tone.on, tone.ago_sec), (false, 0.01));
    }
}
