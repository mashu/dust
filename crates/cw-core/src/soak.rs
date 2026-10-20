//! A randomised soak over the domain: settings from anywhere, sessions driven
//! by an adversarial event stream, and the invariants checked at every step.
//!
//! The unit tests describe behaviour somebody thought of. This describes what
//! must hold whatever happens, which is where the cases nobody thought of turn
//! up. It is deterministic — every failure prints the seed that produced it.

#![cfg(test)]

use crate::auto_level::apply_auto_level;
use crate::callsign::{CALLSIGN_TIER_MIN, parse_callsign};
use crate::contest::bot::{Copied, copy_of};
use crate::level::LEVEL_MIN;
use crate::machine::{GroupSource, SessionEffect, SessionEvent, SessionMachine};
use crate::session::MIN_RESPONSE_MS_PER_CHAR;
use crate::timing::resolve_pileup;
use crate::{
    Aim, Answered, AutoLevelCounters, CALLSIGN_TIER_MAX, CharSamplingState, CharSetMode,
    ContestEffect, ContestEvent, ContestKind, ContestMachine, ContestPhase, CopyTally,
    FILTER_WIDTH_MAX, FILTER_WIDTH_MIN, Fields, MonoMs, OpKey, PILEUP_LEVEL_MAX_DB,
    PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN, PauseWhy, QsoError, Rng, RowStatus,
    STATIONS_MAX, STATIONS_MIN, Sampler, ScoredQso, Sender, SessionId, SessionPhase, SessionResult,
    SplitMix64, TrainingSettings, Transmission, align_group, compute_char_pool, contest_score,
    evaluate_auto_level, generate_training_group, plan_morse_playback_for, plan_transmission,
    plan_turn, resolve_station,
};

/// Enough seeds to be worth running on every commit, and cheap enough to.
/// Turn it up when hunting: the failures name the seed they came from, so a
/// deep run reproduces exactly on a short one.
const SEEDS: u64 = 2_000;

/// Settings from anywhere, including values no screen would ever produce.
fn wild_settings(rng: &mut SplitMix64) -> TrainingSettings {
    let mut s = TrainingSettings::default();
    s.curriculum.level = rng.usize_in(0, 60) as u32;
    s.curriculum.digits_level = rng.usize_in(0, 20) as u32;
    s.curriculum.mixed_letters_percent = rng.usize_in(0, 200) as u32;
    s.curriculum.num_groups = rng.usize_in(0, 40) as u32;
    s.curriculum.min_group_size = rng.usize_in(0, 20) as u32;
    s.curriculum.max_group_size = rng.usize_in(0, 20) as u32;
    s.curriculum.link_group_size = rng.f64() < 0.5;
    s.curriculum.adaptive_group_size = rng.f64() < 0.5;
    s.playback.adaptive_speed = rng.f64() < 0.5;
    s.curriculum.aim = Aim::ALL[rng.usize_in(0, 2)];
    s.curriculum.char_set_mode = match rng.usize_in(0, 4) {
        0 => CharSetMode::Koch,
        1 => CharSetMode::Digits,
        2 => CharSetMode::Mixed,
        3 => CharSetMode::Words,
        _ => CharSetMode::Callsign,
    };
    s.curriculum.callsign_level = rng.usize_in(0, 20) as u32;
    // A list as it might be stored by hand: lower case, duplicates, words
    // Morse cannot send, one too long for a group. Or none at all.
    let words = [
        "cq",
        "CQ",
        "",
        "ÄRGER",
        "73",
        "R/P",
        "AAAAAAAAAAAAAAAAAAAAAAAAA",
        "qth",
    ];
    s.curriculum.word_list = words
        .iter()
        .filter(|_| rng.f64() < 0.5)
        .map(|word| (*word).to_string())
        .collect();
    if rng.f64() < 0.5 {
        let take = rng.usize_in(0, 10);
        s.curriculum.custom_sequence = "KMURESNAPTLWI0123456789".chars().take(take).collect();
        s.curriculum.sequence_is_custom = true;
    }
    s.playback.char_wpm_min = rng.pick_in_range(-10.0, 120.0);
    s.playback.char_wpm_max = rng.pick_in_range(-10.0, 120.0);
    s.playback.effective_wpm_min = rng.pick_in_range(-10.0, 120.0);
    s.playback.effective_wpm_max = rng.pick_in_range(-10.0, 120.0);
    s.playback.link_char_to_effective = rng.f64() < 0.5;
    s.playback.extra_word_space_multiplier = rng.pick_in_range(-5.0, 30.0);
    s.playback.group_timeout = rng.pick_in_range(-5.0, 600.0);
    s.playback.group_pause_sec = rng.pick_in_range(-5.0, 40.0);
    s.playback.countdown_sec = rng.usize_in(0, 40) as u32;
    s.playback.group_repeat_min = rng.usize_in(0, 12) as u32;
    s.playback.group_repeat_max = rng.usize_in(0, 12) as u32;
    s.playback.fist_variation = rng.pick_in_range(-1.0, 4.0);
    s.band.qsb_depth = rng.pick_in_range(-1.0, 4.0);
    s.band.qsb_rate_hz = rng.pick_in_range(-1.0, 40.0);
    s.band.qrn_level = rng.pick_in_range(-1.0, 4.0);
    s.band.noise_level = rng.pick_in_range(-1.0, 4.0);
    s.band.agc_enabled = rng.f64() < 0.5;
    s.band.filter_shape = match rng.usize_in(0, 2) {
        0 => crate::settings::FilterShape::Soft,
        1 => crate::settings::FilterShape::Sharp,
        _ => crate::settings::FilterShape::Peaked,
    };
    s.band.filter_width_hz = rng.pick_in_range(-500.0, 6_000.0);
    s.band.stations_min = rng.usize_in(0, 12) as u32;
    s.band.stations_max = rng.usize_in(0, 12) as u32;
    s.band.pileup_spread_hz = rng.pick_in_range(-100.0, 2_000.0);
    s.band.pileup_level_db = rng.pick_in_range(-20.0, 90.0);
    s.band.side_tone_min = rng.pick_in_range(-100.0, 4_000.0);
    s.band.side_tone_max = rng.pick_in_range(-100.0, 4_000.0);
    s.band.volume_min = rng.pick_in_range(-1.0, 4.0);
    s.band.volume_max = rng.pick_in_range(-1.0, 4.0);
    s
}

