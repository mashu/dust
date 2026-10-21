//! Realistic amateur callsigns, drawn the way real ones are built.
//!
//! A callsign is `prefix + separator digit + suffix`: `W1AW`, `DL1ABC`, `9A1AA`.
//! The prefix says which country issued it, and so does what may follow it: a
//! country hands out only some of the ten digits, and suffixes of only some
//! lengths. Spain's `EA` is followed by 1 to 5 and 7, because `EA6`, `EA8` and
//! `EA9` are the Balearics, the Canaries, and Ceuta and Melilla; Russia's lone
//! `R` by any digit, `R1` to `R7` in Europe and `R8`, `R9` and `R0` in Asia;
//! the US `AA` to `AK` blocks by one or two letters, never three.
//!
//! So the table in `callsign/prefixes.rs` is counted from real calls, by
//! `scripts/callsign-prefixes.py`: Super Check Partial's list of the calls
//! active in contests, and Tlf's. It holds every prefix with enough calls behind
//! it, the country cty.dat places those calls in, and how many of them have
//! each separator digit and each suffix length. A call is drawn in those
//! proportions — a prefix as often as real calls carry it, then a digit and a
//! suffix length as often as that prefix's calls have them, then the letters
//! — so every call is one its country could have issued, and a band of them
//! sounds like a real one. Only the counts are kept, never a call, so the
//! crate stays pure and the output endless.
//!
//! Difficulty is the *shape* of the call, not the characters in it: even `W1AW`
//! needs four different characters, so there is no Koch-style ladder to climb
//! here. [`CALLSIGN_TIER_MAX`] tiers add suffixes of every length, then
//! two-letter prefixes, prefixes with a digit in them, portable operation
//! (`G3XYZ/P`), and last a station operating abroad (`DL/W1AW`). Tiers are
//! cumulative, so moving up never takes practice away — a high tier sounds
//! like a real band.
//!
//! A contest's callers are drawn the same way by [`generate_call`], which
//! also says where each one is and can keep to the countries a contest
//! hears ([`CallSpec`]). Reading a call back — its parts, its country and
//! its WPX prefix — is [`parse_callsign`], [`country_of`] and [`wpx_prefix`].

mod prefixes;

use crate::rng::{Rng, SplitMix64};
use prefixes::PREFIXES;

pub const CALLSIGN_TIER_MIN: u32 = 1;
pub const CALLSIGN_TIER_MAX: u32 = 6;

const LETTERS: &[char] = &[
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

/// How often a tier that allows portables actually sends one.
const PORTABLE_SHARE: f64 = 0.25;

/// How often a tier that allows it sends a station operating abroad.
const ABROAD_SHARE: f64 = 0.12;

/// The same two shares in a contest. The tiers send portables and stations
/// abroad often, because they are what is being practised there; a contest's
/// callers are mostly at home, as on a real band.
const CONTEST_PORTABLE_SHARE: f64 = 0.04;
const CONTEST_ABROAD_SHARE: f64 = 0.03;

/// The countries a contest's callers may be in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Countries {
    /// Anywhere: WPX, CQ WW.
    #[default]
    Any,
    /// W/VE as the ARRL DX contest counts it: the USA, without Alaska,
    /// Hawaii or the islands, and Canada.
    WVe,
    /// Everyone else: who W/VE works in the ARRL DX contest.
    NotWVe,
    /// The USA with Alaska, Hawaii and its Caribbean islands, and Canada:
    /// who calls in the North American contests, Sprint and Field Day.
    NorthAmerica,
}

impl Countries {
    /// Whether a station in `country`, by its DXCC primary prefix as the
    /// table names it, may call.
    pub fn admits(self, country: &str) -> bool {
        let w_ve = matches!(country, "K" | "VE");
        match self {
            Self::Any => true,
            Self::WVe => w_ve,
            Self::NotWVe => !w_ve,
            Self::NorthAmerica => w_ve || matches!(country, "KL" | "KH6" | "KP4" | "KP2"),
        }
    }
}

/// Which calls to draw: from which countries, and whether portables and
/// stations abroad come as often as a contest hears them or as often as the
/// tier offers them for practice. The default is the tier's own draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallSpec {
    pub countries: Countries,
    pub contest_shares: bool,
}

impl CallSpec {
    /// A contest's callers, from `countries`.
    pub const fn contest(countries: Countries) -> Self {
        Self {
            countries,
            contest_shares: true,
        }
    }
}

/// A call drawn for a station, and what the station it belongs to is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedCall {
    /// The call as it is sent: `W1AW`, `G3XYZ/P`, `DL/W1AW`.
    pub text: String,
    /// The country the station is in, by its DXCC primary prefix: the host's
    /// for a station operating abroad, whose contacts count for the country
    /// it is in.
    pub country: &'static str,
    /// The call's own separator digit: the `1` of `W1AW`.
    pub digit: char,
    /// The prefix a station operating abroad signs first: the `DL` of
    /// `DL/W1AW`.
    pub host: Option<&'static str>,
    /// What follows the slash of a portable: `P`, `M`, `QRP`, `A`, or a
    /// call-area digit.
    pub marker: Option<&'static str>,
}

