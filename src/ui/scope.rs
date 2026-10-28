//! The receiver's level scope, as a rig with a panadapter and an audio scope
//! shows its AGC at work: on top, RF, what reaches the AGC through the RF
//! gain, against the dashed line where the AGC starts to act and a dotted
//! mark at the band's own hiss; below, AF, what the receiver puts out,
//! before the volume you listen at, as a rig's audio scope reads it,
//! against where the limiter starts at full volume; and a lamp lit while
//! the AGC holds the gain down. What the RF gain and the AGC's threshold
//! are set by: a station over the line comes out level with the others, one
//! under it at its own level, and with the line down in the hiss, the hiss
//! comes up with them.

use std::collections::VecDeque;

use cw_core::band::{
    FilterDesign, SOFT_LIMIT_KNEE, agc_knee, band_hiss_rms, gain_to_db, threshold_factor,
    threshold_over_hiss_db,
};
use cw_core::{BandSettings, TrainingSettings};
use dioxus::prelude::*;

use crate::audio::{METER_FRAME_SEC, MeterFrame};
use crate::state::AppCtx;
use crate::time::sleep_ms;

/// How many columns the scope draws, the newest at the right, a unit of the
/// drawing wide: each two meter frames ([`FRAMES_A_COLUMN`]), so seven
/// seconds. A whole round of the preview's three calls at 25 WPM and
/// faster, and two of them side by side at its slowest default speed: loud,
/// medium and weak are compared where they stand, not remembered.
const COLUMNS: usize = 350;

/// How many meter frames make a column, merged as the meter merges its own
/// samples: the louder peaks and the lower gain.
const FRAMES_A_COLUMN: usize = 2;

/// What the scope keeps: its columns' frames, and one more, the newest,
/// waiting for its partner.
const KEPT_FRAMES: usize = COLUMNS * FRAMES_A_COLUMN + 1;

/// How much of the past the readout's room under the limiter is of: the
/// last half second. At 10 WPM and over a dah or two of the station is
/// always in it; in a long Farnsworth gap it reads the hiss coming out
/// between a slow sender's characters.
const READ_OVER: usize = (0.5 / METER_FRAME_SEC) as usize;

/// How often the scope takes the frames the player has metered.
const POLL_MS: u32 = 50;

/// How many of the newest frames the lamp is lit by, one poll's worth: the
/// AGC is acting while its hang has held the gain down through every one of
/// them.
const HELD_OVER: usize = 5;

/// How far down the AGC's hang has to have held the gain for the lamp to
/// light: a decibel, through [`HELD_OVER`] frames on end. The hang holds
/// only what stood over the threshold for a Morse element, so a station
/// over it lights the lamp and the hang and the decay after it keep it lit,
/// and so does the odd long crash the AGC takes for a signal, as a rig's
/// hang AGC does; the crackle of static, which the AGC squashes for an
/// instant and lets go of, never lights it.
const LAMP_DB: f64 = 1.0;

/// One pane of the scope: where it starts down the drawing, and the levels
/// at its top and its bottom, a decibel to a unit, as both panes are so that
/// a step going in can be held against the step coming out.
struct Pane {
    top: f64,
    top_db: f64,
    bottom_db: f64,
}

impl Pane {
    fn height(&self) -> f64 {
        self.top_db - self.bottom_db
    }

    fn base(&self) -> f64 {
        self.top + self.height()
    }

    /// How far down the drawing `level_db` sits in this pane: held to it,
    /// and silence at its base.
    fn y(&self, level_db: f64) -> f64 {
        if level_db.is_nan() {
            return self.base();
        }
        self.top + (self.top_db - level_db).clamp(0.0, self.height())
    }
}

/// RF: what reaches the AGC, in decibels over the band's own knee, where
/// the threshold at 0 dB puts the AGC. The strongest station stands about
/// 28 over it, and the crests of the quietest hiss about 7 under it; with
/// the RF gain backed off all the way they are still on it, 37 under, and
/// the line is, wherever the threshold puts it, from 20 under to 30 over.
/// The hiss mark, a few decibels under its crests, leaves the pane before
/// them, as a panadapter's floor drops off its screen.
const RF: Pane = Pane {
    top: 0.0,
    top_db: 32.0,
    bottom_db: -40.0,
};

/// AF: what the receiver puts out, before the volume, in decibels against
/// where the limiter starts to bend at full volume: 6 over it, room for a
/// crash going into it, down to 42 under, beneath the hiss of a band played
/// with the AGC off. Full volume takes the strongest station to just under
/// it, so the volume, which only turns down, never takes anything over it.
const AF: Pane = Pane {
    top: 80.0,
    top_db: 6.0,
    bottom_db: -42.0,
};

