//! Session scoring, the way RufzXP scores a callsign, with what copying groups
//! adds to it.
//!
//! RufzXP's manual describes its points without publishing the formula: the
//! most a call can earn grows with the square of the speed and with the
//! length of the call; every error cuts what is left hard — one error to a
//! quarter, two to a ninth, three to a sixteenth; hearing the call again
//! costs half; and the time taken to type it comes off logarithmically. Each
//! group here is scored that way — what is known of the rules, in a form of
//! our own — and a session is what its groups earn, added up.
//!
//! Two things RufzXP has no need of, here do the work it leaves to the
//! callsigns themselves. A callsign is drawn from the whole alphabet; a group
//! may be drawn from two Koch letters, and copying one of two is far less
//! than copying one of twenty-six. So every character is worth what it
//! tells you — `log₂` of how many it could have been — against a character
//! of the full alphabet. And a group may be sent with Farnsworth spacing,
//! its characters fast and the gaps between them long, so its speed is the
//! two speeds together.

use crate::alignment::align_group;
use crate::morse::is_scored_char;
use crate::session::{GroupResult, SessionTiming};

const MAX_EFFECTIVE_ALPHABET: f64 = 48.0;

/// The speed a group earns its plain points at.
pub const REFERENCE_WPM: f64 = 20.0;
/// What one character is worth, copied clean from the full alphabet at
/// [`REFERENCE_WPM`], answered at once, from a single send.
pub const POINTS_PER_CHAR: f64 = 10.0;
/// The alphabet a character is worth [`POINTS_PER_CHAR`] in: the letters.
const REFERENCE_ALPHABET: f64 = 26.0;
/// The quickest a listener can be said to answer, per character: as fast
/// as anyone types one. An answer this quick costs nothing, and none is kept
/// as quicker: one finished while the group was still being sent, typed
/// along, has no response time worth the name.
pub const MIN_RESPONSE_MS_PER_CHAR: f64 = 100.0;
/// What each tenfold of time a character beyond that costs, of the points.
const TIME_COST_PER_DECADE: f64 = 0.25;
/// The least a slow answer keeps: slowness costs, but never everything.
const SLOWEST_KEEPS: f64 = 0.25;
/// What each send of a group after the first leaves, as RufzXP's replay.
const RESENT_KEEPS: f64 = 0.5;

/// The average answer of a session with none to average: a second a
/// character.
const NO_ANSWERS_MS: f64 = 1_000.0;

/// How one group was copied: everything its points are worked out from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroupCopy {
    /// Characters sent.
    pub chars: u32,
    /// Characters missed, wrong or typed in excess.
    pub errors: u32,
    /// The speed it was sent at, in words a minute ([`copy_speed`]).
    pub wpm: f64,
    /// How long the answer took, a character, from the end of the send.
    pub ms_per_char: f64,
    /// How many times it was sent before it was answered.
    pub sends: u32,
}

impl GroupCopy {
    /// `sent`, copied as `received`.
    pub fn of(sent: &str, received: &str, wpm: f64, ms_per_char: f64, sends: u32) -> Self {
        let errors = align_group(sent, received)
            .iter()
            .filter(|pair| !pair.matched)
            .count() as u32;
        Self {
            chars: count_valid_letters(sent),
            errors,
            wpm,
            ms_per_char,
            sends,
        }
    }
}

/// The speed a group is scored at: the characters' own speed and the pace
/// the spacing sets, together. With the two linked it is simply the speed
/// set; Farnsworth spacing — fast characters, long gaps — earns between the
/// two.
pub fn copy_speed(char_wpm: f64, effective_wpm: f64) -> f64 {
    let (char_wpm, effective_wpm) = (char_wpm.max(1.0), effective_wpm.max(1.0));
    (char_wpm * effective_wpm.min(char_wpm)).sqrt()
}

/// Faster earns more, with the square of the speed.
pub fn speed_factor(wpm: f64) -> f64 {
    let wpm = if wpm.is_finite() { wpm.max(1.0) } else { 1.0 };
    (wpm / REFERENCE_WPM).powi(2)
}

/// What is left after `errors`: a quarter for one, a ninth for two.
pub fn error_factor(errors: u32) -> f64 {
    1.0 / f64::from(errors + 1).powi(2)
}

