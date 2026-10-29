//! Error-weighted + coverage-balanced character sampling for group generation.

use std::collections::BTreeMap;

use crate::alignment::CopyTally;
use crate::belief::{SESSION_FADE, sessions_faded};
use crate::callsign::{CallSpec, GeneratedCall, generate_call};
use crate::contest::{CallSource, Side};
use crate::machine::GroupSource;
use crate::pool::compute_char_pool;
use crate::rng::{Rng, SplitMix64, weighted_random_pick};
use crate::session::{Answered, SessionResult};
use crate::settings::{CharSetMode, LevelAxis, SettingsSection, TrainingSettings};
use crate::span::SpanBelief;
use crate::speed::{SpeedBelief, SpeedPath};

pub const CHAR_SAMPLING_PRIOR_ALPHA: f64 = 1.0;
pub const CHAR_SAMPLING_PRIOR_BETA: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharBetaBelief {
    pub alpha: f64,
    pub beta: f64,
}

impl Default for CharBetaBelief {
    fn default() -> Self {
        Self {
            alpha: CHAR_SAMPLING_PRIOR_ALPHA,
            beta: CHAR_SAMPLING_PRIOR_BETA,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharSamplingState {
    pub beliefs: BTreeMap<char, CharBetaBelief>,
    pub session_sample_counts: BTreeMap<char, u32>,
}

impl CharSamplingState {
    /// What the history says about each character: every character copied
    /// right moves its belief one way, every miss the other. Every session
    /// counts, whatever character set, unlock order or level it was
    /// practised at: a character is as hard to copy whichever of them sent
    /// it, and no setting hides what was learned.
    ///
    /// Older sessions count for less, as they do for the span and the
    /// speed: a session ten sessions back counts half as much as the latest
    /// ([`sessions_faded`]). A weakness put right long ago stops drawing the
    /// character more often, and a character not heard for a while is less
    /// certain again.
    pub fn from_history(history: &[SessionResult]) -> Self {
        let mut state = Self::default();
        for (session, worth) in sessions_faded(history) {
            for (character, stats) in session.letter_accuracy().iter() {
                let belief = state.beliefs.entry(*character).or_default();
                belief.alpha += worth * f64::from(stats.correct);
                belief.beta += worth * f64::from(stats.total.saturating_sub(stats.correct));
            }
        }
        state
    }

    /// The beliefs a new session starts from: the history's, all of it one
    /// session older, so the last session counts for a little less than
    /// the one about to be sent, as the span's and the speed's do.
    pub fn for_session(history: &[SessionResult]) -> Self {
        let mut state = Self::from_history(history);
        for belief in state.beliefs.values_mut() {
            belief.alpha = CHAR_SAMPLING_PRIOR_ALPHA
                + (belief.alpha - CHAR_SAMPLING_PRIOR_ALPHA) * SESSION_FADE;
            belief.beta =
                CHAR_SAMPLING_PRIOR_BETA + (belief.beta - CHAR_SAMPLING_PRIOR_BETA) * SESSION_FADE;
        }
        state
    }

    /// Learn from one answer: each character sent was copied or it was not.
    pub fn learn(&mut self, copied: &CopyTally) {
        for (character, stats) in &copied.sent {
            let belief = self.beliefs.entry(*character).or_default();
            belief.alpha += f64::from(stats.correct);
            belief.beta += f64::from(stats.total.saturating_sub(stats.correct));
        }
    }

    fn count_sent(&mut self, group: &str) {
        for character in group.chars() {
            *self.session_sample_counts.entry(character).or_insert(0) += 1;
        }
    }
}

/// The trainer's own group source: groups drawn for the curriculum, leaning
/// on the characters the history says are hard and on those this session
/// has not sent yet, and learning from every answer as it comes. With
/// adaptive group size on, their lengths lean on what you copy whole, too,
/// and with adaptive speed on their speeds, or a contest's callers', on
/// how fast you copy.
#[derive(Clone, Debug)]
pub struct Sampler {
    state: CharSamplingState,
    span: SpanBelief,
    speed: SpeedBelief,
    rng: SplitMix64,
}

impl Sampler {
    /// A sampler for a session of `settings`, seeded with what the history
    /// knows about the alphabet it trains. `seed` should differ from session
    /// to session.
    pub fn new(history: &[SessionResult], settings: &TrainingSettings, seed: u64) -> Self {
        Self {
            state: CharSamplingState::for_session(history),
            span: SpanBelief::for_session(history, settings),
            speed: SpeedBelief::for_session(history, settings),
            rng: SplitMix64::new(seed),
        }
    }
}

impl GroupSource for Sampler {
    fn next_group(&mut self, settings: &TrainingSettings) -> (String, u32) {
        let length = adaptive_bounds(settings).map(|(shortest, longest)| {
            let target = settings.curriculum.aim.groups_whole();
            self.span
                .draw_length(shortest, longest, target, &mut self.rng)
        });
        let group = generate_group(settings, &mut self.state, length, &mut self.rng);
        (group, resolve_group_repeats(settings, &mut self.rng))
    }

    fn learn(&mut self, answered: Answered<'_>) {
        self.state.learn(answered.copied);
        self.span.learn_copy(answered.copied);
        if let Some(copy) = played_speed(answered) {
            self.speed.learn(answered.sent, answered.received, copy);
        }
    }

    /// Drawn from the copy speed while group training's adaptive speed is
    /// on. In Contest mode the switch is the callers' alone, so Listen's
    /// stream of groups comes at speeds drawn evenly from the range there,
    /// as it always has.
    fn next_speed(&mut self, settings: &TrainingSettings) -> Option<(f64, f64)> {
        if settings.curriculum.char_set_mode == CharSetMode::Contest {
            return None;
        }
        self.draw_speed(settings)
    }
}

/// The copy speed `answered` was played at, once it is known.
fn played_speed(answered: Answered<'_>) -> Option<f64> {
    let (char_wpm, effective_wpm) = answered.played?;
    Some(crate::score::copy_speed(char_wpm, effective_wpm))
}

impl Sampler {
    /// The character and effective speed to send at next, drawn from the
    /// copy speed's belief within the speed settings, aiming for the share
    /// of characters "Aim for" asks: a group's, or a contest's caller's.
    /// None while the speed does not adapt ([`adaptive_speed_path`]).
    fn draw_speed(&mut self, settings: &TrainingSettings) -> Option<(f64, f64)> {
        let path = adaptive_speed_path(settings)?;
        let (low, high) = path.bounds();
        let target = settings.curriculum.aim.chars_right();
        let copy = self.speed.draw_speed(low, high, target, &mut self.rng);
        Some(path.voice_at(copy))
    }
}

/// How many times a contest's caller is drawn again when its call is one to
/// avoid, before the last draw is taken all the same.
const CALL_REDRAWS: usize = 8;

impl CallSource for Sampler {
    /// Drawn as Callsigns mode draws a call — the candidate whose characters
    /// the sampler wants most, at the callsign tier — from the countries the
    /// contest hears.
    fn next_call(
        &mut self,
        settings: &TrainingSettings,
        spec: CallSpec,
        avoid: &dyn Fn(&str) -> bool,
    ) -> GeneratedCall {
        let config = config_from_settings(settings);
        let mut call = wanted_call(settings, &self.state, &config, &mut self.rng, spec);
        for _ in 0..CALL_REDRAWS {
            if !avoid(&call.text) {
                break;
            }
            call = wanted_call(settings, &self.state, &config, &mut self.rng, spec);
        }
        self.state.count_sent(&call.text);
        call
    }

    /// Only what was copied of each character: the span belief draws group
    /// lengths, which a contest does not, and the speed belief hears only
    /// the calls and numbers ([`CallSource::learn_speed`]).
    fn learn(&mut self, answered: Answered<'_>) {
        self.state.learn(answered.copied);
    }

    /// As the history will read it once the run is in it, so a run's belief
    /// moves as the history's will: every character of a call or a number
    /// counts, those lost off its end too, as no span answers for them.
    fn learn_speed(&mut self, answered: Answered<'_>) {
        if let Some(copy) = played_speed(answered) {
            self.speed
                .learn_whole(answered.sent, answered.received, copy);
        }
    }

    /// As the next group's is drawn, when the callers' speed adapts.
    fn next_speed(&mut self, settings: &TrainingSettings) -> Option<(f64, f64)> {
        self.draw_speed(settings)
    }
}

/// Whether `settings` have each group's speed, or each new caller's in a
/// contest, chosen by adaptive speed.
pub fn adapts_speed(settings: &TrainingSettings) -> bool {
    adaptive_speed_path(settings).is_some()
}

/// Whether the adaptive speed switch of the mode `settings` train is on,
/// room or not: in Contest mode the callers' own, on as it ships, so group
/// training's, off as it ships, is left as it was; in any other, group
/// training's.
pub fn adaptive_speed_on(settings: &TrainingSettings) -> bool {
    if settings.curriculum.char_set_mode == CharSetMode::Contest {
        settings.contest.contest_adapt_speed
    } else {
        settings.playback.adaptive_speed
    }
}

/// The line adaptive speed moves along, when it has one: the mode's switch
/// is on, and the speed settings leave more than one speed to choose from.
pub(crate) fn adaptive_speed_path(settings: &TrainingSettings) -> Option<SpeedPath> {
    let path = SpeedPath::of(settings);
    (adaptive_speed_on(settings) && path.has_room()).then_some(path)
}

/// The shortest and longest group `settings` asks for. At least one
/// character: a group of none is a silent send nobody can answer, and only
/// settings that were never clamped could ask for it.
pub(crate) fn group_size_bounds(settings: &TrainingSettings) -> (usize, usize) {
    let shortest = settings.curriculum.min_group_size.max(1) as usize;
    let longest = (settings.curriculum.max_group_size as usize).max(shortest);
    (shortest, longest)
}

/// Whether `settings` have group lengths drawn by adaptive group size: what
/// the screens say about it, read off the same rule the sampler follows.
pub fn adapts_group_size(settings: &TrainingSettings) -> bool {
    adaptive_bounds(settings).is_some()
}

/// The settings card that shows the aim, and so puts it back: Session shape
/// while group size adapts, for both; Speed while only the speed does. With
/// neither adapting it shows nowhere, and Session shape keeps it.
pub fn aim_section(settings: &TrainingSettings) -> SettingsSection {
    if adapts_speed(settings) && !adapts_group_size(settings) {
        SettingsSection::Speed
    } else {
        SettingsSection::SessionShape
    }
}

/// The range adaptive group size draws within, when it has one to draw in:
/// it is switched on, there is more than one length to choose from, and the
/// groups are drawn a character at a time: callsigns and words are as long as
/// they are.
pub(crate) fn adaptive_bounds(settings: &TrainingSettings) -> Option<(usize, usize)> {
    let (shortest, longest) = group_size_bounds(settings);
    (settings.curriculum.adaptive_group_size
        && settings.curriculum.char_set_mode.draws_characters()
        && shortest < longest)
        .then_some((shortest, longest))
}

#[derive(Clone, Debug)]
pub struct CharSamplingConfig {
    pub error_weight_strength: f64,
    pub coverage_strength: f64,
    pub mixed_letters_percent: u32,
    pub char_set_mode: CharSetMode,
}

pub fn config_from_settings(settings: &TrainingSettings) -> CharSamplingConfig {
    CharSamplingConfig {
        error_weight_strength: settings.auto_level.error_weight_strength,
        coverage_strength: settings.auto_level.char_sampling_coverage_strength,
        mixed_letters_percent: settings.curriculum.mixed_letters_percent,
        char_set_mode: settings.curriculum.char_set_mode,
    }
}

pub fn belief_for(state: &CharSamplingState, character: char) -> CharBetaBelief {
    state.beliefs.get(&character).copied().unwrap_or_default()
}

pub fn beta_posterior_mean_error(belief: CharBetaBelief) -> f64 {
    let denom = belief.alpha + belief.beta;
    if denom <= 0.0 {
        0.5
    } else {
        belief.beta / denom
    }
}

fn coverage_factors(
    pool: &[char],
    session_sample_counts: &BTreeMap<char, u32>,
    coverage_strength: f64,
) -> BTreeMap<char, f64> {
    if coverage_strength <= 0.0 {
        return pool.iter().map(|c| (*c, 1.0)).collect();
    }
    let total_samples: u32 = pool
        .iter()
        .map(|c| session_sample_counts.get(c).copied().unwrap_or(0))
        .sum();
    let expected = if pool.is_empty() {
        0.0
    } else {
        f64::from(total_samples) / pool.len() as f64
    };
    let mut factors = BTreeMap::new();
    for character in pool {
        let seen = f64::from(session_sample_counts.get(character).copied().unwrap_or(0));
        let deficit = if expected > 0.0 {
            (expected - seen).max(0.0) / expected
        } else {
            1.0
        };
        factors.insert(*character, 1.0 + coverage_strength * deficit);
    }
    factors
}

/// How often the sampler takes a character to go wrong when it weighs it:
/// the belief's best guess and one standard deviation above it, at most
/// always.
///
/// Of two characters heard as often, the one missed more weighs more. Of
/// two with the same best guess, the one the trainer has heard less of
/// weighs more, its belief being wider: a new character, never heard, is
/// taken to go wrong about four times in five (a half and a spread of 0.29),
/// and comes round more until the trainer is sure of it. Within its prior
/// of one hit and one miss the bound only rises with the share missed, so a
/// worse character never weighs less than a better one heard as often.
///
/// A draw from the belief in its place, Thompson sampling, did neither on
/// average: weighed in proportion, a draw averages back to the best guess.
pub fn error_to_weigh(belief: CharBetaBelief) -> f64 {
    let (right, wrong) = (belief.alpha.max(0.0), belief.beta.max(0.0));
    let heard = right + wrong;
    if !(heard > 0.0 && heard.is_finite()) {
        return error_to_weigh(CharBetaBelief::default());
    }
    let guess = wrong / heard;
    let spread = (right * wrong / (heard * heard * (heard + 1.0))).sqrt();
    (guess + spread).min(1.0)
}

fn difficulty_factor(p_error: f64, error_weight_strength: f64) -> f64 {
    if error_weight_strength <= 0.0 {
        1.0
    } else {
        1.0 + p_error * error_weight_strength
    }
}

/// Each character's weight in the draw: one, and the error it is weighed
/// by ([`error_to_weigh`]) times the error-weight strength, times its
/// coverage lift for this session.
pub fn compute_raw_sampling_weights(
    pool: &[char],
    state: &CharSamplingState,
    config: &CharSamplingConfig,
) -> BTreeMap<char, f64> {
    let coverage = coverage_factors(pool, &state.session_sample_counts, config.coverage_strength);
    let mut weights = BTreeMap::new();
    for character in pool {
        let p_error = error_to_weigh(belief_for(state, *character));
        let coverage_factor = coverage.get(character).copied().unwrap_or(1.0);
        weights.insert(
            *character,
            difficulty_factor(p_error, config.error_weight_strength) * coverage_factor,
        );
    }
    weights
}

/// The chance each character of `pool` is the next one drawn, in the pool's
/// order, from the sampler's `weights`: what the sampler draws from and what
/// the Sampling tab shows, so the two cannot drift apart.
///
/// Mixed gives the letters and the digits the share of the draws asked for,
/// when the pool holds both, and each character its weight's part of its
/// own side's share: ten letters and two digits at 70% letters draw each
/// digit 15% of the time, not one time in twelve. Anywhere else a character
/// is drawn by its weight from the whole pool. A side whose weights come to
/// nothing is drawn from evenly.
pub(crate) fn draw_chances(
    pool: &[char],
    weights: &BTreeMap<char, f64>,
    config: &CharSamplingConfig,
) -> Vec<f64> {
    let is_digit = |c: &char| c.is_ascii_digit();
    let both = pool.iter().any(is_digit) && !pool.iter().all(is_digit);
    let letters_share = (config.char_set_mode == CharSetMode::Mixed && both)
        .then(|| f64::from(config.mixed_letters_percent.min(100)) / 100.0);
    let side = |c: &char| match letters_share {
        Some(share) if is_digit(c) => (true, 1.0 - share),
        Some(share) => (false, share),
        None => (false, 1.0),
    };
    let weight = |c: &char| {
        weights
            .get(c)
            .copied()
            .filter(|w| w.is_finite() && *w > 0.0)
            .unwrap_or(0.0)
    };
    // Each side's total weight and size, digits apart from the rest.
    let mut totals = [(0.0, 0usize); 2];
    for c in pool {
        let total = &mut totals[usize::from(side(c).0)];
        total.0 += weight(c);
        total.1 += 1;
    }
    pool.iter()
        .map(|c| {
            let (digit, share) = side(c);
            let (total, count) = totals[usize::from(digit)];
            if total > 0.0 {
                share * weight(c) / total
            } else {
                share / count.max(1) as f64
            }
        })
        .collect()
}

pub fn sample_training_group(
    pool: &[char],
    group_size: usize,
    state: &mut CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
) -> String {
    let weights = compute_raw_sampling_weights(pool, state, config);
    let chances = draw_chances(pool, &weights, config);
    let mut group = String::with_capacity(group_size);
    for _ in 0..group_size {
        group.extend(weighted_random_pick(pool, &chances, rng));
    }
    state.count_sent(&group);
    group
}

/// How many realistic callsigns are weighed against each other before one is
/// sent. Enough for the weak characters to show up, few enough that the mix
/// still sounds like a band rather than a drill.
const CALLSIGN_CANDIDATES: usize = 6;

/// A callsign, chosen the way a group is: several realistic candidates, and the
/// one whose characters the sampler wants most wins.
///
/// The structure is never bent to fit the sampler — every candidate is already
/// a well-formed call — so this can only decide *which* real callsign to send.
/// It scores them with [`compute_raw_sampling_weights`], the same weighting the
/// group sampler uses, so the error-weight and coverage settings mean exactly
/// what they mean everywhere else. With both turned off every weight is equal,
/// the first candidate wins, and the draw is plainly random.
pub(crate) fn generate_callsign_group(
    settings: &TrainingSettings,
    state: &mut CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
) -> String {
    let best = wanted_call(settings, state, config, rng, CallSpec::default()).text;
    state.count_sent(&best);
    best
}

/// The call a contest's next caller would have, as [`Sampler`] draws it for
/// a run of `settings`: from the countries the contest's rules have call
/// you, with a contest's few portables and stations abroad. Counted as sent.
pub(crate) fn generate_contest_call(
    settings: &TrainingSettings,
    state: &mut CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
) -> String {
    let side = Side::of_call(&settings.contest.my_call);
    let spec = settings.contest.contest_kind.rules().callers(side);
    let call = wanted_call(settings, state, config, rng, spec).text;
    state.count_sent(&call);
    call
}

/// The call Callsigns mode would send, from the countries `spec` admits, at
/// the callsign tier: of [`CALLSIGN_CANDIDATES`] drawn, the one whose
/// characters the sampler wants most. Nothing is counted as sent.
fn wanted_call(
    settings: &TrainingSettings,
    state: &CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
    spec: CallSpec,
) -> GeneratedCall {
    let tier = settings.level(LevelAxis::Callsign);
    // Field Day's mix of North America and DX is settled once per caller,
    // before the candidates: with a chance each, the most wanted of them
    // would be DX far more often than Field Day's callers are.
    let spec = CallSpec {
        countries: spec.countries.for_one_call(rng),
        ..spec
    };
    most_wanted(settings, state, config, rng, CALLSIGN_CANDIDATES, |rng| {
        generate_call(tier, spec, rng)
    })
}

/// How many words of the list are weighed against each other before one is
/// sent: the same balance as for callsigns, between leaning on the weak
/// characters and keeping the draw from the whole list.
const WORD_CANDIDATES: usize = 6;

/// A word from the list, chosen the way a callsign is: of a few drawn at
/// random, the one whose characters the sampler wants most.
pub(crate) fn generate_word_group(
    settings: &TrainingSettings,
    state: &mut CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
) -> String {
    let words = settings.words();
    let best = most_wanted(settings, state, config, rng, WORD_CANDIDATES, |rng| {
        words[rng.usize_in(0, words.len() - 1)].to_string()
    });
    state.count_sent(&best);
    best
}

/// Of `candidates` groups `draw` makes, the one whose characters the sampler
/// wants most, scored with [`compute_raw_sampling_weights`] so the
/// error-weight and coverage settings mean what they mean everywhere else.
/// When it wants them all alike, the first wins, and the draw is plainly
/// random.
fn most_wanted<R: Rng, T: AsRef<str>>(
    settings: &TrainingSettings,
    state: &CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut R,
    candidates: usize,
    mut draw: impl FnMut(&mut R) -> T,
) -> T {
    let pool = compute_char_pool(settings);
    let weights = compute_raw_sampling_weights(&pool, state, config);
    let appetite = |group: &str| {
        let mut total = 0.0;
        let mut count = 0.0;
        for ch in group.chars() {
            total += weights.get(&ch).copied().unwrap_or(1.0);
            count += 1.0;
        }
        if count > 0.0 { total / count } else { 0.0 }
    };
    let mut best = draw(rng);
    let mut best_appetite = appetite(best.as_ref());
    for _ in 1..candidates {
        let candidate = draw(rng);
        let candidate_appetite = appetite(candidate.as_ref());
        if candidate_appetite > best_appetite {
            best = candidate;
            best_appetite = candidate_appetite;
        }
    }
    best
}

/// How many times one group is sent before the answer window opens.
pub fn resolve_group_repeats(settings: &TrainingSettings, rng: &mut impl Rng) -> u32 {
    let min = settings.playback.group_repeat_min.clamp(
        crate::settings::GROUP_REPEAT_MIN,
        crate::settings::GROUP_REPEAT_MAX,
    );
    let max = settings
        .playback
        .group_repeat_max
        .clamp(min, crate::settings::GROUP_REPEAT_MAX);
    if settings.playback.link_group_repeat || min == max {
        return min;
    }
    rng.usize_in(min as usize, max as usize) as u32
}

/// The next group for `settings`, drawn by the sampler and counted as sent.
pub fn generate_training_group(
    settings: &TrainingSettings,
    state: &mut CharSamplingState,
    rng: &mut impl Rng,
) -> String {
    generate_group(settings, state, None, rng)
}

/// The next group, `length` characters long when one is given and otherwise
/// any length the range allows, evenly.
fn generate_group(
    settings: &TrainingSettings,
    state: &mut CharSamplingState,
    length: Option<usize>,
    rng: &mut impl Rng,
) -> String {
    // A callsign or a word is a group like any other once it exists — it is
    // only built differently. This is the single place the modes part company.
    let config = config_from_settings(settings);
    match settings.curriculum.char_set_mode {
        CharSetMode::Callsign | CharSetMode::Contest => {
            return generate_callsign_group(settings, state, &config, rng);
        }
        CharSetMode::Words => return generate_word_group(settings, state, &config, rng),
        CharSetMode::Koch | CharSetMode::Digits | CharSetMode::Mixed => {}
    }
    let pool = compute_char_pool(settings);
    let group_size = length.unwrap_or_else(|| {
        let (shortest, longest) = group_size_bounds(settings);
        rng.usize_in(shortest, longest)
    });
    sample_training_group(&pool, group_size, state, &config, rng)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::GroupResult;

    /// Teach `sampler` that `typed` was the answer to `sent`, played at
    /// `played` if it was measured.
    fn answer(sampler: &mut Sampler, sent: &str, typed: &str, played: Option<(f64, f64)>) {
        GroupSource::learn(
            sampler,
            Answered {
                sent,
                received: typed,
                copied: &CopyTally::of(sent, typed),
                played,
            },
        );
    }

    /// A contest's caller is drawn as Callsigns mode draws a call, and with
    /// a contest's spec keeps to its countries.
    #[test]
    fn a_contest_caller_is_drawn_as_callsigns_mode_draws_a_call() {
        use crate::callsign::{Countries, country_of};
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level = 4;
        let mut groups = Sampler::new(&[], &settings, 21);
        let mut callers = Sampler::new(&[], &settings, 21);
        for _ in 0..20 {
            let (group, _) = groups.next_group(&settings);
            let call = callers.next_call(&settings, CallSpec::default(), &|_| false);
            assert_eq!(call.text, group);
        }
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        let spec = CallSpec::contest(Countries::WVe);
        for _ in 0..200 {
            let call = callers.next_call(&settings, spec, &|_| false);
            assert!(
                matches!(country_of(&call.text), Some("K" | "VE")),
                "{call:?}"
            );
        }
    }

    /// Field Day's callers are DX as often as its entries are, however the
    /// sampler leans: the mix is settled once per caller, so the most wanted
    /// of the candidates cannot pick the DX one out of six chances, nor a
    /// learner weak at its rarer characters draw more of it.
    #[test]
    fn field_days_dx_share_holds_through_the_sampler() {
        use crate::callsign::Countries;
        use crate::contest::ContestKind;
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        settings.contest.contest_kind = ContestKind::FieldDay;
        let settings = settings.clamp();
        let spec = CallSpec::contest(Countries::FieldDay);
        let mut weak = Sampler::new(&[], &settings, 6);
        for _ in 0..20 {
            CallSource::learn(
                &mut weak,
                Answered {
                    sent: "JQZXY",
                    received: "",
                    copied: &CopyTally::of("JQZXY", ""),
                    played: None,
                },
            );
        }
        for mut sampler in [Sampler::new(&[], &settings, 5), weak] {
            let dx = (0..10_000)
                .map(|_| sampler.next_call(&settings, spec, &|_| false))
                .filter(|call| !Countries::NorthAmerica.admits(call.country))
                .count();
            // 0.9% of 10,000 is 90.
            assert!(dx > 55 && dx < 130, "{dx} of 10000 DX");
        }
    }

    /// Settings in Contest mode whose callers come at `low` to `high` WPM.
    fn contest_speeds(low: f64, high: f64) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        settings.playback.char_wpm_min = low;
        settings.playback.char_wpm_max = high;
        settings.playback.effective_wpm_min = low;
        settings.playback.effective_wpm_max = high;
        settings.clamp()
    }

    /// The character speeds of `n` callers in a row, as `sampler` draws
    /// them for `settings`.
    fn callers_speeds(sampler: &mut Sampler, settings: &TrainingSettings, n: usize) -> Vec<f64> {
        (0..n)
            .map(|_| {
                CallSource::next_speed(sampler, settings)
                    .expect("the callers' speed adapts")
                    .0
            })
            .collect()
    }

    fn mean(speeds: &[f64]) -> f64 {
        speeds.iter().sum::<f64>() / speeds.len() as f64
    }

    /// A contest's callers adapt by a switch of their own, on as it ships:
    /// group training's switch, carried over from Callsigns, says nothing of
    /// them, nor theirs of a group session's or of Listen's stream of
    /// groups, and one speed leaves nothing to adapt.
    #[test]
    fn a_contests_callers_adapt_by_a_switch_of_their_own() {
        let mut contest = contest_speeds(20.0, 35.0);
        assert!(adapts_speed(&contest), "the callers' is on as it ships");
        for group in [false, true] {
            contest.playback.adaptive_speed = group;
            contest.contest.contest_adapt_speed = false;
            assert!(!adapts_speed(&contest), "group training's {group}");
            assert_eq!(
                CallSource::next_speed(&mut Sampler::new(&[], &contest, 1), &contest),
                None
            );
            contest.contest.contest_adapt_speed = true;
            assert!(adapts_speed(&contest), "group training's {group}");
            assert_eq!(
                GroupSource::next_speed(&mut Sampler::new(&[], &contest, 1), &contest),
                None,
                "group training's {group}"
            );
        }
        let mut calls = contest.clone();
        calls.curriculum.char_set_mode = CharSetMode::Callsign;
        calls.playback.adaptive_speed = false;
        assert!(!adapts_speed(&calls), "Callsigns goes by group training's");
        let mut fixed = contest;
        fixed.playback.char_wpm_max = 20.0;
        fixed.playback.effective_wpm_max = 20.0;
        assert!(adaptive_speed_path(&fixed.clamp()).is_none(), "one speed");
    }

    /// A contest's next caller comes at a speed drawn from your copy speed,
    /// within the ranges: calls and numbers copied clean draw faster
    /// callers, misses slower ones, smoothly as the belief moves. A field
    /// from a short list, which only `learn` hears, moves nothing.
    #[test]
    fn a_contests_callers_speed_follows_the_calls_and_numbers_copied() {
        let settings = contest_speeds(15.0, 40.0);
        let after = |typed: &str, wpm: f64, copies: usize, tells: bool| {
            let mut sampler = Sampler::new(&[], &settings, 3);
            for _ in 0..copies {
                let copied = CopyTally::of("K5XYZ", typed);
                let answered = Answered {
                    sent: "K5XYZ",
                    received: typed,
                    copied: &copied,
                    played: Some((wpm, wpm)),
                };
                CallSource::learn(&mut sampler, answered);
                if tells {
                    CallSource::learn_speed(&mut sampler, answered);
                }
            }
            let speeds = callers_speeds(&mut sampler, &settings, 400);
            assert!(
                speeds.iter().all(|wpm| (15.0..=40.0).contains(wpm)),
                "{speeds:?}"
            );
            mean(&speeds)
        };
        let fresh = after("K5XYZ", 25.0, 0, true);
        assert_eq!(
            after("K5XYZ", 30.0, 20, false),
            fresh,
            "a name says nothing"
        );
        let few_clean = after("K5XYZ", 30.0, 3, true);
        let clean = after("K5XYZ", 30.0, 20, true);
        let missed = after("K7XQC", 22.0, 20, true);
        assert!(
            few_clean > fresh,
            "{few_clean} after 3 clean, {fresh} fresh"
        );
        assert!(clean > few_clean + 2.0, "{clean} after 20 clean");
        assert!(
            missed < fresh - 2.0,
            "{missed} after 20 missed, {fresh} fresh"
        );
    }

    /// A call or a number copied without its end is a miss, a busted call
    /// in the log: no copy span answers for a contest's lost end, as it does
    /// for a group's. Copied clean, the same calls draw faster callers.
    #[test]
    fn a_call_or_number_copied_without_its_tail_is_a_miss() {
        let settings = contest_speeds(15.0, 40.0);
        let after = |pairs: &[(&str, &str)]| {
            let mut sampler = Sampler::new(&[], &settings, 3);
            for _ in 0..20 {
                for (sent, typed) in pairs {
                    let copied = CopyTally::of(sent, typed);
                    CallSource::learn_speed(
                        &mut sampler,
                        Answered {
                            sent,
                            received: typed,
                            copied: &copied,
                            played: Some((30.0, 30.0)),
                        },
                    );
                }
            }
            mean(&callers_speeds(&mut sampler, &settings, 400))
        };
        let fresh = after(&[]);
        let busted = after(&[("K5XYZ", "K5XY"), ("214", "21")]);
        let clean = after(&[("K5XYZ", "K5XYZ"), ("214", "214")]);
        assert!(
            busted < fresh,
            "{busted} after 20 busted calls and numbers, {fresh} fresh"
        );
        assert!(clean > fresh + 2.0, "{clean} after 20 clean, {fresh} fresh");
        assert_eq!(after(&[("214", "")]), fresh, "an empty field says nothing");
    }

    /// Contest runs of ten QSOs at `timestamp`, `wpm`, each its call copied
    /// as `call` and its number as `nr`, as the history keeps them; with
    /// Show copy on when `assisted`, each copy a send more.
    fn contest_run(
        timestamp: u64,
        call: &str,
        nr: &str,
        wpm: f64,
        assisted: bool,
    ) -> SessionResult {
        use crate::contest::ContestLog;
        use crate::session::SessionTiming;
        let groups: Vec<GroupResult> = (0..10)
            .flat_map(|_| [("K5XYZ", call), ("214", nr)])
            .map(|(sent, received)| GroupResult {
                sent: sent.into(),
                received: received.into(),
                correct: sent == received,
            })
            .collect();
        let qsos: Vec<serde_json::Value> = (1..=10)
            .map(|n| serde_json::json!({ "t": n, "h": 0, "g": 2, "nr": n }))
            .collect();
        let layout = serde_json::json!({
            "v": 1, "kind": "wpx", "style": "single", "callers": [1, 1], "side": "",
            "myCountry": "DL", "myContinent": "EU", "myZone": 14, "fields": ["nr"],
            "limitS": 600, "runMs": 600_000, "pausedMs": 0, "myWpm": wpm,
            "assisted": assisted, "qsos": qsos,
        });
        SessionResult {
            timestamp,
            char_set_mode: CharSetMode::Contest,
            alphabet_fingerprint: if assisted {
                "contest:wpx:a"
            } else {
                "contest:wpx"
            }
            .into(),
            contest: Some(ContestLog(layout)),
            group_timings: groups
                .iter()
                .map(|_| SessionTiming {
                    time_to_complete_ms: 900.0,
                    char_wpm: Some(wpm),
                    effective_wpm: Some(wpm),
                    sends: 1 + u32::from(assisted),
                    ..SessionTiming::default()
                })
                .collect(),
            groups,
            ..SessionResult::empty_for_test()
        }
    }

    /// A run's callers start from the copy speed the contest runs in the
    /// history leave, by their calls and numbers: runs copied clean at 34
    /// WPM draw faster callers than none, and runs that missed at 20 slower,
    /// as do runs whose calls and numbers lost their ends at 30, read whole
    /// as the run learned them.
    /// An assisted run counts as any run does, blind to its sends as the
    /// copy-speed series is, its copies what you had by ear when each text
    /// showed; and group sessions, at whatever speed, say nothing of it.
    #[test]
    fn a_runs_callers_start_from_the_contest_runs_in_the_history() {
        let settings = contest_speeds(15.0, 40.0);
        let start = |history: &[SessionResult]| {
            mean(&callers_speeds(
                &mut Sampler::new(history, &settings, 5),
                &settings,
                400,
            ))
        };
        let runs = |call: &str, nr: &str, wpm: f64, assisted: bool| -> Vec<SessionResult> {
            (1..=3)
                .map(|at| contest_run(at, call, nr, wpm, assisted))
                .collect()
        };
        let fresh = start(&[]);
        let fast = start(&runs("K5XYZ", "214", 34.0, false));
        let slow = start(&runs("K7XQC", "819", 20.0, false));
        assert!(fast > fresh + 2.0, "{fast} after clean runs, {fresh} fresh");
        assert!(
            slow < fresh - 2.0,
            "{slow} after missed runs, {fresh} fresh"
        );
        let cut = start(&runs("K5XY", "21", 30.0, false));
        assert!(
            cut < fresh,
            "{cut} after runs that lost the ends, {fresh} fresh"
        );
        assert_eq!(start(&runs("K5XYZ", "214", 34.0, true)), fast, "assisted");
        let groups: Vec<SessionResult> = runs("K5XYZ", "214", 38.0, false)
            .into_iter()
            .map(|run| SessionResult {
                char_set_mode: CharSetMode::Callsign,
                alphabet_fingerprint: String::new(),
                contest: None,
                ..run
            })
            .collect();
        assert_eq!(start(&groups), fresh, "Callsigns sessions are their own");
    }

    /// A call to avoid is drawn again, a few times at most, and every call
    /// drawn counts as sent.
    #[test]
    fn a_call_to_avoid_is_drawn_again() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Contest;
        let spec = CallSpec::contest(crate::callsign::Countries::Any);
        let mut sampler = Sampler::new(&[], &settings, 8);
        let first = sampler.clone().next_call(&settings, spec, &|_| false);
        let other = sampler.next_call(&settings, spec, &|call| call == first.text);
        assert_ne!(other.text, first.text);
        // Avoiding everything still gives a call, after the last redraw.
        let draws = std::cell::Cell::new(0);
        let call = sampler.next_call(&settings, spec, &|_| {
            draws.set(draws.get() + 1);
            true
        });
        assert!(!call.text.is_empty());
        assert_eq!(draws.get(), CALL_REDRAWS);
        // The first draw was a copy's: only the two calls drawn here count.
        let counted: u32 = sampler.state.session_sample_counts.values().sum();
        assert_eq!(counted as usize, other.text.len() + call.text.len());
    }

    /// What a caller's call teaches the sampler is what each character was
    /// copied as, as for a group.
    #[test]
    fn a_contest_caller_teaches_the_characters() {
        let settings = TrainingSettings::default();
        let mut sampler = Sampler::new(&[], &settings, 1);
        CallSource::learn(
            &mut sampler,
            Answered {
                sent: "K5XYZ",
                received: "K5XYC",
                copied: &CopyTally::of("K5XYZ", "K5XYC"),
                played: Some((25.0, 25.0)),
            },
        );
        assert_eq!(belief_for(&sampler.state, 'Z').beta, 2.0);
        assert_eq!(belief_for(&sampler.state, 'K').alpha, 2.0);
    }

    #[test]
    fn group_length_respects_fixed_size() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        settings.curriculum.min_group_size = 3;
        settings.curriculum.max_group_size = 3;
        let mut state = CharSamplingState::default();
        let mut rng = SplitMix64::default();
        let group = generate_training_group(&settings, &mut state, &mut rng);
        assert_eq!(group.len(), 3);
        for ch in group.chars() {
            assert!(ch == 'K' || ch == 'M');
        }
    }

