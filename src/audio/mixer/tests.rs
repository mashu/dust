//! The mixer, listened to the way the stream plays it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use cw_core::band::{
    BandMixer, FilterDesign, Qsb, Receiver, SOFT_LIMIT_KNEE, agc_lookahead_samples,
};
use cw_core::{
    CALLSIGN_TIER_MAX, FILTER_WIDTH_MIN, MONITOR_LEVEL, PlannedTransmission, STATIONS_MAX, Segment,
    Sender, SplitMix64, StationVoice, TrainingSettings, Transmission, Turn, VOLUME_MAX, VOLUME_MIN,
    generate_callsign, plan_morse_playback_for, plan_transmission, plan_turn, resolve_station,
};

/// Low enough to keep the long listens quick; the band sounds the same at
/// any rate.
const SR: u32 = 16_000;

/// The default band with its static off: the steady hiss, whose level is
/// the thing to hear a seam in.
fn hiss() -> TrainingSettings {
    let mut settings = TrainingSettings::default();
    settings.band.qrn_enabled = false;
    settings.band.qsb_enabled = false;
    settings
}

fn band(settings: &TrainingSettings, seed: u64) -> BandVoice {
    BandVoice::new(BandMixer::new(SR, settings, seed), SR)
}

fn send(settings: &TrainingSettings, text: &str) -> SendVoice {
    let voice = resolve_station(settings, &mut SplitMix64::new(5));
    let plan = plan_transmission(&Transmission::alone(text, voice), settings);
    SendVoice::new(&plan, &FilterDesign::from_settings(settings), None, SR, 0.0)
}

/// How many samples the mixer plays everything late: the receiver's
/// look-ahead.
fn late() -> usize {
    agc_lookahead_samples(SR)
}

fn render(mixer: &mut Mixer, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    mixer.render(&mut out, 0);
    out
}

fn power(samples: &[f32]) -> f64 {
    samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// The furthest a waveform swings from nothing.
fn loudest(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |a, s| a.max(s.abs()))
}

/// The biggest change between neighbouring samples: where a waveform is cut
/// off, or a layer drops out, it steps.
fn biggest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// A band rebuilt — every slider move in the band preview does it — comes
/// in without a hole in the hiss. Switched in cold, a new band's filter was
/// empty and its AGC's look-ahead full of silence, and the old one faded out
/// under it: some ten decibels gone for a few milliseconds on every rebuild.
/// A straight-line crossfade of two unrelated noises sags 3 dB on its own.
///
/// Averaged over many rebuilds, in 5 ms windows, against the hiss before
/// each one.
#[test]
fn a_new_band_comes_in_without_a_hole_in_the_hiss() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(Some(band(&settings, 1)), &mut swept);
    render(&mut mixer, SR as usize / 2);
    let window = SR as usize / 200;
    let windows = 10;
    let swaps = 300;
    let (mut steady, mut around) = (0.0, vec![0.0; windows]);
    for k in 0..swaps {
        steady += power(&render(&mut mixer, window * windows));
        let mut swept = Swept::new();
        mixer.set_band(Some(band(&settings, 100 + k)), &mut swept);
        let after = render(&mut mixer, window * windows);
        for (w, chunk) in after.chunks(window).enumerate() {
            around[w] += power(chunk);
        }
    }
    let levels: Vec<f64> = around.iter().map(|p| db(p / steady)).collect();
    assert!(
        levels.iter().all(|level| level.abs() < 1.0),
        "the hiss through a rebuild, in dB against before it: {levels:.2?}"
    );
}

/// The volume and the AF gain set the level of everything the receiver
/// plays, the band and the Morse over it alike, and neither steps: the
/// volume moves in a straight line at the gate's pace, and the AF gain,
/// which can move tens of decibels as the receiver changes, glides there a
/// decibel at a time over a band's fade. A slider moved under a tone never
/// clicks. The AF gain comes ahead of the receiver's audio stage, whose
/// roll-off lags a glide by a few percent of the level and then holds it
/// exactly.
#[test]
fn the_volume_and_the_af_gain_move_all_the_receiver_plays_without_a_step() {
    // The AGC off, so the Morse plays at its own level rather than held
    // down to the knee's at a volume of one.
    let mut settings = hiss();
    settings.band.agc_enabled = false;
    // Moved once the band is in and the send has begun.
    let play = |af: f64, volume: f64| {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.set_band_at(Some(band(&settings, 9)), Gains::UNITY, &mut swept);
        mixer.start(send(&settings, "TEST"), &mut swept);
        render(&mut mixer, SR as usize / 10);
        mixer.set_gains(Gains {
            rf: 1.0,
            af,
            volume,
        });
        render(&mut mixer, SR as usize)
    };
    let plain = play(1.0, 1.0);
    assert!(loudest(&plain) > 0.1, "the Morse was heard");
    let fade = (BAND_FADE_SEC * f64::from(SR)).round();
    // The audio stage has caught up with the glide 30 ms after it ends.
    let caught_up = fade + 0.03 * f64::from(SR);
    let lag = 0.03 * f64::from(loudest(&plain));
    let lifted = play(4.0, 1.0);
    for (i, (a, b)) in plain.iter().zip(&lifted).enumerate() {
        let gain = 4f64.powf(((i + 1) as f64 / fade).min(1.0));
        let off = (f64::from(*b) - gain * f64::from(*a)).abs();
        let allowed = if i as f64 >= caught_up { 1e-5 } else { lag };
        assert!(off < allowed, "{off} off at {i}, the AF gain lifted");
    }
    let step = 1.0 / (GATE_RAMP_SEC * f64::from(SR));
    let quieter = play(1.0, 0.25);
    for (i, (a, b)) in plain.iter().zip(&quieter).enumerate() {
        let gain = (1.0 - step * (i + 1) as f64).max(0.25);
        let off = (f64::from(*b) - gain * f64::from(*a)).abs();
        assert!(off < 1e-5, "{off} off at {i}, the volume turned down");
    }
}

/// The RF gain turns down all the receiver hears ahead of its AGC, the band
/// and the station on it alike, and the AF gain does not make it up. With
/// the AGC off the station plays as many decibels down as the RF gain was
/// turned; with it on, by only a few, as the AGC lets go of what it was
/// taking off while the station is still over its knee; and backed off a
/// little, nothing plays louder than at full.
#[test]
fn the_rf_gain_turns_the_receiver_down_ahead_of_its_agc() {
    let mut settings = hiss();
    settings.band.link_volume = true;
    settings.band.volume_min = VOLUME_MAX;
    // How loud the station played on average, and its loudest sample.
    let play = |agc: bool, rf_gain_db: f64| {
        let mut settings = settings.clone();
        settings.band.agc_enabled = agc;
        settings.band.rf_gain_db = rf_gain_db;
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.set_band(Some(band(&settings, 3)), &mut swept);
        mixer.set_gains(Gains::of(&settings));
        render(&mut mixer, SR as usize / 2);
        mixer.start(send(&settings, "TEST"), &mut swept);
        let played = render(&mut mixer, SR as usize);
        (power(&played), f64::from(loudest(&played)))
    };
    let mut drops = Vec::new();
    for agc in [false, true] {
        let (full, full_peak) = play(agc, 0.0);
        let (backed, _) = play(agc, -20.0);
        drops.push(-db(backed / full));
        let (_, nearly_peak) = play(agc, -6.0);
        assert!(
            nearly_peak < full_peak,
            "the AGC {agc}: {nearly_peak:.3} backed off against {full_peak:.3}"
        );
    }
    let (off, on) = (drops[0], drops[1]);
    assert!(
        (19.0..20.5).contains(&off),
        "{off:.2} dB down with the AGC off"
    );
    assert!(
        on > 0.5 && on < 5.0,
        "{on:.2} dB down with the AGC on, {off:.2} off"
    );
}

/// The band preview's three CQs, loud, medium and weak, through the default
/// receiver at full volume: with the AGC on they come out within a couple of
/// decibels of each other; with it off as far apart as they were sent.
#[test]
fn the_agc_evens_out_the_previews_three_strengths() {
    use crate::playback::BAND_PREVIEW_STRENGTHS;
    // How loud a CQ at `strength` plays: the loudest of its 10 ms stretches
    // but the very loudest few, which in the gaps are the hiss.
    let played = |settings: &TrainingSettings, strength: f64| -> f64 {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.set_band(Some(band(settings, 3)), &mut swept);
        mixer.set_gains(Gains::of(settings));
        render(&mut mixer, SR as usize / 2);
        let voice = keyer(settings.side_tone_center(), strength);
        let plan = plan_transmission(&Transmission::alone("CQ CQ", voice), settings);
        let receiver = FilterDesign::from_settings(settings);
        mixer.start(SendVoice::new(&plan, &receiver, None, SR, 0.0), &mut swept);
        let frames = late() + (plan.wanted.duration_sec * f64::from(SR)) as usize;
        let mut peaks: Vec<f32> = render(&mut mixer, frames)
            .chunks(SR as usize / 100)
            .map(loudest)
            .collect();
        peaks.sort_by(f32::total_cmp);
        f64::from(peaks[peaks.len() * 9 / 10])
    };
    let spread = |settings: &TrainingSettings| -> [f64; 3] {
        let loud = played(settings, BAND_PREVIEW_STRENGTHS[0]);
        BAND_PREVIEW_STRENGTHS.map(|strength| db((played(settings, strength) / loud).powi(2)))
    };
    let on = spread(&hiss());
    assert!(on.iter().all(|db| *db > -2.5), "AGC on: {on:.2?} dB");
    let mut off = hiss();
    off.band.agc_enabled = false;
    let off = spread(&off);
    assert!(
        (off[1] + 10.5).abs() < 1.0 && (off[2] + 20.0).abs() < 1.0,
        "AGC off: {off:.2?} dB"
    );
}

