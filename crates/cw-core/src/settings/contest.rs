//! What Contest mode adds to the settings: which contest a run is, whether
//! one station calls at a time or a pile-up, how its callers operate,
//! whether some flutter and whether their speed follows your copy, how long
//! a run lasts, how often your TU signs, how fast and how loud you send,
//! whether you hear the band while you do, and your station.
//! Everything else a run uses is shared with group training.

use serde::{Deserialize, Serialize};

use super::{STATIONS_MIN, TrainingSettings, WPM_MAX, WPM_MIN};
use crate::callsign::parse_callsign;
use crate::contest::data::places::{DX, call_section, fold_labrador, in_us_or_canada, place_code};
use crate::contest::data::sections::{fold_yukon, listed_section};
use crate::contest::data::zones::zone_of;
use crate::contest::exchange::{KILOWATT_SPELLINGS, POWER_MAX, ZONE_MAX};
use crate::contest::{CallerHabits, ContestKind, Field, MONITOR_LEVEL, RunStyle, Side, TuCall};

/// The longest run, in minutes, and where a run left open ends.
pub const CONTEST_MINUTES_MAX: u32 = 60;

/// Your speed, in whole words a minute, inside the Speed card's bounds: 0
/// below them sends at the middle of the callers' character speed range
/// ([`TrainingSettings::my_wpm`]).
pub const MY_WPM_MIN: u32 = WPM_MIN as u32;
pub const MY_WPM_MAX: u32 = WPM_MAX as u32;

/// How loud you hear your own messages, in decibels from the loudest, as
/// MorseRunner's self-monitor level is set (`MonLevel`, -60 to 0 dB): at the
/// top, [`MONITOR_LEVEL`]; at the bottom, nothing at all.
pub const MY_SENDING_MAX_DB: i32 = 0;
pub const MY_SENDING_OFF_DB: i32 = -60;

/// Your sending until you set it: level with an average caller as you hear
/// it. Its loudest, 0 dB, is the limiter's knee, about where the strongest
/// station a receiver plays at full volume crests; the AGC holds every
/// caller over its threshold within a few decibels of that, a caller drawn
/// anywhere in the default strength range about 2 dB under it. Three
/// down, on the control's own steps, your message sits a decibel or so
/// under the caller, about level, as a rig's sidetone set to the receiver's
/// audio is. Measured through the default receiver at full volume: the
/// caller at 0.57 of full scale, your message at 0.50.
const MY_SENDING_DEFAULT_DB: i32 = -3;

/// The longest callsign kept: room for a host and a marker around a call,
/// `VP2E/DL1ABC/P` aside.
pub const MY_CALL_MAX: usize = 12;

/// The longest name kept: a given name as operators send it is short.
pub const MY_NAME_MAX: usize = 10;

/// The highest CQ zone, and the zone setting's top: 0 below it takes your
/// zone from your call.
pub const MY_ZONE_MAX: u32 = ZONE_MAX;

/// The most power kept, in watts: the most an amateur station runs, as the
/// exchange reads a power.
pub const MY_POWER_MAX: u32 = POWER_MAX;

/// The power sent until you give yours, and what a power that is none is
/// kept as.
const MY_POWER_DEFAULT: &str = "100";

/// The Field Day class sent until you give yours, and what a class that is
/// none is kept as: one transmitter at home, the class most stations send.
const MY_CLASS_DEFAULT: &str = "1D";

/// The most callers at once a pile-up starts from, picked while the Band
/// card's stations range holds one station: one to three, as the design
/// gives a pile-up (§8).
const PILEUP_CALLERS_MAX: u32 = 3;

