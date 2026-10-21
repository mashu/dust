//! What Contest mode adds to the settings: which contest a run is, whether
//! one station calls at a time or a pile-up, how long a run lasts, and your
//! station. Everything else a run uses is shared with group training.

use serde::{Deserialize, Serialize};

use super::{STATIONS_MIN, TrainingSettings};
use crate::callsign::parse_callsign;
use crate::contest::data::places::{DX, in_us_or_canada, place_code};
use crate::contest::{ContestKind, RunStyle};

/// The longest run, in minutes, and where a run left open ends.
pub const CONTEST_MINUTES_MAX: u32 = 60;

/// The longest callsign kept: room for a host and a marker around a call,
/// `VP2E/DL1ABC/P` aside.
pub const MY_CALL_MAX: usize = 12;

/// The longest name kept: a given name as operators send it is short.
pub const MY_NAME_MAX: usize = 10;

/// The most callers at once a pile-up starts from, picked while the Band
/// card's stations range holds one station: one to three, as the design
/// gives a pile-up (§8).
const PILEUP_CALLERS_MAX: u32 = 3;

/// Your call where Hear a QSO plays one and none a contest can read is
/// given yet: a North American one for a contest that sends your state, as
/// its callers are there.
const STAND_IN_CALL: (&str, &str) = ("DL1ABC", "K1ABC");
/// Your name and state, where a contest sends them and none is given yet.
const STAND_IN_NAME: &str = "MIKE";
const STAND_IN_STATE: &str = "MA";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContestSettings {
    /// The contest a run is, chosen where the alphabet is in other modes.
    pub contest_kind: ContestKind,
    /// One caller at a time, or a pile-up of as many as the Band card's
    /// stations range draws.
    pub contest_style: RunStyle,
    /// How long a run lasts, in minutes. Zero runs until you stop it, and
    /// ends at [`CONTEST_MINUTES_MAX`] all the same.
    pub contest_minutes: u32,
    /// Your callsign: upper case, letters, digits and single slashes inside
    /// it. Empty until you give one.
    pub my_call: String,
    /// Your name, as you send it in a contest whose exchange holds one:
    /// letters only, upper case. Empty until you give one.
    pub my_name: String,
    /// Your US state, Canadian province or territory, by its code, or DX,
    /// as you send it in a contest whose exchange holds one. Empty until you
    /// give one, when a call outside the US and Canada sends DX
    /// ([`Self::state_sent`]).
    pub my_state: String,
}

