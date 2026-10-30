//! How fast you can copy: a Bayesian estimate of your copy speed, and each
//! group's speed drawn near its edge.
//!
//! It is the copy span's twin ([`crate::span`]), over speed rather than
//! length, and it learns from characters rather than whole groups. At a speed
//! `v` a character is copied right with probability
//!
//! ```text
//! P(right | v) = (1 − λ) · σ(ℓ + (ln V − ln v) / w)
//! ```
//!
//! `V` is your copy speed, the speed at which you copy nine characters in
//! ten (`ℓ` is what puts it there); `w` is how sharply accuracy falls away
//! as the speed rises, on a log scale, so ten percent faster means the same
//! at 15 WPM as at 40; `λ` is a small lapse rate. The speed is the one the
//! score uses ([`crate::score::copy_speed`]): with Farnsworth spacing, the
//! character speed and the spacing's pace together. The posterior over
//! `(V, w)` is held exactly, as log weights on a fixed grid.
//!
//! Each group counts once, whatever its length: its characters are not
//! independent of each other — a send too fast loses several together — so
//! a long group must not weigh like many short ones. It counts as the share
//! of its characters copied right, and only the characters a faster or slower
//! send could have changed ([`speed_tally`]). A group can end early two ways:
//! too long to hold, or broken off at a character not recognised. One group
//! cannot tell which, so the character copy broke off at counts against the
//! speed unless adaptive group size can still answer for it by sending
//! shorter groups; the rest of a lost end is the length's, which the copy
//! span learns from. So the span and the speed learn from different
//! evidence, and both can steer at once without pulling against each other.
//! A contest's call or number is as long as it is, and no span answers for
//! its end, so there every character counts, those lost off the end too.
//!
//! Each group's speed is drawn the way the span draws a length: one `(V, w)`
//! is drawn from the posterior, and the speeds whose accuracy comes closest
//! to the target are favoured, with a share drawn evenly from the whole
//! range so the slope keeps being learned. The speed moves along the line
//! from the slowest corner of the speed settings to the fastest: character
//! speed and spacing together, so a fixed character speed with a range of
//! spacing adapts the spacing alone, the way Farnsworth training does.

use std::sync::OnceLock;

use crate::aim::{self, Leaning, Spread};
use crate::alignment::{CopyTally, align_group, align_holding};
use crate::belief::{
    Estimate, EstimatePoint, FirstGuess, GridBelief, Tally, joined, pick_index, range_of,
    sessions_in_order,
};
use crate::callsign::parse_callsign;
use crate::contest::{Field, GroupCopy};
use crate::rng::Rng;
use crate::score::copy_speed;
use crate::session::SessionResult;
use crate::settings::{CharSetMode, TrainingSettings};
use crate::timing::station_speed_grid;

/// The chance of fumbling a character however slowly it was sent.
const LAPSE: f64 = 0.02;
/// The share of characters copied right at the speed the grid names.
const REFERENCE_RATE: f64 = 0.9;
/// The copy speeds the grid holds: 4 WPM up to about 80, two percent apart.
const SPEED_FROM: f64 = 4.0;
const LOG_STEP: f64 = 0.02;
const SPEED_POINTS: usize = 151;
/// How sharply accuracy falls away as the speed rises, in log speed a step
/// of the logit.
const SLOPES: [f64; 5] = [0.04, 0.08, 0.12, 0.2, 0.35];
const CELLS: usize = SPEED_POINTS * SLOPES.len();
/// How far the first guess at a copy speed may be off, in log speed: half
/// as fast again, or two thirds as fast.
const PRIOR_SD: f64 = 0.4;
/// How the speeds lean towards the aim, in log speed: within about five
/// percent of it, with 15% of them from a wider spread, about a fifth either
/// way, so the slope keeps being learned. Not from the whole range: a 48 WPM
/// group in a run at 18 teaches little and costs the run.
const LEANING: Leaning = Leaning {
    width: 0.05,
    variety: 0.15,
    spread: Spread::Around(0.2),
    reach: 0.15,
};
/// How far apart, in log speed, the speeds the draw chooses from are.
const CANDIDATE_STEP: f64 = 0.01;
/// The most speeds a forecast lists.
const FORECAST_ROWS_MAX: usize = 12;
/// How much of the posterior a forecast of the draw goes through: the rest
/// is cells too unlikely to move a share by a tenth of a percent.
const FORECAST_MASS: f64 = 0.999;

fn ln_speed_of(cell: usize) -> f64 {
    SPEED_FROM.ln() + (cell / SLOPES.len()) as f64 * LOG_STEP
}

fn slope_of(cell: usize) -> f64 {
    SLOPES[cell % SLOPES.len()]
}

fn logit(p: f64) -> f64 {
    (p / (1.0 - p)).ln()
}

/// What puts the copy speed at [`REFERENCE_RATE`]: the curve's logit there.
fn reference_logit() -> f64 {
    logit(REFERENCE_RATE / (1.0 - LAPSE))
}

/// The chance of copying a character right at `ln_speed`, for one cell.
fn p_right(ln_speed: f64, cell: usize) -> f64 {
    let x = reference_logit() + (ln_speed_of(cell) - ln_speed) / slope_of(cell);
    (1.0 - LAPSE) / (1.0 + (-x).exp())
}

/// The speed, in log WPM, at which one cell copies `rate` of characters.
fn ln_speed_at(rate: f64, cell: usize) -> f64 {
    let rate = rate.clamp(0.01, (1.0 - LAPSE) * 0.999);
    ln_speed_of(cell) - slope_of(cell) * (logit(rate / (1.0 - LAPSE)) - reference_logit())
}

/// The speed of grid point `bin`, in WPM.
fn grid_speed(bin: usize) -> f64 {
    (SPEED_FROM.ln() + bin as f64 * LOG_STEP).exp()
}

/// The grid point nearest a speed, for the table: speeds past either end of
/// the grid count as its end.
fn speed_bin(ln_speed: f64) -> usize {
    let bin = ((ln_speed - SPEED_FROM.ln()) / LOG_STEP).round();
    if bin.is_finite() {
        (bin.max(0.0) as usize).min(SPEED_POINTS - 1)
    } else {
        0
    }
}

/// `ln P(right)` and `ln P(wrong)` at every grid speed for every cell,
/// worked out once, so that replaying a history is additions rather than
/// logarithms.
fn log_chance_table() -> &'static [[f64; 2]] {
    static TABLE: OnceLock<Vec<[f64; 2]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..SPEED_POINTS)
            .flat_map(|bin| {
                let ln_speed = SPEED_FROM.ln() + bin as f64 * LOG_STEP;
                (0..CELLS).map(move |cell| {
                    let p = p_right(ln_speed, cell).clamp(1e-12, 1.0 - 1e-12);
                    [p.ln(), (1.0 - p).ln()]
                })
            })
            .collect()
    })
}

