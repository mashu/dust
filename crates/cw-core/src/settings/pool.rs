//! Character pool selection from a leveled alphabet (Koch, digits, mixed, custom).

use crate::level::unlocked_prefix;
use crate::settings::{
    CharSetMode, LevelAxis, MixedAutoLevelAxis, PracticeWindow, TrainingSettings,
};

/// What `axis` has unlocked so far. A callsign tier is all of its
/// characters at once, and so is a word list: there is no prefix to unlock.
fn unlocked(settings: &TrainingSettings, axis: LevelAxis) -> Vec<char> {
    let alphabet = settings.alphabet(axis);
    match axis {
        LevelAxis::Callsign | LevelAxis::Words => alphabet,
        LevelAxis::Letters | LevelAxis::Digits => unlocked_prefix(&alphabet, settings.level(axis)),
    }
}

/// What each axis the mode draws from has unlocked.
///
/// Mixed draws from whichever of its two axes the share of letters leaves
/// in play — unless that leaves nothing, as a sequence of only digits with
/// letters at 100% would. Then the other axis sends: a session of silent
/// groups is not a choice anyone made.
fn unlocked_axes(settings: &TrainingSettings) -> Vec<Vec<char>> {
    let mode = settings.curriculum.char_set_mode;
    if mode != CharSetMode::Mixed {
        return vec![unlocked(settings, mode.axis())];
    }
    let axes = [MixedAutoLevelAxis::Letters, MixedAutoLevelAxis::Digits].map(|axis| {
        (
            settings.mixed_axis_practised(axis),
            unlocked(settings, axis.into()),
        )
    });
    let practised: Vec<Vec<char>> = axes
        .iter()
        .filter(|(practised, chars)| *practised && !chars.is_empty())
        .map(|(_, chars)| chars.clone())
        .collect();
    if practised.is_empty() {
        axes.into_iter().map(|(_, chars)| chars).collect()
    } else {
        practised
    }
}

/// How many of the newest `unlocked` characters `window` keeps. A window
/// wider than what is unlocked keeps all of it.
fn window_len(unlocked: usize, window: PracticeWindow) -> usize {
    match window {
        PracticeWindow::All => unlocked,
        PracticeWindow::Last3 => unlocked.min(3),
        PracticeWindow::Last5 => unlocked.min(5),
    }
}

/// The characters a group is drawn from: what is unlocked, narrowed to the
/// practice window — each axis by its own length, so Mixed's newest three are
/// three letters and three digits. A callsign tier is never narrowed, nor
/// are the characters of a word list.
pub fn compute_char_pool(settings: &TrainingSettings) -> Vec<char> {
    let whole = !settings.curriculum.char_set_mode.draws_characters();
    let window = settings.curriculum.practice_window;
    let mut pool: Vec<char> = Vec::new();
    for chars in unlocked_axes(settings) {
        let keep = if whole {
            chars.len()
        } else {
            window_len(chars.len(), window)
        };
        for ch in &chars[chars.len() - keep..] {
            if !pool.contains(ch) {
                pool.push(*ch);
            }
        }
    }
    pool
}

/// How many characters are unlocked, on the wider axis for Mixed: what the
/// practice window is measured against.
pub fn unlocked_practice_count(settings: &TrainingSettings) -> usize {
    unlocked_axes(settings)
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
}

/// The window in force: the one chosen, or everything while fewer characters
/// are unlocked than it would keep. The choice itself stays stored, so Last5
/// comes back by itself once five characters are in.
pub fn current_practice_window(settings: &TrainingSettings) -> PracticeWindow {
    let unlocked = unlocked_practice_count(settings);
    match settings.curriculum.practice_window {
        PracticeWindow::Last5 if unlocked >= 5 => PracticeWindow::Last5,
        PracticeWindow::Last3 if unlocked >= 3 => PracticeWindow::Last3,
        _ => PracticeWindow::All,
    }
}