impl Default for ContestSettings {
    fn default() -> Self {
        Self {
            contest_kind: ContestKind::Wpx,
            contest_style: RunStyle::Single,
            contest_minutes: 10,
            my_call: String::new(),
            my_name: String::new(),
            my_state: String::new(),
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

    /// Your state, province or territory, as typed: kept as its code, as
    /// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) keeps
    /// it, and nothing for what is none.
    pub fn set_my_state(&mut self, typed: &str) {
        self.my_state = clean_state(typed);
    }

    /// The state, province or territory your exchange sends: the one you
    /// gave, or else DX for a call Dust places outside the US and Canada,
    /// as Dust has every station there send (the real contests have one
    /// elsewhere in North America send its country). Empty while a call in
    /// the US or Canada, Alaska and Hawaii among them, has none, or one Dust
    /// cannot place, which may be there, or no call reads.
    pub fn state_sent(&self) -> &str {
        if !self.my_state.is_empty() {
            return &self.my_state;
        }
        if in_us_or_canada(&self.my_call) == Some(false) {
            DX
        } else {
            ""
        }
    }

    /// What a run of these settings still needs before it can start, said
    /// for the screen; None once it has everything: a contest whose phase has
    /// shipped, a callsign Dust can read, and your name and your state where
    /// the contest sends them.
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
        // The contest's name kept on one line: "an NA Sprint".
        let a_run = format!("{} {}", kind.article(), kind.label().replace(' ', "\u{a0}"));
        if rules.sends_name() && self.my_name.is_empty() {
            return Some(format!("Add your name to start {a_run}"));
        }
        if rules.sends_state() && self.state_sent().is_empty() {
            return Some(format!("Add your state or province to start {a_run}"));
        }
        None
    }

    /// Fill in with stand-ins what of your station a run still needs, as
    /// Hear a QSO plays it before you have given it, and say what your
    /// station is then, if any of it stands in: `DL1ABC`, `K1ABC MIKE MA`.
    /// It answers each thing [`Self::missing`] asks for, beside it so that
    /// the two change together; nothing for a contest not ready yet.
    pub(crate) fn stand_in(&mut self) -> Option<String> {
        let rules = self.contest_kind.rules()?;
        let mut stood_in = false;
        if parse_callsign(&self.my_call).is_none() {
            let (dx, home) = STAND_IN_CALL;
            self.my_call = if rules.sends_state() { home } else { dx }.to_string();
            stood_in = true;
        }
        if rules.sends_name() && self.my_name.is_empty() {
            self.my_name = STAND_IN_NAME.to_string();
            stood_in = true;
        }
        if rules.sends_state() && self.state_sent().is_empty() {
            self.my_state = STAND_IN_STATE.to_string();
            stood_in = true;
        }
        if !stood_in {
            return None;
        }
        let mut station = vec![self.my_call.clone()];
        if rules.sends_name() {
            station.push(self.my_name.clone());
        }
        if rules.sends_state() {
            station.push(self.state_sent().to_string());
        }
        Some(station.join(" "))
    }
}

impl TrainingSettings {
    /// Run contests in `style`. A pile-up of one caller is none at all, so
    /// picking one while the Band card's stations range holds a single
    /// station opens the range to one to three callers, as MorseRunner's
    /// pile-up starts busier than its single calls (`Ini.pas:261`); a range
    /// you have set is left as it is.
    pub fn set_contest_style(&mut self, style: RunStyle) {
        self.contest.contest_style = style;
        if style == RunStyle::Pileup && self.band.stations_max <= STATIONS_MIN {
            self.band.stations_max = PILEUP_CALLERS_MAX;
        }
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

/// What `text`, typed as your state, province or territory, reads as: its
/// code, upper case, an old one as the new (PQ is QC, NF is NL), or DX.
/// None for anything else, a name written out among it.
pub fn state_code(text: &str) -> Option<&'static str> {
    let letters: String = text
        .chars()
        .map(|c| c.to_ascii_uppercase())
        .filter(char::is_ascii_uppercase)
        .collect();
    place_code(&letters)
}

/// `text` as a state, province or territory is kept: its code
/// ([`state_code`]), or nothing.
pub(super) fn clean_state(text: &str) -> String {
    state_code(text).unwrap_or_default().to_string()
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

    /// Picking a pile-up on a stations range of one opens it to one to
    /// three callers; a range already set is kept, and so is the range when
    /// a single caller is picked again.
    #[test]
    fn a_pile_up_picked_on_a_range_of_one_opens_it() {
        let mut settings = TrainingSettings::default();
        settings.set_contest_style(RunStyle::Pileup);
        assert_eq!(settings.contest.contest_style, RunStyle::Pileup);
        assert_eq!(
            (settings.band.stations_min, settings.band.stations_max),
            (1, 3)
        );
        settings.set_contest_style(RunStyle::Single);
        assert_eq!(settings.contest.contest_style, RunStyle::Single);
        assert_eq!(
            (settings.band.stations_min, settings.band.stations_max),
            (1, 3)
        );
        settings.band.stations_min = 2;
        settings.band.stations_max = 2;
        settings.set_contest_style(RunStyle::Pileup);
        assert_eq!(
            (settings.band.stations_min, settings.band.stations_max),
            (2, 2)
        );
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

    /// A state is kept as its code: a valid one, an old one as the new, DX,
    /// or nothing.
    #[test]
    fn a_state_is_kept_as_its_code() {
        assert_eq!(clean_state("ma"), "MA");
        assert_eq!(clean_state(" o.r "), "OR");
        assert_eq!(clean_state("PQ"), "QC");
        assert_eq!(clean_state("nf"), "NL");
        assert_eq!(clean_state("dx"), "DX");
        for none in ["", "XX", "Mass", "LB", "ORE", "D X X"] {
            assert_eq!(clean_state(none), "", "{none:?}");
            assert_eq!(state_code(none), None, "{none:?}");
        }
        assert_eq!(state_code(" o.r "), Some("OR"));
        for text in ["ma", "PQ", "Mass", "dx"] {
            let once = clean_state(text);
            assert_eq!(clean_state(&once), once, "{text:?} settles in one pass");
        }
        let mut settings = ContestSettings::default();
        settings.set_my_state(" bc ");
        assert_eq!(settings.my_state, "BC");
        settings.set_my_state("Mass");
        assert_eq!(settings.my_state, "", "no state, rather than a wrong one");
    }

    /// Outside the US and Canada your state is DX until you give one; in
    /// them, Alaska and Hawaii too, it is what you give, as it is for a call
    /// Dust cannot place, which may be in them.
    #[test]
    fn your_state_is_dx_outside_the_us_and_canada() {
        let mut settings = ContestSettings::default();
        for (call, sent) in [
            ("DL1ABC", "DX"),
            ("VP2E/DL1ABC", "DX"),
            ("KP4AB", "DX"),
            ("KH2AB", "DX"),
            ("DL/K1ABC", "DX"),
            ("K1ABC", ""),
            ("VE3ABC", ""),
            ("VG3ABC", ""),
            ("CF3ABC", ""),
            ("KL7AB", ""),
            ("WL7AB", ""),
            ("KH6AB", ""),
            ("C31AB", ""),
            ("ABC", ""),
            ("", ""),
        ] {
            settings.my_call = call.to_string();
            assert_eq!(settings.state_sent(), sent, "{call:?}");
        }
        settings.my_call = "DL1ABC".to_string();
        settings.my_state = "MA".to_string();
        assert_eq!(settings.state_sent(), "MA", "what you give is sent");
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
            Some("Add your name to start a Simple\u{a0}QSO")
        );
        settings.my_name = "MAT".to_string();
        assert_eq!(settings.missing(), None);
        // WPX sends no name, so it needs none.
        settings.my_name.clear();
        settings.contest_kind = ContestKind::Wpx;
        assert_eq!(settings.missing(), None);
        // The Sprint sends your name and your state: a call in the US asks
        // for both, one outside it only for the name, its state being DX.
        settings.contest_kind = ContestKind::Sprint;
        settings.my_call = "K1ABC".to_string();
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your name to start an NA\u{a0}Sprint")
        );
        settings.my_name = "MIKE".to_string();
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your state or province to start an NA\u{a0}Sprint")
        );
        settings.my_state = "MA".to_string();
        assert_eq!(settings.missing(), None);
        settings.my_state.clear();
        settings.my_call = "DL1ABC".to_string();
        assert_eq!(settings.missing(), None, "DX, outside North America");
        settings.contest_kind = ContestKind::Cqww;
        assert_eq!(
            settings.missing().as_deref(),
            Some("CQ WW is not ready yet")
        );
    }

