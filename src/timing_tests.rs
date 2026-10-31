//! The regression check for characters keyed along, or copied after their
//! group, and left untimed.
//!
//! Each test runs a whole group session the way a learner does: Start on
//! the Practice screen, each character keyed in the answer box or on the
//! keypad, through the screen's own handlers, then Enter, and on to the
//! Results screen. Keyed along, each a moment after its sound reached the
//! ear, every character has a recognition time; copied once its group has
//! ended, the usual way to copy, every letter has a copy time, the pause
//! before its key. The tests read what the learner reads: the figure on
//! Results, no "not timed" line there, the session as the history keeps
//! it, and Letters (docs/recognition.md).
//!
//! A session keyed either way that comes out untimed, by a change anywhere
//! from the screen's key handler through the session, its results and its
//! storage, fails here.

use std::rc::Rc;

use cw_core::{SessionPhase, SessionResult, TrainingSettings};
use dioxus::prelude::ReadableExt;

use crate::audio::fake::Recorder;
use crate::persist::Store;
use crate::testing::{Ui, kpi, run, test_settings};
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

/// Every character keyed along is recognised: on Results, and in the
/// history; none of them copy-timed.
fn every_character_timed(ui: &Ui, keyed: usize) {
    let html = ui.html();
    assert!(keyed > 0);
    let recognised = kpi(&html, "kpi-recognised");
    assert!(
        recognised.contains(&format!(">{keyed} timed<")),
        "{keyed} keyed, not all timed: {recognised} in {html}"
    );
    assert!(!ui.shows("kpi-copy"), "copy-timed: {html}");
    assert!(!ui.shows("untimed-hint"), "{html}");
    let session = kept(ui);
    let untimed = cw_core::session_untimed(&session);
    assert_eq!(
        (
            untimed.chars,
            untimed.timed,
            untimed.copied,
            untimed.held_up
        ),
        (keyed, keyed, 0, 0),
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
        assert!(
            cw_core::copy_times(&group.sent, &group.received, times)
                .iter()
                .all(Option::is_none),
            "{} keyed along, yet copy-timed: {times:?}",
            group.sent
        );
    }
}

/// A group copied after it ended: when its end reached the ear, and when
/// each of its letters was keyed, on the app's clock.
#[derive(Debug)]
struct Copied {
    heard_end: u64,
    keys: Vec<u64>,
}

/// Run a session from the Practice screen's Start to its Results, copying
/// every group once it has ended, as most learners copy: its first letter
/// `first_ms` after its end reached the ear, each later one `gaps_ms` after
/// the one before, in turn, then closing it. What was keyed, group by
/// group.
async fn copy_after(
    ui: &mut Ui,
    recorder: &Recorder,
    by: By,
    first_ms: u64,
    gaps_ms: &[u64],
) -> Vec<Copied> {
    ui.click("btn-start-training");
    assert_eq!(ui.screen(), "training", "{}", ui.html());
    let mut copied: Vec<Copied> = Vec::new();
    let mut sends_seen = 0;
    let mut gap = gaps_ms.iter().cycle();
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
        if index < copied.len() {
            continue;
        }
        let text = sent(ui, index);
        let last_end = send
            .plan
            .wanted
            .chars
            .last()
            .map_or(0, |span| (span.end_sec * 1_000.0).round() as u64);
        let heard_end =
            send.at.0 + (recorder.latency_sec.get() * 1_000.0).round() as u64 + last_end;
        // Its sound over and the box waiting for its answer.
        while !matches!(phase(ui), Some(SessionPhase::AwaitingAnswer { index: open }) if open == index)
        {
            assert!(mono_ms() < deadline, "never answered: {}", ui.html());
            ui.advance(u64::from(POLL_MS)).await;
        }
        until(ui, heard_end + first_ms).await;
        let mut keys = Vec::new();
        let mut typed = String::new();
        for letter in text.chars() {
            if let Some(&before) = keys.last() {
                until(ui, before + gap.next().copied().unwrap_or(200)).await;
            }
            keys.push(mono_ms());
            typed.push(letter);
            key(ui, by, index, letter, &typed);
        }
        copied.push(Copied { heard_end, keys });
        if matches!(phase(ui), Some(SessionPhase::AwaitingAnswer { index: open }) if open == index)
        {
            confirm(ui, by, index);
        }
    }
    copied
}

