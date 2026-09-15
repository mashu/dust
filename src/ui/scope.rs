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
use cw_core::timing::StationVoice;
use cw_core::TrainingSettings;
use dioxus::prelude::*;

const VIEW_W: f64 = 320.0;
const VIEW_H: f64 = 96.0;
const PAD_X: f64 = 8.0;
const BASE_Y: f64 = 80.0;
const AMP: f64 = 62.0;

/// A station the receiver is hearing, as far as the scope is concerned.
///
/// Deliberately a pitch and a strength and nothing else. What a station is
/// *sending* has no business on the screen you are copying on, so the text
/// cannot reach here to be drawn by accident.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heard {
    pub voice: StationVoice,
    /// The station being copied, as opposed to somebody calling across it.
    pub wanted: bool,
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

/// The receiver's passband with whoever is in it right now.
#[component]
pub fn BandScope(settings: TrainingSettings, sending: Vec<Heard>, live: bool) -> Element {
    let geometry = scope_geometry(&settings, &sending);
    let callers = geometry.blips.len();
    let caption = if !live {
        "Between sends".to_string()
    } else if callers > 1 {
        format!("{callers} stations in the passband")
    } else {
        "One station in the passband".to_string()
    };
    rsx! {
        div { class: if live { "scope live" } else { "scope" },
            div { class: "row-between scope-head",
                span { class: "scope-title", "Passband" }
                span { class: "scope-read", "{settings.band.filter_bandwidth_hz:.0} Hz wide" }
            }
            svg {
                class: "scope-face",
                view_box: "0 0 {VIEW_W} {VIEW_H}",
                preserve_aspect_ratio: "none",
                role: "img",
                "aria-label": "{caption}",
                defs {
                    linearGradient { id: "dust-scope-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                        stop { offset: "0%", stop_color: "var(--copper)", stop_opacity: "0.26" }
                        stop { offset: "100%", stop_color: "var(--copper)", stop_opacity: "0.02" }
                    }
                }
                // The bandwidth the control names, marked on the shape it makes.
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
        }
    }

    fn heard(voice: StationVoice, wanted: bool) -> Heard {
        Heard { voice, wanted }
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
}
