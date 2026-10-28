//! Who makes Dust: the author, the maintainers who publish it in a store
//! under their own account, everyone else who has worked on it, and the
//! closed testing group. The card sits in Settings, beside the privacy card,
//! so a store reviewer or a user can see whose app it is without leaving it.
//! It starts folded to its head, and opens when the head is pressed: the list
//! only grows, and the settings above it are what the screen is for.

use dioxus::prelude::*;

use crate::ui::widgets::{FoldHead, FoldPanel};

/// Where the code, and the issues, live.
pub const SOURCE_URL: &str = "https://github.com/mashu/dust";

/// What someone does for Dust. The card lists them in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Created Dust, leads its development, and builds it.
    Author,
    /// Publishes a build in a store under their own account.
    Maintainer,
    /// Code, design, ideas: any other work on the app. Testers have their
    /// own list, [`TESTERS`].
    Contributor,
}

impl Role {
    const ORDER: [Self; 3] = [Self::Author, Self::Maintainer, Self::Contributor];

    /// The heading over the people in this role.
    fn heading(self, count: usize) -> &'static str {
        match (self, count) {
            (Self::Author, 1) => "Author",
            (Self::Author, _) => "Authors",
            (Self::Maintainer, 1) => "Maintainer",
            (Self::Maintainer, _) => "Maintainers",
            (Self::Contributor, 1) => "Contributor",
            (Self::Contributor, _) => "Contributors",
        }
    }
}

/// One person on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Credit {
    pub name: &'static str,
    /// Their amateur radio callsign, if they have one.
    pub callsign: Option<&'static str>,
    pub role: Role,
    /// What they do for Dust, in a sentence.
    pub work: &'static str,
}

/// Everyone the app credits. To be listed, add a line; a role with nobody
/// in it is left off the card. A maintainer who publishes Dust in a store
/// reads:
///
/// ```text
/// Credit {
///     name: "Their Name",
///     callsign: Some("CALL"),
///     role: Role::Maintainer,
///     work: "Publishes Dust on the App Store.",
/// },
/// ```
pub const CREDITS: &[Credit] = &[
    Credit {
        name: "Mateusz K",
        callsign: Some("SO5KM"),
        role: Role::Author,
        work: "Created Dust, leads its development and builds every release.",
    },
    Credit {
        name: "Michał",
        callsign: Some("SP5TLS"),
        role: Role::Maintainer,
        work: "Publishes Dust on the iOS App Store.",
    },
];

/// Someone in the closed testing group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tester {
    pub name: &'static str,
    /// Their amateur radio callsign, if they have one.
    pub callsign: Option<&'static str>,
}

/// The closed testing group: hams who tried Dust before its release and
/// sent feedback, in the order the card shows them. They are a few of many,
/// and the card says so. To credit one more, add a line.
pub const TESTERS: &[Tester] = &[
    Tester {
        name: "Pawel",
        callsign: Some("SP5DNA"),
    },
    Tester {
        name: "Szymon",
        callsign: Some("SO7NA"),
    },
    Tester {
        name: "Ryszard",
        callsign: Some("M0RPA"),
    },
    Tester {
        name: "Mariusz",
        callsign: Some("SP8KM"),
    },
    Tester {
        name: "Dariusz",
        callsign: Some("SQ5VDR"),
    },
    Tester {
        name: "Mariusz",
        callsign: Some("SP7UDB"),
    },
    Tester {
        name: "Krzysztof",
        callsign: Some("SP8BFG"),
    },
    Tester {
        name: "Czarek",
        callsign: Some("SP2CRY"),
    },
    Tester {
        name: "Damian",
        callsign: Some("SP9CRB"),
    },
];

/// Dust is its own work. Contest mode took a few ideas from how
/// MorseRunner plays a contest, and one line says so.
pub const INSPIRATION: &str = "Contest mode takes some inspiration from MorseRunner.";

