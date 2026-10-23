//! The Letter accuracy card: how often you copy each character right, how
//! sure that figure is, which way it is going, and how fast you answer it,
//! the ones that need the most work first.
//!
//! It is drawn the way "What comes up next" is: a band a row, inked where
//! the accuracy is likely and fading where it is not, a dot at what was
//! measured, and the part of the scale under the Mastered line washed
//! copper. A summary above names the weak, the building and the ones with
//! too few tries to tell; a character tried once is no longer called weak.

use cw_core::{
    CONTEST_TIMES_NOTE, CharSetMode, CharacterDiagnostic, MASTERED_MIN_ACCURACY, MasteryStatus,
    SessionResult, character_diagnostics,
};
use dioxus::prelude::*;

use crate::ui::bigram::id_part;
use crate::ui::widgets::{CardHead, IconName, control_id};

/// The share of the belief the likely range holds.
const LIKELY: f64 = 0.8;
/// Steps the belief is read in, and stops the band is drawn with.
const GRID: usize = 600;
const STOPS: usize = 40;
/// The density the band is at its darkest from.
const FULL_INK: f64 = 8.0;
/// How far recent accuracy has to move from earlier, in points, to be a
/// trend rather than noise.
const TREND_POINTS: f64 = 5.0;

fn status_class(status: MasteryStatus) -> &'static str {
    match status {
        MasteryStatus::Mastered => "solid",
        MasteryStatus::Building => "building",
        MasteryStatus::Weak => "to-work-on",
        MasteryStatus::FewTries => "finding-out",
    }
}

fn status_word(status: MasteryStatus) -> &'static str {
    match status {
        MasteryStatus::Mastered => "mastered",
        MasteryStatus::Building => "building",
        MasteryStatus::Weak => "weak",
        MasteryStatus::FewTries => "few tries",
    }
}

/// The left end of the scale: half right, or lower, in tens, when a
/// character is worse than that.
fn scale_bottom(letters: &[CharacterDiagnostic]) -> f64 {
    let worst = letters
        .iter()
        .map(|letter| letter.accuracy_pct / 100.0)
        .fold(1.0_f64, f64::min);
    ((worst * 10.0).floor() / 10.0).clamp(0.0, 0.5)
}

/// Where along the scale from `bottom` to all right, in percent.
fn along(accuracy: f64, bottom: f64) -> f64 {
    ((accuracy - bottom) / (1.0 - bottom) * 100.0).clamp(0.0, 100.0)
}

/// Which way a character is going: up or down by enough points, steady, or
/// nothing to say yet.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Trend {
    Up(f64),
    Down(f64),
    Steady,
    Unknown,
}

fn trend(letter: &CharacterDiagnostic) -> Trend {
    match (letter.recent_pct, letter.earlier_pct) {
        (Some(recent), Some(earlier)) if recent - earlier >= TREND_POINTS => {
            Trend::Up(recent - earlier)
        }
        (Some(recent), Some(earlier)) if earlier - recent >= TREND_POINTS => {
            Trend::Down(earlier - recent)
        }
        (Some(_), Some(_)) => Trend::Steady,
        _ => Trend::Unknown,
    }
}

fn seconds(ms: f64) -> String {
    format!("{:.1} s", ms / 1000.0)
}

/// What a screen reader hears for a row.
fn letter_label(letter: &CharacterDiagnostic) -> String {
    let (low, high) = letter.accuracy_interval(LIKELY);
    let mut text = format!(
        "{}: {:.0}% right of {}, likely {:.0}% to {:.0}%, {}",
        letter.letter,
        letter.accuracy_pct,
        letter.total,
        low * 100.0,
        high * 100.0,
        status_word(letter.status)
    );
    match trend(letter) {
        Trend::Up(points) => text.push_str(&format!(", up {points:.0} points lately")),
        Trend::Down(points) => text.push_str(&format!(", down {points:.0} points lately")),
        Trend::Steady => text.push_str(", steady lately"),
        Trend::Unknown => {}
    }
    if letter.avg_ms > 0.0 {
        text.push_str(&format!(
            ", answered in {}{}",
            seconds(letter.avg_ms),
            if letter.is_slow { ", slow" } else { "" }
        ));
    }
    text
}

