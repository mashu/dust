//! Contest mode: runs against stations that answer your CQ, as a contester
//! works them. `docs/contest-mode.md` is the plan it is built from.
//!
//! - [`rules`]: each contest as one row of a table: what is sent, what is
//!   copied, who calls and how it scores.
//! - [`exchange`], [`parse`]: an exchange's fields, their canonical values and
//!   cut numbers, and how what you type is read.
//! - [`esm`]: what Enter sends next (Enter Sends Message).
//! - [`clock`], [`serial`]: the run's clock, and the serials callers send.
//! - [`check`], [`score`]: a logged QSO checked against what was sent, and a
//!   log's contest score and rates.
//! - [`record`]: how a run is kept in the history, so that every build, older
//!   or newer, reads it back as practice and keeps what it does not know.
//! - [`caller`]: the stations that answer.
//! - [`place`]: where callers sit against the receiver, and how loud.
//! - [`qrm`]: under rough habits, stations on your frequency that are not
//!   calling you.
//! - [`turn`]: each thing you send and every reply to it, as one send, each
//!   caller fading on a path of its own.
//! - [`machine`]: the run itself, as a pure state machine.
//! - [`data`]: the values callers' exchanges are drawn from.
//! - [`bot`]: operators that work a run as a person would, and the QSOs
//!   Hear a QSO plays.

pub(crate) mod bot;
mod caller;
mod check;
mod clock;
pub(crate) mod data;
mod esm;
pub(crate) mod exchange;
mod machine;
mod parse;
mod place;
mod qrm;
mod record;
mod rules;
mod score;
mod serial;
mod turn;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::callsign::{CallSpec, GeneratedCall};
use crate::session::Answered;
use crate::settings::TrainingSettings;

pub use bot::{HeardQsos, HeardTurn, hear_qsos};
#[cfg(test)]
pub(crate) use caller::{FLUTTER_DEPTH, FLUTTER_RATE_HZ, QRS_MOST};
pub use check::{QsoCheck, QsoError, QsoText, check_qso};
pub use clock::RunClock;
pub use data::places::{home_country, place_name};
pub use data::sections::section_name;
pub use data::zones::home_continent;
pub use esm::{
    EntryField, Esm, Fields, MessagePart, QsoState, esm, loggable, message_text, space_to_call,
};
pub use exchange::{
    Field, cut_power, cut_serial, cut_zone, keyed_token, read_number, send_power, send_report,
    send_serial, send_zone,
};
pub use machine::{
    AirView, ContestEffect, ContestEvent, ContestMachine, ContestPhase, ContestView, LogRow, OpKey,
    PauseWhy, RowStatus, ShownText,
};
pub use parse::{ExchangeState, ReadField, Reading, normalize_call, parse_exchange};
pub use place::filter_capacity;
#[cfg(test)]
pub(crate) use record::LAYOUT;
pub(crate) use record::{CONTEST_MARK, GroupCopy};
pub use record::{ContestLog, ContestRun, QsoRecord, RunErrors, RunQso, RunSummary, StoredLog};
pub use rules::{ContestRules, Fill, Multiplier, PerSide, Points, Scoring, Side};
pub use score::{ContestScore, MyStation, Rates, ScoredQso, Tally, contest_score, rate_at};
pub use serial::{SPRINT_SERIALS, SerialFit, SerialModel, WPX_SERIALS};
pub use turn::{Fade, MONITOR_LEVEL, Segment, Sender, Turn, plan_turn};

/// Run time is kept in milliseconds; rates and contests are told in minutes
/// and hours.
const MS_PER_MINUTE: u64 = 60_000;
const MS_PER_HOUR: u64 = 60 * MS_PER_MINUTE;

/// The contests Dust knows, in the order the card lists them, each with its
/// row of rules ([`ContestKind::rules`]). A contest joins with its row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ContestKind {
    /// The NCJ North American Sprint.
    Sprint,
    /// The ARRL International DX contest.
    ArrlDx,
    /// CQ WPX, the default.
    #[default]
    Wpx,
    /// CQ World Wide DX.
    Cqww,
    /// ARRL Field Day.
    FieldDay,
    /// An everyday QSO, Dust's own: a first real one for a learner.
    SimpleQso,
}

impl ContestKind {
    pub const ALL: [Self; 6] = [
        Self::Sprint,
        Self::ArrlDx,
        Self::Wpx,
        Self::Cqww,
        Self::FieldDay,
        Self::SimpleQso,
    ];

