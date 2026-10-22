//! Other stations on the band.
//!
//! A real CW segment is never empty. Behind the station you are copying there
//! is faint Morse at other pitches and speeds, coming and going, now and then
//! a steady carrier. Without it a background is a noise generator — which is
//! what the ear hears as wind.
//!
//! Like the noise floor and the static, the stations here are excitation:
//! tones at their own audio pitch, summed in before the receiver's filter. The
//! filter then does to them exactly what it does to a real band — a station
//! inside the passband comes through, one beside it is pushed down by the
//! skirt, and widening the filter lets more of them in. The AGC and the
//! browser's stream get them for nothing, because they ride the same source.
//!
//! Everything is calibrated against off-air recordings of the 20 m CW band
//! (KiwiSDR, Hermes-Lite 2 and contest captures), measured the way an
//! operator would hear them through a 500 Hz filter — the reference filter,
//! sharp and 500 Hz wide at 3 dB. A narrower receiver hears less of
//! the same band, as a real one does:
//!
//! - **How busy.** Another station is audible in 0.12-0.53 of all one-second
//!   stretches across the recordings, and inside the passband itself in
//!   0.10-0.30 of them; tuned to random spots, the seven recordings average
//!   0.44. With one other station allowed to call, the band here sits at
//!   about 0.3, in the middle of that; with the most, at a contest's 0.5-0.7.
//!   That is 0.3-0.6 stations transmitting per kHz at once on a normal band,
//!   about 0.8 in a contest.
//! - **How loud.** Per 5.86 Hz analysis bin, over the noise in that bin: a
//!   median of about 22 dB with a spread of 9 dB, from barely there to 50 dB.
//!   Contest stations run about 8 dB louder.
//! - **What they send.** Almost all of it is keyed CW — CQ calls, callsigns,
//!   reports, "TU" and "73" — at a median of 22-25 WPM (17 to 38 between the
//!   10th and 90th percentiles), with dahs three dits long give or take. A
//!   station sends one to three overs of a few seconds each, a few seconds
//!   apart, at the same pitch, fading on its own path as it goes. One in ten
//!   drifts; one in ten is hard-keyed, and clicks through the filter's skirt.
//! - **What else.** Steady carriers are rare: about 0.07 per kHz, so one lands
//!   within 700 Hz of your pitch in roughly one session in ten. Most are local
//!   birdies, steady and faint; the rest are transmitted carriers, loud and
//!   fading, and some of those go away again. Very occasionally something
//!   sweeps across the band in under a second.
//!
//! How many of them there are is the same setting as the pile-up: "stations
//! calling at once". At one — just the station you are copying — the band is
//! empty. Above that, never more than the rest of that count are keying at
//! once within earshot of the filter, and a pile-up's callers come out of the
//! same count while they call: a station whose turn comes when the band is
//! full waits for a gap, the way an operator waits for a clear moment. The
//! more that may call, the busier the band, from an ordinary evening at two
//! to a contest at the top.
//!
//! What it deliberately does not do is crowd the station you are copying. This
//! is a trainer: inside that station's pitch range nothing else is allowed
//! within 6 dB of it, and no carrier within 10 dB.
//!
//! Allocation-free per sample: a fixed pool of transmitters, and all the text
//! they send encoded once, when the band is built.

use std::f64::consts::{PI, SQRT_2, TAU};

use super::calibration::{REFERENCE_BANDWIDTH_HZ, SIGNAL_RMS, reference_floor_rms};
use super::dsp::{db_to_gain, finite_or, gain_to_db};
use super::filter::{FilterDesign, passband_edges};
use super::qsb::MULTIPATH_RATES;
use super::random::Draw;
use crate::callsign::{country_of, generate_callsign};
use crate::contest::data::names::draw_name;
use crate::morse::morse_for;
use crate::rng::Rng;
use crate::settings::{BandSettings, STATIONS_MAX, STATIONS_MIN};

/// Transmitters that can be on the air at once, carriers included: enough
/// for the busiest band the stations setting makes, heard through the widest
/// filter. A station that finds no room is simply not heard.
const MAX_EMITTERS: usize = 16;

/// How often the slow parts — fading, drift, a sweep — are brought up to
/// date, in samples. All of them move over seconds, so a few hundred updates
/// a second is far finer than anything audible.
const CONTROL_SAMPLES: u32 = 32;

// ------------------------------------------------------------------ levels

/// The analysis the levels are quoted in: a Hann window with 5.86 Hz bins,
/// 8192 points at 48 kHz, which is what the recordings were measured with.
const ANALYSIS_BIN_HZ: f64 = 48_000.0 / 8_192.0;
/// A Hann window's equivalent noise bandwidth, in bins.
const HANN_NOISE_BINS: f64 = 1.5;

/// How far a tone's level per analysis bin stands above its signal-to-noise
/// ratio in [`REFERENCE_BANDWIDTH_HZ`]: the bin holds all of the tone but only
/// 8.8 Hz worth of the noise. About 17.5 dB.
fn bin_over_reference_db() -> f64 {
    10.0 * (REFERENCE_BANDWIDTH_HZ / (HANN_NOISE_BINS * ANALYSIS_BIN_HZ)).log10()
}

/// Level per bin over the band noise: a median of 22 dB, with the loud tail
/// longer than the quiet one — p10 about 13, p90 35-45 as measured, which is
/// a spread of 9 dB on average but 7 below the median and 12 above it —
/// clipped to what the recordings span.
const LEVEL_MEDIAN_DB: f64 = 22.0;
const LEVEL_SIGMA_DB: (f64, f64) = (7.0, 12.0);
const LEVEL_LIMITS_DB: (f64, f64) = (10.0, 50.0);
/// How much louder the median contest station is: the big stations are all
/// on, and running power.
const CONTEST_LEVEL_RISE_DB: f64 = 8.0;
/// Nothing on the band is louder than this per bin, carriers included, however
/// it fades. The loudest station in any recording was about 50 dB.
const LEVEL_CEILING_DB: f64 = 50.0;

/// How far below the station you are copying a background station has to stay
/// inside that station's pitch range, and how far a carrier has to. A trainer
/// keeps the wanted station the one you hear.
const GUARD_KEYED_DB: f64 = 6.0;
const GUARD_CARRIER_DB: f64 = 10.0;
/// How many points across the side-tone range the guard checks the receiver
/// at, for the pitch it passes least well.
const GUARD_POINTS: usize = 16;
/// How far either side of the wanted station's pitch range the guard reaches.
const GUARD_MARGIN_HZ: f64 = 150.0;

