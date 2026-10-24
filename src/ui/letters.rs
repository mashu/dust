//! The Letter accuracy card: how often you copy each character right, how
//! sure that figure is, which way it is going, and how fast you recognise
//! it, the ones that need the most work first.
//!
//! A row is a bar, drawn as "What comes up next" draws its own: as long as
//! the share copied right, in the colour of where the character stands,
//! with a notch at the Mastered line and a paler stretch past the bar to
//! the top of where the accuracy likely is. The line under it says the
//! rest, how fast it is recognised among it, in the colour of its band:
//! green fast, yellow ICR, orange slow, red too slow, as Settings →
//! Recognition sets them. A summary above names the weak, the building and
//! the ones with too few tries to tell, and those recognised at once (ICR);
//! a character tried once is no longer called weak.

use cw_core::{
    AutoLevelSettings, CONTEST_TIMES_NOTE, CharSetMode, CharacterDiagnostic, MASTERED_MIN_ACCURACY,
    MasteryStatus, RECOGNITION_MIN_TRIES, RecognitionBand, SessionResult, TrainingSettings,
    character_diagnostics,
};
use dioxus::prelude::*;

use crate::ui::bigram::id_part;
use crate::ui::help;
use crate::ui::widgets::{CardHead, IconName, control_id};

/// The share of the belief the likely range holds.
const LIKELY: f64 = 0.8;
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

/// A recognition time, in milliseconds to the nearest ten: "320 ms".
fn millis(ms: f64) -> String {
    format!("{:.0} ms", (ms / 10.0).round() * 10.0)
}

/// How fast a character is recognised, as far as it has been timed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Recognition {
    /// Timed enough to say: its time, and the band it falls in.
    Banded(f64, RecognitionBand),
    /// Timed this many times, too few to say for sure: the median so far.
    Provisional(f64, u32),
    /// Never timed.
    Untimed,
}

fn recognition(letter: &CharacterDiagnostic, bands: &AutoLevelSettings) -> Recognition {
    match (letter.recognition_band(bands), letter.recognition_ms) {
        (Some(band), Some(ms)) => Recognition::Banded(ms, band),
        (None, Some(ms)) => Recognition::Provisional(ms, letter.recognition_tries),
        _ => Recognition::Untimed,
    }
}

/// A band's class: its colour, green to red.
fn band_class(band: RecognitionBand) -> &'static str {
    match band {
        RecognitionBand::Fast => "rec-fast",
        RecognitionBand::Instant => "rec-icr",
        RecognitionBand::Slow => "rec-slow",
        RecognitionBand::TooSlow => "rec-too-slow",
    }
}

/// A band in a sentence.
fn band_word(band: RecognitionBand) -> &'static str {
    match band {
        RecognitionBand::Fast => "fast, ICR",
        RecognitionBand::Instant => "ICR",
        RecognitionBand::Slow => "slow",
        RecognitionBand::TooSlow => "too slow",
    }
}

/// The start of the line under a row's bar: where it stands, how often it
/// was tried, and where its accuracy likely is.
fn letter_note(letter: &CharacterDiagnostic) -> String {
    let (low, high) = letter.accuracy_interval(LIKELY);
    let word = status_word(letter.status);
    let mut status = word.chars();
    let status = status
        .next()
        .map(|first| first.to_uppercase().chain(status).collect::<String>())
        .unwrap_or_default();
    let tries = match letter.total {
        1 => "1 try".to_string(),
        total => format!("{total} tries"),
    };
    format!(
        "{status} · {tries} · likely {:.0}–{:.0}%",
        low * 100.0,
        high * 100.0
    )
}

/// What a screen reader hears for a row.
fn letter_label(letter: &CharacterDiagnostic, bands: &AutoLevelSettings) -> String {
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
    match recognition(letter, bands) {
        Recognition::Banded(ms, band) => text.push_str(&format!(
            ", recognised in {}, {}",
            millis(ms),
            band_word(band)
        )),
        Recognition::Provisional(ms, tries) => text.push_str(&format!(
            ", recognised in about {} so far, {tries} of {RECOGNITION_MIN_TRIES} tries timed",
            millis(ms)
        )),
        Recognition::Untimed => text.push_str(", not timed yet"),
    }
    text
}