    /// The name it is saved under.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sprint => "sprint",
            Self::ArrlDx => "arrlDx",
            Self::Wpx => "wpx",
            Self::Cqww => "cqww",
            Self::FieldDay => "fieldDay",
            Self::SimpleQso => "simpleQso",
        }
    }

    /// The contest saved as `name`, if this build knows it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// What the screens call it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Sprint => "NA Sprint",
            Self::ArrlDx => "ARRL DX",
            Self::Wpx => "CQ WPX",
            Self::Cqww => "CQ WW",
            Self::FieldDay => "Field Day",
            Self::SimpleQso => "Simple QSO",
        }
    }

    /// The article its label is said with: an NA Sprint, a Simple QSO.
    pub fn article(self) -> &'static str {
        match self {
            Self::Sprint | Self::ArrlDx => "an",
            _ => "a",
        }
    }

    /// Its rules: every contest on the card has its row, as a test checks.
    pub fn rules(self) -> &'static ContestRules {
        rules::RULES
            .iter()
            .find(|rules| rules.kind == self)
            .expect("every contest has its row of rules")
    }

    /// Whether a run of it has a contest score, as every contest but Simple
    /// QSO does, which is told by its pace and its copy instead.
    pub fn scored(self) -> bool {
        self.rules().scoring.is_some()
    }
}

/// How many callers answer at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RunStyle {
    /// One station answers each CQ.
    #[default]
    Single,
    /// Several call at once, from the Band card's stations range.
    Pileup,
}

impl RunStyle {
    pub const ALL: [Self; 2] = [Self::Single, Self::Pileup];

    /// The name it is saved under.
    pub fn name(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Pileup => "pileup",
        }
    }

    /// The style saved as `name`, if this build knows it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.name() == name)
    }
}

/// How the callers operate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CallerHabits {
    /// Plain digits, and no one makes a mistake: for learning.
    #[default]
    Easy,
    /// Numbers as MorseRunner's callers send them: padded, cut, `ENN`.
    Real,
    /// As real, with careless operators and on-frequency QRM.
    Rough,
}

impl CallerHabits {
    pub const ALL: [Self; 3] = [Self::Easy, Self::Real, Self::Rough];

    /// The name it is saved under.
    pub fn name(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Real => "real",
            Self::Rough => "rough",
        }
    }

    /// The habits saved as `name`, if this build knows them.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|habits| habits.name() == name)
    }
}

/// How often your TU signs with your call, with one caller at a time or a
/// pile-up, where the contest's TU does not sign by its own rules: every
/// QSO unless set otherwise, as a run's TU goes out on the air; every
/// third is MorseRunner's station ID rate (`StationIdRate`,
/// `Contest.pas:468-474`). A station tuning in hears who is running from
/// it; the callers call after your TU whether it signs or not, as
/// MorseRunner's do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TuCall {
    /// Every TU signs.
    #[default]
    Every,
    /// Every third QSO's TU signs.
    Third,
    /// No TU signs.
    Never,
}

impl TuCall {
    pub const ALL: [Self; 3] = [Self::Every, Self::Third, Self::Never];

    /// The name it is saved under.
    pub fn name(self) -> &'static str {
        match self {
            Self::Every => "every",
            Self::Third => "everyThird",
            Self::Never => "never",
        }
    }

    /// The choice saved as `name`, if this build knows it. `third` is a save
    /// from before every TU signed out of the box: every third was the
    /// default then, and saved as `third` whether chosen or not, so it reads
    /// as every QSO, once. Every third chosen now saves as `everyThird`.
    pub fn from_name(name: &str) -> Option<Self> {
        if name == "third" {
            return Some(Self::Every);
        }
        Self::ALL.into_iter().find(|choice| choice.name() == name)
    }

    /// Whether the TU that closes your `number`th QSO signs with your call.
    pub fn signs(self, number: u32) -> bool {
        match self {
            Self::Every => true,
            Self::Third => number.is_multiple_of(3),
            Self::Never => false,
        }
    }
}

// Each is saved by its name. Read back, anything this version does not know
// — a contest a later version added, or a save edited by hand — comes back as
// the default rather than costing the rest of the save, as the filter shape
// does.

impl Serialize for ContestKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for ContestKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(&name).unwrap_or_default())
    }
}

impl Serialize for RunStyle {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for RunStyle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(&name).unwrap_or_default())
    }
}

impl Serialize for CallerHabits {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for CallerHabits {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(&name).unwrap_or_default())
    }
}

impl Serialize for TuCall {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for TuCall {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(&name).unwrap_or_default())
    }
}