/// A band with nothing on it but the station still has its receiver's AGC:
/// noise and static off, the preview's loud and weak CQs come out within a
/// few decibels of each other with it on, as a rig's AGC rides whatever
/// reaches it, and as far apart as they were sent with it off.
#[test]
fn the_agc_rides_a_station_on_a_quiet_band() {
    let quiet = |agc: bool| {
        let mut settings = hiss();
        settings.band.noise_enabled = false;
        settings.band.agc_enabled = agc;
        settings
    };
    let played = |settings: &TrainingSettings, strength: f64| -> f64 {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.set_gains(Gains::of(settings));
        mixer.set_band(BandVoice::for_settings(settings, SR, 3), &mut swept);
        render(&mut mixer, SR as usize / 4);
        let voice = keyer(settings.side_tone_center(), strength);
        let plan = plan_transmission(&Transmission::alone("CQ CQ", voice), settings);
        let receiver = FilterDesign::from_settings(settings);
        mixer.start(SendVoice::new(&plan, &receiver, None, SR, 0.0), &mut swept);
        let frames = late() + (plan.wanted.duration_sec * f64::from(SR)) as usize;
        let mut peaks: Vec<f32> = render(&mut mixer, frames)
            .chunks(SR as usize / 100)
            .map(loudest)
            .collect();
        peaks.sort_by(f32::total_cmp);
        f64::from(peaks[peaks.len() * 9 / 10])
    };
    let on = quiet(true);
    assert!(!on.band.has_background() && on.band.runs_receiver());
    let apart =
        |settings: &TrainingSettings| db((played(settings, 0.1) / played(settings, 1.0)).powi(2));
    let even = apart(&on);
    assert!(even > -3.0, "AGC on: the weak one {even:.2} dB");
    let off = quiet(false);
    assert!(
        BandVoice::for_settings(&off, SR, 3).is_none(),
        "nothing to run"
    );
    let raw = apart(&off);
    assert!(
        (raw + 20.0).abs() < 1.0,
        "AGC off: the weak one {raw:.2} dB"
    );
}

/// Switching the AGC off under a station, as the band preview invites,
/// plays it no louder than full volume plays any station clean: the AF
/// gain full volume sized for the receiver with the AGC on goes out with
/// that band, and the station eases from the one to the other over the
/// change, where it once rode the new band's open gain at the old AF gain,
/// twenty-odd decibels into the limiter for a sixth of a second.
#[test]
fn switching_the_agc_off_under_a_station_never_drives_it_into_the_limiter() {
    let on = hiss();
    let mut off = on.clone();
    off.band.agc_enabled = false;
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_gains(Gains::of(&on));
    mixer.set_band(BandVoice::for_settings(&on, SR, 3), &mut swept);
    render(&mut mixer, SR as usize / 2);
    let voice = keyer(on.side_tone_center(), VOLUME_MAX);
    let plan = plan_transmission(&Transmission::alone("TTTTTTTTTT", voice), &on);
    let receiver = FilterDesign::from_settings(&on);
    mixer.start(SendVoice::new(&plan, &receiver, None, SR, 0.0), &mut swept);
    let before = render(&mut mixer, SR as usize * 6 / 10);
    assert!(loudest(&before) > 0.6, "the station played full");
    // Both ways, the AGC on again as well.
    for (switch, to) in [(&off, "off"), (&on, "on")] {
        let band = BandVoice::for_settings(switch, SR, 3);
        mixer.set_band_at(band, Gains::of(switch), &mut swept);
        let after = render(&mut mixer, SR as usize * 3 / 10);
        let peak = f64::from(loudest(&after));
        assert!(
            peak < SOFT_LIMIT_KNEE * 1.05,
            "the AGC switched {to}: {peak:.3} into the limiter"
        );
    }
}

/// The RF gain turned up under a strong station, as a tap at the end of its
/// slider does, plays the station no louder than the AGC holds it: the send
/// waits out the look-ahead through the RF gain the AGC heard it at, as
/// the band does, rather than at the one it is played at, which the AGC
/// never heard and let through ten to twenty-five decibels over.
#[test]
fn turning_the_rf_gain_up_under_a_station_never_overshoots() {
    let settings = hiss();
    let voice = keyer(settings.side_tone_center(), VOLUME_MAX);
    let plan = plan_transmission(&Transmission::alone("TTTTTTTTTT", voice), &settings);
    let receiver = FilterDesign::from_settings(&settings);
    let mut backed = settings.clone();
    backed.band.rf_gain_db = -30.0;
    // A fresh receiver for every moment the RF gain comes up, through a
    // dah and the space after it.
    let play = |step_at: Option<usize>| {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        let first = if step_at.is_some() {
            &backed
        } else {
            &settings
        };
        mixer.set_gains(Gains::of(first));
        mixer.set_band(BandVoice::for_settings(first, SR, 3), &mut swept);
        render(&mut mixer, SR as usize / 2);
        mixer.start(SendVoice::new(&plan, &receiver, None, SR, 0.0), &mut swept);
        let settle = SR as usize / 2 + step_at.unwrap_or(0);
        render(&mut mixer, settle);
        mixer.set_gains(Gains::of(&settings));
        render(&mut mixer, SR as usize / 10)
    };
    let held = f64::from(loudest(&play(None)));
    for k in 0..12 {
        let step_at = k * SR as usize / 100;
        let peak = f64::from(loudest(&play(Some(step_at))));
        assert!(
            peak < held * 1.41,
            "{:.1} dB over the station held, the RF gain up {} ms on",
            db((peak / held).powi(2)),
            step_at * 1_000 / SR as usize
        );
    }
}

/// However high the rate a stream opens at, the send waits out as long a
/// look-ahead as the AGC listens ahead, so the AGC has heard every element
/// of it coming.
#[test]
fn the_look_ahead_is_the_agcs_at_any_rate() {
    for rate in [8_000, 48_000, 384_000, 705_600, 768_000] {
        let mut mixer = Mixer::new(rate);
        let expected = agc_lookahead_samples(rate) as f64 / f64::from(rate);
        assert_eq!(mixer.lookahead_sec(), expected, "made at {rate} Hz");
        let mut swept = Swept::new();
        mixer.clear(rate, &mut swept);
        assert_eq!(mixer.lookahead_sec(), expected, "reopened at {rate} Hz");
        let mut out = vec![0.0f32; 4_096];
        mixer.render(&mut out, 0);
    }
}

/// Full volume is as loud as the Morse plays clean, through whatever
/// receiver is set: a station at full strength, on its own, never reaches
/// where the limiter starts to bend, AGC or none, whatever the filter's
/// shape and width, the keying's edges or the pitch, and through every one
/// of them it comes close, full volume sized to that receiver.
#[test]
fn at_full_volume_no_station_reaches_the_limiter_and_every_one_comes_close() {
    use cw_core::FilterShape;
    for agc in [true, false] {
        for shape in [FilterShape::Peaked, FilterShape::Sharp, FilterShape::Soft] {
            for width in [FILTER_WIDTH_MIN, 2_000.0] {
                for (low, high) in [(300.0, 600.0), (200.0, 200.0), (1_200.0, 1_200.0)] {
                    let mut settings = hiss();
                    settings.band.agc_enabled = agc;
                    settings.band.filter_shape = shape;
                    settings.band.filter_width_hz = width;
                    settings.band.steepness = cw_core::STEEPNESS_MIN_MS;
                    settings.band.side_tone_min = low;
                    settings.band.side_tone_max = high;
                    settings.band.link_volume = true;
                    settings.band.volume_min = VOLUME_MAX;
                    let settings = settings.clamp();
                    assert_eq!(settings.master_volume(), 1.0, "full out of the box");
                    let receiver = FilterDesign::from_settings(&settings);
                    let mut loudest_here = 0.0f64;
                    for seed in 0..5 {
                        let mut mixer = Mixer::new(SR);
                        let mut swept = Swept::new();
                        mixer.set_band(Some(band(&settings, seed)), &mut swept);
                        mixer.set_gains(Gains::of(&settings));
                        render(&mut mixer, SR as usize / 2);
                        let voice = resolve_station(&settings, &mut SplitMix64::new(seed));
                        let plan =
                            plan_transmission(&Transmission::alone("TEST EE", voice), &settings);
                        mixer.start(SendVoice::new(&plan, &receiver, None, SR, 0.0), &mut swept);
                        let frames = ((plan.wanted.duration_sec + 0.2) * f64::from(SR)) as usize;
                        let peak = f64::from(loudest(&render(&mut mixer, frames)));
                        let at = format!("the AGC {agc}, {shape:?} {width} Hz, {low}-{high} Hz");
                        assert!(peak < SOFT_LIMIT_KNEE, "{peak:.3} with {at}, seed {seed}");
                        loudest_here = loudest_here.max(peak);
                    }
                    assert!(
                        loudest_here > 0.8 * SOFT_LIMIT_KNEE,
                        "only {loudest_here:.3} with the AGC {agc}, {shape:?} {width} Hz, {low}-{high} Hz"
                    );
                }
            }
        }
    }
}

/// The band preview's pause after its loudest call lets the band back at
/// Fast, so the hiss is heard returning and the next call starts on a gain
/// of its own, and at Slow it is still held down at the end of it, which is
/// what Slow is.
#[test]
fn the_preview_pause_lets_the_band_back_at_fast_and_holds_it_at_slow() {
    use crate::playback::{BAND_PREVIEW_CALL, BAND_PREVIEW_STRENGTHS, band_preview_gap_ms};
    // How far under the hiss before the loud call the hiss plays over the
    // last fifth of a second of the pause after it, in decibels.
    let held_down = |decay: cw_core::AgcDecay| -> f64 {
        let mut settings = hiss();
        settings.band.agc_decay = decay;
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.set_band(Some(band(&settings, 3)), &mut swept);
        mixer.set_gains(Gains::of(&settings));
        render(&mut mixer, SR as usize / 2);
        let before = power(&render(&mut mixer, SR as usize / 2));
        let voice = keyer(settings.side_tone_center(), BAND_PREVIEW_STRENGTHS[0]);
        let plan = plan_transmission(&Transmission::alone(BAND_PREVIEW_CALL, voice), &settings);
        let receiver = FilterDesign::from_settings(&settings);
        mixer.start(SendVoice::new(&plan, &receiver, None, SR, 0.0), &mut swept);
        let sent = late() + (plan.sound_end_sec() * f64::from(SR)) as usize;
        render(&mut mixer, sent);
        let pause = render(
            &mut mixer,
            band_preview_gap_ms() as usize * SR as usize / 1_000,
        );
        let last = &pause[pause.len() - SR as usize / 5..];
        -db(power(last) / before)
    };
    let fast = held_down(cw_core::AgcDecay::Fast);
    assert!(fast < 1.0, "Fast still holds the band {fast:.1} dB down");
    let slow = held_down(cw_core::AgcDecay::Slow);
    assert!(slow > 6.0, "Slow let the band back to {slow:.1} dB down");
}

