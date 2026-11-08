use cw_core::{
    ContestRules, EntryField, Esm, Field, Fields, MY_CALL_MAX, MonoMs, QsoState, Reading, Side,
    esm, loggable, normalize_call, parse_exchange, space_to_call,
};
use dioxus::prelude::*;

use crate::time::mono_ms;

/// The most the Call field takes, in characters: twice the longest callsign
/// kept ([`MY_CALL_MAX`]), which has room for a host and a marker about a
/// call already.
const CALL_LEN_MAX: usize = 2 * MY_CALL_MAX;

/// The most the Exchange field takes, in characters: the longest exchange
/// typed twice over, a name and a serial among it, with room to spare for
/// what is put right in it.
const EXCHANGE_LEN_MAX: usize = 64;

/// The most `field` takes, in characters: far more than a call or an
/// exchange is typed with, and far less than a page pasted in, which a run
/// would key on the air and keep in its log.
pub fn typed_len_max(field: EntryField) -> usize {
    match field {
        EntryField::Call => CALL_LEN_MAX,
        EntryField::Exchange => EXCHANGE_LEN_MAX,
    }
}

/// The run's Call and Exchange fields, as typed.
///
/// The fields keep what is typed into them, and the runtime reads it here
/// at each key ([`ContestEntry::snapshot`]). Nothing is drawn for a key: an
/// input drawn again while it is typed in drops the key typed in between, on
/// the desktop and phone builds, so the inputs are drawn only for what the
/// page has not seen — a letter the paddles keyed, the fields cleared — and
/// what reads the fields, the hint and Enter's label, follows `typed`.
#[derive(Clone, Copy, PartialEq)]
pub struct ContestEntry {
    /// Both fields, with when the call was last edited and when the
    /// exchange first read complete.
    fields: CopyValue<Fields>,
    /// Which field has the cursor. Only what says how to get back to the
    /// call reads it as it moves; the inputs never do.
    focus: Signal<EntryField>,
    /// What the fields are read by: the run's contest, from your side.
    reads: CopyValue<Option<(&'static ContestRules, Side)>>,
    /// Moved on by every edit. Only what reads the fields reads it.
    typed: Signal<u64>,
    /// Moved on only for what the inputs have not seen. Only they read it.
    pub(crate) repaint: Signal<u64>,
    /// Ends a paddle letter keyed in full, while the run screen is up.
    finish: CopyValue<Option<Callback>>,
}

impl ContestEntry {
    pub fn new() -> Self {
        Self {
            fields: CopyValue::new(Fields::default()),
            focus: Signal::new(EntryField::Call),
            reads: CopyValue::new(None),
            typed: Signal::new(0),
            repaint: Signal::new(0),
            finish: CopyValue::new(None),
        }
    }

    /// A new run, its fields read by `rules` from `side`: both empty, the
    /// cursor in Call.
    pub(crate) fn start(mut self, rules: &'static ContestRules, side: Side) {
        self.reads.set(Some((rules, side)));
        self.focus.set(EntryField::Call);
        self.clear();
    }

    /// What `field` holds, for its input to show. Reading it subscribes the
    /// caller to the rewrites the input has not seen, and to nothing else.
    pub fn shown(self, field: EntryField) -> String {
        let _ = self.repaint.read();
        self.text(field)
    }

    fn text(self, field: EntryField) -> String {
        let fields = self.fields.peek();
        match field {
            EntryField::Call => fields.call.clone(),
            EntryField::Exchange => fields.exch.clone(),
        }
    }

    /// A character the paddles or the keypad keyed into the field with the
    /// cursor at `keyed`. The page has not seen it, so that input is drawn
    /// again.
    pub fn push(mut self, letter: char, keyed: Option<MonoMs>) {
        let field = *self.focus.peek();
        let mut text = self.text(field);
        text.push(letter);
        self.edit(field, text, keyed);
        *self.repaint.write() += 1;
    }

    /// A space typed into the field with the cursor: it parts the
    /// exchange's items, and is no character of one.
    pub fn push_space(self) {
        self.push(' ', None);
    }

    /// The keypad's ⌫ took the last character out of the field with the
    /// cursor. The page has not seen it, so that input is drawn again.
    pub fn pop(mut self) {
        let field = *self.focus.peek();
        let mut text = self.text(field);
        if text.pop().is_some() {
            self.edit(field, text, None);
            *self.repaint.write() += 1;
        }
    }

    /// Both fields empty again, and their inputs drawn so.
    pub(crate) fn clear(mut self) {
        self.fields.set(Fields::cleared(MonoMs(mono_ms())));
        *self.typed.write() += 1;
        *self.repaint.write() += 1;
    }

