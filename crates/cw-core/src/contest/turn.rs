//! A turn: one send holding your message and every reply to it.
//!
//! The players hold one send at a time, and a timer on the screen fires too
//! late for a reply that starts a tenth of a second after you stop. So each
//! thing you send is one turn: your message, then each caller's reply after
//! its own delay, all against one clock. Your message is fixed when you key
//! it, so the replies are decided then as well.
//!
//! Each caller fades on a path of its own: the band's fading, from a point of
//! its own in it, laid into the envelope of every tone it sends, so that two
//! callers in one turn rise and fall apart and your own message never fades.

use std::sync::Arc;

use crate::band::Qsb;
use crate::morse::morse_for;
use crate::settings::{TrainingSettings, VOLUME_MAX};
use crate::timing::{
    Interferer, PlannedTransmission, PlaybackPlan, StationVoice, ToneEvent, Transmission,
    plan_morse_playback_at, plan_morse_playback_for,
};

/// How loud you hear your own message, as a peak of full scale. It is
/// played past the receiver, as a rig's sidetone is, so no band setting
/// moves its level or takes any of it away.
///
/// Half of full scale: about 3 dB under the paddle's sidetone at the default
/// volume floor (0.7), so a message sounds like your own keying; 4.4 dB over
/// the loudest caller (0.3, before the AGC), so at the hardest noise the
/// floor sits about 11 dB under it through the default filter, where the
/// loudest caller stands 7 dB clear; and under the output stage's limiter,
/// which starts to bend at 0.7, so it is never bent on its own and the
/// band's crests on top of it are caught rather than clipped.
///
/// The worst band is the widest filter at the hardest noise and static:
/// there your message is only about 5 dB over the band's RMS, and the
/// limiter takes the band's crests on top of it for some 6% of the time. A
/// steady tone still stands well clear of hiss spread over two kilohertz,
/// and going louder would only bend more of it.
pub const MONITOR_LEVEL: f64 = 0.5;

/// Who sends a segment of a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sender {
    You,
    /// A caller, by its number in the run.
    Caller(u32),
}

/// How a caller's segment fades: the band's fading, as the Band card sets
/// it, read from `from_sec` on its clock where the segment starts. Each
/// caller reads it from a point of its own, so no two fade together.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub qsb: Qsb,
    pub from_sec: f64,
}

/// One station's part of a turn.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub who: Sender,
    pub text: String,
    pub voice: StationVoice,
    /// When it starts and ends, in seconds from the start of the turn.
    pub start_sec: f64,
    pub end_sec: f64,
    /// A caller's fading; none for your message, or with fading off.
    pub fade: Option<Fade>,
}

/// One send: your message, if there is one, and the replies to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub id: u64,
    /// Run time when it was keyed.
    pub at_run_ms: u64,
    pub segments: Vec<Segment>,
}

impl Turn {
    /// When its last sound ends, in seconds from its start.
    pub fn duration_sec(&self) -> f64 {
        self.segments
            .iter()
            .map(|segment| segment.end_sec)
            .fold(0.0, f64::max)
    }

    /// What the turn says, as a send is written down: your message from your
    /// voice, and each reply as a station calling across it after its delay.
    /// A turn of callers alone is written from the first caller's voice, with
    /// no text of yours. The players play [`plan_turn`]'s plan; this is the
    /// words, for a player that keeps them. None for a turn with nothing in
    /// it, which is never sent.
    pub fn transcript(&self) -> Option<Transmission> {
        let yours = self
            .segments
            .iter()
            .find(|segment| segment.who == Sender::You);
        let first = yours.or(self.segments.first())?;
        Some(Transmission {
            text: yours
                .map(|segment| segment.text.clone())
                .unwrap_or_default(),
            voice: first.voice,
            others: self
                .segments
                .iter()
                .filter(|segment| segment.who != Sender::You)
                .map(|segment| Interferer {
                    text: segment.text.clone(),
                    voice: segment.voice,
                    delay_sec: segment.start_sec,
                })
                .collect(),
        })
    }
}

/// The voice you send in: the receiver's centre pitch, your speed with no
/// Farnsworth spacing, and a keyer's fist. Its volume is full, but
/// [`plan_turn`] plays it at [`MONITOR_LEVEL`] whatever the volume says:
/// you hear your message as you send it, not through the receiver.
pub(super) fn your_voice(settings: &TrainingSettings, wpm: f64) -> StationVoice {
    StationVoice {
        tone_hz: settings.side_tone_center(),
        char_wpm: wpm,
        effective_wpm: wpm,
        volume: VOLUME_MAX,
        weight: 1.0,
        dash_ratio: 3.0,
    }
}