/// Your call where Hear a QSO plays one and none a contest can read is
/// given yet: a North American one for a contest that sends your state or
/// your section, as its callers are there.
const STAND_IN_CALL: (&str, &str) = ("DL1ABC", "K1ABC");
/// Your name, state and section, where a contest sends them and none is
/// given yet: the stand-in call's.
const STAND_IN_NAME: &str = "MIKE";
const STAND_IN_STATE: &str = "MA";
const STAND_IN_SECTION: &str = "EMA";
/// Your CQ zone, where a contest sends it and your call does not say: the
/// stand-in call's.
const STAND_IN_ZONE: u32 = 14;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContestSettings {
    /// The contest a run is, chosen where the alphabet is in other modes.
    pub contest_kind: ContestKind,
    /// One caller at a time, or a pile-up of as many as the Band card's
    /// stations range draws.
    pub contest_style: RunStyle,
    /// How the callers operate: plain digits and no mistakes, numbers as
    /// contesters send them, or that with careless operators and stations
    /// on your frequency that are not calling you. Set on the Band card.
    pub contest_habits: CallerHabits,
    /// Flutter: three callers in ten have a signal that flutters, rising
    /// and falling 3 to 33 times a second as a signal over a polar path
    /// does, as MorseRunner's flutter has them. Set on the Band card.
    #[serde(deserialize_with = "forgiving_off")]
    pub contest_flutter: bool,
    /// How long a run lasts, in minutes. Zero runs until you stop it, and
    /// ends at [`CONTEST_MINUTES_MAX`] all the same.
    pub contest_minutes: u32,
    /// Show each caller's text on the run screen once it has been sent. A
    /// run with it on is assisted: counted in every stat, but never placed
    /// on the board.
    #[serde(deserialize_with = "forgiving_off")]
    pub contest_show_copy: bool,
    /// How often your TU signs with your call, where the contest's TU does
    /// not sign by its own rules: every QSO, every third, or never. Set on
    /// the contest card, with the run style.
    pub contest_tu_call: TuCall,
    /// Each new caller's speed drawn from your copy speed in contests, within
    /// the Speed card's ranges: faster as you copy calls and numbers clean,
    /// slower as you miss them. Off, it is drawn evenly from the ranges. A
    /// switch of its own, so group training's adaptive speed is left as it
    /// is. Set on the Speed card.
    #[serde(deserialize_with = "forgiving_on")]
    pub contest_adapt_speed: bool,
    /// Your speed, in words a minute, kept from run to run as − and + leave
    /// it. Zero, until you set it, sends at the middle of the callers'
    /// character speed range ([`TrainingSettings::my_wpm`]). Set on the
    /// Speed card.
    #[serde(deserialize_with = "forgiving_wpm")]
    pub my_wpm: u32,
    /// How loud you hear your own messages, in decibels from the loudest,
    /// [`MONITOR_LEVEL`]; [`MY_SENDING_OFF_DB`] is silent. Set on the Tone
    /// and volume card.
    #[serde(deserialize_with = "forgiving_sending")]
    pub my_sending_db: i32,
    /// Full break-in, QSK: the band is heard through your own messages, as
    /// a rig with full break-in hears between the elements it sends. Off, as
    /// MorseRunner ships it, the receiver is muted while you send, as a rig
    /// without it mutes, and a caller sending across your message is not
    /// heard. Set on the Tone and volume card.
    #[serde(deserialize_with = "forgiving_off")]
    pub contest_qsk: bool,
    /// Your callsign: upper case, letters, digits and single slashes inside
    /// it. Empty until you give one.
    pub my_call: String,
    /// Your name, as you send it in a contest whose exchange holds one:
    /// letters only, upper case. Empty until you give one.
    pub my_name: String,
    /// Your US state, Canadian province or territory, by its code, or DX,
    /// as you send it in a contest whose exchange holds one; or LB for
    /// Labrador, which ARRL DX counts apart. Empty until you give one, when
    /// a call outside the US and Canada sends DX ([`Self::state_sent`]).
    pub my_state: String,
    /// Your CQ zone, as you send it in CQ WW, for a call whose zone it
    /// cannot tell, or one Dust cannot place: 1 to 40, and 0 to take it
    /// from your call ([`Self::zone`]).
    pub my_zone: u32,
    /// Your power, as you send it in ARRL DX from outside W/VE: watts from
    /// 1 to 1500, or `K`, `KW` or `1K` for a kilowatt.
    pub my_power: String,
    /// Your Field Day class: how many transmitters, 1 to 99, and a letter
    /// from A to F.
    pub my_class: String,
    /// Your ARRL or RAC section, by its code, Yukon's YT, or DX, as you send
    /// it in Field Day. Empty until you give one, when a call that tells its
    /// section by itself sends that one ([`Self::section_sent`]).
    pub my_section: String,
}

impl Default for ContestSettings {
    fn default() -> Self {
        Self {
            contest_kind: ContestKind::Wpx,
            contest_style: RunStyle::Single,
            contest_habits: CallerHabits::Easy,
            contest_flutter: false,
            contest_minutes: 10,
            contest_show_copy: false,
            contest_tu_call: TuCall::Every,
            contest_adapt_speed: true,
            my_wpm: 0,
            my_sending_db: MY_SENDING_DEFAULT_DB,
            contest_qsk: false,
            my_call: String::new(),
            my_name: String::new(),
            my_state: String::new(),
            my_zone: 0,
            my_power: MY_POWER_DEFAULT.to_string(),
            my_class: MY_CLASS_DEFAULT.to_string(),
            my_section: String::new(),
        }
    }
}

/// A switch saved as true or false. Anything else, a save edited by hand,
/// reads as the switch ships, `fallback`, rather than costing the rest of
/// the save, as a contest this build does not know does.
fn forgiving_switch<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
    fallback: bool,
) -> Result<bool, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(fallback))
}

/// A switch that ships off, as saved; anything else is off.
fn forgiving_off<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    forgiving_switch(deserializer, false)
}

/// A switch that ships on, as saved; anything else is on.
fn forgiving_on<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    forgiving_switch(deserializer, true)
}

/// A whole number saved as one, as `whole` reads it; anything else, a save
/// edited by hand, is `fallback` rather than costing the rest of the save.
/// A number past what `whole` holds is held at its end, for
/// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) to bound.
fn forgiving_number<'de, D: serde::Deserializer<'de>, T>(
    deserializer: D,
    whole: fn(f64) -> T,
    fallback: T,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_f64()
        .filter(|number| number.is_finite())
        .map_or(fallback, |number| whole(number.round())))
}

/// Your speed as saved: a speed of your own, or 0 for the middle of the
/// callers' range, which is what anything but a number reads as.
fn forgiving_wpm<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    // A float cast to an integer saturates at its ends.
    forgiving_number(deserializer, |wpm| wpm as u32, 0)
}

/// Your sending as saved, in decibels; anything but a number is the
/// level it starts at.
fn forgiving_sending<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i32, D::Error> {
    forgiving_number(deserializer, |db| db as i32, MY_SENDING_DEFAULT_DB)
}

impl ContestSettings {
    /// How loud your own messages play, as a peak of full scale: the
    /// monitor's level, [`MONITOR_LEVEL`], turned down by your sending's
    /// decibels, and nothing at all at [`MY_SENDING_OFF_DB`].
    pub fn sending_level(&self) -> f64 {
        if self.my_sending_db <= MY_SENDING_OFF_DB {
            return 0.0;
        }
        let db = f64::from(self.my_sending_db.min(MY_SENDING_MAX_DB));
        MONITOR_LEVEL * 10f64.powf(db / 20.0)
    }

    /// How long a run lasts at most, in minutes: what it was set to, or the
    /// longest for a run left open.
    pub fn limit_minutes(&self) -> u32 {
        match self.contest_minutes {
            0 => CONTEST_MINUTES_MAX,
            minutes => minutes.min(CONTEST_MINUTES_MAX),
        }
    }

    /// Your callsign, as typed: kept as a callsign can be written, as
    /// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) keeps it.
    pub fn set_my_call(&mut self, typed: &str) {
        self.my_call = clean_call(typed);
    }

    /// Your name, as typed: kept as a name is sent, as
    /// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) keeps it.
    pub fn set_my_name(&mut self, typed: &str) {
        self.my_name = clean_name(typed);
    }

