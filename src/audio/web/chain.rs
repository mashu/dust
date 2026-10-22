//! The receiver's output stage in nodes: where everything is mixed, the AGC
//! as a compressor, and the soft limiter last of all, with the audio-stage
//! floor's way around the compressor.

use cw_core::TrainingSettings;
use cw_core::band::{
    AGC_ATTACK_SEC, AGC_COMPRESSOR_KNEE_DB, AGC_RELEASE_SEC, FilterDesign, agc_compressor,
    agc_knee, limiter_feed, soft_limit_curve,
};
use web_sys::{AudioContext, DynamicsCompressorNode, GainNode};

/// Points in the soft limiter's lookup table.
const LIMITER_POINTS: usize = 4_097;

/// The fixed part of the graph, built once with the player.
pub struct Chain {
    /// Where the band and the Morse are mixed, on their way into the
    /// receiver's filter.
    pub mix: GainNode,
    /// Where the receiver's filter sections meet, on their way to the AGC.
    pub receiver_out: GainNode,
    agc: DynamicsCompressorNode,
    /// The gain into the limiter, which also takes back the make-up gain the
    /// compressor adds of its own accord.
    limiter_in: GainNode,
    /// The way into the limiter around the compressor: for the audio-stage
    /// floor, which the native player adds after its AGC, for the paddle's
    /// sidetone, and for your own message in a contest. None of them has
    /// make-up gain to take back.
    pub floor_in: GainNode,
}

fn gain(ctx: &AudioContext, value: f64, what: &str) -> Result<GainNode, String> {
    let node = ctx.create_gain().map_err(|e| format!("{what}: {e:?}"))?;
    node.gain()
        .set_value_at_time(value as f32, ctx.current_time())
        .map_err(|e| format!("{what} set: {e:?}"))?;
    Ok(node)
}

impl Chain {
    pub fn build(ctx: &AudioContext) -> Result<Self, String> {
        let now = ctx.current_time();
        // The receiver's filter sits where a real one does: at the end, with
        // everything already mixed into it. The native player filters the
        // send and the background separately, which comes to the same thing —
        // the filter is linear — but here they are already together. Its
        // resonators go in between `mix` and `receiver_out` once the settings
        // say what to build ([`super::receiver::Receiver::tune`]).
        let mix = gain(ctx, 1.0, "mix gain")?;
        let receiver_out = gain(ctx, 1.0, "receiver gain")?;
        // The receiver's AGC, in the one place the browser can put it: after
        // the filter, with the send and the background already together. The
        // native player works the same gain out in Rust, sample by sample,
        // from the two together; here a compressor does it. Its threshold and
        // ratio are set per band from the native AGC's knee; until then it is
        // parked where it does nothing.
        let agc = ctx
            .create_dynamics_compressor()
            .map_err(|e| format!("agc: {e:?}"))?;
        for (param, value, what) in [
            (agc.knee(), AGC_COMPRESSOR_KNEE_DB, "agc knee"),
            (agc.threshold(), 0.0, "agc threshold"),
            (agc.ratio(), 1.0, "agc ratio"),
            (agc.attack(), AGC_ATTACK_SEC, "agc attack"),
            (agc.release(), AGC_RELEASE_SEC, "agc release"),
        ] {
            param
                .set_value_at_time(value as f32, now)
                .map_err(|e| format!("{what}: {e:?}"))?;
        }
        receiver_out
            .connect_with_audio_node(&agc)
            .map_err(|e| format!("agc connect: {e:?}"))?;
        // And the native player's soft limiter, as a shaper. A compressor's
        // attack lets the first milliseconds of a crash through, and a crash
        // through a wide-open filter on a noisy band is well past full scale:
        // without this the browser squared it off where the desktop catches it.
        let feed = limiter_feed(&agc_compressor(None));
        let limiter_in = gain(ctx, feed.receiver, "limiter gain")?;
        let floor_in = gain(ctx, feed.audio_floor, "floor gain")?;
        let limiter = ctx
            .create_wave_shaper()
            .map_err(|e| format!("limiter: {e:?}"))?;
        limiter.set_curve_opt_f32_slice(Some(&mut soft_limit_curve(LIMITER_POINTS)));
        limiter.set_oversample(web_sys::OverSampleType::N2x);
        agc.connect_with_audio_node(&limiter_in)
            .map_err(|e| format!("limiter connect: {e:?}"))?;
        limiter_in
            .connect_with_audio_node(&limiter)
            .map_err(|e| format!("limiter connect: {e:?}"))?;
        floor_in
            .connect_with_audio_node(&limiter)
            .map_err(|e| format!("floor connect: {e:?}"))?;
        limiter
            .connect_with_audio_node(&ctx.destination())
            .map_err(|e| format!("mix connect: {e:?}"))?;
        Ok(Self {
            mix,
            receiver_out,
            agc,
            limiter_in,
            floor_in,
        })
    }

    /// Set the compressor to the native AGC's law for this band.
    ///
    /// The same knee the native receiver uses ([`agc_knee`]: set from the
    /// band's own floor, measured at a low rate because this runs on the main
    /// thread every time a band setting moves), the same two-to-one slope
    /// above it, attack and release to match. A compressor has no hang, so
    /// between elements it lets go a little where the native AGC holds — a
    /// decibel or two of ripple — but its static curve is the native one.
    /// With the AGC off, or nothing on the band for it to ride, it is parked
    /// where it is transparent, as the native player is with no band running.
    ///
    /// Every browser adds make-up gain after a compressor, scaled by how hard
    /// it compresses at full scale. A receiver's AGC only ever turns things
    /// down, so the gain into the limiter takes that back off — for the
    /// receiver's output, which has been through the compressor, and not for
    /// the audio-stage floor, which has not ([`limiter_feed`]).
    pub fn tune_agc(&self, ctx: &AudioContext, settings: &TrainingSettings) {
        let knee = if settings.band.has_background() {
            agc_knee(&settings.band, &FilterDesign::from_settings(settings))
        } else {
            None
        };
        let setting = agc_compressor(knee);
        let now = ctx.current_time();
        let _ = self
            .agc
            .threshold()
            .set_value_at_time(setting.threshold_db.clamp(-100.0, 0.0) as f32, now);
        let _ = self
            .agc
            .ratio()
            .set_value_at_time(setting.ratio.clamp(1.0, 20.0) as f32, now);
        let feed = limiter_feed(&setting);
        let _ = self
            .limiter_in
            .gain()
            .set_value_at_time(feed.receiver as f32, now);
        let _ = self
            .floor_in
            .gain()
            .set_value_at_time(feed.audio_floor as f32, now);
    }
}
