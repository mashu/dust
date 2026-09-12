//! Keying-envelope scope: draws the exact attack/decay shape the audio backends
//! apply, and sends short test samples with the current settings.

use cw_core::{envelope_shape, EnvelopeShape, TrainingSettings};
use dioxus::prelude::*;

use crate::ui::widgets::{pretty_number, Icon, SliderField};

const VIEW_W: f64 = 320.0;
const VIEW_H: f64 = 120.0;
const PAD_X: f64 = 10.0;
const MID_Y: f64 = 62.0;
const AMP: f64 = 44.0;

struct Chip {
    label: &'static str,
    text: &'static str,
    code: &'static str,
}

const CHIPS: &[Chip] = &[
    Chip {
        label: "Dit",
        text: "E",
        code: "·",
    },
    Chip {
        label: "Dah",
        text: "T",
        code: "−",
    },
    Chip {
        label: "Di-dah",
        text: "A",
        code: "·−",
    },
    Chip {
        label: "Fast dits",
        text: "5",
        code: "·····",
    },
    Chip {
        label: "CQ",
        text: "CQ",
        code: "−·−· −−·−",
    },
];

fn x_at(t_sec: f64, span_sec: f64) -> f64 {
    PAD_X + (t_sec / span_sec.max(1e-9)) * (VIEW_W - 2.0 * PAD_X)
}

/// Symmetric scope trace: the envelope above the centre line and mirrored below.
fn envelope_path(shape: &EnvelopeShape) -> String {
    if shape.points.len() < 2 {
        return String::new();
    }
    let span = shape.total_sec;
    let mut top = String::new();
    let mut bottom = String::new();
    for (i, point) in shape.points.iter().enumerate() {
        let x = x_at(point.t_sec, span);
        let dy = point.gain * AMP;
        if i == 0 {
            top.push_str(&format!("M{x:.2},{:.2}", MID_Y - dy));
        } else {
            top.push_str(&format!(" L{x:.2},{:.2}", MID_Y - dy));
        }
    }
    for point in shape.points.iter().rev() {
        let x = x_at(point.t_sec, span);
        let dy = point.gain * AMP;
        bottom.push_str(&format!(" L{x:.2},{:.2}", MID_Y + dy));
    }
    format!("{top}{bottom} Z")
}

