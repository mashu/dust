/// A stable element id for a control, built from the label it shows.
///
/// Every interactive control carries one: it is what a stylesheet, an anchor or
/// a test reaches for, and deriving it from the label keeps the two in step.
///
/// The prefix names the kind of control — `btn`, `slider`, `switch`, `seg` and
/// so on. One of them is load-bearing: `disclosure` marks a button whose only
/// job is to reveal more of the page. Screen-wide sweeps open every one of
/// those before they look for controls, so a panel that hides behind a toggle
/// is still tested.
pub fn control_id(prefix: &str, label: &str) -> String {
    let mut slug = String::new();
    let mut gap = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            if gap && !slug.is_empty() {
                slug.push('-');
            }
            slug.extend(ch.to_lowercase());
            gap = false;
        } else {
            gap = true;
        }
    }
    if slug.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}-{slug}")
    }
}

/// The script that gives the input called `id` the focus, unless it has it.
///
/// Every build runs what it is given to eval as the body of a function, so the
/// script may return early.
pub fn focus_script(id: &str) -> String {
    format!(
        r#"const input = document.getElementById("{id}");
if (!input || document.activeElement === input) {{
    return;
}}
input.focus({{ preventScroll: true }});"#
    )
}

/// The script that gives the input called `id` the focus, unless it has it,
/// with the cursor at the end of what it holds and nothing selected: a run's
/// field taken back to finish or put right.
pub fn focus_end_script(id: &str) -> String {
    format!(
        r#"{}
const end = input.value.length;
input.setSelectionRange(end, end);"#,
        focus_script(id)
    )
}

/// Where the cursor goes in a control given the focus ([`focus_by_id`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caret {
    /// Where the control has it ([`focus_script`]): a button, or a field as
    /// it was left.
    AsIs,
    /// At the end of what the field holds, nothing selected
    /// ([`focus_end_script`]).
    AtEnd,
}

impl Caret {
    /// The script that gives the control `id` the focus, unless it has it,
    /// with the cursor where this says.
    fn script(self, id: &str) -> String {
        match self {
            Self::AsIs => focus_script(id),
            Self::AtEnd => focus_end_script(id),
        }
    }
}

/// Give the control `id` the focus, unless it has it, with the cursor as
/// `caret` says, from a task of the calling component. The script looks
/// for the control at once, so it has to be drawn already.
pub fn focus_by_id(id: &str, caret: Caret) {
    let script = caret.script(id);
    // Tests have no page to run it on: the harness looks for the control.
    #[cfg(test)]
    crate::testing::ask_focus(id, &script);
    #[cfg(not(test))]
    dioxus::prelude::spawn(async move {
        let _ = dioxus::prelude::document::eval(&script).await;
    });
}

/// As [`focus_by_id`], from a task of the app's own rather than the calling
/// component's: for what is not a component, a run's timers and the tasks
/// on the root, which no component outlives.
pub fn focus_by_id_anywhere(id: &str, caret: Caret) {
    let script = caret.script(id);
    // Tests have no page to run it on: the harness looks for the control.
    #[cfg(test)]
    crate::testing::ask_focus(id, &script);
    #[cfg(not(test))]
    dioxus::core::spawn_forever(async move {
        let _ = dioxus::prelude::document::eval(&script).await;
    });
}

/// The script that brings the first control of `ids` the page has into
/// sight, clear of the guide's dock and with room for the ring it draws,
/// and leaves it where it is if it is in sight already: a move made by hand
/// points at the control next to it, and the page should not jump for
/// that.
fn scroll_script(ids: &[String]) -> String {
    let ids: Vec<String> = ids.iter().map(|id| format!("{id:?}")).collect();
    format!(
        r#"const target = [{}].map((id) => document.getElementById(id)).find((el) => el);
if (!target) {{
    return;
}}
// The ring the guide draws reaches 7px past the control.
const ring = 8;
// The dock covers the foot or the head of the window, whichever it is
// nearer: what is under it, and the nav under a dock at the foot, is out of
// sight, and the rest is clear.
const dock = document.getElementById("tour-dock");
const cover = dock ? dock.getBoundingClientRect() : null;
let top = 0;
let bottom = window.innerHeight;
if (cover !== null && cover.top - top > bottom - cover.bottom) {{
    bottom = cover.top;
}} else if (cover !== null) {{
    top = cover.bottom;
}}
const box = target.getBoundingClientRect();
if (box.top - ring >= top && box.bottom + ring <= bottom) {{
    return;
}}
// Into the middle of what is clear, or its head for a control taller than
// that. A guide docked at the foot covers the middle of a short window, so
// centring in the window would put the control behind it.
const by = box.height + 2 * ring > bottom - top
    ? box.top - ring - top
    : (box.top + box.bottom - top - bottom) / 2;
window.scrollBy({{ top: by, behavior: "smooth" }});"#,
        ids.join(", ")
    )
}

