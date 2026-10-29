//! What a player has to rebuild or retune when the settings move, as values
//! it can compare.
//!
//! The band — its noise, its static, its stations — is a running generator,
//! built at random: building it again is a new band. The receiver it is
//! heard through, its filter and its AGC, is not the band: a rig's filter and
//! AGC act on whatever is on the air, so moving them retunes the band playing
//! ([`super::BandMixer::retuned`]) rather than drawing another. The fading is
//! part of neither: it is applied to the Morse alone, a send at a time
//! ([`super::Qsb`]), so moving the fading never touches the band.

use super::calibration::{noise_level, qrn_level};
use super::memo::Exact;
use crate::settings::{AgcDecay, BandSettings, FilterShape, STATIONS_MIN};

/// Everything the band and its receiver are built from. Two settings with
/// the same key sound the same; two whose keys are the same band
/// ([`BandKey::same_band`]) are the one band through another receiver.
///
/// Levels are as heard: a layer switched off is the same band whatever its
/// level control says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandKey {
    band: SourceKey,
    receiver: ReceiverKey,
}

/// What is on the air.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SourceKey {
    /// Whether there is a receiver running at all: a band to hear, or an AGC
    /// to ride the station on a band with nothing else on it.
    runs: bool,
    /// The side-tone range the station you copy is sent in, and the
    /// quietest it is sent, which the band's own stations keep clear of;
    /// the strength nothing to a band with none.
    side_tone_min: Exact,
    side_tone_max: Exact,
    volume_min: Option<Exact>,
    noise: Exact,
    qrn: Exact,
    stations_max: u32,
}

/// What it is heard through.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ReceiverKey {
    filter_width_hz: Exact,
    filter_shape: FilterShape,
    agc_enabled: bool,
    /// The AGC's decay; nothing with the AGC off.
    agc_decay: Option<AgcDecay>,
}

impl BandKey {
    pub fn of(band: &BandSettings) -> Self {
        Self {
            band: SourceKey {
                runs: band.runs_receiver(),
                side_tone_min: Exact(band.side_tone_min),
                side_tone_max: Exact(band.side_tone_max),
                volume_min: (band.stations_max > STATIONS_MIN).then_some(Exact(band.volume_min)),
                noise: Exact(noise_level(band)),
                qrn: Exact(qrn_level(band)),
                stations_max: band.stations_max,
            },
            receiver: ReceiverKey {
                filter_width_hz: Exact(band.filter_width_hz),
                filter_shape: band.filter_shape,
                agc_enabled: band.agc_enabled,
                agc_decay: band.agc_enabled.then_some(band.agc_decay),
            },
        }
    }

    /// Whether `other` is this band, perhaps through another receiver: one a
    /// band on the air can be retuned to, rather than built afresh.
    pub fn same_band(&self, other: &Self) -> bool {
        self.band == other.band
    }

    /// What it takes to bring a player whose band was built from `playing`,
    /// if it has one, to this key.
    pub fn change_from(&self, playing: Option<&Self>) -> BandChange {
        match playing {
            Some(playing) if playing == self => BandChange::None,
            Some(playing) if playing.same_band(self) => BandChange::Retune,
            _ => BandChange::Rebuild,
        }
    }
}

/// What the settings moving asks of the band a player has on the air.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandChange {
    /// Nothing: it sounds the same.
    None,
    /// The same band through another receiver ([`super::Receiver`]).
    Retune,
    /// Another band, or the first.
    Rebuild,
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
            |s| s.band.agc_decay = AgcDecay::Slow,
            |s| s.band.filter_width_hz = 250.0,
            |s| s.band.filter_shape = FilterShape::Soft,
        ];
        for edit in builds {
            let (changed, fading) = key(edit);
            assert!(!seen.contains(&changed), "band key repeated: {changed:?}");
            assert_eq!(fading, qsb, "{changed:?} moved the fading");
            seen.push(changed);
        }
        // The receiver's controls retune the band on the air; the rest are
        // another band.
        let retunes: Vec<fn(&mut TrainingSettings)> = vec![
            |s| s.band.agc_enabled = false,
            |s| s.band.agc_decay = AgcDecay::Slow,
            |s| s.band.filter_width_hz = 250.0,
            |s| s.band.filter_shape = FilterShape::Soft,
        ];
        for edit in retunes {
            let (changed, _) = key(edit);
            assert!(changed != band && changed.same_band(&band), "{changed:?}");
        }
        for edit in [
            (|s| s.band.side_tone_min = 333.0) as fn(&mut TrainingSettings),
            |s| s.band.qrn_enabled = false,
            |s| s.band.noise_level = 0.9,
            |s| s.band.stations_max = 3,
        ] {
            assert!(!key(edit).0.same_band(&band));
        }
        // With nothing on the band, the AGC is the whole receiver: switched
        // off, there is none to retune.
        let mut dead = base.clone();
        dead.band.noise_enabled = false;
        dead.band.qrn_enabled = false;
        let running = BandKey::of(&dead.band);
        dead.band.agc_enabled = false;
        assert!(!BandKey::of(&dead.band).same_band(&running));
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