/// Every letter copied after its group is copy-timed: on Results, apart
/// from any recognition time, nothing called untimed, and in the history,
/// each pause what it was keyed with: the first from the group's end as
/// heard, each later one from the key before.
fn every_letter_copy_timed(ui: &Ui, copied: &[Copied], locked: bool) {
    let html = ui.html();
    let n: usize = copied.iter().map(|group| group.keys.len()).sum();
    assert!(n > 0);
    let session = kept(ui);
    let copy = cw_core::session_copy(&session).expect("a copy time");
    assert_eq!(copy.timed, n, "{:?}", session.group_timings);
    let tile = kpi(&html, "kpi-copy");
    assert!(tile.contains(&format!(">{n} timed<")), "{tile}");
    assert!(
        tile.contains(&format!(">{:.0} ms<", copy.median_ms)),
        "{tile}"
    );
    assert!(!ui.shows("kpi-recognised"), "{html}");
    assert!(!ui.shows("untimed-hint"), "{html}");
    assert!(!html.contains("Nothing was timed"), "{html}");
    let untimed = cw_core::session_untimed(&session);
    assert_eq!(
        (
            untimed.chars,
            untimed.timed,
            untimed.copied,
            untimed.held_up
        ),
        (n, 0, n, 0),
        "{:?}",
        session.group_timings
    );
    assert_eq!(session.typing_locked, locked);
    assert_eq!(session.groups.len(), copied.len());
    for ((group, timing), keyed) in session
        .groups
        .iter()
        .zip(&session.group_timings)
        .zip(copied)
    {
        let times = timing
            .char_times
            .as_ref()
            .unwrap_or_else(|| panic!("{} kept no keys: {timing:?}", group.sent));
        assert_eq!(timing.untimed, None, "{timing:?}");
        assert_eq!(times.box_open, !locked, "{times:?}");
        assert!(cw_core::copied_after(times), "{times:?}");
        assert!(
            cw_core::recognise(&group.sent, &group.received, times)
                .iter()
                .all(Option::is_none),
            "{} copied, yet recognised: {times:?}",
            group.sent
        );
        let took = cw_core::copy_times(&group.sent, &group.received, times);
        let first = took[0].expect("its first letter copy-timed");
        let want = keyed.keys[0] - keyed.heard_end;
        assert!(
            u64::from(first).abs_diff(want) <= 5,
            "first {first} ms, keyed {want} ms after: {times:?}"
        );
        for (at, pause) in took.iter().enumerate().skip(1) {
            assert_eq!(
                *pause,
                Some(u32::try_from(keyed.keys[at] - keyed.keys[at - 1]).expect("a pause")),
                "{}: {took:?}",
                group.sent
            );
        }
    }
}

/// The pauses between the letters a learner copies: as they come, uneven.
const GAPS: [u64; 4] = [180, 260, 150, 300];

/// Groups of three at 20 WPM, each copied half a second after it ended,
/// the usual way to copy: every letter has a copy time, on Results and in
/// the history, and Letters shows each one's, none "not timed yet".
#[test]
fn copied_after_each_group_on_a_keyboard_every_letter_has_a_copy_time() {
    run(|| async {
        let (mut ui, recorder) = Ui::app_with_settings(along(4, 3, 20.0));
        let copied = copy_after(&mut ui, &recorder, By::Keyboard, 500, &GAPS).await;
        assert_eq!(copied.len(), 4);
        every_letter_copy_timed(&ui, &copied, false);
        let letters: std::collections::BTreeSet<char> = kept(&ui)
            .groups
            .iter()
            .flat_map(|group| group.sent.chars().collect::<Vec<_>>())
            .collect();
        ui.click("btn-back");
        ui.click("nav-stats");
        ui.click("seg-letters");
        let html = ui.html();
        for letter in letters {
            let id = format!("id=\"letter-{}\"", letter.to_ascii_lowercase());
            let at = html
                .find(&id)
                .unwrap_or_else(|| panic!("no {id} in {html}"));
            // Its own row: to the next letter's, or the end of the list.
            let row = &html[at..];
            let row = &row[..row[1..]
                .find("id=\"letter-")
                .map_or(row.len(), |next| next + 1)];
            assert!(
                row.contains(" · copy time"),
                "{letter} has no copy time: {row}"
            );
        }
        assert!(!html.contains("not timed yet"), "{html}");
    });
}

