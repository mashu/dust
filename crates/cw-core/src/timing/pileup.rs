//! Whoever else is calling, placed where the receiver lets you hear them.
//!
//! The only part of Morse timing that looks at the receiver: where a caller
//! can sit is decided by the filter you are listening through.

use super::station::{StationVoice, resolve_station};
use crate::band::FADE_OFFSET_MAX_SEC;
use crate::rng::Rng;
use crate::settings::TrainingSettings;

/// Another station calling over the top of the one you want.
#[derive(Clone, Debug, PartialEq)]
pub struct Interferer {
    pub text: String,
    pub voice: StationVoice,
    /// How late this station starts after the one you copy, in seconds;
    /// before it, when less than nothing. Nobody in a pile-up is
    /// synchronised, and a stagger is most of what makes one hard to pick
    /// apart: the one you want is no likelier than any other to be first.
    pub delay_sec: f64,
    /// Where its path is in the band's fading, in seconds of the fade past
    /// the station you copy's: each caller arrives over a path of its own,
    /// so they rise and fall apart, and which of them stands out changes as
    /// they do.
    pub fade_from_sec: f64,
}

/// Everything the receiver hears during one send: the station you are copying,
/// and whoever else is calling across it.
#[derive(Clone, Debug, PartialEq)]
pub struct Transmission {
    pub text: String,
    pub voice: StationVoice,
    pub others: Vec<Interferer>,
}

impl Transmission {
    /// One station, alone on the frequency.
    pub fn alone(text: impl Into<String>, voice: StationVoice) -> Self {
        Self {
            text: text.into(),
            voice,
            others: Vec::new(),
        }
    }

    /// `text` from `voice`, with whoever else the settings have calling
    /// across it, each sending what `say` gives them. The callers are drawn
    /// from `rng`, which `say` is handed as well, so one generator decides
    /// the whole pile-up.
    pub fn calling<R: Rng>(
        settings: &TrainingSettings,
        text: impl Into<String>,
        voice: StationVoice,
        rng: &mut R,
        mut say: impl FnMut(&mut R) -> String,
    ) -> Self {
        let text = text.into();
        let others = settings
            .band
            .stations_max
            .clamp(crate::settings::STATIONS_MIN, crate::settings::STATIONS_MAX)
            .saturating_sub(1) as usize;
        if others == 0 {
            return Self::alone(text, voice);
        }
        let texts: Vec<String> = (0..others).map(|_| say(rng)).collect();
        Self {
            others: resolve_pileup(settings, &voice, &text, &texts, rng),
            text,
            voice,
        }
    }
}

/// The closest another station may land to the one you want.
///
/// This is a fact about ears, not about radios. Two tones within about a
/// hundred hertz of each other at CW pitch fall inside one critical band, and
/// the quieter one is then masked rather than heard — it arrives as roughness
/// on the station you want, not as a second station. Twenty-five hertz apart,
/// which is where this started, you hear one station and a warble.
pub const PILEUP_MIN_SEPARATION_HZ: f64 = 130.0;
/// The closest two of the others may land to each other.
///
/// Less than the distance from the station you want, because they need only
/// be told apart as two callers rather than copied: at sixty hertz two tones
/// are a pair, where at a few hertz they fuse into one beating, and a
/// pile-up of five collapses into what sounds like two.
pub const PILEUP_CALLER_SEPARATION_HZ: f64 = 60.0;
/// How far down the receiver's skirt the others may sit, in decibels.
///
/// A real pile-up does not stop at the filter's 6 dB points: the callers just
/// outside them are still there, quieter, and they are what makes reaching
/// for the filter worth it. Fifteen decibels down a caller is plainly
/// audible over a 300 Hz filter's hiss; any further and it is gone.
pub const PILEUP_SKIRT_DB: f64 = -15.0;
/// The longest anyone in a pile-up waits before joining in, the station you
/// copy among them: when one stands by, the callers come back within a
/// moment of each other, in no order.
pub(crate) const PILEUP_MAX_DELAY_SEC: f64 = 0.6;