/// Bands rebuilt faster than one can fade out each fade out in full: none is
/// dropped part of the way down, which is a step in the waveform.
#[test]
fn bands_rebuilt_in_quick_succession_all_fade_out() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(Some(band(&settings, 1)), &mut swept);
    let steady = biggest_step(&render(&mut mixer, SR as usize * 2));
    let mut worst = 0.0f32;
    for k in 0..40 {
        let mut through = Vec::new();
        for j in 0..3 {
            let mut swept = Swept::new();
            mixer.set_band(Some(band(&settings, 10 * k + j)), &mut swept);
            through.extend(render(&mut mixer, SR as usize / 500));
        }
        through.extend(render(&mut mixer, SR as usize / 20));
        worst = worst.max(biggest_step(&through));
    }
    assert!(
        worst < 1.5 * steady,
        "rebuilds stepped by {worst}, against {steady} in the hiss itself"
    );
}

/// A send started while the last one is still sounding lets the last one
/// ride down over its release rather than cutting it off — whatever point
/// of its cycle it was at.
#[test]
fn a_new_send_lets_the_last_one_ride_out_its_release() {
    let settings = hiss();
    for offset in 0..6 {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.start(send(&settings, "T"), &mut swept);
        // Into the middle of the dah, at a different point of its cycle
        // each time.
        let into = render(&mut mixer, SR as usize / 10 + 3 * offset);
        assert!(into.iter().any(|s| s.abs() > 0.1), "the dah is sounding");
        mixer.start(send(&settings, "T"), &mut swept);
        let mut across = vec![into[into.len() - 1]];
        across.extend(render(&mut mixer, SR as usize / 50));
        // A 600 Hz tone at full level moves at most a quarter of its height
        // between samples at this rate; a cut is a step of all of it.
        let step = biggest_step(&across);
        assert!(step < 0.1, "offset {offset}: a step of {step}");
    }
}

/// A send stopped part way leaves nothing of itself in the receiver: the
/// band comes back up as if it had never been ducked. A send stopped mid-way
/// used to leave its level with the AGC for good, and the band sat 4 dB
/// down until the next send overwrote it.
#[test]
fn a_stopped_send_leaves_the_band_as_it_was() {
    let settings = hiss();
    let seconds = |s: f64| (s * f64::from(SR)) as usize;
    let (mut heard, mut untouched) = (Mixer::new(SR), Mixer::new(SR));
    let mut swept = Swept::new();
    heard.set_band(Some(band(&settings, 7)), &mut swept);
    untouched.set_band(Some(band(&settings, 7)), &mut swept);
    heard.start(send(&settings, "TTTTTTTTTT"), &mut swept);
    render(&mut heard, seconds(0.5));
    render(&mut untouched, seconds(0.5));
    heard.stop();
    render(&mut heard, seconds(2.0));
    render(&mut untouched, seconds(2.0));
    let (a, b) = (
        render(&mut heard, seconds(1.0)),
        render(&mut untouched, seconds(1.0)),
    );
    let level = db(power(&a) / power(&b));
    assert!(
        level.abs() < 0.2,
        "two seconds after the stop the band was {level:.2} dB off"
    );
}

/// A send is over once its last sample has been heard, `latency` frames
/// after it was played.
#[test]
fn a_send_finishes_when_it_has_been_heard() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    let voice = send(&settings, "E");
    let finished = voice.finished_flag();
    mixer.start(voice, &mut swept);
    let latency = SR as usize / 10;
    let mut played = 0;
    while !finished.load(Ordering::Acquire) && played < SR as usize * 2 {
        let mut out = vec![0.0f32; 64];
        mixer.render(&mut out, latency);
        played += 64;
    }
    let expected = send(&settings, "E");
    let mut alone = Mixer::new(SR);
    let flag = expected.finished_flag();
    alone.start(expected, &mut swept);
    let mut unheard = 0;
    while !flag.load(Ordering::Acquire) && unheard < SR as usize * 2 {
        let mut out = vec![0.0f32; 64];
        alone.render(&mut out, 0);
        unheard += 64;
    }
    assert!(
        played >= unheard + latency && played <= unheard + latency + 64,
        "finished after {played} frames, {unheard} with no latency"
    );
    // And the next send goes to the output after the look-ahead, and is
    // reckoned heard as far behind that as the speaker was.
    let ahead = late() as f64 / f64::from(SR);
    assert!((mixer.lookahead_sec() - ahead).abs() < 1e-12);
    assert!((mixer.output_latency_sec() - 0.1).abs() < 1e-9);
    assert!(alone.output_latency_sec().abs() < 1e-12);
}

/// A keyer at `tone_hz` and `volume`, at 25 WPM.
fn keyer(tone_hz: f64, volume: f64) -> StationVoice {
    StationVoice {
        tone_hz,
        char_wpm: 25.0,
        effective_wpm: 25.0,
        volume,
        weight: 1.0,
        dash_ratio: 3.0,
    }
}

/// `who` sending `text` in a contest's turn, from `start_sec`.
fn segment(
    settings: &TrainingSettings,
    who: Sender,
    text: &str,
    voice: StationVoice,
    start_sec: f64,
) -> Segment {
    let end_sec = start_sec + plan_morse_playback_for(text, settings, &voice).duration_sec;
    Segment {
        who,
        text: text.into(),
        voice,
        start_sec,
        end_sec,
        fade: None,
        skip_sec: 0.0,
    }
}

/// A contest's turn, as the run plans it.
fn planned_turn(settings: &TrainingSettings, segments: &[&Segment]) -> PlannedTransmission {
    let turn = Turn {
        id: 1,
        at_run_ms: 0,
        starts_at: None,
        segments: segments.iter().map(|segment| (*segment).clone()).collect(),
    };
    plan_turn(&turn, settings)
}

/// What the mixer plays of `plan` over `band`, if there is one, from its
/// start to a little past its end: from where its first sample is heard,
/// the look-ahead after it was started.
fn hear(
    settings: &TrainingSettings,
    plan: &PlannedTransmission,
    band: Option<BandVoice>,
) -> Vec<f32> {
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(band, &mut swept);
    let receiver = FilterDesign::from_settings(settings);
    mixer.start(SendVoice::new(plan, &receiver, None, SR, 0.0), &mut swept);
    let frames = ((plan.wanted.duration_sec + 0.1) * f64::from(SR)) as usize;
    render(&mut mixer, late() + frames).split_off(late())
}

/// Your message in a contest is not received, so the receiver's filter
/// takes none of it: sent a kilohertz off the narrowest filter, which all
/// but silences it through the receiver, it is heard at the monitor's level
/// with your sending at its loudest.
#[test]
fn your_message_is_not_filtered() {
    let mut settings = hiss();
    settings.contest.my_sending_db = 0;
    settings.band.filter_width_hz = FILTER_WIDTH_MIN;
    let off = settings.side_tone_center() + 1_000.0;
    let mine = segment(&settings, Sender::You, "TTT", keyer(off, 1.0), 0.0);
    let dry = planned_turn(&settings, &[&mine]);
    let monitored = f64::from(loudest(&hear(&settings, &dry, None)));
    assert!(
        (monitored - MONITOR_LEVEL).abs() < 0.02 * MONITOR_LEVEL,
        "your message peaked at {monitored}"
    );
    let received = PlannedTransmission {
        dry_wanted: false,
        ..dry
    };
    let filtered = f64::from(loudest(&hear(&settings, &received, None)));
    assert!(
        filtered < 0.1 * MONITOR_LEVEL,
        "through the receiver it peaked at {filtered}"
    );
}

/// Your sending sets the level your message is heard at, the monitor's
/// turned down by its decibels, the dry sample and nothing else: the
/// replies play as they would at any level of it; off, your message is
/// silence, and the send lasts as long, its reply where it was.
#[test]
fn your_sending_sets_the_dry_level_alone() {
    let mut settings = hiss();
    let centre = settings.side_tone_center();
    let mine = segment(&settings, Sender::You, "TTT", keyer(centre, 1.0), 0.0);
    let reply = segment(
        &settings,
        Sender::Caller(0),
        "K5XYZ",
        keyer(centre, 1.0),
        mine.end_sec + 0.2,
    );
    let mut loudest_reply = None;
    for db in [0, -12, -30, cw_core::MY_SENDING_OFF_DB] {
        settings.contest.my_sending_db = db;
        let level = settings.contest.sending_level();
        let yours = planned_turn(&settings, &[&mine]);
        let heard = f64::from(loudest(&hear(&settings, &yours, None)));
        if db == cw_core::MY_SENDING_OFF_DB {
            assert_eq!(heard, 0.0, "off is silence");
        } else {
            assert!(
                (heard - level).abs() < 0.02 * level,
                "{db} dB: {heard} for {level}"
            );
        }
        let turn = planned_turn(&settings, &[&mine, &reply]);
        let played = hear(&settings, &turn, None);
        let sr = f64::from(SR);
        let from = (reply.start_sec * sr) as usize;
        let tail = played[from..].to_vec();
        match &loudest_reply {
            None => loudest_reply = Some((played.len(), tail)),
            Some((length, before)) => {
                assert_eq!(played.len(), *length, "{db} dB: the send's length");
                assert_eq!(&tail, before, "{db} dB: the reply as ever");
            }
        }
    }
}

/// The AGC never hears your message, so the weakest reply right after it
/// comes in as it would after silence: over the reply's first 0.3 s the
/// receiver plays exactly what it plays for the reply alone. Through the
/// receiver, as your message used to go, it ducked them.
#[test]
fn your_message_never_ducks_the_reply_after_it() {
    let settings = hiss();
    let centre = settings.side_tone_center();
    let mine = segment(
        &settings,
        Sender::You,
        "K5XYZ 5NN TT7",
        keyer(centre, 1.0),
        0.0,
    );
    let weak = keyer(centre, VOLUME_MIN);
    let reply = segment(
        &settings,
        Sender::Caller(0),
        "TU 5NN 214",
        weak,
        mine.end_sec + 0.1,
    );
    let sr = f64::from(SR);
    let first = (reply.start_sec * sr) as usize..((reply.start_sec + 0.3) * sr) as usize;
    let on_band = || Some(band(&settings, 3));

    let after = hear(
        &settings,
        &planned_turn(&settings, &[&mine, &reply]),
        on_band(),
    );
    let alone = hear(&settings, &planned_turn(&settings, &[&reply]), on_band());
    assert_eq!(after[first.clone()], alone[first.clone()]);

    let mut received = planned_turn(&settings, &[&mine, &reply]);
    received.dry_wanted = false;
    let ducked = hear(&settings, &received, on_band());
    let level = db(power(&ducked[first.clone()]) / power(&alone[first]));
    assert!(
        level < -1.0,
        "through the receiver the reply came in {level:.2} dB down"
    );
}

