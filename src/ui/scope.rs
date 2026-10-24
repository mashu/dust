//! The receiver's level scope: what reaches its AGC over the last few
//! seconds, against where the AGC starts to act, and how far the AGC takes
//! it down. What the RF gain is set by: a station's peaks above the line are
//! ridden by the AGC, under it they come through as they are.

use std::collections::VecDeque;

use cw_core::band::{FilterDesign, SOFT_LIMIT_KNEE, agc_knee, rf_gain};
use cw_core::{BandSettings, TrainingSettings};
use dioxus::prelude::*;

use crate::audio::{METER_FRAME_SEC, MeterFrame};
use crate::state::AppCtx;
use crate::time::sleep_ms;

/// How much of the past the scope shows.
const SHOWN: usize = (3.0 / METER_FRAME_SEC) as usize;

/// How much of the past the readout under it is of: the last half second,
/// so a dah or two of the station is always in it.
const READ_OVER: usize = (0.5 / METER_FRAME_SEC) as usize;

/// How often the scope takes the frames the player has metered.
const POLL_MS: u32 = 50;

/// The levels the scope spans, in decibels over the AGC's threshold: the
/// strongest station at full RF gain stands about ten over it, and the
/// default band's hiss well under the bottom.
const TOP_DB: f64 = 18.0;
const BOTTOM_DB: f64 = -36.0;

/// The most the AGC ever takes off, which its lane spans.
const AGC_LANE_DB: f64 = 12.0;

/// The scope's drawing, in its own units: the level pane, a gap, and the
/// AGC's lane under it.
const WIDTH: f64 = 300.0;
const LEVEL_HEIGHT: f64 = 72.0;
const LANE_TOP: f64 = 78.0;
const LANE_HEIGHT: f64 = 18.0;

/// Where the AGC starts to act for these settings, as the amplitude of a
/// steady tone at the AGC, the RF gain backed off from it: what the scope's
/// line is drawn at. With the AGC off, where it would act if it were on,
/// so the line still says where a station stands.
pub(crate) fn threshold(settings: &TrainingSettings) -> f64 {
    let receiver = FilterDesign::from_settings(settings);
    let on = BandSettings {
        agc_enabled: true,
        ..settings.band.clone()
    };
    let knee = agc_knee(&on, &receiver).unwrap_or(1.0);
    let rf = if settings.band.agc_enabled {
        rf_gain(&settings.band)
    } else {
        1.0
    };
    knee / rf
}

fn db(ratio: f64) -> f64 {
    if ratio > 0.0 {
        20.0 * ratio.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// How far down the level pane `level_db` over the threshold sits.
fn level_y(level_db: f64) -> f64 {
    let share = ((TOP_DB - level_db) / (TOP_DB - BOTTOM_DB)).clamp(0.0, 1.0);
    share * LEVEL_HEIGHT
}

/// The scope's three readings over its last stretch: the loudest at the AGC
/// over its threshold, the most the AGC took off, and the loudest played,
/// under the limiter's knee; all in decibels.
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
        let room = if self.headroom_db.is_finite() {
            format!("{:.0} dB under the limiter", self.headroom_db.max(0.0))
        } else {
            "silent".to_string()
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
    let (threshold, agc_on) = use_memo(move || {
        let settings = settings.read();
        (threshold(&settings), settings.band.agc_enabled)
    })();
    let frames = frames.read();
    let step = WIDTH / SHOWN as f64;
    // The newest frame at the right-hand edge, the rest running back from it.
    let start = SHOWN.saturating_sub(frames.len());
    let x = |i: usize| (start + i) as f64 * step;
    let mut level = format!("0,{LEVEL_HEIGHT:.1}");
    let mut agc = format!("0,{LANE_TOP:.1}");
    for (i, frame) in frames.iter().enumerate() {
        let y = level_y(db(f64::from(frame.input) / threshold));
        let taken = (-db(f64::from(frame.gain))).clamp(0.0, AGC_LANE_DB);
        let lane = LANE_TOP + taken / AGC_LANE_DB * LANE_HEIGHT;
        level += &format!(" {:.1},{y:.1} {:.1},{y:.1}", x(i), x(i) + step);
        agc += &format!(" {:.1},{lane:.1} {:.1},{lane:.1}", x(i), x(i) + step);
    }
    level += &format!(" {WIDTH:.1},{LEVEL_HEIGHT:.1}");
    agc += &format!(" {WIDTH:.1},{LANE_TOP:.1}");
    let line = level_y(0.0);
    let reading = Reading::of(&frames, threshold);
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
                polygon { class: "scope-level", points: "{level}" }
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
                polygon { class: "scope-agc", points: "{agc}" }
            }
            p { class: "muted scope-legend",
                "The band at the AGC over the last three seconds; dashed, where the AGC starts; underneath, how much it takes off, up to 12 dB."
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
    }

    /// The line sits where the AGC starts, the RF gain backing it off; with
    /// the AGC off, where it would start, whatever the RF gain says.
    #[test]
    fn the_line_is_the_agc_threshold_backed_off_by_the_rf_gain() {
        let mut settings = TrainingSettings::default();
        let full = threshold(&settings);
        settings.band.rf_gain_db = -10.0;
        assert!((db(threshold(&settings) / full) - 10.0).abs() < 1e-9);
        settings.band.agc_enabled = false;
        assert_eq!(threshold(&settings), full);
    }
}