/// What the trainer needs to be true of any settings before it starts.
fn check_clamped(s: &TrainingSettings, seed: u64) {
    let once = s.clone().clamp();
    assert_eq!(
        once.clone().clamp(),
        once,
        "clamp must settle in one pass (seed {seed})"
    );
    let c = &once.curriculum;
    assert!(c.num_groups >= 1, "seed {seed}");
    assert!(
        (CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX).contains(&c.callsign_level),
        "seed {seed}"
    );
    assert!(c.min_group_size >= 1, "seed {seed}");
    assert!(c.max_group_size >= c.min_group_size, "seed {seed}");
    let p = &once.playback;
    assert!(
        p.char_wpm_min.is_finite() && p.char_wpm_min > 0.0,
        "seed {seed}"
    );
    assert!(p.char_wpm_max >= p.char_wpm_min, "seed {seed}");
    assert!(
        p.effective_wpm_min.is_finite() && p.effective_wpm_min > 0.0,
        "seed {seed}"
    );
    assert!(p.effective_wpm_max >= p.effective_wpm_min, "seed {seed}");
    assert!(
        (0.0..=1.0).contains(&p.fist_variation),
        "seed {seed}: fists stray by {}",
        p.fist_variation
    );
    assert!(p.group_repeat_min >= 1, "seed {seed}");
    assert!(p.group_repeat_max >= p.group_repeat_min, "seed {seed}");
    // Zero is the "wait for me" setting, so only a negative or wild one is wrong.
    assert!(
        p.group_timeout.is_finite() && p.group_timeout >= 0.0,
        "seed {seed}"
    );
    assert!(
        p.group_pause_sec.is_finite() && (0.0..=15.0).contains(&p.group_pause_sec),
        "seed {seed}"
    );
    assert!(
        p.countdown_sec <= crate::settings::COUNTDOWN_MAX_SEC,
        "seed {seed}"
    );
    let b = &once.band;
    assert!(
        b.side_tone_min > 0.0 && b.side_tone_max >= b.side_tone_min,
        "seed {seed}"
    );
    assert!((0.0..=1.0).contains(&b.volume_min), "seed {seed}");
    assert!(
        b.volume_max >= b.volume_min && b.volume_max <= 1.0,
        "seed {seed}"
    );
    assert!(
        (FILTER_WIDTH_MIN..=FILTER_WIDTH_MAX).contains(&b.filter_width_hz),
        "seed {seed}: the receiver came out {} Hz wide",
        b.filter_width_hz
    );
    // And what is built from it passes the whole side-tone range, however
    // the two were set, without ever going wider than a receiver is built.
    let built = crate::band::FilterDesign::from_settings(&once).width_hz();
    assert!(
        built >= b.filter_width_hz && built <= crate::band::MAX_RECEIVER_WIDTH_HZ,
        "seed {seed}: asked for {} Hz, built {built} Hz",
        b.filter_width_hz
    );
    assert!(
        (0.0..=1.0).contains(&b.noise_level) && (0.0..=1.0).contains(&b.qrn_level),
        "seed {seed}: the band came out at noise {} and static {}",
        b.noise_level,
        b.qrn_level
    );
    assert!(
        (STATIONS_MIN..=STATIONS_MAX).contains(&b.stations_min)
            && (b.stations_min..=STATIONS_MAX).contains(&b.stations_max),
        "seed {seed}: {}..{} stations",
        b.stations_min,
        b.stations_max
    );
    assert!(
        (PILEUP_SPREAD_MIN..=PILEUP_SPREAD_MAX).contains(&b.pileup_spread_hz),
        "seed {seed}: the pile-up spans {} Hz",
        b.pileup_spread_hz
    );
    assert!(
        (PILEUP_LEVEL_MIN_DB..=PILEUP_LEVEL_MAX_DB).contains(&b.pileup_level_db),
        "seed {seed}: the pile-up is {} dB down",
        b.pileup_level_db
    );
    assert!(
        !compute_char_pool(&once).is_empty(),
        "a session can always be started (seed {seed})"
    );
}

#[test]
fn any_settings_clamp_into_something_the_trainer_can_start_from() {
    for seed in 0..SEEDS {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0x9E37_79B9).wrapping_add(1));
        let settings = wild_settings(&mut rng);
        check_clamped(&settings, seed);

        // And the clamped settings really do produce a sendable group.
        let settings = settings.clamp();
        let group = generate_training_group(&settings, &mut CharSamplingState::default(), &mut rng);
        assert!(
            !group.is_empty(),
            "an empty group is never sendable (seed {seed})"
        );
        if settings.curriculum.char_set_mode == CharSetMode::Callsign {
            // A callsign is as long as it is; the group-size settings have no
            // say over it, which is why the screen hides them in this mode.
            assert!(
                parse_callsign(&group).is_some(),
                "seed {seed}: {group:?} is not a callsign"
            );
        } else if settings.curriculum.char_set_mode == CharSetMode::Words {
            // A word is sent whole, and only one the list kept.
            assert!(
                settings.words().contains(&group.as_str()),
                "seed {seed}: {group:?} is not one of the words"
            );
        } else {
            assert!(
                group.chars().count() >= settings.curriculum.min_group_size as usize,
                "seed {seed}: {group:?}"
            );
            assert!(
                group.chars().count() <= settings.curriculum.max_group_size as usize,
                "seed {seed}: {group:?}"
            );
        }
        // The pool, not `active_alphabet`: in Mixed mode that one is the letter
        // progression, and the sender draws digits alongside it.
        let pool = compute_char_pool(&settings);
        assert!(
            group.chars().all(|ch| pool.contains(&ch)),
            "seed {seed}: {group:?} strays outside {pool:?}"
        );

        let plan =
            plan_morse_playback_for(&group, &settings, &resolve_station(&settings, &mut rng));
        assert!(
            plan.duration_sec.is_finite() && plan.duration_sec > 0.0,
            "seed {seed}"
        );
        assert!(
            plan.resolved_char_wpm.is_finite() && plan.resolved_char_wpm > 0.0,
            "seed {seed}"
        );
        assert!(!plan.events.is_empty(), "seed {seed}");
        for event in &plan.events {
            assert!(
                event.start_sec.is_finite() && event.start_sec >= 0.0,
                "seed {seed}"
            );
            assert!(
                event.duration_sec.is_finite() && event.duration_sec > 0.0,
                "seed {seed}"
            );
            assert!(
                event.frequency_hz.is_finite() && event.frequency_hz > 0.0,
                "seed {seed}"
            );
            assert!(
                event.start_sec + event.duration_sec <= plan.duration_sec + 1e-6,
                "seed {seed}: a tone runs past the end of the plan"
            );
            assert!(
                event
                    .envelope
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                "seed {seed}: envelope off the rails"
            );
        }
    }
}

