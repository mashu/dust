//! Contest mode: runs against stations that answer your CQ, as a contester
//! works them. `docs/contest-mode.md` is the plan it is built from.
//!
//! - [`rules`]: each contest as one row of a table: what is sent, what is
//!   copied, who calls and how it scores.
//! - [`exchange`], [`parse`]: an exchange's fields, their canonical values and
//!   cut numbers, and how what you type is read.
//! - [`esm`]: what Enter sends next, as MorseRunner's Enter Sends Message.
//! - [`clock`], [`serial`]: the run's clock, and the serials callers send.
//! - [`check`], [`score`]: a logged QSO checked against what was sent, and a
//!   log's contest score and rates.
//! - [`record`]: how a run is kept in the history, so that every build, older
//!   or newer, reads it back as practice and keeps what it does not know.
//! - [`caller`]: the stations that answer, as MorseRunner's callers behave.
//! - [`turn`]: each thing you send and every reply to it, as one send.
//! - [`machine`]: the run itself, as a pure state machine.

#[cfg(test)]
pub(crate) mod bot;
mod caller;
mod check;
mod clock;
mod esm;
mod exchange;
mod machine;
mod parse;
mod record;
mod rules;
mod score;
mod serial;
mod turn;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::callsign::{CallSpec, GeneratedCall};
use crate::session::Answered;
use crate::settings::TrainingSettings;

pub use check::{QsoCheck, QsoError, QsoText, check_qso};
pub use clock::RunClock;
pub use esm::{EntryField, Esm, Fields, MessagePart, QsoState, esm, message_text};
pub use exchange::{Field, cut_serial, keyed_token, read_number, send_serial};
pub use machine::{
    AirView, ContestEffect, ContestEvent, ContestMachine, ContestPhase, ContestView, LogRow, OpKey,
    PauseWhy, RowStatus,
};
pub use parse::{ExchangeState, ReadField, Reading, normalize_call, parse_exchange};
pub(crate) use record::{CONTEST_MARK, GroupCopy};
pub use record::{ContestLog, ContestRun, QsoRecord, RunErrors, RunQso, RunSummary, StoredLog};
pub use rules::{ContestRules, Multiplier, PerSide, Scoring, Side};
pub use score::{ContestScore, Rates, ScoredQso, Tally, contest_score, rate_at};
pub use serial::{SerialFit, SerialModel, WPX_SERIALS};
pub use turn::{Segment, Sender, Turn, plan_turn};

/// Run time is kept in milliseconds; rates and contests are told in minutes
/// and hours.
const MS_PER_MINUTE: u64 = 60_000;
const MS_PER_HOUR: u64 = 60 * MS_PER_MINUTE;

/// The contests Dust knows, in the order the card lists them. Only those in
/// [`ContestKind::shipped`] can be run; the others join as their phase ships.
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

    /// Its rules, once its phase has shipped.
    pub fn rules(self) -> Option<&'static ContestRules> {
        rules::RULES.iter().find(|rules| rules.kind == self)
    }

    /// The contests that can be run: those with a row of rules.
    pub fn shipped() -> impl Iterator<Item = Self> {
        rules::RULES.iter().map(|rules| rules.kind)
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

/// Where a contest's callers come from, and where what was copied of their
/// calls goes back to, as [`crate::machine::GroupSource`] is for groups.
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
        let names: Vec<&str> = ContestKind::ALL.iter().map(|kind| kind.name()).collect();
        assert_eq!(
            names,
            ["sprint", "arrlDx", "wpx", "cqww", "fieldDay", "simpleQso"]
        );
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
        assert_eq!(ContestKind::from_name("cwt"), None);
    }

    #[test]
    fn only_wpx_has_shipped_so_far() {
        let shipped: Vec<ContestKind> = ContestKind::shipped().collect();
        assert_eq!(shipped, [ContestKind::Wpx]);
        for kind in ContestKind::ALL {
            assert_eq!(kind.rules().is_some(), shipped.contains(&kind), "{kind:?}");
            assert!(!kind.label().is_empty());
        }
        assert_eq!(ContestKind::default(), ContestKind::Wpx);
        assert_eq!(ContestKind::Wpx.label(), "CQ WPX");
    }
}
