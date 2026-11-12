//! Seeded walks through the life of a session of groups and of a contest
//! run, driven through the calls the screens make: a start, taps and keys,
//! Pause and Resume, Back, the page going out of sight, End and Discard,
//! with time passing between them, held back now and then as a frozen
//! phone or a webview holds it, and the fake player failing, stalling,
//! running late or skipping. After every action a walk writes down what
//! the app shows, plays, says, marks and stores, as one line, and checks
//! what must always hold:
//!
//! - nothing is left borrowing what is live;
//! - what is live is what is published, and has the training screen;
//! - what is live holds the audio unless it is paused, and its stage says
//!   so: Running while it holds the audio, Paused while it shows paused;
//! - while it is paused, its mark holds what End would keep were it pressed
//!   then, so that the app stopping keeps that;
//! - a group, or a run's copy, that says why it holds no times — paused,
//!   held up, sent again — holds none, in the history and in the mark;
//! - the history only grows, and a session stored stays as it was stored.
//!
//! The walks held what the lifecycle did, line for line, while it was
//! made one for both kinds ([`crate::live_run`]); what they hold now is
//! what must always hold, whatever the lifecycle is made to do next. A
//! change meant to change nothing can still be checked line for line, its
//! build's walks against those of the build before:
//!
//! - `DUST_LIFECYCLE=<dir>` writes each walk's lines to `<dir>`.
//! - `DUST_LIFECYCLE_SEEDS=<n>` has the ignored walks take seeds 1 to `n` of
//!   each kind, by hand: `cargo test lifecycle_walk -- --ignored`.
//!
//! A group, stored or in a mark, is written as what was sent, what was
//! taken, and how it is timed: `t` where it holds its characters' times,
//! `p`, `h` or `s` where it says why it holds none (paused, held up, sent
//! again), and `-` where it does neither. After a session's groups comes a
//! hash of the whole of it as it is stored, `#` and 16 hex digits, all but
//! when it ended (`date`, `timestamp`), which the wall clock decides: its
//! score, each group's sends, times and characters' times, and a run's log.

use std::fmt::Write as _;
use std::time::Duration;

use cw_core::{
    ContestEffect, ContestEvent, ContestPhase, CountersKey, EntryField, MonoMs, OpKey, PauseWhy,
    Rng, RowStatus, RunStyle, SessionEffect, SessionEvent, SessionResult, SessionTiming,
    SplitMix64, TrainingSettings, Untimed, answer_length_matches,
};

use dioxus::prelude::*;

use crate::audio::fake::{Behaviour, Call};
use crate::contest_runtime::{change_speed, pause_run, press_key, resume_run, tick};
use crate::persist::Store;
use crate::session_runtime::{abort_run, finish_run, hold_run, send_event};
use crate::state::live::Stage;
use crate::state::{Finished, Live, Screen};
use crate::testing::{
    Harness, contest_settings, copy_what_was_heard, quiet, run, test_settings, type_into,
};
use crate::time::{POLL_MS, mono_ms};

/// How often the run screen's clock ticks, telling the run the app is still
/// running it, as the screen's own loop does.
const TICK_MS: u64 = 250;

/// The seeds the suite walks of each kind: 1 to this many, and the kind's
/// [`Kind::further`].
const SUITE_SEEDS: u64 = 100;

/// The suite's walks of each kind are split into this many tests, to run
/// side by side.
const PARTS: u64 = 5;

/// The ignored walks taken by hand are split into this many tests a kind.
const BY_HAND_PARTS: u64 = 4;

/// What a walk walks through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Groups,
    Contest,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Groups => "groups",
            Self::Contest => "contest",
        }
    }

    /// Seeds past the first [`SUITE_SEEDS`] that the suite walks too, as
    /// they reach what those never do, for the check that a copy saying
    /// why it is untimed holds no times ([`Walk::check_timing`]) to check:
    /// contest 1940 a pause cutting short the turn a copy was heard in, and
    /// 1582 a turn told of as held up only after the pause.
    fn further(self) -> &'static [u64] {
        match self {
            Self::Groups => &[],
            Self::Contest => &[1582, 1940],
        }
    }
}

