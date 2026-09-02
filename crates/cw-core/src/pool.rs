//! Character pool selection from a leveled alphabet (Koch, digits, mixed, custom).

use crate::level::{max_level_for_len, unlocked_prefix, LEVEL_MIN};
use crate::morse::{is_digit, DIGITS};
use crate::settings::{CharSetMode, PracticeWindow, TrainingSettings};

pub fn compute_char_pool(settings: &TrainingSettings) -> Vec<char> {
    match settings.char_set_mode {
        CharSetMode::Mixed => mixed_pool(settings),
        CharSetMode::Digits => leveled_pool(&settings.active_alphabet(), settings.active_level(), settings),
        CharSetMode::Custom | CharSetMode::Koch => {
            leveled_pool(&settings.progress_alphabet(), settings.level, settings)
        }
    }
}

pub fn unlocked_practice_count(settings: &TrainingSettings) -> usize {
    match settings.char_set_mode {
        CharSetMode::Digits => unlocked_prefix(DIGITS, settings.digits_level).len(),
        CharSetMode::Mixed => {
            let pct = settings.mixed_letters_percent.min(100);
            let letters = if pct == 0 {
                0
            } else {
                unlocked_prefix(&settings.progress_alphabet(), settings.level).len()
            };
            let digits = if pct == 100 {
                0
            } else {
                unlocked_prefix(DIGITS, settings.digits_level).len()
            };
            letters.max(digits)
        }
        _ => unlocked_prefix(&settings.progress_alphabet(), settings.level).len(),
    }
}

fn apply_window_range(settings: &mut TrainingSettings, window: PracticeWindow) {
    let n = unlocked_practice_count(settings).max(1) as u32;
    match window {
        PracticeWindow::All => {
            settings.sliding_window_start = 1;
            settings.sliding_window_end = n;
        }
        PracticeWindow::Last3 => {
            settings.sliding_window_start = n.saturating_sub(2).max(1);
            settings.sliding_window_end = n;
        }
        PracticeWindow::Last5 => {
            settings.sliding_window_start = n.saturating_sub(4).max(1);
            settings.sliding_window_end = n;
        }
    }
}

/// Remember the named window and retarget start/end to the current alphabet.
pub fn apply_practice_window(settings: &mut TrainingSettings, window: PracticeWindow) {
    settings.practice_window = Some(window);
    apply_window_range(settings, resolved_practice_window(settings));
}

fn infer_practice_window(settings: &TrainingSettings) -> Option<PracticeWindow> {
    let n = unlocked_practice_count(settings).max(1) as u32;
    let start = settings.sliding_window_start.max(1).min(n);
    let end = settings.sliding_window_end.max(1).min(n);
    let start_idx = start.min(end);
    let end_idx = start.max(end);
    if start_idx == 1 && end_idx >= n {
        Some(PracticeWindow::All)
    } else if n >= 3 && start_idx == n.saturating_sub(2) && end_idx == n {
        Some(PracticeWindow::Last3)
    } else if n >= 5 && start_idx == n.saturating_sub(4) && end_idx == n {
        Some(PracticeWindow::Last5)
    } else {
        None
    }
}

fn named_practice_window(settings: &TrainingSettings) -> PracticeWindow {
    settings
        .practice_window
        .or_else(|| infer_practice_window(settings))
        .unwrap_or(PracticeWindow::All)
}

/// Window actually used for the current unlocked count. Last5 stays Last5 in
/// settings even while fewer than five characters are unlocked.
pub fn resolved_practice_window(settings: &TrainingSettings) -> PracticeWindow {
    let n = unlocked_practice_count(settings);
    match named_practice_window(settings) {
        PracticeWindow::Last5 if n >= 5 => PracticeWindow::Last5,
        PracticeWindow::Last3 if n >= 3 => PracticeWindow::Last3,
        _ => PracticeWindow::All,
    }
}

/// Cap levels to the current alphabets and retarget All/Last3/Last5.
pub fn fit_settings_to_alphabet(settings: &mut TrainingSettings) {
    settings.custom_sequence = TrainingSettings::unique_alphabet(&settings.custom_sequence);
    settings.custom_set = TrainingSettings::unique_alphabet(&settings.custom_set);
    settings.level = settings
        .level
        .clamp(LEVEL_MIN, settings.max_letter_level());
    settings.digits_level = settings.digits_level.clamp(
        LEVEL_MIN,
        max_level_for_len(DIGITS.len()),
    );
    if settings.practice_window.is_none() {
        settings.practice_window = Some(named_practice_window(settings));
    }
    apply_window_range(settings, resolved_practice_window(settings));
}

pub fn current_practice_window(settings: &TrainingSettings) -> Option<PracticeWindow> {
    Some(resolved_practice_window(settings))
}