/// What is left after answering `ms_per_char` a character: everything as
/// fast as anyone types, a quarter less for each tenfold beyond that.
pub fn time_factor(ms_per_char: f64) -> f64 {
    let ms = if ms_per_char.is_finite() {
        ms_per_char.max(MIN_RESPONSE_MS_PER_CHAR)
    } else {
        MIN_RESPONSE_MS_PER_CHAR
    };
    (1.0 - TIME_COST_PER_DECADE * (ms / MIN_RESPONSE_MS_PER_CHAR).log10()).max(SLOWEST_KEEPS)
}

/// What is left of a group sent `sends` times: half for each send after the
/// first.
pub fn repeat_factor(sends: u32) -> f64 {
    RESENT_KEEPS.powi(sends.max(1) as i32 - 1)
}

/// What a group earns.
pub fn group_points(copy: &GroupCopy) -> f64 {
    POINTS_PER_CHAR
        * f64::from(copy.chars)
        * speed_factor(copy.wpm)
        * error_factor(copy.errors)
        * time_factor(copy.ms_per_char)
        * repeat_factor(copy.sends)
}

/// What a character is worth from an alphabet of `effective_size`: what it
/// tells you, against a letter of the full alphabet.
pub fn alphabet_weight(effective_size: f64) -> f64 {
    let size = if effective_size.is_finite() {
        effective_size.max(2.0)
    } else {
        2.0
    };
    size.ln() / REFERENCE_ALPHABET.ln()
}

/// What a session earns: its groups' points added up, each character worth
/// what it tells you from the alphabet the session drew on.
pub fn session_points(copies: &[GroupCopy], effective_alphabet_size: f64) -> f64 {
    let points: f64 = copies.iter().map(group_points).sum();
    let score = points * alphabet_weight(effective_alphabet_size);
    (score * 100.0).round() / 100.0
}

fn count_valid_letters(text: &str) -> u32 {
    text.chars()
        .filter(|ch| is_scored_char(ch.to_ascii_uppercase()))
        .count() as u32
}

pub fn calculate_total_chars(sent_groups: &[impl AsRef<str>]) -> u32 {
    let total: u32 = sent_groups
        .iter()
        .map(|g| count_valid_letters(g.as_ref()))
        .sum();
    total.max(1)
}

/// Shannon entropy (natural log) converted to an effective alphabet size, with Miller–Madow bias correction.
pub fn calculate_effective_alphabet_size(sent_groups: &[impl AsRef<str>]) -> f64 {
    let mut counts = std::collections::BTreeMap::new();
    let mut total = 0u32;
    for g in sent_groups {
        for ch in g.as_ref().chars() {
            let up = ch.to_ascii_uppercase();
            if is_scored_char(up) {
                *counts.entry(up).or_insert(0u32) += 1;
                total += 1;
            }
        }
    }
    let k = counts.len();
    if total == 0 || k == 0 {
        return 1.0;
    }
    let mut h = 0.0;
    for count in counts.values() {
        let p = f64::from(*count) / f64::from(total);
        if p > 0.0 {
            h += -p * p.ln();
        }
    }
    h += f64::from((k.saturating_sub(1)) as u32) / (2.0 * f64::from(total));
    h.exp().clamp(1.0, MAX_EFFECTIVE_ALPHABET)
}

