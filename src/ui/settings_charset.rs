use cw_core::{
    CALLSIGN_TIER_MAX, CallerOdds, CharSetMode, ContestKind, ContestPreset, FAREWELLS, Fill,
    GROUP_SIZE_MAX, LevelAxis, MAX_DIGITS_LEVEL, Odd, PracticeWindow, RunStyle, SEQUENCE_PRESETS,
    STARTER_WORDS, STYLE_EVEN, Sender, SequenceChoice, SettingsSection, TrainingSettings, TuCall,
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
use crate::ui::odds::{OddsFold, OddsGroup};
use crate::ui::paddle::use_typed_field;
use crate::ui::widgets::{
    CardHead, DISCLOSURE, Icon, IconName, NumberField, SectionReset, Seg, SliderField, TextDraft,
    TextField, control_id, range_label, whole,
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
    /// How often callers do each thing they do now and then, and which of
    /// those shares this contest has: the groups of its Caller odds.
    odds: CallerOdds,
    odds_groups: Vec<OddsGroup>,
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
            styles: {
                let (at, off) = (contest.style_at(kind), contest.styles_off(kind));
                rules
                    .caller_styles(side, call_or_stand_in(&contest.my_call), at, off)
                    .map(|styles| {
                        let dx = (kind == ContestKind::Pota).then_some(contest.hunter_dx_percent);
                        StylesCard::of(styles, kind, at, off, dx)
                    })
            },
            odds: contest.caller_odds,
            odds_groups: odds_groups(rules, style),
            preset,
        }
    }
}

/// The groups of shares a contest's Caller odds hold: who calls, how
/// pile-up callers call (only in a pile-up), how callers ask and put your
/// call right, and how your TU signs off, each only where the contest has
/// it: dupes where a contest scores, DX where Field Day's callers send it,
/// `NR?` where the contest's queries differ, `73 44` where your TU signs
/// off with a farewell.
fn odds_groups(rules: &cw_core::ContestRules, style: RunStyle) -> Vec<OddsGroup> {
    let mut who = Vec::new();
    if rules.scoring.is_some() {
        who.push(Odd::Dupe);
    }
    who.extend([Odd::Portable, Odd::Abroad]);
    if rules.kind == ContestKind::FieldDay {
        who.push(Odd::FieldDayDx);
    }
    let mut asking = Vec::new();
    if rules.their_query[0] != rules.their_query[1] {
        asking.push(Odd::AsksNr);
    }
    asking.push(Odd::FixesWithExchange);
    let mut groups = vec![
        OddsGroup {
            head: "Who calls",
            odds: who,
            disabled: false,
        },
        OddsGroup {
            head: "Pile-up callers",
            odds: vec![Odd::CallsTwice, Odd::MissesExchange],
            disabled: style != RunStyle::Pileup,
        },
        OddsGroup {
            head: "Asking and putting your call right",
            odds: asking,
            disabled: false,
        },
    ];
    if rules.tu.wve.contains("{bye}") || rules.tu.dx.contains("{bye}") {
        groups.push(OddsGroup {
            head: "Your TU",
            odds: vec![Odd::Farewell44],
            disabled: false,
        });
    }
    groups
}

/// How a contest's callers reply, as the card shows it: where you put the
/// knob in the shape of their replies, which you turned off, how many of
/// the hundred send each, and each reply, a corner each, in the corners'
/// order: whole, and apart as its corner names it.
#[derive(Clone, PartialEq)]
struct StylesCard {
    /// The contest whose callers these are.
    kind: ContestKind,
    /// Where the knob stands, across and down from the middle.
    at: [f64; 2],
    /// Where it stands until moved: the mix heard on the air.
    on_air: [f64; 2],
    /// What the right side of the shape is headed with, and the left.
    heads: [&'static str; 2],
    /// How many POTA hunters in a hundred are DX; none in any other
    /// contest.
    dx_percent: Option<u32>,
    /// The replies turned off, by their corners from the top right, from
    /// nought.
    off: Vec<usize>,
    /// The share of hunters that send each reply, exactly, for the shape.
    shares: Vec<f64>,
    /// Each reply, whole, and how many in a hundred send it, in whole
    /// percents that make a hundred.
    replies: Vec<(String, u32)>,
    /// The words each reply starts with, beside its corner.
    leads: Vec<String>,
    /// Each reply's exchange after its first words: the right side's and
    /// the left's head the shape.
    exchanges: Vec<String>,
}

impl StylesCard {
    fn of(
        styles: cw_core::CallerStyles,
        kind: ContestKind,
        at: [f64; 2],
        off: &[usize],
        dx_percent: Option<u32>,
    ) -> Self {
        let shares = styles.replies.iter().map(|(_, share)| *share).collect();
        Self {
            kind,
            at,
            on_air: styles.on_air,
            heads: styles.heads,
            dx_percent,
            off: off.to_vec(),
            shares,
            replies: in_percent(styles.replies),
            leads: styles.leads,
            exchanges: styles.exchanges,
        }
    }

