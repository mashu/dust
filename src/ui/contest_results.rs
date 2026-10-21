//! A contest run's results (§7 of `docs/contest-mode.md`), fresh from the
//! run or opened again from History: the share of QSOs logged clean, a
//! sentence on how it went, where it stands among runs of its contest, its
//! QSOs, rate, accuracy and copy points, its contest score raw and verified
//! with what the checks found, and its log beside what was really sent. A
//! contest without a score, as an everyday QSO is, gives the minutes a QSO
//! in place of the rate, and how well each part of a QSO was copied in
//! place of the score.
//!
//! Two scores, two words: Points are Dust's copy points, as every session
//! earns them; the contest score is how the run operated, and its columns
//! never say Points.

use cw_core::{
    ContestKind, ContestRules, ContestScore, Field, QsoCheck, QsoError, RunErrors, RunQso,
    RunSummary, SessionResult,
};
use dioxus::prelude::*;

use crate::ui::contest::LogRowView;
use crate::ui::format::{counted, counted_in_words, minutes_seconds, short_date};
use crate::ui::results::{PlaceBanner, place_line};
use crate::ui::widgets::{CardHead, Icon, IconName, ScoreRing, control_id};

/// A run's results. Given `on_again`, a run just finished, with Run again;
/// without, one opened again from History, only to be read.
#[component]
pub fn ContestResults(
    result: SessionResult,
    /// Where the run stands among runs of its contest on the board, and of
    /// how many.
    #[props(default)]
    place: Option<(usize, usize)>,
    on_again: Option<EventHandler<()>>,
    on_back: EventHandler<()>,
) -> Element {
    let kind = result.contest_kind().map_or("Contest", ContestKind::label);
    // A contest without a score is told by its pace and its copy.
    let scored = result.contest_kind().is_none_or(ContestKind::scored);
    let rules = result.contest_kind().and_then(ContestKind::rules);
    let run = result.contest_run();
    let groups = &result.groups;
    let log = run
        .as_ref()
        .and_then(|run| run.log(groups, &result.group_timings))
        .unwrap_or_default();
    let errors = run
        .as_ref()
        .and_then(|run| run.errors(groups))
        .unwrap_or_default();
    let score = run.as_ref().and_then(|run| run.score(groups));
    let accuracy = run
        .as_ref()
        .and_then(|run| run.accuracy_by_field(groups))
        .unwrap_or_default();
    let run_ms = run.as_ref().map_or(0, |run| run.run_ms);
    // The figures History shows for the run, from the same reading.
    let figures = run.as_ref().and_then(|run| run.summary(groups));
    let shown_figures = figures.unwrap_or_default();
    let RunSummary {
        qsos, clean, rate, ..
    } = shown_figures;
    let share = (qsos > 0).then(|| clean as f64 / qsos as f64 * 100.0);
    let shown = share.map_or("—".to_string(), |share| format!("{:.0}%", share.round()));
    let acc_pct = (result.accuracy * 100.0).round();
    let mut about = format!("{kind} · {}", minutes_seconds(run_ms));
    if on_again.is_none() {
        // The date kept whole, however narrow the line.
        about = format!(
            "{about} · {}",
            short_date(&result.date).replace(' ', "\u{a0}")
        );
    }
    let said = match &figures {
        Some(figures) => summary(figures, &errors, scored),
        None => "This version cannot read its log.".to_string(),
    };
    rsx! {
        div { class: "stack", id: "run-results",
            // Opened from History, the way back is at the top as well as
            // the bottom: the results are long.
            if on_again.is_none() {
                button {
                    id: control_id("btn", "history"),
                    class: "btn btn-secondary btn-sm run-results-back",
                    onclick: move |_| on_back.call(()),
                    Icon { name: IconName::Back }
                    "History"
                }
            }
            div { class: "card",
                div { class: "score-hero",
                    // One word in the ring, as the group results have; said
                    // whole to a screen reader.
                    div { class: "run-ring", role: "img", aria_label: "{shown} of QSOs logged clean",
                        ScoreRing {
                            pct: share.unwrap_or(0.0),
                            value: shown.clone(),
                            caption: "CLEAN".to_string(),
                        }
                    }
                    div { class: "score-copy",
                        h2 { "Run complete" }
                        p { class: "muted run-about", "{about}" }
                        p { class: "run-summary", "{said}" }
                    }
                }
            }
            if let (Some((place, of)), Some(per_group)) = (place, cw_core::per_group(&result)) {
                {
                    let (text, medal) = place_line(place, of, kind, "run", per_group);
                    rsx! {
                        PlaceBanner { text, place, medal }
                    }
                }
            }
            div { class: "grid-4",
                div { class: "kpi blue",
                    div { class: "tiny", "QSOs" }
                    div { class: "value", "{clean}/{qsos}" }
                }
                if scored {
                    div { class: "kpi emerald",
                        div { class: "tiny", "Rate" }
                        div { class: "value", "{rate_text(rate)}" }
                    }
                } else {
                    div { class: "kpi emerald",
                        div { class: "tiny", "Per QSO" }
                        div { class: "value", "{pace_text(shown_figures.minutes_per_qso())}" }
                    }
                }
                div { class: "kpi purple",
                    div { class: "tiny", "Accuracy" }
                    div { class: "value", "{acc_pct}%" }
                }
                div { class: "kpi indigo",
                    div { class: "tiny", "Points" }
                    div { class: "value", "{result.score.round()}" }
                }
            }
            if let Some(score) = score {
                ScoreTable { score, errors }
            } else if !scored && !accuracy.is_empty() {
                FieldTable { parts: accuracy, errors }
            }
            div { class: "card",
                CardHead {
                    icon: IconName::Letters,
                    title: "Log, with what was really sent",
                    note: "What you logged, and where it was wrong, what the station sent.",
                }
                if log.is_empty() {
                    p { class: "muted", "No QSOs in this run's log." }
                } else {
                    ol { class: "run-log-rows results",
                        for (at, qso) in log.iter().enumerate() {
                            LogRowView {
                                key: "{at}",
                                n: qso.n.map(|n| n.to_string()).unwrap_or_default(),
                                at_ms: qso.at_ms,
                                call: call_text(qso),
                                fields: fields_text(qso, rules),
                                wpm: qso.wpm,
                                asked: qso.asked,
                                check: check_text(qso),
                                wrong: is_wrong(qso),
                            }
                        }
                    }
                }
            }
            div { class: "hero-actions", style: "justify-content: center;",
                if let Some(on_again) = on_again {
                    button {
                        id: control_id("btn", "run again"),
                        class: "btn btn-primary",
                        onclick: move |_| on_again.call(()),
                        Icon { name: IconName::Repeat }
                        "Run again"
                    }
                }
                button {
                    id: control_id("btn", "back"),
                    class: "btn btn-secondary",
                    onclick: move |_| on_back.call(()),
                    Icon { name: IconName::Back }
                    "Back"
                }
            }
        }
    }
}

