//! How loud a station comes out of the receiver as it is set, and so how far
//! full volume can lift what the receiver plays before the limiter would bend
//! the Morse.
//!
//! Worked out by listening rather than from a formula, because what sets it
//! is the receiver's own behaviour at the edges: the filter overshoots every
//! keying edge, by more the sharper the filter and the harder the edge, and
//! the AGC rides it all down by however far it stands over the knee. So the
//! strongest station there can be is sent through the same filter and the
//! same AGC the players run, look-ahead and all; and its loudest sample, with
//! the crests of the band's hiss on top of it, is what full volume is sized
//! by. The hiss's crests stay under the knee, so the AGC never hears them.
//! With the AGC off nothing rides a pile-up down, so there full volume
//! leaves room for the stations that can sound together.

use super::agc::{AGC_MAX_DUCK_DB, Agc, agc_lookahead_samples};
use super::calibration::{heard_floor_rms, noise_level};
use super::dsp::db_to_gain;
use super::filter::{FilterDesign, ReceiverFilter};
use super::knee::agc_knee;
use super::memo::{Exact, LastAnswer};
use super::output::SOFT_LIMIT_KNEE;
use crate::contest::RunStyle;
use crate::settings::{
    AgcDecay, CharSetMode, PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, STATIONS_MAX, STATIONS_MIN,
    STEEPNESS_MAX_MS, STEEPNESS_MIN_MS, TrainingSettings, VOLUME_MAX, WPM_MAX, WPM_MIN,
};
use crate::timing::{StationVoice, ToneEvent, plan_morse_playback_for};

/// The rate a station is listened to at. A peak is read off samples, and at
/// this rate the crest of the highest pitch the dial has falls less than 3%
/// between two of them; [`CLEAN_MARGIN`] takes that back.
const LISTEN_RATE: u32 = 32_000;

/// What the station sends: a dah from silence, as every over starts and the
/// AGC is widest open, then dits and dahs under its hang.
const LISTEN_TEXT: &str = "TEST";

/// The most of each pitch listened to. At the slowest speeds the text runs
/// to seconds, and its first elements have already shown everything there
/// is to hear.
const LISTEN_SEC: f64 = 2.0;

