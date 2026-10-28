//! How loud a station comes out of the receiver as it is set, and so how far
//! full volume can lift what the receiver plays before the limiter would bend
//! the Morse.
//!
//! Worked out by listening rather than from a formula, because what sets it
//! is the receiver's own behaviour at the edges: the filter overshoots every
//! keying edge, by more the sharper the filter and the harder the edge, and
//! the AGC rides it all down by however far it stands over the knee. So the
//! strongest station there can be is sent through the same filter and the
//! same AGC the players run, look-ahead and all, over the band's own hiss,
//! which the AGC hears with it and, with the threshold down in it, rides as
//! well; and its loudest sample, with the crests of that hiss on top of it,
//! is what full volume is sized by.

use super::agc::{AGC_MAX_DUCK_DB, Agc, agc_lookahead_samples};
use super::calibration::{heard_floor_rms, noise_floor_rms, noise_level};
use super::dsp::db_to_gain;
use super::filter::{FilterDesign, ReceiverFilter};
use super::knee::receiver_knee;
use super::memo::{Exact, LastAnswer};
use super::noise::NoiseFloor;
use super::output::SOFT_LIMIT_KNEE;
use crate::settings::{
    AgcDecay, STEEPNESS_MAX_MS, STEEPNESS_MIN_MS, TrainingSettings, VOLUME_MAX, WPM_MAX, WPM_MIN,
};
use crate::timing::{StationVoice, ToneEvent, plan_morse_playback_for};
use std::sync::Arc;

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

/// How long the AGC hears the band's hiss alone before the station's first
/// element: the band is always on, so with the threshold down in the hiss
/// the station meets an AGC already riding it. The AGC comes down onto the
/// hiss at its attack, from wide open, so a few of the hiss's own holds are
/// long enough for it to settle there.
const HISS_RUN_IN_SEC: f64 = 0.5;

/// The hiss the station is listened to over: any will do, the station
/// riding it deciding how loud it plays, and the same every time, so the
/// same settings are always sized alike.
const HISS_SEED: u64 = 0x4855_5353;

/// How long the filter rings on after the last element listened to, and the
/// look-ahead holds the last of it back.
const RING_SEC: f64 = 0.05;

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
        let knee = receiver_knee(&settings.band, &receiver);
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
/// gain off only ever plays it quieter: below the AGC's threshold by as
/// much, above it by less, never by nothing. The static and the band's other
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
/// Never more than the AGC ever takes off ([`AGC_MAX_DUCK_DB`]), which the
/// strongest station needs with the threshold all the way down: here so a
/// receiver that somehow delivered nothing could not ask for an endless
/// gain.
pub fn full_volume_gain(settings: &TrainingSettings) -> f64 {
    let loudest = loudest_station(settings);
    if loudest.is_finite() && loudest > 0.0 {
        (SOFT_LIMIT_KNEE * CLEAN_MARGIN / loudest).clamp(1.0, db_to_gain(AGC_MAX_DUCK_DB))
    } else {
        1.0
    }
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
    // The AGC run in on the hiss alone, and the hiss it hears under the
    // station after that, the same at every pitch: where the hiss can work
    // it, its crests reaching the knee. Under it, the hiss never qualifies
    // nor moves the law, and listening over it would change the answer by
    // nothing measurable; switched off, or with no hiss, there is nothing
    // for the AGC to hear but the station.
    let heard = match ask.knee {
        Some(knee) if crest > knee.0 => heard_hiss(ask),
        _ => Arc::from(Vec::new()),
    };
    let (before, under) = heard.split_at(hiss_run_in().min(heard.len()));
    let mut agc = match ask.knee {
        Some(knee) => Agc::new(LISTEN_RATE, knee.0, ask.decay),
        None => Agc::off(LISTEN_RATE),
    };
    for sample in before {
        agc.next_gain(*sample);
    }
    // The hiss on its own, between overs, its crests at the gain the AGC's
    // law holds it at: at full with the threshold clear of it, and taken
    // down with the rest with the threshold in it. Worked out rather than
    // read off a moment of the run-in, which would size the band by
    // whichever way that moment's hiss happened to fall.
    let alone = crest * agc.law(crest);
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
            loudest_through(&plan.events, &ask.receiver, agc.clone(), under, crest)
        })
        .fold(alone, f64::max)
}

/// How many samples the AGC hears the hiss alone for before the station.
fn hiss_run_in() -> usize {
    (HISS_RUN_IN_SEC * f64::from(LISTEN_RATE)) as usize
}

/// The band's hiss through the receiver `ask` describes, as long as the
/// run-in and the listening after it: remembered, because it depends on the
/// noise and the receiver alone, so moving the threshold, the decay or the
/// keying listens over the same hiss again.
fn heard_hiss(ask: &LoudestAsk) -> Arc<[f64]> {
    static LAST: LastAnswer<(Exact, FilterDesign), Arc<[f64]>> = LastAnswer::new();
    LAST.get_or((ask.noise, ask.receiver.clone()), || {
        let sr = f64::from(LISTEN_RATE);
        let listened = ((LISTEN_SEC + RING_SEC) * sr) as usize + agc_lookahead_samples(LISTEN_RATE);
        let mut floor = NoiseFloor::new(LISTEN_RATE, noise_floor_rms(ask.noise.0), HISS_SEED);
        let mut filter = ReceiverFilter::new(LISTEN_RATE, &ask.receiver);
        (0..hiss_run_in() + listened)
            .map(|_| filter.process(floor.next_sample()))
            .collect()
    })
}

