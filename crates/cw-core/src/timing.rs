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

/// Another station calling over the top of the one you want.
#[derive(Clone, Debug, PartialEq)]
pub struct Interferer {
    pub text: String,
    pub voice: StationVoice,
    /// How late this station starts. Nobody in a pile-up is synchronised, and
    /// a stagger is most of what makes one hard to pick apart.
    pub delay_sec: f64,
}

/// Everything the receiver hears during one send: the station you are copying,
/// and whoever else is calling across it.
#[derive(Clone, Debug, PartialEq)]
pub struct Transmission {
    pub text: String,
    pub voice: StationVoice,
    pub others: Vec<Interferer>,
}

impl Transmission {
    /// One station, alone on the frequency.
    pub fn alone(text: impl Into<String>, voice: StationVoice) -> Self {
        Self {
            text: text.into(),
            voice,
            others: Vec::new(),
        }
    }
}

/// The wanted send and whatever is on top of it, as scheduled sound.
#[derive(Clone, Debug)]
pub struct PlannedTransmission {
    pub wanted: PlaybackPlan,
    pub others: Vec<PlaybackPlan>,
}

/// The closest another station may land to the one you want.
///
/// This is a fact about ears, not about radios. Two tones within about a
/// hundred hertz of each other at CW pitch fall inside one critical band, and
/// the quieter one is then masked rather than heard — it arrives as roughness
/// on the station you want, not as a second station. Twenty-five hertz apart,
/// which is where this started, you hear one station and a warble.
pub const PILEUP_MIN_SEPARATION_HZ: f64 = 130.0;
/// The longest another station can wait before joining in.
const PILEUP_MAX_DELAY_SEC: f64 = 0.6;

/// Tune in whoever else is calling.
///
/// They sit either side of the wanted station, off-frequency and a good margin
/// below it, so the one you want stays the strongest thing in the passband —
/// the whole exercise is copying *that* one. Being off-frequency is what makes
/// the receiver's filter worth reaching for: narrow it and they thin out,
/// because they are further from its centre than the station you are on.
///
/// Takes more texts than it may need and decides how many stations are calling
/// this time, so the count varies from group to group like a real pile-up.
/// Anything matching `wanted_text` is dropped: nobody else has your callsign.
pub fn resolve_pileup(
    settings: &TrainingSettings,
    wanted: &StationVoice,
    wanted_text: &str,
    texts: &[String],
    rng: &mut impl Rng,
) -> Vec<Interferer> {
    let most = settings
        .band
        .stations_max
        .clamp(crate::settings::STATIONS_MIN, crate::settings::STATIONS_MAX);
    if most <= 1 || texts.is_empty() {
        return Vec::new();
    }
    // One station, plus however many others turn up this time.
    let calling = rng.usize_in(1, most as usize);
    let spread = settings.band.pileup_spread_hz.clamp(
        crate::settings::PILEUP_SPREAD_MIN,
        crate::settings::PILEUP_SPREAD_MAX,
    );
    let down_db = settings.band.pileup_level_db.clamp(
        crate::settings::PILEUP_LEVEL_MIN_DB,
        crate::settings::PILEUP_LEVEL_MAX_DB,
    );

    // Where the others may sit is decided by the receiver, because the
    // receiver is what you hear. Two rules fall out of that, and between them
    // they are the whole placement.
    //
    // They have to land inside the passband, or they are not in the pile-up at
    // all — which is why squeezing the filter thins it, and why reaching for
    // the filter is worth teaching.
    //
    // And none of them may sit closer to the middle of the passband than the
    // station you want, or the filter would favour the wrong one and hand you
    // an interferer louder than your own station however far down we set it.
    // Tuning the one you want toward the middle is the other half of the same
    // tactic.
    let centre = settings.side_tone_center();
    let half = settings.band.filter_bandwidth_hz.clamp(
        crate::settings::FILTER_BANDWIDTH_MIN,
        crate::settings::FILTER_BANDWIDTH_MAX,
    ) / 2.0;
    let off_centre = (wanted.tone_hz - centre).abs();
    let (outward_low, outward_high) = (centre - off_centre, centre + off_centre);
    let (passband_low, passband_high) = (centre - half, centre + half);

    let mut others = Vec::new();
    for text in texts.iter().take(calling.saturating_sub(1)) {
        // Two stations with the same call is not a pile-up, it is a mistake —
        // and on a short alphabet the generator will hand us one sooner or
        // later. Dropping it costs a station rather than the sense of it.
        if text.is_empty() || text == wanted_text {
            continue;
        }
        let reach = spread.max(PILEUP_MIN_SEPARATION_HZ);
        // Either side: far enough off to be told apart, no further out than
        // the spread allows, inside the passband, and no nearer the middle of
        // it than the station you want. Whichever side still has room wins; if
        // neither does, this caller is one the receiver does not give you.
        let below = (
            (wanted.tone_hz - reach).max(passband_low),
            (wanted.tone_hz - PILEUP_MIN_SEPARATION_HZ).min(outward_low),
        );
        let above = (
            (wanted.tone_hz + PILEUP_MIN_SEPARATION_HZ).max(outward_high),
            (wanted.tone_hz + reach).min(passband_high),
        );
        let fits = |(low, high): (f64, f64)| high >= low;
        let side = match (fits(below), fits(above)) {
            (true, true) if rng.f64() < 0.5 => below,
            (true, true) => above,
            (true, false) => below,
            (false, true) => above,
            (false, false) => continue,
        };
        // Each one is its own operator, and its own distance away.
        let extra_db = rng.pick_in_range(0.0, 5.0);
        let gain = 10f64.powf(-(down_db + extra_db) / 20.0);
        let mut voice = resolve_station(settings, rng);
        voice.tone_hz = rng.pick_in_range(side.0, side.1).max(20.0);
        voice.volume = (wanted.volume * gain).clamp(0.0, 1.0);
        others.push(Interferer {
            text: text.clone(),
            voice,
            delay_sec: rng.pick_in_range(0.0, PILEUP_MAX_DELAY_SEC),
        });
    }
    others
}

