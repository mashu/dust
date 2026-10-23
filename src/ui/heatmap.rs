use cw_core::{
    HeatmapCell, HeatmapColorMode, HeatmapGrid, SessionResult, StreakState, StreakStatus,
};
use dioxus::prelude::*;

use crate::ui::format::{counted, long_date, month_name};
use crate::ui::widgets::{CardHead, IconName, Seg, control_id, html_bool};

#[component]
pub fn StreakCard(status: StreakStatus) -> Element {
    let freeze = if status.freezes_available > 0 {
        Some(status.freezes_available)
    } else {
        None
    };
    // No streak is no card. Answering that here rather than in a guard above
    // means there is no arm left to be unreachable — and no panic a later edit
    // to the guard could reach.
    let Some((class, emoji, body)) = (match status.state {
        StreakState::Safe => Some({
            let used = if status.freezes_used > 0 {
                format!(" ({} used)", counted(status.freezes_used, "freeze"))
            } else {
                String::new()
            };
            (
                "streak safe",
                "🔥",
                format!("{}-day streak — today is in the bag.{used}", status.days),
            )
        }),
        StreakState::AtRisk => Some((
            "streak risk",
            "⚠️",
            format!(
                "{}-day streak at risk — practice today to keep it alive.",
                status.days
            ),
        )),
        StreakState::Lost => Some((
            "streak lost",
            "💔",
            format!(
                "Your {}-day streak ended — one session today starts the next one.",
                status.lost_streak_days.unwrap_or(0)
            ),
        )),
        StreakState::None => None,
    }) else {
        return rsx! {};
    };
    rsx! {
        div { class: class,
            span { class: "streak-emoji", "{emoji}" }
            p { "{body}" }
            if let Some(count) = freeze {
                span { class: "freeze-chip", title: "A freeze covers one missed day. Earn one every 7 practiced days.", "🧊 ×{count}" }
            }
        }
    }
}

/// The shade of a day on the practice calendar: red through yellow to green
/// as `intensity` runs from 0 to 1. One formula for the grid and its legend.
fn heat_color(intensity: f64) -> String {
    format!("hsl({:.0}, 75%, 45%)", 120.0 * intensity.clamp(0.0, 1.0))
}

#[component]
pub fn ActivityHeatmap(sessions: Vec<SessionResult>, today: String) -> Element {
    let mut mode = use_signal(|| HeatmapColorMode::Volume);
    let Some(grid) = cw_core::build_heatmap(&sessions, &today, cw_core::HEATMAP_MONTHS, mode())
    else {
        return rsx! {};
    };
    let legend = if mode() == HeatmapColorMode::Volume {
        "More chars"
    } else {
        "Better copy"
    };
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Timer,
                title: "Practice calendar",
                note: "One square per day",
                div { class: "segmented",
                    Seg {
                        label: "Volume".to_string(),
                        active: mode() == HeatmapColorMode::Volume,
                        onclick: move |_| mode.set(HeatmapColorMode::Volume),
                    }
                    Seg {
                        label: "Accuracy".to_string(),
                        active: mode() == HeatmapColorMode::Accuracy,
                        onclick: move |_| mode.set(HeatmapColorMode::Accuracy),
                    }
                }
            }
            HeatmapGridView { grid: grid }
            div { class: "heatmap-legend",
                span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "Less" }
                span { class: "heat-cell" }
                for intensity in [0.0, 0.5, 1.0] {
                    span { class: "heat-cell", style: "background: {heat_color(intensity)};" }
                }
                span { class: "tiny", style: "text-transform: none; letter-spacing: 0;", "{legend}" }
            }
            p { class: "muted", "Tap a day for details." }
        }
    }
}

/// Weekday headings over each month's columns, Monday first.
const WEEKDAYS: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];

/// What a screen reader says for a day's square.
fn day_summary(label: &str, cell: &HeatmapCell) -> String {
    if cell.sessions == 0 {
        return format!("{label}: no practice");
    }
    let mut text = format!(
        "{label}: {}, {}",
        counted(cell.chars, "character"),
        counted(cell.sessions, "session")
    );
    if let Some(accuracy) = cell.avg_accuracy {
        text.push_str(&format!(", {:.0}% accuracy", accuracy * 100.0));
    }
    text
}

