//! An exchange's fields: what each one holds, the one way each value is
//! written for comparing (its canonical form), and the cut numbers operators
//! send in place of digits.
//!
//! Operators shorten digits to letters with fewer elements: `T` for 0, `N`
//! for 9, `A` for 1, `E` for 5, so a serial of 7 goes out as `TT7` and a
//! report of 599 as `5NN`. A cut letter is read only in a field that holds a
//! number: elsewhere `TN` is Tennessee and `AL` a name.

use super::CallerHabits;
use super::data::places::place_code;
use super::data::sections::{fold_yukon, listed_section};
use crate::alignment::align_group;
use crate::rng::Rng;

/// A field of an exchange, as it is copied and logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Field {
    /// The signal report: `599`, sent `5NN`.
    Rst,
    /// A serial number.
    Nr,
    /// The operator's name.
    Name,
    /// A US state, a Canadian province or territory, or `DX`.
    St,
    /// A CQ zone.
    Zn,
    /// Transmitter power.
    Pwr,
    /// A Field Day class: how many transmitters, and a letter.
    Cl,
    /// An ARRL or RAC section.
    Sec,
}

/// The longest token read as a number: nine digits always fit a `u32`, and
/// no exchange has a number that long.
const NUMBER_LEN_MAX: usize = 9;

/// The highest CQ zone.
pub(crate) const ZONE_MAX: u32 = 40;

/// The most power an amateur station runs, in watts.
pub(crate) const POWER_MAX: u32 = 1_500;

/// Power that is logged as `KW`.
const KILOWATT: u32 = 1_000;

/// How a kilowatt is written, each read as `KW` and sent as written.
pub(crate) const KILOWATT_SPELLINGS: [&str; 3] = ["K", "KW", "1K"];

impl Field {
    pub const ALL: [Self; 8] = [
        Self::Rst,
        Self::Nr,
        Self::Name,
        Self::St,
        Self::Zn,
        Self::Pwr,
        Self::Cl,
        Self::Sec,
    ];