    /// `field` now holds `text`, as its input does already, so nothing is
    /// drawn for it: what is new in it keyed at `keyed`, or untimed at
    /// `None`. Past what the field takes ([`typed_len_max`]) it is cut, and
    /// the field drawn as it is kept.
    pub fn edit(mut self, field: EntryField, mut text: String, keyed: Option<MonoMs>) {
        // Only a run has fields to type in, and it reads them from the
        // start.
        let Some((rules, side)) = *self.reads.peek() else {
            return;
        };
        let cut = text
            .char_indices()
            .nth(typed_len_max(field))
            .map(|(at, _)| at);
        if let Some(at) = cut {
            text.truncate(at);
        }
        self.fields
            .write()
            .edit(field, text, MonoMs(mono_ms()), keyed, rules, side);
        *self.typed.write() += 1;
        if cut.is_some() {
            *self.repaint.write() += 1;
        }
    }

    /// Which field has the cursor: where a paddle letter goes.
    pub fn focus(self) -> EntryField {
        *self.focus.peek()
    }

    /// The same, for what follows the cursor from field to field: the hint
    /// and the key that say how to get back to the call. Reading it
    /// subscribes the caller to the cursor moving.
    pub fn focused(self) -> EntryField {
        *self.focus.read()
    }

    /// `field` has the cursor: a tap, a click, or the run moving it.
    pub fn set_focus(mut self, field: EntryField) {
        if *self.focus.peek() != field {
            self.focus.set(field);
        }
    }

    /// Whether Space in the Exchange field takes the cursor back to Call,
    /// as the exchange typed stands ([`space_to_call`]), rather than typing
    /// the space before its next item.
    pub fn space_to_call(self) -> bool {
        self.reads
            .peek()
            .is_some_and(|(rules, side)| space_to_call(&self.fields.peek().exch, rules, side))
    }

    /// The fields as they are now, for a key to carry.
    pub(crate) fn snapshot(self) -> Fields {
        self.fields.peek().clone()
    }

    /// What Enter does with the QSO at `qso` and the fields as typed, for
    /// the hint and Enter's label. Reading it subscribes the caller to every
    /// edit. Nothing before a run has started.
    pub fn enter(self, qso: &QsoState) -> Option<Esm> {
        let _ = self.typed.read();
        let (rules, side) = (*self.reads.peek())?;
        Some(esm(qso, &self.fields.peek(), rules, side))
    }

    /// The Exchange field as the run reads it, for "Reads as". Reading it
    /// subscribes the caller to every edit. Nothing before a run has
    /// started.
    pub fn reading(self) -> Option<Reading> {
        let _ = self.typed.read();
        let (rules, side) = (*self.reads.peek())?;
        Some(parse_exchange(rules, side, &self.fields.peek().exch))
    }

    /// Whether the fields hold a QSO a key can log without Enter, for the
    /// keys that do. Reading it subscribes the caller to every edit.
    /// Nothing before a run has started.
    pub fn loggable(self) -> bool {
        let _ = self.typed.read();
        self.reads
            .peek()
            .is_some_and(|(rules, side)| loggable(&self.fields.peek(), rules, side))
    }

    /// The Call field as the run reads it, for what is said of the call
    /// typed: "B4 07:31". Reading it subscribes the caller to every edit.
    pub fn call(self) -> String {
        let _ = self.typed.read();
        normalize_call(&self.fields.peek().call)
    }

    /// What the run copies from each caller, for the Exchange field's label.
    pub fn copies(self) -> &'static [Field] {
        self.reads
            .peek()
            .map_or(&[], |(rules, side)| rules.copied(side))
    }

    /// While the calling component is mounted, `finish` ends a paddle letter
    /// keyed in full, so a key pressed before its letter space has run out
    /// carries it.
    pub fn use_letters(self, mut finish: impl FnMut() + 'static) {
        let finish = use_callback(move |()| finish());
        let mut held = self.finish;
        use_hook(move || held.set(Some(finish)));
        use_drop(move || {
            // Only its own: a screen that has already taken over stays.
            if *held.peek() == Some(finish) {
                held.set(None);
            }
        });
    }

    /// End a paddle letter keyed in full now, in the field it was keyed for,
    /// and the paddles' word with it, so no word space after it presses
    /// Space: before a key, and before anything typed, pressed or tapped by
    /// hand, which would otherwise come before the letter, or have the word
    /// space move the cursor a second time. Whether a letter landed.
    pub fn finish_letter(self) -> bool {
        let finish = *self.finish.peek();
        let typed = *self.typed.peek();
        if let Some(finish) = finish {
            finish.call(());
        }
        *self.typed.peek() != typed
    }
}
