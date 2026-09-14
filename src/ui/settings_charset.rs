use cw_core::{
    apply_custom_sequence, apply_practice_window, apply_sequence_preset, current_practice_window,
    fit_settings_to_alphabet, sequence_preset_id, tier_examples, unlocked_practice_count,
    CharSetMode, PracticeWindow, TrainingSettings, CALLSIGN_TIER_MAX, MAX_DIGITS_LEVEL,
    SEQUENCE_PRESETS,
};
use dioxus::prelude::*;

use crate::ui::widgets::{control_id, Icon, ModePill, NumberField, Seg};

#[component]
pub fn CharsetCard(settings: Signal<TrainingSettings>) -> Element {
    let s = settings();
    let seq = s.sequence();
    let level_max = s.max_active_level();
    let preset = sequence_preset_id(&s);
    let unlocked = unlocked_practice_count(&s);
    let window = current_practice_window(&s);
    let pool = cw_core::compute_char_pool(&s);
    let callsigns = s.curriculum.char_set_mode == CharSetMode::Callsign;
    let mode_note = match s.curriculum.char_set_mode {
        CharSetMode::Koch => "Letters unlocked in Koch order",
        CharSetMode::Digits => "Digits only",
        CharSetMode::Mixed => "Letters and digits together",
        CharSetMode::Custom => "Your own character list",
        CharSetMode::Callsign => "Real callsigns, off the air",
    };
    rsx! {
        div { class: "card stack-sm",
            div { class: "card-head",
                div { class: "card-head-main",
                    span { class: "card-icon", Icon { name: "letters" } }
                    div {
                        h3 { class: "card-title", "Character set" }
                        if callsigns {
                            p { class: "card-note", "{mode_note} · tier {s.curriculum.callsign_level} of {CALLSIGN_TIER_MAX}" }
                        } else {
                            p { class: "card-note", "{mode_note} · {unlocked} in play" }
                        }
                    }
                }
            }
            div { class: "segmented",
                for (label, mode) in [
                    ("Koch", CharSetMode::Koch),
                    ("Digits", CharSetMode::Digits),
                    ("Mixed", CharSetMode::Mixed),
                    ("Custom", CharSetMode::Custom),
                    ("Callsigns", CharSetMode::Callsign),
                ] {
                    Seg {
                        label: label.to_string(),
                        active: s.curriculum.char_set_mode == mode,
                        onclick: move |_| {
                            let w = &mut *settings.write();
                            w.set_char_set_mode(mode);
                            fit_settings_to_alphabet(w);
                        },
                    }
                }
            }
            if s.curriculum.char_set_mode != CharSetMode::Digits && !callsigns {
                div { class: "eyebrow", "Unlock order" }
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
                div { class: "field",
                    label { "Sequence order" }
                    input {
                        id: control_id("field", "sequence order"),
                        class: "mono",
                        value: "{seq.iter().collect::<String>()}",
                        oninput: move |e| {
                            let w = &mut *settings.write();
                            let typed: Vec<char> = e.value().chars().collect();
                            w.curriculum.custom_sequence = TrainingSettings::unique_alphabet(&typed);
                            w.curriculum.sequence_is_custom = !w.curriculum.custom_sequence.is_empty();
                            fit_settings_to_alphabet(w);
                        }
                    }
                }
            }
            div { class: "field-grid",
                NumberField {
                    label: if callsigns {
                        format!("Callsign tier (1–{level_max})")
                    } else {
                        format!("Level (1–{level_max})")
                    },
                    id: "field-level".to_string(),
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
                if s.curriculum.char_set_mode == CharSetMode::Mixed {
                    NumberField {
                        label: format!("Digits level (1–{MAX_DIGITS_LEVEL})"),
                        id: "field-digits-level".to_string(),
                        value: s.curriculum.digits_level as f64,
                        min: 1.0,
                        max: MAX_DIGITS_LEVEL as f64,
                        step: 1.0,
                        onchange: move |v| {
                            let w = &mut *settings.write();
                            w.curriculum.digits_level = v as u32;
                            fit_settings_to_alphabet(w);
                        }
                    }
                }
            }
            if s.curriculum.char_set_mode == CharSetMode::Mixed {
                NumberField {
                    label: "Share of letters".to_string(),
                    value: s.curriculum.mixed_letters_percent as f64,
                    min: 0.0,
                    max: 100.0,
                    step: 5.0,
                    unit: "%".to_string(),
                    onchange: move |v| {
                        let w = &mut *settings.write();
                        w.curriculum.mixed_letters_percent = v as u32;
                        fit_settings_to_alphabet(w);
                    },
                }
            }
            if s.curriculum.char_set_mode == CharSetMode::Custom {
                div { class: "field",
                    label { "Custom alphabet" }
                    input {
                        id: control_id("field", "custom alphabet"),
                        class: "mono",
                        value: "{s.curriculum.custom_set.iter().collect::<String>()}",
                        oninput: move |e| {
                            let w = &mut *settings.write();
                            w.curriculum.custom_set = TrainingSettings::unique_alphabet(
                                &e.value().chars().collect::<Vec<_>>(),
                            );
                            fit_settings_to_alphabet(w);
                        }
                    }
                }
                p { class: "muted", style: "margin: 0;", "Level unlocks this list from the start. Leave it empty to use the sequence above." }
            }
            if !callsigns {
            div { class: "eyebrow", "Practice window" }
            div { class: "mode-pills",
                ModePill {
                    label: "Everything".to_string(),
                    active: window == Some(PracticeWindow::All),
                    onclick: move |_| apply_practice_window(&mut settings.write(), PracticeWindow::All),
                }
                if unlocked >= 3 {
                    ModePill {
                        label: "Newest 3".to_string(),
                        active: window == Some(PracticeWindow::Last3),
                        onclick: move |_| apply_practice_window(&mut settings.write(), PracticeWindow::Last3),
                    }
                }
                if unlocked >= 5 {
                    ModePill {
                        label: "Newest 5".to_string(),
                        active: window == Some(PracticeWindow::Last5),
                        onclick: move |_| apply_practice_window(&mut settings.write(), PracticeWindow::Last5),
                    }
                }
            }
            }
            if callsigns {
                div { class: "eyebrow", "What this tier sends" }
                div { class: "chars", style: "gap: 6px;",
                    for call in tier_examples(s.curriculum.callsign_level) {
                        span { class: "ch sent-row", "{call}" }
                    }
                }
                p { class: "muted", style: "margin: 0;",
                    "Every tier keeps what the ones below it send, so moving up only adds. Prefixes are real allocations, weighted the way you hear them."
                }
            }
            div { class: "chars", style: "gap: 4px;",
                for ch in pool.iter() {
                    span { class: "ch sent-row", "{ch}" }
                }
            }
        }
    }
}
