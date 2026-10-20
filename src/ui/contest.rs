//! A contest run's screen, as plain as it can be while every part of the
//! run works: its clock and QSOs, what Enter will send, the Call and
//! Exchange fields, the message buttons, what the run last said, and the
//! log. The design's full layout (§7 of `docs/contest-mode.md`) is drawn
//! over these same parts.

use cw_core::{ContestPhase, ContestView, EntryField, LogRow, MonoMs, OpKey, PauseWhy, QsoState};
use dioxus::prelude::*;

use crate::contest_runtime::{focus_field, pause_run, press_key, resume_run, tick};
use crate::session_runtime::{abort_run, finish_run};
use crate::state::AppCtx;
use crate::time::{mono_ms, sleep_ms};
use crate::ui::format::minutes_seconds;
use crate::ui::paddle::PaddleKeys;
use crate::ui::widgets::{Icon, IconName, control_id, html_bool};

/// How often the run's clock is drawn again, which is also how often the
/// run hears that the app is still running.
const TICK_MS: u32 = 250;

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
    let message = move |key: OpKey, label: &'static str| {
        rsx! {
            button {
                id: control_id("btn", label),
                class: "btn btn-secondary btn-sm",
                r#type: "button",
                disabled: html_bool(!running),
                onclick: move |_| press_key(ctx, key),
                "{label}"
            }
        }
    };
    rsx! {
        div { class: "stack train",
            div { class: "card",
                div { class: "row-between",
                    RunClock {}
                    span { class: "mono", "QSOs {view.qsos} · {view.clean} clean" }
                }
            }
            div { class: "card",
                if let Some(why) = paused {
                    p { class: "train-hint", role: "status", "{paused_words(why)}" }
                    button {
                        id: control_id("btn", "resume"),
                        class: "btn btn-primary",
                        onclick: move |_| resume_run(ctx),
                        Icon { name: IconName::Play }
                        "Resume"
                    }
                } else {
                    EnterLine { qso: view.qso.clone(), live: running }
                    div { class: "field-grid",
                        EntryInput { field: EntryField::Call, disabled: countdown.is_some() }
                        EntryInput { field: EntryField::Exchange, disabled: countdown.is_some() }
                    }
                    div { class: "row",
                        {message(OpKey::Cq, "CQ")}
                        {message(OpKey::Again, "? Again")}
                        {message(OpKey::Wipe, "Wipe")}
                        {message(OpKey::StopTx, "Stop")}
                    }
                }
                if let Some(said) = ctx.announced.read().as_deref() {
                    p { class: "muted", role: "status", aria_live: "polite", "{said}" }
                }
            }
            div { class: "card",
                if let Some(left) = countdown {
                    p { class: "muted", "Your first CQ goes out in {left} s" }
                } else if view.log.is_empty() {
                    p { class: "muted", "Each QSO you log is listed here." }
                } else {
                    for row in view.log.iter().rev() {
                        p { key: "{row.n}", class: "mono", "{log_line(row)}" }
                    }
                }
            }
            div { class: "train-actions",
                if paused.is_none() {
                    button {
                        id: control_id("btn", "pause"),
                        class: "btn btn-secondary",
                        onclick: move |_| pause_run(ctx, PauseWhy::You),
                        Icon { name: IconName::Stop }
                        "Pause"
                    }
                }
                button {
                    id: control_id("btn", "end run"),
                    class: "btn btn-primary",
                    onclick: move |_| finish_run(ctx),
                    Icon { name: IconName::Flag }
                    "End run"
                }
                button {
                    id: control_id("btn", "discard"),
                    class: "btn btn-danger",
                    onclick: move |_| abort_run(ctx),
                    Icon { name: IconName::X }
                    "Discard"
                }
            }
        }
    }
}