/// Tune in whoever else is calling.
///
/// They sit either side of the wanted station, off-frequency and a good margin
/// below it, so the one you want stays the strongest thing in the passband —
/// the whole exercise is copying *that* one. Being off-frequency is what makes
/// the receiver's filter worth reaching for: narrow it and they thin out,
/// because they are further from its centre than the station you are on.
///
/// Takes more texts than it may need and decides how many stations are calling
/// this time, so the count varies from group to group like a real pile-up.
/// Anything matching `wanted_text` is dropped: nobody else has your callsign.
pub(crate) fn resolve_pileup(
    settings: &TrainingSettings,
    wanted: &StationVoice,
    wanted_text: &str,
    texts: &[String],
    rng: &mut impl Rng,
) -> Vec<Interferer> {
    let most = settings
        .band
        .stations_max
        .clamp(crate::settings::STATIONS_MIN, crate::settings::STATIONS_MAX);
    if most <= 1 || texts.is_empty() {
        return Vec::new();
    }
    // How many are calling this time, somewhere in the range asked for.
    let fewest = settings
        .band
        .stations_min
        .clamp(crate::settings::STATIONS_MIN, most) as usize;
    let calling = rng.usize_in(fewest, most as usize);
    let spread = settings.band.pileup_spread_hz.clamp(
        crate::settings::PILEUP_SPREAD_MIN,
        crate::settings::PILEUP_SPREAD_MAX,
    );
    let down_db = settings.band.pileup_level_db.clamp(
        crate::settings::PILEUP_LEVEL_MIN_DB,
        crate::settings::PILEUP_LEVEL_MAX_DB,
    );

    // Where the others may sit is decided by the receiver, because the
    // receiver is what you hear. Three rules fall out of that, and between
    // them they are the whole placement.
    //
    // They have to land where the receiver lets them through — inside the
    // passband or out on its skirt, down to PILEUP_SKIRT_DB — or they are not
    // in the pile-up at all. That is why squeezing the filter thins it, and
    // why reaching for the filter is worth teaching. The reach is read off the
    // filter's real response, so the shape counts as well as the width.
    //
    // None of them may sit closer to the middle of the passband than the
    // station you want, or the filter would favour the wrong one and hand you
    // an interferer louder than your own station however far down we set it.
    // Tuning the one you want toward the middle is the other half of the same
    // tactic.
    //
    // And they keep their distance from each other too, or a narrow filter,
    // which leaves them little room, bunches them into clumps a few hertz
    // apart that the ear hears as one station.
    //
    // The first two are the filter's own geometry. Like a real IF filter it is
    // symmetric in hertz, passing a tone at f exactly as well as one at
    // 2·centre − f, so "no nearer the middle" means outside the pair the
    // wanted tone and its mirror make; and its reach stops short of zero
    // beat, where a wide filter would only be offering the other sideband.
    let receiver = crate::band::FilterDesign::from_settings(settings);
    let centre = receiver.pitch_hz();
    let mirror = crate::band::mirror_hz(centre, wanted.tone_hz);
    let (outward_low, outward_high) = (wanted.tone_hz.min(mirror), wanted.tone_hz.max(mirror));
    let (reach_low, reach_high) = receiver.edges_at(PILEUP_SKIRT_DB);
    let reach = spread.max(PILEUP_MIN_SEPARATION_HZ);
    // Either side: far enough off to be told apart, no further out than the
    // spread allows, inside the receiver's reach, and no nearer the middle
    // than the station you want.
    let below = (
        (wanted.tone_hz - reach).max(reach_low),
        (wanted.tone_hz - PILEUP_MIN_SEPARATION_HZ).min(outward_low),
    );
    let above = (
        (wanted.tone_hz + PILEUP_MIN_SEPARATION_HZ).max(outward_high),
        (wanted.tone_hz + reach).min(reach_high),
    );

    // When the one you copy comes back, in the same stagger as the rest.
    let wanted_start = rng.pick_in_range(0.0, PILEUP_MAX_DELAY_SEC);
    let mut others: Vec<Interferer> = Vec::new();
    for text in texts.iter().take(calling.saturating_sub(1)) {
        // Two stations with the same call is not a pile-up, it is a mistake —
        // and on a short alphabet the generator will hand us one sooner or
        // later. Dropping it costs a station rather than the sense of it.
        if text.is_empty() || text == wanted_text {
            continue;
        }
        // What is left of each side once the callers already placed have
        // their room. Whichever side still has some wins, either one at even
        // odds when both do; if neither has, this caller is one the receiver
        // does not give you.
        let placed: Vec<f64> = others.iter().map(|o| o.voice.tone_hz).collect();
        let free_below = free_room(below, &placed);
        let free_above = free_room(above, &placed);
        let side = match (free_below.is_empty(), free_above.is_empty()) {
            (false, false) if rng.f64() < 0.5 => free_below,
            (false, false) => free_above,
            (false, true) => free_below,
            (true, false) => free_above,
            (true, true) => continue,
        };
        // Each one is its own operator, and its own distance away — a margin
        // that holds as heard, through the receiver. The placement rules keep
        // a caller no louder through the filter than the station you want on
        // a symmetric filter; a peaked one, opened wide enough to reach zero
        // beat, is not quite symmetric there, so the margin is taken against
        // what the filter really does to the two of them.
        let extra_db = rng.pick_in_range(0.0, 5.0);
        let gain = 10f64.powf(-(down_db + extra_db) / 20.0);
        let mut voice = resolve_station(settings, rng);
        voice.tone_hz = pick_in_room(&side, rng).max(20.0);
        let through = receiver.response_at(wanted.tone_hz)
            / receiver.response_at(voice.tone_hz).max(f64::MIN_POSITIVE);
        voice.volume = (wanted.volume * gain * through.min(1.0)).clamp(0.0, 1.0);
        others.push(Interferer {
            text: text.clone(),
            voice,
            delay_sec: rng.pick_in_range(0.0, PILEUP_MAX_DELAY_SEC) - wanted_start,
            fade_from_sec: rng.pick_in_range(0.0, FADE_OFFSET_MAX_SEC),
        });
    }
    others
}

