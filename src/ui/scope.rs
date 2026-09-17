//! Band scope: where the stations sit in the receiver's passband.
//!
//! This is the one display that can be on screen while you are copying. A
//! picture of the keying would hand you the answer — you would read the group
//! off the trace instead of hearing it — so the scope stays in the frequency
//! domain, which tells you where everyone is without telling you what anyone
//! is sending.
//!
//! Which is also the half of the trainer that had no picture at all. The
//! filter, the pitch spread and the pile-up are three settings whose whole
//! effect is where things land relative to each other, and until now you could
//! only find that out by ear.

use cw_core::band::receiver_response_at;
use cw_core::scope::{scope_trace, ScopeStation, SCOPE_WINDOW_SEC};
use cw_core::timing::StationVoice;
use cw_core::TrainingSettings;
use std::fmt::Write as _;

use dioxus::prelude::*;

use crate::time::{mark, since_ms, sleep_ms};

const VIEW_W: f64 = 320.0;
const TRACE_H: f64 = 70.0;
/// Fast enough to read as a live trace, slow enough that a phone is not
/// redrawing an SVG flat out for the length of a session.
const FRAME_MS: u32 = 40;
/// Enough points to show several cycles of a CW note without a path so long it
/// costs more to ship to the renderer than to draw. At 600 Hz over an 8 ms
/// face this is roughly twenty points a cycle, which is already smooth.
const TRACE_POINTS: usize = 96;
const VIEW_H: f64 = 96.0;
const PAD_X: f64 = 8.0;
const BASE_Y: f64 = 80.0;
const AMP: f64 = 62.0;

/// A station the receiver is hearing, as far as the scope is concerned.
///
/// Deliberately a pitch and a strength and nothing else. What a station is
/// *sending* has no business on the screen you are copying on, so the text
/// cannot reach here to be drawn by accident.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub voice: StationVoice,
    /// The station being copied, as opposed to somebody calling across it.
    pub wanted: bool,
    /// When this station's key is down, as `(start, end)` seconds from the
    /// top of the send.
    ///
    /// The trace needs this to breathe with what you are hearing rather than
    /// sitting at a constant height. It is never drawn as a shape — see the
    /// module note on why an envelope trace would be a way of reading the
    /// morse instead of copying it.
    pub key: Vec<(f64, f64)>,
    /// How long the keying takes to rise, so the trace softens at the edges
    /// the way the audio does rather than snapping on.
    pub rise_sec: f64,
}

impl Heard {
    /// How loud this station is at `t_sec` into the send, before the filter.
    pub fn amplitude_at(&self, t_sec: f64) -> f64 {
        let rise = self.rise_sec.max(1e-4);
        let shape = self
            .key
            .iter()
            .map(|(start, end)| {
                if t_sec < *start || t_sec > *end {
                    return 0.0;
                }
                // Raised cosine in and out, which is the keying the audio uses.
                let into = ((t_sec - start) / rise).clamp(0.0, 1.0);
                let outof = ((end - t_sec) / rise).clamp(0.0, 1.0);
                let ramp = |x: f64| 0.5 - 0.5 * (std::f64::consts::PI * x).cos();
                ramp(into).min(ramp(outof))
            })
            .fold(0.0_f64, f64::max);
        self.voice.volume * shape
    }
}

/// One station as the scope draws it.
pub struct Blip {
    pub x: f64,
    pub top_y: f64,
    /// The station being copied, as opposed to somebody calling across it.
    pub wanted: bool,
}

/// Everything the scope needs, in view coordinates.
pub struct ScopeGeometry {
    pub curve: String,
    pub skirt: String,
    pub centre_x: f64,
    pub edge_low_x: f64,
    pub edge_high_x: f64,
    pub blips: Vec<Blip>,
    pub span_low: f64,
    pub span_high: f64,
}

