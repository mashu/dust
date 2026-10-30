use std::rc::Rc;

use cw_core::{
    AccuracyPoint, CharSetMode, EstimatePoint, EstimateReading, TrainingSettings, accuracy_chart,
    adaptive_speed_on, adapts_group_size, adapts_speed, best_accuracy, level_up_line,
    mastered_count, overall_accuracy, practised_modes, span_history, speed_history,
};
use dioxus::prelude::*;

use crate::state::SessionPick;
use crate::ui::backup::Backups;
use crate::ui::chart::{CHART_H, marks, path, polyline, series, trend_chart};
use crate::ui::endurance::EnduranceCard;
use crate::ui::figures::History;
use crate::ui::format::counted;
use crate::ui::help;
use crate::ui::letters::LettersTab;
use crate::ui::scores::ScoresTab;
use crate::ui::settings::aim_label;
use crate::ui::settings_charset::mode_label;
use crate::ui::stats_detail::{HistoryTab, MistakesTab, SamplingTab};
use crate::ui::widgets::{CardHead, IconName, Seg, control_id};

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatsTab {
    Overview,
    Scores,
    Letters,
    Mistakes,
    Sampling,
    History,
}

/// The tabs, in the order the control offers them.
const TABS: [(StatsTab, &str); 6] = [
    (StatsTab::Overview, "Overview"),
    (StatsTab::Scores, "Scores"),
    (StatsTab::Letters, "Letters"),
    (StatsTab::Mistakes, "Mistakes"),
    (StatsTab::Sampling, "Sampling"),
    (StatsTab::History, "History"),
];

/// Chart shape for the accuracy series. `None` when there is nothing to draw.
struct ChartGeometry {
    line: String,
    area: String,
    dots: Vec<(f64, f64)>,
    floor_pct: f64,
    threshold_y: Option<f64>,
}

/// What the Overview reads of the whole history, worked out once for it.
struct Overview {
    /// The accuracy chart, a point a session.
    chart: Vec<AccuracyPoint>,
    average: Option<f64>,
    best: Option<f64>,
    mastered: usize,
}

/// The character sets `sessions` hold any practice in ([`practised_modes`]),
/// as every card that offers them reads them.
pub(crate) fn practised(sessions: &History) -> Rc<Vec<CharSetMode>> {
    sessions.figure("practised modes", (), practised_modes)
}

/// Accuracy clusters near the top, so the axis starts below the worst session
/// instead of at zero — otherwise every series is a flat line in the top tenth.
fn chart_floor(points: &[AccuracyPoint]) -> f64 {
    let min = points
        .iter()
        .map(|p| p.accuracy_pct.clamp(0.0, 100.0))
        .fold(100.0_f64, f64::min);
    (((min - 6.0) / 10.0).floor() * 10.0).clamp(0.0, 80.0)
}

/// The chart of `points`, with a line at `threshold_pct` where there is one
/// to draw.
fn chart_geometry(points: &[AccuracyPoint], threshold_pct: Option<f64>) -> Option<ChartGeometry> {
    if points.is_empty() {
        return None;
    }
    let floor_pct = chart_floor(points);
    let span = (100.0 - floor_pct).max(1.0);
    let y_at = |pct: f64| {
        let pct = pct.clamp(0.0, 100.0);
        (CHART_H - ((pct - floor_pct) / span) * CHART_H).clamp(0.0, CHART_H)
    };

    let values: Vec<f64> = points.iter().map(|p| p.accuracy_pct).collect();
    let coords = series(&values, y_at);
    let (&(first_x, _), &(last_x, _)) = (coords.first()?, coords.last()?);
    // The line, closed down to the floor at either end.
    let outline = std::iter::once((first_x, CHART_H))
        .chain(coords.iter().copied())
        .chain(std::iter::once((last_x, CHART_H)));

    Some(ChartGeometry {
        line: polyline(&coords),
        area: format!("{} Z", path(outline)),
        dots: marks(&coords, points.len()),
        floor_pct,
        threshold_y: threshold_pct
            .filter(|pct| *pct > floor_pct && *pct < 100.0)
            .map(y_at),
    })
}

