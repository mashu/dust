//! The "What comes up next" card: how often you get each character wrong,
//! against your level-up threshold, and each one's chance of coming up
//! next.
//!
//! A row is a bar, drawn as the speed and length bars below it are: as long
//! as the best guess at how often you get the character wrong, in the
//! colour of where it stands, with a notch at your threshold. The trainer
//! does not hold one rate for a character but a belief, narrow for one
//! heard hundreds of times and wide for one just unlocked: a paler stretch
//! past the bar runs to the top of where the rate likely is, and the line
//! under it says the range. A summary above names the characters to work
//! on, the ones it is still finding out about, and the ones not heard yet,
//! which have no row: there is nothing yet to draw of them.

use cw_core::{CharSetMode, SamplingRow, TrainingSettings, sampling_rows};
use dioxus::prelude::*;

use crate::ui::figures::{History, without_band};

use crate::ui::bigram::id_part;
use crate::ui::help;
use crate::ui::widgets::{CardHead, IconName, Seg, control_id};

/// The share of the belief the likely range holds.
const LIKELY: f64 = 0.8;
/// A likely range wider than this, and the trainer is still finding out.
const UNSURE_WIDTH: f64 = 0.2;

/// Where a character stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    /// Under your threshold, and known to be.
    Solid,
    /// Over it.
    ToWorkOn,
    /// Heard, but not yet enough to tell.
    FindingOut,
    /// Never heard.
    New,
}

impl Standing {
    /// How much work it needs, most first: over your threshold, then still
    /// finding out, then never heard, then solid.
    fn work_rank(self) -> u8 {
        match self {
            Self::ToWorkOn => 0,
            Self::FindingOut => 1,
            Self::New => 2,
            Self::Solid => 3,
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Solid => "solid",
            Self::ToWorkOn => "to-work-on",
            Self::FindingOut => "finding-out",
            Self::New => "new",
        }
    }
}

/// The right end of the bars: 60% wrong, past which everything is bad
/// enough, unless your threshold sits near it.
fn scale_top(threshold: f64) -> f64 {
    if threshold > 0.45 { 1.0 } else { 0.6 }
}

/// One character's row.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    character: char,
    /// The best guess at its error rate, and the range it is likely in.
    guess: f64,
    likely: (f64, f64),
    standing: Standing,
    /// Its chance of coming up next; none when the set does not draw it.
    chance: Option<f64>,
}

fn line(row: &SamplingRow, threshold: f64) -> Line {
    let likely = row.error_interval(LIKELY);
    let standing = if row.tries == 0 {
        Standing::New
    } else if likely.1 - likely.0 > UNSURE_WIDTH {
        Standing::FindingOut
    } else if row.p_error > threshold {
        Standing::ToWorkOn
    } else {
        Standing::Solid
    };
    Line {
        character: row.character,
        guess: row.p_error,
        likely,
        standing,
        chance: row.drawn_now.then_some(row.sampling_prob),
    }
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

/// The line under a row's bar: where its rate likely is, and its chance of
/// coming up next.
fn line_note(line: &Line) -> String {
    let (low, high) = line.likely;
    let likely = format!(
        "likely {:.0}–{:.0}% wrong",
        low * 100.0,
        (high * 100.0).max(1.0)
    );
    match line.chance {
        Some(chance) => format!("{likely} · {} chance next", percent(chance)),
        None => format!("{likely} · not drawn now"),
    }
}

/// What a screen reader hears for a row.
fn line_label(line: &Line) -> String {
    let error = if line.standing == Standing::New {
        "not heard yet".to_string()
    } else {
        format!(
            "wrong about {}, likely {} to {}",
            percent(line.guess),
            percent(line.likely.0),
            percent(line.likely.1)
        )
    };
    match line.chance {
        Some(chance) => format!(
            "{}: {error}, {} chance next",
            line.character,
            percent(chance)
        ),
        None => format!("{}: {error}, not drawn now", line.character),
    }
}

/// Where along the bar, in percent.
fn along(rate: f64, top: f64) -> f64 {
    (rate / top * 100.0).clamp(0.0, 100.0)
}

fn row_view(line: &Line, top: f64, threshold: f64) -> Element {
    let class = format!(
        "letter-row stat-row {}{}",
        line.standing.class(),
        if line.chance.is_none() {
            " not-drawn"
        } else {
            ""
        }
    );
    let (low, high) = (along(line.likely.0, top), along(line.likely.1, top));
    rsx! {
        div {
            id: control_id("sample", &id_part(line.character)),
            class,
            role: "listitem",
            aria_label: "{line_label(line)}",
            span { class: "letter-key group-text", "{line.character}" }
            div { class: "bar-track marked",
                span {
                    class: "stat-range",
                    style: "left: {low:.1}%; width: {high - low:.1}%;",
                }
                span {
                    class: "bar-fill stat-fill",
                    style: "width: {along(line.guess, top):.1}%;",
                }
                span {
                    class: "bar-mark",
                    style: "left: {along(threshold, top):.1}%;",
                }
            }
            span { class: "mono muted stat-value", "{percent(line.guess)}" }
            span { class: "tiny", "{line_note(line)}" }
        }
    }
}

/// The order the rows are drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    /// The most work needed first: where they stand, then the worst first.
    MostWork,
    /// The likeliest to come up next first.
    MostLikely,
    /// As taught.
    Taught,
}