/// One thing a walk does.
#[derive(Clone, Copy, Debug)]
enum Do {
    /// Let time pass, the run screen ticking as it goes.
    Advance,
    /// Hold every task back for a while, as a frozen phone does, and
    /// perhaps take a tap meanwhile, as a webview's own thread does.
    Freeze,
    /// The page goes out of sight: its sound parks, and its listener
    /// pauses what is live.
    Hide,
    /// The page comes back into sight.
    Show,
    /// The system's Back: what is live is held, and ended if it was held.
    Back,
    End,
    Discard,
    /// The fake player is made to fail, stall, run late or skip.
    Fault,
    /// Something outside stops the send playing.
    Stop,
    /// A session of groups: typing into the box, right, wrong or in part.
    Type(Typing),
    /// The box's pause runs out, and what it holds goes to the session.
    HandOver,
    Confirm,
    Pause,
    Hold,
    Resume,
    /// A run: listening until the air falls quiet, then copying what the
    /// last turn's callers sent.
    Copy,
    /// Copying what the last turn's callers sent at once, heard or not.
    CopyNow,
    /// Listening until the air falls quiet, copying what was heard, and
    /// pressing Enter: a run worked.
    Work,
    WrongCall,
    Key(OpKey),
    Speed(i32),
    PauseRun,
    ResumeRun,
}

#[derive(Clone, Copy, Debug)]
enum Typing {
    Right,
    Wrong,
    Part,
}

impl Do {
    /// Whether it takes what a run's screen offers only while the run is
    /// on: the fields, which a paused run's screen does not show, and
    /// − / +, which it disables, as it does the keyboard's speed keys.
    fn wants_a_run_on(self) -> bool {
        matches!(
            self,
            Self::Copy | Self::CopyNow | Self::Work | Self::WrongCall | Self::Speed(_)
        )
    }

    /// Whether it is a tap, which a webview takes even while every task
    /// is held back.
    fn is_tap(self) -> bool {
        !matches!(
            self,
            Self::Advance
                | Self::Freeze
                | Self::Hide
                | Self::Show
                | Self::Fault
                | Self::Stop
                | Self::Work
        )
    }
}

/// What a walk of a session of groups does, and how often.
const GROUPS: &[(u32, Do)] = &[
    (30, Do::Advance),
    (4, Do::Freeze),
    (2, Do::Hide),
    (2, Do::Show),
    (2, Do::Back),
    (2, Do::End),
    (1, Do::Discard),
    (5, Do::Fault),
    (1, Do::Stop),
    (10, Do::Type(Typing::Right)),
    (3, Do::Type(Typing::Wrong)),
    (4, Do::Type(Typing::Part)),
    (4, Do::HandOver),
    (5, Do::Confirm),
    (4, Do::Pause),
    (2, Do::Hold),
    (8, Do::Resume),
];

/// What a walk of a run does, and how often: mostly a run worked as an
/// operator works it, listening, copying and pressing Enter.
const RUNS: &[(u32, Do)] = &[
    (25, Do::Advance),
    (2, Do::Freeze),
    (1, Do::Hide),
    (2, Do::Show),
    (1, Do::Back),
    (2, Do::End),
    (1, Do::Discard),
    (3, Do::Fault),
    (1, Do::Stop),
    (14, Do::Work),
    (4, Do::Copy),
    (2, Do::CopyNow),
    (2, Do::WrongCall),
    (6, Do::Key(OpKey::Esm)),
    (1, Do::Key(OpKey::Cq)),
    (1, Do::Key(OpKey::HisCall)),
    (2, Do::Key(OpKey::TuAndLog)),
    (2, Do::Key(OpKey::Again)),
    (1, Do::Key(OpKey::StopTx)),
    (1, Do::Key(OpKey::Wipe)),
    (1, Do::Speed(2)),
    (1, Do::Speed(-2)),
    (2, Do::PauseRun),
    (6, Do::ResumeRun),
];

/// One of `table`, each as likely as its weight, those `keep` refuses left
/// out.
fn pick(rng: &mut SplitMix64, table: &[(u32, Do)], keep: impl Fn(Do) -> bool) -> Do {
    let kept: Vec<(u32, Do)> = table
        .iter()
        .copied()
        .filter(|(_, what)| keep(*what))
        .collect();
    let total: u32 = kept.iter().map(|(weight, _)| weight).sum();
    let mut at = (rng.f64() * f64::from(total)) as u32;
    for (weight, what) in &kept {
        if at < *weight {
            return *what;
        }
        at -= weight;
    }
    kept[kept.len() - 1].1
}

