//! Runs a contest: the machine decides, and this carries out what it decides
//! — turns, timers, the results — under the run's lease on the audio. The
//! run screen hands it keys, with the fields as typed, and says when the
//! page has gone out of sight.
//!
//! As for a session of groups ([`crate::session_runtime`]), its tasks are
//! spawned on the root scope, and the lease, not a component, decides how
//! long they live. A pause gives the audio back altogether, so the band
//! falls silent and the screen may sleep; Resume claims it again inside the
//! tap, where a browser lets a player start.

use cw_core::{
    ContestEffect, ContestEvent, ContestMachine, ContestPhase, ContestRules, EntryField, Esm,
    Field, Fields, MonoMs, OpKey, PauseWhy, QsoState, Reading, Sampler, SendStart, Side,
    TrainingSettings, Turn, esm, loggable, normalize_call, parse_exchange, plan_turn,
    space_to_call,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use crate::playback::Preview;
use crate::player::{AudioError, Lease};
use crate::session_runtime::{Live, keep_and_show};
use crate::state::{AppCtx, Finished, Said, Screen};
use crate::time::{local_date_string, mono_ms, now_ms, seed_rng};

/// A gap this long between two of the run screen's ticks is time the app
/// was not running: a phone locked, a process put on hold. The ticks come
/// every quarter second, and a busy page delays one by far less.
const HEARTBEAT_GAP_MS: u64 = 2_000;

/// A run in progress: the machine that decides it, the settings its turns
/// are played with, and its hold on the audio.
pub struct LiveContest {
    machine: ContestMachine<Sampler>,
    /// The settings the run started with, fading off. Each caller brings a
    /// fade of its own, laid into its tones ([`plan_turn`]), so no send has
    /// fading laid on as it plays, your message least of all; and the band
    /// has no fading in it, so it plays on, never rebuilt.
    sound: TrainingSettings,
    lease: Lease,
    /// When the run screen's clock last ticked, once it has.
    last_tick: Option<MonoMs>,
    /// What the run has to say to a screen reader, held while anything is
    /// on the air so that speech never covers the Morse.
    unsaid: Vec<String>,
}

/// The run's Call and Exchange fields, as typed.
///
/// The fields keep what is typed into them, and the runtime reads it here
/// at each key ([`ContestEntry::snapshot`]). Nothing is drawn for a key: an
/// input drawn again while it is typed in drops the key typed in between, on
/// the desktop and phone builds, so the inputs are drawn only for what the
/// page has not seen — a letter the paddles keyed, the fields cleared — and
/// what reads the fields, the hint and Enter's label, follows `typed`.
#[derive(Clone, Copy, PartialEq)]
pub struct ContestEntry {
    /// Both fields, with when the call was last edited and when the
    /// exchange first read complete.
    fields: CopyValue<Fields>,
    /// Which field has the cursor. Only what says how to get back to the
    /// call reads it as it moves; the inputs never do.
    focus: Signal<EntryField>,
    /// What the fields are read by: the run's contest, from your side.
    reads: CopyValue<Option<(&'static ContestRules, Side)>>,
    /// Moved on by every edit. Only what reads the fields reads it.
    typed: Signal<u64>,
    /// Moved on only for what the inputs have not seen. Only they read it.
    repaint: Signal<u64>,
    /// Ends a paddle letter keyed in full, while the run screen is up.
    finish: CopyValue<Option<Callback>>,
}

impl ContestEntry {
    pub fn new() -> Self {
        Self {
            fields: CopyValue::new(Fields::default()),
            focus: Signal::new(EntryField::Call),
            reads: CopyValue::new(None),
            typed: Signal::new(0),
            repaint: Signal::new(0),
            finish: CopyValue::new(None),
        }
    }

    /// A new run, its fields read by `rules` from `side`: both empty, the
    /// cursor in Call.
    fn start(mut self, rules: &'static ContestRules, side: Side) {
        self.reads.set(Some((rules, side)));
        self.focus.set(EntryField::Call);
        self.clear();
    }

    /// What `field` holds, for its input to show. Reading it subscribes the
    /// caller to the rewrites the input has not seen, and to nothing else.
    pub fn shown(self, field: EntryField) -> String {
        let _ = self.repaint.read();
        self.text(field)
    }

    fn text(self, field: EntryField) -> String {
        let fields = self.fields.peek();
        match field {
            EntryField::Call => fields.call.clone(),
            EntryField::Exchange => fields.exch.clone(),
        }
    }

    /// A character the paddles or the keypad keyed into the field with the
    /// cursor at `keyed`. The page has not seen it, so that input is drawn
    /// again.
    pub fn push(mut self, letter: char, keyed: Option<MonoMs>) {
        let field = *self.focus.peek();
        let mut text = self.text(field);
        text.push(letter);
        self.edit(field, text, keyed);
        *self.repaint.write() += 1;
    }

    /// A space typed into the field with the cursor: it parts the
    /// exchange's items, and is no character of one.
    pub fn push_space(self) {
        self.push(' ', None);
    }

    /// The keypad's ⌫ took the last character out of the field with the
    /// cursor. The page has not seen it, so that input is drawn again.
    pub fn pop(mut self) {
        let field = *self.focus.peek();
        let mut text = self.text(field);
        if text.pop().is_some() {
            self.edit(field, text, None);
            *self.repaint.write() += 1;
        }
    }

    /// Both fields empty again, and their inputs drawn so.
    fn clear(mut self) {
        self.fields.set(Fields::cleared(MonoMs(mono_ms())));
        *self.typed.write() += 1;
        *self.repaint.write() += 1;
    }

    /// `field` now holds `text`, as its input does already, so nothing is
    /// drawn for it: what is new in it keyed at `keyed`, or untimed at
    /// `None`.
    pub fn edit(mut self, field: EntryField, text: String, keyed: Option<MonoMs>) {
        // Only a run has fields to type in, and it reads them from the
        // start.
        let Some((rules, side)) = *self.reads.peek() else {
            return;
        };
        self.fields
            .write()
            .edit(field, text, MonoMs(mono_ms()), keyed, rules, side);
        *self.typed.write() += 1;
    }

    /// Which field has the cursor: where a paddle letter goes.
    pub fn focus(self) -> EntryField {
        *self.focus.peek()
    }

    /// The same, for what follows the cursor from field to field: the hint
    /// and the key that say how to get back to the call. Reading it
    /// subscribes the caller to the cursor moving.
    pub fn focused(self) -> EntryField {
        *self.focus.read()
    }

    /// `field` has the cursor: a tap, a click, or the run moving it.
    pub fn set_focus(mut self, field: EntryField) {
        if *self.focus.peek() != field {
            self.focus.set(field);
        }
    }

    /// Whether Space in the Exchange field takes the cursor back to Call,
    /// as the exchange typed stands ([`space_to_call`]), rather than typing
    /// the space before its next item.
    pub fn space_to_call(self) -> bool {
        self.reads
            .peek()
            .is_some_and(|(rules, side)| space_to_call(&self.fields.peek().exch, rules, side))
    }

    /// The fields as they are now, for a key to carry.
    fn snapshot(self) -> Fields {
        self.fields.peek().clone()
    }

    /// What Enter does with the QSO at `qso` and the fields as typed, for
    /// the hint and Enter's label. Reading it subscribes the caller to every
    /// edit. Nothing before a run has started.
    pub fn enter(self, qso: &QsoState) -> Option<Esm> {
        let _ = self.typed.read();
        let (rules, side) = (*self.reads.peek())?;
        Some(esm(qso, &self.fields.peek(), rules, side))
    }

    /// The Exchange field as the run reads it, for "Reads as". Reading it
    /// subscribes the caller to every edit. Nothing before a run has
    /// started.
    pub fn reading(self) -> Option<Reading> {
        let _ = self.typed.read();
        let (rules, side) = (*self.reads.peek())?;
        Some(parse_exchange(rules, side, &self.fields.peek().exch))
    }

    /// Whether the fields hold a QSO a key can log without Enter, for the
    /// keys that do. Reading it subscribes the caller to every edit.
    /// Nothing before a run has started.
    pub fn loggable(self) -> bool {
        let _ = self.typed.read();
        self.reads
            .peek()
            .is_some_and(|(rules, side)| loggable(&self.fields.peek(), rules, side))
    }

    /// The Call field as the run reads it, for what is said of the call
    /// typed: "B4 07:31". Reading it subscribes the caller to every edit.
    pub fn call(self) -> String {
        let _ = self.typed.read();
        normalize_call(&self.fields.peek().call)
    }

    /// What the run copies from each caller, for the Exchange field's label.
    pub fn copies(self) -> &'static [Field] {
        self.reads
            .peek()
            .map_or(&[], |(rules, side)| rules.copied(side))
    }

    /// While the calling component is mounted, `finish` ends a paddle letter
    /// keyed in full, so a key pressed before its letter space has run out
    /// carries it.
    pub fn use_letters(self, mut finish: impl FnMut() + 'static) {
        let finish = use_callback(move |()| finish());
        let mut held = self.finish;
        use_hook(move || held.set(Some(finish)));
        use_drop(move || {
            // Only its own: a screen that has already taken over stays.
            if *held.peek() == Some(finish) {
                held.set(None);
            }
        });
    }

    /// End a paddle letter keyed in full now, in the field it was keyed for,
    /// and the paddles' word with it, so no word space after it presses
    /// Space: before a key, and before anything typed, pressed or tapped by
    /// hand, which would otherwise come before the letter, or have the word
    /// space move the cursor a second time. Whether a letter landed.
    pub fn finish_letter(self) -> bool {
        let finish = *self.finish.peek();
        let typed = *self.typed.peek();
        if let Some(finish) = finish {
            finish.call(());
        }
        *self.typed.peek() != typed
    }
}

/// Start a run of `settings`, clamped already: the training screen comes up
/// on its countdown, or on its first CQ. A run that cannot start says what
/// it still needs, and nothing changes.
pub fn start_run(ctx: AppCtx, settings: TrainingSettings) {
    if let Some(missing) = settings.contest.missing() {
        ctx.toasts.show(missing);
        return;
    }
    let rules = settings.contest.contest_kind.rules();
    let side = settings.contest.side();
    let AppCtx {
        mut screen,
        contest: mut view,
        mut announced,
        mut preview,
        sessions,
        toasts,
        entry,
        ..
    } = ctx;
    // The audio is claimed with what the run plays from, so every claim and
    // every turn of it has the one band.
    let mut sound = settings.clone();
    sound.band.qsb_enabled = false;
    // The run has the audio now, whether or not it can open it: any preview
    // that had it has stopped.
    preview.set(Preview::Idle);
    let app = ctx.app();
    let lease = match app.player.claim(&sound) {
        Ok(lease) => lease,
        Err(err) => {
            toasts.show(err.to_string());
            return;
        }
    };
    // The callers' calls lean on what the history says is hard, and on what
    // the run copies as it goes.
    let sampler = Sampler::new(&sessions.peek(), &settings, seed_rng());
    let now = MonoMs(mono_ms());
    let (machine, effects) = ContestMachine::start(seed_rng(), now, settings, sampler);
    entry.start(rules, side);
    announced.set(None);
    view.set(Some(machine.view()));
    screen.set(Screen::Training);
    *app.session.borrow_mut() = Some(Live::Contest(Box::new(LiveContest {
        machine,
        sound,
        lease: lease.clone(),
        last_tick: None,
        unsaid: Vec::new(),
    })));
    run_effects(ctx, &lease, effects);
}

/// Hand the run something the user did: End run, Discard, the page going
/// out of sight. Nothing happens when no run is live.
pub fn send_run_event(ctx: AppCtx, event: ContestEvent) {
    apply(ctx, Some(event));
}

/// A key or a message button: whatever paddle letter is keyed in full goes
/// into its field first, the player is woken inside the press, and the key
/// carries the fields as they are then.
pub fn press_key(ctx: AppCtx, key: OpKey) {
    ctx.entry.finish_letter();
    let Some(lease) = run_lease(ctx) else {
        return;
    };
    lease.wake();
    let fields = ctx.entry.snapshot();
    apply(
        ctx,
        Some(ContestEvent::Key {
            key,
            fields: Box::new(fields),
        }),
    );
}

/// − / +: your speed, moved by `delta` WPM from your next message on, and
/// kept as your speed for the next run. The player is woken inside the
/// press, as for a key. The setting is stored once the run screen is left,
/// as nothing is stored while a run is on.
pub fn change_speed(ctx: AppCtx, delta: i32) {
    let Some(lease) = run_lease(ctx) else {
        return;
    };
    lease.wake();
    apply(ctx, Some(ContestEvent::Speed { delta }));
    let moved = ctx.contest.peek().as_ref().map(|view| view.my_wpm);
    if let Some(wpm) = moved {
        // Whole already: − / + move it from the whole speed nearest it.
        let wpm = wpm.round() as u32;
        let mut settings = ctx.settings;
        if settings.peek().contest.my_wpm != wpm {
            settings.write().contest.my_wpm = wpm;
        }
    }
}

/// Pause the run, from now: you pressed Pause, or the page went out of
/// sight.
pub fn pause_run(ctx: AppCtx, why: PauseWhy) {
    let at = MonoMs(mono_ms());
    apply(ctx, Some(ContestEvent::Pause { why, at }));
}

/// Carry on after a pause. The audio is claimed afresh here, inside the tap
/// on Resume, so the player opens — or, after the sound failed, is built
/// anew — where a browser lets it start. A run that is not paused is left
/// alone: a claim would cut the turn on the air from under it.
pub fn resume_run(ctx: AppCtx) {
    let app = ctx.app();
    let paused = match app.session.borrow().as_ref() {
        Some(Live::Contest(live))
            if matches!(live.machine.phase(), ContestPhase::Paused { .. }) =>
        {
            Some(live.sound.clone())
        }
        _ => None,
    };
    let Some(sound) = paused else {
        return;
    };
    let lease = match app.player.claim(&sound) {
        Ok(lease) => lease,
        Err(err) => {
            ctx.toasts.show(err.to_string());
            return;
        }
    };
    if let Some(Live::Contest(live)) = app.session.borrow_mut().as_mut() {
        live.lease = lease;
    }
    apply(ctx, Some(ContestEvent::Resume));
    // The paused page is shorter, so a phone scrolled down to Pause is left
    // part way down it, the run bar and the fields above the screen's top:
    // the run carries on from its top, as every screen opens. Pausing
    // scrolls nothing, so nothing moves under the finger that paused.
    crate::app::scroll_to_top();
}

/// The run screen's clock ticked. A tick long after the one before is time
/// the app was not running, and the run is paused from that one first.
pub fn tick(ctx: AppCtx) {
    let now = MonoMs(mono_ms());
    let away = match ctx.app().session.borrow().as_ref() {
        Some(Live::Contest(live)) => live.away_since(now).is_some(),
        _ => return,
    };
    if away {
        apply(ctx, None);
    }
    if let Some(Live::Contest(live)) = ctx.app().session.borrow_mut().as_mut() {
        live.last_tick = Some(now);
    }
}

impl LiveContest {
    /// The tick the app was last seen running at, if it has not been seen
    /// since for longer than a tick can take.
    fn away_since(&self, now: MonoMs) -> Option<MonoMs> {
        self.last_tick
            .filter(|tick| now.since(*tick) > HEARTBEAT_GAP_MS)
    }
}

/// The run's hold on the audio, while a run is live.
fn run_lease(ctx: AppCtx) -> Option<Lease> {
    match ctx.app().session.borrow().as_ref() {
        Some(Live::Contest(live)) => Some(live.lease.clone()),
        _ => None,
    }
}

/// Hand the run news from its own tasks: a timer, a send. News from a lease
/// the run no longer holds — a run that has ended, a send a pause let go of
/// — is dropped.
fn dispatch(ctx: AppCtx, lease: &Lease, event: ContestEvent) {
    let current = matches!(
        ctx.app().session.borrow().as_ref(),
        Some(Live::Contest(live)) if live.lease.is(lease) && lease.is_current()
    );
    if current {
        apply(ctx, Some(event));
    }
}

/// Let the machine decide what `event` means, then do what it says.
///
/// Before anything is heard, a gap since the run screen last ticked longer
/// than any tick takes is credited as paused, from that tick: a process
/// frozen whole — a locked phone — hears nothing until it thaws, and then
/// its first news is old.
fn apply(ctx: AppCtx, event: Option<ContestEvent>) {
    let mut view = ctx.contest;
    let app = ctx.app();
    let (lease, effects) = {
        let mut slot = app.session.borrow_mut();
        let Some(Live::Contest(live)) = slot.as_mut() else {
            return;
        };
        let now = MonoMs(mono_ms());
        let mut effects = Vec::new();
        if let Some(tick) = live.away_since(now) {
            live.last_tick = Some(now);
            let away = ContestEvent::Pause {
                why: PauseWhy::Hidden,
                at: tick,
            };
            effects.extend(live.machine.apply(away, now));
        }
        if let Some(event) = event {
            effects.extend(live.machine.apply(event, now));
        }
        // Much of what arrives changes nothing — a stale send's news, a key
        // that sends nothing — and publishing it anyway would redraw the
        // screen for it.
        let shown = live.machine.view();
        if view.peek().as_ref() != Some(&shown) {
            view.set(Some(shown));
        }
        (live.lease.clone(), effects)
    };
    run_effects(ctx, &lease, effects);
}

fn run_effects(ctx: AppCtx, lease: &Lease, effects: Vec<ContestEffect>) {
    for effect in effects {
        match effect {
            ContestEffect::PlayTurn(turn) => play_turn(ctx, lease, turn),
            ContestEffect::StopAudio => lease.stop_sending(),
            ContestEffect::ReleaseAudio => {
                lease.release();
            }
            ContestEffect::StartTimer { id, ms } => {
                let lease = lease.clone();
                // One wait for the whole of it. A timer the machine has since
                // forgotten wakes once, is told so, and is gone; one a pause
                // let go of with the audio never wakes.
                spawn_forever(async move {
                    if lease.sleep(ms).await.is_ok() {
                        dispatch(ctx, &lease, ContestEvent::TimerFired { id });
                    }
                });
            }
            ContestEffect::ClearFields => ctx.entry.clear(),
            ContestEffect::Focus(field) => focus_field(ctx, field),
            ContestEffect::Announce(text) => announce(ctx, text),
            ContestEffect::PersistAndShowResults => keep_run(ctx),
            ContestEffect::ShowNothingKept => keep_nothing(ctx),
            ContestEffect::AbortToHome => abandon_run(ctx),
        }
    }
    speak_if_silent(ctx);
}

/// Hold `text` for a screen reader until the air is silent.
fn announce(ctx: AppCtx, text: String) {
    match ctx.app().session.borrow_mut().as_mut() {
        Some(Live::Contest(live)) => live.unsaid.push(text),
        _ => say(ctx, vec![text]),
    }
}

/// Say what the run holds for a screen reader, once nothing is on the air.
fn speak_if_silent(ctx: AppCtx) {
    let silent = ctx
        .contest
        .peek()
        .as_ref()
        .is_some_and(|view| view.air.is_none());
    if !silent {
        return;
    }
    let unsaid = match ctx.app().session.borrow_mut().as_mut() {
        Some(Live::Contest(live)) => std::mem::take(&mut live.unsaid),
        _ => return,
    };
    say(ctx, unsaid);
}

/// Say `unsaid`, all of it at once, in the order it came.
fn say(ctx: AppCtx, unsaid: Vec<String>) {
    if unsaid.is_empty() {
        return;
    }
    let mut announced = ctx.announced;
    announced.set(Some(Said::new(unsaid.join(". "))));
}

/// Play `turn`, and tell the machine when each attempt at it started, and
/// how it went.
///
/// It plays from the run's own settings, fading off ([`LiveContest::sound`]),
/// not from whatever the settings screen holds now.
fn play_turn(ctx: AppCtx, lease: &Lease, turn: Turn) {
    let sound = match ctx.app().session.borrow().as_ref() {
        Some(Live::Contest(live)) if live.lease.is(lease) => Some(live.sound.clone()),
        _ => None,
    };
    let Some(settings) = sound else {
        return;
    };
    let id = turn.id;
    let lease = lease.clone();
    let Some(transcript) = turn.transcript() else {
        // A turn with nothing in it is over as soon as it begins; the
        // machine never plays one.
        let ended = ContestEvent::TurnEnded { id, on_time: true };
        dispatch(ctx, &lease, ended);
        return;
    };
    let plan = plan_turn(&turn, &settings);
    // A continuation starts where the turn before gives way, which plays on
    // till then: no character of your message goes twice, and none waits.
    let starts_at = turn.starts_at;
    // Asked for now, as the turn goes on the air, not when its task first
    // runs: a stop handled before then ends it, and a turn before it still
    // waiting out a retry gives up rather than start over it.
    let asked = lease.ask_to_send();
    spawn_forever(async move {
        let heard = lease.clone();
        // Each attempt, retries too: when its sound goes to the output, and
        // when it reaches the ear, the player's lead, any wait for its
        // moment and latency after it starts.
        let started = move |start: &SendStart| {
            let event = ContestEvent::TurnStarted {
                id,
                heard_from: start.heard_at(),
                output_from: start.output_at,
            };
            dispatch(ctx, &heard, event);
        };
        let event = match lease
            .send_planned(asked, &plan, &transcript, &settings, starts_at, started)
            .await
        {
            Ok(sent) => ContestEvent::TurnEnded {
                id,
                on_time: sent.on_time,
            },
            // A stop, a newer turn, a pause: the machine has moved on.
            Err(AudioError::Cancelled) => ContestEvent::TurnCancelled { id },
            Err(failure) => {
                ctx.toasts.show(failure.to_string());
                ContestEvent::TurnFailed { id }
            }
        };
        dispatch(ctx, &lease, event);
    });
}

/// Move the cursor to `field`, on the page and for the paddles: to the end
/// of what it holds, with nothing selected, so the next key goes after it
/// and ⌫ takes its last character.
pub fn focus_field(ctx: AppCtx, field: EntryField) {
    ctx.entry.set_focus(field);
    let (id, _) = crate::ui::contest::field_name(field);
    let script = crate::ui::widgets::focus_end_script(&id);
    // Tests have no page to run it on: the harness looks for the field.
    #[cfg(test)]
    crate::testing::ask_focus(&id, &script);
    #[cfg(not(test))]
    spawn_forever(async move {
        let _ = document::eval(&script).await;
    });
}

/// While a run has the training screen, the cursor back in its field, as it
/// was: after a press of a button that is not the run's own, which a
/// keyboard may have moved the cursor to.
pub fn refocus_run(ctx: AppCtx) {
    if *ctx.screen.peek() == Screen::Training && ctx.contest.peek().is_some() {
        focus_field(ctx, ctx.entry.focus());
    }
}

/// The run, taken out of the app, with the audio given back. Its sound has
/// stopped, so what it held for a screen reader is said now.
fn end_run(ctx: AppCtx) -> Option<LiveContest> {
    let taken = ctx
        .app()
        .session
        .borrow_mut()
        .take_if(|live| matches!(live, Live::Contest(_)));
    let Some(Live::Contest(mut live)) = taken else {
        return None;
    };
    live.lease.release();
    say(ctx, std::mem::take(&mut live.unsaid));
    Some(*live)
}

/// Store the run as a session of the history, its log beside its groups,
/// and show it, as a session of groups is shown.
fn keep_run(ctx: AppCtx) {
    let Some(live) = end_run(ctx) else {
        return;
    };
    let AppCtx {
        contest: mut view,
        mut auto_message,
        ..
    } = ctx;
    view.set(None);
    // A run moves no level.
    auto_message.set(None);
    keep_and_show(ctx, live.machine.result(now_ms(), local_date_string()));
}

/// A run with no station worked has nothing for any stat: it is let go, and
/// the results screen says so, with another run a press away.
fn keep_nothing(ctx: AppCtx) {
    let AppCtx {
        mut screen,
        contest: mut view,
        mut result,
        mut auto_message,
        ..
    } = ctx;
    end_run(ctx);
    view.set(None);
    result.set(Some(Finished::NothingKept));
    auto_message.set(None);
    screen.set(Screen::Results);
}

/// End the run without keeping it.
fn abandon_run(ctx: AppCtx) {
    let AppCtx {
        mut screen,
        contest: mut view,
        ..
    } = ctx;
    end_run(ctx);
    view.set(None);
    screen.set(Screen::Home);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::{Behaviour, Call};
    use crate::persist::Store;
    use crate::persist::backup::{read_backup, write_backup};
    use crate::testing::{Harness, contest_settings, run};
    use crate::time::POLL_MS;
    use cw_core::band::BandKey;
    use cw_core::{
        CharSetMode, ContestView, MONITOR_LEVEL, parse_callsign, plan_morse_playback_for,
    };

    impl LiveContest {
        /// The run's hold on the audio, for a test to end or to stop.
        pub(crate) fn lease(&self) -> &Lease {
            &self.lease
        }
    }

    fn view(h: &Harness) -> Option<ContestView> {
        h.ctx.contest.peek().clone()
    }

    fn phase(h: &Harness) -> Option<ContestPhase> {
        view(h).map(|view| view.phase)
    }

    /// Whether the run is on and nothing is on the air: the moment to key.
    fn quiet(h: &Harness) -> bool {
        view(h).is_some_and(|view| view.phase == ContestPhase::Running && view.air.is_none())
    }

    /// Press `key` the way the run screen's keys and buttons do.
    fn press(h: &mut Harness, key: OpKey) {
        let ctx = h.ctx;
        h.in_app(|| press_key(ctx, key));
        h.pump();
    }

    fn type_into(h: &mut Harness, field: EntryField, text: &str) {
        let entry = h.ctx.entry;
        h.in_app(|| entry.edit(field, text.to_string(), Some(MonoMs(mono_ms()))));
    }

    /// What the callers sent in the last turn, as the player was told it.
    fn last_replies(h: &Harness) -> Vec<String> {
        h.recorder
            .starts()
            .last()
            .map(|start| {
                start
                    .transcript
                    .others
                    .iter()
                    .map(|other| other.text.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Copy what the last turn's callers sent into the fields, as an
    /// operator who hears every character would: a call, a call put right,
    /// or an exchange's serial.
    fn copy_what_was_heard(h: &mut Harness) {
        for text in last_replies(h) {
            let words: Vec<&str> = text.split_whitespace().collect();
            match words.as_slice() {
                ["DE", call] => type_into(h, EntryField::Call, call),
                [one, two] if one == two => type_into(h, EntryField::Call, one),
                [call] if parse_callsign(call).is_some() => type_into(h, EntryField::Call, call),
                _ if words.contains(&"5NN") => {
                    let serial = words.last().copied().unwrap_or_default();
                    type_into(h, EntryField::Exchange, serial);
                }
                _ => {}
            }
        }
    }

    /// Work the run as a perfect operator until it is over or `budget_ms`
    /// has passed: each time the air falls quiet, copy and press Enter. With
    /// `qsos`, stop once that many are logged instead.
    async fn work(h: &mut Harness, budget_ms: u64, qsos: Option<u32>) {
        let mut waited = 0;
        while waited < budget_ms && h.screen() == Screen::Training {
            if qsos.is_some_and(|qsos| view(h).is_some_and(|view| view.qsos >= qsos)) {
                return;
            }
            if quiet(h) {
                copy_what_was_heard(h);
                press(h, OpKey::Esm);
            }
            h.advance(u64::from(POLL_MS)).await;
            waited += u64::from(POLL_MS);
        }
    }

    /// A one-minute run worked to its end is stored, log and all, and shown
    /// on the results screen; it reads back as a run, from the store's
    /// parsing and from a backup alike.
    #[test]
    fn a_full_run_stores_its_log_and_shows_the_results() {
        run(|| async {
            let mut settings = contest_settings();
            settings.contest.contest_minutes = 1;
            let mut h = Harness::with_settings(settings.clone());
            h.start_training();
            work(&mut h, 90_000, None).await;
            assert_eq!(h.screen(), Screen::Results);
            assert!(h.ctx.contest.peek().is_none());
            assert!(h.app.session.borrow().is_none());
            assert!(
                h.calls().contains(&Call::Shutdown),
                "the audio was given back"
            );

            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            assert_eq!(result.char_set_mode, CharSetMode::Contest);
            assert_eq!(result.alphabet_fingerprint, "contest:wpx");
            let log = result.contest.as_ref().expect("its log");
            let stored = log.run(result.groups.len()).expect("the log reads back");
            let logged = stored.qsos.iter().filter(|qso| qso.k.is_empty()).count();
            assert!(logged >= 2, "{logged} QSOs logged");
            // The QSO open as time ran out, if one was, is a lost copy.
            assert!(stored.qsos.len() <= logged + 1, "{:?}", stored.qsos);
            assert_eq!(stored.run_ms, 60_000);
            // Every copy is right, but the number of that QSO, which was
            // never logged.
            let mut at = 0;
            for qso in &stored.qsos {
                let owned = &result.groups[at..at + qso.g as usize];
                at += qso.g as usize;
                let copied = if qso.k.is_empty() { owned } else { &owned[..1] };
                assert!(copied.iter().all(|group| group.correct), "{qso:?}");
            }
            assert_eq!(h.ctx.sessions.peek().len(), 1);
            assert_eq!(h.store.load_sessions(), vec![result.clone()]);
            assert_eq!(h.store.keep_asks.get(), 1);
            assert_eq!(*h.ctx.auto_message.peek(), None, "a run moves no level");

            let file = write_backup(&settings, &h.store.load_sessions(), 0);
            let back = read_backup(&file).expect("a backup").sessions;
            assert_eq!(back.len(), 1);
            assert_eq!(back[0].char_set_mode, CharSetMode::Contest);
            assert_eq!(back[0].contest, result.contest);
            assert_eq!(back[0].groups, result.groups);
        });
    }

    /// Ending a run with one QSO in the log ends it there, keeping it. The
    /// run screen's End run and the header's Exit both end it so.
    #[test]
    fn ending_with_one_qso_keeps_it_and_shows_the_results() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            assert_eq!(view(&h).map(|view| view.qsos), Some(1));
            let ctx = h.ctx;
            h.in_app(|| crate::session_runtime::finish_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Results);
            let result = h
                .ctx
                .result
                .peek()
                .clone()
                .and_then(Finished::kept)
                .expect("a result");
            let stored = result
                .contest
                .as_ref()
                .and_then(|log| log.run(result.groups.len()))
                .expect("its log");
            assert_eq!(stored.qsos.len(), 1);
            assert_eq!(result.groups.len(), 2, "the call and its number");
        });
    }

    /// The player is handed your message and every reply whole, with no
    /// fading laid on, and the band it plays over is the run's.
    #[test]
    fn the_recorder_hears_no_fading_your_message_last_and_no_reply_cut() {
        run(|| async {
            let settings = contest_settings();
            let mut h = Harness::with_settings(settings.clone());
            h.start_training();
            assert!(h.run_until(10_000, quiet).await);
            let starts = h.recorder.starts();
            let cq = &starts[0];
            assert!(
                cq.transcript.text.starts_with("CQ DL1ABC"),
                "{:?}",
                cq.transcript
            );
            assert_eq!(cq.qsb, None, "the fading was left on");
            assert_eq!(cq.transcript.others.len(), 1, "one caller answers");
            let ends = cq.plan.others.iter().map(|other| other.duration_sec);
            assert!(
                ends.into_iter()
                    .all(|end| end <= cq.plan.wanted.duration_sec),
                "a reply outlasts your message"
            );
            for (reply, played) in cq.transcript.others.iter().zip(&cq.plan.others) {
                let whole = plan_morse_playback_for(&reply.text, &settings, &reply.voice);
                assert_eq!(
                    played.events.len(),
                    whole.events.len(),
                    "{} was cut",
                    reply.text
                );
            }
            let band = BandKey::of(&settings.band);
            assert!(h.calls().iter().all(|call| match call {
                Call::Band(key) => *key == band,
                _ => true,
            }));
        });
    }

    /// Your message in a run goes around the receiver: every turn the player
    /// is handed is planned dry, your tones at your sending's level, the
    /// monitor's turned down. A session of groups never is: the station you
    /// copy is one the receiver hears.
    #[test]
    fn a_runs_turns_are_planned_dry_and_a_sessions_groups_never() {
        run(|| async {
            let settings = contest_settings();
            let level = settings.contest.sending_level();
            assert!(level > 0.0 && level < MONITOR_LEVEL);
            let mut h = Harness::with_settings(settings);
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            let starts = h.recorder.starts();
            assert!(starts.len() >= 3, "{} turns", starts.len());
            assert!(starts.iter().all(|start| start.plan.dry_wanted));
            let cq = &starts[0].plan.wanted.events;
            assert!(!cq.is_empty(), "your CQ was not played");
            assert!(cq.iter().all(|tone| tone.target_gain == level));

            let mut h = Harness::new();
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let starts = h.recorder.starts();
            assert!(!starts.is_empty(), "no group was sent");
            assert!(starts.iter().all(|start| !start.plan.dry_wanted));
        });
    }

    /// With your sending off, your messages are silent and nothing else
    /// changes: each turn the player is handed has no tones of yours and
    /// lasts as long, the air line shows your message going out, the heard
    /// timer brings the caller's call to Enter as ever, and a QSO is worked
    /// and logged clean.
    #[test]
    fn your_sending_off_is_silent_and_the_run_keeps_its_time() {
        run(|| async {
            let mut settings = contest_settings();
            settings.contest.my_sending_db = cw_core::MY_SENDING_OFF_DB;
            let mut loud = settings.clone();
            loud.contest.my_sending_db = 0;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.advance(300).await;
            let air = view(&h).and_then(|view| view.air).expect("your CQ");
            assert!(air.still_sending(MonoMs(mono_ms())).starts_with("CQ"));
            assert!(h.run_until(10_000, quiet).await);
            work(&mut h, 60_000, Some(1)).await;
            let starts = h.recorder.starts();
            assert!(starts.len() >= 3, "{} turns", starts.len());
            for start in &starts {
                assert!(start.plan.dry_wanted && start.plan.wanted.events.is_empty());
                // As long as it would be heard at the loudest.
                let turn_sec =
                    plan_morse_playback_for(&start.transcript.text, &loud, &start.transcript.voice)
                        .duration_sec;
                assert!(start.plan.wanted.duration_sec + 1e-9 >= turn_sec);
            }
            let clean = |h: &Harness| {
                let Some(row) = view(h).and_then(|view| view.log.first().cloned()) else {
                    return false;
                };
                matches!(&row.status, cw_core::RowStatus::Checked(check) if check.is_clean())
            };
            assert!(h.run_until(15_000, clean).await, "logged and checked clean");
        });
    }

    /// − and + move your speed, and it is kept: written to the settings as
    /// it moves, though nothing is stored mid-run, and the next run sends
    /// at it, storing it as it starts. Callers keep the Speed card's speeds.
    #[test]
    fn your_speed_is_kept_for_the_next_run() {
        run(|| async {
            let settings = contest_settings();
            assert_eq!(settings.contest.my_wpm, 0, "the callers' middle, 30");
            let mut h = Harness::with_settings(settings);
            h.start_training();
            let ctx = h.ctx;
            h.in_app(|| change_speed(ctx, 2));
            h.in_app(|| change_speed(ctx, 2));
            assert_eq!(view(&h).map(|view| view.my_wpm), Some(34.0));
            assert_eq!(h.ctx.settings.peek().contest.my_wpm, 34);
            assert_eq!(
                h.store.load_settings().contest.my_wpm,
                0,
                "not stored mid-run"
            );
            h.in_app(|| crate::session_runtime::abort_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            h.start_training();
            assert_eq!(h.store.load_settings().contest.my_wpm, 34);
            assert_eq!(view(&h).map(|view| view.my_wpm), Some(34.0));
            let cq = h
                .recorder
                .starts()
                .last()
                .expect("the next run's CQ")
                .clone();
            assert_eq!(cq.transcript.voice.char_wpm, 34.0);
            assert!(
                cq.transcript
                    .others
                    .iter()
                    .all(|caller| caller.voice.char_wpm == 30.0),
                "the callers keep the Speed card's"
            );
        });
    }

    /// Each attempt at a turn tells the run when its sound reaches the ear:
    /// the start, and the player's latency after it.
    #[test]
    fn a_turn_starting_tells_the_run_when_it_is_heard() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.recorder.latency_sec.set(0.3);
            h.start_training();
            let air = view(&h)
                .and_then(|view| view.air)
                .expect("the CQ on the air");
            assert_eq!(air.heard_from, MonoMs(300));
        });
    }

    /// A key while your message is still going out is spliced on where the
    /// message reaches the start of its next character: the player is asked
    /// to start the continuation at that moment, the rest of your message
    /// going on from that character and the one sounding at the key never
    /// sent again, and the run hears it from then.
    #[test]
    fn a_key_mid_message_is_spliced_on_where_the_message_gives_way() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            h.advance(300).await;
            let pressed = mono_ms();
            press(&mut h, OpKey::Again);
            let starts = h.recorder.starts();
            let (cq, next) = (&starts[0], starts.last().expect("the continuation"));
            assert!(
                cq.transcript.text.starts_with("CQ "),
                "{}",
                cq.transcript.text
            );
            // The C sounding at the key goes out whole in the CQ's send; the
            // rest goes on from the Q, the C's four elements before it.
            assert!(
                next.transcript.text.starts_with("Q DL1ABC"),
                "{}",
                next.transcript.text
            );
            let q_at = cq.plan.wanted.events[4].start_sec;
            let heard_in = next.heard_in.expect("a moment to start at");
            let meant = q_at - (pressed as f64 / 1_000.0);
            assert!((heard_in - meant).abs() < 0.002, "{heard_in} for {meant}");
            let air = view(&h).and_then(|view| view.air).expect("on the air");
            assert_eq!(air.heard_from, MonoMs(pressed).plus_sec(heard_in));
        });
    }

    /// Without full break-in, as it ships, every turn with a message of
    /// yours mutes the receiver from its start to your message's end, and a
    /// turn of callers alone none; with full break-in, none does.
    #[test]
    fn the_receiver_is_muted_while_you_send_unless_full_break_in() {
        run(|| async {
            for qsk in [false, true] {
                let mut settings = contest_settings();
                settings.contest.contest_qsk = qsk;
                let mut h = Harness::with_settings(settings.clone());
                h.start_training();
                work(&mut h, 30_000, Some(1)).await;
                let starts = h.recorder.starts();
                assert!(starts.len() >= 3, "{} turns", starts.len());
                for start in &starts {
                    let yours = &start.transcript;
                    let muted = (!qsk && !yours.text.is_empty()).then(|| {
                        plan_morse_playback_for(&yours.text, &settings, &yours.voice).duration_sec
                    });
                    assert_eq!(start.plan.muted_until, muted, "{qsk}: {}", yours.text);
                }
            }
        });
    }

    /// A send stopped or replaced is no failure: the run carries on. One that
    /// fails every attempt pauses it, and says so.
    #[test]
    fn a_cancel_never_pauses_and_a_failure_does() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            h.advance(500).await;
            // A key mid-turn replaces it; Stop cuts the one after.
            press(&mut h, OpKey::Cq);
            h.advance(300).await;
            press(&mut h, OpKey::StopTx);
            h.advance(2_000).await;
            assert_eq!(phase(&h), Some(ContestPhase::Running));
            assert!(h.toast().is_none());

            h.set_behaviour(Behaviour::FailMidSend);
            press(&mut h, OpKey::Cq);
            assert!(
                h.run_until(30_000, |h| phase(h)
                    == Some(ContestPhase::Paused {
                        why: PauseWhy::Sound
                    }))
                .await
            );
            assert!(h.toast().is_some(), "the failure is said");
            assert!(!h.lease().is_current(), "the audio was given back");
            assert_eq!(h.screen(), Screen::Training);
        });
    }

    /// A turn replaced while it waits out a retry gives up: it never starts
    /// over the turn that replaced it, which plays to its end, and the run
    /// falls quiet again.
    #[test]
    fn a_turn_waiting_out_a_retry_never_starts_over_a_newer_one() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            assert!(h.run_until(10_000, quiet).await);
            let before = h.texts().len();
            // The CQ's first attempt breaks, and Again is keyed before its
            // retry starts.
            h.recorder.fail_next(1);
            press(&mut h, OpKey::Cq);
            press(&mut h, OpKey::Again);
            assert!(
                h.run_until(30_000, quiet).await,
                "on the air: {:?}",
                view(&h).and_then(|view| view.air)
            );
            let sent = &h.texts()[before..];
            assert_eq!(sent.len(), 2, "{sent:?}");
            assert!(sent[1].ends_with('?'), "{sent:?}");
            assert!(h.toast().is_none(), "a replaced turn's failure was shown");
        });
    }

    /// Esc handled before a turn's send has started stops it all the same:
    /// what the run took back is never heard.
    #[test]
    fn esc_before_a_turn_starts_sounding_stops_it() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            assert!(h.run_until(10_000, quiet).await);
            let before = h.texts().len();
            let ctx = h.ctx;
            h.in_app(|| {
                press_key(ctx, OpKey::Cq);
                press_key(ctx, OpKey::StopTx);
            });
            h.pump();
            // Nothing of yours is on the air: at most a caller answering
            // what it heard of the CQ.
            let air = view(&h).and_then(|view| view.air);
            let yours = air.as_ref().map(|air| air.my_text.as_str());
            assert!(yours.is_none_or(str::is_empty), "{air:?}");
            h.advance(3_000).await;
            // Callers may call again; your CQ never goes out.
            let sent = &h.texts()[before..];
            assert!(sent.iter().all(String::is_empty), "{sent:?}");
        });
    }

    /// Resume after the sound failed opens a fresh player inside the tap,
    /// before the callers come back a second later.
    #[test]
    fn resume_after_a_sound_failure_opens_a_player_before_the_callers_return() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.set_behaviour(Behaviour::FailMidSend);
            h.start_training();
            assert!(
                h.run_until(30_000, |h| matches!(
                    phase(h),
                    Some(ContestPhase::Paused { .. })
                ))
                .await
            );
            h.set_behaviour(Behaviour::Play);
            let built = h.recorder.players_built.get();
            let sends = h.texts().len();
            let ctx = h.ctx;
            h.in_app(|| resume_run(ctx));
            h.pump();
            assert_eq!(h.recorder.players_built.get(), built + 1);
            assert!(h.lease().is_current());
            assert_eq!(phase(&h), Some(ContestPhase::Running));
            assert_eq!(
                h.texts().len(),
                sends,
                "nothing is sent before the callers return"
            );
            assert!(h.run_until(1_500, |h| h.texts().len() > sends).await);
            // Resume on a run that is not paused claims nothing.
            let lease = h.lease();
            h.in_app(|| resume_run(ctx));
            assert!(lease.is_current());
        });
    }

    /// A page hidden, and a gap in the run screen's ticks, are both left
    /// out of run time from when they began.
    #[test]
    fn a_hidden_page_and_a_gap_in_the_ticks_are_credited_as_paused() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let ctx = h.ctx;
            h.advance(2_000).await;
            h.in_app(|| pause_run(ctx, PauseWhy::Hidden));
            let paused_at = mono_ms();
            h.advance(5_000).await;
            let shown = view(&h).expect("a run");
            assert_eq!(
                shown.phase,
                ContestPhase::Paused {
                    why: PauseWhy::Hidden
                }
            );
            let clock = shown.clock.expect("running");
            assert_eq!(clock.paused_ms(MonoMs(mono_ms())), mono_ms() - paused_at);
            h.advance(5_000).await;
            assert!(
                matches!(phase(&h), Some(ContestPhase::Paused { .. })),
                "it resumed by itself"
            );

            let mut h = Harness::with_settings(contest_settings());
            let started = mono_ms();
            h.start_training();
            let ctx = h.ctx;
            h.advance(1_000).await;
            h.in_app(|| tick(ctx));
            let ticked = mono_ms();
            // Frozen: nothing ticks for five seconds.
            h.advance(5_000).await;
            h.in_app(|| tick(ctx));
            let shown = view(&h).expect("a run");
            assert_eq!(
                shown.phase,
                ContestPhase::Paused {
                    why: PauseWhy::Hidden
                }
            );
            let now = MonoMs(mono_ms());
            let clock = shown.clock.expect("running");
            assert_eq!(
                clock.peek_ms(now),
                ticked - started,
                "the gap counted as run time"
            );
            assert_eq!(clock.paused_ms(now), now.0 - ticked);
        });
    }

    /// Without a call Dust can read, the run does not start, and says what
    /// it needs.
    #[test]
    fn a_bad_own_call_refuses_to_start() {
        run(|| async {
            for (call, said) in [
                ("", "Add your callsign to start a contest"),
                ("ABC", "ABC is not a callsign Dust can read"),
            ] {
                let mut settings = contest_settings();
                settings.contest.my_call = call.to_string();
                let mut h = Harness::with_settings(settings);
                h.start_training();
                assert_eq!(h.screen(), Screen::Home);
                assert_eq!(h.toast().as_deref(), Some(said));
                assert!(h.app.session.borrow().is_none());
                assert!(h.ctx.contest.peek().is_none());
                assert!(h.calls().is_empty(), "the audio was claimed");
            }
        });
    }

    /// Discard leaves without keeping anything, and news from the run's old
    /// sends changes nothing after it.
    #[test]
    fn discard_keeps_nothing() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            let ctx = h.ctx;
            h.in_app(|| crate::session_runtime::abort_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.ctx.contest.peek().is_none());
            h.advance(5_000).await;
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.ctx.sessions.peek().is_empty());
        });
    }

    /// What changes nothing is not published: no copy of the view, and no
    /// redraw of the screen showing it.
    #[test]
    fn news_that_changes_nothing_is_not_published() {
        run(|| async {
            use std::sync::atomic::Ordering;

            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            assert!(h.run_until(10_000, quiet).await);
            let writes = h.count_writes(h.ctx.contest);
            let (ctx, lease) = (h.ctx, h.lease());
            h.in_app(|| {
                dispatch(ctx, &lease, ContestEvent::TimerFired { id: 9_999 });
                dispatch(ctx, &lease, ContestEvent::TurnCancelled { id: 1 });
            });
            h.pump();
            assert_eq!(
                writes.load(Ordering::Relaxed),
                0,
                "stale news was published"
            );
            press(&mut h, OpKey::Cq);
            assert!(writes.load(Ordering::Relaxed) > 0, "a CQ went unpublished");
        });
    }

    /// A run counts down before its first CQ, and a pause in the countdown
    /// gives the audio back and starts it again on Resume.
    #[test]
    fn a_run_counts_down_before_its_first_cq() {
        run(|| async {
            let mut settings = contest_settings();
            settings.playback.countdown_sec = 2;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert_eq!(phase(&h), Some(ContestPhase::Countdown { left: 2 }));
            assert!(h.texts().is_empty(), "sent during the countdown");
            let ctx = h.ctx;
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            assert!(!h.lease().is_current());
            h.advance(5_000).await;
            assert!(h.texts().is_empty());
            h.in_app(|| resume_run(ctx));
            assert_eq!(phase(&h), Some(ContestPhase::Countdown { left: 2 }));
            h.advance(2_100).await;
            assert_eq!(phase(&h), Some(ContestPhase::Running));
            assert!(h.texts()[0].starts_with("CQ DL1ABC"), "{:?}", h.texts());
        });
    }

    /// The fields keep what is typed, with when: the call's last edit, and
    /// the first moment the exchange read complete since it was empty. A
    /// paddle letter goes into the field with the cursor, and is drawn.
    #[test]
    fn the_fields_keep_what_is_typed_and_when() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let entry = h.ctx.entry;
            let repaints = |h: &Harness| *h.ctx.entry.repaint.peek();
            let drawn = repaints(&h);
            h.advance(100).await;
            type_into(&mut h, EntryField::Call, "K5XYZ");
            let typed_at = MonoMs(mono_ms());
            h.advance(100).await;
            type_into(&mut h, EntryField::Exchange, "21");
            let complete_at = MonoMs(mono_ms());
            h.advance(100).await;
            type_into(&mut h, EntryField::Exchange, "214");
            let fields = entry.snapshot();
            assert_eq!(
                (fields.call.as_str(), fields.exch.as_str()),
                ("K5XYZ", "214")
            );
            assert_eq!(fields.call_edited, typed_at);
            assert_eq!(
                fields.exch_complete,
                Some(complete_at),
                "stamped when first complete"
            );
            assert_eq!(repaints(&h), drawn, "typing drew the fields");

            h.in_app(|| entry.set_focus(EntryField::Exchange));
            h.in_app(|| entry.push('5', Some(MonoMs(mono_ms()))));
            assert_eq!(entry.snapshot().exch, "2145");
            assert_eq!(repaints(&h), drawn + 1, "a paddle letter is drawn");

            type_into(&mut h, EntryField::Exchange, "");
            assert_eq!(entry.snapshot().exch_complete, None);
            h.in_app(|| entry.clear());
            assert_eq!(entry.snapshot().call, "");
        });
    }

    /// A run that worked nobody keeps nothing, and its results screen says
    /// so in place of a result.
    #[test]
    fn a_run_with_nobody_worked_keeps_nothing() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let ctx = h.ctx;
            h.in_app(|| crate::session_runtime::finish_run(ctx));
            h.pump();
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(*h.ctx.result.peek(), Some(Finished::NothingKept));
            assert!(h.app.session.borrow().is_none());
            assert!(h.ctx.sessions.peek().is_empty());
            // The next run that keeps something shows its result instead.
            h.start_training();
            work(&mut h, 60_000, Some(1)).await;
            h.in_app(|| crate::session_runtime::finish_run(ctx));
            h.pump();
            assert!(
                h.ctx
                    .result
                    .peek()
                    .clone()
                    .and_then(Finished::kept)
                    .is_some()
            );
        });
    }

    /// What the run says as it ends, time's up last, is still said once
    /// the results are up: the region that says it is the app's, not the
    /// run screen's.
    #[test]
    fn times_up_is_said_on_the_results() {
        run(|| async {
            let mut settings = contest_settings();
            settings.contest.contest_minutes = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            work(&mut h, 90_000, None).await;
            assert_eq!(h.screen(), Screen::Results);
            let said = h.ctx.announced.peek().clone().map(|said| said.text);
            assert!(
                said.as_deref()
                    .is_some_and(|said| said.ends_with("Time's up")),
                "{said:?}"
            );
        });
    }

    /// What the run says for a screen reader waits for the air to fall
    /// silent, and comes whole: a check shown as the TU goes out is said
    /// once it has; Pause, which silences the air, is said at once.
    #[test]
    fn what_the_run_says_waits_for_the_air_to_fall_silent() {
        run(|| async {
            let mut h = Harness::with_settings(contest_settings());
            h.start_training();
            let said = |h: &Harness| {
                h.ctx
                    .announced
                    .peek()
                    .as_ref()
                    .map(|said| said.text.clone())
            };
            assert!(h.run_until(30_000, quiet).await);
            copy_what_was_heard(&mut h);
            press(&mut h, OpKey::Esm);
            assert!(h.run_until(30_000, quiet).await);
            copy_what_was_heard(&mut h);
            press(&mut h, OpKey::Esm);
            // Logged with the TU on the air: nothing is said over it.
            assert!(view(&h).is_some_and(|view| view.air.is_some()));
            assert_eq!(said(&h), None);
            assert!(h.run_until(30_000, quiet).await);
            let logged = said(&h).expect("said once the TU was out");
            assert!(logged.ends_with("logged, clean"), "{logged}");
            let ctx = h.ctx;
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            assert_eq!(said(&h).as_deref(), Some("Paused"));
            let first = h.ctx.announced.peek().clone().map(|said| said.serial);
            h.in_app(|| resume_run(ctx));
            h.in_app(|| pause_run(ctx, PauseWhy::You));
            let again = h.ctx.announced.peek().clone().map(|said| said.serial);
            assert_ne!(first, again, "the same words, said again");
        });
    }
}