/// The drawing, in its own units: a column to a unit across, and down, a
/// pixel to a unit, the two panes with a gap between.
const WIDTH: f64 = COLUMNS as f64;
const HEIGHT: f64 = 128.0;

/// How far into its pane, from the top, a pane's tag sits.
const TAG_INSET: f64 = 3.0;

/// The AGC knee of the band the preview plays, and the hiss it is measured
/// against: what the scope draws its line and its hiss mark by, and reads
/// the threshold over the hiss from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct KneeHeard {
    /// The band's own knee, where the threshold at 0 dB puts the AGC, with
    /// the AGC on whether it is or not.
    pub knee: f64,
    /// The band's own hiss out of the receiver's filter, before the RF gain
    /// ([`band_hiss_rms`]); nothing with the noise off.
    pub hiss: f64,
}

impl KneeHeard {
    /// `knee`, measured for the band `on` describes with its AGC on, heard
    /// through `receiver`, and the hiss of that band. Worked out, never
    /// measured.
    pub(crate) fn of(on: &BandSettings, receiver: &FilterDesign, knee: f64) -> Self {
        Self {
            knee,
            hiss: band_hiss_rms(on, receiver),
        }
    }
}

/// What the band plays to, for the scope (`use_band_follow`): as the band
/// follow on the settings screen last measured it ahead, off the thread
/// that draws where there is another, or, for settings the follow took
/// unmeasured with the AGC on, as the band built for them measured it. The
/// scope draws its line by it, and so never measures one, which is three
/// seconds of band.
#[derive(Clone, Copy)]
pub(crate) struct BandKnee(pub Signal<Option<KneeHeard>>);

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
    agc_knee(&on, &receiver).unwrap_or(1.0) * threshold_factor(&settings.band)
}

/// The step the RF trace takes to a level `over_db` over the threshold
/// these settings set, as it is written in the trace: what tests find a
/// level drawn by its knee with.
#[cfg(test)]
pub(crate) fn rf_trace_step(settings: &TrainingSettings, over_db: f64) -> String {
    let over_knee = over_db + gain_to_db(threshold_factor(&settings.band));
    format!("V{}h", tenths(RF.y(over_knee)))
}

/// Where the scope marks the band's hiss, and what it says of the threshold
/// against it, for the knee and the hiss `heard` and the RF gain, the
/// threshold and the AGC `settings` set: the hiss as it reaches the AGC, a
/// steady tone's worth of it, in decibels over the band's own knee, so the
/// mark sits as far under the line at 0 dB as the threshold stands over the
/// hiss; and, with the AGC on, how far the threshold stands over the hiss,
/// or under it, RMS to RMS, as a station's S/N is read. Neither with no
/// hiss.
fn hiss_marks(heard: KneeHeard, settings: &TrainingSettings) -> (Option<f64>, Option<f64>) {
    let hiss = heard.hiss * settings.rf_gain();
    let mark = threshold_over_hiss_db(heard.knee, hiss).map(|over| -over);
    let moved = heard.knee * threshold_factor(&settings.band);
    let over = threshold_over_hiss_db(moved, hiss).filter(|_| settings.band.agc_enabled);
    (mark, over)
}

/// The scope's hiss mark and threshold figure for these settings, their
/// knee measured: what tests hold them to.
#[cfg(test)]
fn marks_of(settings: &TrainingSettings) -> (Option<f64>, Option<f64>) {
    let (on, receiver) = knee_asked(settings);
    agc_knee(&on, &receiver).map_or((None, None), |knee| {
        hiss_marks(KneeHeard::of(&on, &receiver, knee), settings)
    })
}

/// How far the threshold these settings set stands over the band's hiss,
/// as the scope reads it.
#[cfg(test)]
pub(crate) fn over_hiss(settings: &TrainingSettings) -> Option<f64> {
    marks_of(settings).1
}

/// Where the hiss mark is drawn for these settings, as the drawing writes
/// it: what tests find it by.
#[cfg(test)]
pub(crate) fn hiss_mark_at(settings: &TrainingSettings) -> Option<String> {
    let mark = marks_of(settings).0;
    Drawing::of(&[], 1.0, 1.0, mark)
        .hiss
        .map(|y| format!("y1=\"{y:.1}\""))
}

/// A ratio in decibels, and nothing at all, at the foot of any pane, for no
/// level or one that is not a number.
fn db(ratio: f64) -> f64 {
    if ratio > 0.0 {
        gain_to_db(ratio)
    } else {
        f64::NEG_INFINITY
    }
}

fn tenths(units: f64) -> i64 {
    (units * 10.0).round() as i64
}

