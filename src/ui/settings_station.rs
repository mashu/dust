//! Your station: the callsign you sign in a contest, and the fields of the
//! exchange you send, given once: your name, for a contest that sends it.
//! The card shows in Contest mode only, under the character set whose
//! contest it serves, and Home asks for the call, and then the name, in the
//! same words while it has none.

use cw_core::{
    SettingsSection, TrainingSettings, call_fault, country_of, parse_callsign, wpx_prefix,
};
use dioxus::prelude::*;

use crate::ui::widgets::{CardHead, IconName, SectionReset, control_id};

/// What the callsign field and the name field are called, and so their ids.
const CALL_LABEL: &str = "Your callsign";
const NAME_LABEL: &str = "Your name";

/// Give the field called `label` the focus, as a start refused for want of
/// what it holds does.
fn focus_field(label: &'static str) {
    // Tests have no page to run it on.
    #[cfg(not(test))]
    spawn(async move {
        let script = crate::ui::widgets::focus_script(&control_id("field", label));
        let _ = document::eval(&script).await;
    });
    #[cfg(test)]
    let _ = label;
}

/// Give the callsign field the focus, as a start refused for want of a
/// call does.
pub(crate) fn focus_call_field() {
    focus_field(CALL_LABEL);
}

/// Give the name field the focus, as a start refused for want of a name
/// does.
pub(crate) fn focus_name_field() {
    focus_field(NAME_LABEL);
}

/// What a callsign reads as, in a line under it: where a contest places the
/// station, or why it is not a callsign yet. Nothing for no call at all.
///
/// The country is the table's key for it (`DL`), the one fact about a call
/// phase 1 holds besides its WPX prefix; names, continents, zones and your
/// side join the line as the contests that need them come.
fn call_reading(call: &str) -> Option<(String, bool)> {
    if call.is_empty() {
        return None;
    }
    if let Some(fault) = call_fault(call) {
        return Some((format!("Not a callsign yet: {fault}"), false));
    }
    let wpx = wpx_prefix(call).unwrap_or_default();
    Some(match country_of(call) {
        Some(country) => (format!("Reads as {country} · WPX prefix {wpx}"), true),
        None => (
            format!("Reads as WPX prefix {wpx} · unknown prefix, so no country"),
            true,
        ),
    })
}

/// Your callsign, typed in, kept as a call can be written: upper case,
/// letters, digits and single slashes.
#[component]
pub(crate) fn CallField(value: String, on_commit: EventHandler<String>) -> Element {
    rsx! {
        StationText { label: CALL_LABEL, placeholder: "e.g. DL1ABC", value, on_commit }
    }
}

/// Your name, typed in, kept as a name is sent: letters only, upper case.
#[component]
pub(crate) fn NameField(value: String, on_commit: EventHandler<String>) -> Element {
    rsx! {
        StationText { label: NAME_LABEL, placeholder: "e.g. MAT", value, on_commit }
    }
}

