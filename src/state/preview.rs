use cw_core::{ContestKind, Sender};

/// A group the stream has sent, and how fast it was sent.
#[derive(Clone, Debug, PartialEq)]
pub struct HeardGroup {
    pub text: String,
    /// The speed of the characters themselves.
    pub char_wpm: f64,
    /// The overall speed, spacing included; below `char_wpm` under Farnsworth.
    pub effective_wpm: f64,
}

impl HeardGroup {
    /// "24 WPM", or "24 WPM · 18 eff." when the gaps are stretched.
    pub fn speed_label(&self) -> String {
        let char_wpm = self.char_wpm.round();
        let effective_wpm = self.effective_wpm.round();
        if effective_wpm < char_wpm {
            format!("{char_wpm:.0} WPM · {effective_wpm:.0} eff.")
        } else {
            format!("{char_wpm:.0} WPM")
        }
    }
}

/// What Hear a QSO has played so far, for a screen to show as a dialogue.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialogue {
    /// The contest the QSOs are of.
    pub kind: ContestKind,
    /// Your station as they send it, where some of it stands in for yours.
    pub stand_in: Option<String>,
    /// How many QSOs it plays.
    pub qsos: u32,
    /// Each line heard whole so far, in the order they ended.
    pub lines: Vec<DialogueLine>,
}

impl Dialogue {
    /// The QSO it has reached: the one its newest line is part of.
    pub fn qso(&self) -> u32 {
        self.lines.last().map_or(1, |line| line.qso)
    }
}

/// One line of a dialogue: your message, or what callers sent at once, as
/// a pile-up's callers do.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogueLine {
    /// Who sent it: you, the first of the callers, or QRM.
    pub who: Sender,
    /// What was sent, a send each, in the order they ended.
    pub sends: Vec<String>,
    /// Which of the QSOs it is part of, counted from 1.
    pub qso: u32,
}

/// What the app is playing when no session is running.
///
/// These are the four things that can be true of the audio outside training,
/// and exactly one of them is: the band preview, the letter player and an
/// envelope sample all want the same hardware, so a second one starting means
/// the first has stopped.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Preview {
    /// Nothing is playing, or a session has the audio.
    #[default]
    Idle,
    /// The band-conditions preview, looping until stopped.
    Band,
    /// The Listen screen playing a character or the whole pool.
    Letters,
    /// Groups looping with no answers and no score — copy while you do something else.
    Stream,
    /// One short envelope sample, named so the chip that sent it can show it.
    Sample(String),
    /// A few QSOs of the chosen contest, worked by an operator who copies
    /// every character, for a learner to hear before working one.
    Qsos,
}

impl Preview {
    pub fn is_band(&self) -> bool {
        matches!(self, Self::Band)
    }

    pub fn is_letters(&self) -> bool {
        matches!(self, Self::Letters)
    }

    pub fn is_stream(&self) -> bool {
        matches!(self, Self::Stream)
    }

    pub fn is_qsos(&self) -> bool {
        matches!(self, Self::Qsos)
    }

    /// The sample now sounding, for the chip that is waiting to light up.
    pub fn sample(&self) -> Option<String> {
        match self {
            Self::Sample(text) => Some(text.clone()),
            _ => None,
        }
    }
}
