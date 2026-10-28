//! The receiver's level scope: what reaches its AGC over the last few
//! seconds, through the RF gain, against where the AGC starts to act, and
//! how far the AGC takes it down. What the RF gain and the AGC's threshold
//! are set by: a station's peaks above the line are ridden by the AGC,
//! under it they come through as they are.

use std::collections::VecDeque;

use cw_core::band::{AGC_MAX_DUCK_DB, FilterDesign, SOFT_LIMIT_KNEE, agc_knee, threshold_raise};
use cw_core::{BandSettings, TrainingSettings};
use dioxus::prelude::*;

use crate::audio::{METER_FRAME_SEC, MeterFrame};
use crate::state::AppCtx;
use crate::time::sleep_ms;

/// How much of the past the scope shows.
const SHOWN: usize = (3.0 / METER_FRAME_SEC) as usize;

/// How much of the past the readout under it is of: the last half second.
/// At 10 WPM and over a dah or two of the station is always in it; in a
/// long Farnsworth gap it reads the hiss, as a rig's S-meter falls back to
/// the noise between a slow sender's characters.
const READ_OVER: usize = (0.5 / METER_FRAME_SEC) as usize;

/// How often the scope takes the frames the player has metered.
const POLL_MS: u32 = 50;

/// The levels the scope spans, in decibels over the AGC's lowest
/// threshold: the strongest station stands about 28 over it and the
/// quietest hiss about 7 under it; with the RF gain backed off all the way
/// the hiss is still on it, 37 under, and with the threshold raised all the
/// way the line is too, 30 over. A decibel to a unit of the pane's height.
const TOP_DB: f64 = 32.0;
const BOTTOM_DB: f64 = -40.0;

/// The most the AGC ever takes off, which its lane spans.
const AGC_LANE_DB: f64 = AGC_MAX_DUCK_DB;

/// The scope's drawing, in its own units: the level pane, a gap, and the
/// AGC's lane under it.
const WIDTH: f64 = 300.0;
const LEVEL_HEIGHT: f64 = 72.0;
const LANE_TOP: f64 = 78.0;
const LANE_HEIGHT: f64 = 18.0;

/// The AGC knee of the band the preview plays, with the AGC on whether it
/// is or not (`use_band_follow`): as the band follow on the settings screen
/// last measured it ahead, off the thread that draws where there is
/// another, or, for settings the follow took unmeasured with the AGC on, as
/// the band built for them measured it. The scope draws its line by it, and
/// so never measures one, which is three seconds of band.
#[derive(Clone, Copy)]
pub(crate) struct BandKnee(pub Signal<Option<f64>>);

/// What the scope's line is set by: the band these settings describe with
/// its AGC on, whether it is or not, and their receiver. The question the
/// band follow measures ahead, the AGC being on or off one question.
pub(crate) fn knee_asked(settings: &TrainingSettings) -> (BandSettings, FilterDesign) {
    let on = BandSettings {
        agc_enabled: true,
        ..settings.band.clone()
    };
    (on, FilterDesign::from_settings(settings))
}

/// Where the AGC starts to act for these settings, as the amplitude of a
/// steady tone at the AGC, after the RF gain: what the scope's line is
/// drawn at. The RF gain moves what reaches the line, not the line. With
/// the AGC off, where it would act if it were switched on, so the line
/// still says where a station stands. Measured, should it not be
/// remembered: what tests hold the scope's line to.
#[cfg(test)]
pub(crate) fn threshold(settings: &TrainingSettings) -> f64 {
    let (on, receiver) = knee_asked(settings);
    agc_knee(&on, &receiver).unwrap_or(1.0) * threshold_raise(&settings.band)
}