/// The characters of a group a faster or slower send could have changed,
/// each copied right or not, and those typed that were never sent: what a
/// group says about speed, and what auto level judges while group size
/// adapts.
///
/// Everything up to the last character typed counts, misheard, dropped or
/// added. A group can also end early, with characters lost off its end, and
/// that happens two ways: the group was longer than you could hold, or copy
/// broke off at a character you did not recognise and the rest went by while
/// you looked for it. One group cannot tell which. The rest of a lost end is
/// a consequence either way, and is the length's alone. The character copy
/// broke off at, the first one lost, counts as a miss unless adaptive group
/// size can still answer for it: `length_floor` is the shortest group it
/// draws while it adapts, and a group longer than that can be sent shorter.
/// A group already as short as the range allows, or one whose length does
/// not adapt, has nothing else to answer for its break. Left out, a learner
/// who stops typing at the first miss only ever showed the speed characters
/// copied right, and drove it to the top of the range while copying almost
/// nothing.
///
/// A group with nothing typed speaks of no character: neither what was
/// recognised nor where copy broke. While group size adapts it is the
/// length's alone, at the shortest length too: there a blank is as often a
/// slip as a send too fast, and a send too fast shows in what does get
/// typed, now that the character copy broke off at counts. Where the length
/// does not adapt, nothing else learns from it, and it counts as its first
/// character missed.
///
/// Nothing sent, or a blank the length answers for: nothing to count.
pub(crate) fn speed_tally(
    sent: &str,
    received: &str,
    length_floor: Option<usize>,
) -> Option<CopyTally> {
    let sent_chars: Vec<char> = sent.chars().map(|c| c.to_ascii_uppercase()).collect();
    if sent_chars.is_empty() {
        return None;
    }
    let length_answers = length_floor.is_some_and(|floor| sent_chars.len() > floor);
    let mut tally = CopyTally::default();
    let lost = if received.trim().is_empty() {
        if length_floor.is_some() {
            return None;
        }
        sent_chars.len()
    } else {
        let (held, lost) = align_holding(sent, received);
        for pair in held {
            match (pair.sent_char, pair.received_char) {
                (Some(ch), _) => {
                    let entry = tally.sent.entry(ch).or_default();
                    entry.total += 1;
                    entry.correct += u32::from(pair.matched);
                }
                (None, Some(ch)) => *tally.inserted.entry(ch).or_default() += 1,
                (None, None) => {}
            }
        }
        lost
    };
    if lost > 0 && !length_answers {
        let broke_at = sent_chars[sent_chars.len() - lost];
        tally.sent.entry(broke_at).or_default().total += 1;
    }
    (!tally.sent.is_empty() || !tally.inserted.is_empty()).then_some(tally)
}

/// What a group says about speed ([`speed_tally`]): of the characters a
/// faster or slower send could have changed, how many were copied right,
/// and how many count.
pub fn speed_evidence(
    sent: &str,
    received: &str,
    length_floor: Option<usize>,
) -> Option<(u32, u32)> {
    let tally = speed_tally(sent, received, length_floor)?;
    let right = tally.sent.values().map(|each| each.correct).sum::<u32>();
    let counted = tally.sent.values().map(|each| each.total).sum::<u32>()
        + tally.inserted.values().sum::<u32>();
    (counted > 0).then_some((right, counted))
}

/// What a contest's call or number says about speed: as [`speed_evidence`],
/// but every character of it counts, those lost off its end too. A call or
/// a number is as long as it is, so nothing else answers for an end lost: a
/// call logged without its last letter is a busted call, a miss. A field
/// left empty still says nothing, as the run keeps no word of whether it
/// was heard at all: a caller gone before its exchange reached you leaves
/// one, and so does a send too fast to follow.
pub fn whole_evidence(sent: &str, received: &str) -> Option<(u32, u32)> {
    if sent.is_empty() || received.trim().is_empty() {
        return None;
    }
    let aligned = align_group(sent, received);
    let right = aligned.iter().filter(|pair| pair.matched).count() as u32;
    Some((right, aligned.len() as u32))
}

/// A group's evidence as hits and misses: each character counted is its
/// share of the group as sent, so a group copied to its end counts once,
/// whatever its length, and one whose lost end is the length's counts for
/// as much of it as was heard. A miss that broke the copy off weighs as a
/// character misheard does: the characters it cost are not counted, so they
/// do not weigh it down. Counted as a share of what was counted instead, a
/// group broken off at its second character came out half wrong, and a
/// learner who stops at a miss was held to a far slower speed than their
/// own.
fn by_character(sent: &str, (right, counted): (u32, u32)) -> (f64, f64) {
    let length = sent.chars().count().max(1) as f64;
    (
        f64::from(right) / length,
        f64::from(counted.saturating_sub(right)) / length,
    )
}

/// A contest call's or number's evidence as hits and misses: the share of
/// it right and wrong, the whole of it counting once ([`whole_evidence`]).
fn as_whole((right, counted): (u32, u32)) -> (f64, f64) {
    let share = f64::from(right) / f64::from(counted.max(1));
    (share, 1.0 - share)
}

/// Whether a contest run's group says how fast you copy: a call or a number
/// (NR, ZN) does, a field from a short list does not, being half guessed.
pub(crate) fn tells_speed(copy: GroupCopy) -> bool {
    matches!(
        copy,
        GroupCopy::Call | GroupCopy::Field(Field::Nr | Field::Zn)
    )
}

/// Which of `session`'s groups say how fast you copy: every one of a group
/// session; of a contest run, the calls and the numbers ([`tells_speed`]),
/// as its log lays its groups out, each of them whole ([`whole_evidence`]).
/// A run whose log an older version dropped, or this one cannot read, keeps
/// the groups that read as calls.
fn speed_groups(session: &SessionResult) -> Vec<bool> {
    if session.char_set_mode != CharSetMode::Contest {
        return vec![true; session.groups.len()];
    }
    match session.contest_run() {
        Some(run) => run.group_copies().into_iter().map(tells_speed).collect(),
        None => session
            .groups
            .iter()
            .map(|group| parse_callsign(&group.sent).is_some())
            .collect(),
    }
}

/// The speed settings, seen as one line from the slowest corner to the
/// fastest: the character speed and the spacing's pace each move across
/// their range together, on a log scale, the spacing kept within its own
/// range wherever the characters are fast enough to allow it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedPath {
    char_wpm: (f64, f64),
    effective_wpm: (f64, f64),
    /// The spacing's range as set. The slowest corner may sit below it, as
    /// the spacing is never faster than the characters; the line comes up
    /// to it as soon as they allow, as a station drawn from the ranges
    /// does.
    spacing: (f64, f64),
}

