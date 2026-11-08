//! Operators' given names, as they send them: `BOB`, not `ROBERT`.
//!
//! North American callers' names are about 150 short given names, weighted
//! to people born 1935–1975, who are most of the operators a contest hears.
//! The design asks for them to be counted from the SSA's baby-name data
//! (public domain), but the network refused the download when this was
//! written, so the list is written by hand for Dust: common given names of
//! those years, each folded to the form an operator sends (ROBERT and BOBBY
//! are BOB; RICHARD is RICH, RICK or DICK), its weight Dust's rough estimate,
//! in thousands, of the US births of those years that go by it. A script
//! that counts the SSA's files can replace it without changing anything that
//! reads it.
//!
//! Callers elsewhere have short lists written for Dust, a few common names
//! for each region, drawn evenly, and chosen by the caller's country. A name
//! belongs to a caller, never to a call: the same call can come back with
//! another name.

use crate::callsign::pick_weighted;
use crate::rng::Rng;

/// North American given names, as operators send them, each with Dust's
/// estimate of the US births of 1935–1975, in thousands, that go by it.
#[rustfmt::skip]
const NORTH_AMERICA: &[(&str, f64)] = &[
    // Men.
    ("BOB", 3200.0), ("JIM", 3200.0), ("JOHN", 3000.0), ("MIKE", 2600.0),
    ("DAVE", 2400.0), ("BILL", 2200.0), ("TOM", 1700.0), ("CHUCK", 1300.0),
    ("STEVE", 1300.0), ("JOE", 1250.0), ("DAN", 1100.0), ("DON", 1050.0),
    ("RON", 950.0), ("MARK", 950.0), ("KEN", 900.0), ("JERRY", 900.0),
    ("LARRY", 850.0), ("PAUL", 850.0), ("RICK", 800.0), ("GEORGE", 800.0),
    ("GARY", 800.0), ("CHRIS", 800.0), ("ED", 750.0), ("RICH", 700.0),
    ("TIM", 700.0), ("JEFF", 700.0), ("TONY", 700.0), ("AL", 650.0),
    ("FRANK", 600.0), ("RAY", 550.0), ("DENNIS", 500.0), ("GREG", 500.0),
    ("DICK", 450.0), ("DOUG", 450.0), ("BRIAN", 450.0), ("ROGER", 400.0),
    ("TERRY", 400.0), ("KEVIN", 400.0), ("WALT", 350.0), ("PETE", 350.0),
    ("FRED", 350.0), ("SCOTT", 350.0), ("MATT", 350.0), ("HAL", 300.0),
    ("CARL", 300.0), ("JACK", 300.0), ("BRUCE", 300.0), ("RANDY", 300.0),
    ("ERIC", 300.0), ("ART", 250.0), ("HANK", 250.0), ("WAYNE", 250.0),
    ("PHIL", 250.0), ("KEITH", 250.0), ("ROY", 200.0), ("RALPH", 200.0),
    ("GENE", 200.0), ("RUSS", 200.0), ("GLENN", 200.0), ("CRAIG", 200.0),
    ("ANDY", 200.0), ("HOWARD", 150.0), ("LOU", 150.0), ("BARRY", 150.0),
    ("DALE", 150.0), ("MARTY", 150.0), ("ROD", 150.0), ("TODD", 150.0),
    ("VIC", 120.0), ("LEN", 120.0), ("NORM", 120.0), ("STAN", 120.0),
    ("SAM", 120.0), ("CURT", 120.0), ("HARRY", 120.0), ("NICK", 120.0),
    ("TED", 120.0), ("BERNIE", 100.0), ("ERNIE", 100.0), ("EARL", 100.0),
    ("VINCE", 100.0), ("BEN", 80.0), ("LEROY", 80.0), ("LEE", 80.0),
    ("HERB", 80.0), ("MARVIN", 80.0), ("NEIL", 80.0), ("DEAN", 70.0),
    ("GORDON", 60.0), ("LLOYD", 60.0), ("JAY", 60.0), ("CLIFF", 60.0),
    // Women. PAT is Patricia's as well as Patrick's, and CHRIS Christine's
    // as well as Christopher's: one name each.
    ("PAT", 1600.0), ("MARY", 2300.0), ("LINDA", 1300.0), ("DEB", 1100.0),
    ("BARB", 1000.0), ("SUE", 1000.0), ("KAREN", 900.0), ("CAROL", 750.0),
    ("NANCY", 700.0), ("SANDY", 700.0), ("KATHY", 700.0), ("DONNA", 650.0),
    ("SHARON", 600.0), ("JUDY", 600.0), ("PAM", 600.0), ("CINDY", 600.0),
    ("BETH", 600.0), ("LISA", 600.0), ("BRENDA", 500.0), ("JENNY", 500.0),
    ("KIM", 500.0), ("BETTY", 450.0), ("PEGGY", 450.0), ("DIANE", 450.0),
    ("LAURA", 450.0), ("JANET", 400.0), ("JULIE", 400.0), ("CHERYL", 350.0),
    ("LORI", 350.0), ("TERRI", 350.0), ("ANGIE", 350.0), ("JOYCE", 300.0),
    ("DOT", 300.0), ("AMY", 300.0), ("DENISE", 300.0), ("JOAN", 250.0),
    ("MARTHA", 250.0), ("TAMMY", 250.0), ("VICKI", 250.0), ("BEV", 250.0),
    ("ROBIN", 250.0), ("SHELLY", 250.0), ("TINA", 250.0), ("ANNE", 200.0),
    ("JEAN", 200.0), ("GLORIA", 200.0), ("CONNIE", 200.0), ("RUTH", 200.0),
    ("WENDY", 200.0), ("JACKIE", 200.0), ("PAULA", 200.0), ("RHONDA", 200.0),
    ("DAWN", 200.0), ("JANE", 150.0), ("GAIL", 150.0), ("ALICE", 150.0),
    ("ELLEN", 150.0), ("LYNN", 150.0), ("RITA", 150.0), ("FRAN", 150.0),
];

