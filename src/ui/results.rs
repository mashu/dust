use cw_core::{BOARD_MIN_GROUPS, CharSetMode, REACTION_MIN_MS, SessionResult};
use dioxus::prelude::*;

use crate::ui::contest_results::ContestResults;
use crate::ui::format::ordinal;
use crate::ui::scores::{Medal, mode_noun, per_group_label};
use crate::ui::widgets::{CardHead, GroupResultRow, Icon, IconName, ScoreRing, control_id};

/// What the session's place on the score board says, and the medal it
/// earns, if any: among the `noun` sessions kept on this device, or the
/// `noun` runs, those of the length `of_length` says where they are ranked
/// by it, by what each is ranked by, said in `rating`: "72.4 points a
/// group", "1312 verified points".
pub(crate) fn place_line(
    place: usize,
    of: usize,
    noun: &str,
    unit: &str,
    of_length: &str,
    rating: &str,
) -> (String, Option<usize>) {
    let medal = (place <= 3).then_some(place);
    let text = if of == 1 {
        format!("Your first {noun} {unit}{of_length} on the board — {rating}")
    } else if place == 1 {
        format!("New personal best — {rating}, the top of your {of} {noun} {unit}s{of_length}")
    } else {
        format!(
            "{} best of your {of} {noun} {unit}s{of_length} — {rating}",
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
            place_line(1, 1, "callsign", "session", "", "64.0 points a group"),
            (
                "Your first callsign session on the board — 64.0 points a group".to_string(),
                Some(1)
            )
        );
        assert_eq!(
            place_line(1, 8, "Koch", "session", "", "72.4 points a group").0,
            "New personal best — 72.4 points a group, the top of your 8 Koch sessions"
        );
        assert_eq!(
            place_line(3, 8, "Koch", "session", "", "60.0 points a group"),
            (
                "3rd best of your 8 Koch sessions — 60.0 points a group".to_string(),
                Some(3)
            )
        );
        assert_eq!(
            place_line(12, 40, "word", "session", "", "10.0 points a group").1,
            None,
            "medals go to three"
        );
        assert_eq!(
            place_line(3, 14, "CQ WPX", "run", "", "20.7 points a group").0,
            "3rd best of your 14 CQ WPX runs — 20.7 points a group"
        );
        assert_eq!(
            place_line(
                1,
                1,
                "CQ WPX",
                "run",
                " of 10 minutes",
                "312 verified points"
            )
            .0,
            "Your first CQ WPX run of 10 minutes on the board — 312 verified points"
        );
        assert_eq!(
            place_line(
                2,
                5,
                "CQ WPX",
                "run",
                " of 10 minutes",
                "280 verified points"
            )
            .0,
            "2nd best of your 5 CQ WPX runs of 10 minutes — 280 verified points"
        );
    }
}

/// Why little of the session was timed, when under about a third of its
/// characters were, as far as what it kept says, and what would have them
/// timed (docs/recognition.md); and whether the lock it ran with is why.
/// `lock_on` is whether the settings still lock typing while a group is
/// sent.
fn untimed_hint(result: &SessionResult, lock_on: bool) -> Option<(String, bool)> {
    let untimed = cw_core::session_untimed(result);
    if untimed.chars == 0 || untimed.timed * 3 >= untimed.chars {
        return None;
    }
    let lead = match untimed.timed {
        0 => "Nothing was timed".to_string(),
        1 => format!("Only 1 of {} characters was timed", untimed.chars),
        timed => format!("Only {timed} of {} characters were timed", untimed.chars),
    };
    let longer = result
        .groups
        .iter()
        .any(|group| group.sent.chars().count() > 1);
    if result.typing_locked && longer {
        let now = if lock_on {
            "To time each character, turn it off and key each one as you hear it."
        } else {
            "It is off now: key each character as you hear it."
        };
        return Some((
            format!(
                "{lead}: Lock typing while sending was on, so each group was typed only after it had been sent, which shows how well you remember it rather than how fast you recognise it. {now}"
            ),
            true,
        ));
    }
    let groups = untimed.groups;
    // In every group, or in so many of them.
    let in_groups = |some: usize| match some {
        all if all == groups => "in every group".to_string(),
        some => format!("in {some} of {groups} groups"),
    };
    let text = if untimed.held_up * 2 > groups {
        format!(
            "{lead}: the sound was held up on its way {}, so when their characters reached you is not known. That is the audio, not your typing.",
            in_groups(untimed.held_up)
        )
    } else if let Some(late) = untimed.first_key_late_ms {
        format!(
            "{lead}: {}, your first key came about {:.1} s after the group had ended. A group is timed only when you key its first character while it is still sounding.",
            in_groups(untimed.typed_after),
            late / 1_000.0
        )
    } else if untimed.keyed_in_repeat > 0 {
        format!(
            "{lead}: keys during a repeat are not timed, as by then you had heard the group once. Key each character as you hear it the first time it is sent."
        )
    } else {
        format!(
            "{lead}: key each character as you hear it. A character is timed when its key comes at least {REACTION_MIN_MS} ms after its sound, and a group only when you key its first character while it is still sounding."
        )
    };
    Some((text, false))
}

#[component]
pub fn ResultsView(
    result: SessionResult,
    /// Where the session stands on the score board, and of how many.
    #[props(default)]
    place: Option<(usize, usize)>,
    auto_message: Option<String>,
    /// Turn off Lock typing while sending: offered while the settings still
    /// have it on, when the session ran with it and it is why nothing was
    /// timed.
    #[props(default)]
    on_unlock: Option<EventHandler<()>>,
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
    // How fast its characters were recognised, from their sound to their
    // key, as far as any was timed (docs/recognition.md).
    let (recognised, timed) = match cw_core::session_recognition(&result) {
        Some(recognition) => (
            format!("{:.0} ms", recognition.median_ms),
            format!("{} timed", recognition.timed),
        ),
        None => ("—".to_string(), "none timed".to_string()),
    };
    let untimed = untimed_hint(&result, on_unlock.is_some());
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
                        p { class: "muted hint", "{verdict}" }
                    }
                }
            }
            if let (Some((place, of)), Some(per_group)) = (place, cw_core::per_group(&result)) {
                {
                    let rating = format!("{} points a group", per_group_label(per_group));
                    let (text, medal) = place_line(place, of, mode_noun(result.char_set_mode), "session", "", &rating);
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
                    div { class: "tiny", "Recognised in" }
                    div { class: "value", "{recognised}" }
                    div { class: "tiny", "{timed}" }
                }
                div { class: "kpi indigo",
                    div { class: "tiny", "Points" }
                    div { class: "value", "{result.score.round()}" }
                }
            }
            if let Some((hint, locked)) = untimed {
                div { class: "stack-sm",
                    p { id: "untimed-hint", class: "muted", style: "margin: 0; font-size: 0.85rem;",
                        "{hint}"
                    }
                    if let Some(on_unlock) = on_unlock.filter(|_| locked) {
                        button {
                            id: control_id("btn", "turn off lock typing"),
                            class: "btn btn-secondary btn-sm",
                            style: "align-self: flex-start;",
                            onclick: move |_| on_unlock.call(()),
                            "Turn off Lock typing"
                        }
                    }
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
