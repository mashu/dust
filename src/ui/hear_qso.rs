//! Hear a QSO, as the Character set card and the Listen screen show it: a
//! QSO line by line, You and They, and what Hear a QSO has played of the
//! chosen contest, each send once it has been heard whole.

use cw_core::Sender;
use dioxus::prelude::*;

use crate::playback::Dialogue;

/// A QSO line by line, who sends each named before it.
#[component]
pub(crate) fn QsoLines(lines: Vec<(Sender, String)>) -> Element {
    rsx! {
        ol { class: "qso",
            for (who, text) in lines.iter() {
                {qso_line(*who, std::slice::from_ref(text), false)}
            }
        }
    }
}

/// One line of a QSO: who sent it, and each send, callers calling at once
/// side by side. `next` sets it apart as the first of the next QSO.
fn qso_line(who: Sender, sends: &[String], next: bool) -> Element {
    let (class, said) = match (who, next) {
        (Sender::You, false) => ("qso-line you", "You"),
        (Sender::You, true) => ("qso-line you qso-next", "You"),
        (Sender::Caller(_), false) => ("qso-line they", "They"),
        (Sender::Caller(_), true) => ("qso-line they qso-next", "They"),
        // A station on your frequency that is not calling you: muted, and
        // never the start of a QSO.
        (Sender::Qrm, _) => ("qso-line qrm", "QRM"),
    };
    rsx! {
        li { class,
            span { class: "qso-who tiny", "{said}" }
            span { class: "qso-sends",
                for send in sends.iter() {
                    span { class: "call", "{send}" }
                }
            }
        }
    }
}

/// The eyebrow over a QSO, with the button that hears a few or stops them
/// on the same row, so the button stays put however many lines come.
#[component]
pub(crate) fn QsoHead(label: String, children: Element) -> Element {
    rsx! {
        div { class: "qso-head",
            div { class: "eyebrow", "{label}" }
            {children}
        }
    }
}

/// What the eyebrow over QSOs heard says: which one is playing, short
/// enough to keep Stop beside it on a phone, or that they have been heard.
pub(crate) fn heard_label(dialogue: &Dialogue, hearing: bool) -> String {
    if hearing {
        format!("QSO {} of {}", dialogue.qso(), dialogue.qsos)
    } else {
        "QSOs heard".to_string()
    }
}

/// What Hear a QSO has played so far, each QSO set apart from the one
/// before, and where your station stood in for yours. The lines scroll
/// within a box of their own that keeps the newest in view, so they never
/// push the page out from under the reader. Until the first send has been
/// heard whole there is nothing to show while `hearing` but that it is
/// coming.
#[component]
pub(crate) fn HeardDialogue(dialogue: Dialogue, hearing: bool) -> Element {
    rsx! {
        if dialogue.lines.is_empty() {
            if hearing {
                p { class: "muted qso-waiting", "Listening — each line shows once it has been sent." }
            }
        } else {
            div { class: "qso-scroll",
                ol { class: "qso",
                    for (at, line) in dialogue.lines.iter().enumerate() {
                        {
                            let next = at > 0 && dialogue.lines[at - 1].qso != line.qso;
                            qso_line(line.who, &line.sends, next)
                        }
                    }
                }
            }
        }
        if let Some(station) = dialogue.stand_in {
            p { id: "qso-stand-in", class: "muted qso-stand-in",
                "Your station is sent as {station} until you add yours."
            }
        }
        p { class: "muted qso-note", "Played for you to hear: not scored, not saved to History." }
    }
}
