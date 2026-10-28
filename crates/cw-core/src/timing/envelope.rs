//! The keying envelope: how each tone rises and falls, and the preview of it
//! the settings screen draws.

use super::dot_seconds;
use crate::settings::TrainingSettings;

/// Points per second on a keying envelope's curve. The shortest edge the
/// settings allow, a millisecond, is then five steps: enough for its shape
/// to tell a cosine from a straight line, so the smoothing is heard at every
/// rise time.
pub const ENVELOPE_SAMPLE_RATE: u32 = 4800;

/// How far up an edge of the keying envelope stands, `progress` of the way
/// through it: a blend of a straight line and a raised cosine, `smoothing`
/// of the way to the cosine. A rise reads it forwards and a fall backwards,
/// and the paddle's sidetone opens and closes along it too, so your own
/// key sounds as the stations and your messages do.
pub fn edge_gain(progress: f64, smoothing: f64) -> f64 {
    let p = progress.clamp(0.0, 1.0);
    let cosine = (1.0 - (std::f64::consts::PI * p).cos()) / 2.0;
    p * (1.0 - smoothing) + cosine * smoothing
}

/// How long the edges of an element `keyed_sec` long are, with `gap_sec`
/// of silence after it at the least: the rise time asked for, held to the
/// element, so the tone is at full strength by the time the key comes up,
/// and to the gap, so its fall is over before the next one rises.
pub(crate) fn edge_sec(rise_time_sec: f64, keyed_sec: f64, gap_sec: f64) -> f64 {
    rise_time_sec.min(keyed_sec).min(gap_sec).max(0.0)
}

/// One tone's keying envelope: up from silence to `target_gain` over
/// `rise_time_sec`, held, and back down over the same, `duration_sec` in
/// all. Its points are [`ENVELOPE_SAMPLE_RATE`] a second, the first at the
/// tone's first instant and the last at its last, so the players, which
/// stretch it over the tone, put each edge where it is asked for to the
/// nearest point. With no room for two whole edges they meet at the top.
pub fn build_envelope_curve(
    duration_sec: f64,
    rise_time_sec: f64,
    target_gain: f64,
    smoothing: f64,
) -> Vec<f32> {
    // No element is longer than a few seconds, and none is infinite: an
    // endless one would overflow the point count below rather than merely
    // be long.
    const LONGEST_ELEMENT_SEC: f64 = 10.0;
    let rate = f64::from(ENVELOPE_SAMPLE_RATE);
    let duration_sec = if duration_sec.is_finite() {
        duration_sec.clamp(0.0, LONGEST_ELEMENT_SEC)
    } else {
        0.0
    };
    let smoothing = if smoothing.is_finite() {
        smoothing.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let rise_sec = if rise_time_sec.is_finite() {
        rise_time_sec.max(0.0)
    } else {
        0.0
    };
    let intervals = ((rate * duration_sec).round() as usize).max(2);
    let edge = ((rate * rise_sec).round() as usize).clamp(1, intervals / 2);
    (0..=intervals)
        .map(|i| {
            let into_edge = i.min(intervals - i) as f64 / edge as f64;
            (target_gain * edge_gain(into_edge, smoothing)) as f32
        })
        .collect()
}

/// One sampled point of the keying envelope: seconds from the start of the
/// preview, gain normalised to 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopePoint {
    pub t_sec: f64,
    pub gain: f64,
}

/// A dit, one symbol space, then a dah — exactly the shape the audio backends
/// apply, so the settings screen can draw what the ear is about to hear.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvelopeShape {
    pub wpm: f64,
    pub dot_sec: f64,
    /// The dit's edges as it plays them: the rise time, held to the dit.
    pub rise_sec: f64,
    pub smoothing: f64,
    /// From the dit's key-down to the end of the dah's fall.
    pub total_sec: f64,
    /// The rise as a share of one dit. At one the dit is at full gain only
    /// for the instant its key comes up; past a half it is less than half
    /// the time.
    pub rise_share_of_dit: f64,
    pub points: Vec<EnvelopePoint>,
}

const ENVELOPE_PREVIEW_POINTS: usize = 96;

