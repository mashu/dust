//! A backup: the history and the settings in one file the user keeps,
//! outside wherever this build stores them, to put back after that store is
//! lost — a browser's data cleared — or on another device.
//!
//! Reading one is as forgiving as reading the store: a session that no longer
//! parses is left out, a setting that no longer reads falls back to its
//! default, and every session is scored the way sessions score today.

use std::collections::HashMap;

use cw_core::{SessionResult, TrainingSettings};
use serde::Serialize;

use super::{SessionKey, load_settings_from, parse_sessions};

/// The key that marks a file as a backup, and the version of its layout.
const MARK: &str = "dustBackup";
const LAYOUT: u32 = 1;

/// How many sessions may go unbacked before the History tab says so.
pub const BACKUP_DUE_AFTER: usize = 10;

/// The largest file read as a backup. Five hundred long sessions come to a
/// few megabytes.
pub const BACKUP_MAX_BYTES: u64 = 50_000_000;

/// What a backup holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Backup {
    /// When it was saved, in milliseconds since the epoch; zero if it does
    /// not say.
    pub saved_at: u64,
    pub settings: TrainingSettings,
    /// Oldest first.
    pub sessions: Vec<SessionResult>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Written<'a> {
    dust_backup: u32,
    saved_at: u64,
    settings: &'a TrainingSettings,
    sessions: &'a [SessionResult],
}

/// A backup of `settings` and `sessions`, saved at `saved_at`, as the file
/// holds it.
pub fn write_backup(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
    saved_at: u64,
) -> String {
    serde_json::to_string(&Written {
        dust_backup: LAYOUT,
        saved_at,
        settings,
        sessions,
    })
    .unwrap_or_default()
}

/// Why a file could not be restored from, in words that follow its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotRestored {
    /// It is not a backup: some other file, or not JSON at all.
    NotABackup,
    /// It says it is one, but its sessions cannot be read.
    Unreadable,
}

impl NotRestored {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::NotABackup => "is not a Dust backup",
            Self::Unreadable => "is a Dust backup whose sessions cannot be read",
        }
    }
}

/// Read a backup file.
pub fn read_backup(raw: &str) -> Result<Backup, NotRestored> {
    let Ok(serde_json::Value::Object(file)) = serde_json::from_str(raw) else {
        return Err(NotRestored::NotABackup);
    };
    if !file.get(MARK).is_some_and(serde_json::Value::is_u64) {
        return Err(NotRestored::NotABackup);
    }
    let settings = file
        .get("settings")
        .map_or_else(TrainingSettings::default, |settings| {
            load_settings_from(&settings.to_string())
        });
    let sessions = file
        .get("sessions")
        .and_then(|sessions| parse_sessions(&sessions.to_string()))
        .ok_or(NotRestored::Unreadable)?;
    Ok(Backup {
        saved_at: file
            .get("savedAt")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        settings,
        sessions,
    })
}

/// What a backup saved on `date` is called: "dust-backup-2026-10-19.json".
pub fn backup_name(date: &str) -> String {
    format!("dust-backup-{date}.json")
}

/// What a restore makes of the history ([`merge_sessions`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    /// The history with the backup's sessions in it, oldest first.
    pub sessions: Vec<SessionResult>,
    /// The sessions of the backup it added, as the history now holds them,
    /// oldest first.
    pub added: Vec<SessionResult>,
    /// The sessions of its own it filled in from the backup, oldest first.
    pub filled: Vec<Filled>,
}

/// A session a restore filled in: as it was, and as it is now.
#[derive(Clone, Debug, PartialEq)]
pub struct Filled {
    pub was: SessionResult,
    pub now: SessionResult,
}

/// Where a session of a restored history came from.
enum Came {
    Here,
    /// Here, and filled in from the backup: as it was before.
    Filled(Box<SessionResult>),
    Added,
}

/// `current` with every session of `restored` that it does not already
/// have, in the order they were practised; which those were, and which of
/// its own it filled in. A session is the same session if it finished at
/// the same moment on the same day ([`SessionKey`]). A contest run whose
/// log an older version dropped is filled in from a copy that kept it, and
/// so are the per-character records ([`cw_core::CharTimes`]) of the same
/// groups.
pub fn merge_sessions(current: &[SessionResult], restored: Vec<SessionResult>) -> Merged {
    let mut sessions: Vec<(SessionResult, Came)> = current
        .iter()
        .map(|session| (session.clone(), Came::Here))
        .collect();
    let mut here: HashMap<SessionKey, usize> = HashMap::new();
    for (at, (session, _)) in sessions.iter().enumerate() {
        here.entry(SessionKey::of(session)).or_insert(at);
    }
    let mut added = false;
    for session in restored {
        let key = SessionKey::of(&session);
        let Some(at) = here.get(&key).copied() else {
            here.insert(key, sessions.len());
            sessions.push((session, Came::Added));
            added = true;
            continue;
        };
        let (kept, came) = &mut sessions[at];
        let was = if kept.contest.is_none() && session.contest.is_some() {
            std::mem::replace(kept, session)
        } else if dropped_char_times(kept, &session) {
            let was = kept.clone();
            for (timing, restored) in kept.group_timings.iter_mut().zip(session.group_timings) {
                if timing.char_times.is_none() {
                    timing.char_times = restored.char_times;
                }
            }
            was
        } else {
            continue;
        };
        // One filled in twice, or added and then filled in, is still what
        // it was first.
        if let Came::Here = came {
            *came = Came::Filled(Box::new(was));
        }
    }
    if added {
        sessions.sort_by_key(|(session, _)| session.timestamp);
    }
    let mut merged = Merged {
        sessions: Vec::with_capacity(sessions.len()),
        added: Vec::new(),
        filled: Vec::new(),
    };
    for (session, came) in sessions {
        match came {
            Came::Here => {}
            Came::Filled(was) => merged.filled.push(Filled {
                was: *was,
                now: session.clone(),
            }),
            Came::Added => merged.added.push(session.clone()),
        }
        merged.sessions.push(session);
    }
    merged
}