/// How many sessions a trend card draws.
const TREND_SESSIONS_SHOWN: usize = 30;
/// How many times the spreads of the first and the last estimate together a
/// move has to be to be one: less is the estimate firming up, or chance.
const TREND_SPREADS: f64 = 2.0;

/// How an estimate moved over `count` sessions, by `change`, its first and
/// last estimates' spreads together being `noise`, given to `decimals`
/// places. A change within [`TREND_SPREADS`] of the noise, or too small to
/// show at that many places, is steady.
fn trend_words(change: f64, noise: f64, count: usize, decimals: usize) -> String {
    let steady = (0.5 / 10_f64.powi(decimals as i32)).max(TREND_SPREADS * noise);
    if count == 1 {
        "after 1 session".to_string()
    } else if change.abs() < steady {
        format!("steady over {count} sessions")
    } else if change > 0.0 {
        format!("up {change:.decimals$} over {count} sessions")
    } else {
        format!("down {:.decimals$} over {count} sessions", -change)
    }
}

/// An estimate as a card's note gives it, to `decimals` places: "6.4 ± 1.6
/// chars", or past what was sent, only that: "over 3 chars".
fn reading_words(point: &EstimatePoint, unit: &str, decimals: usize) -> String {
    match point.reading() {
        EstimateReading::About(estimate) => {
            format!(
                "{:.decimals$} ± {:.decimals$} {unit}",
                estimate.mean, estimate.sd
            )
        }
        EstimateReading::Over(most) => format!("over {most:.0} {unit}"),
        EstimateReading::Under(least) => format!("under {least:.0} {unit}"),
    }
}

/// The character set a trend card shows: the one chosen, while it has
/// history; else the one practised now, if it has any; else the first that
/// does. Nothing when none has: a set chosen in the settings never hides
/// another's history.
pub(crate) fn shown_mode(
    chosen: Option<CharSetMode>,
    modes: &[CharSetMode],
    current: CharSetMode,
) -> Option<CharSetMode> {
    chosen
        .filter(|mode| modes.contains(mode))
        .or_else(|| modes.contains(&current).then_some(current))
        .or_else(|| modes.first().copied())
}

