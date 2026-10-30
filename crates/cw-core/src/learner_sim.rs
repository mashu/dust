//! Simulated learners practising under auto level, with adaptive group size
//! and adaptive speed on and off: what auto level does with a learner who
//! improves, one who holds steady, and one who struggles.
//!
//! Each learner is the design notes' own: a character is copied right with
//! the adaptive speed note's curve, nine times in ten at its copy speed, and
//! a group is held to its end with the adaptive group size note's span
//! curve, lost from a random point on when it is not. What the notes'
//! learners lack is a level to be ready for, as each character is as easy
//! as the next to them. So here every character has a copy speed of its
//! own: a character just unlocked is copied at half the learner's speed,
//! and comes up to it as it is heard. A learner can come to know only so
//! many characters: past that, a character stays as hard as when it was
//! new.

use std::collections::BTreeMap;

use crate::alignment::CopyTally;
use crate::auto_level::{AutoLevelCounters, CountersKey, apply_auto_level, evaluate_auto_level};
use crate::machine::GroupSource;
use crate::morse::LCWO_SEQUENCE;
use crate::rng::{Rng, SplitMix64};
use crate::sampling::{Sampler, length_floor};
use crate::score::copy_speed;
use crate::session::{Answered, GroupResult, SessionResult, SessionTiming};
use crate::settings::{Aim, CharSetMode, TrainingSettings};

/// The chance of fumbling a character however slowly it was sent.
const LAPSE: f64 = 0.02;
/// How fast a character just unlocked is copied, against one known.
const NEW: f64 = 0.5;
/// How many hearings take a new character most of the way to known: two
/// thirds of the way in this many.
const LEARNED_IN: f64 = 25.0;
/// The level every run starts at: eleven characters.
const START: u32 = 10;

/// A learner, as the design notes model one, with characters to learn.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Learner {
    /// The speed at which they copy nine known characters in ten, in WPM,
    /// at their first session, and what it is multiplied by each session
    /// after.
    pub copy: f64,
    pub growth: f64,
    /// How sharply their copy falls away as the speed rises, in log speed.
    pub slope: f64,
    /// The length they hold to its end about half the time, and how sharply
    /// that falls away, in characters.
    pub span: f64,
    pub spread: f64,
    /// How many characters, in the order Koch training unlocks them, they
    /// can come to know.
    pub capacity: usize,
}

/// A learner who gets faster every session and learns each new character.
pub(crate) const IMPROVING: Learner = Learner {
    copy: 20.0,
    growth: 1.04,
    slope: 0.12,
    span: 7.0,
    spread: 1.0,
    capacity: usize::MAX,
};

/// A learner who holds steady: the eleven characters of the starting level
/// known, and no more to be learned.
pub(crate) const STABLE: Learner = Learner {
    growth: 1.0,
    capacity: START as usize + 1,
    ..IMPROVING
};

/// A learner who holds steady with four of the starting level's eleven
/// characters beyond them.
pub(crate) const STRUGGLING: Learner = Learner {
    capacity: START as usize - 3,
    ..STABLE
};

/// A learner at work: how often each character has been heard, and how many
/// sessions have passed.
struct Copier {
    learner: Learner,
    heard: BTreeMap<char, f64>,
    session: i32,
}

impl Copier {
    /// The learner, the characters of the starting level long known.
    fn new(learner: Learner) -> Self {
        let heard = LCWO_SEQUENCE
            .iter()
            .take(START as usize + 1)
            .map(|c| (*c, 10_000.0))
            .collect();
        Self {
            learner,
            heard,
            session: 0,
        }
    }

    /// The speed at which they copy nine in ten of `character` now.
    fn copy_speed_of(&self, character: char) -> f64 {
        let own = self.learner.copy * self.learner.growth.powi(self.session);
        let place = LCWO_SEQUENCE.iter().position(|each| *each == character);
        if place.is_none_or(|place| place >= self.learner.capacity) {
            return own * NEW;
        }
        let heard = self.heard.get(&character).copied().unwrap_or(0.0);
        own * (1.0 - (1.0 - NEW) * (-heard / LEARNED_IN).exp())
    }

    /// The chance they copy `character` sent at `speed`.
    fn recognises(&self, character: char, speed: f64) -> f64 {
        let reference = (0.9 / (1.0 - LAPSE) / (1.0 - 0.9 / (1.0 - LAPSE))).ln();
        let x = reference + (self.copy_speed_of(character).ln() - speed.ln()) / self.learner.slope;
        (1.0 - LAPSE) / (1.0 + (-x).exp())
    }

