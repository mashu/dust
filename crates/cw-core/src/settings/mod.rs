//! Training settings for the group trainer.
//!
//! Every default is a decision about how the trainer behaves out of the box.
//! The structs spell theirs out in `Default` impls; the enums mark theirs with
//! `#[default]` on the variant, where it reads as part of the type.

use serde::{Deserialize, Serialize, Serializer};

use crate::keyer::KeyerMode;
use crate::level::LEVEL_MIN;

// The range each setting is kept in, declared once: `clamp` holds the
// settings to them and the settings screen draws its controls from them.

/// Groups in one session.
pub const NUM_GROUPS_MIN: u32 = 1;
pub const NUM_GROUPS_MAX: u32 = 200;
/// Characters in one group.
pub const GROUP_SIZE_MIN: u32 = 1;
pub const GROUP_SIZE_MAX: u32 = 20;
/// A group is always sent at least once.
pub const GROUP_REPEAT_MIN: u32 = 1;
/// Upper bound for "send the group N times before the answer window opens".
pub const GROUP_REPEAT_MAX: u32 = 8;
/// Speeds, in words a minute: character, effective and the paddles' own.
pub const WPM_MIN: f64 = 5.0;
pub const WPM_MAX: f64 = 80.0;
/// How far the word space is stretched beyond seven units.
pub const EXTRA_WORD_SPACE_MIN: f64 = 0.1;
pub const EXTRA_WORD_SPACE_MAX: f64 = 8.0;
/// The longest the answer window stays open; zero leaves it open.
pub const GROUP_TIMEOUT_MAX_SEC: f64 = 120.0;
/// The longest pause after a group before the next one.
pub const GROUP_PAUSE_MAX_SEC: f64 = 15.0;
/// The longest count down before a session's first group, in whole seconds.
pub const COUNTDOWN_MAX_SEC: u32 = 10;
/// The side tone: where on the dial a station can be heard.
pub const SIDE_TONE_MIN_HZ: f64 = 200.0;
pub const SIDE_TONE_MAX_HZ: f64 = 1_200.0;
/// A station's strength, as a share of the strongest: how far it stands
/// over the band's noise, which is not how loud the app plays.
pub const VOLUME_MIN: f64 = 0.1;
pub const VOLUME_MAX: f64 = 1.0;
/// How loud everything is played, as a share of full volume: the loudest
/// the receiver plays a station clean ([`crate::band::full_volume_gain`]).
pub const MASTER_VOLUME_MIN: f64 = 0.05;
pub const MASTER_VOLUME_MAX: f64 = 1.0;
/// The keying envelope's rise and fall, in milliseconds.
pub const STEEPNESS_MIN_MS: f64 = 1.0;
pub const STEEPNESS_MAX_MS: f64 = 50.0;
/// How fast the band fades, in fades a second.
pub const QSB_RATE_MIN_HZ: f64 = 0.03;
pub const QSB_RATE_MAX_HZ: f64 = 1.5;
/// Sessions above or below the threshold before the level moves.
pub const ADJUST_COUNT_MAX: u32 = 20;
/// The session accuracy, in percent, that counts as above the threshold.
/// Below half right is not a level anyone has mastered.
pub const ADJUST_THRESHOLD_MIN_PCT: f64 = 50.0;
pub const ADJUST_THRESHOLD_MAX_PCT: f64 = 100.0;
/// How hard the sampler leans on characters you get wrong, and on
/// characters it has not sent yet this session.
pub const ERROR_WEIGHT_MAX: f64 = 10.0;
pub const COVERAGE_STRENGTH_MAX: f64 = 8.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MixedAutoLevelAxis {
    #[default]
    Letters,
    Digits,
}

impl MixedAutoLevelAxis {
    #[must_use]
    pub fn flip(self) -> Self {
        match self {
            Self::Letters => Self::Digits,
            Self::Digits => Self::Letters,
        }
    }
}

/// The shape of the receiver's CW filter, as a modern rig offers it.
///
/// All of them are the same width between their 6 dB points, which is how a
/// rig's filter width is quoted; what differs is the shoulders. Sharp is flat
/// across the top with steep skirts, and rings audibly on noise and on every
/// element when it is narrow. Soft is rounded, lets more through either side,
/// and barely rings at all. These are the SHARP and SOFT filter shapes of
/// Icom's DSP receivers. Peaked is Sharp with the rig's audio peak filter
/// switched in as well: a resonance at the pitch that makes the hiss sing at
/// the note and leaves a station off the pitch a few decibels weaker.
///
/// Sharp is the default: flat across the pitch range a station is drawn
/// from, so every station comes through at its own strength. The peak
/// filter, which was, left one at the ends of the default range nearly 6 dB
/// down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterShape {
    Soft,
    #[default]
    Sharp,
    Peaked,
}

/// How fast the receiver's AGC lets the gain back up once a signal has
/// gone, as a rig's AGC switch offers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgcDecay {
    /// Holds across the gaps inside a word and lets the band back in a word
    /// gap: what most CW operators run.
    #[default]
    Fast,
    /// Holds across word gaps too and comes back over a second or so: the
    /// band stays steady under a station, and a weak one right after a strong
    /// one comes up slowly.
    Slow,
}

impl<'de> Deserialize<'de> for AgcDecay {
    /// By name, anything this version does not know coming back as Fast,
    /// rather than costing the rest of the save.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(match name.as_str() {
            "slow" => Self::Slow,
            _ => Self::Fast,
        })
    }
}

impl<'de> Deserialize<'de> for FilterShape {
    /// By name. Anything this version does not know — a shape a later version
    /// added, or a save edited by hand — comes back as Sharp, the plain IF
    /// filter every shape is built on, rather than costing the rest of the
    /// save.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(match name.as_str() {
            "soft" => Self::Soft,
            "peaked" => Self::Peaked,
            _ => Self::Sharp,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CharSetMode {
    /// Characters unlocked a level at a time, in the order chosen: one of
    /// the presets, or the learner's own. Before an order could be their
    /// own, a list of their own was a character set of its own, Custom;
    /// sessions saved on it read back as this.
    #[serde(alias = "custom")]
    Koch,
    Digits,
    #[default]
    Mixed,
    /// Realistic amateur callsigns instead of drawn groups. The characters are
    /// not what progresses here — the shape of the call is.
    Callsign,
    /// Whole words from a list of the learner's own, each sent as a group.
    /// There is no level to climb: the list is the curriculum.
    Words,
    /// Contest runs: callers answer your CQ and you copy their calls and
    /// exchanges. Not offered by the settings' control yet. It shares the
    /// callsign tier with Callsign, which shapes the callers' calls. On disk
    /// it is written as Callsign with a mark beside it ([`mode_on_disk`]),
    /// so a build that knows no contests still reads a run, and the
    /// settings.
    Contest,
}

/// A mode as it is written to disk: Contest as Callsign, so a build that
/// knows no contests reads a run as the callsign practice it is, and opens
/// the settings in Callsigns at the same tier, where a mode name it did not
/// know would have cost the whole session or every setting. A mark beside it
/// says it was Contest: a run's fingerprint, the settings' `contestActive`.
pub(crate) fn mode_on_disk<S: Serializer>(
    mode: &CharSetMode,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match mode {
        CharSetMode::Contest => CharSetMode::Callsign.serialize(serializer),
        mode => mode.serialize(serializer),
    }
}

impl CharSetMode {
    /// Every character set, in the order the settings list them.
    pub const ALL: [Self; 6] = [
        Self::Koch,
        Self::Digits,
        Self::Mixed,
        Self::Callsign,
        Self::Words,
        Self::Contest,
    ];
}

impl CharSetMode {
    /// Whether a group is drawn a character at a time, at a length the group
    /// size gives it. Callsigns, words and contest calls are not: each is as
    /// long as it is.
    pub fn draws_characters(self) -> bool {
        !matches!(self, Self::Callsign | Self::Words | Self::Contest)
    }

    /// Whether what is sent is callsigns, shaped by the callsign tier: in
    /// Callsigns, and from a contest's callers.
    pub fn sends_callsigns(self) -> bool {
        matches!(self, Self::Callsign | Self::Contest)
    }
}

/// How much of what is unlocked a session draws from: all of it, or only
/// the newest few characters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PracticeWindow {
    #[default]
    All,
    Last3,
    Last5,
}

/// How hard the adaptive features push, group size and speed alike: the
/// share of groups to copy whole, and of characters to copy right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Aim {
    /// Mostly clean copy.
    Comfortable,
    /// About a third of the groups trip you up.
    #[default]
    Balanced,
    /// Right at your span.
    Stretch,
}

impl Aim {
    /// Every aim, in the order the settings screen offers them.
    pub const ALL: [Self; 3] = [Self::Comfortable, Self::Balanced, Self::Stretch];

    /// The share of groups to copy whole, from 0 to 1: what adaptive group
    /// size aims for.
    pub fn groups_whole(self) -> f64 {
        match self {
            Self::Comfortable => 0.85,
            Self::Balanced => 0.70,
            Self::Stretch => 0.50,
        }
    }