/// Put `lines` in `order`; a tie keeps the order taught.
fn sort(lines: &mut [Line], order: Order) {
    match order {
        Order::MostWork => lines.sort_by(|a, b| {
            a.standing
                .work_rank()
                .cmp(&b.standing.work_rank())
                .then(b.guess.total_cmp(&a.guess))
        }),
        Order::MostLikely => {
            lines.sort_by(|a, b| b.chance.unwrap_or(0.0).total_cmp(&a.chance.unwrap_or(0.0)))
        }
        Order::Taught => {}
    }
}

/// The characters of `lines` that stand where `standing` says, as chips.
fn chips(lines: &[Line], standing: Standing) -> Vec<char> {
    lines
        .iter()
        .filter(|line| line.standing == standing)
        .map(|line| line.character)
        .collect()
}

/// How often you get each character wrong, and how sure the trainer is of
/// it, against your level-up threshold; and each one's chance of coming up
/// next: the most work needed first, the likeliest first, or as taught.
/// Those practised before and not drawn now are greyed below.
#[component]
pub fn NextUpCard(settings: TrainingSettings, #[props(into)] sessions: History) -> Element {
    let mut order = use_signal(|| Order::MostWork);
    let threshold = (1.0 - settings.auto_level.auto_adjust_threshold / 100.0).clamp(0.0, 1.0);
    let top = scale_top(threshold);
    let rows = sessions.figure("sampling rows", without_band(&settings), |sessions| {
        sampling_rows(&settings, sessions)
    });
    let (mut drawn, mut others): (Vec<Line>, Vec<Line>) = rows
        .iter()
        .map(|row| line(row, threshold))
        .partition(|line| line.chance.is_some());
    // A character never heard has no row: there is nothing yet to draw of
    // it. The summary names the ones the set draws.
    let unheard = chips(&drawn, Standing::New);
    drawn.retain(|line| line.standing != Standing::New);
    others.retain(|line| line.standing != Standing::New);
    let to_work_on = chips(&drawn, Standing::ToWorkOn);
    let finding_out = chips(&drawn, Standing::FindingOut);
    let solid = chips(&drawn, Standing::Solid).len();
    sort(&mut drawn, order());
    sort(&mut others, order());
    // A contest draws its callers' calls; their exchanges are not drawn.
    let next = if settings.curriculum.char_set_mode == CharSetMode::Contest {
        "turning up in the next caller's call"
    } else {
        "coming up in the next group"
    };
    let limit = percent(threshold);
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Sparkle,
                title: "What comes up next",
                note: "How often you get each character wrong, and its chance of {next}",
                help: help::next_up(threshold),
                div { class: "segmented",
                    for (each , label , id) in [
                        (Order::MostWork, "Most work", "seg-sample-work"),
                        (Order::MostLikely, "Most likely", "seg-sample-likely"),
                        (Order::Taught, "Taught order", "seg-sample-taught"),
                    ]
                    {
                        Seg {
                            label,
                            id: id.to_string(),
                            active: order() == each,
                            onclick: move |_| order.set(each),
                        }
                    }
                }
            }
            if drawn.is_empty() && unheard.is_empty() {
                p { class: "muted hint", "Pool is empty." }
            } else {
                div { class: "err-summary",
                    if !to_work_on.is_empty() {
                        div { class: "err-sum-row",
                            span { class: "err-sum-label to-work-on", "To work on · {to_work_on.len()}" }
                            span { class: "err-chips",
                                for character in to_work_on.iter() {
                                    span { class: "err-chip to-work-on group-text", "{character}" }
                                }
                            }
                        }
                    }
                    if !finding_out.is_empty() {
                        div { class: "err-sum-row",
                            span { class: "err-sum-label finding-out", "Still finding out · {finding_out.len()}" }
                            span { class: "err-chips",
                                for character in finding_out.iter() {
                                    span { class: "err-chip finding-out group-text", "{character}" }
                                }
                            }
                        }
                    }
                    div { class: "err-sum-row",
                        span { class: "err-sum-label solid", "Solid · {solid}" }
                    }
                    if !unheard.is_empty() {
                        div { class: "err-sum-row",
                            span { class: "err-sum-label finding-out", "Not heard yet · {unheard.len()}" }
                            span { class: "err-chips",
                                for character in unheard.iter() {
                                    span { class: "err-chip finding-out group-text", "{character}" }
                                }
                            }
                        }
                    }
                }
                div { class: "err-legend", aria_hidden: "true",
                    span { i { class: "err-swatch solid" } "under your {limit}" }
                    span { i { class: "err-swatch to-work-on" } "over it" }
                    span { i { class: "err-swatch finding-out" } "still finding out" }
                    span { i { class: "err-swatch range" } "where it likely is" }
                    span { i { class: "err-swatch mark" } "your {limit}" }
                }
            }
            div { class: "stat-rows", role: "list",
                for line in drawn.iter() {
                    {row_view(line, top, threshold)}
                }
            }
            if !others.is_empty() {
                div { class: "tiny", style: "margin-top: 0.6rem;", "Not in the current set" }
                p { class: "muted hint",
                    "Practised before, and not drawn now. What you learned of them is kept, and counts again when they come back."
                }
                div { class: "stat-rows", role: "list",
                    for line in others.iter() {
                        {row_view(line, top, threshold)}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row whose belief has `copied` and `missed` beyond the flat prior,
    /// heard as many times.
    fn row(copied: f64, missed: f64, drawn_now: bool) -> SamplingRow {
        let (alpha, beta) = (1.0 + copied, 1.0 + missed);
        SamplingRow {
            character: 'K',
            p_error: beta / (alpha + beta),
            sampling_prob: 0.031,
            alpha,
            beta,
            is_letter: true,
            drawn_now,
            tries: (copied + missed).round() as u32,
        }
    }

    /// Never heard is new; heard a little is still finding out; known well,
    /// it is solid under your threshold and to work on over it. Heard long
    /// ago, its belief faded to next to nothing, it is still finding out
    /// rather than new.
    #[test]
    fn each_character_stands_where_its_belief_puts_it() {
        let standing = |row: SamplingRow| line(&row, 0.1).standing;
        assert_eq!(standing(row(0.0, 0.0, true)), Standing::New);
        assert_eq!(standing(row(4.0, 2.0, true)), Standing::FindingOut);
        assert_eq!(standing(row(190.0, 5.0, true)), Standing::Solid);
        assert_eq!(standing(row(160.0, 40.0, true)), Standing::ToWorkOn);
        let long_ago = SamplingRow {
            tries: 12,
            ..row(0.3, 0.1, true)
        };
        assert_eq!(standing(long_ago), Standing::FindingOut);
    }

    /// The line under the bar says the range its paler stretch runs over,
    /// and the chance next, or that the set does not draw it.
    #[test]
    fn a_row_says_what_it_shows() {
        let known = line(&row(190.0, 10.0, true), 0.1);
        assert_eq!(
            line_label(&known),
            "K: wrong about 5%, likely 4% to 8%, 3% chance next"
        );
        assert_eq!(line_note(&known), "likely 4–8% wrong · 3% chance next");
        let away = line(&row(190.0, 10.0, false), 0.1);
        assert_eq!(line_note(&away), "likely 4–8% wrong · not drawn now");
        let new = line(&row(0.0, 0.0, false), 0.1);
        assert_eq!(line_label(&new), "K: not heard yet, not drawn now");
        assert_eq!(percent(0.004), "<1%");
        assert_eq!(percent(0.0), "0%");
    }

    /// Most work first: over the threshold, the worst first, then still
    /// finding out, then never heard, then solid, the worst of those first.
    #[test]
    fn the_most_work_needed_comes_first() {
        let mut lines: Vec<Line> = [
            ('A', row(190.0, 5.0, true)),
            ('B', row(0.0, 0.0, true)),
            ('C', row(160.0, 40.0, true)),
            ('D', row(4.0, 2.0, true)),
            ('E', row(120.0, 60.0, true)),
            ('F', row(190.0, 10.0, true)),
        ]
        .into_iter()
        .map(|(character, row)| {
            let mut line = line(&row, 0.1);
            line.character = character;
            line
        })
        .collect();
        sort(&mut lines, Order::MostWork);
        let order: String = lines.iter().map(|line| line.character).collect();
        assert_eq!(order, "ECDBFA");
        sort(&mut lines, Order::Taught);
        assert_eq!(
            lines.iter().map(|line| line.character).collect::<String>(),
            "ECDBFA",
            "taught order is the order given"
        );
    }

    #[test]
    fn the_bars_run_to_sixty_percent_unless_the_threshold_is_near_it() {
        assert_eq!(scale_top(0.1), 0.6);
        assert_eq!(scale_top(0.5), 1.0);
        assert_eq!(along(0.3, 0.6), 50.0);
        assert_eq!(along(0.9, 0.6), 100.0);
    }
}