/// The same on the keypad, as on a phone or tablet.
#[test]
fn copied_after_each_group_on_the_keypad_every_letter_has_a_copy_time() {
    run(|| async {
        let (mut ui, recorder) = Ui::app_with_settings(along(4, 3, 20.0));
        let copied = copy_after(&mut ui, &recorder, By::Keypad, 500, &GAPS).await;
        assert_eq!(copied.len(), 4);
        every_letter_copy_timed(&ui, &copied, false);
    });
}

/// An output that says its sound reaches the ear later: the first letter's
/// pause is from when the group's end was heard.
#[test]
fn copied_after_through_an_output_with_latency_is_timed_from_when_it_was_heard() {
    run(|| async {
        let (mut ui, recorder) = Ui::app_with_settings(along(4, 3, 20.0));
        recorder.latency_sec.set(0.15);
        let copied = copy_after(&mut ui, &recorder, By::Keyboard, 500, &GAPS).await;
        every_letter_copy_timed(&ui, &copied, false);
    });
}

/// With Lock typing while sending on, each group is typed once it has been
/// sent: every letter has a copy time, and nothing offers to turn the lock
/// off.
#[test]
fn with_lock_typing_on_each_group_copied_after_has_a_copy_time() {
    run(|| async {
        let mut settings = along(4, 3, 20.0);
        settings.playback.lock_input_during_group_playback = true;
        let (mut ui, recorder) = Ui::app_with_settings(settings);
        let copied = copy_after(&mut ui, &recorder, By::Keyboard, 500, &GAPS).await;
        every_letter_copy_timed(&ui, &copied, true);
        assert!(!ui.shows("btn-turn-off-lock-typing"), "{}", ui.html());
    });
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

/// Short groups sent fast, each first key coming just after its group has
/// ended, as a learner who keeps up keys a group gone by before its first
/// character is: still typed along, every character timed. Two characters
/// of K and M at 30 WPM, sent evenly, leave 400 to 480 ms of the group
/// after its first character; keyed 490 ms after each, the first key comes
/// 10 to 90 ms after the group, inside the slack it is allowed.
#[test]
fn short_fast_groups_keyed_just_after_they_end_are_timed() {
    run(|| async {
        let mut settings = along(5, 2, 30.0);
        settings.playback.fist_variation = 0.0;
        let (mut ui, recorder) = Ui::app_with_settings(settings);
        let keyed = key_along(&mut ui, &recorder, By::Keyboard, 490).await;
        assert_eq!(keyed, 10);
        every_character_timed(&ui, keyed);
        for timing in &kept(&ui).group_timings {
            let times = timing.char_times.as_ref().expect("timed");
            assert!(
                cw_core::first_key_late_ms(times).is_some_and(|late| late > 0),
                "keyed while the group still sounded: {times:?}"
            );
        }
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

/// An older install's settings with Lock typing while sending on, turned
/// off in Settings: the very next session keyed along is timed.
#[test]
fn turned_off_in_settings_the_next_session_keyed_along_is_timed() {
    run(|| async {
        let mut settings = along(3, 3, 20.0);
        settings.playback.lock_input_during_group_playback = true;
        let store = Rc::new(crate::persist::memory::MemoryStore::default());
        store.save_settings(&settings);
        store.mark_tutorial_done();
        let (mut ui, recorder) = Ui::launch(store);
        ui.click("nav-settings");
        ui.commit("switch-lock-typing-while-sending", "false");
        assert!(!ui.with_app(|ctx| {
            ctx.settings
                .peek()
                .playback
                .lock_input_during_group_playback
        }));
        ui.click("nav-practice");
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