/// What the scope has taken of the player's frames: its columns' worth, and
/// how many it has been given in all, which pairs them into columns the
/// same way however the polls fall; and whether anything has reached the
/// AGC since it was shown.
struct Kept {
    frames: VecDeque<MeterFrame>,
    given: u64,
    heard: bool,
}

impl Kept {
    fn new() -> Self {
        Self {
            frames: VecDeque::with_capacity(KEPT_FRAMES + 1),
            given: 0,
            heard: false,
        }
    }

    fn add(&mut self, taken: &mut Vec<MeterFrame>) {
        self.heard |= taken.iter().any(|frame| frame.input > 0.0);
        self.given += taken.len() as u64;
        self.frames.extend(taken.drain(..));
        let extra = self.frames.len().saturating_sub(KEPT_FRAMES);
        self.frames.drain(..extra);
    }

    /// The columns, oldest first, each a pair of frames merged, paired from
    /// the first frame ever given, so a column once drawn never changes. The
    /// newest frame, if it has no partner yet, waits for it.
    fn columns(&self) -> Vec<MeterFrame> {
        let first = self.given - self.frames.len() as u64;
        // The oldest kept may be the second of a pair whose first is gone.
        let skip = (first % FRAMES_A_COLUMN as u64) as usize;
        let frames: Vec<MeterFrame> = self.frames.iter().skip(skip).copied().collect();
        let (pairs, _waiting) = frames.as_chunks::<FRAMES_A_COLUMN>();
        pairs
            .iter()
            .map(|pair| pair.iter().fold(MeterFrame::QUIET, |a, b| a.merge(*b)))
            .collect()
    }
}

/// The scope's drawing: the RF trace against the band's own knee, `knee`,
/// the dashed line where the AGC starts, as many times that as the
/// threshold puts it at, `factor`, and the dotted mark at the band's hiss,
/// `hiss_db` over the knee; and the AF trace against the limiter. The RF gain moves the RF trace and the mark, the threshold the
/// line, as on a panadapter; what the AGC does shows in AF alone.
#[derive(Debug, PartialEq)]
struct Drawing {
    rf: String,
    af: String,
    line: f64,
    hiss: Option<f64>,
}

impl Drawing {
    fn of(columns: &[MeterFrame], knee: f64, factor: f64, hiss_db: Option<f64>) -> Self {
        // The newest column at the right-hand edge, the rest running back
        // from it.
        let start = COLUMNS.saturating_sub(columns.len());
        let rf = columns
            .iter()
            .map(|column| RF.y(db(f64::from(column.input) / knee)));
        let af = columns
            .iter()
            .map(|column| AF.y(db(f64::from(column.output) / SOFT_LIMIT_KNEE)));
        Self {
            rf: trace_path(start, RF.base(), rf),
            af: trace_path(start, AF.base(), af),
            line: RF.y(db(factor)),
            // Under the pane, the mark would sit on the floor as if the hiss
            // were there.
            hiss: hiss_db
                .filter(|level| *level > RF.bottom_db)
                .map(|level| RF.y(level)),
        }
    }
}

/// One trace as an SVG path in tenths of the drawing's units, drawn at
/// `scale(0.1)`, so it is written in whole numbers: along `base` to where
/// the columns start, a step a column at the height each is given, a run
/// of equal steps one stretch, and back to `base` at the right-hand edge.
/// Up to twenty times a second ([`POLL_MS`]), a point a column formatted
/// as two decimals was three times the text.
fn trace_path(start: usize, base: f64, heights: impl Iterator<Item = f64>) -> String {
    use std::fmt::Write;
    let column = tenths(WIDTH / COLUMNS as f64);
    let base = tenths(base);
    let mut path = String::with_capacity(32 + 12 * COLUMNS);
    let _ = write!(path, "M0 {base}H{}", start as i64 * column);
    let (mut at, mut run) = (None, 0);
    for y in heights.map(tenths) {
        if at != Some(y) {
            if run > 0 {
                let _ = write!(path, "h{run}");
            }
            let _ = write!(path, "V{y}");
            (at, run) = (Some(y), 0);
        }
        run += column;
    }
    if run > 0 {
        let _ = write!(path, "h{run}");
    }
    let _ = write!(path, "V{base}Z");
    path
}

/// The readout's two readings, in decibels: how far the AGC has held the
/// gain down through the newest poll's frames, and the loudest played over
/// the last half second, at the volume you listen at, against the
/// limiter's knee, under it or into it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reading {
    held_db: f64,
    headroom_db: f64,
}