impl AsRef<str> for GeneratedCall {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

/// The longest suffix in the table. Longer ones are special-event calls
/// (`YL0YOTA`), which it leaves out.
pub(crate) const SUFFIX_MAX: usize = 3;

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

struct Tier {
    shapes: &'static [PrefixShape],
    suffix_min: usize,
    suffix_max: usize,
    portable: bool,
    /// A station operating abroad, the host country's prefix first.
    abroad: bool,
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
        abroad: false,
    },
    // 2 — the suffix stops being a fixed length.
    Tier {
        shapes: &[Letter],
        suffix_min: 1,
        suffix_max: 3,
        portable: false,
        abroad: false,
    },
    // 3 — two-letter prefixes, so the call can come from anywhere.
    Tier {
        shapes: &[Letter, LetterLetter],
        suffix_min: 1,
        suffix_max: 3,
        portable: false,
        abroad: false,
    },
    // 4 — prefixes with a digit in them, which break the rhythm.
    Tier {
        shapes: &[Letter, LetterLetter, WithDigit],
        suffix_min: 1,
        suffix_max: 3,
        portable: false,
        abroad: false,
    },
    // 5 — portable and mobile operation.
    Tier {
        shapes: &[Letter, LetterLetter, WithDigit],
        suffix_min: 1,
        suffix_max: 3,
        portable: true,
        abroad: false,
    },
    // 6 — stations operating abroad, under the host country's prefix.
    Tier {
        shapes: &[Letter, LetterLetter, WithDigit],
        suffix_min: 1,
        suffix_max: 3,
        portable: true,
        abroad: true,
    },
];

/// One prefix of one country, and how its real calls go on after it.
#[derive(Clone, Copy, Debug)]
struct Prefix {
    prefix: &'static str,
    /// The DXCC entity, by its primary prefix: `K`, `DL`, `UA9`. Countries
    /// can share a prefix — `UA3ABC` is European Russia and `UA9ABC` Asiatic
    /// — so each has an entry of its own.
    country: &'static str,
    /// How many real calls have each separator digit, `0` to `9`. A digit no
    /// call has is one the country does not hand out after this prefix.
    digits: [u32; 10],
    /// How many have a suffix of one, two and three letters.
    lengths: [u32; SUFFIX_MAX],
    /// The country's own operators mark where they are with a call-area
    /// digit — `W1AW/4`, `JA1ABC/3` — rather than only `/P` or `/M`.
    area_marker: bool,
}

/// A prefix of `country`, and how many of its calls have each separator digit
/// and each suffix length.
const fn p(
    prefix: &'static str,
    country: &'static str,
    digits: [u32; 10],
    lengths: [u32; SUFFIX_MAX],
) -> Prefix {
    Prefix {
        prefix,
        country,
        digits,
        lengths,
        area_marker: false,
    }
}

/// The same, for a country whose operators mark a call area after a slash.
const fn area(
    prefix: &'static str,
    country: &'static str,
    digits: [u32; 10],
    lengths: [u32; SUFFIX_MAX],
) -> Prefix {
    Prefix {
        prefix,
        country,
        digits,
        lengths,
        area_marker: true,
    }
}

impl Prefix {
    fn shape(&self) -> PrefixShape {
        if self.prefix.chars().any(|c| c.is_ascii_digit()) {
            WithDigit
        } else if self.prefix.chars().count() == 1 {
            Letter
        } else {
            LetterLetter
        }
    }

    /// How many real calls it has with a suffix `len` letters long.
    fn calls_of_length(&self, len: usize) -> u32 {
        len.checked_sub(1)
            .and_then(|index| self.lengths.get(index))
            .copied()
            .unwrap_or(0)
    }

    /// The suffix lengths `tier` allows, each with how many real calls of
    /// this prefix have it.
    fn lengths_in(&self, tier: &Tier) -> Vec<(usize, f64)> {
        (tier.suffix_min..=tier.suffix_max.min(SUFFIX_MAX))
            .map(|len| (len, f64::from(self.calls_of_length(len))))
            .filter(|(_, calls)| *calls > 0.0)
            .collect()
    }
}

/// Where a station operating abroad most often is, as the prefix it signs
/// first and that country's DXCC entity: `DL/W1AW`, `EA8/G3XYZ`. Mostly CEPT
/// countries, whose licences travel.
const HOSTS: &[(&str, &str, f64)] = &[
    ("DL", "DL", 3.0),
    ("EA", "EA", 2.0),
    ("EA8", "EA8", 1.5),
    ("F", "F", 2.0),
    ("G", "G", 1.2),
    ("GM", "GM", 0.6),
    ("I", "I", 1.5),
    ("OE", "OE", 1.0),
    ("HB9", "HB", 1.0),
    ("PA", "PA", 1.0),
    ("ON", "ON", 0.8),
    ("OH", "OH", 0.8),
    ("SM", "SM", 0.8),
    ("LA", "LA", 0.6),
    ("OZ", "OZ", 0.8),
    ("SP", "SP", 1.0),
    ("OK", "OK", 0.8),
    ("9A", "9A", 1.0),
    ("S5", "S5", 0.5),
    ("SV", "SV", 0.8),
    ("SV9", "SV9", 0.5),
    ("CT", "CT", 0.8),
    ("CT3", "CT3", 0.5),
    ("EI", "EI", 0.6),
    ("TF", "TF", 0.3),
];

/// What follows the slash. `/P` and `/M` are much the most common.
const PORTABLE_MARKERS: &[(&str, f64)] = &[("P", 6.0), ("M", 4.0), ("QRP", 1.5), ("A", 1.0)];

/// The share of portables an area-marking country signs with a call-area
/// digit instead: `W1AW/4`.
const AREA_MARKER_SHARE: f64 = 0.35;

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

