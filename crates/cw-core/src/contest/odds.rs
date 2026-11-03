//! How often callers do each thing they do now and then, and how often
//! you sign off `73 44`: every one a share you set, from nothing to all,
//! none fixed in the code. Each starts where it was heard on the air, as
//! MorseRunner's callers do it where they have it, so a run left as it is
//! goes exactly as it always has.

use serde::{Deserialize, Deserializer, Serialize};

use super::data::counted::FIELD_DAY_DX;

/// How often callers do each thing they do now and then, each a share from
/// nothing to one ([`Odd`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallerOdds {
    /// Of the callers who join once five QSOs are logged, stations you
    /// worked before, where a contest scores (§4.7 of the design).
    pub dupe: f64,
    /// Contest callers that sign portable (`/P`, `/M`, `/QRP`, an area).
    pub portable: f64,
    /// Contest callers operating abroad (`DL/W1AW`).
    pub abroad: f64,
    /// Field Day callers from outside North America, who send DX.
    pub field_day_dx: f64,
    /// Pile-up callers that send their call twice (`DxOper.pas:500-502`).
    pub calls_twice: f64,
    /// Pile-up callers that miss your exchange and ask for it again
    /// (`DxOper.pas:762-770`).
    pub misses_exchange: f64,
    /// A caller waiting for your exchange asks with the contest's first
    /// query rather than its second: `NR?` rather than `AGN`
    /// (`DxOper.pas`, `GetReply`).
    pub asks_nr: f64,
    /// Operators putting your call right once they have your exchange that
    /// send their own with it (`DxOper.pas:851-867`).
    pub fixes_with_exchange: f64,
    /// Your TU's farewell is `73 44` rather than `73`, where the TU signs
    /// off with one, as POTA's does.
    pub farewell_44: f64,
    /// Under the Band card's flutter, callers whose signal flutters
    /// (`DxStn.pas:97-99`).
    pub flutter: f64,
    /// Under real habits, messages that send 599 as `ENN`
    /// (`Station.pas:527-528`).
    pub enn: f64,
    /// Under real habits, a serial's zero alone sent as `T`
    /// (`Station.pas:538-551`).
    pub serial_zero_as_t: f64,
    /// Under real habits, nines sent as `N`, in a serial or a report other
    /// than 599 (`Station.pas:564-565`).
    pub nine_as_n: f64,
    /// Under real habits, any other zero alone sent as `O`.
    pub zero_as_o: f64,
    /// Under real habits, any other zero alone not sent as `O` sent as `T`.
    pub zero_as_t: f64,
    /// Under real habits, callers that cut a zone's or a power's zeros,
    /// ones and nines, each keeping to it for the QSO
    /// (`Station.pas:555-562`).
    pub zone_cut: f64,
    /// Under real habits, a zone below 9 padded with a zero
    /// (`CqWW.pas:305-307`).
    pub zone_padded: f64,
    /// Under rough habits, careless operators that fumble a digit of their
    /// first serial, then send `EEEEE` and the serial right
    /// (`DxStn.pas:73`, `Station.pas:509-521`).
    pub fumbles: f64,
    /// Under rough habits, careless operators that send their exchange with
    /// their call put right before they have yours (`DxOper.pas:879-886`).
    pub early: f64,
    /// Under rough habits, how often a careless operator takes a slip of
    /// its call for its own (`DxOper.pas:651-671`).
    pub slips: f64,
}

impl Default for CallerOdds {
    fn default() -> Self {
        Self {
            dupe: 0.02,
            portable: crate::callsign::CONTEST_PORTABLE_SHARE,
            abroad: crate::callsign::CONTEST_ABROAD_SHARE,
            field_day_dx: FIELD_DAY_DX / 100.0,
            calls_twice: 0.1,
            misses_exchange: 0.1,
            asks_nr: 0.3,
            fixes_with_exchange: 5.0 / 6.0,
            farewell_44: 0.25,
            flutter: 0.3,
            enn: 0.05,
            serial_zero_as_t: 0.98,
            nine_as_n: 0.97,
            zero_as_o: 0.4,
            zero_as_t: 0.97,
            zone_cut: 0.70,
            zone_padded: 0.05,
            fumbles: 0.1,
            early: 0.88 - 5.0 / 6.0,
            slips: 0.04,
        }
    }
}