/// The loudest sample of `tones` through `receiver` and `agc`, played the
/// look-ahead late at the gain the AGC rides at, as the native mixer plays
/// the send, with the crests of the hiss, `crest` high, on top of it at the
/// same gain, as the band rides with the station: from the station's first
/// sample played on, through its gaps and the ringing after it. The AGC
/// hears the station over `hiss`, the band's hiss through the receiver,
/// sample by sample.
fn loudest_through(
    tones: &[ToneEvent],
    receiver: &FilterDesign,
    mut agc: Agc,
    hiss: &[f64],
    crest: f64,
) -> f64 {
    let sr = f64::from(LISTEN_RATE);
    let mut filter = ReceiverFilter::new(LISTEN_RATE, receiver);
    let lookahead = agc_lookahead_samples(LISTEN_RATE);
    let mut waiting = vec![0.0; lookahead];
    let mut next = 0;
    let mut loudest = 0.0f64;
    let mut at = 0usize;
    let mut hiss = hiss.iter().copied();
    let mut playing = false;
    let mut listen = |sample: f64| {
        let heard = filter.process(sample);
        let gain = agc.next_gain(heard + hiss.next().unwrap_or(0.0));
        let due = std::mem::replace(&mut waiting[next], heard);
        next = (next + 1) % waiting.len();
        playing |= due != 0.0;
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

    /// Raising the AGC's threshold lets the strongest station through with
    /// less taken off, so full volume lifts it less, down to what it lifts a
    /// receiver with no AGC at all by once the station never reaches the
    /// threshold.
    #[test]
    fn raising_the_threshold_takes_full_volume_down_to_the_agc_off_gain() {
        let full = gain(|_| {});
        let backed_off = gain(|s| s.band.agc_threshold_db = 10.0);
        let all_the_way = gain(|s| s.band.agc_threshold_db = crate::settings::AGC_THRESHOLD_MAX_DB);
        let off = gain(|s| s.band.agc_enabled = false);
        assert!(
            full > backed_off && backed_off > all_the_way,
            "{full} {backed_off} {all_the_way}"
        );
        assert!(
            (all_the_way / off - 1.0).abs() < 0.02,
            "{all_the_way} against {off}"
        );
    }

    /// Taking the threshold down into the hiss has the AGC take more off the
    /// strongest station, and full volume makes all of it up, all the way
    /// down: the station plays as close to the limiter at the bottom of the
    /// range as at 0 dB, with the hiss holding the AGC under it and with no
    /// hiss at all. Never a cap reached, nor a hiss crest at a gain the AGC
    /// never plays it at.
    #[test]
    fn lowering_the_threshold_into_the_hiss_lifts_full_volume_all_the_way_down() {
        for noise in [true, false] {
            let at = |db: f64| {
                let mut settings = TrainingSettings::default();
                settings.band.noise_enabled = noise;
                settings.band.agc_threshold_db = db;
                settings.clamp()
            };
            let mut last = 0.0;
            for db in [
                0.0,
                -5.0,
                -10.0,
                -15.0,
                crate::settings::AGC_THRESHOLD_MIN_DB,
            ] {
                let settings = at(db);
                let full = full_volume_gain(&settings);
                assert!(
                    full > last * 1.25,
                    "noise {noise}, {db} dB: {full:.1} after {last:.1}"
                );
                let heard = loudest_station(&settings) * full;
                assert!(
                    (heard - SOFT_LIMIT_KNEE * CLEAN_MARGIN).abs() < 1e-9,
                    "noise {noise}, {db} dB: the station plays at {heard:.3}"
                );
                last = full;
            }
        }
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
    /// pitch between those listened to and a fist need; with the threshold
    /// down at the hiss as well. Further in, the slowest station's gaps are
    /// the hiss alone, held as the next test has it.
    #[test]
    fn a_slower_station_stays_clean_at_full_volume() {
        for threshold in [0.0, -10.0] {
            slower_stations_stay_clean_at(threshold);
        }
    }

    /// Down in the hiss, the hiss alone between overs is held by the AGC
    /// and lifted by full volume with the rest, and its rarest crests, the
    /// one in a thousand ten milliseconds loudest, reach no further into the
    /// limiter than the first two decibels of its soft knee, which nothing
    /// of them is heard through: on the widest filters, whose hiss is the
    /// most, and on the default receiver. Clear of the hiss, they stay well
    /// under it.
    #[test]
    fn down_in_the_hiss_its_crests_go_no_further_than_the_soft_knee() {
        use crate::band::BandMixer;
        use crate::settings::FilterShape;
        const SR: u32 = 48_000;
        for (shape, width) in [
            (FilterShape::Sharp, crate::settings::DEFAULT_FILTER_WIDTH_HZ),
            (FilterShape::Sharp, 2_000.0),
            (FilterShape::Soft, 2_000.0),
        ] {
            for (threshold, most_db) in [(0.0, -2.0), (-10.0, 2.0), (-14.0, 2.0), (-20.0, 2.0)] {
                let mut settings = TrainingSettings::default();
                settings.band.qrn_enabled = false;
                settings.band.filter_shape = shape;
                settings.band.filter_width_hz = width;
                settings.band.agc_threshold_db = threshold;
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
                    rarest < most_db,
                    "{shape:?} {width} Hz at {threshold} dB: the hiss crests {rarest:+.1} dB over the knee"
                );
            }
        }
    }

    fn slower_stations_stay_clean_at(threshold: f64) {
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
            settings.band.agc_threshold_db = threshold;
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
                    "{threshold} dB, {decay:?} {shape:?} {width} Hz, {steepness} ms, at {wpm} WPM: {heard:.4}"
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
            |s| s.band.agc_threshold_db = 6.0,
            |s| s.playback.char_wpm_max = 40.0,
        ] {
            let mut moved = base.clone();
            edit(&mut moved);
            assert_ne!(LoudestAsk::of(&base), LoudestAsk::of(&moved));
        }
    }
}
