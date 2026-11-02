use cw_core::{
    CALLSIGN_TIER_MAX, CharSetMode, ContestKind, ContestPreset, FAREWELLS, Fill, GROUP_SIZE_MAX,
    LevelAxis, MAX_DIGITS_LEVEL, PracticeWindow, RunStyle, SEQUENCE_PRESETS, STARTER_WORDS,
    STYLE_EVEN, STYLE_ON_AIR, Sender, SequenceChoice, SettingsSection, TrainingSettings, TuCall,
    WORD_LIST_MAX, WordList, compute_char_pool, corner_shares, current_practice_window, greeting,
    into_shape, parse_callsign, shape_corners, shares_without, tier_examples, toggle_corner,
    unlocked_practice_count,
};
use dioxus::prelude::*;

use crate::playback::Dialogue;
use crate::state::AppCtx;
use crate::time::local_hour;
use crate::ui::file_pick::{ChosenFile, FileButton};
use crate::ui::format::counted;
use crate::ui::hear_qso::{HeardDialogue, QsoHead, QsoLines, heard_label};
use crate::ui::help;
use crate::ui::paddle::use_typed_field;
use crate::ui::widgets::{
    CardHead, DISCLOSURE, Icon, IconName, NumberField, SectionReset, Seg, TextDraft, TextField,
    control_id, range_label, whole,
};

/// The largest file read as a word list. The longest list kept is a few
/// tens of kilobytes; anything near this is not a word list.
const WORD_FILE_MAX_BYTES: u64 = 1_000_000;

/// The character sets, in the order the control offers them.
pub(crate) const CHAR_SET_MODES: [(CharSetMode, &str); 6] = [
    (CharSetMode::Koch, "Koch"),
    (CharSetMode::Digits, "Digits"),
    (CharSetMode::Mixed, "Mixed"),
    (CharSetMode::Callsign, "Callsigns"),
    (CharSetMode::Words, "Words"),
    (CharSetMode::Contest, "Contest"),
];

/// What a character set is called on screen: "Callsigns".
pub(crate) fn mode_label(mode: CharSetMode) -> &'static str {
    CHAR_SET_MODES
        .iter()
        .find(|(each, _)| *each == mode)
        .map(|(_, label)| *label)
        .unwrap_or_default()
}

/// The run styles, in the order the card offers them.
const RUN_STYLES: [(RunStyle, &str); 2] = [
    (RunStyle::Single, "Single caller"),
    (RunStyle::Pileup, "Pile-up"),
];

/// How often your TU signs with your call, in the order the card offers
/// it.
const TU_CALLS: [(TuCall, &str); 3] = [
    (TuCall::Every, "Every QSO"),
    (TuCall::Third, "Every third"),
    (TuCall::Never, "Never"),
];

/// How many call at once in a pile-up of `settings`, from the Band card's
/// stations range, and the noun that counts them: "1–3" callers, "1" caller.
pub(crate) fn pileup_count(settings: &TrainingSettings) -> (String, &'static str) {
    let band = &settings.band;
    let count = range_label(f64::from(band.stations_min), f64::from(band.stations_max));
    let noun = if band.stations_max == 1 {
        "caller"
    } else {
        "callers"
    };
    (count, noun)
}

/// The same in words: "1–3 callers", "3 callers", "1 caller".
pub(crate) fn pileup_callers(settings: &TrainingSettings) -> String {
    let (count, noun) = pileup_count(settings);
    format!("{count} {noun}")
}

/// The practice windows, and how many characters each needs unlocked before
/// it is offered.
const PRACTICE_WINDOWS: [(PracticeWindow, &str, usize); 3] = [
    (PracticeWindow::All, "Everything", 0),
    (PracticeWindow::Last3, "Newest 3", 3),
    (PracticeWindow::Last5, "Newest 5", 5),
];

/// Your call where an example of a QSO has it, or a stand-in until you
/// give one a contest can read.
fn call_or_stand_in(my_call: &str) -> &str {
    if parse_callsign(my_call).is_some() {
        my_call
    } else {
        "<your call>"
    }
}

/// Your name, state or section where an example of a QSO sends it: what
/// you gave or your call tells (DX for a call outside the US and Canada),
/// or `stand_in` until then.
fn or_stand_in<'a>(sent: &'a str, stand_in: &'a str) -> &'a str {
    if sent.is_empty() { stand_in } else { sent }
}

/// Your CQ zone where an example of a QSO sends it: the one you gave or
/// your call reads as, or a stand-in until one is known.
fn zone_or_stand_in(zone: Option<u32>) -> String {
    zone.map_or_else(|| "<your zone>".to_string(), |zone| zone.to_string())
}

/// What the card shows in Contest mode: which contest, what its exchange
/// is, and a QSO of it as it goes, with your call in it.
#[derive(Clone, PartialEq)]
struct ContestCard {
    kind: ContestKind,
    /// One caller at a time, or a pile-up, and how many call in one.
    style: RunStyle,
    callers: String,
    /// How often your TU signs with your call, where the contest's TU
    /// leaves it to you; none where its rules decide.
    tu_call: Option<TuCall>,
    /// What you send: `5NN + serial`.
    sends: String,
    /// What you copy, as the log heads it: `NR`.
    copies: String,
    /// The side you are on and who you work from it, where sides change
    /// it.
    side_note: Option<String>,
    /// What Dust leaves out of the real contest, if anything.
    simplified: Option<&'static str>,
    /// Who sends each line of the QSO, and what.
    qso: Vec<(Sender, String)>,
    /// How the callers' styles vary, where the contest lets you set it.
    styles: Option<StylesCard>,
    /// The preset the settings are on, if any.
    preset: Option<ContestPreset>,
}