/// A prefix of a shape the tier allows, of a country `countries` admits, as
/// often as real calls carry it with a suffix of a length the tier allows.
fn pick_prefix(tier: &Tier, countries: Countries, rng: &mut impl Rng) -> &'static Prefix {
    let usable: Vec<(&'static Prefix, f64)> = PREFIXES
        .iter()
        .filter(|prefix| tier.shapes.contains(&prefix.shape()) && countries.admits(prefix.country))
        .map(|prefix| {
            let calls: f64 = prefix.lengths_in(tier).iter().map(|(_, calls)| calls).sum();
            (prefix, calls)
        })
        .filter(|(_, calls)| *calls > 0.0)
        .collect();
    // Every tier has US calls to draw, so this is only ever a fallback.
    pick_weighted(&usable, rng).copied().unwrap_or(&PREFIXES[0])
}

/// One of the digits real calls have after `prefix`, as often as they have it.
fn separator(prefix: &Prefix, rng: &mut impl Rng) -> char {
    let digits: Vec<(char, f64)> = ('0'..='9')
        .zip(prefix.digits)
        .map(|(digit, calls)| (digit, f64::from(calls)))
        .collect();
    pick_weighted(&digits, rng).copied().unwrap_or('1')
}

/// A suffix length both the tier and the country allow, as often as real
/// calls of the prefix have it.
fn suffix_len(tier: &Tier, prefix: &Prefix, rng: &mut impl Rng) -> usize {
    pick_weighted(&prefix.lengths_in(tier), rng)
        .copied()
        .unwrap_or(tier.suffix_min.max(1))
}

/// One realistic callsign at this tier.
pub fn generate_callsign(tier: u32, rng: &mut impl Rng) -> String {
    generate_call(tier, CallSpec::default(), rng).text
}

/// The digit markers a portable can sign, as text: `W1AW/4`.
const AREA_DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// One realistic call at this tier, from the countries `spec` admits, and
/// what the station is. With the default spec it is drawn exactly as
/// [`generate_callsign`] draws one, draw for draw. Its country is the one
/// [`country_of`] reads off it, so the two never disagree where the table
/// gives one prefix and digit to two countries (`R9`, `VP2`).
pub fn generate_call(tier: u32, spec: CallSpec, rng: &mut impl Rng) -> GeneratedCall {
    let tier = tier_at(tier);
    let prefix = pick_prefix(tier, spec.countries, rng);
    let digit = separator(prefix, rng);
    let mut call = GeneratedCall {
        text: format!("{}{digit}", prefix.prefix),
        country: prefix.country,
        digit,
        host: None,
        marker: None,
    };
    for _ in 0..suffix_len(tier, prefix, rng) {
        call.text.push(LETTERS[rng.usize_in(0, LETTERS.len() - 1)]);
    }
    let (portable_share, abroad_share) = if spec.contest_shares {
        (CONTEST_PORTABLE_SHARE, CONTEST_ABROAD_SHARE)
    } else {
        (PORTABLE_SHARE, ABROAD_SHARE)
    };
    // One of the two at most: abroad, or portable at home.
    if tier.abroad
        && rng.f64() < abroad_share
        && let Some((host, country)) = pick_host(prefix.country, spec.countries, rng)
    {
        call.text = format!("{host}/{}", call.text);
        call.host = Some(host);
        call.country = country;
    } else if tier.portable && rng.f64() < portable_share {
        call.marker = if prefix.area_marker && rng.f64() < AREA_MARKER_SHARE {
            Some(AREA_DIGITS[rng.usize_in(0, 9)])
        } else {
            pick_weighted(PORTABLE_MARKERS, rng).copied()
        };
        if let Some(marker) = call.marker {
            call.text.push('/');
            call.text.push_str(marker);
        }
    }
    call.country = country_of(&call.text).unwrap_or(call.country);
    call
}

/// A country to be operating in that is not `home` and that `countries`
/// admits: the prefix signed first, and its country.
fn pick_host(
    home: &str,
    countries: Countries,
    rng: &mut impl Rng,
) -> Option<(&'static str, &'static str)> {
    let away: Vec<((&'static str, &'static str), f64)> = HOSTS
        .iter()
        .filter(|(_, country, _)| *country != home && countries.admits(country))
        .map(|(prefix, country, weight)| ((*prefix, *country), *weight))
        .collect();
    pick_weighted(&away, rng).copied()
}

/// Every character a callsign at this tier can contain — what the stats and the
/// listen screen practise against.
pub fn callsign_pool(tier: u32) -> Vec<char> {
    let mut pool: Vec<char> = LETTERS.to_vec();
    pool.extend(crate::morse::DIGITS.iter().copied());
    let tier = tier_at(tier);
    if tier.portable || tier.abroad {
        pool.push('/');
    }
    pool
}

/// A few calls to show beside the tier setting. Deterministic, so the preview
/// does not churn every time the screen redraws.
pub fn tier_examples(tier: u32) -> Vec<String> {
    let mut rng = SplitMix64::new(0x51A7_C0DE_u64 ^ u64::from(tier).wrapping_mul(0x9E37_79B9));
    (0..4).map(|_| generate_callsign(tier, &mut rng)).collect()
}

/// A callsign taken apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallsignParts<'a> {
    /// The country a station operating abroad signs first: the `DL` of
    /// `DL/W1AW`.
    pub host: Option<&'a str>,
    pub prefix: &'a str,
    pub separator: char,
    pub suffix: &'a str,
    pub portable: Option<&'a str>,
}

