use cw_core::{TrainingSettings, GROUP_REPEAT_MAX};
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
    let tone_linked = (s.band.side_tone_min - s.band.side_tone_max).abs() < f64::EPSILON;
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
                LinkedRange {
                    label: "Sends per group".to_string(),
                    unit: "×".to_string(),
                    min_value: s.playback.group_repeat_min as f64,
                    max_value: s.playback.group_repeat_max as f64,
                    linked: s.playback.link_group_repeat,
                    min_bound: 1.0,
                    max_bound: GROUP_REPEAT_MAX as f64,
                    step: 1.0,
                    hint: "Each group is repeated this many times before the answer box opens — one word space apart, like a station repeating its call. Switch to Random to vary it per group.".to_string(),
                    on_min: move |v| {
                        let w = &mut *settings.write();
                        w.playback.group_repeat_min = v as u32;
                        if w.playback.link_group_repeat
                            || w.playback.group_repeat_min > w.playback.group_repeat_max
                        {
                            w.playback.group_repeat_max = w.playback.group_repeat_min;
                        }
                    },
                    on_max: move |v| {
                        let w = &mut *settings.write();
                        w.playback.group_repeat_max = v as u32;
                        if w.playback.group_repeat_max < w.playback.group_repeat_min {
                            w.playback.group_repeat_min = w.playback.group_repeat_max;
                        }
                    },
                    on_link: move |on| {
                        let w = &mut *settings.write();
                        w.playback.link_group_repeat = on;
                        if on {
                            w.playback.group_repeat_max = w.playback.group_repeat_min;
                        }
                    }
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
                LinkedRange {
                    label: "Character speed".to_string(),
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
                        label: "Effective speed".to_string(),
                        unit: "WPM".to_string(),
                        min_value: s.playback.effective_wpm_min,
                        max_value: s.playback.effective_wpm_max,
                        linked: s.playback.link_effective_wpm,
                        min_bound: 5.0,
                        max_bound: 60.0,
                        step: 1.0,
                        hint: "Farnsworth: characters stay fast, the gaps between them stretch.".to_string(),
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
                        if on {
                            w.playback.effective_wpm_min = w.playback.char_wpm_min;
                            w.playback.effective_wpm_max = w.playback.char_wpm_max;
                        }
                    },
                }
            }

            div { class: "card stack-sm",
                SectionHead {
                    icon: "volume",
                    title: "Tone & volume",
                    note: "Side tone pitch and sending level",
                }
                LinkedRange {
                    label: "Side tone".to_string(),
                    unit: "Hz".to_string(),
                    min_value: s.band.side_tone_min,
                    max_value: s.band.side_tone_max,
                    linked: tone_linked,
                    min_bound: 200.0,
                    max_bound: 1200.0,
                    step: 10.0,
                    hint: "A range picks a fresh pitch per group, the way different stations sound.".to_string(),
                    on_min: move |v| {
                        let w = &mut *settings.write();
                        w.band.side_tone_min = v;
                        if w.band.side_tone_min > w.band.side_tone_max {
                            w.band.side_tone_max = w.band.side_tone_min;
                        }
                    },
                    on_max: move |v| {
                        let w = &mut *settings.write();
                        w.band.side_tone_max = v;
                        if w.band.side_tone_max < w.band.side_tone_min {
                            w.band.side_tone_min = w.band.side_tone_max;
                        }
                    },
                    on_link: move |on| {
                        let w = &mut *settings.write();
                        if on {
                            w.band.side_tone_max = w.band.side_tone_min;
                        } else {
                            w.band.side_tone_max = (w.band.side_tone_min + 200.0).min(1200.0);
                            if w.band.side_tone_max <= w.band.side_tone_min {
                                w.band.side_tone_min = (w.band.side_tone_max - 200.0).max(200.0);
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
