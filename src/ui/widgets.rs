use cw_core::{align_group, GroupResult};
use dioxus::prelude::*;

fn parse_number_input(raw: &str, min: f64, max: f64, commit: bool) -> Option<f64> {
    let value: f64 = raw.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    if commit {
        return Some(value.clamp(min, max));
    }
    if value > max {
        Some(max)
    } else if value < 0.0 && min >= 0.0 {
        None
    } else {
        Some(value)
    }
}

/// Snap to the step grid so 0.1-steps do not drift into 0.30000000000000004.
fn nudge(value: f64, step: f64, min: f64, max: f64) -> f64 {
    let step = if step.abs() < f64::EPSILON { 1.0 } else { step };
    let snapped = ((value / step).round() * step * 1e6).round() / 1e6;
    snapped.clamp(min, max)
}

fn pct_between(value: f64, min: f64, max: f64) -> f64 {
    if (max - min).abs() < f64::EPSILON {
        return 0.0;
    }
    (((value - min) / (max - min)) * 100.0).clamp(0.0, 100.0)
}

/// Trim trailing zeros so 18.0 reads as "18" and 0.75 stays "0.75".
pub fn pretty_number(value: f64) -> String {
    let rounded = (value * 1000.0).round() / 1000.0;
    if (rounded - rounded.round()).abs() < 1e-9 {
        format!("{}", rounded.round() as i64)
    } else {
        format!("{rounded}")
    }
}

/// Single-colour line icons, 24×24, stroked with `currentColor`.
#[component]
pub fn Icon(name: String) -> Element {
    let paths: &[&str] = match name.as_str() {
        "signal" => &["M2 12h3l2.5-7 3.5 14 3-10 2 3h6"],
        "chart" => &["M4 20V11", "M10 20V4", "M16 20v-6", "M2 20h20"],
        "sliders" => &[
            "M3 7h8",
            "M15 7h6",
            "M3 13h3",
            "M10 13h11",
            "M3 19h11",
            "M18 19h3",
            "M13 5v4",
            "M8 11v4",
            "M16 17v4",
        ],
        "play" => &["M7 4.5l12 7.5-12 7.5z"],
        "stop" => &["M6.5 6.5h11v11h-11z"],
        "headphones" => &["M4 14v-2a8 8 0 0116 0v2", "M4 14h3v6H4z", "M17 14h3v6h-3z"],
        "check" => &["M4.5 12.5l5 5 10-11"],
        "x" => &["M6 6l12 12", "M18 6L6 18"],
        "chevron" => &["M9 5l7 7-7 7"],
        "back" => &["M15 5l-7 7 7 7"],
        "sun" => &[
            "M12 3.5v2",
            "M12 18.5v2",
            "M3.5 12h2",
            "M18.5 12h2",
            "M6 6l1.5 1.5",
            "M16.5 16.5L18 18",
            "M18 6l-1.5 1.5",
            "M7.5 16.5L6 18",
            "M15.5 12a3.5 3.5 0 11-7 0 3.5 3.5 0 017 0z",
        ],
        "moon" => &["M20.5 14.5A8.5 8.5 0 019.5 3.5a8.5 8.5 0 1011 11z"],
        "auto" => &[
            "M21 12a9 9 0 11-18 0 9 9 0 0118 0z",
            "M12 3v18a9 9 0 000-18z",
        ],
        "envelope" => &["M2 18c3.5 0 3-11 7-11s3.5 11 7 11", "M16 18h6", "M2 18h0.5"],
        "gauge" => &["M20 13a8 8 0 10-16 0", "M12 13l4.5-4", "M4 18h16"],
        "waves" => &[
            "M4 14a6 6 0 0116 0",
            "M8 17.5a3.2 3.2 0 017.5 0",
            "M12 21h.01",
            "M12 3v3",
        ],
        "target" => &[
            "M21 12a9 9 0 11-18 0 9 9 0 0118 0z",
            "M16 12a4 4 0 11-8 0 4 4 0 018 0z",
            "M12 11.5h.01",
        ],
        "shuffle" => &[
            "M3 6.5h4l9 11h5",
            "M3 17.5h4l2.5-3",
            "M14 8l2.5-1.5h5",
            "M18 3.5l4 3-4 3",
            "M18 14.5l4 3-4 3",
        ],
        "repeat" => &[
            "M17 2.5l4 4-4 4",
            "M3 11.5v-1a4 4 0 014-4h14",
            "M7 21.5l-4-4 4-4",
            "M21 12.5v1a4 4 0 01-4 4H3",
        ],
        "letters" => &["M3 18l5-12 5 12", "M5 14h6", "M16 9h5", "M18.5 9v9"],
        "timer" => &[
            "M20 13.5a8 8 0 11-16 0 8 8 0 0116 0z",
            "M12 9.5v4l2.5 2",
            "M9 2h6",
        ],
        "volume" => &["M11 5L6.5 9H3v6h3.5L11 19z", "M15 9.5a4 4 0 010 5"],
        "trophy" => &[
            "M7 4h10v5a5 5 0 01-10 0z",
            "M7 6H4v2a3 3 0 003 3",
            "M17 6h3v2a3 3 0 01-3 3",
            "M9 20h6",
            "M12 14v6",
        ],
        "sparkle" => &["M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"],
        "flag" => &["M5 21V4", "M5 5h13l-2.5 4L18 13H5"],
        _ => &["M12 12h.01"],
    };
    rsx! {
        svg {
            class: "icon",
            view_box: "0 0 24 24",
            "aria-hidden": "true",
            "focusable": "false",
            for d in paths.iter() {
                path { d: "{d}" }
            }
        }
    }
}

