//! A contest run's screen (§7 of `docs/contest-mode.md`): the run bar, with
//! the clock, the QSOs, the rate and your speed; one line on what Enter does
//! next; the air; the Call and Exchange fields, and what the exchange reads
//! as; the message keys; and the log, newest first. Paused, the fields give
//! way to what waits, and Resume.
//!
//! Every line is a slot one line high, cut short rather than wrapped, so
//! nothing under a line moves as what it says changes, and on a phone the
//! keypad stays where it is from one turn to the next.

use cw_core::{
    ContestPhase, ContestView, EntryField, Field, LogRow, MonoMs, OpKey, PauseWhy, QsoState,
    RowStatus, normalize_call,
};
use dioxus::prelude::*;

use crate::contest_runtime::{change_speed, focus_field, pause_run, press_key, resume_run, tick};
use crate::session_runtime::{abort_run, finish_run};
use crate::state::AppCtx;
use crate::time::{mono_ms, sleep_ms};
use crate::ui::contest_keys::{SPEED_STEP_WPM, use_run_keys};
use crate::ui::format::minutes_seconds;
use crate::ui::paddle::PaddleKeys;
use crate::ui::training::{ANSWER_INPUT_MODE, Keypad, PadKey, PadRow};
use crate::ui::widgets::{Icon, IconName, ProgressHeader, control_id, html_bool};

/// How often the run's clock and the air are drawn again, which is also how
/// often the run hears that the app is still running.
const TICK_MS: u32 = 250;

/// The keypad's character rows: group training's, but for `,` and `.`, which
/// no call or exchange holds. ⌫ ends the fourth.
const PAD_ROWS: [&str; 4] = ["1234567890", "QWERTYUIOP", "ASDFGHJKL/", "ZXCVBNM?"];

/// The run on the training screen.
#[component]
pub fn ContestRunView() -> Element {
    let ctx = use_context::<AppCtx>();
    let keys = use_context::<PaddleKeys>();
    let entry = ctx.entry;
    let view = ctx.contest.read().clone();
    let running = view
        .as_ref()
        .is_some_and(|view| view.phase == ContestPhase::Running);
    // The paddles key into the field with the cursor while the run is on,
    // and a key takes a letter keyed in full with it.
    keys.use_answer_box(running, move |letter| entry.push(letter));
    entry.use_letters(move || keys.finish_letter());
    use_hidden_page(ctx);
    use_run_keys(ctx);
    let now = use_ticks(ctx);
    // The fields take the cursor back as they open: the run starting, or
    // carrying on after a pause.
    use_effect(use_reactive!(|running| {
        if running {
            focus_field(ctx, entry.focus());
        }
    }));
    let Some(view) = view else {
        return rsx! { p { class: "muted", "Starting…" } };
    };
    let countdown = match view.phase {
        ContestPhase::Countdown { left } => Some(left),
        _ => None,
    };
    let paused = match view.phase {
        ContestPhase::Paused { why } => Some(why),
        _ => None,
    };
    rsx! {
        div { class: run_class(paused.is_some()),
            RunBar {
                now,
                phase: view.phase,
                qsos: view.qsos,
                clean: view.clean,
                my_wpm: view.my_wpm,
            }
            if let Some(why) = paused {
                PausedCard { why, serial: view.serial }
            } else {
                EntryCard {
                    qso: view.qso.clone(),
                    running,
                    countdown,
                    now,
                }
                RunKeypad { qso: view.qso.clone(), open: running }
            }
            RunActions { paused: paused.is_some() }
            RunLog {
                rows: view.log.clone(),
                countdown,
                qsos: view.qsos,
                clean: view.clean,
            }
        }
    }
}

/// The screen's classes. The keypad shows wherever touch is how you type
/// (`training.rs`), and there the buttons that pause and end the run come
/// straight under it, the log below them; anywhere else the log comes
/// first. A paused run has no keypad, and its log comes first everywhere.
fn run_class(paused: bool) -> &'static str {
    let touch = cfg!(any(feature = "mobile", feature = "mobile-silent"));
    match (touch, paused) {
        (_, true) => "stack train run paused",
        (true, false) => "stack train run touch",
        (false, false) => "stack train run",
    }
}

/// The time, moved on every tick: what the clock, the rate and the air are
/// drawn from. Each tick tells the run the app is still running ([`tick`]).
fn use_ticks(ctx: AppCtx) -> Signal<MonoMs> {
    let mut now = use_signal(|| MonoMs(mono_ms()));
    use_future(move || async move {
        loop {
            sleep_ms(TICK_MS).await;
            tick(ctx);
            now.set(MonoMs(mono_ms()));
        }
    });
    now
}

/// The run bar: what the run is, its clock against its length, the QSOs and
/// the rate, and your speed with − and +.
#[component]
fn RunBar(now: Signal<MonoMs>, phase: ContestPhase, qsos: u32, clean: u32, my_wpm: f64) -> Element {
    let ctx = use_context::<AppCtx>();
    // Worked out on every tick, and drawn only when it changes: the clock
    // and its bar once a second, the rate when it moves.
    let figures = use_memo(move || {
        let open = ctx.settings.peek().contest.contest_minutes == 0;
        ctx.contest
            .read()
            .as_ref()
            .map(|view| bar_figures(view, open, now()))
            .unwrap_or_default()
    });
    let (figure, figure_note, pct, rate) = figures();
    let run = {
        let contest = &ctx.settings.peek().contest;
        format!("{} as {}", contest.contest_kind.label(), contest.my_call)
    };
    let status = match phase {
        ContestPhase::Countdown { left } => format!("Starting in {left} · {run}"),
        ContestPhase::Running => format!("On air · {run}"),
        ContestPhase::Paused { .. } => format!("Paused · {run}"),
        ContestPhase::Finished | ContestPhase::Aborted => run,
    };
    let live = matches!(
        phase,
        ContestPhase::Countdown { .. } | ContestPhase::Running
    );
    rsx! {
        ProgressHeader {
            status,
            live: phase == ContestPhase::Running,
            figure,
            figure_note,
            pct,
            timer: true,
            div { class: "run-counts",
                span { class: "run-line run-qsos",
                    "QSOs {qsos} · {clean} clean"
                    if let Some(rate) = rate {
                        span { class: "run-rate",
                            span { class: "run-rate-word", "Rate " }
                            "{rate}"
                        }
                    }
                }
                div { class: "run-speed", role: "group", aria_label: "Your speed",
                    span { class: "run-speed-unit", "WPM" }
                    button {
                        id: control_id("less", "wpm"),
                        class: "step",
                        r#type: "button",
                        aria_label: "Slower by {SPEED_STEP_WPM} WPM",
                        disabled: html_bool(!live),
                        onmousedown: keep_focus,
                        onclick: move |_| from_button(ctx, || change_speed(ctx, -SPEED_STEP_WPM)),
                        "−"
                    }
                    span { class: "run-wpm-value", "{my_wpm:.0}" }
                    button {
                        id: control_id("more", "wpm"),
                        class: "step",
                        r#type: "button",
                        aria_label: "Faster by {SPEED_STEP_WPM} WPM",
                        disabled: html_bool(!live),
                        onmousedown: keep_focus,
                        onclick: move |_| from_button(ctx, || change_speed(ctx, SPEED_STEP_WPM)),
                        "+"
                    }
                }
            }
        }
    }
}

