//! Your station: the callsign you sign in a contest, and the fields of the
//! exchange you send, given once: your name, your state, your CQ zone, your
//! power, and your Field Day class and section, for a contest that sends
//! them. The card shows in Contest mode only, under the character set whose
//! contest it serves, and Home asks for the call, then the name, then the
//! state, then the zone, then the section, in the same words while it has
//! none.

use cw_core::{
    ContestKind, ContestRules, MY_ZONE_MAX, SettingsSection, Side, TrainingSettings, call_fault,
    class_reading, continent_name, country_name, cut_power, cut_zone, home_continent, home_country,
    parse_callsign, place_name, power_reading, section_code, section_name, state_code, wpx_prefix,
};
use dioxus::prelude::*;

use crate::ui::help;
use crate::ui::widgets::{
    CardHead, IconName, NumberField, SectionReset, TextDraft, TextField, control_id,
};

/// What the callsign, name, state, zone and power fields are called, and so
/// their ids.
const CALL_LABEL: &str = "Your callsign";
const NAME_LABEL: &str = "Your name";
const STATE_LABEL: &str = "Your state or province";
const ZONE_LABEL: &str = "Your CQ zone (0–40)";
const POWER_LABEL: &str = "Your power";
const CLASS_LABEL: &str = "Your class";
const SECTION_LABEL: &str = "Your section";
/// What Home's zone field is called, where only a zone of 1 to 40 will do.
const ZONE_ENTRY_LABEL: &str = "Your CQ zone";

