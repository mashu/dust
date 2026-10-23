//! A send laid out as tones against one clock, for any audio backend to play.

use std::sync::Arc;

use super::envelope::build_envelope_curve;
use super::pileup::Transmission;
use super::station::StationVoice;
use super::{clamp_extra_spacing, dot_seconds, spacing_unit_sec};
use crate::morse::morse_for;
use crate::settings::TrainingSettings;

/// The level of a station at full volume, before the receiver.
pub const DEFAULT_TARGET_GAIN: f64 = 0.3;

/// How long a stopped send takes to reach silence, on either player.
///
/// Matched to the keying envelope's own rise time: a trainer this careful
/// about click-free keying should not end a session with a click. Short
/// enough that "stop" still means stop. Kept with the plans rather than
/// the players because a contest splices a continuation onto the send
/// playing, and no sooner than that send can be ridden down.
pub const SEND_RELEASE_SEC: f64 = 0.008;

#[derive(Clone, Debug)]
pub struct ToneEvent {
    pub start_sec: f64,
    pub duration_sec: f64,
    pub frequency_hz: f64,
    pub target_gain: f64,
    /// The keying envelope, sampled over the tone. Shared between every tone
    /// of the same length in a send.
    pub envelope: Arc<[f32]>,
}

#[derive(Clone, Debug)]
pub struct PlaybackPlan {
    pub events: Vec<ToneEvent>,
    pub duration_sec: f64,
    pub resolved_char_wpm: f64,
    pub resolved_effective_wpm: f64,
}

/// The wanted send and whatever is on top of it, as scheduled sound.
#[derive(Clone, Debug)]
pub struct PlannedTransmission {
    pub wanted: PlaybackPlan,
    pub others: Vec<PlaybackPlan>,
    /// `wanted` is played past the receiver, its filter and its AGC, into
    /// the output stage the way the paddle's sidetone goes: your own message
    /// in a contest, which a rig monitors rather than receives. Its tones
    /// carry the level it is heard at. `others` always come through the
    /// receiver.
    pub dry_wanted: bool,
    /// While you send in a contest without full break-in: until when, in
    /// seconds from the start of the send, the receiver's output is muted,
    /// after its AGC, as a rig that is not QSK mutes its receiver while it
    /// transmits. The band and every station the receiver hears go quiet
    /// till then; a dry `wanted`, your own message, and the paddle's
    /// sidetone do not. None with full break-in, and for every send but a
    /// contest's turn with a message of yours in it.
    pub muted_until: Option<f64>,
}

impl PlannedTransmission {
    /// The most of `others` sounding at one moment, each from its first
    /// tone to the end of its last: how many stations the band's own
    /// traffic makes room for while the send plays. A contest's turn plans
    /// each call a caller sends in it as one of `others`, one after another
    /// as often as not, so they are counted by when they sound, not by how
    /// many there are.
    pub fn others_at_once(&self) -> usize {
        let spans: Vec<(f64, f64)> = self
            .others
            .iter()
            .filter_map(|plan| {
                let first = plan.events.first()?;
                let last = plan.events.last()?;
                Some((first.start_sec, last.start_sec + last.duration_sec))
            })
            .collect();
        spans
            .iter()
            .map(|(start, _)| {
                spans
                    .iter()
                    .filter(|(from, to)| from <= start && start < to)
                    .count()
            })
            .max()
            .unwrap_or(0)
    }
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
    PlannedTransmission {
        wanted,
        others,
        dry_wanted: false,
        muted_until: None,
    }
}

/// Plan a send from one station. Every repeat of a group uses the same voice.
pub fn plan_morse_playback_for(
    text: &str,
    settings: &TrainingSettings,
    voice: &StationVoice,
) -> PlaybackPlan {
    let target_gain = DEFAULT_TARGET_GAIN * voice.volume.clamp(0.0, 1.0);
    plan_morse_playback_at(text, settings, voice, target_gain)
}