    /// The share of hunters that would send each reply, the dot standing
    /// `at` instead: what the shape and the list show while a mouse is over
    /// it.
    fn shares_at(&self, at: [f64; 2]) -> Vec<f64> {
        shares_without(&corner_shares(self.replies.len(), at), &self.off)
    }

    /// Each reply and how many in a hundred would send it, the dot standing
    /// `at` instead.
    fn at_point(&self, at: [f64; 2]) -> Vec<(String, u32)> {
        let named = self
            .replies
            .iter()
            .map(|(reply, _)| reply.clone())
            .zip(self.shares_at(at))
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
            at if at == self.on_air && self.off.is_empty() => {
                format!("{count} replies, as on the air")
            }
            STYLE_EVEN if self.off.is_empty() => format!("{count} replies, all alike"),
            _ => format!("{count} replies{off}"),
        }
    }

    /// Who the callers are, as the card names them: POTA's hunt.
    fn who(&self) -> &'static str {
        if self.kind == ContestKind::Pota {
            "hunter styles"
        } else {
            "caller styles"
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

/// How wide the drawing is, in CSS pixels: narrow enough for a phone's
/// card. Its height follows from what it shows ([`ShapeView`]), so the
/// pointer's place on it reads in its own units.
const SHAPE_PX: f64 = 288.0;

/// Each corner's pill, in the shape's units, its corners being one from
/// the middle: its width and height.
const PILL_W: f64 = 0.32;
const PILL_H: f64 = 0.18;

/// The size of the words beside each pill, in the shape's units, and the
/// width of each of their letters, the code face's being six tenths of it.
const LABEL_SIZE: f64 = 0.112;
const LABEL_LETTER: f64 = 0.6 * LABEL_SIZE;

/// The gap between a pill and the words beside it.
const LABEL_GAP: f64 = 0.05;

/// The size of each side's exchange over the shape, and of its letters.
const CAPTION_SIZE: f64 = 0.095;
const CAPTION_LETTER: f64 = 0.6 * CAPTION_SIZE;

/// Where the drawing starts above the middle, with room for the sides'
/// heads over the top pills, and where it ends below, past the bottom ones.
const SHAPE_TOP: f64 = -1.46;
const SHAPE_BOTTOM: f64 = 1.08;

/// How far an arrow key moves the dot, in the shape's units.
const SHAPE_STEP: f64 = 0.1;

/// The words beside a corner whose reply starts with `lead`: an empty lead,
/// a reply that starts with its exchange, is a dash.
fn lead_label(lead: &str) -> &str {
    if lead.is_empty() { "—" } else { lead }
}

/// The part of the shape's own units the drawing shows: as wide as the
/// words beside its widest pill reach either way, so a long call still
/// fits, and from the sides' heads down past the bottom pills.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShapeView {
    /// How far it reaches either side of the middle.
    half: f64,
}

impl ShapeView {
    fn of(leads: &[String], exchanges: &[String]) -> Self {
        let corners = shape_corners(leads.len());
        let labels = corners.iter().zip(leads).map(|(corner, lead)| {
            corner[0].abs()
                + PILL_W / 2.0
                + LABEL_GAP
                + lead_label(lead).chars().count() as f64 * LABEL_LETTER
        });
        let captions = exchanges
            .iter()
            .map(|exchange| exchange.chars().count() as f64 * CAPTION_LETTER);
        let half = labels.chain(captions).fold(1.2, f64::max) + 0.04;
        Self { half }
    }

    fn width(self) -> f64 {
        2.0 * self.half
    }

    fn height(self) -> f64 {
        SHAPE_BOTTOM - SHAPE_TOP
    }

    /// Its height in CSS pixels, at [`SHAPE_PX`] wide.
    fn height_px(self) -> f64 {
        SHAPE_PX * self.height() / self.width()
    }

    fn view_box(self) -> String {
        format!(
            "{} {} {} {}",
            -self.half,
            SHAPE_TOP,
            self.width(),
            self.height()
        )
    }