/// The band: where along the scale, in percent, and how dark.
fn band(letter: &CharacterDiagnostic, bottom: f64) -> Vec<(f64, f64)> {
    let density = letter.accuracy_density(GRID);
    (0..STOPS)
        .map(|stop| {
            let along = stop as f64 / (STOPS - 1) as f64;
            let accuracy = bottom + along * (1.0 - bottom);
            let at = ((accuracy * GRID as f64) as usize).min(GRID - 1);
            let ink = (density[at] / FULL_INK).min(1.0);
            (along * 100.0, 0.06 + 0.64 * ink)
        })
        .collect()
}

fn row_view(letter: &CharacterDiagnostic, bottom: f64) -> Element {
    let gradient = format!("acc-{}", u32::from(letter.letter));
    let (trend_class, trend_text) = match trend(letter) {
        Trend::Up(points) => ("acc-trend up", format!("↑ {points:.0}")),
        Trend::Down(points) => ("acc-trend down", format!("↓ {points:.0}")),
        Trend::Steady => ("acc-trend", "→".to_string()),
        Trend::Unknown => ("acc-trend", String::new()),
    };
    rsx! {
        div {
            id: control_id("letter", &id_part(letter.letter)),
            class: "err-row letters {status_class(letter.status)}",
            role: "listitem",
            aria_label: "{letter_label(letter)}",
            span { class: "letter-key err-key group-text", "{letter.letter}" }
            div {
                class: "err-plot below",
                style: "--thr: {along(MASTERED_MIN_ACCURACY, bottom):.1}%;",
                svg {
                    class: "err-band",
                    view_box: "0 0 100 10",
                    preserve_aspect_ratio: "none",
                    defs {
                        linearGradient { id: "{gradient}", x1: "0", x2: "1",
                            for (offset, ink) in band(letter, bottom) {
                                stop {
                                    offset: "{offset:.1}%",
                                    style: "stop-color: var(--tone); stop-opacity: {ink:.3};",
                                }
                            }
                        }
                    }
                    rect {
                        x: "0",
                        y: "2",
                        width: "100",
                        height: "6",
                        rx: "1.2",
                        ry: "3",
                        fill: "url(#{gradient})",
                    }
                }
                span {
                    class: "err-dot",
                    style: "left: {along(letter.accuracy_pct / 100.0, bottom):.1}%;",
                }
            }
            span { class: trend_class, "{trend_text}" }
            span { class: if letter.is_slow { "acc-time slow" } else { "acc-time" },
                if letter.avg_ms > 0.0 {
                    "{seconds(letter.avg_ms)}"
                } else {
                    "—"
                }
            }
        }
    }
}

/// The characters of `letters` with `status`.
fn with_status(letters: &[CharacterDiagnostic], status: MasteryStatus) -> Vec<char> {
    letters
        .iter()
        .filter(|letter| letter.status == status)
        .map(|letter| letter.letter)
        .collect()
}

