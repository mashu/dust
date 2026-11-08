//! What a store sets aside as it reads, as it cannot read it, for the user
//! to be told of ([`SetAside`]).

/// What a read set aside, as it could not read it, beside what it was
/// written over by ([`super::Store::take_set_aside`]). `kept` is where the copy is,
/// where the user can reach it: a file on a computer; nowhere they can on
/// a phone or in a browser, where it stays in the app's own store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetAside {
    /// The settings, whole: the app went on with its defaults.
    Settings { kept: Option<String> },
    /// The history, whole: the app went on without one.
    History { kept: Option<String> },
    /// `count` sessions of the history: the app went on without them.
    Sessions { count: usize, kept: Option<String> },
}