/// How far down the receiver's response a station still counts as heard, in
/// decibels. The loudest stations here are 50 dB over the noise per bin, so
/// 45 dB down they are at most a whisper in it; further out they are gone.
const HEARD_DB: f64 = -45.0;

// ----------------------------------------------------------------- arrivals

/// Stations arriving per kHz per minute for each station besides yours that
/// may call. Chosen so that with one, a 500 Hz filter hears another station
/// in about 30% of one-second stretches, where the recordings put an ordinary
/// band; with two, about 55%, where they put a contest.
const ARRIVALS_NORMAL: f64 = 3.5;

/// Where stations land: from just above the bottom of the audio passband to
/// well past the filter. Below about 100 Hz is the other sideband; far above
/// the filter a station is 40-60 dB down and inaudible — but a wider filter
/// reaches further, and so does this.
const SPAWN_LOW_HZ: f64 = 150.0;
const SPAWN_ABOVE_PITCH_HZ: f64 = 1_500.0;
const SPAWN_ABOVE_EDGE_HZ: f64 = 1_000.0;

/// How long a station is on the band on average, overs and the gaps between
/// them: what a band switched on has already got on it.
const MEAN_LIFE_SEC: f64 = 20.0;

// ------------------------------------------------------------------- keying

/// Speed: log-normal around 23 WPM, steeper below the median than above it
/// (p10 17, p90 38), and a little faster in a contest.
const WPM_MEDIAN: f64 = 23.0;
const CONTEST_WPM_RISE: f64 = 4.0;
const WPM_SIGMA_LN: (f64, f64) = (0.24, 0.39);
const WPM_LIMITS: (f64, f64) = (12.0, 50.0);
/// Dah length in dits: 3.0 with a spread of 0.2 (measured 2.8-3.4).
const DAH_RATIO: (f64, f64) = (3.0, 0.2);
const DAH_LIMITS: (f64, f64) = (2.5, 3.6);
/// How much a fist wanders element to element, at most: hand-sent Morse is
/// not machine-exact.
const FIST_JITTER_MAX: f64 = 0.06;
/// Raised-cosine keying edges. One transmitter in ten is hard-keyed, and its
/// clicks reach through the filter's skirt from well outside the passband.
const EDGE_SEC: (f64, f64) = (0.003, 0.006);
const HARD_KEYED_SHARE: f64 = 0.1;
const HARD_EDGE_SEC: f64 = 0.001;

/// One to three overs a station, each a few seconds of text (median about
/// 4.5 s, 2-20 s), separated by the other side of the contact. An over runs
/// to the end of the word it is in when its time is up, which adds about a
/// second to the time drawn here.
const OVERS: (usize, usize) = (1, 3);
const OVER_TARGET_SEC: (f64, f64) = (3.4, 0.7);
const OVER_TARGET_LIMITS_SEC: (f64, f64) = (1.0, 20.0);
const OVER_GAP_SEC: (f64, f64) = (3.0, 15.0);
/// A station comes back where it was, give or take a couple of hertz.
const OVER_PITCH_JITTER_HZ: f64 = 2.0;

/// One station in ten drifts while it transmits, up to half a hertz a second,
/// and never by more than this in one over.
const DRIFT_SHARE: f64 = 0.1;
const DRIFT_MAX_HZ_PER_SEC: f64 = 0.5;
const DRIFT_MAX_HZ: f64 = 10.0;

/// Each station fades on its own path: slowly (0.05-0.3 Hz) and by up to
/// 10 dB.
const QSB_RATE_HZ: (f64, f64) = (0.05, 0.3);
const QSB_DEPTH_MAX_DB: f64 = 10.0;

/// The callsign tier the band's callsigns are drawn from: every shape of call
/// but portables, which are a small part of real traffic.
const CALL_TIER: u32 = 4;
/// How many stations' worth of text the band is built with. Stations reuse
/// them, as the same few regulars come back on a real band.
const SCRIPTS: usize = 40;
/// The share of that text that is contest traffic, quiet to contest.
const CONTEST_SHARE: (f64, f64) = (0.2, 0.85);

// ----------------------------------------------------------------- carriers

/// Steady carriers per kHz of the band, for the whole session. Measured 0.07
/// per kHz pooled; slightly more here, because the audio band below 150 Hz is
/// never used, so that one still lands within 700 Hz of your pitch in about
/// one session in eleven.
const CARRIERS_PER_KHZ: f64 = 0.085;
/// Most are local birdies — a switch-mode supply somewhere in the house —
/// faint and dead steady. The rest are transmitted carriers, loud and fading
/// on the path, and some of those are switched off again.
const BIRDIE_SHARE: f64 = 0.6;
const BIRDIE_DB: (f64, f64) = (14.0, 22.0);
const BIRDIE_WANDER_DB: (f64, f64) = (0.5, 2.0);
const BIRDIE_WANDER_HZ: f64 = 0.5;
const CARRIER_DB: (f64, f64) = (38.0, 49.0);
const CARRIER_FADE_SD_DB: (f64, f64) = (3.0, 6.0);
const CARRIER_FADE_RATE_HZ: (f64, f64) = (0.03, 0.15);
const CARRIER_ENDING_SHARE: f64 = 0.3;
const CARRIER_LIFE_SEC: (f64, f64) = (20.0, 120.0);
/// A carrier coming on or going off is keyed, not switched.
const CARRIER_EDGE_SEC: f64 = 0.01;

/// Now and then something sweeps across the band, 250-350 Hz in half a second
/// to a second, as loud as a strong station. One turned up in about 150 kHz
/// minutes of recordings, so through a 500 Hz filter it is a once-an-evening
/// event — which is about right for a thing you notice because it is odd.
const CHIRPS_PER_KHZ_MINUTE: f64 = 0.0065;
const CHIRP_SWEEP_HZ: (f64, f64) = (250.0, 350.0);
const CHIRP_SEC: (f64, f64) = (0.5, 1.0);
const CHIRP_DB: (f64, f64) = (31.0, 41.0);
const CHIRP_EDGE_SEC: f64 = 0.05;