/// Where a contest's callers come from, and how fast they send where it
/// chooses, and where what was copied of their calls goes back to, as
/// [`crate::machine::GroupSource`] is for groups.
pub trait CallSource {
    /// The next caller's call, at the callsign tier `settings` hold, from the
    /// countries `spec` admits. A call `avoid` refuses — one already on the
    /// frequency or in the log — is drawn again, a few times at most.
    fn next_call(
        &mut self,
        settings: &TrainingSettings,
        spec: CallSpec,
        avoid: &dyn Fn(&str) -> bool,
    ) -> GeneratedCall;
    /// A call or field the user has just copied.
    fn learn(&mut self, answered: Answered<'_>);
    /// A call or a number the user has just copied, beside [`Self::learn`]:
    /// what says how fast they copy, as the copy-speed series reads a run.
    /// A field from a short list is half guessed, and says nothing of it.
    fn learn_speed(&mut self, _answered: Answered<'_>) {}
    /// The character and effective speed the next caller sends at, if the
    /// source chooses it. Otherwise the caller draws its own from the Speed
    /// card's ranges.
    fn next_speed(&mut self, _settings: &TrainingSettings) -> Option<(f64, f64)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_reads_back_as_itself() {
        for kind in ContestKind::ALL {
            let json = serde_json::to_value(kind).expect("serialize");
            assert_eq!(json, kind.name());
            assert_eq!(serde_json::from_value::<ContestKind>(json).ok(), Some(kind));
        }
        for style in RunStyle::ALL {
            let json = serde_json::to_value(style).expect("serialize");
            assert_eq!(serde_json::from_value::<RunStyle>(json).ok(), Some(style));
        }
        for habits in CallerHabits::ALL {
            let json = serde_json::to_value(habits).expect("serialize");
            assert_eq!(
                serde_json::from_value::<CallerHabits>(json).ok(),
                Some(habits)
            );
        }
        for choice in TuCall::ALL {
            let json = serde_json::to_value(choice).expect("serialize");
            assert_eq!(json, choice.name());
            assert_eq!(serde_json::from_value::<TuCall>(json).ok(), Some(choice));
        }
        let names: Vec<&str> = ContestKind::ALL.iter().map(|kind| kind.name()).collect();
        assert_eq!(
            names,
            ["sprint", "arrlDx", "wpx", "cqww", "fieldDay", "simpleQso"]
        );
    }

    /// Every contest scores but Simple QSO.
    #[test]
    fn every_contest_but_simple_qso_is_scored() {
        for kind in ContestKind::ALL {
            assert_eq!(kind.scored(), kind != ContestKind::SimpleQso, "{kind:?}");
        }
    }

    /// A name this build does not know is the default, not a failed save.
    #[test]
    fn an_unknown_name_reads_as_the_default() {
        let read = |name: &str| serde_json::from_value::<ContestKind>(name.into()).ok();
        for unknown in ["cwt", "", "WPX", "Sprint"] {
            assert_eq!(read(unknown), Some(ContestKind::Wpx), "{unknown:?}");
        }
        let style = serde_json::from_value::<RunStyle>("swarm".into()).ok();
        assert_eq!(style, Some(RunStyle::Single));
        let habits = serde_json::from_value::<CallerHabits>("lids".into()).ok();
        assert_eq!(habits, Some(CallerHabits::Easy));
        let tu = serde_json::from_value::<TuCall>("sometimes".into()).ok();
        assert_eq!(tu, Some(TuCall::Every));
        assert_eq!(ContestKind::from_name("cwt"), None);
    }

    /// Every TU signs, every third QSO's does, or none does.
    #[test]
    fn your_call_signs_the_tu_as_chosen() {
        let signed =
            |choice: TuCall| -> Vec<u32> { (1..=7).filter(|n| choice.signs(*n)).collect() };
        assert_eq!(signed(TuCall::Every), [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(signed(TuCall::Third), [3, 6]);
        assert!(signed(TuCall::Never).is_empty());
        assert_eq!(TuCall::default(), TuCall::Every);
    }

    /// Every third was the default until every TU signed, and saved as
    /// `third` whether chosen or not: a save from then reads as every QSO.
    /// Every third chosen now saves under a name of its own, and each
    /// choice reads back as it was saved.
    #[test]
    fn an_older_saves_every_third_reads_as_every_qso() {
        let read = |name: &str| serde_json::from_value::<TuCall>(name.into()).ok();
        assert_eq!(read("third"), Some(TuCall::Every));
        assert_eq!(TuCall::Third.name(), "everyThird");
        for choice in TuCall::ALL {
            let saved = serde_json::to_value(choice).expect("it saves");
            assert_eq!(serde_json::from_value::<TuCall>(saved).ok(), Some(choice));
        }
    }

    /// Every contest on the card has its row of rules, and one only.
    #[test]
    fn every_contest_has_its_rules() {
        assert_eq!(rules::RULES.len(), ContestKind::ALL.len());
        for kind in ContestKind::ALL {
            assert_eq!(kind.rules().kind, kind);
            assert!(!kind.label().is_empty());
        }
        assert_eq!(ContestKind::default(), ContestKind::Wpx);
        assert_eq!(ContestKind::Wpx.label(), "CQ WPX");
    }
}