    /// Your state, province or territory, as typed: kept as its code, as
    /// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) keeps
    /// it, and nothing for what is none.
    pub fn set_my_state(&mut self, typed: &str) {
        self.my_state = clean_state(typed);
    }

    /// Your CQ zone, as typed: a zone from 1 to [`MY_ZONE_MAX`], and 0, to
    /// take it from your call, for anything else.
    pub fn set_my_zone(&mut self, typed: &str) {
        self.my_zone = typed
            .trim()
            .parse()
            .ok()
            .filter(|zone| *zone <= MY_ZONE_MAX)
            .unwrap_or(0);
    }

    /// Your power, as typed: kept as
    /// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) keeps
    /// it, and 100 W for what is none.
    pub fn set_my_power(&mut self, typed: &str) {
        self.my_power = clean_power(typed);
    }

    /// Your Field Day class, as typed: kept as
    /// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) keeps
    /// it, and 1D for what is none.
    pub fn set_my_class(&mut self, typed: &str) {
        self.my_class = clean_class(typed);
    }

    /// Your section, as typed: kept as its code, as
    /// [`TrainingSettings::clamp`](super::TrainingSettings::clamp) keeps
    /// it, and nothing for what is none.
    pub fn set_my_section(&mut self, typed: &str) {
        self.my_section = clean_section(typed);
    }

    /// Your side of the ARRL DX contest, from your call.
    pub fn side(&self) -> Side {
        Side::of_call(&self.my_call)
    }

    /// The CQ zone your call reads as: by your state or province where you
    /// give one in the US or Canada, else by where your call puts you. None
    /// for a call Dust cannot place.
    pub fn zone_from_call(&self) -> Option<u32> {
        zone_of(&self.my_call, Some(self.my_state.as_str()))
    }

    /// Your state, province or territory, where it and not your call alone
    /// gives the zone your call reads as ([`Self::zone_from_call`]): one
    /// you give for a call in the US or Canada whose zone it changes, or
    /// whose digit names no place. It does so in every contest, those that
    /// do not send it among them.
    pub fn zone_state(&self) -> Option<&'static str> {
        let state = place_code(&self.my_state)?;
        (zone_of(&self.my_call, Some(state)) != zone_of(&self.my_call, None)).then_some(state)
    }

    /// The CQ zone your exchange sends: the one you gave, or else the one
    /// your call reads as ([`Self::zone_from_call`]). None while neither is
    /// known.
    pub fn zone(&self) -> Option<u32> {
        match self.my_zone {
            0 => self.zone_from_call(),
            zone => Some(zone),
        }
    }

    /// The state, province or territory your exchange sends: the one you
    /// gave, or else DX for a call Dust places outside the US and Canada,
    /// as Dust has every station there send (the real contests have one
    /// elsewhere in North America send its country). Labrador is NL but in
    /// a contest that counts it apart. Empty while a call in the US or
    /// Canada, Alaska and Hawaii among them, has none, or one Dust cannot
    /// place, which may be there, or no call reads.
    pub fn state_sent(&self) -> &str {
        if !self.my_state.is_empty() {
            let apart = self.contest_kind.rules().labrador;
            return if apart {
                &self.my_state
            } else {
                fold_labrador(&self.my_state)
            };
        }
        if in_us_or_canada(&self.my_call) == Some(false) {
            DX
        } else {
            ""
        }
    }

    /// The section your exchange sends: the one you gave, Yukon's as the
    /// Territories', which ARRL contests count it in; or else the one your
    /// call tells by itself: AK for Alaska's prefixes, PAC for Hawaii's and
    /// the Pacific islands', PR and VI for the Caribbean islands', NS for
    /// Sable and St Paul Islands', and DX outside the sections. Empty while
    /// a call in the US's districts or Canada has none, as its section goes
    /// by where in them it is, or one Dust cannot place, or no call reads.
    pub fn section_sent(&self) -> &str {
        if self.my_section.is_empty() {
            call_section(&self.my_call).unwrap_or_default()
        } else {
            fold_yukon(&self.my_section)
        }
    }

    /// What a run of these settings still needs before it can start, said
    /// for the screen; None once it has everything: a callsign Dust can
    /// read, and your name, your state, your zone and your section where the
    /// contest sends them. Your power and your class are always given.
    pub fn missing(&self) -> Option<String> {
        let kind = self.contest_kind;
        let rules = kind.rules();
        if self.my_call.is_empty() {
            return Some("Add your callsign to start a contest".to_string());
        }
        if parse_callsign(&self.my_call).is_none() {
            return Some(format!("{} is not a callsign Dust can read", self.my_call));
        }
        // The contest's name kept on one line: "an NA Sprint".
        let a_run = format!("{} {}", kind.article(), kind.label().replace(' ', "\u{a0}"));
        let side = self.side();
        if rules.sends_name() && self.my_name.is_empty() {
            return Some(format!("Add your name to start {a_run}"));
        }
        if rules.sends_state(side) && self.state_sent().is_empty() {
            return Some(format!("Add your state or province to start {a_run}"));
        }
        if rules.sends_zone(side) && self.zone().is_none() {
            return Some(format!("Add your CQ zone to start {a_run}"));
        }
        if rules.sends_section(side) && self.section_sent().is_empty() {
            return Some(format!("Add your section to start {a_run}"));
        }
        None
    }

    /// Fill in with stand-ins what of your station a run still needs, as
    /// Hear a QSO plays it before you have given it, and say what your
    /// station is then, if any of it stands in: `DL1ABC`, `K1ABC MIKE MA`,
    /// `C31AB zone 14`, `K1ABC 1D EMA`.
    /// It answers each thing [`Self::missing`] asks for, beside it so that
    /// the two change together.
    pub(crate) fn stand_in(&mut self) -> Option<String> {
        let rules = self.contest_kind.rules();
        let mut stood_in = false;
        if parse_callsign(&self.my_call).is_none() {
            // A North American call for a contest whose every side sends
            // your state or your section, as its callers are there.
            let (dx, home) = STAND_IN_CALL;
            let home_only = (rules.sends_state(Side::Dx) && rules.sends_state(Side::WVe))
                || (rules.sends_section(Side::Dx) && rules.sends_section(Side::WVe));
            self.my_call = if home_only { home } else { dx }.to_string();
            stood_in = true;
        }
        let side = self.side();
        if rules.sends_name() && self.my_name.is_empty() {
            self.my_name = STAND_IN_NAME.to_string();
            stood_in = true;
        }
        if rules.sends_state(side) && self.state_sent().is_empty() {
            self.my_state = STAND_IN_STATE.to_string();
            stood_in = true;
        }
        // Your zone stands in only for a call that does not say it; your
        // power is always given.
        let zone_stood_in = rules.sends_zone(side) && self.zone().is_none();
        if zone_stood_in {
            self.my_zone = STAND_IN_ZONE;
            stood_in = true;
        }
        // Your class is always given; your section stands in where your
        // call does not tell it.
        if rules.sends_section(side) && self.section_sent().is_empty() {
            self.my_section = STAND_IN_SECTION.to_string();
            stood_in = true;
        }
        if !stood_in {
            return None;
        }
        let mut station = vec![self.my_call.clone()];
        if rules.sends_name() {
            station.push(self.my_name.clone());
        }
        if rules.sends_state(side) {
            station.push(self.state_sent().to_string());
        }
        if zone_stood_in {
            station.push(format!("zone {STAND_IN_ZONE}"));
        }
        if rules.sends_class(side) {
            station.push(self.my_class.clone());
        }
        if rules.sends_section(side) {
            station.push(self.section_sent().to_string());
        }
        Some(station.join(" "))
    }
}

