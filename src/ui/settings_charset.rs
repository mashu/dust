use cw_core::{
    apply_custom_sequence, apply_practice_window, apply_sequence_preset, current_practice_window,
    fit_settings_to_alphabet, sequence_preset_id, unlocked_practice_count, CharSetMode,
    PracticeWindow, TrainingSettings, MAX_DIGITS_LEVEL, SEQUENCE_PRESETS,
};
use dioxus::prelude::*;

use crate::ui::widgets::{ModePill, NumberField};

#[component]
pub fn CharsetCard(settings: Signal<TrainingSettings>) -> Element {
    let s = settings();
    let seq = s.sequence();
    let level_max = s.max_active_level();
    let preset = sequence_preset_id(&s);
    let unlocked = unlocked_practice_count(&s);
    let window = current_practice_window(&s);
    let preview: String = cw_core::compute_char_pool(&s).into_iter().collect();
    rsx! {
        div { class: "card stack",
            div { class: "tiny", "Character set" }
            div { class: "mode-pills",
                ModePill { label: "Koch".to_string(), active: s.char_set_mode == CharSetMode::Koch, onclick: move |_| {
                    let w = &mut *settings.write();
                    w.char_set_mode = CharSetMode::Koch;
                    w.practice_window = Some(PracticeWindow::All);
                    fit_settings_to_alphabet(w);
                } }
                ModePill { label: "Digits".to_string(), active: s.char_set_mode == CharSetMode::Digits, onclick: move |_| {
                    let w = &mut *settings.write();
                    w.char_set_mode = CharSetMode::Digits;
                    w.practice_window = Some(PracticeWindow::All);
                    fit_settings_to_alphabet(w);
                } }
                ModePill { label: "Mixed".to_string(), active: s.char_set_mode == CharSetMode::Mixed, onclick: move |_| {
                    let w = &mut *settings.write();
                    w.char_set_mode = CharSetMode::Mixed;
                    w.practice_window = Some(PracticeWindow::All);
                    fit_settings_to_alphabet(w);
                } }
                ModePill { label: "Custom".to_string(), active: s.char_set_mode == CharSetMode::Custom, onclick: move |_| {
                    let w = &mut *settings.write();
                    w.char_set_mode = CharSetMode::Custom;
                    w.practice_window = Some(PracticeWindow::All);
                    fit_settings_to_alphabet(w);
                } }
            }
            if s.char_set_mode != CharSetMode::Digits {
                div { class: "tiny", "Sequence" }
                div { class: "mode-pills",
                    for preset_def in SEQUENCE_PRESETS.iter() {
                        {
                            let id = preset_def.id;
                            let active = preset == id;
                            rsx! {
                                ModePill {
                                    label: preset_def.name.to_string(),
                                    active,
                                    onclick: move |_| {
                                        let w = &mut *settings.write();
                                        apply_sequence_preset(w, id);
                                        fit_settings_to_alphabet(w);
                                        *w = w.clone().clamp();
                                    }
                                }
                            }
                        }
                    }
                    ModePill {
                        label: "Custom".to_string(),
                        active: preset == "custom",
                        onclick: move |_| {
                            let w = &mut *settings.write();
                            apply_custom_sequence(w);
                            fit_settings_to_alphabet(w);
                            *w = w.clone().clamp();
                        }
                    }
                }
                p { class: "muted sequence-preview", "{seq.iter().collect::<String>()}" }
                div { class: "field",
                    label { "Sequence order" }
                    input {
                        value: "{seq.iter().collect::<String>()}",
                        oninput: move |e| {
                            let w = &mut *settings.write();
                            let typed: Vec<char> = e.value().chars().collect();
                            w.custom_sequence = TrainingSettings::unique_alphabet(&typed);
                            w.sequence_is_custom = !w.custom_sequence.is_empty();
                            fit_settings_to_alphabet(w);
                        }
                    }
                }
            }
            NumberField {
                label: format!("Level (1–{level_max}) · {unlocked} unlocked"),
                value: s.active_level() as f64,
                min: 1.0,
                max: level_max as f64,
                step: 1.0,
                onchange: move |v| {
                    let w = &mut *settings.write();
                    w.set_active_level(v as u32);
                    fit_settings_to_alphabet(w);
                }
            }
            if s.char_set_mode == CharSetMode::Mixed {
                NumberField {
                    label: "Digits level (1–{MAX_DIGITS_LEVEL})",
                    value: s.digits_level as f64,
                    min: 1.0,
                    max: MAX_DIGITS_LEVEL as f64,
                    step: 1.0,
                    onchange: move |v| {
                        let w = &mut *settings.write();
                        w.digits_level = v as u32;
                        fit_settings_to_alphabet(w);
                    }
                }
            }
            if s.char_set_mode == CharSetMode::Mixed {
                NumberField {
                    label: "Mixed letters %".to_string(),
                    value: s.mixed_letters_percent as f64,
                    min: 0.0,
                    max: 100.0,
                    step: 5.0,
                    onchange: move |v| {
                        let w = &mut *settings.write();
                        w.mixed_letters_percent = v as u32;
                        fit_settings_to_alphabet(w);
                    },
                }
            }
            if s.char_set_mode == CharSetMode::Custom {
                div { class: "field",
                    label { "Custom alphabet" }
                    input {
                        value: "{s.custom_set.iter().collect::<String>()}",
                        oninput: move |e| {
                            let w = &mut *settings.write();
                            w.custom_set = TrainingSettings::unique_alphabet(
                                &e.value().chars().collect::<Vec<_>>(),
                            );
                            fit_settings_to_alphabet(w);
                        }
                    }
                }
                p { class: "muted", "Level unlocks this list from the start. Leave empty to use the sequence above." }
            }
            div { class: "tiny", "Practice window" }
            div { class: "mode-pills",
                ModePill {
                    label: "All".to_string(),
                    active: window == Some(PracticeWindow::All),
                    onclick: move |_| apply_practice_window(&mut settings.write(), PracticeWindow::All),
                }
                if unlocked >= 3 {
                    ModePill {
                        label: "Last 3".to_string(),
                        active: window == Some(PracticeWindow::Last3),
                        onclick: move |_| apply_practice_window(&mut settings.write(), PracticeWindow::Last3),
                    }
                }
                if unlocked >= 5 {
                    ModePill {
                        label: "Last 5".to_string(),
                        active: window == Some(PracticeWindow::Last5),
                        onclick: move |_| apply_practice_window(&mut settings.write(), PracticeWindow::Last5),
                    }
                }
            }
            p { class: "pool", "{preview}" }
        }
    }
}