/// Split a callsign into its parts, or `None` if it is not one.
///
/// The separator is the *last* digit before the suffix, which is what makes
/// `9A1AA` split as `9A` + `1` + `AA` rather than at its leading digit. A
/// slash has a marker after the call (`G3XYZ/P`) or a host country before it
/// (`DL/W1AW`); whichever side is a whole call is the call. Some hosts read
/// as a call too (`VP2M`, `VK9X`, `3D2R`), so when both sides do, the shorter
/// is the host or the marker, as MorseRunner's `ExtractPrefix` takes it, and
/// at equal lengths the side with the longer suffix is the call.
pub fn parse_callsign(call: &str) -> Option<CallsignParts<'_>> {
    let Some((left, right)) = call.split_once('/') else {
        return parse_base(call, None, None);
    };
    let marker = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    };
    if !marker(left) || !marker(right) {
        return None;
    }
    match (
        parse_base(left, None, Some(right)),
        parse_base(right, Some(left), None),
    ) {
        (Some(on_left), Some(on_right)) => {
            let left_is_call =
                (left.len(), on_left.suffix.len()) > (right.len(), on_right.suffix.len());
            Some(if left_is_call { on_left } else { on_right })
        }
        (on_left, on_right) => on_left.or(on_right),
    }
}

/// Why `call` is not a callsign, in a few plain words ("needs a digit"), or
/// `None` when [`parse_callsign`] reads it. `call` is written as a callsign
/// is kept: upper case, with slashes only inside it. Of the two sides of a
/// slash, the longer is taken for the call, as the one meant.
pub fn call_fault(call: &str) -> Option<&'static str> {
    if parse_callsign(call).is_some() {
        return None;
    }
    if call.matches('/').count() > 1 {
        return Some("one slash at most");
    }
    let base = match call.split_once('/') {
        Some((left, right)) if right.len() > left.len() => right,
        Some((left, _)) => left,
        None => call,
    };
    let Some(digit) = base.rfind(|c: char| c.is_ascii_digit()) else {
        return Some("needs a digit");
    };
    Some(if digit == 0 {
        "needs a prefix before the digit"
    } else if digit + 1 == base.len() {
        "needs letters after the last digit"
    } else {
        "only letters, digits and a slash"
    })
}

/// One side of a call: `prefix + separator + suffix`, or `None` if it is not
/// a whole call.
fn parse_base<'a>(
    base: &'a str,
    host: Option<&'a str>,
    portable: Option<&'a str>,
) -> Option<CallsignParts<'a>> {
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
        host,
        prefix,
        separator: base[separator..].chars().next()?,
        suffix,
        portable,
    })
}

/// What can follow a call's slash without saying where it is: portable,
/// mobile, low power, an alternative address, at sea or in the air. Contest
/// rules count such a call by its own prefix.
const MODIFIERS: &[&str] = &["P", "M", "QRP", "A", "MM", "AM"];

impl CallsignParts<'_> {
    /// Where the station says it is, when that is not its own call's
    /// country: the host it signs first (`DL/W1AW`), or a country after the
    /// slash (`W1AW/KH6`). A single digit after it is a call area at home,
    /// and a modifier says nothing about where.
    fn place(&self) -> Option<&str> {
        self.host.or(self
            .portable
            .filter(|marker| !MODIFIERS.contains(marker) && self.area_digit().is_none()))
    }

    /// The call-area digit after the slash: the `4` of `W1AW/4`.
    fn area_digit(&self) -> Option<char> {
        let marker = self.portable?;
        let mut chars = marker.chars();
        let digit = chars.next().filter(char::is_ascii_digit)?;
        chars.next().is_none().then_some(digit)
    }
}

/// A host as MorseRunner cuts one: its letters after the last digit go, but
/// two characters stay (`VP2M` is `VP2`, `9A` stays `9A`).
fn host_prefix(place: &str) -> &str {
    let keep = place
        .rfind(|c: char| c.is_ascii_digit())
        .map_or(0, |at| at + 1)
        .max(2)
        .min(place.len());
    &place[..keep]
}

/// A call's WPX prefix, the CQ WPX contest's multiplier, or `None` if it is
/// not a call.
///
/// It is everything up to the last digit (`9A1AA` is `9A1`); a call-area
/// digit after the slash takes the place of that digit (`W1XYZ/4` is `W4`);
/// a station abroad counts by its host, cut as MorseRunner cuts one (`VP2M`
/// is `VP2`), with a `0` added when it has no digit at its end (`PA/N8XYZ`
/// is `PA0`); a country after the slash counts as a host (`W1XYZ/KH6` is
/// `KH6`, `W1XYZ/VE` is `VE0`); and the modifiers `/P`, `/M`, `/QRP`, `/A`,
/// `/MM` and `/AM` change nothing.
pub fn wpx_prefix(call: &str) -> Option<String> {
    let parts = parse_callsign(call)?;
    if let Some(place) = parts.place() {
        let mut prefix = host_prefix(place).to_string();
        if !prefix.ends_with(|c: char| c.is_ascii_digit()) {
            prefix.push('0');
        }
        return Some(prefix);
    }
    let digit = parts.area_digit().unwrap_or(parts.separator);
    Some(format!("{}{digit}", parts.prefix))
}

/// The country a call is in, by its DXCC primary prefix as the table names
/// it (`K`, `DL`, `UA9`), or `None` if it is not a call or the table cannot
/// place it: its prefix is not in the table, or is one that countries share
/// by a digit none of them has (`VP8`). A station abroad is in its host's
/// country.
pub fn country_of(call: &str) -> Option<&'static str> {
    let parts = parse_callsign(call)?;
    match parts.place() {
        Some(place) => country_by_prefix(host_prefix(place)),
        None => country_by_prefix(&format!("{}{}", parts.prefix, parts.separator)),
    }
}

