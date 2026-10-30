//! A log's contest score and rates: how well you operated, beside the copy
//! points ([`crate::score`]) that measure how well you copied.
//!
//! The score comes twice, as MorseRunner's does. Raw is what the log claims:
//! every logged QSO that is not a dupe, NILs and busted calls included,
//! its points and multipliers as typed, and the multipliers of the QSOs
//! verified counts. Verified is what survives the check: clean QSOs only, a
//! dupe scoring nothing. Verified is never above raw.
//!
//! What a QSO is worth, and what it counts for, can turn on where you are:
//! CQ WW's points on your country and continent, ARRL DX's points and
//! multipliers on your side ([`MyStation`]). A call logged is placed where
//! it is at home ([`home_country`]); one Dust cannot place is worth a CQ WW
//! point and no country, as MorseRunner leaves a call its list cannot
//! place at its default point (`Log.pas:724`), and one on your own side of
//! ARRL DX is worth nothing, as the contest has W/VE work only DX.

use std::collections::BTreeSet;

use super::check::QsoCheck;
use super::data::places::{DX, fold_labrador, home_country, w_ve, w_ve_place};
use super::data::zones::home_continent;
use super::exchange::Field;
use super::rules::{Multiplier, Points, Scoring, Side};
use super::{MS_PER_HOUR, MS_PER_MINUTE};
use crate::callsign::wpx_prefix;

/// Where you are, as a contest's score counts it: your side, and your
/// country and continent, by the call table's keys (`DL`, `EU`), as
/// [`home_country`] and [`home_continent`] read your call; empty for a call
/// Dust cannot place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MyStation<'a> {
    pub side: Side,
    pub country: &'a str,
    pub continent: &'a str,
}

/// One logged QSO, as the score reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoredQso<'a> {
    /// When it was logged, in run time.
    pub at_ms: u64,
    /// The call as you logged it.
    pub call: &'a str,
    /// The exchange as you logged it, each field's token or value.
    pub fields: &'a [(Field, String)],
    pub check: &'a QsoCheck,
}

impl ScoredQso<'_> {
    /// The value logged for `field`, as the field reads it.
    fn logged(&self, field: Field) -> Option<String> {
        self.fields
            .iter()
            .find(|(held, _)| *held == field)
            .and_then(|(_, token)| field.canonical(token))
    }

    /// What it counts for as `multiplier` counts them, as logged: its WPX
    /// prefix; the state, province or territory logged, as the Sprint
    /// counts them, or as a DX station in ARRL DX counts a W/VE call's; the
    /// country of its call, as a W/VE station in ARRL DX counts a DX call's;
    /// or its zone and its country, as CQ WW counts them; or the one the log
    /// is, where nothing multiplies the points, as in Field Day. Nothing for
    /// what counts for none.
    fn multipliers(&self, multiplier: Multiplier) -> Vec<String> {
        let country = || home_country(self.call).map(str::to_string);
        match multiplier {
            Multiplier::WpxPrefix => wpx_prefix(self.call).into_iter().collect(),
            Multiplier::Place => self
                .logged(Field::St)
                .map(|place| fold_labrador(&place).to_string())
                .filter(|place| place != DX)
                .into_iter()
                .collect(),
            Multiplier::WVePlace => self
                .logged(Field::St)
                .filter(|place| w_ve_place(place) && w_ve(self.call))
                .into_iter()
                .collect(),
            Multiplier::Country => country().filter(|_| !w_ve(self.call)).into_iter().collect(),
            Multiplier::ZoneAndCountry => self
                .logged(Field::Zn)
                .map(|zone| format!("zone {zone}"))
                .into_iter()
                .chain(country())
                .collect(),
            // Every QSO counts the one multiplier, the log, so the score
            // is the points.
            Multiplier::Nothing => vec![String::new()],
        }
    }

    /// What it is worth as `points` counts it, for `me`.
    fn points(&self, points: Points, me: &MyStation<'_>) -> u32 {
        match points {
            Points::Each(each) => each,
            Points::OtherSide(each) if Side::of_call(self.call) != me.side => each,
            Points::OtherSide(_) => 0,
            Points::ByDistance => self.distance_points(me),
        }
    }

    /// What it is worth by how far its call puts the station from `me`, as
    /// CQ WW counts it: nothing in your own country, one on your continent,
    /// two between North American countries and three on another
    /// continent, or yours unknown. A call Dust cannot place is worth one,
    /// MorseRunner's default for a call it cannot place (`Log.pas:724`,
    /// `CqWW.pas:258-275`).
    fn distance_points(&self, me: &MyStation<'_>) -> u32 {
        let Some(country) = home_country(self.call) else {
            return 1;
        };
        if country == me.country {
            return 0;
        }
        match home_continent(self.call) {
            Some(continent) if continent == me.continent && continent == "NA" => 2,
            Some(continent) if continent == me.continent => 1,
            _ => 3,
        }
    }
}

