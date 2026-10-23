use cw_core::BoardEntry;
use dioxus::prelude::*;

use super::{Medal, mode_label, per_group_label, speed};
use crate::ui::format::{ago, clock, long_date};

/// When a session was set, in full: "Friday 16 October 2026 at 14:32".
fn when_set(entry: &BoardEntry) -> String {
    match clock(entry.timestamp) {
        Some(time) => format!("{} at {time}", long_date(&entry.date)),
        None => long_date(&entry.date),
    }
}

/// The personal best, with its gold: what it earned a group and in all,
/// when it was set, and how it was sent. Beside it, the form: the median a
/// group over the last sessions.
#[component]
pub(super) fn BestCard(
    best: BoardEntry,
    /// The form after the last session, if there is one.
    form: Option<f64>,
    /// How many sessions the form is the median of.
    form_over: usize,
    /// Today's date, to say how long ago the best was set.
    today: String,
    /// The board is of every mode, so the best says which it was.
    show_mode: bool,
) -> Element {
    let set_ago = ago(&best.date, &today);
    rsx! {
        div { class: "card best-card",
            Medal { place: 1, large: true }
            div { class: "best-copy",
                div { class: "tiny", "Personal best" }
                div { class: "best-score",
                    "{per_group_label(best.per_group)}"
                    span { class: "best-unit", " points a group" }
                }
                p { class: "best-when",
                    "{best.score.round()} points over {best.groups} groups"
                }
                p { class: "best-when",
                    "{when_set(&best)}"
                    if let Some(set_ago) = set_ago {
                        span { class: "muted", " · {set_ago}" }
                    }
                }
                div { class: "best-chips",
                    span { class: "chip", "{speed(&best)}" }
                    span { class: "chip good", "{(best.accuracy * 100.0).round()}% accuracy" }
                    if show_mode {
                        span { class: "chip",
                            "{best.contest.map_or(mode_label(best.mode), |kind| kind.label())}"
                        }
                    }
                }
            }
            if let Some(form) = form {
                div { class: "best-form",
                    div { class: "tiny", "Form" }
                    div { class: "best-form-value", "{per_group_label(form)}" }
                    div { class: "best-form-note",
                        if form_over == 1 {
                            "your one session"
                        } else {
                            "median of your last {form_over}"
                        }
                    }
                }
            }
        }
    }
}
