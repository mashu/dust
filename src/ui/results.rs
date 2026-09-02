use cw_core::SessionResult;
use dioxus::prelude::*;

use crate::ui::widgets::GroupResultRow;

#[component]
pub fn ResultsView(
    result: SessionResult,
    auto_message: Option<String>,
    on_again: EventHandler<()>,
    on_home: EventHandler<()>,
) -> Element {
    let acc_pct = (result.accuracy * 100.0).round();
    let acc_class = if result.accuracy >= 0.9 {
        "kpi emerald"
    } else if result.accuracy >= 0.7 {
        "kpi amber"
    } else {
        "kpi rose"
    };
    let correct = result.groups.iter().filter(|g| g.correct).count();
    let avg = if result.avg_response_ms > 0.0 {
        format!("{}ms", result.avg_response_ms.round())
    } else {
        "—".into()
    };
    rsx! {
        div { class: "stack",
            div { style: "text-align: center;",
                h2 { style: "margin: 0;", "Session complete" }
                p { class: "muted", "Here's how you did" }
            }
            if let Some(msg) = auto_message {
                div { class: "card", style: "border-color: var(--emerald-200); background: var(--emerald-50);", "{msg}" }
            }
            div { class: "grid-4",
                div { class: acc_class,
                    div { class: "tiny", "Accuracy" }
                    div { class: "value", "{acc_pct}%" }
                }
                div { class: "kpi blue",
                    div { class: "tiny", "Groups" }
                    div { class: "value", "{correct}/{result.groups.len()}" }
                }
                div { class: "kpi purple",
                    div { class: "tiny", "Avg time" }
                    div { class: "value", "{avg}" }
                }
                div { class: "kpi indigo",
                    div { class: "tiny", "Score" }
                    div { class: "value", "{result.score.round()}" }
                }
            }
            div { class: "card stack",
                h3 { style: "margin: 0;", "Group results" }
                div { class: "stack", style: "max-height: 50vh; overflow: auto;",
                    for (idx, group) in result.groups.iter().cloned().enumerate() {
                        GroupResultRow { index: idx, group }
                    }
                }
            }
            div { class: "row", style: "justify-content: center;",
                button { class: "btn btn-primary", onclick: move |_| on_again.call(()), "Train again" }
                button { class: "btn btn-secondary", onclick: move |_| on_home.call(()), "Back" }
            }
        }
    }
}