#[component]
pub fn LettersTab(sessions: Vec<SessionResult>) -> Element {
    let letters = character_diagnostics(&sessions);
    if letters.is_empty() {
        return rsx! {
            div { class: "card", p { class: "muted", "No letter data yet." } }
        };
    }
    let contest = sessions
        .iter()
        .any(|session| session.char_set_mode == CharSetMode::Contest);
    let bottom = scale_bottom(&letters);
    let step = if bottom < 0.4 { 0.2 } else { 0.1 };
    let ticks: Vec<f64> = (0..)
        .map(|n| 1.0 - f64::from(n) * step)
        .take_while(|tick| *tick >= bottom - 1e-9)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let groups = [
        (MasteryStatus::Weak, "Weak"),
        (MasteryStatus::Building, "Building"),
        (MasteryStatus::FewTries, "Few tries"),
    ];
    let mastered = with_status(&letters, MasteryStatus::Mastered).len();
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Letters,
                title: "Letter accuracy",
                note: "How often you copy each character right, which way it is going, and how fast you answer",
            }
            div { class: "err-summary",
                for (status , label) in groups {
                    {
                        let characters = with_status(&letters, status);
                        rsx! {
                            if !characters.is_empty() {
                                div { class: "err-sum-row",
                                    span { class: "err-sum-label {status_class(status)}", "{label} · {characters.len()}" }
                                    span { class: "err-chips",
                                        for character in characters.iter() {
                                            span { class: "err-chip {status_class(status)} group-text", "{character}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "err-sum-row",
                    span { class: "err-sum-label solid", "Mastered · {mastered}" }
                }
            }
            div { class: "err-legend", aria_hidden: "true",
                span { i { class: "err-swatch solid" } "mastered, {MASTERED_MIN_ACCURACY * 100.0:.0}% or more" }
                span { i { class: "err-swatch building" } "building" }
                span { i { class: "err-swatch to-work-on" } "weak" }
                span { i { class: "err-swatch new" } "few tries" }
                span { i { class: "err-swatch band" } "where it likely is" }
                span {
                    b { class: "acc-trend up", "↑ 8 " }
                    "points better lately than before"
                }
            }
            if contest {
                p { class: "muted", style: "margin: 0;", "{CONTEST_TIMES_NOTE}" }
            }
            div { class: "err-rows", role: "list",
                for letter in letters.iter() {
                    {row_view(letter, bottom)}
                }
            }
            div { class: "err-axis letters", aria_hidden: "true",
                span {}
                span { class: "err-ticks",
                    for tick in ticks.iter() {
                        span { style: "left: {along(*tick, bottom):.1}%;", "{tick * 100.0:.0}%" }
                    }
                }
                span { class: "err-next", "trend" }
                span { class: "err-next", "time" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(correct: u32, total: u32) -> CharacterDiagnostic {
        let accuracy = f64::from(correct) / f64::from(total.max(1));
        CharacterDiagnostic {
            letter: 'P',
            accuracy_pct: accuracy * 100.0,
            avg_ms: 650.0,
            total,
            correct,
            status: if total < 5 {
                MasteryStatus::FewTries
            } else if accuracy >= MASTERED_MIN_ACCURACY {
                MasteryStatus::Mastered
            } else {
                MasteryStatus::Weak
            },
            is_slow: false,
            recent_pct: None,
            earlier_pct: None,
        }
    }

    #[test]
    fn a_row_reads_out_what_it_shows() {
        let mut p = letter(1, 2);
        assert_eq!(p.status, MasteryStatus::FewTries);
        assert_eq!(
            letter_label(&p),
            "P: 50% right of 2, likely 20% to 80%, few tries, answered in 0.7 s"
        );
        p = letter(27, 30);
        p.recent_pct = Some(95.0);
        p.earlier_pct = Some(80.0);
        p.is_slow = true;
        assert!(
            letter_label(&p).ends_with("mastered, up 15 points lately, answered in 0.7 s, slow"),
            "{}",
            letter_label(&p)
        );
    }

    /// A trend needs five points to be one; less is steady, and without
    /// both windows there is nothing to say.
    #[test]
    fn a_trend_is_five_points_or_more() {
        let mut p = letter(27, 30);
        assert_eq!(trend(&p), Trend::Unknown);
        p.recent_pct = Some(92.0);
        p.earlier_pct = Some(90.0);
        assert_eq!(trend(&p), Trend::Steady);
        p.earlier_pct = Some(80.0);
        assert_eq!(trend(&p), Trend::Up(12.0));
        p.earlier_pct = Some(99.0);
        assert_eq!(trend(&p), Trend::Down(7.0));
    }

    /// The scale starts at half right, or at the ten below the worst
    /// character when it is worse.
    #[test]
    fn the_scale_takes_in_the_worst_character() {
        assert_eq!(scale_bottom(&[letter(9, 10), letter(6, 10)]), 0.5);
        assert_eq!(scale_bottom(&[letter(3, 10)]), 0.3);
        assert_eq!(along(0.9, 0.5), 80.0);
        assert_eq!(along(0.2, 0.5), 0.0);
    }
}
