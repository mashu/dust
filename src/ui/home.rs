use cw_core::{compute_streak_status, AutoLevelProgress, SessionResult, TrainingSettings};
use dioxus::prelude::*;

use crate::ui::auto_level::AutoLevelCard;
use crate::ui::heatmap::{ActivityHeatmap, StreakCard};
use crate::ui::tips::TipsCarousel;
use crate::ui::widgets::ProgressHeader;

#[component]
pub fn Home(
    settings: TrainingSettings,
    last_accuracy: Option<f64>,
    session_count: usize,
    pool: String,
    sessions: Vec<SessionResult>,
    today: String,
    auto_progress: Option<AutoLevelProgress>,
    on_start: EventHandler<()>,
    on_listen: EventHandler<()>,
) -> Element {
    let level_label = match settings.char_set_mode {
        cw_core::CharSetMode::Digits => "Digits level",
        cw_core::CharSetMode::Mixed => "Levels",
        _ => "Koch level",
    };
    let level_value = match settings.char_set_mode {
        cw_core::CharSetMode::Digits => format!("{}", settings.digits_level),
        cw_core::CharSetMode::Mixed => {
            format!("{} / {}", settings.koch_level, settings.digits_level)
        }
        _ => format!("{}", settings.koch_level),
    };
    let acc = last_accuracy
        .map(|a| format!("{}%", (a * 100.0).round()))
        .unwrap_or_else(|| "—".into());
    let dates: Vec<String> = sessions.iter().map(|s| s.date.clone()).collect();
    let streak = compute_streak_status(&dates, &today);
    rsx! {
        div { class: "stack",
            header {
                h2 { class: "page-title", style: "margin: 0 0 0.25rem;", "Practice" }
                p { class: "muted", "Hear the group first, then answer from memory." }
            }
            TipsCarousel {}
            if let Some(progress) = auto_progress {
                AutoLevelCard { progress }
            }
            StreakCard { status: streak }
            if session_count > 0 {
                div { class: "grid-3",
                    div { class: "kpi emerald",
                        div { class: "tiny", "Last accuracy" }
                        div { class: "value", "{acc}" }
                    }
                    div { class: "kpi indigo",
                        div { class: "tiny", "Sessions" }
                        div { class: "value", "{session_count}" }
                    }
                    div { class: "kpi blue",
                        div { class: "tiny", "{level_label}" }
                        div { class: "value", "{level_value}" }
                    }
                }
                ActivityHeatmap { sessions, today }
            }
            div { class: "card",
                div { class: "tiny", "Current alphabet" }
                p { class: "pool", "{pool}" }
                p { class: "muted", style: "margin: 0.4rem 0 0.8rem;",
                    "{settings.char_wpm_min as u32}–{settings.char_wpm_max as u32} WPM · {settings.min_group_size}–{settings.max_group_size} chars · {settings.num_groups} groups"
                }
                div { class: "home-actions",
                    button { class: "btn btn-primary", onclick: move |_| on_start.call(()), "Start training" }
                    button { class: "btn btn-secondary", onclick: move |_| on_listen.call(()), "Listen to letters" }
                }
            }
        }
    }
}

#[component]
pub fn TrainingView(
    current: usize,
    total: usize,
    groups: Vec<String>,
    inputs: Vec<String>,
    confirmed: Vec<bool>,
    focused: usize,
    playing: bool,
    locked: bool,
    status: String,
    on_change: EventHandler<(usize, String)>,
    on_confirm: EventHandler<usize>,
    on_focus: EventHandler<usize>,
    on_submit: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "stack",
            if !status.is_empty() {
                div { class: "card", style: "border-color: var(--amber-200); background: var(--amber-50); color: var(--amber-800);", "{status}" }
            }
            ProgressHeader { current, total }
            div { class: "card",
                p { class: "muted", "Enter answers per group (auto-advances when complete)." }
                div { class: "stack group-list",
                    for (idx, sent) in groups.iter().enumerate() {
                        {
                            let is_focused = focused == idx;
                            let is_active = current == idx;
                            let is_confirmed = confirmed.get(idx).copied().unwrap_or(false);
                            let disabled = playing && !is_active && !is_confirmed;
                            let input_locked = locked && is_active && !is_confirmed;
                            let value = inputs.get(idx).cloned().unwrap_or_default();
                            let shown = if is_confirmed { sent.clone() } else { "••••".into() };
                            let cls = if is_focused { "group focused" } else { "group" };
                            let input_cls = if disabled {
                                "answer"
                            } else if input_locked {
                                "answer locked"
                            } else {
                                "answer"
                            };
                            let placeholder = if input_locked {
                                "Listening..."
                            } else if disabled {
                                "Waiting..."
                            } else {
                                "Type group answer..."
                            };
                            rsx! {
                                div { class: cls,
                                    div { class: "row", style: "justify-content: space-between;",
                                        div { class: "row",
                                            span { class: if is_focused { "badge current" } else { "badge" }, "Group {idx + 1}" }
                                            if is_focused {
                                                span { class: "badge current", "Current" }
                                            }
                                        }
                                        div { class: "row",
                                            span { class: "sent", "{shown}" }
                                            if is_confirmed {
                                                if value.trim().eq_ignore_ascii_case(sent) {
                                                    span { style: "color: var(--emerald-600);", "✓" }
                                                } else {
                                                    span { style: "color: var(--rose-600);", "✗" }
                                                }
                                            }
                                        }
                                    }
                                    input {
                                        id: "group-input-{idx}",
                                        class: input_cls,
                                        value: "{value}",
                                        disabled: disabled,
                                        readonly: input_locked,
                                        placeholder: "{placeholder}",
                                        autocomplete: "off",
                                        autocapitalize: "characters",
                                        spellcheck: false,
                                        enterkeyhint: "done",
                                        inputmode: "text",
                                        onfocus: move |_| on_focus.call(idx),
                                        oninput: move |e| on_change.call((idx, e.value())),
                                        onkeydown: move |e| {
                                            if input_locked {
                                                e.prevent_default();
                                                return;
                                            }
                                            if e.key().to_string() == "Enter" && !disabled {
                                                on_confirm.call(idx);
                                            }
                                        }
                                    }
                                    if is_confirmed {
                                        crate::ui::widgets::CharacterComparison {
                                            sent: sent.clone(),
                                            received: value.trim().to_ascii_uppercase(),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                p { class: "tiny", style: "margin-top: 0.6rem; text-transform: none; letter-spacing: 0;",
                    "Auto-advances when the group is complete · Enter to confirm"
                }
            }
            div { class: "train-actions",
                button { class: "btn btn-primary", onclick: move |_| on_submit.call(()), "Submit" }
                button { class: "btn btn-danger", onclick: move |_| on_stop.call(()), "Stop" }
            }
        }
    }
}
