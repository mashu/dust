//! The Group length and Speed cards of the Sampling tab: for each length
//! and each speed the next group can come at, how well you copy it, and
//! its chance of coming next.
//!
//! They are drawn as "What comes up next" and Letter accuracy are: a bar a
//! row, as long as the share copied (groups whole, or characters right), in
//! the colour of where it stands against your aim, with a notch at the aim
//! and a paler stretch past the bar to the top of where it likely is. A
//! summary above says how far your aim reaches, never past the longest or
//! fastest you have been sent, and how your copy runs across the range; a
//! line under it, how the next one is drawn. Before any session of the set
//! they say there is none to read, rather than the model's first guess.

use cw_core::{
    CharSetMode, EstimateReading, SpeedForecast, SpeedPath, TrainingSettings, adapts_speed,
    group_length_forecast, speed_forecast,
};
use dioxus::prelude::*;

use crate::ui::figures::{History, without_band};
use crate::ui::help;
use crate::ui::settings_charset::mode_label;
use crate::ui::stats::{YOUR_SPEED_KEYS, contest_speeds, span_history_of, speed_history_of};
use crate::ui::widgets::{CardHead, IconName, control_id};

/// How far either side of the best guess the likely range runs, in its
/// spreads: the middle 80%, as the other cards' likely ranges hold.
const LIKELY_Z: f64 = 1.2816;
/// How far under the aim, as a share, a row is still near it.
const NEAR: f64 = 0.10;

/// Where a share copied stands against `aim`: at or over it, near under it,
/// or further under; in whole percents, as the row shows it, so that a row
/// reading 94% is at an aim of 94%.
fn standing(share: f64, aim: f64) -> &'static str {
    if meets(share, aim) {
        "solid"
    } else if whole_percent(share) >= whole_percent(aim) - NEAR * 100.0 {
        "building"
    } else {
        "to-work-on"
    }
}

/// Whether a share copied is at or over `aim`, in the whole percents the
/// rows show: what a row and the summary both go by.
fn meets(share: f64, aim: f64) -> bool {
    whole_percent(share) >= whole_percent(aim)
}

fn whole_percent(share: f64) -> f64 {
    (share * 100.0).round()
}

/// The range a share is likely in: the middle of what its spread allows.
fn likely(share: f64, sd: f64) -> (f64, f64) {
    (
        (share - LIKELY_Z * sd).clamp(0.0, 1.0),
        (share + LIKELY_Z * sd).clamp(0.0, 1.0),
    )
}