/// The countries whose callers go by [`NORTH_AMERICA`]'s names: the USA,
/// Canada, and the US states and islands that sign their own prefixes.
const NORTH_AMERICAN: &[&str] = &["K", "VE", "KH6", "KL", "KP2"];

/// A few common names for the callers of some countries, written for Dust.
struct Region {
    countries: &'static [&'static str],
    names: &'static [&'static str],
}

/// Callers outside North America, by region. A country in none of them,
/// the English-speaking ones among them, goes by [`ELSEWHERE`].
#[rustfmt::skip]
const REGIONS: &[Region] = &[
    Region {
        countries: &["DL", "OE", "HB", "LX"],
        names: &["HANS", "KLAUS", "JURGEN", "UWE", "DIRK", "JORG", "UDO", "FRANK", "PETER",
            "WOLF", "HORST", "HEINZ", "RALF", "BERND", "GERD", "DIETER", "KARL", "ULI", "WERNER"],
    },
    Region {
        countries: &["PA", "ON", "PJ4", "P4", "PZ"],
        names: &["JAN", "PIET", "HENK", "KEES", "WIM", "JOOP", "LUC", "MARC", "GERT", "BART",
            "RUUD", "JAAP", "ERIK", "JOS"],
    },
    Region {
        countries: &["F", "FM", "TK"],
        names: &["JEAN", "PIERRE", "MICHEL", "ALAIN", "LUC", "ERIC", "DENIS", "GILLES", "RENE",
            "YVES", "MARC", "SERGE", "BRUNO", "PASCAL", "GUY"],
    },
    Region {
        countries: &["I", "IS"],
        names: &["MARIO", "LUIGI", "PAOLO", "GIANNI", "FRANCO", "BRUNO", "SERGIO", "CARLO",
            "PIERO", "ENZO", "MAURO", "FABIO", "MARCO", "LUCA", "RENZO", "GINO"],
    },
    Region {
        countries: &["EA", "EA6", "EA8", "EA9", "LU", "CE", "CX", "XE", "HK", "YV", "OA", "HC",
            "TI", "TG", "HP", "CM", "HI", "KP4", "ZP"],
        names: &["JOSE", "JUAN", "LUIS", "CARLOS", "PACO", "PEPE", "JORGE", "PEDRO", "RAUL",
            "MIGUEL", "JAVIER", "ANGEL", "MANOLO", "RAMON"],
    },
    Region {
        countries: &["CT", "CT3", "CU", "PY", "D4"],
        names: &["JOAO", "PAULO", "RUI", "NUNO", "JOSE", "LUIS", "CARLOS", "MARIO", "PEDRO",
            "CHICO", "FABIO", "MARCOS"],
    },
    Region {
        countries: &["UA", "UA2", "UA9", "UR", "EU", "UN", "EX", "UK"],
        names: &["YURI", "IGOR", "OLEG", "SERGEI", "ALEX", "VLAD", "IVAN", "BORIS", "VICTOR",
            "ANDREI", "DIMA", "SASHA", "VOVA", "MISHA", "PAVEL", "VALERY"],
    },
    Region {
        countries: &["SP"],
        names: &["JUREK", "MAREK", "ADAM", "TOMEK", "PIOTR", "JACEK", "KRZYS", "WOJTEK",
            "BOGDAN", "JANUSZ", "LECH"],
    },
    Region {
        countries: &["OK", "OM"],
        names: &["JIRI", "PETR", "PAVEL", "JAN", "KAREL", "MILAN", "VACLAV", "JOSEF", "LADA",
            "TOMAS", "JOZEF", "MIRO"],
    },
    Region {
        countries: &["9A", "S5", "YU", "E7", "Z3", "Z6", "4O", "LZ"],
        names: &["ZORAN", "MILAN", "DRAGAN", "GORAN", "IVO", "BRANKO", "MARKO", "ZLATKO",
            "BOJAN", "IVAN", "PETAR", "DIMO"],
    },
    Region {
        countries: &["HA"],
        names: &["LACI", "JANOS", "ISTVAN", "ZOLI", "PISTA", "GABOR", "TIBOR", "FERI", "ATTILA",
            "SANYI"],
    },
    Region {
        countries: &["YO", "ER"],
        names: &["ION", "DAN", "MIHAI", "VIOREL", "DORU", "SORIN", "MARIUS", "NICU", "GELU"],
    },
    Region {
        countries: &["SV", "SV9", "5B"],
        names: &["NIKOS", "YANNIS", "KOSTAS", "TAKIS", "MAKIS", "PANOS", "SAKIS", "SPYROS",
            "GEORGE"],
    },
    Region {
        countries: &["SM", "LA", "OH", "OH0", "OZ", "TF"],
        names: &["LARS", "OLE", "NILS", "ERIK", "PER", "BENGT", "JAN", "KARL", "SVEN", "BJORN",
            "MATS", "LEIF", "PEKKA", "JUHA", "TIMO", "JARI", "HANNU", "KJELL", "JENS"],
    },
    Region {
        countries: &["LY", "YL", "ES"],
        names: &["ALGIS", "JONAS", "PETRAS", "JANIS", "ANDRIS", "TOOMAS", "ARVO", "JAAN"],
    },
    Region {
        countries: &["JA"],
        names: &["KENJI", "HIRO", "KAZU", "SHIN", "YOSHI", "TORU", "AKIRA", "MASA", "KOJI",
            "JUN", "ISAO", "HIDEO", "TETSU", "NOBU", "TAKA"],
    },
    Region {
        countries: &["BY", "BV", "VR", "HL"],
        names: &["MING", "WEI", "HUA", "YONG", "BIN", "TAO", "LEI", "MIN", "JIN", "SUNG"],
    },
    Region {
        countries: &["VU", "S2"],
        names: &["RAJ", "RAVI", "ANIL", "SUNIL", "VIJAY", "RAMESH", "RAJU", "MOHAN", "SANJAY",
            "ARUN"],
    },
    Region {
        countries: &["YB", "9M2", "9M6", "9V", "DU", "HS"],
        names: &["AGUS", "BUDI", "EKO", "DEDI", "HARI", "JOJO", "RAMON", "NOEL", "BONG",
            "AHMAD", "AZMAN", "SOMSAK"],
    },
    Region {
        countries: &["A4", "A6", "A7", "9K", "HZ", "OD", "CN"],
        names: &["ALI", "AHMED", "OMAR", "SAMI", "HASSAN", "KHALID", "FAHAD", "NASSER", "SALEH",
            "YOUSEF"],
    },
    Region {
        countries: &["TA"],
        names: &["MEHMET", "ALI", "AHMET", "CEM", "MURAT", "KEMAL", "ERDAL", "OZAN", "BARIS",
            "HAKAN"],
    },
    Region {
        countries: &["4X"],
        names: &["AVI", "YOSSI", "MOSHE", "ERAN", "DANNY", "ZVI", "BENNY", "ILAN", "AMIR",
            "GADI"],
    },
];

