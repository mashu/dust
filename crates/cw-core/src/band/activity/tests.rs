//! The band's traffic, counted and listened to.
//!
//! Two kinds of test. The bookkeeping ones read the generator's own record —
//! every transmitter as it came on — and hold it to the measured statistics.
//! The listening ones render the band through the receiver's filter and
//! measure it the way the off-air recordings were measured, because that is
//! the number the calibration is for.

use super::*;
use crate::band::test_support::{Fft, hann, receiver_for};
use crate::settings::{FILTER_WIDTH_MAX, TrainingSettings};

/// Rate for the bookkeeping tests, which never listen: a station's dits are
/// still ten samples long, and an hour of band costs next to nothing.
const CENSUS_RATE: u32 = 200;
/// Rate for the listening tests: the 5.86 Hz analysis bins the recordings were
/// measured with are exactly 1024 points here, and everything a 500 Hz filter
/// lets through is far below the Nyquist frequency.
const LISTEN_RATE: u32 = 6_000;
const LISTEN_POINTS: usize = 1_024;

/// The band with `others` stations besides yours allowed to call, but no cap
/// on how many may key at once: what the bookkeeping is measured on, so that
/// every station the band draws comes on the air.
fn uncapped(sample_rate: u32, settings: &TrainingSettings, others: u32, seed: u64) -> BandActivity {
    let profile = TrafficProfile {
        cap: MAX_EMITTERS,
        ..TrafficProfile::for_others(others).expect("a band with others on it")
    };
    BandActivity::new(sample_rate, profile, tuning(settings), seed)
}

/// Where `settings` listen to the band from.
fn tuning(settings: &TrainingSettings) -> Tuning {
    Tuning::new(&settings.band, &FilterDesign::from_settings(settings))
}