/// The contest score, raw and verified, and what the checks found.
#[component]
fn ScoreTable(score: ContestScore, errors: RunErrors) -> Element {
    rsx! {
        div { class: "card",
            CardHead {
                icon: IconName::Medal,
                title: "Contest score",
                note: "How you operated: raw as you logged it, verified against what was sent. Points, above, are how you copied.",
            }
            table { class: "score-table",
                thead {
                    tr {
                        th {}
                        th { scope: "col", "QSO pts" }
                        th { scope: "col", "Mults" }
                        th { scope: "col", "Score" }
                    }
                }
                tbody {
                    for (name, tally) in [("Raw", score.raw), ("Verified", score.verified)] {
                        tr {
                            th { scope: "row", "{name}" }
                            td { "{tally.qso_points}" }
                            td { "{tally.mults}" }
                            td { "{tally.score}" }
                        }
                    }
                }
            }
            p { class: "muted score-errors", "{errors_line(&errors)}" }
        }
    }
}

/// How well each part of a QSO was copied over a run without a contest
/// score, the calls and then each field, and what the checks found.
#[component]
fn FieldTable(parts: Vec<(Option<Field>, f64)>, errors: RunErrors) -> Element {
    rsx! {
        div { class: "card",
            CardHead {
                icon: IconName::Target,
                title: "Accuracy per field",
                note: "How much of each you copied right, stations never logged included. Nothing else scores an everyday QSO.",
            }
            table { class: "score-table",
                thead {
                    tr {
                        th {}
                        th { scope: "col", "Accuracy" }
                    }
                }
                tbody {
                    for (part, accuracy) in parts {
                        tr {
                            th { scope: "row", "{part_label(part)}" }
                            td { "{(accuracy * 100.0).round()}%" }
                        }
                    }
                }
            }
            p { class: "muted score-errors", "{errors_line(&errors)}" }
        }
    }
}