/// How many times [`SpeedPath::voice_at`] halves the line to find a speed:
/// to well under a millionth of it.
const VOICE_HALVINGS: usize = 48;

impl SpeedPath {
    /// The line `settings` allow. The spacing is never faster than the
    /// characters, and tied to them it is the characters' own speed.
    pub fn of(settings: &TrainingSettings) -> Self {
        let p = &settings.playback;
        let char_wpm = (
            p.char_wpm_min.max(1.0),
            p.char_wpm_max.max(p.char_wpm_min).max(1.0),
        );
        let spacing = if p.link_char_to_effective {
            char_wpm
        } else {
            (
                p.effective_wpm_min.max(1.0),
                p.effective_wpm_max.max(p.effective_wpm_min).max(1.0),
            )
        };
        Self {
            char_wpm,
            effective_wpm: (spacing.0.min(char_wpm.0), spacing.1.min(char_wpm.1)),
            spacing,
        }
    }

    /// The slowest and fastest copy speed along it.
    pub fn bounds(&self) -> (f64, f64) {
        (
            copy_speed(self.char_wpm.0, self.effective_wpm.0),
            copy_speed(self.char_wpm.1, self.effective_wpm.1),
        )
    }

    /// Whether there is more than one speed on it.
    pub fn has_room(&self) -> bool {
        let (low, high) = self.bounds();
        high > low * (1.0 + 1e-6)
    }

    /// The character speed and the spacing's pace that send at `copy`,
    /// kept on the line.
    pub fn voice_at(&self, copy: f64) -> (f64, f64) {
        if !self.has_room() || !copy.is_finite() {
            return self.voice_along(0.0);
        }
        // The copy speed only rises along the line, so halving it finds
        // where it sends at `copy`, or the end nearer it.
        let (mut slower, mut faster) = (0.0, 1.0);
        for _ in 0..VOICE_HALVINGS {
            let middle = 0.5 * (slower + faster);
            let (char_wpm, effective_wpm) = self.voice_along(middle);
            if copy_speed(char_wpm, effective_wpm) < copy {
                slower = middle;
            } else {
                faster = middle;
            }
        }
        self.voice_along(0.5 * (slower + faster))
    }

    /// The character speed and the spacing's pace `along` the line, from
    /// the slowest corner at 0 to the fastest at 1: the spacing within its
    /// range wherever the characters are as fast as its slowest, and never
    /// faster than they are.
    fn voice_along(&self, along: f64) -> (f64, f64) {
        let between = |(from, to): (f64, f64)| (from.ln() + along * (to.ln() - from.ln())).exp();
        let char_wpm = between(self.char_wpm);
        let (slowest, fastest) = self.spacing;
        let effective_wpm =
            between(self.effective_wpm).clamp(slowest.min(char_wpm), fastest.min(char_wpm));
        (char_wpm, effective_wpm)
    }
}

/// One speed the next group can be sent at, as it sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedForecast {
    /// The speed in WPM, as the score counts it: with Farnsworth spacing,
    /// the characters and the spacing together.
    pub wpm: f64,
    /// The chance the next group is sent nearer this speed than any other
    /// in the forecast.
    pub drawn: f64,
    /// The share of characters copied right at this speed, and how far the
    /// posterior spreads it.
    pub right: f64,
    pub right_sd: f64,
}

/// What the history and this session say about your copy speed.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeedBelief {
    belief: GridBelief,
    /// The shortest group adaptive group size draws, while it adapts: a
    /// group longer than that answers for its own lost end
    /// ([`speed_tally`]).
    length_floor: Option<usize>,
}

impl SpeedBelief {
    /// A belief with nothing learned yet, its first guess at the copy speed
    /// being `centre` WPM.
    pub fn new(centre: f64) -> Self {
        let centre = centre.max(1.0).ln();
        let prior = (0..CELLS)
            .map(|cell| -0.5 * ((ln_speed_of(cell) - centre) / PRIOR_SD).powi(2))
            .collect();
        Self {
            belief: GridBelief::new(prior),
            length_floor: None,
        }
    }

    /// The belief the history leaves for `settings`, its first guess the
    /// middle of the speeds they allow.
    pub fn from_history(history: &[SessionResult], settings: &TrainingSettings) -> Self {
        Self::replay(history, settings, FirstGuess::Settings, |_, _, _| {})
    }

    /// The belief a new session starts from: the history's, all of it one
    /// session older.
    pub fn for_session(history: &[SessionResult], settings: &TrainingSettings) -> Self {
        let mut speed = Self::from_history(history, settings);
        speed.belief.fade();
        speed
    }

    /// The copy speed at `rate` of characters right after each session of
    /// the history that counts towards it, oldest first, with the slowest
    /// and the fastest speed it was read from up to then. Its first guess is
    /// the middle of what the first session sent, not of today's speeds, so
    /// that a change to the speeds does not redraw the past.
    pub fn history(
        history: &[SessionResult],
        settings: &TrainingSettings,
        rate: f64,
    ) -> Vec<EstimatePoint> {
        let mut points = Vec::new();
        let mut sent = None;
        Self::replay(
            history,
            settings,
            FirstGuess::FirstSession,
            |session, speed, speeds| {
                sent = joined(sent, speeds);
                if let Some(sent) = sent {
                    points.push(EstimatePoint {
                        timestamp: session.timestamp,
                        estimate: speed.estimate(rate),
                        sent,
                    });
                }
            },
        );
        points
    }

    /// Learn from the history in the order it was practised, its first
    /// guess as `first` says, telling `after` where the belief stands after
    /// each session and the slowest and fastest speed the session said
    /// anything at. Only sessions of the character set `settings` trains
    /// count: a callsign at 30 WPM is not a two-letter Koch group at 30 WPM.
    /// Groups cut short are read as `settings` would read them now: their
    /// lost ends the length's while group size adapts and the group could be
    /// shorter ([`speed_tally`]).
    fn replay(
        history: &[SessionResult],
        settings: &TrainingSettings,
        first: FirstGuess,
        mut after: impl FnMut(&SessionResult, &Self, Option<(f64, f64)>),
    ) -> Self {
        let floor = crate::sampling::adaptive_bounds(settings).map(|(shortest, _)| shortest);
        let sessions: Vec<_> = sessions_in_order(history, settings.curriculum.char_set_mode)
            .into_iter()
            .map(|session| (session, session_evidence(session, floor)))
            .collect();
        let first_sent = match first {
            FirstGuess::Settings => None,
            FirstGuess::FirstSession => sessions.iter().find_map(|(_, (_, sent))| *sent),
        };
        let (low, high) = first_sent.unwrap_or_else(|| SpeedPath::of(settings).bounds());
        let mut speed = Self::new((low * high).sqrt());
        speed.length_floor = floor;
        for (session, (tally, sent)) in sessions {
            speed.belief.fade();
            for (bin, hits, misses) in tally.rows() {
                speed.observe(bin, hits, misses);
            }
            after(session, &speed, sent);
        }
        speed
    }