/// How long `text` takes from `voice`, in seconds: what a turn's segment
/// spans, worked out by the planner that plays it.
pub(super) fn sending_sec(text: &str, settings: &TrainingSettings, voice: &StationVoice) -> f64 {
    plan_morse_playback_for(text, settings, voice).duration_sec
}

/// Where each character of `text` that sounds starts and ends, in seconds
/// from the start of the text: its byte index, start and end.
pub(super) fn char_spans(
    text: &str,
    settings: &TrainingSettings,
    voice: &StationVoice,
) -> Vec<(usize, f64, f64)> {
    let plan = plan_morse_playback_for(text, settings, voice);
    let mut tones = plan.events.iter();
    text.char_indices()
        .filter(|(_, c)| *c != ' ')
        .filter_map(|(at, c)| {
            let elements = morse_for(c)?.len();
            let first = tones.next()?;
            let mut end = first.start_sec + first.duration_sec;
            for tone in tones.by_ref().take(elements - 1) {
                end = tone.start_sec + tone.duration_sec;
            }
            Some((at, first.start_sec, end))
        })
        .collect()
}

/// The byte index of the character sounding `at_sec` into a text with
/// `spans`, or of the next one when `at_sec` falls between two: where the
/// rest of it starts, that character sent again whole. None once all of it
/// has been sent.
pub(super) fn sounding_at(spans: &[(usize, f64, f64)], at_sec: f64) -> Option<usize> {
    spans
        .iter()
        .find(|(_, _, end)| *end > at_sec)
        .map(|(at, _, _)| *at)
}

/// The sound of `turn`. `wanted` is your message, stretched to end with the
/// last reply, because both players play a send for as long as `wanted`
/// lasts. It is dry: played past the receiver at [`MONITOR_LEVEL`], so the
/// filter never shapes it and the AGC never ducks a reply for it. `others`
/// are the replies, through the receiver, each after its delay and never
/// cut, which is why [`crate::plan_transmission`], which cuts them where
/// `wanted` ends, is not used. A turn of callers alone has a `wanted` with
/// no tones in it.
///
/// A caller's fade is laid into its tones' envelopes ([`fade_tones`]), as
/// both players play a tone at its envelope; the send itself carries no
/// fading, so none reaches your message.
pub fn plan_turn(turn: &Turn, settings: &TrainingSettings) -> PlannedTransmission {
    let mut wanted: Option<PlaybackPlan> = None;
    let mut others = Vec::new();
    for segment in &turn.segments {
        let mut plan = match segment.who {
            Sender::You => {
                plan_morse_playback_at(&segment.text, settings, &segment.voice, MONITOR_LEVEL)
            }
            Sender::Caller(_) => {
                let mut plan = plan_morse_playback_for(&segment.text, settings, &segment.voice);
                if let Some(fade) = segment.fade {
                    fade_tones(&mut plan.events, fade);
                }
                plan
            }
        };
        for tone in &mut plan.events {
            tone.start_sec += segment.start_sec;
        }
        plan.duration_sec += segment.start_sec;
        match (segment.who, wanted.as_mut()) {
            (Sender::You, Some(yours)) => {
                yours.events.extend(plan.events);
                yours.duration_sec = yours.duration_sec.max(plan.duration_sec);
            }
            (Sender::You, None) => wanted = Some(plan),
            (Sender::Caller(_), _) => others.push(plan),
        }
    }
    let ends = others
        .iter()
        .map(|plan| plan.duration_sec)
        .fold(0.0, f64::max);
    let mut wanted = wanted.unwrap_or_else(|| {
        // No message of yours: silence, as long as the replies, at the
        // speed of whoever sends.
        let (char_wpm, effective_wpm) = others.first().map_or((0.0, 0.0), |plan| {
            (plan.resolved_char_wpm, plan.resolved_effective_wpm)
        });
        PlaybackPlan {
            events: Vec::new(),
            duration_sec: 0.0,
            resolved_char_wpm: char_wpm,
            resolved_effective_wpm: effective_wpm,
        }
    });
    wanted.duration_sec = wanted.duration_sec.max(ends);
    PlannedTransmission {
        wanted,
        others,
        dry_wanted: true,
    }
}