/// One of your station's fields, typed in.
///
/// What is typed stays as typed until it is committed, on Enter or on
/// leaving the box, and only then is it kept as the setting keeps it. A key
/// does not redraw the box, which on the desktop and phone builds would
/// reach it a frame late, over what was typed since; each commit does, so
/// the box shows what was kept even when that is what it held already.
#[component]
fn StationText(
    label: &'static str,
    placeholder: &'static str,
    value: String,
    on_commit: EventHandler<String>,
) -> Element {
    let mut typed = use_hook(|| CopyValue::new(None::<String>));
    let mut repaint = use_signal(|| 0u64);
    let _ = repaint.read();
    let shown = typed.peek().clone().unwrap_or(value);
    let id = control_id("field", label);
    rsx! {
        div { class: "field call-field",
            label { r#for: "{id}", "{label}" }
            input {
                id,
                class: "group-text",
                value: "{shown}",
                placeholder,
                autocapitalize: "characters",
                autocomplete: "off",
                spellcheck: false,
                oninput: move |e| typed.set(Some(e.value())),
                onchange: move |e| {
                    typed.set(None);
                    repaint += 1;
                    on_commit.call(e.value());
                },
            }
        }
    }
}

/// The line under a callsign: what it reads as, or why it does not read;
/// with no call, `unset`, if there is something to say then.
#[component]
pub(crate) fn CallReading(call: String, #[props(default)] unset: Option<String>) -> Element {
    let Some((line, read)) = call_reading(&call).or(unset.map(|line| (line, true))) else {
        return rsx! {};
    };
    rsx! {
        p {
            id: control_id("reading", "your callsign"),
            class: if read { "call-reading" } else { "call-reading bad" },
            aria_live: "polite",
            "{line}"
        }
    }
}

/// What a name still missing says, under the name field: what a run is
/// missing, in the words Home says it in, once the call is not.
#[component]
fn NameReading(missing: Option<String>) -> Element {
    let Some(line) = missing else {
        return rsx! {};
    };
    rsx! {
        p {
            id: control_id("reading", "your name"),
            class: "call-reading",
            aria_live: "polite",
            "{line}"
        }
    }
}

/// What Your station shows: the call, whether a contest can read it, your
/// name where the contest sends it, and what a run is missing.
#[derive(Clone, PartialEq)]
struct Station {
    my_call: String,
    readable: bool,
    /// Your name, where the contest sends one; none where it does not.
    my_name: Option<String>,
    missing: Option<String>,
}

#[component]
pub fn StationCard(settings: Signal<TrainingSettings>) -> Element {
    let station = use_memo(move || {
        let contest = &settings.read().contest;
        let sends_name = contest
            .contest_kind
            .rules()
            .is_some_and(|rules| rules.sends_name());
        Station {
            readable: parse_callsign(&contest.my_call).is_some(),
            my_call: contest.my_call.clone(),
            my_name: sends_name.then(|| contest.my_name.clone()),
            missing: contest.missing(),
        }
    });
    let s = station.read();
    let note = match (&s.my_name, s.readable) {
        (Some(name), true) if !name.is_empty() => format!("Signing {} · {name}", s.my_call),
        (_, true) => format!("Signing {}", s.my_call),
        (Some(_), false) => "Your callsign and name, for the contests you run".to_string(),
        (None, false) => "Your callsign, for the contests you run".to_string(),
    };
    // Said under the call while it is missing, and under the name once only
    // the name is.
    let name_missing = s
        .my_name
        .as_ref()
        .filter(|name| name.is_empty() && s.readable)
        .and(s.missing.clone());
    rsx! {
        div { class: "card stack-sm",
            CardHead { icon: IconName::Flag, title: "Your station", note,
                SectionReset {
                    label: "your station",
                    on_reset: move |()| settings.write().reset_section(SettingsSection::Station),
                }
            }
            // WPX sends nothing of yours but the call; the fields a contest
            // sends join the card with it.
            CallField {
                value: s.my_call.clone(),
                on_commit: move |typed: String| settings.write().contest.set_my_call(&typed),
            }
            // With no call yet, what a run is missing, in the words Home
            // says it in.
            CallReading { call: s.my_call.clone(), unset: s.missing.clone() }
            p { class: "muted", style: "margin: 0;",
                "No licence yet? Make up a call from your country's prefix, a digit and two or three letters. Stored on this device only."
            }
            if let Some(name) = s.my_name.clone() {
                NameField {
                    value: name,
                    on_commit: move |typed: String| settings.write().contest.set_my_name(&typed),
                }
                NameReading { missing: name_missing }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::call_reading;

    #[test]
    fn a_call_reads_as_where_a_contest_places_it() {
        assert_eq!(
            call_reading("DL1ABC"),
            Some(("Reads as DL · WPX prefix DL1".to_string(), true))
        );
        assert_eq!(
            call_reading("W1AW/4"),
            Some(("Reads as K · WPX prefix W4".to_string(), true))
        );
        // A call the table cannot place still has its WPX prefix.
        assert_eq!(
            call_reading("C31AB").map(|(line, read)| (line.contains("unknown prefix"), read)),
            Some((true, true))
        );
        assert_eq!(
            call_reading("ABC"),
            Some(("Not a callsign yet: needs a digit".to_string(), false))
        );
        assert_eq!(call_reading(""), None);
    }
}
