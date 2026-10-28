use cw_core::{BoardEntry, FORM_SESSIONS, Ranking, TimelinePoint};
use dioxus::prelude::*;

use super::{focus, rating_said, what_it_was};
use crate::ui::chart::path;
use crate::ui::format::{clock, counted, ordinal, short_date};
use crate::ui::help;
use crate::ui::widgets::{CardHead, IconName};

const TIMELINE_W: f64 = 420.0;
const TIMELINE_H: f64 = 120.0;
/// Room at the chart's edges, so the outermost dots are not cut in half.
const TIMELINE_PAD: f64 = 8.0;

/// The best rating on the timeline: the top of its scale.
fn timeline_top(points: &[TimelinePoint]) -> f64 {
    points.iter().map(|point| point.rating).fold(0.0, f64::max)
}

/// How high `value` sits on a scale whose top is `top`.
fn timeline_y(value: f64, top: f64) -> f64 {
    if top > 0.0 {
        TIMELINE_PAD + (1.0 - value / top) * (TIMELINE_H - 2.0 * TIMELINE_PAD)
    } else {
        TIMELINE_H - TIMELINE_PAD
    }
}

/// Where each session sits on the timeline: across by when it was set, up
/// by what it is ranked by, against the best.
fn timeline_xy(points: &[TimelinePoint]) -> Vec<(f64, f64)> {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    let span = last.timestamp.saturating_sub(first.timestamp) as f64;
    let top = timeline_top(points);
    let width = TIMELINE_W - 2.0 * TIMELINE_PAD;
    points
        .iter()
        .map(|point| {
            let x = if span > 0.0 {
                TIMELINE_PAD + (point.timestamp - first.timestamp) as f64 / span * width
            } else {
                TIMELINE_W / 2.0
            };
            (x, timeline_y(point.rating, top))
        })
        .collect()
}

/// The form as it went: a line through the median after each session.
fn form_path(points: &[TimelinePoint], xy: &[(f64, f64)]) -> String {
    let top = timeline_top(points);
    path(
        points
            .iter()
            .zip(xy)
            .map(|(point, (x, _))| (*x, timeline_y(point.form, top))),
    )
}

/// The record as it stood over time: a step up at every session that set
/// one, flat in between, on to the last session.
fn record_path(points: &[TimelinePoint], xy: &[(f64, f64)]) -> String {
    let top = timeline_top(points);
    if top <= 0.0 {
        return String::new();
    }
    let mut corners = Vec::new();
    let mut level: Option<f64> = None;
    for (point, (x, _)) in points.iter().zip(xy) {
        let y = timeline_y(point.record, top);
        match level {
            None => corners.push((*x, y)),
            Some(before) if (before - y).abs() > f64::EPSILON => {
                corners.push((*x, before));
                corners.push((*x, y));
            }
            Some(_) => {}
        }
        level = Some(y);
    }
    if let (Some(y), Some((x, _))) = (level, xy.last()) {
        corners.push((*x, y));
    }
    path(corners)
}

/// What the board ranks by over time: a dot a session, larger where it set
/// the record, the record as it stood and the form through them, with the
/// session marked said in full beneath. Marking another redraws the mark
/// and what is said of it, not the dots.
#[component]
pub(super) fn ScoreTimeline(
    /// Every session on the board, oldest first.
    timeline: Vec<TimelinePoint>,
    /// The board's rows, best first.
    board: Vec<BoardEntry>,
    /// When the best was set, marked while no row is.
    best_at: u64,
    /// The row pointed at, else the row tapped.
    marked: Memo<Option<u64>>,
    /// How many sessions are on the board.
    total: usize,
    /// The dates of the first session and the last, under either end.
    first_date: String,
    last_date: String,
    /// What the board ranks by.
    ranking: Ranking,
) -> Element {
    #[cfg(test)]
    crate::testing::rendered("ScoreTimeline");
    let xy = timeline_xy(&timeline);
    let record = record_path(&timeline, &xy);
    let form_line = form_path(&timeline, &xy);
    let spots: Vec<(u64, f64, f64)> = timeline
        .iter()
        .zip(&xy)
        .map(|(point, (x, y))| (point.timestamp, *x, *y))
        .collect();
    let records = timeline.iter().filter(|point| point.set_record).count();
    let (title, noun) = match ranking {
        Ranking::PerGroup => ("Points a group over time", "session"),
        Ranking::Verified(_) => ("Verified score over time", "run"),
        Ranking::CleanRate => ("Clean QSOs an hour over time", "run"),
    };
    rsx! {
        div { class: "card chart-card",
            CardHead {
                icon: IconName::Chart,
                title,
                help: help::timeline(ranking),
                note: format!(
                    "{} · the record set {}",
                    counted(total, noun),
                    if records == 1 { "once".to_string() } else { format!("{records} times") }
                ),
            }
            svg {
                class: "sparkline score-timeline",
                view_box: "0 0 420 120",
                if timeline.len() > 1 {
                    path { class: "form-line", d: "{form_line}" }
                }
                if !record.is_empty() {
                    path { class: "record-line", d: "{record}" }
                }
                TimelineMark {
                    spots: spots.clone(),
                    best_at,
                    marked,
                    ring: false,
                }
                for (point, (x, y)) in timeline.iter().zip(xy.iter()) {
                    circle {
                        class: if point.set_record { "timeline-dot record" } else { "timeline-dot" },
                        cx: "{x:.1}",
                        cy: "{y:.1}",
                        r: if point.set_record { "3.4" } else { "2.6" },
                    }
                }
                TimelineMark {
                    spots,
                    best_at,
                    marked,
                    ring: true,
                }
            }
            TimelineFocus {
                board,
                best_at,
                marked,
                ranking,
            }
            div { class: "timeline-legend",
                span { class: "legend-record", "Record" }
                span { class: "legend-form", "Form: median of the last {FORM_SESSIONS}" }
            }
            div { class: "row-between",
                span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                    "{first_date}"
                }
                span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                    "{last_date}"
                }
            }
        }
    }
}

