//! The receiver's output stage: the soft limiter everything it plays passes
//! through last, and how the browser builds the same one out of nodes.

use super::agc::CompressorSetting;
use super::dsp::db_to_gain;

/// Where [`soft_limit`] starts to bend, as a share of full scale: below it
/// whatever is played comes out exactly as it went in.
pub const SOFT_LIMIT_KNEE: f64 = 0.7;

/// What the output stage lifts the band, the Morse and your own messages by
/// at full volume, before the limiter: as far as a station goes without
/// reaching where the limiter starts to bend ([`SOFT_LIMIT_KNEE`]), through
/// any receiver.
///
/// The band is calibrated with the strongest station peaking at 0.3 of full
/// scale, and the AGC rides it some 5 dB lower: played as it is, full
/// volume came out about 15 dB under what the speaker can give, and a phone
/// at its own top volume was too quiet. A station on its own peaks highest,
/// at about 0.35, through the sharp filter at its narrowest with the AGC
/// off and the keying's edges at their hardest, where the filter overshoots
/// each one; nearly twice that, 5.6 dB, still leaves it a little under the
/// knee, so no volume and no receiver bends the Morse. What the limiter is
/// left to catch is what it always caught: crashes, and the crests of a hard
/// band or of several stations keying at once.
pub const FULL_VOLUME_GAIN: f64 = 1.9;

/// What a receiver does with a loud crash: catches it, rather than letting it
/// square off. Linear up to the knee, asymptotic to full scale above it — so
/// the biggest static is loud without turning into a buzz.
///
/// The last thing before the speaker, once, on everything played together:
/// the band, the Morse over it and the paddle's sidetone. Limited apart and
/// then added, any two of them could still square off.
pub fn soft_limit(x: f64) -> f64 {
    let magnitude = x.abs();
    if magnitude <= SOFT_LIMIT_KNEE {
        return x;
    }
    let over = (magnitude - SOFT_LIMIT_KNEE) / (1.0 - SOFT_LIMIT_KNEE);
    (SOFT_LIMIT_KNEE + (1.0 - SOFT_LIMIT_KNEE) * over.tanh()).copysign(x)
}

/// How far past full scale [`soft_limit_curve`] reaches before it simply holds
/// at its last value. Far beyond anything the band and the Morse add up to.
const SOFT_LIMIT_HEADROOM: f64 = 4.0;

/// `soft_limit` as a lookup table, for a Web Audio `WaveShaperNode`.
///
/// A shaper maps its input over -1..=1 onto the curve, so the input is to be
/// scaled down by `SOFT_LIMIT_HEADROOM` first; the curve maps that back up
/// before limiting. Between the points the shaper interpolates linearly, which
/// below the knee is exact — the limiter is a straight line there.
pub fn soft_limit_curve(points: usize) -> Vec<f32> {
    let points = points.max(2);
    let last = (points - 1) as f64;
    (0..points)
        .map(|i| {
            let x = (2.0 * i as f64 / last - 1.0) * SOFT_LIMIT_HEADROOM;
            soft_limit(x) as f32
        })
        .collect()
}

/// The gains the browser feeds its soft limiter through, for a compressor
/// set as `setting`.
///
/// The browser's limiter is a shaper whose input is scaled down by
/// `SOFT_LIMIT_HEADROOM`, so whatever reaches it has to be too. What has
/// come through the compressor also carries the make-up gain the compressor
/// adds of its own accord, which a receiver's AGC never would, so the
/// receiver's feed takes that back off. The audio-stage floor joins after the
/// compressor, as it does after the native AGC, and has nothing to take back:
/// sent through the receiver's feed it would come out quieter by the make-up
/// gain whenever the AGC is on. Either way, both reach the limiter at the
/// gain the native player gives them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterFeed {
    /// For the receiver's output, after the compressor.
    pub receiver: f64,
    /// For the [`super::AudioFloor`], which bypasses the compressor.
    pub audio_floor: f64,
}

