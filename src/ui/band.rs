use cw_core::band::{FilterDesign, heard_snr_db};
use cw_core::{
    BandSettings, CharSetMode, FILTER_WIDTH_MAX, FILTER_WIDTH_MIN, FilterShape,
    PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN,
    QSB_RATE_MAX_HZ, QSB_RATE_MIN_HZ, RangeSetting, STATIONS_MAX, STATIONS_MIN, SettingsSection,
    TrainingSettings,
};
use dioxus::prelude::*;

use crate::ui::widgets::{
    CardHead, DISCLOSURE, Icon, IconName, SectionReset, Seg, SettingsRange, SliderField, Switch,
    control_id,
};

/// The filter shapes, in the order the control offers them.
const FILTER_SHAPES: [(FilterShape, &str); 3] = [
    (FilterShape::Sharp, "Sharp"),
    (FilterShape::Soft, "Soft"),
    (FilterShape::Peaked, "Peaked"),
];

/// What the band card shows: the band settings, and the receiver they build.
#[derive(Clone, PartialEq)]
struct Band {
    band: BandSettings,
    /// The receiver as it is really built: the side-tone range can widen it
    /// past the slider, and its shape decides how much noise it lets in.
    receiver_width_hz: f64,
    noise_bandwidth_hz: f64,
    /// A contest run of single callers, the only run style until pile-ups
    /// come: the stations are the band around it, and there is no wanted
    /// station for a pile-up to spread around or sit under.
    single_callers: bool,
}

impl Band {
    fn of(settings: &TrainingSettings) -> Self {
        let receiver = FilterDesign::from_settings(settings);
        Self {
            band: settings.band.clone(),
            receiver_width_hz: receiver.width_hz(),
            noise_bandwidth_hz: receiver.noise_bandwidth_hz(),
            single_callers: settings.curriculum.char_set_mode == CharSetMode::Contest,
        }
    }
}