impl ContestCard {
    /// The card for the contest `settings` choose.
    fn of(settings: &TrainingSettings) -> Self {
        let contest = &settings.contest;
        let kind = contest.contest_kind;
        let side = contest.side();
        let style = contest.contest_style;
        let callers = pileup_callers(settings);
        let preset = ContestPreset::of(settings);
        let rules = kind.rules();
        let copies: Vec<&str> = rules
            .copied(side)
            .iter()
            .map(|field| field.label())
            .collect();
        let zone = zone_or_stand_in(contest.zone());
        Self {
            kind,
            style,
            callers,
            tu_call: rules.tu_call_chosen().then_some(contest.contest_tu_call),
            sends: rules.sends(side),
            copies: copies.join("\u{a0}"),
            side_note: rules.side_note(side, parse_callsign(&contest.my_call).is_some()),
            simplified: rules.simplified,
            qso: rules.example_qso(
                side,
                &Fill {
                    my_call: call_or_stand_in(&contest.my_call),
                    my_name: or_stand_in(&contest.my_name, "<your name>"),
                    my_state: or_stand_in(contest.state_sent(), "<your state>"),
                    my_zone: &zone,
                    my_power: &contest.my_power,
                    my_class: &contest.my_class,
                    my_section: or_stand_in(contest.section_sent(), "<your section>"),
                    // The QSO is your first: its TU signs as a run's
                    // first would, with your call in TU every QSO.
                    tu_signed: rules.tu_adds_call(contest.contest_tu_call, 1),
                    // And it greets as a run started now would, and signs
                    // off with the farewell a run sends now and then,
                    // `73 44`, so the card shows it.
                    greeting: greeting(local_hour()),
                    farewell: FAREWELLS[1],
                    ..Fill::default()
                },
            ),
            styles: rules
                .caller_styles(
                    side,
                    call_or_stand_in(&contest.my_call),
                    contest.hunter_style,
                    &contest.hunter_styles_off,
                )
                .map(|styles| {
                    StylesCard::of(styles, contest.hunter_style, &contest.hunter_styles_off)
                }),
            preset,
        }
    }
}

/// How POTA hunters reply, as the card shows it: where you put the dot in
/// the shape of their replies, which you turned off, how many of the
/// hundred send each, and each reply whole as sent to you, a corner each,
/// in the corners' order.
#[derive(Clone, PartialEq)]
struct StylesCard {
    /// Where the dot stands, across and down from the middle.
    at: [f64; 2],
    /// The replies turned off, by their corners from the top, from nought.
    off: Vec<usize>,
    /// The share of hunters that send each reply, exactly, for the shape.
    shares: Vec<f64>,
    /// Each reply, whole, and how many in a hundred send it, in whole
    /// percents that make a hundred.
    replies: Vec<(String, u32)>,
}

impl StylesCard {
    fn of(styles: cw_core::CallerStyles, at: [f64; 2], off: &[usize]) -> Self {
        let shares = styles.replies.iter().map(|(_, share)| *share).collect();
        Self {
            at,
            off: off.to_vec(),
            shares,
            replies: in_percent(styles.replies),
        }
    }

    /// Each reply and how many in a hundred would send it, the dot standing
    /// `at` instead: what the list shows while a mouse is over the shape.
    fn at_point(&self, at: [f64; 2]) -> Vec<(String, u32)> {
        let shares = shares_without(&corner_shares(self.replies.len(), at), &self.off);
        let named = self
            .replies
            .iter()
            .map(|(reply, _)| reply.clone())
            .zip(shares)
            .collect();
        in_percent(named)
    }

    /// How many replies come at all, a hundredth or more of them.
    fn replies_heard(&self) -> usize {
        self.replies.iter().filter(|(_, pct)| *pct > 0).count()
    }

    /// Where the dot stands, in a few words: how many replies come, and
    /// whether that is as on the air or all alike. It heads the folded
    /// line, so the setting reads closed too.
    fn reading(&self) -> String {
        let count = self.replies_heard();
        let off = match self.off.len() {
            0 => String::new(),
            turned => format!(", {turned} off"),
        };
        match self.at {
            _ if count <= 1 => format!("One reply{off}"),
            STYLE_ON_AIR if self.off.is_empty() => format!("{count} replies, as on the air"),
            STYLE_EVEN if self.off.is_empty() => format!("{count} replies, all alike"),
            _ => format!("{count} replies{off}"),
        }
    }
}

/// Shares as whole percents that add up to 100, each rounded down and the
/// rest handed out to those with the largest remainders, so a list never
/// reads 99% or 101% in all.
fn in_percent(shares: Vec<(String, f64)>) -> Vec<(String, u32)> {
    let exact: Vec<f64> = shares.iter().map(|(_, share)| share * 100.0).collect();
    let mut whole: Vec<u32> = exact.iter().map(|pct| pct.floor() as u32).collect();
    let short = 100u32.saturating_sub(whole.iter().sum());
    let mut by_remainder: Vec<usize> = (0..exact.len()).collect();
    by_remainder.sort_by(|a, b| {
        let rest = |at: usize| exact[at] - exact[at].floor();
        rest(*b).total_cmp(&rest(*a))
    });
    for at in by_remainder.into_iter().take(short as usize) {
        whole[at] += 1;
    }
    shares
        .into_iter()
        .map(|(text, _)| text)
        .zip(whole)
        .collect()
}

/// How wide and high the shape is drawn, in CSS pixels, as `.style-shape`
/// has it: the pointer's place on it is read in these.
const SHAPE_PX: f64 = 240.0;

/// How far the drawing reaches from the middle either way, in the shape's
/// own units, its corners being one from the middle: room for the corners'
/// marks round it.
const SHAPE_REACH: f64 = 1.3;

/// How far an arrow key moves the dot, in the shape's units.
const SHAPE_STEP: f64 = 0.1;

/// How near a corner the pointer must be, in the shape's units, for a click
/// to turn its reply off or on rather than move the dot: about its mark.
const CORNER_HIT: f64 = 0.2;

/// The shape's point `x` and `y` pixels across and down the drawing, where
/// it is, in or out of the shape.
fn shape_unit(x: f64, y: f64) -> [f64; 2] {
    let unit = |px: f64| px / SHAPE_PX * 2.0 * SHAPE_REACH - SHAPE_REACH;
    [unit(x), unit(y)]
}

/// The point of the shape of `corners` corners under the pointer, `x` and
/// `y` pixels across and down the drawing, brought inside it.
fn shape_point(corners: usize, x: f64, y: f64) -> [f64; 2] {
    tidy(into_shape(corners, shape_unit(x, y)))
}

/// The corner of the shape of `corners` corners whose mark the pointer is
/// on, `x` and `y` pixels across and down the drawing, if any.
fn corner_under(corners: usize, x: f64, y: f64) -> Option<usize> {
    let [px, py] = shape_unit(x, y);
    shape_corners(corners)
        .iter()
        .position(|[cx, cy]| (px - cx).hypot(py - cy) <= CORNER_HIT)
}

/// A point kept to hundredths, so a save holds it as it reads.
fn tidy(at: [f64; 2]) -> [f64; 2] {
    at.map(|value| (value * 100.0).round() / 100.0)
}

/// How far the round palette the replies mix in reaches from its middle, in
/// the shape's units: just past the corners, so their glows sit at its rim.
const PALETTE_REACH: f64 = 1.12;

/// The colour of each reply, by its corner, as the stylesheet names them:
/// each sets the colour its corner's mark, glow and number are drawn in.
const MIX_COLOURS: [&str; 8] = [
    "mix-1", "mix-2", "mix-3", "mix-4", "mix-5", "mix-6", "mix-7", "mix-8",
];