/// One of [`CallerOdds`]: what a slider sets, a save names and the clamp
/// keeps to its bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Odd {
    Dupe,
    Portable,
    Abroad,
    FieldDayDx,
    CallsTwice,
    MissesExchange,
    AsksNr,
    FixesWithExchange,
    Farewell44,
    Flutter,
    Enn,
    SerialZeroAsT,
    NineAsN,
    ZeroAsO,
    ZeroAsT,
    ZoneCut,
    ZonePadded,
    Fumbles,
    Early,
    Slips,
}

/// The card a share is set on, and its Reset puts back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OddsCard {
    /// The Band conditions card: flutter, and the callers' habits.
    Band,
    /// The contest card: who calls, how they call, and your TU.
    Contest,
}

impl Odd {
    pub const ALL: [Self; 20] = [
        Self::Dupe,
        Self::Portable,
        Self::Abroad,
        Self::FieldDayDx,
        Self::CallsTwice,
        Self::MissesExchange,
        Self::AsksNr,
        Self::FixesWithExchange,
        Self::Farewell44,
        Self::Flutter,
        Self::Enn,
        Self::SerialZeroAsT,
        Self::NineAsN,
        Self::ZeroAsO,
        Self::ZeroAsT,
        Self::ZoneCut,
        Self::ZonePadded,
        Self::Fumbles,
        Self::Early,
        Self::Slips,
    ];

    /// Its name in a save.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dupe => "dupe",
            Self::Portable => "portable",
            Self::Abroad => "abroad",
            Self::FieldDayDx => "fieldDayDx",
            Self::CallsTwice => "callsTwice",
            Self::MissesExchange => "missesExchange",
            Self::AsksNr => "asksNr",
            Self::FixesWithExchange => "fixesWithExchange",
            Self::Farewell44 => "farewell44",
            Self::Flutter => "flutter",
            Self::Enn => "enn",
            Self::SerialZeroAsT => "serialZeroAsT",
            Self::NineAsN => "nineAsN",
            Self::ZeroAsO => "zeroAsO",
            Self::ZeroAsT => "zeroAsT",
            Self::ZoneCut => "zoneCut",
            Self::ZonePadded => "zonePadded",
            Self::Fumbles => "fumbles",
            Self::Early => "early",
            Self::Slips => "slips",
        }
    }

    /// The card it is set on.
    pub fn card(self) -> OddsCard {
        match self {
            Self::Flutter
            | Self::Enn
            | Self::SerialZeroAsT
            | Self::NineAsN
            | Self::ZeroAsO
            | Self::ZeroAsT
            | Self::ZoneCut
            | Self::ZonePadded
            | Self::Fumbles
            | Self::Early
            | Self::Slips => OddsCard::Band,
            _ => OddsCard::Contest,
        }
    }

    /// Its share in `odds`.
    pub fn of(self, odds: &CallerOdds) -> f64 {
        match self {
            Self::Dupe => odds.dupe,
            Self::Portable => odds.portable,
            Self::Abroad => odds.abroad,
            Self::FieldDayDx => odds.field_day_dx,
            Self::CallsTwice => odds.calls_twice,
            Self::MissesExchange => odds.misses_exchange,
            Self::AsksNr => odds.asks_nr,
            Self::FixesWithExchange => odds.fixes_with_exchange,
            Self::Farewell44 => odds.farewell_44,
            Self::Flutter => odds.flutter,
            Self::Enn => odds.enn,
            Self::SerialZeroAsT => odds.serial_zero_as_t,
            Self::NineAsN => odds.nine_as_n,
            Self::ZeroAsO => odds.zero_as_o,
            Self::ZeroAsT => odds.zero_as_t,
            Self::ZoneCut => odds.zone_cut,
            Self::ZonePadded => odds.zone_padded,
            Self::Fumbles => odds.fumbles,
            Self::Early => odds.early,
            Self::Slips => odds.slips,
        }
    }

    /// Its share in `odds`, to set.
    pub fn of_mut(self, odds: &mut CallerOdds) -> &mut f64 {
        match self {
            Self::Dupe => &mut odds.dupe,
            Self::Portable => &mut odds.portable,
            Self::Abroad => &mut odds.abroad,
            Self::FieldDayDx => &mut odds.field_day_dx,
            Self::CallsTwice => &mut odds.calls_twice,
            Self::MissesExchange => &mut odds.misses_exchange,
            Self::AsksNr => &mut odds.asks_nr,
            Self::FixesWithExchange => &mut odds.fixes_with_exchange,
            Self::Farewell44 => &mut odds.farewell_44,
            Self::Flutter => &mut odds.flutter,
            Self::Enn => &mut odds.enn,
            Self::SerialZeroAsT => &mut odds.serial_zero_as_t,
            Self::NineAsN => &mut odds.nine_as_n,
            Self::ZeroAsO => &mut odds.zero_as_o,
            Self::ZeroAsT => &mut odds.zero_as_t,
            Self::ZoneCut => &mut odds.zone_cut,
            Self::ZonePadded => &mut odds.zone_padded,
            Self::Fumbles => &mut odds.fumbles,
            Self::Early => &mut odds.early,
            Self::Slips => &mut odds.slips,
        }
    }
}