fn db(ratio: f64) -> f64 {
    if ratio > 0.0 {
        20.0 * ratio.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// How far down the level pane `level_db` over the lowest threshold sits.
fn level_y(level_db: f64) -> f64 {
    let share = ((TOP_DB - level_db) / (TOP_DB - BOTTOM_DB)).clamp(0.0, 1.0);
    share * LEVEL_HEIGHT
}

/// The scope's drawing: the level at the AGC against its lowest threshold,
/// `knee`, the AGC's lane, and the dashed line where the AGC starts, as far
/// over the lowest as the threshold control raises it, `raise`. The RF gain
/// moves the level and the threshold the line, as a panadapter's AGC line
/// moves with its knob: on one axis the two would look alike.
#[derive(Debug, PartialEq)]
struct Drawing {
    level: String,
    agc: String,
    line: f64,
}

impl Drawing {
    fn of(frames: &VecDeque<MeterFrame>, knee: f64, raise: f64) -> Self {
        // The newest frame at the right-hand edge, the rest running back
        // from it.
        let start = SHOWN.saturating_sub(frames.len());
        let level = frames
            .iter()
            .map(|frame| level_y(db(f64::from(frame.input) / knee)));
        let agc = frames.iter().map(|frame| {
            let taken = (-db(f64::from(frame.gain))).clamp(0.0, AGC_LANE_DB);
            LANE_TOP + taken / AGC_LANE_DB * LANE_HEIGHT
        });
        Self {
            level: lane_path(start, LEVEL_HEIGHT, level),
            agc: lane_path(start, LANE_TOP, agc),
            line: level_y(db(raise)),
        }
    }
}

/// One lane as an SVG path in tenths of the drawing's units, drawn at
/// `scale(0.1)`, so it is written in whole numbers: along `base` to where
/// the frames start, a step a frame at the height each is given, a run of
/// equal steps one stretch, and back to `base` at the right-hand edge. Five
/// times a second, a point a frame formatted as two decimals was three
/// times the text, and a hung AGC lane, one height, is a few bytes.
fn lane_path(start: usize, base: f64, heights: impl Iterator<Item = f64>) -> String {
    use std::fmt::Write;
    let tenths = |units: f64| (units * 10.0).round() as i64;
    let frame = tenths(WIDTH / SHOWN as f64);
    let base = tenths(base);
    let mut path = String::with_capacity(32 + 12 * SHOWN);
    let _ = write!(path, "M0 {base}H{}", start as i64 * frame);
    let (mut at, mut run) = (None, 0);
    for y in heights.map(tenths) {
        if at != Some(y) {
            if run > 0 {
                let _ = write!(path, "h{run}");
            }
            let _ = write!(path, "V{y}");
            (at, run) = (Some(y), 0);
        }
        run += frame;
    }
    if run > 0 {
        let _ = write!(path, "h{run}");
    }
    let _ = write!(path, "V{base}Z");
    path
}

/// The scope's three readings over its last stretch: the loudest at the AGC
/// over its threshold, the most the AGC took off, and the loudest played
/// against the limiter's knee, under it or into it; all in decibels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub over_db: f64,
    pub agc_db: f64,
    pub headroom_db: f64,
}

impl Reading {
    fn of(frames: &VecDeque<MeterFrame>, threshold: f64) -> Option<Self> {
        let recent = frames.iter().rev().take(READ_OVER);
        let (input, gain, output) = recent.fold((0.0f32, 1.0f32, 0.0f32), |(i, g, o), f| {
            (i.max(f.input), g.min(f.gain), o.max(f.output))
        });
        (input > 0.0).then(|| Self {
            over_db: db(f64::from(input) / threshold),
            agc_db: -db(f64::from(gain)),
            headroom_db: -db(f64::from(output) / SOFT_LIMIT_KNEE),
        })
    }

    /// The readout, in words.
    fn says(&self, agc_on: bool) -> String {
        let over = if self.over_db >= 0.0 {
            format!("Peaks {:.0} dB over the AGC", self.over_db)
        } else {
            format!("Peaks {:.0} dB under the AGC", -self.over_db)
        };
        let agc = if !agc_on {
            "AGC off".to_string()
        } else if self.agc_db < 0.5 {
            "AGC idle".to_string()
        } else {
            format!("AGC takes {:.0} dB", self.agc_db)
        };
        // A peak that only grazes the knee is not said to be into it.
        let room = if !self.headroom_db.is_finite() {
            "silent".to_string()
        } else if self.headroom_db > -0.5 {
            format!("{:.0} dB under the limiter", self.headroom_db.max(0.0))
        } else {
            format!("{:.0} dB into the limiter", -self.headroom_db)
        };
        format!("{over} · {agc} · {room}")
    }
}

/// The level scope, for as long as it is shown: it takes the player's
/// meter frames every [`POLL_MS`] and draws the last few seconds of them.
/// Shown while the band preview plays, so there is a receiver to meter.
#[component]
pub fn LevelScope(settings: Signal<TrainingSettings>) -> Element {
    let ctx = try_use_context::<AppCtx>();
    let mut frames = use_signal(|| VecDeque::<MeterFrame>::with_capacity(SHOWN + 1));
    use_future(move || async move {
        // Drawn outside the app, as a card is in a test, there is no player
        // to look at.
        let Some(ctx) = ctx else {
            return;
        };
        let mut taken = Vec::new();
        loop {
            sleep_ms(POLL_MS).await;
            ctx.app().player.take_meter(&mut taken);
            if taken.is_empty() {
                continue;
            }
            frames.with_mut(|kept| {
                kept.extend(taken.drain(..));
                let extra = kept.len().saturating_sub(SHOWN);
                kept.drain(..extra);
            });
        }
    });
    // The level follows a move as the band does, once the follow has
    // measured its knee; the line follows the threshold, which only raises
    // it, at once.
    let band_knee = try_use_context::<BandKnee>();
    let (knee, raise, agc_on) = use_memo(move || {
        let settings = settings.read();
        let knee = match band_knee {
            Some(BandKnee(knee)) => knee(),
            // Drawn on its own, as a card is in a test, with no follow to
            // measure it.
            None => {
                let (on, receiver) = knee_asked(&settings);
                agc_knee(&on, &receiver)
            }
        };
        (
            knee,
            threshold_raise(&settings.band),
            settings.band.agc_enabled,
        )
    })();
    // Nothing is placed against a line not yet known.
    let frames = frames.read();
    let frames = match knee {
        Some(_) => &*frames,
        None => &VecDeque::new(),
    };
    let knee = knee.unwrap_or(1.0);
    let Drawing { level, agc, line } = Drawing::of(frames, knee, raise);
    let reading = Reading::of(frames, knee * raise);
    let says = reading.map_or_else(|| "Listening…".to_string(), |reading| reading.says(agc_on));
    let view_box = format!("0 0 {WIDTH} {}", LANE_TOP + LANE_HEIGHT);
    rsx! {
        div { id: "level-scope", class: "scope",
            svg {
                class: "scope-plot",
                view_box: "{view_box}",
                preserve_aspect_ratio: "none",
                role: "img",
                "aria-label": "The receiver's level against its AGC over the last three seconds: {says}",
                rect {
                    class: "scope-pane",
                    x: "0",
                    y: "0",
                    width: "{WIDTH}",
                    height: "{LEVEL_HEIGHT}",
                }
                path { class: "scope-level", transform: "scale(0.1)", d: "{level}" }
                line {
                    class: if agc_on { "scope-threshold" } else { "scope-threshold off" },
                    x1: "0",
                    x2: "{WIDTH}",
                    y1: "{line:.1}",
                    y2: "{line:.1}",
                }
                rect {
                    class: "scope-pane",
                    x: "0",
                    y: "{LANE_TOP}",
                    width: "{WIDTH}",
                    height: "{LANE_HEIGHT}",
                }
                path { class: "scope-agc", transform: "scale(0.1)", d: "{agc}" }
            }
            p { class: "muted scope-legend",
                "The band at the AGC over the last three seconds; dashed, where the AGC starts; underneath, how much it takes off, up to 40 dB."
            }
            p { id: "scope-reading", class: "muted scope-reading", "{says}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(of: &[(f32, f32, f32)]) -> VecDeque<MeterFrame> {
        of.iter()
            .map(|&(input, gain, output)| MeterFrame {
                input,
                gain,
                output,
            })
            .collect()
    }

    /// The readout is the last half second's loudest at the AGC against its
    /// line, the most the AGC took off, and the room left under the limiter.
    #[test]
    fn the_reading_is_the_last_half_seconds_loudest() {
        let threshold = 0.1;
        let mut kept = frames(&[(0.9, 0.2, 0.69); 100]);
        kept.extend(frames(&[(0.2, 0.5, 0.35); READ_OVER]));
        let reading = Reading::of(&kept, threshold).expect("something heard");
        assert!((reading.over_db - 6.02).abs() < 0.01, "{reading:?}");
        assert!((reading.agc_db - 6.02).abs() < 0.01, "{reading:?}");
        assert!((reading.headroom_db - 6.02).abs() < 0.01, "{reading:?}");
        assert_eq!(
            reading.says(true),
            "Peaks 6 dB over the AGC · AGC takes 6 dB · 6 dB under the limiter"
        );
        let quiet = Reading::of(&frames(&[(0.05, 1.0, 0.1)]), threshold).expect("heard");
        assert_eq!(
            quiet.says(false),
            "Peaks 6 dB under the AGC · AGC off · 17 dB under the limiter"
        );
        assert_eq!(Reading::of(&frames(&[(0.0, 1.0, 0.0)]), threshold), None);
        // Played past the knee, it says how far into the limiter.
        let hot = Reading::of(&frames(&[(0.2, 1.0, 1.4)]), threshold).expect("heard");
        assert!(
            hot.says(false).ends_with("6 dB into the limiter"),
            "{}",
            hot.says(false)
        );
        let grazing = Reading {
            over_db: 0.0,
            agc_db: 0.0,
            headroom_db: -0.3,
        };
        assert!(grazing.says(true).ends_with("0 dB under the limiter"));
    }

    /// The threshold moves the line and leaves the level where it was; the
    /// RF gain moves the level and leaves the line.
    #[test]
    fn the_threshold_moves_the_line_and_the_rf_gain_the_level() {
        let heard = frames(&[(0.1, 1.0, 0.1); 4]);
        let lowest = Drawing::of(&heard, 0.1, 1.0);
        let raised = Drawing::of(&heard, 0.1, 10f64.powf(0.5));
        assert_eq!(lowest.level, raised.level);
        let per_db = LEVEL_HEIGHT / (TOP_DB - BOTTOM_DB);
        assert!((lowest.line - raised.line - 10.0 * per_db).abs() < 1e-9);
        let backed_off = Drawing::of(&frames(&[(0.01, 1.0, 0.01); 4]), 0.1, 1.0);
        assert_eq!(backed_off.line, lowest.line);
        assert_ne!(backed_off.level, lowest.level);
        // Raised all the way, the line is still on the pane.
        let top = Drawing::of(
            &heard,
            0.1,
            10f64.powf(cw_core::AGC_THRESHOLD_MAX_DB / 20.0),
        );
        assert!(top.line > 0.0, "{}", top.line);
    }

    /// A lane is drawn in whole tenths, along its base to its first frame, a
    /// step a frame, a run of equal frames one stretch, and closed on its
    /// base at the right-hand edge.
    #[test]
    fn a_lane_is_drawn_in_whole_steps() {
        assert_eq!(
            lane_path(SHOWN - 3, LEVEL_HEIGHT, [10.0, 10.0, 20.56].into_iter()),
            format!("M0 720H{}V100h20V206h10V720Z", (SHOWN - 3) * 10)
        );
        let hung = lane_path(0, LANE_TOP, std::iter::repeat_n(LANE_TOP, SHOWN));
        assert_eq!(hung, format!("M0 780H0V780h{}V780Z", SHOWN * 10));
        assert_eq!(
            lane_path(SHOWN, LANE_TOP, std::iter::empty()),
            format!("M0 780H{}V780Z", SHOWN * 10)
        );
    }

    /// The line sits where the AGC starts, as far over its lowest as the
    /// threshold is raised, wherever the RF gain is; with the AGC off, where
    /// it would start were it switched on.
    #[test]
    fn the_line_is_the_agc_threshold() {
        let mut settings = TrainingSettings::default();
        let lowest = threshold(&settings);
        settings.band.agc_threshold_db = 10.0;
        let raised = threshold(&settings);
        assert!((db(raised / lowest) - 10.0).abs() < 1e-9);
        settings.band.rf_gain_db = -20.0;
        assert_eq!(threshold(&settings), raised);
        settings.band.agc_enabled = false;
        assert_eq!(threshold(&settings), raised);
    }
}