/// Your message joins after the AGC, not under its gain: on a band whose
/// static, and a caller at full level calling across you, pull the gain
/// well down, what your message adds to the receiver's output is its own
/// tones at your sending's level, sample for sample, the band heard under it
/// with full break-in. Ridden by the gain, it would duck on every crash.
#[test]
fn your_message_is_not_ridden_by_the_agc() {
    let mut settings = TrainingSettings::default();
    settings.band.qsb_enabled = false;
    settings.band.qrn_level = 1.0;
    // With full break-in, so the band is heard under your message.
    settings.contest.contest_qsk = true;
    let centre = settings.side_tone_center();
    let mine = segment(
        &settings,
        Sender::You,
        "K5XYZ 5NN TT7",
        keyer(centre, 1.0),
        0.0,
    );
    let loud = segment(
        &settings,
        Sender::Caller(0),
        "K5XYZ K5XYZ K5XYZ",
        keyer(centre, VOLUME_MAX),
        0.2,
    );
    assert!(loud.end_sec > mine.end_sec);
    let both = planned_turn(&settings, &[&mine, &loud]);
    let callers = planned_turn(&settings, &[&loud]);
    let on_band = || Some(band(&settings, 3));

    let with = hear(&settings, &both, on_band());
    let without = hear(&settings, &callers, on_band());
    let alone = PlannedTransmission {
        others: Vec::new(),
        ..both
    };
    let yours = hear(&settings, &alone, None);
    assert_eq!(with.len(), without.len());
    for (i, ((a, b), y)) in with.iter().zip(&without).zip(&yours).enumerate() {
        assert!((a - b - y).abs() < 1e-6, "at {i}: {} against {y}", a - b);
    }

    // The gain it was not under, as the band rides the caller alone.
    let mut ridden = band(&settings, 3);
    let receiver = FilterDesign::from_settings(&settings);
    let mut voice = SendVoice::new(&callers, &receiver, None, SR, 0.0);
    ridden.reserve_stations(voice.callers_now());
    let lowest = (0..(mine.end_sec * f64::from(SR)) as usize)
        .map(|_| ridden.next(voice.next().heard, 1.0).gain)
        .fold(1.0, f64::min);
    assert!(
        lowest < 0.5,
        "under your message the gain fell only to {lowest}"
    );
}

/// A turn stretched to its last reply plays until that reply has been
/// heard, though your message ended long before it: dry or not, a send is
/// as long as its plan.
#[test]
fn a_stretched_turn_finishes_once_its_last_reply_is_heard() {
    let settings = hiss();
    let centre = settings.side_tone_center();
    let mine = segment(&settings, Sender::You, "TU", keyer(centre, 1.0), 0.0);
    let slow = StationVoice {
        char_wpm: 12.0,
        effective_wpm: 12.0,
        ..keyer(centre, 1.0)
    };
    let reply = segment(
        &settings,
        Sender::Caller(0),
        "K5XYZ",
        slow,
        mine.end_sec + 0.3,
    );
    let dry = planned_turn(&settings, &[&mine, &reply]);
    assert!(dry.wanted.duration_sec > mine.end_sec + 1.0);
    let latency = SR as usize / 10;
    let finishing = |plan: &PlannedTransmission| {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        let voice = SendVoice::new(plan, &FilterDesign::from_settings(&settings), None, SR, 0.0);
        let finished = voice.finished_flag();
        mixer.start(voice, &mut swept);
        let mut played = Vec::new();
        while !finished.load(Ordering::Acquire) && played.len() < SR as usize * 10 {
            let mut out = vec![0.0f32; 64];
            mixer.render(&mut out, latency);
            played.extend(out);
        }
        played
    };

    let played = finishing(&dry);
    let sr = f64::from(SR);
    let ends = (dry.wanted.duration_sec * sr).ceil() as usize;
    assert!(
        played.len() >= ends + latency && played.len() <= ends + latency + SR as usize / 10,
        "finished after {} frames, its reply heard by {}",
        played.len(),
        ends + latency
    );
    let last = dry.others[0].events.last().expect("a reply");
    let from = (last.start_sec * sr) as usize;
    assert!(loudest(&played[from..ends]) > 0.1, "the reply was cut");
    let received = PlannedTransmission {
        dry_wanted: false,
        ..dry
    };
    assert_eq!(finishing(&received).len(), played.len());
}

/// The output stage catches what is too loud rather than squaring it off:
/// the same curve the band is calibrated through, applied once to the sum.
#[test]
fn the_output_stage_catches_rather_than_clips() {
    assert_eq!(finish(0.5), 0.5, "below the knee it does nothing");
    assert_eq!(finish(-0.5), -0.5);
    assert!(finish(1.4) > 0.9 && finish(1.4) < 1.0);
    assert!(finish(2.0) > finish(1.4), "limiting, not clipping flat");
    assert!(finish(-3.0) < -0.9 && finish(-3.0) >= -1.0);
    assert_eq!(finish(f32::NAN), 0.0);
    assert_eq!(finish(f32::INFINITY), 0.0);
}

/// The audio thread never waits: a callback that finds the UI holding the
/// mixer goes without it for that buffer, and the paddle's sidetone, which
/// needs no lock, plays regardless. A lock poisoned elsewhere is still a
/// good one.
#[test]
fn the_stream_never_waits_for_the_mixer() {
    let settings = hiss();
    let mixer = Arc::new(Mutex::new(Mixer::new(SR)));
    let mut swept = Swept::new();
    mixer
        .lock()
        .expect("mixer")
        .start(send(&settings, "TTT"), &mut swept);
    let control = Arc::new(SidetoneControl::new());
    let failed = Arc::new(AtomicBool::new(false));
    let mut output = Output::new(
        Arc::clone(&mixer),
        Arc::clone(&control),
        Arc::clone(&failed),
        SR,
    );

    let mut clock = OnTime::now();
    control.set(crate::audio::ToneRequest::new(true, 700.0, 0.2));
    // A few buffers: the sidetone's edge is played a buffer late.
    let mut out = vec![0.0f32; 512];
    {
        let _held = mixer.lock().expect("mixer");
        for chunk in out.chunks_mut(128) {
            clock.fill(&mut output, chunk, 0);
        }
    }
    let peak = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    assert!(
        peak > 0.15 && peak <= 0.2 + 1e-6,
        "the sidetone alone, peak {peak}"
    );

    let poisoner = Arc::clone(&mixer);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("poison the lock");
    })
    .join();
    assert!(mixer.is_poisoned());
    control.off();
    let mut out = vec![0.0f32; SR as usize / 5];
    clock.fill(&mut output, &mut out, 0);
    assert!(
        out.iter().skip(SR as usize / 50).any(|s| s.abs() > 0.05),
        "the send still plays through a poisoned lock"
    );
    assert!(!failed.load(Ordering::Acquire));
}

/// The stream's clock, for callbacks that come exactly on time however fast
/// the test runs them: each the length of the one before after it.
struct OnTime(f64);

impl OnTime {
    fn now() -> Self {
        Self(clock_sec())
    }

    fn fill(&mut self, output: &mut Output, out: &mut [f32], latency: usize) {
        output.fill_at(out, latency, self.0);
        self.0 += out.len() as f64 / f64::from(SR);
    }
}

/// The system's allocator, handed every call unchanged, which also counts
/// what a thread allocates and frees while that thread asks it to. It is the
/// allocator of the whole test binary — there can only be one — so to every
/// other test it is the system's in all but one load a call.
struct Counting;

/// What a counted thread has allocated and freed since it asked.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Counts {
    allocations: usize,
    frees: usize,
}

/// How many threads are being counted. Almost always none, and then a call
/// goes straight on to the system. Relaxed is enough: a thread always sees
/// its own count go up, and only its own calls are counted.
static COUNTING: AtomicUsize = AtomicUsize::new(0);

std::thread_local! {
    /// This thread's counts, while it is being counted. Const, and with
    /// nothing to drop, it is a plain thread-local static: reading it
    /// allocates nothing, which is what lets the allocator read it.
    static COUNTED: Cell<Option<Counts>> = const { Cell::new(None) };
}

/// Count one call on this thread, if it is being counted. Never panics, as
/// an allocator must not.
fn note(count: impl FnOnce(&mut Counts)) {
    if COUNTING.load(Ordering::Relaxed) == 0 {
        return;
    }
    let _ = COUNTED.try_with(|counted| {
        if let Some(mut counts) = counted.get() {
            count(&mut counts);
            counted.set(Some(counts));
        }
    });
}

// Safety: every call goes to the system allocator as it came, and the
// counting only touches this thread's own cell.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(|counts| counts.allocations += 1);
        // Safety: the caller's guarantees, passed on as they are.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(|counts| counts.allocations += 1);
        // Safety: as for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // In place or moved, it is the allocator's work: counted as a new
        // block and the old one freed.
        note(|counts| {
            counts.allocations += 1;
            counts.frees += 1;
        });
        // Safety: as for `alloc`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(|counts| counts.frees += 1);
        // Safety: as for `alloc`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// What `body` allocated and freed on this thread.
fn counted(body: impl FnOnce()) -> Counts {
    COUNTED.with(|counted| counted.set(Some(Counts::default())));
    COUNTING.fetch_add(1, Ordering::Relaxed);
    body();
    COUNTING.fetch_sub(1, Ordering::Relaxed);
    COUNTED.with(Cell::take).unwrap_or_default()
}