    /// The shape's point `x` and `y` pixels across and down the drawing,
    /// where it is, in or out of the shape.
    fn unit(self, x: f64, y: f64) -> [f64; 2] {
        let scale = self.width() / SHAPE_PX;
        [-self.half + x * scale, SHAPE_TOP + y * scale]
    }

    /// The point of the shape of `corners` corners under the pointer, `x`
    /// and `y` pixels across and down the drawing, brought inside it
    /// ([`settle`]).
    fn point(self, corners: usize, x: f64, y: f64) -> [f64; 2] {
        settle(corners, self.unit(x, y))
    }

    /// Where on the drawing, in pixels across and down, the shape's point
    /// `at` is: what a test points at.
    #[cfg(test)]
    pub(crate) fn px(self, at: [f64; 2]) -> (f64, f64) {
        let scale = SHAPE_PX / self.width();
        ((at[0] + self.half) * scale, (at[1] - SHAPE_TOP) * scale)
    }
}

/// How `kind`'s card draws the shape for `my_call`: what a test points at.
#[cfg(test)]
pub(crate) fn styles_view(kind: ContestKind, my_call: &str) -> ShapeView {
    let rules = kind.rules();
    let styles = rules
        .caller_styles(cw_core::Side::Dx, my_call, rules.styles_on_air(), &[])
        .expect("its callers vary");
    ShapeView::of(&styles.leads, &styles.exchanges)
}

/// How near a corner, in the shape's units, the knob goes onto it: a
/// corner's place is no round number, and the knob there sends that reply
/// alone.
const CORNER_SNAP: f64 = 0.05;

/// `at` as the knob takes it, in the shape of `corners` corners: brought
/// inside it, onto a corner where it is that near one, and anywhere else
/// kept to hundredths, so a save holds it as it reads.
fn settle(corners: usize, at: [f64; 2]) -> [f64; 2] {
    let at = into_shape(corners, at);
    shape_corners(corners)
        .into_iter()
        .find(|corner| (corner[0] - at[0]).hypot(corner[1] - at[1]) < CORNER_SNAP)
        .unwrap_or_else(|| tidy(at))
}

/// The corner whose pill, or the words beside it, `at` is on, of the
/// replies starting with `leads`, if any: a click there turns its reply
/// rather than moving the dot.
fn corner_under(leads: &[String], at: [f64; 2]) -> Option<usize> {
    let corners = shape_corners(leads.len());
    corners.iter().zip(leads).position(|(corner, lead)| {
        let (across, down) = (at[0] - corner[0], (at[1] - corner[1]).abs());
        let on_pill = across.abs() <= PILL_W / 2.0 + 0.04 && down <= PILL_H / 2.0 + 0.04;
        // The words go outward, away from the middle.
        let out = across * corner[0].signum();
        let reach =
            PILL_W / 2.0 + LABEL_GAP + lead_label(lead).chars().count() as f64 * LABEL_LETTER;
        let on_label = out > 0.0 && out <= reach && down <= PILL_H / 2.0;
        on_pill || on_label
    })
}

/// A point kept to hundredths, so a save holds it as it reads.
fn tidy(at: [f64; 2]) -> [f64; 2] {
    at.map(|value| (value * 100.0).round() / 100.0)
}

/// How strongly the face is shaded toward a reply's corner for its share,
/// `share` against the largest `most`: at the corner, and at the middle,
/// where it fades to little. None for a reply no hunter sends.
fn shade(share: f64, most: f64) -> Option<(f64, f64)> {
    if share <= 0.0 || most <= 0.0 {
        return None;
    }
    let corner = 0.08 + 0.62 * share / most;
    Some((corner, 0.15 * corner))
}

/// How a corner's pill is tinted for its share against the largest: the
/// largest is copper through, the rest a wash of it as strong as their
/// share.
fn tint(share: f64, most: f64) -> f64 {
    if most <= 0.0 {
        0.0
    } else {
        0.08 + 0.5 * share / most
    }
}

/// The reply at `corner` of `kind`'s `count`, turned off, or on again if it
/// was off; never the last one on ([`toggle_corner`]).
fn turn_corner(
    mut settings: Signal<TrainingSettings>,
    kind: ContestKind,
    corner: usize,
    count: usize,
) {
    let off = settings.peek().contest.styles_off(kind).to_vec();
    settings
        .write()
        .contest
        .set_styles_off(kind, toggle_corner(&off, corner, count));
}

/// What a mouse over the shape is on: a point a click would put the dot
/// at, or a corner whose reply a click would turn off or on.
#[derive(Clone, Copy, PartialEq)]
enum Pointing {
    At([f64; 2]),
    Corner(usize),
}

/// Where the hunters' replies are set: an eight-cornered shape standing on
/// an edge, a reply at each corner with a pill saying how many in a hundred
/// send it and the words it starts with beside it, the right side sending
/// report and state twice and the left side each once, as the heads over
/// it say, so each row across starts alike; and a knob inside it. The face
/// is shaded in copper toward each corner as strongly as its share, so the
/// shading follows the knob as the replies mix. In the middle every reply
/// on comes as often as the others; at a corner, that one alone. A click on
/// a corner's pill or words turns its reply off, or on again; anywhere else
/// a click or a drag puts the knob there, as a finger's touch and drag do.
/// A mouse over it shows what a click would do without doing it (`hover`):
/// the mix it would set and a dashed ring, or the corner it would turn. The
/// arrow keys move the knob and Home brings it to the middle; Tab reaches
/// each corner, and Enter or Space turns it.
#[component]
fn StyleShape(
    settings: Signal<TrainingSettings>,
    styles: StylesCard,
    hover: Signal<Option<Pointing>>,
) -> Element {
    let mut hover = hover;
    let mut dragging = use_signal(|| false);
    let count = styles.shares.len();
    let at = styles.at;
    let off = styles.off.clone();
    let view = ShapeView::of(&styles.leads, &styles.exchanges);
    let kind = styles.kind;
    let mut place = move |point: [f64; 2]| settings.write().contest.set_style_at(kind, point);
    let leads = styles.leads.clone();
    let pointed_leads = styles.leads.clone();
    let corners = shape_corners(count);
    let outline = corners
        .iter()
        .map(|[x, y]| format!("{x:.4},{y:.4}"))
        .collect::<Vec<String>>()
        .join(" ");
    // What the mouse would set, where it is over the shape and not where
    // the knob already stands: its mix is the one drawn while it is there.
    let ghost = match hover() {
        Some(Pointing::At(point)) if point != at => Some(point),
        _ => None,
    };
    let shown = match ghost {
        Some(point) => styles.shares_at(point),
        None => styles.shares.clone(),
    };
    let shown_pct = in_percent(shown.iter().map(|share| (String::new(), *share)).collect());
    let most = shown.iter().copied().fold(0.0, f64::max);
    let hot = match hover() {
        Some(Pointing::Corner(corner)) => Some(corner),
        _ => None,
    };
    let (right, left) = (
        styles.exchanges.first().cloned().unwrap_or_default(),
        styles.exchanges.last().cloned().unwrap_or_default(),
    );
    let edge = view.half - 0.02;
    rsx! {
        svg {
            id: control_id("shape", styles.who()),
            class: if hot.is_some() { "style-shape on-corner" } else { "style-shape" },
            view_box: "{view.view_box()}",
            width: "{SHAPE_PX}",
            height: "{view.height_px():.1}",
            tabindex: "0",
            role: "group",
            "aria-label": "Where the knob stands sets how often each reply comes: drag it, or move it with the arrow keys. Tab to a corner and press Enter to turn its reply off or on",
            onpointerdown: move |e: PointerEvent| {
                let point = e.element_coordinates();
                hover.set(None);
                if let Some(corner) = corner_under(&leads, view.unit(point.x, point.y)) {
                    turn_corner(settings, kind, corner, count);
                    return;
                }
                dragging.set(true);
                place(view.point(count, point.x, point.y));
            },
            onpointermove: move |e: PointerEvent| {
                let point = e.element_coordinates();
                if dragging() {
                    place(view.point(count, point.x, point.y));
                } else if e.pointer_type() == "mouse" {
                    let under = corner_under(&pointed_leads, view.unit(point.x, point.y));
                    hover
                        .set(
                            Some(
                                match under {
                                    Some(corner) => Pointing::Corner(corner),
                                    None => Pointing::At(view.point(count, point.x, point.y)),
                                },
                            ),
                        );
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
                place(settle(count, moved));
            },
            defs {
                radialGradient {
                    id: "style-face",
                    cx: "0",
                    cy: "0",
                    r: "1",
                    "gradientUnits": "userSpaceOnUse",
                    stop { class: "style-face-in", offset: "0" }
                    stop { class: "style-face-out", offset: "1" }
                }
                for (number , (corner , share)) in corners.iter().zip(&shown).enumerate() {
                    if let Some((strong, faint)) = shade(*share, most) {
                        linearGradient {
                            id: "style-shade-{number + 1}",
                            "gradientUnits": "userSpaceOnUse",
                            x1: "0",
                            y1: "0",
                            x2: "{corner[0]:.4}",
                            y2: "{corner[1]:.4}",
                            stop {
                                class: "style-shade",
                                offset: "0",
                                "stop-opacity": "{faint:.3}",
                            }
                            stop {
                                class: "style-shade",
                                offset: "1",
                                "stop-opacity": "{strong:.3}",
                            }
                        }
                    }
                }
            }
            text { class: "style-head", x: "{edge}", y: "-1.36", "text-anchor": "end",
                "{styles.heads[0]}"
            }
            text { class: "style-side", x: "{edge}", y: "-1.22", "text-anchor": "end",
                "{right}"
            }
            text { class: "style-head", x: "{-edge}", y: "-1.36", "{styles.heads[1]}" }
            text { class: "style-side", x: "{-edge}", y: "-1.22", "{left}" }
            polygon { class: "style-face", points: "{outline}" }
            // Each corner's part of the face, from the middle out to the
            // corner and halfway along each edge from it, shaded toward it.
            for (number , corner) in corners.iter().enumerate() {
                if shade(shown[number], most).is_some() {
                    {
                        let before = corners[(number + count - 1) % count];
                        let after = corners[(number + 1) % count];
                        let half = |other: [f64; 2]| {
                            [(corner[0] + other[0]) / 2.0, (corner[1] + other[1]) / 2.0]
                        };
                        let ([ax, ay], [bx, by]) = (half(before), half(after));
                        rsx! {
                            path {
                                fill: "url(#style-shade-{number + 1})",
                                d: "M0,0 L{ax:.4},{ay:.4} L{corner[0]:.4},{corner[1]:.4} L{bx:.4},{by:.4} Z",
                            }
                        }
                    }
                }
            }
            polygon { class: "style-rim", points: "{outline}" }
            for (number , corner) in corners.iter().enumerate() {
                {
                    let is_off = off.contains(&number);
                    let is_top = !is_off && shown[number] >= most && most > 0.0;
                    let pill = match (is_off, is_top, hot == Some(number)) {
                        (true, _, true) => "style-pill off hot",
                        (true, _, false) => "style-pill off",
                        (false, true, true) => "style-pill top hot",
                        (false, true, false) => "style-pill top",
                        (false, false, true) => "style-pill hot",
                        (false, false, false) => "style-pill",
                    };
                    let outward = if corner[0] < 0.0 { -1.0 } else { 1.0 };
                    let label_x = corner[0] + outward * (PILL_W / 2.0 + LABEL_GAP);
                    let lead = lead_label(styles.leads.get(number).map_or("", String::as_str))
                        .to_string();
                    let pct = shown_pct.get(number).map_or(0, |(_, pct)| *pct);
                    let shown_number = number + 1;
                    let whole = styles.replies.get(number).map_or("", |(reply, _)| reply.as_str());
                    let label = if is_off {
                        format!("Turn {whole} on again")
                    } else {
                        format!("Turn {whole} off")
                    };
                    rsx! {
                        g {
                            id: control_id("corner", &shown_number.to_string()),
                            class: "style-corner",
                            tabindex: "0",
                            role: "button",
                            "aria-label": "{label}",
                            "aria-pressed": if is_off { "false" } else { "true" },
                            onkeydown: move |e: KeyboardEvent| {
                                if matches!(e.key(), Key::Enter) || e.key() == Key::Character(" ".to_string()) {
                                    e.prevent_default();
                                    e.stop_propagation();
                                    turn_corner(settings, kind, number, count);
                                }
                            },
                            rect {
                                class: "{pill}",
                                x: "{corner[0] - PILL_W / 2.0:.4}",
                                y: "{corner[1] - PILL_H / 2.0:.4}",
                                width: "{PILL_W}",
                                height: "{PILL_H}",
                                rx: "{PILL_H / 2.0}",
                            }
                            if !is_off && !is_top {
                                rect {
                                    class: "style-pill-tint",
                                    x: "{corner[0] - PILL_W / 2.0:.4}",
                                    y: "{corner[1] - PILL_H / 2.0:.4}",
                                    width: "{PILL_W}",
                                    height: "{PILL_H}",
                                    rx: "{PILL_H / 2.0}",
                                    "fill-opacity": "{tint(shown[number], most):.3}",
                                }
                            }
                            text {
                                class: match (is_off, is_top) {
                                    (true, _) => "style-pct off",
                                    (false, true) => "style-pct top",
                                    (false, false) => "style-pct",
                                },
                                x: "{corner[0]:.4}",
                                y: "{corner[1]:.4}",
                                if is_off {
                                    "off"
                                } else {
                                    "{pct}%"
                                }
                            }
                            text {
                                class: match (is_off, is_top) {
                                    (true, _) => "style-lead off",
                                    (false, true) => "style-lead top",
                                    (false, false) => "style-lead",
                                },
                                x: "{label_x:.4}",
                                y: "{corner[1]:.4}",
                                "text-anchor": if outward < 0.0 { "end" } else { "start" },
                                "{lead}"
                            }
                        }
                    }
                }
            }
            if let Some(point) = ghost {
                circle {
                    class: "style-knob ghost",
                    cx: "{point[0]}",
                    cy: "{point[1]}",
                    r: "0.09",
                }
            }
            circle {
                class: "style-knob",
                cx: "{at[0]}",
                cy: "{at[1]}",
                r: "0.09",
            }
        }
    }
}

/// How many of the commonest replies the card writes out in full.
const HEARD_MOST: usize = 3;

/// How a contest's callers reply, POTA's hunters among them, folded away
/// under a line that says how it is set, as the Band card folds its
/// receiver. Open, the shape with a reply at each corner and the knob that
/// sets how often each comes, a line saying what a click would do, and the
/// replies you will hear most, whole, beside how many in a hundred send
/// each. While a mouse is over the shape, they are the ones of the mix it
/// would set, marked as not set yet. In POTA, how many hunters are DX, on a
/// slider. What it does is in its ⓘ.
#[component]
fn ReplyStyles(settings: Signal<TrainingSettings>, styles: StylesCard) -> Element {
    let mut open = use_signal(|| false);
    let mut help = use_signal(|| false);
    let hover = use_signal(|| None::<Pointing>);
    let reading = styles.reading();
    let who = styles.who();
    let title = if styles.dx_percent.is_some() {
        "Hunter styles"
    } else {
        "Caller styles"
    };
    let count = styles.replies.len();
    // The mix the mouse would set, while it is over the shape somewhere
    // else than the knob and its corners.
    let preview = match hover() {
        Some(Pointing::At(point)) if point != styles.at => Some(point),
        _ => None,
    };
    let rows = match preview {
        Some(point) => styles.at_point(point),
        None => styles.replies.clone(),
    };
    let mut most: Vec<(String, u32)> = rows.into_iter().filter(|(_, pct)| *pct > 0).collect();
    most.sort_by_key(|(_, pct)| std::cmp::Reverse(*pct));
    most.truncate(HEARD_MOST);
    let whole = |corner: usize| {
        styles
            .replies
            .get(corner)
            .map_or_else(String::new, |(reply, _)| reply.clone())
    };
    let line = match hover() {
        Some(Pointing::Corner(corner)) if styles.off.contains(&corner) => {
            format!("Click to turn back on: {}", whole(corner))
        }
        Some(Pointing::Corner(_)) if styles.off.len() + 1 >= count => {
            "This is the last reply on".to_string()
        }
        Some(Pointing::Corner(corner)) => format!("Click to turn off: {}", whole(corner)),
        _ if preview.is_some() => "Click to set this mix".to_string(),
        _ => "Drag the knob toward the replies you want more of; tap a corner to turn it off"
            .to_string(),
    };
    rsx! {
        div { class: "card-title-row",
            button {
                id: control_id(DISCLOSURE, who),
                class: if open() { "link-toggle open" } else { "link-toggle" },
                r#type: "button",
                aria_expanded: if open() { "true" } else { "false" },
                aria_controls: "reply-styles",
                onclick: move |_| open.toggle(),
                "{title}: {reading}"
                Icon { name: IconName::Chevron }
            }
            button {
                id: control_id("help", who),
                class: if help() { "icon-btn help-btn on" } else { "icon-btn help-btn" },
                r#type: "button",
                title: if help() { "Hide the help" } else { "What does this mean?" },
                aria_label: if help() { format!("Hide the help on {who}") } else { format!("Help on {who}") },
                aria_expanded: help(),
                onclick: move |_| help.toggle(),
                Icon { name: if help() { IconName::X } else { IconName::Info } }
            }
        }
        if help() {
            div { class: "help-note", role: "note", {help::caller_styles(styles.dx_percent.is_some())} }
        }
        if open() {
            div { id: "reply-styles", class: "stack-sm",
                StyleShape { settings, styles: styles.clone(), hover }
                p { class: "muted hint style-preview-line", "{line}" }
                div { class: "eyebrow", "You'll hear most" }
                ol {
                    class: if preview.is_some() { "qso style-replies preview" } else { "qso style-replies" },
                    aria_label: "The replies you'll hear most, and how often",
                    for (reply , pct) in most {
                        li { class: "qso-line they",
                            span { class: "qso-who tiny", "{pct}%" }
                            span { class: "qso-sends",
                                span { class: "call", "{reply}" }
                            }
                        }
                    }
                }
                if let Some(dx) = styles.dx_percent {
                    SliderField {
                        label: "DX hunters",
                        long_label: "How many hunters are DX",
                        value_label: format!("{dx}%"),
                        value: f64::from(dx),
                        min: 0.0,
                        max: 100.0,
                        step: 5.0,
                        disabled: false,
                        onchange: move |percent: f64| {
                            // Within the slider's ends, so the cast holds.
                            settings.write().contest.hunter_dx_percent = percent.round().clamp(0.0, 100.0)
                                as u32;
                        },
                    }
                    p { class: "muted hint style-dx-note",
                        "Hunters outside the US and Canada send their report alone: log them without a state."
                    }
                }
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
            ReplyStyles { settings, styles }
        }
        OddsFold {
            settings,
            title: "Caller odds",
            odds: contest.odds,
            groups: contest.odds_groups.clone(),
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
    use super::{
        LABEL_LETTER, PILL_W, SHAPE_PX, SHAPE_TOP, ShapeView, StylesCard, corner_under, in_percent,
        shade, tint,
    };
    use cw_core::{ContestKind, STYLE_EVEN, shape_corners};

    /// Where POTA's knob stands until moved.
    fn pota_on_air() -> [f64; 2] {
        ContestKind::Pota.rules().styles_on_air()
    }

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
            StylesCard::of(styles, ContestKind::Pota, at, &[], Some(25))
        };
        let one = card(shape_corners(8)[0]);
        assert_eq!(one.reading(), "One reply");
        assert_eq!(one.replies_heard(), 1);
        assert_eq!(
            one.replies[0],
            ("BK SA0KAM TU UR 579 579 OR OR BK".to_string(), 100)
        );
        let on_air = card(pota_on_air());
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
            .caller_styles(cw_core::Side::Dx, "SA0KAM", pota_on_air(), &off)
            .expect("POTA's hunters vary");
        let card = StylesCard::of(styles, ContestKind::Pota, pota_on_air(), &off, Some(25));
        assert_eq!(card.reading(), "6 replies, 2 off");
        assert_eq!((card.replies[1].1, card.replies[2].1), (0, 0));
        let even = card.at_point(STYLE_EVEN);
        assert_eq!((even[1].1, even[2].1), (0, 0));
        assert!(
            even.iter()
                .all(|(_, pct)| *pct == 0 || (16..=17).contains(pct))
        );
    }

    /// The face is shaded toward a reply's corner as strongly as its share,
    /// fading toward the middle, and a pill's wash follows its share; a
    /// reply no hunter sends has no shade.
    #[test]
    fn the_shade_follows_its_share() {
        let (corner, middle) = shade(0.5, 0.5).expect("a shade");
        assert!((corner - 0.7).abs() < 1e-9 && (middle - 0.105).abs() < 1e-9);
        let (half, _) = shade(0.25, 0.5).expect("a shade");
        assert!((half - 0.39).abs() < 1e-9);
        assert_eq!(shade(0.0, 0.5), None);
        assert_eq!(shade(0.2, 0.0), None);
        assert!(tint(0.5, 0.5) > tint(0.1, 0.5));
        assert_eq!(tint(0.1, 0.0), 0.0);
    }

    fn pota_card(my_call: &str) -> StylesCard {
        let styles = cw_core::ContestKind::Pota
            .rules()
            .caller_styles(cw_core::Side::Dx, my_call, pota_on_air(), &[])
            .expect("POTA's hunters vary");
        StylesCard::of(styles, ContestKind::Pota, pota_on_air(), &[], Some(25))
    }

    /// A corner's pill, or the words beside it, is where a click turns its
    /// reply: on it, or near enough; not the middle, nor the face between
    /// two corners, nor past the words.
    #[test]
    fn a_corner_is_under_the_pointer_on_its_pill_or_words() {
        let card = pota_card("SA0KAM");
        let corners = shape_corners(8);
        let under = |at: [f64; 2]| corner_under(&card.leads, at);
        for (number, corner) in corners.iter().enumerate() {
            assert_eq!(under(*corner), Some(number), "{number}");
        }
        // Its words go outward: right of a right corner, left of a left one.
        let [x, y] = corners[0];
        assert_eq!(under([x + PILL_W / 2.0 + 0.3, y]), Some(0));
        assert_eq!(under([x + PILL_W / 2.0 + 0.95, y]), None, "past the words");
        let [x, y] = corners[7];
        assert_eq!(under([x - PILL_W / 2.0 - 0.3, y]), Some(7));
        assert_eq!(under([x + 0.3, y + 0.2]), None, "into the face");
        assert_eq!(under([0.0, 0.0]), None);
        assert_eq!(under([0.0, -0.6]), None);
    }

    /// The drawing is as wide as the words beside its corners reach, so a
    /// long call widens it; the pointer's place on it is the shape's point
    /// under it, brought inside the shape.
    #[test]
    fn the_pointer_places_the_knob_under_it() {
        let card = pota_card("SA0KAM");
        let view = ShapeView::of(&card.leads, &card.exchanges);
        let longer = pota_card("VE3/SA0KAM/P");
        let wider = ShapeView::of(&longer.leads, &longer.exchanges);
        assert!(wider.half > view.half + 0.3, "{view:?} {wider:?}");
        // Every pill's words fit inside it.
        let corners = shape_corners(8);
        for (corner, lead) in corners.iter().zip(&card.leads) {
            let reach = corner[0].abs() + PILL_W / 2.0 + lead.len() as f64 * LABEL_LETTER;
            assert!(reach < view.half, "{lead}");
        }
        assert!((view.height_px() / SHAPE_PX - view.height() / view.width()).abs() < 1e-12);
        // The drawing's middle across, at the shape's height of nought.
        let down = -SHAPE_TOP / view.width() * SHAPE_PX;
        assert_eq!(view.point(8, SHAPE_PX / 2.0, down), [0.0, 0.0]);
        assert_eq!(view.unit(0.0, 0.0), [-view.half, SHAPE_TOP]);
        // Far right, to the shape's right edge; far up, its top edge.
        let right = view.point(8, SHAPE_PX, down);
        assert_eq!(right, [0.92, 0.0], "an edge stands there");
        let top = view.point(8, SHAPE_PX / 2.0, 0.0);
        assert_eq!(top, [0.0, -0.92]);
        // Near a corner, or past it, onto it exactly: that reply alone.
        let corner = shape_corners(8)[1];
        let (x, y) = view.px([corner[0] - 0.03, corner[1] + 0.02]);
        assert_eq!(view.point(8, x, y), corner);
        let (x, y) = view.px([corner[0] * 1.3, corner[1] * 1.3]);
        assert_eq!(view.point(8, x, y), corner);
        let (x, y) = view.px([0.1, 0.2]);
        assert_eq!(view.point(8, x, y), [0.1, 0.2]);
    }

    /// A contest's callers have a shape of their own, as many corners as
    /// replies: WPX's six, each row across starting alike, the reply that
    /// starts with its exchange beside a dash; its heads say how the
    /// exchange goes on each side.
    #[test]
    fn a_contest_has_a_shape_as_many_cornered_as_its_replies() {
        let rules = ContestKind::Wpx.rules();
        let styles = rules
            .caller_styles(cw_core::Side::Dx, "DL1ABC", rules.styles_on_air(), &[])
            .expect("WPX's callers vary");
        let card = StylesCard::of(styles, ContestKind::Wpx, rules.styles_on_air(), &[], None);
        assert_eq!(card.replies.len(), 6);
        assert_eq!(card.reading(), "6 replies, as on the air");
        assert_eq!(card.who(), "caller styles");
        assert_eq!(card.heads, ["Once", "Twice"]);
        assert_eq!(card.replies[0], ("TU 5NN 214".to_string(), 55));
        assert_eq!(card.replies[1].0, "5NN 214");
        let labels: Vec<&str> = card
            .leads
            .iter()
            .map(|lead| super::lead_label(lead))
            .collect();
        assert_eq!(labels, ["TU", "—", "R", "R", "—", "TU"]);
        // The words beside each corner fit in the drawing.
        let view = ShapeView::of(&card.leads, &card.exchanges);
        for (corner, label) in shape_corners(6).iter().zip(&labels) {
            let reach =
                corner[0].abs() + PILL_W / 2.0 + label.chars().count() as f64 * LABEL_LETTER;
            assert!(reach < view.half, "{label}");
        }
    }
}
