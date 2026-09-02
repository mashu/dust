//! Freeze-aware practice streaks, derived from practice-day history.

pub const STREAK_FREEZE_EARN_DAYS: i64 = 7;
pub const MAX_STORED_FREEZES: i64 = 2;
const LOST_VISIBILITY_DAYS: i64 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreakState {
    None,
    Safe,
    AtRisk,
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreakStatus {
    pub days: u32,
    pub state: StreakState,
    pub freezes_available: u32,
    pub freezes_used: u32,
    pub lost_streak_days: Option<u32>,
}

impl StreakStatus {
    fn none() -> Self {
        Self {
            days: 0,
            state: StreakState::None,
            freezes_available: 0,
            freezes_used: 0,
            lost_streak_days: None,
        }
    }
}

/// Civil `YYYY-MM-DD` as days since Unix epoch (UTC calendar date).
pub fn date_to_day_index(date: &str) -> Option<i64> {
    let (y, m, d) = parse_ymd(date)?;
    Some(days_from_civil(y, m, d))
}

pub fn day_index_to_date(day_index: i64) -> String {
    let (y, m, d) = civil_from_days(day_index);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn parse_ymd(date: &str) -> Option<(i32, u32, u32)> {
    let mut parts = date.split('-');
    let y = parts.next()?.parse::<i32>().ok()?;
    let m = parts.next()?.parse::<u32>().ok()?;
    let d = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

/// Howard Hinnant's days-from-civil (Unix epoch 1970-01-01 = 0).
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let mut y = i64::from(year);
    let m = i64::from(month);
    let d = i64::from(day);
    y -= i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(day_index: i64) -> (i32, u32, u32) {
    let z = day_index + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    (year as i32, m as u32, d as u32)
}

/// Monday = 0 … Sunday = 6.
pub fn weekday_monday0(day_index: i64) -> u32 {
    ((day_index + 3).rem_euclid(7)) as u32
}

pub fn compute_streak_status(practice_dates: &[impl AsRef<str>], today: &str) -> StreakStatus {
    let mut day_indexes: Vec<i64> = practice_dates
        .iter()
        .filter_map(|d| date_to_day_index(d.as_ref()))
        .collect();
    day_indexes.sort_unstable();
    day_indexes.dedup();

    let Some(today_index) = date_to_day_index(today) else {
        return StreakStatus::none();
    };
    if day_indexes.is_empty() {
        return StreakStatus::none();
    }

    let mut streak: i64 = 0;
    let mut freezes: i64 = 0;
    let mut freezes_used: i64 = 0;
    let mut days_since_freeze_earned: i64 = 0;
    let mut previous: Option<i64> = None;

    let earn_on_practiced_day = |days_since: &mut i64, freezes: &mut i64| {
        *days_since += 1;
        if *days_since >= STREAK_FREEZE_EARN_DAYS {
            *freezes = (*freezes + 1).min(MAX_STORED_FREEZES);
            *days_since = 0;
        }
    };

    for day_index in day_indexes {
        if day_index > today_index {
            break;
        }
        if let Some(prev) = previous {
            let gap = day_index - prev - 1;
            if gap == 0 {
                streak += 1;
            } else if gap > 0 && gap <= freezes {
                freezes -= gap;
                freezes_used += gap;
                streak += 1;
            } else {
                streak = 1;
                freezes_used = 0;
                days_since_freeze_earned = 0;
            }
        } else {
            streak = 1;
            freezes_used = 0;
            days_since_freeze_earned = 0;
        }
        earn_on_practiced_day(&mut days_since_freeze_earned, &mut freezes);
        previous = Some(day_index);
    }

    let Some(last_practiced) = previous else {
        return StreakStatus::none();
    };

    if last_practiced == today_index {
        return StreakStatus {
            days: streak as u32,
            state: StreakState::Safe,
            freezes_available: freezes as u32,
            freezes_used: freezes_used as u32,
            lost_streak_days: None,
        };
    }

    let missed = today_index - last_practiced - 1;
    if missed == 0 {
        return StreakStatus {
            days: streak as u32,
            state: StreakState::AtRisk,
            freezes_available: freezes as u32,
            freezes_used: freezes_used as u32,
            lost_streak_days: None,
        };
    }
    if missed <= freezes {
        return StreakStatus {
            days: streak as u32,
            state: StreakState::AtRisk,
            freezes_available: (freezes - missed) as u32,
            freezes_used: (freezes_used + missed) as u32,
            lost_streak_days: None,
        };
    }

    if streak >= 2 && missed <= LOST_VISIBILITY_DAYS {
        return StreakStatus {
            days: 0,
            state: StreakState::Lost,
            freezes_available: freezes as u32,
            freezes_used: 0,
            lost_streak_days: Some(streak as u32),
        };
    }

    StreakStatus {
        days: 0,
        state: StreakState::None,
        freezes_available: freezes as u32,
        freezes_used: 0,
        lost_streak_days: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "2026-07-17";

    fn date_at(offset_days: i64) -> String {
        day_index_to_date(date_to_day_index(TODAY).expect("today") + offset_days)
    }

    #[test]
    fn roundtrips_unix_epoch() {
        assert_eq!(date_to_day_index("1970-01-01"), Some(0));
        assert_eq!(day_index_to_date(0), "1970-01-01");
        assert_eq!(
            date_to_day_index("2026-07-17"),
            date_to_day_index(&date_at(0))
        );
    }

    #[test]
    fn none_with_no_history() {
        let empty: [&str; 0] = [];
        assert_eq!(
            compute_streak_status(&empty, TODAY).state,
            StreakState::None
        );
    }

    #[test]
    fn safe_when_practiced_today() {
        let dates = [date_at(-2), date_at(-1), date_at(0)];
        let status = compute_streak_status(&dates, TODAY);
        assert_eq!(status.state, StreakState::Safe);
        assert_eq!(status.days, 3);
    }

    #[test]
    fn at_risk_when_yesterday_but_not_today() {
        let dates = [date_at(-3), date_at(-2), date_at(-1)];
        let status = compute_streak_status(&dates, TODAY);
        assert_eq!(status.state, StreakState::AtRisk);
        assert_eq!(status.days, 3);
    }

    #[test]
    fn earns_and_spends_a_freeze() {
        let mut dates: Vec<String> = (-10..=-4).map(date_at).collect();
        dates.push(date_at(-2));
        dates.push(date_at(-1));
        dates.push(date_at(0));
        let status = compute_streak_status(&dates, TODAY);
        assert_eq!(status.state, StreakState::Safe);
        assert_eq!(status.days, 10);
        assert_eq!(status.freezes_used, 1);
    }

    #[test]
    fn breaks_when_gap_exceeds_freezes() {
        let dates = [
            date_at(-8),
            date_at(-7),
            date_at(-6),
            date_at(-2),
            date_at(-1),
            date_at(0),
        ];
        let status = compute_streak_status(&dates, TODAY);
        assert_eq!(status.state, StreakState::Safe);
        assert_eq!(status.days, 3);
    }

    #[test]
    fn reports_recently_lost_streak() {
        let dates = [date_at(-6), date_at(-5), date_at(-4)];
        let status = compute_streak_status(&dates, TODAY);
        assert_eq!(status.state, StreakState::Lost);
        assert_eq!(status.days, 0);
        assert_eq!(status.lost_streak_days, Some(3));
    }

    #[test]
    fn hides_old_lapse() {
        let dates = [date_at(-40), date_at(-39)];
        assert_eq!(
            compute_streak_status(&dates, TODAY).state,
            StreakState::None
        );
    }

    #[test]
    fn at_risk_covered_by_banked_freezes() {
        let dates: Vec<String> = (-16..=-3).map(date_at).collect();
        let status = compute_streak_status(&dates, TODAY);
        assert_eq!(status.state, StreakState::AtRisk);
        assert_eq!(status.days, 14);
        assert_eq!(status.freezes_available, 0);
        assert_eq!(status.freezes_used, 2);
    }
}