/// How much of a station's fading each of its paths carries, on the first
/// of the [`MULTIPATH_RATES`]: weights that sum to one, so the swing spans
/// -1..=1.
const FADE_WEIGHTS: [f64; 3] = [0.5, 0.3, 0.2];
/// The RMS of that sum, for setting a fade by its standard deviation.
fn fade_rms() -> f64 {
    (FADE_WEIGHTS.iter().map(|w| w * w).sum::<f64>() / 2.0).sqrt()
}

/// Background stations, as excitation at audio frequency before the receiver
/// filter. Allocation-free per sample.
#[derive(Clone, Debug)]
pub struct BandActivity {
    sample_rate: f64,
    random: Draw,
    tuning: Tuning,
    /// Amplitude of a tone 0 dB per bin over the reference floor.
    unit: f64,
    level_median_db: f64,
    wpm_median: f64,
    /// Station arrivals per second, across the whole spawn window.
    station_rate: f64,
    chirp_rate: f64,
    next_station: u64,
    next_chirp: u64,
    control_left: u32,
    /// The most stations that may be keying at once within earshot, and how
    /// many of those a pile-up is using right now.
    cap: usize,
    reserved: usize,
    traffic: Traffic,
    emitters: [Emitter; MAX_EMITTERS],
}

/// What the stations turn the band's seed by, so that their draws are never
/// the noise's over again.
const STATIONS_SEED: u64 = 0xB4D5_AC71_7174_C0DE;

/// How long a station waits for a clear moment before it gives up and goes
/// elsewhere, in seconds.
const GIVE_UP_SEC: f64 = 20.0;

/// How busy the band is: what kind of traffic it carries, how much of it
/// arrives, and how many of its stations may key at once within earshot.
#[derive(Clone, Copy, Debug)]
struct TrafficProfile {
    /// How far towards a contest, from 0 to 1: an ordinary evening's traffic,
    /// or a contest's — louder, faster, contest exchanges.
    contest: f64,
    /// Stations arriving per kHz of band per minute.
    arrivals_per_khz_minute: f64,
    /// The most that may be keying at once within earshot.
    cap: usize,
}

impl TrafficProfile {
    /// The band when `others` stations besides yours may call at once, or
    /// none at all when that is nobody.
    ///
    /// One is an ordinary evening and the most the setting allows is a
    /// contest, evenly in between. Each of them brings an ordinary band's
    /// worth of arrivals, so every step of the setting is a band audibly
    /// busier than the last and the count is one the band actually reaches.
    fn for_others(others: u32) -> Option<Self> {
        let most = STATIONS_MAX - STATIONS_MIN;
        let others = others.min(most);
        if others == 0 {
            return None;
        }
        Some(Self {
            contest: f64::from(others - 1) / f64::from((most - 1).max(1)),
            arrivals_per_khz_minute: ARRIVALS_NORMAL * f64::from(others),
            cap: others as usize,
        })
    }
}

/// Where the band is listened to from: which pitches stations may land on,
/// and the station they have to keep clear of.
#[derive(Clone, Copy, Debug)]
struct Tuning {
    low_hz: f64,
    high_hz: f64,
    guard_low_hz: f64,
    guard_high_hz: f64,
    /// Where a station is near enough the filter to be heard, and so to count
    /// against how many may be keying at once: wherever the receiver's real
    /// response is no more than [`HEARD_DB`] down. Past that even the loudest
    /// station on the band is down at the noise.
    heard_low_hz: f64,
    heard_high_hz: f64,
    /// The wanted station's key-down level per bin over the reference floor.
    wanted_db: f64,
}

impl Tuning {
    /// The band `band` describes, listened to through `receiver`: the filter
    /// as it is really built, at least wide enough for the side-tone range,
    /// and with the skirts its shape gives it.
    fn new(band: &BandSettings, receiver: &FilterDesign) -> Self {
        let defaults = BandSettings::default();
        let tone_min = finite_or(band.side_tone_min, defaults.side_tone_min).max(100.0);
        let tone_max = finite_or(band.side_tone_max, defaults.side_tone_max).max(tone_min);
        let center = tone_min + (tone_max - tone_min) / 2.0;
        let (_, upper_edge) = passband_edges(center, receiver.width_hz());
        let (heard_low_hz, heard_high_hz) = receiver.edges_at(HEARD_DB);
        let high = (center + SPAWN_ABOVE_PITCH_HZ).max(upper_edge + SPAWN_ABOVE_EDGE_HZ);
        // The quietest the wanted station is ever heard, against the floor
        // every level here is measured from: its quietest send, at the pitch
        // the receiver passes least well, set against a background station at
        // the pitch it passes best anywhere the guard reaches. A peaked
        // receiver takes a few decibels off the ends of the side-tone range,
        // and the guard has to hold there too: the margin is as heard, not as
        // sent.
        let volume = finite_or(band.volume_min, defaults.volume_min).clamp(0.1, 1.0);
        let response_over = |low: f64, high: f64| {
            (0..=GUARD_POINTS).map(move |i| {
                receiver.response_at(low + (high - low) * i as f64 / GUARD_POINTS as f64)
            })
        };
        let weakest = response_over(tone_min, tone_max).fold(1.0f64, f64::min);
        let loudest = response_over(tone_min - GUARD_MARGIN_HZ, tone_max + GUARD_MARGIN_HZ)
            .fold(1.0f64, f64::max);
        let wanted_rms = SIGNAL_RMS * volume * (weakest / loudest).max(1e-6);
        Self {
            low_hz: SPAWN_LOW_HZ,
            high_hz: high.max(SPAWN_LOW_HZ + 500.0),
            guard_low_hz: tone_min - GUARD_MARGIN_HZ,
            guard_high_hz: tone_max + GUARD_MARGIN_HZ,
            heard_low_hz,
            heard_high_hz,
            wanted_db: gain_to_db(wanted_rms / reference_floor_rms()) + bin_over_reference_db(),
        }
    }

    fn width_khz(&self) -> f64 {
        (self.high_hz - self.low_hz) / 1_000.0
    }

    fn heard(&self, hz: f64) -> bool {
        (self.heard_low_hz..=self.heard_high_hz).contains(&hz)
    }

    /// The loudest a transmitter spanning `low..=high` hertz may be: the
    /// ceiling, or `below` decibels under the wanted station if it reaches
    /// into the guarded range.
    fn cap_db(&self, low: f64, high: f64, below: f64) -> f64 {
        if high >= self.guard_low_hz && low <= self.guard_high_hz {
            (self.wanted_db - below).min(LEVEL_CEILING_DB)
        } else {
            LEVEL_CEILING_DB
        }
    }
}

