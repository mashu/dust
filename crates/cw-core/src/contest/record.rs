//! A contest run as the history keeps it.
//!
//! A run is a [`crate::SessionResult`] like any other: each call and
//! exchange field copied is one of its groups, so every stat counts it. Its
//! log rides beside the groups, and is written so that no build loses the run.
//! A build that knows no contests reads it as a callsign session; one that
//! knows an older log layout keeps the newer log untouched.

use serde::{Deserialize, Serialize};

/// What a run's alphabet fingerprint starts with: `contest:wpx`. A build
/// older than contest mode drops the log on its next save but keeps the
/// fingerprint, so the mark is what still makes the run a run after that.
pub(crate) const CONTEST_MARK: &str = "contest:";

/// A run's log, exactly as it was read.
///
/// Kept as plain JSON rather than parsed into a struct, because a struct
/// that reads any layout leniently cannot write a newer layout back
/// unchanged: a field it does not know would be lost, a renamed one
/// defaulted. Held as a value, a log written by a later version goes back to
/// disk as it came, whatever this version makes of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContestLog(pub serde_json::Value);
