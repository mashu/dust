//! Where the receiver's AGC starts to act, set from the band's own floor.

use std::sync::OnceLock;

use super::agc::AGC_TRIGGER;
use super::calibration::{NOISE_LEVEL_PER_DB, heard_floor_rms, noise_level, qrn_level};
use super::dsp::gain_to_db;
use super::filter::{FilterDesign, ReceiverFilter, reference_width_hz};
use super::memo::{Exact, LastAnswer};
use super::source::BandSource;
use crate::settings::{
    AGC_THRESHOLD_DEFAULT_DB, AGC_THRESHOLD_MAX_DB, AGC_THRESHOLD_MIN_DB, BandSettings,
    FilterShape, STATIONS_MIN, TrainingSettings,
};

/// The rate the floor is measured at for the AGC's knee. Both backends use
/// it, so both arrive at the same knee to the bit; it is low because the
/// browser measures on its main thread every time a band setting moves.
pub const FLOOR_MEASURE_RATE: u32 = 16_000;

/// Where the receiver's AGC starts to turn the gain down with its threshold
/// at 0 dB, as the amplitude of a steady tone, for the band `band` describes
/// heard through `receiver`: the band's own knee, which the threshold moves
/// the AGC's from ([`receiver_knee`]). `None` with the AGC switched off.
///
/// `AGC_TRIGGER` times the band's own floor (`band_standing_level`), and
/// never below that of the quietest floor the noise control sets, heard
/// through the reference filter (`reference_width_hz`): 10 dB or more over
/// the hiss in the filter, more with heavy static, which it is kept clear of
/// too ([`threshold_over_hiss_db`]), so every station clear of the noise
/// stands over it and the AGC evens them out, and those down in it keep
/// their own level.
/// Above that it follows the floor, the way an operator backs the RF gain
/// off on a noisy band or a wide filter, so the hiss on its own never pumps
/// the gain. Below it, it stays put, as a real receiver's AGC threshold
/// does: closing the filter or switching the noise off takes noise away
/// from under the knee, and leaves the station you are copying as loud as
/// it was.
///
/// Remembered, because measuring it is three seconds of band: the floor
/// depends only on the noise, the static and the receiver, so moving the
/// fading, the stations or the volume asks the same question again. That is
/// also what lets it be measured ahead, before a band is built: asked of the
/// same settings, the band finds the answer waiting.
pub fn agc_knee(band: &BandSettings, receiver: &FilterDesign) -> Option<f64> {
    if !band.agc_enabled {
        return None;
    }
    Some(KNEE.get_or(KneeAsk::of(band, receiver), || {
        let floor =
            band_standing_level(FLOOR_MEASURE_RATE, band, receiver).max(quietest_floor_level());
        AGC_TRIGGER * floor
    }))
}

/// The band's own hiss as it comes out of the receiver's filter, before the
/// RF gain, as an RMS; nothing with the noise off. What the AGC's threshold
/// is read against ([`threshold_over_hiss_db`]).
pub fn band_hiss_rms(band: &BandSettings, receiver: &FilterDesign) -> f64 {
    heard_floor_rms(noise_level(band), receiver.noise_bandwidth_hz())
}

/// How far a steady tone at the AGC's threshold, of amplitude `knee`, stands
/// over hiss of RMS `hiss`, both as the AGC hears them: RMS to RMS, as the
/// band's S/N is read ([`super::heard_snr_db`]), so the AGC turns a station
/// down while its S/N stands over this. None with no hiss to read it
/// against.
pub fn threshold_over_hiss_db(knee: f64, hiss: f64) -> Option<f64> {
    let over = gain_to_db(knee / std::f64::consts::SQRT_2 / hiss);
    (knee > 0.0 && hiss > 0.0 && over.is_finite()).then_some(over)
}

/// The knee [`agc_knee`] last measured, if it was for this band heard
/// through `receiver`, AGC switched on or not: never measured here. Whether
/// something has spent three seconds of band on these settings, as the
/// settings screen must not on the thread that draws while a slider moves.
pub fn remembered_agc_knee(band: &BandSettings, receiver: &FilterDesign) -> Option<f64> {
    KNEE.remembered(&KneeAsk::of(band, receiver))
}

/// The last knee measured, and what it was measured for.
static KNEE: LastAnswer<KneeAsk, f64> = LastAnswer::new();

/// Where the receiver's AGC starts to turn the gain down for these band
/// settings, heard through `receiver`, at the threshold they set: the band's
/// own knee ([`agc_knee`]), moved by the threshold ([`threshold_factor`]),
/// down into the hiss or up over the weaker stations. The volume after it
/// makes up what the AGC takes off. `None` with the AGC switched off.
///
/// What both players set their AGC by.
pub fn receiver_knee(band: &BandSettings, receiver: &FilterDesign) -> Option<f64> {
    agc_knee(band, receiver).map(|knee| knee * threshold_factor(band))
}

