//! The receiver's output stage in nodes: the volume on what the receiver
//! renders, and the soft limiter over it and the paddle's sidetone, which
//! joins around the volume, set at the level it is heard at. The rest of
//! the receiver is the native player's mixer, rendered ahead
//! ([`crate::audio::stream`]).

use cw_core::band::{SOFT_LIMIT_HEADROOM, soft_limit_curve};
use web_sys::{AudioContext, GainNode};

use super::param::ramp_to;
use crate::audio::shape::GATE_RAMP_SEC;

/// Points in the soft limiter's lookup table.
const LIMITER_POINTS: usize = 4_097;

/// The fixed part of the graph, built once with the player.
pub struct Chain {
    /// The way in for what the receiver renders: the volume, on its way
    /// into the limiter [`SOFT_LIMIT_HEADROOM`] times quieter, as the
    /// shaper's input has to be.
    pub stream_in: GainNode,
    /// The way into the limiter around the volume, for the paddle's
    /// sidetone, which the native player adds after its mixer.
    pub sidetone_in: GainNode,
    volume: f64,
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
        let stream_in = gain(ctx, 1.0 / SOFT_LIMIT_HEADROOM, "volume")?;
        let sidetone_in = gain(ctx, 1.0 / SOFT_LIMIT_HEADROOM, "sidetone gain")?;
        // The native player's soft limiter, as a shaper, on everything
        // played together, so a crash under a loud pile-up is caught rather
        // than squared off.
        let limiter = ctx
            .create_wave_shaper()
            .map_err(|e| format!("limiter: {e:?}"))?;
        limiter.set_curve_opt_f32_slice(Some(&mut soft_limit_curve(LIMITER_POINTS)));
        limiter.set_oversample(web_sys::OverSampleType::N2x);
        for into in [&stream_in, &sidetone_in] {
            into.connect_with_audio_node(&limiter)
                .map_err(|e| format!("limiter connect: {e:?}"))?;
        }
        limiter
            .connect_with_audio_node(&ctx.destination())
            .map_err(|e| format!("output connect: {e:?}"))?;
        Ok(Self {
            stream_in,
            sidetone_in,
            volume: 1.0,
        })
    }

    /// Play the receiver at `volume` from now on: in a straight line at the
    /// receiver gate's pace, a whole unit over [`GATE_RAMP_SEC`], as the
    /// native mixer moves it, so a slider moved under a tone never clicks.
    pub fn set_volume(&mut self, ctx: &AudioContext, volume: f64) {
        if volume == self.volume || !volume.is_finite() {
            return;
        }
        let now = ctx.current_time();
        let until = now + GATE_RAMP_SEC * (volume - self.volume).abs();
        ramp_to(
            &self.stream_in.gain(),
            (volume / SOFT_LIMIT_HEADROOM) as f32,
            now,
            until,
        );
        self.volume = volume;
    }
}