/// One of everything the runtime can send, including events for the wrong
/// group and timers that are already stale.
fn random_event(rng: &mut SplitMix64, groups: usize, timers: &[u64]) -> SessionEvent {
    let index = rng.usize_in(0, groups.saturating_sub(1).max(1));
    // Whatever a keyboard or a paste can produce, not just tidy call signs.
    const KEYS: [char; 12] = [
        'K', 'M', 'u', 'r', '5', ' ', '\n', '\t', 'é', '中', '.', '\u{200b}',
    ];
    let text: String = (0..rng.usize_in(0, 12))
        .map(|_| KEYS[rng.usize_in(0, KEYS.len() - 1)])
        .collect();
    match rng.usize_in(0, 7) {
        0 => SessionEvent::PlaybackEnded {
            index,
            duration_sec: rng.pick_in_range(0.0, 5.0),
            char_wpm: rng.pick_in_range(1.0, 60.0),
            effective_wpm: rng.pick_in_range(1.0, 60.0),
        },
        1 => SessionEvent::PlaybackStopped { index },
        2 => SessionEvent::Input { index, text },
        3 => SessionEvent::Confirm,
        // A timer the machine started, or one it never did.
        4 => SessionEvent::TimerFired {
            id: timers
                .get(rng.usize_in(0, timers.len()))
                .copied()
                .unwrap_or(rng.usize_in(0, 40) as u64),
        },
        5 => SessionEvent::FinishNow,
        6 => SessionEvent::TimerFired {
            id: timers.last().copied().unwrap_or(0),
        },
        _ => SessionEvent::Abort,
    }
}

/// Adaptive group size draws every length inside the range the settings
/// allow, whatever has been copied before it and however badly.
#[test]
fn adaptive_lengths_never_leave_the_range() {
    for seed in 0..SEEDS / 4 {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0x9E37_79B9).wrapping_add(3));
        let mut settings = wild_settings(&mut rng).clamp();
        settings.curriculum.adaptive_group_size = true;
        let shortest = settings.curriculum.min_group_size as usize;
        let longest = settings.curriculum.max_group_size as usize;
        let mut sampler = Sampler::new(&[], &settings, seed);
        for _ in 0..30 {
            let (group, _) = sampler.next_group(&settings);
            if settings.curriculum.char_set_mode.draws_characters() {
                let length = group.chars().count();
                assert!(
                    (shortest..=longest).contains(&length),
                    "seed {seed}: {length} outside {shortest}..={longest}"
                );
            }
            let mut answer: String = group.chars().filter(|_| rng.f64() < 0.8).collect();
            if rng.f64() < 0.1 {
                answer.push('X');
            }
            sampler.learn(Answered {
                sent: &group,
                received: &answer,
                copied: &CopyTally::of(&group, &answer),
                played: None,
            });
        }
    }
}

#[test]
fn no_run_of_events_can_put_a_session_somewhere_impossible() {
    for seed in 0..SEEDS {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0x2545_F491).wrapping_add(7));
        let settings = wild_settings(&mut rng).clamp();
        let groups = settings.curriculum.num_groups.max(1) as usize;
        let sampler = Sampler::new(&[], &settings, seed);
        let (mut machine, mut effects) =
            SessionMachine::start(SessionId::new(seed), MonoMs(0), settings.clone(), sampler);

        let mut now = 0u64;
        let mut timers: Vec<u64> = Vec::new();
        for step in 0..200 {
            check_effects(&effects, groups, seed, step);
            timers.extend(effects.iter().filter_map(|effect| match effect {
                SessionEffect::StartTimer { id, .. } => Some(*id),
                _ => None,
            }));
            now += rng.usize_in(0, 4_000) as u64;
            // A session that has already finished or aborted is done: whatever
            // arrives late — a send that was in flight, a timer, a keystroke —
            // must be dropped, not turned into more work.
            let was_terminal = machine.is_terminal();
            let event = random_event(&mut rng, groups, &timers);
            effects = machine.apply(event.clone(), MonoMs(now));
            if was_terminal {
                assert!(
                    effects.is_empty(),
                    "seed {seed} step {step}: {event:?} restarted a finished session: {effects:?}"
                );
            }

            let view = machine.session().view();
            assert!(
                view.repeat_done <= view.repeat_total,
                "seed {seed} step {step}: {view:?} after {event:?}"
            );
            if let SessionPhase::Playing { index }
            | SessionPhase::RepeatGap { index }
            | SessionPhase::AwaitingAnswer { index } = machine.phase()
            {
                assert!(
                    index < groups,
                    "seed {seed} step {step}: phase points at group {index} of {groups}"
                );
            }
            if let SessionPhase::Countdown { remaining } = machine.phase() {
                assert!(
                    remaining >= 1 && !machine.session().any_confirmed(),
                    "seed {seed} step {step}: counting down from {remaining} mid-session"
                );
            }
            if let SessionPhase::InterGroupGap { next } = machine.phase() {
                assert!(
                    next < groups,
                    "seed {seed} step {step}: gap points past the end"
                );
                assert!(
                    machine
                        .session()
                        .group(next)
                        .is_some_and(|g| !g.sent().is_empty()),
                    "seed {seed} step {step}: waiting on a group that has nothing to send"
                );
            }
        }

        // Whatever happened, the session can still be scored and stored.
        let result = machine.session().result(now, "2026-01-01".into());
        assert!(
            (0.0..=1.0).contains(&result.accuracy),
            "seed {seed}: accuracy {} is not a fraction",
            result.accuracy
        );
        assert!(
            result.score.is_finite() && result.score >= 0.0,
            "seed {seed}"
        );
        assert!(result.groups.len() <= groups, "seed {seed}");
        assert!(
            result
                .group_timings
                .iter()
                .all(|t| t.time_to_complete_ms >= MIN_RESPONSE_MS_PER_CHAR),
            "seed {seed}: an answer quicker than anyone types"
        );
    }
}

