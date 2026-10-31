//! What became of a session or run that ended other than by your hand
//! ([`Notice`]): up over the screen it left you on, until it is put away or
//! the next one starts, so that none is ever lost without a word.

use dioxus::prelude::*;

use crate::persist::{CutOff, SetAside};
use crate::state::{AppCtx, Notice};
use crate::time::local_date_string;
use crate::ui::format::{clock, counted, day_of, short_date};
use crate::ui::widgets::{Icon, IconName, control_id};

#[component]
pub fn NoticeBanner() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut notices = ctx.notices;
    let shown = notices.read();
    if shown.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "notice notice-up", role: "alert",
            Icon { name: IconName::Info }
            div { class: "notice-lines",
                for notice in shown.iter() {
                    p { "{notice_text(notice)}" }
                }
            }
            button {
                id: control_id("btn", "dismiss notice"),
                class: "icon-btn notice-close",
                title: "Dismiss",
                aria_label: "Dismiss",
                onclick: move |_| notices.set(Vec::new()),
                Icon { name: IconName::X }
            }
        }
    }
}

/// A notice in words.
fn notice_text(notice: &Notice) -> String {
    match notice {
        Notice::CutOff(CutOff::Marked(open)) if open.over => {
            // Over, and held only for what the store had yet to take.
            let count = open.unstored.len() + usize::from(open.so_far.is_some());
            if count == 1 {
                "Dust closed before your last session was saved. It is kept now.".into()
            } else {
                format!(
                    "Dust closed before {count} of your sessions were saved. They are kept now."
                )
            }
        }
        Notice::CutOff(CutOff::Marked(open)) => {
            let what = if open.run { "contest run" } else { "session" };
            let when = started(open.started_at)
                .map(|when| format!(" {when}"))
                .unwrap_or_default();
            let kept = match (open.so_far.is_some(), open.run) {
                (false, _) => ", so it was not kept.",
                (true, false) => ". The groups you had answered were kept.",
                (true, true) => ". The QSOs you had logged were kept.",
            };
            let before = match open.unstored.len() {
                0 => String::new(),
                1 => " The session before it, not saved yet, was kept too.".into(),
                count => format!(" The {count} sessions before it, not saved yet, were kept too."),
            };
            format!("Your {what}{when} was cut off: Dust closed while it ran{kept}{before}")
        }
        Notice::CutOff(CutOff::Unread) => {
            "Your last session was cut off: Dust closed while it ran, so it was not kept.".into()
        }
        Notice::History(problem) => problem.clone(),
        Notice::SetAside(set_aside) => set_aside_text(set_aside),
    }
}

/// What was set aside as it could not be read, and how to have it back:
/// from the copy, where the user can reach it, or else from a backup.
fn set_aside_text(set_aside: &SetAside) -> String {
    match set_aside {
        SetAside::Settings { kept } => {
            let back = match kept {
                Some(file) => format!(
                    "The file was kept as {file}: with Dust closed, mend it and put it back as settings.json, or restore a backup."
                ),
                None => "Restore a backup to put yours back.".into(),
            };
            format!("Your settings could not be read, so Dust opened with its defaults. {back}")
        }
        SetAside::History { kept } => {
            let back = match kept {
                Some(file) => format!(
                    "It was kept as {file}. Restore a backup, or that file once it is mended, to put your sessions back."
                ),
                None => "Restore a backup to put your sessions back.".into(),
            };
            format!(
                "Your history could not be read, so Dust set it aside and started a new one. {back}"
            )
        }
        SetAside::Sessions { count, kept } => {
            let (were, them) = if *count == 1 {
                ("it was", "it")
            } else {
                ("they were", "them")
            };
            let back = match kept {
                Some(file) => format!(
                    "{were} set aside as {file}. Restoring that file in a Dust that reads {them} puts {them} back."
                ),
                None => format!(
                    "{were} set aside. A backup restored in a Dust that reads {them} puts {them} back."
                ),
            };
            format!(
                "{} in your history could not be read, perhaps saved by a newer Dust, so {back}",
                counted(*count, "session")
            )
        }
    }
}

