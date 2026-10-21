//! The values callers' exchanges are drawn from: who they are and what they
//! send. Each file names where its values come from.
//!
//! - [`names`]: operators' given names, as they send them.
//! - [`reports`]: the signal reports of an everyday QSO.
//! - [`places`]: the US states and Canadian provinces and territories
//!   North American callers are in, and the codes they send for them.

pub(crate) mod names;
pub(crate) mod places;
pub(super) mod reports;
