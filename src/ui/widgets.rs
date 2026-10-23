use cw_core::{GroupResult, RangeSetting, TrainingSettings, align_group};
use dioxus::prelude::*;

/// What was typed into a number field, held inside its bounds. `None` for
/// anything that is not a finite number, which leaves the value as it was.
fn parse_number_input(raw: &str, min: f64, max: f64) -> Option<f64> {
    let value: f64 = raw.trim().parse().ok()?;
    value.is_finite().then(|| value.clamp(min, max))
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

/// The id prefix for a button whose only job is to reveal more of the page.
///
/// It is a convention with teeth: screen-wide sweeps open every control named
/// this way before they look for anything else, and a disclosure has to carry
/// an `open` class while its panel is up so they can tell it worked.
pub const DISCLOSURE: &str = "disclosure";

/// A stable element id for a control, built from the label it shows.
///
/// Every interactive control carries one: it is what a stylesheet, an anchor or
/// a test reaches for, and deriving it from the label keeps the two in step.
///
/// The prefix names the kind of control — `btn`, `slider`, `switch`, `seg` and
/// so on. One of them is load-bearing: `disclosure` marks a button whose only
/// job is to reveal more of the page. Screen-wide sweeps open every one of
/// those before they look for controls, so a panel that hides behind a toggle
/// is still tested.
pub fn control_id(prefix: &str, label: &str) -> String {
    let mut slug = String::new();
    let mut gap = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            if gap && !slug.is_empty() {
                slug.push('-');
            }
            slug.extend(ch.to_lowercase());
            gap = false;
        } else {
            gap = true;
        }
    }
    if slug.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}-{slug}")
    }
}

/// The script that gives the input called `id` the focus, unless it has it.
///
/// Every build runs what it is given to eval as the body of a function, so the
/// script may return early.
pub fn focus_script(id: &str) -> String {
    format!(
        r#"const input = document.getElementById("{id}");
if (!input || document.activeElement === input) {{
    return;
}}
input.focus({{ preventScroll: true }});"#
    )
}

/// The script that gives the input called `id` the focus, unless it has it,
/// with the cursor at the end of what it holds and nothing selected: a run's
/// field taken back to finish or put right.
pub fn focus_end_script(id: &str) -> String {
    format!(
        r#"{}
const end = input.value.length;
input.setSelectionRange(end, end);"#,
        focus_script(id)
    )
}

/// A button that leaves the cursor where it is: a click would otherwise take
/// it from the box being typed in, so that the letters typed next went
/// nowhere and Enter pressed the button again.
pub fn keep_focus(e: MouseEvent) {
    e.prevent_default();
}

/// Emit a boolean HTML attribute only when it should apply.
///
/// HTML treats the *presence* of `disabled` / `readonly` / `autofocus` as
/// true. Dioxus still serializes `false` as `"false"`, which leaves the
/// control unfocusable.
pub fn html_bool(on: bool) -> Option<bool> {
    on.then_some(true)
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

/// The app's icons. A name that is not one of these does not compile, which
/// is the point: a typo used to draw a dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconName {
    Signal,
    Chart,
    Sliders,
    Play,
    Pause,
    Stop,
    Headphones,
    Check,
    X,
    Back,
    Sun,
    Moon,
    Auto,
    Envelope,
    Gauge,
    Waves,
    Target,
    Shuffle,
    Repeat,
    Reset,
    Letters,
    Timer,
    Volume,
    Sparkle,
    Flag,
    Trash,
    Chevron,
    Help,
    Upload,
    Download,
    Medal,
}