/// Give the field called `label` the focus, as a start refused for want of
/// what it holds does. The script looks for the field at once, so it has to
/// be drawn already: a field the same press brings in is focused from an
/// effect, which runs once the page has the render.
fn focus_field(label: &'static str) {
    let id = control_id("field", label);
    let script = crate::ui::widgets::focus_script(&id);
    // Tests have no page to run it on: the harness looks for the field.
    #[cfg(test)]
    crate::testing::ask_focus(&id, &script);
    #[cfg(not(test))]
    spawn(async move {
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

/// Give Home's zone field the focus, as a start refused for want of a zone
/// does.
pub(crate) fn focus_zone_field() {
    focus_field(ZONE_ENTRY_LABEL);
}

/// Give the section field the focus, as a start refused for want of a
/// section does.
pub(crate) fn focus_section_field() {
    focus_field(SECTION_LABEL);
}

/// A CQ zone as the lines under your call and your zone name it: "zone 3",
/// and "zone 3 in California" where your state, `by`, and not your call
/// alone gave it.
fn zone_text(zone: u32, by: Option<&str>) -> String {
    match by.and_then(place_name) {
        Some(place) => format!("zone {zone} in {place}"),
        None => format!("zone {zone}"),
    }
}

/// What a callsign reads as, in a line under it: where a contest places the
/// station, or why it is not a callsign yet. Nothing for no call at all.
///
/// A call Dust places reads as its country, its continent and `zone`, the
/// CQ zone it reads as, by your state `zone_by` where that gave it; then
/// what `kind` counts it by: its WPX prefix in WPX, and your side in ARRL
/// DX. "Fed. Rep. of Germany · Europe · CQ zone 14 · you are DX". Each part
/// is kept on one line, and a narrow line breaks after a dot.
fn call_reading(
    call: &str,
    kind: ContestKind,
    zone: Option<u32>,
    zone_by: Option<&str>,
) -> Option<(String, bool)> {
    if call.is_empty() {
        return None;
    }
    if let Some(fault) = call_fault(call) {
        return Some((format!("Not a callsign yet: {fault}"), false));
    }
    let mut parts = match home_country(call) {
        Some(country) => {
            let mut parts = vec![country_name(country).unwrap_or(country).to_string()];
            parts.extend(
                home_continent(call)
                    .and_then(continent_name)
                    .map(str::to_string),
            );
            parts.extend(zone.map(|zone| format!("CQ {}", zone_text(zone, zone_by))));
            parts
        }
        None => vec!["unknown prefix, so no country".to_string()],
    };
    if kind == ContestKind::Wpx {
        let wpx = wpx_prefix(call).unwrap_or_default();
        parts.push(format!("WPX prefix {wpx}"));
    }
    if kind == ContestKind::ArrlDx {
        parts.push(format!("you are {}", Side::of_call(call).label()));
    }
    let parts: Vec<String> = parts
        .iter()
        .map(|part| part.replace(' ', "\u{a0}"))
        .collect();
    Some((format!("Reads as {}", parts.join("\u{a0}· ")), true))
}

/// Your callsign, typed in, kept as a call can be written: upper case,
/// letters, digits and single slashes.
#[component]
pub(crate) fn CallField(
    value: String,
    on_commit: EventHandler<String>,
    #[props(default)] draft: Option<TextDraft>,
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
    #[props(default)] draft: Option<TextDraft>,
) -> Element {
    rsx! {
        StationText { label: NAME_LABEL, placeholder: "e.g. MAT", value, on_commit, draft }
    }
}

/// What typing `typed` into a field that keeps only what `reads` leaves
/// for the line under it to say: the text, upper case, where it does not
/// read and so is not kept; nothing where it reads, or nothing was typed.
fn rejected(typed: &str, reads: impl Fn(&str) -> bool) -> Option<String> {
    let typed = typed.trim();
    (!reads(typed) && !typed.is_empty()).then(|| typed.to_uppercase())
}

/// What typing `typed` as your state leaves for the line under the field to
/// say ([`rejected`]): what is no code, kept as none.
pub(crate) fn state_rejected(typed: &str) -> Option<String> {
    rejected(typed, |typed| state_code(typed).is_some())
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
    // Labrador, which only ARRL DX counts apart.
    if let (Some(given), Some(within)) = (place_name(state), place_name(sent))
        && state != sent
    {
        return Some((
            format!("Sends {sent}: this contest counts {given} as part of {within}"),
            true,
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
    #[props(default)] draft: Option<TextDraft>,
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

/// One of your station's fields, typed in ([`TextField`]), as wide as a
/// call. Its `draft` is shared with the buttons that keep the cursor in it:
/// Home's Start and Listen, pressed with a call just typed, keep that call
/// before they act, so that nothing above them changes under the finger
/// between the press and the click.
#[component]
fn StationText(
    label: &'static str,
    placeholder: &'static str,
    value: String,
    on_commit: EventHandler<String>,
    draft: Option<TextDraft>,
) -> Element {
    rsx! {
        TextField {
            label,
            placeholder,
            class: "field call-field",
            value,
            on_commit,
            draft,
        }
    }
}

/// The line under a callsign: what it reads as in the contest `kind`, its
/// CQ zone `zone` among it, by your state `zone_by` where that gave it, or
/// why it does not read; with no call, `unset`, if there is something to
/// say then.
#[component]
pub(crate) fn CallReading(
    call: String,
    kind: ContestKind,
    zone: Option<u32>,
    zone_by: Option<&'static str>,
    #[props(default)] unset: Option<String>,
) -> Element {
    let Some((line, read)) =
        call_reading(&call, kind, zone, zone_by).or(unset.map(|line| (line, true)))
    else {
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

/// What your zone reads as, in a line under it: the zone your call reads
/// as while you give none, by your state `by` where that gave it; what you
/// give and how it goes out, and the zone your call reads as beside one you
/// give that differs; or else `missing`, what a run is missing once that is
/// the zone.
fn zone_reading(
    given: u32,
    from_call: Option<u32>,
    by: Option<&str>,
    missing: Option<&str>,
) -> String {
    let whose = if by.is_some() {
        "your call and state"
    } else {
        "your call"
    };
    match (given, from_call) {
        (0, Some(zone)) => format!(
            "0 takes it from {whose}: {}, sent {}",
            zone_text(zone, by),
            cut_zone(zone)
        ),
        (0, None) => missing
            .unwrap_or("Your call does not say your zone: give it, 1\u{a0}to\u{a0}40")
            .to_string(),
        (zone, Some(read)) if read != zone => format!(
            "Sends {}: zone {zone}, where {whose} reads as {}",
            cut_zone(zone),
            zone_text(read, by)
        ),
        (zone, _) => format!("Sends {}: zone {zone}", cut_zone(zone)),
    }
}

/// The line under your zone.
#[component]
fn ZoneReading(
    given: u32,
    from_call: Option<u32>,
    by: Option<&'static str>,
    missing: Option<String>,
) -> Element {
    let line = zone_reading(given, from_call, by, missing.as_deref());
    rsx! {
        p {
            id: control_id("reading", "your zone"),
            class: "call-reading",
            aria_live: "polite",
            "{line}"
        }
    }
}

/// Your CQ zone, typed in on Home while your call does not say it, and the
/// line under it: a zone from 1 to 40 is kept, and anything else as none.
#[component]
pub(crate) fn ZoneField(
    value: u32,
    on_commit: EventHandler<String>,
    #[props(default)] draft: Option<TextDraft>,
) -> Element {
    let shown = if value == 0 {
        String::new()
    } else {
        value.to_string()
    };
    rsx! {
        StationText {
            label: ZONE_ENTRY_LABEL,
            placeholder: "1 to 40",
            value: shown,
            on_commit,
            draft,
        }
        ZoneReading { given: value, from_call: None, by: None, missing: None }
    }
}

/// What typing `typed` as your power leaves for the line under the field to
/// say ([`rejected`]): what is no power, kept as 100 W.
fn power_rejected(typed: &str) -> Option<String> {
    rejected(typed, |typed| power_reading(typed).is_some())
}

/// Your power as the card's lines say it: "100 W", or none for a
/// kilowatt, which is said as written.
fn watts_text(power: &str) -> Option<String> {
    power
        .chars()
        .all(|c| c.is_ascii_digit())
        .then(|| format!("{power} W"))
}

/// What your power reads as, in a line under it: how it goes out, or why
/// what was typed is none.
fn power_line(power: &str, rejected: Option<&str>) -> (String, bool) {
    if let Some(typed) = rejected {
        return (
            format!("{typed} is not a power: type watts, 1 to 1500, or K, KW or 1K; 100 W is sent"),
            false,
        );
    }
    let what = watts_text(power).unwrap_or_else(|| "a kilowatt".to_string());
    (format!("Sends {}: {what}", cut_power(power)), true)
}

/// The line under your power.
#[component]
fn PowerReading(power: String, rejected: Option<String>) -> Element {
    let (line, read) = power_line(&power, rejected.as_deref());
    rsx! {
        p {
            id: control_id("reading", "your power"),
            class: if read { "call-reading" } else { "call-reading bad" },
            aria_live: "polite",
            "{line}"
        }
    }
}

/// What typing `typed` as your class leaves for the line under the field to
/// say ([`rejected`]): what is no class, kept as 1D.
fn class_rejected(typed: &str) -> Option<String> {
    rejected(typed, |typed| class_reading(typed).is_some())
}

/// What the letter of a Field Day class says of the station: "a home
/// station on commercial power" for D.
fn class_letter(letter: char) -> &'static str {
    match letter {
        'A' => "a club or group in the field",
        'B' => "one or two operators in the field",
        'C' => "a mobile station",
        'D' => "a home station on commercial power",
        'E' => "a home station on emergency power",
        _ => "an Emergency Operations Center",
    }
}

/// What your class reads as, in a line under it: how many transmitters and
/// what kind of station, or why what was typed is none.
fn class_line(class: &str, rejected: Option<&str>) -> (String, bool) {
    if let Some(typed) = rejected {
        return (
            format!(
                "{typed} is not a class: type the number of transmitters, 1 to 99, and a letter A to F, as 1D or 3A; 1D is sent"
            ),
            false,
        );
    }
    let count = class.trim_end_matches(|c: char| c.is_ascii_uppercase());
    let transmitters = if count == "1" {
        "1 transmitter".to_string()
    } else {
        format!("{count} transmitters")
    };
    let kind = class.chars().last().map_or("", class_letter);
    (format!("Sends {class}: {transmitters}, {kind}"), true)
}

/// The line under your class.
#[component]
fn ClassReading(class: String, rejected: Option<String>) -> Element {
    let (line, read) = class_line(&class, rejected.as_deref());
    rsx! {
        p {
            id: control_id("reading", "your class"),
            class: if read { "call-reading" } else { "call-reading bad" },
            aria_live: "polite",
            "{line}"
        }
    }
}

/// What typing `typed` as your section leaves for the line under the field
/// to say ([`rejected`]): what is no code, kept as none.
pub(crate) fn section_rejected(typed: &str) -> Option<String> {
    rejected(typed, |typed| section_code(typed).is_some())
}

/// What your section reads as, in a line under it: the section its code
/// names, Yukon's as the Territories', DX; the one your call tells while
/// you give none; why what was typed is none; or else `missing`, what a run
/// is missing once that is the section, or what a section is, for a
/// learner who has not met ARRL's word for it.
fn section_reading(
    section: &str,
    sent: &str,
    rejected: Option<&str>,
    missing: Option<&str>,
) -> (String, bool) {
    if let Some(typed) = rejected.filter(|_| section.is_empty()) {
        return (
            format!("{typed} is not a section: type its code, as EMA or ONS, or DX"),
            false,
        );
    }
    let outside = "stations outside the ARRL and RAC sections send DX";
    if !section.is_empty() {
        let line = if section != sent {
            // Yukon's, which ARRL contests count in the Territories.
            format!("Sends {sent}: ARRL contests count Yukon in the Territories")
        } else if let Some(name) = section_name(section) {
            format!("Reads as {name}")
        } else {
            format!("Reads as DX: {outside}")
        };
        return (line, true);
    }
    if let Some(name) = section_name(sent) {
        return (format!("Sends {sent} from your call: {name}"), true);
    }
    if !sent.is_empty() {
        return (format!("Sends DX from your call: {outside}"), true);
    }
    let line = missing
        .unwrap_or("Your ARRL or RAC section, as EMA for Eastern Massachusetts; DX outside them");
    (line.to_string(), true)
}

/// The line under your section.
#[component]
fn SectionReading(
    section: String,
    sent: String,
    rejected: Option<String>,
    missing: Option<String>,
) -> Element {
    let (line, read) = section_reading(&section, &sent, rejected.as_deref(), missing.as_deref());
    rsx! {
        p {
            id: control_id("reading", "your section"),
            class: if read { "call-reading" } else { "call-reading bad" },
            aria_live: "polite",
            "{line}"
        }
    }
}

/// Your ARRL or RAC section, typed in and kept as its code (`EMA`, `ONS`,
/// or `DX`), and the line under it, as Your station and Home both ask for
/// it. What is no code is kept as none: the one asking keeps what was typed
/// of it ([`section_rejected`]) as `rejected`, for as long as that is what
/// the field was last left with, for the line to say so.
#[component]
pub(crate) fn SectionEntry(
    value: String,
    /// What your exchange sends: the section, or the one your call tells
    /// where none is given; nothing where neither is, as on Home, which asks
    /// only then.
    #[props(default)]
    sent: String,
    #[props(default)] missing: Option<String>,
    rejected: Option<String>,
    on_commit: EventHandler<String>,
    #[props(default)] draft: Option<TextDraft>,
) -> Element {
    rsx! {
        StationText {
            label: SECTION_LABEL,
            placeholder: "e.g. EMA, ONS or DX",
            value: value.clone(),
            on_commit,
            draft,
        }
        SectionReading { section: value, sent, rejected, missing }
    }
}

/// What Your station shows: the call, whether a contest can read it, your
/// name, state, zone, power, class and section where the contest sends
/// them, and what a run is missing.
#[derive(Clone, PartialEq)]
struct Station {
    my_call: String,
    readable: bool,
    kind: ContestKind,
    /// The CQ zone your call reads as, and your state where that and not
    /// your call alone gives it; and the zone your exchange sends.
    zone_from_call: Option<u32>,
    zone_by: Option<&'static str>,
    zone: Option<u32>,
    /// Your name, where the contest sends one; none where it does not.
    my_name: Option<String>,
    /// Your state as given, and as sent, where the contest sends one.
    my_state: Option<(String, String)>,
    /// Your zone as given, 0 for the one your call reads as, where the
    /// contest sends one.
    my_zone: Option<u32>,
    /// Your power, where the contest sends it.
    my_power: Option<String>,
    /// Your class, where the contest sends it.
    my_class: Option<String>,
    /// Your section as given, and as sent, where the contest sends one.
    my_section: Option<(String, String)>,
    missing: Option<String>,
}

#[component]
pub fn StationCard(settings: Signal<TrainingSettings>) -> Element {
    // A state typed that is none, said under the field until anything else
    // of your station changes: your call, your name, the contest, or the
    // card's reset.
    let mut state_said = use_signal(|| None::<String>);
    // And a power, a class or a section typed that is none, the same way.
    let mut power_said = use_signal(|| None::<String>);
    let mut class_said = use_signal(|| None::<String>);
    let mut section_said = use_signal(|| None::<String>);
    let mut unsay = move || {
        state_said.set(None);
        power_said.set(None);
        class_said.set(None);
        section_said.set(None);
    };
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
        unsay();
    });
    let station = use_memo(move || {
        let contest = &settings.read().contest;
        let rules = contest.contest_kind.rules();
        let side = contest.side();
        let sends = |sends: fn(&ContestRules, Side) -> bool| sends(rules, side);
        let sends_name = rules.sends_name();
        Station {
            readable: parse_callsign(&contest.my_call).is_some(),
            my_call: contest.my_call.clone(),
            kind: contest.contest_kind,
            zone_from_call: contest.zone_from_call(),
            zone_by: contest.zone_state(),
            zone: contest.zone(),
            my_name: sends_name.then(|| contest.my_name.clone()),
            my_state: sends(ContestRules::sends_state)
                .then(|| (contest.my_state.clone(), contest.state_sent().to_string())),
            my_zone: sends(ContestRules::sends_zone).then_some(contest.my_zone),
            my_power: sends(ContestRules::sends_power).then(|| contest.my_power.clone()),
            my_class: sends(ContestRules::sends_class).then(|| contest.my_class.clone()),
            my_section: sends(ContestRules::sends_section).then(|| {
                (
                    contest.my_section.clone(),
                    contest.section_sent().to_string(),
                )
            }),
            missing: contest.missing(),
        }
    });
    let s = station.read();
    // Your zone, where the contest sends it.
    let zone = s.my_zone.and(s.zone);
    let note = if s.readable {
        // What you sign with, as far as it is given: "K1ABC · MIKE · MA",
        // "DL1ABC · zone 14", "DL1ABC · 100 W", "K1ABC · 1D · EMA".
        let name = s.my_name.iter().filter(|name| !name.is_empty()).cloned();
        let state = s
            .my_state
            .iter()
            .map(|(_, sent)| sent.clone())
            .filter(|sent| !sent.is_empty());
        let zone = zone.map(|zone| format!("zone {zone}"));
        let power = s
            .my_power
            .iter()
            .map(|power| watts_text(power).unwrap_or_else(|| power.clone()));
        let section = s
            .my_section
            .iter()
            .map(|(_, sent)| sent.clone())
            .filter(|sent| !sent.is_empty());
        let signing: Vec<String> = std::iter::once(s.my_call.clone())
            .chain(name)
            .chain(state)
            .chain(zone)
            .chain(power)
            .chain(s.my_class.clone())
            .chain(section)
            .collect();
        format!("Signing {}", signing.join(" · "))
    } else {
        // What the card asks for: "Your callsign, name and state".
        let mut asks = vec!["callsign"];
        asks.extend(s.my_name.as_ref().map(|_| "name"));
        asks.extend(s.my_state.as_ref().map(|_| "state"));
        asks.extend(s.my_zone.map(|_| "CQ zone"));
        asks.extend(s.my_power.as_ref().map(|_| "power"));
        asks.extend(s.my_class.as_ref().map(|_| "class"));
        asks.extend(s.my_section.as_ref().map(|_| "section"));
        let asks = match asks.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            _ => asks.join(""),
        };
        format!("Your {asks}, for the contests you run")
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
    // And under the zone once only the zone is.
    let zone_missing = s
        .my_zone
        .filter(|_| zone.is_none() && s.readable)
        .and(s.missing.clone());
    // And under the section once only the section is.
    let section_missing = s
        .my_section
        .as_ref()
        .filter(|(_, sent)| sent.is_empty() && s.readable)
        .and(s.missing.clone());
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Flag,
                title: "Your station",
                note,
                help: help::station(),
                SectionReset {
                    card: "Your station",
                    settings,
                    section: SettingsSection::Station,
                    on_reset: move |()| unsay(),
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
            CallReading {
                call: s.my_call.clone(),
                kind: s.kind,
                zone: s.zone_from_call,
                zone_by: s.zone_by,
                unset: s.missing.clone(),
            }
            p { class: "muted hint",
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
                    rejected: state_said(),
                    on_commit: move |typed: String| {
                        state_said.set(state_rejected(&typed));
                        settings.write().contest.set_my_state(&typed);
                    },
                }
            }
            if let Some(given) = s.my_zone {
                NumberField {
                    label: ZONE_LABEL,
                    id: control_id("field", ZONE_LABEL),
                    value: f64::from(given),
                    min: 0.0,
                    max: f64::from(MY_ZONE_MAX),
                    step: 1.0,
                    onchange: move |zone: f64| settings.write().contest.my_zone = zone as u32,
                }
                ZoneReading {
                    given,
                    from_call: s.zone_from_call,
                    by: s.zone_by,
                    missing: zone_missing,
                }
            }
            if let Some(power) = s.my_power.clone() {
                StationText {
                    label: POWER_LABEL,
                    placeholder: "e.g. 100 or KW",
                    value: power.clone(),
                    on_commit: move |typed: String| {
                        power_said.set(power_rejected(&typed));
                        settings.write().contest.set_my_power(&typed);
                    },
                    draft: None,
                }
                PowerReading { power, rejected: power_said() }
            }
            if let Some(class) = s.my_class.clone() {
                StationText {
                    label: CLASS_LABEL,
                    placeholder: "e.g. 1D or 3A",
                    value: class.clone(),
                    on_commit: move |typed: String| {
                        class_said.set(class_rejected(&typed));
                        settings.write().contest.set_my_class(&typed);
                    },
                    draft: None,
                }
                ClassReading { class, rejected: class_said() }
            }
            if let Some((section, sent)) = s.my_section.clone() {
                SectionEntry {
                    value: section,
                    sent,
                    missing: section_missing,
                    rejected: section_said(),
                    on_commit: move |typed: String| {
                        section_said.set(section_rejected(&typed));
                        settings.write().contest.set_my_section(&typed);
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        call_reading, class_line, class_rejected, power_line, power_rejected, section_reading,
        section_rejected, state_reading, state_rejected, zone_reading,
    };
    use cw_core::ContestKind;

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
        // Labrador reads as itself where it is counted apart, and as part
        // of Newfoundland and Labrador where it is not.
        assert_eq!(
            state_reading("LB", "LB", None, None),
            Some(("Reads as Labrador".to_string(), true))
        );
        assert_eq!(
            state_reading("LB", "NL", None, None),
            Some((
                "Sends NL: this contest counts Labrador as part of Newfoundland and Labrador"
                    .to_string(),
                true
            ))
        );
        // What is said of a state typed: nothing for a code, or nothing.
        assert_eq!(state_rejected(" Mass "), Some("MASS".to_string()));
        for kept in ["ma", "PQ", "dx", "  ", ""] {
            assert_eq!(state_rejected(kept), None, "{kept:?}");
        }
    }

    /// A call reads as its country, its continent and its CQ zone, and
    /// what the contest counts it by: its WPX prefix in WPX, your side in
    /// ARRL DX. Each part stays on one line.
    #[test]
    fn a_call_reads_as_where_a_contest_places_it() {
        let read = |call, kind, zone, by| {
            call_reading(call, kind, zone, by).map(|(line, _)| line.replace('\u{a0}', " "))
        };
        assert_eq!(
            read("DL1ABC", ContestKind::ArrlDx, Some(14), None).as_deref(),
            Some("Reads as Fed. Rep. of Germany · Europe · CQ zone 14 · you are DX")
        );
        assert_eq!(
            read("DL1ABC", ContestKind::Wpx, Some(14), None).as_deref(),
            Some("Reads as Fed. Rep. of Germany · Europe · CQ zone 14 · WPX prefix DL1")
        );
        assert_eq!(
            read("W1AW/4", ContestKind::Cqww, Some(5), None).as_deref(),
            Some("Reads as United States · North America · CQ zone 5")
        );
        assert_eq!(
            read("K1ABC", ContestKind::ArrlDx, Some(5), None).as_deref(),
            Some("Reads as United States · North America · CQ zone 5 · you are W/VE")
        );
        assert_eq!(
            read("KH6XYZ", ContestKind::ArrlDx, Some(31), None).as_deref(),
            Some("Reads as Hawaii · Oceania · CQ zone 31 · you are DX")
        );
        // Your state, where that and not your call alone gave the zone.
        assert_eq!(
            read("K1ABC", ContestKind::Cqww, Some(3), Some("CA")).as_deref(),
            Some("Reads as United States · North America · CQ zone 3 in California")
        );
        // Alaska's and Canada's prefixes the call table does not hold, and
        // European Turkey.
        assert_eq!(
            read("NL7AB", ContestKind::ArrlDx, Some(1), None).as_deref(),
            Some("Reads as Alaska · North America · CQ zone 1 · you are DX")
        );
        assert_eq!(
            read("TA1ABC", ContestKind::Cqww, Some(20), None).as_deref(),
            Some("Reads as Asiatic Turkey · Europe · CQ zone 20")
        );
        // A call the table cannot place has its WPX prefix in WPX only.
        assert_eq!(
            read("C31AB", ContestKind::Wpx, None, None).as_deref(),
            Some("Reads as unknown prefix, so no country · WPX prefix C31")
        );
        assert_eq!(
            read("C31AB", ContestKind::ArrlDx, None, None).as_deref(),
            Some("Reads as unknown prefix, so no country · you are DX")
        );
        assert_eq!(
            read("KH2AB", ContestKind::Cqww, None, None).as_deref(),
            Some("Reads as unknown prefix, so no country")
        );
        // A narrow line breaks only after a dot.
        let line = call_reading("DL1ABC", ContestKind::Cqww, Some(14), None)
            .map(|(line, _)| line)
            .unwrap_or_default();
        assert_eq!(
            line.split(' ').collect::<Vec<_>>(),
            [
                "Reads",
                "as",
                "Fed.\u{a0}Rep.\u{a0}of\u{a0}Germany\u{a0}·",
                "Europe\u{a0}·",
                "CQ\u{a0}zone\u{a0}14"
            ]
        );
        assert_eq!(
            call_reading("ABC", ContestKind::Wpx, None, None),
            Some(("Not a callsign yet: needs a digit".to_string(), false))
        );
        assert_eq!(call_reading("", ContestKind::Wpx, None, None), None);
    }

    /// The line under your zone: the one your call reads as while you give
    /// none, by your state where that gave it, and what you give, cut as it
    /// goes out.
    #[test]
    fn a_zone_reads_as_what_is_sent() {
        assert_eq!(
            zone_reading(0, Some(14), None, None),
            "0 takes it from your call: zone 14, sent A4"
        );
        assert_eq!(
            zone_reading(0, Some(3), Some("CA"), None),
            "0 takes it from your call and state: zone 3 in California, sent 3"
        );
        assert_eq!(zone_reading(5, Some(5), None, None), "Sends 5: zone 5");
        assert_eq!(
            zone_reading(15, Some(14), None, None),
            "Sends A5: zone 15, where your call reads as zone 14"
        );
        assert_eq!(
            zone_reading(4, Some(3), Some("CA"), None),
            "Sends 4: zone 4, where your call and state reads as zone 3 in California"
        );
        assert_eq!(
            zone_reading(0, None, None, None),
            "Your call does not say your zone: give it, 1\u{a0}to\u{a0}40"
        );
        let missing = "Add your CQ zone to start a CQ\u{a0}WW";
        assert_eq!(zone_reading(0, None, None, Some(missing)), missing);
    }

    /// The line under your class: how many transmitters and what kind of
    /// station, or why what was typed is none.
    #[test]
    fn a_class_reads_as_what_is_sent() {
        assert_eq!(
            class_line("1D", None),
            (
                "Sends 1D: 1 transmitter, a home station on commercial power".to_string(),
                true
            )
        );
        assert_eq!(
            class_line("3A", None),
            (
                "Sends 3A: 3 transmitters, a club or group in the field".to_string(),
                true
            )
        );
        assert_eq!(
            class_line("12F", None).0,
            "Sends 12F: 12 transmitters, an Emergency Operations Center"
        );
        assert_eq!(
            class_line("1D", Some("1G")),
            (
                "1G is not a class: type the number of transmitters, 1 to 99, and a letter A to F, as 1D or 3A; 1D is sent"
                    .to_string(),
                false
            )
        );
        assert_eq!(class_rejected(" 1g "), Some("1G".to_string()));
        for kept in ["1d", "03A", " 2 B ", ""] {
            assert_eq!(class_rejected(kept), None, "{kept:?}");
        }
    }

    /// The line under your section: the section it names, Yukon's as the
    /// Territories', DX, the one your call tells while you give none, why
    /// what was typed is none, what a run is missing, or what a section is.
    #[test]
    fn a_section_reads_as_what_is_sent() {
        let read =
            |section, sent, rejected, missing| section_reading(section, sent, rejected, missing).0;
        assert_eq!(
            read("EMA", "EMA", None, None),
            "Reads as Eastern Massachusetts"
        );
        assert_eq!(read("OR", "OR", None, None), "Reads as Oregon");
        assert_eq!(
            read("YT", "TER", None, None),
            "Sends TER: ARRL contests count Yukon in the Territories"
        );
        assert_eq!(
            read("DX", "DX", None, None),
            "Reads as DX: stations outside the ARRL and RAC sections send DX"
        );
        assert_eq!(
            read("", "PAC", None, None),
            "Sends PAC from your call: Pacific"
        );
        assert_eq!(
            read("", "DX", None, None),
            "Sends DX from your call: stations outside the ARRL and RAC sections send DX"
        );
        assert_eq!(
            section_reading("", "", Some("MASS"), None),
            (
                "MASS is not a section: type its code, as EMA or ONS, or DX".to_string(),
                false
            )
        );
        let missing = "Add your section to start a Field\u{a0}Day";
        assert_eq!(read("", "", None, Some(missing)), missing);
        // On Home, which asks only while it is missing, it says what one is.
        assert_eq!(
            read("", "", None, None),
            "Your ARRL or RAC section, as EMA for Eastern Massachusetts; DX outside them"
        );
        assert_eq!(section_rejected(" Mass "), Some("MASS".to_string()));
        for kept in ["ema", "yt", "dx", "  ", ""] {
            assert_eq!(section_rejected(kept), None, "{kept:?}");
        }
    }

    /// The line under your power: how it goes out, or why what was typed
    /// is none.
    #[test]
    fn a_power_reads_as_what_is_sent() {
        assert_eq!(
            power_line("100", None),
            ("Sends 1TT: 100 W".to_string(), true)
        );
        assert_eq!(
            power_line("KW", None),
            ("Sends KW: a kilowatt".to_string(), true)
        );
        assert_eq!(
            power_line("100", Some("MAX")),
            (
                "MAX is not a power: type watts, 1 to 1500, or K, KW or 1K; 100 W is sent"
                    .to_string(),
                false
            )
        );
        assert_eq!(power_rejected(" max "), Some("MAX".to_string()));
        for kept in ["100", "kw", " 5 ", ""] {
            assert_eq!(power_rejected(kept), None, "{kept:?}");
        }
    }
}
