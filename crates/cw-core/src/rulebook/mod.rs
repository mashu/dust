//! What a contest is, apart from any run of it: the contests Dust knows, how
//! each is saved, and the tables a run reads.
//!
//! - [`kind`]: the contests Dust knows, and how a run of one is set up, each
//!   saved by its name.
//! - [`rules`]: each contest as one row of a table: what is sent, what is
//!   copied, who calls and how it scores.
//! - [`exchange`]: an exchange's fields, their canonical values and cut
//!   numbers.
//! - [`odds`]: how often callers do each thing they do now and then.
//! - [`serial`]: the serials callers send.
//! - [`data`]: the values callers' exchanges are drawn from.

pub(crate) mod data;
pub(crate) mod exchange;
mod kind;
pub(crate) mod odds;
pub(crate) mod rules;
pub(crate) mod serial;

pub use data::places::{home_country, place_name};
pub use data::sections::section_name;
pub use data::zones::home_continent;
pub use exchange::{
    Field, cut_power, cut_serial, cut_zone, keyed_token, read_number, send_power, send_report,
    send_serial, send_zone,
};
pub use kind::{CallerHabits, ContestKind, RunStyle, TuCall};
pub(crate) use kind::{MS_PER_HOUR, MS_PER_MINUTE, QRS_DROP, QRS_MORE, QRS_MOST};
pub use odds::{CallerOdds, HUNTER_DX_PERCENT, Odd, OddsCard};
pub use rules::{
    CallerStyles, ContestRules, FAREWELLS, Fill, Form, GREETING_HOUR, Multiplier, PerSide, Points,
    STYLE_EVEN, Scoring, Sender, Side, Sides, Styles, corner_shares, greeting, into_shape,
    shape_corners, shares_without, toggle_corner,
};
pub use serial::{SPRINT_SERIALS, SerialFit, SerialModel, WPX_SERIALS};