/// Every effect a machine hands back has to be one the runtime can act on.
fn check_effects(effects: &[SessionEffect], groups: usize, seed: u64, step: usize) {
    for effect in effects {
        match effect {
            SessionEffect::Play { index, text } => {
                assert!(
                    *index < groups,
                    "seed {seed} step {step}: play past the end"
                );
                assert!(!text.is_empty(), "seed {seed} step {step}: an empty send");
            }
            SessionEffect::StartTimer { ms, .. } => {
                assert!(*ms > 0, "seed {seed} step {step}: a zero-length timer");
            }
            SessionEffect::StopAudio
            | SessionEffect::PersistAndShowResults
            | SessionEffect::AbortToHome => {}
        }
    }
}

#[test]
fn scoring_holds_for_anything_a_listener_can_type() {
    const KEYS: [char; 14] = [
        'K', 'M', 'U', 'R', 'E', '5', '0', 'x', ' ', '\n', 'é', '中', '.', '\u{200b}',
    ];
    for seed in 0..SEEDS {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0xA24B_AED4).wrapping_add(3));
        let sent: String = (0..rng.usize_in(0, 12))
            .map(|_| KEYS[rng.usize_in(0, 6)])
            .collect();
        let received: String = (0..rng.usize_in(0, 14))
            .map(|_| KEYS[rng.usize_in(0, KEYS.len() - 1)])
            .collect();

        // The alignment may pad either side, but it may never lose a character
        // or put one back in a different order: the whole letter-level score
        // rests on the pairing being faithful.
        let pairs = align_group(&sent, &received);
        let aligned_sent: String = pairs.iter().filter_map(|p| p.sent_char).collect();
        assert_eq!(
            aligned_sent,
            sent.to_ascii_uppercase(),
            "seed {seed}: the alignment lost or reordered what was sent"
        );
        let aligned_received: String = pairs.iter().filter_map(|p| p.received_char).collect();
        assert_eq!(
            aligned_received,
            received.to_ascii_uppercase(),
            "seed {seed}: the alignment lost or reordered what was heard"
        );
        assert!(
            pairs
                .iter()
                .all(|p| p.sent_char.is_some() || p.received_char.is_some()),
            "seed {seed}: an alignment pair with nothing in it"
        );
        assert!(
            pairs
                .iter()
                .all(|p| !p.matched || p.sent_char == p.received_char),
            "seed {seed}: a pair counted as a match without matching"
        );

        let tally = CopyTally::of(&sent, &received);
        let overall = tally.accuracy();
        assert!(
            (0.0..=1.0).contains(&overall),
            "seed {seed}: {overall} is not a fraction for {sent:?}/{received:?}"
        );
        for (ch, acc) in &tally.sent {
            assert!(
                acc.correct <= acc.total && acc.total > 0,
                "seed {seed}: {ch:?} scored {}/{}",
                acc.correct,
                acc.total
            );
        }
        // Hearing exactly what was sent is always full marks — unless there
        // was nothing to hear, which scores nothing rather than everything.
        let perfect = CopyTally::of(&sent, &sent).accuracy();
        assert_eq!(
            perfect,
            if sent.is_empty() { 0.0 } else { 1.0 },
            "seed {seed}: {sent:?} against itself"
        );
    }
}

/// Settings have to survive the trip to storage and back, and to survive
/// coming back from a build that did not have every field yet. The platform
/// notes promise that updating never clears anything; this is that promise,
/// tested.
#[test]
fn settings_survive_storage_and_older_saves() {
    for seed in 0..SEEDS {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0x1D87_2B41).wrapping_add(11));
        let settings = wild_settings(&mut rng).clamp();

        let json = serde_json::to_string(&settings).expect("settings serialize");
        let back: TrainingSettings = serde_json::from_str(&json).expect("settings load");
        assert_eq!(
            back, settings,
            "seed {seed}: a round trip changed the settings"
        );

        // An older save: one section is missing entirely. It has to load, and
        // what it does carry has to come back unchanged.
        let mut object: serde_json::Value = serde_json::from_str(&json).expect("object");
        let keys: Vec<String> = object
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        let dropped = &keys[rng.usize_in(0, keys.len() - 1)];
        object
            .as_object_mut()
            .expect("an object")
            .remove(dropped)
            .expect("the key was there");
        let older: TrainingSettings = serde_json::from_value(object.clone()).unwrap_or_else(|e| {
            panic!("seed {seed}: a save without {dropped:?} would not load: {e}")
        });
        let repaired = older.clamp();
        assert_eq!(
            repaired.clone().clamp(),
            repaired,
            "seed {seed}: an older save did not settle"
        );
        assert!(
            !compute_char_pool(&repaired).is_empty(),
            "seed {seed}: a save without {dropped:?} left nothing to practise"
        );
    }
}

