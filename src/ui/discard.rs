//! Discard, which asks once more: what it throws away is kept nowhere.
//!
//! A session's answers and a run's QSOs go with it, so the first press only
//! asks. Asking, Keep it stands where Discard stood, so the second tap of a
//! double tap keeps what it would have thrown away; the Discard that does it
//! stands apart, lower down.

use dioxus::prelude::*;

use crate::ui::tour::bring_into_sight;
use crate::ui::widgets::{Icon, IconName, control_id, focus_script, keep_focus};

/// What a Discard throws away, as its buttons and its question name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discarded {
    /// A session of groups, and its answers.
    Session,
    /// A contest run, and its QSOs.
    Run,
}

impl Discarded {
    fn noun(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Run => "run",
        }
    }

    fn question(self) -> &'static str {
        match self {
            Self::Session => "Discard this session? Its answers are not kept",
            Self::Run => "Discard this run? Its QSOs are not kept",
        }
    }

    /// The id of the Discard that does it, once asked.
    pub fn confirm_id(self) -> String {
        control_id("btn", &format!("discard {}", self.noun()))
    }

    fn keep_id(self) -> String {
        control_id("btn", &format!("keep {}", self.noun()))
    }
}

/// Whether a row's Discard is asking, and whether it was asked from the
/// keyboard.
#[derive(Clone, Copy, PartialEq)]
pub struct Asking {
    open: Signal<bool>,
    by_key: Signal<bool>,
}

impl Asking {
    pub fn is_open(self) -> bool {
        (self.open)()
    }

    /// Ask no more: kept.
    pub fn close(mut self) {
        self.open.set(false);
    }
}

/// The row's asking. The question takes more room than the row it replaces,
/// and its Discard can land below the window, or under the guide: it is
/// brought into sight once the page has it. Asked from the keyboard, Keep it
/// takes the cursor, which the question took Discard from under: Enter again
/// keeps what Discard would have thrown away.
pub fn use_discard_ask(what: Discarded) -> Asking {
    let asking = Asking {
        open: use_signal(|| false),
        by_key: use_signal(|| false),
    };
    use_effect(move || {
        if (asking.open)() {
            bring_into_sight(&[what.confirm_id()]);
            if *asking.by_key.peek() {
                focus(what.keep_id());
            }
        }
    });
    asking
}

/// Give the button `id` the cursor.
fn focus(id: String) {
    let script = focus_script(&id);
    // Tests have no page to run it on: the harness looks for the button.
    #[cfg(test)]
    crate::testing::ask_focus(&id, &script);
    #[cfg(not(test))]
    spawn(async move {
        let _ = document::eval(&script).await;
    });
}

/// Discard as it first stands: it only asks. Pressed, it leaves the cursor
/// where it was.
#[component]
pub fn DiscardButton(asking: Asking) -> Element {
    let Asking {
        mut open,
        mut by_key,
    } = asking;
    rsx! {
        button {
            id: control_id("btn", "discard"),
            class: "btn btn-danger",
            onmousedown: move |e| {
                keep_focus(e);
                by_key.set(false);
            },
            onkeydown: move |e| {
                if e.key() == Key::Enter || e.key() == Key::Character(" ".into()) {
                    by_key.set(true);
                }
            },
            onclick: move |_| open.set(true),
            Icon { name: IconName::X }
            "Discard"
        }
    }
}

/// The question, the Discard that does it, and Keep it, where Discard stood.
/// Keep it leaves the cursor where it was.
#[component]
pub fn DiscardAsk(
    what: Discarded,
    on_discard: EventHandler<()>,
    on_keep: EventHandler<()>,
) -> Element {
    rsx! {
        p { class: "discard-ask", role: "alert", "{what.question()}" }
        button {
            id: what.confirm_id(),
            class: "btn btn-danger",
            onclick: move |_| on_discard.call(()),
            Icon { name: IconName::X }
            "Discard"
        }
        button {
            id: what.keep_id(),
            class: "btn btn-secondary",
            onmousedown: keep_focus,
            onclick: move |_| on_keep.call(()),
            "Keep it"
        }
    }
}
