//! Where a contest's callers come from.

use crate::callsign::{CallSpec, GeneratedCall};
use crate::session::Answered;
use crate::settings::TrainingSettings;

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
