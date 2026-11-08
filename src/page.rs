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
    use super::{control_id, focus_end_script, focus_script};

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
