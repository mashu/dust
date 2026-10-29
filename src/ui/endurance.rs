//! The Endurance card: how far into a session your copy holds before it
//! drops, for each character set with sessions long enough to tell.
//!
//! It says it first in words, "Steady through 20 groups" or "Fades after
//! group 10", then shows it: a bar a stretch of the session with your
//! accuracy over it, the first bar how you start and a dashed line at its
//! level across the rest, so a stretch that sinks below it, or a warm-up
//! that climbs above it, shows at a glance.

use cw_core::{
    DROP_POINTS, ENDURANCE_MIN_SESSIONS, Endurance, EndurancePoint, GROUPS_A_STRETCH,
    MINUTES_A_STRETCH, SessionResult, StretchUnit, TrainingSettings, endurance, practised_modes,
};
use dioxus::prelude::*;

use crate::ui::format::counted;
use crate::ui::help;
use crate::ui::settings_charset::mode_label;
use crate::ui::stats::shown_mode;
use crate::ui::widgets::{CardHead, IconName, Seg, control_id};

/// The tallest a bar stands, as a share of the chart: room above it for
/// its figure.
const TALLEST: f64 = 82.0;
/// A stretch this many points or more over the start is a warm-up worth
/// saying so of.
const WARM_UP_POINTS: f64 = 3.0;

/// How far into a session a place is: "group 20", "minute 6".
fn place(unit: StretchUnit, at: u32) -> String {
    match unit {
        StretchUnit::Groups => format!("group {at}"),
        StretchUnit::Minutes => format!("minute {at}"),
    }
}

/// A stretch's label under its bar: "6–10", "2–4 min".
fn stretch_label(unit: StretchUnit, point: &EndurancePoint) -> String {
    match unit {
        StretchUnit::Groups => format!("{}–{}", point.from, point.to),
        StretchUnit::Minutes => format!("{}–{} min", point.from, point.to),
    }
}

/// Accuracy over a stretch, in percent.
fn accuracy(curve: &Endurance, point: &EndurancePoint) -> f64 {
    (curve.opening_pct + point.change).clamp(0.0, 100.0)
}

/// The headline, and whether it is good news.
fn headline(curve: &Endurance) -> (String, bool) {
    let held = place(curve.unit, curve.holds_to());
    match curve.first_drop() {
        Some(_) => (format!("Fades after {held}"), false),
        None => (format!("Steady through {held}"), true),
    }
}

/// The line under the headline: where it ends up against how you start,
/// any warm-up, and what to try.
fn summary(curve: &Endurance) -> String {
    let start = curve.opening_pct;
    let session = match curve.unit {
        StretchUnit::Groups => "session",
        StretchUnit::Minutes => "run",
    };
    let warm = curve
        .points
        .iter()
        .skip(1)
        .max_by(|a, b| a.change.total_cmp(&b.change))
        .filter(|point| point.change >= WARM_UP_POINTS)
        .map(|point| {
            let unit = match curve.unit {
                StretchUnit::Groups => "groups",
                StretchUnit::Minutes => "minutes",
            };
            format!(
                " You warm up: you are at your best over {unit} {}–{}, {:.0} points above your start.",
                point.from, point.to, point.change
            )
        })
        .unwrap_or_default();
    match curve.first_drop() {
        Some(_) => {
            let low = curve
                .points
                .iter()
                .min_by(|a, b| a.change.total_cmp(&b.change))
                .expect("a drop is a point");
            format!(
                "You start a {session} copying {start:.0}%; by {} you are down to {:.0}%, {:.0} points lower.{warm}",
                place(curve.unit, low.to),
                accuracy(curve, low),
                -low.change
            )
        }
        None => format!(
            "You start a {session} copying {start:.0}% and stay within {DROP_POINTS:.0} points of it all the way.{warm} Longer {session}s would show where you tire."
        ),
    }
}

/// Where the bars' scale starts, in percent: ten below the lowest bar, in
/// tens, so that a few points show without a small dip looking like a fall.
fn floor(curve: &Endurance) -> f64 {
    let lowest = curve
        .points
        .iter()
        .map(|point| accuracy(curve, point))
        .fold(100.0_f64, f64::min);
    ((lowest / 10.0).floor() * 10.0 - 10.0).max(0.0)
}

/// How tall a bar of `accuracy` stands, in percent of the chart.
fn height(accuracy: f64, floor: f64) -> f64 {
    if floor >= 100.0 {
        return TALLEST;
    }
    ((accuracy - floor) / (100.0 - floor) * TALLEST).clamp(2.0, TALLEST)
}