#[component]
pub fn EnvelopeCard(
    settings: Signal<TrainingSettings>,
    sample_playing: Option<String>,
    on_play: EventHandler<String>,
    on_stop: EventHandler<()>,
) -> Element {
    let s = settings();
    let shape = envelope_shape(&s);
    let path = envelope_path(&shape);
    let span = shape.total_sec;
    let rise_w = (x_at(shape.rise_sec.min(shape.dot_sec), span) - PAD_X).max(0.0);
    let dit_share = (shape.dot_sec * 2.0 / span * 100.0).round();
    let dot_ms = (shape.dot_sec * 1000.0).round();
    let sharpness = if shape.smoothing >= 0.75 {
        "Soft — rounded edges, no key clicks"
    } else if shape.smoothing >= 0.35 {
        "Balanced — some snap, little click"
    } else {
        "Hard — crisp edges, clicks on fast keying"
    };
    let rise_warning = shape.rise_share_of_dit > 0.45;
    let playing = sample_playing.clone();
    rsx! {
        div { class: "card stack-sm",
            div { class: "card-head",
                div { class: "card-head-main",
                    span { class: "card-icon", Icon { name: "envelope" } }
                    div {
                        h3 { class: "card-title", "Keying envelope" }
                        p { class: "card-note",
                            "A dit and a dah at {shape.wpm as u32} WPM · dit {dot_ms} ms"
                        }
                    }
                }
            }
            div { class: "envelope-stage",
                svg {
                    class: "envelope-svg",
                    view_box: "0 0 320 120",
                    preserve_aspect_ratio: "none",
                    defs {
                        linearGradient { id: "dust-env-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                            stop { offset: "0%", style: "stop-color: var(--copper-lift); stop-opacity: 0.9;" }
                            stop { offset: "50%", style: "stop-color: var(--copper); stop-opacity: 0.55;" }
                            stop { offset: "100%", style: "stop-color: var(--copper-lift); stop-opacity: 0.9;" }
                        }
                    }
                    line {
                        x1: "{PAD_X}",
                        y1: "{MID_Y}",
                        x2: "{VIEW_W - PAD_X}",
                        y2: "{MID_Y}",
                        stroke: "var(--line-strong)",
                        stroke_width: "1",
                        stroke_dasharray: "3 4",
                    }
                    if rise_w > 0.5 {
                        rect {
                            x: "{PAD_X}",
                            y: "10",
                            width: "{rise_w}",
                            height: "{VIEW_H - 30.0}",
                            fill: "var(--info)",
                            opacity: "0.14",
                        }
                    }
                    path {
                        d: "{path}",
                        fill: "url(#dust-env-fill)",
                        stroke: "var(--copper-deep)",
                        stroke_width: "1.3",
                        stroke_linejoin: "round",
                    }
                }
                div { class: "envelope-axis", "aria-hidden": "true",
                    span { style: "width: {dit_share}%;", "dit" }
                    span { "dah" }
                }
                div { class: "envelope-legend",
                    span { class: "legend-item",
                        span { class: "legend-swatch" }
                        "Keyed tone"
                    }
                    span { class: "legend-item",
                        span { class: "legend-swatch rise" }
                        "Rise window {pretty_number(s.band.steepness)} ms"
                    }
                    span { class: "legend-item", "{sharpness}" }
                }
            }
            if rise_warning {
                p { class: "muted", style: "margin: 0; color: var(--warn);",
                    "The ramp covers most of a dit at this speed — shorten the rise time or slow the sending."
                }
            }
            div { class: "field-grid",
                SliderField {
                    label: "Rise time".to_string(),
                    value_label: format!("{} ms", pretty_number(s.band.steepness)),
                    value: s.band.steepness,
                    min: 1.0,
                    max: 40.0,
                    step: 1.0,
                    disabled: false,
                    onchange: move |v| settings.write().band.steepness = v,
                }
                SliderField {
                    label: "Edge smoothing".to_string(),
                    value_label: format!("{:.0}%", s.band.envelope_smoothing * 100.0),
                    value: s.band.envelope_smoothing,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                    disabled: false,
                    onchange: move |v| settings.write().band.envelope_smoothing = v,
                }
            }
            div { class: "row-between",
                span { class: "eyebrow", "Test it" }
                if playing.is_some() {
                    button { class: "btn btn-secondary btn-sm", onclick: move |_| on_stop.call(()),
                        Icon { name: "stop" }
                        "Stop"
                    }
                }
            }
            div { class: "test-chips",
                for chip in CHIPS.iter() {
                    {
                        let text = chip.text.to_string();
                        let is_playing = playing.as_deref() == Some(chip.text);
                        rsx! {
                            button {
                                class: if is_playing { "test-chip playing" } else { "test-chip" },
                                onclick: move |_| on_play.call(text.clone()),
                                Icon { name: if is_playing { "stop" } else { "play" } }
                                "{chip.label}"
                                span { class: "code", "{chip.code}" }
                            }
                        }
                    }
                }
            }
            p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                "Samples use your current tone, speed and band settings."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{envelope_path, x_at, PAD_X, VIEW_W};
    use cw_core::{envelope_shape, EnvelopePoint, EnvelopeShape, TrainingSettings};

    fn shape() -> EnvelopeShape {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_max = 25.0;
        settings.band.steepness = 10.0;
        settings.band.envelope_smoothing = 0.75;
        envelope_shape(&settings)
    }

    #[test]
    fn time_maps_across_the_padded_width() {
        let total = 0.24;
        assert_eq!(x_at(0.0, total), PAD_X);
        assert_eq!(x_at(total, total), VIEW_W - PAD_X);
        assert!(x_at(total / 2.0, total) > PAD_X);
        // A degenerate span must not divide by zero.
        assert!(x_at(0.0, 0.0).is_finite());
    }

    #[test]
    fn the_trace_is_a_closed_mirrored_shape() {
        let shape = shape();
        let path = envelope_path(&shape);
        assert!(path.starts_with('M'));
        assert!(path.ends_with(" Z"));
        // Every sample appears twice: once above the centre line, once below.
        let commands = path.matches(" L").count() + 1;
        assert_eq!(commands, shape.points.len() * 2);
    }

    #[test]
    fn too_few_points_draw_nothing() {
        let mut empty = shape();
        empty.points.clear();
        assert!(envelope_path(&empty).is_empty());
        empty.points.push(EnvelopePoint {
            t_sec: 0.0,
            gain: 1.0,
        });
        assert!(envelope_path(&empty).is_empty());
    }
}
