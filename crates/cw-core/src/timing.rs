//! Farnsworth Morse timing and a platform-agnostic playback plan.

use crate::morse::morse_for;
use crate::rng::Rng;
use crate::settings::TrainingSettings;

pub const DEFAULT_TARGET_GAIN: f64 = 0.3;
/// Envelope curve resolution. High enough that a 10 ms rise is a real curve and
/// not a two-point straight line at the speeds the trainer sends.
pub const ENVELOPE_SAMPLE_RATE: u32 = 1200;
pub const EXTRA_SPACING_MULTIPLIER_MIN: f64 = 0.1;

pub fn clamp_extra_spacing(value: f64) -> f64 {
    value.max(EXTRA_SPACING_MULTIPLIER_MIN)
}

/// PARIS-style dot duration in seconds from character WPM.
pub fn dot_seconds(wpm: f64) -> f64 {
    1.2 / wpm.max(1.0)
}

pub fn compute_group_gap_ms(settings: &TrainingSettings) -> u32 {
    compute_group_gap_for_wpm(
        settings.playback.char_wpm_min,
        settings.playback.effective_wpm_min,
        settings.playback.extra_word_space_multiplier,
    )
}

/// Word-space gap for the WPM that was actually sent, not the settings minimum.
pub fn compute_group_gap_for_wpm(char_wpm: f64, effective_wpm: f64, extra_word_space: f64) -> u32 {
    let char_wpm = char_wpm.max(1.0);
    let effective_wpm = effective_wpm.max(1.0).min(char_wpm);
    let dot_effective_sec = 1.2 / effective_wpm;
    let word_space_sec = 7.0 * dot_effective_sec * clamp_extra_spacing(extra_word_space);
    (word_space_sec * 1000.0).round() as u32
}

#[derive(Clone, Debug)]
pub struct ToneEvent {
    pub start_sec: f64,
    pub duration_sec: f64,
    pub frequency_hz: f64,
    pub target_gain: f64,
    pub envelope: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct PlaybackPlan {
    pub events: Vec<ToneEvent>,
    pub duration_sec: f64,
    pub resolved_char_wpm: f64,
    pub resolved_effective_wpm: f64,
    pub rise_time_sec: f64,
    pub envelope_smoothing: f64,
}

fn resolve_char_wpm(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    rng.pick_in_range(
        settings.playback.char_wpm_min.max(1.0),
        settings.playback.char_wpm_max.max(1.0),
    )
    .max(1.0)
}

fn resolve_effective_wpm(settings: &TrainingSettings, char_wpm: f64, rng: &mut impl Rng) -> f64 {
    let sampled = rng
        .pick_in_range(
            settings.playback.effective_wpm_min.max(1.0),
            settings.playback.effective_wpm_max.max(1.0),
        )
        .max(1.0);
    sampled.min(char_wpm)
}

/// How many times one group is sent before the answer window opens.
pub fn resolve_group_repeats(settings: &TrainingSettings, rng: &mut impl Rng) -> u32 {
    let min = settings.playback.group_repeat_min.clamp(
        crate::settings::GROUP_REPEAT_MIN,
        crate::settings::GROUP_REPEAT_MAX,
    );
    let max = settings
        .playback
        .group_repeat_max
        .clamp(min, crate::settings::GROUP_REPEAT_MAX);
    if settings.playback.link_group_repeat || min == max {
        return min;
    }
    rng.usize_in(min as usize, max as usize) as u32
}

fn resolve_volume(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    let min = settings.band.volume_min.clamp(0.1, 1.0);
    let max = settings.band.volume_max.clamp(0.1, 1.0);
    if settings.band.link_volume || (min - max).abs() < f64::EPSILON {
        min
    } else {
        rng.pick_in_range(min, max)
    }
}

fn resolve_tone_hz(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    let min = settings.band.side_tone_min.max(100.0);
    let max = settings.band.side_tone_max.max(min);
    if (min - max).abs() < f64::EPSILON {
        min
    } else {
        rng.pick_in_range_inclusive_int(min, max)
    }
}

pub fn build_envelope_curve(
    duration_sec: f64,
    rise_time_sec: f64,
    target_gain: f64,
    smoothing: f64,
) -> Vec<f32> {
    let smoothing = smoothing.clamp(0.0, 1.0);
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
        let linear = t;
        let cosine = (1.0 - (std::f64::consts::PI * t).cos()) / 2.0;
        let blend = linear * (1.0 - smoothing) + cosine * smoothing;
        curve[idx] = (target_gain * blend) as f32;
        idx += 1;
    }
    for _ in 0..sustain_steps {
        curve[idx] = target_gain as f32;
        idx += 1;
    }
    for i in 0..decay_steps {
        let t = i as f64 / (decay_steps - 1) as f64;
        let linear = 1.0 - t;
        let cosine = (1.0 + (std::f64::consts::PI * t).cos()) / 2.0;
        let blend = linear * (1.0 - smoothing) + cosine * smoothing;
        if idx < curve.len() {
            curve[idx] = (target_gain * blend) as f32;
            idx += 1;
        }
    }
    curve
}