/// The run's clock, drawn again every tick from the monotonic clock, kept
/// apart from the view: the machine has no tick of its own. Each tick tells
/// the run the app is still running ([`tick`]).
#[component]
fn RunClock() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut shown = use_signal(move || clock_label(ctx));
    use_future(move || async move {
        loop {
            sleep_ms(TICK_MS).await;
            tick(ctx);
            // Drawn only when what it says has changed.
            let now = clock_label(ctx);
            if *shown.peek() != now {
                shown.set(now);
            }
        }
    });
    rsx! {
        span { class: "mono", role: "timer", aria_live: "off", "{shown}" }
    }
}

/// What the run's clock says now.
fn clock_label(ctx: AppCtx) -> String {
    let open = ctx.settings.peek().contest.contest_minutes == 0;
    ctx.contest
        .peek()
        .as_ref()
        .map(|view| clock_text(view, open, MonoMs(mono_ms())))
        .unwrap_or_default()
}

/// "07:42 / 10:00", or "07:42 · up to 60:00" for a run left `open`; the
/// countdown before it starts.
fn clock_text(view: &ContestView, open: bool, now: MonoMs) -> String {
    if let ContestPhase::Countdown { left } = view.phase {
        return format!("Starting in {left}");
    }
    let run_ms = view
        .clock
        .map_or(0, |clock| clock.peek_ms(now))
        .min(view.limit_ms);
    let (run, limit) = (minutes_seconds(run_ms), minutes_seconds(view.limit_ms));
    if open {
        format!("{run} · up to {limit}")
    } else {
        format!("{run} / {limit}")
    }
}

/// What a paused run says.
fn paused_words(why: PauseWhy) -> &'static str {
    match why {
        PauseWhy::You => "Paused. Your QSO waits.",
        PauseWhy::Hidden => "Paused while Dust was in the background",
        PauseWhy::Sound => "Sound stopped — Resume to carry on",
    }
}

/// One line on what to do next, and Enter, labelled with what it sends:
/// both read from the fields as typed, so they follow every edit while
/// the fields themselves are never drawn for one.
#[component]
fn EnterLine(qso: QsoState, live: bool) -> Element {
    let ctx = use_context::<AppCtx>();
    let Some(enter) = ctx.entry.enter(&qso) else {
        return rsx! {};
    };
    rsx! {
        p { class: "muted train-hint", "{enter.hint}" }
        button {
            id: control_id("btn", "enter"),
            class: "btn btn-primary",
            r#type: "button",
            disabled: html_bool(!live),
            onclick: move |_| press_key(ctx, OpKey::Esm),
            "Enter · {enter.label}"
        }
    }
}

