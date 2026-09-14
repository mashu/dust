//! Realistic amateur callsigns, built from their structure rather than read from
//! a list.
//!
//! A callsign is `prefix + separator digit + suffix`, optionally followed by a
//! portable marker: `W1AW`, `DL1ABC`, `9A1AA`, `G3XYZ/P`. The prefix says which
//! country issued it, so the tables below are real ITU allocations weighted by
//! how much of the band they actually are — US-heavy, then Europe and Japan,
//! then everywhere else. Generating from structure rather than a callsign
//! database keeps this crate pure and the output endless.
//!
//! Difficulty is the *shape* of the call, not the characters in it: even `W1AW`
//! needs four different characters, so there is no Koch-style ladder to climb
//! here. [`CALLSIGN_TIER_MAX`] tiers add longer suffixes, then two-letter and
//! digit-bearing prefixes, then portable suffixes. Tiers are cumulative, so
//! moving up never takes practice away — a high tier sounds like a real band.

use crate::rng::{FastrandRng, Rng};

pub const CALLSIGN_TIER_MIN: u32 = 1;
pub const CALLSIGN_TIER_MAX: u32 = 6;

const LETTERS: &[char] = &[
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

/// How often a tier that allows portables actually sends one.
const PORTABLE_SHARE: f64 = 0.25;

/// What a callsign's prefix looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrefixShape {
    /// One letter: `W1AW`, `G3XYZ`.
    Letter,
    /// Two letters: `DL1ABC`, `VE3AB`.
    LetterLetter,
    /// A digit in the prefix, either way round: `9A1AA`, `S51AB`.
    WithDigit,
}

/// Shapes are not equally common on the air, so a tier that allows the exotic
/// ones still sends mostly ordinary calls.
fn shape_weight(shape: PrefixShape) -> f64 {
    match shape {
        PrefixShape::Letter => 5.0,
        PrefixShape::LetterLetter => 4.0,
        PrefixShape::WithDigit => 1.0,
    }
}

struct Tier {
    shapes: &'static [PrefixShape],
    suffix_min: usize,
    suffix_max: usize,
    portable: bool,
}

use PrefixShape::{Letter, LetterLetter, WithDigit};

/// Each tier is the one before it plus something new.
const TIERS: &[Tier] = &[
    // 1 — the shape everyone knows: one letter, a digit, two letters.
    Tier {
        shapes: &[Letter],
        suffix_min: 2,
        suffix_max: 2,
        portable: false,
    },
    // 2 — the suffix stops being a fixed length.
    Tier {
        shapes: &[Letter],
        suffix_min: 1,
        suffix_max: 3,
        portable: false,
    },
    // 3 — two-letter prefixes, so the call can come from anywhere.
    Tier {
        shapes: &[Letter, LetterLetter],
        suffix_min: 1,
        suffix_max: 3,
        portable: false,
    },
    // 4 — the long suffixes.
    Tier {
        shapes: &[Letter, LetterLetter],
        suffix_min: 1,
        suffix_max: 4,
        portable: false,
    },
    // 5 — prefixes with a digit in them, which break the rhythm.
    Tier {
        shapes: &[Letter, LetterLetter, WithDigit],
        suffix_min: 1,
        suffix_max: 4,
        portable: false,
    },
    // 6 — portable and mobile operation.
    Tier {
        shapes: &[Letter, LetterLetter, WithDigit],
        suffix_min: 1,
        suffix_max: 4,
        portable: true,
    },
];

/// Single-letter prefixes actually in use, weighted by how much of the band
/// they are.
const SINGLE_LETTER: &[(&str, f64)] = &[
    ("W", 9.0),
    ("K", 9.0),
    ("N", 7.0), // USA
    ("G", 4.0),
    ("M", 2.0), // UK
    ("F", 2.5), // France
    ("I", 2.5), // Italy
    ("R", 2.5),
    ("U", 1.0), // Russia
];

