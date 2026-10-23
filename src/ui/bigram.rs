//! The pairs of characters that go wrong together: the hardest of them as a
//! list, and every pair as a grid of squares, each one a button that shows
//! the pair's card.

use cw_core::{
    BigramCell, BigramHeatmap, CharTally, GROUP_START_BIGRAM_TOKEN, HARD_PAIR_MIN_TRIES,
    HARD_PAIR_RATIO,
};
use dioxus::prelude::*;

use crate::ui::elsewhere::{PAIR_CARD, use_click_elsewhere};
use crate::ui::widgets::{CardHead, IconName, Seg, control_id, html_bool};

/// A pair as the character before and the one after it.
type Pair = (char, char);

/// How a pair reads: "M after K", or "M at the start of a group".
fn pair_name(row: char, col: char) -> String {
    if row == GROUP_START_BIGRAM_TOKEN {
        format!("{col} at the start of a group")
    } else {
        format!("{col} after {row}")
    }
}

/// A character as part of a control's id: itself where it is a letter or a
/// digit, `start` for the start of a group, and its code point otherwise,
/// so that `/` and `?` never share an id.
pub(crate) fn id_part(ch: char) -> String {
    if ch == GROUP_START_BIGRAM_TOKEN {
        "start".to_string()
    } else if ch.is_ascii_alphanumeric() {
        ch.to_string()
    } else {
        format!("u{:04x}", u32::from(ch))
    }
}

/// The id of a pair's square: `pair-k-m`, `pair-start-k`.
fn pair_id(row: char, col: char) -> String {
    control_id("pair", &format!("{} {}", id_part(row), id_part(col)))
}

/// What a screen reader says for a pair's square.
fn pair_label(cell: &BigramCell) -> String {
    let name = pair_name(cell.row, cell.col);
    if cell.total == 0 {
        format!("{name}: not heard yet")
    } else {
        format!("{name}: {} of {} wrong", cell.wrong, cell.total)
    }
}

fn percent(rate: f64) -> String {
    format!("{:.0}%", rate * 100.0)
}

/// One line of a pair's card, drawn the way the calendar's day card draws
/// its lines: what it is, how much, and a bar when it is a share.
#[derive(Clone, Debug, PartialEq)]
struct CardRow {
    label: String,
    value: String,
    note: Option<String>,
    /// The bar's length, from 0 to 1; no bar when it is not a share.
    share: Option<f64>,
    /// The bar's class, which colours it.
    tone: &'static str,
}

/// What a pair's card shows.
#[derive(Clone, Debug, PartialEq)]
struct PairCard {
    title: String,
    rows: Vec<CardRow>,
    /// How the pair stands against its second character overall.
    verdict: String,
}

/// A heard pair's card: how often it went wrong, how often its second
/// character does overall, what it was taken for, and what that says.
fn pair_card(cell: &BigramCell, overall: CharTally) -> PairCard {
    let col = cell.col;
    let mut rows = vec![
        CardRow {
            label: "Wrong".to_string(),
            value: percent(cell.rate),
            note: Some(format!("{} of {}", cell.wrong, cell.total)),
            share: Some(cell.rate),
            tone: "fill-miss",
        },
        CardRow {
            label: format!("{col} overall"),
            value: percent(overall.rate()),
            note: Some(format!("{} of {}", overall.wrong, overall.total)),
            share: Some(overall.rate()),
            tone: "fill-overall",
        },
    ];
    if !cell.taken_for.is_empty() {
        rows.push(CardRow {
            label: "Taken for".to_string(),
            value: taken_for(&cell.taken_for),
            note: None,
            share: None,
            tone: "",
        });
    }
    PairCard {
        title: pair_name(cell.row, col),
        rows,
        verdict: verdict(cell, overall),
    }
}