/// What the run bar shows at `now`: the clock and what follows it, how much
/// of the run's length has gone, in whole seconds, and the rate once the run
/// is a minute old.
fn bar_figures(
    view: &ContestView,
    open: bool,
    now: MonoMs,
) -> (String, String, f64, Option<String>) {
    let whole_ms = view.run_ms(now) / 1_000 * 1_000;
    let pct = if view.limit_ms == 0 {
        0.0
    } else {
        whole_ms as f64 / view.limit_ms as f64 * 100.0
    };
    let rate = view.rate(now).map(|rate| format!("{rate:.0}/h"));
    let (clock, note) = clock_text(view, open, now);
    (clock, note, pct, rate)
}

/// "07:42 / 10:00"; or for a run left `open`, "07:42" and " · up to 60:00",
/// which a phone leaves out to keep room for what the run is.
fn clock_text(view: &ContestView, open: bool, now: MonoMs) -> (String, String) {
    let (run, limit) = (
        minutes_seconds(view.run_ms(now)),
        minutes_seconds(view.limit_ms),
    );
    if open {
        (run, format!(" · up to {limit}"))
    } else {
        (format!("{run} / {limit}"), String::new())
    }
}

/// A button of the run's leaves the cursor where it is: a click would
/// otherwise take it from the field, so that the letters typed next went
/// nowhere and Enter pressed the button again.
fn keep_focus(e: MouseEvent) {
    e.prevent_default();
}

/// A button of the run's pressed: what it does, then the cursor back in the
/// field it was in, for one pressed from a keyboard or with nothing focused.
fn from_button(ctx: AppCtx, press: impl FnOnce()) {
    press();
    focus_field(ctx, ctx.entry.focus());
}

/// What to do next, the air, the two fields and what the exchange reads as,
/// and the message keys.
#[component]
fn EntryCard(qso: QsoState, running: bool, countdown: Option<u32>, now: Signal<MonoMs>) -> Element {
    let ctx = use_context::<AppCtx>();
    let (exchange_id, _) = field_name(EntryField::Exchange);
    let exchange = exchange_label(ctx.entry.copies());
    rsx! {
        div { class: "card run-entry",
            HintLine { qso: qso.clone(), countdown }
            AirLine { now }
            div { class: "field-grid run-fields",
                CallField { qso: qso.clone(), disabled: countdown.is_some() }
                div { class: "field",
                    label { r#for: exchange_id, "{exchange}" }
                    EntryInput { field: EntryField::Exchange, disabled: countdown.is_some() }
                }
            }
            ReadsAs {}
            KeyBar { qso, live: running }
        }
    }
}

/// "Exchange · NR": the field, and what goes in it.
fn exchange_label(copies: &[Field]) -> String {
    let fields: Vec<&str> = copies.iter().map(|field| field.label()).collect();
    if fields.is_empty() {
        "Exchange".to_string()
    } else {
        format!("Exchange · {}", fields.join(" "))
    }
}

/// One line on what to do next: what Enter does, read from the fields as
/// typed, so it follows every edit while the fields themselves are never
/// drawn for one. Before the first CQ, the seconds to it, where the eye is:
/// on a phone the log's place, where the countdown stands, is below the
/// keypad.
#[component]
fn HintLine(qso: QsoState, countdown: Option<u32>) -> Element {
    let ctx = use_context::<AppCtx>();
    let hint = if let Some(left) = countdown {
        format!("Your first CQ goes out in {left} s")
    } else {
        ctx.entry
            .enter(&qso)
            .map(|enter| enter.hint)
            .unwrap_or_default()
    };
    rsx! {
        p { class: "run-line run-hint", "{hint}" }
    }
}

/// What is on the air: the part of your message still being sent, and how
/// many stations are sending. What they send is never shown.
#[component]
fn AirLine(now: Signal<MonoMs>) -> Element {
    let ctx = use_context::<AppCtx>();
    // Worked out on every tick, and drawn only when it changes: a character
    // gone out, a station starting or stopping.
    let air = use_memo(move || {
        let now = now();
        ctx.contest
            .read()
            .as_ref()
            .and_then(|view| view.air.as_ref())
            .map(|air| (air.still_sending(now).to_string(), air.calling(now)))
            .unwrap_or_default()
    });
    let (sending, calling) = air();
    rsx! {
        div { class: "run-air",
            // Each says "—" when it has nothing, as "Reads as" does.
            p { class: "run-line run-tx",
                span { class: "run-tag", "TX ▸" }
                if sending.is_empty() {
                    span { class: "muted", "—" }
                } else {
                    span { class: "mono", "{sending}" }
                }
            }
            p { class: "run-line run-rx",
                span { class: "run-tag", "RX" }
                if calling > 0 {
                    span { "▮ {calling} calling" }
                } else {
                    span { class: "muted", "—" }
                }
            }
        }
    }
}

/// The Call field, its border turned to a warning while the station's
/// correction waits to be put right. Its own component, so that only its
/// frame follows the typing, never the input inside it.
#[component]
fn CallField(qso: QsoState, disabled: bool) -> Element {
    let ctx = use_context::<AppCtx>();
    let corrected = ctx.entry.enter(&qso).is_some_and(|enter| enter.corrected);
    let (id, label) = field_name(EntryField::Call);
    rsx! {
        div { class: if corrected { "field corrected" } else { "field" },
            label { r#for: id,
                "{label}"
                if corrected {
                    span { class: "field-tag", "corrected" }
                }
            }
            EntryInput { field: EntryField::Call, disabled }
        }
    }
}

/// One of the two fields.
///
/// A controlled input drawn again while it is typed in drops the key typed
/// in between, so it is drawn only for what the page has not seen: a keypad
/// or paddle letter, the fields cleared
/// ([`crate::contest_runtime::ContestEntry`]). Upper case is the
/// stylesheet's; what is typed is read as typed. The phone's own keyboard is
/// kept away, as from the answer box: the keypad types here.
#[component]
fn EntryInput(field: EntryField, disabled: bool) -> Element {
    let ctx = use_context::<AppCtx>();
    let entry = ctx.entry;
    let shown = entry.shown(field);
    let (id, _) = field_name(field);
    rsx! {
        input {
            id,
            class: "answer",
            value: "{shown}",
            disabled: html_bool(disabled),
            autocomplete: "off",
            autocorrect: "off",
            autocapitalize: "characters",
            spellcheck: false,
            enterkeyhint: "send",
            inputmode: ANSWER_INPUT_MODE,
            lang: "zxx",
            oninput: move |e| entry.edit(field, e.value()),
            onfocus: move |_| entry.set_focus(field),
            onkeydown: move |e| {
                if e.key() == Key::Enter {
                    e.prevent_default();
                    press_key(ctx, OpKey::Esm);
                } else if field == EntryField::Call && e.code() == Code::Space {
                    // A call has no spaces: Space moves on to Exchange.
                    e.prevent_default();
                    focus_field(ctx, EntryField::Exchange);
                }
            },
        }
    }
}

/// A field's id, and what it is called.
pub fn field_name(field: EntryField) -> (String, &'static str) {
    let label = match field {
        EntryField::Call => "Call",
        EntryField::Exchange => "Exchange",
    };
    (control_id("field", label), label)
}

/// What the exchange reads as: each field read, a token a later one of its
/// kind replaced, and what cannot be read, which Enter will not send.
#[component]
fn ReadsAs() -> Element {
    let ctx = use_context::<AppCtx>();
    let Some(reading) = ctx.entry.reading() else {
        return rsx! { p { class: "run-line run-reads" } };
    };
    let read: Vec<String> = reading
        .fields
        .iter()
        .map(|read| format!("{} {}", read.field.label(), read.value))
        .chain(reading.unused.iter().map(|token| format!("{token} unused")))
        .collect();
    let unreadable = reading.unreadable.join(" ");
    let nothing = read.is_empty() && unreadable.is_empty();
    let between = if read.is_empty() { "" } else { " · " };
    let read = read.join(" · ");
    rsx! {
        p { class: "run-line run-reads",
            span { class: "muted", "Reads as: " }
            if nothing {
                span { class: "muted", "—" }
            }
            "{read}"
            if !unreadable.is_empty() {
                span { class: "reads-bad", "{between}cannot read {unreadable}" }
            }
        }
    }
}

/// The message keys, each with its key on a keyboard, and Enter named for
/// what it sends. Where touch is how you type, the keypad's own Enter takes
/// the chip's place, and Stop, which a finger never needs, goes.
#[component]
fn KeyBar(qso: QsoState, live: bool) -> Element {
    let ctx = use_context::<AppCtx>();
    let message =
        move |key: OpKey, keyboard: &'static str, label: &'static str, class: &'static str| {
            rsx! {
                button {
                    id: control_id("btn", label),
                    class,
                    r#type: "button",
                    disabled: html_bool(!live),
                    onmousedown: keep_focus,
                    onclick: move |_| from_button(ctx, || press_key(ctx, key)),
                    kbd { "{keyboard}" }
                    "{label}"
                }
            }
        };
    rsx! {
        div { class: "run-keys", role: "group", aria_label: "Messages",
            EnterChip { qso, live }
            {message(OpKey::Cq, "F1", "CQ", "btn btn-secondary btn-sm run-key")}
            {message(OpKey::Again, "F7", "? Again", "btn btn-secondary btn-sm run-key")}
            {message(OpKey::Wipe, "Alt+W", "Wipe", "btn btn-secondary btn-sm run-key")}
            {message(OpKey::StopTx, "Esc", "Stop", "btn btn-secondary btn-sm run-key run-key-desk")}
        }
    }
}

