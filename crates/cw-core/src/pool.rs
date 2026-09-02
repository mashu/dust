//! Character pool selection for Koch, digits, mixed, and custom modes.

use crate::morse::{
    digits_unlocked_count, is_digit, unlocked_char_count_for_level, DIGITS, LCWO_SEQUENCE,
};
use crate::settings::{CharSetMode, TrainingSettings};

pub fn compute_char_pool(settings: &TrainingSettings) -> Vec<char> {
    match settings.char_set_mode {
        CharSetMode::Mixed => mixed_pool(settings),
        CharSetMode::Digits => digits_pool(settings),
        CharSetMode::Custom => {
            let pool = unique_upper(&settings.custom_set);
            if pool.is_empty() {
                koch_pool(settings)
            } else {
                pool
            }
        }
        CharSetMode::Koch => koch_pool(settings),
    }
}

fn unique_upper(chars: &[char]) -> Vec<char> {
    let mut out = Vec::new();
    for ch in chars {
        let up = ch.to_ascii_uppercase();
        if up.is_whitespace() {
            continue;
        }
        if !out.contains(&up) {
            out.push(up);
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PracticeWindow {
    All,
    Last3,
    Last5,
}

pub fn unlocked_practice_count(settings: &TrainingSettings) -> usize {
    match settings.char_set_mode {
        CharSetMode::Digits => digits_unlocked_count(settings.digits_level.max(1)),
        _ => {
            let sequence = sequence_for(settings);
            unlocked_char_count_for_level(settings.koch_level.max(1)).min(sequence.len())
        }
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

fn sequence_for(settings: &TrainingSettings) -> &[char] {
    if settings.custom_sequence.is_empty() {
        LCWO_SEQUENCE
    } else {
        &settings.custom_sequence
    }
}

fn mixed_pool(settings: &TrainingSettings) -> Vec<char> {
    let koch_pool = koch_pool(settings);
    let digit_count = digits_unlocked_count(settings.digits_level.max(1)).min(DIGITS.len());
    let digits_pool: Vec<char> = DIGITS[..digit_count].to_vec();
    let mut union = koch_pool.clone();
    for d in digits_pool {
        if !union.contains(&d) {
            union.push(d);
        }
    }
    if union.len() >= 2 {
        union
    } else if koch_pool.len() >= 2 {
        koch_pool
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

fn digits_pool(settings: &TrainingSettings) -> Vec<char> {
    let count = digits_unlocked_count(settings.digits_level.max(1));
    let full_unlocked = DIGITS[..count].to_vec();
    apply_sliding_window(&full_unlocked, settings)
}

fn koch_pool(settings: &TrainingSettings) -> Vec<char> {
    let sequence = sequence_for(settings);
    let char_count = unlocked_char_count_for_level(settings.koch_level.max(1)).min(sequence.len());
    let full_unlocked = sequence[..char_count.max(2.min(sequence.len()))].to_vec();
    apply_sliding_window(&full_unlocked, settings)
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
        s.koch_level = 1;
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
        s.koch_level = 1;
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
        s.koch_level = 10;
        apply_practice_window(&mut s, PracticeWindow::Last3);
        let pool = compute_char_pool(&s);
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
}
