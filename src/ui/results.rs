use cw_core::{BOARD_MIN_GROUPS, CharSetMode, SessionResult};
use dioxus::prelude::*;

use crate::ui::contest_results::ContestResults;
use crate::ui::format::ordinal;
use crate::ui::scores::{Medal, mode_noun, per_group_label};
use crate::ui::widgets::{CardHead, GroupResultRow, Icon, IconName, ScoreRing, control_id};

/// What the session's place on the score board says, and the medal it
/// earns, if any: among the `noun` sessions kept on this device, or the
/// `noun` runs, by what each earned a group.
pub(crate) fn place_line(
    place: usize,
    of: usize,
    noun: &str,
    unit: &str,
    per_group: f64,
) -> (String, Option<usize>) {
    let medal = (place <= 3).then_some(place);
    let rating = per_group_label(per_group);
    let text = if of == 1 {
        format!("Your first {noun} {unit} on the board — {rating} points a group")
    } else if place == 1 {
        format!("New personal best — {rating} points a group, the top of your {of} {noun} {unit}s")
    } else {
        format!(
            "{} best of your {of} {noun} {unit}s — {rating} points a group",
            ordinal(place)
        )
    };
    (text, medal)
}

/// The banner that says where a session stands on the board, its medal or
/// its place beside the words.
#[component]
pub(crate) fn PlaceBanner(text: String, place: usize, medal: Option<usize>) -> Element {
    rsx! {
        div { class: if medal == Some(1) { "place-banner best" } else { "place-banner" },
            if let Some(medal) = medal {
                Medal { place: medal }
            } else {
                span { class: "board-number", "{place}" }
            }
            span { "{text}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::place_line;

    #[test]
    fn a_place_is_said_among_the_sessions_of_its_mode() {
        assert_eq!(
            place_line(1, 1, "callsign", "session", 64.0),
            (
                "Your first callsign session on the board — 64.0 points a group".to_string(),
                Some(1)
            )
        );
        assert_eq!(
            place_line(1, 8, "Koch", "session", 72.44).0,
            "New personal best — 72.4 points a group, the top of your 8 Koch sessions"
        );
        assert_eq!(
            place_line(3, 8, "Koch", "session", 60.0),
            (
                "3rd best of your 8 Koch sessions — 60.0 points a group".to_string(),
                Some(3)
            )
        );
        assert_eq!(
            place_line(12, 40, "word", "session", 10.0).1,
            None,
            "medals go to three"
        );
        assert_eq!(
            place_line(3, 14, "CQ WPX", "run", 20.7).0,
            "3rd best of your 14 CQ WPX runs — 20.7 points a group"
        );
    }
}

#[component]
pub fn ResultsView(
    result: SessionResult,
    /// Where the session stands on the score board, and of how many.
    #[props(default)]
    place: Option<(usize, usize)>,
    auto_message: Option<String>,
    on_again: EventHandler<()>,
    on_home: EventHandler<()>,
) -> Element {
    // A contest run has results of its own: its QSOs, its rate and its
    // contest score. It moves no level, so it has no message.
    if result.char_set_mode == CharSetMode::Contest {
        return rsx! {
            ContestResults {
                result,
                place,
                on_again: Some(on_again),
                on_back: on_home,
            }
        };
    }
    let acc_pct = (result.accuracy * 100.0).round();
    let correct = result.groups.iter().filter(|g| g.correct).count();
    let avg = if result.avg_response_ms > 0.0 {
        format!("{}ms", result.avg_response_ms.round())
    } else {
        "—".into()
    };
    // About the copy, not the level: whether the level moves is the
    // learner's own rule, and the banner below says so when it does.
    let verdict = if result.accuracy >= 0.95 {
        "Clean copy — that pace is yours."
    } else if result.accuracy >= 0.9 {
        "Solid session — a few slips, nothing that stuck."
    } else if result.accuracy >= 0.7 {
        "Getting there — the weak letters come round more often now."
    } else {
        "Rough band. Slow the speed or shrink the pool for a run."
    };
    rsx! {
        div { class: "stack",
            div { class: "card",
                div { class: "score-hero",
                    ScoreRing { pct: acc_pct, value: format!("{acc_pct}%"), caption: "ACCURACY".to_string() }
                    div { class: "score-copy",
                        h2 { "Session complete" }
                        p { class: "muted", style: "margin: 0;", "{verdict}" }
                    }
                }
            }
            if let (Some((place, of)), Some(per_group)) = (place, cw_core::per_group(&result)) {
                {
                    let (text, medal) = place_line(place, of, mode_noun(result.char_set_mode), "session", per_group);
                    rsx! {
                        PlaceBanner { text, place, medal }
                    }
                }
            } else if result.groups.len() < BOARD_MIN_GROUPS && result.score > 0.0 {
                p { class: "muted place-note",
                    "Sessions of {BOARD_MIN_GROUPS} groups or more go on the score board."
                }
            }
            if let Some(msg) = auto_message {
                div { class: "auto-banner",
                    Icon { name: IconName::Sparkle }
                    span { "{msg}" }
                }
            }
            div { class: "grid-3",
                div { class: "kpi blue",
                    div { class: "tiny", "Groups" }
                    div { class: "value", "{correct}/{result.groups.len()}" }
                }
                div { class: "kpi purple",
                    div { class: "tiny", "Avg time" }
                    div { class: "value", "{avg}" }
                }
                div { class: "kpi indigo",
                    div { class: "tiny", "Points" }
                    div { class: "value", "{result.score.round()}" }
                }
            }
            div { class: "card",
                CardHead {
                    icon: IconName::Letters,
                    title: "Group by group",
                    note: "Sent on top, your copy below.",
                }
                div { class: "result-rows",
                    for (idx, group) in result.groups.iter().cloned().enumerate() {
                        GroupResultRow { index: idx, group }
                    }
                }
            }
            div { class: "hero-actions", style: "justify-content: center;",
                button {
                    id: control_id("btn", "train again"),
                    class: "btn btn-primary",
                    onclick: move |_| on_again.call(()),
                    Icon { name: IconName::Repeat }
                    "Train again"
                }
                button {
                    id: control_id("btn", "back"),
                    class: "btn btn-secondary",
                    onclick: move |_| on_home.call(()),
                    Icon { name: IconName::Back }
                    "Back"
                }
            }
        }
    }
}