/// An estimate over the last sessions, with its uncertainty, and whether the
/// feature it belongs to is steering by it, each point kept within what was
/// sent up to then. Nothing before the first session it can be read from.
#[component]
fn TrendCard(
    /// The estimate after each session, oldest first.
    points: Vec<EstimatePoint>,
    icon: IconName,
    title: &'static str,
    /// What the estimate is counted in: "chars", "WPM".
    unit: &'static str,
    /// How many decimal places it is given to.
    decimals: usize,
    /// The colour it is drawn in.
    colour: &'static str,
    /// What the estimate stands for, said before its scale.
    #[props(into)]
    meaning: String,
    /// What its ⓘ opens.
    help: Element,
    /// The aim, while the feature is steering by it.
    steering: Option<&'static str>,
    /// What the chip says while it is not.
    off: &'static str,
    /// The character sets there is history for, and the one shown: a pill
    /// for each, when there is more than one.
    modes: Vec<CharSetMode>,
    mode: CharSetMode,
    on_mode: EventHandler<CharSetMode>,
    /// What the pills' ids begin with.
    pills: &'static str,
    /// A line under the chart, where the series shown needs one.
    note: Option<String>,
) -> Element {
    let shown = &points[points.len().saturating_sub(TREND_SESSIONS_SHOWN)..];
    let bands: Vec<(f64, f64, f64)> = shown.iter().map(EstimatePoint::within_sent).collect();
    let (Some(first), Some(last), Some(chart)) = (shown.first(), shown.last(), trend_chart(&bands))
    else {
        return rsx! {};
    };
    let trend = trend_words(
        last.within_sent().1 - first.within_sent().1,
        first.estimate.sd.hypot(last.estimate.sd),
        shown.len(),
        decimals,
    );
    let chip = steering.map_or_else(|| off.to_string(), |aim| format!("Adapting · {aim}"));
    rsx! {
        div { class: "card chart-card",
            CardHead {
                icon,
                title,
                note: format!("{} · {trend}", reading_words(last, unit, decimals)),
                help,
            }
            if modes.len() > 1 {
                div { class: "mode-pills",
                    for each in modes.iter().copied() {
                        Seg {
                            label: mode_label(each),
                            id: control_id(pills, mode_label(each)),
                            pill: true,
                            active: each == mode,
                            onclick: move |_| on_mode.call(each),
                        }
                    }
                }
            }
            svg {
                class: "sparkline",
                view_box: "0 0 420 96",
                path { d: "{chart.band}", fill: colour, fill_opacity: "0.16", stroke: "none" }
                polyline {
                    points: "{chart.line}",
                    fill: "none",
                    stroke: colour,
                    stroke_width: "2.2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                }
                for (x, y) in chart.dots.iter() {
                    circle {
                        cx: "{x:.1}",
                        cy: "{y:.1}",
                        r: "3",
                        fill: colour,
                        stroke: "var(--surface)",
                        stroke_width: "1.5",
                    }
                }
            }
            div { class: "row-between",
                span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                    "{meaning} · scale {chart.low:.0}–{chart.high:.0} {unit}"
                }
                span { class: if steering.is_some() { "chip good" } else { "chip" }, "{chip}" }
            }
            // The card is no stack, so the note keeps its own distance from
            // the chip row above it.
            if let Some(note) = note {
                p { class: "muted", style: "margin: 0.6rem 0 0; font-size: 0.85rem;", "{note}" }
            }
        }
    }
}

/// What the chip says of a character set that is not the one practised now.
const NOT_NOW: &str = "Not the set practised now";

/// What the run screen's keys do to the speeds, however the callers' speeds
/// come.
pub(crate) const YOUR_SPEED_KEYS: &str = "The run screen's − and + move only your own speed.";

/// How a contest's callers' speeds come under `settings`: from your copy
/// speed in contests while their switch is on; every one at the one speed
/// set while it is on with no room to adapt; and evenly from your range
/// while it is off. Then what − and + do, the same either way.
pub(crate) fn contest_speeds(settings: &TrainingSettings) -> String {
    let callers = if adapts_speed(settings) {
        "Each new caller's speed is drawn from your copy speed, within your range: faster as you copy calls and numbers clean, slower as you miss them."
    } else if adaptive_speed_on(settings) {
        "One speed is set, so every caller comes at it."
    } else {
        "Callers' speeds are drawn evenly from your range."
    };
    format!("{callers} {YOUR_SPEED_KEYS}")
}

/// The newest of `points`, as many as a trend card draws.
fn newest(points: &[EstimatePoint]) -> Vec<EstimatePoint> {
    points[points.len().saturating_sub(TREND_SESSIONS_SHOWN)..].to_vec()
}

/// Your copy span after each session of the character set `settings`
/// train ([`span_history`]), as the Overview and the Sampling tab read it:
/// it reads the set alone.
pub(crate) fn span_history_of(
    sessions: &History,
    settings: &TrainingSettings,
) -> Rc<Vec<EstimatePoint>> {
    let mode = settings.curriculum.char_set_mode;
    sessions.figure("span history", mode, |sessions| {
        span_history(settings, sessions)
    })
}

/// Your copy speed after each session of the character set `settings`
/// train ([`speed_history`]), as the Overview and the Sampling tab read it:
/// it reads the curriculum alone, the set, the aim and the group lengths.
pub(crate) fn speed_history_of(
    sessions: &History,
    settings: &TrainingSettings,
) -> Rc<Vec<EstimatePoint>> {
    let curriculum = settings.curriculum.clone();
    sessions.figure("speed history", curriculum, |sessions| {
        speed_history(settings, sessions)
    })
}