fn pct(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

/// One row: `key` at the left; a bar as long as `share`, in the colour of
/// where it stands against `aim`, paler past it to the top of where it
/// likely is, with a notch at the aim; the share beside it, and `note`
/// under it.
fn bar_row(id: String, key: String, share: f64, sd: f64, aim: f64, note: String) -> Element {
    let (low, high) = likely(share, sd);
    let along = |share: f64| (share * 100.0).clamp(0.0, 100.0);
    rsx! {
        div {
            id,
            class: "letter-row stat-row {standing(share, aim)}",
            role: "listitem",
            span { class: "letter-key mono", "{key}" }
            div { class: "bar-track marked",
                span {
                    class: "stat-range",
                    style: "left: {along(low):.1}%; width: {along(high) - along(low):.1}%;",
                }
                span { class: "bar-fill stat-fill", style: "width: {along(share):.1}%;" }
                span { class: "bar-mark", style: "left: {along(aim):.1}%;" }
            }
            span { class: "mono muted stat-value", "{pct(share)}" }
            span { class: "tiny", "{note}" }
        }
    }
}

/// The key under the summary: what each colour says against the aim.
fn legend(aim: f64) -> Element {
    rsx! {
        div { class: "err-legend", aria_hidden: "true",
            span { i { class: "err-swatch solid" } "at or over your aim" }
            span { i { class: "err-swatch building" } "within {NEAR * 100.0:.0} points" }
            span { i { class: "err-swatch to-work-on" } "further under" }
            span { i { class: "err-swatch range" } "where it likely is" }
            span { i { class: "err-swatch mark" } "your aim, {pct(aim)}" }
        }
    }
}

/// A line of the summary: its label, and what it says.
fn summary_row(label: String, text: String) -> Element {
    rsx! {
        div { class: "err-sum-row",
            span { class: "err-sum-label", "{label}" }
            span { class: "err-sum-text", "{text}" }
        }
    }
}

/// The longest length of `rows` copied whole `aim` of the time or more,
/// as the summary says it: by [`meets`], so that it never says "not yet"
/// of a row drawn at the aim; and never past `longest_sent`, the longest
/// group the history holds, past which the model only guesses.
fn length_reach(lengths: &[(usize, f64)], aim: f64, longest_sent: usize) -> String {
    let (Some((shortest, _)), Some((longest, _))) = (lengths.first(), lengths.last()) else {
        return String::new();
    };
    let reach = lengths
        .iter()
        .take_while(|(length, whole)| *length <= longest_sent && meets(*whole, aim))
        .last()
        .map(|(length, _)| *length);
    match reach {
        Some(length) if length == *longest => {
            format!("every length in your range, up to {longest} characters")
        }
        Some(length) if length == longest_sent => {
            format!("up to {length} characters, the longest sent yet")
        }
        Some(length) => format!("up to {length} characters"),
        None if *shortest > longest_sent => {
            format!("nothing as long as {shortest} characters sent yet")
        }
        None => format!("not yet at {shortest} characters"),
    }
}

/// A Sampling card for a character set with no session to read yet: its
/// head, and what it will show. Nothing is drawn from the model's first
/// guess alone.
fn no_history_card(
    icon: IconName,
    title: &'static str,
    help: Element,
    mode: CharSetMode,
    shows: &str,
) -> Element {
    rsx! {
        div { class: "card stack-sm",
            CardHead { icon, title, help }
            p { class: "muted hint",
                "No {mode_label(mode)} sessions to read yet. Once you have copied one, this shows {shows}."
            }
        }
    }
}

/// The group lengths the next session can send: how often a group that
/// long is copied whole against your aim, and how likely each is next.
/// Nothing for callsigns, which are as long as they are.
pub fn group_length_card(settings: &TrainingSettings, sessions: &History) -> Element {
    let rows = sessions.figure(
        "group length forecast",
        without_band(settings),
        |sessions| group_length_forecast(settings, sessions),
    );
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return rsx! {};
    };
    let aim = settings.curriculum.aim.groups_whole();
    let Some(sent) = span_history_of(sessions, settings)
        .last()
        .map(|point| point.sent)
    else {
        return no_history_card(
            IconName::Signal,
            "Group length",
            help::group_length(aim),
            settings.curriculum.char_set_mode,
            "how often you copy each length whole, and how likely each is to come next",
        );
    };
    let curriculum = &settings.curriculum;
    let note = if !curriculum.adaptive_group_size {
        "Adaptive group size is off, so every length is as likely as the next.".to_string()
    } else if rows.len() == 1 {
        "The group size is fixed, so there is no length to choose.".to_string()
    } else {
        format!(
            "Adaptive group size leans each group's length towards where you copy {} of groups whole.",
            pct(aim)
        )
    };
    let reach = length_reach(
        &rows
            .iter()
            .map(|row| (row.length, row.whole))
            .collect::<Vec<_>>(),
        aim,
        sent.1 as usize,
    );
    let across = if rows.len() > 1 {
        format!(
            "{} whole at {} characters, {} at {}",
            pct(first.whole),
            first.length,
            pct(last.whole),
            last.length
        )
    } else {
        format!("{} whole at {} characters", pct(first.whole), first.length)
    };
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Signal,
                title: "Group length",
                note: "How often you copy a group that long without a mistake, and its chance of coming up next",
                help: help::group_length(aim),
            }
            div { class: "err-summary",
                {summary_row(format!("Your aim · {} whole", pct(aim)), reach)}
                {summary_row("Across your range".to_string(), across)}
            }
            {legend(aim)}
            p { class: "muted hint", "{note}" }
            div { class: "stat-rows", role: "list",
                for row in rows.iter() {
                    {
                        let (low, high) = likely(row.whole, row.whole_sd);
                        bar_row(
                            control_id("next-length", &row.length.to_string()),
                            row.length.to_string(),
                            row.whole,
                            row.whole_sd,
                            aim,
                            format!(
                                "likely {:.0}–{:.0}% whole · {} chance next",
                                low * 100.0,
                                high * 100.0,
                                pct(row.drawn)
                            ),
                        )
                    }
                }
            }
        }
    }
}