/// The US two-letter blocks, which are most of the two-letter calls you hear.
/// `A` only runs to `AL`; the rest take the whole alphabet.
const US_TWO_LETTER: &[(char, &str)] = &[
    ('A', "ABCDEFGHIJKL"),
    ('K', "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
    ('N', "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
    ('W', "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
];

/// How much of the two-letter traffic is American.
const US_TWO_LETTER_SHARE: f64 = 0.45;

/// Two-letter prefixes from outside the US.
const DX_TWO_LETTER: &[(&str, f64)] = &[
    // Germany
    ("DL", 9.0),
    ("DK", 4.0),
    ("DJ", 3.0),
    ("DF", 2.0),
    ("DG", 1.5),
    // Japan
    ("JA", 8.0),
    ("JE", 2.5),
    ("JH", 3.0),
    ("JR", 2.5),
    ("JF", 1.5),
    // Rest of Europe
    ("EA", 4.0),
    ("EB", 1.0),
    ("IK", 3.0),
    ("IZ", 2.0),
    ("IW", 1.5),
    ("PA", 3.5),
    ("PD", 1.0),
    ("ON", 2.5),
    ("OK", 3.0),
    ("OM", 2.0),
    ("SP", 3.5),
    ("SQ", 1.5),
    ("OH", 3.0),
    ("SM", 3.0),
    ("SA", 1.0),
    ("LA", 2.5),
    ("OZ", 2.0),
    ("HB", 2.0),
    ("OE", 2.5),
    ("YO", 2.0),
    ("LZ", 1.5),
    ("SV", 2.0),
    ("TA", 1.5),
    ("YU", 1.5),
    ("YT", 1.0),
    ("CT", 2.0),
    ("EI", 1.5),
    ("HA", 2.0),
    ("HG", 1.0),
    ("TF", 0.6),
    ("LY", 1.5),
    ("YL", 1.5),
    ("ES", 1.2),
    ("EW", 1.0),
    // UK regions
    ("GM", 1.5),
    ("GW", 1.2),
    ("GI", 0.8),
    ("MM", 0.6),
    // Former USSR
    ("UA", 4.0),
    ("RA", 3.5),
    ("RN", 2.0),
    ("RK", 1.5),
    ("UR", 2.5),
    ("UT", 2.0),
    ("UX", 1.2),
    // Canada
    ("VE", 4.5),
    ("VA", 2.0),
    ("VO", 0.8),
    // Rest of the world
    ("VK", 3.0),
    ("ZL", 2.0),
    ("ZS", 1.2),
    ("PY", 3.0),
    ("PP", 1.0),
    ("PU", 1.5),
    ("LU", 2.5),
    ("CE", 1.5),
    ("CX", 0.8),
    ("HK", 1.0),
    ("XE", 1.5),
    ("BA", 1.5),
    ("BY", 1.0),
    ("BG", 1.5),
    ("HL", 1.5),
    ("DS", 1.0),
    ("VU", 1.5),
];

/// Prefixes carrying a digit, both ways round: `9A` and `S5` alike.
const DIGIT_PREFIX: &[(&str, f64)] = &[
    ("9A", 3.0), // Croatia
    ("S5", 3.0), // Slovenia
    ("4X", 2.0),
    ("4Z", 1.5), // Israel
    ("Z3", 1.5), // North Macedonia
    ("E7", 1.5), // Bosnia and Herzegovina
    ("T7", 0.8), // San Marino
    ("9H", 1.2), // Malta
    ("3A", 0.6), // Monaco
    ("4O", 0.8), // Montenegro
    ("4L", 1.0), // Georgia
    ("5B", 1.0), // Cyprus
    ("9K", 0.8), // Kuwait
    ("9V", 0.8), // Singapore
    ("9M", 1.0), // Malaysia
    ("E2", 0.8), // Thailand
    ("3B", 0.5), // Mauritius
    ("8P", 0.5), // Barbados
    ("7X", 0.5), // Algeria
    ("6Y", 0.6), // Jamaica
    ("9Y", 0.6), // Trinidad and Tobago
];

/// Suffix lengths are not uniform either: two and three letters dominate.
const SUFFIX_WEIGHTS: &[(usize, f64)] = &[(1, 1.0), (2, 5.0), (3, 5.0), (4, 1.0)];

/// What follows the slash. `/P` and `/M` are much the most common.
const PORTABLE_MARKERS: &[(&str, f64)] = &[
    ("P", 6.0),
    ("M", 4.0),
    ("QRP", 1.5),
    ("A", 1.0),
    ("0", 0.4),
    ("1", 0.4),
    ("2", 0.4),
    ("3", 0.4),
    ("4", 0.4),
    ("5", 0.4),
    ("6", 0.4),
    ("7", 0.4),
    ("8", 0.4),
    ("9", 0.4),
];

fn tier_at(tier: u32) -> &'static Tier {
    let index = tier.clamp(CALLSIGN_TIER_MIN, CALLSIGN_TIER_MAX) - CALLSIGN_TIER_MIN;
    &TIERS[index as usize]
}

/// Draw from a weighted table. Weights are the point of these tables, so a
/// table that somehow sums to nothing still yields its first entry rather than
/// nothing at all.
fn pick_weighted<'t, T>(table: &'t [(T, f64)], rng: &mut impl Rng) -> Option<&'t T> {
    let total: f64 = table.iter().map(|(_, weight)| weight.max(0.0)).sum();
    if total <= 0.0 {
        return table.first().map(|(item, _)| item);
    }
    let mut target = rng.f64() * total;
    for (item, weight) in table {
        target -= weight.max(0.0);
        if target <= 0.0 {
            return Some(item);
        }
    }
    table.last().map(|(item, _)| item)
}