    /// The share of characters to copy right, from 0 to 1: what adaptive
    /// speed aims for. Over a group of five, each about matches the share
    /// of groups copied whole above (0.97⁵ ≈ 0.86, 0.94⁵ ≈ 0.73,
    /// 0.90⁵ ≈ 0.59).
    pub fn chars_right(self) -> f64 {
        match self {
            Self::Comfortable => 0.97,
            Self::Balanced => 0.94,
            Self::Stretch => 0.90,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CurriculumSettings {
    /// Progress through the letter alphabet (level 1 unlocks two characters).
    /// Saves from the first versions called it the Koch level.
    #[serde(alias = "kochLevel")]
    pub level: u32,
    #[serde(serialize_with = "mode_on_disk")]
    pub char_set_mode: CharSetMode,
    pub digits_level: u32,
    /// When mixed: 0–100 percent of characters that are letters (rest digits).
    pub mixed_letters_percent: u32,
    pub custom_sequence: Vec<char>,
    /// True when the user chose Unlock order → Your own, even if the order still matches a preset.
    pub sequence_is_custom: bool,
    pub practice_window: PracticeWindow,
    /// Callsign structure tier, the level of the callsign character set. Its own
    /// field, the way digits have one, so switching modes never reinterprets a
    /// number that meant something else.
    pub callsign_level: u32,
    /// The learner's own words, for Words: upper-cased, sendable, each once.
    /// Empty sends the starter list.
    pub word_list: Vec<String>,
    pub num_groups: u32,
    pub min_group_size: u32,
    pub max_group_size: u32,
    pub link_group_size: bool,
    /// Draw each group's length from what the history says you copy whole,
    /// inside the group size range, rather than evenly across it. Off unless
    /// asked for; older saves have no field and load with it off.
    pub adaptive_group_size: bool,
    /// What adaptive group size and adaptive speed aim for. Saved under the
    /// name it had when only group size adapted.
    #[serde(rename = "groupSizeTarget")]
    pub aim: Aim,
}

impl Default for CurriculumSettings {
    fn default() -> Self {
        Self {
            level: LEVEL_MIN,
            char_set_mode: CharSetMode::Mixed,
            digits_level: 1,
            mixed_letters_percent: 70,
            custom_sequence: Vec::new(),
            sequence_is_custom: false,
            practice_window: PracticeWindow::All,
            callsign_level: LEVEL_MIN,
            word_list: Vec::new(),
            num_groups: 20,
            min_group_size: 2,
            max_group_size: 3,
            link_group_size: false,
            adaptive_group_size: false,
            aim: Aim::Balanced,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaybackSettings {
    pub char_wpm_min: f64,
    pub char_wpm_max: f64,
    pub link_char_wpm: bool,
    pub effective_wpm_min: f64,
    pub effective_wpm_max: f64,
    pub link_effective_wpm: bool,
    pub link_char_to_effective: bool,
    /// Let the trainer choose each group's speed within the ranges above,
    /// learning your copy speed as it goes ([`crate::speed`]). Off draws the
    /// speed evenly from the ranges. Older saves have no field and load off.
    pub adaptive_speed: bool,
    pub extra_word_space_multiplier: f64,
    pub group_timeout: f64,
    /// Seconds to sit on the last group after it is sent, so the card can be
    /// read before the next one starts. Zero, the default, is contest pace.
    /// This is not Morse spacing — repeats of the same group still use a word
    /// space.
    pub group_pause_sec: f64,
    /// Seconds counted down, out loud on the screen, between Start training
    /// and the first group: time to get the phone in hand and the ear ready.
    /// Zero sends the first group at once.
    pub countdown_sec: u32,
    pub lock_input_during_group_playback: bool,
    /// `[` is dit and `]` is dah on a USB keyer. Set when the paddles are wired the other way.
    pub paddle_swap: bool,
    /// How the `[` / `]` paddles time elements. Iambic B unless chosen
    /// otherwise; older saves have no field and load at it too.
    pub keyer_mode: KeyerMode,
    /// Speed the paddles send at. Independent of the station's character
    /// speed, which can be a random range.
    pub keyer_wpm: f64,
    /// How far operators' fists stray from a keyer's. Zero is a band of
    /// machines, which is what this was before there was a setting.
    pub fist_variation: f64,
    /// Lowest number of times a group is sent before the answer window opens.
    pub group_repeat_min: u32,
    pub group_repeat_max: u32,
    pub link_group_repeat: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            char_wpm_min: 15.0,
            char_wpm_max: 30.0,
            link_char_wpm: false,
            effective_wpm_min: 15.0,
            effective_wpm_max: 30.0,
            link_effective_wpm: false,
            link_char_to_effective: true,
            adaptive_speed: false,
            extra_word_space_multiplier: 1.0,
            group_timeout: 10.0,
            group_pause_sec: 0.0,
            countdown_sec: 3,
            lock_input_during_group_playback: true,
            paddle_swap: false,
            keyer_mode: KeyerMode::IambicB,
            keyer_wpm: 20.0,
            fist_variation: 0.35,
            group_repeat_min: 1,
            group_repeat_max: 1,
            link_group_repeat: true,
        }
    }
}

/// The narrowest and widest the receiver's width control goes, in hertz
/// between the filter's 6 dB points. The bottom is the classic narrow CW
/// position; the top is wide open, where the filter stops being the thing you
/// notice.
pub const FILTER_WIDTH_MIN: f64 = 150.0;
pub const FILTER_WIDTH_MAX: f64 = 2_000.0;
/// The receiver's width out of the box: 300 Hz at 6 dB, a rig's usual CW
/// filter, narrow enough that the hiss rings at the note the way a real
/// receiver's does.
pub const DEFAULT_FILTER_WIDTH_HZ: f64 = 300.0;

/// How many stations can be calling at once. One is just the station you want,
/// on an otherwise empty band; above that the others are QRM. The count is
/// drawn fresh for each group so a pile-up never arrives the same way twice,
/// and the band's own traffic around it never has more than the rest of it
/// keying at once.
pub const STATIONS_MIN: u32 = 1;
pub const STATIONS_MAX: u32 = 5;
/// How far either side of the wanted station the others can land, in hertz.
/// The floor is above one critical band of hearing for a reason — see the
/// closest the pile-up places another station, in the timing module.
pub const PILEUP_SPREAD_MIN: f64 = 150.0;
pub const PILEUP_SPREAD_MAX: f64 = 800.0;
/// How far below the wanted station the others sit, in decibels. The floor is
/// what keeps it answerable: the one you want stays the strongest. It is not
/// far above zero because a station much more than ten decibels down, this
/// close in pitch, is simply masked rather than quiet.
pub const PILEUP_LEVEL_MIN_DB: f64 = 3.0;
pub const PILEUP_LEVEL_MAX_DB: f64 = 20.0;
/// How far over its lowest the AGC's threshold can be raised, in decibels.
/// The lowest is just above the band's own hiss; thirty over it, the default
/// station never reaches it.
pub const AGC_THRESHOLD_MIN_DB: f64 = 0.0;
pub const AGC_THRESHOLD_MAX_DB: f64 = 30.0;
/// How far the receiver's RF gain backs off from full, in decibels. Thirty
/// down, the default station reaches the AGC twenty under its lowest
/// threshold and the band's hiss is all but gone.
pub const RF_GAIN_MIN_DB: f64 = -30.0;
pub const RF_GAIN_MAX_DB: f64 = 0.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BandSettings {
    pub side_tone_min: f64,
    pub side_tone_max: f64,
    /// The strength of the station you are copying, drawn from this range
    /// for each group: how far it stands over the band's noise. How loud
    /// everything plays is [`Self::master_volume`].
    pub volume_min: f64,
    pub volume_max: f64,
    pub link_volume: bool,
    /// How loud everything is played, as a share of full volume: the band and
    /// the stations, after the AGC and before the limiter
    /// ([`TrainingSettings::af_gain`]), and your own messages and the
    /// paddle's sidetone with them. Full, out of the box, is as loud as this
    /// receiver plays the strongest station clean; older saves have no field
    /// and load at it.
    pub master_volume: f64,
    /// The keying envelope's rise and fall, in milliseconds. A rig keys in
    /// four to eight; ten sounded soft.
    pub steepness: f64,
    pub envelope_smoothing: f64,
    pub qsb_enabled: bool,
    pub qsb_depth: f64,
    pub qsb_rate_hz: f64,
    pub qrn_enabled: bool,
    pub qrn_level: f64,
    /// The band's noise floor: the steady hiss every receiver hears through
    /// its filter. Saves from before it was a noise floor called the same
    /// switch the receiver background, and before that `qrm`.
    #[serde(alias = "receiverEnabled", alias = "qrmEnabled")]
    pub noise_enabled: bool,
    /// How loud the noise floor is, held to whole decibels of
    /// signal-to-noise ([`crate::band::NOISE_LEVEL_PER_DB`]). Deliberately
    /// not read from the old receiver-background level: that drove a
    /// different sound entirely, and its value says nothing about how much
    /// hiss someone wants. Out of the box it is a subtle hiss, a decibel in
    /// from the quiet end, which the default receiver hears about 40 dB
    /// under the station.
    pub noise_level: f64,
    /// The receiver's RF gain, in decibels from full: how far everything
    /// reaching the AGC is turned down, the band's noise and static and the
    /// stations alike ([`TrainingSettings::rf_gain`]). Full, out of the box,
    /// is the receiver as calibrated; older saves have no field and load at
    /// it. The volume does not follow it: backed off, the band plays
    /// quieter, as a rig's does, and the AGC has less to ride.
    pub rf_gain_db: f64,
    /// The receiver's automatic gain control. Off is a receiver run on its
    /// RF gain, the way contesters often do.
    pub agc_enabled: bool,
    /// How fast the AGC lets the gain back up once a signal has gone. Older
    /// saves have no field and load at Fast, which is how the AGC ran.
    pub agc_decay: AgcDecay,
    /// Where the AGC starts to turn signals down, in decibels over the lowest
    /// it goes: how much stronger a signal has to be before the AGC acts
    /// ([`crate::band::receiver_knee`]). The lowest, out of the box, starts
    /// the AGC just above the band's own hiss; older saves have no field and
    /// load at it. The volume follows it, so raising it changes how hard the
    /// AGC works, not how loud the band plays.
    pub agc_threshold_db: f64,
    /// The receiver's selectivity: its width in hertz between its 6 dB
    /// points, the way a rig quotes a CW filter. Everything you hear goes
    /// through it — the Morse as much as the noise — so narrowing it does what
    /// narrowing a real filter does: less static gets through, the signal
    /// loses a little of its keying sidebands, and the filter rings longer.
    /// The receiver is built at least wide enough to pass the whole side-tone
    /// range, whatever is set here ([`crate::band::FilterDesign::from_settings`]).
    ///
    /// Older saves held a width at 3 dB, as `filterBandwidthHz`. That key is
    /// deliberately not read: almost every one of them is the old default
    /// rather than a choice, and its number meant something else, so they
    /// come back at the new default.
    pub filter_width_hz: f64,
    /// Sharp or soft shoulders on that filter, which is what sets how much it
    /// rings — or sharp with the audio peak filter behind it, ringing at the
    /// note.
    pub filter_shape: FilterShape,
    /// The fewest stations that can call at once, counting the one you want.
    ///
    /// Saves written before the pile-up became a range said only "up to this
    /// many", which is this floor left at one.
    pub stations_min: u32,
    /// The most that can call at once. With both ends at 1 there is no
    /// pile-up, which is where this starts: one station, fixed.
    pub stations_max: u32,
    /// How far either side of the wanted station the others can land.
    pub pileup_spread_hz: f64,
    /// How far below the wanted station the others sit.
    pub pileup_level_db: f64,
}

impl Default for BandSettings {
    fn default() -> Self {
        Self {
            side_tone_min: 300.0,
            side_tone_max: 600.0,
            volume_min: 0.7,
            volume_max: 1.0,
            link_volume: false,
            master_volume: MASTER_VOLUME_MAX,
            steepness: 6.0,
            envelope_smoothing: 0.75,
            qsb_enabled: true,
            qsb_depth: 0.35,
            qsb_rate_hz: 0.12,
            qrn_enabled: true,
            qrn_level: 0.25,
            noise_enabled: true,
            noise_level: crate::band::NOISE_LEVEL_PER_DB,
            rf_gain_db: RF_GAIN_MAX_DB,
            agc_enabled: true,
            agc_decay: AgcDecay::Fast,
            agc_threshold_db: AGC_THRESHOLD_MIN_DB,
            filter_width_hz: DEFAULT_FILTER_WIDTH_HZ,
            filter_shape: FilterShape::default(),
            stations_min: STATIONS_MIN,
            stations_max: STATIONS_MIN,
            pileup_spread_hz: 350.0,
            pileup_level_db: 7.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoLevelSettings {
    pub mixed_auto_level_next_axis: MixedAutoLevelAxis,
    /// Saves from the first versions called it adjusting the Koch level.
    #[serde(alias = "autoAdjustKoch")]
    pub auto_adjust_level: bool,
    pub auto_adjust_threshold: f64,
    pub auto_adjust_below_threshold_count: u32,
    pub auto_adjust_above_threshold_count: u32,
    pub error_weight_strength: f64,
    pub char_sampling_coverage_strength: f64,
    pub char_sampling_thompson: bool,
}

impl Default for AutoLevelSettings {
    fn default() -> Self {
        Self {
            mixed_auto_level_next_axis: MixedAutoLevelAxis::Letters,
            auto_adjust_level: true,
            auto_adjust_threshold: 90.0,
            auto_adjust_below_threshold_count: 1,
            auto_adjust_above_threshold_count: 2,
            error_weight_strength: 3.0,
            char_sampling_coverage_strength: 1.0,
            char_sampling_thompson: false,
        }
    }
}

/// A card on the settings screen, as far as "put this back how it was" is
/// concerned.
///
/// Between them these cover every stored preference exactly once, which is
/// what `every_setting_belongs_to_exactly_one_section` holds them to: add a
/// field and forget to put it in a section and that test fails rather than the
/// reset button quietly missing it.
///
/// Progress is not a preference, so the level you have reached, what the
/// sampler has learned about you and your history are not in here and no
/// reset touches them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsSection {
    /// How many groups, how long, how often repeated.
    SessionShape,
    /// Character and effective speed, and the spacing between words; in a
    /// contest, your own speed too.
    Speed,
    /// Attack and decay of the keying.
    KeyingEnvelope,
    /// Which characters are practised, and how far into them you are; in
    /// a contest, which one, how it runs and how often your TU signs.
    CharacterSet,
    /// How loud everything plays, and the pitch and strength of the station
    /// you are copying; in a contest, how loud you hear your own sending,
    /// and whether the band is heard through it.
    ToneAndVolume,
    /// Fading, static, the noise floor, the filter, who else is calling,
    /// and in a contest how the callers operate and whether some flutter.
    BandConditions,
    /// Whether the trainer moves your level for you, and on what evidence.
    AutoLevel,
    /// How the next group is drawn from the characters you know.
    CharacterSampling,
    /// Your callsign, and the exchange you send in a contest.
    Station,
}

impl SettingsSection {
    /// Every section, for the tests that sweep them.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 9] = [
        Self::SessionShape,
        Self::Speed,
        Self::KeyingEnvelope,
        Self::CharacterSet,
        Self::ToneAndVolume,
        Self::BandConditions,
        Self::AutoLevel,
        Self::CharacterSampling,
        Self::Station,
    ];
}

/// The min/max pairs the settings screen edits. Keeping the invariants here —
/// max never below min, "linked" collapsing the pair, character speed dragging
/// effective speed along — keeps them testable and out of the UI's callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeSetting {
    CharWpm,
    EffectiveWpm,
    GroupSize,
    GroupRepeat,
    SideTone,
    Volume,
    Stations,
}

/// Current state of one range: the pair, and whether it is collapsed to a
/// single value rather than sampled per group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeValues {
    pub min: f64,
    pub max: f64,
    pub linked: bool,
}

/// Everything the trainer can be set to. Written to a save and read back
/// from one through [`save`], which also reads the older ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainingSettings {
    pub curriculum: CurriculumSettings,
    pub playback: PlaybackSettings,
    pub band: BandSettings,
    pub auto_level: AutoLevelSettings,
    pub contest: ContestSettings,
}

mod clamp;
mod contest;
mod contest_preset;
mod curriculum;
mod experience;
mod ranges;
mod reset;
mod save;

pub use contest::{
    CONTEST_MINUTES_MAX, ContestSettings, MY_CALL_MAX, MY_NAME_MAX, MY_POWER_MAX,
    MY_SENDING_MAX_DB, MY_SENDING_OFF_DB, MY_WPM_MAX, MY_WPM_MIN, MY_ZONE_MAX, class_reading,
    power_reading, section_code, state_code,
};
pub use contest_preset::ContestPreset;
pub(crate) use curriculum::LETTER_LEVEL_CEILING;
pub use curriculum::LevelAxis;
pub use experience::Experience;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_links_and_bounds() {
        let mut s = TrainingSettings::default();
        s.playback.char_wpm_min = 3.0;
        s.playback.char_wpm_max = 200.0;
        s.playback.link_char_wpm = true;
        s.playback.link_char_to_effective = true;
        s.curriculum.min_group_size = 8;
        s.curriculum.max_group_size = 2;
        s.curriculum.link_group_size = true;
        s.curriculum.level = 99;
        let s = s.clamp();
        assert_eq!(s.playback.char_wpm_min, 5.0);
        assert_eq!(s.playback.char_wpm_max, 5.0);
        assert_eq!(s.playback.effective_wpm_min, 5.0);
        assert_eq!(s.playback.effective_wpm_max, 5.0);
        assert_eq!(s.curriculum.min_group_size, 8);
        assert_eq!(s.curriculum.max_group_size, 8);
        assert_eq!(s.level(LevelAxis::Letters), s.max_level(LevelAxis::Letters));
    }

    #[test]
    fn paddle_speed_is_not_the_station_speed() {
        let mut s = TrainingSettings::default();
        s.playback.char_wpm_min = 18.0;
        s.playback.char_wpm_max = 40.0;
        s.playback.link_char_wpm = false;
        s.playback.keyer_wpm = 25.0;
        let s = s.clamp();
        assert_eq!(s.playback.keyer_wpm, 25.0);
        assert_eq!(s.playback.char_wpm_min, 18.0);
        assert_eq!(s.playback.char_wpm_max, 40.0);
        let mut s = s;
        s.playback.keyer_wpm = 900.0;
        let s = s.clamp();
        assert_eq!(s.playback.keyer_wpm, 80.0);
    }

    #[test]
    fn group_repeats_clamp_and_link() {
        let mut s = TrainingSettings::default();
        assert_eq!(s.playback.group_repeat_min, 1);
        assert_eq!(s.playback.group_repeat_max, 1);
        s.playback.link_group_repeat = false;
        s.playback.group_repeat_min = 0;
        s.playback.group_repeat_max = 99;
        let s = s.clamp();
        assert_eq!(s.playback.group_repeat_min, GROUP_REPEAT_MIN);
        assert_eq!(s.playback.group_repeat_max, GROUP_REPEAT_MAX);

        let mut linked = TrainingSettings::default();
        linked.playback.link_group_repeat = true;
        linked.playback.group_repeat_min = 3;
        linked.playback.group_repeat_max = 7;
        let linked = linked.clamp();
        assert_eq!(linked.playback.group_repeat_max, 3);

        let mut inverted = TrainingSettings::default();
        inverted.playback.link_group_repeat = false;
        inverted.playback.group_repeat_min = 4;
        inverted.playback.group_repeat_max = 2;
        let inverted = inverted.clamp();
        assert_eq!(inverted.playback.group_repeat_max, 4);
    }

    #[test]
    fn moving_a_bound_keeps_the_pair_ordered() {
        let mut s = TrainingSettings::default();
        s.playback.link_char_wpm = false;
        s.playback.link_char_to_effective = false;

        s.set_range_min(RangeSetting::CharWpm, 30.0);
        let range = s.range(RangeSetting::CharWpm);
        assert_eq!((range.min, range.max), (30.0, 30.0), "max follows min up");

        s.set_range_max(RangeSetting::CharWpm, 40.0);
        assert_eq!(s.range(RangeSetting::CharWpm).max, 40.0);

        s.set_range_max(RangeSetting::CharWpm, 12.0);
        let range = s.range(RangeSetting::CharWpm);
        assert_eq!((range.min, range.max), (12.0, 12.0), "min follows max down");
    }

    #[test]
    fn a_linked_range_moves_as_one() {
        let mut s = TrainingSettings::default();
        s.set_range_linked(RangeSetting::GroupRepeat, true);
        s.set_range_min(RangeSetting::GroupRepeat, 3.0);
        let range = s.range(RangeSetting::GroupRepeat);
        assert_eq!((range.min, range.max, range.linked), (3.0, 3.0, true));

        // Unlinking leaves the value where it was; the range opens by editing.
        s.set_range_linked(RangeSetting::GroupRepeat, false);
        s.set_range_max(RangeSetting::GroupRepeat, 5.0);
        let range = s.range(RangeSetting::GroupRepeat);
        assert_eq!((range.min, range.max, range.linked), (3.0, 5.0, false));
        assert_eq!(s.playback.group_repeat_min, 3);
        assert_eq!(s.playback.group_repeat_max, 5);
    }

    #[test]
    fn group_sizes_round_to_whole_characters() {
        let mut s = TrainingSettings::default();
        s.set_range_min(RangeSetting::GroupSize, 4.0);
        s.set_range_max(RangeSetting::GroupSize, 6.0);
        assert_eq!(s.curriculum.min_group_size, 4);
        assert_eq!(s.curriculum.max_group_size, 6);
        // Negative input from a text field cannot underflow the unsigned field.
        s.set_range_min(RangeSetting::GroupSize, -3.0);
        assert_eq!(s.curriculum.min_group_size, 0);
    }

    #[test]
    fn volume_keeps_its_own_link_flag() {
        let mut s = TrainingSettings::default();
        s.set_range_linked(RangeSetting::Volume, true);
        assert!(s.band.link_volume);
        s.set_range_min(RangeSetting::Volume, 0.5);
        assert_eq!(
            s.range(RangeSetting::Volume),
            RangeValues {
                min: 0.5,
                max: 0.5,
                linked: true
            }
        );
    }

    /// The volume is a share of full volume, at full out of the box and in
    /// a save from before it, and full lifts the band as it is calibrated;
    /// nothing set takes it past full or under its floor.
    #[test]
    fn the_volume_is_a_share_of_full_volume() {
        let mut s = TrainingSettings::default();
        let full = crate::band::full_volume_gain(&s);
        assert_eq!(s.band.master_volume, 1.0);
        assert_eq!(s.af_gain(), full);
        assert!(full > 1.0, "full is louder than the band as calibrated");
        s.band.master_volume = 0.5;
        assert_eq!((s.master_volume(), s.af_gain()), (0.5, 0.5 * full));
        for wild in [90.0, -9.0, 0.0, f64::NAN, f64::INFINITY] {
            s.band.master_volume = wild;
            let share = s.master_volume();
            assert!(
                (MASTER_VOLUME_MIN..=1.0).contains(&share),
                "{wild}: {share}"
            );
        }
        let old: TrainingSettings =
            serde_json::from_str(r#"{"volumeMin":0.5}"#).expect("an old save");
        assert_eq!(old.band.master_volume, 1.0);
    }

    /// The RF gain is decibels from full, at full out of the box and in a
    /// save from before it, and inside its range whatever is set; full
    /// volume is the receiver's at full RF gain, so backing it off leaves
    /// the AF gain where it was.
    #[test]
    fn the_rf_gain_is_decibels_from_full_and_full_volume_does_not_follow_it() {
        let mut s = TrainingSettings::default();
        assert_eq!((s.band.rf_gain_db, s.rf_gain()), (RF_GAIN_MAX_DB, 1.0));
        let af = s.af_gain();
        s.band.rf_gain_db = -20.0;
        assert!((s.rf_gain() - 0.1).abs() < 1e-12);
        assert_eq!(s.af_gain(), af);
        for (wild, reads) in [
            (-90.0, RF_GAIN_MIN_DB),
            (9.0, RF_GAIN_MAX_DB),
            (f64::NAN, RF_GAIN_MAX_DB),
        ] {
            s.band.rf_gain_db = wild;
            assert_eq!(s.rf_gain(), 10f64.powf(reads / 20.0), "{wild}");
        }
        let old: TrainingSettings =
            serde_json::from_str(r#"{"volumeMin":0.5}"#).expect("an old save");
        assert_eq!(old.band.rf_gain_db, RF_GAIN_MAX_DB);
    }

    #[test]
    fn a_level_is_read_inside_its_alphabet() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.custom_sequence = vec!['A', 'B', 'C', 'D'];
        s.curriculum.level = 99;
        let s = s.clamp();
        assert_eq!(s.active_level(), 3);
        assert_eq!(s.max_active_level(), 3);
        assert_eq!(s.progress_alphabet(), vec!['A', 'B', 'C', 'D']);
    }

    /// Koch and Mixed's letters share one level, and Mixed's alphabet is
    /// shorter by its digits. Every switch used to clamp the stored level to
    /// the alphabet of the moment, so one look at Mixed, or at a short
    /// unlock order, cost the Koch level for good.
    #[test]
    fn a_shorter_alphabet_does_not_cost_the_level_a_longer_one_reached() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.level = 38;
        let mut s = s.clamp();
        s.set_char_set_mode(CharSetMode::Mixed);
        let mut s = s.clamp();
        assert!(s.active_level() < 38, "Mixed has fewer letters to unlock");
        s.set_char_set_mode(CharSetMode::Koch);
        let s = s.clamp();
        assert_eq!(s.active_level(), 38);

        // A sequence shortened while it is being typed keeps it too.
        let mut s = TrainingSettings::default();
        s.curriculum.level = 9;
        s.curriculum.custom_sequence = vec!['K', 'M'];
        let s = s.clamp();
        assert_eq!(s.active_level(), 1);
        assert_eq!(s.curriculum.level, 9);
    }

    /// Putting the character set back is clamped with it: a level stored
    /// above what the default alphabet carries is read inside it at once,
    /// rather than waiting for the next pass to repair it.
    #[test]
    fn resetting_a_section_leaves_nothing_to_repair() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.custom_sequence = crate::sequences::ALPHABETICAL_SEQUENCE.to_vec();
        s.curriculum.level = 39;
        s.curriculum.practice_window = PracticeWindow::Last3;
        s.reset_section(SettingsSection::CharacterSet);
        assert_eq!(s.clone().clamp(), s);
        assert_eq!(s.curriculum.char_set_mode, CharSetMode::Mixed);
        assert_eq!(s.active_level(), s.max_active_level());
        assert_eq!(s.curriculum.practice_window, PracticeWindow::All);
    }

