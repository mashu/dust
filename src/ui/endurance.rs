//! The Endurance card: how far into a session your copy holds before it
//! drops, for each character set with sessions long enough to tell.

use cw_core::{
    DROP_POINTS, ENDURANCE_MIN_SESSIONS, Endurance, GROUPS_A_STRETCH, MINUTES_A_STRETCH,
    SessionResult, StretchUnit, TrainingSettings, endurance, practised_modes,
};
use dioxus::prelude::*;

use crate::ui::chart::{CHART_H, CHART_W, marks, path, series};
use crate::ui::settings_charset::mode_label;
use crate::ui::stats::shown_mode;
use crate::ui::widgets::{CardHead, IconName, Seg, control_id};

/// Room above and below the line, so its dots are not cut by the edge.
const PAD: f64 = 8.0;

/// How a session's place reads: "group 20", "minute 6".
fn place(unit: StretchUnit, at: u32) -> String {
    match unit {
        StretchUnit::Groups => format!("group {at}"),
        StretchUnit::Minutes => format!("minute {at}"),
    }
}

/// What the curve says: how far your copy holds within [`DROP_POINTS`] of
/// how you start, and where it goes from there.
fn verdict(curve: &Endurance) -> String {
    let holds = place(curve.unit, curve.holds_to());
    let last = curve.points.last().map_or(0, |point| point.to);
    let of = match curve.unit {
        StretchUnit::Groups => "a session",
        StretchUnit::Minutes => "a run",
    };
    match curve.first_drop() {
        Some(_) => {
            let lowest = curve
                .points
                .iter()
                .map(|point| point.change)
                .fold(0.0_f64, f64::min);
            let low_at = curve
                .points
                .iter()
                .find(|point| point.change == lowest)
                .map_or(last, |point| point.to);
            format!(
                "Your copy holds within {DROP_POINTS:.0} points of how you start {of} through {holds}, and is {:.0} points down by {}.",
                -lowest,
                place(curve.unit, low_at)
            )
        }
        None => format!(
            "Your copy holds within {DROP_POINTS:.0} points of how you start {of} through {holds}, as far as your sessions go: longer ones would show where it drops."
        ),
    }
}

/// The short form, for the chip.
fn chip(curve: &Endurance) -> (String, &'static str) {
    let holds = place(curve.unit, curve.holds_to());
    match curve.first_drop() {
        Some(_) => (format!("Drops after {holds}"), "chip warn"),
        None => (format!("Holds to {holds}"), "chip good"),
    }
}

/// The chart's scale, in points, bottom to top: whole fives, never less
/// than ten below the start and five above it.
fn scale(curve: &Endurance) -> (f64, f64) {
    let changes = curve.points.iter().map(|point| point.change);
    let low = changes.clone().fold(-10.0_f64, f64::min);
    let high = changes.fold(5.0_f64, f64::max);
    // A hair of give, so that thirty points down, worked out as a hair
    // past thirty, still scales to thirty.
    const GIVE: f64 = 1e-9;
    (
        ((low + GIVE) / 5.0).floor() * 5.0,
        ((high - GIVE) / 5.0).ceil() * 5.0,
    )
}

/// Where a change of `points` sits on a chart scaled `low` to `high`.
fn y_at(points: f64, low: f64, high: f64) -> f64 {
    PAD + (high - points) / (high - low) * (CHART_H - 2.0 * PAD)
}

/// What the line is measured against, in words.
fn against(unit: StretchUnit) -> String {
    match unit {
        StretchUnit::Groups => format!(
            "Change in character accuracy from each session's first {GROUPS_A_STRETCH} groups"
        ),
        StretchUnit::Minutes => format!(
            "Change in character accuracy from each run's first {MINUTES_A_STRETCH} minutes"
        ),
    }
}