/// A pile-up, as a session sends it: the station being copied, with every
/// other station the settings allow calling across it, through the receiver
/// and under the fading. `dry`, the station being copied is played past the
/// receiver, as your message in a contest is.
fn pile_up(settings: &TrainingSettings, rng: &mut SplitMix64, at_sec: f64, dry: bool) -> SendVoice {
    let voice = resolve_station(settings, rng);
    let call = Transmission::calling(settings, "CQ TEST", voice, rng, |rng| {
        generate_callsign(CALLSIGN_TIER_MAX, rng)
    });
    let mut plan = plan_transmission(&call, settings);
    plan.dry_wanted = dry;
    SendVoice::new(
        &plan,
        &FilterDesign::from_settings(settings),
        Qsb::of(&settings.band),
        SR,
        at_sec,
    )
}

/// A change made the way the player makes one: under the lock, with what it
/// sweeps let go of only once the lock is released.
fn on_ui(mixer: &Mutex<Mixer>, change: impl FnOnce(&mut Mixer, &mut Swept)) {
    let mut swept = Swept::new();
    change(&mut mixer.lock().expect("mixer"), &mut swept);
    drop(swept);
}

/// The audio callback allocates nothing and frees nothing, whatever the UI
/// does between its buffers. An allocator may wait on a lock of its own or
/// on the system, and a callback that waits is a hole in the sound.
///
/// A minute of a busy band — its stations, its static, a pile-up calling
/// over the sends, the fading on them, every other send with its wanted
/// station played past the receiver — while the UI rebuilds the band as a
/// dragged slider does and takes it away, starts sends over each other,
/// stops one and lets another play out, keys the paddle's sidetone, and now
/// and then holds the mixer through a buffer — and for a moment rebuilds the
/// band and starts a send on every one of a host's shortest buffers, with
/// more sends riding out their release than the mixer keeps room for.
/// Everything is built before the lock is taken and let go of after it, as
/// the player does it; the host hands over buffers of every size, up to the
/// third of a second a default one can.
#[test]
fn the_audio_callback_never_allocates_or_frees() {
    const SECONDS: usize = 60;
    const FRAMES: [usize; 6] = [64, 160, 256, 441, 1_024, 5_333];
    const SHORTEST: usize = 32;
    const LATENCY: usize = 512;

    // Without this the rest would pass for nothing.
    let freed = counted(|| drop(std::hint::black_box(vec![0u8; 16])));
    assert_eq!(
        freed,
        Counts {
            allocations: 1,
            frees: 1
        },
        "the counting counts"
    );

    let mut settings = TrainingSettings::default();
    settings.band.stations_max = STATIONS_MAX;
    settings.band.qrn_level = 1.0;
    let settings = settings.clamp();
    let mixer = Arc::new(Mutex::new(Mixer::new(SR)));
    let control = Arc::new(SidetoneControl::new());
    let failed = Arc::new(AtomicBool::new(false));
    let mut output = Output::new(
        Arc::clone(&mixer),
        Arc::clone(&control),
        Arc::clone(&failed),
        SR,
    );
    let mut rng = SplitMix64::new(11);
    let mut clock = OnTime::now();
    // Made once, as the stream's own scratch buffer is.
    let mut buffer = vec![0.0f32; FRAMES.into_iter().max().unwrap_or(0)];
    let mut sends = Vec::new();
    let (mut played, mut peak) = (0, 0.0f32);
    let (mut total, mut first) = (Counts::default(), None);
    let mut k = 0;
    while played < SECONDS * SR as usize {
        // Every quarter of a minute or so: a slider dragged, the band
        // rebuilt on every buffer; a send every half second or so, each over
        // the last, and one of them stopped; a burst of both on every one of
        // a run of the shortest buffers, bands replaced before they are in
        // and sends faster than they can ride out; then the last send left
        // to play out, and the band taken away a while.
        let cycle = k % 200;
        let burst = (70..100).contains(&cycle);
        if burst || cycle < 20 || cycle % 13 == 0 {
            let next = (cycle != 130).then(|| band(&settings, k as u64));
            on_ui(&mixer, |mixer, swept| mixer.set_band(next, swept));
        }
        // And the receiver's controls turned, the band on the air retuned
        // under them, the retune made on the audio thread.
        if burst || (20..70).contains(&cycle) {
            let mut turned = settings.clone();
            turned.band.filter_width_hz = 200.0 + 50.0 * (k % 9) as f64;
            turned.band.agc_enabled = k % 4 != 0;
            let receiver = Receiver::new(SR, &turned);
            on_ui(&mixer, |mixer, swept| {
                let _ = mixer.retune_band(receiver, Gains::UNITY, swept);
            });
        }
        if burst || (cycle < 100 && cycle % 8 == 0) {
            let at_sec = played as f64 / f64::from(SR);
            let voice = pile_up(&settings, &mut rng, at_sec, sends.len() % 2 == 0);
            sends.push(voice.finished_flag());
            on_ui(&mixer, |mixer, swept| mixer.start(voice, swept));
        }
        if cycle == 52 {
            mixer.lock().expect("mixer").stop();
        }
        if k % 3 == 0 {
            let pitch = 500.0 + 50.0 * (k % 7) as f64;
            control.set(crate::audio::ToneRequest::new(k % 6 == 0, pitch, 0.2));
        }

        let frames = if burst {
            SHORTEST
        } else {
            FRAMES[k % FRAMES.len()]
        };
        let out = &mut buffer[..frames];
        let held = (k % 50 == 7).then(|| mixer.lock().expect("mixer"));
        let counts = counted(|| clock.fill(&mut output, out, LATENCY));
        drop(held);

        if counts != Counts::default() && first.is_none() {
            first = Some(k);
        }
        total.allocations += counts.allocations;
        total.frees += counts.frees;
        peak = out.iter().fold(peak, |a, s| a.max(s.abs()));
        played += frames;
        k += 1;
    }

    assert_eq!(
        total,
        Counts::default(),
        "the callback allocated or freed, first in buffer {first:?}"
    );
    assert!(!failed.load(Ordering::Acquire));
    assert!(
        sends.iter().any(|done| done.load(Ordering::Acquire)),
        "no send was heard out"
    );
    assert!(peak > 0.1, "nothing much played: a peak of {peak}");
}

/// How many samples the receiver's gate takes to move all the way.
fn gate_ramp() -> usize {
    (GATE_RAMP_SEC * f64::from(SR)).round() as usize
}

/// Your message, and a loud caller calling across it and on past it.
fn sending_across(settings: &TrainingSettings) -> (Segment, Segment) {
    let centre = settings.side_tone_center();
    let mine = segment(
        settings,
        Sender::You,
        "K5XYZ 5NN TT7",
        keyer(centre, 1.0),
        0.0,
    );
    let across = segment(
        settings,
        Sender::Caller(0),
        "K5XYZ K5XYZ",
        keyer(centre + 150.0, VOLUME_MAX),
        0.2,
    );
    assert!(across.end_sec > mine.end_sec);
    (mine, across)
}

/// Without full break-in the receiver is muted while you send, after its
/// AGC: once the gate has shut, all that is heard is your message, sample
/// for sample, the band and the caller across it gone; once it has opened
/// again after your message, the receiver is heard exactly as with full
/// break-in, its AGC having run on under the gate. Your message is never
/// muted, and with full break-in the band is heard through it.
#[test]
fn the_receiver_is_muted_while_you_send_and_never_your_message() {
    let settings = hiss();
    assert!(!settings.contest.contest_qsk, "off as it ships");
    let (mine, across) = sending_across(&settings);
    let gated = planned_turn(&settings, &[&mine, &across]);
    assert_eq!(gated.muted_until, Some(mine.end_sec));
    let mut qsk = settings.clone();
    qsk.contest.contest_qsk = true;
    let open = planned_turn(&qsk, &[&mine, &across]);
    assert_eq!(open.muted_until, None);
    let on_band = || Some(band(&settings, 3));
    let shut = hear(&settings, &gated, on_band());
    let through = hear(&settings, &open, on_band());
    let alone = PlannedTransmission {
        others: Vec::new(),
        muted_until: None,
        ..gated
    };
    let yours = hear(&settings, &alone, None);
    assert_eq!(shut.len(), through.len());
    let ramp = gate_ramp();
    let ends = (mine.end_sec * f64::from(SR)).round() as usize;
    for i in ramp + 1..ends {
        assert_eq!(shut[i], yours[i], "at {i}: muted, only your message");
    }
    for i in ends + ramp + 1..shut.len() {
        assert_eq!(shut[i], through[i], "at {i}: open again, as with QSK");
    }
    let band_under = power(&through[ramp..ends]) - power(&yours[ramp..ends]);
    // However far down the AGC holds it under the caller.
    assert!(band_under > 1e-6, "with QSK the band is heard under you");
    assert!(
        loudest(&yours[ramp..ends]) > 0.1,
        "your message is never muted"
    );
}

/// The gate moves a straight line at a time, the same small step a
/// sample, shutting over the ramp from your message's start, shut to its
/// end, open again over the ramp after it; a stop opens it from where it
/// stands, your sending over. It never steps.
#[test]
fn the_gate_ramps_and_never_steps() {
    let settings = hiss();
    let (mine, across) = sending_across(&settings);
    let gated = planned_turn(&settings, &[&mine, &across]);
    let receiver = FilterDesign::from_settings(&settings);
    let step = 1.0 / (GATE_RAMP_SEC * f64::from(SR));
    let ramp = gate_ramp();
    let ends = (mine.end_sec * f64::from(SR)).round() as usize;
    let gates = |stop_at: Option<usize>| {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.set_band(Some(band(&settings, 4)), &mut swept);
        mixer.start(SendVoice::new(&gated, &receiver, None, SR, 0.0), &mut swept);
        // In heard time: from where the send's first sample is heard, the
        // look-ahead after it was started. A stop is made in the mixer's.
        let mut gates = Vec::new();
        for at in 0..late() + ends + 4 * ramp {
            if Some(at) == stop_at {
                mixer.stop();
            }
            render(&mut mixer, 1);
            gates.push(mixer.gate);
        }
        gates.split_off(late())
    };
    let played = gates(None);
    assert!(
        played
            .windows(2)
            .all(|pair| (pair[1] - pair[0]).abs() <= step + 1e-12),
        "a step"
    );
    assert!(
        played[ramp - 2] > 0.0 && played[ramp] == 0.0,
        "shut over the ramp"
    );
    assert!(played[ramp..ends].iter().all(|gate| *gate == 0.0));
    assert!(played[ends + ramp - 2] < 1.0 && played[ends + ramp] == 1.0);
    assert!(played[ends + ramp..].iter().all(|gate| *gate == 1.0));
    // Stopped half way through your message, the receiver comes back once
    // what was already on its way through the look-ahead has played.
    let stopped = gates(Some(ends / 2));
    assert_eq!(stopped[ends / 2 - 1], 0.0);
    assert_eq!(stopped[ends / 2 + ramp], 1.0);
    assert!(
        stopped
            .windows(2)
            .all(|pair| (pair[1] - pair[0]).abs() <= step + 1e-12)
    );
}