/// The trainer moves your level for you. However long it runs and whatever
/// you score, it has to leave settings the trainer can still start from — and
/// it has to stay inside the alphabet you are actually practising.
#[test]
fn auto_levelling_never_walks_the_settings_out_of_range() {
    for seed in 0..SEEDS {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0x6C07_8965).wrapping_add(5));
        let mut settings = wild_settings(&mut rng).clamp();
        settings.auto_level.auto_adjust_level = true;
        settings.auto_level.auto_adjust_above_threshold_count = rng.usize_in(0, 5) as u32;
        settings.auto_level.auto_adjust_below_threshold_count = rng.usize_in(0, 5) as u32;
        settings.auto_level.auto_adjust_threshold = rng.pick_in_range(
            crate::ADJUST_THRESHOLD_MIN_PCT,
            crate::ADJUST_THRESHOLD_MAX_PCT,
        );
        let mut counters = AutoLevelCounters::default();

        for round in 0..30 {
            let mut session = crate::session::SessionResult::empty_for_test();
            session.accuracy = rng.pick_in_range(0.0, 1.0);
            session.groups = vec![Default::default(); rng.usize_in(0, 40)];
            if let Some(change) = evaluate_auto_level(&session, &settings, &mut counters) {
                assert_eq!(
                    change.from.abs_diff(change.to),
                    1,
                    "seed {seed} round {round}: a level moved by more than one"
                );
                assert!(change.to >= LEVEL_MIN, "seed {seed} round {round}");
                apply_auto_level(&mut settings, &change);
            }
            assert_eq!(
                settings.clone().clamp(),
                settings,
                "seed {seed} round {round}: auto-levelling left settings needing a clamp"
            );
            assert!(
                !compute_char_pool(&settings).is_empty(),
                "seed {seed} round {round}: nothing left to practise"
            );
            assert!(
                settings.active_level() <= settings.max_active_level(),
                "seed {seed} round {round}: level past the end of the alphabet"
            );
        }
    }
}

/// Callsigns are the one group the trainer does not draw character by
/// character, so they get their own sweep: whatever the tier, what comes out
/// has to be a callsign a radio could send and the app could score.
#[test]
fn every_callsign_the_trainer_can_send_is_a_real_one() {
    for seed in 0..SEEDS {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0xB5297A4D).wrapping_add(17));
        let mut settings = wild_settings(&mut rng).clamp();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level =
            rng.usize_in(CALLSIGN_TIER_MIN as usize, CALLSIGN_TIER_MAX as usize) as u32;
        let tier = settings.curriculum.callsign_level;
        let pool = compute_char_pool(&settings);
        let mut next = CharSamplingState::default();

        let call = generate_training_group(&settings, &mut next, &mut rng);
        assert!(
            parse_callsign(&call).is_some(),
            "seed {seed} tier {tier}: {call:?} is not a callsign"
        );
        assert!(
            call.chars().all(|ch| pool.contains(&ch)),
            "seed {seed} tier {tier}: {call:?} strays outside the pool"
        );
        // Whatever was sent has to be counted, or coverage balancing goes blind.
        for ch in call.chars() {
            assert!(
                next.session_sample_counts.get(&ch).copied().unwrap_or(0) > 0,
                "seed {seed}: {ch:?} was sent but not counted"
            );
        }

        // And it has to be sendable, at whatever speed the settings ask for.
        let plan = plan_morse_playback_for(&call, &settings, &resolve_station(&settings, &mut rng));
        assert!(
            plan.duration_sec.is_finite() && plan.duration_sec > 0.0,
            "seed {seed}: {call:?} has no length"
        );
        assert!(
            !plan.events.is_empty(),
            "seed {seed}: {call:?} made no tones"
        );
    }
}

/// However the band is set, the receiver has to produce something a person
/// could listen to: finite, inside full scale, and actually audible rather
/// than a dead stream of zeros.
#[test]
fn no_band_setting_makes_an_unlistenable_receiver() {
    for seed in 0..SEEDS / 20 {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0x27D4_EB2D).wrapping_add(19));
        let mut settings = wild_settings(&mut rng).clamp();
        settings.band.qrn_enabled = true;
        settings.band.noise_enabled = true;
        let settings = settings.clamp();

        let mut mixer = crate::band::BandMixer::new(16_000, &settings, seed | 1);
        let mut out = vec![0.0f32; 16_000];
        mixer.fill_background(&mut out);

        assert!(
            out.iter().all(|s| s.is_finite()),
            "seed {seed}: the band produced something that is not a number"
        );
        // A crash close enough to pin the receiver is a real thing, and the
        // limiter is what catches it. What must never happen is the output
        // *living* at full scale, which is a runaway rather than a crash.
        let pinned = out.iter().filter(|s| s.abs() >= 1.0).count();
        assert!(
            out.iter().all(|s| s.abs() <= 1.0),
            "seed {seed}: the band went past full scale"
        );
        assert!(
            pinned * 20 < out.len(),
            "seed {seed}: {pinned} of {} samples sat at full scale",
            out.len()
        );
        let loudest = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        // The floor is continuous, so any of it is heard at once. Static is a
        // Poisson process: at the bottom of its control a quiet second with no
        // sferic in it is the truth, not a fault, so it only has to be heard
        // once it is busy enough that a silent second would be absurd.
        if settings.band.noise_level > 0.05 || settings.band.qrn_level > 0.5 {
            assert!(
                loudest > 1e-6,
                "seed {seed}: the band was switched on and made no sound"
            );
        }
    }
}

