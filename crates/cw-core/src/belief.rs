//! A belief held exactly on a fixed grid, the way adaptive testing holds a
//! threshold (QUEST, the Ψ method): a log prior and a log likelihood for
//! every cell, the likelihood fading as sessions pass, so the belief follows
//! you as you improve. The copy span ([`crate::span`]) and the copy speed
//! ([`crate::speed`]) are both one, each with a grid and a curve of its own.

use crate::rng::Rng;
use crate::session::SessionResult;
use crate::settings::CharSetMode;

/// What a session's evidence is worth once another has followed it: half,
/// ten sessions on.
pub(crate) const SESSION_FADE: f64 = 0.933_032_991_536_807_4;

/// An estimate: the posterior mean and standard deviation of whatever the
/// belief measures, in its own units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    pub mean: f64,
    pub sd: f64,
}

/// An estimate as it stood after one session of the history.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EstimatePoint {
    pub timestamp: u64,
    pub estimate: Estimate,
}

/// The posterior over a grid of cells, kept as log weights: there is no
/// approximation to drift and nothing to diverge.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GridBelief {
    /// Log prior, one per cell.
    prior: Vec<f64>,
    /// Log likelihood of everything observed, faded by age.
    evidence: Vec<f64>,
}

impl GridBelief {
    /// A belief with nothing learned yet: `prior` is its log prior, one per
    /// cell.
    pub(crate) fn new(prior: Vec<f64>) -> Self {
        let evidence = vec![0.0; prior.len()];
        Self { prior, evidence }
    }

    /// One more session has passed: everything learned so far counts for a
    /// little less.
    pub(crate) fn fade(&mut self) {
        for evidence in &mut self.evidence {
            *evidence *= SESSION_FADE;
        }
    }

    /// Learn from `hits` successes and `misses` failures, `chances` giving
    /// every cell's `[ln P(hit), ln P(miss)]` in turn.
    pub(crate) fn observe(
        &mut self,
        chances: impl IntoIterator<Item = [f64; 2]>,
        hits: f64,
        misses: f64,
    ) {
        for (evidence, [hit, miss]) in self.evidence.iter_mut().zip(chances) {
            *evidence += hits * hit + misses * miss;
        }
    }

    /// The posterior, one probability per cell. A posterior that cannot be
    /// read is spread evenly.
    pub(crate) fn posterior(&self) -> Vec<f64> {
        let log: Vec<f64> = self
            .prior
            .iter()
            .zip(&self.evidence)
            .map(|(prior, evidence)| prior + evidence)
            .collect();
        let top = log.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mut weights: Vec<f64> = log.iter().map(|value| (value - top).exp()).collect();
        let total: f64 = weights.iter().sum();
        if total.is_finite() && total > 0.0 {
            for weight in &mut weights {
                *weight /= total;
            }
        } else {
            weights = vec![1.0 / log.len().max(1) as f64; log.len()];
        }
        weights
    }

    /// One cell, drawn from the posterior: the Thompson draw.
    pub(crate) fn draw(&self, rng: &mut impl Rng) -> usize {
        pick_index(&self.posterior(), rng)
    }

    /// The posterior mean and standard deviation of `value`, which says what
    /// each cell stands for.
    pub(crate) fn estimate(&self, value: impl Fn(usize) -> f64) -> Estimate {
        let posterior = self.posterior();
        let values: Vec<f64> = (0..posterior.len()).map(value).collect();
        let mean: f64 = posterior.iter().zip(&values).map(|(p, v)| p * v).sum();
        let variance: f64 = posterior
            .iter()
            .zip(&values)
            .map(|(p, v)| p * (v - mean).powi(2))
            .sum();
        Estimate {
            mean,
            sd: variance.max(0.0).sqrt(),
        }
    }
}

/// One index, in proportion to `weights`. A table with no weight in it, or
/// one that cannot be read, is drawn from evenly.
pub(crate) fn pick_index(weights: &[f64], rng: &mut impl Rng) -> usize {
    let total: f64 = weights.iter().sum();
    if !(total.is_finite() && total > 0.0) {
        return rng.usize_in(0, weights.len().saturating_sub(1));
    }
    let mut left = rng.f64() * total;
    for (index, weight) in weights.iter().enumerate() {
        if left < *weight {
            return index;
        }
        left -= weight;
    }
    weights.len().saturating_sub(1)
}

/// The sessions of `mode` that have groups to read, in the order they were
/// practised: what a belief is rebuilt from. A session with no groups says
/// nothing, and is not one more session for what came before it to fade
/// behind.
pub(crate) fn sessions_in_order(
    history: &[SessionResult],
    mode: CharSetMode,
) -> Vec<&SessionResult> {
    let mut sessions: Vec<&SessionResult> = history
        .iter()
        .filter(|session| session.char_set_mode == mode && !session.groups.is_empty())
        .collect();
    sessions.sort_by_key(|session| session.timestamp);
    sessions
}