/// How wide a slice of the band to draw.
///
/// Wide enough for the filter's skirts, for wherever the tuning range can put
/// a station, and for however far the pile-up can spread. Then widened again
/// around whoever is actually on: the slice is derived from what it has to
/// hold, so nothing can be drawn off the edge of it whatever the settings say.
fn span(settings: &TrainingSettings, sending: &[Heard]) -> (f64, f64) {
    let centre = settings.side_tone_center();
    let half_filter = settings.band.filter_bandwidth_hz / 2.0;
    let reach = if settings.band.stations_max > 1 {
        settings.band.pileup_spread_hz
    } else {
        0.0
    };
    let tones = (settings.band.side_tone_max - settings.band.side_tone_min).abs() / 2.0;
    let mut half = (half_filter * 1.25)
        .max(reach + 90.0)
        .max(tones + 90.0)
        .max(180.0);
    for station in sending {
        half = half.max((station.voice.tone_hz - centre).abs() + 40.0);
    }
    ((centre - half).max(0.0), centre + half)
}

fn scope_geometry(settings: &TrainingSettings, sending: &[Heard]) -> ScopeGeometry {
    let centre = settings.side_tone_center();
    let bandwidth = settings.band.filter_bandwidth_hz;
    let (span_low, span_high) = span(settings, sending);
    let width = (span_high - span_low).max(1.0);
    let x_at = |hz: f64| PAD_X + ((hz - span_low) / width) * (VIEW_W - 2.0 * PAD_X);
    let y_at = |gain: f64| BASE_Y - gain.clamp(0.0, 1.0) * AMP;

    // The filter's own shape, sampled across the slice on screen.
    const STEPS: usize = 96;
    let mut curve = String::new();
    for step in 0..=STEPS {
        let hz = span_low + width * (step as f64 / STEPS as f64);
        let (x, y) = (x_at(hz), y_at(receiver_response_at(centre, bandwidth, hz)));
        if step == 0 {
            curve.push_str(&format!("M{x:.1},{y:.1}"));
        } else {
            curve.push_str(&format!(" L{x:.1},{y:.1}"));
        }
    }
    let skirt = format!(
        "{curve} L{:.1},{BASE_Y:.1} L{:.1},{BASE_Y:.1} Z",
        x_at(span_high),
        x_at(span_low)
    );

    // A station is drawn at the height you hear it: its own strength, taken
    // down by however much the filter gives back at its pitch.
    let blips = sending
        .iter()
        .map(|station| Blip {
            x: x_at(station.voice.tone_hz),
            top_y: y_at(
                station.voice.volume
                    * receiver_response_at(centre, bandwidth, station.voice.tone_hz),
            ),
            wanted: station.wanted,
        })
        .collect();

    ScopeGeometry {
        curve,
        skirt,
        centre_x: x_at(centre),
        edge_low_x: x_at(centre - bandwidth / 2.0),
        edge_high_x: x_at(centre + bandwidth / 2.0),
        blips,
        span_low,
        span_high,
    }
}

