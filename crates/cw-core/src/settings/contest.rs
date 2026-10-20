//! What Contest mode adds to the settings: which contest a run is, how long
//! it lasts, and your station. Everything else a run uses is shared with
//! group training.

use serde::{Deserialize, Serialize};

use crate::contest::ContestKind;

/// The longest run, in minutes, and where a run left open ends.
pub const CONTEST_MINUTES_MAX: u32 = 60;

/// The longest callsign kept: room for a host and a marker around a call,
/// `VP2E/DL1ABC/P` aside.
pub const MY_CALL_MAX: usize = 12;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContestSettings {
    /// The contest a run is, chosen where the alphabet is in other modes.
    pub contest_kind: ContestKind,
    /// How long a run lasts, in minutes. Zero runs until you stop it, and
    /// ends at [`CONTEST_MINUTES_MAX`] all the same.
    pub contest_minutes: u32,
    /// Your callsign: upper case, letters, digits and single slashes inside
    /// it. Empty until you give one.
    pub my_call: String,
}

impl Default for ContestSettings {
    fn default() -> Self {
        Self {
            contest_kind: ContestKind::Wpx,
            contest_minutes: 10,
            my_call: String::new(),
        }
    }
}

impl ContestSettings {
    /// How long a run lasts at most, in minutes: what it was set to, or the
    /// longest for a run left open.
    pub fn limit_minutes(&self) -> u32 {
        match self.contest_minutes {
            0 => CONTEST_MINUTES_MAX,
            minutes => minutes.min(CONTEST_MINUTES_MAX),
        }
    }
}

/// `text` as a callsign can be written: upper case, letters, digits and
/// slashes, with no slash at either end or next to another, and no longer
/// than [`MY_CALL_MAX`].
pub(super) fn clean_call(text: &str) -> String {
    let mut call = String::new();
    for c in text.chars().map(|c| c.to_ascii_uppercase()) {
        let slash_inside = c == '/' && !call.is_empty() && !call.ends_with('/');
        if c.is_ascii_uppercase() || c.is_ascii_digit() || slash_inside {
            call.push(c);
        }
    }
    // Every character kept is ASCII, so this cuts between characters.
    call.truncate(MY_CALL_MAX);
    while call.ends_with('/') {
        call.pop();
    }
    call
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_kept_as_a_call_can_be_written() {
        assert_eq!(clean_call("dl1abc"), "DL1ABC");
        assert_eq!(clean_call(" dl/ w1aw "), "DL/W1AW");
        assert_eq!(clean_call("/W1AW//P/"), "W1AW/P");
        assert_eq!(clean_call("K5-XYZ!é"), "K5XYZ");
        assert_eq!(clean_call("VP2E/DL1ABC/P"), "VP2E/DL1ABC");
        assert_eq!(clean_call("ABCDEFGHIJK/L"), "ABCDEFGHIJK");
        assert_eq!(clean_call("ABCDEFGHIJKLMNOP"), "ABCDEFGHIJKL");
        assert_eq!(clean_call("///"), "");
        for text in ["/W1AW//P/", "ABCDEFGHIJK/L", "a/b/c/d/e/f/g/h", "é/1"] {
            let once = clean_call(text);
            assert_eq!(clean_call(&once), once, "{text:?} settles in one pass");
        }
    }

    #[test]
    fn an_open_run_ends_at_the_longest() {
        let mut settings = ContestSettings::default();
        assert_eq!(settings.limit_minutes(), 10);
        settings.contest_minutes = 0;
        assert_eq!(settings.limit_minutes(), CONTEST_MINUTES_MAX);
        settings.contest_minutes = 25;
        assert_eq!(settings.limit_minutes(), 25);
    }
}
