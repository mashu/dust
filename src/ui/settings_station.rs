//! Your station: the callsign you sign in a contest, and the fields of the
//! exchange you send, given once: your name and your state, for a contest
//! that sends them. The card shows in Contest mode only, under the
//! character set whose contest it serves, and Home asks for the call, then
//! the name, then the state, in the same words while it has none.

use cw_core::{
    SettingsSection, TrainingSettings, call_fault, country_of, parse_callsign, place_name,
    state_code, wpx_prefix,
};
use dioxus::prelude::*;

use crate::ui::widgets::{CardHead, IconName, SectionReset, control_id};

/// What the callsign, name and state fields are called, and so their ids.
const CALL_LABEL: &str = "Your callsign";
const NAME_LABEL: &str = "Your name";
const STATE_LABEL: &str = "Your state or province";

/// Give the field called `label` the focus, as a start refused for want of
/// what it holds does. The script looks for the field at once, so it has to
/// be drawn already: a field the same press brings in is focused from an
/// effect, which runs once the page has the render.
fn focus_field(label: &'static str) {
    let id = control_id("field", label);
    // Tests have no page to run it on: the harness looks for the field.
    #[cfg(test)]
    crate::testing::ask_focus(&id);
    #[cfg(not(test))]
    spawn(async move {
        let script = crate::ui::widgets::focus_script(&id);
        let _ = document::eval(&script).await;
    });
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

/// Give the state field the focus, as a start refused for want of a state
/// does.
pub(crate) fn focus_state_field() {
    focus_field(STATE_LABEL);
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

/// What is typed into one of your station's fields and not kept yet, which
/// the field shares with the buttons that keep the cursor in it: Home's
/// Start and Listen, pressed with a call just typed, keep that call before
/// they act, so that nothing above them changes under the finger between
/// the press and the click.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct StationDraft {
    typed: CopyValue<Option<String>>,
    repaint: Signal<u64>,
}

impl StationDraft {
    pub(crate) fn use_draft() -> Self {
        Self {
            typed: use_hook(|| CopyValue::new(None)),
            repaint: use_signal(|| 0),
        }
    }

    /// What was typed and not kept yet, taken: from then on the box shows
    /// what the setting keeps, as it does once it is left.
    pub(crate) fn take(mut self) -> Option<String> {
        let typed = self.typed.write().take();
        if typed.is_some() {
            self.repaint += 1;
        }
        typed
    }
}

/// Your callsign, typed in, kept as a call can be written: upper case,
/// letters, digits and single slashes.
#[component]
pub(crate) fn CallField(
    value: String,
    on_commit: EventHandler<String>,
    #[props(default)] draft: Option<StationDraft>,
) -> Element {
    rsx! {
        StationText { label: CALL_LABEL, placeholder: "e.g. DL1ABC", value, on_commit, draft }
    }
}

/// Your name, typed in, kept as a name is sent: letters only, upper case.
#[component]
pub(crate) fn NameField(
    value: String,
    on_commit: EventHandler<String>,
    #[props(default)] draft: Option<StationDraft>,
) -> Element {
    rsx! {
        StationText { label: NAME_LABEL, placeholder: "e.g. MAT", value, on_commit, draft }
    }
}

/// What typing `typed` as your state leaves for the line under the field to
/// say: the text, upper case, where it is no code and so is kept as none;
/// nothing where it is a code, or nothing was typed.
pub(crate) fn state_rejected(typed: &str) -> Option<String> {
    let typed = typed.trim();
    (state_code(typed).is_none() && !typed.is_empty()).then(|| typed.to_uppercase())
}

/// What your state reads as, in a line under it: the place its code names,
/// why what was typed is none, DX for a call outside the US and Canada, or
/// else `missing`, what a run is missing once that is the state.
fn state_reading(
    state: &str,
    sent: &str,
    rejected: Option<&str>,
    missing: Option<&str>,
) -> Option<(String, bool)> {
    if let Some(typed) = rejected.filter(|_| state.is_empty()) {
        return Some((
            format!("{typed} is not a state or province: type its code, as MA or ON, or DX"),
            false,
        ));
    }
    if let Some(name) = place_name(state) {
        return Some((format!("Reads as {name}"), true));
    }
    // Sent and no place: DX, the one code that names none.
    if !sent.is_empty() {
        let how = if state.is_empty() {
            "Sends"
        } else {
            "Reads as"
        };
        return Some((
            format!("{how} DX: in Dust every station outside the US and Canada sends DX"),
            true,
        ));
    }
    missing.map(|missing| (missing.to_string(), true))
}

/// The line under your state.
#[component]
fn StateReading(
    state: String,
    sent: String,
    rejected: Option<String>,
    missing: Option<String>,
) -> Element {
    let Some((line, read)) = state_reading(&state, &sent, rejected.as_deref(), missing.as_deref())
    else {
        return rsx! {};
    };
    rsx! {
        p {
            id: control_id("reading", "your state"),
            class: if read { "call-reading" } else { "call-reading bad" },
            aria_live: "polite",
            "{line}"
        }
    }
}

/// Your state, province or territory, typed in and kept as its code (`MA`,
/// `ON`, or `DX`), and the line under it, as Your station and Home both ask
/// for it. What is no code is kept as none: the one asking keeps what was
/// typed of it ([`state_rejected`]) as `rejected`, for as long as that is
/// what the field was last left with, for the line to say so.
#[component]
pub(crate) fn StateEntry(
    value: String,
    /// What your exchange sends: the state, or DX where none is given and
    /// the call is outside the US and Canada; nothing where that is never
    /// the case, as on Home, which asks only while it is not.
    #[props(default)]
    sent: String,
    #[props(default)] missing: Option<String>,
    rejected: Option<String>,
    on_commit: EventHandler<String>,
    #[props(default)] draft: Option<StationDraft>,
) -> Element {
    rsx! {
        StationText {
            label: STATE_LABEL,
            placeholder: "e.g. MA, ON or DX",
            value: value.clone(),
            on_commit,
            draft,
        }
        StateReading { state: value, sent, rejected, missing }
    }
}

/// One of your station's fields, typed in.
///
/// What is typed stays as typed until it is committed, on Enter or on
/// leaving the box, or by a button given its `draft`, and only then is it
/// kept as the setting keeps it. A key does not redraw the box, which on the
/// desktop and phone builds would reach it a frame late, over what was typed
/// since; each commit does, so the box shows what was kept even when that is
/// what it held already.
#[component]
fn StationText(
    label: &'static str,
    placeholder: &'static str,
    value: String,
    on_commit: EventHandler<String>,
    draft: Option<StationDraft>,
) -> Element {
    let own = StationDraft::use_draft();
    let draft = draft.unwrap_or(own);
    let mut typed = draft.typed;
    let _ = draft.repaint.read();
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
                    draft.take();
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
/// name and state where the contest sends them, and what a run is missing.
#[derive(Clone, PartialEq)]
struct Station {
    my_call: String,
    readable: bool,
    /// Your name, where the contest sends one; none where it does not.
    my_name: Option<String>,
    /// Your state as given, and as sent, where the contest sends one.
    my_state: Option<(String, String)>,
    missing: Option<String>,
}

#[component]
pub fn StationCard(settings: Signal<TrainingSettings>) -> Element {
    // A state typed that is none, said under the field until anything else
    // of your station changes: your call, your name, the contest, or the
    // card's reset.
    let mut rejected = use_signal(|| None::<String>);
    let around = use_memo(move || {
        let contest = &settings.read().contest;
        (
            contest.my_call.clone(),
            contest.my_name.clone(),
            contest.contest_kind,
        )
    });
    use_effect(move || {
        around.read();
        rejected.set(None);
    });
    let station = use_memo(move || {
        let contest = &settings.read().contest;
        let rules = contest.contest_kind.rules();
        let sends_name = rules.is_some_and(|rules| rules.sends_name());
        let sends_state = rules.is_some_and(|rules| rules.sends_state());
        Station {
            readable: parse_callsign(&contest.my_call).is_some(),
            my_call: contest.my_call.clone(),
            my_name: sends_name.then(|| contest.my_name.clone()),
            my_state: sends_state
                .then(|| (contest.my_state.clone(), contest.state_sent().to_string())),
            missing: contest.missing(),
        }
    });
    let s = station.read();
    let note = if s.readable {
        // What you sign with, as far as it is given: "K1ABC · MIKE · MA".
        let name = s.my_name.iter().filter(|name| !name.is_empty());
        let state = s
            .my_state
            .iter()
            .map(|(_, sent)| sent)
            .filter(|sent| !sent.is_empty());
        let signing: Vec<&str> = std::iter::once(&s.my_call)
            .chain(name)
            .chain(state)
            .map(String::as_str)
            .collect();
        format!("Signing {}", signing.join(" · "))
    } else {
        match (&s.my_name, &s.my_state) {
            (Some(_), Some(_)) => "Your callsign, name and state, for the contests you run",
            (Some(_), None) => "Your callsign and name, for the contests you run",
            _ => "Your callsign, for the contests you run",
        }
        .to_string()
    };
    // Said under the call while it is missing, and under the name once only
    // the name is.
    let name_missing = s
        .my_name
        .as_ref()
        .filter(|name| name.is_empty() && s.readable)
        .and(s.missing.clone());
    // And under the state once only the state is.
    let state_missing = s
        .my_state
        .as_ref()
        .filter(|(_, sent)| sent.is_empty() && s.readable && name_missing.is_none())
        .and(s.missing.clone());
    rsx! {
        div { class: "card stack-sm",
            CardHead { icon: IconName::Flag, title: "Your station", note,
                SectionReset {
                    label: "your station",
                    on_reset: move |()| {
                        rejected.set(None);
                        settings.write().reset_section(SettingsSection::Station);
                    },
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
            if let Some((state, sent)) = s.my_state.clone() {
                StateEntry {
                    value: state,
                    sent,
                    missing: state_missing,
                    rejected: rejected(),
                    on_commit: move |typed: String| {
                        rejected.set(state_rejected(&typed));
                        settings.write().contest.set_my_state(&typed);
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{call_reading, state_reading, state_rejected};

    /// The line under your state: the place it names, DX outside the US
    /// and Canada, why what was typed is no state, or what a run is
    /// missing, once that is the state.
    #[test]
    fn a_state_reads_as_the_place_it_names() {
        assert_eq!(
            state_reading("MA", "MA", None, None),
            Some(("Reads as Massachusetts".to_string(), true))
        );
        assert_eq!(
            state_reading("QC", "QC", None, None),
            Some(("Reads as Quebec".to_string(), true))
        );
        assert_eq!(
            state_reading("", "DX", None, None),
            Some((
                "Sends DX: in Dust every station outside the US and Canada sends DX".to_string(),
                true
            ))
        );
        assert_eq!(
            state_reading("DX", "DX", None, None),
            Some((
                "Reads as DX: in Dust every station outside the US and Canada sends DX".to_string(),
                true
            ))
        );
        assert_eq!(
            state_reading("", "", Some("MASS"), None),
            Some((
                "MASS is not a state or province: type its code, as MA or ON, or DX".to_string(),
                false
            ))
        );
        let missing = "Add your state or province to start an NA\u{a0}Sprint";
        assert_eq!(
            state_reading("", "", None, Some(missing)),
            Some((missing.to_string(), true))
        );
        assert_eq!(state_reading("", "", None, None), None);
        // What is said of a state typed: nothing for a code, or nothing.
        assert_eq!(state_rejected(" Mass "), Some("MASS".to_string()));
        for kept in ["ma", "PQ", "dx", "  ", ""] {
            assert_eq!(state_rejected(kept), None, "{kept:?}");
        }
    }

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
