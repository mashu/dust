//! Software paddle keyer for HID `[` / `]` mappings.
//!
//! Holding a paddle repeats that element at PARIS speed — dah-dah-dah for O
//! is a hold, not three taps. Keyboard auto-repeat is the wrong clock; this
//! module is the keyer.
//!
//! Modes follow the Curtis / Accu-keyer / W6SRY conventions, not the mangled
//! "ultimatic" in most web trainers:
//!
//! - **Straight** — the key is a contact. Hold length is the element; two dits
//!   is the dit/dah split.
//! - **Iambic A** (Curtis) — squeeze alternates. Releasing both during an
//!   element finishes that element and stops. A tap of the opposite paddle
//!   during an element is remembered once.
//! - **Iambic B** (Accu-keyer) — same squeeze, but the opposite paddle being
//!   down at any point in the element latches one extra opposite element after
//!   you let go. That extra dit after a squeeze-released dah is the whole
//!   difference.
//! - **Ultimatic** (W6SRY) — last paddle still held wins and *repeats*, it does
//!   not alternate. Squeeze-dit while holding dah is a run of dits; release the
//!   dit and the still-held dah continues. A tap of the opposite paddle inserts
//!   one opposite element (dit/dah memory) and then the held paddle resumes.
//!   That last-dash-or-dot rule is how X (`-..-`) and P (`.--.`) are sent.

use serde::{Deserialize, Serialize};

use crate::morse::{decode_morse_pattern, is_morse_code_prefix};
use crate::timing::dot_seconds;

/// Dot duration in milliseconds for paddle letter-spacing.
pub fn dit_ms_for_wpm(wpm: f64) -> u64 {
    (dot_seconds(wpm) * 1000.0).round().clamp(1.0, 1_000.0) as u64
}

/// Gap, in dits, at which a letter is over. Intra-character spacing is one
/// dit; inter-character is three. Two is the split between them.
pub const LETTER_GAP_DITS: u64 = 2;

/// Hold longer than this many dits and a straight key is a dah.
pub const STRAIGHT_DAH_DITS: u64 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyerMode {
    Straight,
    #[default]
    IambicA,
    IambicB,
    Ultimatic,
}

impl KeyerMode {
    pub fn is_straight(self) -> bool {
        matches!(self, Self::Straight)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Straight => "Straight",
            Self::IambicA => "Iambic A",
            Self::IambicB => "Iambic B",
            Self::Ultimatic => "Ultimatic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paddle {
    Dit,
    Dah,
}

impl Paddle {
    pub fn mark(self) -> char {
        match self {
            Self::Dit => '.',
            Self::Dah => '-',
        }
    }

    pub fn flipped(self) -> Self {
        match self {
            Self::Dit => Self::Dah,
            Self::Dah => Self::Dit,
        }
    }

    /// Element length in dits, not counting the trailing space.
    pub fn dits(self) -> u64 {
        match self {
            Self::Dit => 1,
            Self::Dah => 3,
        }
    }
}

/// `[` is dit and `]` is dah, unless `swap` is set.
pub fn paddle_from_bracket(ch: char, swap: bool) -> Option<Paddle> {
    let paddle = match ch {
        '[' => Paddle::Dit,
        ']' => Paddle::Dah,
        _ => return None,
    };
    Some(if swap { paddle.flipped() } else { paddle })
}

/// Running Morse character being squeezed on the paddles.
#[derive(Clone, Debug, Default)]
pub struct PaddleDecoder {
    pattern: String,
    last_at_ms: Option<u64>,
}

impl PaddleDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.pattern.is_empty()
    }

    pub fn push(&mut self, paddle: Paddle) {
        self.pattern.push(paddle.mark());
        if !is_morse_code_prefix(&self.pattern) {
            self.pattern.pop();
        }
    }

    /// Record one element. If the pause before this hit already ended a
    /// letter, that letter is returned first and this hit starts the next.
    pub fn hit(&mut self, paddle: Paddle, now_ms: u64, dit_ms: u64) -> Option<char> {
        let flushed = self.poll(now_ms, dit_ms);
        self.push(paddle);
        self.last_at_ms = Some(now_ms);
        flushed
    }

    /// Commit a letter once the paddles have been quiet for a letter-space.
    pub fn poll(&mut self, now_ms: u64, dit_ms: u64) -> Option<char> {
        let last = self.last_at_ms?;
        if self.pattern.is_empty() {
            return None;
        }
        let gap = now_ms.saturating_sub(last);
        if gap < dit_ms.saturating_mul(LETTER_GAP_DITS) {
            return None;
        }
        let ch = decode_morse_pattern(&self.pattern);
        self.pattern.clear();
        self.last_at_ms = None;
        ch
    }

    /// Commit whatever is in the pattern, ignoring the clock. Used after a
    /// letter-space sleep that already waited the right number of dits.
    pub fn take_letter(&mut self) -> Option<char> {
        if self.pattern.is_empty() {
            return None;
        }
        let ch = decode_morse_pattern(&self.pattern);
        self.pattern.clear();
        self.last_at_ms = None;
        ch
    }

    pub fn reset(&mut self) {
        self.pattern.clear();
        self.last_at_ms = None;
    }
}