fn pick_shape(tier: &Tier, rng: &mut impl Rng) -> PrefixShape {
    let table: Vec<(PrefixShape, f64)> = tier
        .shapes
        .iter()
        .map(|shape| (*shape, shape_weight(*shape)))
        .collect();
    pick_weighted(&table, rng).copied().unwrap_or(Letter)
}

fn us_two_letter(rng: &mut impl Rng) -> Option<String> {
    let (first, seconds) = US_TWO_LETTER.get(rng.usize_in(0, US_TWO_LETTER.len() - 1))?;
    let choices: Vec<char> = seconds.chars().collect();
    let second = choices.get(rng.usize_in(0, choices.len().saturating_sub(1)))?;
    Some(format!("{first}{second}"))
}

fn prefix_for(shape: PrefixShape, rng: &mut impl Rng) -> String {
    match shape {
        Letter => pick_weighted(SINGLE_LETTER, rng)
            .map(|prefix| (*prefix).to_string())
            .unwrap_or_else(|| "W".to_string()),
        LetterLetter => {
            if rng.f64() < US_TWO_LETTER_SHARE {
                if let Some(prefix) = us_two_letter(rng) {
                    return prefix;
                }
            }
            pick_weighted(DX_TWO_LETTER, rng)
                .map(|prefix| (*prefix).to_string())
                .unwrap_or_else(|| "DL".to_string())
        }
        WithDigit => pick_weighted(DIGIT_PREFIX, rng)
            .map(|prefix| (*prefix).to_string())
            .unwrap_or_else(|| "9A".to_string()),
    }
}

fn suffix_len(tier: &Tier, rng: &mut impl Rng) -> usize {
    let allowed: Vec<(usize, f64)> = SUFFIX_WEIGHTS
        .iter()
        .copied()
        .filter(|(len, _)| *len >= tier.suffix_min && *len <= tier.suffix_max)
        .collect();
    pick_weighted(&allowed, rng)
        .copied()
        .unwrap_or(tier.suffix_min.max(1))
}

/// One realistic callsign at this tier.
pub fn generate_callsign(tier: u32, rng: &mut impl Rng) -> String {
    let tier = tier_at(tier);
    let shape = pick_shape(tier, rng);
    let mut call = prefix_for(shape, rng);
    call.push(char::from_digit(rng.usize_in(0, 9) as u32, 10).unwrap_or('1'));
    for _ in 0..suffix_len(tier, rng) {
        call.push(LETTERS[rng.usize_in(0, LETTERS.len() - 1)]);
    }
    if tier.portable && rng.f64() < PORTABLE_SHARE {
        if let Some(marker) = pick_weighted(PORTABLE_MARKERS, rng) {
            call.push('/');
            call.push_str(marker);
        }
    }
    call
}

/// Every character a callsign at this tier can contain — what the stats and the
/// listen screen practise against.
pub fn callsign_pool(tier: u32) -> Vec<char> {
    let mut pool: Vec<char> = LETTERS.to_vec();
    pool.extend(crate::morse::DIGITS.iter().copied());
    if tier_at(tier).portable {
        pool.push('/');
    }
    pool
}

/// A few calls to show beside the tier setting. Deterministic, so the preview
/// does not churn every time the screen redraws.
pub fn tier_examples(tier: u32) -> Vec<String> {
    let mut rng = FastrandRng(0x51A7_C0DE_u64 ^ u64::from(tier).wrapping_mul(0x9E37_79B9));
    (0..4).map(|_| generate_callsign(tier, &mut rng)).collect()
}

