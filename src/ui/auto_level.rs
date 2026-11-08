use cw_core::{AutoLevelProgress, MixedAutoLevelAxis};
use dioxus::prelude::*;

use crate::format::auto_level::{percent_to_tenth, spaced_out, wpm};
use crate::ui::widgets::{CardHead, IconName};

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
            Some(MixedAutoLevelAxis::Letters) => "Alternating letters/digits — next: Letters",
            Some(MixedAutoLevelAxis::Digits) => "Alternating letters/digits — next: Digits",
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
    let pass = format!("{} to pass", percent_to_tenth(progress.threshold));
    // Where adaptive speed lets a session count, each end as the Speed
    // settings put it.
    let speed_hint = progress.judged_at.map(|(slowest, fastest)| {
        let farnsworth = if spaced_out(slowest) || spaced_out(fastest) {
            " Each reads Character/Effective speed."
        } else {
            ""
        };
        format!(
            "Adapt speed to me is on: a session counts only once it comes at the top of your speed range, {}, or its bottom, {}, and is judged on the groups sent there. In between it counts neither way.{farnsworth}",
            wpm(fastest),
            wpm(slowest),
        )
    });
    rsx! {
        section { class: "card",
            CardHead { icon: IconName::Target, title: "Auto level", note: mixed,
                span { class: "chip", "{pass}" }
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
            if progress.judged_on_reached {
                p { class: "muted hint",
                    "Adapt group size to me is on: a session is judged on the characters you reached. Those lost off the end of a group it can still shorten are left out, so a session can pass with its own accuracy under the threshold."
                }
            }
            if let Some(hint) = speed_hint {
                p { class: "muted hint", "{hint}" }
            }
        }
    }
}