    /// Groups sent at grid speed `bin`: `hits` and `misses` are their shares
    /// of characters right and wrong, added up.
    fn observe(&mut self, bin: usize, hits: f64, misses: f64) {
        if hits + misses <= 0.0 {
            return;
        }
        if let Some(row) = log_chance_table().get(bin * CELLS..(bin + 1) * CELLS) {
            self.belief.observe(row.iter().copied(), hits, misses);
        }
    }

    /// Learn from one group sent at `copy` WPM: what was sent, and what came
    /// back.
    pub fn learn(&mut self, sent: &str, received: &str, copy: f64) {
        let evidence = speed_evidence(sent, received, self.length_floor);
        self.learn_share(evidence.map(|counts| by_character(sent, counts)), copy);
    }

    /// Learn from a contest's call or number sent at `copy` WPM, every
    /// character of it counting ([`whole_evidence`]), as the history will
    /// read it once its run is stored.
    pub fn learn_whole(&mut self, sent: &str, received: &str, copy: f64) {
        self.learn_share(whole_evidence(sent, received).map(as_whole), copy);
    }

    /// Learn from what a copy says, its hits and misses as shares of a
    /// group, sent at `copy` WPM.
    fn learn_share(&mut self, evidence: Option<(f64, f64)>, copy: f64) {
        if let Some((hits, misses)) = evidence {
            self.observe(speed_bin(copy.max(1.0).ln()), hits, misses);
        }
    }

    /// The speed, in WPM, at which `rate` of characters are copied right.
    pub fn estimate(&self, rate: f64) -> Estimate {
        self.belief.estimate(|cell| ln_speed_at(rate, cell).exp())
    }

    /// The share of characters copied right at `wpm`, over everything the
    /// posterior allows, and how far it spreads it.
    pub fn right_at(&self, wpm: f64) -> Estimate {
        let ln_speed = wpm.max(1.0).ln();
        self.belief.estimate(|cell| p_right(ln_speed, cell))
    }

    /// How the next group's speed spreads over `speeds`, as [`Self::draw_speed`]
    /// draws it between `low` and `high` aiming for `target`: for each, the
    /// chance the speed drawn is nearer it than any other.
    pub fn drawn_near(&self, speeds: &[f64], low: f64, high: f64, target: f64) -> Vec<f64> {
        let mut drawn = vec![0.0; speeds.len()];
        if speeds.is_empty() {
            return drawn;
        }
        let (low, high) = (low.min(high).max(1.0), high.max(low).max(1.0));
        let candidates = candidates(low, high);
        if candidates.len() < 2 {
            drawn[nearest(speeds, low)] = 1.0;
            return drawn;
        }
        let row_of: Vec<usize> = candidates
            .iter()
            .map(|ln_speed| nearest(speeds, ln_speed.exp().clamp(low, high)))
            .collect();
        let mut cells: Vec<(usize, f64)> =
            self.belief.posterior().into_iter().enumerate().collect();
        cells.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut covered = 0.0;
        for (cell, p) in cells {
            if covered >= FORECAST_MASS {
                break;
            }
            covered += p;
            let right: Vec<f64> = candidates.iter().map(|v| p_right(*v, cell)).collect();
            let weights = aim::lean(&candidates, &right, target, &LEANING);
            for (weight, row) in weights.into_iter().zip(&row_of) {
                drawn[*row] += p * weight;
            }
        }
        if covered > 0.0 {
            for share in &mut drawn {
                *share /= covered;
            }
        }
        drawn
    }

    /// The next group's copy speed, between `low` and `high` WPM, aiming for
    /// `target` of characters right.
    pub fn draw_speed(&self, low: f64, high: f64, target: f64, rng: &mut impl Rng) -> f64 {
        let (low, high) = (low.min(high).max(1.0), high.max(low).max(1.0));
        let candidates = candidates(low, high);
        if candidates.len() < 2 {
            return low;
        }
        let cell = self.belief.draw(rng);
        let right: Vec<f64> = candidates.iter().map(|v| p_right(*v, cell)).collect();
        let weights = aim::lean(&candidates, &right, target, &LEANING);
        candidates
            .get(pick_index(&weights, rng))
            .map_or(low, |ln_speed| ln_speed.exp().clamp(low, high))
    }
}

/// What `session` says about speed, its groups cut short read with `floor`
/// as the shortest group adaptive group size draws ([`speed_tally`]): the
/// hits and misses at each grid speed, and the slowest and the fastest
/// speed they were sent at, kept to the grid.
fn session_evidence(
    session: &SessionResult,
    floor: Option<usize>,
) -> (Tally<usize>, Option<(f64, f64)>) {
    let contest = session.char_set_mode == CharSetMode::Contest;
    let evidence = |sent: &str, received: &str| {
        if contest {
            whole_evidence(sent, received).map(as_whole)
        } else {
            speed_evidence(sent, received, floor).map(|counts| by_character(sent, counts))
        }
    };
    let mut tally = Tally::new();
    let mut speeds = Vec::new();
    let counted = session.groups.iter().zip(speed_groups(session));
    for (index, (group, counts)) in counted.enumerate() {
        if !counts {
            continue;
        }
        let timing = session.group_timings.get(index);
        let char_wpm = timing
            .and_then(|timing| timing.char_wpm)
            .unwrap_or(session.char_wpm);
        let effective_wpm = timing
            .and_then(|timing| timing.effective_wpm)
            .unwrap_or(session.effective_wpm);
        if let Some((hits, misses)) = evidence(&group.sent, &group.received) {
            let copy = copy_speed(char_wpm, effective_wpm);
            tally.add(speed_bin(copy.ln()), hits, misses);
            if copy.is_finite() {
                speeds.push(copy.clamp(grid_speed(0), grid_speed(SPEED_POINTS - 1)));
            }
        }
    }
    (tally, range_of(speeds))
}