/// The character the latest level brought in: the one the learner has had
/// least time with. Mixed answers for the axis that moved last, while it is
/// being practised. A callsign tier brings in a shape, not a character, and
/// a word list brings in nothing at all, so neither has one.
pub fn newest_unlocked(settings: &TrainingSettings) -> Option<char> {
    let newest = |axis: LevelAxis| unlocked(settings, axis).last().copied();
    match settings.curriculum.char_set_mode {
        CharSetMode::Callsign | CharSetMode::Words | CharSetMode::Contest => None,
        CharSetMode::Mixed => {
            let moved_last = settings.auto_level.mixed_auto_level_next_axis.flip();
            [moved_last, moved_last.flip()]
                .into_iter()
                .filter(|axis| settings.mixed_axis_practised(*axis))
                .find_map(|axis| newest(axis.into()))
                // A share that leaves its axis nothing to send draws from
                // the other one, and so does the newest character.
                .or_else(|| compute_char_pool(settings).last().copied())
        }
        mode => newest(mode.axis()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_level_3(mode: CharSetMode) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = mode;
        settings.curriculum.level = 3;
        settings.curriculum.digits_level = 3;
        settings
    }

    #[test]
    fn the_newest_character_is_the_one_the_level_brought_in() {
        let koch = at_level_3(CharSetMode::Koch);
        assert_eq!(
            newest_unlocked(&koch),
            compute_char_pool(&koch).last().copied()
        );
        assert_eq!(newest_unlocked(&at_level_3(CharSetMode::Digits)), Some('3'));
        // However narrow the window, the newest is in it.
        let mut newest_three = koch.clone();
        newest_three.curriculum.practice_window = PracticeWindow::Last3;
        assert_eq!(newest_unlocked(&newest_three), newest_unlocked(&koch));
        // Level 1 brings in its two characters at once; the second is newest.
        let mut first = koch;
        first.curriculum.level = 1;
        assert_eq!(newest_unlocked(&first), Some('M'));
    }

    #[test]
    fn mixed_points_at_the_axis_that_moved_last() {
        let mut mixed = at_level_3(CharSetMode::Mixed);
        // The next move is the letters', so the digits moved last.
        mixed.auto_level.mixed_auto_level_next_axis = MixedAutoLevelAxis::Letters;
        assert_eq!(newest_unlocked(&mixed), Some('3'));
        mixed.auto_level.mixed_auto_level_next_axis = MixedAutoLevelAxis::Digits;
        assert!(newest_unlocked(&mixed).is_some_and(|ch| !ch.is_ascii_digit()));
        // All letters: the digits are not practised, whatever moved last.
        mixed.curriculum.mixed_letters_percent = 100;
        mixed.auto_level.mixed_auto_level_next_axis = MixedAutoLevelAxis::Letters;
        assert!(newest_unlocked(&mixed).is_some_and(|ch| !ch.is_ascii_digit()));
    }

    #[test]
    fn a_callsign_tier_has_no_newest_character() {
        assert_eq!(newest_unlocked(&at_level_3(CharSetMode::Callsign)), None);
    }

    #[test]
    fn koch_level_1_is_k_and_m() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.level = 1;
        let pool = compute_char_pool(&s);
        assert_eq!(pool, vec!['K', 'M']);
    }

    #[test]
    fn digits_level_1_is_0_and_1() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Digits;
        s.curriculum.digits_level = 1;
        let pool = compute_char_pool(&s);
        assert_eq!(pool, vec!['0', '1']);
    }

    #[test]
    fn mixed_unions_letters_and_digits() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.level = 1;
        s.curriculum.digits_level = 1;
        let pool = compute_char_pool(&s);
        assert!(pool.contains(&'K'));
        assert!(pool.contains(&'M'));
        assert!(pool.contains(&'0'));
        assert!(pool.contains(&'1'));
    }

    #[test]
    fn last3_window_keeps_newest_letters() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.level = 10;
        s.curriculum.practice_window = PracticeWindow::Last3;
        let pool = compute_char_pool(&s);
        assert_eq!(pool.len(), 3);
        assert_eq!(current_practice_window(&s), PracticeWindow::Last3);
    }

    #[test]
    fn last3_follows_level_from_named_window() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.level = 10;
        s.curriculum.practice_window = PracticeWindow::Last3;
        s.curriculum.level = 11;
        let newest = s.progress_alphabet()[crate::level::unlocked_count_for_level(11) - 1];
        let pool = compute_char_pool(&s);
        assert!(pool.contains(&newest));
        assert_eq!(pool.len(), 3);
        assert_eq!(current_practice_window(&s), PracticeWindow::Last3);
    }

    #[test]
    fn a_level_unlocks_the_start_of_an_order_of_ones_own() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.custom_sequence = vec!['Q', 'R', 'S', 'T', 'U'];
        s.curriculum.level = 1;
        assert_eq!(compute_char_pool(&s), vec!['Q', 'R']);
        s.curriculum.level = 3;
        assert_eq!(compute_char_pool(&s), vec!['Q', 'R', 'S', 'T']);
    }

    #[test]
    fn digits_max_level_unlocks_all_ten() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Digits;
        s.curriculum.digits_level = crate::morse::MAX_DIGITS_LEVEL;
        let pool = compute_char_pool(&s);
        assert_eq!(pool.len(), 10);
        s.curriculum.digits_level = 10;
        let clamped = s.clamp();
        assert_eq!(clamped.curriculum.digits_level, 9);
        assert_eq!(compute_char_pool(&clamped).len(), 10);
    }

    #[test]
    fn switching_mode_resets_stale_window() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.level = 10;
        s.curriculum.practice_window = PracticeWindow::Last3;
        s.curriculum.char_set_mode = CharSetMode::Digits;
        s.curriculum.digits_level = 1;
        s = s.clamp();
        let pool = compute_char_pool(&s);
        assert_eq!(pool, vec!['0', '1']);
        assert_eq!(current_practice_window(&s), PracticeWindow::All);
    }

    #[test]
    fn last5_survives_level_up_from_five_chars() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.level = 4;
        s.curriculum.practice_window = PracticeWindow::Last5;
        assert_eq!(unlocked_practice_count(&s), 5);
        assert_eq!(current_practice_window(&s), PracticeWindow::Last5);
        s.curriculum.level = 5;
        s = s.clamp();
        assert_eq!(current_practice_window(&s), PracticeWindow::Last5);
        assert_eq!(compute_char_pool(&s).len(), 5);
        let newest = s.progress_alphabet()[crate::level::unlocked_count_for_level(5) - 1];
        assert!(compute_char_pool(&s).contains(&newest));
    }

    #[test]
    fn last3_retargets_after_level_change() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.level = 10;
        s.curriculum.practice_window = PracticeWindow::Last3;
        s.curriculum.level = 11;
        s = s.clamp();
        assert_eq!(current_practice_window(&s), PracticeWindow::Last3);
        let newest = s.progress_alphabet()[crate::level::unlocked_count_for_level(11) - 1];
        assert!(compute_char_pool(&s).contains(&newest));
    }

    #[test]
    fn mixed_letter_pool_excludes_sequence_digits() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.level = s.max_level(LevelAxis::Letters);
        s.curriculum.practice_window = PracticeWindow::All;
        assert!(compute_char_pool(&s).contains(&'5'));
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.digits_level = 1;
        s = s.clamp();
        let pool = compute_char_pool(&s);
        assert!(!pool.contains(&'5'));
        assert!(pool.contains(&'0'));
        assert!(pool.contains(&'1'));
        assert!(
            pool.iter()
                .filter(|c| c.is_ascii_digit())
                .all(|c| *c == '0' || *c == '1')
        );
    }

    #[test]
    fn unknown_characters_in_an_order_are_dropped() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.custom_sequence = vec!['A', '*', 'B'];
        s.curriculum.level = 1;
        s.curriculum.practice_window = PracticeWindow::All;
        assert_eq!(compute_char_pool(&s), vec!['A', 'B']);
    }

    #[test]
    fn mixed_last3_windows_letters_and_digits_separately() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.level = 10;
        s.curriculum.digits_level = crate::morse::MAX_DIGITS_LEVEL;
        s.curriculum.practice_window = PracticeWindow::Last3;
        let pool = compute_char_pool(&s);
        let letters: Vec<char> = pool
            .iter()
            .copied()
            .filter(|c| !c.is_ascii_digit())
            .collect();
        let digits: Vec<char> = pool
            .iter()
            .copied()
            .filter(|c| c.is_ascii_digit())
            .collect();
        assert_eq!(letters.len(), 3);
        assert_eq!(digits, vec!['7', '8', '9']);
    }

    #[test]
    fn mixed_100_percent_omits_digits_from_pool() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.level = 5;
        s.curriculum.digits_level = crate::morse::MAX_DIGITS_LEVEL;
        s.curriculum.mixed_letters_percent = 100;
        s.curriculum.practice_window = PracticeWindow::All;
        let pool = compute_char_pool(&s);
        assert!(!pool.is_empty());
        assert!(pool.iter().all(|c| !c.is_ascii_digit()));
    }

    #[test]
    fn mixed_0_percent_omits_letters_from_pool() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.level = 10;
        s.curriculum.digits_level = 3;
        s.curriculum.mixed_letters_percent = 0;
        s.curriculum.practice_window = PracticeWindow::All;
        let pool = compute_char_pool(&s);
        assert!(!pool.is_empty());
        assert!(pool.iter().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn mixed_0_percent_last3_follows_digit_count() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.level = 1;
        s.curriculum.digits_level = crate::morse::MAX_DIGITS_LEVEL;
        s.curriculum.mixed_letters_percent = 0;
        s.curriculum.practice_window = PracticeWindow::Last3;
        assert!(unlocked_practice_count(&s) >= 3);
        assert_eq!(current_practice_window(&s), PracticeWindow::Last3);
        let pool = compute_char_pool(&s);
        assert_eq!(pool, vec!['7', '8', '9']);
    }

    fn koch(level: u32) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.level = level;
        s
    }

    #[test]
    fn a_named_window_narrower_than_the_pool_relaxes_to_everything() {
        // Last5 stays stored, but only three characters are unlocked.
        let mut s = koch(2);
        s.curriculum.practice_window = PracticeWindow::Last5;
        assert_eq!(current_practice_window(&s), PracticeWindow::All);
        assert_eq!(s.curriculum.practice_window, PracticeWindow::Last5);
        assert_eq!(compute_char_pool(&s).len(), 3);
    }

    #[test]
    fn a_window_never_leaves_fewer_than_two_characters() {
        // Level 1 unlocks exactly two; Last3 cannot cut into that.
        let mut s = koch(1);
        s.curriculum.practice_window = PracticeWindow::Last3;
        assert_eq!(compute_char_pool(&s).len(), 2);
    }

    #[test]
    fn mixed_counts_the_wider_of_the_two_axes() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.level = 9; // ten letters
        s.curriculum.digits_level = 1; // two digits
        assert_eq!(unlocked_practice_count(&s), 10);

        // Letters only: digits stop counting.
        s.curriculum.mixed_letters_percent = 100;
        assert_eq!(unlocked_practice_count(&s), 10);
        assert!(compute_char_pool(&s).iter().all(|c| !c.is_ascii_digit()));

        // Digits only: letters stop counting.
        s.curriculum.mixed_letters_percent = 0;
        assert_eq!(unlocked_practice_count(&s), 2);
        assert!(compute_char_pool(&s).iter().all(char::is_ascii_digit));
    }

    #[test]
    fn a_mixed_pool_of_one_letter_keeps_the_letters_it_has() {
        // One letter in the sequence and no digits leaves a single-character
        // union; the letter list is used rather than an empty pool.
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.custom_sequence = vec!['K'];
        s.curriculum.mixed_letters_percent = 100;
        assert_eq!(compute_char_pool(&s), vec!['K']);
    }

    /// Mixed with a sequence of nothing but digits has no letters to give,
    /// and at 100% letters it used to give no digits either: an empty pool,
    /// and a session of silent groups.
    #[test]
    fn a_share_that_leaves_nothing_in_play_falls_back_to_the_other_axis() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.custom_sequence = vec!['0', '1', '2', '3'];
        s.curriculum.sequence_is_custom = true;
        s.curriculum.mixed_letters_percent = 100;
        s.curriculum.digits_level = 3;
        let s = s.clamp();
        assert_eq!(compute_char_pool(&s), vec!['0', '1', '2', '3']);
        assert_eq!(unlocked_practice_count(&s), 4);
    }

    /// Whatever the settings, there is always something to send.
    #[test]
    fn no_setting_leaves_the_pool_empty() {
        for mode in [
            CharSetMode::Koch,
            CharSetMode::Digits,
            CharSetMode::Mixed,
            CharSetMode::Callsign,
        ] {
            for sequence in [vec![], vec!['#', '*'], vec!['0', '1'], vec!['K']] {
                for letters_percent in [0, 50, 100] {
                    let mut s = TrainingSettings::default();
                    s.curriculum.char_set_mode = mode;
                    s.curriculum.custom_sequence = sequence.clone();
                    s.curriculum.mixed_letters_percent = letters_percent;
                    assert!(
                        !compute_char_pool(&s).is_empty(),
                        "{mode:?} {sequence:?} {letters_percent}%"
                    );
                }
            }
        }
    }
}