/// The people in `credits`, by role, in the card's order, leaving out the
/// roles nobody holds.
fn by_role(credits: &[Credit]) -> Vec<(Role, Vec<Credit>)> {
    Role::ORDER
        .into_iter()
        .map(|role| {
            let people = credits.iter().copied().filter(|c| c.role == role);
            (role, people.collect::<Vec<_>>())
        })
        .filter(|(_, people)| !people.is_empty())
        .collect()
}

#[component]
pub fn CreditsCard(
    #[props(default = CREDITS)] credits: &'static [Credit],
    #[props(default = TESTERS)] testers: &'static [Tester],
) -> Element {
    let version = env!("CARGO_PKG_VERSION");
    let open = use_signal(|| false);
    rsx! {
        div { class: "card stack-sm legal",
            FoldHead { title: "Credits", note: "The people who make Dust", open }
            FoldPanel { title: "Credits", open,
                for (role, people) in by_role(credits) {
                    div { class: "credit-group",
                        div { class: "tiny", "{role.heading(people.len())}" }
                        for person in people {
                            div { class: "credit",
                                p { class: "credit-name",
                                    "{person.name}"
                                    if let Some(callsign) = person.callsign {
                                        span { class: "mono credit-call", "{callsign}" }
                                    }
                                }
                                p { class: "muted", "{person.work}" }
                            }
                        }
                    }
                }
                if !testers.is_empty() {
                    div { class: "credit-group",
                        div { class: "tiny", "Closed testing" }
                        div { class: "tester-list",
                            for tester in testers {
                                span { class: "tester",
                                    "{tester.name}"
                                    if let Some(callsign) = tester.callsign {
                                        span { class: "mono credit-call", "{callsign}" }
                                    }
                                }
                            }
                        }
                        p { class: "muted",
                            "Thanks to them and many others for testing Dust and for the useful feedback."
                        }
                    }
                }
                p { class: "muted", "{INSPIRATION}" }
                p { class: "muted",
                    "Dust {version} is free software, under the GNU General Public License version 3 or later, with permission to publish it in app stores for iOS, Android or any other platform as long as it stays free of charge and its source stays available: share it and change it, and what is built from it stays free. "
                    a {
                        class: "credit-link",
                        href: SOURCE_URL,
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "Source code and issues"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Ui;

    /// The card, opened.
    fn render(credits: &'static [Credit], testers: &'static [Tester]) -> String {
        let props = CreditsCardProps { credits, testers };
        let mut ui = Ui::new(CreditsCard, props);
        ui.open_disclosures();
        ui.html()
    }

    #[test]
    fn the_card_starts_folded_and_its_head_opens_and_closes_it() {
        let props = CreditsCardProps {
            credits: CREDITS,
            testers: TESTERS,
        };
        let mut ui = Ui::new(CreditsCard, props);
        assert!(ui.has("The people who make Dust"), "the head always shows");
        assert!(ui.has("aria-expanded=\"false\""));
        for folded in ["SO5KM", "Closed testing", "SP5DNA", SOURCE_URL] {
            assert!(!ui.has(folded), "{folded} shows on a folded card");
        }

        ui.click("disclosure-credits");
        assert!(ui.has("aria-expanded=\"true\""));
        for shown in ["SO5KM", "Closed testing", "SP5DNA", SOURCE_URL] {
            assert!(ui.has(shown), "{shown} is missing from the open card");
        }

        ui.click("disclosure-credits");
        assert!(!ui.has("SO5KM"), "the head folds the card again");
    }

    /// How many callsigns the card shows.
    fn callsigns(html: &str) -> usize {
        html.matches("class=\"mono credit-call\"").count()
    }

    /// The app is SO5KM's, whoever else publishes or works on it.
    #[test]
    fn the_author_stays_credited() {
        assert!(
            CREDITS.iter().any(|c| c.name == "Mateusz K"
                && c.callsign == Some("SO5KM")
                && c.role == Role::Author),
            "{CREDITS:?}"
        );
        assert_eq!(
            by_role(CREDITS)[0].0,
            Role::Author,
            "the author comes first"
        );
    }

    #[test]
    fn everyone_is_listed_under_their_role_and_empty_roles_are_left_off() {
        const TEAM: &[Credit] = &[
            Credit {
                name: "A Maintainer",
                callsign: Some("N0CALL"),
                role: Role::Maintainer,
                work: "Publishes Dust on the App Store.",
            },
            Credit {
                name: "Mateusz K",
                callsign: Some("SO5KM"),
                role: Role::Author,
                work: "Created Dust.",
            },
            Credit {
                name: "No Callsign",
                callsign: None,
                role: Role::Maintainer,
                work: "Publishes Dust on Google Play.",
            },
        ];
        let html = render(TEAM, &[]);
        let author = html.find(">Author<").expect("an author heading");
        let maintainers = html.find(">Maintainers<").expect("a maintainers heading");
        assert!(author < maintainers, "the author comes first: {html}");
        assert!(
            !html.contains("Contributor") && !html.contains("Closed testing"),
            "nobody contributes or tests here: {html}"
        );
        for needle in [
            "SO5KM",
            "A Maintainer",
            "N0CALL",
            "Publishes Dust on the App Store.",
            "No Callsign",
        ] {
            assert!(
                html.contains(needle),
                "the card should show {needle}: {html}"
            );
        }
        assert_eq!(
            callsigns(&html),
            2,
            "a callsign only for those who have one"
        );
    }

    #[test]
    fn the_closed_testing_group_follows_the_team_with_a_thank_you() {
        let html = render(CREDITS, TESTERS);
        let author = html.find(">Author<").expect("an author heading");
        let testing = html.find(">Closed testing<").expect("a testing heading");
        assert!(author < testing, "testers come after the team: {html}");
        let mut at = testing;
        for word in ["Pawel", "SP5DNA", "Mariusz", "SP8KM", "Krzysztof", "SP8BFG"] {
            let found = html[at..]
                .find(&format!(">{word}<"))
                .unwrap_or_else(|| panic!("{word} is missing, or out of order: {html}"));
            at += found;
        }
        let team = CREDITS.iter().filter(|c| c.callsign.is_some()).count();
        assert_eq!(callsigns(&html), team + TESTERS.len());
        assert!(
            html.contains("many others"),
            "they are a few of many: {html}"
        );
    }

    #[test]
    fn a_tester_without_a_callsign_is_credited_by_name() {
        const GROUP: &[Tester] = &[
            Tester {
                name: "With Call",
                callsign: Some("N0CALL"),
            },
            Tester {
                name: "Name Only",
                callsign: None,
            },
        ];
        let html = render(&[], GROUP);
        assert!(html.contains(">Name Only<"), "{html}");
        assert_eq!(
            callsigns(&html),
            1,
            "a callsign only for those who have one"
        );
    }

    /// Other programs get one line between them, not a list: Dust is its
    /// own work, and only Contest mode borrowed ideas.
    #[test]
    fn other_programs_get_one_line() {
        let html = render(CREDITS, TESTERS);
        assert_eq!(html.matches("MorseRunner").count(), 1, "{html}");
        assert!(html.contains(INSPIRATION), "{html}");
        assert!(!html.contains("Built on"), "{html}");
    }

    #[test]
    fn the_card_names_the_version_and_links_the_source() {
        let html = render(CREDITS, TESTERS);
        assert!(html.contains(&format!("Dust {}", env!("CARGO_PKG_VERSION"))));
        assert!(html.contains(SOURCE_URL));
        assert!(html.contains("GNU General Public License version 3"));
        assert!(html.contains("permission to publish it in app stores"));
        assert!(html.contains("stays free of charge and its source stays available"));
        assert!(!html.contains("MIT"), "no other licence: {html}");
    }
}