    /// What they type for `sent` at `speed`, hearing every character of it.
    fn answer(&mut self, sent: &str, speed: f64, rng: &mut SplitMix64) -> String {
        let length = sent.chars().count();
        let held = 0.97 / (1.0 + ((length as f64 - self.learner.span) / self.learner.spread).exp());
        let kept = if rng.f64() < held {
            length
        } else {
            rng.usize_in(0, length.saturating_sub(1))
        };
        let typed = sent
            .chars()
            .take(kept)
            .map(|c| {
                if rng.f64() < self.recognises(c, speed) {
                    c
                } else {
                    '#'
                }
            })
            .collect();
        for c in sent.chars() {
            *self.heard.entry(c).or_default() += 1.0;
        }
        typed
    }
}

/// What adapts while a learner practises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Adapting {
    Neither,
    Length,
    Speed,
    Both,
}

impl Adapting {
    pub(crate) const ALL: [Self; 4] = [Self::Neither, Self::Length, Self::Speed, Self::Both];

    /// Koch at the starting level, auto level as it ships. Speed fixed at
    /// 15 WPM, or adapting from 15 to 30; groups of two or three, or
    /// adapting from two to eight.
    fn settings(self, aim: Aim) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = START;
        settings.curriculum.aim = aim;
        let length = matches!(self, Self::Length | Self::Both);
        let speed = matches!(self, Self::Speed | Self::Both);
        settings.curriculum.adaptive_group_size = length;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = if length { 8 } else { 3 };
        settings.playback.adaptive_speed = speed;
        settings.playback.link_char_to_effective = true;
        settings.playback.char_wpm_min = 15.0;
        settings.playback.char_wpm_max = if speed { 30.0 } else { 15.0 };
        settings.clamp()
    }
}

/// How auto level judges the sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Judged {
    /// As it does now.
    Now,
    /// As it did before: on each session's own accuracy, whatever adapts.
    Before,
}

/// The level after each of `sessions` sessions of `learner` practising with
/// `adapting` at `aim`, auto level judging as `judged` says.
pub(crate) fn levels(
    learner: Learner,
    adapting: Adapting,
    aim: Aim,
    judged: Judged,
    sessions: i32,
    seed: u64,
) -> Vec<u32> {
    let mut settings = adapting.settings(aim);
    let mut copier = Copier::new(learner);
    let mut rng = SplitMix64::new(seed);
    let mut history: Vec<SessionResult> = Vec::new();
    let mut counters: BTreeMap<CountersKey, AutoLevelCounters> = BTreeMap::new();
    let mut levels = Vec::new();
    for session in 0..sessions {
        copier.session = session;
        let mut sampler =
            Sampler::new(&history, &settings, seed.wrapping_mul(977) + session as u64);
        let mut result = SessionResult {
            timestamp: session as u64 + 1,
            level: settings.curriculum.level,
            alphabet_fingerprint: settings.alphabet_fingerprint(),
            length_floor: length_floor(&settings).map(|floor| floor as u32),
            ..SessionResult::empty_for_test()
        };
        let fixed = settings.playback.char_wpm_min;
        for _ in 0..settings.curriculum.num_groups {
            let (sent, _) = sampler.next_group(&settings);
            let played = GroupSource::next_speed(&mut sampler, &settings).unwrap_or((fixed, fixed));
            let typed = copier.answer(&sent, copy_speed(played.0, played.1), &mut rng);
            let copied = CopyTally::of(&sent, &typed);
            GroupSource::learn(
                &mut sampler,
                Answered {
                    sent: &sent,
                    received: &typed,
                    copied: &copied,
                    played: Some(played),
                },
            );
            result.tally.merge(&copied);
            result.group_timings.push(SessionTiming {
                char_wpm: Some(played.0),
                effective_wpm: Some(played.1),
                ..SessionTiming::default()
            });
            result.groups.push(GroupResult {
                correct: sent == typed,
                sent,
                received: typed,
            });
        }
        result.accuracy = result.tally.accuracy();
        let key = CountersKey::of(&settings);
        let mut run = counters.get(&key).copied().unwrap_or_default();
        // Judged as before, the session is held to the threshold as it was
        // scored: as though nothing adapted.
        let mut judging = settings.clone();
        if judged == Judged::Before {
            judging.curriculum.adaptive_group_size = false;
            judging.playback.adaptive_speed = false;
        }
        match evaluate_auto_level(&result, &judging, &mut run) {
            Some(change) => {
                for cleared in &change.counters_cleared {
                    counters.remove(cleared);
                }
                apply_auto_level(&mut settings, &change);
            }
            None => {
                counters.insert(key, run);
            }
        }
        history.push(result);
        levels.push(settings.curriculum.level);
    }
    levels
}