/// One frame of the trace, as an SVG path.
///
/// The height of each station is what you *hear* — its own strength, keyed by
/// its own timeline, taken down by whatever the filter gives back at its pitch
/// — so a station off to the side of a narrow filter draws a smaller trace,
/// exactly as it sounds smaller.
fn trace_path(
    settings: &TrainingSettings,
    sending: &[Heard],
    live: bool,
    elapsed_ms: u64,
) -> String {
    let centre = settings.side_tone_center();
    let bandwidth = settings.band.filter_bandwidth_hz;
    let t_sec = elapsed_ms as f64 / 1000.0;
    let stations: Vec<ScopeStation> = if live {
        sending
            .iter()
            .map(|station| ScopeStation {
                tone_hz: station.voice.tone_hz,
                amplitude: station.amplitude_at(t_sec)
                    * receiver_response_at(centre, bandwidth, station.voice.tone_hz),
            })
            .collect()
    } else {
        Vec::new()
    };
    // Even with nothing being sent the receiver is open, and an open receiver
    // hisses. A flat line would say the radio was off.
    let noise = if settings.band.qrn_enabled {
        (settings.band.qrn_level * 0.22).clamp(0.012, 0.5)
    } else {
        0.012
    };
    let values = scope_trace(
        &stations,
        noise,
        t_sec,
        TRACE_POINTS,
        elapsed_ms.wrapping_add(1),
    );
    let mid = TRACE_H / 2.0;
    let amp = TRACE_H / 2.0 - 3.0;
    let last = (values.len().max(2) - 1) as f64;
    // Written straight into one buffer: this string is rebuilt every frame and
    // shipped to the renderer whole, so its length is the per-frame cost.
    let mut path = String::with_capacity(values.len() * 10);
    for (i, value) in values.iter().enumerate() {
        let x = i as f64 / last * VIEW_W;
        let y = mid - value * amp;
        // x is a fixed ramp, so it is the same every frame; rounding it to
        // whole units to save a character would make the spacing uneven and
        // the sine visibly lumpy, which is the one thing the face must not be.
        let _ = write!(path, "{}{x:.1},{y:.1}", if i == 0 { "M" } else { "L" });
    }
    path
}

