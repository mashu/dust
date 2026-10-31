//! The regression check for characters keyed along and left untimed.
//!
//! Each test runs a whole group session the way a learner does, with Lock
//! typing while sending off: Start on the Practice screen, each character
//! keyed a moment after its sound reached the ear, in the answer box or on
//! the keypad, through the screen's own handlers, then Enter, and on to the
//! Results screen. It then reads what the learner reads: the count of
//! characters timed on Results, no "not timed" line there, and the session
//! as the history keeps it, every character timed from its own sound
//! (docs/recognition.md).
//!
//! A session keyed along that comes out untimed, by a change anywhere from
//! the screen's key handler through the session, its results and its
//! storage, fails here.

use std::rc::Rc;

use cw_core::{SessionPhase, SessionResult, TrainingSettings};
use dioxus::prelude::ReadableExt;

use crate::audio::fake::Recorder;
use crate::persist::Store;
use crate::testing::{Ui, run, test_settings};
use crate::time::{POLL_MS, mono_ms};

/// How a learner keys.
#[derive(Clone, Copy, Debug)]
enum By {
    /// In the answer box, on a keyboard, then Enter.
    Keyboard,
    /// On the screen's keypad, then Next, as on a phone or tablet.
    Keypad,
}

/// A learner's session: `groups` groups of `size` characters at `wpm`, the
/// lock off, as a learner who keys along has it.
fn along(groups: u32, size: u32, wpm: f64) -> TrainingSettings {
    let mut settings = test_settings();
    settings.curriculum.num_groups = groups;
    settings.curriculum.min_group_size = size;
    settings.curriculum.max_group_size = size;
    settings.playback.char_wpm_min = wpm;
    settings.playback.char_wpm_max = wpm;
    settings.playback.effective_wpm_min = wpm;
    settings.playback.effective_wpm_max = wpm;
    settings.playback.lock_input_during_group_playback = false;
    settings
}

/// Where the session is.
fn phase(ui: &mut Ui) -> Option<SessionPhase> {
    ui.with_app(|ctx| ctx.runtime.peek().as_ref().map(|session| session.phase()))
}

/// What the session sent for group `index`.
fn sent(ui: &mut Ui, index: usize) -> String {
    ui.with_app(|ctx| {
        ctx.runtime
            .peek()
            .as_ref()
            .and_then(|session| session.group(index).map(|group| group.sent().to_string()))
            .unwrap_or_default()
    })
}

/// Run the clock on to `at`, on the app's clock.
async fn until(ui: &mut Ui, at: u64) {
    let now = mono_ms();
    if at > now {
        ui.advance(at - now).await;
    }
}

/// Key `letter` for group `index`, `typed` being the box's text with it.
fn key(ui: &mut Ui, by: By, index: usize, letter: char, typed: &str) {
    match by {
        By::Keyboard => ui.type_into(&format!("group-input-{index}"), typed),
        By::Keypad => ui.tap(&format!("key-{}", letter.to_ascii_lowercase())),
    }
}

/// Close group `index`'s answer, as the learner does once it is keyed.
fn confirm(ui: &mut Ui, by: By, index: usize) {
    match by {
        By::Keyboard => ui.press_enter(&format!("group-input-{index}")),
        By::Keypad => ui.tap("key-next"),
    }
}

/// Run a session from the Practice screen's Start to its Results, keying
/// every group along with its first send `react_ms` after each character
/// reached the ear, then closing it once the box is open. Returns how many
/// characters were keyed.
async fn key_along(ui: &mut Ui, recorder: &Recorder, by: By, react_ms: u64) -> usize {
    ui.click("btn-start-training");
    assert_eq!(ui.screen(), "training", "{}", ui.html());
    let mut keyed_groups = 0;
    let mut keyed_chars = 0;
    let mut sends_seen = 0;
    let deadline = mono_ms() + 120_000;
    while ui.screen() != "results" {
        assert!(mono_ms() < deadline, "no results: {}", ui.html());
        let starts = recorder.starts();
        let Some(send) = starts.get(sends_seen).cloned() else {
            ui.advance(u64::from(POLL_MS)).await;
            continue;
        };
        sends_seen += 1;
        let Some(SessionPhase::Playing { index }) = phase(ui) else {
            continue;
        };
        if index < keyed_groups {
            // A repeat of a group already keyed.
            continue;
        }
        keyed_groups = index + 1;
        let text = sent(ui, index);
        let heard = send.at.0 + (recorder.latency_sec.get() * 1_000.0).round() as u64;
        let ends = &send.plan.wanted.chars;
        assert_eq!(ends.len(), text.chars().count(), "{text}: {ends:?}");
        let mut typed = String::new();
        for (letter, span) in text.chars().zip(ends) {
            until(
                ui,
                heard + (span.end_sec * 1_000.0).round() as u64 + react_ms,
            )
            .await;
            typed.push(letter);
            key(ui, by, index, letter, &typed);
            keyed_chars += 1;
        }
        // Closed once its sends are over and the box waits for its answer.
        while matches!(
            phase(ui),
            Some(SessionPhase::Playing { index: i } | SessionPhase::RepeatGap { index: i })
                if i == index
        ) {
            assert!(mono_ms() < deadline, "still sending: {}", ui.html());
            ui.advance(u64::from(POLL_MS)).await;
        }
        if matches!(phase(ui), Some(SessionPhase::AwaitingAnswer { index: open }) if open == index)
        {
            confirm(ui, by, index);
        }
    }
    keyed_chars
}

/// The session the history kept last.
fn kept(ui: &Ui) -> SessionResult {
    ui.store
        .load_sessions()
        .last()
        .cloned()
        .expect("the session is kept")
}