pub fn plan_morse_playback(
    text: &str,
    settings: &TrainingSettings,
    rng: &mut impl Rng,
) -> PlaybackPlan {
    let resolved_char_wpm = resolve_char_wpm(settings, rng);
    let resolved_effective_wpm = resolve_effective_wpm(settings, resolved_char_wpm, rng);
    let extra = clamp_extra_spacing(settings.playback.extra_word_space_multiplier);
    let side_tone = resolve_tone_hz(settings, rng);

    let dot_char = dot_seconds(resolved_char_wpm);
    let dot_eff = dot_seconds(resolved_effective_wpm);
    let dot_duration = dot_char;
    let dash_duration = dot_char * 3.0;
    let symbol_space = dot_char;
    let char_space = dot_eff * 3.0;
    let word_space = dot_eff * 7.0 * extra;
    let rise_time = settings.band.steepness / 1000.0;
    let smoothing = settings.band.envelope_smoothing.clamp(0.0, 1.0);

    let chars: Vec<char> = text.chars().collect();
    let last_morse_idx = chars.iter().enumerate().rev().find_map(|(i, raw)| {
        let ch = raw.to_ascii_uppercase();
        (*raw != ' ' && morse_for(ch).is_some()).then_some(i)
    });
    let mut current_time = 0.0;
    let mut events = Vec::new();

    for (i, raw) in chars.iter().enumerate() {
        if *raw == ' ' {
            current_time += (word_space - char_space).max(0.0);
            continue;
        }
        let ch = raw.to_ascii_uppercase();
        let Some(morse) = morse_for(ch) else {
            continue;
        };
        for symbol in morse.chars() {
            let duration = if symbol == '.' {
                dot_duration
            } else {
                dash_duration
            };
            let volume = resolve_volume(settings, rng);
            let target_gain = DEFAULT_TARGET_GAIN * volume;
            let envelope = build_envelope_curve(duration, rise_time, target_gain, smoothing);
            events.push(ToneEvent {
                start_sec: current_time,
                duration_sec: duration,
                frequency_hz: side_tone,
                target_gain,
                envelope,
            });
            current_time += duration + symbol_space;
        }
        if last_morse_idx != Some(i) {
            current_time += char_space - symbol_space;
        }
    }

    PlaybackPlan {
        events,
        duration_sec: current_time,
        resolved_char_wpm,
        resolved_effective_wpm,
        rise_time_sec: rise_time,
        envelope_smoothing: smoothing,
    }
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
    use crate::rng::FastrandRng;
    use crate::settings::TrainingSettings;

    #[test]
    fn k_at_20_wpm_has_positive_duration() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.playback.effective_wpm_min = 20.0;
        settings.playback.effective_wpm_max = 20.0;
        settings.playback.link_char_to_effective = true;
        let mut rng = FastrandRng::default();
        let plan = plan_morse_playback("K", &settings, &mut rng);
        assert!(plan.duration_sec > 0.0);
        assert_eq!(plan.events.len(), 3); // -.-
    }

    #[test]
    fn farnsworth_slower_effective_lengthens_gaps() {
        let mut fast = TrainingSettings::default();
        fast.playback.char_wpm_min = 20.0;
        fast.playback.char_wpm_max = 20.0;
        fast.playback.effective_wpm_min = 20.0;
        fast.playback.effective_wpm_max = 20.0;
        let mut slow = fast.clone();
        slow.playback.effective_wpm_min = 10.0;
        slow.playback.effective_wpm_max = 10.0;
        slow.playback.link_char_to_effective = false;
        let mut rng_a = FastrandRng(1);
        let mut rng_b = FastrandRng(1);
        let a = plan_morse_playback("KM", &fast, &mut rng_a);
        let b = plan_morse_playback("KM", &slow, &mut rng_b);
        assert!(b.duration_sec > a.duration_sec);
    }

    #[test]
    fn inter_group_gap_follows_farnsworth_effective_wpm() {
        let mut even = TrainingSettings::default();
        even.playback.char_wpm_min = 20.0;
        even.playback.effective_wpm_min = 20.0;
        even.playback.link_char_to_effective = false;
        even.playback.extra_word_space_multiplier = 1.0;
        let mut farnsworth = even.clone();
        farnsworth.playback.effective_wpm_min = 10.0;
        assert!(compute_group_gap_ms(&farnsworth) > compute_group_gap_ms(&even));
        assert!(
            compute_group_gap_for_wpm(40.0, 40.0, 1.0) < compute_group_gap_for_wpm(18.0, 18.0, 1.0)
        );
    }

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
            assert!(shape
                .points
                .windows(2)
                .all(|w| w[1].t_sec >= w[0].t_sec - 1e-12));
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
    fn group_repeats_stay_inside_the_range() {
        let mut settings = TrainingSettings::default();
        let mut rng = FastrandRng::default();
        assert_eq!(resolve_group_repeats(&settings, &mut rng), 1);
        settings.playback.link_group_repeat = false;
        settings.playback.group_repeat_min = 2;
        settings.playback.group_repeat_max = 4;
        let mut seen_low = false;
        let mut seen_high = false;
        for _ in 0..400 {
            let n = resolve_group_repeats(&settings, &mut rng);
            assert!((2..=4).contains(&n));
            seen_low |= n == 2;
            seen_high |= n == 4;
        }
        assert!(seen_low && seen_high);
    }

    #[test]
    fn unknown_trailing_char_does_not_add_char_space() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.playback.effective_wpm_min = 20.0;
        settings.playback.effective_wpm_max = 20.0;
        settings.playback.link_char_to_effective = true;
        let a = plan_morse_playback("KM", &settings, &mut FastrandRng(1));
        let b = plan_morse_playback("KM#", &settings, &mut FastrandRng(1));
        assert!(morse_for('#').is_none());
        assert!((a.duration_sec - b.duration_sec).abs() < 1e-12);
        assert_eq!(a.events.len(), b.events.len());
    }
}