/// A part of a QSO as a log heads it: `CALL`, or the field's `RST`.
fn part_label(part: Option<Field>) -> &'static str {
    part.map_or("CALL", Field::label)
}

/// Minutes a QSO, as a tile says it: "1.4 min".
fn pace_text(minutes: Option<f64>) -> String {
    minutes.map_or_else(|| "—".to_string(), |minutes| format!("{minutes:.1} min"))
}

/// QSOs an hour, as a tile says it: "72/h".
fn rate_text(rate: Option<f64>) -> String {
    rate.map_or_else(|| "—".to_string(), |rate| format!("{rate:.0}/h"))
}

/// "0 busted calls · 1 wrong NR · 0 NIL · 0 dupes".
fn errors_line(errors: &RunErrors) -> String {
    let mut parts = vec![counted(errors.busted_calls, "busted call")];
    parts.extend(
        errors
            .wrong
            .iter()
            .map(|(field, count)| format!("{count} wrong {}", field.label())),
    );
    parts.push(format!("{} NIL", errors.nils));
    parts.push(counted(errors.dupes, "dupe"));
    // One piece each, and the dot at the end of a line, not the start of
    // the next, where a narrow line breaks.
    let parts: Vec<String> = parts
        .iter()
        .map(|part| part.replace(' ', "\u{a0}"))
        .collect();
    parts.join("\u{a0}· ")
}

/// `n` of them was, or were.
fn was(n: usize) -> &'static str {
    if n == 1 { "was" } else { "were" }
}

/// The run in a sentence or two: "12 QSOs, 72 an hour. Two QSOs needed a
/// repeat; one NR was wrong." A run not `scored` gives its pace in minutes:
/// "8 QSOs, 1.2 minutes each."
fn summary(figures: &RunSummary, errors: &RunErrors, scored: bool) -> String {
    let RunSummary {
        qsos,
        rate,
        repeated,
        lost,
        ..
    } = *figures;
    if qsos == 0 {
        return "No QSO logged: every station you worked was lost.".to_string();
    }
    let opening = match (rate, figures.minutes_per_qso()) {
        (Some(rate), _) if scored => format!("{}, {rate:.0} an hour.", counted(qsos, "QSO")),
        (_, Some(minutes)) if !scored => {
            format!("{}, {minutes:.1} minutes each.", counted(qsos, "QSO"))
        }
        _ => format!("{}.", counted(qsos, "QSO")),
    };
    let mut notes = Vec::new();
    if repeated > 0 {
        notes.push(format!(
            "{} needed a repeat",
            counted_in_words(repeated, "QSO")
        ));
    }
    let busted = errors.busted_calls as usize;
    if busted > 0 {
        notes.push(format!(
            "{} {} busted",
            counted_in_words(busted, "call"),
            was(busted)
        ));
    }
    for (field, count) in &errors.wrong {
        let count = *count as usize;
        if count > 0 {
            notes.push(format!(
                "{} {} wrong",
                counted_in_words(count, field.label()),
                was(count)
            ));
        }
    }
    if lost > 0 {
        notes.push(format!(
            "{} {} never logged",
            counted_in_words(lost, "station"),
            was(lost)
        ));
    }
    let nils = errors.nils as usize;
    if nils > 0 {
        notes.push(format!(
            "{} {} not in their log",
            counted_in_words(nils, "QSO"),
            was(nils)
        ));
    }
    let wrong = busted + nils + errors.wrong.iter().map(|(_, n)| *n as usize).sum::<usize>();
    if wrong == 0 {
        notes.push("every one logged clean".to_string());
    }
    let mut notes = notes.join("; ");
    if let Some(first) = notes.get(..1) {
        notes = format!("{}{}", first.to_uppercase(), &notes[1..]);
    }
    format!("{opening} {notes}.")
}

