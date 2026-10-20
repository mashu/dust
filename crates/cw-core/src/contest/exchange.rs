//! An exchange's fields: what each one holds, the one way each value is
//! written for comparing (its canonical form), and the cut numbers operators
//! send in place of digits.
//!
//! Operators shorten digits to letters with fewer elements: `T` for 0, `N`
//! for 9, `A` for 1, `E` for 5, so a serial of 7 goes out as `TT7` and a
//! report of 599 as `5NN`. A cut letter is read only in a field that holds a
//! number: elsewhere `TN` is Tennessee and `AL` a name.

use super::CallerHabits;
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
const ZONE_MAX: u32 = 40;

/// The most power an amateur station runs, in watts.
const POWER_MAX: u32 = 1_500;

/// Power that is logged as `KW`.
const KILOWATT: u32 = 1_000;

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

    /// Whether it holds a number, the only place a cut letter is read.
    pub fn numeric(self) -> bool {
        matches!(self, Self::Rst | Self::Nr | Self::Zn | Self::Pwr)
    }

    /// `token` written the one way this field compares it, or `None` if it
    /// cannot be this field: a serial with no leading zeros (`TT1` is `1`),
    /// a report as three digits (`5NN` is `599`), power as watts or `KW`, a
    /// class without leading zeros (`03A` is `3A`). Tokens are upper case.
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
            // States and sections are read off their lists, which come with
            // the contests that use them: Sprint, then ARRL DX and Field Day.
            // Until then nothing reads as either.
            Self::St | Self::Sec => None,
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
    match digit {
        0 => Some('T'),
        1 => Some('A'),
        5 => Some('E'),
        9 => Some('N'),
        _ => None,
    }
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
    if matches!(token, "K" | "KW" | "1K") {
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

/// Your serial as you send it, as MorseRunner sends yours: padded to three
/// digits, its zeros cut to `T` and nines to `N` (`TT7`, `1N5`).
pub fn cut_serial(serial: u32) -> String {
    format!("{serial:03}")
        .chars()
        .map(|c| match c {
            '0' => 'T',
            '9' => 'N',
            c => c,
        })
        .collect()
}

/// How often a caller with real habits cuts a zero that is not padding, and
/// a nine.
const REAL_CUT_ZERO: f64 = 0.98;
const REAL_CUT_NINE: f64 = 0.97;

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

/// A field as a session's group keeps it, so Letters and Mistakes see what
/// was heard: `sent` is the token that went out (`1N5`, `TT7`). When what
/// was typed holds the same value, it is `sent`, so a logger's `7` for `TT7`
/// is clean. Otherwise it is what was typed, with a typed digit written as
/// the cut letter where the sent token had one: a heard `N` typed as 0 is
/// `N`→`T`. Nothing typed is nothing kept, a miss.
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
    let heard: Vec<char> = sent.chars().collect();
    let values: String = heard
        .iter()
        .map(|&c| {
            cut_digit(c)
                .and_then(|digit| char::from_digit(digit, 10))
                .unwrap_or(c)
        })
        .collect();
    let mut next_heard = heard.iter();
    align_group(&values, typed)
        .into_iter()
        .filter_map(|pair| {
            let heard = pair.sent_char.and_then(|_| next_heard.next().copied());
            let typed = pair.received_char?;
            let as_cut = heard
                .filter(|heard| cut_digit(*heard).is_some())
                .and_then(|heard| cut_letter(typed.to_digit(10)?, heard));
            Some(as_cut.unwrap_or(typed))
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
        // States and sections have no list yet, so nothing is one.
        assert_eq!(canonical(Field::St, "OR"), None);
        assert_eq!(canonical(Field::Sec, "EMA"), None);
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
        }
        assert_eq!(Field::from_name("NR"), None);
    }
}
