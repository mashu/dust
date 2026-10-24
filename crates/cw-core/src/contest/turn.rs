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
//! Some flutter too, under the Band card's flutter, in the same way.
//!
//! Without full break-in the receiver is muted while you send, as a rig that
//! is not QSK mutes it: a turn's plan says until when
//! ([`PlannedTransmission::muted_until`]), and nothing a caller sends while it
//! is muted reaches you.

use std::sync::Arc;

use crate::band::Qsb;
use crate::morse::morse_for;
use crate::session::MonoMs;
use crate::settings::{TrainingSettings, VOLUME_MAX};
use crate::timing::{
    Interferer, PlannedTransmission, PlaybackPlan, StationVoice, ToneEvent, Transmission,
    plan_from, plan_morse_playback_at, plan_morse_playback_for,
};

/// The loudest you hear your own message, as a peak of full scale: your
/// sending at 0 dB, which the Tone and volume card turns down from here
/// ([`crate::ContestSettings::sending_level`]). It is played past the
/// receiver, as a rig's sidetone is, so no band setting moves its level or
/// takes any of it away.
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
///
/// All of that is the band as it is calibrated, before the volume. At full
/// volume ([`crate::band::FULL_VOLUME_GAIN`]) your message is lifted with
/// the callers, and from about −9 dB up it reaches the limiter's knee.
pub const MONITOR_LEVEL: f64 = 0.5;

/// Who sends a segment of a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sender {
    You,
    /// A caller, by its number in the run.
    Caller(u32),
    /// A station on your frequency that is not calling you, heard through
    /// the receiver as a caller is: the QRM of rough habits (§5.6 of the
    /// design). It never answers, and is no caller and no part of a QSO.
    Qrm,
}

impl Sender {
    /// Whether it is a caller: a station that may answer you.
    pub fn is_caller(self) -> bool {
        matches!(self, Self::Caller(_))
    }
}

/// How a caller's segment fades, or QRM's: the band's fading, as the Band
/// card sets it, and a caller's flutter, if it has one, read from
/// `from_sec` on its clock where the segment starts. Each station reads
/// them from a point of its own, so no two fade together.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    /// The band's fading; none with the Band card's QSB off.
    pub qsb: Option<Qsb>,
    /// The station's flutter; none for most, and for all with the Band
    /// card's flutter off.
    pub flutter: Option<Qsb>,
    pub from_sec: f64,
}

impl Fade {
    /// How a segment starting `start_sec` into a turn starting at `run_ms`
    /// fades, sent by a station whose path through the band's fading `qsb`,
    /// and its own `flutter`, starts `path_sec` in; none with neither.
    pub(super) fn along(
        qsb: Option<Qsb>,
        flutter: Option<Qsb>,
        path_sec: f64,
        run_ms: u64,
        start_sec: f64,
    ) -> Option<Self> {
        (qsb.is_some() || flutter.is_some()).then(|| Self {
            qsb,
            flutter,
            from_sec: path_sec + run_ms as f64 / 1_000.0 + start_sec,
        })
    }

    /// How much of the signal gets through `t_sec` into the segment: the
    /// band's fading and the flutter, one over the other.
    pub fn gain_at(&self, t_sec: f64) -> f32 {
        let at = self.from_sec + t_sec;
        let gain = |fading: Option<Qsb>| fading.map_or(1.0, |fading| fading.gain_at(at));
        gain(self.qsb) * gain(self.flutter)
    }
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
    /// How far into its text it starts, in seconds. A caller or QRM your
    /// key carries on sends the character sounding at the splice again
    /// whole; carried on again while that character is still going, it
    /// goes on from where it is rather than start it over, so keys over a
    /// station sending slower than you never hold it back ([`plan_turn`]).
    /// Nothing for a text sent from its start.
    pub skip_sec: f64,
}

