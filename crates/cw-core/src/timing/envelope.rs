//! The keying envelope: how each tone rises and falls, and the preview of it
//! the settings screen draws.

use super::dot_seconds;
use crate::settings::TrainingSettings;

/// Envelope curve resolution. High enough that a 10 ms rise is a real curve and
/// not a two-point straight line at the speeds the trainer sends.
pub const ENVELOPE_SAMPLE_RATE: u32 = 1200;

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

pub fn build_envelope_curve(
    duration_sec: f64,
    rise_time_sec: f64,
    target_gain: f64,
    smoothing: f64,
) -> Vec<f32> {
    // No element is longer than a minute, and none is infinite: an endless
    // one would overflow the step count below rather than merely be long.
    const LONGEST_ELEMENT_SEC: f64 = 60.0;
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
    let rise = rise_time_sec.min(duration_sec / 2.0).max(0.0);
    let attack_steps = ((ENVELOPE_SAMPLE_RATE as f64) * rise).floor().max(2.0) as usize;
    let sustain_steps = ((ENVELOPE_SAMPLE_RATE as f64) * (duration_sec - 2.0 * rise).max(0.0))
        .floor()
        .max(0.0) as usize;
    let decay_steps = attack_steps;
    let total = (attack_steps + sustain_steps + decay_steps).max(2);
    let mut curve = vec![0.0f32; total];
    let mut idx = 0;
    for i in 0..attack_steps {
        let t = i as f64 / (attack_steps - 1) as f64;
        curve[idx] = (target_gain * edge_gain(t, smoothing)) as f32;
        idx += 1;
    }
    for _ in 0..sustain_steps {
        curve[idx] = target_gain as f32;
        idx += 1;
    }
    for i in 0..decay_steps {
        let t = i as f64 / (decay_steps - 1) as f64;
        if idx < curve.len() {
            curve[idx] = (target_gain * edge_gain(1.0 - t, smoothing)) as f32;
            idx += 1;
        }
    }
    curve
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
    pub rise_sec: f64,
    pub smoothing: f64,
    pub total_sec: f64,
    /// Attack length as a share of one dit. Above ~0.5 the dit never reaches full gain.
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
pub fn envelope_shape(settings: &TrainingSettings) -> EnvelopeShape {
    let wpm = settings
        .playback
        .char_wpm_max
        .max(settings.playback.char_wpm_min)
        .max(1.0);
    let dot_sec = dot_seconds(wpm);
    let dash_sec = dot_sec * 3.0;
    let rise_sec = (settings.band.steepness / 1000.0).max(0.0);
    let smoothing = settings.band.envelope_smoothing.clamp(0.0, 1.0);
    let mut points = Vec::new();
    let dit = build_envelope_curve(dot_sec, rise_sec, 1.0, smoothing);
    sample_curve(&dit, 0.0, dot_sec, &mut points);
    let dah = build_envelope_curve(dash_sec, rise_sec, 1.0, smoothing);
    sample_curve(&dah, dot_sec * 2.0, dash_sec, &mut points);
    EnvelopeShape {
        wpm,
        dot_sec,
        rise_sec,
        smoothing,
        total_sec: dot_sec * 5.0,
        rise_share_of_dit: (rise_sec / dot_sec.max(f64::EPSILON)).min(1.0),
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
        assert_eq!(shape.total_sec, shape.dot_sec * 5.0);
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
}