impl TrainingSettings {
    /// The speed you send at in a run, in words a minute: the one you set,
    /// or else the middle of the callers' character speed range, as before
    /// you set one.
    pub fn my_wpm(&self) -> f64 {
        match self.contest.my_wpm {
            0 => (self.playback.char_wpm_min + self.playback.char_wpm_max) / 2.0,
            wpm => f64::from(wpm.clamp(MY_WPM_MIN, MY_WPM_MAX)),
        }
    }

    /// Run contests in `style`. A pile-up of one caller is none at all, so
    /// picking one while the Band card's stations range holds a single
    /// station opens the range to one to three callers, as MorseRunner's
    /// pile-up starts busier than its single calls (`Ini.pas:261`); a range
    /// you have set is left as it is.
    pub fn set_contest_style(&mut self, style: RunStyle) {
        self.contest.contest_style = style;
        if style == RunStyle::Pileup && self.band.stations_max <= STATIONS_MIN {
            self.band.stations_max = PILEUP_CALLERS_MAX;
        }
    }
}

/// `text` as a name is sent: upper case letters, no longer than
/// [`MY_NAME_MAX`]. Anything else, spaces and accents among them, goes.
pub(super) fn clean_name(text: &str) -> String {
    text.chars()
        .map(|c| c.to_ascii_uppercase())
        .filter(char::is_ascii_uppercase)
        .take(MY_NAME_MAX)
        .collect()
}

/// The letters of `text`, upper case, and nothing else: a code as typed,
/// with dots, spaces and digits among it dropped.
fn typed_letters(text: &str) -> String {
    text.chars()
        .map(|c| c.to_ascii_uppercase())
        .filter(char::is_ascii_uppercase)
        .collect()
}

/// `text`, upper case, without its spaces: a value as typed, `1 d` as `1D`.
fn typed_compact(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// What `text`, typed as your state, province or territory, reads as: its
/// code, upper case, an old one as the new (PQ is QC, NF is NL), LB for
/// Labrador, or DX. None for anything else, a name written out among it.
pub fn state_code(text: &str) -> Option<&'static str> {
    place_code(&typed_letters(text))
}

/// `text` as a state, province or territory is kept: its code
/// ([`state_code`]), or nothing.
pub(super) fn clean_state(text: &str) -> String {
    state_code(text).unwrap_or_default().to_string()
}

/// What `text`, typed as your Field Day class, reads as: a number of
/// transmitters from 1 to 99 without leading zeros, and a letter from A to
/// F (`03a` is `3A`). None for anything else.
pub fn class_reading(text: &str) -> Option<String> {
    Field::Cl.canonical(&typed_compact(text))
}

/// `text` as your class is kept: [`class_reading`], or 1D.
pub(super) fn clean_class(text: &str) -> String {
    class_reading(text).unwrap_or_else(|| MY_CLASS_DEFAULT.to_string())
}

/// What `text`, typed as your section, reads as: its code, upper case, one
/// of the 85, Yukon's YT, or DX. None for anything else, a name written out
/// among it.
pub fn section_code(text: &str) -> Option<&'static str> {
    listed_section(&typed_letters(text))
}

/// `text` as a section is kept: its code ([`section_code`]), or nothing.
pub(super) fn clean_section(text: &str) -> String {
    section_code(text).unwrap_or_default().to_string()
}

/// What `text`, typed as your power, reads as: watts from 1 to
/// [`MY_POWER_MAX`], without leading zeros, or a kilowatt as written, `K`,
/// `KW` or `1K`. None for anything else.
pub fn power_reading(text: &str) -> Option<String> {
    let typed = typed_compact(text);
    if KILOWATT_SPELLINGS.contains(&typed.as_str()) {
        return Some(typed);
    }
    if !typed.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let watts: u32 = typed.parse().ok()?;
    (1..=MY_POWER_MAX)
        .contains(&watts)
        .then(|| watts.to_string())
}

/// `text` as your power is kept: [`power_reading`], or 100 W.
pub(super) fn clean_power(text: &str) -> String {
    power_reading(text).unwrap_or_else(|| MY_POWER_DEFAULT.to_string())
}

