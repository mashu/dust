//! Morse timing: the units and gaps a send is built from, the stations that
//! send it, who else is calling, and the plan of what is heard when.
//!
//! - this module: dot lengths, Farnsworth spacing and the gaps between sends;
//! - [`station`]: the operator at the other end — pitch, speed, level, fist;
//! - [`pileup`]: whoever else is calling, placed where the receiver hears them;
//! - [`plan`]: a send laid out as tones against one clock, for any backend;
//! - [`envelope`]: the keying envelope each tone is shaped by.

use crate::settings::{EXTRA_WORD_SPACE_MAX, EXTRA_WORD_SPACE_MIN};

mod envelope;
mod pileup;
mod plan;
mod station;

pub use envelope::{EnvelopePoint, EnvelopeShape, envelope_shape};
#[cfg(test)]
pub(crate) use pileup::resolve_pileup;
pub use pileup::{Interferer, Transmission};
pub(crate) use pileup::{PILEUP_CALLER_SEPARATION_HZ, PILEUP_SKIRT_DB, free_room, pick_in_room};
pub use plan::{
    DEFAULT_TARGET_GAIN, PlannedTransmission, PlaybackPlan, SEND_RELEASE_SEC, ToneEvent,
    plan_morse_playback_for, plan_transmission,
};
pub(crate) use plan::{plan_from, plan_morse_playback_at};
pub(crate) use station::station_speed_grid;
pub use station::{StationVoice, resolve_station};

/// The word-space stretch, inside its range whatever is set.
fn clamp_extra_spacing(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(EXTRA_WORD_SPACE_MIN, EXTRA_WORD_SPACE_MAX)
    } else {
        1.0
    }
}

/// PARIS-style dot duration in seconds from character WPM.
pub fn dot_seconds(wpm: f64) -> f64 {
    1.2 / wpm.max(1.0)
}

/// Word space plus the reading pause, used between different groups.
pub fn compute_after_group_gap_ms(
    char_wpm: f64,
    effective_wpm: f64,
    extra_word_space: f64,
    pause_sec: f64,
) -> u32 {
    let word = compute_group_gap_for_wpm(char_wpm, effective_wpm, extra_word_space);
    let pause = (pause_sec.max(0.0) * 1000.0).round() as u32;
    word.saturating_add(pause)
}

/// Word-space gap for the WPM that was actually sent, not the settings minimum.
pub fn compute_group_gap_for_wpm(char_wpm: f64, effective_wpm: f64, extra_word_space: f64) -> u32 {
    let word_space_sec =
        7.0 * spacing_unit_sec(char_wpm, effective_wpm) * clamp_extra_spacing(extra_word_space);
    (word_space_sec * 1000.0).round() as u32
}

/// One unit of the gaps between characters and words, in seconds: three of
/// them between letters, seven between words.
///
/// At an effective speed below the character speed this is Farnsworth
/// timing as the ARRL defines it (Jon Bloom, QEX 1990). PARIS has fifty
/// units, nineteen of them letter and word gaps; the characters keep their
/// own speed and the whole of the slow-down goes into those nineteen, so that
/// a word still takes sixty seconds over the effective speed. Stretching them
/// only to the effective speed's dot would leave the word arriving faster
/// than the speed asked for.
fn spacing_unit_sec(char_wpm: f64, effective_wpm: f64) -> f64 {
    let char_wpm = char_wpm.max(1.0);
    let effective_wpm = effective_wpm.max(1.0).min(char_wpm);
    if effective_wpm >= char_wpm {
        return dot_seconds(char_wpm);
    }
    let delay_per_word = (60.0 * char_wpm - 37.2 * effective_wpm) / (effective_wpm * char_wpm);
    delay_per_word / 19.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_space_between_groups_follows_the_slower_speed() {
        let fast = compute_group_gap_for_wpm(20.0, 20.0, 1.0);
        let slow = compute_group_gap_for_wpm(20.0, 10.0, 1.0);
        assert!(slow > fast);
        // Extra spacing multiplies it.
        assert_eq!(compute_group_gap_for_wpm(20.0, 20.0, 2.0), fast * 2);
        // An effective speed above the character speed is ignored.
        assert_eq!(compute_group_gap_for_wpm(20.0, 40.0, 1.0), fast);
        // Nonsense speeds do not divide by zero.
        assert!(compute_group_gap_for_wpm(0.0, 0.0, 0.0) > 0);
        assert_eq!(clamp_extra_spacing(-1.0), EXTRA_WORD_SPACE_MIN);
        assert_eq!(clamp_extra_spacing(f64::NAN), 1.0);
    }

    #[test]
    fn the_pause_after_a_group_sits_on_top_of_the_word_space() {
        let word = compute_group_gap_for_wpm(20.0, 20.0, 1.0);
        assert_eq!(
            compute_after_group_gap_ms(20.0, 20.0, 1.0, 2.0),
            word + 2_000
        );
        assert_eq!(compute_after_group_gap_ms(20.0, 20.0, 1.0, 0.0), word);
        assert_eq!(compute_after_group_gap_ms(20.0, 20.0, 1.0, -1.0), word);
    }
}
