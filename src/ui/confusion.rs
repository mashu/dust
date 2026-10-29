//! The Common confusions card: what each character was taken for, the two
//! of them in Morse with the elements that differ picked out, and how
//! often, of every one sent. A mix-up heard only once is a chip at the foot,
//! where it does not crowd out the ones that keep happening.

use cw_core::{ConfusionEntry, morse_for};
use dioxus::prelude::*;

use crate::ui::help;
use crate::ui::widgets::{CardHead, IconName};

/// The fewest confusions drawn in full, even when each happened once.
const SHOWN_IN_FULL: usize = 3;

/// One element of a character's Morse, and whether it differs from the
/// element in the same place in the character it was mixed up with: the
/// place where the ear went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mark {
    dah: bool,
    differs: bool,
}

/// `pattern`'s elements, each against `other`'s in the same place; past the
/// end of `other`, every one differs.
fn elements(pattern: &str, other: &str) -> Vec<Mark> {
    let mut other = other.chars();
    pattern
        .chars()
        .map(|mark| Mark {
            dah: mark == '-',
            differs: other.next() != Some(mark),
        })
        .collect()
}

/// A character's Morse, small, its elements set against `other`'s.
fn morse(ch: char, other: Option<char>) -> Element {
    let other = other.and_then(morse_for).unwrap_or("");
    let marks = elements(morse_for(ch).unwrap_or(""), other);
    rsx! {
        span { class: "morse-mini", aria_hidden: "true",
            for mark in marks {
                i {
                    class: match (mark.dah, mark.differs) {
                        (false, false) => "dit",
                        (false, true) => "dit differs",
                        (true, false) => "dah",
                        (true, true) => "dah differs",
                    },
                }
            }
        }
    }
}

/// How many of `rows`, most common first, are drawn in full: every one that
/// happened more than once, and at least a few.
fn in_full(rows: &[ConfusionEntry]) -> usize {
    rows.iter()
        .take_while(|row| row.count > 1)
        .count()
        .max(SHOWN_IN_FULL.min(rows.len()))
}

/// What a screen reader hears for a confusion.
fn confusion_label(row: &ConfusionEntry) -> String {
    let taken = match row.typed {
        Some(typed) => format!("{} taken for {typed}", row.sent),
        None => format!("{} left out", row.sent),
    };
    format!(
        "{taken}: {} {}, {:.0}% of every {} sent",
        row.count,
        if row.count == 1 { "time" } else { "times" },
        row.percentage,
        row.sent
    )
}

/// The card.
pub fn confusions_card(rows: &[ConfusionEntry]) -> Element {
    let (full, once) = rows.split_at(in_full(rows));
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Shuffle,
                title: "Common confusions",
                note: "What each character was taken for, its Morse against yours",
                help: help::confusions(),
            }
            if rows.is_empty() {
                p { class: "muted hint", "No substitution errors yet." }
            }
            div { class: "confusions",
                for row in full.iter() {
                    div { class: "confusion", aria_label: "{confusion_label(row)}",
                        div { class: "confusion-line",
                            span { class: "confusion-chars",
                                span { class: "letter-key group-text", "{row.sent}" }
                                {morse(row.sent, row.typed)}
                                span { class: "confusion-arrow", "→" }
                                if let Some(typed) = row.typed {
                                    span { class: "letter-key typed group-text", "{typed}" }
                                    {morse(typed, Some(row.sent))}
                                } else {
                                    span { class: "letter-key left-out", "—" }
                                    span { class: "confusion-left-out", "left out" }
                                }
                            }
                            span { class: "chip", "{row.count}×" }
                        }
                        div { class: "confusion-share",
                            div { class: "bar-track",
                                span {
                                    class: "bar-fill",
                                    style: "width: {row.percentage.clamp(0.0, 100.0):.1}%;",
                                }
                            }
                            span { class: "tiny confusion-of", "{row.percentage.round()}% of every {row.sent}" }
                        }
                    }
                }
            }
            if !once.is_empty() {
                div { class: "confusion-once",
                    span { class: "tiny", "Once each" }
                    for row in once.iter() {
                        span { class: "chip quiet group-text", aria_label: "{confusion_label(row)}",
                            "{row.sent} → {row.typed.map_or('—', |typed| typed)}"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sent: char, typed: Option<char>, count: u32) -> ConfusionEntry {
        ConfusionEntry {
            sent,
            typed,
            count,
            percentage: 10.0,
        }
    }

    /// The elements that differ are the ones in the same place that do not
    /// match, and every one past the end of the other character.
    #[test]
    fn the_elements_that_differ_are_picked_out() {
        let marks = |pattern, other| -> Vec<(bool, bool)> {
            elements(pattern, other)
                .into_iter()
                .map(|e| (e.dah, e.differs))
                .collect()
        };
        // 2 taken for 1: the second dit heard as a dah.
        assert_eq!(
            marks("..---", ".----"),
            [
                (false, false),
                (false, true),
                (true, false),
                (true, false),
                (true, false)
            ]
        );
        // 5 taken for H: the last dit missed.
        assert_eq!(marks(".....", "....")[4], (false, true));
        assert!(marks("....", ".....").iter().all(|(_, differs)| !differs));
        // Nothing to set it against: every element stands out.
        assert!(marks("-.-", "").iter().all(|(_, differs)| *differs));
    }

    /// Every confusion that happened more than once is drawn in full, and
    /// at least three; the rest are chips.
    #[test]
    fn the_ones_that_keep_happening_are_drawn_in_full() {
        let rows = [
            row('2', Some('1'), 10),
            row('K', Some('R'), 2),
            row('P', Some('1'), 1),
            row('L', Some('0'), 1),
            row('E', None, 1),
        ];
        assert_eq!(in_full(&rows), 3, "at least three");
        let busy = [
            row('2', Some('1'), 10),
            row('K', Some('R'), 2),
            row('E', Some('T'), 2),
            row('N', Some('A'), 2),
            row('P', Some('1'), 1),
        ];
        assert_eq!(in_full(&busy), 4);
        assert_eq!(in_full(&rows[..1]), 1);
        assert_eq!(in_full(&[]), 0);
    }

    #[test]
    fn a_confusion_reads_out_whole() {
        let mut taken = row('2', Some('1'), 10);
        taken.percentage = 29.4;
        assert_eq!(
            confusion_label(&taken),
            "2 taken for 1: 10 times, 29% of every 2 sent"
        );
        assert_eq!(
            confusion_label(&row('E', None, 1)),
            "E left out: 1 time, 10% of every E sent"
        );
    }
}