impl BandActivity {
    /// The band `band` describes, heard through `receiver`: as busy as the
    /// number of stations that may call at once allows, and none at all when
    /// that is only yours. `seed` is the band's own, which the stations turn
    /// into one of their own.
    pub fn for_band(
        sample_rate: u32,
        band: &BandSettings,
        receiver: &FilterDesign,
        seed: u64,
    ) -> Option<Self> {
        let others = band.stations_max.clamp(STATIONS_MIN, STATIONS_MAX) - STATIONS_MIN;
        let profile = TrafficProfile::for_others(others)?;
        Some(Self::new(
            sample_rate,
            profile,
            Tuning::new(band, receiver),
            seed,
        ))
    }

    /// A band carrying `profile`'s traffic, listened to from `tuning`. One
    /// with no station on it at this moment is still a band — someone will
    /// be along.
    fn new(sample_rate: u32, profile: TrafficProfile, tuning: Tuning, seed: u64) -> Self {
        let fs = f64::from(sample_rate.max(1));
        let contest = profile.contest.clamp(0.0, 1.0);
        let mut band = Self {
            sample_rate: fs,
            tuning,
            unit: SQRT_2 * reference_floor_rms() * db_to_gain(-bin_over_reference_db()),
            level_median_db: LEVEL_MEDIAN_DB + CONTEST_LEVEL_RISE_DB * contest,
            wpm_median: WPM_MEDIAN + CONTEST_WPM_RISE * contest,
            station_rate: profile.arrivals_per_khz_minute.max(0.0) * tuning.width_khz() / 60.0,
            chirp_rate: CHIRPS_PER_KHZ_MINUTE * tuning.width_khz() / 60.0,
            next_station: u64::MAX,
            next_chirp: u64::MAX,
            control_left: 0,
            cap: profile.cap,
            reserved: 0,
            traffic: Traffic::default(),
            emitters: [Emitter::default(); MAX_EMITTERS],
            random: Draw::new(seed ^ STATIONS_SEED),
        };
        let contest_share = CONTEST_SHARE.0 + (CONTEST_SHARE.1 - CONTEST_SHARE.0) * contest;
        band.traffic = Traffic::write(contest_share, &mut band.random);
        band.light_carriers();
        band.populate();
        band.next_station = band.random.wait(band.station_rate, fs);
        band.next_chirp = band.random.wait(band.chirp_rate, fs);
        band
    }

    /// A pile-up is calling with `callers` stations besides yours: they come
    /// out of the count of stations that may key at once until it is over.
    /// Nobody new starts an over until there is room, and of the stations
    /// already keying within earshot, any beyond what is left of the count
    /// finish the character they are in and stand by — the way a station
    /// hearing the frequency fill up breaks off.
    pub fn reserve(&mut self, callers: usize) {
        self.reserved = callers;
        let mut keep = self.cap.saturating_sub(callers);
        // Called every buffer: with room to spare, nobody is cut short.
        if self.keying_heard() <= keep {
            return;
        }
        let tuning = self.tuning;
        for emitter in self
            .emitters
            .iter_mut()
            .filter(|e| e.on_air() && tuning.heard(e.pitch_hz))
        {
            // One whose over is ending anyway is not kept; it just ends sooner.
            // The choice among the rest is the same every call, so nobody kept
            // once is cut the next time round.
            if keep > 0 && !emitter.winding_up() {
                keep -= 1;
            } else {
                emitter.wind_up();
            }
        }
    }

    pub fn next_sample(&mut self) -> f64 {
        if self.control_left == 0 {
            self.control();
            self.control_left = CONTROL_SAMPLES;
        }
        self.control_left -= 1;
        if self.next_station == 0 {
            let station = self.new_station();
            self.place(station);
            self.next_station = self.random.wait(self.station_rate, self.sample_rate);
        } else {
            self.next_station -= 1;
        }
        if self.next_chirp == 0 {
            let chirp = self.new_chirp();
            self.place(chirp);
            self.next_chirp = self.random.wait(self.chirp_rate, self.sample_rate);
        } else {
            self.next_chirp -= 1;
        }
        let traffic = &self.traffic;
        let mut out = 0.0;
        for emitter in &mut self.emitters {
            if emitter.kind != Kind::Idle {
                out += emitter.step(traffic);
            }
        }
        out
    }

    fn control(&mut self) {
        let dt = f64::from(CONTROL_SAMPLES) / self.sample_rate;
        for emitter in &mut self.emitters {
            emitter.control(dt, CONTROL_SAMPLES);
        }
        self.let_stations_on();
    }

    /// Stations within earshot keying right now.
    fn keying_heard(&self) -> usize {
        self.emitters
            .iter()
            .filter(|e| e.on_air() && self.tuning.heard(e.pitch_hz))
            .count()
    }

    /// Let stations whose turn has come start their overs, as far as the
    /// count allows. One out of earshot never waits: it is not one of the
    /// stations you are hearing.
    fn let_stations_on(&mut self) {
        let mut room = self
            .cap
            .saturating_sub(self.reserved)
            .saturating_sub(self.keying_heard());
        let tuning = self.tuning;
        // Out of earshot never waits: it is not one of the stations you hear.
        for emitter in self
            .emitters
            .iter_mut()
            .filter(|e| e.ready && !tuning.heard(e.pitch_hz))
        {
            emitter.go();
        }
        // Whoever has waited longest goes first.
        while room > 0 {
            let next = self
                .emitters
                .iter_mut()
                .filter(|e| e.ready)
                .max_by_key(|e| e.waited);
            let Some(emitter) = next else { break };
            emitter.go();
            room -= 1;
        }
        let give_up = (GIVE_UP_SEC * self.sample_rate) as u32;
        for emitter in self.emitters.iter_mut().filter(|e| e.ready) {
            emitter.waited = emitter.waited.saturating_add(CONTROL_SAMPLES);
            if emitter.waited >= give_up {
                emitter.ready = false;
                emitter.overs_left = 0;
            }
        }
    }

    /// Put a transmitter on the air, if there is a free slot for it.
    fn place(&mut self, emitter: Emitter) -> bool {
        match self.emitters.iter_mut().find(|e| e.kind == Kind::Idle) {
            Some(slot) => {
                *slot = emitter;
                true
            }
            None => false,
        }
    }

    /// Peak amplitude of a tone `db` per bin over the reference floor.
    fn amplitude(&self, db: f64) -> f64 {
        self.unit * db_to_gain(db)
    }