/// The settings a walk of `kind` runs on, drawn from `rng`.
fn settings(kind: Kind, rng: &mut SplitMix64) -> TrainingSettings {
    match kind {
        Kind::Groups => {
            let mut settings = test_settings();
            settings.playback.countdown_sec = if rng.f64() < 0.3 { 2 } else { 0 };
            settings.playback.group_timeout = [0.0, 1.0, 3.0][rng.usize_in(0, 2)];
            settings.curriculum.num_groups = rng.usize_in(1, 4) as u32;
            settings.playback.lock_input_during_group_playback = rng.f64() < 0.5;
            let sends = rng.usize_in(1, 2) as u32;
            settings.playback.group_repeat_min = sends;
            settings.playback.group_repeat_max = sends;
            settings
        }
        // No contest whose exchange greets: a greeting goes by the hour.
        Kind::Contest => {
            let mut settings = contest_settings();
            if rng.f64() < 0.5 {
                settings.set_contest_style(RunStyle::Pileup);
            }
            settings.playback.countdown_sec = if rng.f64() < 0.3 { 2 } else { 0 };
            settings.contest.contest_show_copy = rng.f64() < 0.25;
            settings.contest.contest_minutes = rng.usize_in(1, 2) as u32;
            settings
        }
    }
}

/// One walk: the app, what drives it, and the lines written so far.
struct Walk {
    h: Harness,
    kind: Kind,
    seed: u64,
    rng: SplitMix64,
    lines: String,
    /// How many of the fake player's calls the lines have told of.
    calls: usize,
    /// Each session stored so far, as it was stored.
    stored: Vec<String>,
    /// When the run screen next ticks, while it is up.
    next_tick: Option<u64>,
    /// The page is out of sight.
    hidden: bool,
}

impl Walk {
    fn live(&self) -> bool {
        self.h.app.session.borrow().is_some()
    }

    /// Whether the run screen shows a paused run: what it offers then, and
    /// what it does not ([`Do::wants_a_run_on`]).
    fn shows_a_paused_run(&self) -> bool {
        self.h
            .ctx
            .contest
            .peek()
            .as_ref()
            .is_some_and(|view| matches!(view.phase, ContestPhase::Paused { .. }))
    }

    /// The run screen comes up with a run, and its clock ticks from then;
    /// it goes with it.
    fn keep_time(&mut self) {
        let up = self.kind == Kind::Contest
            && self.h.screen() == Screen::Training
            && self.h.ctx.contest.peek().is_some();
        match (up, self.next_tick) {
            (true, None) => self.next_tick = Some(mono_ms() + TICK_MS),
            (false, Some(_)) => self.next_tick = None,
            _ => {}
        }
    }

    /// The run screen's clock ticks, if it is up.
    fn tick(&mut self) {
        self.keep_time();
        if self.next_tick.is_some() {
            let ctx = self.h.ctx;
            self.h.in_app(|| tick(ctx));
            self.next_tick = Some(mono_ms() + TICK_MS);
        }
    }

    /// Let `ms` pass, everything running, the run screen ticking.
    async fn advance(&mut self, ms: u64) {
        let until = mono_ms() + ms;
        loop {
            self.keep_time();
            let now = mono_ms();
            match self.next_tick {
                Some(at) if at <= until => {
                    if at > now {
                        self.h.advance(at - now).await;
                    }
                    self.tick();
                }
                _ => {
                    if until > now {
                        self.h.advance(until - now).await;
                    }
                    return;
                }
            }
        }
    }

    /// Let time pass until the run's air falls quiet, or for a while drawn
    /// up to four seconds, as a copy is keyed along or once it is heard:
    /// how long that took.
    async fn listen(&mut self) -> u64 {
        let from = mono_ms();
        let most = self.rng.usize_in(200, 4_000) as u64;
        while !quiet(&self.h) && mono_ms() - from < most && self.live() {
            self.advance(u64::from(POLL_MS)).await;
        }
        mono_ms() - from
    }

    /// The group the box types into, and what was sent of it.
    fn current_group(&self) -> Option<(usize, String)> {
        let runtime = self.h.ctx.runtime.peek();
        let session = runtime.as_ref()?;
        let index = session.view().current;
        let sent = session.group(index)?.sent().to_string();
        Some((index, sent))
    }