/// How copy holds up through a session, for each character set with enough
/// long sessions, against the session's own start; and before there are,
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
                CardHead { icon: IconName::Timer, title: "Endurance" }
                p { class: "muted", style: "margin: 0;",
                    "After {ENDURANCE_MIN_SESSIONS} sessions of {2 * GROUPS_A_STRETCH} groups or more, or contest runs of {2 * MINUTES_A_STRETCH} minutes or more, this shows how far into a session your copy holds before it drops."
                }
            }
        };
    };
    let mode = *mode;
    let (low, high) = scale(curve);
    let changes: Vec<f64> = curve.points.iter().map(|point| point.change).collect();
    let coords = series(&changes, |points| y_at(points, low, high));
    let dots = marks(&coords, coords.len());
    let zero = y_at(0.0, low, high);
    let drop = y_at(-DROP_POINTS, low, high);
    let (first, last) = (
        curve.points.first().map_or(0, |point| point.from),
        curve.points.last().map_or(0, |point| point.to),
    );
    let span = match curve.unit {
        StretchUnit::Groups => format!("Groups {first} to {last}"),
        StretchUnit::Minutes => format!("Minutes {first} to {last}"),
    };
    let (chip_text, chip_class) = chip(curve);
    rsx! {
        div { class: "card chart-card",
            CardHead {
                icon: IconName::Timer,
                title: "Endurance",
                note: format!(
                    "{:.0}% at the start · {}",
                    curve.opening_pct,
                    crate::ui::format::counted(curve.sessions, "session"),
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
            svg {
                class: "sparkline",
                view_box: "0 0 420 96",
                rect {
                    x: "0",
                    y: "{drop:.1}",
                    width: "{CHART_W}",
                    height: "{(CHART_H - drop).max(0.0):.1}",
                    fill: "var(--bad)",
                    fill_opacity: "0.07",
                }
                line {
                    x1: "0",
                    y1: "{zero:.1}",
                    x2: "420",
                    y2: "{zero:.1}",
                    stroke: "var(--ink-dim)",
                    stroke_width: "1",
                    stroke_dasharray: "4 4",
                    opacity: "0.65",
                }
                path {
                    d: "{path(coords.iter().copied())}",
                    fill: "none",
                    stroke: "var(--violet)",
                    stroke_width: "2.2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                }
                for (x, y) in dots.iter() {
                    circle {
                        cx: "{x:.1}",
                        cy: "{y:.1}",
                        r: "3",
                        fill: "var(--violet)",
                        stroke: "var(--surface)",
                        stroke_width: "1.5",
                    }
                }
            }
            div { class: "row-between",
                span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                    "{span} · scale {low:+.0} to {high:+.0} points"
                }
                span { class: chip_class, "{chip_text}" }
            }
            p { class: "muted", style: "margin: 0.6rem 0 0; font-size: 0.85rem;",
                "{against(curve.unit)}, the dashed line; the shaded band is {DROP_POINTS:.0} points or more below it. {verdict(curve)}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::EndurancePoint;

    fn curve(unit: StretchUnit, changes: &[f64]) -> Endurance {
        let size = match unit {
            StretchUnit::Groups => GROUPS_A_STRETCH,
            StretchUnit::Minutes => MINUTES_A_STRETCH,
        };
        Endurance {
            unit,
            opening_pct: 92.0,
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
            sessions: 3,
        }
    }

    #[test]
    fn the_card_says_how_far_copy_holds_and_where_it_goes() {
        let dropping = curve(StretchUnit::Groups, &[0.0, -1.0, -6.0, -9.0, -7.0]);
        assert_eq!(
            verdict(&dropping),
            "Your copy holds within 5 points of how you start a session through group 10, and is 9 points down by group 20."
        );
        assert_eq!(
            chip(&dropping),
            ("Drops after group 10".to_string(), "chip warn")
        );
        let holding = curve(StretchUnit::Groups, &[0.0, 2.0, -3.0, -4.0]);
        assert_eq!(
            verdict(&holding),
            "Your copy holds within 5 points of how you start a session through group 20, as far as your sessions go: longer ones would show where it drops."
        );
        assert_eq!(
            chip(&holding),
            ("Holds to group 20".to_string(), "chip good")
        );
        let run = curve(StretchUnit::Minutes, &[0.0, -5.5]);
        assert_eq!(
            verdict(&run),
            "Your copy holds within 5 points of how you start a run through minute 2, and is 6 points down by minute 4."
        );
    }

    /// The scale is whole fives, ten below the start and five above it at
    /// the least, and wider for a curve that goes further.
    #[test]
    fn the_scale_takes_in_the_whole_curve() {
        assert_eq!(
            scale(&curve(StretchUnit::Groups, &[0.0, -2.0])),
            (-10.0, 5.0)
        );
        assert_eq!(
            scale(&curve(StretchUnit::Groups, &[0.0, 7.0, -12.5])),
            (-15.0, 10.0)
        );
        assert_eq!(
            scale(&curve(StretchUnit::Groups, &[0.0, 100.0 * (0.7 - 1.0)])),
            (-30.0, 5.0)
        );
        assert_eq!(y_at(5.0, -10.0, 5.0), PAD);
        assert_eq!(y_at(-10.0, -10.0, 5.0), CHART_H - PAD);
    }
}