/// How far your aim reaches in speed, against the range: "about 22 WPM",
/// "about 15 WPM, under your range"; past the fastest or slowest you have
/// been sent, only that: "over 20 WPM, the fastest sent yet".
fn speed_reach(copy: EstimateReading, low: f64, high: f64) -> String {
    match copy {
        EstimateReading::About(copy) if copy.mean < low - 0.5 => {
            format!("about {:.0} WPM, under your range", copy.mean)
        }
        EstimateReading::About(copy) if copy.mean > high + 0.5 => {
            format!("about {:.0} WPM, over your range", copy.mean)
        }
        EstimateReading::About(copy) => format!("about {:.0} WPM", copy.mean),
        EstimateReading::Over(fastest) => format!("over {fastest:.0} WPM, the fastest sent yet"),
        EstimateReading::Under(slowest) => {
            format!("under {slowest:.0} WPM, the slowest sent yet")
        }
    }
}

/// The speeds the next session can send at: the share of characters
/// copied right at each against your aim, and how likely the next group is
/// to be sent at about each.
pub fn speed_card(settings: &TrainingSettings, sessions: &History) -> Element {
    let rows = sessions.figure("speed forecast", without_band(settings), |sessions| {
        speed_forecast(settings, sessions)
    });
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return rsx! {};
    };
    let aim = settings.curriculum.aim.chars_right();
    // The speed your aim is reached at, as the Overview's copy speed says.
    let Some(reach) = speed_history_of(sessions, settings)
        .last()
        .map(|point| speed_reach(point.reading(), first.wpm, last.wpm))
    else {
        return no_history_card(
            IconName::Gauge,
            "Speed",
            help::speed(aim),
            settings.curriculum.char_set_mode,
            "how much you copy right at each speed, and how likely each is to come next",
        );
    };
    let playback = &settings.playback;
    let contest = settings.curriculum.char_set_mode == CharSetMode::Contest;
    let mut note = if contest && adapts_speed(settings) {
        adapting_callers_note(&rows)
    } else if contest {
        contest_speeds(settings)
    } else if !playback.adaptive_speed {
        "Adaptive speed is off, so each group's speed is drawn evenly from your range.".to_string()
    } else if !SpeedPath::of(settings).has_room() {
        "One speed is set, so there is no speed to choose.".to_string()
    } else {
        format!(
            "Adaptive speed leans each group's speed towards where you copy {} of characters right.",
            pct(aim)
        )
    };
    if !playback.link_char_to_effective {
        note.push_str(
            " With Farnsworth spacing, a speed counts the spacing too, as the score does.",
        );
    }
    let across = if rows.len() > 1 {
        format!(
            "{} right at {:.0} WPM, {} at {:.0}",
            pct(first.right),
            first.wpm,
            pct(last.right),
            last.wpm
        )
    } else {
        format!("{} right at {:.0} WPM", pct(first.right), first.wpm)
    };
    let next = if contest {
        "chance of the first callers"
    } else {
        "chance next"
    };
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Gauge,
                title: "Speed",
                note: "How much you copy right at each speed, in WPM, and its chance of coming up next",
                help: help::speed(aim),
            }
            div { class: "err-summary",
                {summary_row(format!("Your aim · {} right", pct(aim)), reach)}
                {summary_row("Across your range".to_string(), across)}
            }
            {legend(aim)}
            p { class: "muted hint", "{note}" }
            div { class: "stat-rows", role: "list",
                for row in rows.iter() {
                    {
                        let (low, high) = likely(row.right, row.right_sd);
                        bar_row(
                            control_id("next-speed", &format!("{:.0}", row.wpm)),
                            format!("{:.0}", row.wpm),
                            row.right,
                            row.right_sd,
                            aim,
                            format!(
                                "likely {:.0}–{:.0}% right · {} {next}",
                                low * 100.0,
                                high * 100.0,
                                pct(row.drawn)
                            ),
                        )
                    }
                }
            }
        }
    }
}