/// Round speeds from about `low` to about `high`, 1, 2, 5, 10 or 20 WPM
/// apart: as close together as keeps them to a dozen.
pub(crate) fn forecast_speeds(low: f64, high: f64) -> Vec<f64> {
    if !(low.is_finite() && high.is_finite()) {
        return Vec::new();
    }
    let (low, high) = (low.min(high).max(1.0), high.max(low).max(1.0));
    let ends = |step: f64| {
        let first = (low / step).round().max(1.0);
        (first, (high / step).round().max(first))
    };
    let fits = |step: f64| {
        let (first, last) = ends(step);
        last - first < FORECAST_ROWS_MAX as f64
    };
    let step = [1.0, 2.0, 5.0, 10.0, 20.0]
        .into_iter()
        .find(|step| fits(*step))
        .unwrap_or_else(|| ((high - low) / (FORECAST_ROWS_MAX - 1) as f64).ceil());
    let (first, last) = ends(step);
    (0..=(last - first) as usize)
        .map(|n| (first + n as f64) * step)
        .collect()
}

/// Which of `speeds` is nearest `wpm`: the first, if two are as near.
fn nearest(speeds: &[f64], wpm: f64) -> usize {
    speeds
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (*a - wpm).abs().total_cmp(&(*b - wpm).abs()))
        .map_or(0, |(index, _)| index)
}

/// How the speeds stations are sent at spread over `speeds`, when adaptive
/// speed does not choose them: for each, the share of stations nearer it
/// than any other.
pub(crate) fn station_shares(settings: &TrainingSettings, speeds: &[f64]) -> Vec<f64> {
    if speeds.is_empty() {
        return Vec::new();
    }
    let mut stations = vec![0_usize; speeds.len()];
    for (char_wpm, effective_wpm) in station_speed_grid(settings) {
        stations[nearest(speeds, copy_speed(char_wpm, effective_wpm))] += 1;
    }
    let total = stations.iter().sum::<usize>().max(1) as f64;
    stations
        .into_iter()
        .map(|count| count as f64 / total)
        .collect()
}

