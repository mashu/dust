use cw_core::{AutoLevelChange, LevelAxis, MixedAutoLevelAxis};

use super::counted;

/// A percentage as the level's messages give it: whole, or to a tenth where
/// it has one.
///
/// `value` is in percent already, 87.5 for "87.5%": not the share from 0 to
/// 1 that [`super::percent`] is handed and gives in whole percent.
pub fn percent_to_tenth(value: f64) -> String {
    let tenth = (value * 10.0).round() / 10.0;
    if tenth.fract() == 0.0 {
        format!("{tenth:.0}%")
    } else {
        format!("{tenth:.1}%")
    }
}

/// An accuracy beside the `threshold` it was held to, in whole percent, a
/// half going to the even one as every share on screen does, unless
/// rounding would carry it across the threshold: then to the tenth, cut
/// towards its own side. A session at 89.6% read "accuracy 90%, threshold
/// 90%" as it went down a level.
fn percent_beside(value: f64, threshold: f64) -> String {
    let whole = value.round_ties_even();
    match (value >= threshold, whole >= threshold) {
        (false, true) => format!("{:.1}%", (value * 10.0).floor() / 10.0),
        (true, false) => format!("{:.1}%", (value * 10.0).ceil() / 10.0),
        _ => format!("{whole:.0}%"),
    }
}

/// Whether a character and effective speed are spaced out, Farnsworth, as
/// far as whole WPM show it.
pub fn spaced_out((char_wpm, effective_wpm): (f64, f64)) -> bool {
    char_wpm - effective_wpm >= 0.5
}

/// A speed as the Speed settings put it: "30 WPM", or under Farnsworth
/// "20/15 WPM", the Character speed and then the Effective speed, as a
/// session's speed chip shows it.
pub fn wpm(speed: (f64, f64)) -> String {
    let (char_wpm, effective_wpm) = speed;
    if spaced_out(speed) {
        format!("{char_wpm:.0}/{effective_wpm:.0} WPM")
    } else {
        format!("{char_wpm:.0} WPM")
    }
}

/// What to tell the user when a session moved their level.
pub fn change_message(change: &AutoLevelChange) -> String {
    let verb = if change.raised() {
        "increased"
    } else {
        "decreased"
    };
    let at = change
        .at_wpm
        .map(|speed| format!(" at {}", wpm(speed)))
        .unwrap_or_default();
    let evidence = format!(
        "accuracy {}{at}, threshold {}, {} {}",
        percent_beside(change.accuracy_pct, change.threshold_pct),
        percent_to_tenth(change.threshold_pct),
        counted(change.sessions, "session"),
        if change.raised() { "above" } else { "below" },
    );
    let Some(next) = change.next_mixed_axis else {
        let label = if change.axis == LevelAxis::Callsign {
            "Callsign tier"
        } else {
            "Level"
        };
        return format!("{label} {verb} to {} ({evidence})", change.to);
    };
    let (axis, level) = if change.axis == LevelAxis::Digits {
        ("digit", format!("digits {}", change.to))
    } else {
        ("letter", format!("alphabet {}", change.to))
    };
    let next = match next {
        MixedAutoLevelAxis::Letters => "letter",
        MixedAutoLevelAxis::Digits => "digit",
    };
    format!("Mixed {axis} level {verb} — {level} (next: {next}, {evidence})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::{CharSetMode, CountersKey, SessionResult, TrainingSettings, evaluate_auto_level};

    fn change(settings: &TrainingSettings, accuracy: f64) -> AutoLevelChange {
        let result = SessionResult {
            accuracy,
            groups: vec![Default::default(); 20],
            ..crate::testing::stored_session("2026-09-01", &[("KM", "KM")])
        };
        evaluate_auto_level(&result, settings, &mut Default::default()).expect("a change")
    }

    /// A share reads whole unless rounding would carry it onto or across
    /// its threshold.
    #[test]
    fn a_share_beside_its_threshold_stays_on_its_own_side() {
        assert_eq!(percent_beside(95.0, 90.0), "95%");
        assert_eq!(percent_beside(89.6, 90.0), "89.6%");
        assert_eq!(percent_beside(89.99, 90.0), "89.9%");
        assert_eq!(percent_beside(90.0, 90.0), "90%");
        assert_eq!(percent_beside(87.4, 87.5), "87%");
        assert_eq!(percent_beside(87.5, 87.5), "88%");
        assert_eq!(percent_beside(90.2, 90.4), "90%");
        assert_eq!(percent_beside(90.45, 90.45), "90.5%");
        assert_eq!(percent_beside(40.0, 90.0), "40%");
        assert_eq!(percent_to_tenth(87.5), "87.5%");
    }

    /// Read whole, a share half way between two percents goes to the even
    /// one, as every share on screen does, and beside its threshold it
    /// still keeps to its own side.
    #[test]
    fn a_half_beside_a_threshold_goes_to_the_even_percent() {
        assert_eq!(percent_beside(86.5, 90.0), "86%");
        assert_eq!(percent_beside(90.5, 80.0), "90%");
        assert_eq!(percent_beside(88.5, 89.0), "88%");
        assert_eq!(percent_beside(89.5, 90.0), "89.5%");
    }

    #[test]
    fn the_message_names_the_level_and_the_evidence() {
        let mut koch = TrainingSettings::default();
        koch.curriculum.char_set_mode = CharSetMode::Koch;
        koch.curriculum.level = 4;
        koch.auto_level.auto_adjust_above_threshold_count = 1;
        assert_eq!(
            change_message(&change(&koch, 0.95)),
            "Level increased to 5 (accuracy 95%, threshold 90%, 1 session above)"
        );
        // Short of the threshold by less than rounding hides: to the tenth.
        let mut short = change(&koch, 0.2);
        (short.accuracy_pct, short.sessions) = (89.96, 2);
        assert_eq!(
            change_message(&short),
            "Level decreased to 3 (accuracy 89.9%, threshold 90%, 2 sessions below)"
        );
        short.accuracy_pct = 89.4;
        assert!(change_message(&short).contains("(accuracy 89%, threshold 90%"));
        // Sent at the top of an adaptive speed range, it says so.
        let mut fast = change(&koch, 0.95);
        fast.at_wpm = Some((30.0, 30.0));
        assert!(
            change_message(&fast).contains("(accuracy 95% at 30 WPM, threshold 90%"),
            "{}",
            change_message(&fast)
        );
        // Under Farnsworth, as the Character and Effective speed it was
        // sent at, not as the copy speed they make together, 17 WPM, which
        // no setting shows.
        fast.at_wpm = Some((20.0, 15.0));
        assert!(
            change_message(&fast).contains("(accuracy 95% at 20/15 WPM, threshold 90%"),
            "{}",
            change_message(&fast)
        );

        let mut callsigns = koch.clone();
        callsigns.curriculum.char_set_mode = CharSetMode::Callsign;
        callsigns.curriculum.callsign_level = 3;
        assert!(
            change_message(&change(&callsigns, 0.2)).starts_with("Callsign tier decreased to 2")
        );

        let mut mixed = koch;
        mixed.curriculum.char_set_mode = CharSetMode::Mixed;
        let message = change_message(&change(&mixed, 1.0));
        assert!(
            message.starts_with("Mixed letter level increased — alphabet 5 (next: digit"),
            "{message}"
        );
        assert_ne!(
            CountersKey::of(&mixed),
            change(&mixed, 1.0).counters_cleared[1]
        );
    }
}