/// How copy holds up through a session, for each character set with enough
/// long sessions, against how the session starts; and before there are,
/// what it takes.
#[component]
pub fn EnduranceCard(settings: TrainingSettings, sessions: Vec<SessionResult>) -> Element {
    let mut chosen = use_signal(|| None);
    let curves: Vec<_> = practised_modes(&sessions)
        .into_iter()
        .filter_map(|mode| endurance(&sessions, mode).map(|curve| (mode, curve)))
        .collect();
    let modes: Vec<_> = curves.iter().map(|(mode, _)| *mode).collect();
    let current = settings.curriculum.char_set_mode;
    let shown = shown_mode(chosen(), &modes, current)
        .and_then(|mode| curves.iter().find(|(each, _)| *each == mode));
    let Some((mode, curve)) = shown else {
        return rsx! {
            div { class: "card",
                CardHead { icon: IconName::Timer, title: "Endurance", help: help::endurance() }
                p { class: "muted hint",
                    "How long you can keep copying at your best. After {ENDURANCE_MIN_SESSIONS} sessions of {2 * GROUPS_A_STRETCH} groups or more, or contest runs of {2 * MINUTES_A_STRETCH} minutes or more, this shows whether your accuracy holds up as a session goes on, or where it starts to slip."
                }
            }
        };
    };
    let mode = *mode;
    let (title, good) = headline(curve);
    let floor = floor(curve);
    let start = height(curve.opening_pct, floor);
    let axis = match curve.unit {
        StretchUnit::Groups => "Groups into the session",
        StretchUnit::Minutes => "Minutes into the run",
    };
    let count = curve.points.len();
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Timer,
                title: "Endurance",
                help: help::endurance(),
                note: format!(
                    "Accuracy as a session goes on · {} · last {}",
                    mode_label(mode),
                    counted(curve.sessions, "session"),
                ),
            }
            if modes.len() > 1 {
                div { class: "mode-pills",
                    for each in modes.iter().copied() {
                        Seg {
                            label: mode_label(each),
                            id: control_id("endurance", mode_label(each)),
                            pill: true,
                            active: each == mode,
                            onclick: move |_| chosen.set(Some(each)),
                        }
                    }
                }
            }
            div { class: if good { "endu-headline good" } else { "endu-headline fades" }, "{title}" }
            p { class: "endu-summary", "{summary(curve)}" }
            div {
                class: "endu-chart",
                style: "grid-template-columns: repeat({count}, minmax(0, 1fr));",
                div { class: "endu-start", style: "bottom: {start:.1}%;" }
                for (n , point) in curve.points.iter().enumerate() {
                    {
                        let value = accuracy(curve, point);
                        let class = if n == 0 {
                            "endu-bar first"
                        } else if point.change <= -DROP_POINTS {
                            "endu-bar fades"
                        } else {
                            "endu-bar"
                        };
                        rsx! {
                            div { class: "endu-col",
                                span { class: "endu-value", "{value:.0}%" }
                                span { class: class, style: "height: {height(value, floor):.1}%;" }
                            }
                        }
                    }
                }
            }
            div {
                class: "endu-labels",
                style: "grid-template-columns: repeat({count}, minmax(0, 1fr));",
                for point in curve.points.iter() {
                    span { "{stretch_label(curve.unit, point)}" }
                }
            }
            div { class: "endu-axis", "{axis}" }
            p { class: "muted", style: "margin: 0; font-size: 0.85rem;",
                "Each bar is your character accuracy over that stretch, averaged over your last sessions long enough to tell, each measured against its own start: the first bar, and the dashed line at its level. A bar {DROP_POINTS:.0} points or more below the line is where you start to tire."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(unit: StretchUnit, changes: &[f64]) -> Endurance {
        let size = match unit {
            StretchUnit::Groups => GROUPS_A_STRETCH,
            StretchUnit::Minutes => MINUTES_A_STRETCH,
        };
        Endurance {
            unit,
            opening_pct: 89.0,
            points: changes
                .iter()
                .enumerate()
                .map(|(at, change)| {
                    let at = at as u32;
                    let from = match unit {
                        StretchUnit::Groups => at * size + 1,
                        StretchUnit::Minutes => at * size,
                    };
                    EndurancePoint {
                        from,
                        to: (at + 1) * size,
                        change: *change,
                        sessions: 3,
                    }
                })
                .collect(),
            sessions: 5,
        }
    }

    #[test]
    fn the_card_says_it_first_in_words() {
        let fading = curve(StretchUnit::Groups, &[0.0, -1.0, -6.0, -9.0]);
        assert_eq!(
            headline(&fading),
            ("Fades after group 10".to_string(), false)
        );
        assert_eq!(
            summary(&fading),
            "You start a session copying 89%; by group 20 you are down to 80%, 9 points lower."
        );
        let steady = curve(StretchUnit::Groups, &[0.0, 2.0, 4.0, 1.0]);
        assert_eq!(
            headline(&steady),
            ("Steady through group 20".to_string(), true)
        );
        assert_eq!(
            summary(&steady),
            "You start a session copying 89% and stay within 5 points of it all the way. You warm up: you are at your best over groups 11–15, 4 points above your start. Longer sessions would show where you tire."
        );
        let run = curve(StretchUnit::Minutes, &[0.0, -5.5]);
        assert_eq!(headline(&run), ("Fades after minute 2".to_string(), false));
        assert_eq!(
            stretch_label(StretchUnit::Minutes, &run.points[1]),
            "2–4 min"
        );
        assert_eq!(
            stretch_label(StretchUnit::Groups, &steady.points[1]),
            "6–10"
        );
    }

    /// The bars stand on a floor ten below the lowest, in tens, the
    /// tallest leaving room for its figure.
    #[test]
    fn the_bars_stand_on_a_floor_below_the_lowest() {
        let fading = curve(StretchUnit::Groups, &[0.0, -1.0, -6.0, -9.0]);
        // 80% at the lowest: the floor is 70%.
        assert_eq!(floor(&fading), 70.0);
        assert_eq!(height(100.0, 70.0), TALLEST);
        assert!((height(85.0, 70.0) - TALLEST / 2.0).abs() < 1e-9);
        assert_eq!(height(50.0, 70.0), 2.0, "never quite nothing");
    }
}