/// Whether a QSO went wrong: a check that found something, or never logged.
fn is_wrong(qso: &RunQso) -> bool {
    qso.check.as_ref().is_none_or(|check| !check.is_clean())
}

/// The call as logged, and the station's where they differ: "K5ABD→K5ABC".
fn call_text(qso: &RunQso) -> String {
    let busted = qso.check.as_ref().is_some_and(|check| {
        check
            .errors
            .iter()
            .any(|error| matches!(error, QsoError::Call { .. }))
    });
    let unlogged_slip = qso.check.is_none() && !qso.station.is_empty() && qso.call != qso.station;
    if busted || unlogged_slip {
        format!("{}→{}", qso.call, qso.station)
    } else {
        qso.call.clone()
    }
}

/// What was logged of the exchange, and what was sent where it differs as
/// the run's contest reads it, `rules`: "13→31".
fn fields_text(qso: &RunQso, rules: Option<&ContestRules>) -> String {
    let fields: Vec<String> = qso
        .fields
        .iter()
        .map(|(field, typed, sent)| {
            let same = rules.map_or_else(
                || field.same(typed, sent),
                |rules| rules.same(*field, typed, sent),
            );
            if sent.is_empty() || same {
                typed.clone()
            } else if typed.is_empty() {
                format!("—→{sent}")
            } else {
                format!("{typed}→{sent}")
            }
        })
        .collect();
    fields.join(" ")
}

/// How a QSO checked, in words; one never logged says so.
fn check_text(qso: &RunQso) -> String {
    qso.check
        .as_ref()
        .map_or_else(|| "not logged".to_string(), QsoCheck::words)
}

