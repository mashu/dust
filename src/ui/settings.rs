use cw_core::{RangeSetting, TrainingSettings, GROUP_REPEAT_MAX};
use dioxus::prelude::*;

use crate::ui::envelope::EnvelopeCard;
use crate::ui::settings_charset::CharsetCard;
use crate::ui::widgets::{Icon, LinkedRange, NumberField, Switch};

#[component]
fn SectionHead(icon: &'static str, title: &'static str, note: &'static str) -> Element {
    rsx! {
        div { class: "card-head",
            div { class: "card-head-main",
                span { class: "card-icon", Icon { name: icon } }
                div {
                    h3 { class: "card-title", "{title}" }
                    p { class: "card-note", "{note}" }
                }
            }
        }
    }
}

/// One min/max setting. Every edit goes through `TrainingSettings`, which owns
/// the ordering and link invariants, so this stays declarative.
#[component]
fn SettingsRange(
    settings: Signal<TrainingSettings>,
    which: RangeSetting,
    label: String,
    unit: String,
    min_bound: f64,
    max_bound: f64,
    step: f64,
    hint: Option<String>,
) -> Element {
    let range = settings().range(which);
    rsx! {
        LinkedRange {
            label,
            unit,
            min_value: range.min,
            max_value: range.max,
            linked: range.linked,
            min_bound,
            max_bound,
            step,
            hint,
            on_min: move |v| settings.write().set_range_min(which, v),
            on_max: move |v| settings.write().set_range_max(which, v),
            on_link: move |on| settings.write().set_range_linked(which, on),
        }
    }
}