/// However the band is set, a pile-up has to stay answerable: the station you
/// are copying is the strongest, everyone else is off its frequency, and
/// nothing runs past the send they are laid over.
#[test]
fn a_pile_up_is_always_something_you_could_copy() {
    for seed in 0..SEEDS / 4 {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0x85EB_CA6B).wrapping_add(29));
        let settings = wild_settings(&mut rng).clamp();
        let mut state = CharSamplingState::default();
        let wanted_text = generate_training_group(&settings, &mut state, &mut rng);
        let voice = resolve_station(&settings, &mut rng);
        let texts: Vec<String> = (0..settings.band.stations_max)
            .map(|_| generate_training_group(&settings, &mut state, &mut rng))
            .collect();
        let others = resolve_pileup(&settings, &voice, &wanted_text, &texts, &mut rng);

        assert!(
            others.len() < settings.band.stations_max as usize,
            "seed {seed}: more stations than the settings allow"
        );
        // Note what the floor does *not* promise. It says how many call, not
        // how many you end up hearing: a caller with nowhere to sit inside a
        // narrow passband is one the receiver does not give you, which is the
        // filter doing exactly what it is for. `the_floor_is_how_many_call`
        // pins the floor itself, where there is room for everyone.
        for other in &others {
            assert!(
                other.voice.volume < voice.volume,
                "seed {seed}: an interferer outgunned the station you want"
            );
            assert!(other.voice.tone_hz > 0.0 && other.voice.tone_hz.is_finite());
            assert_ne!(other.text, wanted_text, "seed {seed}: a duplicate call");
        }

        let sent = Transmission {
            text: wanted_text,
            voice,
            others,
        };
        let planned = plan_transmission(&sent, &settings);
        let ends_at = planned.wanted.duration_sec;
        for plan in &planned.others {
            for event in &plan.events {
                assert!(
                    event.start_sec + event.duration_sec <= ends_at + 1e-9,
                    "seed {seed}: a station ran past the send"
                );
                assert!(event.frequency_hz.is_finite() && event.frequency_hz > 0.0);
                assert!(event.target_gain.is_finite() && event.target_gain >= 0.0);
            }
        }
    }
}

/// Settings from anywhere, in Contest mode: a call from a few that read in
/// different ways, none among them, and runs short enough to reach their
/// end.
fn contest_settings(rng: &mut SplitMix64) -> TrainingSettings {
    const CALLS: [&str; 6] = ["DL1ABC", "K1ABC", "VP2E/DL1ABC", "JA1XYZ", "W1AW/4", ""];
    let mut s = wild_settings(rng);
    s.curriculum.char_set_mode = CharSetMode::Contest;
    s.contest.my_call = CALLS[rng.usize_in(0, CALLS.len() - 1)].to_string();
    s.contest.contest_minutes = [1, 2, 3, 4, 0][rng.usize_in(0, 4)];
    s.clamp()
}

/// `text` one edit off: a character changed, dropped or added.
fn one_edit_off(text: &str, rng: &mut SplitMix64) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    let at = rng.usize_in(0, chars.len());
    match rng.usize_in(0, 2) {
        0 if at < chars.len() => chars[at] = if chars[at] == 'X' { 'Y' } else { 'X' },
        1 if at < chars.len() => {
            chars.remove(at);
        }
        _ => chars.insert(at, '7'),
    }
    chars.into_iter().collect()
}

/// A key and its fields: 40% as heard, a quarter of those with the call
/// heard before the last, as an operator logging a QSO it forgot types it;
/// 20% one edit off, 10% empty, 10% a partial, 20% junk.
fn contest_key(rng: &mut SplitMix64, now: u64, seen: &Seen) -> ContestEvent {
    use OpKey::*;
    const KEYS: [OpKey; 15] = [
        Esm,
        Cq,
        Exchange,
        Tu,
        MyCall,
        HisCall,
        HisCallAndExchange,
        B4,
        Again,
        Nil,
        NrQuery,
        TuAndLog,
        LogOnly,
        Wipe,
        StopTx,
    ];
    const JUNK: [char; 14] = [
        'K', '5', 'X', 'Y', 'Z', '?', '/', ' ', 'é', '中', 't', '1', 'N', '\u{200b}',
    ];
    let key = if rng.f64() < 0.6 {
        Esm
    } else {
        KEYS[rng.usize_in(0, KEYS.len() - 1)]
    };
    let junk = |rng: &mut SplitMix64| -> String {
        (0..rng.usize_in(0, 9))
            .map(|_| JUNK[rng.usize_in(0, JUNK.len() - 1)])
            .collect()
    };
    let (call, serial) = (seen.call.as_str(), seen.serial.as_str());
    let draw = rng.f64();
    let (call, exch) = if draw < 0.1 {
        (seen.previous.clone(), serial.to_string())
    } else if draw < 0.4 {
        (call.to_string(), serial.to_string())
    } else if draw < 0.6 {
        (one_edit_off(call, rng), one_edit_off(serial, rng))
    } else if draw < 0.7 {
        (String::new(), String::new())
    } else if draw < 0.8 {
        (call.chars().take(2).chain(['?']).collect(), String::new())
    } else {
        (junk(rng), junk(rng))
    };
    let stamp = |rng: &mut SplitMix64| MonoMs(now.saturating_sub(rng.usize_in(0, 3_000) as u64));
    let fields = Fields {
        call,
        exch,
        call_edited: stamp(rng),
        exch_complete: (rng.f64() < 0.7).then(|| stamp(rng)),
    };
    ContestEvent::Key { key, fields }
}

/// What came of every contest run across the seeds, to show the soak
/// reached every kind of QSO.
#[derive(Debug, Default)]
struct Outcomes {
    clean: u32,
    busted: u32,
    field: u32,
    nil: u32,
    lost: u32,
}

/// What a contest soak run has seen of the machine's effects: every id
/// handed out, the turns and timers among them, the last call and serial
/// a listener would have copied from the callers, and the call before it.
#[derive(Debug, Default)]
struct Seen {
    ids: std::collections::BTreeSet<u64>,
    turns: Vec<u64>,
    timers: Vec<u64>,
    call: String,
    serial: String,
    previous: String,
}

/// Every effect a contest run hands back has to be one the runtime can act
/// on: an id never handed out before, a timer that runs, a turn with
/// something in it and no reply past your message.
fn check_contest_effects(
    effects: &[ContestEffect],
    settings: &TrainingSettings,
    seed: u64,
    step: usize,
    seen: &mut Seen,
) {
    for effect in effects {
        match effect {
            ContestEffect::PlayTurn(turn) => {
                assert!(
                    seen.ids.insert(turn.id),
                    "seed {seed} step {step}: id {} again",
                    turn.id
                );
                seen.turns.push(turn.id);
                assert!(!turn.segments.is_empty(), "seed {seed}: an empty turn");
                let planned = plan_turn(turn, settings);
                for other in &planned.others {
                    assert!(
                        other.duration_sec <= planned.wanted.duration_sec + 1e-9,
                        "seed {seed} step {step}: a reply ends after your message"
                    );
                }
                for segment in &turn.segments {
                    if segment.who == Sender::You {
                        continue;
                    }
                    match copy_of(&segment.text) {
                        Some(Copied::Call(call) | Copied::Correction(call)) => {
                            if seen.call != call {
                                seen.previous = std::mem::replace(&mut seen.call, call.to_string());
                            }
                        }
                        Some(Copied::Serial(serial)) => seen.serial = serial.to_string(),
                        None => {}
                    }
                }
            }
            ContestEffect::StartTimer { id, ms } => {
                assert!(
                    seen.ids.insert(*id),
                    "seed {seed} step {step}: id {id} again"
                );
                assert!(*ms > 0, "seed {seed} step {step}: a zero-length timer");
                seen.timers.push(*id);
            }
            _ => {}
        }
    }
}

