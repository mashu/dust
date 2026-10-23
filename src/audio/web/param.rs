//! What every part of the graph does with its gains: move one smoothly from
//! wherever it is, play a curve on it, and keep a fading node connected
//! until its fade has played.

use js_sys::{Array, Float32Array};
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{AudioParam, GainNode};

/// Move `param` from wherever it is now to `to` in a straight line ending at
/// `until`. What was scheduled is cancelled and the value held where it
/// stands first, so the ramp starts from what is playing rather than
/// jumping to wherever the cancelled automation began.
pub fn ramp_to(param: &AudioParam, to: f32, now: f64, until: f64) {
    let _ = param.cancel_scheduled_values(now);
    let _ = param.set_value_at_time(param.value(), now);
    let _ = param.linear_ramp_to_value_at_time(to, until);
}

/// A curve for a gain to play: a tone's keying envelope, a band's fade, the
/// fading on a send.
///
/// Handed to the browser as a plain JS Array. `setValueCurveAtTime` reads
/// its curve as a list of numbers, and Chromium reads the typed array
/// web-sys passes several times slower than an Array of the very same
/// values: tens of microseconds a curve, for every tone of a send, on the
/// main thread just as the send starts. Every `f32` is exactly a JS number
/// and back again, so what the browser plays is the same either way.
///
/// Made once, a curve can be played on any number of gains: the browser
/// takes its own copy each time.
pub struct Curve<'a> {
    /// The values themselves, for a browser that will not take the Array.
    values: &'a [f32],
    array: Array,
}

impl<'a> Curve<'a> {
    /// `values`, copied out of the wasm into the Array the browser is
    /// handed.
    pub fn new(values: &'a [f32]) -> Self {
        Self {
            values,
            array: Array::from(&Float32Array::from(values)),
        }
    }

    /// Play the curve on `param`, over `duration` from `start`. Should the
    /// browser refuse it as an Array, the same values go again the way
    /// web-sys passes them before the curve is given up on.
    pub fn play_on(&self, param: &AudioParam, start: f64, duration: f64) -> Result<(), JsValue> {
        param
            .unchecked_ref::<CurveParam>()
            .set_value_curve_at_time_with_array(&self.array, start, duration)
            .or_else(|_| {
                param
                    .set_value_curve_at_time(&mut self.values.to_vec(), start, duration)
                    .map(|_| ())
            })
    }
}

#[wasm_bindgen]
extern "C" {
    /// An [`AudioParam`], for the one way of handing it a curve that web-sys
    /// does not bind: as a plain Array. A type of its own only because a
    /// binding cannot be added to web-sys's.
    #[wasm_bindgen(extends = AudioParam, js_name = AudioParam)]
    type CurveParam;

    /// `setValueCurveAtTime`, given its curve as an Array. Caught, so a
    /// curve the browser refuses comes back as an error to fall back from
    /// rather than an exception thrown through the wasm.
    #[wasm_bindgen(method, catch, js_class = "AudioParam", js_name = setValueCurveAtTime)]
    fn set_value_curve_at_time_with_array(
        this: &CurveParam,
        values: &Array,
        start_time: f64,
        duration: f64,
    ) -> Result<(), JsValue>;
}

/// Something in the graph that has to be taken out of it once it has gone
/// quiet.
pub trait Unplug {
    fn unplug(&self);
}

impl Unplug for GainNode {
    fn unplug(&self) {
        let _ = self.disconnect();
    }
}

/// Things fading out, each with the context time it is silent by.
///
/// They stay connected until then: taking a node out of the graph while its
/// fade is still playing is a click. Unplugged once the fade has run — a node
/// left connected goes on processing silence for the life of the page.
pub struct Retiring<T: Unplug>(Vec<(T, f64)>);

impl<T: Unplug> Retiring<T> {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn retire(&mut self, item: T, silent_at: f64) {
        self.0.push((item, silent_at));
    }

    /// Bring forward every fade still to run past `until`: `fade` moves it
    /// from `now` to then instead, and it is let go once that has run. What
    /// was to play on, a send a continuation takes over from at a moment
    /// ahead, falls quiet as soon as a stop does.
    pub fn hasten(&mut self, now: f64, until: f64, fade: impl Fn(&T, f64, f64)) {
        for (item, silent_at) in &mut self.0 {
            if *silent_at > until {
                fade(item, now, until);
                *silent_at = until;
            }
        }
    }

    /// Unplug whatever has gone quiet by `now`.
    pub fn collect(&mut self, now: f64) {
        self.0.retain(|(item, silent_at)| {
            let quiet = *silent_at <= now;
            if quiet {
                item.unplug();
            }
            !quiet
        });
    }
}