/// When something started at `at`: "started at 14:32" today, "started Fri
/// 16 Oct 2026 at 14:32" before.
fn started(at: u64) -> Option<String> {
    let (day, time) = (day_of(at)?, clock(at)?);
    Some(if day == local_date_string() {
        format!("started at {time}")
    } else {
        format!("started {} at {time}", short_date(&day))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::OpenSession;

    #[test]
    fn a_cut_off_session_says_when_it_started_and_that_it_was_not_kept() {
        let at = crate::time::now_ms();
        let text = notice_text(&Notice::CutOff(CutOff::Marked(OpenSession::starting(
            false, at,
        ))));
        let time = clock(at).expect("a time today");
        assert_eq!(
            text,
            format!(
                "Your session started at {time} was cut off: Dust closed while it ran, so it was not kept."
            )
        );
        let run = notice_text(&Notice::CutOff(CutOff::Marked(OpenSession::starting(
            true,
            at - 3 * 86_400_000,
        ))));
        assert!(run.starts_with("Your contest run started "), "{run}");
        assert!(run.contains(" at "), "{run}");
        assert_eq!(
            notice_text(&Notice::CutOff(CutOff::Unread)),
            "Your last session was cut off: Dust closed while it ran, so it was not kept."
        );
    }

    /// What was set aside says so, and how to have it back: from the copy,
    /// where it can be reached, or else from a backup.
    #[test]
    fn what_was_set_aside_says_how_to_have_it_back() {
        let text = |set_aside| notice_text(&Notice::SetAside(set_aside));
        assert_eq!(
            text(SetAside::Settings {
                kept: Some("/d/settings.unreadable-9.json".into())
            }),
            "Your settings could not be read, so Dust opened with its defaults. The file was kept as /d/settings.unreadable-9.json: with Dust closed, mend it and put it back as settings.json, or restore a backup."
        );
        assert_eq!(
            text(SetAside::History { kept: None }),
            "Your history could not be read, so Dust set it aside and started a new one. Restore a backup to put your sessions back."
        );
        assert_eq!(
            text(SetAside::Sessions {
                count: 3,
                kept: Some("/d/sessions.unreadable-9.json".into())
            }),
            "3 sessions in your history could not be read, perhaps saved by a newer Dust, so they were set aside as /d/sessions.unreadable-9.json. Restoring that file in a Dust that reads them puts them back."
        );
        assert_eq!(
            text(SetAside::Sessions {
                count: 1,
                kept: None
            }),
            "1 session in your history could not be read, perhaps saved by a newer Dust, so it was set aside. A backup restored in a Dust that reads it puts it back."
        );
    }

    /// One whose mark held what it had done by then says that was kept.
    #[test]
    fn a_cut_off_session_kept_so_far_says_so() {
        let at = crate::time::now_ms();
        let time = clock(at).expect("a time today");
        let kept = |run| {
            notice_text(&Notice::CutOff(CutOff::Marked(OpenSession {
                so_far: Some(Box::new(crate::persist::SoFar {
                    result: cw_core::SessionResult::empty_for_test(),
                    settings: None,
                })),
                ..OpenSession::starting(run, at)
            })))
        };
        assert_eq!(
            kept(false),
            format!(
                "Your session started at {time} was cut off: Dust closed while it ran. The groups you had answered were kept."
            )
        );
        assert_eq!(
            kept(true),
            format!(
                "Your contest run started at {time} was cut off: Dust closed while it ran. The QSOs you had logged were kept."
            )
        );
    }

    /// A mark that carried sessions the store had yet to take says they
    /// were kept: those finished before a session cut off, or, once its own
    /// was over, the sessions it was held for alone.
    #[test]
    fn a_cut_off_mark_says_what_it_carried_was_kept() {
        let at = crate::time::now_ms();
        let time = clock(at).expect("a time today");
        let session = cw_core::SessionResult::empty_for_test();
        let carrying = |count, over| {
            notice_text(&Notice::CutOff(CutOff::Marked(OpenSession {
                unstored: vec![session.clone(); count],
                over,
                ..OpenSession::starting(false, at)
            })))
        };
        assert_eq!(
            carrying(1, false),
            format!(
                "Your session started at {time} was cut off: Dust closed while it ran, so it was not kept. The session before it, not saved yet, was kept too."
            )
        );
        assert!(
            carrying(2, false)
                .ends_with(" The 2 sessions before it, not saved yet, were kept too."),
            "{}",
            carrying(2, false)
        );
        assert_eq!(
            carrying(1, true),
            "Dust closed before your last session was saved. It is kept now."
        );
        assert_eq!(
            carrying(3, true),
            "Dust closed before 3 of your sessions were saved. They are kept now."
        );
        let finished = notice_text(&Notice::CutOff(CutOff::Marked(OpenSession {
            so_far: Some(Box::new(crate::persist::SoFar {
                result: session.clone(),
                settings: None,
            })),
            over: true,
            ..OpenSession::starting(false, at)
        })));
        assert_eq!(
            finished,
            "Dust closed before your last session was saved. It is kept now."
        );
    }
}