/// How many times the band's own knee these settings put the AGC's
/// threshold at: one at 0 dB, under one taken down into the hiss, inside
/// its range whatever is set, and at 0 dB for a threshold that is not a
/// number, as the settings put it back.
pub fn threshold_factor(band: &BandSettings) -> f64 {
    let db = if band.agc_threshold_db.is_finite() {
        band.agc_threshold_db
            .clamp(AGC_THRESHOLD_MIN_DB, AGC_THRESHOLD_MAX_DB)
    } else {
        AGC_THRESHOLD_DEFAULT_DB
    };
    10f64.powf(db / 20.0)
}

/// What the knee is measured from, and so what it is remembered by: the
/// noise and the static as they are heard, and the receiver.
#[derive(Clone, Debug, PartialEq)]
struct KneeAsk {
    noise: Exact,
    qrn: Exact,
    receiver: FilterDesign,
}

impl KneeAsk {
    fn of(band: &BandSettings, receiver: &FilterDesign) -> Self {
        Self {
            noise: Exact(noise_level(band)),
            qrn: Exact(qrn_level(band)),
            receiver: receiver.clone(),
        }
    }
}

/// How high this band's floor ordinarily peaks, through the receiver.
///
/// The level a 60 ms peak follower on the filtered band sits at, as a median
/// over three seconds so a crash that happens to land in the measurement does
/// not move it: about 2.2 times the floor's RMS for plain hiss, more on a band
/// busy with crackle. Other stations are left out — they are what the AGC
/// rides over, not the floor it is set against.
///
/// Measured rather than worked out, because the filter's shape and width and
/// the static all move it. The band is calibrated to sound the same at any
/// sample rate, so a low rate measures it as well as a high one.
pub fn band_standing_level(sample_rate: u32, band: &BandSettings, receiver: &FilterDesign) -> f64 {
    const SEED: u64 = 0x5EED_1234;
    const SETTLE_SEC: f64 = 0.25;
    const MEASURE_SEC: f64 = 2.75;
    const FOLLOWER_SEC: f64 = 0.06;
    const BLOCK_SEC: f64 = 0.01;
    let sample_rate = sample_rate.max(1);
    let sr = f64::from(sample_rate);
    // The floor is the band without its stations: they are what the knee is
    // there to answer, so they cannot be part of setting it.
    let floor = BandSettings {
        stations_min: STATIONS_MIN,
        stations_max: STATIONS_MIN,
        ..band.clone()
    };
    let mut source = BandSource::new(sample_rate, &floor, receiver, SEED);
    let mut filter = ReceiverFilter::new(sample_rate, receiver);
    let fall = (-1.0 / (FOLLOWER_SEC * sr)).exp();
    let mut follower = 0.0f64;
    let mut step = || {
        let heard = filter.process(source.next_sample()).abs();
        follower = heard.max(follower * fall);
        follower
    };
    for _ in 0..(SETTLE_SEC * sr) as usize {
        step();
    }
    let block = ((BLOCK_SEC * sr) as usize).max(1);
    let mut levels: Vec<f64> = (0..((MEASURE_SEC / BLOCK_SEC) as usize))
        .map(|_| {
            let mut last = 0.0;
            for _ in 0..block {
                last = step();
            }
            last
        })
        .collect();
    levels.sort_by(f64::total_cmp);
    levels.get(levels.len() / 2).copied().unwrap_or(0.0)
}