#[component]
pub fn BandConditionsCard(
    settings: Signal<TrainingSettings>,
    previewing: bool,
    on_preview: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    let view = use_memo(move || Band::of(&settings.read()));
    let view = view.read();
    let b = &view.band;
    let mut show_help = use_signal(|| false);
    // As heard through the filter you have set, so narrowing it visibly buys
    // signal-to-noise — which is the whole reason to reach for it.
    let noise_reading = if b.noise_enabled && b.noise_level > 0.0 {
        format!(
            "S/N {:.0} dB",
            heard_snr_db(b.noise_level, view.noise_bandwidth_hz)
        )
    } else {
        "Off".to_string()
    };
    let widened = (view.receiver_width_hz > b.filter_width_hz + 0.5).then(|| {
        format!(
            "{:.0} Hz — widened to {:.0} Hz to pass your pitch range",
            b.filter_width_hz, view.receiver_width_hz
        )
    });
    // A run of single callers has the band around it, and no pile-up.
    let (stations, stations_hint) = if view.single_callers {
        (
            "Band activity",
            "How busy the band is around your run, your caller counted. At 1 there is nobody on it but your caller; above that other stations work each other at their own pitches and speeds, never more of them keying at once than the rest of the count.",
        )
    } else {
        (
            "Stations calling",
            "How many call at once, yours included, drawn afresh for each group. At 1 the band is just your station; above that the others call across it, and the band has its own traffic around it — never more than the rest of the count keying at once. Hold both ends together for the same number every time, or open them up so you never know what you are walking into.",
        )
    };
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Waves,
                title: "Band conditions",
                note: "Stations, fading, static, noise and the receiver that hears them",
                if previewing {
                    button {
                        id: control_id("btn", "band stop"),
                        class: "btn btn-secondary btn-sm",
                        onclick: move |_| on_stop.call(()),
                        Icon { name: IconName::Stop }
                        "Stop"
                    }
                } else {
                    button {
                        id: control_id("btn", "band preview"),
                        class: "btn btn-primary btn-sm",
                        onclick: move |_| on_preview.call(()),
                        Icon { name: IconName::Play }
                        "Live preview"
                    }
                }
                SectionReset {
                    label: "band conditions",
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
                    Icon { name: if show_help() { IconName::X } else { IconName::Target } }
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
                    label: "Filter width (-6 dB)".to_string(),
                    id: "slider-filter-width".to_string(),
                    value_label: format!("{:.0} Hz", b.filter_width_hz),
                    value: b.filter_width_hz,
                    min: FILTER_WIDTH_MIN,
                    max: FILTER_WIDTH_MAX,
                    step: 10.0,
                    disabled: false,
                    onchange: move |v| settings.write().band.filter_width_hz = v,
                }
            }
            if let Some(widened) = widened {
                p { id: "filter-widened", class: "muted", style: "margin: 0;", "{widened}" }
            }
            div { class: "field",
                span { class: "field-label", "Filter shape" }
                div { class: "segmented",
                    for (shape, label) in FILTER_SHAPES {
                        Seg {
                            label,
                            id: control_id("seg", &format!("filter {label}")),
                            active: b.filter_shape == shape,
                            onclick: move |_| settings.write().band.filter_shape = shape,
                        }
                    }
                }
            }
            p { class: "muted", style: "margin: 0;",
                "Everything you hear comes through this. Narrow it and less noise gets in and the filter rings longer, but a station off your pitch fades with it. Sharp: a modern DSP filter, flat with steep sides. Soft: rounded like an older crystal filter. Peaked: Sharp plus the rig's audio peak filter — a resonance at your pitch; stations off your pitch come out a few dB weaker."
            }
            SettingsRange {
                settings,
                which: RangeSetting::Stations,
                label: stations,
                unit: "at once",
                min_bound: f64::from(STATIONS_MIN),
                max_bound: f64::from(STATIONS_MAX),
                step: 1.0,
                hint: stations_hint,
            }
            if !view.single_callers && b.stations_max > STATIONS_MIN {
                p { class: "muted", style: "margin: 0;",
                    "The others sit either side of the one you want, further out and a good way under it, and only ever where your filter passes them, out onto its skirts — copy the strongest, and narrow the filter until the rest drop away. A narrow filter leaves them little room, so it thins the pile-up: not everyone who calls gets through. Between groups the band carries on: other stations working each other at their own pitches and speeds, coming and going."
                }
                div { class: "field-grid",
                    SliderField {
                        label: "Pile-up spread".to_string(),
                        value_label: format!("±{:.0} Hz", b.pileup_spread_hz),
                        value: b.pileup_spread_hz,
                        min: PILEUP_SPREAD_MIN,
                        max: PILEUP_SPREAD_MAX,
                        step: 5.0,
                        disabled: false,
                        onchange: move |v| settings.write().band.pileup_spread_hz = v,
                    }
                    SliderField {
                        label: "Pile-up is weaker by".to_string(),
                        value_label: format!("{:.0} dB", b.pileup_level_db),
                        value: b.pileup_level_db,
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
                checked: b.qsb_enabled,
                onchange: move |on| settings.write().band.qsb_enabled = on,
            }
            div { class: "field-grid",
                SliderField {
                    label: "Depth".to_string(),
                    value_label: format!("{:.0}%", b.qsb_depth * 100.0),
                    value: b.qsb_depth,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                    disabled: !b.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_depth = v,
                }
                SliderField {
                    label: "Rate".to_string(),
                    value_label: format!("{:.2} Hz", b.qsb_rate_hz),
                    value: b.qsb_rate_hz,
                    min: QSB_RATE_MIN_HZ,
                    max: QSB_RATE_MAX_HZ,
                    step: 0.01,
                    disabled: !b.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_rate_hz = v,
                }
            }
            Switch {
                title: "QRN static".to_string(),
                description: "Lightning: distant crackle and the crash of a near storm.".to_string(),
                checked: b.qrn_enabled,
                onchange: move |on| settings.write().band.qrn_enabled = on,
            }
            SliderField {
                label: "Intensity".to_string(),
                id: "slider-qrn-intensity".to_string(),
                value_label: format!("{:.0}%", b.qrn_level * 100.0),
                value: b.qrn_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !b.qrn_enabled,
                onchange: move |v| settings.write().band.qrn_level = v,
            }
            Switch {
                title: "Band noise".to_string(),
                description: "The steady hiss of the band, through your filter.".to_string(),
                checked: b.noise_enabled,
                onchange: move |on| settings.write().band.noise_enabled = on,
            }
            SliderField {
                label: "Level".to_string(),
                id: "slider-noise-level".to_string(),
                value_label: noise_reading,
                value: b.noise_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !b.noise_enabled,
                onchange: move |v| settings.write().band.noise_level = v,
            }
            Switch {
                title: "AGC".to_string(),
                description: "The receiver rides its gain: the band ducks under a strong station and behind a crash.".to_string(),
                checked: b.agc_enabled,
                onchange: move |on| settings.write().band.agc_enabled = on,
            }
        }
    }
}
