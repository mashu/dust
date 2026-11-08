//! Contest mode: runs against stations that answer your CQ, as a contester
//! works them. `docs/contest-mode.md` is the plan it is built from.
//!
//! - [`crate::rulebook`]: the contests and their rules, exchange fields,
//!   caller odds, serials and data a run reads, re-exported here.
//! - [`parse`]: how what you type is read.
//! - [`esm`]: what Enter sends next (Enter Sends Message).
//! - [`message`]: the parts a message you send is made of.
//! - [`clock`]: the run's clock.
//! - [`check`], [`score`]: a logged QSO checked against what was sent, and a
//!   log's contest score and rates.
//! - [`record`]: how a run is kept in the history, so that every build, older
//!   or newer, reads it back as practice and keeps what it does not know.
//! - [`caller`]: the stations that answer.
//! - [`source`]: where the callers come from.
//! - [`place`]: where callers sit against the receiver, and how loud.
//! - [`qrm`]: under rough habits, stations on your frequency that are not
//!   calling you.
//! - [`turn`]: each thing you send and every reply to it, as one send, each
//!   caller fading on a path of its own.
//! - [`machine`]: the run itself, as a pure state machine.
//! - [`bot`]: operators that work a run as a person would, and the QSOs
//!   Hear a QSO plays.

pub(crate) mod bot;
mod caller;
mod check;
mod clock;
mod esm;
mod machine;
mod message;
mod parse;
mod place;
mod qrm;
mod record;
mod score;
mod source;
mod turn;

pub(crate) use crate::rulebook::{data, exchange, odds, rules, serial};

use crate::rulebook::{MS_PER_HOUR, MS_PER_MINUTE};

pub use bot::{HeardQsos, HeardTurn, hear_qsos};
#[cfg(test)]
pub(crate) use caller::{FLUTTER_DEPTH, FLUTTER_RATE_HZ, QRS_MOST};
pub use check::{QsoCheck, QsoError, QsoText, check_qso};
pub use clock::RunClock;
pub use esm::{EntryField, Esm, Fields, QsoState, esm, loggable, message_text, space_to_call};
pub use machine::{
    AirView, ContestEffect, ContestEvent, ContestMachine, ContestPhase, ContestView, LogRow, OpKey,
    RowStatus, ShownText,
};
pub use message::MessagePart;
pub use parse::{ExchangeState, ReadField, Reading, normalize_call, parse_exchange};
pub use place::filter_capacity;
#[cfg(test)]
pub(crate) use record::LAYOUT;
pub(crate) use record::tells_speed;
pub use record::{ContestRun, QsoRecord, RunErrors, RunQso, RunSummary, StoredLog};
pub use score::{ContestScore, MyStation, Placed, Rates, ScoredQso, Tally, contest_score, rate_at};
pub use source::CallSource;
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
