//! The keyer on a clock: paddles in, sidetone and letters out.

use super::{
    KeyerMode, LETTER_GAP_DITS, Paddle, PaddleDecoder, PaddleKeyer, WORD_GAP_DITS, dit_ms_for_wpm,
};

/// What the keyer is doing between one instant and the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Nothing sounding and no letter open.
    Idle,
    /// The straight key's contact is closed. The element is as long as it
    /// stays that way.
    Contact,
    /// An element sounding, until `until`.
    Element { until: u64 },
    /// The space after an element, until `until`.
    Space { until: u64 },
    /// Quiet since `since`. A letter space ends the letter; a paddle closing
    /// before then carries it on.
    Gap { since: u64 },
    /// The letter ended, quiet still since `since`, where its gap began. A
    /// word space after its last element ends the word; a paddle closing
    /// before then starts its next letter.
    Word { since: u64 },
}

/// What the host has to do now that the keyer has moved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[must_use = "the sidetone, the letter and the next wake are the host's to act on"]
pub struct KeyerOutput {
    /// The sidetone goes on, or off, when it has changed.
    pub tone: Option<bool>,
    /// A letter that has just ended and spells a character.
    pub letter: Option<char>,
    /// A word space has run out after the last letter: the word is over,
    /// which a screen of fields takes as Space.
    pub space: bool,
    /// When the keyer next has something to do: call [`Keyer::tick`] then.
    pub wake_at: Option<u64>,
}

/// An electronic keyer, timed by whoever drives it.
///
/// Every call takes the instant it happens at, in milliseconds on any
/// monotonic clock, so the keyer never sleeps and never needs to be woken
/// early: the host acts on each [`KeyerOutput`] — sounds the tone, takes the
/// letter or the word's end — and calls [`Keyer::tick`] when `wake_at` comes
/// round. A tick that comes before then, or twice, changes nothing, so a
/// timer the host forgot to cancel is harmless. Each phase is timed from the
/// instant it really began, so a tick that comes late lengthens the element
/// that was sounding rather than eating into the space after it.
#[derive(Clone, Debug)]
pub struct Keyer {
    paddles: PaddleKeyer,
    decoder: PaddleDecoder,
    dit_ms: u64,
    phase: Phase,
}

impl Keyer {
    pub fn new(mode: KeyerMode, wpm: f64) -> Self {
        Self {
            paddles: PaddleKeyer::with_mode(mode),
            decoder: PaddleDecoder::default(),
            dit_ms: dit_ms_for_wpm(wpm),
            phase: Phase::Idle,
        }
    }

    pub fn mode(&self) -> KeyerMode {
        self.paddles.mode()
    }

    /// Key another way. Whatever was half keyed the old way is dropped.
    pub fn set_mode(&mut self, mode: KeyerMode) -> KeyerOutput {
        if mode == self.mode() {
            return KeyerOutput::default();
        }
        let out = self.stop();
        self.paddles.set_mode(mode);
        out
    }

    /// The speed the paddles send at, from the next element on.
    pub fn set_wpm(&mut self, wpm: f64) {
        self.dit_ms = dit_ms_for_wpm(wpm);
    }

