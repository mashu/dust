//! The guided setup: a first session, then the settings that matter most when
//! starting out, one step at a time, with the control to press pointed at on
//! the page.
//!
//! It starts where a learner does, on the practice screen: start a session,
//! copy one group, discard the session, and go to Settings. Those steps move
//! on by themselves once they are done, as doing them is what takes you to
//! the screen the next one is on. On Settings every step is a few moves —
//! Random, Min 15, Max 25 — and each move knows whether the settings show it
//! made already. So the guide follows whatever is done with the real
//! controls: it ticks a move off however it was made, and points at the next.
//!
//! Show me makes the moves still to make, one at a time and pointing at each
//! first, through the same calls the controls make: it presses Start
//! training, types the group it heard into the box the way a keyed letter
//! goes in, discards the session, and sets each setting as its control does.
//!
//! It points by the ids the controls carry already ([`control_id`]), in a
//! style rule of its own, and needs nothing from the screens.

use cw_core::{
    CharSetMode, GroupSession, PresetId, RangeSetting, SequenceChoice, SessionPhase,
    TrainingSettings,
};
use dioxus::core::Task;
use dioxus::prelude::*;

use crate::session_runtime::{abort_run, start_session};
use crate::state::{AppCtx, Screen, Tour};
use crate::time::{POLL_MS, sleep_ms};
use crate::ui::paddle::PaddleKeys;
use crate::ui::widgets::{Icon, IconName, control_id, html_bool, pretty_number};

/// How long Show me points at a control before it makes the move there.
const POINT_MS: u32 = 700;
/// How long a move it made stays in sight before it points at the next.
const SHOWN_MS: u32 = 500;
/// How long Show me spends on each letter it types.
const LETTER_MS: u32 = 250;

/// A min–max setting, by the label the settings screen shows it under, which
/// is what the ids of its controls are made from.
#[derive(Clone, Copy, Debug)]
struct Range {
    which: RangeSetting,
    label: &'static str,
    unit: &'static str,
}

const CHARACTER_SPEED: Range = Range {
    which: RangeSetting::CharWpm,
    label: "Character speed",
    unit: "WPM",
};
const GROUP_SIZE: Range = Range {
    which: RangeSetting::GroupSize,
    label: "Group size",
    unit: "chars",
};
const SIDE_TONE: Range = Range {
    which: RangeSetting::SideTone,
    label: "Side tone",
    unit: "Hz",
};

/// The auto-level controls, by the labels they are shown under.
const AUTO_LEVEL_SWITCH: &str = "Adjust the level for me";
const LEVEL_UP_FIELD: &str = "Sessions above to level up";
const LEVEL_DOWN_FIELD: &str = "Sessions below to level down";

/// What a step goes by: the settings, the screen, what the learner has done
/// since the guide opened, and the group a running session is on.
struct Seen<'a> {
    settings: &'a TrainingSettings,
    screen: Screen,
    tour: Tour,
    group: Option<usize>,
}