/// Enter, labelled with what it sends: read from the fields as typed. The
/// chip keeps one width, cutting a long label short, so the keys after it
/// stay on their rows whatever it says.
#[component]
fn EnterChip(qso: QsoState, live: bool) -> Element {
    let ctx = use_context::<AppCtx>();
    let label = ctx
        .entry
        .enter(&qso)
        .map(|enter| enter.label)
        .unwrap_or_default();
    rsx! {
        button {
            id: control_id("btn", "enter"),
            class: "btn btn-primary btn-sm run-key run-key-desk run-key-enter",
            r#type: "button",
            disabled: html_bool(!live),
            onmousedown: keep_focus,
            onclick: move |_| from_button(ctx, || press_key(ctx, OpKey::Esm)),
            kbd { "Enter" }
            span { class: "run-key-label", "{label}" }
        }
    }
}

/// The keypad, as a run has it: the characters a call or an exchange can
/// hold, ⌫, the next field, and Enter named for what it sends.
#[component]
fn RunKeypad(qso: QsoState, open: bool) -> Element {
    let ctx = use_context::<AppCtx>();
    let entry = ctx.entry;
    let label = entry
        .enter(&qso)
        .map(|enter| enter.label)
        .unwrap_or_default();
    let on_key = use_callback(move |pressed: PadKey| match pressed {
        PadKey::Char(ch) => entry.push(ch),
        PadKey::Back => entry.pop(),
        PadKey::Field => match entry.focus() {
            EntryField::Call => focus_field(ctx, EntryField::Exchange),
            EntryField::Exchange => entry.push(' '),
        },
        PadKey::Send(_) | PadKey::Next => press_key(ctx, OpKey::Esm),
    });
    rsx! {
        Keypad { open, rows: pad_rows(label), on_key }
    }
}

/// The run's keypad rows, Enter labelled `enter`.
fn pad_rows(enter: String) -> Vec<PadRow> {
    let mut rows: Vec<PadRow> = PAD_ROWS[..3].iter().copied().map(PadRow::chars).collect();
    let mut tail = PadRow::chars(PAD_ROWS[3]);
    tail.keys.push(PadKey::Back);
    tail.class = "keypad-row keypad-tail";
    rows.push(tail);
    rows.push(PadRow {
        keys: vec![PadKey::Field, PadKey::Send(enter)],
        class: "keypad-row keypad-send",
    });
    rows
}

/// A paused run: what waits for you, why it paused if you did not, and
/// Resume.
#[component]
fn PausedCard(why: PauseWhy, serial: u32) -> Element {
    let ctx = use_context::<AppCtx>();
    let call = normalize_call(&ctx.entry.shown(EntryField::Call));
    rsx! {
        div { class: "card run-paused",
            p { class: "run-paused-line",
                Icon { name: IconName::Pause }
                "{paused_line(&call, serial)}"
            }
            if let Some(why) = paused_why(why) {
                p { class: "muted run-paused-why", "{why}" }
            }
            button {
                id: control_id("btn", "resume"),
                class: "btn btn-primary run-resume",
                onclick: move |_| resume_run(ctx),
                Icon { name: IconName::Play }
                "Resume"
            }
        }
    }
}

