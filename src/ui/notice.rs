//! What became of a session or run that ended other than by your hand
//! ([`Notice`]): up over the screen it left you on, until it is put away or
//! the next one starts, so that none is ever lost without a word.

use dioxus::prelude::*;

use crate::persist::CutOff;
use crate::state::{AppCtx, Notice};
use crate::time::local_date_string;
use crate::ui::format::{clock, day_of, short_date};
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
        Notice::CutOff(CutOff::Marked(open)) => {
            let what = if open.run { "contest run" } else { "session" };
            let when = started(open.started_at)
                .map(|when| format!(" {when}"))
                .unwrap_or_default();
            format!("Your {what}{when} was cut off: Dust closed while it ran, so it was not kept.")
        }
        Notice::CutOff(CutOff::Unread) => {
            "Your last session was cut off: Dust closed while it ran, so it was not kept.".into()
        }
        Notice::SoundStopped { why, answered } => {
            let why = why.trim_end_matches('.');
            if *answered {
                format!(
                    "The session ended early because its sound stopped: {why}. The groups you answered were scored."
                )
            } else {
                format!(
                    "The session ended before any group was answered, because its sound stopped: {why}."
                )
            }
        }
        Notice::History(problem) => problem.clone(),
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
        let text = notice_text(&Notice::CutOff(CutOff::Marked(OpenSession {
            started_at: at,
            run: false,
        })));
        let time = clock(at).expect("a time today");
        assert_eq!(
            text,
            format!(
                "Your session started at {time} was cut off: Dust closed while it ran, so it was not kept."
            )
        );
        let run = notice_text(&Notice::CutOff(CutOff::Marked(OpenSession {
            started_at: at - 3 * 86_400_000,
            run: true,
        })));
        assert!(run.starts_with("Your contest run started "), "{run}");
        assert!(run.contains(" at "), "{run}");
        assert_eq!(
            notice_text(&Notice::CutOff(CutOff::Unread)),
            "Your last session was cut off: Dust closed while it ran, so it was not kept."
        );
    }

    #[test]
    fn a_session_its_sound_stopped_says_why_and_whether_any_was_answered() {
        let answered = notice_text(&Notice::SoundStopped {
            why: "Audio playback stalled.".into(),
            answered: true,
        });
        assert_eq!(
            answered,
            "The session ended early because its sound stopped: Audio playback stalled. The groups you answered were scored."
        );
        let none = notice_text(&Notice::SoundStopped {
            why: "Audio stream: device is gone".into(),
            answered: false,
        });
        assert_eq!(
            none,
            "The session ended before any group was answered, because its sound stopped: Audio stream: device is gone."
        );
    }
}
