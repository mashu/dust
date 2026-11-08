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
mod esm;
mod machine;
mod parse;
mod place;
mod qrm;
mod record;
mod score;
mod turn;

pub(crate) use crate::rulebook::{data, exchange, odds, rules, serial};

use crate::callsign::{CallSpec, GeneratedCall};
use crate::rulebook::{MS_PER_HOUR, MS_PER_MINUTE};
use crate::session::Answered;
use crate::settings::TrainingSettings;

pub use bot::{HeardQsos, HeardTurn, hear_qsos};
#[cfg(test)]
pub(crate) use caller::{FLUTTER_DEPTH, FLUTTER_RATE_HZ, QRS_MOST};
pub use check::{QsoCheck, QsoError, QsoText, check_qso};
pub use clock::RunClock;
pub use esm::{
    EntryField, Esm, Fields, MessagePart, QsoState, esm, loggable, message_text, space_to_call,
};
pub use machine::{
    AirView, ContestEffect, ContestEvent, ContestMachine, ContestPhase, ContestView, LogRow, OpKey,
    RowStatus, ShownText,
};
pub use parse::{ExchangeState, ReadField, Reading, normalize_call, parse_exchange};
pub use place::filter_capacity;
#[cfg(test)]
pub(crate) use record::LAYOUT;
pub(crate) use record::tells_speed;
pub use record::{ContestRun, QsoRecord, RunErrors, RunQso, RunSummary, StoredLog};
pub use score::{ContestScore, MyStation, Placed, Rates, ScoredQso, Tally, contest_score, rate_at};
pub use turn::{Fade, MONITOR_LEVEL, Segment, Turn, plan_turn};

pub use crate::rulebook::{
    CallerHabits, CallerOdds, CallerStyles, ContestKind, ContestRules, FAREWELLS, Field, Fill,
    Form, GREETING_HOUR, Multiplier, Odd, OddsCard, PerSide, Points, RunStyle, SPRINT_SERIALS,
    STYLE_EVEN, Scoring, Sender, SerialFit, SerialModel, Side, Sides, Styles, TuCall, WPX_SERIALS,
    corner_shares, cut_power, cut_serial, cut_zone, greeting, home_continent, home_country,
    into_shape, keyed_token, place_name, read_number, section_name, send_power, send_report,
    send_serial, send_zone, shape_corners, shares_without, toggle_corner,
};
pub use crate::session::{ContestLog, PauseWhy};

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
