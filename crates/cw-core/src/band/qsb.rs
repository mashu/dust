//! Fading: how much of the signal the path is letting through.

/// The weakest a fade leaves the signal. Below this a group would simply be
/// lost rather than hard to copy.
pub const QSB_MIN_GAIN: f64 = 0.25;

/// Opens the multipath sum back out to the depth a single sine would have had.
/// Four sines rarely all line up, so their sum sits nearer the middle; this
/// scales it back by the ratio of the two RMS values, and the clamp above
/// catches the rare moment they do line up — which is a fade bottoming out,
/// and sounds like one.
pub const QSB_SPREAD: f64 = 1.85;

/// How many propagation paths the fading is made of.
pub const QSB_PATHS: usize = 4;

/// Each path's rate, as a multiple of the rate setting, and how much of the
/// fading it carries.
///
/// The ratios are deliberately not simple fractions. Four sines at 1, 2 and 3
/// times a rate come back round together and you are back to a pattern; at
/// these they do not, so the fading wanders and never repeats.
///
/// Weights sum to one, so the swing still spans -1..=1 at its widest and the
/// depth setting keeps meaning what it meant.
pub fn qsb_path(index: usize) -> (f64, f64) {
    const PATHS: [(f64, f64); QSB_PATHS] =
        [(1.000, 0.40), (0.633, 0.28), (1.471, 0.20), (2.309, 0.12)];
    PATHS[index.min(QSB_PATHS - 1)]
}

/// The QSB envelope: how much of the signal is getting through at `t_sec`.
///
/// Fading is not a tremolo. A single sine gives you a fade you can set your
/// watch by, which is the one thing real QSB never is — what actually happens
/// is several paths arriving with slightly different Doppler and beating
/// against each other, so the signal wanders down and back up on no schedule.
/// That is a sum of sinusoids at close but unrelated rates, which is the
/// classical multipath model and is also the only shape the browser's
/// oscillators can build, so both backends fade the same way.
///
/// `QSB_SPREAD` is there because a sum of four sines spends most of its time
/// nearer the middle than one sine does; without it, turning multipath on
/// would quietly halve the depth of every fade.
pub fn qsb_gain_at(t_sec: f64, enabled: bool, depth: f64, rate_hz: f64) -> f32 {
    if !enabled || depth <= 0.0 {
        return 1.0;
    }
    let depth = depth.clamp(0.0, 1.0);
    let rate = rate_hz.clamp(0.03, 1.5);
    let gain_range = depth.min(1.0 - QSB_MIN_GAIN);
    let base = 1.0 - gain_range / 2.0;
    let half = gain_range / 2.0;
    let swing: f64 = (0..QSB_PATHS)
        .map(|i| {
            let (multiple, weight) = qsb_path(i);
            weight * (std::f64::consts::TAU * rate * multiple * t_sec).sin()
        })
        .sum();
    let swing = (swing * QSB_SPREAD).clamp(-1.0, 1.0);
    (base + half * swing) as f32
}
