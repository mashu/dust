//! The shapes the Stats sparklines are drawn from: where each session sits,
//! and the SVG lines, paths and bands through them.

// 420×96 keeps the drawing box at the same aspect the card gives it, so the
// SVG can scale uniformly: stretched coordinates would turn the dots into
// ellipses and thin the stroke unevenly.
pub const CHART_W: f64 = 420.0;
pub const CHART_H: f64 = 96.0;

/// The points marked with a dot are every session's up to this many; past
/// it, only the last.
const MARK_EVERY_UP_TO: usize = 30;

/// Where each of `values` sits across the chart: spread over its whole
/// width, or, for a single value, a flat line across it — which beats an
/// empty chart, the way a one-point polyline renders.
pub fn series(values: &[f64], y_at: impl Fn(f64) -> f64) -> Vec<(f64, f64)> {
    if let [only] = values {
        let y = y_at(*only);
        return vec![(0.0, y), (CHART_W, y)];
    }
    let last = values.len().saturating_sub(1).max(1) as f64;
    values
        .iter()
        .enumerate()
        .map(|(i, value)| (i as f64 / last * CHART_W, y_at(*value)))
        .collect()
}

/// The `points` of an SVG polyline through `coords`.
pub fn polyline(coords: &[(f64, f64)]) -> String {
    coords
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// An SVG path through `coords`, in order: a move to the first, a line to
/// each after it.
pub fn path(coords: impl IntoIterator<Item = (f64, f64)>) -> String {
    coords
        .into_iter()
        .enumerate()
        .map(|(n, (x, y))| format!("{}{x:.1},{y:.1}", if n == 0 { "M" } else { " L" }))
        .collect()
}

/// The points marked with a dot: every session on a short history, the last
/// on a long one, the middle of the line for a single session.
pub fn marks(coords: &[(f64, f64)], sessions: usize) -> Vec<(f64, f64)> {
    if sessions == 1 {
        coords
            .first()
            .map(|(_, y)| (CHART_W / 2.0, *y))
            .into_iter()
            .collect()
    } else if sessions <= MARK_EVERY_UP_TO {
        coords.to_vec()
    } else {
        coords.last().copied().into_iter().collect()
    }
}

/// An estimate over sessions — the copy span, the copy speed — drawn as its
/// mean, with a band about one standard deviation either side that narrows
/// as the estimate firms up.
#[derive(Clone, Debug, PartialEq)]
pub struct TrendChart {
    pub line: String,
    pub band: String,
    pub dots: Vec<(f64, f64)>,
    /// The scale, in the estimate's own units, bottom to top.
    pub low: f64,
    pub high: f64,
}

/// The chart of `bands`, one a session, oldest first, each as (below,
/// mean, above). Nothing to draw without one.
pub fn trend_chart(bands: &[(f64, f64, f64)]) -> Option<TrendChart> {
    if bands.is_empty() {
        return None;
    }
    let low = bands
        .iter()
        .map(|(below, _, _)| *below)
        .fold(f64::INFINITY, f64::min);
    let high = bands
        .iter()
        .map(|(_, _, above)| *above)
        .fold(f64::NEG_INFINITY, f64::max);
    let low = low.floor().max(0.0);
    let high = high.ceil().max(low + 2.0);
    let y_at = |value: f64| (CHART_H - (value - low) / (high - low) * CHART_H).clamp(0.0, CHART_H);

    let means: Vec<f64> = bands.iter().map(|(_, mean, _)| *mean).collect();
    let tops: Vec<f64> = bands.iter().map(|(_, _, above)| *above).collect();
    let bottoms: Vec<f64> = bands.iter().map(|(below, _, _)| *below).collect();
    let coords = series(&means, y_at);
    let edge = series(&tops, y_at)
        .into_iter()
        .chain(series(&bottoms, y_at).into_iter().rev());

    Some(TrendChart {
        line: polyline(&coords),
        band: format!("{} Z", path(edge)),
        dots: marks(&coords, bands.len()),
        low,
        high,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimate(mean: f64, sd: f64) -> (f64, f64, f64) {
        (mean - sd, mean, mean + sd)
    }

    #[test]
    fn a_path_moves_to_the_first_point_and_draws_to_the_rest() {
        assert_eq!(
            path([(0.0, 1.0), (2.0, 3.04), (4.0, 5.0)]),
            "M0.0,1.0 L2.0,3.0 L4.0,5.0"
        );
        assert_eq!(path([]), "");
        assert_eq!(polyline(&[(0.0, 1.0), (2.0, 3.0)]), "0.0,1.0 2.0,3.0");
    }

    #[test]
    fn one_value_is_a_flat_line_marked_in_the_middle() {
        let coords = series(&[5.0], |_| 40.0);
        assert_eq!(coords, vec![(0.0, 40.0), (CHART_W, 40.0)]);
        assert_eq!(marks(&coords, 1), vec![(CHART_W / 2.0, 40.0)]);
        let long: Vec<(f64, f64)> = (0..40).map(|i| (f64::from(i), 0.0)).collect();
        assert_eq!(marks(&long, 40), vec![(39.0, 0.0)], "only the last");
        assert_eq!(marks(&long[..3], 3).len(), 3, "every one");
    }

    /// The band runs along the top edge and back along the bottom, and the
    /// scale holds both, in whole units, at least two apart.
    #[test]
    fn a_trend_is_its_mean_inside_a_band() {
        assert!(trend_chart(&[]).is_none());
        let chart = trend_chart(&[estimate(4.0, 1.0), estimate(5.0, 0.4)]).unwrap();
        assert_eq!((chart.low, chart.high), (3.0, 6.0));
        assert_eq!(chart.line, "0.0,64.0 420.0,32.0");
        assert_eq!(chart.band, "M0.0,32.0 L420.0,19.2 L420.0,44.8 L0.0,96.0 Z");
        assert_eq!(chart.dots.len(), 2);
        let narrow = trend_chart(&[estimate(20.0, 0.0)]).unwrap();
        assert_eq!((narrow.low, narrow.high), (20.0, 22.0));
    }
}
