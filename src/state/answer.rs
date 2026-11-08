use cw_core::{KeyLog, Keys, MonoMs, SessionEvent, answer_chars};
use dioxus::prelude::*;

/// The open answer box's text, on its way to the session.
///
/// The box tells the session what is typed once typing pauses, so a burst of
/// keys is one event rather than one per character. Until then the text waits
/// here, and the runtime hands it to the machine ahead of whatever comes next
/// — the answer window closing, a send ending, Enter, End session, Exit. So
/// nothing that moves the session on can overtake a keystroke, however close
/// the two land.
///
/// It keeps when each character went in, key by key, as the box is typed
/// into: the text waits out the pause, but recognition is timed to the keys,
/// and the moments travel with the text.
///
/// The box also has a say in what is still being entered and not in it yet:
/// a letter the paddles are keying. At a deadline it finishes what is keyed
/// in full; auto-confirm waits until nothing is under way.
#[derive(Clone, Copy, PartialEq)]
pub struct AnswerOutbox(CopyValue<Outbox>);

#[derive(Default)]
struct Outbox {
    /// Typed into the box for this group, and not handed over yet.
    waiting: Option<(usize, String)>,
    /// The group the box was last typed into, and when each of its
    /// characters went in.
    typing: Option<usize>,
    keys: KeyLog,
    entry: Option<Entry>,
}

/// What the box held when it was handed over: group `index`'s answer so far,
/// and its keys.
#[derive(Clone, Debug, PartialEq)]
pub struct TypedAnswer {
    pub index: usize,
    pub text: String,
    pub keys: Keys<MonoMs>,
}

impl From<TypedAnswer> for SessionEvent {
    fn from(typed: TypedAnswer) -> Self {
        SessionEvent::Input {
            index: typed.index,
            text: typed.text,
            keys: typed.keys,
        }
    }
}

/// What the box says about an entry still under way.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Entry {
    /// Puts into the box whatever is entered in full but not there yet.
    pub(crate) finish: Callback,
    /// Whether anything is still being entered.
    pub(crate) open: Callback<(), bool>,
}

impl AnswerOutbox {
    pub fn new() -> Self {
        Self(CopyValue::new(Outbox::default()))
    }

    /// Group `index`'s box now holds `text`, not handed over yet: what is
    /// new in it was keyed at `at`, or typed for you, untimed, at `None`.
    pub fn hold(mut self, index: usize, text: String, at: Option<MonoMs>) {
        let mut outbox = self.0.write();
        if outbox.typing != Some(index) {
            outbox.typing = Some(index);
            outbox.keys.clear();
        }
        outbox.keys.typed(answer_chars(&text), at);
        outbox.waiting = Some((index, text));
    }

    /// What group `index`'s box holds that is not handed over yet, if any.
    pub fn waiting_for(self, index: usize) -> Option<String> {
        let outbox = self.0.peek();
        let (at, text) = outbox.waiting.as_ref()?;
        (*at == index).then(|| text.clone())
    }

    /// Whatever is waiting, now handed over.
    pub fn take(mut self) -> Option<TypedAnswer> {
        let mut outbox = self.0.write();
        let (index, text) = outbox.waiting.take()?;
        let keys = outbox.keys.keys();
        Some(TypedAnswer { index, text, keys })
    }

    /// While the calling component is mounted, `finish` puts into the box
    /// whatever is entered in full but not there yet, and `open` says
    /// whether anything is still being entered.
    pub fn use_entry(
        self,
        mut finish: impl FnMut() + 'static,
        mut open: impl FnMut() -> bool + 'static,
    ) {
        let entry = Entry {
            finish: use_callback(move |()| finish()),
            open: use_callback(move |()| open()),
        };
        use_hook(move || self.set_entry(Some(entry)));
        use_drop(move || {
            // Only its own: a box that has already taken over stays.
            if self.0.peek().entry == Some(entry) {
                self.set_entry(None);
            }
        });
    }

    pub(crate) fn set_entry(mut self, entry: Option<Entry>) {
        self.0.write().entry = entry;
    }

    /// Have the box finish what is entered in full. It may hand the session
    /// an answer of its own on the way.
    pub(crate) fn finish(self) {
        let entry = self.0.peek().entry;
        if let Some(entry) = entry {
            entry.finish.call(());
        }
    }

    /// Whether anything is still being entered into the box.
    pub(crate) fn entry_open(self) -> bool {
        let entry = self.0.peek().entry;
        entry.is_some_and(|entry| entry.open.call(()))
    }

    /// A new session: nothing typed for the last one carries over.
    pub(crate) fn clear(mut self) {
        let mut outbox = self.0.write();
        outbox.waiting = None;
        outbox.typing = None;
        outbox.keys.clear();
    }
}