/// A callsign taken apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallsignParts<'a> {
    pub prefix: &'a str,
    pub separator: char,
    pub suffix: &'a str,
    pub portable: Option<&'a str>,
}

/// Split a callsign into its parts, or `None` if it is not one.
///
/// The separator is the *last* digit before the suffix, which is what makes
/// `9A1AA` split as `9A` + `1` + `AA` rather than at its leading digit.
pub fn parse_callsign(call: &str) -> Option<CallsignParts<'_>> {
    let (base, portable) = match call.split_once('/') {
        Some((base, portable)) => (base, Some(portable)),
        None => (call, None),
    };
    if let Some(marker) = portable {
        if marker.is_empty()
            || !marker
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return None;
        }
    }
    let separator = base.rfind(|c: char| c.is_ascii_digit())?;
    let prefix = &base[..separator];
    let suffix = &base[separator + 1..];
    if prefix.is_empty() || suffix.is_empty() {
        return None;
    }
    if !prefix
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return None;
    }
    if !suffix.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    Some(CallsignParts {
        prefix,
        separator: base[separator..].chars().next()?,
        suffix,
        portable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::morse::morse_for;

    fn calls(tier: u32, count: usize) -> Vec<String> {
        let mut rng = FastrandRng(u64::from(tier).wrapping_mul(0x2545_F491) | 1);
        (0..count)
            .map(|_| generate_callsign(tier, &mut rng))
            .collect()
    }

    #[test]
    fn every_tier_sends_something_a_radio_could_send() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            for call in calls(tier, 500) {
                let parts = parse_callsign(&call)
                    .unwrap_or_else(|| panic!("tier {tier}: {call:?} is not a callsign"));
                assert!(!parts.prefix.is_empty());
                assert!(parts.separator.is_ascii_digit());
                assert!(!parts.suffix.is_empty());
                // Every character has to be sendable, or the group is silent.
                for ch in call.chars() {
                    assert!(
                        morse_for(ch).is_some(),
                        "tier {tier}: {call:?} contains {ch:?}, which has no Morse"
                    );
                }
            }
        }
    }

    #[test]
    fn a_tier_only_sends_what_it_is_allowed_to() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            let spec = tier_at(tier);
            for call in calls(tier, 500) {
                let parts = parse_callsign(&call).expect("a callsign");
                let len = parts.suffix.chars().count();
                assert!(
                    len >= spec.suffix_min && len <= spec.suffix_max,
                    "tier {tier}: {call:?} has a {len}-character suffix"
                );
                if !spec.portable {
                    assert!(
                        parts.portable.is_none(),
                        "tier {tier}: {call:?} is portable"
                    );
                    assert!(!call.contains('/'), "tier {tier}: {call:?} has a slash");
                }
                let has_digit_prefix = parts.prefix.chars().any(|c| c.is_ascii_digit());
                if !spec.shapes.contains(&WithDigit) {
                    assert!(
                        !has_digit_prefix,
                        "tier {tier}: {call:?} has a digit in its prefix"
                    );
                }
                if !spec.shapes.contains(&LetterLetter) && !has_digit_prefix {
                    assert_eq!(
                        parts.prefix.chars().count(),
                        1,
                        "tier {tier}: {call:?} has a long prefix"
                    );
                }
            }
        }
    }

    #[test]
    fn the_tiers_keep_what_the_ones_below_them_had() {
        // Each tier must still be able to produce the shapes below it, or moving
        // up would take practice away rather than add to it.
        for tier in (CALLSIGN_TIER_MIN + 1)..=CALLSIGN_TIER_MAX {
            let lower = tier_at(tier - 1);
            let upper = tier_at(tier);
            assert!(
                lower
                    .shapes
                    .iter()
                    .all(|shape| upper.shapes.contains(shape)),
                "tier {tier} dropped a prefix shape"
            );
            assert!(upper.suffix_min <= lower.suffix_min);
            assert!(upper.suffix_max >= lower.suffix_max);
            assert!(upper.portable || !lower.portable);
        }
    }

    #[test]
    fn the_first_tier_is_the_shape_everybody_knows() {
        for call in calls(1, 200) {
            assert_eq!(call.chars().count(), 4, "{call:?} is not a 1x2 call");
            let parts = parse_callsign(&call).expect("a callsign");
            assert_eq!(parts.prefix.chars().count(), 1);
            assert_eq!(parts.suffix.chars().count(), 2);
        }
    }

    #[test]
    fn the_top_tier_goes_portable_sometimes_but_not_always() {
        let calls = calls(CALLSIGN_TIER_MAX, 500);
        let portable = calls.iter().filter(|call| call.contains('/')).count();
        assert!(portable > 0, "no portable calls at the top tier");
        assert!(portable < calls.len(), "every call was portable");
    }

    #[test]
    fn the_pool_covers_everything_a_tier_can_send() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            let pool = callsign_pool(tier);
            for call in calls(tier, 500) {
                for ch in call.chars() {
                    assert!(
                        pool.contains(&ch),
                        "tier {tier}: {ch:?} is outside the pool"
                    );
                }
            }
            assert!(pool.contains(&'/') == tier_at(tier).portable);
        }
    }

    #[test]
    fn a_tier_outside_the_ladder_is_pulled_back_onto_it() {
        let mut rng = FastrandRng(7);
        let low = generate_callsign(0, &mut rng);
        assert_eq!(low.chars().count(), 4, "tier 0 should behave as tier 1");
        let high = generate_callsign(9_999, &mut rng);
        assert!(parse_callsign(&high).is_some());
        assert_eq!(callsign_pool(0), callsign_pool(CALLSIGN_TIER_MIN));
        assert_eq!(callsign_pool(9_999), callsign_pool(CALLSIGN_TIER_MAX));
    }

    #[test]
    fn examples_are_stable_and_belong_to_their_tier() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            let first = tier_examples(tier);
            assert_eq!(first, tier_examples(tier), "the preview must not churn");
            assert_eq!(first.len(), 4);
            for call in first {
                assert!(parse_callsign(&call).is_some(), "{call:?}");
            }
        }
    }

    #[test]
    fn what_is_not_a_callsign_is_refused() {
        assert!(parse_callsign("").is_none());
        assert!(parse_callsign("W1").is_none(), "no suffix");
        assert!(parse_callsign("1AW").is_none(), "no prefix");
        assert!(parse_callsign("WAW").is_none(), "no separator");
        assert!(parse_callsign("W1A/").is_none(), "an empty portable marker");
        assert!(parse_callsign("W1A/P/M").is_none(), "two slashes");
        assert!(parse_callsign("w1aw").is_none(), "lower case");
        assert!(parse_callsign("W1A?").is_none(), "punctuation");
        let parts = parse_callsign("9A1AA").expect("a callsign");
        assert_eq!(parts.prefix, "9A");
        assert_eq!(parts.separator, '1');
        assert_eq!(parts.suffix, "AA");
        assert_eq!(parts.portable, None);
        let portable = parse_callsign("W1AW/4").expect("a callsign");
        assert_eq!(portable.prefix, "W");
        assert_eq!(portable.suffix, "AW");
        assert_eq!(portable.portable, Some("4"));
    }

    #[test]
    fn the_prefix_tables_are_real_prefixes() {
        let tables: [&[(&str, f64)]; 3] = [SINGLE_LETTER, DX_TWO_LETTER, DIGIT_PREFIX];
        for table in tables {
            assert!(!table.is_empty());
            for (prefix, weight) in table {
                assert!(*weight > 0.0, "{prefix} has no weight");
                assert!(!prefix.is_empty());
                assert!(
                    prefix
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
                    "{prefix} is not a prefix"
                );
                for ch in prefix.chars() {
                    assert!(morse_for(ch).is_some(), "{prefix} cannot be sent");
                }
            }
        }
        // A prefix listed twice would quietly double its share of the band.
        let mut seen: Vec<&str> = SINGLE_LETTER
            .iter()
            .chain(DX_TWO_LETTER)
            .chain(DIGIT_PREFIX)
            .map(|(prefix, _)| *prefix)
            .collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(before, seen.len(), "a prefix is in the tables twice");
    }

    #[test]
    fn a_weighted_table_always_yields_something() {
        let mut rng = FastrandRng(3);
        assert_eq!(pick_weighted(&[("only", 0.0)], &mut rng), Some(&"only"));
        assert_eq!(pick_weighted::<&str>(&[], &mut rng), None);
        // Weight decides: a thousand draws can only land on the heavy entry.
        let table = [("rare", 0.0), ("common", 1.0)];
        for _ in 0..1_000 {
            assert_eq!(pick_weighted(&table, &mut rng), Some(&"common"));
        }
    }
}