/// With your sending off you still transmit: the receiver is muted all
/// the same, and nothing at all is heard while you send.
#[test]
fn your_sending_off_still_mutes_the_receiver() {
    let mut settings = hiss();
    settings.contest.my_sending_db = cw_core::MY_SENDING_OFF_DB;
    let (mine, across) = sending_across(&settings);
    let gated = planned_turn(&settings, &[&mine, &across]);
    assert!(gated.wanted.events.is_empty());
    let shut = hear(&settings, &gated, Some(band(&settings, 5)));
    let ends = (mine.end_sec * f64::from(SR)).round() as usize;
    assert!(shut[gate_ramp() + 1..ends].iter().all(|s| *s == 0.0));
    assert!(loudest(&shut[ends + gate_ramp()..]) > 0.0, "and back after");
}

/// The paddle's sidetone is never muted: while the receiver is shut for
/// your message, with your sending off, the stream plays the sidetone and
/// nothing else.
#[test]
fn the_paddle_sidetone_is_never_muted() {
    let mut settings = hiss();
    settings.contest.my_sending_db = cw_core::MY_SENDING_OFF_DB;
    let (mine, across) = sending_across(&settings);
    let gated = planned_turn(&settings, &[&mine, &across]);
    let receiver = FilterDesign::from_settings(&settings);
    let fill = |sidetone: bool| {
        let mixer = Arc::new(Mutex::new(Mixer::new(SR)));
        let control = Arc::new(SidetoneControl::new());
        {
            let mut mixer = mixer.lock().expect("the mixer");
            let mut swept = Swept::new();
            mixer.set_band(Some(band(&settings, 6)), &mut swept);
            mixer.start(SendVoice::new(&gated, &receiver, None, SR, 0.0), &mut swept);
        }
        control.set(crate::audio::ToneRequest::new(sidetone, 700.0, 0.4));
        let mut output = Output::new(mixer, control, Arc::new(AtomicBool::new(false)), SR);
        // In buffers as a device asks for them: the sidetone's edge is a
        // buffer late, and none of one rendered whole is left to it.
        let mut clock = OnTime::now();
        let mut out = vec![0.0f32; SR as usize / 2];
        for chunk in out.chunks_mut(256) {
            clock.fill(&mut output, chunk, 0);
        }
        out
    };
    // Once your message is heard, the look-ahead in.
    let shut = late() + gate_ramp() + 1;
    let quiet = fill(false);
    assert!(quiet[shut..].iter().all(|s| *s == 0.0), "the band muted");
    let keyed = fill(true);
    assert!(loudest(&keyed[shut..]) > 0.1, "the sidetone sounds");
}

/// Where a turn's caller flutters, the mixer plays it rising and falling
/// with its flutter, at the slowest and the fastest rate and the shallowest
/// and the deepest depth flutter goes to: the slowest to a few hundredths
/// of its level, but for the filter's moment of delay, the fastest in step
/// with its own flutter and with no other; never deeper than its depth, and
/// near the bottom of it. Two callers in one turn each follow their own;
/// the same turn plays the same twice; and your message, whatever fade it
/// is handed, never flutters.
#[test]
fn a_fluttering_caller_flutters_at_its_own_rate_and_depth() {
    use cw_core::Fade;
    let mut settings = hiss();
    settings.band.filter_width_hz = cw_core::FILTER_WIDTH_MAX;
    let centre = settings.side_tone_center();
    let steady = StationVoice {
        char_wpm: 8.0,
        effective_wpm: 8.0,
        ..keyer(centre, 1.0)
    };
    let flutters = [Qsb::flutter(0.5, 3.0), Qsb::flutter(0.75, 33.0)];
    let mut first = segment(&settings, Sender::Caller(0), "TTTTTT", steady, 0.0);
    let mut second = segment(
        &settings,
        Sender::Caller(1),
        "TTTTTT",
        steady,
        first.end_sec + 0.5,
    );
    let plain = planned_turn(&settings, &[&first, &second]);
    let along = |flutter: Qsb, from_sec: f64| Fade {
        qsb: None,
        flutter: Some(flutter),
        from_sec,
    };
    first.fade = Some(along(flutters[0], 0.0));
    second.fade = Some(along(flutters[1], 0.0));
    let fluttered = planned_turn(&settings, &[&first, &second]);
    let (x, y) = (
        hear(&settings, &plain, None),
        hear(&settings, &fluttered, None),
    );
    assert_eq!(y, hear(&settings, &fluttered, None), "the same twice");
    let sr = f64::from(SR);
    for (at, (segment, flutter)) in [(&first, flutters[0]), (&second, flutters[1])]
        .into_iter()
        .enumerate()
    {
        let from = (segment.start_sec * sr) as usize;
        let to = (segment.end_sec * sr) as usize;
        let peak = loudest(&x[from..to]);
        // Where the tone is well clear of nothing, and the level played there
        // against the unfluttered tone's.
        let clear: Vec<usize> = (from + 48..to)
            .filter(|i| x[*i].abs() > 0.3 * peak)
            .collect();
        let ratios: Vec<f64> = clear.iter().map(|i| f64::from(y[*i] / x[*i])).collect();
        // Each flutter as it would be heard `lag` samples late, the filter's
        // moment of delay.
        let along = |flutter: Qsb, lag: usize| -> Vec<f64> {
            clear
                .iter()
                .map(|i| f64::from(flutter.gain_at((i - lag) as f64 / sr - segment.start_sec)))
                .collect()
        };
        let lags = || 0..48usize;
        let worst = |lag: usize| {
            ratios
                .iter()
                .zip(along(flutter, lag))
                .map(|(played, meant)| (played - meant).abs())
                .fold(0.0, f64::max)
        };
        let in_step = |flutter: Qsb| {
            lags()
                .map(|lag| correlation(&ratios, &along(flutter, lag)))
                .fold(f64::MIN, f64::max)
        };
        if at == 0 {
            let fit = lags().map(worst).fold(f64::MAX, f64::min);
            assert!(fit < 0.06, "3 Hz: off by {fit:.3}");
        }
        let own = in_step(flutter);
        let other = in_step(flutters[1 - at]);
        assert!(own > 0.9, "{:.0} Hz: in step {own:.2}", flutter.rate_hz());
        assert!(
            other < 0.5,
            "{:.0} Hz: in step with the other {other:.2}",
            flutter.rate_hz()
        );
        let (low, high) = ratios
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), r| (lo.min(*r), hi.max(*r)));
        assert!(
            low < 1.0 - 0.8 * flutter.depth() && high > 0.95,
            "{:.0} Hz: {low:.2}..{high:.2}",
            flutter.rate_hz()
        );
        assert!(
            low > 1.0 - flutter.depth() - 0.05,
            "never deeper than its depth"
        );
    }
    // Your message, handed a flutter, plays as it would without one.
    let mine = segment(&settings, Sender::You, "TTT", keyer(centre, 1.0), 0.0);
    let mut fluttering = mine.clone();
    fluttering.fade = Some(along(flutters[1], 0.0));
    assert_eq!(
        hear(&settings, &planned_turn(&settings, &[&fluttering]), None),
        hear(&settings, &planned_turn(&settings, &[&mine]), None)
    );
}

/// How closely two series rise and fall together, from -1 to 1.
fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(a), mean(b));
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma).powi(2);
        bb += (y - mb).powi(2);
    }
    ab / (aa * bb).sqrt()
}

/// Where each element starts sounding in `samples`: a rise out of a
/// silence longer than a cycle of the tone.
fn onsets(samples: &[f32]) -> Vec<usize> {
    let peak = loudest(samples);
    let quiet = SR as usize / 500;
    (quiet..samples.len())
        .filter(|i| samples[*i].abs() > 0.05 * peak)
        .filter(|i| {
            samples[i - quiet..*i]
                .iter()
                .all(|s| s.abs() <= 0.05 * peak)
        })
        .collect()
}

/// A continuation taking over where the send playing gives way, at the
/// start of the first character of your message yet to sound, plays on from
/// there: every element of your message sounds where one send of the whole
/// of it has it, to a sample or two of rounding, none twice and no gap; the
/// send it takes over from rides out its release from there, silent by then.
#[test]
fn a_continuation_takes_over_where_the_send_before_gives_way() {
    let settings = hiss();
    let voice = keyer(settings.side_tone_center(), 1.0);
    let receiver = FilterDesign::from_settings(&settings);
    let whole = planned_turn(
        &settings,
        &[&segment(&settings, Sender::You, "K5XYZ 5NN", voice, 0.0)],
    );
    let reference = hear(&settings, &whole, None);
    let first = planned_turn(
        &settings,
        &[&segment(&settings, Sender::You, "K5XYZ", voice, 0.0)],
    );
    let rest = planned_turn(
        &settings,
        &[&segment(&settings, Sender::You, "5XYZ 5NN", voice, 0.0)],
    );
    let sr = f64::from(SR);
    // The 5 starts with the fourth element of the call.
    let into = (first.wanted.events[3].start_sec * sr).round() as usize;
    for key_ms in [20, 120, 300] {
        let key = SR as usize * key_ms / 1_000;
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.start(SendVoice::new(&first, &receiver, None, SR, 0.0), &mut swept);
        let mut played = render(&mut mixer, key);
        let next = SendVoice::new(&rest, &receiver, None, SR, 0.0);
        let waits = mixer.start_at(next, Some(into), 0, &mut swept);
        assert_eq!(waits, into - key, "{key_ms} ms in");
        played.extend(render(&mut mixer, late() + reference.len() - key));
        // In heard time, the look-ahead after the mixer's.
        let played = played.split_off(late());
        let (heard, meant) = (onsets(&played), onsets(&reference));
        assert_eq!(
            heard.len(),
            meant.len(),
            "{key_ms} ms in: {heard:?} {meant:?}"
        );
        for (at, should) in heard.iter().zip(&meant) {
            assert!(
                at.abs_diff(*should) <= 2,
                "{key_ms} ms in: {at} for {should}"
            );
        }
    }
}