/// The colour of the reply at corner `number`, from nought.
fn mix_colour(number: usize) -> &'static str {
    MIX_COLOURS.get(number).copied().unwrap_or_default()
}

/// How a reply's glow is drawn for its share, `share` against the largest
/// `most`: its reach from its corner and how strong it is, both by the
/// square root of the share, so a glow's area follows its share and a
/// small share still shows. None for a reply no hunter sends.
fn glow(share: f64, most: f64) -> Option<(f64, f64)> {
    if share <= 0.0 || most <= 0.0 {
        return None;
    }
    let part = (share / most).sqrt();
    Some((0.35 + 0.95 * part, 0.25 + 0.75 * part))
}

/// What a mouse over the shape is on: a point a click would put the dot
/// at, or a corner whose reply a click would turn off or on.
#[derive(Clone, Copy, PartialEq)]
enum Pointing {
    At([f64; 2]),
    Corner(usize),
}

/// Where the hunters' replies are set: a round palette with a reply at each
/// of its corners, numbered as the list under it numbers them and each in
/// a colour of its own, its glow as large and as strong as its share, so
/// the colours blend where the replies mix; and a dot inside it. In the
/// middle every reply on comes as often as the others; at a corner, that
/// one alone. A click on a corner's mark turns its reply off, or on again;
/// anywhere else a click or a drag puts the dot there, as a finger's touch
/// and drag do. A mouse over it shows what a click would do without doing
/// it (`hover`): the glows of the mix it would set and a hollow dot, or the
/// corner it would turn. The arrow keys move the dot, and Home brings it to
/// the middle. The colours are the validated categorical palette's, in its
/// order round the corners, each corner's number beside its colour.
#[component]
fn StyleShape(
    settings: Signal<TrainingSettings>,
    at: [f64; 2],
    off: Vec<usize>,
    shares: Vec<f64>,
    hover: Signal<Option<Pointing>>,
) -> Element {
    let mut hover = hover;
    let mut dragging = use_signal(|| false);
    let count = shares.len();
    let mut place = move |point: [f64; 2]| settings.write().contest.hunter_style = point;
    let turned = off.clone();
    let mut turn = move |corner: usize| {
        settings.write().contest.hunter_styles_off = toggle_corner(&turned, corner, count);
    };
    let corners = shape_corners(count);
    // What the mouse would set, where it is over the palette and not where
    // the dot already stands: its mix is the one drawn while it is there.
    let ghost = match hover() {
        Some(Pointing::At(point)) if point != at => Some(point),
        _ => None,
    };
    let shown = match ghost {
        Some(point) => shares_without(&corner_shares(count, point), &off),
        None => shares.clone(),
    };
    let most = shown.iter().copied().fold(0.0, f64::max);
    let hot = match hover() {
        Some(Pointing::Corner(corner)) => Some(corner),
        _ => None,
    };
    let view = format!(
        "{} {} {} {}",
        -SHAPE_REACH,
        -SHAPE_REACH,
        2.0 * SHAPE_REACH,
        2.0 * SHAPE_REACH
    );
    rsx! {
        svg {
            id: control_id("shape", "hunter styles"),
            class: if hot.is_some() { "style-shape on-corner" } else { "style-shape" },
            view_box: "{view}",
            tabindex: "0",
            role: "group",
            "aria-label": "Where the dot stands sets how often each reply comes: drag it, or move it with the arrow keys; a click on a numbered mark turns its reply off or on",
            onpointerdown: move |e: PointerEvent| {
                let point = e.element_coordinates();
                hover.set(None);
                if let Some(corner) = corner_under(count, point.x, point.y) {
                    turn(corner);
                    return;
                }
                dragging.set(true);
                place(shape_point(count, point.x, point.y));
            },
            onpointermove: move |e: PointerEvent| {
                let point = e.element_coordinates();
                if dragging() {
                    place(shape_point(count, point.x, point.y));
                } else if e.pointer_type() == "mouse" {
                    hover.set(Some(match corner_under(count, point.x, point.y) {
                        Some(corner) => Pointing::Corner(corner),
                        None => Pointing::At(shape_point(count, point.x, point.y)),
                    }));
                }
            },
            onpointerup: move |_| dragging.set(false),
            onpointerleave: move |_| {
                dragging.set(false);
                hover.set(None);
            },
            onkeydown: move |e: KeyboardEvent| {
                let [x, y] = at;
                let moved = match e.key() {
                    Key::ArrowLeft => [x - SHAPE_STEP, y],
                    Key::ArrowRight => [x + SHAPE_STEP, y],
                    Key::ArrowUp => [x, y - SHAPE_STEP],
                    Key::ArrowDown => [x, y + SHAPE_STEP],
                    Key::Home => STYLE_EVEN,
                    _ => return,
                };
                e.prevent_default();
                place(tidy(into_shape(count, moved)));
            },
            defs {
                for number in 0..count {
                    radialGradient {
                        id: "mix-glow-{number + 1}",
                        class: "{mix_colour(number)}",
                        stop {
                            class: "mix-stop",
                            offset: "0",
                            "stop-opacity": "0.9",
                        }
                        stop {
                            class: "mix-stop",
                            offset: "0.45",
                            "stop-opacity": "0.4",
                        }
                        stop {
                            class: "mix-stop",
                            offset: "1",
                            "stop-opacity": "0",
                        }
                    }
                }
                clipPath { id: "mix-palette",
                    circle { r: "{PALETTE_REACH}" }
                }
            }
            circle { class: "mix-disc", r: "{PALETTE_REACH}" }
            g { "clip-path": "url(#mix-palette)",
                for (number , (corner , share)) in corners.iter().zip(&shown).enumerate() {
                    if let Some((reach, strength)) = glow(*share, most) {
                        circle {
                            cx: "{corner[0]}",
                            cy: "{corner[1]}",
                            r: "{reach:.3}",
                            fill: "url(#mix-glow-{number + 1})",
                            opacity: "{strength:.3}",
                        }
                    }
                }
            }
            circle { class: "mix-rim", r: "{PALETTE_REACH}" }
            for (number , corner) in corners.iter().enumerate() {
                circle {
                    class: match (off.contains(&number), hot == Some(number)) {
                        (true, true) => "mix-marker off hot".to_string(),
                        (true, false) => "mix-marker off".to_string(),
                        (false, true) => format!("mix-marker hot {}", mix_colour(number)),
                        (false, false) => format!("mix-marker {}", mix_colour(number)),
                    },
                    cx: "{corner[0]}",
                    cy: "{corner[1]}",
                    r: "0.13",
                }
                text {
                    class: if off.contains(&number) { "mix-marker-number off" } else { "mix-marker-number" },
                    x: "{corner[0]}",
                    y: "{corner[1]}",
                    "{number + 1}"
                }
                if off.contains(&number) {
                    line {
                        class: "mix-marker-cross",
                        x1: "{corner[0] - 0.09}",
                        y1: "{corner[1] + 0.09}",
                        x2: "{corner[0] + 0.09}",
                        y2: "{corner[1] - 0.09}",
                    }
                }
            }
            if let Some(point) = ghost {
                circle {
                    class: "style-dot ghost",
                    cx: "{point[0]}",
                    cy: "{point[1]}",
                    r: "0.075",
                }
            }
            circle {
                class: "style-dot",
                cx: "{at[0]}",
                cy: "{at[1]}",
                r: "0.075",
            }
        }
    }
}