/// One column of the score: QSO points, multipliers, and their product.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub qso_points: u32,
    pub mults: u32,
    pub score: u32,
}

/// QSOs an hour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rates {
    /// Over the last five minutes of the run, as MorseRunner shows it.
    pub last_5_min: Option<f64>,
    /// Over the whole run.
    pub whole_run: Option<f64>,
    /// Over the best five minutes.
    pub best_5_min: Option<f64>,
}

/// A log's score, raw and verified, and its rates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContestScore {
    pub raw: Tally,
    pub verified: Tally,
    pub rates: Rates,
}

/// The window a rate is counted over.
const RATE_WINDOW_MS: u64 = 5 * MS_PER_MINUTE;

/// The score of `qsos`, in the order they were logged, over a run of
/// `run_ms`, as `scoring` counts it for `me`.
pub fn contest_score(
    scoring: &Scoring,
    me: &MyStation<'_>,
    qsos: &[ScoredQso<'_>],
    run_ms: u64,
) -> ContestScore {
    // Raw drops a call already logged, as a logger marks a dupe: by the
    // call as typed. Verified keeps clean QSOs that are not dupes, each
    // station once, so it can only count calls raw counts. Raw's
    // multipliers are those of the QSOs it counts and of those verified
    // counts: a clean QSO logged after a NIL with the same call is
    // verified, and the state it sent counts there, so raw counts it too;
    // a dupe's state counts in neither.
    let mut typed = BTreeSet::new();
    let raw: Vec<&ScoredQso<'_>> = qsos.iter().filter(|qso| typed.insert(qso.call)).collect();
    let mut clean = BTreeSet::new();
    let verified: Vec<&ScoredQso<'_>> = qsos
        .iter()
        .filter(|qso| qso.check.is_clean() && !qso.check.dupe && clean.insert(qso.call))
        .collect();
    let claimed: Vec<&ScoredQso<'_>> = raw.iter().chain(&verified).copied().collect();
    let times: Vec<u64> = qsos.iter().map(|qso| qso.at_ms).collect();
    ContestScore {
        raw: tally(scoring, me, &raw, &claimed),
        verified: tally(scoring, me, &verified, &verified),
        rates: Rates::of(&times, run_ms),
    }
}

/// The points of the QSOs that count, `counted`, and the multipliers of
/// those that count them, `mults_of`, for `me`.
fn tally(
    scoring: &Scoring,
    me: &MyStation<'_>,
    counted: &[&ScoredQso<'_>],
    mults_of: &[&ScoredQso<'_>],
) -> Tally {
    let multiplier = scoring.multiplier.on(me.side);
    let mults: BTreeSet<String> = mults_of
        .iter()
        .flat_map(|qso| qso.multipliers(multiplier))
        .collect();
    let qso_points = counted
        .iter()
        .map(|qso| qso.points(scoring.points, me))
        .fold(0u32, u32::saturating_add);
    let mults = u32::try_from(mults.len()).unwrap_or(u32::MAX);
    Tally {
        qso_points,
        mults,
        score: qso_points.saturating_mul(mults),
    }
}

impl Rates {
    /// The rates of QSOs logged at `times`, over a run of `run_ms`.
    pub fn of(times: &[u64], run_ms: u64) -> Self {
        let window = run_ms.min(RATE_WINDOW_MS);
        let best = times
            .iter()
            .map(|&end| count_in(times, end, window))
            .max()
            .unwrap_or(0);
        Self {
            last_5_min: rate_at(times, run_ms),
            whole_run: (run_ms > 0).then(|| per_hour(times.len(), run_ms)),
            best_5_min: (window > 0).then(|| per_hour(best, window)),
        }
    }
}

/// QSOs an hour at `now_ms` of run time, counted over the last five
/// minutes, or over the run so far while it is shorter; none at its start.
pub fn rate_at(times: &[u64], now_ms: u64) -> Option<f64> {
    let window = now_ms.min(RATE_WINDOW_MS);
    (window > 0).then(|| per_hour(count_in(times, now_ms, window), window))
}

/// How many of `times` fall in the `window` that ends at `end`.
fn count_in(times: &[u64], end: u64, window: u64) -> usize {
    times
        .iter()
        .filter(|&&at| at <= end && end - at < window)
        .count()
}

fn per_hour(count: usize, over_ms: u64) -> f64 {
    count as f64 * MS_PER_HOUR as f64 / over_ms as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::{ContestKind, QsoError};

    fn wpx() -> Scoring {
        ContestKind::Wpx.rules().scoring.expect("WPX scores")
    }

    fn cqww() -> Scoring {
        ContestKind::Cqww.rules().scoring.expect("CQ WW scores")
    }

    fn arrl_dx() -> Scoring {
        ContestKind::ArrlDx.rules().scoring.expect("ARRL DX scores")
    }

    fn field_day() -> Scoring {
        ContestKind::FieldDay
            .rules()
            .scoring
            .expect("Field Day scores")
    }

    /// You as DL1ABC, in Germany, in Europe, DX.
    const DL: MyStation<'static> = MyStation {
        side: Side::Dx,
        country: "DL",
        continent: "EU",
    };

    /// You as K1ABC, in the USA, in North America, W/VE.
    const K1: MyStation<'static> = MyStation {
        side: Side::WVe,
        country: "K",
        continent: "NA",
    };

    fn clean() -> QsoCheck {
        QsoCheck::default()
    }

    fn logged<'a>(at_s: u64, call: &'a str, check: &'a QsoCheck) -> ScoredQso<'a> {
        logged_with(at_s, call, &[], check)
    }

    /// A QSO logged at `at_s` seconds with `fields`, checked as `check`.
    fn logged_with<'a>(
        at_s: u64,
        call: &'a str,
        fields: &'a [(Field, String)],
        check: &'a QsoCheck,
    ) -> ScoredQso<'a> {
        ScoredQso {
            at_ms: at_s * 1_000,
            call,
            fields,
            check,
        }
    }

    fn sprint() -> Scoring {
        ContestKind::Sprint
            .rules()
            .scoring
            .expect("the Sprint scores")
    }

    fn wrong() -> QsoCheck {
        QsoCheck {
            errors: vec![QsoError::Field(crate::contest::Field::Nr)],
            dupe: false,
        }
    }

    /// A ten-minute run worked by hand: seven QSOs, one a busted call, one a
    /// wrong number, one NIL and one a dupe.
    #[test]
    fn a_hand_worked_wpx_log_scores_raw_and_verified() {
        let busted = QsoCheck {
            errors: vec![QsoError::Call {
                correct: "K5XYZ".into(),
            }],
            dupe: false,
        };
        let nil = QsoCheck {
            errors: vec![QsoError::Nil],
            dupe: false,
        };
        let dupe = QsoCheck {
            errors: Vec::new(),
            dupe: true,
        };
        let (clean, wrong) = (clean(), wrong());
        let log = [
            // WPX prefix K5: clean.
            logged(30, "K5ABC", &clean),
            // DL1: clean.
            logged(90, "DL1ABC", &clean),
            // K5 again (K5XYW was really K5XYZ): busted, raw only.
            logged(150, "K5XYW", &busted),
            // 9A1: wrong NR, raw only.
            logged(240, "9A1AA", &wrong),
            // W4 (W1XYZ/4): NIL, raw only.
            logged(330, "W1XYZ/4", &nil),
            // PA0 (PA/N8XYZ): clean.
            logged(420, "PA/N8XYZ", &clean),
            // DL1ABC again: a dupe copied right, which scores nothing.
            logged(500, "DL1ABC", &dupe),
        ];
        let score = contest_score(&wpx(), &DL, &log, 600_000);
        // Raw: six calls (the dupe dropped), prefixes K5 DL1 9A1 W4 PA0.
        assert_eq!(
            score.raw,
            Tally {
                qso_points: 6,
                mults: 5,
                score: 30
            }
        );
        // Verified: K5ABC, DL1ABC and PA/N8XYZ; prefixes K5 DL1 PA0.
        assert_eq!(
            score.verified,
            Tally {
                qso_points: 3,
                mults: 3,
                score: 9
            }
        );
        assert!(score.verified.score <= score.raw.score);
        // Seven logged in ten minutes is 42 an hour; the last five minutes
        // (from 5:00) hold the last three.
        assert_eq!(score.rates.whole_run, Some(42.0));
        assert_eq!(score.rates.last_5_min, Some(36.0));
        // The best five minutes, to 4:00, hold four: 48 an hour.
        assert_eq!(score.rates.best_5_min, Some(48.0));
    }

    /// A dupe costs nothing but its own points: it is no error, and the
    /// rest of the log scores as it would without it.
    #[test]
    fn a_dupe_scores_nothing_and_is_no_error() {
        let dupe = QsoCheck {
            errors: Vec::new(),
            dupe: true,
        };
        let clean = clean();
        assert!(dupe.is_clean());
        let one = [logged(1, "K5XYZ", &clean)];
        let with_dupe = [one[0], logged(2, "K5XYZ", &dupe)];
        let without = contest_score(&wpx(), &DL, &one, 60_000);
        let with = contest_score(&wpx(), &DL, &with_dupe, 60_000);
        assert_eq!(with.raw, without.raw);
        assert_eq!(with.verified, without.verified);
    }

    /// Verified never passes raw, whatever the log holds, in a contest
    /// whose multipliers come from the call or from what was logged.
    #[test]
    fn verified_is_never_above_raw() {
        use crate::rng::{Rng, SplitMix64};
        let calls = [
            "K5XYZ", "K5ABC", "DL1ABC", "DL2ABC", "9A1AA", "W1XYZ/4", "K5XYW",
        ];
        let checks = [
            clean(),
            wrong(),
            QsoCheck {
                errors: vec![QsoError::Nil],
                dupe: false,
            },
            QsoCheck {
                errors: Vec::new(),
                dupe: true,
            },
        ];
        let places: Vec<Vec<(Field, String)>> = ["OR", "MA", "NM", "MD", "DX", "LB"]
            .iter()
            .map(|st| {
                vec![
                    (Field::St, (*st).to_string()),
                    (Field::Zn, st.len().to_string()),
                ]
            })
            .collect();
        let mut rng = SplitMix64::new(4);
        for scoring in [wpx(), sprint(), cqww(), arrl_dx(), field_day()] {
            for _ in 0..500 {
                let log: Vec<ScoredQso<'_>> = (0..rng.usize_in(0, 12))
                    .map(|i| ScoredQso {
                        at_ms: i as u64 * 40_000,
                        call: calls[rng.usize_in(0, calls.len() - 1)],
                        fields: &places[rng.usize_in(0, places.len() - 1)],
                        check: &checks[rng.usize_in(0, checks.len() - 1)],
                    })
                    .collect();
                let score = contest_score(&scoring, &K1, &log, 600_000);
                assert!(score.verified.qso_points <= score.raw.qso_points);
                assert!(score.verified.mults <= score.raw.mults);
                assert!(score.verified.score <= score.raw.score);
            }
        }
    }

    /// A NIL logged with a call, then a clean QSO with the same call and
    /// another state: raw counts the call once, and the clean QSO's state
    /// among its multipliers, as verified counts that QSO, so verified
    /// stays under it. The same call logged once more, a dupe with a third
    /// state, adds a multiplier to neither, as a logger counts none for a
    /// dupe.
    #[test]
    fn a_clean_qso_after_a_nil_with_its_call_keeps_verified_under_raw() {
        let place = |st: &str| vec![(Field::St, st.to_string())];
        let (nm, md, ma) = (place("NM"), place("MD"), place("MA"));
        let (clean, nil, dupe) = (
            clean(),
            QsoCheck {
                errors: vec![QsoError::Nil],
                dupe: false,
            },
            QsoCheck {
                errors: Vec::new(),
                dupe: true,
            },
        );
        let at = |at_ms: u64, call, fields, check| ScoredQso {
            at_ms,
            call,
            fields,
            check,
        };
        let log = [
            at(20_000, "N5KGS", &nm, &clean),
            at(40_000, "W3BRL", &nm, &nil),
            at(60_000, "W3BRL", &md, &clean),
            at(80_000, "W3BRL", &ma, &dupe),
        ];
        let score = contest_score(&sprint(), &K1, &log, 600_000);
        let both = Tally {
            qso_points: 2,
            mults: 2,
            score: 4,
        };
        assert_eq!((score.raw, score.verified), (both, both));
    }

    /// A Sprint run worked by hand: a point a QSO, times each state,
    /// province or territory once, DC and Alaska among them, as logged for
    /// raw and as sent for verified; DX counts no multiplier.
    #[test]
    fn a_hand_worked_sprint_log_scores_raw_and_verified() {
        let place = |st: &str| vec![(Field::Nr, "1".to_string()), (Field::St, st.to_string())];
        let (or, ma, dc, ak, qc, pq, tx, dx) = (
            place("OR"),
            place("MA"),
            place("DC"),
            place("AK"),
            place("QC"),
            place("PQ"),
            place("TX"),
            place("DX"),
        );
        let clean = clean();
        let wrong_st = QsoCheck {
            errors: vec![QsoError::Field(Field::St)],
            dupe: false,
        };
        let dupe = QsoCheck {
            errors: Vec::new(),
            dupe: true,
        };
        let log = [
            logged_with(20, "W7XYZ", &or, &clean),
            // Oregon again: a QSO more, no multiplier more.
            logged_with(40, "K7ABC", &or, &clean),
            logged_with(60, "W1ABC", &ma, &clean),
            logged_with(80, "K3DC", &dc, &clean),
            logged_with(100, "KL7AB", &ak, &clean),
            logged_with(120, "VE2ABC", &qc, &clean),
            // PQ is Quebec, which is counted already.
            logged_with(140, "VA2XYZ", &pq, &clean),
            // Logged as Texas but sent as something else: raw counts TX.
            logged_with(160, "W5ABC", &tx, &wrong_st),
            // A DX station is a QSO, and no multiplier.
            logged_with(180, "DL1ABC", &dx, &clean),
            // A dupe scores nothing.
            logged_with(200, "W7XYZ", &or, &dupe),
        ];
        let score = contest_score(&sprint(), &K1, &log, 600_000);
        // Raw: nine calls; OR MA DC AK QC TX.
        assert_eq!(
            score.raw,
            Tally {
                qso_points: 9,
                mults: 6,
                score: 54
            }
        );
        // Verified: all but W5ABC; OR MA DC AK QC.
        assert_eq!(
            score.verified,
            Tally {
                qso_points: 8,
                mults: 5,
                score: 40
            }
        );
    }

    /// A CQ WW run worked by hand as DL1ABC: nothing for a German, a point
    /// for a European, three for anyone else, times each zone and each
    /// country once, Alaska and Hawaii countries of their own; raw counts
    /// what was logged, verified what was copied right.
    #[test]
    fn a_hand_worked_cqww_log_scores_raw_and_verified() {
        let zone = |zn: &str| vec![(Field::Zn, zn.to_string())];
        let (z14, a4, z25, z5, z1, z31, z15) = (
            zone("14"),
            zone("A4"),
            zone("25"),
            zone("5"),
            zone("1"),
            zone("31"),
            zone("15"),
        );
        let clean = clean();
        let busted = QsoCheck {
            errors: vec![QsoError::Call {
                correct: "JA1XYZ".into(),
            }],
            dupe: false,
        };
        let wrong_zone = QsoCheck {
            errors: vec![QsoError::Field(Field::Zn)],
            dupe: false,
        };
        let dupe = QsoCheck {
            errors: Vec::new(),
            dupe: true,
        };
        let log = [
            // Your own country: no points, but its zone and country count.
            logged_with(20, "DL2ABC", &z14, &clean),
            // Your continent, A4 read as zone 14: a point, and France.
            logged_with(40, "F5ABC", &a4, &clean),
            logged_with(60, "JA1XYZ", &z25, &clean),
            logged_with(80, "K1ABC", &z5, &clean),
            logged_with(100, "KL7AB", &z1, &clean),
            logged_with(120, "KH6AB", &z31, &clean),
            // A busted call is claimed for raw: three points, Japan again.
            logged_with(140, "JA1XYW", &z25, &busted),
            // A zone copied wrong: a point, England and zone 15 for raw.
            logged_with(160, "G4XYZ", &z15, &wrong_zone),
            // A dupe scores nothing.
            logged_with(180, "F5ABC", &z14, &dupe),
        ];
        let score = contest_score(&cqww(), &DL, &log, 600_000);
        // Raw: 0 + 1 + 3 + 3 + 3 + 3 + 3 + 1 points; zones 14 25 5 1 31
        // 15 and countries DL F JA K KL KH6 G.
        assert_eq!(
            score.raw,
            Tally {
                qso_points: 17,
                mults: 13,
                score: 221
            }
        );
        // Verified: the six clean; zones 14 25 5 1 31, countries DL F JA K
        // KL KH6.
        assert_eq!(
            score.verified,
            Tally {
                qso_points: 13,
                mults: 11,
                score: 143
            }
        );
    }

    /// Every CQ WW points rule, from Europe and from North America: none in
    /// your own country, one on your continent, two between North American
    /// countries, three on another continent; a call Dust cannot place is
    /// worth one, as in MorseRunner, and from a call it cannot place every
    /// placed station is on another continent. A pair in one country is
    /// worth nothing on two continents too, as in MorseRunner, whose list
    /// gives a country one continent.
    #[test]
    fn cqww_points_go_by_country_and_continent() {
        let clean = clean();
        let points = |me: &MyStation<'_>, call: &str| {
            let qso = logged_with(1, call, &[], &clean);
            contest_score(&cqww(), me, &[qso], 60_000).raw.qso_points
        };
        for (call, from_dl, from_k1) in [
            ("DL2ABC", 0, 3),
            ("DL/W1AW", 0, 3),
            ("F5ABC", 1, 3),
            ("UA3ABC", 1, 3),
            ("UA9ABC", 3, 3),
            ("JA1XYZ", 3, 3),
            ("K5XYZ", 3, 0),
            ("W1AW/4", 3, 0),
            ("VE3ABC", 3, 2),
            ("XE1AB", 3, 2),
            ("KP4AB", 3, 2),
            ("KL7AB", 3, 2),
            ("NL7AB", 3, 2),
            ("VG3ABC", 3, 2),
            ("KH6AB", 3, 3),
            // European Turkey is in Europe, the rest of Turkey in Asia.
            ("TA1XYZ", 1, 3),
            ("YM1AB", 1, 3),
            ("TA2XYZ", 3, 3),
            ("C31AB", 1, 1),
            ("KH2AB", 1, 1),
        ] {
            assert_eq!(points(&DL, call), from_dl, "{call} from DL");
            assert_eq!(points(&K1, call), from_k1, "{call} from K1");
        }
        // Your own country and continent, as a run keeps them: where your
        // call is at home, Alaska's and Canada's prefixes the call table
        // does not hold among them.
        let me = |call| MyStation {
            side: Side::of_call(call),
            country: home_country(call).unwrap_or_default(),
            continent: home_continent(call).unwrap_or_default(),
        };
        for (mine, call, worth) in [
            ("NL7AB", "KL7XYZ", 0),
            ("NL7AB", "K1ABC", 2),
            ("VG3ABC", "VE3XYZ", 0),
            ("VG3ABC", "K1ABC", 2),
            ("JA1XYZ", "TA1XYZ", 3),
            ("JA1XYZ", "TA2XYZ", 1),
            ("TA1ABC", "DL2ABC", 1),
            ("TA2ABC", "TA1XYZ", 0),
            ("TA1ABC", "TA2XYZ", 0),
            // VP2 is three countries, told apart by the letter after it.
            ("VP2E/K1ABC", "VP2MAB", 2),
            ("VP2E/K1ABC", "VP2EAB", 0),
            ("VP2E/K1ABC", "VP2VAB", 2),
        ] {
            assert_eq!(points(&me(mine), call), worth, "{call} from {mine}");
        }
        let nowhere = me("KH2AB");
        assert_eq!((nowhere.country, nowhere.continent), ("", ""));
        assert_eq!(points(&nowhere, "DL2ABC"), 3);
        assert_eq!(points(&nowhere, "KH6XYZ"), 3);
        assert_eq!(points(&nowhere, "C31AB"), 1);
    }

    /// ARRL DX worked by hand from either side: three points a QSO with the
    /// other side and none with your own; as DX, each state, province or
    /// territory of the 63 a W/VE call sends once, Labrador apart, and
    /// Alaska, which is DX, nothing; as W/VE, each DXCC country of a DX call
    /// once, Hawaii among them, and Canada, which is W/VE, nothing.
    #[test]
    fn a_hand_worked_arrl_dx_log_scores_by_your_side() {
        let st = |st: &str| vec![(Field::St, st.to_string())];
        let pwr = |pwr: &str| vec![(Field::Pwr, pwr.to_string())];
        let clean = clean();
        let (or, ma, on, lb, nl, ak, or2) = (
            st("OR"),
            st("MA"),
            st("ON"),
            st("LB"),
            st("NL"),
            st("AK"),
            st("OR"),
        );
        let as_dx = [
            logged_with(20, "W7XYZ", &or, &clean),
            logged_with(40, "K1ABC", &ma, &clean),
            logged_with(60, "VE3ABC", &on, &clean),
            logged_with(80, "VO2AB", &lb, &clean),
            logged_with(100, "VO1AB", &nl, &clean),
            logged_with(120, "KL7AB", &ak, &clean),
            logged_with(140, "K7ABC", &or2, &clean),
        ];
        let score = contest_score(&arrl_dx(), &DL, &as_dx, 600_000);
        // Six W/VE calls; OR MA ON LB NL.
        assert_eq!(
            score.verified,
            Tally {
                qso_points: 18,
                mults: 5,
                score: 90
            }
        );
        let (kw, hundred, five) = (pwr("KW"), pwr("100"), pwr("500"));
        let as_w_ve = [
            logged_with(20, "G4XYZ", &kw, &clean),
            logged_with(40, "DL1ABC", &hundred, &clean),
            logged_with(60, "JA1XYZ", &five, &clean),
            logged_with(80, "KH6AB", &kw, &clean),
            logged_with(100, "VE3ABC", &kw, &clean),
            logged_with(120, "DL2ABC", &hundred, &clean),
        ];
        let score = contest_score(&arrl_dx(), &K1, &as_w_ve, 600_000);
        // Five DX calls; G DL JA KH6.
        assert_eq!(
            score.verified,
            Tally {
                qso_points: 15,
                mults: 4,
                score: 60
            }
        );
        // A call on your own side, busted or not, claims nothing raw:
        // neither its points nor its place or country.
        let busted = QsoCheck {
            errors: vec![QsoError::Call {
                correct: "W7XYZ".into(),
            }],
            dupe: false,
        };
        let own_side = [logged_with(20, "G4XYZ", &ma, &busted)];
        let score = contest_score(&arrl_dx(), &DL, &own_side, 600_000);
        assert_eq!(score.raw, Tally::default());
        let own_side = [logged_with(20, "VE3XYZ", &hundred, &busted)];
        let score = contest_score(&arrl_dx(), &K1, &own_side, 600_000);
        assert_eq!(score.raw, Tally::default());
    }

    /// Field Day worked by hand: two points a QSO, nothing to multiply
    /// them, so the score is the points; a dupe scores nothing, and raw
    /// claims a busted call or a wrong section that verified drops. DX and
    /// W/VE are worth the same, from either side.
    #[test]
    fn a_hand_worked_field_day_log_scores_its_points() {
        let fd =
            |cl: &str, sec: &str| vec![(Field::Cl, cl.to_string()), (Field::Sec, sec.to_string())];
        let (or, ema, dx, pac, ter) = (
            fd("3A", "OR"),
            fd("1D", "EMA"),
            fd("1D", "DX"),
            fd("2A", "PAC"),
            fd("1B", "YT"),
        );
        let clean = clean();
        let busted = QsoCheck {
            errors: vec![QsoError::Call {
                correct: "W1ABC".into(),
            }],
            dupe: false,
        };
        let wrong_sec = QsoCheck {
            errors: vec![QsoError::Field(Field::Sec)],
            dupe: false,
        };
        let dupe = QsoCheck {
            errors: Vec::new(),
            dupe: true,
        };
        let log = [
            logged_with(20, "W7XYZ", &or, &clean),
            logged_with(40, "W1ABC", &ema, &clean),
            logged_with(60, "DL1ABC", &dx, &clean),
            logged_with(80, "KH6AB", &pac, &clean),
            logged_with(100, "VY1AB", &ter, &clean),
            // Claimed raw, dropped verified.
            logged_with(120, "W1ABD", &ema, &busted),
            logged_with(140, "K7ABC", &or, &wrong_sec),
            // A dupe scores nothing.
            logged_with(160, "W7XYZ", &or, &dupe),
        ];
        for me in [K1, DL] {
            let score = contest_score(&field_day(), &me, &log, 600_000);
            assert_eq!(
                score.raw,
                Tally {
                    qso_points: 14,
                    mults: 1,
                    score: 14
                }
            );
            assert_eq!(
                score.verified,
                Tally {
                    qso_points: 10,
                    mults: 1,
                    score: 10
                }
            );
        }
        let empty = contest_score(&field_day(), &K1, &[], 600_000);
        assert_eq!(
            (empty.raw, empty.verified),
            (Tally::default(), Tally::default())
        );
    }

    #[test]
    fn a_rate_counts_the_last_five_minutes_or_the_run_so_far() {
        assert_eq!(rate_at(&[], 0), None, "no rate at the start");
        assert_eq!(rate_at(&[], 60_000), Some(0.0));
        // One QSO in the first minute is 60 an hour.
        assert_eq!(rate_at(&[30_000], 60_000), Some(60.0));
        // Ten minutes in, only the last five count.
        let times = [60_000, 120_000, 400_000, 500_000, 590_000];
        assert_eq!(rate_at(&times, 600_000), Some(36.0));
        // A QSO exactly five minutes back has left the window.
        assert_eq!(rate_at(&[300_000], 600_000), Some(0.0));
        let empty = Rates::of(&[], 0);
        assert_eq!(empty, Rates::default());
    }
}
