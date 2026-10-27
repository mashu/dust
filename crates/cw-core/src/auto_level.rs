//! Automatic level adjustment after a session.

use crate::session::{GroupSession, SessionResult};
use crate::settings::{
    ADJUST_THRESHOLD_MAX_PCT, ADJUST_THRESHOLD_MIN_PCT, CharSetMode, LevelAxis, MixedAutoLevelAxis,
    TrainingSettings,
};

/// Where one run of auto-level counters is kept.
///
/// Every level of every alphabet counts its own sessions, so a run earned at
/// one never moves another. The text is the storage key, and older saves
/// hold their counters under it, so its shape does not change.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CountersKey(String);

impl CountersKey {
    /// The counters for the settings as they are now.
    pub fn of(settings: &TrainingSettings) -> Self {
        let fingerprint = settings.alphabet_fingerprint();
        let level = settings.active_level();
        Self(match settings.curriculum.char_set_mode {
            CharSetMode::Mixed => format!(
                "mixed_{level}_{}_{fingerprint}",
                settings.level(LevelAxis::Digits)
            ),
            CharSetMode::Digits => format!("digits_{level}_{fingerprint}"),
            // A contest run never reaches auto level; its tier is the
            // callsign one, so its key is too.
            CharSetMode::Callsign | CharSetMode::Contest => format!("callsign_{level}"),
            CharSetMode::Words => format!("words_{level}"),
            CharSetMode::Koch => format!("koch_{level}_{fingerprint}"),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutoLevelCounters {
    pub above: u32,
    pub below: u32,
}

/// A level the trainer moved after a session, and the evidence it moved on.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoLevelChange {
    pub axis: LevelAxis,
    pub from: u32,
    pub to: u32,
    /// The session's accuracy, and the threshold it was held to, in percent.
    pub accuracy_pct: f64,
    pub threshold_pct: f64,
    /// Sessions above the threshold for a rise, below it for a fall.
    pub sessions: u32,
    /// In Mixed, the axis that moves next time the evidence does not say.
    pub next_mixed_axis: Option<MixedAutoLevelAxis>,
    /// The runs to forget: the level left and the level arrived at both start
    /// counting afresh.
    pub counters_cleared: [CountersKey; 2],
}

impl AutoLevelChange {
    pub fn raised(&self) -> bool {
        self.to > self.from
    }
}

/// Whether a session was long enough to count: half the groups it set out to
/// send. One fumbled group, or an audio failure after the first, is not a
/// session's worth of evidence either way.
fn counts_toward_level(result: &SessionResult, settings: &TrainingSettings) -> bool {
    let planned = settings.curriculum.num_groups.max(1) as usize;
    result.groups.len() * 2 >= planned
}

/// The axes a change could move, most deserving first.
///
/// Mixed holds its two axes to their own share of the session: a fall moves
/// the axis that fell short and a rise the one that earned it, so perfect
/// letters are never demoted for bad digits. When neither axis says — no
/// characters of it came up, or both cleared the bar the session as a whole
/// missed — it takes turns, starting from the stored next axis.
fn candidate_axes(
    result: &SessionResult,
    settings: &TrainingSettings,
    raise: bool,
    threshold_pct: f64,
) -> Vec<LevelAxis> {
    let mode = settings.curriculum.char_set_mode;
    if mode != CharSetMode::Mixed {
        return vec![mode.axis()];
    }
    let next = settings.auto_level.mixed_auto_level_next_axis;
    let practised: Vec<MixedAutoLevelAxis> = [next, next.flip()]
        .into_iter()
        .filter(|axis| settings.mixed_axis_practised(*axis))
        .collect();
    let earned = |axis: &MixedAutoLevelAxis| {
        let digits = *axis == MixedAutoLevelAxis::Digits;
        result
            .tally
            .accuracy_where(|c| c.is_ascii_digit() == digits)
            .is_some_and(|accuracy| (accuracy * 100.0 >= threshold_pct) == raise)
    };
    let deserving: Vec<MixedAutoLevelAxis> = practised.iter().copied().filter(earned).collect();
    let chosen = if deserving.is_empty() {
        practised
    } else {
        deserving
    };
    chosen.into_iter().map(LevelAxis::from).collect()
}

/// Decide whether the session just finished moves a level. Counts it in
/// `counters`, the run for the level it was at, and returns the change when
/// the run is long enough and there is somewhere to move.
pub fn evaluate_auto_level(
    result: &SessionResult,
    settings: &TrainingSettings,
    counters: &mut AutoLevelCounters,
) -> Option<AutoLevelChange> {
    if !settings.auto_level.auto_adjust_level || !counts_toward_level(result, settings) {
        return None;
    }
    let threshold_pct = settings
        .auto_level
        .auto_adjust_threshold
        .clamp(ADJUST_THRESHOLD_MIN_PCT, ADJUST_THRESHOLD_MAX_PCT);
    let accuracy_pct = result.accuracy * 100.0;
    // Counters come back from storage, so they are not trusted to be small.
    if accuracy_pct >= threshold_pct {
        counters.above = counters.above.saturating_add(1);
    } else {
        counters.below = counters.below.saturating_add(1);
    }

    let above_target = settings.auto_level.auto_adjust_above_threshold_count;
    let below_target = settings.auto_level.auto_adjust_below_threshold_count;
    let rise_due = above_target > 0 && counters.above >= above_target;
    let fall_due = below_target > 0 && counters.below >= below_target;
    let raise = match (rise_due, fall_due) {
        (true, true) => counters.above >= counters.below,
        (true, false) => true,
        (false, true) => false,
        (false, false) => return None,
    };

    let delta = if raise { 1 } else { -1 };
    let moved = candidate_axes(result, settings, raise, threshold_pct)
        .into_iter()
        .find_map(|axis| {
            let from = settings.level(axis);
            let to = step_level(from, delta, axis.min_level(), settings.max_level(axis));
            (to != from).then_some((axis, from, to))
        });
    let Some((axis, from, to)) = moved else {
        // Nowhere to go: the run is spent rather than kept pressing.
        if raise {
            counters.above = 0;
        } else {
            counters.below = 0;
        }
        return None;
    };

    let next_mixed_axis = (settings.curriculum.char_set_mode == CharSetMode::Mixed).then(|| {
        let moved = if axis == LevelAxis::Digits {
            MixedAutoLevelAxis::Digits
        } else {
            MixedAutoLevelAxis::Letters
        };
        // Take turns, but only onto an axis that is being practised.
        if settings.mixed_axis_practised(moved.flip()) {
            moved.flip()
        } else {
            moved
        }
    });
    let mut after = settings.clone();
    after.set_level(axis, to);
    Some(AutoLevelChange {
        axis,
        from,
        to,
        accuracy_pct,
        threshold_pct,
        sessions: if raise {
            counters.above
        } else {
            counters.below
        },
        next_mixed_axis,
        counters_cleared: [CountersKey::of(settings), CountersKey::of(&after)],
    })
}

/// Put a change into the settings.
pub fn apply_auto_level(settings: &mut TrainingSettings, change: &AutoLevelChange) {
    settings.set_level(change.axis, change.to);
    if let Some(axis) = change.next_mixed_axis {
        settings.auto_level.mixed_auto_level_next_axis = axis;
    }
    *settings = std::mem::take(settings).clamp();
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoLevelProgress {
    pub threshold: f64,
    pub above_count: u32,
    pub below_count: u32,
    pub above_target: u32,
    pub below_target: u32,
    pub above_disabled: bool,
    pub below_disabled: bool,
    pub alternating_mixed: bool,
    pub next_mixed_axis: Option<MixedAutoLevelAxis>,
}

pub fn auto_level_progress(
    settings: &TrainingSettings,
    counters: AutoLevelCounters,
) -> Option<AutoLevelProgress> {
    // Off, or a word list, which has no level to move, or a contest, whose
    // runs never move one.
    if !settings.auto_level.auto_adjust_level
        || matches!(
            settings.curriculum.char_set_mode,
            CharSetMode::Words | CharSetMode::Contest
        )
    {
        return None;
    }
    let mixed = settings.curriculum.char_set_mode == CharSetMode::Mixed;
    // The axis that moves next, as far as one is being practised.
    let next_mixed_axis = mixed.then(|| {
        let stored = settings.auto_level.mixed_auto_level_next_axis;
        if !settings.mixed_axis_practised(stored) && settings.mixed_axis_practised(stored.flip()) {
            stored.flip()
        } else {
            stored
        }
    });
    let above_target = settings.auto_level.auto_adjust_above_threshold_count;
    let below_target = settings.auto_level.auto_adjust_below_threshold_count;
    Some(AutoLevelProgress {
        threshold: settings
            .auto_level
            .auto_adjust_threshold
            .clamp(ADJUST_THRESHOLD_MIN_PCT, ADJUST_THRESHOLD_MAX_PCT),
        above_count: counters.above,
        below_count: counters.below,
        above_target,
        below_target,
        above_disabled: above_target == 0,
        below_disabled: below_target == 0,
        alternating_mixed: mixed,
        next_mixed_axis,
    })
}

/// A session as it ends: scored, and what it does to the level.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub result: SessionResult,
    /// The level the session moved, if its run earned a move.
    pub change: Option<AutoLevelChange>,
    /// The settings to go on with: the session's own, the change made.
    pub settings: TrainingSettings,
    /// What becomes of the auto-level counters.
    pub counters: CountersUpdate,
}

/// What finishing a session does to the stored auto-level counters.
#[derive(Clone, Debug, PartialEq)]
pub enum CountersUpdate {
    /// The run goes on: these are its counters now.
    Keep(CountersKey, AutoLevelCounters),
    /// The level moved: the runs it left and arrived at start afresh.
    Clear([CountersKey; 2]),
}

/// Finish `session`: score it, count it towards its level's run, and move
/// the level if the run has earned it.
///
/// `counters` are the run so far, for the level the session was at.
/// `finished_at` is the wall time it ended at, and `date` the local day it
/// counts towards.
pub fn complete_session(
    session: &GroupSession,
    mut counters: AutoLevelCounters,
    finished_at: u64,
    date: String,
) -> Completion {
    let settings = session.settings();
    let result = session.result(finished_at, date);
    let change = evaluate_auto_level(&result, settings, &mut counters);
    let mut next = settings.clone();
    let counters = match &change {
        Some(change) => {
            apply_auto_level(&mut next, change);
            CountersUpdate::Clear(change.counters_cleared.clone())
        }
        None => CountersUpdate::Keep(CountersKey::of(settings), counters),
    };
    Completion {
        result,
        change,
        settings: next,
        counters,
    }
}

/// Move a level by `delta`, staying within `min..=max`. In unsigned
/// arithmetic throughout: a stored level is not trusted to fit an `i32`, and
/// a round trip through one turned a huge level into a negative number.
///
/// A step only ever moves the way it points. A level already outside its
/// range — a save from a longer alphabet — is left where it is rather than
/// "increased" downwards by the clamp.
fn step_level(level: u32, delta: i32, min: u32, max: u32) -> u32 {
    let stepped = level.saturating_add_signed(delta).clamp(min, max.max(min));
    if (delta > 0 && stepped < level) || (delta < 0 && stepped > level) {
        level
    } else {
        stepped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alignment::CopyTally;
    use crate::callsign::{CALLSIGN_TIER_MAX, CALLSIGN_TIER_MIN};
    use crate::session::GroupResult;

    /// A finished session of `groups` groups at `accuracy`, with no
    /// character-by-character tally to say which axis earned it.
    fn session(accuracy: f64, groups: usize) -> SessionResult {
        let mut result = SessionResult::empty_for_test();
        result.accuracy = accuracy;
        result.groups = vec![
            GroupResult {
                sent: "KM".into(),
                received: "KM".into(),
                correct: true,
            };
            groups
        ];
        result
    }

    /// A full session — as many groups as the default settings plan.
    fn after(accuracy: f64) -> SessionResult {
        session(accuracy, 20)
    }

    /// A full session copied as `copies`, sent and received pairs.
    fn copied(copies: &[(&str, &str)]) -> SessionResult {
        let mut tally = CopyTally::default();
        for (sent, received) in copies {
            tally.add(sent, received);
        }
        let mut result = after(tally.accuracy());
        result.tally = tally;
        result
    }

    fn koch(level: u32) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = level;
        settings.auto_level.auto_adjust_above_threshold_count = 1;
        settings.auto_level.auto_adjust_below_threshold_count = 1;
        settings
    }

    fn mixed(letters_pct: u32) -> TrainingSettings {
        let mut s = koch(5);
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        s.curriculum.mixed_letters_percent = letters_pct;
        s.curriculum.digits_level = 5;
        s
    }

    #[test]
    fn a_level_steps_only_the_way_it_points() {
        assert_eq!(step_level(5, 1, 1, 10), 6);
        assert_eq!(step_level(5, -1, 1, 10), 4);
        assert_eq!(step_level(10, 1, 1, 10), 10, "already at the top");
        assert_eq!(step_level(1, -1, 1, 10), 1, "already at the bottom");
        assert_eq!(
            step_level(50, 1, 1, 10),
            50,
            "an increase never lowers a level"
        );
        assert_eq!(step_level(0, -1, 1, 10), 0, "a decrease never raises one");
        assert_eq!(step_level(u32::MAX, 1, 1, 10), u32::MAX);
        assert_eq!(step_level(3, i32::MIN, 1, 10), 1);
    }

    #[test]
    fn a_good_run_raises_the_level_and_a_bad_one_lowers_it() {
        let mut settings = koch(1);
        settings.auto_level.auto_adjust_above_threshold_count = 5;
        let mut counters = AutoLevelCounters { above: 4, below: 0 };
        let change = evaluate_auto_level(&after(1.0), &settings, &mut counters).expect("up");
        assert_eq!(
            (change.axis, change.from, change.to),
            (LevelAxis::Letters, 1, 2)
        );
        assert!(change.raised());
        assert_eq!(change.sessions, 5);

        let settings = koch(3);
        let mut counters = AutoLevelCounters::default();
        let change = evaluate_auto_level(&after(0.5), &settings, &mut counters).expect("down");
        assert_eq!((change.from, change.to), (3, 2));
        assert!(!change.raised());
        assert_eq!(change.accuracy_pct, 50.0);
        assert_eq!(change.threshold_pct, 90.0);
    }

    /// One fumbled group and then Finish, or an audio failure after the
    /// first group, is not a session: it used to demote the learner.
    #[test]
    fn a_session_cut_short_does_not_count() {
        let settings = koch(10);
        let mut counters = AutoLevelCounters::default();
        assert!(evaluate_auto_level(&session(0.67, 1), &settings, &mut counters).is_none());
        assert!(evaluate_auto_level(&session(1.0, 9), &settings, &mut counters).is_none());
        assert_eq!(
            counters,
            AutoLevelCounters::default(),
            "and nothing is counted"
        );
        // Half the planned groups is enough.
        let change = evaluate_auto_level(&session(0.67, 10), &settings, &mut counters);
        assert_eq!(change.map(|c| c.to), Some(9));
    }

    #[test]
    fn nothing_moves_while_auto_level_is_off() {
        let mut s = TrainingSettings::default();
        s.auto_level.auto_adjust_level = false;
        let mut counters = AutoLevelCounters::default();
        assert!(evaluate_auto_level(&after(1.0), &s, &mut counters).is_none());
        assert_eq!(counters, AutoLevelCounters::default());
        assert!(auto_level_progress(&s, counters).is_none());
    }

    #[test]
    fn a_session_only_counts_until_the_run_is_long_enough() {
        let mut s = koch(1);
        s.auto_level.auto_adjust_above_threshold_count = 3;
        s.auto_level.auto_adjust_below_threshold_count = 3;
        let mut counters = AutoLevelCounters::default();
        assert!(evaluate_auto_level(&after(1.0), &s, &mut counters).is_none());
        assert_eq!(counters.above, 1);
        assert!(evaluate_auto_level(&after(1.0), &s, &mut counters).is_none());
        let change = evaluate_auto_level(&after(1.0), &s, &mut counters).expect("level up");
        assert_eq!(change.to, 2);
        assert_eq!(change.sessions, 3);
        assert_ne!(change.counters_cleared[0], change.counters_cleared[1]);
    }

    #[test]
    fn a_zero_target_switches_that_direction_off() {
        let mut s = koch(5);
        s.auto_level.auto_adjust_above_threshold_count = 0;
        s.auto_level.auto_adjust_below_threshold_count = 0;
        let mut counters = AutoLevelCounters::default();
        assert!(evaluate_auto_level(&after(1.0), &s, &mut counters).is_none());
        assert!(evaluate_auto_level(&after(0.0), &s, &mut counters).is_none());
        assert_eq!((counters.above, counters.below), (1, 1));
        let progress = auto_level_progress(&s, counters).expect("progress");
        assert!(progress.above_disabled && progress.below_disabled);
    }

    #[test]
    fn when_both_runs_are_due_the_longer_one_wins() {
        let s = koch(5);
        let mut up = AutoLevelCounters { above: 4, below: 2 };
        let change = evaluate_auto_level(&after(1.0), &s, &mut up);
        assert_eq!(change.map(|c| c.raised()), Some(true));

        let mut down = AutoLevelCounters { above: 1, below: 9 };
        let change = evaluate_auto_level(&after(1.0), &s, &mut down);
        assert_eq!(change.map(|c| c.raised()), Some(false));
    }

    #[test]
    fn a_level_already_at_the_end_of_the_alphabet_clears_its_run() {
        let mut s = koch(1);
        s.curriculum.level = s.max_level(LevelAxis::Letters);
        let mut counters = AutoLevelCounters { above: 5, below: 0 };
        assert!(evaluate_auto_level(&after(1.0), &s, &mut counters).is_none());
        assert_eq!(counters.above, 0);

        // Same at the bottom.
        s.curriculum.level = 1;
        let mut counters = AutoLevelCounters { above: 0, below: 5 };
        assert!(evaluate_auto_level(&after(0.0), &s, &mut counters).is_none());
        assert_eq!(counters.below, 0);

        // And on an alphabet of two characters, which only has level 1.
        s.curriculum.custom_sequence = vec!['K', 'M'];
        let mut counters = AutoLevelCounters::default();
        assert!(evaluate_auto_level(&after(1.0), &s, &mut counters).is_none());
        assert_eq!(counters, AutoLevelCounters::default());
    }

    #[test]
    fn each_mode_moves_its_own_level() {
        let mut own = koch(1);
        own.curriculum.custom_sequence = vec!['Q', 'R', 'S', 'T'];
        own.curriculum.sequence_is_custom = true;
        let change = evaluate_auto_level(&after(1.0), &own, &mut Default::default());
        apply_auto_level(&mut own, &change.expect("up"));
        assert_eq!(own.curriculum.level, 2);

        let mut digits = koch(1);
        digits.curriculum.char_set_mode = CharSetMode::Digits;
        digits.curriculum.digits_level = 3;
        let change =
            evaluate_auto_level(&after(1.0), &digits, &mut Default::default()).expect("up");
        assert_eq!(change.axis, LevelAxis::Digits);
        assert!(change.counters_cleared[0].as_str().starts_with("digits_3_"));
        assert!(change.counters_cleared[1].as_str().starts_with("digits_4_"));
        apply_auto_level(&mut digits, &change);
        assert_eq!(digits.curriculum.digits_level, 4);
        assert_eq!(digits.curriculum.level, 1);
    }

    /// A word list has no level to climb: the best run there is moves
    /// nothing, the worst neither, no other level is touched, and there is
    /// no run to show.
    #[test]
    fn a_word_run_moves_no_level() {
        let mut settings = koch(12);
        settings.curriculum.char_set_mode = CharSetMode::Words;
        for accuracy in [1.0, 0.0] {
            let mut counters = Default::default();
            for _ in 0..5 {
                assert!(evaluate_auto_level(&after(accuracy), &settings, &mut counters).is_none());
            }
        }
        assert_eq!(settings.curriculum.level, 12);
        assert!(auto_level_progress(&settings, Default::default()).is_none());
    }

    /// A contest's runs never move a level, so Home shows no progress
    /// towards one, whatever the switch says.
    #[test]
    fn a_contest_shows_no_level_progress() {
        let mut settings = koch(12);
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        assert!(auto_level_progress(&settings, Default::default()).is_some());
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        assert!(settings.auto_level.auto_adjust_level);
        assert!(auto_level_progress(&settings, Default::default()).is_none());
    }

    #[test]
    fn a_callsign_run_moves_the_tier_and_leaves_the_koch_level_alone() {
        let mut settings = koch(30);
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level = 3;
        let change = evaluate_auto_level(&after(0.98), &settings, &mut Default::default())
            .expect("a tier change");
        assert_eq!((change.axis, change.to), (LevelAxis::Callsign, 4));
        apply_auto_level(&mut settings, &change);
        assert_eq!(settings.curriculum.callsign_level, 4);
        assert_eq!(settings.curriculum.level, 30, "the Koch level is untouched");

        let change = evaluate_auto_level(&after(0.2), &settings, &mut Default::default());
        assert_eq!(change.map(|c| c.to), Some(3));

        settings.curriculum.callsign_level = CALLSIGN_TIER_MAX;
        assert!(evaluate_auto_level(&after(0.99), &settings, &mut Default::default()).is_none());
        settings.curriculum.callsign_level = CALLSIGN_TIER_MIN;
        assert!(evaluate_auto_level(&after(0.1), &settings, &mut Default::default()).is_none());
    }

    #[test]
    fn mixed_takes_turns_when_the_session_does_not_say_which_axis() {
        let mut s = mixed(50);
        let first =
            evaluate_auto_level(&after(1.0), &s, &mut Default::default()).expect("letters first");
        assert_eq!((first.axis, first.to), (LevelAxis::Letters, 6));
        assert_eq!(first.next_mixed_axis, Some(MixedAutoLevelAxis::Digits));
        apply_auto_level(&mut s, &first);
        assert_eq!(s.curriculum.level, 6);
        assert_eq!(
            s.auto_level.mixed_auto_level_next_axis,
            MixedAutoLevelAxis::Digits
        );

        let second =
            evaluate_auto_level(&after(1.0), &s, &mut Default::default()).expect("digits next");
        assert_eq!((second.axis, second.to), (LevelAxis::Digits, 6));
        apply_auto_level(&mut s, &second);
        assert_eq!(s.curriculum.digits_level, 6);
    }

    /// Every letter right and every digit wrong is 80% overall, and with
    /// letters next in turn the letter level used to fall — the one axis that
    /// was perfect.
    #[test]
    fn mixed_lowers_the_axis_that_fell_short() {
        let s = mixed(50);
        assert_eq!(
            s.auto_level.mixed_auto_level_next_axis,
            MixedAutoLevelAxis::Letters
        );
        let result = copied(&[("KMUR0", "KMUR5"), ("ES1NA", "ES9NA")]);
        assert!((result.accuracy - 0.8).abs() < 1e-9);
        let change = evaluate_auto_level(&result, &s, &mut Default::default()).expect("down");
        assert_eq!((change.axis, change.to), (LevelAxis::Digits, 4));

        // With the digits already at the bottom there is nothing to lower,
        // and the perfect letters stay where they are.
        let mut bottom = s;
        bottom.curriculum.digits_level = 1;
        assert!(evaluate_auto_level(&result, &bottom, &mut Default::default()).is_none());
    }

    /// And the other way round: perfect digits used to carry letters below
    /// the threshold over the line.
    #[test]
    fn mixed_raises_the_axis_that_earned_it() {
        let s = mixed(50);
        let result = copied(&[
            ("KMURE", "KMURE"),
            ("SNAPT", "SXAXT"),
            ("01234", "01234"),
            ("56789", "56789"),
            ("98765", "98765"),
        ]);
        assert!(result.accuracy >= 0.9);
        let letters = result.tally.accuracy_where(|c| !c.is_ascii_digit());
        assert_eq!(letters, Some(0.8));
        let change = evaluate_auto_level(&result, &s, &mut Default::default()).expect("up");
        assert_eq!((change.axis, change.to), (LevelAxis::Digits, 6));
    }

    #[test]
    fn mixed_skips_an_axis_that_is_not_being_practised() {
        // Letters only: the digit axis never moves, and the next axis stays letters.
        let mut s = mixed(100);
        s.auto_level.mixed_auto_level_next_axis = MixedAutoLevelAxis::Digits;
        let change =
            evaluate_auto_level(&after(1.0), &s, &mut Default::default()).expect("letters");
        assert_eq!(change.axis, LevelAxis::Letters);
        assert_eq!(change.next_mixed_axis, Some(MixedAutoLevelAxis::Letters));

        // Digits only: the letter axis never moves.
        let mut s = mixed(0);
        let change = evaluate_auto_level(&after(0.0), &s, &mut Default::default()).expect("digits");
        assert_eq!((change.axis, change.to), (LevelAxis::Digits, 4));
        assert_eq!(change.next_mixed_axis, Some(MixedAutoLevelAxis::Digits));
        apply_auto_level(&mut s, &change);
        assert_eq!(s.curriculum.digits_level, 4);
        assert_eq!(s.curriculum.level, 5);

        // A letter-only Mixed with a two-letter alphabet has nowhere to go.
        let mut s = mixed(100);
        s.curriculum.custom_sequence = vec!['K', 'M'];
        s.curriculum.level = 1;
        s.curriculum.digits_level = 1;
        let mut counters = AutoLevelCounters::default();
        assert!(evaluate_auto_level(&after(1.0), &s, &mut counters).is_none());
        assert_eq!(counters.above, 0);
    }

    #[test]
    fn mixed_at_both_ceilings_clears_its_run_instead_of_moving() {
        let mut s = mixed(50);
        s.curriculum.level = s.max_level(LevelAxis::Letters);
        s.curriculum.digits_level = crate::morse::MAX_DIGITS_LEVEL;
        let mut counters = AutoLevelCounters { above: 3, below: 0 };
        assert!(evaluate_auto_level(&after(1.0), &s, &mut counters).is_none());
        assert_eq!(counters.above, 0);

        s.curriculum.level = 1;
        s.curriculum.digits_level = 1;
        let mut counters = AutoLevelCounters { above: 0, below: 3 };
        assert!(evaluate_auto_level(&after(0.0), &s, &mut counters).is_none());
        assert_eq!(counters.below, 0);
    }

    #[test]
    fn progress_reports_the_axis_that_is_actually_practised() {
        let mut s = mixed(100);
        s.auto_level.mixed_auto_level_next_axis = MixedAutoLevelAxis::Digits;
        let progress =
            auto_level_progress(&s, AutoLevelCounters { above: 2, below: 1 }).expect("progress");
        assert!(progress.alternating_mixed);
        assert_eq!(progress.next_mixed_axis, Some(MixedAutoLevelAxis::Letters));
        assert_eq!((progress.above_count, progress.below_count), (2, 1));

        let progress = auto_level_progress(&mixed(50), AutoLevelCounters::default()).expect("p");
        assert_eq!(progress.next_mixed_axis, Some(MixedAutoLevelAxis::Letters));

        // Outside mixed there is no axis at all.
        let progress = auto_level_progress(&koch(1), AutoLevelCounters::default()).expect("p");
        assert!(!progress.alternating_mixed);
        assert_eq!(progress.next_mixed_axis, None);
    }

    /// The keys are where counters already sit in people's saves, so their
    /// shape is fixed.
    #[test]
    fn counter_keys_separate_modes_levels_and_alphabets() {
        let koch = koch(1);
        let mut own = koch.clone();
        own.curriculum.custom_sequence = vec!['K', 'M'];
        let mut mixed_settings = koch.clone();
        mixed_settings.curriculum.char_set_mode = CharSetMode::Mixed;
        let mut digits = koch.clone();
        digits.curriculum.char_set_mode = CharSetMode::Digits;
        let mut callsigns = koch.clone();
        callsigns.curriculum.char_set_mode = CharSetMode::Callsign;
        callsigns.curriculum.callsign_level = 3;

        let keys = [&koch, &own, &mixed_settings, &digits, &callsigns].map(CountersKey::of);
        assert_eq!(
            keys[0].as_str(),
            format!("koch_1_{}", koch.alphabet_fingerprint())
        );
        assert_eq!(keys[1].as_str(), "koch_1_KM");
        assert!(keys[2].as_str().starts_with("mixed_1_1_"));
        assert_eq!(keys[3].as_str(), "digits_1_0123456789");
        assert_eq!(keys[4].as_str(), "callsign_3");
        let mut unique = keys.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), keys.len());

        // A different sequence is a different key at the same level.
        let mut other = koch.clone();
        other.curriculum.custom_sequence = vec!['A', 'B', 'C'];
        assert_ne!(CountersKey::of(&koch), CountersKey::of(&other));
    }

    /// A session of `settings.curriculum.num_groups` groups, every one sent
    /// as KM and answered `answer`.
    fn played(settings: TrainingSettings, answer: &str) -> GroupSession {
        use crate::machine::{GroupSource, SessionEffect, SessionEvent, SessionMachine};
        use crate::session::{MonoMs, SessionId};

        struct Km;
        impl GroupSource for Km {
            fn next_group(&mut self, _: &TrainingSettings) -> (String, u32) {
                ("KM".into(), 1)
            }
            fn learn(&mut self, _: crate::session::Answered<'_>) {}
        }

        let groups = settings.curriculum.num_groups as usize;
        let (mut machine, _) = SessionMachine::start(SessionId::new(1), MonoMs(0), settings, Km);
        let mut now = MonoMs(0);
        for index in 0..groups {
            now = now.plus(1_000);
            let start = crate::SendStart {
                char_wpm: 20.0,
                effective_wpm: 20.0,
                output_at: now,
                char_ends_ms: Vec::new(),
                latency_ms: 0,
            };
            let _ = machine.apply(SessionEvent::PlaybackStarted { index, start }, now);
            let ended = SessionEvent::PlaybackEnded {
                index,
                duration_sec: 0.5,
            };
            let _ = machine.apply(ended, now);
            let text = answer.to_string();
            let keys = crate::recognition::Keys::default();
            let _ = machine.apply(SessionEvent::Input { index, text, keys }, now);
            for effect in machine.apply(SessionEvent::Confirm, now) {
                if let SessionEffect::StartTimer { id, .. } = effect {
                    let _ = machine.apply(SessionEvent::TimerFired { id }, now);
                }
            }
        }
        machine.session().clone()
    }

    fn levelling(above_target: u32) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        settings.curriculum.num_groups = 4;
        settings.auto_level.auto_adjust_level = true;
        settings.auto_level.auto_adjust_threshold = 50.0;
        settings.auto_level.auto_adjust_above_threshold_count = above_target;
        // Played straight through: no count down to wait out first.
        settings.playback.countdown_sec = 0;
        settings
    }

    #[test]
    fn a_session_that_moves_nothing_adds_to_its_run() {
        let settings = levelling(3);
        let done = complete_session(
            &played(settings.clone(), "KM"),
            AutoLevelCounters { above: 1, below: 0 },
            7,
            "2026-09-01".into(),
        );
        assert_eq!(done.result.groups.len(), 4);
        assert_eq!(
            (done.result.timestamp, done.result.date.as_str()),
            (7, "2026-09-01")
        );
        assert_eq!(done.change, None);
        assert_eq!(done.settings, settings);
        assert_eq!(
            done.counters,
            CountersUpdate::Keep(
                CountersKey::of(&settings),
                AutoLevelCounters { above: 2, below: 0 }
            )
        );
    }

    #[test]
    fn a_run_that_earns_a_rise_moves_the_level_and_starts_afresh() {
        let settings = levelling(2);
        let done = complete_session(
            &played(settings.clone(), "KM"),
            AutoLevelCounters { above: 1, below: 0 },
            7,
            "2026-09-01".into(),
        );
        let change = done.change.expect("a rise");
        assert_eq!((change.from, change.to), (1, 2));
        assert_eq!(done.settings.curriculum.level, 2);
        let mut after = settings.clone();
        after.curriculum.level = 2;
        assert_eq!(
            done.counters,
            CountersUpdate::Clear([CountersKey::of(&settings), CountersKey::of(&after)])
        );
    }
}
