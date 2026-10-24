//! Where the receiver's AGC starts to act, set from the band's own floor.

use std::sync::OnceLock;

use super::agc::AGC_TRIGGER;
use super::calibration::{REFERENCE_NOISE_LEVEL, noise_level, qrn_level};
use super::filter::{FilterDesign, ReceiverFilter, reference_width_hz};
use super::memo::{Exact, LastAnswer};
use super::source::BandSource;
use crate::settings::{
    BandSettings, FilterShape, RF_GAIN_MAX_DB, RF_GAIN_MIN_DB, STATIONS_MIN, TrainingSettings,
};

/// The rate the floor is measured at for the AGC's knee. Both backends use
/// it, so both arrive at the same knee to the bit; it is low because the
/// browser measures on its main thread every time a band setting moves.
pub const FLOOR_MEASURE_RATE: u32 = 16_000;

/// Where the receiver's AGC starts to turn the gain down, as the amplitude of
/// a steady tone, for the band `band` describes heard through `receiver`;
/// `None` with the AGC switched off.
///
/// `AGC_TRIGGER` times the band's own floor (`band_standing_level`), and
/// never below that of the reference floor heard through the reference
/// filter (`reference_width_hz`). Above that it follows the floor, the way
/// an operator backs the RF gain off on a noisy band or a wide filter, so the
/// hiss on its own never pumps the gain. Below it, it stays put, as a real
/// receiver's AGC threshold does: closing the filter or finding a quiet band
/// takes noise away from under the knee, and leaves the station you are
/// copying exactly as loud as it was.
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
    static LAST: LastAnswer<KneeAsk, f64> = LastAnswer::new();
    Some(LAST.get_or(KneeAsk::of(band, receiver), || {
        let floor =
            band_standing_level(FLOOR_MEASURE_RATE, band, receiver).max(reference_floor_level());
        AGC_TRIGGER * floor
    }))
}

/// Where the receiver's AGC starts to turn the gain down for these band
/// settings, heard through `receiver`, at the RF gain they set: the band's
/// own knee ([`agc_knee`]), raised by however far the RF gain is backed off.
/// Turning the RF gain down lowers everything reaching the AGC alike, which
/// to the AGC is its threshold rising by as much; the volume after it makes
/// up the rest. `None` with the AGC switched off.
///
/// What both players set their AGC by.
pub fn receiver_knee(band: &BandSettings, receiver: &FilterDesign) -> Option<f64> {
    agc_knee(band, receiver).map(|knee| knee / rf_gain(band))
}

/// The RF gain as a gain: one at full, and less as it is backed off, inside
/// its range whatever is set.
pub fn rf_gain(band: &BandSettings) -> f64 {
    let db = if band.rf_gain_db.is_finite() {
        band.rf_gain_db.clamp(RF_GAIN_MIN_DB, RF_GAIN_MAX_DB)
    } else {
        RF_GAIN_MAX_DB
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

/// [`band_standing_level`] of the reference floor alone
/// ([`REFERENCE_NOISE_LEVEL`]), through the reference filter: a sharp one
/// 500 Hz wide at 3 dB, on a 500 Hz note whatever the default pitch. Measured
/// once.
pub fn reference_floor_level() -> f64 {
    static LEVEL: OnceLock<f64> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        let mut settings = TrainingSettings::default();
        settings.band.side_tone_min = 500.0;
        settings.band.side_tone_max = 500.0;
        settings.band.noise_enabled = true;
        settings.band.noise_level = REFERENCE_NOISE_LEVEL;
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

    /// Backing the RF gain off raises the knee the players set their AGC by
    /// as far as it backs off, from the band's own knee at full; with the
    /// AGC off there is none, and a gain no setting allows reads as full.
    #[test]
    fn the_rf_gain_raises_the_knee_as_far_as_it_backs_off() {
        let mut settings = TrainingSettings::default();
        let receiver = FilterDesign::from_settings(&settings);
        let full = agc_knee(&settings.band, &receiver).expect("the AGC is on");
        assert_eq!(receiver_knee(&settings.band, &receiver), Some(full));
        settings.band.rf_gain_db = -10.0;
        let backed_off = receiver_knee(&settings.band, &receiver).expect("still on");
        assert!((20.0 * (backed_off / full).log10() - 10.0).abs() < 1e-9);
        for wild in [f64::NAN, 40.0] {
            settings.band.rf_gain_db = wild;
            assert_eq!(
                receiver_knee(&settings.band, &receiver),
                Some(full),
                "{wild}"
            );
        }
        settings.band.rf_gain_db = -90.0;
        let floor = receiver_knee(&settings.band, &receiver).expect("still on");
        assert!((20.0 * (floor / full).log10() + RF_GAIN_MIN_DB).abs() < 1e-9);
        settings.band.agc_enabled = false;
        assert_eq!(receiver_knee(&settings.band, &receiver), None);
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