/// Held-paddle state for an electronic keyer: squeeze and hold, elements
/// come out at the session WPM until you let go.
#[derive(Clone, Debug)]
pub struct PaddleKeyer {
    mode: KeyerMode,
    dit_held: bool,
    dah_held: bool,
    /// First paddle that went down in this hold, so an iambic squeeze starts
    /// with the side you closed first.
    lead: Option<Paddle>,
    /// Most recently pressed paddle that is still down. Ultimatic uses this.
    last_pressed: Option<Paddle>,
    last_sent: Option<Paddle>,
    sending: Option<Paddle>,
    dit_mem: bool,
    dah_mem: bool,
    straight_down_at: Option<u64>,
    decoder: PaddleDecoder,
}

impl Default for PaddleKeyer {
    fn default() -> Self {
        Self::with_mode(KeyerMode::default())
    }
}

impl PaddleKeyer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_mode(mode: KeyerMode) -> Self {
        Self {
            mode,
            dit_held: false,
            dah_held: false,
            lead: None,
            last_pressed: None,
            last_sent: None,
            sending: None,
            dit_mem: false,
            dah_mem: false,
            straight_down_at: None,
            decoder: PaddleDecoder::new(),
        }
    }

    pub fn mode(&self) -> KeyerMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: KeyerMode) {
        self.mode = mode;
        self.reset();
    }

    pub fn decoder_mut(&mut self) -> &mut PaddleDecoder {
        &mut self.decoder
    }

    pub fn any_held(&self) -> bool {
        self.dit_held || self.dah_held
    }

    fn held(&self, paddle: Paddle) -> bool {
        match paddle {
            Paddle::Dit => self.dit_held,
            Paddle::Dah => self.dah_held,
        }
    }

    fn set_held(&mut self, paddle: Paddle, on: bool) {
        match paddle {
            Paddle::Dit => self.dit_held = on,
            Paddle::Dah => self.dah_held = on,
        }
    }

    fn set_mem(&mut self, paddle: Paddle, on: bool) {
        match paddle {
            Paddle::Dit => self.dit_mem = on,
            Paddle::Dah => self.dah_mem = on,
        }
    }

    pub fn press(&mut self, paddle: Paddle) {
        self.press_at(paddle, 0);
    }

    pub fn press_at(&mut self, paddle: Paddle, now_ms: u64) {
        let was_up = !self.held(paddle);
        let was_any = self.any_held();
        if self.mode.is_straight() && !was_any {
            self.straight_down_at = Some(now_ms);
        }
        self.set_held(paddle, true);
        if !was_any {
            self.lead = Some(paddle);
        }
        self.last_pressed = Some(paddle);
        if self.mode.is_straight() || !was_up {
            return;
        }
        // A newly closed paddle during the opposite element is remembered once.
        // Iambic A only latches this edge; Iambic B also latches "already held"
        // at the start of the next element.
        if self.sending == Some(paddle.flipped()) {
            self.set_mem(paddle, true);
        }
    }

    pub fn release(&mut self, paddle: Paddle) {
        self.set_held(paddle, false);
        if self.last_pressed == Some(paddle) {
            self.last_pressed = if self.dit_held {
                Some(Paddle::Dit)
            } else if self.dah_held {
                Some(Paddle::Dah)
            } else {
                None
            };
        }
        if !self.any_held() {
            self.lead = None;
        }
    }

    /// Classify a completed straight-key hold. The contact must already be up.
    pub fn take_straight(&mut self, now_ms: u64, dit_ms: u64) -> Option<Paddle> {
        if !self.mode.is_straight() || self.any_held() {
            return None;
        }
        let start = self.straight_down_at.take()?;
        let duration = now_ms.saturating_sub(start);
        Some(if duration < dit_ms.saturating_mul(STRAIGHT_DAH_DITS) {
            Paddle::Dit
        } else {
            Paddle::Dah
        })
    }

    /// The element to send at the next slot. None means the paddles are up and
    /// no memory is waiting.
    pub fn next_element(&mut self) -> Option<Paddle> {
        if self.mode.is_straight() {
            return None;
        }
        let pick = if self.dit_held && self.dah_held {
            match self.mode {
                KeyerMode::Ultimatic => self.last_pressed.or(self.lead).unwrap_or(Paddle::Dit),
                KeyerMode::IambicA | KeyerMode::IambicB => self
                    .last_sent
                    .map(Paddle::flipped)
                    .or(self.lead)
                    .unwrap_or(Paddle::Dit),
                KeyerMode::Straight => return None,
            }
        } else if self.mode == KeyerMode::Ultimatic && self.dit_held && self.dah_mem {
            Paddle::Dah
        } else if self.mode == KeyerMode::Ultimatic && self.dah_held && self.dit_mem {
            Paddle::Dit
        } else if self.dit_held {
            Paddle::Dit
        } else if self.dah_held {
            Paddle::Dah
        } else if self.dit_mem {
            Paddle::Dit
        } else if self.dah_mem {
            Paddle::Dah
        } else {
            self.sending = None;
            return None;
        };
        self.set_mem(pick, false);
        self.last_sent = Some(pick);
        self.sending = Some(pick);
        if self.mode == KeyerMode::IambicB && self.held(pick.flipped()) {
            self.set_mem(pick.flipped(), true);
        }
        Some(pick)
    }

    pub fn reset(&mut self) {
        let mode = self.mode;
        *self = Self::with_mode(mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets_map_to_dit_and_dah() {
        assert_eq!(paddle_from_bracket('[', false), Some(Paddle::Dit));
        assert_eq!(paddle_from_bracket(']', false), Some(Paddle::Dah));
        assert_eq!(paddle_from_bracket('[', true), Some(Paddle::Dah));
        assert_eq!(paddle_from_bracket(']', true), Some(Paddle::Dit));
        assert_eq!(paddle_from_bracket('A', false), None);
    }

    #[test]
    fn k_commits_after_a_two_dit_pause() {
        let mut decoder = PaddleDecoder::new();
        let dit = 60;
        assert_eq!(decoder.hit(Paddle::Dah, 0, dit), None);
        assert_eq!(decoder.hit(Paddle::Dit, 60, dit), None);
        assert_eq!(decoder.hit(Paddle::Dah, 120, dit), None);
        assert_eq!(decoder.poll(120 + dit, dit), None);
        assert_eq!(decoder.poll(120 + dit * 2, dit), Some('K'));
        assert!(decoder.is_empty());
    }

    #[test]
    fn a_short_gap_stays_inside_the_letter() {
        let mut decoder = PaddleDecoder::new();
        let dit = 60;
        decoder.hit(Paddle::Dit, 0, dit);
        decoder.hit(Paddle::Dit, dit, dit);
        assert_eq!(decoder.poll(dit + dit - 1, dit), None);
        assert_eq!(decoder.poll(dit * 3, dit), Some('I'));
    }

    #[test]
    fn the_next_letter_flushes_the_one_before() {
        let mut decoder = PaddleDecoder::new();
        let dit = 50;
        decoder.hit(Paddle::Dah, 0, dit);
        assert_eq!(decoder.hit(Paddle::Dit, dit * 3, dit), Some('T'));
        assert_eq!(decoder.poll(dit * 5, dit), Some('E'));
    }

    #[test]
    fn extra_elements_that_leave_the_table_are_ignored() {
        let mut decoder = PaddleDecoder::new();
        let dit = 40;
        for i in 0..8 {
            decoder.hit(Paddle::Dit, i * dit, dit);
        }
        assert_eq!(decoder.poll(20 * dit, dit), Some('5'));
    }

    #[test]
    fn dit_ms_follows_paris() {
        assert_eq!(dit_ms_for_wpm(20.0), 60);
        assert_eq!(dit_ms_for_wpm(12.0), 100);
        assert!(dit_ms_for_wpm(0.0) >= 1);
    }

    #[test]
    fn take_letter_commits_without_waiting() {
        let mut decoder = PaddleDecoder::new();
        decoder.hit(Paddle::Dit, 0, 60);
        decoder.hit(Paddle::Dit, 60, 60);
        assert_eq!(decoder.take_letter(), Some('I'));
        assert!(decoder.is_empty());
    }

    #[test]
    fn holding_dah_keeps_producing_dahs() {
        let mut keyer = PaddleKeyer::new();
        keyer.press(Paddle::Dah);
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        keyer.release(Paddle::Dah);
        assert_eq!(keyer.next_element(), None);
    }

    #[test]
    fn squeezing_both_paddles_alternates() {
        let mut keyer = PaddleKeyer::new();
        keyer.press(Paddle::Dit);
        keyer.press(Paddle::Dah);
        assert_eq!(keyer.next_element(), Some(Paddle::Dit));
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        assert_eq!(keyer.next_element(), Some(Paddle::Dit));
    }

    #[test]
    fn iambic_a_stops_when_the_squeeze_is_released() {
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::IambicA);
        keyer.press(Paddle::Dah);
        keyer.press(Paddle::Dit);
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        keyer.release(Paddle::Dah);
        keyer.release(Paddle::Dit);
        assert_eq!(keyer.next_element(), None);
    }

    #[test]
    fn iambic_b_adds_the_opposite_after_a_squeeze_release() {
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::IambicB);
        keyer.press(Paddle::Dah);
        keyer.press(Paddle::Dit);
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        keyer.release(Paddle::Dah);
        keyer.release(Paddle::Dit);
        assert_eq!(keyer.next_element(), Some(Paddle::Dit));
        assert_eq!(keyer.next_element(), None);
    }

    #[test]
    fn iambic_a_remembers_a_tap_during_the_opposite_element() {
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::IambicA);
        keyer.press(Paddle::Dah);
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        keyer.press(Paddle::Dit);
        keyer.release(Paddle::Dit);
        keyer.release(Paddle::Dah);
        assert_eq!(keyer.next_element(), Some(Paddle::Dit));
        assert_eq!(keyer.next_element(), None);
    }

    #[test]
    fn ultimatic_repeats_the_last_paddle_instead_of_alternating() {
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::Ultimatic);
        keyer.press(Paddle::Dah);
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        keyer.press(Paddle::Dit);
        assert_eq!(keyer.next_element(), Some(Paddle::Dit));
        assert_eq!(keyer.next_element(), Some(Paddle::Dit));
        keyer.release(Paddle::Dit);
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        keyer.release(Paddle::Dah);
        assert_eq!(keyer.next_element(), None);
    }

    #[test]
    fn ultimatic_sends_x_by_squeezing_the_middle_dits() {
        // X is -..- : dah, squeeze dit for the two dits, release dit for the
        // last dah. Online tools that alternate on squeeze send C instead.
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::Ultimatic);
        let mut decoder = PaddleDecoder::new();
        keyer.press(Paddle::Dah);
        decoder.hit(keyer.next_element().unwrap(), 0, 60);
        keyer.press(Paddle::Dit);
        decoder.hit(keyer.next_element().unwrap(), 60, 60);
        decoder.hit(keyer.next_element().unwrap(), 120, 60);
        keyer.release(Paddle::Dit);
        decoder.hit(keyer.next_element().unwrap(), 180, 60);
        keyer.release(Paddle::Dah);
        assert_eq!(decoder.take_letter(), Some('X'));
    }

    #[test]
    fn ultimatic_sends_p_by_squeezing_the_middle_dahs() {
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::Ultimatic);
        let mut decoder = PaddleDecoder::new();
        keyer.press(Paddle::Dit);
        decoder.hit(keyer.next_element().unwrap(), 0, 60);
        keyer.press(Paddle::Dah);
        decoder.hit(keyer.next_element().unwrap(), 60, 60);
        decoder.hit(keyer.next_element().unwrap(), 120, 60);
        keyer.release(Paddle::Dah);
        decoder.hit(keyer.next_element().unwrap(), 180, 60);
        keyer.release(Paddle::Dit);
        assert_eq!(decoder.take_letter(), Some('P'));
    }

    #[test]
    fn ultimatic_a_tap_inserts_one_opposite_then_the_held_paddle_resumes() {
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::Ultimatic);
        keyer.press(Paddle::Dah);
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
        keyer.press(Paddle::Dit);
        keyer.release(Paddle::Dit);
        assert_eq!(keyer.next_element(), Some(Paddle::Dit));
        assert_eq!(keyer.next_element(), Some(Paddle::Dah));
    }

    #[test]
    fn a_short_straight_press_is_a_dit() {
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::Straight);
        keyer.press_at(Paddle::Dit, 0);
        keyer.release(Paddle::Dit);
        assert_eq!(keyer.take_straight(50, 60), Some(Paddle::Dit));
        assert_eq!(keyer.next_element(), None);
    }

    #[test]
    fn a_long_straight_press_is_a_dah() {
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::Straight);
        keyer.press_at(Paddle::Dah, 10);
        keyer.release(Paddle::Dah);
        assert_eq!(keyer.take_straight(200, 60), Some(Paddle::Dah));
    }

    #[test]
    fn either_bracket_is_the_straight_key_contact() {
        let mut keyer = PaddleKeyer::with_mode(KeyerMode::Straight);
        keyer.press_at(Paddle::Dit, 0);
        keyer.press_at(Paddle::Dah, 20);
        keyer.release(Paddle::Dit);
        assert!(keyer.any_held());
        assert_eq!(keyer.take_straight(200, 60), None);
        keyer.release(Paddle::Dah);
        assert_eq!(keyer.take_straight(200, 60), Some(Paddle::Dah));
    }
}