/// The receiver, as a scope and a passband.
///
/// The face is live while a send is running: it is the tone you are listening
/// to, on a timebase short enough that the keying cannot be read off it, over
/// the receiver's own hiss. Under it, where each station sits in the filter.
#[component]
pub fn BandScope(
    settings: TrainingSettings,
    sending: Vec<Heard>,
    live: bool,
    send_id: u64,
) -> Element {
    // A scope only looks like a scope if it is redrawn. Everything else on the
    // screen is still, so this is the one place a frame loop earns its keep.
    // Where this send began. Everything on the face is drawn from the clock's
    // distance past it, not from a count of frames: a frame costs 40 ms of
    // waiting plus the render, so counting frames runs slow and the trace
    // drifts further behind the sound the longer a send goes on. Reading the
    // clock means a slow frame is a dropped frame, never a shifted one.
    let mut started = use_signal(mark);
    let mut elapsed_ms = use_signal(|| 0u64);
    use_hook(|| {
        spawn(async move {
            loop {
                sleep_ms(FRAME_MS).await;
                let since = since_ms(*started.peek());
                elapsed_ms.set(since);
            }
        });
    });
    // Every send starts the timebase again, because the keying it is drawn
    // from starts again too: `Heard::key` is seconds from the top of *this*
    // send, not from the top of the session.
    use_effect(use_reactive!(|send_id| {
        let _ = send_id;
        started.set(mark());
        elapsed_ms.set(0);
    }));
    let geometry = scope_geometry(&settings, &sending);
    let callers = geometry.blips.len();
    let trace = trace_path(&settings, &sending, live, elapsed_ms());
    let caption = if !live {
        "Receiver idle".to_string()
    } else if callers > 1 {
        format!("{callers} stations in the passband")
    } else {
        "One station in the passband".to_string()
    };
    rsx! {
        div { class: if live { "scope live" } else { "scope" },
            div { class: "row-between scope-head",
                span { class: "scope-title", "Receiver" }
                span { class: "scope-read", "{settings.band.filter_bandwidth_hz:.0} Hz · {SCOPE_WINDOW_SEC * 1000.0:.0} ms/div" }
            }
            svg {
                class: "scope-trace-face",
                view_box: "0 0 {VIEW_W} {TRACE_H}",
                preserve_aspect_ratio: "none",
                role: "img",
                "aria-label": "{caption}",
                line {
                    class: "scope-base",
                    x1: "0",
                    y1: "{TRACE_H / 2.0:.1}",
                    x2: "{VIEW_W:.1}",
                    y2: "{TRACE_H / 2.0:.1}",
                }
                path { class: "scope-trace", d: "{trace}", fill: "none" }
            }
            svg {
                class: "scope-face",
                view_box: "0 0 {VIEW_W} {VIEW_H}",
                preserve_aspect_ratio: "none",
                "aria-hidden": "true",
                defs {
                    linearGradient { id: "dust-scope-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                        stop { offset: "0%", stop_color: "var(--copper)", stop_opacity: "0.26" }
                        stop { offset: "100%", stop_color: "var(--copper)", stop_opacity: "0.02" }
                    }
                }
                rect {
                    class: "scope-window",
                    x: "{geometry.edge_low_x:.1}",
                    y: "6",
                    width: "{(geometry.edge_high_x - geometry.edge_low_x).max(0.0):.1}",
                    height: "{BASE_Y - 6.0:.1}",
                }
                path { class: "scope-skirt", d: "{geometry.skirt}", fill: "url(#dust-scope-fill)" }
                path { class: "scope-curve", d: "{geometry.curve}", fill: "none" }
                line {
                    class: "scope-centre",
                    x1: "{geometry.centre_x:.1}",
                    y1: "6",
                    x2: "{geometry.centre_x:.1}",
                    y2: "{BASE_Y:.1}",
                }
                line {
                    class: "scope-base",
                    x1: "{PAD_X:.1}",
                    y1: "{BASE_Y:.1}",
                    x2: "{VIEW_W - PAD_X:.1}",
                    y2: "{BASE_Y:.1}",
                }
                for blip in geometry.blips.iter() {
                    line {
                        class: if blip.wanted { "scope-blip wanted" } else { "scope-blip" },
                        x1: "{blip.x:.1}",
                        y1: "{BASE_Y:.1}",
                        x2: "{blip.x:.1}",
                        y2: "{blip.top_y:.1}",
                    }
                }
                for blip in geometry.blips.iter() {
                    circle {
                        class: if blip.wanted { "scope-cap wanted" } else { "scope-cap" },
                        cx: "{blip.x:.1}",
                        cy: "{blip.top_y:.1}",
                        r: if blip.wanted { "3.4" } else { "2.4" },
                    }
                }
            }
            div { class: "row-between scope-foot",
                span { "{geometry.span_low:.0} Hz" }
                span { class: "scope-caption", "{caption}" }
                span { "{geometry.span_high:.0} Hz" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(tone_hz: f64, volume: f64) -> StationVoice {
        StationVoice {
            tone_hz,
            char_wpm: 20.0,
            effective_wpm: 20.0,
            volume,
            weight: 1.0,
            dash_ratio: 3.0,
        }
    }

    /// A scope fed one station keyed for the first tenth of every send, with a
    /// button that starts the next send.
    #[component]
    fn ScopeHarness() -> Element {
        let mut send = use_signal(|| 0u64);
        let mut station = heard(voice(500.0, 1.0), true);
        station.key = vec![(0.0, 0.1)];
        rsx! {
            div {
                button {
                    id: crate::ui::widgets::control_id("btn", "next send"),
                    onclick: move |_| send += 1,
                    "next"
                }
                BandScope {
                    settings: settings(500.0, 1),
                    sending: vec![station],
                    live: true,
                    send_id: send(),
                }
            }
        }
    }

    /// A station that sounds at the top of the send and again a second later,
    /// with a quiet stretch between the two.
    #[component]
    fn LateKeyHarness() -> Element {
        let mut station = heard(voice(500.0, 1.0), true);
        station.key = vec![(0.0, 0.1), (1.0, 1.1)];
        rsx! {
            BandScope {
                settings: settings(500.0, 1),
                sending: vec![station],
                live: true,
                send_id: 0,
            }
        }
    }

    /// How far the rendered trace swings from the centre line.
    fn drawn_height(html: &str) -> f64 {
        let from = html
            .find("class=\"scope-trace\"")
            .expect("the trace should be on screen");
        let d_at = html[from..].find("d=\"").expect("the trace needs a path") + from + 3;
        let end = html[d_at..].find('"').unwrap() + d_at;
        let mid = TRACE_H / 2.0;
        html[d_at..end]
            .split(['M', 'L'])
            .filter_map(|point| point.split(',').nth(1))
            .filter_map(|y| y.trim().parse::<f64>().ok())
            .fold(0.0_f64, |acc, y| acc.max((y - mid).abs()))
    }

    /// How far the drawn trace swings away from the centre line.
    fn trace_height(settings: &TrainingSettings, sending: &[Heard], elapsed_ms: u64) -> f64 {
        let mid = TRACE_H / 2.0;
        trace_path(settings, sending, true, elapsed_ms)
            .split(['M', 'L'])
            .filter_map(|point| point.split(',').nth(1))
            .filter_map(|y| y.trim().parse::<f64>().ok())
            .fold(0.0_f64, |acc, y| acc.max((y - mid).abs()))
    }

    fn heard(voice: StationVoice, wanted: bool) -> Heard {
        Heard {
            voice,
            wanted,
            // Key down for a second straight, so amplitude_at is simple to
            // reason about in the tests that care about it.
            key: vec![(0.0, 1.0)],
            rise_sec: 0.005,
        }
    }

    fn settings(bandwidth: f64, stations: u32) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.band.filter_bandwidth_hz = bandwidth;
        s.band.stations_max = stations;
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        s.clamp()
    }

    /// Nothing is being sent, so there is nothing in the passband — but the
    /// filter is still the filter, and it is still worth looking at.
    #[test]
    fn the_filter_is_drawn_even_with_nobody_calling() {
        let geometry = scope_geometry(&settings(500.0, 1), &[]);
        assert!(geometry.blips.is_empty());
        assert!(geometry.curve.starts_with('M'));
        assert!(geometry.skirt.ends_with('Z'));
    }

    /// Every station on screen, and the one you want drawn as the one you want.
    #[test]
    fn everyone_calling_gets_a_mark() {
        let sending = [
            heard(voice(600.0, 1.0), true),
            heard(voice(760.0, 0.4), false),
            heard(voice(440.0, 0.3), false),
        ];
        let geometry = scope_geometry(&settings(500.0, 3), &sending);
        assert_eq!(geometry.blips.len(), 3);
        assert_eq!(geometry.blips.iter().filter(|b| b.wanted).count(), 1);

        // Drawn at the height you hear them, so the one you want stands tallest.
        let wanted = geometry.blips.iter().find(|b| b.wanted).unwrap();
        for other in geometry.blips.iter().filter(|b| !b.wanted) {
            assert!(
                other.top_y > wanted.top_y,
                "a caller was drawn above the station you want"
            );
        }
    }

    /// Off-pitch is off to the side, in the direction you would expect.
    #[test]
    fn a_higher_pitch_is_drawn_further_right() {
        let sending = [
            heard(voice(600.0, 1.0), true),
            heard(voice(760.0, 0.5), false),
        ];
        let geometry = scope_geometry(&settings(500.0, 2), &sending);
        let wanted = geometry.blips.iter().find(|b| b.wanted).unwrap();
        let other = geometry.blips.iter().find(|b| !b.wanted).unwrap();
        assert!(other.x > wanted.x);
        assert!(
            (wanted.x - geometry.centre_x).abs() < 0.001,
            "tuned dead on"
        );
    }

    /// The window drawn across the face is the bandwidth the control names,
    /// and narrowing the control narrows what you see.
    #[test]
    fn the_window_narrows_with_the_filter() {
        let wide = scope_geometry(&settings(1_000.0, 1), &[]);
        let narrow = scope_geometry(&settings(200.0, 1), &[]);
        for geometry in [&wide, &narrow] {
            assert!(geometry.edge_low_x < geometry.centre_x);
            assert!(geometry.edge_high_x > geometry.centre_x);
        }
        let width = |g: &ScopeGeometry| {
            (g.edge_high_x - g.edge_low_x) / (VIEW_W - 2.0 * PAD_X) * (g.span_high - g.span_low)
        };
        assert!((width(&wide) - 1_000.0).abs() < 1.0);
        assert!((width(&narrow) - 200.0).abs() < 1.0);
    }

    /// Whatever the settings, the drawing stays on the face.
    #[test]
    fn nothing_is_ever_drawn_off_the_face() {
        for bandwidth in [150.0, 500.0, 2_000.0] {
            for stations in [1u32, 5] {
                for (low, high) in [(200.0, 200.0), (400.0, 600.0), (200.0, 1_200.0)] {
                    let mut s = settings(bandwidth, stations);
                    s.band.side_tone_min = low;
                    s.band.side_tone_max = high;
                    let s = s.clamp();
                    let sending = [
                        heard(voice(s.side_tone_center(), 1.0), true),
                        heard(
                            voice(s.side_tone_center() + s.band.pileup_spread_hz, 0.4),
                            false,
                        ),
                    ];
                    let geometry = scope_geometry(&s, &sending);
                    for blip in &geometry.blips {
                        assert!(
                            blip.x >= PAD_X - 0.001 && blip.x <= VIEW_W - PAD_X + 0.001,
                            "{bandwidth} Hz, {low}..{high}: a station at x {:.1} ran off the face",
                            blip.x
                        );
                        assert!(blip.top_y >= 0.0 && blip.top_y <= BASE_Y + 0.001);
                    }
                }
            }
        }
    }

    /// The trace has to be alive, or it is a picture of a scope. Two frames
    /// apart it is a different drawing.
    #[test]
    fn the_trace_is_redrawn_every_frame() {
        let s = settings(500.0, 1);
        let sending = [heard(voice(600.0, 1.0), true)];
        let first = trace_path(&s, &sending, true, 40);
        let second = trace_path(&s, &sending, true, 80);
        assert!(!first.is_empty() && first.starts_with('M'));
        assert_ne!(first, second, "the trace stood still between frames");
    }

    /// And alive *because of the audio*: the face follows the keying, so it
    /// stands up while the key is down and drops to the hiss in the gaps.
    #[test]
    fn the_trace_follows_the_keying() {
        let s = settings(500.0, 1);
        let mut station = heard(voice(500.0, 1.0), true);
        // Key down for the first tenth of a second, then a gap.
        station.key = vec![(0.0, 0.1)];
        let sending = [station];

        // Keyed for the first 100 ms: 50 ms in is mid-element, 500 ms is past
        // the end of it and quiet.
        let keyed = trace_height(&s, &sending, 50);
        let gap = trace_height(&s, &sending, 500);
        assert!(
            keyed > gap * 2.0,
            "key down drew {keyed:.1} against {gap:.1} in the gap — the trace \
             is not following the audio"
        );
    }

    /// Between sends the receiver is still open, and an open receiver hisses.
    /// A flat line would say the radio was off.
    #[test]
    fn an_idle_receiver_still_shows_its_own_noise() {
        let s = settings(500.0, 1);
        let path = trace_path(&s, &[], false, 3);
        let mid = TRACE_H / 2.0;
        let moved = path
            .split(['M', 'L'])
            .filter_map(|point| point.split(',').nth(1))
            .filter_map(|y| y.trim().parse::<f64>().ok())
            .any(|y| (y - mid).abs() > 0.2);
        assert!(moved, "an idle receiver drew a dead flat line");
    }

    /// The keying drives the height, but its *shape* is never drawn. The face
    /// never spans a whole element, at any speed the app allows, so there is
    /// no dot or dash on it to read — what is drawn is the tone.
    ///
    /// At the trainer's usual speeds the margin is wide: 25 WPM puts a dit at
    /// 48 ms against an 8 ms face. It narrows at the 80 WPM ceiling, where a
    /// dit is 15 ms, and that is the case this pins.
    #[test]
    fn the_face_never_spans_a_whole_element() {
        let dit_at = |wpm: f64| 1.2 / wpm;
        assert!(
            SCOPE_WINDOW_SEC < dit_at(FASTEST_WPM),
            "the face spans {SCOPE_WINDOW_SEC}s against a {}s dit",
            dit_at(FASTEST_WPM)
        );
        // And comfortably so wherever anyone actually practises.
        assert!(SCOPE_WINDOW_SEC * 5.0 < dit_at(25.0));
    }

    /// The ceiling `TrainingSettings::clamp` puts on character speed. Pinned
    /// here because the face has to stay shorter than a dit at that speed.
    const FASTEST_WPM: f64 = 80.0;

    #[test]
    fn the_speed_ceiling_is_still_what_the_scope_assumes() {
        let mut s = TrainingSettings::default();
        s.playback.link_char_wpm = false;
        s.playback.char_wpm_min = 500.0;
        s.playback.char_wpm_max = 500.0;
        assert_eq!(s.clamp().playback.char_wpm_max, FASTEST_WPM);
    }

    /// The bug that made the scope useless after the first group: the trace
    /// ran off a clock that started when the screen did, while the keying it
    /// is drawn from starts again at zero on every send. A group or two in,
    /// every station read as silent and the face showed nothing but hiss for
    /// the rest of the session.
    ///
    /// This has to be driven through the component, because the timebase is
    /// the component's: `trace_path` only ever sees a frame within one send.
    #[test]
    fn a_later_send_gets_a_live_trace_too() {
        use crate::testing::{run, Ui};

        run(|| async {
            let mut ui = Ui::new(ScopeHarness, ());

            // Into the first send, while the key is down.
            ui.advance(u64::from(FRAME_MS) + 5).await;
            let first_send = drawn_height(&ui.html());

            // Well past the end of the keying: hiss only, as it should be.
            ui.advance(3_000).await;
            let between = drawn_height(&ui.html());
            assert!(
                first_send > between * 2.0,
                "the first send drew {first_send:.1} against {between:.1} after it"
            );

            // The next send, at the same point in its keying, has to come back
            // just as strongly. Before the fix it stayed at the noise floor.
            ui.click(&crate::ui::widgets::control_id("btn", "next send"));
            ui.advance(u64::from(FRAME_MS) + 5).await;
            let later_send = drawn_height(&ui.html());
            assert!(
                later_send > between * 2.0,
                "a later send drew {later_send:.1}, barely above the {between:.1} \
                 noise floor — the scope stopped following the audio"
            );
        });
    }

    /// A second into a send, the face is drawing the second element — not
    /// still somewhere in the quiet stretch before it.
    ///
    /// Note what this does *not* prove. The reason the face reads a clock
    /// rather than counting frames is that a frame costs its 40 ms wait plus
    /// the render, so a tally runs slow and the trace slides further behind
    /// the sound the longer a send goes on. Under the paused test clock a
    /// sleep is exact and nothing else consumes time, so a tally and the clock
    /// agree perfectly and this test passes either way — the drift is a
    /// property of real time and cannot be reproduced here. What is pinned is
    /// the observable part: at a given moment in the send, the right thing is
    /// on the face.
    #[test]
    fn the_face_is_on_the_right_moment_of_the_send() {
        use crate::testing::{run, Ui};

        run(|| async {
            let mut ui = Ui::new(LateKeyHarness, ());

            // Right at the start, while the first element is sounding.
            ui.advance(u64::from(FRAME_MS) + 5).await;
            let early = drawn_height(&ui.html());

            // The quiet stretch in the middle.
            ui.advance(500).await;
            let quiet = drawn_height(&ui.html());
            assert!(early > quiet * 2.0, "the first element should stand up");

            // A full second in, where the second element sounds. A trace
            // running off a frame tally would still be somewhere in the quiet
            // stretch by now and would draw nothing.
            ui.advance(560).await;
            let late = drawn_height(&ui.html());
            assert!(
                late > quiet * 2.0,
                "a second into the send the trace drew {late:.1}, barely over \
                 the {quiet:.1} noise floor — the face is behind the sound"
            );
        });
    }
}
