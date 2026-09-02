use cw_core::{QrmProfile, TrainingSettings};
use dioxus::prelude::*;

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
        div { class: "card stack",
            div { class: "row", style: "justify-content: space-between;",
                div { class: "tiny", "Band conditions" }
                div { class: "row",
                    if previewing {
                        button {
                            class: "btn btn-secondary",
                            style: "padding: 0.35rem 0.75rem;",
                            onclick: move |_| on_stop.call(()),
                            "Stop preview"
                        }
                    } else {
                        button {
                            class: "btn btn-primary",
                            style: "padding: 0.35rem 0.75rem;",
                            onclick: move |_| on_preview.call(()),
                            "Live preview"
                        }
                    }
                    button {
                        class: "btn btn-ghost",
                        style: "padding: 0.25rem 0.6rem;",
                        onclick: move |_| show_help.set(!show_help()),
                        if show_help() { "Hide help" } else { "What is this?" }
                    }
                }
            }
            if previewing {
                p { class: "muted", "Looping “CQ” with the current QSB/QRN/QRM mix. Change sliders to hear them live." }
            }
            if show_help() {
                div { class: "tips",
                    p { class: "muted", style: "margin: 0;",
                        "QSB slowly fades the Morse signal. QRN is atmospheric static in the CW passband. Receiver background is narrow-filter hiss and ringing."
                    }
                }
            }
            ToggleRow {
                label: "QSB fading",
                description: "Slow gain changes on the Morse signal only.",
                enabled: s.band.qsb_enabled,
                onchange: move |on| settings.write().band.qsb_enabled = on,
            }
            div { class: "field-grid",
                RangeField {
                    label: format!("Depth ({:.0}%)", s.band.qsb_depth * 100.0),
                    value: s.band.qsb_depth,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                    disabled: !s.band.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_depth = v,
                }
                RangeField {
                    label: format!("Rate ({:.2} Hz)", s.band.qsb_rate_hz),
                    value: s.band.qsb_rate_hz,
                    min: 0.03,
                    max: 1.5,
                    step: 0.01,
                    disabled: !s.band.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_rate_hz = v,
                }
            }
            ToggleRow {
                label: "QRN static",
                description: "Atmospheric noise inside the CW passband.",
                enabled: s.band.qrn_enabled,
                onchange: move |on| settings.write().band.qrn_enabled = on,
            }
            RangeField {
                label: format!("Intensity ({:.0}%)", s.band.qrn_level * 100.0),
                value: s.band.qrn_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !s.band.qrn_enabled,
                onchange: move |v| settings.write().band.qrn_level = v,
            }
            ToggleRow {
                label: "Receiver background",
                description: "Narrow-filter hiss, ringing, and passband breathing.",
                enabled: s.band.qrm_enabled,
                onchange: move |on| settings.write().band.qrm_enabled = on,
            }
            div { class: "field-grid",
                RangeField {
                    label: format!("Intensity ({:.0}%)", s.band.qrm_level * 100.0),
                    value: s.band.qrm_level,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                    disabled: !s.band.qrm_enabled,
                    onchange: move |v| settings.write().band.qrm_level = v,
                }
                div { class: "field",
                    label { "Profile" }
                    div { class: "mode-pills",
                        ModePill { label: "Whistle", active: s.band.qrm_profile == QrmProfile::Whistle, disabled: !s.band.qrm_enabled, onclick: move |_| settings.write().band.qrm_profile = QrmProfile::Whistle }
                        ModePill { label: "Ringing", active: s.band.qrm_profile == QrmProfile::Ringing, disabled: !s.band.qrm_enabled, onclick: move |_| settings.write().band.qrm_profile = QrmProfile::Ringing }
                        ModePill { label: "Mixed", active: s.band.qrm_profile == QrmProfile::Mixed, disabled: !s.band.qrm_enabled, onclick: move |_| settings.write().band.qrm_profile = QrmProfile::Mixed }
                    }
                }
            }
            button {
                class: "advanced-toggle",
                onclick: move |_| show_advanced.set(!show_advanced()),
                span {
                    div { class: "tiny", "Advanced receiver tuning" }
                    div { class: "muted",
                        "Gain {s.band.receiver_background_gain:.0}× · Q {s.band.receiver_background_resonance:.0} · offset {s.band.receiver_background_offset_hz:.0} Hz"
                    }
                }
                span { class: "muted", if show_advanced() { "▼" } else { "▶" } }
            }
            if show_advanced() {
                div { class: "field-grid",
                    RangeField {
                        label: format!("Model gain ({:.1}×)", s.band.receiver_background_gain),
                        value: s.band.receiver_background_gain,
                        min: 0.0,
                        max: 20.0,
                        step: 0.1,
                        disabled: !s.band.qrm_enabled,
                        onchange: move |v| settings.write().band.receiver_background_gain = v,
                    }
                    RangeField {
                        label: format!("Excitation ({:.0}/s)", s.band.receiver_background_excitation_rate),
                        value: s.band.receiver_background_excitation_rate,
                        min: 0.1,
                        max: 500.0,
                        step: 1.0,
                        disabled: !s.band.qrm_enabled,
                        onchange: move |v| settings.write().band.receiver_background_excitation_rate = v,
                    }
                    RangeField {
                        label: format!("Resonance Q ({:.0})", s.band.receiver_background_resonance),
                        value: s.band.receiver_background_resonance,
                        min: 0.5,
                        max: 240.0,
                        step: 0.5,
                        disabled: !s.band.qrm_enabled,
                        onchange: move |v| settings.write().band.receiver_background_resonance = v,
                    }
                    RangeField {
                        label: format!("Decay ({:.3})", s.band.receiver_background_decay),
                        value: s.band.receiver_background_decay,
                        min: 0.5,
                        max: 0.9999,
                        step: 0.0001,
                        disabled: !s.band.qrm_enabled,
                        onchange: move |v| settings.write().band.receiver_background_decay = v,
                    }
                    RangeField {
                        label: format!("Filter offset ({:.0} Hz)", s.band.receiver_background_offset_hz),
                        value: s.band.receiver_background_offset_hz,
                        min: -1000.0,
                        max: 1000.0,
                        step: 5.0,
                        disabled: !s.band.qrm_enabled,
                        onchange: move |v| settings.write().band.receiver_background_offset_hz = v,
                    }
                    RangeField {
                        label: format!("Wobble depth ({:.0} Hz)", s.band.receiver_background_offset_mod_depth_hz),
                        value: s.band.receiver_background_offset_mod_depth_hz,
                        min: 0.0,
                        max: 1000.0,
                        step: 5.0,
                        disabled: !s.band.qrm_enabled,
                        onchange: move |v| settings.write().band.receiver_background_offset_mod_depth_hz = v,
                    }
                    RangeField {
                        label: format!("Wobble rate ({:.2} Hz)", s.band.receiver_background_offset_mod_rate_hz),
                        value: s.band.receiver_background_offset_mod_rate_hz,
                        min: 0.0,
                        max: 20.0,
                        step: 0.01,
                        disabled: !s.band.qrm_enabled,
                        onchange: move |v| settings.write().band.receiver_background_offset_mod_rate_hz = v,
                    }
                }
            }
        }
    }
}