/// [`plan_morse_playback_for`], with every tone at `target_gain` whatever
/// the voice's volume says: for a send that is never heard through the
/// receiver, whose level is not a station's.
pub(crate) fn plan_morse_playback_at(
    text: &str,
    settings: &TrainingSettings,
    voice: &StationVoice,
    target_gain: f64,
) -> PlaybackPlan {
    let resolved_char_wpm = voice.char_wpm.max(1.0);
    let resolved_effective_wpm = voice.effective_wpm.max(1.0).min(resolved_char_wpm);
    let extra = clamp_extra_spacing(settings.playback.extra_word_space_multiplier);
    let side_tone = voice.tone_hz;

    let dot_char = dot_seconds(resolved_char_wpm);
    let unit = spacing_unit_sec(resolved_char_wpm, resolved_effective_wpm);
    // The operator's fist. Elements are stretched or clipped by their weight
    // and the gap after them takes the difference back, so a heavy fist
    // crowds its spaces rather than simply sending slower — which is what
    // weight actually is, and why it does not quietly change the speed.
    let weight = voice.weight.clamp(0.7, 1.3);
    let dash_ratio = voice.dash_ratio.clamp(2.0, 4.5);
    let dot_duration = dot_char * weight;
    let dash_duration = dot_char * dash_ratio * weight;
    let symbol_space = dot_char * (2.0 - weight);
    let char_space = unit * 3.0;
    let word_space = unit * 7.0 * extra;
    let rise_time = settings.band.steepness / 1000.0;
    let smoothing = settings.band.envelope_smoothing.clamp(0.0, 1.0);
    // Every dit of a send has the same envelope, and every dah: two curves,
    // shared by every tone that uses them.
    let dit_curve: Arc<[f32]> =
        build_envelope_curve(dot_duration, rise_time, target_gain, smoothing).into();
    let dah_curve: Arc<[f32]> =
        build_envelope_curve(dash_duration, rise_time, target_gain, smoothing).into();

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
        let last_symbol = morse.chars().count().saturating_sub(1);
        for (k, symbol) in morse.chars().enumerate() {
            let (duration, envelope) = if symbol == '.' {
                (dot_duration, &dit_curve)
            } else {
                (dash_duration, &dah_curve)
            };
            events.push(ToneEvent {
                start_sec: current_time,
                duration_sec: duration,
                frequency_hz: side_tone,
                target_gain,
                envelope: Arc::clone(envelope),
            });
            current_time += duration;
            // The send ends on its last element. A gap after it would be
            // added to the word space whoever plays this puts before the
            // next group, and the groups would sit a dit too far apart.
            if !(last_morse_idx == Some(i) && k == last_symbol) {
                current_time += symbol_space;
            }
        }
        if last_morse_idx != Some(i) {
            // The letter gap is measured from where the element would have
            // ended at a keyer's weight, so it takes the fist's difference
            // back just as the gaps inside a letter do — a heavy fist crowds
            // its spaces rather than sending slower.
            current_time += char_space - dot_char;
        }
    }

    PlaybackPlan {
        events,
        duration_sec: current_time,
        resolved_char_wpm,
        resolved_effective_wpm,
    }
}