impl CallerOdds {
    /// Each share kept from nothing to one, and one that is no number
    /// where it starts.
    pub fn clamped(mut self) -> Self {
        let start = Self::default();
        for odd in Odd::ALL {
            let share = odd.of_mut(&mut self);
            *share = if share.is_finite() {
                share.clamp(0.0, 1.0)
            } else {
                odd.of(&start)
            };
        }
        self
    }

    /// The shares set on `card` taken from `from`, the rest kept: what that
    /// card's Reset does.
    pub fn reset_card(&mut self, card: OddsCard, from: &Self) {
        for odd in Odd::ALL.into_iter().filter(|odd| odd.card() == card) {
            *odd.of_mut(self) = odd.of(from);
        }
    }
}

/// As saved: each share by its name, a number kept to its bounds; one left
/// out, or anything but a number, where it starts; anything but such a
/// list, every one where it starts.
impl<'de> Deserialize<'de> for CallerOdds {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let mut odds = Self::default();
        for odd in Odd::ALL {
            if let Some(share) = value
                .get(odd.name())
                .and_then(serde_json::Value::as_f64)
                .filter(|share| share.is_finite())
            {
                *odd.of_mut(&mut odds) = share.clamp(0.0, 1.0);
            }
        }
        Ok(odds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each share starts where it was heard on the air, its name in a save
    /// its own, and every share is named once.
    #[test]
    fn each_share_starts_as_heard_on_the_air() {
        let odds = CallerOdds::default();
        assert_eq!(odds.flutter, 0.3);
        assert_eq!(odds.farewell_44, 0.25);
        assert!((odds.field_day_dx - 0.009).abs() < 1e-12);
        let names: std::collections::HashSet<&str> =
            Odd::ALL.iter().map(|odd| odd.name()).collect();
        assert_eq!(names.len(), Odd::ALL.len());
        let json = serde_json::to_value(odds).expect("serialize");
        for odd in Odd::ALL {
            assert_eq!(json[odd.name()].as_f64(), Some(odd.of(&odds)), "{odd:?}");
            assert!((0.0..=1.0).contains(&odd.of(&odds)), "{odd:?}");
        }
    }

    /// Read back, each share is kept to its bounds, and one that is missing
    /// or no number starts where it did; a save that is no list is every
    /// share as it starts.
    #[test]
    fn shares_read_back_forgivingly() {
        let read = |json: serde_json::Value| -> CallerOdds {
            serde_json::from_value(json).expect("reads")
        };
        let odds = read(serde_json::json!({ "flutter": 0.5, "dupe": 7, "enn": -1, "slips": "x" }));
        assert_eq!(odds.flutter, 0.5);
        assert_eq!(odds.dupe, 1.0);
        assert_eq!(odds.enn, 0.0);
        assert_eq!(odds.slips, CallerOdds::default().slips);
        assert_eq!(odds.calls_twice, CallerOdds::default().calls_twice);
        assert_eq!(read(serde_json::json!("lots")), CallerOdds::default());
        let wild = CallerOdds {
            fumbles: f64::NAN,
            abroad: 3.0,
            ..CallerOdds::default()
        };
        let kept = wild.clamped();
        assert_eq!(kept.fumbles, CallerOdds::default().fumbles);
        assert_eq!(kept.abroad, 1.0);
    }

    /// A card's Reset puts back its own shares and no other card's.
    #[test]
    fn a_cards_reset_puts_back_its_own() {
        let mut odds = CallerOdds {
            flutter: 0.9,
            dupe: 0.5,
            ..CallerOdds::default()
        };
        odds.reset_card(OddsCard::Band, &CallerOdds::default());
        assert_eq!(odds.flutter, 0.3);
        assert_eq!(odds.dupe, 0.5);
        odds.reset_card(OddsCard::Contest, &CallerOdds::default());
        assert_eq!(odds, CallerOdds::default());
    }
}