/// Every character keyed is timed: on Results, and in the history.
fn every_character_timed(ui: &Ui, keyed: usize) {
    let html = ui.html();
    assert!(keyed > 0);
    assert!(
        html.contains(&format!("{keyed} timed")),
        "{keyed} keyed, not all timed: {html}"
    );
    assert!(!ui.shows("untimed-hint"), "{html}");
    let session = kept(ui);
    let untimed = cw_core::session_untimed(&session);
    assert_eq!(
        (untimed.chars, untimed.timed, untimed.held_up),
        (keyed, keyed, 0),
        "{:?}",
        session.group_timings
    );
    assert!(!session.typing_locked);
    for (group, timing) in session.groups.iter().zip(&session.group_timings) {
        let times = timing
            .char_times
            .as_ref()
            .unwrap_or_else(|| panic!("{} kept no times: {timing:?}", group.sent));
        let took = cw_core::recognise(&group.sent, &group.received, times);
        assert!(
            took.iter().all(Option::is_some),
            "{} keyed as {}: {took:?} from {times:?}",
            group.sent,
            group.received
        );
    }
}

/// Groups of three at 20 WPM, each character keyed a quarter of a second
/// after it is heard: every one timed.
#[test]
fn keyed_along_on_a_keyboard_every_character_is_timed() {
    run(|| async {
        let (mut ui, recorder) = Ui::app_with_settings(along(4, 3, 20.0));
        let keyed = key_along(&mut ui, &recorder, By::Keyboard, 250).await;
        assert_eq!(keyed, 12);
        every_character_timed(&ui, keyed);
    });
}

/// The same on the keypad, as on a phone or tablet.
#[test]
fn keyed_along_on_the_keypad_every_character_is_timed() {
    run(|| async {
        let (mut ui, recorder) = Ui::app_with_settings(along(4, 3, 20.0));
        let keyed = key_along(&mut ui, &recorder, By::Keypad, 250).await;
        assert_eq!(keyed, 12);
        every_character_timed(&ui, keyed);
    });
}

/// Short groups sent fast, keyed as soon as a learner who keeps up does:
/// each first key after its group has ended, within the along floor.
#[test]
fn short_fast_groups_keyed_along_are_timed() {
    run(|| async {
        let (mut ui, recorder) = Ui::app_with_settings(along(5, 2, 30.0));
        let keyed = key_along(&mut ui, &recorder, By::Keyboard, 300).await;
        assert_eq!(keyed, 10);
        every_character_timed(&ui, keyed);
    });
}

/// Longer groups, keyed well behind each character but keeping up.
#[test]
fn longer_groups_keyed_half_a_second_behind_are_timed() {
    run(|| async {
        let (mut ui, recorder) = Ui::app_with_settings(along(3, 5, 15.0));
        let keyed = key_along(&mut ui, &recorder, By::Keyboard, 500).await;
        assert_eq!(keyed, 15);
        every_character_timed(&ui, keyed);
    });
}

/// An output that says its sound reaches the ear later: keyed along with
/// what is heard, every character is timed, from when it was heard.
#[test]
fn keyed_along_through_an_output_with_latency_every_character_is_timed() {
    run(|| async {
        let (mut ui, recorder) = Ui::app_with_settings(along(4, 3, 20.0));
        recorder.latency_sec.set(0.15);
        let keyed = key_along(&mut ui, &recorder, By::Keyboard, 250).await;
        every_character_timed(&ui, keyed);
    });
}

/// Each group sent twice (Sends per group 2×), keyed along the first send:
/// every character is timed from it.
#[test]
fn keyed_along_the_first_of_two_sends_every_character_is_timed() {
    run(|| async {
        let mut settings = along(3, 3, 20.0);
        settings.playback.group_repeat_min = 2;
        settings.playback.group_repeat_max = 2;
        let (mut ui, recorder) = Ui::app_with_settings(settings);
        let keyed = key_along(&mut ui, &recorder, By::Keyboard, 250).await;
        assert_eq!(keyed, 9);
        every_character_timed(&ui, keyed);
    });
}

/// The session the lock-ask card turns the lock off for: an older
/// install's settings with it on, answered "Type along" on Home. The very
/// next session keyed along is timed.
#[test]
fn turned_off_from_home_the_next_session_keyed_along_is_timed() {
    run(|| async {
        let mut settings = along(3, 3, 20.0);
        settings.playback.lock_input_during_group_playback = true;
        let store = Rc::new(crate::persist::memory::MemoryStore::default());
        store.save_settings(&settings);
        store.mark_tutorial_done();
        let (mut ui, recorder) = Ui::launch(store);
        assert!(ui.shows("lock-ask"), "{}", ui.html());
        ui.click("btn-type-along");
        assert!(!ui.with_app(|ctx| {
            ctx.settings
                .peek()
                .playback
                .lock_input_during_group_playback
        }));
        let keyed = key_along(&mut ui, &recorder, By::Keyboard, 250).await;
        every_character_timed(&ui, keyed);
    });
}

/// A browser's stream that skips a sliver of every send, as one whose audio
/// clock comes in bursts does: no character of any send is lost, and what
/// follows a skip plays on its planned time, so every character keyed
/// along is timed.
#[test]
fn a_sliver_skipped_from_every_send_leaves_every_character_timed() {
    run(|| async {
        let (mut ui, recorder) = Ui::app_with_settings(along(4, 3, 20.0));
        recorder.skip_every.set(Some((0.0, 0.0005)));
        let keyed = key_along(&mut ui, &recorder, By::Keyboard, 250).await;
        assert_eq!(keyed, 12);
        assert_eq!(recorder.texts().len(), 4, "nothing sent again");
        every_character_timed(&ui, keyed);
    });
}