/// The session marked, on the timeline: the guide down to it, under the
/// dots, or the ring around its dot, over them.
#[component]
fn TimelineMark(
    /// When each session was set, and where its dot sits.
    spots: Vec<(u64, f64, f64)>,
    best_at: u64,
    marked: Memo<Option<u64>>,
    /// The ring, else the guide.
    ring: bool,
) -> Element {
    let focus = focus(marked, best_at);
    let Some(&(_, x, y)) = spots.iter().find(|(at, ..)| *at == focus) else {
        return rsx! {};
    };
    if ring {
        rsx! {
            circle { class: "timeline-mark", cx: "{x:.1}", cy: "{y:.1}", r: "6.5" }
        }
    } else {
        rsx! {
            line {
                class: "timeline-guide",
                x1: "{x:.1}",
                y1: "0",
                x2: "{x:.1}",
                y2: "120",
            }
        }
    }
}

/// The session marked, said in full under the timeline.
#[component]
fn TimelineFocus(
    board: Vec<BoardEntry>,
    best_at: u64,
    marked: Memo<Option<u64>>,
    ranking: Ranking,
) -> Element {
    let focus = focus(marked, best_at);
    let Some(entry) = board.iter().find(|entry| entry.timestamp == focus) else {
        return rsx! {};
    };
    rsx! {
        p { class: "timeline-focus",
            span { class: "timeline-focus-place", "{ordinal(entry.place)}" }
            " · {rating_said(ranking, entry.rating)} · {what_it_was(entry)} · {short_date(&entry.date)}"
            if let Some(time) = clock(entry.timestamp) {
                " {time}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(timestamp: u64, per_group: f64, record: f64, set_record: bool) -> TimelinePoint {
        TimelinePoint {
            index: 0,
            timestamp,
            rating: per_group,
            record,
            set_record,
            form: per_group,
        }
    }

    /// Across by time, up by score: the best at the top, the first and last
    /// at the edges.
    #[test]
    fn the_timeline_spreads_sessions_by_when_they_were_set() {
        let points = [
            point(0, 100.0, 100.0, true),
            point(50, 50.0, 100.0, false),
            point(100, 200.0, 200.0, true),
        ];
        let xy = timeline_xy(&points);
        assert_eq!(xy[0].0, TIMELINE_PAD);
        assert_eq!(xy[1].0, TIMELINE_W / 2.0);
        assert_eq!(xy[2].0, TIMELINE_W - TIMELINE_PAD);
        assert_eq!(xy[2].1, TIMELINE_PAD, "the best is at the top");
        assert!(xy[1].1 > xy[0].1, "a lower score sits lower");
        // One session sits in the middle.
        let one = timeline_xy(&[point(5, 10.0, 10.0, true)]);
        assert_eq!(one[0].0, TIMELINE_W / 2.0);
        assert!(timeline_xy(&[]).is_empty());
    }

    /// The record steps up where it was set and stays flat in between.
    #[test]
    fn the_record_line_steps_up_at_each_new_record() {
        let points = [
            point(0, 100.0, 100.0, true),
            point(50, 50.0, 100.0, false),
            point(100, 200.0, 200.0, true),
        ];
        let xy = timeline_xy(&points);
        let path = record_path(&points, &xy);
        assert!(path.starts_with("M8.0,"), "{path}");
        // Flat to the third session, then up to the new record.
        assert_eq!(path.matches(" L").count(), 3, "{path}");
        assert!(
            path.ends_with(&format!("L{:.1},{:.1}", xy[2].0, xy[2].1)),
            "{path}"
        );
        assert!(record_path(&[], &[]).is_empty());
    }

    #[test]
    fn the_form_line_runs_through_every_session() {
        let mut points = [
            point(0, 100.0, 100.0, true),
            point(50, 50.0, 100.0, false),
            point(100, 200.0, 200.0, true),
        ];
        points[2].form = 100.0;
        let xy = timeline_xy(&points);
        let path = form_path(&points, &xy);
        assert!(path.starts_with("M8.0,"), "{path}");
        assert_eq!(path.matches(" L").count(), 2, "{path}");
        // The last form sits halfway up a scale topped by 200.
        let half = timeline_y(100.0, 200.0);
        assert!(path.ends_with(&format!("{half:.1}")), "{path}");
    }
}
