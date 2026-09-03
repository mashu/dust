use cw_core::TrainingSettings;
use dioxus::prelude::*;

use crate::ui::settings_charset::CharsetCard;
use crate::ui::widgets::{LinkedRange, NumberField};

#[component]
pub fn SettingsView(
    settings: Signal<TrainingSettings>,
    previewing: bool,
    on_preview_band: EventHandler<()>,
    on_stop_band: EventHandler<()>,
) -> Element {
    let s = settings();
    rsx! {
        div { class: "stack settings-page",
            h2 { class: "page-title", "Settings" }
            CharsetCard { settings }
            div { class: "card stack",
                div { class: "tiny", "Groups" }
                NumberField { label: "Groups per session".to_string(), value: s.curriculum.num_groups as f64, min: 1.0, max: 100.0, step: 1.0, onchange: move |v| settings.write().curriculum.num_groups = v as u32 }
                LinkedRange {
                    label: "Group size".to_string(),
                    unit: "chars".to_string(),
                    min_value: s.curriculum.min_group_size as f64,
                    max_value: s.curriculum.max_group_size as f64,
                    linked: s.curriculum.link_group_size,
                    min_bound: 1.0,
                    max_bound: 15.0,
                    step: 1.0,
                    on_min: move |v| {
                        let w = &mut *settings.write();
                        w.curriculum.min_group_size = v as u32;
                        if w.curriculum.link_group_size
                            || w.curriculum.min_group_size > w.curriculum.max_group_size
                        {
                            w.curriculum.max_group_size = w.curriculum.min_group_size;
                        }
                    },
                    on_max: move |v| {
                        let w = &mut *settings.write();
                        w.curriculum.max_group_size = v as u32;
                        if w.curriculum.max_group_size < w.curriculum.min_group_size {
                            w.curriculum.min_group_size = w.curriculum.max_group_size;
                        }
                    },
                    on_link: move |on| {
                        let w = &mut *settings.write();
                        w.curriculum.link_group_size = on;
                        if on {
                            w.curriculum.max_group_size = w.curriculum.min_group_size;
                        }
                    }
                }
                NumberField { label: "Group timeout (seconds, 0 = off)".to_string(), value: s.playback.group_timeout, min: 0.0, max: 60.0, step: 1.0, onchange: move |v| settings.write().playback.group_timeout = v }
                label { class: "check",
                    input { r#type: "checkbox", checked: s.playback.lock_input_during_group_playback, onchange: move |e| settings.write().playback.lock_input_during_group_playback = e.checked() }
                    "Lock typing until the group finishes playing"
                }
            }
            div { class: "card stack",
                div { class: "tiny", "Speed (WPM)" }
                LinkedRange {
                    label: "Character WPM".to_string(),
                    unit: "WPM".to_string(),
                    min_value: s.playback.char_wpm_min,
                    max_value: s.playback.char_wpm_max,
                    linked: s.playback.link_char_wpm,
                    min_bound: 5.0,
                    max_bound: 60.0,
                    step: 1.0,
                    on_min: move |v| {
                        let w = &mut *settings.write();
                        w.playback.char_wpm_min = v;
                        if w.playback.link_char_wpm
                            || w.playback.char_wpm_min > w.playback.char_wpm_max
                        {
                            w.playback.char_wpm_max = w.playback.char_wpm_min;
                        }
                        if w.playback.link_char_to_effective {
                            w.playback.effective_wpm_min = w.playback.char_wpm_min;
                            w.playback.effective_wpm_max = w.playback.char_wpm_max;
                        }
                    },
                    on_max: move |v| {
                        let w = &mut *settings.write();
                        w.playback.char_wpm_max = v;
                        if w.playback.char_wpm_max < w.playback.char_wpm_min {
                            w.playback.char_wpm_min = w.playback.char_wpm_max;
                        }
                        if w.playback.link_char_to_effective {
                            w.playback.effective_wpm_min = w.playback.char_wpm_min;
                            w.playback.effective_wpm_max = w.playback.char_wpm_max;
                        }
                    },
                    on_link: move |on| {
                        let w = &mut *settings.write();
                        w.playback.link_char_wpm = on;
                        if on {
                            w.playback.char_wpm_max = w.playback.char_wpm_min;
                            if w.playback.link_char_to_effective {
                                w.playback.effective_wpm_max = w.playback.char_wpm_max;
                            }
                        }
                    }
                }
                if !s.playback.link_char_to_effective {
                    LinkedRange {
                        label: "Effective WPM".to_string(),
                        unit: "WPM".to_string(),
                        min_value: s.playback.effective_wpm_min,
                        max_value: s.playback.effective_wpm_max,
                        linked: s.playback.link_effective_wpm,
                        min_bound: 5.0,
                        max_bound: 60.0,
                        step: 1.0,
                        on_min: move |v| {
                            let w = &mut *settings.write();
                            w.playback.effective_wpm_min = v;
                            if w.playback.link_effective_wpm
                                || w.playback.effective_wpm_min > w.playback.effective_wpm_max
                            {
                                w.playback.effective_wpm_max = v;
                            }
                        },
                        on_max: move |v| {
                            let w = &mut *settings.write();
                            w.playback.effective_wpm_max = v;
                            if w.playback.effective_wpm_max < w.playback.effective_wpm_min {
                                w.playback.effective_wpm_min = v;
                            }
                        },
                        on_link: move |on| {
                            let w = &mut *settings.write();
                            w.playback.link_effective_wpm = on;
                            if on {
                                w.playback.effective_wpm_max = w.playback.effective_wpm_min;
                            }
                        }
                    }
                }
                NumberField { label: "Extra word spacing".to_string(), value: s.playback.extra_word_space_multiplier, min: 0.1, max: 8.0, step: 0.1, onchange: move |v| settings.write().playback.extra_word_space_multiplier = v }
                label { class: "check",
                    input { r#type: "checkbox", checked: s.playback.link_char_to_effective, onchange: move |e| {
                        let on = e.checked();
                        let w = &mut *settings.write();
                        w.playback.link_char_to_effective = on;
                        if on {
                            w.playback.effective_wpm_min = w.playback.char_wpm_min;
                            w.playback.effective_wpm_max = w.playback.char_wpm_max;
                        }
                    } }
                    "Keep effective WPM equal to character WPM"
                }
            }
            div { class: "card stack",
                div { class: "tiny", "Tone & volume" }
                div { class: "field-grid",
                    NumberField {
                        label: "Side tone min (Hz)".to_string(),
                        value: s.band.side_tone_min,
                        min: 200.0,
                        max: 1200.0,
                        step: 10.0,
                        onchange: move |v| {
                            let w = &mut *settings.write();
                            w.band.side_tone_min = v;
                            if w.band.side_tone_min > w.band.side_tone_max {
                                w.band.side_tone_max = w.band.side_tone_min;
                            }
                        }
                    }
                    NumberField {
                        label: "Side tone max (Hz)".to_string(),
                        value: s.band.side_tone_max,
                        min: 200.0,
                        max: 1200.0,
                        step: 10.0,
                        onchange: move |v| {
                            let w = &mut *settings.write();
                            w.band.side_tone_max = v;
                            if w.band.side_tone_max < w.band.side_tone_min {
                                w.band.side_tone_min = w.band.side_tone_max;
                            }
                        }
                    }
                }
                LinkedRange {
                    label: "Volume".to_string(),
                    unit: "gain".to_string(),
                    min_value: s.band.volume_min,
                    max_value: s.band.volume_max,
                    linked: s.band.link_volume,
                    min_bound: 0.1,
                    max_bound: 1.0,
                    step: 0.05,
                    on_min: move |v| {
                        let w = &mut *settings.write();
                        w.band.volume_min = v;
                        if w.band.link_volume || w.band.volume_min > w.band.volume_max {
                            w.band.volume_max = v;
                        }
                    },
                    on_max: move |v| {
                        let w = &mut *settings.write();
                        w.band.volume_max = v;
                        if w.band.volume_max < w.band.volume_min {
                            w.band.volume_min = v;
                        }
                    },
                    on_link: move |on| {
                        let w = &mut *settings.write();
                        w.band.link_volume = on;
                        if on {
                            w.band.volume_max = w.band.volume_min;
                        }
                    }
                }
                NumberField { label: "Rise time (ms)".to_string(), value: s.band.steepness, min: 1.0, max: 40.0, step: 1.0, onchange: move |v| settings.write().band.steepness = v }
                NumberField { label: "Envelope smoothing".to_string(), value: s.band.envelope_smoothing, min: 0.0, max: 1.0, step: 0.05, onchange: move |v| settings.write().band.envelope_smoothing = v }
            }
            div { class: "card stack",
                div { class: "tiny", "Auto level" }
                label { class: "check",
                    input { r#type: "checkbox", checked: s.auto_level.auto_adjust_level, onchange: move |e| settings.write().auto_level.auto_adjust_level = e.checked() }
                    "Automatically adjust level from session accuracy"
                }
                NumberField { label: "Accuracy threshold %".to_string(), value: s.auto_level.auto_adjust_threshold, min: 50.0, max: 100.0, step: 1.0, onchange: move |v| settings.write().auto_level.auto_adjust_threshold = v }
                NumberField { label: "Sessions above to increase".to_string(), value: s.auto_level.auto_adjust_above_threshold_count as f64, min: 0.0, max: 20.0, step: 1.0, onchange: move |v| settings.write().auto_level.auto_adjust_above_threshold_count = v as u32 }
                NumberField { label: "Sessions below to decrease".to_string(), value: s.auto_level.auto_adjust_below_threshold_count as f64, min: 0.0, max: 20.0, step: 1.0, onchange: move |v| settings.write().auto_level.auto_adjust_below_threshold_count = v as u32 }
            }
            div { class: "card stack",
                div { class: "tiny", "Character sampling" }
                NumberField { label: "Error-weight strength".to_string(), value: s.auto_level.error_weight_strength, min: 0.0, max: 10.0, step: 0.5, onchange: move |v| settings.write().auto_level.error_weight_strength = v }
                NumberField { label: "Coverage strength".to_string(), value: s.auto_level.char_sampling_coverage_strength, min: 0.0, max: 8.0, step: 0.5, onchange: move |v| settings.write().auto_level.char_sampling_coverage_strength = v }
                label { class: "check",
                    input { r#type: "checkbox", checked: s.auto_level.char_sampling_thompson, onchange: move |e| settings.write().auto_level.char_sampling_thompson = e.checked() }
                    "Thompson sampling — explore uncertain letters"
                }
                p { class: "muted", "When on, each group draws from the letter’s uncertainty instead of the average error rate." }
            }
            crate::ui::band::BandConditionsCard {
                settings,
                previewing,
                on_preview: on_preview_band,
                on_stop: on_stop_band,
            }
        }
    }
}