    /// Do `what`, a tap, as the screen does it, then let what it set off
    /// run, unless every task is held back (`pump` false).
    fn tap(&mut self, what: Do, pump: bool) -> String {
        let ctx = self.h.ctx;
        let did = match what {
            Do::Type(typing) => {
                let Some((index, sent)) = self.current_group() else {
                    return "type, with no group".into();
                };
                let text = match typing {
                    Typing::Right => sent.clone(),
                    Typing::Wrong => {
                        let mut wrong: String = sent.chars().skip(1).collect();
                        wrong.insert(0, if sent.starts_with('E') { 'T' } else { 'E' });
                        wrong
                    }
                    Typing::Part => sent.chars().take(1).collect(),
                };
                let now = MonoMs(mono_ms());
                self.h
                    .in_app(|| ctx.answer.hold(index, text.clone(), Some(now)));
                // A box typed to the group's length hands it over at once.
                if answer_length_matches(&sent, &text) {
                    self.h.in_app(|| hand_over(ctx));
                }
                format!("type {text} into {index}")
            }
            Do::HandOver => {
                self.h.in_app(|| hand_over(ctx));
                "hand over".into()
            }
            Do::Confirm => self.send(SessionEvent::Confirm),
            Do::Pause => self.send(SessionEvent::Pause),
            Do::Hold => self.send(SessionEvent::Hold),
            Do::Resume => self.send(SessionEvent::Resume),
            Do::Back => {
                let held = self.h.in_app(|| hold_run(ctx));
                if !held {
                    self.h.in_app(|| finish_run(ctx));
                }
                format!("back, {}", if held { "held" } else { "ended" })
            }
            Do::End => {
                self.h.in_app(|| finish_run(ctx));
                "end".into()
            }
            Do::Discard => {
                self.h.in_app(|| abort_run(ctx));
                "discard".into()
            }
            Do::Copy | Do::CopyNow => {
                copy_what_was_heard(&mut self.h);
                let entry = self.h.ctx.entry;
                format!(
                    "copy {} {}",
                    entry.shown(EntryField::Call),
                    entry.shown(EntryField::Exchange)
                )
            }
            Do::WrongCall => {
                type_into(&mut self.h, EntryField::Call, "W9XYZ");
                "copy W9XYZ".into()
            }
            Do::Key(key) => {
                self.h.in_app(|| press_key(ctx, key));
                format!("key {key:?}")
            }
            Do::Speed(delta) => {
                self.h.in_app(|| change_speed(ctx, delta));
                format!("speed {delta:+}")
            }
            Do::PauseRun => {
                self.h.in_app(|| pause_run(ctx, PauseWhy::You));
                "pause".into()
            }
            Do::ResumeRun => {
                self.h.in_app(|| resume_run(ctx));
                "resume".into()
            }
            Do::Advance | Do::Freeze | Do::Hide | Do::Show | Do::Fault | Do::Stop | Do::Work => {
                unreachable!("{what:?} is no tap")
            }
        };
        if pump {
            self.h.pump();
        }
        did
    }

    fn send(&mut self, event: SessionEvent) -> String {
        let ctx = self.h.ctx;
        let did = format!("{event:?}").to_lowercase();
        self.h.in_app(|| send_event(ctx, event));
        did
    }

    /// The fake player made to fail, stall, run late or skip, or set right.
    fn fault(&mut self) -> String {
        let recorder = &self.h.recorder;
        match self.rng.usize_in(0, 5) {
            0 => {
                let sends = self.rng.usize_in(1, 3) as u32;
                recorder.fail_next(sends);
                format!("fail the next {sends}")
            }
            1 | 2 => {
                let behaviour = [
                    Behaviour::Play,
                    Behaviour::Play,
                    Behaviour::Late,
                    Behaviour::Stall,
                    Behaviour::FailMidSend,
                ][self.rng.usize_in(0, 4)];
                recorder.set(behaviour);
                format!("play {behaviour:?}")
            }
            3 => {
                let skip = [(0.02, 0.06), (1.0, 60.0), (0.0, 0.0005)][self.rng.usize_in(0, 2)];
                recorder.skip_next.set(Some(skip));
                format!("skip {skip:?} of the next")
            }
            4 => {
                let refuse = !recorder.build_error.get();
                recorder.build_error.set(refuse);
                format!("refuse to open {refuse}")
            }
            _ => {
                recorder.set(Behaviour::Play);
                "play Play".into()
            }
        }
    }

