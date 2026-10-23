//! The "What comes up next" card: a tile for every character the next
//! group can draw, coloured by how often you get it wrong, green through
//! yellow to red, with a bar of its chance of coming up next. The sampler
//! draws the characters you miss more often, and the tiles show it doing
//! so at a glance, where a list of rows took a screen and a half.

use cw_core::{CharSetMode, SamplingRow, SessionResult, TrainingSettings, sampling_rows};
use dioxus::prelude::*;

use crate::ui::bigram::id_part;
use crate::ui::widgets::{CardHead, IconName, Seg, control_id};

/// The error rate the colour scale reaches red at: past it, a character is
/// as red as it gets. At 90% copy, the usual level-up threshold, a
/// character is yellow-green.
const RED_AT: f64 = 0.4;

/// Less than this much evidence and a character counts as new: its error
/// is still the prior's even chance, not anything measured.
const NEW_BELOW: f64 = 0.5;

/// The hue of an error rate: 120 (green) for none, 0 (red) at [`RED_AT`] and
/// past it.
fn error_hue(p_error: f64) -> f64 {
    120.0 * (1.0 - p_error / RED_AT).clamp(0.0, 1.0)
}

fn is_new(row: &SamplingRow) -> bool {
    row.evidence() < NEW_BELOW
}

/// A share as a percentage, and under one percent as such rather than as
/// nothing.
fn percent(share: f64) -> String {
    let pct = share * 100.0;
    if pct > 0.0 && pct < 0.5 {
        "<1%".to_string()
    } else {
        format!("{pct:.0}%")
    }
}

/// What a screen reader hears for a tile.
fn tile_label(row: &SamplingRow) -> String {
    let error = if is_new(row) {
        "not heard yet".to_string()
    } else {
        format!("wrong {}", percent(row.p_error))
    };
    if row.drawn_now {
        format!(
            "{}: {}, {} chance next",
            row.character,
            error,
            percent(row.sampling_prob)
        )
    } else {
        format!("{}: {}, not drawn now", row.character, error)
    }
}

/// One character's tile.
fn tile(row: &SamplingRow, max_chance: f64) -> Element {
    let new = is_new(row);
    let class = match (new, row.drawn_now) {
        (false, true) => "sample-tile",
        (true, true) => "sample-tile new",
        (false, false) => "sample-tile not-drawn",
        (true, false) => "sample-tile new not-drawn",
    };
    let width = if max_chance > 0.0 {
        (row.sampling_prob / max_chance * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    rsx! {
        div {
            id: control_id("sample", &id_part(row.character)),
            class,
            role: "listitem",
            style: "--hue: {error_hue(row.p_error):.0};",
            aria_label: "{tile_label(row)}",
            span { class: "sample-char group-text", "{row.character}" }
            span { class: "sample-err",
                if new {
                    "new"
                } else {
                    "{percent(row.p_error)}"
                }
            }
            if row.drawn_now {
                span { class: "sample-bar",
                    span { class: "sample-fill", style: "width: {width:.1}%;" }
                }
                span { class: "sample-next", "{percent(row.sampling_prob)} next" }
            }
        }
    }
}

/// The characters the next group can draw, as tiles, in the order taught
/// or the most likely first; then those practised before and not drawn now,
/// greyed.
#[component]
pub fn SamplingTilesCard(settings: TrainingSettings, sessions: Vec<SessionResult>) -> Element {
    let mut likely_first = use_signal(|| false);
    let rows = sampling_rows(&settings, &sessions);
    let max_chance = rows
        .iter()
        .map(|row| row.sampling_prob)
        .fold(0.0_f64, f64::max);
    let (mut drawn, others): (Vec<&SamplingRow>, Vec<&SamplingRow>) =
        rows.iter().partition(|row| row.drawn_now);
    if likely_first() {
        drawn.sort_by(|a, b| b.sampling_prob.total_cmp(&a.sampling_prob));
    }
    // A contest draws its callers' calls; their exchanges are not drawn.
    let next = if settings.curriculum.char_set_mode == CharSetMode::Contest {
        "turning up in the next caller's call"
    } else {
        "coming up in the next group"
    };
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Sparkle,
                title: "What comes up next",
                note: "The trainer draws the characters you miss more often",
                div { class: "segmented",
                    Seg {
                        label: "Taught order",
                        id: "seg-sample-taught".to_string(),
                        active: !likely_first(),
                        onclick: move |_| likely_first.set(false),
                    }
                    Seg {
                        label: "Most likely",
                        id: "seg-sample-likely".to_string(),
                        active: likely_first(),
                        onclick: move |_| likely_first.set(true),
                    }
                }
            }
            p { class: "muted", style: "margin: 0;",
                "Each tile's colour is how often you get the character wrong, its bar its chance of {next}. What every character has taught counts, whatever set it was sent in."
            }
            div { class: "sample-legend", aria_hidden: "true",
                span { class: "tiny", "Right" }
                for p_error in [0.0, 0.1, 0.2, 0.3, 0.4] {
                    span { class: "sample-swatch", style: "--hue: {error_hue(p_error):.0};" }
                }
                span { class: "tiny", "Wrong" }
                span { class: "sample-swatch new" }
                span { class: "tiny", "New" }
            }
            if drawn.is_empty() {
                p { class: "muted", style: "margin: 0;", "Pool is empty." }
            }
            div { class: "sample-grid", role: "list",
                for row in drawn {
                    {tile(row, max_chance)}
                }
            }
            if !others.is_empty() {
                div { class: "tiny", style: "margin-top: 0.4rem;", "Not in the current set" }
                p { class: "muted", style: "margin: 0;",
                    "Practised before, and not drawn now. What you learned of them is kept, and counts again when they come back."
                }
                div { class: "sample-grid", role: "list",
                    for row in others {
                        {tile(row, max_chance)}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(p_error: f64, heard: f64, drawn_now: bool) -> SamplingRow {
        SamplingRow {
            character: 'K',
            p_error,
            sampling_prob: 0.031,
            alpha: 1.0 + heard,
            beta: 1.0,
            is_letter: true,
            drawn_now,
        }
    }

    /// Green when right every time, yellow at a fifth wrong, red from two
    /// in five wrong on.
    #[test]
    fn the_colour_runs_from_green_through_yellow_to_red() {
        assert_eq!(error_hue(0.0), 120.0);
        assert_eq!(error_hue(0.2), 60.0);
        assert_eq!(error_hue(0.4), 0.0);
        assert_eq!(error_hue(0.9), 0.0);
        assert_eq!(error_hue(-1.0), 120.0);
    }

    #[test]
    fn a_tile_reads_out_what_it_shows() {
        assert_eq!(
            tile_label(&row(0.07, 12.0, true)),
            "K: wrong 7%, 3% chance next"
        );
        assert_eq!(
            tile_label(&row(0.5, 0.0, true)),
            "K: not heard yet, 3% chance next"
        );
        assert_eq!(
            tile_label(&row(0.2, 5.0, false)),
            "K: wrong 20%, not drawn now"
        );
        assert_eq!(percent(0.004), "<1%");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.031), "3%");
    }
}
