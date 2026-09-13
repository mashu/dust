//! Alphabet leveling: level 1 unlocks two characters; each following level adds one.

pub const LEVEL_MIN: u32 = 1;

/// Characters unlocked at level `L` before clamping to an alphabet length.
pub fn unlocked_count_for_level(level: u32) -> usize {
    let level = if level < LEVEL_MIN { LEVEL_MIN } else { level };
    level as usize + 1
}

pub const fn max_level_for_len(alphabet_len: usize) -> u32 {
    let n = alphabet_len.saturating_sub(1) as u32;
    if n < LEVEL_MIN {
        LEVEL_MIN
    } else {
        n
    }
}

/// Prefix of `alphabet` unlocked at `level`. Always at least two characters when possible.
pub fn unlocked_prefix(alphabet: &[char], level: u32) -> Vec<char> {
    if alphabet.is_empty() {
        return Vec::new();
    }
    let n = unlocked_count_for_level(level)
        .min(alphabet.len())
        .max(2.min(alphabet.len()));
    alphabet[..n].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_one_unlocks_two() {
        let alphabet = ['A', 'B', 'C', 'D'];
        assert_eq!(unlocked_prefix(&alphabet, 1), vec!['A', 'B']);
        assert_eq!(unlocked_prefix(&alphabet, 3), vec!['A', 'B', 'C', 'D']);
        assert_eq!(max_level_for_len(alphabet.len()), 3);
    }

    #[test]
    fn short_alphabet_unlocks_all() {
        assert_eq!(unlocked_prefix(&['K', 'M'], 99), vec!['K', 'M']);
        assert_eq!(unlocked_prefix(&['X'], 1), vec!['X']);
    }
}

#[cfg(test)]
mod level_tests {
    use super::*;

    #[test]
    fn level_one_unlocks_two_characters() {
        assert_eq!(unlocked_count_for_level(0), 2);
        assert_eq!(unlocked_count_for_level(LEVEL_MIN), 2);
        assert_eq!(unlocked_count_for_level(5), 6);
    }

    #[test]
    fn the_top_level_is_one_below_the_alphabet_length() {
        assert_eq!(max_level_for_len(0), LEVEL_MIN);
        assert_eq!(max_level_for_len(1), LEVEL_MIN);
        assert_eq!(max_level_for_len(2), LEVEL_MIN);
        assert_eq!(max_level_for_len(10), 9);
    }

    #[test]
    fn a_prefix_never_runs_past_the_alphabet() {
        let alphabet = ['K', 'M', 'U'];
        assert!(unlocked_prefix(&[], 4).is_empty());
        assert_eq!(unlocked_prefix(&alphabet, 1), vec!['K', 'M']);
        assert_eq!(unlocked_prefix(&alphabet, 99), vec!['K', 'M', 'U']);
        // A one-character alphabet cannot be padded to two.
        assert_eq!(unlocked_prefix(&['K'], 1), vec!['K']);
    }
}
