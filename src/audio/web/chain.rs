//! The receiver's output stage in nodes: where everything is mixed, the RF
//! gain, the AGC as a compressor, the gate that mutes the receiver while you
//! send without full break-in, the volume, and the soft limiter last of all,
//! with the audio-stage floor's way around the RF gain and the compressor
//! and the paddle's sidetone's around the volume.

use cw_core::AgcDecay;
use cw_core::TrainingSettings;
use cw_core::band::{
    AGC_ATTACK_SEC, AGC_COMPRESSOR_KNEE_DB, FilterDesign, agc_compressor,
    agc_compressor_release_sec, limiter_feed, receiver_knee, soft_limit_curve,
};
use web_sys::{AnalyserNode, AudioContext, AudioParam, DynamicsCompressorNode, GainNode};

use super::param::ramp_to;
use crate::audio::shape::{GATE_RAMP_SEC, Gains};

/// Points in the soft limiter's lookup table.
const LIMITER_POINTS: usize = 4_097;

/// How many of the latest samples each of the meter's analysers holds:
/// more than a poll of the level scope apart at any rate a browser runs.
pub const METER_WINDOW: u32 = 4_096;

/// The fixed part of the graph, built once with the player.
pub struct Chain {
    /// Where the band and the Morse are mixed, on their way into the
    /// receiver's filter.
    pub mix: GainNode,
    /// Where the receiver's filter sections meet, on their way to the AGC.
    pub receiver_out: GainNode,
    /// The RF gain, between the filter and the AGC: the filter is linear, so
    /// turning everything down after it is turning it down before.
    rf: GainNode,
    agc: DynamicsCompressorNode,
    /// The gain into the limiter, which also takes back the make-up gain the
    /// compressor adds of its own accord.
    limiter_in: GainNode,
    /// The way into the limiter around the compressor, for the audio-stage
    /// floor, which the native player adds after its AGC: no make-up gain to
    /// take back.
    pub floor_in: GainNode,
    /// The way in for your own message in a contest, which is monitored, not
    /// received: around the compressor and the AF gain, through the volume
    /// alone.
    pub monitor_in: GainNode,
    /// The way into the limiter around both volumes, for the paddle's
    /// sidetone, which the native player adds after its mixer: it is set at
    /// the level it is heard at.
    pub sidetone_in: GainNode,
    /// The AF gain on the receiver's output and `floor_in`, the volume on
    /// `monitor_in`, on their way into the limiter, and what they and the RF
    /// gain were last set to.
    received: GainNode,
    monitored: GainNode,
    gains: Gains,
    /// The receiver's gate, after the AGC: shut while you send without
    /// full break-in, as a rig that is not QSK mutes its receiver. One on
    /// the receiver's output, one on the band's audio-stage floor, which
    /// is the receiver's too; your own message goes into `monitor_in` around
    /// both, and the paddle's sidetone into `sidetone_in`.
    gate: GainNode,
    /// The band's floor goes in here, through the gate's second half.
    pub band_floor: GainNode,
    /// The level scope's taps: what reaches the AGC, after the RF gain, and
    /// what comes out of both volumes on the way to the limiter, which is
    /// [`cw_core::band::SOFT_LIMIT_HEADROOM`] times quieter than it is played.
    pub meter_input: AnalyserNode,
    pub meter_output: AnalyserNode,
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
        // Full, until the settings say otherwise.
        let rf = gain(ctx, 1.0, "rf gain")?;
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
            (
                agc.release(),
                agc_compressor_release_sec(AgcDecay::Fast),
                "agc release",
            ),
        ] {
            param
                .set_value_at_time(value as f32, now)
                .map_err(|e| format!("{what}: {e:?}"))?;
        }
        receiver_out
            .connect_with_audio_node(&rf)
            .map_err(|e| format!("rf gain connect: {e:?}"))?;
        rf.connect_with_audio_node(&agc)
            .map_err(|e| format!("agc connect: {e:?}"))?;
        // And the native player's soft limiter, as a shaper. A compressor's
        // attack lets the first milliseconds of a crash through, and a crash
        // through a wide-open filter on a noisy band is well past full scale:
        // without this the browser squared it off where the desktop catches it.
        let feed = limiter_feed(&agc_compressor(None));
        let limiter_in = gain(ctx, feed.receiver, "limiter gain")?;
        let floor_in = gain(ctx, feed.audio_floor, "floor gain")?;
        let monitor_in = gain(ctx, feed.audio_floor, "monitor gain")?;
        let sidetone_in = gain(ctx, feed.audio_floor, "sidetone gain")?;
        // The band as calibrated, until the settings say how loud.
        let received = gain(ctx, 1.0, "af gain")?;
        let monitored = gain(ctx, 1.0, "monitor volume")?;
        // Open until a send of yours shuts it.
        let gate = gain(ctx, 1.0, "gate")?;
        let band_floor = gain(ctx, 1.0, "band floor gate")?;
        let limiter = ctx
            .create_wave_shaper()
            .map_err(|e| format!("limiter: {e:?}"))?;
        limiter.set_curve_opt_f32_slice(Some(&mut soft_limit_curve(LIMITER_POINTS)));
        limiter.set_oversample(web_sys::OverSampleType::N2x);
        agc.connect_with_audio_node(&gate)
            .map_err(|e| format!("gate connect: {e:?}"))?;
        gate.connect_with_audio_node(&limiter_in)
            .map_err(|e| format!("limiter connect: {e:?}"))?;
        band_floor
            .connect_with_audio_node(&floor_in)
            .map_err(|e| format!("band floor connect: {e:?}"))?;
        limiter_in
            .connect_with_audio_node(&received)
            .map_err(|e| format!("af gain connect: {e:?}"))?;
        floor_in
            .connect_with_audio_node(&received)
            .map_err(|e| format!("floor connect: {e:?}"))?;
        monitor_in
            .connect_with_audio_node(&monitored)
            .map_err(|e| format!("monitor connect: {e:?}"))?;
        for volume in [&received, &monitored] {
            volume
                .connect_with_audio_node(&limiter)
                .map_err(|e| format!("limiter connect: {e:?}"))?;
        }
        sidetone_in
            .connect_with_audio_node(&limiter)
            .map_err(|e| format!("sidetone connect: {e:?}"))?;
        limiter
            .connect_with_audio_node(&ctx.destination())
            .map_err(|e| format!("mix connect: {e:?}"))?;
        // An analyser is always listening, connected onward or not.
        let analyser = |what: &str| -> Result<AnalyserNode, String> {
            let node = ctx
                .create_analyser()
                .map_err(|e| format!("{what}: {e:?}"))?;
            node.set_fft_size(METER_WINDOW);
            Ok(node)
        };
        let meter_input = analyser("meter input")?;
        let meter_output = analyser("meter output")?;
        rf.connect_with_audio_node(&meter_input)
            .map_err(|e| format!("meter input connect: {e:?}"))?;
        for volume in [&received, &monitored] {
            volume
                .connect_with_audio_node(&meter_output)
                .map_err(|e| format!("meter output connect: {e:?}"))?;
        }
        Ok(Self {
            mix,
            receiver_out,
            rf,
            agc,
            limiter_in,
            floor_in,
            monitor_in,
            sidetone_in,
            received,
            monitored,
            gains: Gains::UNITY,
            gate,
            band_floor,
            meter_input,
            meter_output,
        })
    }

    /// The gain the AGC is riding at right now, as a gain: one with nothing
    /// to ride.
    pub fn agc_gain(&self) -> f64 {
        10f64.powf(f64::from(self.agc.reduction()) / 20.0)
    }

    /// Play at `gains` from now on. Each moves at the receiver gate's pace,
    /// a whole unit of gain over [`GATE_RAMP_SEC`], as the native mixer
    /// moves it, so a slider moved under a tone never clicks.
    pub fn set_gains(&mut self, ctx: &AudioContext, gains: Gains) {
        let now = ctx.current_time();
        for (node, to, from) in [
            (&self.rf, gains.rf, self.gains.rf),
            (&self.received, gains.received(), self.gains.received()),
            (&self.monitored, gains.monitored(), self.gains.monitored()),
        ] {
            if to != from && to.is_finite() {
                ramp_to(
                    &node.gain(),
                    to as f32,
                    now,
                    now + GATE_RAMP_SEC * (to - from).abs(),
                );
            }
        }
        self.gains = gains;
    }

    /// The receiver's gate, both halves of it, to schedule alike.
    pub fn gates(&self) -> [AudioParam; 2] {
        [self.gate.gain(), self.band_floor.gain()]
    }

    /// Set the compressor to the native AGC's law for this band.
    ///
    /// The same knee the native receiver uses ([`receiver_knee`]: set from the
    /// band's own floor, measured at a low rate because this runs on the main
    /// thread every time a band setting moves), the same ten-to-one slope
    /// above it, attack to match and the release of the decay asked for. A
    /// compressor has no hang, so between elements it lets go a little where
    /// the native AGC holds — a decibel or two of ripple — but its static
    /// curve is the native one. Its own look-ahead, six milliseconds in every
    /// browser, catches the first element of an over as the native one does.
    /// With the AGC off it is parked where it is transparent, as the native
    /// player runs none; on, it rides the station on a band with nothing
    /// else on it as on any other.
    ///
    /// Every browser adds make-up gain after a compressor, scaled by how hard
    /// it compresses at full scale. A receiver's AGC only ever turns things
    /// down, so the gain into the limiter takes that back off — for the
    /// receiver's output, which has been through the compressor, and not for
    /// the audio-stage floor, which has not ([`limiter_feed`]).
    pub fn tune_agc(&self, ctx: &AudioContext, settings: &TrainingSettings) {
        let knee = receiver_knee(&settings.band, &FilterDesign::from_settings(settings));
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
        let release = agc_compressor_release_sec(settings.band.agc_decay);
        let _ = self.agc.release().set_value_at_time(release as f32, now);
        let feed = limiter_feed(&setting);
        let _ = self
            .limiter_in
            .gain()
            .set_value_at_time(feed.receiver as f32, now);
        for path in [&self.floor_in, &self.monitor_in, &self.sidetone_in] {
            let _ = path.gain().set_value_at_time(feed.audio_floor as f32, now);
        }
    }
}
