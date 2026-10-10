//! The band's traffic, counted and listened to.
//!
//! Two kinds of test. The bookkeeping ones read the generator's own record —
//! every transmitter as it came on — and hold it to the measured statistics.
//! The listening ones render the band through the receiver's filter and
//! measure it the way the off-air recordings were measured, because that is
//! the number the calibration is for.

use super::*;
use crate::band::ReceiverFilter;
use crate::settings::FILTER_BANDWIDTH_MAX;

/// Rate for the bookkeeping tests, which never listen: a station's dits are
/// still ten samples long, and an hour of band costs next to nothing.
const CENSUS_RATE: u32 = 200;
/// Rate for the listening tests: the 5.86 Hz analysis bins the recordings were
/// measured with are exactly 1024 points here, and everything a 500 Hz filter
/// lets through is far below the Nyquist frequency.
const LISTEN_RATE: u32 = 6_000;
const LISTEN_POINTS: usize = 1_024;

fn band_at(level: f64) -> TrainingSettings {
    let mut s = TrainingSettings::default();
    s.band.activity_enabled = true;
    s.band.activity_level = level;
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
fn census(settings: &TrainingSettings, seconds: f64, seed: u64) -> (BandActivity, Vec<Emitter>) {
    let mut band = BandActivity::from_settings(CENSUS_RATE, settings, seed);
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

/// `count` stations as they come on to a band at `level`, drawn straight from
/// the generator at an ordinary sample rate.
fn stations(level: f64, count: usize, seed: u64) -> (BandActivity, Vec<Emitter>) {
    let mut band = BandActivity::from_settings(48_000, &band_at(level), seed);
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
    for level in [0.05, 0.5, 1.0] {
        let (_, seen) = census(&band_at(level), 1_800.0, 3);
        let keyed = seen.iter().filter(|e| e.kind == Kind::Keyed).count();
        let share = keyed as f64 / seen.len() as f64;
        assert!(share >= 0.95, "level {level}: only {share:.3} of it keyed");
        assert!(keyed > 15, "level {level}: {keyed} stations");
    }
    let (_, keyed) = stations(0.5, 2_000, 5);
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
    let (_, contest) = stations(1.0, 2_000, 5);
    let contest: Vec<f64> = contest.iter().map(wpm).collect();
    assert!(percentile(&contest, 0.5) > p50 + 2.0);
}

/// Levels per bin as measured off the air: a median a little over 20 dB, a
/// long loud tail, nothing under 10 or over 50 — and a contest louder.
#[test]
fn stations_are_as_loud_as_real_ones() {
    let levels = |level: f64, seed: u64| {
        let (band, drawn) = stations(level, 2_000, seed);
        drawn
            .iter()
            .map(|e| level_db(&band, e))
            .collect::<Vec<f64>>()
    };
    let normal = levels(0.5, 7);
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
    let contest = percentile(&levels(1.0, 7), 0.5);
    assert!(
        (5.0..11.0).contains(&(contest - p50)),
        "contest median {contest:.1} against {p50:.1}"
    );
}

/// Overs of a few seconds — a CQ, a report, a "TU" — one to three of them, a
/// few seconds apart, and each back where the last one was.
#[test]
fn stations_send_overs_of_a_few_seconds() {
    let mut band = BandActivity::from_settings(1_000, &band_at(0.5), 11);
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

/// A radix-2 FFT with its tables built once, which is what keeps a ten-minute
/// listen affordable in a debug build.
struct Fft {
    twiddles: Vec<(f64, f64)>,
    reverse: Vec<usize>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let bits = n.trailing_zeros();
        Self {
            twiddles: (0..n / 2)
                .map(|k| {
                    let (s, c) = (-TAU * k as f64 / n as f64).sin_cos();
                    (c, s)
                })
                .collect(),
            reverse: (0..n)
                .map(|i| i.reverse_bits() >> (usize::BITS - bits))
                .collect(),
        }
    }

    fn run(&self, re: &mut [f64], im: &mut [f64]) {
        let n = re.len();
        for (i, &j) in self.reverse.iter().enumerate() {
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut half = 1;
        while half < n {
            let stride = n / (2 * half);
            for start in (0..n).step_by(2 * half) {
                for k in 0..half {
                    let (c, s) = self.twiddles[k * stride];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            half *= 2;
        }
    }
}

/// What a listener hears of the band, measured the way the recordings were:
/// a Hann STFT, with a station counted wherever its bin stands 10 dB over the
/// noise the default floor would put in that bin.
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
    let window: Vec<f64> = (0..points)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / points as f64).cos())
        .collect();
    let energy: f64 = window.iter().map(|w| w * w).sum();
    // The reference floor is white at this variance, and the filter's
    // passband leaves it so.
    let variance = reference_rms().powi(2) * fs / (2.0 * REFERENCE_BANDWIDTH_HZ);
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
fn through_filter(settings: &TrainingSettings, fs: u32, seconds: f64, seed: u64) -> Vec<f64> {
    let mut band = stations_only(BandActivity::from_settings(fs, settings, seed));
    let mut filter = ReceiverFilter::from_settings(fs, settings);
    (0..(seconds * f64::from(fs)) as usize)
        .map(|_| filter.process(band.next_sample()))
        .collect()
}

/// Three ten-minute sessions at `level`, through the default receiver.
fn heard_at(level: f64) -> (f64, f64) {
    let settings = band_at(level);
    let center = settings.side_tone_center();
    let seeds = [21, 22, 23];
    let (mut near, mut inside) = (0.0, 0.0);
    for seed in seeds {
        let x = through_filter(&settings, LISTEN_RATE, 600.0, seed);
        let heard = listen(&x, LISTEN_RATE, LISTEN_POINTS, 0.1, center);
        near += heard.near / seeds.len() as f64;
        inside += heard.inside / seeds.len() as f64;
    }
    eprintln!("level {level}: near {near:.3} inside {inside:.3}");
    (near, inside)
}

/// The calibration target. Through the default 500 Hz filter another station
/// is heard in 0.12-0.53 of one-second stretches on the 20 m recordings, and
/// inside the passband in 0.10-0.30; the normal band sits in the middle.
#[test]
fn a_normal_band_has_someone_on_it_about_a_third_of_the_time() {
    let (near, inside) = heard_at(0.5);
    assert!((0.15..0.4).contains(&near), "occupied {near:.3}");
    assert!(
        (0.1..0.3).contains(&inside),
        "inside the passband {inside:.3}"
    );
}

/// A quiet evening: 0.055 on the emptiest recording.
#[test]
fn a_quiet_band_is_nearly_empty() {
    let (near, _) = heard_at(0.02);
    assert!((0.01..0.1).contains(&near), "occupied {near:.3}");
}

/// The contest recording: someone audible about half the time and more. Over
/// twenty sessions the generator averages 0.54, and single sessions run
/// 0.36-0.68, so three of them are held to a little under the recording's
/// lower end.
#[test]
fn a_contest_is_busy() {
    let (near, inside) = heard_at(1.0);
    assert!((0.42..0.72).contains(&near), "occupied {near:.3}");
    assert!(inside > 0.35, "inside the passband {inside:.3}");
}

/// Opening the filter up lets more of the band in: through 2 kHz there are
/// several times as many stations to hear as through 500 Hz.
#[test]
fn a_wider_filter_lets_more_stations_in() {
    let tones = |bandwidth: f64| {
        let mut settings = band_at(0.5);
        settings.band.filter_bandwidth_hz = bandwidth;
        let settings = settings.clamp();
        let mut total = 0.0;
        for seed in [31, 32] {
            // 12 kHz: the widest filter reaches past 3 kHz.
            let x = through_filter(&settings, 12_000, 300.0, seed);
            total += listen(&x, 12_000, 2_048, 0.1, settings.side_tone_center()).tones;
        }
        total
    };
    let (narrow, wide) = (tones(500.0), tones(FILTER_BANDWIDTH_MAX));
    eprintln!("tones narrow {narrow:.3} wide {wide:.3}");
    assert!(narrow > 0.0);
    assert!(
        wide >= 2.5 * narrow,
        "{wide:.3} tones through 2 kHz against {narrow:.3} through 500 Hz"
    );
}