/// The country of `text`, a call's prefix and digit (`R9`, `KH6`) or a host
/// (`DL`, `HB9`, `S5`), as the table places it: an entry whose prefix and a
/// digit its calls have spell the whole of it, or whose prefix alone does.
/// Where countries share it, the one with the most calls that way wins: `R9`
/// is Asiatic Russia, `EA8` the Canaries. A digit the table has never seen
/// after a prefix still places it when one country has that prefix (`KG3` is
/// the USA), and nothing places a prefix the table lacks: `C31`, `NL7`,
/// `VP8` are `None` rather than a country that merely shares a letter.
fn country_by_prefix(text: &str) -> Option<&'static str> {
    // The digit that follows `entry`'s prefix and ends `text`.
    let digit_after = |entry: &Prefix| {
        let mut rest = text.strip_prefix(entry.prefix)?.chars();
        let digit = rest.next()?.to_digit(10)?;
        rest.next().is_none().then_some(digit as usize)
    };
    PREFIXES
        .iter()
        .filter_map(|entry| {
            let calls = if entry.prefix == text {
                entry.digits.iter().sum()
            } else {
                entry.digits[digit_after(entry)?]
            };
            (calls > 0).then_some((calls, entry.country))
        })
        .max_by_key(|(calls, _)| *calls)
        .map(|(_, country)| country)
        .or_else(|| {
            let mut countries = PREFIXES
                .iter()
                .filter(|entry| digit_after(entry).is_some())
                .map(|entry| entry.country);
            let first = countries.next()?;
            countries.all(|country| country == first).then_some(first)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::morse::morse_for;

    fn calls(tier: u32, count: usize) -> Vec<String> {
        let mut rng = SplitMix64::new(u64::from(tier).wrapping_mul(0x2545_F491) | 1);
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
                }
                if !spec.abroad {
                    assert!(parts.host.is_none(), "tier {tier}: {call:?} is abroad");
                }
                if !spec.portable && !spec.abroad {
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
            assert!(upper.abroad || !lower.abroad);
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
    fn the_top_tiers_go_portable_and_abroad_sometimes_but_not_always() {
        let portable = calls(5, 500);
        let marked = portable.iter().filter(|call| call.contains('/')).count();
        assert!(marked > 0, "no portable calls at tier 5");
        assert!(marked < portable.len(), "every call was portable");
        let top = calls(CALLSIGN_TIER_MAX, 1_000);
        let abroad = top
            .iter()
            .filter(|call| parse_callsign(call).is_some_and(|parts| parts.host.is_some()))
            .count();
        assert!(abroad > 0, "nobody operating abroad at the top tier");
        assert!(abroad < top.len() / 4, "{abroad} of {} abroad", top.len());
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
            let spec = tier_at(tier);
            assert!(pool.contains(&'/') == (spec.portable || spec.abroad));
        }
    }

    #[test]
    fn a_tier_outside_the_ladder_is_pulled_back_onto_it() {
        let mut rng = SplitMix64::new(7);
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
        assert_eq!(portable.host, None);
        let abroad = parse_callsign("EA8/G3XYZ").expect("a callsign");
        assert_eq!(abroad.host, Some("EA8"));
        assert_eq!(abroad.prefix, "G");
        assert_eq!(abroad.separator, '3');
        assert_eq!(abroad.suffix, "XYZ");
        assert_eq!(abroad.portable, None);
        assert!(parse_callsign("DL/W1AW").is_some_and(|parts| parts.host == Some("DL")));
    }

    /// A call that does not read says why, and one that does says nothing,
    /// whatever was typed and kept.
    #[test]
    fn what_is_not_a_callsign_says_why() {
        for (call, fault) in [
            ("ABC", "needs a digit"),
            ("", "needs a digit"),
            ("1AW", "needs a prefix before the digit"),
            ("W1", "needs letters after the last digit"),
            ("DL1AB2", "needs letters after the last digit"),
            ("W1A/P/M", "one slash at most"),
            ("P/W1", "needs letters after the last digit"),
            ("DL/ABCD", "needs a digit"),
        ] {
            assert_eq!(call_fault(call), Some(fault), "{call:?}");
        }
        let mut rng = SplitMix64::new(11);
        for tier in 1..=CALLSIGN_TIER_MAX {
            for _ in 0..500 {
                let call = generate_callsign(tier, &mut rng);
                assert_eq!(call_fault(&call), None, "{call}");
            }
        }
        for typed in [
            "K",
            "KK",
            "9A",
            "Q/1",
            "A/B",
            "W1AW/P",
            "DL/W1AW",
            "ABCDEFGHIJ1",
        ] {
            assert_eq!(
                call_fault(typed).is_none(),
                parse_callsign(typed).is_some(),
                "{typed:?}"
            );
        }
    }

    /// The countries whose real calls include `prefix`, then `digit`, then a
    /// suffix `len` letters long.
    fn issuing(prefix: &str, digit: char, len: usize) -> Vec<&'static Prefix> {
        let Some(digit) = digit.to_digit(10) else {
            return Vec::new();
        };
        PREFIXES
            .iter()
            .filter(|entry| {
                entry.prefix == prefix
                    && entry.digits[digit as usize] > 0
                    && entry.calls_of_length(len) > 0
            })
            .collect()
    }

    /// Every call is one its country could have issued: real calls have its
    /// prefix, then its digit, then a suffix of its length; a station abroad
    /// is not abroad at home; and only a country that marks call areas signs
    /// one after the slash.
    #[test]
    fn every_call_is_one_its_country_could_issue() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            for call in calls(tier, 2_000) {
                let parts = parse_callsign(&call).expect("a callsign");
                let len = parts.suffix.chars().count();
                let countries = issuing(parts.prefix, parts.separator, len);
                assert!(
                    !countries.is_empty(),
                    "{call:?}: no country issues {}{} with a {len}-letter suffix",
                    parts.prefix,
                    parts.separator
                );
                if let Some(host) = parts.host {
                    let (_, host_country, _) = HOSTS
                        .iter()
                        .find(|(prefix, _, _)| *prefix == host)
                        .unwrap_or_else(|| panic!("{call:?}: {host} is not a host"));
                    assert!(
                        countries
                            .iter()
                            .any(|country| country.country != *host_country),
                        "{call:?} is abroad at home"
                    );
                }
                if let Some(marker) = parts.portable
                    && marker.chars().all(|c| c.is_ascii_digit())
                {
                    assert!(
                        countries.iter().any(|country| country.area_marker),
                        "{call:?}: {} marks no call area",
                        parts.prefix
                    );
                }
            }
        }
    }

    /// The table says what the regulators say, because real calls do.
    #[test]
    fn the_table_knows_what_countries_issue() {
        // Russia's lone R takes every digit, R1 to R7 in Europe and R8, R9
        // and R0 in Asia, and two or three letters after it.
        for digit in '0'..='9' {
            assert!(!issuing("R", digit, 2).is_empty(), "R{digit}");
            assert!(!issuing("R", digit, 3).is_empty(), "R{digit}");
        }
        for (digit, country) in [('3', "UA"), ('9', "UA9"), ('0', "UA9")] {
            assert!(
                issuing("R", digit, 2)
                    .iter()
                    .any(|entry| entry.country == country),
                "R{digit} is {country}"
            );
        }
        // There is no lone U: UA, UR and US are prefixes, U is not.
        assert!(PREFIXES.iter().all(|entry| entry.prefix != "U"));
        // EA6, EA8 and EA9 are the Balearics, the Canaries, and Ceuta and
        // Melilla, not Spain.
        for digit in ['6', '8', '9'] {
            let countries = issuing("EA", digit, 2);
            assert!(!countries.is_empty(), "EA{digit}");
            assert!(
                countries.iter().all(|entry| entry.country != "EA"),
                "EA{digit} is not Spain"
            );
        }
        // The US AA to AK blocks are 2x1 and 2x2 calls: AA1AA, never AA1AAA.
        for block in ["AA", "AB", "AC", "AD", "AE", "AF", "AG", "AI", "AJ", "AK"] {
            for digit in '0'..='9' {
                assert!(issuing(block, digit, 3).is_empty(), "{block}{digit}");
            }
        }
        // Hawaii is KH6, and Alaska KL7.
        for (prefix, digit, country) in [("KH", '6', "KH6"), ("KL", '7', "KL")] {
            let countries = issuing(prefix, digit, 2);
            assert!(
                !countries.is_empty() && countries.iter().all(|entry| entry.country == country),
                "{prefix}{digit} is {country}"
            );
        }
    }

    #[test]
    fn the_prefix_table_is_well_formed() {
        assert!(!PREFIXES.is_empty());
        for entry in PREFIXES {
            let prefix = entry.prefix;
            let letters = prefix.chars().filter(char::is_ascii_uppercase).count();
            let digits = prefix.chars().filter(char::is_ascii_digit).count();
            let expected = match entry.shape() {
                Letter => (1, 0),
                LetterLetter => (2, 0),
                WithDigit => (1, 1),
            };
            assert_eq!(
                (letters, digits),
                expected,
                "{prefix} is not a {:?}",
                entry.shape()
            );
            for ch in prefix.chars() {
                assert!(morse_for(ch).is_some(), "{prefix} cannot be sent");
            }
            assert!(entry.digits.iter().any(|&calls| calls > 0), "{prefix}");
            assert!(entry.lengths.iter().any(|&calls| calls > 0), "{prefix}");
            assert!(!entry.country.is_empty(), "{prefix}");
        }
        // A prefix listed twice for one country would quietly double its
        // share of the band.
        let mut seen: Vec<(&str, &str)> = PREFIXES
            .iter()
            .map(|entry| (entry.prefix, entry.country))
            .collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(before, seen.len(), "a prefix is in the table twice");
        // Hosts name their country as the table does, or a station could be
        // abroad at home.
        for (host, country, weight) in HOSTS {
            assert!(*weight > 0.0, "{host}");
            assert!(host.chars().all(|c| morse_for(c).is_some()), "{host}");
            assert!(
                PREFIXES.iter().any(|entry| entry.country == *country),
                "{host}: no prefix of {country}"
            );
        }
        // Every tier has something to draw.
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            let spec = tier_at(tier);
            assert!(
                PREFIXES
                    .iter()
                    .any(|entry| spec.shapes.contains(&entry.shape())
                        && !entry.lengths_in(spec).is_empty()),
                "tier {tier} has no prefix"
            );
        }
    }

    /// Real calls are mostly American, and so is the band: a prefix comes up
    /// as often as real calls carry it.
    #[test]
    fn prefixes_come_up_as_often_as_real_calls_carry_them() {
        let top = calls(CALLSIGN_TIER_MAX, 4_000);
        let share = |country: &str| {
            let hits = top
                .iter()
                .filter_map(|call| parse_callsign(call))
                .filter(|parts| {
                    let len = parts.suffix.chars().count();
                    issuing(parts.prefix, parts.separator, len)
                        .iter()
                        .any(|entry| entry.country == country)
                })
                .count();
            hits as f64 / top.len() as f64
        };
        let us = share("K");
        assert!((0.2..0.5).contains(&us), "{us:.2} of the band is American");
        assert!(share("DL") > share("9A"), "more Germans than Croats");
        assert!(share("9A") > 0.0, "some Croats");
    }

    #[test]
    fn a_weighted_table_always_yields_something() {
        let mut rng = SplitMix64::new(3);
        assert_eq!(pick_weighted(&[("only", 0.0)], &mut rng), Some(&"only"));
        assert_eq!(pick_weighted::<&str>(&[], &mut rng), None);
        // Weight decides: a thousand draws can only land on the heavy entry.
        let table = [("rare", 0.0), ("common", 1.0)];
        for _ in 0..1_000 {
            assert_eq!(pick_weighted(&table, &mut rng), Some(&"common"));
        }
    }

    /// The calls Callsigns mode sends must never change under it: a contest's
    /// draw shares the code, and only a spec of its own may draw differently.
    #[test]
    fn generate_callsign_draws_what_it_always_drew() {
        let golden: [&[&str]; 6] = [
            &[
                "N2CD", "N7RF", "R0LL", "N3AR", "N8YY", "R2NW", "W5TF", "F4FF", "W4DX", "W6TP",
                "K2EO", "W1MV",
            ],
            &[
                "M7UHW", "F6HHK", "W3BN", "M6FXI", "W3ULJ", "N6VBJ", "G0AWY", "K5QTZ", "N6ARB",
                "W1BWR", "F1RLX", "K1FBO",
            ],
            &[
                "W4LK", "GW7ZMK", "RU4YT", "W4ZDB", "OM1OW", "OZ1UL", "W4DG", "WB2NBO", "F8WP",
                "OM4LEK", "HB9FG", "EA6JD",
            ],
            &[
                "KD8ENR", "JP3LTE", "JJ1ALW", "UB8N", "K0RS", "EG1WPO", "W7BMC", "OE1JPD", "W0JIW",
                "M0IEG", "DU9LX", "R3WP",
            ],
            &[
                "YD8YDG", "AA2SO", "VK4ZB", "GW3KRZ/P", "W0HAF", "TA0XJ", "KC1WHQ/8", "IK5LCF",
                "WB2OBU", "2M0OKA", "W9FYP", "DL1YE/P",
            ],
            &[
                "RD3HG",
                "DL/PU5KSF",
                "HZ1BI/M",
                "SP/W6PJO",
                "UA0XDQ",
                "LZ7AX",
                "K2AZ",
                "W3QFS",
                "R3RY",
                "EA5QZ",
                "N0YC",
                "DL5RWL",
            ],
        ];
        for (tier, expected) in (CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX).zip(golden) {
            let mut rng = SplitMix64::new(0x00C0_FFEE ^ u64::from(tier));
            let drawn: Vec<String> = (0..expected.len())
                .map(|_| generate_callsign(tier, &mut rng))
                .collect();
            assert_eq!(drawn, expected, "tier {tier}");
        }
    }

    fn generated(tier: u32, spec: CallSpec, count: usize) -> Vec<GeneratedCall> {
        let mut rng = SplitMix64::new(u64::from(tier).wrapping_mul(0x9E37_79B9) ^ 0xCA11);
        (0..count)
            .map(|_| generate_call(tier, spec, &mut rng))
            .collect()
    }

    /// Every call a contest can send reads back as a call, with the parts
    /// the generator says it has.
    #[test]
    fn every_generated_call_parses_back_into_its_parts() {
        for spec in [CallSpec::default(), CallSpec::contest(Countries::Any)] {
            for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
                for call in generated(tier, spec, 10_000) {
                    let parts = parse_callsign(&call.text)
                        .unwrap_or_else(|| panic!("tier {tier}: {:?} does not parse", call.text));
                    assert_eq!(parts.separator, call.digit, "{call:?}");
                    assert_eq!(parts.host, call.host, "{call:?}");
                    assert_eq!(parts.portable, call.marker, "{call:?}");
                    assert_eq!(country_of(&call.text), Some(call.country), "{call:?}");
                }
            }
        }
    }

    /// A spec keeps to its countries, by what the generator says and by what
    /// the call itself reads as.
    #[test]
    fn each_spec_gives_only_its_countries() {
        let all = [
            Countries::Any,
            Countries::WVe,
            Countries::NotWVe,
            Countries::NorthAmerica,
        ];
        for countries in all {
            for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
                let calls = generated(tier, CallSpec::contest(countries), 2_000);
                for call in &calls {
                    assert!(countries.admits(call.country), "{countries:?}: {call:?}");
                    let read = country_of(&call.text).expect("a country");
                    assert!(
                        countries.admits(read),
                        "{countries:?}: {call:?} reads as {read}"
                    );
                }
            }
        }
        // The specs mean what they say about the edges of W/VE.
        assert!(Countries::WVe.admits("K") && Countries::WVe.admits("VE"));
        assert!(!Countries::WVe.admits("KH6") && !Countries::WVe.admits("KL"));
        assert!(Countries::NotWVe.admits("KH6") && !Countries::NotWVe.admits("VE"));
        assert!(Countries::NorthAmerica.admits("KL") && Countries::NorthAmerica.admits("KP4"));
        assert!(!Countries::NorthAmerica.admits("XE") && !Countries::NorthAmerica.admits("DL"));
        // Alaska, Hawaii and the islands do call in a North American contest.
        let na = generated(
            CALLSIGN_TIER_MAX,
            CallSpec::contest(Countries::NorthAmerica),
            4_000,
        );
        assert!(na.iter().any(|call| call.country == "KL"));
        assert!(na.iter().any(|call| call.country == "KH6"));
        assert!(na.iter().any(|call| call.country == "VE"));
    }

    /// A contest's callers are mostly at home: portables and stations abroad
    /// come at the contest shares, not the tier's.
    #[test]
    fn a_contest_hears_few_portables_and_few_stations_abroad() {
        let share = |calls: &[GeneratedCall], of: fn(&GeneratedCall) -> bool| {
            calls.iter().filter(|call| of(call)).count() as f64 / calls.len() as f64
        };
        let contest = generated(CALLSIGN_TIER_MAX, CallSpec::contest(Countries::Any), 20_000);
        let abroad = share(&contest, |call| call.host.is_some());
        let portable = share(&contest, |call| call.marker.is_some());
        assert!((0.02..0.04).contains(&abroad), "{abroad:.3} abroad");
        assert!((0.03..0.05).contains(&portable), "{portable:.3} portable");
        let practice = generated(CALLSIGN_TIER_MAX, CallSpec::default(), 20_000);
        assert!(share(&practice, |call| call.host.is_some()) > 0.1);
        assert!(share(&practice, |call| call.marker.is_some()) > 0.15);
        // Below the tiers that send them, a contest sends neither.
        let tier4 = generated(4, CallSpec::contest(Countries::Any), 2_000);
        assert!(tier4.iter().all(|call| !call.text.contains('/')));
    }

    #[test]
    fn a_wpx_prefix_is_read_as_the_contest_reads_it() {
        let cases = [
            // Up to the last digit.
            ("K5XYZ", "K5"),
            ("DL1ABC", "DL1"),
            ("9A1AA", "9A1"),
            ("2E0ABC", "2E0"),
            ("VP2EAB", "VP2"),
            // A call-area digit takes the place of the call's own.
            ("W1XYZ/4", "W4"),
            ("JA1ABC/3", "JA3"),
            // Portable, mobile and low power change nothing.
            ("W1XYZ/P", "W1"),
            ("W1XYZ/M", "W1"),
            ("DL1ABC/QRP", "DL1"),
            ("G3XYZ/A", "G3"),
            ("G3XYZ/MM", "G3"),
            // A host is the prefix, with a 0 when it ends without a digit.
            ("PA/N8XYZ", "PA0"),
            ("F/G3XYZ", "F0"),
            ("9A/DL1ABC", "9A0"),
            ("EA8/G3XYZ", "EA8"),
            ("HB9/DL1ABC", "HB9"),
            ("S5/W1AW", "S5"),
            // And so is a country after the call.
            ("W1XYZ/KH6", "KH6"),
            ("W1XYZ/VE", "VE0"),
            // A host that reads as a call is still the host: the shorter side.
            ("VP2M/G3XYZ", "VP2"),
            ("G3XYZ/VP2M", "VP2"),
            ("VK9X/DL1ABC", "VK9"),
            ("3D2R/W1AW", "3D2"),
        ];
        for (call, prefix) in cases {
            assert_eq!(wpx_prefix(call).as_deref(), Some(prefix), "{call}");
        }
        for not_a_call in ["", "K5", "K5?", "W1A/P/M", "k5xyz"] {
            assert_eq!(wpx_prefix(not_a_call), None, "{not_a_call:?}");
        }
    }

    #[test]
    fn a_country_is_the_longest_match_and_a_host_comes_first() {
        let cases = [
            ("K1ABC", "K"),
            ("W1AW", "K"),
            ("AA1AA", "K"),
            ("DL1ABC", "DL"),
            ("VE3ABC", "VE"),
            ("VY2AB", "VE"),
            ("KH6XYZ", "KH6"),
            ("AH6XY", "KH6"),
            ("KL7XYZ", "KL"),
            ("KP4ABC", "KP4"),
            ("9A1AA", "9A"),
            ("S51AB", "S5"),
            // The digit decides where a prefix is shared.
            ("EA1ABC", "EA"),
            ("EA8ABC", "EA8"),
            ("R3ABC", "UA"),
            ("R9ABC", "UA9"),
            ("UA9ABC", "UA9"),
            ("OH2ABC", "OH"),
            ("OH0ABC", "OH0"),
            // Abroad is where the host is; portable is still at home.
            ("DL/W1AW", "DL"),
            ("PA/N8XYZ", "PA"),
            ("EA8/G3XYZ", "EA8"),
            ("HB9/DL1ABC", "HB"),
            ("W1AW/4", "K"),
            ("G3XYZ/P", "G"),
            ("W1XYZ/KH6", "KH6"),
            // A host that reads as a call is still the host.
            ("VP2M/G3XYZ", "VP2M"),
            ("G3XYZ/VP2M", "VP2M"),
            // A digit the table has not seen, after a prefix one country has.
            ("KG3ABC", "K"),
        ];
        for (call, country) in cases {
            assert_eq!(country_of(call), Some(country), "{call}");
        }
        // A prefix the table lacks is no country, not one that shares its
        // first letter: Andorra, Sable Island, St Paul, the Falklands, the
        // Cook Islands, Guadeloupe, the Isle of Man and Alaska's NL and WL.
        for unknown in [
            "3B8ABC", "", "W1", "K5?", "C31AB", "CY0AA", "CY9AA", "VP8AB", "E51AB", "FG5AB",
            "GD4AB", "MD0AB", "NL7AB", "WL7AB",
        ] {
            assert_eq!(country_of(unknown), None, "{unknown:?}");
        }
    }
}
