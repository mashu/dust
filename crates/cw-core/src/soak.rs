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
use crate::contest::{FLUTTER_DEPTH, FLUTTER_RATE_HZ, QRS_MOST};
use crate::level::LEVEL_MIN;
use crate::machine::{GroupSource, SessionEffect, SessionEvent, SessionMachine};
use crate::session::{MIN_RESPONSE_MS_PER_CHAR, whole_ms};
use crate::timing::{SEND_RELEASE_SEC, resolve_pileup};
use crate::{
    Aim, Answered, AutoLevelCounters, CALLSIGN_TIER_MAX, CallerHabits, CharSamplingState,
    CharSetMode, ContestEffect, ContestEvent, ContestKind, ContestMachine, ContestPhase, CopyTally,
    FILTER_WIDTH_MAX, FILTER_WIDTH_MIN, Fields, MonoMs, OpKey, PILEUP_LEVEL_MAX_DB,
    PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN, PauseWhy, QsoError, Rng, RowStatus,
    RunStyle, STATIONS_MAX, STATIONS_MIN, Sampler, ScoredQso, Sender, SessionId, SessionPhase,
    SessionResult, SplitMix64, TrainingSettings, Transmission, WPM_MIN, align_group,
    compute_char_pool, contest_score, evaluate_auto_level, generate_training_group, parse_exchange,
    plan_morse_playback_for, plan_transmission, plan_turn, read_number, resolve_station,
};

/// Enough seeds to be worth running on every commit, and cheap enough to.
/// Turn it up when hunting: the failures name the seed they came from, so a
/// deep run reproduces exactly on a short one.
const SEEDS: u64 = 2_000;

/// The contest soak's seeds of single callers, half as many again as
/// [`SEEDS`]: the runs are shared among the contests, and a Sprint's
/// exchange, three fields long, is put wrong by a slip or junk more often
/// than a lone number, so fewer of its QSOs reach the log with a slip in the
/// call, and the floor of each kind needs more runs to stand on.
const CONTEST_SEEDS: u64 = SEEDS + SEEDS / 2;

/// The contest soak's pile-up seeds, half as many again as
/// [`CONTEST_SEEDS`]: a caller that hears you go to someone else drops out
/// of its QSO, and Enter sends the caller you go on to your exchange rather
/// than a TU that logs it at once, so fewer of a pile-up's QSOs reach the
/// log with a slip in the call.
const PILEUP_SEEDS: u64 = CONTEST_SEEDS + CONTEST_SEEDS / 2;

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

/// How often, at least, the contest soak hears what the callers' habits
/// bring, in each run style, under rough habits; real ones bring the
/// numbers cut and `ENN` alone ([`HabitsHeard::floors`]). The rarer come
/// seldom by design: `ENN` in one message in twenty that sends a report
/// nobody copies, a fumble once from one careless caller in ten, in the two
/// contests with serials, and an early exchange from one operator in twenty
/// whose call you sent wrong or asked for with a partial.
const HABIT_FLOORS: HabitsHeard = HabitsHeard {
    cut: 200,
    enn: 15,
    fumbled: 10,
    early: 10,
    qrm: 150,
};