/// The copy span: how long a group you copy whole, and how that has moved,
/// for each character set with groups drawn a character at a time in its
/// history. Callsigns and words are as long as they are.
#[component]
fn SpanCard(settings: TrainingSettings, sessions: History) -> Element {
    let mut chosen = use_signal(|| None::<CharSetMode>);
    let modes: Vec<CharSetMode> = practised(&sessions)
        .iter()
        .copied()
        .filter(|mode| mode.draws_characters())
        .collect();
    let current = settings.curriculum.char_set_mode;
    let Some(mode) = shown_mode(chosen(), &modes, current) else {
        return rsx! {};
    };
    let mut shown = settings.clone();
    shown.curriculum.char_set_mode = mode;
    let now = mode == current;
    let points = span_history_of(&sessions, &shown);
    rsx! {
        TrendCard {
            points: newest(&points),
            icon: IconName::Target,
            title: "Copy span",
            unit: "chars",
            decimals: 1,
            colour: "var(--info)",
            meaning: "Copied whole about half the time",
            help: help::copy_span(),
            steering: (now && adapts_group_size(&settings)).then(|| aim_label(settings.curriculum.aim)),
            off: if now { "Adaptive length off" } else { NOT_NOW },
            modes,
            mode,
            on_mode: move |mode| chosen.set(Some(mode)),
            pills: "span",
            note: None,
        }
    }
}

/// Your copy speed: the speed at which you copy the share of characters
/// "Aim for" asks, and how that has moved, for each character set in the
/// history.
#[component]
fn CopySpeedCard(settings: TrainingSettings, sessions: History) -> Element {
    let mut chosen = use_signal(|| None::<CharSetMode>);
    let modes = practised(&sessions).to_vec();
    let current = settings.curriculum.char_set_mode;
    let Some(mode) = shown_mode(chosen(), &modes, current) else {
        return rsx! {};
    };
    let mut shown = settings.clone();
    shown.curriculum.char_set_mode = mode;
    let now = mode == current;
    let aim = settings.curriculum.aim;
    let contest = mode == CharSetMode::Contest;
    // Switched on with one speed set, nothing adapts, but the switch is not
    // off either.
    let off = match (now, contest) {
        (false, _) => NOT_NOW,
        (true, _) if adaptive_speed_on(&settings) => "One speed set",
        (true, true) => "Adaptive callers off",
        (true, false) => "Adaptive speed off",
    };
    let points = speed_history_of(&sessions, &shown);
    rsx! {
        TrendCard {
            points: newest(&points),
            icon: IconName::Gauge,
            title: "Copy speed",
            unit: "WPM",
            decimals: 0,
            colour: "var(--good)",
            meaning: format!("{:.0}% of characters right", aim.chars_right() * 100.0),
            help: help::copy_speed(aim.chars_right()),
            steering: (now && adapts_speed(&settings)).then(|| aim_label(aim)),
            off,
            modes,
            mode,
            on_mode: move |mode| chosen.set(Some(mode)),
            pills: "speed",
            note: contest.then(|| contest_speeds(&shown)),
        }
    }
}