/// One send: your message, if there is one, and the replies to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub id: u64,
    /// Run time when it starts: when it was keyed, or, for a continuation,
    /// when it takes over from the turn before ([`Self::starts_at`]).
    pub at_run_ms: u64,
    /// When its sound is to reach the ear, for a continuation: the moment
    /// the turn before has played your message up to where this one goes
    /// on with it, so no character of it goes out twice and none waits
    /// (§5.2 of the design). None for a turn that starts as soon as the
    /// player can start it.
    pub starts_at: Option<MonoMs>,
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

    /// Where your message in it ends, in seconds from its start; none for a
    /// turn of callers alone.
    pub fn your_end_sec(&self) -> Option<f64> {
        self.segments
            .iter()
            .filter(|segment| segment.who == Sender::You)
            .map(|segment| segment.end_sec)
            .reduce(f64::max)
    }

    /// Until when the receiver is muted in it, in seconds from its start:
    /// without full break-in (`qsk`), from the start of a turn holding a
    /// message of yours to that message's end, as a rig that is not QSK
    /// mutes it while it transmits. None with full break-in, or for a turn
    /// of callers alone.
    pub fn muted_until(&self, qsk: bool) -> Option<f64> {
        self.your_end_sec().filter(|_| !qsk)
    }

    /// Whether `segment` of it is lost to the receiver's gate
    /// ([`Self::muted_until`]): a caller's send, or QRM's, any of which
    /// falls while the receiver is muted is not heard. Nothing of it is: a
    /// call or an exchange heard in part is no call or exchange heard. Your
    /// own message is never lost to it.
    pub fn muted(&self, segment: &Segment, qsk: bool) -> bool {
        segment.who != Sender::You
            && self
                .muted_until(qsk)
                .is_some_and(|until| segment.start_sec < until)
    }

    /// What the turn says, as a send is written down: your message from your
    /// voice, and each reply, and any QRM, as a station calling across it
    /// after its delay.
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
/// [`plan_turn`] plays it at your sending's level whatever the volume
/// says: you hear your message as you send it, not through the receiver.
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

