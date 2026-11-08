//! Where a session or a run stands on the board, as the screens say it:
//! its medal, its place, and what the board ranks it by.

use cw_core::{CharSetMode, Ranking};
use dioxus::prelude::*;

use crate::ui::format::ordinal;

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

/// What a session of `mode` is called in a sentence: "callsign sessions".
pub fn mode_noun(mode: CharSetMode) -> &'static str {
    match mode {
        CharSetMode::Koch => "Koch",
        CharSetMode::Digits => "digit",
        CharSetMode::Mixed => "mixed",
        CharSetMode::Callsign => "callsign",
        CharSetMode::Words => "word",
        CharSetMode::Contest => "contest",
    }
}

/// What a session earned a group, as the board shows it: "72.4".
pub fn per_group_label(per_group: f64) -> String {
    format!("{per_group:.1}")
}

/// What a board ranks by, as the board shows it: "72.4" points a group, a
/// verified score of "1312", "86" clean QSOs an hour.
pub fn rating_label(ranking: Ranking, rating: f64) -> String {
    match ranking {
        Ranking::PerGroup => per_group_label(rating),
        Ranking::Verified(_) | Ranking::CleanRate => format!("{rating:.0}"),
    }
}

/// What follows a rating's figure: "points a group".
pub fn rating_unit(ranking: Ranking) -> &'static str {
    match ranking {
        Ranking::PerGroup => "points a group",
        Ranking::Verified(_) => "verified points",
        Ranking::CleanRate => "clean QSOs an hour",
    }
}

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