    /// Whether a letter is under way: an element sounding, or keyed and not
    /// yet ended by its letter space. Waiting out a word space is not one.
    pub fn letter_open(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Word { .. })
    }

    /// Whether the sidetone is sounding.
    pub fn sounding(&self) -> bool {
        matches!(self.phase, Phase::Contact | Phase::Element { .. })
    }

    /// A paddle closed at `now`. An electronic keyer that is between letters,
    /// or waiting out a letter or word space, starts the element at once: a
    /// dit at 20 WPM is 60 ms, and waiting for the next tick to begin it
    /// would be the whole character feeling late.
    pub fn press(&mut self, paddle: Paddle, now: u64) -> KeyerOutput {
        let was_sounding = self.sounding();
        self.paddles.press(paddle, now);
        if self.mode().is_straight() {
            self.phase = Phase::Contact;
        } else if matches!(
            self.phase,
            Phase::Idle | Phase::Gap { .. } | Phase::Word { .. }
        ) {
            self.start_next(now);
        }
        self.output(was_sounding, None)
    }

    /// A paddle opened at `now`. An electronic keyer finishes the element it
    /// is on; a straight key's element ends here.
    pub fn release(&mut self, paddle: Paddle, now: u64) -> KeyerOutput {
        let was_sounding = self.sounding();
        self.paddles.release(paddle);
        if self.phase == Phase::Contact && !self.paddles.any_held() {
            if let Some(element) = self.paddles.take_straight(now, self.dit_ms) {
                self.decoder.push(element);
            }
            self.phase = Phase::Gap { since: now };
        }
        self.output(was_sounding, None)
    }

    /// The clock has reached `now`: end whatever was due to end by then.
    pub fn tick(&mut self, now: u64) -> KeyerOutput {
        let was_sounding = self.sounding();
        let mut letter = None;
        let mut space = false;
        match self.phase {
            Phase::Element { until } if now >= until => {
                self.phase = Phase::Space {
                    until: now.saturating_add(self.dit_ms),
                };
            }
            Phase::Space { until } if now >= until => self.start_next(now),
            Phase::Gap { since } if now >= since.saturating_add(self.letter_gap_ms()) => {
                letter = self.decoder.take_letter();
                // A letter begins a word; what spells nothing begins none.
                self.phase = if letter.is_some() {
                    Phase::Word { since }
                } else {
                    Phase::Idle
                };
            }
            Phase::Word { since } if now >= since.saturating_add(self.word_gap_ms()) => {
                space = true;
                self.phase = Phase::Idle;
            }
            _ => {}
        }
        KeyerOutput {
            space,
            ..self.output(was_sounding, letter)
        }
    }

    /// End the letter now, without waiting out its letter space: a letter
    /// keyed in full is decoded at once. One still being keyed — an element
    /// sounding, a paddle down, or an element owed — is left as it is.
    ///
    /// For a deadline. The answer window closing is the end of the letter,
    /// however recently its last element ended; and a key pressed is the end
    /// of its word too, so no word space follows the letter, nor one already
    /// ended.
    pub fn finish_letter(&mut self) -> KeyerOutput {
        let keyed_in_full = match self.phase {
            Phase::Gap { .. } => true,
            Phase::Space { .. } => !self.paddles.any_held() && !self.paddles.any_remembered(),
            Phase::Word { .. } => {
                self.phase = Phase::Idle;
                return self.output(false, None);
            }
            Phase::Idle | Phase::Contact | Phase::Element { .. } => false,
        };
        if !keyed_in_full {
            return self.output(self.sounding(), None);
        }
        self.phase = Phase::Idle;
        let letter = self.decoder.take_letter();
        self.output(false, letter)
    }

    /// Drop everything — the element sounding, the paddles held, the letter
    /// half keyed — and fall silent.
    pub fn stop(&mut self) -> KeyerOutput {
        let was_sounding = self.sounding();
        self.paddles.reset();
        self.decoder = PaddleDecoder::default();
        self.phase = Phase::Idle;
        self.output(was_sounding, None)
    }

    /// Send the next element now, or start the letter space when there is
    /// none to send.
    fn start_next(&mut self, now: u64) {
        self.phase = match self.paddles.next_element() {
            Some(paddle) => {
                self.decoder.push(paddle);
                Phase::Element {
                    until: now.saturating_add(self.dit_ms.saturating_mul(paddle.dits())),
                }
            }
            None => Phase::Gap { since: now },
        };
    }

    fn letter_gap_ms(&self) -> u64 {
        self.dit_ms.saturating_mul(LETTER_GAP_DITS)
    }

    /// From the start of a letter's gap to the end of its word: a word space
    /// of quiet after its last element, less the dit of space an electronic
    /// keyer keys after each element before the gap starts. A straight key
    /// keys none.
    fn word_gap_ms(&self) -> u64 {
        let keyed_space = u64::from(!self.mode().is_straight());
        self.dit_ms.saturating_mul(WORD_GAP_DITS - keyed_space)
    }

    fn output(&self, was_sounding: bool, letter: Option<char>) -> KeyerOutput {
        let sounding = self.sounding();
        KeyerOutput {
            tone: (sounding != was_sounding).then_some(sounding),
            letter,
            space: false,
            wake_at: match self.phase {
                Phase::Idle | Phase::Contact => None,
                Phase::Element { until } | Phase::Space { until } => Some(until),
                Phase::Gap { since } => Some(since.saturating_add(self.letter_gap_ms())),
                Phase::Word { since } => Some(since.saturating_add(self.word_gap_ms())),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Paddle::{Dah, Dit};

    /// 20 WPM: a dit is 60 ms.
    const WPM: f64 = 20.0;

    /// Drives a keyer the way the app does — ticking at every wake — and
    /// writes down what it heard: `"180 off"`, `"840 O"`, `"1080 space"`.
    struct Host {
        keyer: Keyer,
        wake: Option<u64>,
        log: Vec<String>,
    }

    impl Host {
        fn new(mode: KeyerMode) -> Self {
            Self {
                keyer: Keyer::new(mode, WPM),
                wake: None,
                log: Vec::new(),
            }
        }

        fn act(&mut self, now: u64, out: KeyerOutput) {
            if let Some(on) = out.tone {
                self.log
                    .push(format!("{now} {}", if on { "on" } else { "off" }));
            }
            if let Some(letter) = out.letter {
                self.log.push(format!("{now} {letter}"));
            }
            if out.space {
                self.log.push(format!("{now} space"));
            }
            self.wake = out.wake_at;
        }

        /// Run the clock up to `now`, ticking at each wake on the way.
        fn run_to(&mut self, now: u64) {
            while let Some(at) = self.wake.filter(|at| *at <= now) {
                let out = self.keyer.tick(at);
                self.act(at, out);
            }
        }

        fn press(&mut self, paddle: Paddle, now: u64) {
            self.run_to(now);
            let out = self.keyer.press(paddle, now);
            self.act(now, out);
        }

        fn release(&mut self, paddle: Paddle, now: u64) {
            self.run_to(now);
            let out = self.keyer.release(paddle, now);
            self.act(now, out);
        }

        fn heard(&mut self) -> Vec<String> {
            self.run_to(u64::MAX / 2);
            std::mem::take(&mut self.log)
        }
    }

    #[test]
    fn a_held_dah_paddle_keys_dahs_until_it_is_let_go() {
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dah, 0);
        // Let go during the third dah: it still finishes, then the letter
        // space after it ends the letter, and the word space the word.
        host.release(Dah, 500);
        assert_eq!(
            host.heard(),
            [
                "0 on",
                "180 off",
                "240 on",
                "420 off",
                "480 on",
                "660 off",
                "840 O",
                "1080 space"
            ]
        );
    }

    #[test]
    fn a_squeeze_alternates_from_the_paddle_closed_first() {
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dah, 0);
        host.press(Dit, 10);
        host.release(Dah, 370);
        host.release(Dit, 370);
        // Dah, dit, dah — the third already under way when both let go.
        assert_eq!(
            host.heard(),
            [
                "0 on",
                "180 off",
                "240 on",
                "300 off",
                "360 on",
                "540 off",
                "720 K",
                "960 space"
            ]
        );
    }

    #[test]
    fn a_paddle_closed_in_the_letter_space_carries_the_letter_on() {
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dit, 0);
        host.release(Dit, 30);
        // The dit ends at 60 and its space at 120; the letter would end at
        // 240. A second dit at 200 makes it an I.
        host.press(Dit, 200);
        host.release(Dit, 230);
        assert_eq!(
            host.heard(),
            ["0 on", "60 off", "200 on", "260 off", "440 I", "680 space"]
        );
    }

    /// A word space after a letter ends the word: seven dits of quiet after
    /// its last element, as the letter space's three end the letter. A
    /// paddle closed before then keys the word's next letter, and the word
    /// ends after that one instead. Waiting it out is no letter keyed.
    #[test]
    fn a_word_space_after_a_letter_ends_the_word() {
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dit, 0);
        host.release(Dit, 30);
        // The dit ends at 60: the letter at 60 + 3 × 60, the word at
        // 60 + 7 × 60.
        host.run_to(300);
        assert!(!host.keyer.letter_open(), "waiting out a word space");
        assert_eq!(host.wake, Some(480));
        assert_eq!(host.heard(), ["0 on", "60 off", "240 E", "480 space"]);

        // A paddle closed six dits after the dit: the same word's next
        // letter.
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dit, 0);
        host.release(Dit, 30);
        host.press(Dah, 420);
        host.release(Dah, 440);
        assert_eq!(
            host.heard(),
            [
                "0 on",
                "60 off",
                "240 E",
                "420 on",
                "600 off",
                "780 T",
                "1020 space"
            ]
        );
    }

    /// A key pressed in the word space ends the word: no space follows the
    /// letter. Nor does one follow what spells no letter at all.
    #[test]
    fn no_word_space_follows_a_finished_word_or_no_letter() {
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dit, 0);
        host.release(Dit, 30);
        host.run_to(300);
        let out = host.keyer.finish_letter();
        assert_eq!(out, KeyerOutput::default());
        host.act(300, out);
        assert_eq!(host.heard(), ["0 on", "60 off", "240 E"]);

        // Ten dits held: no character.
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dit, 0);
        host.release(Dit, 1_150);
        let heard = host.heard();
        assert_eq!(
            heard.last().map(String::as_str),
            Some("1140 off"),
            "{heard:?}"
        );
    }

    #[test]
    fn a_straight_key_is_timed_by_its_contact() {
        let mut host = Host::new(KeyerMode::Straight);
        host.press(Dit, 0);
        host.release(Dit, 50);
        host.press(Dah, 100);
        host.release(Dah, 300);
        // Short then long: A, once the contact has been open two dits, a dit
        // sooner than an electronic keyer's letter, as a straight key keys
        // no space after its elements; and the word over once it has been
        // open seven, a word space, as an electronic keyer's is.
        assert_eq!(
            host.heard(),
            ["0 on", "50 off", "100 on", "300 off", "420 A", "720 space"]
        );
    }

    /// A deadline ends a letter keyed in full at once, letter space or not,
    /// and the letter is not heard a second time when the space runs out.
    #[test]
    fn a_letter_keyed_in_full_can_be_finished_early() {
        // In the letter space: the dah ended at 180, its space at 240.
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dah, 0);
        host.release(Dah, 100);
        host.run_to(250);
        assert!(
            host.keyer.letter_open(),
            "keyed, its letter space not run out"
        );
        let out = host.keyer.finish_letter();
        assert!(!host.keyer.letter_open());
        assert_eq!(out.letter, Some('T'));
        assert_eq!(out.wake_at, None);
        host.act(250, out);
        assert_eq!(host.heard(), ["0 on", "180 off", "250 T"]);

        // In the space after its last element, with nothing more to send.
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dit, 0);
        host.release(Dit, 30);
        host.run_to(90);
        assert_eq!(host.keyer.finish_letter().letter, Some('E'));
    }

    /// A letter still being keyed is not cut short: what it would have been
    /// is still to come.
    #[test]
    fn a_letter_still_being_keyed_is_not_finished() {
        // An element sounding.
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dah, 0);
        host.run_to(100);
        assert_eq!(host.keyer.finish_letter().letter, None);
        host.release(Dah, 100);
        assert_eq!(host.heard(), ["0 on", "180 off", "360 T", "600 space"]);

        // A paddle still down, between elements.
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dit, 0);
        host.run_to(90);
        assert_eq!(host.keyer.finish_letter().letter, None);

        // An element owed: the other paddle tapped while the dah sounded.
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dah, 0);
        host.press(Dit, 50);
        host.release(Dit, 60);
        host.release(Dah, 100);
        host.run_to(200);
        assert_eq!(host.keyer.finish_letter().letter, None);
        let heard = host.heard();
        assert_eq!(heard[heard.len() - 2..], ["480 N", "720 space"]);

        // A straight key's contact still closed, and nothing keyed at all.
        let mut host = Host::new(KeyerMode::Straight);
        host.press(Dit, 0);
        assert!(host.keyer.letter_open());
        assert_eq!(host.keyer.finish_letter().letter, None);
        assert_eq!(
            Keyer::new(KeyerMode::IambicA, WPM).finish_letter(),
            KeyerOutput::default()
        );
    }

    #[test]
    fn early_or_repeated_ticks_change_nothing() {
        let mut keyer = Keyer::new(KeyerMode::IambicA, WPM);
        let first = keyer.press(Dit, 0);
        assert_eq!(first.wake_at, Some(60));
        for now in [0, 30, 59, 59] {
            assert_eq!(
                keyer.tick(now),
                KeyerOutput {
                    wake_at: Some(60),
                    ..KeyerOutput::default()
                }
            );
        }
        assert_eq!(keyer.tick(60).tone, Some(false));
        assert_eq!(keyer.tick(60).tone, None);
    }

    #[test]
    fn a_late_tick_lengthens_the_element_and_keeps_the_space_whole() {
        let mut keyer = Keyer::new(KeyerMode::IambicA, WPM);
        let _ = keyer.press(Dit, 0);
        let _ = keyer.release(Dit, 10);
        let late = keyer.tick(100);
        assert_eq!(late.tone, Some(false));
        assert_eq!(late.wake_at, Some(160), "a whole dit of space after it");
    }

    #[test]
    fn stopping_drops_the_half_keyed_letter_and_silences_the_tone() {
        let mut keyer = Keyer::new(KeyerMode::IambicA, WPM);
        let _ = keyer.press(Dah, 0);
        let stopped = keyer.stop();
        assert_eq!(
            stopped,
            KeyerOutput {
                tone: Some(false),
                ..KeyerOutput::default()
            }
        );
        // The timer for the dah that was sounding comes round to nothing.
        assert_eq!(keyer.tick(180), KeyerOutput::default());
        assert_eq!(keyer.tick(10_000), KeyerOutput::default());
        assert_eq!(keyer.stop(), KeyerOutput::default(), "already quiet");
    }

    #[test]
    fn a_new_mode_drops_what_the_old_one_had_keyed() {
        let mut host = Host::new(KeyerMode::IambicA);
        host.press(Dit, 0);
        host.release(Dit, 30);
        let out = host.keyer.set_mode(KeyerMode::Straight);
        host.act(40, out);
        assert_eq!(host.keyer.mode(), KeyerMode::Straight);
        assert_eq!(host.heard(), ["0 on", "40 off"]);
        assert_eq!(
            host.keyer.set_mode(KeyerMode::Straight),
            KeyerOutput::default()
        );
    }

    #[test]
    fn a_new_speed_times_the_next_element() {
        let mut keyer = Keyer::new(KeyerMode::IambicA, WPM);
        keyer.set_wpm(12.0);
        assert_eq!(keyer.press(Dit, 0).wake_at, Some(100));
    }
}