/// One line of a day's card: what it is, how much, and a bar of how that
/// stands against the most any day on the calendar had.
#[derive(Clone, Debug, PartialEq)]
struct TipRow {
    label: &'static str,
    value: String,
    note: Option<String>,
    /// The bar's length, from 0 to 1.
    share: f64,
    /// The bar's class, which colours it.
    tone: &'static str,
    /// A colour of its own instead, for accuracy: the calendar's.
    heat: Option<String>,
}

fn share(value: f64, most: f64) -> f64 {
    if most > 0.0 && value.is_finite() {
        (value / most).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// What a day's card shows, line by line. A day without practice has none.
fn tip_rows(cell: &HeatmapCell, grid: &HeatmapGrid) -> Vec<TipRow> {
    if cell.sessions == 0 {
        return Vec::new();
    }
    let several = cell.sessions > 1;
    let mut rows = vec![
        TipRow {
            label: "Characters",
            value: cell.chars.to_string(),
            note: None,
            share: share(f64::from(cell.chars), f64::from(grid.max_chars)),
            tone: "fill-chars",
            heat: None,
        },
        TipRow {
            label: "Sessions",
            value: cell.sessions.to_string(),
            note: None,
            share: share(f64::from(cell.sessions), f64::from(grid.max_sessions)),
            tone: "fill-sessions",
            heat: None,
        },
        TipRow {
            label: "Groups",
            value: cell.group_count.to_string(),
            note: (cell.group_count > 0).then(|| {
                let per_group = format!(
                    "~{:.1} chars/group",
                    f64::from(cell.chars) / f64::from(cell.group_count)
                );
                if several {
                    format!(
                        "~{:.1}/session · {per_group}",
                        f64::from(cell.group_count) / f64::from(cell.sessions)
                    )
                } else {
                    per_group
                }
            }),
            share: share(f64::from(cell.group_count), f64::from(grid.max_groups)),
            tone: "fill-groups",
            heat: None,
        },
    ];
    if let Some(char_wpm) = cell.char_wpm {
        // Farnsworth: characters at one speed, spaced out to a slower one.
        let spaced = cell
            .effective_wpm
            .filter(|effective| char_wpm - effective >= 0.5);
        rows.push(TipRow {
            label: "Speed",
            value: format!("{char_wpm:.0} WPM"),
            note: spaced.map(|effective| format!("spaced to {effective:.0} WPM")),
            share: share(char_wpm, grid.max_char_wpm),
            tone: "fill-speed",
            heat: None,
        });
    }
    if let Some(accuracy) = cell.avg_accuracy {
        rows.push(TipRow {
            label: "Accuracy",
            value: format!(
                "{:.1}%{}",
                accuracy * 100.0,
                if several { " avg" } else { "" }
            ),
            note: None,
            share: accuracy.clamp(0.0, 1.0),
            tone: "fill-accuracy",
            heat: Some(heat_color(accuracy)),
        });
    }
    if let Some(score) = cell.best_score {
        rows.push(TipRow {
            label: if several { "Best points" } else { "Points" },
            value: format!("{score:.0}"),
            note: None,
            share: share(score, grid.max_score),
            tone: "fill-score",
            heat: None,
        });
    }
    rows
}

/// Where a day's card goes: on the square it is for, kept on screen.
#[derive(Clone, Debug, PartialEq)]
struct TipAt {
    date: String,
    /// The square's left edge, from the window's, in CSS pixels.
    cell_left: f64,
    /// Below the square, because the window has no room for it above.
    below: bool,
}

/// The room a card needs above its square, in CSS pixels. A square nearer
/// the top of the window than this opens its card below it.
const TIP_ROOM_PX: f64 = 300.0;

impl TipAt {
    /// From where an event happened, in the window and in the square: the
    /// square's own corner is the one less the other.
    fn new(date: &str, in_window: (f64, f64), in_square: (f64, f64)) -> Self {
        let left = in_window.0 - in_square.0;
        let top = in_window.1 - in_square.1;
        Self {
            date: date.to_string(),
            cell_left: if left.is_finite() { left.max(0.0) } else { 0.0 },
            below: top.is_nan() || top < TIP_ROOM_PX,
        }
    }
}

/// A day's card, opened by pointing at its square with a mouse or pinned by
/// tapping it. It is the square's own child, so it scrolls with it; it is
/// hidden from screen readers, which hear the square's label instead.
fn day_tip(label: &str, cell: &HeatmapCell, grid: &HeatmapGrid, at: &TipAt) -> Element {
    let class = if at.below { "day-tip below" } else { "day-tip" };
    let rows = tip_rows(cell, grid);
    rsx! {
        span {
            class: class,
            role: "tooltip",
            aria_hidden: "true",
            style: "--cell-left: {at.cell_left:.0}px;",
            span { class: "tip-date", "{label}" }
            if rows.is_empty() {
                span { class: "tip-empty", "No practice" }
            }
            for row in rows {
                span { class: "tip-row",
                    span { class: "tip-line",
                        span { class: "tip-label", "{row.label}" }
                        span { class: "tip-value", "{row.value}" }
                    }
                    span { class: "tip-bar",
                        if row.share > 0.0 {
                            span {
                                class: "tip-fill {row.tone}",
                                style: {
                                    let width = format!("width: {:.1}%;", row.share * 100.0);
                                    match &row.heat {
                                        Some(colour) => format!("{width} background: {colour};"),
                                        None => width,
                                    }
                                },
                            }
                        }
                    }
                    if let Some(note) = row.note {
                        span { class: "tip-note", "{note}" }
                    }
                }
            }
        }
    }
}

/// Where the calendar keeps its listener on the page.
const ELSEWHERE_SLOT: &str = "__dustDayCard";

/// The script that tells the calendar of each click that lands anywhere but
/// on a day's square, a button (the legend's swatches are not), and each
/// press of Escape. A listener left there before is taken off first.
fn elsewhere_script() -> String {
    format!(
        r#"{}
const clicked = (e) => {{
    if (!(e.target instanceof Element && e.target.closest("button.heat-cell"))) {{
        dioxus.send(true);
    }}
}};
const keyed = (e) => {{
    if (e.key === "Escape") {{
        dioxus.send(true);
    }}
}};
document.addEventListener("click", clicked);
document.addEventListener("keydown", keyed);
window.{ELSEWHERE_SLOT} = () => {{
    document.removeEventListener("click", clicked);
    document.removeEventListener("keydown", keyed);
}};"#,
        stop_listening()
    )
}

/// The script that takes the listener off again.
fn stop_listening() -> String {
    format!(
        r#"if (window.{ELSEWHERE_SLOT}) {{
    window.{ELSEWHERE_SLOT}();
    window.{ELSEWHERE_SLOT} = null;
}}"#
    )
}