    /// Whatever of your station is given, the stand-ins leave nothing a
    /// run of a contest that has shipped still needs, and stand in only
    /// where it needed something.
    #[test]
    fn a_station_stood_in_for_has_all_a_run_needs() {
        for kind in ContestKind::shipped() {
            for call in ["", "ABC", "DL1ABC", "K1ABC", "VE3ABC/P"] {
                for name in ["", "ANN"] {
                    for state in ["", "OR"] {
                        let given = ContestSettings {
                            contest_kind: kind,
                            my_call: call.to_string(),
                            my_name: name.to_string(),
                            my_state: state.to_string(),
                            ..ContestSettings::default()
                        };
                        let mut stood = given.clone();
                        let stand_in = stood.stand_in();
                        let case = format!("{kind:?} {call:?} {name:?} {state:?}");
                        assert_eq!(stood.missing(), None, "{case}");
                        assert_eq!(stand_in.is_some(), given.missing().is_some(), "{case}");
                        if stand_in.is_none() {
                            assert_eq!(stood, given, "{case}");
                        }
                    }
                }
            }
        }
        let mut not_ready = ContestSettings {
            contest_kind: ContestKind::Cqww,
            ..ContestSettings::default()
        };
        assert_eq!(not_ready.stand_in(), None);
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