#[component]
pub fn StatsView(
    settings: TrainingSettings,
    /// The history, shared: read where it is kept, never copied.
    #[props(into)]
    sessions: History,
    /// Where deleting sessions from the history goes. Without it the history
    /// is only shown.
    on_delete: Option<EventHandler<Vec<SessionPick>>>,
    /// Saving and restoring backups, on the History tab.
    backups: Option<Backups>,
) -> Element {
    let mut tab = use_signal(|| StatsTab::Overview);
    // Accuracy, letters and mistakes are counted over the whole history: a
    // letter is the same letter whichever character set it was sent under.
    // Only the sampling snapshot narrows to the current set, and it does that
    // itself so it keeps matching what the trainer will actually draw. Each
    // tab works out its own figures when it is shown, and only then: a tab
    // that is not open costs nothing; and what it works out is kept with
    // the history until that changes, for the tab shown again and for Stats
    // opened again ([`History::figure`]).
    //
    // History lists the sessions as they are stored, and picks one out by
    // its stamp to delete it; every other tab shows them as the clock reads
    // them, any stamped ahead of it settled after the latest that is not.
    let sessions = match tab() {
        StatsTab::History => sessions,
        _ => sessions.shown(),
    };
    rsx! {
        div { class: "stack",
            header { class: "page-head",
                h2 { class: "page-title", "Stats" }
                p { class: "page-sub", "Every session scored on this device, whatever it practised." }
            }
            div { class: "segmented",
                for (which, label) in TABS {
                    Seg { label, active: tab() == which, onclick: move |_| tab.set(which) }
                }
            }
            match tab() {
                StatsTab::Overview => rsx! {
                    OverviewTab { settings, sessions }
                },
                StatsTab::Scores => rsx! { ScoresTab { sessions } },
                StatsTab::Letters => rsx! { LettersTab { settings, sessions } },
                StatsTab::Mistakes => rsx! { MistakesTab { sessions } },
                StatsTab::Sampling => rsx! { SamplingTab { settings, sessions } },
                StatsTab::History => rsx! { HistoryTab { sessions, on_delete, backups } },
            }
        }
    }
}