/// While the calendar is on the page, `on_elsewhere` hears each click that
/// lands anywhere but on a day's square, and each press of Escape.
fn use_elsewhere(mut on_elsewhere: impl FnMut() + 'static) {
    let on_elsewhere = use_callback(move |()| on_elsewhere());
    // Tests have no page to run it on: they click elsewhere through
    // [`crate::testing::Ui::click_elsewhere`].
    #[cfg(not(test))]
    {
        use_hook(move || {
            spawn(async move {
                let mut page = document::eval(&elsewhere_script());
                while page.recv::<bool>().await.is_ok() {
                    on_elsewhere.call(());
                }
            })
        });
        use_drop(|| {
            dioxus::core::spawn_forever(async {
                let _ = document::eval(&stop_listening()).await;
            });
        });
    }
    #[cfg(test)]
    use_hook(move || crate::testing::listen_elsewhere(on_elsewhere));
}

#[component]
fn HeatmapGridView(grid: HeatmapGrid) -> Element {
    // A mouse pointing at a square opens its card for as long as it stays;
    // a tap pins one until the same square is tapped again, or anywhere
    // else is. Touch has no pointing, so only a mouse's counts.
    let mut hovered = use_signal(|| None::<TipAt>);
    let mut pinned = use_signal(|| None::<TipAt>);
    use_elsewhere(move || {
        if pinned.peek().is_some() {
            pinned.set(None);
        }
    });
    let shown = hovered().or_else(&*pinned);
    let pinned_date = pinned().map(|at| at.date);
    rsx! {
        div { class: "heatmap-months",
            for month in grid.months.iter() {
                div { class: "heatmap-month",
                    span { class: "heatmap-month-label", "{month_name(month.year, month.month)}" }
                    div { class: "heatmap-month-grid",
                        for day in WEEKDAYS {
                            span { class: "heatmap-day-label", "{day}" }
                        }
                        for (n, cell) in month.cells.iter().enumerate() {
                            {
                                let label = long_date(&cell.date);
                                let tip = shown.as_ref().filter(|at| at.date == cell.date);
                                let is_selected = pinned_date.as_deref() == Some(cell.date.as_str());
                                let cls = match (is_selected, tip.is_some()) {
                                    (true, true) => "heat-cell selected has-tip",
                                    (true, false) => "heat-cell selected",
                                    (false, true) => "heat-cell has-tip",
                                    (false, false) => "heat-cell",
                                };
                                // The 1st sits under its weekday, and every day after it follows on.
                                let column = (n == 0).then(|| format!("grid-column-start: {};", month.lead + 1));
                                let paint = cell.intensity.map(|intensity| format!("background: {};", heat_color(intensity)));
                                let style: Vec<String> = [column, paint].into_iter().flatten().collect();
                                let style = (!style.is_empty()).then(|| style.join(" "));
                                let summary = day_summary(&label, cell);
                                let in_future = cell.in_future;
                                let (enter, leave, click) = (cell.date.clone(), cell.date.clone(), cell.date.clone());
                                rsx! {
                                    button {
                                        id: control_id("day", &cell.date),
                                        class: cls,
                                        style: style,
                                        aria_label: "{summary}",
                                        disabled: html_bool(in_future),
                                        onpointerenter: move |evt: PointerEvent| {
                                            if !in_future && evt.pointer_type() == "mouse" {
                                                let window = evt.client_coordinates();
                                                let square = evt.element_coordinates();
                                                hovered.set(Some(TipAt::new(&enter, (window.x, window.y), (square.x, square.y))));
                                            }
                                        },
                                        onpointerleave: move |evt: PointerEvent| {
                                            let here = hovered.peek().as_ref().is_some_and(|at| at.date == leave);
                                            if here && evt.pointer_type() == "mouse" {
                                                hovered.set(None);
                                            }
                                        },
                                        onclick: move |evt: MouseEvent| {
                                            let again = pinned.peek().as_ref().is_some_and(|at| at.date == click);
                                            if again {
                                                pinned.set(None);
                                            } else {
                                                let window = evt.client_coordinates();
                                                let square = evt.element_coordinates();
                                                pinned.set(Some(TipAt::new(&click, (window.x, window.y), (square.x, square.y))));
                                            }
                                        },
                                        if let Some(at) = tip {
                                            {day_tip(&label, cell, &grid, at)}
                                        }
                                    }
                                }
                            }
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

    #[test]
    fn days_shade_from_red_through_yellow_to_green() {
        assert_eq!(heat_color(0.0), "hsl(0, 75%, 45%)");
        assert_eq!(heat_color(0.5), "hsl(60, 75%, 45%)");
        assert_eq!(heat_color(1.0), "hsl(120, 75%, 45%)");
        // Out-of-range input is clamped rather than producing a broken colour.
        assert_eq!(heat_color(-1.0), heat_color(0.0));
        assert_eq!(heat_color(9.0), heat_color(1.0));
    }

    fn day(sessions: u32) -> HeatmapCell {
        HeatmapCell {
            date: "2026-09-08".to_string(),
            in_future: false,
            chars: 60,
            sessions,
            group_count: 20,
            avg_accuracy: Some(0.959),
            char_wpm: Some(25.0),
            effective_wpm: Some(25.0),
            best_score: Some(800.0),
            intensity: Some(0.5),
        }
    }

    fn grid() -> HeatmapGrid {
        HeatmapGrid {
            max_chars: 120,
            max_sessions: 4,
            max_groups: 20,
            max_char_wpm: 50.0,
            max_score: 1_600.0,
            months: Vec::new(),
        }
    }

    fn row<'r>(rows: &'r [TipRow], label: &str) -> &'r TipRow {
        rows.iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| panic!("no {label} row in {rows:?}"))
    }

    /// Every figure stands against the most any day on the calendar had.
    #[test]
    fn a_day_card_measures_each_figure_against_the_busiest_day() {
        let rows = tip_rows(&day(1), &grid());
        let labels: Vec<&str> = rows.iter().map(|row| row.label).collect();
        assert_eq!(
            labels,
            [
                "Characters",
                "Sessions",
                "Groups",
                "Speed",
                "Accuracy",
                "Points"
            ]
        );
        assert_eq!(row(&rows, "Characters").value, "60");
        assert_eq!(row(&rows, "Characters").share, 0.5);
        assert_eq!(row(&rows, "Sessions").share, 0.25);
        assert_eq!(row(&rows, "Groups").value, "20");
        assert_eq!(row(&rows, "Groups").share, 1.0);
        assert_eq!(
            row(&rows, "Groups").note.as_deref(),
            Some("~3.0 chars/group")
        );
        assert_eq!(row(&rows, "Speed").value, "25 WPM");
        assert_eq!(row(&rows, "Speed").note, None, "no Farnsworth, no note");
        assert_eq!(row(&rows, "Accuracy").value, "95.9%");
        assert_eq!(row(&rows, "Accuracy").heat, Some(heat_color(0.959)));
        assert_eq!(row(&rows, "Characters").heat, None);
        assert_eq!(row(&rows, "Characters").tone, "fill-chars");
        assert_eq!(row(&rows, "Points").value, "800");
        assert_eq!(row(&rows, "Points").share, 0.5);
    }

    #[test]
    fn a_day_of_several_sessions_shows_averages_and_its_best() {
        let mut busy = day(2);
        busy.effective_wpm = Some(15.0);
        let rows = tip_rows(&busy, &grid());
        assert_eq!(row(&rows, "Groups").value, "20");
        assert_eq!(
            row(&rows, "Groups").note.as_deref(),
            Some("~10.0/session · ~3.0 chars/group")
        );
        assert_eq!(row(&rows, "Accuracy").value, "95.9% avg");
        assert_eq!(row(&rows, "Best points").value, "800");
        assert_eq!(
            row(&rows, "Speed").note.as_deref(),
            Some("spaced to 15 WPM")
        );
    }

    #[test]
    fn a_day_without_practice_has_nothing_to_show() {
        let quiet = HeatmapCell {
            sessions: 0,
            ..day(1)
        };
        assert!(tip_rows(&quiet, &grid()).is_empty());
        assert_eq!(
            day_summary("Wednesday 9 September 2026", &quiet),
            "Wednesday 9 September 2026: no practice"
        );
    }

    /// What could not be read is left off rather than shown as nothing.
    #[test]
    fn what_a_day_does_not_know_it_leaves_off() {
        let unknown = HeatmapCell {
            char_wpm: None,
            avg_accuracy: None,
            best_score: None,
            ..day(1)
        };
        let labels: Vec<&str> = tip_rows(&unknown, &grid())
            .iter()
            .map(|row| row.label)
            .collect();
        assert_eq!(labels, ["Characters", "Sessions", "Groups"]);
        // An empty calendar draws no bar rather than dividing by nothing.
        assert_eq!(share(5.0, 0.0), 0.0);
        assert_eq!(share(f64::NAN, 10.0), 0.0);
        assert_eq!(share(20.0, 10.0), 1.0);
    }

    /// The harness never mounts anything on a page, so the script is the
    /// part a test can see: a click off every square tells the calendar, as
    /// Escape does, a click on one is the square's own, and the listener
    /// comes off again, one left before first.
    #[test]
    fn the_page_says_when_it_is_clicked_off_the_squares() {
        let script = elsewhere_script();
        assert!(
            script.contains(r#"e.target.closest("button.heat-cell")"#),
            "{script}"
        );
        assert!(script.contains(r#"e.key === "Escape""#), "{script}");
        assert_eq!(script.matches("dioxus.send(true)").count(), 2, "{script}");
        for (on, off) in [
            (
                r#"addEventListener("click", clicked)"#,
                r#"removeEventListener("click", clicked)"#,
            ),
            (
                r#"addEventListener("keydown", keyed)"#,
                r#"removeEventListener("keydown", keyed)"#,
            ),
        ] {
            assert!(script.contains(on), "{script}");
            assert!(script.contains(off), "{script}");
        }
        let off = script.find("window.__dustDayCard();").expect("taken off");
        let on = script.find("addEventListener").expect("put on");
        assert!(off < on, "{script}");
        assert!(stop_listening().contains("window.__dustDayCard = null;"));
    }

    #[test]
    fn a_card_opens_above_its_square_unless_the_window_has_no_room() {
        let low = TipAt::new("2026-09-08", (410.0, 520.0), (10.0, 20.0));
        assert_eq!(low.cell_left, 400.0);
        assert!(!low.below);
        let high = TipAt::new("2026-09-08", (410.0, 120.0), (10.0, 20.0));
        assert!(high.below);
        // A click from the keyboard has no position: the card still opens,
        // kept on screen.
        let keyed = TipAt::new("2026-09-08", (0.0, 0.0), (0.0, 0.0));
        assert_eq!(keyed.cell_left, 0.0);
        assert!(keyed.below);
        let nowhere = TipAt::new("2026-09-08", (f64::NAN, f64::NAN), (0.0, 0.0));
        assert_eq!(nowhere.cell_left, 0.0);
        assert!(nowhere.below);
    }
}