    /// The name a run's log keeps it under: `nr`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rst => "rst",
            Self::Nr => "nr",
            Self::Name => "name",
            Self::St => "st",
            Self::Zn => "zn",
            Self::Pwr => "pwr",
            Self::Cl => "cl",
            Self::Sec => "sec",
        }
    }

    /// The field a log names `name`, if this build knows it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.name() == name)
    }

    /// Its column heading, as loggers write it: `NR`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rst => "RST",
            Self::Nr => "NR",
            Self::Name => "NAME",
            Self::St => "ST",
            Self::Zn => "ZN",
            Self::Pwr => "PWR",
            Self::Cl => "CL",
            Self::Sec => "SEC",
        }
    }

    /// What it is in a sentence: "copy their number".
    pub fn noun(self) -> &'static str {
        match self {
            Self::Rst => "report",
            Self::Nr => "number",
            Self::Name => "name",
            Self::St => "state",
            Self::Zn => "zone",
            Self::Pwr => "power",
            Self::Cl => "class",
            Self::Sec => "section",
        }
    }

    /// A value a station might send for it, for showing what an exchange
    /// is like: `214`.
    pub fn example(self) -> &'static str {
        match self {
            Self::Rst => "579",
            Self::Nr => "214",
            Self::Name => "BOB",
            Self::St => "OR",
            Self::Zn => "25",
            Self::Pwr => "KW",
            Self::Cl => "3A",
            Self::Sec => "OR",
        }
    }

    /// Whether it holds a number, the only place a cut letter is read.
    pub fn numeric(self) -> bool {
        matches!(self, Self::Rst | Self::Nr | Self::Zn | Self::Pwr)
    }

    /// `token` written the one way this field compares it, or `None` if it
    /// cannot be this field: a serial with no leading zeros (`TT1` is `1`),
    /// a report as three digits (`5NN` is `599`), power as watts or `KW`, a
    /// class without leading zeros (`03A` is `3A`), Yukon's section as the
    /// Territories' (`YT` is `TER`). Tokens are upper case.
    pub fn canonical(self, token: &str) -> Option<String> {
        match self {
            Self::Nr => read_number(token).map(|n| n.to_string()),
            Self::Zn => read_number(token)
                .filter(|zone| (1..=ZONE_MAX).contains(zone))
                .map(|zone| zone.to_string()),
            Self::Rst => report(token),
            Self::Pwr => power(token),
            Self::Cl => class(token),
            Self::Name => (!token.is_empty() && token.chars().all(|c| c.is_ascii_uppercase()))
                .then(|| token.to_string()),
            // A state, province or territory by its code, an old code as the
            // new (PQ is QC, NF is NL), Labrador (LB), or DX. A contest that
            // counts Labrador as part of NL, as the Sprint does, reads LB as
            // NL (`ContestRules::canonical`).
            Self::St => place_code(token).map(str::to_string),
            // An ARRL or RAC section by its code, or DX. Yukon's own code is
            // the Territories', which ARRL contests count it in.
            Self::Sec => listed_section(token).map(|code| fold_yukon(code).to_string()),
        }
    }

    /// Whether two tokens hold the same value of this field: `7` and `TT7`
    /// for a serial, `K` and `1K` for power.
    pub fn same(self, a: &str, b: &str) -> bool {
        match (self.canonical(a), self.canonical(b)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

/// The digit a cut letter stands for.
fn cut_digit(letter: char) -> Option<u32> {
    match letter {
        'T' | 'O' => Some(0),
        'A' => Some(1),
        'E' => Some(5),
        'N' => Some(9),
        _ => None,
    }
}

/// The letter `digit` is cut to: `heard` when that is one of its letters,
/// so a typed 0 under a sent `O` stays `O`; otherwise the usual one.
fn cut_letter(digit: u32, heard: char) -> Option<char> {
    if cut_digit(heard) == Some(digit) {
        return Some(heard);
    }
    usual_cut(digit)
}

/// The letter `digit` is usually cut to: `T` for 0, `A` for 1, `E` for 5
/// and `N` for 9.
fn usual_cut(digit: u32) -> Option<char> {
    match digit {
        0 => Some('T'),
        1 => Some('A'),
        5 => Some('E'),
        9 => Some('N'),
        _ => None,
    }
}

/// `digits` with each of the digits in `cut` as its usual letter
/// ([`usual_cut`]): `cut("190", &['0', '9'])` is `1NT`.
fn cut(digits: &str, cut: &[char]) -> String {
    digits
        .chars()
        .map(|c| match c.to_digit(10) {
            Some(digit) if cut.contains(&c) => usual_cut(digit).unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// A token read as a number, digits and cut letters alike: `1N5` is 195,
/// `TT7` is 7. `None` if anything else is in it.
pub fn read_number(token: &str) -> Option<u32> {
    if token.is_empty() || token.len() > NUMBER_LEN_MAX {
        return None;
    }
    token.chars().try_fold(0, |number, c| {
        let digit = c.to_digit(10).or_else(|| cut_digit(c))?;
        Some(number * 10 + digit)
    })
}

/// A report as its three digits: readability 1 to 5, strength and tone 1
/// to 9.
fn report(token: &str) -> Option<String> {
    let digits: Vec<u32> = token
        .chars()
        .map(|c| c.to_digit(10).or_else(|| cut_digit(c)))
        .collect::<Option<_>>()?;
    match digits[..] {
        [r @ 1..=5, s @ 1..=9, t @ 1..=9] => Some(format!("{r}{s}{t}")),
        _ => None,
    }
}

/// Power in watts, or `KW` for a kilowatt however it is written: `K`, `KW`,
/// `1K` or `1000`.
fn power(token: &str) -> Option<String> {
    if KILOWATT_SPELLINGS.contains(&token) {
        return Some("KW".to_string());
    }
    match read_number(token)? {
        KILOWATT => Some("KW".to_string()),
        watts @ 1..=POWER_MAX => Some(watts.to_string()),
        _ => None,
    }
}

/// A Field Day class: 1 to 99 transmitters and a letter from A to F.
fn class(token: &str) -> Option<String> {
    let letter = token.chars().last().filter(|c| ('A'..='F').contains(c))?;
    let count = &token[..token.len() - 1];
    if count.is_empty() || !count.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let count: u32 = count.trim_start_matches('0').parse().ok()?;
    (1..=99)
        .contains(&count)
        .then(|| format!("{count}{letter}"))
}

/// The digits a serial is cut by, and a zone or a power: zeros and nines,
/// and a zone's ones too, as MorseRunner cuts them.
const SERIAL_CUTS: [char; 2] = ['0', '9'];
const ZONE_CUTS: [char; 3] = ['0', '1', '9'];

/// Your serial as you send it, as MorseRunner sends yours: padded to three
/// digits, its zeros cut to `T` and nines to `N` (`TT7`, `1N5`).
pub fn cut_serial(serial: u32) -> String {
    cut(&format!("{serial:03}"), &SERIAL_CUTS)
}

/// Your CQ zone as you send it, cut as MorseRunner sends yours: zeros as
/// `T`, ones as `A` and nines as `N`, and no padding (`A4` for 14, `5` for
/// 5).
pub fn cut_zone(zone: u32) -> String {
    cut(&zone.to_string(), &ZONE_CUTS)
}

/// Your power as you send it, cut as MorseRunner sends yours: watts with
/// zeros as `T` and nines as `N` (`1TT` for 100), and a kilowatt as you
/// write it (`K`, `KW`, `1K`).
pub fn cut_power(power: &str) -> String {
    if !power.chars().all(|c| c.is_ascii_digit()) {
        return power.to_string();
    }
    cut(power, &SERIAL_CUTS)
}

/// How often a caller with real habits cuts a zero that is not padding, and
/// a nine.
const REAL_CUT_ZERO: f64 = 0.98;
const REAL_CUT_NINE: f64 = 0.97;

/// How many callers with real habits cut a zone's or a power's zeros, ones
/// and nines, each keeping to it for the QSO; and how often one pads a
/// zone below 9 with a zero, as MorseRunner's do.
const REAL_CUT_ZONE: f64 = 0.70;
const REAL_PAD_ZONE: f64 = 0.05;
const REAL_PAD_BELOW: u32 = 9;

/// A caller's serial as it sends it. Easy habits send plain digits (`7`);
/// real ones send as MorseRunner's callers do: padded to three with the
/// padding as `TT`, other zeros as `T` almost always and nines as `N`
/// nearly as often.
pub fn send_serial(serial: u32, habits: CallerHabits, rng: &mut impl Rng) -> String {
    if habits == CallerHabits::Easy {
        return serial.to_string();
    }
    let cut_zero = rng.f64() < REAL_CUT_ZERO;
    let cut_nine = rng.f64() < REAL_CUT_NINE;
    let padded = format!("{serial:03}");
    let padding = padded.len() - padded.trim_start_matches('0').len();
    padded
        .chars()
        .enumerate()
        .map(|(at, c)| match c {
            '0' if at < padding || cut_zero => 'T',
            '9' if cut_nine => 'N',
            c => c,
        })
        .collect()
}

/// A caller's CQ zone as it sends it. Easy habits send plain digits (`5`,
/// `14`); real ones send as MorseRunner's callers do: a zone below 9 now
/// and then padded with a zero (`05`), and its zeros, ones and nines cut
/// by most callers (`T5`, `A4`).
pub fn send_zone(zone: u32, habits: CallerHabits, rng: &mut impl Rng) -> String {
    let plain = zone.to_string();
    if habits == CallerHabits::Easy {
        return plain;
    }
    let padded = if zone < REAL_PAD_BELOW && rng.f64() < REAL_PAD_ZONE {
        format!("0{plain}")
    } else {
        plain
    };
    if rng.f64() < REAL_CUT_ZONE {
        cut(&padded, &ZONE_CUTS)
    } else {
        padded
    }
}

/// A caller's power as it sends it. Easy habits send it as written (`100`,
/// `KW`); real ones as MorseRunner's callers do: a run of zeros always as
/// `TT` (`1TT`), a zero alone as `T` almost always, and most callers cut
/// ones and nines too (`ATT`). A kilowatt goes as written.
pub fn send_power(power: &str, habits: CallerHabits, rng: &mut impl Rng) -> String {
    if habits == CallerHabits::Easy || !power.chars().all(|c| c.is_ascii_digit()) {
        return power.to_string();
    }
    let cut_zero = rng.f64() < REAL_CUT_ZERO;
    let zeros = |at: usize| power[at..].starts_with("00") || power[..at].ends_with('0');
    let sent: String = power
        .char_indices()
        .map(|(at, c)| match c {
            '0' if cut_zero || zeros(at) => 'T',
            c => c,
        })
        .collect();
    if rng.f64() < REAL_CUT_ZONE {
        cut(&sent, &ZONE_CUTS)
    } else {
        sent
    }
}

/// A caller's report as it sends it, where the report is copied: plain
/// digits under easy habits (`579`), and under real ones each nine as `N`
/// nearly always, as they cut a serial's (`57N`, `5NN`).
pub fn send_report(report: u32, habits: CallerHabits, rng: &mut impl Rng) -> String {
    let plain = report.to_string();
    if habits == CallerHabits::Easy {
        return plain;
    }
    let cut_nine = rng.f64() < REAL_CUT_NINE;
    plain
        .chars()
        .map(|c| if c == '9' && cut_nine { 'N' } else { c })
        .collect()
}

/// A field as a session's group keeps it, so Letters and Mistakes see what
/// was heard: `sent` is the token that went out (`1N5`, `TT7`). When what
/// was typed holds the same value, it is `sent`, so a logger's `7` for `TT7`
/// is clean. Otherwise it is what was typed, read as values (a typed cut
/// letter is its digit, and leading zeros beyond the sent token's padding
/// go), each digit that matches the sent one written as sent and each that
/// does not as the cut letter where the sent token had one: a heard `N`
/// typed as 0 is `N`→`T`. Nothing typed is nothing kept, a miss.
pub fn keyed_token(field: Field, sent: &str, typed: &str) -> String {
    if typed.is_empty() {
        return String::new();
    }
    if field.same(sent, typed) {
        return sent.to_string();
    }
    if !field.numeric() {
        return typed.to_string();
    }
    // Lined up by value, so a heard `N` typed as 9 is a match: matched by
    // character, an added or dropped digit could pair it with something else
    // and keep a confusion that never happened.
    let value = |c: char| {
        cut_digit(c)
            .and_then(|digit| char::from_digit(digit, 10))
            .unwrap_or(c)
    };
    let padding = |values: &str| values.len() - values.trim_start_matches('0').len();
    let heard: Vec<char> = sent.chars().collect();
    let values: String = heard.iter().map(|&c| value(c)).collect();
    let typed: String = typed.chars().map(value).collect();
    // Zeros that change no value are no copy: only as many lead as the
    // sent token's, and a lone zero stays.
    let extra = padding(&typed).saturating_sub(padding(&values));
    let typed = &typed[extra.min(typed.len().saturating_sub(1))..];
    let mut next_heard = heard.iter();
    align_group(&values, typed)
        .into_iter()
        .filter_map(|pair| {
            let heard = pair.sent_char.and_then(|_| next_heard.next().copied());
            let typed = pair.received_char?;
            Some(match heard {
                Some(heard) if value(heard) == typed => heard,
                Some(heard) => typed
                    .to_digit(10)
                    .filter(|_| cut_digit(heard).is_some())
                    .and_then(|digit| cut_letter(digit, heard))
                    .unwrap_or(typed),
                None => typed,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;

    fn canonical(field: Field, token: &str) -> Option<String> {
        field.canonical(token)
    }

    /// Every equality the design's table names, field by field.
    #[test]
    fn every_equality_in_the_table_holds() {
        // NR and ZN: integers with no leading zeros.
        assert!(Field::Nr.same("TT1", "1"));
        assert!(Field::Nr.same("05", "5"));
        assert!(Field::Nr.same("1N5", "195"));
        assert!(Field::Nr.same("T1T", "10"));
        assert!(Field::Nr.same("OO7", "7"));
        assert!(Field::Zn.same("05", "5"));
        assert!(Field::Zn.same("A4", "14"));
        assert!(Field::Zn.same("TN", "9"));
        // RST: three digits, 5NN and ENN both 599.
        assert!(Field::Rst.same("5NN", "599"));
        assert!(Field::Rst.same("ENN", "599"));
        assert!(Field::Rst.same("57N", "579"));
        // PWR: watts, or a kilowatt however it is written.
        for kilowatt in ["K", "KW", "1K", "1000", "1TTT"] {
            assert!(Field::Pwr.same(kilowatt, "KW"), "{kilowatt}");
        }
        assert!(Field::Pwr.same("1TT", "100"));
        assert_eq!(canonical(Field::Pwr, "5").as_deref(), Some("5"));
        // CL: a number and a letter.
        assert!(Field::Cl.same("03A", "3A"));
        assert!(Field::Cl.same("1D", "01D"));
        // ST: a code, equal after PQ is read as QC and NF as NL; DX is DX.
        assert!(Field::St.same("OR", "OR"));
        assert!(Field::St.same("PQ", "QC"));
        assert!(Field::St.same("NF", "NL"));
        // Labrador is a code of its own; the Sprint reads it as NL
        // (`ContestRules::same`).
        assert!(!Field::St.same("LB", "NL"));
        assert!(Field::St.same("LB", "LB"));
        assert!(Field::St.same("DX", "DX"));
        assert!(!Field::St.same("OR", "WA"));
        assert!(!Field::St.same("NL", "NS"));
        // SEC: a code, equal as it is, Yukon's YT the Territories' TER;
        // and no other alias: NFL is Northern Florida, not Newfoundland.
        assert!(Field::Sec.same("EMA", "EMA"));
        assert!(Field::Sec.same("YT", "TER"));
        assert!(Field::Sec.same("DX", "DX"));
        assert!(!Field::Sec.same("NFL", "NL"));
        assert!(!Field::Sec.same("EMA", "WMA"));
        assert!(!Field::Sec.same("ONS", "ON"));
        assert_eq!(canonical(Field::Sec, "YT").as_deref(), Some("TER"));
        assert_eq!(canonical(Field::Sec, "NFL").as_deref(), Some("NFL"));
        // CL: leading zeros go.
        assert!(Field::Cl.same("03A", "3A"));
        assert!(!Field::Cl.same("3A", "3B"));
        assert!(!Field::Cl.same("13A", "3A"));
        assert_eq!(canonical(Field::Cl, "001D").as_deref(), Some("1D"));
        // NAME: letters, equal as they are.
        assert!(Field::Name.same("BOB", "BOB"));
        assert!(!Field::Name.same("BOB", "ROB"));
        // Different values are different.
        assert!(!Field::Nr.same("TT7", "17"));
        assert!(!Field::Rst.same("579", "599"));
        assert!(!Field::Pwr.same("100", "KW"));
    }

    #[test]
    fn what_is_not_a_value_of_the_field_is_refused() {
        for bad in ["", "1X5", "5-5", "1234567890", "?"] {
            assert_eq!(canonical(Field::Nr, bad), None, "{bad:?}");
        }
        for bad in ["0", "41", "TT", "NT"] {
            assert_eq!(canonical(Field::Zn, bad), None, "{bad:?}");
        }
        for bad in ["59", "5999", "699", "509", "5N"] {
            assert_eq!(canonical(Field::Rst, bad), None, "{bad:?}");
        }
        for bad in ["0", "2000", "KWW", "1KW"] {
            assert_eq!(canonical(Field::Pwr, bad), None, "{bad:?}");
        }
        for bad in ["D", "0D", "1G", "100A", "AD", "1"] {
            assert_eq!(canonical(Field::Cl, bad), None, "{bad:?}");
        }
        for bad in ["", "B0B", "BO B"] {
            assert_eq!(canonical(Field::Name, bad), None, "{bad:?}");
        }
        for bad in ["", "XX", "ORE", "or", "TN?", "1", "LBX"] {
            assert_eq!(canonical(Field::St, bad), None, "{bad:?}");
        }
        for bad in ["", "MAR", "NT", "GTA", "YUK", "NF", "MD", "ema", "3A"] {
            assert_eq!(canonical(Field::Sec, bad), None, "{bad:?}");
        }
    }

    /// A cut letter is a digit only in a numeric field: `TN` stays
    /// Tennessee, `AL` a name, `ONE` a section, and a class's letter is its
    /// letter.
    #[test]
    fn no_cut_letter_is_read_outside_a_numeric_field() {
        for field in Field::ALL {
            let cut = canonical(field, "TN");
            if field.numeric() {
                continue;
            }
            assert_ne!(cut.as_deref(), Some("9"), "{field:?} read TN as a number");
            assert_ne!(cut.as_deref(), Some("09"), "{field:?} read TN as a number");
        }
        assert_eq!(canonical(Field::Name, "AL").as_deref(), Some("AL"));
        assert_eq!(canonical(Field::Name, "TONE").as_deref(), Some("TONE"));
        assert_eq!(canonical(Field::St, "TN").as_deref(), Some("TN"));
        assert_eq!(canonical(Field::St, "NE").as_deref(), Some("NE"));
        assert_eq!(canonical(Field::Sec, "ONE").as_deref(), Some("ONE"));
        assert_eq!(canonical(Field::Sec, "TN").as_deref(), Some("TN"));
        assert_eq!(canonical(Field::Cl, "1A").as_deref(), Some("1A"));
        assert_eq!(canonical(Field::Cl, "TA"), None, "no cut digits in a class");
        assert_eq!(canonical(Field::Cl, "1T"), None);
        assert!(Field::ALL.iter().filter(|field| field.numeric()).count() == 4);
    }

    #[test]
    fn numbers_read_digits_and_cut_letters_alike() {
        assert_eq!(read_number("214"), Some(214));
        assert_eq!(read_number("TT7"), Some(7));
        assert_eq!(read_number("1N5"), Some(195));
        assert_eq!(read_number("ENN"), Some(599));
        assert_eq!(read_number("A"), Some(1));
        assert_eq!(read_number("999999999"), Some(999_999_999));
        assert_eq!(read_number("BOB"), None);
        assert_eq!(read_number(""), None);
        assert_eq!(read_number("1234567890"), None, "too long to be a number");
    }

    /// Your zone goes out cut and unpadded, and your power cut, a kilowatt
    /// as you write it, each read back as what it is.
    #[test]
    fn your_zone_and_power_are_cut() {
        assert_eq!(cut_zone(14), "A4");
        assert_eq!(cut_zone(5), "5");
        assert_eq!(cut_zone(25), "25");
        assert_eq!(cut_zone(10), "AT");
        assert_eq!(cut_zone(19), "AN");
        assert_eq!(cut_zone(40), "4T");
        for zone in 1..=40 {
            assert!(Field::Zn.same(&cut_zone(zone), &zone.to_string()), "{zone}");
        }
        assert_eq!(cut_power("100"), "1TT");
        assert_eq!(cut_power("500"), "5TT");
        assert_eq!(cut_power("5"), "5");
        assert_eq!(cut_power("1500"), "15TT");
        assert_eq!(cut_power("99"), "NN");
        for kilowatt in ["K", "KW", "1K"] {
            assert_eq!(cut_power(kilowatt), kilowatt);
            assert!(Field::Pwr.same(&cut_power(kilowatt), "1000"));
        }
        for watts in 1..=1_500 {
            let power = watts.to_string();
            assert!(Field::Pwr.same(&cut_power(&power), &power), "{power}");
        }
    }

    /// Your serial goes out padded and cut, as MorseRunner sends yours.
    #[test]
    fn your_serial_is_padded_and_cut() {
        assert_eq!(cut_serial(7), "TT7");
        assert_eq!(cut_serial(1), "TT1");
        assert_eq!(cut_serial(195), "1N5");
        assert_eq!(cut_serial(100), "1TT");
        assert_eq!(cut_serial(214), "214");
        assert_eq!(cut_serial(1_009), "1TTN");
        for serial in 1..2_000 {
            assert_eq!(read_number(&cut_serial(serial)), Some(serial));
        }
    }

    #[test]
    fn callers_send_their_serial_as_their_habits_have_it() {
        let mut rng = SplitMix64::new(5);
        assert_eq!(send_serial(7, CallerHabits::Easy, &mut rng), "7");
        assert_eq!(send_serial(190, CallerHabits::Easy, &mut rng), "190");
        let mut cut_zeros = 0;
        let mut cut_nines = 0;
        for _ in 0..1_000 {
            let sent = send_serial(90, CallerHabits::Real, &mut rng);
            assert_eq!(read_number(&sent), Some(90), "{sent}");
            assert!(sent.starts_with('T'), "{sent}: padding is always cut");
            cut_zeros += usize::from(sent.ends_with('T'));
            cut_nines += usize::from(sent.contains('N'));
        }
        assert!(
            cut_zeros > 950 && cut_zeros < 1_000,
            "{cut_zeros} zeros cut"
        );
        assert!(
            cut_nines > 930 && cut_nines < 1_000,
            "{cut_nines} nines cut"
        );
        for habits in [CallerHabits::Real, CallerHabits::Rough] {
            for serial in [1, 9, 10, 99, 100, 214, 1_000] {
                let sent = send_serial(serial, habits, &mut rng);
                assert_eq!(read_number(&sent), Some(serial), "{sent}");
                assert!(sent.len() >= 3, "{sent} is padded");
            }
        }
    }

    /// A copied report goes out plain under easy habits, and with its nines
    /// cut under real ones, read back as the report it is.
    #[test]
    fn callers_send_their_report_as_their_habits_have_it() {
        let mut rng = SplitMix64::new(6);
        assert_eq!(send_report(579, CallerHabits::Easy, &mut rng), "579");
        assert_eq!(send_report(599, CallerHabits::Easy, &mut rng), "599");
        assert_eq!(send_report(449, CallerHabits::Easy, &mut rng), "449");
        let mut cut = 0;
        for _ in 0..1_000 {
            let sent = send_report(599, CallerHabits::Real, &mut rng);
            assert!(Field::Rst.same(&sent, "599"), "{sent}");
            cut += usize::from(sent == "5NN");
        }
        assert!(cut > 930 && cut < 1_000, "{cut} cut");
        assert!(Field::Rst.same(&send_report(579, CallerHabits::Rough, &mut rng), "579"));
    }

    /// A zone goes out plain under easy habits; under real ones most
    /// callers cut its zeros, ones and nines, and a few pad a zone below 9,
    /// each read back as the zone it is.
    #[test]
    fn callers_send_their_zone_as_their_habits_have_it() {
        let mut rng = SplitMix64::new(7);
        for zone in [5, 14, 19, 40] {
            assert_eq!(
                send_zone(zone, CallerHabits::Easy, &mut rng),
                zone.to_string()
            );
        }
        let (mut cut, mut padded) = (0, 0);
        for _ in 0..2_000 {
            let sent = send_zone(14, CallerHabits::Real, &mut rng);
            assert!(["14", "A4"].contains(&sent.as_str()), "{sent}");
            cut += usize::from(sent == "A4");
            let sent = send_zone(5, CallerHabits::Real, &mut rng);
            assert!(["5", "05", "T5"].contains(&sent.as_str()), "{sent}");
            padded += usize::from(sent.len() == 2);
        }
        assert!(cut > 1_300 && cut < 1_500, "{cut} of 2000 cut");
        assert!(padded > 60 && padded < 150, "{padded} of 2000 padded");
        for habits in [CallerHabits::Real, CallerHabits::Rough] {
            for zone in 1..=40 {
                let sent = send_zone(zone, habits, &mut rng);
                assert!(Field::Zn.same(&sent, &zone.to_string()), "{sent}");
            }
        }
        // A zone of 9 or more is never padded.
        assert!((0..500).all(|_| send_zone(25, CallerHabits::Real, &mut rng).len() == 2));
    }

    /// Power goes out as written under easy habits; under real ones a run
    /// of zeros is always cut, a zero alone almost always, and most callers
    /// cut ones and nines too; a kilowatt goes as written, and each reads
    /// back as the power it is.
    #[test]
    fn callers_send_their_power_as_their_habits_have_it() {
        let mut rng = SplitMix64::new(8);
        for power in ["100", "5", "KW", "K", "1K"] {
            assert_eq!(send_power(power, CallerHabits::Easy, &mut rng), power);
        }
        let mut cut = 0;
        for _ in 0..2_000 {
            let sent = send_power("100", CallerHabits::Real, &mut rng);
            assert!(["1TT", "ATT"].contains(&sent.as_str()), "{sent}");
            cut += usize::from(sent == "ATT");
            let sent = send_power("50", CallerHabits::Real, &mut rng);
            assert!(["5T", "50"].contains(&sent.as_str()), "{sent}");
        }
        assert!(cut > 1_300 && cut < 1_500, "{cut} of 2000 cut");
        for habits in [CallerHabits::Real, CallerHabits::Rough] {
            for power in ["5", "10", "99", "100", "150", "500", "750", "1500"] {
                let sent = send_power(power, habits, &mut rng);
                assert!(Field::Pwr.same(&sent, power), "{power} as {sent}");
            }
            for kilowatt in KILOWATT_SPELLINGS {
                assert_eq!(send_power(kilowatt, habits, &mut rng), kilowatt);
            }
        }
    }

    /// What a group keeps of a field: the sent token when the values match,
    /// and otherwise what was typed with its digits cut where the sent token
    /// was.
    #[test]
    fn a_field_keeps_the_token_as_keyed() {
        // The same value, however typed, is the token that went out.
        assert_eq!(keyed_token(Field::Nr, "TT7", "7"), "TT7");
        assert_eq!(keyed_token(Field::Nr, "TT7", "007"), "TT7");
        assert_eq!(keyed_token(Field::Rst, "5NN", "599"), "5NN");
        // A heard N typed as 0 is N→T, and the other digits stay digits.
        assert_eq!(keyed_token(Field::Nr, "1N5", "105"), "1T5");
        assert_eq!(keyed_token(Field::Nr, "214", "215"), "215");
        assert_eq!(keyed_token(Field::Nr, "1N5", "175"), "175");
        // A typed 1 under a heard T is the A it would be cut to.
        assert_eq!(keyed_token(Field::Nr, "T1T", "110"), "A1T");
        // A typed 5 under a heard cut letter is the E it would be cut to:
        // a heard T taken for an E.
        assert_eq!(keyed_token(Field::Nr, "TT7", "57"), "E7");
        // A dropped character is dropped, not misread.
        assert_eq!(keyed_token(Field::Nr, "1N5", "15"), "15");
        // A cut letter copied right stays right beside a digit added: the
        // digits line up by value.
        assert_eq!(keyed_token(Field::Nr, "1N5", "1955"), "1N55");
        // Which 9 was added cannot be told; either way T and N were copied.
        let kept = keyed_token(Field::Nr, "2TN", "2099");
        assert!(matches!(kept.as_str(), "2TN9" | "2T9N"), "{kept}");
        // Nothing typed is a miss.
        assert_eq!(keyed_token(Field::Nr, "TT7", ""), "");
        // Outside a number, what was typed is what was typed.
        assert_eq!(keyed_token(Field::Name, "BOB", "ROB"), "ROB");
        assert_eq!(keyed_token(Field::Name, "BOB", "BOB"), "BOB");
        // Its value is always the value typed.
        for (sent, typed) in [
            ("1N5", "105"),
            ("TT7", "17"),
            ("T1T", "110"),
            ("1N5", "1955"),
            ("2TN", "2099"),
            ("TT7", "57"),
        ] {
            let kept = keyed_token(Field::Nr, sent, typed);
            assert!(Field::Nr.same(&kept, typed), "{sent} {typed} kept {kept}");
        }
    }

    #[test]
    fn every_field_has_a_name_a_label_and_a_noun() {
        for field in Field::ALL {
            assert_eq!(Field::from_name(field.name()), Some(field));
            assert_eq!(field.label(), field.name().to_ascii_uppercase());
            assert!(!field.noun().is_empty());
            // What the card shows a station sending is a value it could send.
            assert!(field.canonical(field.example()).is_some(), "{field:?}");
        }
        assert_eq!(Field::from_name("NR"), None);
    }

    /// A typed cut letter is its digit, and typed padding is no copy: `A5`
    /// for `14` is 1 copied and 4 taken for 5, `008` for `7` is 7 taken for
    /// 8, each one error and no false confusion.
    #[test]
    fn a_typed_cut_letter_or_padding_is_read_by_value() {
        assert_eq!(keyed_token(Field::Nr, "14", "A5"), "15");
        assert_eq!(keyed_token(Field::Nr, "7", "008"), "8");
        assert_eq!(keyed_token(Field::Nr, "195", "1N4"), "194");
        assert_eq!(keyed_token(Field::Nr, "7", "TT8"), "8");
        // Padding the sent token has stays, as sent.
        assert_eq!(keyed_token(Field::Nr, "TT7", "0008"), "TT8");
        assert_eq!(keyed_token(Field::Nr, "TT7", "8"), "8");
        // A lone zero is a value.
        assert_eq!(keyed_token(Field::Nr, "7", "0"), "0");
        assert_eq!(keyed_token(Field::Nr, "1N5", "A0"), "10");
        for (sent, typed) in [("14", "A5"), ("7", "008"), ("195", "1N4"), ("7", "TT8")] {
            let kept = keyed_token(Field::Nr, sent, typed);
            let copy = crate::score::GroupCopy::of(sent, &kept, 25.0, 100.0, 1);
            assert_eq!(copy.errors, 1, "{sent} {typed} kept {kept}");
        }
    }
}
