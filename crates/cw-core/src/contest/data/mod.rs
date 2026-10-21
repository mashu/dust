//! The values callers' exchanges are drawn from: who they are and what they
//! send. Each file names where its values come from.
//!
//! - [`names`]: operators' given names, as they send them.
//! - [`reports`]: the signal reports of an everyday QSO.
//! - [`places`]: the US states and Canadian provinces and territories
//!   North American callers are in, the codes they send for them, and the
//!   ARRL DX contest's sides.
//! - [`zones`]: the CQ zones stations are in.
//! - [`power`]: the power DX stations send in the ARRL DX contest.

pub(crate) mod names;
pub(crate) mod places;
pub(super) mod power;
pub(super) mod reports;
pub(crate) mod zones;