    /// One step of the walk: what it did.
    async fn step(&mut self) -> String {
        self.keep_time();
        if !self.live() {
            return self.idle().await;
        }
        let table = match self.kind {
            Kind::Groups => GROUPS,
            Kind::Contest => RUNS,
        };
        let hidden = self.hidden;
        let paused = self.shows_a_paused_run();
        let what = pick(&mut self.rng, table, |what| match what {
            Do::Hide => !hidden,
            Do::Show => hidden,
            what => !(paused && what.wants_a_run_on()),
        });
        match what {
            Do::Advance => {
                let ms = self.rng.usize_in(50, 3_000) as u64;
                self.advance(ms).await;
                format!("advance {ms}")
            }
            Do::Freeze => {
                let ms = self.rng.usize_in(2_100, 60_000) as u64;
                tokio::time::advance(Duration::from_millis(ms)).await;
                tokio::task::yield_now().await;
                let tap = if self.rng.f64() < 0.5 {
                    // As the screen was when everything stopped.
                    let tap = pick(&mut self.rng, table, |what| {
                        what.is_tap() && !(paused && what.wants_a_run_on())
                    });
                    format!(", {} meanwhile", self.tap(tap, false))
                } else {
                    String::new()
                };
                self.h.pump();
                // The screen's tick came due in the gap.
                self.tick();
                format!("freeze {ms}{tap}")
            }
            Do::Hide => {
                self.h.set_page_hidden(true);
                self.hidden = true;
                self.advance(u64::from(POLL_MS)).await;
                let ctx = self.h.ctx;
                match self.kind {
                    Kind::Groups => self.h.in_app(|| send_event(ctx, SessionEvent::Pause)),
                    Kind::Contest => self.h.in_app(|| pause_run(ctx, PauseWhy::Hidden)),
                }
                self.h.pump();
                "hide".into()
            }
            Do::Show => {
                self.h.set_page_hidden(false);
                self.hidden = false;
                "show".into()
            }
            Do::Fault => self.fault(),
            Do::Copy => {
                let waited = self.listen().await;
                format!("listen {waited}, {}", self.tap(Do::Copy, true))
            }
            Do::Work => {
                let waited = self.listen().await;
                let copied = self.tap(Do::Copy, false);
                let keyed = self.tap(Do::Key(OpKey::Esm), true);
                format!("listen {waited}, {copied}, {keyed}")
            }
            Do::Stop => {
                self.h.lease().stop_sending();
                "stop from outside".into()
            }
            tap => self.tap(tap, true),
        }
    }

    /// Nothing is live: start again, mostly.
    async fn idle(&mut self) -> String {
        if self.h.recorder.build_error.get() && self.rng.f64() < 0.5 {
            self.h.recorder.build_error.set(false);
            return "refuse to open false".into();
        }
        if self.rng.f64() < 0.75 {
            self.h.start_training();
            return "start".into();
        }
        let ms = self.rng.usize_in(50, 1_000) as u64;
        self.advance(ms).await;
        format!("advance {ms}")
    }

    /// Check what must always hold after `did`, step `step`.
    fn check(&mut self, step: usize, did: &str) {
        let at = format!(
            "{} walk {}, step {step} ({did})",
            self.kind.name(),
            self.seed
        );
        assert!(
            self.h.app.session.try_borrow_mut().is_ok(),
            "{at}: what is live is still borrowed"
        );
        {
            let slot = self.h.app.session.borrow();
            let runtime = self.h.ctx.runtime.peek();
            let contest = self.h.ctx.contest.peek();
            match slot.as_ref() {
                None => assert!(
                    runtime.is_none() && contest.is_none(),
                    "{at}: published with nothing live"
                ),
                Some(Live::Groups(live)) => {
                    assert!(
                        runtime.as_ref() == Some(live.machine.session()) && contest.is_none(),
                        "{at}: the session published is not the session live"
                    );
                    assert!(
                        live.lease.is_current() || live.machine.session().paused(),
                        "{at}: a session not paused without the audio"
                    );
                }
                Some(Live::Contest(live)) => {
                    assert!(
                        contest.as_ref() == Some(&live.machine.view()) && runtime.is_none(),
                        "{at}: the run published is not the run live"
                    );
                    assert!(
                        live.lease.is_current()
                            || matches!(live.machine.phase(), ContestPhase::Paused { .. }),
                        "{at}: a run not paused without the audio"
                    );
                }
            }
            if let Some(live) = slot.as_ref() {
                let held = live.lease().is_current();
                let paused = runtime.as_ref().is_some_and(|session| session.paused())
                    || contest
                        .as_ref()
                        .is_some_and(|view| matches!(view.phase, ContestPhase::Paused { .. }));
                assert_eq!(
                    live.stage() == Stage::Running,
                    held,
                    "{at}: {:?} where the audio is held: {held}",
                    live.stage()
                );
                assert_eq!(
                    live.stage() == Stage::Paused,
                    paused,
                    "{at}: {:?} where it shows paused: {paused}",
                    live.stage()
                );
            }
            assert_eq!(
                self.h.screen() == Screen::Training,
                slot.is_some(),
                "{at}: the training screen and what is live disagree"
            );
        }
        self.check_mark(&at);
        self.check_timing(&at);
        let stored: Vec<String> = self
            .h
            .store
            .load_sessions()
            .iter()
            .map(|session| serde_json::to_string(session).expect("a session writes"))
            .collect();
        let mut left = stored.clone();
        for before in &self.stored {
            let Some(at_now) = left.iter().position(|now| now == before) else {
                panic!("{at}: a session stored before is gone or changed: {before}");
            };
            left.remove(at_now);
        }
        self.stored = stored;
    }