/// The pitches across the side-tone range a station is listened to at, as
/// shares of the way from its bottom to its top: a filter peaks wherever its
/// passband does, and a station can be drawn anywhere in the range.
const LISTEN_AT: [f64; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// How close to the limiter's knee full volume takes the loudest station:
/// a little short of it, for a crest between two samples, a pitch between
/// two listened to and a fist that is not a keyer's, which take 3%, and for
/// a station slower than the fastest, which is the one listened to. Slower,
/// its gaps run longer and the AGC has opened further by its next element,
/// whose first crests come out up to 7% over the fastest station's: at 9 to
/// 16 WPM with the fast decay, and at the slowest with the slow one
/// (`a_slower_station_stays_clean_at_full_volume`).
const CLEAN_MARGIN: f64 = 0.90;

/// How far over its RMS the band's hiss is allowed to reach on top of the
/// station: hiss through a filter is Gaussian, and a crest this high comes
/// once in a few thousand samples. Higher ones are rare and brief, and the
/// limiter's knee is soft enough that nothing of them is heard.
const HISS_CREST: f64 = 3.5;

/// How long the filter rings on after the last element listened to, and the
/// look-ahead holds the last of it back.
const RING_SEC: f64 = 0.05;

/// How far out of the filter the station has to have come, as a share of
/// the hiss's crest, before what plays counts as the station. Before that it
/// is the hiss alone, which on a loud-hiss band would decide full volume by
/// its crest at a gain the AGC never plays the station at; any more and the
/// first of the station's own crests on the noisiest bands go uncounted.
const STATION_OUT_OF_THE_HISS: f64 = 0.25;

/// What the loudest station is remembered by: everything the receiver and
/// the keying are made of, and nothing else. Not the RF gain: the station
/// is listened to at full, which is the loudest any RF gain plays it.
#[derive(Clone, Debug, PartialEq)]
struct LoudestAsk {
    receiver: FilterDesign,
    knee: Option<Exact>,
    decay: AgcDecay,
    noise: Exact,
    low_hz: Exact,
    high_hz: Exact,
    steepness_ms: Exact,
    smoothing: Exact,
    wpm: Exact,
}

impl LoudestAsk {
    fn of(settings: &TrainingSettings) -> Self {
        let receiver = FilterDesign::from_settings(settings);
        // The AGC the players run, whatever is on the band: none only with
        // it switched off.
        let knee = agc_knee(&settings.band, &receiver);
        let range = settings.side_tone_range();
        let held = |value: f64, low: f64, high: f64, fallback: f64| {
            if value.is_finite() {
                value.clamp(low, high)
            } else {
                fallback
            }
        };
        Self {
            knee: knee.map(Exact),
            decay: settings.band.agc_decay,
            noise: Exact(noise_level(&settings.band)),
            receiver,
            low_hz: Exact(*range.start()),
            high_hz: Exact(*range.end()),
            steepness_ms: Exact(held(
                settings.band.steepness,
                STEEPNESS_MIN_MS,
                STEEPNESS_MAX_MS,
                STEEPNESS_MIN_MS,
            )),
            smoothing: Exact(held(settings.band.envelope_smoothing, 0.0, 1.0, 0.0)),
            wpm: Exact(held(
                settings.playback.char_wpm_max,
                WPM_MIN,
                WPM_MAX,
                WPM_MAX,
            )),
        }
    }
}

/// The loudest a station at full strength comes out of the receiver these
/// settings describe, before the volume, as a peak of full scale: through
/// its filter at every pitch the side-tone range can draw, keyed as the
/// settings key it at the fastest speed they send, at full RF gain, ridden
/// by its AGC, with the band's hiss cresting on top of it. Backing the RF
/// gain off only ever plays it quieter: below the AGC's knee by as much,
/// above it by less, never by nothing. The static and the band's other
/// stations are left out: their crests are what the limiter is for.
///
/// Remembered, like the AGC's knee, because it takes a few milliseconds of
/// listening: moving anything but the receiver, the keying and the speed
/// asks the same question again.
pub fn loudest_station(settings: &TrainingSettings) -> f64 {
    static LAST: LastAnswer<LoudestAsk, f64> = LastAnswer::new();
    let ask = LoudestAsk::of(settings);
    LAST.get_or(ask.clone(), || listen(&ask, settings))
}

/// What full volume lifts the band, the stations and the receiver's own
/// hiss by, before the limiter: as far as takes the loudest a station comes
/// out of this receiver ([`loudest_station`]) to just short of where the
/// limiter starts to bend, so that no receiver and no volume ever bends the
/// Morse, and every receiver plays it as loud as it can clean at full RF
/// gain. It does not follow the RF gain: backed off, the band plays
/// quieter, as a rig's does.
///
/// Never more than the AGC ever takes off ([`AGC_MAX_DUCK_DB`]): here so a
/// receiver that somehow delivered nothing could not ask for an endless
/// gain.
///
/// With the AGC off, nothing rides a pile-up down where its stations'
/// elements overlap and their crests add, so full volume is turned down by
/// as many times as they can be louder together than the loudest station
/// alone ([`stations_together`]). A station alone plays as loud as it
/// would anyway, and with the AGC on so does a pile-up.
pub fn full_volume_gain(settings: &TrainingSettings) -> f64 {
    let loudest = loudest_station(settings);
    let alone = if loudest.is_finite() && loudest > 0.0 {
        (SOFT_LIMIT_KNEE * CLEAN_MARGIN / loudest).clamp(1.0, db_to_gain(AGC_MAX_DUCK_DB))
    } else {
        1.0
    };
    alone / stations_together(settings)
}

/// How many times as loud as the loudest station alone the stations that
/// can call at once come to together, at most: one with the AGC on, whose
/// ceiling rides their sum down, and one for a station alone. With it off,
/// in a group session, the station you copy and every other caller the
/// pile-up level under it as the filter passes them
/// ([`crate::Transmission::calling`]); in a contest's pile-up, every caller
/// the stations range lets call, each as strong as the top of the strength
/// range. QRM and the band's own stations are left out, as for one station.
fn stations_together(settings: &TrainingSettings) -> f64 {
    if settings.band.agc_enabled {
        return 1.0;
    }
    let most = f64::from(settings.band.stations_max.clamp(STATIONS_MIN, STATIONS_MAX));
    let top = *settings.volume_range().end() / VOLUME_MAX;
    let together = if settings.curriculum.char_set_mode == CharSetMode::Contest {
        match settings.contest.contest_style {
            RunStyle::Single => 1.0,
            RunStyle::Pileup => most * top,
        }
    } else {
        let down_db = settings
            .band
            .pileup_level_db
            .clamp(PILEUP_LEVEL_MIN_DB, PILEUP_LEVEL_MAX_DB);
        top * (1.0 + (most - 1.0) * db_to_gain(-down_db))
    };
    together.max(1.0)
}

fn listen(ask: &LoudestAsk, settings: &TrainingSettings) -> f64 {
    listen_at(ask, settings, ask.wpm.0)
}

/// The loudest a station at full strength comes out of the receiver `ask`
/// describes, sent at `wpm`, over the band's hiss.
fn listen_at(ask: &LoudestAsk, settings: &TrainingSettings, wpm: f64) -> f64 {
    let mut settings = settings.clone();
    settings.band.steepness = ask.steepness_ms.0;
    settings.band.envelope_smoothing = ask.smoothing.0;
    let (low, high) = (ask.low_hz.0, ask.high_hz.0);
    let crest = HISS_CREST * heard_floor_rms(ask.noise.0, ask.receiver.noise_bandwidth_hz());
    let agc = match ask.knee {
        Some(knee) => Agc::new(LISTEN_RATE, knee.0, ask.decay),
        None => Agc::off(LISTEN_RATE),
    };
    LISTEN_AT
        .iter()
        .map(|share| {
            let voice = StationVoice {
                tone_hz: low + (high - low) * share,
                char_wpm: wpm,
                effective_wpm: wpm,
                volume: VOLUME_MAX,
                weight: 1.0,
                dash_ratio: 3.0,
            };
            let plan = plan_morse_playback_for(LISTEN_TEXT, &settings, &voice);
            loudest_through(&plan.events, &ask.receiver, agc.clone(), crest)
        })
        .fold(0.0, f64::max)
}

/// The loudest sample of `tones` through `receiver` and `agc`, played the
/// look-ahead late at the gain the AGC rides at, as the native mixer plays
/// the send, with the crests of the hiss, `crest` high, on top of it at the
/// same gain, as the band rides with the station: from where the station
/// first plays [`STATION_OUT_OF_THE_HISS`] of a crest high on, through its
/// gaps and the ringing after it.
fn loudest_through(tones: &[ToneEvent], receiver: &FilterDesign, mut agc: Agc, crest: f64) -> f64 {
    let sr = f64::from(LISTEN_RATE);
    let mut filter = ReceiverFilter::new(LISTEN_RATE, receiver);
    let lookahead = agc_lookahead_samples(LISTEN_RATE);
    let mut waiting = vec![0.0; lookahead];
    let mut next = 0;
    let mut loudest = 0.0f64;
    let mut at = 0usize;
    let mut playing = false;
    let mut listen = |sample: f64| {
        let heard = filter.process(sample);
        let gain = agc.next_gain(heard);
        let due = std::mem::replace(&mut waiting[next], heard);
        next = (next + 1) % waiting.len();
        playing |= due.abs() > STATION_OUT_OF_THE_HISS * crest;
        if playing {
            loudest = loudest.max((due.abs() + crest) * gain);
        }
    };
    let end = (LISTEN_SEC * sr) as usize;
    for tone in tones {
        let start = (tone.start_sec * sr).round().max(0.0) as usize;
        if start >= end {
            break;
        }
        while at < start {
            listen(0.0);
            at += 1;
        }
        let len = ((tone.duration_sec * sr).round().max(0.0) as usize).max(1);
        let angular = std::f64::consts::TAU * tone.frequency_hz;
        for i in 0..len.min(end - start) {
            let t = i as f64 / sr;
            listen((angular * t).sin() * tone.envelope_at(t));
        }
        at = start + len;
    }
    for _ in 0..(RING_SEC * sr) as usize + lookahead {
        listen(0.0);
    }
    loudest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FilterShape;
    use crate::timing::PILEUP_CALLER_SEPARATION_HZ;

    fn gain(edit: impl FnOnce(&mut TrainingSettings)) -> f64 {
        let mut settings = TrainingSettings::default();
        edit(&mut settings);
        full_volume_gain(&settings.clamp())
    }

    /// The default receiver's AGC rides the strongest station down, so full
    /// volume lifts it further than a receiver without one. Not by all the
    /// AGC takes off the steady tone: it meets each over's first element
    /// wide open, and that onset is the loudest the station gets.
    #[test]
    fn full_volume_makes_up_what_the_agc_takes() {
        let with = gain(|_| {});
        let without = gain(|s| s.band.agc_enabled = false);
        assert!(
            with > without * 1.15,
            "{with:.2} with the AGC, {without:.2} without"
        );
        assert!(without > 1.5, "{without:.2}");
        let default = TrainingSettings::default();
        let heard = loudest_station(&default) * full_volume_gain(&default);
        assert!(
            (heard - SOFT_LIMIT_KNEE * CLEAN_MARGIN).abs() < 1e-9,
            "{heard}"
        );
    }

    /// With the AGC off nothing rides a pile-up down where its stations'
    /// elements overlap, so full volume leaves room for them: the station
    /// you copy at full strength and four callers, each as loud through the
    /// filter as the pile-up level lets one be, keyed together, stay under
    /// the limiter's knee. A station alone plays as before, and so does a
    /// pile-up with the AGC on; a contest's pile-up leaves room for each
    /// caller at full strength, and its single caller for one.
    #[test]
    fn with_the_agc_off_a_pileup_stays_under_the_limiter() {
        let pileup = |agc: bool| {
            let mut settings = TrainingSettings::default();
            settings.band.agc_enabled = agc;
            settings.band.stations_min = 5;
            settings.band.stations_max = 5;
            settings.clamp()
        };
        let alone = gain(|s| s.band.agc_enabled = false);
        assert_eq!(full_volume_gain(&pileup(true)), gain(|_| {}));
        let settings = pileup(false);
        let full = full_volume_gain(&settings);
        assert!(full < alone, "{full:.2} against {alone:.2} alone");
        let receiver = FilterDesign::from_settings(&settings);
        let centre = receiver.pitch_hz();
        let level = db_to_gain(-settings.band.pileup_level_db);
        let wpm = settings.playback.char_wpm_max;
        // The nearest a caller may sit, and the next caller beyond it.
        let (near, next) = (130.0, 130.0 + PILEUP_CALLER_SEPARATION_HZ);
        let voices = [0.0, -near, near, -next, next].map(|off| {
            let tone_hz = centre + off;
            let through = receiver.response_at(centre) / receiver.response_at(tone_hz);
            StationVoice {
                tone_hz,
                char_wpm: wpm,
                effective_wpm: wpm,
                volume: if off == 0.0 {
                    VOLUME_MAX
                } else {
                    (level * through).min(VOLUME_MAX)
                },
                weight: 1.0,
                dash_ratio: 3.0,
            }
        });
        let sr = f64::from(LISTEN_RATE);
        let mut summed = vec![0.0; (LISTEN_SEC * sr) as usize];
        for voice in &voices {
            for tone in plan_morse_playback_for(LISTEN_TEXT, &settings, voice).events {
                let start = (tone.start_sec * sr).round() as usize;
                let len = (tone.duration_sec * sr).round() as usize;
                let angular = std::f64::consts::TAU * tone.frequency_hz;
                for (i, sample) in summed.iter_mut().skip(start).take(len).enumerate() {
                    let t = i as f64 / sr;
                    *sample += (angular * t).sin() * tone.envelope_at(t);
                }
            }
        }
        let mut filter = ReceiverFilter::new(LISTEN_RATE, &receiver);
        let loudest = summed
            .iter()
            .map(|&sample| filter.process(sample).abs())
            .fold(0.0, f64::max);
        assert!(
            loudest * full < SOFT_LIMIT_KNEE,
            "the pile-up peaks {:.2} at full volume",
            loudest * full
        );
        assert!(
            loudest * alone > SOFT_LIMIT_KNEE,
            "sized for one station, it would not"
        );
        // A contest's pile-up of three, each caller as strong as the top
        // of the range, and a contest's single caller.
        let contest = |style: RunStyle| {
            gain(|s| {
                s.band.agc_enabled = false;
                s.band.stations_max = 3;
                s.curriculum.char_set_mode = CharSetMode::Contest;
                s.contest.contest_style = style;
            })
        };
        assert!((contest(RunStyle::Pileup) - alone / 3.0).abs() < 1e-9);
        assert_eq!(contest(RunStyle::Single), alone);
    }

    /// The hiss crests on top of the station: the more of it the receiver
    /// lets in, the less full volume lifts the band, down to none at all
    /// at the hardest noise, where its crests already reach the limiter as
    /// the band is calibrated.
    #[test]
    fn a_louder_hiss_leaves_full_volume_less_to_lift() {
        let quiet = gain(|_| {});
        let wide = gain(|s| s.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX);
        let loud = gain(|s| s.band.noise_level = 0.5);
        let hardest = gain(|s| s.band.noise_level = 1.0);
        assert!(quiet > wide && quiet > loud, "{quiet} {wide} {loud}");
        assert_eq!(hardest, 1.0);
    }

    /// The sharp filter at its narrowest, keyed at its hardest, overshoots
    /// every edge: full volume lifts it least.
    #[test]
    fn the_hardest_edges_through_the_sharpest_filter_are_lifted_least() {
        let soft = gain(|s| {
            s.band.agc_enabled = false;
            s.band.filter_shape = FilterShape::Soft;
        });
        let sharp = gain(|s| {
            s.band.agc_enabled = false;
            s.band.filter_width_hz = crate::settings::FILTER_WIDTH_MIN;
            s.band.side_tone_min = 600.0;
            s.band.side_tone_max = 600.0;
            s.band.steepness = STEEPNESS_MIN_MS;
        });
        assert!(sharp < soft, "{sharp} against {soft}");
        assert!(sharp > 1.5, "{sharp}");
    }

    /// Full volume is sized by the fastest station the settings send, and a
    /// slower one meets the AGC wider open after each of its longer gaps:
    /// at any speed down to the slowest, on the receivers where that opens
    /// it most, it still comes out with the room a crest between samples, a
    /// pitch between those listened to and a fist need.
    #[test]
    fn a_slower_station_stays_clean_at_full_volume() {
        for (decay, shape, width, steepness) in [
            (AgcDecay::Fast, FilterShape::Sharp, 100.0, 6.0),
            (AgcDecay::Fast, FilterShape::Peaked, 100.0, 6.0),
            (AgcDecay::Fast, FilterShape::Soft, 2_000.0, STEEPNESS_MIN_MS),
            (AgcDecay::Slow, FilterShape::Sharp, 100.0, 6.0),
            (
                AgcDecay::Slow,
                FilterShape::Sharp,
                2_000.0,
                STEEPNESS_MIN_MS,
            ),
            (AgcDecay::Slow, FilterShape::Soft, 2_000.0, STEEPNESS_MIN_MS),
        ] {
            let mut settings = TrainingSettings::default();
            settings.band.agc_decay = decay;
            settings.band.filter_shape = shape;
            settings.band.filter_width_hz = width;
            settings.band.steepness = steepness;
            settings.band.side_tone_min = 300.0;
            settings.band.side_tone_max = 600.0;
            settings.playback.char_wpm_max = 30.0;
            let settings = settings.clamp();
            let ask = LoudestAsk::of(&settings);
            let full = full_volume_gain(&settings);
            for wpm in [WPM_MIN, 7.0, 9.0, 11.0, 14.0, 16.0, 20.0, 25.0] {
                let heard = listen_at(&ask, &settings, wpm) * full;
                assert!(
                    heard < SOFT_LIMIT_KNEE * 0.97,
                    "{decay:?} {shape:?} {width} Hz, {steepness} ms, at {wpm} WPM: {heard:.4}"
                );
            }
        }
    }

    /// The hiss alone between overs is lifted by full volume with the rest,
    /// and its rarest crests, the one in a thousand ten milliseconds
    /// loudest, stay well under the limiter: on the widest filters, whose
    /// hiss is the most, and on the default receiver.
    #[test]
    fn between_overs_the_hiss_crests_stay_under_the_limiter() {
        use crate::band::BandMixer;
        use crate::settings::FilterShape;
        const SR: u32 = 48_000;
        for (shape, width) in [
            (FilterShape::Sharp, crate::settings::DEFAULT_FILTER_WIDTH_HZ),
            (FilterShape::Sharp, 2_000.0),
            (FilterShape::Soft, 2_000.0),
        ] {
            {
                let mut settings = TrainingSettings::default();
                settings.band.qrn_enabled = false;
                settings.band.filter_shape = shape;
                settings.band.filter_width_hz = width;
                let settings = settings.clamp();
                let full = full_volume_gain(&settings);
                let mut mixer = BandMixer::new(SR, &settings, 7);
                for _ in 0..SR {
                    mixer.next(0.0);
                }
                // Ten seconds of 10 ms stretches, each at its loudest.
                let frame = SR as usize / 100;
                let mut peaks: Vec<f64> = (0..1_000)
                    .map(|_| {
                        (0..frame)
                            .map(|_| (mixer.next(0.0).band * full).abs())
                            .fold(0.0, f64::max)
                    })
                    .collect();
                peaks.sort_by(f64::total_cmp);
                let rarest =
                    crate::band::gain_to_db(peaks[peaks.len() * 999 / 1000] / SOFT_LIMIT_KNEE);
                assert!(
                    rarest < -2.0,
                    "{shape:?} {width} Hz: the hiss crests {rarest:+.1} dB over the knee"
                );
            }
        }
    }

    /// Asked again of the same receiver, the answer is the one remembered,
    /// whatever else moved, the RF gain included; anything the receiver is
    /// made of asks anew.
    #[test]
    fn only_the_receiver_and_the_keying_ask_again() {
        let base = TrainingSettings::default();
        let mut louder = base.clone();
        louder.band.volume_min = 0.2;
        louder.band.qsb_depth = 0.9;
        louder.band.master_volume = 0.3;
        louder.band.rf_gain_db = -20.0;
        assert_eq!(LoudestAsk::of(&base), LoudestAsk::of(&louder));
        for edit in [
            (|s: &mut TrainingSettings| s.band.filter_width_hz = 500.0)
                as fn(&mut TrainingSettings),
            |s| s.band.steepness = 2.0,
            |s| s.band.side_tone_max = 900.0,
            |s| s.band.agc_enabled = false,
            |s| s.playback.char_wpm_max = 40.0,
        ] {
            let mut moved = base.clone();
            edit(&mut moved);
            assert_ne!(LoudestAsk::of(&base), LoudestAsk::of(&moved));
        }
    }
}