/// Without full break-in the receiver stays muted across a continuation
/// taking over part way through your message: from the first send's gate
/// shut to the end of the continuation's message nothing of the band, nor
/// of a loud caller sending across, is heard, and the gate never moves, a
/// second key taking over from the first continuation at its own start
/// included, a moment of clock rounding past it or not. With your sending
/// off, all that is heard meanwhile is nothing; the receiver comes back
/// after your message.
#[test]
fn the_gate_stays_shut_across_a_splice() {
    let mut settings = hiss();
    settings.contest.my_sending_db = cw_core::MY_SENDING_OFF_DB;
    let centre = settings.side_tone_center();
    let voice = keyer(centre, 1.0);
    let loud = keyer(centre + 150.0, VOLUME_MAX);
    let turn = |text: &str| {
        let mine = segment(&settings, Sender::You, text, voice, 0.0);
        let across = segment(&settings, Sender::Caller(0), "K5XYZ K5XYZ", loud, 0.0);
        planned_turn(&settings, &[&mine, &across])
    };
    let (first, rest, again) = (turn("K5XYZ"), turn("5XYZ 5NN"), turn("5XYZ 5NN TT1"));
    let receiver = FilterDesign::from_settings(&settings);
    let sr = f64::from(SR);
    // The 5 starts with the fourth element of the call.
    let call = plan_morse_playback_for("K5XYZ", &settings, &voice);
    let into = (call.events[3].start_sec * sr).round() as usize;
    let key = SR as usize * 120 / 1_000;
    let ramp = gate_ramp();
    for chained in [false, true] {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.set_band(Some(band(&settings, 3)), &mut swept);
        mixer.start(SendVoice::new(&first, &receiver, None, SR, 0.0), &mut swept);
        let (mut played, mut gates) = (Vec::new(), Vec::new());
        let mut play = |mixer: &mut Mixer, frames: usize| {
            for _ in 0..frames {
                played.extend(render(mixer, 1));
                gates.push(mixer.gate);
            }
        };
        play(&mut mixer, key);
        let next = SendVoice::new(&rest, &receiver, None, SR, 0.0);
        assert_eq!(mixer.start_at(next, Some(into), 0, &mut swept), into - key);
        let (mut last, mut so_far) = (&rest, key);
        if chained {
            let half = (into - key) / 2;
            play(&mut mixer, half);
            so_far += half;
            // A second key before the first continuation is heard, asked
            // for a millisecond past its start.
            let next = SendVoice::new(&again, &receiver, None, SR, 0.0);
            let waits = mixer.start_at(next, Some(SR as usize / 1_000), 0, &mut swept);
            assert_eq!(waits, into - key - half, "at the first's own start");
            last = &again;
        }
        let ends = into + (last.muted_until.expect("muted") * sr).round() as usize;
        play(&mut mixer, late() + ends + 4 * ramp - so_far);
        // In heard time, the look-ahead after the mixer's.
        let (played, gates) = (&played[late()..], &gates[late()..]);
        let opened = gates[ramp..ends].iter().position(|gate| *gate != 0.0);
        assert_eq!(opened, None, "chained {chained}: the gate moved");
        let heard = played[ramp + 1..ends].iter().position(|s| *s != 0.0);
        assert_eq!(heard, None, "chained {chained}: the receiver heard");
        assert_eq!(gates[ends + ramp], 1.0, "chained {chained}: open after");
        assert!(loudest(&played[ends + ramp..]) > 0.0);
    }
}

/// A continuation that takes over once your message has gone out, half a
/// word space after its last character, as the machine puts it, leaves
/// that character whole: the send it takes over from is ridden down in the
/// silence after it, so your last element plays sample for sample as one
/// send of the whole would play it, and the next message starts a word
/// space after it, as one message would have it. The receiver comes back in
/// that word space, as the machine hears a reply that starts in it, and is
/// muted again from the takeover.
#[test]
fn a_continuation_after_your_last_character_leaves_it_whole() {
    let settings = hiss();
    let voice = keyer(settings.side_tone_center(), 1.0);
    let receiver = FilterDesign::from_settings(&settings);
    let sr = f64::from(SR);
    let whole = planned_turn(
        &settings,
        &[&segment(&settings, Sender::You, "K5XYZ 5NN", voice, 0.0)],
    );
    let reference = hear(&settings, &whole, None);
    // The call's twenty elements, then the 5 of the exchange.
    let events = &whole.wanted.events;
    let z_ends = events[19].start_sec + events[19].duration_sec;
    let gap = events[20].start_sec - z_ends;
    let takes_over = z_ends + gap / 2.0;
    let first = planned_turn(
        &settings,
        &[&segment(&settings, Sender::You, "K5XYZ", voice, 0.0)],
    );
    let next = planned_turn(
        &settings,
        &[&segment(
            &settings,
            Sender::You,
            "5NN",
            voice,
            gap - gap / 2.0,
        )],
    );
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.start(SendVoice::new(&first, &receiver, None, SR, 0.0), &mut swept);
    let (mut played, mut gates) = (Vec::new(), Vec::new());
    let mut play = |mixer: &mut Mixer, frames: usize| {
        for _ in 0..frames {
            played.extend(render(mixer, 1));
            gates.push(mixer.gate);
        }
    };
    let key = (events[19].start_sec * sr) as usize;
    play(&mut mixer, key);
    let into = (takes_over * sr).round() as usize;
    let continuation = SendVoice::new(&next, &receiver, None, SR, 0.0);
    assert_eq!(
        mixer.start_at(continuation, Some(into), 0, &mut swept),
        into - key
    );
    play(&mut mixer, late() + reference.len() - key);
    // In heard time, the look-ahead after the mixer's.
    let (played, gates) = (played.split_off(late()), gates.split_off(late()));
    let ramp = gate_ramp();
    let yours_end = (first.muted_until.expect("muted") * sr).round() as usize;
    let next_end = into + (next.muted_until.expect("muted") * sr).round() as usize;
    assert!(gates[ramp..yours_end].iter().all(|gate| *gate == 0.0));
    assert!(
        gates[yours_end + ramp..into]
            .iter()
            .all(|gate| *gate == 1.0)
    );
    assert!(gates[into + ramp..next_end].iter().all(|gate| *gate == 0.0));
    let z = (z_ends * sr).ceil() as usize + 1;
    assert_eq!(
        played[..z],
        reference[..z],
        "your last element as one send has it"
    );
    assert!(
        loudest(&played[z..into + 1]) < 1e-6,
        "ridden down in silence"
    );
    let (heard, meant) = (onsets(&played), onsets(&reference));
    assert_eq!(heard.len(), meant.len(), "{heard:?} {meant:?}");
    for (at, should) in heard.iter().zip(&meant) {
        assert!(at.abs_diff(*should) <= 2, "{at} for {should}");
    }
}

/// A send waiting to take over is put off by nothing but its time: asked
/// for once the send playing has passed the moment, it starts at once; with
/// nothing playing, it waits what it was asked to; and stopped before it
/// starts, neither it nor the send it was to take over from is heard again.
#[test]
fn a_send_waiting_to_take_over_starts_on_time_or_not_at_all() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    assert_eq!(
        mixer.start_at(send(&settings, "T"), Some(400), 250, &mut swept),
        250,
        "nothing to take over from"
    );
    render(&mut mixer, SR as usize);
    let mut mixer = Mixer::new(SR);
    mixer.start(send(&settings, "TTTTT"), &mut swept);
    render(&mut mixer, 1_000);
    assert_eq!(
        mixer.start_at(send(&settings, "T"), Some(600), 0, &mut swept),
        0,
        "late, at once"
    );
    // Stopped before its time.
    let mut mixer = Mixer::new(SR);
    mixer.start(send(&settings, "TTTTT"), &mut swept);
    render(&mut mixer, 1_000);
    let waits = mixer.start_at(send(&settings, "TTTTT"), Some(3_000), 0, &mut swept);
    assert_eq!(waits, 2_000);
    render(&mut mixer, 500);
    mixer.stop();
    let release = (crate::audio::shape::SEND_RELEASE_SEC * f64::from(SR)).ceil() as usize;
    // Its release, and the look-ahead it is played behind; then only the
    // audio stage's own tail, 60 dB down at once and gone within a quarter
    // of a second.
    render(&mut mixer, release + 1 + late());
    let after = render(&mut mixer, SR as usize);
    assert!(after.iter().all(|s| s.abs() < 1e-3), "the send played on");
    assert!(after[SR as usize / 4..].iter().all(|s| *s == 0.0));
}