/// The band `settings` describe, if it has others calling on it.
fn band_for(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Option<BandActivity> {
    let receiver = FilterDesign::from_settings(settings);
    BandActivity::for_band(sample_rate, &settings.band, &receiver, seed)
}

/// The band `settings` describe, which has others calling on it.
fn calling_band(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> BandActivity {
    band_for(sample_rate, settings, seed).expect("a band with others on it")
}

/// An ordinary evening's band, and a contest's: one other station allowed to
/// call, and the most the setting allows.
const ORDINARY: u32 = 1;
const CONTEST: u32 = STATIONS_MAX - STATIONS_MIN;

/// The settings with up to `stations` calling at once, yours included.
fn calling(stations: u32) -> TrainingSettings {
    let mut s = TrainingSettings::default();
    s.band.stations_max = stations;
    s.clamp()
}

/// The same, through the receiver the off-air recordings were measured
/// through: the reference filter, a sharp one 500 Hz wide at 3 dB.
fn calling_through_500_hz(stations: u32) -> TrainingSettings {
    let mut s = calling(stations);
    s.band.filter_width_hz = crate::band::filter::reference_width_hz();
    s.band.filter_shape = crate::settings::FilterShape::Sharp;
    s.clamp()
}

fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn level_db(band: &BandActivity, emitter: &Emitter) -> f64 {
    20.0 * (emitter.peak / band.unit).log10()
}

fn wpm(emitter: &Emitter) -> f64 {
    1.2 * emitter.sample_rate / emitter.unit
}

/// Every transmitter that comes on during `seconds` of band, as it was when
/// it came on — the ones already there when the band was switched on
/// included.
fn census(others: u32, seconds: f64, seed: u64) -> (BandActivity, Vec<Emitter>) {
    let mut band = uncapped(CENSUS_RATE, &TrainingSettings::default(), others, seed);
    let mut seen: Vec<Emitter> = band
        .emitters
        .iter()
        .filter(|e| e.kind != Kind::Idle)
        .copied()
        .collect();
    let mut before = band.emitters;
    for _ in 0..(seconds * f64::from(CENSUS_RATE)) as usize {
        band.next_sample();
        for (now, then) in band.emitters.iter().zip(&before) {
            let new = now.kind != then.kind || now.pitch_hz != then.pitch_hz;
            if now.kind != Kind::Idle && new {
                seen.push(*now);
            }
        }
        before = band.emitters;
    }
    (band, seen)
}

/// `count` stations as they come on to a band with `others` calling, drawn
/// straight from the generator at an ordinary sample rate, heard on the
/// 400-600 Hz side-tone range their levels were measured against: a wider
/// range widens the guard that holds the stations near it under yours.
fn stations(others: u32, count: usize, seed: u64) -> (BandActivity, Vec<Emitter>) {
    let mut settings = TrainingSettings::default();
    settings.band.side_tone_min = 400.0;
    settings.band.side_tone_max = 600.0;
    let mut band = uncapped(48_000, &settings, others, seed);
    let drawn = (0..count).map(|_| band.new_station()).collect();
    (band, drawn)
}

/// One station on its own, from the moment it comes on until it goes: each
/// over as (start, end) in seconds, and the pitch it was sent at.
fn overs(band: &BandActivity, mut station: Emitter) -> Vec<(f64, f64, f64)> {
    let fs = station.sample_rate;
    let dt = f64::from(CONTROL_SAMPLES) / fs;
    let mut overs: Vec<(f64, f64, f64)> = Vec::new();
    let mut last_mark: Option<f64> = None;
    for n in 0..(300.0 * fs) as usize {
        if n % CONTROL_SAMPLES as usize == 0 {
            station.control(dt, CONTROL_SAMPLES);
            // Alone on the band, so there is always room.
            if station.ready {
                station.go();
            }
        }
        if station.kind == Kind::Idle {
            break;
        }
        station.step(&band.traffic);
        if station.key_down {
            let t = n as f64 / fs;
            match (overs.last_mut(), last_mark) {
                (Some(over), Some(last)) if t - last < 2.0 => over.1 = t,
                _ => overs.push((t, t, station.over_hz)),
            }
            last_mark = Some(t);
        }
    }
    overs
}

// ------------------------------------------------------------------ the log

/// What is on the band is nearly all Morse — carriers and sweeps are the odd
/// one out — sent by people at the speeds people send at.
#[test]
fn the_band_is_keyed_cw_at_real_speeds() {
    for others in ORDINARY..=CONTEST {
        let (_, seen) = census(others, 1_800.0, 3);
        let keyed = seen.iter().filter(|e| e.kind == Kind::Keyed).count();
        let share = keyed as f64 / seen.len() as f64;
        assert!(
            share >= 0.95,
            "{others} others: only {share:.3} of it keyed"
        );
        assert!(keyed > 15, "{others} others: {keyed} stations");
    }
    let (_, keyed) = stations(ORDINARY, 2_000, 5);
    let speeds: Vec<f64> = keyed.iter().map(wpm).collect();
    let (p10, p50, p90) = (
        percentile(&speeds, 0.1),
        percentile(&speeds, 0.5),
        percentile(&speeds, 0.9),
    );
    assert!((21.0..26.0).contains(&p50), "median {p50:.1} WPM");
    assert!((15.0..19.5).contains(&p10), "p10 {p10:.1} WPM");
    assert!((33.0..43.0).contains(&p90), "p90 {p90:.1} WPM");
    let dahs: Vec<f64> = keyed.iter().map(|e| e.dah).collect();
    let mean = dahs.iter().sum::<f64>() / dahs.len() as f64;
    let sd = (dahs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / dahs.len() as f64).sqrt();
    assert!((2.95..3.05).contains(&mean), "dah/dit {mean:.3}");
    assert!((0.15..0.25).contains(&sd), "dah/dit spread {sd:.3}");
    // Hard keying: the ones that click through the skirt.
    let hard = keyed
        .iter()
        .filter(|e| e.edge_step * e.sample_rate * HARD_EDGE_SEC > 0.99)
        .count() as f64
        / keyed.len() as f64;
    assert!((0.05..0.16).contains(&hard), "{hard:.3} hard-keyed");
    let drifting =
        keyed.iter().filter(|e| e.drift_hz_per_sec != 0.0).count() as f64 / keyed.len() as f64;
    assert!((0.05..0.16).contains(&drifting), "{drifting:.3} drifting");
    assert!(
        keyed
            .iter()
            .all(|e| e.drift_hz_per_sec.abs() <= DRIFT_MAX_HZ_PER_SEC)
    );
    // A contest is faster.
    let (_, contest) = stations(CONTEST, 2_000, 5);
    let contest: Vec<f64> = contest.iter().map(wpm).collect();
    assert!(percentile(&contest, 0.5) > p50 + 2.0);
}

/// Levels per bin as measured off the air: a median a little over 20 dB, a
/// long loud tail, nothing under 10 or over 50 — and a contest louder.
#[test]
fn stations_are_as_loud_as_real_ones() {
    let levels = |others: u32, seed: u64| {
        let (band, drawn) = stations(others, 2_000, seed);
        drawn
            .iter()
            .map(|e| level_db(&band, e))
            .collect::<Vec<f64>>()
    };
    let normal = levels(ORDINARY, 7);
    let (p10, p50, p90) = (
        percentile(&normal, 0.1),
        percentile(&normal, 0.5),
        percentile(&normal, 0.9),
    );
    assert!((19.0..25.0).contains(&p50), "median {p50:.1} dB");
    assert!((10.0..15.0).contains(&p10), "p10 {p10:.1} dB");
    assert!((31.0..40.0).contains(&p90), "p90 {p90:.1} dB");
    let (low, high) = LEVEL_LIMITS_DB;
    assert!(
        normal
            .iter()
            .all(|l| (low - 1e-9..=high + 1e-9).contains(l))
    );
    let contest = percentile(&levels(CONTEST, 7), 0.5);
    assert!(
        (5.0..11.0).contains(&(contest - p50)),
        "contest median {contest:.1} against {p50:.1}"
    );
}

/// Overs of a few seconds — a CQ, a report, a "TU" — one to three of them, a
/// few seconds apart, and each back where the last one was.
#[test]
fn stations_send_overs_of_a_few_seconds() {
    let mut band = uncapped(1_000, &TrainingSettings::default(), ORDINARY, 11);
    let mut lengths = Vec::new();
    let mut counts = Vec::new();
    let mut gaps = Vec::new();
    for _ in 0..300 {
        let station = band.new_station();
        let sent = overs(&band, station);
        counts.push(sent.len());
        lengths.extend(sent.iter().map(|(start, end, _)| end - start));
        gaps.extend(sent.windows(2).map(|w| w[1].0 - w[0].1));
        for (_, _, hz) in &sent {
            assert!((hz - station.pitch_hz).abs() <= OVER_PITCH_JITTER_HZ + 1e-9);
        }
    }
    let median = percentile(&lengths, 0.5);
    let (p10, p90) = (percentile(&lengths, 0.1), percentile(&lengths, 0.9));
    assert!((3.5..6.0).contains(&median), "median over {median:.2} s");
    assert!(p10 > 1.0, "p10 {p10:.2} s");
    assert!(p90 < 20.0, "p90 {p90:.1} s");
    assert!(counts.iter().all(|c| (1..=3).contains(c)), "{counts:?}");
    let mean = counts.iter().sum::<usize>() as f64 / counts.len() as f64;
    assert!((1.8..2.2).contains(&mean), "{mean:.2} overs a station");
    // Measured from the last mark to the next, so the drawn silence plus the
    // over's trailing space.
    assert!(
        gaps.iter().all(|g| (2.9..15.5).contains(g)),
        "gaps {gaps:?}"
    );
}

// ---------------------------------------------------------------- listening

/// What a listener hears of the band, measured the way the recordings were:
/// a Hann STFT, with a station counted wherever its bin stands 10 dB over the
/// noise the reference floor would put in that bin.
struct Heard {
    /// Share of one-second stretches with a station within 600 Hz of the
    /// pitch for at least 0.2 s of it.
    near: f64,
    /// The same within 250 Hz: in the passband itself.
    inside: f64,
    /// How many separate tones stand out at once, on average, anywhere.
    tones: f64,
}

/// `hop_sec` should divide 0.2 s; a one-second stretch is judged on the
/// frames that start in it.
fn listen(x: &[f64], fs: u32, points: usize, hop_sec: f64, center: f64) -> Heard {
    let fs = f64::from(fs);
    let window = hann(points);
    let energy: f64 = window.iter().map(|w| w * w).sum();
    // The reference floor is white at this variance, and the filter's
    // passband leaves it so.
    let variance = reference_floor_rms().powi(2) * fs / (2.0 * REFERENCE_BANDWIDTH_HZ);
    let threshold = 10.0 * variance * energy;
    let bin_hz = fs / points as f64;
    let hop = (hop_sec * fs).round() as usize;
    let fft = Fft::new(points);
    let (mut re, mut im) = (vec![0.0; points], vec![0.0; points]);
    let mut power = vec![0.0; points / 2];
    let (mut near, mut inside, mut tones) = (Vec::new(), Vec::new(), 0usize);
    let mut start = 0;
    while start + points <= x.len() {
        for i in 0..points {
            re[i] = x[start + i] * window[i];
            im[i] = 0.0;
        }
        fft.run(&mut re, &mut im);
        for (k, p) in power.iter_mut().enumerate() {
            *p = re[k] * re[k] + im[k] * im[k];
        }
        let (mut a, mut b) = (false, false);
        for k in 1..points / 2 - 1 {
            if power[k] < threshold {
                continue;
            }
            let offset = (k as f64 * bin_hz - center).abs();
            a |= offset <= 600.0;
            b |= offset <= 250.0;
            if power[k] >= power[k - 1] && power[k] > power[k + 1] {
                tones += 1;
            }
        }
        near.push(a);
        inside.push(b);
        start += hop;
    }
    let per_second = (1.0 / hop_sec).round() as usize;
    let needed = (0.2 / hop_sec).round() as usize;
    let share = |frames: &[bool]| {
        let seconds = frames.len() / per_second;
        (0..seconds)
            .filter(|s| {
                frames[s * per_second..(s + 1) * per_second]
                    .iter()
                    .filter(|f| **f)
                    .count()
                    >= needed
            })
            .count() as f64
            / seconds.max(1) as f64
    };
    Heard {
        near: share(&near),
        inside: share(&inside),
        tones: tones as f64 / near.len().max(1) as f64,
    }
}

/// The keyed traffic alone: carriers and sweeps have their own, much rarer,
/// statistics, and would swamp a session they happened to land in.
fn stations_only(mut band: BandActivity) -> BandActivity {
    for emitter in &mut band.emitters {
        if emitter.kind != Kind::Keyed {
            *emitter = Emitter::default();
        }
    }
    band.next_chirp = u64::MAX;
    band
}

/// `seconds` of the band's stations through the receiver's filter.
fn through_filter(
    settings: &TrainingSettings,
    others: u32,
    fs: u32,
    seconds: f64,
    seed: u64,
) -> Vec<f64> {
    through_receiver(uncapped(fs, settings, others, seed), settings, fs, seconds)
}

/// `seconds` of a band's stations through the receiver `settings` describe.
fn through_receiver(
    band: BandActivity,
    settings: &TrainingSettings,
    fs: u32,
    seconds: f64,
) -> Vec<f64> {
    let mut band = stations_only(band);
    let mut filter = receiver_for(fs, settings);
    (0..(seconds * f64::from(fs)) as usize)
        .map(|_| filter.process(band.next_sample()))
        .collect()
}

/// Three ten-minute sessions of the band `settings` describe, through the
/// receiver they describe.
fn heard_with(settings: &TrainingSettings) -> (f64, f64) {
    let center = settings.side_tone_center();
    // Six ten-minute sessions: with three, one step of the setting could not
    // be told from the next by more than chance.
    let seeds = [21, 22, 23, 24, 25, 26];
    let (mut near, mut inside) = (0.0, 0.0);
    for seed in seeds {
        let band = calling_band(LISTEN_RATE, settings, seed);
        let x = through_receiver(band, settings, LISTEN_RATE, 600.0);
        let heard = listen(&x, LISTEN_RATE, LISTEN_POINTS, 0.1, center);
        near += heard.near / seeds.len() as f64;
        inside += heard.inside / seeds.len() as f64;
    }
    eprintln!(
        "{} calling, {:?} {:.0} Hz: near {near:.3} inside {inside:.3}",
        settings.band.stations_max, settings.band.filter_shape, settings.band.filter_width_hz
    );
    (near, inside)
}

/// The calibration target. Through a 500 Hz filter — the one the 20 m
/// recordings were measured through — another station is heard in 0.12-0.53
/// of one-second stretches, and inside the passband in 0.10-0.30. With one
/// other station allowed to call, the band sits in that.
///
/// Six sessions measure it only so closely: other sets of six seeds put the
/// same band anywhere from 0.26 to 0.36 inside the passband. The top of the
/// range is held that loosely, or a change to what the stations send rather
/// than how often they send it — new callsigns, here — fails it by chance.
#[test]
fn one_other_station_is_an_ordinary_band() {
    let (near, inside) = heard_with(&calling_through_500_hz(2));
    assert!((0.15..0.4).contains(&near), "occupied {near:.3}");
    assert!(
        (0.1..0.32).contains(&inside),
        "inside the passband {inside:.3}"
    );
}

/// The contest recording: someone audible about half the time and more. Two
/// others allowed to call is that band.
#[test]
fn two_other_stations_are_a_contest() {
    let (near, inside) = heard_with(&calling_through_500_hz(3));
    assert!((0.42..0.72).contains(&near), "occupied {near:.3}");
    assert!(inside > 0.35, "inside the passband {inside:.3}");
}

/// The same band through the default receiver, the peaked 300 Hz filter, is
/// heard less, because the receiver hears less of it: a typical station has
/// to be within about 150 Hz of the note to come through 10 dB over the
/// noise, where the 500 Hz filter let one through from twice as far out. So
/// with one other station allowed to call, someone is heard in 0.13 of the
/// stretches rather than 0.34 — still at the bottom of what the recordings
/// put inside a passband, 0.10 — and every step of the setting is audibly
/// busier than the last, until with the most calling the band is heard at
/// least as often as an ordinary evening is through the wide filter: 0.49
/// measured, against the recordings' 0.3.
#[test]
fn through_the_default_receiver_every_step_is_busier() {
    let heard: Vec<f64> = (2..=STATIONS_MAX)
        .map(|stations| heard_with(&calling(stations)).0)
        .collect();
    assert!(
        heard.windows(2).all(|w| w[1] > w[0] + 0.05),
        "a step of the setting was not audibly busier: {heard:?}"
    );
    assert!((0.1..0.3).contains(&heard[0]), "ordinary band: {heard:?}");
    assert!(heard[heard.len() - 1] >= 0.3, "busiest band: {heard:?}");
}

/// Opening the filter up lets more of the band in: through 2 kHz there are
/// several times as many stations to hear as through 500 Hz.
#[test]
fn a_wider_filter_lets_more_stations_in() {
    let tones = |bandwidth: f64| {
        let mut settings = TrainingSettings::default();
        settings.band.filter_width_hz = bandwidth;
        let settings = settings.clamp();
        let mut total = 0.0;
        for seed in [31, 32] {
            // 12 kHz: the widest filter reaches past 3 kHz.
            let x = through_filter(&settings, ORDINARY, 12_000, 300.0, seed);
            total += listen(&x, 12_000, 2_048, 0.1, settings.side_tone_center()).tones;
        }
        total
    };
    let (narrow, wide) = (tones(500.0), tones(FILTER_WIDTH_MAX));
    eprintln!("tones narrow {narrow:.3} wide {wide:.3}");
    assert!(narrow > 0.0);
    assert!(
        wide >= 2.5 * narrow,
        "{wide:.3} tones through 2 kHz against {narrow:.3} through 500 Hz"
    );
}

// ------------------------------------------------------------ the count

/// How many stations within earshot are keying, sampled every control block
/// across `seconds` of band.
fn keying_over(band: &mut BandActivity, seconds: f64) -> Vec<usize> {
    let block = CONTROL_SAMPLES as usize;
    let blocks = (seconds * band.sample_rate) as usize / block;
    (0..blocks)
        .map(|_| {
            for _ in 0..block {
                band.next_sample();
            }
            band.keying_heard()
        })
        .collect()
}

/// One station calling is the one you want, on its own: the band has nobody
/// else on it at all.
#[test]
fn one_station_calling_is_an_empty_band() {
    assert!(band_for(8_000, &calling(1), 1).is_none());
    assert!(TrafficProfile::for_others(0).is_none());
}

/// However many may call, never more than the rest of them key at once
/// within earshot — and the more that may, the busier the band, until at two
/// the one other station is often there and at the most the band is rarely
/// empty for long.
///
/// The cap is a cap that is reached, not a number nobody hears: the whole
/// count is keying at once a tenth of the time and more through the 500 Hz
/// filter. Through the default 300 Hz one, earshot — wherever the receiver
/// is no more than 45 dB down — is 494 Hz across against that filter's
/// 880 Hz, so about half as many stations are in it at any moment, and all
/// of the others being there at once is rarer still: measured, 0.30, 0.15,
/// 0.048 and 0.028 of the time for two to five calling. There it has to be
/// reached a fiftieth of the time, half a minute in every twenty-five.
#[test]
fn never_more_than_the_rest_of_the_count_key_at_once() {
    for (receiver, reached) in [(0, 0.02), (1, 0.1)] {
        let settings = |stations: u32| match receiver {
            0 => calling(stations),
            _ => calling_through_500_hz(stations),
        };
        let mut busy = Vec::new();
        for stations in 2..=STATIONS_MAX {
            let others = (stations - 1) as usize;
            let (mut on, mut full) = (0.0, 0.0);
            let mut most = 0;
            // Twelve independent bands: three were too few to tell one step
            // of the setting from the next by more than chance.
            const SEEDS: u64 = 12;
            for seed in 41..41 + SEEDS {
                let mut band = calling_band(CENSUS_RATE, &settings(stations), seed);
                let counts = keying_over(&mut band, 900.0);
                most = most.max(counts.iter().copied().max().unwrap_or(0));
                let n = counts.len() as f64 * SEEDS as f64;
                on += counts.iter().filter(|c| **c > 0).count() as f64 / n;
                full += counts.iter().filter(|c| **c >= others).count() as f64 / n;
            }
            let name = format!(
                "{stations} calling, {:?}",
                settings(stations).band.filter_shape
            );
            eprintln!("{name}: on {on:.3}, full {full:.3}");
            assert!(
                most <= others,
                "{name}: {most} other stations keyed at once"
            );
            assert!(full > reached, "{name}: full only {full:.3} of the time");
            busy.push(on);
        }
        assert!(
            busy.windows(2).all(|w| w[1] > w[0]),
            "more calling did not make a busier band: {busy:?}"
        );
    }
}

/// While a pile-up calls, its callers use the band's count: once the overs
/// already under way have finished, nobody new starts one until the pile-up
/// is over, and then the band comes back.
#[test]
fn a_pile_up_holds_the_band_back_while_it_calls() {
    let mut band = calling_band(CENSUS_RATE, &calling(3), 7);
    keying_over(&mut band, 60.0);
    band.reserve(2);
    // An over runs to the end of its word, 20 s at the very longest.
    keying_over(&mut band, 25.0);
    let held = keying_over(&mut band, 120.0);
    assert!(held.iter().all(|c| *c == 0), "keyed during the pile-up");
    band.reserve(0);
    let after = keying_over(&mut band, 120.0);
    assert!(after.iter().any(|c| *c > 0), "the band never came back");
}

/// While a pile-up calls, the band and the pile-up between them never have
/// more keying than the rest of the count — once the stations caught keying
/// when it starts have finished the character they were in.
#[test]
fn a_pile_up_and_the_band_together_stay_within_the_count() {
    // A four-second send every ten seconds, the way a session runs.
    const SEND_SEC: f64 = 4.0;
    const EVERY_SEC: f64 = 10.0;
    // The longest character at the slowest fist: five dahs at 12 WPM.
    const BREAK_OFF_SEC: f64 = 2.5;
    for stations in 2..=STATIONS_MAX {
        let others = (stations - 1) as usize;
        let mut band = calling_band(CENSUS_RATE, &calling(stations), 13);
        let block = CONTROL_SAMPLES as usize;
        let block_sec = block as f64 / f64::from(CENSUS_RATE);
        let mut worst = 0;
        for n in 0..(900.0 / block_sec) as usize {
            let t = n as f64 * block_sec;
            let into = t % EVERY_SEC;
            let callers = if into < SEND_SEC { others } else { 0 };
            // Every block, as the native player does every buffer.
            band.reserve(callers);
            for _ in 0..block {
                band.next_sample();
            }
            if into >= BREAK_OFF_SEC {
                worst = worst.max(band.keying_heard() + band.reserved);
            }
        }
        assert!(
            worst <= others,
            "{stations} calling: {worst} on the air with the pile-up"
        );
    }
}

/// A station whose turn comes on a full band waits for a gap rather than
/// piling in, and one that waits too long goes elsewhere, giving its slot up
/// rather than holding it for ever.
#[test]
fn a_station_that_finds_the_band_full_waits_and_then_gives_up() {
    let mut band = calling_band(CENSUS_RATE, &calling(2), 9);
    // A pile-up as big as the count: nobody heard may start at all.
    band.reserve(1);
    let give_up = (GIVE_UP_SEC * f64::from(CENSUS_RATE)) as u32;
    let mut before = band.emitters;
    let (mut longest, mut gave_up) = (0, 0);
    for _ in 0..(300 * CENSUS_RATE) {
        band.next_sample();
        for (now, then) in band.emitters.iter().zip(&before) {
            if now.ready {
                longest = longest.max(now.waited);
            }
            // Waiting a moment ago, and now done with the band without
            // having started: it gave up, and its slot goes at the next look.
            let same = now.kind == then.kind && now.pitch_hz == then.pitch_hz;
            if same && then.ready && !now.ready && now.overs_left == 0 {
                gave_up += 1;
            }
        }
        before = band.emitters;
    }
    assert!(longest < give_up, "a station waited {longest} samples");
    assert!(gave_up > 0, "nobody ever gave up and freed a slot");
}

/// The band is told how many are calling on every buffer, pile-up or not; with
/// room to spare that must leave every over to end where it would have.
#[test]
fn being_told_there_is_room_changes_nothing() {
    let settings = calling(3);
    let mut told = calling_band(CENSUS_RATE, &settings, 17);
    let mut left = calling_band(CENSUS_RATE, &settings, 17);
    for _ in 0..(600 * CENSUS_RATE) {
        told.reserve(0);
        assert_eq!(told.next_sample().to_bits(), left.next_sample().to_bits());
    }
}

/// Inside the wanted station's pitch range nothing keyed is heard within 6 dB
/// of it — as heard, through the receiver, with the wanted station at the
/// pitch the receiver passes least well and its quietest send. A peaked
/// receiver takes a few decibels off the ends of the range; the guard has to
/// hold there as well as in the middle.
#[test]
fn nothing_keyed_is_heard_near_the_station_you_want() {
    use crate::band::FilterDesign;
    use crate::settings::FilterShape;
    for shape in [FilterShape::Peaked, FilterShape::Sharp, FilterShape::Soft] {
        for (low, high) in [
            (400.0, 600.0),
            (400.0, 800.0),
            (300.0, 900.0),
            (200.0, 1_200.0),
        ] {
            let mut settings = calling(STATIONS_MAX);
            settings.band.filter_shape = shape;
            settings.band.side_tone_min = low;
            settings.band.side_tone_max = high;
            let settings = settings.clamp();
            let design = FilterDesign::from_settings(&settings);
            let weakest = (0..=64)
                .map(|i| design.response_at(low + (high - low) * f64::from(i) / 64.0))
                .fold(f64::INFINITY, f64::min);
            let volume = settings.band.volume_min.clamp(0.1, 1.0);
            let wanted_heard = 20.0
                * (SIGNAL_RMS * volume * weakest / reference_floor_rms()).log10()
                + bin_over_reference_db();
            let (band, drawn) = {
                let mut band = calling_band(48_000, &settings, 23);
                let drawn: Vec<Emitter> = (0..3_000).map(|_| band.new_station()).collect();
                (band, drawn)
            };
            let guard = (low - GUARD_MARGIN_HZ)..=(high + GUARD_MARGIN_HZ);
            for station in drawn.iter().filter(|e| guard.contains(&e.pitch_hz)) {
                // At the top of its own fading, and wherever drift or a new
                // over can take it within the guard.
                let reach = OVER_PITCH_JITTER_HZ
                    + if station.drift_hz_per_sec == 0.0 {
                        0.0
                    } else {
                        DRIFT_MAX_HZ
                    };
                let loudest = (0..=8)
                    .map(|i| {
                        design.response_at(
                            station.pitch_hz - reach + 2.0 * reach * f64::from(i) / 8.0,
                        )
                    })
                    .fold(0.0f64, f64::max);
                let heard =
                    level_db(&band, station) + station.fade.swing_db + 20.0 * loudest.log10();
                assert!(
                    heard <= wanted_heard - GUARD_KEYED_DB + 0.1,
                    "{shape:?} {low}-{high}: a station at {:.0} Hz is heard {:.2} dB under the one you want",
                    station.pitch_hz,
                    wanted_heard - heard
                );
            }
        }
    }
}

/// The band's stations rise and fall on their paths as they do on the air,
/// by several decibels within an over, not by the decibel the ear can barely
/// tell: the typical keyed station fades by a standard deviation over 2.5 dB.
#[test]
fn the_bands_stations_fade_as_heard_on_the_air() {
    let (_, seen) = census(4, 3_600.0, 41);
    let mut fades: Vec<f64> = seen
        .iter()
        .filter(|e| e.kind == Kind::Keyed)
        .map(|e| e.fade.swing_db * fade_rms())
        .collect();
    assert!(fades.len() > 200, "{} stations", fades.len());
    fades.sort_by(f64::total_cmp);
    let median = fades[fades.len() / 2];
    assert!(median > 2.5, "a typical station fades by {median:.2} dB");
    assert!(
        fades
            .iter()
            .all(|sd| (QSB_SD_DB.0..=QSB_SD_DB.1).contains(sd))
    );
}