/// One of everything the runtime and a careless operator can send a run at
/// `now`: keys, news of the turn on the air or of a stale one, timers by
/// any id, pauses stamped no earlier than `shown_at`, when the run was
/// last shown, resumes, speed changes and early ends.
fn random_contest_event(
    rng: &mut SplitMix64,
    now: u64,
    shown_at: u64,
    seen: &Seen,
) -> ContestEvent {
    // Mostly the latest id, sometimes an older one, now and then one never
    // handed out.
    let pick = |rng: &mut SplitMix64, ids: &[u64]| {
        if ids.is_empty() || rng.f64() < 0.1 {
            rng.usize_in(0, 60) as u64
        } else if rng.f64() < 0.7 {
            ids[ids.len() - 1]
        } else {
            ids[rng.usize_in(0, ids.len() - 1)]
        }
    };
    let draw = rng.f64();
    if draw < 0.36 {
        contest_key(rng, now, seen)
    } else if draw < 0.46 {
        ContestEvent::TurnStarted {
            id: pick(rng, &seen.turns),
            heard_from: MonoMs(now + rng.usize_in(0, 200) as u64),
        }
    } else if draw < 0.62 {
        ContestEvent::TurnEnded {
            id: pick(rng, &seen.turns),
        }
    } else if draw < 0.65 {
        ContestEvent::TurnCancelled {
            id: pick(rng, &seen.turns),
        }
    } else if draw < 0.67 {
        ContestEvent::TurnFailed {
            id: pick(rng, &seen.turns),
        }
    } else if draw < 0.87 {
        ContestEvent::TimerFired {
            id: pick(rng, &seen.timers),
        }
    } else if draw < 0.91 {
        // Stamped as late as the heartbeat stamps one: never before the run
        // was last shown, at the event before.
        ContestEvent::Pause {
            why: [PauseWhy::You, PauseWhy::Hidden, PauseWhy::Sound][rng.usize_in(0, 2)],
            at: MonoMs(
                now.saturating_sub(rng.usize_in(0, 2_500) as u64)
                    .max(shown_at),
            ),
        }
    } else if draw < 0.97 {
        ContestEvent::Resume
    } else if draw < 0.995 {
        ContestEvent::Speed {
            delta: [2, -2, 9, -40][rng.usize_in(0, 3)],
        }
    } else {
        ContestEvent::FinishNow
    }
}

/// One contest soak run: every event applied, with when, what each gave,
/// the machine as it ended, and when the run started and ended.
struct SoakRun {
    events: Vec<(ContestEvent, MonoMs)>,
    effects: Vec<Vec<ContestEffect>>,
    machine: ContestMachine<Sampler>,
    started: Option<u64>,
    ended: Option<u64>,
}

/// Soak a run of seed `seed`, on events of its own or those of `replay`.
fn contest_run(
    seed: u64,
    settings: &TrainingSettings,
    replay: Option<&[(ContestEvent, MonoMs)]>,
) -> SoakRun {
    let mut rng = SplitMix64::new(seed.wrapping_mul(0x4F1B_BCDD).wrapping_add(31));
    let sampler = Sampler::new(&[], settings, seed);
    let (mut machine, first) = ContestMachine::start(seed, MonoMs(0), settings.clone(), sampler);
    let mut applied = Vec::new();
    let mut all_effects = vec![first.clone()];
    let mut seen = Seen::default();
    let mut now = 0u64;
    let mut shown = 0u64;
    let mut shown_at = 0u64;
    let mut started = (machine.phase() == ContestPhase::Running).then_some(0);
    let mut ended = None;
    let mut effects = first;
    let steps = replay.map_or(400, <[_]>::len);
    for step in 0..steps {
        check_contest_effects(&effects, settings, seed, step, &mut seen);
        let (event, at) = match replay {
            Some(events) => events[step].clone(),
            None => {
                now += match rng.usize_in(0, 9) {
                    0 => 0,
                    1..=6 => rng.usize_in(0, 1_500) as u64,
                    _ => rng.usize_in(0, 6_000) as u64,
                };
                let event = random_contest_event(&mut rng, now, shown_at, &seen);
                (event, MonoMs(now))
            }
        };
        let was_terminal = machine.is_terminal();
        effects = machine.apply(event.clone(), at);
        if was_terminal {
            assert!(
                effects.is_empty(),
                "seed {seed} step {step}: {event:?} after the end"
            );
        }
        if started.is_none() && machine.phase() == ContestPhase::Running {
            started = Some(at.0);
        }
        if !was_terminal && machine.is_terminal() {
            ended = Some(at.0);
        }
        let view = machine.view(at);
        if let Some(mut clock) = view.clock {
            let reading = clock.elapsed_ms(at);
            // The one step back by design: a send whose every retry failed
            // gives its retries back as time paused.
            let failed = matches!(event, ContestEvent::TurnFailed { .. })
                && view.phase
                    == (ContestPhase::Paused {
                        why: PauseWhy::Sound,
                    });
            assert!(
                reading >= shown || failed,
                "seed {seed} step {step}: the clock went back"
            );
            shown = reading;
        }
        shown_at = at.0;
        applied.push((event, at));
        all_effects.push(effects.clone());
    }
    SoakRun {
        events: applied,
        effects: all_effects,
        machine,
        started,
        ended,
    }
}