/// Every session that copied any character, oldest first, each with what
/// its evidence is worth by now: the latest counts one, and each one before
/// it [`SESSION_FADE`] as much as the one after it, half ten sessions back,
/// as the span and the speed fade. Whatever character set it practised: a
/// character is as hard to copy whichever of them sent it. A session that
/// copied nothing is not one more session for the rest to fade behind.
pub(crate) fn sessions_faded(history: &[SessionResult]) -> Vec<(&SessionResult, f64)> {
    let mut sessions: Vec<&SessionResult> = history
        .iter()
        .filter(|session| !session.letter_accuracy().is_empty())
        .collect();
    sessions.sort_by_key(|session| session.timestamp);
    let count = sessions.len();
    sessions
        .into_iter()
        .enumerate()
        .map(|(index, session)| {
            let back = i32::try_from(count - 1 - index).unwrap_or(i32::MAX);
            (session, SESSION_FADE.powi(back))
        })
        .collect()
}

/// Hits and misses counted per key — a length, a speed — so that a session
/// costs one update per key it sent, not one per group. Keys keep the order
/// they first came in.
#[derive(Debug)]
pub(crate) struct Tally<K> {
    rows: Vec<(K, f64, f64)>,
}

impl<K: PartialEq> Tally<K> {
    pub(crate) fn new() -> Self {
        Self { rows: Vec::new() }
    }

    pub(crate) fn add(&mut self, key: K, hits: f64, misses: f64) {
        match self.rows.iter_mut().find(|(seen, _, _)| *seen == key) {
            Some((_, h, m)) => {
                *h += hits;
                *m += misses;
            }
            None => self.rows.push((key, hits, misses)),
        }
    }

    pub(crate) fn rows(self) -> Vec<(K, f64, f64)> {
        self.rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;

    fn belief() -> GridBelief {
        GridBelief::new(vec![0.0, 0.0, 0.0])
    }

    #[test]
    fn with_nothing_learned_the_posterior_is_the_prior() {
        let posterior = belief().posterior();
        assert!(posterior.iter().all(|p| (p - 1.0 / 3.0).abs() < 1e-12));
        assert!(GridBelief::new(Vec::new()).posterior().is_empty());
    }

    #[test]
    fn evidence_moves_the_posterior_and_fades_with_age() {
        let mut learned = belief();
        // The first cell explains a hit best.
        let chances = [[-0.1, -2.3], [-0.7, -0.7], [-2.3, -0.1]];
        learned.observe(chances, 3.0, 0.0);
        let fresh = learned.posterior();
        assert!(fresh[0] > fresh[1] && fresh[1] > fresh[2], "{fresh:?}");
        learned.fade();
        let faded = learned.posterior();
        assert!(faded[0] < fresh[0], "older evidence counts for less");
        let estimate = learned.estimate(|cell| cell as f64);
        assert!(estimate.mean < 1.0 && estimate.sd > 0.0, "{estimate:?}");
    }

    #[test]
    fn an_unreadable_posterior_is_drawn_evenly() {
        let mut broken = belief();
        broken.observe([[f64::NAN, 0.0]; 3], 1.0, 0.0);
        assert_eq!(broken.posterior(), vec![1.0 / 3.0; 3]);
        let mut rng = SplitMix64::new(1);
        assert!(broken.draw(&mut rng) < 3);
        assert!(pick_index(&[0.0, 0.0], &mut rng) < 2);
        assert_eq!(pick_index(&[0.0, 1.0], &mut rng), 1);
    }

    #[test]
    fn a_tally_adds_by_key_in_the_order_keys_came() {
        let mut tally = Tally::new();
        tally.add(5, 1.0, 0.0);
        tally.add(3, 0.0, 1.0);
        tally.add(5, 0.5, 0.5);
        assert_eq!(tally.rows(), vec![(5, 1.5, 0.5), (3, 0.0, 1.0)]);
    }

    /// Every session that copied anything fades behind the ones after it,
    /// whatever it practised, by its place in time rather than the order
    /// it was stored in; one that copied nothing is no session at all.
    #[test]
    fn character_evidence_halves_every_ten_sessions_back() {
        let session = |timestamp, mode, copied: &str| {
            let mut session = SessionResult {
                timestamp,
                char_set_mode: mode,
                ..SessionResult::empty_for_test()
            };
            session.tally.add(copied, copied);
            session
        };
        let mut history: Vec<SessionResult> = (0..11)
            .rev()
            .map(|at| session(at, CharSetMode::Koch, "K"))
            .collect();
        history[3].char_set_mode = CharSetMode::Digits;
        history.push(session(99, CharSetMode::Koch, ""));
        let faded = sessions_faded(&history);
        assert_eq!(faded.len(), 11);
        assert_eq!(faded[10].0.timestamp, 10);
        assert_eq!(faded[10].1, 1.0, "the latest counts in full");
        assert!((faded[0].1 - 0.5).abs() < 1e-12, "ten back counts half");
        assert!(faded.windows(2).all(|pair| pair[0].1 < pair[1].1));
    }

    #[test]
    fn only_sessions_of_the_mode_with_groups_count_oldest_first() {
        let session = |timestamp, mode, groups: usize| SessionResult {
            timestamp,
            char_set_mode: mode,
            groups: vec![crate::session::GroupResult::default(); groups],
            ..SessionResult::empty_for_test()
        };
        let history = [
            session(3, CharSetMode::Koch, 1),
            session(1, CharSetMode::Koch, 1),
            session(2, CharSetMode::Callsign, 1),
            session(4, CharSetMode::Koch, 0),
        ];
        let order: Vec<u64> = sessions_in_order(&history, CharSetMode::Koch)
            .iter()
            .map(|session| session.timestamp)
            .collect();
        assert_eq!(order, [1, 3]);
    }
}
