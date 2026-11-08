use dioxus::prelude::*;

use crate::format::ordinal;

/// A medal for the first three places: gold, silver and bronze, with the
/// place on it.
#[component]
pub fn Medal(place: usize, #[props(default)] large: bool) -> Element {
    let class = match (place, large) {
        (1, false) => "medal medal-gold",
        (1, true) => "medal medal-gold large",
        (2, false) => "medal medal-silver",
        (2, true) => "medal medal-silver large",
        (_, false) => "medal medal-bronze",
        (_, true) => "medal medal-bronze large",
    };
    let label = format!("{} place", ordinal(place));
    rsx! {
        svg {
            class: class,
            view_box: "0 0 32 40",
            role: "img",
            "aria-label": "{label}",
            path { class: "medal-ribbon-left", d: "M7 1h7l4.5 15h-7z" }
            path { class: "medal-ribbon-right", d: "M25 1h-7l-4.5 15h7z" }
            circle { class: "medal-disc", cx: "16", cy: "26.5", r: "12" }
            circle { class: "medal-ring", cx: "16", cy: "26.5", r: "8.8" }
            text {
                class: "medal-number",
                x: "16",
                y: "30.3",
                text_anchor: "middle",
                "{place}"
            }
        }
    }
}