    /// A group, or a run's copy, that says why it holds no times holds
    /// none: the owner's rule for a turn a pause cut short or a send gone
    /// wrong, in the history, in the mark, and in what the mark holds for
    /// the store to take.
    fn check_timing(&self, at: &str) {
        let mut kept = self.h.store.load_sessions();
        if let Some(open) = self.h.store.open() {
            kept.extend(open.so_far.map(|so_far| so_far.result));
            kept.extend(open.unstored);
        }
        for result in &kept {
            for (group, timing) in result.groups.iter().zip(&result.group_timings) {
                assert!(
                    timing.untimed.is_none() || timing.char_times.is_none(),
                    "{at}: {} says why it is untimed, {:?}, and holds times",
                    group.sent,
                    timing.untimed
                );
            }
        }
    }

    /// While what is live is paused, its mark holds what End would keep
    /// were it pressed now, worked out on a copy of its machine: what the
    /// app stopping then keeps. Nothing to keep, nothing marked.
    fn check_mark(&self, at: &str) {
        let now = MonoMs(mono_ms());
        let end_now = match self.h.app.session.borrow().as_ref() {
            Some(Live::Groups(live)) if live.machine.session().paused() => {
                let mut ended = live.machine.clone();
                let effects = ended.apply(SessionEvent::FinishNow, now);
                Some(
                    effects
                        .contains(&SessionEffect::PersistAndShowResults)
                        .then(|| ended.session().result(0, String::new())),
                )
            }
            Some(Live::Contest(live))
                if matches!(live.machine.phase(), ContestPhase::Paused { .. }) =>
            {
                let mut ended = live.machine.clone();
                let effects = ended.apply(ContestEvent::FinishNow, now);
                Some(
                    effects
                        .contains(&ContestEffect::PersistAndShowResults)
                        .then(|| ended.result(0, String::new())),
                )
            }
            _ => None,
        };
        let Some(end_now) = end_now else {
            return;
        };
        let marked = self
            .h
            .store
            .open()
            .and_then(|open| open.so_far)
            .map(|so_far| so_far.result);
        assert!(
            marked.as_ref().map(as_kept) == end_now.as_ref().map(as_kept),
            "{at}: paused, the mark holds {} where End would keep {}",
            marked.as_ref().map_or("nothing".into(), session),
            end_now.as_ref().map_or("nothing".into(), session),
        );
    }

    /// Write down what the app shows, plays, says, marks and stores after
    /// step `step`, which did `did`.
    fn record(&mut self, step: usize, did: &str) {
        self.check(step, did);
        let mut line = format!("{step:03} {did} | {:?}", self.h.screen());
        let lease = self
            .h
            .app
            .session
            .borrow()
            .as_ref()
            .map(|live| live.lease().is_current());
        let lease = match lease {
            None => "-",
            Some(true) => "audio held",
            Some(false) => "audio given back",
        };
        let _ = write!(line, " {lease} |");
        if let Some(session) = self.h.ctx.runtime.peek().as_ref() {
            let view = session.view();
            let confirmed: String = view
                .confirmed
                .iter()
                .map(|confirmed| if *confirmed { '1' } else { '0' })
                .collect();
            let _ = write!(
                line,
                " {:?} at {} confirmed {confirmed} typed [{}] locked {} |",
                session.phase(),
                view.current,
                view.inputs.join(","),
                u8::from(view.locked)
            );
        }
        if let Some(view) = self.h.ctx.contest.peek().as_ref() {
            let now = MonoMs(mono_ms());
            let (run_ms, paused_ms) = view
                .clock
                .map_or((0, 0), |clock| (clock.peek_ms(now), clock.paused_ms(now)));
            let log: Vec<String> = view
                .log
                .iter()
                .map(|row| {
                    let fields: Vec<&str> =
                        row.fields.iter().map(|(_, text)| text.as_str()).collect();
                    let status = match &row.status {
                        RowStatus::Pending => "pending".to_string(),
                        RowStatus::Checked(check) => check.words(),
                    };
                    format!("{} {} {status}", row.call, fields.join(" "))
                })
                .collect();
            let air = view
                .air
                .as_ref()
                .map_or("-".to_string(), |air| format!("[{}]", air.my_text));
            let _ = write!(
                line,
                " {:?} run {run_ms} paused {paused_ms} qsos {} log [{}] air {air} wpm {} shown {} |",
                view.phase,
                view.qsos,
                log.join("; "),
                view.my_wpm,
                view.shown.len()
            );
        }
        let calls = self.h.calls();
        let played: Vec<String> = calls[self.calls..]
            .iter()
            .map(|call| match call {
                Call::New => "new".to_string(),
                Call::Band(_) => "band".to_string(),
                Call::Start(text) => format!("send[{text}]"),
                Call::Stop => "stop".to_string(),
                Call::Shutdown => "shut".to_string(),
                Call::LiveTone(on) => format!("tone {on}"),
            })
            .collect();
        self.calls = calls.len();
        let toast = self.h.toast().unwrap_or_default();
        let said = self
            .h
            .ctx
            .announced
            .peek()
            .as_ref()
            .map(|said| said.text.clone())
            .unwrap_or_default();
        let _ = write!(
            line,
            " plays [{}] toast [{toast}] said [{said}] |",
            played.join(" ")
        );
        match self.h.store.open() {
            None => line.push_str(" mark - |"),
            Some(open) => {
                let so_far = open.so_far.as_ref().map_or("-".to_string(), |so_far| {
                    format!(
                        "{} settings {}",
                        session(&so_far.result),
                        u8::from(so_far.settings.is_some())
                    )
                });
                let _ = write!(
                    line,
                    " mark {} {} so far {so_far} unstored {} |",
                    if open.run { "run" } else { "groups" },
                    if open.over { "over" } else { "open" },
                    open.unstored.len()
                );
            }
        }
        let stored: Vec<String> = self.h.store.load_sessions().iter().map(session).collect();
        let result = match self.h.ctx.result.peek().as_ref() {
            None => "-".to_string(),
            Some(Finished::NothingKept) => "nothing kept".to_string(),
            Some(Finished::Kept(result, _)) => format!("kept {}", result.groups.len()),
        };
        let level = self.h.ctx.auto_message.peek().clone().unwrap_or_default();
        let counters = self
            .h
            .store
            .load_auto_counters(&CountersKey::of(&self.h.ctx.settings.peek()));
        let _ = write!(
            line,
            " stored [{}] | result {result} level [{level}] counted {} {}",
            stored.join("; "),
            counters.above,
            counters.below
        );
        self.lines.push_str(&line);
        self.lines.push('\n');
    }
}