/// The speeds the draw chooses from, in log WPM: from `low` to `high`, a
/// percent apart, both ends among them.
fn candidates(low: f64, high: f64) -> Vec<f64> {
    let (from, to) = (low.ln(), high.ln());
    let width = to - from;
    if width.is_nan() || width <= 1e-9 {
        return vec![from];
    }
    let steps = (width / CANDIDATE_STEP).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|step| from + width * step as f64 / steps as f64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::belief::EstimateReading;
    use crate::contest::ContestLog;
    use crate::rng::SplitMix64;
    use crate::session::{GroupResult, SessionTiming};

    /// A learner whose copy speed is `copy` WPM with slope `slope`: copies
    /// each character with the model's own chance.
    fn learner_right(copy: f64, slope: f64, speed: f64) -> f64 {
        let x = reference_logit() + (copy.ln() - speed.ln()) / slope;
        (1.0 - LAPSE) / (1.0 + (-x).exp())
    }

    /// Five characters sent at `speed`, copied by that learner.
    fn copy_group(copy: f64, slope: f64, speed: f64, rng: &mut SplitMix64) -> (String, String) {
        let sent = "KMRSU".to_string();
        let received: String = sent
            .chars()
            .map(|c| {
                if rng.f64() < learner_right(copy, slope, speed) {
                    c
                } else {
                    'E'
                }
            })
            .collect();
        (sent, received)
    }

    fn settings(low: f64, high: f64) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = low;
        settings.playback.char_wpm_max = high;
        settings.playback.link_char_to_effective = true;
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings
    }

    #[test]
    fn with_nothing_learned_the_guess_is_the_middle_of_the_range() {
        let belief = SpeedBelief::from_history(&[], &settings(16.0, 36.0));
        // The middle on a log scale, at the reference accuracy.
        let estimate = belief.estimate(REFERENCE_RATE);
        assert!((estimate.mean - 24.0).abs() < 3.0, "{estimate:?}");
        assert!(
            estimate.sd > 5.0,
            "a first guess is a wide one: {estimate:?}"
        );
    }

    #[test]
    fn clean_copy_raises_the_speed_and_misses_lower_it() {
        let start = SpeedBelief::new(20.0);
        let mut clean = start.clone();
        let mut sloppy = start.clone();
        for _ in 0..10 {
            clean.learn("KMRSU", "KMRSU", 24.0);
            sloppy.learn("KMRSU", "KXRXU", 18.0);
        }
        let at = |belief: &SpeedBelief| belief.estimate(REFERENCE_RATE).mean;
        assert!(at(&clean) > at(&start), "{} vs {}", at(&clean), at(&start));
        assert!(
            at(&sloppy) < at(&start),
            "{} vs {}",
            at(&sloppy),
            at(&start)
        );
    }

    /// What was misheard, dropped or added inside a group is the speed's.
    /// Of an end lost, the character copy broke off at is the speed's too,
    /// unless adaptive group size can still send the group shorter: then
    /// the whole end is the length's, a blank included. The rest of an end
    /// is the length's either way.
    #[test]
    fn a_group_cut_short_counts_its_break_unless_the_length_answers_for_it() {
        // Group size does not adapt: nothing else answers for an end.
        let fixed = |received| speed_evidence("KMRSU", received, None);
        assert_eq!(fixed("KMRSU"), Some((5, 5)));
        assert_eq!(fixed("KMR"), Some((3, 4)), "broke off at S");
        assert_eq!(fixed("KXRSU"), Some((4, 5)), "misheard one");
        assert_eq!(fixed("KRSU"), Some((4, 5)), "dropped one inside");
        assert_eq!(fixed("KMRSUE"), Some((5, 6)), "added one");
        assert_eq!(fixed("KE"), Some((1, 3)), "one wrong, broke off at R");
        assert_eq!(fixed(""), Some((0, 1)), "broke off at the first");
        assert_eq!(speed_evidence("kmrsu", "KMR", None), Some((3, 4)), "case");
        assert_eq!(speed_evidence("", "K", None), None, "nothing sent");
        let tally = speed_tally("KMRSU", "KMR", None).expect("a tally");
        assert_eq!(tally.sent[&'S'].total, 1);
        assert_eq!(tally.sent[&'S'].correct, 0);
        assert!(!tally.sent.contains_key(&'U'), "the rest is the length's");

        // Group size adapts from two: a group of five could be shorter.
        let adapts = |received| speed_evidence("KMRSU", received, Some(2));
        assert_eq!(adapts("KMR"), Some((3, 3)), "lost the end");
        assert_eq!(adapts("KE"), Some((1, 2)), "lost the end, one wrong");
        assert_eq!(adapts(""), None, "a blank is the length's");
        assert_eq!(adapts("KXRSU"), Some((4, 5)), "inside is the speed's still");
        // At the shortest it draws, the length can do no more.
        assert_eq!(speed_evidence("KM", "K", Some(2)), Some((1, 2)));
        assert_eq!(
            speed_evidence("KM", "", Some(2)),
            None,
            "a blank is the length's while it adapts"
        );

        // A prefix copied clean, of a group the length answers for, raises
        // the speed; broken off where nothing else answers, it lowers it.
        let start = SpeedBelief::new(20.0);
        let at = |belief: &SpeedBelief| belief.estimate(REFERENCE_RATE).mean;
        let mut cut = start.clone();
        cut.length_floor = Some(2);
        cut.learn("KMRSUAB", "KMR", 22.0);
        assert!(at(&cut) >= at(&start));
        let mut broke = start.clone();
        for _ in 0..5 {
            broke.learn("KMRSUAB", "KMR", 22.0);
        }
        assert!(at(&broke) < at(&start), "{} vs {}", at(&broke), at(&start));
    }

    /// A belief built for settings knows which ends the length answers for,
    /// and reads the history so: a history of groups broken off early says
    /// less against the speed while group size adapts.
    #[test]
    fn the_history_is_read_as_the_settings_answer_for_lost_ends() {
        let mut adaptive = settings(15.0, 40.0);
        adaptive.curriculum.min_group_size = 2;
        adaptive.curriculum.max_group_size = 8;
        adaptive.curriculum.adaptive_group_size = true;
        let fixed = settings(15.0, 40.0);
        assert_eq!(
            SpeedBelief::from_history(&[], &adaptive).length_floor,
            Some(2)
        );
        assert_eq!(SpeedBelief::from_history(&[], &fixed).length_floor, None);
        let broken: Vec<SessionResult> = (1..=4).map(|at| stored(at, 30.0, "KM")).collect();
        let speed = |settings: &TrainingSettings| {
            SpeedBelief::from_history(&broken, settings)
                .estimate(REFERENCE_RATE)
                .mean
        };
        assert!(speed(&adaptive) > speed(&fixed) + 2.0);
    }

    /// A contest's call or number counts whole: an end lost is a miss, as no
    /// span answers for it, and only an empty field says nothing.
    #[test]
    fn a_call_or_number_cut_short_is_a_miss() {
        assert_eq!(whole_evidence("K5XYZ", "K5XYZ"), Some((5, 5)));
        assert_eq!(
            whole_evidence("K5XYZ", "K5XY"),
            Some((4, 5)),
            "lost the end"
        );
        assert_eq!(whole_evidence("214", "2"), Some((1, 3)));
        assert_eq!(whole_evidence("K5XYZ", "K7XYZ"), Some((4, 5)), "misheard");
        assert_eq!(whole_evidence("K5XYZ", "K5XYZE"), Some((5, 6)), "added one");
        assert_eq!(whole_evidence("214", " "), None, "an empty field");
        assert_eq!(whole_evidence("", "K"), None);
        let start = SpeedBelief::new(20.0);
        let mut cut = start.clone();
        cut.learn_whole("K5XYZAB", "K5X", 22.0);
        assert!(cut.estimate(REFERENCE_RATE).mean < start.estimate(REFERENCE_RATE).mean);
    }

    /// A simulated learner is found, and copies near the target at the
    /// speeds drawn for them.
    #[test]
    fn a_simulated_learner_is_found_and_copies_near_the_target() {
        let (copy, slope, target) = (26.0, 0.12, 0.94);
        let mut belief = SpeedBelief::new(18.0);
        let mut rng = SplitMix64::new(11);
        let (mut right, mut sent) = (0usize, 0usize);
        for n in 0..300 {
            let speed = belief.draw_speed(10.0, 50.0, target, &mut rng);
            let (group, copied) = copy_group(copy, slope, speed, &mut rng);
            if n >= 150 {
                right += group
                    .chars()
                    .zip(copied.chars())
                    .filter(|(a, b)| a == b)
                    .count();
                sent += group.chars().count();
            }
            belief.learn(&group, &copied, speed);
        }
        let found = belief.estimate(REFERENCE_RATE).mean;
        assert!((found - copy).abs() < 3.0, "found {found:.1} for {copy}");
        let rate = right as f64 / sent as f64;
        assert!(
            (0.90..=0.97).contains(&rate),
            "copied {rate:.3} for {target}"
        );
    }

    /// Too easy a range leans on its fastest speeds; too hard, its slowest.
    #[test]
    fn a_range_too_easy_or_too_hard_leans_on_the_right_end() {
        let mut fast_learner = SpeedBelief::new(40.0);
        let mut slow_learner = SpeedBelief::new(10.0);
        for _ in 0..40 {
            fast_learner.learn("KMRSU", "KMRSU", 40.0);
            slow_learner.learn("KMRSU", "KXXSU", 12.0);
        }
        let mut rng = SplitMix64::new(3);
        let mean = |belief: &SpeedBelief, rng: &mut SplitMix64| {
            (0..400)
                .map(|_| belief.draw_speed(15.0, 25.0, 0.94, rng))
                .sum::<f64>()
                / 400.0
        };
        assert!(mean(&fast_learner, &mut rng) > 23.5);
        assert!(mean(&slow_learner, &mut rng) < 16.5);
    }

    #[test]
    fn speeds_stay_inside_the_range_whatever_the_generator_does() {
        struct Stuck(f64);
        impl Rng for Stuck {
            fn f64(&mut self) -> f64 {
                self.0
            }
        }
        let belief = SpeedBelief::new(20.0);
        for stuck in [0.0, 0.999_999] {
            let speed = belief.draw_speed(18.0, 25.0, 0.94, &mut Stuck(stuck));
            assert!((18.0..=25.0 + 1e-9).contains(&speed), "{speed}");
        }
        // One speed is all there is.
        assert_eq!(belief.draw_speed(20.0, 20.0, 0.94, &mut Stuck(0.5)), 20.0);
    }

    /// The line runs from the slowest corner of the settings to the fastest:
    /// with a fixed character speed only the spacing moves.
    #[test]
    fn the_speed_settings_are_one_line_from_slowest_to_fastest() {
        let mut farnsworth = settings(20.0, 20.0);
        farnsworth.playback.link_char_to_effective = false;
        farnsworth.playback.effective_wpm_min = 8.0;
        farnsworth.playback.effective_wpm_max = 15.0;
        let path = SpeedPath::of(&farnsworth);
        let (low, high) = path.bounds();
        assert!((low - copy_speed(20.0, 8.0)).abs() < 1e-9);
        assert!((high - copy_speed(20.0, 15.0)).abs() < 1e-9);
        let (char_wpm, effective_wpm) = path.voice_at((low * high).sqrt());
        assert!((char_wpm - 20.0).abs() < 1e-9, "the characters stay put");
        assert!(effective_wpm > 8.0 && effective_wpm < 15.0);
        assert!((copy_speed(char_wpm, effective_wpm) - (low * high).sqrt()).abs() < 1e-6);

        let tied = SpeedPath::of(&settings(15.0, 30.0));
        let near =
            |(a, b): (f64, f64), (c, d): (f64, f64)| (a - c).abs() < 1e-9 && (b - d).abs() < 1e-9;
        assert!(near(tied.voice_at(22.0), (22.0, 22.0)));
        assert!(near(tied.voice_at(99.0), (30.0, 30.0)), "kept on the line");
        assert!(!SpeedPath::of(&settings(20.0, 20.0)).has_room());

        // Spacing set from 20, above the slowest characters: the line starts
        // with the spacing at the characters' own speed, and keeps it within
        // its range once they are as fast as its slowest, as a station drawn
        // from the ranges is.
        let mut spaced = settings(15.0, 40.0);
        spaced.playback.link_char_to_effective = false;
        spaced.playback.effective_wpm_min = 20.0;
        spaced.playback.effective_wpm_max = 25.0;
        let path = SpeedPath::of(&spaced.clamp());
        let (low, high) = path.bounds();
        assert!((low - 15.0).abs() < 1e-9 && (high - copy_speed(40.0, 25.0)).abs() < 1e-9);
        let mut last = (0.0, 0.0);
        for step in 0..=60 {
            let copy = low * (high / low).powf(f64::from(step) / 60.0);
            let (char_wpm, effective_wpm) = path.voice_at(copy);
            assert!(
                (copy_speed(char_wpm, effective_wpm) - copy).abs() < 1e-6,
                "{copy}"
            );
            assert!(
                effective_wpm >= 20.0_f64.min(char_wpm) - 1e-9
                    && effective_wpm <= 25.0_f64.min(char_wpm) + 1e-9,
                "{copy}: {char_wpm}/{effective_wpm}"
            );
            assert!(char_wpm >= last.0 && effective_wpm >= last.1, "{copy}");
            last = (char_wpm, effective_wpm);
        }
    }

    fn stored(timestamp: u64, speed: f64, received: &str) -> SessionResult {
        SessionResult {
            timestamp,
            char_set_mode: CharSetMode::Koch,
            groups: vec![GroupResult {
                sent: "KMRSU".into(),
                received: received.into(),
                correct: received == "KMRSU",
            }],
            group_timings: vec![SessionTiming {
                time_to_complete_ms: 500.0,
                char_wpm: Some(speed),
                effective_wpm: Some(speed),
                sends: 1,
                ..SessionTiming::default()
            }],
            ..SessionResult::empty_for_test()
        }
    }

    /// The copy speed shown for the history takes nothing from today's
    /// speeds: a wider range redraws none of it. Copying near the ceiling at
    /// the one speed ever sent says only that the copy speed is past it, so
    /// that is all it says, however the evidence piles up.
    #[test]
    fn the_copy_speed_shown_is_the_historys_and_never_past_what_was_sent() {
        let history: Vec<SessionResult> = (0..30)
            .map(|n| {
                let mut session = stored(n, 20.0, "KMRSU");
                session.groups = vec![session.groups[0].clone(); 20];
                session.group_timings = vec![session.group_timings[0].clone(); 20];
                session
            })
            .collect();
        let points = SpeedBelief::history(&history, &settings(15.0, 30.0), 0.94);
        let wide = SpeedBelief::history(&history, &settings(15.0, 40.0), 0.94);
        assert_eq!(points, wide);
        assert_eq!(points.len(), 30);
        for point in &points {
            assert!((point.sent.0 - 20.0).abs() < 1e-9 && (point.sent.1 - 20.0).abs() < 1e-9);
            assert_eq!(point.reading(), EstimateReading::Over(point.sent.1));
        }
    }

    #[test]
    fn the_history_is_learned_oldest_first_and_older_sessions_count_for_less() {
        let settings = settings(15.0, 40.0);
        // Clean copy at 30 recently, misses at 30 long ago: it should lean
        // towards copying at 30.
        let recent_clean = [stored(2, 30.0, "KMRSU"), stored(1, 30.0, "KXXXU")];
        let recent_miss = [stored(1, 30.0, "KMRSU"), stored(2, 30.0, "KXXXU")];
        let a = SpeedBelief::from_history(&recent_clean, &settings).estimate(REFERENCE_RATE);
        let b = SpeedBelief::from_history(&recent_miss, &settings).estimate(REFERENCE_RATE);
        assert!(a.mean > b.mean, "{a:?} vs {b:?}");
        assert_eq!(
            SpeedBelief::history(&recent_clean, &settings, REFERENCE_RATE).len(),
            2
        );
    }

    #[test]
    fn only_sessions_of_the_set_being_trained_count() {
        let settings = settings(15.0, 40.0);
        let mut calls = stored(1, 35.0, "KMRSU");
        calls.char_set_mode = CharSetMode::Callsign;
        let belief = SpeedBelief::from_history(&[calls], &settings);
        assert_eq!(belief, SpeedBelief::from_history(&[], &settings));
    }

    /// A contest run of QSOs that each copied `(call, nr, name)` as typed,
    /// at 30 WPM, with a dupe that owns no groups between them; with `log`
    /// off, as an older version keeps it.
    fn contest_run(copies: &[(&str, &str, &str)], log: bool) -> SessionResult {
        let sent = ["K5XYZ", "214", "BOB"];
        let groups: Vec<GroupResult> = copies
            .iter()
            .flat_map(|(call, nr, name)| [*call, *nr, *name])
            .zip(sent.iter().cycle())
            .map(|(received, sent)| GroupResult {
                sent: (*sent).into(),
                received: received.into(),
                correct: received == *sent,
            })
            .collect();
        let mut qsos: Vec<serde_json::Value> = copies
            .iter()
            .map(|_| serde_json::json!({ "t": 1, "h": 0, "g": 3, "nr": 1 }))
            .collect();
        qsos.insert(
            1,
            serde_json::json!({ "t": 1, "h": 0, "g": 0, "nr": 2, "sc": "K5XYZ" }),
        );
        let layout = serde_json::json!({
            "v": 1, "kind": "wpx", "style": "single", "callers": [1, 1], "side": "",
            "myCountry": "DL", "myContinent": "", "myZone": 0, "fields": ["nr", "name"],
            "limitS": 600, "runMs": 600_000, "pausedMs": 0, "myWpm": 30.0, "qsos": qsos,
        });
        SessionResult {
            timestamp: 1,
            char_set_mode: CharSetMode::Contest,
            alphabet_fingerprint: "contest:wpx".into(),
            contest: log.then_some(ContestLog(layout)),
            group_timings: groups
                .iter()
                .map(|_| SessionTiming {
                    time_to_complete_ms: 900.0,
                    char_wpm: Some(30.0),
                    effective_wpm: Some(30.0),
                    sends: 1,
                    ..SessionTiming::default()
                })
                .collect(),
            groups,
            ..SessionResult::empty_for_test()
        }
    }

    /// A contest's copy speed is read from its calls and numbers alone: a
    /// field from a short list is half guessed. Its log says which group is
    /// which; without one, the groups that read as calls are the calls.
    #[test]
    fn a_contest_run_teaches_the_speed_by_its_calls_and_numbers() {
        let run = contest_run(&[("K5XYZ", "214", "BOB"), ("K5XYZ", "214", "BOB")], true);
        assert!(run.contest_run().is_some(), "the log reads");
        assert_eq!(speed_groups(&run), [true, true, false, true, true, false]);
        let dropped = contest_run(&[("K5XYZ", "214", "BOB")], false);
        assert_eq!(speed_groups(&dropped), [true, false, false]);
        assert_eq!(speed_groups(&stored(1, 30.0, "KMRSU")), [true]);

        let mut contest = settings(15.0, 40.0);
        contest.curriculum.char_set_mode = CharSetMode::Contest;
        let belief = |copies: &[(&str, &str, &str)]| {
            SpeedBelief::from_history(&[contest_run(copies, true)], &contest)
                .estimate(REFERENCE_RATE)
        };
        let names_right = belief(&[("K5XYZ", "214", "BOB"); 3]);
        let names_wrong = belief(&[("K5XYZ", "214", "XX"); 3]);
        let numbers_wrong = belief(&[("K5XYZ", "819", "BOB"); 3]);
        assert_eq!(names_right, names_wrong, "the names say nothing of speed");
        assert!(numbers_wrong.mean < names_right.mean, "the numbers do");
    }

    /// A higher accuracy is copied at a lower speed.
    #[test]
    fn a_stricter_aim_is_a_slower_speed() {
        let mut belief = SpeedBelief::new(25.0);
        for _ in 0..20 {
            belief.learn("KMRSU", "KMRSU", 25.0);
        }
        let comfortable = belief.estimate(0.97).mean;
        let stretch = belief.estimate(0.90).mean;
        assert!(comfortable < stretch, "{comfortable} vs {stretch}");
    }
    /// Round speeds, as close together as keeps them to a dozen, from about
    /// the slowest to about the fastest.
    #[test]
    fn a_forecast_lists_round_speeds_across_the_range() {
        assert_eq!(
            forecast_speeds(18.0, 25.0),
            [18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0, 25.0]
        );
        assert_eq!(
            forecast_speeds(18.0, 30.0),
            [18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 30.0]
        );
        assert_eq!(
            forecast_speeds(10.0, 50.0),
            [10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0]
        );
        assert_eq!(forecast_speeds(5.0, 80.0).len(), 8);
        // Farnsworth spacing makes a speed that is not a whole number.
        assert_eq!(forecast_speeds(19.36, 25.0).first(), Some(&19.0));
        assert_eq!(forecast_speeds(20.0, 20.0), [20.0]);
        assert!(forecast_speeds(1.0, 5_000.0).len() <= FORECAST_ROWS_MAX + 1);
        assert!(forecast_speeds(f64::NAN, 20.0).is_empty());
        assert_eq!(nearest(&[10.0, 15.0], 12.5), 0, "a tie goes to the slower");
    }

    /// Without adaptive speed the station draws the speed, evenly across the
    /// range: the grid spreads the way its draws do.
    #[test]
    fn stations_spread_over_the_forecast_the_way_they_are_drawn() {
        let mut farnsworth = settings(15.0, 40.0);
        farnsworth.playback.link_char_to_effective = false;
        farnsworth.playback.effective_wpm_min = 10.0;
        farnsworth.playback.effective_wpm_max = 30.0;
        for settings in [settings(10.0, 50.0), farnsworth] {
            let (low, high) = SpeedPath::of(&settings).bounds();
            let speeds = forecast_speeds(low, high);
            let shares = station_shares(&settings, &speeds);
            assert!((shares.iter().sum::<f64>() - 1.0).abs() < 1e-9);
            let mut drawn = vec![0.0; speeds.len()];
            let mut rng = SplitMix64::new(5);
            let draws = 40_000;
            for _ in 0..draws {
                let voice = crate::timing::resolve_station(&settings, &mut rng);
                drawn[nearest(&speeds, copy_speed(voice.char_wpm, voice.effective_wpm))] +=
                    1.0 / f64::from(draws);
            }
            for ((speed, share), seen) in speeds.iter().zip(&shares).zip(&drawn) {
                assert!(
                    (share - seen).abs() < 0.01,
                    "{speed}: {share} worked out, {seen} drawn"
                );
            }
        }
        // Tied to the characters, every speed between the ends is as likely.
        let even = station_shares(&settings(10.0, 50.0), &forecast_speeds(10.0, 50.0));
        assert!(
            even[1..8].iter().all(|share| (share - 0.125).abs() < 1e-9),
            "{even:?}"
        );
    }

    /// With adaptive speed, the forecast is what the draw itself does.
    #[test]
    fn the_forecast_of_the_draw_is_what_the_draw_does() {
        let mut belief = SpeedBelief::new(24.0);
        let mut rng = SplitMix64::new(11);
        for _ in 0..30 {
            let (sent, received) = copy_group(24.0, 0.08, 22.0, &mut rng);
            belief.learn(&sent, &received, 22.0);
        }
        let speeds = forecast_speeds(10.0, 50.0);
        let forecast = belief.drawn_near(&speeds, 10.0, 50.0, 0.94);
        assert!((forecast.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        let mut drawn = vec![0.0; speeds.len()];
        let draws = 6_000;
        for _ in 0..draws {
            let speed = belief.draw_speed(10.0, 50.0, 0.94, &mut rng);
            drawn[nearest(&speeds, speed)] += 1.0 / f64::from(draws);
        }
        for ((speed, share), seen) in speeds.iter().zip(&forecast).zip(&drawn) {
            assert!(
                (share - seen).abs() < 0.03,
                "{speed}: {share} forecast, {seen} drawn"
            );
        }
        let busiest = forecast
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| speeds[index]);
        assert_eq!(busiest, Some(20.0), "{forecast:?}");
        // Faster is copied less well.
        assert!(belief.right_at(15.0).mean > belief.right_at(25.0).mean);
        assert!(belief.right_at(25.0).mean > belief.right_at(40.0).mean);
        // One speed is the only one drawn.
        assert_eq!(belief.drawn_near(&[20.0], 20.0, 20.0, 0.94), [1.0]);
        assert!(belief.drawn_near(&[], 10.0, 50.0, 0.94).is_empty());
    }
}