/// `plan` from `from_sec` into it on, moved to start at nothing: for a send
/// that goes on from where one cut short was. A tone sounding at `from_sec`
/// goes on to its end, with a rise of its own rather than starting on a
/// step, unless less is left of it than a rise and a fall; tones before it
/// are gone.
pub(crate) fn plan_from(
    plan: PlaybackPlan,
    from_sec: f64,
    settings: &TrainingSettings,
) -> PlaybackPlan {
    if !from_sec.is_finite() || from_sec <= 0.0 {
        return plan;
    }
    let rise_time = settings.band.steepness / 1000.0;
    let smoothing = settings.band.envelope_smoothing.clamp(0.0, 1.0);
    let events = plan
        .events
        .into_iter()
        .filter_map(|mut tone| {
            let end = tone.start_sec + tone.duration_sec;
            if tone.start_sec < from_sec {
                let left = end - from_sec;
                if left < 2.0 * rise_time.max(0.0) || left <= 0.0 {
                    return None;
                }
                tone.start_sec = from_sec;
                tone.duration_sec = left;
                tone.envelope =
                    build_envelope_curve(left, rise_time, tone.target_gain, smoothing).into();
            }
            tone.start_sec -= from_sec;
            Some(tone)
        })
        .collect();
    PlaybackPlan {
        events,
        duration_sec: (plan.duration_sec - from_sec).max(0.0),
        ..plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::{Rng, SplitMix64};
    use crate::timing::compute_group_gap_for_wpm;
    use crate::timing::station::resolve_station;

    struct Fixed(f64);

    impl Rng for Fixed {
        fn f64(&mut self) -> f64 {
            self.0
        }
    }

    /// A send from a station tuned in from `rng`.
    fn planned(text: &str, settings: &TrainingSettings, rng: &mut impl Rng) -> PlaybackPlan {
        plan_morse_playback_for(text, settings, &resolve_station(settings, rng))
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

    fn fist(variation: f64) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.playback.fist_variation = variation;
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        s.clamp()
    }

    #[test]
    fn k_at_20_wpm_has_positive_duration() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.playback.effective_wpm_min = 20.0;
        settings.playback.effective_wpm_max = 20.0;
        settings.playback.link_char_to_effective = true;
        let mut rng = SplitMix64::default();
        let plan = planned("K", &settings, &mut rng);
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
        let mut rng_a = SplitMix64::new(1);
        let mut rng_b = SplitMix64::new(1);
        let a = planned("KM", &fast, &mut rng_a);
        let b = planned("KM", &slow, &mut rng_b);
        assert!(b.duration_sec > a.duration_sec);
    }

    #[test]
    fn unknown_trailing_char_does_not_add_char_space() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.playback.effective_wpm_min = 20.0;
        settings.playback.effective_wpm_max = 20.0;
        settings.playback.link_char_to_effective = true;
        let a = planned("KM", &settings, &mut SplitMix64::new(1));
        let b = planned("KM#", &settings, &mut SplitMix64::new(1));
        assert!(morse_for('#').is_none());
        assert!((a.duration_sec - b.duration_sec).abs() < 1e-12);
        assert_eq!(a.events.len(), b.events.len());
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
        let mut rng = crate::rng::SplitMix64::new(7);
        let plan = planned("HELLO", &settings, &mut rng);
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
        let plan = planned("E", &settings, &mut rng);
        assert_eq!(plan.events[0].target_gain, DEFAULT_TARGET_GAIN * 0.5);
    }

    #[test]
    fn a_space_stretches_the_gap_to_a_word_space() {
        let settings = fixed_speed(20.0);
        let mut rng = Fixed(0.5);
        let tight = planned("EE", &settings, &mut rng);
        let mut rng = Fixed(0.5);
        let spaced = planned("E E", &settings, &mut rng);
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
        let plain = planned("EE", &settings, &mut rng);
        let mut rng = Fixed(0.5);
        let noisy = planned("E#E", &settings, &mut rng);
        assert_eq!(plain.events.len(), noisy.events.len());
        assert_eq!(plain.duration_sec, noisy.duration_sec);
        // Text with nothing sendable in it plays nothing at all.
        let mut rng = Fixed(0.5);
        let empty = planned("###", &settings, &mut rng);
        assert!(empty.events.is_empty());
        assert_eq!(empty.duration_sec, 0.0);
    }

    #[test]
    fn lower_case_is_sent_the_same_as_upper_case() {
        let settings = fixed_speed(20.0);
        let mut rng = Fixed(0.5);
        let upper = planned("KM", &settings, &mut rng);
        let mut rng = Fixed(0.5);
        let lower = planned("km", &settings, &mut rng);
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
        let plan = planned("K", &settings, &mut rng);
        assert_eq!(plan.resolved_char_wpm, 15.0);
        assert_eq!(plan.resolved_effective_wpm, 15.0);
    }

    /// A send ends on its last element. Whoever plays it puts a word space
    /// before the next group; a gap left at the end of the send was added to
    /// that, and groups sat eight dits apart instead of seven.
    #[test]
    fn a_send_ends_where_its_last_element_ends() {
        let s = fixed_speed(20.0);
        let voice = StationVoice {
            tone_hz: 600.0,
            char_wpm: 20.0,
            effective_wpm: 20.0,
            volume: 1.0,
            weight: 1.0,
            dash_ratio: 3.0,
        };
        let dot = dot_seconds(20.0);
        for text in ["E", "T", "K", "PARIS", "W1AW"] {
            let plan = plan_morse_playback_for(text, &s, &voice);
            let last = plan.events.last().expect("tones");
            assert!(
                (plan.duration_sec - (last.start_sec + last.duration_sec)).abs() < 1e-12,
                "{text} ran {:.4}s past its last element",
                plan.duration_sec - (last.start_sec + last.duration_sec)
            );
        }
        // PARIS is fifty dots with its word space, the definition of a word a
        // minute; without the trailing seven it is forty-three.
        let paris = plan_morse_playback_for("PARIS", &s, &voice).duration_sec;
        assert!(
            (paris / dot - 43.0).abs() < 1e-9,
            "PARIS took {} dots",
            paris / dot
        );
        assert!(
            (f64::from(compute_group_gap_for_wpm(20.0, 20.0, 1.0)) / 1000.0 / dot - 7.0).abs()
                < 0.01
        );
    }

    /// Effective speed is the speed a word arrives at: PARIS and the word
    /// space after it take sixty seconds over the effective WPM, whatever the
    /// characters inside it are sent at. That is the ARRL's definition of
    /// Farnsworth timing, and stretching only the letter and word gaps to
    /// three and seven dots at the effective speed fell well short of it —
    /// ten words a minute played at fourteen and a half.
    #[test]
    fn paris_and_its_word_space_take_a_minute_over_the_effective_speed() {
        let s = fixed_speed(20.0);
        for (char_wpm, effective_wpm) in [(20.0, 20.0), (20.0, 10.0), (25.0, 5.0), (18.0, 12.0)] {
            let voice = StationVoice {
                tone_hz: 600.0,
                char_wpm,
                effective_wpm,
                volume: 1.0,
                weight: 1.0,
                dash_ratio: 3.0,
            };
            let word = plan_morse_playback_for("PARIS", &s, &voice).duration_sec
                + f64::from(compute_group_gap_for_wpm(char_wpm, effective_wpm, 1.0)) / 1000.0;
            let heard_wpm = 60.0 / word;
            assert!(
                (heard_wpm - effective_wpm).abs() / effective_wpm < 0.01,
                "{char_wpm}/{effective_wpm} WPM played at {heard_wpm:.2} WPM"
            );
        }
    }

    #[test]
    fn a_nonsense_speed_is_treated_as_one_word_a_minute() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 0.0;
        settings.playback.char_wpm_max = 0.0;
        settings.playback.effective_wpm_min = 0.0;
        settings.playback.effective_wpm_max = 0.0;
        let mut rng = Fixed(0.5);
        let plan = planned("E", &settings, &mut rng);
        assert_eq!(plan.resolved_char_wpm, 1.0);
        assert_eq!(dot_seconds(0.0), 1.2);
    }

    /// A fist is a person, not a moment. The same station sending again sends
    /// the same way, or a repeat would be a different operator with your call.
    #[test]
    fn a_repeat_is_the_same_operator() {
        let s = fist(1.0);
        let voice = resolve_station(&s, &mut SplitMix64::new(99));
        let once = plan_morse_playback_for("W1AW", &s, &voice);
        let twice = plan_morse_playback_for("W1AW", &s, &voice);
        assert_eq!(once.events.len(), twice.events.len());
        for (a, b) in once.events.iter().zip(&twice.events) {
            assert!((a.start_sec - b.start_sec).abs() < 1e-12);
            assert!((a.duration_sec - b.duration_sec).abs() < 1e-12);
        }
    }

    /// Weight is how the time inside a character is divided, not how much of
    /// it there is. A heavy fist crowds its spaces; it does not send slower,
    /// or the speed setting would stop meaning anything.
    #[test]
    fn a_heavy_fist_is_not_a_slow_one() {
        let s = fist(1.0);
        let keyer = StationVoice {
            tone_hz: 600.0,
            char_wpm: 20.0,
            effective_wpm: 20.0,
            volume: 1.0,
            weight: 1.0,
            dash_ratio: 3.0,
        };
        let heavy = StationVoice {
            weight: 1.18,
            ..keyer
        };
        let light = StationVoice {
            weight: 0.82,
            ..keyer
        };
        // "EEEEE" is all dits and gaps, where weight trades one against the
        // other exactly.
        let at = |voice: &StationVoice| plan_morse_playback_for("EEEEE", &s, voice).duration_sec;
        let (base, long, short) = (at(&keyer), at(&heavy), at(&light));
        assert!(
            (long - base).abs() / base < 0.05 && (short - base).abs() / base < 0.05,
            "weight moved the speed: {short:.3}s and {long:.3}s against {base:.3}s"
        );
        // But the key-down time really did change, which is the audible part.
        let down = |voice: &StationVoice| {
            plan_morse_playback_for("EEEEE", &s, voice)
                .events
                .iter()
                .map(|e| e.duration_sec)
                .sum::<f64>()
        };
        assert!(down(&heavy) > down(&light) * 1.2, "the fists sound alike");
    }

    /// Every dit of a send has the same envelope, and so does every dah:
    /// two curves, shared, rather than one built for every tone.
    #[test]
    fn the_tones_of_a_send_share_their_envelopes() {
        let settings = fixed_speed(20.0);
        let plan = planned("PARIS", &settings, &mut Fixed(0.5));
        let mut curves: Vec<&std::sync::Arc<[f32]>> = Vec::new();
        for event in &plan.events {
            if !curves
                .iter()
                .any(|curve| std::sync::Arc::ptr_eq(curve, &event.envelope))
            {
                curves.push(&event.envelope);
            }
        }
        assert_eq!(curves.len(), 2, "one curve for the dits, one for the dahs");
    }
}