/// [`band_standing_level`] of the quietest floor the noise control sets
/// ([`NOISE_LEVEL_PER_DB`], a station 39 dB over it) alone, through the
/// reference filter: a sharp one 500 Hz wide at 3 dB, on a 500 Hz note
/// whatever the default pitch. Measured once.
pub fn quietest_floor_level() -> f64 {
    static LEVEL: OnceLock<f64> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        let mut settings = TrainingSettings::default();
        settings.band.side_tone_min = 500.0;
        settings.band.side_tone_max = 500.0;
        settings.band.noise_enabled = true;
        settings.band.noise_level = NOISE_LEVEL_PER_DB;
        settings.band.qrn_enabled = false;
        settings.band.stations_min = STATIONS_MIN;
        settings.band.stations_max = STATIONS_MIN;
        settings.band.filter_width_hz = reference_width_hz();
        settings.band.filter_shape = FilterShape::Sharp;
        let receiver = FilterDesign::from_settings(&settings);
        band_standing_level(FLOOR_MEASURE_RATE, &settings.band, &receiver)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FILTER_WIDTH_MAX, FILTER_WIDTH_MIN, SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ};

    const SR: u32 = 48_000;

    /// How far the band's own knee, the threshold at 0 dB, stands over the
    /// band's own hiss, through the default receiver.
    fn own_knee_over_hiss(edit: impl FnOnce(&mut TrainingSettings)) -> f64 {
        let mut settings = TrainingSettings::default();
        edit(&mut settings);
        let settings = settings.clamp();
        let receiver = FilterDesign::from_settings(&settings);
        let knee = agc_knee(&settings.band, &receiver).expect("the AGC is on");
        threshold_over_hiss_db(knee, band_hiss_rms(&settings.band, &receiver)).expect("hiss")
    }

    /// With the threshold at 0 dB the knee stands 10 dB or more over the
    /// band's own hiss in the filter, as the threshold's words say: a steady tone at it is
    /// 10-11 dB over the hiss through the default receiver at any floor loud
    /// enough for the knee to follow it, with the static off, and 12-13 dB on
    /// the default band, its light static or none. Heavy static it is kept
    /// clear of too, so it stands further over the hiss.
    #[test]
    fn the_bands_own_knee_stands_ten_db_or_more_over_the_hiss() {
        for level in [0.4, 0.6, 0.8] {
            let over = own_knee_over_hiss(|s| {
                s.band.qrn_enabled = false;
                s.band.noise_level = level;
            });
            assert!((10.0..11.0).contains(&over), "{level}: {over:.2} dB");
        }
        let quiet = own_knee_over_hiss(|s| s.band.qrn_enabled = false);
        let light = own_knee_over_hiss(|_| {});
        for over in [quiet, light] {
            assert!((12.0..13.0).contains(&over), "{over:.1} dB");
        }
        let stormy = own_knee_over_hiss(|s| s.band.qrn_level = 0.5);
        assert!(stormy > quiet + 5.0, "{stormy:.1} against {quiet:.1} dB");
    }

    /// The threshold over the hiss is read as a station's S/N is: a full
    /// station's own level at the threshold reads its S/N, a threshold
    /// raised 10 dB reads 10 more, and so does hiss 10 dB weaker, as backing
    /// the RF gain off gives; with no hiss there is nothing to read.
    #[test]
    fn the_threshold_over_the_hiss_is_read_as_a_stations_s_n() {
        use super::super::calibration::heard_snr_db;
        use crate::timing::DEFAULT_TARGET_GAIN;
        let settings = TrainingSettings::default().clamp();
        let receiver = FilterDesign::from_settings(&settings);
        let hiss = band_hiss_rms(&settings.band, &receiver);
        let full = threshold_over_hiss_db(DEFAULT_TARGET_GAIN, hiss).expect("hiss");
        let snr = heard_snr_db(noise_level(&settings.band), receiver.noise_bandwidth_hz());
        assert!((full - snr).abs() < 1e-9, "{full} against {snr}");
        let ten = 10f64.powf(0.5);
        let raised = threshold_over_hiss_db(DEFAULT_TARGET_GAIN * ten, hiss).expect("hiss");
        assert!((raised - full - 10.0).abs() < 1e-9);
        let backed_off = threshold_over_hiss_db(DEFAULT_TARGET_GAIN, hiss / ten).expect("hiss");
        assert!((backed_off - full - 10.0).abs() < 1e-9);
        let mut silent = settings.band;
        silent.noise_enabled = false;
        assert_eq!(band_hiss_rms(&silent, &receiver), 0.0);
        assert_eq!(threshold_over_hiss_db(DEFAULT_TARGET_GAIN, 0.0), None);
    }

    /// The threshold moves the knee the players set their AGC by as far as
    /// it is moved, up or down from the band's own knee at 0 dB; with the
    /// AGC off there is none, a threshold past either end reads as that end
    /// and one that is not a number as 0 dB.
    #[test]
    fn the_threshold_moves_the_knee_as_far_as_it_is_moved() {
        let mut settings = TrainingSettings::default();
        let receiver = FilterDesign::from_settings(&settings);
        let own = agc_knee(&settings.band, &receiver).expect("the AGC is on");
        assert_eq!(receiver_knee(&settings.band, &receiver), Some(own));
        let over = |settings: &TrainingSettings| {
            gain_to_db(receiver_knee(&settings.band, &receiver).expect("still on") / own)
        };
        for (set, moved) in [
            (10.0, 10.0),
            (-12.0, -12.0),
            (90.0, AGC_THRESHOLD_MAX_DB),
            (-40.0, AGC_THRESHOLD_MIN_DB),
            (f64::NAN, AGC_THRESHOLD_DEFAULT_DB),
        ] {
            settings.band.agc_threshold_db = set;
            assert!((over(&settings) - moved).abs() < 1e-9, "{set}");
        }
        settings.band.agc_enabled = false;
        assert_eq!(receiver_knee(&settings.band, &receiver), None);
    }

    /// The knee is one question whether the AGC is on or off, so a knee
    /// measured with it on is the one the level scope finds with it off;
    /// and looking finds nothing for a band whose floor no one measured.
    /// (The memo is the whole app's: other tests ask it meanwhile, so what
    /// it holds is looked at in its own tests.)
    #[test]
    fn the_knee_is_one_question_with_the_agc_on_or_off() {
        let mut settings = TrainingSettings::default();
        let receiver = FilterDesign::from_settings(&settings);
        let on = KneeAsk::of(&settings.band, &receiver);
        settings.band.agc_enabled = false;
        assert_eq!(KneeAsk::of(&settings.band, &receiver), on);
        settings.band.noise_level = 0.0123;
        assert_eq!(remembered_agc_knee(&settings.band, &receiver), None);
    }

    /// The band preview measures the knee ahead of building the band, for
    /// the settings as the app holds them — clamped once — and the band then
    /// asks for it with those settings clamped again (`BandMixer::new`). The
    /// two have to be one question, or what was measured ahead is never what
    /// is asked for: a second clamp must not move anything the knee is
    /// remembered by, whatever the settings came in as.
    #[test]
    fn clamping_twice_asks_the_knee_the_same_question() {
        let ask = |settings: &TrainingSettings| {
            KneeAsk::of(&settings.band, &FilterDesign::from_settings(settings))
        };
        let levels = [f64::NAN, f64::NEG_INFINITY, -1.0, 0.0, 0.4, 1.0, 9.0];
        let widths = [
            f64::NAN,
            -500.0,
            FILTER_WIDTH_MIN,
            500.0,
            FILTER_WIDTH_MAX,
            90_000.0,
        ];
        let tones = [
            (f64::NAN, f64::INFINITY),
            (9_000.0, 1.0),
            (SIDE_TONE_MIN_HZ, SIDE_TONE_MAX_HZ),
            (700.0, 700.0),
        ];
        let mut case = 0;
        for shape in [FilterShape::Sharp, FilterShape::Soft, FilterShape::Peaked] {
            for width in widths {
                for (low, high) in tones {
                    let mut s = TrainingSettings::default();
                    s.band.filter_shape = shape;
                    s.band.filter_width_hz = width;
                    s.band.side_tone_min = low;
                    s.band.side_tone_max = high;
                    // The levels and switches turn over at their own pace,
                    // mixed in with the filters rather than multiplying them.
                    s.band.noise_enabled = case % 3 != 0;
                    s.band.qrn_enabled = case % 2 == 0;
                    s.band.noise_level = levels[case % levels.len()];
                    s.band.qrn_level = levels[(case / 2) % levels.len()];
                    case += 1;
                    let once = s.clamp();
                    let twice = once.clone().clamp();
                    assert_eq!(
                        ask(&once),
                        ask(&twice),
                        "{shape:?} at {width} Hz over {low}-{high} Hz"
                    );
                }
            }
        }
    }

    fn standing(sample_rate: u32, settings: &TrainingSettings) -> f64 {
        let settings = settings.clone().clamp();
        band_standing_level(
            sample_rate,
            &settings.band,
            &FilterDesign::from_settings(&settings),
        )
    }

    /// The AGC's knee follows this number up on a noisy band, so it has to be
    /// the floor the receiver actually hears: it falls with the filter.
    #[test]
    fn the_measured_floor_falls_with_the_filter() {
        let mut s = TrainingSettings::default();
        s.band.qrn_level = 0.6;
        s.band.noise_level = 0.6;
        let floor_at = |bandwidth: f64| {
            let mut s = s.clone();
            s.band.filter_width_hz = bandwidth;
            standing(SR, &s)
        };
        let (wide, narrow) = (floor_at(1_000.0), floor_at(FILTER_WIDTH_MIN));
        assert!(wide > 0.0 && narrow > 0.0, "the band measured as silent");
        assert!(
            wide > narrow * 1.4,
            "the floor barely moved: {wide:.5} against {narrow:.5}"
        );
    }

    /// Both backends measure the floor at a low sample rate — the browser does
    /// it on its main thread — which is only sound because the floor does not
    /// depend on it.
    #[test]
    fn the_measured_floor_does_not_depend_on_the_sample_rate() {
        for shape in [FilterShape::Sharp, FilterShape::Soft] {
            let mut s = TrainingSettings::default();
            s.band.filter_shape = shape;
            s.band.noise_level = 0.6;
            let low = standing(FLOOR_MEASURE_RATE, &s);
            let high = standing(48_000, &s);
            let spread = 20.0 * (low / high).log10();
            assert!(
                spread.abs() < 1.5,
                "{shape:?}: 16 kHz and 48 kHz floors differ by {spread:.2} dB"
            );
        }
    }
}