    /// A level from the measured spread, kept under `cap`. One drawn over it
    /// is drawn again, so the guard thins the loud end out rather than piling
    /// stations up just under the line.
    fn draw_level(&mut self, cap: f64) -> f64 {
        let (low, high) = LEVEL_LIMITS_DB;
        for _ in 0..8 {
            let z = self.random.normal();
            let sigma = if z < 0.0 {
                LEVEL_SIGMA_DB.0
            } else {
                LEVEL_SIGMA_DB.1
            };
            let db = (self.level_median_db + sigma * z).clamp(low, high);
            if db <= cap {
                return db;
            }
        }
        cap.min(high)
    }

    /// A station that has just come on: where, how loud, how fast, and what it
    /// is going to send.
    fn new_station(&mut self) -> Emitter {
        let pitch = self.random.between(self.tuning.low_hz, self.tuning.high_hz);
        let drift = if self.random.chance(DRIFT_SHARE) {
            self.random
                .between(-DRIFT_MAX_HZ_PER_SEC, DRIFT_MAX_HZ_PER_SEC)
        } else {
            0.0
        };
        // The station's level is where it sits on average; its own fading
        // swings it either side, and the top of the swing is what the guard
        // and the ceiling hold down.
        let depth = self.random.between(0.0, QSB_DEPTH_MAX_DB);
        let qsb_rate = self.random.between(QSB_RATE_HZ.0, QSB_RATE_HZ.1);
        let fade = Fade::new(&mut self.random, qsb_rate, depth / 2.0, 0.0);
        let reach = OVER_PITCH_JITTER_HZ + if drift == 0.0 { 0.0 } else { DRIFT_MAX_HZ };
        let cap = self
            .tuning
            .cap_db(pitch - reach, pitch + reach, GUARD_KEYED_DB);
        let level_db = self.draw_level(cap - fade.swing_db);
        let wpm = self
            .random
            .split_log_normal(self.wpm_median, WPM_SIGMA_LN)
            .clamp(WPM_LIMITS.0, WPM_LIMITS.1);
        let dah =
            (DAH_RATIO.0 + DAH_RATIO.1 * self.random.normal()).clamp(DAH_LIMITS.0, DAH_LIMITS.1);
        let edge_sec = if self.random.chance(HARD_KEYED_SHARE) {
            HARD_EDGE_SEC
        } else {
            self.random.between(EDGE_SEC.0, EDGE_SEC.1)
        };
        let (first, last) = self
            .traffic
            .script(self.random.index(self.traffic.scripts()));
        let overs = self.random.index(OVERS.1 - OVERS.0 + 1) + OVERS.0;
        let mut station = Emitter {
            kind: Kind::Keyed,
            draw: Draw::new(self.random.next_u64()),
            sample_rate: self.sample_rate,
            pitch_hz: pitch,
            peak: self.amplitude(level_db),
            fade,
            drift_hz_per_sec: drift,
            unit: 1.2 / wpm * self.sample_rate,
            dah,
            jitter: self.random.between(0.0, FIST_JITTER_MAX),
            edge_step: 1.0 / (edge_sec * self.sample_rate).max(1.0),
            script: (first, last),
            overs_left: overs.min(usize::from(u8::MAX)) as u8,
            life_left: u64::MAX,
            ..Emitter::default()
        };
        station.start(self.random.f64());
        station.enter_phrase(&self.traffic, first);
        // On the air as soon as there is room for it.
        station.ready = true;
        station
    }

    /// A sweep across the band: some piece of equipment somewhere being tuned
    /// or switched on.
    fn new_chirp(&mut self) -> Emitter {
        let sweep = self.random.between(CHIRP_SWEEP_HZ.0, CHIRP_SWEEP_HZ.1);
        let seconds = self.random.between(CHIRP_SEC.0, CHIRP_SEC.1);
        let span = (self.tuning.high_hz - self.tuning.low_hz - sweep).max(0.0);
        let low = self.tuning.low_hz + self.random.f64() * span;
        let upward = self.random.chance(0.5);
        let start = if upward { low } else { low + sweep };
        let cap = self.tuning.cap_db(low, low + sweep, GUARD_KEYED_DB);
        let level_db = self.random.between(CHIRP_DB.0, CHIRP_DB.1).min(cap);
        let mut chirp = Emitter {
            kind: Kind::Chirp,
            sample_rate: self.sample_rate,
            pitch_hz: start,
            over_hz: start,
            sweep_hz_per_sec: if upward { sweep } else { -sweep } / seconds,
            peak: self.amplitude(level_db),
            fade: Fade::steady(),
            key_down: true,
            edge_step: 1.0 / (CHIRP_EDGE_SEC * self.sample_rate).max(1.0),
            life_left: (seconds * self.sample_rate).round().max(1.0) as u64,
            ..Emitter::default()
        };
        chirp.start(self.random.f64());
        chirp
    }

    /// The carriers this session has: a Poisson number across the band,
    /// most of them on for the whole of it.
    fn light_carriers(&mut self) {
        let count = self
            .random
            .poisson(CARRIERS_PER_KHZ * self.tuning.width_khz(), 4 * MAX_EMITTERS);
        for _ in 0..count {
            let carrier = self.new_carrier();
            self.place(carrier);
        }
    }

