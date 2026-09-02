//! Farnsworth Morse timing and a platform-agnostic playback plan.

use crate::morse::morse_for;
use crate::rng::Rng;
use crate::settings::TrainingSettings;

pub const DEFAULT_TARGET_GAIN: f64 = 0.3;
pub const ENVELOPE_SAMPLE_RATE: u32 = 256;
pub const EXTRA_SPACING_MULTIPLIER_MIN: f64 = 0.1;

pub fn clamp_extra_spacing(value: f64) -> f64 {
    value.max(EXTRA_SPACING_MULTIPLIER_MIN)
}

/// PARIS-style dot duration in seconds from character WPM.
pub fn dot_seconds(wpm: f64) -> f64 {
    1.2 / wpm.max(1.0)
}

pub fn compute_group_gap_ms(settings: &TrainingSettings) -> u32 {
    let char_wpm = settings.char_wpm_min.max(1.0);
    // Match intra-group Farnsworth: effective WPM never exceeds character WPM.
    let effective_wpm = settings.effective_wpm_min.max(1.0).min(char_wpm);
    let dot_effective_sec = 1.2 / effective_wpm;
    let word_space_sec =
        7.0 * dot_effective_sec * clamp_extra_spacing(settings.extra_word_space_multiplier);
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
        settings.char_wpm_min.max(1.0),
        settings.char_wpm_max.max(1.0),
    )
    .max(1.0)
}

fn resolve_effective_wpm(settings: &TrainingSettings, char_wpm: f64, rng: &mut impl Rng) -> f64 {
    let sampled = rng
        .pick_in_range(
            settings.effective_wpm_min.max(1.0),
            settings.effective_wpm_max.max(1.0),
        )
        .max(1.0);
    sampled.min(char_wpm)
}

fn resolve_volume(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    let min = settings.volume_min.clamp(0.1, 1.0);
    let max = settings.volume_max.clamp(0.1, 1.0);
    if settings.link_volume || (min - max).abs() < f64::EPSILON {
        min
    } else {
        rng.pick_in_range(min, max)
    }
}

fn resolve_tone_hz(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    let min = settings.side_tone_min.max(100.0);
    let max = settings.side_tone_max.max(min);
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
    if smoothing == 0.0 {
        return vec![0.0, target_gain as f32, target_gain as f32, 0.0];
    }
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
    let extra = clamp_extra_spacing(settings.extra_word_space_multiplier);
    let side_tone = resolve_tone_hz(settings, rng);

    let dot_char = dot_seconds(resolved_char_wpm);
    let dot_eff = dot_seconds(resolved_effective_wpm);
    let dot_duration = dot_char;
    let dash_duration = dot_char * 3.0;
    let symbol_space = dot_char;
    let char_space = dot_eff * 3.0;
    let word_space = dot_eff * 7.0 * extra;
    let rise_time = settings.steepness / 1000.0;
    let smoothing = settings.envelope_smoothing.clamp(0.0, 1.0);

    let chars: Vec<char> = text.chars().collect();
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
        let is_last = i + 1 == chars.len();
        if !is_last {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::FastrandRng;
    use crate::settings::TrainingSettings;

    #[test]
    fn k_at_20_wpm_has_positive_duration() {
        let mut settings = TrainingSettings::default();
        settings.char_wpm_min = 20.0;
        settings.char_wpm_max = 20.0;
        settings.effective_wpm_min = 20.0;
        settings.effective_wpm_max = 20.0;
        settings.link_char_to_effective = true;
        let mut rng = FastrandRng::default();
        let plan = plan_morse_playback("K", &settings, &mut rng);
        assert!(plan.duration_sec > 0.0);
        assert_eq!(plan.events.len(), 3); // -.-
    }

    #[test]
    fn farnsworth_slower_effective_lengthens_gaps() {
        let mut fast = TrainingSettings::default();
        fast.char_wpm_min = 20.0;
        fast.char_wpm_max = 20.0;
        fast.effective_wpm_min = 20.0;
        fast.effective_wpm_max = 20.0;
        let mut slow = fast.clone();
        slow.effective_wpm_min = 10.0;
        slow.effective_wpm_max = 10.0;
        slow.link_char_to_effective = false;
        let mut rng_a = FastrandRng(1);
        let mut rng_b = FastrandRng(1);
        let a = plan_morse_playback("KM", &fast, &mut rng_a);
        let b = plan_morse_playback("KM", &slow, &mut rng_b);
        assert!(b.duration_sec > a.duration_sec);
    }

    #[test]
    fn inter_group_gap_follows_farnsworth_effective_wpm() {
        let mut even = TrainingSettings::default();
        even.char_wpm_min = 20.0;
        even.effective_wpm_min = 20.0;
        even.link_char_to_effective = false;
        even.extra_word_space_multiplier = 1.0;
        let mut farnsworth = even.clone();
        farnsworth.effective_wpm_min = 10.0;
        assert!(compute_group_gap_ms(&farnsworth) > compute_group_gap_ms(&even));
    }
}