impl IconName {
    /// Every icon, for the test that draws them all.
    #[cfg(test)]
    pub const ALL: [Self; 31] = [
        Self::Signal,
        Self::Chart,
        Self::Sliders,
        Self::Play,
        Self::Pause,
        Self::Stop,
        Self::Headphones,
        Self::Check,
        Self::X,
        Self::Back,
        Self::Sun,
        Self::Moon,
        Self::Auto,
        Self::Envelope,
        Self::Gauge,
        Self::Waves,
        Self::Target,
        Self::Shuffle,
        Self::Repeat,
        Self::Reset,
        Self::Letters,
        Self::Timer,
        Self::Volume,
        Self::Sparkle,
        Self::Flag,
        Self::Trash,
        Self::Chevron,
        Self::Help,
        Self::Upload,
        Self::Download,
        Self::Medal,
    ];

    /// The strokes, on a 24×24 grid.
    fn paths(self) -> &'static [&'static str] {
        match self {
            Self::Signal => &["M2 12h3l2.5-7 3.5 14 3-10 2 3h6"],
            Self::Chart => &["M4 20V11", "M10 20V4", "M16 20v-6", "M2 20h20"],
            Self::Sliders => &[
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
            Self::Play => &["M7 4.5l12 7.5-12 7.5z"],
            Self::Pause => &["M8.5 5v14", "M15.5 5v14"],
            Self::Stop => &["M6.5 6.5h11v11h-11z"],
            Self::Headphones => &["M4 14v-2a8 8 0 0116 0v2", "M4 14h3v6H4z", "M17 14h3v6h-3z"],
            Self::Check => &["M4.5 12.5l5 5 10-11"],
            Self::Trash => &[
                "M4 7h16",
                "M9.5 7V4.5h5V7",
                "M6.5 7l1 12.5h9l1-12.5",
                "M10.2 11v5",
                "M13.8 11v5",
            ],
            Self::X => &["M6 6l12 12", "M18 6L6 18"],
            Self::Back => &["M15 5l-7 7 7 7"],
            Self::Sun => &[
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
            Self::Moon => &["M20.5 14.5A8.5 8.5 0 019.5 3.5a8.5 8.5 0 1011 11z"],
            Self::Auto => &[
                "M21 12a9 9 0 11-18 0 9 9 0 0118 0z",
                "M12 3v18a9 9 0 000-18z",
            ],
            Self::Envelope => &["M2 18c3.5 0 3-11 7-11s3.5 11 7 11", "M16 18h6", "M2 18h0.5"],
            Self::Gauge => &["M20 13a8 8 0 10-16 0", "M12 13l4.5-4", "M4 18h16"],
            Self::Waves => &[
                "M4 14a6 6 0 0116 0",
                "M8 17.5a3.2 3.2 0 017.5 0",
                "M12 21h.01",
                "M12 3v3",
            ],
            Self::Target => &[
                "M21 12a9 9 0 11-18 0 9 9 0 0118 0z",
                "M16 12a4 4 0 11-8 0 4 4 0 018 0z",
                "M12 11.5h.01",
            ],
            Self::Shuffle => &[
                "M3 6.5h4l9 11h5",
                "M3 17.5h4l2.5-3",
                "M14 8l2.5-1.5h5",
                "M18 3.5l4 3-4 3",
                "M18 14.5l4 3-4 3",
            ],
            Self::Repeat => &[
                "M17 2.5l4 4-4 4",
                "M3 11.5v-1a4 4 0 014-4h14",
                "M7 21.5l-4-4 4-4",
                "M21 12.5v1a4 4 0 01-4 4H3",
            ],
            // A single turn back round: undo, not the two-arrow loop of "repeat".
            Self::Reset => &["M3 12a9 9 0 109-9 9 9 0 00-6.4 2.6L3 8", "M3 3.5V8h4.5"],
            Self::Letters => &["M3 18l5-12 5 12", "M5 14h6", "M16 9h5", "M18.5 9v9"],
            Self::Timer => &[
                "M20 13.5a8 8 0 11-16 0 8 8 0 0116 0z",
                "M12 9.5v4l2.5 2",
                "M9 2h6",
            ],
            Self::Volume => &["M11 5L6.5 9H3v6h3.5L11 19z", "M15 9.5a4 4 0 010 5"],
            Self::Sparkle => &["M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"],
            Self::Flag => &["M5 21V4", "M5 5h13l-2.5 4L18 13H5"],
            // Points down, at what a closed disclosure holds.
            Self::Chevron => &["M6 9.5l6 6 6-6"],
            Self::Upload => &["M12 15.5V4.5", "M7.5 9L12 4.5 16.5 9", "M4.5 15v4.5h15V15"],
            Self::Download => &["M12 4.5v11", "M7.5 11L12 15.5 16.5 11", "M4.5 15v4.5h15V15"],
            // A medal on its ribbon.
            Self::Medal => &[
                "M8 2.5h3.5l2 6",
                "M16 2.5h-3.5l-1 3",
                "M17.5 15a5.5 5.5 0 11-11 0 5.5 5.5 0 0111 0z",
                "M12 12.5v5",
            ],
            Self::Help => &[
                "M21 12a9 9 0 11-18 0 9 9 0 0118 0z",
                "M9.4 9.4a2.7 2.7 0 015.2 1c0 1.8-2.6 2.2-2.6 3.8",
                "M12 17.3h.01",
            ],
        }
    }
}

/// Single-colour line icons, 24×24, stroked with `currentColor`.
#[component]
pub fn Icon(name: IconName) -> Element {
    rsx! {
        svg {
            class: "icon",
            view_box: "0 0 24 24",
            "aria-hidden": "true",
            "focusable": "false",
            for d in name.paths() {
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
        div { class: "stack-sm comparison", style: "gap: 3px;",
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
                Icon { name: if group.correct { IconName::Check } else { IconName::X } }
            }
        }
    }
}

/// The dot before a training screen's status, which says at a glance whose
/// move it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusDot {
    /// Something is going out: copper, pulsing.
    Live,
    /// Yours: green.
    Ready,
    /// Neither: the next thing is on its way, and the dot is plain.
    Waiting,
}

/// The head of a training screen: what is going on, how far it has come as
/// a figure and a bar, and anything the screen adds under the bar.
#[component]
pub fn ProgressHeader(
    /// What a guide points at it by, where one does: a run's.
    id: Option<String>,
    status: String,
    dot: StatusDot,
    /// How far it has come: "3 / 20", or a run's clock.
    figure: String,
    /// Said after the figure where there is room for it: an open run's
    /// longest length. A phone leaves it out.
    #[props(default)]
    figure_note: String,
    /// The bar's fill, in percent.
    pct: f64,
    /// The figure is a clock: a screen reader reads it when asked, and does
    /// not read every tick out.
    #[props(default)]
    timer: bool,
    #[props(default)] children: Element,
) -> Element {
    let pct = pct.clamp(0.0, 100.0);
    let dot = match dot {
        StatusDot::Live => "status-dot live",
        StatusDot::Ready => "status-dot ready",
        StatusDot::Waiting => "status-dot",
    };
    rsx! {
        div { id, class: "card",
            div { class: "row-between progress-head",
                div { class: "train-status",
                    span { class: dot }
                    span { class: "progress-status", "{status}" }
                }
                if timer {
                    span { class: "mono progress-figure", role: "timer", aria_live: "off",
                        "{figure}"
                        if !figure_note.is_empty() {
                            span { class: "progress-figure-note", "{figure_note}" }
                        }
                    }
                } else {
                    span { class: "mono progress-figure", "{figure}" }
                }
            }
            div { class: "progress-bar",
                span { style: "width: {pct}%;" }
            }
            {children}
        }
    }
}

/// The head every card wears: an icon, a title, a note under it, and the
/// card's own tools to the right.
#[component]
pub fn CardHead(
    icon: IconName,
    #[props(into)] title: String,
    #[props(into)] note: Option<String>,
    #[props(default)] children: Element,
) -> Element {
    rsx! {
        div { class: "card-head",
            div { class: "card-head-main",
                span { class: "card-icon", Icon { name: icon } }
                div {
                    h3 { class: "card-title", "{title}" }
                    if let Some(note) = note {
                        p { class: "card-note", "{note}" }
                    }
                }
            }
            div { class: "card-tools", {children} }
        }
    }
}

/// One choice among a few: a segment of a `.segmented` control, or with
/// `pill`, one of a row of `.mode-pills`.
#[component]
pub fn Seg(
    #[props(into)] label: String,
    active: bool,
    #[props(default)] pill: bool,
    /// Overrides the id derived from the label, where one label appears twice
    /// on the same screen.
    id: Option<String>,
    onclick: EventHandler<()>,
) -> Element {
    let (kind, class) = match (pill, active) {
        (false, false) => ("seg", "seg"),
        (false, true) => ("seg", "seg active"),
        (true, false) => ("pill", "pill"),
        (true, true) => ("pill", "pill active"),
    };
    rsx! {
        button {
            id: id.unwrap_or_else(|| control_id(kind, &label)),
            class,
            onclick: move |_| onclick.call(()),
            "{label}"
        }
    }
}

#[component]
pub fn Switch(
    #[props(into)] title: String,
    #[props(into)] description: Option<String>,
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
                    id: control_id("switch", &title),
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
    #[props(into)] label: String,
    value_label: String,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    disabled: bool,
    /// Overrides the id derived from the label, where one label appears twice.
    id: Option<String>,
    onchange: EventHandler<f64>,
) -> Element {
    let pct = pct_between(value, min, max);
    let name = id.unwrap_or_else(|| control_id("slider", &label));
    let at_min = value <= min;
    let at_max = value >= max;
    let less = format!("Decrease {label}");
    let more = format!("Increase {label}");
    rsx! {
        div { class: if disabled { "slider disabled" } else { "slider" },
            div { class: "slider-head",
                span { class: "slider-name", "{label}" }
                span { class: "slider-value", "{value_label}" }
            }
            div { class: "slider-row",
                button {
                    id: control_id("less", &name),
                    class: "step",
                    r#type: "button",
                    aria_label: less,
                    disabled: html_bool(disabled || at_min),
                    onclick: move |_| onchange.call(nudge(value - step, step, min, max)),
                    "−"
                }
                div { class: "slider-bar",
                    span { class: "slider-track" }
                    span { class: "slider-fill", style: "width: {pct}%;" }
                    span { class: "slider-knob", style: "left: {pct}%;" }
                    input {
                        id: name.clone(),
                        r#type: "range",
                        aria_label: "{label}",
                        min: "{min}",
                        max: "{max}",
                        step: "{step}",
                        value: "{value}",
                        disabled: html_bool(disabled),
                        oninput: move |e| {
                            if let Ok(v) = e.value().parse::<f64>() {
                                onchange.call(v);
                            }
                        }
                    }
                }
                button {
                    id: control_id("more", &name),
                    class: "step",
                    r#type: "button",
                    aria_label: more,
                    disabled: html_bool(disabled || at_max),
                    onclick: move |_| onchange.call(nudge(value + step, step, min, max)),
                    "+"
                }
            }
        }
    }
}

/// A number typed in, or stepped with − and +.
///
/// What is typed stays in the box, and reaches `onchange` only once it is
/// committed — on Enter, or on leaving the field — inside its bounds. A number
/// is typed through its prefixes: on the way to 30 the box holds 3, and a
/// setting that took every key would take that too, which for one end of a
/// range drags the other end down with it.
///
/// A key does not redraw the field. Every redraw sends the box its `value`
/// again, and on the desktop and phone builds that lands a frame later, over
/// whatever was typed in the meantime: a quick 120 could come out as 10. So
/// the field redraws only when it is committed or stepped, or when what it
/// is given changes.
///
/// − and + show the value they leave in the box, over anything typed and not
/// committed, even when the setting did not take the step.
#[component]
pub fn NumberField(
    #[props(into)] label: String,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    #[props(into)] unit: Option<String>,
    /// Overrides the id derived from the label, for fields whose label is not
    /// unique or not stable.
    id: Option<String>,
    onchange: EventHandler<f64>,
) -> Element {
    let name = id.unwrap_or_else(|| control_id("number", &label));
    // The text being typed, until it is committed or stepped over. Nothing
    // subscribes to it, so a key costs no redraw; a redraw for any other
    // reason shows the latest of it rather than taking the box back to the
    // value.
    let mut typed = use_hook(|| CopyValue::new(None::<String>));
    // Moved on by every commit and every step. Reading it here is what puts
    // the value back in the box afterwards, even when the value itself did
    // not move — 999 into a field already at its top, or a step the setting
    // did not take.
    let mut repaint = use_signal(|| 0u64);
    let _ = repaint.read();
    // Drop what is typed, redraw the box, and hand `to` on if there is one.
    // − and + do this as well as a commit, because some typing is never
    // committed. Text put back the way it was has nothing to commit. A key
    // typed straight after Enter can be written over by the commit's redraw,
    // a frame late, which puts the box back as it was committed. And a phone
    // can keep the box focused through a tap on − or +. The text is still
    // held then, and a step that kept it would show it in place of the
    // number stepped to.
    let mut settle = move |to: Option<f64>| {
        typed.set(None);
        repaint += 1;
        if let Some(v) = to {
            onchange.call(v);
        }
    };
    let shown = typed.peek().clone().unwrap_or_else(|| value.to_string());
    let less = format!("Decrease {label}");
    let more = format!("Increase {label}");
    rsx! {
        div { class: "field",
            label { r#for: "{name}", "{label}" }
            div { class: "stepper",
                button {
                    id: control_id("less", &name),
                    class: "step",
                    r#type: "button",
                    aria_label: less,
                    disabled: html_bool(value <= min),
                    onclick: move |_| settle(Some(nudge(value - step, step, min, max))),
                    "−"
                }
                input {
                    id: name.clone(),
                    r#type: "number",
                    inputmode: "decimal",
                    min: "{min}",
                    max: "{max}",
                    step: "{step}",
                    value: "{shown}",
                    oninput: move |e| typed.set(Some(e.value())),
                    onchange: move |e| settle(parse_number_input(&e.value(), min, max)),
                }
                if let Some(unit) = unit {
                    span { class: "unit", "{unit}" }
                }
                button {
                    id: control_id("more", &name),
                    class: "step",
                    r#type: "button",
                    aria_label: more,
                    disabled: html_bool(value >= max),
                    onclick: move |_| settle(Some(nudge(value + step, step, min, max))),
                    "+"
                }
            }
        }
    }
}

/// Put one card back to its defaults.
///
/// Sits with the card's own tools rather than in one page-wide "reset
/// everything", because the thing you want back is almost always the corner
/// you have just been experimenting in — and a single global reset is the one
/// nobody dares press.
#[component]
pub fn SectionReset(#[props(into)] label: String, on_reset: EventHandler<()>) -> Element {
    let title = format!("Reset {label} to defaults");
    rsx! {
        button {
            id: control_id("reset", &label),
            class: "icon-btn",
            r#type: "button",
            title: "{title}",
            aria_label: "{title}",
            onclick: move |_| on_reset.call(()),
            Icon { name: IconName::Reset }
        }
    }
}

/// A value that is either fixed or drawn from a min–max range each time it is used.
#[component]
pub fn LinkedRange(
    label: &'static str,
    unit: &'static str,
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
                        id: control_id("link", label),
                        class: if linked { "link-toggle on" } else { "link-toggle" },
                        r#type: "button",
                        onclick: move |_| on_link.call(!linked),
                        Icon { name: if linked { IconName::Target } else { IconName::Shuffle } }
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
                    id: control_id("fixed", label),
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
                        id: control_id("from", label),
                        value: min_value,
                        min: min_bound,
                        max: max_bound,
                        step,
                        onchange: move |v: f64| on_min.call(v),
                    }
                    NumberField {
                        label: format!("Max ({unit})"),
                        id: control_id("to", label),
                        value: max_value,
                        min: min_bound,
                        max: max_bound,
                        step,
                        onchange: move |v: f64| on_max.call(v),
                    }
                }
            }
            if let Some(hint) = hint {
                p { class: "muted", style: "margin: 0; font-size: 0.8rem;", "{hint}" }
            }
        }
    }
}

