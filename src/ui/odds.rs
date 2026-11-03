//! How often callers do each thing they do now and then, on sliders: each
//! share of [`CallerOdds`] from nothing to all, folded away under a line
//! that says whether any is moved, as the Band card folds its receiver.

use cw_core::{CallerOdds, Odd, TrainingSettings};
use dioxus::prelude::*;

use crate::ui::widgets::{DISCLOSURE, Icon, IconName, SliderField, control_id};

/// What a share's slider is called on screen.
pub(crate) fn odd_label(odd: Odd) -> &'static str {
    match odd {
        Odd::Dupe => "Callers you worked before",
        Odd::Portable => "Calls signed portable",
        Odd::Abroad => "Calls from abroad",
        Odd::FieldDayDx => "DX callers",
        Odd::CallsTwice => "Send their call twice",
        Odd::MissesExchange => "Miss your exchange",
        Odd::AsksNr => "Ask NR? rather than AGN",
        Odd::FixesWithExchange => "Send their exchange with a fix",
        Odd::Farewell44 => "Your TU signs 73 44",
        Odd::Flutter => "Callers that flutter",
        Odd::Enn => "5NN sent as ENN",
        Odd::SerialZeroAsT => "Serial zeros sent as T",
        Odd::NineAsN => "Nines sent as N",
        Odd::ZeroAsO => "Other zeros sent as O",
        Odd::ZeroAsT => "Other zeros sent as T",
        Odd::ZoneCut => "Zones and power cut",
        Odd::ZonePadded => "Zones padded with 0",
        Odd::Fumbles => "Fumble a serial",
        Odd::Early => "Send before you finish",
        Odd::Slips => "Take a wrong call",
    }
}

/// A share as its slider says it, in percent: whole percents, and a tenth
/// where it is below ten and not whole, so a share as small as Field Day's
/// DX still reads.
pub(crate) fn percent_label(share: f64) -> String {
    let percent = share * 100.0;
    if percent < 10.0 && (percent - percent.round()).abs() > 0.05 {
        format!("{percent:.1}%")
    } else {
        format!("{percent:.0}%")
    }
}

/// How many of `odds` set on `shown` are moved from where they start.
pub(crate) fn moved(odds: &CallerOdds, shown: &[Odd]) -> usize {
    let start = CallerOdds::default();
    shown
        .iter()
        .filter(|odd| (odd.of(odds) - odd.of(&start)).abs() > 1e-9)
        .count()
}

/// One share's slider, from nothing to all in whole percents. `disabled`
/// where what it sets does not come in the run as it is set, as habits'
/// numbers under easy habits.
#[component]
pub(crate) fn OddSlider(
    settings: Signal<TrainingSettings>,
    odd: Odd,
    share: f64,
    disabled: bool,
) -> Element {
    let label = odd_label(odd);
    rsx! {
        SliderField {
            id: control_id("odds", odd.name()),
            label,
            value_label: percent_label(share),
            value: share * 100.0,
            min: 0.0,
            max: 100.0,
            step: 1.0,
            disabled,
            onchange: move |percent: f64| {
                *odd.of_mut(&mut settings.write().contest.caller_odds) = (percent / 100.0)
                    .clamp(0.0, 1.0);
            },
        }
    }
}

/// A group of shares under a heading, each on its slider, all `disabled`
/// together where the run as it is set has none of them.
#[derive(Clone, PartialEq)]
pub(crate) struct OddsGroup {
    pub head: &'static str,
    pub odds: Vec<Odd>,
    pub disabled: bool,
}

/// Shares folded away under a line, `title`, that says whether any is
/// moved: open, each group under its heading, a slider for each share.
#[component]
pub(crate) fn OddsFold(
    settings: Signal<TrainingSettings>,
    title: &'static str,
    odds: CallerOdds,
    groups: Vec<OddsGroup>,
) -> Element {
    let mut open = use_signal(|| false);
    let shown: Vec<Odd> = groups.iter().flat_map(|group| group.odds.clone()).collect();
    let reading = match moved(&odds, &shown) {
        0 => "as on the air".to_string(),
        1 => "1 moved".to_string(),
        count => format!("{count} moved"),
    };
    let panel = control_id("odds", title);
    rsx! {
        button {
            id: control_id(DISCLOSURE, title),
            class: if open() { "link-toggle open" } else { "link-toggle" },
            style: "align-self: flex-start;",
            r#type: "button",
            aria_expanded: if open() { "true" } else { "false" },
            aria_controls: "{panel}",
            onclick: move |_| open.toggle(),
            "{title}: {reading}"
            Icon { name: IconName::Chevron }
        }
        if open() {
            div { id: "{panel}", class: "stack-sm",
                for group in groups {
                    div { class: "eyebrow", "{group.head}" }
                    for odd in group.odds {
                        OddSlider {
                            settings,
                            odd,
                            share: odd.of(&odds),
                            disabled: group.disabled,
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shares read in whole percents, and a tenth where small and not
    /// whole.
    #[test]
    fn shares_read_in_percent() {
        assert_eq!(percent_label(0.3), "30%");
        assert_eq!(percent_label(0.009), "0.9%");
        assert_eq!(percent_label(0.04), "4%");
        assert_eq!(percent_label(0.88 - 5.0 / 6.0), "4.7%");
        assert_eq!(percent_label(5.0 / 6.0), "83%");
        assert_eq!(percent_label(1.0), "100%");
        assert_eq!(percent_label(0.0), "0%");
    }

    /// Every share has a name of its own on screen, and only moved ones
    /// count as moved.
    #[test]
    fn every_share_is_named_and_counted_when_moved() {
        let names: std::collections::HashSet<&str> =
            Odd::ALL.iter().map(|odd| odd_label(*odd)).collect();
        assert_eq!(names.len(), Odd::ALL.len());
        let mut odds = CallerOdds::default();
        assert_eq!(moved(&odds, &Odd::ALL), 0);
        odds.flutter = 0.5;
        odds.dupe = 0.0;
        assert_eq!(moved(&odds, &Odd::ALL), 2);
        assert_eq!(moved(&odds, &[Odd::Flutter]), 1);
    }
}