#[component]
pub fn SettingsView(
    settings: Signal<TrainingSettings>,
    previewing: bool,
    sample_playing: Option<String>,
    on_preview_band: EventHandler<()>,
    on_stop_band: EventHandler<()>,
    on_play_sample: EventHandler<String>,
) -> Element {
    let s = settings();
    rsx! {
        div { class: "stack settings-page",
            header { class: "page-head",
                h2 { class: "page-title", "Settings" }
                p { class: "page-sub", "Everything the trainer sends, and how hard it makes you work." }
            }

            CharsetCard { settings }

            div { class: "card stack-sm",
                SectionHead {
                    icon: "timer",
                    title: "Session shape",
                    note: "How much gets sent in one run",
                }
                div { class: "field-grid",
                    NumberField {
                        label: "Groups per session".to_string(),
                        value: s.curriculum.num_groups as f64,
                        min: 1.0,
                        max: 100.0,
                        step: 1.0,
                        onchange: move |v| settings.write().curriculum.num_groups = v as u32,
                    }
                    NumberField {
                        label: "Answer timeout".to_string(),
                        value: s.playback.group_timeout,
                        min: 0.0,
                        max: 60.0,
                        step: 1.0,
                        unit: "sec".to_string(),
                        onchange: move |v| settings.write().playback.group_timeout = v,
                    }
                }
                SettingsRange {
                    settings,
                    which: RangeSetting::GroupSize,
                    label: "Group size".to_string(),
                    unit: "chars".to_string(),
                    min_bound: 1.0,
                    max_bound: 15.0,
                    step: 1.0,
                }
                SettingsRange {
                    settings,
                    which: RangeSetting::GroupRepeat,
                    label: "Sends per group".to_string(),
                    unit: "×".to_string(),
                    min_bound: 1.0,
                    max_bound: GROUP_REPEAT_MAX as f64,
                    step: 1.0,
                    hint: "Each group is repeated this many times before the answer box opens — one word space apart, like a station repeating its call. Switch to Random to vary it per group.".to_string(),
                }
                Switch {
                    title: "Lock typing while sending".to_string(),
                    description: "Keeps you copying by ear instead of typing along.".to_string(),
                    checked: s.playback.lock_input_during_group_playback,
                    onchange: move |on| settings.write().playback.lock_input_during_group_playback = on,
                }
            }

            div { class: "card stack-sm",
                SectionHead {
                    icon: "gauge",
                    title: "Speed",
                    note: "Character speed and Farnsworth spacing",
                }
                SettingsRange {
                    settings,
                    which: RangeSetting::CharWpm,
                    label: "Character speed".to_string(),
                    unit: "WPM".to_string(),
                    min_bound: 5.0,
                    max_bound: 60.0,
                    step: 1.0,
                }
                if !s.playback.link_char_to_effective {
                    SettingsRange {
                        settings,
                        which: RangeSetting::EffectiveWpm,
                        label: "Effective speed".to_string(),
                        unit: "WPM".to_string(),
                        min_bound: 5.0,
                        max_bound: 60.0,
                        step: 1.0,
                        hint: "Farnsworth: characters stay fast, the gaps between them stretch.".to_string(),
                    }
                }
                NumberField {
                    label: "Extra word spacing".to_string(),
                    value: s.playback.extra_word_space_multiplier,
                    min: 0.1,
                    max: 8.0,
                    step: 0.1,
                    unit: "×".to_string(),
                    onchange: move |v| settings.write().playback.extra_word_space_multiplier = v,
                }
                Switch {
                    title: "Tie effective speed to character speed".to_string(),
                    description: "Off gives you Farnsworth spacing controls.".to_string(),
                    checked: s.playback.link_char_to_effective,
                    onchange: move |on| {
                        let w = &mut *settings.write();
                        w.playback.link_char_to_effective = on;
                        w.sync_effective_to_char();
                    },
                }
            }

            div { class: "card stack-sm",
                SectionHead {
                    icon: "volume",
                    title: "Tone & volume",
                    note: "Side tone pitch and sending level",
                }
                SettingsRange {
                    settings,
                    which: RangeSetting::SideTone,
                    label: "Side tone".to_string(),
                    unit: "Hz".to_string(),
                    min_bound: 200.0,
                    max_bound: 1200.0,
                    step: 10.0,
                    hint: "A range picks a fresh pitch per group, the way different stations sound.".to_string(),
                }
                SettingsRange {
                    settings,
                    which: RangeSetting::Volume,
                    label: "Volume".to_string(),
                    unit: "gain".to_string(),
                    min_bound: 0.1,
                    max_bound: 1.0,
                    step: 0.05,
                }
            }

            EnvelopeCard {
                settings,
                sample_playing,
                on_play: on_play_sample,
                on_stop: on_stop_band,
            }

            div { class: "card stack-sm",
                SectionHead {
                    icon: "target",
                    title: "Auto level",
                    note: "Move the level from session accuracy",
                }
                Switch {
                    title: "Adjust the level for me".to_string(),
                    description: "Unlocks a new character after a run of good sessions.".to_string(),
                    checked: s.auto_level.auto_adjust_level,
                    onchange: move |on| settings.write().auto_level.auto_adjust_level = on,
                }
                NumberField {
                    label: "Accuracy threshold".to_string(),
                    value: s.auto_level.auto_adjust_threshold,
                    min: 50.0,
                    max: 100.0,
                    step: 1.0,
                    unit: "%".to_string(),
                    onchange: move |v| settings.write().auto_level.auto_adjust_threshold = v,
                }
                div { class: "field-grid",
                    NumberField {
                        label: "Sessions above to level up".to_string(),
                        value: s.auto_level.auto_adjust_above_threshold_count as f64,
                        min: 0.0,
                        max: 20.0,
                        step: 1.0,
                        onchange: move |v| settings.write().auto_level.auto_adjust_above_threshold_count = v as u32,
                    }
                    NumberField {
                        label: "Sessions below to level down".to_string(),
                        value: s.auto_level.auto_adjust_below_threshold_count as f64,
                        min: 0.0,
                        max: 20.0,
                        step: 1.0,
                        onchange: move |v| settings.write().auto_level.auto_adjust_below_threshold_count = v as u32,
                    }
                }
            }

            div { class: "card stack-sm",
                SectionHead {
                    icon: "shuffle",
                    title: "Character sampling",
                    note: "Which characters come round more often",
                }
                div { class: "field-grid",
                    NumberField {
                        label: "Error-weight strength".to_string(),
                        value: s.auto_level.error_weight_strength,
                        min: 0.0,
                        max: 10.0,
                        step: 0.5,
                        onchange: move |v| settings.write().auto_level.error_weight_strength = v,
                    }
                    NumberField {
                        label: "Coverage strength".to_string(),
                        value: s.auto_level.char_sampling_coverage_strength,
                        min: 0.0,
                        max: 8.0,
                        step: 0.5,
                        onchange: move |v| settings.write().auto_level.char_sampling_coverage_strength = v,
                    }
                }
                Switch {
                    title: "Thompson sampling".to_string(),
                    description: "Draws from each letter's uncertainty instead of its average error rate.".to_string(),
                    checked: s.auto_level.char_sampling_thompson,
                    onchange: move |on| settings.write().auto_level.char_sampling_thompson = on,
                }
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