#[component]
fn OverviewTab(settings: TrainingSettings, sessions: History) -> Element {
    #[cfg(test)]
    crate::testing::rendered("OverviewTab");
    // A line, and a chip, only where a session's own accuracy is what levels
    // you up: not with auto level off or never levelling up, nor while an
    // adaptive feature steers and sessions are judged otherwise.
    let threshold = level_up_line(&settings);
    // Over every character sent, and the best of the sessions long enough
    // to go on the board: a group or two copied right is no best.
    let overview = sessions.figure("overview", (), |sessions| Overview {
        chart: accuracy_chart(sessions),
        average: overall_accuracy(sessions),
        best: best_accuracy(sessions),
        mastered: mastered_count(sessions),
    });
    let pct = |accuracy: Option<f64>| {
        accuracy.map_or("—".to_string(), |accuracy| {
            format!("{}%", (accuracy * 100.0).round())
        })
    };
    let avg = pct(overview.average);
    let best = pct(overview.best);
    let mastered = overview.mastered;
    let chart = &overview.chart;
    let geometry = sessions.figure("accuracy chart", threshold, |_| {
        chart_geometry(chart, threshold)
    });
    let latest = chart.last().map(|p| p.accuracy_pct).unwrap_or(0.0);
    rsx! {
        div { class: "stack",
            if sessions.is_empty() {
                div { class: "card",
                    CardHead { icon: IconName::Chart, title: "Nothing scored yet" }
                    p { class: "muted hint",
                        "Finish a session to unlock accuracy over time, letter mastery and sampling weights."
                    }
                }
            } else {
                div { class: "grid-3",
                    div { class: "kpi emerald",
                        div { class: "tiny", "Average" }
                        div { class: "value", "{avg}" }
                    }
                    div { class: "kpi indigo",
                        div { class: "tiny", "Best" }
                        div { class: "value", "{best}" }
                    }
                    div { class: "kpi blue",
                        div { class: "tiny", "Mastered" }
                        div { class: "value", "{mastered}" }
                    }
                }
                if let Some(geometry) = geometry.as_ref() {
                    div { class: "card chart-card",
                        CardHead {
                            icon: IconName::Chart,
                            title: "Accuracy over time",
                            note: format!("{} · latest {}%", counted(sessions.len(), "session"), latest.round()),
                            help: help::accuracy_over_time(threshold.map(|pct| pct / 100.0)),
                        }
                        svg {
                            class: "sparkline",
                            view_box: "0 0 420 96",
                            defs {
                                linearGradient { id: "dust-acc-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                                    stop { offset: "0%", style: "stop-color: var(--copper); stop-opacity: 0.42;" }
                                    stop { offset: "100%", style: "stop-color: var(--copper); stop-opacity: 0.02;" }
                                }
                            }
                            if let Some(y) = geometry.threshold_y {
                                line {
                                    x1: "0",
                                    y1: "{y:.1}",
                                    x2: "420",
                                    y2: "{y:.1}",
                                    stroke: "var(--good)",
                                    stroke_width: "1",
                                    stroke_dasharray: "4 4",
                                    opacity: "0.65",
                                }
                            }
                            path { d: "{geometry.area}", fill: "url(#dust-acc-fill)", stroke: "none" }
                            polyline {
                                points: "{geometry.line}",
                                fill: "none",
                                stroke: "var(--copper)",
                                stroke_width: "2.2",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                            }
                            for (x, y) in geometry.dots.iter() {
                                circle {
                                    cx: "{x:.1}",
                                    cy: "{y:.1}",
                                    r: "3",
                                    fill: "var(--copper)",
                                    stroke: "var(--surface)",
                                    stroke_width: "1.5",
                                }
                            }
                        }
                        div { class: "row-between",
                            span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                                "Scale {geometry.floor_pct}–100%"
                            }
                            if let Some(threshold) = threshold {
                                span { class: "chip good", "Level up at {threshold.round()}%" }
                            }
                        }
                    }
                }
                SpanCard { settings: settings.clone(), sessions: sessions.clone() }
                CopySpeedCard { settings: settings.clone(), sessions: sessions.clone() }
                EnduranceCard { settings, sessions }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{chart_floor, chart_geometry, reading_words, trend_words};
    use crate::ui::chart::{CHART_H, CHART_W};
    use cw_core::{AccuracyPoint, Estimate, EstimatePoint};

    fn points(pcts: &[f64]) -> Vec<AccuracyPoint> {
        pcts.iter()
            .enumerate()
            .map(|(i, pct)| AccuracyPoint {
                date: format!("2026-09-{:02}", i + 1),
                accuracy_pct: *pct,
                timestamp: 1_757_000_000_000 + i as u64,
            })
            .collect()
    }

    #[test]
    fn no_sessions_means_no_chart() {
        assert!(chart_geometry(&[], Some(90.0)).is_none());
    }

    #[test]
    fn one_session_draws_a_flat_line_with_a_point() {
        let geometry = chart_geometry(&points(&[89.0]), Some(90.0)).unwrap();
        // Two coordinates, so the polyline actually has a segment to draw.
        assert_eq!(geometry.line.split(' ').count(), 2);
        assert!(geometry.line.starts_with("0.0,"));
        assert!(geometry.area.starts_with("M0.0,96.0 L0.0,"));
        assert!(geometry.area.ends_with(" L420.0,96.0 Z"));
        assert_eq!(geometry.dots.len(), 1);
        assert!((geometry.dots[0].0 - CHART_W / 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_axis_starts_below_the_worst_session() {
        assert_eq!(chart_floor(&points(&[88.0, 94.0])), 80.0);
        assert_eq!(chart_floor(&points(&[71.0, 94.0])), 60.0);
        // Never below zero, and never so high that the span disappears.
        assert_eq!(chart_floor(&points(&[3.0])), 0.0);
        assert_eq!(chart_floor(&points(&[100.0])), 80.0);
    }

    #[test]
    fn the_series_spans_the_full_width_and_height() {
        let geometry = chart_geometry(&points(&[60.0, 100.0, 80.0]), Some(90.0)).unwrap();
        let coords: Vec<(f64, f64)> = geometry
            .line
            .split(' ')
            .map(|pair| {
                let (x, y) = pair.split_once(',').unwrap();
                (x.parse().unwrap(), y.parse().unwrap())
            })
            .collect();
        assert_eq!(coords.len(), 3);
        assert_eq!(coords[0].0, 0.0);
        assert_eq!(coords[2].0, CHART_W);
        // 100% sits on the top edge; the worst session sits above the bottom,
        // because the axis floor is the round ten below it.
        assert_eq!(coords[1].1, 0.0);
        assert_eq!(geometry.floor_pct, 50.0);
        assert!(coords[0].1 > coords[2].1, "60% must sit below 80%");
        assert!(coords.iter().all(|(_, y)| (0.0..=CHART_H).contains(y)));
        assert_eq!(geometry.dots.len(), 3);
    }

    #[test]
    fn only_the_last_point_is_marked_on_a_long_history() {
        let long: Vec<f64> = (0..40).map(|i| 60.0 + f64::from(i % 20)).collect();
        let geometry = chart_geometry(&points(&long), Some(90.0)).unwrap();
        assert_eq!(geometry.dots.len(), 1);
    }

    #[test]
    fn the_threshold_line_hides_when_it_is_off_the_scale() {
        // Sessions in the 90s: the 90% guide sits inside the visible range.
        let inside = chart_geometry(&points(&[92.0, 97.0]), Some(90.0)).unwrap();
        assert!(inside.threshold_y.is_some());
        // A threshold under the floor has nowhere to sit.
        let below = chart_geometry(&points(&[96.0, 99.0]), Some(50.0)).unwrap();
        assert_eq!(below.floor_pct, 80.0);
        assert!(below.threshold_y.is_none());
        // Neither does one at the very top.
        let top = chart_geometry(&points(&[80.0]), Some(100.0)).unwrap();
        assert!(top.threshold_y.is_none());
        // And with no threshold to mark, there is no line.
        let none = chart_geometry(&points(&[92.0, 97.0]), None).unwrap();
        assert!(none.threshold_y.is_none());
    }

    /// A move is said to the places the estimate is given to; one that would
    /// show as nothing is steady.
    #[test]
    fn a_trend_says_which_way_and_how_far() {
        assert_eq!(trend_words(3.0, 0.0, 1, 1), "after 1 session");
        assert_eq!(trend_words(0.04, 0.0, 8, 1), "steady over 8 sessions");
        assert_eq!(trend_words(0.36, 0.0, 8, 1), "up 0.4 over 8 sessions");
        assert_eq!(trend_words(-1.24, 0.0, 8, 1), "down 1.2 over 8 sessions");
        assert_eq!(trend_words(0.4, 0.0, 8, 0), "steady over 8 sessions");
        assert_eq!(trend_words(2.6, 0.0, 8, 0), "up 3 over 8 sessions");
        assert_eq!(trend_words(-5.0, 0.0, 30, 0), "down 5 over 30 sessions");
    }

    /// A move the first and last estimates' spreads could make is steady:
    /// an estimate firming up, or wandering, is not you getting faster.
    #[test]
    fn a_move_within_the_spreads_is_steady() {
        // 25.9 ± 7.5 to 29.9 ± 7.3: four up, with eleven of spread.
        let noise = 7.5_f64.hypot(7.3);
        assert_eq!(trend_words(4.0, noise, 30, 0), "steady over 30 sessions");
        assert_eq!(trend_words(4.0, 1.0, 30, 0), "up 4 over 30 sessions");
        assert_eq!(trend_words(-2.6, 1.0, 30, 0), "down 3 over 30 sessions");
        assert_eq!(trend_words(-1.9, 1.0, 30, 0), "steady over 30 sessions");
    }

    /// An estimate past what was sent says only that it is past it.
    #[test]
    fn an_estimate_past_what_was_sent_says_only_that() {
        let point = |mean: f64, sent: (f64, f64)| EstimatePoint {
            timestamp: 1,
            estimate: Estimate { mean, sd: 1.6 },
            sent,
        };
        assert_eq!(
            reading_words(&point(6.44, (2.0, 8.0)), "chars", 1),
            "6.4 ± 1.6 chars"
        );
        assert_eq!(
            reading_words(&point(6.44, (2.0, 3.0)), "chars", 1),
            "over 3 chars"
        );
        assert_eq!(
            reading_words(&point(14.2, (19.9, 20.1)), "WPM", 0),
            "under 20 WPM"
        );
    }
}