/// One of the two fields.
///
/// A controlled input drawn again while it is typed in drops the key typed
/// in between, so it is drawn only for what the page has not seen: a paddle
/// letter, the fields cleared ([`crate::contest_runtime::ContestEntry`]).
/// Upper case is the stylesheet's; what is typed is read as typed.
#[component]
fn EntryInput(field: EntryField, disabled: bool) -> Element {
    let ctx = use_context::<AppCtx>();
    let entry = ctx.entry;
    let shown = entry.shown(field);
    let (id, label) = field_name(field);
    rsx! {
        div { class: "field",
            label { r#for: id.clone(), "{label}" }
            input {
                id,
                class: "answer",
                value: "{shown}",
                disabled: html_bool(disabled),
                autocomplete: "off",
                autocorrect: "off",
                autocapitalize: "characters",
                spellcheck: false,
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
}

/// A field's id, and what it is called.
pub fn field_name(field: EntryField) -> (String, &'static str) {
    let label = match field {
        EntryField::Call => "Call",
        EntryField::Exchange => "Exchange",
    };
    (control_id("field", label), label)
}

/// A logged QSO as the log lists it: "3  07:31  K5XYZ  NR 214  24 WPM  ?×1
/// clean", its check `…` until the station is done with you.
fn log_line(row: &LogRow) -> String {
    let mut parts = vec![
        row.n.to_string(),
        minutes_seconds(row.at_ms),
        row.call.clone(),
    ];
    parts.extend(
        row.fields
            .iter()
            .map(|(field, value)| format!("{} {value}", field.label())),
    );
    if let Some(wpm) = row.wpm {
        parts.push(format!("{wpm:.0} WPM"));
    }
    if row.asked > 0 {
        parts.push(format!("?×{}", row.asked));
    }
    parts.push(match &row.status {
        cw_core::RowStatus::Pending => "…".to_string(),
        cw_core::RowStatus::Checked(check) => check.words(),
    });
    parts.join("  ")
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
    use crate::persist::Store;
    use crate::testing::{Ui, contest_settings, run};
    use cw_core::{Field, QsoCheck, QsoError, RowStatus, RunClock};

    /// Typing reaches the run however the screen is drawn meanwhile: the
    /// field is never drawn over what was typed in it, while the hint and
    /// Enter's label follow every key.
    #[test]
    fn a_key_typed_between_two_renders_survives() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert!(ui.has("Enter · CQ"));
            // Your CQ, and the caller's answer, heard whole.
            ui.advance(8_000).await;
            let starts = recorder.starts();
            assert_eq!(starts.len(), 1, "one turn so far");
            let call = starts[0].transcript.others[0].text.clone();

            let untouched = ui.input_tag("field-call");
            ui.type_into("field-call", &call[..2]);
            assert!(
                !ui.has("Enter · CQ"),
                "Enter's label did not follow the key"
            );
            // The clock ticks, and the screen is drawn again around the
            // field: the run says something.
            ui.advance(600).await;
            ui.with_app(|ctx| {
                let mut announced = ctx.announced;
                announced.set(Some("Said between two keys".to_string()));
            });
            assert!(ui.has("Said between two keys"));
            ui.type_into("field-call", &call);
            assert_eq!(
                ui.input_tag("field-call"),
                untouched,
                "the field was drawn over what was typed in it"
            );
            assert!(
                ui.has(&format!("Enter · Send {call} + exch")),
                "{}",
                ui.html()
            );
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

    /// A QSO worked on the screen is logged there, and End run, or Exit in
    /// the header, takes the run to its results, kept with its log.
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
                assert!(ui.has(&format!("{call}  NR {serial}")), "{}", ui.html());
                assert!(ui.has("QSOs 1"));
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
            rate: None,
        }
    }

    /// The clock counts run time up against the limit, and stands still
    /// while paused.
    #[test]
    fn the_clock_reads_run_time_against_the_limit() {
        let countdown = view(ContestPhase::Countdown { left: 3 }, None);
        assert_eq!(clock_text(&countdown, false, MonoMs(0)), "Starting in 3");
        let mut clock = RunClock::start(MonoMs(1_000));
        let running = view(ContestPhase::Running, Some(clock));
        assert_eq!(
            clock_text(&running, false, MonoMs(463_000)),
            "07:42 / 10:00"
        );
        assert_eq!(
            clock_text(&running, true, MonoMs(463_000)),
            "07:42 · up to 10:00"
        );
        assert_eq!(
            clock_text(&running, false, MonoMs(9_000_000)),
            "10:00 / 10:00"
        );
        clock.pause(MonoMs(61_000));
        let paused = view(ContestPhase::Paused { why: PauseWhy::You }, Some(clock));
        assert_eq!(clock_text(&paused, false, MonoMs(400_000)), "01:00 / 10:00");
    }

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
        assert_eq!(log_line(&row), "3  07:31  K5XYZ  NR 214  …");
        row.wpm = Some(24.2);
        row.asked = 1;
        row.status = RowStatus::Checked(QsoCheck {
            errors: vec![QsoError::Field(Field::Nr)],
            dupe: false,
        });
        assert_eq!(
            log_line(&row),
            "3  07:31  K5XYZ  NR 214  24 WPM  ?×1  wrong NR"
        );
    }
}
