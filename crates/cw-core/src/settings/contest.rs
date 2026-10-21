//! What Contest mode adds to the settings: which contest a run is, how long
//! it lasts, and your station. Everything else a run uses is shared with
//! group training.

use serde::{Deserialize, Serialize};

use crate::callsign::parse_callsign;
use crate::contest::ContestKind;

/// The longest run, in minutes, and where a run left open ends.
pub const CONTEST_MINUTES_MAX: u32 = 60;

/// The longest callsign kept: room for a host and a marker around a call,
/// `VP2E/DL1ABC/P` aside.
pub const MY_CALL_MAX: usize = 12;

/// The longest name kept: a given name as operators send it is short.
pub const MY_NAME_MAX: usize = 10;

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
    /// Your name, as you send it in a contest whose exchange holds one:
    /// letters only, upper case. Empty until you give one.
    pub my_name: String,
}

impl Default for ContestSettings {
    fn default() -> Self {
        Self {
            contest_kind: ContestKind::Wpx,
            contest_minutes: 10,
            my_call: String::new(),
            my_name: String::new(),
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

    /// Your callsign, as typed: kept as a callsign can be written, as
    /// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) keeps it.
    pub fn set_my_call(&mut self, typed: &str) {
        self.my_call = clean_call(typed);
    }

    /// Your name, as typed: kept as a name is sent, as
    /// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) keeps it.
    pub fn set_my_name(&mut self, typed: &str) {
        self.my_name = clean_name(typed);
    }

    /// What a run of these settings still needs before it can start, said
    /// for the screen; None once it has everything: a contest whose phase has
    /// shipped, a callsign Dust can read, and your name where the contest
    /// sends it.
    pub fn missing(&self) -> Option<String> {
        let kind = self.contest_kind;
        let Some(rules) = kind.rules() else {
            return Some(format!("{} is not ready yet", kind.label()));
        };
        if self.my_call.is_empty() {
            return Some("Add your callsign to start a contest".to_string());
        }
        if parse_callsign(&self.my_call).is_none() {
            return Some(format!("{} is not a callsign Dust can read", self.my_call));
        }
        if rules.sends_name() && self.my_name.is_empty() {
            return Some(format!("Add your name to start a {}", kind.label()));
        }
        None
    }
}

/// `text` as a name is sent: upper case letters, no longer than
/// [`MY_NAME_MAX`]. Anything else, spaces and accents among them, goes.
pub(super) fn clean_name(text: &str) -> String {
    text.chars()
        .map(|c| c.to_ascii_uppercase())
        .filter(char::is_ascii_uppercase)
        .take(MY_NAME_MAX)
        .collect()
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
        let mut settings = ContestSettings::default();
        settings.set_my_call(" dl1abc/p ");
        assert_eq!(settings.my_call, "DL1ABC/P");
    }

    /// A name is kept as it is sent: letters only, upper case, ten at most.
    #[test]
    fn a_name_is_kept_as_it_is_sent() {
        assert_eq!(clean_name("mat"), "MAT");
        assert_eq!(clean_name(" Jean-Luc "), "JEANLUC");
        assert_eq!(clean_name("Jürgen 2"), "JRGEN");
        assert_eq!(clean_name("Maximilianus"), "MAXIMILIAN");
        assert_eq!(clean_name("123 ?"), "");
        for text in ["Jean-Luc", "Maximilianus", "é1a"] {
            let once = clean_name(text);
            assert_eq!(clean_name(&once), once, "{text:?} settles in one pass");
            assert!(once.len() <= MY_NAME_MAX);
        }
        let mut settings = ContestSettings::default();
        settings.set_my_name(" bob ");
        assert_eq!(settings.my_name, "BOB");
    }

    /// A run starts only with a contest that has shipped, a call that reads
    /// as one, and your name where the contest sends it, and otherwise says
    /// what it is missing.
    #[test]
    fn a_run_says_what_it_needs_before_it_can_start() {
        let mut settings = ContestSettings::default();
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your callsign to start a contest")
        );
        settings.my_call = "DL1ABC".to_string();
        assert_eq!(settings.missing(), None);
        settings.my_call = "ABC".to_string();
        assert_eq!(
            settings.missing().as_deref(),
            Some("ABC is not a callsign Dust can read")
        );
        settings.my_call = "DL1ABC".to_string();
        settings.contest_kind = ContestKind::SimpleQso;
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your name to start a Simple QSO")
        );
        settings.my_name = "MAT".to_string();
        assert_eq!(settings.missing(), None);
        // WPX sends no name, so it needs none.
        settings.my_name.clear();
        settings.contest_kind = ContestKind::Wpx;
        assert_eq!(settings.missing(), None);
        settings.contest_kind = ContestKind::Sprint;
        assert_eq!(
            settings.missing().as_deref(),
            Some("NA Sprint is not ready yet")
        );
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
