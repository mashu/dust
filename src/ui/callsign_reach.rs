//! What the callsign tier sends, drawn on the Character set card rather
//! than told, in Callsigns and in Contest, which share the tier: the row of
//! tiers, each labelled with the shape of call it adds and filled up to the
//! tier chosen, and where the calls come from, as a bar split between the
//! United States, the next commonest countries and all the others. Callsigns
//! draws its calls as evenly as its country balance has them; a contest
//! draws its callers from the countries it hears, as on the air. The words
//! for both are in the card's help ([`crate::ui::help::character_set`]).

use cw_core::{
    CALLSIGN_TIER_MAX, CALLSIGN_TIER_MIN, CallSpec, callers_by_country, country_name, tier_adds,
    tier_shape,
};
use dioxus::prelude::*;

/// How many of the commonest countries after the United States the bar
/// draws as one part, between it and all the others.
const NEXT: usize = 9;

/// The share of the bar a part needs before it prints its own share
/// inside: below it, "35%" would not fit a phone's bar with room on both
/// sides, and the part's tap, tooltip and accessible name carry it.
const LABELLED: f64 = 0.12;

/// A share as the bar and its parts say it: whole percent, a half going to
/// the even one as [`crate::format::percent`] has it, and "under 1%" rather
/// than a 0% that is not nothing.
pub fn percent(share: f64) -> String {
    let percent = crate::format::whole_percent(share);
    if percent < 1.0 {
        "under 1%".to_string()
    } else {
        format!("{percent}%")
    }
}

/// A country's name as the table gives it, or its prefix.
fn named(country: &str) -> &str {
    country_name(country).unwrap_or(country)
}

/// Where a tier's calls come from: how many countries, and the parts the
/// bar is split into, in the order it draws them.
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub countries: usize,
    pub parts: Vec<Part>,
}

/// One part of the bar.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// What it is, and its class: `lead` the United States, `next` the
    /// commonest countries after it, `rest` all the others.
    pub kind: &'static str,
    /// What the legend calls it: "Next 9 countries".
    pub name: String,
    /// Its share of the calls, from 0 to 1.
    pub share: f64,
    /// Who is in it, with their shares, as a tap on it shows and its
    /// tooltip says: "Germany 4%, Japan 3%, …".
    pub who: String,
}

impl Split {
    /// The calls drawn at `tier` to `spec`: Callsigns' practice, evened
    /// out as its balance has them, or a contest's callers.
    pub fn of(tier: u32, spec: CallSpec) -> Self {
        let shares = callers_by_country(tier, spec);
        let countries = shares.len();
        let (us, others): (Vec<_>, Vec<_>) =
            shares.into_iter().partition(|(country, _)| *country == "K");
        let (next, rest) = if others.len() > NEXT {
            others.split_at(NEXT)
        } else {
            others.split_at(0)
        };
        let total = |part: &[(&str, f64)]| part.iter().map(|(_, share)| share).sum::<f64>();
        let mut parts = Vec::new();
        if let Some((_, share)) = us.first() {
            parts.push(Part {
                kind: "lead",
                name: named("K").to_string(),
                share: *share,
                who: format!("{} {}", named("K"), percent(*share)),
            });
        }
        if !next.is_empty() {
            let who: Vec<String> = next
                .iter()
                .map(|(country, share)| format!("{} {}", named(country), percent(*share)))
                .collect();
            // After the United States the next; with none, the commonest.
            let name = if us.is_empty() {
                format!("{} commonest countries", next.len())
            } else {
                format!("Next {} countries", next.len())
            };
            parts.push(Part {
                kind: "next",
                name,
                share: total(next),
                who: who.join(", "),
            });
        }
        if let Some((commonest, most)) = rest.first() {
            // One country alone is named: W/VE's callers are the US and
            // Canada.
            let name = if rest.len() == 1 {
                named(commonest).to_string()
            } else {
                format!("{} other countries", rest.len())
            };
            let who = if rest.len() == 1 {
                format!("{} {}", named(commonest), percent(*most))
            } else {
                format!(
                    "{name} together {}; the commonest, {}, {}",
                    percent(total(rest)),
                    named(commonest),
                    percent(*most)
                )
            };
            parts.push(Part {
                kind: "rest",
                name,
                share: total(rest),
                who,
            });
        }
        Self { countries, parts }
    }

    /// The bar's accessible name: every part, with its share.
    pub fn label(&self) -> String {
        let parts: Vec<String> = self
            .parts
            .iter()
            .map(|part| format!("{} {}", part.name, percent(part.share)))
            .collect();
        format!(
            "Calls from {} countries: {}",
            self.countries,
            parts.join(", ")
        )
    }
}

/// The row of tiers, `tier` chosen: each labelled with the shape of call
/// it adds, those it sends filled. A picture of the ladder, not a control:
/// the tier field above sets it.
#[component]
pub fn TierSteps(tier: u32) -> Element {
    let sent: Vec<&str> = (CALLSIGN_TIER_MIN..=tier).map(tier_shape).collect();
    let label = format!(
        "Tier {tier} of {CALLSIGN_TIER_MAX}, sending calls shaped as {}",
        sent.join(", ")
    );
    let steps: Vec<(u32, &str)> = (CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX)
        .map(|step| {
            let class = match step.cmp(&tier) {
                std::cmp::Ordering::Less => "tier-step in",
                std::cmp::Ordering::Equal => "tier-step in at",
                std::cmp::Ordering::Greater => "tier-step",
            };
            (step, class)
        })
        .collect();
    rsx! {
        div { id: "tier-steps", class: "tier-steps", role: "img", aria_label: "{label}",
            for (step, class) in steps {
                span { class, title: "Tier {step}: {tier_adds(step)}",
                    span { class: "tier-num", "{step}" }
                    span { class: "tier-pip" }
                    span { class: "tier-shape", "{tier_shape(step)}" }
                }
            }
        }
    }
}