/// How POTA hunters reply, folded away under a line that says how it is
/// set, as the Band card folds its receiver. Open, the shape with a reply
/// at each corner and the dot that sets how often each comes, a line
/// saying what a click would do, and each reply whole, numbered as its
/// corner, beside how many in a hundred send it: one line at a corner,
/// every reply alike in the middle, one turned off kept in the list and
/// marked so. Each number turns its reply off or on, as a click on its
/// corner does. While a mouse is over the shape, the list shows the mix
/// it would set, marked as not set yet. What it does is in its ⓘ.
#[component]
fn HunterStyles(settings: Signal<TrainingSettings>, styles: StylesCard) -> Element {
    let mut open = use_signal(|| false);
    let mut help = use_signal(|| false);
    let hover = use_signal(|| None::<Pointing>);
    let reading = styles.reading();
    let count = styles.replies.len();
    // The mix the mouse would set, while it is over the shape somewhere
    // else than the dot and its corners.
    let preview = match hover() {
        Some(Pointing::At(point)) if point != styles.at => Some(point),
        _ => None,
    };
    let rows = match preview {
        Some(point) => styles.at_point(point),
        None => styles.replies.clone(),
    };
    let line = match hover() {
        Some(Pointing::Corner(corner)) if styles.off.contains(&corner) => {
            format!("Click to turn reply {} on again", corner + 1)
        }
        Some(Pointing::Corner(corner)) if styles.off.len() + 1 >= count => {
            format!("Reply {} is the last one on", corner + 1)
        }
        Some(Pointing::Corner(corner)) => format!("Click to turn reply {} off", corner + 1),
        _ if preview.is_some() => "Click to set this mix".to_string(),
        _ => {
            "Drag the dot toward the replies you want more of; click a corner to turn its reply off"
                .to_string()
        }
    };
    rsx! {
        div { class: "card-title-row",
            button {
                id: control_id(DISCLOSURE, "hunter styles"),
                class: if open() { "link-toggle open" } else { "link-toggle" },
                r#type: "button",
                aria_expanded: if open() { "true" } else { "false" },
                aria_controls: "hunter-styles",
                onclick: move |_| open.toggle(),
                "Hunter styles: {reading}"
                Icon { name: IconName::Chevron }
            }
            button {
                id: control_id("help", "hunter styles"),
                class: if help() { "icon-btn help-btn on" } else { "icon-btn help-btn" },
                r#type: "button",
                title: if help() { "Hide the help" } else { "What does this mean?" },
                aria_label: if help() { "Hide the help on hunter styles" } else { "Help on hunter styles" },
                aria_expanded: help(),
                onclick: move |_| help.toggle(),
                Icon { name: if help() { IconName::X } else { IconName::Info } }
            }
        }
        if help() {
            div { class: "help-note", role: "note", {help::hunter_styles()} }
        }
        if open() {
            div { id: "hunter-styles", class: "stack-sm",
                StyleShape {
                    settings,
                    at: styles.at,
                    off: styles.off.clone(),
                    shares: styles.shares.clone(),
                    hover,
                }
                p { class: "muted hint style-preview-line", "{line}" }
                ol {
                    class: if preview.is_some() { "qso style-replies preview" } else { "qso style-replies" },
                    aria_label: "The replies hunters send, and how often",
                    for (number , (reply , pct)) in rows.iter().enumerate() {
                        if styles.off.contains(&number) || *pct > 0 {
                            ReplyRow {
                                settings,
                                number,
                                count,
                                reply: reply.clone(),
                                pct: *pct,
                                off: styles.off.clone(),
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One reply in the list: its number, a button that turns it off or on
/// again as a click on its corner does, how many in a hundred send it, or
/// that it is off, and the reply whole.
#[component]
fn ReplyRow(
    settings: Signal<TrainingSettings>,
    number: usize,
    count: usize,
    reply: String,
    pct: u32,
    off: Vec<usize>,
) -> Element {
    let is_off = off.contains(&number);
    let shown = number + 1;
    let label = if is_off {
        format!("Turn reply {shown} on again")
    } else {
        format!("Turn reply {shown} off")
    };
    rsx! {
        li { class: if is_off { "qso-line they style-reply off" } else { "qso-line they style-reply" },
            span { class: "qso-who tiny",
                button {
                    id: control_id("reply", &shown.to_string()),
                    // Its corner's colour, round its number.
                    class: if is_off { "style-number off".to_string() } else { format!("style-number {}", mix_colour(number)) },
                    r#type: "button",
                    title: "{label}",
                    aria_label: "{label}",
                    aria_pressed: if is_off { "false" } else { "true" },
                    onclick: move |_| {
                        settings.write().contest.hunter_styles_off = toggle_corner(&off, number, count);
                    },
                    "{shown}"
                }
                if is_off {
                    "off"
                } else {
                    "{pct}%"
                }
            }
            span { class: "qso-sends",
                span { class: "call", "{reply}" }
            }
        }
    }
}

/// What the character-set card shows, worked out from the settings. A memo
/// of this is what the card reads, so it redraws when one of these moves and
/// not for a slider on another card.
#[derive(Clone, PartialEq)]
struct Charset {
    mode: CharSetMode,
    /// In Contest mode, the contest; none in any other.
    contest: Option<ContestCard>,
    callsigns: bool,
    sequence: String,
    choice: SequenceChoice,
    level: u32,
    level_max: u32,
    digits_level: u32,
    letters_percent: u32,
    unlocked: usize,
    window: PracticeWindow,
    pool: Vec<char>,
    /// A few calls of the tier, as Callsigns shows them; none in Contest,
    /// whose QSO shows a call.
    tier_examples: Vec<String>,
    /// The words Words sends, and whether they are the learner's own.
    words: Vec<String>,
    own_words: bool,
}

impl Charset {
    fn of(settings: &TrainingSettings) -> Self {
        let c = &settings.curriculum;
        let callsigns = c.char_set_mode.sends_callsigns();
        let contest = (c.char_set_mode == CharSetMode::Contest).then(|| ContestCard::of(settings));
        let tier_examples = if callsigns && contest.is_none() {
            tier_examples(settings.active_level())
        } else {
            Vec::new()
        };
        Self {
            mode: c.char_set_mode,
            contest,
            callsigns,
            sequence: settings.sequence().iter().collect(),
            choice: settings.sequence_choice(),
            level: settings.active_level(),
            level_max: settings.max_active_level(),
            digits_level: settings.level(LevelAxis::Digits),
            letters_percent: c.mixed_letters_percent,
            unlocked: unlocked_practice_count(settings),
            window: current_practice_window(settings),
            pool: compute_char_pool(settings),
            tier_examples,
            // Only Words shows them, and every settings write comes here.
            words: if c.char_set_mode == CharSetMode::Words {
                settings.words().into_iter().map(str::to_string).collect()
            } else {
                Vec::new()
            },
            own_words: settings.has_own_words(),
        }
    }

    fn note(&self) -> String {
        let what = match self.mode {
            CharSetMode::Koch => match self.choice {
                SequenceChoice::Preset(id) => format!("Unlocked in {} order", id.preset().name),
                SequenceChoice::Custom => "Unlocked in your own order".to_string(),
            },
            CharSetMode::Digits => "Digits only".to_string(),
            CharSetMode::Mixed => "Letters and digits together".to_string(),
            CharSetMode::Callsign => "Real callsigns, off the air".to_string(),
            // A contest is told by its name; its tier is the stepper's.
            CharSetMode::Contest => {
                let kind = self.contest.as_ref().map(|card| card.kind);
                return format!("Contest · {}", kind.unwrap_or_default().label());
            }
            CharSetMode::Words if self.own_words => "Your own words, sent whole".to_string(),
            CharSetMode::Words => "Starter words, sent whole".to_string(),
        };
        if self.callsigns {
            format!("{what} · tier {} of {CALLSIGN_TIER_MAX}", self.level)
        } else if self.mode == CharSetMode::Words {
            format!("{what} · {}", count_words(self.words.len()))
        } else {
            format!("{what} · {} in play", self.unlocked)
        }
    }
}

/// What a preset sets, in a line: "Callers at 15–18 WPM", kept in one
/// piece where a narrow line breaks.
fn preset_summary(preset: ContestPreset) -> String {
    let (slowest, fastest) = preset.char_wpm();
    format!("Callers at {} WPM", range_label(slowest, fastest)).replace(' ', "\u{a0}")
}

/// The character set card. In Contest mode it also plays Hear a QSO:
/// `hearing` while it plays, `dialogue` what it has played, `on_hear` to
/// start it and `on_stop` to stop it.
#[component]
pub fn CharsetCard(
    settings: Signal<TrainingSettings>,
    #[props(default)] hearing: bool,
    #[props(default)] dialogue: Option<Dialogue>,
    #[props(default)] on_hear: EventHandler<()>,
    #[props(default)] on_stop: EventHandler<()>,
) -> Element {
    let charset = use_memo(move || Charset::of(&settings.read()));
    let c = charset.read();
    let callsigns = c.callsigns;
    let words = c.mode == CharSetMode::Words;
    let has_unlock_order = matches!(c.mode, CharSetMode::Koch | CharSetMode::Mixed);
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Letters,
                title: "Character set",
                note: c.note(),
                help: help::character_set(),
                SectionReset {
                    card: "Character set",
                    settings,
                    section: SettingsSection::CharacterSet,
                }
            }
            div { class: "segmented charset-modes", role: "group", aria_label: "Character set",
                for (mode, label) in CHAR_SET_MODES {
                    Seg {
                        label,
                        active: c.mode == mode,
                        onclick: move |_| settings.write().set_char_set_mode(mode),
                    }
                }
            }
            if let Some(contest) = c.contest.clone() {
                ContestBody {
                    settings,
                    contest,
                    hearing,
                    dialogue,
                    on_hear,
                    on_stop,
                }
            }
            if has_unlock_order {
                div { class: "eyebrow", "Unlock order" }
                div { class: "mode-pills", role: "group", aria_label: "Unlock order",
                    for choice in SEQUENCE_PRESETS
                        .iter()
                        .map(|preset| (SequenceChoice::Preset(preset.id), preset.name))
                        .chain([(SequenceChoice::Custom, "Your own")])
                    {
                        Seg {
                            label: choice.1,
                            pill: true,
                            active: c.choice == choice.0,
                            onclick: move |_| settings.write().choose_sequence(choice.0),
                        }
                    }
                }
                // Cleaned down to what can be sent once it is committed, and
                // not before. Cleaned on every key, the box would move under
                // the cursor whenever a key was dropped, and could never be
                // emptied to start again: an empty unlock order is LCWO's, so
                // clearing it would fill it straight back up.
                TextField {
                    label: "Sequence order",
                    value: c.sequence.clone(),
                    on_commit: move |typed: String| settings.write().type_sequence(&typed),
                }
            }
            // A word list has no level: every word is in from the start.
            if !words {
            div { class: "field-grid",
                NumberField {
                    label: if callsigns {
                        format!("Callsign tier (1–{})", c.level_max)
                    } else {
                        format!("Level (1–{})", c.level_max)
                    },
                    id: "field-level",
                    value: f64::from(c.level),
                    min: 1.0,
                    max: f64::from(c.level_max),
                    step: 1.0,
                    onchange: move |v| settings.write().set_active_level(whole(v)),
                }
                if c.mode == CharSetMode::Mixed {
                    NumberField {
                        label: format!("Digits level (1–{MAX_DIGITS_LEVEL})"),
                        id: "field-digits-level",
                        value: f64::from(c.digits_level),
                        min: 1.0,
                        max: f64::from(MAX_DIGITS_LEVEL),
                        step: 1.0,
                        onchange: move |v| settings.write().curriculum.digits_level = whole(v),
                    }
                }
            }
            }
            if let Some(contest) = c.contest.clone() {
                RunStylePicker { settings, style: contest.style, callers: contest.callers }
                if let Some(tu_call) = contest.tu_call {
                    TuCallPicker { settings, tu_call }
                }
            }
            if c.mode == CharSetMode::Mixed {
                NumberField {
                    label: "Share of letters",
                    value: f64::from(c.letters_percent),
                    min: 0.0,
                    max: 100.0,
                    step: 5.0,
                    unit: "%",
                    onchange: move |v| settings.write().curriculum.mixed_letters_percent = whole(v),
                }
            }
            if words {
                WordListEditor {
                    settings,
                    words: c.words.clone(),
                    own: c.own_words,
                }
            } else if !c.tier_examples.is_empty() {
                div { class: "eyebrow", "What this tier sends" }
                div { class: "calls",
                    for call in c.tier_examples.iter() {
                        span { class: "call", "{call}" }
                    }
                    // A few of endless calls, not the lot.
                    span { class: "muted", aria_label: "and more", "…" }
                }
                p { class: "muted hint",
                    "Every tier keeps what the ones below it send, so moving up only adds. Prefixes are real allocations, weighted the way you hear them."
                }
            } else if !callsigns {
                div { class: "eyebrow", "Practice window" }
                div { class: "mode-pills", role: "group", aria_label: "Practice window",
                    for (window, label, needs) in PRACTICE_WINDOWS {
                        if c.unlocked >= needs {
                            Seg {
                                label,
                                pill: true,
                                active: c.window == window,
                                onclick: move |_| settings.write().curriculum.practice_window = window,
                            }
                        }
                    }
                }
            }
            // The characters groups are drawn from, one at a time. A word or
            // a callsign is sent whole, never a character of it on its own,
            // so for them the characters would say nothing.
            if !words && !callsigns {
                div { class: "chars", style: "gap: 4px;",
                    for ch in c.pool.iter() {
                        span { class: "ch sent-row", "{ch}" }
                    }
                }
            }
        }
    }
}

/// A contest in place of an alphabet: a pill for each contest, what you send
/// and copy, and a first QSO of it, line by line, with a few to hear (Hear a
/// QSO); then where to start from, a preset.
#[component]
fn ContestBody(
    settings: Signal<TrainingSettings>,
    contest: ContestCard,
    hearing: bool,
    dialogue: Option<Dialogue>,
    on_hear: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    // What Hear a QSO played, while it is of the contest chosen now: kept
    // once it ends, but not when it stopped before a line was heard.
    let heard = dialogue
        .filter(|dialogue| dialogue.kind == contest.kind)
        .filter(|dialogue| hearing || !dialogue.lines.is_empty());
    let eyebrow = heard
        .as_ref()
        .map_or_else(|| "A QSO".to_string(), |heard| heard_label(heard, hearing));
    rsx! {
        div { class: "eyebrow", "Contest" }
        div { class: "mode-pills", role: "group", aria_label: "Contest",
            for kind in ContestKind::ALL {
                Seg {
                    label: kind.label(),
                    id: control_id("contest", kind.label()),
                    pill: true,
                    active: contest.kind == kind,
                    onclick: move |_| settings.write().contest.contest_kind = kind,
                }
            }
        }
        // What you copy keeps to one piece, after the dot where a narrow
        // line breaks.
        p { class: "muted hint",
            "You send {contest.sends}\u{a0}· You\u{a0}copy\u{a0}{contest.copies}"
        }
        if let Some(side_note) = &contest.side_note {
            p { id: "side-line", class: "muted hint", "{side_note}" }
        }
        if let Some(simplified) = contest.simplified {
            p { class: "muted hint", "{simplified}" }
        }
        QsoHead { label: eyebrow,
            if hearing {
                button {
                    id: control_id("btn", "stop hearing"),
                    class: "btn btn-secondary btn-sm",
                    r#type: "button",
                    onclick: move |_| on_stop.call(()),
                    Icon { name: IconName::Stop }
                    "Stop"
                }
            } else {
                button {
                    id: control_id("btn", "hear a qso"),
                    class: "btn btn-secondary btn-sm",
                    r#type: "button",
                    onclick: move |_| on_hear.call(()),
                    Icon { name: IconName::Signal }
                    "Hear a QSO"
                }
            }
        }
        if let Some(heard) = heard {
            HeardDialogue { dialogue: heard, hearing }
        } else {
            QsoLines { lines: contest.qso.clone() }
        }
        if let Some(styles) = contest.styles.clone() {
            HunterStyles { settings, styles }
        }
        PresetPicker { settings, preset: contest.preset }
    }
}

/// What the line under the contest presets says while none is chosen.
pub(crate) const PRESET_CUSTOM: &str = "Custom · a preset sets the Speed card's speeds, which group training uses too, and nothing else";

/// Where a run starts from: a preset for a first QSO, a club operator or a
/// contester, each setting how fast the callers come and nothing else. That
/// is the Speed card's speeds, which group training sends at too, as a
/// contest has no speed of its own, and the line under the presets says so.
/// The settings read Custom once that is changed, as the Experience presets
/// do.
#[component]
fn PresetPicker(settings: Signal<TrainingSettings>, preset: Option<ContestPreset>) -> Element {
    let line = match preset {
        Some(preset) => preset_summary(preset),
        None => PRESET_CUSTOM.to_string(),
    };
    rsx! {
        div { class: "eyebrow", "Preset" }
        div { class: "mode-pills", role: "group", aria_label: "Preset",
            for each in ContestPreset::ALL {
                Seg {
                    label: each.name(),
                    id: control_id("preset", each.name()),
                    pill: true,
                    active: preset == Some(each),
                    onclick: move |_| each.apply(&mut settings.write()),
                }
            }
        }
        p { id: "preset-line", class: "muted hint", "{line}" }
    }
}

/// How a run goes: one station at a time, or a pile-up of as many as the
/// Band card's stations range draws, which the line under it names.
#[component]
fn RunStylePicker(settings: Signal<TrainingSettings>, style: RunStyle, callers: String) -> Element {
    let line = match style {
        RunStyle::Single => "One station calls at a time".to_string(),
        RunStyle::Pileup => format!("{callers} · set on the Band card"),
    };
    rsx! {
        div { class: "eyebrow", "Run style" }
        div { class: "mode-pills", role: "group", aria_label: "Run style",
            for (each, label) in RUN_STYLES {
                Seg {
                    label,
                    id: control_id("style", label),
                    pill: true,
                    active: style == each,
                    onclick: move |_| settings.write().set_contest_style(each),
                }
            }
        }
        p { id: "run-style-line", class: "muted hint", "{line}" }
    }
}

/// How often your TU signs with your call, with one caller at a time or a
/// pile-up: every QSO, every third, as MorseRunner's station ID rate has
/// it, or never. The callers call after your TU whichever it is.
#[component]
fn TuCallPicker(settings: Signal<TrainingSettings>, tu_call: TuCall) -> Element {
    let line = match tu_call {
        TuCall::Every => "Every TU signs with your call",
        TuCall::Third => {
            "Every third QSO's TU signs with your call, so a station tuning in hears who is running"
        }
        TuCall::Never => "No TU signs with your call; the callers still call after each",
    };
    rsx! {
        div { class: "eyebrow", "Your call in TU" }
        div { class: "mode-pills", role: "group", aria_label: "Your call in TU",
            for (each, label) in TU_CALLS {
                Seg {
                    label,
                    id: control_id("tu", label),
                    pill: true,
                    active: tu_call == each,
                    onclick: move |_| settings.write().contest.contest_tu_call = each,
                }
            }
        }
        p { id: "tu-call-line", class: "muted hint", "{line}" }
    }
}

/// "1 word", "412 words".
fn count_words(count: usize) -> String {
    counted(count, "word")
}

/// What reading a list came to, in a line: how many words it holds, and why
/// any were left out. `source` names the file it came from, if one did.
fn list_report(list: &WordList, source: Option<&str>) -> String {
    let mut parts = vec![count_words(list.words.len())];
    if list.unsendable > 0 {
        parts.push(format!(
            "{} left out: a character Morse has no code for, or longer than {GROUP_SIZE_MAX}",
            list.unsendable
        ));
    }
    if list.repeated > 0 {
        parts.push(format!("{} repeated", list.repeated));
    }
    if list.over > 0 {
        parts.push(format!(
            "{} past the {WORD_LIST_MAX} a list keeps",
            list.over
        ));
    }
    let report = parts.join(" · ");
    match source {
        Some(name) => format!("{name}: {report}"),
        None => report,
    }
}

/// Make `list` the words, and say what reading it came to. A list that kept
/// nothing leaves the words as they were, unless it was meant to be empty:
/// emptying the box goes back to the starter words. In the app a message
/// says so and offers Undo ([`AppCtx::starter_words`]), and the line under
/// the box goes back to the count, so that it is still right after Undo.
fn take_list(
    app: Option<AppCtx>,
    mut settings: Signal<TrainingSettings>,
    mut report: Signal<Option<String>>,
    text: &str,
    source: Option<&str>,
) {
    let list = WordList::read(text);
    if list.words.is_empty() && list.left_out() == 0 {
        match app {
            Some(app) => {
                report.set(None);
                app.starter_words();
            }
            None => {
                settings.write().curriculum.word_list.clear();
                report.set(Some("Back to the starter words.".to_string()));
            }
        }
        return;
    }
    if list.words.is_empty() {
        let what = source.unwrap_or("The list");
        report.set(Some(format!(
            "{what}: no word in it can be sent, so the words are as they were."
        )));
        return;
    }
    report.set(Some(list_report(&list, source)));
    settings.write().curriculum.word_list = list.words;
}

/// Take a chosen file as the word list.
fn take_word_file(
    app: Option<AppCtx>,
    settings: Signal<TrainingSettings>,
    mut report: Signal<Option<String>>,
    file: ChosenFile,
) {
    let ChosenFile { name, size, text } = file;
    if size > WORD_FILE_MAX_BYTES {
        report.set(Some(format!(
            "{name} is over a megabyte, far more than a word list: nothing was read."
        )));
        return;
    }
    match text {
        Some(text) => take_list(app, settings, report, &text, Some(&name)),
        None => report.set(Some(format!(
            "{name} could not be read as text: is it a plain .txt file?"
        ))),
    }
}

/// The words Words sends: typed or pasted in, a word to a line, or loaded
/// from a text file.
///
/// What is typed stays as typed until it is committed, on leaving the box,
/// and is then read as a list: upper-cased, each word once, what cannot be
/// sent left out and said. A key does not redraw the box ([`TextDraft`]),
/// so the count of what is being typed is drawn beside it instead. The
/// paddles key nothing into it, so the keys right of P type there: the
/// starter list's `+` and `=` on any keyboard
/// ([`crate::ui::paddle::TypedField`]).
#[component]
fn WordListEditor(settings: Signal<TrainingSettings>, words: Vec<String>, own: bool) -> Element {
    let app = try_use_context::<AppCtx>();
    let draft = TextDraft::use_draft();
    let typed = use_typed_field();
    // The text being typed again, for the count under the box, which alone
    // reads it: the box goes by the draft, which redraws nothing.
    let mut counting = use_signal(|| None::<String>);
    let report = use_signal(|| None::<String>);
    // Every commit puts the list back in the box, even when the list did not
    // change: text that kept nothing goes.
    let mut settle = move || {
        draft.settle();
        counting.set(None);
    };
    // The starter list, unfolded.
    let mut show_list = use_signal(|| false);
    let shown = draft.shown(|| if own { words.join("\n") } else { String::new() });
    rsx! {
        div { class: "eyebrow", "Your words" }
        textarea {
            id: control_id("field", "word list"),
            class: "group-text word-list",
            rows: "7",
            value: "{shown}",
            placeholder: "Paste or type your words here, one to a line",
            aria_label: "Your words, one to a line",
            autocapitalize: "characters",
            spellcheck: false,
            "data-typed": "true",
            onfocus: move |_| typed.focus(),
            onblur: move |_| typed.blur(),
            oninput: move |e| {
                draft.hold(e.value());
                counting.set(Some(e.value()));
            },
            onchange: move |e| {
                settle();
                take_list(app, settings, report, &e.value(), None);
            },
        }
        div { class: "word-tools",
            FileButton {
                id: control_id("file", "word list"),
                accept: ".txt,text/plain",
                max_bytes: WORD_FILE_MAX_BYTES,
                icon: IconName::Upload,
                label: "Load a text file",
                on_choose: move |file| {
                    settle();
                    take_word_file(app, settings, report, file);
                },
            }
            if own {
                button {
                    id: control_id("btn", "starter words"),
                    class: "btn btn-secondary btn-sm",
                    r#type: "button",
                    title: "Send the starter words instead of yours",
                    onclick: move |_| {
                        settle();
                        take_list(app, settings, report, "", None);
                    },
                    Icon { name: IconName::Reset }
                    "Starter words"
                }
            }
        }
        WordCount { typing: counting, report, count: words.len(), own }
        if !own {
            p { class: "muted hint",
                "Until you add your own, Words sends a starter list of the abbreviations and Q-codes a contact is made of."
            }
            // How many words is the summary; the list itself is there to
            // look through when wanted, folded away until then.
            button {
                id: control_id(DISCLOSURE, "starter list"),
                class: if show_list() { "link-toggle open" } else { "link-toggle" },
                style: "align-self: flex-start;",
                aria_expanded: if show_list() { "true" } else { "false" },
                aria_controls: "starter-list",
                onclick: move |_| show_list.toggle(),
                "{count_words(STARTER_WORDS.len())} to draw from"
                Icon { name: IconName::Chevron }
            }
            if show_list() {
                div { id: "starter-list", class: "calls",
                    for word in STARTER_WORDS.iter() {
                        span { class: "call", "{word}" }
                    }
                }
            }
        }
    }
}

/// The line under the word box: what is being typed comes to, while it is,
/// and otherwise what the last list read came to, or how many words there
/// are.
#[component]
fn WordCount(
    typing: Signal<Option<String>>,
    report: Signal<Option<String>>,
    count: usize,
    own: bool,
) -> Element {
    let line = match typing() {
        Some(text) => format!(
            "{} — taken when you leave the box",
            list_report(&WordList::read(&text), None)
        ),
        None => report().unwrap_or_else(|| {
            if own {
                count_words(count)
            } else {
                String::new()
            }
        }),
    };
    rsx! {
        p { class: "word-report", aria_live: "polite", "{line}" }
    }
}

#[cfg(test)]
mod tests {
    use super::{SHAPE_PX, SHAPE_REACH, StylesCard, corner_under, glow, in_percent, shape_point};
    use cw_core::{STYLE_EVEN, STYLE_ON_AIR, shape_corners};

    /// Shares read as whole percents that add up to 100, the rest going to
    /// the largest remainders.
    #[test]
    fn shares_read_as_whole_percents_that_make_a_hundred() {
        let pct = |shares: &[f64]| -> Vec<u32> {
            let named = shares.iter().map(|share| (String::new(), *share)).collect();
            in_percent(named).into_iter().map(|(_, pct)| pct).collect()
        };
        assert_eq!(pct(&[0.45, 0.3, 0.15, 0.1]), [45, 30, 15, 10]);
        assert_eq!(pct(&[0.7, 0.3]), [70, 30]);
        assert_eq!(pct(&[1.0 / 3.0; 3]), [34, 33, 33]);
        assert_eq!(pct(&[0.004, 0.996]), [0, 100]);
        for shares in [[0.333, 0.333, 0.334], [0.256, 0.248, 0.496]] {
            assert_eq!(pct(&shares).iter().sum::<u32>(), 100, "{shares:?}");
        }
    }

    /// The folded line says how many replies come and where the dot
    /// stands: one reply at a corner, eight as on the air where it stands
    /// until moved, eight alike in the middle; the list keeps the corners'
    /// order and makes a hundred, and a point the mouse is over gives the
    /// shares it would set.
    #[test]
    fn the_styles_read_by_the_replies_that_come() {
        let card = |at| {
            let styles = cw_core::ContestKind::Pota
                .rules()
                .caller_styles(cw_core::Side::Dx, "SA0KAM", at, &[])
                .expect("POTA's hunters vary");
            StylesCard::of(styles, at, &[])
        };
        let one = card(shape_corners(8)[0]);
        assert_eq!(one.reading(), "One reply");
        assert_eq!(one.replies_heard(), 1);
        assert_eq!(
            one.replies[0],
            ("BK SA0KAM TU UR 579 579 OR OR BK".to_string(), 100)
        );
        let on_air = card(STYLE_ON_AIR);
        assert_eq!(on_air.reading(), "8 replies, as on the air");
        assert_eq!(on_air.replies.iter().map(|(_, pct)| pct).sum::<u32>(), 100);
        assert_eq!(on_air.replies[0].0, "BK SA0KAM TU UR 579 579 OR OR BK");
        assert_eq!(on_air.replies[7].0, "BK SA0KAM TU UR 579 OR BK");
        let even = card(STYLE_EVEN);
        assert_eq!(even.reading(), "8 replies, all alike");
        assert!(even.replies.iter().all(|(_, pct)| (12..=13).contains(pct)));
        assert_eq!(card([0.2, 0.3]).reading(), "8 replies");
        // What the mouse would set: the middle's from where it stands.
        assert_eq!(on_air.at_point(STYLE_EVEN), even.replies);
        assert_eq!(on_air.at_point(shape_corners(8)[0]), one.replies);
    }

    /// Replies turned off are said on the folded line, and the mouse's
    /// preview leaves them out too.
    #[test]
    fn replies_turned_off_are_said_and_left_out() {
        let off = [1, 2];
        let styles = cw_core::ContestKind::Pota
            .rules()
            .caller_styles(cw_core::Side::Dx, "SA0KAM", STYLE_ON_AIR, &off)
            .expect("POTA's hunters vary");
        let card = StylesCard::of(styles, STYLE_ON_AIR, &off);
        assert_eq!(card.reading(), "6 replies, 2 off");
        assert_eq!((card.replies[1].1, card.replies[2].1), (0, 0));
        let even = card.at_point(STYLE_EVEN);
        assert_eq!((even[1].1, even[2].1), (0, 0));
        assert!(
            even.iter()
                .all(|(_, pct)| *pct == 0 || (16..=17).contains(pct))
        );
    }

    /// A reply's glow grows with the square root of its share, all the way
    /// for the largest, and a reply no hunter sends has none.
    #[test]
    fn a_glow_follows_its_share() {
        let (largest, full) = glow(0.5, 0.5).expect("a glow");
        assert!((largest - 1.3).abs() < 1e-9 && (full - 1.0).abs() < 1e-9);
        let (quarter, strength) = glow(0.125, 0.5).expect("a glow");
        assert!((quarter - (0.35 + 0.95 * 0.5)).abs() < 1e-9);
        assert!((strength - (0.25 + 0.75 * 0.5)).abs() < 1e-9);
        assert_eq!(glow(0.0, 0.5), None);
        assert_eq!(glow(0.2, 0.0), None);
    }

    /// A corner's mark is where a click turns its reply: on it, or near
    /// enough; not the middle, nor between two corners.
    #[test]
    fn a_corner_is_under_the_pointer_on_its_mark() {
        let at = |unit: [f64; 2]| {
            let px = |value: f64| (value + SHAPE_REACH) / (2.0 * SHAPE_REACH) * SHAPE_PX;
            corner_under(8, px(unit[0]), px(unit[1]))
        };
        assert_eq!(at([0.0, -1.0]), Some(0));
        assert_eq!(at([0.1, -0.95]), Some(0));
        assert_eq!(at([1.0, 0.0]), Some(2));
        assert_eq!(at([0.0, 0.0]), None);
        assert_eq!(at([0.0, -0.6]), None);
    }

    /// The pointer's place on the drawing is the shape's point under it:
    /// the drawing's middle is the shape's, and its top edge past the top
    /// corner, which a point beyond is brought back to.
    #[test]
    fn the_pointer_places_the_dot_under_it() {
        assert_eq!(shape_point(8, SHAPE_PX / 2.0, SHAPE_PX / 2.0), [0.0, 0.0]);
        assert_eq!(shape_point(8, SHAPE_PX / 2.0, 0.0), [0.0, -1.0]);
        let right = shape_point(8, SHAPE_PX, SHAPE_PX / 2.0);
        assert_eq!(right, [1.0, 0.0], "an octagon has a corner there");
        assert_eq!(shape_point(8, SHAPE_PX * 0.75, SHAPE_PX / 2.0), [0.65, 0.0]);
    }
}