/// What the speed card says of a contest's callers while their speed
/// adapts: its chances are the first callers' of the next run, which the
/// run moves on from, and when most of them sit at an end of the range, the
/// copy speed is past it.
fn adapting_callers_note(rows: &[SpeedForecast]) -> String {
    let mut note = format!(
        "The chances are for the first callers of your next run. Within the run they come faster as you copy calls and numbers clean, slower as you miss them. {YOUR_SPEED_KEYS}"
    );
    let most = |row: Option<&SpeedForecast>| row.is_some_and(|row| row.drawn > 0.5);
    if most(rows.last()) {
        note.push_str(" Your copy speed is above this range, so most come at its top: a wider range on the Speed card lets them go faster.");
    } else if most(rows.first()) {
        note.push_str(" Your copy speed is below this range, so most come at its bottom: a wider range on the Speed card lets them go slower.");
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At or over the aim is solid, within ten points under it near, and
    /// further under to work on.
    #[test]
    fn a_row_stands_against_the_aim() {
        assert_eq!(standing(0.95, 0.94), "solid");
        assert_eq!(standing(0.94, 0.94), "solid");
        assert_eq!(standing(0.937, 0.94), "solid", "it reads 94%");
        assert_eq!(standing(0.85, 0.94), "building");
        assert_eq!(standing(0.83, 0.94), "to-work-on");
    }

    /// The likely range is the middle 80% of the spread, kept to shares.
    #[test]
    fn the_likely_range_is_kept_to_shares() {
        let (low, high) = likely(0.5, 0.1);
        assert!((low - 0.3718).abs() < 1e-4 && (high - 0.6282).abs() < 1e-4);
        assert_eq!(likely(0.98, 0.1), (0.98 - LIKELY_Z * 0.1, 1.0));
        assert_eq!(likely(0.5, 0.0), (0.5, 0.5));
    }

    /// The summary says how far the aim reaches: the longest length at or
    /// over it, every length, or none yet; a speed, and where it sits
    /// against the range.
    #[test]
    fn the_summary_says_how_far_the_aim_reaches() {
        let lengths = [(2, 0.9), (3, 0.75), (4, 0.6)];
        assert_eq!(length_reach(&lengths, 0.7, 4), "up to 3 characters");
        assert_eq!(
            length_reach(&lengths, 0.5, 4),
            "every length in your range, up to 4 characters"
        );
        assert_eq!(length_reach(&lengths, 0.95, 4), "not yet at 2 characters");
        let about = |mean| EstimateReading::About(cw_core::Estimate { mean, sd: 1.0 });
        assert_eq!(speed_reach(about(21.6), 18.0, 25.0), "about 22 WPM");
        assert_eq!(
            speed_reach(about(15.2), 18.0, 25.0),
            "about 15 WPM, under your range"
        );
        assert_eq!(
            speed_reach(about(27.0), 18.0, 25.0),
            "about 27 WPM, over your range"
        );
    }

    /// The summary never reaches past the longest group or the fastest
    /// speed the history holds: past them the model only guesses.
    #[test]
    fn the_summary_never_reaches_past_what_was_sent() {
        let lengths = [(2, 0.97), (3, 0.95), (4, 0.9), (5, 0.8)];
        assert_eq!(
            length_reach(&lengths, 0.7, 3),
            "up to 3 characters, the longest sent yet"
        );
        assert_eq!(
            length_reach(&lengths[2..], 0.7, 3),
            "nothing as long as 4 characters sent yet"
        );
        assert_eq!(
            speed_reach(EstimateReading::Over(20.0), 15.0, 30.0),
            "over 20 WPM, the fastest sent yet"
        );
        assert_eq!(
            speed_reach(EstimateReading::Under(15.0), 15.0, 30.0),
            "under 15 WPM, the slowest sent yet"
        );
    }

    /// The summary and the rows agree on what is at the aim: a length copied
    /// whole 93.7% of the time reads 94%, solid at an aim of 94%, and the
    /// summary reaches it. It used to say "not yet" under a solid row.
    #[test]
    fn the_summary_reaches_a_row_drawn_at_the_aim() {
        let aim = 0.94;
        assert_eq!(standing(0.937, aim), "solid");
        assert_eq!(
            length_reach(&[(2, 0.937)], aim, 2),
            "every length in your range, up to 2 characters"
        );
        assert_eq!(
            length_reach(&[(2, 0.96), (3, 0.937), (4, 0.934)], aim, 4),
            "up to 3 characters"
        );
        assert_eq!(standing(0.934, aim), "building", "it reads 93%");
        assert_eq!(
            length_reach(&[(2, 0.934)], aim, 2),
            "not yet at 2 characters"
        );
    }
}