/// Lay out every station's sound against one clock.
///
/// The others are pushed back by their own delay and cut off at the end of the
/// wanted send — whole elements only, so nothing stops mid-tone. What the
/// listener gets is stations still calling as the one they want finishes,
/// which is exactly how it sounds on the air.
pub fn plan_transmission(
    transmission: &Transmission,
    settings: &TrainingSettings,
) -> PlannedTransmission {
    let wanted = plan_morse_playback_for(&transmission.text, settings, &transmission.voice);
    let ends_at = wanted.duration_sec;
    let others = transmission
        .others
        .iter()
        .map(|other| {
            let mut plan = plan_morse_playback_for(&other.text, settings, &other.voice);
            let delay = other.delay_sec.max(0.0);
            plan.events.retain_mut(|event| {
                event.start_sec += delay;
                event.start_sec + event.duration_sec <= ends_at
            });
            plan.duration_sec = (plan.duration_sec + delay).min(ends_at);
            plan
        })
        .collect();
    PlannedTransmission { wanted, others }
}

/// Plan a send from a station tuned in on the spot — a preview, a sample, one
/// character on the listen screen. A session uses [`plan_morse_playback_for`]
/// so the station stays the same across a group's repeats.
pub fn plan_morse_playback(
    text: &str,
    settings: &TrainingSettings,
    rng: &mut impl Rng,
) -> PlaybackPlan {
    let voice = resolve_station(settings, rng);
    plan_morse_playback_for(text, settings, &voice)
}

/// The operator at the other end: one pitch, one fist, one signal strength.
///
/// Drawn per station rather than per send, because a station that repeats its
/// call does not change frequency, speed or strength between repeats. Leaving
/// this to whenever the planner happened to reach for the RNG meant three
/// sends of one group arrived as three different operators.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StationVoice {
    pub tone_hz: f64,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    pub volume: f64,
}

