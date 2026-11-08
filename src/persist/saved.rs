//! What became of a change made to the history, and what the user is
//! told of it ([`HistorySaved`]).

use super::record::{self, RecordId};

/// What became of a change made to the history by record
/// ([`record::HistoryChange`]).
///
/// Whatever was not stored stays in the app, and in the mark of the session
/// it came from ([`super::OpenSession::unstored`]), and is stored with the next
/// change the store takes: nothing is let go for want of room, but where a
/// browser's page storage keeps the history whole ([`HistorySaved::Squeezed`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistorySaved {
    /// Stored: `kept` is how many records the change stored.
    Stored { kept: usize },
    /// Stored, but the browser's page storage, where it keeps the history
    /// whole for want of a database ([`super::legacy::BlobHistory`]), had room for
    /// only the newest `kept` sessions: `trimmed` are the oldest, which it
    /// no longer holds. Records have no such limit. `runs` says the history
    /// held a contest run, whose log takes more room than a session of
    /// groups.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    Squeezed {
        kept: usize,
        runs: bool,
        trimmed: Vec<RecordId>,
    },
    /// Not stored. The history a browser keeps whole could not be read, nor
    /// moved out of the way, when it was last read, so it is kept as it is
    /// rather than written over.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    Held,
    /// Not stored: the store has no room left. The session is kept here,
    /// in the app, until it has.
    Full,
    /// Not stored: a newer Dust keeps the history here in a way this one
    /// would harm, so it writes nothing to it ([`record::NEWER_LAYOUT`]).
    Newer,
    /// Not stored, for the reason given.
    Failed(String),
}

impl HistorySaved {
    /// How many records the store took, when it took the change at all.
    pub fn kept(&self) -> Option<usize> {
        match self {
            Self::Stored { kept } | Self::Squeezed { kept, .. } => Some(*kept),
            Self::Held | Self::Full | Self::Newer | Self::Failed(_) => None,
        }
    }

    /// What to tell the user of a session kept, when there is anything to
    /// tell.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Stored { .. } => None,
            Self::Squeezed { kept, runs, .. } => {
                let why = if *runs {
                    " Long contest runs take more room."
                } else {
                    ""
                };
                Some(format!(
                    "This browser keeps your history in its small page storage, which has room for only your newest {kept} sessions; older ones were let go.{why}"
                ))
            }
            Self::Held => Some(
                "Your saved history could not be read, so it was left as it is: this session is kept here until it can be saved. Save a backup to keep it safe."
                    .into(),
            ),
            Self::Full => Some(
                if cfg!(feature = "web") {
                    "This browser has no room left for this site: this session is kept here until there is. Save a backup to keep it safe."
                } else {
                    "This device has no room left: this session is kept here until there is. Save a backup to keep it safe."
                }
                .into(),
            ),
            Self::Newer => Some(record::NEWER_LAYOUT.into()),
            Self::Failed(reason) => Some(format!(
                "This session could not be saved: {}. It is kept here, and saved as soon as it can be.",
                reason.trim_end_matches('.')
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What became of a session is said plainly: that it is kept here
    /// until it is stored, where it is not; and only a browser's page
    /// storage, by name, ever says older sessions were let go.
    #[test]
    fn what_became_of_a_history_is_said_plainly() {
        assert_eq!(HistorySaved::Stored { kept: 4 }.problem(), None);
        let squeezed = |runs| HistorySaved::Squeezed {
            kept: 40,
            runs,
            trimmed: Vec::new(),
        };
        assert_eq!(squeezed(false).kept(), Some(40));
        let told = squeezed(false).problem().expect("told");
        assert!(told.contains("page storage"), "{told}");
        assert!(
            told.contains("newest 40") && !told.contains("contest"),
            "{told}"
        );
        assert!(
            squeezed(true)
                .problem()
                .is_some_and(|text| text.ends_with("Long contest runs take more room."))
        );
        assert_eq!(HistorySaved::Held.kept(), None);
        let held = HistorySaved::Held.problem().expect("told");
        assert!(held.contains("could not be read"), "{held}");
        assert!(held.contains("kept here until it can be saved"), "{held}");
        let failed = HistorySaved::Failed("disk full.".into())
            .problem()
            .expect("told");
        assert_eq!(
            failed,
            "This session could not be saved: disk full. It is kept here, and saved as soon as it can be."
        );
        for kept_here in [HistorySaved::Full, HistorySaved::Held, HistorySaved::Newer] {
            let told = kept_here.problem().expect("told");
            assert!(
                !told.contains("let go") && !told.contains("newest"),
                "{told}"
            );
        }
        // A full store keeps the session in the app, and says so.
        assert_eq!(HistorySaved::Full.kept(), None);
        let full = HistorySaved::Full.problem().expect("told");
        assert!(full.contains("no room left"), "{full}");
        assert!(full.contains("kept here until there is"), "{full}");
        assert!(full.ends_with("Save a backup to keep it safe."), "{full}");
        assert_eq!(HistorySaved::Newer.kept(), None);
        assert_eq!(
            HistorySaved::Newer.problem().as_deref(),
            Some(record::NEWER_LAYOUT)
        );
    }
}