    #[test]
    fn clamping_cleans_the_alphabets_down_to_sendable_characters() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Koch;
        // Duplicates, lower case, spaces and unsendable characters all go.
        s.curriculum.custom_sequence = vec!['u', 'U', ' ', '#', 'r'];
        s.curriculum.digits_level = 99;
        let s = s.clamp();
        assert_eq!(s.curriculum.custom_sequence, vec!['U', 'R']);
        assert_eq!(s.curriculum.digits_level, crate::morse::MAX_DIGITS_LEVEL);
    }

    #[test]
    fn alphabet_fingerprint_tracks_sequence() {
        let mut lcwo = TrainingSettings::default();
        lcwo.curriculum.char_set_mode = CharSetMode::Koch;
        lcwo.curriculum.custom_sequence.clear();
        let mut mania = lcwo.clone();
        mania.curriculum.custom_sequence = crate::sequences::TRADITIONAL_KOCH_SEQUENCE.to_vec();
        assert_ne!(lcwo.alphabet_fingerprint(), mania.alphabet_fingerprint());
        let mut digits = TrainingSettings::default();
        digits.curriculum.char_set_mode = CharSetMode::Digits;
        assert_eq!(digits.alphabet_fingerprint(), "0123456789");
    }

    #[test]
    fn missing_json_fields_use_training_defaults() {
        let s: TrainingSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.curriculum.mixed_letters_percent, 70);
        assert!(s.playback.lock_input_during_group_playback);
        assert!(s.playback.link_char_to_effective);
        assert_eq!(s.playback.group_timeout, 10.0);
        assert_eq!(s.playback.group_pause_sec, 0.0);
        assert_eq!(s.playback.countdown_sec, 3);
        assert_eq!(s.auto_level.auto_adjust_threshold, 90.0);
        assert_eq!(s.curriculum.num_groups, 20);
        assert_eq!(s.curriculum.digits_level, 1);
        assert_eq!(s.playback.char_wpm_min, 15.0);
        assert_eq!(s.playback.group_repeat_min, 1);
        assert_eq!(s.playback.group_repeat_max, 1);
        assert!(s.playback.link_group_repeat);
        assert_eq!(s.playback.keyer_mode, crate::keyer::KeyerMode::IambicB);
        assert_eq!(s.playback.keyer_wpm, 20.0);
        let koch: TrainingSettings = serde_json::from_str(r#"{"charSetMode":"koch"}"#).unwrap();
        assert_eq!(koch.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(koch.curriculum.mixed_letters_percent, 70);
        assert!(koch.playback.lock_input_during_group_playback);
    }

    #[test]
    fn nested_settings_roundtrip_keeps_flat_keys() {
        let json = serde_json::to_value(TrainingSettings::default()).unwrap();
        assert!(json.get("numGroups").is_some());
        assert!(json.get("curriculum").is_none());
        assert!(json.get("charWpmMin").is_some());
        assert!(json.get("autoAdjustLevel").is_some());
        let back: TrainingSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back, TrainingSettings::default());
    }

    /// Every field pushed past both ends of its range, so one pass of `clamp`
    /// has something to do everywhere.
    fn wild() -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.curriculum.level = 999;
        s.curriculum.digits_level = 999;
        s.curriculum.mixed_letters_percent = 900;
        s.curriculum.num_groups = 9_000;
        s.curriculum.min_group_size = 99;
        s.curriculum.max_group_size = 0;
        s.curriculum.custom_sequence = vec!['k', 'k', ' ', 'm'];
        s.curriculum.callsign_level = 999;
        s.playback.char_wpm_min = 900.0;
        s.playback.char_wpm_max = -5.0;
        s.playback.keyer_wpm = 900.0;
        s.playback.effective_wpm_min = -1.0;
        s.playback.effective_wpm_max = 900.0;
        s.playback.extra_word_space_multiplier = -3.0;
        s.playback.group_timeout = 9_000.0;
        s.playback.group_pause_sec = 9_000.0;
        s.playback.countdown_sec = 9_000;
        s.playback.group_repeat_min = 99;
        s.playback.group_repeat_max = 0;
        s.band.side_tone_min = 9_000.0;
        s.band.side_tone_max = 1.0;
        s.band.volume_min = 9.0;
        s.band.volume_max = -9.0;
        s.band.master_volume = 900.0;
        s.band.steepness = 900.0;
        s.band.envelope_smoothing = 9.0;
        s.band.qsb_depth = 9.0;
        s.band.qsb_rate_hz = 9.0;
        s.band.qrn_level = 9.0;
        s.band.noise_level = 9.0;
        s.band.agc_threshold_db = 900.0;
        s.band.rf_gain_db = 900.0;
        s.band.filter_width_hz = 90_000.0;
        s.auto_level.auto_adjust_threshold = 900.0;
        s.auto_level.error_weight_strength = -1.0;
        s.auto_level.char_sampling_coverage_strength = -1.0;
        s.auto_level.auto_adjust_above_threshold_count = 900;
        s.auto_level.auto_adjust_below_threshold_count = 900;
        s.contest.contest_kind = crate::contest::ContestKind::Sprint;
        s.contest.contest_style = crate::contest::RunStyle::Pileup;
        s.contest.contest_habits = crate::contest::CallerHabits::Rough;
        s.contest.contest_flutter = true;
        s.contest.contest_minutes = 9_000;
        s.contest.contest_show_copy = true;
        s.contest.contest_tu_call = crate::contest::TuCall::Never;
        s.contest.contest_adapt_speed = false;
        s.contest.my_wpm = 9_000;
        s.contest.my_sending_db = 9_000;
        s.contest.contest_qsk = true;
        s.contest.my_call = "//dl1/ abc//qrp é/0123456789/".to_string();
        s.contest.my_name = " jean-luc maximilian ".to_string();
        s.contest.my_state = " p.q. ".to_string();
        s.contest.my_zone = 9_000;
        s.contest.my_power = "9000 watts".to_string();
        s.contest.my_class = " 0003a ".to_string();
        s.contest.my_section = "e.m.a.x".to_string();
        s
    }

    /// The app re-runs clamp on every settings change and writes the result
    /// back into the same signal. A second pass that moved anything would
    /// make that effect loop forever.
    #[test]
    fn clamping_reaches_a_fixed_point_in_one_pass() {
        for mode in CharSetMode::ALL {
            for linked in [false, true] {
                let mut first = wild();
                first.curriculum.char_set_mode = mode;
                first.curriculum.link_group_size = linked;
                first.playback.link_char_wpm = linked;
                first.playback.link_effective_wpm = linked;
                first.playback.link_char_to_effective = linked;
                first.playback.link_group_repeat = linked;
                first.band.link_volume = linked;
                let first = first.clamp();
                let second = first.clone().clamp();
                assert_eq!(first, second, "mode {mode:?}, linked {linked}");
            }
        }
    }

    /// `clamp` is the sanitiser everything else trusts, so nothing may make
    /// it panic — and NaN used to: a NaN minimum became the lower bound of the
    /// maximum's clamp, and `f64::clamp` panics on that.
    #[test]
    fn clamping_survives_numbers_that_are_not_numbers() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut s = wild();
            s.playback.char_wpm_min = bad;
            s.playback.char_wpm_max = bad;
            s.playback.effective_wpm_min = bad;
            s.playback.effective_wpm_max = bad;
            s.playback.extra_word_space_multiplier = bad;
            s.playback.group_timeout = bad;
            s.playback.group_pause_sec = bad;
            s.playback.keyer_wpm = bad;
            s.playback.fist_variation = bad;
            s.band.side_tone_min = bad;
            s.band.side_tone_max = bad;
            s.band.volume_min = bad;
            s.band.volume_max = bad;
            s.band.master_volume = bad;
            s.band.steepness = bad;
            s.band.envelope_smoothing = bad;
            s.band.qsb_depth = bad;
            s.band.qsb_rate_hz = bad;
            s.band.qrn_level = bad;
            s.band.noise_level = bad;
            s.band.filter_width_hz = bad;
            s.band.pileup_spread_hz = bad;
            s.band.pileup_level_db = bad;
            s.band.agc_threshold_db = bad;
            s.band.rf_gain_db = bad;
            s.auto_level.auto_adjust_threshold = bad;
            s.auto_level.error_weight_strength = bad;
            s.auto_level.char_sampling_coverage_strength = bad;
            let s = s.clamp();
            let json = serde_json::to_value(&s).expect("serialize");
            for (key, value) in json.as_object().expect("an object") {
                if let Some(number) = value.as_f64() {
                    assert!(number.is_finite(), "{key} came out {number} from {bad}");
                }
                // serde_json writes a non-finite float as null.
                assert!(!value.is_null(), "{key} came out null from {bad}");
            }
            assert_eq!(s.clone().clamp(), s, "and it still settles in one pass");
        }
    }

    #[test]
    fn clamping_puts_every_range_the_right_way_round() {
        let s = wild().clamp();
        assert!(s.playback.char_wpm_min <= s.playback.char_wpm_max);
        assert!(s.playback.effective_wpm_min <= s.playback.effective_wpm_max);
        assert!(s.band.side_tone_min <= s.band.side_tone_max);
        assert!(s.band.volume_min <= s.band.volume_max);
        assert!(s.curriculum.min_group_size <= s.curriculum.max_group_size);
        assert!(s.playback.group_repeat_min <= s.playback.group_repeat_max);
        assert_eq!(s.curriculum.mixed_letters_percent, 100);
        assert_eq!(s.curriculum.num_groups, 200);
        assert_eq!(s.playback.group_timeout, 120.0);
        assert_eq!(s.playback.group_pause_sec, 15.0);
        assert_eq!(s.playback.countdown_sec, COUNTDOWN_MAX_SEC);
        assert_eq!(s.playback.group_repeat_max, GROUP_REPEAT_MAX);
        assert_eq!(s.band.master_volume, MASTER_VOLUME_MAX);
        assert_eq!(s.band.steepness, 50.0);
        assert_eq!(s.band.envelope_smoothing, 1.0);
        assert_eq!(s.band.noise_level, 1.0);
        assert_eq!(s.band.agc_threshold_db, AGC_THRESHOLD_MAX_DB);
        assert_eq!(s.band.rf_gain_db, RF_GAIN_MAX_DB);
        assert_eq!(s.band.filter_width_hz, FILTER_WIDTH_MAX);
        assert_eq!(s.playback.extra_word_space_multiplier, EXTRA_WORD_SPACE_MIN);
        assert_eq!(s.auto_level.auto_adjust_threshold, 100.0);
        assert_eq!(s.auto_level.error_weight_strength, 0.0);
        assert_eq!(s.auto_level.char_sampling_coverage_strength, 0.0);
        assert_eq!(
            s.auto_level.auto_adjust_above_threshold_count,
            ADJUST_COUNT_MAX
        );
        assert_eq!(
            s.curriculum.callsign_level,
            crate::callsign::CALLSIGN_TIER_MAX
        );
        assert_eq!(s.contest.contest_minutes, CONTEST_MINUTES_MAX);
        assert_eq!(s.contest.my_call, "DL1/ABC/QRP");
        assert_eq!(s.contest.my_name, "JEANLUCMAX");
        assert_eq!(s.contest.my_state, "QC");
        assert_eq!(s.contest.my_zone, super::contest::MY_ZONE_MAX);
        assert_eq!(s.contest.my_power, "100");
        assert_eq!(s.contest.my_class, "3A");
        assert_eq!(s.contest.my_section, "");
        assert_eq!(s.contest.my_wpm, MY_WPM_MAX);
        assert_eq!(s.contest.my_sending_db, MY_SENDING_MAX_DB);

        // The strengths have tops as well as bottoms, the ones the settings
        // screen offers.
        let mut strong = TrainingSettings::default();
        strong.playback.extra_word_space_multiplier = 90.0;
        strong.auto_level.error_weight_strength = 90.0;
        strong.auto_level.char_sampling_coverage_strength = 90.0;
        let strong = strong.clamp();
        assert_eq!(
            strong.playback.extra_word_space_multiplier,
            EXTRA_WORD_SPACE_MAX
        );
        assert_eq!(strong.auto_level.error_weight_strength, ERROR_WEIGHT_MAX);
        assert_eq!(
            strong.auto_level.char_sampling_coverage_strength,
            COVERAGE_STRENGTH_MAX
        );
    }

    #[test]
    fn linked_ranges_collapse_onto_their_lower_bound() {
        let mut s = TrainingSettings::default();
        s.playback.link_char_wpm = true;
        s.playback.link_effective_wpm = true;
        s.playback.link_char_to_effective = false;
        s.curriculum.link_group_size = true;
        s.playback.link_group_repeat = true;
        s.band.link_volume = true;
        s.playback.char_wpm_min = 22.0;
        s.playback.char_wpm_max = 30.0;
        s.playback.effective_wpm_min = 12.0;
        s.playback.effective_wpm_max = 19.0;
        s.curriculum.min_group_size = 4;
        s.curriculum.max_group_size = 9;
        s.playback.group_repeat_min = 2;
        s.playback.group_repeat_max = 5;
        s.band.volume_min = 0.4;
        s.band.volume_max = 0.9;
        let s = s.clamp();
        assert_eq!(s.playback.char_wpm_max, 22.0);
        assert_eq!(s.playback.effective_wpm_max, 12.0);
        assert_eq!(s.curriculum.max_group_size, 4);
        assert_eq!(s.playback.group_repeat_max, 2);
        assert_eq!(s.band.volume_max, 0.4);
    }

    /// Tied, the effective range is the character range, link and all: an
    /// effective link left over from before the tie must not collapse one
    /// copy of the range while the other stays open.
    #[test]
    fn a_tied_effective_speed_takes_the_whole_character_range() {
        for link_char in [false, true] {
            let mut s = TrainingSettings::default();
            s.playback.link_char_to_effective = true;
            s.playback.link_char_wpm = link_char;
            s.playback.link_effective_wpm = !link_char;
            s.playback.char_wpm_min = 15.0;
            s.playback.char_wpm_max = 28.0;
            let s = s.clamp();
            assert_eq!(
                (s.playback.effective_wpm_min, s.playback.effective_wpm_max),
                (s.playback.char_wpm_min, s.playback.char_wpm_max),
                "char link {link_char}"
            );
            assert_eq!(s.playback.link_effective_wpm, s.playback.link_char_wpm);
            assert_eq!(s.clone().clamp(), s);
        }
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        assert_eq!(CharSetMode::default(), CharSetMode::Mixed);
        assert_eq!(
            crate::keyer::KeyerMode::default(),
            crate::keyer::KeyerMode::IambicB
        );
        assert_eq!(FilterShape::default(), FilterShape::Sharp);
        let s = TrainingSettings::default();
        let range = |min, max, linked| RangeValues { min, max, linked };
        assert_eq!(s.range(RangeSetting::CharWpm), range(15.0, 30.0, false));
        assert_eq!(
            s.range(RangeSetting::EffectiveWpm),
            range(15.0, 30.0, false)
        );
        assert_eq!(s.range(RangeSetting::GroupSize), range(2.0, 3.0, false));
        assert_eq!(s.range(RangeSetting::SideTone), range(300.0, 600.0, false));
        assert_eq!(s.range(RangeSetting::Stations), range(1.0, 1.0, true));
        assert_eq!(s.playback.keyer_mode, crate::keyer::KeyerMode::IambicB);
        assert_eq!(s.playback.group_pause_sec, 0.0);
        assert_eq!(s.auto_level.auto_adjust_above_threshold_count, 2);
        assert_eq!(s.auto_level.auto_adjust_below_threshold_count, 1);
        assert_eq!(s.clone().clamp(), s, "the defaults are in bounds");
        assert_eq!(MixedAutoLevelAxis::default(), MixedAutoLevelAxis::Letters);
        assert_eq!(
            MixedAutoLevelAxis::Letters.flip(),
            MixedAutoLevelAxis::Digits
        );
        assert_eq!(
            MixedAutoLevelAxis::Digits.flip(),
            MixedAutoLevelAxis::Letters
        );
    }

    #[test]
    fn the_active_level_follows_the_character_set() {
        let mut s = TrainingSettings::default();
        s.curriculum.level = 4;
        s.curriculum.digits_level = 6;

        s.curriculum.char_set_mode = CharSetMode::Digits;
        assert_eq!(s.active_level(), 6);
        assert_eq!(s.alphabet(LevelAxis::Digits), crate::morse::DIGITS.to_vec());
        assert_eq!(s.max_active_level(), 9);
        s.set_active_level(3);
        assert_eq!(s.curriculum.digits_level, 3);
        assert_eq!(s.curriculum.level, 4);

        s.curriculum.char_set_mode = CharSetMode::Koch;
        assert_eq!(s.active_level(), 4);
        assert_eq!(s.max_active_level(), s.max_level(LevelAxis::Letters));
        assert_eq!(s.alphabet(LevelAxis::Letters), s.progress_alphabet());
        s.set_active_level(7);
        assert_eq!(s.curriculum.level, 7);
    }

    /// The modes that send callsigns are the ones the callsign tier shapes.
    #[test]
    fn callsigns_are_sent_where_the_callsign_tier_shapes_them() {
        for mode in CharSetMode::ALL {
            assert_eq!(
                mode.sends_callsigns(),
                mode.axis() == LevelAxis::Callsign,
                "{mode:?}"
            );
        }
        assert!(CharSetMode::Contest.sends_callsigns());
    }

    #[test]
    fn the_fingerprint_identifies_the_alphabet_being_trained() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Digits;
        assert_eq!(s.alphabet_fingerprint(), "0123456789");
        s.curriculum.char_set_mode = CharSetMode::Koch;
        assert!(s.alphabet_fingerprint().starts_with("KMURE"));
        // Mixed trains letters on the letter axis, so digits are not in it.
        s.curriculum.char_set_mode = CharSetMode::Mixed;
        assert!(!s.alphabet_fingerprint().contains('5'));
        // A different sequence is a different alphabet.
        s.curriculum.custom_sequence = vec!['A', 'B'];
        assert_eq!(s.alphabet_fingerprint(), "AB");
    }

    #[test]
    fn the_side_tone_centre_is_the_middle_of_the_range() {
        let mut s = TrainingSettings::default();
        s.band.side_tone_min = 400.0;
        s.band.side_tone_max = 600.0;
        assert_eq!(s.side_tone_center(), 500.0);
        // A reversed pair still reports its lower bound rather than a negative span.
        s.band.side_tone_max = 100.0;
        assert_eq!(s.side_tone_center(), 400.0);
    }

    #[test]
    fn opening_a_fixed_side_tone_spreads_it_without_leaving_the_dial() {
        let mut s = TrainingSettings::default();
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        assert!(s.range(RangeSetting::SideTone).linked);
        s.set_range_linked(RangeSetting::SideTone, false);
        assert_eq!(s.band.side_tone_min, 600.0);
        assert_eq!(s.band.side_tone_max, 800.0);

        // At the top of the dial the pair spreads downwards instead.
        s.band.side_tone_min = 1200.0;
        s.band.side_tone_max = 1200.0;
        s.set_range_linked(RangeSetting::SideTone, false);
        assert_eq!(s.band.side_tone_max, 1200.0);
        assert_eq!(s.band.side_tone_min, 1000.0);

        // Collapsing sends both ends to the lower bound.
        s.set_range_linked(RangeSetting::SideTone, true);
        assert_eq!(s.band.side_tone_max, 1000.0);
    }

    #[test]
    fn every_range_can_be_read_moved_and_linked() {
        let cases = [
            (RangeSetting::CharWpm, 20.0, 30.0),
            (RangeSetting::EffectiveWpm, 10.0, 15.0),
            (RangeSetting::GroupSize, 3.0, 6.0),
            (RangeSetting::GroupRepeat, 2.0, 4.0),
            (RangeSetting::SideTone, 400.0, 700.0),
            (RangeSetting::Volume, 0.3, 0.8),
        ];
        for (which, low, high) in cases {
            let mut s = TrainingSettings::default();
            s.playback.link_char_to_effective = false;
            s.set_range_linked(which, false);
            s.set_range_min(which, low);
            s.set_range_max(which, high);
            let range = s.range(which);
            assert_eq!((range.min, range.max), (low, high), "{which:?}");

            // Pushing the lower bound past the upper one drags it along.
            s.set_range_min(which, high + 1.0);
            assert_eq!(s.range(which).max, high + 1.0, "{which:?}");
            // And the other way round.
            s.set_range_max(which, low);
            assert_eq!(s.range(which).min, low, "{which:?}");

            // Linked, both ends move together.
            s.set_range_linked(which, true);
            let range = s.range(which);
            assert_eq!(range.min, range.max, "{which:?}");
            s.set_range_min(which, high);
            assert_eq!(s.range(which).max, high, "{which:?}");
        }
    }

    #[test]
    fn character_speed_drags_effective_speed_when_they_are_tied() {
        let mut s = TrainingSettings::default();
        s.playback.link_char_to_effective = true;
        s.set_range_min(RangeSetting::CharWpm, 25.0);
        assert_eq!(s.playback.effective_wpm_min, 25.0);
        s.set_range_max(RangeSetting::CharWpm, 33.0);
        assert_eq!(s.playback.effective_wpm_max, 33.0);

        // Untied, effective speed keeps its own value.
        s.playback.link_char_to_effective = false;
        s.set_range_min(RangeSetting::EffectiveWpm, 9.0);
        s.set_range_min(RangeSetting::CharWpm, 28.0);
        assert_eq!(s.playback.effective_wpm_min, 9.0);
    }

    #[test]
    fn switching_the_character_set_resets_the_practice_window() {
        let mut s = TrainingSettings::default();
        s.curriculum.practice_window = PracticeWindow::Last3;
        s.set_char_set_mode(CharSetMode::Digits);
        assert_eq!(s.curriculum.char_set_mode, CharSetMode::Digits);
        assert_eq!(s.curriculum.practice_window, PracticeWindow::All);
    }

    /// Every save written before the pile-up became a range carries
    /// `stationsMax` and no floor. Those have to load, and to mean what they
    /// meant when they were written: up to that many, from one.
    #[test]
    fn a_save_from_before_the_range_still_means_what_it_said() {
        // The sections are flattened, so a save is one flat object.
        let older = serde_json::json!({ "stationsMax": 4, "filterBandwidthHz": 500.0 });
        let settings: TrainingSettings =
            serde_json::from_value(older).expect("an older save should load");
        let settings = settings.clamp();
        assert_eq!(settings.band.stations_min, STATIONS_MIN);
        assert_eq!(settings.band.stations_max, 4);
        assert!(!settings.range(RangeSetting::Stations).linked);
    }

    /// The width used to be stored as `filterBandwidthHz`, a width at 3 dB,
    /// and nearly every save holds the old 500 Hz default there rather than a
    /// choice. It is not read: such a save comes back at today's default
    /// width. The shape is read by name, the peaked one included, and a name
    /// this build does not know comes back as the plain sharp filter rather
    /// than costing the save.
    #[test]
    fn an_old_filter_width_is_not_carried_over() {
        let older = serde_json::json!({ "filterBandwidthHz": 500.0, "filterShape": "sharp" });
        let settings: TrainingSettings =
            serde_json::from_value(older).expect("an older save should load");
        assert_eq!(settings.band.filter_width_hz, DEFAULT_FILTER_WIDTH_HZ);
        assert_eq!(settings.band.filter_shape, FilterShape::Sharp);

        let mut peaked = TrainingSettings::default();
        peaked.band.filter_shape = FilterShape::Peaked;
        peaked.band.filter_width_hz = 450.0;
        let json = serde_json::to_value(&peaked).expect("settings serialize");
        assert_eq!(json["filterShape"], "peaked");
        assert_eq!(json["filterWidthHz"], 450.0);
        let back: TrainingSettings = serde_json::from_value(json).expect("and load back");
        assert_eq!(back.band.filter_shape, FilterShape::Peaked);
        assert_eq!(back.band.filter_width_hz, 450.0);

        for unknown in ["notch", "", "SHARP"] {
            let save = serde_json::json!({ "filterShape": unknown, "stationsMax": 3 });
            let settings: TrainingSettings =
                serde_json::from_value(save).expect("an unknown shape should not cost the save");
            assert_eq!(
                settings.band.filter_shape,
                FilterShape::Sharp,
                "{unknown:?}"
            );
            assert_eq!(settings.band.stations_max, 3);
        }
    }

    /// The AGC's decay is saved by name, loads at Fast from a save from
    /// before it, and a name this build does not know comes back as Fast
    /// rather than costing the save.
    #[test]
    fn the_agc_decay_is_saved_by_name() {
        let mut slow = TrainingSettings::default();
        assert_eq!(slow.band.agc_decay, AgcDecay::Fast);
        slow.band.agc_decay = AgcDecay::Slow;
        let json = serde_json::to_value(&slow).expect("settings serialize");
        assert_eq!(json["agcDecay"], "slow");
        let back: TrainingSettings = serde_json::from_value(json).expect("and load back");
        assert_eq!(back.band.agc_decay, AgcDecay::Slow);
        for save in [
            serde_json::json!({ "stationsMax": 3 }),
            serde_json::json!({ "agcDecay": "medium", "stationsMax": 3 }),
        ] {
            let settings: TrainingSettings = serde_json::from_value(save).expect("loads");
            assert_eq!(settings.band.agc_decay, AgcDecay::Fast);
            assert_eq!(settings.band.stations_max, 3);
        }
    }

    /// Dragging either end past the other takes the other with it, so the
    /// range can never be stored back to front.
    #[test]
    fn the_ends_of_the_station_range_push_each_other() {
        let mut s = TrainingSettings::default();
        s.band.stations_min = 2;
        s.band.stations_max = 4;

        // The floor pushed above the ceiling carries it up.
        s.set_range_min(RangeSetting::Stations, 5.0);
        assert_eq!((s.band.stations_min, s.band.stations_max), (5, 5));

        // And the ceiling pulled below the floor carries it down.
        s.set_range_max(RangeSetting::Stations, 2.0);
        assert_eq!((s.band.stations_min, s.band.stations_max), (2, 2));

        // Held together, both ends move as one.
        s.set_range_linked(RangeSetting::Stations, true);
        s.set_range_max(RangeSetting::Stations, 4.0);
        assert_eq!((s.band.stations_min, s.band.stations_max), (4, 4));
        assert!(s.range(RangeSetting::Stations).linked);
    }

    /// The Fixed/Random button has to actually do something. Neither the side
    /// tone nor the station count stores a link flag, so opening one up means
    /// pushing its bounds apart — miss that and the button is dead.
    #[test]
    fn opening_the_station_range_back_up_gives_it_room() {
        for start in STATIONS_MIN..=STATIONS_MAX {
            let mut s = TrainingSettings::default();
            s.band.stations_min = start;
            s.band.stations_max = start;
            assert!(s.range(RangeSetting::Stations).linked);

            s.set_range_linked(RangeSetting::Stations, false);
            let range = s.range(RangeSetting::Stations);
            assert!(
                !range.linked,
                "unlinking at {start} left the ends together at {}",
                range.min
            );
            assert_eq!(s.clone().clamp(), s, "unlinking left settings to repair");
            assert!((STATIONS_MIN..=STATIONS_MAX).contains(&s.band.stations_max));

            // And closing it again pins it to the floor.
            s.set_range_linked(RangeSetting::Stations, true);
            assert!(s.range(RangeSetting::Stations).linked);
        }
    }

    /// The reset buttons have to cover everything between them, or one card
    /// keeps a setting nobody can put back.
    ///
    /// Rather than restate the field lists here — which would only be the same
    /// mistake written twice — this walks a fully-changed settings object
    /// through every section's reset and checks what is left. Anything still
    /// altered is a preference no card owns. Add a field, forget to place it,
    /// and this names it.
    #[test]
    fn every_setting_belongs_to_exactly_one_section() {
        // Progress, not preference: where you have got to, and what the
        // sampler has learned. No reset button touches these.
        const PROGRESS: &[&str] = &["level", "digitsLevel", "callsignLevel"];

        let mut changed = wildly_different();
        for section in SettingsSection::ALL {
            changed.reset_section(section);
        }

        let after = serde_json::to_value(&changed).expect("serialize");
        let fresh = serde_json::to_value(TrainingSettings::default()).expect("serialize");
        let (after, fresh) = (
            after.as_object().expect("an object"),
            fresh.as_object().expect("an object"),
        );

        let orphans: Vec<&String> = after
            .iter()
            .filter(|(key, value)| {
                !PROGRESS.contains(&key.as_str()) && fresh.get(*key) != Some(*value)
            })
            .map(|(key, _)| key)
            .collect();
        assert!(
            orphans.is_empty(),
            "no section resets these, so nothing can put them back: {orphans:?}"
        );
    }

    /// And a reset stays inside its own card: changing everything, then
    /// resetting one section, must leave the other sections changed.
    #[test]
    fn resetting_one_section_leaves_the_others_alone() {
        for section in SettingsSection::ALL {
            let mut one = wildly_different();
            one.reset_section(section);

            let mut all = wildly_different();
            for other in SettingsSection::ALL {
                all.reset_section(other);
            }
            assert_ne!(
                one, all,
                "{section:?} on its own put everything back — it is too broad"
            );
            assert_ne!(
                one,
                wildly_different(),
                "{section:?} changed nothing at all"
            );
        }
    }

    /// The aim is shared by group size and speed, and shown by one card at a
    /// time: that card's reset puts it back, and the other card's leaves it.
    #[test]
    fn the_aim_goes_back_with_the_card_that_shows_it() {
        let mut speed_only = TrainingSettings::default();
        speed_only.playback.adaptive_speed = true;
        speed_only.playback.char_wpm_min = 18.0;
        speed_only.playback.char_wpm_max = 30.0;
        speed_only.playback.link_char_wpm = false;
        speed_only.curriculum.aim = Aim::Stretch;
        let mut both = speed_only.clone();
        both.curriculum.adaptive_group_size = true;

        let reset = |settings: &TrainingSettings, section| {
            let mut settings = settings.clone();
            settings.reset_section(section);
            settings.curriculum.aim
        };
        assert_eq!(reset(&speed_only, SettingsSection::Speed), Aim::Balanced);
        assert_eq!(
            reset(&speed_only, SettingsSection::SessionShape),
            Aim::Stretch
        );
        assert_eq!(reset(&both, SettingsSection::Speed), Aim::Stretch);
        assert_eq!(reset(&both, SettingsSection::SessionShape), Aim::Balanced);
    }

    /// Settings with every stored preference moved off its default, so a reset
    /// has something to undo whichever section it is asked about.
    fn wildly_different() -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.curriculum.num_groups = 7;
        s.curriculum.min_group_size = 2;
        s.curriculum.max_group_size = 9;
        s.curriculum.link_group_size = true;
        s.curriculum.adaptive_group_size = true;
        s.curriculum.aim = Aim::Stretch;
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.mixed_letters_percent = 33;
        s.curriculum.custom_sequence = vec!['Z', 'Q'];
        s.curriculum.sequence_is_custom = true;
        s.curriculum.practice_window = PracticeWindow::Last3;
        s.playback.char_wpm_min = 31.0;
        s.playback.char_wpm_max = 33.0;
        s.playback.link_char_wpm = true;
        s.playback.effective_wpm_min = 11.0;
        s.playback.effective_wpm_max = 13.0;
        s.playback.link_effective_wpm = true;
        s.playback.link_char_to_effective = false;
        s.playback.extra_word_space_multiplier = 2.5;
        s.playback.group_timeout = 9_000.0;
        s.playback.group_pause_sec = 6.0;
        s.playback.lock_input_during_group_playback = false;
        s.playback.paddle_swap = true;
        s.playback.keyer_mode = KeyerMode::Ultimatic;
        s.playback.keyer_wpm = 32.0;
        s.playback.group_repeat_min = 2;
        s.playback.group_repeat_max = 4;
        s.playback.link_group_repeat = true;
        s.band.side_tone_min = 700.0;
        s.band.side_tone_max = 900.0;
        s.band.volume_min = 0.3;
        s.band.volume_max = 0.6;
        s.band.link_volume = true;
        s.band.master_volume = 0.5;
        s.band.steepness = 7.0;
        s.band.envelope_smoothing = 0.7;
        s.band.qsb_enabled = false;
        s.band.qsb_depth = 0.9;
        s.band.qsb_rate_hz = 0.9;
        s.band.qrn_enabled = false;
        s.band.qrn_level = 0.9;
        s.band.noise_enabled = false;
        s.band.noise_level = 0.9;
        s.band.agc_enabled = false;
        s.band.agc_threshold_db = 12.0;
        s.band.agc_decay = AgcDecay::Slow;
        s.band.rf_gain_db = -12.0;
        s.band.filter_width_hz = 250.0;
        s.band.filter_shape = FilterShape::Soft;
        s.band.stations_min = 2;
        s.band.stations_max = 4;
        s.band.pileup_spread_hz = 400.0;
        s.band.pileup_level_db = 9.0;
        s.auto_level.auto_adjust_level = false;
        s.auto_level.auto_adjust_threshold = 77.0;
        s.auto_level.auto_adjust_below_threshold_count = 4;
        s.auto_level.auto_adjust_above_threshold_count = 9;
        s.auto_level.mixed_auto_level_next_axis = MixedAutoLevelAxis::Digits;
        s.auto_level.error_weight_strength = 0.9;
        s.auto_level.char_sampling_coverage_strength = 0.9;
        s.auto_level.char_sampling_thompson = false;
        s.contest.contest_kind = crate::contest::ContestKind::Sprint;
        s.contest.contest_style = crate::contest::RunStyle::Pileup;
        s.contest.contest_habits = crate::contest::CallerHabits::Real;
        s.contest.contest_flutter = true;
        s.contest.contest_minutes = 25;
        s.contest.contest_show_copy = true;
        s.contest.contest_tu_call = crate::contest::TuCall::Every;
        s.contest.contest_adapt_speed = false;
        s.contest.my_wpm = 34;
        s.contest.my_sending_db = -21;
        s.contest.contest_qsk = true;
        s.contest.my_call = "DL1ABC".to_string();
        s.contest.my_name = "MAT".to_string();
        s.contest.my_state = "MA".to_string();
        s.contest.my_zone = 15;
        s.contest.my_power = "KW".to_string();
        s.contest.my_class = "3A".to_string();
        s.contest.my_section = "EMA".to_string();
        s
    }

    /// Your callsign is kept the way a callsign can be written, and a run
    /// is never longer than the longest.
    #[test]
    fn clamping_keeps_your_call_a_call_and_a_run_inside_its_hour() {
        let mut s = TrainingSettings::default();
        s.contest.my_call = " k1abc/ ".to_string();
        s.contest.contest_minutes = 61;
        let s = s.clamp();
        assert_eq!(s.contest.my_call, "K1ABC");
        assert_eq!(s.contest.contest_minutes, CONTEST_MINUTES_MAX);
        let mut open = TrainingSettings::default();
        open.contest.contest_minutes = 0;
        assert_eq!(
            open.clamp().contest.contest_minutes,
            0,
            "0 is a run left open"
        );
        assert!(s.contest.my_call.len() <= MY_CALL_MAX);
    }

    /// Your state, zone, power, class and section are Your station's: its
    /// reset puts them back with your call and name, and no other section's
    /// touches them.
    #[test]
    fn your_state_zone_power_class_and_section_belong_to_your_station() {
        for section in SettingsSection::ALL {
            let mut s = wildly_different();
            s.reset_section(section);
            let cleared = section == SettingsSection::Station;
            assert_eq!(s.contest.my_state.is_empty(), cleared, "{section:?}");
            assert_eq!(s.contest.my_zone == 0, cleared, "{section:?}");
            assert_eq!(s.contest.my_power == "100", cleared, "{section:?}");
            assert_eq!(s.contest.my_class == "1D", cleared, "{section:?}");
            assert_eq!(s.contest.my_section.is_empty(), cleared, "{section:?}");
        }
    }

    /// The callers' habits are the Band card's: its reset puts them back
    /// to easy, and no other section's touches them.
    #[test]
    fn the_callers_habits_belong_to_the_band() {
        for section in SettingsSection::ALL {
            let mut s = wildly_different();
            s.reset_section(section);
            let easy = s.contest.contest_habits == crate::contest::CallerHabits::Easy;
            assert_eq!(
                easy,
                section == SettingsSection::BandConditions,
                "{section:?}"
            );
        }
    }

    /// Flutter is the Band card's, beside the callers' habits, and full
    /// break-in Tone and volume's, beside your sending: each card's reset
    /// turns its own off, and no other's touches it.
    #[test]
    fn flutter_and_full_break_in_belong_to_their_cards() {
        for section in SettingsSection::ALL {
            let mut s = wildly_different();
            s.reset_section(section);
            assert_eq!(
                !s.contest.contest_flutter,
                section == SettingsSection::BandConditions,
                "{section:?}"
            );
            assert_eq!(
                !s.contest.contest_qsk,
                section == SettingsSection::ToneAndVolume,
                "{section:?}"
            );
        }
    }

    /// Show copy is Session shape's: its reset turns it off, and no other
    /// section's touches it.
    #[test]
    fn show_copy_belongs_to_the_session_shape() {
        for section in SettingsSection::ALL {
            let mut s = wildly_different();
            s.reset_section(section);
            assert_eq!(
                !s.contest.contest_show_copy,
                section == SettingsSection::SessionShape,
                "{section:?}"
            );
        }
    }

    /// Your speed is the Speed card's, your sending Tone and volume's, and
    /// your call in TU the contest card's, the Character set's: each
    /// card's reset puts its own back, and no other's touches it.
    #[test]
    fn your_speed_sending_and_tu_call_belong_to_their_cards() {
        for section in SettingsSection::ALL {
            let mut s = wildly_different();
            s.reset_section(section);
            assert_eq!(
                s.contest.my_wpm == 0,
                section == SettingsSection::Speed,
                "{section:?}"
            );
            assert_eq!(
                s.contest.my_sending_db == -3,
                section == SettingsSection::ToneAndVolume,
                "{section:?}"
            );
            assert_eq!(
                s.contest.contest_tu_call == crate::contest::TuCall::Third,
                section == SettingsSection::CharacterSet,
                "{section:?}"
            );
        }
    }

    /// The callers' adaptive speed is the Speed card's, beside group
    /// training's: its reset turns it back on, and no other section's
    /// touches it, nor group training's switch.
    #[test]
    fn the_callers_adaptive_speed_belongs_to_the_speed_card() {
        for section in SettingsSection::ALL {
            let mut s = wildly_different();
            s.reset_section(section);
            assert_eq!(
                s.contest.contest_adapt_speed,
                section == SettingsSection::Speed,
                "{section:?}"
            );
        }
        let mut s = TrainingSettings::default();
        assert!(s.contest.contest_adapt_speed, "on as it ships");
        assert!(!s.playback.adaptive_speed, "group training's stays off");
        s.contest.contest_adapt_speed = false;
        s.playback.adaptive_speed = true;
        let s = s.clamp();
        assert!(!s.contest.contest_adapt_speed && s.playback.adaptive_speed);
        // In Contest the aim shows beside the switch while the callers
        // adapt: the reset that turns the switch back on brings the aim onto
        // the card it resets, so it puts the aim back too; Session shape's
        // reset leaves an aim it does not show.
        let mut contest = TrainingSettings::default();
        contest.curriculum.char_set_mode = CharSetMode::Contest;
        contest.curriculum.aim = Aim::Stretch;
        contest.contest.contest_adapt_speed = false;
        let contest = contest.clamp();
        let mut shape = contest.clone();
        shape.contest.contest_adapt_speed = true;
        shape.reset_section(SettingsSection::SessionShape);
        assert_eq!(shape.curriculum.aim, Aim::Stretch, "not Session shape's");
        let mut speed = contest;
        speed.reset_section(SettingsSection::Speed);
        assert!(speed.contest.contest_adapt_speed);
        assert_eq!(crate::sampling::aim_section(&speed), SettingsSection::Speed);
        assert_eq!(speed.curriculum.aim, Aim::Balanced, "the Speed card's");
    }

    /// Your speed is a speed of your own inside the Speed card's bounds, or
    /// none; your sending runs from silent to the loudest.
    #[test]
    fn clamping_keeps_your_speed_and_sending_in_their_ranges() {
        let clamped = |wpm: u32, db: i32| {
            let mut s = TrainingSettings::default();
            s.contest.my_wpm = wpm;
            s.contest.my_sending_db = db;
            let s = s.clamp();
            (s.contest.my_wpm, s.contest.my_sending_db)
        };
        assert_eq!(clamped(0, -9), (0, -9), "no speed of your own");
        assert_eq!(clamped(1, -61), (MY_WPM_MIN, -60));
        assert_eq!(clamped(33, -60), (33, -60));
        assert_eq!(clamped(81, 1), (MY_WPM_MAX, 0));
        assert_eq!(clamped(u32::MAX, i32::MIN), (80, -60));
    }

    #[test]
    fn contest_settings_default_to_ten_minutes_of_wpx_with_no_call() {
        let s = TrainingSettings::default();
        assert_eq!(s.contest.contest_kind, crate::contest::ContestKind::Wpx);
        assert_eq!(s.contest.contest_minutes, 10);
        assert!(s.contest.my_call.is_empty());
        let json = serde_json::to_value(&s).expect("serialize");
        assert_eq!(json["contestKind"], "wpx");
        assert_eq!(json["contestStyle"], "single");
        assert_eq!(json["contestHabits"], "easy");
        assert_eq!(json["contestFlutter"], false);
        assert_eq!(json["contestMinutes"], 10);
        assert_eq!(json["contestShowCopy"], false);
        assert_eq!(json["contestTuCall"], "third");
        assert_eq!(json["contestAdaptSpeed"], true);
        assert_eq!(json["myWpm"], 0);
        assert_eq!(json["mySendingDb"], -3);
        assert_eq!(json["contestQsk"], false);
        assert_eq!(json["myCall"], "");
        assert_eq!(json["myName"], "");
        assert_eq!(json["myState"], "");
        assert_eq!(json["myZone"], 0);
        assert_eq!(json["myPower"], "100");
        assert_eq!(json["myClass"], "1D");
        assert_eq!(json["mySection"], "");
        assert!(s.contest.my_name.is_empty());
        assert!(json.get("contest").is_none(), "flattened");
    }
}