/// A run that worked nobody keeps nothing, and the results screen says so,
/// with another run a press away.
#[component]
pub fn NothingKept(on_again: EventHandler<()>, on_home: EventHandler<()>) -> Element {
    rsx! {
        div { class: "stack",
            div { class: "card nothing-kept",
                h2 { "No stations worked — nothing kept" }
                p { class: "muted",
                    "A run is kept once you have worked a station. Call CQ, or press Enter, and copy the call that answers."
                }
            }
            div { class: "hero-actions", style: "justify-content: center;",
                button {
                    id: control_id("btn", "run again"),
                    class: "btn btn-primary",
                    onclick: move |_| on_again.call(()),
                    Icon { name: IconName::Repeat }
                    "Run again"
                }
                button {
                    id: control_id("btn", "back"),
                    class: "btn btn-secondary",
                    onclick: move |_| on_home.call(()),
                    Icon { name: IconName::Back }
                    "Back"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(busted: u32, wrong: u32, nils: u32, dupes: u32) -> RunErrors {
        RunErrors {
            busted_calls: busted,
            wrong: vec![(Field::Nr, wrong)],
            nils,
            dupes,
        }
    }

    /// A run's figures: QSOs logged, the rate, those you asked a repeat
    /// for, and stations never logged.
    fn figures(qsos: usize, rate: Option<f64>, repeated: usize, lost: usize) -> RunSummary {
        RunSummary {
            qsos,
            clean: 0,
            rate,
            repeated,
            lost,
        }
    }

    /// The sentence a run with a contest score gets.
    fn summary_of(figures: &RunSummary, errors: &RunErrors) -> String {
        summary(figures, errors, true)
    }

    /// A run without a score is said with its pace in minutes.
    #[test]
    fn a_run_without_a_score_is_said_with_its_pace() {
        assert_eq!(
            summary(&figures(8, Some(48.0), 0, 0), &errors(0, 0, 0, 0), false),
            "8 QSOs, 1.2 minutes each. Every one logged clean."
        );
        assert_eq!(
            summary(&figures(3, None, 0, 0), &errors(0, 0, 0, 0), false),
            "3 QSOs. Every one logged clean."
        );
        assert_eq!(pace_text(Some(1.24)), "1.2 min");
        assert_eq!(
            (part_label(None), part_label(Some(Field::Name))),
            ("CALL", "NAME")
        );
        assert_eq!(pace_text(None), "—");
    }

    #[test]
    fn a_run_is_said_in_a_sentence() {
        assert_eq!(
            summary_of(&figures(12, Some(72.0), 2, 0), &errors(0, 1, 0, 0)),
            "12 QSOs, 72 an hour. Two QSOs needed a repeat; one NR was wrong."
        );
        assert_eq!(
            summary_of(&figures(1, Some(6.0), 0, 0), &errors(0, 0, 0, 1)),
            "1 QSO, 6 an hour. Every one logged clean."
        );
        assert_eq!(
            summary_of(&figures(14, None, 0, 0), &errors(1, 12, 2, 0)),
            "14 QSOs. One call was busted; 12 NRs were wrong; two QSOs were not in their log."
        );
        assert_eq!(
            summary_of(&figures(2, Some(12.0), 0, 1), &errors(0, 0, 0, 0)),
            "2 QSOs, 12 an hour. One station was never logged; every one logged clean."
        );
        assert_eq!(
            summary_of(&figures(0, Some(0.0), 0, 1), &errors(0, 0, 0, 0)),
            "No QSO logged: every station you worked was lost."
        );
    }

    #[test]
    fn the_checks_are_counted_under_the_score() {
        let line = errors_line(&errors(0, 1, 0, 0));
        assert_eq!(
            line.replace('\u{a0}', " "),
            "0 busted calls · 1 wrong NR · 0 NIL · 0 dupes"
        );
        // A narrow line breaks only after a dot.
        assert_eq!(
            line.split(' ').collect::<Vec<_>>(),
            [
                "0\u{a0}busted\u{a0}calls\u{a0}·",
                "1\u{a0}wrong\u{a0}NR\u{a0}·",
                "0\u{a0}NIL\u{a0}·",
                "0\u{a0}dupes"
            ]
        );
        assert_eq!(
            errors_line(&errors(1, 0, 2, 1)).replace('\u{a0}', " "),
            "1 busted call · 0 wrong NR · 2 NIL · 1 dupe"
        );
        assert_eq!(rate_text(Some(71.6)), "72/h");
        assert_eq!(rate_text(None), "—");
    }

    fn qso(call: &str, station: &str, typed: &str, sent: &str, check: Option<QsoCheck>) -> RunQso {
        RunQso {
            n: check.as_ref().map(|_| 2),
            at_ms: 80_000,
            call: call.into(),
            station: station.into(),
            fields: vec![(Field::Nr, typed.into(), sent.into())],
            wpm: Some(26.0),
            asked: 1,
            check,
        }
    }

    /// The log shows what was really sent wherever it differs from what was
    /// logged, and says how each QSO checked.
    #[test]
    fn the_log_shows_what_was_really_sent() {
        let wrong = QsoCheck {
            errors: vec![QsoError::Field(Field::Nr)],
            dupe: false,
        };
        let row = qso("DL2XYZ", "DL2XYZ", "13", "31", Some(wrong));
        assert_eq!(
            (call_text(&row), fields_text(&row, None), check_text(&row)),
            ("DL2XYZ".into(), "13→31".into(), "wrong NR".into())
        );
        assert!(is_wrong(&row));
        let busted = QsoCheck {
            errors: vec![QsoError::Call {
                correct: "K5ABC".into(),
            }],
            dupe: false,
        };
        let row = qso("K5ABD", "K5ABC", "7", "TT7", Some(busted));
        assert_eq!(
            (call_text(&row), fields_text(&row, None)),
            ("K5ABD→K5ABC".into(), "7".into()),
            "a cut number logged plain is right"
        );
        let clean = qso("K5ABC", "K5ABC", "214", "214", Some(QsoCheck::default()));
        assert!(!is_wrong(&clean));
        let lost = qso("G4AB", "G4ABC", "", "TT7", None);
        assert_eq!(
            (
                call_text(&lost),
                fields_text(&lost, None),
                check_text(&lost)
            ),
            ("G4AB→G4ABC".into(), "—→TT7".into(), "not logged".into())
        );
        assert!(is_wrong(&lost));
    }
}