/// What the character was copied as when it went wrong: `N 2× · left out 1×`.
fn taken_for(typed: &[(Option<char>, u32)]) -> String {
    typed
        .iter()
        .map(|(ch, count)| match ch {
            Some(ch) => format!("{ch} {count}×"),
            None => format!("left out {count}×"),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Whether the pair is the trouble, or the letter: how often it goes wrong
/// here against how often it does overall.
fn verdict(cell: &BigramCell, overall: CharTally) -> String {
    let col = cell.col;
    let usual = overall.rate();
    if cell.wrong == 0 {
        "Copied right every time here.".to_string()
    } else if usual <= 0.0 {
        format!("{col} goes wrong only here.")
    } else if cell.rate >= usual * HARD_PAIR_RATIO {
        format!(
            "Wrong {:.1}× as often as {col} overall: the pair is the trouble.",
            cell.rate / usual
        )
    } else if cell.rate * HARD_PAIR_RATIO <= usual {
        format!("Wrong less often than {col} overall.")
    } else {
        format!("Wrong about as often as {col} overall.")
    }
}

/// The order the grid's rows and columns are drawn in, as indexes into the
/// heatmap's: as taught, or with the most errors first, which gathers the
/// darkest squares into the top left corner. A row counts the errors on the
/// character after it, a column those on its own character; a tie keeps the
/// order taught. Sorting by count rather than by rate keeps a pair heard
/// once and missed from leading the grid.
fn order(heat: &BigramHeatmap, most_errors: bool) -> (Vec<usize>, Vec<usize>) {
    let cols = heat.cols.len();
    let mut rows: Vec<usize> = (0..heat.rows.len()).collect();
    let mut columns: Vec<usize> = (0..cols).collect();
    if most_errors {
        let row_errors = |r: usize| -> u32 {
            heat.cells
                .iter()
                .skip(r * cols)
                .take(cols)
                .map(|cell| cell.wrong)
                .sum()
        };
        rows.sort_by_key(|r| std::cmp::Reverse(row_errors(*r)));
        columns.sort_by_key(|c| {
            std::cmp::Reverse(heat.overall.get(*c).map_or(0, |tally| tally.wrong))
        });
    }
    (rows, columns)
}

/// The pairs that go wrong more often than their second character does
/// overall, where it is the pair that trips you rather than the letter.
pub fn hardest_pairs_card(heat: &BigramHeatmap) -> Element {
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Target,
                title: "Hardest pairs",
                note: "Where it is the pair that trips you, not the letter",
            }
            if heat.hardest.is_empty() {
                p { class: "muted", style: "margin: 0;",
                    "None stands out yet. A pair is listed once it has been heard {HARD_PAIR_MIN_TRIES} times, missed more than once, and goes wrong more often than its second character does overall."
                }
            } else {
                div { class: "confusions",
                    for pair in heat.hardest.iter() {
                        div { class: "confusion",
                            div { class: "confusion-line",
                                span { class: "confusion-chars",
                                    span { class: "letter-key group-text", "{pair.row}" }
                                    span { class: "letter-key typed group-text", "{pair.col}" }
                                    span { class: "pair-name", "{pair_name(pair.row, pair.col)}" }
                                }
                                span { class: "chip", "{pair.wrong} of {pair.total}" }
                            }
                            div { class: "confusion-share",
                                div { class: "bar-track marked",
                                    span {
                                        class: "bar-fill",
                                        style: "width: {pair.rate.clamp(0.0, 1.0) * 100.0:.1}%;",
                                    }
                                    span {
                                        class: "bar-mark",
                                        style: "left: {pair.overall.clamp(0.0, 1.0) * 100.0:.1}%;",
                                    }
                                }
                                span { class: "tiny confusion-of",
                                    "{percent(pair.rate)} here · {pair.col} {percent(pair.overall)} overall"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Every pair as a square, darker the more often its second character went
/// wrong there. A mouse pointing at a square shows its card for as long as
/// it stays; a tap pins it until the same square is tapped again, anywhere
/// else is, or Escape is pressed. Touch has no pointing, so only a mouse's
/// counts. The pair shown lights up its row and column.
///
/// The grid scrolls sideways, which would cut off a card hung on its square,
/// so the card docks above the nav instead, where it stays in sight however
/// far down a tall grid the square is.
#[component]
pub fn BigramCard(heat: BigramHeatmap) -> Element {
    let mut hovered = use_signal(|| None::<Pair>);
    let mut pinned = use_signal(|| None::<Pair>);
    // The most errors first, as every list of characters on Stats starts
    // with the ones that need the most work.
    let mut most_errors = use_signal(|| true);
    use_click_elsewhere(PAIR_CARD, move || {
        if pinned.peek().is_some() {
            pinned.set(None);
        }
    });
    let cols = heat.cols.len();
    let pinned_now = pinned();
    // A pair no longer in the grid, after a history was cut, shows nothing.
    let shown = hovered()
        .or(pinned_now)
        .and_then(|(row, col)| {
            heat.cells
                .iter()
                .find(|cell| (cell.row, cell.col) == (row, col))
        })
        .filter(|cell| cell.total > 0);
    let line = shown.map(|cell| (cell.row, cell.col));
    let card = shown.map(|cell| pair_card(cell, heat.overall_of(cell.col)));
    let (row_order, col_order) = order(&heat, most_errors());
    rsx! {
        div { class: "card stack",
            CardHead {
                icon: IconName::Chart,
                title: "Bigram error rate",
                note: "How often a character goes wrong after each other one",
                div { class: "segmented",
                    Seg {
                        label: "Taught order",
                        active: !most_errors(),
                        onclick: move |_| most_errors.set(false),
                    }
                    Seg {
                        label: "Most errors",
                        active: most_errors(),
                        onclick: move |_| most_errors.set(true),
                    }
                }
            }
            p { class: "muted", style: "margin: 0;",
                "Rows are the character before ({GROUP_START_BIGRAM_TOKEN} = start of group), columns the one after. Darker means more errors; an empty square is a pair not heard yet."
            }
            div { class: "bigram-scroll",
                div {
                    class: "bigram-grid",
                    style: "grid-template-columns: repeat({cols + 1}, minmax(1.35rem, 1fr));",
                    div { class: "bigram-head row-head" }
                    for col in col_order.iter().map(|c| heat.cols[*c]) {
                        div {
                            class: if line.is_some_and(|(_, c)| c == col) { "bigram-head group-text in-line" } else { "bigram-head group-text" },
                            "{col}"
                        }
                    }
                    for r in row_order.iter().copied() {
                        div {
                            class: if line.is_some_and(|(at, _)| at == heat.rows[r]) { "bigram-head row-head group-text in-line" } else { "bigram-head row-head group-text" },
                            "{heat.rows[r]}"
                        }
                        for cell in col_order.iter().map(|c| &heat.cells[r * cols + c]) {
                            {
                                let pair = (cell.row, cell.col);
                                let heard = cell.total > 0;
                                let mut class = vec!["bigram-cell"];
                                if !heard {
                                    class.push("empty");
                                }
                                if line.is_some_and(|(r, c)| r == cell.row || c == cell.col) {
                                    class.push("in-line");
                                }
                                if line == Some(pair) {
                                    class.push("shown");
                                }
                                if pinned_now == Some(pair) {
                                    class.push("selected");
                                }
                                let class = class.join(" ");
                                let alpha = if heat.max_rate <= 0.0 { 0.0 } else { cell.rate / heat.max_rate } * 0.85;
                                let style = heard.then(|| {
                                    format!("background-image: linear-gradient(rgba(180, 83, 9, {alpha:.3}), rgba(180, 83, 9, {alpha:.3}));")
                                });
                                rsx! {
                                    button {
                                        id: pair_id(cell.row, cell.col),
                                        class: class,
                                        style: style,
                                        aria_label: "{pair_label(cell)}",
                                        disabled: html_bool(!heard),
                                        onpointerenter: move |evt: PointerEvent| {
                                            if heard && evt.pointer_type() == "mouse" {
                                                hovered.set(Some(pair));
                                            }
                                        },
                                        onpointerleave: move |evt: PointerEvent| {
                                            if evt.pointer_type() == "mouse" && *hovered.peek() == Some(pair) {
                                                hovered.set(None);
                                            }
                                        },
                                        onclick: move |_| {
                                            if heard {
                                                let again = *pinned.peek() == Some(pair);
                                                pinned.set((!again).then_some(pair));
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
            }
            p { class: "muted", "Tap a pair for details." }
            if let Some(card) = card {
                div { class: "pair-dock",
                    div { class: "pair-card",
                        span { class: "tip-date", "{card.title}" }
                        for row in card.rows {
                            span { class: "tip-row",
                                span { class: "tip-line",
                                    span { class: "tip-label", "{row.label}" }
                                    span { class: "tip-value", "{row.value}" }
                                }
                                if let Some(share) = row.share {
                                    span { class: "tip-bar",
                                        if share > 0.0 {
                                            span {
                                                class: "tip-fill {row.tone}",
                                                style: "width: {share * 100.0:.1}%;",
                                            }
                                        }
                                    }
                                }
                                if let Some(note) = row.note {
                                    span { class: "tip-note", "{note}" }
                                }
                            }
                        }
                        span { class: "pair-verdict", "{card.verdict}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(row: char, col: char, wrong: u32, total: u32) -> BigramCell {
        BigramCell {
            row,
            col,
            rate: f64::from(wrong) / f64::from(total.max(1)),
            total,
            wrong,
            taken_for: Vec::new(),
        }
    }

    /// Most errors first gathers the worst rows and columns to the top
    /// left; a tie keeps the order taught, and taught order is the
    /// heatmap's own.
    #[test]
    fn most_errors_first_sorts_by_count_and_keeps_ties_taught() {
        let rows = vec![GROUP_START_BIGRAM_TOKEN, 'K', 'M'];
        let cols = vec!['K', 'M'];
        let wrong = [[0, 1], [0, 5], [2, 0]];
        let heat = BigramHeatmap {
            cells: rows
                .iter()
                .enumerate()
                .flat_map(|(r, row)| {
                    cols.iter()
                        .enumerate()
                        .map(move |(c, col)| cell(*row, *col, wrong[r][c], 10))
                })
                .collect(),
            overall: vec![
                CharTally {
                    wrong: 2,
                    total: 30,
                },
                CharTally {
                    wrong: 6,
                    total: 30,
                },
            ],
            rows,
            cols,
            max_rate: 0.5,
            hardest: Vec::new(),
        };
        assert_eq!(order(&heat, false), (vec![0, 1, 2], vec![0, 1]));
        // K's row has five errors, M's two, the start's one; M's column six.
        assert_eq!(order(&heat, true), (vec![1, 2, 0], vec![1, 0]));
    }

    #[test]
    fn a_pair_reads_as_the_character_after_the_one_before() {
        assert_eq!(pair_name('K', 'M'), "M after K");
        assert_eq!(
            pair_name(GROUP_START_BIGRAM_TOKEN, 'M'),
            "M at the start of a group"
        );
        assert_eq!(pair_label(&cell('K', 'M', 2, 6)), "M after K: 2 of 6 wrong");
        assert_eq!(
            pair_label(&cell('K', 'M', 0, 0)),
            "M after K: not heard yet"
        );
    }

    /// Every pair has an id of its own, punctuation and the start of a group
    /// included.
    #[test]
    fn every_pair_has_an_id_of_its_own() {
        assert_eq!(pair_id('K', 'M'), "pair-k-m");
        assert_eq!(pair_id(GROUP_START_BIGRAM_TOKEN, '5'), "pair-start-5");
        assert_eq!(pair_id('/', '?'), "pair-u002f-u003f");
        let marks = ['/', '?', '.', ',', '=', '+', GROUP_START_BIGRAM_TOKEN, 'A'];
        let mut ids: Vec<String> = marks
            .iter()
            .flat_map(|row| marks.iter().map(move |col| pair_id(*row, *col)))
            .collect();
        let all = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all);
    }

    #[test]
    fn a_pair_card_holds_it_against_its_letter_overall() {
        let mut km = cell('K', 'M', 3, 6);
        km.taken_for = vec![(Some('N'), 2), (None, 1)];
        let card = pair_card(
            &km,
            CharTally {
                wrong: 5,
                total: 50,
            },
        );
        assert_eq!(card.title, "M after K");
        let lines: Vec<(&str, &str, Option<&str>)> = card
            .rows
            .iter()
            .map(|row| (row.label.as_str(), row.value.as_str(), row.note.as_deref()))
            .collect();
        assert_eq!(
            lines,
            [
                ("Wrong", "50%", Some("3 of 6")),
                ("M overall", "10%", Some("5 of 50")),
                ("Taken for", "N 2× · left out 1×", None),
            ]
        );
        assert_eq!(card.rows[0].share, Some(0.5));
        assert_eq!(card.rows[2].share, None, "no bar for what it was taken for");
        assert_eq!(
            card.verdict,
            "Wrong 5.0× as often as M overall: the pair is the trouble."
        );
    }

    #[test]
    fn the_verdict_says_whether_the_pair_or_the_letter_is_the_trouble() {
        let overall = CharTally {
            wrong: 2,
            total: 10,
        };
        let said = |wrong, total| verdict(&cell('K', 'M', wrong, total), overall);
        assert_eq!(said(0, 4), "Copied right every time here.");
        assert_eq!(said(1, 5), "Wrong about as often as M overall.");
        assert_eq!(said(1, 10), "Wrong less often than M overall.");
        assert_eq!(
            said(4, 10),
            "Wrong 2.0× as often as M overall: the pair is the trouble."
        );
        // A card always has a tally under it, but nothing divides by none.
        assert_eq!(
            verdict(&cell('K', 'M', 1, 2), CharTally::default()),
            "M goes wrong only here."
        );
        let card = pair_card(&cell('K', 'M', 0, 4), overall);
        assert_eq!(card.rows.len(), 2, "nothing was taken for anything");
    }
}
