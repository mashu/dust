//! Contest mode: runs against stations that answer your CQ, as a contester
//! works them. `docs/contest-mode.md` is the plan it is built from.
//!
//! - [`record`]: how a run is kept in the history, so that every build, older
//!   or newer, reads it back as practice and keeps what it does not know.

mod record;

pub(crate) use record::CONTEST_MARK;
pub use record::ContestLog;
