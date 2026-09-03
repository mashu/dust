use cw_core::{align_group, GroupResult};
use dioxus::prelude::*;

#[component]
pub fn CharacterComparison(sent: String, received: String) -> Element {
    let sent_up = sent.to_ascii_uppercase();
    let recv_up = received.to_ascii_uppercase();
    let alignment = align_group(&sent_up, &recv_up);
    rsx! {
        div { class: "stack", style: "gap: 2px;",
            div { class: "chars",
                for ch in sent_up.chars() {
                    span { class: "ch", "{ch}" }
                }
            }
            div { class: "chars",
                for pair in alignment {
                    {
                        let class = if pair.matched {
                            "ch"
                        } else if pair.received_char.is_none() {
                            "ch miss"
                        } else {
                            "ch bad"
                        };
                        let shown = pair.received_char.unwrap_or('_');
                        rsx! { span { class: class, "{shown}" } }
                    }
                }
            }
        }
    }
}

#[component]
pub fn GroupResultRow(index: usize, group: GroupResult) -> Element {
    rsx! {
        div { class: "card", style: "padding: 0.7rem 0.85rem;",
            div { class: "row", style: "justify-content: space-between;",
                span { class: "tiny", "Group {index + 1}" }
                if group.correct {
                    span { style: "color: var(--emerald-600);", "✓" }
                } else {
                    span { style: "color: var(--rose-600);", "✗" }
                }
            }
            CharacterComparison { sent: group.sent.clone(), received: group.received.clone() }
            if !group.correct && group.received.is_empty() {
                div { class: "tiny", style: "color: var(--rose-500); text-transform: none; letter-spacing: 0;", "(no answer given)" }
            }
        }
    }
}

#[component]
pub fn ProgressHeader(current: usize, total: usize) -> Element {
    let pct = if total == 0 {
        0.0
    } else {
        ((current + 1) as f64 / total as f64) * 100.0
    };
    rsx! {
        div { class: "card",
            div { class: "row", style: "justify-content: space-between; margin-bottom: 0.5rem;",
                span { class: "tiny", "Progress" }
                span { class: "muted", "{current + 1} / {total}" }
            }
            div { class: "progress-bar",
                span { style: "width: {pct}%;" }
            }
        }
    }
}

#[component]
pub fn ModePill(label: String, active: bool, onclick: EventHandler<()>) -> Element {
    rsx! {
        button {
            class: if active { "pill active" } else { "pill" },
            onclick: move |_| onclick.call(()),
            "{label}"
        }
    }
}

#[component]
pub fn NumberField(
    label: String,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    onchange: EventHandler<f64>,
) -> Element {
    rsx! {
        div { class: "field",
            label { "{label}" }
            input {
                r#type: "number",
                inputmode: "decimal",
                min: "{min}",
                max: "{max}",
                step: "{step}",
                value: "{value}",
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<f64>() {
                        onchange.call(v);
                    }
                },
                onchange: move |e| {
                    if let Ok(v) = e.value().parse::<f64>() {
                        onchange.call(v.clamp(min, max));
                    }
                },
            }
        }
    }
}

#[component]
pub fn LinkedRange(
    label: String,
    unit: String,
    min_value: f64,
    max_value: f64,
    linked: bool,
    min_bound: f64,
    max_bound: f64,
    step: f64,
    on_min: EventHandler<f64>,
    on_max: EventHandler<f64>,
    on_link: EventHandler<bool>,
) -> Element {
    rsx! {
        div { class: "linked-range",
            div { class: "row", style: "justify-content: space-between;",
                span { class: "tiny", "{label}" }
                button {
                    class: if linked { "link-toggle on" } else { "link-toggle" },
                    onclick: move |_| on_link.call(!linked),
                    if linked { "Min = max" } else { "Range" }
                }
            }
            if linked {
                NumberField {
                    label: format!("{unit}"),
                    value: min_value,
                    min: min_bound,
                    max: max_bound,
                    step,
                    onchange: move |v: f64| on_min.call(v.clamp(min_bound, max_bound)),
                }
            } else {
                div { class: "field-grid",
                    NumberField {
                        label: format!("Min ({unit})"),
                        value: min_value,
                        min: min_bound,
                        max: max_bound,
                        step,
                        onchange: move |v: f64| {
                            let min = v.clamp(min_bound, max_bound);
                            on_min.call(min);
                            if min > max_value {
                                on_max.call(min);
                            }
                        },
                    }
                    NumberField {
                        label: format!("Max ({unit})"),
                        value: max_value,
                        min: min_bound,
                        max: max_bound,
                        step,
                        onchange: move |v: f64| {
                            let max = v.clamp(min_bound, max_bound);
                            on_max.call(max);
                            if max < min_value {
                                on_min.call(max);
                            }
                        },
                    }
                }
            }
        }
    }
}