/// The box hands the session whatever it holds, as its pause running out
/// does.
fn hand_over(ctx: crate::state::AppCtx) {
    if let Some(typed) = ctx.answer.take() {
        send_event(ctx, typed.into());
    }
}

/// A session as a line tells of it: its accuracy, each group, and a hash
/// of the whole of it as it is stored, but for when it ended, which the
/// wall clock decides.
fn session(result: &SessionResult) -> String {
    let groups: Vec<String> = result
        .groups
        .iter()
        .zip(&result.group_timings)
        .map(|(group, timing)| format!("{}>{}:{}", group.sent, group.received, timed(timing)))
        .collect();
    format!(
        "{:.3} [{}] #{:016x}",
        result.accuracy,
        groups.join(" "),
        fnv1a(stored(result).to_string().as_bytes())
    )
}

/// `result` as it is stored, but for when it ended: its `date` and
/// `timestamp`.
fn stored(result: &SessionResult) -> serde_json::Value {
    let mut stored = serde_json::to_value(result).expect("a session writes");
    if let Some(stored) = stored.as_object_mut() {
        stored.remove("date");
        stored.remove("timestamp");
    }
    stored
}

/// `result` as End would keep it at any moment of a pause: as it is
/// stored, but for when it ended and, for a run, how long it has been
/// paused, which grows while it is.
fn as_kept(result: &SessionResult) -> serde_json::Value {
    let mut kept = stored(result);
    if let Some(log) = kept
        .get_mut("contest")
        .and_then(serde_json::Value::as_object_mut)
    {
        log.remove("pausedMs");
    }
    kept
}

/// How a group is timed: `t` with its characters' times, then why it holds
/// none, if it says (`p`aused, `h`eld up, `s`ent again), or `-` for neither.
fn timed(timing: &SessionTiming) -> String {
    let mut timed = String::new();
    if timing.char_times.is_some() {
        timed.push('t');
    }
    match timing.untimed {
        Some(Untimed::Paused) => timed.push('p'),
        Some(Untimed::HeldUp) => timed.push('h'),
        Some(Untimed::SentAgain) => timed.push('s'),
        None => {}
    }
    if timed.is_empty() {
        timed.push('-');
    }
    timed
}