/// The average of a session's answer times a character, its `groups` timed
/// as `timings` say, as older builds show it
/// ([`crate::SessionResult::avg_response_ms`]).
pub fn compute_average_response_ms(groups: &[GroupResult], timings: &[SessionTiming]) -> f64 {
    let (sum, count) = groups
        .iter()
        .zip(timings)
        .map(|(group, timing)| timing.ms_per_char(&group.sent))
        .filter(|v| v.is_finite() && *v > 0.0)
        .fold((0.0, 0u32), |(sum, count), v| (sum + v, count + 1));
    if count == 0 {
        NO_ANSWERS_MS
    } else {
        sum / f64::from(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(chars: u32, wpm: f64) -> GroupCopy {
        GroupCopy {
            chars,
            errors: 0,
            wpm,
            ms_per_char: MIN_RESPONSE_MS_PER_CHAR,
            sends: 1,
        }
    }

    /// Thirty-five words a minute is worth more than twenty: with the square
    /// of the speed, about three times as much.
    #[test]
    fn speed_earns_with_its_square() {
        let at_20 = group_points(&clean(5, 20.0));
        let at_35 = group_points(&clean(5, 35.0));
        assert_eq!(at_20, 5.0 * POINTS_PER_CHAR);
        assert!((at_35 / at_20 - (35.0f64 / 20.0).powi(2)).abs() < 1e-9);
        assert!(group_points(&clean(5, f64::NAN)).is_finite());
    }

    /// A longer group earns more, in proportion.
    #[test]
    fn length_earns_in_proportion() {
        assert_eq!(
            group_points(&clean(6, 20.0)),
            2.0 * group_points(&clean(3, 20.0))
        );
    }

    /// RufzXP's cuts: one error leaves a quarter, two a ninth, three a
    /// sixteenth; nothing copied leaves next to nothing.
    #[test]
    fn errors_cut_hard() {
        assert_eq!(error_factor(0), 1.0);
        assert_eq!(error_factor(1), 0.25);
        assert!((error_factor(2) - 1.0 / 9.0).abs() < 1e-12);
        assert_eq!(error_factor(3), 1.0 / 16.0);
        let missed = GroupCopy::of("KMRSU", "", 20.0, 500.0, 1);
        assert_eq!(missed.errors, 5);
        assert!(group_points(&missed) < 0.03 * group_points(&clean(5, 20.0)));
        let one_wrong = GroupCopy::of("KMRSU", "KMXSU", 20.0, 100.0, 1);
        assert_eq!((one_wrong.chars, one_wrong.errors), (5, 1));
        let one_extra = GroupCopy::of("KMR", "KMRR", 20.0, 100.0, 1);
        assert_eq!(
            one_extra.errors, 1,
            "typing a character too many is an error"
        );
    }

    /// Time costs, gently: nothing as fast as anyone types, a quarter for
    /// each tenfold beyond, and never more than three quarters.
    #[test]
    fn slow_answers_cost_a_little_and_never_everything() {
        assert_eq!(time_factor(0.0), 1.0);
        assert_eq!(time_factor(100.0), 1.0);
        assert!((time_factor(1_000.0) - 0.75).abs() < 1e-12);
        assert!((time_factor(10_000.0) - 0.5).abs() < 1e-12);
        assert_eq!(time_factor(1e9), SLOWEST_KEEPS);
        assert_eq!(
            time_factor(f64::INFINITY),
            1.0,
            "nothing measured, nothing taken"
        );
        assert!(time_factor(600.0) > time_factor(1_200.0));
    }

    /// Each send after the first halves what the group is worth, as hearing
    /// the call again does in RufzXP.
    #[test]
    fn hearing_it_again_costs_half() {
        assert_eq!(repeat_factor(0), 1.0);
        assert_eq!(repeat_factor(1), 1.0);
        assert_eq!(repeat_factor(2), 0.5);
        assert_eq!(repeat_factor(3), 0.25);
    }

    /// A character from two Koch letters tells you a bit; one from the
    /// alphabet tells you nearly five, and is worth that much more.
    #[test]
    fn a_character_is_worth_what_it_tells_you() {
        assert_eq!(alphabet_weight(26.0), 1.0);
        assert!((alphabet_weight(2.0) - 2f64.ln() / 26f64.ln()).abs() < 1e-12);
        assert_eq!(alphabet_weight(1.0), alphabet_weight(2.0));
        assert!(alphabet_weight(40.0) > 1.0);
        assert!(alphabet_weight(f64::NAN).is_finite());
    }

    /// Farnsworth spacing earns between its two speeds; linked, the speed is
    /// the one set, and spacing cannot be set faster than the characters.
    #[test]
    fn farnsworth_spacing_earns_between_its_speeds() {
        assert_eq!(copy_speed(25.0, 25.0), 25.0);
        let farnsworth = copy_speed(25.0, 10.0);
        assert!(10.0 < farnsworth && farnsworth < 25.0, "{farnsworth}");
        assert_eq!(copy_speed(20.0, 30.0), 20.0);
    }

    /// A session is its groups added up, weighted by its alphabet.
    #[test]
    fn a_session_adds_its_groups_up() {
        let copies = [clean(5, 20.0), clean(5, 35.0)];
        let expected =
            (group_points(&copies[0]) + group_points(&copies[1])) * alphabet_weight(10.0);
        assert!((session_points(&copies, 10.0) - expected).abs() < 0.01);
        assert_eq!(session_points(&[], 26.0), 0.0);
    }
}