/// Out of the box, fading is heard under the AGC as it is on the air: the
/// station the AGC rides holds nearly steady, while the hiss between its
/// elements swells up and sinks back with each fade, by most of the fade's
/// depth. A fade as shallow as the old default's was, under the AGC, no
/// fading at all to the ear.
#[test]
fn fading_is_heard_under_the_agc_out_of_the_box() {
    let mut settings = TrainingSettings::default();
    // Static would cover the hiss it is heard in; a quicker fade puts
    // several in the listen.
    settings.band.qrn_enabled = false;
    settings.band.qsb_rate_hz = 0.5;
    for wpm in [
        &mut settings.playback.char_wpm_min,
        &mut settings.playback.char_wpm_max,
        &mut settings.playback.effective_wpm_min,
        &mut settings.playback.effective_wpm_max,
    ] {
        *wpm = 20.0;
    }
    let voice = resolve_station(&settings, &mut SplitMix64::new(5));
    let plan = plan_transmission(&Transmission::alone("5".repeat(64), voice), &settings);
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(Some(band(&settings, 3)), &mut swept);
    mixer.start(
        SendVoice::new(
            &plan,
            &FilterDesign::from_settings(&settings),
            Qsb::of(&settings.band),
            SR,
            0.0,
        ),
        &mut swept,
    );
    let out = render(
        &mut mixer,
        late() + (plan.wanted.duration_sec * f64::from(SR)) as usize,
    );
    let at = |t: f64| late() + (t * f64::from(SR)) as usize;
    let ms = 0.001;
    // Each second, from the third on: the station within its elements, and
    // the hiss in the gaps between them, past the filter's ringing.
    let (mut marks, mut gaps) = (vec![(0.0, 0); 48], vec![(0.0, 0); 48]);
    let add = |into: &mut (f64, usize), samples: &[f32]| {
        into.0 += power(samples);
        into.1 += 1;
    };
    for pair in plan.wanted.events.windows(2) {
        let (tone, next) = (&pair[0], &pair[1]);
        let second = tone.start_sec as usize;
        if second < 2 || second >= marks.len() {
            continue;
        }
        let up = tone.start_sec + tone.keyed_sec;
        add(
            &mut marks[second],
            &out[at(tone.start_sec + 10.0 * ms)..at(up - 5.0 * ms)],
        );
        if next.start_sec - up > 0.05 {
            add(
                &mut gaps[second],
                &out[at(up + 25.0 * ms)..at(next.start_sec - 5.0 * ms)],
            );
        }
    }
    let mean = |(sum, count): &(f64, usize)| sum / *count as f64;
    let seconds: Vec<(f64, f64)> = marks
        .iter()
        .zip(&gaps)
        .filter(|(mark, gap)| mark.1 > 0 && gap.1 > 0)
        .map(|(mark, gap)| (db(mean(mark)), db(mean(mark) / mean(gap))))
        .collect();
    assert!(seconds.len() > 15, "{} seconds heard", seconds.len());
    let range = |pick: fn(&(f64, f64)) -> f64| {
        let values = seconds.iter().map(pick);
        values.clone().fold(f64::MIN, f64::max) - values.fold(f64::MAX, f64::min)
    };
    let (station, under) = (range(|s| s.0), range(|s| s.1));
    assert!(station < 4.5, "the station moved {station:.1} dB");
    assert!(under >= 9.0, "the hiss under it moved {under:.1} dB");
}

/// The hiss in 5 ms windows over a retune's handover, in decibels against
/// the hiss before it, averaged over many retunes from `from` to `to`, with
/// one back between each; and how far the retune moved the hiss.
fn hiss_across_retunes(from: &TrainingSettings, to: &TrainingSettings) -> (f64, Vec<f64>) {
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(Some(band(from, 1)), &mut swept);
    render(&mut mixer, SR as usize / 2);
    let window = SR as usize / 200;
    let windows = 16;
    let (mut before, mut across, mut after) = (0.0, vec![0.0; windows], 0.0);
    let retunes = 300;
    for _ in 0..retunes {
        before += power(&render(&mut mixer, window * windows));
        let mut swept = Swept::new();
        assert!(mixer.retune_band(Receiver::new(SR, to), Gains::UNITY, &mut swept));
        for (w, chunk) in render(&mut mixer, window * windows)
            .chunks(window)
            .enumerate()
        {
            across[w] += power(chunk);
        }
        after += power(&render(&mut mixer, window * windows));
        assert!(mixer.retune_band(Receiver::new(SR, from), Gains::UNITY, &mut swept));
        render(&mut mixer, window * windows);
    }
    (
        db(after / before),
        across.iter().map(|p| db(p / before)).collect(),
    )
}

/// Two bands put in a few milliseconds apart under a station the AGC is
/// riding: the one put in first goes before the one it replaced, and the
/// station rides on at the gain it had as each hands over, never at one
/// with nothing ducked, nor stepping when the last going out has gone.
#[test]
fn a_band_replaced_as_it_came_in_never_lets_the_station_jump() {
    // Nothing on the band but its AGC, ducking the station hard: what
    // comes out is the station, at what it rides.
    let mut settings = hiss();
    settings.band.noise_enabled = false;
    settings.playback.char_wpm_min = 5.0;
    settings.playback.char_wpm_max = 5.0;
    settings.band.link_volume = true;
    settings.band.volume_min = VOLUME_MAX;
    let full = Gains::of(&settings.clone().clamp());
    for volume in [cw_core::MASTER_VOLUME_MIN, 0.3, 0.6, 1.0] {
        settings.band.master_volume = volume;
        let settings = settings.clone().clamp();
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        // At full with no band on, then the band in and the volume turned
        // down under it: the AF gain with no band on stays where it was,
        // as far from what the send rides as the volume was turned.
        mixer.set_gains(full);
        render(&mut mixer, SR as usize / 10);
        mixer.set_band(Some(band(&settings, 1)), &mut swept);
        mixer.set_gains(Gains::of(&settings));
        // Well into a dah 0.72 s long, the AGC riding it down.
        mixer.start(send(&settings, "T"), &mut swept);
        let before = render(&mut mixer, SR as usize * 3 / 10);
        mixer.set_band(Some(band(&settings, 2)), &mut swept);
        let mut after = render(&mut mixer, SR as usize * 3 / 1000);
        mixer.set_band(Some(band(&settings, 3)), &mut swept);
        after.extend(render(&mut mixer, SR as usize * 40 / 1000));
        // Two milliseconds hold a crest at any pitch the dial has.
        let window = SR as usize / 500;
        let steady = f64::from(loudest(&before[before.len() - 5 * window..]));
        for (k, stretch) in after.chunks(window).enumerate() {
            let moved = 20.0 * (f64::from(loudest(stretch)) / steady).log10();
            assert!(
                moved.abs() < 1.0,
                "volume {volume}: {moved:+.1} dB {} ms after the first band came in",
                2 * k
            );
        }
    }
}

/// A receiver control turned retunes the band on the air: the new filter
/// and AGC warm unheard on the same noise and stations, then the two cross
/// with gains that sum to one, as the same band twice adds. Retuned to the
/// receiver it has, it plays on exactly as the band left alone, through the
/// handover and after; to another, the hiss goes from the one level to the
/// other without a hole or a swell past either.
#[test]
fn a_retune_carries_the_band_on_without_a_hole() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(Some(band(&settings, 2)), &mut swept);
    render(&mut mixer, SR as usize / 2);
    for _ in 0..20 {
        let mut alone = mixer.clone();
        assert!(mixer.retune_band(Receiver::new(SR, &settings), Gains::UNITY, &mut swept));
        let (heard, left) = (
            render(&mut mixer, SR as usize / 10),
            render(&mut alone, SR as usize / 10),
        );
        let off = |from: usize| {
            heard[from..]
                .iter()
                .zip(&left[from..])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max)
                / loudest(&left)
        };
        // The new filter has filled by the handover, if not yet to the
        // waveform; a few tens of milliseconds on it is the old one's.
        assert!(off(0) < 0.05, "{:.4} off the band left alone", off(0));
        let after = SR as usize / 12;
        assert!(off(after) < 1e-3, "{:.5} off it after", off(after));
        mixer.sweep(&mut swept);
    }
    let settings = hiss();
    for edit in [
        (|s| s.band.filter_width_hz = 250.0) as fn(&mut TrainingSettings),
        |s| s.band.filter_shape = cw_core::FilterShape::Peaked,
        |s| s.band.agc_enabled = false,
    ] {
        let mut turned = settings.clone();
        edit(&mut turned);
        let (moved, levels) = hiss_across_retunes(&settings, &turned);
        let (low, high) = (moved.min(0.0) - 1.0, moved.max(0.0) + 1.0);
        assert!(
            levels.iter().all(|level| (low..high).contains(level)),
            "the hiss across a retune that moves it {moved:.2} dB: {levels:.2?}"
        );
    }
}

/// One step of the width slider can turn the filter over at the pitch, its
/// BFO phase half a turn from the last; handed over as the same band twice,
/// the two cancelled mid-fade, the hiss dropping 8 to 20 dB for a few
/// milliseconds. The new filter takes the playing one's sign, and the hiss
/// goes across without a hole.
#[test]
fn a_retune_one_step_on_never_cancels_the_band() {
    use cw_core::FilterShape::{Peaked, Sharp, Soft};
    for (shape, from, to) in [
        (Peaked, 400.0, 410.0),
        (Peaked, 1_590.0, 1_600.0),
        (Soft, 1_640.0, 1_650.0),
        (Sharp, 1_250.0, 1_260.0),
    ] {
        let mut before = hiss();
        before.band.filter_shape = shape;
        before.band.filter_width_hz = from;
        let mut after = before.clone();
        after.band.filter_width_hz = to;
        let (moved, levels) = hiss_across_retunes(&before, &after);
        let (low, high) = (moved.min(0.0) - 1.5, moved.max(0.0) + 1.5);
        assert!(
            levels.iter().all(|level| (low..high).contains(level)),
            "{shape:?} {from} to {to} Hz, the hiss moving {moved:.2} dB: {levels:.2?}"
        );
    }
}

/// A slider dragged sends retunes faster than a handover takes. They do not
/// queue up: each handover goes to where the slider has got to, and once it
/// stops the band is heard through the receiver it stopped at, the very
/// band a single retune there would have left.
#[test]
fn a_dragged_slider_ends_on_the_receiver_it_stops_at() {
    let settings = hiss();
    let mut dragged = Mixer::new(SR);
    let mut swept = Swept::new();
    dragged.set_band(Some(band(&settings, 4)), &mut swept);
    render(&mut dragged, SR as usize / 2);
    let mut once = dragged.clone();
    let mut last = settings;
    for step in 0..40 {
        last.band.filter_width_hz = 600.0 - 10.0 * f64::from(step);
        let receiver = Receiver::new(SR, &last);
        assert!(dragged.retune_band(receiver, Gains::UNITY, &mut swept));
        render(&mut dragged, SR as usize / 100);
        dragged.sweep(&mut swept);
    }
    assert!(once.retune_band(Receiver::new(SR, &last), Gains::UNITY, &mut swept));
    render(&mut once, SR as usize * 40 / 100);
    let (a, b) = (
        render(&mut dragged, SR as usize),
        render(&mut once, SR as usize),
    );
    let worst = a
        .iter()
        .zip(&b)
        .skip(SR as usize / 2)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(power(&a) > 1e-8, "nothing played");
    assert!(worst < 1e-5, "{worst} off the band retuned once");
    dragged.sweep(&mut swept);
    assert!(dragged.fading.is_empty(), "handovers left over");
    // With no band on the air there is nothing to retune.
    let mut quiet = Mixer::new(SR);
    assert!(!quiet.retune_band(Receiver::new(SR, &last), Gains::UNITY, &mut swept));
}
