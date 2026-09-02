//! Character pool selection from a leveled alphabet (Koch, digits, mixed, custom).

use crate::level::{max_level_for_len, unlocked_prefix, LEVEL_MIN};
use crate::morse::{is_digit, DIGITS};
use crate::settings::{CharSetMode, TrainingSettings};

pub fn compute_char_pool(settings: &TrainingSettings) -> Vec<char> {
    match settings.char_set_mode {
        CharSetMode::Mixed => mixed_pool(settings),
        CharSetMode::Digits => leveled_pool(&settings.active_alphabet(), settings.active_level(), settings),
        CharSetMode::Custom | CharSetMode::Koch => {
            leveled_pool(&settings.progress_alphabet(), settings.level, settings)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PracticeWindow {
    All,
    Last3,
    Last5,
}

pub fn unlocked_practice_count(settings: &TrainingSettings) -> usize {
    match settings.char_set_mode {
        CharSetMode::Digits => unlocked_prefix(DIGITS, settings.digits_level).len(),
        _ => unlocked_prefix(&settings.progress_alphabet(), settings.level).len(),
    }
}

pub fn apply_practice_window(settings: &mut TrainingSettings, window: PracticeWindow) {
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

/// Cap levels to the current alphabets and keep All/Last3/Last5, or reset to All.
pub fn fit_settings_to_alphabet(settings: &mut TrainingSettings) {
    settings.level = settings
        .level
        .clamp(LEVEL_MIN, settings.max_letter_level());
    settings.digits_level = settings.digits_level.clamp(
        LEVEL_MIN,
        max_level_for_len(DIGITS.len()),
    );
    match current_practice_window(settings) {
        Some(window) => apply_practice_window(settings, window),
        None => apply_practice_window(settings, PracticeWindow::All),
    }
}

pub fn current_practice_window(settings: &TrainingSettings) -> Option<PracticeWindow> {
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

fn mixed_pool(settings: &TrainingSettings) -> Vec<char> {
    let letters = leveled_pool(&settings.progress_alphabet(), settings.level, settings);
    let digits = unlocked_prefix(DIGITS, settings.digits_level);
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

fn apply_sliding_window(full_unlocked: &[char], settings: &TrainingSettings) -> Vec<char> {
    let n = full_unlocked.len();
    if n == 0 {
        return Vec::new();
    }
    let start1 = settings.sliding_window_start.max(1).min(n as u32);
    let end1 = settings.sliding_window_end.max(1).min(n as u32);
    let start_idx = start1.min(end1) as usize;
    let end_idx = start1.max(end1) as usize;
    let pool = full_unlocked[start_idx.saturating_sub(1)..end_idx].to_vec();
    if pool.len() >= 2 {
        pool
    } else {
        full_unlocked[n.saturating_sub(2)..].to_vec()
    }
}

fn leveled_pool(alphabet: &[char], level: u32, settings: &TrainingSettings) -> Vec<char> {
    apply_sliding_window(&unlocked_prefix(alphabet, level), settings)
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
    fn last3_reapplied_after_level_up_includes_newest() {
        let mut s = TrainingSettings::default();
        s.char_set_mode = CharSetMode::Koch;
        s.level = 10;
        apply_practice_window(&mut s, PracticeWindow::Last3);
        s.level = 11;
        let stale = compute_char_pool(&s);
        let newest = s.progress_alphabet()[crate::level::unlocked_count_for_level(11) - 1];
        assert!(!stale.contains(&newest));
        apply_practice_window(&mut s, PracticeWindow::Last3);
        let pool = compute_char_pool(&s);
        assert!(pool.contains(&newest));
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
}