/// The names of callers outside North America that no region lists: in
/// Britain and Ireland, Australia and New Zealand, South Africa and the
/// Caribbean, and anywhere else.
#[rustfmt::skip]
const ELSEWHERE: &[&str] = &["IAN", "NIGEL", "COLIN", "ALAN", "GRAHAM", "NEIL", "DEREK",
    "TREVOR", "KEITH", "BRIAN", "ROY", "DAVE", "JOHN", "PETE", "MIKE", "STEVE", "PHIL", "CHRIS",
    "GEOFF", "NICK", "BRUCE", "SEAN", "PADDY"];

/// A name for an operator in `country`, by its key in the call table (`K`,
/// `DL`), drawn from `rng`: a North American one as often as people go by
/// it, any other evenly from its region's list.
pub(crate) fn draw_name(country: &str, rng: &mut impl Rng) -> &'static str {
    if NORTH_AMERICAN.contains(&country) {
        return pick_weighted(NORTH_AMERICA, rng).copied().unwrap_or("BOB");
    }
    let names = REGIONS
        .iter()
        .find(|region| region.countries.contains(&country))
        .map_or(ELSEWHERE, |region| region.names);
    names[rng.usize_in(0, names.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::Field;
    use crate::rng::SplitMix64;

    /// Every list, North America's first.
    fn lists() -> Vec<Vec<&'static str>> {
        let mut lists = vec![NORTH_AMERICA.iter().map(|(name, _)| *name).collect()];
        lists.extend(REGIONS.iter().map(|region| region.names.to_vec()));
        lists.push(ELSEWHERE.to_vec());
        lists
    }

    /// Each name is one an operator sends and a NAME field reads: two to
    /// six letters, A to Z, never a report, which the field beside it would
    /// take (`ANN` reads as 199). No list holds one twice.
    #[test]
    fn every_name_is_short_plain_and_once_in_its_list() {
        for list in lists() {
            let mut seen = std::collections::BTreeSet::new();
            for name in &list {
                assert!((2..=6).contains(&name.len()), "{name}");
                assert!(name.chars().all(|c| c.is_ascii_uppercase()), "{name}");
                assert_eq!(Field::Name.canonical(name).as_deref(), Some(*name));
                assert_eq!(Field::Rst.canonical(name), None, "{name} reads as a report");
                assert!(seen.insert(*name), "{name} twice");
            }
        }
        assert!((140..=160).contains(&NORTH_AMERICA.len()));
        assert!(NORTH_AMERICA.iter().all(|(_, weight)| *weight > 0.0));
    }

    /// North American names are the forms operators send, never the names
    /// they were given.
    #[test]
    fn names_are_folded_to_what_operators_send() {
        let names: Vec<&str> = NORTH_AMERICA.iter().map(|(name, _)| *name).collect();
        for (given, sent) in [
            ("ROBERT", "BOB"),
            ("WILLIAM", "BILL"),
            ("JAMES", "JIM"),
            ("MICHAEL", "MIKE"),
            ("DAVID", "DAVE"),
            ("RICHARD", "RICH"),
            ("THOMAS", "TOM"),
            ("CHARLES", "CHUCK"),
            ("JOSEPH", "JOE"),
            ("STEVEN", "STEVE"),
            ("PATRICIA", "PAT"),
            ("BARBARA", "BARB"),
            ("DEBORAH", "DEB"),
            ("BOBBY", "BOB"),
            ("JIMMY", "JIM"),
        ] {
            assert!(!names.contains(&given), "{given} is sent as {sent}");
            assert!(names.contains(&sent), "{sent}");
        }
    }

    /// A caller's name comes by its country: North American ones as often as
    /// people go by them, the others from their region's list, and a
    /// country no region lists from the English-speaking one.
    #[test]
    fn a_name_comes_from_the_callers_country() {
        let mut rng = SplitMix64::new(3);
        let draws: Vec<&str> = (0..20_000).map(|_| draw_name("K", &mut rng)).collect();
        let count = |name: &str| draws.iter().filter(|drawn| **drawn == name).count();
        assert!(count("BOB") > 5 * count("CLIFF"), "weighted by births");
        assert!(
            draws
                .iter()
                .all(|name| NORTH_AMERICA.iter().any(|(na, _)| na == name))
        );
        for (country, list) in [("DL", 0), ("JA", 15), ("UA9", 6), ("KP4", 4)] {
            for _ in 0..50 {
                let name = draw_name(country, &mut rng);
                assert!(REGIONS[list].names.contains(&name), "{country} {name}");
            }
        }
        for country in ["G", "JT", ""] {
            assert!(
                ELSEWHERE.contains(&draw_name(country, &mut rng)),
                "{country:?}"
            );
        }
        // One list for each country.
        let mut placed: Vec<&str> = NORTH_AMERICAN.to_vec();
        for region in REGIONS {
            placed.extend(region.countries);
        }
        let all = placed.len();
        placed.sort_unstable();
        placed.dedup();
        assert_eq!(placed.len(), all, "a country in two lists");
    }
}