#[component]
pub fn CharacterComparison(sent: String, received: String) -> Element {
    let sent_up = sent.to_ascii_uppercase();
    let recv_up = received.to_ascii_uppercase();
    let alignment = align_group(&sent_up, &recv_up);
    rsx! {
        div { class: "stack-sm", style: "gap: 3px;",
            div { class: "chars",
                for ch in sent_up.chars() {
                    span { class: "ch sent-row", "{ch}" }
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
    let class = if group.correct {
        "result-row"
    } else {
        "result-row bad"
    };
    rsx! {
        div { class: class,
            span { class: "result-index mono", "{index + 1}" }
            div { style: "flex: 1; min-width: 0;",
                CharacterComparison { sent: group.sent.clone(), received: group.received.clone() }
                if !group.correct && group.received.is_empty() {
                    p { class: "muted", style: "margin: 0.3rem 0 0; font-size: 0.78rem;", "No answer given" }
                }
            }
            span { class: if group.correct { "badge good" } else { "badge bad" },
                Icon { name: if group.correct { "check" } else { "x" } }
            }
        }
    }
}

#[component]
pub fn ProgressHeader(current: usize, total: usize, status: String, live: bool) -> Element {
    let pct = if total == 0 {
        0.0
    } else {
        ((current + 1) as f64 / total as f64) * 100.0
    };
    let dot = if live {
        "status-dot live"
    } else {
        "status-dot ready"
    };
    rsx! {
        div { class: "card",
            div { class: "row-between", style: "margin-bottom: 0.65rem;",
                div { class: "train-status",
                    span { class: dot }
                    span { style: "font-weight: 700;", "{status}" }
                }
                span { class: "mono", style: "color: var(--ink-dim); font-size: 0.85rem;",
                    "{current + 1} / {total}"
                }
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

/// One segment of a `.segmented` control.
#[component]
pub fn Seg(label: String, active: bool, onclick: EventHandler<()>) -> Element {
    rsx! {
        button {
            class: if active { "seg active" } else { "seg" },
            onclick: move |_| onclick.call(()),
            "{label}"
        }
    }
}

#[component]
pub fn Switch(
    title: String,
    description: Option<String>,
    checked: bool,
    onchange: EventHandler<bool>,
) -> Element {
    rsx! {
        label { class: "switch-row",
            div { class: "switch-copy",
                div { class: "switch-title", "{title}" }
                if let Some(text) = description {
                    p { class: "switch-desc", "{text}" }
                }
            }
            span { class: if checked { "switch on" } else { "switch" },
                input {
                    r#type: "checkbox",
                    checked,
                    onchange: move |e| onchange.call(e.checked()),
                }
                i {}
            }
        }
    }
}

#[component]
pub fn SliderField(
    label: String,
    value_label: String,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    disabled: bool,
    onchange: EventHandler<f64>,
) -> Element {
    let pct = pct_between(value, min, max);
    rsx! {
        div { class: if disabled { "slider disabled" } else { "slider" },
            div { class: "slider-head",
                span { class: "slider-name", "{label}" }
                span { class: "slider-value", "{value_label}" }
            }
            input {
                r#type: "range",
                min: "{min}",
                max: "{max}",
                step: "{step}",
                value: "{value}",
                disabled,
                style: "--pct: {pct}%;",
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<f64>() {
                        onchange.call(v);
                    }
                }
            }
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
    unit: Option<String>,
    onchange: EventHandler<f64>,
) -> Element {
    rsx! {
        div { class: "field",
            label { "{label}" }
            div { class: "stepper",
                button {
                    class: "step",
                    r#type: "button",
                    aria_label: "Decrease",
                    disabled: value <= min,
                    onclick: move |_| onchange.call(nudge(value - step, step, min, max)),
                    "−"
                }
                input {
                    r#type: "number",
                    inputmode: "decimal",
                    min: "{min}",
                    max: "{max}",
                    step: "{step}",
                    value: "{value}",
                    oninput: move |e| {
                        if let Some(v) = parse_number_input(&e.value(), min, max, false) {
                            onchange.call(v);
                        }
                    },
                    onchange: move |e| {
                        if let Some(v) = parse_number_input(&e.value(), min, max, true) {
                            onchange.call(v);
                        }
                    },
                }
                if let Some(unit) = unit {
                    span { class: "unit", "{unit}" }
                }
                button {
                    class: "step",
                    r#type: "button",
                    aria_label: "Increase",
                    disabled: value >= max,
                    onclick: move |_| onchange.call(nudge(value + step, step, min, max)),
                    "+"
                }
            }
        }
    }
}

/// A value that is either fixed or drawn from a min–max range each time it is used.
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
    hint: Option<String>,
    on_min: EventHandler<f64>,
    on_max: EventHandler<f64>,
    on_link: EventHandler<bool>,
) -> Element {
    let low = pct_between(min_value, min_bound, max_bound);
    let high = pct_between(max_value, min_bound, max_bound);
    let width = (high - low).max(2.5);
    let summary = if linked {
        format!("{} {unit}", pretty_number(min_value))
    } else {
        format!(
            "{}–{} {unit}",
            pretty_number(min_value),
            pretty_number(max_value)
        )
    };
    rsx! {
        div { class: "range-field",
            div { class: "range-head",
                span { class: "field-label", "{label}" }
                div { class: "row", style: "gap: 0.45rem;",
                    span { class: "range-value", "{summary}" }
                    button {
                        class: if linked { "link-toggle on" } else { "link-toggle" },
                        r#type: "button",
                        onclick: move |_| on_link.call(!linked),
                        Icon { name: if linked { "target" } else { "shuffle" } }
                        if linked { "Fixed" } else { "Random" }
                    }
                }
            }
            div { class: "range-vis",
                span { style: "left: {low}%; width: {width}%;" }
            }
            if linked {
                NumberField {
                    label: format!("Value ({unit})"),
                    value: min_value,
                    min: min_bound,
                    max: max_bound,
                    step,
                    onchange: move |v: f64| on_min.call(v),
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
                            on_min.call(v);
                            if v > max_value {
                                on_max.call(v);
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
                            on_max.call(v);
                            if v < min_value {
                                on_min.call(v);
                            }
                        },
                    }
                }
            }
            if let Some(hint) = hint {
                p { class: "muted", style: "margin: 0; font-size: 0.8rem;", "{hint}" }
            }
        }
    }
}

/// Circular gauge used for the session score.
#[component]
pub fn ScoreRing(pct: f64, value: String, caption: String) -> Element {
    let pct = pct.clamp(0.0, 100.0);
    let radius = 48.0_f64;
    let circumference = 2.0 * std::f64::consts::PI * radius;
    let filled = circumference * (pct / 100.0);
    let gap = circumference - filled;
    let stroke = if pct >= 90.0 {
        "var(--good)"
    } else if pct >= 70.0 {
        "var(--warn)"
    } else {
        "var(--bad)"
    };
    rsx! {
        svg { class: "score-ring", view_box: "0 0 120 120",
            circle {
                cx: "60",
                cy: "60",
                r: "{radius}",
                fill: "none",
                stroke: "var(--surface-sunken)",
                stroke_width: "10",
            }
            circle {
                cx: "60",
                cy: "60",
                r: "{radius}",
                fill: "none",
                stroke: "{stroke}",
                stroke_width: "10",
                stroke_linecap: "round",
                stroke_dasharray: "{filled:.2} {gap:.2}",
                transform: "rotate(-90 60 60)",
            }
            text {
                x: "60",
                y: "58",
                text_anchor: "middle",
                fill: "var(--ink)",
                font_size: "26",
                font_weight: "700",
                "{value}"
            }
            text {
                x: "60",
                y: "78",
                text_anchor: "middle",
                fill: "var(--ink-dim)",
                font_size: "11",
                font_weight: "700",
                letter_spacing: "1.4",
                "{caption}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{nudge, parse_number_input, pct_between, pretty_number};

    #[test]
    fn typing_below_min_is_kept_until_commit() {
        assert_eq!(parse_number_input("1", 5.0, 60.0, false), Some(1.0));
        assert_eq!(parse_number_input("18", 5.0, 60.0, false), Some(18.0));
        assert_eq!(parse_number_input("1", 5.0, 60.0, true), Some(5.0));
    }

    #[test]
    fn input_above_max_is_capped() {
        assert_eq!(parse_number_input("99", 5.0, 60.0, false), Some(60.0));
    }

    #[test]
    fn negative_values_are_ignored_when_min_is_non_negative() {
        assert_eq!(parse_number_input("-1", 1.0, 100.0, false), None);
        assert_eq!(parse_number_input("1e500", 1.0, 100.0, false), None);
    }

    #[test]
    fn stepping_stays_on_the_step_grid() {
        assert_eq!(nudge(0.2 + 0.1, 0.1, 0.0, 1.0), 0.3);
        assert_eq!(nudge(0.95 + 0.1, 0.05, 0.0, 1.0), 1.0);
        assert_eq!(nudge(5.0 - 1.0, 1.0, 5.0, 60.0), 5.0);
    }

    #[test]
    fn range_percentages_are_clamped() {
        assert_eq!(pct_between(5.0, 5.0, 60.0), 0.0);
        assert_eq!(pct_between(60.0, 5.0, 60.0), 100.0);
        assert_eq!(pct_between(1.0, 5.0, 60.0), 0.0);
        assert_eq!(pct_between(7.0, 7.0, 7.0), 0.0);
    }

    #[test]
    fn numbers_drop_trailing_zeros() {
        assert_eq!(pretty_number(18.0), "18");
        assert_eq!(pretty_number(0.75), "0.75");
        assert_eq!(pretty_number(0.1 + 0.2), "0.3");
    }
}