/// "Paused. Your QSO waits: W7XYZ · 37", after the pause sign: the call as
/// you typed it, and your serial, which a contest without one leaves out.
/// The sign is drawn, as `⏸` is missing from many a phone's fonts.
fn paused_line(call: &str, serial: u32) -> String {
    match (call.is_empty(), serial) {
        (true, 0) => "Paused.".to_string(),
        (true, serial) => format!("Paused. Your next QSO is {serial}"),
        (false, 0) => format!("Paused. Your QSO waits: {call}"),
        (false, serial) => format!("Paused. Your QSO waits: {call} · {serial}"),
    }
}

/// Why a run paused that you did not pause yourself.
fn paused_why(why: PauseWhy) -> Option<&'static str> {
    match why {
        PauseWhy::You => None,
        PauseWhy::Hidden => Some("Paused while Dust was in the background"),
        PauseWhy::Sound => Some("Sound stopped — Resume to carry on"),
    }
}

/// Pause, End run and Discard, which asks once more first: the QSOs it
/// throws away are not kept anywhere.
#[component]
fn RunActions(paused: bool) -> Element {
    let ctx = use_context::<AppCtx>();
    let mut asking = use_signal(|| false);
    if asking() {
        return rsx! {
            div { class: "train-actions run-actions asking",
                p { class: "run-ask", role: "alert", "Discard this run? Its QSOs are not kept" }
                button {
                    id: control_id("btn", "keep run"),
                    class: "btn btn-secondary",
                    onclick: move |_| asking.set(false),
                    "Keep it"
                }
                button {
                    id: control_id("btn", "discard run"),
                    class: "btn btn-danger",
                    onclick: move |_| abort_run(ctx),
                    Icon { name: IconName::X }
                    "Discard"
                }
            }
        };
    }
    rsx! {
        div { class: "train-actions run-actions",
            if !paused {
                button {
                    id: control_id("btn", "pause"),
                    class: "btn btn-secondary",
                    onclick: move |_| pause_run(ctx, PauseWhy::You),
                    Icon { name: IconName::Pause }
                    "Pause"
                }
            }
            // Paused, Resume above is the one thing to press.
            button {
                id: control_id("btn", "end run"),
                class: if paused { "btn btn-secondary" } else { "btn btn-primary" },
                onclick: move |_| finish_run(ctx),
                Icon { name: IconName::Flag }
                "End run"
            }
            button {
                id: control_id("btn", "discard"),
                class: "btn btn-danger",
                onclick: move |_| asking.set(true),
                Icon { name: IconName::X }
                "Discard"
            }
        }
    }
}