/// One thing to do, on one control.
#[derive(Clone, Copy, Debug)]
enum Move {
    /// Start a session.
    Start,
    /// Copy a group into its box.
    Answer,
    /// Leave the session without keeping it.
    Discard,
    /// Go to the settings screen.
    OpenSettings,
    /// Open a range up, so every group draws its own value from it.
    Random(Range),
    /// Where an open range starts.
    Min(Range, f64),
    /// Where it ends.
    Max(Range, f64),
    /// One of these unlock orders. Show me takes the first.
    Order(&'static [PresetId]),
    /// Let the trainer move the level.
    AutoLevel,
    /// How many good sessions move the level up.
    UpAfter(u32),
    /// How many poor ones move it back down.
    DownAfter(u32),
}

/// Whether the character set is learnt in an unlock order. Digits,
/// callsigns and words have none, and the settings screen offers none for
/// them.
fn has_unlock_order(settings: &TrainingSettings) -> bool {
    matches!(
        settings.curriculum.char_set_mode,
        CharSetMode::Koch | CharSetMode::Mixed
    )
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Whether any group of `session` has been answered: confirmed with
/// something typed, not let go by.
fn any_answered(session: &GroupSession) -> bool {
    (0..session.group_count())
        .filter_map(|index| session.group(index))
        .any(|group| group.confirmed() && !group.input().trim().is_empty())
}

impl Move {
    /// What it reads as on the guide.
    fn label(self) -> String {
        match self {
            Self::Start => "Start training".to_string(),
            Self::Answer => "Type the group".to_string(),
            Self::Discard => "Discard".to_string(),
            Self::OpenSettings => "Settings".to_string(),
            Self::Random(_) => "Random".to_string(),
            Self::Min(range, value) => format!("Min {} {}", pretty_number(value), range.unit),
            Self::Max(range, value) => format!("Max {} {}", pretty_number(value), range.unit),
            Self::Order(presets) => presets
                .iter()
                .map(|id| id.preset().name)
                .collect::<Vec<_>>()
                .join(" or "),
            Self::AutoLevel => "Auto level on".to_string(),
            Self::UpAfter(sessions) => format!("Up after {sessions}"),
            Self::DownAfter(sessions) => format!("Down after {sessions}"),
        }
    }

    /// Whether it is made.
    fn done(self, seen: &Seen) -> bool {
        let settings = seen.settings;
        let auto = &settings.auto_level;
        match self {
            Self::Start => seen.tour.started,
            Self::Answer => seen.tour.answered,
            Self::Discard => seen.tour.left,
            Self::OpenSettings => seen.screen == Screen::Settings,
            Self::Random(range) => !settings.range(range.which).linked,
            Self::Min(range, value) => {
                let values = settings.range(range.which);
                !values.linked && same(values.min, value)
            }
            Self::Max(range, value) => {
                let values = settings.range(range.which);
                !values.linked && same(values.max, value)
            }
            Self::Order(presets) => {
                has_unlock_order(settings)
                    && presets
                        .iter()
                        .any(|id| settings.sequence_choice() == SequenceChoice::Preset(*id))
            }
            Self::AutoLevel => auto.auto_adjust_level,
            Self::UpAfter(sessions) => auto.auto_adjust_above_threshold_count == sessions,
            Self::DownAfter(sessions) => auto.auto_adjust_below_threshold_count == sessions,
        }
    }

    /// Make it, the way its control does.
    async fn make(self, ctx: AppCtx, keys: PaddleKeys) {
        let mut settings = ctx.settings;
        match self {
            Self::Start | Self::Discard | Self::OpenSettings => self.take_there(ctx),
            Self::Answer => copy_group(ctx, keys).await,
            Self::Random(range) => settings.write().set_range_linked(range.which, false),
            Self::Min(range, value) => settings.write().set_range_min(range.which, value),
            Self::Max(range, value) => settings.write().set_range_max(range.which, value),
            Self::Order(presets) => {
                let mut settings = settings.write();
                if !has_unlock_order(&settings) {
                    settings.set_char_set_mode(CharSetMode::Koch);
                }
                if let Some(id) = presets.first() {
                    settings.choose_sequence(SequenceChoice::Preset(*id));
                }
            }
            Self::AutoLevel => settings.write().auto_level.auto_adjust_level = true,
            Self::UpAfter(sessions) => {
                settings
                    .write()
                    .auto_level
                    .auto_adjust_above_threshold_count = sessions;
            }
            Self::DownAfter(sessions) => {
                settings
                    .write()
                    .auto_level
                    .auto_adjust_below_threshold_count = sessions;
            }
        }
    }

    /// Make a move that takes the learner somewhere, at once: what Next does
    /// for a step left undone, so the step after it is on the screen it
    /// expects. A group copied would have to wait for its send, so Next
    /// leaves that one to the learner, and nothing else needs doing.
    fn take_there(self, ctx: AppCtx) {
        match self {
            Self::Start => start_session(ctx),
            Self::Discard => abort_run(ctx),
            Self::OpenSettings => ctx.go_to(Screen::Settings),
            _ => {}
        }
    }

    /// The ids of the controls it is made on, as their screen draws them now.
    fn targets(self, seen: &Seen) -> Vec<String> {
        let settings = seen.settings;
        match self {
            Self::Start => vec![control_id("btn", "start training")],
            // The box of the group being sent, which opens once it is.
            Self::Answer => seen
                .group
                .map(|index| format!("group-input-{index}"))
                .into_iter()
                .collect(),
            Self::Discard => vec![control_id("btn", "discard")],
            Self::OpenSettings => vec![control_id("nav", "settings")],
            Self::Random(range) => vec![control_id("link", range.label)],
            Self::Min(range, _) => vec![control_id("from", range.label)],
            Self::Max(range, _) => vec![control_id("to", range.label)],
            Self::Order(presets) if has_unlock_order(settings) => presets
                .iter()
                .map(|id| control_id("pill", id.preset().name))
                .collect(),
            // The orders are offered once there are letters to learn: Koch
            // first.
            Self::Order(_) => vec![control_id("seg", "Koch")],
            Self::AutoLevel => vec![control_id("switch", AUTO_LEVEL_SWITCH)],
            Self::UpAfter(_) => vec![control_id("number", LEVEL_UP_FIELD)],
            Self::DownAfter(_) => vec![control_id("number", LEVEL_DOWN_FIELD)],
        }
    }
}

/// Type the group being answered into its box, a letter at a time, once the
/// box opens: the session sends the group first.
async fn copy_group(ctx: AppCtx, keys: PaddleKeys) {
    let sent = loop {
        let open = ctx
            .runtime
            .peek()
            .as_ref()
            .map(|session| match session.phase() {
                SessionPhase::AwaitingAnswer { index } => {
                    session.group(index).map(|group| group.sent().to_string())
                }
                _ => None,
            });
        match open {
            // There is no session to answer in.
            None => return,
            Some(Some(sent)) => break sent,
            Some(None) => sleep_ms(POLL_MS).await,
        }
    };
    // A moment to see the box open before it fills.
    sleep_ms(POINT_MS).await;
    for letter in sent.chars() {
        // A box closed meanwhile takes no more.
        if !keys.type_letter(letter) {
            return;
        }
        sleep_ms(LETTER_MS).await;
    }
}

/// Whether `step_move` is made, as the app stands now.
fn made_now(ctx: AppCtx, step_move: Move) -> bool {
    let settings = ctx.settings.peek();
    let seen = Seen {
        settings: &settings,
        screen: *ctx.screen.peek(),
        tour: ctx.tour.peek().unwrap_or_default(),
        group: ctx.runtime.peek().as_ref().map(GroupSession::current_group),
    };
    step_move.done(&seen)
}

/// One stop on the guide.
struct Step {
    title: &'static str,
    text: &'static str,
    moves: &'static [Move],
    /// The screen its controls are on, where it has one of its own.
    screen: Option<Screen>,
    /// It moves on by itself once its moves are made, as making them takes
    /// the learner to where the next step is.
    moves_on: bool,
}

/// Where the guide's settings steps are.
const ON_SETTINGS: Option<Screen> = Some(Screen::Settings);

const STEPS: &[Step] = &[
    Step {
        title: "Start a session",
        text: "Press Start training. The trainer sends a group of characters, and you copy it by ear.",
        moves: &[Move::Start],
        screen: Some(Screen::Home),
        moves_on: true,
    },
    Step {
        title: "Copy a group",
        text: "Listen to the group. When it finishes the box opens: type what you heard, on the keypad or a keyboard. Take your time — it waits for you.",
        moves: &[Move::Answer],
        screen: Some(Screen::Training),
        moves_on: true,
    },
    Step {
        title: "Discard the session",
        text: "That is the whole drill: hear a group, type it. Press Discard to leave without keeping this session.",
        moves: &[Move::Discard],
        screen: Some(Screen::Training),
        moves_on: true,
    },
    Step {
        title: "Open Settings",
        text: "Now set the trainer up for you. Settings is at the foot of the screen.",
        moves: &[Move::OpenSettings],
        screen: None,
        moves_on: true,
    },
    Step {
        title: "Unlock order",
        text: "The order new characters arrive in, one per level. MorseMania is the classic Koch order, from K M R S U; CW Academy starts with the commonest letters, E T A O N.",
        moves: &[Move::Order(&[PresetId::MorseMania, PresetId::CwAcademy])],
        screen: ON_SETTINGS,
        moves_on: false,
    },
    Step {
        title: "Character speed",
        text: "Press Random, then set where the range starts and ends. Every group is sent at a speed of its own from inside it.",
        moves: &[
            Move::Random(CHARACTER_SPEED),
            Move::Min(CHARACTER_SPEED, 15.0),
            Move::Max(CHARACTER_SPEED, 25.0),
        ],
        screen: ON_SETTINGS,
        moves_on: false,
    },
    Step {
        title: "Group size",
        text: "Random again, for groups of 2 to 3 characters: short enough to copy whole while the letters are new.",
        moves: &[
            Move::Random(GROUP_SIZE),
            Move::Min(GROUP_SIZE, 2.0),
            Move::Max(GROUP_SIZE, 3.0),
        ],
        screen: ON_SETTINGS,
        moves_on: false,
    },
    Step {
        title: "Side tone",
        text: "A range gives every group a pitch of its own, the way each station on the band sounds different. 500–700 Hz is a start; set what suits your ear.",
        moves: &[
            Move::Random(SIDE_TONE),
            Move::Min(SIDE_TONE, 500.0),
            Move::Max(SIDE_TONE, 700.0),
        ],
        screen: ON_SETTINGS,
        moves_on: false,
    },
    Step {
        title: "Auto level",
        text: "Let the trainer move the level: three good sessions unlock the next character, and one poor session takes it back. A session is good at or above the accuracy threshold.",
        moves: &[Move::AutoLevel, Move::UpAfter(3), Move::DownAfter(1)],
        screen: ON_SETTINGS,
        moves_on: false,
    },
    Step {
        title: "All set",
        text: "Every change is saved as you make it, and the reset button on each card puts that card back as it shipped. Start a session from Practice.",
        moves: &[],
        screen: None,
        moves_on: false,
    },
];

/// Whether every move of step `at` is made.
fn step_done(at: usize, seen: &Seen) -> bool {
    STEPS
        .get(at)
        .is_some_and(|step| step.moves.iter().all(|step_move| step_move.done(seen)))
}

/// The move the guide points at: the one Show me is making, or else the
/// first still to make.
fn pointed(step: &Step, seen: &Seen, showing: Option<usize>) -> Option<usize> {
    showing.or_else(|| {
        step.moves
            .iter()
            .position(|step_move| !step_move.done(seen))
    })
}

/// The nav button that goes to `screen`, where there is one.
fn way_to(screen: Screen) -> Option<String> {
    match screen {
        Screen::Home => Some(control_id("nav", "practice")),
        Screen::Settings => Some(control_id("nav", "settings")),
        Screen::Stats => Some(control_id("nav", "stats")),
        Screen::Training | Screen::Results | Screen::Listen => None,
    }
}

/// The controls to point at for move `index` of `step`: its own, on the
/// screen they are on, and from anywhere else the way to that screen.
fn aim(step: &Step, seen: &Seen, index: usize) -> Vec<String> {
    match step.screen {
        Some(screen) if screen != seen.screen => way_to(screen).into_iter().collect(),
        _ => step
            .moves
            .get(index)
            .map(|step_move| step_move.targets(seen))
            .unwrap_or_default(),
    }
}

/// The move Show me is making on step `at`, if it is making one there.
fn showing_on(showing: Option<(usize, usize)>, at: usize) -> Option<usize> {
    showing.filter(|(on, _)| *on == at).map(|(_, index)| index)
}

/// The rule that points at the controls `ids` name, those of them the page
/// draws. Its pulse is the stylesheet's.
fn pointer_css(ids: &[String]) -> String {
    if ids.is_empty() {
        return String::new();
    }
    let selectors: Vec<String> = ids
        .iter()
        .map(|id| {
            // A switch's box lies see-through over its track: the knob is
            // what shows.
            if id.starts_with("switch-") {
                format!("#{id} + i")
            } else {
                format!("#{id}")
            }
        })
        .collect();
    format!(
        "{} {{ outline: 2px solid var(--copper); outline-offset: 2px; animation: tour-ring 1.4s ease-in-out infinite; animation-timing-function: steps(6, jump-none); }}",
        selectors.join(", ")
    )
}

/// The script that brings the first control of `ids` the page has into
/// sight, clear of the guide's dock, and leaves it where it is if it is in
/// sight already: a move made by hand points at the control next to it, and
/// the page should not jump for that.
fn scroll_script(ids: &[String]) -> String {
    let ids: Vec<String> = ids.iter().map(|id| format!("{id:?}")).collect();
    format!(
        r#"const target = [{}].map((id) => document.getElementById(id)).find((el) => el);
if (!target) {{
    return;
}}
// The dock covers the foot of the window, or its head during a session:
// what is under it is out of sight.
const dock = document.getElementById("tour-dock");
const cover = dock ? dock.getBoundingClientRect() : null;
const box = target.getBoundingClientRect();
const inWindow = box.top >= 0 && box.bottom <= window.innerHeight;
const underDock = cover !== null && box.bottom > cover.top && box.top < cover.bottom;
if (inWindow && !underDock) {{
    return;
}}
target.scrollIntoView({{ block: "center", inline: "nearest", behavior: "smooth" }});"#,
        ids.join(", ")
    )
}

fn bring_into_sight(ids: &[String]) {
    // Tests have no page to run it on.
    #[cfg(not(test))]
    {
        let script = scroll_script(ids);
        spawn(async move {
            let _ = document::eval(&script).await;
        });
    }
    #[cfg(test)]
    let _ = ids;
}

/// Open the guide at its start, on the practice screen where a first session
/// starts, and remember that it has been opened: it opens by itself only on
/// a first launch.
fn open(ctx: AppCtx) {
    if ctx.session_running() {
        return;
    }
    if *ctx.screen.peek() != Screen::Home {
        ctx.go_to(Screen::Home);
    }
    ctx.app().store.mark_tutorial_done();
    let mut tour = ctx.tour;
    tour.set(Some(Tour::default()));
}

/// Open the guide, or close it if it is open.
fn toggle(ctx: AppCtx) {
    let mut tour = ctx.tour;
    if tour.peek().is_some() {
        tour.set(None);
    } else {
        open(ctx);
    }
}

/// Whether this launch is the app's first: the guide has never been opened,
/// and nothing has been practised. An install from before the guide, with a
/// history, is not new to the app, and is not shown it unasked.
fn first_launch(ctx: AppCtx) -> bool {
    !ctx.app().store.tutorial_done() && ctx.sessions.peek().is_empty()
}

/// Move on from step `at`. A step that takes the learner somewhere, left
/// undone, is done on the way: Next out of a session discards it, rather
/// than leaving it running under a step that points at Settings.
fn next(ctx: AppCtx, at: usize) {
    if let Some(step) = STEPS.get(at).filter(|step| step.moves_on) {
        for step_move in step.moves.iter().copied() {
            if !made_now(ctx, step_move) {
                step_move.take_there(ctx);
            }
        }
    }
    go_to_step(ctx.tour, at + 1);
}

/// Go to step `at`.
fn go_to_step(mut tour: Signal<Option<Tour>>, at: usize) {
    if let Some(now) = tour.write().as_mut() {
        now.step = at;
    }
}

/// The way in: a small button in the header.
#[component]
pub fn TourButton() -> Element {
    let ctx = use_context::<AppCtx>();
    // The guide's steps point at a session's group boxes, which a contest
    // run has none of: in Contest it waits for steps of its own (phase 5).
    let contest =
        use_memo(move || ctx.settings.read().curriculum.char_set_mode == CharSetMode::Contest);
    let open = ctx.tour.read().is_some();
    if contest() && !open {
        return rsx! {};
    }
    rsx! {
        button {
            id: control_id("btn", "guided setup"),
            class: if open { "icon-btn on" } else { "icon-btn" },
            r#type: "button",
            title: "Guided setup",
            aria_label: "Guided setup",
            aria_pressed: open,
            onclick: move |_| toggle(ctx),
            Icon { name: IconName::Help }
        }
    }
}

/// The guide itself, docked above the nav, or at the head of the screen
/// during a session, and the rule that points at the control it means.
#[component]
pub fn TourDock() -> Element {
    let ctx = use_context::<AppCtx>();
    let keys = use_context::<PaddleKeys>();
    let AppCtx {
        mut tour,
        settings,
        screen,
        runtime,
        ..
    } = ctx;
    // The move Show me is making, and the step it is making it on.
    let mut showing = use_signal(|| None::<(usize, usize)>);
    let mut player = use_signal(|| None::<Task>);
    let at = use_memo(move || tour().map(|now| now.step));
    let group = use_memo(move || runtime.read().as_ref().map(GroupSession::current_group));
    let answered = use_memo(move || runtime.read().as_ref().is_some_and(any_answered));

    // A first launch opens the guide by itself. After that it waits for the
    // button in the header.
    use_effect(move || {
        if first_launch(ctx) {
            open(ctx);
        }
    });

    // Show me stops wherever the guide goes: another step, or away.
    use_effect(move || {
        let _ = at();
        if let Some(task) = player.write().take() {
            task.cancel();
        }
        showing.set(None);
    });

    // Follow what the learner does: a session started, a group answered in
    // it, the session left again. A step that moves on by itself does so
    // when it is done there, and not when it is come back to done already.
    let mut last = use_hook(|| CopyValue::new(None::<(usize, bool)>));
    use_effect(move || {
        let (screen, answered, _) = (screen(), answered(), at());
        let Some(before) = *tour.peek() else {
            last.set(None);
            return;
        };
        let mut now = before;
        now.started |= screen == Screen::Training;
        now.answered |= now.started && answered;
        now.left |= now.started && screen != Screen::Training;
        let settings = settings.peek();
        let mut seen = Seen {
            settings: &settings,
            screen,
            tour: now,
            group: *group.peek(),
        };
        let done = step_done(now.step, &seen);
        let moves_on = STEPS.get(now.step).is_some_and(|step| step.moves_on);
        if done && moves_on && *last.peek() == Some((now.step, false)) {
            now.step += 1;
            seen.tour = now;
        }
        last.set(Some((now.step, step_done(now.step, &seen))));
        if now != before {
            tour.set(Some(now));
        }
    });

    // What to bring into sight: the controls pointed at, or once every move
    // of the step is made, those of its first.
    let sight = use_memo(move || {
        let Some(now) = tour() else {
            return Vec::new();
        };
        let Some(step) = STEPS.get(now.step) else {
            return Vec::new();
        };
        let settings = settings.read();
        let seen = Seen {
            settings: &settings,
            screen: screen(),
            tour: now,
            group: group(),
        };
        let shown = showing_on(showing(), now.step);
        let index = pointed(step, &seen, shown).unwrap_or_default();
        aim(step, &seen, index)
    });
    use_effect(move || {
        let ids = sight();
        if !ids.is_empty() {
            bring_into_sight(&ids);
        }
    });

    let Some(now) = tour() else {
        return rsx! {};
    };
    let at = now.step;
    let Some(step) = STEPS.get(at) else {
        return rsx! {};
    };
    let screen_now = screen();
    let shown = showing_on(showing(), at);
    let (chips, pointer) = {
        let settings = settings.read();
        let seen = Seen {
            settings: &settings,
            screen: screen_now,
            tour: now,
            group: group(),
        };
        let pointing = pointed(step, &seen, shown);
        let chips: Vec<(String, &'static str)> = step
            .moves
            .iter()
            .enumerate()
            .map(|(index, step_move)| {
                let class = if step_move.done(&seen) {
                    "tour-move done"
                } else if pointing == Some(index) {
                    "tour-move next"
                } else {
                    "tour-move"
                };
                (step_move.label(), class)
            })
            .collect();
        let pointer = pointing
            .map(|index| pointer_css(&aim(step, &seen, index)))
            .unwrap_or_default();
        (chips, pointer)
    };
    let all_made = chips.iter().all(|(_, class)| *class == "tour-move done");
    let last_step = at + 1 == STEPS.len();
    let total = STEPS.len();
    // A session has the foot of the screen: its answers, and the way out.
    let dock_class = if screen_now == Screen::Training {
        "tour-dock top"
    } else {
        "tour-dock"
    };

    let show_me = move |_| {
        let task = spawn(async move {
            let moves = STEPS.get(at).map_or(&[][..], |step| step.moves);
            for (index, step_move) in moves.iter().copied().enumerate() {
                if made_now(ctx, step_move) {
                    continue;
                }
                showing.set(Some((at, index)));
                sleep_ms(POINT_MS).await;
                // The guide went elsewhere while it pointed.
                if (*tour.peek()).map(|now| now.step) != Some(at) {
                    return;
                }
                step_move.make(ctx, keys).await;
                sleep_ms(SHOWN_MS).await;
            }
            showing.set(None);
        });
        player.set(Some(task));
    };

    rsx! {
        div { class: dock_class,
            section { id: "tour-dock", class: "tour", aria_label: "Guided setup",
                div { class: "tour-head",
                    span { class: "eyebrow", "Guided setup · {at + 1} of {total}" }
                    button {
                        id: control_id("tour", "close"),
                        class: "icon-btn",
                        r#type: "button",
                        title: "Close the guided setup",
                        aria_label: "Close the guided setup",
                        onclick: move |_| tour.set(None),
                        Icon { name: IconName::X }
                    }
                }
                div { aria_live: "polite",
                    h3 { class: "tour-title", "{step.title}" }
                    p { class: "tour-text", "{step.text}" }
                }
                if !chips.is_empty() {
                    div { class: "tour-moves",
                        for (label, class) in chips {
                            span { class,
                                if class == "tour-move done" {
                                    Icon { name: IconName::Check }
                                }
                                "{label}"
                            }
                        }
                    }
                }
                div { class: "tour-actions",
                    button {
                        id: control_id("tour", "back"),
                        class: "btn btn-secondary btn-sm",
                        r#type: "button",
                        disabled: html_bool(at == 0),
                        onclick: move |_| go_to_step(tour, at.saturating_sub(1)),
                        Icon { name: IconName::Back }
                        "Back"
                    }
                    if !step.moves.is_empty() {
                        button {
                            id: control_id("tour", "show me"),
                            class: "btn btn-secondary btn-sm",
                            r#type: "button",
                            disabled: html_bool(all_made || shown.is_some()),
                            onclick: show_me,
                            Icon { name: IconName::Play }
                            "Show me"
                        }
                    }
                    if last_step {
                        button {
                            id: control_id("tour", "start practising"),
                            class: "btn btn-primary btn-sm tour-next",
                            r#type: "button",
                            onclick: move |_| {
                                tour.set(None);
                                ctx.go_to(Screen::Home);
                            },
                            "Start practising"
                        }
                    } else {
                        button {
                            id: control_id("tour", "next"),
                            class: if all_made { "btn btn-primary btn-sm tour-next" } else { "btn btn-secondary btn-sm tour-next" },
                            r#type: "button",
                            onclick: move |_| next(ctx, at),
                            "Next"
                        }
                    }
                }
            }
            if !pointer.is_empty() {
                style { "{pointer}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use cw_core::{
        CharSetMode, PresetId, RangeSetting, RangeValues, SequenceChoice, TrainingSettings,
    };

    use super::{LETTER_MS, Move, POINT_MS, SHOWN_MS, STEPS, Seen, pointer_css, scroll_script};
    use crate::audio::fake::Recorder;
    use crate::persist::Store;
    use crate::persist::memory::MemoryStore;
    use crate::state::{Screen, Tour};
    use crate::testing::{Ui, run, stored_session, test_settings};
    use crate::ui::widgets::pretty_number;

    /// Settings on which every move the guide asks for is still to make: a
    /// fixed speed, size and pitch, callsigns, which have no unlock order,
    /// and no auto level.
    fn needs_everything() -> TrainingSettings {
        let mut settings = test_settings();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.min_group_size = 4;
        settings.curriculum.max_group_size = 4;
        settings.curriculum.link_group_size = true;
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.playback.link_char_wpm = true;
        settings.sync_effective_to_char();
        settings.band.side_tone_min = 600.0;
        settings.band.side_tone_max = 600.0;
        settings.auto_level.auto_adjust_level = false;
        settings.auto_level.auto_adjust_above_threshold_count = 5;
        settings.auto_level.auto_adjust_below_threshold_count = 2;
        settings
    }

    /// What the guide sets the trainer up for, from [`needs_everything`].
    fn assert_set_up(settings: &TrainingSettings) {
        let open = |min, max| RangeValues {
            min,
            max,
            linked: false,
        };
        assert_eq!(settings.range(RangeSetting::CharWpm), open(15.0, 25.0));
        assert_eq!(settings.range(RangeSetting::GroupSize), open(2.0, 3.0));
        assert_eq!(settings.range(RangeSetting::SideTone), open(500.0, 700.0));
        assert_eq!(settings.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::MorseMania)
        );
        let auto = &settings.auto_level;
        assert!(auto.auto_adjust_level);
        assert_eq!(auto.auto_adjust_above_threshold_count, 3);
        assert_eq!(auto.auto_adjust_below_threshold_count, 1);
    }

    /// The settings screen as a settings move sees it.
    fn on_settings(settings: &TrainingSettings) -> Seen<'_> {
        Seen {
            settings,
            screen: Screen::Settings,
            tour: Tour::default(),
            group: None,
        }
    }

    /// Make a settings move by hand, on the control with this id.
    fn by_hand(ui: &mut Ui, step_move: Move, id: &str) {
        match step_move {
            Move::Random(_) | Move::Order(_) => ui.click(id),
            Move::AutoLevel => ui.commit(id, "true"),
            Move::Min(_, value) | Move::Max(_, value) => ui.commit(id, &pretty_number(value)),
            Move::UpAfter(sessions) | Move::DownAfter(sessions) => {
                ui.commit(id, &sessions.to_string());
            }
            Move::Start | Move::Answer | Move::Discard | Move::OpenSettings => {
                panic!("{step_move:?} is not made on the settings screen")
            }
        }
    }

    fn moves_made(ui: &Ui) -> usize {
        ui.html().matches("tour-move done").count()
    }

    /// Whether the guide shows step `at`.
    fn on_step(ui: &Ui, at: usize) -> bool {
        ui.has(&format!("Guided setup · {} of {}", at + 1, STEPS.len()))
    }

    /// The first step on the settings screen.
    fn first_on_settings() -> usize {
        STEPS
            .iter()
            .position(|step| step.screen == Some(Screen::Settings))
            .expect("a settings step")
    }

    /// The guide open at the step called `title`, on the settings screen.
    fn opened_at(settings: TrainingSettings, title: &str) -> (Ui, std::rc::Rc<Recorder>) {
        let (mut ui, recorder) = Ui::app_with_settings(settings);
        ui.click("btn-guided-setup");
        ui.click("nav-settings");
        let at = STEPS
            .iter()
            .position(|step| step.title == title)
            .expect("a step of that title");
        for _ in 0..at {
            ui.click("tour-next");
        }
        assert!(on_step(&ui, at), "{}", ui.html());
        (ui, recorder)
    }

    /// The value the answer box for the first group shows.
    fn first_box(ui: &Ui) -> String {
        let tag = ui.input_tag("group-input-0");
        tag.split("value=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_default()
            .to_string()
    }

    /// The first session, by hand: Start training, the group heard typed
    /// into its box, Discard, then Settings. Each step points at its control
    /// and moves on by itself once it is done.
    async fn first_session_by_hand(ui: &mut Ui, recorder: &Recorder) {
        assert!(on_step(ui, 0), "{}", ui.html());
        assert!(ui.has("#btn-start-training {"), "{}", ui.html());
        ui.click("btn-start-training");
        assert_eq!(ui.screen(), "training");
        assert!(on_step(ui, 1), "{}", ui.html());
        assert!(
            ui.has("tour-dock top"),
            "the dock leaves a session its foot"
        );
        assert!(ui.has("#group-input-0 {"), "{}", ui.html());
        assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
        let sent = recorder.texts().first().cloned().expect("a group was sent");
        ui.type_into("group-input-0", &sent);
        assert!(
            ui.run_until(2_000, |ui| on_step(ui, 2)).await,
            "{}",
            ui.html()
        );
        assert!(ui.has("#btn-discard {"), "{}", ui.html());
        ui.click("btn-discard");
        assert_eq!(ui.screen(), "home");
        assert!(on_step(ui, 3), "{}", ui.html());
        assert!(!ui.has("tour-dock top"));
        assert!(ui.has("#nav-settings {"), "{}", ui.html());
        ui.click("nav-settings");
        assert!(on_step(ui, first_on_settings()), "{}", ui.html());
    }

    /// The first launch opens the guide by itself, and only the first: the
    /// next launch waits for the button, which still opens it.
    #[test]
    fn a_first_launch_opens_the_guide_by_itself_and_no_later_one_does() {
        run(|| async {
            let store = std::rc::Rc::new(MemoryStore::default());
            let (mut ui, _recorder) = Ui::launch(store.clone());
            assert_eq!(ui.screen(), "home");
            assert!(on_step(&ui, 0), "{}", ui.html());
            assert!(store.tutorial_done(), "the first launch was not remembered");
            ui.click("tour-close");
            drop(ui);

            let (mut ui, _recorder) = Ui::launch(store);
            assert!(!ui.shows("tour-next"), "it opened by itself again");
            ui.click("btn-guided-setup");
            assert!(on_step(&ui, 0));
        });
    }

    /// An install from before the guide has practised already, so it is not
    /// new to the app: the guide waits to be asked, and once asked, it is
    /// remembered as seen.
    #[test]
    fn an_install_with_a_history_is_not_shown_the_guide_unasked() {
        run(|| async {
            let store = std::rc::Rc::new(MemoryStore::default());
            store.save_sessions(&[stored_session("2026-10-01", &[("KM", "KM")])]);
            let (mut ui, _recorder) = Ui::launch(store.clone());
            assert!(!ui.shows("tour-next"), "{}", ui.html());
            assert!(!store.tutorial_done());
            ui.click("btn-guided-setup");
            assert!(on_step(&ui, 0));
            assert!(store.tutorial_done());
        });
    }

    #[test]
    fn the_guide_opens_on_the_practice_screen_and_closes_again() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            ui.click("nav-settings");
            assert!(!ui.shows("tour-next"));
            ui.click("btn-guided-setup");
            assert_eq!(ui.screen(), "home", "a first session starts on Practice");
            assert!(on_step(&ui, 0));
            assert!(ui.has(STEPS[0].title));
            assert!(ui.input_tag("tour-back").contains("disabled"));
            assert!(ui.has("#btn-start-training {"), "{}", ui.html());
            ui.click("tour-close");
            assert!(!ui.shows("tour-next"));
            assert!(!ui.has("tour-ring"), "the pointer stayed up");
            // The header button opens it, and closes it again.
            ui.click("btn-guided-setup");
            assert!(ui.shows("tour-next"));
            ui.click("btn-guided-setup");
            assert!(!ui.shows("tour-next"));
        });
    }

    /// The guide followed by hand on the real controls: a first session,
    /// then every settings move on the control the guide points at. A label
    /// changed on the settings screen and not here would leave the guide
    /// pointing at nothing, and fails this.
    #[test]
    fn following_the_guide_by_hand_sets_the_trainer_up() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(needs_everything());
            ui.click("btn-guided-setup");
            first_session_by_hand(&mut ui, &recorder).await;
            assert!(ui.store.load_sessions().is_empty(), "the session was kept");
            for (at, step) in STEPS.iter().enumerate().skip(first_on_settings()) {
                assert!(on_step(&ui, at), "step {at} is not {}", step.title);
                for step_move in step.moves.iter().copied() {
                    // Callsigns take two presses to an unlock order: Koch,
                    // then the order.
                    for _ in 0..2 {
                        let settings = ui.stored_settings().await;
                        let seen = on_settings(&settings);
                        if step_move.done(&seen) {
                            break;
                        }
                        let id = step_move
                            .targets(&seen)
                            .into_iter()
                            .find(|id| ui.shows(id))
                            .unwrap_or_else(|| panic!("{step_move:?} has no control on the page"));
                        assert!(
                            ui.has(&format!("#{id}")),
                            "the guide does not point at {id} for {step_move:?}"
                        );
                        by_hand(&mut ui, step_move, &id);
                    }
                    let settings = ui.stored_settings().await;
                    assert!(
                        step_move.done(&on_settings(&settings)),
                        "{step_move:?} was not made"
                    );
                }
                assert_eq!(moves_made(&ui), step.moves.len(), "{}", step.title);
                assert!(
                    !ui.has("tour-ring"),
                    "every move is made, and it still points"
                );
                if at + 1 < STEPS.len() {
                    ui.click("tour-next");
                }
            }
            ui.click("tour-start-practising");
            assert_eq!(ui.screen(), "home");
            assert!(!ui.shows("tour-next"));
            assert_set_up(&ui.store.load_settings());
        });
    }

    #[test]
    fn show_me_on_every_step_takes_a_first_session_and_sets_the_trainer_up() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(needs_everything());
            ui.click("btn-guided-setup");
            for (at, step) in STEPS.iter().enumerate().take(STEPS.len() - 1) {
                assert!(on_step(&ui, at), "not on {}: {}", step.title, ui.html());
                ui.click("tour-show-me");
                if step.moves_on {
                    assert!(
                        ui.run_until(30_000, |ui| on_step(ui, at + 1)).await,
                        "{} did not move on: {}",
                        step.title,
                        ui.html()
                    );
                } else {
                    let budget = 3 * u64::from(POINT_MS + SHOWN_MS) * step.moves.len() as u64;
                    assert!(
                        ui.run_until(budget, |ui| moves_made(ui) == step.moves.len())
                            .await,
                        "{} was not shown: {}",
                        step.title,
                        ui.html()
                    );
                    ui.click("tour-next");
                }
            }
            ui.click("tour-start-practising");
            assert_set_up(&ui.store.load_settings());
            assert!(ui.store.load_sessions().is_empty(), "the session was kept");
        });
    }

    /// Show me copies the group into its box once it has been sent, a letter
    /// at a time, the way keyed letters go in, and gets it right.
    #[test]
    fn show_me_types_the_group_into_its_box_a_letter_at_a_time() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(needs_everything());
            ui.click("btn-guided-setup");
            ui.click("btn-start-training");
            ui.click("tour-show-me");
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            assert!(sent.chars().count() > 1, "{sent}");
            assert_eq!(first_box(&ui), "", "it typed before the box was in sight");
            let first: String = sent.chars().take(1).collect();
            assert!(
                ui.run_until(u64::from(3 * POINT_MS), |ui| first_box(ui) == first)
                    .await,
                "the first letter did not go in alone: {}",
                ui.html()
            );
            let budget = u64::from(LETTER_MS) * (sent.chars().count() as u64 + 8);
            assert!(ui.run_until(budget, |ui| on_step(ui, 2)).await);
            assert!(ui.has("badge good"), "the group was not copied right");
        });
    }

    /// Next does a step that takes you somewhere on the way, so the next one
    /// is on the screen it expects. Out of a session with nothing typed,
    /// Next on Discard discards it: the guide used to point at a Settings
    /// tab the session hides, with the group still waiting to be copied.
    #[test]
    fn next_out_of_a_session_discards_it_on_the_way() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(needs_everything());
            ui.click("btn-guided-setup");
            ui.click("btn-start-training");
            assert!(on_step(&ui, 1));
            // Nothing typed: Next leaves the group to the session.
            ui.click("tour-next");
            assert!(on_step(&ui, 2), "{}", ui.html());
            assert_eq!(ui.screen(), "training");
            ui.click("tour-next");
            assert_eq!(
                ui.screen(),
                "home",
                "Next on Discard left the session running"
            );
            assert!(on_step(&ui, 3), "{}", ui.html());
            assert!(ui.has("#nav-settings {"), "{}", ui.html());
            assert!(ui.store.load_sessions().is_empty(), "the session was kept");
            ui.click("tour-next");
            assert_eq!(ui.screen(), "settings");
            assert!(on_step(&ui, first_on_settings()), "{}", ui.html());
        });
    }

    /// Next on Start training starts the session, as the step asks.
    #[test]
    fn next_on_the_first_step_starts_the_session() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(needs_everything());
            ui.click("btn-guided-setup");
            ui.click("tour-next");
            assert_eq!(ui.screen(), "training");
            assert!(on_step(&ui, 1), "it moved on twice: {}", ui.html());
        });
    }

    /// Ending the session rather than discarding it leaves it too, and the
    /// guide moves on to Settings all the same, from the results.
    #[test]
    fn ending_the_session_instead_also_moves_the_guide_on() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(needs_everything());
            ui.click("btn-guided-setup");
            ui.click("btn-start-training");
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(ui.run_until(2_000, |ui| on_step(ui, 2)).await);
            ui.click("btn-end-session");
            assert_eq!(ui.screen(), "results");
            assert!(on_step(&ui, 3), "{}", ui.html());
            assert!(ui.has("#nav-settings {"));
        });
    }

    /// A step that moves on by itself does so when it is done there, not
    /// when it is come back to done already.
    #[test]
    fn coming_back_to_a_step_done_already_stays_on_it() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-guided-setup");
            ui.click("nav-settings");
            for _ in 0..first_on_settings() {
                ui.click("tour-next");
            }
            ui.click("tour-back");
            ui.advance(500).await;
            assert!(on_step(&ui, first_on_settings() - 1), "{}", ui.html());
            assert_eq!(moves_made(&ui), 1, "Settings is open");
        });
    }

    /// Away from the screen a step's controls are on, the guide points at
    /// the way there.
    #[test]
    fn away_from_its_screen_a_step_points_the_way_there() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-guided-setup");
            ui.click("nav-stats");
            assert!(ui.has("#nav-practice {"), "{}", ui.html());
            // Next runs the first session on the way, and ends on Settings.
            for _ in 0..first_on_settings() {
                ui.click("tour-next");
            }
            assert_eq!(ui.screen(), "settings");
            ui.click("nav-stats");
            assert!(ui.has("#nav-settings {"), "{}", ui.html());
            ui.click("nav-settings");
            assert!(
                ui.has("#pill-morsemania, #pill-cw-academy {"),
                "{}",
                ui.html()
            );
        });
    }

    /// Show me points at a control before it makes the move there, then
    /// points at the next.
    #[test]
    fn show_me_points_at_each_control_before_it_makes_the_move() {
        run(|| async {
            let (mut ui, _recorder) = opened_at(needs_everything(), "Character speed");
            ui.click("tour-show-me");
            assert!(ui.input_tag("tour-show-me").contains("disabled"));
            assert!(ui.has("#link-character-speed {"));
            ui.advance(u64::from(POINT_MS) / 2).await;
            assert!(ui.shows("fixed-character-speed"), "Random was made unseen");
            ui.advance(u64::from(POINT_MS)).await;
            assert!(ui.shows("from-character-speed"), "Random was not made");
            assert_eq!(moves_made(&ui), 1);
            ui.advance(u64::from(SHOWN_MS)).await;
            assert!(ui.has("#from-character-speed {"));

            assert!(
                ui.run_until(5_000, |ui| moves_made(ui) == 3).await,
                "{}",
                ui.html()
            );
            let speed = ui.stored_settings().await.range(RangeSetting::CharWpm);
            assert_eq!((speed.min, speed.max, speed.linked), (15.0, 25.0, false));
            assert!(!ui.input_tag("tour-next").contains("secondary"));
        });
    }

    /// A move already made is left as it is, and so is anything Show me did
    /// not reach before the guide went elsewhere.
    #[test]
    fn show_me_makes_only_what_is_left_and_stops_when_the_guide_moves() {
        run(|| async {
            for away in ["tour-next", "tour-back", "tour-close"] {
                let mut settings = needs_everything();
                settings.playback.link_char_wpm = false;
                settings.playback.char_wpm_max = 25.0;
                let (mut ui, _recorder) = opened_at(settings, "Character speed");
                assert_eq!(moves_made(&ui), 2, "Random and Max 25 are made already");
                ui.click("tour-show-me");
                assert!(ui.has("#from-character-speed {"), "{away}");
                ui.click(away);
                ui.advance(5_000).await;
                let stored = ui.stored_settings().await;
                let speed = stored.range(RangeSetting::CharWpm);
                assert_eq!((speed.min, speed.max), (20.0, 25.0), "after {away}");
                assert_eq!(
                    stored.range(RangeSetting::GroupSize).min,
                    4.0,
                    "after {away}"
                );
            }
        });
    }

    #[test]
    fn a_switch_is_pointed_at_by_its_knob() {
        let css = pointer_css(&[
            "switch-adjust-the-level-for-me".to_string(),
            "pill-koch".to_string(),
        ]);
        assert!(
            css.starts_with("#switch-adjust-the-level-for-me + i, #pill-koch {"),
            "{css}"
        );
        assert!(css.contains("animation: tour-ring"), "{css}");
        assert!(pointer_css(&[]).is_empty());
        assert!(
            include_str!("../../assets/styles.css").contains("@keyframes tour-ring"),
            "the pointer's pulse is the stylesheet's"
        );
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
        // or the head of the window.
        let under = script
            .find("box.bottom > cover.top && box.top < cover.bottom")
            .expect("a control under the dock is not in sight");
        let in_sight = script
            .find("if (inWindow && !underDock)")
            .expect("a control in sight is left where it is");
        let scroll = script
            .find(r#"target.scrollIntoView({ block: "center""#)
            .expect("it is centred");
        assert!(under < in_sight && in_sight < scroll, "{script}");
    }

    /// Every move reads as what it does.
    #[test]
    fn each_move_says_what_it_does() {
        let labels: Vec<Vec<String>> = STEPS
            .iter()
            .map(|step| {
                step.moves
                    .iter()
                    .map(|step_move| step_move.label())
                    .collect()
            })
            .collect();
        assert_eq!(
            labels,
            [
                vec!["Start training"],
                vec!["Type the group"],
                vec!["Discard"],
                vec!["Settings"],
                vec!["MorseMania or CW Academy"],
                vec!["Random", "Min 15 WPM", "Max 25 WPM"],
                vec!["Random", "Min 2 chars", "Max 3 chars"],
                vec!["Random", "Min 500 Hz", "Max 700 Hz"],
                vec!["Auto level on", "Up after 3", "Down after 1"],
                vec![],
            ]
        );
    }
}