impl Reading {
    /// The readings of `frames`, the newest last, played at `volume`; none
    /// with none to read.
    fn of(frames: &VecDeque<MeterFrame>, volume: f64) -> Option<Self> {
        let newest = |n: usize| frames.iter().rev().take(n);
        // The most gain any of them let through: what was held throughout.
        let held = newest(HELD_OVER).fold(None, |most: Option<f32>, frame| {
            Some(most.map_or(frame.held, |most| most.max(frame.held)))
        })?;
        let output = newest(READ_OVER).fold(0.0f32, |loudest, frame| loudest.max(frame.output));
        Some(Self {
            held_db: -db(f64::from(held)),
            headroom_db: -db(f64::from(output) * volume / SOFT_LIMIT_KNEE),
        })
    }

    /// Whether the AGC is holding the gain down: what lights the lamp, and
    /// what it is said to take.
    fn acting(&self) -> bool {
        self.held_db >= LAMP_DB
    }

    /// The readout in words, after the lamp's "AGC": what the AGC is doing,
    /// and the room left under the limiter.
    fn says(&self, agc_on: bool) -> String {
        let agc = if !agc_on {
            "off".to_string()
        } else if self.acting() {
            format!("takes {:.0} dB", self.held_db)
        } else {
            "idle".to_string()
        };
        // A peak that only grazes the knee is not said to be into it.
        let room = if !self.headroom_db.is_finite() {
            "silent".to_string()
        } else if self.headroom_db > -0.5 {
            format!("{:.0} dB under the limiter", self.headroom_db.max(0.0))
        } else {
            format!("{:.0} dB into the limiter", -self.headroom_db)
        };
        format!("{agc} · {room}")
    }
}

/// The lamp's look: lit while the AGC acts, dark while it is idle or
/// nothing has been heard, and dashed with the AGC switched off.
fn lamp_class(agc_on: bool, reading: Option<&Reading>) -> &'static str {
    if !agc_on {
        "badge scope-lamp off"
    } else if reading.is_some_and(Reading::acting) {
        "badge scope-lamp on"
    } else {
        "badge scope-lamp"
    }
}

/// What the scope says of a threshold `over_db` over the band's hiss, or
/// under it, in whole decibels: over it, the S/N a station needs to be held
/// level; at it and under it, the hiss works the AGC too, as on a rig with
/// its threshold set into the noise.
fn hiss_note(over_db: f64) -> String {
    let over = over_db.round();
    if over >= 1.0 {
        format!(
            "Threshold {over:.0} dB over the hiss: a station is held level while its S/N stands over it."
        )
    } else if over > -1.0 {
        "Threshold at the hiss: the hiss starts to work the AGC.".to_string()
    } else {
        format!(
            "Threshold {:.0} dB under the hiss: the hiss works the AGC and comes up with the stations.",
            -over
        )
    }
}