/// Tune in a station, within whatever range the settings allow.
pub fn resolve_station(settings: &TrainingSettings, rng: &mut impl Rng) -> StationVoice {
    let char_wpm = resolve_char_wpm(settings, rng);
    StationVoice {
        tone_hz: resolve_tone_hz(settings, rng),
        char_wpm,
        effective_wpm: resolve_effective_wpm(settings, char_wpm, rng),
        volume: resolve_volume(settings, rng),
    }
}

/// Plan a send from one station. Every repeat of a group uses the same voice.
pub fn plan_morse_playback_for(
    text: &str,
    settings: &TrainingSettings,
    voice: &StationVoice,
) -> PlaybackPlan {
    let resolved_char_wpm = voice.char_wpm.max(1.0);
    let resolved_effective_wpm = voice.effective_wpm.max(1.0).min(resolved_char_wpm);
    let extra = clamp_extra_spacing(settings.playback.extra_word_space_multiplier);
    let side_tone = voice.tone_hz;
    let target_gain = DEFAULT_TARGET_GAIN * voice.volume.clamp(0.0, 1.0);

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

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::rng::Rng;
    use crate::settings::TrainingSettings;

    struct Fixed(f64);

    impl Rng for Fixed {
        fn f64(&mut self) -> f64 {
            self.0
        }
    }

    fn fixed_speed(wpm: f64) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.playback.char_wpm_min = wpm;
        s.playback.char_wpm_max = wpm;
        s.playback.effective_wpm_min = wpm;
        s.playback.effective_wpm_max = wpm;
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        s
    }

    #[test]
    fn one_send_keeps_one_tone_and_one_level_throughout() {
        // Tone, speed and volume belong to the whole send, not to each symbol.
        let mut settings = fixed_speed(20.0);
        settings.band.side_tone_min = 400.0;
        settings.band.side_tone_max = 900.0;
        settings.band.volume_min = 0.2;
        settings.band.volume_max = 1.0;
        settings.band.link_volume = false;
        let mut rng = crate::rng::FastrandRng(7);
        let plan = plan_morse_playback("HELLO", &settings, &mut rng);
        let first = &plan.events[0];
        assert!(plan.events.len() > 4);
        for event in &plan.events {
            assert_eq!(event.frequency_hz, first.frequency_hz);
            assert_eq!(event.target_gain, first.target_gain);
        }
        assert!((400.0..=900.0).contains(&first.frequency_hz));
        assert!(first.target_gain <= DEFAULT_TARGET_GAIN);
    }

    #[test]
    fn a_linked_volume_is_used_as_is() {
        let mut settings = fixed_speed(20.0);
        settings.band.link_volume = true;
        settings.band.volume_min = 0.5;
        settings.band.volume_max = 1.0;
        let mut rng = Fixed(0.99);
        let plan = plan_morse_playback("E", &settings, &mut rng);
        assert_eq!(plan.events[0].target_gain, DEFAULT_TARGET_GAIN * 0.5);
    }

    #[test]
    fn a_space_stretches_the_gap_to_a_word_space() {
        let settings = fixed_speed(20.0);
        let mut rng = Fixed(0.5);
        let tight = plan_morse_playback("EE", &settings, &mut rng);
        let mut rng = Fixed(0.5);
        let spaced = plan_morse_playback("E E", &settings, &mut rng);
        assert_eq!(tight.events.len(), spaced.events.len());
        let gap = spaced.events[1].start_sec - tight.events[1].start_sec;
        let dot = dot_seconds(20.0);
        // A word space is seven dots where a character space is three.
        assert!((gap - 4.0 * dot).abs() < 1e-9, "gap was {gap}");
    }

    #[test]
    fn characters_with_no_morse_code_are_skipped() {
        let settings = fixed_speed(20.0);
        let mut rng = Fixed(0.5);
        let plain = plan_morse_playback("EE", &settings, &mut rng);
        let mut rng = Fixed(0.5);
        let noisy = plan_morse_playback("E#E", &settings, &mut rng);
        assert_eq!(plain.events.len(), noisy.events.len());
        assert_eq!(plain.duration_sec, noisy.duration_sec);
        // Text with nothing sendable in it plays nothing at all.
        let mut rng = Fixed(0.5);
        let empty = plan_morse_playback("###", &settings, &mut rng);
        assert!(empty.events.is_empty());
        assert_eq!(empty.duration_sec, 0.0);
    }

    #[test]
    fn lower_case_is_sent_the_same_as_upper_case() {
        let settings = fixed_speed(20.0);
        let mut rng = Fixed(0.5);
        let upper = plan_morse_playback("KM", &settings, &mut rng);
        let mut rng = Fixed(0.5);
        let lower = plan_morse_playback("km", &settings, &mut rng);
        assert_eq!(upper.events.len(), lower.events.len());
        assert_eq!(upper.duration_sec, lower.duration_sec);
    }

    #[test]
    fn effective_speed_never_outruns_character_speed() {
        let mut settings = TrainingSettings::default();
        settings.playback.link_char_to_effective = false;
        settings.playback.char_wpm_min = 15.0;
        settings.playback.char_wpm_max = 15.0;
        settings.playback.effective_wpm_min = 40.0;
        settings.playback.effective_wpm_max = 40.0;
        let mut rng = Fixed(0.5);
        let plan = plan_morse_playback("K", &settings, &mut rng);
        assert_eq!(plan.resolved_char_wpm, 15.0);
        assert_eq!(plan.resolved_effective_wpm, 15.0);
    }

    #[test]
    fn a_nonsense_speed_is_treated_as_one_word_a_minute() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 0.0;
        settings.playback.char_wpm_max = 0.0;
        settings.playback.effective_wpm_min = 0.0;
        settings.playback.effective_wpm_max = 0.0;
        let mut rng = Fixed(0.5);
        let plan = plan_morse_playback("E", &settings, &mut rng);
        assert_eq!(plan.resolved_char_wpm, 1.0);
        assert_eq!(dot_seconds(0.0), 1.2);
    }

    #[test]
    fn repeats_are_drawn_from_the_range_when_it_is_not_linked() {
        let mut settings = TrainingSettings::default();
        settings.playback.link_group_repeat = false;
        settings.playback.group_repeat_min = 2;
        settings.playback.group_repeat_max = 4;
        let mut low = Fixed(0.0);
        assert_eq!(resolve_group_repeats(&settings, &mut low), 2);
        let mut high = Fixed(0.999_999);
        assert_eq!(resolve_group_repeats(&settings, &mut high), 4);

        // Linked, the lower bound is used whatever the draw.
        settings.playback.link_group_repeat = true;
        let mut high = Fixed(0.999_999);
        assert_eq!(resolve_group_repeats(&settings, &mut high), 2);

        // Out-of-range settings are pulled back into the allowed span.
        settings.playback.link_group_repeat = false;
        settings.playback.group_repeat_min = 0;
        settings.playback.group_repeat_max = 99;
        let mut high = Fixed(0.999_999);
        assert_eq!(
            resolve_group_repeats(&settings, &mut high),
            crate::settings::GROUP_REPEAT_MAX
        );
    }

    #[test]
    fn the_word_space_between_groups_follows_the_slower_speed() {
        let fast = compute_group_gap_for_wpm(20.0, 20.0, 1.0);
        let slow = compute_group_gap_for_wpm(20.0, 10.0, 1.0);
        assert!(slow > fast);
        // Extra spacing multiplies it.
        assert_eq!(compute_group_gap_for_wpm(20.0, 20.0, 2.0), fast * 2);
        // An effective speed above the character speed is ignored.
        assert_eq!(compute_group_gap_for_wpm(20.0, 40.0, 1.0), fast);
        // Nonsense speeds do not divide by zero.
        assert!(compute_group_gap_for_wpm(0.0, 0.0, 0.0) > 0);
        assert_eq!(clamp_extra_spacing(-1.0), EXTRA_SPACING_MULTIPLIER_MIN);
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
        assert!(shape
            .points
            .iter()
            .any(|p| (p.t_sec - shape.dot_sec * 2.0).abs() < 1e-9));
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

#[cfg(test)]
mod pileup_tests {
    use super::*;
    use crate::settings::{
        TrainingSettings, FILTER_BANDWIDTH_MAX, FILTER_BANDWIDTH_MIN, PILEUP_LEVEL_MIN_DB,
        PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN, STATIONS_MAX, STATIONS_MIN,
    };
    use crate::FastrandRng;

    fn settings(max: u32) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.band.stations_max = max;
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        s.band.link_volume = true;
        s.band.volume_min = 1.0;
        s.clamp()
    }

    fn texts(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("TEST{i}")).collect()
    }

    fn pileup(max: u32, seed: u64) -> (StationVoice, Vec<Interferer>) {
        let s = settings(max);
        let mut rng = FastrandRng(seed.wrapping_mul(7919).wrapping_add(3));
        let wanted = resolve_station(&s, &mut rng);
        let others = resolve_pileup(&s, &wanted, "W1AW", &texts(max as usize), &mut rng);
        (wanted, others)
    }

    /// The default is one station, alone. A pile-up is something you ask for.
    #[test]
    fn nobody_else_is_calling_until_you_say_so() {
        assert_eq!(TrainingSettings::default().band.stations_max, STATIONS_MIN);
        for seed in 0..50 {
            assert!(pileup(STATIONS_MIN, seed).1.is_empty());
        }
    }

    /// The exercise is copying the strongest station, so the one you want has
    /// to be the strongest — every time, not usually.
    #[test]
    fn the_station_you_want_is_always_the_strongest() {
        let floor = settings(STATIONS_MAX).band.pileup_level_db;
        for seed in 0..400 {
            let (wanted, others) = pileup(STATIONS_MAX, seed);
            for other in &others {
                assert!(
                    other.voice.volume < wanted.volume,
                    "seed {seed}: another station matched the one you want"
                );
                let under = -20.0 * (other.voice.volume / wanted.volume).log10();
                assert!(
                    under >= floor - 0.001,
                    "seed {seed}: only {under:.1} dB under, asked for {floor}"
                );
            }
        }
    }

    /// They sit either side, far enough off to be pickable apart by ear and by
    /// filter, and not so far they stop being a pile-up.
    #[test]
    fn the_others_sit_around_you_rather_than_on_top_of_you() {
        let spread = settings(STATIONS_MAX).band.pileup_spread_hz;
        let mut below = 0;
        let mut above = 0;
        for seed in 0..400 {
            let (wanted, others) = pileup(STATIONS_MAX, seed);
            for other in &others {
                let offset = other.voice.tone_hz - wanted.tone_hz;
                assert!(
                    offset.abs() >= PILEUP_MIN_SEPARATION_HZ - 0.001,
                    "seed {seed}: a station landed {offset:.0} Hz away"
                );
                assert!(
                    offset.abs() <= spread + 0.001,
                    "seed {seed}: a station strayed {offset:.0} Hz out"
                );
                assert!(other.voice.tone_hz > 0.0);
                if offset < 0.0 {
                    below += 1;
                } else {
                    above += 1;
                }
            }
        }
        assert!(below > 0 && above > 0, "they all went the same way");
    }

    /// A pile-up that arrived the same way every time would be one puzzle, not
    /// a drill.
    #[test]
    fn how_many_are_calling_changes_from_group_to_group() {
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..400 {
            seen.insert(pileup(STATIONS_MAX, seed).1.len() + 1);
        }
        assert_eq!(
            seen,
            (1..=STATIONS_MAX as usize).collect(),
            "every count from one to the maximum should turn up"
        );

        // And the stagger is real: they do not all start together.
        let delays: Vec<f64> = (0..200)
            .flat_map(|seed| pileup(STATIONS_MAX, seed).1)
            .map(|other| other.delay_sec)
            .collect();
        assert!(delays.iter().any(|d| *d > 0.05), "nobody was late");
        assert!(
            delays
                .iter()
                .all(|d| (0.0..=PILEUP_MAX_DELAY_SEC).contains(d)),
            "a station waited longer than the send"
        );
    }

    /// The same group has to bring the same pile-up, or a repeat would be a
    /// different puzzle and a retry after a stalled send would change the
    /// answer underneath you.
    #[test]
    fn the_same_draw_brings_the_same_stations() {
        let s = settings(STATIONS_MAX);
        let once = {
            let mut rng = FastrandRng(99);
            let wanted = resolve_station(&s, &mut rng);
            resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng)
        };
        let twice = {
            let mut rng = FastrandRng(99);
            let wanted = resolve_station(&s, &mut rng);
            resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng)
        };
        assert_eq!(once, twice);
    }

    /// Everyone is laid against the wanted station's clock: pushed back by
    /// their own delay, and cut at its end — whole elements only, because a
    /// tone stopped halfway through is a click.
    #[test]
    fn the_others_are_cut_at_the_end_and_never_mid_tone() {
        let s = settings(STATIONS_MAX);
        for seed in 0..200 {
            let mut rng = FastrandRng(seed * 31 + 7);
            let wanted = resolve_station(&s, &mut rng);
            let others = resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng);
            if others.is_empty() {
                continue;
            }
            let sent = Transmission {
                text: "W1AW".into(),
                voice: wanted,
                others: others.clone(),
            };
            let planned = plan_transmission(&sent, &s);
            let ends_at = planned.wanted.duration_sec;
            assert!(ends_at > 0.0);
            for (plan, other) in planned.others.iter().zip(&others) {
                for event in &plan.events {
                    assert!(
                        event.start_sec >= other.delay_sec - 1e-9,
                        "seed {seed}: a station started before it was due"
                    );
                    assert!(
                        event.start_sec + event.duration_sec <= ends_at + 1e-9,
                        "seed {seed}: a station ran past the send it was over"
                    );
                }
                assert!(plan.duration_sec <= ends_at + 1e-9);
            }
        }
    }

    /// Whatever the settings say, what comes out has to be playable.
    #[test]
    fn a_pile_up_is_always_sendable() {
        for max in STATIONS_MIN..=STATIONS_MAX {
            for spread in [PILEUP_SPREAD_MIN, 140.0, PILEUP_SPREAD_MAX] {
                let mut s = settings(max);
                s.band.pileup_spread_hz = spread;
                let s = s.clamp();
                for seed in 0..40 {
                    let mut rng = FastrandRng(seed * 13 + 1);
                    let wanted = resolve_station(&s, &mut rng);
                    let others =
                        resolve_pileup(&s, &wanted, "K5ABC", &texts(max as usize), &mut rng);
                    let sent = Transmission {
                        text: "K5ABC".into(),
                        voice: wanted,
                        others,
                    };
                    let planned = plan_transmission(&sent, &s);
                    for plan in std::iter::once(&planned.wanted).chain(&planned.others) {
                        for event in &plan.events {
                            assert!(event.frequency_hz.is_finite() && event.frequency_hz > 0.0);
                            assert!(event.target_gain.is_finite() && event.target_gain >= 0.0);
                            assert!(event.duration_sec.is_finite() && event.duration_sec > 0.0);
                        }
                    }
                }
            }
        }
    }

    /// Reaching for a narrower filter is the tactic this whole feature exists
    /// to teach, so it has to work: squeeze the passband and the callers
    /// beside you drop out of it one by one, until the only station left is
    /// the one you were tuned to.
    #[test]
    fn a_narrower_filter_thins_the_pile_up() {
        // The real side-tone spread, not a pinned tone: how much room there is
        // beside you depends on where in the passband you happen to sit.
        let base = {
            let mut s = TrainingSettings::default();
            s.band.stations_max = STATIONS_MAX;
            s.clamp()
        };

        let heard = |bandwidth: f64| {
            let mut s = base.clone();
            s.band.filter_bandwidth_hz = bandwidth;
            let s = s.clamp();
            let centre = s.side_tone_center();
            let half = s.band.filter_bandwidth_hz / 2.0;
            let mut count = 0usize;
            for seed in 0..400u64 {
                let mut rng = FastrandRng(seed * 2_654_435_761 + 11);
                let wanted = resolve_station(&s, &mut rng);
                for other in resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng) {
                    // Nobody is ever placed where the filter would bury them.
                    assert!(
                        other.voice.tone_hz >= centre - half - 0.001
                            && other.voice.tone_hz <= centre + half + 0.001,
                        "a station at {:.0} Hz sat outside a {bandwidth:.0} Hz passband",
                        other.voice.tone_hz
                    );
                    count += 1;
                }
            }
            count
        };

        let wide = heard(FILTER_BANDWIDTH_MAX);
        let narrow = heard(FILTER_BANDWIDTH_MIN);

        assert!(wide > 0, "nobody else was ever heard, even wide open");
        // Wide open it is the spread that holds them in, not the filter, so a
        // filter only starts costing callers once it is narrower than that.
        // By the time it is at its narrowest, most of them are gone.
        assert!(
            narrow * 4 < wide,
            "a {FILTER_BANDWIDTH_MIN:.0} Hz filter still passed {narrow} of {wide} callers"
        );
    }

    /// The level setting says how far down the others are before the receiver.
    /// What you hear is after it, and a filter does not treat every pitch
    /// alike — so this is the one that matters: whatever the tuning and
    /// whatever the bandwidth, the station you want comes out on top.
    #[test]
    fn the_station_you_want_is_still_the_strongest_through_the_filter() {
        use crate::band::ReceiverFilter;

        const SAMPLE_RATE: u32 = 48_000;

        // Steady-state amplitude of a tone at `hz` once it is through the
        // receiver: let the filter settle first, then measure.
        fn through_filter(settings: &TrainingSettings, hz: f64) -> f64 {
            let mut filter = ReceiverFilter::from_settings(SAMPLE_RATE, settings);
            let step = std::f64::consts::TAU * hz / f64::from(SAMPLE_RATE);
            let settle = SAMPLE_RATE as usize / 4;
            let measure = SAMPLE_RATE as usize / 8;
            let mut sum_sq = 0.0;
            for n in 0..settle + measure {
                let out = filter.process((step * n as f64).sin());
                if n >= settle {
                    sum_sq += out * out;
                }
            }
            (sum_sq / measure as f64).sqrt()
        }

        for bandwidth in [FILTER_BANDWIDTH_MIN, 300.0, 500.0, FILTER_BANDWIDTH_MAX] {
            let mut s = TrainingSettings::default();
            s.band.stations_max = STATIONS_MAX;
            s.band.filter_bandwidth_hz = bandwidth;
            let s = s.clamp();
            for seed in 0..150u64 {
                let mut rng = FastrandRng(seed * 6_364_136_223 + 17);
                let wanted = resolve_station(&s, &mut rng);
                let yours = wanted.volume * through_filter(&s, wanted.tone_hz);
                for other in resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng) {
                    let theirs = other.voice.volume * through_filter(&s, other.voice.tone_hz);
                    let margin = 20.0 * (yours / theirs).log10();
                    // Not merely ahead — ahead by at least what the weakest
                    // setting promises, or the two would be a coin toss.
                    assert!(
                        margin >= PILEUP_LEVEL_MIN_DB,
                        "{bandwidth:.0} Hz, seed {seed}: a caller at {:.0} Hz came out only \
                         {margin:.1} dB under the station you want at {:.0} Hz",
                        other.voice.tone_hz,
                        wanted.tone_hz
                    );
                }
            }
        }
    }
}