/// The log, newest first, six rows tall; before the first CQ, the countdown
/// in its place.
#[component]
fn RunLog(rows: Vec<LogRow>, countdown: Option<u32>, qsos: u32, clean: u32) -> Element {
    rsx! {
        div { class: "card run-log",
            div { class: "row-between run-log-head",
                h3 { class: "card-title", "Log" }
                span { class: "muted mono", "{qsos} QSOs · {clean} clean" }
            }
            // Six rows tall whatever it holds, so what comes under it stays
            // put as the log fills.
            div { class: "run-log-body",
                // Not read out each second: the polite region says what the
                // run does, and the hint the seconds left.
                if let Some(left) = countdown {
                    div { class: "group countdown run-countdown",
                        span { id: "countdown", class: "countdown-num", "{left}" }
                        span { class: "muted", "Your first CQ goes out in {left} s" }
                    }
                } else if rows.is_empty() {
                    p { class: "muted run-log-empty", "Each QSO you log is listed here." }
                } else {
                    ol { class: "run-log-rows",
                        for row in rows.iter().rev() {
                            LogRowView {
                                key: "{row.n}",
                                n: row.n.to_string(),
                                at_ms: row.at_ms,
                                call: row.call.clone(),
                                fields: fields_text(row),
                                wpm: row.wpm,
                                asked: row.asked,
                                check: check_text(row),
                                wrong: is_wrong(row),
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One row of a log, the run's or its results': its number, when, the call
/// and the exchange, the station's speed, the repeats you asked for, and how
/// it checked, in words. One that went `wrong` reads in the colour of a
/// mistake, beside its words.
#[component]
pub(crate) fn LogRowView(
    n: String,
    at_ms: u64,
    call: String,
    fields: String,
    wpm: Option<f64>,
    asked: u32,
    check: String,
    wrong: bool,
) -> Element {
    rsx! {
        li { class: if wrong { "log-row wrong" } else { "log-row" },
            span { class: "log-n", "{n}" }
            span { class: "log-at", "{minutes_seconds(at_ms)}" }
            span { class: "log-call", "{call}" }
            span { class: "log-fields", "{fields}" }
            span { class: "log-wpm",
                if let Some(wpm) = wpm {
                    "{wpm:.0} WPM"
                }
            }
            span { class: "log-asked",
                if asked > 0 {
                    "?×{asked}"
                }
            }
            span { class: "log-check", "{check}" }
        }
    }
}

/// Whether a logged QSO's check found something wrong.
fn is_wrong(row: &LogRow) -> bool {
    matches!(&row.status, RowStatus::Checked(check) if !check.is_clean())
}

/// What was logged of a QSO's exchange: "214".
fn fields_text(row: &LogRow) -> String {
    let values: Vec<&str> = row.fields.iter().map(|(_, value)| value.as_str()).collect();
    values.join(" ")
}

/// A row's check in words, `…` until the station is done with you.
fn check_text(row: &LogRow) -> String {
    match &row.status {
        RowStatus::Pending => "…".to_string(),
        RowStatus::Checked(check) => check.words(),
    }
}

/// The one place the run speaks to a screen reader: discrete events only,
/// each once the air is silent. It sits in the app's frame, not the run
/// screen, so that what the run says as it ends is still said on the
/// results.
#[component]
pub fn RunAnnouncer() -> Element {
    let ctx = use_context::<AppCtx>();
    let said = ctx.announced.read().clone();
    rsx! {
        div { class: "sr-only", role: "status", aria_live: "polite",
            if let Some(said) = said {
                p { key: "{said.serial}", "{said.text}" }
            }
        }
    }
}

/// While the run screen is up, the page going out of sight pauses the run:
/// a tab left, a phone locked, a window minimised. A run paused so never
/// carries on by itself. A process frozen whole, which hears neither, is
/// caught by the clock's ticks instead ([`tick`]).
fn use_hidden_page(ctx: AppCtx) {
    listen_visibility(ctx);
    listen_minimised(ctx);
}

/// The script that tells the run whether the page is hidden, each time that
/// changes. A listener left by a run before is taken off first.
fn visibility_script() -> String {
    format!(
        r#"{STOP_WATCHING}
const tell = () => dioxus.send(document.hidden);
document.addEventListener("visibilitychange", tell);
window.__dustRunHidden = () => document.removeEventListener("visibilitychange", tell);"#
    )
}

/// The script that takes the listener off again.
const STOP_WATCHING: &str = r#"if (window.__dustRunHidden) {
    window.__dustRunHidden();
    window.__dustRunHidden = null;
}"#;

fn listen_visibility(ctx: AppCtx) {
    // Tests have no page to run it on.
    #[cfg(not(test))]
    {
        use_hook(move || {
            spawn(async move {
                let mut page = document::eval(&visibility_script());
                while let Ok(hidden) = page.recv::<bool>().await {
                    if hidden {
                        pause_run(ctx, PauseWhy::Hidden);
                    }
                }
            })
        });
        use_drop(|| {
            dioxus::core::spawn_forever(async {
                let _ = document::eval(STOP_WATCHING).await;
            });
        });
    }
    #[cfg(test)]
    let _ = ctx;
}

/// A minimised window is never hidden as a page, so the desktop build asks
/// the window itself whenever it is resized or loses the focus.
#[cfg(all(feature = "desktop", not(test)))]
fn listen_minimised(ctx: AppCtx) {
    let desktop = use_hook(dioxus::desktop::window);
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{Event, WindowEvent};
        let Event::WindowEvent {
            window_id,
            event: WindowEvent::Resized(_) | WindowEvent::Focused(_),
            ..
        } = event
        else {
            return;
        };
        if *window_id == desktop.window.id() && desktop.window.is_minimized() {
            pause_run(ctx, PauseWhy::Hidden);
        }
    });
}

#[cfg(not(all(feature = "desktop", not(test))))]
fn listen_minimised(_ctx: AppCtx) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::Recorder;
    use crate::persist::Store;
    use crate::testing::{Ui, contest_settings, run};
    use cw_core::{QsoCheck, QsoError, RunClock};

    /// The text of the first element whose class list starts with `class`,
    /// with the tags inside it taken out and the markup's escapes undone.
    fn text_of(ui: &Ui, class: &str) -> String {
        let html = ui.html();
        let at = html
            .find(&format!("class=\"{class}"))
            .unwrap_or_else(|| panic!("no {class} in {html}"));
        let open = html[..at].rfind('<').expect("a tag opens");
        let tag: String = html[open + 1..]
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        let inside = at + html[at..].find('>').expect("a tag closes") + 1;
        // Up to its own closing tag, past any of its kind inside it.
        let close = format!("</{tag}>");
        let opens = [format!("<{tag}>"), format!("<{tag} ")];
        let mut depth = 1;
        let mut end = inside;
        while depth > 0 {
            let next_close = end + html[end..].find(&close).expect("closed");
            let nested = opens
                .iter()
                .filter_map(|open| html[end..next_close].find(open.as_str()))
                .min();
            match nested {
                Some(nested) => {
                    depth += 1;
                    end += nested + tag.len() + 1;
                }
                None => {
                    depth -= 1;
                    end = if depth == 0 {
                        next_close
                    } else {
                        next_close + close.len()
                    };
                }
            }
        }
        let mut text = String::new();
        let mut in_tag = false;
        for ch in html[inside..end].chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => text.push(ch),
                _ => {}
            }
        }
        text.replace("&amp;", "&")
            .replace("&#38;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#x27;", "'")
    }

    /// The hint line, and what Enter's chip says it sends.
    fn enter(ui: &Ui) -> (String, String) {
        let label = text_of(ui, "btn btn-primary btn-sm run-key");
        let label = label.strip_prefix("Enter").unwrap_or(&label).to_string();
        (text_of(ui, "run-line run-hint"), label)
    }

    /// A run started on the app, with its first caller heard: the call it
    /// sent.
    async fn first_caller() -> (Ui, std::rc::Rc<Recorder>, String) {
        let (mut ui, recorder) = Ui::app_with_settings(contest_settings());
        ui.click("btn-start-training");
        ui.advance(8_000).await;
        let call = recorder.starts()[0].transcript.others[0].text.clone();
        (ui, recorder, call)
    }

    /// Typing reaches the run however the screen is drawn meanwhile: the
    /// field is never drawn over what was typed in it, while the hint and
    /// Enter's label follow every key.
    #[test]
    fn a_key_typed_between_two_renders_survives() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert_eq!(enter(&ui).1, "CQ");
            // Your CQ, and the caller's answer, heard whole.
            ui.advance(8_000).await;
            let starts = recorder.starts();
            assert_eq!(starts.len(), 1, "one turn so far");
            let call = starts[0].transcript.others[0].text.clone();

            let untouched = ui.input_tag("field-call");
            ui.type_into("field-call", &call[..2]);
            assert_ne!(enter(&ui).1, "CQ", "Enter's label did not follow the key");
            // The clock ticks, and the screen is drawn again around the
            // field: the run publishes what it shows.
            ui.advance(600).await;
            ui.with_app(|ctx| {
                let mut contest = ctx.contest;
                contest.write();
            });
            ui.type_into("field-call", &call);
            assert_eq!(
                ui.input_tag("field-call"),
                untouched,
                "the field was drawn over what was typed in it"
            );
            assert_eq!(enter(&ui).1, format!("Send {call} + exch"));
            ui.press_enter("field-call");
            let sent = recorder
                .starts()
                .last()
                .map(|start| start.transcript.text.clone());
            assert!(
                sent.as_deref()
                    .is_some_and(|sent| sent.starts_with(&format!("{call} 5NN"))),
                "{sent:?}"
            );
        });
    }

    /// A QSO worked on the screen is logged there, in words, and End run,
    /// or Exit in the header, takes the run to its results, kept with its
    /// log.
    #[test]
    fn a_run_worked_on_the_screen_ends_on_its_results() {
        run(|| async {
            for button in ["btn-end-run", "btn-exit"] {
                let (mut ui, recorder) = Ui::app_with_settings(contest_settings());
                ui.click("btn-start-training");
                assert!(ui.has("Each QSO you log is listed here."));
                ui.advance(8_000).await;
                let call = recorder.starts()[0].transcript.others[0].text.clone();
                ui.type_into("field-call", &call);
                ui.press_enter("field-call");
                ui.advance(8_000).await;
                let starts = recorder.starts();
                assert_eq!(starts.len(), 2, "your call and exchange, and its own");
                let exchange = starts[1].transcript.others[0].text.clone();
                let serial = exchange.split_whitespace().last().unwrap_or_default();
                ui.type_into("field-exchange", serial);
                ui.press_enter("field-exchange");
                assert_eq!(text_of(&ui, "log-call"), call);
                assert_eq!(text_of(&ui, "log-fields"), serial);
                assert_eq!(text_of(&ui, "log-check"), "…", "the TU is on the air");
                assert!(
                    ui.run_until(15_000, |ui| text_of(ui, "log-check") == "clean")
                        .await
                );
                assert!(ui.has("QSOs 1 · 1 clean"));
                ui.click(button);
                assert_eq!(ui.screen(), "results", "{button}");
                let stored = ui.store.load_sessions();
                assert_eq!(stored.len(), 1, "{button}");
                let run = stored[0]
                    .contest
                    .as_ref()
                    .and_then(|log| log.run(stored[0].groups.len()))
                    .expect("its log");
                assert_eq!(run.qsos.len(), 1, "{button}");
                assert_eq!(stored[0].groups[0].received, call, "{button}");
            }
        });
    }

    /// Enter's chip and the hint say what Enter does in each row of the
    /// design's table (§4.4), and "Reads as" what the exchange reads as.
    #[test]
    fn the_hint_and_enter_say_what_enter_does_in_each_row() {
        run(|| async {
            let (mut ui, recorder, call) = first_caller().await;
            assert_eq!(
                enter(&ui),
                ("Type the call you hear, then Enter".into(), "CQ".into())
            );
            ui.type_into("field-call", &call[..2]);
            assert_eq!(
                enter(&ui),
                (
                    "Not a whole call: finish it, or put ? for a gap".into(),
                    format!("Send {}", &call[..2])
                )
            );
            ui.type_into("field-call", &format!("{}?", &call[..2]));
            assert_eq!(
                enter(&ui),
                (
                    "Enter asks for the call: fill in what you hear".into(),
                    format!("Ask {}?", &call[..2])
                )
            );
            ui.type_into("field-call", &call);
            assert_eq!(
                enter(&ui),
                (
                    "Enter sends their call and your exchange".into(),
                    format!("Send {call} + exch")
                )
            );
            ui.press_enter("field-call");
            assert!(ui.run_until(15_000, |ui| enter(ui).1 == "? Again").await);
            assert_eq!(
                enter(&ui),
                ("Copy their number, then TU & log".into(), "? Again".into())
            );
            assert_eq!(text_of(&ui, "run-line run-reads"), "Reads as: —");
            ui.type_into("field-exchange", "BOB");
            assert_eq!(
                enter(&ui),
                (
                    "Exchange cannot be read: fix it, then Enter".into(),
                    "Fix exchange".into()
                )
            );
            assert_eq!(
                text_of(&ui, "run-line run-reads"),
                "Reads as: cannot read BOB"
            );
            assert!(ui.has("class=\"reads-bad\""), "marked as a fault");
            let exchange = recorder.starts()[1].transcript.others[0].text.clone();
            let serial = exchange.split_whitespace().last().unwrap_or_default();
            ui.type_into("field-exchange", &format!("12 {serial}"));
            assert_eq!(
                enter(&ui),
                ("Enter sends TU and logs the QSO".into(), "TU & log".into())
            );
            let number = cw_core::read_number(serial).unwrap_or_default();
            assert_eq!(
                text_of(&ui, "run-line run-reads"),
                format!("Reads as: NR {number} · 12 unused")
            );
        });
    }

    /// A station's correction turns the Call field's border to a warning,
    /// and Enter waits for the call to be put right.
    #[test]
    fn a_correction_marks_the_call_until_it_is_put_right() {
        run(|| async {
            let (mut ui, _recorder, call) = first_caller().await;
            // One letter of its suffix wrong: still a call, and close.
            let base = call.split('/').next().unwrap_or(&call);
            let (at, letter) = base
                .char_indices()
                .rfind(|(_, ch)| ch.is_ascii_alphabetic())
                .expect("a suffix");
            let other = if letter == 'Q' { "X" } else { "Q" };
            let slip = format!("{}{other}{}", &call[..at], &call[at + 1..]);
            assert!(cw_core::parse_callsign(&slip).is_some(), "{slip}");
            ui.type_into("field-call", &slip);
            assert!(!ui.has("field corrected"));
            ui.press_enter("field-call");
            // Your call and exchange, then the correction, heard whole.
            assert!(ui.run_until(15_000, |ui| enter(ui).1 == "Fix call").await);
            assert_eq!(
                enter(&ui),
                (
                    "They corrected you: fix the call, then Enter".into(),
                    "Fix call".into()
                )
            );
            assert!(ui.has("class=\"field corrected\""), "{}", ui.html());
            ui.type_into("field-call", &call);
            assert!(!ui.has("field corrected"));
        });
    }

    /// Before the first CQ the countdown stands where the log will be, and
    /// the fields and keys wait.
    #[test]
    fn the_countdown_stands_in_the_logs_place() {
        run(|| async {
            let mut settings = contest_settings();
            settings.playback.countdown_sec = 3;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            // The seconds stand in the hint, where the eye is, and in the
            // log's place; neither is read out each second.
            assert_eq!(
                text_of(&ui, "run-line run-hint"),
                "Your first CQ goes out in 3 s"
            );
            assert!(ui.has("Your first CQ goes out in 3 s</span>"));
            assert!(!ui.input_tag("countdown").contains("role="));
            assert!(!ui.has(r#"run-countdown" role"#));
            assert!(!ui.has("Each QSO you log is listed here."));
            assert!(ui.input_tag("field-call").contains("disabled"));
            assert!(ui.input_tag("field-exchange").contains("disabled"));
            assert!(ui.input_tag("btn-cq").contains("disabled"));
            assert!(ui.has("Starting in 3 · CQ WPX as DL1ABC"));
            assert!(ui.has("00:00 / 10:00"));
            ui.advance(3_100).await;
            assert!(ui.has("Each QSO you log is listed here."));
            assert!(!ui.input_tag("field-call").contains("disabled"));
            assert!(ui.has("On air · CQ WPX as DL1ABC"));
        });
    }

    /// Running: the clock is a timer that is not read out tick by tick, the
    /// air shows your message as it goes and who is calling, never what they
    /// send, and the rate shows once the run is a minute old.
    #[test]
    fn the_run_bar_and_the_air_follow_the_run() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-start-training");
            ui.advance(300).await;
            let tx = text_of(&ui, "run-line run-tx");
            assert!(tx.starts_with("TX ▸CQ DL1ABC"), "{tx}");
            assert!(ui.has(r#"role="timer""#) && ui.has(r#"aria-live="off""#));
            assert!(ui.run_until(8_000, |ui| ui.has("1 calling")).await);
            let call = recorder.starts()[0].transcript.others[0].text.clone();
            assert!(!text_of(&ui, "run-air").contains(&call), "what they send");
            assert_eq!(text_of(&ui, "run-line run-tx"), "TX ▸—", "yours is out");
            assert!(!ui.has("run-rate"));
            ui.advance(61_000).await;
            assert_eq!(text_of(&ui, "run-rate"), "Rate 0/h");
            assert!(ui.has("01:0"));
        });
    }

    /// Paused, the fields give way to what waits — the call as typed, and
    /// your serial — with Resume, the log, and End run and Discard below.
    /// A pause you did not ask for says why.
    #[test]
    fn a_paused_run_says_what_waits() {
        run(|| async {
            let (mut ui, _recorder, call) = first_caller().await;
            ui.type_into("field-call", &call.to_lowercase());
            ui.click("btn-pause");
            // Said once, in the one region a screen reader listens to.
            assert_eq!(text_of(&ui, "sr-only"), "Paused");
            assert!(ui.has(r#"class="sr-only" role="status" aria-live="polite""#));
            assert_eq!(
                text_of(&ui, "run-paused-line"),
                format!("Paused. Your QSO waits: {call} · 1")
            );
            assert!(!ui.shows("field-call") && !ui.shows("key-q"));
            assert!(!ui.shows("btn-pause"));
            assert!(ui.shows("btn-end-run") && ui.shows("btn-discard"));
            assert!(ui.has("Each QSO you log is listed here."));
            assert!(!ui.has("in the background"));
            ui.click("btn-resume");
            assert!(ui.shows("field-call"));
            ui.with_app(|ctx| crate::contest_runtime::pause_run(ctx, PauseWhy::Hidden));
            assert!(ui.has("Paused while Dust was in the background"));
            ui.click("btn-resume");
            // Discard asks once more, and Keep it keeps it.
            ui.click("btn-discard");
            assert!(ui.has("Discard this run? Its QSOs are not kept"));
            ui.click("btn-keep-run");
            assert_eq!(ui.screen(), "training");
            ui.click("btn-discard");
            ui.click("btn-discard-run");
            assert_eq!(ui.screen(), "home");
        });
    }

    /// The keypad keeps the characters a call or an exchange holds, leaves
    /// out what neither does, and ends in the next field and Enter, named
    /// for what it sends, aloud as on screen.
    #[test]
    fn the_keypad_types_calls_and_exchanges() {
        run(|| async {
            let (mut ui, recorder, call) = first_caller().await;
            for gone in [
                "key-equals",
                "key-plus",
                "key-comma",
                "key-period",
                "key-next",
            ] {
                assert!(!ui.shows(gone), "{gone}");
            }
            for kept in [
                "key-q",
                "key-0",
                "key-slash",
                "key-question-mark",
                "key-delete",
            ] {
                assert!(ui.shows(kept), "{kept}");
            }
            assert!(ui.input_tag("key-enter").contains(r#"aria-label="CQ""#));
            assert!(ui.input_tag("field-call").contains(r#"inputmode="none""#));
            assert!(
                ui.input_tag("field-exchange")
                    .contains(r#"inputmode="none""#)
            );
            for ch in call.chars() {
                let key = if ch == '/' {
                    "key-slash".to_string()
                } else {
                    control_id("key", &ch.to_string())
                };
                ui.tap(&key);
            }
            ui.tap("key-q");
            ui.tap("key-delete");
            assert!(
                ui.input_tag("field-call")
                    .contains(&format!(r#"value="{call}""#))
            );
            let label = format!("Send {call} + exch");
            assert!(
                ui.input_tag("key-enter")
                    .contains(&format!(r#"aria-label="{label}""#))
            );
            assert!(ui.has(&format!(r#"<span class="key-label">{label}</span>"#)));
            assert!(ui.has(r#"<span class="key-return">↵</span>"#));
            ui.tap("key-field");
            ui.tap("key-1");
            ui.tap("key-field");
            ui.tap("key-2");
            assert!(ui.input_tag("field-exchange").contains(r#"value="1 2""#));
            // Enter sends the call and your exchange: the station has not
            // sent its own yet, so what was typed is no copy of it.
            ui.tap("key-enter");
            let sent = recorder
                .starts()
                .last()
                .map(|start| start.transcript.text.clone());
            assert!(
                sent.as_deref()
                    .is_some_and(|sent| sent.starts_with(&format!("{call} 5NN"))),
                "{sent:?}"
            );
            assert!(ui.run_until(15_000, |ui| enter(ui).1 == "TU & log").await);
        });
    }

    /// A run that worked nobody ends on a panel that says nothing was kept,
    /// and offers another.
    #[test]
    fn a_run_with_nobody_worked_says_nothing_was_kept() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-start-training");
            ui.click("btn-end-run");
            assert_eq!(ui.screen(), "results");
            assert!(ui.has("No stations worked — nothing kept"));
            assert!(ui.store.load_sessions().is_empty());
            ui.click("btn-run-again");
            assert_eq!(ui.screen(), "training");
            assert!(ui.shows("field-call"));
        });
    }

    /// Space in the Call field moves on to Exchange, as a call has no
    /// spaces, and types nothing.
    #[test]
    fn space_in_the_call_moves_on_to_the_exchange() {
        run(|| async {
            let (mut ui, _recorder, call) = first_caller().await;
            ui.type_into("field-call", &call);
            assert_eq!(ui.with_app(|ctx| ctx.entry.focus()), EntryField::Call);
            ui.press_key("field-call", Key::Character(" ".into()), Code::Space);
            assert_eq!(ui.with_app(|ctx| ctx.entry.focus()), EntryField::Exchange);
            assert_eq!(ui.with_app(|ctx| ctx.entry.shown(EntryField::Call)), call);
            // A space in Exchange is a space.
            ui.press_key("field-exchange", Key::Character(" ".into()), Code::Space);
            assert_eq!(ui.with_app(|ctx| ctx.entry.focus()), EntryField::Exchange);
        });
    }

    /// A button of the run's is pressed by its click alone, and a press of
    /// it, or a key from the keyboard, leaves the cursor in the field it was
    /// in.
    #[test]
    fn a_button_leaves_the_cursor_in_its_field() {
        run(|| async {
            let (mut ui, recorder, _call) = first_caller().await;
            ui.with_app(|ctx| focus_field(ctx, EntryField::Exchange));
            let sent = recorder.starts().len();
            ui.tap("btn-cq");
            assert_eq!(recorder.starts().len(), sent + 1, "one CQ");
            assert_eq!(ui.with_app(|ctx| ctx.entry.focus()), EntryField::Exchange);
            ui.tap("more-wpm");
            assert!(ui.has(r#"run-wpm-value">32<"#), "{}", ui.html());
            assert_eq!(ui.with_app(|ctx| ctx.entry.focus()), EntryField::Exchange);
        });
    }

    /// The guided setup points at a session's group boxes, which a run does
    /// not have: in Contest mode its button waits.
    #[test]
    fn the_guided_setup_waits_in_contest_mode() {
        run(|| async {
            let (ui, _recorder) = Ui::app_with_settings(contest_settings());
            assert!(!ui.shows("btn-guided-setup"));
            let (ui, _recorder) = Ui::app();
            assert!(ui.shows("btn-guided-setup"));
        });
    }

    /// The harness never mounts anything on a page, so the script is the
    /// part a test can see: the page tells the run each time it is hidden or
    /// shown, and the listener comes off again, a run before's first.
    #[test]
    fn the_page_says_when_it_is_hidden_until_the_run_is_over() {
        let script = visibility_script();
        assert!(script.contains("dioxus.send(document.hidden)"), "{script}");
        assert!(
            script.contains(r#"document.addEventListener("visibilitychange", tell)"#),
            "{script}"
        );
        assert!(
            script.contains(r#"document.removeEventListener("visibilitychange", tell)"#),
            "{script}"
        );
        let off = script.find("window.__dustRunHidden();").expect("taken off");
        let on = script.find("addEventListener").expect("put on");
        assert!(off < on, "{script}");
        assert!(STOP_WATCHING.contains("window.__dustRunHidden = null;"));
    }

    fn view(phase: ContestPhase, clock: Option<RunClock>) -> ContestView {
        ContestView {
            phase,
            clock,
            limit_ms: 600_000,
            my_wpm: 25.0,
            qso: QsoState::default(),
            air: None,
            log: Vec::new(),
            qsos: 0,
            clean: 0,
            serial: 1,
        }
    }

    /// The clock counts run time up against the limit, and stands still
    /// while paused; its bar moves a second at a time. An open run's limit
    /// is a note after the clock, which a phone leaves out.
    #[test]
    fn the_clock_reads_run_time_against_the_limit() {
        let clock_at = |view: &ContestView, open: bool, now: u64| {
            let (clock, note) = clock_text(view, open, MonoMs(now));
            format!("{clock}{note}")
        };
        let countdown = view(ContestPhase::Countdown { left: 3 }, None);
        assert_eq!(clock_at(&countdown, false, 0), "00:00 / 10:00");
        let mut clock = RunClock::start(MonoMs(1_000));
        let running = view(ContestPhase::Running, Some(clock));
        assert_eq!(clock_at(&running, false, 463_000), "07:42 / 10:00");
        assert_eq!(
            clock_text(&running, true, MonoMs(463_000)),
            ("07:42".to_string(), " · up to 10:00".to_string())
        );
        assert_eq!(clock_at(&running, false, 9_000_000), "10:00 / 10:00");
        let (_, _, half, rate) = bar_figures(&running, false, MonoMs(301_400));
        assert_eq!((half, rate), (50.0, Some("0/h".to_string())));
        let (_, _, early, rate) = bar_figures(&running, false, MonoMs(31_999));
        assert_eq!((early, rate), (5.0, None), "no rate in the first minute");
        clock.pause(MonoMs(61_000));
        let paused = view(ContestPhase::Paused { why: PauseWhy::You }, Some(clock));
        assert_eq!(clock_at(&paused, false, 400_000), "01:00 / 10:00");
    }

    /// A log row says what was logged and how it checked, in words.
    #[test]
    fn a_log_row_says_what_was_logged_and_how_it_checked() {
        let mut row = LogRow {
            n: 3,
            at_ms: 451_200,
            call: "K5XYZ".to_string(),
            fields: vec![(Field::Nr, "214".to_string())],
            wpm: None,
            asked: 0,
            status: RowStatus::Pending,
        };
        assert_eq!(
            (fields_text(&row), check_text(&row)),
            ("214".into(), "…".into())
        );
        assert!(!is_wrong(&row));
        row.wpm = Some(24.2);
        row.asked = 1;
        row.status = RowStatus::Checked(QsoCheck {
            errors: vec![QsoError::Call {
                correct: "K5XYW".into(),
            }],
            dupe: false,
        });
        assert_eq!(check_text(&row), "busted call, was K5XYW");
        assert!(is_wrong(&row));
    }

    #[test]
    fn a_pause_says_what_waits() {
        assert_eq!(
            paused_line("W7XYZ", 37),
            "Paused. Your QSO waits: W7XYZ · 37"
        );
        assert_eq!(paused_line("", 37), "Paused. Your next QSO is 37");
        assert_eq!(paused_line("W7XYZ", 0), "Paused. Your QSO waits: W7XYZ");
        assert_eq!(paused_line("", 0), "Paused.");
        assert_eq!(paused_why(PauseWhy::You), None);
        assert_eq!(
            paused_why(PauseWhy::Sound),
            Some("Sound stopped — Resume to carry on")
        );
    }

    /// The keypad's rows: group training's characters but `,` and `.`, ⌫
    /// after `?`, then the next field and Enter.
    #[test]
    fn the_run_keypad_has_no_keys_a_contest_never_types() {
        let rows = pad_rows("TU & log".into());
        let chars: String = rows
            .iter()
            .flat_map(|row| &row.keys)
            .filter_map(|key| match key {
                PadKey::Char(ch) => Some(*ch),
                _ => None,
            })
            .collect();
        for gone in ['=', '+', ',', '.'] {
            assert!(!chars.contains(gone), "{gone}");
        }
        for kept in ('A'..='Z').chain('0'..='9').chain(['/', '?']) {
            assert_eq!(chars.matches(kept).count(), 1, "{kept}");
        }
        assert_eq!(rows[3].keys.last(), Some(&PadKey::Back));
        assert_eq!(
            rows[4].keys,
            [PadKey::Field, PadKey::Send("TU & log".into())]
        );
        assert_eq!(exchange_label(&[Field::Nr]), "Exchange · NR");
        assert_eq!(exchange_label(&[]), "Exchange");
    }

    /// The longest lines a WPX run shows stay one line in a phone's width:
    /// each is a slot the stylesheet cuts short rather than wraps.
    #[test]
    fn the_longest_lines_keep_to_one_line() {
        run(|| async {
            let (mut ui, _recorder, call) = first_caller().await;
            ui.type_into("field-call", &format!("{}?", &call[..1]));
            ui.type_into("field-exchange", "5NN TT1 12 BOB 3X");
            for class in [
                "run-line run-hint",
                "run-line run-tx",
                "run-line run-rx",
                "run-line run-reads",
                "run-line run-qsos",
            ] {
                assert!(ui.has(&format!("class=\"{class}")), "{class}");
            }
            assert!(ui.has(r#"class="progress-status""#));
            assert!(ui.has(r#"class="key-label""#));
            let css = include_str!("../../assets/styles.css");
            // The slots, the run bar's status, and the label on the
            // keypad's Enter, before its ↵.
            for selector in [".run-line {", ".progress-status {", ".key-label {"] {
                let rule = css
                    .split(selector)
                    .nth(1)
                    .and_then(|rest| rest.split('}').next())
                    .unwrap_or_else(|| panic!("a rule for {selector}"));
                for needed in [
                    "white-space: nowrap",
                    "overflow: hidden",
                    "text-overflow: ellipsis",
                ] {
                    assert!(rule.contains(needed), "{needed} in {selector}{rule}");
                }
            }
        });
    }
}