/// Settings from anywhere, in Contest mode, of run `style`: any contest, a
/// call from a few that read in different ways and are on either side of
/// ARRL DX, none among them, a name or none, a state or none, Labrador among
/// them, a zone or none, any power, a Field Day class and a section or none,
/// Yukon's among them, callers of any habits, and runs short enough to
/// reach their end.
fn contest_settings(rng: &mut SplitMix64, style: RunStyle) -> TrainingSettings {
    const CALLS: [&str; 8] = [
        "DL1ABC",
        "K1ABC",
        "VP2E/DL1ABC",
        "JA1XYZ",
        "W1AW/4",
        "VO2AB",
        "KH6XYZ",
        "",
    ];
    let mut s = wild_settings(rng);
    s.curriculum.char_set_mode = CharSetMode::Contest;
    s.contest.contest_style = style;
    let kinds = ContestKind::ALL;
    s.contest.contest_kind = kinds[rng.usize_in(0, kinds.len() - 1)];
    s.contest.my_call = CALLS[rng.usize_in(0, CALLS.len() - 1)].to_string();
    s.contest.my_name = ["MAT", ""][rng.usize_in(0, 1)].to_string();
    s.contest.my_state = ["MA", "", "DX", "LB"][rng.usize_in(0, 3)].to_string();
    s.contest.my_zone = [0, 0, 14, 40][rng.usize_in(0, 3)];
    s.contest.my_power = ["100", "KW", "5", "1K"][rng.usize_in(0, 3)].to_string();
    s.contest.my_class = ["1D", "3A", "12F"][rng.usize_in(0, 2)].to_string();
    s.contest.my_section = ["EMA", "", "DX", "YT"][rng.usize_in(0, 3)].to_string();
    s.contest.contest_minutes = [1, 2, 3, 4, 0][rng.usize_in(0, 4)];
    s.contest.contest_habits = CallerHabits::ALL[rng.usize_in(0, CallerHabits::ALL.len() - 1)];
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
    let (call, heard) = (seen.call.as_str(), seen.exch.as_str());
    let draw = rng.f64();
    let (call, exch) = if draw < 0.1 {
        (seen.previous.clone(), heard.to_string())
    } else if draw < 0.4 {
        (call.to_string(), heard.to_string())
    } else if draw < 0.6 {
        (one_edit_off(call, rng), one_edit_off(heard, rng))
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
        earlier: Vec::new(),
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

impl Outcomes {
    /// Each kind of QSO, and how many came up.
    fn counts(&self) -> [(&'static str, u32); 5] {
        [
            ("clean", self.clean),
            ("busted", self.busted),
            ("field error", self.field),
            ("NIL", self.nil),
            ("lost", self.lost),
        ]
    }
}

/// What a contest soak run has seen of the machine's effects: every id
/// handed out, the turns and timers among them, the last call and exchange
/// a listener would have copied from the callers, and the call before it.
#[derive(Debug, Default)]
struct Seen {
    ids: std::collections::BTreeSet<u64>,
    turns: Vec<u64>,
    timers: Vec<u64>,
    call: String,
    exch: String,
    previous: String,
    heard: HabitsHeard,
}

/// What a contest soak run has heard of its callers' habits: numbers sent
/// cut, reports sent as `ENN`, serials fumbled and put right, exchanges
/// sent early with a call put right, and messages of QRM.
#[derive(Debug, Default, Clone, Copy)]
struct HabitsHeard {
    cut: u32,
    enn: u32,
    fumbled: u32,
    early: u32,
    qrm: u32,
}

impl HabitsHeard {
    fn add(&mut self, other: Self) {
        self.cut += other.cut;
        self.enn += other.enn;
        self.fumbled += other.fumbled;
        self.early += other.early;
        self.qrm += other.qrm;
    }

    /// How much of each the soak hears at least under `habits`: nothing
    /// under easy ones, numbers cut and `ENN` under real ones, and all of
    /// [`HABIT_FLOORS`] under rough ones.
    fn floors(habits: CallerHabits) -> Self {
        match habits {
            CallerHabits::Easy => Self::default(),
            CallerHabits::Real => Self {
                cut: HABIT_FLOORS.cut,
                enn: HABIT_FLOORS.enn,
                ..Self::default()
            },
            CallerHabits::Rough => HABIT_FLOORS,
        }
    }

    /// Each count, with what it counts.
    fn counts(self) -> [(&'static str, u32); 5] {
        [
            ("cut numbers", self.cut),
            ("ENN", self.enn),
            ("fumbled serials", self.fumbled),
            ("early exchanges", self.early),
            ("QRM messages", self.qrm),
        ]
    }
}

/// Every effect a contest run hands back has to be one the runtime can act
/// on: an id never handed out before, a timer that runs, a turn with
/// something in it, no station sending over itself and no reply past your
/// message; and in a pile-up, its callers sixty hertz apart or more. What
/// the callers' habits bring is tallied, `machine` as the effects left it
/// telling an exchange sent early.
fn check_contest_effects(
    effects: &[ContestEffect],
    machine: &ContestMachine<Sampler>,
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
                // Without full break-in the receiver is muted from the
                // turn's start to the end of your message; with it, never.
                let muted = turn
                    .your_end_sec()
                    .filter(|_| !settings.contest.contest_qsk);
                assert_eq!(planned.muted_until, muted, "seed {seed} step {step}");
                for other in &planned.others {
                    assert!(
                        other.duration_sec <= planned.wanted.duration_sec + 1e-9,
                        "seed {seed} step {step}: a reply ends after your message"
                    );
                }
                let callers: Vec<_> = turn
                    .segments
                    .iter()
                    .filter(|segment| segment.who != Sender::You)
                    .collect();
                for (at, caller) in callers.iter().enumerate() {
                    for other in &callers[..at] {
                        // A caller keyed over sends again only once its rest
                        // is done, and so does QRM.
                        let apart_in_time =
                            caller.start_sec >= other.end_sec || other.start_sec >= caller.end_sec;
                        assert!(
                            caller.who != other.who || apart_in_time,
                            "seed {seed} step {step}: a station sends over itself"
                        );
                        // QRM may sit anywhere the filter passes it.
                        if caller.who != other.who
                            && caller.who.is_caller()
                            && other.who.is_caller()
                            && settings.contest.contest_style == RunStyle::Pileup
                        {
                            let apart = (caller.voice.tone_hz - other.voice.tone_hz).abs();
                            assert!(
                                apart >= 60.0 - 1e-6,
                                "seed {seed} step {step}: two callers {apart:.1} Hz apart"
                            );
                        }
                    }
                }
                let rough = settings.contest.contest_habits == CallerHabits::Rough;
                let easy = settings.contest.contest_habits == CallerHabits::Easy;
                for segment in &turn.segments {
                    if segment.who == Sender::Qrm {
                        // QRM comes only with rough habits, and is heard by
                        // nobody as a caller.
                        assert!(rough, "seed {seed} step {step}: QRM under easier habits");
                        seen.heard.qrm += 1;
                        continue;
                    }
                    if segment.who == Sender::You {
                        assert_eq!(segment.fade, None, "seed {seed} step {step}: yours faded");
                        continue;
                    }
                    // Some callers flutter, each within flutter's rates and
                    // depths, under the Band card's flutter alone.
                    if let Some(flutter) = segment.fade.and_then(|fade| fade.flutter) {
                        let (slowest, fastest) = FLUTTER_RATE_HZ;
                        let (shallowest, deepest) = FLUTTER_DEPTH;
                        assert!(
                            settings.contest.contest_flutter
                                && (slowest..=fastest).contains(&flutter.rate_hz())
                                && (shallowest - 1e-9..=deepest + 1e-9).contains(&flutter.depth()),
                            "seed {seed} step {step}: a flutter of {:.1} Hz, {:.2} deep",
                            flutter.rate_hz(),
                            flutter.depth()
                        );
                    }
                    // A caller sends within the Speed card's ranges, drawn
                    // evenly or from your copy: its spacing within its own
                    // wherever its characters are as fast as its slowest,
                    // and never outrunning them. It is slower only as it
                    // slows down for you under easy habits, by QRS_MOST at
                    // most, and never under the card's slowest speed.
                    let (voice, p) = (segment.voice, &settings.playback);
                    let slack = if easy { 1.0 - QRS_MOST } else { 1.0 };
                    let floor = |wpm: f64| (wpm * slack).max(WPM_MIN).min(wpm);
                    assert!(
                        voice.char_wpm <= p.char_wpm_max + 1e-9
                            && voice.char_wpm >= floor(p.char_wpm_min) - 1e-9
                            && voice.effective_wpm
                                <= voice.char_wpm.min(p.effective_wpm_max) + 1e-9
                            && voice.effective_wpm
                                >= floor(p.effective_wpm_min).min(voice.char_wpm) - 1e-9,
                        "seed {seed} step {step}: a caller at {:.2}/{:.2} WPM, the ranges \
                         {}–{} and {}–{}",
                        voice.char_wpm,
                        voice.effective_wpm,
                        p.char_wpm_min,
                        p.char_wpm_max,
                        p.effective_wpm_min,
                        p.effective_wpm_max
                    );
                    let words: Vec<&str> = segment.text.split_whitespace().collect();
                    let fumbled = words.iter().any(|word| word.ends_with("EEEEE"));
                    let enn = words.contains(&"ENN");
                    assert!(
                        !fumbled || rough,
                        "seed {seed} step {step}: a fumbled serial under easier habits: {}",
                        segment.text
                    );
                    assert!(
                        !enn || !easy,
                        "seed {seed} step {step}: ENN under easy habits: {}",
                        segment.text
                    );
                    seen.heard.fumbled += u32::from(fumbled);
                    seen.heard.enn += u32::from(enn);
                    let (call, exch) = match copy_of(&segment.text) {
                        Some(Copied::Call(call)) => (Some(call), None),
                        Some(Copied::Correction(call, exch)) => {
                            // A correction carrying an exchange from a station
                            // still waiting for yours: a careless operator's,
                            // under rough habits alone.
                            let early = exch.is_some() && machine.sends_exchange_early(segment.who);
                            assert!(
                                !early || rough,
                                "seed {seed} step {step}: an early exchange under easier habits"
                            );
                            seen.heard.early += u32::from(early);
                            (Some(call), exch)
                        }
                        Some(Copied::Exchange(exch)) => (None, Some(exch)),
                        None => (None, None),
                    };
                    if let Some(call) = call
                        && seen.call != call
                    {
                        seen.previous = std::mem::replace(&mut seen.call, call.to_string());
                    }
                    if let Some(exch) = exch {
                        // Callers with easy habits send their numbers in
                        // plain digits: a report is all that goes cut.
                        let rules = settings.contest.contest_kind.rules();
                        let reading = parse_exchange(rules, settings.contest.side(), &exch);
                        // A kilowatt is written with a letter, and is no
                        // number cut.
                        let cut = reading.fields.iter().any(|read| {
                            read.field.numeric()
                                && read.token.chars().any(|c| c.is_ascii_alphabetic())
                                && read_number(&read.token).is_some()
                        });
                        assert!(
                            !cut || !easy,
                            "seed {seed} step {step}: a number sent cut under easy habits: {exch}"
                        );
                        seen.heard.cut += u32::from(cut);
                        seen.exch = exch;
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
    heard: HabitsHeard,
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
        check_contest_effects(&effects, &machine, settings, seed, step, &mut seen);
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
        let lost = machine.nil_and_lost_copies();
        effects = machine.apply(event.clone(), at);
        // A continuation takes over from the turn on the air where that
        // one leaves off: never so soon after the key that the turn on the
        // air could not be ridden down first.
        for effect in &effects {
            if let ContestEffect::PlayTurn(turn) = effect {
                assert!(
                    turn.starts_at
                        .is_none_or(|starts| starts >= at.plus(whole_ms(SEND_RELEASE_SEC))),
                    "seed {seed} step {step}: a turn to start too soon after its key"
                );
            }
        }
        // A station you left for another, or whose QSO ended with another
        // station's call in the Call field, keeps the call you had for it,
        // and a NIL logged for a station you left is no copy of the one you
        // work: no copy lost or charged now is a call another station took
        // from you.
        let crossed = machine.crossed_since(&lost);
        assert!(
            crossed.is_empty(),
            "seed {seed} step {step}: lost or NIL copies that are another station's call: {crossed:?}"
        );
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
        let view = machine.view();
        if let Some(mut clock) = view.clock {
            let reading = clock.elapsed_ms(at);
            // The one step back by design: a send whose every retry failed
            // gives its retries back as time paused, whether that pauses
            // the run or, past its limit all the same, ends it.
            let failed = matches!(event, ContestEvent::TurnFailed { .. })
                && matches!(
                    view.phase,
                    ContestPhase::Paused {
                        why: PauseWhy::Sound
                    } | ContestPhase::Finished
                );
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
        heard: seen.heard,
    }
}

/// Contest runs driven by a stream of anything the runtime and a careless
/// operator can send (§13 of the design): keys with fields as heard, a slip
/// off, empty, partial or junk; turn news for the turn on the air and for
/// stale ones; timers by any id; pauses, resumes, hidden pages and early
/// ends. Whatever happens, the run stays whole and keeps what it should, a
/// run of single callers and a pile-up alike.
#[test]
fn no_run_of_events_can_put_a_contest_run_somewhere_impossible() {
    for style in RunStyle::ALL {
        soak_contest_runs(style);
    }
}

/// The contest soak, every seed a run of `style`: single callers on the
/// first [`CONTEST_SEEDS`] seeds, pile-ups on the [`PILEUP_SEEDS`] after
/// them, so a failure's seed says which.
fn soak_contest_runs(style: RunStyle) {
    let mut outcomes = Outcomes::default();
    // QSOs logged in each contest, so every one is seen to be soaked, and
    // under each of the callers' habits, with what was heard of them.
    let mut logged_in: std::collections::BTreeMap<&str, u32> = Default::default();
    let mut logged_with: std::collections::BTreeMap<&str, u32> = Default::default();
    let mut heard_with: std::collections::BTreeMap<&str, HabitsHeard> = Default::default();
    let seeds = match style {
        RunStyle::Single => 0..CONTEST_SEEDS,
        RunStyle::Pileup => CONTEST_SEEDS..CONTEST_SEEDS + PILEUP_SEEDS,
    };
    for seed in seeds {
        let mut rng = SplitMix64::new(seed.wrapping_mul(0x2F7A_9C35).wrapping_add(37));
        let mut settings = contest_settings(&mut rng, style);
        // Show copy on every third run, set by the seed so that every other
        // draw is as it was: it changes only the copies the groups keep. The
        // callers' speed follows your copy on every other run, set so too.
        settings.contest.contest_show_copy = seed % 3 == 0;
        settings.contest.contest_adapt_speed = seed % 2 == 0;
        // Full break-in on half the runs, so callers sending across your
        // message are heard on some and muted on the rest; flutter on two
        // in five, drawn from streams of their own as it is.
        settings.contest.contest_qsk = seed % 4 < 2;
        settings.contest.contest_flutter = seed % 5 < 2;
        let SoakRun {
            mut events,
            mut machine,
            started,
            ended,
            heard,
            ..
        } = contest_run(seed, &settings, None);
        let with = settings.contest.contest_habits.name();
        heard_with.entry(with).or_default().add(heard);
        let finished_at = ended.unwrap_or_else(|| {
            let end = events.last().map_or(0, |(_, at)| at.0) + 1;
            let lost = machine.nil_and_lost_copies();
            let _ = machine.apply(ContestEvent::FinishNow, MonoMs(end));
            let crossed = machine.crossed_since(&lost);
            assert!(
                crossed.is_empty(),
                "seed {seed}: lost at the end, copies that are another station's call: {crossed:?}"
            );
            events.push((ContestEvent::FinishNow, MonoMs(end)));
            end
        });
        let view = machine.view();
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
        let rules = settings.contest.contest_kind.rules();
        assert_eq!(run.kind, rules.kind.name(), "seed {seed}");
        // A run with Show copy on is marked assisted, by its log and its
        // mark alike.
        assert_eq!(
            run.assisted, settings.contest.contest_show_copy,
            "seed {seed}"
        );
        assert_eq!(result.assisted(), run.assisted, "seed {seed}");
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
            // A contest without serials sends none.
            let serial = |n: u32| if rules.serial { n } else { 0 };
            if qso.k == "lost" {
                assert_eq!(
                    qso.nr,
                    serial(logged + 1),
                    "seed {seed}: a lost QSO's serial"
                );
                outcomes.lost += 1;
            } else {
                logged += 1;
                assert_eq!(qso.nr, serial(logged), "seed {seed}: serials run 1, 2, 3…");
            }
        }
        assert_eq!(logged, view.qsos, "seed {seed}");
        *logged_in.entry(rules.kind.name()).or_default() += logged;
        *logged_with.entry(with).or_default() += logged;
        let checks: Vec<ScoredQso<'_>> = view
            .log
            .iter()
            .filter_map(|row| match &row.status {
                RowStatus::Checked(check) => Some(ScoredQso {
                    at_ms: row.at_ms,
                    call: &row.call,
                    fields: &row.fields,
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
        match rules.scoring {
            Some(scoring) => {
                let live = contest_score(&scoring, &run.me(), &checks, run.run_ms);
                let stored = run.score(&result.groups).expect("a stored run scores");
                assert_eq!(
                    stored, live,
                    "seed {seed}: the stored run scores differently"
                );
                assert!(stored.verified.score <= stored.raw.score, "seed {seed}");
            }
            None => assert_eq!(run.score(&result.groups), None, "seed {seed}"),
        }
        // Its results list each QSO logged as the run's own log did.
        let listed = run
            .log(&result.groups, &result.group_timings)
            .expect("a stored run lists its QSOs");
        let listed: Vec<_> = listed.iter().filter(|qso| qso.n.is_some()).collect();
        assert_eq!(listed.len(), view.log.len(), "seed {seed}");
        for (row, qso) in view.log.iter().zip(listed) {
            assert_eq!(
                (&qso.call, qso.at_ms, qso.wpm, qso.asked),
                (&row.call, row.at_ms, row.wpm, row.asked),
                "seed {seed}: QSO {} lists otherwise than it ran",
                row.n
            );
            assert_eq!(
                Some(&row.status),
                qso.check.clone().map(RowStatus::Checked).as_ref(),
                "seed {seed}: QSO {} checks otherwise than it ran",
                row.n
            );
        }

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
    // Every kind of QSO came up, many times over, and every contest.
    for (kind, count) in outcomes.counts() {
        assert!(
            count >= 100,
            "{style:?}: only {count} {kind} QSOs across the seeds"
        );
    }
    for kind in ContestKind::ALL {
        let count = logged_in.get(kind.name()).copied().unwrap_or(0);
        assert!(
            count >= 100,
            "{style:?}: only {count} QSOs logged in {} across the seeds",
            kind.label()
        );
    }
    // Every habit of the callers was soaked, and what each brings came up:
    // numbers cut and `ENN` under real and rough habits, and under rough
    // ones serials fumbled and put right, exchanges sent early, and QRM.
    for habits in CallerHabits::ALL {
        let name = habits.name();
        let count = logged_with.get(name).copied().unwrap_or(0);
        assert!(
            count >= 100,
            "{style:?}: only {count} QSOs logged under {name} habits"
        );
        let heard = heard_with.get(name).copied().unwrap_or_default();
        let floors = HabitsHeard::floors(habits).counts();
        for ((what, count), (_, floor)) in heard.counts().into_iter().zip(floors) {
            assert!(
                count >= floor,
                "{style:?}: only {count} {what} under {name} habits"
            );
        }
    }
}