/// Lay `fade` into each of `tones`, timed from the start of their segment:
/// every point of a tone's envelope, which is sampled at the envelope's
/// rate (`ENVELOPE_SAMPLE_RATE`), is scaled by the fade at the moment it
/// plays, so a fade a few times a second still moves smoothly through a long
/// dah. Each tone has an envelope of its own then, no longer one shared by
/// every dit, and its level, which a player reads only where it cannot play
/// the envelope, is scaled by the fade at its middle.
fn fade_tones(tones: &mut [ToneEvent], fade: Fade) {
    for tone in tones {
        let points = tone.envelope.len();
        let at = |point: usize| {
            let through = if points > 1 {
                point as f64 / (points - 1) as f64
            } else {
                0.5
            };
            fade.qsb
                .gain_at(fade.from_sec + tone.start_sec + tone.duration_sec * through)
        };
        let faded: Vec<f32> = tone
            .envelope
            .iter()
            .enumerate()
            .map(|(point, level)| level * at(point))
            .collect();
        tone.envelope = Arc::from(faded);
        tone.target_gain *= f64::from(
            fade.qsb
                .gain_at(fade.from_sec + tone.start_sec + tone.duration_sec / 2.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;
    use crate::timing::{DEFAULT_TARGET_GAIN, plan_transmission, resolve_station};

    fn settings() -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.playback.fist_variation = 1.0;
        settings.clamp()
    }

    fn segment(who: Sender, text: &str, voice: StationVoice, start_sec: f64) -> Segment {
        let end_sec = start_sec + sending_sec(text, &settings(), &voice);
        Segment {
            who,
            text: text.to_string(),
            voice,
            start_sec,
            end_sec,
            fade: None,
        }
    }

    /// Your message, then a reply that runs on well past it.
    fn turn() -> Turn {
        let settings = settings();
        let you = segment(
            Sender::You,
            "K5XYZ 5NN TT7",
            your_voice(&settings, 30.0),
            0.0,
        );
        let mut slow = resolve_station(&settings, &mut SplitMix64::new(3));
        slow.char_wpm = 12.0;
        slow.effective_wpm = 12.0;
        let reply = segment(Sender::Caller(0), "TU 5NN 214", slow, you.end_sec + 0.4);
        Turn {
            id: 1,
            at_run_ms: 0,
            segments: vec![you, reply],
        }
    }

    #[test]
    fn your_message_lasts_until_the_last_reply_ends() {
        let turn = turn();
        let planned = plan_turn(&turn, &settings());
        let reply_end = planned.others[0].duration_sec;
        assert!(reply_end > 0.0);
        assert!((planned.wanted.duration_sec - reply_end).abs() < 1e-9);
        assert!((planned.wanted.duration_sec - turn.duration_sec()).abs() < 1e-9);
        for other in &planned.others {
            assert!(other.duration_sec <= planned.wanted.duration_sec + 1e-9);
        }
    }

    #[test]
    fn no_reply_is_cut() {
        let turn = turn();
        let settings = settings();
        let planned = plan_turn(&turn, &settings);
        let reply = &turn.segments[1];
        let whole = plan_morse_playback_for(&reply.text, &settings, &reply.voice);
        assert_eq!(planned.others[0].events.len(), whole.events.len());
        let first = &planned.others[0].events[0];
        assert!(
            (first.start_sec - reply.start_sec).abs() < 1e-9,
            "it waits its delay"
        );
        for tone in &planned.others[0].events {
            assert!(tone.start_sec + tone.duration_sec <= planned.wanted.duration_sec + 1e-9);
        }
    }

    /// Your tones are a keyer's at the monitor's level: every one the same
    /// gain, its envelope never above it, nothing faded.
    #[test]
    fn your_tones_are_unfaded() {
        let planned = plan_turn(&turn(), &settings());
        let gain = MONITOR_LEVEL;
        assert!(!planned.wanted.events.is_empty());
        for tone in &planned.wanted.events {
            assert!((tone.target_gain - gain).abs() < 1e-12);
            let peak = tone.envelope.iter().fold(0.0f32, |a, v| a.max(*v));
            assert!((f64::from(peak) - gain).abs() < 1e-3, "peak {peak}");
            assert!((tone.frequency_hz - settings().side_tone_center()).abs() < 1e-9);
        }
        // A keyer's fist, whatever the fists of the band: every dit alike.
        let shortest = planned
            .wanted
            .events
            .iter()
            .map(|tone| tone.duration_sec)
            .fold(f64::MAX, f64::min);
        assert!((shortest - 1.2 / 30.0).abs() < 1e-9);
    }

    /// Fading as deep and as quick as the Band card goes.
    fn deep_fast_fade() -> Qsb {
        let band = crate::settings::BandSettings {
            qsb_enabled: true,
            qsb_depth: 1.0,
            qsb_rate_hz: crate::settings::QSB_RATE_MAX_HZ,
            ..crate::settings::BandSettings::default()
        };
        Qsb::of(&band).expect("fading is on")
    }

    /// The turn, its reply read from `from_sec` of a deep, quick fade.
    fn faded(from_sec: f64) -> Turn {
        let mut turn = turn();
        turn.segments[1].fade = Some(Fade {
            qsb: deep_fast_fade(),
            from_sec,
        });
        turn
    }

    /// A caller's fade is laid into each of its tones, point by point at the
    /// moment each point plays, every tone with an envelope of its own and
    /// its level scaled to match; it moves smoothly from point to point, and
    /// two callers read from their own points in it rise and fall apart.
    #[test]
    fn callers_fade_independently_and_smoothly() {
        let settings = settings();
        let qsb = deep_fast_fade();
        let plain = plan_turn(&turn(), &settings);
        let mut gains = Vec::new();
        for from_sec in [0.0, 37.3] {
            let planned = plan_turn(&faded(from_sec), &settings);
            let reply = &turn().segments[1];
            let (tones, unfaded) = (&planned.others[0].events, &plain.others[0].events);
            assert_eq!(tones.len(), unfaded.len());
            let mut heard = Vec::new();
            for (at, (tone, base)) in tones.iter().zip(unfaded).enumerate() {
                assert_eq!(tone.envelope.len(), base.envelope.len());
                // Timed from the reply's start, which the turn moved it to.
                let start = tone.start_sec - reply.start_sec;
                let points = tone.envelope.len() - 1;
                for (point, (level, level_then)) in
                    tone.envelope.iter().zip(base.envelope.iter()).enumerate()
                {
                    let t = from_sec + start + tone.duration_sec * point as f64 / points as f64;
                    let expected = level_then * qsb.gain_at(t);
                    assert!((level - expected).abs() < 1e-6, "tone {at}, point {point}");
                }
                let middle = qsb.gain_at(from_sec + start + tone.duration_sec / 2.0);
                assert!((tone.target_gain - base.target_gain * f64::from(middle)).abs() < 1e-9);
                for other in &tones[..at] {
                    assert!(
                        !Arc::ptr_eq(&tone.envelope, &other.envelope),
                        "one envelope a tone"
                    );
                }
                // Smoothly: from one point to the next the fade moves a
                // hair, never in a step.
                let steps = tone.envelope.iter().zip(base.envelope.iter());
                let ratios: Vec<f64> = steps
                    .filter(|(_, level)| **level > 0.05)
                    .map(|(faded, level)| f64::from(faded / level))
                    .collect();
                assert!(
                    ratios.windows(2).all(|w| (w[1] - w[0]).abs() < 0.01),
                    "tone {at}"
                );
                heard.push(f64::from(middle));
            }
            // Deep enough to be heard: it moves across the reply.
            let (low, high) = heard
                .iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), g| (lo.min(*g), hi.max(*g)));
            assert!(high - low > 0.05, "{from_sec}: {low:.2}..{high:.2}");
            gains.push(heard);
        }
        let apart = gains[0]
            .iter()
            .zip(&gains[1])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(apart > 0.2, "two callers faded together: {apart:.2}");
    }

    /// Faded callers or not, your message is never faded: your tones keep
    /// the monitor's level and share their envelopes, as a keyer's do.
    #[test]
    fn your_tones_stay_unfaded_beside_faded_callers() {
        let mut turn = faded(12.0);
        turn.segments[0].fade = Some(Fade {
            qsb: deep_fast_fade(),
            from_sec: 3.0,
        });
        let planned = plan_turn(&turn, &settings());
        let unfaded = plan_turn(&self::turn(), &settings());
        assert_eq!(planned.wanted.events.len(), unfaded.wanted.events.len());
        for (tone, base) in planned.wanted.events.iter().zip(&unfaded.wanted.events) {
            assert_eq!(tone.target_gain, MONITOR_LEVEL);
            assert_eq!(&tone.envelope[..], &base.envelope[..]);
        }
    }

    /// Your message is played past the receiver, and only yours: `wanted`
    /// holds your tones alone, at the monitor's level, and each caller comes
    /// through the receiver at its own, as a group session's every station
    /// does.
    #[test]
    fn your_message_is_dry_and_the_callers_are_not() {
        let turn = turn();
        let settings = settings();
        let planned = plan_turn(&turn, &settings);
        assert!(planned.dry_wanted);
        let (mine, reply) = (&turn.segments[0], &turn.segments[1]);
        let yours = plan_morse_playback_for(&mine.text, &settings, &mine.voice);
        assert_eq!(planned.wanted.events.len(), yours.events.len());
        assert_eq!(planned.others.len(), 1);
        let level = DEFAULT_TARGET_GAIN * reply.voice.volume;
        assert!(level < MONITOR_LEVEL);
        for tone in &planned.others[0].events {
            assert!((tone.target_gain - level).abs() < 1e-12);
        }

        // A turn of callers alone is still yours to monitor: nothing in it.
        let mut callers = turn.clone();
        callers.segments.remove(0);
        let alone = plan_turn(&callers, &settings);
        assert!(alone.dry_wanted && alone.wanted.events.is_empty());

        let transcript = turn.transcript().expect("a transcript");
        assert!(!plan_transmission(&transcript, &settings).dry_wanted);
    }

    /// Written down, a turn is your message and each reply after its delay;
    /// one of callers alone has no text of yours, and an empty one nothing.
    #[test]
    fn a_turns_transcript_holds_every_segment() {
        let turn = turn();
        let transcript = turn.transcript().expect("a transcript");
        assert_eq!(transcript.text, "K5XYZ 5NN TT7");
        assert_eq!(transcript.voice, turn.segments[0].voice);
        assert_eq!(transcript.others.len(), 1);
        let reply = &transcript.others[0];
        assert_eq!(reply.text, "TU 5NN 214");
        assert_eq!(reply.voice, turn.segments[1].voice);
        assert_eq!(reply.delay_sec, turn.segments[1].start_sec);

        let mut callers = turn.clone();
        callers.segments.remove(0);
        let alone = callers.transcript().expect("a transcript");
        assert_eq!(alone.text, "");
        assert_eq!(alone.voice, turn.segments[1].voice);
        assert_eq!(alone.others.len(), 1);

        callers.segments.clear();
        assert_eq!(callers.transcript(), None);
    }

    #[test]
    fn a_turn_of_callers_alone_has_a_silent_wanted() {
        let mut turn = turn();
        turn.segments.remove(0);
        let planned = plan_turn(&turn, &settings());
        assert!(planned.wanted.events.is_empty());
        assert!((planned.wanted.duration_sec - turn.duration_sec()).abs() < 1e-9);
        assert_eq!(planned.others.len(), 1);
    }

    /// Wherever a send is cut, what it had sent and what is sent again
    /// together hold every character, the one sounding sent again whole.
    #[test]
    fn a_continuation_keeps_every_character_of_your_message() {
        let settings = settings();
        let text = "K5XYZ 5NN TT7";
        let voice = your_voice(&settings, 25.0);
        let spans = char_spans(text, &settings, &voice);
        assert_eq!(spans.len(), text.chars().filter(|c| *c != ' ').count());
        let length = sending_sec(text, &settings, &voice);
        assert!((spans.last().map_or(0.0, |span| span.2) - length).abs() < 1e-9);
        for step in 0..=200 {
            let at_sec = length * f64::from(step) / 200.0 - 0.05;
            let sent: String = spans
                .iter()
                .filter(|(_, _, end)| *end <= at_sec)
                .map(|(at, _, _)| &text[*at..=*at])
                .collect();
            let rest = sounding_at(&spans, at_sec).map_or("", |from| &text[from..]);
            let whole: String = text.chars().filter(|c| *c != ' ').collect();
            let rest: String = rest.chars().filter(|c| *c != ' ').collect();
            assert_eq!(format!("{sent}{rest}"), whole, "cut at {at_sec:.3} s");
            // A character sounding at the cut goes again, from its start.
            if let Some((at, ..)) = spans.iter().find(|(_, s, e)| *s <= at_sec && at_sec < *e) {
                assert_eq!(sounding_at(&spans, at_sec), Some(*at));
            }
        }
        assert_eq!(sounding_at(&spans, length + 1.0), None);
        assert_eq!(sounding_at(&spans, -1.0), Some(0));
    }
}
