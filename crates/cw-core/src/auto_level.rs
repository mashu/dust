//! Automatic level adjustment after a session.

use crate::level::LEVEL_MIN;
use crate::morse::{
    MixedAutoLevelAxis, MAX_DIGITS_LEVEL, MIN_DIGITS_LEVEL,
};
use crate::settings::{CharSetMode, TrainingSettings};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoAdjustMode {
    Alphabet,
    Digits,
    Mixed,
}

impl AutoAdjustMode {
    pub fn from_char_set(mode: CharSetMode) -> Self {
        match mode {
            CharSetMode::Digits => Self::Digits,
            CharSetMode::Mixed => Self::Mixed,
            CharSetMode::Koch | CharSetMode::Custom => Self::Alphabet,
        }
    }

    pub fn level_for(self, settings: &TrainingSettings) -> u32 {
        match self {
            Self::Digits => settings.digits_level,
            _ => settings.level,
        }
    }

    pub fn digits_for(self, settings: &TrainingSettings) -> Option<u32> {
        matches!(self, Self::Mixed).then_some(settings.digits_level)
    }

    pub fn storage_key_for(self, settings: &TrainingSettings) -> String {
        self.storage_key_at(settings, self.level_for(settings), self.digits_for(settings))
    }

    pub fn storage_key_at(
        self,
        settings: &TrainingSettings,
        level: u32,
        digits_level: Option<u32>,
    ) -> String {
        let fingerprint = settings.alphabet_fingerprint();
        match self {
            Self::Mixed => {
                format!("mixed_{level}_{}_{fingerprint}", digits_level.unwrap_or(0))
            }
            Self::Digits => format!("digits_{level}_{fingerprint}"),
            Self::Alphabet => match settings.char_set_mode {
                CharSetMode::Custom => format!("custom_{level}_{fingerprint}"),
                _ => format!("koch_{level}_{fingerprint}"),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutoLevelCounters {
    pub above: u32,
    pub below: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AutoLevelResult {
    pub delta: i32,
    pub next_level: u32,
    pub next_digits_level: Option<u32>,
    pub adjusted_mixed_axis: Option<MixedAutoLevelAxis>,
    pub next_mixed_auto_level_axis: Option<MixedAutoLevelAxis>,
    pub message: String,
    pub counters_cleared_keys: Vec<String>,
}

fn apply_mixed_axis_delta(
    axis: MixedAutoLevelAxis,
    delta: i32,
    current_level: u32,
    current_digits: u32,
    max_letters: u32,
    max_digits: u32,
) -> Option<(u32, u32, MixedAutoLevelAxis)> {
    match axis {
        MixedAutoLevelAxis::Letters => {
            let candidate =
                (current_level as i32 + delta).clamp(LEVEL_MIN as i32, max_letters as i32) as u32;
            if candidate != current_level {
                Some((candidate, current_digits, MixedAutoLevelAxis::Letters))
            } else {
                None
            }
        }
        MixedAutoLevelAxis::Digits => {
            let candidate = (current_digits as i32 + delta)
                .clamp(MIN_DIGITS_LEVEL as i32, max_digits as i32)
                as u32;
            if candidate != current_digits {
                Some((current_level, candidate, MixedAutoLevelAxis::Digits))
            } else {
                None
            }
        }
    }
}

fn max_letter_level(settings: &TrainingSettings) -> u32 {
    settings.max_letter_level()
}

fn mixed_letters_active(settings: &TrainingSettings) -> bool {
    settings.mixed_letters_percent.min(100) > 0
}

fn mixed_digits_active(settings: &TrainingSettings) -> bool {
    settings.mixed_letters_percent.min(100) < 100
}

fn mixed_axis_active(settings: &TrainingSettings, axis: MixedAutoLevelAxis) -> bool {
    match axis {
        MixedAutoLevelAxis::Letters => mixed_letters_active(settings),
        MixedAutoLevelAxis::Digits => mixed_digits_active(settings),
    }
}

fn mixed_try_axis(
    settings: &TrainingSettings,
    axis: MixedAutoLevelAxis,
    delta: i32,
    current_level: u32,
    current_digits: u32,
) -> Option<(u32, u32, MixedAutoLevelAxis)> {
    if !mixed_axis_active(settings, axis) {
        return None;
    }
    apply_mixed_axis_delta(
        axis,
        delta,
        current_level,
        current_digits,
        max_letter_level(settings),
        MAX_DIGITS_LEVEL,
    )
}

/// After a mixed adjustment, flip only onto an axis that is actually practiced.
fn mixed_active_next_axis(
    settings: &TrainingSettings,
    adjusted_axis: MixedAutoLevelAxis,
) -> MixedAutoLevelAxis {
    let flipped = adjusted_axis.flip();
    if mixed_axis_active(settings, flipped) {
        flipped
    } else {
        adjusted_axis
    }
}

fn mixed_display_next_axis(settings: &TrainingSettings) -> MixedAutoLevelAxis {
    let stored = settings.mixed_auto_level_next_axis;
    if mixed_axis_active(settings, stored) {
        stored
    } else if mixed_axis_active(settings, stored.flip()) {
        stored.flip()
    } else {
        stored
    }
}

/// Evaluate whether the training level should change. Mutates `counters` for the current level.
/// Returns `None` when no level change is warranted.
pub fn evaluate_auto_level(
    accuracy_fraction: f64,
    settings: &TrainingSettings,
    counters: &mut AutoLevelCounters,
) -> Option<AutoLevelResult> {
    if !settings.auto_adjust_level {
        return None;
    }
    let mode = AutoAdjustMode::from_char_set(settings.char_set_mode);
    let threshold = settings.auto_adjust_threshold.clamp(0.0, 100.0);
    let accuracy_pct = accuracy_fraction * 100.0;
    if accuracy_pct >= threshold {
        counters.above += 1;
    } else {
        counters.below += 1;
    }

    let increase_enabled = settings.auto_adjust_above_threshold_count > 0;
    let decrease_enabled = settings.auto_adjust_below_threshold_count > 0;
    let should_increase =
        increase_enabled && counters.above >= settings.auto_adjust_above_threshold_count;
    let should_decrease =
        decrease_enabled && counters.below >= settings.auto_adjust_below_threshold_count;

    let delta = if should_increase && should_decrease {
        if counters.above >= counters.below {
            1
        } else {
            -1
        }
    } else if should_increase {
        1
    } else if should_decrease {
        -1
    } else {
        0
    };
    if delta == 0 {
        return None;
    }

    let current_level = match mode {
        AutoAdjustMode::Digits => settings.digits_level,
        _ => settings.level,
    };

    if mode != AutoAdjustMode::Mixed {
        let max_level = if mode == AutoAdjustMode::Digits {
            MAX_DIGITS_LEVEL
        } else {
            max_letter_level(settings)
        };
        let next_level =
            (current_level as i32 + delta).clamp(LEVEL_MIN as i32, max_level as i32) as u32;
        if next_level == current_level {
            if delta > 0 {
                counters.above = 0;
            } else {
                counters.below = 0;
            }
            return None;
        }
        let count_text = if delta > 0 {
            format!("{} sessions above", counters.above)
        } else {
            format!("{} sessions below", counters.below)
        };
        let label = "Level";
        let verb = if delta > 0 { "increased" } else { "decreased" };
        return Some(AutoLevelResult {
            delta,
            next_level,
            next_digits_level: None,
            adjusted_mixed_axis: None,
            next_mixed_auto_level_axis: None,
            message: format!(
                "{label} {verb} to {next_level} (accuracy {}%, threshold {threshold}%, {count_text})",
                accuracy_pct.round()
            ),
            counters_cleared_keys: vec![
                mode.storage_key_at(settings, current_level, None),
                mode.storage_key_at(settings, next_level, None),
            ],
        });
    }

    let current_digits = settings.digits_level.max(MIN_DIGITS_LEVEL);
    let primary = settings.mixed_auto_level_next_axis;
    let secondary = primary.flip();
    let mixed = mixed_try_axis(settings, primary, delta, current_level, current_digits)
        .or_else(|| mixed_try_axis(settings, secondary, delta, current_level, current_digits));
    let Some((next_level, next_digits_level, adjusted_axis)) = mixed else {
        if delta > 0 {
            counters.above = 0;
        } else {
            counters.below = 0;
        }
        return None;
    };
    let next_axis = mixed_active_next_axis(settings, adjusted_axis);
    let count_text = if delta > 0 {
        format!("{} sessions above", counters.above)
    } else {
        format!("{} sessions below", counters.below)
    };
    let verb = if delta > 0 { "increased" } else { "decreased" };
    let axis_label = match adjusted_axis {
        MixedAutoLevelAxis::Letters => "letter",
        MixedAutoLevelAxis::Digits => "digit",
    };
    let level_part = match adjusted_axis {
        MixedAutoLevelAxis::Letters => format!("alphabet {next_level}"),
        MixedAutoLevelAxis::Digits => format!("digits {next_digits_level}"),
    };
    let next_part = match next_axis {
        MixedAutoLevelAxis::Letters => "letter",
        MixedAutoLevelAxis::Digits => "digit",
    };
    Some(AutoLevelResult {
        delta,
        next_level,
        next_digits_level: Some(next_digits_level),
        adjusted_mixed_axis: Some(adjusted_axis),
        next_mixed_auto_level_axis: Some(next_axis),
        message: format!(
            "Mixed {axis_label} level {verb} — {level_part} (next: {next_part}, accuracy {}%, threshold {threshold}%, {count_text})",
            accuracy_pct.round()
        ),
        counters_cleared_keys: vec![
            mode.storage_key_at(settings, current_level, Some(current_digits)),
            mode.storage_key_at(settings, next_level, Some(next_digits_level)),
        ],
    })
}

pub fn apply_auto_level(settings: &mut TrainingSettings, result: &AutoLevelResult) {
    match AutoAdjustMode::from_char_set(settings.char_set_mode) {
        AutoAdjustMode::Digits => settings.digits_level = result.next_level,
        AutoAdjustMode::Alphabet => settings.level = result.next_level,
        AutoAdjustMode::Mixed => {
            settings.level = result.next_level;
            if let Some(d) = result.next_digits_level {
                settings.digits_level = d;
            }
            if let Some(axis) = result.next_mixed_auto_level_axis {
                settings.mixed_auto_level_next_axis = axis;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoLevelProgress {
    pub threshold: f64,
    pub above_count: u32,
    pub below_count: u32,
    pub above_target: u32,
    pub below_target: u32,
    pub above_disabled: bool,
    pub below_disabled: bool,
    pub alternating_mixed: bool,
    pub next_mixed_axis: Option<MixedAutoLevelAxis>,
}

pub fn auto_level_progress(
    settings: &TrainingSettings,
    counters: AutoLevelCounters,
) -> Option<AutoLevelProgress> {
    if !settings.auto_adjust_level {
        return None;
    }
    Some(AutoLevelProgress {
        threshold: settings.auto_adjust_threshold.clamp(0.0, 100.0),
        above_count: counters.above,
        below_count: counters.below,
        above_target: settings.auto_adjust_above_threshold_count,
        below_target: settings.auto_adjust_below_threshold_count,
        above_disabled: settings.auto_adjust_above_threshold_count == 0,
        below_disabled: settings.auto_adjust_below_threshold_count == 0,
        alternating_mixed: settings.char_set_mode == CharSetMode::Mixed,
        next_mixed_axis: (settings.char_set_mode == CharSetMode::Mixed)
            .then_some(mixed_display_next_axis(settings)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_above_increases_koch() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Koch;
        settings.level = 1;
        settings.auto_adjust_above_threshold_count = 5;
        let mut counters = AutoLevelCounters { above: 4, below: 0 };
        let result = evaluate_auto_level(1.0, &settings, &mut counters).expect("level up");
        assert_eq!(result.next_level, 2);
        assert_eq!(result.delta, 1);
    }

    #[test]
    fn below_threshold_decreases() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Koch;
        settings.level = 3;
        settings.auto_adjust_below_threshold_count = 1;
        let mut counters = AutoLevelCounters::default();
        let result = evaluate_auto_level(0.5, &settings, &mut counters).expect("level down");
        assert_eq!(result.next_level, 2);
        assert_eq!(result.delta, -1);
    }

    #[test]
    fn progress_none_when_disabled() {
        let mut settings = TrainingSettings::default();
        settings.auto_adjust_level = false;
        assert!(auto_level_progress(&settings, AutoLevelCounters::default()).is_none());
    }

    #[test]
    fn progress_reports_counters() {
        let settings = TrainingSettings::default();
        let progress = auto_level_progress(&settings, AutoLevelCounters { above: 2, below: 1 })
            .expect("enabled");
        assert_eq!(progress.above_count, 2);
        assert_eq!(progress.below_count, 1);
        assert!(progress.alternating_mixed);
    }

    #[test]
    fn blocked_increase_at_max_resets_above_counter() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Koch;
        settings.custom_sequence = vec!['K', 'M'];
        settings.level = 1;
        settings.auto_adjust_above_threshold_count = 1;
        let mut counters = AutoLevelCounters::default();
        assert!(evaluate_auto_level(1.0, &settings, &mut counters).is_none());
        assert_eq!(counters.above, 0);
        assert_eq!(counters.below, 0);
    }

    #[test]
    fn custom_mode_increases_level_within_alphabet() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Custom;
        settings.custom_set = vec!['Q', 'R', 'S', 'T'];
        settings.level = 1;
        settings.auto_adjust_above_threshold_count = 1;
        let mut counters = AutoLevelCounters::default();
        let result = evaluate_auto_level(1.0, &settings, &mut counters).expect("level up");
        assert_eq!(result.next_level, 2);
        apply_auto_level(&mut settings, &result);
        assert_eq!(settings.level, 2);
    }

    #[test]
    fn digits_mode_increases_level_within_alphabet() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Digits;
        settings.digits_level = 1;
        settings.auto_adjust_above_threshold_count = 1;
        let mut counters = AutoLevelCounters::default();
        let result = evaluate_auto_level(1.0, &settings, &mut counters).expect("level up");
        assert_eq!(result.next_level, 2);
        apply_auto_level(&mut settings, &result);
        assert_eq!(settings.digits_level, 2);
    }

    #[test]
    fn storage_keys_differ_for_different_sequences() {
        let mut lcwo = TrainingSettings::default();
        lcwo.char_set_mode = CharSetMode::Koch;
        lcwo.custom_sequence.clear();
        let mut mania = lcwo.clone();
        mania.custom_sequence = crate::sequences::TRADITIONAL_KOCH_SEQUENCE.to_vec();
        let mode = AutoAdjustMode::Alphabet;
        assert_ne!(
            mode.storage_key_for(&lcwo),
            mode.storage_key_for(&mania)
        );
        let mut custom_a = TrainingSettings::default();
        custom_a.char_set_mode = CharSetMode::Custom;
        custom_a.custom_set = vec!['Q', 'R', 'S'];
        let mut custom_b = custom_a.clone();
        custom_b.custom_set = vec!['X', 'Y', 'Z'];
        assert_ne!(
            mode.storage_key_for(&custom_a),
            mode.storage_key_for(&custom_b)
        );
        assert_eq!(
            mode.storage_key_at(&lcwo, 5, None),
            format!("koch_5_{}", lcwo.alphabet_fingerprint())
        );
    }

    #[test]
    fn mixed_100_percent_does_not_adjust_digits() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Mixed;
        settings.mixed_letters_percent = 100;
        settings.custom_sequence = vec!['K', 'M'];
        settings.level = 1;
        settings.digits_level = 1;
        settings.mixed_auto_level_next_axis = MixedAutoLevelAxis::Letters;
        settings.auto_adjust_above_threshold_count = 1;
        let mut counters = AutoLevelCounters::default();
        assert!(evaluate_auto_level(1.0, &settings, &mut counters).is_none());
        assert_eq!(counters.above, 0);
        assert_eq!(settings.digits_level, 1);
    }

    #[test]
    fn mixed_0_percent_skips_letter_axis() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Mixed;
        settings.mixed_letters_percent = 0;
        settings.level = 1;
        settings.digits_level = 1;
        settings.mixed_auto_level_next_axis = MixedAutoLevelAxis::Letters;
        settings.auto_adjust_above_threshold_count = 1;
        let mut counters = AutoLevelCounters::default();
        let result = evaluate_auto_level(1.0, &settings, &mut counters).expect("digit up");
        assert_eq!(result.next_level, 1);
        assert_eq!(result.next_digits_level, Some(2));
        assert_eq!(result.adjusted_mixed_axis, Some(MixedAutoLevelAxis::Digits));
        assert_eq!(
            result.next_mixed_auto_level_axis,
            Some(MixedAutoLevelAxis::Digits)
        );
    }

    #[test]
    fn mixed_100_percent_keeps_letter_next_axis() {
        let mut settings = TrainingSettings::default();
        settings.char_set_mode = CharSetMode::Mixed;
        settings.mixed_letters_percent = 100;
        settings.level = 1;
        settings.digits_level = 1;
        settings.mixed_auto_level_next_axis = MixedAutoLevelAxis::Letters;
        settings.auto_adjust_above_threshold_count = 1;
        let mut counters = AutoLevelCounters::default();
        let result = evaluate_auto_level(1.0, &settings, &mut counters).expect("letter up");
        assert_eq!(result.next_level, 2);
        assert_eq!(result.adjusted_mixed_axis, Some(MixedAutoLevelAxis::Letters));
        assert_eq!(
            result.next_mixed_auto_level_axis,
            Some(MixedAutoLevelAxis::Letters)
        );
    }
}
