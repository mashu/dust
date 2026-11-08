//! Whoever else is calling, placed where the receiver lets you hear them.
//!
//! The only part of Morse timing that looks at the receiver: where a caller
//! can sit is decided by the filter you are listening through.

use super::station::StationVoice;

/// Another station calling over the top of the one you want.
#[derive(Clone, Debug, PartialEq)]
pub struct Interferer {
    pub text: String,
    pub voice: StationVoice,
    /// How late this station starts after the one you copy, in seconds;
    /// before it, when less than nothing. Nobody in a pile-up is
    /// synchronised, and a stagger is most of what makes one hard to pick
    /// apart: the one you want is no likelier than any other to be first.
    pub delay_sec: f64,
    /// Where its path is in the band's fading, in seconds of the fade past
    /// the station you copy's: each caller arrives over a path of its own,
    /// so they rise and fall apart, and which of them stands out changes as
    /// they do.
    pub fade_from_sec: f64,
}

/// Everything the receiver hears during one send: the station you are copying,
/// and whoever else is calling across it.
#[derive(Clone, Debug, PartialEq)]
pub struct Transmission {
    pub text: String,
    pub voice: StationVoice,
    pub others: Vec<Interferer>,
}

impl Transmission {
    /// One station, alone on the frequency.
    pub fn alone(text: impl Into<String>, voice: StationVoice) -> Self {
        Self {
            text: text.into(),
            voice,
            others: Vec::new(),
        }
    }
}
