use cw_core::band::heard_snr_db;
use cw_core::{
    FILTER_BANDWIDTH_MAX, FILTER_BANDWIDTH_MIN, FilterShape, PILEUP_LEVEL_MAX_DB,
    PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN, RangeSetting, STATIONS_MAX,
    STATIONS_MIN, SettingsSection, TrainingSettings,
};
use dioxus::prelude::*;

use crate::ui::widgets::{
    DISCLOSURE, Icon, LinkedRange, SectionReset, Seg, SliderField, Switch, control_id,
};

#[component]
pub fn BandConditionsCard(
    settings: Signal<TrainingSettings>,
    previewing: bool,
    on_preview: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    let s = settings();
    let mut show_help = use_signal(|| false);
    // As heard through the filter you have set, so narrowing it visibly buys
    // signal-to-noise — which is the whole reason to reach for it.
    let noise_reading = if s.band.noise_level > 0.0 {
        format!(
            "S/N {:.0} dB",
            heard_snr_db(s.band.noise_level, s.band.filter_bandwidth_hz)
        )
    } else {
        "Off".to_string()
    };
    rsx! {
        div { class: "card stack-sm",
            div { class: "card-head",
                div { class: "card-head-main",
                    span { class: "card-icon", Icon { name: "waves" } }
                    div {
                        h3 { class: "card-title", "Band conditions" }
                        p { class: "card-note", "Fading, static, noise and the receiver's filter" }
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
                    SectionReset {
                        label: "band conditions".to_string(),
                        on_reset: move |()| {
                            settings.write().reset_section(SettingsSection::BandConditions);
                        },
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
                        "Everything you hear passes through one CW filter, the way it does in a real receiver: the band noise is a steady hiss pitched at the filter, and static and every dit ring in it for a moment. QSB slowly fades the signal. QRN is lightning: the crackle of distant storms and the crash of a near one, with the receiver's AGC ducking the band behind it. Turn the noise up and the band reaches the signal."
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
            div { class: "field",
                span { class: "field-label", "Filter shape" }
                div { class: "segmented",
                    Seg {
                        label: "Sharp".to_string(),
                        id: "seg-filter-sharp".to_string(),
                        active: s.band.filter_shape == FilterShape::Sharp,
                        onclick: move |_| settings.write().band.filter_shape = FilterShape::Sharp,
                    }
                    Seg {
                        label: "Soft".to_string(),
                        id: "seg-filter-soft".to_string(),
                        active: s.band.filter_shape == FilterShape::Soft,
                        onclick: move |_| settings.write().band.filter_shape = FilterShape::Soft,
                    }
                }
            }
            p { class: "muted", style: "margin: 0;",
                "Everything you hear comes through this. Narrow it and less noise gets in and the filter rings longer, but a station off your pitch fades with it. Sharp is a crystal filter's steep skirts and ringing; soft rounds them off and barely rings."
            }
            LinkedRange {
                label: "Stations calling".to_string(),
                unit: "at once".to_string(),
                min_value: f64::from(s.band.stations_min),
                max_value: f64::from(s.band.stations_max),
                linked: s.band.stations_min == s.band.stations_max,
                min_bound: f64::from(STATIONS_MIN),
                max_bound: f64::from(STATIONS_MAX),
                step: 1.0,
                hint: "How many call at once, drawn afresh for each group. Hold both ends together for the same number every time, or open them up so you never know what you are walking into.".to_string(),
                on_min: move |v| settings.write().set_range_min(RangeSetting::Stations, v),
                on_max: move |v| settings.write().set_range_max(RangeSetting::Stations, v),
                on_link: move |on| settings.write().set_range_linked(RangeSetting::Stations, on),
            }
            if s.band.stations_max > STATIONS_MIN {
                p { class: "muted", style: "margin: 0;",
                    "The others sit either side of the one you want, further out and a good way under it, and only ever where your filter passes them — copy the strongest, and narrow the filter until the rest drop away."
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
                description: "Lightning: distant crackle and the crash of a near storm.".to_string(),
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
                title: "Band activity".to_string(),
                description: "Other stations: faint CW at other pitches, coming and going.".to_string(),
                checked: s.band.activity_enabled,
                onchange: move |on| settings.write().band.activity_enabled = on,
            }
            SliderField {
                label: "Busy".to_string(),
                id: "slider-activity-level".to_string(),
                value_label: activity_reading(s.band.activity_level),
                value: s.band.activity_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !s.band.activity_enabled,
                onchange: move |v| settings.write().band.activity_level = v,
            }
            Switch {
                title: "Band noise".to_string(),
                description: "The steady hiss of the band, through your filter.".to_string(),
                checked: s.band.noise_enabled,
                onchange: move |on| settings.write().band.noise_enabled = on,
            }
            SliderField {
                label: "Level".to_string(),
                id: "slider-noise-level".to_string(),
                value_label: noise_reading,
                value: s.band.noise_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !s.band.noise_enabled,
                onchange: move |v| settings.write().band.noise_level = v,
            }
            Switch {
                title: "AGC".to_string(),
                description: "The receiver rides its gain: the band ducks under a strong station and behind a crash.".to_string(),
                checked: s.band.agc_enabled,
                onchange: move |on| settings.write().band.agc_enabled = on,
            }
        }
    }
}

/// The busy-ness control, in words a ham would use.
fn activity_reading(level: f64) -> String {
    match level {
        l if l <= 0.0 => "Off",
        l if l < 0.3 => "Quiet",
        l if l < 0.7 => "Normal",
        l if l < 0.9 => "Busy",
        _ => "Contest",
    }
    .to_string()
}
