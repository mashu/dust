//! What the receiver's output looks like on a scope, right now.
//!
//! A trainer must not draw the keying. An envelope trace — the shape of the
//! dits and dahs over a second or two — can simply be read, and then you are
//! not copying morse, you are reading a bar chart.
//!
//! A scope on a short timebase has the opposite problem and so is safe: a few
//! milliseconds across the face is a fraction of one dit (a dit at 20 WPM runs
//! 60 ms), so what is on screen is the *tone*, not the message. You can see
//! that a signal is there, how strong it is, and — the useful part — whether
//! it is one station or several, because two tones close together beat against
//! each other and the trace goes lumpy in a way a single tone never does.
//!
//! That is the same reason an operator glances at a scope: not to read the
//! traffic, but to see what is on the frequency.

use crate::rng::{FastrandRng, Rng};

/// A station as the trace sees it: a pitch, and how loud it is this instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeStation {
    pub tone_hz: f64,
    pub amplitude: f64,
}

/// How much of a second the face spans.
///
/// Short enough that no keying can be read off it, long enough to hold several
/// cycles of a CW note and a beat or two between two stations.
pub const SCOPE_WINDOW_SEC: f64 = 0.008;

/// A scope stands still because it starts drawing at the same point of the
/// wave every time. Without that the trace is a new random slice each frame
/// and reads as noise however clean the signal is.
fn trigger_at(stations: &[ScopeStation], t_sec: f64) -> f64 {
    let strongest = stations
        .iter()
        .filter(|s| s.amplitude > 1e-6 && s.tone_hz > 1e-6)
        .max_by(|a, b| a.amplitude.total_cmp(&b.amplitude));
    match strongest {
        // The next upward zero crossing of the station you are copying.
        Some(station) => (t_sec * station.tone_hz).ceil() / station.tone_hz,
        None => t_sec,
    }
}

/// One frame of the trace: `points` values, nominally in -1..=1.
///
/// `noise` is the receiver's hiss, which is what you see when nobody is
/// sending — a scope with a live receiver on it is never a flat line.
pub fn scope_trace(
    stations: &[ScopeStation],
    noise: f64,
    t_sec: f64,
    points: usize,
    seed: u64,
) -> Vec<f64> {
    let points = points.max(2);
    let start = trigger_at(stations, t_sec);
    let noise = noise.clamp(0.0, 1.0);
    let mut rng = FastrandRng(seed | 1);
    (0..points)
        .map(|i| {
            let t = start + SCOPE_WINDOW_SEC * (i as f64 / (points - 1) as f64);
            let signal: f64 = stations
                .iter()
                .map(|s| s.amplitude * (std::f64::consts::TAU * s.tone_hz * t).sin())
                .sum();
            let hiss = (rng.f64() * 2.0 - 1.0) * noise;
            (signal + hiss).clamp(-1.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(tone_hz: f64, amplitude: f64) -> ScopeStation {
        ScopeStation { tone_hz, amplitude }
    }

    fn peak(values: &[f64]) -> f64 {
        values.iter().fold(0.0_f64, |acc, v| acc.max(v.abs()))
    }

    /// A live receiver with nothing on it still shows its own hiss.
    #[test]
    fn an_empty_frequency_still_shows_the_receiver_noise() {
        let quiet = scope_trace(&[], 0.0, 0.0, 160, 7);
        assert!(peak(&quiet) < 1e-9, "no noise asked for, none drawn");
        let hissing = scope_trace(&[], 0.35, 0.0, 160, 7);
        assert!(
            peak(&hissing) > 0.05,
            "a live receiver is never a flat line"
        );
    }

    /// Louder is taller, which is the one thing a scope must get right.
    #[test]
    fn a_stronger_station_draws_a_taller_trace() {
        let weak = scope_trace(&[station(600.0, 0.2)], 0.0, 0.0, 240, 3);
        let strong = scope_trace(&[station(600.0, 0.9)], 0.0, 0.0, 240, 3);
        assert!(peak(&strong) > peak(&weak) * 2.0);
    }

    /// The trace holds still between frames, or it reads as noise however
    /// clean the signal is. Triggering is what buys that.
    #[test]
    fn the_trace_stands_still_from_frame_to_frame() {
        let stations = [station(600.0, 0.8)];
        let first = scope_trace(&stations, 0.0, 0.0, 200, 1);
        // A frame later, at a time that is not a whole number of cycles away.
        let later = scope_trace(&stations, 0.0, 0.0417, 200, 1);
        let drift = first
            .iter()
            .zip(&later)
            .fold(0.0_f64, |acc, (a, b)| acc.max((a - b).abs()));
        assert!(drift < 0.02, "the trace slid by {drift:.3} between frames");
    }

    /// The point of the thing: one station is a clean note, two close together
    /// beat against each other, and the trace says so without saying a word
    /// about what either is sending.
    #[test]
    fn a_second_station_makes_the_trace_lumpy() {
        // How much the peak of each cycle varies across the face.
        let ripple = |values: &[f64]| {
            let mut highs = Vec::new();
            for window in values.windows(3) {
                if window[1] > window[0] && window[1] >= window[2] && window[1] > 0.05 {
                    highs.push(window[1]);
                }
            }
            let max = highs.iter().fold(0.0_f64, |a, v| a.max(*v));
            let min = highs.iter().fold(f64::INFINITY, |a, v| a.min(*v));
            if highs.len() < 2 {
                0.0
            } else {
                max - min
            }
        };

        let alone = scope_trace(&[station(600.0, 0.8)], 0.0, 0.0, 400, 5);
        let crowded = scope_trace(
            &[station(600.0, 0.8), station(780.0, 0.45)],
            0.0,
            0.0,
            400,
            5,
        );
        assert!(
            ripple(&crowded) > ripple(&alone) + 0.1,
            "two stations should not look like one: {:.3} against {:.3}",
            ripple(&crowded),
            ripple(&alone)
        );
    }

    /// Whatever is thrown at it, the trace stays on the face and stays finite.
    #[test]
    fn the_trace_is_always_drawable() {
        for tone in [20.0, 400.0, 600.0, 1_200.0] {
            for amplitude in [0.0, 0.5, 1.0] {
                for noise in [0.0, 0.5, 1.0] {
                    for t in [0.0, 0.37, 12.5] {
                        let values = scope_trace(
                            &[station(tone, amplitude), station(tone + 150.0, amplitude)],
                            noise,
                            t,
                            120,
                            9,
                        );
                        assert_eq!(values.len(), 120);
                        for v in values {
                            assert!(v.is_finite() && (-1.0..=1.0).contains(&v));
                        }
                    }
                }
            }
        }
    }
}