/// Contest runs driven by a stream of anything the runtime and a careless
/// operator can send (§13 of the design): keys with fields as heard, a slip
/// off, empty, partial or junk; turn news for the turn on the air and for
/// stale ones; timers by any id; pauses, resumes, hidden pages and early
/// ends. Whatever happens, the run stays whole and keeps what it should.
#[test]
fn no_run_of_events_can_put_a_contest_run_somewhere_impossible() {
    let mut outcomes = Outcomes::default();
    for seed in 0..SEEDS {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0x2F7A_9C35).wrapping_add(37));
        let settings = contest_settings(&mut rng);
        let SoakRun {
            mut events,
            mut machine,
            started,
            ended,
            ..
        } = contest_run(seed, &settings, None);
        let finished_at = ended.unwrap_or_else(|| {
            let end = events.last().map_or(0, |(_, at)| at.0) + 1;
            let _ = machine.apply(ContestEvent::FinishNow, MonoMs(end));
            events.push((ContestEvent::FinishNow, MonoMs(end)));
            end
        });
        let view = machine.view(MonoMs(finished_at));
        let result = machine.result(finished_at, "2026-10-20".into());
        // One station, one copy: every station worked, logged or lost has
        // its groups in exactly one of its QSOs, however often it was
        // logged or lost.
        for (station, copies) in machine.copies_by_station() {
            assert_eq!(
                copies, 1,
                "seed {seed}: station {station} copied {copies} times"
            );
        }
        assert!(
            view.log
                .iter()
                .all(|row| matches!(row.status, RowStatus::Checked(_))),
            "seed {seed}: a row is still waiting at the end"
        );
        assert!(
            result.score.is_finite() && result.score >= 0.0,
            "seed {seed}"
        );
        assert!((0.0..=1.0).contains(&result.accuracy), "seed {seed}");
        assert!(
            result
                .group_timings
                .iter()
                .zip(&result.groups)
                .all(|(t, g)| t.time_to_complete_ms
                    >= MIN_RESPONSE_MS_PER_CHAR * g.sent.chars().count() as f64),
            "seed {seed}: a copy quicker than anyone types"
        );
        let json = serde_json::to_string(&result).expect("a run serializes");
        let back: SessionResult = serde_json::from_str(&json).expect("a run reads back");
        assert_eq!(back, result, "seed {seed}: the round trip changed the run");
        let Some(log) = &result.contest else {
            assert!(result.groups.is_empty(), "seed {seed}: groups with no log");
            continue;
        };
        let run = log
            .run(result.groups.len())
            .unwrap_or_else(|| panic!("seed {seed}: the log does not read back"));
        let owned: u32 = run.qsos.iter().map(|qso| qso.g).sum();
        assert_eq!(owned as usize, result.groups.len(), "seed {seed}");
        let limit_ms = u64::from(run.limit_s) * 1_000;
        assert!(run.run_ms <= limit_ms, "seed {seed}: ran past the limit");
        let span = started.map_or(0, |started| finished_at - started);
        if run.run_ms < limit_ms {
            assert_eq!(run.run_ms + run.paused_ms, span, "seed {seed}");
        } else {
            assert!(run.run_ms + run.paused_ms <= span, "seed {seed}");
        }
        let mut last = 0;
        let mut logged = 0;
        for qso in &run.qsos {
            assert!(qso.t >= last, "seed {seed}: QSO times went back");
            assert!(
                qso.t <= run.run_ms && qso.h <= qso.t,
                "seed {seed}: {qso:?}"
            );
            last = qso.t;
            if qso.k == "lost" {
                assert_eq!(qso.nr, logged + 1, "seed {seed}: a lost QSO's serial");
                outcomes.lost += 1;
            } else {
                logged += 1;
                assert_eq!(qso.nr, logged, "seed {seed}: serials run 1, 2, 3…");
            }
        }
        assert_eq!(logged, view.qsos, "seed {seed}");
        let checks: Vec<ScoredQso<'_>> = view
            .log
            .iter()
            .filter_map(|row| match &row.status {
                RowStatus::Checked(check) => Some(ScoredQso {
                    at_ms: row.at_ms,
                    call: &row.call,
                    check,
                }),
                RowStatus::Pending => None,
            })
            .collect();
        for scored in &checks {
            let errors = &scored.check.errors;
            if errors.contains(&QsoError::Nil) {
                outcomes.nil += 1;
            } else if scored.check.is_clean() {
                outcomes.clean += 1;
            }
            if errors
                .iter()
                .any(|error| matches!(error, QsoError::Call { .. }))
            {
                outcomes.busted += 1;
            }
            if errors
                .iter()
                .any(|error| matches!(error, QsoError::Field(_)))
            {
                outcomes.field += 1;
            }
        }
        let scoring = ContestKind::Wpx
            .rules()
            .and_then(|rules| rules.scoring)
            .expect("WPX scores");
        let live = contest_score(&scoring, &checks, run.run_ms);
        let stored = run.score(&result.groups).expect("a stored WPX run scores");
        assert_eq!(
            stored, live,
            "seed {seed}: the stored run scores differently"
        );
        assert!(stored.verified.score <= stored.raw.score, "seed {seed}");

        // The same events, replayed, make the same run.
        let replays: Vec<_> = (0..2)
            .map(|_| {
                let replay = contest_run(seed, &settings, Some(&events));
                let result = replay.machine.result(finished_at, "2026-10-20".into());
                (replay.effects, result)
            })
            .collect();
        assert_eq!(replays[0], replays[1], "seed {seed}: a replay differed");
        assert_eq!(
            replays[0].1, result,
            "seed {seed}: the replay is another run"
        );
    }
    // Every kind of QSO came up, many times over.
    let Outcomes {
        clean,
        busted,
        field,
        nil,
        lost,
    } = outcomes;
    for (kind, count) in [
        ("clean", clean),
        ("busted", busted),
        ("field error", field),
        ("NIL", nil),
        ("lost", lost),
    ] {
        assert!(count >= 100, "only {count} {kind} QSOs across the seeds");
    }
}