/// One min/max setting. Every edit goes through `TrainingSettings`, which owns
/// the ordering and link invariants, so this stays declarative. It reads its
/// own range and nothing else, so it redraws only when that moves.
#[component]
pub fn SettingsRange(
    settings: Signal<TrainingSettings>,
    which: RangeSetting,
    label: &'static str,
    unit: &'static str,
    min_bound: f64,
    max_bound: f64,
    step: f64,
    hint: Option<&'static str>,
) -> Element {
    let range = use_memo(move || settings.read().range(which))();
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
            hint: hint.map(str::to_string),
            on_min: move |v| settings.write().set_range_min(which, v),
            on_max: move |v| settings.write().set_range_max(which, v),
            on_link: move |on| settings.write().set_range_linked(which, on),
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
    use dioxus::prelude::*;

    use super::{
        NumberField, control_id, focus_end_script, focus_script, html_bool, nudge,
        parse_number_input, pct_between, pretty_number,
    };
    use crate::testing::Ui;

    /// The harness never mounts anything on a page, so the script is the one
    /// part of an input taking the focus a test can see.
    #[test]
    fn an_input_takes_the_focus_unless_it_has_it() {
        let script = focus_script("group-input-7");
        assert!(
            script.contains(r#"getElementById("group-input-7")"#),
            "{script}"
        );
        let has = script
            .find("document.activeElement === input")
            .expect("an input with the focus is left alone");
        let focus = script
            .find("input.focus({ preventScroll: true })")
            .expect("the input takes the focus where it is");
        assert!(has < focus, "{script}");
    }

    /// A run's field takes the focus the same way, then has the cursor put
    /// at the end of what it holds, nothing selected: once it has the
    /// focus, and never in a field that had it already.
    #[test]
    fn a_runs_field_takes_the_focus_at_its_end() {
        let script = focus_end_script("field-call");
        assert!(script.starts_with(&focus_script("field-call")), "{script}");
        let focus = script.find("input.focus(").expect("focused");
        let end = script
            .find("input.setSelectionRange(end, end)")
            .expect("the cursor at the end, nothing selected");
        assert!(focus < end, "{script}");
        assert!(script.contains("const end = input.value.length;"));
    }

    /// A number field whose unit can be changed under it, which redraws it
    /// without moving its value.
    #[component]
    fn ChangingUnit() -> Element {
        let mut value = use_signal(|| 2.0);
        let mut unit = use_signal(|| "sec");
        rsx! {
            NumberField {
                label: "Count",
                value: value(),
                min: 0.0,
                max: 200.0,
                step: 1.0,
                unit: unit(),
                onchange: move |v| value.set(v),
            }
            button {
                id: control_id("btn", "change unit"),
                onclick: move |_| unit.set("min"),
                "change unit"
            }
        }
    }

    /// A field redrawn while something is typed into it keeps what is typed:
    /// a redraw sends the box its value, and until it is committed that is
    /// the text typed, not the number it is to replace.
    #[test]
    fn a_field_redrawn_mid_typing_keeps_what_is_typed() {
        let mut ui = Ui::new(ChangingUnit, ());
        let _ = ui.take_work();
        ui.type_into("number-count", "12");
        assert_eq!(ui.take_work(), 0, "a key redrew the field");
        ui.click("btn-change-unit");
        assert!(ui.has(">min<"), "{}", ui.html());
        assert!(ui.input_tag("number-count").contains("value=\"12\""));
        ui.commit("number-count", "120");
        assert!(ui.input_tag("number-count").contains("value=\"120\""));
    }

    /// A number field on a page of its own, over a setting that goes no
    /// higher than `ceiling`. The setting is written only when it moves, as
    /// the settings page passes on only what changed: a page redrawn on
    /// every edit would redraw the field with it, whether or not the field
    /// redraws itself.
    #[component]
    fn UpTo(start: f64, ceiling: f64) -> Element {
        let mut value = use_signal(|| start);
        rsx! {
            NumberField {
                label: "Count",
                value: value(),
                min: 0.0,
                max: 200.0,
                step: 1.0,
                onchange: move |v: f64| {
                    let v = v.min(ceiling);
                    if v != *value.peek() {
                        value.set(v);
                    }
                },
            }
        }
    }

    /// 999 committed into a field already at its top leaves the value where
    /// it was, so only the field itself can take the 999 out of the box. The
    /// page holds nothing else, and redrawing the field changes nothing but
    /// the box's `value`, so the commit's one write is the top going back
    /// over the 999: none means it stays on screen.
    #[test]
    fn a_commit_that_leaves_the_value_where_it_was_still_writes_the_box() {
        let mut ui = Ui::new(
            UpTo,
            UpToProps {
                start: 200.0,
                ceiling: 200.0,
            },
        );
        let _ = ui.take_work();
        ui.type_into("number-count", "999");
        assert_eq!(ui.take_work(), 0, "typing wrote over the box");
        ui.commit("number-count", "999");
        assert_eq!(
            ui.take_work(),
            1,
            "the commit should write the box, and only that"
        );
        assert!(ui.input_tag("number-count").contains("value=\"200\""));
    }

    /// A step the setting does not take still shows its value, over text
    /// typed and never committed. At 5, short of the field's own top, + is
    /// offered and the setting stays where it is, so again the one write is
    /// the field's own.
    #[test]
    fn a_step_the_setting_does_not_take_still_shows_its_value() {
        let mut ui = Ui::new(
            UpTo,
            UpToProps {
                start: 5.0,
                ceiling: 5.0,
            },
        );
        ui.type_into("number-count", "57");
        let _ = ui.take_work();
        ui.click("more-number-count");
        assert_eq!(ui.take_work(), 1, "the step left 57 in the box");
        assert!(ui.input_tag("number-count").contains("value=\"5\""));
    }

    #[test]
    fn a_typed_number_is_held_inside_its_bounds() {
        assert_eq!(parse_number_input("18", 5.0, 60.0), Some(18.0));
        assert_eq!(parse_number_input(" 18 ", 5.0, 60.0), Some(18.0));
        assert_eq!(parse_number_input("1", 5.0, 60.0), Some(5.0));
        assert_eq!(parse_number_input("99", 5.0, 60.0), Some(60.0));
        assert_eq!(parse_number_input("-1", 1.0, 100.0), Some(1.0));
    }

    #[test]
    fn anything_but_a_finite_number_is_ignored() {
        assert_eq!(parse_number_input("", 1.0, 100.0), None);
        assert_eq!(parse_number_input("-", 1.0, 100.0), None);
        assert_eq!(parse_number_input("1e500", 1.0, 100.0), None);
        assert_eq!(parse_number_input("NaN", 1.0, 100.0), None);
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

    #[test]
    fn boolean_html_attributes_are_omitted_when_off() {
        assert_eq!(html_bool(true), Some(true));
        assert_eq!(html_bool(false), None);
    }

    #[test]
    fn a_control_id_is_the_label_slugged_under_its_prefix() {
        assert_eq!(control_id("btn", "Start training"), "btn-start-training");
        assert_eq!(control_id("seg", "CW Academy"), "seg-cw-academy");
        assert_eq!(control_id("day", "2026-09-08"), "day-2026-09-08");
        assert_eq!(control_id("number", "Level (1–40)"), "number-level-1-40");
        // Punctuation and accents never reach the id.
        assert_eq!(control_id("field", "Min (°)"), "field-min");
        // A label with nothing usable in it leaves the prefix on its own.
        assert_eq!(control_id("btn", "—"), "btn");
        assert_eq!(control_id("btn", ""), "btn");
    }
}