/// The gap `voice` leaves between the end of a word's last element and the
/// first of the next: where a message sent after one that has just ended
/// would start, as the two were one text.
pub(super) fn word_gap_sec(settings: &TrainingSettings, voice: &StationVoice) -> f64 {
    let spans = char_spans("E E", settings, voice);
    match spans[..] {
        [(_, _, first_end), (_, second_start, _)] => second_start - first_end,
        _ => 0.0,
    }
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
/// lasts. It is dry: played past the receiver at your sending's level
/// ([`crate::ContestSettings::sending_level`]), so the filter never shapes
/// it and the AGC never ducks a reply for it; with your sending off it has
/// no tones, and lasts as long all the same, so the turn keeps its time and
/// the heard timer with it. `others` are the replies, through the
/// receiver, each after its delay and never cut, which is why
/// [`crate::plan_transmission`], which cuts them where `wanted` ends, is
/// not used. A turn of callers alone has a `wanted` with no tones in it.
///
/// A caller's fade is laid into its tones' envelopes ([`fade_tones`]), as
/// both players play a tone at its envelope; the send itself carries no
/// fading, so none reaches your message.
///
/// Without full break-in the receiver is muted while you send
/// ([`PlannedTransmission::muted_until`]): from the turn's start to the end of
/// your message, your sending off or not, as a rig is muted while it
/// transmits whether or not you monitor it. The players mute the receiver's
/// output, after its AGC, and leave your message and the paddle's sidetone
/// alone.
pub fn plan_turn(turn: &Turn, settings: &TrainingSettings) -> PlannedTransmission {
    let level = settings.contest.sending_level();
    let mut wanted: Option<PlaybackPlan> = None;
    let mut others = Vec::new();
    for segment in &turn.segments {
        let mut plan = match segment.who {
            Sender::You => {
                let mut plan =
                    plan_morse_playback_at(&segment.text, settings, &segment.voice, level);
                // Silent, a tone is only work for the player: its time is
                // kept in the plan's length.
                if level <= 0.0 {
                    plan.events.clear();
                }
                plan
            }
            Sender::Caller(_) | Sender::Qrm => {
                let plan = plan_morse_playback_for(&segment.text, settings, &segment.voice);
                // A rest goes on from where it was.
                let mut plan = plan_from(plan, segment.skip_sec, settings);
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
            (Sender::Caller(_) | Sender::Qrm, _) => others.push(plan),
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
        muted_until: turn.muted_until(settings.contest.contest_qsk),
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
            fade.gain_at(tone.start_sec + tone.duration_sec * through)
        };
        let faded: Vec<f32> = tone
            .envelope
            .iter()
            .enumerate()
            .map(|(point, level)| level * at(point))
            .collect();
        tone.envelope = Arc::from(faded);
        tone.target_gain *= f64::from(fade.gain_at(tone.start_sec + tone.duration_sec / 2.0));
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
            skip_sec: 0.0,
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
            starts_at: None,
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

    /// A caller's rest that goes on from part way into its text plays the tones of
    /// the whole text from there, at the times they had: those over before
    /// it are gone, the one sounding goes on to its end with a rise of its
    /// own, and it lasts as long as its segment says.
    #[test]
    fn a_rest_goes_on_from_where_it_was() {
        let settings = settings();
        let voice = resolve_station(&settings, &mut SplitMix64::new(3));
        let whole = segment(Sender::Caller(0), "K5XYZ", voice, 0.0);
        let all = plan_turn(
            &Turn {
                id: 1,
                at_run_ms: 0,
                starts_at: None,
                segments: vec![whole.clone()],
            },
            &settings,
        );
        let tones = &all.others[0].events;
        // Part way into the third tone.
        let at = tones[2].start_sec + tones[2].duration_sec * 0.4;
        let rest = Segment {
            end_sec: whole.end_sec - at,
            skip_sec: at,
            ..whole.clone()
        };
        let turn = Turn {
            id: 2,
            at_run_ms: 0,
            starts_at: None,
            segments: vec![rest.clone()],
        };
        let planned = plan_turn(&turn, &settings);
        let played = &planned.others[0];
        assert!((played.duration_sec - (rest.end_sec - rest.start_sec)).abs() < 1e-9);
        assert_eq!(played.events.len(), tones.len() - 2);
        let sounding = &played.events[0];
        assert_eq!(sounding.start_sec, 0.0);
        let left = tones[2].duration_sec * 0.6;
        assert!((sounding.duration_sec - left).abs() < 1e-9);
        assert_eq!(sounding.envelope.first(), Some(&0.0), "it rises");
        assert!(sounding.envelope.iter().any(|level| *level > 0.0));
        for (later, was) in played.events[1..].iter().zip(&tones[3..]) {
            assert!((later.start_sec - (was.start_sec - at)).abs() < 1e-9);
            assert_eq!(later.duration_sec, was.duration_sec);
        }
        // Between two tones, nothing is cut, and a rest from its start is
        // the text as ever.
        let gap = tones[2].start_sec + tones[2].duration_sec + 1e-4;
        let between = Turn {
            segments: vec![Segment {
                end_sec: whole.end_sec - gap,
                skip_sec: gap,
                ..whole.clone()
            }],
            ..turn
        };
        let planned = plan_turn(&between, &settings);
        assert_eq!(planned.others[0].events.len(), tones.len() - 3);
        let from_start = Turn {
            segments: vec![whole],
            ..turn
        };
        assert_eq!(
            plan_turn(&from_start, &settings).others[0].events.len(),
            tones.len()
        );
    }

    /// The band makes room for the callers sounding at once, not for every
    /// send in the turn: two calling over each other, then one of them
    /// calling again once both are done, is two; a group's pile-up, all
    /// calling over the wanted station, is all of them.
    #[test]
    fn the_band_makes_room_for_the_callers_sounding_at_once() {
        let settings = settings();
        let voice = resolve_station(&settings, &mut SplitMix64::new(3));
        let first = segment(Sender::Caller(0), "K5XYZ", voice, 0.2);
        let over = segment(Sender::Caller(1), "W7ABC", voice, 0.5);
        let again = segment(Sender::Caller(0), "K5XYZ", voice, over.end_sec + 0.3);
        let turn = Turn {
            id: 1,
            at_run_ms: 0,
            starts_at: None,
            segments: vec![first, over, again],
        };
        let planned = plan_turn(&turn, &settings);
        assert_eq!(planned.others.len(), 3);
        assert_eq!(planned.others_at_once(), 2);
        let alone = Turn {
            segments: turn.segments[..1].to_vec(),
            ..turn.clone()
        };
        assert_eq!(plan_turn(&alone, &settings).others_at_once(), 1);
        let cq = Turn {
            segments: vec![segment(Sender::You, "CQ", your_voice(&settings, 30.0), 0.0)],
            ..turn
        };
        assert_eq!(plan_turn(&cq, &settings).others_at_once(), 0);
        let group = plan_transmission(
            &Transmission {
                text: "K5XYZ".to_string(),
                voice,
                others: (0..3)
                    .map(|k| Interferer {
                        text: "W7ABC".to_string(),
                        voice,
                        delay_sec: 0.1 * f64::from(k),
                    })
                    .collect(),
            },
            &settings,
        );
        assert_eq!(group.others_at_once(), 3);
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

    /// Your tones are a keyer's at your sending's level: every one the same
    /// gain, its envelope never above it, nothing faded.
    #[test]
    fn your_tones_are_unfaded() {
        let planned = plan_turn(&turn(), &settings());
        let gain = settings().contest.sending_level();
        assert!(
            gain > 0.0 && gain < MONITOR_LEVEL,
            "turned down from the top"
        );
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
            qsb: Some(deep_fast_fade()),
            flutter: None,
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
    /// your sending's level and share their envelopes, as a keyer's do.
    #[test]
    fn your_tones_stay_unfaded_beside_faded_callers() {
        let mut turn = faded(12.0);
        turn.segments[0].fade = Some(Fade {
            qsb: Some(deep_fast_fade()),
            flutter: None,
            from_sec: 3.0,
        });
        let planned = plan_turn(&turn, &settings());
        let unfaded = plan_turn(&self::turn(), &settings());
        assert_eq!(planned.wanted.events.len(), unfaded.wanted.events.len());
        for (tone, base) in planned.wanted.events.iter().zip(&unfaded.wanted.events) {
            assert_eq!(tone.target_gain, settings().contest.sending_level());
            assert_eq!(&tone.envelope[..], &base.envelope[..]);
        }
    }

    /// Your message is played past the receiver, and only yours: `wanted`
    /// holds your tones alone, at your sending's level, and each caller comes
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

    /// Your sending, from the loudest down to off: your tones at its level,
    /// their envelopes peaking there; off, none at all, and the turn as
    /// long as ever, the replies where they were.
    #[test]
    fn your_tones_play_at_your_sendings_level_and_none_when_off() {
        let turn = turn();
        let mut settings = settings();
        for db in [0, -9, -30] {
            settings.contest.my_sending_db = db;
            let level = settings.contest.sending_level();
            let planned = plan_turn(&turn, &settings);
            assert!(!planned.wanted.events.is_empty(), "{db} dB");
            for tone in &planned.wanted.events {
                assert!((tone.target_gain - level).abs() < 1e-12, "{db} dB");
                let peak = tone.envelope.iter().fold(0.0f32, |a, v| a.max(*v));
                assert!((f64::from(peak) - level).abs() < 1e-3, "{db} dB: {peak}");
            }
        }
        settings.contest.my_sending_db = 0;
        let loudest = plan_turn(&turn, &settings);
        assert!((loudest.wanted.events[0].target_gain - MONITOR_LEVEL).abs() < 1e-12);
        settings.contest.my_sending_db = crate::settings::MY_SENDING_OFF_DB;
        let off = plan_turn(&turn, &settings);
        assert!(off.dry_wanted && off.wanted.events.is_empty());
        assert!((off.wanted.duration_sec - loudest.wanted.duration_sec).abs() < 1e-12);
        assert!((off.wanted.duration_sec - turn.duration_sec()).abs() < 1e-9);
        assert_eq!(off.others.len(), loudest.others.len());
        assert_eq!(off.others[0].events.len(), loudest.others[0].events.len());
        assert_eq!(
            off.others[0].events[0].start_sec,
            loudest.others[0].events[0].start_sec
        );
        // A CQ nobody answers still lasts as long as it is sent.
        let cq = Turn {
            segments: vec![segment(
                Sender::You,
                "CQ DL1ABC TEST",
                your_voice(&settings, 30.0),
                0.0,
            )],
            ..turn
        };
        let alone = plan_turn(&cq, &settings);
        assert!(alone.wanted.events.is_empty());
        assert!((alone.wanted.duration_sec - cq.duration_sec()).abs() < 1e-9);
    }

    /// Without full break-in the receiver is muted from the turn's start to
    /// the end of your message, your sending off or not, and a caller's send
    /// any of which falls in that time is lost to it; a reply after your
    /// message is not. With full break-in nothing is muted, nor in a turn of
    /// callers alone.
    #[test]
    fn the_receiver_is_muted_while_you_send_without_full_break_in() {
        let mut turn = turn();
        let settings = settings();
        assert!(!settings.contest.contest_qsk, "off as it ships");
        let your_end = turn.segments[0].end_sec;
        assert_eq!(turn.your_end_sec(), Some(your_end));
        assert_eq!(turn.muted_until(false), Some(your_end));
        assert_eq!(plan_turn(&turn, &settings).muted_until, Some(your_end));
        let mut silent = settings.clone();
        silent.contest.my_sending_db = crate::settings::MY_SENDING_OFF_DB;
        assert_eq!(
            plan_turn(&turn, &silent).muted_until,
            Some(your_end),
            "you transmit all the same"
        );
        let mut qsk = settings.clone();
        qsk.contest.contest_qsk = true;
        assert_eq!(turn.muted_until(true), None);
        assert_eq!(plan_turn(&turn, &qsk).muted_until, None);

        // The reply after your message is heard; a caller sending across it
        // is not, though most of it comes after.
        let reply = turn.segments[1].clone();
        assert!(!turn.muted(&reply, false));
        let mut across = reply.clone();
        across.start_sec = your_end - 0.05;
        across.end_sec = across.start_sec + 2.0;
        assert!(turn.muted(&across, false));
        assert!(!turn.muted(&across, true), "heard through it with QSK");
        assert!(!turn.muted(&turn.segments[0], false), "never your own");
        turn.segments.push(across);

        // A continuation whose message starts a word space in is muted from
        // its start, the rig still sending.
        let mut continuation = turn.clone();
        continuation.segments[0].start_sec = 0.3;
        continuation.segments[0].end_sec += 0.3;
        assert_eq!(
            plan_turn(&continuation, &settings).muted_until,
            Some(your_end + 0.3)
        );

        let mut callers = turn;
        callers.segments.remove(0);
        assert_eq!(callers.your_end_sec(), None);
        assert_eq!(callers.muted_until(false), None);
        assert_eq!(plan_turn(&callers, &settings).muted_until, None);
        assert!(!callers.muted(&reply, false));
    }

    /// A fluttering caller's tones carry its flutter, laid in over the
    /// band's fading as the band's fading is, and a caller with flutter and
    /// no fading still flutters; your message flutters no more than it
    /// fades.
    #[test]
    fn a_flutter_is_laid_in_and_your_message_never_flutters() {
        let settings = settings();
        let flutter = Qsb::flutter(0.6, 25.0);
        let plain = plan_turn(&turn(), &settings);
        for qsb in [None, Some(deep_fast_fade())] {
            let mut fluttering = turn();
            let fade = Fade {
                qsb,
                flutter: Some(flutter),
                from_sec: 11.0,
            };
            fluttering.segments[1].fade = Some(fade);
            fluttering.segments[0].fade = Some(fade);
            let planned = plan_turn(&fluttering, &settings);
            let reply = &fluttering.segments[1];
            let (tones, unfaded) = (&planned.others[0].events, &plain.others[0].events);
            let mut heard = Vec::new();
            for (tone, base) in tones.iter().zip(unfaded) {
                let start = tone.start_sec - reply.start_sec;
                let points = tone.envelope.len() - 1;
                for (point, (level, level_then)) in
                    tone.envelope.iter().zip(base.envelope.iter()).enumerate()
                {
                    let t = start + tone.duration_sec * point as f64 / points as f64;
                    let both =
                        qsb.map_or(1.0, |qsb| qsb.gain_at(11.0 + t)) * flutter.gain_at(11.0 + t);
                    assert!((level - level_then * both).abs() < 1e-6);
                    assert!((fade.gain_at(t) - both).abs() < 1e-6);
                }
                heard.push(fade.gain_at(start + tone.duration_sec / 2.0));
            }
            let (low, high) = heard
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), g| (lo.min(*g), hi.max(*g)));
            assert!(high - low > 0.2, "it flutters: {low:.2}..{high:.2}");
            for (tone, base) in planned.wanted.events.iter().zip(&plain.wanted.events) {
                assert_eq!(&tone.envelope[..], &base.envelope[..], "yours unfluttered");
            }
        }
        // Neither fading nor flutter: no fade at all.
        assert_eq!(Fade::along(None, None, 3.0, 1_000, 0.5), None);
        let along = Fade::along(None, Some(flutter), 3.0, 1_000, 0.5).expect("a flutter");
        assert!((along.from_sec - 4.5).abs() < 1e-12);
    }

    /// The gap a voice leaves between two words: what a message sent after
    /// one that has just ended starts after.
    #[test]
    fn the_word_gap_is_what_a_space_leaves() {
        let settings = settings();
        let voice = your_voice(&settings, 25.0);
        let gap = word_gap_sec(&settings, &voice);
        let one = sending_sec("K", &settings, &voice);
        let two = sending_sec("K K", &settings, &voice);
        assert!((two - (2.0 * one + gap)).abs() < 1e-9);
        // Seven dots at a keyer's weight.
        assert!((gap - 7.0 * 1.2 / 25.0).abs() < 1e-9, "{gap}");
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