/// A share as a place along the bar, in percent.
fn along(share: f64) -> f64 {
    (share * 100.0).clamp(0.0, 100.0)
}

/// One character's row; `timing` is whether any character has been timed,
/// so that one never timed says so.
fn row_view(letter: &CharacterDiagnostic, bands: &AutoLevelSettings, timing: bool) -> Element {
    let (low, high) = letter.accuracy_interval(LIKELY);
    let (low, high) = (along(low), along(high));
    rsx! {
        div {
            id: control_id("letter", &id_part(letter.letter)),
            class: "letter-row stat-row {status_class(letter.status)}",
            role: "listitem",
            aria_label: "{letter_label(letter, bands)}",
            span { class: "letter-key group-text", "{letter.letter}" }
            div { class: "bar-track marked",
                span {
                    class: "stat-range",
                    style: "left: {low:.1}%; width: {high - low:.1}%;",
                }
                span {
                    class: "bar-fill stat-fill",
                    style: "width: {letter.accuracy_pct.clamp(0.0, 100.0):.1}%;",
                }
                span {
                    class: "bar-mark",
                    style: "left: {along(MASTERED_MIN_ACCURACY):.1}%;",
                }
            }
            span { class: "mono muted stat-value", "{letter.accuracy_pct.round()}%" }
            span { class: "tiny",
                "{letter_note(letter)}"
                match trend(letter) {
                    Trend::Up(points) => rsx! {
                        " · "
                        b { class: "acc-trend up", "↑ {points:.0}" }
                        " lately"
                    },
                    Trend::Down(points) => rsx! {
                        " · "
                        b { class: "acc-trend down", "↓ {points:.0}" }
                        " lately"
                    },
                    Trend::Steady => rsx! { " · steady lately" },
                    Trend::Unknown => rsx! {},
                }
                match recognition(letter, bands) {
                    Recognition::Banded(ms, band) => rsx! {
                        " · "
                        b { class: "rec-chip {band_class(band)}",
                            if band.icr() {
                                "ICR {millis(ms)}"
                            } else {
                                "{millis(ms)}"
                            }
                        }
                    },
                    Recognition::Provisional(ms, tries) => rsx! { " · about {millis(ms)} so far, {tries} of {RECOGNITION_MIN_TRIES} tries timed" },
                    Recognition::Untimed if timing => rsx! { " · not timed yet" },
                    Recognition::Untimed => rsx! {},
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
pub fn LettersTab(settings: TrainingSettings, sessions: Vec<SessionResult>) -> Element {
    let bands = &settings.auto_level;
    let letters = character_diagnostics(&sessions);
    if letters.is_empty() {
        return rsx! {
            div { class: "card", p { class: "muted", "No letter data yet." } }
        };
    }
    let contest = sessions
        .iter()
        .any(|session| session.char_set_mode == CharSetMode::Contest);
    let groups = [
        (MasteryStatus::Weak, "Weak"),
        (MasteryStatus::Building, "Building"),
        (MasteryStatus::FewTries, "Few tries"),
    ];
    let mastered = with_status(&letters, MasteryStatus::Mastered).len();
    let line = MASTERED_MIN_ACCURACY * 100.0;
    // Recognised at once, of those timed enough to say, each with its band;
    // and whether any is timed enough, or at all.
    let instant: Vec<(char, RecognitionBand)> = letters
        .iter()
        .filter_map(|letter| Some((letter.letter, letter.recognition_band(bands)?)))
        .filter(|(_, band)| band.icr())
        .collect();
    let timed = letters
        .iter()
        .any(|letter| letter.recognition_band(bands).is_some());
    let timing = letters.iter().any(|letter| letter.recognition_tries > 0);
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Letters,
                title: "Letter accuracy",
                note: "How often you copy each character right, which way it is going, and how fast you recognise it",
                help: help::letters(bands),
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
                if timed {
                    div { class: "err-sum-row",
                        span { class: "err-sum-label solid", "Instant (ICR) · {instant.len()}" }
                        span { class: "err-chips",
                            for (character , band) in instant.iter() {
                                span { class: "err-chip {band_class(*band)} group-text", "{character}" }
                            }
                        }
                    }
                }
            }
            if !timing {
                p { class: "muted", style: "margin: 0; font-size: 0.85rem;",
                    "How fast you recognise each character is timed as you type along: with Lock typing while sending off (Settings → Session shape), or with groups of one character. Type each one as soon as you hear it."
                }
            }
            div { class: "err-legend", aria_hidden: "true",
                span { i { class: "err-swatch solid" } "mastered" }
                span { i { class: "err-swatch building" } "building" }
                span { i { class: "err-swatch to-work-on" } "weak" }
                span { i { class: "err-swatch finding-out" } "few tries" }
                span { i { class: "err-swatch range" } "where it likely is" }
                span { i { class: "err-swatch mark" } "mastered from {line:.0}%" }
            }
            if timing {
                div { class: "err-legend", aria_hidden: "true",
                    span { "Recognised in" }
                    span { i { class: "err-swatch rec-fast" } "under {bands.recognition_fast_ms} ms" }
                    span { i { class: "err-swatch rec-icr" } "ICR, under {bands.recognition_icr_ms}" }
                    span { i { class: "err-swatch rec-slow" } "under {bands.recognition_slow_ms}" }
                    span { i { class: "err-swatch rec-too-slow" } "{bands.recognition_slow_ms} and over" }
                }
            }
            if contest {
                p { class: "muted", style: "margin: 0;", "{CONTEST_TIMES_NOTE}" }
            }
            div { class: "stat-rows", role: "list",
                for letter in letters.iter() {
                    {row_view(letter, bands, timing)}
                }
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
            total,
            correct,
            status: if total < 5 {
                MasteryStatus::FewTries
            } else if accuracy >= MASTERED_MIN_ACCURACY {
                MasteryStatus::Mastered
            } else {
                MasteryStatus::Weak
            },
            recent_pct: None,
            earlier_pct: None,
            recognition_ms: None,
            recognition_tries: 0,
        }
    }

    #[test]
    fn a_row_reads_out_what_it_shows() {
        let bands = AutoLevelSettings::default();
        let label = |p: &CharacterDiagnostic| letter_label(p, &bands);
        let mut p = letter(1, 2);
        assert_eq!(p.status, MasteryStatus::FewTries);
        assert_eq!(
            label(&p),
            "P: 50% right of 2, likely 20% to 80%, few tries, not timed yet"
        );
        assert_eq!(letter_note(&p), "Few tries · 2 tries · likely 20–80%");
        p = letter(1, 1);
        assert!(
            letter_note(&p).contains(" · 1 try · "),
            "{}",
            letter_note(&p)
        );
        p = letter(27, 30);
        p.recent_pct = Some(95.0);
        p.earlier_pct = Some(80.0);
        p.recognition_tries = 3;
        p.recognition_ms = Some(452.0);
        assert!(
            label(&p).ends_with(
                "mastered, up 15 points lately, recognised in about 450 ms so far, 3 of 8 tries timed"
            ),
            "{}",
            label(&p)
        );
        assert!(letter_note(&p).starts_with("Mastered · 30 tries · likely "));
        // Timed enough, each in the band its time falls in.
        p.recognition_tries = 12;
        for (ms, band, said) in [
            (
                321.0,
                RecognitionBand::Fast,
                ", recognised in 320 ms, fast, ICR",
            ),
            (
                520.0,
                RecognitionBand::Instant,
                ", recognised in 520 ms, ICR",
            ),
            (640.0, RecognitionBand::Slow, ", recognised in 640 ms, slow"),
            (
                910.0,
                RecognitionBand::TooSlow,
                ", recognised in 910 ms, too slow",
            ),
        ] {
            p.recognition_ms = Some(ms);
            assert_eq!(recognition(&p, &bands), Recognition::Banded(ms, band));
            assert!(label(&p).ends_with(said), "{}", label(&p));
        }
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
}