/// The mean of `levels`.
pub(crate) fn mean(levels: &[u32]) -> f64 {
    levels.iter().map(|level| f64::from(*level)).sum::<f64>() / levels.len().max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSIONS: i32 = 36;

    /// Auto level with the adaptive features on and off, for a learner who
    /// improves, one who holds steady and one who struggles: the first
    /// levels up, the second stays where it started, and the third levels
    /// down. A session's own accuracy is noisy, and auto level as it ships
    /// lowers the level after one session under the threshold, so the
    /// steady learner wanders a level or two with nothing adapting too: it
    /// is held to staying within a level on average, and ending within two.
    #[test]
    fn auto_level_follows_the_learner_whatever_adapts() {
        let start = f64::from(START);
        let run =
            |learner, adapting, aim, judged| levels(learner, adapting, aim, judged, SESSIONS, 3);
        let stays = |levels: &[u32]| {
            (mean(levels) - start).abs() <= 1.0
                && levels.last().is_some_and(|last| last.abs_diff(START) <= 2)
        };
        for adapting in Adapting::ALL {
            let improving = run(IMPROVING, adapting, Aim::Balanced, Judged::Now);
            let last = *improving.last().expect("sessions");
            assert!(
                last >= START + 2 && mean(&improving[24..]) > start + 1.0,
                "{adapting:?}: an improving learner ended at {last}: {improving:?}"
            );

            let stable = run(STABLE, adapting, Aim::Balanced, Judged::Now);
            assert!(
                stays(&stable),
                "{adapting:?}: a stable learner wandered: {stable:?}"
            );

            let struggling = run(STRUGGLING, adapting, Aim::Balanced, Judged::Now);
            let last = *struggling.last().expect("sessions");
            assert!(
                last <= START - 2 && mean(&struggling[24..]) < start - 1.0,
                "{adapting:?}: a struggling learner ended at {last}: {struggling:?}"
            );
        }
    }

    /// Judged on each session's own accuracy, as it was, the adaptive
    /// features held it under the threshold by design. With group size
    /// adapting, even the improving learner went down; with the speed
    /// adapting at Stretch, the steady one did, where it now stays.
    #[test]
    fn judged_as_before_the_adaptive_features_only_lowered_the_level() {
        for adapting in [Adapting::Length, Adapting::Both] {
            let before = levels(
                IMPROVING,
                adapting,
                Aim::Balanced,
                Judged::Before,
                SESSIONS,
                3,
            );
            let last = *before.last().expect("sessions");
            assert!(
                last <= START - 4,
                "{adapting:?}: went to {last}: {before:?}"
            );
        }
        let before = levels(
            STABLE,
            Adapting::Speed,
            Aim::Stretch,
            Judged::Before,
            SESSIONS,
            3,
        );
        let last = *before.last().expect("sessions");
        assert!(last <= START - 4, "went to {last}: {before:?}");
        let now = levels(
            STABLE,
            Adapting::Speed,
            Aim::Stretch,
            Judged::Now,
            SESSIONS,
            3,
        );
        assert!((mean(&now) - f64::from(START)).abs() <= 1.0, "now: {now:?}");
    }

    /// The figures the design notes report. `cargo test -p cw-core
    /// --release auto_level_simulation -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn auto_level_simulation() {
        println!(
            "| Learner | Adapting | Judged | Level after 12 sessions, Comfortable · Balanced · Stretch | after 36 | Mean level |"
        );
        println!("| --- | --- | --- | --- | --- | --- |");
        for (name, learner) in [
            ("improving", IMPROVING),
            ("stable", STABLE),
            ("struggling", STRUGGLING),
        ] {
            for adapting in Adapting::ALL {
                for judged in [Judged::Before, Judged::Now] {
                    if adapting == Adapting::Neither && judged == Judged::Before {
                        continue;
                    }
                    let (mut twelve, mut last, mut means) = (Vec::new(), Vec::new(), Vec::new());
                    for aim in Aim::ALL {
                        for seed in [3, 4, 5] {
                            let levels = levels(learner, adapting, aim, judged, SESSIONS, seed);
                            twelve.push(levels[11].to_string());
                            last.push(levels[35].to_string());
                            means.push(mean(&levels));
                        }
                    }
                    let group = |each: &[String]| {
                        each.chunks(3)
                            .map(|seeds| seeds.join(" "))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    };
                    println!(
                        "| {name} | {adapting:?} | {judged:?} | {} | {} | {:.1} |",
                        group(&twelve),
                        group(&last),
                        means.iter().sum::<f64>() / means.len() as f64
                    );
                }
            }
        }
    }
}