/// Bring the first control of `ids` the page draws into sight, clear of the
/// guide where it is open ([`scroll_script`]): what the guide points at, or
/// a control a press has just put in place of the one it pointed at.
pub(crate) fn bring_into_sight(ids: &[String]) {
    // Tests have no page to run it on.
    #[cfg(not(test))]
    {
        let script = scroll_script(ids);
        dioxus::prelude::spawn(async move {
            let _ = dioxus::prelude::document::eval(&script).await;
        });
    }
    #[cfg(test)]
    let _ = ids;
}

/// Scroll the page back to its top. The script outlives the component that
/// asked for it, which a press of Resume takes off the page.
pub(crate) fn scroll_to_top() {
    // Tests have no page to run it on: the harness counts the asking.
    #[cfg(test)]
    crate::testing::ask_scroll_to_top();
    #[cfg(not(test))]
    dioxus::core::spawn_forever(async {
        let _ = dioxus::prelude::document::eval("window.scrollTo(0, 0);").await;
    });
}

#[cfg(test)]
mod tests {
    use super::{control_id, focus_end_script, focus_script, scroll_script};

    /// The harness never mounts anything on a page, so the script is the one
    /// part of an input taking the focus a test can see.
    #[test]
    fn an_input_takes_the_focus_unless_it_has_it() {
        let script = focus_script("group-input-7");
        assert!(
            script.contains(r#"getElementById("group-input-7")"#),
            "{script}"
        );
        let has = script
            .find("document.activeElement === input")
            .expect("an input with the focus is left alone");
        let focus = script
            .find("input.focus({ preventScroll: true })")
            .expect("the input takes the focus where it is");
        assert!(has < focus, "{script}");
    }

    /// A run's field takes the focus the same way, then has the cursor put
    /// at the end of what it holds, nothing selected: once it has the
    /// focus, and never in a field that had it already.
    #[test]
    fn a_runs_field_takes_the_focus_at_its_end() {
        let script = focus_end_script("field-call");
        assert!(script.starts_with(&focus_script("field-call")), "{script}");
        let focus = script.find("input.focus(").expect("focused");
        let end = script
            .find("input.setSelectionRange(end, end)")
            .expect("the cursor at the end, nothing selected");
        assert!(focus < end, "{script}");
        assert!(script.contains("const end = input.value.length;"));
    }

    /// The harness never mounts anything on a page, so the script is the one
    /// part of bringing a control into sight a test can see.
    #[test]
    fn the_first_control_there_is_is_brought_into_sight_clear_of_the_dock() {
        let script = scroll_script(&["from-side-tone".to_string(), "to-side-tone".to_string()]);
        assert!(
            script.contains(r#"["from-side-tone", "to-side-tone"].map("#),
            "{script}"
        );
        assert!(
            script.contains(r#"document.getElementById("tour-dock")"#),
            "{script}"
        );
        // Under the dock is out of sight, whether it is docked at the foot
        // or the head of the window, and so is the ring past the control.
        let under = script
            .find("if (cover !== null && cover.top - top > bottom - cover.bottom)")
            .expect("a control under the dock is not in sight");
        let in_sight = script
            .find("if (box.top - ring >= top && box.bottom + ring <= bottom)")
            .expect("a control in sight is left where it is");
        let scroll = script
            .find("(box.top + box.bottom - top - bottom) / 2")
            .expect("it is brought to the middle of what is clear");
        assert!(under < in_sight && in_sight < scroll, "{script}");
        assert!(script.contains("window.scrollBy("), "{script}");
    }

    #[test]
    fn a_control_id_is_the_label_slugged_under_its_prefix() {
        assert_eq!(control_id("btn", "Start training"), "btn-start-training");
        assert_eq!(control_id("seg", "CW Academy"), "seg-cw-academy");
        assert_eq!(control_id("day", "2026-09-08"), "day-2026-09-08");
        assert_eq!(control_id("number", "Level (1–40)"), "number-level-1-40");
        // Punctuation and accents never reach the id.
        assert_eq!(control_id("field", "Min (°)"), "field-min");
        // A label with nothing usable in it leaves the prefix on its own.
        assert_eq!(control_id("btn", "—"), "btn");
        assert_eq!(control_id("btn", ""), "btn");
    }
}
