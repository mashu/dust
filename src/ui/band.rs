use cw_core::{
    ReceiverProfile, TrainingSettings, FILTER_BANDWIDTH_MAX, FILTER_BANDWIDTH_MIN,
    PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN, STATIONS_MAX,
    STATIONS_MIN,
};
use dioxus::prelude::*;

use crate::ui::widgets::{control_id, Icon, NumberField, Seg, SliderField, Switch, DISCLOSURE};

#[component]
pub fn BandConditionsCard(
    settings: Signal<TrainingSettings>,
    previewing: bool,
    on_preview: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    let s = settings();
    let mut show_help = use_signal(|| false);
    let mut show_advanced = use_signal(|| false);
    rsx! {
        div { class: "card stack-sm",
            div { class: "card-head",
                div { class: "card-head-main",
                    span { class: "card-icon", Icon { name: "waves" } }
                    div {
                        h3 { class: "card-title", "Band conditions" }
                        p { class: "card-note", "Fading, static and receiver character" }
                    }
                }
                div { class: "card-tools",
                    if previewing {
                        button {
                            id: control_id("btn", "band stop"),
                            class: "btn btn-secondary btn-sm",
                            onclick: move |_| on_stop.call(()),
                            Icon { name: "stop" }
                            "Stop"
                        }
                    } else {
                        button {
                            id: control_id("btn", "band preview"),
                            class: "btn btn-primary btn-sm",
                            onclick: move |_| on_preview.call(()),
                            Icon { name: "play" }
                            "Live preview"
                        }
                    }
                    button {
                        id: control_id(DISCLOSURE, "band help"),
                        class: if show_help() { "icon-btn open" } else { "icon-btn" },
                        title: "What is this?",
                        aria_label: "What is this?",
                        onclick: move |_| show_help.set(!show_help()),
                        Icon { name: if show_help() { "x" } else { "target" } }
                    }
                }
            }
            if previewing {
                p { class: "muted", style: "margin: 0;",
                    "Looping “CQ” with the current mix. Move a slider and you hear it immediately."
                }
            }
            if show_help() {
                div { class: "tips",
                    span { class: "tips-mark", "QRx" }
                    p { class: "muted", style: "margin: 0;",
                        "QSB slowly fades the signal. QRN is lightning: sharp crashes through the CW passband, not a steady hiss. Receiver background is narrow-filter hiss, ringing and passband breathing. Turn the intensities up and the band reaches the signal."
                    }
                }
            }
            div { class: "field-grid",
                SliderField {
                    label: "Filter width".to_string(),
                    value_label: format!("{:.0} Hz", s.band.filter_bandwidth_hz),
                    value: s.band.filter_bandwidth_hz,
                    min: FILTER_BANDWIDTH_MIN,
                    max: FILTER_BANDWIDTH_MAX,
                    step: 10.0,
                    disabled: false,
                    onchange: move |v| settings.write().band.filter_bandwidth_hz = v,
                }
            }
            p { class: "muted", style: "margin: 0;",
                "Everything you hear comes through this. Narrow it and less static gets in and the filter rings longer, but a station off your pitch fades with it."
            }
            div { class: "field-grid",
                NumberField {
                    label: format!("Stations calling (1–{STATIONS_MAX})"),
                    id: "field-stations".to_string(),
                    value: s.band.stations_max as f64,
                    min: STATIONS_MIN as f64,
                    max: STATIONS_MAX as f64,
                    step: 1.0,
                    onchange: move |v| settings.write().band.stations_max = v as u32,
                }
            }
            if s.band.stations_max > STATIONS_MIN {
                p { class: "muted", style: "margin: 0;",
                    "Up to this many at once, drawn afresh for each group. The others sit either side of the one you want and a good way under it — copy the strongest, and narrow the filter on the rest."
                }
                div { class: "field-grid",
                    SliderField {
                        label: "Pile-up spread".to_string(),
                        value_label: format!("±{:.0} Hz", s.band.pileup_spread_hz),
                        value: s.band.pileup_spread_hz,
                        min: PILEUP_SPREAD_MIN,
                        max: PILEUP_SPREAD_MAX,
                        step: 5.0,
                        disabled: false,
                        onchange: move |v| settings.write().band.pileup_spread_hz = v,
                    }
                    SliderField {
                        label: "Pile-up is weaker by".to_string(),
                        value_label: format!("{:.0} dB", s.band.pileup_level_db),
                        value: s.band.pileup_level_db,
                        min: PILEUP_LEVEL_MIN_DB,
                        max: PILEUP_LEVEL_MAX_DB,
                        step: 1.0,
                        disabled: false,
                        onchange: move |v| settings.write().band.pileup_level_db = v,
                    }
                }
            }
            Switch {
                title: "QSB fading".to_string(),
                description: "Slow gain swells on the Morse signal only.".to_string(),
                checked: s.band.qsb_enabled,
                onchange: move |on| settings.write().band.qsb_enabled = on,
            }
            div { class: "field-grid",
                SliderField {
                    label: "Depth".to_string(),
                    value_label: format!("{:.0}%", s.band.qsb_depth * 100.0),
                    value: s.band.qsb_depth,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                    disabled: !s.band.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_depth = v,
                }
                SliderField {
                    label: "Rate".to_string(),
                    value_label: format!("{:.2} Hz", s.band.qsb_rate_hz),
                    value: s.band.qsb_rate_hz,
                    min: 0.03,
                    max: 1.5,
                    step: 0.01,
                    disabled: !s.band.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_rate_hz = v,
                }
            }
            Switch {
                title: "QRN static".to_string(),
                description: "Crashes of static, the way lightning arrives.".to_string(),
                checked: s.band.qrn_enabled,
                onchange: move |on| settings.write().band.qrn_enabled = on,
            }
            SliderField {
                label: "Intensity".to_string(),
                id: "slider-qrn-intensity".to_string(),
                value_label: format!("{:.0}%", s.band.qrn_level * 100.0),
                value: s.band.qrn_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !s.band.qrn_enabled,
                onchange: move |v| settings.write().band.qrn_level = v,
            }
            Switch {
                title: "Receiver background".to_string(),
                description: "Narrow-filter hiss, ringing and passband breathing.".to_string(),
                checked: s.band.receiver_enabled,
                onchange: move |on| settings.write().band.receiver_enabled = on,
            }
            SliderField {
                label: "Intensity".to_string(),
                id: "slider-receiver-intensity".to_string(),
                value_label: format!("{:.0}%", s.band.receiver_level * 100.0),
                value: s.band.receiver_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !s.band.receiver_enabled,
                onchange: move |v| settings.write().band.receiver_level = v,
            }
            div { class: "field",
                span { class: "field-label", "Filter character" }
                div { class: "segmented",
                    Seg {
                        label: "Whistle".to_string(),
                        id: "seg-receiver-whistle".to_string(),
                        active: s.band.receiver_profile == ReceiverProfile::Whistle,
                        onclick: move |_| settings.write().band.receiver_profile = ReceiverProfile::Whistle,
                    }
                    Seg {
                        label: "Ringing".to_string(),
                        id: "seg-receiver-ringing".to_string(),
                        active: s.band.receiver_profile == ReceiverProfile::Ringing,
                        onclick: move |_| settings.write().band.receiver_profile = ReceiverProfile::Ringing,
                    }
                    Seg {
                        label: "Mixed".to_string(),
                        id: "seg-receiver-mixed".to_string(),
                        active: s.band.receiver_profile == ReceiverProfile::Mixed,
                        onclick: move |_| settings.write().band.receiver_profile = ReceiverProfile::Mixed,
                    }
                }
            }
            button {
                id: control_id(DISCLOSURE, "band advanced"),
                class: if show_advanced() { "advanced-toggle open" } else { "advanced-toggle" },
                onclick: move |_| show_advanced.set(!show_advanced()),
                span {
                    div { class: "tiny", "Advanced receiver tuning" }
                    div { class: "muted",
                        "Gain {s.band.receiver_background_gain:.0}× · Q {s.band.receiver_background_resonance:.0} · offset {s.band.receiver_background_offset_hz:.0} Hz"
                    }
                }
                Icon { name: "chevron" }
            }
            if show_advanced() {
                div { class: "field-grid",
                    SliderField {
                        label: "Model gain".to_string(),
                        value_label: format!("{:.1}×", s.band.receiver_background_gain),
                        value: s.band.receiver_background_gain,
                        min: 0.0,
                        max: 20.0,
                        step: 0.1,
                        disabled: !s.band.receiver_enabled,
                        onchange: move |v| settings.write().band.receiver_background_gain = v,
                    }
                    SliderField {
                        label: "Excitation".to_string(),
                        value_label: format!("{:.0}/s", s.band.receiver_background_excitation_rate),
                        value: s.band.receiver_background_excitation_rate,
                        min: 0.1,
                        max: 500.0,
                        step: 1.0,
                        disabled: !s.band.receiver_enabled,
                        onchange: move |v| settings.write().band.receiver_background_excitation_rate = v,
                    }
                    SliderField {
                        label: "Resonance Q".to_string(),
                        value_label: format!("{:.0}", s.band.receiver_background_resonance),
                        value: s.band.receiver_background_resonance,
                        min: 0.5,
                        max: 240.0,
                        step: 0.5,
                        disabled: !s.band.receiver_enabled,
                        onchange: move |v| settings.write().band.receiver_background_resonance = v,
                    }
                    SliderField {
                        label: "Decay".to_string(),
                        value_label: format!("{:.3}", s.band.receiver_background_decay),
                        value: s.band.receiver_background_decay,
                        min: 0.5,
                        max: 0.9999,
                        step: 0.0001,
                        disabled: !s.band.receiver_enabled,
                        onchange: move |v| settings.write().band.receiver_background_decay = v,
                    }
                    SliderField {
                        label: "Filter offset".to_string(),
                        value_label: format!("{:.0} Hz", s.band.receiver_background_offset_hz),
                        value: s.band.receiver_background_offset_hz,
                        min: -1000.0,
                        max: 1000.0,
                        step: 5.0,
                        disabled: !s.band.receiver_enabled,
                        onchange: move |v| settings.write().band.receiver_background_offset_hz = v,
                    }
                    SliderField {
                        label: "Wobble depth".to_string(),
                        value_label: format!("{:.0} Hz", s.band.receiver_background_offset_mod_depth_hz),
                        value: s.band.receiver_background_offset_mod_depth_hz,
                        min: 0.0,
                        max: 1000.0,
                        step: 5.0,
                        disabled: !s.band.receiver_enabled,
                        onchange: move |v| settings.write().band.receiver_background_offset_mod_depth_hz = v,
                    }
                    SliderField {
                        label: "Wobble rate".to_string(),
                        value_label: format!("{:.2} Hz", s.band.receiver_background_offset_mod_rate_hz),
                        value: s.band.receiver_background_offset_mod_rate_hz,
                        min: 0.0,
                        max: 20.0,
                        step: 0.01,
                        disabled: !s.band.receiver_enabled,
                        onchange: move |v| settings.write().band.receiver_background_offset_mod_rate_hz = v,
                    }
                }
            }
        }
    }
}
