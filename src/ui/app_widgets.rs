//! Widgets that read a context the app provides, such as the paddles'
//! keyer. Those in [`crate::ui::widgets`] read none.

use dioxus::prelude::*;

use crate::ui::paddle::use_typed_field;
use crate::ui::widgets::{TextDraft, control_id};

/// A line of text typed in.
///
/// What is typed stays as typed until it is committed, on Enter or on
/// leaving the box, or by a button given its `draft`, and only then is it
/// kept as the setting keeps it ([`TextDraft`]). Each commit redraws the
/// box, so it shows what was kept even when that is what it held already.
///
/// The paddles key nothing into it, so the keys right of P type there
/// ([`crate::ui::paddle::TypedField`]): a call's `/`, or an unlock order's
/// `+`.
#[component]
pub fn TextField(
    label: &'static str,
    value: String,
    #[props(default)] placeholder: Option<&'static str>,
    /// The field's class: `field`, or a field of a width of its own.
    #[props(default = "field")]
    class: &'static str,
    on_commit: EventHandler<String>,
    /// Where something outside the field keeps what is typed in it, as
    /// Home's Start does; otherwise the field holds its own.
    #[props(default)]
    draft: Option<TextDraft>,
) -> Element {
    let own = TextDraft::use_draft();
    let draft = draft.unwrap_or(own);
    let shown = draft.shown(|| value);
    let id = control_id("field", label);
    let typed = use_typed_field();
    rsx! {
        div { class,
            label { r#for: "{id}", "{label}" }
            input {
                id,
                class: "group-text",
                value: "{shown}",
                placeholder,
                autocapitalize: "characters",
                autocomplete: "off",
                spellcheck: false,
                "data-typed": "true",
                onfocus: move |_| typed.focus(),
                onblur: move |_| typed.blur(),
                oninput: move |e| draft.hold(e.value()),
                onchange: move |e| {
                    draft.settle();
                    on_commit.call(e.value());
                },
            }
        }
    }
}