/// `text` as a callsign can be written: upper case, letters, digits and
/// slashes, with no slash at either end or next to another, and no longer
/// than [`MY_CALL_MAX`].
pub(super) fn clean_call(text: &str) -> String {
    let mut call = String::new();
    for c in text.chars().map(|c| c.to_ascii_uppercase()) {
        let slash_inside = c == '/' && !call.is_empty() && !call.ends_with('/');
        if c.is_ascii_uppercase() || c.is_ascii_digit() || slash_inside {
            call.push(c);
        }
    }
    // Every character kept is ASCII, so this cuts between characters.
    call.truncate(MY_CALL_MAX);
    while call.ends_with('/') {
        call.pop();
    }
    call
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_kept_as_a_call_can_be_written() {
        assert_eq!(clean_call("dl1abc"), "DL1ABC");
        assert_eq!(clean_call(" dl/ w1aw "), "DL/W1AW");
        assert_eq!(clean_call("/W1AW//P/"), "W1AW/P");
        assert_eq!(clean_call("K5-XYZ!é"), "K5XYZ");
        assert_eq!(clean_call("VP2E/DL1ABC/P"), "VP2E/DL1ABC");
        assert_eq!(clean_call("ABCDEFGHIJK/L"), "ABCDEFGHIJK");
        assert_eq!(clean_call("ABCDEFGHIJKLMNOP"), "ABCDEFGHIJKL");
        assert_eq!(clean_call("///"), "");
        for text in ["/W1AW//P/", "ABCDEFGHIJK/L", "a/b/c/d/e/f/g/h", "é/1"] {
            let once = clean_call(text);
            assert_eq!(clean_call(&once), once, "{text:?} settles in one pass");
        }
        let mut settings = ContestSettings::default();
        settings.set_my_call(" dl1abc/p ");
        assert_eq!(settings.my_call, "DL1ABC/P");
    }

    /// Picking a pile-up on a stations range of one opens it to one to
    /// three callers; a range already set is kept, and so is the range when
    /// a single caller is picked again.
    #[test]
    fn a_pile_up_picked_on_a_range_of_one_opens_it() {
        let mut settings = TrainingSettings::default();
        settings.set_contest_style(RunStyle::Pileup);
        assert_eq!(settings.contest.contest_style, RunStyle::Pileup);
        assert_eq!(
            (settings.band.stations_min, settings.band.stations_max),
            (1, 3)
        );
        settings.set_contest_style(RunStyle::Single);
        assert_eq!(settings.contest.contest_style, RunStyle::Single);
        assert_eq!(
            (settings.band.stations_min, settings.band.stations_max),
            (1, 3)
        );
        settings.band.stations_min = 2;
        settings.band.stations_max = 2;
        settings.set_contest_style(RunStyle::Pileup);
        assert_eq!(
            (settings.band.stations_min, settings.band.stations_max),
            (2, 2)
        );
    }

    /// A name is kept as it is sent: letters only, upper case, ten at most.
    #[test]
    fn a_name_is_kept_as_it_is_sent() {
        assert_eq!(clean_name("mat"), "MAT");
        assert_eq!(clean_name(" Jean-Luc "), "JEANLUC");
        assert_eq!(clean_name("Jürgen 2"), "JRGEN");
        assert_eq!(clean_name("Maximilianus"), "MAXIMILIAN");
        assert_eq!(clean_name("123 ?"), "");
        for text in ["Jean-Luc", "Maximilianus", "é1a"] {
            let once = clean_name(text);
            assert_eq!(clean_name(&once), once, "{text:?} settles in one pass");
            assert!(once.len() <= MY_NAME_MAX);
        }
        let mut settings = ContestSettings::default();
        settings.set_my_name(" bob ");
        assert_eq!(settings.my_name, "BOB");
    }

    /// A state is kept as its code: a valid one, an old one as the new, DX,
    /// or nothing.
    #[test]
    fn a_state_is_kept_as_its_code() {
        assert_eq!(clean_state("ma"), "MA");
        assert_eq!(clean_state(" o.r "), "OR");
        assert_eq!(clean_state("PQ"), "QC");
        assert_eq!(clean_state("nf"), "NL");
        assert_eq!(clean_state("dx"), "DX");
        assert_eq!(clean_state("lb"), "LB", "Labrador, for ARRL DX");
        for none in ["", "XX", "Mass", "LBX", "ORE", "D X X"] {
            assert_eq!(clean_state(none), "", "{none:?}");
            assert_eq!(state_code(none), None, "{none:?}");
        }
        assert_eq!(state_code(" o.r "), Some("OR"));
        for text in ["ma", "PQ", "Mass", "dx"] {
            let once = clean_state(text);
            assert_eq!(clean_state(&once), once, "{text:?} settles in one pass");
        }
        let mut settings = ContestSettings::default();
        settings.set_my_state(" bc ");
        assert_eq!(settings.my_state, "BC");
        settings.set_my_state("Mass");
        assert_eq!(settings.my_state, "", "no state, rather than a wrong one");
    }

    /// Outside the US and Canada your state is DX until you give one; in
    /// them, Alaska and Hawaii too, it is what you give, as it is for a call
    /// Dust cannot place, which may be in them.
    #[test]
    fn your_state_is_dx_outside_the_us_and_canada() {
        let mut settings = ContestSettings::default();
        for (call, sent) in [
            ("DL1ABC", "DX"),
            ("VP2E/DL1ABC", "DX"),
            ("KP4AB", "DX"),
            ("KH2AB", "DX"),
            ("DL/K1ABC", "DX"),
            ("K1ABC", ""),
            ("VE3ABC", ""),
            ("VG3ABC", ""),
            ("CF3ABC", ""),
            ("KL7AB", ""),
            ("WL7AB", ""),
            ("KH6AB", ""),
            ("C31AB", ""),
            ("ABC", ""),
            ("", ""),
        ] {
            settings.my_call = call.to_string();
            assert_eq!(settings.state_sent(), sent, "{call:?}");
        }
        settings.my_call = "DL1ABC".to_string();
        settings.my_state = "MA".to_string();
        assert_eq!(settings.state_sent(), "MA", "what you give is sent");
    }

    /// A run starts only with a call that reads as one, and your name where
    /// the contest sends it, and otherwise says what it is missing.
    #[test]
    fn a_run_says_what_it_needs_before_it_can_start() {
        let mut settings = ContestSettings::default();
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your callsign to start a contest")
        );
        settings.my_call = "DL1ABC".to_string();
        assert_eq!(settings.missing(), None);
        settings.my_call = "ABC".to_string();
        assert_eq!(
            settings.missing().as_deref(),
            Some("ABC is not a callsign Dust can read")
        );
        settings.my_call = "DL1ABC".to_string();
        settings.contest_kind = ContestKind::SimpleQso;
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your name to start a Simple\u{a0}QSO")
        );
        settings.my_name = "MAT".to_string();
        assert_eq!(settings.missing(), None);
        // WPX sends no name, so it needs none.
        settings.my_name.clear();
        settings.contest_kind = ContestKind::Wpx;
        assert_eq!(settings.missing(), None);
        // The Sprint sends your name and your state: a call in the US asks
        // for both, one outside it only for the name, its state being DX.
        settings.contest_kind = ContestKind::Sprint;
        settings.my_call = "K1ABC".to_string();
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your name to start an NA\u{a0}Sprint")
        );
        settings.my_name = "MIKE".to_string();
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your state or province to start an NA\u{a0}Sprint")
        );
        settings.my_state = "MA".to_string();
        assert_eq!(settings.missing(), None);
        settings.my_state.clear();
        settings.my_call = "DL1ABC".to_string();
        assert_eq!(settings.missing(), None, "DX, outside North America");
        // CQ WW takes your zone from your call, and asks for it only of a
        // call Dust cannot place.
        settings.contest_kind = ContestKind::Cqww;
        assert_eq!(settings.missing(), None);
        settings.my_call = "C31AB".to_string();
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your CQ zone to start a CQ\u{a0}WW")
        );
        settings.my_zone = 14;
        assert_eq!(settings.missing(), None);
        // ARRL DX: W/VE sends a state, DX its power, which is always given.
        settings.contest_kind = ContestKind::ArrlDx;
        settings.my_call = "K1ABC".to_string();
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your state or province to start an ARRL\u{a0}DX")
        );
        settings.my_call = "DL1ABC".to_string();
        assert_eq!(settings.missing(), None);
        settings.my_call = "KH6AB".to_string();
        assert_eq!(settings.missing(), None, "Hawaii is DX");
        // Field Day sends your class, always given, and your section: a
        // call in the US's districts or Canada asks for it, one that tells
        // it does not.
        settings.contest_kind = ContestKind::FieldDay;
        assert_eq!(settings.missing(), None, "Hawaii is PAC");
        settings.my_call = "K1ABC".to_string();
        settings.my_state = "MA".to_string();
        assert_eq!(
            settings.missing().as_deref(),
            Some("Add your section to start a Field\u{a0}Day")
        );
        settings.my_section = "EMA".to_string();
        assert_eq!(settings.missing(), None);
        settings.my_section.clear();
        settings.my_call = "DL1ABC".to_string();
        assert_eq!(settings.missing(), None, "DX");
        settings.my_call = "C31AB".to_string();
        assert!(settings.missing().is_some(), "a call Dust cannot place");
    }

    /// Your section is the one you give, Yukon's as the Territories'; or
    /// else the one your call tells by itself, for every prefix that tells
    /// one; and none for a call in the US's districts or Canada.
    #[test]
    fn your_section_is_given_or_told_by_your_call() {
        let mut settings = ContestSettings::default();
        for (call, sent) in [
            ("KL7AB", "AK"),
            ("AL7AB", "AK"),
            ("NL7AB", "AK"),
            ("WL7AB", "AK"),
            ("KH6AB", "PAC"),
            ("KH7AB", "PAC"),
            ("NH6AB", "PAC"),
            ("KH0AB", "PAC"),
            ("KH2AB", "PAC"),
            ("KH8AB", "PAC"),
            ("AH2AB", "PAC"),
            ("KP3AB", "PR"),
            ("KP4AB", "PR"),
            ("WP4AB", "PR"),
            ("KP2AB", "VI"),
            ("NP2AB", "VI"),
            ("CY0AA", "NS"),
            ("CY9AA", "NS"),
            ("DL1ABC", "DX"),
            ("VP2E/DL1ABC", "DX"),
            ("XE1ABC", "DX"),
            ("KH9AB", "DX"),
            ("KP1AB", "DX"),
            ("KG4AB", "DX"),
            ("K1ABC", ""),
            ("W1AW/4", ""),
            ("VE3ABC", ""),
            ("VY1AB", ""),
            ("C31AB", ""),
            ("ABC", ""),
            ("", ""),
        ] {
            settings.my_call = call.to_string();
            assert_eq!(settings.section_sent(), sent, "{call:?}");
        }
        settings.my_call = "KH6AB".to_string();
        settings.my_section = "EMA".to_string();
        assert_eq!(settings.section_sent(), "EMA", "what you give is sent");
        settings.my_section = "YT".to_string();
        assert_eq!(settings.section_sent(), "TER", "Yukon is the Territories");
    }

    /// A class is kept as a class: a number of transmitters and a letter,
    /// leading zeros dropped, and anything else as 1D.
    #[test]
    fn a_class_is_kept_as_a_class() {
        for (typed, kept) in [
            ("1d", "1D"),
            (" 3 A ", "3A"),
            ("03A", "3A"),
            ("99F", "99F"),
            ("10a", "10A"),
        ] {
            assert_eq!(class_reading(typed).as_deref(), Some(kept), "{typed:?}");
            assert_eq!(clean_class(typed), kept, "{typed:?}");
        }
        for none in ["", "D", "0D", "1G", "100A", "AD", "1", "1DD", "TA"] {
            assert_eq!(class_reading(none), None, "{none:?}");
            assert_eq!(clean_class(none), "1D", "{none:?}");
        }
        for text in ["1d", "03A", "1G"] {
            let once = clean_class(text);
            assert_eq!(clean_class(&once), once, "{text:?} settles in one pass");
        }
        let mut settings = ContestSettings::default();
        assert_eq!(settings.my_class, "1D");
        settings.set_my_class("2a");
        assert_eq!(settings.my_class, "2A");
    }

    /// A section is kept as its code: one of the 85, Yukon's YT, DX, or
    /// nothing; no other alias.
    #[test]
    fn a_section_is_kept_as_its_code() {
        for (typed, kept) in [
            ("ema", "EMA"),
            (" o.r ", "OR"),
            ("yt", "YT"),
            ("dx", "DX"),
            ("Ter", "TER"),
            ("nfl", "NFL"),
        ] {
            assert_eq!(section_code(typed), Some(kept), "{typed:?}");
            assert_eq!(clean_section(typed), kept, "{typed:?}");
        }
        for none in ["", "Mass", "NF", "YUK", "MAR", "GTA", "NT", "MD", "PQ"] {
            assert_eq!(section_code(none), None, "{none:?}");
            assert_eq!(clean_section(none), "", "{none:?}");
        }
        for text in ["ema", "Mass", "yt"] {
            let once = clean_section(text);
            assert_eq!(clean_section(&once), once, "{text:?} settles in one pass");
        }
        let mut settings = ContestSettings::default();
        settings.set_my_section(" ons ");
        assert_eq!(settings.my_section, "ONS");
        settings.set_my_section("Ontario");
        assert_eq!(
            settings.my_section, "",
            "no section, rather than a wrong one"
        );
    }

    /// Your zone is the one you give, or else the one your call reads as:
    /// by your state in the US or Canada, else by where your call is.
    #[test]
    fn your_zone_is_given_or_read_from_your_call() {
        let mut settings = ContestSettings {
            my_call: "DL1ABC".to_string(),
            ..ContestSettings::default()
        };
        assert_eq!(
            (settings.zone_from_call(), settings.zone()),
            (Some(14), Some(14))
        );
        settings.my_zone = 15;
        assert_eq!(
            (settings.zone_from_call(), settings.zone()),
            (Some(14), Some(15))
        );
        settings.my_zone = 0;
        settings.my_call = "K7ABC".to_string();
        assert_eq!(settings.zone(), Some(3));
        assert_eq!(settings.zone_state(), None);
        settings.my_state = "MT".to_string();
        assert_eq!(settings.zone(), Some(4), "Montana is zone 4");
        assert_eq!(settings.zone_state(), Some("MT"), "not the call alone");
        settings.my_state = "WA".to_string();
        assert_eq!(settings.zone_state(), None, "the call's own zone");
        settings.my_call = "C31AB".to_string();
        settings.my_state.clear();
        assert_eq!(settings.zone(), None);
        // A zone typed is kept as a zone, and anything else as 0.
        for (typed, kept) in [
            (" 14 ", 14),
            ("40", 40),
            ("0", 0),
            ("41", 0),
            ("A4", 0),
            ("", 0),
        ] {
            settings.set_my_zone(typed);
            assert_eq!(settings.my_zone, kept, "{typed:?}");
        }
    }

    /// Your power is kept as watts from 1 to 1500 or a kilowatt as written,
    /// and anything else as 100 W.
    #[test]
    fn your_power_is_kept_as_watts_or_a_kilowatt() {
        for (typed, kept) in [
            ("100", "100"),
            (" 0100 ", "100"),
            ("5", "5"),
            ("1500", "1500"),
            ("1000", "1000"),
            ("k", "K"),
            ("kw", "KW"),
            ("1k", "1K"),
            (" k w ", "KW"),
        ] {
            assert_eq!(power_reading(typed).as_deref(), Some(kept), "{typed:?}");
            assert_eq!(clean_power(typed), kept, "{typed:?}");
        }
        for none in ["", "0", "1501", "1TT", "100W", "2KW", "MAX", "99999999999"] {
            assert_eq!(power_reading(none), None, "{none:?}");
            assert_eq!(clean_power(none), "100", "{none:?}");
        }
        for text in ["100", " 0100 ", "kw", "MAX"] {
            let once = clean_power(text);
            assert_eq!(clean_power(&once), once, "{text:?} settles in one pass");
        }
        let mut settings = ContestSettings::default();
        assert_eq!(settings.my_power, "100");
        settings.set_my_power("kw");
        assert_eq!(settings.my_power, "KW");
    }

    /// Labrador is sent as LB in ARRL DX, which counts it apart, and as NL
    /// in every other contest.
    #[test]
    fn labrador_is_sent_apart_only_in_arrl_dx() {
        let mut settings = ContestSettings {
            my_call: "VO2AB".to_string(),
            my_state: "LB".to_string(),
            contest_kind: ContestKind::ArrlDx,
            ..ContestSettings::default()
        };
        assert_eq!(settings.state_sent(), "LB");
        settings.contest_kind = ContestKind::Sprint;
        assert_eq!(settings.state_sent(), "NL");
        assert_eq!(settings.zone(), Some(2), "Labrador is zone 2");
    }

    /// Whatever of your station is given, the stand-ins leave nothing a
    /// run of any contest still needs, and stand in only where it needed
    /// something.
    #[test]
    fn a_station_stood_in_for_has_all_a_run_needs() {
        for kind in ContestKind::ALL {
            for call in ["", "ABC", "DL1ABC", "K1ABC", "VE3ABC/P", "C31AB", "KH6AB"] {
                for name in ["", "ANN"] {
                    for (state, section) in [("", ""), ("OR", ""), ("", "OR"), ("OR", "OR")] {
                        let given = ContestSettings {
                            contest_kind: kind,
                            my_call: call.to_string(),
                            my_name: name.to_string(),
                            my_state: state.to_string(),
                            my_section: section.to_string(),
                            ..ContestSettings::default()
                        };
                        let mut stood = given.clone();
                        let stand_in = stood.stand_in();
                        let case = format!("{kind:?} {call:?} {name:?} {state:?} {section:?}");
                        assert_eq!(stood.missing(), None, "{case}");
                        assert_eq!(stand_in.is_some(), given.missing().is_some(), "{case}");
                        if stand_in.is_none() {
                            assert_eq!(stood, given, "{case}");
                        }
                    }
                }
            }
        }
        // Field Day stands in a North American call and its section.
        let mut field_day = ContestSettings {
            contest_kind: ContestKind::FieldDay,
            ..ContestSettings::default()
        };
        assert_eq!(field_day.stand_in().as_deref(), Some("K1ABC 1D EMA"));
        let mut hawaii = ContestSettings {
            contest_kind: ContestKind::FieldDay,
            my_call: "KH6AB".to_string(),
            my_class: "2A".to_string(),
            ..ContestSettings::default()
        };
        assert_eq!(hawaii.stand_in(), None, "its call tells its section");
    }

    /// Your sending is the monitor's level turned down by its decibels:
    /// the loudest at 0 dB, half as loud in amplitude 6 dB down, and
    /// nothing at all at the bottom.
    #[test]
    fn your_sending_turns_the_monitor_down() {
        let mut settings = ContestSettings::default();
        assert_eq!(settings.my_sending_db, -3);
        let level = |db: i32| {
            ContestSettings {
                my_sending_db: db,
                ..ContestSettings::default()
            }
            .sending_level()
        };
        assert!((level(0) - MONITOR_LEVEL).abs() < 1e-12);
        assert!((level(-6) / MONITOR_LEVEL - 0.501).abs() < 1e-3);
        assert!((level(-9) / MONITOR_LEVEL - 0.355).abs() < 1e-3);
        assert!(level(-57) > 0.0, "quiet, but heard");
        assert_eq!(level(MY_SENDING_OFF_DB), 0.0);
        assert_eq!(level(-200), 0.0);
        assert!(
            (level(12) - MONITOR_LEVEL).abs() < 1e-12,
            "never past the top"
        );
        settings.my_sending_db = MY_SENDING_OFF_DB;
        assert_eq!(settings.sending_level(), 0.0);
        // At full volume the top is where the limiter starts to bend, and
        // no further.
        let heard = level(MY_SENDING_MAX_DB);
        assert!(
            (heard - crate::band::SOFT_LIMIT_KNEE).abs() < 1e-12,
            "{heard}"
        );
    }

    /// Your speed is the one you set; until then, the middle of the
    /// callers' character speed range, which follows the range.
    #[test]
    fn your_speed_is_yours_or_the_callers_middle() {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 18.0;
        settings.playback.char_wpm_max = 25.0;
        assert_eq!(settings.contest.my_wpm, 0);
        assert_eq!(settings.my_wpm(), 21.5);
        settings.playback.char_wpm_max = 31.0;
        assert_eq!(settings.my_wpm(), 24.5, "it follows the range");
        settings.contest.my_wpm = 35;
        assert_eq!(settings.my_wpm(), 35.0);
        settings.playback.char_wpm_max = 40.0;
        assert_eq!(settings.my_wpm(), 35.0, "set, it stays");
        settings.contest.my_wpm = 900;
        assert_eq!(
            settings.my_wpm(),
            WPM_MAX,
            "held in the Speed card's bounds"
        );
    }

    /// Read back, your speed and your sending take any number, held to
    /// their ends later by the clamp, and anything else, a save edited by
    /// hand, as their defaults, costing the rest of the save nothing; your
    /// call in TU reads a name this build does not know as every QSO.
    #[test]
    fn your_speed_sending_and_tu_call_read_back_forgiving() {
        let read = |json: serde_json::Value| -> ContestSettings {
            serde_json::from_value(json).expect("the save reads")
        };
        let missing = read(serde_json::json!({ "myCall": "DL1ABC" }));
        assert_eq!(
            (
                missing.my_wpm,
                missing.my_sending_db,
                missing.contest_tu_call
            ),
            (0, -3, TuCall::Every)
        );
        assert_eq!(missing.my_call, "DL1ABC");
        let set = read(serde_json::json!({
            "myWpm": 32,
            "mySendingDb": -21,
            "contestTuCall": "never",
        }));
        assert_eq!(
            (set.my_wpm, set.my_sending_db, set.contest_tu_call),
            (32, -21, TuCall::Never)
        );
        let odd = read(serde_json::json!({
            "myWpm": "fast",
            "mySendingDb": null,
            "contestTuCall": "sometimes",
            "myCall": "K1ABC",
        }));
        assert_eq!(
            (odd.my_wpm, odd.my_sending_db, odd.contest_tu_call),
            (0, -3, TuCall::Every)
        );
        assert_eq!(odd.my_call, "K1ABC", "the rest of the save is kept");
        let wild = read(serde_json::json!({ "myWpm": -4.6, "mySendingDb": 1e12 }));
        assert_eq!((wild.my_wpm, wild.my_sending_db), (0, i32::MAX));
        let rounded = read(serde_json::json!({ "myWpm": 29.6, "mySendingDb": -8.6 }));
        assert_eq!((rounded.my_wpm, rounded.my_sending_db), (30, -9));
        let json = serde_json::to_value(ContestSettings::default()).expect("serialize");
        assert_eq!(json["contestTuCall"], "every");
        assert_eq!(json["myWpm"], 0);
        assert_eq!(json["mySendingDb"], -3);
    }

    #[test]
    fn an_open_run_ends_at_the_longest() {
        let mut settings = ContestSettings::default();
        assert_eq!(settings.limit_minutes(), 10);
        settings.contest_minutes = 0;
        assert_eq!(settings.limit_minutes(), CONTEST_MINUTES_MAX);
        settings.contest_minutes = 25;
        assert_eq!(settings.limit_minutes(), 25);
    }
}
