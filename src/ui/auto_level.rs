use cw_core::AutoLevelProgress;
use dioxus::prelude::*;

#[component]
pub fn AutoLevelCard(progress: AutoLevelProgress) -> Element {
    let up_pct = if progress.above_disabled || progress.above_target == 0 {
        0.0
    } else {
        (f64::from(progress.above_count) / f64::from(progress.above_target) * 100.0).min(100.0)
    };
    let down_pct = if progress.below_disabled || progress.below_target == 0 {
        0.0
    } else {
        (f64::from(progress.below_count) / f64::from(progress.below_target) * 100.0).min(100.0)
    };
    let mixed = if progress.alternating_mixed {
        match progress.next_mixed_axis {
            Some(cw_core::MixedAutoLevelAxis::Letters) => "Alternating letters/digits — next: Letters",
            Some(cw_core::MixedAutoLevelAxis::Digits) => "Alternating letters/digits — next: Digits",
            None => "Alternates letter/digit level",
        }
    } else {
        "Resets on level change"
    };
    let up_label = if progress.above_disabled {
        "—".to_string()
    } else {
        format!("{}/{}", progress.above_count, progress.above_target)
    };
    let down_label = if progress.below_disabled {
        "—".to_string()
    } else {
        format!("{}/{}", progress.below_count, progress.below_target)
    };
    rsx! {
        section { class: "card auto-level",
            div { class: "row", style: "justify-content: space-between;",
                span { class: "tiny", "Auto level" }
                span { class: "chip", "{progress.threshold as u32}% accuracy" }
            }
            div { class: "auto-bars",
                div { class: if progress.above_disabled { "auto-meter disabled" } else { "auto-meter" },
                    div { class: "row", style: "justify-content: space-between;",
                        span { class: "muted", "Up" }
                        span { class: "mono", "{up_label}" }
                    }
                    div { class: "meter-track up",
                        span { style: "width: {up_pct}%;" }
                    }
                }
                div { class: if progress.below_disabled { "auto-meter disabled" } else { "auto-meter" },
                    div { class: "row", style: "justify-content: space-between;",
                        span { class: "muted", "Down" }
                        span { class: "mono", "{down_label}" }
                    }
                    div { class: "meter-track down",
                        span { style: "width: {down_pct}%;" }
                    }
                }
            }
            p { class: "tiny", style: "text-transform: none; letter-spacing: 0; margin: 0.5rem 0 0;", "{mixed}" }
        }
    }
}