/// Where the tier's calls come from, as a tall bar split into its parts,
/// each its share wide and printing its share where it fits, over a
/// legend of their names. A tap on a part, or Enter on it, puts who is in
/// it in the legend's place; again, and the legend is back.
#[component]
pub fn CountrySplit(split: Split) -> Element {
    let mut open = use_signal(|| None::<&'static str>);
    let shown = open().and_then(|kind| split.parts.iter().find(|part| part.kind == kind));
    rsx! {
        div { id: "country-split", class: "country-split", role: "group", aria_label: "{split.label()}",
            for part in split.parts.iter().filter(|part| part.share > 0.0) {
                button {
                    r#type: "button",
                    id: "split-{part.kind}",
                    class: "split-part {part.kind}",
                    style: "flex-grow: {part.share};",
                    title: "{part.who}",
                    aria_label: "{part.name}: {part.who}",
                    aria_pressed: shown.is_some_and(|open| open.kind == part.kind),
                    onclick: {
                        let kind = part.kind;
                        move |_| open.set(if open() == Some(kind) { None } else { Some(kind) })
                    },
                    if part.share >= LABELLED {
                        span { class: "split-share", "{percent(part.share)}" }
                    }
                }
            }
        }
        if let Some(part) = shown {
            p { id: "split-who", class: "split-who", "{part.who}" }
        } else {
            div { class: "split-legend", aria_hidden: "true",
                for part in split.parts.iter() {
                    span { class: "split-key",
                        span { class: "split-swatch {part.kind}" }
                        "{part.name}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Callsigns' calls at `tier`, evened out by `balance`.
    fn practice(tier: u32, balance: f64) -> Split {
        Split::of(tier, CallSpec::default().with_balance(balance))
    }

    /// The parts are the whole, the United States first; past ten
    /// countries the next nine are a part of their own.
    #[test]
    fn the_bar_is_the_whole_split_three_ways_or_two() {
        let many = practice(4, 0.5);
        assert_eq!(many.countries, 126);
        let kinds: Vec<&str> = many.parts.iter().map(|part| part.kind).collect();
        assert_eq!(kinds, ["lead", "next", "rest"]);
        assert_eq!(many.parts[1].name, "Next 9 countries");
        assert_eq!(many.parts[2].name, "116 other countries");
        let sum: f64 = many.parts.iter().map(|part| part.share).sum();
        assert!((sum - 1.0).abs() < 1e-9);
        assert!(many.parts[1].who.starts_with("Fed. Rep. of Germany"));
        assert_eq!(many.parts[1].who.matches(", ").count(), 8, "all nine named");

        let few = practice(1, 0.0);
        let kinds: Vec<&str> = few.parts.iter().map(|part| part.kind).collect();
        assert_eq!(kinds, ["lead", "rest"]);
        assert_eq!(few.parts[1].name, "8 other countries");
        assert_eq!(percent(few.parts[0].share), "86%");
        assert_eq!(
            few.label(),
            "Calls from 9 countries: United States 86%, 8 other countries 14%"
        );
    }

    /// Evened out, the United States is a sliver and the others most of
    /// the bar; and a share too small for a whole percent says so.
    #[test]
    fn evened_out_the_others_are_most_of_the_bar() {
        let even = practice(4, 1.0);
        assert_eq!(percent(even.parts[0].share), "1%");
        assert!(even.parts[2].share > 0.9, "{:?}", even.parts[2]);
        assert_eq!(percent(0.004), "under 1%");
        assert_eq!(percent(0.351), "35%");
    }

    /// A contest's bar is its callers': W/VE's the US and Canada, the DX
    /// side's everyone but them, with no United States to lead it.
    #[test]
    fn a_contests_bar_is_its_callers() {
        use cw_core::Countries;
        let w_ve = Split::of(4, CallSpec::contest(Countries::WVe));
        assert_eq!(w_ve.countries, 2);
        let kinds: Vec<&str> = w_ve.parts.iter().map(|part| part.kind).collect();
        assert_eq!(kinds, ["lead", "rest"]);
        assert_eq!(w_ve.parts[1].name, "Canada");
        assert!(w_ve.parts[1].who.starts_with("Canada "));

        let dx = Split::of(4, CallSpec::contest(Countries::NotWVe));
        let kinds: Vec<&str> = dx.parts.iter().map(|part| part.kind).collect();
        assert_eq!(kinds, ["next", "rest"]);
        assert_eq!(dx.parts[0].name, "9 commonest countries");
    }

    /// A share half way between two whole percents reads the even one, as
    /// every other share does; one that would read 0% reads under 1%.
    #[test]
    fn a_share_reads_in_whole_percents_as_everywhere_else() {
        assert_eq!(percent(0.125), "12%");
        assert_eq!(percent(0.625), "62%");
        assert_eq!(percent(0.35), "35%");
        assert_eq!(percent(0.015), "2%");
        assert_eq!(percent(0.005), "under 1%");
    }
}