fn sample_curve(curve: &[f32], start_sec: f64, duration_sec: f64, out: &mut Vec<EnvelopePoint>) {
    if curve.len() < 2 || duration_sec <= 0.0 {
        return;
    }
    let last = curve.len() - 1;
    let step = (curve.len() / ENVELOPE_PREVIEW_POINTS).max(1);
    let mut i = 0;
    while i <= last {
        out.push(EnvelopePoint {
            t_sec: start_sec + (i as f64 / last as f64) * duration_sec,
            gain: f64::from(curve[i]).clamp(0.0, 1.0),
        });
        if i == last {
            break;
        }
        i = (i + step).min(last);
    }
}

/// Keying envelope preview for the current rise time and smoothing, drawn at
/// the fastest character speed in the settings — the worst case for clicks.
/// Each element rises from its key-down and falls from its key-up, as the
/// planner keys it, so the dah's fall runs on past its five dits.
pub fn envelope_shape(settings: &TrainingSettings) -> EnvelopeShape {
    let wpm = settings
        .playback
        .char_wpm_max
        .max(settings.playback.char_wpm_min)
        .max(1.0);
    let dot_sec = dot_seconds(wpm);
    let dash_sec = dot_sec * 3.0;
    let rise = (settings.band.steepness / 1000.0).max(0.0);
    let smoothing = settings.band.envelope_smoothing.clamp(0.0, 1.0);
    let (dit_edge, dah_edge) = (
        edge_sec(rise, dot_sec, dot_sec),
        edge_sec(rise, dash_sec, dot_sec),
    );
    let mut points = Vec::new();
    let dit = build_envelope_curve(dot_sec + dit_edge, dit_edge, 1.0, smoothing);
    sample_curve(&dit, 0.0, dot_sec + dit_edge, &mut points);
    let dah = build_envelope_curve(dash_sec + dah_edge, dah_edge, 1.0, smoothing);
    sample_curve(&dah, dot_sec * 2.0, dash_sec + dah_edge, &mut points);
    EnvelopeShape {
        wpm,
        dot_sec,
        rise_sec: dit_edge,
        smoothing,
        total_sec: dot_sec * 5.0 + dah_edge,
        rise_share_of_dit: (rise / dot_sec.max(f64::EPSILON)).min(1.0),
        points,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_starts_and_ends_silent() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_max = 20.0;
        settings.band.steepness = 8.0;
        for smoothing in [0.0, 0.5, 1.0] {
            settings.band.envelope_smoothing = smoothing;
            let shape = envelope_shape(&settings);
            assert!(shape.points.len() > 8);
            assert!(shape.points.first().unwrap().gain.abs() < 1e-6);
            assert!(shape.points.last().unwrap().gain.abs() < 1e-6);
            assert!(shape.points.iter().any(|p| p.gain > 0.99));
            assert!(
                shape
                    .points
                    .windows(2)
                    .all(|w| w[1].t_sec >= w[0].t_sec - 1e-12)
            );
            assert!(shape.points.last().unwrap().t_sec <= shape.total_sec + 1e-9);
        }
    }

    #[test]
    fn rise_time_shortens_the_full_gain_plateau() {
        let mut fast = TrainingSettings::default();
        fast.playback.char_wpm_max = 20.0;
        fast.band.steepness = 2.0;
        let mut slow = fast.clone();
        slow.band.steepness = 20.0;
        let plateau = |s: &TrainingSettings| {
            envelope_shape(s)
                .points
                .iter()
                .filter(|p| p.t_sec <= envelope_shape(s).dot_sec && p.gain > 0.98)
                .count()
        };
        assert!(plateau(&fast) > plateau(&slow));
        assert!(envelope_shape(&slow).rise_share_of_dit > envelope_shape(&fast).rise_share_of_dit);
    }

    #[test]
    fn an_envelope_survives_nonsense_lengths() {
        for duration in [f64::INFINITY, f64::NAN, -1.0, 1e12] {
            let curve = build_envelope_curve(duration, 0.005, 0.3, f64::NAN);
            assert!(
                curve.len() >= 2 && curve.len() <= 80_000,
                "{duration}: {} steps",
                curve.len()
            );
            assert!(curve.iter().all(|v| v.is_finite()), "{duration}");
        }
    }

    #[test]
    fn the_envelope_preview_is_a_dit_then_a_dah() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.band.steepness = 10.0;
        let shape = envelope_shape(&settings);
        assert_eq!(shape.wpm, 20.0);
        assert!((shape.dot_sec - 0.06).abs() < 1e-9);
        // To the end of the dah's fall, an edge past its key-up.
        assert!((shape.total_sec - (shape.dot_sec * 5.0 + 0.01)).abs() < 1e-9);
        assert!(shape.rise_share_of_dit > 0.0 && shape.rise_share_of_dit <= 1.0);
        assert!(shape.points.len() > 10);
        // The trace starts and ends silent, and never leaves the unit range.
        assert_eq!(shape.points.first().map(|p| p.gain), Some(0.0));
        assert_eq!(shape.points.last().map(|p| p.gain), Some(0.0));
        assert!(shape.points.iter().all(|p| (0.0..=1.0).contains(&p.gain)));
        // The dah starts two dits in.
        assert!(
            shape
                .points
                .iter()
                .any(|p| (p.t_sec - shape.dot_sec * 2.0).abs() < 1e-9)
        );
    }

    #[test]
    fn a_rise_longer_than_the_dit_still_draws_a_shape() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 60.0;
        settings.playback.char_wpm_max = 60.0;
        settings.band.steepness = 50.0;
        let shape = envelope_shape(&settings);
        assert_eq!(shape.rise_share_of_dit, 1.0);
        assert!(shape.points.iter().all(|p| p.gain.is_finite()));
    }

    #[test]
    fn an_envelope_with_no_room_for_a_ramp_is_still_two_points() {
        let curve = build_envelope_curve(0.0, 0.0, 1.0, 0.5);
        assert!(curve.len() >= 2);
        assert!(curve.iter().all(|g| g.is_finite()));
    }

    /// A curve is whole intervals of the envelope's rate from the tone's
    /// first instant to its last, so stretched over the tone its points sit
    /// where they say.
    #[test]
    fn a_curve_spans_its_tone_in_whole_steps() {
        let rate = f64::from(ENVELOPE_SAMPLE_RATE);
        for duration in [0.0, 0.000_3, 0.015, 0.06, 0.186, 1.4] {
            let curve = build_envelope_curve(duration, 0.006, 0.3, 0.75);
            let intervals = ((rate * duration).round() as usize).max(2);
            assert_eq!(curve.len() - 1, intervals, "{duration} s");
            assert_eq!(curve[0], 0.0);
            assert_eq!(curve[intervals], 0.0);
        }
    }

    /// Each edge lasts the rise asked for, to the nearest point, from a
    /// millisecond up, on dits at 20 and 40 WPM; and a millisecond is a
    /// different edge from two.
    #[test]
    fn each_edge_lasts_the_rise_asked_for() {
        for wpm in [20.0, 40.0] {
            let dit = dot_seconds(wpm);
            for ms in 1..=8 {
                let rise = f64::from(ms) / 1000.0;
                let duration = dit + rise;
                let curve = build_envelope_curve(duration, rise, 1.0, 0.75);
                let intervals = (curve.len() - 1) as f64;
                let top = curve.iter().position(|g| *g == 1.0).expect("full gain");
                let ramp = top as f64 * duration / intervals;
                assert!(
                    (ramp - rise).abs() < 0.000_15,
                    "{wpm} WPM, {ms} ms: rose in {:.3} ms",
                    ramp * 1000.0
                );
                let falls_from = curve.iter().rposition(|g| *g == 1.0).expect("full gain");
                assert_eq!(
                    curve.len() - 1 - falls_from,
                    top,
                    "the fall mirrors the rise"
                );
            }
        }
        assert_ne!(
            build_envelope_curve(0.06, 0.001, 1.0, 0.75),
            build_envelope_curve(0.06, 0.002, 1.0, 0.75)
        );
    }

    /// The smoothing shapes even the shortest edges: at a millisecond or two
    /// a straight line and a cosine are told apart.
    #[test]
    fn the_smoothing_shapes_even_the_shortest_edges() {
        for rise in [0.001, 0.002] {
            let line = build_envelope_curve(0.06, rise, 1.0, 0.0);
            let cosine = build_envelope_curve(0.06, rise, 1.0, 1.0);
            let apart = line
                .iter()
                .zip(&cosine)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(apart > 0.05, "{rise} s: the shapes differ by {apart}");
        }
    }
}