    #[test]
    fn generated_group_is_never_empty_for_koch() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        settings.curriculum.min_group_size = 1;
        settings.curriculum.max_group_size = 1;
        let mut state = CharSamplingState::default();
        let mut rng = SplitMix64::default();
        let group = generate_training_group(&settings, &mut state, &mut rng);
        assert!(!group.is_empty());
    }

    #[test]
    fn repeated_letters_update_belief_once_per_occurrence() {
        let mut next = CharSamplingState::default();
        next.learn(&CopyTally::of("KK", "KK"));
        let k = next.beliefs.get(&'K').copied().unwrap_or_default();
        assert!((k.alpha - (CHAR_SAMPLING_PRIOR_ALPHA + 2.0)).abs() < 1e-9);
        assert!((k.beta - CHAR_SAMPLING_PRIOR_BETA).abs() < 1e-9);
    }

    #[test]
    fn mismatch_increments_beta() {
        let mut next = CharSamplingState::default();
        next.learn(&CopyTally::of("KM", "KX"));
        let m = next.beliefs.get(&'M').copied().unwrap_or_default();
        assert!((m.beta - (CHAR_SAMPLING_PRIOR_BETA + 1.0)).abs() < 1e-9);
    }

    #[test]
    fn mixed_100_percent_never_samples_digits() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.mixed_letters_percent = 100;
        settings.curriculum.level = 5;
        settings.curriculum.digits_level = 9;
        settings.curriculum.min_group_size = 4;
        settings.curriculum.max_group_size = 4;
        let mut state = CharSamplingState::default();
        let mut rng = SplitMix64::default();
        for _ in 0..40 {
            let group = generate_training_group(&settings, &mut state, &mut rng);
            assert!(!group.is_empty());
            assert!(group.chars().all(|c| !c.is_ascii_digit()), "{group}");
        }
    }

    /// Returns a fixed sequence of "random" numbers, so every branch that
    /// depends on a draw can be aimed at deliberately.
    struct ScriptedRng {
        values: Vec<f64>,
        next: usize,
    }

    impl ScriptedRng {
        fn new(values: &[f64]) -> Self {
            Self {
                values: values.to_vec(),
                next: 0,
            }
        }
    }

    impl Rng for ScriptedRng {
        fn f64(&mut self) -> f64 {
            let value = self.values[self.next % self.values.len()];
            self.next += 1;
            value
        }
    }

    fn config(mode: CharSetMode, pct: u32) -> CharSamplingConfig {
        CharSamplingConfig {
            error_weight_strength: 3.0,
            coverage_strength: 1.0,
            mixed_letters_percent: pct,
            char_set_mode: mode,
        }
    }

    #[test]
    fn a_belief_with_no_mass_is_a_coin_flip() {
        assert_eq!(
            beta_posterior_mean_error(CharBetaBelief {
                alpha: 0.0,
                beta: 0.0
            }),
            0.5
        );
    }

    #[test]
    fn an_untouched_character_starts_on_the_flat_prior() {
        let state = CharSamplingState::default();
        let belief = belief_for(&state, 'K');
        assert_eq!(belief.alpha, CHAR_SAMPLING_PRIOR_ALPHA);
        assert_eq!(belief.beta, CHAR_SAMPLING_PRIOR_BETA);
        assert_eq!(beta_posterior_mean_error(belief), 0.5);
    }

    #[test]
    fn coverage_lifts_the_characters_that_have_been_seen_least() {
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        state.session_sample_counts.insert('K', 10);
        let config = config(CharSetMode::Koch, 70);
        let weights = compute_raw_sampling_weights(&pool, &state, &config);
        assert!(weights[&'M'] > weights[&'K']);

        // With coverage off both characters weigh the same.
        let mut flat = config;
        flat.coverage_strength = 0.0;
        let weights = compute_raw_sampling_weights(&pool, &state, &flat);
        assert_eq!(weights[&'M'], weights[&'K']);
    }

    #[test]
    fn a_fresh_session_gives_every_character_the_same_coverage_lift() {
        // Nothing sampled yet, so there is no deficit to compute a ratio from.
        let pool = ['K', 'M'];
        let state = CharSamplingState::default();
        let config = config(CharSetMode::Koch, 70);
        let weights = compute_raw_sampling_weights(&pool, &state, &config);
        assert_eq!(weights[&'K'], weights[&'M']);
        assert!(weights[&'K'] > 0.0);
    }

    #[test]
    fn error_weighting_can_be_switched_off() {
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        state.beliefs.insert(
            'M',
            CharBetaBelief {
                alpha: 1.0,
                beta: 20.0,
            },
        );
        let mut config = config(CharSetMode::Koch, 70);
        config.error_weight_strength = 0.0;
        config.coverage_strength = 0.0;
        let weights = compute_raw_sampling_weights(&pool, &state, &config);
        assert_eq!(weights[&'K'], weights[&'M']);
    }

    /// The share of the characters drawn that are `character`, over many
    /// groups from `state` with coverage off.
    fn share_drawn(pool: &[char], state: &CharSamplingState, character: char) -> f64 {
        let mut config = config(CharSetMode::Koch, 70);
        config.coverage_strength = 0.0;
        let mut rng = SplitMix64::new(3);
        let (mut hits, mut all) = (0usize, 0usize);
        for _ in 0..4_000 {
            let group = sample_training_group(pool, 5, &mut state.clone(), &config, &mut rng);
            hits += group.chars().filter(|c| *c == character).count();
            all += group.chars().count();
        }
        hits as f64 / all as f64
    }

    fn believed(heard: &[(char, f64, f64)]) -> CharSamplingState {
        let mut state = CharSamplingState::default();
        for (character, right, wrong) in heard {
            state.beliefs.insert(
                *character,
                CharBetaBelief {
                    alpha: CHAR_SAMPLING_PRIOR_ALPHA + right,
                    beta: CHAR_SAMPLING_PRIOR_BETA + wrong,
                },
            );
        }
        state
    }

    /// Heard as often, the character missed more is drawn more: at every
    /// amount of evidence, from one try to a thousand.
    #[test]
    fn a_worse_character_is_drawn_more_than_a_better_one_heard_as_often() {
        for heard in [1.0, 4.0, 40.0, 1_000.0] {
            for (worse, better) in [(0.6, 0.4), (0.3, 0.1), (1.0, 0.75), (0.05, 0.0)] {
                let state = believed(&[
                    ('M', heard * (1.0 - worse), heard * worse),
                    ('K', heard * (1.0 - better), heard * better),
                ]);
                let weighed = |c| error_to_weigh(belief_for(&state, c));
                assert!(
                    weighed('M') > weighed('K'),
                    "{heard} heard, {worse} against {better} wrong"
                );
            }
        }
        let state = believed(&[('M', 30.0, 10.0), ('K', 36.0, 4.0)]);
        let (m, k) = (
            share_drawn(&['K', 'M'], &state, 'M'),
            share_drawn(&['K', 'M'], &state, 'K'),
        );
        assert!(m > k + 0.05, "M {m:.3}, K {k:.3}");
    }

    /// With the same best guess, the character the trainer has heard less
    /// of is drawn more: a new one more than one heard a thousand times,
    /// whether that one is missed half the time or one time in ten. A
    /// Thompson draw in its place drew the unsure one no more, and in fact a
    /// little less, than the one it knew.
    #[test]
    fn an_uncertain_character_is_drawn_more_than_a_well_known_one_with_the_same_guess() {
        for (right, wrong) in [(1.0, 1.0), (9.0, 1.0), (1.0, 3.0)] {
            let state = believed(&[
                ('Q', right - 1.0, wrong - 1.0),
                ('K', 500.0 * right, 500.0 * wrong),
            ]);
            let guess = |c| beta_posterior_mean_error(belief_for(&state, c));
            assert!((guess('Q') - guess('K')).abs() < 1e-3);
            let weighed = |c| error_to_weigh(belief_for(&state, c));
            assert!(weighed('Q') > weighed('K') + 0.05, "{right}/{wrong}");
        }
        // Q never heard; the rest known a thousand times, missed half the
        // time and one time in ten.
        for known in [0.5, 0.1] {
            let pool: Vec<char> = "KMRSUAQ".chars().collect();
            let heard: Vec<(char, f64, f64)> = "KMRSUA"
                .chars()
                .map(|c| (c, 1_000.0 * (1.0 - known), 1_000.0 * known))
                .collect();
            let state = believed(&heard);
            let q = share_drawn(&pool, &state, 'Q');
            assert!(
                q > 1.0 / 7.0 + 0.01,
                "Q drawn {q:.3} of the time against six known {known} wrong"
            );
        }
    }

    /// What the help says the weighing does, at the strength it ships with:
    /// once both are well known, one missed half the time comes 2½ times as
    /// often as one never missed; one never heard, more than three times as
    /// often, and less than four.
    #[test]
    fn the_weighing_does_what_the_help_says() {
        let strength = TrainingSettings::default().auto_level.error_weight_strength;
        assert_eq!(strength, 3.0);
        let weight = |right: f64, wrong: f64| {
            let state = believed(&[('K', right, wrong)]);
            difficulty_factor(error_to_weigh(belief_for(&state, 'K')), strength)
        };
        let never_missed = weight(10_000.0, 0.0);
        let ratio = weight(5_000.0, 5_000.0) / never_missed;
        assert!((ratio - 2.5).abs() < 0.03, "{ratio}");
        let new = weight(0.0, 0.0) / never_missed;
        assert!(new > 3.0 && new < 4.0, "{new}");
        assert!((error_to_weigh(CharBetaBelief::default()) - 0.789).abs() < 1e-3);
        // A belief that cannot be read is taken as one never heard.
        let broken = CharBetaBelief {
            alpha: f64::NAN,
            beta: 0.0,
        };
        assert_eq!(
            error_to_weigh(broken),
            error_to_weigh(CharBetaBelief::default())
        );
    }

    /// Mixed gives letters and digits the share asked for, however many of
    /// each the pool holds, and each character its weight's part of its
    /// side: the chances the sampler draws from are the ones the Sampling
    /// tab shows.
    #[test]
    fn mixed_chances_give_each_side_its_share_whatever_the_pool() {
        let pool: Vec<char> = "KMURESNAPT01".chars().collect();
        let even: BTreeMap<char, f64> = pool.iter().map(|c| (*c, 1.0)).collect();
        let chances = draw_chances(&pool, &even, &config(CharSetMode::Mixed, 70));
        assert!((chances.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        for (c, chance) in pool.iter().zip(&chances) {
            let expected = if c.is_ascii_digit() { 0.15 } else { 0.07 };
            assert!((chance - expected).abs() < 1e-12, "{c}: {chance}");
        }
        // Within a side, by weight.
        let mut heavy = even.clone();
        heavy.insert('0', 3.0);
        let chances = draw_chances(&pool, &heavy, &config(CharSetMode::Mixed, 70));
        assert!((chances[10] - 0.225).abs() < 1e-12 && (chances[11] - 0.075).abs() < 1e-12);
        assert!((chances[0] - 0.07).abs() < 1e-12, "the letters keep theirs");

        // Outside Mixed, or with only one side in the pool, the whole pool
        // is drawn from by weight.
        let koch = draw_chances(&pool, &even, &config(CharSetMode::Koch, 70));
        assert!(
            koch.iter()
                .all(|chance| (chance - 1.0 / 12.0).abs() < 1e-12)
        );
        let letters = draw_chances(&['K', 'M'], &even, &config(CharSetMode::Mixed, 70));
        assert_eq!(letters, [0.5, 0.5]);
        // A share of none draws nothing of that side.
        let all_letters = draw_chances(&pool, &even, &config(CharSetMode::Mixed, 100));
        assert_eq!(&all_letters[10..], [0.0, 0.0]);

        // Weights that come to nothing, or are missing, are drawn evenly.
        let none = draw_chances(
            &['K', 'M', 'U'],
            &BTreeMap::new(),
            &config(CharSetMode::Koch, 70),
        );
        assert!(none.iter().all(|chance| (chance - 1.0 / 3.0).abs() < 1e-12));
        assert!(draw_chances(&[], &even, &config(CharSetMode::Mixed, 70)).is_empty());
    }

    #[test]
    fn sampling_an_empty_pool_returns_an_empty_group() {
        let mut state = CharSamplingState::default();
        let config = config(CharSetMode::Koch, 70);
        let mut rng = SplitMix64::default();
        let group = sample_training_group(&[], 3, &mut state, &config, &mut rng);
        assert!(group.is_empty());
        assert_eq!(state, CharSamplingState::default());
        let group = sample_training_group(&['K'], 0, &mut state, &config, &mut rng);
        assert!(group.is_empty());
    }

    #[test]
    fn a_mixed_group_follows_the_letter_share() {
        let pool = ['K', 'M', '0', '1'];
        let mut next = CharSamplingState::default();
        let config = config(CharSetMode::Mixed, 50);
        // Each character is a quarter of the draws: the lowest quarter is K,
        // the highest 1.
        let mut rng = ScriptedRng::new(&[0.1, 0.9]);
        let group = sample_training_group(&pool, 2, &mut next, &config, &mut rng);
        assert_eq!(group.chars().count(), 2);
        let mut chars = group.chars();
        assert!(!chars.next().unwrap().is_ascii_digit());
        assert!(chars.next().unwrap().is_ascii_digit());
        assert_eq!(next.session_sample_counts.values().sum::<u32>(), 2);
    }

    /// Settings nobody clamped can ask for groups of no characters at all;
    /// a group still has one.
    #[test]
    fn a_group_is_never_empty() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.min_group_size = 0;
        settings.curriculum.max_group_size = 0;
        let mut rng = SplitMix64::new(5);
        let group = generate_training_group(&settings, &mut CharSamplingState::default(), &mut rng);
        assert_eq!(group.chars().count(), 1);
    }

    #[test]
    fn a_mixed_group_with_only_letters_unlocked_still_fills() {
        // No digits in the pool, so the digit half falls back to the whole pool.
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        let config = config(CharSetMode::Mixed, 50);
        let mut rng = ScriptedRng::new(&[0.9, 0.0]);
        let group = sample_training_group(&pool, 2, &mut state, &config, &mut rng);
        assert_eq!(group.chars().count(), 2);
        assert!(group.chars().all(|c| c == 'K' || c == 'M'));
    }

    #[test]
    fn sample_counts_accumulate_across_groups() {
        let pool = ['K'];
        let mut state = CharSamplingState::default();
        let config = config(CharSetMode::Koch, 70);
        let mut rng = SplitMix64::default();
        sample_training_group(&pool, 2, &mut state, &config, &mut rng);
        sample_training_group(&pool, 2, &mut state, &config, &mut rng);
        assert_eq!(state.session_sample_counts.get(&'K'), Some(&4));
    }

    #[test]
    fn an_answer_moves_only_the_characters_that_were_sent() {
        let mut next = CharSamplingState::default();
        // "KM" copied as "KX": K right, M wrong, X never sent.
        next.learn(&CopyTally::of("KM", "KX"));
        assert_eq!(next.beliefs[&'K'].alpha, CHAR_SAMPLING_PRIOR_ALPHA + 1.0);
        assert_eq!(next.beliefs[&'K'].beta, CHAR_SAMPLING_PRIOR_BETA);
        assert_eq!(next.beliefs[&'M'].beta, CHAR_SAMPLING_PRIOR_BETA + 1.0);
        assert!(!next.beliefs.contains_key(&'X'));
    }

    /// Every session seeds the beliefs, a digits session's K as much as a
    /// Koch one's, each counting a little less than the one after it.
    #[test]
    fn history_from_earlier_sessions_seeds_the_beliefs() {
        let session = |timestamp, copies: &[(&str, &str)]| {
            let mut result = SessionResult {
                timestamp,
                ..SessionResult::empty_for_test()
            };
            for (sent, received) in copies {
                result.tally.add(sent, received);
            }
            result
        };
        let first = session(1, &[("KKKKK", "KKKXX")]);
        let second = session(2, &[("K", "K")]);
        let mut digits = session(3, &[("K", "X")]);
        digits.char_set_mode = CharSetMode::Digits;
        let state = CharSamplingState::from_history(&[digits, second, first]);
        let belief = state.beliefs[&'K'];
        let fade = SESSION_FADE;
        let near = |a: f64, b: f64| (a - b).abs() < 1e-12;
        assert!(near(
            belief.alpha,
            CHAR_SAMPLING_PRIOR_ALPHA + 3.0 * fade * fade + fade
        ));
        assert!(near(
            belief.beta,
            CHAR_SAMPLING_PRIOR_BETA + 2.0 * fade * fade + 1.0
        ));
        assert!(state.session_sample_counts.is_empty());
        // A session starts from all of it one session older.
        let once = [session(1, &[("KKKKK", "KKKXX")])];
        let started = CharSamplingState::for_session(&once).beliefs[&'K'];
        assert!(near(started.alpha, CHAR_SAMPLING_PRIOR_ALPHA + 3.0 * fade));
        assert!(near(started.beta, CHAR_SAMPLING_PRIOR_BETA + 2.0 * fade));
    }

    /// A character missed for weeks and copied clean ever since stops being
    /// drawn as a weak one: its misses fade as the sessions pass, halving
    /// every ten, and it ends up drawn about as often as one never missed.
    /// Counted as one sum, its old misses would hold it near one in three
    /// wrong for good.
    #[test]
    fn a_weakness_put_right_long_ago_stops_weighing() {
        let session = |timestamp, copies: &[(&str, &str)]| {
            let mut result = SessionResult {
                timestamp,
                ..SessionResult::empty_for_test()
            };
            for (sent, received) in copies {
                result.tally.add(sent, received);
            }
            result
        };
        let mut history: Vec<SessionResult> = (0..10)
            .map(|at| session(at, &[("MMMMMMMMMM", "MMMXXXXXXX"), ("KKKKK", "KKKKK")]))
            .collect();
        history.extend((10..50).map(|at| session(at, &[("MMMMM", "MMMMM"), ("KKKKK", "KKKKK")])));
        let state = CharSamplingState::from_history(&history);
        let error = |c| beta_posterior_mean_error(belief_for(&state, c));
        let summed = (70.0 + 1.0) / (300.0 + 2.0);
        assert!(
            error('M') < 0.07,
            "M: {:.3}, {summed:.3} summed",
            error('M')
        );
        let pool = ['K', 'M'];
        let config = config(CharSetMode::Koch, 70);
        let weights = compute_raw_sampling_weights(&pool, &state, &config);
        let chances = draw_chances(&pool, &weights, &config);
        assert!(chances[1] < 0.55, "M drawn {:.3} of the time", chances[1]);
        // Recent misses still count in full.
        history.push(session(50, &[("MMMMM", "XXXXX")]));
        let state = CharSamplingState::from_history(&history);
        assert!(beta_posterior_mean_error(belief_for(&state, 'M')) > 0.1);
    }

    /// The sampler a session draws from learns as the session goes: a
    /// character missed is drawn more after it.
    #[test]
    fn the_sampler_learns_from_each_answer() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        let mut sampler = Sampler::new(&[], &settings, 3);
        let (group, repeats) = sampler.next_group(&settings);
        assert!(!group.is_empty());
        assert_eq!(repeats, 1);
        answer(&mut sampler, "MMMMM", "XXXXX", None);
        let missed = sampler.state.beliefs[&'M'];
        assert_eq!(missed.beta, CHAR_SAMPLING_PRIOR_BETA + 5.0);
    }

    #[test]
    fn group_repeats_stay_inside_the_range() {
        let mut settings = TrainingSettings::default();
        let mut rng = SplitMix64::default();
        assert_eq!(resolve_group_repeats(&settings, &mut rng), 1);
        settings.playback.link_group_repeat = false;
        settings.playback.group_repeat_min = 2;
        settings.playback.group_repeat_max = 4;
        let mut seen_low = false;
        let mut seen_high = false;
        for _ in 0..400 {
            let n = resolve_group_repeats(&settings, &mut rng);
            assert!((2..=4).contains(&n));
            seen_low |= n == 2;
            seen_high |= n == 4;
        }
        assert!(seen_low && seen_high);
    }

    #[test]
    fn repeats_are_drawn_from_the_range_when_it_is_not_linked() {
        let mut settings = TrainingSettings::default();
        settings.playback.link_group_repeat = false;
        settings.playback.group_repeat_min = 2;
        settings.playback.group_repeat_max = 4;
        let mut low = ScriptedRng::new(&[0.0]);
        assert_eq!(resolve_group_repeats(&settings, &mut low), 2);
        let mut high = ScriptedRng::new(&[0.999_999]);
        assert_eq!(resolve_group_repeats(&settings, &mut high), 4);

        // Linked, the lower bound is used whatever the draw.
        settings.playback.link_group_repeat = true;
        let mut high = ScriptedRng::new(&[0.999_999]);
        assert_eq!(resolve_group_repeats(&settings, &mut high), 2);

        // Out-of-range settings are pulled back into the allowed span.
        settings.playback.link_group_repeat = false;
        settings.playback.group_repeat_min = 0;
        settings.playback.group_repeat_max = 99;
        let mut high = ScriptedRng::new(&[0.999_999]);
        assert_eq!(
            resolve_group_repeats(&settings, &mut high),
            crate::settings::GROUP_REPEAT_MAX
        );
    }

    #[test]
    fn group_size_stays_inside_the_configured_range() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 4;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 5;
        let mut state = CharSamplingState::default();
        let mut rng = SplitMix64::new(99);
        for _ in 0..50 {
            let group = generate_training_group(&settings, &mut state, &mut rng);
            let len = group.chars().count();
            assert!(
                (2..=5).contains(&len),
                "group {group:?} has {len} characters"
            );
        }
    }

    fn length_settings(adaptive: bool) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 10;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 10;
        settings.curriculum.adaptive_group_size = adaptive;
        settings
    }

    fn history_of(sent: &str, correct: bool) -> Vec<SessionResult> {
        (0..5)
            .map(|i| SessionResult {
                timestamp: i,
                groups: vec![
                    GroupResult {
                        sent: sent.to_string(),
                        received: if correct {
                            sent.to_string()
                        } else {
                            String::new()
                        },
                        correct,
                    };
                    20
                ],
                ..SessionResult::empty_for_test()
            })
            .collect()
    }

    /// Off means off: not one draw differs from the sampler before adaptive
    /// group size existed.
    #[test]
    fn with_adaptive_length_off_the_draws_are_what_they_always_were() {
        let settings = length_settings(false);
        let history = history_of("KMKMKMKM", true);
        let mut sampler = Sampler::new(&history, &settings, 42);
        let mut state = CharSamplingState::for_session(&history);
        let mut rng = SplitMix64::new(42);
        for _ in 0..50 {
            let expected = (
                generate_training_group(&settings, &mut state, &mut rng),
                resolve_group_repeats(&settings, &mut rng),
            );
            assert_eq!(sampler.next_group(&settings), expected);
        }
    }

    /// A history of long groups copied whole draws longer groups than one of
    /// short groups missed, and neither leaves the range.
    #[test]
    fn with_adaptive_length_on_the_lengths_follow_the_history() {
        let settings = length_settings(true);
        let mean_length = |history: &[SessionResult]| {
            let mut sampler = Sampler::new(history, &settings, 7);
            let lengths: Vec<usize> = (0..200)
                .map(|_| sampler.next_group(&settings).0.chars().count())
                .collect();
            assert!(lengths.iter().all(|length| (2..=10).contains(length)));
            lengths.iter().sum::<usize>() as f64 / lengths.len() as f64
        };
        let strong = mean_length(&history_of("KMKMKMKMK", true));
        let weak = mean_length(&history_of("KMK", false));
        assert!(strong > weak + 3.0, "strong {strong}, weak {weak}");
    }

    #[test]
    fn adaptive_length_needs_a_range_and_groups_that_are_drawn() {
        let mut settings = length_settings(true);
        assert_eq!(adaptive_bounds(&settings), Some((2, 10)));
        settings.curriculum.max_group_size = 2;
        assert_eq!(adaptive_bounds(&settings), None, "one length is no choice");
        settings.curriculum.max_group_size = 10;
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        assert_eq!(
            adaptive_bounds(&settings),
            None,
            "callsigns are as long as they are"
        );
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.adaptive_group_size = false;
        assert_eq!(adaptive_bounds(&settings), None);
    }

    /// The sampler learns lengths as the session goes, not only from the
    /// history it started with.
    #[test]
    fn misses_during_a_session_shorten_the_groups() {
        let settings = length_settings(true);
        // One session of six-character groups, all copied whole: a span
        // somewhere past six, and nothing yet to say how far.
        let history = history_of("KMKMKM", true)[..1].to_vec();
        let mut fresh = Sampler::new(&history, &settings, 9);
        let mut struggling = fresh.clone();
        for _ in 0..30 {
            let (group, _) = struggling.next_group(&settings);
            answer(&mut struggling, &group, "", None);
        }
        let mean = |sampler: &mut Sampler| {
            (0..100)
                .map(|_| sampler.next_group(&settings).0.chars().count())
                .sum::<usize>() as f64
                / 100.0
        };
        let (struggling, fresh) = (mean(&mut struggling), mean(&mut fresh));
        assert!(
            struggling < fresh - 1.0,
            "struggling {struggling}, fresh {fresh}"
        );
    }

    /// A learner for both beliefs at once. Each character is recognised with
    /// a chance that falls as the speed rises, and the group is held to its
    /// end with a chance that falls as it grows longer; a group not held is
    /// lost from a random point on. With `span_falls`, the span shrinks at
    /// speed as well, with the square root of the speed against 20 WPM.
    struct Learner {
        copy: f64,
        slope: f64,
        span: f64,
        spread: f64,
        span_falls: bool,
    }

    impl Learner {
        fn recognises(&self, speed: f64) -> f64 {
            let x = (0.9_f64 / 0.98 / (1.0 - 0.9 / 0.98)).ln()
                + (self.copy.ln() - speed.ln()) / self.slope;
            0.98 / (1.0 + (-x).exp())
        }

        fn span_at(&self, speed: f64) -> f64 {
            if self.span_falls {
                self.span * (20.0 / speed).sqrt()
            } else {
                self.span
            }
        }

        /// The speed at which they recognise `rate` of characters.
        fn speed_for(&self, rate: f64) -> f64 {
            let reference = (0.9_f64 / 0.98 / (1.0 - 0.9 / 0.98)).ln();
            let x = (rate / 0.98 / (1.0 - rate / 0.98)).ln();
            (self.copy.ln() - self.slope * (x - reference)).exp()
        }

        fn answer(&self, sent: &str, speed: f64, rng: &mut SplitMix64) -> String {
            let length = sent.chars().count();
            let held = 0.97 / (1.0 + ((length as f64 - self.span_at(speed)) / self.spread).exp());
            let kept = if rng.f64() < held {
                length
            } else {
                rng.usize_in(0, length.saturating_sub(1))
            };
            let recognised = self.recognises(speed);
            sent.chars()
                .take(kept)
                .map(|c| if rng.f64() < recognised { c } else { 'E' })
                .collect()
        }
    }

    /// What a run of both adaptive features made of one learner: over its
    /// second half, the share of characters right (of those a speed could
    /// change), of groups copied whole, and the mean speed and length; at
    /// its end, the speed the belief puts at the target and the span.
    struct Run {
        chars_right: f64,
        whole: f64,
        speed: f64,
        length: f64,
        found_speed: f64,
        found_span: f64,
    }

    fn run_both(learner: &Learner, groups: usize, seed: u64) -> Run {
        run_with(learner, groups, seed, false)
    }

    /// As [`run_both`], or, `naive`, with the speed learning from whole
    /// groups the way the span does: a group copied whole all right, any
    /// other all wrong.
    fn run_with(learner: &Learner, groups: usize, seed: u64, naive: bool) -> Run {
        let mut settings = length_settings(true);
        settings.playback.char_wpm_min = 10.0;
        settings.playback.char_wpm_max = 50.0;
        settings.playback.link_char_to_effective = true;
        settings.playback.adaptive_speed = true;
        let target = settings.curriculum.aim.chars_right();
        let mut sampler = Sampler::new(&[], &settings, seed);
        let mut rng = SplitMix64::new(seed ^ 0xA5A5);
        let (mut right, mut counted, mut whole, mut speeds, mut lengths) = (0, 0, 0, 0.0, 0);
        let half = groups / 2;
        for n in 0..groups {
            let (sent, _) = sampler.next_group(&settings);
            let played =
                GroupSource::next_speed(&mut sampler, &settings).expect("adaptive speed is on");
            let speed = crate::score::copy_speed(played.0, played.1);
            let typed = learner.answer(&sent, speed, &mut rng);
            if n >= half {
                if let Some((r, c)) = crate::speed::speed_evidence(&sent, &typed) {
                    right += r;
                    counted += c;
                }
                whole += usize::from(typed == sent);
                speeds += speed;
                lengths += sent.chars().count();
            }
            let all_wrong: String = sent.chars().map(|_| '#').collect();
            let heard = if naive && typed != sent {
                &all_wrong
            } else {
                &typed
            };
            GroupSource::learn(
                &mut sampler,
                Answered {
                    sent: &sent,
                    received: heard,
                    copied: &CopyTally::of(&sent, &typed),
                    played: Some(played),
                },
            );
        }
        let counted_groups = (groups - half) as f64;
        Run {
            chars_right: f64::from(right) / f64::from(counted.max(1)),
            whole: whole as f64 / counted_groups,
            speed: speeds / counted_groups,
            length: lengths as f64 / counted_groups,
            found_speed: sampler.speed.estimate(target).mean,
            found_span: sampler.span.estimate().mean,
        }
    }

    fn learners() -> [(&'static str, Learner); 4] {
        [
            (
                "copies 90% at 20, span 5",
                Learner {
                    copy: 20.0,
                    slope: 0.1,
                    span: 5.0,
                    spread: 0.7,
                    span_falls: false,
                },
            ),
            (
                "copies 90% at 30, span 7",
                Learner {
                    copy: 30.0,
                    slope: 0.12,
                    span: 7.0,
                    spread: 1.0,
                    span_falls: false,
                },
            ),
            (
                "copies 90% at 40, span 4",
                Learner {
                    copy: 40.0,
                    slope: 0.08,
                    span: 4.0,
                    spread: 0.7,
                    span_falls: false,
                },
            ),
            (
                "copies 90% at 30, span 7 at 20 WPM, shrinking with speed",
                Learner {
                    copy: 30.0,
                    slope: 0.12,
                    span: 7.0,
                    spread: 1.0,
                    span_falls: true,
                },
            ),
        ]
    }

    /// Both adaptive features together settle where each was asked to: the
    /// speed where the characters are copied at the target, the length where
    /// groups are copied whole about as often as theirs. Neither runs away
    /// with the other — no shortest groups at the top speed.
    #[test]
    fn adaptive_speed_and_length_settle_together() {
        let aim = crate::settings::Aim::Balanced;
        for (name, learner) in learners() {
            let run = run_both(&learner, 400, 7);
            let true_speed = learner.speed_for(aim.chars_right());
            assert!(
                (0.89..=0.98).contains(&run.chars_right),
                "{name}: {:.3} of characters right",
                run.chars_right
            );
            assert!(
                (run.found_speed / true_speed - 1.0).abs() < 0.15,
                "{name}: found {:.1} WPM for {true_speed:.1}",
                run.found_speed
            );
            assert!(
                (0.5..=0.9).contains(&run.whole),
                "{name}: {:.2} copied whole",
                run.whole
            );
            assert!(
                run.length > 2.2,
                "{name}: groups shrank to {:.1}",
                run.length
            );
            assert!(
                (11.0..49.0).contains(&run.speed),
                "{name}: speed ran to {:.1}",
                run.speed
            );
        }
    }

    /// The numbers the design note reports. `cargo test -p cw-core
    /// adaptive_speed_simulation -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn adaptive_speed_simulation() {
        let aim = crate::settings::Aim::Balanced;
        println!(
            "| Learner | Speed for 94% | Found | Sent at | Characters right | Span found | Length | Copied whole |"
        );
        println!("| --- | --- | --- | --- | --- | --- | --- | --- |");
        for (name, learner) in learners() {
            for seed in [7, 8, 9] {
                let run = run_both(&learner, 400, seed);
                println!(
                    "| {name} | {:.1} | {:.1} | {:.1} | {:.1}% | {:.1} | {:.1} | {:.0}% |",
                    learner.speed_for(aim.chars_right()),
                    run.found_speed,
                    run.speed,
                    run.chars_right * 100.0,
                    run.found_span,
                    run.length,
                    run.whole * 100.0
                );
            }
        }
        println!();
        println!("Naive: the speed learning from whole groups too.");
        println!(
            "| Learner | Speed for 94% | Sent at | Characters right | Length | Copied whole |"
        );
        println!("| --- | --- | --- | --- | --- | --- |");
        for (name, learner) in learners() {
            for seed in [7, 8, 9] {
                let run = run_with(&learner, 400, seed, true);
                println!(
                    "| {name} | {:.1} | {:.1} | {:.1}% | {:.1} | {:.0}% |",
                    learner.speed_for(aim.chars_right()),
                    run.speed,
                    run.chars_right * 100.0,
                    run.length,
                    run.whole * 100.0
                );
            }
        }
    }
}