/// The [`LimiterFeed`] for the browser's compressor set as `setting`.
pub fn limiter_feed(setting: &CompressorSetting) -> LimiterFeed {
    LimiterFeed {
        receiver: db_to_gain(-setting.makeup_db) / SOFT_LIMIT_HEADROOM,
        audio_floor: 1.0 / SOFT_LIMIT_HEADROOM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::band::agc::agc_compressor;
    use crate::band::test_support::knee_for;
    use crate::settings::TrainingSettings;

    /// However loud it gets, it is caught rather than squared off — which is
    /// what a receiver does with a crash, and the difference between loud
    /// static and a buzz.
    #[test]
    fn the_soft_limit_catches_rather_than_clips() {
        assert_eq!(soft_limit(0.0), 0.0);
        assert_eq!(soft_limit(0.5), 0.5, "below the knee it does nothing");
        assert_eq!(soft_limit(-0.5), -0.5);
        assert!(soft_limit(4.0) < 1.0 && soft_limit(4.0) > 0.9);
        assert!(soft_limit(-4.0) > -1.0 && soft_limit(-4.0) < -0.9);
        assert!(
            soft_limit(2.0) < soft_limit(3.0),
            "limiting, not clipping flat"
        );
    }

    /// The browser limits with a lookup table of the same curve, so the table
    /// has to be the curve: identical below the knee, never past full scale,
    /// always rising.
    #[test]
    fn the_soft_limit_table_is_the_soft_limit() {
        let curve = soft_limit_curve(4_097);
        assert_eq!(curve.len(), 4_097);
        assert!(
            curve.windows(2).all(|w| w[1] >= w[0]),
            "it must rise everywhere"
        );
        // At most full scale: the far ends round to exactly 1.0 in f32.
        assert!(curve.iter().all(|v| v.abs() <= 1.0));
        // The midpoint is silence, and the ends are as loud as it gets.
        assert_eq!(curve[2_048], 0.0);
        assert!(curve[4_096] > 0.99 && curve[0] < -0.99);
        // Below the knee it is the identity, once the headroom scaling is
        // undone.
        for (i, value) in curve.iter().enumerate() {
            let x = (2.0 * i as f64 / 4_096.0 - 1.0) * SOFT_LIMIT_HEADROOM;
            if x.abs() <= 0.7 {
                assert!((f64::from(*value) - x).abs() < 1e-6, "at {x}");
            }
        }
        assert_eq!(soft_limit_curve(0).len(), 2);
    }

    /// The browser's floor comes into the graph after the compressor and goes
    /// straight to the limiter, so it has none of the compressor's make-up
    /// gain to take back, and arrives at unity — as the native player adds
    /// it — whether the AGC is on or off. The receiver's output, which does
    /// carry the make-up gain, has it taken back off, and arrives at unity
    /// below the knee.
    #[test]
    fn the_browser_feeds_its_limiter_at_the_native_gains() {
        let on = knee_for(&TrainingSettings::default());
        assert!(on.is_some(), "the AGC is on by default");
        for knee in [None, on, Some(1e-4), Some(0.5)] {
            let setting = agc_compressor(knee);
            let feed = limiter_feed(&setting);
            // The shaper maps its input back up by the headroom before
            // limiting.
            let floor = feed.audio_floor * SOFT_LIMIT_HEADROOM;
            let receiver = db_to_gain(setting.makeup_db) * feed.receiver * SOFT_LIMIT_HEADROOM;
            assert!((floor - 1.0).abs() < 1e-12, "{knee:?}: floor at {floor}");
            assert!(
                (receiver - 1.0).abs() < 1e-12,
                "{knee:?}: receiver at {receiver}"
            );
        }
        // With the AGC on there is a make-up gain to keep the floor clear of:
        // through the receiver's feed it would have come out that much
        // quieter.
        let setting = agc_compressor(on);
        assert!(setting.makeup_db > 1.0, "{} dB make-up", setting.makeup_db);
        assert!(limiter_feed(&setting).receiver < limiter_feed(&setting).audio_floor);
    }
}