/// Walk the walk of `kind` that `seed` draws: its lines.
async fn walk(kind: Kind, seed: u64) -> String {
    crate::time::pin_seeds(Some(seed));
    let mut rng = SplitMix64::new(seed).fork(kind as u64);
    let settings = settings(kind, &mut rng);
    let steps = rng.usize_in(80, 150);
    let mut walk = Walk {
        h: Harness::with_settings(settings),
        kind,
        seed,
        rng,
        lines: String::new(),
        calls: 0,
        stored: Vec::new(),
        next_tick: None,
        hidden: false,
    };
    for step in 0..steps {
        let did = walk.step().await;
        walk.record(step, &did);
    }
    walk.lines
}

/// The lines of the walk of `kind` that `seed` draws, on a test runtime of
/// its own.
fn walked(kind: Kind, seed: u64) -> String {
    let mut lines = String::new();
    let out = &mut lines;
    run(move || async move {
        *out = walk(kind, seed).await;
    });
    lines
}

/// FNV-1a, 64 bits.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Walk `seeds` of `kind`, every one of them checked after every step; with
/// `DUST_LIFECYCLE=<dir>`, write their lines there.
fn walks(kind: Kind, seeds: impl Iterator<Item = u64>) {
    let dir = std::env::var("DUST_LIFECYCLE").unwrap_or_default();
    for seed in seeds {
        let lines = walked(kind, seed);
        if !dir.is_empty() {
            let dir = std::path::Path::new(&dir);
            std::fs::create_dir_all(dir).expect("the walks' folder");
            let file = dir.join(format!("{}-{seed:04}.txt", kind.name()));
            std::fs::write(file, &lines).expect("a walk's lines are written");
        }
    }
}

/// Part `part` of the suite's walks of `kind`.
fn suite(kind: Kind, part: u64) {
    walks(
        kind,
        (1..=SUITE_SEEDS)
            .chain(kind.further().iter().copied())
            .filter(|seed| seed % PARTS == part),
    );
}

/// Part `part` of the walks of `kind` taken by hand, seeds 1 to
/// `DUST_LIFECYCLE_SEEDS`.
fn by_hand(kind: Kind, part: u64) {
    let seeds: u64 = std::env::var("DUST_LIFECYCLE_SEEDS")
        .ok()
        .and_then(|seeds| seeds.trim().parse().ok())
        .unwrap_or(0);
    walks(
        kind,
        (1..=seeds).filter(|seed| seed % BY_HAND_PARTS == part),
    );
}

#[test]
fn groups_walks_0() {
    suite(Kind::Groups, 0);
}

#[test]
fn groups_walks_1() {
    suite(Kind::Groups, 1);
}

#[test]
fn groups_walks_2() {
    suite(Kind::Groups, 2);
}

#[test]
fn groups_walks_3() {
    suite(Kind::Groups, 3);
}

#[test]
fn groups_walks_4() {
    suite(Kind::Groups, 4);
}

#[test]
fn contest_walks_0() {
    suite(Kind::Contest, 0);
}

#[test]
fn contest_walks_1() {
    suite(Kind::Contest, 1);
}

#[test]
fn contest_walks_2() {
    suite(Kind::Contest, 2);
}

#[test]
fn contest_walks_3() {
    suite(Kind::Contest, 3);
}

#[test]
fn contest_walks_4() {
    suite(Kind::Contest, 4);
}

#[test]
#[ignore = "by hand: DUST_LIFECYCLE_SEEDS=<n>"]
fn groups_walks_by_hand_0() {
    by_hand(Kind::Groups, 0);
}

#[test]
#[ignore = "by hand: DUST_LIFECYCLE_SEEDS=<n>"]
fn groups_walks_by_hand_1() {
    by_hand(Kind::Groups, 1);
}

#[test]
#[ignore = "by hand: DUST_LIFECYCLE_SEEDS=<n>"]
fn groups_walks_by_hand_2() {
    by_hand(Kind::Groups, 2);
}

#[test]
#[ignore = "by hand: DUST_LIFECYCLE_SEEDS=<n>"]
fn groups_walks_by_hand_3() {
    by_hand(Kind::Groups, 3);
}

#[test]
#[ignore = "by hand: DUST_LIFECYCLE_SEEDS=<n>"]
fn contest_walks_by_hand_0() {
    by_hand(Kind::Contest, 0);
}

#[test]
#[ignore = "by hand: DUST_LIFECYCLE_SEEDS=<n>"]
fn contest_walks_by_hand_1() {
    by_hand(Kind::Contest, 1);
}

#[test]
#[ignore = "by hand: DUST_LIFECYCLE_SEEDS=<n>"]
fn contest_walks_by_hand_2() {
    by_hand(Kind::Contest, 2);
}

#[test]
#[ignore = "by hand: DUST_LIFECYCLE_SEEDS=<n>"]
fn contest_walks_by_hand_3() {
    by_hand(Kind::Contest, 3);
}