/// The level scope, for as long as it is shown: it takes the player's
/// meter frames every [`POLL_MS`] and draws the last seven seconds of them.
/// Shown while the band preview plays, so there is a receiver to meter.
#[component]
pub fn LevelScope(settings: Signal<TrainingSettings>) -> Element {
    let ctx = try_use_context::<AppCtx>();
    let mut kept = use_signal(Kept::new);
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
            kept.with_mut(|kept| kept.add(&mut taken));
        }
    });
    // The traces follow a move as the band does, once the follow has
    // measured its knee; the line follows the threshold at once, the hiss
    // mark the RF gain, and the readout's room the volume.
    let band_knee = try_use_context::<BandKnee>();
    let (heard, factor, agc_on, volume, (hiss_db, over_hiss)) = use_memo(move || {
        let settings = settings.read();
        let heard = match band_knee {
            Some(BandKnee(heard)) => heard(),
            // Drawn on its own, as a card is in a test, with no follow to
            // measure it.
            None => {
                let (on, receiver) = knee_asked(&settings);
                agc_knee(&on, &receiver).map(|knee| KneeHeard::of(&on, &receiver, knee))
            }
        };
        (
            heard,
            threshold_factor(&settings.band),
            settings.band.agc_enabled,
            settings.master_volume(),
            heard.map_or((None, None), |heard| hiss_marks(heard, &settings)),
        )
    })();
    // Nothing is placed against a line not yet known.
    let kept = kept.read();
    let known = heard.is_some();
    let empty = VecDeque::new();
    let frames = if known { &kept.frames } else { &empty };
    let columns = if known { kept.columns() } else { Vec::new() };
    let knee = heard.map_or(1.0, |heard| heard.knee);
    let Drawing {
        rf,
        af,
        line,
        hiss: hiss_y,
    } = Drawing::of(&columns, knee, factor, hiss_db);
    let reading = (known && kept.heard)
        .then(|| Reading::of(frames, volume))
        .flatten();
    let says = reading.map_or_else(|| "Listening…".to_string(), |reading| reading.says(agc_on));
    let lamp = lamp_class(agc_on, reading.as_ref());
    let note = over_hiss.map(hiss_note);
    let label = match note.as_deref() {
        Some(note) => format!("AGC {says}. {note}"),
        None => format!("AGC {says}."),
    };
    let view_box = format!("0 0 {WIDTH} {HEIGHT}");
    rsx! {
        div { id: "level-scope", class: "scope",
            div { class: "scope-screen",
                svg {
                    class: "scope-plot",
                    view_box: "{view_box}",
                    height: "{HEIGHT}",
                    preserve_aspect_ratio: "none",
                    role: "img",
                    "aria-label": "The receiver's level into its AGC and out of it, over the last seven seconds: {label}",
                    rect {
                        class: "scope-pane",
                        x: "0",
                        y: "{RF.top}",
                        width: "{WIDTH}",
                        height: "{RF.height()}",
                    }
                    path {
                        id: "scope-rf",
                        class: "scope-level",
                        transform: "scale(0.1)",
                        d: "{rf}",
                    }
                    if let Some(hiss_y) = hiss_y {
                        line {
                            class: "scope-hiss",
                            x1: "0",
                            x2: "{WIDTH}",
                            y1: "{hiss_y:.1}",
                            y2: "{hiss_y:.1}",
                        }
                    }
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
                        y: "{AF.top}",
                        width: "{WIDTH}",
                        height: "{AF.height()}",
                    }
                    path {
                        id: "scope-af",
                        class: "scope-level",
                        transform: "scale(0.1)",
                        d: "{af}",
                    }
                    line {
                        class: "scope-limit",
                        x1: "0",
                        x2: "{WIDTH}",
                        y1: "{AF.y(0.0)}",
                        y2: "{AF.y(0.0)}",
                    }
                }
                span {
                    class: "scope-tag",
                    style: "top: {RF.top + TAG_INSET}px;",
                    "aria-hidden": "true",
                    "RF"
                }
                span {
                    class: "scope-tag",
                    style: "top: {AF.top + TAG_INSET}px;",
                    "aria-hidden": "true",
                    "AF"
                }
            }
            div { class: "scope-meter",
                span { id: "scope-lamp", class: "{lamp}", "AGC" }
                p { id: "scope-reading", class: "muted scope-reading", "{says}" }
            }
            if let Some(note) = note {
                p { id: "scope-over-hiss", class: "muted scope-legend", "{note}" }
            }
            p { class: "muted scope-legend",
                "Top, RF: the band as it reaches the AGC, dashed where the AGC starts, dotted at the band's own hiss. Below, AF: what the receiver puts out before the volume, against where the limiter starts at full volume. The last seven seconds; the AGC lamp is lit while the AGC holds the gain down."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(of: &[(f32, f32, f32)]) -> VecDeque<MeterFrame> {
        of.iter()
            .map(|&(input, held, output)| MeterFrame {
                input,
                held,
                output,
            })
            .collect()
    }

    fn gain(db: f64) -> f32 {
        10f64.powf(db / 20.0) as f32
    }

    /// The readout is the gain the AGC has held through the newest poll's
    /// frames and the room left under the limiter over the last half
    /// second, said after the lamp's "AGC".
    #[test]
    fn the_reading_is_the_gain_held_and_the_last_half_seconds_loudest() {
        let mut kept = frames(&[(0.9, 0.2, 0.69); 100]);
        kept.extend(frames(&[(0.2, 0.5, 0.35); READ_OVER]));
        let reading = Reading::of(&kept, 1.0).expect("something heard");
        assert!((reading.held_db - 6.02).abs() < 0.01, "{reading:?}");
        assert!((reading.headroom_db - 6.02).abs() < 0.01, "{reading:?}");
        assert_eq!(reading.says(true), "takes 6 dB · 6 dB under the limiter");
        assert_eq!(reading.says(false), "off · 6 dB under the limiter");
        let quiet = Reading::of(&frames(&[(0.05, 1.0, 0.1)]), 1.0).expect("heard");
        assert_eq!(quiet.says(true), "idle · 17 dB under the limiter");
        assert_eq!(Reading::of(&VecDeque::new(), 1.0), None);
        let silent = Reading::of(&frames(&[(0.0, 1.0, 0.0)]), 1.0).expect("read");
        assert_eq!(silent.says(true), "idle · silent");
        // Played past the knee, it says how far into the limiter.
        let hot = Reading::of(&frames(&[(0.2, 1.0, 1.4)]), 1.0).expect("heard");
        assert!(
            hot.says(false).ends_with("6 dB into the limiter"),
            "{}",
            hot.says(false)
        );
        let grazing = Reading {
            held_db: 0.0,
            headroom_db: -0.3,
        };
        assert!(grazing.says(true).ends_with("0 dB under the limiter"));
        // Listened to 6 dB down, what was into the limiter at full volume
        // is under it.
        let turned_down = Reading::of(&frames(&[(0.2, 1.0, 1.4)]), 0.5).expect("heard");
        assert!(
            turned_down.says(false).ends_with("0 dB under the limiter"),
            "{}",
            turned_down.says(false)
        );
    }

    /// The threshold is read against the hiss in whole decibels: over it,
    /// the S/N a station needs to be held level; at it and under it, the
    /// hiss works the AGC too.
    #[test]
    fn the_threshold_is_read_over_at_or_under_the_hiss() {
        assert_eq!(
            hiss_note(12.6),
            "Threshold 13 dB over the hiss: a station is held level while its S/N stands over it."
        );
        for at in [0.4, 0.0, -0.0, -0.4] {
            assert_eq!(
                hiss_note(at),
                "Threshold at the hiss: the hiss starts to work the AGC.",
                "{at}"
            );
        }
        assert_eq!(
            hiss_note(-7.4),
            "Threshold 7 dB under the hiss: the hiss works the AGC and comes up with the stations."
        );
    }

    /// The lamp is lit only while the AGC's hang has held the gain a decibel
    /// or more down through every one of the newest poll's frames: not by a
    /// crash the AGC only squashes, which leaves the hang at one, nor by a
    /// hold only just begun, and out as soon as it lets go. Dashed with the
    /// AGC off, dark before anything is heard.
    #[test]
    fn the_lamp_lights_only_while_the_gain_is_held_a_decibel_down() {
        let lit = |kept: &VecDeque<MeterFrame>| lamp_class(true, Reading::of(kept, 1.0).as_ref());
        let held = frames(&[(0.5, gain(-1.2), 0.5); HELD_OVER]);
        assert_eq!(lit(&held), "badge scope-lamp on");
        let reading = Reading::of(&held, 1.0).expect("heard");
        assert_eq!(reading.says(true), "takes 1 dB · 3 dB under the limiter");
        let mut letting_go = frames(&[(0.5, gain(-6.0), 0.5); HELD_OVER - 1]);
        letting_go.extend(frames(&[(0.5, gain(-0.5), 0.5)]));
        assert_eq!(lit(&letting_go), "badge scope-lamp");
        let mut crash = frames(&[(0.1, 1.0, 0.1); 49]);
        crash.extend(frames(&[(0.9, 1.0, 0.6); HELD_OVER]));
        assert_eq!(lit(&crash), "badge scope-lamp");
        let mut begun = frames(&[(0.1, 1.0, 0.1); 49]);
        begun.extend(frames(&[(0.9, gain(-12.0), 0.6)]));
        assert_eq!(lit(&begun), "badge scope-lamp");
        let mut gone = frames(&[(0.5, gain(-6.0), 0.5); 50]);
        gone.extend(frames(&[(0.01, 1.0, 0.01)]));
        assert_eq!(lit(&gone), "badge scope-lamp");
        assert_eq!(
            lamp_class(false, Reading::of(&held, 1.0).as_ref()),
            "badge scope-lamp off"
        );
        assert_eq!(lamp_class(true, None), "badge scope-lamp");
    }

    /// Two frames make a column, the louder peaks and the lower gain, paired
    /// from the first frame given so that a column once drawn never changes
    /// however the polls fall; an odd frame waits for its partner, and the
    /// oldest go once the scope is full.
    #[test]
    fn two_frames_make_a_column_and_a_column_never_changes() {
        let frame = |i: usize| MeterFrame {
            input: i as f32,
            held: 1.0 / (i as f32 + 1.0),
            output: (i % 3) as f32,
        };
        let give = |kept: &mut Kept, given: &mut usize, n: usize| {
            let mut taken: Vec<MeterFrame> = (*given..*given + n).map(frame).collect();
            *given += n;
            kept.add(&mut taken);
        };
        let (mut kept, mut given) = (Kept::new(), 0);
        give(&mut kept, &mut given, 3);
        assert_eq!(kept.columns(), [frame(0).merge(frame(1))]);
        give(&mut kept, &mut given, 1);
        assert_eq!(
            kept.columns(),
            [frame(0).merge(frame(1)), frame(2).merge(frame(3))]
        );
        while given < 2 * COLUMNS + 9 {
            give(&mut kept, &mut given, 3);
        }
        let columns = kept.columns();
        assert_eq!(columns.len(), COLUMNS);
        // The newest frame given waits for its partner, if it has none.
        let newest_pair = given / 2 * 2 - 2;
        assert_eq!(
            columns.last(),
            Some(&frame(newest_pair).merge(frame(newest_pair + 1)))
        );
        // Its partner come, the oldest kept is the second of a pair whose
        // first is gone: the columns move along by one, none paired anew.
        give(&mut kept, &mut given, 1);
        let moved = kept.columns();
        assert_eq!(moved[..COLUMNS - 1], columns[1..]);
        assert_eq!(
            moved.last(),
            Some(&frame(given - 2).merge(frame(given - 1)))
        );
    }

    /// Nothing is heard until something reaches the AGC: a scope given
    /// only silence is still listening, and once heard it stays heard
    /// through the silence after.
    #[test]
    fn silence_alone_is_not_heard_and_once_heard_stays_heard() {
        let quiet = |n| vec![MeterFrame::QUIET; n];
        let mut kept = Kept::new();
        kept.add(&mut quiet(5));
        assert!(!kept.heard, "silence alone was heard");
        kept.add(&mut vec![MeterFrame {
            input: 0.01,
            ..MeterFrame::QUIET
        }]);
        kept.add(&mut quiet(5));
        assert!(kept.heard, "the silence after a call was not heard through");
    }

    /// The threshold moves the line and leaves both traces where they were;
    /// the RF gain moves the RF trace and the hiss mark and leaves the line.
    /// A decibel to a unit, and moved all the way either way the line is
    /// still on the pane.
    #[test]
    fn the_threshold_moves_the_line_and_the_rf_gain_the_trace() {
        let heard = [MeterFrame {
            input: 0.1,
            held: 1.0,
            output: 0.1,
        }; 4];
        let lowest = Drawing::of(&heard, 0.1, 1.0, Some(-12.0));
        let raised = Drawing::of(&heard, 0.1, 10f64.powf(0.5), Some(-12.0));
        assert_eq!((&lowest.rf, &lowest.af), (&raised.rf, &raised.af));
        assert!((lowest.line - raised.line - 10.0).abs() < 1e-9);
        let backed_off = Drawing::of(
            &[MeterFrame {
                input: 0.01,
                ..heard[0]
            }; 4],
            0.1,
            1.0,
            Some(-32.0),
        );
        assert_eq!(backed_off.line, lowest.line);
        assert_ne!(backed_off.rf, lowest.rf);
        assert_eq!(backed_off.hiss.zip(lowest.hiss), Some((64.0, 44.0)));
        // A hiss under the pane leaves it rather than sit on its floor.
        assert_eq!(Drawing::of(&heard, 0.1, 1.0, Some(-42.6)).hiss, None);
        assert_eq!(Drawing::of(&heard, 0.1, 1.0, Some(-39.9)).hiss, Some(71.9));
        let top = Drawing::of(
            &heard,
            0.1,
            10f64.powf(cw_core::AGC_THRESHOLD_MAX_DB / 20.0),
            None,
        );
        assert!(top.line > 0.0, "{}", top.line);
        assert_eq!(top.hiss, None);
        // Taken all the way down, the line is 20 under where it stands at
        // 0 dB, and still on the pane.
        let bottom = Drawing::of(
            &heard,
            0.1,
            10f64.powf(cw_core::AGC_THRESHOLD_MIN_DB / 20.0),
            None,
        );
        assert!((bottom.line - lowest.line - 20.0).abs() < 1e-9);
        assert!(bottom.line < RF.base(), "{}", bottom.line);
    }

    /// What comes out is drawn against the limiter: at its knee on the
    /// line, with room over it for what goes into it, a decibel to a unit,
    /// whatever the knee or the threshold.
    #[test]
    fn the_af_pane_is_drawn_against_the_limiter() {
        let played = |output: f64| MeterFrame {
            input: 0.1,
            held: 1.0,
            output: output as f32,
        };
        let at = |output: f64, knee: f64, raise: f64| {
            Drawing::of(&[played(output)], knee, raise, None).af
        };
        assert!(at(SOFT_LIMIT_KNEE, 0.1, 1.0).contains("V860h"));
        assert!(at(SOFT_LIMIT_KNEE * 2.0, 0.1, 1.0).contains("V800h"));
        assert!(at(SOFT_LIMIT_KNEE * 10f64.powf(-42.0 / 20.0), 0.1, 1.0).contains("V1280h"));
        assert!(at(SOFT_LIMIT_KNEE * 0.5, 0.1, 1.0).contains("V920h"));
        assert_eq!(at(0.3, 0.1, 1.0), at(0.3, 0.02, 10.0));
        assert_eq!(AF.y(0.0), 86.0);
        assert_eq!(AF.base(), HEIGHT);
        assert_eq!(RF.base(), 72.0);
        assert_eq!(AF.y(f64::NAN), HEIGHT);
    }

    /// A trace is drawn in whole tenths, along its base to its first column,
    /// a step a column, a run of equal columns one stretch, and closed on
    /// its base at the right-hand edge; never more than twelve bytes a
    /// column, however its heights alternate.
    #[test]
    fn a_trace_is_drawn_in_whole_steps() {
        assert_eq!(
            trace_path(COLUMNS - 3, RF.base(), [10.0, 10.0, 20.56].into_iter()),
            format!("M0 720H{}V100h20V206h10V720Z", (COLUMNS - 3) * 10)
        );
        let hung = trace_path(0, AF.base(), std::iter::repeat_n(AF.base(), COLUMNS));
        assert_eq!(hung, format!("M0 1280H0V1280h{}V1280Z", COLUMNS * 10));
        assert_eq!(
            trace_path(COLUMNS, AF.base(), std::iter::empty()),
            format!("M0 1280H{}V1280Z", COLUMNS * 10)
        );
        let busiest = trace_path(
            0,
            AF.base(),
            (0..COLUMNS).map(|k| if k % 2 == 0 { AF.top } else { AF.base() }),
        );
        assert!(busiest.len() <= 32 + 12 * COLUMNS, "{}", busiest.len());
        let mut settings = TrainingSettings::default();
        assert_eq!(rf_trace_step(&settings, 9.0), "V230h");
        settings.band.agc_threshold_db = 10.0;
        assert_eq!(rf_trace_step(&settings, 9.0), "V130h");
    }

    /// The scope spans seven seconds, its lamp a poll's worth of frames and
    /// its room half a second.
    #[test]
    fn the_scope_spans_seven_seconds() {
        let span = (COLUMNS * FRAMES_A_COLUMN) as f64 * METER_FRAME_SEC;
        assert!((span - 7.0).abs() < 1e-12, "{span}");
        let held = HELD_OVER as f64 * METER_FRAME_SEC;
        assert!(
            (held - f64::from(POLL_MS) / 1_000.0).abs() < 1e-12,
            "{held}"
        );
        assert!((READ_OVER as f64 * METER_FRAME_SEC - 0.5).abs() < 1e-12);
    }

    /// The line sits where the AGC starts, as far from the band's own knee
    /// as the threshold moves it, wherever the RF gain is; with the AGC off,
    /// where it would start were it switched on. The figure under the
    /// readout is that threshold over the band's hiss, moved with it, and
    /// with the RF gain backed off, and says nothing with the AGC off; taken
    /// down into the hiss, it goes under it. The hiss mark sits as far under
    /// the line at 0 dB as that figure, and moves with the RF gain alone.
    #[test]
    fn the_line_is_the_agc_threshold() {
        let mut settings = TrainingSettings::default();
        let lowest = threshold(&settings);
        let (mark, over) = marks_of(&settings);
        let (mark, over) = (mark.expect("a band with hiss"), over.expect("hiss"));
        assert!((10.0..14.0).contains(&over), "{over:.1} dB over the hiss");
        assert!((mark + over).abs() < 1e-9, "{mark:.2} against {over:.2}");
        settings.band.agc_threshold_db = 10.0;
        let raised = threshold(&settings);
        assert!((db(raised / lowest) - 10.0).abs() < 1e-9);
        let (raised_mark, raised_over) = marks_of(&settings);
        assert!((raised_over.expect("hiss") - over - 10.0).abs() < 1e-9);
        assert_eq!(raised_mark, Some(mark), "the threshold moved the mark");
        let mut into = settings.clone();
        into.band.agc_threshold_db = cw_core::AGC_THRESHOLD_MIN_DB;
        assert!((db(threshold(&into) / lowest) + 20.0).abs() < 1e-9);
        let (into_mark, into_over) = marks_of(&into);
        let into_over = into_over.expect("hiss");
        assert!((into_over - over + 20.0).abs() < 1e-9);
        assert!(into_over < 0.0, "{into_over:.1} dB: under the hiss");
        assert_eq!(into_mark, Some(mark), "the threshold moved the mark");
        settings.band.rf_gain_db = -20.0;
        assert_eq!(threshold(&settings), raised);
        let (backed_off_mark, backed_off_over) = marks_of(&settings);
        assert!((backed_off_over.expect("hiss") - over - 30.0).abs() < 1e-9);
        assert!((backed_off_mark.expect("hiss") - mark + 20.0).abs() < 1e-9);
        settings.band.agc_enabled = false;
        assert_eq!(threshold(&settings), raised);
        assert_eq!(over_hiss(&settings), None, "a figure with the AGC off");
        assert!(marks_of(&settings).0.is_some(), "the hiss is still marked");
        settings.band.noise_enabled = false;
        assert_eq!(marks_of(&settings), (None, None));
    }
}