#[component]
fn ToggleRow(
    label: &'static str,
    description: &'static str,
    enabled: bool,
    onchange: EventHandler<bool>,
) -> Element {
    rsx! {
        label { class: "toggle-row",
            div {
                div { style: "font-weight: 700;", "{label}" }
                p { class: "muted", style: "margin: 0.15rem 0 0;", "{description}" }
            }
            input {
                r#type: "checkbox",
                checked: enabled,
                onchange: move |e| onchange.call(e.checked()),
            }
        }
    }
}

#[component]
fn ModePill(
    label: &'static str,
    active: bool,
    disabled: bool,
    onclick: EventHandler<()>,
) -> Element {
    rsx! {
        button {
            class: if active { "pill active" } else { "pill" },
            disabled: disabled,
            onclick: move |_| onclick.call(()),
            "{label}"
        }
    }
}

#[component]
fn RangeField(
    label: String,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    disabled: bool,
    onchange: EventHandler<f64>,
) -> Element {
    rsx! {
        div { class: "field",
            label { "{label}" }
            input {
                r#type: "range",
                min: "{min}",
                max: "{max}",
                step: "{step}",
                value: "{value}",
                disabled: disabled,
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<f64>() {
                        onchange.call(v);
                    }
                }
            }
        }
    }
}