/// What is left of one side of the pile-up, `low..=high`, once every caller
/// already `placed` has [`PILEUP_CALLER_SEPARATION_HZ`] of room either side of
/// it: the pieces a new caller can still land in. Empty when the side had no
/// room to begin with or has none left.
pub(crate) fn free_room((low, high): (f64, f64), placed: &[f64]) -> Vec<(f64, f64)> {
    let mut pieces = Vec::new();
    if high >= low {
        pieces.push((low, high));
    }
    for &taken in placed {
        let (near, far) = (
            taken - PILEUP_CALLER_SEPARATION_HZ,
            taken + PILEUP_CALLER_SEPARATION_HZ,
        );
        pieces = pieces
            .into_iter()
            .flat_map(|(a, b): (f64, f64)| {
                if b <= near || a >= far {
                    return [Some((a, b)), None];
                }
                [
                    (near >= a).then_some((a, near)),
                    (b >= far).then_some((far, b)),
                ]
            })
            .flatten()
            .collect();
    }
    pieces
}

/// A tone somewhere in `pieces`, evenly over their combined length, so a
/// side broken up by the callers already on it is no likelier to be used at
/// one end than the other. Where all that is left are single points, one of
/// them.
pub(crate) fn pick_in_room(pieces: &[(f64, f64)], rng: &mut impl Rng) -> f64 {
    let total: f64 = pieces.iter().map(|(a, b)| b - a).sum();
    if total <= 0.0 {
        let last = pieces.len().saturating_sub(1);
        return pieces.get(rng.usize_in(0, last)).map_or(0.0, |(a, _)| *a);
    }
    let mut left = rng.pick_in_range(0.0, total);
    for &(a, b) in pieces {
        if left <= b - a {
            return a + left;
        }
        left -= b - a;
    }
    pieces.last().map_or(0.0, |(_, b)| *b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;
    use crate::settings::{
        FILTER_WIDTH_MAX, FILTER_WIDTH_MIN, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX,
        PILEUP_SPREAD_MIN, STATIONS_MAX, STATIONS_MIN, TrainingSettings,
    };
    use crate::timing::{PlaybackPlan, plan_morse_playback_for, plan_transmission};

    fn settings(max: u32) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.band.stations_max = max;
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        s.band.link_volume = true;
        s.band.volume_min = 1.0;
        s.clamp()
    }

    fn texts(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("TEST{i}")).collect()
    }

    fn pileup(max: u32, seed: u64) -> (StationVoice, Vec<Interferer>) {
        let s = settings(max);
        let mut rng = SplitMix64::new(seed.wrapping_mul(7919).wrapping_add(3));
        let wanted = resolve_station(&s, &mut rng);
        let others = resolve_pileup(&s, &wanted, "W1AW", &texts(max as usize), &mut rng);
        (wanted, others)
    }

    /// One station calling is sent alone; more bring a pile-up of their own
    /// texts, every one of them drawn from the generator given.
    #[test]
    fn a_call_brings_the_pile_up_the_settings_ask_for() {
        let mut rng = SplitMix64::new(11);
        let voice = resolve_station(&settings(1), &mut rng);
        let say = |_: &mut SplitMix64| "DL1ABC".to_string();
        let alone = Transmission::calling(&settings(1), "CQ", voice, &mut rng, say);
        assert_eq!(alone, Transmission::alone("CQ", voice));

        let mut s = settings(4);
        s.band.stations_min = 4;
        s.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        let call = Transmission::calling(&s, "CQ", voice, &mut rng, say);
        assert_eq!((call.text.as_str(), call.voice), ("CQ", voice));
        assert!(
            (1..=3).contains(&call.others.len()),
            "{}",
            call.others.len()
        );
        assert!(call.others.iter().all(|other| other.text == "DL1ABC"));
    }

    /// The default is one station, alone. A pile-up is something you ask for.
    #[test]
    fn nobody_else_is_calling_until_you_say_so() {
        assert_eq!(TrainingSettings::default().band.stations_max, STATIONS_MIN);
        for seed in 0..50 {
            assert!(pileup(STATIONS_MIN, seed).1.is_empty());
        }
    }

    /// The exercise is copying the strongest station, so the one you want has
    /// to be the strongest — every time, not usually.
    #[test]
    fn the_station_you_want_is_always_the_strongest() {
        let floor = settings(STATIONS_MAX).band.pileup_level_db;
        for seed in 0..400 {
            let (wanted, others) = pileup(STATIONS_MAX, seed);
            for other in &others {
                assert!(
                    other.voice.volume < wanted.volume,
                    "seed {seed}: another station matched the one you want"
                );
                let under = -20.0 * (other.voice.volume / wanted.volume).log10();
                assert!(
                    under >= floor - 0.001,
                    "seed {seed}: only {under:.1} dB under, asked for {floor}"
                );
            }
        }
    }

    /// They sit either side, far enough off to be pickable apart by ear and by
    /// filter, and not so far they stop being a pile-up.
    #[test]
    fn the_others_sit_around_you_rather_than_on_top_of_you() {
        let spread = settings(STATIONS_MAX).band.pileup_spread_hz;
        let mut below = 0;
        let mut above = 0;
        for seed in 0..400 {
            let (wanted, others) = pileup(STATIONS_MAX, seed);
            for other in &others {
                let offset = other.voice.tone_hz - wanted.tone_hz;
                assert!(
                    offset.abs() >= PILEUP_MIN_SEPARATION_HZ - 0.001,
                    "seed {seed}: a station landed {offset:.0} Hz away"
                );
                assert!(
                    offset.abs() <= spread + 0.001,
                    "seed {seed}: a station strayed {offset:.0} Hz out"
                );
                assert!(other.voice.tone_hz > 0.0);
                if offset < 0.0 {
                    below += 1;
                } else {
                    above += 1;
                }
            }
        }
        assert!(below > 0 && above > 0, "they all went the same way");
    }

    /// A pile-up that arrived the same way every time would be one puzzle, not
    /// a drill.
    ///
    /// Given room to put them — a wide filter, so nobody is squeezed out —
    /// every count from one to the most turns up. Through the default 300 Hz
    /// receiver, with the station you want in the middle of it, there is only
    /// room for one caller either side of it — 130 Hz off and no more than
    /// 15 dB down the skirt leaves each side 30 Hz, and callers keep 60 Hz
    /// apart — so there the count tops out at three, and still varies.
    #[test]
    fn how_many_are_calling_changes_from_group_to_group() {
        let counts = |width: f64| {
            let mut s = settings(STATIONS_MAX);
            s.band.filter_width_hz = width;
            let s = s.clamp();
            let mut seen = std::collections::BTreeSet::new();
            for seed in 0..400u64 {
                let mut rng = SplitMix64::new(seed.wrapping_mul(7919).wrapping_add(3));
                let wanted = resolve_station(&s, &mut rng);
                let others =
                    resolve_pileup(&s, &wanted, "W1AW", &texts(STATIONS_MAX as usize), &mut rng);
                seen.insert(others.len() + 1);
            }
            seen
        };
        assert_eq!(
            counts(FILTER_WIDTH_MAX),
            (1..=STATIONS_MAX as usize).collect(),
            "every count from one to the maximum should turn up"
        );
        assert_eq!(
            counts(crate::settings::DEFAULT_FILTER_WIDTH_HZ),
            (1..=3).collect(),
            "through the default receiver, one to three"
        );

        // And the stagger is real: they do not all start together, and the
        // one you copy is as often after the others as before them.
        let delays: Vec<f64> = (0..200)
            .flat_map(|seed| pileup(STATIONS_MAX, seed).1)
            .map(|other| other.delay_sec)
            .collect();
        assert!(delays.iter().any(|d| *d > 0.05), "nobody was late");
        assert!(delays.iter().any(|d| *d < -0.05), "nobody was early");
        assert!(
            delays
                .iter()
                .all(|d| (-PILEUP_MAX_DELAY_SEC..=PILEUP_MAX_DELAY_SEC).contains(d)),
            "a station waited longer than the stagger"
        );
    }

    /// Whoever calls first starts the send, and the one you copy is no
    /// likelier than another to be first: in a pile-up of four through the
    /// widest receiver, a caller is on before it most of the time. The send
    /// starts at its first tone, and the one you copy keeps its characters
    /// with its tones, however far it is put back.
    #[test]
    fn the_one_you_copy_is_not_always_first() {
        let mut s = settings(STATIONS_MAX);
        s.band.filter_width_hz = FILTER_WIDTH_MAX;
        let s = s.clamp();
        let (mut piled, mut beaten) = (0, 0);
        for seed in 0..400u64 {
            let mut rng = SplitMix64::new(seed * 131 + 17);
            let wanted = resolve_station(&s, &mut rng);
            let others = resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng);
            let sent = Transmission {
                text: "W1AW".into(),
                voice: wanted,
                others,
            };
            let planned = plan_transmission(&sent, &s);
            let alone = plan_morse_playback_for("W1AW", &s, &wanted);
            let first = |plan: &PlaybackPlan| plan.events.first().map(|e| e.start_sec);
            let yours = first(&planned.wanted).expect("your tones");
            let theirs = planned
                .others
                .iter()
                .filter_map(first)
                .fold(f64::MAX, f64::min);
            assert!(
                yours.min(theirs).abs() < 1e-9,
                "seed {seed}: the send started on nothing"
            );
            for (span, was) in planned.wanted.chars.iter().zip(&alone.chars) {
                assert!((span.start_sec - was.start_sec - yours).abs() < 1e-9);
                assert!((span.end_sec - was.end_sec - yours).abs() < 1e-9);
            }
            assert!((planned.wanted.duration_sec - alone.duration_sec - yours).abs() < 1e-9);
            if planned.others.iter().any(|plan| !plan.events.is_empty()) {
                piled += 1;
                if theirs < yours {
                    beaten += 1;
                }
            }
        }
        assert!(piled > 100, "only {piled} pile-ups");
        assert!(
            beaten * 10 >= piled * 4,
            "a caller was first in {beaten} of {piled}"
        );
    }

    /// The same group has to bring the same pile-up, or a repeat would be a
    /// different puzzle and a retry after a stalled send would change the
    /// answer underneath you.
    #[test]
    fn the_same_draw_brings_the_same_stations() {
        let s = settings(STATIONS_MAX);
        let once = {
            let mut rng = SplitMix64::new(99);
            let wanted = resolve_station(&s, &mut rng);
            resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng)
        };
        let twice = {
            let mut rng = SplitMix64::new(99);
            let wanted = resolve_station(&s, &mut rng);
            resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng)
        };
        assert_eq!(once, twice);
    }

    /// Everyone is laid against the wanted station's clock: pushed back by
    /// their own delay, and cut at its end — whole elements only, because a
    /// tone stopped halfway through is a click.
    #[test]
    fn the_others_are_cut_at_the_end_and_never_mid_tone() {
        let s = settings(STATIONS_MAX);
        for seed in 0..200 {
            let mut rng = SplitMix64::new(seed * 31 + 7);
            let wanted = resolve_station(&s, &mut rng);
            let others = resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng);
            if others.is_empty() {
                continue;
            }
            let sent = Transmission {
                text: "W1AW".into(),
                voice: wanted,
                others: others.clone(),
            };
            let planned = plan_transmission(&sent, &s);
            let ends_at = planned.wanted.duration_sec;
            assert!(ends_at > 0.0);
            // The one you copy starts as far in as anyone called before it.
            let lead = others.iter().map(|o| -o.delay_sec).fold(0.0, f64::max);
            for (plan, other) in planned.others.iter().zip(&others) {
                for event in &plan.events {
                    assert!(
                        event.start_sec >= other.delay_sec + lead - 1e-9,
                        "seed {seed}: a station started before it was due"
                    );
                    assert!(
                        event.start_sec + event.keyed_sec <= ends_at + 1e-9,
                        "seed {seed}: a station ran past the send it was over"
                    );
                }
                assert!(plan.duration_sec <= ends_at + 1e-9);
            }
        }
    }

    /// Whatever the settings say, what comes out has to be playable.
    #[test]
    fn a_pile_up_is_always_sendable() {
        for max in STATIONS_MIN..=STATIONS_MAX {
            for spread in [PILEUP_SPREAD_MIN, 140.0, PILEUP_SPREAD_MAX] {
                let mut s = settings(max);
                s.band.pileup_spread_hz = spread;
                let s = s.clamp();
                for seed in 0..40 {
                    let mut rng = SplitMix64::new(seed * 13 + 1);
                    let wanted = resolve_station(&s, &mut rng);
                    let others =
                        resolve_pileup(&s, &wanted, "K5ABC", &texts(max as usize), &mut rng);
                    let sent = Transmission {
                        text: "K5ABC".into(),
                        voice: wanted,
                        others,
                    };
                    let planned = plan_transmission(&sent, &s);
                    for plan in std::iter::once(&planned.wanted).chain(&planned.others) {
                        for event in &plan.events {
                            assert!(event.frequency_hz.is_finite() && event.frequency_hz > 0.0);
                            assert!(event.target_gain.is_finite() && event.target_gain >= 0.0);
                            assert!(event.duration_sec.is_finite() && event.duration_sec > 0.0);
                        }
                    }
                }
            }
        }
    }

    /// Reaching for a narrower filter is the tactic this whole feature exists
    /// to teach, so it has to work: squeeze the passband and the callers
    /// beside you drop out of it one by one, until the only station left is
    /// the one you were tuned to.
    ///
    /// Nobody is placed where the filter would bury them: never further down
    /// its skirt than [`PILEUP_SKIRT_DB`], read off the receiver as it is
    /// really built — at the width the side-tone range widens it to, and in
    /// its own shape.
    #[test]
    fn a_narrower_filter_thins_the_pile_up() {
        // A side-tone spread, not a pinned tone: how much room there is
        // beside you depends on where in the passband you happen to sit.
        // 400-600 Hz, which a 233 Hz receiver passes: the default 300-600 Hz
        // holds the receiver at least 353 Hz wide, and the slider narrows
        // nothing under that.
        let base = {
            let mut s = TrainingSettings::default();
            s.band.side_tone_min = 400.0;
            s.band.side_tone_max = 600.0;
            s.band.stations_max = STATIONS_MAX;
            s.clamp()
        };

        let heard = |mut s: TrainingSettings, bandwidth: f64| {
            s.band.filter_width_hz = bandwidth;
            let s = s.clamp();
            let receiver = crate::band::FilterDesign::from_settings(&s);
            let skirt = 10f64.powf(PILEUP_SKIRT_DB / 20.0);
            let mut count = 0usize;
            for seed in 0..400u64 {
                let mut rng = SplitMix64::new(seed * 2_654_435_761 + 11);
                let wanted = resolve_station(&s, &mut rng);
                for other in resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng) {
                    let through = receiver.response_at(other.voice.tone_hz);
                    assert!(
                        through >= skirt * (1.0 - 1e-6),
                        "a station at {:.0} Hz was {:.1} dB down a {bandwidth:.0} Hz filter",
                        other.voice.tone_hz,
                        20.0 * through.log10()
                    );
                    count += 1;
                }
            }
            count
        };

        let widths = [FILTER_WIDTH_MAX, 1_000.0, 500.0, 300.0, FILTER_WIDTH_MIN];
        let counts = widths.map(|width| heard(base.clone(), width));
        let (wide, narrow) = (counts[0], counts[widths.len() - 1]);
        println!("callers heard at {widths:?} Hz: {counts:?}");

        assert!(wide > 0, "nobody else was ever heard, even wide open");
        // Wide open it is the spread that holds them in, not the filter, so a
        // filter only starts costing callers once it is narrower than that,
        // and from there every step narrower costs more of them.
        assert!(
            counts.windows(2).all(|w| w[1] <= w[0]) && counts[3] < counts[2],
            "narrowing the filter did not thin the pile-up: {counts:?}"
        );
        // By the narrowest it goes, more than half of them are gone. Not three in four
        // of them, as when the receiver could be squeezed to the slider's
        // 150 Hz and nobody sat outside its 3 dB points: it still has to pass
        // the 400-600 Hz side-tone range, which takes 233 Hz, and the callers
        // it leaves sit out on its skirt. Measured, 345 of 799 are left
        // through the sharp default, and 300 through the peaked one.
        assert!(
            narrow * 2 < wide,
            "a {FILTER_WIDTH_MIN:.0} Hz filter still passed {narrow} of {wide} callers"
        );
        // And with the station you want held in the middle, the filter does
        // go down to 150 Hz, and then it is the only one left.
        let mut pinned = base;
        pinned.band.side_tone_min = 600.0;
        pinned.band.side_tone_max = 600.0;
        assert_eq!(heard(pinned, FILTER_WIDTH_MIN), 0);
    }

    /// Five calling through a 300 Hz filter — the default receiver, and the
    /// other two shapes at its width — is still a pile-up you hear as more
    /// than one station. The callers the filter has room for keep their
    /// distance instead of bunching a few hertz apart, where the ear fuses
    /// them into one: in the typical group the closest two are 40 Hz apart or
    /// more. And every one is out on the skirt no further than it can still
    /// be heard, 20 dB down at the very most.
    #[test]
    fn a_narrow_filter_keeps_the_callers_apart() {
        for shape in [
            crate::settings::FilterShape::Peaked,
            crate::settings::FilterShape::Sharp,
            crate::settings::FilterShape::Soft,
        ] {
            let mut s = TrainingSettings::default();
            s.band.stations_min = STATIONS_MAX;
            s.band.stations_max = STATIONS_MAX;
            s.band.pileup_spread_hz = 350.0;
            s.band.filter_shape = shape;
            let s = s.clamp();
            assert_eq!(
                s.band.filter_width_hz,
                crate::settings::DEFAULT_FILTER_WIDTH_HZ
            );
            let receiver = crate::band::FilterDesign::from_settings(&s);
            let (mut gaps, mut placed, mut quietest) = (Vec::new(), 0usize, 0.0f64);
            const GROUPS: u64 = 2_000;
            for seed in 0..GROUPS {
                let mut rng = SplitMix64::new(seed * 40_503 + 5);
                let wanted = resolve_station(&s, &mut rng);
                let others = resolve_pileup(&s, &wanted, "W1AW", &texts(5), &mut rng);
                placed += others.len();
                for other in &others {
                    let level = 20.0 * receiver.response_at(other.voice.tone_hz).log10();
                    quietest = quietest.min(level);
                    assert!(
                        level >= -20.0,
                        "{shape:?} seed {seed}: a caller {level:.1} dB down the filter"
                    );
                }
                let mut tones: Vec<f64> = others.iter().map(|o| o.voice.tone_hz).collect();
                tones.sort_by(f64::total_cmp);
                if let Some(closest) = tones.windows(2).map(|w| w[1] - w[0]).reduce(f64::min) {
                    gaps.push(closest);
                }
            }
            gaps.sort_by(f64::total_cmp);
            assert!(!gaps.is_empty(), "{shape:?}: never two callers at once");
            let median = gaps[gaps.len() / 2];
            println!(
                "{shape:?}/300, five calling: {:.2} callers heard a group, {} groups with two \
                 or more, closest two a median {median:.0} Hz apart (least {:.0}); quietest \
                 {quietest:.1} dB",
                placed as f64 / GROUPS as f64,
                gaps.len(),
                gaps[0]
            );
            assert!(
                median >= 40.0,
                "{shape:?}: callers a median {median:.1} Hz apart"
            );
        }
    }

    /// The level setting says how far down the others are before the receiver.
    /// What you hear is after it, and a filter does not treat every pitch
    /// alike — so this is the one that matters: whatever the tuning, the
    /// width and the shape, the station you want comes out on top.
    /// Through any receiver the controls build — every shape, wide open, over
    /// wide side-tone ranges, at the smallest margin and the widest spread —
    /// each caller is heard at least the margin below the station you want.
    /// A peaked receiver wide enough to reach zero beat is not symmetric
    /// there, which the placement alone would not allow for.
    #[test]
    fn every_caller_is_heard_the_margin_below_the_station_you_want() {
        use crate::settings::{FilterShape, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX};
        let mut worst = f64::INFINITY;
        for shape in [FilterShape::Sharp, FilterShape::Soft, FilterShape::Peaked] {
            for width in [300.0, 1_000.0, 1_500.0, 1_700.0, FILTER_WIDTH_MAX] {
                for (low, high) in [(400.0, 600.0), (300.0, 700.0), (200.0, 800.0)] {
                    let mut s = TrainingSettings::default();
                    s.band.stations_min = STATIONS_MAX;
                    s.band.stations_max = STATIONS_MAX;
                    s.band.filter_shape = shape;
                    s.band.filter_width_hz = width;
                    s.band.side_tone_min = low;
                    s.band.side_tone_max = high;
                    s.band.pileup_level_db = PILEUP_LEVEL_MIN_DB;
                    s.band.pileup_spread_hz = PILEUP_SPREAD_MAX;
                    let s = s.clamp();
                    let receiver = crate::band::FilterDesign::from_settings(&s);
                    for seed in 0..200u64 {
                        let mut rng = SplitMix64::new(
                            seed.wrapping_mul(2_862_933_555_777_941_757).wrapping_add(3),
                        );
                        let wanted = resolve_station(&s, &mut rng);
                        let yours = wanted.volume * receiver.response_at(wanted.tone_hz);
                        for other in resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng) {
                            let theirs =
                                other.voice.volume * receiver.response_at(other.voice.tone_hz);
                            worst = worst.min(20.0 * (yours / theirs).log10());
                        }
                    }
                }
            }
        }
        assert!(
            worst >= PILEUP_LEVEL_MIN_DB - 1e-9,
            "a caller came within {worst:.2} dB of the station you want"
        );
    }

    #[test]
    fn the_station_you_want_is_still_the_strongest_through_the_filter() {
        use crate::band::{FilterDesign, ReceiverFilter};

        const SAMPLE_RATE: u32 = 48_000;

        // Steady-state amplitude of a tone at `hz` once it is through the
        // receiver: let the filter settle first, then measure.
        fn through_filter(settings: &TrainingSettings, hz: f64) -> f64 {
            let mut filter =
                ReceiverFilter::new(SAMPLE_RATE, &FilterDesign::from_settings(settings));
            let step = std::f64::consts::TAU * hz / f64::from(SAMPLE_RATE);
            let settle = SAMPLE_RATE as usize / 4;
            let measure = SAMPLE_RATE as usize / 8;
            let mut sum_sq = 0.0;
            for n in 0..settle + measure {
                let out = filter.process((step * n as f64).sin());
                if n >= settle {
                    sum_sq += out * out;
                }
            }
            (sum_sq / measure as f64).sqrt()
        }

        for (shape, bandwidth) in [
            crate::settings::FilterShape::Sharp,
            crate::settings::FilterShape::Soft,
            crate::settings::FilterShape::Peaked,
        ]
        .into_iter()
        .flat_map(|shape| {
            [FILTER_WIDTH_MIN, 300.0, 500.0, FILTER_WIDTH_MAX].map(|bandwidth| (shape, bandwidth))
        }) {
            let mut s = TrainingSettings::default();
            s.band.stations_max = STATIONS_MAX;
            s.band.filter_width_hz = bandwidth;
            s.band.filter_shape = shape;
            let s = s.clamp();
            for seed in 0..150u64 {
                let mut rng = SplitMix64::new(seed * 6_364_136_223 + 17);
                let wanted = resolve_station(&s, &mut rng);
                let yours = wanted.volume * through_filter(&s, wanted.tone_hz);
                for other in resolve_pileup(&s, &wanted, "W1AW", &texts(4), &mut rng) {
                    let theirs = other.voice.volume * through_filter(&s, other.voice.tone_hz);
                    let margin = 20.0 * (yours / theirs).log10();
                    // Not merely ahead — ahead by at least what the weakest
                    // setting promises, or the two would be a coin toss.
                    assert!(
                        margin >= PILEUP_LEVEL_MIN_DB,
                        "{shape:?} {bandwidth:.0} Hz, seed {seed}: a caller at {:.0} Hz came \
                         out only {margin:.1} dB under the station you want at {:.0} Hz",
                        other.voice.tone_hz,
                        wanted.tone_hz
                    );
                }
            }
        }
    }

    /// The floor is a floor: ask for at least three and three call, every
    /// group, not "sometimes one".
    ///
    /// Given room to put them — a wide filter, so nobody is squeezed out of
    /// the passband — the count drawn is the count you get.
    #[test]
    fn the_floor_is_how_many_call() {
        for (fewest, most) in [(2u32, 5u32), (3, 3), (4, 5), (5, 5)] {
            let mut s = settings(most);
            s.band.stations_min = fewest;
            s.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
            s.band.pileup_spread_hz = crate::settings::PILEUP_SPREAD_MAX;
            let s = s.clamp();
            for seed in 0..200u64 {
                let mut rng = SplitMix64::new(seed * 7919 + 3);
                let wanted = resolve_station(&s, &mut rng);
                let others = resolve_pileup(&s, &wanted, "W1AW", &texts(most as usize), &mut rng);
                let calling = others.len() + 1;
                assert!(
                    calling >= fewest as usize && calling <= most as usize,
                    "seed {seed}: {calling} calling, asked for {fewest}..{most}"
                );
            }
        }
    }

    /// Both ends together means the same number every time, which is the whole
    /// reason to want a floor: a steady three-station pile-up to work on,
    /// rather than one that keeps handing you an easy group.
    #[test]
    fn holding_both_ends_together_fixes_the_count() {
        let mut s = settings(3);
        s.band.stations_min = 3;
        s.band.filter_width_hz = crate::settings::FILTER_WIDTH_MAX;
        s.band.pileup_spread_hz = crate::settings::PILEUP_SPREAD_MAX;
        let s = s.clamp();
        for seed in 0..200u64 {
            let mut rng = SplitMix64::new(seed * 31 + 11);
            let wanted = resolve_station(&s, &mut rng);
            let others = resolve_pileup(&s, &wanted, "W1AW", &texts(3), &mut rng);
            assert_eq!(others.len() + 1, 3, "seed {seed}");
        }
    }

    /// A floor above the ceiling is not a range. Whatever is stored, what
    /// comes out is the right way round.
    #[test]
    fn the_range_is_always_the_right_way_round() {
        for fewest in 0..8u32 {
            for most in 0..8u32 {
                let mut s = TrainingSettings::default();
                s.band.stations_min = fewest;
                s.band.stations_max = most;
                let s = s.clamp();
                assert!(
                    s.band.stations_min <= s.band.stations_max,
                    "{fewest}..{most} clamped to {}..{}",
                    s.band.stations_min,
                    s.band.stations_max
                );
                assert!((STATIONS_MIN..=STATIONS_MAX).contains(&s.band.stations_min));
                assert!((STATIONS_MIN..=STATIONS_MAX).contains(&s.band.stations_max));
            }
        }
    }
}