    fn new_carrier(&mut self) -> Emitter {
        let pitch = self.random.between(self.tuning.low_hz, self.tuning.high_hz);
        let birdie = self.random.chance(BIRDIE_SHARE);
        let (kind, level_db, fade, life) = if birdie {
            let swing = self.random.between(BIRDIE_WANDER_DB.0, BIRDIE_WANDER_DB.1) / 2.0;
            let rate = self.random.between(0.01, 0.05);
            let level = self.random.between(BIRDIE_DB.0, BIRDIE_DB.1);
            (
                Kind::Birdie,
                level,
                Fade::new(&mut self.random, rate, swing, 0.0),
                u64::MAX,
            )
        } else {
            let sd = self
                .random
                .between(CARRIER_FADE_SD_DB.0, CARRIER_FADE_SD_DB.1);
            let rate = self
                .random
                .between(CARRIER_FADE_RATE_HZ.0, CARRIER_FADE_RATE_HZ.1);
            let level = self.random.between(CARRIER_DB.0, CARRIER_DB.1);
            let life = if self.random.chance(CARRIER_ENDING_SHARE) {
                let sec = self.random.between(CARRIER_LIFE_SEC.0, CARRIER_LIFE_SEC.1);
                (sec * self.sample_rate).round().max(1.0) as u64
            } else {
                u64::MAX
            };
            (
                Kind::Carrier,
                level,
                Fade::new(&mut self.random, rate, sd / fade_rms(), 0.0),
                life,
            )
        };
        let reach = if birdie { BIRDIE_WANDER_HZ } else { 0.0 };
        let cap = self
            .tuning
            .cap_db(pitch - reach, pitch + reach, GUARD_CARRIER_DB);
        // Its loudest moment, not its median, is what has to stay under.
        let peak_over = fade.offset_db + fade.swing_db;
        let level_db = level_db.min(cap - peak_over);
        let mut carrier = Emitter {
            kind,
            sample_rate: self.sample_rate,
            pitch_hz: pitch,
            over_hz: pitch,
            peak: self.amplitude(level_db),
            fade,
            key_down: true,
            // On the air before the receiver was.
            edge: 1.0,
            edge_step: 1.0 / (CARRIER_EDGE_SEC * self.sample_rate).max(1.0),
            life_left: life,
            ..Emitter::default()
        };
        carrier.start(self.random.f64());
        carrier
    }

    /// A band switched on has been going for a while: stations already part
    /// way through their overs, others between them.
    fn populate(&mut self) {
        let count = self
            .random
            .poisson(self.station_rate * MEAN_LIFE_SEC, 4 * MAX_EMITTERS);
        for _ in 0..count {
            let mut station = self.new_station();
            let overs = usize::from(station.overs_left);
            station.overs_left = (self.random.index(overs) + 1) as u8;
            // About as long between overs as in them — and only part way
            // through one if the count leaves room for it.
            let room = !self.tuning.heard(station.pitch_hz)
                || self.keying_heard() < self.cap.saturating_sub(self.reserved);
            if room && self.random.chance(0.5) {
                station.go();
                let (first, last) = station.script;
                let phrase = first + self.random.index((last - first) as usize) as u32;
                station.enter_phrase(&self.traffic, phrase);
                station.over_target = (f64::from(station.over_target) * self.random.f64()) as u32;
            } else {
                station.ready = false;
                let gap = self.random.between(0.0, OVER_GAP_SEC.1);
                station.wait = (gap * self.sample_rate).round() as u32 + 1;
            }
            self.place(station);
        }
    }
}

/// What a transmitter is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
    /// A free slot.
    #[default]
    Idle,
    /// Someone sending Morse.
    Keyed,
    /// A local birdie: steady, faint, there all session.
    Birdie,
    /// A transmitted carrier: loud, fading on its path.
    Carrier,
    /// A fast sweep across the band.
    Chirp,
}

/// One transmitter on the band, running. Plain data, so the pool is a fixed
/// array.
#[derive(Clone, Copy, Debug, Default)]
struct Emitter {
    kind: Kind,
    draw: Draw,
    sample_rate: f64,
    /// Where the station is; each over lands within a couple of hertz of it.
    pitch_hz: f64,
    /// Where this over, or this sweep, started.
    over_hz: f64,
    hz: f64,
    drift_hz_per_sec: f64,
    sweep_hz_per_sec: f64,
    /// Amplitude at full strength.
    peak: f64,
    fade: Fade,
    /// The fade's gain, ramped sample by sample between control updates.
    gain: f64,
    gain_step: f64,
    /// Seconds on the air, and into the current over.
    age: f64,
    over_age: f64,
    /// The oscillator, as a rotating phasor: one complex multiply a sample.
    re: f64,
    im: f64,
    rot_re: f64,
    rot_im: f64,
    /// The keying envelope: where along its raised-cosine edge it is.
    key_down: bool,
    edge: f64,
    edge_step: f64,
    /// Samples per dit, and the rest of the fist.
    unit: f64,
    dah: f64,
    jitter: f64,
    /// The phrases this station sends, and where it is in them.
    script: (u32, u32),
    phrase: u32,
    pos: u32,
    end: u32,
    element_left: u32,
    over_elapsed: u32,
    over_target: u32,
    overs_left: u8,
    /// Samples of silence before the next over.
    wait: u32,
    /// Its turn has come and it is waiting for a clear moment, and for how
    /// many samples so far.
    ready: bool,
    waited: u32,
    /// Making room for a pile-up: ending the over at the next character.
    breaking_off: bool,
    /// Samples before a carrier or a sweep goes off; `u64::MAX` is never.
    life_left: u64,
}

impl Emitter {
    /// Start the oscillator at `phase` (a fraction of a turn) and the fade at
    /// its own starting point.
    fn start(&mut self, phase: f64) {
        let (sin, cos) = (TAU * phase).sin_cos();
        self.re = cos;
        self.im = sin;
        self.gain = self.fade.gain(0.0);
        self.tune(if self.over_hz > 0.0 {
            self.over_hz
        } else {
            self.pitch_hz
        });
    }

    fn tune(&mut self, hz: f64) {
        self.hz = hz;
        let (sin, cos) = (TAU * hz / self.sample_rate.max(1.0)).sin_cos();
        self.rot_re = cos;
        self.rot_im = sin;
    }

    fn step(&mut self, traffic: &Traffic) -> f64 {
        match self.kind {
            Kind::Idle => return 0.0,
            Kind::Keyed => self.key(traffic),
            Kind::Birdie | Kind::Carrier | Kind::Chirp => self.hold(),
        }
        if self.key_down {
            if self.edge < 1.0 {
                self.edge = (self.edge + self.edge_step).min(1.0);
            }
        } else if self.edge > 0.0 {
            self.edge = (self.edge - self.edge_step).max(0.0);
        }
        self.gain += self.gain_step;
        if self.edge <= 0.0 {
            return 0.0;
        }
        let envelope = if self.edge >= 1.0 {
            1.0
        } else {
            0.5 - 0.5 * (PI * self.edge).cos()
        };
        let re = self.re * self.rot_re - self.im * self.rot_im;
        let im = self.re * self.rot_im + self.im * self.rot_re;
        self.re = re;
        self.im = im;
        self.peak * self.gain * envelope * im
    }

    /// A carrier or a sweep: on until its life runs out.
    fn hold(&mut self) {
        if self.life_left == u64::MAX {
            return;
        }
        if self.life_left > 0 {
            self.life_left -= 1;
        } else {
            self.key_down = false;
        }
    }