fn window_for_len(n: usize, named: PracticeWindow) -> PracticeWindow {
    match named {
        PracticeWindow::Last5 if n >= 5 => PracticeWindow::Last5,
        PracticeWindow::Last3 if n >= 3 => PracticeWindow::Last3,
        _ => PracticeWindow::All,
    }
}

/// Slice an unlocked prefix by All/Last3/Last5 using that slice's own length.
fn window_unlocked(unlocked: &[char], named: PracticeWindow) -> Vec<char> {
    let n = unlocked.len();
    if n == 0 {
        return Vec::new();
    }
    let (start, end) = match window_for_len(n, named) {
        PracticeWindow::All => (1, n),
        PracticeWindow::Last3 => (n.saturating_sub(2).max(1), n),
        PracticeWindow::Last5 => (n.saturating_sub(4).max(1), n),
    };
    let pool = unlocked[start.saturating_sub(1)..end].to_vec();
    if pool.len() >= 2 {
        pool
    } else {
        unlocked[n.saturating_sub(2)..].to_vec()
    }
}

fn mixed_pool(settings: &TrainingSettings) -> Vec<char> {
    let named = named_practice_window(settings);
    let pct = settings.mixed_letters_percent.min(100);
    let letters = if pct == 0 {
        Vec::new()
    } else {
        window_unlocked(
            &unlocked_prefix(&settings.progress_alphabet(), settings.level),
            named,
        )
    };
    let digits = if pct == 100 {
        Vec::new()
    } else {
        window_unlocked(&unlocked_prefix(DIGITS, settings.digits_level), named)
    };
    let mut union = letters.clone();
    for d in digits {
        if !union.contains(&d) {
            union.push(d);
        }
    }
    if union.len() >= 2 {
        union
    } else if letters.len() >= 2 {
        letters
    } else {
        union
    }
}

fn leveled_pool(alphabet: &[char], level: u32, settings: &TrainingSettings) -> Vec<char> {
    window_unlocked(
        &unlocked_prefix(alphabet, level),
        named_practice_window(settings),
    )
}

pub fn letters_subset(pool: &[char]) -> Vec<char> {
    pool.iter().copied().filter(|c| !is_digit(*c)).collect()
}

