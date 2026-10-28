//! Moving a gain smoothly from wherever it is.

use web_sys::AudioParam;

/// Move `param` from wherever it is now to `to` in a straight line ending at
/// `until`. What was scheduled is cancelled and the value held where it
/// stands first, so the ramp starts from what is playing rather than
/// jumping to wherever the cancelled automation began.
pub fn ramp_to(param: &AudioParam, to: f32, now: f64, until: f64) {
    let _ = param.cancel_scheduled_values(now);
    let _ = param.set_value_at_time(param.value(), now);
    let _ = param.linear_ramp_to_value_at_time(to, until);
}