/// Whether `restored`, the same groups as `kept`, holds per-character
/// records `kept` has not: an older version drops them.
fn dropped_char_times(kept: &SessionResult, restored: &SessionResult) -> bool {
    kept.groups == restored.groups
        && kept.group_timings.len() == restored.group_timings.len()
        && kept
            .group_timings
            .iter()
            .zip(&restored.group_timings)
            .any(|(kept, restored)| kept.char_times.is_none() && restored.char_times.is_some())
}

/// How many of `sessions` finished after the backup saved at `last`: all of
/// them, when there has been none.
pub fn sessions_since(sessions: &[SessionResult], last: Option<u64>) -> usize {
    match last {
        Some(at) => sessions
            .iter()
            .filter(|session| session.timestamp > at)
            .count(),
        None => sessions.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::tests::session;

    fn at(timestamp: u64, date: &str) -> SessionResult {
        SessionResult {
            timestamp,
            ..session(date)
        }
    }

    #[test]
    fn a_backup_reads_back_what_it_was_saved_with() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.level = 14;
        settings.playback.char_wpm_min = 22.0;
        let sessions = vec![at(1, "2026-10-01"), at(2, "2026-10-02")];
        let raw = write_backup(&settings, &sessions, 1_790_000_000_000);
        let backup = read_backup(&raw).expect("a backup");
        assert_eq!(backup.saved_at, 1_790_000_000_000);
        assert_eq!(backup.settings, settings.clamp());
        assert_eq!(backup.sessions.len(), 2);
        assert_eq!(backup.sessions[1].date, "2026-10-02");
    }

    #[test]
    fn only_a_backup_is_read_as_one() {
        assert_eq!(read_backup("not json"), Err(NotRestored::NotABackup));
        assert_eq!(read_backup("[]"), Err(NotRestored::NotABackup));
        // A settings file is JSON, but not a backup.
        let settings = serde_json::to_string(&TrainingSettings::default()).unwrap();
        assert_eq!(read_backup(&settings), Err(NotRestored::NotABackup));
        assert_eq!(
            read_backup(r#"{"dustBackup":1,"sessions":{}}"#),
            Err(NotRestored::Unreadable)
        );
        assert!(
            NotRestored::NotABackup
                .reason()
                .contains("not a Dust backup")
        );
    }

    /// One bad setting or session costs that one alone, as in the store.
    #[test]
    fn a_damaged_backup_gives_back_what_it_can() {
        let good = serde_json::to_string(&at(5, "2026-10-05")).unwrap();
        let raw = format!(
            r#"{{"dustBackup":1,"settings":{{"level":9,"groupTimeout":"nonsense"}},
                "sessions":[{good},{{"date":"broken"}}]}}"#
        );
        let backup = read_backup(&raw).expect("a backup");
        assert_eq!(backup.settings.curriculum.level, 9);
        assert_eq!(backup.sessions.len(), 1);
        assert_eq!(backup.saved_at, 0, "it does not say when");
        // A newer layout is still read for what it has.
        assert!(read_backup(r#"{"dustBackup":7,"sessions":[]}"#).is_ok());
    }

    #[test]
    fn restoring_adds_only_the_sessions_not_already_here() {
        let current = vec![at(10, "2026-10-10"), at(30, "2026-10-30")];
        let restored = vec![
            at(10, "2026-10-10"),
            at(20, "2026-10-20"),
            at(30, "2026-10-30"),
            at(5, "2026-10-05"),
        ];
        let merged = merge_sessions(&current, restored);
        assert_eq!(merged.added, [at(5, "2026-10-05"), at(20, "2026-10-20")]);
        assert!(merged.filled.is_empty());
        let merged = merged.sessions;
        assert_eq!(
            merged.iter().map(|s| s.timestamp).collect::<Vec<_>>(),
            [5, 10, 20, 30],
            "in the order they were practised"
        );
        assert_eq!(
            merge_sessions(&merged, merged.clone()),
            Merged {
                sessions: merged,
                added: Vec::new(),
                filled: Vec::new()
            },
            "restoring twice does nothing"
        );
    }

    /// A run whose log an older version dropped takes it back from a copy
    /// that kept it, where it sits; a run that has its log keeps it, and a
    /// copy without one takes nothing away.
    #[test]
    fn restoring_fills_in_a_runs_dropped_log() {
        let run = |timestamp: u64, log: bool| {
            let qso = [("K5XYZ", "214", "K5XYZ", "214")];
            let mut run = crate::testing::stored_run("2026-10-10", &qso);
            run.timestamp = timestamp;
            if !log {
                run.contest = None;
            }
            run
        };
        let current = vec![run(10, false), at(20, "2026-10-20"), run(30, true)];
        let restored = vec![run(10, true), run(30, false), run(40, true)];
        let Merged {
            sessions: merged,
            added,
            filled,
        } = merge_sessions(&current, restored);
        assert_eq!(added, [run(40, true)]);
        assert_eq!(
            filled,
            [Filled {
                was: run(10, false),
                now: run(10, true)
            }],
            "as it was, and as it is now"
        );
        assert_eq!(
            merged.iter().map(|s| s.timestamp).collect::<Vec<_>>(),
            [10, 20, 30, 40]
        );
        assert!(merged[0].contest.is_some(), "filled in");
        assert!(merged[2].contest.is_some(), "kept");
        assert_eq!(merged[0].char_set_mode, cw_core::CharSetMode::Contest);
        let again = merge_sessions(&merged, merged.clone()).filled;
        assert!(
            again.is_empty(),
            "a log already here is not filled in twice"
        );
        // A backup holding the run twice, its log dropped from the first:
        // added once, as it is filled in.
        let twice = merge_sessions(&[], vec![run(10, false), run(10, true)]);
        assert_eq!(twice.added, [run(10, true)]);
        assert!(twice.filled.is_empty());
    }

    /// A session whose per-character records an older version dropped takes
    /// them back from a copy that kept them; one that has them keeps its
    /// own.
    #[test]
    fn restoring_fills_in_dropped_character_times() {
        let times = |key: u32| cw_core::CharTimes {
            ends: vec![300],
            keys: vec![Some(key)],
            corrected: Vec::new(),
            latency_ms: 0,
            box_open: true,
        };
        let session = |kept: Option<u32>| {
            let mut session =
                crate::testing::stored_session("2026-10-10", &[("K", "K"), ("M", "M")]);
            session.timestamp = 10;
            session.group_timings[1].char_times = kept.map(times);
            session
        };
        let merged = merge_sessions(&[session(None)], vec![session(Some(700))]);
        assert_eq!((merged.added.len(), merged.filled.len()), (0, 1));
        assert_eq!(
            merged.sessions[0].group_timings[1].char_times,
            Some(times(700))
        );
        let merged = merge_sessions(&merged.sessions, vec![session(Some(900))]);
        assert!(merged.filled.is_empty(), "its own kept");
        assert_eq!(
            merged.sessions[0].group_timings[1].char_times,
            Some(times(700))
        );
    }

    /// An assisted run, `contest:wpx:a`, read back from a backup is still a
    /// WPX run, its log dropped or not, and is filled in as any run is.
    #[test]
    fn an_assisted_run_is_restored_as_its_contest() {
        let qso = [("K5XYZ", "214", "K5XYZ", "214")];
        let kept = crate::testing::assisted(crate::testing::stored_run("2026-10-10", &qso));
        // As an older version writes it back: its mode Callsigns and its
        // log dropped, its mark kept.
        let mut older = serde_json::to_value(&kept).expect("a run writes");
        if let Some(run) = older.as_object_mut() {
            run.remove("contest");
        }
        let raw = format!(r#"{{"dustBackup":1,"savedAt":0,"sessions":[{older}]}}"#);
        let backup = read_backup(&raw).expect("a backup");
        let read = &backup.sessions[0];
        assert_eq!(read.alphabet_fingerprint, "contest:wpx:a");
        assert_eq!(read.char_set_mode, cw_core::CharSetMode::Contest);
        assert_eq!(read.contest_kind(), Some(cw_core::ContestKind::Wpx));
        assert!(read.assisted(), "by its mark");
        let merged = merge_sessions(&backup.sessions, vec![kept.clone()]);
        assert_eq!((merged.added.len(), merged.filled.len()), (0, 1));
        assert_eq!(merged.sessions[0], kept);
    }

    #[test]
    fn sessions_since_a_backup_are_counted() {
        let sessions = vec![at(10, "a"), at(20, "b"), at(30, "c")];
        assert_eq!(sessions_since(&sessions, None), 3);
        assert_eq!(sessions_since(&sessions, Some(20)), 1);
        assert_eq!(sessions_since(&sessions, Some(99)), 0);
        assert_eq!(backup_name("2026-10-19"), "dust-backup-2026-10-19.json");
    }
}