    /// One sample of Morse: through the gap before an over, or on through the
    /// current element.
    fn key(&mut self, traffic: &Traffic) {
        if self.overs_left == 0 {
            return;
        }
        if self.wait > 0 {
            self.wait -= 1;
            if self.wait == 0 {
                self.ready = true;
            }
            return;
        }
        if self.ready {
            return;
        }
        self.over_elapsed = self.over_elapsed.saturating_add(1);
        if self.element_left == 0 {
            self.next_element(traffic);
        }
        self.element_left = self.element_left.saturating_sub(1);
    }

    /// Keying an over right now, gaps between its characters included.
    fn on_air(&self) -> bool {
        self.kind == Kind::Keyed && self.overs_left > 0 && self.wait == 0 && !self.ready
    }

    /// Its over is ending: it has run its length, or been told to break off.
    fn winding_up(&self) -> bool {
        self.breaking_off || self.over_elapsed >= self.over_target
    }

    /// Break the over off at the end of the character it is in.
    fn wind_up(&mut self) {
        self.breaking_off = true;
    }

    /// Its turn, and room for it: start the over.
    fn go(&mut self) {
        self.ready = false;
        self.waited = 0;
        self.breaking_off = false;
        self.begin_over();
    }

    fn begin_over(&mut self) {
        let (median, sigma) = OVER_TARGET_SEC;
        let target = self
            .draw
            .log_normal(median, sigma)
            .clamp(OVER_TARGET_LIMITS_SEC.0, OVER_TARGET_LIMITS_SEC.1);
        self.over_target = (target * self.sample_rate).round() as u32;
        self.over_elapsed = 0;
        self.over_age = 0.0;
        self.element_left = 0;
        self.over_hz = self.pitch_hz
            + self
                .draw
                .between(-OVER_PITCH_JITTER_HZ, OVER_PITCH_JITTER_HZ);
        self.tune(self.over_hz);
    }

    /// Hand over to the other side of the contact, and wait for it. The next
    /// over picks the text up where this one left it.
    fn end_over(&mut self) {
        self.key_down = false;
        self.overs_left = self.overs_left.saturating_sub(1);
        if self.overs_left > 0 {
            let gap = self.draw.between(OVER_GAP_SEC.0, OVER_GAP_SEC.1);
            self.wait = (gap * self.sample_rate).round().max(1.0) as u32;
        }
    }

    fn next_phrase(&self) -> u32 {
        let (first, last) = self.script;
        if self.phrase + 1 >= last {
            first
        } else {
            self.phrase + 1
        }
    }

    fn enter_phrase(&mut self, traffic: &Traffic, phrase: u32) {
        let (start, end) = traffic.phrase(phrase);
        self.phrase = phrase;
        self.pos = start;
        self.end = end;
    }

    /// The next mark or space. Once the over has run its length it ends at
    /// the next space between words; until then the text runs on, phrase
    /// after phrase.
    fn next_element(&mut self, traffic: &Traffic) {
        let long_enough = self.over_elapsed >= self.over_target;
        if self.pos >= self.end {
            self.enter_phrase(traffic, self.next_phrase());
            if long_enough || self.breaking_off {
                self.end_over();
            } else {
                self.key_down = false;
                self.element_left = self.samples(6.0);
            }
            return;
        }
        let code = traffic.code(self.pos);
        self.pos += 1;
        let between_characters = code == CHAR_GAP || code == WORD_GAP;
        if (code == WORD_GAP && long_enough) || (between_characters && self.breaking_off) {
            self.end_over();
            return;
        }
        let (down, units) = match code {
            DIT => (true, 1.0),
            DAH => (true, self.dah),
            GAP => (false, 1.0),
            CHAR_GAP => (false, 2.0),
            _ => (false, 6.0),
        };
        self.key_down = down;
        self.element_left = self.samples(units);
    }

    /// How many samples `units` dits last, as this fist sends them.
    fn samples(&mut self, units: f64) -> u32 {
        let spread = (1.0 + self.jitter * self.draw.normal()).clamp(0.7, 1.3);
        (units * self.unit * spread)
            .round()
            .clamp(1.0, f64::from(u32::MAX)) as u32
    }

    /// Whether it is done and can give its slot up.
    fn finished(&self) -> bool {
        let silent = !self.key_down && self.edge <= 0.0;
        match self.kind {
            Kind::Idle => false,
            Kind::Keyed => self.overs_left == 0 && silent,
            Kind::Birdie | Kind::Carrier | Kind::Chirp => silent,
        }
    }

    /// The slow work: retune for drift or sweep, set the fade's ramp for the
    /// next block, and keep the oscillator on the unit circle.
    fn control(&mut self, dt: f64, block: u32) {
        if self.kind == Kind::Idle {
            return;
        }
        if self.finished() {
            self.kind = Kind::Idle;
            return;
        }
        self.age += dt;
        self.over_age += dt;
        let hz = match self.kind {
            Kind::Keyed if self.drift_hz_per_sec != 0.0 => {
                self.over_hz
                    + (self.drift_hz_per_sec * self.over_age).clamp(-DRIFT_MAX_HZ, DRIFT_MAX_HZ)
            }
            Kind::Birdie => {
                self.pitch_hz + BIRDIE_WANDER_HZ * (TAU * self.fade.rate_hz * self.age).sin()
            }
            Kind::Chirp => self.over_hz + self.sweep_hz_per_sec * self.age,
            _ => self.hz,
        };
        if hz != self.hz {
            self.tune(hz);
        }
        let target = self.fade.gain(self.age);
        self.gain_step = (target - self.gain) / f64::from(block.max(1));
        // A rotating phasor creeps off the unit circle by rounding; one
        // Newton step a block pulls it back.
        let scale = 1.5 - 0.5 * (self.re * self.re + self.im * self.im);
        self.re *= scale;
        self.im *= scale;
    }
}

/// A slow wander in level, in decibels: `offset_db` plus `swing_db` times a
/// sum of sines that spans -1..=1.
#[derive(Clone, Copy, Debug, Default)]
struct Fade {
    rate_hz: f64,
    phases: [f64; 3],
    swing_db: f64,
    offset_db: f64,
}

impl Fade {
    fn new(random: &mut Draw, rate_hz: f64, swing_db: f64, offset_db: f64) -> Self {
        Self {
            rate_hz,
            phases: [random.f64() * TAU, random.f64() * TAU, random.f64() * TAU],
            swing_db,
            offset_db,
        }
    }