pub fn digits_subset(pool: &[char]) -> Vec<char> {
    pool.iter().copied().filter(|c| is_digit(*c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn koch_level_1_is_k_and_m() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Koch;
        s.level = 1;
        s.sliding_window_start = 1;
        s.sliding_window_end = 41;
        let pool = compute_char_pool(&s);
        assert_eq!(pool, vec!['K', 'M']);
    }

    #[test]
    fn digits_level_1_is_0_and_1() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Digits;
        s.digits_level = 1;
        s.sliding_window_start = 1;
        s.sliding_window_end = 10;
        let pool = compute_char_pool(&s);
        assert_eq!(pool, vec!['0', '1']);
    }

    #[test]
    fn mixed_unions_letters_and_digits() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Mixed;
        s.level = 1;
        s.digits_level = 1;
        let pool = compute_char_pool(&s);
        assert!(pool.contains(&'K'));
        assert!(pool.contains(&'M'));
        assert!(pool.contains(&'0'));
        assert!(pool.contains(&'1'));
    }

    #[test]
    fn last3_window_keeps_newest_letters() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Koch;
        s.level = 10;
        apply_practice_window(&mut s, PracticeWindow::Last3);
        let pool = compute_char_pool(&s);
        assert_eq!(pool.len(), 3);
        assert_eq!(current_practice_window(&s), Some(PracticeWindow::Last3));
    }

    #[test]
    fn last3_follows_level_from_named_window() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Koch;
        s.level = 10;
        apply_practice_window(&mut s, PracticeWindow::Last3);
        s.level = 11;
        let newest = s.progress_alphabet()[crate::level::unlocked_count_for_level(11) - 1];
        let pool = compute_char_pool(&s);
        assert!(pool.contains(&newest));
        assert_eq!(pool.len(), 3);
        assert_eq!(current_practice_window(&s), Some(PracticeWindow::Last3));
    }

    #[test]
    fn empty_custom_falls_back_to_koch() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Custom;
        s.custom_set.clear();
        let pool = compute_char_pool(&s);
        assert!(pool.contains(&'K'));
        assert!(pool.contains(&'M'));
    }

    #[test]
    fn custom_level_unlocks_alphabet_prefix() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Custom;
        s.custom_set = vec!['Q', 'R', 'S', 'T', 'U'];
        s.level = 1;
        s.sliding_window_start = 1;
        s.sliding_window_end = 40;
        assert_eq!(compute_char_pool(&s), vec!['Q', 'R']);
        s.level = 3;
        assert_eq!(compute_char_pool(&s), vec!['Q', 'R', 'S', 'T']);
    }

    #[test]
    fn digits_max_level_unlocks_all_ten() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Digits;
        s.digits_level = max_level_for_len(DIGITS.len());
        s.sliding_window_start = 1;
        s.sliding_window_end = 10;
        let pool = compute_char_pool(&s);
        assert_eq!(pool.len(), 10);
        s.digits_level = 10;
        let clamped = s.clone().clamp();
        assert_eq!(clamped.digits_level, 9);
        assert_eq!(compute_char_pool(&clamped).len(), 10);
    }

    #[test]
    fn switching_mode_resets_stale_window() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Koch;
        s.level = 10;
        apply_practice_window(&mut s, PracticeWindow::Last3);
        s.char_set_mode = CharSetMode::Digits;
        s.digits_level = 1;
        fit_settings_to_alphabet(&mut s);
        let pool = compute_char_pool(&s);
        assert_eq!(pool, vec!['0', '1']);
        assert_eq!(current_practice_window(&s), Some(PracticeWindow::All));
    }

    #[test]
    fn last5_survives_level_up_from_five_chars() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Koch;
        s.level = 4;
        apply_practice_window(&mut s, PracticeWindow::Last5);
        assert_eq!(unlocked_practice_count(&s), 5);
        assert_eq!(current_practice_window(&s), Some(PracticeWindow::Last5));
        s.level = 5;
        fit_settings_to_alphabet(&mut s);
        assert_eq!(current_practice_window(&s), Some(PracticeWindow::Last5));
        assert_eq!(compute_char_pool(&s).len(), 5);
        let newest = s.progress_alphabet()[crate::level::unlocked_count_for_level(5) - 1];
        assert!(compute_char_pool(&s).contains(&newest));
    }

    #[test]
    fn last3_retargets_after_level_change() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Koch;
        s.level = 10;
        apply_practice_window(&mut s, PracticeWindow::Last3);
        s.level = 11;
        fit_settings_to_alphabet(&mut s);
        assert_eq!(current_practice_window(&s), Some(PracticeWindow::Last3));
        let newest = s.progress_alphabet()[crate::level::unlocked_count_for_level(11) - 1];
        assert!(compute_char_pool(&s).contains(&newest));
    }

    #[test]
    fn mixed_letter_pool_excludes_sequence_digits() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Koch;
        s.level = s.max_letter_level();
        apply_practice_window(&mut s, PracticeWindow::All);
        assert!(compute_char_pool(&s).contains(&'5'));
        s.char_set_mode = CharSetMode::Mixed;
        s.digits_level = 1;
        fit_settings_to_alphabet(&mut s);
        let pool = compute_char_pool(&s);
        assert!(!pool.contains(&'5'));
        assert!(pool.contains(&'0'));
        assert!(pool.contains(&'1'));
        assert!(pool.iter().filter(|c| c.is_ascii_digit()).all(|c| *c == '0' || *c == '1'));
    }

    #[test]
    fn unknown_custom_chars_are_dropped() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Custom;
        s.custom_set = vec!['A', '*', 'B'];
        s.level = 1;
        apply_practice_window(&mut s, PracticeWindow::All);
        assert_eq!(compute_char_pool(&s), vec!['A', 'B']);
    }

    #[test]
    fn mixed_last3_windows_letters_and_digits_separately() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Mixed;
        s.level = 10;
        s.digits_level = max_level_for_len(DIGITS.len());
        apply_practice_window(&mut s, PracticeWindow::Last3);
        let pool = compute_char_pool(&s);
        let letters: Vec<char> = pool.iter().copied().filter(|c| !c.is_ascii_digit()).collect();
        let digits: Vec<char> = pool.iter().copied().filter(|c| c.is_ascii_digit()).collect();
        assert_eq!(letters.len(), 3);
        assert_eq!(digits, vec!['7', '8', '9']);
    }

    #[test]
    fn mixed_100_percent_omits_digits_from_pool() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Mixed;
        s.level = 5;
        s.digits_level = max_level_for_len(DIGITS.len());
        s.mixed_letters_percent = 100;
        apply_practice_window(&mut s, PracticeWindow::All);
        let pool = compute_char_pool(&s);
        assert!(!pool.is_empty());
        assert!(pool.iter().all(|c| !c.is_ascii_digit()));
    }

    #[test]
    fn mixed_0_percent_omits_letters_from_pool() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Mixed;
        s.level = 10;
        s.digits_level = 3;
        s.mixed_letters_percent = 0;
        apply_practice_window(&mut s, PracticeWindow::All);
        let pool = compute_char_pool(&s);
        assert!(!pool.is_empty());
        assert!(pool.iter().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn mixed_0_percent_last3_follows_digit_count() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Mixed;
        s.level = 1;
        s.digits_level = max_level_for_len(DIGITS.len());
        s.mixed_letters_percent = 0;
        apply_practice_window(&mut s, PracticeWindow::Last3);
        assert!(unlocked_practice_count(&s) >= 3);
        assert_eq!(current_practice_window(&s), Some(PracticeWindow::Last3));
        let pool = compute_char_pool(&s);
        assert_eq!(pool, vec!['7', '8', '9']);
    }
}