    fn steady() -> Self {
        Self::default()
    }

    fn db(&self, t: f64) -> f64 {
        let swing: f64 = MULTIPATH_RATES
            .iter()
            .zip(FADE_WEIGHTS)
            .zip(&self.phases)
            .map(|((multiple, weight), phase)| {
                weight * (TAU * self.rate_hz * multiple * t + phase).sin()
            })
            .sum();
        self.offset_db + self.swing_db * swing
    }

    fn gain(&self, t: f64) -> f64 {
        db_to_gain(self.db(t))
    }
}

// ------------------------------------------------------------------ traffic

/// Element codes in the encoded text.
const DIT: u8 = 0;
const DAH: u8 = 1;
/// The one-dit space after every element.
const GAP: u8 = 2;
/// Two more dits after a character's last element: three between characters.
const CHAR_GAP: u8 = 3;
/// Six more: seven between words.
const WORD_GAP: u8 = 4;

/// Everything the band's stations will ever send, encoded to Morse elements
/// once, when the band is built: phrases ("CQ TEST DL1ABC DL1ABC", "5NN 14",
/// "TU"), grouped into scripts that each belong to one station.
#[derive(Clone, Debug, Default)]
struct Traffic {
    codes: Vec<u8>,
    phrases: Vec<(u32, u32)>,
    scripts: Vec<(u32, u32)>,
}

const QTHS: &[&str] = &[
    "BERLIN", "OHIO", "PARIS", "OSLO", "TEXAS", "KRAKOW", "TOKYO", "MAINE", "LYON", "RIGA", "BRNO",
    "IDAHO", "KIEV", "BARI",
];
const REPORTS: &[&str] = &["599", "579", "559", "449", "569", "589"];
const GREETINGS: &[&str] = &["GM", "GA", "GE"];

impl Traffic {
    fn write(contest_share: f64, random: &mut Draw) -> Self {
        let mut traffic = Self::default();
        for _ in 0..SCRIPTS {
            let own = generate_callsign(CALL_TIER, random);
            let phrases = if random.chance(contest_share) {
                if random.chance(0.6) {
                    contest_runner(&own, random)
                } else {
                    contest_caller(&own, random)
                }
            } else if random.chance(0.4) {
                calling_cq(&own, random)
            } else {
                ragchew(&own, random)
            };
            traffic.add_script(&phrases);
        }
        traffic
    }

    fn add_script(&mut self, phrases: &[String]) {
        let first = self.phrases.len() as u32;
        for phrase in phrases {
            let start = self.codes.len() as u32;
            self.encode(phrase);
            let end = self.codes.len() as u32;
            if end > start {
                self.phrases.push((start, end));
            }
        }
        let last = self.phrases.len() as u32;
        if last > first {
            self.scripts.push((first, last));
        }
    }

    fn encode(&mut self, phrase: &str) {
        let mut first_word = true;
        for word in phrase.split_whitespace() {
            let mut first_char = true;
            for ch in word.chars() {
                let Some(pattern) = morse_for(ch) else {
                    continue;
                };
                if !first_char {
                    self.codes.push(CHAR_GAP);
                } else if !first_word {
                    self.codes.push(WORD_GAP);
                }
                first_char = false;
                first_word = false;
                for mark in pattern.bytes() {
                    self.codes.push(if mark == b'-' { DAH } else { DIT });
                    self.codes.push(GAP);
                }
            }
        }
    }

    fn scripts(&self) -> usize {
        self.scripts.len()
    }

    fn script(&self, index: usize) -> (u32, u32) {
        self.scripts.get(index).copied().unwrap_or((0, 0))
    }

    fn phrase(&self, index: u32) -> (u32, u32) {
        self.phrases.get(index as usize).copied().unwrap_or((0, 0))
    }

    fn code(&self, pos: u32) -> u8 {
        self.codes.get(pos as usize).copied().unwrap_or(WORD_GAP)
    }
}

fn pick<'a>(random: &mut Draw, from: &[&'a str]) -> &'a str {
    from.get(random.index(from.len())).copied().unwrap_or("")
}

/// A contest exchange: a zone or a serial number.
fn exchange(random: &mut Draw) -> String {
    if random.chance(0.5) {
        format!("{:02}", random.index(40) + 1)
    } else {
        format!("{}", random.index(400) + 1)
    }
}

fn calling_cq(own: &str, random: &mut Draw) -> Vec<String> {
    let ending = if random.chance(0.3) { "PSE K" } else { "K" };
    vec![
        format!("CQ CQ CQ DE {own} {own} {ending}"),
        format!("CQ CQ DE {own} {own} {own} {ending}"),
        format!("QRZ? DE {own} K"),
    ]
}

fn ragchew(own: &str, random: &mut Draw) -> Vec<String> {
    let other = generate_callsign(CALL_TIER, random);
    // The names contest callers go by, by the country of the call.
    let name = draw_name(country_of(own).unwrap_or_default(), random);
    let qth = pick(random, QTHS);
    let report = pick(random, REPORTS);
    let greeting = pick(random, GREETINGS);
    vec![
        format!("{other} DE {own}"),
        format!("{greeting} TNX FER CALL"),
        format!("UR RST {report} {report}"),
        format!("NAME {name} {name}"),
        format!("QTH {qth} {qth}"),
        format!("HW? {other} DE {own} KN"),
        format!("{other} DE {own} R R"),
        "FB TNX FER QSO 73".to_string(),
        format!("{other} DE {own} TU E E"),
    ]
}

fn contest_runner(own: &str, random: &mut Draw) -> Vec<String> {
    let report = if random.chance(0.8) { "5NN" } else { "599" };
    let mut phrases = vec![format!("CQ TEST {own} {own}")];
    for _ in 0..3 {
        let caller = generate_callsign(CALL_TIER, random);
        phrases.push(format!("{caller} {report} {}", exchange(random)));
        phrases.push(if random.chance(0.5) {
            format!("TU {own}")
        } else {
            format!("TU {own} TEST")
        });
    }
    phrases.push(format!("CQ {own} TEST"));
    phrases
}

fn contest_caller(own: &str, random: &mut Draw) -> Vec<String> {
    let exchange = exchange(random);
    vec![
        own.to_string(),
        format!("{own} {own}"),
        format!("TU 5NN {exchange}"),
        format!("R 5NN {exchange} TU"),
    ]
}

#[cfg(test)]
mod tests;
